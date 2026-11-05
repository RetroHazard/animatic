# Feature parity audit

This audit maps the reverse-engineered AOM behavior to Animatic. “Equivalent” means the workflow is available with a modern implementation; it does not require preserving Delphi control layout or bugs.

| Legacy area | Animatic equivalent |
|---|---|
| Startup databank loading | Transactional encrypted `kowai` import into SQLite, with cancellable progress |
| Hash queue | Background ED2K-red, CRC32, MD5, and SHA-1 hashing with pause/cancel and one disk pass |
| Batch/file manager | Recursive managed-folder scans, extension filters, safe remove-missing plans gated by root availability, and per-folder MyList watched/state/storage/source/other policy |
| Hashed files | Known/unknown local-file inventory, AniDB identification queue, local paths and hash metadata |
| Anime/mylist trees | Searchable Anime and My list tables, watched/state editing, details, episodes, genres, relations, reviews, and AniDB links |
| Advanced search | Title, year, type, mylist-only, and watched-only filters over the local cache |
| Groups and genres | Searchable group and genre views with library counts |
| Add/edit mylist | Atomic latest-intent edits, never-reused outbox generations, pending-intent overlays, strict reply parsing, and explicit synchronization |
| AniDB session | UDP authentication, optional encryption, compression, flood spacing, re-authentication, typed replies, offline mode, and persisted cooldowns |
| Full update/statistics | HTTP anime/title/image support, queued title and mylist-stat updates, XML/INI statistics export |
| Rename files | Compatible expression parser/evaluator, user functions, database-backed objects, preview, and safe apply |
| Hash export | Original sequential `%token%` expansion and configurable export template |
| Players/AVDump | Configurable external process launchers with parsed arguments |
| Settings | JSON settings plus native UTF-16 AOM settings import with non-secret review, Apply/Cancel and offline application; account, UI, hashing, managers, players, AVDump, exports, and rename configuration |
| Add anime/episode/file change requests | Opens AniDB’s maintained web editors; AOM’s retired TCP `ADDANIMEDATA`/`ADDEP` route is not used |
| Command line/shell handoff | Media paths, `--database`, `--full-update`, and legacy `/database`/`/fullupdate` arguments |
| Status/progress/log | Virtualized queue/activity views, retained blocked failures, explicit retry, cooldown status, job progress, pause, and cancel |

This table maps workflow areas, not complete behavior of every persisted option.
Automatic managed renaming and some imported hashing/refresh options still lack runtime
consumers. See the [forum/product opportunity audit](aom-forum-audit.md) for these gaps
and the limits of historical forum evidence.
Full-account MyList bootstrap/reconciliation is not implemented; local files and imported
entries are not equivalent to a complete account download. Library snapshots now load
off-thread, but are still whole-library projections rather than fully paged storage queries.

## Deliberate non-ports

The object inspector was a developer RTTI viewer. The header setup and anime-genre editor were unreachable resource-only/dead UI in AOM. The Add File dialog could not submit because its OK button was permanently disabled. Splash screens, MDI tab duplication, custom painted tree controls, exact colors, and Delphi-specific registry/COM shell-extension behavior are presentation or platform artifacts rather than library workflows.

Animatic uses native file pickers, platform application-data directories, external HTTPS links for current AniDB metadata editing, and a fixed modern table layout. These choices avoid reviving discontinued protocols and Windows-only COM dependencies.
