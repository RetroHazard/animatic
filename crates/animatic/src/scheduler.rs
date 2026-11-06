//! Scheduled folder scans (roadmap M5.1).
//!
//! A dedicated worker-side thread decides when a scan is due and submits the ordinary
//! [`Job::Scan`]; the UI thread never waits for it. The thread polls instead of watching the
//! filesystem, which behaves the same on Windows and Linux and on network drives.
//!
//! The timing decision is the pure [`Schedule`], driven by explicit `Instant`s so tests control
//! the clock. [`ScanControl`] carries the settings and the UI's blocking state to the thread, and
//! [`spawn`] runs the loop. A due scan is only handed to the worker through
//! [`crate::worker::submit_if_idle`], so it never overlaps a running job or a manual scan, and it
//! is held back while the UI reports that an import, a settings review, a restore or unsaved
//! commands are in progress.

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use animatic_core::settings::FileManagerSettings;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::worker::{Event, Job, submit_if_idle};

/// How often the thread looks at the clock and at changed settings. A scan that is due but held
/// back is retried at this rhythm, so it starts shortly after the blocker is gone.
pub const POLL: Duration = Duration::from_secs(1);

/// The scheduling part of the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleConfig {
    pub enabled: bool,
    pub interval: Duration,
}

impl ScheduleConfig {
    pub fn of(settings: &FileManagerSettings) -> ScheduleConfig {
        ScheduleConfig {
            enabled: settings.scheduled_scan,
            interval: settings.scan_interval(),
        }
    }
}

/// What one [`Schedule::poll`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    /// Nothing is due.
    Idle,
    /// A scan was due and has been submitted.
    Submitted,
    /// A scan is due but could not start; it is retried at the next poll. `first` is true for
    /// the first poll of one postponement, which is the only one worth reporting.
    Postponed { reason: &'static str, first: bool },
}

/// When scans are due. Pure state: the caller supplies the time and whether a scan may start.
#[derive(Debug)]
pub struct Schedule {
    config: ScheduleConfig,
    next_due: Option<Instant>,
    start_due: bool,
    reported: bool,
}

impl Schedule {
    pub fn new(now: Instant, config: ScheduleConfig) -> Schedule {
        Schedule {
            config,
            next_due: config.enabled.then(|| now + config.interval),
            start_due: false,
            reported: false,
        }
    }

    /// A scan is due at the next poll, independent of the interval (scan at start).
    pub fn request_scan(&mut self) {
        self.start_due = true;
    }

    /// Adopt changed settings. Enabling, or changing the interval, counts the interval from
    /// `now`; disabling forgets a due interval scan, but not a requested start scan.
    pub fn configure(&mut self, now: Instant, config: ScheduleConfig) {
        if config == self.config {
            return;
        }
        let restart =
            config.enabled && (!self.config.enabled || config.interval != self.config.interval);
        self.config = config;
        if !config.enabled {
            self.next_due = None;
        } else if restart {
            self.next_due = Some(now + config.interval);
        }
    }

    /// How long until the next scan is due; `None` when nothing is scheduled.
    pub fn due_in(&self, now: Instant) -> Option<Duration> {
        if self.start_due {
            return Some(Duration::ZERO);
        }
        self.next_due.map(|due| due.saturating_duration_since(now))
    }

    /// If a scan is due, ask `start` to submit it. `start` returns why it cannot start now.
    /// A scan that cannot start stays due; the next interval is counted from the submission.
    pub fn poll(
        &mut self,
        now: Instant,
        start: &mut dyn FnMut() -> Result<(), &'static str>,
    ) -> Tick {
        if self.due_in(now) != Some(Duration::ZERO) {
            return Tick::Idle;
        }
        match start() {
            Ok(()) => {
                self.start_due = false;
                self.reported = false;
                self.next_due = self.config.enabled.then(|| now + self.config.interval);
                Tick::Submitted
            }
            Err(reason) => {
                let first = !self.reported;
                self.reported = true;
                Tick::Postponed { reason, first }
            }
        }
    }
}

#[derive(Debug)]
struct Inputs {
    managers: FileManagerSettings,
    extensions: Vec<String>,
    blocker: Option<&'static str>,
    start_requested: bool,
}

/// What the scheduler thread needs from the application: the current settings, whether the
/// application is in a state that forbids starting a scan, and the one-time start request.
#[derive(Debug)]
pub struct ScanControl {
    inputs: Mutex<Inputs>,
}

impl ScanControl {
    pub fn new() -> ScanControl {
        ScanControl {
            inputs: Mutex::new(Inputs {
                managers: FileManagerSettings::default(),
                extensions: Vec::new(),
                blocker: None,
                start_requested: false,
            }),
        }
    }

    /// Publish only the blocker, leaving the settings as last synced.
    pub fn set_blocker(&self, blocker: Option<&'static str>) {
        self.lock().blocker = blocker;
    }

    fn lock(&self) -> MutexGuard<'_, Inputs> {
        self.inputs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Publish the current settings and blocker. Cheap when nothing changed: the managers are
    /// only cloned when they differ.
    pub fn sync(
        &self,
        managers: &FileManagerSettings,
        extensions: &[String],
        blocker: Option<&'static str>,
    ) {
        let mut inputs = self.lock();
        if inputs.managers != *managers {
            inputs.managers = managers.clone();
        }
        if inputs.extensions != extensions {
            inputs.extensions = extensions.to_vec();
        }
        inputs.blocker = blocker;
    }

    /// Ask for one scan as soon as nothing blocks it (scan at start).
    pub fn request_scan(&self) {
        self.lock().start_requested = true;
    }

    fn take_request(&self) -> bool {
        std::mem::take(&mut self.lock().start_requested)
    }

    fn snapshot(&self) -> (ScheduleConfig, Option<&'static str>) {
        let inputs = self.lock();
        (ScheduleConfig::of(&inputs.managers), inputs.blocker)
    }

    fn scan_job(&self) -> Job {
        let inputs = self.lock();
        Job::Scan {
            settings: inputs.managers.clone(),
            extensions: inputs.extensions.join(","),
        }
    }
}

impl Default for ScanControl {
    fn default() -> Self {
        ScanControl::new()
    }
}

/// Start the scheduler thread. It ends when `stop` is signalled or its sender is dropped, within
/// one `poll`, and never starts anything after that.
pub fn spawn(
    control: Arc<ScanControl>,
    jobs: Sender<Job>,
    pending: Arc<AtomicUsize>,
    events: Sender<Event>,
    stop: Receiver<()>,
    poll: Duration,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("animatic-scheduler".into())
        .spawn(move || {
            let (config, _) = control.snapshot();
            let mut schedule = Schedule::new(Instant::now(), config);
            loop {
                let wait = schedule
                    .due_in(Instant::now())
                    .map_or(poll, |due| due.min(poll));
                match stop.recv_timeout(wait) {
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                }
                let now = Instant::now();
                let (config, blocker) = control.snapshot();
                schedule.configure(now, config);
                if control.take_request() {
                    schedule.request_scan();
                }
                let tick = schedule.poll(now, &mut || {
                    if let Some(reason) = blocker {
                        return Err(reason);
                    }
                    if submit_if_idle(&jobs, &pending, control.scan_job()) {
                        Ok(())
                    } else {
                        Err("another task is running")
                    }
                });
                let note = match tick {
                    Tick::Idle => None,
                    Tick::Submitted => Some("Scheduled folder scan started".to_string()),
                    Tick::Postponed {
                        reason,
                        first: true,
                    } => Some(format!(
                        "Scheduled folder scan postponed ({reason}); it starts when that is over"
                    )),
                    Tick::Postponed { first: false, .. } => None,
                };
                if let Some(note) = note {
                    let _ = events.send(Event::Info(note));
                }
            }
        })
        .expect("start scan scheduler")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;

    const INTERVAL: Duration = Duration::from_secs(600);

    fn config(enabled: bool) -> ScheduleConfig {
        ScheduleConfig {
            enabled,
            interval: INTERVAL,
        }
    }

    fn ok() -> Result<(), &'static str> {
        Ok(())
    }

    #[test]
    fn fires_at_the_interval_and_counts_from_the_submission() {
        let t0 = Instant::now();
        let mut schedule = Schedule::new(t0, config(true));

        let early = t0 + INTERVAL - Duration::from_secs(1);
        assert_eq!(schedule.poll(early, &mut ok), Tick::Idle);
        assert_eq!(schedule.poll(t0 + INTERVAL, &mut ok), Tick::Submitted);
        let again = t0 + INTERVAL + INTERVAL - Duration::from_secs(1);
        assert_eq!(schedule.poll(again, &mut ok), Tick::Idle);
        assert_eq!(schedule.poll(t0 + INTERVAL * 2, &mut ok), Tick::Submitted);
    }

    #[test]
    fn a_disabled_schedule_never_fires() {
        let t0 = Instant::now();
        let mut schedule = Schedule::new(t0, config(false));
        assert_eq!(schedule.due_in(t0), None);
        assert_eq!(schedule.poll(t0 + INTERVAL * 100, &mut ok), Tick::Idle);
    }

    #[test]
    fn a_blocked_scan_stays_due_is_reported_once_and_starts_when_free() {
        let t0 = Instant::now();
        let mut schedule = Schedule::new(t0, config(true));
        let mut blocked = || Err("a task is running");

        let due = t0 + INTERVAL;
        assert_eq!(
            schedule.poll(due, &mut blocked),
            Tick::Postponed {
                reason: "a task is running",
                first: true
            }
        );
        assert_eq!(
            schedule.poll(due + Duration::from_secs(1), &mut blocked),
            Tick::Postponed {
                reason: "a task is running",
                first: false
            }
        );
        let free = due + Duration::from_secs(30);
        assert_eq!(schedule.poll(free, &mut ok), Tick::Submitted);
        // The next interval is counted from when the scan really started.
        let before_next = free + INTERVAL - Duration::from_secs(1);
        assert_eq!(schedule.poll(before_next, &mut ok), Tick::Idle);
        assert_eq!(schedule.poll(free + INTERVAL, &mut ok), Tick::Submitted);
        // A later postponement is reported again.
        let later = free + INTERVAL * 2;
        assert!(matches!(
            schedule.poll(later, &mut blocked),
            Tick::Postponed { first: true, .. }
        ));
    }

    #[test]
    fn a_requested_scan_runs_once_even_when_the_schedule_is_off() {
        let t0 = Instant::now();
        let mut schedule = Schedule::new(t0, config(false));
        schedule.request_scan();

        assert_eq!(schedule.poll(t0, &mut ok), Tick::Submitted);
        assert_eq!(schedule.poll(t0 + INTERVAL * 3, &mut ok), Tick::Idle);
    }

    #[test]
    fn disabling_forgets_the_interval_and_changing_it_restarts_the_count() {
        let t0 = Instant::now();
        let mut schedule = Schedule::new(t0, config(true));
        let due = t0 + INTERVAL;
        assert!(matches!(
            schedule.poll(due, &mut || Err("busy")),
            Tick::Postponed { .. }
        ));
        schedule.configure(due, config(false));
        assert_eq!(schedule.poll(due, &mut ok), Tick::Idle);

        schedule.configure(due, config(true));
        assert_eq!(
            schedule.poll(due + INTERVAL - Duration::from_secs(1), &mut ok),
            Tick::Idle
        );
        schedule.configure(
            due + Duration::from_secs(10),
            ScheduleConfig {
                enabled: true,
                interval: INTERVAL * 2,
            },
        );
        assert_eq!(schedule.poll(due + INTERVAL * 2, &mut ok), Tick::Idle);
        assert_eq!(
            schedule.poll(due + Duration::from_secs(10) + INTERVAL * 2, &mut ok),
            Tick::Submitted
        );
    }

    struct Harness {
        control: Arc<ScanControl>,
        jobs: Receiver<Job>,
        pending: Arc<AtomicUsize>,
        events: Receiver<Event>,
        stop: Sender<()>,
        thread: JoinHandle<()>,
    }

    fn harness() -> Harness {
        let control = Arc::new(ScanControl::new());
        let (job_tx, jobs) = crossbeam_channel::unbounded();
        let (event_tx, events) = crossbeam_channel::unbounded();
        let (stop, stop_rx) = crossbeam_channel::bounded(1);
        let pending = Arc::new(AtomicUsize::new(0));
        let thread = spawn(
            control.clone(),
            job_tx,
            pending.clone(),
            event_tx,
            stop_rx,
            Duration::from_millis(20),
        );
        Harness {
            control,
            jobs,
            pending,
            events,
            stop,
            thread,
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

    #[test]
    fn the_thread_waits_for_a_running_job_and_for_the_ui_blocker_before_scanning() {
        let h = harness();
        let settings = FileManagerSettings::default();
        h.control.sync(
            &settings,
            &["mkv".to_string()],
            Some("an import is waiting"),
        );
        h.pending.store(1, Ordering::SeqCst);
        h.control.request_scan();

        assert!(h.jobs.recv_timeout(Duration::from_millis(300)).is_err());
        h.control.sync(&settings, &["mkv".to_string()], None);
        assert!(
            h.jobs.recv_timeout(Duration::from_millis(300)).is_err(),
            "a running job still blocks the scan"
        );
        h.pending.store(0, Ordering::SeqCst);

        let job = h
            .jobs
            .recv_timeout(Duration::from_secs(5))
            .expect("scan submitted");
        assert!(matches!(&job, Job::Scan { extensions, .. } if extensions == "mkv"));
        assert_eq!(
            h.pending.load(Ordering::SeqCst),
            1,
            "counted as a pending job"
        );
        assert!(
            h.jobs.recv_timeout(Duration::from_millis(200)).is_err(),
            "exactly one scan"
        );
        let notes = infos(&h.events);
        assert_eq!(
            notes.iter().filter(|n| n.contains("postponed")).count(),
            1,
            "{notes:?}"
        );
        assert!(notes.iter().any(|n| n.contains("started")), "{notes:?}");

        h.stop.send(()).unwrap();
        h.thread.join().unwrap();
    }

    #[test]
    fn stopping_ends_the_thread_without_starting_a_due_scan() {
        let h = harness();
        h.control
            .sync(&FileManagerSettings::default(), &[], Some("restoring"));
        h.control.request_scan();

        let started = Instant::now();
        drop(h.stop);
        h.thread.join().unwrap();

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(h.jobs.try_recv().is_err());
    }
}
