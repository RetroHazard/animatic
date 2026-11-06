//! Opt-in automatic queue runs (roadmap M5.5).
//!
//! With **Run the command queue automatically** on, a dedicated thread submits the ordinary
//! [`Job::AutoProcessQueue`] shortly after new commands arrive (a debounce, so a burst of edits
//! becomes one run) and at an interval for whatever is still waiting. It follows the pattern of
//! the folder-scan scheduler ([`crate::scheduler`]): the timing decision is the pure
//! [`QueueSchedule`], driven by explicit `Instant`s so tests control the clock, and a due run
//! is only handed to the worker through [`submit_if_idle`], so it never overlaps a running
//! job, a manual run or an import review.
//!
//! A run is held back, and starts when that is over, while Work offline is on, a persisted
//! cooldown is active, the account is not set, or the UI reports an import,
//! a settings review, a restore or unsaved commands. Blocked commands are not work: they wait
//! for an explicit retry. `MylistDelete` is not work either: an automatic run never sends it
//! (see [`QueueScope::Automatic`]), so a queue that holds nothing else starts no run at all.
//! Every packet of a run takes its slot from the shared limiter, so however many commands a
//! burst queues, a run never sends faster than AniDB allows.
//!
//! Commands are queued from many places (the UI thread, the worker after hashing, full updates),
//! so the thread watches the queue in the database instead of being told: ids are never
//! reused, so a newer id or a higher count than at the last look means commands arrived.

use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use animatic_core::db::Db;
use animatic_core::settings::AnidbSettings;
use animatic_core::sync::{QueuePending, QueueScope};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::scheduler::Tick;
use crate::worker::{Event, Job, submit_if_idle};

/// How long new commands wait for more before a run starts, counted from the newest one.
pub const DEBOUNCE: Duration = Duration::from_secs(30);

/// How often a run is considered for whatever is still queued (commands that a run left
/// because it was stopped, blocked by a cooldown or postponed), counted from the last start.
pub const INTERVAL: Duration = Duration::from_secs(15 * 60);

/// How often the thread looks at the settings and the queue.
pub const POLL: Duration = Duration::from_secs(2);

/// The timing of the scheduler thread; tests shorten it.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub poll: Duration,
    pub debounce: Duration,
    pub interval: Duration,
}

impl Timing {
    pub const DEFAULT: Timing = Timing {
        poll: POLL,
        debounce: DEBOUNCE,
        interval: INTERVAL,
    };
}

/// When automatic runs are due. Pure state: the caller supplies the time, what the queue
/// holds and whether a run may start.
#[derive(Debug)]
pub struct QueueSchedule {
    enabled: bool,
    debounce: Duration,
    interval: Duration,
    /// When the debounce after the newest commands runs out.
    debounce_due: Option<Instant>,
    /// When the next interval run is due; counted from enabling and from every start.
    interval_due: Option<Instant>,
    /// What the queue looked like at the last look.
    seen: Option<QueuePending>,
    reported: bool,
}

impl QueueSchedule {
    pub fn new(now: Instant, enabled: bool, debounce: Duration, interval: Duration) -> Self {
        QueueSchedule {
            enabled,
            debounce,
            interval,
            debounce_due: None,
            interval_due: enabled.then(|| now + interval),
            seen: None,
            reported: false,
        }
    }

    /// Adopt the setting. Switching on counts the interval from `now` and treats what is
    /// already queued as new, so it runs after one debounce; switching off forgets everything.
    pub fn configure(&mut self, now: Instant, enabled: bool) {
        if enabled == self.enabled {
            return;
        }
        self.enabled = enabled;
        self.debounce_due = None;
        self.seen = None;
        self.reported = false;
        self.interval_due = enabled.then(|| now + self.interval);
    }

    /// Look at the queue (`pending`, `None` when it could not be read) and, if a run is due,
    /// ask `start` to submit it. `start` returns why it cannot start now. A run that cannot
    /// start stays due; both timers restart when one does.
    pub fn poll(
        &mut self,
        now: Instant,
        pending: Option<QueuePending>,
        start: &mut dyn FnMut() -> Result<(), &'static str>,
    ) -> Tick {
        if !self.enabled {
            return Tick::Idle;
        }
        if let Some(p) = pending {
            // Our own runs only ever shrink the queue: that is not news.
            let arrived = p.count > 0
                && self
                    .seen
                    .is_none_or(|s| p.newest > s.newest || p.count > s.count);
            if arrived {
                self.debounce_due = Some(now + self.debounce);
            }
            if p.count == 0 {
                self.debounce_due = None;
                self.reported = false;
            }
            self.seen = Some(p);
        }
        let due = [self.debounce_due, self.interval_due]
            .into_iter()
            .flatten()
            .any(|at| at <= now);
        if !due {
            return Tick::Idle;
        }
        match pending {
            Some(QueuePending { count: 0, .. }) => {
                // Nothing to send: look again a whole interval from now.
                self.debounce_due = None;
                self.interval_due = Some(now + self.interval);
                self.reported = false;
                return Tick::Idle;
            }
            None => return self.postponed("the command queue could not be read"),
            Some(_) => {}
        }
        match start() {
            Ok(()) => {
                self.debounce_due = None;
                self.interval_due = Some(now + self.interval);
                self.reported = false;
                Tick::Submitted
            }
            Err(reason) => self.postponed(reason),
        }
    }

    fn postponed(&mut self, reason: &'static str) -> Tick {
        let first = !self.reported;
        self.reported = true;
        Tick::Postponed { reason, first }
    }
}

struct Inputs {
    anidb: AnidbSettings,
    blocker: Option<&'static str>,
}

/// What the thread needs from the application: the AniDB settings and whether the application
/// is in a state that forbids starting a run. The worker also reads the settings, to look
/// after a kept session (see [`crate::session`]).
pub struct QueueControl {
    inputs: Mutex<Inputs>,
}

/// The decision inputs of one look.
struct Snapshot {
    enabled: bool,
    blocker: Option<&'static str>,
}

impl QueueControl {
    pub fn new() -> QueueControl {
        QueueControl {
            inputs: Mutex::new(Inputs {
                anidb: AnidbSettings::default(),
                blocker: None,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inputs> {
        self.inputs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Publish only the blocker, leaving the settings as last synced (while Options is open
    /// the half-edited values must never start a run).
    pub fn set_blocker(&self, blocker: Option<&'static str>) {
        self.lock().blocker = blocker;
    }

    /// Publish the current settings and blocker. Cheap when nothing changed.
    pub fn sync(&self, anidb: &AnidbSettings, blocker: Option<&'static str>) {
        let mut inputs = self.lock();
        if inputs.anidb != *anidb {
            inputs.anidb = anidb.clone();
        }
        inputs.blocker = blocker;
    }

    /// The settings as last published.
    pub fn anidb(&self) -> AnidbSettings {
        self.lock().anidb.clone()
    }

    fn snapshot(&self) -> Snapshot {
        let inputs = self.lock();
        let a = &inputs.anidb;
        let blocker = inputs
            .blocker
            .or_else(|| a.offline.then_some("Work offline is on"))
            .or_else(|| {
                (a.username.is_empty() || a.password.is_empty())
                    .then_some("the AniDB account is not set")
            });
        Snapshot {
            enabled: a.auto_run_queue,
            blocker,
        }
    }

    fn job(&self) -> Job {
        Job::AutoProcessQueue(self.lock().anidb.clone())
    }
}

impl Default for QueueControl {
    fn default() -> Self {
        QueueControl::new()
    }
}

/// The queue as the thread sees it. The connection is only held while automatic runs are on,
/// and dropped after any error so the next look opens a fresh one.
struct Watch {
    db_path: PathBuf,
    db: Option<Db>,
}

impl Watch {
    fn db(&mut self) -> Option<&Db> {
        if self.db.is_none() {
            match Db::open(&self.db_path) {
                Ok(db) => self.db = Some(db),
                Err(e) => log::warn!("automatic queue runs cannot open the library: {e}"),
            }
        }
        self.db.as_ref()
    }

    fn pending(&mut self) -> Option<QueuePending> {
        let result = self.db()?.queue_pending(QueueScope::Automatic);
        match result {
            Ok(pending) => Some(pending),
            Err(e) => {
                log::warn!("automatic queue runs cannot read the queue: {e}");
                self.db = None;
                None
            }
        }
    }

    /// Fails closed: a cooldown that cannot be read counts as an active one.
    fn cooling_down(&mut self) -> bool {
        match self.db().map(Db::sync_cooldown) {
            Some(Ok(None)) => false,
            Some(Ok(Some(_))) => true,
            Some(Err(e)) => {
                log::warn!("automatic queue runs cannot read the sync cooldown: {e}");
                self.db = None;
                true
            }
            None => true,
        }
    }
}

/// Start the thread. It ends when `stop` is signalled or its sender is dropped, within one
/// poll, and never starts anything after that.
pub fn spawn(
    control: Arc<QueueControl>,
    db_path: PathBuf,
    jobs: Sender<Job>,
    pending_jobs: Arc<AtomicUsize>,
    events: Sender<Event>,
    stop: Receiver<()>,
    timing: Timing,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("animatic-queue-scheduler".into())
        .spawn(move || {
            let mut schedule = QueueSchedule::new(
                Instant::now(),
                control.snapshot().enabled,
                timing.debounce,
                timing.interval,
            );
            let mut watch = Watch { db_path, db: None };
            loop {
                match stop.recv_timeout(timing.poll) {
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                }
                let now = Instant::now();
                let snapshot = control.snapshot();
                schedule.configure(now, snapshot.enabled);
                if !snapshot.enabled {
                    watch.db = None;
                    continue;
                }
                let pending = watch.pending();
                let tick = schedule.poll(now, pending, &mut || {
                    // Read again, not from the snapshot of this poll: a review that opened
                    // meanwhile must not be raced by a run that was decided a moment ago.
                    if let Some(reason) = control.snapshot().blocker {
                        return Err(reason);
                    }
                    if watch.cooling_down() {
                        return Err("an AniDB cooldown is active");
                    }
                    if submit_if_idle(&jobs, &pending_jobs, control.job()) {
                        Ok(())
                    } else {
                        Err("another task is running")
                    }
                });
                let note = match tick {
                    Tick::Idle => None,
                    Tick::Submitted => Some("Automatic command queue run started".to_string()),
                    Tick::Postponed {
                        reason,
                        first: true,
                    } => Some(format!(
                        "Automatic command queue run postponed ({reason}); it starts when that is over"
                    )),
                    Tick::Postponed { first: false, .. } => None,
                };
                if let Some(note) = note {
                    let _ = events.send(Event::Info(note));
                }
            }
        })
        .expect("start queue scheduler")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use animatic_core::format;
    use animatic_core::sync::Command;

    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    const DEBOUNCE_FOR_TESTS: Duration = Duration::from_secs(30);
    const INTERVAL_FOR_TESTS: Duration = Duration::from_secs(900);

    fn schedule(t0: Instant) -> QueueSchedule {
        QueueSchedule::new(t0, true, DEBOUNCE_FOR_TESTS, INTERVAL_FOR_TESTS)
    }

    fn queue(count: i64, newest: i64) -> Option<QueuePending> {
        Some(QueuePending { count, newest })
    }

    fn ok() -> Result<(), &'static str> {
        Ok(())
    }

    #[test]
    fn new_commands_start_a_run_after_the_debounce_and_every_further_command_restarts_it() {
        let t0 = Instant::now();
        let mut schedule = schedule(t0);
        assert_eq!(schedule.poll(t0, queue(0, 0), &mut ok), Tick::Idle);

        // Two commands arrive; the run waits for the debounce, counted from the newest.
        let arrived = t0 + secs(10);
        assert_eq!(schedule.poll(arrived, queue(1, 1), &mut ok), Tick::Idle);
        assert_eq!(
            schedule.poll(arrived + secs(20), queue(2, 2), &mut ok),
            Tick::Idle
        );
        assert_eq!(
            schedule.poll(arrived + secs(49), queue(2, 2), &mut ok),
            Tick::Idle,
            "30 s after the first command is not yet 30 s after the second"
        );
        assert_eq!(
            schedule.poll(arrived + secs(50), queue(2, 2), &mut ok),
            Tick::Submitted
        );
        // One run for the whole burst.
        assert_eq!(
            schedule.poll(arrived + secs(60), queue(2, 2), &mut ok),
            Tick::Idle
        );
    }

    #[test]
    fn a_run_finishing_the_queue_is_not_news_but_later_commands_and_retries_are() {
        let t0 = Instant::now();
        let mut schedule = schedule(t0);
        assert_eq!(schedule.poll(t0, queue(3, 7), &mut ok), Tick::Idle);
        assert_eq!(
            schedule.poll(t0 + secs(30), queue(3, 7), &mut ok),
            Tick::Submitted
        );
        // The run removes commands one by one: no new debounce.
        for (i, count) in [2, 1, 0].into_iter().enumerate() {
            let at = t0 + secs(31 + i as u64);
            assert_eq!(schedule.poll(at, queue(count, 7), &mut ok), Tick::Idle);
        }
        assert_eq!(
            schedule.poll(t0 + secs(120), queue(0, 0), &mut ok),
            Tick::Idle
        );

        // A command queued afterwards is news ...
        let later = t0 + secs(200);
        assert_eq!(schedule.poll(later, queue(1, 8), &mut ok), Tick::Idle);
        assert_eq!(
            schedule.poll(later + secs(30), queue(1, 8), &mut ok),
            Tick::Submitted
        );
        // ... and so is a blocked command that the user retried: the count grows although the
        // newest id does not.
        assert_eq!(
            schedule.poll(later + secs(60), queue(0, 0), &mut ok),
            Tick::Idle
        );
        let waiting = later + secs(100);
        assert_eq!(schedule.poll(waiting, queue(1, 4), &mut ok), Tick::Idle);
        assert_eq!(
            schedule.poll(waiting + secs(30), queue(1, 4), &mut ok),
            Tick::Submitted
        );
        let retried = waiting + secs(40);
        assert_eq!(schedule.poll(retried, queue(2, 4), &mut ok), Tick::Idle);
        assert_eq!(
            schedule.poll(retried + secs(30), queue(2, 4), &mut ok),
            Tick::Submitted
        );
    }

    #[test]
    fn the_interval_sends_what_is_still_waiting_and_an_empty_queue_starts_nothing() {
        let t0 = Instant::now();
        let mut schedule = schedule(t0);
        // Commands that a stopped run left behind: seen at the start, run at the debounce,
        // left behind again (the run was refused), then picked up by the interval.
        assert_eq!(schedule.poll(t0, queue(2, 5), &mut ok), Tick::Idle);
        assert_eq!(
            schedule.poll(t0 + secs(30), queue(2, 5), &mut ok),
            Tick::Submitted
        );
        let before = t0 + secs(30) + INTERVAL_FOR_TESTS - secs(1);
        assert_eq!(schedule.poll(before, queue(2, 5), &mut ok), Tick::Idle);
        assert_eq!(
            schedule.poll(before + secs(1), queue(2, 5), &mut ok),
            Tick::Submitted,
            "the interval counts from the last start"
        );

        // Nothing queued at the interval: nothing starts, and the next look is a whole
        // interval away.
        let empty = before + INTERVAL_FOR_TESTS + secs(1);
        let mut never = || -> Result<(), &'static str> { panic!("no run for an empty queue") };
        assert_eq!(schedule.poll(empty, queue(0, 0), &mut never), Tick::Idle);
        assert_eq!(
            schedule.poll(
                empty + INTERVAL_FOR_TESTS - secs(1),
                queue(0, 0),
                &mut never
            ),
            Tick::Idle
        );
    }

    #[test]
    fn a_refused_run_stays_due_is_reported_once_and_starts_when_nothing_blocks_it() {
        let t0 = Instant::now();
        let mut schedule = schedule(t0);
        let mut blocked = || Err("Work offline is on");
        assert_eq!(schedule.poll(t0, queue(1, 1), &mut blocked), Tick::Idle);

        let due = t0 + secs(30);
        assert_eq!(
            schedule.poll(due, queue(1, 1), &mut blocked),
            Tick::Postponed {
                reason: "Work offline is on",
                first: true
            }
        );
        assert_eq!(
            schedule.poll(due + secs(2), queue(1, 1), &mut blocked),
            Tick::Postponed {
                reason: "Work offline is on",
                first: false
            }
        );
        // An unreadable queue is a reason too.
        assert!(matches!(
            schedule.poll(due + secs(4), None, &mut ok),
            Tick::Postponed { first: false, .. }
        ));
        assert_eq!(
            schedule.poll(due + secs(600), queue(1, 1), &mut ok),
            Tick::Submitted
        );
        // A later refusal is reported again.
        let later = due + secs(600) + INTERVAL_FOR_TESTS;
        assert!(matches!(
            schedule.poll(later, queue(1, 1), &mut blocked),
            Tick::Postponed { first: true, .. }
        ));
    }

    #[test]
    fn a_refusal_that_ends_because_the_queue_emptied_is_forgotten() {
        let t0 = Instant::now();
        let mut schedule = schedule(t0);
        let mut blocked = || Err("another task is running");
        assert_eq!(schedule.poll(t0, queue(1, 1), &mut blocked), Tick::Idle);
        assert!(matches!(
            schedule.poll(t0 + secs(30), queue(1, 1), &mut blocked),
            Tick::Postponed { first: true, .. }
        ));
        // The user cleared the queue.
        assert_eq!(
            schedule.poll(t0 + secs(32), queue(0, 0), &mut blocked),
            Tick::Idle
        );
        assert_eq!(
            schedule.poll(t0 + secs(40), queue(1, 2), &mut blocked),
            Tick::Idle
        );
        assert!(matches!(
            schedule.poll(t0 + secs(70), queue(1, 2), &mut blocked),
            Tick::Postponed { first: true, .. }
        ));
    }

    #[test]
    fn a_disabled_schedule_never_fires_and_enabling_treats_the_queue_as_new() {
        let t0 = Instant::now();
        let mut schedule = QueueSchedule::new(t0, false, DEBOUNCE_FOR_TESTS, INTERVAL_FOR_TESTS);
        assert_eq!(
            schedule.poll(t0 + INTERVAL_FOR_TESTS * 10, queue(5, 9), &mut ok),
            Tick::Idle
        );

        let on = t0 + secs(100);
        schedule.configure(on, true);
        assert_eq!(schedule.poll(on, queue(5, 9), &mut ok), Tick::Idle);
        assert_eq!(
            schedule.poll(on + secs(30), queue(5, 9), &mut ok),
            Tick::Submitted,
            "the commands queued while it was off run one debounce after switching on"
        );

        // Switching off forgets a due run.
        let mut blocked = || Err("busy");
        let later = on + secs(30) + INTERVAL_FOR_TESTS + secs(5);
        assert!(matches!(
            schedule.poll(later, queue(5, 9), &mut blocked),
            Tick::Postponed { .. }
        ));
        schedule.configure(later, false);
        assert_eq!(schedule.poll(later, queue(5, 9), &mut ok), Tick::Idle);
    }

    // The thread, over a real database and with shortened timing.

    struct TempDb {
        dir: PathBuf,
        path: PathBuf,
    }

    impl TempDb {
        fn new() -> TempDb {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "animatic-auto-queue-test-{}-{}",
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

    fn settings(auto: bool) -> AnidbSettings {
        AnidbSettings {
            username: "animatic-test-user".into(),
            password: "synthetic-password".into(),
            auto_run_queue: auto,
            ..AnidbSettings::default()
        }
    }

    const TEST_TIMING: Timing = Timing {
        poll: Duration::from_millis(15),
        debounce: Duration::from_millis(120),
        interval: Duration::from_secs(3600),
    };

    struct Harness {
        db: TempDb,
        control: Arc<QueueControl>,
        jobs: Receiver<Job>,
        pending: Arc<AtomicUsize>,
        events: Receiver<Event>,
        stop: Sender<()>,
        thread: JoinHandle<()>,
    }

    fn harness() -> Harness {
        let db = TempDb::new();
        drop(db.open());
        let control = Arc::new(QueueControl::new());
        let (job_tx, jobs) = crossbeam_channel::unbounded();
        let (event_tx, events) = crossbeam_channel::unbounded();
        let (stop, stop_rx) = crossbeam_channel::bounded(1);
        let pending = Arc::new(AtomicUsize::new(0));
        let thread = spawn(
            control.clone(),
            db.path.clone(),
            job_tx,
            pending.clone(),
            event_tx,
            stop_rx,
            TEST_TIMING,
        );
        Harness {
            db,
            control,
            jobs,
            pending,
            events,
            stop,
            thread,
        }
    }

    impl Harness {
        fn finish(self) {
            self.stop.send(()).unwrap();
            self.thread.join().unwrap();
        }

        /// Nothing is submitted for `wait`.
        fn stays_quiet(&self, wait: Duration) {
            assert!(self.jobs.recv_timeout(wait).is_err(), "a run was submitted");
        }

        fn run_submitted(&self) -> Job {
            self.jobs
                .recv_timeout(Duration::from_secs(5))
                .expect("a run was submitted")
        }
    }

    fn infos(events: &Receiver<Event>) -> Vec<String> {
        events
            .try_iter()
            .filter_map(|e| match e {
                Event::Info(text) => Some(text),
                _ => None,
            })
            .collect()
    }

    fn queue_edit(db: &Db, aid: i64) {
        db.queue_push(&Command::FetchAnime { aid }).unwrap();
    }

    #[test]
    fn with_the_option_off_nothing_is_ever_submitted() {
        let h = harness();
        h.control.sync(&settings(false), None);
        queue_edit(&h.db.open(), 1);
        h.stays_quiet(Duration::from_millis(500));
        h.finish();
    }

    #[test]
    fn new_commands_start_exactly_one_automatic_run_with_the_current_settings() {
        let h = harness();
        h.control.sync(&settings(true), None);
        std::thread::sleep(Duration::from_millis(60));
        let db = h.db.open();
        queue_edit(&db, 1);
        queue_edit(&db, 2);
        queue_edit(&db, 3);

        let job = h.run_submitted();
        let Job::AutoProcessQueue(sent) = job else {
            panic!("an automatic run is its own job: {job:?}");
        };
        assert_eq!(sent, settings(true));
        assert_eq!(
            h.pending.load(Ordering::SeqCst),
            1,
            "counted as a pending job like any other"
        );
        h.stays_quiet(Duration::from_millis(400));
        assert!(
            infos(&h.events).iter().any(|n| n.contains("started")),
            "the start is logged"
        );
        drop(db);
        h.finish();
    }

    #[test]
    fn a_run_waits_for_work_offline_a_cooldown_a_busy_worker_and_the_ui() {
        let h = harness();
        let mut offline = settings(true);
        offline.offline = true;
        h.control.sync(&offline, None);
        queue_edit(&h.db.open(), 1);
        h.stays_quiet(Duration::from_millis(400));

        // Online again, but an AniDB cooldown is active. Each new reason is put in place
        // before the old one is lifted, so the run is never free to start in between.
        h.db.open()
            .set_sync_cooldown(format::now() + 3600, "synthetic ban")
            .unwrap();
        h.control.sync(&settings(true), None);
        h.stays_quiet(Duration::from_millis(400));

        // The cooldown is over, but the worker is busy.
        h.pending.store(1, Ordering::SeqCst);
        h.db.open().clear_sync_cooldown().unwrap();
        h.stays_quiet(Duration::from_millis(400));

        // The worker is free, but the UI has an import under review.
        h.control
            .sync(&settings(true), Some("an import is in progress"));
        h.pending.store(0, Ordering::SeqCst);
        h.stays_quiet(Duration::from_millis(400));

        h.control.sync(&settings(true), None);
        assert!(matches!(h.run_submitted(), Job::AutoProcessQueue(_)));
        h.stays_quiet(Duration::from_millis(300));

        // One refusal lasts from the first postponement to the start, and is reported once,
        // with the reason it began with.
        let notes = infos(&h.events);
        let postponed: Vec<_> = notes.iter().filter(|n| n.contains("postponed")).collect();
        assert_eq!(postponed.len(), 1, "{notes:?}");
        assert!(postponed[0].contains("Work offline is on"), "{notes:?}");
        h.finish();
    }

    #[test]
    fn an_unset_account_holds_the_run_back_instead_of_blocking_the_queue() {
        let h = harness();
        h.control.sync(
            &AnidbSettings {
                auto_run_queue: true,
                ..AnidbSettings::default()
            },
            None,
        );
        queue_edit(&h.db.open(), 1);
        h.stays_quiet(Duration::from_millis(400));
        assert_eq!(h.db.open().queue_blocked_len().unwrap(), 0);
        h.control.sync(&settings(true), None);
        assert!(matches!(h.run_submitted(), Job::AutoProcessQueue(_)));
        h.finish();
    }

    #[test]
    fn a_queue_with_only_mylist_deletes_starts_no_run() {
        let h = harness();
        h.control.sync(&settings(true), None);
        let db = h.db.open();
        db.queue_push(&Command::MylistDelete { lid: 5 }).unwrap();
        h.stays_quiet(Duration::from_millis(500));
        assert_eq!(
            db.queue_len().unwrap(),
            1,
            "the removal waits for the user's run"
        );
        // Other work arrives: the run starts, and only that counts as work.
        queue_edit(&db, 1);
        assert!(matches!(h.run_submitted(), Job::AutoProcessQueue(_)));
        drop(db);
        h.finish();
    }

    #[test]
    fn stopping_ends_the_thread_without_starting_a_due_run() {
        let h = harness();
        h.control
            .sync(&settings(true), Some("a restore is in progress"));
        queue_edit(&h.db.open(), 1);
        std::thread::sleep(Duration::from_millis(200));

        let started = Instant::now();
        drop(h.stop);
        h.thread.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(h.jobs.try_recv().is_err());
    }
}
