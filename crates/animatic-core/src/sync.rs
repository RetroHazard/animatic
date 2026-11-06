//! AniDB commands queued by the UI and executed by the network worker.
//!
//! Commands are persisted in `command_queue` so work done offline (or interrupted by
//! a restart) is replayed once AniDB is reachable again — the legacy "Work offline" mode.

use std::time::Duration;

use animatic_anidb::AnidbError;
use animatic_anidb::commands::{
    self, FileRef, MylistAddResult, MylistEdit, MylistExportCancelReply, MylistExportReply,
};
use animatic_anidb::error::TRANSIENT_COOLDOWN;
use animatic_anidb::http::{HttpClient, TitlesLimits};
use animatic_anidb::udp::UdpClient;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::{Error, Result};
use crate::format;
use crate::library::MylistEntry;
use crate::limits::{
    MYLIST_EXPORT_ID, RequestKind, anime_wait_message, file_wait_message,
    mylist_export_wait_message, titles_dump_wait_message,
};
use crate::settings::FileManager;

/// A command that failed this many times (counting only failures its own fault can explain,
/// see [`CommandError::counts_attempt`]) is blocked instead of retried again.
pub const MAX_QUEUE_TRIES: i64 = 5;

/// One row of the persistent queue as shown to the user.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueEntry {
    pub id: i64,
    /// `None` when the stored payload cannot be read by this version; such a row is kept
    /// (blocked, with an error) and never deleted behind the user's back.
    pub command: Option<Command>,
    pub description: String,
    pub tries: i64,
    pub last_error: Option<String>,
    /// Permanently failed or out of attempts: skipped by the worker until retried.
    pub blocked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Command {
    /// Look a hashed file up on AniDB; optionally add it to mylist with `add`.
    Identify {
        size: u64,
        ed2k: String,
        add: Option<MylistEdit>,
    },
    /// Add a file (or edit when already present) — "Quick add" / "Add to mylist".
    MylistAdd {
        file: FileRef,
        edit: MylistEdit,
    },
    /// Edit an existing mylist entry.
    MylistEdit {
        lid: i64,
        edit: MylistEdit,
    },
    MylistDelete {
        lid: i64,
    },
    /// Re-read one mylist entry ("Force mylist update" runs this for every entry).
    MylistRefresh {
        lid: i64,
    },
    FetchAnime {
        aid: i64,
    },
    FetchEpisode {
        eid: i64,
    },
    FetchGroup {
        gid: i64,
    },
    FetchFile {
        fid: i64,
    },
    MylistStats,
    TitlesDump,
    /// Ask AniDB to prepare a full MyList export (`MYLISTEXPORT`, user-initiated, never
    /// scheduled). The file itself is downloaded from AniDB's web export page and imported
    /// through [`crate::mylist_export`].
    MylistExport,
    /// Cancel the queued MyList export (`MYLISTEXPORT cancel=1`).
    MylistExportCancel,
}

impl Command {
    pub fn describe(&self) -> String {
        match self {
            Command::Identify { size, ed2k, add } => {
                format!(
                    "Identify file {ed2k} ({size} bytes){}",
                    if add.is_some() {
                        " + add to mylist"
                    } else {
                        ""
                    }
                )
            }
            Command::MylistAdd { file, .. } => format!("Add to mylist: {}", file_ref_text(file)),
            Command::MylistEdit { lid, .. } => format!("Edit mylist entry {lid}"),
            Command::MylistDelete { lid } => format!("Remove mylist entry {lid}"),
            Command::MylistRefresh { lid } => format!("Refresh mylist entry {lid}"),
            Command::FetchAnime { aid } => format!("Fetch anime {aid}"),
            Command::FetchEpisode { eid } => format!("Fetch episode {eid}"),
            Command::FetchGroup { gid } => format!("Fetch group {gid}"),
            Command::FetchFile { fid } => format!("Fetch file {fid}"),
            Command::MylistStats => "Fetch mylist statistics".into(),
            Command::TitlesDump => "Download anime title list".into(),
            Command::MylistExport => "Request MyList export from AniDB".into(),
            Command::MylistExportCancel => "Cancel the queued MyList export".into(),
        }
    }

    /// Lower runs first. Mylist changes before lookups, bulk refreshes last.
    pub fn priority(&self) -> i64 {
        match self {
            Command::MylistAdd { .. }
            | Command::MylistEdit { .. }
            | Command::MylistDelete { .. } => 0,
            Command::Identify { .. } => 1,
            Command::FetchFile { .. }
            | Command::FetchEpisode { .. }
            | Command::FetchGroup { .. } => 2,
            Command::FetchAnime { .. }
            | Command::MylistStats
            | Command::TitlesDump
            | Command::MylistExport
            | Command::MylistExportCancel => 3,
            Command::MylistRefresh { .. } => 4,
        }
    }

    pub fn uses_http(&self) -> bool {
        matches!(self, Command::FetchAnime { .. } | Command::TitlesDump)
    }

    /// The mylist entry this command adds to, edits or deletes, for pending-intent lookups.
    pub fn mylist_lid(&self) -> Option<i64> {
        match self {
            Command::MylistEdit { lid, .. }
            | Command::MylistDelete { lid }
            | Command::MylistAdd {
                file: FileRef::Lid(lid),
                ..
            } => Some(*lid),
            _ => None,
        }
    }
}

fn file_ref_text(f: &FileRef) -> String {
    match f {
        FileRef::Fid(fid) => format!("file {fid}"),
        FileRef::Hash { size, ed2k } => format!("{ed2k} ({size} bytes)"),
        FileRef::Lid(lid) => format!("entry {lid}"),
    }
}

/// What a command achieved, for the log and UI refresh decisions.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Identified {
        fid: i64,
        aid: i64,
        lid: Option<i64>,
    },
    Unknown,
    MylistAdded {
        lid: i64,
    },
    MylistEdited,
    MylistDeleted,
    Updated(String),
    Stats(commands::MylistStats),
    NotFound(String),
    /// The command was completed without a request because AniDB's limits do not allow it yet
    /// (title dump once a day, minimum age per anime and per file). The text says when it is
    /// allowed again. It is not a failure and counts no attempt.
    Skipped(String),
}

impl Outcome {
    pub fn describe(&self) -> String {
        match self {
            Outcome::Identified { fid, aid, lid } => match lid {
                Some(l) => format!("known file {fid} (anime {aid}), mylist entry {l}"),
                None => format!("known file {fid} (anime {aid}), not in mylist"),
            },
            Outcome::Unknown => "file unknown to AniDB".into(),
            Outcome::MylistAdded { lid } => format!("added as mylist entry {lid}"),
            Outcome::MylistEdited => "mylist entry updated".into(),
            Outcome::MylistDeleted => "removed from mylist".into(),
            Outcome::Updated(s) => s.clone(),
            Outcome::Stats(s) => format!(
                "{} anime, {} episodes, {} files in mylist",
                s.animes, s.episodes, s.files
            ),
            Outcome::NotFound(s) | Outcome::Skipped(s) => s.clone(),
        }
    }

    /// True when the change affects what the library views display. Account statistics are
    /// stored for the overview, so they count.
    pub fn changes_library(&self) -> bool {
        // Account statistics are stored for the overview, so they count; a command skipped by a
        // request guard changed nothing.
        !matches!(
            self,
            Outcome::Unknown | Outcome::NotFound(_) | Outcome::Skipped(_)
        )
    }
}

/// Mylist parameters a file manager applies to newly added files.
pub fn manager_mylist_edit(m: &FileManager) -> MylistEdit {
    let now = format::now();
    MylistEdit {
        state: m.mylist_state_check.then_some(m.mylist_state),
        viewed: m.mylist_watched_check.then_some(m.mylist_watched),
        viewdate: (m.mylist_watched_check && m.mylist_watched).then_some(now),
        storage: m.mylist_storage_check.then(|| m.mylist_storage.clone()),
        source: m.mylist_source_check.then(|| m.mylist_source.clone()),
        other: m.mylist_other_check.then(|| m.mylist_other.clone()),
    }
}

/// Fold `newer` into `base`: the result is the single edit equivalent to applying both in that
/// order. Unset fields keep the older value; `Some("")` is a real value (it clears the field
/// on AniDB), not "unset". Marking unwatched drops the watched date, and a later "watched"
/// carries its own date, so true/false/true ends as watched at the last timestamp.
fn merge_edit(base: &mut MylistEdit, newer: &MylistEdit) {
    if newer.state.is_some() {
        base.state = newer.state;
    }
    match newer.viewed {
        Some(true) => {
            base.viewed = Some(true);
            base.viewdate = newer.viewdate;
        }
        Some(false) => {
            base.viewed = Some(false);
            base.viewdate = None;
        }
        None => {
            if newer.viewdate.is_some() && base.viewed != Some(false) {
                base.viewdate = newer.viewdate;
            }
        }
    }
    if newer.storage.is_some() {
        base.storage.clone_from(&newer.storage);
    }
    if newer.source.is_some() {
        base.source.clone_from(&newer.source);
    }
    if newer.other.is_some() {
        base.other.clone_from(&newer.other);
    }
}

/// Apply an edit to a mylist entry value.
fn apply_edit_to_entry(e: &mut MylistEntry, edit: &MylistEdit) {
    if let Some(s) = edit.state {
        e.state = s;
    }
    match (edit.viewed, edit.viewdate) {
        (Some(true), date) => {
            e.viewed = true;
            e.viewdate = date.unwrap_or_else(format::now);
        }
        (Some(false), _) => {
            e.viewed = false;
            e.viewdate = 0;
        }
        (None, Some(date)) if e.viewed => e.viewdate = date,
        (None, _) => {}
    }
    if let Some(s) = &edit.storage {
        e.storage = Some(s.clone());
    }
    if let Some(s) = &edit.source {
        e.source = Some(s.clone());
    }
    if let Some(s) = &edit.other {
        e.other = Some(s.clone());
    }
}

/// Apply an edit to the local copy of a mylist entry once AniDB accepted it. A newer edit
/// still waiting in the queue is re-applied on top, so an answer for an older command can
/// never undo what the user changed since. Read, overlay and write are one transaction, so
/// a concurrent enqueue cannot slip in between.
pub fn apply_edit_locally(db: &Db, lid: i64, edit: &MylistEdit) -> Result<()> {
    db.write_tx(|| {
        if let Some(mut e) = db.mylist_entry(lid)? {
            apply_edit_to_entry(&mut e, edit);
            db.overlay_pending_mylist(&mut e)?;
            db.upsert_mylist(&e)?;
        }
        Ok(())
    })
}

/// A queued command row that targets one mylist entry.
struct LidRow {
    id: i64,
    command: Command,
    tries: i64,
    last_error: Option<String>,
}

/// Short single-line excerpt of an unreadable payload for the queue listing.
fn payload_preview(payload: &str) -> String {
    let mut preview: String = payload.chars().take(80).collect();
    if payload.chars().nth(80).is_some() {
        preview.push('…');
    }
    preview
}

/// Persistent queue operations.
impl Db {
    /// Queue a command. Returns the new row id, or 0 when an identical command is already
    /// queued. A `MylistEdit` is merged with the entry's outstanding edits instead (see
    /// [`Db::queue_edit_mylist`]).
    pub fn queue_push(&self, cmd: &Command) -> Result<i64> {
        self.write_tx(|| self.push_locked(cmd))
    }

    /// Queue many commands in one transaction; returns how many were actually added.
    pub fn queue_push_many(&self, cmds: &[Command]) -> Result<usize> {
        self.write_tx(|| {
            let mut added = 0;
            for cmd in cmds {
                if self.push_locked(cmd)? != 0 {
                    added += 1;
                }
            }
            Ok(added)
        })
    }

    fn push_locked(&self, cmd: &Command) -> Result<i64> {
        if let Command::MylistEdit { lid, edit } = cmd {
            return self.coalesce_edits(*lid, Some(edit));
        }
        if let Command::MylistDelete { lid } = cmd {
            let rows = self.queue_rows_for_lid(*lid)?;
            if rows
                .iter()
                .any(|r| matches!(r.command, Command::MylistDelete { .. }))
            {
                return Ok(0);
            }
            // Edits of an entry that is being removed are moot: none may run (or be retried)
            // after, or reordered around, the delete. An edit already in flight just finishes.
            for row in rows {
                if matches!(row.command, Command::MylistEdit { .. }) {
                    self.queue_remove(row.id)?;
                }
            }
        }
        let payload = serde_json::to_string(cmd)?;
        let duplicate: bool = self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM command_queue WHERE payload = ?1)",
            [&payload],
            |r| r.get(0),
        )?;
        if duplicate {
            return Ok(0);
        }
        self.insert_queue_row(cmd, &payload, 0, None)
    }

    fn insert_queue_row(
        &self,
        cmd: &Command,
        payload: &str,
        tries: i64,
        last_error: Option<&str>,
    ) -> Result<i64> {
        self.conn().execute(
            "INSERT INTO command_queue(created, priority, payload, tries, last_error, lid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                format::now(),
                cmd.priority(),
                payload,
                tries,
                last_error,
                cmd.mylist_lid()
            ],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    /// Record the user's latest intent for a mylist entry: optimistically update the local
    /// copy and enqueue the change in one transaction. All outstanding edits of the same entry
    /// (including failed/blocked ones, so their fields are not lost) are merged with this one
    /// in chronological order, deleted, and replaced by one row with a fresh id. Ids are never
    /// reused, so a command that was already in flight under an old id can neither be
    /// acknowledged against, nor removed together with, the newer intent.
    ///
    /// Returns the new row id, or 0 when nothing was queued: the entry has a pending delete,
    /// or the merged edit changes nothing.
    pub fn queue_edit_mylist(&self, lid: i64, edit: &MylistEdit) -> Result<i64> {
        self.write_tx(|| self.coalesce_edits(lid, Some(edit)))
    }

    /// Merge the outstanding `MylistEdit` rows of `lid` (plus `new`) into one fresh row.
    /// Without `new` only duplicates are merged and nothing is applied locally.
    fn coalesce_edits(&self, lid: i64, new: Option<&MylistEdit>) -> Result<i64> {
        let rows = self.queue_rows_for_lid(lid)?;
        if rows
            .iter()
            .any(|r| matches!(r.command, Command::MylistDelete { .. }))
        {
            // The entry is being deleted: new edits are refused, and old edits (retried
            // from legacy rows) are dropped rather than re-queued behind the delete.
            if new.is_none() {
                for row in &rows {
                    if matches!(row.command, Command::MylistEdit { .. }) {
                        self.queue_remove(row.id)?;
                    }
                }
            }
            return Ok(0);
        }
        let mut merged = MylistEdit::default();
        let mut superseded = Vec::new();
        let mut tries: i64 = 0;
        let mut last_error = None;
        for row in rows {
            if let Command::MylistEdit { edit, .. } = row.command {
                merge_edit(&mut merged, &edit);
                superseded.push(row.id);
                tries = tries.max(row.tries);
                if row.last_error.is_some() {
                    last_error = row.last_error;
                }
            }
        }
        match new {
            Some(new) => {
                let mut new = new.clone();
                if new.viewed == Some(true) && new.viewdate.is_none() {
                    // The watch time is when the user marked it, not when AniDB hears of it.
                    new.viewdate = Some(format::now());
                }
                merge_edit(&mut merged, &new);
            }
            None if superseded.len() < 2 => {
                return Ok(superseded.first().copied().unwrap_or(0));
            }
            None => {}
        }
        for id in &superseded {
            self.queue_remove(*id)?;
        }
        if merged == MylistEdit::default() {
            return Ok(0);
        }
        let cmd = Command::MylistEdit {
            lid,
            edit: merged.clone(),
        };
        let id = self.insert_queue_row(
            &cmd,
            &serde_json::to_string(&cmd)?,
            tries,
            last_error.as_deref(),
        )?;
        if new.is_some()
            && let Some(mut entry) = self.mylist_entry(lid)?
        {
            apply_edit_to_entry(&mut entry, &merged);
            self.upsert_mylist(&entry)?;
        }
        Ok(id)
    }

    fn queue_rows_for_lid(&self, lid: i64) -> Result<Vec<LidRow>> {
        let mut st = self.conn().prepare_cached(
            "SELECT id, payload, tries, last_error FROM command_queue WHERE lid = ?1 ORDER BY id",
        )?;
        let rows = st.query_map([lid], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, payload, tries, last_error) = row?;
            if let Ok(command) = serde_json::from_str(&payload) {
                out.push(LidRow {
                    id,
                    command,
                    tries,
                    last_error,
                });
            }
        }
        Ok(out)
    }

    /// Apply every queued edit of the entry (blocked ones too: they are still the user's
    /// latest intent) on top of `entry`. Call before writing data that came from AniDB, so a
    /// stale reply cannot overwrite local changes that were not sent yet.
    pub fn overlay_pending_mylist(&self, entry: &mut MylistEntry) -> Result<()> {
        let mut pending: Option<MylistEdit> = None;
        for row in self.queue_rows_for_lid(entry.lid)? {
            if let Command::MylistEdit { edit, .. } = row.command {
                merge_edit(pending.get_or_insert_with(MylistEdit::default), &edit);
            }
        }
        if let Some(edit) = pending {
            apply_edit_to_entry(entry, &edit);
        }
        Ok(())
    }

    /// True when a command that adds to, edits or deletes this mylist entry is queued
    /// (blocked ones included): local data must not be dropped or overwritten from AniDB.
    pub fn mylist_has_pending_intent(&self, lid: i64) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM command_queue WHERE lid = ?1)",
            [lid],
            |r| r.get(0),
        )?)
    }

    /// True when something was queued for this entry after command `id` (e.g. new edits made
    /// after the queue was cleared while `id` was in flight).
    pub(crate) fn mylist_has_pending_intent_after(&self, lid: i64, id: i64) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM command_queue WHERE lid = ?1 AND id > ?2)",
            params![lid, id],
            |r| r.get(0),
        )?)
    }

    /// True while the command row still exists, i.e. it was neither acknowledged, superseded
    /// by a newer edit, nor cleared.
    pub(crate) fn queue_contains(&self, id: i64) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM command_queue WHERE id = ?1)",
            [id],
            |r| r.get(0),
        )?)
    }

    /// Next runnable command: lowest priority value, oldest first; blocked rows are skipped.
    /// A row this version cannot read is blocked with an error (never deleted) and skipped.
    pub fn queue_peek(&self) -> Result<Option<(i64, Command, i64)>> {
        loop {
            let row: Option<(i64, String, i64)> = self
                .conn()
                .query_row(
                    "SELECT id, payload, tries FROM command_queue WHERE blocked = 0 ORDER BY priority, id LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let Some((id, payload, tries)) = row else {
                return Ok(None);
            };
            match serde_json::from_str(&payload) {
                Ok(cmd) => return Ok(Some((id, cmd, tries))),
                Err(e) => {
                    log::warn!("blocking unreadable queued command {id}: {e}");
                    self.queue_block(id, &format!("unreadable queued command: {e}"))?;
                }
            }
        }
    }

    /// Acknowledge a finished command. Does nothing when `id` was superseded or cleared in
    /// the meantime: ids are never reused, so newer intent is never removed by an old ack.
    pub fn queue_remove(&self, id: i64) -> Result<()> {
        self.conn()
            .execute("DELETE FROM command_queue WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Record a failed attempt. The command keeps its place in the order; after
    /// [`MAX_QUEUE_TRIES`] attempts it is blocked.
    pub fn queue_failed(&self, id: i64, err: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE command_queue SET tries = tries + 1, last_error = ?2,
               blocked = CASE WHEN tries + 1 >= ?3 THEN 1 ELSE blocked END
             WHERE id = ?1",
            params![id, err, MAX_QUEUE_TRIES],
        )?;
        Ok(())
    }

    /// Record why a command did not run without counting an attempt (the network, the server
    /// or a cooldown stopped it, not the command).
    pub fn queue_deferred(&self, id: i64, err: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE command_queue SET last_error = ?2 WHERE id = ?1",
            params![id, err],
        )?;
        Ok(())
    }

    /// The command can never succeed as queued: keep it, visibly, and never run it again
    /// until [`Db::queue_retry_failed`].
    pub fn queue_block(&self, id: i64, err: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE command_queue SET tries = tries + 1, last_error = ?2, blocked = 1 WHERE id = ?1",
            params![id, err],
        )?;
        Ok(())
    }

    /// Block every runnable command (credentials or client version need correcting first).
    /// Returns how many were blocked.
    pub fn queue_block_all(&self, err: &str) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE command_queue SET last_error = ?1, blocked = 1 WHERE blocked = 0",
            [err],
        )?)
    }

    /// Put blocked commands back in the queue with fresh attempts. Edits of one mylist entry
    /// are first merged into the latest one, so a retried old edit can never overwrite a newer
    /// one. Returns how many blocked rows there were.
    pub fn queue_retry_failed(&self) -> Result<usize> {
        self.write_tx(|| {
            let lids: Vec<i64> = self
                .conn()
                .prepare(
                    "SELECT lid FROM command_queue WHERE lid IS NOT NULL GROUP BY lid HAVING count(*) > 1",
                )?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let blocked = self.queue_blocked_len()?;
            self.conn().execute(
                "UPDATE command_queue SET blocked = 0, tries = 0 WHERE blocked = 1",
                [],
            )?;
            for lid in lids {
                self.coalesce_edits(lid, None)?;
            }
            Ok(blocked as usize)
        })
    }

    pub fn queue_len(&self) -> Result<i64> {
        Ok(self
            .conn()
            .query_row("SELECT count(*) FROM command_queue", [], |r| r.get(0))?)
    }

    pub fn queue_blocked_len(&self) -> Result<i64> {
        Ok(self.conn().query_row(
            "SELECT count(*) FROM command_queue WHERE blocked = 1",
            [],
            |r| r.get(0),
        )?)
    }

    /// Every queued command in execution order, blocked ones last.
    pub fn queue_list(&self) -> Result<Vec<QueueEntry>> {
        let mut st = self.conn().prepare(
            "SELECT id, payload, tries, last_error, blocked FROM command_queue ORDER BY blocked, priority, id",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, payload, tries, last_error, blocked) = r?;
            out.push(match serde_json::from_str::<Command>(&payload) {
                Ok(command) => QueueEntry {
                    id,
                    description: command.describe(),
                    command: Some(command),
                    tries,
                    last_error,
                    blocked: blocked != 0,
                },
                Err(e) => QueueEntry {
                    id,
                    command: None,
                    description: format!(
                        "Unreadable queued command: {}",
                        payload_preview(&payload)
                    ),
                    tries,
                    last_error: last_error
                        .or_else(|| Some(format!("unreadable queued command: {e}"))),
                    blocked: true,
                },
            });
        }
        Ok(out)
    }

    pub fn queue_clear(&self) -> Result<()> {
        self.conn().execute("DELETE FROM command_queue", [])?;
        Ok(())
    }
}

/// Error wrapper keeping the information whether a retry later makes sense.
#[derive(Debug, Clone)]
pub struct CommandError {
    pub message: String,
    /// Network or server trouble: the command may succeed later.
    pub transient: bool,
    /// AniDB refuses the client (banned/outdated/bad credentials): stop the queue.
    pub fatal: bool,
    /// How long all network use must pause (ban, outage, network failure). `None` on a fatal
    /// error means it needs correcting first (credentials, client version); `None` on a
    /// non-transient, non-fatal error means this command can never succeed and must be blocked.
    pub retry_after: Option<Duration>,
    /// The failure may be this command's own fault (counts towards [`MAX_QUEUE_TRIES`]);
    /// false for connectivity, outage and ban errors, which say nothing about the command.
    pub counts_attempt: bool,
}

impl From<AnidbError> for CommandError {
    fn from(e: AnidbError) -> Self {
        CommandError {
            transient: e.is_transient(),
            fatal: e.is_fatal(),
            retry_after: e.retry_after(),
            counts_attempt: matches!(e, AnidbError::Http(_) | AnidbError::ServerError(600, _)),
            message: e.to_string(),
        }
    }
}

impl From<Error> for CommandError {
    fn from(e: Error) -> Self {
        // Storage trouble may pass; anything else (bad XML/JSON, settings, ...) is permanent.
        let storage = matches!(e, Error::Io(_) | Error::Db(_));
        CommandError {
            message: e.to_string(),
            transient: storage,
            fatal: false,
            retry_after: storage.then_some(TRANSIENT_COOLDOWN),
            counts_attempt: true,
        }
    }
}

/// AniDB has no mylist entry for an edit. While that edit is still queued it is the user's
/// current intent and must not be turned into a "successful removal": it is kept (the caller
/// blocks it) with an actionable error, and the local entry stays. An edit that was superseded
/// or cleared meanwhile is left alone: the newer intent decides.
fn edit_target_missing(
    db: &Db,
    queue_id: i64,
    lid: i64,
) -> std::result::Result<Outcome, CommandError> {
    if db.queue_contains(queue_id)? {
        return Err(CommandError {
            message: format!(
                "mylist entry {lid} no longer exists on AniDB; the edit is kept (blocked): discard it from the queue or refresh the entry"
            ),
            transient: false,
            fatal: false,
            retry_after: None,
            counts_attempt: true,
        });
    }
    Ok(Outcome::NotFound(format!(
        "mylist entry {lid} no longer exists on AniDB (the edit was superseded)"
    )))
}

/// AniDB removed the entry. Mirror that locally unless the user queued something for the
/// entry after this delete (e.g. cleared the queue while it was in flight and edited again):
/// that newer intent must not be deleted with it. Check and delete are one transaction.
fn mylist_deleted_on_anidb(db: &Db, queue_id: i64, lid: i64) -> Result<Outcome> {
    let removed = db.write_tx(|| {
        if db.mylist_has_pending_intent_after(lid, queue_id)? {
            return Ok(false);
        }
        db.delete_mylist(lid)?;
        Ok(true)
    })?;
    Ok(if removed {
        Outcome::MylistDeleted
    } else {
        Outcome::Updated(format!(
            "mylist entry {lid} was removed on AniDB; kept locally because of newer pending changes"
        ))
    })
}

/// Execute the queued command `queue_id` (from `queue_peek`). Network calls are rate limited
/// by the clients. The id lets replies be applied only while they still match the queue.
pub fn execute(
    queue_id: i64,
    cmd: &Command,
    db: &Db,
    udp: &mut UdpClient,
    http: &mut HttpClient,
) -> std::result::Result<Outcome, CommandError> {
    match cmd {
        Command::Identify { size, ed2k, add } => {
            let Some(info) = commands::file_by_hash(udp, *size, ed2k)? else {
                return Ok(Outcome::Unknown);
            };
            db.store_file_info(&info)?;
            let mut lid = (info.lid != 0).then_some(info.lid);
            if lid.is_none()
                && let Some(edit) = add
            {
                match commands::mylist_add(udp, &FileRef::Fid(info.fid), edit, false)? {
                    MylistAddResult::Added { lid: new } => {
                        store_added_entry(db, new, info.fid, edit)?;
                        lid = Some(new);
                    }
                    MylistAddResult::AlreadyInMylist(m) => {
                        db.store_api_mylist(&m)?;
                        lid = Some(m.lid);
                    }
                    _ => {}
                }
            }
            Ok(Outcome::Identified {
                fid: info.fid,
                aid: info.aid,
                lid,
            })
        }
        Command::MylistAdd { file, edit } => {
            let fid = match file {
                FileRef::Fid(f) => *f,
                FileRef::Hash { size, ed2k } => match commands::file_by_hash(udp, *size, ed2k)? {
                    Some(info) => {
                        db.store_file_info(&info)?;
                        info.fid
                    }
                    None => return Ok(Outcome::Unknown),
                },
                FileRef::Lid(lid) => db.mylist_entry(*lid)?.map(|e| e.fid).unwrap_or(0),
            };
            match commands::mylist_add(udp, &FileRef::Fid(fid), edit, false)? {
                MylistAddResult::Added { lid } => {
                    store_added_entry(db, lid, fid, edit)?;
                    Ok(Outcome::MylistAdded { lid })
                }
                MylistAddResult::AlreadyInMylist(m) => {
                    // Already present: apply the requested values as an edit.
                    db.store_api_mylist(&m)?;
                    if *edit != MylistEdit::default() {
                        commands::mylist_add(udp, &FileRef::Lid(m.lid), edit, true)?;
                        apply_edit_locally(db, m.lid, edit)?;
                    }
                    Ok(Outcome::MylistEdited)
                }
                MylistAddResult::NoSuchFile => {
                    Ok(Outcome::NotFound(format!("AniDB has no file {fid}")))
                }
                _ => Ok(Outcome::NotFound("mylist add failed".into())),
            }
        }
        Command::MylistEdit { lid, edit } => {
            match commands::mylist_add(udp, &FileRef::Lid(*lid), edit, true)? {
                MylistAddResult::Edited { .. } | MylistAddResult::Added { .. } => {
                    apply_edit_locally(db, *lid, edit)?;
                    Ok(Outcome::MylistEdited)
                }
                MylistAddResult::NoSuchEntry => edit_target_missing(db, queue_id, *lid),
                other => Ok(Outcome::NotFound(format!(
                    "unexpected mylist reply {other:?}"
                ))),
            }
        }
        Command::MylistDelete { lid } => {
            commands::mylist_del(udp, &FileRef::Lid(*lid))?;
            Ok(mylist_deleted_on_anidb(db, queue_id, *lid)?)
        }
        Command::MylistRefresh { lid } => match commands::mylist(udp, &FileRef::Lid(*lid))? {
            Some(m) => {
                db.store_api_mylist(&m)?;
                Ok(Outcome::Updated(format!("mylist entry {lid} refreshed")))
            }
            None => {
                // Check and delete in one transaction: local changes queued meanwhile survive.
                let removed = db.write_tx(|| {
                    if db.mylist_has_pending_intent(*lid)? {
                        return Ok(false);
                    }
                    db.delete_mylist(*lid)?;
                    Ok(true)
                })?;
                Ok(Outcome::Updated(if removed {
                    format!("mylist entry {lid} was removed on AniDB")
                } else {
                    format!(
                        "mylist entry {lid} is missing on AniDB; kept because of pending local changes"
                    )
                }))
            }
        },
        Command::FetchAnime { aid } => {
            let now = format::now();
            if let Some(until) = db.request_next_allowed(RequestKind::Anime, *aid, now)? {
                return Ok(Outcome::Skipped(anime_wait_message(*aid, until)));
            }
            // A request that cannot be sent (no client name) is not an attempt.
            http.check_client()?;
            // Saved before the request leaves: a failure, crash or restart cannot allow a repeat.
            db.record_request_attempt(RequestKind::Anime, *aid, now)?;
            let xml = http.anime(*aid)?;
            db.store_anime_xml(&xml)?;
            Ok(Outcome::Updated(format!("anime {aid} updated")))
        }
        Command::FetchEpisode { eid } => match commands::episode(udp, *eid)? {
            Some(e) => {
                db.store_episode_info(&e)?;
                Ok(Outcome::Updated(format!("episode {eid} updated")))
            }
            None => Ok(Outcome::NotFound(format!("no episode {eid}"))),
        },
        Command::FetchGroup { gid } => match commands::group(udp, *gid)? {
            Some(g) => {
                db.store_group_info(&g)?;
                Ok(Outcome::Updated(format!("group {gid} updated")))
            }
            None => Ok(Outcome::NotFound(format!("no group {gid}"))),
        },
        Command::FetchFile { fid } => {
            let now = format::now();
            if let Some(until) = db.request_next_allowed(RequestKind::File, *fid, now)? {
                return Ok(Outcome::Skipped(file_wait_message(*fid, until)));
            }
            // Log in first: a login that fails (wrong password, no reply) sent no FILE request.
            udp.ensure_login()?;
            db.record_request_attempt(RequestKind::File, *fid, now)?;
            match commands::file_by_fid(udp, *fid)? {
                Some(info) => {
                    db.store_file_info(&info)?;
                    Ok(Outcome::Updated(format!("file {fid} updated")))
                }
                None => Ok(Outcome::NotFound(format!("no file {fid}"))),
            }
        }
        Command::MylistStats => {
            let stats = commands::mylist_stats(udp)?;
            db.store_mylist_stats(&stats, format::now())?;
            Ok(Outcome::Stats(stats))
        }
        Command::MylistExport => {
            let now = format::now();
            if let Some(until) =
                db.request_next_allowed(RequestKind::MylistExport, MYLIST_EXPORT_ID, now)?
            {
                return Ok(Outcome::Skipped(mylist_export_wait_message(until)));
            }
            // Log in first: a login that fails sent no export request, so it is no attempt.
            udp.ensure_login()?;
            // Saved before the request leaves: a failure, crash or restart cannot allow a repeat.
            db.record_request_attempt(RequestKind::MylistExport, MYLIST_EXPORT_ID, now)?;
            Ok(
                match commands::mylist_export(udp, commands::MYLIST_EXPORT_TEMPLATE)? {
                    MylistExportReply::Queued => Outcome::Updated(
                        "MyList export requested: AniDB announces it as a message when it is ready (up to 24 hours); download it from the export page and use File > Import MyList export"
                            .into(),
                    ),
                    // An export is already on its way: report it, never retry or fail.
                    MylistExportReply::AlreadyQueued => Outcome::Skipped(
                        "A MyList export is already queued on AniDB; wait for its message, then download it from the export page"
                            .into(),
                    ),
                },
            )
        }
        Command::MylistExportCancel => Ok(match commands::mylist_export_cancel(udp)? {
            MylistExportCancelReply::Cancelled => {
                Outcome::Updated("The queued MyList export was cancelled".into())
            }
            MylistExportCancelReply::NothingToCancel => {
                Outcome::Skipped("AniDB has no MyList export queued or being prepared".into())
            }
        }),
        Command::TitlesDump => {
            if let Some(until) = db.titles_dump_next_allowed(format::now())? {
                return Ok(Outcome::Skipped(titles_dump_wait_message(until)));
            }
            // The download is spooled outside the library database; it is only written to
            // (in one short transaction) once the whole dump has been validated.
            let limits = TitlesLimits::default();
            let import = db.import_title_dump::<CommandError>(|sink| {
                // Saved before the request leaves (and after the local spool was set up, so a
                // local failure is not an attempt): a failed or truncated download, a crash or
                // a restart cannot allow a second request inside the interval.
                db.record_titles_dump_attempt(format::now())?;
                http.titles_dump(&limits, sink)
            })?;
            Ok(Outcome::Updated(format!(
                "{} titles downloaded ({} new)",
                import.titles, import.added
            )))
        }
    }
}

fn store_added_entry(db: &Db, lid: i64, fid: i64, edit: &MylistEdit) -> Result<()> {
    let (eid, aid, gid): (i64, i64, i64) = db
        .conn()
        .query_row(
            "SELECT eid, aid, gid FROM file WHERE fid = ?1",
            [fid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap_or((0, 0, 0));
    let viewed = edit.viewed.unwrap_or(false);
    db.upsert_mylist(&crate::library::MylistEntry {
        lid,
        fid,
        eid,
        aid,
        gid,
        date: format::now(),
        state: edit.state.unwrap_or(0),
        viewed,
        viewdate: if viewed {
            edit.viewdate.unwrap_or_else(format::now)
        } else {
            0
        },
        storage: edit.storage.clone(),
        source: edit.source.clone(),
        other: edit.other.clone(),
        filestate: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use animatic_anidb::error::BAN_COOLDOWN;

    fn entry(db: &Db, lid: i64) {
        db.upsert_mylist(&MylistEntry {
            lid,
            fid: lid + 100,
            ..Default::default()
        })
        .unwrap();
    }

    fn watched(date: i64) -> MylistEdit {
        MylistEdit {
            viewed: Some(true),
            viewdate: Some(date),
            ..Default::default()
        }
    }

    fn unwatched() -> MylistEdit {
        MylistEdit {
            viewed: Some(false),
            ..Default::default()
        }
    }

    fn state(state: i64) -> MylistEdit {
        MylistEdit {
            state: Some(state),
            ..Default::default()
        }
    }

    /// The one queued edit of an entry: there must never be more than one.
    fn sole_edit(db: &Db, lid: i64) -> (i64, MylistEdit) {
        let rows: Vec<_> = db
            .queue_list()
            .unwrap()
            .into_iter()
            .filter_map(|e| match e.command {
                Some(Command::MylistEdit { lid: l, edit }) if l == lid => Some((e.id, edit)),
                _ => None,
            })
            .collect();
        assert_eq!(rows.len(), 1, "one coalesced edit per entry");
        rows.into_iter().next().unwrap()
    }

    fn temp_db_path(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "animatic-queue-{name}-{}.sqlite",
            std::process::id()
        ));
        remove_db_files(&path);
        path
    }

    fn remove_db_files(path: &std::path::Path) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn queue_orders_by_priority_and_skips_duplicates() {
        let db = Db::memory().unwrap();
        db.queue_push(&Command::FetchAnime { aid: 1 }).unwrap();
        db.queue_push(&Command::MylistDelete { lid: 5 }).unwrap();
        assert_eq!(db.queue_push(&Command::FetchAnime { aid: 1 }).unwrap(), 0);
        assert_eq!(db.queue_len().unwrap(), 2);
        let (_, cmd, _) = db.queue_peek().unwrap().unwrap();
        assert_eq!(cmd, Command::MylistDelete { lid: 5 });
    }

    #[test]
    fn failed_command_keeps_its_place_and_blocks_after_bounded_tries() {
        let db = Db::memory().unwrap();
        db.queue_push(&Command::FetchAnime { aid: 1 }).unwrap();
        db.queue_push(&Command::MylistDelete { lid: 5 }).unwrap();
        let (id, _, _) = db.queue_peek().unwrap().unwrap();
        for attempt in 1..MAX_QUEUE_TRIES {
            db.queue_failed(id, "timeout").unwrap();
            let (again, cmd, tries) = db.queue_peek().unwrap().unwrap();
            assert_eq!(
                (again, cmd, tries),
                (id, Command::MylistDelete { lid: 5 }, attempt),
                "a failed command keeps its place and its tries"
            );
        }
        db.queue_failed(id, "timeout").unwrap();
        let (_, next, _) = db.queue_peek().unwrap().unwrap();
        assert_eq!(
            next,
            Command::FetchAnime { aid: 1 },
            "out of attempts: blocked"
        );
        let list = db.queue_list().unwrap();
        assert_eq!(list.len(), 2, "a blocked command is kept, never dropped");
        assert_eq!(list[1].id, id);
        assert!(list[1].blocked && !list[0].blocked);
        assert_eq!(list[1].tries, MAX_QUEUE_TRIES);
        assert_eq!(list[1].last_error.as_deref(), Some("timeout"));
    }

    #[test]
    fn deferred_commands_do_not_use_up_attempts() {
        let db = Db::memory().unwrap();
        db.queue_push(&Command::FetchAnime { aid: 1 }).unwrap();
        let (id, _, _) = db.queue_peek().unwrap().unwrap();
        for _ in 0..(MAX_QUEUE_TRIES * 2) {
            db.queue_deferred(id, "offline").unwrap();
        }
        let (again, _, tries) = db.queue_peek().unwrap().unwrap();
        assert_eq!((again, tries), (id, 0));
        assert_eq!(
            db.queue_list().unwrap()[0].last_error.as_deref(),
            Some("offline")
        );
    }

    #[test]
    fn blocked_commands_are_skipped_until_retried() {
        let db = Db::memory().unwrap();
        db.queue_push(&Command::FetchAnime { aid: 1 }).unwrap();
        db.queue_push(&Command::FetchAnime { aid: 2 }).unwrap();
        let (id, _, _) = db.queue_peek().unwrap().unwrap();
        db.queue_block(id, "illegal input").unwrap();
        let (_, next, _) = db.queue_peek().unwrap().unwrap();
        assert_eq!(next, Command::FetchAnime { aid: 2 });
        assert_eq!(db.queue_blocked_len().unwrap(), 1);

        assert_eq!(
            db.queue_block_all("authentication: wrong password")
                .unwrap(),
            1
        );
        assert!(db.queue_peek().unwrap().is_none(), "everything is blocked");
        assert_eq!(db.queue_len().unwrap(), 2, "nothing is dropped");

        assert_eq!(db.queue_retry_failed().unwrap(), 2);
        let (first, _, tries) = db.queue_peek().unwrap().unwrap();
        assert_eq!((first, tries), (id, 0));
        assert_eq!(db.queue_blocked_len().unwrap(), 0);
    }

    #[test]
    fn unreadable_rows_are_blocked_and_listed_not_deleted() {
        let db = Db::memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO command_queue(created, priority, payload) VALUES (0, 0, '{\"FromTheFuture\":1}')",
                [],
            )
            .unwrap();
        db.queue_push(&Command::FetchAnime { aid: 1 }).unwrap();
        let (_, next, _) = db.queue_peek().unwrap().unwrap();
        assert_eq!(next, Command::FetchAnime { aid: 1 });
        assert_eq!(db.queue_len().unwrap(), 2, "the unreadable row is kept");
        assert_eq!(db.queue_blocked_len().unwrap(), 1);
        let list = db.queue_list().unwrap();
        assert_eq!(list[1].command, None);
        assert!(list[1].blocked);
        assert!(list[1].description.starts_with("Unreadable queued command"));
        assert!(list[1].last_error.is_some());
    }

    #[test]
    fn latest_watched_intent_wins_true_false_true_despite_old_in_flight_ack() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        let first = db.queue_edit_mylist(1, &watched(10)).unwrap();
        let (in_flight, _, _) = db.queue_peek().unwrap().unwrap();
        assert_eq!(in_flight, first);
        let second = db.queue_edit_mylist(1, &unwatched()).unwrap();
        let third = db.queue_edit_mylist(1, &watched(30)).unwrap();
        assert!(
            first < second && second < third,
            "every edit gets a fresh id"
        );
        assert_eq!(db.queue_len().unwrap(), 1);
        assert_eq!(sole_edit(&db, 1), (third, watched(30)));
        let e = db.mylist_entry(1).unwrap().unwrap();
        assert_eq!((e.viewed, e.viewdate), (true, 30));

        // The old command's outcome arrives after the newer intent was queued.
        db.queue_failed(first, "timeout").unwrap();
        db.queue_block(first, "illegal input").unwrap();
        db.queue_remove(first).unwrap();
        let list = db.queue_list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(
            (list[0].id, list[0].tries, list[0].blocked),
            (third, 0, false)
        );
        assert_eq!(list[0].last_error, None);
        apply_edit_locally(&db, 1, &watched(10)).unwrap();
        apply_edit_locally(&db, 1, &unwatched()).unwrap();
        let e = db.mylist_entry(1).unwrap().unwrap();
        assert_eq!(
            (e.viewed, e.viewdate),
            (true, 30),
            "old acks cannot undo newer intent"
        );
        assert_eq!(sole_edit(&db, 1), (third, watched(30)));
    }

    #[test]
    fn unwatched_last_clears_the_watched_date() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        db.queue_edit_mylist(1, &watched(10)).unwrap();
        db.queue_edit_mylist(1, &unwatched()).unwrap();
        assert_eq!(sole_edit(&db, 1).1, unwatched());
        let e = db.mylist_entry(1).unwrap().unwrap();
        assert_eq!((e.viewed, e.viewdate), (false, 0));
    }

    #[test]
    fn marking_watched_without_a_date_stamps_the_time_of_the_click() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        let before = format::now();
        db.queue_edit_mylist(
            1,
            &MylistEdit {
                viewed: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        let (_, edit) = sole_edit(&db, 1);
        let stamped = edit.viewdate.expect("watch time is recorded when queued");
        assert!(stamped >= before);
        assert_eq!(db.mylist_entry(1).unwrap().unwrap().viewdate, stamped);
    }

    #[test]
    fn partial_edits_merge_and_checked_empty_text_clears() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        db.queue_edit_mylist(
            1,
            &MylistEdit {
                state: Some(2),
                storage: Some("A".into()),
                ..Default::default()
            },
        )
        .unwrap();
        db.queue_edit_mylist(
            1,
            &MylistEdit {
                storage: Some(String::new()),
                other: Some("N".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let expected = MylistEdit {
            state: Some(2),
            storage: Some(String::new()),
            other: Some("N".into()),
            ..Default::default()
        };
        assert_eq!(sole_edit(&db, 1).1, expected);
        let e = db.mylist_entry(1).unwrap().unwrap();
        assert_eq!(e.state, 2);
        assert_eq!(e.storage.as_deref(), Some(""));
        assert_eq!(e.other.as_deref(), Some("N"));
        assert_eq!(e.source, None);
    }

    #[test]
    fn edits_of_different_entries_stay_separate() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        entry(&db, 2);
        db.queue_edit_mylist(1, &state(1)).unwrap();
        db.queue_edit_mylist(2, &state(2)).unwrap();
        db.queue_edit_mylist(1, &state(3)).unwrap();
        assert_eq!(db.queue_len().unwrap(), 2);
        assert_eq!(sole_edit(&db, 1).1, state(3));
        assert_eq!(sole_edit(&db, 2).1, state(2));
    }

    #[test]
    fn old_ack_after_queue_clear_cannot_remove_a_new_edit() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        let old = db.queue_edit_mylist(1, &state(1)).unwrap();
        db.queue_clear().unwrap();
        let new = db.queue_edit_mylist(1, &state(2)).unwrap();
        assert!(
            new > old,
            "ids are never reused, even after the queue empties"
        );
        db.queue_remove(old).unwrap();
        assert_eq!(sole_edit(&db, 1), (new, state(2)));
    }

    #[test]
    fn queued_edit_survives_a_restart() {
        let path = temp_db_path("restart");
        let previous = {
            let db = Db::open(&path).unwrap();
            entry(&db, 1);
            db.queue_edit_mylist(1, &watched(10)).unwrap();
            db.queue_edit_mylist(1, &state(3)).unwrap()
        };
        let db = Db::open(&path).unwrap();
        let (id, edit) = sole_edit(&db, 1);
        assert_eq!(id, previous);
        assert_eq!(
            edit,
            MylistEdit {
                state: Some(3),
                viewed: Some(true),
                viewdate: Some(10),
                ..Default::default()
            }
        );
        let next = db.queue_edit_mylist(1, &unwatched()).unwrap();
        assert!(next > previous, "ids keep growing across restarts");
        drop(db);
        remove_db_files(&path);
    }

    #[test]
    fn stale_remote_reply_cannot_clobber_pending_edit() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        db.queue_edit_mylist(
            1,
            &MylistEdit {
                state: Some(3),
                viewed: Some(true),
                viewdate: Some(50),
                storage: Some("Mine".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let mut remote = MylistEntry {
            lid: 1,
            fid: 101,
            state: 1,
            storage: Some("Old".into()),
            source: Some("Remote".into()),
            ..Default::default()
        };
        db.overlay_pending_mylist(&mut remote).unwrap();
        assert_eq!(remote.state, 3);
        assert_eq!((remote.viewed, remote.viewdate), (true, 50));
        assert_eq!(remote.storage.as_deref(), Some("Mine"));
        assert_eq!(
            remote.source.as_deref(),
            Some("Remote"),
            "untouched fields come from AniDB"
        );

        let mut unrelated = MylistEntry {
            lid: 2,
            state: 1,
            ..Default::default()
        };
        db.overlay_pending_mylist(&mut unrelated).unwrap();
        assert_eq!(unrelated.state, 1);
    }

    #[test]
    fn pending_intent_is_visible_and_a_pending_delete_wins_over_new_edits() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        assert!(!db.mylist_has_pending_intent(1).unwrap());
        db.queue_edit_mylist(1, &state(1)).unwrap();
        assert!(db.mylist_has_pending_intent(1).unwrap());
        db.queue_push(&Command::MylistDelete { lid: 2 }).unwrap();
        assert!(db.mylist_has_pending_intent(2).unwrap());
        db.queue_push(&Command::MylistRefresh { lid: 3 }).unwrap();
        assert!(
            !db.mylist_has_pending_intent(3).unwrap(),
            "a refresh is not a local change"
        );
        assert_eq!(db.queue_edit_mylist(2, &state(1)).unwrap(), 0);
        assert_eq!(db.queue_len().unwrap(), 3);
    }

    #[test]
    fn failed_edit_fields_are_preserved_when_a_new_edit_merges_them() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        db.queue_push(&Command::FetchAnime { aid: 1 }).unwrap();
        let failed = db
            .queue_edit_mylist(
                1,
                &MylistEdit {
                    storage: Some("Shelf".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        db.queue_block(failed, "illegal input").unwrap();
        let (_, next, _) = db.queue_peek().unwrap().unwrap();
        assert_eq!(
            next,
            Command::FetchAnime { aid: 1 },
            "blocked edit is skipped"
        );

        let mut remote = MylistEntry {
            lid: 1,
            ..Default::default()
        };
        db.overlay_pending_mylist(&mut remote).unwrap();
        assert_eq!(
            remote.storage.as_deref(),
            Some("Shelf"),
            "blocked intent still shows locally"
        );

        let fresh = db.queue_edit_mylist(1, &state(2)).unwrap();
        assert!(fresh > failed);
        let (id, edit) = sole_edit(&db, 1);
        assert_eq!(id, fresh);
        assert_eq!(
            edit,
            MylistEdit {
                state: Some(2),
                storage: Some("Shelf".into()),
                ..Default::default()
            }
        );
        let row = db
            .queue_list()
            .unwrap()
            .into_iter()
            .find(|e| e.id == fresh)
            .unwrap();
        assert!(!row.blocked, "the user's new edit makes it runnable again");
        assert_eq!(row.tries, 1, "the attempt history is kept");
        assert_eq!(row.last_error.as_deref(), Some("illegal input"));
        assert_eq!(db.queue_blocked_len().unwrap(), 0);
    }

    #[test]
    fn retry_merges_duplicate_edit_generations_into_the_latest() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        // Rows as written by builds that queued every edit separately.
        for s in [1, 2] {
            let cmd = Command::MylistEdit {
                lid: 1,
                edit: state(s),
            };
            db.conn()
                .execute(
                    "INSERT INTO command_queue(created, priority, payload, blocked, lid) VALUES (0, 0, ?1, 1, 1)",
                    [serde_json::to_string(&cmd).unwrap()],
                )
                .unwrap();
        }
        assert_eq!(db.queue_retry_failed().unwrap(), 2);
        assert_eq!(
            sole_edit(&db, 1).1,
            state(2),
            "an old edit must not overwrite the newer one"
        );
        assert_eq!(db.queue_blocked_len().unwrap(), 0);
        let (_, _, tries) = db.queue_peek().unwrap().unwrap();
        assert_eq!(tries, 0);
    }

    #[test]
    fn missing_entry_keeps_the_current_edit_and_the_local_entry() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        let id = db.queue_edit_mylist(1, &state(2)).unwrap();
        let err = edit_target_missing(&db, id, 1).unwrap_err();
        assert!(!err.transient && !err.fatal && err.retry_after.is_none());
        assert!(err.message.contains("no longer exists"));
        assert!(
            db.mylist_entry(1).unwrap().is_some(),
            "the local entry stays"
        );
        // The worker blocks it: the user's intent stays queued and visible.
        db.queue_block(id, &err.message).unwrap();
        assert_eq!(db.queue_blocked_len().unwrap(), 1);
        assert_eq!(sole_edit(&db, 1).1, state(2));
        let mut remote = MylistEntry {
            lid: 1,
            ..Default::default()
        };
        db.overlay_pending_mylist(&mut remote).unwrap();
        assert_eq!(remote.state, 2);
    }

    #[test]
    fn missing_entry_for_a_superseded_edit_changes_nothing() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        let old = db.queue_edit_mylist(1, &state(1)).unwrap();
        let new = db.queue_edit_mylist(1, &state(2)).unwrap();
        let outcome = edit_target_missing(&db, old, 1).unwrap();
        assert!(matches!(outcome, Outcome::NotFound(_)));
        assert!(db.mylist_entry(1).unwrap().is_some());
        assert_eq!(sole_edit(&db, 1), (new, state(2)));

        // An old edit whose queue was cleared meanwhile is just as harmless.
        db.queue_clear().unwrap();
        let outcome = edit_target_missing(&db, new, 1).unwrap();
        assert!(matches!(outcome, Outcome::NotFound(_)));
        assert!(db.mylist_entry(1).unwrap().is_some());
    }

    #[test]
    fn delete_reply_mirrors_the_removal_unless_newer_intent_was_queued() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        let current = db.queue_push(&Command::MylistDelete { lid: 1 }).unwrap();
        assert_eq!(
            mylist_deleted_on_anidb(&db, current, 1).unwrap(),
            Outcome::MylistDeleted
        );
        assert!(db.mylist_entry(1).unwrap().is_none());

        // The queue was cleared while the delete was in flight and the user edited again.
        entry(&db, 2);
        let old = db.queue_push(&Command::MylistDelete { lid: 2 }).unwrap();
        db.queue_clear().unwrap();
        let newer = db.queue_edit_mylist(2, &state(4)).unwrap();
        let outcome = mylist_deleted_on_anidb(&db, old, 2).unwrap();
        assert!(matches!(outcome, Outcome::Updated(_)));
        assert!(
            db.mylist_entry(2).unwrap().is_some(),
            "newer intent is not deleted"
        );
        assert_eq!(sole_edit(&db, 2), (newer, state(4)));

        // Cleared without anything newer: the local copy follows AniDB.
        entry(&db, 3);
        let old = db.queue_push(&Command::MylistDelete { lid: 3 }).unwrap();
        db.queue_clear().unwrap();
        assert_eq!(
            mylist_deleted_on_anidb(&db, old, 3).unwrap(),
            Outcome::MylistDeleted
        );
        assert!(db.mylist_entry(3).unwrap().is_none());
    }

    #[test]
    fn delete_supersedes_blocked_edits_and_retry_never_sends_an_edit_after_it() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        let edit = db.queue_edit_mylist(1, &state(2)).unwrap();
        db.queue_block(edit, "illegal input").unwrap();
        let delete = db.queue_push(&Command::MylistDelete { lid: 1 }).unwrap();
        assert!(delete > edit);
        assert!(
            !db.queue_contains(edit).unwrap(),
            "the delete replaced the moot edit"
        );
        assert_eq!(db.queue_len().unwrap(), 1);
        assert_eq!(
            db.queue_edit_mylist(1, &state(3)).unwrap(),
            0,
            "no edits after a delete"
        );
        assert_eq!(db.queue_push(&Command::MylistDelete { lid: 1 }).unwrap(), 0);

        db.queue_block(delete, "synthetic").unwrap();
        assert_eq!(db.queue_retry_failed().unwrap(), 1);
        let (id, cmd, _) = db.queue_peek().unwrap().unwrap();
        assert_eq!((id, cmd), (delete, Command::MylistDelete { lid: 1 }));
        assert_eq!(db.queue_len().unwrap(), 1);
    }

    #[test]
    fn retry_drops_legacy_edits_queued_before_a_later_delete() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        // Rows as written by builds that queued every command separately.
        for command in [
            Command::MylistEdit {
                lid: 1,
                edit: state(1),
            },
            Command::MylistEdit {
                lid: 1,
                edit: state(2),
            },
            Command::MylistDelete { lid: 1 },
        ] {
            db.conn()
                .execute(
                    "INSERT INTO command_queue(created, priority, payload, blocked, lid) VALUES (0, 0, ?1, 1, 1)",
                    [serde_json::to_string(&command).unwrap()],
                )
                .unwrap();
        }
        assert_eq!(db.queue_retry_failed().unwrap(), 3);
        let list = db.queue_list().unwrap();
        assert_eq!(
            list.len(),
            1,
            "edits must not be re-queued behind the delete"
        );
        assert_eq!(list[0].command, Some(Command::MylistDelete { lid: 1 }));
        assert!(!list[0].blocked);
    }

    #[test]
    fn queue_push_many_adds_in_one_go_and_skips_duplicates() {
        let db = Db::memory().unwrap();
        let cmds = [
            Command::FetchAnime { aid: 1 },
            Command::FetchAnime { aid: 2 },
            Command::FetchAnime { aid: 1 },
        ];
        assert_eq!(db.queue_push_many(&cmds).unwrap(), 2);
        assert_eq!(db.queue_len().unwrap(), 2);
    }

    #[test]
    fn merge_ignores_a_bare_date_after_unwatched() {
        let mut base = unwatched();
        merge_edit(
            &mut base,
            &MylistEdit {
                viewdate: Some(9),
                ..Default::default()
            },
        );
        assert_eq!(base, unwatched());
    }

    #[test]
    fn command_errors_carry_cooldown_and_blocking_information() {
        let banned = CommandError::from(AnidbError::Banned("synthetic".into()));
        assert!(banned.fatal);
        assert_eq!(banned.retry_after, Some(BAN_COOLDOWN));
        assert!(!banned.counts_attempt);

        let wait = Duration::from_secs(123);
        let unavailable = CommandError::from(AnidbError::ServerUnavailable(wait));
        assert_eq!(unavailable.retry_after, Some(wait));
        assert!(!unavailable.fatal && !unavailable.counts_attempt);

        let network = CommandError::from(AnidbError::Network("synthetic".into()));
        assert!(network.transient && !network.fatal && !network.counts_attempt);
        assert_eq!(network.retry_after, Some(TRANSIENT_COOLDOWN));

        for correctable in [
            AnidbError::Auth("synthetic".into()),
            AnidbError::ClientOutdated,
        ] {
            let e = CommandError::from(correctable);
            assert!(e.fatal && e.retry_after.is_none());
        }
        for permanent in [
            AnidbError::Protocol("synthetic".into()),
            AnidbError::IllegalInput("synthetic".into()),
            AnidbError::Xml("synthetic".into()),
            AnidbError::Unexpected(999, "synthetic".into()),
        ] {
            let e = CommandError::from(permanent);
            assert!(!e.transient && !e.fatal && e.retry_after.is_none());
        }
        assert!(CommandError::from(AnidbError::ServerError(600, String::new())).counts_attempt);

        let broken_xml = CommandError::from(Error::Xml("synthetic".into()));
        assert!(!broken_xml.transient && broken_xml.retry_after.is_none());
        let storage = CommandError::from(Error::Io(std::io::Error::other("synthetic")));
        assert!(storage.transient && storage.counts_attempt);
        assert_eq!(storage.retry_after, Some(TRANSIENT_COOLDOWN));
    }

    #[test]
    fn manager_rules_only_send_checked_fields() {
        let m = FileManager {
            mylist_state: 2,
            mylist_state_check: true,
            mylist_watched: true,
            mylist_watched_check: false,
            mylist_storage: "Shelf".into(),
            mylist_storage_check: true,
            ..FileManager::default()
        };
        let e = manager_mylist_edit(&m);
        assert_eq!(e.state, Some(2));
        assert_eq!(e.viewed, None);
        assert_eq!(e.storage.as_deref(), Some("Shelf"));
        assert_eq!(e.source, None);
    }

    #[test]
    fn checked_empty_manager_fields_clear_existing_mylist_values() {
        let db = Db::memory().unwrap();
        db.upsert_mylist(&MylistEntry {
            lid: 1,
            fid: 2,
            storage: Some("Synthetic shelf".into()),
            source: Some("Synthetic source".into()),
            other: Some("Synthetic note".into()),
            ..Default::default()
        })
        .unwrap();
        let manager = FileManager {
            mylist_storage_check: true,
            mylist_source_check: true,
            mylist_other_check: true,
            ..Default::default()
        };
        db.queue_push(&Command::MylistEdit {
            lid: 1,
            edit: manager_mylist_edit(&manager),
        })
        .unwrap();
        let (_, edit) = sole_edit(&db, 1);
        assert_eq!(edit.storage.as_deref(), Some(""));
        assert_eq!(edit.source.as_deref(), Some(""));
        assert_eq!(edit.other.as_deref(), Some(""));
        let entry = db.mylist_entry(1).unwrap().unwrap();
        assert_eq!(entry.storage.as_deref(), Some(""));
        assert_eq!(entry.source.as_deref(), Some(""));
        assert_eq!(entry.other.as_deref(), Some(""));
    }

    #[test]
    fn local_edit_sets_and_clears_watched_date() {
        let db = Db::memory().unwrap();
        db.upsert_mylist(&MylistEntry {
            lid: 1,
            fid: 2,
            ..Default::default()
        })
        .unwrap();
        apply_edit_locally(&db, 1, &watched(42)).unwrap();
        let e = db.mylist_entry(1).unwrap().unwrap();
        assert_eq!((e.viewed, e.viewdate), (true, 42));
        apply_edit_locally(
            &db,
            1,
            &MylistEdit {
                viewed: Some(false),
                state: Some(3),
                ..Default::default()
            },
        )
        .unwrap();
        let e = db.mylist_entry(1).unwrap().unwrap();
        assert_eq!((e.viewed, e.viewdate, e.state), (false, 0, 3));
    }

    #[test]
    fn entry_editor_edits_merge_clear_fields_and_keep_the_view_date() {
        let db = Db::memory().unwrap();
        db.upsert_mylist(&MylistEntry {
            lid: 1,
            fid: 2,
            storage: Some("Shelf".into()),
            source: Some("Retail".into()),
            other: Some("Note".into()),
            ..Default::default()
        })
        .unwrap();
        // Checked and empty clears the field; unchecked fields stay out of the edit.
        db.queue_edit_mylist(
            1,
            &MylistEdit {
                storage: Some(String::new()),
                ..Default::default()
            },
        )
        .unwrap();
        // A watched date edit merges with the pending storage edit instead of replacing it.
        db.queue_edit_mylist(
            1,
            &MylistEdit {
                state: Some(2),
                viewed: Some(true),
                viewdate: Some(1_700_000_000),
                other: Some("Revised".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let (_, edit) = sole_edit(&db, 1);
        assert_eq!(
            edit,
            MylistEdit {
                state: Some(2),
                viewed: Some(true),
                viewdate: Some(1_700_000_000),
                storage: Some(String::new()),
                source: None,
                other: Some("Revised".into()),
            }
        );
        let local = db.mylist_entry(1).unwrap().unwrap();
        assert_eq!(local.storage.as_deref(), Some(""));
        assert_eq!(local.source.as_deref(), Some("Retail"), "left unchanged");
        assert_eq!(local.other.as_deref(), Some("Revised"));
        assert_eq!((local.viewed, local.viewdate), (true, 1_700_000_000));

        // Correcting only the date of an entry that is watched keeps it watched.
        db.queue_edit_mylist(
            1,
            &MylistEdit {
                viewed: Some(true),
                viewdate: Some(1_700_100_000),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(sole_edit(&db, 1).1.viewdate, Some(1_700_100_000));
        assert_eq!(db.mylist_entry(1).unwrap().unwrap().viewdate, 1_700_100_000);

        // Marking it unwatched drops the date locally and in the queued edit.
        db.queue_edit_mylist(1, &unwatched()).unwrap();
        let (_, edit) = sole_edit(&db, 1);
        assert_eq!((edit.viewed, edit.viewdate), (Some(false), None));
        let local = db.mylist_entry(1).unwrap().unwrap();
        assert_eq!((local.viewed, local.viewdate), (false, 0));
        assert_eq!(edit.storage.as_deref(), Some(""), "earlier fields survive");
    }

    #[test]
    fn queued_add_survives_a_restart_and_is_not_queued_twice() {
        let path = temp_db_path("add-restart");
        let add = Command::MylistAdd {
            file: FileRef::Hash {
                size: 1234,
                ed2k: "00112233445566778899aabbccddeeff".into(),
            },
            edit: MylistEdit {
                state: Some(1),
                viewed: Some(true),
                viewdate: Some(10),
                storage: Some("Shelf".into()),
                other: Some(String::new()),
                ..Default::default()
            },
        };
        let id = {
            let db = Db::open(&path).unwrap();
            let id = db.queue_push(&add).unwrap();
            assert_ne!(id, 0);
            assert_eq!(
                db.queue_push(&add).unwrap(),
                0,
                "an identical add is skipped"
            );
            id
        };
        let db = Db::open(&path).unwrap();
        assert_eq!(db.queue_peek().unwrap(), Some((id, add.clone(), 0)));
        assert_eq!(db.queue_len().unwrap(), 1);
        // An add runs before lookups and bulk refreshes.
        db.queue_push(&Command::FetchAnime { aid: 1 }).unwrap();
        assert_eq!(db.queue_peek().unwrap().unwrap().1, add);
        drop(db);
        remove_db_files(&path);
    }

    #[test]
    fn removal_keeps_the_row_until_the_acknowledgement_and_a_blocked_one_stays_queued() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        let delete = db.queue_push(&Command::MylistDelete { lid: 1 }).unwrap();
        assert!(
            db.mylist_entry(1).unwrap().is_some(),
            "queueing the removal leaves the local row"
        );
        // The command fails: it is blocked, still listed, and the row is untouched.
        db.queue_block(delete, "synthetic failure").unwrap();
        assert!(db.mylist_entry(1).unwrap().is_some());
        let listed = db.queue_list().unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].blocked);
        assert_eq!(listed[0].last_error.as_deref(), Some("synthetic failure"));
        // Only the acknowledgement removes the row.
        assert_eq!(
            mylist_deleted_on_anidb(&db, delete, 1).unwrap(),
            Outcome::MylistDeleted
        );
        assert!(db.mylist_entry(1).unwrap().is_none());
    }

    #[test]
    fn an_edit_after_an_in_flight_removal_wins_in_queue_order() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        let delete = db.queue_push(&Command::MylistDelete { lid: 1 }).unwrap();
        // Still queued: the removal is the latest intent and refuses the edit.
        assert_eq!(db.queue_edit_mylist(1, &state(2)).unwrap(), 0);
        // The removal left the queue (cleared while in flight), then the user edited again.
        db.queue_clear().unwrap();
        let edit = db.queue_edit_mylist(1, &state(2)).unwrap();
        assert!(edit > delete);
        let outcome = mylist_deleted_on_anidb(&db, delete, 1).unwrap();
        assert!(matches!(outcome, Outcome::Updated(_)));
        assert_eq!(
            db.mylist_entry(1).unwrap().unwrap().state,
            2,
            "the newer edit keeps the entry and its local state"
        );
        assert_eq!(sole_edit(&db, 1), (edit, state(2)));
    }

    #[test]
    fn refresh_commands_are_queued_once_and_never_count_as_local_intent() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        let commands = [
            Command::MylistRefresh { lid: 1 },
            Command::FetchEpisode { eid: 10 },
            Command::FetchGroup { gid: 7 },
        ];
        assert_eq!(db.queue_push_many(&commands).unwrap(), 3);
        assert_eq!(db.queue_push_many(&commands).unwrap(), 0);
        let listed: Vec<_> = db
            .queue_list()
            .unwrap()
            .into_iter()
            .map(|e| e.description)
            .collect();
        assert_eq!(
            listed,
            [
                "Fetch episode 10",
                "Fetch group 7",
                "Refresh mylist entry 1"
            ]
        );
        assert!(!db.mylist_has_pending_intent(1).unwrap());
    }

    #[test]
    fn refreshed_entry_data_replaces_the_cache_but_keeps_pending_edits() {
        let db = Db::memory().unwrap();
        entry(&db, 1);
        db.queue_edit_mylist(
            1,
            &MylistEdit {
                state: Some(3),
                storage: Some("Mine".into()),
                ..Default::default()
            },
        )
        .unwrap();
        db.queue_push(&Command::MylistRefresh { lid: 1 }).unwrap();
        db.store_api_mylist(&animatic_anidb::commands::MylistEntry {
            lid: 1,
            fid: 101,
            state: 1,
            storage: Some("Old".into()),
            source: Some("Fresh source".into()),
            ..Default::default()
        })
        .unwrap();
        let e = db.mylist_entry(1).unwrap().unwrap();
        assert_eq!(e.state, 3, "pending state edit wins");
        assert_eq!(e.storage.as_deref(), Some("Mine"));
        assert_eq!(
            e.source.as_deref(),
            Some("Fresh source"),
            "the rest is refreshed"
        );
    }

    #[test]
    fn account_statistics_are_stored_with_their_time_and_reload_after_a_restart() {
        let path = temp_db_path("stats");
        let stats = commands::MylistStats {
            animes: 12,
            episodes: 345,
            files: 678,
            size_mb: 9_000,
            viewed_episodes: 300,
            ..Default::default()
        };
        {
            let db = Db::open(&path).unwrap();
            assert_eq!(db.mylist_stats_snapshot().unwrap(), None);
            db.store_mylist_stats(&commands::MylistStats::default(), 1)
                .unwrap();
            db.store_mylist_stats(&stats, 1_700_000_000).unwrap();
        }
        let db = Db::open(&path).unwrap();
        let stored = db.mylist_stats_snapshot().unwrap().unwrap();
        assert_eq!(
            stored.fetched, 1_700_000_000,
            "the latest result replaces the old one"
        );
        assert_eq!(stored.stats, stats);
        // An unreadable value is no statistics, not a failure of the whole library view.
        db.set_meta(crate::store::MYLIST_STATS_KEY, "not json")
            .unwrap();
        assert_eq!(db.mylist_stats_snapshot().unwrap(), None);
        assert!(
            Outcome::Stats(stats).changes_library(),
            "the library view reloads to show them"
        );
        drop(db);
        remove_db_files(&path);
    }
}
