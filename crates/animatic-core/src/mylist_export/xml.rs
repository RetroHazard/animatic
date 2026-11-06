//! The MyList export document: a strict, streaming reader.
//!
//! The accepted structure is documented in
//! [Storage and network](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#mylist-export-import).
//! Every element the importer reads is validated (numbers, dates, hashes, required ids, no
//! element twice in the same parent); elements it does not read are skipped together with their
//! content, up to the depth limit. A document that is truncated, not well formed, has the wrong
//! root, repeats an anime or goes over a limit is an error, never a partial result.

use std::collections::HashSet;
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};

use animatic_anidb::commands::EpNo;
use quick_xml::Reader;
use quick_xml::events::{BytesRef, BytesStart, Event};

use super::{ExportLimits, Fault};
use crate::error::{Error, Result};

/// One anime of the export with the MyList files below its episodes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimeRecord {
    pub aid: i64,
    pub year: Option<String>,
    pub name: Option<String>,
    pub type_name: Option<String>,
    pub episodes: Option<i64>,
    pub specials: Option<i64>,
    /// Unix time (midnight UTC) of the first and last air date.
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub episode_list: Vec<EpisodeRecord>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EpisodeRecord {
    pub eid: i64,
    pub epno: EpNo,
    /// Unix time (UTC) the episode aired.
    pub aired: Option<i64>,
    /// Minutes.
    pub length: Option<i64>,
    pub english: Option<String>,
    pub romaji: Option<String>,
    pub kanji: Option<String>,
    pub files: Vec<FileRecord>,
}

/// A file of an episode together with the account's MyList entry for it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileRecord {
    pub fid: i64,
    pub lid: i64,
    pub size: i64,
    pub gid: i64,
    pub group_name: Option<String>,
    pub filetype: Option<String>,
    pub crc: Option<String>,
    pub md5: Option<String>,
    pub sha1: Option<String>,
    pub tth: Option<String>,
    pub ed2k: Option<String>,
    /// MyList state (`mystate`), when the export carries it.
    pub state: Option<i64>,
    /// `None` when the export does not say whether the entry is watched.
    pub viewed: Option<bool>,
    /// Unix time (UTC) it was watched; `Some(0)` for "not watched".
    pub viewdate: Option<i64>,
    /// Unix time (UTC) the entry was added to the MyList.
    pub added: Option<i64>,
    pub storage: Option<String>,
}

/// What the whole document contained, counted while reading.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    /// Anime with at least one MyList file (handed to the sink).
    pub anime: usize,
    /// Anime without any file: wishlist rows, not imported.
    pub without_files: usize,
    pub episodes: usize,
    /// `file` elements; a file of several episodes appears once per episode.
    pub files: usize,
}

/// State shared by all documents of one export (a tar or zip archive may hold several).
pub(super) struct Run<'a> {
    pub limits: &'a ExportLimits,
    pub fault: &'a Fault,
    pub cancel: &'a AtomicBool,
    pub seen: HashSet<i64>,
    pub totals: Totals,
}

impl<'a> Run<'a> {
    pub fn new(limits: &'a ExportLimits, fault: &'a Fault, cancel: &'a AtomicBool) -> Self {
        Run {
            limits,
            fault,
            cancel,
            seen: HashSet::new(),
            totals: Totals::default(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    AnimeId,
    Year,
    AnimeName,
    AnimeType,
    Count,
    StartDate,
    EndDate,
    Episodes,
    Aired,
    Length,
    EpName,
    EpRomaji,
    EpKanji,
    Files,
    Size,
    Group,
    FileType,
    Crc,
    Md5,
    Sha1,
    Tth,
    Ed2k,
    Ed2kKey,
    MyState,
    State,
    IsWatched,
    ViewDate,
    MyDate,
    Storage,
}

impl Field {
    /// Which element repeats are checked against: 0 anime, 1 episode, 2 file.
    fn scope(self) -> usize {
        use Field::*;
        match self {
            AnimeId | Year | AnimeName | AnimeType | Count | StartDate | EndDate | Episodes => 0,
            Aired | Length | EpName | EpRomaji | EpKanji | Files => 1,
            _ => 2,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ctx {
    Root,
    Anime,
    Episodes,
    Episode,
    Files,
    File,
    /// An element whose text is a value.
    Leaf(Field),
    /// A date given as text or as a nested `<long>`.
    Dated(Field),
    Long(Field),
    Group,
    Ed2k,
    State,
    /// An element with attributes only.
    Attrs(Field),
    /// Something the importer does not read.
    Skip,
}

/// The context of child `name` inside `parent`.
fn child(parent: Ctx, name: &str) -> Result<Ctx> {
    use Field::*;
    Ok(match (parent, name) {
        (Ctx::Root, "anime") => Ctx::Anime,
        (Ctx::Anime, "id") => Ctx::Leaf(AnimeId),
        (Ctx::Anime, "year") => Ctx::Leaf(Year),
        (Ctx::Anime, "name") => Ctx::Leaf(AnimeName),
        (Ctx::Anime, "type") => Ctx::Leaf(AnimeType),
        (Ctx::Anime, "count") => Ctx::Attrs(Count),
        (Ctx::Anime, "startdate") => Ctx::Dated(StartDate),
        (Ctx::Anime, "enddate") => Ctx::Dated(EndDate),
        (Ctx::Anime, "episodes") => Ctx::Episodes,
        (Ctx::Episodes, "episode") => Ctx::Episode,
        (Ctx::Episode, "aired") => Ctx::Dated(Aired),
        (Ctx::Episode, "length") => Ctx::Leaf(Length),
        (Ctx::Episode, "name") => Ctx::Leaf(EpName),
        (Ctx::Episode, "name_romaji") => Ctx::Leaf(EpRomaji),
        (Ctx::Episode, "name_kanji") => Ctx::Leaf(EpKanji),
        (Ctx::Episode, "files") => Ctx::Files,
        (Ctx::Files, "file") => Ctx::File,
        (Ctx::File, "size_plain") => Ctx::Leaf(Size),
        (Ctx::File, "group") => Ctx::Group,
        (Ctx::File, "filetype") => Ctx::Leaf(FileType),
        (Ctx::File, "hash_crc") => Ctx::Leaf(Crc),
        (Ctx::File, "hash_md5") => Ctx::Leaf(Md5),
        (Ctx::File, "hash_sha1") => Ctx::Leaf(Sha1),
        (Ctx::File, "hash_tth") => Ctx::Leaf(Tth),
        (Ctx::File, "hash_ed2k") => Ctx::Ed2k,
        (Ctx::Ed2k, "key") => Ctx::Leaf(Ed2kKey),
        (Ctx::File, "mystate") => Ctx::Attrs(MyState),
        (Ctx::File, "state") => Ctx::State,
        (Ctx::State, "iswatched") => Ctx::Leaf(IsWatched),
        (Ctx::File, "viewdate") => Ctx::Dated(ViewDate),
        (Ctx::File, "mydate") => Ctx::Dated(MyDate),
        (Ctx::File, "storage") => Ctx::Leaf(Storage),
        (Ctx::Dated(f), "long") => Ctx::Long(f),
        // A value element holds text and nothing else.
        (Ctx::Leaf(_) | Ctx::Long(_) | Ctx::Group | Ctx::Attrs(_), _) => {
            return Err(bad(format!("unexpected element <{name}> inside a value")));
        }
        _ => Ctx::Skip,
    })
}

/// The element that may appear once per parent, for `ctx`.
fn once_field(ctx: Ctx) -> Option<Field> {
    match ctx {
        Ctx::Leaf(f) | Ctx::Dated(f) | Ctx::Attrs(f) => Some(f),
        Ctx::Group => Some(Field::Group),
        Ctx::Ed2k => Some(Field::Ed2k),
        Ctx::State => Some(Field::State),
        Ctx::Episodes => Some(Field::Episodes),
        Ctx::Files => Some(Field::Files),
        _ => None,
    }
}

fn bad(message: impl Into<String>) -> Error {
    Error::Xml(message.into())
}

fn xml_err(e: impl std::fmt::Display) -> Error {
    Error::Xml(e.to_string())
}

/// Every attribute of the tag, parsed and unescaped (a duplicate name is an error).
fn attributes(e: &BytesStart, max: usize) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(xml_err)?;
        let value = a
            .normalized_value(quick_xml::XmlVersion::default())
            .map_err(xml_err)?;
        if value.len() > max {
            return Err(bad(format!(
                "an attribute of <{}> is longer than {max} bytes",
                e.name().0
            )));
        }
        out.push((a.key.0.to_string(), value.trim().to_string()));
    }
    Ok(out)
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn required<'a>(attrs: &'a [(String, String)], name: &str, tag: &str) -> Result<&'a str> {
    attr(attrs, name)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| bad(format!("<{tag}> is missing the {name} attribute")))
}

/// A decimal number of at most 18 digits.
fn number(text: &str, what: &str) -> Result<i64> {
    let t = text.trim();
    if t.is_empty() || t.len() > 18 || !t.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad(format!("{what} is not a number: {text:?}")));
    }
    t.parse::<i64>()
        .map_err(|_| bad(format!("{what} is not a number: {text:?}")))
}

fn positive(text: &str, what: &str) -> Result<i64> {
    match number(text, what)? {
        0 => Err(bad(format!("{what} must be positive"))),
        n => Ok(n),
    }
}

/// An optional number: empty or `-` means "not given".
fn optional_number(text: &str, what: &str) -> Result<Option<i64>> {
    match text.trim() {
        "" | "-" => Ok(None),
        t => number(t, what).map(Some),
    }
}

/// `dd.mm.yyyy` or `dd.mm.yyyy hh:mm`, read as UTC. Empty and `-` mean "not given". With
/// `lenient`, a value with `?` for unknown parts (a partial release date) also means unknown.
fn date(text: &str, what: &str, lenient: bool) -> Result<Option<i64>> {
    let t = text.trim();
    if t.is_empty() || t == "-" || (lenient && t.contains('?')) {
        return Ok(None);
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(t, "%d.%m.%Y %H:%M") {
        return Ok(Some(dt.and_utc().timestamp()));
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(t, "%d.%m.%Y")
        && let Some(dt) = d.and_hms_opt(0, 0, 0)
    {
        return Ok(Some(dt.and_utc().timestamp()));
    }
    Err(bad(format!(
        "{what} is not a date (dd.mm.yyyy or dd.mm.yyyy hh:mm): {t:?}"
    )))
}

/// Lowercase hexadecimal of exactly `len` digits; empty and `-` mean "not given".
fn hex(text: &str, len: usize, what: &str) -> Result<Option<String>> {
    match text.trim() {
        "" | "-" => Ok(None),
        t if t.len() == len && t.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Ok(Some(t.to_ascii_lowercase()))
        }
        t => Err(bad(format!(
            "{what} is not {len} hexadecimal digits: {t:?}"
        ))),
    }
}

/// A non-empty string, or `None` for empty text.
fn text_value(text: &str) -> Option<String> {
    let t = text.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// `12`, `S3`, `C1`, `T2`, `P1`, `O4`: an optional kind letter and the number.
fn episode_number(text: &str) -> Result<EpNo> {
    let t = text.trim();
    let (kind, digits) = match t.chars().next() {
        Some('S') => (2, &t[1..]),
        Some('C') => (3, &t[1..]),
        Some('T') => (4, &t[1..]),
        Some('P') => (5, &t[1..]),
        Some('O') => (6, &t[1..]),
        _ => (1, t),
    };
    if digits.is_empty() || digits.len() > 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad(format!("episode number {t:?} is not valid")));
    }
    Ok(EpNo {
        kind,
        number: digits.parse().unwrap_or(0),
    })
}

fn ref_char(g: &BytesRef) -> Option<char> {
    if let Ok(Some(c)) = g.resolve_char_ref() {
        return Some(c);
    }
    match &**g {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

/// The error behind a failed read: a recorded limit, else the parser's own failure.
fn xml_failure(fault: &Fault, e: quick_xml::Error) -> Error {
    if let Some(cause) = fault.take() {
        return Error::Other(cause);
    }
    match e {
        quick_xml::Error::Io(io) => {
            Error::Other(format!("the export is damaged or ends too early ({io})"))
        }
        other => xml_err(other),
    }
}

/// The document being read.
struct Doc {
    stack: Vec<Ctx>,
    text: String,
    long_seen: bool,
    done: bool,
    seen: [u64; 3],
    anime: AnimeRecord,
    episode: EpisodeRecord,
    file: FileRecord,
    iswatched: Option<bool>,
    viewdate: Option<Option<i64>>,
    anime_files: usize,
}

impl Doc {
    fn new() -> Self {
        Doc {
            stack: Vec::new(),
            text: String::new(),
            long_seen: false,
            done: false,
            seen: [0; 3],
            anime: AnimeRecord::default(),
            episode: EpisodeRecord::default(),
            file: FileRecord::default(),
            iswatched: None,
            viewdate: None,
            anime_files: 0,
        }
    }

    fn mark(&mut self, field: Field, name: &str) -> Result<()> {
        let scope = field.scope();
        let bit = 1u64 << (field as u32);
        if self.seen[scope] & bit != 0 {
            return Err(bad(format!("<{name}> appears twice in the same element")));
        }
        self.seen[scope] |= bit;
        Ok(())
    }

    fn start(&mut self, e: &BytesStart, run: &mut Run) -> Result<()> {
        if self.done {
            return Err(bad("content after the end of the root element"));
        }
        if self.stack.len() >= run.limits.max_depth {
            return Err(bad(format!(
                "elements are nested deeper than {}",
                run.limits.max_depth
            )));
        }
        let name = e.name().0;
        let ctx = match self.stack.last() {
            None => match name {
                "mylist" => Ctx::Root,
                "anime" => Ctx::Anime,
                other => {
                    return Err(bad(format!(
                        "root element is <{other}>, expected <mylist> or <anime>"
                    )));
                }
            },
            Some(&parent) => child(parent, name)?,
        };
        if let Some(field) = once_field(ctx) {
            self.mark(field, name)?;
        }
        self.text.clear();
        let max = run.limits.max_text_bytes;
        match ctx {
            Ctx::Anime => {
                if run.seen.len() >= run.limits.max_anime {
                    return Err(Error::Other(format!(
                        "the export has more than {} anime",
                        run.limits.max_anime
                    )));
                }
                attributes(e, max)?;
                self.anime = AnimeRecord::default();
                self.seen = [0; 3];
                self.anime_files = 0;
            }
            Ctx::Episode => {
                let attrs = attributes(e, max)?;
                if self.anime.episode_list.len() >= run.limits.max_episodes_per_anime {
                    return Err(Error::Other(format!(
                        "an anime has more than {} episodes",
                        run.limits.max_episodes_per_anime
                    )));
                }
                self.episode = EpisodeRecord {
                    eid: positive(required(&attrs, "id", name)?, "episode id")?,
                    epno: episode_number(required(&attrs, "number", name)?)?,
                    ..Default::default()
                };
                self.seen[1] = 0;
            }
            Ctx::File => {
                let attrs = attributes(e, max)?;
                self.file = FileRecord {
                    fid: positive(required(&attrs, "id", name)?, "file id")?,
                    lid: positive(required(&attrs, "lid", name)?, "MyList entry id (lid)")?,
                    ..Default::default()
                };
                self.iswatched = None;
                self.viewdate = None;
                self.seen[2] = 0;
            }
            Ctx::Attrs(Field::Count) => {
                let attrs = attributes(e, max)?;
                self.anime.episodes = attr(&attrs, "eps")
                    .map(|v| optional_number(v, "episode count"))
                    .transpose()?
                    .flatten();
                self.anime.specials = attr(&attrs, "specials")
                    .map(|v| optional_number(v, "special count"))
                    .transpose()?
                    .flatten();
            }
            Ctx::Attrs(Field::MyState) => {
                let attrs = attributes(e, max)?;
                self.file.state = attr(&attrs, "id")
                    .map(|v| optional_number(v, "MyList state"))
                    .transpose()?
                    .flatten();
            }
            Ctx::Group => {
                let attrs = attributes(e, max)?;
                self.file.gid = attr(&attrs, "id")
                    .map(|v| optional_number(v, "group id"))
                    .transpose()?
                    .flatten()
                    .unwrap_or(0);
            }
            Ctx::Dated(_) => {
                attributes(e, max)?;
                self.long_seen = false;
            }
            _ => {
                attributes(e, max)?;
            }
        }
        self.stack.push(ctx);
        Ok(())
    }

    fn text(&mut self, s: &str, max: usize) -> Result<()> {
        match self.stack.last() {
            Some(Ctx::Leaf(_) | Ctx::Long(_) | Ctx::Dated(_) | Ctx::Group) => {
                if self.text.len() + s.len() > max {
                    return Err(bad(format!("a value is longer than {max} bytes")));
                }
                self.text.push_str(s);
                Ok(())
            }
            Some(Ctx::Skip | Ctx::Attrs(_)) => Ok(()),
            _ if s.chars().all(char::is_whitespace) => Ok(()),
            _ => Err(bad("text outside of a value")),
        }
    }

    fn entity(&mut self, g: &BytesRef, max: usize) -> Result<()> {
        match ref_char(g) {
            Some(c) => self.text(c.encode_utf8(&mut [0; 4]), max),
            None if matches!(self.stack.last(), Some(Ctx::Skip | Ctx::Attrs(_))) => Ok(()),
            None => Err(bad("unsupported entity reference in a value")),
        }
    }

    fn end(&mut self, run: &mut Run, out: &mut dyn FnMut(AnimeRecord) -> Result<()>) -> Result<()> {
        let Some(ctx) = self.stack.pop() else {
            return Err(bad("unbalanced end tag"));
        };
        let raw = std::mem::take(&mut self.text);
        let t = raw.trim();
        match ctx {
            Ctx::Leaf(field) => self.leaf(field, t)?,
            Ctx::Long(field) => {
                self.long_seen = true;
                self.date(field, t)?;
            }
            Ctx::Dated(field) => {
                if !self.long_seen {
                    self.date(field, t)?;
                }
                self.long_seen = false;
            }
            Ctx::Group => self.file.group_name = text_value(t),
            Ctx::File => self.finish_file(run)?,
            Ctx::Episode => {
                let episode = std::mem::take(&mut self.episode);
                run.totals.episodes += 1;
                self.anime.episode_list.push(episode);
            }
            Ctx::Anime => self.finish_anime(run, out)?,
            _ => {}
        }
        let mut raw = raw;
        raw.clear();
        self.text = raw;
        if self.stack.is_empty() {
            self.done = true;
        }
        Ok(())
    }

    fn leaf(&mut self, field: Field, t: &str) -> Result<()> {
        use Field::*;
        match field {
            AnimeId => self.anime.aid = positive(t, "anime id")?,
            Year => self.anime.year = text_value(t).filter(|y| y != "-"),
            AnimeName => self.anime.name = text_value(t),
            AnimeType => self.anime.type_name = text_value(t),
            Length => self.episode.length = optional_number(t, "episode length")?,
            EpName => self.episode.english = text_value(t),
            EpRomaji => self.episode.romaji = text_value(t),
            EpKanji => self.episode.kanji = text_value(t),
            Size => self.file.size = optional_number(t, "file size")?.unwrap_or(0),
            FileType => self.file.filetype = text_value(t).filter(|v| v != "-"),
            Crc => self.file.crc = hex(t, 8, "CRC32")?,
            Md5 => self.file.md5 = hex(t, 32, "MD5")?,
            Sha1 => self.file.sha1 = hex(t, 40, "SHA-1")?,
            Tth => {
                self.file.tth = match t {
                    "" | "-" => None,
                    t if t.len() <= 64 && t.bytes().all(|b| b.is_ascii_alphanumeric()) => {
                        Some(t.to_string())
                    }
                    t => return Err(bad(format!("TTH hash is not valid: {t:?}"))),
                }
            }
            Ed2kKey => self.file.ed2k = hex(t, 32, "ED2K hash")?,
            IsWatched => {
                self.iswatched = match t {
                    "" => None,
                    "1" | "true" => Some(true),
                    "0" | "false" => Some(false),
                    other => {
                        return Err(bad(format!("iswatched must be 0 or 1, not {other:?}")));
                    }
                }
            }
            Storage => self.file.storage = text_value(t),
            Count | StartDate | EndDate | Episodes | Aired | Files | Group | Ed2k | MyState
            | State | ViewDate | MyDate => {}
        }
        Ok(())
    }

    fn date(&mut self, field: Field, t: &str) -> Result<()> {
        match field {
            Field::StartDate => self.anime.start = date(t, "start date", true)?,
            Field::EndDate => self.anime.end = date(t, "end date", true)?,
            Field::Aired => self.episode.aired = date(t, "air date", true)?,
            Field::ViewDate => self.viewdate = Some(date(t, "view date", false)?),
            Field::MyDate => self.file.added = date(t, "date added", false)?,
            _ => {}
        }
        Ok(())
    }

    fn finish_file(&mut self, run: &mut Run) -> Result<()> {
        let mut file = std::mem::take(&mut self.file);
        run.totals.files += 1;
        self.anime_files += 1;
        if run.totals.files > run.limits.max_files {
            return Err(Error::Other(format!(
                "the export has more than {} files",
                run.limits.max_files
            )));
        }
        if self.anime_files > run.limits.max_files_per_anime {
            return Err(Error::Other(format!(
                "an anime has more than {} files",
                run.limits.max_files_per_anime
            )));
        }
        if file.gid == 0 {
            file.group_name = None;
        }
        // The watched flag wins; without it a given view date decides.
        let viewed = self
            .iswatched
            .or_else(|| self.viewdate.map(|d| d.is_some()));
        file.viewed = viewed;
        file.viewdate = match (viewed, self.viewdate.flatten()) {
            (Some(false), _) => Some(0),
            (_, Some(date)) => Some(date),
            _ => None,
        };
        if file.ed2k.is_none() && file.size > 0 {
            // A file with content always has an ED2K hash on AniDB; without one the row could
            // never be matched with a local file.
            return Err(bad(format!(
                "file {} has a size but no ED2K hash",
                file.fid
            )));
        }
        self.episode.files.push(file);
        Ok(())
    }

    fn finish_anime(
        &mut self,
        run: &mut Run,
        out: &mut dyn FnMut(AnimeRecord) -> Result<()>,
    ) -> Result<()> {
        let record = std::mem::take(&mut self.anime);
        if record.aid == 0 {
            return Err(bad("an anime has no <id>"));
        }
        if !run.seen.insert(record.aid) {
            return Err(bad(format!(
                "anime {} appears twice in the export",
                record.aid
            )));
        }
        if run.cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        if self.anime_files == 0 {
            run.totals.without_files += 1;
        } else {
            run.totals.anime += 1;
            out(record)?;
        }
        Ok(())
    }
}

/// Read one export document from `input` (UTF-8 XML, already decompressed) and hand every
/// anime that has MyList files to `out`. `Ok` is returned only for a complete document.
pub(super) fn read_document<R: BufRead>(
    mut input: R,
    run: &mut Run,
    out: &mut dyn FnMut(AnimeRecord) -> Result<()>,
) -> Result<()> {
    // A UTF-8 byte order mark is only allowed at the very start.
    if let Ok(head) = input.fill_buf()
        && head.starts_with(&[0xef, 0xbb, 0xbf])
    {
        input.consume(3);
    }
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(false);
    reader.config_mut().check_comments = true;
    let max = run.limits.max_text_bytes;
    let mut doc = Doc::new();
    let mut buf = Vec::new();
    let mut first = true;
    loop {
        buf.clear();
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| xml_failure(run.fault, e))?;
        let at_start = std::mem::replace(&mut first, false);
        match event {
            Event::Start(e) => doc.start(&e, run)?,
            Event::Empty(e) => {
                doc.start(&e, run)?;
                doc.end(run, out)?;
            }
            Event::End(_) => doc.end(run, out)?,
            Event::Text(t) => doc.text(&t, max)?,
            Event::CData(t) => doc.text(&t, max)?,
            Event::GeneralRef(g) => doc.entity(&g, max)?,
            Event::DocType(_) => return Err(bad("document type declarations are not accepted")),
            Event::Decl(d) => {
                if !at_start {
                    return Err(bad(
                        "an XML declaration is only allowed at the very start of the document",
                    ));
                }
                if let Some(enc) = d.encoding() {
                    let enc = enc.map_err(xml_err)?;
                    if !enc.eq_ignore_ascii_case("utf-8") {
                        return Err(bad("only UTF-8 exports are supported"));
                    }
                }
            }
            Event::PI(_) | Event::Comment(_) => {}
            Event::Eof => {
                if !doc.done {
                    return Err(bad(if doc.stack.is_empty() {
                        "no <mylist> or <anime> root element".to_string()
                    } else {
                        "truncated export: the root element is not closed".to_string()
                    }));
                }
                return Ok(());
            }
        }
    }
}
