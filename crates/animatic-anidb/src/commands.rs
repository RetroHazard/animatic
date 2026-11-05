//! Typed wrappers around the UDP API commands Animatic uses.

use serde::{Deserialize, Serialize};

use crate::error::{AnidbError, Result};
use crate::udp::{Response, UdpClient};

/// FILE fmask fields in protocol order (byte 1 bit 7 → byte 5 bit 0). `None` = not requested.
const FILE_FMASK_FIELDS: [Option<&str>; 40] = [
    None,
    Some("aid"),
    Some("eid"),
    Some("gid"),
    Some("lid"),
    Some("other_eps"),
    Some("deprecated"),
    Some("state"),
    Some("size"),
    Some("ed2k"),
    Some("md5"),
    Some("sha1"),
    Some("crc32"),
    None,
    None,
    None,
    Some("quality"),
    Some("source"),
    Some("audio_codecs"),
    Some("audio_bitrates"),
    Some("video_codec"),
    Some("video_bitrate"),
    Some("resolution"),
    Some("filetype"),
    Some("dub_langs"),
    Some("sub_langs"),
    Some("length"),
    Some("description"),
    Some("aired"),
    None,
    None,
    Some("anidb_filename"),
    Some("ml_state"),
    Some("ml_filestate"),
    Some("ml_viewed"),
    Some("ml_viewdate"),
    Some("ml_storage"),
    Some("ml_source"),
    Some("ml_other"),
    None,
];

/// FILE amask fields (byte 1 bit 7 → byte 4 bit 0).
const FILE_AMASK_FIELDS: [Option<&str>; 32] = [
    Some("a_episodes"),
    Some("a_highest_ep"),
    Some("a_year"),
    Some("a_type"),
    None,
    None,
    None,
    None,
    Some("a_romaji"),
    Some("a_kanji"),
    Some("a_english"),
    Some("a_other"),
    Some("a_short"),
    Some("a_synonyms"),
    None,
    None,
    Some("epno"),
    Some("ep_name"),
    Some("ep_romaji"),
    Some("ep_kanji"),
    Some("ep_rating"),
    Some("ep_votes"),
    None,
    None,
    Some("g_name"),
    Some("g_short"),
    None,
    None,
    None,
    None,
    None,
    Some("a_updated"),
];

/// ANIME amask fields (7 bytes).
const ANIME_AMASK_FIELDS: [Option<&str>; 56] = [
    Some("aid"),
    None,
    Some("year"),
    Some("type"),
    Some("related_aids"),
    Some("related_types"),
    None,
    None,
    Some("romaji"),
    Some("kanji"),
    Some("english"),
    Some("other"),
    Some("short"),
    Some("synonyms"),
    None,
    None,
    Some("episodes"),
    Some("highest_ep"),
    Some("special_eps"),
    Some("air_date"),
    Some("end_date"),
    Some("url"),
    Some("picname"),
    None,
    Some("rating"),
    Some("votes"),
    Some("temp_rating"),
    Some("temp_votes"),
    Some("review_rating"),
    Some("reviews"),
    None,
    Some("restricted"),
    None,
    Some("ann_id"),
    Some("allcinema_id"),
    Some("animenfo_id"),
    None,
    None,
    None,
    Some("updated"),
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    Some("specials"),
    Some("credits"),
    Some("others"),
    Some("trailers"),
    Some("parodies"),
    None,
    None,
    None,
];

fn mask_hex<const N: usize>(fields: &[Option<&str>; N]) -> String {
    let mut bytes = vec![0u8; N / 8];
    for (i, f) in fields.iter().enumerate() {
        if f.is_some() {
            bytes[i / 8] |= 0x80 >> (i % 8);
        }
    }
    hex::encode_upper(bytes)
}

fn named<'a, const N: usize>(fields: &[Option<&'a str>; N]) -> impl Iterator<Item = &'a str> {
    fields.iter().flatten().copied()
}

fn num<T: std::str::FromStr + Default>(s: &str) -> T {
    s.trim().parse().unwrap_or_default()
}

fn opt(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn list(s: &str) -> Vec<String> {
    s.split('\'')
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect()
}

/// Episode number as AniDB writes it: "12", "S3", "C1", "T2", "P1", "O1".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EpNo {
    /// 1 regular, 2 special, 3 credit, 4 trailer, 5 parody, 6 other.
    pub kind: u8,
    pub number: u32,
}

impl EpNo {
    pub fn parse(s: &str) -> EpNo {
        let s = s.trim();
        let (kind, rest) = match s.chars().next() {
            Some('S') => (2, &s[1..]),
            Some('C') => (3, &s[1..]),
            Some('T') => (4, &s[1..]),
            Some('P') => (5, &s[1..]),
            Some('O') => (6, &s[1..]),
            _ => (1, s),
        };
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        EpNo {
            kind,
            number: digits.parse().unwrap_or(0),
        }
    }
}

impl std::fmt::Display for EpNo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let p = match self.kind {
            2 => "S",
            3 => "C",
            4 => "T",
            5 => "P",
            6 => "O",
            _ => "",
        };
        write!(f, "{p}{}", self.number)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MylistEntry {
    pub lid: i64,
    pub fid: i64,
    pub eid: i64,
    pub aid: i64,
    pub gid: i64,
    pub date: i64,
    pub state: i64,
    pub viewdate: i64,
    pub storage: Option<String>,
    pub source: Option<String>,
    pub other: Option<String>,
    pub filestate: i64,
}

impl MylistEntry {
    fn from_fields(f: &[String]) -> Option<MylistEntry> {
        if f.len() < 12 {
            return None;
        }
        Some(MylistEntry {
            lid: num(&f[0]),
            fid: num(&f[1]),
            eid: num(&f[2]),
            aid: num(&f[3]),
            gid: num(&f[4]),
            date: num(&f[5]),
            state: num(&f[6]),
            viewdate: num(&f[7]),
            storage: opt(&f[8]),
            source: opt(&f[9]),
            other: opt(&f[10]),
            filestate: num(&f[11]),
        })
    }

    pub fn viewed(&self) -> bool {
        self.viewdate != 0
    }
}

/// Everything a FILE lookup returns (file, its episode/anime/group summary and mylist state).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileInfo {
    pub fid: i64,
    pub aid: i64,
    pub eid: i64,
    pub gid: i64,
    pub lid: i64,
    /// Additional episodes as (eid, percentage).
    pub other_episodes: Vec<(i64, i64)>,
    pub deprecated: bool,
    pub state: i64,
    pub size: i64,
    pub ed2k: Option<String>,
    pub md5: Option<String>,
    pub sha1: Option<String>,
    pub crc32: Option<String>,
    pub quality: Option<String>,
    pub source: Option<String>,
    pub audio_codecs: Vec<String>,
    pub audio_bitrates: Vec<String>,
    pub video_codec: Option<String>,
    pub video_bitrate: i64,
    pub resolution: Option<String>,
    pub filetype: Option<String>,
    pub dub_langs: Vec<String>,
    pub sub_langs: Vec<String>,
    pub length: i64,
    pub description: Option<String>,
    pub aired: i64,
    pub anidb_filename: Option<String>,
    pub mylist: Option<MylistEntry>,
    pub anime_episodes: i64,
    pub anime_highest_ep: i64,
    pub anime_year: Option<String>,
    pub anime_type: Option<String>,
    pub anime_romaji: Option<String>,
    pub anime_kanji: Option<String>,
    pub anime_english: Option<String>,
    pub anime_other: Vec<String>,
    pub anime_short: Vec<String>,
    pub anime_synonyms: Vec<String>,
    pub epno: EpNo,
    pub ep_name: Option<String>,
    pub ep_romaji: Option<String>,
    pub ep_kanji: Option<String>,
    pub ep_rating: i64,
    pub ep_votes: i64,
    pub group_name: Option<String>,
    pub group_short: Option<String>,
    pub anime_updated: i64,
}

impl FileInfo {
    fn from_response(resp: &Response) -> Result<FileInfo> {
        let f = resp.fields(0);
        let names: Vec<&str> = std::iter::once("fid")
            .chain(named(&FILE_FMASK_FIELDS))
            .chain(named(&FILE_AMASK_FIELDS))
            .collect();
        if f.len() < names.len() {
            return Err(AnidbError::Protocol(format!(
                "FILE reply has {} fields, expected {}",
                f.len(),
                names.len()
            )));
        }
        let mut i = FileInfo::default();
        let mut ml = MylistEntry::default();
        for (name, v) in names.iter().zip(f.iter()) {
            let v = v.as_str();
            match *name {
                "fid" => i.fid = num(v),
                "aid" => i.aid = num(v),
                "eid" => i.eid = num(v),
                "gid" => i.gid = num(v),
                "lid" => i.lid = num(v),
                "other_eps" => {
                    i.other_episodes = list(v)
                        .iter()
                        .filter_map(|e| {
                            let mut p = e.split(',');
                            Some((num(p.next()?), p.next().map(num).unwrap_or(100)))
                        })
                        .collect()
                }
                "deprecated" => i.deprecated = v == "1",
                "state" => i.state = num(v),
                "size" => i.size = num(v),
                "ed2k" => i.ed2k = opt(v).map(|s| s.to_ascii_lowercase()),
                "md5" => i.md5 = opt(v).map(|s| s.to_ascii_lowercase()),
                "sha1" => i.sha1 = opt(v).map(|s| s.to_ascii_lowercase()),
                "crc32" => i.crc32 = opt(v).map(|s| s.to_ascii_lowercase()),
                "quality" => i.quality = opt(v),
                "source" => i.source = opt(v),
                "audio_codecs" => i.audio_codecs = list(v),
                "audio_bitrates" => i.audio_bitrates = list(v),
                "video_codec" => i.video_codec = opt(v),
                "video_bitrate" => i.video_bitrate = num(v),
                "resolution" => i.resolution = opt(v),
                "filetype" => i.filetype = opt(v),
                "dub_langs" => i.dub_langs = list(v),
                "sub_langs" => i.sub_langs = list(v),
                "length" => i.length = num(v),
                "description" => i.description = opt(v),
                "aired" => i.aired = num(v),
                "anidb_filename" => i.anidb_filename = opt(v),
                "ml_state" => ml.state = num(v),
                "ml_filestate" => ml.filestate = num(v),
                "ml_viewed" => {}
                "ml_viewdate" => ml.viewdate = num(v),
                "ml_storage" => ml.storage = opt(v),
                "ml_source" => ml.source = opt(v),
                "ml_other" => ml.other = opt(v),
                "a_episodes" => i.anime_episodes = num(v),
                "a_highest_ep" => i.anime_highest_ep = num(v),
                "a_year" => i.anime_year = opt(v),
                "a_type" => i.anime_type = opt(v),
                "a_romaji" => i.anime_romaji = opt(v),
                "a_kanji" => i.anime_kanji = opt(v),
                "a_english" => i.anime_english = opt(v),
                "a_other" => i.anime_other = list(v),
                "a_short" => i.anime_short = list(v),
                "a_synonyms" => i.anime_synonyms = list(v),
                "epno" => i.epno = EpNo::parse(v),
                "ep_name" => i.ep_name = opt(v),
                "ep_romaji" => i.ep_romaji = opt(v),
                "ep_kanji" => i.ep_kanji = opt(v),
                "ep_rating" => i.ep_rating = num(v),
                "ep_votes" => i.ep_votes = num(v),
                "g_name" => i.group_name = opt(v),
                "g_short" => i.group_short = opt(v),
                "a_updated" => i.anime_updated = num(v),
                _ => {}
            }
        }
        if i.lid != 0 {
            ml.lid = i.lid;
            ml.fid = i.fid;
            ml.eid = i.eid;
            ml.aid = i.aid;
            ml.gid = i.gid;
            i.mylist = Some(ml);
        }
        Ok(i)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnimeInfo {
    pub aid: i64,
    pub year: Option<String>,
    pub type_name: Option<String>,
    pub related: Vec<(i64, i64)>,
    pub romaji: Option<String>,
    pub kanji: Option<String>,
    pub english: Option<String>,
    pub other: Vec<String>,
    pub short: Vec<String>,
    pub synonyms: Vec<String>,
    pub episodes: i64,
    pub highest_ep: i64,
    pub special_eps: i64,
    pub air_date: i64,
    pub end_date: i64,
    pub url: Option<String>,
    pub picname: Option<String>,
    pub rating: i64,
    pub votes: i64,
    pub temp_rating: i64,
    pub temp_votes: i64,
    pub review_rating: i64,
    pub reviews: i64,
    pub restricted: bool,
    pub ann_id: i64,
    pub allcinema_id: i64,
    pub animenfo_id: Option<String>,
    pub updated: i64,
    pub specials: i64,
    pub credits: i64,
    pub others: i64,
    pub trailers: i64,
    pub parodies: i64,
}

impl AnimeInfo {
    fn from_response(resp: &Response) -> Result<AnimeInfo> {
        let f = resp.fields(0);
        let names: Vec<&str> = named(&ANIME_AMASK_FIELDS).collect();
        if f.len() < names.len() {
            return Err(AnidbError::Protocol(format!(
                "ANIME reply has {} fields, expected {}",
                f.len(),
                names.len()
            )));
        }
        let mut a = AnimeInfo::default();
        let mut related_aids = Vec::new();
        let mut related_types = Vec::new();
        for (name, v) in names.iter().zip(f.iter()) {
            let v = v.as_str();
            match *name {
                "aid" => a.aid = num(v),
                "year" => a.year = opt(v),
                "type" => a.type_name = opt(v),
                "related_aids" => related_aids = list(v).iter().map(|x| num::<i64>(x)).collect(),
                "related_types" => related_types = list(v).iter().map(|x| num::<i64>(x)).collect(),
                "romaji" => a.romaji = opt(v),
                "kanji" => a.kanji = opt(v),
                "english" => a.english = opt(v),
                "other" => a.other = list(v),
                "short" => a.short = list(v),
                "synonyms" => a.synonyms = list(v),
                "episodes" => a.episodes = num(v),
                "highest_ep" => a.highest_ep = num(v),
                "special_eps" => a.special_eps = num(v),
                "air_date" => a.air_date = num(v),
                "end_date" => a.end_date = num(v),
                "url" => a.url = opt(v),
                "picname" => a.picname = opt(v),
                "rating" => a.rating = num(v),
                "votes" => a.votes = num(v),
                "temp_rating" => a.temp_rating = num(v),
                "temp_votes" => a.temp_votes = num(v),
                "review_rating" => a.review_rating = num(v),
                "reviews" => a.reviews = num(v),
                "restricted" => a.restricted = v == "1",
                "ann_id" => a.ann_id = num(v),
                "allcinema_id" => a.allcinema_id = num(v),
                "animenfo_id" => a.animenfo_id = opt(v),
                "updated" => a.updated = num(v),
                "specials" => a.specials = num(v),
                "credits" => a.credits = num(v),
                "others" => a.others = num(v),
                "trailers" => a.trailers = num(v),
                "parodies" => a.parodies = num(v),
                _ => {}
            }
        }
        a.related = related_aids.into_iter().zip(related_types).collect();
        Ok(a)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EpisodeInfo {
    pub eid: i64,
    pub aid: i64,
    pub length: i64,
    pub rating: i64,
    pub votes: i64,
    pub epno: EpNo,
    pub english: Option<String>,
    pub romaji: Option<String>,
    pub kanji: Option<String>,
    pub aired: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupInfo {
    pub gid: i64,
    pub rating: i64,
    pub votes: i64,
    pub anime_count: i64,
    pub file_count: i64,
    pub name: Option<String>,
    pub short: Option<String>,
    pub irc_channel: Option<String>,
    pub irc_server: Option<String>,
    pub url: Option<String>,
    pub picname: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MylistStats {
    pub animes: i64,
    pub episodes: i64,
    pub files: i64,
    pub size_mb: i64,
    pub added_animes: i64,
    pub added_episodes: i64,
    pub added_files: i64,
    pub added_groups: i64,
    pub leech_pct: i64,
    pub glory_pct: i64,
    pub viewed_pct_of_db: i64,
    pub mylist_pct_of_db: i64,
    pub viewed_pct_of_mylist: i64,
    pub viewed_episodes: i64,
    pub votes: i64,
    pub reviews: i64,
    pub viewed_minutes: i64,
}

/// What MYLISTADD should do with the entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MylistEdit {
    /// None = leave unchanged when editing / server default when adding.
    pub state: Option<i64>,
    pub viewed: Option<bool>,
    pub viewdate: Option<i64>,
    pub storage: Option<String>,
    pub source: Option<String>,
    pub other: Option<String>,
}

impl MylistEdit {
    fn params(&self) -> Vec<(&'static str, String)> {
        let mut p = Vec::new();
        if let Some(s) = self.state {
            p.push(("state", s.to_string()));
        }
        if let Some(v) = self.viewed {
            p.push(("viewed", u8::from(v).to_string()));
        }
        if let Some(d) = self.viewdate.filter(|_| self.viewed != Some(false)) {
            p.push(("viewdate", d.to_string()));
        }
        if let Some(s) = &self.storage {
            p.push(("storage", s.clone()));
        }
        if let Some(s) = &self.source {
            p.push(("source", s.clone()));
        }
        if let Some(s) = &self.other {
            p.push(("other", s.clone()));
        }
        p
    }
}

/// How a file is identified for mylist commands.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FileRef {
    Fid(i64),
    Hash { size: u64, ed2k: String },
    Lid(i64),
}

impl FileRef {
    fn params(&self) -> Vec<(&'static str, String)> {
        match self {
            FileRef::Fid(f) => vec![("fid", f.to_string())],
            FileRef::Hash { size, ed2k } => vec![
                ("size", size.to_string()),
                ("ed2k", ed2k.to_ascii_lowercase()),
            ],
            FileRef::Lid(l) => vec![("lid", l.to_string())],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MylistAddResult {
    Added { lid: i64 },
    AlreadyInMylist(MylistEntry),
    Edited { count: i64 },
    NoSuchFile,
    NoSuchEntry,
}

pub fn file_by_hash(c: &mut UdpClient, size: u64, ed2k: &str) -> Result<Option<FileInfo>> {
    let resp = c.request(
        "FILE",
        &[
            ("size", size.to_string()),
            ("ed2k", ed2k.to_ascii_lowercase()),
            ("fmask", mask_hex(&FILE_FMASK_FIELDS)),
            ("amask", mask_hex(&FILE_AMASK_FIELDS)),
        ],
    )?;
    file_reply(resp)
}

pub fn file_by_fid(c: &mut UdpClient, fid: i64) -> Result<Option<FileInfo>> {
    let resp = c.request(
        "FILE",
        &[
            ("fid", fid.to_string()),
            ("fmask", mask_hex(&FILE_FMASK_FIELDS)),
            ("amask", mask_hex(&FILE_AMASK_FIELDS)),
        ],
    )?;
    file_reply(resp)
}

fn file_reply(resp: Response) -> Result<Option<FileInfo>> {
    match resp.code {
        220 => FileInfo::from_response(&resp).map(Some),
        320 => Ok(None),
        322 => Err(AnidbError::Protocol("multiple files found".into())),
        c => Err(AnidbError::Unexpected(c, resp.message)),
    }
}

pub fn anime(c: &mut UdpClient, aid: i64) -> Result<Option<AnimeInfo>> {
    let resp = c.request(
        "ANIME",
        &[
            ("aid", aid.to_string()),
            ("amask", mask_hex(&ANIME_AMASK_FIELDS)),
        ],
    )?;
    match resp.code {
        230 => AnimeInfo::from_response(&resp).map(Some),
        330 => Ok(None),
        code => Err(AnidbError::Unexpected(code, resp.message)),
    }
}

pub fn episode(c: &mut UdpClient, eid: i64) -> Result<Option<EpisodeInfo>> {
    let resp = c.request("EPISODE", &[("eid", eid.to_string())])?;
    match resp.code {
        240 => {
            let f = resp.fields(0);
            if f.len() < 10 {
                return Err(AnidbError::Protocol("short EPISODE reply".into()));
            }
            Ok(Some(EpisodeInfo {
                eid: num(&f[0]),
                aid: num(&f[1]),
                length: num(&f[2]),
                rating: num(&f[3]),
                votes: num(&f[4]),
                epno: EpNo::parse(&f[5]),
                english: opt(&f[6]),
                romaji: opt(&f[7]),
                kanji: opt(&f[8]),
                aired: num(&f[9]),
            }))
        }
        340 => Ok(None),
        code => Err(AnidbError::Unexpected(code, resp.message)),
    }
}

pub fn group(c: &mut UdpClient, gid: i64) -> Result<Option<GroupInfo>> {
    let resp = c.request("GROUP", &[("gid", gid.to_string())])?;
    match resp.code {
        250 => {
            let f = resp.fields(0);
            if f.len() < 11 {
                return Err(AnidbError::Protocol("short GROUP reply".into()));
            }
            Ok(Some(GroupInfo {
                gid: num(&f[0]),
                rating: num(&f[1]),
                votes: num(&f[2]),
                anime_count: num(&f[3]),
                file_count: num(&f[4]),
                name: opt(&f[5]),
                short: opt(&f[6]),
                irc_channel: opt(&f[7]),
                irc_server: opt(&f[8]),
                url: opt(&f[9]),
                picname: opt(&f[10]),
            }))
        }
        350 => Ok(None),
        code => Err(AnidbError::Unexpected(code, resp.message)),
    }
}

pub fn mylist_add(
    c: &mut UdpClient,
    file: &FileRef,
    edit: &MylistEdit,
    edit_existing: bool,
) -> Result<MylistAddResult> {
    let mut p = file.params();
    p.extend(edit.params());
    if edit_existing {
        p.push(("edit", "1".into()));
    }
    let resp = c.request("MYLISTADD", &p)?;
    match resp.code {
        210 => Ok(MylistAddResult::Added {
            lid: resp.lines.first().map(|l| num(l)).unwrap_or(0),
        }),
        310 => Ok(MylistEntry::from_fields(&resp.fields(0))
            .map(MylistAddResult::AlreadyInMylist)
            .unwrap_or(MylistAddResult::AlreadyInMylist(MylistEntry::default()))),
        311 => Ok(MylistAddResult::Edited {
            count: resp.lines.first().map(|l| num(l)).unwrap_or(1),
        }),
        320 => Ok(MylistAddResult::NoSuchFile),
        411 => Ok(MylistAddResult::NoSuchEntry),
        code => Err(AnidbError::Unexpected(code, resp.message)),
    }
}

/// Returns the number of deleted entries (0 when no such entry existed).
pub fn mylist_del(c: &mut UdpClient, file: &FileRef) -> Result<i64> {
    let resp = c.request("MYLISTDEL", &file.params())?;
    match resp.code {
        211 => Ok(resp.lines.first().map(|l| num(l)).unwrap_or(1)),
        411 => Ok(0),
        code => Err(AnidbError::Unexpected(code, resp.message)),
    }
}

pub fn mylist(c: &mut UdpClient, file: &FileRef) -> Result<Option<MylistEntry>> {
    let resp = c.request("MYLIST", &file.params())?;
    match resp.code {
        221 => Ok(MylistEntry::from_fields(&resp.fields(0))),
        321 => Ok(None),
        code => Err(AnidbError::Unexpected(code, resp.message)),
    }
}

pub fn mylist_stats(c: &mut UdpClient) -> Result<MylistStats> {
    let resp = c.request("MYLISTSTATS", &[])?;
    if resp.code != 222 {
        return Err(AnidbError::Unexpected(resp.code, resp.message));
    }
    let f: Vec<i64> = resp.fields(0).iter().map(|s| num(s)).collect();
    let g = |i: usize| f.get(i).copied().unwrap_or(0);
    Ok(MylistStats {
        animes: g(0),
        episodes: g(1),
        files: g(2),
        size_mb: g(3),
        added_animes: g(4),
        added_episodes: g(5),
        added_files: g(6),
        added_groups: g(7),
        leech_pct: g(8),
        glory_pct: g(9),
        viewed_pct_of_db: g(10),
        mylist_pct_of_db: g(11),
        viewed_pct_of_mylist: g(12),
        viewed_episodes: g(13),
        votes: g(14),
        reviews: g(15),
        viewed_minutes: g(16),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_encode_requested_fields() {
        assert_eq!(mask_hex(&FILE_FMASK_FIELDS), "7FF8FFF9FE");
        assert_eq!(mask_hex(&FILE_AMASK_FIELDS), "F0FCFCC1");
        assert_eq!(mask_hex(&ANIME_AMASK_FIELDS), "BCFCFEFD7100F8");
    }

    #[test]
    fn episode_numbers() {
        assert_eq!(
            EpNo::parse("12"),
            EpNo {
                kind: 1,
                number: 12
            }
        );
        assert_eq!(EpNo::parse("S3"), EpNo { kind: 2, number: 3 });
        assert_eq!(EpNo::parse("C01").to_string(), "C1");
    }

    #[test]
    fn file_reply_parses_fields_in_mask_order() {
        let fields: Vec<String> = std::iter::once("312498".to_string())
            .chain(named(&FILE_FMASK_FIELDS).map(|n| match n {
                "aid" => "4896".into(),
                "eid" => "113".into(),
                "gid" => "7".into(),
                "lid" => "0".into(),
                "other_eps" => "114,50'115,100".into(),
                "size" => "233340928".into(),
                "ed2k" => "A62E4C6D3A1CB5A5D7E5A4E3B7D2C1F0".into(),
                "audio_codecs" => "AAC'AC3".into(),
                "dub_langs" => "japanese".into(),
                "sub_langs" => "english'german".into(),
                "description" => "line`s one<br />two".into(),
                _ => String::new(),
            }))
            .chain(named(&FILE_AMASK_FIELDS).map(|n| match n {
                "epno" => "S2".into(),
                "g_short" => "V-A".into(),
                _ => String::new(),
            }))
            .collect();
        let resp = Response {
            code: 220,
            message: "FILE".into(),
            lines: vec![fields.join("|")],
        };
        let fi = FileInfo::from_response(&resp).unwrap();
        assert_eq!(fi.fid, 312498);
        assert_eq!(fi.aid, 4896);
        assert_eq!(fi.other_episodes, vec![(114, 50), (115, 100)]);
        assert_eq!(fi.size, 233_340_928);
        assert_eq!(fi.ed2k.as_deref(), Some("a62e4c6d3a1cb5a5d7e5a4e3b7d2c1f0"));
        assert_eq!(fi.audio_codecs, vec!["AAC", "AC3"]);
        assert_eq!(fi.sub_langs, vec!["english", "german"]);
        assert_eq!(fi.description.as_deref(), Some("line's one\ntwo"));
        assert_eq!(fi.epno, EpNo { kind: 2, number: 2 });
        assert_eq!(fi.group_short.as_deref(), Some("V-A"));
        assert!(fi.mylist.is_none());
    }
}
