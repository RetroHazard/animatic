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

The window opens with the size, position and maximized state it had when you last used
Animatic (first start: 1280 × 800, minimum 900 × 600); see
[window and layout state](Settings-Reference.md#window-and-layout-state) for what is ignored
as unsafe.

All data on screen comes from a **snapshot** of the local database that a background thread
loads and swaps in whole. Changes appear after the next snapshot (requests are coalesced, at
most one every 250 ms). **File → Refresh** and the Overview "Refresh data" button request one
immediately. If loading fails, the Log shows `Unable to refresh library: …` and the old
snapshot stays.

Tables share a few behaviors: columns can be resized, rows are striped, text is truncated
instead of wrapped, and a horizontal scrollbar appears when the window is narrower than the
columns. [Headers sort, a right click chooses columns, and rows are tinted by
status](#sorting-columns-and-row-colors); column widths, hidden columns and the sort order are
remembered. The window has no keyboard shortcuts and does not accept drag-and-drop of files;
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
| Queueing full update | `--full-update`, Tools → Full update… (after confirming) | No effect | – |
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

## Sorting, columns and row colors

**Sorting.** Click a column header to sort by it: first ascending (`▲`), then descending (`▼`),
then back to the default order. Text sorts case-insensitively with numbers read by value
(`Ep2` before `Ep10`); rows that compare equal keep the default order, and a missing value
(for example an unidentified file's AniDB id) sorts before any present one. The **Queue** table
is not sortable because its order is the order commands run in.

**Columns.** Right-click a header to open the **Columns** menu and hide or show columns; at least
one stays visible, and hiding the sorted column returns the table to its default order.
Drag a header edge to resize. The last visible column fills the remaining width. Sorting and hiding do not change a selection (it follows the row, not its position), but the column you click to select a row (**File** on Hashed files, **Episode** on My list) is the only click target: keep it visible, or on Hashed files use **Select all shown**.

**Remembered.** Widths, hidden columns and the sort order of every table are kept with the
settings (`ui.columns`, see the [settings reference](Settings-Reference.md#window-and-layout-state)):
written by **Save options** and when Animatic exits normally, not while
[recovered settings](Settings-Files.md#recovery-matrix) are unsaved.

**Row colors.** With **Use status colors** on (Options → Appearance) rows are tinted by status:

| Table | Status | Written in the row as |
|---|---|---|
| Hashed files | CRC mismatch | CRC column `Mismatch` |
| | Not identified | AniDB column `Unknown` |
| | Watched | Mylist column `lid N · watched` |
| | In MyList | Mylist column `lid N` |
| Anime, Advanced search | Fully watched | Mylist column `… · all watched` |
| | In MyList | Mylist column `N eps / M …` |
| My list | Watched | Watched checkbox ticked |

Color is never the only signal: every status is also text or a control state. The text on a
tinted row is black or white, whichever reads better on the color. The colors come from
`ui.colors` (see [row colors](Settings-Reference.md#row-colors-uicolors)); plain rows stay
uncolored unless you set a color for them.

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
| Tools | Download anime titles | Queues the title-list download (nothing is sent yet). The list is requested at most once per 24 hours: inside that window after the last attempt (even a failed one) nothing is queued and the log says when the next download is allowed. |
| Tools | Full update… | Works out in the background what a [full update](AniDB-Synchronization.md#synchronization) would queue (the title list, a file refresh and an anime refresh for each identified hashed local file, minus what AniDB's limits do not allow yet and what the queue holds already) and asks for confirmation, showing the counts, how many were left out and when the title list is next allowed. **Queue** adds them (the same as `--full-update`); nothing is sent until you run the queue. Disabled during an import or settings review and while a preview is open. |
| Tools | Fetch mylist statistics | Queues a MyList-statistics request. When run, the figures are stored with the time they arrived and shown on the [Overview](Library-Pages.md#overview) as **AniDB account figures**, apart from the local figures; the log also gets a line (`N anime, N episodes, N files in mylist`). |
| Tools | Check library | Runs [`integrity_check` and `foreign_key_check`](Backups-and-Recovery.md#check-and-compact-the-library) in the background; never repairs. Disabled while a job runs, during an import or settings review, or during a restore. |
| Tools | Compact library… | After a confirmation runs `VACUUM` in the background ([details](Backups-and-Recovery.md#check-and-compact-the-library)). Disabled like Check library. |
| Tools | Edit AniDB data ▸ | Opens AniDB web pages in the browser: Add anime, Add episode, Add file, Submit change request (`anidb.net/anime/add`, `/episode/add`, `/file/add`, `/creq`). |
| Tools | Run command queue | Starts the [synchronization](AniDB-Synchronization.md#synchronization) job. Disabled during a settings review or an active cooldown. |
| Help | AniDB website | Opens `https://anidb.net/`. |
| Help | Check for updates… | Opens the [Software updates window](Updating-Animatic.md) and starts a check for a newer stable release. This is an explicit request and works even when *Work offline* is on (that setting only governs AniDB and the automatic startup check). Disabled on platforms other than Windows or Linux x86_64. |
| Help | About Animatic | Version and link, plus **View license** and **Third-party notices** buttons that open the license documents next to the program: the versioned `LICENSE-vVERSION` / `THIRD-PARTY-NOTICES-vVERSION.html` for the running version if present (left by an update), otherwise the bundled `LICENSE` / `THIRD-PARTY-NOTICES.html`. A button is disabled when neither file exists (for example a bare executable copied out of an archive). |

There is no menu item for downloading images (a
[cover](Library-Pages.md#cover) is downloaded when an anime detail window opens). MyList
adds, removals and refreshes are buttons on the [Hashed files](Library-Pages.md#hashed-files) and
[My list](Library-Pages.md#my-list) pages.

## System tray

Options → **System tray** has two controls: **Show tray icon** and **Minimize to tray** (both on by
default; the second needs the first). Both act as you click, not only on **Save options**.

- **Windows:** the icon sits in the notification area (it may be in the overflow flyout, depending
  on your taskbar settings).
- **Linux:** the icon uses the StatusNotifierItem protocol over the session D-Bus, so it needs a
  desktop with a tray host: KDE Plasma, Xfce, Cinnamon, MATE and others have one; GNOME needs the
  AppIndicator extension. Animatic needs no extra system package for it.
- **No tray available** (no host running, no session bus, or a platform without support): both
  controls are disabled and a yellow line says why. Nothing is ever hidden without an icon to bring
  the window back. If the tray host disappears while Animatic runs (for example the desktop panel
  restarts), the icon is dropped, a hidden window is shown again and the tray stays off until you
  restart Animatic.

**Minimize hides, close quits.** With *Minimize to tray* on, minimizing the window hides it and
leaves the icon. A left click on the icon, or **Show Animatic** in its menu, restores and focuses
the window. Closing the window (the title-bar button, **File → Quit**, or **Quit** in the tray menu)
still quits Animatic. On Linux, a compositor that does not report a minimized window to the
application (some Wayland sessions) never triggers the hide; the window then just minimizes.

**Tray menu:** *Show Animatic*, *Run command queue* and *Quit*. *Run command queue* does what
**Tools → Run command queue** does and is disabled under the same conditions (an AOM settings
import waiting for review, or a sync cooldown in force). It runs with the window hidden too; with
*Work offline* on it reports that it is offline and does not contact AniDB.
