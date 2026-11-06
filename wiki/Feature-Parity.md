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
| Startup databank loading | Desktop | **File > Import AOM databank...** runs a cancellable import of an encrypted `kowai` directory in the worker; the source is only read. A command-line example (`import-legacy`) imports into a chosen file | Transactions are **per collection**: a failing collection is skipped with a warning and earlier ones stay. Cancel is checked between steps, not inside the final transaction or derived-data step, so a late cancel can still finish successfully. Designed for a new database; nothing merges with existing rows that share ids. Decrypted plaintext copies can remain in the temporary folder after any failed import or a crash; that folder may hold unrelated files, so inspect it and remove only confirmed importer output. See [Architecture](https://github.com/RetroHazard/animatic/blob/main/docs/architecture.md#legacy-migration) |
| Hash queue | Desktop | **Add files...** and command-line paths hash in one pass (ED2K-red, CRC32, MD5, SHA-1) with progress, pause and cancel | The pending list lives in memory only; it is not persisted. Only the ED2K-red variant is used by the application (`Ed2kVariant::Blue` is core only). Media-property extraction (fps, codecs, ...) is not performed |
| Batch/file manager | Partial | **Tools > Scan managed folders** applies per-folder rules: path, recursion, hidden-file exclusion, auto-hash, auto-rehash on changed modification time, auto-remove of *database records* for files affirmatively missing under a reachable root; plus per-folder MyList add/state/watched/storage/source/other policy and a rename base path | The scan is manual: no schedule, and the scan always filters by the hashing extension list. Discovery (hash/rehash) is **first rule wins in list order** among rules with a path and auto-hash or auto-rehash; longest-path matching applies only to the post-hash MyList policy, the rename base path and auto-remove matching. The scan itself never deletes or moves media files and sends nothing to AniDB, but each newly hashed file can queue an `Identify` that carries a MyList add when identify-after-hashing is on and the matching folder has MyList auto-add enabled; that queued add reaches AniDB when the queue is run. `auto_rename`, `clear_empty_folders`, `exclude_system` and the delay/sleep timers are stored only |
| Hashed files | Desktop | Known/unknown inventory with AniDB and MyList columns, CRC mismatch indication, Open (operating-system default handler), Play, AVDump, Rename, Identify after hashing | Identification is queued work: nothing is sent until the queue is run |
| Anime/mylist trees | Desktop | Searchable **Anime** and **My list** tables, a detail window with titles, episodes, genres/tags, relations (clickable) and links to AniDB | Tables, not trees. Reviews appear only if they came from a legacy import. No pictures or covers are shown |
| Advanced search | Desktop | Title (or exact id), year, type, MyList-only, watched-only filters over the local cache | Local data only |
| Groups and genres | Desktop | Searchable lists with library counts | |
| Add/edit MyList | Partial | State and Watched are editable in **My list**; edits are merged per entry, saved with the optimistic local change in one transaction and synchronized by the queue. New files can be added to MyList automatically through the folder policy (`Identify` with an add) | There is no control to add an arbitrary entry, delete an entry, or edit an entry's storage/source/other text (only folder policy at add time). `MylistAdd`, `MylistDelete` and `MylistRefresh` commands exist in the core (Core only). A FILE reply that explicitly reports "not in MyList" removes the matching *local* entry unless a command for it is queued; the absence of a delete control does not mean synchronization never removes local rows |
| AniDB session | Desktop | UDP login, optional encrypted session (API key), compressed replies, flood spacing, session re-authentication, typed replies, offline mode, persisted cooldowns, blocked failures with explicit retry | The queue runs only on request. The client version is a settings-file/import value with no Options field. The UDP channel is not TLS ([Transport security](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#transport-security)). Keep-alive `PING` and idle logout (`idle_logout_minutes`) are not used |
| Full update | Partial | `--full-update` queues the title dump, a `FetchFile` for each file id and a `FetchAnime` for each anime id found among the *identified hashed local files* (idempotent); **Tools > Download anime titles**, **Fetch mylist statistics** and the anime detail **Refresh** queue single items | It does not refresh every cached entity or your whole MyList. No menu action for a full update. The statistics result is only logged. No per-anime refresh throttle |
| Statistics export | Desktop | **File > Export statistics...** and optional automatic export, XML or INI, computed on the projection thread | The file is written in place (not an atomic replace). The figures come from several separate queries, not one snapshot, and a failed scalar total counts as 0 |
| HTTP API and images | Partial | Anime XML and the public title dump are fetched through queued commands | Image download (`HttpClient::image`) and the image-server setting are **Core only / Stored only**; no picture is displayed |
| Rename files | Partial | Compatible expression language, user functions, database-backed `A`/`E`/`F`/`G`/`H`/`M` objects, live diagnostics, preview and explicit one-file apply ([Rename language](Rename-Language.md)) | Manual only; **automatic renaming is not implemented**. The file must be AniDB-identified with its episode and anime cached. Applying is a plain `fs::rename` with no cross-volume fallback or checksum ([guarantees](https://github.com/RetroHazard/animatic/blob/main/docs/architecture.md#what-the-code-guarantees-and-what-it-does-not)) |
| Hash export | Partial | **File > Export hashes...** writes one line per hashed file from a template | Only `%filename%`, `%filepath%`, `%ed2k%`, `%md5%`, `%sha1%`, `%crc32%`, `%size%` (short text) and `%fulled2k%` are real; the media tokens (`%fps%`, `%length%`, `%height%`, `%width%`, `%vcodec%`, `%vbitrate%`, `%acodec%`, `%asample%`, `%abitrate%`) are replaced with constants. The file is written on the UI thread with `fs::write` |
| Players/AVDump | Partial | A configured default player launches with its *play* arguments; AVDump launches when enabled; both start the executable directly (no shell) | The player `open` and `enqueue` templates (`PlayerAction::Open`/`Enqueue`) are **Core only**: the Options window edits the executable and play arguments (the `name` control is never read), and the **Open** button uses the operating-system default handler instead. Placeholders are substituted as raw text into the template *before* it is split with POSIX-style `shell_words` rules. Quoting is only a mitigation: a space or quote inside a substituted value (path, `{user}`, key) can split it or inject extra arguments (not shell execution). Omitting `{path}`/`%file%` from a player `play` template appends the path as one intact argument; AVDump has no such route. Launched programs are not sandboxed |
| Settings | Partial | JSON settings with backup recovery; native UTF-16 AOM settings import with non-secret review and offline apply | Imported settings replace the current ones, with unmentioned values at their defaults. Many fields are persisted without effect ([next section](#settings-persisted-without-runtime-effect)); see the [settings reference](Settings-Reference.md) |
| Add anime/episode/file/change request | Replaced | **Tools > Edit AniDB data** opens AniDB's maintained web editors | AOM's retired TCP `ADDANIMEDATA`/`ADDEP` route is not implemented |
| Command line / shell handoff | Desktop | Media paths (hashed at startup), `--database`/`-d`, `--full-update`, `--help`; legacy `/database`, `/fullupdate`; `/utf8` and `/ucs2` accepted and ignored | No shell-extension integration. Unknown `-` options are rejected |
| Status, progress, log | Desktop | Progress with pause/cancel, activity log, Queue page with Pending/N retries/Blocked, cooldown banner, "N commands NOT saved yet" | |
| Local cache recovery tooling | Not implemented | Settings backup recovery exists | No user-facing database backup/restore or repair workflow |
| Full-account MyList bootstrap | Not implemented | Entries arrive through legacy import, FILE replies for hashed local files, queued adds/edits | Local state is not equivalent to a complete account download; there is no reconciliation of the whole MyList |
| Scheduled folder scanning | Not implemented | Manual scan only | |
| Tray icon, colour-coded rows, saved columns | Not implemented | Settings fields exist | See below |

## Core-only capabilities

Available to code that links the crates, covered by tests where noted in the source, but with
**no desktop control**:

| Capability | Where |
|---|---|
| `Command::{MylistAdd, MylistDelete, MylistRefresh, FetchEpisode, FetchGroup}` and their executor arms | [`sync.rs`](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/src/sync.rs) |
| UDP `ANIME` wrapper, `PING`, the `new_version_available` flag | [`commands.rs`](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-anidb/src/commands.rs), [`udp.rs`](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-anidb/src/udp.rs) |
| Image download | `HttpClient::image` |
| ED2K-blue variant | `hash::Ed2kVariant::Blue` |
| Player `open`/`enqueue` actions | `external::PlayerAction` |
| `Db::is_empty`, `Db::clear_local_files` | [`db`](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/src/db/mod.rs), [`store.rs`](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/src/store.rs) |
| Command-line import example | `cargo run -p animatic-core --example import-legacy -- <kowai> <output.sqlite3>` |

Dependencies `trash` and `image` are declared by the application crate, but no source in the
repository calls them: there is no recycle-bin handling and no image decoding.

## Settings persisted without runtime effect

Nothing reads these fields; some have an Options checkbox that changes only the stored value.
A saved setting is not proof of behavior. The [settings reference](Settings-Reference.md) has
the complete per-field table.

- AniDB: `image_server`, `update_file_information` (checkbox), `idle_logout_minutes`.
- Hashing: `autostart` (checkbox), `priority`, `integrate_into_shell`, `ed2k_space_replacement`.
- File managers: `action_delay_ms`, `sleep_time_ms`; per folder `auto_rename` (checkbox),
  `clear_empty_folders`, `exclude_system`.
- UI: `colored_rows` ("Use status colors" checkbox) and `colors`, `columns`, `window`,
  `maximized`, tree expand/collapse, local folder size, root folder hiding, tray options.
  `Theme::System` applies no visuals of its own.

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

Equivalents of the original AOM workflows are tracked in [feature parity](Feature-Parity.md).
