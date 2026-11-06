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
| Batch/file manager | Partial | **Tools > Scan managed folders** applies per-folder rules: path, recursion, hidden-file exclusion, auto-hash, auto-rehash on changed modification time, auto-remove of *database records* for files affirmatively missing under a reachable root; plus per-folder MyList add/state/watched/storage/source/other policy and a rename base path | The scan is manual: no schedule, and the scan always filters by the hashing extension list. Discovery (hash/rehash) is **first rule wins in list order** among rules with a path and auto-hash or auto-rehash; longest-path matching applies only to the post-hash MyList policy, the rename base path and auto-remove matching. The scan itself never deletes or moves media files and sends nothing to AniDB, but each newly hashed file can queue an `Identify` that carries a MyList add when identify-after-hashing is on and the matching folder has MyList auto-add enabled; that queued add reaches AniDB when the queue is run. `auto_rename`, `clear_empty_folders`, `exclude_system` and the delay/sleep timers are stored only |
| Hashed files | Desktop | Known/unknown inventory with AniDB and MyList columns, CRC mismatch indication, Open (operating-system default handler), Play, AVDump, Rename, Identify after hashing | Identification is queued work: nothing is sent until the queue is run |
| Anime/mylist trees | Desktop | Searchable **Anime** and **My list** tables, a detail window with titles, episodes, genres/tags, relations (clickable) and links to AniDB | Tables, not trees. Reviews appear only if they came from a legacy import. No pictures or covers are shown |
| Advanced search | Desktop | Title (or exact id), year, type, MyList-only, watched-only filters over the local cache | Local data only |
| Groups and genres | Desktop | Searchable lists with library counts | |
| Add/edit MyList | Desktop | **Hashed files → Add to MyList…** queues adds for the selected files (identified files by file id, others by hash) with state, watched, view date, storage, source and other. **My list** edits State and Watched in the row and every field, including clearing a text field and the view date, in **Edit…**; **Remove from MyList…** (with confirmation) and **Refresh entry** queue a removal and a refresh. Edits are merged per entry, saved with the optimistic local change in one transaction and synchronized by the queue; the folder policy can still add files automatically (`Identify` with an add) | Adds start from hashed local files only: there is no add by AniDB id and no bulk edit of existing entries. A queued add shows as `Add queued` on Hashed files but creates no My list row until AniDB answers, because the entry id comes from AniDB. A removal deletes the local row only after AniDB acknowledges it and never deletes a media file. A FILE reply that explicitly reports "not in MyList" removes the matching *local* entry unless a command for it is queued |
| AniDB session | Desktop | UDP login, optional encrypted session (API key), compressed replies, flood spacing, session re-authentication, typed replies, offline mode, persisted cooldowns, blocked failures with explicit retry | The queue runs only on request. The client version is a settings-file/import value with no Options field. The UDP channel is not TLS ([Transport security](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#transport-security)). Keep-alive `PING` and idle logout (`idle_logout_minutes`) are not used |
| Full update | Partial | `--full-update` queues the title dump, a `FetchFile` for each file id and a `FetchAnime` for each anime id found among the *identified hashed local files* (idempotent); **Tools > Download anime titles**, **Fetch mylist statistics**, the anime detail **Refresh**, the per-episode **Refresh**, **Groups > Refresh** and **My list > Refresh entry** queue single items | It does not refresh every cached entity or your whole MyList. No menu action for a full update. The statistics result is stored and shown on Overview, apart from the local figures. No per-anime refresh throttle |
| Statistics export | Desktop | **File > Export statistics...** and optional automatic export, XML or INI, computed on the projection thread | The figures are read in one database snapshot and a failed query is an error, not zeros. The file is replaced atomically (temporary file, flush, rename); a failed write leaves the previous file intact. A hard crash can leave a hidden `.<name>.*.tmp` file |
| HTTP API and images | Partial | Anime XML and the public title dump are fetched through queued commands | Image download (`HttpClient::image`) and the image-server setting are **Core only / Stored only**; no picture is displayed |
| Rename files | Partial | Compatible expression language, user functions, database-backed `A`/`E`/`F`/`G`/`H`/`M` objects, live diagnostics, preview and explicit one-file apply ([Rename language](Rename-Language.md)) | Manual only; **automatic renaming is not implemented**. The file must be AniDB-identified with its episode and anime cached. Applying is a plain `fs::rename` with no cross-volume fallback or checksum ([guarantees](https://github.com/RetroHazard/animatic/blob/main/docs/architecture.md#what-the-code-guarantees-and-what-it-does-not)) |
| Hash export | Partial | **File > Export hashes...** writes one line per hashed file from a template | Only `%filename%`, `%filepath%`, `%ed2k%`, `%md5%`, `%sha1%`, `%crc32%`, `%size%` (short text) and `%fulled2k%` are real; the media tokens (`%fps%`, `%length%`, `%height%`, `%width%`, `%vcodec%`, `%vbitrate%`, `%acodec%`, `%asample%`, `%abitrate%`) are replaced with constants. The export is built and written on the projection thread (the UI thread only opens the file dialog) and replaces the target atomically |
| Players/AVDump | Partial | A configured default player launches with its *play* arguments; AVDump launches when enabled; both start the executable directly (no shell) | The player `open` and `enqueue` templates (`PlayerAction::Open`/`Enqueue`) are **Core only**: the Options window edits the executable and play arguments (the `name` control is never read), and the **Open** button uses the operating-system default handler instead. Templates are split into arguments first and placeholders substituted inside each argument, so a path, user name or key always stays one argument. Omitting `{path}`/`%file%` from a player `play` template appends the path as its own argument. Launched programs are not sandboxed |
| Settings | Partial | JSON settings with backup recovery; native UTF-16 AOM settings import with non-secret review and offline apply | Imported settings replace the current ones, with unmentioned values at their defaults. Many fields are persisted without effect ([next section](#settings-persisted-without-runtime-effect)); see the [settings reference](Settings-Reference.md) |
| Add anime/episode/file/change request | Replaced | **Tools > Edit AniDB data** opens AniDB's maintained web editors | AOM's retired TCP `ADDANIMEDATA`/`ADDEP` route is not implemented |
| Command line / shell handoff | Desktop | Media paths (hashed at startup), `--database`/`-d`, `--full-update`, `--help`; legacy `/database`, `/fullupdate`; `/utf8` and `/ucs2` accepted and ignored | No shell-extension integration. Unknown `-` options are rejected |
| Status, progress, log | Desktop | Progress with pause/cancel, activity log, Queue page with Pending/N retries/Blocked, cooldown banner, "N commands NOT saved yet" | |
| Local cache recovery tooling | Partial | **File > Back up library...** (online backup, atomic file), **File > Restore library...** (validated, confirmed, keeps a timestamped copy of the replaced library), **Tools > Check library** (`integrity_check` and `foreign_key_check`) and **Tools > Compact library...** (`VACUUM`), all in the background; settings backup recovery | No repair, no automatic or rolling backups, no scheduled checks; a backup holds the database only (not `settings.json`); restore and compaction wait for an idle worker |
| Full-account MyList bootstrap | Not implemented | Entries arrive through legacy import, FILE replies for hashed local files, queued adds/edits | Local state is not equivalent to a complete account download; there is no reconciliation of the whole MyList |
| Scheduled folder scanning | Not implemented | Manual scan only | |
| Tray icon, colour-coded rows, saved columns | Not implemented | Settings fields exist | See below |

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

Nothing reads these fields. Five have an Options control, shown disabled with a "Not active yet" hover text; the stored value is kept.
A saved setting is not proof of behavior. The [settings reference](Settings-Reference.md) has
the complete per-field table.

- AniDB: `image_server`, `update_file_information` (disabled checkbox), `idle_logout_minutes`.
- Hashing: `autostart` (disabled checkbox), `priority`, `integrate_into_shell`, `ed2k_space_replacement`.
- File managers: `action_delay_ms`, `sleep_time_ms`; per folder `auto_rename` (disabled checkbox),
  `clear_empty_folders`, `exclude_system`.
- UI: `colored_rows` ("Use status colors", disabled checkbox) and `colors`, `columns`, `window`,
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
  automatic renaming; background watchers; automatic or rolling database backups and database
  repair (backup, restore, check and compaction are manual menu actions).
- Adding a MyList entry by AniDB id or editing several entries at once; editing anime metadata
  (the Edit AniDB data menu opens web pages).
- Images and covers (the HTTP image function is unused; no image UI); system tray and
  minimize-to-tray; row coloring; saved columns, sorting and window state.
- A menu item for full update (use `--full-update`); queue or player *open/enqueue* actions;
  resumable downloads; shell integration.
- Encrypted storage of credentials; Linux runtime and production AniDB verification.
- Unattended or scheduled updates, update channels (prereleases), release signatures, rollback,
  package or installer integration; an update replaces only the executable and versioned license files.
