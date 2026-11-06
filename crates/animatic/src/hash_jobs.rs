//! Worker side of the persistent hash queue (roadmap M5.4).
//!
//! Every way of hashing files (**Add files**, command-line paths, files forwarded by another
//! launch, scans, **Resume hashing** and the autostart request of the scheduler thread) ends in
//! [`drain`]: the files are queued in the `hash_queue` table first, then drained in order by the
//! single-pass hasher. A cancel stops the drain and leaves the rest queued.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use animatic_core::db::Db;
use animatic_core::hash_queue::{DrainOptions, DrainStep, HashSource};
use crossbeam_channel::Sender;

use crate::worker::Event;

fn file_label(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Hash the waiting entries in order, reporting progress, results and failures as events. The
/// caller owns `Started` and `Finished`.
pub(crate) fn drain(
    db: &Db,
    pause_between: Duration,
    events: &Sender<Event>,
    cancel: &AtomicBool,
    pause: &AtomicBool,
) {
    let outcome = animatic_core::hash_queue::drain(
        db,
        DrainOptions { pause_between },
        cancel,
        pause,
        &mut |step| match step {
            DrainStep::Progress {
                path,
                index,
                total,
                done,
                size,
            } => {
                let _ = events.send(Event::Progress {
                    label: file_label(path),
                    current: index,
                    total,
                    fraction: done as f32 / size.max(1) as f32,
                });
            }
            DrainStep::Hashed {
                path,
                local_id,
                hashes,
            } => {
                let _ = events.send(Event::Hashed {
                    path: path.to_path_buf(),
                    local_id,
                    size: hashes.size,
                    ed2k: hashes.ed2k.clone(),
                });
            }
            DrainStep::Failed { path, reason } => {
                let _ = events.send(Event::Error(format!(
                    "{}: {reason}. It stays in the hash queue until it is retried or cleared.",
                    path.display()
                )));
            }
        },
    );
    match outcome {
        Ok(report) if report.cancelled => {
            let left = db.hash_queue_pending().unwrap_or(0);
            if left > 0 {
                let _ = events.send(Event::Info(format!(
                    "Hashing stopped; {left} file(s) stay in the hash queue"
                )));
            }
        }
        Ok(_) => {}
        Err(e) => {
            let _ = events.send(Event::Error(format!("Hashing stopped: {e}")));
        }
    }
    let _ = events.send(Event::HashQueueChanged);
}

/// Drain whatever is queued, announcing the run with `Started`/`Finished`. Nothing at all is
/// reported when the queue is empty, so an autostart request on an empty queue is silent.
pub(crate) fn run_queue(
    db: &Db,
    pause_between: Duration,
    events: &Sender<Event>,
    cancel: &AtomicBool,
    pause: &AtomicBool,
) {
    let total = match db.hash_queue_pending() {
        Ok(0) => return,
        Ok(total) => total,
        Err(e) => {
            let _ = events.send(Event::Error(format!("Hash queue unreadable: {e}")));
            return;
        }
    };
    let _ = events.send(Event::Started {
        label: "Hashing files".into(),
        total,
    });
    drain(db, pause_between, events, cancel, pause);
    let _ = events.send(Event::Finished);
}

/// Queue `paths` and drain the queue.
pub(crate) fn run_hash(
    db_path: &Path,
    paths: Vec<PathBuf>,
    source: HashSource,
    pause_between: Duration,
    events: &Sender<Event>,
    cancel: &AtomicBool,
    pause: &AtomicBool,
) {
    let db = match Db::open(db_path) {
        Ok(db) => db,
        Err(e) => {
            let _ = events.send(Event::Error(e.to_string()));
            return;
        }
    };
    if !paths.is_empty() {
        match enqueue(&db, &paths, source, events) {
            Ok(()) => {}
            Err(e) => {
                let _ = events.send(Event::Error(format!(
                    "Could not queue files for hashing: {e}"
                )));
                return;
            }
        }
    }
    run_queue(&db, pause_between, events, cancel, pause);
}

/// Queue `paths` and tell the log how many were new.
pub(crate) fn enqueue(
    db: &Db,
    paths: &[PathBuf],
    source: HashSource,
    events: &Sender<Event>,
) -> animatic_core::Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let report = db.hash_queue_enqueue(paths, source)?;
    let skipped = if report.duplicates > 0 {
        format!(" ({} already queued)", report.duplicates)
    } else {
        String::new()
    };
    let _ = events.send(Event::Info(format!(
        "Queued {} file(s) for hashing{skipped}",
        report.added
    )));
    let _ = events.send(Event::HashQueueChanged);
    Ok(())
}

/// **Resume hashing**: drain the queue, queueing failed entries again first when asked.
pub(crate) fn run_resume(
    db_path: &Path,
    retry_failed: bool,
    pause_between: Duration,
    events: &Sender<Event>,
    cancel: &AtomicBool,
    pause: &AtomicBool,
) {
    let db = match Db::open(db_path) {
        Ok(db) => db,
        Err(e) => {
            let _ = events.send(Event::Error(e.to_string()));
            return;
        }
    };
    if retry_failed {
        match db.hash_queue_retry_failed() {
            Ok(0) => {}
            Ok(n) => {
                let _ = events.send(Event::Info(format!("Queued {n} failed file(s) again")));
            }
            Err(e) => {
                let _ = events.send(Event::Error(format!("Could not retry failed files: {e}")));
                return;
            }
        }
    }
    run_queue(&db, pause_between, events, cancel, pause);
}

/// **Clear hash queue**: remove every entry. Hashed files and the library are untouched.
pub(crate) fn run_clear(db_path: &Path, events: &Sender<Event>) {
    let _ = events.send(Event::Started {
        label: "Clearing hash queue".into(),
        total: 0,
    });
    match Db::open(db_path).and_then(|db| db.hash_queue_clear()) {
        Ok(n) => {
            let _ = events.send(Event::Info(format!(
                "Removed {n} file(s) from the hash queue"
            )));
            let _ = events.send(Event::HashQueueChanged);
        }
        Err(e) => {
            let _ = events.send(Event::Error(format!("Could not clear the hash queue: {e}")));
        }
    }
    let _ = events.send(Event::Finished);
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    use super::*;
    use crate::worker::{Event, Job, Worker};

    struct Fixture {
        dir: tempfile::TempDir,
        db_path: PathBuf,
    }

    impl Fixture {
        fn new() -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            let db_path = dir.path().join("library.sqlite3");
            drop(Db::open(&db_path).unwrap());
            Fixture { dir, db_path }
        }

        fn db(&self) -> Db {
            Db::open(&self.db_path).unwrap()
        }

        fn media(&self, names: &[&str]) -> Vec<PathBuf> {
            names
                .iter()
                .map(|name| {
                    let path = self.dir.path().join(name);
                    std::fs::write(&path, name.as_bytes()).unwrap();
                    path
                })
                .collect()
        }
    }

    fn wait_for(worker: &Worker, mut accept: impl FnMut(&Event) -> bool) {
        let end = Instant::now() + Duration::from_secs(30);
        while Instant::now() < end {
            if let Ok(event) = worker.events.recv_timeout(Duration::from_millis(100))
                && accept(&event)
            {
                return;
            }
        }
        panic!("the worker did not report the expected event");
    }

    fn hashed_until_finished(worker: &Worker) -> Vec<PathBuf> {
        let mut hashed = Vec::new();
        wait_for(worker, |e| {
            if let Event::Hashed { path, .. } = e {
                hashed.push(path.clone());
            }
            matches!(e, Event::Finished)
        });
        hashed
    }

    fn add_files(paths: Vec<PathBuf>) -> Job {
        Job::Hash {
            paths,
            source: HashSource::AddFiles,
            pause_between: Duration::ZERO,
        }
    }

    #[test]
    fn cancel_after_the_first_file_leaves_the_rest_queued_and_a_restart_hashes_them() {
        let fx = Fixture::new();
        let files = fx.media(&["a.mkv", "b.mkv", "c.mkv"]);
        {
            // A first session: queue all three, cancel as soon as the first is hashed.
            let db = fx.db();
            db.hash_queue_enqueue(&files, HashSource::AddFiles).unwrap();
            let cancel = AtomicBool::new(false);
            let pause = AtomicBool::new(false);
            let report = animatic_core::hash_queue::drain(
                &db,
                DrainOptions::default(),
                &cancel,
                &pause,
                &mut |step| {
                    if matches!(step, DrainStep::Hashed { .. }) {
                        cancel.store(true, Ordering::Relaxed);
                    }
                },
            )
            .unwrap();
            assert!(report.cancelled);
        }
        assert_eq!(fx.db().hash_queue_pending().unwrap(), 2);
        assert_eq!(fx.db().local_files().unwrap().len(), 1);

        // Restart: a new worker on the same database drains the queue on request.
        let worker = Worker::spawn(fx.db_path.clone());
        worker.submit(Job::ResumeHashQueue {
            retry_failed: false,
            pause_between: Duration::ZERO,
        });
        let hashed = hashed_until_finished(&worker);

        assert_eq!(hashed, vec![files[1].clone(), files[2].clone()]);
        assert_eq!(fx.db().hash_queue_pending().unwrap(), 0);
        assert_eq!(fx.db().local_files().unwrap().len(), 3, "no duplicates");
    }

    #[test]
    fn resuming_an_empty_queue_reports_nothing() {
        let fx = Fixture::new();
        let worker = Worker::spawn(fx.db_path.clone());
        worker.submit(Job::ResumeHashQueue {
            retry_failed: false,
            pause_between: Duration::ZERO,
        });
        worker.submit(Job::CheckLibrary);

        // The first report of the run is the library check, not a hashing run.
        wait_for(&worker, |e| {
            assert!(
                !matches!(e, Event::Started { label, .. } if label == "Hashing files"),
                "an empty queue starts no run"
            );
            matches!(e, Event::Finished)
        });
    }

    #[test]
    fn a_missing_file_is_reported_and_kept_as_failed_until_cleared() {
        let fx = Fixture::new();
        let mut files = fx.media(&["present.mkv"]);
        files.insert(0, fx.dir.path().join("gone.mkv"));
        let worker = Worker::spawn(fx.db_path.clone());

        worker.submit(add_files(files));
        let mut errors = Vec::new();
        let mut hashed = 0;
        wait_for(&worker, |e| {
            match e {
                Event::Error(text) => errors.push(text.clone()),
                Event::Hashed { .. } => hashed += 1,
                _ => {}
            }
            matches!(e, Event::Finished)
        });

        assert_eq!(hashed, 1);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("gone.mkv") && errors[0].contains("not found"));
        let view = fx.db().hash_queue_view().unwrap();
        assert_eq!((view.pending, view.failed), (0, 1));

        worker.submit(Job::ClearHashQueue);
        wait_for(&worker, |e| matches!(e, Event::Finished));
        assert_eq!(
            fx.db().hash_queue_view().unwrap(),
            animatic_core::hash_queue::HashQueueView::default()
        );
    }
}
