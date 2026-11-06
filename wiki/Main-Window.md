The parts of the window that every page shares: layout, the status bar and progress, searching
and tables, and the menus. The individual pages are described in [Library pages](Library-Pages.md).

## Window layout

- **Menu bar** (top): [File, Tools, Help](#menus).
- **Navigation** (left): nine pages in this order: Overview, My list, Anime, Advanced search,
  Hashed files, Groups, Genres, Queue, Log. At the bottom it shows `N anime` and
  `N mylist files` (counts of the loaded snapshot).
- **Page** (center): the selected page. Table pages are virtualized: only the visible rows
  are drawn, so very large libraries stay responsive.
- **Status bar** (bottom): current activity or last message, connection state and warnings.
- **Floating windows**: [Anime detail](Library-Pages.md#anime-detail-window), [Options](Options-Window.md),
  "Confirm rename", "Review imported AOM settings", "Import into a database that has data?",
  "Leftover import files", "Settings recovered", the
  "Restore library", "Restoring library", "Library unavailable" and "Compact library" dialogs
  (modal: nothing else can be used while one is open), the
  [Software updates](Updating-Animatic.md) window and "About Animatic" (the build's version from
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
| `AniDB sync paused until HH:MM:SS: <reason>` (yellow) | An active [cooldown](AniDB-Synchronization.md#failure-handling); the time is local. |
| `N blocked` / `blocked commands (refreshing)` (red) | Blocked queue entries, or a blocked event newer than the snapshot on screen. |
| `N commands NOT saved yet` (yellow) | Queue commands held in memory because the database was busy. See below. |

Jobs and what the controls do:

| Job (status label) | Started by | Pause | Cancel |
|---|---|---|---|
| Hashing files | Add files…, command-line paths | Pauses between 4 MiB blocks | Stops after the current block; the file in progress is discarded (its error `…: cancelled` is logged) and the remaining files are skipped |
| Scanning managed folders | Tools → Scan managed folders | Pauses while a file is being hashed | Stops before the next planned action |
| Importing legacy databank | File → Import AOM databank… | No effect | Checked only between steps (see [migration](Migrating-from-AOM.md#migrate-an-aom-databank)); a late Cancel can still end in success |
| Reading AOM settings | File → Import AOM settings… | No effect | No effect; the review window decides whether anything changes |
| Synchronizing with AniDB | Run command queue / Synchronize now | No effect | Stops before the next command; a request already in flight finishes or times out (UDP 20 s, HTTP 60 s) |
| Queueing full update | `--full-update` | No effect | – |
| Backing up library | File → Back up library… | No effect | No effect; it runs to completion and a half-written file is never left |
| Checking backup file | File → Restore library… | No effect | No effect; the check changes nothing |
| Checking library | Tools → Check library | No effect | No effect |
| Compacting library | Tools → Compact library… | No effect | No effect; `VACUUM` is atomic |

Jobs run one at a time in submission order. Library snapshots and the statistics and hash exports run on
their own thread and do not show a progress bar or take part in Cancel.

**NOT saved yet.** Identify, MyList add, removal and refresh, episode and group refresh, title-download, statistics-fetch and anime-refresh commands are
written to the durable queue. If the worker holds the database lock, the command is kept in
memory, flagged in the status bar and retried every 500 ms with no retry-count ceiling. Up to 1000
distinct unsaved commands can be held at once; a new command is refused with `NOT queued: …` only
when 1000 others are already unsaved. They are not durable until they appear on the Queue
page. On exit Animatic tries once more for up to 5 seconds; if that fails the loss is only
written to a log that is not installed, so wait for the indicator to clear before quitting.
MyList edits are different: if they cannot be saved immediately you see
`Could not edit mylist entry N: the database is busy…; nothing was changed, try again`.

**Log page / activity log.** See [Log](Library-Pages.md#log). Messages carry `HH:MM:SS`; errors add `ERROR`.

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

## Menus

| Menu | Item | Behavior |
|---|---|---|
| File | Add files… | Picker; hashes the chosen files. Disabled during a settings review. Remembers the last folder. When *Restrict file extensions* is on the picker filter is "Configured media files". |
| File | Import AOM databank… | Folder picker → [databank import](Migrating-from-AOM.md#migrate-an-aom-databank); a database that already holds data first shows **Import into a database that has data?** (Continue / Cancel). Disabled during a settings review. |
| File | Import AOM settings… | File picker (`.xml`) → [settings import](Migrating-from-AOM.md#import-aom-settings). |
| File | Export statistics… | Save dialog (default `animatic-stats.xml`/`.ini`) → [statistics export](Exports.md#statistics-export). |
| File | Export hashes… | Save dialog (default `animatic-hashes.txt`) → [hash export](Exports.md#hash-export). |
| File | Back up library… | Save dialog (default `animatic-backup-YYYYMMDD-HHMMSS.sqlite3`) → [online backup](Backups-and-Recovery.md#back-up-and-restore-inside-animatic) of the library. Disabled while a job runs, during an import or settings review, or during a restore. |
| File | Restore library… | File picker → background validation → confirmation window → [restore](Backups-and-Recovery.md#back-up-and-restore-inside-animatic). Disabled for the same reasons as Back up library and while commands are not saved yet. |
| File | Refresh | Requests a new snapshot. |
| File | Quit | Closes; saves settings (unless guarded) and tries to save unsaved commands. |
| Tools | Options… | Opens [Options](Options-Window.md). |
| Tools | Scan managed folders | Starts a [scan](Managed-Folders-and-Scans.md). Disabled during a settings review. |
| Tools | Download anime titles | Queues the title-list download (nothing is sent yet). |
| Tools | Fetch mylist statistics | Queues a MyList-statistics request. When run, the figures are stored with the time they arrived and shown on the [Overview](Library-Pages.md#overview) as **AniDB account figures**, apart from the local figures; the log also gets a line (`N anime, N episodes, N files in mylist`). |
| Tools | Check library | Runs [`integrity_check` and `foreign_key_check`](Backups-and-Recovery.md#check-and-compact-the-library) in the background; never repairs. Disabled while a job runs, during an import or settings review, or during a restore. |
| Tools | Compact library… | After a confirmation runs `VACUUM` in the background ([details](Backups-and-Recovery.md#check-and-compact-the-library)). Disabled like Check library. |
| Tools | Edit AniDB data ▸ | Opens AniDB web pages in the browser: Add anime, Add episode, Add file, Submit change request (`anidb.net/anime/add`, `/episode/add`, `/file/add`, `/creq`). |
| Tools | Run command queue | Starts the [synchronization](AniDB-Synchronization.md#synchronization) job. Disabled during a settings review or an active cooldown. |
| Help | AniDB website | Opens `https://anidb.net/`. |
| Help | Check for updates… | Opens the [Software updates window](Updating-Animatic.md) and starts a check for a newer stable release. This is an explicit request and works even when *Work offline* is on (that setting only governs AniDB and the automatic startup check). Disabled on platforms other than Windows or Linux x86_64. |
| Help | About Animatic | Version and link, plus **View license** and **Third-party notices** buttons that open the license documents next to the program: the versioned `LICENSE-vVERSION` / `THIRD-PARTY-NOTICES-vVERSION.html` for the running version if present (left by an update), otherwise the bundled `LICENSE` / `THIRD-PARTY-NOTICES.html`. A button is disabled when neither file exists (for example a bare executable copied out of an archive). |

There is no menu item for a full update (command line only) or for downloading images. MyList
adds, removals and refreshes are buttons on the [Hashed files](Library-Pages.md#hashed-files) and
[My list](Library-Pages.md#my-list) pages.
