# AOM forum research and Animatic opportunity audit

Research date: 2026-11-05. Scope: the [AniDB O’Matic board (16)](https://anidb.net/forum/16), also reachable through the [original board URL](https://anidb.net/perl-bin/animedb.pl?show=threads&id=16).

## Coverage and evidence boundary

**This is a partial, source-linked audit, not a completed read of the forum history.** Direct reads of the original URL, canonical board, paginated thread index, alternate AniDB hostnames, and wiki returned HTTP 403. A real browser remained on Cloudflare’s security-verification page. Public search indexes supplied fragments of the board index; they did not expose the individual AOM thread IDs or post bodies. Do not mistake a topic title for a diagnosed defect, request specification, or resolution.

The indexed board reports 134 topics over five pages. Those pages and the release thread’s replies have **not** been exhaustively inspected. Search snapshots vary; reply/view counts are not stable evidence and are not used to prioritize work. The visible release-topic creation date does not establish a last release, abandonment date, or last support response. No credentials were used and no production AniDB API requests were made.

### Follow-up: user-supplied Top 25

A subsequent user-supplied crawl summary supplied 25 ranked issues and concrete topic
links, including the [release discussion](https://anidb.net/forum/thread/55697),
[watched-state resets](https://anidb.net/forum/thread/44113),
[MyList connection failures](https://anidb.net/forum/thread/28672),
[connection bans](https://anidb.net/forum/thread/807720), and
[out-of-memory workarounds](https://anidb.net/forum/thread/816212).
The source file remains outside this repository. Direct thread reads still returned 403;
historical reports below are attributed to that supplied summary, not a claimed firsthand
read of all posts. Its ranking is qualitative and includes resolved historical defects.

## Verified minimum and research leads

| Indexed topic / evidence | Date visible | Source | What it establishes / what remains unknown |
|---|---|---|---|
| AOM 0.5.18.276, by PetriW | Created 2015-11-25 12:18 | [Release thread](https://anidb.net/forum/thread/55697), supplied in the follow-up summary | Release/support discussion; the initial search-only audit did not expose the post bodies. |
| Difference in stats | Created 2022-09-01 22:07; indexed last activity 2022-09-02 16:48 | [Board index](https://anidb.net/forum/16) | A post-2015 statistics-discrepancy topic exists. Which counts differ, expected behavior, cause and resolution are unknown. |
| Help - Handling Local Cache Fai… | Not exposed | [Board index](https://anidb.net/forum/16) | A truncated cache-help title is indexed. Full title, date, failure mode and outcome are unverified. |
| AOM still functional? | Not exposed | [Thread 805321](https://anidb.net/forum/thread/805321), supplied in the follow-up summary | A user asked about continued functionality; detailed answers are attributed to the supplied summary, not independently fetched posts. |
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

*Update 2026-11-06 (historical rows above are unchanged):* automatic managed renaming is now an opt-in, per-managed-folder setting (Off, Dry run, On; off by default) that runs only through the verified rename worker job and skips and logs collisions; see AGENTS.md rule 19 and the [wiki](https://github.com/RetroHazard/animatic/blob/main/wiki/Renaming-Files.md).

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

## Initial implementation verification

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

## First reliability tranche from the Top 25

The selected work addresses trust failures before adding destructive automation or
more feature breadth. Historical symptoms motivated the priorities; Animatic defects
were separately identified in code and reproduced with synthetic inputs.

| Summary item | Implemented response | Deliberate limit |
|---|---|---|
| 17: MyList empty/out of sync | Strict success-reply parsing, no malformed-as-absence deletion, transactional FILE/MyList application, guarded stale destructive replies | Full-account MyList bootstrap/reconciliation is still a separate gap |
| 18: watched status resets | Atomic latest-intent edit merging, never-reused generations, pending-intent overlays, deletion/retry ordering | No claim that every historical AOM cause was identical |
| 21: bans/connections | Permanent failures retained as blocked work, durable cooldowns, bounded counted failures, connectivity distinguished from payload errors | No universal ten-minute ban-expiry assumption; real AniDB was not contacted |
| 7 and large-library concerns | Separate background projection/export thread, coalesced generations, cached search indices, virtualized tables and horizontal scrolling | Snapshots still materialize the library; this is not fully paged storage |

### Exercised reliability verification

- Before/after external SQLite/localhost-UDP probes: latest watched intent survived an
  older acknowledgement, and a truncated successful MyList reply changed from accepted
  absence to a protocol error.
- The actual worker changed from repeatedly sending a permanent failure to one attempt,
  then a retained blocked entry and normal completion.
- Local UDP regression fixtures exercised banned, unavailable, rejected-credential and
  permanent-error paths, including fresh-client cooldown enforcement.
- Native release exercise with 20,000 anime, MyList entries, local files and queued
  commands, plus 1,000 groups and genres. All table pages were opened. MyList instantiated
  31 visible checkbox rows rather than all 20,000.
- Native cached search, rapid watched/unwatched/watched edits, hidden-selection clearing,
  blocked/cooldown display, retry without cooldown bypass, and restart persistence passed.
  A narrow-window capture showed the horizontal-scroll layout.
- An accepted explicit statistics export completed with the expected full content when
  the worker was immediately dropped for graceful shutdown.
- Windows: 114 workspace tests, formatting check, Clippy with warnings denied, and release
  build passed. Linux runtime and production AniDB synchronization were not exercised in this
  audit; both were later verified for roadmap item M1.7 (see the Feature Parity page's
  verification status).

All runtime inputs and probes were synthetic and outside the repository. No private
databank, personal library, credentials, raw forum export, or workstation path was added.

## Backlog v2 priority mapping

Source: the user-supplied backlog_v2 (authoritative for priorities; its external file, paths and forum author handles are deliberately not reproduced). This audit remains partial, not a full crawl. Issue IDs below refer to that backlog; links are the existing ones above.

| Backlog IDs | Status |
|---|---|
| A3, A4, A16 | Backup recovery and portable startup diagnostics implemented; historical native crashes, cloud-sync conflicts and Wine behavior were not reproduced |
| A1, A2, F3, F4 | Improved for public titles only (streamed, validated title dump); legacy full cache, all-account dump, and resume remain unimplemented |
| A8, F12 | Addressed: specific rename padding and validation cases |
| A13, A15 | Covered by the earlier reliability tranche |
| A14 | Safety constraint only; no new delete workflow |
| A9, F6 | Automatic renaming remains unimplemented and must never be enabled implicitly |

*Update 2026-11-06:* the A9/F6 constraint now reads "opt-in per managed folder, off by default, never implicit" (AGENTS.md rule 19); a stored or AOM-imported `auto_rename` flag that was `true` loads as dry run, never on.

Corrections to v1: the XP SP3 anecdote concerns out-of-memory, not startup; `False.mkv` and blank-name pattern errors are correct rejections, not successes or defects; the 510/ten-minute anecdote is not a retry policy.

### V2 verification (observed only)

- External Rust probes, failing before and passing after: a corrupt primary settings file with a valid backup now recovers offline with all bytes unchanged; the default rename filename no longer changes when the count metadata goes 1 to 100; an XML document with an unrelated title is rejected.
- A further probe streamed 100,000 synthetic titles from gzip through the real `read_titles_dump` and `Db::import_title_dump`. Bad CRC, truncation and late-invalid XML left library rows and the completion timestamp intact; a second zero-busy-timeout connection wrote during the read closure.
- Review regressions reproduced before and passed after: duplicate attributes, a late XML declaration and a malformed comment now reject the document. BOM acceptance and fragmented input remain covered by the parser behavior checks.
- Windows: **151 workspace tests**, formatting check, Clippy with warnings denied and release build passed.
- Native application: recovery notice and forced offline state; closing without Save preserved both settings files byte-for-byte. A first-run-card dismissal that previously overwrote the recovered primary now also preserves both files. Explicit Save preserved the good backup and persisted the recovered offline settings.
- Native rename workflow: a saved Unicode custom pattern survived recovery; stable-default reset/restore and live syntax diagnostics worked. An empty-result preview disabled apply; selected-file validation followed by explicit confirmation renamed a synthetic Unicode file and updated its durable library path.
- Windows superscript device names (`COM¹`–`COM³`, `LPT¹`–`LPT³`, including extensions) were accepted before review fixes and rejected afterwards.
- Runtime fixtures and throwaway probes were outside the repository and removed after verification. No real AniDB request was made; Linux runtime was not exercised.

## Remaining research prerequisite

The supplied summary resolves the earlier lack of topic IDs for the selected reports and
is sufficient to prioritize this tranche. An exhaustive historical inventory would still
need complete post/page coverage, dates, and resolutions; the summary describes a sample.
Keep any future authorized exports outside the repository and redact private data.
Do not present this tranche as complete AOM parity or as a full read of the forum.
