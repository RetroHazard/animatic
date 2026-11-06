These pages cover backups, settings recovery, synchronization failures, filesystem problems
and privacy-safe support: this index, [Backups and recovery](Backups-and-Recovery.md),
[Network and sync problems](Network-and-Sync-Problems.md),
[File and import problems](File-and-Import-Problems.md) and
[Reporting problems](Reporting-Problems.md). They describe what the code in this repository does. Message texts
are quoted from the source so you can recognize them; wording in your build can differ if the
source has changed since.

## What Animatic does and does not do

Know the limits before you rely on a recovery path:

- **Backups are manual and there is no repair.** **File → Back up library…**, **File → Restore
  library…**, **Tools → Check library** and **Tools → Compact library…** exist (see
  [Back up and restore inside Animatic](Backups-and-Recovery.md#back-up-and-restore-inside-animatic)),
  but nothing backs the database up automatically, there is no rotation and no repair command
  in the application or the command line. A library backup does not include the settings. The
  only automatic copy is the single settings backup (`settings.json.bak`), described under
  [Settings problems](Backups-and-Recovery.md#settings-problems).
- **No checksums or verification of copies.** Nothing records or checks a checksum of your
  database, settings or any backup you make.
- **Credentials.** The password, API key and AVDump key are kept in the operating-system keystore
  when one is available; if Options shows the settings-file notice, the settings file and its
  backup hold them in plain text instead. The database and backups are not encrypted.
- **Personal data in the database.** The SQLite library holds your viewing history, file
  paths, hashes and queued edits.
- **No media deletion, no trash.** Animatic never deletes a media file on its own account, and
  the interface has no recycle-bin or trash workflow. A rename moves a file inside the
  filesystem; when the destination is on another drive it copies the file, verifies the copy
  and only then removes the original, which is the one case where a file is deleted (see
  [Rename problems](File-and-Import-Problems.md#rename-problems)); there is no undo. The only
  folders Animatic removes are completely empty ones that a rename emptied inside a managed folder
  whose **Remove emptied folders** is on (off by default).
- **No direct account download.** Animatic does not fetch your complete AniDB MyList entry by
  entry; you request a MyList export from AniDB and import the file you download (see
  [The library is missing entries](Network-and-Sync-Problems.md#the-library-is-missing-entries)).
- **No scheduled or automatic work.** Folder scans run only from **Tools → Scan managed
  folders**, the queue runs from **Tools → Run command queue** or **Synchronize now** (or by
  itself only if you switch on **Run the command queue automatically**, off by default, see
  [Automatic runs](AniDB-Synchronization.md#automatic-runs-and-the-anidb-session)),
  and renaming is automatic only in a managed folder you set to **Auto rename: On**. The one unasked action is the optional startup
  *check* for a newer Animatic release on GitHub (`updates.auto_check`); it downloads and
  installs nothing. See [Update problems](Network-and-Sync-Problems.md#update-problems).
- **No logging backend.** Diagnostics are the in-memory **Log** page and the status bar. See
  [Privacy-safe support](Reporting-Problems.md).

Evidence limits: Linux runtime behavior and production AniDB behavior have not been exercised
by the project's recorded verification. Steps in these pages that refer to Linux paths or to AniDB's
replies follow the source code, not a recorded run.

## Find the right section

| Symptom | Go to |
|---|---|
| "Animatic could not start (database: …, settings: …)" | [Startup problems](#startup-problems) |
| "Settings recovered" window, or an error naming `settings.json` and its backup | [Settings problems](Backups-and-Recovery.md#settings-problems) |
| The library looks empty after changing how you start Animatic | [Which files is Animatic using?](#which-files-is-animatic-using) |
| I want a safe copy of my data | [Backups and restore](Backups-and-Recovery.md#backups-and-restore) |
| "AniDB sync paused until …" | [Cooldowns and bans](Network-and-Sync-Problems.md#cooldowns-and-bans) |
| Red **N blocked** counter, **Blocked** rows, **Retry failed** | [Queue states](Network-and-Sync-Problems.md#queue-states-blocked-deferred-and-unsaved) |
| "NOT saved yet", "database is busy" | [Database busy and unsaved commands](Network-and-Sync-Problems.md#database-busy-and-unsaved-commands) |
| "login failed", "client version outdated", "no AniDB client name configured" | [Credentials and client registration](Network-and-Sync-Problems.md#credentials-and-client-registration) |
| Timeouts, "cannot resolve", "no IPv4 address" | [Network and firewall](Network-and-Sync-Problems.md#network-and-firewall) |
| Files not hashed or scan hashes nothing | [Scans and managed folders](File-and-Import-Problems.md#scans-and-managed-folders) |
| Records disappeared after a scan; a drive was unplugged | [Offline drives and automatic removal](File-and-Import-Problems.md#offline-drives-and-automatic-removal) |
| Hashing is slow or was cancelled | [Hashing large files](File-and-Import-Problems.md#hashing-large-files) |
| "Rename failed", "Rename blocked", "Rename not possible" | [Rename problems](File-and-Import-Problems.md#rename-problems) |
| Import finished with fewer collections than expected | [AOM import problems](File-and-Import-Problems.md#aom-import-problems) |
| Disk full, leftover temporary files | [Disk space, temporary files and leftovers](Backups-and-Recovery.md#disk-space-temporary-files-and-leftovers) |
| Preparing a bug report | [Privacy-safe support](Reporting-Problems.md) |
| Update check, download, install or restart failed; Animatic will not start after an update | [Update problems](Network-and-Sync-Problems.md#update-problems) |

## Which files is Animatic using?

Animatic stores a SQLite database and a JSON settings file. Which ones it opens depends on how
it was started:

| Setting | Highest → lowest priority |
|---|---|
| Database | `--database FILE` (also `-d`, `/database`), then `ANIMATIC_DB`, then the default |
| Settings | `ANIMATIC_SETTINGS`, then the default (there is no command-line option for settings) |

Defaults, from the `directories` crate with qualifier `net`, organization `AniDB` and
application `Animatic` (see [Data locations](Data-Locations.md#where-data-lives)):

| | Windows | Linux |
|---|---|---|
| Settings | `%APPDATA%\AniDB\Animatic\config\settings.json` | `$XDG_CONFIG_HOME/animatic/settings.json`, else `~/.config/animatic/settings.json` |
| Database | `%APPDATA%\AniDB\Animatic\data\animatic.sqlite3` | `$XDG_DATA_HOME/animatic/animatic.sqlite3`, else `~/.local/share/animatic/animatic.sqlite3` |

On Windows these are in the **Roaming** application-data folder. The Linux paths follow the
crate's source; Animatic itself has not been recorded running on Linux.

Files you may find next to them:

| File | What it is |
|---|---|
| `animatic.sqlite3-wal`, `animatic.sqlite3-shm` | SQLite write-ahead-log companions. They exist while the database is open and may remain after exit |
| `settings.json.bak` | Copy of the previous valid settings, made on a save |
| `settings.json.tmp`, `settings.json.bak.tmp` | Transient files during a save |
| `animatic-legacy-import-*/` (system temporary folder) | Decrypted intermediates during an AOM databank import; a marker file inside identifies it, and Animatic offers to remove leftovers at the next start |

The backup and temporary names replace the settings file's extension: for
`ANIMATIC_SETTINGS=/x/custom.cfg` they are `/x/custom.json.bak` and `/x/custom.json.tmp`.
The database companions append `-wal` and `-shm` to the full database file name.

**The library looks empty or the settings look reset.** The most common cause is a different
start method: a shortcut that does not set `ANIMATIC_DB`/`ANIMATIC_SETTINGS` or `--database`
opens the default locations, while your usual session may use overrides (the environment
variables apply to every launch in the shell or session that has them). Compare how you
started, close Animatic, and start it again with the same overrides. The paths are printed in
the startup failure message; nothing in the normal window shows them. Starting with a path
that does not exist creates a fresh empty database there, so an empty library is not proof
that your data is gone.

A bare, non-option argument is a media file to hash at startup, and an unknown option
beginning with `-` is rejected with `unknown option: …`. `--help` prints one line and
creates no files.

## Startup problems

Animatic is built as a console-subsystem program on Windows (no GUI-subsystem attribute in
`main.rs`), and startup failures are returned from `main`, so they are printed to standard
error. If double-clicking appears to do nothing, run it from a terminal to read the message.

| Message | Cause and action |
|---|---|
| `Animatic could not start (database: <path>, settings: <path>)` followed by a cause | Startup opens the database and loads settings. Read the cause line: a SQLite error means the database cannot be opened or created; a `settings:` error is covered above |
| `I/O error: <os error>` | The database's parent folder could not be created (permissions, a file in the way, read-only or full disk). Fix it or choose another location with `--database` |
| `database error: unable to open database file…` | The path is not writable, the disk is read-only, or the path is a directory. Fix permissions or choose another location with `--database`. SQLite's exact wording can include more text |
| `database error: database is locked…` | Another process holds the database longer than the 30 s busy timeout (a second Animatic, a backup program, a sync client, antivirus). Close it and retry |
| `database error: database disk image is malformed` / `file is not a database` | The file is damaged or is not an Animatic database. Restore a backup ([Damaged database](Backups-and-Recovery.md#damaged-database)) |
| `unable to determine the default application directory; use --database and ANIMATIC_SETTINGS` | The platform did not provide a home or profile directory. Set both overrides |
| `--database requires a file name` | The option was last on the command line with no value |
| `unknown option: <text>` | An argument starting with `-` that Animatic does not know (`/utf8` and `/ucs2` are accepted and ignored) |
| `the window system failed: <error>` | The windowing or graphics layer could not start. This is outside Animatic's data; check the desktop session, display variables and graphics drivers. The project has not recorded Linux window-system behavior |

Permission checklist for the folders involved:

1. The database folder must allow creating and writing files (the database, `-wal` and `-shm`
   are created there); the system temporary folder must allow creating the import's work folder.
2. The settings folder must allow creating `settings.json.tmp`, `settings.json.bak.tmp`,
   `settings.json.bak` and `settings.json`, and renaming over existing files.
3. Security software with folder protection can block writes to profile folders; allow
   Animatic or choose a folder it permits with the overrides.
4. A read-only settings location lets Animatic load but never persist changes. Because the
   exit save is silent, you only learn about it from **Save options** (`Could not save
   settings: …`).

Startup creates the database and its parent folder immediately; the settings folder is created
only when settings are first saved.

## Index of recurring messages

| Where | Text | Section |
|---|---|---|
| Startup | `Animatic could not start (database: …, settings: …)` | [Startup problems](#startup-problems) |
| Window | **Settings recovered** | [The recovery window](Backups-and-Recovery.md#the-recovery-window) |
| Status bar / Queue | `AniDB sync paused until …` | [Cooldowns and bans](Network-and-Sync-Problems.md#cooldowns-and-bans) |
| Log | `Command blocked: …` | [Queue states](Network-and-Sync-Problems.md#queue-states-blocked-deferred-and-unsaved) |
| Log | `Database busy: queued command is NOT saved yet, retrying` | [Database busy](Network-and-Sync-Problems.md#database-busy-and-unsaved-commands) |
| Log | `Offline mode is enabled; commands remain queued` | [Credentials](Network-and-Sync-Problems.md#credentials-and-client-registration) |
| Log | `Rename failed: …` | [Rename problems](File-and-Import-Problems.md#rename-problems) |
| Log | `Legacy import failed: …` | [AOM import problems](File-and-Import-Problems.md#aom-import-problems) |
