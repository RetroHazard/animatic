//! Import of a complete legacy `kowai` databank into the new database.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use super::blob;
use super::crypto;
use crate::db::Db;
use crate::error::{Error, Result};

/// Legacy collection files in the order they are imported (lookup tables first).
pub const LEGACY_FILES: &[&str] = &[
    "catcat",
    "catlang",
    "catqual",
    "cattype",
    "catvcodec",
    "catacodec",
    "catres",
    "catsub",
    "genres",
    "groups",
    "animes",
    "animetitles",
    "animegenres",
    "animerelations",
    "episodes",
    "episodetitles",
    "files",
    "fileepisoderelations",
    "filerelations",
    "reviews",
    "mylist",
    "local_hashes",
    "local_files",
];

#[derive(Debug, Default, Clone)]
pub struct ImportReport {
    pub imported: Vec<(String, usize)>,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
}

impl ImportReport {
    pub fn count(&self, name: &str) -> usize {
        self.imported
            .iter()
            .find(|(n, _)| n == name)
            .map_or(0, |(_, c)| *c)
    }
}

/// Progress callback: (stage label, fraction 0..1 of the whole import). Return `false` to cancel.
pub type Progress<'a> = &'a mut dyn FnMut(&str, f32) -> bool;

/// Returns true when `dir` looks like a legacy databank folder.
pub fn is_databank(dir: &Path) -> bool {
    dir.join("mylist.data").is_file()
        || dir.join("local_files.data").is_file()
        || dir.join("animes.data").is_file()
}

/// Locate the databank folder from either the AOM install dir or the kowai dir itself.
pub fn find_databank(path: &Path) -> Option<PathBuf> {
    if is_databank(path) {
        return Some(path.to_path_buf());
    }
    let kowai = path.join("kowai");
    is_databank(&kowai).then_some(kowai)
}

fn filetime_to_unix(ft: i64) -> i64 {
    if ft <= 0 {
        return 0;
    }
    ft / 10_000_000 - 11_644_473_600
}

struct Ctx<'a> {
    conn: &'a Connection,
    report: ImportReport,
}

pub fn import_databank(
    db: &mut Db,
    kowai: &Path,
    temp_dir: &Path,
    progress: Progress,
) -> Result<ImportReport> {
    std::fs::create_dir_all(temp_dir)?;
    let present: Vec<&str> = LEGACY_FILES
        .iter()
        .copied()
        .filter(|n| kowai.join(format!("{n}.data")).is_file())
        .collect();
    if present.is_empty() {
        return Err(Error::Legacy(format!(
            "no legacy databank files found in {}",
            kowai.display()
        )));
    }
    let weights: Vec<u64> = present
        .iter()
        .map(|n| std::fs::metadata(kowai.join(format!("{n}.data"))).map_or(1, |m| m.len().max(1)))
        .collect();
    let total: u64 = weights.iter().sum();
    let mut done: u64 = 0;

    let conn = db.conn_mut();
    conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    let mut ctx = Ctx {
        conn,
        report: ImportReport::default(),
    };

    for (name, weight) in present.iter().zip(&weights) {
        let src = kowai.join(format!("{name}.data"));
        let plain = temp_dir.join(format!("{name}.sqlite"));
        let base = done as f32 / total as f32;
        let span = *weight as f32 / total as f32;
        let label_dec = format!("Decrypting {name}.data");
        let mut cancelled = false;
        crypto::decrypt_file(&src, &plain, |d, t| {
            if !progress(&label_dec, base + span * 0.4 * (d as f32 / t.max(1) as f32)) {
                cancelled = true;
            }
        })?;
        if cancelled {
            let _ = std::fs::remove_file(&plain);
            return Err(Error::Cancelled);
        }
        if !progress(&format!("Importing {name}"), base + span * 0.4) {
            let _ = std::fs::remove_file(&plain);
            return Err(Error::Cancelled);
        }
        ctx.conn
            .execute("ATTACH DATABASE ?1 AS legacy", [plain.to_string_lossy()])?;
        let result = ctx
            .conn
            .execute_batch("BEGIN")
            .map_err(Error::from)
            .and_then(|_| import_one(&mut ctx, name));
        match result {
            Ok(n) => {
                ctx.conn.execute_batch("COMMIT")?;
                ctx.report.imported.push((name.to_string(), n));
            }
            Err(e) => {
                let _ = ctx.conn.execute_batch("ROLLBACK");
                ctx.report.warnings.push(format!("{name}: {e}"));
                ctx.report.skipped.push(name.to_string());
            }
        }
        ctx.conn.execute_batch("DETACH DATABASE legacy")?;
        let _ = std::fs::remove_file(&plain);
        done += weight;
        progress(&format!("Imported {name}"), done as f32 / total as f32);
    }

    progress("Computing derived data", 0.99);
    add_missing_episode_names(ctx.conn)?;
    ctx.conn.execute_batch(
        "BEGIN;
         UPDATE anime SET
           highest_ep = COALESCE((SELECT max(epno) FROM episode e WHERE e.aid = anime.aid AND e.type = 1), 0),
           special_eps = COALESCE((SELECT count(*) FROM episode e WHERE e.aid = anime.aid AND e.type = 2), 0);
         UPDATE mylist SET gid = COALESCE((SELECT gid FROM file f WHERE f.fid = mylist.fid), 0) WHERE gid = 0;
         COMMIT;",
    )?;
    let report = ctx.report;
    db.set_meta("legacy_import_source", &kowai.to_string_lossy())?;
    db.set_meta(
        "legacy_import_date",
        &chrono::Utc::now().timestamp().to_string(),
    )?;
    if let Some(tag) = db.meta("legacy_dump_date_tmp")? {
        db.set_meta("legacy_dump_date", &tag)?;
        db.conn()
            .execute("DELETE FROM meta WHERE key = 'legacy_dump_date_tmp'", [])?;
    }
    let _ = std::fs::remove_dir(temp_dir);
    progress("Import complete", 1.0);
    Ok(report)
}
fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT count(*) FROM legacy.sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

fn for_each_blob(
    conn: &Connection,
    mut f: impl FnMut(i64, blob::Record) -> Result<()>,
) -> Result<usize> {
    if !table_exists(conn, "dataTable")? {
        return Ok(0);
    }
    let mut stmt = conn.prepare("SELECT id, data FROM legacy.dataTable ORDER BY id")?;
    let mut rows = stmt.query([])?;
    let mut n = 0;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let data: Vec<u8> = row.get(1)?;
        match blob::parse(&data) {
            Ok(rec) => {
                f(id, rec)?;
                n += 1;
            }
            Err(e) => log::warn!("legacy record {id} unreadable: {e}"),
        }
    }
    Ok(n)
}

fn remember_dump_date(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "dateTable")? {
        return Ok(());
    }
    let tag: Option<i64> = conn
        .query_row("SELECT tag FROM legacy.dateTable WHERE id = 1", [], |r| {
            r.get(0)
        })
        .ok();
    if let Some(tag) = tag {
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('legacy_dump_date_tmp', ?1)
             ON CONFLICT(key) DO UPDATE SET value = max(CAST(value AS INTEGER), CAST(excluded.value AS INTEGER))",
            [tag.to_string()],
        )?;
    }
    Ok(())
}

const I: &str = "CAST(COALESCE(NULLIF({}, ''), 0) AS INTEGER)";

fn int(col: &str) -> String {
    I.replace("{}", col)
}

fn import_one(ctx: &mut Ctx, name: &str) -> Result<usize> {
    let conn = ctx.conn;
    remember_dump_date(conn)?;
    match name {
        n if n.starts_with("cat") => import_categories(conn),
        "genres" => {
            let mut st = conn.prepare("INSERT OR REPLACE INTO genre(id, name) VALUES (?1, ?2)")?;
            for_each_blob(conn, |_, r| {
                st.execute(params![r.int(1), r.text(2).unwrap_or_default()])?;
                Ok(())
            })
        }
        "groups" => {
            let mut st = conn.prepare(
                "INSERT OR REPLACE INTO grp(gid, name, short, url, email, irc_server, irc_channel, picname, other, added, updated, rating, votes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?;
            for_each_blob(conn, |_, r| {
                st.execute(params![
                    r.int(1),
                    r.text(2),
                    r.text(3),
                    r.text(4),
                    r.text(5),
                    r.text(6),
                    r.text(7),
                    r.text(8),
                    r.text(9),
                    r.int(10),
                    r.int(11),
                    r.int(12),
                    r.int(13)
                ])?;
                Ok(())
            })
        }
        "animegenres" => {
            let mut st = conn.prepare("INSERT OR REPLACE INTO anime_genre(id, aid, genre_id) VALUES (?1, ?2, ?3)")?;
            for_each_blob(conn, |id, r| {
                let rid = if r.int(3) != 0 { r.int(3) } else { id };
                st.execute(params![rid, r.int(0x0101), r.int(0x0102)])?;
                Ok(())
            })
        }
        "animerelations" => {
            let mut st =
                conn.prepare("INSERT OR REPLACE INTO anime_relation(id, aid, related_aid, type) VALUES (?1, ?2, ?3, ?4)")?;
            for_each_blob(conn, |id, r| {
                let rid = if r.int(0x0201) != 0 { r.int(0x0201) } else { id };
                st.execute(params![rid, r.int(0x0202), r.int(0x0203), r.int(0x0204)])?;
                Ok(())
            })
        }
        "mylist" => {
            let mut st = conn.prepare(
                "INSERT OR REPLACE INTO mylist(lid, fid, eid, aid, date, source, viewed, viewdate, storage, other, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            for_each_blob(conn, |_, r| {
                st.execute(params![
                    r.int(1),
                    r.int(2),
                    r.int(3),
                    r.int(4),
                    r.int(5),
                    r.text(6),
                    r.bool(7),
                    r.int(8),
                    r.text(9),
                    r.text(10),
                    r.int(11)
                ])?;
                Ok(())
            })
        }
        "local_hashes" => {
            let mut st = conn.prepare(
                "INSERT OR REPLACE INTO local_hash(id, ed2k, size, crc32, md5, sha1, fps, length, video_fourcc,
                   video_compression, video_codec, video_height, video_width, video_bitrate, video_vbr,
                   audio_ident, audio_codec, audio_samplerate, audio_bitrate, audio_vbr)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
            )?;
            for_each_blob(conn, |_, r| {
                let ed2k = r.text(2).unwrap_or_default().to_ascii_lowercase();
                if ed2k.len() != 32 {
                    return Ok(());
                }
                st.execute(params![
                    r.int(1),
                    ed2k,
                    r.int(3),
                    r.text(4).map(|s| s.to_ascii_uppercase()),
                    r.text(5).map(|s| s.to_ascii_uppercase()),
                    r.text(6).map(|s| s.to_ascii_uppercase()),
                    r.float(7),
                    r.float(8),
                    r.text(9),
                    r.text(10),
                    r.text(11),
                    r.get(12).and_then(blob::Value::as_i64),
                    r.get(13).and_then(blob::Value::as_i64),
                    r.float(14),
                    r.get(15).and_then(blob::Value::as_i64),
                    r.get(16).and_then(blob::Value::as_i64),
                    r.text(17),
                    r.get(18).and_then(blob::Value::as_i64),
                    r.float(19),
                    r.get(20).and_then(blob::Value::as_i64)
                ])?;
                Ok(())
            })
        }
        "local_files" => {
            let now = chrono::Utc::now().timestamp();
            let mut st = conn.prepare(
                "INSERT OR REPLACE INTO local_file(id, hash_id, path, mtime, added) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for_each_blob(conn, |id, r| {
                let Some(path) = r.text(4).or_else(|| r.text(3)) else {
                    return Ok(());
                };
                let rid = if r.int(1) != 0 { r.int(1) } else { id };
                st.execute(params![rid, r.int(2), path, filetime_to_unix(r.int(5)), now])?;
                Ok(())
            })
        }
        "animes" => copy_table(
            conn,
            "anime(aid, year, producer, picname, url, description, added, updated, rating, votes, episodes, type_id,
                   animenfo_id, reviews, review_rating, animenfo_str, animeplanet_id, air_date, end_date, allcinema_id,
                   awards, restricted)",
            &format!(
                "CAST(id AS INTEGER), NULLIF(col_year, ''), NULLIF(col_producer, ''), NULLIF(col_picurl, ''),
                 NULLIF(col_url, ''), NULLIF(col_other, ''), {}, {}, {}, {}, {}, max(1, {}), {}, {}, {},
                 NULLIF(col_animenfostr, ''), NULLIF(NULLIF(col_aplanet, ''), '0'), {}, {}, {}, NULLIF(col_awards, ''), {}",
                int("col_date"),
                int("col_update"),
                int("col_rating"),
                int("col_votes"),
                int("col_eps"),
                int("col_cid"),
                int("col_animenfoid"),
                int("col_reviews"),
                int("col_reviewrating"),
                int("col_airdate"),
                int("col_enddate"),
                int("col_allcinemaid"),
                int("col_restricted")
            ),
        ),
        "animetitles" => copy_table(
            conn,
            "anime_title(id, aid, name, type, lang)",
            &format!(
                "CAST(id AS INTEGER), {}, col_name, {},
                 COALESCE((SELECT code FROM main.category c WHERE c.kind = 'lang' AND c.id = {}), 'x-unk')",
                int("col_aid"),
                int("col_type"),
                int("col_langid")
            ),
        ),
        "episodes" => {
            let n = copy_table(
                conn,
                "episode(eid, aid, epno, length, other, added, updated, type, recap, aired)",
                &format!(
                    "CAST(id AS INTEGER), {}, {}, {}, NULLIF(col_other, ''), {}, {}, max(1, {}), {}, {}",
                    int("col_aid"),
                    int("col_epno"),
                    int("col_length"),
                    int("col_date"),
                    int("col_update"),
                    int("col_type"),
                    int("col_is_recap"),
                    int("col_aired")
                ),
            )?;
            // Keep the episode's own (English) name as a fallback title until episodetitles is imported.
            conn.execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS legacy_episode_name(eid INTEGER PRIMARY KEY, name TEXT);
                 INSERT OR REPLACE INTO temp.legacy_episode_name(eid, name)
                   SELECT CAST(id AS INTEGER), col_name FROM legacy.data WHERE COALESCE(col_name, '') <> '';",
            )?;
            Ok(n)
        }
        "episodetitles" => {
            let n = copy_table(
                conn,
                "episode_title(id, eid, name, lang)",
                &format!(
                    "CAST(id AS INTEGER), {}, col_name,
                     COALESCE((SELECT code FROM main.category c WHERE c.kind = 'lang' AND c.id = {}), 'x-unk')",
                    int("col_eid"),
                    int("col_langid")
                ),
            )?;
            add_missing_episode_names(conn)?;
            Ok(n)
        }
        "files" => copy_table(
            conn,
            "file(fid, aid, eid, gid, size, md5, crc32, ed2k, filetype, released, quality_id, source_id, other,
                  added, updated, state, user_count, sha1, length, file_type, verified, tth)",
            &format!(
                "CAST(id AS INTEGER), {}, {}, {}, {}, NULLIF(lower(col_md5), ''), NULLIF(lower(col_crc), ''),
                 NULLIF(lower(col_ed2k), ''), NULLIF(col_filetype, ''), {}, max(1, {}), max(1, {}), NULLIF(col_other, ''),
                 {}, {}, {}, {}, NULLIF(lower(col_sha1), ''), {}, {}, {}, {}",
                int("col_aid"),
                int("col_eid"),
                int("col_gid"),
                int("col_size"),
                int("col_released"),
                int("col_qualid"),
                int("col_typeid"),
                int("col_date"),
                int("col_update"),
                int("col_state"),
                int("col_ucnt"),
                int("col_length"),
                int("col_type"),
                int("col_aomverified"),
                if column_exists(conn, "col_tth")? { "NULLIF(lower(col_tth), '')" } else { "NULL" }
            ),
        ),
        "fileepisoderelations" => copy_table(
            conn,
            "file_episode(id, fid, eid, start_pct, end_pct, added)",
            &format!(
                "CAST(id AS INTEGER), {}, {}, {}, {}, {}",
                int("col_fid"),
                int("col_eid"),
                int("col_startp"),
                int("col_endp"),
                int("col_date")
            ),
        ),
        "filerelations" => copy_table(
            conn,
            "file_relation(id, fid, other_fid, type, added)",
            &format!(
                "CAST(id AS INTEGER), {}, {}, {}, {}",
                int("col_fid"),
                int("col_otherfid"),
                int("col_type"),
                int("col_date")
            ),
        ),
        "reviews" => copy_table(
            conn,
            "review(id, aid, vote, vote_anim, vote_sound, vote_story, vote_char, vote_value, vote_enjoy, added, updated,
                    rating, votes, uid, uname, body)",
            &format!(
                "CAST(id AS INTEGER), {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, col_uname, col_body",
                int("col_aid"),
                int("col_vote"),
                int("col_voteanim"),
                int("col_votesound"),
                int("col_votestory"),
                int("col_votechar"),
                int("col_voteval"),
                int("col_voteenj"),
                int("col_date"),
                int("col_update"),
                int("col_rating"),
                int("col_votes"),
                int("col_uid")
            ),
        ),
        other => Err(Error::Legacy(format!("unknown legacy collection {other}"))),
    }
}

fn column_exists(conn: &Connection, col: &str) -> Result<bool> {
    let mut st = conn.prepare("SELECT name FROM pragma_table_info('data', 'legacy')")?;
    let names: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(names.iter().any(|n| n == col))
}

fn copy_table(conn: &Connection, target: &str, select: &str) -> Result<usize> {
    if !table_exists(conn, "data")? {
        return Ok(0);
    }
    let sql = format!("INSERT OR REPLACE INTO main.{target} SELECT {select} FROM legacy.data");
    Ok(conn.execute(&sql, [])?)
}

fn add_missing_episode_names(conn: &Connection) -> Result<()> {
    let has_tmp: i64 = conn.query_row(
        "SELECT count(*) FROM temp.sqlite_master WHERE name = 'legacy_episode_name'",
        [],
        |r| r.get(0),
    )?;
    if has_tmp == 0 {
        return Ok(());
    }
    conn.execute_batch(
        "INSERT INTO main.episode_title(eid, name, lang)
           SELECT n.eid, n.name, 'en' FROM temp.legacy_episode_name n
           WHERE NOT EXISTS (SELECT 1 FROM main.episode_title t WHERE t.eid = n.eid);
         DROP TABLE temp.legacy_episode_name;",
    )?;
    Ok(())
}

fn import_categories(conn: &Connection) -> Result<usize> {
    let mut st = conn.prepare(
        "INSERT OR REPLACE INTO category(kind, id, name, is_default, sort_order, code) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for_each_blob(conn, |_, r| {
        let kind = r.text(1).unwrap_or_default();
        if kind.is_empty() {
            return Ok(());
        }
        st.execute(params![
            kind,
            r.int(2),
            r.text(3).unwrap_or_default(),
            r.bool(4),
            r.int(5),
            r.text(6)
        ])?;
        Ok(())
    })
}
