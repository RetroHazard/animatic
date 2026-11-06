//! The cover thread: shows an anime's cover from the cache, or downloads it once.
//!
//! Rule 1 and 8: nothing here runs on the UI thread, and the only request it can make goes
//! through the process-wide limiter (the `Arc<Limits>` the worker also gives its queue runs).
//! Which request may be sent is decided by `animatic_core::covers::plan_cover`: never offline,
//! never with downloads switched off, never during a ban or cooldown, never for a cover that is
//! cached, and not again within an hour of a failed try. Decoding happens here too, so the
//! window only uploads finished pixels.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use animatic_anidb::error::AnidbError;
use animatic_anidb::http::{HttpClient, MAX_IMAGE_BYTES};
use animatic_anidb::limits::Limits;
use animatic_anidb::{HTTP_CLIENT, HTTP_CLIENT_VERSION};
use animatic_core::covers::{
    CoverError, CoverPlan, CoverRequest, covers_dir, discard_cover, download_cover, plan_cover,
};
use animatic_core::db::Db;
use animatic_core::format;
use crossbeam_channel::{Receiver, Sender};

use super::{Event, start_cooldown};

/// Longest side of a decoded cover. Larger pictures are scaled down on this thread; a window
/// never shows more than this many pixels of a cover.
pub const MAX_COVER_SIDE: u32 = 1024;

/// Widest or tallest picture the decoder agrees to read, and the memory it may use for one.
const MAX_DECODE_SIDE: u32 = 16_384;
const MAX_DECODE_ALLOC: u64 = 256 << 20;

/// One request for a cover.
#[derive(Debug, Clone)]
pub struct CoverJob {
    pub aid: i64,
    pub picname: String,
    /// `anidb.image_server`: used until a login announced the host to use.
    pub image_server: String,
    /// `anidb.download_covers`.
    pub download: bool,
    /// `anidb.offline`.
    pub offline: bool,
}

/// A decoded cover: straight (non-premultiplied) RGBA, at most [`MAX_COVER_SIDE`] on a side.
pub struct DecodedCover {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for DecodedCover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DecodedCover({}x{})", self.width, self.height)
    }
}

/// The answer to one [`CoverJob`].
#[derive(Debug, Clone)]
pub enum CoverOutcome {
    /// The picture, ready to upload. `note` says what went wrong around it (for example that
    /// it could not be cached).
    Ready {
        image: Arc<DecodedCover>,
        note: Option<String>,
    },
    /// There is no picture to show, and why.
    Unavailable(String),
    /// A newer request was waiting, so nothing was sent for this one; ask again if it is still
    /// wanted.
    Superseded,
}

pub(super) enum CoverMsg {
    Load(CoverJob),
    Stop,
}

/// Decode a picture into RGBA pixels, scaled to fit [`MAX_COVER_SIDE`]. Only JPEG, PNG, WebP
/// and GIF (first frame) are read, with limits on the dimensions and the memory used, so a
/// hostile file cannot exhaust either.
pub fn decode_cover(bytes: &[u8]) -> Result<DecodedCover, String> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let mut picture = reader.decode().map_err(|e| e.to_string())?;
    if picture.width() > MAX_COVER_SIDE || picture.height() > MAX_COVER_SIDE {
        picture = picture.resize(
            MAX_COVER_SIDE,
            MAX_COVER_SIDE,
            image::imageops::FilterType::Triangle,
        );
    }
    let rgba = picture.into_rgba8();
    Ok(DecodedCover {
        width: rgba.width() as usize,
        height: rgba.height() as usize,
        rgba: rgba.into_raw(),
    })
}

/// Read and decode a cached cover. The size is checked first so a swapped-in giant file is not
/// read whole.
fn decode_file(path: &Path) -> Result<DecodedCover, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_IMAGE_BYTES {
        return Err(format!("the file is larger than {MAX_IMAGE_BYTES} bytes"));
    }
    decode_cover(&std::fs::read(path).map_err(|e| e.to_string())?)
}

pub(super) fn run_covers(
    db_path: PathBuf,
    messages: Receiver<CoverMsg>,
    events: Sender<Event>,
    limits: Arc<Limits>,
    shutdown: Arc<AtomicBool>,
) {
    while let Ok(CoverMsg::Load(job)) = messages.recv() {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        let env = CoverEnv {
            db_path: &db_path,
            limits: &limits,
            // A newer request is waiting: this one is not sent (see `load_cover`).
            superseded: &|| !messages.is_empty(),
            #[cfg(test)]
            image_base: None,
        };
        let outcome = load_cover(&env, &job, &events);
        let picname = job.picname;
        let _ = events.send(Event::CoverLoaded { picname, outcome });
    }
}

pub(super) struct CoverEnv<'a> {
    pub db_path: &'a Path,
    pub limits: &'a Arc<Limits>,
    /// True when a newer request is waiting. Checked just before a download starts, so a user
    /// who clicks through many anime does not queue one download per click: only the cover
    /// they stopped on is fetched.
    pub superseded: &'a dyn Fn() -> bool,
    /// Loopback image server for tests.
    #[cfg(test)]
    pub image_base: Option<&'a str>,
}

/// Produce the outcome for one job. Sends at most one HTTP request, never on the UI thread.
pub(super) fn load_cover(
    env: &CoverEnv<'_>,
    job: &CoverJob,
    events: &Sender<Event>,
) -> CoverOutcome {
    let db = match Db::open(env.db_path) {
        Ok(db) => db,
        Err(e) => return CoverOutcome::Unavailable(format!("Cover not available: {e}")),
    };
    let dir = covers_dir(env.db_path);
    let request = CoverRequest {
        aid: job.aid,
        picname: &job.picname,
        image_server_setting: &job.image_server,
        download: job.download,
        offline: job.offline,
    };
    let mut note = None;
    // At most twice: a cached file that is not a picture is removed and fetched again.
    for _ in 0..2 {
        let plan = match plan_cover(&db, &dir, &request, format::now()) {
            Ok(plan) => plan,
            Err(e) => return CoverOutcome::Unavailable(format!("Cover not available: {e}")),
        };
        match plan {
            CoverPlan::Cached(path) => match decode_file(&path) {
                Ok(image) => {
                    return CoverOutcome::Ready {
                        image: Arc::new(image),
                        note: None,
                    };
                }
                Err(why) => {
                    log::warn!("cached cover {} is unreadable: {why}", path.display());
                    if let Err(e) = discard_cover(&dir, &job.picname) {
                        return CoverOutcome::Unavailable(format!(
                            "The cached cover cannot be read ({why}) or removed ({e})"
                        ));
                    }
                    note = Some(format!(
                        "The cached cover could not be read ({why}) and was removed"
                    ));
                }
            },
            CoverPlan::Skip(skip) => {
                return CoverOutcome::Unavailable(with_note(skip.to_string(), note));
            }
            CoverPlan::Download { host } => {
                return download(env, job, &db, &dir, &host, events, note);
            }
        }
    }
    CoverOutcome::Unavailable(with_note("Cover not available".into(), note))
}

fn with_note(message: String, note: Option<String>) -> String {
    match note {
        Some(note) => format!("{message} ({note})"),
        None => message,
    }
}

fn download(
    env: &CoverEnv<'_>,
    job: &CoverJob,
    db: &Db,
    dir: &Path,
    host: &str,
    events: &Sender<Event>,
    note: Option<String>,
) -> CoverOutcome {
    if (env.superseded)() {
        return CoverOutcome::Superseded;
    }
    #[allow(unused_mut)]
    let mut http = HttpClient::new(HTTP_CLIENT, HTTP_CLIENT_VERSION, env.limits.clone());
    #[cfg(test)]
    if let Some(base) = env.image_base {
        http = http.with_loopback_image_base(base);
    }
    let mut decoded = None;
    let result = download_cover(db, dir, &mut http, job.aid, &job.picname, host, |bytes| {
        decode_cover(bytes).map(|image| decoded = Some(image))
    });
    match (result, decoded) {
        (Ok(got), Some(image)) => CoverOutcome::Ready {
            image: Arc::new(image),
            note: got
                .cached
                .err()
                .map(|e| format!("The cover could not be saved for next time: {e}"))
                .or(note),
        },
        (Ok(_), None) => CoverOutcome::Unavailable("Cover not available".into()),
        (Err(CoverError::Rejected(why)), _) => CoverOutcome::Unavailable(with_note(
            format!("Cover not shown: the downloaded file is not a readable picture ({why})"),
            note,
        )),
        (Err(CoverError::Request(e)), _) => {
            // A ban or rate limit pauses every AniDB request, like one met by the queue.
            if matches!(e, AnidbError::Banned(_) | AnidbError::ServerUnavailable(_))
                && let Some(wait) = e.retry_after()
                && let Err(cooldown) = start_cooldown(db, wait, &e.to_string(), events)
            {
                log::warn!("could not save the cooldown after a cover request: {cooldown}");
            }
            CoverOutcome::Unavailable(with_note(format!("Cover not downloaded: {e}"), note))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use animatic_anidb::limits::Protocol;
    use animatic_anidb::testing::{HttpFixture, HttpReply, test_limits};
    use animatic_core::limits::RequestKind;
    use parking_lot::Mutex;

    use super::*;

    struct TempDb {
        dir: PathBuf,
        path: PathBuf,
    }

    impl TempDb {
        fn new() -> TempDb {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "animatic-cover-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            let path = dir.join("library.db");
            TempDb { dir, path }
        }

        fn open(&self) -> Db {
            Db::open(&self.path).unwrap()
        }

        fn covers(&self) -> PathBuf {
            covers_dir(&self.path)
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(width, height, image::Rgba([200, 30, 60, 255]))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn job(picname: &str) -> CoverJob {
        CoverJob {
            aid: 7,
            picname: picname.into(),
            image_server: "cdn.example.test".into(),
            download: true,
            offline: false,
        }
    }

    struct Rig {
        db: TempDb,
        limits: animatic_anidb::testing::TestLimits,
        events: (Sender<Event>, Receiver<Event>),
        fixture: HttpFixture,
        heads: Arc<Mutex<Vec<String>>>,
    }

    impl Rig {
        fn new(reply: impl Fn(usize) -> HttpReply + Send + 'static) -> Rig {
            let heads = Arc::new(Mutex::new(Vec::new()));
            let seen = heads.clone();
            Rig {
                db: TempDb::new(),
                limits: test_limits(),
                events: crossbeam_channel::unbounded(),
                fixture: HttpFixture::start(move |index, head| {
                    seen.lock().push(head.to_string());
                    reply(index)
                }),
                heads,
            }
        }

        fn load(&self, job: &CoverJob) -> CoverOutcome {
            self.load_with(job, &|| false)
        }

        fn load_with(&self, job: &CoverJob, superseded: &dyn Fn() -> bool) -> CoverOutcome {
            load_cover(
                &CoverEnv {
                    db_path: &self.db.path,
                    limits: &self.limits.limits,
                    superseded,
                    image_base: Some(self.fixture.base()),
                },
                job,
                &self.events.0,
            )
        }

        fn requests(&self) -> usize {
            self.fixture.requests()
        }

        fn limiter_sends(&self) -> usize {
            self.limits.store.sends(Protocol::Http).len()
        }
    }

    fn ready(outcome: &CoverOutcome) -> &DecodedCover {
        match outcome {
            CoverOutcome::Ready { image, .. } => image,
            other => panic!("expected a cover, got {other:?}"),
        }
    }

    fn unavailable(outcome: &CoverOutcome) -> &str {
        match outcome {
            CoverOutcome::Unavailable(why) => why,
            other => panic!("expected no cover, got {other:?}"),
        }
    }

    #[test]
    fn a_cover_is_downloaded_once_through_the_shared_limiter_and_then_served_from_disk() {
        let rig = Rig::new(|_| HttpReply::ok(png(40, 60)));
        // The queue (or any other client) just used the shared limiter.
        rig.limits.limits.wait_http().unwrap();
        let slept = rig.limits.clock.sleeps().len();

        let first = rig.load(&job("42.png"));
        let image = ready(&first);
        assert_eq!((image.width, image.height), (40, 60));
        assert_eq!(&image.rgba[..4], [200, 30, 60, 255]);
        assert_eq!(rig.requests(), 1);
        assert_eq!(
            rig.limiter_sends(),
            2,
            "the cover request took a limiter slot"
        );
        assert_eq!(
            rig.limits.clock.sleeps().len(),
            slept + 1,
            "and waited for it"
        );
        assert!(rig.db.covers().join("42.png").is_file());
        let head = rig.heads.lock()[0].to_ascii_lowercase();
        assert!(head.starts_with("get /images/main/42.png "), "{head}");
        assert!(!head.contains("referer"), "{head}");

        // Cached from now on: no request, whatever the settings say.
        for (offline, download) in [(false, true), (true, true), (false, false)] {
            let mut again = job("42.png");
            again.offline = offline;
            again.download = download;
            assert_eq!(ready(&rig.load(&again)).width, 40);
        }
        assert_eq!(rig.requests(), 1);
        assert_eq!(rig.limiter_sends(), 2);
        assert!(
            rig.db
                .open()
                .request_last_attempt(RequestKind::Cover, 7)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_new_picture_name_is_a_new_file_and_the_old_one_stays() {
        let rig = Rig::new(|index| HttpReply::ok(png(10 + index as u32, 10)));
        let mut first = job("1.png");
        ready(&rig.load(&first));
        // The anime got a new picture; its attempt guard is the only thing in the way.
        first.picname = "2.png".into();
        let wait = rig.load(&first);
        assert!(unavailable(&wait).contains("tried recently"), "{wait:?}");
        rig.db
            .open()
            .record_request_attempt(RequestKind::Cover, 7, 1)
            .unwrap();
        assert_eq!(ready(&rig.load(&first)).width, 11);
        assert!(rig.db.covers().join("1.png").is_file());
        assert!(rig.db.covers().join("2.png").is_file());
        assert_eq!(rig.requests(), 2);
    }

    #[test]
    fn offline_or_switched_off_sends_nothing_and_takes_no_slot() {
        let rig = Rig::new(|_| HttpReply::ok(png(8, 8)));
        let mut offline = job("42.png");
        offline.offline = true;
        let message = rig.load(&offline);
        assert_eq!(unavailable(&message), "Cover not downloaded (offline)");

        let mut off = job("42.png");
        off.download = false;
        assert!(unavailable(&rig.load(&off)).contains("off in Options"));

        assert_eq!(rig.requests(), 0);
        assert_eq!(rig.limiter_sends(), 0);
        assert!(
            rig.db
                .open()
                .request_last_attempt(RequestKind::Cover, 7)
                .unwrap()
                .is_none(),
            "nothing was attempted"
        );
        assert!(!rig.db.covers().join("42.png").exists());
    }

    #[test]
    fn an_active_cooldown_stops_the_download() {
        let rig = Rig::new(|_| HttpReply::ok(png(8, 8)));
        rig.db
            .open()
            .set_sync_cooldown(format::now() + 600, "banned for synthetic reasons")
            .unwrap();
        let message = rig.load(&job("42.png"));
        assert!(unavailable(&message).contains("paused"), "{message:?}");
        assert_eq!(rig.requests(), 0);
    }

    #[test]
    fn a_ban_met_by_a_cover_pauses_all_requests_for_everyone() {
        let rig = Rig::new(|_| HttpReply::status("403 Forbidden"));
        let message = rig.load(&job("42.png"));
        assert!(unavailable(&message).contains("banned"), "{message:?}");
        let (until, _) = rig
            .db
            .open()
            .sync_cooldown()
            .unwrap()
            .expect("cooldown saved");
        assert!(until >= format::now() + 29 * 60);
        assert!(
            rig.events
                .1
                .try_iter()
                .any(|e| matches!(e, Event::Cooldown { .. })),
            "the window learns about the pause"
        );
        // A different anime is held back by the same pause.
        let mut other = job("43.png");
        other.aid = 8;
        assert!(unavailable(&rig.load(&other)).contains("paused"));
        assert_eq!(rig.requests(), 1);
    }

    #[test]
    fn a_missing_picture_is_reported_and_not_retried_at_once() {
        let rig = Rig::new(|_| HttpReply::status("404 Not Found"));
        assert!(unavailable(&rig.load(&job("42.png"))).contains("not downloaded"));
        assert!(
            rig.db.open().sync_cooldown().unwrap().is_none(),
            "404 is no ban"
        );
        assert!(unavailable(&rig.load(&job("42.png"))).contains("tried recently"));
        assert_eq!(rig.requests(), 1);
    }

    #[test]
    fn a_corrupt_download_is_reported_and_never_cached() {
        let rig = Rig::new(|_| HttpReply::ok(b"<html>this is not a picture</html>".to_vec()));
        let message = rig.load(&job("42.png"));
        assert!(
            unavailable(&message).contains("not a readable picture"),
            "{message:?}"
        );
        assert!(!rig.db.covers().join("42.png").exists());
        assert_eq!(rig.requests(), 1);
    }

    #[test]
    fn a_truncated_download_is_not_cached() {
        let rig = Rig::new(|_| HttpReply::truncated(png(64, 64)));
        let message = rig.load(&job("42.png"));
        assert!(
            unavailable(&message).contains("not downloaded"),
            "{message:?}"
        );
        assert!(!rig.db.covers().join("42.png").exists());
    }

    #[test]
    fn a_corrupt_cached_file_is_removed_and_fetched_again() {
        let rig = Rig::new(|_| HttpReply::ok(png(16, 16)));
        std::fs::create_dir_all(rig.db.covers()).unwrap();
        std::fs::write(rig.db.covers().join("42.png"), b"half a pict").unwrap();
        let outcome = rig.load(&job("42.png"));
        let CoverOutcome::Ready { image, note } = &outcome else {
            panic!("{outcome:?}")
        };
        assert_eq!(image.width, 16);
        assert!(note.as_deref().unwrap().contains("removed"));
        assert_eq!(rig.requests(), 1);
        assert_eq!(
            image::load_from_memory(&std::fs::read(rig.db.covers().join("42.png")).unwrap())
                .unwrap()
                .width(),
            16
        );
    }

    #[test]
    fn a_corrupt_cached_file_offline_shows_no_cover_and_sends_nothing() {
        let rig = Rig::new(|_| HttpReply::ok(png(16, 16)));
        std::fs::create_dir_all(rig.db.covers()).unwrap();
        std::fs::write(rig.db.covers().join("42.png"), b"half a pict").unwrap();
        let mut offline = job("42.png");
        offline.offline = true;
        assert!(unavailable(&rig.load(&offline)).contains("offline"));
        assert_eq!(rig.requests(), 0);
    }

    #[test]
    fn a_bad_picture_name_never_touches_the_disk_or_the_network() {
        let rig = Rig::new(|_| HttpReply::ok(png(8, 8)));
        for bad in ["../evil.png", "a/b.png", "..\\evil.png", ""] {
            let message = rig.load(&job(bad));
            assert!(
                unavailable(&message).contains("picture name"),
                "{bad:?}: {message:?}"
            );
        }
        assert_eq!(rig.requests(), 0);
        assert!(!rig.db.covers().exists());
        assert!(!rig.db.dir.join("evil.png").exists());
    }

    #[test]
    fn a_request_that_a_newer_one_replaced_is_not_sent() {
        let rig = Rig::new(|_| HttpReply::ok(png(8, 8)));
        let outcome = rig.load_with(&job("42.png"), &|| true);
        assert!(matches!(outcome, CoverOutcome::Superseded), "{outcome:?}");
        assert_eq!(rig.requests(), 0);
        assert!(
            rig.db
                .open()
                .request_last_attempt(RequestKind::Cover, 7)
                .unwrap()
                .is_none(),
            "no attempt is counted for a request that was never sent"
        );
        // A cached cover is served even when newer requests are waiting.
        std::fs::create_dir_all(rig.db.covers()).unwrap();
        std::fs::write(rig.db.covers().join("42.png"), png(5, 5)).unwrap();
        assert_eq!(ready(&rig.load_with(&job("42.png"), &|| true)).width, 5);
    }

    #[test]
    fn large_pictures_are_scaled_down_and_hostile_ones_refused() {
        let wide = decode_cover(&png(3000, 100)).unwrap();
        assert_eq!(wide.width, MAX_COVER_SIDE as usize);
        assert!(wide.height >= 1 && wide.height < 100);
        assert_eq!(wide.rgba.len(), wide.width * wide.height * 4);
        let small = decode_cover(&png(7, 9)).unwrap();
        assert_eq!((small.width, small.height), (7, 9));

        assert!(decode_cover(b"").is_err());
        assert!(decode_cover(b"GIF89a-but-not-really").is_err());
        let mut cut = png(64, 64);
        cut.truncate(cut.len() / 2);
        assert!(decode_cover(&cut).is_err());
        // Dimensions far beyond the limit are refused before any pixel memory is allocated.
        let mut huge = png(2, 2);
        huge[16..20].copy_from_slice(&40_000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&40_000u32.to_be_bytes());
        assert!(decode_cover(&huge).is_err());
    }

    #[test]
    fn the_cover_thread_answers_every_request_and_stops() {
        let rig = Rig::new(|_| HttpReply::ok(png(12, 12)));
        std::fs::create_dir_all(rig.db.covers()).unwrap();
        std::fs::write(rig.db.covers().join("42.png"), png(12, 12)).unwrap();
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(CoverMsg::Load(job("42.png"))).unwrap();
        tx.send(CoverMsg::Stop).unwrap();
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        run_covers(
            rig.db.path.clone(),
            rx,
            events_tx,
            rig.limits.limits.clone(),
            Arc::new(AtomicBool::new(false)),
        );
        let answered: Vec<Event> = events_rx.try_iter().collect();
        assert_eq!(answered.len(), 1);
        assert!(matches!(
            &answered[0],
            Event::CoverLoaded { picname, outcome: CoverOutcome::Ready { .. } } if picname == "42.png"
        ));
        assert_eq!(rig.requests(), 0);
    }
}
