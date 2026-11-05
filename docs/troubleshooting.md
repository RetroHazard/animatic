# Troubleshooting

This guide covers backups, settings recovery, synchronization failures, filesystem problems
and privacy-safe support. It describes what the code in this repository does. Message texts
are quoted from the source so you can recognize them; wording in your build can differ if the
source has changed since.

## What Animatic does and does not do

Know the limits before you rely on a recovery path:

- **No backup or repair tooling.** There is no backup, restore, integrity-check or repair
  command in the application or the command line. There is no automatic database backup and
  no rotation. The only automatic copy is the single settings backup (`settings.json.bak`),
  described under [Settings problems](#settings-problems).
- **No checksums or verification of copies.** Nothing records or checks a checksum of your
  database, settings or any backup you make.
- **Plain-text secrets.** The settings file and its backup hold your AniDB password, API key
  and AVDump key in plain text. Nothing is encrypted at rest.
- **Personal data in the database.** The SQLite library holds your viewing history, file
  paths, hashes and queued edits.
- **No media deletion, no trash.** Animatic never deletes media files, and the interface has
  no recycle-bin or trash workflow. A rename moves a file inside the filesystem (see
  [Rename problems](#rename-problems)); there is no undo.
- **No full-account import.** Animatic does not download your complete AniDB MyList (see
  [The library is missing entries](#the-library-is-missing-entries)).
- **No scheduled or automatic work.** Folder scans run only from **Tools → Scan managed
  folders**, the queue runs only from **Tools → Run command queue** or **Synchronize now**,
  and renaming is never automatic. The one automatic action is the optional startup
  *check* for a newer Animatic release on GitHub (`updates.auto_check`); it downloads and
  installs nothing. See [Update problems](#update-problems).
- **No logging backend.** Diagnostics are the in-memory **Log** page and the status bar. See
  [Privacy-safe support](#privacy-safe-support-and-bug-reports).

Evidence limits: Linux runtime behavior and production AniDB behavior have not been exercised
by the project's recorded verification. Steps below that refer to Linux paths or to AniDB's
replies follow the source code, not a recorded run.

## Find the right section

| Symptom | Go to |
|---|---|
| "Animatic could not start (database: …, settings: …)" | [Startup problems](#startup-problems) |
| "Settings recovered" window, or an error naming `settings.json` and its backup | [Settings problems](#settings-problems) |
| The library looks empty after changing how you start Animatic | [Which files is Animatic using?](#which-files-is-animatic-using) |
| I want a safe copy of my data | [Backups and restore](#backups-and-restore) |
| "AniDB sync paused until …" | [Cooldowns and bans](#cooldowns-and-bans) |
| Red **N blocked** counter, **Blocked** rows, **Retry failed** | [Queue states](#queue-states-blocked-deferred-and-unsaved) |
| "NOT saved yet", "database is busy" | [Database busy and unsaved commands](#database-busy-and-unsaved-commands) |
| "login failed", "client version outdated", "no AniDB client name configured" | [Credentials and client registration](#credentials-and-client-registration) |
| Timeouts, "cannot resolve", "no IPv4 address" | [Network and firewall](#network-and-firewall) |
| Files not hashed or scan hashes nothing | [Scans and managed folders](#scans-and-managed-folders) |
| Records disappeared after a scan; a drive was unplugged | [Offline drives and automatic removal](#offline-drives-and-automatic-removal) |
| Hashing is slow or was cancelled | [Hashing large files](#hashing-large-files) |
| "Rename failed", "Rename blocked", "Rename not possible" | [Rename problems](#rename-problems) |
| Import finished with fewer collections than expected | [AOM import problems](#aom-import-problems) |
| Disk full, leftover temporary files | [Disk space, temporary files and leftovers](#disk-space-temporary-files-and-leftovers) |
| Preparing a bug report | [Privacy-safe support](#privacy-safe-support-and-bug-reports) |
| Update check, download, install or restart failed; Animatic will not start after an update | [Update problems](#update-problems) |

## Which files is Animatic using?

Animatic stores a SQLite database and a JSON settings file. Which ones it opens depends on how
it was started:

| Setting | Highest → lowest priority |
|---|---|
| Database | `--database FILE` (also `-d`, `/database`), then `ANIMATIC_DB`, then the default |
| Settings | `ANIMATIC_SETTINGS`, then the default (there is no command-line option for settings) |

Defaults, from the `directories` crate with qualifier `net`, organization `AniDB` and
application `Animatic` (see [Getting started](getting-started.md#where-data-lives)):

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
| `import-temp/` (in the database's folder) | Decrypted intermediates during an AOM databank import |

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

## Backups and restore

### What to back up and why it is sensitive

| Data | Files | Sensitivity |
|---|---|---|
| Library and queue | `animatic.sqlite3` plus its `-wal` and `-shm` files if present | Viewing history, file paths, hashes, queued edits |
| Settings | `settings.json` and `settings.json.bak` | **Plain-text AniDB password, API key and AVDump key** |

Treat every backup of either as a secret. Store it somewhere only you can read. Animatic does
not encrypt anything; if you need an encrypted backup, use your own encryption tooling.
`settings.json.bak` holds the **previous** valid settings, so it can still contain a password
or key you have since changed or removed.

### A consistent copy

The database runs in SQLite WAL mode, so committed changes can live in the `-wal` file until
SQLite folds them back into the main file. Copying only `animatic.sqlite3` while Animatic is
running, or after it exited without a clean shutdown, can produce a copy that is missing
recent changes or is inconsistent. Do not copy an open database "blind".

1. **Finish or cancel background work.** Watch the status bar. Wait for a running job to
   finish or press **Cancel**. Do not quit while it shows **N commands NOT saved yet**
   (see [Database busy and unsaved commands](#database-busy-and-unsaved-commands)).
2. **Close Animatic** with **File → Quit** and make sure the process has ended. Quitting
   cancels a running job, drains accepted statistics exports and writes settings (unless
   the [recovery guard](#settings-problems) holds).
3. **Copy the files as one set into a new folder**, for example one dated folder per backup:
   `animatic.sqlite3` and, **if they exist, `animatic.sqlite3-wal` and `animatic.sqlite3-shm`**,
   plus `settings.json` and `settings.json.bak`. After a clean close the companions are often
   absent; after an unclean exit they can hold committed data, so copy whichever exist and
   keep them together with the database file. Never delete a `-wal` file to "tidy up" after a
   crash.
4. **Keep the copy outside the repository and outside synchronized folders** (see
   [Cloud sync, roaming profiles and network drives](#cloud-sync-roaming-profiles-and-network-drives)).
5. **Verify without touching the backup.** Copy the backup set to a scratch folder and start
   Animatic against that copy, never against the backup itself (opening a database creates
   WAL files and applies idempotent schema upgrades):

   ```sh
   ANIMATIC_DB=/scratch/check/animatic.sqlite3 \
   ANIMATIC_SETTINGS=/scratch/check/settings.json \
   animatic
   ```

   ```powershell
   $savedDb = $env:ANIMATIC_DB
   $savedSettings = $env:ANIMATIC_SETTINGS
   try {
       $env:ANIMATIC_DB = "$env:TEMP\check\animatic.sqlite3"
       $env:ANIMATIC_SETTINGS = "$env:TEMP\check\settings.json"
       .\animatic.exe   # returns when you close the window
   } finally {
       $env:ANIMATIC_DB = $savedDb
       $env:ANIMATIC_SETTINGS = $savedSettings
   }
   ```

   Use a copy of the settings with **Work offline** ticked, so a check can never send
   anything to AniDB, then look at the **Overview**, **My list**, **Hashed files** and
   **Queue** pages. The SQLite command-line shell is not shipped with Animatic; if you have
   it, its `PRAGMA integrity_check;` or `.backup` command on a closed copy is an additional
   option, but that is SQLite's own feature and the project has not recorded using it.

Take a backup **before** anything that rewrites a lot of data: importing an AOM databank,
running a scan with `auto_remove`, choosing **Clear queue**, upgrading to a build with schema
changes (upgrades are applied when the database is opened and no downgrade is provided or
tested), or editing the settings JSON by hand.

### Restore

1. Close Animatic.
2. Move the current database, its `-wal` and `-shm` files and the current settings files into
   a "before restore" folder. Do not delete them until the restored library is confirmed.
3. Make sure **no stale `-wal` or `-shm` from the old database remains** in the target folder.
   A write-ahead log belongs to the database file it was written for; leaving one beside a
   different database file can apply unrelated pages.
4. Copy the backed-up database file (and its `-wal`/`-shm`, if the backup set has them) and
   the settings files into place under their original names.
5. Start Animatic. If the settings are not offline, tick **Work offline** first and check the
   **Queue** before using the network: a restored queue can contain commands you had already
   run on a later date.

If a restored settings file is invalid, follow [Settings problems](#settings-problems).

### Damaged database

If startup fails with a SQLite message such as `database disk image is malformed` or `file is
not a database`, Animatic has no repair path. Restore a backup. SQLite's own recovery tools
may salvage rows from a damaged file; they are not part of Animatic, and the project has not
verified their output against its schema, so work on a copy and re-check the result.

### Cloud sync, roaming profiles and network drives

- The Windows default locations are in the **Roaming** profile. If your environment
  synchronizes or roams that folder, the database and its WAL files can be uploaded or copied
  piecemeal while Animatic is running.
- SQLite's WAL mode needs the database on a local filesystem. Do not place the database on a
  network share, and avoid cloud-synchronized folders (OneDrive, Dropbox and similar) because
  a sync client can copy the main file without its `-wal`.
- Prefer a fixed local disk, for example by setting `ANIMATIC_DB` (or `--database`) to a path
  outside synchronized folders on every launch.
- Do not synchronize the settings file between machines: it contains credentials, and two
  machines saving at once will overwrite each other (the save is atomic per machine, with no
  merge).
- Do not run two Animatic processes on the same settings file either. Saves share the same
  `settings.json.tmp` and `settings.json.bak.tmp` names, so concurrent saves can fail or
  cross-write; the outcome is not a predictable "last writer wins".
- Do not run two Animatic instances against the same database. The source contains no
  application-level instance lock; SQLite's file locking and busy timeouts are the only
  protection, and the second instance will compete for the write lock.

## Settings problems

For field-level behavior see the [Settings reference](settings-reference.md#recovery-matrix).

### How loading and saving work

- **Load** reads `settings.json`. If it is valid, that is the result. If it is missing or
  invalid, it tries `settings.json.bak`. If that is valid, Animatic loads it, forces **offline
  mode**, shows the **Settings recovered** window, and changes no file. If both are missing,
  defaults are used. If the primary cannot be read (permissions, a folder where the file
  should be, an I/O error), Animatic stops with an error naming the file and **does not fall
  back to the backup**. If the primary is invalid and the backup is invalid or unreadable,
  Animatic stops with an error naming both files.
- **Valid** means a JSON **object** at the root that deserializes into the settings. A
  wrong type for any single value, a truncated file, or a root of `[]`, `null`, `42`,
  `"text"` or `true` makes the whole file invalid. Missing keys are fine and take defaults.
- **Save** writes `settings.json.tmp`, flushes it, copies the previous primary to
  `settings.json.bak` through `settings.json.bak.tmp` **only if that previous primary is still
  valid**, then renames the temporary file over `settings.json`. An invalid primary never
  replaces a good backup. A failed save removes the temporary file and leaves the old primary
  in place. A failure while rotating the backup is ignored, so the primary can be replaced
  while `settings.json.bak` is stale or absent.
- **When saves happen:** **Save options**, applying reviewed AOM settings, **Dismiss first-run
  card**, and closing the application. Edits in **Options** take effect in memory at once and
  are written when the application exits even if you never click **Save options** (unless
  the recovery guard holds), so a mistaken edit does not need a click to become permanent.
  A failure on exit is silent.

Consequences of the one-generation backup: the backup is the state before your last save, so
recovered settings can be older than you expect, and a wrong but syntactically valid edit
becomes the backup on the next save, which can be the exit save. Make your own copies before
manual edits.

### Error messages

Messages report only the failure class and a position, never file contents (so a bad
credential line is not echoed). The classes are `invalid JSON syntax`, `truncated JSON`,
`unexpected field type or value` and `read error`, each with `at line N column M`.

| Message | Meaning and action |
|---|---|
| `cannot read settings file <path>: <os error>; the file was not modified` | The primary exists but cannot be read. Fix permissions, or remove a folder or other object occupying the name. The backup is not consulted. |
| `settings file <path> is not valid (<class> at line N column M) and backup <path> does not exist; neither file was modified. Repair or remove them to start with default settings` | Open the file in an editor, go to the reported line and column, and correct it (or restore it from your own backup). To start over, move both files into a safe folder and start Animatic. |
| `… and backup <path> is not valid (…)` or `… cannot be read (…)` | Both copies are unusable. Repair one, restore from your own backup, or move both aside to start with defaults. |
| `Could not save settings: <error>` | The save failed (see below). The previous primary is still in place. |

Truncated JSON (an empty file included) is reported as `truncated JSON`. A root array or other
non-object is reported as an unexpected value near line 1.

### The recovery window

The **Settings recovered** window says that the settings file could not be used, that the
backup was loaded, that Animatic is offline, and that the original file has not been changed.
Do the following:

1. Dismiss it with **OK** and look at **Tools → Options…**. The values come from the backup
   and can be one save behind.
2. Decide whether to keep them. **Closing Animatic does not write settings** while the
   recovery guard holds, and **Dismiss first-run card** does not write either. Your changes
   in this session exist only in memory.
3. To adopt the recovered settings, choose **Save options**. That writes them (including
   `offline: true`) over the unreadable primary. The invalid old primary does not replace the
   backup. Afterwards untick **Work offline** only when you are ready for network use.
4. To keep the original file for inspection, copy it somewhere safe **before** saving; saving
   replaces it.

Applying reviewed AOM settings counts as an explicit save and also releases the guard.

### Save failures and leftover files

`Could not save settings: …` is shown with the operating-system error. Typical causes:

- the folder is read-only or the disk is full (see
  [Disk space, temporary files and leftovers](#disk-space-temporary-files-and-leftovers));
- another program holds the file open (an editor, an antivirus scanner, a sync client) and the
  final rename fails;
- a folder or file occupies `settings.json.tmp` (a directory in that place makes every save
  fail until you remove it).

Remove the obstacle and use **Save options** again; the exit save does not report errors, so
use **Save options** when you need to be sure. A `settings.json.tmp` or
`settings.json.bak.tmp` left by a crash or power loss is transient: with Animatic closed and a
valid `settings.json` in place, delete them. Do not rename a leftover temporary file into
place as a recovery step; it may be incomplete.

### Resetting settings deliberately

Move `settings.json` and `settings.json.bak` into a backup folder (a copy that still holds
credentials) and start Animatic: with neither file present it uses defaults and creates no
file until you save. A fresh configuration has `offline` set to `false` and no client
name, so the first thing to do is open **Tools → Options…**, tick **Work offline**, and save.

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
| `database error: database disk image is malformed` / `file is not a database` | The file is damaged or is not an Animatic database. Restore a backup ([Damaged database](#damaged-database)) |
| `unable to determine the default application directory; use --database and ANIMATIC_SETTINGS` | The platform did not provide a home or profile directory. Set both overrides |
| `--database requires a file name` | The option was last on the command line with no value |
| `unknown option: <text>` | An argument starting with `-` that Animatic does not know (`/utf8` and `/ucs2` are accepted and ignored) |
| `the window system failed: <error>` | The windowing or graphics layer could not start. This is outside Animatic's data; check the desktop session, display variables and graphics drivers. The project has not recorded Linux window-system behavior |

Permission checklist for the folders involved:

1. The database folder must allow creating and writing files (the database, `-wal`, `-shm`
   and the `import-temp` folder are created there).
2. The settings folder must allow creating `settings.json.tmp`, `settings.json.bak.tmp`,
   `settings.json.bak` and `settings.json`, and renaming over existing files.
3. Security software with folder protection can block writes to profile folders; allow
   Animatic or choose a folder it permits with the overrides.
4. A read-only settings location lets Animatic load but never persist changes. Because the
   exit save is silent, you only learn about it from **Save options** (`Could not save
   settings: …`).

Startup creates the database and its parent folder immediately; the settings folder is created
only when settings are first saved.

## Update problems

How updating works and what it changes: [User guide](user-guide.md#updating-animatic). The
update path is separate from AniDB: an update failure never touches the command queue or an
AniDB cooldown, and an AniDB cooldown never blocks it.

| Symptom | Cause and action |
|---|---|
| No check happens at startup | By design when `updates.auto_check` is off, *Work offline* is on, or settings were recovered from a backup. **Help → Check for updates…** still works |
| Help → Check for updates… is disabled or the platform is reported unsupported | Only Windows and Linux on x86_64 are supported. Download releases manually elsewhere (none are built) |
| The check fails, or reports a rate limit | Unauthenticated GitHub API requests are limited per IP address, and a firewall, proxy or offline machine also fails. Nothing is retried automatically; try again later. A manual check always shows the error, a startup check writes it only to the **Log** |
| The check reports "not found" or the release feed as unavailable | GitHub answers HTTP 404 when the release data is not visible to an anonymous client: the repository's releases are not publicly readable, or nothing is published. This is an error, never "up to date". Animatic has no token or sign-in setting, so until the releases are publicly readable, update manually ([Releases](releases.md#install-a-release)). This guide does not state whether the repository is currently public |
| "Animatic is up to date" although a newer tag exists on GitHub | Only published, non-draft, non-prerelease releases with a valid CalVer tag (no `-` suffix), and only when strictly newer than the running version, are offered; the platform archive and `SHA256SUMS` must be present and correctly named. Drafts, release candidates and older versions are never offered. If the newest stable release is incomplete the check reports an error instead of offering an older one |
| Download or verify fails (size, checksum or archive error) | Nothing was installed. The staged files are in a private temporary folder that is removed when the update is dropped or Animatic exits. Retry; if the checksum keeps failing, do not install and download manually from the [releases page](https://github.com/RetroHazard/animatic/releases) as described in [Releases](releases.md#install-a-release), and report it |
| Install or Restart now is disabled | They are blocked while a job is running, an AOM import is in progress, imported settings are waiting to be applied, or some changes are not yet saved to the database; Restart is also blocked while settings recovered from a backup are unsaved (use **Save options**). The reason is shown in the window. Finish or cancel that work first |
| Install fails with a permission or read-only error | The folder holding the executable must be writable by your account. There is no elevation. Update the copy the way it was installed (administrator copy, package manager) or move Animatic to a user-writable folder |
| Install refuses because the executable does not match | The running file is not the one Animatic started from (moved, replaced or launched through an unusual path). Restart Animatic normally and try again |
| `LICENSE-vVERSION` and `THIRD-PARTY-NOTICES-vVERSION.html` appear beside the executable | Expected after an install, even a failed one. They are the new release's license files, are never overwritten, and are harmless to keep. A different pre-existing file with the same name makes the install stop; inspect or rename it |
| An empty `.animatic-update.lock` file beside the executable, or a message that another install is running | Created by the install step; the OS holds an advisory lock on it only while an install runs. The empty file is harmless and may remain. The message means another Animatic process holds the lock: wait for it or close that instance |
| Dot-prefixed files next to the executable (`.animatic-recovery-*.exe`, `.animatic-restore-*`, or helper files from `self-replace` such as `.__selfdelete__.exe`) or a renamed old executable | `.animatic-recovery-*.exe` is the Windows copy of the previous executable made before the swap; it is removed after a successful install and **kept after any failed one**. Other files are left if the machine lost power or Animatic was killed mid-install. Animatic never deletes them automatically. Check which executable starts, and delete leftovers by hand only after confirming a working copy |
| The window says the installation did not finish and the program files may be in an unknown state | The swap itself failed. Animatic stops offering updates and does not restart by itself. Check that the executable still starts; the message names the recovery copy and, if the old file was not restored, which file to copy to which name. Otherwise install the release manually ([Releases](releases.md#install-a-release)). Restart Animatic by hand before trying again |
| Animatic does not start after an update | Download the archive for your platform from the releases page, verify `SHA256SUMS`, and run or copy the executable from it ([Releases](releases.md#install-a-release)). Your database and settings were not changed by the update |
| The documentation or notices beside the executable are older than the program | Expected: only the executable and versioned license files are updated. Replace the whole folder from a newly downloaded archive to refresh the bundled documents |
| Restart did not start a new window | The new process is started only after the old one exits, with only your `--database` path (and `ANIMATIC_SETTINGS` when it was set). Start Animatic yourself; nothing is lost because settings were saved before closing |

The check, download and install are bounded by byte limits and timeouts and run on their own
thread, so a slow or stuck GitHub connection never freezes the window; use **Cancel** during
a download, or close Animatic (which cancels it). Closing during the short install step waits for it.

## Disk space, temporary files and leftovers

| Operation | Where it needs space | Notes |
|---|---|---|
| Database growth and the WAL | The database folder | The `-wal` file grows during large writes and is folded back by SQLite later. A full disk produces a SQLite `database or disk is full` error; the transaction is rolled back |
| Settings save | The settings folder | A temporary file and the backup are written there |
| Anime titles download | The system temporary directory | The download is spooled into a private SQLite temporary database, then merged atomically. The download limits are 32 MiB compressed, 128 MiB decoded, 250,000 anime, 2,000,000 titles and 2,048 bytes per name. Keep comfortable free space on the volume that holds the temp directory |
| Update download | The system temporary directory (a private staging folder) and the executable's folder | Up to the archive (limit 256 MiB) plus the extracted executable (limit 512 MiB) are staged in the temporary directory; the install then needs room for a copy of the new executable beside the current one |
| AOM databank import | The folder `import-temp` beside the database | One decrypted collection at a time, roughly the size of the largest source `.data` file in addition to the database growth |

### Leftover files and what is safe to delete

Close Animatic first, confirm no import is running, and keep anything you are unsure about
until you have a verified backup.

- `import-temp/` (UI import) or `<output>.import-temp` (the example; for an output of
  `library.sqlite3` it is `library.import-temp`): holds **decrypted plaintext SQLite copies of
  your private databank collections** (`<name>.sqlite`, where `<name>` is a collection such as
  `animes` or `mylist`). Each plaintext file is removed after its collection is processed, and
  the (empty) folder is removed after a successful import. After a failure of any step
  (decryption, attaching, committing, detaching), a cancel or a crash, a partial or complete
  plaintext file can remain. The importer does not require the folder to be new or empty, so
  it may also hold files that are not Animatic's. **Look inside first**, delete only the
  `<collection>.sqlite` files you recognize as importer output, and remove the folder itself
  only when it is then empty. The original databank is never modified.
- `settings.json.tmp`, `settings.json.bak.tmp`: see
  [Save failures and leftover files](#save-failures-and-leftover-files).
- SQLite's anonymous temporary files (the title spool and sort temporaries) are removed by
  SQLite. After a crash a leftover can remain in the system temp directory (SQLite's temporary
  file names typically begin with `etilqs_`); delete such files only when no Animatic process
  is running.
- `-wal` and `-shm` beside the database: **not** leftovers. See [A consistent
  copy](#a-consistent-copy).

## Network and firewall

What the code uses (source: `animatic-anidb`):

| Purpose | Endpoint | Notes |
|---|---|---|
| UDP API | `api.anidb.net:9000` by default (**AniDB account** settings can change the server and local port) | IPv4 only. The client binds `0.0.0.0` on the configured local port (default 29456) and falls back to a random port if binding fails. Each exchange waits up to 20 s and a first timeout is retried once |
| HTTP API | `http://api.anidb.net:9001/httpapi` | Plain HTTP; requests are spaced at least 2 s apart; 60 s timeout |
| Titles dump | `https://anidb.net/api/anime-titles.xml.gz` | Public download, bounded as described above |
| Update check and download | `https://api.github.com/` and `https://github.com/` (plus the CDN hosts GitHub redirects asset downloads to), HTTPS only | Not AniDB: no flood spacing and no cooldown. Anonymous: sends your IP address and `User-Agent: Animatic/VERSION`, no credentials or library data. Bounded sizes and timeouts. Used only for update checks and downloads you start, plus the optional startup check |

The UDP client enforces AniDB packet spacing (2 s between packets for the first five, then
4 s, resetting after 10 idle minutes) and re-authenticates when a session expires. A long
queue therefore takes a long time; that is not a hang. The HTTP client is built with the
HTTP library's defaults and Animatic sets no proxy of its own; the UDP API cannot use an HTTP
proxy.

| Message | Cause and action |
|---|---|
| `network error: cannot resolve <server>: …` | DNS failure or a wrong server name in the account settings |
| `network error: no IPv4 address for <server>` | The name resolves to IPv6 only. The UDP client needs an IPv4 address |
| `network error: cannot bind UDP socket: …` | Neither the configured local port nor a random port could be opened. Check security software |
| `no reply from AniDB (timeout)` | Outbound UDP to the server is blocked or lost. Allow outbound UDP to the server's port, and replies back to the local port (NAT and router behavior is outside Animatic) |
| `HTTP error: HTTP status 5xx` | A server-side failure from the HTTP API; counted toward the command's retry budget |
| `AniDB unavailable, retry in N min` | The server asked you to wait (or the HTTP API returned status 429, treated as a 30-minute wait) |

Transient network failures never lose queued work: the command stays in the queue and syncing
pauses for 60 seconds (see [Cooldowns and bans](#cooldowns-and-bans)).

## Credentials and client registration

Both AniDB APIs require a registered **client name**; the project ships none and its tests use
only synthetic local fixtures. Credentials and the client name live in **Tools → Options… →
AniDB account**. The Options window has no client-version field; `anidb.client_version`
defaults to `1` and can be changed only by editing the JSON file; AOM settings import does not
set it (see the [Settings reference](settings-reference.md)). For registration rules see
[Getting started](getting-started.md#anidb-client-registration).

The queue never runs on its own. **Tools → Run command queue** and **Queue → Synchronize now**
take a snapshot of the **current in-memory options** (saved or not) at the moment you click.
With **Work offline** ticked they send nothing, log `Offline mode is enabled; commands remain
queued`, and leave the queue alone.

| Message (in the **Last error** column, the **Log** or the status bar) | Meaning | Effect | Action |
|---|---|---|---|
| `authentication: no AniDB client name configured` | The client name is empty | Fatal: **every** non-blocked command is blocked | Enter the registered client name, then **Retry failed** |
| `authentication: username and password required` | Missing user name or password | Fatal, as above | Enter them, then **Retry failed** |
| `authentication: login failed: wrong username or password` | Login code 500 | Fatal, as above | Correct the credentials, then **Retry failed** |
| `authentication: illegal input or access denied (is the client registered?)` | Login code 505 | Fatal, as above | Verify the client registration |
| `authentication: API key not defined in your AniDB profile` / `no such user` | Encrypted-session setup failed (codes 309, 394) | Fatal, as above | Fix or clear the API key; check the user name |
| `authentication: access denied` | A later command was refused (code 502) | Fatal, as above | Check the account |
| `client version outdated - check the AniDB client registration` | Code 503 | Fatal, as above | Correct the client name or version registration |
| `authentication: HTTP API rejected the client registration (…)` | The HTTP API refused the client | Fatal, as above | Verify the client name and version |

After a fatal credential or client error the status bar shows **N blocked** and the Log shows
`Command blocked: N queued commands blocked: <message>`. Correct the cause, optionally **Save
options**, then **Queue → Retry failed**, then **Synchronize now**. A wrong password that you
keep retrying will keep failing; stop and fix it first.

Settings credentials are stored in plain text. Do not paste them into bug reports; see
[Privacy-safe support](#privacy-safe-support-and-bug-reports).

## Cooldowns and bans

When AniDB refuses the client or the network fails, Animatic **stops the whole queue run** and
stores a cooldown so that it holds across restarts and new jobs:

- It is stored in the database's `meta` table (key `sync_cooldown`: an expiry time and the
  reason), so it belongs to **that database**. Another database or machine does not know about
  it; waiting still makes sense because the server-side condition remains.
- While it is active, **Run command queue** and **Synchronize now** are disabled (hover for the
  text) and the status bar and **Queue** page show `AniDB sync paused until HH:MM:SS: <reason>`
  in local time. A run started through another path logs `AniDB sync is paused for another …`
  and sends nothing.
- **Retry failed does not bypass a cooldown.** It only unblocks entries; the run still checks the
  stored cooldown first, and a stored value that cannot be read never locks syncing.
- There is no control to clear a cooldown, and editing the database to remove it is not
  supported. The cooldown exists to avoid immediate repeat requests after a refusal.

| Condition | Pause | What happens to the command |
|---|---|---|
| Connectivity failure: DNS, connect, TLS, timeout, no reply | 60 s | Stays queued, no attempt is counted |
| UDP server code 604 | 60 s | Stays queued, no attempt counted |
| UDP server codes 600, 601, 602 | 30 min | Stays queued; code 600 counts one attempt |
| `AniDB unavailable, retry in N min` | The duration given (HTTP 429: 30 min) | Stays queued |
| Ban: UDP codes 504, 510, 555; HTTP 403; HTTP `banned` / error 500 | 30 min (Animatic's conservative default when no duration is known) | Stays queued; a ban never blocks entries |
| HTTP status ≥ 500 or malformed server answers | 60 s | The command **counts an attempt**; after 5 counted attempts it is blocked |
| Local storage errors while applying a reply | 60 s | Counts an attempt |
| Code 598 (unknown command), 505 on a command (`illegal input: …`), unexpected reply codes, protocol errors, XML errors | none | That command is **blocked** and the run continues with the next |

Notes:

- The ban text may include the server's wording. If you see `client banned by AniDB: …`, stop
  starting more runs and wait.
- A run that finishes sends a logout. Cancel acts **between commands**: a command already in
  flight can take up to its timeout (20 s for UDP, 60 s for HTTP) to end. Closing the window
  does not wait for it.
- Durations are Animatic's own conservative choices unless AniDB supplied one; the project
  has no recorded production AniDB observation to cite for AniDB's actual ban lengths.

## Queue states: blocked, deferred and unsaved

The **Queue** page lists every saved command with its **Status** and **Last error**. There are
three different situations, and they are handled differently.

| State | Where you see it | Meaning | What to do |
|---|---|---|---|
| **Pending** | Status `Pending`, no retries | Waiting to run | Nothing; run the queue |
| **N retries** | Yellow status | Failed `N` times with errors that count toward the budget (at most 5) | Wait; it becomes **Blocked** at 5 |
| **Deferred** (not a separate label) | `Pending` or `N retries`, with a **Last error** such as a timeout or ban | Not attempted again because of connectivity or a cooldown, which do not count as attempts | Wait out the cooldown, fix connectivity, run again |
| **Blocked** | Red **Blocked**, red **N blocked** in the toolbar and status bar | A permanent failure, a protocol/format error, rejected credentials, or an unreadable stored command. It is skipped by runs and stays visible | Read **Last error**, fix the cause, then **Retry failed** |
| **blocked commands (refreshing)** | Red text | A newer blocked event arrived than the data on screen; the view is refreshing | Wait a moment |
| **NOT saved yet** | Yellow `N commands NOT saved yet` in the status bar | The command is only in memory | See below |

Rules that matter:

- **Retry failed** is enabled only when something is blocked. It unblocks and resets the
  attempt counter of every blocked command, merges multiple edits of the same MyList entry into
  the latest, and does not touch the cooldown. It does not clear **Last error** text; a stale
  message can remain until the command is retried or removed.
- **Clear queue** deletes **every** queued command, including unsynchronized MyList edits and
  blocked commands. It asks for no confirmation. It does not undo the optimistic local state
  those edits already applied, so the local view can then disagree with AniDB. It does not
  clear the cooldown. There is no per-command discard in the interface; to drop one command you
  must clear the whole queue, so make a backup first when other work is pending.
- A blocked MyList edit whose entry no longer exists on AniDB says `mylist entry N no longer
  exists on AniDB; the edit is kept (blocked): discard it from the queue or refresh the entry`.
  Retrying it fails again; only **Clear queue** discards it from the interface.
- An unreadable stored command is blocked with `unreadable queued command: …` rather than being
  deleted. Because Animatic cannot read it, such a row cannot overlay its pending intent onto
  values received from AniDB; readable queued edits are preserved over remote values.
- `file unknown to AniDB` after identification is a normal answer for a file AniDB does not
  know, not an error, and is not queued again.
- Command identifiers are never reused, so an older reply cannot erase newer intent.

### Database busy and unsaved commands

Two connections write to the database: the background worker and the interface. A long write
(for example an import, a title merge or a large registration) makes the other side wait.

| Side | Wait before giving up | What you see |
|---|---|---|
| Worker | 30 s | A failed job reports the database error |
| Interface | 1.5 s | The behavior below |

- **A MyList edit that cannot be saved** reports `<action>: the database is busy with a
  background write; nothing was changed, try again`. The edit is not applied; repeat it.
- **A queued command (identify, title download, refresh)** that hits a busy database is kept in
  memory: the Log says `Database busy: queued command is NOT saved yet, retrying` and the
  status bar shows `N commands NOT saved yet`. Animatic retries every half second and logs
  `Saved N previously unsaved commands` when it succeeds. At most 1000 are held; beyond that
  you get `NOT queued: <command> (database busy and 1000 commands are already unsaved)`.
- **Closing the window** makes one final attempt of up to 5 s. If the database is still busy,
  the unsaved commands are lost, and the only record is a `log` message that nothing prints
  (the application installs no logger). **Do not quit while the unsaved counter is non-zero.**
- Causes of long busy periods: a legacy import (each collection is one transaction), the
  titles merge, another program touching the files (backup tool, antivirus, sync client) or a
  second Animatic instance. Wait for the running job to finish or cancel it.
- `Unable to refresh library: …` means the background snapshot failed; a later refresh
  (**File → Refresh**) usually succeeds once the contention ends.

## The library is missing entries

- **A full-account bootstrap is not implemented.** Animatic never downloads your whole AniDB
  MyList. **Tools → Fetch mylist statistics** only fetches counts. **Tools → Download anime
  titles** fetches the public titles dump only. `--full-update` (command-line only, no menu
  item) queues the titles dump plus lookups for the files and anime your **hashed local files**
  already reference. Library content comes from an AOM import, from hashing and identifying
  files, from the anime detail window's **Refresh from AniDB**, and from your own edits.
- Not every hashed file is known to AniDB. A file AniDB does not know logs `file unknown to
  AniDB`.
- **Download anime titles** failures leave your existing titles and the "last updated"
  timestamp unchanged: the download is validated (XML, gzip end-of-stream and CRC, size and
  count limits) and merged in a single transaction only when complete. Errors you may see
  include `titles dump larger than the N byte download limit`, `… decompressed limit`,
  `unexpected data after the compressed titles dump`, `invalid compressed body: …` and `title
  longer than N bytes`. Format and size errors block the `TitlesDump` command; connectivity
  errors pause syncing instead (see [Cooldowns and bans](#cooldowns-and-bans)). After a block,
  use **Retry failed** once the cause is gone, or queue the download again from the Tools
  menu. A download is not resumable.
- Cached **MyList** displays include optimistic local state for queued edits; a blocked or
  cleared edit can leave the local view different from AniDB.
- A file lookup that **explicitly** reports "not in MyList" removes the local MyList rows for
  that file, unless a queued local edit for the entry exists. A malformed successful reply is
  a protocol error, never treated as absence. If an entry vanished after an online run,
  check the **Log** for the identification result and compare with AniDB's website before
  assuming data loss; a backup taken before the run is the way back.

## Scans and managed folders

A scan runs only when you choose **Tools → Scan managed folders** (it is disabled while an AOM
import or settings review is pending). It reads the managed-folder rules from the current
in-memory settings and plans three possible actions:

| Action | Condition |
|---|---|
| Hash | A file under a rule with **auto hash** whose extension is allowed and whose path is not yet in the library. If several rules cover the same path, the **first rule in list order** that takes part in discovery wins (longest-path matching applies to MyList defaults, the rename base path and removal, not to discovery) |
| Rehash | A known file under a rule with **auto rehash** whose modification time (in seconds) changed |
| Remove missing | A library path under a rule with **auto remove** that is affirmatively absent; see [Offline drives](#offline-drives-and-automatic-removal) |

If a scan hashes nothing:

1. **No rule has auto hash or auto rehash**, or its folder path is empty or not an existing
   directory. A rule with neither flag is skipped for hashing.
2. **Extensions:** scans always filter by the extension list in **Tools → Options… → Hashing**,
   whatever **Restrict file extensions** is set to. A file without an extension never matches
   and an empty list hashes nothing. Entries may include a leading dot and are matched
   case-insensitively. (Files passed on the command line are not filtered.)
3. **Hidden files** are skipped when the rule's **exclude hidden** is on, and names starting
   with `.` are hidden by this definition. Symbolic links are not followed. **Include
   subdirectories** controls depth: a non-recursive rule sees only its own folder.
4. Unreadable subfolders are skipped silently; they produce no hash actions.
5. Other settings in the rule (`auto_rename`, clear empty folders, exclude system files, the
   delay fields) are stored but not used by scans; nothing renames or deletes automatically.

Per-file hashing errors appear in the Log as `<path>: <error>` and the scan continues with the
next file. After a scan, each hashed file queues an **Identify** command when **Identify files
with AniDB after hashing** is on; with the default folder rule that command may also add the
file to MyList when the queue later runs online. Keep **Work offline** ticked until you have
reviewed the queue.

Moving or renaming files outside Animatic does not update library paths. The old record keeps
its path until a scan with **auto remove** drops it and **auto hash** adds the new location
(which hashes that file again), or until you add the file again with **File → Add files…**.

## Offline drives and automatic removal

Automatic removal only ever deletes **local inventory rows** (the path record and a stored hash
that no other path uses). It does not delete media, change MyList or touch AniDB. A removed
record means that file must be **hashed again** if you want it back, which re-reads the file.

A path is planned for removal only when all of these hold:

- a matching folder rule exists and has **auto remove** on (a non-matching path never falls
  back to the default rule, and a non-recursive rule does not cover its subfolders);
- looking up the file reports that it **does not exist** (a permission or I/O error preserves
  the record);
- the rule's root folder is **currently a directory** (an unmounted Windows drive letter or an
  unreachable network root fails this root check, so records are preserved).

Remaining risk to plan for: the root check only asks whether the folder exists. A folder that
**exists but is empty because its storage is not mounted** (a Linux mount point, a placeholder
for a network mount, a cloud "online-only" folder) looks like an available root that contains
no files, so every record under it can be removed by a scan. Likewise, storage can disconnect
between planning and execution; the check is a conservative plan, not an atomic guarantee.

Recommendations:

- Leave **auto remove** off for removable, network or cloud-backed folders.
- Back up the database before a scan with **auto remove** enabled, and check the drive is
  mounted and shows its files first.
- If records were removed because storage was absent, restore the database from a backup made
  before the scan, or remount the storage and let a scan hash the files again.

## Hashing large files

Hashing computes ED2K (the AniDB-compatible red variant), CRC32, MD5 and SHA-1 in **one pass**
over each file; nothing is read twice. Memory use does not grow with file size (4 MiB blocks).
Speed is limited by the disk, network share, USB bridge or security scanner reading the file.

- Files are hashed one after another on the background worker. The status bar shows the file
  and a progress bar; **Pause** and **Cancel** act between 4 MiB blocks.
- A cancelled file is not recorded; the remaining files in that job are skipped. The Log shows
  the file with an error such as `cancelled`. Quitting also cancels.
- A file that cannot be read (permissions, a lock by another program, a vanished network share)
  logs `<path>: <error>` and the job continues with the next file.
- A hash is stored only after the whole file was read. Nothing is saved for a crash or
  power loss mid-file; hash it again.
- Hashing does not delay the interface; the library views read their own snapshot (WAL).
  The queue runs separately and is not started by hashing.

## Rename problems

Renaming is manual and previewed: choose a file on **Hashed files**, **Rename…**, review the
**Confirm rename** window, then **Rename file**. Nothing renames automatically, and there is
no undo.

| Message | Meaning and action |
|---|---|
| `Select a hashed file first` | No file is selected |
| `The selected file is no longer in the list` / `The selected file is not identified by AniDB` | The preview needs the file's AniDB identification. Run identification online first |
| `Rename not possible: <problem>` | The pattern or the data produced an error. The window shows details |
| `Rename blocked: the renaming pattern reported warnings` | Evaluation warnings block **Rename file**. Use **Edit pattern in Options** |
| `The renaming pattern is empty. …` | Use **Use stable default** or write a pattern |
| `The renaming pattern has a syntax error: …` | Fix the pattern; **Validate on selected file** in Options dry-runs it |
| `The stable default needs the anime title, which has not been fetched yet. …` / `… needs the episode number, which is not known yet. …` | The default pattern needs fetched title and episode data. Use the anime detail window's **Refresh from AniDB**, make sure the file is identified, and run the queue online, then retry |
| `The pattern ended with the truth value "…" instead of a file name. …` | The last expression must evaluate to the new name |
| `The pattern produced an empty file name; nothing would be renamed. …` | Same cause; an empty target is an error, never a silent no-op |
| `Renaming "<old>" to "<new>" would result in an invalid file name (<why>); rename skipped!` | A path component contains one of `<>:"\|?*` or a control character, is empty, is longer than 255 (UTF-16 units on Windows, UTF-8 bytes elsewhere), or is a reserved device name such as `CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`. Trailing dots and spaces are trimmed first. The rules apply on every platform |
| `The file already has the name the pattern produces` | Nothing to do |
| `Rename failed: destination already exists: <path>` | A collision. The preview does **not** check for an existing target; the check happens when you apply it. Change the pattern or move the other file |
| `Rename failed: I/O error: <os error>` | The filesystem operation failed (see below) |
| `Rename failed: database error: …` | See the warning below |

Details:

- **Collisions** are detected only at apply time. Two files that map to the same name will
  succeed once and fail the second time.
- **Cross-volume moves fail.** A rename is a filesystem rename, not copy and delete. If the
  pattern or the rule's base path puts the target on a different volume or drive, the
  operating system refuses and nothing is moved. There is no partial copy.
- **Missing folders are created before the rename.** If the rename then fails, the new empty
  folders remain; Animatic does not remove them.
- **Open files and permissions:** a file held open by a player or scanner, a read-only
  destination, or an operating-system limit on total path length (the planner limits each
  name component but not the whole path) causes an I/O error.
- The library path is updated only **after** a successful rename. If the file moved but the
  database update then failed (`Rename failed: database error: …`, for example from a busy
  database), the library still shows the old path. Add the file again at its new location with
  **File → Add files…**, or let a scan hash it, and remove the stale record with a scan that
  has **auto remove** or accept it as stale.
- The existence check and the rename are two separate steps, so a file created at the target in
  between is not detected by Animatic. On Linux the rename can then **replace** that newly
  created file instead of failing with a collision, so do not let other programs write to
  the destination folder while you rename.

For syntax and functions see [Rename language](rename-language.md).

## AOM import problems

### Databank import

**File → Import AOM databank…** opens a folder picker titled **Choose AOM kowai folder**. The
folder you choose must itself contain `mylist.data`, `local_files.data` or `animes.data`.
Choosing the AOM install folder that merely contains `kowai` is rejected with `No AOM
databank found in <path>`; choose the `kowai` folder.

The import reads the source files only (they are never modified), decrypts each collection
into `import-temp/`, imports it in **one transaction per collection**, and tries to remove the
plaintext (a failure can leave it behind). Results to be aware of:

- **Partial results.** A collection that fails to import is rolled back and skipped, and the
  import continues. The interface reports only `Imported N records from M collections`; the
  per-collection warnings and skipped list are not shown. If M is smaller than the number of
  recognized `.data` files in the folder, some collections were skipped.
- **A decryption error aborts the run.** The text is `Legacy import failed: <error>`.
  Collections already committed stay in the database; the remaining ones and the final
  derived-data step (highest episode, special counts, group fix-up) did not run, and the
  `legacy_import_source`/`legacy_import_date` markers were not written.
- **Cancel.** **Cancel** is cooperative and is not guaranteed to stop the import at once. It is
  honored while a collection decrypts and just before each collection's transaction; the log
  then shows `Legacy import failed: cancelled`. Decrypting a collection cannot be interrupted
  midway, so cancelling can take until the current collection finishes decrypting. A Cancel
  that arrives during the final collection's transaction or the derived-data step is ignored:
  the import can complete and report success. Already committed collections remain.
- **Recovering.** Restore a backup taken before the import, or rerun the import after fixing
  the cause. Most collections are written with replace-by-key statements, so a rerun replaces
  rows instead of duplicating them; that also means a rerun overwrites rows with the
  databank's older values, so import into a fresh database, or one you have backed up, rather
  than into a library with newer synchronized data.
- Remove leftover decrypted files as described in
  [Disk space, temporary files and leftovers](#disk-space-temporary-files-and-leftovers).
- The import menu items are disabled while an import or settings review is pending, and other
  job submissions are blocked until the review ends.

### Settings import

**File → Import AOM settings…** parses the chosen XML on the worker and shows **Review imported
AOM settings**. Errors appear as `Could not import AOM settings: …` (for example `XML: No
supported AOM settings found in the selected XML` for unrelated XML). Failure or **Cancel**
leaves your active settings unchanged and releases the lock on other actions. **Apply imported
settings** saves first and forces offline mode; if the save fails you see `Could not save
settings: …`, your old settings stay active, and the review remains open. The import starts
from defaults and replaces whole sections, so settings the XML does not mention are replaced
by the importer's fallbacks rather than kept. The imported values include the AOM password
in plain text; review managed folders, players, credentials and the registered client name
before going online (see the [Settings reference](settings-reference.md)).

## External tools and exports

- `player executable is not configured`: set a player in **Options → External player**.
- `invalid external command arguments: …`: the argument template could not be split (for
  example an unbalanced quote). Templates are split into an argument list after placeholder
  substitution, with no shell involved; see the [Settings reference](settings-reference.md)
  for placeholders.
- `AVDump integration is disabled` / `external executable is not configured`: enable and
  configure it in **Options → AVDump**. Templates are expanded as raw text and then split into
  arguments, so a value containing a space or quote (the user name, key or a path) can split
  into extra arguments; quoting a placeholder is only a mitigation. A template that uses `{key}`
  places that key on the child process's command line.
- `Hash export failed: …`, `Automatic statistics export failed: …` or a manual statistics
  export error: the destination is not writable or the path is invalid. These write to the path
  you chose; they reveal your library, so keep them out of the repository.

## Privacy-safe support and bug reports

### What is sensitive

- **Settings and backups:** `settings.json`, `settings.json.bak` and any `.tmp` copy hold the
  AniDB password, API key and AVDump key in plain text. Never attach or paste them.
- **The database** (`animatic.sqlite3` and its `-wal`/`-shm`), exports (statistics XML/INI,
  hash lists) and `import-temp` contents are personal data: viewing history, file paths and
  names, hashes, AniDB identifiers.
- **The Log page** is the only diagnostic channel, and it is kept in memory only (older lines
  are discarded as it grows, **Clear** empties it, and it is lost on exit; there is no log
  file and no export button). Lines include full file paths (which can contain your user and
  drive names and series titles), ED2K hashes, local and AniDB identifiers, server reply text
  and cooldown reasons. Animatic does not intentionally write credentials into log lines, but
  review every line you share.
- **Startup errors** print the database and settings paths, which often contain your user name.

### How to sanitize

1. Copy only the lines you need and replace: user and drive names in paths with `<user>`,
   `<root>`; titles and file names with `<file>`; ED2K hashes, `lid`/`fid`/`aid`/`gid`
   numbers and session text with `<id>`; any user name, password, API key or client name with
   placeholders.
2. Keep exact message wording (for example `AniDB sync paused until <time>: <reason>`), the
   status-bar indicators (**Offline**, **N blocked**, **N commands NOT saved yet**), and what
   you clicked. Those usually identify the problem without any personal data.
3. Never post settings files or databases. If a settings detail matters, describe the field
   and value type (for example "`anidb.offline` is true"), not the file.
4. Include: the version shown in **Help → About Animatic**, the operating system, whether you
   ran a release archive, a CI artifact or a local build, and whether the settings were recovered.
5. To reproduce, build a **synthetic** case instead of sharing yours: temporary database and
   settings paths (see [Getting started](getting-started.md#portable-and-rehearsal-setups)),
   **Work offline** ticked, small generated files under a fictitious root such as
   `<temp>/animatic-test-root`, and the user name `animatic-test-user`.
6. Do not try to reproduce a problem by sending requests to AniDB, and never use made-up client
   names or credentials against it.

Example of a sanitized report:

```text
Animatic X.Y.Z, Windows (local build). Settings loaded normally.
Steps: Tools -> Run command queue, offline unticked.
Result: Log "<time>  ERROR  <command>: authentication: login failed: wrong username or password"
        then "Command blocked: 3 queued commands blocked: authentication: ..."
Status bar: "3 blocked". Retry failed + Synchronize now repeats the same error.
```

The repository's bug report form asks for these fields and requires you to confirm that the
report is sanitized. Issues are public.

This guide defines no security-reporting process, and the repository contains none. If you
believe you found a vulnerability, share sanitized details through your normal channel with the
maintainers, and do not publish credentials or private library data anywhere public.

## Index of recurring messages

| Where | Text | Section |
|---|---|---|
| Startup | `Animatic could not start (database: …, settings: …)` | [Startup problems](#startup-problems) |
| Window | **Settings recovered** | [The recovery window](#the-recovery-window) |
| Status bar / Queue | `AniDB sync paused until …` | [Cooldowns and bans](#cooldowns-and-bans) |
| Log | `Command blocked: …` | [Queue states](#queue-states-blocked-deferred-and-unsaved) |
| Log | `Database busy: queued command is NOT saved yet, retrying` | [Database busy](#database-busy-and-unsaved-commands) |
| Log | `Offline mode is enabled; commands remain queued` | [Credentials](#credentials-and-client-registration) |
| Log | `Rename failed: …` | [Rename problems](#rename-problems) |
| Log | `Legacy import failed: …` | [AOM import problems](#aom-import-problems) |
