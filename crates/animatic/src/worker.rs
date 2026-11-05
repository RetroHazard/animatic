use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use animatic_anidb::error::TRANSIENT_COOLDOWN;
use animatic_anidb::http::HttpClient;
use animatic_anidb::udp::{Config as UdpConfig, UdpClient};
use animatic_core::db::Db;
use animatic_core::file_manager::hash_and_register;
use animatic_core::format;
use animatic_core::hash::Ed2kVariant;
use animatic_core::library::LibraryProjection;
use animatic_core::settings::{AnidbSettings, FileManagerSettings, Settings, StatFormat};
use animatic_core::sync::{Command, CommandError, MAX_QUEUE_TRIES};
use animatic_core::update::{
    InstallationTarget, Platform, ReleaseInfo, UpdateClient, VerifiedUpdate,
};
use crossbeam_channel::{Receiver, Sender};

#[derive(Debug)]
pub enum Job {
    Hash(Vec<PathBuf>),
    Scan {
        settings: FileManagerSettings,
        extensions: String,
    },
    ImportLegacy {
        source: PathBuf,
        temp: PathBuf,
    },
    ImportLegacySettings {
        source: PathBuf,
    },
    ProcessQueue(AnidbSettings),
    /// Queue the commands of a full update from the hashed local files, off the UI thread.
    QueueFullUpdate,
    /// Reload the library read model on the projection thread (see `Worker::submit`).
    LoadLibrary {
        generation: u64,
        export: Option<(PathBuf, String, StatFormat)>,
    },
    /// Write the statistics file on the projection thread (no started/finished events).
    ExportStatistics {
        path: PathBuf,
        username: String,
        format: StatFormat,
    },
    Stop,
}
#[derive(Debug, Clone)]
pub enum Event {
    Started {
        label: String,
        total: usize,
    },
    Progress {
        label: String,
        current: usize,
        total: usize,
        fraction: f32,
    },
    Hashed {
        path: PathBuf,
        local_id: i64,
        size: u64,
        ed2k: String,
    },
    Imported {
        summary: String,
    },
    LegacySettingsImported(Box<Settings>),
    LegacySettingsImportFailed(String),
    Synced {
        description: String,
        changed: bool,
    },
    Error(String),
    /// A queue entry was blocked (permanent failure, rejected credentials or too many attempts).
    Blocked {
        description: String,
    },
    /// AniDB sync is paused until `until` (epoch seconds); persisted in the database.
    Cooldown {
        until: i64,
        reason: String,
    },
    LibraryLoaded {
        generation: u64,
        projection: Box<LibraryProjection>,
    },
    LibraryLoadFailed {
        generation: u64,
        message: String,
    },
    Finished,
}
pub struct Worker {
    pub jobs: Sender<Job>,
    pub events: Receiver<Event>,
    /// Cancels the job running right now. The worker clears it when it starts the next job.
    pub cancel: Arc<AtomicBool>,
    pub pause: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    /// Jobs submitted to the main worker thread that have not finished yet (queued or running).
    pending: Arc<AtomicUsize>,
    loads: Sender<Background>,
    projection: Option<std::thread::JoinHandle<()>>,
    main: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    pub fn spawn(db_path: PathBuf) -> Worker {
        let (job_tx, job_rx) = crossbeam_channel::unbounded();
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let (load_tx, load_rx) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let pending = Arc::new(AtomicUsize::new(0));
        let flags = RunFlags {
            cancel: cancel.clone(),
            pause: pause.clone(),
            shutdown: shutdown.clone(),
            pending: pending.clone(),
        };
        let projection_db = db_path.clone();
        let projection_events = event_tx.clone();
        let projection_shutdown = shutdown.clone();
        let main_loads = load_tx.clone();
        let projection = std::thread::Builder::new()
            .name("animatic-projection".into())
            .spawn(move || {
                run_projections(
                    projection_db,
                    load_rx,
                    projection_events,
                    projection_shutdown,
                )
            })
            .expect("start projection worker");
        let main = std::thread::Builder::new()
            .name("animatic-worker".into())
            .spawn(move || run(db_path, job_rx, event_tx, main_loads, flags))
            .expect("start worker");
        Worker {
            jobs: job_tx,
            events: event_rx,
            cancel,
            pause,
            shutdown,
            pending,
            loads: load_tx,
            projection: Some(projection),
            main: Some(main),
        }
    }

    /// Queue a job. Projection loads are served immediately by their own thread and neither
    /// reset nor observe the cancel flag.
    pub fn submit(&self, job: Job) {
        match job {
            Job::LoadLibrary { generation, export } => {
                let _ = self
                    .loads
                    .send(Background::Load(LoadRequest { generation, export }));
            }
            Job::ExportStatistics {
                path,
                username,
                format,
            } => {
                let _ = self.loads.send(Background::Export {
                    path,
                    username,
                    format,
                });
            }
            job => {
                self.pending.fetch_add(1, Ordering::SeqCst);
                if self.jobs.send(job).is_err() {
                    self.pending.fetch_sub(1, Ordering::SeqCst);
                }
            }
        }
    }

    /// True when no job of the main worker thread is queued or running. Unlike the
    /// `Started`/`Finished` events, which the UI sees a frame later, this is the actual state.
    pub fn is_idle(&self) -> bool {
        self.pending.load(Ordering::SeqCst) == 0
    }

    /// Stops the worker and waits for the main worker thread too, so that its database
    /// connection is closed when this returns. Only meant for an idle worker (restart): a job in
    /// a network exchange would delay the return by up to its timeout.
    pub fn stop_and_join(&mut self) {
        self.close(true);
    }

    fn close(&mut self, join_main: bool) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.jobs.send(Job::Stop);
        let _ = self.loads.send(Background::Stop);
        if let Some(projection) = self.projection.take() {
            let _ = projection.join();
        }
        if join_main && let Some(main) = self.main.take() {
            let _ = main.join();
        }
    }
}

/// One counted job of the main worker thread; the count drops when the job is finished.
struct PendingJob(Option<Arc<AtomicUsize>>);

impl PendingJob {
    /// Only jobs that `Worker::submit` counted are uncounted again here: `Stop` and projection
    /// requests are never counted.
    fn of(job: &Job, pending: &Arc<AtomicUsize>) -> PendingJob {
        match job {
            Job::Stop | Job::LoadLibrary { .. } | Job::ExportStatistics { .. } => PendingJob(None),
            _ => PendingJob(Some(pending.clone())),
        }
    }
}

impl Drop for PendingJob {
    fn drop(&mut self) {
        if let Some(pending) = &self.0 {
            pending.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl Drop for Worker {
    /// Cancels the running job and waits only for the projection thread: it finishes accepted
    /// statistics exports and a load in progress (local database reads/writes, no network). The
    /// main worker thread is not joined: it may be inside a network exchange of up to 20 s.
    fn drop(&mut self) {
        self.close(false);
    }
}

/// The shared control flags of the main worker thread.
struct RunFlags {
    cancel: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    pending: Arc<AtomicUsize>,
}

fn run(
    db_path: PathBuf,
    jobs: Receiver<Job>,
    events: Sender<Event>,
    loads: Sender<Background>,
    flags: RunFlags,
) {
    let RunFlags {
        cancel,
        pause,
        shutdown,
        pending,
    } = flags;
    while let Ok(job) = jobs.recv() {
        let _counted = PendingJob::of(&job, &pending);
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        let job = match job {
            Job::Stop => break,
            // Normally routed by `Worker::submit`; never touches the cancel flag.
            Job::LoadLibrary { generation, export } => {
                let _ = loads.send(Background::Load(LoadRequest { generation, export }));
                continue;
            }
            Job::ExportStatistics {
                path,
                username,
                format,
            } => {
                let _ = loads.send(Background::Export {
                    path,
                    username,
                    format,
                });
                continue;
            }
            other => other,
        };
        // A cancel request applies to the job that is running when it is made. It is only
        // cleared here, when the next job starts, so a following `submit` cannot swallow it.
        cancel.store(false, Ordering::Relaxed);
        match job {
            Job::Stop | Job::LoadLibrary { .. } | Job::ExportStatistics { .. } => {
                unreachable!("handled before dispatch")
            }
            Job::Hash(paths) => {
                let total = paths.len();
                let _ = events.send(Event::Started {
                    label: "Hashing files".into(),
                    total,
                });
                for (index, path) in paths.into_iter().enumerate() {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let label = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let result = Db::open(&db_path).and_then(|db| {
                        hash_and_register(
                            &db,
                            &path,
                            Ed2kVariant::Red,
                            &cancel,
                            &pause,
                            &mut |done, size| {
                                let _ = events.send(Event::Progress {
                                    label: label.clone(),
                                    current: index + 1,
                                    total,
                                    fraction: done as f32 / size.max(1) as f32,
                                });
                            },
                        )
                    });
                    match result {
                        Ok((local_id, hashes)) => {
                            let _ = events.send(Event::Hashed {
                                path,
                                local_id,
                                size: hashes.size,
                                ed2k: hashes.ed2k,
                            });
                        }
                        Err(e) => {
                            let _ = events.send(Event::Error(format!("{}: {e}", path.display())));
                        }
                    }
                }
                let _ = events.send(Event::Finished);
            }
            Job::ProcessQueue(settings) => process_queue(&db_path, settings, &events, &cancel),
            Job::QueueFullUpdate => {
                let _ = events.send(Event::Started {
                    label: "Queueing full update".into(),
                    total: 0,
                });
                match queue_full_update(&db_path) {
                    Ok(n) => {
                        let _ = events.send(Event::Synced {
                            description: format!("Queued {n} commands for the full update"),
                            changed: true,
                        });
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(format!("Full update failed: {e}")));
                    }
                }
                let _ = events.send(Event::Finished);
            }
            Job::Scan {
                settings,
                extensions,
            } => {
                let db = match Db::open(&db_path) {
                    Ok(db) => db,
                    Err(e) => {
                        let _ = events.send(Event::Error(e.to_string()));
                        continue;
                    }
                };
                let actions =
                    match animatic_core::file_manager::plan_scan(&db, &settings, &extensions) {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = events.send(Event::Error(e.to_string()));
                            continue;
                        }
                    };
                let total = actions.len();
                let _ = events.send(Event::Started {
                    label: "Scanning managed folders".into(),
                    total,
                });
                for (index, action) in actions.into_iter().enumerate() {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    match action {
                        animatic_core::file_manager::ScanAction::Hash { path, .. }
                        | animatic_core::file_manager::ScanAction::Rehash { path, .. } => {
                            let label = path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned();
                            match hash_and_register(
                                &db,
                                &path,
                                Ed2kVariant::Red,
                                &cancel,
                                &pause,
                                &mut |done, size| {
                                    let _ = events.send(Event::Progress {
                                        label: label.clone(),
                                        current: index + 1,
                                        total,
                                        fraction: done as f32 / size.max(1) as f32,
                                    });
                                },
                            ) {
                                Ok((local_id, hashes)) => {
                                    let _ = events.send(Event::Hashed {
                                        path,
                                        local_id,
                                        size: hashes.size,
                                        ed2k: hashes.ed2k,
                                    });
                                }
                                Err(e) => {
                                    let _ = events
                                        .send(Event::Error(format!("{}: {e}", path.display())));
                                }
                            }
                        }
                        animatic_core::file_manager::ScanAction::RemoveMissing {
                            local_id,
                            path,
                        } => match animatic_core::file_manager::remove_missing(&db, local_id) {
                            Ok(()) => {
                                let _ = events.send(Event::Synced {
                                    description: format!("Removed missing path {}", path.display()),
                                    changed: true,
                                });
                            }
                            Err(e) => {
                                let _ = events.send(Event::Error(e.to_string()));
                            }
                        },
                    }
                }
                let _ = events.send(Event::Finished);
            }
            Job::ImportLegacySettings { source } => {
                let _ = events.send(Event::Started {
                    label: "Reading AOM settings".into(),
                    total: 1,
                });
                match animatic_core::settings::import_legacy_settings(&source) {
                    Ok(settings) => {
                        let _ = events.send(Event::LegacySettingsImported(Box::new(settings)));
                    }
                    Err(e) => {
                        let _ = events.send(Event::LegacySettingsImportFailed(format!(
                            "Could not import AOM settings: {e}"
                        )));
                    }
                }
                let _ = events.send(Event::Finished);
            }
            Job::ImportLegacy { source, temp } => {
                let _ = events.send(Event::Started {
                    label: "Importing legacy databank".into(),
                    total: 1,
                });
                let result = Db::open(&db_path).and_then(|mut db| {
                    animatic_core::legacy::import::import_databank(
                        &mut db,
                        &source,
                        &temp,
                        &mut |label, fraction| {
                            let _ = events.send(Event::Progress {
                                label: label.into(),
                                current: 1,
                                total: 1,
                                fraction,
                            });
                            !cancel.load(Ordering::Relaxed)
                        },
                    )
                });
                match result {
                    Ok(report) => {
                        let records: usize = report.imported.iter().map(|(_, n)| *n).sum();
                        let _ = events.send(Event::Imported {
                            summary: format!(
                                "Imported {records} records from {} collections",
                                report.imported.len()
                            ),
                        });
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(format!("Legacy import failed: {e}")));
                    }
                }
                let _ = events.send(Event::Finished);
            }
        }
    }
}

/// A request for the updater thread (see [`Updater`]).
#[derive(Debug)]
pub enum UpdateJob {
    /// Look for a newer stable release. Needs only the public release list.
    Check,
    /// Download and verify the archive of a release found by a check.
    Download(Box<ReleaseInfo>),
    /// Replace the executable with the verified update. Runs to completion once started.
    Install(Box<VerifiedUpdate>),
}

/// What the updater thread reports. Kept apart from [`Event`]: these never drive the activity
/// bar or a library refresh.
#[derive(Debug)]
pub enum UpdateEvent {
    Checked(Option<Box<ReleaseInfo>>),
    CheckFailed(String),
    Progress {
        done: u64,
        total: u64,
    },
    /// The archive matched its published checksum and the files are staged for installation.
    Ready(Box<VerifiedUpdate>),
    DownloadFailed(String),
    /// A check or download stopped because the user cancelled it.
    Cancelled,
    /// The executable was replaced; the value is the installed version.
    Installed(String),
    /// `attempted`: the replacement of the executable was entered (or the thread panicked
    /// inside it), so the installed files may be in an unknown state. Otherwise nothing was
    /// replaced, though the staged update is gone.
    InstallFailed {
        message: String,
        attempted: bool,
    },
}

/// What an installation did in this process. Once the replacement of the executable was entered,
/// whatever its outcome, the file on disk may differ from the running build (a failed
/// replacement can leave it changed), so the process neither checks, downloads nor installs
/// again. The lock also serializes install requests of every updater in the process.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InstallAttempt {
    None,
    /// The executable was replaced; restarting runs the new version.
    Succeeded,
    /// The attempt ended in an error or a panic after the replacement was entered; the
    /// installed files are in an unknown state.
    Failed,
}

impl InstallAttempt {
    fn refusal(self) -> Option<&'static str> {
        match self {
            InstallAttempt::None => None,
            InstallAttempt::Succeeded => Some(
                "Animatic has already been updated; restart it before checking for further updates",
            ),
            InstallAttempt::Failed => Some(
                "An earlier installation attempt in this session did not finish cleanly; no \
                 further updates are possible until Animatic has been restarted or reinstalled \
                 manually",
            ),
        }
    }
}

static INSTALL_ATTEMPT: parking_lot::Mutex<InstallAttempt> =
    parking_lot::Mutex::new(InstallAttempt::None);
/// Progress events are at most this frequent; the final one is always sent.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Update checks, downloads and installations on a thread of their own, with a cancel flag of
/// their own: the main worker clears its flag whenever a job starts and may be busy for hours.
///
/// Dropping the updater cancels a check or download and **waits** for the thread. A running
/// installation is never interrupted, so the executable cannot be replaced behind the back of an
/// exiting process; queued jobs are discarded and an installation that has not started does not
/// start. Network waits are bounded by the core client's timeouts.
pub struct Updater {
    jobs: Option<Sender<UpdateJob>>,
    pub events: Receiver<UpdateEvent>,
    cancel: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Updater {
    /// `installation` identifies the executable the process was started from, captured before
    /// any replacement; without it installation is refused.
    pub fn spawn(installation: Option<Arc<InstallationTarget>>) -> Updater {
        let (job_tx, job_rx) = crossbeam_channel::unbounded();
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let (c, s) = (cancel.clone(), shutdown.clone());
        let thread = std::thread::Builder::new()
            .name("animatic-updater".into())
            .spawn(move || run_updates(job_rx, event_tx, c, s, installation))
            .expect("start updater");
        Updater {
            jobs: Some(job_tx),
            events: event_rx,
            cancel,
            shutdown,
            thread: Some(thread),
        }
    }

    /// The UI submits only while no operation is in flight (it waits for the typed reply or
    /// cancel acknowledgement), so the cancel flag is cleared here, before the job is queued:
    /// a cancel request made while the job is still queued then reaches the job.
    pub fn submit(&self, job: UpdateJob) {
        if let Some(jobs) = &self.jobs {
            self.cancel.store(false, Ordering::SeqCst);
            let _ = jobs.send(job);
        }
    }

    /// Cancels the check or download that is running now (never an installation).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

impl Drop for Updater {
    fn drop(&mut self) {
        // `submit` cannot run concurrently with this, and the thread never clears `cancel`, so
        // the request below always stands for whatever job is running or queued.
        self.shutdown.store(true, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_updates(
    jobs: Receiver<UpdateJob>,
    events: Sender<UpdateEvent>,
    cancel: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    installation: Option<Arc<InstallationTarget>>,
) {
    let mut client: Option<UpdateClient> = None;
    while let Ok(job) = jobs.recv() {
        // `cancel` is only raised by the user or by `Updater::drop`; `submit` clears it.
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        let event = match job {
            UpdateJob::Check => guarded(
                || {
                    if let Some(refusal) = INSTALL_ATTEMPT.lock().refusal() {
                        return UpdateEvent::CheckFailed(refusal.into());
                    }
                    let Some(platform) = Platform::current() else {
                        return UpdateEvent::CheckFailed(
                            "Updates are only available for 64-bit Windows and Linux".into(),
                        );
                    };
                    match update_client(&mut client)
                        .and_then(|c| c.check(env!("CARGO_PKG_VERSION"), platform, &cancel))
                    {
                        Ok(release) => UpdateEvent::Checked(release.map(Box::new)),
                        Err(_) if cancel.load(Ordering::SeqCst) => UpdateEvent::Cancelled,
                        Err(e) => UpdateEvent::CheckFailed(e.to_string()),
                    }
                },
                UpdateEvent::CheckFailed("The update check failed unexpectedly".into()),
            ),
            UpdateJob::Download(release) => guarded(
                || {
                    if let Some(refusal) = INSTALL_ATTEMPT.lock().refusal() {
                        return UpdateEvent::DownloadFailed(refusal.into());
                    }
                    let mut last = None::<Instant>;
                    let mut progress = |done: u64, total: u64| {
                        let due = last.is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL);
                        if due || done >= total {
                            last = Some(Instant::now());
                            let _ = events.send(UpdateEvent::Progress { done, total });
                        }
                    };
                    match update_client(&mut client)
                        .and_then(|c| c.download(&release, &cancel, &mut progress))
                    {
                        Ok(update) => UpdateEvent::Ready(Box::new(update)),
                        Err(_) if cancel.load(Ordering::SeqCst) => UpdateEvent::Cancelled,
                        Err(e) => UpdateEvent::DownloadFailed(e.to_string()),
                    }
                },
                UpdateEvent::DownloadFailed("The download failed unexpectedly".into()),
            ),
            UpdateJob::Install(update) => {
                let Some(target) = installation.as_deref() else {
                    let _ = events.send(UpdateEvent::InstallFailed {
                        message: "Animatic could not identify the program it was started from"
                            .into(),
                        attempted: false,
                    });
                    continue;
                };
                let mut attempt = INSTALL_ATTEMPT.lock();
                if let Some(refusal) = attempt.refusal() {
                    UpdateEvent::InstallFailed {
                        message: refusal.into(),
                        attempted: true,
                    }
                } else if shutdown.load(Ordering::SeqCst) {
                    break;
                } else {
                    // Latched before the call: a panic inside must count as an attempt too.
                    *attempt = InstallAttempt::Failed;
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        (*update).install(target)
                    }));
                    match outcome {
                        Ok(Ok(version)) => {
                            *attempt = InstallAttempt::Succeeded;
                            UpdateEvent::Installed(version)
                        }
                        Ok(Err(e)) => {
                            let attempted = e.install_attempted();
                            if !attempted {
                                // Refused before anything was replaced (another installer, a
                                // different file on disk): the process is unchanged.
                                *attempt = InstallAttempt::None;
                            }
                            UpdateEvent::InstallFailed {
                                message: e.to_string(),
                                attempted,
                            }
                        }
                        Err(_) => UpdateEvent::InstallFailed {
                            message: "The installation failed unexpectedly".into(),
                            attempted: true,
                        },
                    }
                }
            }
        };
        let _ = events.send(event);
    }
}

/// The client is built on first use so that a failure to build it is an ordinary check error.
fn update_client(slot: &mut Option<UpdateClient>) -> animatic_core::update::Result<&UpdateClient> {
    if slot.is_none() {
        *slot = Some(UpdateClient::new()?);
    }
    Ok(slot.as_ref().expect("client was just stored"))
}

/// Runs `job`; a panic inside it becomes `on_panic` instead of silently ending the thread and
/// leaving the UI waiting for a reply.
fn guarded(job: impl FnOnce() -> UpdateEvent, on_panic: UpdateEvent) -> UpdateEvent {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).unwrap_or(on_panic)
}

/// A request for the projection thread.
struct LoadRequest {
    generation: u64,
    export: Option<(PathBuf, String, StatFormat)>,
}

enum Background {
    Load(LoadRequest),
    /// Finish the accepted requests before it, then end the projection thread.
    Stop,
    Export {
        path: PathBuf,
        username: String,
        format: StatFormat,
    },
}

/// Serve library projections and statistics exports on a thread of their own so that an
/// hours-long AniDB queue or a big hashing job never delays the UI. SQLite's WAL mode lets this
/// connection read while the main worker writes. Of several pending loads only the newest runs;
/// every export that was accepted runs, even during shutdown (`Background::Stop` is queued
/// behind them), but loads are pointless then and are skipped.
fn run_projections(
    db_path: PathBuf,
    requests: Receiver<Background>,
    events: Sender<Event>,
    shutdown: Arc<AtomicBool>,
) {
    let mut slot: Option<Db> = None;
    while let Ok(first) = requests.recv() {
        let mut batch = vec![first];
        batch.extend(requests.try_iter());
        let stopping = batch.iter().any(|r| matches!(r, Background::Stop));
        let newest_load = batch.iter().rposition(|r| matches!(r, Background::Load(_)));
        for (index, request) in batch.into_iter().enumerate() {
            let event = match request {
                Background::Stop => continue,
                Background::Load(request) => {
                    if stopping || shutdown.load(Ordering::Relaxed) || Some(index) != newest_load {
                        continue;
                    }
                    load_projection(&db_path, &mut slot, request, &events)
                }
                Background::Export {
                    path,
                    username,
                    format,
                } => export_statistics(&db_path, &mut slot, &path, &username, format),
            };
            let _ = events.send(event);
        }
        if stopping {
            return;
        }
    }
}

fn export_statistics(
    db_path: &Path,
    slot: &mut Option<Db>,
    path: &Path,
    username: &str,
    format: StatFormat,
) -> Event {
    let db = match slot.take() {
        Some(db) => db,
        None => match Db::open(db_path) {
            Ok(db) => db,
            Err(e) => return Event::Error(format!("Statistics export failed: {e}")),
        },
    };
    let result = db
        .library_stats()
        .and_then(|stats| stats.write(path, username, format));
    *slot = Some(db);
    match result {
        Ok(()) => Event::Synced {
            description: format!("Exported statistics to {}", path.display()),
            changed: false,
        },
        Err(e) => Event::Error(format!("Statistics export failed: {e}")),
    }
}

fn load_projection(
    db_path: &Path,
    slot: &mut Option<Db>,
    request: LoadRequest,
    events: &Sender<Event>,
) -> Event {
    let generation = request.generation;
    let failed = |message: String| Event::LibraryLoadFailed {
        generation,
        message,
    };
    let db = match slot.take() {
        Some(db) => db,
        None => match Db::open(db_path) {
            Ok(db) => db,
            Err(e) => return failed(format!("Unable to open the database: {e}")),
        },
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        LibraryProjection::load(&db).map(|projection| {
            let export_error = request
                .export
                .as_ref()
                .and_then(|(path, username, format)| {
                    db.library_stats()
                        .and_then(|stats| stats.write(path, username, *format))
                        .err()
                });
            (projection, export_error)
        })
    }));
    match outcome {
        Ok(Ok((projection, export_error))) => {
            *slot = Some(db);
            if let Some(e) = export_error {
                let _ = events.send(Event::Error(format!(
                    "Automatic statistics export failed: {e}"
                )));
            }
            Event::LibraryLoaded {
                generation,
                projection: Box::new(projection),
            }
        }
        Ok(Err(e)) => {
            *slot = Some(db);
            failed(format!("Unable to refresh library: {e}"))
        }
        // The connection may be mid-transaction after a panic: reopen it next time.
        Err(_) => failed("Unable to refresh library: the loader failed unexpectedly".into()),
    }
}

fn describe_wait(seconds: i64) -> String {
    if seconds < 120 {
        format!("{}s", seconds.max(1))
    } else {
        format!("{} min", (seconds + 59) / 60)
    }
}

/// Persist a cooldown so it also holds after a restart, and tell the UI.
fn start_cooldown(
    db: &Db,
    wait: Duration,
    reason: &str,
    events: &Sender<Event>,
) -> animatic_core::Result<()> {
    let until = format::now() + wait.as_secs() as i64 + i64::from(wait.subsec_nanos() > 0);
    db.set_sync_cooldown(until, reason)?;
    let _ = events.send(Event::Cooldown {
        until,
        reason: reason.to_string(),
    });
    Ok(())
}

/// Record the failure of one queued command. Returns whether the queue may continue with the
/// next entry. Every outcome either stops the queue or removes the entry from the eligible set
/// (blocked), so no command can be attempted in a tight loop.
fn fail_command(
    db: &Db,
    id: i64,
    tries: i64,
    command: &Command,
    error: &CommandError,
    events: &Sender<Event>,
) -> bool {
    let description = command.describe();
    let _ = events.send(Event::Error(format!("{description}: {}", error.message)));
    let recorded = (|| -> animatic_core::Result<bool> {
        if error.fatal {
            if let Some(wait) = error.retry_after {
                // Ban: keep the entry, stop everything for the cooldown.
                db.queue_deferred(id, &error.message)?;
                start_cooldown(db, wait, &error.message, events)?;
            } else {
                // Rejected credentials / outdated client: nothing can succeed until corrected.
                let n = db.queue_block_all(&error.message)?;
                let _ = events.send(Event::Blocked {
                    description: format!("{n} queued commands blocked: {}", error.message),
                });
            }
            Ok(false)
        } else if error.transient {
            let wait = error.retry_after.unwrap_or(TRANSIENT_COOLDOWN);
            if error.counts_attempt {
                db.queue_failed(id, &error.message)?;
                if tries + 1 >= MAX_QUEUE_TRIES {
                    let _ = events.send(Event::Blocked {
                        description: format!(
                            "{description} blocked after {MAX_QUEUE_TRIES} failed attempts"
                        ),
                    });
                }
            } else {
                db.queue_deferred(id, &error.message)?;
            }
            start_cooldown(db, wait, &error.message, events)?;
            Ok(false)
        } else {
            db.queue_block(id, &error.message)?;
            let _ = events.send(Event::Blocked {
                description: format!("{description} blocked: {}", error.message),
            });
            Ok(true)
        }
    })();
    match recorded {
        Ok(proceed) => proceed,
        Err(e) => {
            let _ = events.send(Event::Error(format!(
                "Could not record the failure of {description}: {e}"
            )));
            false
        }
    }
}

/// Run the AniDB command queue until it is empty, cancelled, blocked or put on cooldown.
fn process_queue(
    db_path: &Path,
    settings: AnidbSettings,
    events: &Sender<Event>,
    cancel: &AtomicBool,
) {
    if settings.offline {
        let _ = events.send(Event::Error(
            "Offline mode is enabled; commands remain queued".into(),
        ));
        return;
    }
    let db = match Db::open(db_path) {
        Ok(db) => db,
        Err(e) => {
            let _ = events.send(Event::Error(e.to_string()));
            return;
        }
    };
    // The cooldown is checked before any transport exists, and fails closed.
    match db.sync_cooldown() {
        Ok(None) => {}
        Ok(Some((until, reason))) => {
            let _ = events.send(Event::Error(format!(
                "AniDB sync is paused for another {} ({reason})",
                describe_wait(until - format::now())
            )));
            let _ = events.send(Event::Cooldown { until, reason });
            return;
        }
        Err(e) => {
            let _ = events.send(Event::Error(format!("Cannot read the sync cooldown: {e}")));
            return;
        }
    }
    let _ = events.send(Event::Started {
        label: "Synchronizing with AniDB".into(),
        total: 0,
    });
    let mut udp = UdpClient::new(UdpConfig {
        server: settings.udp_server.clone(),
        local_port: settings.local_port,
        client: settings.client_name.clone(),
        client_version: settings.client_version,
        username: settings.username.clone(),
        password: settings.password.clone(),
        api_key: (!settings.api_key.is_empty()).then(|| settings.api_key.clone()),
        ..Default::default()
    });
    let mut http = HttpClient::new(&settings.client_name, settings.client_version);
    while !cancel.load(Ordering::Relaxed) {
        let next = match db.queue_peek() {
            Ok(v) => v,
            Err(e) => {
                let _ = events.send(Event::Error(e.to_string()));
                break;
            }
        };
        let Some((id, command, tries)) = next else {
            break;
        };
        let _ = events.send(Event::Progress {
            label: command.describe(),
            current: 0,
            total: 0,
            fraction: 0.0,
        });
        match animatic_core::sync::execute(id, &command, &db, &mut udp, &mut http) {
            Ok(outcome) => {
                let changed = outcome.changes_library();
                let description = outcome.describe();
                if let Err(e) = db.queue_remove(id) {
                    let _ = events.send(Event::Error(e.to_string()));
                    break;
                }
                let _ = events.send(Event::Synced {
                    description,
                    changed,
                });
            }
            Err(e) => {
                if !fail_command(&db, id, tries, &command, &e, events) {
                    break;
                }
            }
        }
    }
    let _ = udp.logout();
    let _ = events.send(Event::Finished);
}

/// Queue the commands of a full update (`--full-update`): the titles dump plus every anime and
/// file known from hashed local files.
fn queue_full_update(db_path: &Path) -> animatic_core::Result<usize> {
    let db = Db::open(db_path)?;
    let mut fids = BTreeSet::new();
    let mut aids = BTreeSet::new();
    for file in db.local_files()? {
        fids.extend(file.fid);
        aids.extend(file.aid);
    }
    let mut commands = Vec::with_capacity(1 + fids.len() + aids.len());
    commands.push(Command::TitlesDump);
    commands.extend(fids.into_iter().map(|fid| Command::FetchFile { fid }));
    commands.extend(aids.into_iter().map(|aid| Command::FetchAnime { aid }));
    db.queue_push_many(&commands)
}

#[cfg(test)]
mod tests {
    use std::net::{SocketAddr, UdpSocket};
    use std::sync::atomic::AtomicUsize;
    use std::thread::JoinHandle;
    use std::time::Instant;

    use super::*;

    /// Local stand-in for the AniDB UDP server. `reply(n, request)` is the reply (without tag)
    /// to the n-th packet; the fixture echoes the request tag like the real server.
    struct Fixture {
        addr: SocketAddr,
        packets: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    impl Fixture {
        fn start(reply: impl Fn(usize, &str) -> String + Send + 'static) -> Fixture {
            let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
            socket
                .set_read_timeout(Some(Duration::from_millis(25)))
                .unwrap();
            let addr = socket.local_addr().unwrap();
            let packets = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let (p, s) = (packets.clone(), stop.clone());
            let handle = std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while !s.load(Ordering::Relaxed) {
                    let Ok((n, from)) = socket.recv_from(&mut buf) else {
                        continue;
                    };
                    let index = p.fetch_add(1, Ordering::SeqCst);
                    let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let tag = request
                        .rsplit_once("&tag=")
                        .map(|(_, t)| t.trim().to_string())
                        .unwrap_or_default();
                    let _ = socket
                        .send_to(format!("{tag} {}", reply(index, &request)).as_bytes(), from);
                }
            });
            Fixture {
                addr,
                packets,
                stop,
                handle: Some(handle),
            }
        }

        fn packets(&self) -> usize {
            self.packets.load(Ordering::SeqCst)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    struct TempDb {
        dir: PathBuf,
        path: PathBuf,
    }

    impl TempDb {
        fn new() -> TempDb {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "animatic-worker-test-{}-{}",
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

    fn settings(server: SocketAddr) -> AnidbSettings {
        AnidbSettings {
            username: "synthetic-user".into(),
            password: "synthetic-password".into(),
            client_name: "syntheticclient".into(),
            udp_server: server.to_string(),
            local_port: 0,
            ..Default::default()
        }
    }

    /// Run the queue once as a fresh job (new clients every time) and return its events.
    fn run_queue(db: &TempDb, settings: &AnidbSettings) -> Vec<Event> {
        let (tx, rx) = crossbeam_channel::unbounded();
        process_queue(&db.path, settings.clone(), &tx, &AtomicBool::new(false));
        rx.try_iter().collect()
    }

    fn queue_two(db: &Db) {
        db.queue_push(&Command::MylistStats).unwrap();
        db.queue_push(&Command::MylistRefresh { lid: 1 }).unwrap();
    }

    fn has_error(events: &[Event], needle: &str) -> bool {
        events
            .iter()
            .any(|e| matches!(e, Event::Error(m) if m.contains(needle)))
    }

    #[test]
    fn ban_sets_a_persistent_cooldown_that_new_clients_obey() {
        let fixture = Fixture::start(|_, _| "555 BANNED".into());
        let db = TempDb::new();
        queue_two(&db.open());
        let s = settings(fixture.addr);

        let started = format::now();
        let events = run_queue(&db, &s);
        assert!(events.iter().any(|e| matches!(e, Event::Cooldown { .. })));
        assert_eq!(fixture.packets(), 1, "only the login was sent");
        let (until, _) = db.open().sync_cooldown().unwrap().expect("cooldown stored");
        assert!(until >= started + 29 * 60, "conservative ban cooldown");
        let d = db.open();
        assert_eq!(d.queue_len().unwrap(), 2, "nothing is dropped");
        assert_eq!(
            d.queue_blocked_len().unwrap(),
            0,
            "a ban never blocks entries"
        );

        // A brand new job (new UDP/HTTP clients) must not touch the network.
        let events = run_queue(&db, &s);
        assert_eq!(fixture.packets(), 1, "cooldown holds for new clients");
        assert!(has_error(&events, "paused"));
        assert!(!events.iter().any(|e| matches!(e, Event::Started { .. })));
    }

    #[test]
    fn code_510_is_treated_as_a_ban_and_never_retried() {
        let fixture = Fixture::start(|_, _| "510 BANNED".into());
        let db = TempDb::new();
        queue_two(&db.open());
        let started = format::now();
        run_queue(&db, &settings(fixture.addr));
        let (until, _) = db.open().sync_cooldown().unwrap().expect("cooldown stored");
        assert!(until >= started + 29 * 60);
        assert_eq!(fixture.packets(), 1);
        assert_eq!(db.open().queue_blocked_len().unwrap(), 0);
    }

    #[test]
    fn server_refusal_pauses_new_jobs_without_blocking_pending_work() {
        let fixture = Fixture::start(|_, _| "604 TIMEOUT - DELAY AND RESUBMIT".into());
        let db = TempDb::new();
        queue_two(&db.open());
        let started = format::now();
        run_queue(&db, &settings(fixture.addr));
        let d = db.open();
        let (until, _) = d.sync_cooldown().unwrap().expect("cooldown stored");
        assert!(
            (started + 55..=started + 70).contains(&until),
            "{until}-{started}"
        );
        let entries = d.queue_list().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| !e.blocked));
        assert_eq!(fixture.packets(), 1);
        run_queue(&db, &settings(fixture.addr));
        assert_eq!(
            fixture.packets(),
            1,
            "a new job must obey the stored cooldown"
        );
    }

    #[test]
    fn rejected_credentials_block_the_queue_until_corrected() {
        let fixture = Fixture::start(|_, _| "500 LOGIN FAILED".into());
        let db = TempDb::new();
        queue_two(&db.open());
        let s = settings(fixture.addr);
        let events = run_queue(&db, &s);
        assert!(events.iter().any(|e| matches!(e, Event::Blocked { .. })));
        let d = db.open();
        assert_eq!(d.queue_blocked_len().unwrap(), 2);
        assert!(d.sync_cooldown().unwrap().is_none());
        assert_eq!(fixture.packets(), 1);

        // Nothing is eligible any more: a retry of the job does not even try to log in.
        run_queue(&db, &s);
        assert_eq!(fixture.packets(), 1);
    }

    #[test]
    fn unexpected_replies_block_each_entry_once_and_continue() {
        let fixture = Fixture::start(|n, _| {
            if n == 0 {
                "200 session 127.0.0.1:1 LOGIN ACCEPTED".into()
            } else {
                "999 NOT A REAL REPLY".into()
            }
        });
        let db = TempDb::new();
        queue_two(&db.open());
        let events = run_queue(&db, &settings(fixture.addr));
        let blocked = events
            .iter()
            .filter(|e| matches!(e, Event::Blocked { .. }))
            .count();
        assert_eq!(blocked, 2, "both entries were attempted exactly once");
        assert_eq!(fixture.packets(), 4, "login, one packet per entry, logout");
        let d = db.open();
        assert_eq!(d.queue_blocked_len().unwrap(), 2);
        assert!(d.queue_peek().unwrap().is_none());
        assert!(d.sync_cooldown().unwrap().is_none());
        assert!(events.iter().any(|e| matches!(e, Event::Finished)));
    }

    #[test]
    fn offline_mode_leaves_the_queue_alone() {
        let fixture = Fixture::start(|_, _| "555 BANNED".into());
        let db = TempDb::new();
        queue_two(&db.open());
        let mut s = settings(fixture.addr);
        s.offline = true;
        let events = run_queue(&db, &s);
        assert!(has_error(&events, "Offline"));
        assert_eq!(fixture.packets(), 0);
    }

    fn wait_for(worker: &Worker, mut accept: impl FnMut(&Event) -> bool) -> Event {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let event = worker
                .events
                .recv_timeout(left)
                .expect("worker event within 20 s");
            if accept(&event) {
                return event;
            }
        }
    }

    #[test]
    fn projection_loads_leave_progress_and_cancellation_alone() {
        let db = TempDb::new();
        drop(db.open());
        let export = db.dir.join("stats.xml");
        let worker = Worker::spawn(db.path.clone());

        // A pending cancel survives a projection request.
        worker.cancel.store(true, Ordering::Relaxed);
        worker.submit(Job::LoadLibrary {
            generation: 7,
            export: Some((export.clone(), "synthetic".into(), StatFormat::Xml)),
        });
        let event = wait_for(&worker, |e| {
            matches!(
                e,
                Event::LibraryLoaded { .. } | Event::LibraryLoadFailed { .. }
            )
        });
        assert!(
            matches!(event, Event::LibraryLoaded { generation: 7, .. }),
            "{event:?}"
        );
        assert!(
            export.exists(),
            "statistics exported by the background load"
        );
        assert!(worker.cancel.load(Ordering::Relaxed));
        assert!(
            worker.events.try_recv().is_err(),
            "no progress/finished events"
        );

        // The cancel only ever applies to work that was already running: the next job clears it.
        worker.submit(Job::QueueFullUpdate);
        wait_for(&worker, |e| matches!(e, Event::Finished));
        assert!(!worker.cancel.load(Ordering::Relaxed));
        assert_eq!(db.open().queue_len().unwrap(), 1, "titles dump queued");
    }

    #[test]
    fn idle_tracks_unfinished_jobs_and_survives_a_stopped_worker() {
        let db = TempDb::new();
        drop(db.open());
        let mut worker = Worker::spawn(db.path.clone());
        assert!(worker.is_idle());
        worker.submit(Job::QueueFullUpdate);
        wait_for(&worker, |e| matches!(e, Event::Finished));
        // `Finished` is sent before the job is counted off.
        let deadline = Instant::now() + Duration::from_secs(20);
        while !worker.is_idle() {
            assert!(Instant::now() < deadline, "worker never became idle");
            std::thread::sleep(Duration::from_millis(5));
        }

        worker.stop_and_join();
        worker.submit(Job::QueueFullUpdate);
        assert!(
            worker.is_idle(),
            "a job nobody can run must not stay counted"
        );
    }

    #[test]
    fn manual_statistics_export_runs_in_the_background_without_progress_events() {
        let db = TempDb::new();
        drop(db.open());
        let target = db.dir.join("manual.ini");
        let worker = Worker::spawn(db.path.clone());
        worker.submit(Job::ExportStatistics {
            path: target.clone(),
            username: "synthetic".into(),
            format: StatFormat::Ini,
        });
        let event = wait_for(&worker, |e| {
            matches!(e, Event::Synced { .. } | Event::Error(_))
        });
        assert!(
            matches!(&event, Event::Synced { changed: false, description } if description.contains("manual.ini")),
            "{event:?}"
        );
        assert!(target.exists());
        assert!(worker.events.try_recv().is_err());
    }

    #[test]
    fn dropping_the_worker_still_completes_an_accepted_export() {
        let db = TempDb::new();
        drop(db.open());
        let target = db.dir.join("on-exit.xml");
        let worker = Worker::spawn(db.path.clone());
        worker.submit(Job::ExportStatistics {
            path: target.clone(),
            username: "synthetic".into(),
            format: StatFormat::Xml,
        });
        drop(worker);
        assert!(
            target.exists(),
            "export finished before the worker went away"
        );
    }

    #[test]
    fn full_update_queues_each_known_file_and_anime_once() {
        let db = TempDb::new();
        db.open()
            .conn()
            .execute_batch(
                "INSERT INTO local_hash(id, ed2k, size) VALUES (1,'aa',1),(2,'bb',1),(3,'cc',1);
                 INSERT INTO local_file(id, hash_id, path) VALUES (1,1,'/s/1'),(2,1,'/s/2'),(3,2,'/s/3'),(4,3,'/s/4');
                 INSERT INTO file(fid, aid, ed2k, size) VALUES (10,100,'aa',1),(11,100,'bb',1);",
            )
            .unwrap();
        // TitlesDump + files 10 and 11 + anime 100; the duplicate and unidentified files add nothing.
        assert_eq!(queue_full_update(&db.path).unwrap(), 4);
        assert_eq!(
            queue_full_update(&db.path).unwrap(),
            0,
            "queueing is idempotent"
        );
        assert_eq!(db.open().queue_len().unwrap(), 4);
    }
}
