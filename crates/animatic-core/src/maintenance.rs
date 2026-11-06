//! Library safety: online backup, validated restore, integrity check and compaction.
//!
//! Everything here is blocking file and database work. The application runs it on a worker or
//! a dedicated thread, never on the UI thread.
//!
//! * [`backup_to`] copies the live database with SQLite's online backup API into a temporary
//!   file in the target folder and renames it into place, so a backup is either complete or
//!   absent and the live database stays usable (the queue may keep changing meanwhile).
//! * [`validate_backup`] decides whether a file may replace the library; [`restore_database`]
//!   re-validates it, keeps a timestamped copy of the current database and swaps the files.
//!   Callers must have closed every connection to the database first.
//! * [`check_file`] reports damage without ever repairing it; [`compact`] runs `VACUUM`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::backup::Backup;
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::db::{Db, SCHEMA_VERSION};
use crate::error::{Error, Result};

/// Tables every Animatic library has had since schema version 1. A restore source must hold
/// all of them; tables added later are created when the restored database is opened.
const REQUIRED_TABLES: [&str; 6] = [
    "meta",
    "anime",
    "mylist",
    "local_file",
    "local_hash",
    "command_queue",
];
/// Problems and violations kept per check; the rest is only counted.
const REPORT_LIMIT: usize = 100;
/// Pages the backup API copies per step before it yields to other connections.
const BACKUP_PAGES_PER_STEP: i32 = 256;
const BACKUP_PAUSE: Duration = Duration::from_millis(2);
const CHECK_BUSY_TIMEOUT: Duration = Duration::from_secs(30);

/// Result of [`check_file`]. Nothing is ever repaired.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntegrityReport {
    /// What `PRAGMA integrity_check` reported (not `ok`), or why the file could not be read.
    pub problems: Vec<String>,
    /// Rows whose foreign key has no parent (`PRAGMA foreign_key_check`).
    pub foreign_key_violations: Vec<String>,
    /// More findings existed than the report keeps.
    pub truncated: bool,
}

impl IntegrityReport {
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty() && self.foreign_key_violations.is_empty()
    }

    /// One-line description for the status bar.
    pub fn summary(&self) -> String {
        if self.is_ok() {
            "Library check passed: no problems found".into()
        } else {
            format!(
                "Library check FAILED: {} integrity problem(s), {} foreign key violation(s){}",
                self.problems.len(),
                self.foreign_key_violations.len(),
                if self.truncated {
                    " (list truncated)"
                } else {
                    ""
                }
            )
        }
    }

    /// Every finding, one per line, for the log.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.problems
            .iter()
            .chain(&self.foreign_key_violations)
            .map(String::as_str)
    }
}

/// What a restore candidate contains, shown before the user confirms the replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub schema_version: i64,
    pub size: u64,
    pub anime: i64,
    pub mylist: i64,
    pub local_files: i64,
    pub queued_commands: i64,
}

/// Outcome of a successful [`restore_database`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreOutcome {
    pub info: BackupInfo,
    /// Copy of the library that was replaced; `None` when there was no database file.
    pub previous_copy: Option<PathBuf>,
}

/// Outcome of [`compact`]: database size (pages in use plus free pages) before and after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactReport {
    pub before: u64,
    pub after: u64,
}

/// Sibling files SQLite keeps next to a database in WAL mode.
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    }
}

/// Canonical form of a path that may not exist yet: the folder is canonicalized instead.
fn canonical(path: &Path) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    match (std::fs::canonicalize(parent_dir(path)), path.file_name()) {
        (Ok(dir), Some(name)) => dir.join(name),
        _ => path.to_path_buf(),
    }
}

/// True when `candidate` is the database file or one of its SQLite sidecar files.
fn is_database_file(db_path: &Path, candidate: &Path) -> bool {
    let candidate = canonical(candidate);
    ["", "-wal", "-shm", "-journal"]
        .iter()
        .any(|suffix| canonical(&sidecar(db_path, suffix)) == candidate)
}

/// Copy the database behind `source` into `dest` as a standalone file (journal mode DELETE,
/// no sidecar files), atomically: the copy is made in a temporary file next to `dest` and
/// renamed over it. Returns the size of the copy.
fn copy_database(source: &Connection, dest: &Path) -> Result<u64> {
    let temp = tempfile::Builder::new()
        .prefix(".animatic-copy-")
        .suffix(".tmp")
        .tempfile_in(parent_dir(dest))?
        .into_temp_path();
    let mut copy = Connection::open(&temp)?;
    {
        let backup = Backup::new(source, &mut copy)?;
        backup.run_to_completion(BACKUP_PAGES_PER_STEP, BACKUP_PAUSE, None)?;
    }
    copy.pragma_update(None, "journal_mode", "DELETE")?;
    copy.close().map_err(|(_, e)| Error::Db(e))?;
    temp.persist(dest).map_err(|e| Error::Io(e.error))?;
    Ok(std::fs::metadata(dest)?.len())
}

/// Back the library up into `dest` with SQLite's online backup API. The live database stays
/// open and writable; the backup is a consistent snapshot and a valid library on its own.
/// Refuses to write over the library itself. Returns the size of the backup file.
pub fn backup_to(db: &Db, dest: &Path) -> Result<u64> {
    if db.path() != Path::new(":memory:") && is_database_file(db.path(), dest) {
        return Err(Error::Other(
            "Choose a different file: a backup cannot overwrite the library itself".into(),
        ));
    }
    copy_database(db.conn(), dest)
}

fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(CHECK_BUSY_TIMEOUT)?;
    Ok(conn)
}

fn push_limited(list: &mut Vec<String>, truncated: &mut bool, item: String) {
    if list.len() < REPORT_LIMIT {
        list.push(item);
    } else {
        *truncated = true;
    }
}

/// Run `PRAGMA integrity_check` (a superset of `quick_check`) and `PRAGMA foreign_key_check`
/// on the database file without modifying it. A file SQLite cannot read at all is reported as
/// a problem, not as an error; only a missing file is an error.
pub fn check_file(path: &Path) -> Result<IntegrityReport> {
    if !path.is_file() {
        return Err(Error::NotFound(path.to_path_buf()));
    }
    let mut report = IntegrityReport::default();
    let conn = match open_read_only(path) {
        Ok(conn) => conn,
        Err(e) => {
            report
                .problems
                .push(format!("cannot open the database: {e}"));
            return Ok(report);
        }
    };
    if let Err(e) = collect_integrity(&conn, &mut report) {
        push_limited(
            &mut report.problems,
            &mut report.truncated,
            format!("cannot read the database: {e}"),
        );
    }
    Ok(report)
}

fn collect_integrity(conn: &Connection, report: &mut IntegrityReport) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("PRAGMA integrity_check")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let line: String = row.get(0)?;
        if line != "ok" {
            push_limited(&mut report.problems, &mut report.truncated, line);
        }
    }
    drop(rows);
    drop(stmt);
    // Only meaningful on a readable file; a corrupt one already failed above.
    if report.problems.is_empty() {
        let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let table: String = row.get(0)?;
            let rowid: Option<i64> = row.get(1)?;
            let parent: String = row.get(2)?;
            let line = match rowid {
                Some(rowid) => {
                    format!("table {table}, row {rowid}: no matching row in {parent}")
                }
                None => format!("table {table}: a row has no matching row in {parent}"),
            };
            push_limited(
                &mut report.foreign_key_violations,
                &mut report.truncated,
                line,
            );
        }
    }
    Ok(())
}

/// Removes the `-wal`/`-shm` files of a database that did not have them when the guard was
/// made. Only for files nothing else has open (a restore source, never the live library).
struct SidecarGuard(Vec<PathBuf>);

impl SidecarGuard {
    fn new(database: &Path) -> SidecarGuard {
        SidecarGuard(
            ["-wal", "-shm"]
                .iter()
                .map(|suffix| sidecar(database, suffix))
                .filter(|path| !path.exists())
                .collect(),
        )
    }
}

impl Drop for SidecarGuard {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A restore source must not be the library itself (or one of its files).
pub fn check_restore_source(db_path: &Path, source: &Path) -> Result<()> {
    if is_database_file(db_path, source) {
        return Err(Error::Other(
            "Choose a backup file: that is the current library itself".into(),
        ));
    }
    Ok(())
}

/// Decide whether `source` may replace the library: it must be a readable SQLite database
/// that passes `integrity_check`, hold the Animatic tables and carry a `meta.schema_version`
/// no newer than this version supports. The error says why a file is refused.
pub fn validate_backup(source: &Path) -> Result<BackupInfo> {
    // Opening a database in WAL mode creates `-wal`/`-shm` files; do not litter the folder of
    // a backup that has none.
    let _sidecars = SidecarGuard::new(source);
    let report = check_file(source)?;
    if let Some(problem) = report.problems.first() {
        return Err(Error::Other(format!(
            "{} is damaged or is not a SQLite database ({problem})",
            source.display()
        )));
    }
    let conn = open_read_only(source)?;
    let not_a_library = |why: &str| {
        Error::Other(format!(
            "{} is a database, but not an Animatic library ({why})",
            source.display()
        ))
    };
    let mut missing = Vec::new();
    for table in REQUIRED_TABLES {
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |r| r.get(0),
            )
            .optional()?;
        if found.is_none() {
            missing.push(table);
        }
    }
    if !missing.is_empty() {
        return Err(not_a_library(&format!(
            "missing table(s): {}",
            missing.join(", ")
        )));
    }
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let Some(version) = version.and_then(|v| v.trim().parse::<i64>().ok()) else {
        return Err(not_a_library("no schema version recorded"));
    };
    if version > SCHEMA_VERSION {
        return Err(Error::Other(format!(
            "{} was written by a newer Animatic (database schema {version}; this version \
             supports up to {SCHEMA_VERSION}). Update Animatic before restoring it",
            source.display()
        )));
    }
    let count = |table: &str| -> rusqlite::Result<i64> {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
    };
    Ok(BackupInfo {
        schema_version: version,
        size: std::fs::metadata(source)?.len(),
        anime: count("anime")?,
        mylist: count("mylist")?,
        local_files: count("local_file")?,
        queued_commands: count("command_queue")?,
    })
}

/// Name of the copy of the replaced database: `<db>.before-restore-YYYYMMDD-HHMMSS`, with a
/// counter when that name is taken (two restores within one second).
fn before_restore_path(db_path: &Path) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let base = sidecar(db_path, &format!(".before-restore-{stamp}"));
    let mut candidate = base.clone();
    let mut counter = 2;
    while candidate.exists() || sidecar(&candidate, "-wal").exists() {
        candidate = sidecar(&base, &format!("-{counter}"));
        counter += 1;
    }
    candidate
}

/// Keep a copy of the current database at `dest`, byte for byte: a damaged database (the usual
/// reason to restore) must stay exactly as it was. When the database is readable its
/// write-ahead log is folded into the main file first; otherwise the `-wal` is copied too, as
/// `<dest>-wal`, so nothing the old library held is lost. Nothing is left at `dest` on failure.
fn preserve_current(db_path: &Path, dest: &Path) -> Result<()> {
    if let Ok(conn) = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        let _ = conn.busy_timeout(CHECK_BUSY_TIMEOUT);
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }
    let wal = sidecar(db_path, "-wal");
    let copied = copy_file_atomically(db_path, dest).and_then(|()| {
        if std::fs::metadata(&wal).is_ok_and(|m| m.len() > 0) {
            copy_file_atomically(&wal, &sidecar(dest, "-wal"))
        } else {
            Ok(())
        }
    });
    if let Err(e) = copied {
        let _ = std::fs::remove_file(dest);
        let _ = std::fs::remove_file(sidecar(dest, "-wal"));
        return Err(Error::Other(format!(
            "could not keep a copy of the current library, so nothing was replaced: {e}"
        )));
    }
    Ok(())
}

fn copy_file_atomically(from: &Path, to: &Path) -> std::io::Result<()> {
    let temp = tempfile::Builder::new()
        .prefix(".animatic-copy-")
        .suffix(".tmp")
        .tempfile_in(parent_dir(to))?
        .into_temp_path();
    std::fs::copy(from, &temp)?;
    temp.persist(to).map_err(|e| e.error)
}

fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Replace the database at `db_path` with the library in `source`.
///
/// The source is validated first and nothing is touched when it is refused. It is then copied
/// into a staging file next to the database and validated again, the current database is kept
/// as `<db>.before-restore-YYYYMMDD-HHMMSS`, its `-wal`/`-shm` files are removed and the
/// staging file is renamed over it, so `db_path` is always either the old or the new library.
///
/// No connection to `db_path` may be open (the worker and projection threads and the UI
/// connection must be closed): the swap cannot succeed on Windows otherwise, and a stale
/// write-ahead log must never meet the new file.
pub fn restore_database(db_path: &Path, source: &Path) -> Result<RestoreOutcome> {
    check_restore_source(db_path, source)?;
    // Opening a source that is in WAL mode creates `-wal`/`-shm` files next to it.
    let _sidecars = SidecarGuard::new(source);
    validate_backup(source)?;

    let staging = sidecar(db_path, ".restore-staging");
    let staged = (|| {
        remove_if_present(&staging)?;
        let from = open_read_only(source)?;
        copy_database(&from, &staging)?;
        drop(from);
        validate_backup(&staging)
    })();
    let info = match staged {
        Ok(info) => info,
        Err(e) => {
            let _ = std::fs::remove_file(&staging);
            return Err(e);
        }
    };

    let previous_copy = if db_path.is_file() {
        let dest = before_restore_path(db_path);
        if let Err(e) = preserve_current(db_path, &dest) {
            let _ = std::fs::remove_file(&staging);
            return Err(e);
        }
        Some(dest)
    } else {
        None
    };
    let kept = previous_copy
        .as_ref()
        .map(|p| format!(" The previous library was kept as {}.", p.display()))
        .unwrap_or_default();
    let swap = remove_if_present(&sidecar(db_path, "-wal"))
        .and_then(|()| remove_if_present(&sidecar(db_path, "-shm")))
        .and_then(|()| std::fs::rename(&staging, db_path).map_err(Error::from));
    if let Err(e) = swap {
        let _ = std::fs::remove_file(&staging);
        return Err(Error::Other(format!(
            "could not replace the library: {e}.{kept}"
        )));
    }
    Ok(RestoreOutcome {
        info,
        previous_copy,
    })
}

fn database_bytes(conn: &Connection) -> rusqlite::Result<u64> {
    let pages: i64 = conn.pragma_query_value(None, "page_count", |r| r.get(0))?;
    let page_size: i64 = conn.pragma_query_value(None, "page_size", |r| r.get(0))?;
    Ok((pages.max(0) * page_size.max(0)) as u64)
}

/// `VACUUM` the library: rewrites it without free pages. Needs the write lock for its
/// duration. Temporary files go to disk rather than memory, which the application's
/// connections otherwise prefer, because a large library would not fit in memory.
pub fn compact(db: &Db) -> Result<CompactReport> {
    let conn = db.conn();
    let before = database_bytes(conn)?;
    conn.pragma_update(None, "temp_store", "FILE")?;
    let vacuum = conn.execute_batch("VACUUM");
    let _ = conn.pragma_update(None, "temp_store", "MEMORY");
    vacuum?;
    // Hand the freed space back to the file system now rather than at the next checkpoint.
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    Ok(CompactReport {
        before,
        after: database_bytes(conn)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::Command;
    use std::io::{Seek, SeekFrom, Write};

    fn library(dir: &Path, name: &str, aids: &[i64]) -> PathBuf {
        let path = dir.join(name);
        let db = Db::open(&path).unwrap();
        for aid in aids {
            db.queue_push(&Command::FetchAnime { aid: *aid }).unwrap();
        }
        db.set_meta("marker", name).unwrap();
        path
    }

    fn queue(path: &Path) -> Vec<(i64, String)> {
        let db = Db::open(path).unwrap();
        db.conn()
            .prepare("SELECT id, payload FROM command_queue ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Overwrite the root page of `table` in a standalone database file with 0xFF bytes.
    fn corrupt_table_page(path: &Path, table: &str) {
        let (root, page_size): (i64, i64) = {
            let conn = Connection::open(path).unwrap();
            (
                conn.query_row(
                    "SELECT rootpage FROM sqlite_master WHERE name = ?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap(),
                conn.pragma_query_value(None, "page_size", |r| r.get(0))
                    .unwrap(),
            )
        };
        let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.seek(SeekFrom::Start(((root - 1) * page_size) as u64))
            .unwrap();
        file.write_all(&vec![0xFF; page_size as usize]).unwrap();
    }

    #[test]
    fn backup_taken_while_the_queue_holds_commands_is_a_valid_library() {
        let dir = tempfile::tempdir().unwrap();
        let live = library(dir.path(), "live.sqlite3", &[1, 2, 3]);
        let db = Db::open(&live).unwrap();
        let target = dir.path().join("backup.sqlite3");

        let bytes = backup_to(&db, &target).unwrap();
        db.queue_push(&Command::FetchAnime { aid: 4 }).unwrap();

        assert_eq!(bytes, std::fs::metadata(&target).unwrap().len());
        assert!(check_file(&target).unwrap().is_ok());
        let info = validate_backup(&target).unwrap();
        assert_eq!(info.queued_commands, 3);
        assert_eq!(info.schema_version, SCHEMA_VERSION);
        assert_eq!(queue(&target), queue(&live)[..3], "same rows and ids");
        assert_eq!(
            entries(dir.path()),
            [
                "backup.sqlite3",
                "live.sqlite3",
                "live.sqlite3-shm",
                "live.sqlite3-wal"
            ]
            .map(String::from),
            "no temporary file and no sidecar of the backup is left"
        );
    }

    #[test]
    fn backup_refuses_to_overwrite_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let live = library(dir.path(), "live.sqlite3", &[1, 2]);
        let db = Db::open(&live).unwrap();
        for target in [live.clone(), sidecar(&live, "-wal")] {
            assert!(backup_to(&db, &target).is_err(), "{}", target.display());
        }
        assert_eq!(db.queue_peek().unwrap().map(|(id, ..)| id), Some(1));
        assert_eq!(queue(&live).len(), 2);
    }

    #[test]
    fn restore_refuses_damaged_foreign_and_newer_files_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let live = library(dir.path(), "live.sqlite3", &[1, 2]);
        let valid = library(dir.path(), "valid.sqlite3", &[9]);
        let valid_bytes = std::fs::read(&valid).unwrap();

        let garbage = dir.path().join("garbage.sqlite3");
        std::fs::write(&garbage, b"this is not a database ".repeat(500)).unwrap();
        let truncated = dir.path().join("truncated.sqlite3");
        std::fs::write(&truncated, &valid_bytes[..8192]).unwrap();
        let corrupt = dir.path().join("corrupt.sqlite3");
        std::fs::copy(&valid, &corrupt).unwrap();
        corrupt_table_page(&corrupt, "command_queue");
        let foreign = dir.path().join("foreign.sqlite3");
        {
            let conn = Connection::open(&foreign).unwrap();
            conn.execute_batch("CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT); INSERT INTO notes(body) VALUES ('x');")
                .unwrap();
        }
        let newer = dir.path().join("newer.sqlite3");
        std::fs::copy(&valid, &newer).unwrap();
        Connection::open(&newer)
            .unwrap()
            .execute(
                "UPDATE meta SET value = '999' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        let empty = dir.path().join("empty.sqlite3");
        std::fs::write(&empty, b"").unwrap();

        // Close every connection (the helpers drop theirs) before taking the reference bytes.
        let live_bytes = std::fs::read(&live).unwrap();
        let before = entries(dir.path());
        for name in [
            "garbage",
            "truncated",
            "corrupt",
            "foreign",
            "newer",
            "empty",
        ] {
            let source = dir.path().join(format!("{name}.sqlite3"));
            let err = restore_database(&live, &source)
                .expect_err(name)
                .to_string();
            assert!(err.contains(&format!("{name}.sqlite3")), "{name}: {err}");
            assert_eq!(std::fs::read(&live).unwrap(), live_bytes, "{name}");
            assert_eq!(entries(dir.path()), before, "{name}: no stray files");
        }
        let newer_err = restore_database(&live, &newer).unwrap_err().to_string();
        assert!(newer_err.contains("newer Animatic"), "{newer_err}");
        let foreign_err = restore_database(&live, &foreign).unwrap_err().to_string();
        assert!(
            foreign_err.contains("not an Animatic library"),
            "{foreign_err}"
        );
        let itself = restore_database(&live, &live).unwrap_err().to_string();
        assert!(itself.contains("current library"), "{itself}");
        assert_eq!(std::fs::read(&live).unwrap(), live_bytes);
    }

    #[test]
    fn restore_replaces_the_library_and_keeps_a_copy_of_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let live = library(dir.path(), "live.sqlite3", &[1]);
        let old_queue = queue(&live);
        let backup = library(dir.path(), "other.sqlite3", &[7, 8]);
        let restored_queue = queue(&backup);
        // A leftover write-ahead log of the old library must never meet the new file.
        std::fs::write(sidecar(&live, "-wal"), vec![0xAB; 64]).unwrap();

        let outcome = restore_database(&live, &backup).unwrap();

        assert_eq!(outcome.info.queued_commands, 2);
        assert_eq!(queue(&live), restored_queue);
        assert_eq!(
            Db::open(&live).unwrap().meta("marker").unwrap().as_deref(),
            Some("other.sqlite3")
        );
        let previous = outcome.previous_copy.expect("previous library kept");
        assert!(
            previous
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("live.sqlite3.before-restore-"),
            "{}",
            previous.display()
        );
        assert!(check_file(&previous).unwrap().is_ok());
        assert_eq!(queue(&previous), old_queue);
        assert_eq!(queue(&backup), restored_queue, "the source is untouched");
        assert!(
            !entries(dir.path())
                .iter()
                .any(|n| n.contains("staging") || n.ends_with(".tmp")),
            "{:?}",
            entries(dir.path())
        );
    }

    #[test]
    fn restore_can_replace_a_damaged_library() {
        let dir = tempfile::tempdir().unwrap();
        let live = library(dir.path(), "live.sqlite3", &[1, 2, 3]);
        let backup = library(dir.path(), "good.sqlite3", &[5]);
        corrupt_table_page(&live, "command_queue");
        assert!(!check_file(&live).unwrap().is_ok());
        let damaged = std::fs::read(&live).unwrap();

        let outcome = restore_database(&live, &backup).unwrap();

        assert_eq!(queue(&live), queue(&backup));
        let previous = outcome.previous_copy.unwrap();
        assert_eq!(
            std::fs::read(&previous).unwrap(),
            damaged,
            "the damaged library is preserved byte for byte"
        );
    }

    #[test]
    fn restore_into_a_missing_library_needs_no_previous_copy() {
        let dir = tempfile::tempdir().unwrap();
        let backup = library(dir.path(), "good.sqlite3", &[5]);
        let target = dir.path().join("fresh").join("live.sqlite3");
        std::fs::create_dir(target.parent().unwrap()).unwrap();
        let outcome = restore_database(&target, &backup).unwrap();
        assert_eq!(outcome.previous_copy, None);
        assert_eq!(queue(&target), queue(&backup));
    }

    #[test]
    fn integrity_check_passes_a_healthy_library_and_reports_a_damaged_page() {
        let dir = tempfile::tempdir().unwrap();
        let live = library(dir.path(), "live.sqlite3", &(1..200).collect::<Vec<_>>());
        let copy = dir.path().join("copy.sqlite3");
        backup_to(&Db::open(&live).unwrap(), &copy).unwrap();

        let healthy = check_file(&copy).unwrap();
        assert!(healthy.is_ok(), "{:?}", healthy);
        assert!(healthy.summary().contains("passed"));

        corrupt_table_page(&copy, "command_queue");
        let damaged = check_file(&copy).unwrap();
        assert!(!damaged.is_ok());
        assert!(!damaged.problems.is_empty());
        assert!(damaged.summary().contains("FAILED"));
        assert!(damaged.lines().count() > 0);

        let garbage = dir.path().join("garbage.sqlite3");
        std::fs::write(&garbage, b"not a database ".repeat(400)).unwrap();
        assert!(!check_file(&garbage).unwrap().is_ok());
        assert!(matches!(
            check_file(&dir.path().join("missing.sqlite3")),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn integrity_check_reports_foreign_key_violations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fk.sqlite3");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "PRAGMA foreign_keys = OFF;
                 CREATE TABLE parent (id INTEGER PRIMARY KEY);
                 CREATE TABLE child (id INTEGER PRIMARY KEY, pid INTEGER REFERENCES parent(id));
                 INSERT INTO child(pid) VALUES (42);",
            )
            .unwrap();
        }
        let report = check_file(&path).unwrap();
        assert!(report.problems.is_empty());
        assert_eq!(report.foreign_key_violations.len(), 1);
        assert!(!report.is_ok());
    }

    #[test]
    fn compact_shrinks_the_library_and_keeps_its_contents() {
        let dir = tempfile::tempdir().unwrap();
        let live = library(dir.path(), "live.sqlite3", &[1, 2, 3]);
        let db = Db::open(&live).unwrap();
        let filler = "x".repeat(2048);
        for n in 0..1500 {
            db.set_meta(&format!("filler{n}"), &filler).unwrap();
        }
        db.conn()
            .execute("DELETE FROM meta WHERE key LIKE 'filler%'", [])
            .unwrap();

        let report = compact(&db).unwrap();

        assert!(report.after < report.before, "{report:?}");
        assert_eq!(db.queue_peek().unwrap().map(|(id, ..)| id), Some(1));
        assert_eq!(queue(&live).len(), 3);
        assert!(check_file(&live).unwrap().is_ok());
    }

    #[test]
    fn required_tables_exist_in_the_current_schema() {
        let db = Db::memory().unwrap();
        for table in REQUIRED_TABLES {
            let found: i64 = db
                .conn()
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "{table}");
        }
    }
}
