//! The persistent hash queue (roadmap M5.4).
//!
//! Files waiting to be hashed live in the `hash_queue` table, so they survive a quit, a crash
//! and a restart. **Add files**, command-line paths, files forwarded by another launch and folder
//! scans all [`Db::hash_queue_enqueue`]; the worker then [`drain`]s the queue in order with the
//! ordinary single-pass hasher ([`hash_and_register`]: one read per file).
//!
//! An entry is removed only after its result is stored in the library. A file that cannot be
//! hashed (missing, unreadable) is not retried on its own: it stays in the table as `failed`
//! with a visible reason until the user re-adds it, retries failures or clears the queue.
//! Cancelling a drain leaves every remaining entry (and the file being hashed) queued.
//!
//! Entries are unique by canonical path, so the same file queued twice, through a different
//! spelling of its path or through a link, is hashed once. The path that is hashed and recorded
//! in the library is the absolute path as given, not the resolved one, so a library that
//! records files below a linked folder keeps recording them there.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use rusqlite::{OptionalExtension, params};

use crate::db::Db;
use crate::error::{Error, Result};
use crate::file_manager::{hash_and_register, interruptible_sleep};
use crate::format;
use crate::hash::{Ed2kVariant, Hashes};

/// How many entries [`HashQueueView`] lists; the counts are always exact.
pub const VIEW_LIMIT: usize = 500;

/// Where a queued path came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashSource {
    /// **Add files** (or the dashboard button).
    AddFiles,
    /// A path on the command line of this launch.
    CommandLine,
    /// A scan of the managed folders.
    Scan,
    /// A path forwarded by another launch (single-instance handoff, file-menu entry).
    Handoff,
}

impl HashSource {
    /// The value stored in the `source` column.
    pub fn as_str(self) -> &'static str {
        match self {
            HashSource::AddFiles => "add-files",
            HashSource::CommandLine => "command-line",
            HashSource::Scan => "scan",
            HashSource::Handoff => "handoff",
        }
    }

    fn parse(text: &str) -> HashSource {
        match text {
            "command-line" => HashSource::CommandLine,
            "scan" => HashSource::Scan,
            "handoff" => HashSource::Handoff,
            _ => HashSource::AddFiles,
        }
    }

    /// Short name for the queue list.
    pub fn label(self) -> &'static str {
        match self {
            HashSource::AddFiles => "Add files",
            HashSource::CommandLine => "Command line",
            HashSource::Scan => "Scan",
            HashSource::Handoff => "Other launch",
        }
    }
}

/// Whether an entry is waiting or has been given up on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashState {
    Queued,
    Failed,
}

/// One row of the hash queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashQueueEntry {
    pub id: i64,
    /// Absolute path that is hashed and recorded.
    pub path: PathBuf,
    /// Unix time the file was queued.
    pub added: i64,
    pub source: HashSource,
    pub state: HashState,
    /// Why a failed entry failed.
    pub reason: Option<String>,
}

/// What the interface shows of the queue: exact counts and the first [`VIEW_LIMIT`] entries
/// (failed ones first, then waiting ones in hashing order).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HashQueueView {
    /// Entries still waiting to be hashed.
    pub pending: usize,
    /// Entries that failed and wait for a retry or a clear.
    pub failed: usize,
    pub entries: Vec<HashQueueEntry>,
}

/// Outcome of [`Db::hash_queue_enqueue`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnqueueReport {
    /// Paths that are now waiting: new entries and failed entries queued again.
    pub added: usize,
    /// Paths that were already waiting, or repeated within the request.
    pub duplicates: usize,
}

/// The absolute path to hash and the key that decides whether two paths are the same file.
fn normalize(path: &Path) -> (PathBuf, String) {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let resolved = std::fs::canonicalize(&absolute).unwrap_or_else(|_| absolute.clone());
    (absolute, strip_verbatim(&resolved.to_string_lossy()))
}

/// Windows resolves paths to the verbatim form (`\\?\C:\x`, `\\?\UNC\host\share\x`), which
/// would not equal the ordinary spelling of the same file.
fn strip_verbatim(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

fn entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HashQueueEntry> {
    let source: String = row.get(3)?;
    let state: String = row.get(4)?;
    Ok(HashQueueEntry {
        id: row.get(0)?,
        path: PathBuf::from(row.get::<_, String>(1)?),
        added: row.get(2)?,
        source: HashSource::parse(&source),
        state: if state == "failed" {
            HashState::Failed
        } else {
            HashState::Queued
        },
        reason: row.get(5)?,
    })
}

const ENTRY_COLUMNS: &str = "id, path, added, source, state, reason";

impl Db {
    /// Queue `paths` for hashing, in order, skipping files that already wait. A failed entry for
    /// the same file is queued again. Atomic: all paths are queued or none.
    pub fn hash_queue_enqueue(
        &self,
        paths: &[PathBuf],
        source: HashSource,
    ) -> Result<EnqueueReport> {
        let normalized: Vec<(PathBuf, String)> = paths.iter().map(|p| normalize(p)).collect();
        self.write_tx(|| {
            let mut insert = self.conn().prepare_cached(
                "INSERT INTO hash_queue(path, key, added, source) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(key) DO UPDATE SET state = 'queued', reason = NULL, path = excluded.path
                 WHERE hash_queue.state = 'failed'",
            )?;
            let now = format::now();
            let mut report = EnqueueReport::default();
            for (path, key) in &normalized {
                let changed = insert.execute(params![
                    path.to_string_lossy(),
                    key,
                    now,
                    source.as_str()
                ])?;
                if changed > 0 {
                    report.added += 1;
                } else {
                    report.duplicates += 1;
                }
            }
            Ok(report)
        })
    }

    /// Entries waiting to be hashed (failed ones excluded).
    pub fn hash_queue_pending(&self) -> Result<usize> {
        Ok(self.conn().query_row(
            "SELECT count(*) FROM hash_queue WHERE state = 'queued'",
            [],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }

    /// The entry to hash next: the oldest waiting one.
    pub fn hash_queue_next(&self) -> Result<Option<HashQueueEntry>> {
        Ok(self
            .conn()
            .query_row(
                &format!(
                    "SELECT {ENTRY_COLUMNS} FROM hash_queue WHERE state = 'queued' ORDER BY id LIMIT 1"
                ),
                [],
                entry_from_row,
            )
            .optional()?)
    }

    /// Drop an entry whose result is stored.
    pub fn hash_queue_remove(&self, id: i64) -> Result<()> {
        self.conn()
            .execute("DELETE FROM hash_queue WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Give up on an entry and keep the reason where the user can see it.
    pub fn hash_queue_fail(&self, id: i64, reason: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE hash_queue SET state = 'failed', reason = ?2 WHERE id = ?1",
            params![id, reason],
        )?;
        Ok(())
    }

    /// Queue every failed entry again; returns how many.
    pub fn hash_queue_retry_failed(&self) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE hash_queue SET state = 'queued', reason = NULL WHERE state = 'failed'",
            [],
        )?)
    }

    /// Remove every entry, waiting and failed; returns how many. Hashed files are untouched.
    pub fn hash_queue_clear(&self) -> Result<usize> {
        Ok(self.conn().execute("DELETE FROM hash_queue", [])?)
    }

    /// Counts and the first [`VIEW_LIMIT`] entries. The two reads are consistent with each other
    /// inside the read transaction of `LibraryProjection::load`; used alone they may differ by
    /// an entry that changed in between.
    pub fn hash_queue_view(&self) -> Result<HashQueueView> {
        let (pending, failed) = self.conn().query_row(
            "SELECT coalesce(sum(state = 'queued'), 0), coalesce(sum(state = 'failed'), 0)
             FROM hash_queue",
            [],
            |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize)),
        )?;
        let entries = self
            .conn()
            .prepare(&format!(
                "SELECT {ENTRY_COLUMNS} FROM hash_queue
                 ORDER BY (state = 'failed') DESC, id LIMIT {VIEW_LIMIT}"
            ))?
            .query_map([], entry_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(HashQueueView {
            pending,
            failed,
            entries,
        })
    }
}

/// How [`drain`] runs.
#[derive(Debug, Clone, Copy, Default)]
pub struct DrainOptions {
    /// Pause between two hashed files (`file_managers.sleep_time_ms`).
    pub pause_between: Duration,
}

/// What a [`drain`] reports while it runs.
#[derive(Debug)]
pub enum DrainStep<'a> {
    /// Bytes read of the file being hashed; `index` counts from 1 within this drain.
    Progress {
        path: &'a Path,
        index: usize,
        total: usize,
        done: u64,
        size: u64,
    },
    /// The file is hashed, stored in the library and removed from the queue.
    Hashed {
        path: &'a Path,
        local_id: i64,
        hashes: &'a Hashes,
    },
    /// The file could not be hashed; it stays in the queue as failed with this reason.
    Failed { path: &'a Path, reason: &'a str },
}

/// How a [`drain`] ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrainReport {
    pub hashed: usize,
    pub failed: usize,
    /// Stopped by the cancel flag; the rest of the queue is still queued.
    pub cancelled: bool,
}

/// Hash the waiting entries in order until none is left or `cancel` is set. Every file is read
/// once ([`hash_and_register`]). The entry leaves the queue only after the library holds its
/// result, so a quit or crash in between hashes the file again instead of losing it; a database
/// error ends the drain with the entry still queued.
pub fn drain(
    db: &Db,
    options: DrainOptions,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    on: &mut dyn FnMut(DrainStep<'_>),
) -> Result<DrainReport> {
    use std::sync::atomic::Ordering;

    let total = db.hash_queue_pending()?;
    let mut report = DrainReport::default();
    loop {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        let Some(entry) = db.hash_queue_next()? else {
            break;
        };
        let handled = report.hashed + report.failed;
        if handled > 0 && !interruptible_sleep(options.pause_between, cancel) {
            report.cancelled = true;
            break;
        }
        let index = handled + 1;
        let total = total.max(index);
        let result = hash_and_register(
            db,
            &entry.path,
            Ed2kVariant::Red,
            cancel,
            pause,
            &mut |done, size| {
                on(DrainStep::Progress {
                    path: &entry.path,
                    index,
                    total,
                    done,
                    size,
                })
            },
        );
        match result {
            Ok((local_id, hashes)) => {
                db.hash_queue_remove(entry.id)?;
                report.hashed += 1;
                on(DrainStep::Hashed {
                    path: &entry.path,
                    local_id,
                    hashes: &hashes,
                });
            }
            Err(Error::Cancelled) => {
                report.cancelled = true;
                break;
            }
            Err(e @ Error::Db(_)) => return Err(e),
            Err(e) => {
                let reason = failure_reason(&e);
                db.hash_queue_fail(entry.id, &reason)?;
                report.failed += 1;
                on(DrainStep::Failed {
                    path: &entry.path,
                    reason: &reason,
                });
            }
        }
    }
    Ok(report)
}

fn failure_reason(error: &Error) -> String {
    match error {
        Error::Io(e) if e.kind() == std::io::ErrorKind::NotFound => {
            "file not found (moved, renamed or deleted, or its drive is unavailable)".into()
        }
        Error::Io(e) => format!("the file could not be read: {e}"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn synthetic(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn drain_all(db: &Db) -> (DrainReport, Vec<PathBuf>) {
        let cancel = AtomicBool::new(false);
        let pause = AtomicBool::new(false);
        let mut hashed = Vec::new();
        let report = drain(db, DrainOptions::default(), &cancel, &pause, &mut |step| {
            if let DrainStep::Hashed { path, .. } = step {
                hashed.push(path.to_path_buf());
            }
        })
        .unwrap();
        (report, hashed)
    }

    #[test]
    fn queued_files_survive_a_restart_and_are_hashed_once() {
        let dir = temp_dir();
        let db_path = dir.path().join("library.sqlite3");
        let a = synthetic(dir.path(), "a.mkv", b"first synthetic file");
        let b = synthetic(dir.path(), "b.mkv", b"second synthetic file");
        let c = synthetic(dir.path(), "c.mkv", b"third synthetic file");

        {
            let db = Db::open(&db_path).unwrap();
            let report = db
                .hash_queue_enqueue(&[a.clone(), b.clone(), c.clone()], HashSource::AddFiles)
                .unwrap();
            assert_eq!(report.added, 3);
            // Hash only the first, then stop as a cancel would.
            let cancel = AtomicBool::new(false);
            let pause = AtomicBool::new(false);
            let report = drain(&db, DrainOptions::default(), &cancel, &pause, &mut |step| {
                if matches!(step, DrainStep::Hashed { .. }) {
                    cancel.store(true, Ordering::Relaxed);
                }
            })
            .unwrap();
            assert_eq!((report.hashed, report.cancelled), (1, true));
        }

        // A new connection, as after a restart: the same files are queued again by a handoff and
        // by a scan, and the queue holds each of them once.
        let db = Db::open(&db_path).unwrap();
        let view = db.hash_queue_view().unwrap();
        assert_eq!(view.pending, 2);
        let again = db
            .hash_queue_enqueue(&[b.clone(), c.clone()], HashSource::Handoff)
            .unwrap();
        assert_eq!((again.added, again.duplicates), (0, 2));
        let (report, hashed) = drain_all(&db);
        assert_eq!(report.hashed, 2);
        assert_eq!(hashed, vec![b, c]);
        assert_eq!(db.hash_queue_view().unwrap(), HashQueueView::default());
        assert_eq!(db.local_files().unwrap().len(), 3, "one record per file");
    }

    #[test]
    fn the_same_file_under_another_spelling_is_queued_once() {
        let dir = temp_dir();
        let db = Db::memory().unwrap();
        let file = synthetic(dir.path(), "episode.mkv", b"synthetic");
        let roundabout = dir.path().join("sub").join("..").join("episode.mkv");
        std::fs::create_dir(dir.path().join("sub")).unwrap();

        let report = db
            .hash_queue_enqueue(&[file.clone(), roundabout, file.clone()], HashSource::Scan)
            .unwrap();

        assert_eq!((report.added, report.duplicates), (1, 2));
        assert_eq!(db.hash_queue_pending().unwrap(), 1);
    }

    #[test]
    fn a_missing_file_fails_with_a_reason_and_the_rest_still_hash() {
        let dir = temp_dir();
        let db = Db::memory().unwrap();
        let gone = dir.path().join("gone.mkv");
        let present = synthetic(dir.path(), "present.mkv", b"synthetic");
        db.hash_queue_enqueue(&[gone.clone(), present.clone()], HashSource::AddFiles)
            .unwrap();

        let (report, hashed) = drain_all(&db);

        assert_eq!((report.hashed, report.failed), (1, 1));
        assert_eq!(hashed, vec![present]);
        let view = db.hash_queue_view().unwrap();
        assert_eq!((view.pending, view.failed), (0, 1));
        let entry = &view.entries[0];
        assert_eq!(entry.path, std::path::absolute(&gone).unwrap());
        assert_eq!(entry.state, HashState::Failed);
        assert!(entry.reason.as_deref().unwrap().contains("not found"));

        // Draining again leaves it alone; re-adding the file queues it once more.
        let (again, _) = drain_all(&db);
        assert_eq!((again.hashed, again.failed), (0, 0));
        std::fs::write(&gone, b"restored").unwrap();
        let report = db
            .hash_queue_enqueue(std::slice::from_ref(&gone), HashSource::AddFiles)
            .unwrap();
        assert_eq!(report.added, 1);
        let (report, _) = drain_all(&db);
        assert_eq!(report.hashed, 1);
        assert_eq!(db.hash_queue_view().unwrap(), HashQueueView::default());
    }

    #[test]
    fn retrying_failures_queues_them_again_and_clear_removes_everything() {
        let dir = temp_dir();
        let db = Db::memory().unwrap();
        let gone = dir.path().join("gone.mkv");
        let waiting = synthetic(dir.path(), "waiting.mkv", b"synthetic");
        db.hash_queue_enqueue(std::slice::from_ref(&gone), HashSource::Scan)
            .unwrap();
        drain_all(&db);
        db.hash_queue_enqueue(&[waiting], HashSource::Scan).unwrap();

        assert_eq!(db.hash_queue_retry_failed().unwrap(), 1);
        let view = db.hash_queue_view().unwrap();
        assert_eq!((view.pending, view.failed), (2, 0));
        assert_eq!(db.hash_queue_clear().unwrap(), 2);
        assert_eq!(db.hash_queue_view().unwrap(), HashQueueView::default());
    }

    #[test]
    fn cancel_leaves_the_file_being_hashed_queued() {
        let dir = temp_dir();
        let db = Db::memory().unwrap();
        let big = synthetic(dir.path(), "big.mkv", &vec![7u8; 5 << 20]);
        let next = synthetic(dir.path(), "next.mkv", b"synthetic");
        db.hash_queue_enqueue(&[big, next], HashSource::AddFiles)
            .unwrap();
        let cancel = AtomicBool::new(false);
        let pause = AtomicBool::new(false);

        let report = drain(&db, DrainOptions::default(), &cancel, &pause, &mut |step| {
            if matches!(step, DrainStep::Progress { .. }) {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();

        assert!(report.cancelled);
        assert_eq!(report.hashed, 0);
        assert_eq!(db.hash_queue_pending().unwrap(), 2);
        assert!(db.local_files().unwrap().is_empty());
    }

    #[test]
    fn the_library_projection_shows_the_queue_and_its_failures() {
        let dir = temp_dir();
        let db = Db::memory().unwrap();
        let gone = dir.path().join("gone.mkv");
        let waiting = synthetic(dir.path(), "waiting.mkv", b"synthetic");
        db.hash_queue_enqueue(std::slice::from_ref(&gone), HashSource::Scan)
            .unwrap();
        drain_all(&db);
        db.hash_queue_enqueue(std::slice::from_ref(&waiting), HashSource::CommandLine)
            .unwrap();

        let projection = crate::library::LibraryProjection::load(&db).unwrap();

        let view = &projection.hash_queue;
        assert_eq!((view.pending, view.failed), (1, 1));
        assert_eq!(
            view.entries[0].state,
            HashState::Failed,
            "failed files first"
        );
        assert_eq!(view.entries[1].source, HashSource::CommandLine);
    }

    #[test]
    fn strip_verbatim_restores_the_ordinary_spelling() {
        assert_eq!(strip_verbatim(r"\\?\C:\media\a.mkv"), r"C:\media\a.mkv");
        assert_eq!(
            strip_verbatim(r"\\?\UNC\host\share\a.mkv"),
            r"\\host\share\a.mkv"
        );
        assert_eq!(strip_verbatim("/media/a.mkv"), "/media/a.mkv");
    }
}
