The nine navigation pages in order, and the anime detail window. Search and table behavior
shared by the pages is in [Main window](Main-Window.md#searching-and-tables). The page title is the
same as the sidebar label.

## Overview

From top to bottom:

1. **Getting started** (only while `first_launch` is true, see below).
2. **Needs attention:** what asks for action now, each with the button that resolves it. When there
   is nothing, one line: `✓ Nothing needs your attention.`
3. The buttons **Add files…**, **Scan managed folders** and **Open hashed files**.
4. **Your library:** four figures counted from this database, and a line with the identification
   count.
5. **Your AniDB account:** the figures AniDB reported for the whole account.

| Tile (Your library) | Definition |
|---|---|
| Anime | Number of anime rows in the database. This includes rows created by the [title-list download](AniDB-Synchronization.md#synchronization), so it can far exceed your own library. |
| MyList entries | Number of MyList entries |
| Watched | MyList entries marked watched |
| Size on disk | Total size of hashed files (binary units, e.g. `1.23 GB`) |

Below the tiles: `N of N hashed files identified by AniDB`.

**Needs attention** lists, in this order, only what applies:

- the [hash queue](Hashing-and-Identification.md#hash-queue) panel while files wait to be hashed or
  failed (pending and failed counts with **Resume hashing**, **Retry failed** and **Clear hash
  queue**);
- a red `⚠ N files do not match the CRC in their name` (`⚠ 1 file does not match the CRC in its
  name`) when AniDB knows a CRC for a file that differs from the one Animatic computed (compared
  case-insensitively; no AniDB CRC means no mismatch), with **Open hashed files**;
- `N hashed files are not identified by AniDB yet`, with **Open hashed files**;
- a red `⚠ N commands could not be sent` for blocked queue commands, with **Open queue**;
- `N commands wait to be sent to AniDB`, with **Run command queue**; while *Work offline* is on the
  text adds `(working offline)` and the button is **Go online**, which turns *Work offline* off
  and saves the settings;
- a yellow `N commands not saved yet; Animatic keeps retrying` while queue commands are held in
  memory (see [Status bar](Main-Window.md#status-bar-progress-and-the-log)).

**Your AniDB account:** the result of the last **Tools → Fetch MyList statistics** that AniDB
answered, with the time it was received: anime, episodes, files, size, watched episodes, watch
time, **MyList watched** (share of MyList watched) and votes / reviews. Before any result the section reads
`No account statistics yet.` with a **Fetch MyList statistics** button (it only queues the
request). They describe your whole AniDB account, so they differ from the local tiles above (which
count only the rows stored in this database); the page says so and never mixes the two. The result is
kept in the database (the `mylist_stats` entry of the `meta` table) and shown again after a
restart; each new result replaces the old one.

There is no refresh button: the page follows the library snapshot, and **File → Reload** (`F5`)
re-reads it at once.

The **Getting started** card is shown while `first_launch` is true. It is a checklist: ✓ or ○ for
*Enter your AniDB account, or keep working offline for now* (button **Open account options**, done
once a user name is entered) and *Choose the folders Animatic watches and renames into* (button **Open
folder options**, done once a folder rule has a path); both open [Options](Options-Window.md) at the
matching category. Below it, "Coming from AniDB O'Matic (AOM)?" has a folder box, **Browse…** and
**Import** (same as
[File → Import → AOM databank…](Migrating-from-AOM.md#migrate-an-aom-databank), but you may paste a folder path). **Hide**
clears the flag and writes the settings file at once (unless
[recovered settings](Options-Window.md) are guarding it).

## MyList

The page groups the MyList by anime: one top-level row per anime, collapsed by default, and the
anime's MyList entries (episodes and files) as child rows when it is expanded. Click the arrow in
front of the title to expand or collapse one anime; **Expand all** and **Collapse all** in the
toolbar do it for every anime in the table (the expand state is kept while you work and is not
saved when you quit). Child rows are always in AniDB episode order: regular episodes by number,
then specials (`S`), credits (`C`), trailers (`T`), parodies (`P`) and others (`O`) by type and
number; entries whose episode is unknown come last.

Columns of an **anime row**: **Anime** (the title; a link that opens the
[detail window](#anime-detail-window)), **Episode** (`12 eps`, or `15 files` when an episode has
several files; hover shows both counts; **click it to select every entry of the anime**),
**Group** (the group's name, or `various`), **State** (a combo box showing the common state, or
`Mixed`), **Watched** (a checkbox: ticked when every entry is watched, unticked when none is, a
dash when only some are), **Size** (the sum) and **Queue** (how many entries wait for an edit, a
removal or are blocked, for example `2 queued, 1 blocked`). The row is tinted like a watched
entry once all of its entries are watched.

Columns of an **entry row**: **Anime** (the episode's title, indented), **Episode** (`12`, `S1`,
`C1`, `T1`, `P1`, `O1`, or `—` if unknown; **click it to select the entry**), **Group** (short
name, else full name), **State** (combo box), **Watched** (checkbox), **Size**, **Queue**
(`Edit queued`, `Removal queued`, or the same with `blocked`; hover shows the last error).

Choosing a **State** or ticking **Watched** on an anime row applies it to every entry of the
anime in one step: one merged `Edit mylist entry N` per entry, all saved in a single database
transaction, with the new values shown at once. Entries that already have the value are left alone
(nothing is queued for them and a watched date is not restamped), entries waiting to be removed
are skipped, and marking watched stamps the moment of the click. Clicking the dash of a partly
watched anime marks every entry watched; click again to unmark them all.

The sort buttons in the headers sort the anime rows by their totals (title, episodes/files,
group, state, watched share, size, queue); the entries below an anime stay in episode order
whatever the sort. Without a sort the anime are in title order. The search box filters by anime
title: a matching anime shows with all of its entries.

The page lists the MyList entries stored locally. They come from an AOM import, from files
identified or added through the [queue](#queue), or from a
[MyList export import](AniDB-Synchronization.md#full-account-mylist-import) (**File → Import →
MyList export…**); Animatic never downloads your MyList entry by entry (see
[Not implemented](Feature-Parity.md#not-implemented)). State and Watched toggle in the row; the
buttons above the table act on the selection: an entry (click its episode) or all entries of an
anime (click the episode count of its row). A weak hint says so until something is selected, then
the toolbar shows `Entry N · <anime — episode>` or `<anime> · all N entries selected`. All changes
are queued first, so they work offline; see
[MyList editing](AniDB-Synchronization.md#mylist-editing).

| Button | Does |
|---|---|
| **Edit…** | Opens the entry editor for the selected entry (below), or for every entry of the selected anime. Disabled while the entry waits to be removed (for an anime: while all its entries do; entries already waiting are left out). |
| **Remove from MyList…** | Opens a confirmation dialog (Esc or a click outside cancels); its **Remove from MyList** button queues `Remove mylist entry N`, one per selected entry (every entry of the anime for an anime row). Your media files are never touched. The rows stay until AniDB confirms, show `Removal queued` meanwhile and `Removal blocked` if the request failed permanently (they stay listed on the [Queue](#queue) page until you retry or discard them). Edits of the entries still waiting in the queue are discarded; later edits are refused while the removal waits. |
| **Refresh from AniDB** | Queues `Refresh mylist entry N` for the selected entry, or for each entry of the selected anime. The reply replaces the cached row but never overwrites a change you made that has not been sent yet. If AniDB says the entry no longer exists, the row is removed unless you have queued changes for it. |
| **Expand all** / **Collapse all** | Show the entries of every anime in the table, or one row per anime. |

**Entry editor** (window **Edit MyList entry**, with the anime and episode below the title; for an
anime row the title names the anime and how many entries are edited). Fields: **state**, **watched**, **view date**, **storage**, **source** and
**other**. Each has a *Change* checkbox, ticked automatically when you edit the value. An
**unticked field is left as it is on AniDB; a ticked empty text field clears it** (the same rule as
the [folder policy](Options-Window.md)). The view date (`YYYY-MM-DD HH:MM` or `YYYY-MM-DD`, in your
local time zone) is offered while the entry is watched; ticking it with no text means *now*. Marking
an entry watched stamps the moment you saved it, not the moment AniDB hears of it, and marking it
unwatched drops the date. **Save** queues one merged `Edit mylist entry N` (merged with any edit still
waiting, so a stale acknowledgement never undoes a newer edit) and shows the new values at once;
**Cancel** changes nothing. The edit survives a restart. Edited for an anime, the fields start
unticked and empty (the entries differ, so there is nothing to show), a ticked field is set on every
entry that does not already have the value, and all the changes are saved in one transaction.

State choices are `Unknown`, `On HDD`, `On CD/DVD`, `Deleted`, `Remote/Shared` (stored
as 0–4), shown the same way in the closed combo box. Any other stored value
shows as `Other (N)`; it cannot be selected.

## Anime

Subtitle: *every anime stored locally*. Columns: **Title** (click to select and open the detail window), **Type** (`TV Series`, `OVA`,
`Movie`, `Other`, `Web`, `TV Special`, `Music Video`, else `Unknown`), **Year**, **Episodes**
(total count; 0 if unknown), **Rating** (two decimals, blank when none), **MyList**
(`N eps / M files` when any of its files are in MyList, followed by ` · all watched` when every
episode is watched).

**Filtering by genre.** A `Genres:` line above the table shows the chosen genres, each as a
button with **✕** that removes it, and a **Filter by genre…** / **Add genre…** list (each genre
with its anime count) that adds one. With several genres chosen the page lists the anime that
have **all** of them, and **Clear** removes every genre. Clicking a genre on the
[Genres](#genres) page or in an anime's detail window shows that genre alone; **Ctrl+click** (or
Shift+click) adds it to the genres already chosen. Picking a genre that way clears the search
text, which then narrows the list as usual. The genres stay chosen while you move between pages.

Search is described in [Searching and tables](Main-Window.md#searching-and-tables).

## Advanced search

Fields, combined with AND:

| Control | Rule |
|---|---|
| Title or AniDB ID | As the shared search |
| Year | Text typed here (trimmed) must be **contained in** the year text: `2012` matches `2012`; `20` matches 2012 and 2020 |
| Type combo | Any type, TV Series, OVA, Movie, Other, Web, TV Special, Music Video |
| In MyList | Anime has at least one MyList entry |
| Fully watched | Watched regular episodes ≥ the anime's episode count (total, else highest known number); requires a known count |

Result columns match the Anime page, but the MyList column reads `N eps / M watched`
(distinct episodes in MyList / watched episodes, followed by ` · all watched` when every
episode is watched). Clicking a title opens the detail window.

## Hashed files

Local files Animatic has hashed, by default sorted by path. Columns: **File** (click to select; Ctrl+click
toggles a file and Shift+click selects the range from the first selected file),
**Size**, **AniDB** (`fid N` or `Not identified`), **MyList** (`In MyList · lid N`, `Watched · lid N` once watched, `Add queued` while an add waits in
the queue, or `—`), **CRC** (local CRC32
in upper-case hex, or `Mismatch`), **Path** (double-click opens the file with the operating
system's default program). The toolbar ends with the count, `N files` or `M of N files` under a
filter, plus `· N selected` when files are selected.
If a search hides the selected row the selection is cleared so no action can hit an
invisible file; **Add to MyList…** and **Rename N selected…** likewise only act on files that
are listed. There is one selection on this page: the files you select this way are the ones
both of those buttons use.

**Hash queue panel.** While files wait to be hashed (or failed), a framed panel between the
toolbar and the table shows `N files waiting to be hashed` and `M failed`, and lists them (up to
500: failed files first with their reason, then waiting files in hashing order, each tagged with
where it came from: Add files, Command line, Scan or Other launch). Its buttons: **Resume
hashing**, **Retry failed** and **Clear hash queue** (they wait while another task runs). With
*Start hashing automatically* off, a note says hashing does not resume on its own. See
[Hash queue](Hashing-and-Identification.md#hash-queue).

The toolbar wraps onto several lines instead of clipping and groups its buttons: **Add files…** |
**Rename…**, **Rename N selected…**, **Add to MyList…**, **Remove from list…** | **Play**, **Open file**, the player buttons,
**AVDump** | **Select all**, **Clear selection**, the count. Buttons that need a selected file
(Rename…, Remove from list…, Play, Open file, Open in player, Enqueue and AVDump) are disabled until you select
one; hover the disabled button for the reason. **Add to MyList…**, **Rename N selected…** and **Remove from list…** take
one or several (with nothing selected the second reads **Rename selected…**). Right-clicking a file name also offers **Remove from list…**:

| Button | Does |
|---|---|
| Add files… | File picker (multiple); queues the files and starts [hashing](Hashing-and-Identification.md) |
| Rename… | Previews a [manual rename](Renaming-Files.md) of the selected file |
| Rename N selected… | Plans the selected files in the background and opens the [Batch rename](Renaming-Files.md#several-files-batch-rename) preview |
| Select all / Clear selection | Selects every listed file / clears the selection (**Clear selection** appears once files are selected) |
| Open file | Opens the file with the OS default program (not your configured player) |
| Play | Starts the chosen [external player](External-Tools.md) (the default one unless you pick another) with its *Play* arguments |
| Player chooser | Appears next to Play only when several players are configured; lists their names |
| Open in player, Enqueue | Appear only when the chosen player has *Open* or *Enqueue arguments* set; start it with those. Right-clicking a file name offers the same actions |
| AVDump | Enabled only when AVDump integration is on; starts [AVDump](External-Tools.md) |
| Add to MyList… | Opens a dialog for the selected files: state, watched, view date, storage, source and other (tick a field to set it; unticked fields keep AniDB's defaults). **Queue add** queues one `Add to mylist` per file: an identified file by its file id, an unidentified one by size and ED2K hash, which AniDB resolves first (a file AniDB does not know is skipped when the queue runs). Files already in MyList, files with an add already waiting and further copies of the same content are left out; the dialog says how many. The MyList column shows `Add queued` at once and `lid N` after AniDB confirmed. |
| Remove from list… | Asks first (Esc or Cancel closes the window), then removes Animatic's records of the selected files, one or several: their hash, path and [media properties](External-Tools.md#media-properties-ffprobe). **Your media files on disk are not touched, and neither are MyList entries** (the local rows and anything queued for AniDB). It is a manual cleanup of the list, never automatic: a later [scan](Managed-Folders-and-Scans.md) of the folder or **Add files…** lists the file again (and hashes it again). It runs as a background task, so it waits while another task runs, and the log says `Removed N files from the list` |

"AniDB: fid N" means the hash matches a file record already stored locally (from the import or
from an identify command that ran); `Not identified` means no such record, not necessarily that
AniDB does not know the file.

## Groups

Searchable table: **Name**, **Short name**, **Rating**, **Votes**, **Website** (`Open` link
launches the browser; `—` if none), **Refresh** (queues `Fetch group N`; the reply replaces the cached group).

## Genres

Searchable table: **Genre** and **Anime** (number of anime carrying it). Click a genre to list its
anime on the [Anime](#anime) page; Ctrl+click adds it to the genres already chosen there.

## Queue

The durable AniDB command queue (subtitle: *AniDB commands waiting to be sent*). Header: **Run command queue**, **Retry failed**,
`N pending commands`, a red `N blocked` (or `blocked commands (refreshing)`), and, at the right, **Clear queue…**.
The sidebar entry shows the number of pending commands, or a red `N blocked`.

| Button | Does |
|---|---|
| **Run command queue** | Starts the same job as **Tools → Run command queue** (shortcut `Ctrl+Shift+S`). Disabled during a settings review or an active cooldown (the tooltip shows the cooldown). |
| **Retry failed** | Enabled when something is blocked. Unblocks every blocked row with fresh attempts (edits of the same MyList entry are merged first). Does not run anything and does not bypass a cooldown. |
| **Clear queue…** (red) | Disabled while the queue is empty. Asks for confirmation in a dialog ("Clear the command queue?"); **Discard commands** then deletes *every* queue row, blocked ones and unsynchronized MyList edits included. **Cancel**, Esc or a click outside discards nothing. See [MyList editing](AniDB-Synchronization.md#mylist-editing) for the effect on the local copy. |

An empty queue shows "Nothing is waiting for AniDB" with an explanation instead of the table.
A yellow line repeats an active cooldown. While **Run the command queue automatically** is on, a
grey line says that automatic runs never remove MyList entries, and a yellow line counts the
removals that wait for you to click **Run command queue**. Table columns: **ID** (`#n`, never reused),
**Command**, **Status**, **Last error** (hover for the full text).

| Status | Meaning |
|---|---|
| `Pending` | Not yet attempted |
| `N retries` (yellow) | Attempted before; still runnable |
| `Blocked` (red) | Skipped until **Retry failed** |

Order of execution (and of the list, blocked rows last): lower priority number first, then
oldest. Priorities: MyList writes 0; Identify 1; file/episode/group lookups 2; anime refresh,
MyList statistics, title list, MyList export 3; MyList entry refresh 4.

Command descriptions you will see: `Identify file <ed2k> (<n> bytes)` (with ` + add to
mylist` when the folder rule adds it), `Add to mylist: file N` or `Add to mylist: <ed2k> (<n>
bytes)`, `Edit mylist entry N`, `Remove mylist entry N`, `Refresh mylist entry N`, `Fetch anime N`,
`Fetch episode N`, `Fetch group N`, `Fetch file N` (from the full update), `Fetch mylist
statistics`, `Download anime title list`, `Request MyList export from AniDB`, `Cancel the queued
MyList export`. A row whose stored payload this version cannot read is kept, blocked, as `Unreadable
queued command: …`; it is never deleted silently.

## Log

In-memory activity log for this session (subtitle: *this session's activity*), newest at the bottom (it scrolls with new lines). **Clear**
empties it (disabled while the log is empty, which shows "No activity yet" with an explanation instead). Lines with `ERROR` are red.
It is not written to disk, resets on exit, and is trimmed to recent entries once it grows past about 2000.
It records starts, hash results (`Hashed <path> (local #N, <ed2k>)`), sync outcomes, blocked
commands, cooldowns, imports, exports and every error. Hash output reveals file paths and
hashes, so mind what you paste when asking for support.

## Anime detail window

Opened from any anime link or title (or a Relations entry). The window title is the anime's name
(`Anime N` until the record is available). At the top: **Open on AniDB** and **Refresh from AniDB**. Then the [cover](#cover) on the
left, `type · year · N episodes · AniDB <id>`, the description if one is stored, and collapsible sections, each
listed with its count: **Episodes (N)** (open by default, a grid of episode number, name and a **Refresh** button that queues `Fetch episode N`), **Titles** (language
code and text), **Genres** (each genre is a link that opens the [Anime](#anime) page filtered to
it, or with Ctrl+click adds it to the chosen genres, and closes the window), **Relations** (relation
kind and title; clicking one switches the window to that anime, and shows `No detail record is
available.` if it is not stored), **Reviews** (author or `Anonymous`, rating, body). The
description and review bodies keep AniDB's formatting: `[b]`, `[i]`, `[u]`, `[s]` and `[url]`
tags show as emphasised (brighter), italic, underlined, struck-through and link-coloured text,
and so do AniDB's `http://anidb.net/ch123 [Name]` links (links are not clickable); `<br />` is a
line break, character references such as `&amp;` are the character, and the backtick AniDB
stores for an apostrophe is shown as `'`. `[quote]` is italic, `[spoiler]` is italic after
`Spoiler:`, `[hr]` is a line across, `[ul]`/`[ol]` with `[li]` or `[*]` become bulleted or
numbered lines, and `[img]` shows `Image:` and the picture's address; pictures are never
downloaded. Other bracketed text is shown as written. A section with no entries shows `(0)` and
cannot be opened.

- **Open on AniDB** opens `https://anidb.net/anime/<id>` in your browser.
- **Refresh from AniDB** queues `Fetch anime N` (HTTP API). Nothing is sent until you run the
  queue; a duplicate of an already queued refresh is ignored. AniDB asks clients to request an
  anime at most once per 24 hours, so inside that window after the last request (successful or
  not) the button queues nothing and the status bar and log say when the refresh is allowed
  ([Request limits](AniDB-Synchronization.md#request-limits)).

### Cover

The cover sits left of the heading. It comes from a folder on disk, never from a bulk download:

- A picture that is already in the [covers folder](Data-Locations.md#cover-pictures) is shown at
  once, whatever the network state, *Work offline* or **Download anime covers** say. It is never
  downloaded again; if AniDB gives the anime a new picture, the new one is a new file.
- Otherwise, opening the window asks AniDB's image server for that one picture, but only when
  **Download anime covers when a detail window opens** is on (Options → AniDB account, on by
  default), *Work offline* is off, no ban or [cooldown](AniDB-Synchronization.md#request-limits)
  is active, and the last try for this anime is more than an hour ago. It is one request, paced
  like every AniDB request (the same 2 s spacing as the queue), made in the background: the window
  shows `Loading cover…` and stays usable. No other picture is ever requested, and nothing
  downloads covers for the whole library.
- When no picture is shown the placeholder says why, for example `Cover not downloaded
  (offline)`, `Cover not downloaded (cover downloads are off in Options)` or `No cover on record`
  (the anime has no picture name yet; refresh it from AniDB). **Try again** asks once more; closing
  and reopening the window does too.
- JPEG, PNG, WebP and GIF pictures are read (large ones are scaled down for display). A download
  that is not a readable picture is shown as an error and is not kept; a kept file that turns out
  unreadable is removed and fetched again.
- Covers are for your own display. AniDB does not give a licence for the artwork, so Animatic keeps
  covers only on your computer, never uploads, exports or re-hosts them, and library backups do not
  contain them. Copying the covers folder to share it is not what it is for.

Close the window with its title-bar button or with Esc.
