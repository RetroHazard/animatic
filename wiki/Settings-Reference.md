Animatic keeps all your settings in one JSON document, `settings.json`. This page lists every
key of that document with its type, default, the Options control that edits it (if any), and what
the application actually does with the value. Use it when you want to know what a control stores,
when you must edit the file by hand (for the few keys Options does not show), or when you are
reading a settings file someone gave you.

**A key that exists in the file is not necessarily a feature.** The settings model was built to
round-trip AniDB O'Matic (AOM) configuration, so several keys are stored but never read by the
desktop application. Every key below carries a status from the legend, so you can tell a working
option from a placeholder before relying on it.

Where the file lives, the `ANIMATIC_SETTINGS` / `ANIMATIC_DB` overrides and the command-line flags
are described in [Data locations](Data-Locations.md) and [Running Animatic](Running-Animatic.md).
The controls themselves, with pictures, are in the [Options window](Options-Window.md). Day-to-day
tasks (hashing, scanning, MyList edits, exports) are in the
[user guide pages](Home.md#all-pages). How the file is loaded, saved, backed up and recovered
is in [Settings files](Settings-Files.md); recovery steps and support guidance are in
[Troubleshooting](Troubleshooting.md).

Defaults come from the program's built-in settings. The statuses below come from reading the
program, not from new test runs: "Stored only" means nothing in the application reads the value.

## Status legend

| Status | Meaning |
|---|---|
| **Active** | The value is read at runtime and changes behavior. |
| **Active, JSON only** | Read at runtime, but Options has no control; edit the file (with Animatic closed) or import it. |
| **Partly active** | Read, but only by some paths; the effect column says which. |
| **Stored only** | Saved and loaded, no Options control, no consumer. |
| **Import only** | Stored only, and additionally filled in by the AOM settings importer. |

Options is **Tools → Options…** (`Ctrl+,`); the [Options window](Options-Window.md) page describes every control. Its
categories, in order: AniDB account, Appearance (including System tray), Managed folders, Hashing,
Renaming, Players and tools (External players, AVDump, Media properties), Exports (Hash export,
Statistics export) and Updates (which also lists the MyList state names read-only).

## Document layout

The root of the file must be a JSON object. Keys use `snake_case`, exactly as the tables spell them.

| Root key | Type | Default | Section |
|---|---|---|---|
| `anidb` | object | see [AniDB account](#anidb-account) | Account, client identity, transport, offline mode |
| `hashing` | object | see [Hashing](#hashing) | Extension filter, hash export, post-hash identification |
| `ui` | object | see [UI](#ui) | Theme, scale, colors, column and window state |
| `players` | array of objects | `[]` | [External players](#external-players-players) |
| `file_managers` | object | two built-in entries | [Managed folders](#managed-folders-file_managers) |
| `renaming_pattern` | string | the [Group] Title - episode [CRC] convention | [Top-level keys](#top-level-keys) |
| `stat_whore` | object | disabled | [Statistics export](#statistics-export-stat_whore) |
| `avdump` | object | disabled | [AVDump](#avdump-avdump) |
| `ed2k_space_replacement` | string or `null` | `null` | [Top-level keys](#top-level-keys) |
| `first_launch` | boolean | `true` | [Top-level keys](#top-level-keys) |
| `secrets_in_keystore` | boolean | `false` | [Top-level keys](#top-level-keys) |
| `updates` | object | `{ "auto_check": true, "channel": "stable" }` | [Updates](#updates) |
| `media` | object | `{ "read_properties": false, "ffprobe_path": "" }` | [Media properties](#media-properties-media) |

### Editing the file by hand

Almost everything is better changed in Options. Edit the file only for the keys Options does not
show (`anidb.udp_server`, `anidb.local_port`, `anidb.image_server`, and the like).

1. Quit Animatic. It writes the whole file when it quits, so an edit made while it runs is overwritten.
2. Make a copy of `settings.json` somewhere safe. Do not post it anywhere: it can hold your credentials
   when no system keystore is available.
3. Open the original in a plain-text editor that keeps UTF-8. Change the value, keeping the JSON type
   shown in the tables (strings in quotes, numbers without quotes, `true` / `false` in lower case).
4. Save and start Animatic.

A single wrong type, an unknown spelling of `theme` or `format`, or a number out of range for its type
makes the **whole file invalid**; Animatic then falls back to the backup copy, offline. Unknown keys
are ignored and dropped on the next save. See
[Omitted keys, unknown keys and wrong types](Settings-Files.md#omitted-keys-unknown-keys-and-wrong-types).

## AniDB account

Object `anidb`. See [Storage and network contracts](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md) for what the queue sends
and how failures are handled. Animatic contacts **AniDB** only when the command queue runs (when you
start it with **Tools → Run command queue** (`Ctrl+Shift+S`) or the **Run command queue** button on the Queue page, or, if you switch on `auto_run_queue`,
when Animatic starts a run itself, see
[Automatic runs](AniDB-Synchronization.md#automatic-runs-and-the-anidb-session)) and when an anime
detail window needs a cover picture that is not on disk yet. A manual run receives a copy of this
object when you start it, so edits apply from the next run; an automatic run uses the values from
when Options was last closed. (Update checks contact GitHub, not AniDB; see [Updates](#updates).)

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `username` | string | `""` | Username | **Active.** UDP login; `{user}` in AVDump arguments; written into statistics exports. |
| `password` | string | `""` | Password (masked) | **Active.** UDP login. Kept in the system keystore and stored empty in the file; clear text in the file only while no keystore is available, see [Credentials](Settings-Files.md#credentials-and-the-system-keystore). |
| `api_key` | string | `""` | "API key (optional)" | **Active.** When non-empty the UDP session is encrypted with this key (profile UDP API key on anidb.net). Empty means an unencrypted session. Stored like `password`. |
| `udp_server` | string | `api.anidb.net:9000` | none | **Active, JSON only.** `host:port` of the UDP API. |
| `local_port` | integer (u16) | `29456` | none | **Active, JSON only.** Local UDP port to bind (all interfaces). AniDB bans clients that use many different source ports, so Animatic uses exactly this port and **never falls back to another one**: if it cannot be bound (another program uses it) the run stops with an error naming `anidb.local_port`, nothing is sent and the commands stay queued; free the port or choose another above 1024. Also passed to AVDump as `{port}`. `0` asks the operating system for a port, a different one every run, which is meant for tests and risks that ban. |
| `image_server` | string | `cdn.anidb.net` | none | **Active, JSON only** (imported from AOM). Host that anime covers are downloaded from (`https://<host>/images/main/<picture>`), used until a login has told Animatic which image host AniDB wants used; after that the host AniDB announced at login is used and this value is only the fallback. A host name with an optional port; anything else (a path, a scheme, spaces) makes the cover download stop with an error. |
| `download_covers` | boolean | `true` | "Download anime covers when a detail window opens" | **Active.** When true, opening an anime's detail window downloads its cover once if it is not on disk yet (never offline, never during a ban or cooldown; a failed download is not retried for an hour); see [Cover](Library-Pages.md#cover). When false nothing is downloaded, and covers already in the [covers folder](Data-Locations.md#cover-pictures) are still shown. Files without the key load as `true`. |
| `update_file_information` | boolean | `false` | "Update known file information" | **Active.** When true, hashing a file that is already identified (it has an AniDB file id) again queues a refresh of its AniDB record (`Fetch file`), at most once per file per 7 days. With the default `false`, re-hashing a known file queues a refresh only through the normal post-hash `Identify` (when `identify_after_hashing` is on and the matching rule has `mylist_auto_add`); otherwise it queues nothing. Unidentified files follow *Identify after hashing*. (Import fills it from AOM.) |
| `offline` | boolean | `false` | "Work offline (keep commands queued)" (AniDB account → Synchronization); also the **Online** / **Offline** button in the status bar | **Active.** When true, Run command queue sends nothing and reports "Offline mode is enabled; commands remain queued", and no cover is downloaded (covers already on disk are still shown). Commands are still queued locally. The status bar shows a button labelled **Offline** (amber dot) or **Online** (green dot), and clicking it toggles the setting. Forced to `true` by backup recovery and by applying imported AOM settings. It also suppresses the automatic startup [update check](#updates); an explicit **Help → Check for updates…** is still allowed. |
| `idle_logout_minutes` | integer (u32) | `10` | "Log out of AniDB when idle for … min" (enabled only with automatic runs) | **Active, with `auto_run_queue`.** While automatic runs keep one AniDB session between runs, the client logs out after this many minutes without a command (unless more commands are waiting for an automatic run). Kept from 1 to 30 on load, so a session is always logged out before AniDB expires it after 35 idle minutes. Without `auto_run_queue` it has no effect: every run logs out when it ends. |
| `auto_run_queue` | boolean | `false` | "Run the command queue automatically" | **Active.** Off: nothing is sent unless you start a run. On: Animatic starts a queue run 30 seconds after new commands are queued and every 15 minutes for anything still waiting, and keeps one AniDB login between runs. Never while **Work offline** is on, an AniDB cooldown is active, the account is not set, another task is running or an import/restore review is open. Blocked commands are not retried, and **MyList removals are never sent automatically**: they stay queued until you run the queue yourself. Not imported from AOM. |

The AniDB client identifiers are not settings: Animatic always sends its own registered clients,
`animatic` version 1 on the UDP API and `animatichttp` version 1 on the HTTP API. Older settings
files may still contain `client_name`, `client_version`, `http_client_name` and
`http_client_version` in `anidb`; they are ignored and are dropped the next time the settings are
saved. See [AniDB client registration](First-Launch.md#anidb-client-registration).

## Hashing

Object `hashing`.

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `autostart` | boolean | `true` | "Start hashing automatically" | **Active.** When Animatic starts with files still in the [persistent hash queue](Hashing-and-Identification.md#hash-queue), hashing resumes by itself once nothing blocks it (an import review, a restore, the settings recovery notice or a running task). With `false` nothing hashes at start; the Hashed files page and Overview show the pending count with a **Resume hashing** button. Read at start, so a change applies at the next start. (AOM import sets it from `autostartHashing`, default `false` when absent.) |
| `priority` | integer (u8); AOM used 0-6 | `1` | none | **Stored only** (kept for AOM compatibility, no effect). Legacy thread-priority index (0 idle to 6 time critical). Hashing always runs at the normal thread priority, because there is no cross-platform thread-priority control; the importer clamps the value to 6. The file accepts any 0-255; 256 or more makes the whole file invalid. |
| `use_extensions` | boolean | `false` | "Restrict file extensions" | **Partly active.** Only applies a file-type filter to the **Add files…** picker. It does not filter files given on the command line, and it does not switch scanning on or off (scans always use `extensions`; see below). |
| `extensions` | array of strings | the 52 legacy AOM extensions (`7z`, `aac`, `ac3` … `zip`) | multi-line box, comma separated | **Active.** Scan allow-list, see below. Add files picker filter when `use_extensions` is true. |
| `export_template` | string | `%fulled2k%` | "Hash export" (**Options → Exports**) | **Active.** Template for **File → Export → Hashes…**; see [Hash export tokens](#hash-export-template-tokens). |
| `integrate_into_shell` | boolean | `false` | "Add "Hash with Animatic" to the file menu" | **Active.** **Save options** registers a per-user file-menu entry when it is `true` and removes an entry Animatic created when it is `false`. The value alone never registers anything: loading settings and AOM import (`general/integrateIntoShell`) only store it, and Options shows whether the entry is really present. See [File-menu entry](Options-Window.md#file-menu-entry). |
| `identify_after_hashing` | boolean | `true` | "Identify files with AniDB after hashing" | **Active.** After each file is hashed (Add files, command line, or scan), the UI queues an `Identify` command. If the file's [managed-folder rule](#how-a-file-picks-its-rule) has `mylist_auto_add`, the command carries that rule's MyList parameters. Offline, the command only waits in the queue. |

How the extension list is used:

- **Scans** (**Tools → Scan managed folders**) always use `extensions`, regardless of
  `use_extensions`. Entries are split on comma, semicolon or space, a leading `.` is removed and
  matching is case-insensitive. A file without an extension never matches, and an **empty list
  means a scan hashes nothing**.
- **Add files…** uses the entries verbatim as dialog filter names, only when `use_extensions` is
  true. Write entries without a leading dot.
- Files passed on the command line are hashed without any extension check.
- The Options text box splits on commas only; the file may hold any strings, but use plain
  extensions.

The built-in list, in one line:

```text
7z,aac,ac3,ace,asf,ass,avi,dts,dtshd,flac,flv,idx,js,lrc,m2ts,m4a,mk3d,mka,mks,mkv,mov,mp3,mp4,mpeg,mpg,ogg,ogm,ogv,pjs,qt,ra,rar,rm,rmvb,rt,smi,smil,srt,ssa,sub,swf,thd,tmp,ts,tts,txt,wav,webm,wma,wmv,xss,zip
```

It is a legacy AOM list, not a media-only list.

The ED2K variant is fixed (the "red" variant AniDB expects); there is no setting for it.

## UI

Object `ui`. Theme, color scheme, scale, window geometry, column layouts and status row colors are applied by
the desktop application. `tree_auto_expand`, `tree_auto_collapse`, `show_local_folder_size` and `hide_root_folder` mirror AOM interface options and are **not** read; all other keys below are.

### Appearance and paths

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `theme` | `"Dark"`, `"Light"` or `"System"` | `"Dark"` | "Theme" combo (Dark, Light, "Same as the system") | **Active.** Applied at startup, at once when you choose it in Options, and by **Save options** or **Apply imported settings**. `System` follows the operating system's light or dark mode and changes with it while Animatic runs (where the platform does not report a mode, it is dark). Values are case-sensitive. |
| `color_scheme` | `"Sakura"`, `"Ocean"`, `"Forest"`, `"Sunset"`, `"Wisteria"` or `"Graphite"` | `"Sakura"` | "Color scheme" combo (each entry with a swatch of its accent) | **Active.** The accent family of the interface, used with the dark or the light palette that `theme` picks; applied at startup, at once when you choose it in Options, and by **Save options** or **Apply imported settings** (an AOM import keeps the scheme you chose, since AOM has no such setting). A missing key or a name this version does not know loads as `"Sakura"` without affecting the rest of the file. Every scheme is checked to keep body text at 7:1 contrast or better and links, accent text and selected text at 4.5:1. |
| `zoom` | number (f32) | `1.0` | "UI scale" slider 0.75-2.0 | **Active.** Applied (clamped to 0.75-2.0) at startup and at the same moments as `theme`. Load-time fixup resets values `<= 0.3` to `1.0`; other out-of-range values are kept in the file and clamped when applied. |
| `last_directory` | string | `""` | none | **Active.** Start folder for the Add files picker and the AOM settings XML picker; updated to the folder of the files you last added. |
| `colored_rows` | boolean | `true` | "Tint table rows by status" (Status colors) | **Active.** Tints table rows by status with the [row colors](#row-colors-uicolors) below; `false` draws every row plainly. Every status is also written in the row, so the color is never the only signal. Takes effect immediately, no save needed. |
| `row_style` | `"Soft"` or `"Solid"` | `"Soft"` | "Row color style" | **Active** while `colored_rows` is on. `Soft` (shown as "Soft (tint and marker)") blends the status color into the page background and keeps the theme's text colors, with a 3-point bar in the full status color at the left edge of the row; statuses that ask for action (unknown, CRC mismatch, duplicate) get a stronger tint than the expected ones (watched, in MyList), and the tint is weakened wherever the text would drop below a 4.5 contrast ratio. `Solid` ("Solid (classic AOM)") fills the whole row with the full status color and black or white text, like the legacy client. A slot with its own `text` color in `colors` is always drawn Solid. A file without `row_style` (every earlier version's file) loads as `Soft`; an older build ignores and drops the key on its next save. Values are case-sensitive. Takes effect immediately. |
| `copy_length_as_minutes` | boolean | `false` | "Export %length% in minutes" | **Active.** Read by hash export: `%length%` is the duration in whole minutes instead of seconds. Without [media data](#media-properties-media) `%length%` keeps its constant: empty in seconds mode, `0` in minutes mode. See [token table](#hash-export-template-tokens). |
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
| `mylist_table` | MyList | `anime`, `episode`, `group`, `state`, `watched`, `size`, `queue` |
| `local_table` | Hashed files | `file`, `size`, `anidb`, `mylist`, `crc`, `path` |
| `groups` | Groups | `name`, `short_name`, `rating`, `votes`, `website`, `refresh` |
| `genres` | Genres | `genre`, `anime` |
| `queue_table` | Queue | `id`, `command`, `status`, `last_error` (no sorting) |

### Row colors (`ui.colors`)

`colors` is an object whose keys are color-slot names and whose values are
`{ "background": number, "text": number or null }`. Numbers are `0xRRGGBB` written as decimal
integers (JSON has no hex literals); `text` may be omitted or `null` (no text override), but
`background` is required in every entry. An absent slot uses the default background below.
With `colored_rows` on, the table rows take these colors. **Options → Appearance → Customize
status colors** edits the eight slots the tables use (listed under "Where Animatic uses a slot"
below): a background color, an optional text color ("Text" ticked) and a sample row drawn like a
table row in the current `row_style` and theme, per slot, plus a per-slot **Reset** and
**Reset all** (which touch only those eight slots). Edits apply at once and **Save options**
writes them; choosing a slot's default background with no text color removes its entry. A chosen
text color whose contrast on its background is below 4.5 shows a yellow note and is kept anyway
(a soft tint is weakened by the painter until the theme text reads, and the automatic black or
white text always reaches 4.5, so only a chosen text color can warn). The other slots are not in
Options; an AOM import fills them. The
background is the status color of the `Solid` row style, and in the
default `Soft` style it is blended into the page background and drawn as the left marker bar.
Without a `text` color a solid row's text is black or white, whichever contrasts
more with the background; the defaults all reach a contrast ratio of at least 4.5. A slot that
has a `text` color is always drawn as a solid row, because you chose that pair. Slots marked
*plain* describe an unremarkable row and are only drawn when you set them, because their default
light grays would glare in the dark theme. Unknown slot names are not rejected and are kept on
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
| | AniDB `Not identified` | `LocalFileUnknown` |
| | MyList `Watched · lid N` | `Watched` |
| | MyList `In MyList · lid N` | `LocalFileInMylist` |
| | otherwise | `LocalFile` (plain) |
| Anime, Advanced search | MyList `... · all watched` | `Watched` |
| | MyList `N eps / M files` (Anime) or `N eps / M watched` (Advanced search) | `AnimeInMylist` |
| | otherwise | `Anime` (plain) |
| MyList | Watched checkbox ticked | `Watched` |
| | otherwise | `File` (plain) |

The other slots (`LocalRoot`, `LocalFolder`, `LocalFileDuplicate`, `Episode`, `EpisodeInMylist`,
`FileCrcOk`, `FileCrcInvalid`) are stored for AOM compatibility and no table uses them yet.

Importing converts AOM's color integers (Windows BGR order) to RGB; an AOM text color of `0` means
"no override". An entry equal to the default background with no text color is dropped.

## Managed folders (`file_managers`)

Object `file_managers`: global scan timing values plus a list of per-folder rules ("file managers",
the AOM term). Folder rules drive **Tools → Scan managed folders**, the optional scan at start and
scheduled scan, the MyList parameters sent with identified files, and the rename destination. A
scan runs only when you start it or when one of the scan automation settings below is on; see
[Scheduled scans](Managed-Folders-and-Scans.md#scheduled-scans) and
[Settle delay and pacing](Managed-Folders-and-Scans.md#settle-delay-and-pacing).

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `action_delay_ms` | integer (u64) | `500` | "Hash a file once it is unchanged for" | **Active.** Settle delay of a scan: a file is hashed only once its modification time is at least this old and its size and modification time stay unchanged for this long; otherwise it is deferred to the next scan. `0` hashes at once. Load-time fixup limits values to 0-600000 (values above are lowered to 600000). |
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
| `name` | string | `""` | Name | Display label of the rule in Options and in the automatic-rename confirmation and log lines. With an empty name the Options list reads `Unnamed folder` and the confirmation shows the path. |
| `path` | string | `""` | Path (with "Browse…") | **Active.** Folder the rule applies to. Empty paths never match a file and are skipped by scans. |
| `include_subdirs` | boolean | `true` | "Include subfolders" | **Active.** Scan depth (all levels or direct children only) and whether files below the folder match this rule. |
| `auto_hash` | boolean | `false` | "Hash new files" | **Active in scans.** A scan hashes files that are not yet in the database. The scan itself writes no MyList data, but each hashed file raises the same post-hash event as Add files, so with `identify_after_hashing` and the matching rule's `mylist_auto_add` it queues an `Identify` that adds the file to MyList on AniDB when the queue next runs online. |
| `auto_rehash` | boolean | `false` | "Rehash changed files" | **Active in scans.** A scan rehashes known files whose modification time (whole seconds) differs from the stored value. |
| `auto_remove` | boolean | `false` | "Remove missing files from the library" | **Active in scans.** A scan removes the local file record (and any hash record no longer used by another local file) of a file that is positively missing (not-found), provided a rule matches its path by an explicit path match (no fallback rule) and the rule's folder is a reachable directory. Never deletes media files. The removal itself writes only those local tables. |
| `auto_rename` | string (`"off"`, `"dry_run"`, `"on"`) | `"off"` | "Auto rename" selector | **Active.** Opt-in automatic rename of identified files inside this folder (and below it when `include_subdirs` is on), see [Automatic rename](Renaming-Files.md#automatic-rename-opt-in-per-managed-folder). `off`: never. `dry_run`: write the planned renames to the Log, move nothing. `on`: rename in a background job that verifies each file; skip and log collisions, never overwrite. Options asks you to confirm a dry-run preview whenever you switch a folder to On. **Compatibility:** earlier versions stored a boolean here (and an AOM settings import still reads AOM's flag as one). `false` loads as `off` and `true` loads as `dry_run`, deliberately **never `on`**, so no file is renamed until you choose On yourself; the key is rewritten as a string on the next save. Only rules with a non-empty `path` count; the built-in Default fallback never renames. |
| `base_path` | string | `""` | "Move renamed files to another folder" and its "Destination" | **Active.** Folder used as the rename target base and exposed to rename patterns as `BASEPATH`; empty keeps the file's current folder (the checkbox shows ticked while it is not empty, and unticking it clears the value). See [Rename language](Rename-Language.md). |
| `exclude_hidden` | boolean | `true` | none | **Active.** Scans skip entries (files and folders) whose name starts with `.`. The OS "hidden" attribute is not consulted. |
| `exclude_system` | boolean | `true` | "Skip system files" | **Active on Windows.** Scans skip files and folders that have the Windows system attribute (a skipped folder is not entered). **No effect on Linux**, which has no such attribute. |
| `clear_empty_folders` | boolean | `false` | "Remove folders a rename leaves empty" | **Active.** After a manual, batch or automatic rename moves a file, the folders it left are removed while they are strictly below this rule's folder and completely empty (hidden files keep a folder). The rule's own folder is never removed and nothing is deleted recursively. See [Removing folders a rename emptied](Managed-Folders-and-Scans.md#removing-folders-a-rename-emptied). |
| `mylist_auto_add` | boolean | `false` | "Add identified files to MyList" | **Active.** When true and `identify_after_hashing` is on, the post-hash `Identify` command (queued after Add files, command-line hashing or a scan) carries the MyList parameters below, so the file is **added to your MyList on AniDB when the queue runs online**. Offline, the command only waits locally. |
| `mylist_watched_check` | boolean | `false` | "Set watched" | **Active.** Gates `mylist_watched`: when false the watched flag is not sent. |
| `mylist_watched` | boolean | `false` | "Watched" (shown when the check is on) | **Active.** Watched flag sent when the check is on; `true` also sends the current time as view date. |
| `mylist_state_check` | boolean | `true` | "Set state" | **Active.** Gates `mylist_state`. |
| `mylist_state` | integer (i64) | `1` | state combo (greyed out while the check is off) | **Active.** One of the labels in `Unknown`=0, `On HDD`=1, `On CD/DVD`=2, `Deleted`=3, `Remote/Shared`=4. Other integers are accepted by the file and sent as written. |
| `mylist_storage_check` | boolean | `false` | "Set storage" | **Active.** Gates `mylist_storage`. |
| `mylist_storage` | string | `""` | Storage | **Active.** Sent when its check is on (an empty string is sent as an empty value). |
| `mylist_source_check` | boolean | `false` | "Set source" | **Active.** Gates `mylist_source`. |
| `mylist_source` | string | `""` | Source | **Active** when its check is on. |
| `mylist_other_check` | boolean | `false` | "Set other" | **Active.** Gates `mylist_other`. |
| `mylist_other` | string | `""` | Other | **Active** when its check is on. |
| `can_edit_path` | boolean | `true` | none | **Active.** `false` on the two built-in rules. Together with `path` `\\` it marks the built-in network rule, whose path Options shows no field for. Any rule can be removed in Options while another remains. |
| `default` | boolean | `false` | none | **Active.** Marks the Default (fallback) rule, see below. Options cannot change it; it hides the folder and scan controls of that rule. |

"Add folder rule" creates an entry named "New folder" using the omitted-key defaults above, which
means it has `include_subdirs`, `exclude_hidden`, `exclude_system` and `mylist_state_check` on.

### Built-in entries

A fresh settings file contains two rules:

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

Scan discovery (**Tools → Scan managed folders**):

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
4. If none matches, the rule flagged `default` is used (the first flagged one). Without such a rule
   a neutral policy applies: nothing is added to MyList and renamed files stay in their folder;
   another folder's rule is never used. The fallback is used for MyList parameters and rename
   destination, but **never** for automatic removal.

Options can remove any rule, the built-in ones included, but never the last one; **Restore
built-in rules** adds a missing built-in rule back with the values above. A removed Default rule
stays removed when the file is loaded again. Only an explicit empty `managers` array is repaired on
load: it yields the built-in "Default" entry alone.

## External players (`players`)

Array of objects, default `[]`. Options lists every player with Name, Executable, Open arguments,
Play arguments, Enqueue arguments and "Default player" plus a "Remove player" button, and an "Add
player" button below the list. Each object accepts any subset of keys.

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `name` | string | `""` | Name | **Active.** Shown in the player chooser on Hashed files when more than one player is configured; an empty name appears as "Player N" (its position in the list). |
| `path` | string | `""` | Executable | **Active.** Program started by every player action. Empty is rejected with "player executable is not configured". |
| `play` | string | `""` | Play arguments | **Active.** Argument template for Play. Always offered; an empty template just passes the file. |
| `open` | string | `""` | Open arguments | **Active.** Argument template for **Open in player**. The action is offered only while this is not blank. |
| `enqueue` | string | `""` | Enqueue arguments | **Active.** Argument template for **Enqueue**. The action is offered only while this is not blank. |
| `default` | boolean | `false` | "Default player" | **Active.** The player Play uses until you pick another in the chooser is the first with `default: true`, otherwise the first in the array. Ticking the control makes that player the only default. |

Argument templates are described under [External command templates](#external-command-templates).

## Statistics export (`stat_whore`)

Object `stat_whore` (the AOM name for the statistics feature).

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `enabled` | boolean | `false` | "Write statistics after library updates" | **Active.** With a non-empty `file`, the statistics file is rewritten every time the library view reloads in the background (after hashing, queue activity, imports and similar events). |
| `format` | `"Xml"` or `"Ini"` | `"Xml"` | Format combo | **Active.** Format of the automatic file and of the suggested type for **File → Export → Statistics…**. Values are case-sensitive. |
| `file` | string | `""` | Output file | **Active.** Target of the automatic export; missing parent folders are created and the file is replaced atomically (temporary file, flush, rename), so a failed write keeps the previous file. |

**File → Export → Statistics…** always asks for a path and ignores `enabled` and `file`. Both exports
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
| `renaming_pattern` | string | the `[Group] Title - episode [CRC]` convention (the first built-in naming convention); settings saved earlier keep their pattern | **Options → Renaming**: Naming convention list and custom editor | **Active.** Pattern used by every rename. Only the text is stored: a text equal to a built-in convention shows as that convention, anything else as Custom pattern. The section reports syntax errors, shows an example name, offers the Pattern reference, "Restore my custom pattern" and "Validate on selected file". Saving does not validate the pattern; an empty or invalid pattern simply blocks renames. Custom text is never modified by the application. Language: [Rename language](Rename-Language.md). |
| `ed2k_space_replacement` | string or `null` | `null` | none | **Stored only.** No consumer; ED2K links are always built with the file name as is. |
| `first_launch` | boolean | `true` | "Hide" on the Overview's "Getting started" card | **Active.** While true the Overview page shows the Getting started card; hiding it sets this false and saves. After an AOM settings import it is set to whether the imported username is empty. |
| `secrets_in_keystore` | boolean | `false` | none (the keystore notice in Options depends on whether the keystore works, not on this key) | **Active, written by Animatic.** `true` after a save that moved `anidb.password`, `anidb.api_key` and `avdump.key` into the operating-system keystore and stored them empty here; loading then fills empty credential fields from the keystore. Do not edit by hand; a file with real values in those keys and `true` uses the file's values. See [Credentials and the system keystore](Settings-Files.md#credentials-and-the-system-keystore). |

## Updates

Object `updates`. It controls the *automatic* check for a newer Animatic release on GitHub and which releases the checks offer.
Checking, downloading and installing are always started by you; see
[Updating Animatic](Updating-Animatic.md) for the flow and
[Storage and network](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#github-update-transport) for the traffic.

| Key | Type | Default | Options control | Status and effect |
|---|---|---|---|---|
| `auto_check` | boolean | `true` | "Check for updates at startup" | **Active.** When true, one background check for a newer release on the selected channel runs at startup, but only if `anidb.offline` is false and the settings were not recovered from a backup. It never downloads or installs anything. When false, no request is made until you choose **Help → Check for updates…**, which works regardless of this key and of `anidb.offline`. |
| `channel` | string | `"stable"` | "Release channel" (**Stable** / **Beta**) | **Active.** `"stable"` offers only published, non-prerelease `YYYY.M.PATCH` releases. `"beta"` also offers newer GitHub prereleases versioned `YYYY.M.PATCH-rc.N`; other prerelease forms and drafts are never offered. Only a strictly newer version is ever offered, so switching back to `"stable"` never installs an older version. Changing it discards an "available" or "up to date" result found on the other channel; the next check (startup or Help → Check for updates…) uses the new channel. |

There is no automatic install, no last-check timestamp and no skipped-version list. A file
without an `updates` object (every earlier version's file) loads with `auto_check` true, so
upgrading to this version turns the startup check on unless you set it to `false`; a file
without `channel` loads as `"stable"`. An older Animatic build ignores and drops the unknown
`updates` key on its next save.
Applying an AOM settings import keeps your current `updates` object (including `channel`) because AOM has no such preference.

## Hash export template tokens

Used by **File → Export → Hashes…** (it exports every hashed file in the library, in list order, to
the file you choose, default name `animatic-hashes.txt`; written in the background, replacing the file atomically). `hashing.export_template` is expanded once
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

1. The template is split into arguments with POSIX-style quoting (the rules of a Unix shell, without running one). An
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

Every Options control does something. This table lists, per Options category, the keys its
controls edit; the [Options window](Options-Window.md) page describes each control.

| Options category | Keys |
|---|---|
| [AniDB account](Options-Window.md#anidb-account) | `anidb.username`, `anidb.password`, `anidb.api_key`, `anidb.offline`, `anidb.auto_run_queue`, `anidb.idle_logout_minutes`, `anidb.update_file_information`, `anidb.download_covers` |
| [Appearance](Options-Window.md#appearance) | `ui.theme`, `ui.color_scheme`, `ui.zoom`, `ui.colored_rows`, `ui.row_style`, `ui.colors` (Customize status colors), `ui.show_tray_icon`, `ui.minimize_to_tray` |
| [Managed folders](Options-Window.md#managed-folders) | `file_managers.managers[]` (every per-folder key except `exclude_hidden`, `can_edit_path` and `default`), `file_managers.scan_at_start`, `scheduled_scan`, `scan_interval_minutes`, `action_delay_ms`, `sleep_time_ms` |
| [Hashing](Options-Window.md#hashing) | `hashing.autostart`, `identify_after_hashing`, `integrate_into_shell`, `use_extensions`, `extensions` |
| [Renaming](Options-Window.md#renaming) | `renaming_pattern` |
| [Players and tools](Options-Window.md#players-and-tools) | `players[]` (every key), `avdump.enabled`, `executable`, `arguments`, `key`, `media.ffprobe_path`, `media.read_properties` |
| [Exports](Options-Window.md#exports) | `hashing.export_template`, `ui.copy_length_as_minutes`, `stat_whore.enabled`, `format`, `file` |
| [Updates](Options-Window.md#updates) | `updates.auto_check`, `updates.channel` |

Keys that work but have no Options control:

- `anidb.udp_server`, `anidb.local_port`, `anidb.image_server` (edit the file);
- per-folder `exclude_hidden`, `can_edit_path` and `default`;
- `ui.window`, `ui.maximized`, `ui.columns` and `ui.last_directory`, which Animatic changes as you
  use the window, its tables and the file pickers;
- `first_launch` (the **Hide** button of the Overview's Getting started card) and
  `secrets_in_keystore` (written by Animatic when it moves credentials to the system keystore).

Keys that exist only for AOM compatibility and are never read by the desktop application, so they
have no effect today:

- `hashing.priority` (**Stored only**; hashing always runs at the normal thread priority);
- `ed2k_space_replacement` (**Stored only**);
- `ui.tree_auto_expand`, `ui.tree_auto_collapse`, `ui.show_local_folder_size`,
  `ui.hide_root_folder` (**Import only**);
- the `ui.colors` slots `LocalRoot`, `LocalFolder`, `LocalFileDuplicate`, `Episode`,
  `EpisodeInMylist`, `FileCrcOk` and `FileCrcInvalid`, which no table uses yet.

## See also

- [Options window](Options-Window.md): every control, with pictures, and when a change applies.
- [Settings files](Settings-Files.md): loading, saving, backup recovery, the keystore and the AOM import.
- [Data locations](Data-Locations.md): where `settings.json` and its backup live.
- [Troubleshooting](Troubleshooting.md): a settings file that will not load.
