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
    pub fn library_stats(&self) -> Result<LibraryStats> {
        let c = self.conn();
        let scalar = |sql: &str| -> i64 { c.query_row(sql, [], |r| r.get(0)).unwrap_or(0) };
        let anidb = Totals {
            anime: scalar("SELECT count(*) FROM anime"),
            episodes: scalar("SELECT count(*) FROM episode"),
            files: scalar("SELECT count(*) FROM file"),
            groups: scalar("SELECT count(*) FROM grp"),
            size: scalar("SELECT COALESCE(sum(size),0) FROM file"),
            length: scalar("SELECT COALESCE(sum(length),0) FROM file"),
        };
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

    pub fn write(&self, path: &Path, username: &str, format: StatFormat) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.render(username, format))?;
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
}
