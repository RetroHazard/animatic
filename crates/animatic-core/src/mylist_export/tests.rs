//! Behavior of the MyList export request and import, with synthetic exports built in code and
//! loopback UDP only. Nothing here contacts AniDB.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use crate::format;

use animatic_anidb::commands::MylistEdit;
use animatic_anidb::http::HttpClient;
use animatic_anidb::limits::Protocol;
use animatic_anidb::testing::{TestLimits, UdpFixture, test_limits};
use animatic_anidb::udp::{Config, UdpClient};

use super::*;
use crate::sync::{Command, Outcome, execute};

const ED2K_1: &str = "00112233445566778899aabbccddeeff";
const ED2K_2: &str = "ffeeddccbbaa99887766554433221100";
const ED2K_4: &str = "0123456789abcdef0123456789abcdef";

fn ts(y: i32, m: u32, d: u32, h: u32, min: u32) -> i64 {
    chrono::NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(h, min, 0)
        .unwrap()
        .and_utc()
        .timestamp()
}

/// An export with three anime: one with a watched file, an unwatched file, a file that covers
/// two episodes and a special; one with a generic file of size 0; one wishlist anime. It also
/// carries elements the importer does not read.
fn export_xml() -> String {
    let kanji = "\u{30c6}\u{30b9}\u{30c8}";
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- synthetic export -->
<mylist>
 <info><user>animatic-test-user</user></info>
 <anime>
  <id>1001</id>
  <name>Synthetic Show One</name>
  <year>2001</year>
  <type id="2">TV Series</type>
  <rating>9.00</rating>
  <count eps="2" specials="1"/>
  <startdate><long>05.04.2001</long></startdate>
  <enddate><long>20.09.2001</long></enddate>
  <mylist_entry><count eps="2" specials="1" total="3"/></mylist_entry>
  <episodes>
   <episode id="5001" number="1">
    <aired>05.04.2001 00:00</aired>
    <length>24</length>
    <name>Synthetic Pilot</name>
    <name_romaji>Shinseshisu Pairotto</name_romaji>
    <name_kanji>{kanji}</name_kanji>
    <files>
     <file id="7001" lid="9001">
      <size_plain>1000</size_plain>
      <group id="301">Synthetic Group</group>
      <source>www</source>
      <filetype>mkv</filetype>
      <hash_crc>DEADBEEF</hash_crc>
      <hash_md5>00000000000000000000000000000001</hash_md5>
      <hash_sha1>0000000000000000000000000000000000000001</hash_sha1>
      <hash_ed2k><key>{ED2K_1}</key></hash_ed2k>
      <mystate id="1">on HDD</mystate>
      <state><iswatched>1</iswatched></state>
      <viewdate><long>01.02.2020 20:15</long></viewdate>
      <mydate><long>03.01.2020 10:00</long></mydate>
      <storage>Shelf A &amp; B</storage>
     </file>
    </files>
   </episode>
   <episode id="5002" number="2">
    <aired>12.04.2001 00:00</aired>
    <length>24</length>
    <name>Synthetic Second</name>
    <files>
     <file id="7002" lid="9002">
      <size_plain>2000</size_plain>
      <group id="0"/>
      <filetype>avi</filetype>
      <hash_ed2k><key>{ED2K_2}</key></hash_ed2k>
      <mystate id="2"/>
      <state><iswatched>0</iswatched></state>
      <viewdate><long>-</long></viewdate>
      <storage></storage>
     </file>
     <file id="7004" lid="9004">
      <size_plain>4000</size_plain>
      <group id="301">Synthetic Group</group>
      <hash_ed2k><key>{ED2K_4}</key></hash_ed2k>
      <mystate id="1"/>
      <viewdate><long>-</long></viewdate>
     </file>
    </files>
   </episode>
   <episode id="5003" number="S1">
    <aired>??.??.2001</aired>
    <length></length>
    <name>Synthetic Special</name>
    <files>
     <file id="7004" lid="9004">
      <size_plain>4000</size_plain>
      <group id="301">Synthetic Group</group>
      <hash_ed2k><key>{ED2K_4}</key></hash_ed2k>
      <mystate id="1"/>
      <viewdate><long>-</long></viewdate>
     </file>
    </files>
   </episode>
  </episodes>
 </anime>
 <anime>
  <id>1002</id>
  <name>Synthetic Show Two</name>
  <episodes>
   <episode id="5101" number="1">
    <files>
     <file id="7101" lid="9101">
      <size_plain>0</size_plain>
      <group id="0"/>
      <mystate id="3"/>
      <state><iswatched>1</iswatched></state>
     </file>
    </files>
   </episode>
  </episodes>
 </anime>
 <anime>
  <id>1003</id>
  <name>Synthetic Wishlist</name>
  <episodes/>
 </anime>
</mylist>
"#
    )
}

struct Scratch {
    dir: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Self {
        Scratch {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(bytes).unwrap();
    gz.finish().unwrap()
}

fn tar(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        builder.append_data(&mut header, name, *data).unwrap();
    }
    builder.into_inner().unwrap()
}

fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn import(db: &Db, path: &Path) -> Result<ImportReport> {
    db.import_mylist_export(path, &ExportLimits::default(), &AtomicBool::new(false))
}

/// Every table a library import can touch, as text, in a stable order.
fn dump(db: &Db) -> Vec<String> {
    let mut out = Vec::new();
    for table in [
        "anime",
        "anime_title",
        "episode",
        "episode_title",
        "grp",
        "file",
        "file_episode",
        "mylist",
        "meta",
        "command_queue",
        "request_attempt",
    ] {
        let mut st = db
            .conn()
            .prepare(&format!("SELECT * FROM {table} ORDER BY 1, 2"))
            .unwrap();
        let columns = st.column_count();
        let mut rows = st.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let cells: Vec<String> = (0..columns)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap()))
                .collect();
            out.push(format!("{table}: {}", cells.join(" | ")));
        }
    }
    out
}

#[test]
fn an_export_into_an_empty_library_reproduces_the_mylist() {
    let db = Db::memory().unwrap();
    let scratch = Scratch::new();
    let path = scratch.file("export.xml", export_xml().as_bytes());
    let report = import(&db, &path).unwrap();
    assert_eq!(
        report,
        ImportReport {
            anime: 2,
            without_files: 1,
            episodes: 4,
            files: 4,
            entries: 4,
            added: 4,
            ..Default::default()
        }
    );
    assert!(report.describe().contains("4 entries in 2 anime"));

    // The MyList rows, as the MyList view reads them.
    let items = db.mylist_items().unwrap();
    assert_eq!(items.len(), 4);
    let watched = db.mylist_entry(9001).unwrap().unwrap();
    assert_eq!(
        (watched.fid, watched.aid, watched.eid, watched.gid),
        (7001, 1001, 5001, 301)
    );
    assert_eq!(watched.state, 1);
    assert!(watched.viewed);
    assert_eq!(watched.viewdate, ts(2020, 2, 1, 20, 15));
    assert_eq!(watched.date, ts(2020, 1, 3, 10, 0));
    assert_eq!(watched.storage.as_deref(), Some("Shelf A & B"));
    let unwatched = db.mylist_entry(9002).unwrap().unwrap();
    assert!(!unwatched.viewed);
    assert_eq!((unwatched.viewdate, unwatched.state), (0, 2));
    assert_eq!(unwatched.storage, None);
    let generic = db.mylist_entry(9101).unwrap().unwrap();
    assert!(generic.viewed, "watched without a date");
    assert_eq!(generic.viewdate, 0);

    // The anime, episode, file and group data the rows are shown with.
    let anime = db.anime(1001).unwrap().unwrap();
    assert_eq!(anime.year, "2001");
    assert_eq!((anime.episodes, anime.special_eps), (2, 1));
    assert_eq!(anime.air_date, ts(2001, 4, 5, 0, 0));
    assert_eq!(anime.end_date, ts(2001, 9, 20, 0, 0));
    assert_eq!(anime.type_id, 2);
    assert_eq!(anime.name, "Synthetic Show One");
    assert!(
        db.anime(1003).unwrap().is_none(),
        "a wishlist anime has no files and is not imported"
    );
    let episode = db.episode(5001).unwrap().unwrap();
    assert_eq!((episode.length, episode.aired), (24, ts(2001, 4, 5, 0, 0)));
    assert_eq!(db.episode(5003).unwrap().unwrap().epno_str(), "S1");
    assert_eq!(db.episode(5003).unwrap().unwrap().aired, 0, "unknown date");
    assert_eq!(
        db.episode_title_lang(5001, "en").unwrap().as_deref(),
        Some("Synthetic Pilot")
    );
    assert_eq!(
        db.episode_title_lang(5001, "ja").unwrap().as_deref(),
        Some("\u{30c6}\u{30b9}\u{30c8}")
    );
    let file = db.file(7001).unwrap().unwrap();
    assert_eq!((file.size, file.eid, file.gid), (1000, 5001, 301));
    assert_eq!(file.ed2k.as_deref(), Some(ED2K_1));
    assert_eq!(file.crc32.as_deref(), Some("deadbeef"));
    assert_eq!(db.group(301).unwrap().unwrap().name, "Synthetic Group");
    assert!(
        db.file_by_hash(&ED2K_1.to_ascii_uppercase(), 1000)
            .unwrap()
            .is_some(),
        "a local file with this hash finds the imported file"
    );
    // The file of two episodes belongs to both.
    let in_special: Vec<i64> = db
        .files_of_episode(5003)
        .unwrap()
        .iter()
        .map(|f| f.fid)
        .collect();
    assert_eq!(in_special, [7004]);
    assert_eq!(db.mylist_entry(9004).unwrap().unwrap().eid, 5002);
    assert!(db.anime(1002).unwrap().is_some());
    assert_eq!(db.file(7101).unwrap().unwrap().ed2k, None);
}

#[test]
fn a_second_import_changes_nothing() {
    let db = Db::memory().unwrap();
    let scratch = Scratch::new();
    let path = scratch.file("export.xml", export_xml().as_bytes());
    import(&db, &path).unwrap();
    let before = dump(&db);
    assert!(before.len() > 20, "{before:#?}");

    let again = import(&db, &path).unwrap();
    assert_eq!(dump(&db), before);
    assert_eq!((again.added, again.updated), (0, 0));
    assert_eq!(again.unchanged, 4);
}

#[test]
fn every_container_gives_the_same_library() {
    let xml = export_xml();
    let scratch = Scratch::new();
    let expected = {
        let db = Db::memory().unwrap();
        import(&db, &scratch.file("plain.xml", xml.as_bytes())).unwrap();
        dump(&db)
    };
    let mut with_bom = vec![0xef, 0xbb, 0xbf];
    with_bom.extend_from_slice(xml.as_bytes());
    let tar_gz = gzip(&tar(&[
        ("README.txt", b"not a document".as_slice()),
        ("mylist.xml", xml.as_bytes()),
    ]));
    // One document per anime, as the full export template delivers.
    let (first, second) = xml.split_once(" <anime>\n  <id>1002</id>").unwrap();
    let part_one = format!("{first}</mylist>\n");
    let part_two = format!("<mylist>\n <anime>\n  <id>1002</id>{second}");
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("bom.xml", with_bom),
        ("export.xml.gz", gzip(xml.as_bytes())),
        ("export.tgz", tar_gz),
        (
            "export.tar",
            tar(&[
                ("anime/a1.xml", part_one.as_bytes()),
                ("anime/a2.xml", part_two.as_bytes()),
            ]),
        ),
        ("export.zip", zip(&[("mylist.xml", xml.as_bytes())])),
        (
            "no-extension",
            gzip(&tar(&[("mylist.xml", xml.as_bytes())])),
        ),
    ];
    for (name, bytes) in cases {
        let db = Db::memory().unwrap();
        let report =
            import(&db, &scratch.file(name, &bytes)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.entries, 4, "{name}");
        assert_eq!(dump(&db), expected, "{name}");
    }
}

/// A library with one entry of its own, so a refused import has something to leave untouched.
fn library_with_a_local_entry() -> Db {
    let db = Db::memory().unwrap();
    db.upsert_mylist(&MylistEntry {
        lid: 9001,
        fid: 7001,
        eid: 5001,
        aid: 1001,
        gid: 301,
        state: 3,
        storage: Some("Local box".into()),
        ..Default::default()
    })
    .unwrap();
    db.queue_push(&Command::MylistStats).unwrap();
    db
}

fn refused(db: &Db, name: &str, bytes: &[u8], limits: &ExportLimits) -> String {
    let before = dump(db);
    let scratch = Scratch::new();
    let path = scratch.file(name, bytes);
    let error = db
        .import_mylist_export(&path, limits, &AtomicBool::new(false))
        .expect_err(name);
    assert_eq!(dump(db), before, "{name}: the library must stay unchanged");
    error.to_string()
}

#[test]
fn truncated_malformed_and_oversized_files_change_nothing() {
    let db = library_with_a_local_entry();
    let xml = export_xml();
    let limits = ExportLimits::default();
    let cut = |text: &str| xml.replacen(text, "", 1);
    let change = |from: &str, to: &str| {
        assert!(xml.contains(from), "{from}");
        xml.replacen(from, to, 1)
    };

    // Truncation, in the XML and in every container.
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("cut mid-document", xml.as_bytes()[..xml.len() / 2].to_vec()),
        ("cut before the root end", cut("</mylist>\n").into_bytes()),
        ("cut mid-tag", xml.as_bytes()[..xml.len() - 4].to_vec()),
        ("empty file", Vec::new()),
        ("not an export", b"hello, this is not xml".to_vec()),
        ("gzip cut", {
            let all = gzip(xml.as_bytes());
            all[..all.len() - 10].to_vec()
        }),
        ("gzip with a damaged checksum", {
            let mut all = gzip(xml.as_bytes());
            let n = all.len();
            all[n - 8] ^= 0xff;
            all
        }),
        ("gzip followed by data", {
            let mut all = gzip(xml.as_bytes());
            all.extend_from_slice(b"trailing");
            all
        }),
        ("tgz cut", {
            let all = gzip(&tar(&[("mylist.xml", xml.as_bytes())]));
            all[..all.len() / 2].to_vec()
        }),
        ("tar cut", {
            let all = tar(&[("mylist.xml", xml.as_bytes())]);
            all[..all.len() / 2].to_vec()
        }),
        ("zip cut", {
            let all = zip(&[("mylist.xml", xml.as_bytes())]);
            all[..all.len() / 2].to_vec()
        }),
        (
            "archive without a document",
            tar(&[("notes.txt", b"x".as_slice())]),
        ),
        (
            "archive with the same document twice",
            tar(&[("a.xml", xml.as_bytes()), ("a.xml", xml.as_bytes())]),
        ),
        // Structure and values.
        (
            "wrong root",
            change("<mylist>", "<other>")
                .replace("</mylist>", "</other>")
                .into_bytes(),
        ),
        (
            "content after the root",
            format!("{xml}<mylist/>").into_bytes(),
        ),
        (
            "document type",
            format!("<!DOCTYPE mylist>{xml}").into_bytes(),
        ),
        ("not UTF-8", change("UTF-8", "ISO-8859-1").into_bytes()),
        (
            "text in the structure",
            change("<info>", "<info>")
                .replace("<episodes>\n", "<episodes>stray\n")
                .into_bytes(),
        ),
        (
            "anime twice",
            change("<id>1002</id>", "<id>1001</id>").into_bytes(),
        ),
        ("anime without id", change("<id>1002</id>", "").into_bytes()),
        (
            "anime id zero",
            change("<id>1002</id>", "<id>0</id>").into_bytes(),
        ),
        (
            "id appears twice",
            change("<id>1002</id>", "<id>1002</id><id>1002</id>").into_bytes(),
        ),
        (
            "bad episode count",
            change("eps=\"2\" specials", "eps=\"two\" specials").into_bytes(),
        ),
        (
            "bad air date",
            change("05.04.2001 00:00", "yesterday").into_bytes(),
        ),
        (
            "bad view date",
            change("01.02.2020 20:15", "31.02.2020 20:15").into_bytes(),
        ),
        (
            "bad episode number",
            change("number=\"S1\"", "number=\"S\"").into_bytes(),
        ),
        (
            "episode id zero",
            change("id=\"5101\"", "id=\"0\"").into_bytes(),
        ),
        (
            "episode twice",
            change("id=\"5101\"", "id=\"5001\"").into_bytes(),
        ),
        ("file without lid", change(" lid=\"9101\"", "").into_bytes()),
        ("bad hash", change("DEADBEEF", "xyz").into_bytes()),
        ("bad ed2k", change(ED2K_1, "1234").into_bytes()),
        (
            "size without ed2k",
            change(&format!("<hash_ed2k><key>{ED2K_2}</key></hash_ed2k>"), "").into_bytes(),
        ),
        (
            "bad watched flag",
            change("<iswatched>0</iswatched>", "<iswatched>maybe</iswatched>").into_bytes(),
        ),
        (
            "file with other data",
            change(
                "<size_plain>4000</size_plain>",
                "<size_plain>4001</size_plain>",
            )
            .into_bytes(),
        ),
        (
            "entry names two files",
            change("id=\"7002\" lid=\"9002\"", "id=\"7002\" lid=\"9004\"").into_bytes(),
        ),
        (
            "element inside a value",
            change("<length>24</length>", "<length>2<b/>4</length>").into_bytes(),
        ),
        (
            "no entries at all",
            b"<mylist><anime><id>1</id><episodes/></anime></mylist>".to_vec(),
        ),
    ];
    let mut reasons = std::collections::HashMap::new();
    for (name, bytes) in cases {
        let message = refused(&db, name, &bytes, &limits);
        reasons.insert(name, message);
    }
    for (name, fragment) in [
        ("cut mid-document", "truncated"),
        ("empty file", "no <mylist> or <anime> root"),
        ("gzip cut", "damaged or ends too early"),
        ("gzip with a damaged checksum", "checksum"),
        ("tgz cut", "damaged or ends too early"),
        ("zip cut", "zip archive is invalid"),
        ("wrong root", "root element is <other>"),
        ("anime twice", "anime 1001 appears twice"),
        ("file without lid", "missing the lid attribute"),
        ("bad air date", "not a date"),
        ("bad ed2k", "32 hexadecimal digits"),
        ("size without ed2k", "no ED2K hash"),
        ("entry names two files", "names two different files"),
        ("no entries at all", "no MyList entries"),
    ] {
        assert!(
            reasons[name].contains(fragment),
            "{name}: {}",
            reasons[name]
        );
    }

    // Over the limits.
    let tight = |edit: fn(&mut ExportLimits)| {
        let mut l = ExportLimits::default();
        edit(&mut l);
        l
    };
    let bytes = xml.as_bytes();
    let size = bytes.len() as u64;
    for (name, data, limits, wanted) in [
        (
            "file size",
            bytes.to_vec(),
            tight(|l| l.max_file_bytes = 100),
            "larger than",
        ),
        (
            "decoded size",
            gzip(bytes),
            ExportLimits {
                max_decoded_bytes: size / 2,
                ..tight(|_| {})
            },
            "larger than",
        ),
        (
            "decoded size of a plain file",
            bytes.to_vec(),
            ExportLimits {
                max_decoded_bytes: size - 1,
                ..tight(|_| {})
            },
            "larger than",
        ),
        (
            "anime",
            bytes.to_vec(),
            tight(|l| l.max_anime = 2),
            "more than 2 anime",
        ),
        (
            "episodes of an anime",
            bytes.to_vec(),
            tight(|l| l.max_episodes_per_anime = 2),
            "more than 2 episodes",
        ),
        (
            "files of an anime",
            bytes.to_vec(),
            tight(|l| l.max_files_per_anime = 2),
            "more than 2 files",
        ),
        (
            "files",
            bytes.to_vec(),
            tight(|l| l.max_files = 3),
            "more than 3 files",
        ),
        (
            "text length",
            bytes.to_vec(),
            tight(|l| l.max_text_bytes = 10),
            "longer than 10",
        ),
        (
            "tag span",
            bytes.to_vec(),
            tight(|l| l.max_span_bytes = 40),
            "between two tags",
        ),
        (
            "nesting",
            bytes.to_vec(),
            tight(|l| l.max_depth = 4),
            "nested deeper",
        ),
        (
            "archive entries",
            tar(&[("a.xml", bytes), ("b.txt", b"x"), ("c.txt", b"x")]),
            tight(|l| l.max_entries = 2),
            "more than 2 entries",
        ),
        (
            "zip entries",
            zip(&[("a.xml", bytes), ("b.txt", b"x")]),
            tight(|l| l.max_entries = 1),
            "more than 1 entries",
        ),
    ] {
        let message = refused(&db, name, &data, &limits);
        assert!(message.contains(wanted), "{name}: {message}");
    }

    // A decompression bomb: far more decoded than the limit, from a tiny file.
    let huge = {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        gz.write_all(b"<mylist>").unwrap();
        for aid in 1..=2000 {
            gz.write_all(format!("<anime><id>{aid}</id><episodes/></anime>").as_bytes())
                .unwrap();
        }
        gz.finish().unwrap()
    };
    let message = refused(
        &db,
        "bomb.gz",
        &huge,
        &tight(|l| l.max_decoded_bytes = 1000),
    );
    assert!(message.contains("larger than"), "{message}");
}

#[test]
fn a_cancelled_import_changes_nothing() {
    let db = library_with_a_local_entry();
    let scratch = Scratch::new();
    let path = scratch.file("export.xml", export_xml().as_bytes());
    let before = dump(&db);
    let cancel = AtomicBool::new(true);
    let result = db.import_mylist_export(&path, &ExportLimits::default(), &cancel);
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert_eq!(dump(&db), before);
}

#[test]
fn queued_local_changes_win_and_richer_local_data_is_kept() {
    let scratch = Scratch::new();
    let path = scratch.file("export.xml", export_xml().as_bytes());
    let db = Db::memory().unwrap();
    // Entry 9001: a queued edit. Entry 9002: a queued removal. Entry 9004: nothing queued, so
    // the export updates it. Entry 9999 is not in the export and stays.
    for (lid, state) in [(9001, 7), (9002, 7), (9004, 7), (9999, 7)] {
        db.upsert_mylist(&MylistEntry {
            lid,
            fid: match lid {
                9001 => 7001,
                9002 => 7002,
                9004 => 7004,
                _ => 7999,
            },
            state,
            storage: Some("Local box".into()),
            ..Default::default()
        })
        .unwrap();
    }
    db.queue_edit_mylist(
        9001,
        &MylistEdit {
            state: Some(4),
            viewed: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    db.queue_push(&Command::MylistDelete { lid: 9002 }).unwrap();
    // Richer local data of the same anime, file and group.
    db.conn()
        .execute_batch(
            "INSERT INTO anime(aid, year, episodes, fetched) VALUES (1001, '1999-2000', 26, 1700000000);
             INSERT INTO anime_title(aid, name, type, lang) VALUES (1001, 'Local Title', 1, 'x-jat');
             INSERT INTO grp(gid, name, short) VALUES (301, 'Local Group Name', 'LGN');
             INSERT INTO file(fid, aid, eid, gid, size, ed2k, anidb_filename) VALUES (7001, 1001, 5001, 301, 1000, 'AAAA', 'local name.mkv');",
        )
        .unwrap();
    let before_queue = db.queue_list().unwrap();

    let report = import(&db, &path).unwrap();
    assert_eq!(
        (
            report.kept_pending,
            report.updated,
            report.added,
            report.not_in_export
        ),
        (2, 1, 1, 1),
        "{report:?}"
    );
    assert_eq!(
        db.queue_list().unwrap(),
        before_queue,
        "the queue is untouched"
    );

    let edited = db.mylist_entry(9001).unwrap().unwrap();
    assert_eq!(
        edited.state, 4,
        "the queued edit's optimistic state stays; the export does not touch the entry"
    );
    assert_eq!(edited.storage.as_deref(), Some("Local box"));
    let removing = db.mylist_entry(9002).unwrap().unwrap();
    assert_eq!(
        removing.state, 7,
        "an entry queued for removal stays until AniDB confirms"
    );
    let updated = db.mylist_entry(9004).unwrap().unwrap();
    assert_eq!(
        updated.state, 1,
        "the export's state replaces the local one"
    );
    assert_eq!(
        updated.storage.as_deref(),
        Some("Local box"),
        "the export has no storage text for it: the local text stays"
    );
    assert!(
        db.mylist_entry(9999).unwrap().is_some(),
        "absence from the export deletes nothing"
    );

    let anime = db.anime(1001).unwrap().unwrap();
    assert_eq!(anime.year, "1999-2000");
    assert_eq!(anime.episodes, 26);
    assert_eq!(anime.name, "Local Title");
    assert_eq!(db.group(301).unwrap().unwrap().name, "Local Group Name");
    let file = db.file(7001).unwrap().unwrap();
    assert_eq!(
        file.ed2k.as_deref(),
        Some("AAAA"),
        "a known file keeps its data"
    );
    assert_eq!(file.crc32.as_deref(), Some("deadbeef"), "blanks are filled");
}

#[test]
fn an_entry_that_names_another_file_is_left_alone() {
    let scratch = Scratch::new();
    let path = scratch.file("export.xml", export_xml().as_bytes());
    let db = Db::memory().unwrap();
    db.upsert_mylist(&MylistEntry {
        lid: 9001,
        fid: 1,
        state: 7,
        ..Default::default()
    })
    .unwrap();
    let report = import(&db, &path).unwrap();
    assert_eq!((report.conflicts, report.added), (1, 3));
    assert_eq!(db.mylist_entry(9001).unwrap().unwrap().fid, 1);
}

// ---- Requesting an export: UDP, through the shared limiter ----

fn udp_client(server: std::net::SocketAddr, t: &TestLimits) -> UdpClient {
    UdpClient::new(
        Config {
            server: server.to_string(),
            client: "syntheticclient".into(),
            username: "synthetic-user".into(),
            password: "synthetic-password".into(),
            timeout: std::time::Duration::from_secs(5),
            ..Config::default()
        },
        t.limits.clone(),
    )
}

fn export_server(answer: &'static str) -> UdpFixture {
    UdpFixture::start(move |_, request| {
        if request.starts_with("AUTH") {
            "200 session 127.0.0.1:1 LOGIN ACCEPTED".into()
        } else if request.starts_with("MYLISTEXPORT") {
            answer.into()
        } else {
            "203 LOGGED OUT".into()
        }
    })
}

fn run(
    cmd: &Command,
    db: &Db,
    server: &UdpFixture,
    t: &TestLimits,
) -> std::result::Result<Outcome, crate::sync::CommandError> {
    execute(
        1,
        cmd,
        db,
        &mut udp_client(server.addr, t),
        &mut HttpClient::new("syntheticclient", 1, t.limits.clone()),
    )
}

#[test]
fn a_queued_export_is_reported_and_not_requested_again_inside_a_day() {
    let db = Db::memory().unwrap();
    let server = UdpFixture::start(|_, request| {
        if request.starts_with("AUTH") {
            "200 session 127.0.0.1:1 LOGIN ACCEPTED".into()
        } else {
            assert!(
                request.starts_with("MYLISTEXPORT template=xml-plain-new"),
                "{request}"
            );
            "217 EXPORT QUEUED".into()
        }
    });
    let t = test_limits();
    let outcome = run(&Command::MylistExport, &db, &server, &t).unwrap();
    let Outcome::Updated(text) = &outcome else {
        panic!("{outcome:?}")
    };
    assert!(text.contains("24 hours"), "{text}");
    assert_eq!(server.packets(), 2, "login and one request");
    assert_eq!(
        t.store.sends(Protocol::Udp).len(),
        2,
        "both packets took their slot from the shared limiter"
    );
    assert!(
        db.mylist_export_next_allowed(format::now())
            .unwrap()
            .is_some()
    );

    let again = run(&Command::MylistExport, &db, &server, &t).unwrap();
    let Outcome::Skipped(text) = &again else {
        panic!("{again:?}")
    };
    assert!(text.contains("allowed after"), "{text}");
    assert_eq!(server.packets(), 2, "no packet inside the day");

    // A day later a new request goes out.
    let day = crate::limits::MYLIST_EXPORT_MIN_INTERVAL.as_secs() as i64;
    db.record_request_attempt(
        RequestKind::MylistExport,
        MYLIST_EXPORT_ID,
        format::now() - day - 1,
    )
    .unwrap();
    assert!(matches!(
        run(&Command::MylistExport, &db, &server, &t),
        Ok(Outcome::Updated(_))
    ));
    assert_eq!(server.packets(), 4, "a new client logs in again");
}

#[test]
fn an_export_that_is_already_queued_is_reported_and_neither_fails_nor_retries() {
    let db = Db::memory().unwrap();
    let server = export_server("318 EXPORT ALREADY IN QUEUE");
    let t = test_limits();
    let outcome = run(&Command::MylistExport, &db, &server, &t).unwrap();
    let Outcome::Skipped(text) = &outcome else {
        panic!("{outcome:?}")
    };
    assert!(text.contains("already queued"), "{text}");
    assert!(!outcome.changes_library());
    assert_eq!(server.packets(), 2);
}

#[test]
fn an_unknown_template_is_a_permanent_error_that_still_counts_as_an_attempt() {
    let db = Db::memory().unwrap();
    let server = export_server("317 EXPORT NO SUCH TEMPLATE");
    let t = test_limits();
    let error = run(&Command::MylistExport, &db, &server, &t).unwrap_err();
    assert!(error.message.contains("xml-plain-new"), "{}", error.message);
    assert!(
        !error.transient && !error.fatal && error.retry_after.is_none(),
        "permanent: the queue entry is blocked, not retried and the queue goes on: {error:?}"
    );
    assert!(
        db.mylist_export_next_allowed(format::now())
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_failed_login_sends_no_export_request_and_is_no_attempt() {
    let db = Db::memory().unwrap();
    let server = UdpFixture::start(|_, _| "500 LOGIN FAILED".into());
    let t = test_limits();
    assert!(
        run(&Command::MylistExport, &db, &server, &t)
            .unwrap_err()
            .fatal
    );
    assert_eq!(server.packets(), 1);
    assert_eq!(db.mylist_export_next_allowed(format::now()).unwrap(), None);
}

#[test]
fn the_queued_export_can_be_cancelled() {
    let db = Db::memory().unwrap();
    let t = test_limits();
    let server = export_server("218 EXPORT CANCELLED");
    let cancelled = run(&Command::MylistExportCancel, &db, &server, &t).unwrap();
    assert!(matches!(cancelled, Outcome::Updated(_)), "{cancelled:?}");
    let server = export_server("319 EXPORT NO EXPORT QUEUED OR IS PROCESSING");
    let nothing = run(&Command::MylistExportCancel, &db, &server, &t).unwrap();
    assert!(matches!(nothing, Outcome::Skipped(_)), "{nothing:?}");
    let server = export_server("500 SURPRISE");
    assert!(run(&Command::MylistExportCancel, &db, &server, &t).is_err());
}

#[test]
fn export_commands_are_user_requests_that_a_full_update_does_not_queue() {
    let db = Db::memory().unwrap();
    assert!(db.queue_push(&Command::MylistExport).unwrap() > 0);
    assert_eq!(
        db.queue_push(&Command::MylistExport).unwrap(),
        0,
        "one at a time"
    );
    assert!(db.queue_push(&Command::MylistExportCancel).unwrap() > 0);
    let stored: Vec<_> = db
        .queue_list()
        .unwrap()
        .into_iter()
        .map(|e| e.command)
        .collect();
    assert_eq!(
        stored,
        [
            Some(Command::MylistExport),
            Some(Command::MylistExportCancel)
        ]
    );
    assert!(!Command::MylistExport.uses_http());
}
