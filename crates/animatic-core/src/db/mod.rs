//! SQLite storage for the AniDB cache and the user's library.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::error::Result;
use crate::sync::Command;

pub const SCHEMA_VERSION: i64 = 1;
const SCHEMA: &str = include_str!("schema.sql");
/// `meta` key holding the persisted sync cooldown as `<until epoch seconds>\n<reason>`.
const SYNC_COOLDOWN_KEY: &str = "sync_cooldown";
/// Queue priorities from this value up were produced by the removed "demote on failure" rule.
const LEGACY_DEMOTED_PRIORITY: i64 = 10;

/// Create/upgrade every table. Upgrades of older databases run first and are idempotent, so
/// concurrent openers and repeated opens are safe.
fn init(conn: &Connection) -> Result<()> {
    migrate_command_queue(conn)?;
    conn.execute_batch(SCHEMA)?;
    Ok(())
}

fn command_queue_needs_rebuild(conn: &Connection) -> Result<bool> {
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'command_queue'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let Some(sql) = sql else {
        return Ok(false);
    };
    let new_columns: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_table_info('command_queue') WHERE name IN ('blocked', 'lid')",
        [],
        |r| r.get(0),
    )?;
    Ok(new_columns < 2 || !sql.to_ascii_uppercase().contains("AUTOINCREMENT"))
}

/// Older databases have a `command_queue` without `blocked`/`lid` whose plain rowid ids are
/// reused after deletes. Rebuild it from `schema.sql` (the single definition) keeping every row
/// and id, and undo the removed priority demotion of failed commands.
fn migrate_command_queue(conn: &Connection) -> Result<()> {
    if !command_queue_needs_rebuild(conn)? {
        return Ok(());
    }
    conn.execute_batch("BEGIN IMMEDIATE")?;
    match rebuild_command_queue(conn) {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
    }
    Ok(())
}

type OldQueueRow = (i64, i64, i64, String, i64, Option<String>, i64);

fn rebuild_command_queue(conn: &Connection) -> Result<()> {
    // Another connection may have finished the upgrade while this one waited for the lock.
    if !command_queue_needs_rebuild(conn)? {
        return Ok(());
    }
    let has_blocked: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_table_info('command_queue') WHERE name = 'blocked'",
        [],
        |r| r.get(0),
    )?;
    let indexes: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'command_queue' AND sql IS NOT NULL",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for name in indexes {
        conn.execute_batch(&format!("DROP INDEX \"{}\"", name.replace('"', "\"\"")))?;
    }
    conn.execute_batch("ALTER TABLE command_queue RENAME TO command_queue_old")?;
    conn.execute_batch(SCHEMA)?;
    let blocked_column = if has_blocked > 0 { "blocked" } else { "0" };
    let rows: Vec<OldQueueRow> = conn
        .prepare(&format!(
            "SELECT id, created, priority, payload, tries, last_error, {blocked_column} FROM command_queue_old ORDER BY id"
        ))?
        .query_map([], |r| -> rusqlite::Result<OldQueueRow> {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut insert = conn.prepare(
        "INSERT INTO command_queue(id, created, priority, payload, tries, last_error, blocked, lid)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    for (id, created, priority, payload, tries, last_error, blocked) in rows {
        let (priority, lid) = match serde_json::from_str::<Command>(&payload) {
            Ok(cmd) => (
                if priority >= LEGACY_DEMOTED_PRIORITY {
                    cmd.priority()
                } else {
                    priority
                },
                cmd.mylist_lid(),
            ),
            Err(_) => (priority, None),
        };
        insert.execute(params![
            id, created, priority, payload, tries, last_error, blocked, lid
        ])?;
    }
    drop(insert);
    conn.execute_batch("DROP TABLE command_queue_old")?;
    Ok(())
}

/// A connection to the application database. Each thread opens its own; the database
/// runs in WAL mode so the UI can read while workers write.
pub struct Db {
    conn: Connection,
    path: PathBuf,
}

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        conn.pragma_update(None, "cache_size", -65536)?;
        init(&conn)?;
        let db = Db {
            conn,
            path: path.to_path_buf(),
        };
        if db.meta("schema_version")?.is_none() {
            db.set_meta("schema_version", &SCHEMA_VERSION.to_string())?;
        }
        Ok(db)
    }

    /// In-memory database (tests, previews).
    pub fn memory() -> Result<Db> {
        let conn = Connection::open_in_memory()?;
        init(&conn)?;
        Ok(Db {
            conn,
            path: PathBuf::from(":memory:"),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn meta_i64(&self, key: &str) -> Result<i64> {
        Ok(self.meta(key)?.and_then(|v| v.parse().ok()).unwrap_or(0))
    }

    /// Run `f` as one atomic unit that holds the database write lock from the start, so a
    /// read-modify-write inside it cannot interleave with another connection's write (a
    /// plain deferred transaction would instead fail with `SQLITE_BUSY` on upgrading).
    /// Nested use joins the enclosing transaction through a savepoint.
    pub(crate) fn write_tx<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let (begin, commit, rollback) = if self.conn.is_autocommit() {
            ("BEGIN IMMEDIATE", "COMMIT", "ROLLBACK")
        } else {
            (
                "SAVEPOINT animatic_write",
                "RELEASE animatic_write",
                "ROLLBACK TO animatic_write; RELEASE animatic_write",
            )
        };
        self.conn.execute_batch(begin)?;
        match f().and_then(|v| {
            self.conn.execute_batch(commit)?;
            Ok(v)
        }) {
            Ok(v) => Ok(v),
            Err(e) => {
                let _ = self.conn.execute_batch(rollback);
                Err(e)
            }
        }
    }

    /// Active AniDB sync cooldown as (epoch seconds until, reason). `None` when none was set,
    /// it has expired, or the stored value is unreadable (an unreadable value must never
    /// lock syncing).
    pub fn sync_cooldown(&self) -> Result<Option<(i64, String)>> {
        let Some(raw) = self.meta(SYNC_COOLDOWN_KEY)? else {
            return Ok(None);
        };
        let Some((until, reason)) = raw
            .split_once('\n')
            .and_then(|(until, reason)| Some((until.parse::<i64>().ok()?, reason)))
        else {
            log::warn!("ignoring unreadable sync cooldown {raw:?}");
            return Ok(None);
        };
        Ok((until > crate::format::now()).then(|| (until, reason.to_string())))
    }

    /// Pause AniDB syncing until `until` (epoch seconds); survives restarts.
    pub fn set_sync_cooldown(&self, until: i64, reason: &str) -> Result<()> {
        self.set_meta(SYNC_COOLDOWN_KEY, &format!("{until}\n{reason}"))
    }

    pub fn clear_sync_cooldown(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM meta WHERE key = ?1", [SYNC_COOLDOWN_KEY])?;
        Ok(())
    }

    /// True when the database holds no AniDB or user data yet.
    pub fn is_empty(&self) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT (SELECT count(*) FROM anime) + (SELECT count(*) FROM mylist) + (SELECT count(*) FROM local_file)",
            [],
            |r| r.get(0),
        )?;
        Ok(n == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    fn temp_db_path(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("animatic-db-{name}-{}.sqlite", std::process::id()));
        remove_db_files(&path);
        path
    }

    fn remove_db_files(path: &Path) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    type QueueRow = (i64, i64, i64, Option<String>, i64, Option<i64>);

    fn queue_rows(db: &Db) -> Vec<QueueRow> {
        db.conn()
            .prepare("SELECT id, priority, tries, last_error, blocked, lid FROM command_queue ORDER BY id")
            .unwrap()
            .query_map([], |r| -> rusqlite::Result<QueueRow> {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn old_command_queue_is_rebuilt_idempotently_keeping_rows_and_ids() {
        let path = temp_db_path("migrate");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE command_queue (
                    id INTEGER PRIMARY KEY, created INTEGER NOT NULL, priority INTEGER NOT NULL DEFAULT 1,
                    payload TEXT NOT NULL, tries INTEGER NOT NULL DEFAULT 0, last_error TEXT);",
            )
            .unwrap();
            let edit = serde_json::to_string(&Command::MylistEdit {
                lid: 5,
                edit: Default::default(),
            })
            .unwrap();
            let fetch = serde_json::to_string(&Command::FetchAnime { aid: 1 }).unwrap();
            let rows: [(i64, i64, &str, i64, Option<&str>); 3] = [
                (3, 10, edit.as_str(), 1, Some("boom")),
                (7, 13, fetch.as_str(), 2, Some("timeout")),
                (9, 1, "not json", 0, None),
            ];
            for (id, priority, payload, tries, last_error) in rows {
                conn.execute(
                    "INSERT INTO command_queue(id, created, priority, payload, tries, last_error) VALUES (?1, 100, ?2, ?3, ?4, ?5)",
                    params![id, priority, payload, tries, last_error],
                )
                .unwrap();
            }
        }
        let expected: Vec<QueueRow> = vec![
            (3, 0, 1, Some("boom".into()), 0, Some(5)),
            (7, 3, 2, Some("timeout".into()), 0, None),
            (9, 1, 0, None, 0, None),
        ];
        let db = Db::open(&path).unwrap();
        assert_eq!(
            queue_rows(&db),
            expected,
            "rows, ids and tries kept; demotion undone"
        );
        drop(db);

        let db = Db::open(&path).unwrap();
        assert_eq!(queue_rows(&db), expected, "reopening changes nothing");
        db.conn()
            .execute("DELETE FROM command_queue WHERE id = 9", [])
            .unwrap();
        let fresh = db.queue_push(&Command::FetchAnime { aid: 2 }).unwrap();
        assert!(fresh > 9, "ids of removed rows are never reused");
        drop(db);
        remove_db_files(&path);
    }

    #[test]
    fn sync_cooldown_persists_expires_and_ignores_garbage() {
        let path = temp_db_path("cooldown");
        let now = crate::format::now();
        {
            let db = Db::open(&path).unwrap();
            assert_eq!(db.sync_cooldown().unwrap(), None);
            db.set_sync_cooldown(now + 600, "banned: synthetic")
                .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(
            db.sync_cooldown().unwrap(),
            Some((now + 600, "banned: synthetic".to_string())),
            "survives a restart"
        );
        db.set_sync_cooldown(now - 1, "expired").unwrap();
        assert_eq!(db.sync_cooldown().unwrap(), None);
        for garbage in ["garbage", "abc\nreason", ""] {
            db.set_meta(SYNC_COOLDOWN_KEY, garbage).unwrap();
            assert_eq!(
                db.sync_cooldown().unwrap(),
                None,
                "{garbage:?} must not lock syncing"
            );
        }
        db.set_sync_cooldown(now + 60, "again").unwrap();
        assert!(db.sync_cooldown().unwrap().is_some());
        db.clear_sync_cooldown().unwrap();
        assert_eq!(db.sync_cooldown().unwrap(), None);
        drop(db);
        remove_db_files(&path);
    }

    #[test]
    fn nested_write_transactions_roll_back_independently() {
        let db = Db::memory().unwrap();
        db.write_tx(|| {
            db.set_meta("outer", "1")?;
            let inner: Result<()> = db.write_tx(|| {
                db.set_meta("inner", "1")?;
                Err(Error::Other("synthetic".into()))
            });
            assert!(inner.is_err());
            Ok(())
        })
        .unwrap();
        assert_eq!(db.meta("outer").unwrap().as_deref(), Some("1"));
        assert_eq!(db.meta("inner").unwrap(), None);

        let failed: Result<()> = db.write_tx(|| {
            db.set_meta("lost", "1")?;
            Err(Error::Other("synthetic".into()))
        });
        assert!(failed.is_err());
        assert_eq!(db.meta("lost").unwrap(), None);
    }
}
