//! Rename-language objects built from the database (the published properties of the legacy
//! `TAniDBAnime`, `TAniDBEpisode`, `TAniDBFile`, `TAniDBGroup`, `TAniDBLocalHash`, `TAniDBMylistEntry`).

use rusqlite::{OptionalExtension, params};

use super::{Bindings, DataSource, Object, Value};
use crate::db::Db;
use crate::library::{Anime, Episode, FileRec, Group, MylistEntry, crc_status};

/// `A.Name` of an anime whose title has not been fetched yet; a stand-in, never real metadata.
pub const TITLE_PENDING: &str = "[Title not yet available]";

/// `DataSource` over the application database.
pub struct DbSource<'a> {
    pub db: &'a Db,
}

fn ws(s: impl Into<String>) -> Value {
    Value::WStr(s.into())
}

fn as_(s: impl Into<String>) -> Value {
    Value::AStr(s.into())
}

fn i(v: i64) -> Value {
    Value::Int(v)
}

fn year_bounds(a: &Anime) -> (i64, i64) {
    let mut parts = a
        .year
        .split(['-', '?'])
        .filter_map(|p| p.trim().get(..4).and_then(|y| y.parse::<i64>().ok()));
    let start = parts.next().unwrap_or(0);
    let end = parts.next().unwrap_or(start);
    (start, end)
}

/// Anime object (`A`).
pub fn anime_object(db: &Db, a: &Anime) -> Object {
    let c = db.conn();
    let count = |sql: &str| -> i64 { c.query_row(sql, [a.aid], |r| r.get(0)).unwrap_or(0) };
    let eps = count("SELECT count(*) FROM episode WHERE aid = ?1 AND type = 1");
    let specials = count("SELECT count(*) FROM episode WHERE aid = ?1 AND type = 2");
    let length = count("SELECT COALESCE(sum(length), 0) FROM episode WHERE aid = ?1 AND type = 1");
    let size = count("SELECT COALESCE(sum(size), 0) FROM file WHERE aid = ?1");
    let ml_eps = count(
        "SELECT count(DISTINCT m.eid) FROM mylist m JOIN episode e ON e.eid = m.eid WHERE m.aid = ?1 AND e.type = 1",
    );
    let ml_specials = count(
        "SELECT count(DISTINCT m.eid) FROM mylist m JOIN episode e ON e.eid = m.eid WHERE m.aid = ?1 AND e.type <> 1",
    );
    let watched = count(
        "SELECT count(DISTINCT m.eid) FROM mylist m JOIN episode e ON e.eid = m.eid
         WHERE m.aid = ?1 AND e.type = 1 AND m.viewed = 1",
    );
    let sp_watched = count(
        "SELECT count(DISTINCT m.eid) FROM mylist m JOIN episode e ON e.eid = m.eid
         WHERE m.aid = ?1 AND e.type <> 1 AND m.viewed = 1",
    );
    let ml_size = count(
        "SELECT COALESCE(sum(f.size), 0) FROM mylist m JOIN file f ON f.fid = m.fid WHERE m.aid = ?1",
    );
    let ml_length = count(
        "SELECT COALESCE(sum(e.length), 0) FROM mylist m JOIN episode e ON e.eid = m.eid WHERE m.aid = ?1",
    );
    let ml_watched_length = count(
        "SELECT COALESCE(sum(e.length), 0) FROM mylist m JOIN episode e ON e.eid = m.eid WHERE m.aid = ?1 AND m.viewed = 1",
    );
    let total = a.effective_episodes();
    let download = if ml_eps == 0 {
        "adsHaveNone"
    } else if a.episodes > 0 && ml_eps >= a.episodes {
        "adsComplete"
    } else if ml_eps >= total.max(1) {
        "adsHaveAll"
    } else {
        "adsIncomplete"
    };
    let watch = if watched == 0 {
        "awsNone"
    } else if watched >= ml_eps.max(total) {
        "awsAll"
    } else {
        "awsPartial"
    };
    let (start_year, end_year) = year_bounds(a);
    Object::new("TAniDBAnime")
        .with("Id", i(a.aid))
        .with(
            "Name",
            ws(if a.name.is_empty() {
                TITLE_PENDING.to_string()
            } else {
                a.name.clone()
            }),
        )
        .with("Category", i(a.type_id))
        .with("CategoryStr", ws(a.type_name()))
        .with("Restricted", Value::Bool(a.restricted))
        .with("ReviewAverage", i(a.review_rating))
        .with("ReviewCount", i(a.reviews))
        .with("TotalEpisodeCount", i(a.episodes))
        .with("VoteAverage", i(a.rating))
        .with("VoteCount", i(a.votes))
        .with("Year", as_(a.year.clone()))
        .with("EpisodeCount", i(eps))
        .with("SpecialEpisodeCount", i(specials))
        .with("DownloadState", as_(download))
        .with("WatchedState", as_(watch))
        .with("StartYear", i(start_year))
        .with("EndYear", i(end_year))
        .with("AirDate", Value::Int64(a.air_date))
        .with("EndDate", Value::Int64(a.end_date))
        .with("CurrentEpisodeCount", i(a.highest_ep))
        .with("Length", i(length))
        .with("MylistCurrentEpisodeCount", i(ml_eps))
        .with("MylistLength", i(ml_length))
        .with("MylistWatchedLength", i(ml_watched_length))
        .with("MylistSpecialEpisodeCount", i(ml_specials))
        .with("Size", Value::Int64(size))
        .with("MylistSize", Value::Int64(ml_size))
        .with("SpecialWatchedCount", i(sp_watched))
        .with("WatchedCount", i(watched))
}

/// Episode object (`E`).
pub fn episode_object(db: &Db, e: &Episode) -> Object {
    let type_count: i64 = db
        .conn()
        .query_row(
            "SELECT COALESCE(max(epno), 0) FROM episode WHERE aid = ?1 AND type = ?2",
            params![e.aid, e.kind as i64],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let width = type_count.max(1).to_string().len();
    Object::new("TAniDBEpisode")
        .with("Id", i(e.eid))
        .with("AirDate", Value::Int64(e.aired))
        .with("EpisodeNo", i(e.epno))
        .with("EpisodeStr", ws(e.epno_str()))
        .with(
            "EpisodeStrPadded",
            ws(format!("{}{:0width$}", e.kind.prefix(), e.epno)),
        )
        .with("EpisodeTypeCount", i(type_count))
        .with(
            "IsSpecial",
            Value::Bool(e.kind != crate::library::EpisodeType::Regular),
        )
        .with("Length", i(e.length))
        .with("Name", ws(e.name.clone()))
        .with("Other", ws(e.other.clone().unwrap_or_default()))
        .with("TypeId", i(e.kind as i64))
        .with("IsRecap", Value::Bool(e.recap))
}

/// File object (`F`).
pub fn file_object(f: &FileRec) -> Object {
    let (w, h) = f
        .resolution
        .as_deref()
        .and_then(|r| r.split_once('x'))
        .map(|(w, h)| (w.trim().parse().unwrap_or(0), h.trim().parse().unwrap_or(0)))
        .unwrap_or((0, 0));
    let crc = match crc_status(f.state) {
        Some(true) => "crcOk",
        Some(false) => "crcInvalid",
        None => "crcUnknown",
    };
    Object::new("TAniDBFile")
        .with("Id", i(f.fid))
        .with(
            "Crc",
            as_(f.crc32.clone().unwrap_or_default().to_ascii_lowercase()),
        )
        .with("CrcStatus", as_(crc))
        .with("Ed2k", as_(f.ed2k.clone().unwrap_or_default()))
        .with("FileType", as_(f.filetype.clone().unwrap_or_default()))
        .with("Length", i(f.length))
        .with("Md5", as_(f.md5.clone().unwrap_or_default()))
        .with("Other", ws(f.other.clone().unwrap_or_default()))
        .with("Quality", ws(f.quality.clone()))
        .with("Released", i(f.released))
        .with("ResolutionHeight", i(h))
        .with("ResolutionWidth", i(w))
        .with("Sha1", as_(f.sha1.clone().unwrap_or_default()))
        .with("Size", Value::Int64(f.size))
        .with("State", i(f.state))
        .with("Source", ws(f.source.clone()))
        .with("SubtitleId", i(0))
        .with("TypeState", i(f.file_type))
        .with("UserCount", i(f.user_count))
        .with("Version", i(f.version()))
        .with("EpisodeStart", i(f.start_pct))
        .with("EpisodeEnd", i(f.end_pct))
        .with("StartPercent", i(f.start_pct))
        .with("EndPercent", i(f.end_pct))
        .with("GroupShort", ws(f.group_short.clone().unwrap_or_default()))
        .with("GroupLong", ws(f.group_name.clone().unwrap_or_default()))
        .with(
            "ResolutionString",
            ws(f.resolution.clone().unwrap_or_default()),
        )
}

/// Group object (`G`).
pub fn group_object(g: &Group) -> Object {
    Object::new("TAniDBGroup")
        .with("Id", i(g.gid))
        .with("Name", ws(g.name.clone()))
        .with("Other", ws(g.other.clone().unwrap_or_default()))
        .with("Rating", i(g.rating))
        .with("Shortname", ws(g.short.clone()))
        .with("Votes", i(g.votes))
}

/// Mylist object (`M`).
pub fn mylist_object(m: &MylistEntry) -> Object {
    Object::new("TAniDBMylistEntry")
        .with("Id", i(m.lid))
        .with("Date", i(m.date))
        .with("EpisodeId", i(m.eid))
        .with("FileId", i(m.fid))
        .with("MylistId", i(m.lid))
        .with("Other", ws(m.other.clone().unwrap_or_default()))
        .with("Source", ws(m.source.clone().unwrap_or_default()))
        .with("State", i(m.state))
        .with("Storage", ws(m.storage.clone().unwrap_or_default()))
        .with("Watched", Value::Bool(m.viewed))
        .with("WatchedDate", i(m.viewdate))
}

/// Local hash object (`H`) for a file, if it was hashed locally.
pub fn hash_object(db: &Db, ed2k: &str, size: i64) -> Option<Object> {
    db.conn()
        .query_row(
            "SELECT id, crc32, ed2k, md5, sha1, size, fps, length, video_fourcc, video_compression, video_codec,
                    video_height, video_width, video_bitrate, video_vbr, audio_ident, audio_codec, audio_samplerate,
                    audio_bitrate, audio_vbr
             FROM local_hash WHERE ed2k = ?1 AND size = ?2",
            params![ed2k.to_ascii_lowercase(), size],
            |r| {
                let s = |k: usize| -> rusqlite::Result<Value> { Ok(as_(r.get::<_, Option<String>>(k)?.unwrap_or_default())) };
                let d = |k: usize| -> rusqlite::Result<Value> { Ok(Value::Double(r.get::<_, Option<f64>>(k)?.unwrap_or(0.0))) };
                let n = |k: usize| -> rusqlite::Result<Value> { Ok(i(r.get::<_, Option<i64>>(k)?.unwrap_or(0))) };
                let b = |k: usize| -> rusqlite::Result<Value> { Ok(Value::Bool(r.get::<_, Option<i64>>(k)?.unwrap_or(0) != 0)) };
                Ok(Object::new("TAniDBLocalHash")
                    .with("Id", i(r.get(0)?))
                    .with("Crc32", s(1)?)
                    .with("Ed2k", s(2)?)
                    .with("Md5", s(3)?)
                    .with("Sha1", s(4)?)
                    .with("Size", Value::Int64(r.get(5)?))
                    .with("FPS", d(6)?)
                    .with("Length", d(7)?)
                    .with("VideoFourCC", s(8)?)
                    .with("VideoCompression", s(9)?)
                    .with("VideoCodec", s(10)?)
                    .with("VideoHeight", n(11)?)
                    .with("VideoWidth", n(12)?)
                    .with("VideoBitrate", d(13)?)
                    .with("VideoVBR", b(14)?)
                    .with("AudioIdentCodec", n(15)?)
                    .with("AudioCodec", s(16)?)
                    .with("AudioSamplerate", n(17)?)
                    .with("AudioBitrate", d(18)?)
                    .with("AudioVBR", b(19)?))
            },
        )
        .optional()
        .ok()
        .flatten()
}

impl DataSource for DbSource<'_> {
    fn anime(&self, aid: i64) -> Option<Object> {
        self.db
            .anime(aid)
            .ok()
            .flatten()
            .map(|a| anime_object(self.db, &a))
    }

    fn episode(&self, eid: i64) -> Option<Object> {
        self.db
            .episode(eid)
            .ok()
            .flatten()
            .map(|e| episode_object(self.db, &e))
    }

    fn language(&self, lang: &str) -> Option<String> {
        let c = self.db.conn();
        let id = crate::rename::value::str_to_int_def(lang, 0);
        if id > 0 {
            return c
                .query_row(
                    "SELECT code FROM category WHERE kind = 'lang' AND id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()
                .ok()
                .flatten()
                .flatten();
        }
        let by_category: Option<String> = c
            .query_row(
                "SELECT COALESCE(code, name) FROM category WHERE kind = 'lang' AND (name = ?1 OR code = ?1) LIMIT 1",
                [lang],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        if by_category.is_some() {
            return by_category;
        }
        // Languages newer than the legacy category table are matched by code directly.
        let known: i64 = c
            .query_row(
                "SELECT (SELECT count(*) FROM anime_title WHERE lang = ?1 LIMIT 1) + (SELECT count(*) FROM episode_title WHERE lang = ?1 LIMIT 1)",
                [lang],
                |r| r.get(0),
            )
            .unwrap_or(0);
        (known > 0).then(|| lang.to_string())
    }

    fn anime_titles(&self, aid: i64) -> Vec<(i64, String, String)> {
        self.db
            .conn()
            .prepare("SELECT type, lang, name FROM anime_title WHERE aid = ?1 ORDER BY id")
            .and_then(|mut st| {
                st.query_map([aid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect()
            })
            .unwrap_or_default()
    }

    fn episode_titles(&self, eid: i64) -> Vec<(String, String)> {
        self.db
            .conn()
            .prepare("SELECT lang, name FROM episode_title WHERE eid = ?1 ORDER BY id")
            .and_then(|mut st| {
                st.query_map([eid], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Build the standard bindings for file `fid` (legacy `TAniDBFile.EvalRenamePattern`).
pub fn bindings_for_file(
    db: &Db,
    fid: i64,
    path: &str,
    base_path: &str,
) -> Result<Bindings, String> {
    let f = db
        .file(fid)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No file entry for file {fid}!"))?;
    let e = db
        .episode(f.eid)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            format!(
                "Can't autorename file, Episode {} is missing for File {fid}!",
                f.eid
            )
        })?;
    let a = db.anime(f.aid).map_err(|e| e.to_string())?.ok_or_else(|| {
        format!(
            "Can't autorename file, Anime {} is missing for File {fid}!",
            f.aid
        )
    })?;
    let g = if f.gid != 0 {
        db.group(f.gid).map_err(|e| e.to_string())?
    } else {
        None
    };
    let m = db.mylist_by_fid(fid).map_err(|e| e.to_string())?;
    let h = f
        .ed2k
        .as_deref()
        .and_then(|ed2k| hash_object(db, ed2k, f.size));
    Ok(Bindings {
        anime: Some(anime_object(db, &a)),
        episode: Some(episode_object(db, &e)),
        file: Some(file_object(&f)),
        group: g.as_ref().map(group_object),
        hash: h,
        mylist: m.as_ref().map(mylist_object),
        path: path.to_string(),
        base_path: base_path.to_string(),
    })
}
