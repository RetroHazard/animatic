This page maps the legacy AniDB O'Matic (AOM) workflows to Animatic and separates four things
that are easy to confuse:

- what a user can do in the desktop application,
- what exists only in the Rust libraries or tests (no desktop path),
- what is persisted in settings but has no runtime effect, and
- what is not implemented.

"Equivalent" means the workflow is available with a modern implementation; Delphi control
layout, bugs and retired protocols are deliberately not reproduced. The evidence behind the
research and the historical forum material is in the [AOM forum audit](https://github.com/RetroHazard/animatic/blob/main/docs/aom-forum-audit.md),
which is a research log and not a specification.

| Label | Meaning |
|---|---|
| **Desktop** | Reachable in the `animatic` application |
| **Partial** | Reachable, with the limits stated |
| **Core only** | Implemented in `animatic-core`/`animatic-anidb`, no desktop control |
| **Stored only** | Persisted in settings/import, but nothing reads it |
| **Not implemented** | No code path |
| **Replaced** | Deliberately handled differently |

## Workflows

| Legacy area | Status | What exists | Limits |
|---|---|---|---|
| Startup databank loading | Desktop | **File > Import AOM databank...** runs a cancellable import of an encrypted `kowai` directory in the worker; the source is only read. Importing into a database that already holds data first asks for confirmation (rows with the same ids are replaced; `--database` starts a new file). Decrypted copies live in a marked Animatic-owned folder in the system temporary folder that is removed when the import ends, and the next start offers to remove marked folders an interrupted import left behind. A command-line example (`import-legacy`) imports into a chosen file | Transactions are **per collection**: a failing collection is skipped with a logged warning and earlier ones stay. Cancel is checked between steps, not inside the final transaction or derived-data step, so a late cancel can still finish successfully. The confirmation covers a database with anime, MyList entries or hashed local files; the example does not ask. A crash can still leave plaintext copies until the next start's offer (only folders with the prefix and the marker are removed; an older `import-temp` folder is not offered). See [Architecture](https://github.com/RetroHazard/animatic/blob/main/docs/architecture.md#legacy-migration) |
| Hash queue | Desktop | **Add files...** and command-line paths hash in one pass (ED2K-red, CRC32, MD5, SHA-1) with progress, pause and cancel | The pending list lives in memory only; it is not persisted. Only the ED2K-red variant is used by the application (`Ed2kVariant::Blue` is core only). Media-property extraction (fps, codecs, ...) is not performed |
| Batch/file manager | Partial | **Tools > Scan managed folders** (and the optional [scheduled scan](Managed-Folders-and-Scans.md#scheduled-scans)) applies per-folder rules: path, recursion, hidden-file and (Windows) system-attribute exclusion, auto-hash, auto-rehash on changed modification time, auto-remove of *database records* for files affirmatively missing under a reachable root; plus per-folder MyList add/state/watched/storage/source/other policy and a rename base path. A file is hashed only once its size and modification time have been unchanged for `action_delay_ms` (a file still being written waits for a later scan), and `sleep_time_ms` paces the hashed files of a scan ([settle delay and pacing](Managed-Folders-and-Scans.md#settle-delay-and-pacing)). Per folder, **Remove emptied folders** removes the folders a manual or batch rename emptied, beneath the managed folder and only when completely empty ([details](Managed-Folders-and-Scans.md#removing-folders-a-rename-emptied)) | The scan always filters by the hashing extension list. Discovery (hash/rehash) is **first rule wins in list order** among rules with a path and auto-hash or auto-rehash; longest-path matching applies only to the post-hash MyList policy, the rename base path and auto-remove matching. The scan itself never deletes or moves media files and sends nothing to AniDB, but each newly hashed file can queue an `Identify` that carries a MyList add when identify-after-hashing is on and the matching folder has MyList auto-add enabled; that queued add reaches AniDB when the queue is run. `sleep_time_ms` no longer means AOM's pause between scans (that value now seeds the scan interval on import). `exclude_system` has no effect on Linux (no such attribute). Emptied-folder removal follows only Animatic's own renames, never a scan, never the managed folder itself and never recursively. `auto_rename` is stored only |
| Hashed files | Desktop | Known/unknown inventory with AniDB and MyList columns, CRC mismatch indication, Open (operating-system default handler), Play, AVDump, Rename, Identify after hashing | Identification is queued work: nothing is sent until the queue is run |
| Anime/mylist trees | Desktop | Searchable **Anime** and **My list** tables, a detail window with titles, episodes, genres/tags, relations (clickable) and links to AniDB | Tables, not trees. Reviews appear only if they came from a legacy import. No pictures or covers are shown |
| Advanced search | Desktop | Title (or exact id), year, type, MyList-only, watched-only filters over the local cache | Local data only |
| Groups and genres | Desktop | Searchable lists with library counts | |
| Add/edit MyList | Desktop | **Hashed files → Add to MyList…** queues adds for the selected files (identified files by file id, others by hash) with state, watched, view date, storage, source and other. **My list** edits State and Watched in the row and every field, including clearing a text field and the view date, in **Edit…**; **Remove from MyList…** (with confirmation) and **Refresh entry** queue a removal and a refresh. Edits are merged per entry, saved with the optimistic local change in one transaction and synchronized by the queue; the folder policy can still add files automatically (`Identify` with an add) | Adds start from hashed local files only: there is no add by AniDB id and no bulk edit of existing entries. A queued add shows as `Add queued` on Hashed files but creates no My list row until AniDB answers, because the entry id comes from AniDB. A removal deletes the local row only after AniDB acknowledges it and never deletes a media file. A FILE reply that explicitly reports "not in MyList" removes the matching *local* entry unless a command for it is queued |
| AniDB session | Desktop | UDP login, optional encrypted session (API key), compressed replies, session re-authentication, typed replies, offline mode, persisted cooldowns, blocked failures with explicit retry; AniDB's request limits hold across runs and restarts: program-wide UDP (2 s, then 4 s after five) and HTTP (2 s) spacing with the send time saved before each request, a fixed local port (a busy port is an error, never another port), a banned client version (504) blocks the queue until corrected, and an unanswered login backs off 30 s, 2 min, 5 min, 10 min, 30 min, then 2 h ([Request limits](AniDB-Synchronization.md#request-limits)) | The queue runs only on request. The registered **Client version** has an Options field next to **Client name** (a whole number of 1 or more); Animatic releases never change it, and the AOM import does not set it. The UDP channel is not TLS ([Transport security](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#transport-security)). Keep-alive `PING` and idle logout (`idle_logout_minutes`) are not used; every queue run still logs in and out |
| Full update | Desktop | **Tools > Full update...** (after a confirmation showing the counts, the number left out and when the title dump is next allowed) or `--full-update` queues the title dump, a `FetchFile` for each file id and a `FetchAnime` for each anime id found among the *identified hashed local files* (idempotent), leaving out what AniDB's limits do not allow yet: the title dump inside 24 h, an anime inside 24 h and a known file inside 7 days of the last request, successful or not; **Tools > Download anime titles** and the anime detail **Refresh** queue single items and refuse inside those windows (saying when the request is allowed); **Fetch mylist statistics**, the per-episode **Refresh**, **Groups > Refresh** and **My list > Refresh entry** queue single items (UDP, subject only to the shared spacing) | It does not refresh every cached entity or your whole MyList. With **Update known file information** on, hashing an identified file again queues a `FetchFile` for it, subject to the 7-day file window. The statistics result is stored and shown on Overview, apart from the local figures |
| Statistics export | Desktop | **File > Export statistics...** and optional automatic export, XML or INI, computed on the projection thread | The figures are read in one database snapshot and a failed query is an error, not zeros. The file is replaced atomically (temporary file, flush, rename); a failed write leaves the previous file intact. A hard crash can leave a hidden `.<name>.*.tmp` file |
| HTTP API and images | Partial | Anime XML and the public title dump are fetched through queued commands | Image download (`HttpClient::image`) and the image-server setting are **Core only / Stored only**; no picture is displayed |
| Rename files | Partial | Compatible expression language, user functions, database-backed `A`/`E`/`F`/`G`/`H`/`M` objects, live diagnostics, preview and explicit apply for one file or a selected batch with per-file diagnostics and one confirmation ([Rename language](Rename-Language.md), [Renaming files](Renaming-Files.md)); apply runs on the background worker with progress and cancel, and a move to another drive copies, verifies the copy against the stored ED2K, then swaps ([guarantees](https://github.com/RetroHazard/animatic/blob/main/docs/architecture.md#what-the-code-guarantees-and-what-it-does-not)) | Manual only; **automatic renaming is not implemented**. The file must be AniDB-identified with its episode and anime cached. A cross-drive move reads the file twice (copy, then verification) and needs free space for a second copy; a same-drive rename is not atomic against a file created at the destination in between. No undo |
| Hash export | Partial | **File > Export hashes...** writes one line per hashed file from a template | Only `%filename%`, `%filepath%`, `%ed2k%`, `%md5%`, `%sha1%`, `%crc32%`, `%size%` (short text) and `%fulled2k%` are real; the media tokens (`%fps%`, `%length%`, `%height%`, `%width%`, `%vcodec%`, `%vbitrate%`, `%acodec%`, `%asample%`, `%abitrate%`) are replaced with constants. The export is built and written on the projection thread (the UI thread only opens the file dialog) and replaces the target atomically |
| Players/AVDump | Partial | A configured default player launches with its *play* arguments; AVDump launches when enabled; both start the executable directly (no shell) | The player `open` and `enqueue` templates (`PlayerAction::Open`/`Enqueue`) are **Core only**: the Options window edits the executable and play arguments (the `name` control is never read), and the **Open** button uses the operating-system default handler instead. Templates are split into arguments first and placeholders substituted inside each argument, so a path, user name or key always stays one argument. Omitting `{path}`/`%file%` from a player `play` template appends the path as its own argument. Launched programs are not sandboxed |
| Settings | Partial | JSON settings with backup recovery; the AniDB password, API key and AVDump key kept in the operating-system keystore (Windows Credential Manager, Linux Secret Service) with a visible settings-file fallback; native UTF-16 AOM settings import with non-secret review and offline apply | Imported settings replace the current ones, with unmentioned values at their defaults. Without a keystore the credentials stay in the settings file in plain text. Many fields are persisted without effect ([next section](#settings-persisted-without-runtime-effect)); see the [settings reference](Settings-Reference.md) and [credentials](Settings-Files.md#credentials-and-the-system-keystore) |
| Add anime/episode/file/change request | Replaced | **Tools > Edit AniDB data** opens AniDB's maintained web editors | AOM's retired TCP `ADDANIMEDATA`/`ADDEP` route is not implemented |
| Command line / shell handoff | Desktop | Media paths (hashed at startup); `--database`/`-d`, `--full-update`, `--help`; legacy `/database`, `/fullupdate`; `/utf8` and `/ucs2` accepted and ignored. A launch with media paths for a database that is already open hands them to the running window instead of opening a second one ([One window per database](Running-Animatic.md#one-window-per-database)). Optional per-user **Hash with Animatic** file-menu entry, off by default: a registry key under `HKEY_CURRENT_USER` on Windows, a `.desktop` file on Linux ([File-menu entry](Options-Window.md#file-menu-entry)) | The file-menu entry covers files, not folders, and installs no shell-extension DLL. Bringing the running window forward is best effort. The Linux desktop-file path is unit-tested but has not been run on Linux. Unknown `-` options are rejected |
| Status, progress, log | Desktop | Progress with pause/cancel, activity log, Queue page with Pending/N retries/Blocked, cooldown banner, "N commands NOT saved yet" | |
| Local cache recovery tooling | Partial | **File > Back up library...** (online backup, atomic file), **File > Restore library...** (validated, confirmed, keeps a timestamped copy of the replaced library), **Tools > Check library** (`integrity_check` and `foreign_key_check`) and **Tools > Compact library...** (`VACUUM`), all in the background; settings backup recovery | No repair, no automatic or rolling backups, no scheduled checks; a backup holds the database only (not `settings.json`); restore and compaction wait for an idle worker |
| Full-account MyList bootstrap | Not implemented | Entries arrive through legacy import, FILE replies for hashed local files, queued adds/edits | Local state is not equivalent to a complete account download; there is no reconciliation of the whole MyList |
| Scheduled folder scanning | Desktop | Optional **scan at start** and **scan every N minutes** (5 minutes to 1 week, off by default) in Options → Managed folders, run by a worker-side timer that submits the ordinary scan; it never overlaps another task, an import or settings review, a restore or unsaved commands | Polling only: there is no filesystem watcher. No tray icon |
| Colour-coded rows, saved columns, sorting, window state, system theme | Desktop | **Use status colors** tints Hashed files, Anime, Advanced search and My list rows by status (CRC mismatch, unidentified, watched, in MyList), always with the status also written in the row; headers sort, a right click chooses columns, and widths, hidden columns and sort order are remembered; the window's size, position and maximized state are restored; **Theme: System** follows the operating system | Colors have no Options editor (`ui.colors`, AOM palette imported); the AOM tree-specific slots (folder, root, episode, duplicate, CRC ok) have no table to color. Positions on a disconnected monitor are not detected before the window exists. Linux runtime is not verified |
| Tray icon | Not implemented | Settings fields exist | See below |

## Core-only capabilities

Available to code that links the crates, covered by tests where noted in the source, but with
**no desktop control**:

| Capability | Where |
|---|---|
| UDP `ANIME` wrapper, `PING`, the `new_version_available` flag | [`commands.rs`](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-anidb/src/commands.rs), [`udp.rs`](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-anidb/src/udp.rs) |
| Image download | `HttpClient::image` |
| ED2K-blue variant | `hash::Ed2kVariant::Blue` |
| Player `open`/`enqueue` actions | `external::PlayerAction` |
| `Db::clear_local_files` | [`db`](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/src/db/mod.rs), [`store.rs`](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/src/store.rs) |
| Command-line import example | `cargo run -p animatic-core --example import-legacy -- <kowai> <output.sqlite3>` |

## Settings persisted without runtime effect

Nothing reads these fields. Four have an Options control, shown disabled with a "Not active yet" hover text; the stored value is kept.
A saved setting is not proof of behavior. The [settings reference](Settings-Reference.md) has
the complete per-field table.

- AniDB: `image_server`, `idle_logout_minutes`.
- Hashing: `autostart` (disabled checkbox), `priority`, `ed2k_space_replacement`.
- File managers: per folder `auto_rename` (disabled checkbox).
- UI: tree expand/collapse, local folder size, root folder hiding, tray options. The window
  state, column layouts, row colors, theme and scale are applied; see the
  [settings reference](Settings-Reference.md#ui).

`hashing.use_extensions` only filters the **Add files** picker; folder scans always use the
extension list, and paths given on the command line are hashed unfiltered.

## Deliberate non-ports

The object inspector was a developer RTTI viewer. The header setup and anime-genre editor were
unreachable resource-only UI in AOM. The Add File dialog could not submit because its OK button
was permanently disabled. Splash screens, MDI tab duplication, custom painted trees, exact
colors and Delphi-specific registry/COM shell-extension behavior are presentation or platform
artifacts rather than library workflows.

Animatic uses native file pickers, platform application-data directories, external HTTPS links
for current AniDB metadata editing and a fixed table layout. These choices avoid reviving
discontinued protocols and Windows-only COM dependencies.

## Compatibility differences worth knowing

- Rename counters use at least two digits for the new/default pattern, independent of count
  metadata; saved custom patterns are never rewritten; invalid-name checks are deliberately
  cross-platform (POSIX UTF-8 bytes versus Windows UTF-16 units, 255). Details:
  [Rename language](Rename-Language.md).
- Retired metadata-change commands are replaced by web links (above).
- The AniDB transports follow current protocol rules (flood spacing, encryption, HTTPS title
  dump), not the retired AOM behavior.

## Verification status

Windows and Linux are supported targets and CI builds and tests both. Local runtime exercise
so far was on Windows with synthetic data; **Linux runtime behavior and production AniDB
synchronization have not been exercised**. Test counts in older documents are historical.

## Not implemented

Verified absent from the application (some exist in `animatic-core` or are stored only):

- Full-account MyList download or reconciliation; automatic renaming; filesystem watchers (folder
  scans are manual, at start or on an interval); automatic or rolling database backups and database
  repair (backup, restore, check and compaction are manual menu actions).
- Adding a MyList entry by AniDB id or editing several entries at once; editing anime metadata
  (the Edit AniDB data menu opens web pages).
- Images and covers (the HTTP image function is unused; no image UI); system tray and
  minimize-to-tray.
- Queue or player *open/enqueue* actions;
  resumable downloads; hashing a folder from the shell file menu.
- Linux runtime and production AniDB verification (the Linux Secret Service keystore path is
  compiled but has not been run); encryption of the database and of backups.
- Unattended or scheduled updates, update channels (prereleases), release signatures, rollback,
  package or installer integration; an update replaces only the executable and versioned license files.
