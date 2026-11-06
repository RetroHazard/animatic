//! AniDB HTTP API (anime XML), anime title dump and image downloads.

use std::cell::Cell;
use std::collections::HashSet;
use std::io::{self, BufRead, BufReader, Read};
use std::sync::Arc;
use std::time::Duration;

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};

use crate::commands::EpNo;
use crate::error::{AnidbError, Result};
use crate::limits::Limits;

pub const HTTP_API: &str = "http://api.anidb.net:9001/httpapi";
pub const TITLES_DUMP: &str = "https://anidb.net/api/anime-titles.xml.gz";
pub const DEFAULT_IMAGE_SERVER: &str = "cdn.anidb.net";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Title {
    pub name: String,
    pub lang: String,
    /// main, synonym, short, official, kana, card
    pub kind: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub weight: i64,
    pub spoiler: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct XmlEpisode {
    pub eid: i64,
    pub epno: EpNo,
    pub length: i64,
    pub airdate: Option<String>,
    pub rating: f64,
    pub votes: i64,
    pub titles: Vec<Title>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnimeXml {
    pub aid: i64,
    pub restricted: bool,
    pub type_name: Option<String>,
    pub episode_count: i64,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub titles: Vec<Title>,
    /// (aid, relation name)
    pub related: Vec<(i64, String)>,
    pub url: Option<String>,
    pub description: Option<String>,
    pub rating: f64,
    pub votes: i64,
    pub temp_rating: f64,
    pub temp_votes: i64,
    pub review_rating: f64,
    pub reviews: i64,
    pub picture: Option<String>,
    pub tags: Vec<Tag>,
    /// (creator role, name)
    pub creators: Vec<(String, String)>,
    pub episodes: Vec<XmlEpisode>,
}

fn attr(e: &quick_xml::events::BytesStart, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.0 == name)
        .and_then(|a| {
            a.normalized_value(quick_xml::XmlVersion::default())
                .ok()
                .map(|v| v.into_owned())
        })
}

/// Text of an entity or character reference (`&amp;`, `&#x41;` ...).
fn general_ref(g: &quick_xml::events::BytesRef) -> String {
    if let Ok(Some(c)) = g.resolve_char_ref() {
        return c.to_string();
    }
    match &**g {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        _ => "",
    }
    .to_string()
}

fn xerr(e: impl std::fmt::Display) -> AnidbError {
    AnidbError::Xml(e.to_string())
}

/// Classify an HTTP API `<error>` reply. Bans and rejected clients stop the queue; any other
/// API error will not change on retry, so it is a permanent protocol failure for that command.
fn api_error(code: Option<&str>, text: &str) -> AnidbError {
    let detail = match (code, text.is_empty()) {
        (Some(c), false) => format!("{c}: {text}"),
        (Some(c), true) => c.to_string(),
        (None, false) => text.to_string(),
        (None, true) => "no details".to_string(),
    };
    let lower = text.to_ascii_lowercase();
    if code == Some("500") || lower.contains("banned") {
        AnidbError::Banned(format!("HTTP API: {detail}"))
    } else if code == Some("302") || lower.contains("client") {
        AnidbError::Auth(format!(
            "HTTP API rejected the client registration ({detail})"
        ))
    } else {
        AnidbError::Xml(format!("AniDB HTTP API error {detail}"))
    }
}

/// Map a transport failure. Connectivity problems (DNS, connect, TLS, timeouts, body I/O) are
/// `Network`/`Timeout`, exactly like the UDP client: they pause syncing but never count as an
/// attempt of the command. Only a response the server actually sent counts as `Http`.
fn transport_error(e: ureq::Error) -> AnidbError {
    match e {
        ureq::Error::StatusCode(403) => AnidbError::Banned("HTTP 403 Forbidden".into()),
        ureq::Error::StatusCode(429) => AnidbError::ServerUnavailable(crate::error::BAN_COOLDOWN),
        ureq::Error::StatusCode(c) if c >= 500 => AnidbError::Http(format!("HTTP status {c}")),
        ureq::Error::StatusCode(c) => AnidbError::Protocol(format!("unexpected HTTP status {c}")),
        ureq::Error::BodyExceedsLimit(limit) => {
            AnidbError::Protocol(format!("reply larger than the {limit} byte limit"))
        }
        ureq::Error::Timeout(_) => AnidbError::Timeout,
        ureq::Error::Io(_)
        | ureq::Error::HostNotFound
        | ureq::Error::ConnectionFailed
        | ureq::Error::Tls(_) => AnidbError::Network(e.to_string()),
        // Our own request cannot be built, or the server loops us around: retrying changes nothing.
        ureq::Error::BadUri(_)
        | ureq::Error::Http(_)
        | ureq::Error::InvalidProxyUrl
        | ureq::Error::RedirectFailed
        | ureq::Error::TooManyRedirects => AnidbError::Protocol(e.to_string()),
        // The server answered with malformed HTTP, or an error ureq may add later.
        other => AnidbError::Http(other.to_string()),
    }
}

/// Parse the HTTP API `request=anime` XML document.
pub fn parse_anime_xml(xml: &str) -> Result<AnimeXml> {
    let mut r = Reader::from_str(xml);
    r.config_mut().trim_text(false);
    let mut a = AnimeXml::default();
    let mut path: Vec<String> = Vec::new();
    let mut cur_title: Option<Title> = None;
    let mut cur_tag: Option<Tag> = None;
    let mut cur_ep: Option<XmlEpisode> = None;
    let mut cur_creator: Option<String> = None;
    let mut cur_rating_count: i64 = 0;
    let mut text = String::new();
    let mut error_code: Option<String> = None;
    loop {
        match r.read_event().map_err(xerr)? {
            Event::Start(e) => {
                let name = e.local_name().as_ref().to_string();
                text.clear();
                let parent = path.last().map(String::as_str).unwrap_or("");
                match (parent, name.as_str()) {
                    ("", "error") => error_code = attr(&e, "code"),
                    ("", "anime") => {
                        a.aid = attr(&e, "id").and_then(|v| v.parse().ok()).unwrap_or(0);
                        a.restricted = attr(&e, "restricted").is_some_and(|v| v == "true");
                    }
                    ("titles", "title") | ("episode", "title") => {
                        cur_title = Some(Title {
                            name: String::new(),
                            lang: attr(&e, "xml:lang").unwrap_or_else(|| "x-unk".into()),
                            kind: attr(&e, "type").unwrap_or_else(|| "official".into()),
                        });
                    }
                    ("relatedanime", "anime") => {
                        if let Some(id) = attr(&e, "id").and_then(|v| v.parse().ok()) {
                            a.related.push((id, attr(&e, "type").unwrap_or_default()));
                        }
                    }
                    ("tags", "tag") => {
                        cur_tag = Some(Tag {
                            id: attr(&e, "id").and_then(|v| v.parse().ok()).unwrap_or(0),
                            name: String::new(),
                            weight: attr(&e, "weight").and_then(|v| v.parse().ok()).unwrap_or(0),
                            spoiler: attr(&e, "localspoiler").is_some_and(|v| v == "true")
                                || attr(&e, "globalspoiler").is_some_and(|v| v == "true"),
                        });
                    }
                    ("creators", "name") => cur_creator = attr(&e, "type"),
                    ("episodes", "episode") => {
                        cur_ep = Some(XmlEpisode {
                            eid: attr(&e, "id").and_then(|v| v.parse().ok()).unwrap_or(0),
                            ..Default::default()
                        });
                    }
                    ("episode", "epno") => {
                        if let Some(ep) = cur_ep.as_mut() {
                            ep.epno.kind =
                                attr(&e, "type").and_then(|v| v.parse().ok()).unwrap_or(1);
                        }
                    }
                    ("episode", "rating") => {
                        if let Some(ep) = cur_ep.as_mut() {
                            ep.votes = attr(&e, "votes").and_then(|v| v.parse().ok()).unwrap_or(0);
                        }
                    }
                    ("ratings", _) => {
                        cur_rating_count =
                            attr(&e, "count").and_then(|v| v.parse().ok()).unwrap_or(0);
                    }
                    _ => {}
                }
                path.push(name);
            }
            Event::Text(t) => {
                text.push_str(&t);
            }
            Event::GeneralRef(g) => text.push_str(&general_ref(&g)),
            Event::CData(t) => text.push_str(&t),
            Event::End(_) => {
                let name = path.pop().unwrap_or_default();
                let parent = path.last().map(String::as_str).unwrap_or("");
                let value = std::mem::take(&mut text).trim().to_string();
                match (parent, name.as_str()) {
                    ("", "error") => {
                        return Err(api_error(error_code.as_deref(), &value));
                    }
                    ("anime", "type") => a.type_name = Some(value),
                    ("anime", "episodecount") => a.episode_count = value.parse().unwrap_or(0),
                    ("anime", "startdate") => a.start_date = Some(value),
                    ("anime", "enddate") => a.end_date = Some(value),
                    ("anime", "url") => a.url = Some(value),
                    ("anime", "description") => a.description = Some(value),
                    ("anime", "picture") => a.picture = Some(value),
                    ("titles", "title") => {
                        if let Some(mut t) = cur_title.take() {
                            t.name = value;
                            a.titles.push(t);
                        }
                    }
                    ("episode", "title") => {
                        if let (Some(mut t), Some(ep)) = (cur_title.take(), cur_ep.as_mut()) {
                            t.name = value;
                            ep.titles.push(t);
                        }
                    }
                    ("ratings", "permanent") => {
                        a.rating = value.parse().unwrap_or(0.0);
                        a.votes = cur_rating_count;
                    }
                    ("ratings", "temporary") => {
                        a.temp_rating = value.parse().unwrap_or(0.0);
                        a.temp_votes = cur_rating_count;
                    }
                    ("ratings", "review") => {
                        a.review_rating = value.parse().unwrap_or(0.0);
                        a.reviews = cur_rating_count;
                    }
                    ("tag", "name") => {
                        if let Some(t) = cur_tag.as_mut() {
                            t.name = value;
                        }
                    }
                    ("tags", "tag") => {
                        if let Some(t) = cur_tag.take() {
                            a.tags.push(t);
                        }
                    }
                    ("creators", "name") => {
                        a.creators
                            .push((cur_creator.take().unwrap_or_default(), value));
                    }
                    ("episode", "epno") => {
                        if let Some(ep) = cur_ep.as_mut() {
                            let kind = ep.epno.kind;
                            ep.epno = EpNo::parse(&value);
                            if kind != 0 {
                                ep.epno.kind = kind;
                            }
                        }
                    }
                    ("episode", "length") => {
                        if let Some(ep) = cur_ep.as_mut() {
                            ep.length = value.parse().unwrap_or(0);
                        }
                    }
                    ("episode", "airdate") => {
                        if let Some(ep) = cur_ep.as_mut() {
                            ep.airdate = Some(value);
                        }
                    }
                    ("episode", "rating") => {
                        if let Some(ep) = cur_ep.as_mut() {
                            ep.rating = value.parse().unwrap_or(0.0);
                        }
                    }
                    ("episode", "summary") => {
                        if let Some(ep) = cur_ep.as_mut() {
                            ep.summary = Some(value);
                        }
                    }
                    ("episodes", "episode") => {
                        if let Some(ep) = cur_ep.take() {
                            a.episodes.push(ep);
                        }
                    }
                    _ => {}
                }
            }
            Event::Empty(e) => {
                let name = e.local_name().as_ref().to_string();
                if name == "error" {
                    return Err(api_error(attr(&e, "code").as_deref(), ""));
                }
            }
            Event::Eof => {
                if !path.is_empty() {
                    return Err(AnidbError::Xml("truncated XML document".into()));
                }
                break;
            }
            _ => {}
        }
    }
    if a.aid == 0 {
        return Err(AnidbError::Xml("no <anime> element in reply".into()));
    }
    Ok(a)
}

/// One row of the anime titles dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpTitle {
    pub aid: i64,
    pub kind: String,
    pub lang: String,
    pub name: String,
}

/// Resource bounds for one titles dump. Exceeding any of them rejects the whole dump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TitlesLimits {
    /// Bytes accepted from the server (the possibly gzip compressed body).
    pub max_compressed: u64,
    /// Bytes of XML accepted after decompression.
    pub max_decompressed: u64,
    /// `<anime>` elements.
    pub max_anime: usize,
    /// `<title>` elements over the whole document.
    pub max_titles: usize,
    /// Bytes of one title text.
    pub max_name_bytes: usize,
}

impl Default for TitlesLimits {
    fn default() -> Self {
        TitlesLimits {
            max_compressed: 32 << 20,
            max_decompressed: 128 << 20,
            max_anime: 250_000,
            max_titles: 2_000_000,
            max_name_bytes: 2048,
        }
    }
}

/// Longest accepted `type` / `xml:lang` attribute value.
const MAX_TITLE_ATTR_BYTES: usize = 32;

/// Why a reader below failed, recorded where the cause is still known: the XML parser and the
/// gzip decoder only see an opaque `io::Error`.
type Fault = Cell<Option<AnidbError>>;

/// Counts and caps the bytes read from the network (or any other source) and remembers
/// transport failures.
struct Source<'a, R> {
    inner: R,
    read: u64,
    limit: u64,
    fault: &'a Fault,
}

impl<R: Read> Read for Source<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // One byte past the limit is enough to tell that the body is too large.
        let room = self.limit.saturating_sub(self.read).saturating_add(1);
        let take = buf.len().min(usize::try_from(room).unwrap_or(usize::MAX));
        match self.inner.read(&mut buf[..take]) {
            Ok(n) => {
                self.read += n as u64;
                if self.read > self.limit {
                    self.fault.set(Some(AnidbError::Protocol(format!(
                        "titles dump larger than the {} byte download limit",
                        self.limit
                    ))));
                    return Err(io::Error::other("download limit exceeded"));
                }
                Ok(n)
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => Err(e),
            Err(e) => {
                self.fault.set(Some(transport_error(ureq::Error::from(e))));
                Err(io::Error::other("body read failed"))
            }
        }
    }
}

/// Fails (instead of silently ending) once more than `left` decompressed bytes are available.
struct Capped<'a, R> {
    inner: R,
    left: u64,
    limit: u64,
    fault: &'a Fault,
}

impl<R: Read> Read for Capped<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            // The limit is reached exactly: only a clean end of stream is acceptable.
            return match self.inner.read(&mut [0u8; 1])? {
                0 => Ok(0),
                _ => {
                    self.fault.set(Some(AnidbError::Protocol(format!(
                        "titles dump larger than the {} byte decompressed limit",
                        self.limit
                    ))));
                    Err(io::Error::other("decompressed limit exceeded"))
                }
            };
        }
        let take = buf
            .len()
            .min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..take])?;
        self.left -= n as u64;
        Ok(n)
    }
}

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// The error behind a failed read: a recorded transport/limit fault, else a corrupt body.
fn read_failure(fault: &Fault, e: &io::Error) -> AnidbError {
    fault
        .take()
        .unwrap_or_else(|| AnidbError::Xml(format!("invalid compressed body: {e}")))
}

fn xml_failure(fault: &Fault, e: quick_xml::Error) -> AnidbError {
    if let Some(cause) = fault.take() {
        return cause;
    }
    match e {
        quick_xml::Error::Io(io) => AnidbError::Xml(format!("invalid compressed body: {io}")),
        other => xerr(other),
    }
}

/// Value of a required, non-empty attribute of at most `max` bytes. Every attribute of the tag
/// is parsed and unescaped, so malformed syntax anywhere in it (including a duplicate name)
/// is an error even after the wanted attribute was found.
fn required_attr(e: &quick_xml::events::BytesStart, name: &str, max: usize) -> Result<String> {
    let mut found = None;
    for a in e.attributes() {
        let a = a.map_err(xerr)?;
        let value = a
            .normalized_value(quick_xml::XmlVersion::default())
            .map_err(xerr)?;
        if a.key.0 == name {
            let value = value.trim();
            if value.is_empty() || value.len() > max {
                return Err(AnidbError::Xml(format!(
                    "attribute {name} of <{}> must be 1..={max} bytes",
                    e.name().0
                )));
            }
            found = Some(value.to_string());
        }
    }
    found
        .ok_or_else(|| AnidbError::Xml(format!("<{}> is missing the {name} attribute", e.name().0)))
}

/// Parse every attribute of a tag whose attributes are otherwise not used.
fn check_attrs(e: &quick_xml::events::BytesStart) -> Result<()> {
    for a in e.attributes() {
        a.map_err(xerr)?
            .normalized_value(quick_xml::XmlVersion::default())
            .map_err(xerr)?;
    }
    Ok(())
}

/// Character of a predefined entity or numeric character reference.
fn ref_char(g: &quick_xml::events::BytesRef) -> Option<char> {
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

fn push_name_text(text: &mut String, s: &str, max: usize) -> Result<()> {
    if text.len() + s.len() > max {
        return Err(AnidbError::Xml(format!("title longer than {max} bytes")));
    }
    text.push_str(s);
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// Before the root element.
    Prolog,
    Root,
    Anime,
    Title,
    /// After the root element's end tag.
    Epilog,
}

/// Validate and stream-parse `anime-titles.xml` (already decompressed). The document must be
/// exactly `<animetitles><anime aid="N"><title type=".." xml:lang="..">name</title>..</anime>..
/// </animetitles>`: every `anime` has a positive unique id and at least one title, every title
/// has non-empty `type`, `xml:lang` and text, and nothing but whitespace, comments and
/// processing instructions surrounds the root. Rows are passed to `out` as soon as their
/// `</title>` is read, so `out` runs on a document that may still turn out invalid: the caller
/// must not make rows permanent before this returns `Ok`. An error from `out` stops parsing.
fn parse_titles_xml<R: BufRead, E: From<AnidbError>>(
    mut input: R,
    limits: &TitlesLimits,
    fault: &Fault,
    mut out: impl FnMut(DumpTitle) -> std::result::Result<(), E>,
) -> std::result::Result<usize, E> {
    macro_rules! bad {
        ($($arg:tt)*) => {
            return Err(E::from(AnidbError::Xml(format!($($arg)*))))
        };
    }
    // Skip a UTF-8 byte order mark at the very start of the stream (and nowhere else). Short
    // reads are collected first; a non-BOM prefix is chained back, so nothing is copied.
    const BOM: [u8; 3] = [0xef, 0xbb, 0xbf];
    let mut head = [0u8; 3];
    let got = read_full(&mut input, &mut head).map_err(|e| read_failure(fault, &e))?;
    let prefix: &[u8] = if head[..got] == BOM {
        &[]
    } else {
        &head[..got]
    };
    let mut r = Reader::from_reader(prefix.chain(input));
    r.config_mut().trim_text(false);
    // Reject `--` inside comments.
    r.config_mut().check_comments = true;
    let mut buf = Vec::new();
    let mut first_event = true;
    let mut scope = Scope::Prolog;
    let mut aid = 0i64;
    let mut seen = HashSet::new();
    let mut anime_titles = 0usize;
    let mut cur: Option<(String, String)> = None;
    let mut text = String::new();
    let mut n = 0usize;
    loop {
        buf.clear();
        let event = r
            .read_event_into(&mut buf)
            .map_err(|e| xml_failure(fault, e))?;
        // A byte order mark is consumed by the reader before the first event.
        let first = std::mem::replace(&mut first_event, false);
        match event {
            Event::Start(e) => match (scope, e.name().0) {
                (Scope::Prolog, "animetitles") => {
                    check_attrs(&e)?;
                    scope = Scope::Root
                }
                (Scope::Prolog, other) => {
                    bad!("root element is <{other}>, expected <animetitles>")
                }
                (Scope::Root, "anime") => {
                    aid = match required_attr(&e, "aid", 20)?.parse::<i64>() {
                        Ok(v) if v > 0 => v,
                        _ => bad!("anime id is not a positive number"),
                    };
                    if seen.len() >= limits.max_anime {
                        bad!("more than {} anime in the titles dump", limits.max_anime);
                    }
                    if !seen.insert(aid) {
                        bad!("anime {aid} appears twice in the titles dump");
                    }
                    anime_titles = 0;
                    scope = Scope::Anime;
                }
                (Scope::Anime, "title") => {
                    cur = Some((
                        required_attr(&e, "type", MAX_TITLE_ATTR_BYTES)?,
                        required_attr(&e, "xml:lang", MAX_TITLE_ATTR_BYTES)?,
                    ));
                    text.clear();
                    scope = Scope::Title;
                }
                (Scope::Epilog, _) => bad!("content after the </animetitles> end tag"),
                (_, other) => bad!("unexpected element <{other}>"),
            },
            Event::Empty(e) => bad!("unexpected empty element <{}/>", e.name().0),
            Event::End(_) => match scope {
                Scope::Title => {
                    let Some((kind, lang)) = cur.take() else {
                        bad!("title without attributes")
                    };
                    let name = text.trim();
                    if name.is_empty() {
                        bad!("anime {aid} has an empty title");
                    }
                    if n >= limits.max_titles {
                        bad!("more than {} titles in the titles dump", limits.max_titles);
                    }
                    out(DumpTitle {
                        aid,
                        kind,
                        lang,
                        name: name.to_string(),
                    })?;
                    n += 1;
                    anime_titles += 1;
                    scope = Scope::Anime;
                }
                Scope::Anime => {
                    if anime_titles == 0 {
                        bad!("anime {aid} has no titles");
                    }
                    scope = Scope::Root;
                }
                Scope::Root => scope = Scope::Epilog,
                // The parser already rejects end tags without a start tag.
                Scope::Prolog | Scope::Epilog => bad!("unbalanced end tag"),
            },
            Event::Text(t) => {
                if scope == Scope::Title {
                    push_name_text(&mut text, &t, limits.max_name_bytes)?;
                } else if !t.chars().all(char::is_whitespace) {
                    bad!("text outside of a title");
                }
            }
            Event::CData(t) => {
                if scope == Scope::Title {
                    push_name_text(&mut text, &t, limits.max_name_bytes)?;
                } else {
                    bad!("text outside of a title");
                }
            }
            Event::GeneralRef(g) => match (scope, ref_char(&g)) {
                (Scope::Title, Some(c)) => {
                    push_name_text(&mut text, c.encode_utf8(&mut [0; 4]), limits.max_name_bytes)?
                }
                (Scope::Title, None) => bad!("unsupported entity reference in a title"),
                _ => bad!("text outside of a title"),
            },
            Event::DocType(_) => bad!("document type declarations are not accepted"),
            Event::Decl(_) => {
                if !first {
                    bad!("an XML declaration is only allowed at the very start of the document");
                }
            }
            Event::PI(_) | Event::Comment(_) => {}
            Event::Eof => {
                match scope {
                    Scope::Epilog => {}
                    Scope::Prolog => bad!("no <animetitles> root element"),
                    _ => bad!("truncated titles dump"),
                }
                if n == 0 {
                    bad!("titles dump contains no titles");
                }
                return Ok(n);
            }
        }
    }
}

/// Read one titles dump from `source` (a gzip file or plain XML, told apart by the gzip magic)
/// and pass every title to `out`, using constant memory regardless of the dump size.
///
/// `Ok` is returned only for a complete, valid document: the XML is closed and well formed, the
/// compressed and decompressed limits held, the gzip trailer (CRC and length) matched and no
/// data follows the compressed stream. Any other outcome is an error, but rows already handed
/// to `out` stay handed over: stage them (the library must not be written, let alone locked,
/// while the network is read) and apply them only after this returns `Ok`, as
/// `Db::import_title_dump` does. Errors of `out` are returned unchanged.
pub fn read_titles_dump<R: Read, E: From<AnidbError>>(
    source: R,
    limits: &TitlesLimits,
    out: impl FnMut(DumpTitle) -> std::result::Result<(), E>,
) -> std::result::Result<usize, E> {
    let fault = Fault::new(None);
    let mut source = Source {
        inner: source,
        read: 0,
        limit: limits.max_compressed,
        fault: &fault,
    };
    let mut magic = [0u8; 2];
    let got = read_full(&mut source, &mut magic).map_err(|e| read_failure(&fault, &e))?;
    let body = (&magic[..got]).chain(source);
    if magic == [0x1f, 0x8b] {
        let mut gz = flate2::bufread::GzDecoder::new(BufReader::with_capacity(32 << 10, body));
        let n = {
            let capped = Capped {
                inner: &mut gz,
                left: limits.max_decompressed,
                limit: limits.max_decompressed,
                fault: &fault,
            };
            parse_titles_xml(
                BufReader::with_capacity(64 << 10, capped),
                limits,
                &fault,
                out,
            )?
        };
        // The XML parser only ends at the decoder's end of stream, which is reported after
        // the CRC and length trailer matched. Nothing may follow the single gzip member.
        let rest = gz
            .get_mut()
            .fill_buf()
            .map_err(|e| read_failure(&fault, &e))?;
        if !rest.is_empty() {
            return Err(E::from(AnidbError::Xml(
                "unexpected data after the compressed titles dump".into(),
            )));
        }
        Ok(n)
    } else {
        let capped = Capped {
            inner: body,
            left: limits.max_decompressed,
            limit: limits.max_decompressed,
            fault: &fault,
        };
        parse_titles_xml(
            BufReader::with_capacity(64 << 10, capped),
            limits,
            &fault,
            out,
        )
    }
}

/// HTTP client for the anime API, the title dump and images. Every request, whatever its kind,
/// first takes a slot from the process-wide [`Limits`] (at least 2 s between requests, saved
/// before sending), so no number of clients can exceed AniDB's HTTP rate.
pub struct HttpClient {
    pub client: String,
    pub client_version: u32,
    limits: Arc<Limits>,
    agent: ureq::Agent,
    anime_api: String,
    titles_dump_url: String,
}

impl HttpClient {
    pub fn new(client: impl Into<String>, client_version: u32, limits: Arc<Limits>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(60)))
            .user_agent(format!("Animatic/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        HttpClient {
            client: client.into(),
            client_version,
            limits,
            agent,
            anime_api: HTTP_API.to_string(),
            titles_dump_url: TITLES_DUMP.to_string(),
        }
    }

    /// Send the anime and title dump requests to local fixtures instead of AniDB, and never
    /// through a proxy picked up from the environment. For tests only.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_loopback_endpoints(
        mut self,
        anime_api: impl Into<String>,
        titles_dump_url: impl Into<String>,
    ) -> Self {
        self.anime_api = anime_api.into();
        self.titles_dump_url = titles_dump_url.into();
        self.agent = ureq::Agent::config_builder()
            .proxy(None)
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(format!("Animatic/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        self
    }

    /// The registered client name is required on every request; without it nothing is sent.
    /// Callers that record a request attempt before sending check this first, so an unsendable
    /// request is not counted as an attempt.
    pub fn check_client(&self) -> Result<()> {
        if self.client.is_empty() {
            return Err(AnidbError::Auth("no AniDB client name configured".into()));
        }
        Ok(())
    }

    pub fn get_bytes(&mut self, url: &str, limit: u64) -> Result<Vec<u8>> {
        self.limits.wait_http()?;
        let mut resp = self.agent.get(url).call().map_err(transport_error)?;
        resp.body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(transport_error)
    }

    /// Fetch the full anime record. AniDB asks clients not to request the same anime more than
    /// once a day; the caller (`animatic-core`) enforces that from the saved attempt time.
    pub fn anime(&mut self, aid: i64) -> Result<AnimeXml> {
        self.check_client()?;
        let url = format!(
            "{}?request=anime&client={}&clientver={}&protover=1&aid={aid}",
            self.anime_api, self.client, self.client_version
        );
        let bytes = self.get_bytes(&url, 32 << 20)?;
        let text = decode_maybe_gzip(&bytes)?;
        parse_anime_xml(&text)
    }

    /// Download the daily titles dump and stream its rows to `out` (see [`read_titles_dump`]
    /// for the guarantees and for what the caller must do to stay atomic). The caller enforces
    /// the once-a-day rule; this method sends one request every time it is called.
    pub fn titles_dump<E: From<AnidbError>>(
        &mut self,
        limits: &TitlesLimits,
        out: impl FnMut(DumpTitle) -> std::result::Result<(), E>,
    ) -> std::result::Result<usize, E> {
        let url = self.titles_dump_url.clone();
        self.titles_dump_from(&url, limits, out)
    }

    fn titles_dump_from<E: From<AnidbError>>(
        &mut self,
        url: &str,
        limits: &TitlesLimits,
        out: impl FnMut(DumpTitle) -> std::result::Result<(), E>,
    ) -> std::result::Result<usize, E> {
        self.limits.wait_http()?;
        let resp = self.agent.get(url).call().map_err(transport_error)?;
        let mut body = resp.into_body();
        // Unlimited here: `read_titles_dump` enforces the byte limits itself.
        read_titles_dump(body.as_reader(), limits, out)
    }

    pub fn image(&mut self, server: &str, picname: &str) -> Result<Vec<u8>> {
        let server = if server.is_empty() {
            DEFAULT_IMAGE_SERVER
        } else {
            server
        };
        self.image_from(&format!("https://{server}"), picname)
    }

    fn image_from(&mut self, base: &str, picname: &str) -> Result<Vec<u8>> {
        self.get_bytes(&format!("{base}/images/main/{picname}"), 16 << 20)
    }
}

/// Decode a body that may still be gzip compressed (AniDB always gzips HTTP API replies).
pub fn decode_maybe_gzip(bytes: &[u8]) -> Result<String> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut s = String::new();
        flate2::read::MultiGzDecoder::new(bytes)
            .read_to_string(&mut s)
            .map_err(|e| AnidbError::Xml(format!("invalid compressed body: {e}")))?;
        Ok(s)
    } else {
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{HttpFixture, HttpReply, TestLimits, test_limits};

    #[test]
    fn transport_failures_are_classified_by_cause() {
        let io = || std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        assert!(matches!(
            transport_error(ureq::Error::Io(io())),
            AnidbError::Network(_)
        ));
        assert!(matches!(
            transport_error(ureq::Error::HostNotFound),
            AnidbError::Network(_)
        ));
        assert!(matches!(
            transport_error(ureq::Error::ConnectionFailed),
            AnidbError::Network(_)
        ));
        assert!(matches!(
            transport_error(ureq::Error::Timeout(ureq::Timeout::Global)),
            AnidbError::Timeout
        ));
        assert!(matches!(
            transport_error(ureq::Error::StatusCode(503)),
            AnidbError::Http(_)
        ));
        assert!(matches!(
            transport_error(ureq::Error::StatusCode(404)),
            AnidbError::Protocol(_)
        ));
        assert!(matches!(
            transport_error(ureq::Error::BodyExceedsLimit(1)),
            AnidbError::Protocol(_)
        ));
        assert!(matches!(
            decode_maybe_gzip(&[0x1f, 0x8b, 0, 1, 2]),
            Err(AnidbError::Xml(_))
        ));
    }

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<anime id="1" restricted="false">
  <type>TV Series</type>
  <episodecount>13</episodecount>
  <startdate>1999-01-03</startdate>
  <enddate>1999-03-28</enddate>
  <titles>
    <title xml:lang="x-jat" type="main">Seikai no Monshou</title>
    <title xml:lang="en" type="official">Crest of the Stars</title>
  </titles>
  <relatedanime><anime id="4" type="Sequel">Seikai no Senki</anime></relatedanime>
  <url>http://www.sunrise-inc.co.jp/seikai/</url>
  <creators><name id="4303" type="Direction">Nagaoka Yasuchika</name></creators>
  <description>Story &amp; stuff</description>
  <ratings><permanent count="4403">8.17</permanent><temporary count="4422">8.24</temporary><review count="12">8.70</review></ratings>
  <picture>224618.jpg</picture>
  <tags><tag id="36" weight="300" localspoiler="false" globalspoiler="false"><name>military</name></tag></tags>
  <episodes>
    <episode id="1" update="2011-07-01">
      <epno type="1">1</epno><length>25</length><airdate>1999-01-03</airdate><rating votes="29">3.31</rating>
      <title xml:lang="en">Invasion</title><title xml:lang="x-jat">Shinryaku</title>
    </episode>
    <episode id="1235"><epno type="2">S1</epno><length>40</length><title xml:lang="en">Special</title></episode>
  </episodes>
</anime>"#;

    #[test]
    fn parses_anime_document() {
        let a = parse_anime_xml(SAMPLE).unwrap();
        assert_eq!(a.aid, 1);
        assert_eq!(a.type_name.as_deref(), Some("TV Series"));
        assert_eq!(a.episode_count, 13);
        assert_eq!(a.titles.len(), 2);
        assert_eq!(a.titles[0].kind, "main");
        assert_eq!(a.related, vec![(4, "Sequel".to_string())]);
        assert_eq!(a.description.as_deref(), Some("Story & stuff"));
        assert_eq!((a.rating, a.votes), (8.17, 4403));
        assert_eq!((a.review_rating, a.reviews), (8.70, 12));
        assert_eq!(a.tags[0].name, "military");
        assert_eq!(
            a.creators,
            vec![("Direction".into(), "Nagaoka Yasuchika".into())]
        );
        assert_eq!(a.episodes.len(), 2);
        assert_eq!(a.episodes[0].votes, 29);
        assert_eq!(a.episodes[0].titles[1].lang, "x-jat");
        assert_eq!(a.episodes[1].epno, EpNo { kind: 2, number: 1 });
    }

    #[test]
    fn reports_api_errors() {
        assert!(matches!(
            parse_anime_xml("<error code=\"302\">client version missing or invalid</error>"),
            Err(AnidbError::Auth(_))
        ));
        assert!(matches!(
            parse_anime_xml("<error code=\"500\">banned</error>"),
            Err(AnidbError::Banned(_))
        ));
        assert!(matches!(
            parse_anime_xml("<error>Anime not found</error>"),
            Err(AnidbError::Xml(_))
        ));
        assert!(matches!(
            parse_anime_xml("<error code=\"500\"/>"),
            Err(AnidbError::Banned(_))
        ));
    }

    #[test]
    fn rejects_truncated_and_empty_documents() {
        for bad in [
            "",
            "<anime id=\"1\"><titles><title type=\"main\">A</title>",
            "<anime id=\"1\"",
        ] {
            assert!(
                matches!(parse_anime_xml(bad), Err(AnidbError::Xml(_))),
                "{bad:?}"
            );
        }
    }

    const DUMP: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- synthetic fixture -->
<animetitles>
<anime aid="1"><title type="main" xml:lang="x-jat">Alpha &amp; Omega</title>
<title type="official" xml:lang="en">Alpha</title></anime>
<anime aid="2"><title type="main" xml:lang="x-jat">3x3 Eyes</title></anime>
</animetitles>
"#;

    fn gzip(data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    fn read_all(source: &[u8], limits: &TitlesLimits) -> (Result<usize>, Vec<DumpTitle>) {
        let mut rows = Vec::new();
        let n = read_titles_dump(source, limits, |t| {
            rows.push(t);
            Ok(())
        });
        (n, rows)
    }

    fn one_title(aid: i64, name: &str) -> String {
        format!(r#"<anime aid="{aid}"><title type="main" xml:lang="en">{name}</title></anime>"#)
    }

    #[test]
    fn reads_plain_and_gzip_titles_dumps() {
        let limits = TitlesLimits::default();
        let (n, plain) = read_all(DUMP.as_bytes(), &limits);
        assert_eq!(n.unwrap(), 3);
        assert_eq!(
            plain[0],
            DumpTitle {
                aid: 1,
                kind: "main".into(),
                lang: "x-jat".into(),
                name: "Alpha & Omega".into()
            }
        );
        assert_eq!(plain[2].name, "3x3 Eyes");
        let (n, zipped) = read_all(&gzip(DUMP.as_bytes()), &limits);
        assert_eq!(n.unwrap(), 3);
        assert_eq!(zipped, plain);
    }

    #[test]
    fn accepts_a_leading_byte_order_mark_before_the_declaration() {
        let mut doc = vec![0xef, 0xbb, 0xbf];
        doc.extend_from_slice(DUMP.as_bytes());
        let limits = TitlesLimits::default();
        assert_eq!(read_all(&doc, &limits).0.unwrap(), 3);
        assert_eq!(read_all(&gzip(&doc), &limits).0.unwrap(), 3);
    }

    #[test]
    fn rejects_malformed_titles_documents() {
        let title = r#"<title type="main" xml:lang="en">A</title>"#;
        let docs = [
            String::new(),
            "not xml".into(),
            format!("<other>{}</other>", one_title(1, "A")),
            format!("<animetitles>{}", one_title(1, "A")),
            format!(r#"<animetitles><anime aid="1">{}"#, &title[..30]),
            "<animetitles></animetitles>".into(),
            format!(
                r#"<animetitles><anime aid="0">{title}</anime></animetitles>"#
            ),
            format!(
                r#"<animetitles><anime aid="x">{title}</anime></animetitles>"#
            ),
            format!("<animetitles><anime>{title}</anime></animetitles>"),
            format!(
                "<animetitles>{}{}</animetitles>",
                one_title(1, "A"),
                one_title(1, "B")
            ),
            r#"<animetitles><anime aid="1"></anime></animetitles>"#.into(),
            r#"<animetitles><anime aid="1"><title xml:lang="en">A</title></anime></animetitles>"#
                .into(),
            r#"<animetitles><anime aid="1"><title type="main">A</title></anime></animetitles>"#
                .into(),
            format!("<animetitles>{}</animetitles>", one_title(1, "   ")),
            format!("<animetitles>{}</animetitles>", one_title(1, "A&nbsp;B")),
            format!(
                "<animetitles>{}</animetitles><animetitles/>",
                one_title(1, "A")
            ),
            format!("<animetitles>{}</animetitles>junk", one_title(1, "A")),
            format!(
                r#"<animetitles><anime aid="1"><note/>{title}</anime></animetitles>"#
            ),
            r#"<animetitles><anime aid="1"><title type="main" xml:lang="en"><b>A</b></title></anime></animetitles>"#
                .into(),
            format!("<!DOCTYPE animetitles><animetitles>{}</animetitles>", one_title(1, "A")),
            r#"<animetitles><anime aid="1"></animetitles>"#.into(),
            // Attributes after the wanted one are still validated.
            format!(r#"<animetitles><anime aid="1" aid="2">{title}</anime></animetitles>"#),
            format!(r#"<animetitles><anime aid="1" broken>{title}</anime></animetitles>"#),
            format!(r#"<animetitles broken><anime aid="1">{title}</anime></animetitles>"#),
            r#"<animetitles><anime aid="1"><title type="main" xml:lang="en" xml:lang="ja">A</title></anime></animetitles>"#
                .into(),
            r#"<animetitles><anime aid="1"><title type="main" xml:lang="en" x=>A</title></anime></animetitles>"#
                .into(),
            // An XML declaration is only valid as the very first thing.
            format!(r#"<?xml version="1.0"?><?xml version="1.0"?><animetitles>{}</animetitles>"#, one_title(1, "A")),
            format!(r#"<animetitles><?xml version="1.0"?>{}</animetitles>"#, one_title(1, "A")),
            format!(r#"<animetitles><anime aid="1"><?xml version="1.0"?>{title}</anime></animetitles>"#),
            format!(r#"<animetitles>{}</animetitles><?xml version="1.0"?>"#, one_title(1, "A")),
            format!(r#" <?xml version="1.0"?><animetitles>{}</animetitles>"#, one_title(1, "A")),
            format!(r#"<!-- c --><?xml version="1.0"?><animetitles>{}</animetitles>"#, one_title(1, "A")),
            // `--` inside a comment is not well formed.
            format!("<animetitles><!--a--b-->{}</animetitles>", one_title(1, "A")),
            format!("<!--a--b--><animetitles>{}</animetitles>", one_title(1, "A")),
        ];
        for doc in docs {
            assert!(
                matches!(
                    read_all(doc.as_bytes(), &TitlesLimits::default()).0,
                    Err(AnidbError::Xml(_))
                ),
                "{doc:?}"
            );
            assert!(
                matches!(
                    read_all(&gzip(doc.as_bytes()), &TitlesLimits::default()).0,
                    Err(AnidbError::Xml(_))
                ),
                "gzip {doc:?}"
            );
        }
    }

    #[test]
    fn gzip_trailer_and_framing_errors_are_never_accepted() {
        let limits = TitlesLimits::default();
        let good = gzip(DUMP.as_bytes());

        // The XML is complete and closed, only the CRC is wrong: still an error, even though
        // every row was already delivered.
        let mut bad_crc = good.clone();
        let crc = bad_crc.len() - 8;
        bad_crc[crc] ^= 0xff;
        let (n, rows) = read_all(&bad_crc, &limits);
        assert!(matches!(n, Err(AnidbError::Xml(_))), "{n:?}");
        assert_eq!(rows.len(), 3);

        let mut bad_len = good.clone();
        let isize = bad_len.len() - 1;
        bad_len[isize] ^= 0x01;
        assert!(matches!(
            read_all(&bad_len, &limits).0,
            Err(AnidbError::Xml(_))
        ));

        for cut in [1, 4, 8, 12, good.len() / 2] {
            let (n, _) = read_all(&good[..good.len() - cut], &limits);
            assert!(matches!(n, Err(AnidbError::Xml(_))), "cut {cut}: {n:?}");
        }

        let mut trailing = good.clone();
        trailing.extend_from_slice(b"\0\0\0");
        assert!(matches!(
            read_all(&trailing, &limits).0,
            Err(AnidbError::Xml(_))
        ));
        let mut second_member = good.clone();
        second_member.extend_from_slice(&good);
        assert!(matches!(
            read_all(&second_member, &limits).0,
            Err(AnidbError::Xml(_))
        ));
    }

    #[test]
    fn titles_dump_limits_reject_oversized_input() {
        let big = format!(
            "<animetitles>{}</animetitles>",
            (1..=200)
                .map(|i| one_title(i, "Repeated title text"))
                .collect::<String>()
        );
        let zipped = gzip(big.as_bytes());
        assert!(zipped.len() < big.len() / 4, "fixture must compress well");
        let fits = TitlesLimits {
            max_decompressed: big.len() as u64,
            max_compressed: big.len() as u64,
            ..TitlesLimits::default()
        };
        assert_eq!(read_all(big.as_bytes(), &fits).0.unwrap(), 200);
        assert_eq!(read_all(&zipped, &fits).0.unwrap(), 200);

        let small_output = TitlesLimits {
            max_decompressed: big.len() as u64 - 1,
            ..fits
        };
        for source in [big.as_bytes(), zipped.as_slice()] {
            assert!(matches!(
                read_all(source, &small_output).0,
                Err(AnidbError::Protocol(_))
            ));
        }
        let small_download = TitlesLimits {
            max_compressed: zipped.len() as u64 - 1,
            ..fits
        };
        assert!(matches!(
            read_all(&zipped, &small_download).0,
            Err(AnidbError::Protocol(_))
        ));
        for limits in [
            TitlesLimits {
                max_anime: 199,
                ..fits
            },
            TitlesLimits {
                max_titles: 199,
                ..fits
            },
            TitlesLimits {
                max_name_bytes: 5,
                ..fits
            },
        ] {
            assert!(matches!(
                read_all(&zipped, &limits).0,
                Err(AnidbError::Xml(_))
            ));
        }
    }

    #[test]
    fn sink_errors_stop_the_dump_unchanged() {
        let mut seen = 0;
        let r = read_titles_dump(&gzip(DUMP.as_bytes())[..], &TitlesLimits::default(), |_| {
            seen += 1;
            Err(AnidbError::Network("sink".into()))
        });
        assert!(matches!(r, Err(AnidbError::Network(m)) if m == "sink"));
        assert_eq!(seen, 1);
    }

    /// A client over its own manual clock that talks to `fixture` instead of AniDB.
    fn client_for(fixture: &HttpFixture) -> (HttpClient, TestLimits) {
        let t = test_limits();
        let client = HttpClient::new("synthetic", 1, t.limits.clone())
            .with_loopback_endpoints(fixture.url("/httpapi"), fixture.url("/titles.xml.gz"));
        (client, t)
    }

    fn fetch(reply: HttpReply) -> (Result<usize>, Vec<DumpTitle>) {
        let fixture = HttpFixture::start(move |_, _| reply.clone());
        let (mut client, _t) = client_for(&fixture);
        let mut rows = Vec::new();
        let n = client.titles_dump(&TitlesLimits::default(), |t| {
            rows.push(t);
            Ok(())
        });
        (n, rows)
    }

    #[test]
    fn titles_dump_streams_from_a_local_http_server() {
        let (n, rows) = fetch(HttpReply::ok(gzip(DUMP.as_bytes())));
        assert_eq!(n.unwrap(), 3);
        assert_eq!(rows.len(), 3);

        // The connection ends before the announced length: a transient network failure.
        let (n, _) = fetch(HttpReply::truncated(gzip(DUMP.as_bytes())));
        assert!(matches!(n, Err(AnidbError::Network(_))), "{n:?}");

        // A corrupt compressed body is a permanent failure, not a network problem.
        let mut bad = gzip(DUMP.as_bytes());
        let crc = bad.len() - 8;
        bad[crc] ^= 0xff;
        let (n, _) = fetch(HttpReply::ok(bad));
        assert!(matches!(n, Err(AnidbError::Xml(_))), "{n:?}");

        let (n, rows) = fetch(HttpReply::status("503 Service Unavailable"));
        assert!(matches!(n, Err(AnidbError::Http(_))), "{n:?}");
        assert!(rows.is_empty());
    }

    #[test]
    fn every_kind_of_http_request_is_spaced_by_the_shared_limiter() {
        let t = test_limits();
        let arrivals = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let (log, clock) = (arrivals.clone(), t.clock.clone());
        let dump = gzip(DUMP.as_bytes());
        let fixture = HttpFixture::start(move |index, _| {
            use crate::limits::Clock;
            log.lock().push(clock.now_ms());
            match index {
                0 => HttpReply::ok(SAMPLE.as_bytes().to_vec()),
                1 => HttpReply::ok(dump.clone()),
                _ => HttpReply::ok(b"picture".to_vec()),
            }
        });
        let new_client = || {
            HttpClient::new("synthetic", 1, t.limits.clone())
                .with_loopback_endpoints(fixture.url("/httpapi"), fixture.url("/titles.xml.gz"))
        };

        // Different clients (as in different queue runs), all kinds of request, one limiter.
        assert_eq!(new_client().anime(1).unwrap().aid, 1);
        let mut sink_rows = 0;
        new_client()
            .titles_dump::<AnidbError>(&TitlesLimits::default(), |_| {
                sink_rows += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(sink_rows, 3);
        assert_eq!(
            new_client().image_from(fixture.base(), "p.jpg").unwrap(),
            b"picture"
        );
        assert_eq!(
            new_client()
                .get_bytes(&fixture.url("/other"), 1024)
                .unwrap(),
            b"picture"
        );

        let arrivals = arrivals.lock().clone();
        assert_eq!(arrivals.len(), 4);
        let gaps: Vec<i64> = arrivals.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(
            gaps,
            [2000, 2000, 2000],
            "no request is exempt from the spacing"
        );
        assert_eq!(t.store.sends(crate::limits::Protocol::Http), arrivals);
    }

    #[test]
    fn http_spacing_continues_after_a_restart() {
        let fixture = HttpFixture::start(|_, _| HttpReply::ok(b"x".to_vec()));
        let t = test_limits();
        let mut first = HttpClient::new("synthetic", 1, t.limits.clone());
        first.get_bytes(&fixture.url("/a"), 16).unwrap();
        t.clock.advance(Duration::from_millis(700));
        // A new process: new limiter, same saved state.
        let restarted = crate::limits::Limits::new(t.clock.clone(), t.store.clone());
        let before = t.clock.sleeps().len();
        let mut second = HttpClient::new("synthetic", 1, restarted);
        second.get_bytes(&fixture.url("/b"), 16).unwrap();
        assert_eq!(
            t.clock.sleeps()[before..],
            [Duration::from_millis(1300)],
            "2 s after the saved send time"
        );
    }

    #[test]
    fn a_client_without_a_name_sends_nothing_and_uses_no_slot() {
        let fixture = HttpFixture::start(|_, _| HttpReply::ok(SAMPLE.as_bytes().to_vec()));
        let t = test_limits();
        let mut client = HttpClient::new("", 1, t.limits.clone())
            .with_loopback_endpoints(fixture.url("/httpapi"), fixture.url("/titles.xml.gz"));
        assert!(matches!(client.anime(1), Err(AnidbError::Auth(_))));
        assert!(client.check_client().is_err());
        assert_eq!(fixture.requests(), 0);
        assert!(t.store.sends(crate::limits::Protocol::Http).is_empty());
    }
}
