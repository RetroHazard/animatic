# AOM forum research and Animatic opportunity audit

Research date: 2026-11-05. Scope: the [AniDB O’Matic board (16)](https://anidb.net/forum/16), also reachable through the [original board URL](https://anidb.net/perl-bin/animedb.pl?show=threads&id=16).

## Coverage and evidence boundary

**This is a partial, source-linked audit, not a completed read of the forum history.** Direct reads of the original URL, canonical board, paginated thread index, alternate AniDB hostnames, and wiki returned HTTP 403. A real browser remained on Cloudflare’s security-verification page. Public search indexes supplied fragments of the board index; they did not expose the individual AOM thread IDs or post bodies. Do not mistake a topic title for a diagnosed defect, request specification, or resolution.

The indexed board reports 134 topics over five pages. Those pages and the release thread’s replies have **not** been exhaustively inspected. Search snapshots vary; reply/view counts are not stable evidence and are not used to prioritize work. The visible release-topic creation date does not establish a last release, abandonment date, or last support response. No credentials were used and no production AniDB API requests were made.

## Verified minimum and research leads

| Indexed topic / evidence | Date visible | Source | What it establishes / what remains unknown |
|---|---|---|---|
| AOM 0.5.18.276, by PetriW | Created 2015-11-25 12:18 | [Board index](https://anidb.net/forum/16), [thread-list view](https://anidb.net/forum/16/thread) | A release/support topic exists. Release notes, subsequent failures, fixes and individual reply dates remain unread. |
| Difference in stats | Created 2022-09-01 22:07; indexed last activity 2022-09-02 16:48 | [Board index](https://anidb.net/forum/16) | A post-2015 statistics-discrepancy topic exists. Which counts differ, expected behavior, cause and resolution are unknown. |
| Help - Handling Local Cache Fai… | Not exposed | [Board index](https://anidb.net/forum/16) | A truncated cache-help title is indexed. Full title, date, failure mode and outcome are unverified. |
| AOM still functional? | Not exposed | [Thread-list view](https://anidb.net/forum/16/thread) | A user asked about continued functionality. This does not establish that all or any AOM operations failed. |
| anidb`o matic downloading files | Not exposed; older contextual discussion | [Thread 10078](https://anidb.net/forum/thread/10078) | An indexed excerpt describes downloading MyList/anime/group/file databases. Board placement and date were not recovered; not treated as a post-abandonment board-16 bug. |

These links intentionally point to the board when the exact thread ID is unavailable. No thread IDs, quotes, failure causes, or request details have been reconstructed from guesses. Generic AniDB page hints about renaming and reports about **AniSub**, Shoko, or unrelated clients are excluded from AOM evidence.

## Baseline comparison and decisions

The implementation comparison below describes the initial baseline, **not** an assertion that inaccessible forum posts requested these exact changes. Existing coverage is not equivalent to proof against a historical failure.

| Need / research lead | Existing Animatic coverage | Gap or opportunity | Decision |
|---|---|---|---|
| Continued operation without retired AOM services | Maintained UDP/HTTP transports, registered-client configuration, offline durable queue; web links replace retired metadata-change protocols | Actual historical connection errors cannot be compared until posts are readable | Preserve offline workflows and flood/session handling; do not claim all old failures solved |
| Statistics discrepancy | `stats.rs` aggregates SQLite library records and renders XML/INI | No verified expected/actual historical figures; local inventory and AniDB MyList may represent different populations | Obtain thread details before changing counting semantics |
| Local cache failure / recovery | SQLite source of truth, reloadable UI projections, read-only transactional legacy import | No verified cache failure scenario; no user-facing backup/restore workflow | High-value research lead, not an implemented repair or evidence-backed diagnosis |
| Preserve settings during migration | UTF-16 AOM settings parser existed, but had no UI caller | Claimed migration workflow was unreachable | Add native settings-import selection, background parse, non-secret review, explicit Apply/Cancel, and offline application |
| Complete per-folder MyList policy | Settings/import/queue models already store storage/source/other flags and values | UI omitted these controls; checked empty strings were discarded by `manager_mylist_edit` | Expose controls; preserve explicit empty values so edits can clear fields |
| Preserve local records when storage is unavailable | Scan planner detects missing paths and supports automatic local-record removal | `!Path::exists()` conflated absence with filesystem errors; disconnected manager roots could purge local records | Require a reachable explicit manager root and affirmative absence before planning removal |
| Automatic managed renaming | Rename engine, manual preview/apply, persisted `auto_rename` option | Worker does not consume `auto_rename`; the visible setting is not proof of automation | Open product gap; design around metadata arrival, collision handling and cancellation before enabling |
| Truthful settings and discoverable refresh | Legacy imports retain hashing autostart/priority and file-information options; full update is available through CLI | Some settings have no runtime consumer; no equivalent full-update menu action | Open product gaps; do not represent persistence as implemented behavior |

## Implemented changes and acceptance

### Safe missing-file planning

Only an explicit matching manager with `auto_remove` enabled and an accessible directory root may schedule removal. A missing nested directory under an accessible root still permits a genuine missing-file action. Unavailable roots, default/empty roots, non-directory roots, and filesystem errors other than affirmative absence preserve records. This removes only local database records through the existing worker path, never media files or AniDB MyList entries.

### Complete folder MyList fields

Storage, source and other fields use independent apply checkboxes. Unchecked means leave unchanged; checked empty means explicitly clear. These values remain serializable in the durable command queue. Existing watched/state and per-folder selection rules remain intact.

### Reachable, reviewable settings migration

Settings import reads the selected AOM XML without modifying it, parses in the background, and requires an explicit review/apply step. Review does not expose usernames, passwords or API keys. XML with no supported settings is rejected. Worker submissions are disabled throughout parsing and review. Cancel/parse failure leaves active settings unchanged; a save failure retains both the active settings and review candidate. Apply persists the candidate before activating it, always offline, so users can review registered-client settings, credentials and folder policies before synchronization or scanning.

Implementation anchors:

- [`file_manager::plan_scan`](../crates/animatic-core/src/file_manager.rs): explicit path/recursion membership without default fallback, affirmative absence and trailing root-availability check; eight removal-boundary regressions.
- [`sync::manager_mylist_edit`](../crates/animatic-core/src/sync.rs): explicit empty fields; regression applies a serialized queued edit to an existing MyList record.
- [`settings::import_legacy_settings`](../crates/animatic-core/src/settings.rs): recognize consumed fields/collections; reject unrelated XML while accepting recognized default values.
- [`app.rs`](../crates/animatic/src/app.rs) and [`worker.rs`](../crates/animatic/src/worker.rs): native picker, typed background import, review gating, save-before-activate and folder controls.

## Exercised verification

Windows verification on the research date used synthetic media/settings and temporary
runtime databases outside the repository. No production AniDB API was contacted.

| Check | Observed result |
|---|---|
| Before/after runtime scan probe | Unavailable-root removal actions changed from **1 to 0**; genuinely missing file under available root remained **1** |
| Before/after default-membership probe | A reachable nonempty default root no longer authorized removal of an unrelated missing path; a matched missing path remained removable |
| Before/after checked-empty probe | Storage/source/other changed from omitted to explicit empty values |
| Before/after unrelated-XML probe | Unsupported XML changed from accepted to rejected |
| Targeted regression tests | File-manager, synchronization and settings suites passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo test --workspace` | **59 passed** |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| `cargo build --release -p animatic` | Passed |
| Native desktop exercise | Picker/review cancellation; non-secret review; save failure retains review; offline apply; unrelated/malformed UTF-16 XML rejection; queue action disabled during review and restored after cancel |
| Native folder editor | Checked empty storage, edited source, and unchecked preserved other persisted to settings |

The release executable was launched and exercised, not only compiled. Linux was not run
locally; the existing Windows/Ubuntu CI matrix remains the cross-platform verification
mechanism. Temporary probes and runtime fixtures were removed after verification.

## Remaining research prerequisite

To finish the requested historical inventory, supply readable board pages or an authorized HTML/text export **outside this repository**. It must include all five topic-index pages and complete topic pagination, especially the 2015 release discussion and later support threads. Redact credentials and personal media/library data. For each report, capture canonical thread/post URL, date, request or reproducible symptom, maintainer/user resolution, and Animatic implementation/test evidence. Deduplicate symptoms only after reading the replies; classify resolved legacy bugs separately from still-relevant requests.

Until that evidence is available, the forum inventory remains blocked and these shipped changes must be described as **code-audit improvements**, not a completed forum-driven backlog.
