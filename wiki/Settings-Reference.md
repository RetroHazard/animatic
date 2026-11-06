Animatic keeps all user configuration in one JSON document, `settings.json`. This page lists every
key of that document with its type, default, the Options control that edits it (if any), and what
the application actually does with the value.

**A key that exists in the file is not necessarily a feature.** The settings model was built to
round-trip AniDB O'Matic (AOM) configuration, so several keys are stored but never read by the
desktop application. Every key below carries a status from the legend, so you can tell a working
option from a placeholder before relying on it.

Where the file lives, the `ANIMATIC_SETTINGS` / `ANIMATIC_DB` overrides and the command-line flags
are described in [Data locations](Data-Locations.md) and [Running Animatic](Running-Animatic.md).
Day-to-day tasks (hashing, scanning, MyList edits, exports) are in the
[user guide pages](Home.md#use-animatic). How the file is loaded, saved, backed up and recovered
is in [Settings files](Settings-Files.md); recovery steps and support guidance are in
[Troubleshooting](Troubleshooting.md). Source of truth for everything here:
`crates/animatic-core/src/settings.rs` (model, defaults, load/save, legacy import) and the
consumers named in each table.

> Evidence: this page was written from reading the source, not from new test runs. "No consumer"
> means a search of the application, worker and core crates found no code that reads the value
> other than the struct, its default, the legacy importer and (where noted) an Options checkbox.

## Status legend

| Status | Meaning |
|---|---|
| **Active** | The value is read at runtime and changes behavior. |
| **Active, JSON only** | Read at runtime, but Options has no control; edit the file (with Animatic closed) or import it. |
| **Partly active** | Read, but only by some paths; the effect column says which. |
| **Inert control** | Options shows the control disabled with a "Not active yet" hover text; the value is kept and saved, but nothing reads it. |
| **Stored only** | Saved and loaded, no Options control, no consumer. |
| **Import only** | Stored only, and additionally filled in by the AOM settings importer. |
| **Core only** | The core library supports it, but no desktop action calls that code. |

Options is **Tools → Options…**. Its sections, in order: AniDB account, Appearance, System tray, Hashing,
Managed folders, Statistics export, External player, AVDump, Renaming pattern, Updates,
Mylist states (read-only label).

## Document layout

The root of the file must be a JSON object. Keys use `snake_case`, exactly as the tables spell them.

| Root key | Type | Default | Section |
|---|---|---|---|
| `anidb` | object | see [AniDB account](#anidb-account) | Account, client identity, transport, offline mode |
| `hashing` | object | see [Hashing](#hashing) | Extension filter, hash export, post-hash identification |
| `ui` | object | see [UI](#ui) | Theme, scale, colors, column and window state |
| `players` | array of objects | `[]` | [External players](#external-players-players) |
| `file_managers` | object | two built-in entries | [Managed folders](#managed-folders-file_managers) |
| `renaming_pattern` | string | stable built-in pattern | [Top-level keys](#top-level-keys) |
| `stat_whore` | object | disabled | [Statistics export](#statistics-export-stat_whore) |
| `avdump` | object | disabled | [AVDump](#avdump-avdump) |
| `ed2k_space_replacement` | string or `null` | `null` | [Top-level keys](#top-level-keys) |
| `first_launch` | boolean | `true` | [Top-level keys](#top-level-keys) |
| `secrets_in_keystore` | boolean | `false` | [Top-level keys](#top-level-keys) |
| `updates` | object | `{ "auto_check": true }` | [Updates](#updates) |
| `media` | object | `{ "read_properties": false, "ffprobe_path": "" }` | [Media properties](#media-properties-media) |

## AniDB account

Object `anidb`. See [Storage and network contracts](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md) for what the queue sends
and how failures are handled. Animatic contacts **AniDB** only when you run the command queue (Tools →
Run command queue, or Queue → Synchronize now) and when an anime detail window needs a cover picture
that is not on disk yet. The worker receives a copy of this object at that
moment, so edits apply from the next run. (Update checks contact GitHub, not AniDB; see [Updates](#updates).)

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `username` | string | `""` | Username | **Active.** UDP login; `{user}` in AVDump arguments; written into statistics exports. |
| `password` | string | `""` | Password (masked) | **Active.** UDP login. Kept in the system keystore and stored empty in the file; clear text in the file only while no keystore is available, see [Credentials](Settings-Files.md#credentials-and-the-system-keystore). |
| `api_key` | string | `""` | API key | **Active.** When non-empty the UDP session is encrypted with this key (profile UDP API key on anidb.net). Empty means an unencrypted session. Stored like `password`. |
| `client_name` | string | `""` | Client name | **Active.** Sent as `client` to both the UDP and HTTP APIs. Empty makes UDP login and HTTP anime requests fail with "no AniDB client name configured". Must be an identifier registered with AniDB for your client; Animatic ships none. |
| `client_version` | integer (u32) | `1` | Client version | **Active.** Sent as `clientver` (UDP and HTTP). The control accepts a whole number of 1 or more; other text is refused (the field shows why, and **Save options** stays disabled until it is corrected). A file that holds `0` is read as `1`. This is the version you registered for your client together with the client name; it identifies the registration, not the program release, so Animatic updates never change it. Not imported from AOM. |
| `udp_server` | string | `api.anidb.net:9000` | none | **Active, JSON only.** `host:port` of the UDP API. |
| `local_port` | integer (u16) | `29456` | none | **Active, JSON only.** Local UDP port to bind (all interfaces). AniDB bans clients that use many different source ports, so Animatic uses exactly this port and **never falls back to another one**: if it cannot be bound (another program uses it) the run stops with an error naming `anidb.local_port`, nothing is sent and the commands stay queued; free the port or choose another above 1024. Also passed to AVDump as `{port}`. `0` asks the operating system for a port, a different one every run, which is meant for tests and risks that ban. |
| `image_server` | string | `cdn.anidb.net` | none | **Active, JSON only** (imported from AOM). Host that anime covers are downloaded from (`https://<host>/images/main/<picture>`), used until a login has told Animatic which image host AniDB wants used; after that the host AniDB announced at login is used and this value is only the fallback. A host name with an optional port; anything else (a path, a scheme, spaces) makes the cover download stop with an error. |
| `download_covers` | boolean | `true` | "Download anime covers when a detail window opens" | **Active.** When true, opening an anime's detail window downloads its cover once if it is not on disk yet (never offline, never during a ban or cooldown); see [Cover](Library-Pages.md#cover). When false nothing is downloaded, and covers already in the [covers folder](Data-Locations.md#cover-pictures) are still shown. Files without the key load as `true`. |
| `update_file_information` | boolean | `false` | "Update known file information" | **Active.** When true, hashing a file that is already identified (it has an AniDB file id) again queues a refresh of its AniDB record (`Fetch file`), at most once per file per 7 days; with the default `false`, re-hashing a known file queues nothing. Unidentified files follow *Identify after hashing*. (Import fills it from AOM.) |
| `offline` | boolean | `false` | "Work offline (keep commands queued)" | **Active.** When true, Run command queue / Synchronize now send nothing and report "Offline mode is enabled; commands remain queued", and no cover is downloaded (covers already on disk are still shown). Commands are still queued locally. The status bar shows "Offline" or "Ready for AniDB". Forced to `true` by backup recovery and by applying imported AOM settings. It also suppresses the automatic startup [update check](#updates); an explicit **Help → Check for updates…** is still allowed. |
| `idle_logout_minutes` | integer (u32) | `10` | none | **Stored only.** Not read. Each queue run ends with a logout regardless of this value. |

Registration of a client identifier and the first online run are covered in
[AniDB client registration](First-Launch.md#anidb-client-registration); verify any imported or hand-typed identifier before going
online.

## Hashing

Object `hashing`.

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `autostart` | boolean | `true` | "Start hashing automatically" | **Active.** When Animatic starts with files still in the [persistent hash queue](Hashing-and-Identification.md#hash-queue), hashing resumes by itself once nothing blocks it (an import review, a restore, the settings recovery notice or a running task). With `false` nothing hashes at start; the Hashed files page and Overview show the pending count with a **Resume hashing** button. Read at start, so a change applies at the next start. (AOM import sets it from `autostartHashing`, default `false` when absent.) |
| `priority` | integer 0-6 (u8) | `1` | none | **Stored only** (kept for AOM compatibility, no effect). Legacy thread-priority index (0 idle to 6 time critical). Hashing always runs at the normal thread priority, because there is no cross-platform thread-priority control; the importer clamps the value to 6. |
| `use_extensions` | boolean | `false` | "Restrict file extensions" | **Partly active.** Only applies a file-type filter to the **Add files…** picker. It does not filter files given on the command line, and it does not switch scanning on or off (scans always use `extensions`; see below). |
| `extensions` | array of strings | the 52 legacy AOM extensions (`7z`, `aac`, `ac3` … `zip`) | multi-line box, comma separated | **Active.** Scan allow-list, see below. Add files picker filter when `use_extensions` is true. |
| `export_template` | string | `%fulled2k%` | "Hash export template" | **Active.** Template for File → Export hashes…; see [Hash export tokens](#hash-export-template-tokens). |
| `integrate_into_shell` | boolean | `false` | "Add "Hash with Animatic" to the file menu" | **Active.** **Save options** registers a per-user file-menu entry when it is `true` and removes an entry Animatic created when it is `false`. The value alone never registers anything: loading settings and AOM import (`general/integrateIntoShell`) only store it, and Options shows whether the entry is really present. See [File-menu entry](Options-Window.md#file-menu-entry). |
| `identify_after_hashing` | boolean | `true` | "Identify files with AniDB after hashing" | **Active.** After each file is hashed (Add files, command line, or scan), the UI queues an `Identify` command. If the file's [managed-folder rule](#how-a-file-picks-its-rule) has `mylist_auto_add`, the command carries that rule's MyList parameters. Offline, the command only waits in the queue. |

Extension handling in detail (from `file_manager.rs` and `app.rs`):

- **Scans (Tools → Scan managed folders)** always pass `extensions` to the planner, regardless of
  `use_extensions`. Entries are split on comma, semicolon or space, a leading `.` is removed and
  matching is case-insensitive. A file without an extension never matches, and an **empty list
  means a scan hashes nothing**.
- **Add files…** uses the entries verbatim as dialog filter names, only when `use_extensions` is
  true. Write entries without a leading dot.
- Files passed on the command line are hashed without any extension check.
- The Options text box splits on commas only; the file may hold any strings, but use plain
  extensions.

The built-in list is `7z,aac,ac3,ace,asf,ass,avi,dts,dtshd,flac,flv,idx,js,lrc,m2ts,m4a,mk3d,mka,
mks,mkv,mov,mp3,mp4,mpeg,mpg,ogg,ogm,ogv,pjs,qt,ra,rar,rm,rmvb,rt,smi,smil,srt,ssa,sub,swf,thd,
tmp,ts,tts,txt,wav,webm,wma,wmv,xss,zip` (a legacy AOM list, not a media-only list).

The ED2K variant is fixed (the "red" variant AniDB expects); there is no setting for it.

## UI

Object `ui`. Theme, scale, window geometry, column layouts and status row colors are applied by
the desktop application; the remaining keys mirror AOM's interface options and are **not** read.

### Appearance and paths

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `theme` | `"Dark"`, `"Light"` or `"System"` | `"Dark"` | Theme combo | **Active.** Applied at startup and by **Save options** or **Apply imported settings**. `System` follows the operating system's light or dark mode and changes with it while Animatic runs (where the platform does not report a mode, it is dark). Values are case-sensitive. |
| `zoom` | number (f32) | `1.0` | "UI scale" slider 0.75-2.0 | **Active.** Applied (clamped to 0.75-2.0) at startup and at the same moments as `theme`. Load-time fixup resets values `<= 0.3` to `1.0`; other out-of-range values are kept in the file and clamped when applied. |
| `last_directory` | string | `""` | none | **Active.** Start folder for the Add files picker and the AOM settings XML picker; updated to the folder of the files you last added. |
| `colored_rows` | boolean | `true` | "Use status colors" | **Active.** Tints table rows by status with the [row colors](#row-colors-uicolors) below; `false` draws every row plainly. Every status is also written in the row, so the color is never the only signal. Takes effect immediately, no save needed. |
| `copy_length_as_minutes` | boolean | `false` | "Export %length% in minutes" | **Active.** Read by hash export: `%length%` is the duration in whole minutes instead of seconds. Without [media data](#media-properties-media) it stays at its constant (`0`). See [token table](#hash-export-template-tokens). |
| `show_tray_icon` | boolean | `true` | "Show tray icon" | **Active.** Puts an icon in the Windows notification area or the Linux StatusNotifierItem tray (menu: Show Animatic, Run command queue, Quit; a left click shows the window). Takes effect as you click, not only on Save. Disabled, with the reason, where the system has no tray; the stored value is then kept and never acted on. See [System tray](Main-Window.md#system-tray). |
| `minimize_to_tray` | boolean | `true` | "Minimize to tray" | **Active.** Minimizing the window hides it to the tray instead of the taskbar. Needs `show_tray_icon` and a tray icon that is actually up; otherwise a minimize is an ordinary minimize. Closing the window still quits. |

### Window and layout state

| Key | Type | Default | Status and effect |
|---|---|---|---|
| `window` | `null` or `[left, top, width, height]` (four numbers) | `null` | **Active.** Outer position and inner size of the window in points at UI scale 1, restored at startup and kept current while the window is neither maximized, minimized nor fullscreen. `null` opens at 1280 x 800 and lets the system place the window. A saved size is raised to the minimum (900 x 600) and lowered to the monitor; a position that is not finite, lies beyond +-32000, leaves less than 100 points of the window right of the desktop's left edge or puts the title bar more than 50 points above the desktop is ignored (the size is still restored). Monitors are not known before the window exists, so a position on a monitor that is no longer connected is left to the operating system. The AOM importer fills it only when the values look sane (position above -100000, size above 200). |
| `maximized` | boolean | `false` | **Active.** The window starts maximized when `true`; the last normal `window` geometry stays what un-maximizing returns to. Kept current while Animatic runs. |
| `columns` | object: table id to array of `{ "id": string, "width": number, "visible": boolean, "sort": "Ascending" or "Descending" }` | `{}` | **Active.** Layout of every data table: width, visibility and sort order of each column, written back when you resize a column, hide or show it in the header's right-click **Columns** menu, or click a header to sort. `id`, `width` and `visible` are required in every column object; `sort` may be omitted (no sort) and is written only on the sorted column. Table and column ids are listed below. Unknown tables and columns are dropped and missing ones added when a table is shown; a width is kept between 40 and 4000 points; if every column of a table is hidden, all are shown; only the first `sort` of a table counts. |
| `tree_auto_expand` | boolean | `false` | **Import only.** No consumer. |
| `tree_auto_collapse` | boolean | `false` | **Import only.** No consumer. |
| `show_local_folder_size` | boolean | `true` | **Import only.** No consumer. |
| `hide_root_folder` | boolean | `false` | **Import only.** No consumer. |

The window geometry and column layouts are kept in memory while Animatic runs and reach the
file with the normal settings save: **Save options** and a normal exit (not when settings were
[recovered from a backup](Settings-Files.md#recovery-matrix) and not yet saved by you).

Column ids are the header text in lower case with other characters replaced by `_`. Table ids and
their columns (all sortable except the queue, whose order is the run order):

| Table id | Page | Column ids |
|---|---|---|
| `anime_table` | Anime | `title`, `type`, `year`, `episodes`, `rating`, `mylist` |
| `advanced-results` | Advanced search | same as `anime_table` |
| `mylist_table` | My list | `anime`, `episode`, `group`, `state`, `watched`, `size`, `queue` |
| `local_table` | Hashed files | `file`, `size`, `anidb`, `mylist`, `crc`, `path` |
| `groups` | Groups | `name`, `short_name`, `rating`, `votes`, `website` |
| `genres` | Genres | `genre`, `anime` |
| `queue_table` | Queue | `id`, `command`, `status`, `last_error` (no sorting) |

### Row colors (`ui.colors`)

`colors` is an object whose keys are color-slot names and whose values are
`{ "background": number, "text": number or null }`. Numbers are `0xRRGGBB` written as decimal
integers (JSON has no hex literals); `text` may be omitted or `null` (no text override), but
`background` is required in every entry. An absent slot uses the default background below.
With `colored_rows` on, the table rows take these colors (no Options control edits them; an AOM
import fills them). Without a `text` color the row text is black or white, whichever contrasts
more with the background; the defaults all reach a contrast ratio of at least 4.5. Slots marked
*plain* describe an unremarkable row and are only drawn when you set them, because their default
light greys would glare in the dark theme. Unknown slot names are not rejected and are kept on
save.

| JSON key (slot) | Label | Default background | AOM tags (background / text) |
|---|---|---|---|
| `LocalRoot` | Hashed files: root | `#C9C9C9` | 1 / 2 |
| `LocalFolder` | Hashed files: folder | `#DFDFDF` | 3 / 4 |
| `LocalFile` | Hashed files: file | `#F4F4F4` | 5 / 6 |
| `LocalFileInMylist` | Hashed files: file in mylist | `#70A0FF` | 7 / 8 |
| `LocalFileUnknown` | Hashed files: file unknown | `#FF7070` | 9 / 10 |
| `LocalFileInvalidCrc` | Hashed files: invalid CRC | `#CB97FF` | 11 / 12 |
| `LocalFileDuplicate` | Hashed files: duplicate | `#FF8000` | 27 / 28 |
| `Anime` | AniDB: anime | `#C9C9C9` | 13 / 14 |
| `Episode` | AniDB: episode | `#DFDFDF` | 15 / 16 |
| `EpisodeInMylist` | AniDB: episode in mylist | `#7070D0` | 17 / 18 |
| `File` | AniDB: file | `#F4F4F4` | 19 / 20 |
| `FileCrcOk` | AniDB: file CRC ok | `#80FF80` | 21 / 22 |
| `FileCrcInvalid` | AniDB: file CRC invalid | `#FF7070` | 23 / 24 |
| `FileInMylist` | AniDB: file in mylist | `#70A0FF` | 25 / 26 |
| `AnimeInMylist` | AniDB: anime in mylist | `#70A0FF` | none (Animatic only; 101 / 102) |
| `Watched` | Watched | `#90E090` | none (Animatic only; 103 / 104) |

Where Animatic uses a slot (first match wins):

| Table | Status (also written in the row) | Slot |
|---|---|---|
| Hashed files | CRC `Mismatch` | `LocalFileInvalidCrc` |
| | AniDB `Unknown` (not identified) | `LocalFileUnknown` |
| | Mylist `lid N · watched` | `Watched` |
| | Mylist `lid N` | `LocalFileInMylist` |
| | otherwise | `LocalFile` (plain) |
| Anime, Advanced search | Mylist `... · all watched` | `Watched` |
| | Mylist `N eps / M files` | `AnimeInMylist` |
| | otherwise | `Anime` (plain) |
| My list | Watched checkbox ticked | `Watched` |
| | otherwise | `File` (plain) |

The other slots (`LocalRoot`, `LocalFolder`, `LocalFileDuplicate`, `Episode`, `EpisodeInMylist`,
`FileCrcOk`, `FileCrcInvalid`) are stored for AOM compatibility and no table uses them yet.

Importing converts AOM's color integers (Windows BGR order) to RGB; an AOM text color of `0` means
"no override". An entry equal to the default background with no text color is dropped.

## Managed folders (`file_managers`)

Object `file_managers`: global scan timing values plus a list of per-folder rules ("file managers",
the AOM term). Folder rules drive Tools → Scan managed folders, the optional scan at start and
scheduled scan, the MyList parameters sent with identified files, and the rename destination. A
scan runs only when you start it or when one of the scan automation settings below is on; see
[Scheduled scans](Managed-Folders-and-Scans.md#scheduled-scans) and
[Settle delay and pacing](Managed-Folders-and-Scans.md#settle-delay-and-pacing).

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `action_delay_ms` | integer (u64) | `500` | "Hash a file once unchanged for" | **Active.** Settle delay of a scan: a file is hashed only once its modification time is at least this old and its size and modification time stay unchanged for this long; otherwise it is deferred to the next scan. `0` hashes at once. Load-time fixup limits values to 0-600000 (values above are lowered to 600000). |
| `sleep_time_ms` | integer (u64) | `0` | "Pause between hashed files" | **Active.** Pause between two hashed files of one scan (not before the first or after the last). Valid 0-10000. **Changed meaning:** earlier versions stored AOM's pause between scans here (default `900000`); load-time fixup resets any value above 10000 to `0`, and an AOM settings import turns `filemanager/sleepTime` into `scan_interval_minutes` instead. |
| `scan_at_start` | boolean | `false` | "Scan managed folders when Animatic starts" | **Active.** Runs one scan after the application starts, as soon as nothing blocks it. |
| `scheduled_scan` | boolean | `false` | "Scan managed folders on a schedule" | **Active.** Runs a scan every `scan_interval_minutes` minutes on the background worker. Never overlaps another task, import, settings review or restore. |
| `scan_interval_minutes` | integer (u32) | `60` | "Scan every N minutes" | **Active** while `scheduled_scan` is on. Load-time fixup limits values to 5-10080 (5 minutes to one week). |
| `managers` | array of objects | built-in "Default" and "\\\\ (Default network path)" entries | "Managed folders" list | See below. If present in the file, the array replaces the default list entirely. |

### Per-folder keys

Each element of `managers` may omit any key; an omitted key takes the value in the Default column
(these differ from the two built-in entries, shown afterwards).

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `name` | string | `""` | Name | Display label in Options only. |
| `path` | string | `""` | Path | **Active.** Folder the rule applies to. Empty paths never match a file and are skipped by scans. |
| `include_subdirs` | boolean | `true` | "Subfolders" | **Active.** Scan depth (all levels or direct children only) and whether files below the folder match this rule. |
| `auto_hash` | boolean | `false` | "Auto hash" | **Active in scans.** A scan hashes files that are not yet in the database. The scan itself writes no MyList data, but each hashed file raises the same post-hash event as Add files, so with `identify_after_hashing` and the matching rule's `mylist_auto_add` it queues an `Identify` that adds the file to MyList on AniDB when the queue next runs online. |
| `auto_rehash` | boolean | `false` | "Rehash changed" | **Active in scans.** A scan rehashes known files whose modification time (whole seconds) differs from the stored value. |
| `auto_remove` | boolean | `false` | "Remove missing" | **Active in scans.** A scan removes the local file record (and any hash record no longer used by another local file) of a file that is positively missing (not-found), provided a rule matches its path by an explicit path match (no fallback rule) and the rule's folder is a reachable directory. Never deletes media files. The removal itself writes only those local tables. |
| `auto_rename` | boolean | `false` | "Auto rename" | **Inert control.** Shown disabled ("Not active yet": planned opt-in automatic rename of identified files in the folder); the value is kept and never read. Renaming is always a preview-and-confirm action. |
| `base_path` | string | `""` | "Rename destination" | **Active.** Folder used as the rename target base and exposed to rename patterns as `BASEPATH`; empty keeps the file's current folder. See [Rename language](Rename-Language.md). |
| `exclude_hidden` | boolean | `true` | none | **Active.** Scans skip entries (files and folders) whose name starts with `.`. The OS "hidden" attribute is not consulted. |
| `exclude_system` | boolean | `true` | "Skip system files" | **Active on Windows.** Scans skip files and folders that have the Windows system attribute (a skipped folder is not entered). **No effect on Linux**, which has no such attribute. |
| `clear_empty_folders` | boolean | `false` | "Remove emptied folders" | **Active.** After a manual rename or batch rename moves a file, the folders it left are removed while they are strictly below this rule's folder and completely empty (hidden files keep a folder). The rule's own folder is never removed and nothing is deleted recursively. See [Removing folders a rename emptied](Managed-Folders-and-Scans.md#removing-folders-a-rename-emptied). |
| `mylist_auto_add` | boolean | `false` | "Add to mylist" | **Active.** When true and `identify_after_hashing` is on, the post-hash `Identify` command (queued after Add files, command-line hashing or a scan) carries the MyList parameters below, so the file is **added to your MyList on AniDB when the queue runs online**. Offline, the command only waits locally. |
| `mylist_watched_check` | boolean | `false` | "Set watched" | **Active.** Gates `mylist_watched`: when false the watched flag is not sent. |
| `mylist_watched` | boolean | `false` | "Watched" (shown when the check is on) | **Active.** Watched flag sent when the check is on; `true` also sends the current time as view date. |
| `mylist_state_check` | boolean | `true` | "Set state" | **Active.** Gates `mylist_state`. |
| `mylist_state` | integer (i64) | `1` | state combo (shown when the check is on) | **Active.** One of the labels in `Unknown`=0, `On HDD`=1, `On CD/DVD`=2, `Deleted`=3, `Remote/Shared`=4. Other integers are accepted by the file and sent as written. |
| `mylist_storage_check` | boolean | `false` | "Set storage" | **Active.** Gates `mylist_storage`. |
| `mylist_storage` | string | `""` | Storage | **Active.** Sent when its check is on (an empty string is sent as an empty value). |
| `mylist_source_check` | boolean | `false` | "Set source" | **Active.** Gates `mylist_source`. |
| `mylist_source` | string | `""` | Source | **Active** when its check is on. |
| `mylist_other_check` | boolean | `false` | "Set other" | **Active.** Gates `mylist_other`. |
| `mylist_other` | string | `""` | Other | **Active** when its check is on. |
| `can_edit_path` | boolean | `true` | hides "Remove folder rule" when false | **Active.** Entries with `false` cannot be removed from Options. |
| `default` | boolean | `false` | none | **Active.** Marks the fallback rule, see below. Options cannot change it. |

"Add folder rule" creates an entry named "New folder" using the omitted-key defaults above, which
means it has `include_subdirs`, `exclude_hidden`, `exclude_system` and `mylist_state_check` on.

### Built-in entries

A fresh configuration contains two rules:

| | "Default" | "\\\\ (Default network path)" |
|---|---|---|
| `path` | `""` | `\\` (JSON: `"\\\\"`) |
| `include_subdirs` | `false` | `false` |
| `exclude_hidden` / `exclude_system` | `true` / `true` | `false` / `false` |
| `mylist_auto_add` | `true` | `false` |
| `mylist_watched_check` / `mylist_watched` | `true` / `false` | `false` / `false` |
| `mylist_state_check` / `mylist_state` | `true` / `1` (On HDD) | `false` / `0` |
| `can_edit_path` / `default` | `false` / `true` | `false` / `false` |
| auto hash, rehash, remove, rename | all `false` | all `false` |

Consequences:

- Out of the box nothing is scanned (`auto_hash` and `auto_rehash` are off and no folder is set).
- A file hashed from anywhere that no rule claims falls back to "Default", which adds it to MyList
  as watched = no, state On HDD once identified (and once the queue runs online).
- The network entry matches any path that starts with `\\` or `//`; because it has
  `mylist_auto_add` off, such files are identified but not auto-added unless a more specific rule
  matches them.

### How a file picks its rule

Two different mechanisms are involved. **Scan discovery** (hash and rehash) is first-rule-wins in
array order; **everything else** (MyList parameters after hashing, rename destination, removal)
uses longest-path matching.

Scan discovery (Tools → Scan managed folders):

- Rules are visited in the order they appear in `managers`. Only rules with a non-empty `path`,
  an existing folder and `auto_hash` or `auto_rehash` set take part.
- The first such rule that reaches a file claims that path for the whole scan, even if that rule's
  own flags then do nothing for it (for example an `auto_rehash`-only rule meeting a not-yet-known
  file). A later, more specific rule cannot act on a path an earlier rule already saw. With
  overlapping scan roots, put the rule you want to win first, or do not overlap them.

Longest-path matching (post-hash MyList parameters, rename destination, `auto_remove`):

1. Only rules with a non-empty `path` are candidates. Paths are compared after turning `\` into
   `/`, trimming trailing separators and (on Windows) lowercasing.
2. A rule with `include_subdirs` matches anything below its folder; without it, only files directly
   inside. The network rule matches every `//...` path.
3. Of several matching rules, the one with the **longest path** wins.
4. If none matches, the rule flagged `default` is used (the first flagged one; if none is flagged,
   the first entry). The fallback is used for MyList parameters and rename destination, but
   **never** for automatic removal.

On load, if no entry is flagged `default`, the built-in "Default" entry is inserted at position 0.
An explicit empty `managers` array therefore yields just that entry; the network entry is not
re-added. The code assumes at least one rule exists; the built-in entries cannot be removed in
Options, but a rule you flagged `default` with `can_edit_path: true` can be, so keep the built-in
"Default" entry in files you edit by hand (source reading; this exact sequence was not exercised).

## External players (`players`)

Array of objects, default `[]`. Options lists every player with Name, Executable, Open arguments,
Play arguments, Enqueue arguments and "Default player" plus a "Remove player" button, and an "Add
player" button below the list. Each object accepts any subset of keys.

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `name` | string | `""` | Name | **Active.** Shown in the player chooser on Hashed files when more than one player is configured; an empty name appears as "Player N" (its position in the list). |
| `path` | string | `""` | Executable | **Active.** Program started by every player action. Empty is rejected with "player executable is not configured". |
| `play` | string | `""` | Play arguments | **Active.** Argument template for Play. Always offered; an empty template just passes the file. |
| `open` | string | `""` | Open arguments | **Active.** Argument template for **Open in player** (`PlayerAction::Open`). The action is offered only while this is not blank. |
| `enqueue` | string | `""` | Enqueue arguments | **Active.** Argument template for **Enqueue** (`PlayerAction::Enqueue`). The action is offered only while this is not blank. |
| `default` | boolean | `false` | "Default player" | **Active.** The player Play uses until you pick another in the chooser is the first with `default: true`, otherwise the first in the array. Ticking the control makes that player the only default. |

Argument templates are described under [External command templates](#external-command-templates).

## Statistics export (`stat_whore`)

Object `stat_whore` (the AOM name for the statistics feature).

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `enabled` | boolean | `false` | "Write statistics after library updates" | **Active.** With a non-empty `file`, the statistics file is rewritten every time the library view reloads in the background (after hashing, queue activity, imports and similar events). |
| `format` | `"Xml"` or `"Ini"` | `"Xml"` | Format combo | **Active.** Format of the automatic file and of the suggested type for File → Export statistics…. Values are case-sensitive. |
| `file` | string | `""` | Output file | **Active.** Target of the automatic export; missing parent folders are created and the file is replaced atomically (temporary file, flush, rename), so a failed write keeps the previous file. |

File → Export statistics… always asks for a path and ignores `enabled` and `file`. Both exports
include the AniDB username (not the password) in the `anidb` block; treat exported statistics as
personal data. Failures appear in the Log and status bar and do not stop the application.

## AVDump (`avdump`)

Object `avdump`.

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `enabled` | boolean | `false` | "Enable AVDump integration" | **Active.** Enables the AVDump button on Hashed files (also checked again before launching). |
| `executable` | string | `""` | Executable | **Active.** Program to start (for example an AVDump command-line build). Empty is rejected. |
| `arguments` | string | `--Consumers=ED2K --Auth={user}:{key} --LPort={port} --PrintEd2kLink "{path}"` | Arguments | **Active.** Template, see below. |
| `key` | string | `""` | "AVDump API key" | **Active.** Value of `{key}`. Kept in the system keystore, or in clear text on disk when none is available ([Credentials](Settings-Files.md#credentials-and-the-system-keystore)); separate from `anidb.api_key`. |

Animatic only starts the program for the selected file and does not read its output or wait for it.

## Media properties (`media`)

Object `media`. Optional, off by default; see [External tools](External-Tools.md#media-properties-ffprobe)
for the workflow and the install.

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `read_properties` | boolean | `false` | "Read media properties after hashing" | **Active.** When true and `ffprobe` is available, a separate background step after each hashing job reads duration, resolution, codecs and bitrates of the files that job hashed and stores them for the [hash export](#hash-export-template-tokens). The control is disabled until `ffprobe` is found; the stored value is kept but nothing is read while it is missing. |
| `ffprobe_path` | string | `""` | "ffprobe path" | **Active.** An `ffprobe` executable, or the folder that holds it. Empty: Animatic's own copy in `tools/ffprobe/` of the [data folder](Data-Locations.md) and `ffprobe` on `PATH` (own copy first on Windows, `PATH` first on Linux). A path that is not usable is reported in Options and the search goes on. |

A file without a `media` object (every earlier version's file) loads with both values at their
defaults. Applying an AOM settings import keeps your current `media` object because AOM has no
such preference. An older Animatic build ignores and drops the unknown `media` key on its next save.

## Top-level keys

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `renaming_pattern` | string | the stable built-in pattern (33 lines, `crates/animatic-core/src/rename/default_pattern.txt`) | "Renaming pattern" editor | **Active.** Pattern used by the rename preview. The editor classifies it as Empty, Stable default or Custom, reports syntax errors, offers "Use stable default", "Restore previous text" and "Validate on selected file". Saving does not validate the pattern; an empty or invalid pattern simply blocks renames. Custom text is never modified by the application. Language: [Rename language](Rename-Language.md). |
| `ed2k_space_replacement` | string or `null` | `null` | none | **Stored only.** No consumer; ED2K links are always built with the file name as is. |
| `first_launch` | boolean | `true` | "Dismiss first-run card" on Overview | **Active.** While true the Overview page shows the first-run card; dismissing sets it false and saves. After an AOM settings import it is set to whether the imported username is empty. |
| `secrets_in_keystore` | boolean | `false` | none (shown as a notice in Options) | **Active, written by Animatic.** `true` after a save that moved `anidb.password`, `anidb.api_key` and `avdump.key` into the operating-system keystore and stored them empty here; loading then fills empty credential fields from the keystore. Do not edit by hand; a file with real values in those keys and `true` uses the file's values. See [Credentials and the system keystore](Settings-Files.md#credentials-and-the-system-keystore). |

## Updates

Object `updates`. It controls only the *automatic* check for a newer Animatic release on GitHub.
Checking, downloading and installing are always started by you; see
[Updating Animatic](Updating-Animatic.md) for the flow and
[Storage and network](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#github-update-transport) for the traffic.

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `auto_check` | boolean | `true` | "Check for updates at startup" | **Active.** When true, one background check for a newer stable GitHub release runs at startup, but only if `anidb.offline` is false and the settings were not recovered from a backup. It never downloads or installs anything. When false, no request is made until you choose **Help → Check for updates…**, which works regardless of this key and of `anidb.offline`. |

There is no automatic install, no last-check timestamp, no skipped-version list and no
release channel setting. A file without an `updates` object (every earlier version's file) loads
with `auto_check` true, so upgrading to this version turns the startup check on unless you set it
to `false`. An older Animatic build ignores and drops the unknown `updates` key on its next save.
Applying an AOM settings import keeps your current `updates` object because AOM has no such preference.

## Hash export template tokens

Used by **File → Export hashes…** (it exports every hashed file in the library, in list order, to
the file you choose, default name `animatic-hashes.txt`; built and written on the background projection thread, replacing the file atomically). `hashing.export_template` is expanded once
per file and followed by a CRLF line ending. An empty template is treated as `%fulled2k%`; `\n`
inside the template is written as CRLF. There is no automatic export and no other UI entry for this
feature.

Tokens are replaced one after another in the order below over the whole text (replaced text is not
escaped, so a file name that itself contains a later token would be expanded too).

| Token | Real data? | Value |
|---|---|---|
| `%filename%` | yes | File name without folders |
| `%filepath%` | yes | Full stored path |
| `%ed2k%` | yes | ED2K hash, lowercase hex |
| `%md5%` | yes | MD5, uppercase hex (empty if not stored) |
| `%sha1%` | yes | SHA-1, uppercase hex (empty if not stored) |
| `%crc32%` | yes | CRC32, uppercase, 8 digits (empty if not stored) |
| `%size%` | yes | Human-readable binary size such as `1.23 GB` or `512 bytes` (not a byte count) |
| `%fulled2k%` | yes | ED2K link, see below |
| `%fps%` | optional | Video frames per second with two decimals; `0.00` without media data |
| `%length%` | optional | Duration in whole seconds, or whole minutes when "Export %length% in minutes" is on; without media data empty (seconds) or `0` (minutes) |
| `%height%` | optional | Video height in pixels; `0` without media data |
| `%width%` | optional | Video width in pixels; `0` without media data |
| `%vcodec%` | optional | Video codec name from `ffprobe`; empty without media data |
| `%vbitrate%` | optional | Video bitrate in kbit/s with two decimals; `0.00` without media data |
| `%acodec%` | optional | Audio codec name from `ffprobe`; empty without media data |
| `%asample%` | optional | Audio sample rate in Hz; `0` without media data |
| `%abitrate%` | optional | Audio bitrate in kbit/s with two decimals; `0.00` without media data |

"Optional" tokens carry real data only for files whose media properties were read with the
optional `ffprobe` tool ([Media properties](#media-properties-media)); every other file, and every
value `ffprobe` did not report, keeps the constant shown, so AOM templates still expand. The ED2K
link has this shape (the size is in bytes):

```text
ed2k://|file|<file name>|<size in bytes>|<ed2k hash>|/
```

## External command templates

Applies to `players[].play` / `open` / `enqueue` and `avdump.arguments`. No shell is involved: the
program in `path` / `executable` is started directly with an argument list.

1. The template is split into arguments with POSIX-style quoting (the `shell-words` crate). An
   unterminated quote is an error and nothing is started.
2. Placeholders are then replaced inside each argument:
   - Players: `{path}` or `%file%` become the full media path.
   - AVDump: `{path}`, `{user}` (`anidb.username`), `{key}` (`avdump.key`) and `{port}`
     (`anidb.local_port`).
3. For players only: if the template contains neither `{path}` nor `%file%`, the path is appended
   as one separate, unparsed last argument.

Practical notes that follow from this order:

- **A substituted value always stays inside the argument that held its placeholder.** Spaces,
  quotes, backslashes or option-like text (`--x`) inside a file name, username or key are passed
  through unchanged and can never split into extra arguments. A value is not scanned again, so a
  file name that happens to contain `{key}` stays literal.
- Quoting a placeholder is not needed for the value, only for template text around it:
  `--title="Now playing {path}"` is one argument.
- Backslashes and quotes typed into the *template itself* follow the quoting rules: an unquoted
  `\` is an escape character, so write literal Windows paths in a template inside double quotes.
  This never affects substituted values.
- `{key}` and `{user}` become part of the AVDump process's command line, which other local
  programs can usually see in the process list. The default template places them there.
- AVDump's template has no automatic path argument: if `{path}` is missing nothing is appended.

## Options controls and keys at a glance

Controls that do nothing: **Auto rename** (per folder). Keys that work but have no control:
`anidb.udp_server`, `local_port`, per-folder `exclude_hidden`, `can_edit_path`
and `default`, extra `players` entries, the window and
column state (`ui.window`, `ui.maximized`, `ui.columns`, changed by using the window) and the row
colors (`ui.colors`). Keys that exist only for AOM compatibility and are never read: everything
marked **Stored only** or **Import only** above.
