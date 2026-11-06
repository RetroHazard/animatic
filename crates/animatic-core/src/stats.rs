//! Library statistics and AOM-compatible XML/INI "Statwhore" exports.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::Result;
use crate::library::Anime;
use crate::settings::StatFormat;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Totals {
    pub anime: i64,
    pub episodes: i64,
    pub files: i64,
    pub groups: i64,
    pub size: i64,
    pub length: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MylistTotals {
    pub anime: i64,
    pub episodes: i64,
    pub files: i64,
    pub size: i64,
    pub anime_watched: i64,
    pub anime_partially_watched: i64,
    pub episodes_watched: i64,
    pub files_watched: i64,
    pub watched_size: i64,
    pub length: i64,
    pub watched_length: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TypeStats {
    pub total: i64,
    pub in_mylist: i64,
    pub all_watched: i64,
    pub part_watched: i64,
    pub size: i64,
    pub length: i64,
    pub watched_length: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusStats {
    pub count: i64,
    pub size: i64,
    pub length: i64,
    pub watched: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryStats {
    pub anidb: Totals,
    pub mylist: MylistTotals,
    /// `tvseries`, `tvspecials`, `movies`, `ovas`, `web`, `other`, `unknown`.
    pub types: BTreeMap<String, TypeStats>,
    /// `unknown`, `onhdd`, `oncd`, `deleted`.
    pub statuses: BTreeMap<String, StatusStats>,
}

const TYPE_KEYS: [(&str, i64); 7] = [
    ("tvseries", 2),
    ("tvspecials", 7),
    ("movies", 4),
    ("ovas", 3),
    ("web", 6),
    ("other", 5),
    ("unknown", 1),
];
const STATUS_KEYS: [(&str, i64); 4] = [("unknown", 0), ("onhdd", 1), ("oncd", 2), ("deleted", 3)];

impl Db {
    /// Compute every figure from one consistent snapshot of the database.
    ///
    /// All reads run inside a single read transaction, so a concurrent writer can neither make
    /// the AniDB totals disagree with the MyList figures nor appear half-applied. A failing query
    /// is returned as an error; it never counts as zero.
    pub fn library_stats(&self) -> Result<LibraryStats> {
        self.library_stats_with(|| {})
    }

    /// [`Self::library_stats`], calling `between_reads` after the AniDB totals were read and
    /// before the MyList rows are. Tests use it to commit a write from another connection at that
    /// exact point and prove it stays out of the snapshot.
    fn library_stats_with(&self, mut between_reads: impl FnMut()) -> Result<LibraryStats> {
        let c = self.conn();
        let tx = c.unchecked_transaction()?;
        let scalar = |sql: &str| -> Result<i64> { Ok(c.query_row(sql, [], |r| r.get(0))?) };
        let anidb = Totals {
            anime: scalar("SELECT count(*) FROM anime")?,
            episodes: scalar("SELECT count(*) FROM episode")?,
            files: scalar("SELECT count(*) FROM file")?,
            groups: scalar("SELECT count(*) FROM grp")?,
            size: scalar("SELECT COALESCE(sum(size),0) FROM file")?,
            length: scalar("SELECT COALESCE(sum(length),0) FROM file")?,
        };
        between_reads();
        let items = self.mylist_items()?;
        let states = self.mylist_state_by_anime(&items);
        let anime = self.all_anime()?;
        let anime_by_id: BTreeMap<i64, &Anime> = anime.iter().map(|a| (a.aid, a)).collect();
        let mut ep_ids = HashSet::new();
        let mut watched_ep_ids = HashSet::new();
        let mut mt = MylistTotals::default();
        for item in &items {
            if item.entry.eid != 0 {
                ep_ids.insert(item.entry.eid);
            }
            if item.entry.viewed && item.entry.eid != 0 {
                watched_ep_ids.insert(item.entry.eid);
            }
            mt.files += 1;
            if let Some(f) = &item.file {
                mt.size += f.size;
                mt.length += f.length;
                if item.entry.viewed {
                    mt.watched_size += f.size;
                    mt.watched_length += f.length;
                }
            }
            if item.entry.viewed {
                mt.files_watched += 1;
            }
        }
        mt.anime = states.len() as i64;
        mt.episodes = ep_ids.len() as i64;
        mt.episodes_watched = watched_ep_ids.len() as i64;
        for (aid, state) in &states {
            if let Some(a) = anime_by_id.get(aid) {
                if state.all_watched(a) {
                    mt.anime_watched += 1;
                } else if state.watched_eps + state.watched_specials > 0 {
                    mt.anime_partially_watched += 1;
                }
            }
        }

        let mut types = BTreeMap::new();
        for (key, id) in TYPE_KEYS {
            let mut out = TypeStats {
                total: anime
                    .iter()
                    .filter(|a| normalize_type(a.type_id) == id)
                    .count() as i64,
                ..Default::default()
            };
            for (aid, state) in &states {
                let Some(a) = anime_by_id.get(aid) else {
                    continue;
                };
                if normalize_type(a.type_id) != id {
                    continue;
                }
                out.in_mylist += 1;
                out.size += state.size;
                out.length += state.length;
                out.watched_length += state.watched_length;
                if state.all_watched(a) {
                    out.all_watched += 1;
                } else if state.watched_eps + state.watched_specials > 0 {
                    out.part_watched += 1;
                }
            }
            types.insert(key.into(), out);
        }
        let mut statuses = BTreeMap::new();
        for (key, id) in STATUS_KEYS {
            let mut out = StatusStats::default();
            for item in &items {
                if item.entry.state != id {
                    continue;
                }
                out.count += 1;
                out.watched += item.entry.viewed as i64;
                if let Some(f) = &item.file {
                    out.size += f.size;
                    out.length += f.length;
                }
            }
            statuses.insert(key.into(), out);
        }
        drop(tx);
        Ok(LibraryStats {
            anidb,
            mylist: mt,
            types,
            statuses,
        })
    }
}

fn normalize_type(id: i64) -> i64 {
    if (2..=7).contains(&id) { id } else { 1 }
}

impl LibraryStats {
    pub fn render(&self, username: &str, format: StatFormat) -> String {
        match format {
            StatFormat::Xml => self.xml(username),
            StatFormat::Ini => self.ini(username),
        }
    }

    /// Render and write the statistics file, replacing `path` atomically (see
    /// [`crate::atomic::write_atomic`]): a failed write leaves the previous file intact.
    pub fn write(&self, path: &Path, username: &str, format: StatFormat) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::atomic::write_atomic(path, self.render(username, format).as_bytes())?;
        Ok(())
    }

    fn xml(&self, username: &str) -> String {
        let mut s = String::new();
        let esc = |v: &str| {
            v.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
        };
        writeln!(s, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r").unwrap();
        writeln!(
            s,
            "<stats>\r\n  <anidb>\r\n    <username>{}</username>\r",
            esc(username)
        )
        .unwrap();
        totals_xml(&mut s, &self.anidb);
        s.push_str("  </anidb>\r\n  <mylist>\r\n");
        mylist_xml(&mut s, &self.mylist);
        s.push_str("    <animetypes>\r\n");
        for (key, _) in TYPE_KEYS {
            let v = self.types.get(key).cloned().unwrap_or_default();
            write!(s, "      <animetype type=\"{key}\">\r\n        <total>{}</total>\r\n        <inmylist>{}</inmylist>\r\n        <allwatched>{}</allwatched>\r\n        <partwatched>{}</partwatched>\r\n        <size>{}</size>\r\n        <length>{}</length>\r\n        <lengthwatched>{}</lengthwatched>\r\n      </animetype>\r\n", v.total, v.in_mylist, v.all_watched, v.part_watched, v.size, v.length, v.watched_length).unwrap();
        }
        s.push_str("    </animetypes>\r\n");
        if self.mylist.files > 0 {
            s.push_str("    <status>\r\n");
            for (key, _) in STATUS_KEYS {
                let v = self.statuses.get(key).cloned().unwrap_or_default();
                write!(s, "      <status type=\"{key}\">\r\n        <count>{}</count>\r\n        <size>{}</size>\r\n        <length>{}</length>\r\n        <watched>{}</watched>\r\n      </status>\r\n", v.count, v.size, v.length, v.watched).unwrap();
            }
            s.push_str("    </status>\r\n");
        }
        s.push_str("  </mylist>\r\n</stats>");
        s
    }

    fn ini(&self, username: &str) -> String {
        let mut s = format!(
            "[anidb]\r\nusername={username}\r\nanimecount={}\r\nepisodecount={}\r\nfilecount={}\r\ngroupcount={}\r\nsize={}\r\nlength={}\r\n\r\n",
            self.anidb.anime,
            self.anidb.episodes,
            self.anidb.files,
            self.anidb.groups,
            self.anidb.size,
            self.anidb.length
        );
        write!(s, "[mylist]\r\nanimecount={}\r\nepisodecount={}\r\nfilecount={}\r\nsize={}\r\nanimewatched={}\r\nanimepartiallywatched={}\r\nepisodeswatched={}\r\nfileswatched={}\r\nwatchedsize={}\r\nlength={}\r\nlengthwatched={}\r\n\r\n", self.mylist.anime, self.mylist.episodes, self.mylist.files, self.mylist.size, self.mylist.anime_watched, self.mylist.anime_partially_watched, self.mylist.episodes_watched, self.mylist.files_watched, self.mylist.watched_size, self.mylist.length, self.mylist.watched_length).unwrap();
        let section_names = [
            "TVSeries",
            "TVSpecials",
            "Movies",
            "OVAs",
            "Web",
            "Other",
            "Unknown",
        ];
        for ((key, _), section) in TYPE_KEYS.into_iter().zip(section_names) {
            let v = self.types.get(key).cloned().unwrap_or_default();
            write!(s, "[animetype{section}]\r\ntotal={}\r\ninmylist={}\r\nallwatched={}\r\npartwatched={}\r\nsize={}\r\nlength={}\r\nlengthwatched={}\r\n", v.total, v.in_mylist, v.all_watched, v.part_watched, v.size, v.length, v.watched_length).unwrap();
            if section != "Unknown" || self.mylist.files > 0 {
                s.push_str("\r\n");
            }
        }
        if self.mylist.files > 0 {
            let sections = ["Unknown", "OnHDD", "OnCD", "Deleted"];
            for (index, ((key, _), section)) in STATUS_KEYS.into_iter().zip(sections).enumerate() {
                let v = self.statuses.get(key).cloned().unwrap_or_default();
                write!(
                    s,
                    "[status{section}]\r\ncount={}\r\nsize={}\r\nlength={}\r\nwatched={}\r\n",
                    v.count, v.size, v.length, v.watched
                )
                .unwrap();
                if index + 1 < STATUS_KEYS.len() {
                    s.push_str("\r\n");
                }
            }
        }
        s
    }
}

fn totals_xml(s: &mut String, v: &Totals) {
    write!(s, "    <animecount>{}</animecount>\r\n    <episodecount>{}</episodecount>\r\n    <filecount>{}</filecount>\r\n    <groupcount>{}</groupcount>\r\n    <size>{}</size>\r\n    <length>{}</length>\r\n", v.anime, v.episodes, v.files, v.groups, v.size, v.length).unwrap();
}

fn mylist_xml(s: &mut String, v: &MylistTotals) {
    write!(s, "    <animecount>{}</animecount>\r\n    <episodecount>{}</episodecount>\r\n    <filecount>{}</filecount>\r\n    <size>{}</size>\r\n    <animewatched>{}</animewatched>\r\n    <animepartiallywatched>{}</animepartiallywatched>\r\n    <episodeswatched>{}</episodeswatched>\r\n    <fileswatched>{}</fileswatched>\r\n    <watchedsize>{}</watchedsize>\r\n    <length>{}</length>\r\n    <lengthwatched>{}</lengthwatched>\r\n", v.anime, v.episodes, v.files, v.size, v.anime_watched, v.anime_partially_watched, v.episodes_watched, v.files_watched, v.watched_size, v.length, v.watched_length).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stats_have_stable_legacy_sections() {
        let db = Db::memory().unwrap();
        let stats = db.library_stats().unwrap();
        let ini = stats.render("a&b", StatFormat::Ini);
        assert!(ini.starts_with("[anidb]\r\nusername=a&b\r\n"));
        assert!(ini.contains("[animetypeTVSeries]"));
        assert!(!ini.contains("[statusUnknown]"));
        let xml = stats.render("a&b", StatFormat::Xml);
        assert!(xml.contains("<username>a&amp;b</username>"));
        assert!(xml.ends_with("</stats>"));
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "animatic-stats-test-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_failed_scalar_query_is_an_error_not_zero() {
        let db = Db::memory().unwrap();
        db.conn().execute_batch("DROP TABLE grp;").unwrap();
        let err = db.library_stats().unwrap_err();
        assert!(matches!(err, crate::error::Error::Db(_)), "{err:?}");
        assert!(err.to_string().contains("grp"), "{err}");
    }

    #[test]
    fn a_failed_mylist_query_is_an_error_not_empty_figures() {
        let db = Db::memory().unwrap();
        db.conn().execute_batch("DROP TABLE mylist;").unwrap();
        assert!(db.library_stats().is_err());
    }

    #[test]
    fn figures_come_from_one_snapshot_even_when_a_write_commits_mid_read() {
        let dir = temp_dir("snapshot");
        let path = dir.join("library.db");
        let db = Db::open(&path).unwrap();
        db.conn()
            .execute_batch(
                "INSERT INTO file(fid, aid, size) VALUES (1, 10, 100);
                 INSERT INTO mylist(lid, fid, aid) VALUES (1, 1, 10);",
            )
            .unwrap();
        let writer = Db::open(&path).unwrap();
        let stats = db
            .library_stats_with(|| {
                writer
                    .conn()
                    .execute_batch(
                        "INSERT INTO file(fid, aid, size) VALUES (2, 10, 200);
                         INSERT INTO mylist(lid, fid, aid) VALUES (2, 2, 10);",
                    )
                    .unwrap();
            })
            .unwrap();
        assert_eq!((stats.anidb.files, stats.anidb.size), (1, 100));
        assert_eq!((stats.mylist.files, stats.mylist.size), (1, 100));
        // The next read sees the committed write, in every figure at once.
        let later = db.library_stats().unwrap();
        assert_eq!((later.anidb.files, later.mylist.files), (2, 2));
        drop((db, writer));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_keeps_the_previous_statistics_file() {
        let dir = temp_dir("write");
        let target = dir.join("stats.ini");
        let stats = Db::memory().unwrap().library_stats().unwrap();
        stats
            .write(&target, "animatic-test-user", StatFormat::Ini)
            .unwrap();
        let first = std::fs::read(&target).unwrap();
        assert!(first.starts_with(b"[anidb]"));

        // A directory in place of the target makes the final rename fail on every platform.
        let blocked = dir.join("blocked.ini");
        std::fs::create_dir(&blocked).unwrap();
        assert!(
            stats
                .write(&blocked, "animatic-test-user", StatFormat::Ini)
                .is_err()
        );
        assert!(blocked.is_dir());
        assert_eq!(std::fs::read(&target).unwrap(), first);
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["blocked.ini", "stats.ini"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
