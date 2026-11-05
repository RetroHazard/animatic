//! Write side: persisting AniDB replies, mylist changes and hashed files.

use animatic_anidb::commands::{
    AnimeInfo, EpisodeInfo, FileInfo, GroupInfo, MylistEntry as ApiMylistEntry,
};
use animatic_anidb::http::{AnimeXml, DumpTitle};
use rusqlite::{OptionalExtension, params};

use crate::db::Db;
use crate::error::Result;
use crate::format;
use crate::hash::Hashes;
use crate::library::{MylistEntry, anime_type_id, relation_id, title_kind_id};

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

    /// Store everything a FILE reply carries.
    pub fn store_file_info(&self, i: &FileInfo) -> Result<()> {
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
             ON CONFLICT(fid) DO UPDATE SET aid = excluded.aid, eid = excluded.eid, gid = excluded.gid, size = excluded.size,
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
            None => {
                // AniDB says the file is not in mylist: drop stale local entries.
                c.execute("DELETE FROM mylist WHERE fid = ?1", [i.fid])?;
            }
        }
        Ok(())
    }

    pub fn store_api_mylist(&self, m: &ApiMylistEntry) -> Result<()> {
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
        self.upsert_mylist(&MylistEntry {
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
        })
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

    /// Insert anime titles from the daily dump (anime rows are created for unknown aids).
    pub fn store_title_dump(&self, titles: &[DumpTitle]) -> Result<usize> {
        let tx = self.conn().unchecked_transaction()?;
        let mut current = 0;
        let mut n = 0;
        for t in titles {
            if t.aid != current {
                current = t.aid;
                self.ensure_anime_row(t.aid)?;
                // Only replace titles of anime without a full HTTP record.
                let fetched: i64 = self.conn().query_row(
                    "SELECT fetched FROM anime WHERE aid = ?1",
                    [t.aid],
                    |r| r.get(0),
                )?;
                if fetched == 0 {
                    self.conn()
                        .execute("DELETE FROM anime_title WHERE aid = ?1", [t.aid])?;
                }
            }
            let exists: i64 = self.conn().query_row(
                "SELECT count(*) FROM anime_title WHERE aid = ?1 AND name = ?2 AND lang = ?3",
                params![t.aid, t.name, t.lang],
                |r| r.get(0),
            )?;
            if exists == 0 {
                self.conn().execute(
                    "INSERT INTO anime_title(aid, name, type, lang) VALUES (?1, ?2, ?3, ?4)",
                    params![t.aid, t.name, title_kind_id(&t.kind), t.lang],
                )?;
                n += 1;
            }
        }
        tx.commit()?;
        Ok(n)
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

        // A later reply without mylist data removes the stale entry.
        db.store_file_info(&FileInfo {
            mylist: None,
            ..info
        })
        .unwrap();
        assert!(db.mylist_entry(77).unwrap().is_none());
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
}
