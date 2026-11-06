use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use animatic_anidb::error::TRANSIENT_COOLDOWN;
use animatic_anidb::http::HttpClient;
use animatic_anidb::limits::Limits;
use animatic_anidb::udp::{Config as UdpConfig, UdpClient};
use animatic_core::db::Db;
use animatic_core::file_manager::hash_and_register;
use animatic_core::file_manager::{MoveControl, MoveMethod, move_file};
use animatic_core::format;
use animatic_core::hash::Ed2kVariant;
use animatic_core::library::LibraryProjection;
use animatic_core::limits::{KNOWN_FILE_MIN_AGE, RequestKind};
use animatic_core::maintenance::{self, BackupInfo, CompactReport, IntegrityReport};
use animatic_core::rename::plan::{BatchRow, Candidate, Plan};
use animatic_core::settings::{AnidbSettings, FileManagerSettings, Settings, StatFormat};
use animatic_core::sync::{Command, CommandError, MAX_QUEUE_TRIES};
use animatic_core::update::{
    InstallationTarget, Platform, ReleaseInfo, UpdateClient, VerifiedUpdate,
};

use crate::scheduler::{self, ScanControl};
use crossbeam_channel::{Receiver, Sender};

/// One file of a rename job: the library entry and where it moves.
#[derive(Debug, Clone)]
pub struct RenameItem {
    pub local_id: i64,
    pub old: PathBuf,
    pub new: PathBuf,
}

#[derive(Debug)]
pub enum Job {
    Hash(Vec<PathBuf>),
    Scan {
        settings: FileManagerSettings,
        extensions: String,
    },
    /// Import an AOM databank. A database that already holds data is not written until the user
    /// confirms (`confirmed`), because rows with the same ids are replaced; without the
    /// confirmation the job only answers with `Event::LegacyImportNeedsConfirmation`.
    /// Decrypted copies go to a marked folder below `temp_root` (see `legacy::workdir`).
    ImportLegacy {
        source: PathBuf,
        temp_root: PathBuf,
        confirmed: bool,
    },
    ImportLegacySettings {
        source: PathBuf,
    },
    /// Back the library up into `dest` (online backup, off the UI thread).
    BackupLibrary {
        dest: PathBuf,
    },
    /// Check that `source` may replace the library; the swap itself is `spawn_restore`.
    ValidateBackup {
        source: PathBuf,
    },
    /// Integrity and foreign key check of the library; never repairs.
    CheckLibrary,
    /// `VACUUM` the library.
    CompactLibrary,
    ProcessQueue(AnidbSettings),
    /// Look for leftover marked import folders below `temp_root` (see `legacy::workdir`).
    FindLegacyLeftovers {
        temp_root: PathBuf,
    },
    /// Remove the given leftover import folders. Only unused marked folders directly below
    /// `temp_root` are removed.
    RemoveLegacyLeftovers {
        temp_root: PathBuf,
        folders: Vec<PathBuf>,
    },
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
    /// Build and write the hash export of every hashed local file on the projection thread
    /// (no started/finished events).
    ExportHashes {
        path: PathBuf,
        template: String,
        copy_length_as_minutes: bool,
    },
    /// Rename apply: move each file to its planned path, in order, and record the new paths.
    /// Moves across volumes copy, verify and swap (see `file_manager::move_file`). Cancel stops
    /// before the next file and during a copy; every file gets a `Renamed` or `RenameFailed`.
    RenameFiles(Vec<RenameItem>),
    /// Plan a batch rename on the projection thread (answered by `Event::RenamesPlanned`; no
    /// started/finished events, never queued behind a long main-worker job).
    PlanRenames {
        request: u64,
        pattern: String,
        files: Vec<Candidate>,
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
    /// The database already holds data and the import has not been confirmed; nothing was written.
    LegacyImportNeedsConfirmation {
        source: PathBuf,
    },
    /// Unused marked import folders found at startup.
    LegacyLeftovers {
        temp_root: PathBuf,
        folders: Vec<PathBuf>,
    },
    LibraryBackedUp {
        path: PathBuf,
        bytes: u64,
    },
    /// A restore source passed validation; the UI asks for confirmation.
    RestoreCandidate {
        source: PathBuf,
        info: BackupInfo,
    },
    LibraryChecked(Box<IntegrityReport>),
    LibraryCompacted(CompactReport),
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
    /// A notice for the activity log (not an error and not tied to a job).
    Info(String),
    LibraryLoaded {
        generation: u64,
        projection: Box<LibraryProjection>,
    },
    LibraryLoadFailed {
        generation: u64,
        message: String,
    },
    /// One file was moved; the library already records `new`.
    Renamed {
        local_id: i64,
        old: PathBuf,
        new: PathBuf,
        /// Copied to another volume (and verified) instead of renamed in place.
        copied: bool,
        /// The old file could not be removed after a verified copy.
        note: Option<String>,
    },
    /// One file was not moved; the file and its library entry are unchanged.
    RenameFailed {
        local_id: i64,
        old: PathBuf,
        new: PathBuf,
        message: String,
    },
    /// End of a rename job (before `Finished`).
    RenameSummary {
        renamed: usize,
        not_renamed: usize,
    },
    /// Answer to `Job::PlanRenames`, one row per requested file in request order.
    RenamesPlanned {
        request: u64,
        rows: Vec<BatchRow>,
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
    /// Settings and blocking state for the scheduled scan; see [`crate::scheduler`].
    scan: Arc<ScanControl>,
    scheduler_stop: Option<Sender<()>>,
    scheduler: Option<std::thread::JoinHandle<()>>,
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
        let scheduler_events = event_tx.clone();
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
        let scan = Arc::new(ScanControl::new());
        let (scheduler_stop, scheduler_stop_rx) = crossbeam_channel::bounded(1);
        let scheduler = scheduler::spawn(
            scan.clone(),
            job_tx.clone(),
            pending.clone(),
            scheduler_events,
            scheduler_stop_rx,
            scheduler::POLL,
        );
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
            scan,
            scheduler_stop: Some(scheduler_stop),
            scheduler: Some(scheduler),
        }
    }

    /// The channel to the scan scheduler thread.
    pub fn scan_control(&self) -> &ScanControl {
        &self.scan
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
            Job::ExportHashes {
                path,
                template,
                copy_length_as_minutes,
            } => {
                let _ = self.loads.send(Background::ExportHashes {
                    path,
                    template,
                    copy_length_as_minutes,
                });
            }
            Job::PlanRenames {
                request,
                pattern,
                files,
            } => {
                let _ = self.loads.send(Background::PlanRenames {
                    request,
                    pattern,
                    files,
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
        // Dropping the sender wakes the scheduler thread, which then ends without submitting.
        drop(self.scheduler_stop.take());
        if let Some(scheduler) = self.scheduler.take() {
            let _ = scheduler.join();
        }
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

/// Submit `job` only if no job of the main worker thread is queued or running; the claim is a
/// single atomic step, so a manual submission cannot slip in between check and submit. Used by
/// the scan scheduler, which must never queue behind other work.
pub(crate) fn submit_if_idle(jobs: &Sender<Job>, pending: &AtomicUsize, job: Job) -> bool {
    if pending
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return false;
    }
    if jobs.send(job).is_err() {
        pending.fetch_sub(1, Ordering::SeqCst);
        return false;
    }
    true
}

/// Log text for files a scan held back because they may still be written.
fn deferred_note(deferred: &[PathBuf]) -> String {
    const SHOWN: usize = 5;
    let names: Vec<String> = deferred
        .iter()
        .take(SHOWN)
        .map(|p| p.display().to_string())
        .collect();
    let more = deferred.len().saturating_sub(SHOWN);
    let more = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    format!(
        "Scan deferred {} file(s) that may still be written, the next scan retries them: {}{more}",
        deferred.len(),
        names.join(", ")
    )
}

/// One counted job of the main worker thread; the count drops when the job is finished.
struct PendingJob(Option<Arc<AtomicUsize>>);

impl PendingJob {
    /// Only jobs that `Worker::submit` counted are uncounted again here: `Stop` and projection
    /// requests are never counted.
    fn of(job: &Job, pending: &Arc<AtomicUsize>) -> PendingJob {
        match job {
            Job::Stop
            | Job::LoadLibrary { .. }
            | Job::ExportStatistics { .. }
            | Job::ExportHashes { .. }
            | Job::PlanRenames { .. } => PendingJob(None),
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
    // One limiter for the whole process: every queue run builds new clients, but they all take
    // their send slots from here, and the send times are saved so a restart keeps the spacing.
    let network = Network::new(animatic_core::limits::open_limits(&db_path));
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
            Job::ExportHashes {
                path,
                template,
                copy_length_as_minutes,
            } => {
                let _ = loads.send(Background::ExportHashes {
                    path,
                    template,
                    copy_length_as_minutes,
                });
                continue;
            }
            Job::PlanRenames {
                request,
                pattern,
                files,
            } => {
                let _ = loads.send(Background::PlanRenames {
                    request,
                    pattern,
                    files,
                });
                continue;
            }
            other => other,
        };
        // A cancel request applies to the job that is running when it is made. It is only
        // cleared here, when the next job starts, so a following `submit` cannot swallow it.
        cancel.store(false, Ordering::Relaxed);
        match job {
            Job::Stop
            | Job::LoadLibrary { .. }
            | Job::ExportStatistics { .. }
            | Job::ExportHashes { .. }
            | Job::PlanRenames { .. } => {
                unreachable!("handled before dispatch")
            }
            Job::RenameFiles(items) => run_renames(&db_path, items, &events, &cancel, &pause),
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
            Job::ProcessQueue(settings) => {
                process_queue(&db_path, settings, &events, &cancel, &network)
            }
            Job::QueueFullUpdate => {
                let _ = events.send(Event::Started {
                    label: "Queueing full update".into(),
                    total: 0,
                });
                match queue_full_update(&db_path) {
                    Ok(report) => {
                        let _ = events.send(Event::Synced {
                            description: report.describe(),
                            changed: report.queued > 0,
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
                // Files that may still be written wait for a later scan (roadmap M5.2).
                let settled = animatic_core::file_manager::settle_scan(
                    actions,
                    settings.settle_delay(),
                    std::time::SystemTime::now(),
                    &mut |wait| animatic_core::file_manager::interruptible_sleep(wait, &cancel),
                );
                if !settled.deferred.is_empty() {
                    let _ = events.send(Event::Info(deferred_note(&settled.deferred)));
                }
                let actions = settled.ready;
                let total = actions.len();
                let _ = events.send(Event::Started {
                    label: "Scanning managed folders".into(),
                    total,
                });
                let pause_between_files = settings.file_pause();
                let mut hashed_before = false;
                for (index, action) in actions.into_iter().enumerate() {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    match action {
                        animatic_core::file_manager::ScanAction::Hash { path, .. }
                        | animatic_core::file_manager::ScanAction::Rehash { path, .. } => {
                            if hashed_before
                                && !animatic_core::file_manager::interruptible_sleep(
                                    pause_between_files,
                                    &cancel,
                                )
                            {
                                break;
                            }
                            hashed_before = true;
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
            Job::BackupLibrary { dest } => {
                let _ = events.send(Event::Started {
                    label: "Backing up library".into(),
                    total: 1,
                });
                match Db::open(&db_path).and_then(|db| maintenance::backup_to(&db, &dest)) {
                    Ok(bytes) => {
                        let _ = events.send(Event::LibraryBackedUp { path: dest, bytes });
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(format!("Backup failed: {e}")));
                    }
                }
                let _ = events.send(Event::Finished);
            }
            Job::ValidateBackup { source } => {
                let _ = events.send(Event::Started {
                    label: "Checking backup file".into(),
                    total: 1,
                });
                match maintenance::check_restore_source(&db_path, &source)
                    .and_then(|()| maintenance::validate_backup(&source))
                {
                    Ok(info) => {
                        let _ = events.send(Event::RestoreCandidate { source, info });
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(format!(
                            "Restore refused, the current library was not changed: {e}"
                        )));
                    }
                }
                let _ = events.send(Event::Finished);
            }
            Job::CheckLibrary => {
                let _ = events.send(Event::Started {
                    label: "Checking library".into(),
                    total: 1,
                });
                match maintenance::check_file(&db_path) {
                    Ok(report) => {
                        let _ = events.send(Event::LibraryChecked(Box::new(report)));
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(format!("Library check failed: {e}")));
                    }
                }
                let _ = events.send(Event::Finished);
            }
            Job::CompactLibrary => {
                let _ = events.send(Event::Started {
                    label: "Compacting library".into(),
                    total: 1,
                });
                match Db::open(&db_path).and_then(|db| maintenance::compact(&db)) {
                    Ok(report) => {
                        let _ = events.send(Event::LibraryCompacted(report));
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(format!("Compacting failed: {e}")));
                    }
                }
                let _ = events.send(Event::Finished);
            }
            Job::ImportLegacy {
                source,
                temp_root,
                confirmed,
            } => {
                let mut db = match open_for_legacy_import(&db_path, confirmed) {
                    Ok(Some(db)) => db,
                    Ok(None) => {
                        let _ = events.send(Event::LegacyImportNeedsConfirmation { source });
                        continue;
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(format!("Legacy import failed: {e}")));
                        let _ = events.send(Event::Finished);
                        continue;
                    }
                };
                let _ = events.send(Event::Started {
                    label: "Importing legacy databank".into(),
                    total: 1,
                });
                let result = animatic_core::legacy::import::import_databank(
                    &mut db,
                    &source,
                    &temp_root,
                    &mut |label, fraction| {
                        let _ = events.send(Event::Progress {
                            label: label.into(),
                            current: 1,
                            total: 1,
                            fraction,
                        });
                        !cancel.load(Ordering::Relaxed)
                    },
                );
                match result {
                    Ok(report) => {
                        let records: usize = report.imported.iter().map(|(_, n)| *n).sum();
                        let _ = events.send(Event::Imported {
                            summary: format!(
                                "Imported {records} records from {} collections",
                                report.imported.len()
                            ),
                        });
                        for warning in report.warnings {
                            let _ = events
                                .send(Event::Error(format!("Legacy import warning: {warning}")));
                        }
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(format!("Legacy import failed: {e}")));
                    }
                }
                let _ = events.send(Event::Finished);
            }
            Job::FindLegacyLeftovers { temp_root } => {
                match animatic_core::legacy::workdir::find_leftover_workdirs(&temp_root) {
                    Ok(folders) if !folders.is_empty() => {
                        let _ = events.send(Event::LegacyLeftovers { temp_root, folders });
                    }
                    Ok(_) => {}
                    Err(e) => log::warn!("could not look for leftover import folders: {e}"),
                }
            }
            Job::RemoveLegacyLeftovers { temp_root, folders } => {
                let outcome =
                    animatic_core::legacy::workdir::remove_leftover_workdirs(&temp_root, &folders);
                if !outcome.removed.is_empty() {
                    let _ = events.send(Event::Synced {
                        description: format!(
                            "Removed {} leftover import folder(s) from {}",
                            outcome.removed.len(),
                            temp_root.display()
                        ),
                        changed: false,
                    });
                }
                for (folder, reason) in outcome.kept {
                    let _ = events.send(Event::Error(format!(
                        "Kept leftover import folder {}: {reason}",
                        folder.display()
                    )));
                }
            }
        }
    }
}

/// What the restore thread reports: the outcome, or why the library was not replaced.
pub type RestoreResult = Result<maintenance::RestoreOutcome, String>;

/// Replace the library file with the backup `source` on a thread of its own (it copies and
/// validates whole databases). The caller must have stopped the worker and the projection
/// thread and closed its own connection first: nothing may have `db_path` open meanwhile.
/// If the thread dies without a result the receiver reports a disconnect.
pub fn spawn_restore(db_path: PathBuf, source: PathBuf) -> Receiver<RestoreResult> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::Builder::new()
        .name("animatic-restore".into())
        .spawn(move || {
            let _ = tx
                .send(maintenance::restore_database(&db_path, &source).map_err(|e| e.to_string()));
        })
        .expect("start restore thread");
    rx
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
    ExportHashes {
        path: PathBuf,
        template: String,
        copy_length_as_minutes: bool,
    },
    PlanRenames {
        request: u64,
        pattern: String,
        files: Vec<Candidate>,
    },
}

/// Serve library projections and the statistics and hash exports on a thread of their own so that an
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
                Background::ExportHashes {
                    path,
                    template,
                    copy_length_as_minutes,
                } => export_hashes(
                    &db_path,
                    &mut slot,
                    &path,
                    &template,
                    copy_length_as_minutes,
                ),
                Background::PlanRenames {
                    request,
                    pattern,
                    files,
                } => plan_renames(&db_path, &mut slot, request, &pattern, &files),
            };
            let _ = events.send(event);
        }
        if stopping {
            return;
        }
    }
}

/// Plan a batch rename on the projection thread. A library that cannot be opened blocks every
/// row with that reason instead of leaving the UI waiting for an answer.
fn plan_renames(
    db_path: &Path,
    slot: &mut Option<Db>,
    request: u64,
    pattern: &str,
    files: &[Candidate],
) -> Event {
    let rows = match with_projection_db(db_path, slot, |db| {
        Ok(animatic_core::rename::plan::plan_batch(db, pattern, files))
    }) {
        Ok(rows) => rows,
        Err(e) => files
            .iter()
            .map(|file| BatchRow {
                local_id: file.local_id,
                old: file.path.clone(),
                plan: Plan {
                    errors: vec![format!("Unable to open the library: {e}")],
                    ..Plan::default()
                },
            })
            .collect(),
    };
    Event::RenamesPlanned { request, rows }
}

/// Rename apply (`Job::RenameFiles`): one `file_manager::move_file` per item, each reported.
fn run_renames(
    db_path: &Path,
    items: Vec<RenameItem>,
    events: &Sender<Event>,
    cancel: &AtomicBool,
    pause: &AtomicBool,
) {
    let total = items.len();
    let _ = events.send(Event::Started {
        label: "Renaming files".into(),
        total,
    });
    let db = Db::open(db_path);
    let (mut renamed, mut not_renamed) = (0, 0);
    for (index, item) in items.into_iter().enumerate() {
        let RenameItem { local_id, old, new } = item;
        let fail = |message: String| Event::RenameFailed {
            local_id,
            old: old.clone(),
            new: new.clone(),
            message,
        };
        let result = match &db {
            _ if cancel.load(Ordering::Relaxed) => Err("Skipped: cancelled".to_string()),
            Err(e) => Err(format!("Unable to open the library: {e}")),
            Ok(db) => {
                let label = old
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                move_file(
                    db,
                    local_id,
                    &old,
                    &new,
                    MoveControl {
                        cancel,
                        pause,
                        progress: &mut |done, bytes| {
                            let _ = events.send(Event::Progress {
                                label: label.clone(),
                                current: index + 1,
                                total,
                                fraction: done as f32 / bytes.max(1) as f32,
                            });
                        },
                    },
                )
                .map_err(|e| match e {
                    animatic_core::error::Error::Cancelled => {
                        "Cancelled; the file and its library entry are unchanged".to_string()
                    }
                    e => e.to_string(),
                })
            }
        };
        match result {
            Ok(outcome) => {
                renamed += 1;
                let _ = events.send(Event::Renamed {
                    local_id,
                    old: old.clone(),
                    new: new.clone(),
                    copied: outcome.method == MoveMethod::Copied,
                    note: outcome.source_left,
                });
            }
            Err(message) => {
                not_renamed += 1;
                let _ = events.send(fail(message));
            }
        }
    }
    let _ = events.send(Event::RenameSummary {
        renamed,
        not_renamed,
    });
    let _ = events.send(Event::Finished);
}

/// Run `export` on the projection thread's connection and keep that connection afterwards.
fn with_projection_db<T>(
    db_path: &Path,
    slot: &mut Option<Db>,
    export: impl FnOnce(&Db) -> animatic_core::error::Result<T>,
) -> animatic_core::error::Result<T> {
    let db = match slot.take() {
        Some(db) => db,
        None => Db::open(db_path)?,
    };
    let result = export(&db);
    *slot = Some(db);
    result
}

fn export_hashes(
    db_path: &Path,
    slot: &mut Option<Db>,
    path: &Path,
    template: &str,
    copy_length_as_minutes: bool,
) -> Event {
    let result = with_projection_db(db_path, slot, |db| {
        db.export_local_hashes(path, template, copy_length_as_minutes)
    });
    match result {
        Ok(count) => Event::Synced {
            description: format!("Exported {count} hashes to {}", path.display()),
            changed: false,
        },
        Err(e) => Event::Error(format!("Hash export failed: {e}")),
    }
}

fn export_statistics(
    db_path: &Path,
    slot: &mut Option<Db>,
    path: &Path,
    username: &str,
    format: StatFormat,
) -> Event {
    let result = with_projection_db(db_path, slot, |db| {
        db.library_stats()
            .and_then(|stats| stats.write(path, username, format))
    });
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

/// What outlives one queue run in the worker: the limiter every client of every run draws its
/// send slots from, and the reply timeout of the UDP clients.
struct Network {
    limits: Arc<Limits>,
    udp_timeout: Duration,
}

impl Network {
    fn new(limits: Arc<Limits>) -> Network {
        Network {
            limits,
            udp_timeout: UdpConfig::default().timeout,
        }
    }
}

/// Run the AniDB command queue until it is empty, cancelled, blocked or put on cooldown.
fn process_queue(
    db_path: &Path,
    settings: AnidbSettings,
    events: &Sender<Event>,
    cancel: &AtomicBool,
    network: &Network,
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
    let mut udp = UdpClient::new(
        UdpConfig {
            server: settings.udp_server.clone(),
            local_port: settings.local_port,
            client: settings.client_name.clone(),
            client_version: settings.client_version,
            username: settings.username.clone(),
            password: settings.password.clone(),
            api_key: (!settings.api_key.is_empty()).then(|| settings.api_key.clone()),
            timeout: network.udp_timeout,
        },
        network.limits.clone(),
    );
    let mut http = HttpClient::new(
        &settings.client_name,
        settings.client_version,
        network.limits.clone(),
    );
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

/// Open the database for a legacy import, or return `None` when the user must confirm first:
/// the import replaces rows that share an id with a databank row, so a database that already
/// holds data is only written after confirmation. An empty database needs none.
fn open_for_legacy_import(db_path: &Path, confirmed: bool) -> animatic_core::Result<Option<Db>> {
    let db = Db::open(db_path)?;
    if !confirmed && !db.is_empty()? {
        return Ok(None);
    }
    Ok(Some(db))
}

/// What a full update queued, and what AniDB's limits left out of it.
#[derive(Debug, Default, PartialEq, Eq)]
struct FullUpdateReport {
    queued: usize,
    /// The title dump is not requested again before this epoch second.
    titles_dump_after: Option<i64>,
    /// Anime left out because they were requested less than a day ago.
    recent_anime: usize,
    /// Files left out because they were requested less than seven days ago.
    recent_files: usize,
}

impl FullUpdateReport {
    fn describe(&self) -> String {
        let mut left_out = Vec::new();
        if let Some(until) = self.titles_dump_after {
            left_out.push(format!(
                "the title dump (allowed again after {})",
                format::datetime_local(until)
            ));
        }
        if self.recent_anime > 0 {
            left_out.push(format!(
                "{} anime requested within the last day",
                self.recent_anime
            ));
        }
        if self.recent_files > 0 {
            left_out.push(format!(
                "{} files requested within the last {} days",
                self.recent_files,
                KNOWN_FILE_MIN_AGE.as_secs() / 86_400
            ));
        }
        let mut text = format!("Queued {} commands for the full update", self.queued);
        if !left_out.is_empty() {
            text.push_str(&format!(
                "; left out because AniDB's limits do not allow them yet: {}",
                left_out.join(", ")
            ));
        }
        text
    }
}

/// Queue the commands of a full update (`--full-update`): the titles dump plus every anime and
/// file known from hashed local files, except what AniDB's limits do not allow yet (the title
/// dump once a day, an anime a day, a known file seven days after the last attempt).
fn queue_full_update(db_path: &Path) -> animatic_core::Result<FullUpdateReport> {
    let db = Db::open(db_path)?;
    let now = format::now();
    let mut fids = BTreeSet::new();
    let mut aids = BTreeSet::new();
    for file in db.local_files()? {
        fids.extend(file.fid);
        aids.extend(file.aid);
    }
    let (files_known, anime_known) = (fids.len(), aids.len());
    let recent_files = db.recent_requests(RequestKind::File, now)?;
    let recent_anime = db.recent_requests(RequestKind::Anime, now)?;
    fids.retain(|fid| !recent_files.contains(fid));
    aids.retain(|aid| !recent_anime.contains(aid));
    let titles_dump_after = db.titles_dump_next_allowed(now)?;
    let mut report = FullUpdateReport {
        queued: 0,
        titles_dump_after,
        recent_anime: anime_known - aids.len(),
        recent_files: files_known - fids.len(),
    };
    let mut commands = Vec::with_capacity(1 + fids.len() + aids.len());
    if titles_dump_after.is_none() {
        commands.push(Command::TitlesDump);
    }
    commands.extend(fids.into_iter().map(|fid| Command::FetchFile { fid }));
    commands.extend(aids.into_iter().map(|aid| Command::FetchAnime { aid }));
    report.queued = db.queue_push_many(&commands)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

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

    /// The worker's network state for a test: the real limiter code over `db`, on a clock that
    /// moves only when the limiter sleeps, so spacing is exact and the test does not wait.
    fn network_for(db: &TempDb, clock: &Arc<ManualClock>) -> Network {
        Network {
            limits: animatic_core::limits::open_limits_with_clock(&db.path, clock.clone()),
            udp_timeout: Duration::from_secs(5),
        }
    }

    fn run_queue_with(db: &TempDb, settings: &AnidbSettings, network: &Network) -> Vec<Event> {
        let (tx, rx) = crossbeam_channel::unbounded();
        process_queue(
            &db.path,
            settings.clone(),
            &tx,
            &AtomicBool::new(false),
            network,
        );
        rx.try_iter().collect()
    }

    /// Run the queue once as a fresh job (new clients and, as after a restart, a new limiter
    /// over the saved state) and return its events.
    fn run_queue(db: &TempDb, settings: &AnidbSettings) -> Vec<Event> {
        let network = network_for(db, &Arc::new(ManualClock::default()));
        run_queue_with(db, settings, &network)
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
        assert!(
            until >= started + 29 * 60,
            "conservative ban cooldown: {}s after {} packet(s)",
            until - started,
            fixture.packets()
        );
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
        assert!(
            until >= started + 29 * 60,
            "ban cooldown: {}s after {} packet(s)",
            until - started,
            fixture.packets()
        );
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

    fn register_media(db: &Db, path: &Path, data: &[u8]) -> i64 {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
        let hashes = animatic_core::hash::hash_bytes(data, Ed2kVariant::Red);
        db.add_local_file(&path.to_string_lossy(), &hashes, 0)
            .unwrap()
    }

    fn recorded(db: &Db, id: i64) -> String {
        db.local_file_record(id).unwrap().unwrap().0
    }

    #[test]
    fn a_rename_job_moves_each_file_and_reports_every_outcome() {
        let db = TempDb::new();
        let library = db.open();
        let media = db.dir.join("animatic-test-media");
        let (a, b, c) = (
            register_media(&library, &media.join("a.mkv"), b"synthetic a"),
            register_media(&library, &media.join("b.mkv"), b"synthetic b"),
            register_media(&library, &media.join("c.mkv"), b"synthetic c"),
        );
        std::fs::write(media.join("taken.mkv"), b"someone else's file").unwrap();
        let item = |id, from: &str, to: &str| RenameItem {
            local_id: id,
            old: media.join(from),
            new: media.join(to),
        };
        let worker = Worker::spawn(db.path.clone());

        worker.submit(Job::RenameFiles(vec![
            item(a, "a.mkv", "Show 01.mkv"),
            item(c, "c.mkv", "taken.mkv"),
            item(b, "b.mkv", "sub/Show 02.mkv"),
        ]));
        let mut renamed = Vec::new();
        let mut failed = Vec::new();
        let mut summary = None;
        wait_for(&worker, |e| {
            match e {
                Event::Renamed { local_id, .. } => renamed.push(*local_id),
                Event::RenameFailed {
                    local_id, message, ..
                } => failed.push((*local_id, message.clone())),
                Event::RenameSummary {
                    renamed,
                    not_renamed,
                } => summary = Some((*renamed, *not_renamed)),
                _ => {}
            }
            matches!(e, Event::Finished)
        });

        assert_eq!(renamed, [a, b]);
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].0, c);
        assert!(failed[0].1.contains("already exists"), "{failed:?}");
        assert_eq!(summary, Some((2, 1)));
        let library = db.open();
        assert_eq!(
            recorded(&library, a),
            media.join("Show 01.mkv").to_string_lossy()
        );
        assert_eq!(
            recorded(&library, b),
            media.join("sub/Show 02.mkv").to_string_lossy()
        );
        assert_eq!(recorded(&library, c), media.join("c.mkv").to_string_lossy());
        assert!(media.join("c.mkv").exists());
        assert_eq!(
            std::fs::read(media.join("taken.mkv")).unwrap(),
            b"someone else's file"
        );
    }

    #[test]
    fn a_cancelled_rename_job_changes_nothing_and_accounts_for_every_file() {
        let db = TempDb::new();
        let library = db.open();
        let media = db.dir.join("animatic-test-media");
        let a = register_media(&library, &media.join("a.mkv"), b"synthetic a");
        let b = register_media(&library, &media.join("b.mkv"), b"synthetic b");
        let (tx, rx) = crossbeam_channel::unbounded();
        let items = vec![
            RenameItem {
                local_id: a,
                old: media.join("a.mkv"),
                new: media.join("Show A.mkv"),
            },
            RenameItem {
                local_id: b,
                old: media.join("b.mkv"),
                new: media.join("Show B.mkv"),
            },
        ];

        run_renames(
            &db.path,
            items,
            &tx,
            &AtomicBool::new(true),
            &AtomicBool::new(false),
        );

        let events: Vec<Event> = rx.try_iter().collect();
        let skipped = events
            .iter()
            .filter(|e| matches!(e, Event::RenameFailed { message, .. } if message.contains("cancelled")))
            .count();
        assert_eq!(skipped, 2);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::RenameSummary {
                renamed: 0,
                not_renamed: 2
            }
        )));
        assert!(matches!(events.last(), Some(Event::Finished)));
        assert!(media.join("a.mkv").exists() && media.join("b.mkv").exists());
        assert!(!media.join("Show A.mkv").exists() && !media.join("Show B.mkv").exists());
        assert_eq!(
            recorded(&db.open(), a),
            media.join("a.mkv").to_string_lossy()
        );
    }

    #[test]
    fn batch_planning_answers_from_the_projection_thread_without_job_events() {
        use animatic_anidb::commands::{EpNo, FileInfo};
        let db = TempDb::new();
        let library = db.open();
        let media = db.dir.join("animatic-test-media");
        library
            .store_file_info(&FileInfo {
                fid: 1,
                aid: 2,
                eid: 21,
                size: 1000,
                ed2k: Some("ab".repeat(16)),
                anime_romaji: Some("Synthetic Show".into()),
                epno: EpNo { kind: 1, number: 1 },
                ..FileInfo::default()
            })
            .unwrap();
        let worker = Worker::spawn(db.path.clone());

        worker.submit(Job::PlanRenames {
            request: 5,
            pattern: "A.Name + ' ' + E.EpisodeNo + '.mkv'".into(),
            files: vec![
                Candidate {
                    local_id: 1,
                    path: media.join("one.mkv"),
                    fid: Some(1),
                    base_path: String::new(),
                },
                Candidate {
                    local_id: 2,
                    path: media.join("two.mkv"),
                    fid: None,
                    base_path: String::new(),
                },
            ],
        });

        match wait_for(&worker, |e| matches!(e, Event::RenamesPlanned { .. })) {
            Event::RenamesPlanned { request, rows } => {
                assert_eq!(request, 5);
                assert_eq!(rows.len(), 2);
                assert!(rows[0].plan.can_apply(), "{:?}", rows[0]);
                assert!(!rows[1].plan.can_apply() && !rows[1].plan.errors.is_empty());
            }
            other => panic!("{other:?}"),
        }
        assert!(worker.is_idle(), "planning never occupies the main worker");
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

    /// A managed folder with `names` as small synthetic media files, and settings that hash it.
    fn media_folder(db: &TempDb, names: &[&str]) -> (PathBuf, FileManagerSettings) {
        let root = db.dir.join("media");
        std::fs::create_dir_all(&root).unwrap();
        for name in names {
            std::fs::write(root.join(name), name.as_bytes()).unwrap();
        }
        let settings = FileManagerSettings {
            action_delay_ms: 0,
            managers: vec![animatic_core::settings::FileManager {
                name: "Synthetic media".into(),
                path: root.to_string_lossy().into_owned(),
                auto_hash: true,
                include_subdirs: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        (root, settings)
    }

    #[test]
    fn scan_pauses_between_hashed_files_but_not_after_the_last() {
        let db = TempDb::new();
        drop(db.open());
        let (_, mut settings) = media_folder(&db, &["a.mkv", "b.mkv", "c.mkv"]);
        settings.sleep_time_ms = 400;
        let worker = Worker::spawn(db.path.clone());
        let started = Instant::now();

        worker.submit(Job::Scan {
            settings,
            extensions: "mkv".into(),
        });
        let mut hashed = Vec::new();
        wait_for(&worker, |e| {
            if matches!(e, Event::Hashed { .. }) {
                hashed.push(Instant::now());
            }
            matches!(e, Event::Finished)
        });

        assert_eq!(hashed.len(), 3);
        for pair in hashed.windows(2) {
            assert!(
                pair[1] - pair[0] >= Duration::from_millis(300),
                "files are paced: {:?}",
                pair[1] - pair[0]
            );
        }
        let total = started.elapsed();
        assert!(
            total < Duration::from_millis(400 * 2 + 1500),
            "no pause after the last file: {total:?}"
        );
    }

    #[test]
    fn scan_defers_a_file_modified_within_the_settle_delay_until_a_later_scan() {
        let db = TempDb::new();
        drop(db.open());
        let (_, mut settings) = media_folder(&db, &["still-copying.mkv"]);
        settings.action_delay_ms = 600_000;
        let worker = Worker::spawn(db.path.clone());

        worker.submit(Job::Scan {
            settings: settings.clone(),
            extensions: "mkv".into(),
        });
        let mut hashed = false;
        let mut notice = None;
        wait_for(&worker, |e| {
            match e {
                Event::Hashed { .. } => hashed = true,
                Event::Info(text) => notice = Some(text.clone()),
                _ => {}
            }
            matches!(e, Event::Finished)
        });
        assert!(!hashed, "a file written moments ago must not be hashed yet");
        assert!(notice.is_some_and(|n| n.contains("deferred 1 file")));
        assert_eq!(db.open().local_files().unwrap().len(), 0);

        // Once it counts as settled (no settle delay), the next scan hashes it.
        settings.action_delay_ms = 0;
        worker.submit(Job::Scan {
            settings,
            extensions: "mkv".into(),
        });
        wait_for(&worker, |e| matches!(e, Event::Hashed { .. }));
        wait_for(&worker, |e| matches!(e, Event::Finished));
        assert_eq!(db.open().local_files().unwrap().len(), 1);
    }

    #[test]
    fn requested_scan_runs_on_the_worker_once_nothing_blocks_it() {
        let db = TempDb::new();
        drop(db.open());
        let (_, settings) = media_folder(&db, &["scheduled.mkv"]);
        let mut worker = Worker::spawn(db.path.clone());
        let extensions = vec!["mkv".to_string()];
        worker
            .scan_control()
            .sync(&settings, &extensions, Some("an import is waiting"));
        worker.scan_control().request_scan();

        wait_for(
            &worker,
            |e| matches!(e, Event::Info(text) if text.contains("postponed")),
        );
        assert!(db.open().local_files().unwrap().is_empty());

        worker.scan_control().sync(&settings, &extensions, None);
        wait_for(&worker, |e| matches!(e, Event::Hashed { .. }));
        wait_for(&worker, |e| matches!(e, Event::Finished));
        assert_eq!(db.open().local_files().unwrap().len(), 1);

        let stopping = Instant::now();
        worker.stop_and_join();
        assert!(stopping.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn library_backup_check_validation_and_restore_work_end_to_end() {
        use animatic_core::sync::Command;
        let db = TempDb::new();
        let live = db.open();
        live.queue_push(&Command::TitlesDump).unwrap();
        let backup = db.dir.join("backup.sqlite3");
        let mut worker = Worker::spawn(db.path.clone());

        worker.submit(Job::BackupLibrary {
            dest: backup.clone(),
        });
        match wait_for(&worker, |e| {
            matches!(e, Event::LibraryBackedUp { .. } | Event::Error(_))
        }) {
            Event::LibraryBackedUp { path, bytes } => {
                assert_eq!(path, backup);
                assert_eq!(bytes, std::fs::metadata(&backup).unwrap().len());
            }
            other => panic!("backup failed: {other:?}"),
        }
        wait_for(&worker, |e| matches!(e, Event::Finished));
        live.queue_push(&Command::MylistStats).unwrap();

        worker.submit(Job::CheckLibrary);
        match wait_for(&worker, |e| matches!(e, Event::LibraryChecked(_))) {
            Event::LibraryChecked(report) => assert!(report.is_ok(), "{report:?}"),
            other => panic!("{other:?}"),
        }
        wait_for(&worker, |e| matches!(e, Event::Finished));

        let garbage = db.dir.join("garbage.sqlite3");
        std::fs::write(&garbage, b"synthetic garbage ".repeat(300)).unwrap();
        worker.submit(Job::ValidateBackup { source: garbage });
        match wait_for(&worker, |e| {
            matches!(e, Event::RestoreCandidate { .. } | Event::Error(_))
        }) {
            Event::Error(message) => assert!(message.contains("not changed"), "{message}"),
            other => panic!("a damaged file must be refused: {other:?}"),
        }
        wait_for(&worker, |e| matches!(e, Event::Finished));
        worker.submit(Job::ValidateBackup {
            source: backup.clone(),
        });
        match wait_for(&worker, |e| {
            matches!(e, Event::RestoreCandidate { .. } | Event::Error(_))
        }) {
            Event::RestoreCandidate { source, info } => {
                assert_eq!(source, backup);
                assert_eq!(info.queued_commands, 1);
            }
            other => panic!("{other:?}"),
        }
        wait_for(&worker, |e| matches!(e, Event::Finished));

        // What the confirmed restore does: stop the threads, close every connection, swap.
        worker.stop_and_join();
        drop(live);
        let outcome = spawn_restore(db.path.clone(), backup)
            .recv_timeout(Duration::from_secs(20))
            .expect("restore thread reports")
            .expect("restore succeeds");
        assert_eq!(db.open().queue_len().unwrap(), 1, "the backup's queue");
        let previous = outcome.previous_copy.expect("the replaced library is kept");
        assert_eq!(Db::open(&previous).unwrap().queue_len().unwrap(), 2);
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
    fn hash_export_runs_in_the_background_and_a_failure_keeps_the_previous_file() {
        let db = TempDb::new();
        db.open()
            .conn()
            .execute_batch(
                "INSERT INTO local_hash(id, ed2k, size) VALUES (1,'aa',1);
                 INSERT INTO local_file(id, hash_id, path) VALUES (1,1,'/synthetic/animatic-test-user/a.mkv');",
            )
            .unwrap();
        let target = db.dir.join("hashes.txt");
        let worker = Worker::spawn(db.path.clone());
        worker.submit(Job::ExportHashes {
            path: target.clone(),
            template: "%filename%".into(),
            copy_length_as_minutes: false,
        });
        let event = wait_for(&worker, |e| {
            matches!(e, Event::Synced { .. } | Event::Error(_))
        });
        assert!(
            matches!(&event, Event::Synced { changed: false, description } if description.contains("Exported 1 hashes")),
            "{event:?}"
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "a.mkv\r\n");

        // A directory cannot be replaced by a file: the export reports it and the old file stays.
        let blocked = db.dir.join("blocked.txt");
        std::fs::create_dir(&blocked).unwrap();
        worker.submit(Job::ExportHashes {
            path: blocked.clone(),
            template: "%filename%".into(),
            copy_length_as_minutes: false,
        });
        let event = wait_for(&worker, |e| {
            matches!(e, Event::Synced { .. } | Event::Error(_))
        });
        assert!(
            matches!(&event, Event::Error(m) if m.starts_with("Hash export failed")),
            "{event:?}"
        );
        assert!(blocked.is_dir());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "a.mkv\r\n");
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
        assert_eq!(queue_full_update(&db.path).unwrap().queued, 4);
        assert_eq!(
            queue_full_update(&db.path).unwrap().queued,
            0,
            "queueing is idempotent"
        );
        assert_eq!(db.open().queue_len().unwrap(), 4);
    }

    /// A synthetic databank folder: one plain SQLite collection, which the importer accepts as
    /// an already decrypted copy and imports as zero records.
    fn synthetic_databank(db: &TempDb) -> PathBuf {
        let kowai = db.dir.join("animatic-test-root").join("kowai");
        std::fs::create_dir_all(&kowai).unwrap();
        drop(Db::open(&kowai.join("mylist.data")).unwrap());
        kowai
    }

    fn import_job(db: &TempDb, kowai: &Path, confirmed: bool) -> Job {
        Job::ImportLegacy {
            source: kowai.to_path_buf(),
            temp_root: db.dir.join("temp"),
            confirmed,
        }
    }

    #[test]
    fn full_update_leaves_out_what_the_limits_do_not_allow_yet() {
        use animatic_core::limits::{KNOWN_FILE_MIN_AGE, RequestKind};
        let db = TempDb::new();
        let d = db.open();
        d.conn()
            .execute_batch(
                "INSERT INTO local_hash(id, ed2k, size) VALUES (1,'aa',1),(2,'bb',1);
                 INSERT INTO local_file(id, hash_id, path) VALUES (1,1,'/s/1'),(2,2,'/s/2');
                 INSERT INTO file(fid, aid, ed2k, size) VALUES (10,100,'aa',1),(11,101,'bb',1);",
            )
            .unwrap();
        let now = format::now();
        d.record_titles_dump_attempt(now - 3600).unwrap();
        d.record_request_attempt(RequestKind::Anime, 100, now - 3600)
            .unwrap();
        d.record_request_attempt(RequestKind::File, 10, now - 86_400)
            .unwrap();

        // Recently requested: the title dump, anime 100 and file 10 stay out of the queue.
        let report = queue_full_update(&db.path).unwrap();
        assert_eq!(report.queued, 2, "{report:?}");
        assert_eq!(report.recent_anime, 1);
        assert_eq!(report.recent_files, 1);
        assert_eq!(report.titles_dump_after, Some(now - 3600 + 86_400));
        let text = report.describe();
        assert!(
            text.contains("title dump") && text.contains("1 anime") && text.contains("1 files"),
            "{text}"
        );
        let queued: Vec<Command> = d
            .queue_list()
            .unwrap()
            .into_iter()
            .filter_map(|e| e.command)
            .collect();
        assert_eq!(
            queued,
            [
                Command::FetchFile { fid: 11 },
                Command::FetchAnime { aid: 101 }
            ]
        );

        // Once the intervals have passed, everything is queued again.
        d.queue_clear().unwrap();
        d.record_titles_dump_attempt(now - 86_400 - 1).unwrap();
        d.record_request_attempt(RequestKind::Anime, 100, now - 86_400 - 1)
            .unwrap();
        let week = KNOWN_FILE_MIN_AGE.as_secs() as i64;
        d.record_request_attempt(RequestKind::File, 10, now - week - 1)
            .unwrap();
        let report = queue_full_update(&db.path).unwrap();
        assert_eq!(report.queued, 5, "{report:?}");
        assert_eq!(report.titles_dump_after, None);
        assert!(!report.describe().contains("left out"));
    }

    /// Reply script of a server that accepts the login and answers every MYLIST request with
    /// "no such entry", noting when (on `clock`) each packet arrived.
    fn accepting_server(clock: &Arc<ManualClock>, arrivals: &Arc<Mutex<Vec<i64>>>) -> Fixture {
        let (clock, arrivals) = (clock.clone(), arrivals.clone());
        Fixture::start(move |_, request| {
            arrivals.lock().push(clock.now_ms());
            if request.starts_with("AUTH") {
                "200 session 127.0.0.1:1 LOGIN ACCEPTED".into()
            } else if request.starts_with("MYLIST ") {
                "321 NO SUCH ENTRY".into()
            } else {
                "203 LOGGED OUT".into()
            }
        })
    }

    fn push_refreshes(db: &TempDb, lids: &[i64]) {
        for lid in lids {
            db.open()
                .queue_push(&Command::MylistRefresh { lid: *lid })
                .unwrap();
        }
    }

    #[test]
    fn importing_into_a_populated_database_waits_for_confirmation() {
        let db = TempDb::new();
        db.open()
            .conn()
            .execute_batch(
                "INSERT INTO local_hash(id, ed2k, size) VALUES (1,'aa',1);
                 INSERT INTO local_file(id, hash_id, path) VALUES (1,1,'/s/1');",
            )
            .unwrap();
        let kowai = synthetic_databank(&db);
        let source_before = std::fs::read(kowai.join("mylist.data")).unwrap();
        let worker = Worker::spawn(db.path.clone());

        worker.submit(import_job(&db, &kowai, false));
        let event = wait_for(&worker, |e| {
            matches!(
                e,
                Event::LegacyImportNeedsConfirmation { .. } | Event::Started { .. }
            )
        });
        assert!(
            matches!(&event, Event::LegacyImportNeedsConfirmation { source } if *source == kowai),
            "{event:?}"
        );
        assert!(
            db.open().meta("legacy_import_source").unwrap().is_none(),
            "nothing was imported"
        );
        assert!(
            !db.dir.join("temp").exists(),
            "no decrypted copy was started"
        );

        worker.submit(import_job(&db, &kowai, true));
        let event = wait_for(&worker, |e| {
            matches!(e, Event::Imported { .. } | Event::Error(_))
        });
        assert!(matches!(event, Event::Imported { .. }), "{event:?}");
        assert!(db.open().meta("legacy_import_source").unwrap().is_some());
        assert_eq!(
            std::fs::read(kowai.join("mylist.data")).unwrap(),
            source_before,
            "the databank is only read"
        );
    }

    #[test]
    fn importing_into_an_empty_database_needs_no_confirmation() {
        let db = TempDb::new();
        drop(db.open());
        let kowai = synthetic_databank(&db);
        let worker = Worker::spawn(db.path.clone());

        worker.submit(import_job(&db, &kowai, false));
        let event = wait_for(&worker, |e| {
            matches!(
                e,
                Event::Imported { .. }
                    | Event::LegacyImportNeedsConfirmation { .. }
                    | Event::Error(_)
            )
        });
        assert!(matches!(event, Event::Imported { .. }), "{event:?}");
    }

    #[test]
    fn the_gate_opens_only_for_empty_or_confirmed_targets() {
        let db = TempDb::new();
        assert!(open_for_legacy_import(&db.path, false).unwrap().is_some());
        db.open()
            .conn()
            .execute_batch("INSERT INTO anime(aid) VALUES (1);")
            .unwrap();
        assert!(open_for_legacy_import(&db.path, false).unwrap().is_none());
        assert!(open_for_legacy_import(&db.path, true).unwrap().is_some());
    }

    #[test]
    fn leftover_import_folders_are_reported_and_only_marked_ones_removed() {
        use animatic_core::legacy::workdir::{WORKDIR_MARKER, WORKDIR_PREFIX};
        let db = TempDb::new();
        drop(db.open());
        let temp_root = db.dir.join("temp");
        let marked = temp_root.join(format!("{WORKDIR_PREFIX}synthetic"));
        let unmarked = temp_root.join(format!("{WORKDIR_PREFIX}unmarked"));
        let unrelated = temp_root.join("unrelated.txt");
        std::fs::create_dir_all(&marked).unwrap();
        std::fs::create_dir_all(&unmarked).unwrap();
        std::fs::write(marked.join(WORKDIR_MARKER), b"").unwrap();
        std::fs::write(marked.join("animes.sqlite"), b"synthetic").unwrap();
        std::fs::write(&unrelated, b"keep").unwrap();
        let worker = Worker::spawn(db.path.clone());

        worker.submit(Job::FindLegacyLeftovers {
            temp_root: temp_root.clone(),
        });
        let event = wait_for(&worker, |e| matches!(e, Event::LegacyLeftovers { .. }));
        let Event::LegacyLeftovers { folders, .. } = event else {
            unreachable!()
        };
        assert_eq!(folders, vec![marked.clone()]);

        // Even when asked to remove more, only the marked folder goes.
        worker.submit(Job::RemoveLegacyLeftovers {
            temp_root: temp_root.clone(),
            folders: vec![marked.clone(), unmarked.clone(), unrelated.clone()],
        });
        let event = wait_for(&worker, |e| matches!(e, Event::Synced { .. }));
        assert!(
            matches!(&event, Event::Synced { description, changed: false } if description.contains("Removed 1")),
            "{event:?}"
        );
        assert!(!marked.exists());
        assert!(unmarked.is_dir());
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"keep");

        worker.submit(Job::FindLegacyLeftovers { temp_root });
        worker.submit(Job::QueueFullUpdate);
        let event = wait_for(&worker, |e| {
            matches!(e, Event::LegacyLeftovers { .. } | Event::Finished)
        });
        assert!(
            matches!(event, Event::Finished),
            "no leftovers are reported once removed"
        );
    }

    #[test]
    fn consecutive_runs_and_a_restart_keep_the_long_term_spacing() {
        let clock = Arc::new(ManualClock::default());
        let arrivals = Arc::new(Mutex::new(Vec::new()));
        let fixture = accepting_server(&clock, &arrivals);
        let db = TempDb::new();
        let s = settings(fixture.addr);
        let process = network_for(&db, &clock);

        // Two runs of one process: each builds new clients, but the burst allowance is shared.
        push_refreshes(&db, &[1, 2]);
        run_queue_with(&db, &s, &process);
        push_refreshes(&db, &[3, 4]);
        run_queue_with(&db, &s, &process);
        assert_eq!(fixture.packets(), 8, "login, two requests, logout, twice");
        // A restart: a new process reads the saved send time and cannot know the burst size.
        push_refreshes(&db, &[5]);
        run_queue_with(&db, &s, &network_for(&db, &clock));

        let arrivals = arrivals.lock().clone();
        let gaps: Vec<i64> = arrivals.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(
            gaps,
            [2000, 2000, 2000, 2000, 4000, 4000, 4000, 4000, 4000, 4000]
        );
        let saved = db
            .open()
            .meta(animatic_core::limits::UDP_LAST_SEND_KEY)
            .unwrap();
        assert_eq!(saved, Some(arrivals.last().unwrap().to_string()));
    }

    #[test]
    fn the_send_time_is_saved_before_the_packet_leaves() {
        let clock = Arc::new(ManualClock::default());
        let db = TempDb::new();
        let (path, at) = (db.path.clone(), clock.clone());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        // The server never answers: the process "dies" with the packet in flight.
        let fixture = Fixture::start(move |_, _| {
            let saved = Db::open(&path)
                .unwrap()
                .meta(animatic_core::limits::UDP_LAST_SEND_KEY)
                .unwrap();
            log.lock().push((saved, at.now_ms()));
            String::new()
        });
        push_refreshes(&db, &[1]);
        let network = Network {
            limits: animatic_core::limits::open_limits_with_clock(&db.path, clock.clone()),
            udp_timeout: Duration::from_millis(300),
        };
        run_queue_with(&db, &settings(fixture.addr), &network);
        let seen = seen.lock().clone();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0].0,
            Some(seen[0].1.to_string()),
            "saved when the packet arrived"
        );
    }

    #[test]
    fn a_busy_local_port_stops_the_run_with_an_error_naming_the_setting() {
        let held = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let fixture = Fixture::start(|_, _| "200 session 127.0.0.1:1 LOGIN ACCEPTED".into());
        let db = TempDb::new();
        queue_two(&db.open());
        let mut s = settings(fixture.addr);
        s.local_port = port;
        let events = run_queue(&db, &s);
        assert!(has_error(&events, "anidb.local_port"), "{events:?}");
        assert_eq!(fixture.packets(), 0, "no packet came from another port");
        let d = db.open();
        assert_eq!(
            d.queue_blocked_len().unwrap(),
            0,
            "the commands wait for a free port"
        );
        assert!(d.sync_cooldown().unwrap().is_some());
        assert!(
            d.meta(animatic_core::limits::UDP_LAST_SEND_KEY)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_banned_client_version_blocks_the_queue_until_it_is_corrected() {
        let fixture = Fixture::start(|_, _| "504 CLIENT VERSION BANNED".into());
        let db = TempDb::new();
        queue_two(&db.open());
        let s = settings(fixture.addr);
        let events = run_queue(&db, &s);
        assert!(has_error(&events, "banned"), "{events:?}");
        assert!(events.iter().any(
            |e| matches!(e, Event::Blocked { description } if description.contains("banned"))
        ));
        let d = db.open();
        assert_eq!(
            d.queue_blocked_len().unwrap(),
            2,
            "every runnable row is blocked"
        );
        assert!(
            d.sync_cooldown().unwrap().is_none(),
            "no timed ban: only a correction helps"
        );
        assert_eq!(fixture.packets(), 1);
        run_queue(&db, &s);
        assert_eq!(fixture.packets(), 1, "nothing is eligible, nothing is sent");
    }

    #[test]
    fn an_unanswered_login_backs_off_in_growing_steps_that_a_login_resets() {
        let clock = Arc::new(ManualClock::default());
        let answer = Arc::new(AtomicBool::new(false));
        let on = answer.clone();
        let fixture = Fixture::start(move |_, request| {
            if !on.load(Ordering::SeqCst) {
                String::new()
            } else if request.starts_with("AUTH") {
                "200 session 127.0.0.1:1 LOGIN ACCEPTED".into()
            } else if request.starts_with("MYLIST ") {
                "321 NO SUCH ENTRY".into()
            } else {
                "203 LOGGED OUT".into()
            }
        });
        let db = TempDb::new();
        let s = settings(fixture.addr);
        push_refreshes(&db, &[1]);
        // One run on a new limiter each time (a restart), and the wait is "over" afterwards.
        let run = |expect_wait: Option<i64>| {
            let network = Network {
                limits: animatic_core::limits::open_limits_with_clock(&db.path, clock.clone()),
                udp_timeout: Duration::from_millis(300),
            };
            let started = format::now();
            let events = run_queue_with(&db, &s, &network);
            let d = db.open();
            let cooldown = d.sync_cooldown().unwrap();
            d.clear_sync_cooldown().unwrap();
            match expect_wait {
                Some(secs) => {
                    let (until, reason) = cooldown.expect("a cooldown");
                    assert!(
                        (started + secs..=started + secs + 3).contains(&until),
                        "wait {secs}s, got {}s",
                        until - started
                    );
                    assert!(reason.contains("no reply to the AniDB login"), "{reason}");
                    assert!(has_error(&events, "AUTH"));
                    assert_eq!(d.queue_blocked_len().unwrap(), 0, "the command waits");
                    assert_eq!(
                        d.queue_list().unwrap()[0].tries,
                        0,
                        "not the command's fault"
                    );
                }
                None => assert!(cooldown.is_none()),
            }
        };
        for secs in [30, 120, 300, 600, 1800, 7200, 7200] {
            run(Some(secs));
        }
        assert_eq!(
            fixture.packets(),
            7,
            "one AUTH per run, no retry inside a run"
        );
        assert_eq!(
            db.open()
                .meta(animatic_core::limits::AUTH_BACKOFF_STEP_KEY)
                .unwrap()
                .as_deref(),
            Some("6"),
            "the step stays at the last value"
        );

        answer.store(true, Ordering::SeqCst);
        run(None);
        assert_eq!(
            db.open().queue_len().unwrap(),
            0,
            "the login worked and the command ran"
        );
        assert_eq!(
            db.open()
                .meta(animatic_core::limits::AUTH_BACKOFF_STEP_KEY)
                .unwrap()
                .as_deref(),
            Some("0"),
            "a successful AUTH resets the backoff"
        );

        answer.store(false, Ordering::SeqCst);
        push_refreshes(&db, &[2]);
        run(Some(30));
    }

    #[test]
    fn a_known_file_is_requested_at_most_every_seven_days_by_the_queue() {
        let fixture = Fixture::start(|_, request| {
            if request.starts_with("AUTH") {
                "200 session 127.0.0.1:1 LOGIN ACCEPTED".into()
            } else if request.starts_with("FILE ") {
                "320 NO SUCH FILE".into()
            } else {
                "203 LOGGED OUT".into()
            }
        });
        let db = TempDb::new();
        let s = settings(fixture.addr);
        db.open()
            .queue_push(&Command::FetchFile { fid: 5 })
            .unwrap();
        run_queue(&db, &s);
        let sent = fixture.packets();
        assert_eq!(sent, 3, "login, FILE, logout");

        // Queued again inside the week: completed as skipped, with no packet and no failure.
        db.open()
            .queue_push(&Command::FetchFile { fid: 5 })
            .unwrap();
        let events = run_queue(&db, &s);
        assert_eq!(
            fixture.packets(),
            sent,
            "nothing was sent, not even a login"
        );
        assert!(events.iter().any(
            |e| matches!(e, Event::Synced { description, changed: false } if description.contains("File 5 skipped") && description.contains("allowed after"))
        ), "{events:?}");
        assert!(!events.iter().any(|e| matches!(
            e,
            Event::Error(_) | Event::Blocked { .. } | Event::Cooldown { .. }
        )));
        let d = db.open();
        assert_eq!(d.queue_len().unwrap(), 0, "the skipped command is removed");
        assert!(d.sync_cooldown().unwrap().is_none());
    }

    #[test]
    fn a_queued_title_dump_inside_the_window_is_completed_as_skipped() {
        let fixture = Fixture::start(|_, _| "555 BANNED".into());
        let db = TempDb::new();
        let d = db.open();
        d.record_titles_dump_attempt(format::now() - 60).unwrap();
        d.queue_push(&Command::TitlesDump).unwrap();
        let events = run_queue(&db, &settings(fixture.addr));
        assert!(events.iter().any(
            |e| matches!(e, Event::Synced { description, .. } if description.contains("Title dump skipped") && description.contains("allowed after"))
        ), "{events:?}");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::Error(_) | Event::Blocked { .. }))
        );
        assert_eq!(d.queue_len().unwrap(), 0);
        assert_eq!(fixture.packets(), 0);
    }
}
