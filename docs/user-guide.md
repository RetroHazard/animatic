# User guide

This guide describes what the Animatic desktop application does today, page by page and task
by task. It is written from the source of `crates/animatic` and `crates/animatic-core`; source
anchors are listed in [Source anchors](#source-anchors). A setting that is stored but has no
effect, or a library capability that has no screen, is labeled as such. For installation and
the command line see [getting started](getting-started.md); for every stored option see the
[settings reference](settings-reference.md).

**Contents**

1. [Window layout](#window-layout)
2. [Status bar, progress and the Log](#status-bar-progress-and-the-log)
3. [Searching and tables](#searching-and-tables)
4. [Pages](#pages): [Overview](#overview) · [My list](#my-list) · [Anime](#anime) ·
   [Advanced search](#advanced-search) · [Hashed files](#hashed-files) · [Groups](#groups) ·
   [Genres](#genres) · [Queue](#queue) · [Log](#log) · [Anime detail window](#anime-detail-window)
5. [Menus](#menus)
6. [Hashing and identification](#hashing-and-identification)
7. [Managed folders and scans](#managed-folders-and-scans)
8. [Synchronization](#synchronization) · [MyList editing](#mylist-editing) ·
   [Failure handling](#failure-handling)
9. [Manual renaming](#manual-renaming)
10. [Statistics export](#statistics-export) · [Hash export](#hash-export)
11. [External players and AVDump](#external-players-and-avdump)
12. [Options window](#options-window)
13. [Updating Animatic](#updating-animatic)
14. [Migrate an AOM databank](#migrate-an-aom-databank) ·
    [Import AOM settings](#import-aom-settings)
15. [Not implemented](#not-implemented)
16. [Source anchors](#source-anchors)

## Window layout

- **Menu bar** (top): [File, Tools, Help](#menus).
- **Navigation** (left): nine pages in this order: Overview, My list, Anime, Advanced search,
  Hashed files, Groups, Genres, Queue, Log. At the bottom it shows `N anime` and
  `N mylist files` (counts of the loaded snapshot).
- **Page** (center): the selected page. Table pages are virtualized: only the visible rows
  are drawn, so very large libraries stay responsive.
- **Status bar** (bottom): current activity or last message, connection state and warnings.
- **Floating windows**: [Anime detail](#anime-detail-window), [Options](#options-window),
  "Confirm rename", "Review imported AOM settings", "Settings recovered", the
  [Software updates](#updating-animatic) window and "About Animatic" (the build's version from
  `Cargo.toml`, an AniDB link and the license buttons).

All data on screen comes from a **snapshot** of the local database that a background thread
loads and swaps in whole. Changes appear after the next snapshot (requests are coalesced, at
most one every 250 ms). **File → Refresh** and the Overview "Refresh data" button request one
immediately. If loading fails, the Log shows `Unable to refresh library: …` and the old
snapshot stays.

Tables share a few behaviors: columns can be resized, rows are striped, text is truncated
instead of wrapped, and a horizontal scrollbar appears when the window is narrower than the
columns. Column headers are labels only; there is no click-to-sort, column chooser or saved
layout. The window has no keyboard shortcuts and does not accept drag-and-drop of files;
add files with **File → Add files…**, the Hashed files page, or the command line.

## Status bar, progress and the Log

Left side: while a background job runs, a progress bar with percentage, the label
`<what> (<current>/<total>)`, a **Pause**/**Resume** button and a **Cancel** button;
otherwise the last status message (for example `Ready`, `Hashed …`, `Command blocked: …`).

Right side, from the edge inwards:

| Text | Meaning |
|---|---|
| `Offline` / `Ready for AniDB` | The *Work offline* option. "Ready for AniDB" only means offline mode is off, not that credentials or the network work. |
| `AniDB sync paused until HH:MM:SS: <reason>` (yellow) | An active [cooldown](#failure-handling); the time is local. |
| `N blocked` / `blocked commands (refreshing)` (red) | Blocked queue entries, or a blocked event newer than the snapshot on screen. |
| `N commands NOT saved yet` (yellow) | Queue commands held in memory because the database was busy. See below. |

Jobs and what the controls do:

| Job (status label) | Started by | Pause | Cancel |
|---|---|---|---|
| Hashing files | Add files…, command-line paths | Pauses between 4 MiB blocks | Stops after the current block; the file in progress is discarded (its error `…: cancelled` is logged) and the remaining files are skipped |
| Scanning managed folders | Tools → Scan managed folders | Pauses while a file is being hashed | Stops before the next planned action |
| Importing legacy databank | File → Import AOM databank… | No effect | Checked only between steps (see [migration](#migrate-an-aom-databank)); a late Cancel can still end in success |
| Reading AOM settings | File → Import AOM settings… | No effect | No effect; the review window decides whether anything changes |
| Synchronizing with AniDB | Run command queue / Synchronize now | No effect | Stops before the next command; a request already in flight finishes or times out (UDP 20 s, HTTP 60 s) |
| Queueing full update | `--full-update` | No effect | – |

Jobs run one at a time in submission order. Library snapshots and statistics exports run on
their own thread and do not show a progress bar or take part in Cancel.

**NOT saved yet.** Identify, title-download, statistics-fetch and anime-refresh commands are
written to the durable queue. If the worker holds the database lock, the command is kept in
memory, flagged in the status bar and retried every 500 ms with no retry-count ceiling. Up to 1000
distinct unsaved commands can be held at once; a new command is refused with `NOT queued: …` only
when 1000 others are already unsaved. They are not durable until they appear on the Queue
page. On exit Animatic tries once more for up to 5 seconds; if that fails the loss is only
written to a log that is not installed, so wait for the indicator to clear before quitting.
MyList edits are different: if they cannot be saved immediately you see
`Could not edit mylist entry N: the database is busy…; nothing was changed, try again`.

**Log page / activity log.** See [Log](#log). Messages carry `HH:MM:SS`; errors add `ERROR`.

## Searching and tables

One search string is shared by every page that has a search box. The box is at the top right of
**Anime**, **My list**, **Hashed files**, **Groups** and **Genres**; **Advanced search** has
its own query field bound to the same string. Typing on one page filters the others when you
switch to them.

Semantics (from `app.rs` and `library.rs`):

- **Substring, not words.** A row matches when the query appears anywhere in the searched text.
  There are no wildcards, regular expressions, quotes, operators or multi-term AND.
- **ASCII case folding only.** Both sides are lower-cased with ASCII rules; `a`/`A` match, but
  `é`/`É` do not match each other.
- **Not trimmed.** A leading or trailing space is part of the query.
- **What is searched:**

| Page | Searched text |
|---|---|
| Anime, Advanced search | The anime's **display name** (main title preferred, then `x-jat`, then `en`) – *not* synonyms, other-language titles or episodes. Also an exact AniDB id. |
| My list | The anime's display name only (not episode, group or file). |
| Hashed files | The **full path**, not just the file name. |
| Groups | Group name or short name. |
| Genres | Genre name. |

- **AniDB id matching** (Anime and Advanced search only): if the query is canonical decimal
  digits (no leading zeros or signs) the row whose id equals it also matches. `3` finds anime 3;
  `03` is treated as plain text and does not.
- Rows without a name display as `aid N`; My list rows whose anime is missing display `Unknown anime`.
- Empty query shows everything.

## Pages

### Overview

Four tiles and a health summary:

| Tile | Definition |
|---|---|
| Anime | Number of anime rows in the database. This includes rows created by the [title-list download](#synchronization), so it can far exceed your own library. |
| Mylist files | Number of MyList entries |
| Watched | MyList entries marked watched |
| Local size | Total size of hashed files (binary units, e.g. `1.23 GB`) |

**Local library health:** `X of Y hashed files identified by AniDB`; a red line `N files have a
CRC mismatch` when AniDB knows a CRC for a file that differs from the one Animatic computed
(compared case-insensitively; no AniDB CRC means no mismatch); `N files are not identified yet`.

Buttons: **Add files**, **Open hashed files**, **Refresh data**.

The **first-run card** ("Bring your AniDB O'Matic library forward") is shown while
`first_launch` is true. It has a folder box, **Browse…**, **Import** (same as
[File → Import AOM databank…](#migrate-an-aom-databank), but you may paste a folder path) and
**Dismiss first-run card**, which clears the flag and writes the settings file at once (unless
[recovered settings](#options-window) are guarding it).

### My list

Columns: **Anime** (link; opens the [detail window](#anime-detail-window)), **Episode**
(`12`, `S1`, `C1`, `T1`, `P1`, `O1`; blank if unknown), **Group** (short name, else full
name), **State** (combo box), **Watched** (checkbox), **Size**.

The page lists the MyList entries stored locally. They come from an AOM import or from files
identified through the [queue](#queue); there is no "download my whole MyList" action (see
[Not implemented](#not-implemented)). State and Watched are the only editable fields; see
[MyList editing](#mylist-editing).

State choices are `Unknown`, `On HDD`, `On CD/DVD`, `Deleted`, `Remote/Shared` (stored
as 0–4). The closed combo box abbreviates some labels (`On CD`, `Shared`) and can show
`Release` for AniDB state 5, which you cannot select.

### Anime

Columns: **Title** (click to select and open the detail window), **Type** (`TV Series`, `OVA`,
`Movie`, `Other`, `Web`, `TV Special`, `Music Video`, else `Unknown`), **Year**, **Episodes**
(total count; 0 if unknown), **Rating** (two decimals, blank when none), **Mylist**
(`N eps / M files` when any of its files are in MyList).

Search is described [above](#searching-and-tables).

### Advanced search

Fields, combined with AND:

| Control | Rule |
|---|---|
| Title or AniDB ID | As the shared search |
| Year | Text typed here (trimmed) must be **contained in** the year text: `2012` matches `2012`; `20` matches 2012 and 2020 |
| Type combo | Any type, TV Series, OVA, Movie, Other, Web, TV Special, Music Video |
| In mylist | Anime has at least one MyList entry |
| Fully watched | Watched regular episodes ≥ the anime's episode count (total, else highest known number); requires a known count |

Result columns match the Anime page, but the Mylist column reads `N eps / M watched`
(distinct episodes in MyList / watched episodes). Clicking a title opens the detail window.

### Hashed files

Local files Animatic has hashed, sorted by path. Columns: **File** (select by clicking),
**Size**, **AniDB** (`fid N` or `Unknown`), **Mylist** (`lid N` or `—`), **CRC** (local CRC32
in upper-case hex, or `Mismatch`), **Path** (double-click opens the file with the operating
system's default program). The row count reads `N files` or `M of N files` under a filter.
If a search hides the selected row the selection is cleared so no action can hit an
invisible file.

Buttons (Rename, Open, Play and AVDump need a selected file):

| Button | Does |
|---|---|
| Add files… | File picker (multiple); starts [hashing](#hashing-and-identification) |
| Rename… | Previews a [manual rename](#manual-renaming) |
| Open | Opens the file with the OS default program (not your configured player) |
| Play | Starts the default [external player](#external-players-and-avdump) with its *Play* arguments |
| AVDump | Enabled only when AVDump integration is on; starts [AVDump](#external-players-and-avdump) |

"AniDB: fid N" means the hash matches a file record already stored locally (from the import or
from an identify command that ran); `Unknown` means no such record, not necessarily that
AniDB does not know the file.

### Groups

Searchable table: **Name**, **Short name**, **Rating**, **Votes**, **Website** (`Open` link
launches the browser; `—` if none).

### Genres

Searchable table: **Genre** and **Anime** (number of anime carrying it).

### Queue

The durable AniDB command queue. Header: `N pending commands`, a red `N blocked` (or
`blocked commands (refreshing)`), and three buttons:

| Button | Does |
|---|---|
| **Clear queue** | Deletes *every* queue row, blocked ones and unsynchronized MyList edits included, with no confirmation. See [MyList editing](#mylist-editing) for the effect on the local copy. |
| **Retry failed** | Enabled when something is blocked. Unblocks every blocked row with fresh attempts (edits of the same MyList entry are merged first). Does not run anything and does not bypass a cooldown. |
| **Synchronize now** | Starts the same job as **Tools → Run command queue**. Disabled during a settings review or an active cooldown (the tooltip shows the cooldown). |

A yellow line repeats an active cooldown. Table columns: **ID** (`#n`, never reused),
**Command**, **Status**, **Last error** (hover for the full text).

| Status | Meaning |
|---|---|
| `Pending` | Not yet attempted |
| `N retries` (yellow) | Attempted before; still runnable |
| `Blocked` (red) | Skipped until **Retry failed** |

Order of execution (and of the list, blocked rows last): lower priority number first, then
oldest. Priorities: MyList writes 0; Identify 1; file/episode/group lookups 2; anime refresh,
MyList statistics, title list 3; MyList entry refresh 4.

Command descriptions you will see: `Identify file <ed2k> (<n> bytes)` (with ` + add to
mylist` when the folder rule adds it), `Edit mylist entry N`, `Fetch anime N`, `Fetch file
N` (from the full update), `Fetch mylist statistics`, `Download anime title list`. The model
also has add, remove and refresh MyList commands and episode/group lookups; **no screen creates
them**. A row whose stored payload this version cannot read is kept, blocked, as `Unreadable
queued command: …`; it is never deleted silently.

### Log

In-memory activity log, newest at the bottom (it scrolls with new lines). **Clear** empties
it. It is not written to disk, resets on exit, and is trimmed to recent entries once it grows past about 2000.
It records starts, hash results (`Hashed <path> (local #N, <ed2k>)`), sync outcomes, blocked
commands, cooldowns, imports, exports and every error. Hash output reveals file paths and
hashes, so mind what you paste when asking for support.

### Anime detail window

Opened from any anime link or title (or a Relations entry). Sections: heading (name, or `Anime N`),
`type · year · N episodes`, description if one is stored, and collapsible **Titles** (language
code and text), **Episodes** (episode number and name), **Genres**, **Relations** (relation
kind and title; clicking one switches the window to that anime, and shows `No detail record is
available.` if it is not stored), **Reviews** (author or `Anonymous`, rating, body).

- **Open on AniDB** opens `https://anidb.net/anime/<id>` in your browser.
- **Refresh from AniDB** queues `Fetch anime N` (HTTP API). Nothing is sent until you run the
  queue; a duplicate of an already queued refresh is ignored.

The window shows no cover image: picture names are stored, but no image is downloaded or shown.
Close it with its title-bar button.

## Menus

| Menu | Item | Behavior |
|---|---|---|
| File | Add files… | Picker; hashes the chosen files. Disabled during a settings review. Remembers the last folder. When *Restrict file extensions* is on the picker filter is "Configured media files". |
| File | Import AOM databank… | Folder picker → [databank import](#migrate-an-aom-databank). Disabled during a settings review. |
| File | Import AOM settings… | File picker (`.xml`) → [settings import](#import-aom-settings). |
| File | Export statistics… | Save dialog (default `animatic-stats.xml`/`.ini`) → [statistics export](#statistics-export). |
| File | Export hashes… | Save dialog (default `animatic-hashes.txt`) → [hash export](#hash-export). |
| File | Refresh | Requests a new snapshot. |
| File | Quit | Closes; saves settings (unless guarded) and tries to save unsaved commands. |
| Tools | Options… | Opens [Options](#options-window). |
| Tools | Scan managed folders | Starts a [scan](#managed-folders-and-scans). Disabled during a settings review. |
| Tools | Download anime titles | Queues the title-list download (nothing is sent yet). |
| Tools | Fetch mylist statistics | Queues a MyList-statistics request. When run, only a log line (`N anime, N episodes, N files in mylist`) is produced; the result is not stored. |
| Tools | Edit AniDB data ▸ | Opens AniDB web pages in the browser: Add anime, Add episode, Add file, Submit change request (`anidb.net/anime/add`, `/episode/add`, `/file/add`, `/creq`). |
| Tools | Run command queue | Starts the [synchronization](#synchronization) job. Disabled during a settings review or an active cooldown. |
| Help | AniDB website | Opens `https://anidb.net/`. |
| Help | Check for updates… | Opens the [Software updates window](#updating-animatic) and starts a check for a newer stable release. This is an explicit request and works even when *Work offline* is on (that setting only governs AniDB and the automatic startup check). Disabled on platforms other than Windows or Linux x86_64. |
| Help | About Animatic | Version and link, plus **View license** and **Third-party notices** buttons that open the license documents next to the program: the versioned `LICENSE-vVERSION` / `THIRD-PARTY-NOTICES-vVERSION.html` for the running version if present (left by an update), otherwise the bundled `LICENSE` / `THIRD-PARTY-NOTICES.html`. A button is disabled when neither file exists (for example a bare executable copied out of an archive). |

There is no menu item for a full update (command line only), for adding or removing MyList
entries, or for downloading images.

## Hashing and identification

**Adding files.** *File → Add files…*, the Hashed files page, the Overview button, or media paths on the
[command line](getting-started.md#command-line-reference). The picker's extension filter is
only a convenience; command-line paths are never filtered. Each file is read **once**; ED2K
(the AniDB-compatible "red" variant), CRC32, MD5 and SHA-1 are computed together on separate
threads. Progress shows the file name, `(n/total)` and the current file's percentage.

Result handling per file:

1. The local hash and file path are stored (the same path is updated in place; identical content at
   a different path shares one hash record). Paths are stored exactly as given, so use absolute
   paths.
2. The Log gets `Hashed <path> (local #N, <ed2k>)`.
3. If *Identify files with AniDB after hashing* is on (default), an **Identify** command is
   queued. If the file's [folder rule](#managed-folders-and-scans) has **Add to mylist**, the
   command also carries that rule's MyList settings.
4. A read error (missing file, permission, a directory) is logged as `<path>: <error>`; the
   remaining files still run. Cancel stops the whole batch.

**Identification** happens only when the queue runs: AniDB is asked for the file by size and ED2K
hash. If AniDB knows it, the file record is stored (so Hashed files shows `fid N`); if the file
is not already in your MyList and the command carries an add policy, it is added with the rule's
state, watched flag, storage, source and other fields; if it is already in MyList the policy is
**not** applied and the MyList entry AniDB reports is stored locally. If AniDB explicitly reports
that the file is **not** in your MyList, local entries for that file are removed unless they have
queued edits. An unknown file logs `file unknown to AniDB` and nothing else changes.

With default settings, the built-in **Default** rule has *Add to mylist* on, *Set state* on with
*On HDD*, and *Set watched* on with *Watched* off, so identified files that match no custom rule are
added to MyList as "On HDD", unwatched, when you run the queue. Review this before online work.

"Start hashing automatically" is stored only: hashing begins as soon as files are chosen or
passed on the command line.

## Managed folders and scans

A **managed folder rule** (Options → Managed folders) combines a folder, scan behavior and
MyList policy. **Scans are manual-only**: run **Tools → Scan managed folders**. There is no
scheduler, tray icon or background watcher; `action_delay_ms` and `sleep_time_ms` are stored
but never read. The scan itself never deletes media files, never renames files and never writes
MyList; it can hash files and forget missing local records. Hashing is not the end of it, though:
every file the scan hashes raises the same event as any other hashing, so with *Identify files with
AniDB after hashing* on and the folder rule's **Add to mylist** on, an Identify command with an add
policy is queued, and **running the queue later adds the file to your AniDB MyList**.

Planning (`file_manager::plan_scan`), per rule in list order:

- A rule is walked only if its **Path** is a directory that exists *and* **Auto hash** or
  **Rehash changed** is on.
- **Subfolders** on = unlimited depth; off = direct children only. Symbolic links are not followed.
- With **exclude_hidden** (the default for new rules; not shown in Options), entries whose name
  starts with `.` are skipped, directories included. The Windows hidden attribute is not
  consulted, and *exclude system files* is stored but unused.
- Only files whose extension is in the **extension list** are considered. This list is always
  applied during a scan, even when *Restrict file extensions* is unchecked (that checkbox only
  filters the file picker). An empty list means a scan finds nothing. Matching is
  case-insensitive; the list is comma-separated in Options.
- A file whose path is not in the database is hashed (**Auto hash**). A known path whose
  modified time differs from the stored one is re-hashed (**Rehash changed**).
- Discovery is **first rule wins** in the order of the rules list: each discovered path is
  recorded once, so a file inside two overlapping rules is planned under the earlier one. (Longest-path
  matching applies only to the post-hash MyList policy, the rename destination and *Remove
  missing*, described below.) Windows compares paths case-insensitively and with `\` and `/`
  unified; Linux compares case-sensitively.

**Remove missing** is evaluated independently of walking. A stored path is removed from the
local inventory only when **all** hold: the path is under an explicit rule that has *Remove
missing* on (subfolders count only for rules with *Subfolders*; the unmatched Default rule never
authorizes removal), the filesystem reports the file **not found** (any other error, such as an
unavailable drive or permission failure, keeps the record), and the rule's own folder exists as a
directory. It then removes only the local record (and a hash record nothing else uses). Media
files and MyList entries are untouched.

Hashes found by a scan follow the normal [identification](#hashing-and-identification) path.
The rule fields `auto_rename`, `clear_empty_folders` and `exclude_system` have no effect; the
**Auto rename** checkbox exists but renaming is only manual
([Manual renaming](#manual-renaming)).

**Which rule applies to a file** (`FileManagerSettings::manager_for`): rules with a non-empty
path are candidates; one applies if the file is below its folder (any depth when *Subfolders*
is on, direct children only otherwise); the longest matching path wins; no match falls back to the
rule marked default (else the first). The fallback supplies the MyList policy for identify-after-hash
and the rename destination, but never authorizes removal.

**MyList policy fields** (apply to newly added files): each of *Set watched*, *Set state*, *Set
storage*, *Set source*, *Set other* is a checkbox. Unchecked = leave that field unchanged;
checked with an empty text = explicitly clear it. **Rename destination** is the base folder
for [manual renames](#manual-renaming); empty keeps files where they are.

## Synchronization

Animatic works **offline-first**. Hashing, browsing, searching and local MyList edits never
touch the network. Network traffic happens only in the *Synchronizing with AniDB* job, started
by **Tools → Run command queue** or **Queue → Synchronize now** (nothing runs on a timer).

Before the first run: set **Client name** (registered with AniDB), **Username** and
**Password**, optionally an **API key** (encrypted UDP sessions), clear **Work offline**, press
**Save options**. See [client registration](getting-started.md#anidb-client-registration). Imported AOM
settings never contain a client name.

The job (`process_queue`):

1. If *Work offline* is on, it logs `Offline mode is enabled; commands remain queued` and stops.
   The queue is unchanged.
2. If a persisted cooldown is active it logs how long remains and stops (this also refuses to
   start the job, via the disabled buttons).
3. Otherwise it builds UDP and HTTP clients from the current in-memory settings and repeatedly runs
   the next unblocked command until the queue is empty, you cancel, a command stops the run or a
   cooldown begins. Every successful command is removed from the queue and logged
   (`known file F (anime A), mylist entry L`, `mylist entry updated`, `anime A updated`,
   `N titles downloaded (M new)`, …). It logs out of UDP at the end.
4. Requests are paced: UDP packets at least 2 s apart (4 s after a burst of five; idle for 10
   minutes resets that), HTTP requests 2 s apart. UDP login is lazy (first UDP command).

What the user-reachable commands do:

| Command | Effect on the local database |
|---|---|
| Identify | Stores AniDB's file record; optionally adds the file to MyList (see above) |
| Edit mylist entry | Sends state/watched changes; on success updates the local entry (keeping any newer pending edit on top) |
| Fetch anime N | Downloads the HTTP-API anime record: titles, episodes, genres, relations, reviews, metadata |
| Fetch file N | Updates a file record (from a full update) |
| Download anime title list | Streams `anime-titles.xml.gz` into a temporary store, validates it, then merges atomically in one transaction. Creates an anime row for every unknown id and replaces titles only of anime without a full HTTP record (imported AOM anime count as such). Failure leaves titles and the completion time untouched. Limits: 32 MiB compressed, 128 MiB decoded, 250 000 anime, 2 000 000 titles, 2048 bytes per name. |
| Fetch mylist statistics | Log line only |

A full update (`--full-update`) queues the title list plus one file lookup and one anime refresh for
each file id and anime id that your **identified hashed local files** resolve to (not the whole local
cache or the title catalog); it does not run them.

## MyList editing

On **My list**, changing **State** or toggling **Watched** is a *local-first* edit:

1. In one database transaction the edit is saved to the outbox **and** the stored MyList entry is
   updated, so the row shows the new value immediately (even before the next snapshot) and
   the change survives restarts.
2. The edit is sent to AniDB only when the queue runs (priority 0, ahead of lookups).
3. Further edits of the same entry before that are **merged** into one fresh queue row with a new,
   never-reused id: unset fields keep the older value, and the last watched/unwatched choice wins
   (watched → unwatched → watched ends watched at the latest time). Marking watched stamps the
   time of your click, not the time AniDB hears of it; unmarking clears the date.
4. When AniDB acknowledges, the local entry is rewritten with the acknowledged values **plus** any
   newer pending edit. An acknowledgement or reply for an older command cannot erase newer
   intent. Queued edits that can be read are re-applied over AniDB data fetched meanwhile, so it
   cannot overwrite an entry that still has them. A queue row this version cannot read is kept
   (blocked) but cannot be overlaid, so remote values may replace the local ones for its entry.
5. A malformed "success" reply is a protocol error, never evidence that something was deleted.

Failures and edge cases:

- *No such entry on AniDB:* the edit is kept and **blocked** with an explanation (the local entry
  stays). Refresh the data or discard the row, then **Retry failed**.
- Permanent errors block the one edit; transient errors pause the queue ([below](#failure-handling));
  authentication problems block everything until fixed.
- **Clear queue** deletes the pending edit rows only. Nothing restores the previous local values, so
  the screen keeps showing the edit even though AniDB will never receive it. A later AniDB
  refresh of that entry may overwrite it [INFERENCE: no pending intent remains to protect it].
- If the database is locked by a background write the edit is refused with a visible message;
  repeat it.
- An entry with a queued removal ignores edits (`Mylist entry N is pending removal; edit
  ignored`); no screen queues removals.
- Storage, source, other and view date are not editable on screen; they are set only by a folder
  rule's add policy.

## Failure handling

How the queue reacts. The retry, cooldown and blocking *policy* lives in the desktop worker
(`fail_command`, `process_queue`); the core library supplies the durable queue operations and a single
`execute` per command, and `AnidbError` classifies the failures:

| Situation | Reaction |
|---|---|
| Ban (UDP 510/555/504; HTTP ban text) | Entry kept, **no attempt counted**, queue stops, cooldown **30 minutes** (persisted) |
| Rejected credentials, missing client name/user/password, outdated client (UDP 503) or HTTP client rejection | **Every** unblocked command is blocked with the reason; the queue stops; no cooldown. Fix the setting, **Save options**, **Retry failed**, run again |
| Network/DNS/timeout, AniDB "unavailable" | Queue stops; cooldown 60 s (or the server's stated wait); not counted as an attempt |
| UDP server errors 600/601/602/604 | 604 → 60 s cooldown; the others 30 minutes. Only error 600 and HTTP responses with an error status count toward the attempt limit |
| Storage I/O or database errors while applying a reply | Treated as transient: 60 s cooldown, counted |
| Attempt limit | After 5 counted failures a command is blocked (`blocked after 5 failed attempts`) |
| Permanent errors (illegal input, protocol/XML problem, unexpected reply, unknown command) | Only that command is blocked; the queue continues |

Cooldowns are stored in the database (`sync_cooldown`) and survive restarts. **Retry failed** does
not clear them; Run/Synchronize are disabled until the shown time passes. Blocked rows stay
visible with their last error until you retry, clear or the underlying problem is fixed.

## Manual renaming

Renaming is **manual and per file**. There is no automatic renaming of any kind.

1. Open **Hashed files** and select a file that AniDB has identified (`AniDB` column shows `fid`).
   Unidentified files report `The selected file is not identified by AniDB`.
2. Optionally edit or validate the pattern in **Tools → Options… → Renaming pattern**
   ([below](#options-window)). The language is documented in the [rename language](rename-language.md).
3. Click **Rename…**. Animatic evaluates the pattern against the local database (no network, no
   file access) and opens **Confirm rename** with the current path, the proposed new path, and any
   errors (red) or warnings (yellow).
4. **Rename file** is enabled only when a new path exists and there are no errors and no
   warnings. Otherwise you can click **Edit pattern in Options** (closes the preview and opens
   Options).

A plan is blocked when: the pattern is empty or has a syntax error; the evaluation fails; it ends
with a true/false value instead of text; it yields an empty name or a name with an unusable part
(characters `< > : " | ? *`, control characters, empty folder names, reserved device names
such as `CON`/`NUL`, or a component over 255 characters – UTF-16 units on Windows, bytes
elsewhere); any evaluation warning occurred. The **stable default** pattern additionally requires the
anime title and a positive episode number to be known locally, so fetch the anime ("Refresh from
AniDB" and run the queue) first. If the file already has the produced name, the dialog says
nothing needs renaming.

The destination is the matching folder rule's **Rename destination** joined with the pattern's result; a
plain name stays in the file's current folder, while path separators in the result create or use
subfolders (missing parents are created).

On **Rename file** Animatic checks that the destination does not exist and then asks the operating
system to rename the file. The check and the rename are two separate steps, not one atomic
operation: if another program creates a file at the destination in between, the rename can **overwrite**
it (on Linux the system call replaces an existing file; Rust's rename also replaces existing files
on Windows). Only after the move succeeds is the stored path updated; a failure
reports `Rename failed: …`, leaves the record unchanged and may leave newly created empty
folders. There is no undo, no AniDB notification, and renaming across drives/filesystems is not
specially handled [behavior of the OS rename; not tested here].

Pattern tools in Options (nothing is saved until **Save options**, but see
[Options window](#options-window)): *Use stable default* (replace the text; the earlier text is
kept so *Restore previous text* can undo it), *Validate on selected
file* (dry run on the file selected on Hashed files; shows the result, diagnostics and `The pattern is
valid for this file.`) and live syntax diagnostics. The built-in default uses a fixed two-digit
episode minimum (`01`, `02`, … `100`); an existing custom pattern is never changed unless you
choose *Use stable default*.

## Statistics export

**File → Export statistics…** asks for a file and writes a library report in the format chosen in
Options → Statistics export (XML or INI). It runs on the background projection thread and logs
`Exported statistics to <path>` or `Statistics export failed: …`.

**Automatic export:** if *Write statistics after library updates* is on and *Output file* is not
empty, the file is rewritten every time a library snapshot is loaded (after hashing, syncing,
edits, refreshes, startup), overwriting it. Failures log `Automatic statistics export failed: …`.
Parent folders are created. Use a path outside the repository.

Content (from `stats.rs`): AniDB totals in your database (anime, episodes, files, groups, total
size, total length), MyList totals (anime, episodes, files, size, anime watched/partially watched,
episodes watched, files watched, watched size, length, watched length), seven anime-type groups
(`tvseries, tvspecials, movies, ovas, web, other, unknown` with total, in mylist, all watched,
part watched, size, length, watched length) and, only when MyList is not empty, four MyList-state
groups (`unknown, onhdd, oncd, deleted`; state *Remote/Shared* is not reported). Lengths are
seconds; sizes are bytes. XML uses `<stats><anidb>/<mylist>` with CRLF line breaks; INI uses
`[anidb]`, `[mylist]`, `[animetypeTVSeries]`…`[animetypeUnknown]` and `[statusUnknown]`…
`[statusDeleted]`. Both contain your **AniDB username** (XML-escaped; INI verbatim); treat the
file as personal data.

## Hash export

**File → Export hashes…** writes one line per hashed file, for *all* hashed files (search
filters and selection are ignored), using the *Hash export template* from Options
(empty template = `%fulled2k%`). It runs on the interface thread and logs `Exported N hashes to
<path>` or `Hash export failed: …`. Lines end with CRLF; a newline inside the template becomes CRLF.

Tokens are replaced sequentially in this order; unknown text is left as is:

| Token | Value |
|---|---|
| `%filename%` | File name |
| `%filepath%` | Full stored path |
| `%ed2k%`, `%md5%`, `%sha1%`, `%crc32%` | Hash (ED2K lower-case; MD5/SHA-1/CRC32 upper-case; CRC32 zero-padded to 8) |
| `%size%` | **Human-readable** size such as `1.00 MB`, not a byte count |
| `%fulled2k%` | `ed2k://\|file\|<name>\|<bytes>\|<ed2k>\|/` |
| `%fps%`, `%height%`, `%width%`, `%vbitrate%`, `%asample%`, `%abitrate%` | Constant placeholders `0.00`/`0` |
| `%vcodec%`, `%acodec%` | Always empty |
| `%length%` | Empty, or `0` when *Export %length% in minutes* is on |

Media tokens are not read from the files, so only the first group is meaningful. See also the
[settings reference](settings-reference.md#hash-export-template-tokens).

## External players and AVDump

**Players** (Options → External player): *Name*, *Executable*, *Play arguments*, *Default player*.
**Play** uses the player marked default, else the first. *Add player* appears only while the list
is empty; imported settings may supply more players. The stored *open* and *enqueue* templates
have no screen and are never used. **Open** on the Hashed files page and a double-click on a path
use the OS default program, not a configured player. An empty executable gives `Could not start
player: player executable is not configured`.

**AVDump** (Options → AVDump): *Enable AVDump integration*, *Executable*, *Arguments*, *AVDump API
key*. The **AVDump** button needs the integration enabled and a selected file. The default
arguments are:

```
--Consumers=ED2K --Auth={user}:{key} --LPort={port} --PrintEd2kLink "{path}"
```

Placeholders: `{path}` (the file), `{user}` (the AniDB username from Options), `{key}` (the
AVDump API key) and `{port}` (the AniDB local UDP port). Players also accept `%file%` for the path.

How arguments are built (`external.rs`, the `shell-words` crate): placeholders are substituted
into the text **first**, then the whole string is split with POSIX-shell rules (spaces separate
words; `'…'`, `"…"` and `\` quote; `#` starts a comment) and the program is started directly,
without a shell. For a player whose arguments do not contain a path placeholder the file is
**appended as one safe argument**. Consequences:

- Prefer leaving the path placeholder out of player arguments (use flags only): the path is then
  appended as its own unparsed argument, the only fully intact route. AVDump has no such route; its
  arguments always go through substitution and splitting, so its `{path}`, `{user}` and `{key}` values
  are raw text.
- Quoting `{path}` is only a mitigation, not escaping: the value is substituted as raw text
  before splitting. On Windows an unquoted path loses its backslashes, because `\` is an escape
  character in this syntax; inside double quotes backslashes survive except before `"`, `\`, `$`
  and `` ` ``. A path with a double quote can break a quoted token and add arguments (no shell is
  involved, so it cannot run commands, but it can change what the program receives).
- Unbalanced quotes are rejected: `invalid external command arguments: …`.
- `{user}`, `{key}` are inserted unquoted; values with spaces, quotes or backslashes will be
  mis-split.
- Animatic does **not** wait for, capture or import the child's output: the ED2K link
  AVDump prints is not shown or stored. The process inherits the application's output
  streams, and its arguments, including your API key, are visible in the process's command
  line to other local tools [INFERENCE from how processes are started].

More examples and the stored-field details: [settings reference](settings-reference.md#external-command-templates).

## Options window

Open with **Tools → Options…**. It is one scrolling window; sections in order: AniDB account,
Appearance, Hashing, Managed folders, Statistics export, External player, AVDump, Renaming pattern,
Updates, and a read-only list of MyList state names. **Save options** at the bottom writes the file, applies
theme and scale and closes the window.

**Edits are live.** Controls change the in-memory settings as you type. Closing the window with
its title-bar button does not revert them: the edited values stay in effect (for example for the next
queue run) and are written when the application exits normally. Only **Save options** writes
immediately. Exceptions: when settings were **recovered from a backup**, exit and the Overview
dismiss button do not write; only **Save options** or an applied import do (a recovery notice says so).
Theme and scale are applied on **Save options** or an applied import; at launch the window
is always dark at normal scale (`Theme: System` never changes the look).

| Section | Controls | Status |
|---|---|---|
| AniDB account | Username, Password (masked), API key (shown in clear), Client name | Used by sync and AVDump |
| | Work offline | Active |
| | Update known file information | **Stored only** |
| Appearance | Theme (Dark/Light/System), UI scale 0.75–2.0 | Dark/Light and scale apply on Save; System does nothing |
| | Use status colors | **Stored only** (no row coloring exists) |
| Hashing | Start hashing automatically | **Stored only** |
| | Identify files with AniDB after hashing | Active |
| | Restrict file extensions + list | The picker filter; the list also always drives [scans](#managed-folders-and-scans) |
| | Hash export template, Export %length% in minutes | Active for [hash export](#hash-export) |
| Managed folders | Per-rule fields, Add folder rule, Remove folder rule (user-created rules only) | See [scans](#managed-folders-and-scans); *Auto rename* **stored only** |
| Statistics export | Enable, Format, Output file | Active |
| External player | Executable, Play arguments, Default (*Name* is a label that is never read) | Active; open/enqueue have no UI |
| AVDump | Enable, Executable, Arguments, API key | Active |
| Renaming pattern | Editor, Use stable default, Restore previous text, Validate on selected file | Active |
| Updates | Check for updates at startup (`updates.auto_check`, default on) | Active; see [Updating Animatic](#updating-animatic) |

Not in Options (JSON only): client version, UDP server, local port, image server, idle logout,
tray, window and column state, row colors, `ed2k_space_replacement`, folder-scan delays.
The full list with active/stored-only status is the [settings reference](settings-reference.md).
Credentials are stored in plain text, including in the backup copy.

If a **"Settings recovered"** window appears at startup, the primary file was missing or invalid and
the backup was loaded **offline**; the original file was not touched. Review the values, then
**Save options** to replace it. See the [recovery matrix](settings-reference.md#recovery-matrix) and
[troubleshooting](troubleshooting.md).

## Updating Animatic

Animatic can replace **its own executable** with a newer stable release published at
<https://github.com/RetroHazard/animatic/releases>. It is supported on Windows x86_64 and
Linux x86_64 only; elsewhere the update controls are disabled. Nothing is ever downloaded or
installed unattended. Linux runtime behavior of the updater has not been exercised by the project.

**What counts as an update.** Only a *published, non-draft, non-prerelease* GitHub release whose
tag is a valid `vYYYY.M.PATCH` CalVer version. Animatic reads the release list (not just the
API's first or "latest" entry) and takes the **highest CalVer version**; it is offered only when
it is strictly newer than the running version, so an equal version or a downgrade is never
offered. A release that lacks the expected archive or `SHA256SUMS` asset for your platform is not
offered.

**The steps** (each needs a click; the **Software updates** window shows the installed version
and the current step):

1. **Check.** Help → **Check for updates…** opens the window and starts a check (**Check now** /
   **Check again** repeat it). The startup check does the same work but does not open the
   window: if it finds a newer release, the Log gets a note and an **Update available: VERSION**
   button appears at the right of the menu bar to open the window. A startup check that finds
   nothing or fails stays quiet (a failure is only written to the Log); a manual check always
   reports its result in the window. The window shows the version, the download size, a link to the
   release page on GitHub and the integrity warning described below.
2. **Download and verify.** The archive and `SHA256SUMS` are downloaded with size limits and
   checked against each other (SHA-256), then unpacked into a private temporary folder. **Cancel**
   stops the download. Nothing in the installation folder changes yet. The verified SHA-256 is
   shown, and **Discard download** throws the staged files away.
3. **Install.** **Install** first shows which file will be replaced; **Confirm installation**
   (or **Back**) is the confirmation. Only then are the staged files installed; see below.
4. **Restart now or Later.** **Restart now** saves your settings, closes the window normally and
   starts the new executable afterwards; **Later** leaves the running version untouched and the
   new one starts the next time you launch Animatic. Until you restart, the running (older)
   program does not check for, download or install further updates.

Only one update operation runs at a time, and it runs on its own background thread, so the
AniDB queue, hashing and library refresh are unaffected. Closing Animatic during a check or
download cancels it; closing during the short install step waits for it to finish.

**Automatic startup check.** *Options → Updates → Check for updates at startup*
(`updates.auto_check`, **on by default**) makes one background check when Animatic starts. It
is skipped when *Work offline* is on, when the settings were recovered from a backup (recovery
always starts offline), or when you turned the option off. Because the factory default for
*Work offline* is **off**, a fresh profile contacts GitHub at its first start. To prevent that,
place a settings file that sets `updates.auto_check` to `false` (or `anidb.offline` to
`true`) at `ANIMATIC_SETTINGS` before the first launch; see the
[settings reference](settings-reference.md#updates). The check only looks for a version and
never downloads an archive. The Help menu check ignores *Work offline* because you asked for
it explicitly.

**What the install changes.** The executable is replaced using the maintained
[`self-replace`](https://crates.io/crates/self-replace) crate, which performs the operating-system
specific swap (on Windows the running executable is renamed aside and removed after it exits).
Before that swap Animatic writes the new release's license files **next to the executable** as
`LICENSE-vVERSION` and `THIRD-PARTY-NOTICES-vVERSION.html` (for example
`LICENSE-v2026.12.0`). An existing different file of that name, or a symlink, is never
overwritten; an identical file is fine. Everything else is left alone:

- The bundled `LICENSE`, `THIRD-PARTY-NOTICES.html`, `README.md`, `CHANGELOG.md` and `docs/`
  stay at the **old** version. They change only if you replace them from a newly downloaded
  archive yourself. The documentation you are reading may therefore describe an older version
  than the executable.
- Your database, settings, backups, profiles, queue, media and any files you have hashed are
  never touched; an update replaces the program only.
- The new version's licenses are the versioned files next to the executable. After the next
  start, **About Animatic → View license / Third-party notices** opens the versioned file for the
  version you run (`LICENSE-vVERSION`, `THIRD-PARTY-NOTICES-vVERSION.html`) and falls back to the
  older bundled copy only when that file does not exist.
- Installing also creates an empty `.animatic-update.lock` file next to the executable. The
  operating system holds an advisory lock on it only while an install runs, so two Animatic
  processes cannot install at once; the file is harmless, may remain afterwards and can be
  deleted by hand while no Animatic is installing.

**What the installer refuses.** **Install**, **Confirm installation** and **Restart now** are
disabled, with the reason shown, while a task is running (hashing, scan, queue run, import), an
AOM databank import is in progress, imported AOM settings are waiting to be applied or cancelled,
or some changes are not yet saved to the database. **Restart now** is also disabled while the
settings were recovered from a backup and not yet saved (use **Save options** first), and a
failed settings save keeps Animatic open instead of restarting. Animatic also needs to know where it
was installed (the executable path captured at startup) and refuses on a mismatch. Its folder must
be writable by your account: there is **no privilege elevation** and no
administrator or `sudo` prompt. If the folder is read-only (for example a system-wide
installation), update that copy the way you installed it. A copy run from a source
checkout (`target/debug` or `target/release`) can be replaced, but that only changes the compiled
binary, never the Rust source; rebuild or `git pull` instead. A copy installed by a package manager
should be updated by that manager, otherwise the next package upgrade will overwrite or
conflict with the replaced file.

**Restart safety.** Restarting happens only after the window has closed and the library
database and worker threads have been released, so two Animatic processes never use the
same files. The new process is started with only the current `--database` path (as an
absolute path). Original media paths and `--full-update` are **not** repeated, so nothing is
re-hashed or re-queued. The environment is inherited; if you started Animatic with `ANIMATIC_SETTINGS`, the new process receives it as the absolute path that was in use.
If the new executable cannot be started, Animatic exits with an error telling you to start it
manually; nothing is retried.

**Integrity and trust.** The check is that the archive's SHA-256 matches the `SHA256SUMS`
file published in the same release. This detects a corrupted or truncated download and a
mismatched file; it is **not** a digital signature and does not prove who published the
release, because both files come from the same GitHub release and are unsigned. Animatic
displays the verified SHA-256 so you can compare it with another source if you want to. Only
install updates from a release page you trust.

**Privacy.** Update traffic goes to GitHub, not AniDB, and is anonymous. GitHub sees your IP address and
the request's `User-Agent`, which names Animatic and its version. No account, token, credentials,
library data or file names are sent, and there is no token or sign-in setting. See
[Storage and network](storage-and-network.md#github-update-transport).

**If something fails.** The window shows the reason. Check, download, verification and most
install refusals (another instance installing, the installed file is not the one this process
started from, a read-only folder, a conflicting different file in place of a versioned license
file) happen **before** the executable is touched: nothing is replaced, the staged download
is dropped and you must press **Download and verify** again. Versioned license files may already
be present next to the executable after such a refusal or a later failure; they are harmless.

If the actual replacement of the executable was entered and failed, Animatic treats the
installation as possibly damaged: it stops offering check, download, install and restart for
the rest of the session, never restarts by itself, and the window says the program files may be in
an unknown state. First check that the executable still starts. On **Windows**, where
`self-replace` is not transactional, Animatic first makes a recovery copy
`.animatic-recovery-<random>.exe` beside the executable and, after a failed swap, restores the old
executable to its original name **only if nothing exists there** (it never overwrites a file
that something else put there). The recovery copy is always kept after a failure; the message
names it and, if the restore did not happen, tells you which file to copy to which name. On
**Linux** the swap is a single atomic rename and the old executable should be unchanged. A
power loss or crash during the swap is not covered: Animatic promises no rollback, and it never
deletes dot-prefixed leftovers (`.animatic-recovery-*`, `.animatic-restore-*`, or helper files
created by `self-replace`) automatically, because one of them may be the only intact copy.
Delete them by hand only after Animatic starts normally from its usual path. If it does not
start, see [troubleshooting](troubleshooting.md#update-problems) and install the archive from the
release page manually as described in [Releases](releases.md#install-a-release).

### When the release feed is unavailable

Animatic reads GitHub's release feed **anonymously**; there is no token, sign-in or other credential
setting. Only releases that GitHub serves to anyone can be found. If GitHub answers HTTP 404 (which
is also what an anonymous client sees for a repository or releases that are not publicly readable,
and for a repository with nothing published), a manual check reports that the release feed is
unavailable or was not found. It never says "up to date" in that case. The update feature works once
the releases are publicly readable; until then, update by downloading the archive manually
([Releases](releases.md#install-a-release)). This guide does not state whether the repository is
currently public. A rate limit (HTTP 403 or 429) is reported separately and is not retried.

## Migrate an AOM databank

AOM keeps its library in encrypted `kowai/*.data` SQLite files. Animatic reads them without
modifying or deleting them.

Before you start: close AOM, **back up the `kowai` folder**, and decide the target. The import
writes into the database Animatic currently has open (default location or `--database`). For a
clean result start with an empty database (a fresh `--database` file) and keep a copy of any
existing database made while Animatic is closed (the database file plus its `-wal`/`-shm` files).

1. **File → Import AOM databank…** (or the Overview first-run card). Choose the `kowai` folder
   itself. It must directly contain `mylist.data`, `local_files.data` or `animes.data`, else
   Animatic says `No AOM databank found in <folder>` (picking AOM's install folder is not enough
   in the app).
2. Watch the progress bar (`Decrypting <name>.data`, `Importing <name>`, `Imported <name>`,
   `Computing derived data`, `Import complete`). Use **Cancel** to stop.
3. The Log ends with `Imported N records from M collections`.
4. Continue with [settings import](#import-aom-settings), then Options (client name!) and the
   [first sync](#synchronization).

What happens (`legacy::import::import_databank`):

- Up to 23 collections are processed **in this order**, skipping any `.data` file that is absent:
  `catcat, catlang, catqual, cattype, catvcodec, catacodec, catres, catsub, genres, groups,
  animes, animetitles, animegenres, animerelations, episodes, episodetitles, files,
  fileepisoderelations, filerelations, reviews, mylist, local_hashes, local_files`.
  If none exist the import fails with `no legacy databank files found`.
- For each collection one file is decrypted into a plain SQLite file in a temporary folder,
  attached, imported inside **one transaction per collection**, committed, detached and the
  temporary file deleted. Progress is weighted by file size.
- **Atomicity is per collection, not for the whole import.** A collection that fails is rolled back
  and skipped while later collections still run. The Log summary reports only how many
  collections were imported, not which failed, so compare `M` with the number of recognized
  `.data` files you have. Unreadable individual records inside a collection are dropped silently.
  A failure while *decrypting* (for example a truncated file) aborts the entire import with
  `Legacy import failed: …`; collections already committed stay in the database.
- **Cancel** is only checked while a collection is being decrypted and at the "Importing <name>"
  step before it. It is *not* checked inside a collection's import transaction, during the final
  derived-data processing or the closing bookkeeping. A decryption in progress finishes first. A
  Cancel that arrives late therefore may still finish with **success** (`Imported N records…`). A Cancel
  that is honored reports `Legacy import failed: cancelled`; collections already committed
  **remain**, the remaining ones are not imported, and the derived-data step and import bookkeeping
  are skipped, so run the import again (it is safe to repeat) rather than using a half-imported
  library.
- **Existing database.** Rows are written with *insert or replace* keyed by AniDB identifiers: a row
  with the same id (anime, file, group, MyList entry `lid`, local hash/file id, …) is replaced as
  a whole by the AOM copy, even if you fetched newer data from AniDB since; columns the
  import does not supply return to their defaults (so imported anime have no "fetched" marker
  and a later title-list merge may replace their titles). Rows that exist only in your database,
  and the command queue, are left alone. Re-importing the same databank converges to the same
  result.
- After the last collection the highest/special episode counts are recomputed, missing MyList
  group ids are filled from file records, and the source folder plus date are recorded in the
  database.
- Nothing is queued, sent to AniDB or deleted. The import does not create queue entries; the
  Queue only fills as you hash/identify or refresh. Imported MyList entries stay as the local copy
  until an identify reply for their file says otherwise (see [Hashing and identification](#hashing-and-identification)).
- Local file paths are imported verbatim (AOM's Windows paths). On Linux those paths do not
  exist; *Remove missing* only acts under an explicit folder rule whose directory exists, so
  they remain as stale records until you configure rules or remove them.
- **Temporary files.** Decrypted collections are plaintext copies of your library. They are
  written to `import-temp` next to the database file (for example
  `<database folder>/import-temp/animes.sqlite`) and deleted after each collection. The folder is
  removed on success. After **any failed or interrupted import** (a decrypt, attach, commit or detach
  error, a crash or a kill) a plain copy can remain, so look in `import-temp` after every failed
  import. The folder name is generic and may already have existed with **unrelated files**; do not
  delete the whole folder blindly. Inspect it and remove only files you can confirm were written by
  the import: `<collection>.sqlite` where the collection is one of the names listed above (for
  example `animes.sqlite`). Delete the folder itself only when it is then empty. A cancelled import
  deletes the file it was working on and may leave the empty folder.
- The `import-legacy` example in `animatic-core` (`cargo run -p animatic-core --example
  import-legacy -- <kowai> <output.sqlite3>`) runs the same import against a database file with
  `output.with_extension("import-temp")` as the temporary folder (an output named `library.sqlite3`
  gives `library.import-temp`, next to it, not `library.sqlite3.import-temp`); use it only with a copy of the data outside the
  repository and delete outputs afterwards ([development](development.md)).

## Import AOM settings

Imports AOM's `local_settings.xml` (UTF-16 or UTF-8). It is read-only for the source file.

1. **File → Import AOM settings…**, choose the XML. **Starting an import cancels whatever job is
   currently running**, then reads the file on the worker. All worker actions (add files, import
   databank, scan, queue) stay disabled until the review ends.
2. **Review imported AOM settings** shows counts (managed folders, players, hash extensions) and
   whether credentials were detected, never the values. It states that applying **replaces** your
   Animatic settings and that offline mode stays on.
3. **Apply imported settings** saves immediately and activates the result **offline**. **Cancel**, or
   closing the window, discards it. If saving fails the error is logged and the review stays open.
   Unrelated or empty XML is rejected (`No supported AOM settings found in the selected XML`) and
   your settings are unchanged.

Important: the imported settings are built on the Animatic **defaults**, not on your current
configuration. Everything the XML does not mention (client name, client version, API key, UDP
server and port, theme, scale, current folder rules when the XML has none, …) reverts to the default,
and applying overwrites your settings file. A save first tries to copy your previous valid file to
`settings.json.bak` (a failed copy is ignored, so the backup can be stale or absent); the backup
holds the *previous* settings, including credentials you may have since removed, and is only loaded
automatically when the primary is missing or invalid. Copy your settings file yourself first if you
may want it back. The one exception is `updates.auto_check`: AOM has no such preference, so applying an
import keeps your current value.

What is read: username and password, image server, file-information flag, hashing flags and extension
list, hash export template, rename pattern, statistics export settings, AVDump enable and key, last
folder, folder rules (with their MyList policy), players, window/tray/tree preferences and row
colors, folder-scan delays. Several of those have no effect in Animatic (see the
[settings reference](settings-reference.md#aom-settings-import)). Credentials are stored in plain text. After
applying: open Options, set the **Client name**, review folder rules and players, check
*Remove missing*/*Add to mylist*, then clear **Work offline** only when ready.

## Not implemented

Verified absent from the application (some exist in `animatic-core` or are stored only):

- Full-account MyList download or reconciliation; scheduled or automatic folder scanning;
  automatic renaming; background watchers.
- Adding or deleting MyList entries from the UI; editing storage, source, other or view date
  after the fact; editing anime metadata (the Edit AniDB data menu opens web pages).
- Images and covers (the HTTP image function is unused; no image UI); system tray and
  minimize-to-tray; row coloring; saved columns, sorting and window state.
- A menu item for full update (use `--full-update`); queue or player *open/enqueue* actions;
  resumable downloads; shell integration.
- Encrypted storage of credentials; Linux runtime and production AniDB verification.
- Unattended or scheduled updates, update channels (prereleases), release signatures, rollback,
  package or installer integration; an update replaces only the executable and versioned license files.

Equivalents of the original AOM workflows are tracked in [feature parity](feature-parity.md).

## Source anchors

Line numbers are for this tree and may drift; search for the function name.

| Topic | Anchor |
|---|---|
| Pages and menus | `crates/animatic/src/app.rs`: `Page::ALL`, `top_bar`, `navigation`, `dashboard`, `mylist_page`, `anime_page`, `search_page`, `hashed_page`, `groups_page`, `genres_page`, `queue_page`, `log_page`, `anime_detail_window`, `settings_window`, `bottom_bar` |
| Search filters | `app.rs`: `filter_anime`, `filter_search`, `matches_anime`, `aid_query`; `library.rs`: `SearchKeys`, `LibraryProjection::assemble` |
| Hash events, identify-after-hash | `app.rs`: `process_events`; `worker.rs`: `Job::Hash`, `Job::Scan` |
| Queue, merge, retry, clear | `crates/animatic-core/src/sync.rs`: `queue_edit_mylist`, `coalesce_edits`, `queue_peek`, `queue_retry_failed`, `queue_clear`, `execute`; `worker.rs`: `process_queue`, `fail_command` |
| Error classes and cooldowns | `crates/animatic-anidb/src/error.rs`; `udp.rs` reply mapping |
| Scan rules | `crates/animatic-core/src/file_manager.rs`: `plan_scan`, `move_file`; `settings.rs`: `FileManagerSettings::matching_manager_for` |
| Rename | `crates/animatic-core/src/rename/plan.rs`, `rename/mod.rs` (`target_path`), `app.rs`: `preview_rename`, `rename_window`, `pattern_editor_ui` |
| Exports | `crates/animatic-core/src/stats.rs`; `hash.rs`: `Hashes::export`; `app.rs`: `export_statistics`, `export_hashes` |
| External tools | `crates/animatic-core/src/external.rs` |
| Migration | `crates/animatic-core/src/legacy/{crypto,blob,import}.rs`, `settings.rs`: `import_legacy_settings`, `app.rs`: `legacy_settings_review_window` |
| Settings load/save/recovery | `settings.rs`: `load_with_recovery`, `save`; `app.rs`: `SettingsGuard`, `on_exit` |
| Updates | `crates/animatic-core/src/update.rs`: `UpdateClient`, `ReleaseInfo`, `VerifiedUpdate`; `worker.rs`: `Updater`, `UpdateJob`, `UpdateEvent`; `app.rs`: the Software updates window (`update_window`), `update_blocker`, `restart_now`; `main.rs`: executable capture and restart after `run_native` |

Related guides: [getting started](getting-started.md) · [settings reference](settings-reference.md) ·
[rename language](rename-language.md) · [troubleshooting](troubleshooting.md) ·
[architecture](architecture.md) · [storage and network contracts](storage-and-network.md).
