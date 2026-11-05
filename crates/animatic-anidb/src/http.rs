//! AniDB HTTP API (anime XML), anime title dump and image downloads.

use std::io::{BufRead, Read};
use std::time::{Duration, Instant};

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};

use crate::commands::EpNo;
use crate::error::{AnidbError, Result};

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

/// Stream-parse `anime-titles.xml` (already decompressed).
pub fn parse_titles_dump<R: BufRead>(input: R, mut out: impl FnMut(DumpTitle)) -> Result<usize> {
    let mut r = Reader::from_reader(input);
    r.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut aid = 0i64;
    let mut cur: Option<(String, String)> = None;
    let mut text = String::new();
    let mut n = 0;
    let mut depth = 0usize;
    loop {
        match r.read_event_into(&mut buf).map_err(xerr)? {
            Event::Start(e) => {
                depth += 1;
                match e.local_name().as_ref() {
                    "anime" => aid = attr(&e, "aid").and_then(|v| v.parse().ok()).unwrap_or(0),
                    "title" => {
                        text.clear();
                        cur = Some((
                            attr(&e, "type").unwrap_or_else(|| "synonym".into()),
                            attr(&e, "xml:lang").unwrap_or_else(|| "x-unk".into()),
                        ));
                    }
                    _ => {}
                }
            }
            Event::Text(t) => text.push_str(&t),
            Event::GeneralRef(g) => text.push_str(&general_ref(&g)),
            Event::End(e) => {
                depth = depth.saturating_sub(1);
                if e.local_name().as_ref() != "title" {
                    buf.clear();
                    continue;
                }
                if let Some((kind, lang)) = cur.take() {
                    out(DumpTitle {
                        aid,
                        kind,
                        lang,
                        name: std::mem::take(&mut text).trim().to_string(),
                    });
                    n += 1;
                }
            }
            Event::Eof => {
                if depth != 0 {
                    return Err(AnidbError::Xml("truncated titles dump".into()));
                }
                if n == 0 {
                    return Err(AnidbError::Xml("titles dump contains no titles".into()));
                }
                break;
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(n)
}

/// HTTP API client with the mandatory 2 second spacing between requests.
pub struct HttpClient {
    pub client: String,
    pub client_version: u32,
    last: Option<Instant>,
    agent: ureq::Agent,
}

impl HttpClient {
    pub fn new(client: impl Into<String>, client_version: u32) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(60)))
            .user_agent(format!("Animatic/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        HttpClient {
            client: client.into(),
            client_version,
            last: None,
            agent,
        }
    }

    fn pace(&mut self) {
        if let Some(last) = self.last {
            let since = last.elapsed();
            if since < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_secs(2) - since);
            }
        }
        self.last = Some(Instant::now());
    }

    pub fn get_bytes(&mut self, url: &str, limit: u64) -> Result<Vec<u8>> {
        let mut resp = self.agent.get(url).call().map_err(transport_error)?;
        resp.body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(transport_error)
    }

    /// Fetch the full anime record. AniDB asks clients not to request the same anime more than once a day.
    pub fn anime(&mut self, aid: i64) -> Result<AnimeXml> {
        if self.client.is_empty() {
            return Err(AnidbError::Auth("no AniDB client name configured".into()));
        }
        self.pace();
        let url = format!(
            "{HTTP_API}?request=anime&client={}&clientver={}&protover=1&aid={aid}",
            self.client, self.client_version
        );
        let bytes = self.get_bytes(&url, 32 << 20)?;
        let text = decode_maybe_gzip(&bytes)?;
        parse_anime_xml(&text)
    }

    /// Download and parse the daily titles dump.
    pub fn titles_dump(&mut self, out: impl FnMut(DumpTitle)) -> Result<usize> {
        let bytes = self.get_bytes(TITLES_DUMP, 256 << 20)?;
        let text = decode_maybe_gzip(&bytes)?;
        parse_titles_dump(text.as_bytes(), out)
    }

    pub fn image(&mut self, server: &str, picname: &str) -> Result<Vec<u8>> {
        let server = if server.is_empty() {
            DEFAULT_IMAGE_SERVER
        } else {
            server
        };
        self.get_bytes(&format!("https://{server}/images/main/{picname}"), 16 << 20)
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

    #[test]
    fn parses_titles_dump() {
        let xml = r#"<animetitles><anime aid="1"><title type="main" xml:lang="x-jat">Seikai no Monshou</title>
          <title type="official" xml:lang="en">Crest of the Stars</title></anime>
          <anime aid="2"><title type="main" xml:lang="x-jat">3x3 Eyes</title></anime></animetitles>"#;
        let mut rows = Vec::new();
        let n = parse_titles_dump(xml.as_bytes(), |t| rows.push(t)).unwrap();
        assert_eq!(n, 3);
        assert_eq!(
            rows[2],
            DumpTitle {
                aid: 2,
                kind: "main".into(),
                lang: "x-jat".into(),
                name: "3x3 Eyes".into()
            }
        );
    }
}
