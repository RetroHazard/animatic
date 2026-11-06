//! Import of a MyList export, the supported way to bring a whole AniDB MyList into the library.
//!
//! No API returns a whole MyList and walking it with `MYLIST` is the bulk extraction AniDB
//! forbids, so the user asks AniDB for an export (`MYLISTEXPORT`, see [`crate::sync::Command`]),
//! downloads the file from AniDB's web export page and imports that file here. Nothing in this
//! module touches the network.
//!
//! The file is read like the title dump (`Db::import_title_dump`): streamed and bounded
//! ([`ExportLimits`]: file size, decoded size, container entries, anime/episode/file counts,
//! string length, tag span, nesting depth), validated completely while it is spooled into a
//! private temporary SQLite database, and only then merged into the library in **one** write
//! transaction. A file that is truncated, malformed, over a limit or cancelled changes nothing.
//!
//! The container is told from the content, not the extension: a `.tgz`/`.tar.gz`/`.tar`
//! archive (every `.xml` entry is a document), a `.zip` archive (likewise), a gzip-compressed
//! XML file or plain XML. The XML structure is documented in
//! [Storage and network](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#mylist-export-import);
//! the XML reader is the private `xml` submodule.
//!
//! # Merge rules
//!
//! * **Absence is never established.** Local MyList rows the export does not list are kept and
//!   only counted ([`ImportReport::not_in_export`]): the export is a snapshot that may be a day
//!   old, and a MyList entry added since would otherwise be deleted.
//! * **Pending local intent wins.** An entry with any queued add, edit or delete (blocked rows
//!   included) is not touched at all (rules 13 and 14).
//! * **Richer local data is kept.** Anime, episode, file and group rows are created when
//!   missing; an existing row only gets blanks filled. An existing MyList row takes the
//!   export's state, watched flag and date, and a storage text when the export has one; it
//!   keeps its own ids and anything the export leaves out.
//! * **Idempotent.** A second import of the same file writes the same values again, so no
//!   table changes.

mod xml;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::{Connection, ErrorCode, params};

pub use xml::{AnimeRecord, EpisodeRecord, FileRecord, Totals};

use crate::db::Db;
use crate::error::{Error, Result};
use crate::library::{MylistEntry, anime_type_id};
use crate::limits::{MYLIST_EXPORT_ID, RequestKind};

/// Bounds for reading an export file. The defaults hold far more than any real MyList (AniDB
/// has about 14 000 anime) and stop a hostile or corrupt file long before it hurts.
#[derive(Debug, Clone)]
pub struct ExportLimits {
    /// Size of the file on disk.
    pub max_file_bytes: u64,
    /// Bytes after decompression, over all documents of the file.
    pub max_decoded_bytes: u64,
    /// Entries of a tar or zip archive.
    pub max_entries: usize,
    pub max_anime: usize,
    pub max_episodes_per_anime: usize,
    pub max_files_per_anime: usize,
    /// `file` elements over the whole export.
    pub max_files: usize,
    /// Bytes of any one text value or attribute.
    pub max_text_bytes: usize,
    /// Bytes between two `<` characters, so that no single text node or tag can be huge.
    pub max_span_bytes: usize,
    /// Nesting depth of elements.
    pub max_depth: usize,
}

impl Default for ExportLimits {
    fn default() -> Self {
        ExportLimits {
            max_file_bytes: 256 << 20,
            max_decoded_bytes: 1 << 30,
            max_entries: 100_000,
            max_anime: 250_000,
            max_episodes_per_anime: 20_000,
            max_files_per_anime: 100_000,
            max_files: 5_000_000,
            max_text_bytes: 2048,
            max_span_bytes: 1 << 20,
            max_depth: 24,
        }
    }
}

/// What a failed read recorded, for the error the caller sees (a reader can only return an
/// `io::Error`, which parsers re-wrap).
#[derive(Clone, Default)]
struct Fault(Rc<RefCell<Option<String>>>);

impl Fault {
    fn set(&self, message: String) {
        *self.0.borrow_mut() = Some(message);
    }

    fn take(&self) -> Option<String> {
        self.0.borrow_mut().take()
    }
}

fn io_failure(fault: &Fault, e: &io::Error) -> Error {
    match fault.take() {
        Some(cause) => Error::Other(cause),
        None => Error::Other(format!("the export cannot be read: {e}")),
    }
}

/// Fails (instead of silently ending) once more bytes than the shared `budget` allows are read.
struct Capped<'a, R> {
    inner: R,
    budget: Rc<Cell<u64>>,
    limit: u64,
    what: &'static str,
    fault: &'a Fault,
}

impl<R: Read> Read for Capped<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.budget.get();
        if left == 0 {
            // Exactly at the limit: only a clean end of the stream is acceptable.
            return match self.inner.read(&mut [0u8; 1])? {
                0 => Ok(0),
                _ => {
                    self.fault.set(format!(
                        "{} is larger than the {} byte limit",
                        self.what, self.limit
                    ));
                    Err(io::Error::other("limit exceeded"))
                }
            };
        }
        let take = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..take])?;
        self.budget.set(left - n as u64);
        Ok(n)
    }
}

/// Fails when more than `max` bytes pass without a `<`: a text node or tag that large is not an
/// export, and the XML reader would otherwise hold all of it in memory.
struct SpanLimit<'a, R> {
    inner: R,
    run: usize,
    max: usize,
    fault: &'a Fault,
}

impl<R: Read> Read for SpanLimit<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        for &b in &buf[..n] {
            if b == b'<' {
                self.run = 0;
            } else {
                self.run += 1;
                if self.run > self.max {
                    self.fault.set(format!(
                        "the export has more than {} bytes between two tags",
                        self.max
                    ));
                    return Err(io::Error::other("span limit exceeded"));
                }
            }
        }
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

fn is_xml_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".xml")
}

/// One XML document from `stream`.
fn read_xml<R: Read>(
    stream: R,
    run: &mut xml::Run,
    out: &mut dyn FnMut(AnimeRecord) -> Result<()>,
) -> Result<()> {
    let spans = SpanLimit {
        inner: stream,
        run: 0,
        max: run.limits.max_span_bytes,
        fault: run.fault,
    };
    xml::read_document(BufReader::with_capacity(64 << 10, spans), run, out)
}

/// A tar archive: every `.xml` entry is a document, other regular entries are skipped.
fn read_tar<R: Read>(
    stream: R,
    run: &mut xml::Run,
    out: &mut dyn FnMut(AnimeRecord) -> Result<()>,
) -> Result<()> {
    let fault = run.fault;
    let mut archive = tar::Archive::new(stream);
    let mut names = HashSet::new();
    let mut count = 0usize;
    let mut documents = 0usize;
    for entry in archive.entries().map_err(|e| io_failure(fault, &e))? {
        let entry = entry.map_err(|e| io_failure(fault, &e))?;
        count += 1;
        if count > run.limits.max_entries {
            return Err(Error::Other(format!(
                "the archive has more than {} entries",
                run.limits.max_entries
            )));
        }
        let kind = entry.header().entry_type();
        if kind.is_dir() || kind.is_pax_global_extensions() {
            continue;
        }
        if !kind.is_file() {
            return Err(Error::Other(
                "the archive contains a link or special file".into(),
            ));
        }
        let name = entry
            .path()
            .map_err(|e| io_failure(fault, &e))?
            .to_string_lossy()
            .into_owned();
        if !names.insert(name.clone()) {
            return Err(Error::Other(format!(
                "the archive has the entry {name} twice"
            )));
        }
        if is_xml_name(&name) {
            documents += 1;
            read_xml(entry, run, out)?;
        }
    }
    if documents == 0 {
        return Err(Error::Other(
            "the archive contains no .xml file with a MyList".into(),
        ));
    }
    // Read to the end of the stream: a compressed stream only proves its checksum there.
    let mut rest = archive.into_inner();
    io::copy(&mut rest, &mut io::sink()).map_err(|e| io_failure(fault, &e))?;
    Ok(())
}

fn zip_failure(e: zip::result::ZipError) -> Error {
    Error::Other(format!("the zip archive is invalid ({e})"))
}

/// A zip archive: every `.xml` entry is a document, other regular entries are skipped.
fn read_zip(
    file: File,
    budget: &Rc<Cell<u64>>,
    run: &mut xml::Run,
    out: &mut dyn FnMut(AnimeRecord) -> Result<()>,
) -> Result<()> {
    let mut zip = zip::ZipArchive::new(file).map_err(zip_failure)?;
    if zip.has_duplicate_names() {
        return Err(Error::Other("the archive has duplicate entries".into()));
    }
    if zip.len() > run.limits.max_entries {
        return Err(Error::Other(format!(
            "the archive has more than {} entries",
            run.limits.max_entries
        )));
    }
    let mut documents = 0usize;
    for index in 0..zip.len() {
        let entry = zip.by_index(index).map_err(zip_failure)?;
        if entry.is_dir() {
            continue;
        }
        if entry.unix_mode().is_some_and(|m| m & 0o170000 != 0o100000) {
            return Err(Error::Other(
                "the archive contains a link or special file".into(),
            ));
        }
        if !is_xml_name(&String::from_utf8_lossy(entry.name_raw())) {
            continue;
        }
        documents += 1;
        let capped = Capped {
            inner: entry,
            budget: budget.clone(),
            limit: run.limits.max_decoded_bytes,
            what: "the decompressed export",
            fault: run.fault,
        };
        read_xml(capped, run, out)?;
    }
    if documents == 0 {
        return Err(Error::Other(
            "the archive contains no .xml file with a MyList".into(),
        ));
    }
    Ok(())
}

/// Plain bytes: a tar archive (told by its `ustar` marker) or an XML document.
fn read_plain<R: Read>(
    mut stream: R,
    run: &mut xml::Run,
    out: &mut dyn FnMut(AnimeRecord) -> Result<()>,
) -> Result<()> {
    let mut head = [0u8; 512];
    let n = read_full(&mut stream, &mut head).map_err(|e| io_failure(run.fault, &e))?;
    let is_tar = n >= 262 && &head[257..262] == b"ustar";
    let stream = (&head[..n]).chain(stream);
    if is_tar {
        read_tar(stream, run, out)
    } else {
        read_xml(stream, run, out)
    }
}

/// Read the export file at `path` and hand every anime that has MyList files to `out`, in
/// document order, using constant memory per anime whatever the file size.
///
/// `Ok` is returned only for a complete, valid export: the container and every document are
/// well formed, every limit held, a compressed stream's checksum matched and nothing follows it.
/// Anime already handed to `out` stay handed over when a later error occurs: stage them and
/// apply them only after this returns `Ok`, as [`Db::import_mylist_export`] does. `cancel`
/// is checked between anime (the error is [`Error::Cancelled`]).
pub fn read_export(
    path: &Path,
    limits: &ExportLimits,
    cancel: &AtomicBool,
    out: &mut dyn FnMut(AnimeRecord) -> Result<()>,
) -> Result<Totals> {
    let fault = Fault::default();
    let mut run = xml::Run::new(limits, &fault, cancel);
    let mut file = File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(Error::Other("the export is not a regular file".into()));
    }
    if meta.len() > limits.max_file_bytes {
        return Err(Error::Other(format!(
            "the export file is larger than the {} byte limit",
            limits.max_file_bytes
        )));
    }
    let mut magic = [0u8; 4];
    let got = read_full(&mut file, &mut magic)?;
    file.seek(SeekFrom::Start(0))?;
    let decoded = Rc::new(Cell::new(limits.max_decoded_bytes));
    if got == 4 && &magic == b"PK\x03\x04" {
        read_zip(file, &decoded, &mut run, out)?;
    } else {
        // The file may grow while it is read: its size is bounded again as it is consumed.
        let raw = Capped {
            inner: file,
            budget: Rc::new(Cell::new(limits.max_file_bytes)),
            limit: limits.max_file_bytes,
            what: "the export file",
            fault: &fault,
        };
        if got >= 2 && magic[..2] == [0x1f, 0x8b] {
            let mut gz = flate2::bufread::GzDecoder::new(BufReader::with_capacity(64 << 10, raw));
            let capped = Capped {
                inner: &mut gz,
                budget: decoded,
                limit: limits.max_decoded_bytes,
                what: "the decompressed export",
                fault: &fault,
            };
            read_plain(capped, &mut run, out)?;
            // The single gzip member ended with its checksum; nothing may follow it.
            let rest = gz
                .get_mut()
                .fill_buf()
                .map_err(|e| io_failure(&fault, &e))?;
            if !rest.is_empty() {
                return Err(Error::Other(
                    "unexpected data after the compressed export".into(),
                ));
            }
        } else {
            let capped = Capped {
                inner: raw,
                budget: decoded,
                limit: limits.max_decoded_bytes,
                what: "the export",
                fault: &fault,
            };
            read_plain(capped, &mut run, out)?;
        }
    }
    if run.totals.files == 0 {
        return Err(Error::Other(
            "the export lists no MyList entries (no anime with files)".into(),
        ));
    }
    Ok(run.totals)
}

/// What an import read and what it did to the library.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Anime with MyList files in the export.
    pub anime: usize,
    /// Anime in the export without any file (wishlist rows); they are not imported.
    pub without_files: usize,
    pub episodes: usize,
    /// Distinct files.
    pub files: usize,
    /// Distinct MyList entries the export lists.
    pub entries: usize,
    /// Entries that were not in the library.
    pub added: usize,
    /// Entries whose state, watched flag, date or storage changed.
    pub updated: usize,
    pub unchanged: usize,
    /// Entries left alone because a local add, edit or removal is queued for them.
    pub kept_pending: usize,
    /// Entries the library holds for another file than the export names; left alone.
    pub conflicts: usize,
    /// Library entries the export does not list. They are kept: only an export complete at
    /// the time can say an entry is gone, and this one may be a day old.
    pub not_in_export: usize,
}

impl ImportReport {
    /// One line for the activity log.
    pub fn describe(&self) -> String {
        let mut text = format!(
            "MyList export imported: {} entries in {} anime ({} added, {} updated, {} unchanged)",
            self.entries, self.anime, self.added, self.updated, self.unchanged
        );
        if self.kept_pending > 0 {
            text.push_str(&format!(
                "; {} kept because of queued changes",
                self.kept_pending
            ));
        }
        if self.conflicts > 0 {
            text.push_str(&format!(
                "; {} left alone because they name another file",
                self.conflicts
            ));
        }
        if self.not_in_export > 0 {
            text.push_str(&format!(
                "; {} library entries are not in the export and were kept",
                self.not_in_export
            ));
        }
        if self.without_files > 0 {
            text.push_str(&format!(
                "; {} anime without files were skipped",
                self.without_files
            ));
        }
        text
    }
}

const SPOOL_SCHEMA: &str = "
CREATE TABLE anime(aid INTEGER PRIMARY KEY, year TEXT, name TEXT, type_id INTEGER NOT NULL,
    episodes INTEGER, specials INTEGER, air_date INTEGER, end_date INTEGER);
CREATE TABLE episode(eid INTEGER PRIMARY KEY, aid INTEGER NOT NULL, epno INTEGER NOT NULL,
    kind INTEGER NOT NULL, aired INTEGER, length INTEGER, en TEXT, romaji TEXT, kanji TEXT);
CREATE TABLE grp(gid INTEGER PRIMARY KEY, name TEXT);
CREATE TABLE file(fid INTEGER PRIMARY KEY, aid INTEGER NOT NULL, eid INTEGER NOT NULL,
    gid INTEGER NOT NULL, size INTEGER NOT NULL, ed2k TEXT, md5 TEXT, sha1 TEXT, crc TEXT,
    tth TEXT, filetype TEXT);
CREATE TABLE file_ep(fid INTEGER NOT NULL, eid INTEGER NOT NULL, PRIMARY KEY (fid, eid))
    WITHOUT ROWID;
CREATE TABLE ml(lid INTEGER PRIMARY KEY, fid INTEGER NOT NULL, aid INTEGER NOT NULL,
    eid INTEGER NOT NULL, gid INTEGER NOT NULL, date INTEGER, state INTEGER, viewed INTEGER,
    viewdate INTEGER, storage TEXT);
BEGIN;";

/// The private temporary database a validated export is staged in (deleted when its connection
/// closes, also on errors and panics).
struct Spool {
    conn: Connection,
}

fn unique<T>(result: rusqlite::Result<T>, what: impl FnOnce() -> String) -> Result<T> {
    match result {
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == ErrorCode::ConstraintViolation => {
            Err(Error::Xml(what()))
        }
        other => Ok(other?),
    }
}

impl Spool {
    fn new() -> Result<Spool> {
        // An empty file name is SQLite's private temporary on-disk database.
        let conn = Connection::open("")?;
        conn.pragma_update(None, "journal_mode", "OFF")?;
        conn.pragma_update(None, "synchronous", "OFF")?;
        conn.pragma_update(None, "cache_size", -8192)?;
        conn.execute_batch(SPOOL_SCHEMA)?;
        Ok(Spool { conn })
    }

    fn add(&self, a: &AnimeRecord) -> Result<()> {
        let c = &self.conn;
        c.prepare_cached(
            "INSERT INTO anime(aid, year, name, type_id, episodes, specials, air_date, end_date)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?
        .execute(params![
            a.aid,
            a.year,
            a.name,
            a.type_name.as_deref().map(anime_type_id).unwrap_or(1),
            a.episodes,
            a.specials,
            a.start,
            a.end
        ])?;
        for ep in &a.episode_list {
            let inserted = c
                .prepare_cached(
                    "INSERT INTO episode(eid, aid, epno, kind, aired, length, en, romaji, kanji)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )?
                .execute(params![
                    ep.eid,
                    a.aid,
                    ep.epno.number,
                    ep.epno.kind.max(1),
                    ep.aired,
                    ep.length,
                    ep.english,
                    ep.romaji,
                    ep.kanji
                ]);
            unique(inserted, || format!("episode {} appears twice", ep.eid))?;
            for f in &ep.files {
                self.add_file(a.aid, ep.eid, f)?;
            }
        }
        Ok(())
    }

    fn add_file(&self, aid: i64, eid: i64, f: &FileRecord) -> Result<()> {
        let c = &self.conn;
        let inserted = c
            .prepare_cached(
                "INSERT OR IGNORE INTO file(fid, aid, eid, gid, size, ed2k, md5, sha1, crc, tth, filetype)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?
            .execute(params![
                f.fid, aid, eid, f.gid, f.size, f.ed2k, f.md5, f.sha1, f.crc, f.tth, f.filetype
            ])?;
        if inserted == 0 {
            // A file of several episodes is listed under each of them; it must be the same file.
            let same: bool = c.query_row(
                "SELECT aid = ?2 AND size = ?3 AND COALESCE(ed2k, '') = ?4 AND gid = ?5
                 FROM file WHERE fid = ?1",
                params![
                    f.fid,
                    aid,
                    f.size,
                    f.ed2k.as_deref().unwrap_or_default(),
                    f.gid
                ],
                |r| r.get(0),
            )?;
            if !same {
                return Err(Error::Xml(format!(
                    "file {} appears with different data",
                    f.fid
                )));
            }
            c.prepare_cached("INSERT OR IGNORE INTO file_ep(fid, eid) VALUES (?1, ?2)")?
                .execute(params![f.fid, eid])?;
        }
        if f.gid > 0 {
            c.prepare_cached(
                "INSERT INTO grp(gid, name) VALUES (?1, ?2)
                 ON CONFLICT(gid) DO UPDATE SET name = COALESCE(name, excluded.name)",
            )?
            .execute(params![f.gid, f.group_name])?;
        }
        let entry = c
            .prepare_cached(
                "INSERT OR IGNORE INTO ml(lid, fid, aid, eid, gid, date, state, viewed, viewdate, storage)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?
            .execute(params![
                f.lid, f.fid, aid, eid, f.gid, f.added, f.state, f.viewed, f.viewdate, f.storage
            ])?;
        if entry == 0 {
            let fid: i64 =
                c.query_row("SELECT fid FROM ml WHERE lid = ?1", [f.lid], |r| r.get(0))?;
            if fid != f.fid {
                return Err(Error::Xml(format!(
                    "MyList entry {} names two different files",
                    f.lid
                )));
            }
        }
        Ok(())
    }

    fn count(&self, table: &str) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
        Ok(usize::try_from(n).unwrap_or(0))
    }
}

/// Rows between two checks of the cancel flag during the merge.
const MERGE_CHECK_ROWS: usize = 512;

fn check_cancel(cancel: &AtomicBool, rows: usize) -> Result<()> {
    if rows.is_multiple_of(MERGE_CHECK_ROWS) && cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    Ok(())
}

/// Merge the staged export into the library. Runs inside one write transaction.
fn merge(db: &Db, spool: &Spool, totals: Totals, cancel: &AtomicBool) -> Result<ImportReport> {
    let c = db.conn();
    let s = &spool.conn;
    let mut report = ImportReport {
        anime: totals.anime,
        without_files: totals.without_files,
        episodes: spool.count("episode")?,
        files: spool.count("file")?,
        entries: spool.count("ml")?,
        ..Default::default()
    };

    let mut rows_done = 0usize;
    {
        let mut st = s.prepare(
            "SELECT aid, year, name, type_id, episodes, specials, air_date, end_date
             FROM anime ORDER BY aid",
        )?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            check_cancel(cancel, rows_done)?;
            rows_done += 1;
            let aid: i64 = r.get(0)?;
            let name: Option<String> = r.get(2)?;
            c.execute("INSERT OR IGNORE INTO anime(aid) VALUES (?1)", [aid])?;
            c.execute(
                "UPDATE anime SET
                   year = CASE WHEN (year IS NULL OR year = '') AND ?2 IS NOT NULL THEN ?2 ELSE year END,
                   type_id = CASE WHEN type_id <= 1 AND ?3 > 1 THEN ?3 ELSE type_id END,
                   episodes = CASE WHEN episodes = 0 AND ?4 > 0 THEN ?4 ELSE episodes END,
                   special_eps = CASE WHEN special_eps = 0 AND ?5 > 0 THEN ?5 ELSE special_eps END,
                   air_date = CASE WHEN air_date = 0 AND ?6 > 0 THEN ?6 ELSE air_date END,
                   end_date = CASE WHEN end_date = 0 AND ?7 > 0 THEN ?7 ELSE end_date END
                 WHERE aid = ?1",
                params![
                    aid,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                    r.get::<_, Option<i64>>(7)?
                ],
            )?;
            if let Some(name) = name {
                c.execute(
                    "INSERT INTO anime_title(aid, name, type, lang)
                     SELECT ?1, ?2, 1, 'x-jat'
                     WHERE NOT EXISTS (SELECT 1 FROM anime_title WHERE aid = ?1)",
                    params![aid, name],
                )?;
            }
        }
    }

    {
        let mut st = s.prepare(
            "SELECT eid, aid, epno, kind, aired, length, en, romaji, kanji FROM episode ORDER BY eid",
        )?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            check_cancel(cancel, rows_done)?;
            rows_done += 1;
            let eid: i64 = r.get(0)?;
            c.execute(
                "INSERT INTO episode(eid, aid, epno, type, aired, length)
                 VALUES (?1, ?2, ?3, ?4, COALESCE(?5, 0), COALESCE(?6, 0))
                 ON CONFLICT(eid) DO UPDATE SET
                   aid = CASE WHEN aid = 0 THEN excluded.aid ELSE aid END,
                   epno = CASE WHEN epno = 0 AND excluded.epno > 0 THEN excluded.epno ELSE epno END,
                   aired = CASE WHEN aired = 0 THEN excluded.aired ELSE aired END,
                   length = CASE WHEN length = 0 THEN excluded.length ELSE length END",
                params![
                    eid,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<i64>>(5)?
                ],
            )?;
            for (column, lang) in [(6, "en"), (7, "x-jat"), (8, "ja")] {
                if let Some(title) = r.get::<_, Option<String>>(column)? {
                    c.execute(
                        "INSERT INTO episode_title(eid, name, lang)
                         SELECT ?1, ?2, ?3
                         WHERE NOT EXISTS (SELECT 1 FROM episode_title WHERE eid = ?1 AND lang = ?3)",
                        params![eid, title, lang],
                    )?;
                }
            }
        }
    }

    {
        let mut st = s.prepare("SELECT gid, name FROM grp ORDER BY gid")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            c.execute(
                "INSERT INTO grp(gid, name) VALUES (?1, ?2)
                 ON CONFLICT(gid) DO UPDATE SET
                   name = CASE WHEN name IS NULL OR name = '' THEN excluded.name ELSE name END",
                params![r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?],
            )?;
        }
    }

    {
        let mut st = s.prepare(
            "SELECT fid, aid, eid, gid, size, ed2k, md5, sha1, crc, tth, filetype
             FROM file ORDER BY fid",
        )?;
        let mut extra = s.prepare("SELECT eid FROM file_ep WHERE fid = ?1 ORDER BY eid")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            check_cancel(cancel, rows_done)?;
            rows_done += 1;
            let fid: i64 = r.get(0)?;
            let existed: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM file WHERE fid = ?1)",
                [fid],
                |r| r.get(0),
            )?;
            c.execute(
                "INSERT INTO file(fid, aid, eid, gid, size, ed2k, md5, sha1, crc32, tth, filetype)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(fid) DO UPDATE SET
                   aid = CASE WHEN aid = 0 THEN excluded.aid ELSE aid END,
                   eid = CASE WHEN eid = 0 THEN excluded.eid ELSE eid END,
                   gid = CASE WHEN gid = 0 THEN excluded.gid ELSE gid END,
                   size = CASE WHEN size = 0 THEN excluded.size ELSE size END,
                   ed2k = CASE WHEN ed2k IS NULL OR ed2k = '' THEN excluded.ed2k ELSE ed2k END,
                   md5 = CASE WHEN md5 IS NULL OR md5 = '' THEN excluded.md5 ELSE md5 END,
                   sha1 = CASE WHEN sha1 IS NULL OR sha1 = '' THEN excluded.sha1 ELSE sha1 END,
                   crc32 = CASE WHEN crc32 IS NULL OR crc32 = '' THEN excluded.crc32 ELSE crc32 END,
                   tth = CASE WHEN tth IS NULL OR tth = '' THEN excluded.tth ELSE tth END,
                   filetype = CASE WHEN filetype IS NULL OR filetype = '' THEN excluded.filetype ELSE filetype END",
                params![
                    fid,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, Option<String>>(9)?,
                    r.get::<_, Option<String>>(10)?
                ],
            )?;
            if !existed {
                // The other episodes of a file that covers several; a known file keeps what
                // AniDB told about it.
                let mut eids = extra.query([fid])?;
                while let Some(e) = eids.next()? {
                    c.execute(
                        "INSERT INTO file_episode(fid, eid, start_pct, end_pct) VALUES (?1, ?2, 0, 100)",
                        params![fid, e.get::<_, i64>(0)?],
                    )?;
                }
            }
        }
    }

    {
        let mut st = s.prepare(
            "SELECT lid, fid, aid, eid, gid, date, state, viewed, viewdate, storage
             FROM ml ORDER BY lid",
        )?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            check_cancel(cancel, rows_done)?;
            rows_done += 1;
            let lid: i64 = r.get(0)?;
            if db.mylist_has_pending_intent(lid)? {
                report.kept_pending += 1;
                continue;
            }
            let fid: i64 = r.get(1)?;
            let (aid, eid, gid): (i64, i64, i64) = (r.get(2)?, r.get(3)?, r.get(4)?);
            let date: Option<i64> = r.get(5)?;
            let state: Option<i64> = r.get(6)?;
            let viewed: Option<bool> = r.get(7)?;
            let viewdate: Option<i64> = r.get(8)?;
            let storage: Option<String> = r.get(9)?;
            let Some(local) = db.mylist_entry(lid)? else {
                let viewed = viewed.unwrap_or(false);
                db.upsert_mylist(&MylistEntry {
                    lid,
                    fid,
                    eid,
                    aid,
                    gid,
                    date: date.unwrap_or(0),
                    state: state.unwrap_or(0),
                    viewed,
                    viewdate: if viewed { viewdate.unwrap_or(0) } else { 0 },
                    storage,
                    ..Default::default()
                })?;
                report.added += 1;
                continue;
            };
            if local.fid != fid {
                report.conflicts += 1;
                continue;
            }
            let mut entry = local.clone();
            if entry.eid == 0 {
                entry.eid = eid;
            }
            if entry.aid == 0 {
                entry.aid = aid;
            }
            if entry.gid == 0 {
                entry.gid = gid;
            }
            if let Some(date) = date.filter(|d| *d > 0) {
                entry.date = date;
            }
            if let Some(state) = state {
                entry.state = state;
            }
            match viewed {
                Some(false) => {
                    entry.viewed = false;
                    entry.viewdate = 0;
                }
                Some(true) => {
                    entry.viewed = true;
                    if let Some(date) = viewdate.filter(|d| *d > 0) {
                        entry.viewdate = date;
                    }
                }
                None => {}
            }
            if let Some(storage) = storage.filter(|s| !s.is_empty()) {
                entry.storage = Some(storage);
            }
            if entry == local {
                report.unchanged += 1;
            } else {
                db.upsert_mylist(&entry)?;
                report.updated += 1;
            }
        }
    }

    let mut locals = c.prepare("SELECT lid FROM mylist ORDER BY lid")?;
    let mut listed = s.prepare("SELECT 1 FROM ml WHERE lid = ?1")?;
    let mut rows = locals.query([])?;
    while let Some(r) = rows.next()? {
        check_cancel(cancel, rows_done)?;
        rows_done += 1;
        if !listed.exists([r.get::<_, i64>(0)?])? {
            report.not_in_export += 1;
        }
    }
    Ok(report)
}

impl Db {
    /// Import the MyList export at `path` (see the module documentation for the formats and the
    /// merge rules). The file is validated completely and staged outside the library; the
    /// library is written once, in one transaction, so any error or cancellation leaves it
    /// exactly as it was. Cancelling gives [`Error::Cancelled`].
    pub fn import_mylist_export(
        &self,
        path: &Path,
        limits: &ExportLimits,
        cancel: &AtomicBool,
    ) -> Result<ImportReport> {
        let spool = Spool::new()?;
        let totals = read_export(path, limits, cancel, &mut |anime| spool.add(&anime))?;
        self.write_tx(|| merge(self, &spool, totals, cancel))
    }

    /// `None` when a MyList export may be requested at `now`, otherwise the epoch second from
    /// which it may be ([`crate::limits::MYLIST_EXPORT_MIN_INTERVAL`] after the last attempt).
    pub fn mylist_export_next_allowed(&self, now: i64) -> Result<Option<i64>> {
        self.request_next_allowed(RequestKind::MylistExport, MYLIST_EXPORT_ID, now)
    }
}

#[cfg(test)]
mod tests;
