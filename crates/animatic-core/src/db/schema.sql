-- Animatic database schema (version 1).
-- AniDB cache tables mirror what the legacy client stored in kowai/*.data plus
-- the extra fields the modern UDP/HTTP APIs return. User tables hold mylist,
-- hashed (local) files and the pending command queue.

PRAGMA foreign_keys = OFF;

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT
);

-- Category lookup tables of the legacy client (cat*.data):
-- kind in ('cat','lang','qual','type','vcodec','acodec','res','sub').
CREATE TABLE IF NOT EXISTS category (
    kind       TEXT    NOT NULL,
    id         INTEGER NOT NULL,
    name       TEXT    NOT NULL,
    is_default INTEGER NOT NULL DEFAULT 0,
    sort_order INTEGER NOT NULL DEFAULT 0,
    code       TEXT,
    PRIMARY KEY (kind, id)
);

CREATE TABLE IF NOT EXISTS anime (
    aid            INTEGER PRIMARY KEY,
    type_id        INTEGER NOT NULL DEFAULT 1,  -- category(kind='cat')
    year           TEXT,
    episodes       INTEGER NOT NULL DEFAULT 0,  -- total normal episodes (0 = unknown/ongoing)
    highest_ep     INTEGER NOT NULL DEFAULT 0,
    special_eps    INTEGER NOT NULL DEFAULT 0,
    air_date       INTEGER NOT NULL DEFAULT 0,  -- unix time
    end_date       INTEGER NOT NULL DEFAULT 0,
    url            TEXT,
    picname        TEXT,
    producer       TEXT,
    description    TEXT,
    rating         INTEGER NOT NULL DEFAULT 0,  -- x100
    votes          INTEGER NOT NULL DEFAULT 0,
    temp_rating    INTEGER NOT NULL DEFAULT 0,
    temp_votes     INTEGER NOT NULL DEFAULT 0,
    review_rating  INTEGER NOT NULL DEFAULT 0,
    reviews        INTEGER NOT NULL DEFAULT 0,
    restricted     INTEGER NOT NULL DEFAULT 0,
    ann_id         INTEGER NOT NULL DEFAULT 0,
    allcinema_id   INTEGER NOT NULL DEFAULT 0,
    animenfo_id    INTEGER NOT NULL DEFAULT 0,
    animenfo_str   TEXT,
    animeplanet_id TEXT,
    awards         TEXT,
    added          INTEGER NOT NULL DEFAULT 0,
    updated        INTEGER NOT NULL DEFAULT 0,
    fetched        INTEGER NOT NULL DEFAULT 0   -- when we last pulled full data from AniDB
);

-- type: 1 main, 2 synonym, 3 short, 4 official, 5/6 other legacy kinds.
CREATE TABLE IF NOT EXISTS anime_title (
    id   INTEGER PRIMARY KEY,
    aid  INTEGER NOT NULL,
    name TEXT    NOT NULL,
    type INTEGER NOT NULL DEFAULT 2,
    lang TEXT    NOT NULL DEFAULT 'x-unk'
);
CREATE INDEX IF NOT EXISTS anime_title_aid ON anime_title(aid);
CREATE INDEX IF NOT EXISTS anime_title_name ON anime_title(name COLLATE NOCASE);

-- type: 1 regular, 2 special, 3 credit, 4 trailer, 5 parody, 6 other.
CREATE TABLE IF NOT EXISTS episode (
    eid      INTEGER PRIMARY KEY,
    aid      INTEGER NOT NULL,
    epno     INTEGER NOT NULL DEFAULT 0,
    type     INTEGER NOT NULL DEFAULT 1,
    length   INTEGER NOT NULL DEFAULT 0,      -- minutes
    aired    INTEGER NOT NULL DEFAULT 0,
    rating   INTEGER NOT NULL DEFAULT 0,
    votes    INTEGER NOT NULL DEFAULT 0,
    other    TEXT,
    recap    INTEGER NOT NULL DEFAULT 0,
    added    INTEGER NOT NULL DEFAULT 0,
    updated  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS episode_aid ON episode(aid);

CREATE TABLE IF NOT EXISTS episode_title (
    id   INTEGER PRIMARY KEY,
    eid  INTEGER NOT NULL,
    name TEXT    NOT NULL,
    lang TEXT    NOT NULL DEFAULT 'x-unk'
);
CREATE INDEX IF NOT EXISTS episode_title_eid ON episode_title(eid);

CREATE TABLE IF NOT EXISTS grp (
    gid         INTEGER PRIMARY KEY,
    name        TEXT,
    short       TEXT,
    url         TEXT,
    email       TEXT,
    irc_server  TEXT,
    irc_channel TEXT,
    picname     TEXT,
    other       TEXT,
    rating      INTEGER NOT NULL DEFAULT 0,
    votes       INTEGER NOT NULL DEFAULT 0,
    added       INTEGER NOT NULL DEFAULT 0,
    updated     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS file (
    fid            INTEGER PRIMARY KEY,
    aid            INTEGER NOT NULL DEFAULT 0,
    eid            INTEGER NOT NULL DEFAULT 0,
    gid            INTEGER NOT NULL DEFAULT 0,
    size           INTEGER NOT NULL DEFAULT 0,
    ed2k           TEXT,
    md5            TEXT,
    sha1           TEXT,
    crc32          TEXT,
    tth            TEXT,
    filetype       TEXT,                       -- extension
    released       INTEGER NOT NULL DEFAULT 0,
    quality_id     INTEGER NOT NULL DEFAULT 1, -- category(kind='qual')
    source_id      INTEGER NOT NULL DEFAULT 1, -- category(kind='type')
    file_type      INTEGER NOT NULL DEFAULT 10,-- legacy col_type (10 video, 20 audio, 30 subtitle, ..., 90 generic?)
    other          TEXT,
    state          INTEGER NOT NULL DEFAULT 0, -- AniDB state bit field (crc ok/err, version, censor)
    user_count     INTEGER NOT NULL DEFAULT 0,
    length         INTEGER NOT NULL DEFAULT 0, -- seconds
    deprecated     INTEGER NOT NULL DEFAULT 0,
    verified       INTEGER NOT NULL DEFAULT 0, -- legacy aomverified
    anidb_filename TEXT,
    video_codec    TEXT,
    video_bitrate  INTEGER NOT NULL DEFAULT 0,
    resolution     TEXT,
    audio_codecs   TEXT,
    audio_bitrates TEXT,
    dub_langs      TEXT,
    sub_langs      TEXT,
    description    TEXT,
    added          INTEGER NOT NULL DEFAULT 0,
    updated        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS file_aid ON file(aid);
CREATE INDEX IF NOT EXISTS file_eid ON file(eid);
CREATE INDEX IF NOT EXISTS file_gid ON file(gid);
CREATE INDEX IF NOT EXISTS file_ed2k ON file(ed2k, size);

-- Extra episodes covered by a file (beyond file.eid).
CREATE TABLE IF NOT EXISTS file_episode (
    id        INTEGER PRIMARY KEY,
    fid       INTEGER NOT NULL,
    eid       INTEGER NOT NULL,
    start_pct INTEGER NOT NULL DEFAULT 0,
    end_pct   INTEGER NOT NULL DEFAULT 100,
    added     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS file_episode_fid ON file_episode(fid);
CREATE INDEX IF NOT EXISTS file_episode_eid ON file_episode(eid);

CREATE TABLE IF NOT EXISTS file_relation (
    id        INTEGER PRIMARY KEY,
    fid       INTEGER NOT NULL,
    other_fid INTEGER NOT NULL,
    type      INTEGER NOT NULL DEFAULT 0,
    added     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS file_relation_fid ON file_relation(fid);

-- Legacy "genres" (AniDB categories of the time) and modern tags share this table.
CREATE TABLE IF NOT EXISTS genre (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS anime_genre (
    id       INTEGER PRIMARY KEY,
    aid      INTEGER NOT NULL,
    genre_id INTEGER NOT NULL,
    weight   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS anime_genre_aid ON anime_genre(aid);
CREATE INDEX IF NOT EXISTS anime_genre_gid ON anime_genre(genre_id);

CREATE TABLE IF NOT EXISTS anime_relation (
    id          INTEGER PRIMARY KEY,
    aid         INTEGER NOT NULL,
    related_aid INTEGER NOT NULL,
    type        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS anime_relation_aid ON anime_relation(aid);

CREATE TABLE IF NOT EXISTS review (
    id          INTEGER PRIMARY KEY,
    aid         INTEGER NOT NULL,
    uid         INTEGER NOT NULL DEFAULT 0,
    uname       TEXT,
    vote        INTEGER NOT NULL DEFAULT 0,
    vote_anim   INTEGER NOT NULL DEFAULT 0,
    vote_sound  INTEGER NOT NULL DEFAULT 0,
    vote_story  INTEGER NOT NULL DEFAULT 0,
    vote_char   INTEGER NOT NULL DEFAULT 0,
    vote_value  INTEGER NOT NULL DEFAULT 0,
    vote_enjoy  INTEGER NOT NULL DEFAULT 0,
    rating      INTEGER NOT NULL DEFAULT 0,
    votes       INTEGER NOT NULL DEFAULT 0,
    body        TEXT,
    added       INTEGER NOT NULL DEFAULT 0,
    updated     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS review_aid ON review(aid);

-- state: 0 unknown, 1 on HDD (internal), 2 on CD (external), 3 deleted, 4 remote/shared.
CREATE TABLE IF NOT EXISTS mylist (
    lid       INTEGER PRIMARY KEY,
    fid       INTEGER NOT NULL,
    eid       INTEGER NOT NULL DEFAULT 0,
    aid       INTEGER NOT NULL DEFAULT 0,
    gid       INTEGER NOT NULL DEFAULT 0,
    date      INTEGER NOT NULL DEFAULT 0,
    state     INTEGER NOT NULL DEFAULT 0,
    viewed    INTEGER NOT NULL DEFAULT 0,
    viewdate  INTEGER NOT NULL DEFAULT 0,
    storage   TEXT,
    source    TEXT,
    other     TEXT,
    filestate INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS mylist_fid ON mylist(fid);
CREATE INDEX IF NOT EXISTS mylist_aid ON mylist(aid);
CREATE INDEX IF NOT EXISTS mylist_eid ON mylist(eid);

-- Hashes of local files (legacy local_hashes.data).
CREATE TABLE IF NOT EXISTS local_hash (
    id               INTEGER PRIMARY KEY,
    ed2k             TEXT    NOT NULL,
    size             INTEGER NOT NULL,
    crc32            TEXT,
    md5              TEXT,
    sha1             TEXT,
    fps              REAL,
    length           REAL,
    video_fourcc     TEXT,
    video_compression TEXT,
    video_codec      TEXT,
    video_height     INTEGER,
    video_width      INTEGER,
    video_bitrate    REAL,
    video_vbr        INTEGER,
    audio_ident      INTEGER,
    audio_codec      TEXT,
    audio_samplerate INTEGER,
    audio_bitrate    REAL,
    audio_vbr        INTEGER,
    UNIQUE (ed2k, size)
);

-- Hashed files on disk (legacy local_files.data).
CREATE TABLE IF NOT EXISTS local_file (
    id       INTEGER PRIMARY KEY,
    hash_id  INTEGER NOT NULL,
    path     TEXT    NOT NULL,
    mtime    INTEGER NOT NULL DEFAULT 0,  -- unix time of last modification when hashed
    added    INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX IF NOT EXISTS local_file_path ON local_file(path);
CREATE INDEX IF NOT EXISTS local_file_hash ON local_file(hash_id);

-- Pending AniDB commands (mylist edits etc.), persisted so offline work survives restarts.
-- AUTOINCREMENT: ids are never reused, so an acknowledgement for a command that was
-- superseded (or cleared) while in flight can never delete a newer command.
-- blocked: permanent failure or exhausted attempts; kept (and shown) until the user retries.
-- lid: mylist entry an add/edit/delete command targets, for pending-intent lookups.
-- Existing databases are rebuilt into this shape by `Db::migrate` before this script runs.
CREATE TABLE IF NOT EXISTS command_queue (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    created   INTEGER NOT NULL,
    priority  INTEGER NOT NULL DEFAULT 1,
    payload   TEXT    NOT NULL,  -- JSON encoded command
    tries     INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    blocked   INTEGER NOT NULL DEFAULT 0,
    lid       INTEGER
);
CREATE INDEX IF NOT EXISTS command_queue_order ON command_queue(blocked, priority, id);
CREATE INDEX IF NOT EXISTS command_queue_lid ON command_queue(lid);
CREATE INDEX IF NOT EXISTS command_queue_payload ON command_queue(payload);

-- Last time a request for one record was sent to AniDB, saved BEFORE sending and whether or not
-- it succeeded, so a failure, crash or restart cannot allow a repeat inside the minimum age.
-- kind: 'anime' (HTTP record) or 'file' (UDP FILE by id); id: aid or fid; attempted: unix time.
-- `anime.fetched` and `meta.titles_dump_date` keep recording only successes.
CREATE TABLE IF NOT EXISTS request_attempt (
    kind      TEXT    NOT NULL,
    id        INTEGER NOT NULL,
    attempted INTEGER NOT NULL,
    PRIMARY KEY (kind, id)
);
