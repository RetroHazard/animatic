//! Write side: persisting AniDB replies, mylist changes and hashed files.

use animatic_anidb::commands::{
    AnimeInfo, EpisodeInfo, FileInfo, GroupInfo, MylistEntry as ApiMylistEntry, MylistStats,
};
use animatic_anidb::http::{AnimeXml, DumpTitle};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::db::Db;
use crate::error::{Error, Result};
use crate::format;
use crate::hash::Hashes;
use crate::library::{MylistEntry, anime_type_id, relation_id, title_kind_id};

/// Metadata key holding the completion time of the last successful titles dump import.
pub const TITLES_DUMP_DATE_KEY: &str = "titles_dump_date";

/// Metadata key holding the latest MYLISTSTATS result as JSON ([`StoredMylistStats`]).
pub const MYLIST_STATS_KEY: &str = "mylist_stats";

/// The latest MyList statistics AniDB reported for the account, with the time they were
/// fetched. These describe the whole AniDB account, a different population from the local
/// library tables, so they are kept apart from them and never mixed into local counts.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredMylistStats {
    /// Unix time the statistics were received.
    pub fetched: i64,
    pub stats: MylistStats,
}

/// What a titles dump import read and what it newly stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TitleImport {
    /// Titles in the dump.
    pub titles: usize,
    /// Titles that were not stored yet.
    pub added: usize,
}

impl Db {
    fn category_or_insert(&self, kind: &str, name: &str) -> Result<i64> {
        if name.trim().is_empty() {
            return Ok(1);
        }
        if let Some(id) = self.category_id_by_name(kind, name)? {
            return Ok(id);
        }
        let next: i64 = self.conn().query_row(
            "SELECT COALESCE(max(id), 1) + 1 FROM category WHERE kind = ?1",
            [kind],
            |r| r.get(0),
        )?;
        self.conn().execute(
            "INSERT INTO category(kind, id, name, is_default, sort_order) VALUES (?1, ?2, ?3, 0, ?2)",
            params![kind, next, name],
        )?;
        Ok(next)
    }

    fn set_anime_title(&self, aid: i64, name: &str, kind: i64, lang: &str) -> Result<()> {
        if name.is_empty() {
            return Ok(());
        }
        let exists: i64 = self.conn().query_row(
            "SELECT count(*) FROM anime_title WHERE aid = ?1 AND name = ?2 AND lang = ?3",
            params![aid, name, lang],
            |r| r.get(0),
        )?;
        if exists == 0 {
            if kind == 1 {
                self.conn().execute(
                    "UPDATE anime_title SET type = 2 WHERE aid = ?1 AND type = 1",
                    [aid],
                )?;
            }
            self.conn().execute(
                "INSERT INTO anime_title(aid, name, type, lang) VALUES (?1, ?2, ?3, ?4)",
                params![aid, name, kind, lang],
            )?;
        } else if kind == 1 {
            self.conn().execute(
                "UPDATE anime_title SET type = CASE WHEN name = ?2 AND lang = ?3 THEN 1 WHEN type = 1 THEN 2 ELSE type END
                 WHERE aid = ?1",
                params![aid, name, lang],
            )?;
        }
        Ok(())
    }

    fn set_episode_title(&self, eid: i64, name: &str, lang: &str) -> Result<()> {
        if name.is_empty() {
            return Ok(());
        }
        let n = self.conn().execute(
            "UPDATE episode_title SET name = ?2 WHERE eid = ?1 AND lang = ?3",
            params![eid, name, lang],
        )?;
        if n == 0 {
            self.conn().execute(
                "INSERT INTO episode_title(eid, name, lang) VALUES (?1, ?2, ?3)",
                params![eid, name, lang],
            )?;
        }
        Ok(())
    }

    fn ensure_anime_row(&self, aid: i64) -> Result<()> {
        self.conn()
            .execute("INSERT OR IGNORE INTO anime(aid) VALUES (?1)", [aid])?;
        Ok(())
    }

    /// Store everything a FILE reply carries, atomically (write lock up front; joins a caller's
    /// transaction through a savepoint).
    pub fn store_file_info(&self, i: &FileInfo) -> Result<()> {
        self.write_tx(|| self.store_file_info_rows(i))
    }

    fn store_file_info_rows(&self, i: &FileInfo) -> Result<()> {
        let now = format::now();
        let c = self.conn();
        let quality = self.category_or_insert("qual", i.quality.as_deref().unwrap_or(""))?;
        let source = self.category_or_insert("type", i.source.as_deref().unwrap_or(""))?;
        c.execute(
            "INSERT INTO file(fid, aid, eid, gid, size, ed2k, md5, sha1, crc32, filetype, quality_id, source_id, state,
                length, deprecated, anidb_filename, video_codec, video_bitrate, resolution, audio_codecs, audio_bitrates,
                dub_langs, sub_langs, description, released, updated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23,
                ?24, ?25, ?26)
             ON CONFLICT(fid) DO UPDATE SET aid = CASE WHEN excluded.aid > 0 THEN excluded.aid ELSE aid END,
               eid = CASE WHEN excluded.eid > 0 THEN excluded.eid ELSE eid END,
               gid = CASE WHEN excluded.gid > 0 THEN excluded.gid ELSE gid END,
               size = CASE WHEN excluded.size > 0 THEN excluded.size ELSE size END,
               ed2k = COALESCE(excluded.ed2k, ed2k), md5 = COALESCE(excluded.md5, md5), sha1 = COALESCE(excluded.sha1, sha1),
                crc32 = COALESCE(excluded.crc32, crc32), filetype = COALESCE(excluded.filetype, filetype),
                quality_id = excluded.quality_id, source_id = excluded.source_id, state = excluded.state,
                length = excluded.length, deprecated = excluded.deprecated, anidb_filename = excluded.anidb_filename,
                video_codec = excluded.video_codec, video_bitrate = excluded.video_bitrate, resolution = excluded.resolution,
                audio_codecs = excluded.audio_codecs, audio_bitrates = excluded.audio_bitrates, dub_langs = excluded.dub_langs,
                sub_langs = excluded.sub_langs, description = excluded.description,
                released = CASE WHEN excluded.released > 0 THEN excluded.released ELSE released END, updated = excluded.updated",
            params![
                i.fid,
                i.aid,
                i.eid,
                i.gid,
                i.size,
                i.ed2k,
                i.md5,
                i.sha1,
                i.crc32,
                i.filetype,
                quality,
                source,
                i.state,
                i.length,
                i.deprecated,
                i.anidb_filename,
                i.video_codec,
                i.video_bitrate,
                i.resolution,
                (!i.audio_codecs.is_empty()).then(|| i.audio_codecs.join(", ")),
                (!i.audio_bitrates.is_empty()).then(|| i.audio_bitrates.join(", ")),
                (!i.dub_langs.is_empty()).then(|| i.dub_langs.join(", ")),
                (!i.sub_langs.is_empty()).then(|| i.sub_langs.join(", ")),
                i.description,
                i.aired,
                now
            ],
        )?;
        c.execute("DELETE FROM file_episode WHERE fid = ?1", [i.fid])?;
        for (eid, pct) in &i.other_episodes {
            c.execute(
                "INSERT INTO file_episode(fid, eid, start_pct, end_pct, added) VALUES (?1, ?2, 0, ?3, ?4)",
                params![i.fid, eid, pct, now],
            )?;
        }
        if i.aid != 0 {
            self.ensure_anime_row(i.aid)?;
            c.execute(
                "UPDATE anime SET episodes = CASE WHEN ?2 > 0 THEN ?2 ELSE episodes END,
                    highest_ep = max(highest_ep, ?3), year = COALESCE(?4, year),
                    type_id = CASE WHEN ?5 > 1 THEN ?5 ELSE type_id END,
                    updated = max(updated, ?6) WHERE aid = ?1",
                params![
                    i.aid,
                    i.anime_episodes,
                    i.anime_highest_ep,
                    i.anime_year,
                    i.anime_type.as_deref().map(anime_type_id).unwrap_or(1),
                    i.anime_updated
                ],
            )?;
            if let Some(n) = &i.anime_romaji {
                self.set_anime_title(i.aid, n, 1, "x-jat")?;
            }
            if let Some(n) = &i.anime_kanji {
                self.set_anime_title(i.aid, n, 4, "ja")?;
            }
            if let Some(n) = &i.anime_english {
                self.set_anime_title(i.aid, n, 4, "en")?;
            }
            for n in &i.anime_synonyms {
                self.set_anime_title(i.aid, n, 2, "x-unk")?;
            }
            for n in &i.anime_short {
                self.set_anime_title(i.aid, n, 3, "x-unk")?;
            }
        }
        if i.eid != 0 {
            c.execute(
                "INSERT INTO episode(eid, aid, epno, type, rating, votes, length) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(eid) DO UPDATE SET aid = excluded.aid, epno = excluded.epno, type = excluded.type,
                   rating = excluded.rating, votes = excluded.votes,
                   length = CASE WHEN length = 0 THEN excluded.length ELSE length END",
                params![
                    i.eid,
                    i.aid,
                    i.epno.number,
                    i.epno.kind.max(1),
                    i.ep_rating,
                    i.ep_votes,
                    if i.length > 0 { (i.length + 30) / 60 } else { 0 }
                ],
            )?;
            if let Some(n) = &i.ep_name {
                self.set_episode_title(i.eid, n, "en")?;
            }
            if let Some(n) = &i.ep_romaji {
                self.set_episode_title(i.eid, n, "x-jat")?;
            }
            if let Some(n) = &i.ep_kanji {
                self.set_episode_title(i.eid, n, "ja")?;
            }
        }
        if i.gid != 0 {
            c.execute(
                "INSERT INTO grp(gid, name, short) VALUES (?1, ?2, ?3)
                 ON CONFLICT(gid) DO UPDATE SET name = COALESCE(excluded.name, name), short = COALESCE(excluded.short, short)",
                params![i.gid, i.group_name, i.group_short],
            )?;
        }
        match &i.mylist {
            Some(m) => self.store_api_mylist(m)?,
            None if i.mylist_absent => {
                // AniDB explicitly reported "not in mylist" (lid 0): drop stale local entries, but
                // never one with queued local intent. An omitted lid column is not this case.
                let lids: Vec<i64> = {
                    let mut st = c.prepare("SELECT lid FROM mylist WHERE fid = ?1")?;
                    st.query_map([i.fid], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?
                };
                for lid in lids {
                    if !self.mylist_has_pending_intent(lid)? {
                        c.execute("DELETE FROM mylist WHERE lid = ?1", [lid])?;
                    }
                }
            }
            None => {}
        }
        Ok(())
    }

    /// Persist an authoritative API mylist entry. The latest queued local intent for the lid is
    /// re-applied on top, so a stale in-flight reply cannot revert a newer pending edit.
    pub fn store_api_mylist(&self, m: &ApiMylistEntry) -> Result<()> {
        self.write_tx(|| self.store_api_mylist_rows(m))
    }

    fn store_api_mylist_rows(&self, m: &ApiMylistEntry) -> Result<()> {
        let fid_gid: Option<(i64, i64, i64)> = self
            .conn()
            .query_row(
                "SELECT eid, aid, gid FROM file WHERE fid = ?1",
                [m.fid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (eid, aid, gid) = match fid_gid {
            Some((e, a, g)) => (
                if m.eid != 0 { m.eid } else { e },
                if m.aid != 0 { m.aid } else { a },
                if m.gid != 0 { m.gid } else { g },
            ),
            None => (m.eid, m.aid, m.gid),
        };
        let mut entry = MylistEntry {
            lid: m.lid,
            fid: m.fid,
            eid,
            aid,
            gid,
            date: m.date,
            state: m.state,
            viewed: m.viewdate != 0,
            viewdate: m.viewdate,
            storage: m.storage.clone(),
            source: m.source.clone(),
            other: m.other.clone(),
            filestate: m.filestate,
        };
        self.overlay_pending_mylist(&mut entry)?;
        self.upsert_mylist(&entry)
    }

    pub fn upsert_mylist(&self, e: &MylistEntry) -> Result<()> {
        self.conn().execute(
            "INSERT INTO mylist(lid, fid, eid, aid, gid, date, state, viewed, viewdate, storage, source, other, filestate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(lid) DO UPDATE SET fid = excluded.fid, eid = excluded.eid, aid = excluded.aid, gid = excluded.gid,
               date = CASE WHEN excluded.date > 0 THEN excluded.date ELSE date END, state = excluded.state,
               viewed = excluded.viewed, viewdate = excluded.viewdate, storage = excluded.storage, source = excluded.source,
               other = excluded.other, filestate = excluded.filestate",
            params![
                e.lid, e.fid, e.eid, e.aid, e.gid, e.date, e.state, e.viewed, e.viewdate, e.storage, e.source, e.other,
                e.filestate
            ],
        )?;
        Ok(())
    }

    pub fn delete_mylist(&self, lid: i64) -> Result<()> {
        self.conn()
            .execute("DELETE FROM mylist WHERE lid = ?1", [lid])?;
        Ok(())
    }

    pub fn store_anime_info(&self, a: &AnimeInfo) -> Result<()> {
        self.ensure_anime_row(a.aid)?;
        self.conn().execute(
            "UPDATE anime SET year = COALESCE(?2, year), type_id = CASE WHEN ?3 > 1 THEN ?3 ELSE type_id END,
               episodes = ?4, highest_ep = ?5, special_eps = ?6, air_date = ?7, end_date = ?8, url = COALESCE(?9, url),
               picname = COALESCE(?10, picname), rating = ?11, votes = ?12, temp_rating = ?13, temp_votes = ?14,
               review_rating = ?15, reviews = ?16, restricted = ?17, ann_id = ?18, allcinema_id = ?19, updated = ?20
             WHERE aid = ?1",
            params![
                a.aid,
                a.year,
                a.type_name.as_deref().map(anime_type_id).unwrap_or(1),
                a.episodes,
                a.highest_ep,
                a.special_eps,
                a.air_date,
                a.end_date,
                a.url,
                a.picname,
                a.rating,
                a.votes,
                a.temp_rating,
                a.temp_votes,
                a.review_rating,
                a.reviews,
                a.restricted,
                a.ann_id,
                a.allcinema_id,
                a.updated
            ],
        )?;
        if let Some(n) = &a.romaji {
            self.set_anime_title(a.aid, n, 1, "x-jat")?;
        }
        if let Some(n) = &a.kanji {
            self.set_anime_title(a.aid, n, 4, "ja")?;
        }
        if let Some(n) = &a.english {
            self.set_anime_title(a.aid, n, 4, "en")?;
        }
        for n in &a.synonyms {
            self.set_anime_title(a.aid, n, 2, "x-unk")?;
        }
        for n in &a.short {
            self.set_anime_title(a.aid, n, 3, "x-unk")?;
        }
        if !a.related.is_empty() {
            self.conn()
                .execute("DELETE FROM anime_relation WHERE aid = ?1", [a.aid])?;
            for (rid, t) in &a.related {
                self.conn().execute(
                    "INSERT INTO anime_relation(aid, related_aid, type) VALUES (?1, ?2, ?3)",
                    params![a.aid, rid, t],
                )?;
            }
        }
        Ok(())
    }

    /// Store a full HTTP API anime record (titles, tags, relations, episodes).
    pub fn store_anime_xml(&self, a: &AnimeXml) -> Result<()> {
        let tx = self.conn().unchecked_transaction()?;
        self.ensure_anime_row(a.aid)?;
        let year = match (a.start_date.as_deref(), a.end_date.as_deref()) {
            (Some(s), Some(e)) if s.get(..4) != e.get(..4) && e.len() >= 4 => {
                Some(format!("{}-{}", &s[..4.min(s.len())], &e[..4]))
            }
            (Some(s), _) if s.len() >= 4 => Some(s[..4].to_string()),
            _ => None,
        };
        let highest = a
            .episodes
            .iter()
            .filter(|e| e.epno.kind == 1)
            .map(|e| i64::from(e.epno.number))
            .max()
            .unwrap_or(0);
        let specials = a.episodes.iter().filter(|e| e.epno.kind == 2).count() as i64;
        let creators = a
            .creators
            .iter()
            .filter(|(role, _)| role.contains("Animation Work") || role.contains("Work"))
            .map(|(_, n)| n.clone())
            .collect::<Vec<_>>()
            .join(", ");
        self.conn().execute(
            "UPDATE anime SET type_id = CASE WHEN ?2 > 1 THEN ?2 ELSE type_id END, episodes = ?3, air_date = ?4,
               end_date = ?5, year = COALESCE(?6, year), url = COALESCE(?7, url), description = COALESCE(?8, description),
               picname = COALESCE(?9, picname), rating = ?10, votes = ?11, temp_rating = ?12, temp_votes = ?13,
               review_rating = ?14, reviews = ?15, restricted = ?16, highest_ep = max(highest_ep, ?17), special_eps = ?18,
               producer = COALESCE(NULLIF(?19, ''), producer), fetched = ?20
             WHERE aid = ?1",
            params![
                a.aid,
                a.type_name.as_deref().map(anime_type_id).unwrap_or(1),
                a.episode_count,
                a.start_date.as_deref().map(format::parse_date).unwrap_or(0),
                a.end_date.as_deref().map(format::parse_date).unwrap_or(0),
                year,
                a.url,
                a.description,
                a.picture,
                (a.rating * 100.0).round() as i64,
                a.votes,
                (a.temp_rating * 100.0).round() as i64,
                a.temp_votes,
                (a.review_rating * 100.0).round() as i64,
                a.reviews,
                a.restricted,
                highest,
                specials,
                creators,
                format::now()
            ],
        )?;
        if !a.titles.is_empty() {
            self.conn()
                .execute("DELETE FROM anime_title WHERE aid = ?1", [a.aid])?;
            for t in &a.titles {
                self.conn().execute(
                    "INSERT INTO anime_title(aid, name, type, lang) VALUES (?1, ?2, ?3, ?4)",
                    params![a.aid, t.name, title_kind_id(&t.kind), t.lang],
                )?;
            }
        }
        if !a.related.is_empty() {
            self.conn()
                .execute("DELETE FROM anime_relation WHERE aid = ?1", [a.aid])?;
            for (rid, kind) in &a.related {
                self.conn().execute(
                    "INSERT INTO anime_relation(aid, related_aid, type) VALUES (?1, ?2, ?3)",
                    params![a.aid, rid, relation_id(kind)],
                )?;
            }
        }
        if !a.tags.is_empty() {
            self.conn().execute(
                "DELETE FROM anime_genre WHERE aid = ?1 AND genre_id >= 100000",
                [a.aid],
            )?;
            for t in a.tags.iter().filter(|t| !t.name.is_empty()) {
                // Tags live next to the legacy genres; offset their ids to avoid collisions.
                let gid = 100_000 + t.id;
                self.conn().execute(
                    "INSERT OR REPLACE INTO genre(id, name) VALUES (?1, ?2)",
                    params![gid, t.name],
                )?;
                self.conn().execute(
                    "INSERT INTO anime_genre(aid, genre_id, weight) VALUES (?1, ?2, ?3)",
                    params![a.aid, gid, t.weight],
                )?;
            }
        }
        for ep in &a.episodes {
            self.conn().execute(
                "INSERT INTO episode(eid, aid, epno, type, length, aired, rating, votes, other, updated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(eid) DO UPDATE SET aid = excluded.aid, epno = excluded.epno, type = excluded.type,
                   length = excluded.length, aired = excluded.aired, rating = excluded.rating, votes = excluded.votes,
                   other = COALESCE(excluded.other, other), updated = excluded.updated",
                params![
                    ep.eid,
                    a.aid,
                    ep.epno.number,
                    ep.epno.kind.max(1),
                    ep.length,
                    ep.airdate.as_deref().map(format::parse_date).unwrap_or(0),
                    (ep.rating * 100.0).round() as i64,
                    ep.votes,
                    ep.summary,
                    format::now()
                ],
            )?;
            for t in &ep.titles {
                self.set_episode_title(ep.eid, &t.name, &t.lang)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn store_episode_info(&self, e: &EpisodeInfo) -> Result<()> {
        self.conn().execute(
            "INSERT INTO episode(eid, aid, epno, type, length, aired, rating, votes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(eid) DO UPDATE SET aid = excluded.aid, epno = excluded.epno, type = excluded.type,
               length = excluded.length, aired = excluded.aired, rating = excluded.rating, votes = excluded.votes",
            params![e.eid, e.aid, e.epno.number, e.epno.kind.max(1), e.length, e.aired, e.rating, e.votes],
        )?;
        if let Some(n) = &e.english {
            self.set_episode_title(e.eid, n, "en")?;
        }
        if let Some(n) = &e.romaji {
            self.set_episode_title(e.eid, n, "x-jat")?;
        }
        if let Some(n) = &e.kanji {
            self.set_episode_title(e.eid, n, "ja")?;
        }
        Ok(())
    }

    pub fn store_group_info(&self, g: &GroupInfo) -> Result<()> {
        self.conn().execute(
            "INSERT INTO grp(gid, name, short, url, irc_server, irc_channel, picname, rating, votes, updated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(gid) DO UPDATE SET name = excluded.name, short = excluded.short, url = excluded.url,
               irc_server = excluded.irc_server, irc_channel = excluded.irc_channel, picname = excluded.picname,
               rating = excluded.rating, votes = excluded.votes, updated = excluded.updated",
            params![g.gid, g.name, g.short, g.url, g.irc_server, g.irc_channel, g.picname, g.rating, g.votes, format::now()],
        )?;
        Ok(())
    }

    /// Import the daily titles dump (anime rows are created for unknown aids; only anime without
    /// a full HTTP record get their titles replaced) and record the completion time.
    ///
    /// `read` streams the dump into the sink it is given and returns `Ok` only for a complete,
    /// validated document (see `animatic_anidb::http::read_titles_dump`). Nothing touches the
    /// library database while `read` runs, however long the download takes: rows are spooled
    /// into a private temporary SQLite database (deleted when its connection closes, also on
    /// errors and panics) and merged in one short write transaction afterwards. A failure of
    /// `read` or of the merge therefore leaves titles and completion time untouched. Memory
    /// stays bounded: the spool is read back row by row.
    pub fn import_title_dump<E: From<Error>>(
        &self,
        read: impl FnOnce(
            &mut dyn FnMut(DumpTitle) -> std::result::Result<(), E>,
        ) -> std::result::Result<usize, E>,
    ) -> std::result::Result<TitleImport, E> {
        fn db_err<E: From<Error>>(e: rusqlite::Error) -> E {
            E::from(Error::from(e))
        }

        // An empty file name is SQLite's private temporary on-disk database.
        let spool = Connection::open("").map_err(db_err::<E>)?;
        spool
            .pragma_update(None, "journal_mode", "OFF")
            .map_err(db_err::<E>)?;
        spool
            .pragma_update(None, "synchronous", "OFF")
            .map_err(db_err::<E>)?;
        spool
            .pragma_update(None, "cache_size", -8192)
            .map_err(db_err::<E>)?;
        spool
            .execute_batch(
                "CREATE TABLE spool(
                     seq  INTEGER PRIMARY KEY,
                     aid  INTEGER NOT NULL,
                     kind INTEGER NOT NULL,
                     lang TEXT    NOT NULL,
                     name TEXT    NOT NULL
                 );
                 BEGIN",
            )
            .map_err(db_err::<E>)?;
        let titles = {
            let mut insert = spool
                .prepare("INSERT INTO spool(aid, kind, lang, name) VALUES (?1, ?2, ?3, ?4)")
                .map_err(db_err::<E>)?;
            let mut sink = |t: DumpTitle| -> std::result::Result<(), E> {
                insert
                    .execute(params![t.aid, title_kind_id(&t.kind), t.lang, t.name])
                    .map_err(db_err::<E>)?;
                Ok(())
            };
            read(&mut sink)?
        };

        // The dump is complete and valid: merge it atomically.
        let conn = self.conn();
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
            .map_err(db_err::<E>)?;
        let mut added = 0;
        {
            let mut ensure = conn
                .prepare("INSERT OR IGNORE INTO anime(aid) VALUES (?1)")
                .map_err(db_err::<E>)?;
            let mut fetched = conn
                .prepare("SELECT fetched FROM anime WHERE aid = ?1")
                .map_err(db_err::<E>)?;
            let mut clear = conn
                .prepare("DELETE FROM anime_title WHERE aid = ?1")
                .map_err(db_err::<E>)?;
            let mut exists = conn
                .prepare(
                    "SELECT EXISTS(SELECT 1 FROM anime_title WHERE aid = ?1 AND name = ?2 AND lang = ?3)",
                )
                .map_err(db_err::<E>)?;
            let mut insert = conn
                .prepare("INSERT INTO anime_title(aid, name, type, lang) VALUES (?1, ?2, ?3, ?4)")
                .map_err(db_err::<E>)?;
            let mut spooled = spool
                .prepare("SELECT aid, kind, lang, name FROM spool ORDER BY seq")
                .map_err(db_err::<E>)?;
            let mut rows = spooled.query([]).map_err(db_err::<E>)?;
            let mut current = None;
            while let Some(row) = rows.next().map_err(db_err::<E>)? {
                let (aid, kind, lang, name): (i64, i64, String, String) = (
                    row.get(0).map_err(db_err::<E>)?,
                    row.get(1).map_err(db_err::<E>)?,
                    row.get(2).map_err(db_err::<E>)?,
                    row.get(3).map_err(db_err::<E>)?,
                );
                if current != Some(aid) {
                    current = Some(aid);
                    ensure.execute([aid]).map_err(db_err::<E>)?;
                    // Only replace titles of anime without a full HTTP record.
                    let fetched: i64 = fetched
                        .query_row([aid], |r| r.get(0))
                        .map_err(db_err::<E>)?;
                    if fetched == 0 {
                        clear.execute([aid]).map_err(db_err::<E>)?;
                    }
                }
                let present: bool = exists
                    .query_row(params![aid, name, lang], |r| r.get(0))
                    .map_err(db_err::<E>)?;
                if !present {
                    insert
                        .execute(params![aid, name, kind, lang])
                        .map_err(db_err::<E>)?;
                    added += 1;
                }
            }
        }
        self.set_meta(TITLES_DUMP_DATE_KEY, &format::now().to_string())?;
        tx.commit().map_err(db_err::<E>)?;
        Ok(TitleImport { titles, added })
    }

    /// Record a hashed file. Returns the local_file id.
    pub fn add_local_file(&self, path: &str, h: &Hashes, mtime: i64) -> Result<i64> {
        let c = self.conn();
        c.execute(
            "INSERT INTO local_hash(ed2k, size, crc32, md5, sha1) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(ed2k, size) DO UPDATE SET crc32 = excluded.crc32,
               md5 = COALESCE(NULLIF(excluded.md5, ''), md5), sha1 = COALESCE(NULLIF(excluded.sha1, ''), sha1)",
            params![h.ed2k, h.size as i64, h.crc32, h.md5, h.sha1],
        )?;
        let hash_id: i64 = c.query_row(
            "SELECT id FROM local_hash WHERE ed2k = ?1 AND size = ?2",
            params![h.ed2k, h.size as i64],
            |r| r.get(0),
        )?;
        c.execute(
            "INSERT INTO local_file(hash_id, path, mtime, added) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET hash_id = excluded.hash_id, mtime = excluded.mtime",
            params![hash_id, path, mtime, format::now()],
        )?;
        Ok(
            c.query_row("SELECT id FROM local_file WHERE path = ?1", [path], |r| {
                r.get(0)
            })?,
        )
    }

    pub fn remove_local_file(&self, id: i64) -> Result<()> {
        self.conn()
            .execute("DELETE FROM local_file WHERE id = ?1", [id])?;
        self.conn().execute(
            "DELETE FROM local_hash WHERE NOT EXISTS (SELECT 1 FROM local_file l WHERE l.hash_id = local_hash.id)",
            [],
        )?;
        Ok(())
    }

    pub fn clear_local_files(&self) -> Result<()> {
        self.conn()
            .execute_batch("DELETE FROM local_file; DELETE FROM local_hash;")?;
        Ok(())
    }

    pub fn move_local_file(&self, id: i64, new_path: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE local_file SET path = ?2 WHERE id = ?1",
            params![id, new_path],
        )?;
        Ok(())
    }

    /// Stored path and hashes of local file `id`, or `None` when the row is gone. Used to check
    /// that a move still concerns the file the user saw and to verify a copy against the hash
    /// recorded when the file was hashed.
    pub fn local_file_record(&self, id: i64) -> Result<Option<(String, Hashes)>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT l.path, h.size, h.ed2k, h.crc32, h.md5, h.sha1
                 FROM local_file l JOIN local_hash h ON h.id = l.hash_id WHERE l.id = ?1",
                [id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        Hashes {
                            size: r.get::<_, i64>(1)? as u64,
                            ed2k: r.get(2)?,
                            crc32: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                            md5: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                            sha1: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                        },
                    ))
                },
            )
            .optional()?)
    }

    pub fn local_file_id_by_path(&self, path: &str) -> Result<Option<(i64, i64)>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, mtime FROM local_file WHERE path = ?1",
                [path],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// Keep the latest MYLISTSTATS result and when it arrived, replacing the previous one.
    pub fn store_mylist_stats(&self, stats: &MylistStats, fetched: i64) -> Result<()> {
        let stored = StoredMylistStats {
            fetched,
            stats: stats.clone(),
        };
        self.set_meta(MYLIST_STATS_KEY, &serde_json::to_string(&stored)?)
    }

    /// The stored AniDB account statistics. A value this version cannot read is reported as
    /// absent rather than failing the whole library projection.
    pub fn mylist_stats_snapshot(&self) -> Result<Option<StoredMylistStats>> {
        Ok(self
            .meta(MYLIST_STATS_KEY)?
            .and_then(|json| serde_json::from_str(&json).ok()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use animatic_anidb::commands::EpNo;

    #[test]
    fn file_info_populates_cache_and_mylist() {
        let db = Db::memory().unwrap();
        let info = FileInfo {
            fid: 5,
            aid: 2,
            eid: 20,
            gid: 3,
            lid: 77,
            size: 1234,
            ed2k: Some("abc".into()),
            crc32: Some("0badf00d".into()),
            quality: Some("high".into()),
            source: Some("Blu-ray".into()),
            filetype: Some("mkv".into()),
            anime_romaji: Some("Romaji".into()),
            anime_english: Some("English".into()),
            anime_type: Some("TV Series".into()),
            anime_episodes: 12,
            epno: EpNo { kind: 1, number: 3 },
            ep_name: Some("Third".into()),
            group_short: Some("GRP".into()),
            other_episodes: vec![(21, 50)],
            mylist: Some(ApiMylistEntry {
                lid: 77,
                fid: 5,
                state: 1,
                viewdate: 99,
                ..Default::default()
            }),
            ..FileInfo::default()
        };
        db.store_file_info(&info).unwrap();
        let a = db.anime(2).unwrap().unwrap();
        assert_eq!((a.name.as_str(), a.type_id, a.episodes), ("Romaji", 2, 12));
        let f = db.file_by_hash("ABC", 1234).unwrap().unwrap();
        assert_eq!(
            (f.fid, f.quality.as_str(), f.group_label().as_str()),
            (5, "high", "GRP")
        );
        assert_eq!(db.files_of_episode(21).unwrap()[0].end_pct, 50);
        let m = db.mylist_entry(77).unwrap().unwrap();
        assert_eq!((m.aid, m.eid, m.gid, m.viewed), (2, 20, 3, true));
        assert_eq!(db.episode(20).unwrap().unwrap().name, "Third");

        // An explicit "lid 0" reply (AniDB says not in mylist) removes the stale entry.
        db.store_file_info(&FileInfo {
            mylist: None,
            mylist_absent: true,
            ..info
        })
        .unwrap();
        assert!(db.mylist_entry(77).unwrap().is_none());
    }

    fn synthetic_file(fid: i64) -> FileInfo {
        FileInfo {
            fid,
            aid: 2,
            eid: 20,
            gid: 3,
            size: 1234,
            ed2k: Some("abc".into()),
            anime_romaji: Some("Romaji".into()),
            epno: EpNo { kind: 1, number: 3 },
            ep_name: Some("Third".into()),
            mylist: Some(ApiMylistEntry {
                lid: 77,
                fid,
                state: 1,
                ..Default::default()
            }),
            ..FileInfo::default()
        }
    }

    fn count(db: &Db, sql: &str) -> i64 {
        db.conn().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn reply_without_mylist_information_preserves_existing_rows() {
        let db = Db::memory().unwrap();
        db.store_file_info(&synthetic_file(5)).unwrap();
        assert!(db.mylist_entry(77).unwrap().is_some());
        // Partial reply: no lid information and most identifiers omitted.
        db.store_file_info(&FileInfo {
            fid: 5,
            ..FileInfo::default()
        })
        .unwrap();
        assert!(db.mylist_entry(77).unwrap().is_some(), "mylist survives");
        let f = db.file_by_hash("ABC", 1234).unwrap().unwrap();
        assert_eq!((f.fid, f.aid, f.eid, f.gid), (5, 2, 20, 3));
        assert_eq!(count(&db, "SELECT count(*) FROM episode WHERE eid = 20"), 1);
    }

    #[test]
    fn file_info_write_is_atomic() {
        let db = Db::memory().unwrap();
        db.conn()
            .execute_batch(
                "CREATE TRIGGER synthetic_fail BEFORE INSERT ON episode
                 BEGIN SELECT RAISE(ABORT, 'synthetic child failure'); END;",
            )
            .unwrap();
        assert!(db.store_file_info(&synthetic_file(5)).is_err());
        assert_eq!(count(&db, "SELECT count(*) FROM file"), 0);
        assert_eq!(count(&db, "SELECT count(*) FROM anime"), 0);
        assert_eq!(count(&db, "SELECT count(*) FROM mylist"), 0);

        // Inside a caller's transaction only this write rolls back; the caller's work survives.
        db.conn().execute_batch("BEGIN").unwrap();
        db.set_meta("synthetic", "kept").unwrap();
        assert!(db.store_file_info(&synthetic_file(5)).is_err());
        db.conn()
            .execute_batch("DROP TRIGGER synthetic_fail")
            .unwrap();
        db.store_file_info(&synthetic_file(6)).unwrap();
        db.conn().execute_batch("COMMIT").unwrap();
        assert_eq!(db.meta("synthetic").unwrap().as_deref(), Some("kept"));
        assert_eq!(count(&db, "SELECT count(*) FROM file WHERE fid = 5"), 0);
        assert_eq!(count(&db, "SELECT count(*) FROM file WHERE fid = 6"), 1);
    }

    #[test]
    fn api_writes_keep_the_latest_pending_local_intent() {
        let db = Db::memory().unwrap();
        db.store_file_info(&synthetic_file(5)).unwrap();
        db.queue_edit_mylist(
            77,
            &animatic_anidb::commands::MylistEdit {
                state: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
        // A stale authoritative reply (state 1) must not undo the queued intent.
        db.store_api_mylist(&ApiMylistEntry {
            lid: 77,
            fid: 5,
            state: 1,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(db.mylist_entry(77).unwrap().unwrap().state, 2);
        // Nor may an explicit "not in mylist" FILE reply delete an entry with outstanding intent.
        db.store_file_info(&FileInfo {
            mylist: None,
            mylist_absent: true,
            ..synthetic_file(5)
        })
        .unwrap();
        assert!(db.mylist_entry(77).unwrap().is_some());
    }

    #[test]
    fn local_files_share_hash_records() {
        let db = Db::memory().unwrap();
        let h = Hashes {
            size: 10,
            ed2k: "e".into(),
            crc32: "C".into(),
            md5: "M".into(),
            sha1: "S".into(),
        };
        let a = db.add_local_file("/x/a.mkv", &h, 1).unwrap();
        let b = db.add_local_file("/y/a.mkv", &h, 2).unwrap();
        assert_ne!(a, b);
        assert_eq!(db.local_files().unwrap().len(), 2);
        db.remove_local_file(a).unwrap();
        db.remove_local_file(b).unwrap();
        let n: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM local_hash", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "orphaned hashes are dropped");
    }

    const DUMP_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<animetitles>
<anime aid="1"><title type="main" xml:lang="x-jat">New One</title>
<title type="official" xml:lang="en">New One EN</title></anime>
<anime aid="2"><title type="main" xml:lang="x-jat">Dump Two</title></anime>
<anime aid="3"><title type="main" xml:lang="x-jat">Dump Three</title></anime>
</animetitles>
"#;

    /// A valid gzip file made of stored (uncompressed) deflate blocks.
    fn gzip_stored(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
        let mut chunks = data.chunks(65535).peekable();
        while let Some(chunk) = chunks.next() {
            let len = chunk.len() as u16;
            out.push(u8::from(chunks.peek().is_none()));
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
        out.extend_from_slice(&crc32fast::hash(data).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out
    }

    fn import_bytes(
        db: &Db,
        bytes: &[u8],
    ) -> std::result::Result<TitleImport, crate::sync::CommandError> {
        db.import_title_dump(|sink| {
            animatic_anidb::http::read_titles_dump(
                bytes,
                &animatic_anidb::http::TitlesLimits::default(),
                sink,
            )
        })
    }

    fn title_rows(db: &Db) -> Vec<(i64, String, i64, String)> {
        db.conn()
            .prepare("SELECT aid, name, type, lang FROM anime_title ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn anime_count(db: &Db) -> i64 {
        db.conn()
            .query_row("SELECT count(*) FROM anime", [], |r| r.get(0))
            .unwrap()
    }

    /// Anime 1 has only dump titles, anime 2 a full HTTP record, and a dump ran before.
    fn seed_titles(db: &Db) {
        db.conn()
            .execute_batch(
                "INSERT INTO anime(aid, fetched) VALUES (1, 0), (2, 500);
                 INSERT INTO anime_title(aid, name, type, lang) VALUES
                   (1, 'Old One', 1, 'x-jat'), (2, 'Kept Two', 1, 'x-jat');",
            )
            .unwrap();
        db.set_meta(TITLES_DUMP_DATE_KEY, "111").unwrap();
    }

    fn assert_untouched(db: &Db, rows: &[(i64, String, i64, String)]) {
        assert_eq!(title_rows(db), rows);
        assert_eq!(anime_count(db), 2);
        assert_eq!(
            db.meta(TITLES_DUMP_DATE_KEY).unwrap().as_deref(),
            Some("111")
        );
    }

    #[test]
    fn title_dump_import_merges_and_records_completion() {
        let db = Db::memory().unwrap();
        seed_titles(&db);
        let import = import_bytes(&db, &gzip_stored(DUMP_XML.as_bytes())).unwrap();
        assert_eq!(
            import,
            TitleImport {
                titles: 4,
                added: 4
            }
        );

        let names = |aid: i64| -> Vec<(String, i64)> {
            title_rows(&db)
                .into_iter()
                .filter(|r| r.0 == aid)
                .map(|r| (r.1, r.2))
                .collect()
        };
        // Without a full record the dump replaces the titles; with one it only adds.
        assert_eq!(
            names(1),
            [("New One".to_string(), 1), ("New One EN".to_string(), 4)]
        );
        assert_eq!(
            names(2),
            [("Kept Two".to_string(), 1), ("Dump Two".to_string(), 1)]
        );
        assert_eq!(names(3), [("Dump Three".to_string(), 1)]);
        assert_eq!(anime_count(&db), 3);
        let stamp: i64 = db
            .meta(TITLES_DUMP_DATE_KEY)
            .unwrap()
            .and_then(|v| v.parse().ok())
            .unwrap();
        assert!(stamp > 111);
        // Plain XML is accepted as well.
        import_bytes(&db, DUMP_XML.as_bytes()).unwrap();
    }

    #[test]
    fn failed_title_dumps_leave_titles_and_timestamp_untouched() {
        let db = Db::memory().unwrap();
        seed_titles(&db);
        let before = title_rows(&db);
        let good = gzip_stored(DUMP_XML.as_bytes());

        // Everything is valid and delivered, but the gzip trailer is wrong.
        let mut bad_crc = good.clone();
        let crc = bad_crc.len() - 8;
        bad_crc[crc] ^= 0xff;
        assert!(import_bytes(&db, &bad_crc).is_err());
        assert_untouched(&db, &before);

        assert!(import_bytes(&db, &good[..good.len() - 5]).is_err());
        assert_untouched(&db, &before);

        // Valid rows first, a broken anime at the very end.
        let late = DUMP_XML.replace("</animetitles>", r#"<anime aid="4"></anime></animetitles>"#);
        assert!(import_bytes(&db, late.as_bytes()).is_err());
        assert!(import_bytes(&db, &gzip_stored(late.as_bytes())).is_err());
        assert_untouched(&db, &before);

        // The download itself dies after every title was accepted.
        let interrupted = db.import_title_dump::<crate::sync::CommandError>(|sink| {
            animatic_anidb::http::read_titles_dump(
                &good[..],
                &animatic_anidb::http::TitlesLimits::default(),
                sink,
            )?;
            Err(animatic_anidb::AnidbError::Network("synthetic".into()).into())
        });
        assert!(interrupted.is_err());
        assert_untouched(&db, &before);

        // A storage failure halfway through the merge rolls back what was already merged.
        db.conn()
            .execute_batch(
                "CREATE TRIGGER poison BEFORE INSERT ON anime_title WHEN NEW.name = 'Dump Three'
                 BEGIN SELECT RAISE(ABORT, 'synthetic'); END;",
            )
            .unwrap();
        assert!(import_bytes(&db, &good).is_err());
        assert_untouched(&db, &before);
        db.conn().execute_batch("DROP TRIGGER poison").unwrap();

        // Nothing is left in a bad state: the next good dump commits.
        assert_eq!(import_bytes(&db, &good).unwrap().titles, 4);
        assert_eq!(anime_count(&db), 3);
    }

    #[test]
    fn title_dump_download_does_not_lock_the_library() {
        let path = std::env::temp_dir().join(format!(
            "animatic-titles-lock-{}.sqlite",
            std::process::id()
        ));
        let remove = || {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
            }
        };
        remove();
        let db = Db::open(&path).unwrap();
        let other = Db::open(&path).unwrap();
        other
            .conn()
            .busy_timeout(std::time::Duration::ZERO)
            .unwrap();
        seed_titles(&db);
        let good = gzip_stored(DUMP_XML.as_bytes());
        let mut delivered = 0;
        let result = db.import_title_dump::<crate::sync::CommandError>(|sink| {
            animatic_anidb::http::read_titles_dump(
                &good[..],
                &animatic_anidb::http::TitlesLimits::default(),
                |title| {
                    delivered += 1;
                    if delivered == 2 {
                        // Mid download another connection (the UI) writes without any wait.
                        other.set_meta("probe", "written during download").unwrap();
                    }
                    sink(title)
                },
            )
        });
        assert_eq!(result.unwrap().titles, 4);
        assert_eq!(
            db.meta("probe").unwrap().as_deref(),
            Some("written during download")
        );
        drop((db, other));
        remove();
    }
}
