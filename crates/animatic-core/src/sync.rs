//! AniDB commands queued by the UI and executed by the network worker.
//!
//! Commands are persisted in `command_queue` so work done offline (or interrupted by
//! a restart) is replayed once AniDB is reachable again — the legacy "Work offline" mode.

use animatic_anidb::AnidbError;
use animatic_anidb::commands::{self, FileRef, MylistAddResult, MylistEdit};
use animatic_anidb::http::HttpClient;
use animatic_anidb::udp::UdpClient;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::{Error, Result};
use crate::format;
use crate::settings::FileManager;
pub type QueueEntry = (i64, Command, i64, Option<String>);

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
            Command::FetchAnime { .. } | Command::MylistStats | Command::TitlesDump => 3,
            Command::MylistRefresh { .. } => 4,
        }
    }

    pub fn uses_http(&self) -> bool {
        matches!(self, Command::FetchAnime { .. } | Command::TitlesDump)
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
            Outcome::NotFound(s) => s.clone(),
        }
    }

    /// True when the change affects what the library views display.
    pub fn changes_library(&self) -> bool {
        !matches!(
            self,
            Outcome::Stats(_) | Outcome::Unknown | Outcome::NotFound(_)
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
        storage: (m.mylist_storage_check && !m.mylist_storage.is_empty())
            .then(|| m.mylist_storage.clone()),
        source: (m.mylist_source_check && !m.mylist_source.is_empty())
            .then(|| m.mylist_source.clone()),
        other: (m.mylist_other_check && !m.mylist_other.is_empty()).then(|| m.mylist_other.clone()),
    }
}

/// Apply an edit to the local copy of a mylist entry (optimistic update, then confirmed by AniDB).
pub fn apply_edit_locally(db: &Db, lid: i64, edit: &MylistEdit) -> Result<()> {
    if let Some(mut e) = db.mylist_entry(lid)? {
        if let Some(s) = edit.state {
            e.state = s;
        }
        if let Some(v) = edit.viewed {
            e.viewed = v;
            e.viewdate = if v {
                edit.viewdate.unwrap_or_else(format::now)
            } else {
                0
            };
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
        db.upsert_mylist(&e)?;
    }
    Ok(())
}

/// Persistent queue operations.
impl Db {
    pub fn queue_push(&self, cmd: &Command) -> Result<i64> {
        let payload = serde_json::to_string(cmd)?;
        let dup: i64 = self.conn().query_row(
            "SELECT count(*) FROM command_queue WHERE payload = ?1",
            [&payload],
            |r| r.get(0),
        )?;
        if dup > 0 {
            return Ok(0);
        }
        self.conn().execute(
            "INSERT INTO command_queue(created, priority, payload) VALUES (?1, ?2, ?3)",
            params![format::now(), cmd.priority(), payload],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    /// Next command (lowest priority value, oldest first).
    pub fn queue_peek(&self) -> Result<Option<(i64, Command, i64)>> {
        let row: Option<(i64, String, i64)> = self
            .conn()
            .query_row(
                "SELECT id, payload, tries FROM command_queue ORDER BY priority, id LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        match row {
            Some((id, payload, tries)) => match serde_json::from_str(&payload) {
                Ok(cmd) => Ok(Some((id, cmd, tries))),
                Err(e) => {
                    log::warn!("dropping unreadable queued command {id}: {e}");
                    self.queue_remove(id)?;
                    self.queue_peek()
                }
            },
            None => Ok(None),
        }
    }

    pub fn queue_remove(&self, id: i64) -> Result<()> {
        self.conn()
            .execute("DELETE FROM command_queue WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn queue_failed(&self, id: i64, err: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE command_queue SET tries = tries + 1, last_error = ?2, priority = priority + 10 WHERE id = ?1",
            params![id, err],
        )?;
        Ok(())
    }

    pub fn queue_len(&self) -> Result<i64> {
        Ok(self
            .conn()
            .query_row("SELECT count(*) FROM command_queue", [], |r| r.get(0))?)
    }

    pub fn queue_list(&self) -> Result<Vec<QueueEntry>> {
        let mut st = self.conn().prepare(
            "SELECT id, payload, tries, last_error FROM command_queue ORDER BY priority, id",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get(2)?,
                r.get(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, p, t, e) = r?;
            if let Ok(c) = serde_json::from_str(&p) {
                out.push((id, c, t, e));
            }
        }
        Ok(out)
    }

    pub fn queue_clear(&self) -> Result<()> {
        self.conn().execute("DELETE FROM command_queue", [])?;
        Ok(())
    }
}

/// Error wrapper keeping the information whether a retry later makes sense.
#[derive(Debug)]
pub struct CommandError {
    pub message: String,
    pub transient: bool,
    /// AniDB refuses the client (banned/outdated/bad credentials): stop the queue.
    pub fatal: bool,
}

impl From<AnidbError> for CommandError {
    fn from(e: AnidbError) -> Self {
        CommandError {
            transient: e.is_transient(),
            fatal: matches!(
                e,
                AnidbError::Banned(_) | AnidbError::ClientOutdated | AnidbError::Auth(_)
            ),
            message: e.to_string(),
        }
    }
}

impl From<Error> for CommandError {
    fn from(e: Error) -> Self {
        CommandError {
            message: e.to_string(),
            transient: false,
            fatal: false,
        }
    }
}

/// Execute one command. Network calls are rate limited by the clients.
pub fn execute(
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
                MylistAddResult::NoSuchEntry => {
                    db.delete_mylist(*lid)?;
                    Ok(Outcome::NotFound(format!(
                        "mylist entry {lid} no longer exists on AniDB"
                    )))
                }
                other => Ok(Outcome::NotFound(format!(
                    "unexpected mylist reply {other:?}"
                ))),
            }
        }
        Command::MylistDelete { lid } => {
            commands::mylist_del(udp, &FileRef::Lid(*lid))?;
            db.delete_mylist(*lid)?;
            Ok(Outcome::MylistDeleted)
        }
        Command::MylistRefresh { lid } => match commands::mylist(udp, &FileRef::Lid(*lid))? {
            Some(m) => {
                db.store_api_mylist(&m)?;
                Ok(Outcome::Updated(format!("mylist entry {lid} refreshed")))
            }
            None => {
                db.delete_mylist(*lid)?;
                Ok(Outcome::Updated(format!(
                    "mylist entry {lid} was removed on AniDB"
                )))
            }
        },
        Command::FetchAnime { aid } => {
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
        Command::FetchFile { fid } => match commands::file_by_fid(udp, *fid)? {
            Some(info) => {
                db.store_file_info(&info)?;
                Ok(Outcome::Updated(format!("file {fid} updated")))
            }
            None => Ok(Outcome::NotFound(format!("no file {fid}"))),
        },
        Command::MylistStats => Ok(Outcome::Stats(commands::mylist_stats(udp)?)),
        Command::TitlesDump => {
            let mut titles = Vec::new();
            http.titles_dump(|t| titles.push(t))?;
            let added = db.store_title_dump(&titles)?;
            db.set_meta("titles_dump_date", &format::now().to_string())?;
            Ok(Outcome::Updated(format!(
                "{} titles downloaded ({added} new)",
                titles.len()
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

    #[test]
    fn queue_orders_by_priority_and_skips_duplicates() {
        let db = Db::memory().unwrap();
        db.queue_push(&Command::FetchAnime { aid: 1 }).unwrap();
        db.queue_push(&Command::MylistDelete { lid: 5 }).unwrap();
        assert_eq!(db.queue_push(&Command::FetchAnime { aid: 1 }).unwrap(), 0);
        assert_eq!(db.queue_len().unwrap(), 2);
        let (id, cmd, _) = db.queue_peek().unwrap().unwrap();
        assert_eq!(cmd, Command::MylistDelete { lid: 5 });
        db.queue_failed(id, "timeout").unwrap();
        assert_eq!(
            db.queue_peek().unwrap().unwrap().1,
            Command::FetchAnime { aid: 1 },
            "failed command is demoted"
        );
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
    fn local_edit_sets_and_clears_watched_date() {
        let db = Db::memory().unwrap();
        db.upsert_mylist(&crate::library::MylistEntry {
            lid: 1,
            fid: 2,
            ..Default::default()
        })
        .unwrap();
        apply_edit_locally(
            &db,
            1,
            &MylistEdit {
                viewed: Some(true),
                viewdate: Some(42),
                ..Default::default()
            },
        )
        .unwrap();
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
}
