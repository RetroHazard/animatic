The nine navigation pages in order, and the anime detail window. Search and table behavior
shared by the pages is in [Main window](Main-Window.md#searching-and-tables).

## Overview

Four tiles and a health summary:

| Tile | Definition |
|---|---|
| Anime | Number of anime rows in the database. This includes rows created by the [title-list download](AniDB-Synchronization.md#synchronization), so it can far exceed your own library. |
| Mylist files | Number of MyList entries |
| Watched | MyList entries marked watched |
| Local size | Total size of hashed files (binary units, e.g. `1.23 GB`) |

**Local library health:** `X of Y hashed files identified by AniDB`; a red line `N files have a
CRC mismatch` when AniDB knows a CRC for a file that differs from the one Animatic computed
(compared case-insensitively; no AniDB CRC means no mismatch); `N files are not identified yet`.

Buttons: **Add files**, **Open hashed files**, **Refresh data**.

The **first-run card** ("Bring your AniDB O'Matic library forward") is shown while
`first_launch` is true. It has a folder box, **Browse…**, **Import** (same as
[File → Import AOM databank…](Migrating-from-AOM.md#migrate-an-aom-databank), but you may paste a folder path) and
**Dismiss first-run card**, which clears the flag and writes the settings file at once (unless
[recovered settings](Options-Window.md) are guarding it).

## My list

Columns: **Anime** (link; opens the [detail window](#anime-detail-window)), **Episode**
(`12`, `S1`, `C1`, `T1`, `P1`, `O1`; blank if unknown), **Group** (short name, else full
name), **State** (combo box), **Watched** (checkbox), **Size**.

The page lists the MyList entries stored locally. They come from an AOM import or from files
identified through the [queue](#queue); there is no "download my whole MyList" action (see
[Not implemented](Feature-Parity.md#not-implemented)). State and Watched are the only editable fields; see
[MyList editing](AniDB-Synchronization.md#mylist-editing).

State choices are `Unknown`, `On HDD`, `On CD/DVD`, `Deleted`, `Remote/Shared` (stored
as 0–4). The closed combo box abbreviates some labels (`On CD`, `Shared`) and can show
`Release` for AniDB state 5, which you cannot select.

## Anime

Columns: **Title** (click to select and open the detail window), **Type** (`TV Series`, `OVA`,
`Movie`, `Other`, `Web`, `TV Special`, `Music Video`, else `Unknown`), **Year**, **Episodes**
(total count; 0 if unknown), **Rating** (two decimals, blank when none), **Mylist**
(`N eps / M files` when any of its files are in MyList).

Search is described in [Searching and tables](Main-Window.md#searching-and-tables).

## Advanced search

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

## Hashed files

Local files Animatic has hashed, sorted by path. Columns: **File** (select by clicking),
**Size**, **AniDB** (`fid N` or `Unknown`), **Mylist** (`lid N` or `—`), **CRC** (local CRC32
in upper-case hex, or `Mismatch`), **Path** (double-click opens the file with the operating
system's default program). The row count reads `N files` or `M of N files` under a filter.
If a search hides the selected row the selection is cleared so no action can hit an
invisible file.

Buttons (Rename, Open, Play and AVDump need a selected file):

| Button | Does |
|---|---|
| Add files… | File picker (multiple); starts [hashing](Hashing-and-Identification.md) |
| Rename… | Previews a [manual rename](Renaming-Files.md) |
| Open | Opens the file with the OS default program (not your configured player) |
| Play | Starts the default [external player](External-Tools.md) with its *Play* arguments |
| AVDump | Enabled only when AVDump integration is on; starts [AVDump](External-Tools.md) |

"AniDB: fid N" means the hash matches a file record already stored locally (from the import or
from an identify command that ran); `Unknown` means no such record, not necessarily that
AniDB does not know the file.

## Groups

Searchable table: **Name**, **Short name**, **Rating**, **Votes**, **Website** (`Open` link
launches the browser; `—` if none).

## Genres

Searchable table: **Genre** and **Anime** (number of anime carrying it).

## Queue

The durable AniDB command queue. Header: `N pending commands`, a red `N blocked` (or
`blocked commands (refreshing)`), and three buttons:

| Button | Does |
|---|---|
| **Clear queue** | Deletes *every* queue row, blocked ones and unsynchronized MyList edits included, with no confirmation. See [MyList editing](AniDB-Synchronization.md#mylist-editing) for the effect on the local copy. |
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

## Log

In-memory activity log, newest at the bottom (it scrolls with new lines). **Clear** empties
it. It is not written to disk, resets on exit, and is trimmed to recent entries once it grows past about 2000.
It records starts, hash results (`Hashed <path> (local #N, <ed2k>)`), sync outcomes, blocked
commands, cooldowns, imports, exports and every error. Hash output reveals file paths and
hashes, so mind what you paste when asking for support.

## Anime detail window

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
