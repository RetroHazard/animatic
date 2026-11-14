//! Builds the synthetic sample library that the wiki screenshots show
//! (`scripts/wiki-screenshots.ps1` runs it; see `docs/development.md`).
//!
//! Usage: `docs-sample-library <ROOT> [--fresh]`
//!
//! Writes `<ROOT>/animatic.sqlite3`, `<ROOT>/settings.json` and small placeholder media files
//! below `<ROOT>/Anime` and `<ROOT>/Downloads`. Every anime, group, file, hash, account figure
//! and path is invented; nothing is read from AniDB or from a real library. `--fresh` writes an
//! empty library with first-launch settings instead. `<ROOT>` must not exist or be empty.

use std::path::{Path, PathBuf};

use animatic_anidb::commands::{FileRef, MylistEdit, MylistStats};
use animatic_core::db::Db;
use animatic_core::hash::Hashes;
use animatic_core::hash_queue::HashSource;
use animatic_core::secrets::{Credentials, MemorySecretStore};
use animatic_core::settings::{FileManager, Player, Settings};
use animatic_core::sync::Command;
use animatic_core::{Error, Result};
use rusqlite::params;

/// 2025-10-04 12:00 UTC: every stored date is relative to this, so the pictures do not change
/// from one run to the next.
const NOW: i64 = 1_759_579_200;
const DAY: i64 = 86_400;

struct Anime {
    aid: i64,
    main: &'static str,
    english: &'static str,
    type_id: i64,
    year: &'static str,
    episodes: i64,
    aired: i64,
    rating: i64,
    votes: i64,
    genres: &'static [&'static str],
    description: &'static str,
}

const ANIME: &[Anime] = &[
    Anime {
        aid: 90001,
        main: "Sakura Shingou",
        english: "Sakura Signal",
        type_id: 2,
        year: "2024",
        episodes: 12,
        aired: NOW - 540 * DAY,
        rating: 812,
        votes: 1843,
        genres: &["School", "Music", "Slice of Life"],
        description: "Four first-years revive their school's [b]broadcast club[/b] and keep it on the air \
            for one spring.\n[i]Source: the Animatic sample library; every title here is invented.[/i]",
    },
    Anime {
        aid: 90002,
        main: "Kiriyama no Tourou",
        english: "Lanterns of Kiriyama",
        type_id: 2,
        year: "2022",
        episodes: 24,
        aired: NOW - 1180 * DAY,
        rating: 856,
        votes: 4210,
        genres: &["Fantasy", "Adventure", "Mystery"],
        description: "A lantern maker's apprentice follows the lights that leave the mountain village \
            every autumn.",
    },
    Anime {
        aid: 90003,
        main: "Ryuusei Haitatsunin",
        english: "Starfall Courier",
        type_id: 4,
        year: "2023",
        episodes: 1,
        aired: NOW - 760 * DAY,
        rating: 790,
        votes: 962,
        genres: &["Science Fiction", "Adventure"],
        description: "One night, one parcel and a courier who has never left the orbital ring.",
    },
    Anime {
        aid: 90004,
        main: "Koke to Shikkui",
        english: "Moss and Mortar",
        type_id: 3,
        year: "2021",
        episodes: 4,
        aired: NOW - 1500 * DAY,
        rating: 734,
        votes: 388,
        genres: &["Comedy", "Slice of Life"],
        description: "Two plasterers restore an old bathhouse, one wall at a time.",
    },
    Anime {
        aid: 90005,
        main: "Kiriyama no Tourou: Hi no Michi",
        english: "Lanterns of Kiriyama: Ember Road",
        type_id: 2,
        year: "2025",
        episodes: 0,
        aired: NOW - 70 * DAY,
        rating: 0,
        votes: 0,
        genres: &["Fantasy", "Adventure"],
        description: "The second season. Currently airing.",
    },
    Anime {
        aid: 90006,
        main: "Kami no Tsuki Radio",
        english: "Paper Moon Radio",
        type_id: 6,
        year: "2020",
        episodes: 6,
        aired: NOW - 1900 * DAY,
        rating: 768,
        votes: 515,
        genres: &["Music", "Drama"],
        description: "A late-night radio show that only plays requests from people who are moving away.",
    },
    Anime {
        aid: 90007,
        main: "Karakuri Tanuki",
        english: "Clockwork Tanuki",
        type_id: 7,
        year: "2019",
        episodes: 1,
        aired: NOW - 2300 * DAY,
        rating: 701,
        votes: 207,
        genres: &["Comedy"],
        description: "A New Year special about a wind-up raccoon dog that keeps running fast.",
    },
];

const EPISODE_TITLES: &[&str] = &[
    "The First Broadcast",
    "Static in the Hallway",
    "A Song for the Rooftop",
    "Rain Delay",
    "Request Hour",
    "The Borrowed Microphone",
    "Festival Frequency",
    "Off the Air",
    "Night Shift",
    "Interference",
    "The Last Request",
    "Signal Strong",
    "Lights on the Ridge",
    "The Kiln Keeper",
    "Paper and Oil",
    "A Village Without Lights",
    "The Ferryman's Lamp",
    "Moths",
    "Under the Cedar",
    "Embers",
    "The Long Stair",
    "Winter Wick",
    "Smoke Signals",
    "Homecoming",
];

struct Group {
    gid: i64,
    name: &'static str,
    short: &'static str,
    rating: i64,
    votes: i64,
}

const GROUPS: &[Group] = &[
    Group {
        gid: 7001,
        name: "Hanabi Subs",
        short: "Hanabi",
        rating: 845,
        votes: 120,
    },
    Group {
        gid: 7002,
        name: "Nightjar Fansubs",
        short: "NJF",
        rating: 801,
        votes: 64,
    },
    Group {
        gid: 7003,
        name: "Kumo Raws",
        short: "Kumo",
        rating: 0,
        votes: 0,
    },
];

/// A release in the sample: which anime and episodes, by which group, and how far the user got.
struct Release {
    aid: i64,
    gid: i64,
    /// AniDB knows files for episodes 1 to `known`; the user has 1 to `local` on disk (hashed)
    /// and 1 to `mylist` in MyList, and watched 1 to `watched`.
    known: i64,
    local: i64,
    mylist: i64,
    watched: i64,
    /// Folder below the library root, and whether the files are still named like downloads.
    folder: &'static str,
    renamed: bool,
    resolution: &'static str,
    mb: u64,
}

const RELEASES: &[Release] = &[
    Release {
        aid: 90001,
        gid: 7001,
        known: 12,
        local: 12,
        mylist: 12,
        watched: 8,
        folder: "Anime/Sakura Signal",
        renamed: true,
        resolution: "1920x1080",
        mb: 1180,
    },
    Release {
        aid: 90002,
        gid: 7002,
        known: 24,
        local: 10,
        mylist: 24,
        watched: 14,
        folder: "Anime/Lanterns of Kiriyama",
        renamed: true,
        resolution: "1920x1080",
        mb: 980,
    },
    Release {
        aid: 90003,
        gid: 7003,
        known: 1,
        local: 1,
        mylist: 1,
        watched: 1,
        folder: "Anime/Starfall Courier",
        renamed: true,
        resolution: "3840x2160",
        mb: 14_200,
    },
    Release {
        aid: 90004,
        gid: 7001,
        known: 4,
        local: 4,
        mylist: 4,
        watched: 0,
        folder: "Anime/Moss and Mortar",
        renamed: true,
        resolution: "1920x1080",
        mb: 1420,
    },
    Release {
        aid: 90005,
        gid: 7002,
        known: 6,
        local: 3,
        mylist: 0,
        watched: 0,
        folder: "Downloads",
        renamed: false,
        resolution: "1920x1080",
        mb: 1350,
    },
    Release {
        aid: 90006,
        gid: 7001,
        known: 6,
        local: 6,
        mylist: 6,
        watched: 0,
        folder: "Downloads",
        renamed: false,
        resolution: "1280x720",
        mb: 410,
    },
];

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let root = PathBuf::from(args.next().ok_or_else(usage)?);
    let fresh = match args.next() {
        None => false,
        Some(flag) if flag == "--fresh" => true,
        Some(_) => return Err(usage()),
    };
    if root.exists() && std::fs::read_dir(&root)?.next().is_some() {
        return Err(Error::Other(format!("{} is not empty", root.display())));
    }
    std::fs::create_dir_all(&root)?;
    let db = Db::open(&root.join("animatic.sqlite3"))?;
    // The sample has no secrets; an in-memory store keeps the real keystore out of it.
    let credentials = Credentials::with_store(MemorySecretStore::new());
    settings(&root, fresh).save(&root.join("settings.json"), &credentials)?;
    if !fresh {
        library(&db, &root)?;
    }
    println!(
        "Wrote the {} sample library to {}",
        if fresh { "empty" } else { "full" },
        root.display()
    );
    Ok(())
}

fn usage() -> Error {
    Error::Other("usage: docs-sample-library <ROOT> [--fresh]".into())
}

fn path_in(root: &Path, relative: &str) -> String {
    let mut path = root.to_path_buf();
    path.extend(relative.split('/'));
    path.to_string_lossy().into_owned()
}

fn settings(root: &Path, fresh: bool) -> Settings {
    let mut s = Settings {
        first_launch: fresh,
        ..Settings::default()
    };
    // Never contact AniDB or GitHub while pictures are taken.
    s.anidb.offline = true;
    s.updates.auto_check = false;
    s.hashing.autostart = false;
    s.ui.show_tray_icon = false;
    s.ui.window = Some([60.0, 60.0, 1280.0, 800.0]);
    if fresh {
        return s;
    }
    s.anidb.username = "animatic-test-user".into();
    s.players.push(Player {
        name: "Sample Player".into(),
        path: path_in(root, "Tools/Sample Player/player.exe"),
        open: "{path}".into(),
        play: "--play {path}".into(),
        enqueue: "--enqueue {path}".into(),
        default: true,
    });
    // A user's own rules come after the built-in ones.
    s.file_managers.managers.extend([
        FileManager {
            name: "Anime".into(),
            path: path_in(root, "Anime"),
            base_path: path_in(root, "Anime"),
            auto_hash: true,
            auto_rehash: true,
            clear_empty_folders: true,
            can_edit_path: true,
            ..FileManager::default()
        },
        FileManager {
            name: "Downloads".into(),
            path: path_in(root, "Downloads"),
            base_path: path_in(root, "Anime"),
            auto_hash: true,
            mylist_auto_add: true,
            mylist_state_check: true,
            mylist_state: 1,
            mylist_storage_check: true,
            mylist_storage: "Library disk".into(),
            can_edit_path: true,
            ..FileManager::default()
        },
    ]);
    s
}

/// Deterministic pseudo-random hex for invented hashes.
fn hex(seed: &str, digits: usize, upper: bool) -> String {
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    for b in seed.bytes() {
        state = (state ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    let mut out = String::with_capacity(digits);
    while out.len() < digits {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push_str(&format!("{state:016x}"));
    }
    out.truncate(digits);
    if upper { out.to_ascii_uppercase() } else { out }
}

fn hashes(seed: &str, size: u64) -> Hashes {
    Hashes {
        size,
        ed2k: hex(&format!("ed2k{seed}"), 32, false),
        crc32: hex(&format!("crc{seed}"), 8, true),
        md5: hex(&format!("md5{seed}"), 32, true),
        sha1: hex(&format!("sha1{seed}"), 40, true),
    }
}

fn touch(path: &str) -> Result<()> {
    let path = Path::new(path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, b"Animatic sample library placeholder, not a video\n")?;
    Ok(())
}

fn library(db: &Db, root: &Path) -> Result<()> {
    let conn = db.conn();
    let mut genre_ids = std::collections::BTreeMap::new();
    for anime in ANIME {
        conn.execute(
            "INSERT INTO anime(aid, type_id, year, episodes, highest_ep, special_eps, air_date, end_date,
                url, picname, description, rating, votes, temp_rating, temp_votes, reviews, review_rating,
                added, updated, fetched)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?12, ?13, ?14, ?15, ?16, ?16, ?16)",
            params![
                anime.aid,
                anime.type_id,
                anime.year,
                anime.episodes,
                if anime.episodes == 0 { 6 } else { anime.episodes },
                i64::from(anime.aid == 90001),
                anime.aired,
                if anime.episodes == 0 { 0 } else { anime.aired + anime.episodes * 7 * DAY },
                "https://example.invalid/animatic-sample",
                format!("sample-{}.png", anime.aid),
                anime.description,
                anime.rating,
                anime.votes,
                i64::from(anime.aid == 90001),
                if anime.aid == 90001 { 800 } else { 0 },
                NOW - 30 * DAY,
            ],
        )?;
        conn.execute(
            "INSERT INTO anime_title(aid, name, type, lang) VALUES (?1, ?2, 1, 'x-jat'), (?1, ?3, 4, 'en')",
            params![anime.aid, anime.main, anime.english],
        )?;
        for genre in anime.genres {
            let next = genre_ids.len() as i64 + 1;
            let id = *genre_ids.entry(*genre).or_insert(next);
            conn.execute(
                "INSERT OR IGNORE INTO genre(id, name) VALUES (?1, ?2)",
                params![id, genre],
            )?;
            conn.execute(
                "INSERT INTO anime_genre(aid, genre_id, weight) VALUES (?1, ?2, 400)",
                params![anime.aid, id],
            )?;
        }
    }
    conn.execute_batch(
        "INSERT INTO anime_title(aid, name, type, lang) VALUES (90001, 'SakuSig', 3, 'x-jat');
         INSERT INTO anime_relation(aid, related_aid, type) VALUES (90002, 90005, 1), (90005, 90002, 2),
             (90002, 90006, 100);
         INSERT INTO category(kind, id, name, is_default, sort_order) VALUES
             ('qual', 2, 'very high', 0, 2), ('qual', 3, 'high', 0, 3),
             ('type', 16, 'Blu-ray', 0, 13), ('type', 11, 'Web', 0, 11);",
    )?;
    conn.execute(
        "INSERT INTO review(aid, uid, uname, vote, rating, votes, body, added) VALUES (90001, 1, ?1, 850, 850, 12, ?2, ?3)",
        params![
            "animatic-test-user",
            "[b]Gentle and warm.[/b] The club scenes carry the show.<br />[spoiler]The finale is a live broadcast.[/spoiler]",
            NOW - 200 * DAY
        ],
    )?;
    for g in GROUPS {
        conn.execute(
            "INSERT INTO grp(gid, name, short, url, rating, votes, added, updated) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                g.gid,
                g.name,
                g.short,
                if g.votes > 0 { Some("https://example.invalid/group") } else { None },
                g.rating,
                g.votes,
                NOW - 90 * DAY
            ],
        )?;
    }

    // Episodes: one id per anime and number, plus one special of Sakura Signal.
    let eid = |aid: i64, ep: i64| aid * 100 + ep;
    for anime in ANIME {
        let count = if anime.episodes == 0 {
            6
        } else {
            anime.episodes
        };
        for ep in 1..=count {
            let title = if anime.episodes == 1 {
                "Complete Movie"
            } else {
                EPISODE_TITLES
                    [((anime.aid - 90001) as usize * 5 + ep as usize - 1) % EPISODE_TITLES.len()]
            };
            conn.execute(
                "INSERT INTO episode(eid, aid, epno, type, length, aired, rating, votes, added, updated)
                 VALUES (?1, ?2, ?3, 1, ?4, ?5, 790, 40, ?6, ?6)",
                params![
                    eid(anime.aid, ep),
                    anime.aid,
                    ep,
                    if anime.type_id == 4 { 104 } else { 24 },
                    anime.aired + (ep - 1) * 7 * DAY,
                    NOW - 30 * DAY
                ],
            )?;
            conn.execute(
                "INSERT INTO episode_title(eid, name, lang) VALUES (?1, ?2, 'en')",
                params![eid(anime.aid, ep), title],
            )?;
        }
    }
    conn.execute(
        "INSERT INTO episode(eid, aid, epno, type, length, aired) VALUES (9000199, 90001, 1, 2, 6, ?1)",
        [NOW - 400 * DAY],
    )?;
    conn.execute(
        "INSERT INTO episode_title(eid, name, lang) VALUES (9000199, 'Club Room Shorts', 'en')",
        [],
    )?;

    let mut lid = 50_000;
    let mut fid = 300_000;
    for r in RELEASES {
        let anime = ANIME
            .iter()
            .find(|a| a.aid == r.aid)
            .expect("release of a sample anime");
        let group = GROUPS
            .iter()
            .find(|g| g.gid == r.gid)
            .expect("release of a sample group");
        for ep in 1..=r.known {
            fid += 1;
            let size = r.mb * 1_048_576 + (ep as u64) * 7_919;
            let seed = format!("{}-{ep}", r.aid);
            let h = hashes(&seed, size);
            let name = if !r.renamed {
                format!(
                    "{}_{}_{ep:02}_{}p.mkv",
                    group.short.to_ascii_lowercase(),
                    anime
                        .english
                        .to_ascii_lowercase()
                        .replace([' ', ':'], "_")
                        .replace("__", "_"),
                    &r.resolution[r.resolution.find('x').map_or(0, |i| i + 1)..]
                )
            } else if anime.episodes == 1 {
                format!("[{}] {} [{}].mkv", group.short, anime.english, h.crc32)
            } else {
                format!(
                    "[{}] {} - {ep:02} [{}].mkv",
                    group.short, anime.english, h.crc32
                )
            };
            conn.execute(
                "INSERT INTO file(fid, aid, eid, gid, size, ed2k, md5, sha1, crc32, filetype, released, quality_id,
                    source_id, state, length, anidb_filename, video_codec, resolution, audio_codecs,
                    dub_langs, sub_langs, added, updated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'mkv', ?10, 2, ?11, 1, ?12, ?13, 'H264/AVC', ?14,
                    'AAC', 'japanese', 'english', ?15, ?15)",
                params![
                    fid,
                    r.aid,
                    eid(r.aid, ep),
                    r.gid,
                    size as i64,
                    h.ed2k,
                    h.md5,
                    h.sha1,
                    h.crc32,
                    anime.aired + ep * 7 * DAY,
                    if r.gid == 7003 { 16 } else { 11 },
                    if anime.type_id == 4 { 6240 } else { 1440 },
                    name,
                    r.resolution,
                    NOW - 60 * DAY,
                ],
            )?;
            if ep <= r.local {
                let path = path_in(root, &format!("{}/{name}", r.folder));
                touch(&path)?;
                db.add_local_file(&path, &h, NOW - 20 * DAY)?;
            }
            if ep <= r.mylist {
                lid += 1;
                let on_disk = ep <= r.local;
                let viewed = ep <= r.watched;
                conn.execute(
                    "INSERT INTO mylist(lid, fid, eid, aid, gid, date, state, viewed, viewdate, storage, source, other)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, NULL)",
                    params![
                        lid,
                        fid,
                        eid(r.aid, ep),
                        r.aid,
                        r.gid,
                        NOW - 120 * DAY + ep * DAY,
                        if on_disk { 1 } else { 2 },
                        i64::from(viewed),
                        if viewed { NOW - 100 * DAY + ep * DAY } else { 0 },
                        if on_disk { "Library disk" } else { "Shelf B, disc 2" },
                    ],
                )?;
            }
        }
    }

    // A file whose name does not match its contents: AniDB knows the episode, the local CRC differs.
    let mismatch = path_in(
        root,
        "Downloads/[Hanabi] Moss and Mortar - 04v2 [0BADC0DE].mkv",
    );
    touch(&mismatch)?;
    let mut bad = hashes("90004-4", 1420 * 1_048_576 + 4 * 7_919);
    bad.crc32 = "0BADC0DE".into();
    conn.execute(
        "INSERT INTO file(fid, aid, eid, gid, size, ed2k, crc32, filetype, state, anidb_filename, resolution, added)
         VALUES (399999, 90004, 9000404, 7001, ?1, ?2, '7C3E91A4', 'mkv', 1, 'moss_04v2.mkv', '1920x1080', ?3)",
        params![bad.size as i64, hex("mismatch", 32, false), NOW - 10 * DAY],
    )?;
    bad.ed2k = hex("mismatch", 32, false);
    db.add_local_file(&mismatch, &bad, NOW - 2 * DAY)?;

    // Hashed but not identified by AniDB yet.
    for name in ["home_video_2019.mkv", "unknown_release_ep03.mkv"] {
        let path = path_in(root, &format!("Downloads/{name}"));
        touch(&path)?;
        let h = hashes(name, 734_003_200);
        db.add_local_file(&path, &h, NOW - DAY)?;
        db.queue_push(&Command::Identify {
            size: h.size,
            ed2k: h.ed2k,
            add: None,
        })?;
    }

    // Commands waiting for AniDB: an edit, an add and one that AniDB refused.
    db.queue_edit_mylist(
        50_041,
        &MylistEdit {
            viewed: Some(true),
            viewdate: Some(NOW - DAY),
            ..MylistEdit::default()
        },
    )?;
    db.queue_push(&Command::MylistAdd {
        file: FileRef::Fid(300_044),
        edit: MylistEdit {
            state: Some(1),
            storage: Some("Library disk".into()),
            ..MylistEdit::default()
        },
    })?;
    // A permanent refusal (505) blocks the command until the user retries or clears it.
    let refused = db.queue_push(&Command::MylistAdd {
        file: FileRef::Fid(300_046),
        edit: MylistEdit {
            state: Some(1),
            ..MylistEdit::default()
        },
    })?;
    db.queue_block(refused, "illegal input: ILLEGAL INPUT OR ACCESS DENIED")?;

    // Files waiting to be hashed, and one that could not be read.
    let mut waiting = Vec::new();
    for name in [
        "[NJF] Ember Road - 04.mkv",
        "[NJF] Ember Road - 05.mkv",
        "damaged_download.mkv",
    ] {
        let path = path_in(root, &format!("Downloads/Incoming/{name}"));
        touch(&path)?;
        waiting.push(PathBuf::from(path));
    }
    db.hash_queue_enqueue(&waiting, HashSource::Scan)?;
    let failed: i64 = conn.query_row(
        "SELECT id FROM hash_queue WHERE path LIKE '%damaged_download.mkv'",
        [],
        |r| r.get(0),
    )?;
    db.hash_queue_fail(
        failed,
        // The text hash_queue writes for an I/O error, with the Windows "drive not ready" error.
        "the file could not be read: The device is not ready. (os error 21)",
    )?;

    db.store_mylist_stats(
        &MylistStats {
            animes: 5,
            episodes: 47,
            files: 47,
            size_mb: 72_310,
            added_animes: 2,
            added_episodes: 31,
            added_files: 12,
            added_groups: 1,
            leech_pct: 18,
            glory_pct: 64,
            viewed_pct_of_db: 1,
            mylist_pct_of_db: 1,
            viewed_pct_of_mylist: 49,
            viewed_episodes: 23,
            votes: 9,
            reviews: 1,
            viewed_minutes: 1_242,
        },
        NOW - 3 * DAY,
    )?;
    db.set_meta("titles_dump_date", &(NOW - 2 * DAY).to_string())?;
    touch(&path_in(root, "Tools/Sample Player/player.exe"))?;
    Ok(())
}
