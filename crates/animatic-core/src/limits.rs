//! Request guards and saved limit state for AniDB traffic.
//!
//! The spacing between requests lives in `animatic_anidb::limits`, which has no database. This
//! module supplies what needs storage: the [`LimitStore`] over the `meta` table that lets spacing
//! and the `AUTH` backoff survive restarts, and the guards that stop a request from being
//! repeated too soon. Every guard counts *attempts*, not successes: the attempt time is saved
//! before the request is sent, so a failed or truncated download, a crash right after sending
//! or a restart cannot allow a repeat inside the interval. `titles_dump_date` and
//! `anime.fetched` keep their meaning (a successful import) and are only consulted as a lower
//! bound for databases that saved no attempts yet.
//!
//! The rules and their sources are recorded in
//! [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use animatic_anidb::limits::{Clock, LimitStore, Limits, Protocol, SystemClock};
use rusqlite::{OptionalExtension, params};

use crate::db::Db;
use crate::error::Result;
use crate::format;
use crate::store::TITLES_DUMP_DATE_KEY;

/// Least time between two requests for the title dump, counted from the last attempt. Rule:
/// "Title dump" in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits)
/// (regenerated daily, not to be requested more than once a day).
pub const TITLES_DUMP_MIN_INTERVAL: Duration = Duration::from_secs(24 * 3600);

/// Least time between two requests for the same anime, counted from the last attempt. Rule:
/// "HTTP API" in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits)
/// (the same dataset more than once a day can lead to a ban).
pub const ANIME_MIN_AGE: Duration = Duration::from_secs(24 * 3600);

/// Least time between two `FILE` requests for a file AniDB already knew, counted from the last
/// attempt. A design choice: AniDB only asks clients to cache file data "for extended periods"
/// and to query only files it did not know last time (see "Caching" in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits));
/// seven days is deliberately conservative.
pub const KNOWN_FILE_MIN_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// Least time between two attempts to download the cover of the same anime, counted from the
/// last attempt. A design choice: AniDB publishes no image rules (see "Images" in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits)).
/// A cover that arrived is never requested again; this only spaces out retries after a failure.
pub const COVER_MIN_AGE: Duration = Duration::from_secs(3600);
/// Least time between two MyList export requests, counted from the last attempt. A design
/// choice: AniDB queues one export at a time, needs up to 24 h to prepare it and asks that the
/// export tool is not abused (see "Whole MyList" in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits)).
pub const MYLIST_EXPORT_MIN_INTERVAL: Duration = Duration::from_secs(24 * 3600);

/// The `request_attempt` id of the MyList export, which is one account-wide request rather than
/// one per record.
pub const MYLIST_EXPORT_ID: i64 = 0;

/// `meta` key: epoch milliseconds of the last UDP packet sent (saved before sending).
pub const UDP_LAST_SEND_KEY: &str = "udp_last_send_ms";
/// `meta` key: epoch milliseconds of the last HTTP request sent (saved before sending).
pub const HTTP_LAST_SEND_KEY: &str = "http_last_send_ms";
/// `meta` key: epoch seconds of the last attempt to download the title dump (saved before the
/// request), whether or not it succeeded.
pub const TITLES_DUMP_ATTEMPT_KEY: &str = "titles_dump_attempt";
/// `meta` key: consecutive `AUTH` attempts without a reply, which selects the backoff step; reset
/// by a successful login.
pub const AUTH_BACKOFF_STEP_KEY: &str = "auth_backoff_step";

/// A record AniDB can be asked for again and again, whose repeats are limited per record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    /// A full anime record (HTTP API), limited by [`ANIME_MIN_AGE`].
    Anime,
    /// A file AniDB already knew (UDP `FILE` by id), limited by [`KNOWN_FILE_MIN_AGE`].
    File,
    /// The cover picture of an anime (image server, by anime id), limited by
    /// [`COVER_MIN_AGE`]. Covers are cached for good, so only a download that failed is ever
    /// repeated.
    Cover,
    /// A MyList export (UDP `MYLISTEXPORT`, id [`MYLIST_EXPORT_ID`]), limited by
    /// [`MYLIST_EXPORT_MIN_INTERVAL`].
    MylistExport,
}

impl RequestKind {
    fn tag(self) -> &'static str {
        match self {
            RequestKind::Anime => "anime",
            RequestKind::File => "file",
            RequestKind::Cover => "cover",
            RequestKind::MylistExport => "export",
        }
    }

    /// Least time between two requests for the same record.
    pub fn min_age(self) -> Duration {
        match self {
            RequestKind::Anime => ANIME_MIN_AGE,
            RequestKind::File => KNOWN_FILE_MIN_AGE,
            RequestKind::Cover => COVER_MIN_AGE,
            RequestKind::MylistExport => MYLIST_EXPORT_MIN_INTERVAL,
        }
    }
}

/// When a request that was last sent at `last` may be sent again: `None` means now, otherwise
/// the epoch second from which it is allowed.
fn next_allowed(last: Option<i64>, now: i64, min_age: Duration) -> Option<i64> {
    let last = last?;
    // A time in the future (the clock was set back) counts as now, so the wait stays bounded
    // by one interval.
    let until = last.min(now).saturating_add(min_age.as_secs() as i64);
    (until > now).then_some(until)
}

/// "2026-11-07 10:00" in local time, for messages.
fn when(until: i64) -> String {
    format::datetime_local(until)
}

/// Why a title dump is not requested now, with the time it is allowed again.
pub fn titles_dump_wait_message(until: i64) -> String {
    format!(
        "Title dump skipped: AniDB allows it once a day and it was requested recently. The next download is allowed after {}",
        when(until)
    )
}

/// Why an anime is not requested now, with the time it is allowed again.
pub fn anime_wait_message(aid: i64, until: i64) -> String {
    format!(
        "Anime {aid} skipped: AniDB asks for each anime at most once a day and it was requested recently. The next refresh is allowed after {}",
        when(until)
    )
}

/// Why a file is not requested now, with the time it is allowed again.
pub fn file_wait_message(fid: i64, until: i64) -> String {
    format!(
        "File {fid} skipped: it was requested from AniDB recently (known files are refreshed at most every {} days). The next refresh is allowed after {}",
        KNOWN_FILE_MIN_AGE.as_secs() / 86_400,
        when(until)
    )
}

/// Why a MyList export is not requested now, with the time it is allowed again.
pub fn mylist_export_wait_message(until: i64) -> String {
    format!(
        "MyList export not requested: one was requested recently and AniDB prepares it for up to 24 hours. A new request is allowed after {}",
        when(until)
    )
}

impl Db {
    /// A saved epoch-second time. An unreadable value counts as none, like an unreadable sync
    /// cooldown: a damaged value must never lock a feature for good.
    fn meta_time(&self, key: &str) -> Result<Option<i64>> {
        Ok(self
            .meta(key)?
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|t| *t > 0))
    }

    /// When the title dump was last requested: the saved attempt, or the last successful import
    /// if that is later (databases from before attempts were saved only have the import).
    pub fn titles_dump_last_request(&self) -> Result<Option<i64>> {
        Ok(self
            .meta_time(TITLES_DUMP_ATTEMPT_KEY)?
            .max(self.meta_time(TITLES_DUMP_DATE_KEY)?))
    }

    /// Save that the title dump is about to be requested. Call it before sending.
    pub fn record_titles_dump_attempt(&self, at: i64) -> Result<()> {
        self.set_meta(TITLES_DUMP_ATTEMPT_KEY, &at.to_string())
    }

    /// `None` when the title dump may be requested at `now`, otherwise the epoch second from
    /// which it may be ([`TITLES_DUMP_MIN_INTERVAL`] after the last attempt).
    pub fn titles_dump_next_allowed(&self, now: i64) -> Result<Option<i64>> {
        Ok(next_allowed(
            self.titles_dump_last_request()?,
            now,
            TITLES_DUMP_MIN_INTERVAL,
        ))
    }

    /// Save that `id` is about to be requested. Call it before sending.
    pub fn record_request_attempt(&self, kind: RequestKind, id: i64, at: i64) -> Result<()> {
        self.conn().execute(
            "INSERT INTO request_attempt(kind, id, attempted) VALUES (?1, ?2, ?3)
             ON CONFLICT(kind, id) DO UPDATE SET attempted = excluded.attempted",
            params![kind.tag(), id, at],
        )?;
        Ok(())
    }

    /// When `id` was last requested: the saved attempt or, for an anime, the time of its last
    /// full record if that is later (databases from before attempts were saved).
    pub fn request_last_attempt(&self, kind: RequestKind, id: i64) -> Result<Option<i64>> {
        let attempt: Option<i64> = self
            .conn()
            .query_row(
                "SELECT attempted FROM request_attempt WHERE kind = ?1 AND id = ?2",
                params![kind.tag(), id],
                |r| r.get(0),
            )
            .optional()?;
        let fetched: Option<i64> = match kind {
            RequestKind::Anime => self
                .conn()
                .query_row("SELECT fetched FROM anime WHERE aid = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()?
                .filter(|t| *t > 0),
            RequestKind::File | RequestKind::Cover | RequestKind::MylistExport => None,
        };
        Ok(attempt.max(fetched))
    }

    /// `None` when `id` may be requested at `now`, otherwise the epoch second from which it
    /// may be (the kind's minimum age after the last attempt).
    pub fn request_next_allowed(
        &self,
        kind: RequestKind,
        id: i64,
        now: i64,
    ) -> Result<Option<i64>> {
        Ok(next_allowed(
            self.request_last_attempt(kind, id)?,
            now,
            kind.min_age(),
        ))
    }

    /// Ids of this kind that may not be requested at `now`: the same answer as
    /// [`Db::request_next_allowed`], for many ids in one query.
    pub fn recent_requests(&self, kind: RequestKind, now: i64) -> Result<HashSet<i64>> {
        let since = now.saturating_sub(kind.min_age().as_secs() as i64);
        let mut ids = HashSet::new();
        let mut attempts = self
            .conn()
            .prepare("SELECT id FROM request_attempt WHERE kind = ?1 AND attempted > ?2")?;
        for id in attempts.query_map(params![kind.tag(), since], |r| r.get::<_, i64>(0))? {
            ids.insert(id?);
        }
        if kind == RequestKind::Anime {
            let mut fetched = self
                .conn()
                .prepare("SELECT aid FROM anime WHERE fetched > ?1")?;
            for id in fetched.query_map([since], |r| r.get::<_, i64>(0))? {
                ids.insert(id?);
            }
        }
        Ok(ids)
    }
}

/// The [`LimitStore`] over the `meta` table of the application database. It opens the database
/// for every call instead of holding a connection: calls are seconds apart and the store is
/// used from whichever thread runs the queue.
pub struct DbLimitStore {
    path: PathBuf,
}

impl DbLimitStore {
    pub fn new(path: &Path) -> DbLimitStore {
        DbLimitStore {
            path: path.to_path_buf(),
        }
    }

    fn open(&self) -> std::result::Result<Db, String> {
        Db::open(&self.path).map_err(|e| e.to_string())
    }
}

fn send_key(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Udp => UDP_LAST_SEND_KEY,
        Protocol::Http => HTTP_LAST_SEND_KEY,
    }
}

impl LimitStore for DbLimitStore {
    fn last_send(&self, protocol: Protocol) -> std::result::Result<Option<i64>, String> {
        let saved = self
            .open()?
            .meta(send_key(protocol))
            .map_err(|e| e.to_string())?;
        // An unreadable value counts as none; a database error is reported and stops sending.
        Ok(saved.and_then(|v| v.trim().parse().ok()))
    }

    fn record_send(&self, protocol: Protocol, at_ms: i64) -> std::result::Result<(), String> {
        self.open()?
            .set_meta(send_key(protocol), &at_ms.to_string())
            .map_err(|e| e.to_string())
    }

    fn auth_unanswered(&self) -> std::result::Result<u32, String> {
        let saved = self
            .open()?
            .meta(AUTH_BACKOFF_STEP_KEY)
            .map_err(|e| e.to_string())?;
        Ok(saved.and_then(|v| v.trim().parse().ok()).unwrap_or(0))
    }

    fn set_auth_unanswered(&self, count: u32) -> std::result::Result<(), String> {
        self.open()?
            .set_meta(AUTH_BACKOFF_STEP_KEY, &count.to_string())
            .map_err(|e| e.to_string())
    }
}

/// The process-wide AniDB limits over the database at `db_path`, on the real clock.
pub fn open_limits(db_path: &Path) -> Arc<Limits> {
    open_limits_with_clock(db_path, Arc::new(SystemClock))
}

/// Like [`open_limits`] with another clock (tests).
pub fn open_limits_with_clock(db_path: &Path, clock: Arc<dyn Clock>) -> Arc<Limits> {
    Limits::new(clock, Arc::new(DbLimitStore::new(db_path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 3600;

    fn temp_db_path(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "animatic-limits-{name}-{}.sqlite",
            std::process::id()
        ));
        remove_db_files(&path);
        path
    }

    fn remove_db_files(path: &Path) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn guards_open_exactly_when_the_minimum_age_has_passed() {
        let db = Db::memory().unwrap();
        let now = 1_800_000_000;
        assert_eq!(db.titles_dump_next_allowed(now).unwrap(), None);
        for kind in [RequestKind::Anime, RequestKind::File] {
            assert_eq!(db.request_next_allowed(kind, 7, now).unwrap(), None);
        }

        db.record_titles_dump_attempt(now - 3600).unwrap();
        assert_eq!(
            db.titles_dump_next_allowed(now).unwrap(),
            Some(now - 3600 + DAY)
        );
        db.record_titles_dump_attempt(now - DAY).unwrap();
        assert_eq!(
            db.titles_dump_next_allowed(now).unwrap(),
            None,
            "24 h are over"
        );
        db.record_titles_dump_attempt(now - DAY + 1).unwrap();
        assert_eq!(db.titles_dump_next_allowed(now).unwrap(), Some(now + 1));

        db.record_request_attempt(RequestKind::Anime, 7, now - 3600)
            .unwrap();
        assert_eq!(
            db.request_next_allowed(RequestKind::Anime, 7, now).unwrap(),
            Some(now - 3600 + DAY)
        );
        assert_eq!(
            db.request_next_allowed(RequestKind::Anime, 8, now).unwrap(),
            None
        );
        db.record_request_attempt(RequestKind::Anime, 7, now - DAY - 1)
            .unwrap();
        assert_eq!(
            db.request_next_allowed(RequestKind::Anime, 7, now).unwrap(),
            None
        );

        // Files and anime are separate namespaces with their own minimum age.
        db.record_request_attempt(RequestKind::File, 7, now - 6 * DAY)
            .unwrap();
        assert_eq!(
            db.request_next_allowed(RequestKind::File, 7, now).unwrap(),
            Some(now - 6 * DAY + 7 * DAY)
        );
        db.record_request_attempt(RequestKind::File, 7, now - 7 * DAY - 1)
            .unwrap();
        assert_eq!(
            db.request_next_allowed(RequestKind::File, 7, now).unwrap(),
            None
        );
    }

    #[test]
    fn an_attempt_in_the_future_waits_one_interval_at_most() {
        let db = Db::memory().unwrap();
        let now = 1_800_000_000;
        db.record_request_attempt(RequestKind::Anime, 1, now + 30 * DAY)
            .unwrap();
        assert_eq!(
            db.request_next_allowed(RequestKind::Anime, 1, now).unwrap(),
            Some(now + DAY)
        );
    }

    #[test]
    fn earlier_successes_count_as_requests_for_databases_without_attempts() {
        let db = Db::memory().unwrap();
        let now = 1_800_000_000;
        db.conn()
            .execute_batch(&format!(
                "INSERT INTO anime(aid, fetched) VALUES (1, {}), (2, {});",
                now - 3600,
                now - 2 * DAY
            ))
            .unwrap();
        db.set_meta(TITLES_DUMP_DATE_KEY, &(now - 3600).to_string())
            .unwrap();
        assert_eq!(
            db.request_next_allowed(RequestKind::Anime, 1, now).unwrap(),
            Some(now - 3600 + DAY)
        );
        assert_eq!(
            db.request_next_allowed(RequestKind::Anime, 2, now).unwrap(),
            None
        );
        assert_eq!(
            db.titles_dump_next_allowed(now).unwrap(),
            Some(now - 3600 + DAY)
        );
        // A recorded attempt does not turn into a success: `anime.fetched` stays untouched.
        db.record_request_attempt(RequestKind::Anime, 2, now)
            .unwrap();
        let fetched: i64 = db
            .conn()
            .query_row("SELECT fetched FROM anime WHERE aid = 2", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fetched, now - 2 * DAY);
    }

    #[test]
    fn recent_requests_agree_with_the_single_record_guard() {
        let db = Db::memory().unwrap();
        let now = 1_800_000_000;
        db.conn()
            .execute("INSERT INTO anime(aid, fetched) VALUES (4, ?1)", [now - 60])
            .unwrap();
        db.record_request_attempt(RequestKind::Anime, 1, now - 60)
            .unwrap();
        db.record_request_attempt(RequestKind::Anime, 2, now - DAY - 1)
            .unwrap();
        db.record_request_attempt(RequestKind::File, 1, now - 6 * DAY)
            .unwrap();
        db.record_request_attempt(RequestKind::File, 2, now - 8 * DAY)
            .unwrap();
        for (kind, ids) in [
            (RequestKind::Anime, HashSet::from([1, 4])),
            (RequestKind::File, HashSet::from([1])),
        ] {
            let recent = db.recent_requests(kind, now).unwrap();
            assert_eq!(recent, ids, "{kind:?}");
            for id in 1..=4 {
                assert_eq!(
                    recent.contains(&id),
                    db.request_next_allowed(kind, id, now).unwrap().is_some(),
                    "{kind:?} {id}"
                );
            }
        }
    }

    #[test]
    fn damaged_saved_times_never_lock_a_request() {
        let db = Db::memory().unwrap();
        for garbage in ["", "soon", "-5", "0"] {
            db.set_meta(TITLES_DUMP_ATTEMPT_KEY, garbage).unwrap();
            assert_eq!(
                db.titles_dump_next_allowed(1_800_000_000).unwrap(),
                None,
                "{garbage:?}"
            );
        }
    }

    #[test]
    fn the_saved_limit_state_survives_a_restart() {
        let path = temp_db_path("store");
        {
            let store = DbLimitStore::new(&path);
            assert_eq!(store.last_send(Protocol::Udp).unwrap(), None);
            assert_eq!(store.auth_unanswered().unwrap(), 0);
            store.record_send(Protocol::Udp, 1_800_000_000_123).unwrap();
            store
                .record_send(Protocol::Http, 1_800_000_000_456)
                .unwrap();
            store.set_auth_unanswered(3).unwrap();
        }
        let store = DbLimitStore::new(&path);
        assert_eq!(
            store.last_send(Protocol::Udp).unwrap(),
            Some(1_800_000_000_123)
        );
        assert_eq!(
            store.last_send(Protocol::Http).unwrap(),
            Some(1_800_000_000_456)
        );
        assert_eq!(store.auth_unanswered().unwrap(), 3);
        let db = Db::open(&path).unwrap();
        assert_eq!(
            db.meta(UDP_LAST_SEND_KEY).unwrap().as_deref(),
            Some("1800000000123")
        );
        db.set_meta(UDP_LAST_SEND_KEY, "garbage").unwrap();
        db.set_meta(AUTH_BACKOFF_STEP_KEY, "garbage").unwrap();
        assert_eq!(
            store.last_send(Protocol::Udp).unwrap(),
            None,
            "damaged value is ignored"
        );
        assert_eq!(store.auth_unanswered().unwrap(), 0);
        drop(db);
        remove_db_files(&path);
    }

    #[test]
    fn a_database_that_cannot_be_opened_stops_requests_instead_of_ignoring_the_state() {
        let path =
            std::env::temp_dir().join(format!("animatic-limits-missing-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        // A directory is not a database: every store call fails.
        let store = DbLimitStore::new(&path);
        assert!(store.last_send(Protocol::Udp).is_err());
        assert!(store.record_send(Protocol::Http, 1).is_err());
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// The guards as the queue executor applies them, against loopback servers.
#[cfg(test)]
mod guarded_requests {
    use std::net::SocketAddr;

    use animatic_anidb::http::HttpClient;
    use animatic_anidb::testing::{HttpFixture, HttpReply, TestLimits, UdpFixture, test_limits};
    use animatic_anidb::udp::{Config, UdpClient};
    use parking_lot::Mutex;

    use super::*;
    use crate::sync::{Command, CommandError, Outcome, execute};

    const DAY: i64 = 24 * 3600;
    const DUMP: &str = r#"<animetitles><anime aid="1"><title type="main" xml:lang="x-jat">Synthetic Alpha</title></anime></animetitles>"#;
    const ANIME: &str = r#"<anime id="1" restricted="false"><type>TV Series</type><titles><title xml:lang="x-jat" type="main">Synthetic Alpha</title></titles></anime>"#;

    fn gzip(text: &str) -> Vec<u8> {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    fn temp_db_path(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "animatic-guards-{name}-{}.sqlite",
            std::process::id()
        ));
        remove_db_files(&path);
        path
    }

    fn remove_db_files(path: &Path) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    fn http_client(fixture: &HttpFixture, t: &TestLimits, name: &str) -> HttpClient {
        HttpClient::new(name, 1, t.limits.clone())
            .with_loopback_endpoints(fixture.url("/httpapi"), fixture.url("/titles.xml.gz"))
    }

    /// A UDP client that is never used by the HTTP commands.
    fn idle_udp(t: &TestLimits) -> UdpClient {
        UdpClient::new(Config::default(), t.limits.clone())
    }

    fn udp_client(server: SocketAddr, t: &TestLimits) -> UdpClient {
        UdpClient::new(
            Config {
                server: server.to_string(),
                client: "syntheticclient".into(),
                username: "synthetic-user".into(),
                password: "synthetic-password".into(),
                timeout: Duration::from_secs(5),
                ..Config::default()
            },
            t.limits.clone(),
        )
    }

    fn run_http(
        db: &Db,
        cmd: &Command,
        fixture: &HttpFixture,
    ) -> std::result::Result<Outcome, CommandError> {
        let t = test_limits();
        execute(
            1,
            cmd,
            db,
            &mut idle_udp(&t),
            &mut http_client(fixture, &t, "syntheticclient"),
        )
    }

    fn is_skipped(outcome: &std::result::Result<Outcome, CommandError>) -> bool {
        matches!(outcome, Ok(Outcome::Skipped(_)))
    }

    /// Make every trace of an earlier request older than `age` seconds.
    fn age_title_dump(db: &Db, age: i64) {
        let then = format::now() - age;
        db.record_titles_dump_attempt(then).unwrap();
        db.set_meta(TITLES_DUMP_DATE_KEY, &then.to_string())
            .unwrap();
    }

    #[test]
    fn a_second_title_dump_inside_a_day_is_refused_without_any_request() {
        let db = Db::memory().unwrap();
        let dump = gzip(DUMP);
        let fixture = HttpFixture::start(move |_, _| HttpReply::ok(dump.clone()));

        let first = run_http(&db, &Command::TitlesDump, &fixture).unwrap();
        assert!(matches!(first, Outcome::Updated(_)), "{first:?}");
        assert_eq!(fixture.requests(), 1);

        let again = run_http(&db, &Command::TitlesDump, &fixture);
        let Ok(Outcome::Skipped(message)) = &again else {
            panic!("{again:?}");
        };
        assert!(message.contains("allowed after"), "{message}");
        assert_eq!(fixture.requests(), 1, "no request was made");
        assert!(!again.unwrap().changes_library());

        // Almost a day later is still too early; a full day later it runs again.
        age_title_dump(&db, DAY - 60);
        assert!(is_skipped(&run_http(&db, &Command::TitlesDump, &fixture)));
        age_title_dump(&db, DAY + 1);
        assert!(matches!(
            run_http(&db, &Command::TitlesDump, &fixture),
            Ok(Outcome::Updated(_))
        ));
        assert_eq!(fixture.requests(), 2);
    }

    #[test]
    fn failed_truncated_and_invalid_dumps_still_block_a_repeat() {
        let cases = [
            ("truncated", HttpReply::truncated(gzip(DUMP))),
            ("invalid document", HttpReply::ok(gzip("<other/>"))),
            ("server error", HttpReply::status("503 Service Unavailable")),
        ];
        for (name, reply) in cases {
            let db = Db::memory().unwrap();
            let fixture = HttpFixture::start(move |_, _| reply.clone());
            let failed = run_http(&db, &Command::TitlesDump, &fixture);
            assert!(failed.is_err(), "{name}: {failed:?}");
            assert_eq!(fixture.requests(), 1);
            assert_eq!(
                db.meta(TITLES_DUMP_DATE_KEY).unwrap(),
                None,
                "{name}: no success recorded"
            );
            assert!(db.titles_dump_last_request().unwrap().is_some(), "{name}");

            // Whether the failure was transient (a retry) or permanent, no second request.
            let again = run_http(&db, &Command::TitlesDump, &fixture);
            assert!(is_skipped(&again), "{name}: {again:?}");
            assert_eq!(fixture.requests(), 1, "{name}");
        }
    }

    #[test]
    fn the_title_dump_attempt_is_saved_before_the_request_and_survives_a_restart() {
        let path = temp_db_path("dump-restart");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (log, location) = (seen.clone(), path.clone());
        // The process "dies" while the request is in flight: the answer never completes.
        let fixture = HttpFixture::start(move |_, _| {
            let db = Db::open(&location).unwrap();
            log.lock().push(db.meta(TITLES_DUMP_ATTEMPT_KEY).unwrap());
            HttpReply::truncated(gzip(DUMP))
        });
        {
            let db = Db::open(&path).unwrap();
            assert!(run_http(&db, &Command::TitlesDump, &fixture).is_err());
        }
        assert!(
            seen.lock()[0].is_some(),
            "the attempt was already saved when the request arrived"
        );
        // A restart: a new connection and new clients.
        let db = Db::open(&path).unwrap();
        assert!(is_skipped(&run_http(&db, &Command::TitlesDump, &fixture)));
        assert_eq!(fixture.requests(), 1);
        drop(db);
        remove_db_files(&path);
    }

    #[test]
    fn a_too_early_anime_fetch_is_refused_without_a_request() {
        let db = Db::memory().unwrap();
        let now = format::now();
        db.record_request_attempt(RequestKind::Anime, 1, now - 3600)
            .unwrap();
        // Answers with the record of the anime that was asked for, like the real API.
        let fixture = HttpFixture::start(|_, head| {
            let aid = head
                .split("aid=")
                .nth(1)
                .and_then(|rest| rest.split([' ', '&']).next())
                .unwrap_or("0");
            HttpReply::ok(
                ANIME
                    .replace("id=\"1\"", &format!("id=\"{aid}\""))
                    .into_bytes(),
            )
        });

        let early = run_http(&db, &Command::FetchAnime { aid: 1 }, &fixture);
        let Ok(Outcome::Skipped(message)) = &early else {
            panic!("{early:?}");
        };
        assert!(
            message.contains("Anime 1") && message.contains("allowed after"),
            "{message}"
        );
        assert_eq!(fixture.requests(), 0);
        assert_eq!(
            db.request_last_attempt(RequestKind::Anime, 1).unwrap(),
            Some(now - 3600),
            "a refusal is not an attempt"
        );

        // Another anime is not affected; a day later the first one runs too.
        let other = run_http(&db, &Command::FetchAnime { aid: 2 }, &fixture);
        assert!(matches!(other, Ok(Outcome::Updated(_))), "{other:?}");
        assert_eq!(fixture.requests(), 1);
        db.record_request_attempt(RequestKind::Anime, 1, now - DAY - 1)
            .unwrap();
        assert!(matches!(
            run_http(&db, &Command::FetchAnime { aid: 1 }, &fixture),
            Ok(Outcome::Updated(_))
        ));
        assert_eq!(fixture.requests(), 2);
    }

    #[test]
    fn a_failed_anime_fetch_still_blocks_a_repeat_and_is_no_success() {
        let failures = [
            (
                "server error",
                HttpReply::status("500 Internal Server Error"),
            ),
            (
                "api error",
                HttpReply::ok(b"<error code=\"500\">Banned</error>".to_vec()),
            ),
            ("truncated", HttpReply::truncated(ANIME.as_bytes().to_vec())),
        ];
        for (name, reply) in failures {
            let db = Db::memory().unwrap();
            let fixture = HttpFixture::start(move |_, _| reply.clone());
            let cmd = Command::FetchAnime { aid: 1 };
            assert!(run_http(&db, &cmd, &fixture).is_err(), "{name}");
            assert_eq!(fixture.requests(), 1, "{name}");
            let fetched: Option<i64> = db
                .conn()
                .query_row("SELECT fetched FROM anime WHERE aid = 1", [], |r| r.get(0))
                .ok();
            assert!(
                fetched.is_none_or(|f| f == 0),
                "{name}: fetched means success only"
            );
            assert!(is_skipped(&run_http(&db, &cmd, &fixture)), "{name}");
            assert_eq!(fixture.requests(), 1, "{name}");
        }
    }

    #[test]
    fn the_anime_attempt_is_saved_before_the_request_and_survives_a_restart() {
        let path = temp_db_path("anime-restart");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (log, location) = (seen.clone(), path.clone());
        let fixture = HttpFixture::start(move |_, _| {
            let db = Db::open(&location).unwrap();
            log.lock()
                .push(db.request_last_attempt(RequestKind::Anime, 1).unwrap());
            HttpReply::truncated(ANIME.as_bytes().to_vec())
        });
        {
            let db = Db::open(&path).unwrap();
            assert!(run_http(&db, &Command::FetchAnime { aid: 1 }, &fixture).is_err());
        }
        assert!(seen.lock()[0].is_some(), "saved before the request arrived");
        let db = Db::open(&path).unwrap();
        assert!(is_skipped(&run_http(
            &db,
            &Command::FetchAnime { aid: 1 },
            &fixture
        )));
        assert_eq!(fixture.requests(), 1);
        drop(db);
        remove_db_files(&path);
    }

    #[test]
    fn an_anime_request_without_a_client_name_is_not_an_attempt() {
        let db = Db::memory().unwrap();
        let fixture = HttpFixture::start(|_, _| HttpReply::ok(ANIME.as_bytes().to_vec()));
        let t = test_limits();
        let result = execute(
            1,
            &Command::FetchAnime { aid: 1 },
            &db,
            &mut idle_udp(&t),
            &mut http_client(&fixture, &t, ""),
        );
        let err = result.unwrap_err();
        assert!(err.fatal, "{err:?}");
        assert_eq!(fixture.requests(), 0);
        assert_eq!(
            db.request_last_attempt(RequestKind::Anime, 1).unwrap(),
            None
        );
    }

    #[test]
    fn a_known_file_is_not_requested_again_for_seven_days() {
        let db = Db::memory().unwrap();
        let server = UdpFixture::start(|_, request| {
            if request.starts_with("AUTH") {
                "200 session 127.0.0.1:1 LOGIN ACCEPTED".into()
            } else {
                "320 NO SUCH FILE".into()
            }
        });
        let t = test_limits();
        let mut udp = udp_client(server.addr, &t);
        let mut http = HttpClient::new("syntheticclient", 1, t.limits.clone());
        let cmd = Command::FetchFile { fid: 5 };
        let mut run = |db: &Db| execute(1, &cmd, db, &mut udp, &mut http);

        assert!(matches!(run(&db), Ok(Outcome::NotFound(_))));
        assert_eq!(server.packets(), 2, "login and one FILE request");
        let again = run(&db);
        let Ok(Outcome::Skipped(message)) = &again else {
            panic!("{again:?}");
        };
        assert!(
            message.contains("File 5") && message.contains("7 days"),
            "{message}"
        );
        assert_eq!(server.packets(), 2, "no packet for a recent file");

        let now = format::now();
        db.record_request_attempt(RequestKind::File, 5, now - 6 * DAY)
            .unwrap();
        assert!(is_skipped(&run(&db)), "six days are too early");
        db.record_request_attempt(RequestKind::File, 5, now - 7 * DAY - 1)
            .unwrap();
        assert!(matches!(run(&db), Ok(Outcome::NotFound(_))));
        assert_eq!(server.packets(), 3, "the session is reused");
    }

    #[test]
    fn a_login_that_fails_leaves_no_file_attempt_behind() {
        let db = Db::memory().unwrap();
        let server = UdpFixture::start(|_, _| "500 LOGIN FAILED".into());
        let t = test_limits();
        let result = execute(
            1,
            &Command::FetchFile { fid: 5 },
            &db,
            &mut udp_client(server.addr, &t),
            &mut HttpClient::new("syntheticclient", 1, t.limits.clone()),
        );
        assert!(result.unwrap_err().fatal);
        assert_eq!(server.packets(), 1);
        assert_eq!(db.request_last_attempt(RequestKind::File, 5).unwrap(), None);
    }
}
