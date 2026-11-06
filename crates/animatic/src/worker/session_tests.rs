//! The kept AniDB session and the automatic runs that use it (roadmap M5.5 and M5.6), against a
//! loopback stand-in for AniDB and a clock that only moves when the limiter sleeps or a test
//! lets time pass.

use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;

use animatic_anidb::limits::Clock;
use animatic_anidb::testing::{ManualClock, UdpFixture as Fixture};
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
            "animatic-session-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let path = dir.join("library.db");
        TempDb { dir, path }
    }

    fn open(&self) -> Db {
        Db::open(&self.path).unwrap()
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn mins(n: u64) -> Duration {
    Duration::from_secs(n * 60)
}

fn settings(server: SocketAddr, automatic: bool) -> AnidbSettings {
    AnidbSettings {
        username: "synthetic-user".into(),
        password: "synthetic-password".into(),
        client_name: "syntheticclient".into(),
        udp_server: server.to_string(),
        local_port: 0,
        auto_run_queue: automatic,
        idle_logout_minutes: 10,
        ..AnidbSettings::default()
    }
}

/// What the stand-in server received: the command and when (on the test clock).
type Log = Arc<Mutex<Vec<(String, i64)>>>;

/// A stand-in AniDB: accepts the login (reporting the client's own port, or another one with
/// `nat`), keeps sessions, answers `MYLIST` with `mylist_reply`, and the upkeep commands.
fn server(
    nat: bool,
    clock: &Arc<ManualClock>,
    mylist_reply: &Arc<Mutex<String>>,
) -> (Fixture, Log) {
    let log: Log = Arc::default();
    let (seen, clock, mylist_reply) = (log.clone(), clock.clone(), mylist_reply.clone());
    let fixture = Fixture::start_from(move |_, request, peer| {
        let name = request
            .split([' ', '&'])
            .next()
            .unwrap_or_default()
            .to_string();
        seen.lock().push((name.clone(), clock.now_ms()));
        let port = if nat { peer.port() ^ 1 } else { peer.port() };
        match name.as_str() {
            "AUTH" => format!("200 session {}:{port} LOGIN ACCEPTED", peer.ip()),
            "MYLIST" => mylist_reply.lock().clone(),
            "MYLISTDEL" => "211 ENTRY DELETED".into(),
            "UPTIME" => "208 UPTIME\n100".into(),
            "PING" => format!("300 PONG\n{port}"),
            _ => "203 LOGGED OUT".into(),
        }
    });
    (fixture, log)
}

struct Rig {
    db: TempDb,
    clock: Arc<ManualClock>,
    server: Fixture,
    log: Log,
    mylist_reply: Arc<Mutex<String>>,
    network: Network,
    open: Arc<AtomicBool>,
    tx: Sender<Event>,
    events: Receiver<Event>,
}

impl Rig {
    fn new(nat: bool) -> Rig {
        let db = TempDb::new();
        drop(db.open());
        let clock = Arc::new(ManualClock::default());
        let mylist_reply = Arc::new(Mutex::new("321 NO SUCH ENTRY".to_string()));
        let (server, log) = server(nat, &clock, &mylist_reply);
        let open = Arc::new(AtomicBool::new(false));
        let network = Network {
            limits: animatic_core::limits::open_limits_with_clock(&db.path, clock.clone()),
            udp_timeout: Duration::from_secs(5),
            session: SessionSlot::new(open.clone()),
        };
        let (tx, events) = crossbeam_channel::unbounded();
        Rig {
            db,
            clock,
            server,
            log,
            mylist_reply,
            network,
            open,
            tx,
            events,
        }
    }

    fn settings(&self, automatic: bool) -> AnidbSettings {
        settings(self.server.addr, automatic)
    }

    fn refresh(&self, lid: i64) {
        self.db
            .open()
            .queue_push(&Command::MylistRefresh { lid })
            .unwrap();
    }

    fn delete(&self, lid: i64) {
        self.db
            .open()
            .queue_push(&Command::MylistDelete { lid })
            .unwrap();
    }

    fn queued(&self) -> i64 {
        self.db.open().queue_len().unwrap()
    }

    fn manual_run(&self, settings: &AnidbSettings) -> Vec<Event> {
        process_queue(
            &self.db.path,
            settings.clone(),
            &self.tx,
            &AtomicBool::new(false),
            &self.network,
        );
        self.events.try_iter().collect()
    }

    fn auto_run(&self, settings: &AnidbSettings) -> Vec<Event> {
        process_auto_queue(
            &self.db.path,
            settings.clone(),
            &self.tx,
            &AtomicBool::new(false),
            &self.network,
        );
        self.events.try_iter().collect()
    }

    /// What the worker does before it waits for the next job.
    fn maintain(&self, settings: &AnidbSettings) -> Option<Duration> {
        self.network
            .session
            .maintain(&self.db.path, settings, &self.tx)
    }

    fn names(&self) -> Vec<String> {
        self.log.lock().iter().map(|(n, _)| n.clone()).collect()
    }

    fn pass(&self, time: Duration) {
        self.clock.advance(time);
    }

    fn infos(&self) -> Vec<String> {
        self.events
            .try_iter()
            .filter_map(|e| match e {
                Event::Info(text) => Some(text),
                _ => None,
            })
            .collect()
    }
}

fn has_error(events: &[Event], needle: &str) -> bool {
    events
        .iter()
        .any(|e| matches!(e, Event::Error(m) if m.contains(needle)))
}

#[test]
fn consecutive_automatic_runs_reuse_one_session_and_keep_the_packet_spacing() {
    let rig = Rig::new(false);
    let auto = rig.settings(true);

    rig.refresh(1);
    rig.auto_run(&auto);
    assert_eq!(
        rig.names(),
        ["AUTH", "MYLIST"],
        "no logout: the session is kept"
    );
    assert!(rig.network.session.is_kept());
    assert!(
        rig.open.load(Ordering::SeqCst),
        "an exit would wait for a logout"
    );

    rig.pass(mins(5));
    rig.refresh(2);
    rig.refresh(3);
    rig.auto_run(&auto);
    assert_eq!(
        rig.names(),
        ["AUTH", "MYLIST", "MYLIST", "MYLIST"],
        "one login for both runs"
    );

    let times: Vec<i64> = rig.log.lock().iter().map(|(_, at)| *at).collect();
    for pair in times.windows(2) {
        assert!(
            pair[1] - pair[0] >= 2000,
            "AniDB's spacing holds: {times:?}"
        );
    }
    assert_eq!(rig.queued(), 0);
}

#[test]
fn without_automatic_runs_every_run_still_logs_in_and_out() {
    let rig = Rig::new(false);
    let manual = rig.settings(false);
    rig.refresh(1);
    rig.manual_run(&manual);
    assert_eq!(rig.names(), ["AUTH", "MYLIST", "LOGOUT"]);
    assert!(!rig.network.session.is_kept());
    assert!(
        !rig.open.load(Ordering::SeqCst),
        "nothing to wait for at exit"
    );

    rig.pass(mins(1));
    rig.refresh(2);
    rig.manual_run(&manual);
    assert_eq!(
        rig.names(),
        ["AUTH", "MYLIST", "LOGOUT", "AUTH", "MYLIST", "LOGOUT"]
    );
    assert_eq!(rig.maintain(&manual), None, "nothing is kept to look after");
}

#[test]
fn a_manual_run_while_automatic_runs_are_on_shares_the_session() {
    let rig = Rig::new(false);
    let auto = rig.settings(true);
    rig.refresh(1);
    rig.auto_run(&auto);
    rig.refresh(2);
    rig.manual_run(&auto);
    assert_eq!(rig.names(), ["AUTH", "MYLIST", "MYLIST"]);
}

#[test]
fn an_automatic_run_skips_mylist_deletes_but_sends_everything_else() {
    let rig = Rig::new(false);
    let auto = rig.settings(true);
    rig.delete(5);
    rig.refresh(1);

    let events = rig.auto_run(&auto);
    assert_eq!(
        rig.names(),
        ["AUTH", "MYLIST"],
        "the refresh went out, the removal did not"
    );
    assert_eq!(rig.queued(), 1, "the removal stays queued");
    assert!(
        !events.iter().any(|e| matches!(e, Event::Error(_))),
        "{events:?}"
    );

    // The user's run sends it, on the same session.
    rig.manual_run(&auto);
    assert_eq!(rig.names(), ["AUTH", "MYLIST", "MYLISTDEL"]);
    assert_eq!(rig.queued(), 0);
}

#[test]
fn an_automatic_run_with_only_removals_sends_nothing_and_says_nothing() {
    let rig = Rig::new(false);
    rig.delete(5);
    let events = rig.auto_run(&rig.settings(true));
    assert!(rig.names().is_empty());
    assert!(
        events.is_empty(),
        "no started or finished either: {events:?}"
    );
    assert_eq!(rig.queued(), 1);
}

#[test]
fn an_automatic_run_does_nothing_while_offline_or_cooling_down() {
    let rig = Rig::new(false);
    rig.refresh(1);

    let mut offline = rig.settings(true);
    offline.offline = true;
    let events = rig.auto_run(&offline);
    assert!(has_error(&events, "Offline mode"), "{events:?}");
    assert!(rig.names().is_empty());

    rig.db
        .open()
        .set_sync_cooldown(format::now() + 3600, "synthetic ban")
        .unwrap();
    let events = rig.auto_run(&rig.settings(true));
    assert!(has_error(&events, "paused"), "{events:?}");
    assert!(rig.names().is_empty());
    assert_eq!(rig.queued(), 1, "everything stays queued");
}

#[test]
fn an_idle_session_logs_out_after_the_configured_minutes() {
    let rig = Rig::new(false);
    let mut auto = rig.settings(true);
    auto.idle_logout_minutes = 5;
    rig.refresh(1);
    rig.auto_run(&auto);

    assert_eq!(
        rig.maintain(&auto),
        Some(mins(5)),
        "the idle time, from the last command"
    );
    rig.pass(mins(5) - Duration::from_secs(1));
    assert_eq!(rig.maintain(&auto), Some(Duration::from_secs(1)));
    assert_eq!(rig.names(), ["AUTH", "MYLIST"], "not logged out yet");
    assert!(rig.network.session.is_kept());

    rig.pass(Duration::from_secs(1));
    assert_eq!(rig.maintain(&auto), None);
    assert_eq!(rig.names(), ["AUTH", "MYLIST", "LOGOUT"]);
    assert!(!rig.network.session.is_kept());
    assert!(!rig.open.load(Ordering::SeqCst));
    assert!(
        rig.infos()
            .iter()
            .any(|n| n.contains("Logged out of AniDB")),
        "the logout is in the activity log"
    );

    // The next run logs in again.
    rig.refresh(2);
    rig.auto_run(&auto);
    assert_eq!(rig.names(), ["AUTH", "MYLIST", "LOGOUT", "AUTH", "MYLIST"]);
}

#[test]
fn a_session_with_waiting_work_is_kept_alive_instead_of_logged_out() {
    let rig = Rig::new(false);
    let auto = rig.settings(true);
    rig.refresh(1);
    rig.auto_run(&auto);

    // A command arrived after the run: more work is expected, so the session stays.
    rig.db
        .open()
        .queue_push(&Command::FetchAnime { aid: 1 })
        .unwrap();
    assert_eq!(rig.maintain(&auto), Some(mins(30)));
    rig.pass(mins(29));
    rig.maintain(&auto);
    assert_eq!(
        rig.names(),
        ["AUTH", "MYLIST"],
        "10 idle minutes do not log it out"
    );
    rig.pass(mins(1));
    assert_eq!(rig.maintain(&auto), Some(mins(30)));
    assert_eq!(
        rig.names(),
        ["AUTH", "MYLIST", "UPTIME"],
        "a command every 30 minutes"
    );

    // The work is gone: the idle time applies from the keep-alive command.
    rig.db.open().queue_clear().unwrap();
    rig.pass(mins(10) - Duration::from_secs(1));
    rig.maintain(&auto);
    assert_eq!(rig.names().len(), 3);
    rig.pass(Duration::from_secs(1));
    assert_eq!(rig.maintain(&auto), None);
    assert_eq!(rig.names(), ["AUTH", "MYLIST", "UPTIME", "LOGOUT"]);
}

#[test]
fn a_queued_removal_is_not_work_that_keeps_the_session() {
    let rig = Rig::new(false);
    let auto = rig.settings(true);
    rig.refresh(1);
    rig.auto_run(&auto);
    rig.delete(5);
    assert_eq!(
        rig.maintain(&auto),
        Some(mins(10)),
        "an automatic run would not send it, so no run is coming"
    );
}

#[test]
fn a_router_that_rewrites_the_port_gets_a_ping_every_4_minutes_until_the_logout() {
    let rig = Rig::new(true);
    let auto = rig.settings(true);
    rig.refresh(1);
    rig.auto_run(&auto);

    let mut wake = rig.maintain(&auto);
    assert_eq!(wake, Some(mins(4)));
    for _ in 0..2 {
        rig.pass(wake.unwrap());
        wake = rig.maintain(&auto);
    }
    assert_eq!(rig.names(), ["AUTH", "MYLIST", "PING", "PING"]);
    rig.pass(wake.unwrap());
    assert_eq!(rig.maintain(&auto), None);
    assert_eq!(rig.names(), ["AUTH", "MYLIST", "PING", "PING", "LOGOUT"]);
}

#[test]
fn nothing_is_sent_to_a_kept_session_while_offline_or_cooling_down() {
    let rig = Rig::new(false);
    let auto = rig.settings(true);
    rig.refresh(1);
    rig.auto_run(&auto);

    // Offline: the idle time passes without a logout packet, and the expired session is simply
    // forgotten once the user is back.
    let mut offline = auto.clone();
    offline.offline = true;
    rig.pass(mins(40));
    assert!(rig.maintain(&offline).is_some(), "it looks again later");
    assert_eq!(rig.names(), ["AUTH", "MYLIST"]);
    assert_eq!(rig.maintain(&auto), None);
    assert_eq!(
        rig.names(),
        ["AUTH", "MYLIST"],
        "AniDB has no session to log out"
    );
    assert!(
        rig.infos().iter().any(|n| n.contains("ran out")),
        "said so in the activity log"
    );

    // Cooling down: the logout waits until the cooldown is over.
    rig.refresh(2);
    rig.auto_run(&auto);
    rig.db
        .open()
        .set_sync_cooldown(format::now() + 3600, "synthetic ban")
        .unwrap();
    rig.pass(mins(11));
    assert!(rig.maintain(&auto).is_some());
    assert_eq!(rig.names().len(), 4, "AUTH, MYLIST, AUTH, MYLIST only");
    rig.db.open().clear_sync_cooldown().unwrap();
    assert_eq!(rig.maintain(&auto), None);
    assert_eq!(rig.names().last().map(String::as_str), Some("LOGOUT"));
}

#[test]
fn switching_automatic_runs_off_ends_a_kept_session() {
    let rig = Rig::new(false);
    let auto = rig.settings(true);
    rig.refresh(1);
    rig.auto_run(&auto);

    assert_eq!(rig.maintain(&rig.settings(false)), None);
    assert_eq!(rig.names(), ["AUTH", "MYLIST", "LOGOUT"]);
    assert!(
        rig.infos()
            .iter()
            .any(|n| n.contains("automatic queue runs are off"))
    );
}

#[test]
fn a_changed_account_never_reuses_the_session_of_the_old_one() {
    let rig = Rig::new(false);
    let auto = rig.settings(true);
    rig.refresh(1);
    rig.auto_run(&auto);

    let mut other = auto.clone();
    other.password = "another-synthetic-password".into();
    rig.refresh(2);
    rig.auto_run(&other);
    assert_eq!(rig.names(), ["AUTH", "MYLIST", "AUTH", "MYLIST"]);
}

#[test]
fn a_ban_drops_the_session_so_the_run_after_the_cooldown_starts_clean() {
    let rig = Rig::new(false);
    let auto = rig.settings(true);
    *rig.mylist_reply.lock() = "555 BANNED".into();
    rig.refresh(1);
    let events = rig.auto_run(&auto);
    assert!(
        events.iter().any(|e| matches!(e, Event::Cooldown { .. })),
        "{events:?}"
    );
    assert_eq!(rig.names(), ["AUTH", "MYLIST"]);
    assert!(
        !rig.network.session.is_kept(),
        "a banned client is never kept"
    );

    // After the cooldown a new run logs in and sends: it is not refused by the old ban.
    rig.db.open().clear_sync_cooldown().unwrap();
    *rig.mylist_reply.lock() = "321 NO SUCH ENTRY".into();
    let events = rig.auto_run(&auto);
    assert!(
        !events.iter().any(|e| matches!(e, Event::Error(_))),
        "{events:?}"
    );
    assert_eq!(rig.names(), ["AUTH", "MYLIST", "AUTH", "MYLIST"]);
    assert_eq!(rig.queued(), 0);
}

#[test]
fn leaving_logs_out_of_a_kept_session_but_stays_silent_offline_or_cooling_down() {
    let rig = Rig::new(false);
    let auto = rig.settings(true);
    rig.refresh(1);
    rig.auto_run(&auto);
    rig.network.session.logout_on_exit(&rig.db.path, &auto);
    assert_eq!(rig.names(), ["AUTH", "MYLIST", "LOGOUT"]);
    assert!(!rig.network.session.is_kept());
    assert!(!rig.open.load(Ordering::SeqCst));

    // Leaving without a session sends nothing.
    rig.network.session.logout_on_exit(&rig.db.path, &auto);
    assert_eq!(rig.names().len(), 3);

    // Offline: AniDB drops the session by itself.
    rig.refresh(2);
    rig.auto_run(&auto);
    let mut offline = auto.clone();
    offline.offline = true;
    rig.network.session.logout_on_exit(&rig.db.path, &offline);
    assert_eq!(rig.names().len(), 5);
    assert!(!rig.network.session.is_kept());

    // Cooling down: the same.
    rig.refresh(3);
    rig.auto_run(&auto);
    rig.db
        .open()
        .set_sync_cooldown(format::now() + 3600, "synthetic ban")
        .unwrap();
    rig.network.session.logout_on_exit(&rig.db.path, &auto);
    assert_eq!(rig.names().len(), 7);
    assert!(!rig.network.session.is_kept());
}

#[test]
fn closing_the_worker_logs_out_of_the_kept_session_before_the_process_can_end() {
    // The real worker on the real clock: login, one command (the limiter spaces them 2 s),
    // then the logout on exit (2 s more).
    let db = TempDb::new();
    drop(db.open());
    let clock = Arc::new(ManualClock::default());
    let mylist_reply = Arc::new(Mutex::new("321 NO SUCH ENTRY".to_string()));
    let (server, log) = self::server(false, &clock, &mylist_reply);
    let auto = settings(server.addr, true);
    db.open()
        .queue_push(&Command::MylistRefresh { lid: 1 })
        .unwrap();

    let worker = Worker::spawn(db.path.clone());
    worker.queue_control().sync(&auto, None);
    worker.submit(Job::AutoProcessQueue(auto));
    let mut finished = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !finished && Instant::now() < deadline {
        if let Ok(event) = worker.events.recv_timeout(Duration::from_millis(100)) {
            finished = matches!(event, Event::Finished);
        }
    }
    assert!(finished, "the run finished");
    let names = |log: &Log| {
        log.lock()
            .iter()
            .map(|(n, _)| n.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(&log),
        ["AUTH", "MYLIST"],
        "the session is kept after the run"
    );

    let started = Instant::now();
    drop(worker);
    assert_eq!(
        names(&log),
        ["AUTH", "MYLIST", "LOGOUT"],
        "closing waited for the logout"
    );
    assert!(
        started.elapsed() < EXIT_LOGOUT_WAIT + Duration::from_secs(2),
        "the wait is bounded"
    );
}

#[test]
fn closing_a_worker_without_a_session_does_not_wait() {
    let db = TempDb::new();
    drop(db.open());
    let worker = Worker::spawn(db.path.clone());
    let started = Instant::now();
    drop(worker);
    assert!(started.elapsed() < Duration::from_secs(2));
}
