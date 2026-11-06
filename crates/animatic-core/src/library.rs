//! Read model of the library used by the UI: anime, episodes, files, groups, mylist and
//! hashed files, plus the derived per-anime mylist state the legacy client displayed.

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::Result;
type EpisodeSets = (
    HashSet<i64>,
    HashSet<i64>,
    HashSet<i64>,
    HashSet<i64>,
    HashSet<i64>,
);

/// SQL expression giving the display name of anime `a.aid`.
pub const ANIME_NAME_SQL: &str = "(SELECT t.name FROM anime_title t WHERE t.aid = a.aid
    ORDER BY (t.type = 1) DESC, (t.lang = 'x-jat') DESC, (t.lang = 'en') DESC, t.type LIMIT 1)";
/// SQL expression giving the display name of episode `e.eid`.
pub const EPISODE_NAME_SQL: &str = "(SELECT t.name FROM episode_title t WHERE t.eid = e.eid
    ORDER BY (t.lang = 'en') DESC, (t.lang = 'x-jat') DESC, t.id LIMIT 1)";

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord, Default,
)]
pub enum EpisodeType {
    #[default]
    Regular = 1,
    Special = 2,
    Credit = 3,
    Trailer = 4,
    Parody = 5,
    Other = 6,
}

impl EpisodeType {
    pub fn from_i64(v: i64) -> EpisodeType {
        match v {
            2 => EpisodeType::Special,
            3 => EpisodeType::Credit,
            4 => EpisodeType::Trailer,
            5 => EpisodeType::Parody,
            6 => EpisodeType::Other,
            _ => EpisodeType::Regular,
        }
    }

    pub fn prefix(self) -> &'static str {
        match self {
            EpisodeType::Regular => "",
            EpisodeType::Special => "S",
            EpisodeType::Credit => "C",
            EpisodeType::Trailer => "T",
            EpisodeType::Parody => "P",
            EpisodeType::Other => "O",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            EpisodeType::Regular => "Regular",
            EpisodeType::Special => "Special",
            EpisodeType::Credit => "Credit",
            EpisodeType::Trailer => "Trailer",
            EpisodeType::Parody => "Parody",
            EpisodeType::Other => "Other",
        }
    }
}

pub fn epno_string(kind: EpisodeType, number: i64) -> String {
    format!("{}{}", kind.prefix(), number)
}

/// Anime type names of the legacy `catcat` table (ids kept for compatibility).
pub fn anime_type_name(type_id: i64) -> &'static str {
    match type_id {
        2 => "TV Series",
        3 => "OVA",
        4 => "Movie",
        5 => "Other",
        6 => "Web",
        7 => "TV Special",
        8 => "Music Video",
        _ => "Unknown",
    }
}

pub fn anime_type_id(name: &str) -> i64 {
    match name.trim().to_ascii_lowercase().as_str() {
        "tv series" | "tv" => 2,
        "ova" => 3,
        "movie" => 4,
        "other" => 5,
        "web" => 6,
        "tv special" => 7,
        "music video" => 8,
        _ => 1,
    }
}

pub fn mylist_state_name(state: i64) -> &'static str {
    match state {
        1 => "On HDD",
        2 => "On CD",
        3 => "Deleted",
        4 => "Shared",
        5 => "Release",
        _ => "Unknown",
    }
}

/// File version from the AniDB state bit field (bit 2..5 = v2..v5).
pub fn file_version(state: i64) -> i64 {
    match state {
        s if s & 32 != 0 => 5,
        s if s & 16 != 0 => 4,
        s if s & 8 != 0 => 3,
        s if s & 4 != 0 => 2,
        _ => 1,
    }
}

/// CRC status from the AniDB state bits: Some(true) = checked ok, Some(false) = known bad.
pub fn crc_status(state: i64) -> Option<bool> {
    if state & 1 != 0 {
        Some(true)
    } else if state & 2 != 0 {
        Some(false)
    } else {
        None
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Anime {
    pub aid: i64,
    pub name: String,
    pub type_id: i64,
    pub year: String,
    pub episodes: i64,
    pub highest_ep: i64,
    pub special_eps: i64,
    pub air_date: i64,
    pub end_date: i64,
    pub rating: i64,
    pub votes: i64,
    pub temp_rating: i64,
    pub temp_votes: i64,
    pub review_rating: i64,
    pub reviews: i64,
    pub restricted: bool,
    pub picname: Option<String>,
    pub url: Option<String>,
    pub producer: Option<String>,
    pub description: Option<String>,
    pub ann_id: i64,
    pub allcinema_id: i64,
    pub animenfo_id: i64,
    pub animenfo_str: Option<String>,
    pub awards: Option<String>,
    pub updated: i64,
    pub fetched: i64,
}

impl Anime {
    pub fn type_name(&self) -> &'static str {
        anime_type_name(self.type_id)
    }

    /// Episode count used for completeness: total if known, else highest known number.
    pub fn effective_episodes(&self) -> i64 {
        if self.episodes > 0 {
            self.episodes
        } else {
            self.highest_ep
        }
    }

    fn from_row(r: &Row) -> rusqlite::Result<Anime> {
        Ok(Anime {
            aid: r.get("aid")?,
            name: r.get::<_, Option<String>>("name")?.unwrap_or_default(),
            type_id: r.get("type_id")?,
            year: r.get::<_, Option<String>>("year")?.unwrap_or_default(),
            episodes: r.get("episodes")?,
            highest_ep: r.get("highest_ep")?,
            special_eps: r.get("special_eps")?,
            air_date: r.get("air_date")?,
            end_date: r.get("end_date")?,
            rating: r.get("rating")?,
            votes: r.get("votes")?,
            temp_rating: r.get("temp_rating")?,
            temp_votes: r.get("temp_votes")?,
            review_rating: r.get("review_rating")?,
            reviews: r.get("reviews")?,
            restricted: r.get::<_, i64>("restricted")? != 0,
            picname: r.get("picname")?,
            url: r.get("url")?,
            producer: r.get("producer")?,
            description: r.get("description")?,
            ann_id: r.get("ann_id")?,
            allcinema_id: r.get("allcinema_id")?,
            animenfo_id: r.get("animenfo_id")?,
            animenfo_str: r.get("animenfo_str")?,
            awards: r.get("awards")?,
            updated: r.get("updated")?,
            fetched: r.get("fetched")?,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Episode {
    pub eid: i64,
    pub aid: i64,
    pub epno: i64,
    pub kind: EpisodeType,
    pub length: i64,
    pub aired: i64,
    pub rating: i64,
    pub votes: i64,
    pub name: String,
    pub other: Option<String>,
    pub recap: bool,
}

impl Episode {
    pub fn epno_str(&self) -> String {
        epno_string(self.kind, self.epno)
    }

    fn from_row(r: &Row) -> rusqlite::Result<Episode> {
        Ok(Episode {
            eid: r.get("eid")?,
            aid: r.get("aid")?,
            epno: r.get("epno")?,
            kind: EpisodeType::from_i64(r.get("type")?),
            length: r.get("length")?,
            aired: r.get("aired")?,
            rating: r.get("rating")?,
            votes: r.get("votes")?,
            name: r.get::<_, Option<String>>("name")?.unwrap_or_default(),
            other: r.get("other")?,
            recap: r.get::<_, i64>("recap")? != 0,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileRec {
    pub fid: i64,
    pub aid: i64,
    pub eid: i64,
    pub gid: i64,
    pub size: i64,
    pub ed2k: Option<String>,
    pub md5: Option<String>,
    pub sha1: Option<String>,
    pub crc32: Option<String>,
    pub filetype: Option<String>,
    pub released: i64,
    pub quality: String,
    pub source: String,
    pub file_type: i64,
    pub other: Option<String>,
    pub state: i64,
    pub user_count: i64,
    pub length: i64,
    pub deprecated: bool,
    pub anidb_filename: Option<String>,
    pub video_codec: Option<String>,
    pub resolution: Option<String>,
    pub audio_codecs: Option<String>,
    pub dub_langs: Option<String>,
    pub sub_langs: Option<String>,
    pub group_name: Option<String>,
    pub group_short: Option<String>,
    pub added: i64,
    /// Start/end percentage when this file covers only part of the episode.
    pub start_pct: i64,
    pub end_pct: i64,
}

impl FileRec {
    pub fn version(&self) -> i64 {
        file_version(self.state)
    }

    pub fn crc_status(&self) -> Option<bool> {
        crc_status(self.state)
    }

    pub fn group_label(&self) -> String {
        self.group_short
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| self.group_name.clone())
            .unwrap_or_default()
    }

    fn from_row(r: &Row) -> rusqlite::Result<FileRec> {
        Ok(FileRec {
            fid: r.get("fid")?,
            aid: r.get("aid")?,
            eid: r.get("eid")?,
            gid: r.get("gid")?,
            size: r.get("size")?,
            ed2k: r.get("ed2k")?,
            md5: r.get("md5")?,
            sha1: r.get("sha1")?,
            crc32: r.get("crc32")?,
            filetype: r.get("filetype")?,
            released: r.get("released")?,
            quality: r.get::<_, Option<String>>("quality")?.unwrap_or_default(),
            source: r.get::<_, Option<String>>("source")?.unwrap_or_default(),
            file_type: r.get("file_type")?,
            other: r.get("other")?,
            state: r.get("state")?,
            user_count: r.get("user_count")?,
            length: r.get("length")?,
            deprecated: r.get::<_, i64>("deprecated")? != 0,
            anidb_filename: r.get("anidb_filename")?,
            video_codec: r.get("video_codec")?,
            resolution: r.get("resolution")?,
            audio_codecs: r.get("audio_codecs")?,
            dub_langs: r.get("dub_langs")?,
            sub_langs: r.get("sub_langs")?,
            group_name: r.get("group_name")?,
            group_short: r.get("group_short")?,
            added: r.get("added")?,
            start_pct: r.get("start_pct")?,
            end_pct: r.get("end_pct")?,
        })
    }
}

const FILE_SELECT: &str = "SELECT f.fid, f.aid, f.eid, f.gid, f.size, f.ed2k, f.md5, f.sha1, f.crc32, f.filetype,
    f.released, (SELECT name FROM category WHERE kind = 'qual' AND id = f.quality_id) AS quality,
    (SELECT name FROM category WHERE kind = 'type' AND id = f.source_id) AS source, f.file_type, f.other, f.state,
    f.user_count, f.length, f.deprecated, f.anidb_filename, f.video_codec, f.resolution, f.audio_codecs, f.dub_langs,
    f.sub_langs, g.name AS group_name, g.short AS group_short, f.added";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Group {
    pub gid: i64,
    pub name: String,
    pub short: String,
    pub url: Option<String>,
    pub email: Option<String>,
    pub irc_server: Option<String>,
    pub irc_channel: Option<String>,
    pub other: Option<String>,
    pub rating: i64,
    pub votes: i64,
    pub added: i64,
    pub updated: i64,
}

impl Group {
    fn from_row(r: &Row) -> rusqlite::Result<Group> {
        Ok(Group {
            gid: r.get("gid")?,
            name: r.get::<_, Option<String>>("name")?.unwrap_or_default(),
            short: r.get::<_, Option<String>>("short")?.unwrap_or_default(),
            url: r.get("url")?,
            email: r.get("email")?,
            irc_server: r.get("irc_server")?,
            irc_channel: r.get("irc_channel")?,
            other: r.get("other")?,
            rating: r.get("rating")?,
            votes: r.get("votes")?,
            added: r.get("added")?,
            updated: r.get("updated")?,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MylistEntry {
    pub lid: i64,
    pub fid: i64,
    pub eid: i64,
    pub aid: i64,
    pub gid: i64,
    pub date: i64,
    pub state: i64,
    pub viewed: bool,
    pub viewdate: i64,
    pub storage: Option<String>,
    pub source: Option<String>,
    pub other: Option<String>,
    pub filestate: i64,
}

impl MylistEntry {
    fn from_row(r: &Row) -> rusqlite::Result<MylistEntry> {
        Ok(MylistEntry {
            lid: r.get("lid")?,
            fid: r.get("fid")?,
            eid: r.get("eid")?,
            aid: r.get("aid")?,
            gid: r.get("gid")?,
            date: r.get("date")?,
            state: r.get("state")?,
            viewed: r.get::<_, i64>("viewed")? != 0,
            viewdate: r.get("viewdate")?,
            storage: r.get("storage")?,
            source: r.get("source")?,
            other: r.get("other")?,
            filestate: r.get("filestate")?,
        })
    }
}

/// One mylist entry joined with everything the mylist tree displays.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MylistItem {
    pub entry: MylistEntry,
    pub file: Option<FileRec>,
    pub episode: Option<Episode>,
}

/// Anime-level aggregate of the mylist (what the legacy client computed per anime node).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnimeMylistState {
    pub aid: i64,
    /// Distinct regular episodes in mylist.
    pub eps: i64,
    pub specials: i64,
    pub watched_eps: i64,
    pub watched_specials: i64,
    pub files: i64,
    pub watched_files: i64,
    pub size: i64,
    pub watched_size: i64,
    /// Seconds (episode length × files).
    pub length: i64,
    pub watched_length: i64,
    /// Distinct regular episodes available (on HDD/CD, not deleted).
    pub available_eps: i64,
    pub states: [i64; 6],
}

impl AnimeMylistState {
    pub fn complete(&self, anime: &Anime) -> bool {
        let total = anime.effective_episodes();
        total > 0 && self.eps >= total
    }

    pub fn all_watched(&self, anime: &Anime) -> bool {
        let total = anime.effective_episodes();
        total > 0 && self.watched_eps >= total
    }
}

/// Local hashed file with AniDB identification.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocalFile {
    pub id: i64,
    pub hash_id: i64,
    pub path: String,
    pub mtime: i64,
    pub size: i64,
    pub ed2k: String,
    pub crc32: Option<String>,
    pub md5: Option<String>,
    pub sha1: Option<String>,
    pub fid: Option<i64>,
    pub aid: Option<i64>,
    pub eid: Option<i64>,
    pub lid: Option<i64>,
    pub mylist_state: Option<i64>,
    pub viewed: bool,
    pub anidb_crc: Option<String>,
    pub file_state: i64,
}

impl LocalFile {
    pub fn known(&self) -> bool {
        self.fid.is_some()
    }

    pub fn in_mylist(&self) -> bool {
        self.lid.is_some()
    }

    /// True when AniDB knows a CRC for the file and it differs from the local hash.
    pub fn crc_mismatch(&self) -> bool {
        match (&self.crc32, &self.anidb_crc) {
            (Some(a), Some(b)) if !b.is_empty() => !a.eq_ignore_ascii_case(b),
            _ => false,
        }
    }

    pub fn file_name(&self) -> &str {
        self.path.rsplit(['/', '\\']).next().unwrap_or(&self.path)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Review {
    pub id: i64,
    pub aid: i64,
    pub uid: i64,
    pub uname: String,
    pub vote: i64,
    pub vote_anim: i64,
    pub vote_sound: i64,
    pub vote_story: i64,
    pub vote_char: i64,
    pub vote_value: i64,
    pub vote_enjoy: i64,
    pub rating: i64,
    pub votes: i64,
    pub body: String,
    pub added: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Title {
    pub name: String,
    pub kind: i64,
    pub lang: String,
}

pub fn title_kind_name(kind: i64) -> &'static str {
    match kind {
        1 => "Main",
        2 => "Synonym",
        3 => "Short",
        4 => "Official",
        5 => "Card",
        6 => "Kana",
        _ => "Other",
    }
}

pub fn title_kind_id(name: &str) -> i64 {
    match name {
        "main" => 1,
        "synonym" | "syn" => 2,
        "short" => 3,
        "official" => 4,
        "card" => 5,
        "kana" => 6,
        _ => 2,
    }
}

pub fn relation_name(t: i64) -> &'static str {
    match t {
        1 => "Sequel",
        2 => "Prequel",
        11 => "Same setting",
        12 => "Alternative setting",
        21 | 22 => "Alternative setting",
        31 | 32 => "Alternative version",
        41 => "Music video",
        42 => "Character",
        51 => "Side story",
        52 => "Parent story",
        61 => "Summary",
        62 => "Full story",
        _ => "Other",
    }
}

pub fn relation_id(name: &str) -> i64 {
    match name.trim().to_ascii_lowercase().as_str() {
        "sequel" => 1,
        "prequel" => 2,
        "same setting" => 11,
        "alternative setting" => 12,
        "alternative version" => 31,
        "music video" => 41,
        "character" => 42,
        "side story" => 51,
        "parent story" => 52,
        "summary" => 61,
        "full story" => 62,
        _ => 100,
    }
}

#[derive(Debug, Clone, Default)]
pub struct AnimeDetail {
    pub anime: Anime,
    pub titles: Vec<Title>,
    pub genres: Vec<(i64, String)>,
    /// (related aid, relation type, related name)
    pub relations: Vec<(i64, i64, String)>,
    pub reviews: Vec<Review>,
    pub episodes: Vec<Episode>,
}

impl Db {
    pub fn anime(&self, aid: i64) -> Result<Option<Anime>> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT a.*, {ANIME_NAME_SQL} AS name FROM anime a WHERE a.aid = ?1"),
                [aid],
                Anime::from_row,
            )
            .optional()?)
    }

    /// All anime with display names.
    pub fn all_anime(&self) -> Result<Vec<Anime>> {
        let mut st = self.conn().prepare(&format!(
            "SELECT a.*, {ANIME_NAME_SQL} AS name FROM anime a"
        ))?;
        let v = st
            .query_map([], Anime::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn anime_detail(&self, aid: i64) -> Result<Option<AnimeDetail>> {
        let Some(anime) = self.anime(aid)? else {
            return Ok(None);
        };
        let c = self.conn();
        let titles = c
            .prepare(
                "SELECT name, type, lang FROM anime_title WHERE aid = ?1 ORDER BY type, lang, name",
            )?
            .query_map([aid], |r| {
                Ok(Title {
                    name: r.get(0)?,
                    kind: r.get(1)?,
                    lang: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let genres = c
            .prepare(
                "SELECT g.id, g.name FROM anime_genre ag JOIN genre g ON g.id = ag.genre_id WHERE ag.aid = ?1
                 ORDER BY ag.weight DESC, g.name",
            )?
            .query_map([aid], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let relations = c
            .prepare(&format!(
                "SELECT r.related_aid, r.type, COALESCE({}, 'aid ' || r.related_aid)
                 FROM anime_relation r LEFT JOIN anime a ON a.aid = r.related_aid WHERE r.aid = ?1 ORDER BY r.type",
                ANIME_NAME_SQL
            ))?
            .query_map([aid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let reviews = c
            .prepare("SELECT * FROM review WHERE aid = ?1 ORDER BY rating DESC, votes DESC")?
            .query_map([aid], |r| {
                Ok(Review {
                    id: r.get("id")?,
                    aid: r.get("aid")?,
                    uid: r.get("uid")?,
                    uname: r.get::<_, Option<String>>("uname")?.unwrap_or_default(),
                    vote: r.get("vote")?,
                    vote_anim: r.get("vote_anim")?,
                    vote_sound: r.get("vote_sound")?,
                    vote_story: r.get("vote_story")?,
                    vote_char: r.get("vote_char")?,
                    vote_value: r.get("vote_value")?,
                    vote_enjoy: r.get("vote_enjoy")?,
                    rating: r.get("rating")?,
                    votes: r.get("votes")?,
                    body: r.get::<_, Option<String>>("body")?.unwrap_or_default(),
                    added: r.get("added")?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let episodes = self.episodes_of(aid)?;
        Ok(Some(AnimeDetail {
            anime,
            titles,
            genres,
            relations,
            reviews,
            episodes,
        }))
    }

    pub fn episodes_of(&self, aid: i64) -> Result<Vec<Episode>> {
        let mut st = self
            .conn()
            .prepare(&format!("SELECT e.*, {EPISODE_NAME_SQL} AS name FROM episode e WHERE e.aid = ?1 ORDER BY e.type, e.epno"))?;
        let v = st
            .query_map([aid], Episode::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn episode(&self, eid: i64) -> Result<Option<Episode>> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT e.*, {EPISODE_NAME_SQL} AS name FROM episode e WHERE e.eid = ?1"),
                [eid],
                Episode::from_row,
            )
            .optional()?)
    }

    pub fn episode_titles(&self, eid: i64) -> Result<Vec<Title>> {
        let v = self
            .conn()
            .prepare("SELECT name, lang FROM episode_title WHERE eid = ?1 ORDER BY lang")?
            .query_map([eid], |r| {
                Ok(Title {
                    name: r.get(0)?,
                    kind: 0,
                    lang: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// Title of an anime in the given language (used by the rename `at()` function).
    pub fn anime_title_lang(&self, aid: i64, lang: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT name FROM anime_title WHERE aid = ?1 AND lang = ?2 ORDER BY (type = 1) DESC, (type = 4) DESC, type, id LIMIT 1",
                params![aid, lang],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn episode_title_lang(&self, eid: i64, lang: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT name FROM episode_title WHERE eid = ?1 AND lang = ?2 ORDER BY id LIMIT 1",
                params![eid, lang],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn file(&self, fid: i64) -> Result<Option<FileRec>> {
        let sql = format!(
            "{FILE_SELECT}, 0 AS start_pct, 100 AS end_pct FROM file f LEFT JOIN grp g ON g.gid = f.gid WHERE f.fid = ?1"
        );
        Ok(self
            .conn()
            .query_row(&sql, [fid], FileRec::from_row)
            .optional()?)
    }

    pub fn file_by_hash(&self, ed2k: &str, size: i64) -> Result<Option<FileRec>> {
        let sql = format!(
            "{FILE_SELECT}, 0 AS start_pct, 100 AS end_pct FROM file f LEFT JOIN grp g ON g.gid = f.gid
             WHERE f.ed2k = ?1 AND f.size = ?2 ORDER BY f.deprecated, f.fid DESC LIMIT 1"
        );
        Ok(self
            .conn()
            .query_row(
                &sql,
                params![ed2k.to_ascii_lowercase(), size],
                FileRec::from_row,
            )
            .optional()?)
    }

    /// Files of an episode, including files that cover it partially (file_episode relation).
    pub fn files_of_episode(&self, eid: i64) -> Result<Vec<FileRec>> {
        let sql = format!(
            "{FILE_SELECT}, 0 AS start_pct, 100 AS end_pct FROM file f LEFT JOIN grp g ON g.gid = f.gid WHERE f.eid = ?1
             UNION ALL
             {FILE_SELECT}, fe.start_pct, fe.end_pct FROM file_episode fe JOIN file f ON f.fid = fe.fid
               LEFT JOIN grp g ON g.gid = f.gid WHERE fe.eid = ?1 AND f.eid <> ?1"
        );
        let mut st = self.conn().prepare(&sql)?;
        let mut v = st
            .query_map([eid], FileRec::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v.sort_by(|a, b| b.user_count.cmp(&a.user_count).then(a.fid.cmp(&b.fid)));
        Ok(v)
    }

    pub fn files_of_anime(&self, aid: i64) -> Result<Vec<FileRec>> {
        let sql = format!(
            "{FILE_SELECT}, 0 AS start_pct, 100 AS end_pct FROM file f LEFT JOIN grp g ON g.gid = f.gid WHERE f.aid = ?1"
        );
        let mut st = self.conn().prepare(&sql)?;
        let v = st
            .query_map([aid], FileRec::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn files_of_group(&self, gid: i64) -> Result<Vec<FileRec>> {
        let sql = format!(
            "{FILE_SELECT}, 0 AS start_pct, 100 AS end_pct FROM file f LEFT JOIN grp g ON g.gid = f.gid WHERE f.gid = ?1"
        );
        let mut st = self.conn().prepare(&sql)?;
        let v = st
            .query_map([gid], FileRec::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// Anime a group released files for, with file counts.
    pub fn anime_of_group(&self, gid: i64) -> Result<Vec<(i64, String, i64)>> {
        let sql = format!(
            "SELECT a.aid, COALESCE({ANIME_NAME_SQL}, 'aid ' || x.aid), x.n FROM
               (SELECT aid, count(*) AS n FROM file WHERE gid = ?1 GROUP BY aid) x
             LEFT JOIN anime a ON a.aid = x.aid ORDER BY 2"
        );
        let v = self
            .conn()
            .prepare(&sql)?
            .query_map([gid], |r| {
                Ok((
                    r.get::<_, Option<i64>>(0)?.unwrap_or(0),
                    r.get(1)?,
                    r.get(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn group(&self, gid: i64) -> Result<Option<Group>> {
        Ok(self
            .conn()
            .query_row("SELECT * FROM grp WHERE gid = ?1", [gid], Group::from_row)
            .optional()?)
    }

    pub fn all_groups(&self) -> Result<Vec<Group>> {
        let v = self
            .conn()
            .prepare("SELECT * FROM grp ORDER BY name COLLATE NOCASE")?
            .query_map([], Group::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// (genre id, name, anime count)
    pub fn genres(&self) -> Result<Vec<(i64, String, i64)>> {
        let v = self
            .conn()
            .prepare(
                "SELECT g.id, g.name, (SELECT count(*) FROM anime_genre ag WHERE ag.genre_id = g.id)
                 FROM genre g ORDER BY g.name COLLATE NOCASE",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn anime_ids_with_genre(&self, genre_id: i64) -> Result<Vec<i64>> {
        let v = self
            .conn()
            .prepare("SELECT DISTINCT aid FROM anime_genre WHERE genre_id = ?1")?
            .query_map([genre_id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// aid -> genre ids, for search filters.
    pub fn anime_genre_map(&self) -> Result<HashMap<i64, Vec<i64>>> {
        let mut map: HashMap<i64, Vec<i64>> = HashMap::new();
        let mut st = self
            .conn()
            .prepare("SELECT aid, genre_id FROM anime_genre")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            map.entry(r.get(0)?).or_default().push(r.get(1)?);
        }
        Ok(map)
    }

    pub fn mylist_entry(&self, lid: i64) -> Result<Option<MylistEntry>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT * FROM mylist WHERE lid = ?1",
                [lid],
                MylistEntry::from_row,
            )
            .optional()?)
    }

    pub fn mylist_by_fid(&self, fid: i64) -> Result<Option<MylistEntry>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT * FROM mylist WHERE fid = ?1 ORDER BY lid LIMIT 1",
                [fid],
                MylistEntry::from_row,
            )
            .optional()?)
    }

    /// The complete mylist with file and episode data (anime data is looked up separately).
    pub fn mylist_items(&self) -> Result<Vec<MylistItem>> {
        let c = self.conn();
        let entries = c
            .prepare("SELECT * FROM mylist")?
            .query_map([], MylistEntry::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut files: HashMap<i64, FileRec> = HashMap::new();
        {
            let sql = format!(
                "{FILE_SELECT}, 0 AS start_pct, 100 AS end_pct FROM mylist m JOIN file f ON f.fid = m.fid
                 LEFT JOIN grp g ON g.gid = f.gid"
            );
            let mut st = c.prepare(&sql)?;
            for f in st.query_map([], FileRec::from_row)? {
                let f = f?;
                files.insert(f.fid, f);
            }
        }
        let mut episodes: HashMap<i64, Episode> = HashMap::new();
        {
            let sql = format!(
                "SELECT e.*, {EPISODE_NAME_SQL} AS name FROM episode e WHERE e.eid IN (SELECT eid FROM mylist)"
            );
            let mut st = c.prepare(&sql)?;
            for e in st.query_map([], Episode::from_row)? {
                let e = e?;
                episodes.insert(e.eid, e);
            }
        }
        Ok(entries
            .into_iter()
            .map(|entry| {
                let file = files.get(&entry.fid).cloned();
                let eid = if entry.eid != 0 {
                    entry.eid
                } else {
                    file.as_ref().map_or(0, |f| f.eid)
                };
                let episode = episodes.get(&eid).cloned();
                MylistItem {
                    entry,
                    file,
                    episode,
                }
            })
            .collect())
    }

    /// Per-anime aggregates of the mylist.
    pub fn mylist_state_by_anime(&self, items: &[MylistItem]) -> HashMap<i64, AnimeMylistState> {
        let mut out: HashMap<i64, AnimeMylistState> = HashMap::new();
        let mut eps: HashMap<i64, EpisodeSets> = HashMap::new();
        for it in items {
            let e = &it.entry;
            let s = out.entry(e.aid).or_insert_with(|| AnimeMylistState {
                aid: e.aid,
                ..Default::default()
            });
            s.files += 1;
            let size = it.file.as_ref().map_or(0, |f| f.size);
            let len = item_length(it);
            s.size += size;
            s.length += len;
            if (0..6).contains(&e.state) {
                s.states[e.state as usize] += 1;
            }
            if e.viewed {
                s.watched_files += 1;
                s.watched_size += size;
                s.watched_length += len;
            }
            let sets = eps.entry(e.aid).or_default();
            if let Some(ep) = &it.episode {
                match ep.kind {
                    EpisodeType::Regular => {
                        sets.0.insert(ep.eid);
                        if e.viewed {
                            sets.1.insert(ep.eid);
                        }
                        if e.state != 3 {
                            sets.4.insert(ep.eid);
                        }
                    }
                    _ => {
                        sets.2.insert(ep.eid);
                        if e.viewed {
                            sets.3.insert(ep.eid);
                        }
                    }
                }
            }
        }
        for (aid, (n, w, sp, wsp, avail)) in eps {
            if let Some(s) = out.get_mut(&aid) {
                s.eps = n.len() as i64;
                s.watched_eps = w.len() as i64;
                s.specials = sp.len() as i64;
                s.watched_specials = wsp.len() as i64;
                s.available_eps = avail.len() as i64;
            }
        }
        out
    }

    pub fn local_files(&self) -> Result<Vec<LocalFile>> {
        let sql = "SELECT l.id, l.hash_id, l.path, l.mtime, h.size, h.ed2k, h.crc32, h.md5, h.sha1,
                f.fid, f.aid, f.eid, m.lid, m.state AS mstate, COALESCE(m.viewed, 0) AS viewed, f.crc32 AS acrc, COALESCE(f.state, 0) AS fstate
            FROM local_file l
            JOIN local_hash h ON h.id = l.hash_id
            LEFT JOIN file f ON f.fid = (SELECT fid FROM file f2 WHERE f2.ed2k = h.ed2k AND f2.size = h.size ORDER BY f2.deprecated, f2.fid DESC LIMIT 1)
            LEFT JOIN mylist m ON m.lid = (SELECT lid FROM mylist m2 WHERE m2.fid = f.fid ORDER BY m2.lid LIMIT 1)
            ORDER BY l.path";
        let v = self
            .conn()
            .prepare(sql)?
            .query_map([], |r| {
                Ok(LocalFile {
                    id: r.get(0)?,
                    hash_id: r.get(1)?,
                    path: r.get(2)?,
                    mtime: r.get(3)?,
                    size: r.get(4)?,
                    ed2k: r.get(5)?,
                    crc32: r.get(6)?,
                    md5: r.get(7)?,
                    sha1: r.get(8)?,
                    fid: r.get(9)?,
                    aid: r.get(10)?,
                    eid: r.get(11)?,
                    lid: r.get(12)?,
                    mylist_state: r.get(13)?,
                    viewed: r.get::<_, i64>(14)? != 0,
                    anidb_crc: r.get(15)?,
                    file_state: r.get(16)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// Expand `template` once per hashed local file (in path order), one line each, in a single
    /// read of the database. Returns the text and the number of files.
    pub fn local_hashes_export_text(
        &self,
        template: &str,
        copy_length_as_minutes: bool,
    ) -> Result<(String, usize)> {
        let files = self.local_files()?;
        let mut text = String::new();
        for f in &files {
            let hashes = crate::hash::Hashes {
                size: f.size as u64,
                ed2k: f.ed2k.clone(),
                crc32: f.crc32.clone().unwrap_or_default(),
                md5: f.md5.clone().unwrap_or_default(),
                sha1: f.sha1.clone().unwrap_or_default(),
            };
            text.push_str(&hashes.export(
                std::path::Path::new(&f.path),
                template,
                copy_length_as_minutes,
            ));
            text.push_str("\r\n");
        }
        Ok((text, files.len()))
    }

    /// Write the hash export for every hashed local file to `path`, replacing it atomically
    /// (see [`crate::atomic::write_atomic`]): on failure the previous file stays intact.
    /// Returns the number of files exported.
    pub fn export_local_hashes(
        &self,
        path: &std::path::Path,
        template: &str,
        copy_length_as_minutes: bool,
    ) -> Result<usize> {
        let (text, count) = self.local_hashes_export_text(template, copy_length_as_minutes)?;
        crate::atomic::write_atomic(path, text.as_bytes())?;
        Ok(count)
    }

    /// Local paths of a file (by fid) that are known on disk.
    pub fn local_paths_of_fid(&self, fid: i64) -> Result<Vec<String>> {
        let v = self
            .conn()
            .prepare(
                "SELECT l.path FROM local_file l JOIN local_hash h ON h.id = l.hash_id
                 JOIN file f ON f.ed2k = h.ed2k AND f.size = h.size WHERE f.fid = ?1",
            )?
            .query_map([fid], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn local_hash_by_fid(&self, fid: i64) -> Result<Option<crate::hash::Hashes>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT h.size, h.ed2k, h.crc32, h.md5, h.sha1 FROM local_hash h JOIN file f ON f.ed2k = h.ed2k AND f.size = h.size
                 WHERE f.fid = ?1 LIMIT 1",
                [fid],
                |r| {
                    Ok(crate::hash::Hashes {
                        size: r.get::<_, i64>(0)? as u64,
                        ed2k: r.get(1)?,
                        crc32: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                        md5: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                        sha1: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    })
                },
            )
            .optional()?)
    }

    /// Categories of a kind as id -> name.
    pub fn categories(&self, kind: &str) -> Result<BTreeMap<i64, String>> {
        let mut st = self
            .conn()
            .prepare("SELECT id, name FROM category WHERE kind = ?1 ORDER BY sort_order, id")?;
        let v = st
            .query_map([kind], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
        Ok(v)
    }

    pub fn category_id_by_name(&self, kind: &str, name: &str) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id FROM category WHERE kind = ?1 AND lower(name) = lower(?2)",
                params![kind, name],
                |r| r.get(0),
            )
            .optional()?)
    }
}

/// Playing time of one mylist item in seconds: file length if known, else episode length.
pub fn item_length(it: &MylistItem) -> i64 {
    if let Some(f) = &it.file
        && f.length > 0
    {
        return f.length;
    }
    it.episode.as_ref().map_or(0, |e| e.length * 60)
}

/// Aggregates the overview page shows, computed once per projection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LibrarySummary {
    /// Hashed files AniDB has identified.
    pub local_known: usize,
    /// Hashed files whose local CRC differs from AniDB's.
    pub local_crc_mismatch: usize,
    /// Total size of hashed files in bytes.
    pub local_bytes: i64,
    /// Mylist entries marked watched.
    pub mylist_watched: usize,
    /// Queue entries blocked pending correction or an explicit retry.
    pub queue_blocked: usize,
}

/// Lower-cased search keys parallel to the projection tables, built once off the UI thread so
/// filtering never allocates per row.
#[derive(Debug, Clone, Default)]
pub struct SearchKeys {
    /// Lower-cased display name of `LibraryProjection::anime[i]`.
    pub anime: Vec<String>,
    /// Index into `anime` of the anime of `mylist[i]`, if known.
    pub mylist_anime: Vec<Option<usize>>,
    /// Lower-cased title shown for `mylist[i]`.
    pub mylist: Vec<String>,
    /// Lower-cased path of `local[i]`.
    pub local: Vec<String>,
    /// Lower-cased (name, short name) of `groups[i]`.
    pub groups: Vec<(String, String)>,
    /// Lower-cased name of `genres[i]`.
    pub genres: Vec<String>,
}

/// Owned, immutable snapshot of everything the UI tables display. Loaded from one consistent
/// database read transaction on a background thread and swapped into the UI whole.
#[derive(Debug, Clone, Default)]
pub struct LibraryProjection {
    pub anime: Vec<Anime>,
    pub mylist: Vec<MylistItem>,
    pub states: HashMap<i64, AnimeMylistState>,
    pub local: Vec<LocalFile>,
    pub groups: Vec<Group>,
    /// (genre id, name, anime count)
    pub genres: Vec<(i64, String, i64)>,
    pub queue: Vec<crate::sync::QueueEntry>,
    /// Active sync cooldown as (epoch seconds until, reason).
    pub cooldown: Option<(i64, String)>,
    pub summary: LibrarySummary,
    pub keys: SearchKeys,
}

impl LibraryProjection {
    /// Read every table of the projection inside one read transaction.
    pub fn load(db: &Db) -> Result<LibraryProjection> {
        let tx = db.conn().unchecked_transaction()?;
        let anime = db.all_anime()?;
        let mylist = db.mylist_items()?;
        let states = db.mylist_state_by_anime(&mylist);
        let local = db.local_files()?;
        let groups = db.all_groups()?;
        let genres = db.genres()?;
        let queue = db.queue_list()?;
        let cooldown = db.sync_cooldown()?;
        drop(tx);
        Ok(Self::assemble(
            anime, mylist, states, local, groups, genres, queue, cooldown,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(
        anime: Vec<Anime>,
        mylist: Vec<MylistItem>,
        states: HashMap<i64, AnimeMylistState>,
        local: Vec<LocalFile>,
        groups: Vec<Group>,
        genres: Vec<(i64, String, i64)>,
        queue: Vec<crate::sync::QueueEntry>,
        cooldown: Option<(i64, String)>,
    ) -> LibraryProjection {
        let by_aid: HashMap<i64, usize> =
            anime.iter().enumerate().map(|(i, a)| (a.aid, i)).collect();
        let mylist_anime: Vec<Option<usize>> = mylist
            .iter()
            .map(|m| by_aid.get(&m.entry.aid).copied())
            .collect();
        let mut keys = SearchKeys {
            anime: anime.iter().map(|a| a.name.to_ascii_lowercase()).collect(),
            mylist: Vec::with_capacity(mylist.len()),
            local: local.iter().map(|f| f.path.to_ascii_lowercase()).collect(),
            groups: groups
                .iter()
                .map(|g| (g.name.to_ascii_lowercase(), g.short.to_ascii_lowercase()))
                .collect(),
            genres: genres
                .iter()
                .map(|(_, name, _)| name.to_ascii_lowercase())
                .collect(),
            mylist_anime,
        };
        for (item, idx) in mylist.iter().zip(&keys.mylist_anime) {
            let title = mylist_title(item, idx.map(|i| &anime[i]));
            keys.mylist.push(title.to_ascii_lowercase());
        }
        let summary = LibrarySummary {
            local_known: local.iter().filter(|f| f.known()).count(),
            local_crc_mismatch: local.iter().filter(|f| f.crc_mismatch()).count(),
            local_bytes: local.iter().map(|f| f.size).sum(),
            mylist_watched: mylist.iter().filter(|m| m.entry.viewed).count(),
            queue_blocked: queue.iter().filter(|e| e.blocked).count(),
        };
        LibraryProjection {
            anime,
            mylist,
            states,
            local,
            groups,
            genres,
            queue,
            cooldown,
            summary,
            keys,
        }
    }

    /// Title displayed for `mylist[i]`.
    pub fn mylist_title(&self, i: usize) -> String {
        mylist_title(
            &self.mylist[i],
            self.keys.mylist_anime[i].map(|a| &self.anime[a]),
        )
    }
}

fn mylist_title(item: &MylistItem, anime: Option<&Anime>) -> String {
    match anime {
        None => "Unknown anime".into(),
        Some(a) if a.name.is_empty() => format!("aid {}", item.entry.aid),
        Some(a) => a.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample_db() -> Db {
        let db = Db::memory().unwrap();
        db.conn()
            .execute_batch(
                "INSERT INTO category VALUES ('qual', 2, 'very high', 0, 2, NULL), ('type', 16, 'Blu-ray', 0, 13, NULL);
                 INSERT INTO anime(aid, type_id, year, episodes) VALUES (1, 2, '1999', 2);
                 INSERT INTO anime_title(aid, name, type, lang) VALUES (1, 'Seikai no Monshou', 1, 'x-jat'),
                    (1, 'Crest of the Stars', 4, 'en');
                 INSERT INTO episode(eid, aid, epno, type, length) VALUES (10, 1, 1, 1, 25), (11, 1, 2, 1, 25), (12, 1, 1, 2, 5);
                 INSERT INTO episode_title(eid, name, lang) VALUES (10, 'Invasion', 'en'), (10, 'Shinryaku', 'x-jat');
                 INSERT INTO grp(gid, name, short) VALUES (7, 'Anime Group', 'AG');
                 INSERT INTO file(fid, aid, eid, gid, size, ed2k, crc32, quality_id, source_id, state, user_count)
                    VALUES (100, 1, 10, 7, 1000, 'aa', 'deadbeef', 2, 16, 1|4, 5),
                           (101, 1, 11, 7, 2000, 'bb', NULL, 2, 16, 0, 9),
                           (102, 1, 12, 0, 300, 'cc', NULL, 1, 1, 0, 1);
                 INSERT INTO file_episode(fid, eid, start_pct, end_pct) VALUES (101, 10, 50, 100);
                 INSERT INTO mylist(lid, fid, eid, aid, gid, state, viewed, viewdate) VALUES
                    (1000, 100, 10, 1, 7, 1, 1, 5), (1001, 101, 11, 1, 7, 3, 0, 0), (1002, 102, 12, 1, 0, 1, 1, 9);
                 INSERT INTO local_hash(id, ed2k, size, crc32) VALUES (1, 'aa', 1000, 'DEADBEEF'), (2, 'zz', 5, 'X');
                 INSERT INTO local_file(id, hash_id, path) VALUES (1, 1, '/a/x.mkv'), (2, 2, '/a/unknown.mkv');",
            )
            .unwrap();
        db
    }

    #[test]
    fn hash_export_writes_one_line_per_local_file_and_keeps_the_old_file_on_failure() {
        let db = sample_db();
        let dir =
            std::env::temp_dir().join(format!("animatic-hash-export-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("animatic-hashes.txt");
        std::fs::write(&target, "previous").unwrap();
        let count = db
            .export_local_hashes(&target, "%filename% %crc32%", false)
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "unknown.mkv X\r\nx.mkv DEADBEEF\r\n"
        );

        let blocked = dir.join("blocked.txt");
        std::fs::create_dir(&blocked).unwrap();
        assert!(db.export_local_hashes(&blocked, "%ed2k%", false).is_err());
        assert!(blocked.is_dir());
        assert_eq!(std::fs::read_to_string(&target).unwrap().lines().count(), 2);

        db.conn().execute_batch("DROP TABLE local_file;").unwrap();
        assert!(db.export_local_hashes(&target, "%ed2k%", false).is_err());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap().lines().count(),
            2,
            "a failed read must not replace the previous export"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_and_episode_titles_resolve_by_preference() {
        let db = sample_db();
        assert_eq!(db.anime(1).unwrap().unwrap().name, "Seikai no Monshou");
        assert_eq!(
            db.anime_title_lang(1, "en").unwrap().as_deref(),
            Some("Crest of the Stars")
        );
        let eps = db.episodes_of(1).unwrap();
        assert_eq!(eps[0].name, "Invasion");
        assert_eq!(eps[2].epno_str(), "S1");
    }

    #[test]
    fn episode_files_include_partial_coverage_sorted_by_popularity() {
        let db = sample_db();
        let files = db.files_of_episode(10).unwrap();
        assert_eq!(
            files.iter().map(|f| f.fid).collect::<Vec<_>>(),
            vec![101, 100]
        );
        assert_eq!((files[0].start_pct, files[0].end_pct), (50, 100));
        assert_eq!(files[1].quality, "very high");
        assert_eq!(files[1].source, "Blu-ray");
        assert_eq!(files[1].version(), 2);
        assert_eq!(files[1].crc_status(), Some(true));
    }

    #[test]
    fn mylist_aggregates_count_distinct_episodes() {
        let db = sample_db();
        let items = db.mylist_items().unwrap();
        let s = &db.mylist_state_by_anime(&items)[&1];
        assert_eq!(
            (s.eps, s.watched_eps, s.specials, s.watched_specials),
            (2, 1, 1, 1)
        );
        assert_eq!((s.files, s.watched_files, s.size), (3, 2, 3300));
        assert_eq!(
            s.available_eps, 1,
            "deleted entry does not count as available"
        );
        assert_eq!(s.length, (25 + 25 + 5) * 60);
        let a = db.anime(1).unwrap().unwrap();
        assert!(s.complete(&a));
        assert!(!s.all_watched(&a));
    }

    #[test]
    fn local_files_join_identification_and_mylist() {
        let db = sample_db();
        let lf = db.local_files().unwrap();
        let known = lf.iter().find(|l| l.path == "/a/x.mkv").unwrap();
        assert_eq!((known.fid, known.lid), (Some(100), Some(1000)));
        assert!(!known.crc_mismatch());
        let unknown = lf.iter().find(|l| l.path == "/a/unknown.mkv").unwrap();
        assert!(!unknown.known());
        assert_eq!(unknown.file_name(), "unknown.mkv");
    }

    #[test]
    fn projection_loads_all_views_with_search_keys_and_summary() {
        let db = sample_db();
        let p = LibraryProjection::load(&db).unwrap();
        assert_eq!(p.anime.len(), 1);
        assert_eq!(p.mylist.len(), 3);
        assert_eq!(p.local.len(), 2);
        assert_eq!(p.groups.len(), 1);
        assert_eq!(p.states[&1].files, 3);
        assert!(p.queue.is_empty());
        assert_eq!(p.cooldown, None);
        assert_eq!(p.keys.anime, vec!["seikai no monshou".to_string()]);
        assert_eq!(p.keys.local.len(), p.local.len());
        assert_eq!(p.mylist_title(0), "Seikai no Monshou");
        assert!(p.keys.mylist.iter().all(|k| k == "seikai no monshou"));
        assert_eq!(
            (p.summary.local_known, p.summary.local_crc_mismatch),
            (1, 0)
        );
        assert_eq!(p.summary.local_bytes, 1005);
        assert_eq!(p.summary.mylist_watched, 2);
    }
}
