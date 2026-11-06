//! Anime cover pictures: the permanent on-disk cache and the rules for when one may be fetched.
//!
//! AniDB does not own the artwork and grants no licence, so covers are kept locally for the
//! user's own display and are never redistributed or re-hosted (no export, backup or upload
//! includes them). A cover is fetched only when something asks for it (the anime detail
//! window), one file per request, never as a crawl. Picture names are immutable: a cover that is
//! in the cache is never requested again, and a new `picname` is a new file.
//!
//! The decision lives in [`plan_cover`] and the download in [`download_cover`]; decoding the
//! picture is the application's job (this crate has no image decoder), so the download takes the
//! decoder's verdict as a closure and caches nothing that is not a readable picture. Every
//! request goes through the [`HttpClient`] and therefore the process-wide limiter, and the
//! attempt is saved before it is sent ([`RequestKind::Cover`]).
//!
//! The rules and their sources are recorded in
//! [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits).

use std::fs;
use std::path::{Path, PathBuf};

use animatic_anidb::error::AnidbError;
use animatic_anidb::http::{
    DEFAULT_IMAGE_SERVER, HttpClient, MAX_IMAGE_BYTES, validate_image_host, validate_picname,
};

use crate::db::Db;
use crate::limits::RequestKind;
use crate::{Result, atomic, format};

/// Folder of the cover cache, inside the data directory (next to the database).
pub const COVERS_DIR: &str = "covers";

/// `meta` key: the image host the last successful `AUTH` announced (`imgserver=1`).
pub const IMAGE_SERVER_KEY: &str = "image_server";

/// The cover cache that belongs to the database at `db_path`: `covers` in the same folder, the
/// data directory of the default profile.
pub fn covers_dir(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(COVERS_DIR)
}

/// Where the cover `picname` is or would be cached. The name is validated, so the path always
/// stays directly inside `dir`.
pub fn cover_path(dir: &Path, picname: &str) -> Result<PathBuf> {
    validate_picname(picname).map_err(|e| crate::Error::Other(e.to_string()))?;
    Ok(dir.join(picname))
}

/// The cached file of `picname`, if there is one.
pub fn cached_cover(dir: &Path, picname: &str) -> Result<Option<PathBuf>> {
    let path = cover_path(dir, picname)?;
    Ok(path.is_file().then_some(path))
}

/// Cache `bytes` as the cover `picname`: the folder is created if needed and the file appears
/// complete or not at all ([`atomic::write_atomic`]). Refuses pictures over
/// [`MAX_IMAGE_BYTES`].
pub fn store_cover(dir: &Path, picname: &str, bytes: &[u8]) -> Result<PathBuf> {
    let path = cover_path(dir, picname)?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(crate::Error::Other(format!(
            "cover {picname} is larger than {MAX_IMAGE_BYTES} bytes"
        )));
    }
    fs::create_dir_all(dir)?;
    atomic::write_atomic(&path, bytes)?;
    Ok(path)
}

/// Remove a cached cover that cannot be read as a picture, so that the next request fetches it
/// again (subject to [`RequestKind::Cover`]).
pub fn discard_cover(dir: &Path, picname: &str) -> Result<()> {
    match fs::remove_file(cover_path(dir, picname)?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

impl Db {
    /// The image host the last successful login announced, if it was saved.
    pub fn image_server(&self) -> Result<Option<String>> {
        Ok(self.meta(IMAGE_SERVER_KEY)?.filter(|h| !h.is_empty()))
    }

    /// Remember the image host from a login reply, so covers can be fetched later without a
    /// login. Something that is not a plain host name is ignored: it ends up in a URL.
    pub fn remember_image_server(&self, host: &str) -> Result<()> {
        if validate_image_host(host).is_err() {
            log::warn!("ignoring an image server that is not a plain host name: {host:?}");
            return Ok(());
        }
        if self.meta(IMAGE_SERVER_KEY)?.as_deref() != Some(host) {
            self.set_meta(IMAGE_SERVER_KEY, host)?;
        }
        Ok(())
    }
}

/// What the caller wants to know about one cover.
#[derive(Debug, Clone)]
pub struct CoverRequest<'a> {
    pub aid: i64,
    pub picname: &'a str,
    /// The `anidb.image_server` setting, used until a login announced a host.
    pub image_server_setting: &'a str,
    /// The "download covers" setting. Cached covers are shown either way.
    pub download: bool,
    /// Work offline.
    pub offline: bool,
}

/// Why a cover is not fetched now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverSkip {
    /// The picture name is not a plain file name, or the image server is not a plain host.
    Invalid(String),
    /// Cover downloads are switched off in the settings.
    Disabled,
    /// Work offline.
    Offline,
    /// A ban or other pause of AniDB traffic is active (epoch seconds, reason).
    Paused { until: i64, reason: String },
    /// A download of this anime's cover was attempted recently; allowed again at `until`.
    Recent { until: i64 },
}

impl std::fmt::Display for CoverSkip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoverSkip::Invalid(why) => write!(f, "Cover not downloaded: {why}"),
            CoverSkip::Disabled => {
                write!(
                    f,
                    "Cover not downloaded (cover downloads are off in Options)"
                )
            }
            CoverSkip::Offline => write!(f, "Cover not downloaded (offline)"),
            CoverSkip::Paused { until, reason } => write!(
                f,
                "Cover not downloaded: AniDB requests are paused until {} ({reason})",
                format::datetime_local(*until)
            ),
            CoverSkip::Recent { until } => write!(
                f,
                "Cover not downloaded: a download was tried recently, the next try is allowed after {}",
                format::datetime_local(*until)
            ),
        }
    }
}

/// What to do for one cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverPlan {
    /// The cover is cached: no request, ever.
    Cached(PathBuf),
    /// Download it from this image host with [`download_cover`].
    Download { host: String },
    /// Do not request it now.
    Skip(CoverSkip),
}

/// The image host to use: the one the last login announced, else the setting (empty means
/// AniDB's default host).
fn resolve_host(saved: Option<String>, setting: &str) -> std::result::Result<String, CoverSkip> {
    if let Some(saved) = saved.filter(|h| validate_image_host(h).is_ok()) {
        return Ok(saved);
    }
    let setting = setting.trim();
    if setting.is_empty() {
        return Ok(DEFAULT_IMAGE_SERVER.to_string());
    }
    validate_image_host(setting)
        .map(|()| setting.to_string())
        .map_err(|e| CoverSkip::Invalid(e.to_string()))
}

/// Decide what to do for `request` at epoch second `now`. Only reads: nothing is saved here.
///
/// The order matters: a cached cover wins over everything, so a cover that was fetched once is
/// shown offline, while banned, and with downloads switched off. Only then do the settings, the
/// shared pause (a ban or other cooldown of AniDB traffic) and the per-anime attempt guard
/// decide.
pub fn plan_cover(db: &Db, dir: &Path, request: &CoverRequest<'_>, now: i64) -> Result<CoverPlan> {
    if let Err(e) = validate_picname(request.picname) {
        return Ok(CoverPlan::Skip(CoverSkip::Invalid(e.to_string())));
    }
    if let Some(path) = cached_cover(dir, request.picname)? {
        return Ok(CoverPlan::Cached(path));
    }
    if !request.download {
        return Ok(CoverPlan::Skip(CoverSkip::Disabled));
    }
    if request.offline {
        return Ok(CoverPlan::Skip(CoverSkip::Offline));
    }
    if let Some((until, reason)) = db.sync_cooldown()? {
        return Ok(CoverPlan::Skip(CoverSkip::Paused { until, reason }));
    }
    if let Some(until) = db.request_next_allowed(RequestKind::Cover, request.aid, now)? {
        return Ok(CoverPlan::Skip(CoverSkip::Recent { until }));
    }
    let host = match resolve_host(db.image_server()?, request.image_server_setting) {
        Ok(host) => host,
        Err(skip) => return Ok(CoverPlan::Skip(skip)),
    };
    Ok(CoverPlan::Download { host })
}

/// Why a download did not produce a cover.
#[derive(Debug, Clone)]
pub enum CoverError {
    /// Nothing usable arrived: the request was refused or failed. Bans and rate limits
    /// (`Banned`, `ServerUnavailable`) must pause all AniDB traffic like any other ban.
    Request(AnidbError),
    /// The server answered with something that is not a readable picture; nothing was cached.
    Rejected(String),
}

impl std::fmt::Display for CoverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoverError::Request(e) => write!(f, "{e}"),
            CoverError::Rejected(why) => write!(f, "the file is not a readable picture ({why})"),
        }
    }
}

/// A downloaded cover.
#[derive(Debug)]
pub struct Downloaded {
    /// The picture as received.
    pub bytes: Vec<u8>,
    /// Where it was cached, or why it could not be (the picture is still good for display).
    pub cached: std::result::Result<PathBuf, String>,
}

/// Fetch the cover `picname` of anime `aid` from `host` and cache it.
///
/// The attempt is saved before anything is sent, success or not ([`RequestKind::Cover`]); the
/// request takes its slot from the client's shared limiter and sends no `Referer`. `readable`
/// is the decoder's verdict on the bytes: a picture it rejects is not cached, because a cached
/// file is never fetched again.
pub fn download_cover(
    db: &Db,
    dir: &Path,
    http: &mut HttpClient,
    aid: i64,
    picname: &str,
    host: &str,
    readable: impl FnOnce(&[u8]) -> std::result::Result<(), String>,
) -> std::result::Result<Downloaded, CoverError> {
    validate_picname(picname).map_err(CoverError::Request)?;
    db.record_request_attempt(RequestKind::Cover, aid, format::now())
        .map_err(|e| CoverError::Request(AnidbError::Storage(e.to_string())))?;
    let bytes = http.image(host, picname).map_err(CoverError::Request)?;
    readable(&bytes).map_err(CoverError::Rejected)?;
    let cached = store_cover(dir, picname, &bytes).map_err(|e| e.to_string());
    Ok(Downloaded { bytes, cached })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use animatic_anidb::testing::{HttpFixture, HttpReply, test_limits};
    use parking_lot::Mutex;

    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn request<'a>(picname: &'a str, offline: bool, download: bool) -> CoverRequest<'a> {
        CoverRequest {
            aid: 7,
            picname,
            image_server_setting: "cdn.example.test",
            download,
            offline,
        }
    }

    fn folder() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn the_cache_folder_sits_next_to_the_database() {
        assert_eq!(
            covers_dir(Path::new("/data/animatic.sqlite3")),
            Path::new("/data").join("covers")
        );
    }

    #[test]
    fn picture_names_that_could_leave_the_folder_are_refused_everywhere() {
        let dir = folder();
        for bad in [
            "../x.jpg", "a/b.jpg", "a\\b.jpg", "..", "", "nul.jpg", "C:x.jpg",
        ] {
            assert!(cover_path(dir.path(), bad).is_err(), "{bad:?}");
            assert!(cached_cover(dir.path(), bad).is_err(), "{bad:?}");
            assert!(store_cover(dir.path(), bad, b"x").is_err(), "{bad:?}");
            assert!(discard_cover(dir.path(), bad).is_err(), "{bad:?}");
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        assert_eq!(
            cover_path(dir.path(), "1.jpg").unwrap().parent(),
            Some(dir.path())
        );
    }

    #[test]
    fn a_stored_cover_is_complete_and_leaves_no_temporary_file() {
        let dir = folder();
        let cache = dir.path().join("covers");
        assert_eq!(cached_cover(&cache, "1.jpg").unwrap(), None);
        let path = store_cover(&cache, "1.jpg", b"first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");
        assert_eq!(cached_cover(&cache, "1.jpg").unwrap(), Some(path.clone()));
        // A new picname is a new file; the old one stays.
        store_cover(&cache, "2.jpg", b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");
        let mut names: Vec<_> = fs::read_dir(&cache)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["1.jpg", "2.jpg"], "no temporary file is left");

        discard_cover(&cache, "1.jpg").unwrap();
        discard_cover(&cache, "1.jpg").unwrap();
        assert_eq!(cached_cover(&cache, "1.jpg").unwrap(), None);
    }

    #[test]
    fn a_failed_write_leaves_no_cover_behind() {
        let dir = folder();
        // The "folder" is a file, so nothing can be created in it.
        let blocker = dir.path().join("covers");
        fs::write(&blocker, b"not a folder").unwrap();
        assert!(store_cover(&blocker, "1.jpg", b"x").is_err());
        assert_eq!(cached_cover(&blocker, "1.jpg").unwrap(), None);
        let big = vec![0u8; MAX_IMAGE_BYTES as usize + 1];
        assert!(store_cover(&dir.path().join("other"), "1.jpg", &big).is_err());
        assert!(!dir.path().join("other").join("1.jpg").exists());
    }

    #[test]
    fn a_cached_cover_needs_no_network_setting_and_no_permission() {
        let dir = folder();
        store_cover(dir.path(), "1.jpg", b"x").unwrap();
        let db = Db::memory().unwrap();
        db.set_sync_cooldown(NOW + 3600, "ban").unwrap();
        // Paused, offline and switched off at once: the file is still shown.
        let plan = plan_cover(&db, dir.path(), &request("1.jpg", true, false), NOW).unwrap();
        assert_eq!(plan, CoverPlan::Cached(dir.path().join("1.jpg")));
    }

    #[test]
    fn nothing_is_requested_offline_while_off_or_paused_or_after_a_recent_try() {
        let dir = folder();
        let db = Db::memory().unwrap();
        let plan = |req: CoverRequest<'_>, db: &Db| plan_cover(db, dir.path(), &req, NOW).unwrap();

        assert_eq!(
            plan(request("1.jpg", true, true), &db),
            CoverPlan::Skip(CoverSkip::Offline)
        );
        assert_eq!(
            plan(request("1.jpg", false, false), &db),
            CoverPlan::Skip(CoverSkip::Disabled)
        );
        assert!(matches!(
            plan(request("../1.jpg", false, true), &db),
            CoverPlan::Skip(CoverSkip::Invalid(_))
        ));
        assert_eq!(
            plan(request("1.jpg", false, true), &db),
            CoverPlan::Download {
                host: "cdn.example.test".into()
            }
        );

        db.record_request_attempt(RequestKind::Cover, 7, NOW - 60)
            .unwrap();
        assert_eq!(
            plan(request("1.jpg", false, true), &db),
            CoverPlan::Skip(CoverSkip::Recent {
                until: NOW - 60 + 3600
            })
        );
        db.record_request_attempt(RequestKind::Cover, 7, NOW - 3600)
            .unwrap();
        assert!(matches!(
            plan(request("1.jpg", false, true), &db),
            CoverPlan::Download { .. }
        ));
    }

    #[test]
    fn a_ban_or_cooldown_stops_cover_requests() {
        let dir = folder();
        let db = Db::memory().unwrap();
        let until = crate::format::now() + 1800;
        db.set_sync_cooldown(until, "banned").unwrap();
        let skip = plan_cover(&db, dir.path(), &request("1.jpg", false, true), NOW).unwrap();
        assert_eq!(
            skip,
            CoverPlan::Skip(CoverSkip::Paused {
                until,
                reason: "banned".into()
            })
        );
    }

    #[test]
    fn the_host_from_the_login_wins_over_the_setting_and_is_remembered() {
        let dir = folder();
        let db = Db::memory().unwrap();
        let host = |db: &Db, setting: &str| {
            let mut req = request("1.jpg", false, true);
            req.image_server_setting = setting;
            match plan_cover(db, dir.path(), &req, NOW).unwrap() {
                CoverPlan::Download { host } => Ok(host),
                CoverPlan::Skip(skip) => Err(skip),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(host(&db, ""), Ok(DEFAULT_IMAGE_SERVER.to_string()));
        assert_eq!(
            host(&db, "mirror.example.test"),
            Ok("mirror.example.test".into())
        );
        assert!(matches!(host(&db, "bad/host"), Err(CoverSkip::Invalid(_))));

        db.remember_image_server("img7.example.test").unwrap();
        assert_eq!(
            db.image_server().unwrap().as_deref(),
            Some("img7.example.test")
        );
        assert_eq!(
            host(&db, "mirror.example.test"),
            Ok("img7.example.test".into())
        );
        // Even an unusable setting no longer matters.
        assert_eq!(host(&db, "bad/host"), Ok("img7.example.test".into()));

        // Something that is not a host name is never saved.
        db.remember_image_server("evil.example/../x").unwrap();
        assert_eq!(
            db.image_server().unwrap().as_deref(),
            Some("img7.example.test")
        );
    }

    /// A tiny stand-in for a decoder: only `OK:` pictures are readable.
    fn readable(bytes: &[u8]) -> std::result::Result<(), String> {
        if bytes.starts_with(b"OK:") {
            Ok(())
        } else {
            Err("synthetic decoder says no".into())
        }
    }

    #[test]
    fn a_download_is_limited_recorded_before_sending_and_cached_once() {
        let heads = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = heads.clone();
        let fixture = HttpFixture::start(move |_, head| {
            seen.lock().push(head.to_string());
            HttpReply::ok(b"OK:picture".to_vec())
        });
        let t = test_limits();
        let dir = folder();
        let db = Db::memory().unwrap();
        let mut http = HttpClient::new("synthetic", 1, t.limits.clone())
            .with_loopback_image_base(fixture.base());
        // A neighbouring request just used the shared limiter.
        t.limits.wait_http().unwrap();
        let slept = t.clock.sleeps().len();

        let got =
            download_cover(&db, dir.path(), &mut http, 7, "1.jpg", "ignored", readable).unwrap();
        assert_eq!(got.bytes, b"OK:picture");
        assert_eq!(got.cached.as_ref().unwrap(), &dir.path().join("1.jpg"));
        assert_eq!(fs::read(dir.path().join("1.jpg")).unwrap(), b"OK:picture");
        assert_eq!(
            t.clock.sleeps()[slept..].len(),
            1,
            "waited for the shared slot"
        );
        assert_eq!(
            t.store.sends(animatic_anidb::limits::Protocol::Http).len(),
            2
        );
        assert!(
            db.request_last_attempt(RequestKind::Cover, 7)
                .unwrap()
                .is_some()
        );
        let head = heads.lock()[0].to_ascii_lowercase();
        assert!(head.starts_with("get /images/main/1.jpg "), "{head}");
        assert!(!head.contains("referer"), "{head}");

        // From here on the cover is cached and nothing asks again.
        for _ in 0..3 {
            let plan = plan_cover(&db, dir.path(), &request("1.jpg", false, true), NOW).unwrap();
            assert!(matches!(plan, CoverPlan::Cached(_)));
        }
        assert_eq!(fixture.requests(), 1);
    }

    #[test]
    fn a_picture_the_decoder_rejects_is_not_cached_but_the_attempt_counts() {
        let fixture = HttpFixture::start(|_, _| HttpReply::ok(b"<html>not a picture".to_vec()));
        let t = test_limits();
        let dir = folder();
        let db = Db::memory().unwrap();
        let mut http = HttpClient::new("synthetic", 1, t.limits.clone())
            .with_loopback_image_base(fixture.base());
        let err =
            download_cover(&db, dir.path(), &mut http, 7, "1.jpg", "x", readable).unwrap_err();
        assert!(matches!(err, CoverError::Rejected(_)), "{err}");
        assert_eq!(cached_cover(dir.path(), "1.jpg").unwrap(), None);
        assert!(
            db.request_last_attempt(RequestKind::Cover, 7)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_refused_request_is_an_error_and_a_ban_is_visible_to_the_caller() {
        let fixture = HttpFixture::start(|index, _| match index {
            0 => HttpReply::status("404 Not Found"),
            _ => HttpReply::status("403 Forbidden"),
        });
        let t = test_limits();
        let dir = folder();
        let db = Db::memory().unwrap();
        let mut http = HttpClient::new("synthetic", 1, t.limits.clone())
            .with_loopback_image_base(fixture.base());
        let missing =
            download_cover(&db, dir.path(), &mut http, 7, "1.jpg", "x", readable).unwrap_err();
        assert!(matches!(missing, CoverError::Request(_)));
        let banned =
            download_cover(&db, dir.path(), &mut http, 8, "2.jpg", "x", readable).unwrap_err();
        assert!(
            matches!(banned, CoverError::Request(AnidbError::Banned(_))),
            "{banned}"
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_bad_picture_name_sends_nothing_and_saves_no_attempt() {
        let fixture = HttpFixture::start(|_, _| HttpReply::ok(b"OK:x".to_vec()));
        let t = test_limits();
        let dir = folder();
        let db = Db::memory().unwrap();
        let mut http = HttpClient::new("synthetic", 1, t.limits.clone())
            .with_loopback_image_base(fixture.base());
        assert!(download_cover(&db, dir.path(), &mut http, 7, "../x.jpg", "x", readable).is_err());
        assert_eq!(fixture.requests(), 0);
        assert!(
            db.request_last_attempt(RequestKind::Cover, 7)
                .unwrap()
                .is_none()
        );
    }
}
