# Animatic repository guidance

This file is the primary context handoff for coding agents. Read it before changing the repository.

## Project intent

Animatic is a clean-room, cross-platform Rust successor to AniDB O'Matic (AOM). It must preserve useful AOM library workflows while using maintained AniDB APIs and a modern native UI. Windows and Linux are equal supported targets.

The repository is self-contained. Run repository commands from the root of your local clone;
do not hardcode a developer's workstation or home-directory path.

Git remote `origin` uses SSH:

`git@github.com:RetroHazard/animatic.git`

## Privacy and provenance boundary

Never add any of the following to this repository:

- The original `AOM/` directory, executables, DLLs, resources, or bundled text files.
- AOM `kowai/*.data` databanks or decrypted/intermediate databases.
- The former `re/` reverse-engineering workspace or raw disassembly/extraction artifacts.
- Real settings, usernames, passwords, API keys, media paths, statistics, hashes, filenames, logs, or library records.
- Generated SQLite databases or exported XML/INI/statistics files.

Use synthetic fixtures with names such as `animatic-test-user` and paths under a clearly fictitious test root. `.gitignore` is a defense, not permission to create private artifacts inside the repository.
Construct binary records and encrypted test pages from synthetic values in code. Never embed record dumps, ciphertext, or header excerpts copied from a real databank, even when encoded as hex.

The migration implementation necessarily contains clean-room format knowledge and constants required to read a user-supplied databank. That code is part of Animatic; original AOM files are not.

## Workspace structure

- `crates/animatic-core`: storage, schema, library queries, hashing, migration, settings, rename language, file-manager policy, statistics, external tools, durable command model, and `update` (GitHub release check, bounded download, `SHA256SUMS` verification and `self-replace` install, with its own error type).
- `crates/animatic-anidb`: AniDB UDP and HTTP transports plus typed command/reply handling.
- `crates/animatic`: native eframe/egui application, background worker (including the dedicated updater thread), UI state, restart handoff and CLI entry point.
- `docs/README.md`: documentation index; user guides, complete settings reference, rename language, storage/network contracts, development and troubleshooting.
- `docs/architecture.md`: architecture, data flow, invariants, and extension points.
- `docs/feature-parity.md`: mapping from legacy workflows to Animatic equivalents.
- `docs/aom-forum-audit.md`: source-linked historical research, access limitations, baseline product gaps, and improvement verification.
- `docs/releases.md`: versioning, changelog, release tool, workflow contracts, maintainer procedure, archive contents, checksum verification and the in-app update contract (asset names, archive layout, limits).
- `.github/workflows/ci.yml`: CI for pull requests targeting `main`, `main` pushes and manual runs (no release or publish on any push): `plan`, then an always-run `quality` job that must pass before the planner-selected Rust/license jobs (locked `windows-latest`/`ubuntu-22.04` matrix and license gate, which then run in parallel), and the stable aggregate check `Required checks`; reusable (`workflow_call` inputs `ref`, `package`) and full when called. GitHub can still start the workflow for a stacked child pull request (base other than `main`) with `main` as the branch-filter context, so the trigger filter is only an optimization: a cheap `scope` job (no checkout) asks the REST API for the pull request's actual base and, when it is not `main`, defers every code and expensive job and reports a separately named *Stacked PR (deferred)* job, never `Required checks`; a `scope` API error fails closed. Source-head compilation happens only after `scope` allows, then `plan`, then a passing `quality`. Retargeting to `main` (a base-change `edited` event) runs the full checks. Title/body `edited` events do start the workflow but skip all code checks and report only a separately named metadata job, never `Required checks` (a skipped `Required checks` could mask an earlier failure on the same SHA). Any branch can be checked by a manual full run. Server-side branch protection is **not** configured (private repository, API returns 403); activation steps are in `docs/development.md`.
- `.github/workflows/release-prepare.yml` and `release.yml`: manual, `dry_run`-default release preparation (bot branch and PR) and tag-driven draft GitHub Release; see `docs/releases.md`.
- `scripts/release.py`, `scripts/ci.py` and their `test_*.py` tests: Python 3.11+ stdlib release tooling (`prepare`, `validate`, `notes`, `package`, `checksums`) and CI tooling (`plan` selection, `docs` Markdown validation, `gate` aggregate decision).
- `CHANGELOG.md`: curated `Unreleased` section plus generated per-release sections.

## Architectural rules

1. The UI thread owns egui state and must not perform hashing, network requests, migration, or long database work.
2. `worker.rs` runs background `Job`s and reports typed `Event`s over channels.
3. SQLite is the source of truth for library state and the persistent command queue. UI caches are reloadable projections.
4. User edits that require AniDB synchronization must survive restarts in `command_queue`.
5. Hash ED2K-red, CRC32, MD5, and SHA-1 in one file pass. Do not add redundant reads.
6. Legacy imports read source files only. Never rewrite or delete the selected AOM databank.
7. Preserve rename-language compatibility intentionally; document deliberate differences and add behavior tests.
8. Network code must retain AniDB flood spacing, authentication/session recovery, optional encryption, and offline behavior. GitHub update traffic is a separate transport (`animatic_core::update`) that never reuses AniDB spacing, cooldowns or error types.
9. Prefer current AniDB web editors over reviving retired metadata-change protocols.
10. Avoid platform-specific behavior unless isolated behind a cross-platform abstraction and verified on Windows and Linux.
11. Automatic local-record removal requires affirmative file absence beneath a reachable, explicitly configured manager root. Unavailable drives and filesystem errors must preserve library records.
12. AOM settings import runs on the worker, requires non-secret review, and applies offline only after successful persistence. Cancel/failure must preserve active settings; block other worker submissions until review ends.
13. MyList outbox IDs must never be reused. Save edits and optimistic state atomically, merge latest intent by entry, and guard stale response/ack effects by queue generation.
14. Malformed successful API replies are protocol errors, never authoritative absence. Preserve pending local intent when applying remote records.
15. Permanent failures remain blocked and visible; cooldowns survive queue runs and restarts. Explicit retry must not bypass an active cooldown.
16. Load library projections and export statistics off the UI thread. Coalesce refreshes and virtualize tables; stale snapshots must not override newer edits or queue status.
17. Settings load via `load_with_recovery`: read the backup only if the primary is missing/invalid; recovery is offline, shows a recovery window, skips autosave until an explicit Save, and modifies no source on load. Both missing means defaults; both bad/unreadable is an error exposing no JSON values. Only a valid object primary rotates the backup. Portable overrides need no default directory; `--help` creates no files.
18. Title dump import streams (compressed 32 MiB, decoded 128 MiB, 250k anime, 2M titles, 2048 bytes/name via `TitlesLimits`), validates XML and single-member gzip EOF/CRC, spools into private SQLite with no library lock during network, then merges atomically with the timestamp. No full-MyList bootstrap, resume, or external cryptographic checksum; cancel acts between commands (60 s timeout).
19. The built-in rename pattern uses a minimum of 2 episode digits, independent of count metadata; values >=100 and prefixes are preserved. Saved custom/old patterns are untouched. Compile/selected-file diagnostics are shown; warnings, empty or boolean bad targets block Apply. Invalid-filename policy is deliberately cross-platform: POSIX UTF-8 bytes vs Windows UTF-16 units, 255. Automatic rename stays unimplemented and never implicit.
20. Updates are explicit and user-confirmed: no unattended download or install, no privilege elevation, and no credentials, tokens or library data sent to GitHub; requests are anonymous (IP address and `Animatic/VERSION` user agent only) and there is no token or auth configuration. A 404 from the release feed is an explicit "feed unavailable" error, never "up to date" (a non-public repository looks like 404 to an anonymous client); do not claim the repository's visibility. Only a stable, published, strictly newer CalVer release is offered (never a prerelease, equal or lower version), found by the highest version across release pages rather than API order. Checking, downloading and installing run on the dedicated `Updater` thread (`UpdateJob`/`UpdateEvent`, its own cancel flag), never as normal worker `Job`s or `Started`/`Finished` activity; there is one operation at a time, dropping the updater cancels network work and waits for an active install, and install/restart stay disabled while activity is busy, an import is pending, commands are deferred or imported settings are unsaved. Concurrent installs from separate processes are serialized by an OS advisory lock on an empty `.animatic-update.lock` file beside the executable. Install replaces only the executable through `self-replace` (Windows keeps a `.animatic-recovery-*.exe` copy and restores best effort; no power-loss or rollback promise; dot-prefixed leftovers are never auto-deleted) and adds `LICENSE-vVERSION`/`THIRD-PARTY-NOTICES-vVERSION.html` beside it (never overwriting different files or symlinks); bundled README, changelog and docs stay old until a manual archive update. After an install attempt that entered the replacement, no retry or automatic restart. The executable identity (`InstallationTarget`) is captured at startup, and a restart spawns only after `run_native` returns, passing only the current `--database`. Never run `self-replace` against a test harness executable; checks are SHA-256 integrity only, so do not claim signing or authenticity. The AOM settings import must preserve `updates`.

## Commands

From the repository root:

```sh
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p animatic
```

Run the desktop application:

```sh
cargo run -p animatic
```

Portable test paths:

```sh
ANIMATIC_DB=/tmp/animatic.sqlite3 \
ANIMATIC_SETTINGS=/tmp/animatic-settings.json \
cargo run -p animatic
```

On PowerShell:

```powershell
$env:ANIMATIC_DB = "$env:TEMP\animatic.sqlite3"
$env:ANIMATIC_SETTINGS = "$env:TEMP\animatic-settings.json"
cargo run -p animatic
```

A migration smoke test requires a private databank outside the repository:

```sh
cargo run -p animatic-core --example import-legacy -- /outside/repo/kowai /tmp/animatic-import.sqlite3
```

Delete all smoke outputs afterward.

## Verification expectations

- Core/storage/network changes: targeted behavior test plus full workspace tests and Clippy.
- UI behavior: launch the native application and exercise the changed path; tests alone are insufficient.
- Migration changes: use synthetic unit fixtures, then—when privately available—run a smoke import outside the repository and check SQLite integrity.
- Cross-platform changes: keep the `Required checks` result green on both `windows-latest` and `ubuntu-22.04`.
- Release tooling, CI or workflow changes: run `python -m unittest discover -s scripts -p "test_*.py"`, `python scripts/release.py validate` and `python scripts/ci.py docs`, and validate workflows with **both** `actionlint` and a real `shellcheck` on `PATH` (without `shellcheck`, `actionlint` silently skips shell analysis and misses findings such as SC2129); never disable `shellcheck` or suppress a warning to pass; never publish, tag or push a release to verify them. Use synthetic fixtures only. CI `cargo-about` installs need `--features cli` (see `docs/development.md`). Do not claim CI passed until a run has been observed; the observed `quality` fix passed on GitHub run 42018000734 (job 127213424092) only, which is not full CI green or all-Rust success. Older or superseded migration runs may already be queued and must be cancelled when unnecessary.
- Updater changes: loopback servers and synthetic archives only; never contact GitHub or AniDB from tests, and keep the asset names, archive layout and `SHA256SUMS` format in lockstep with `scripts/release.py`. Do not claim Linux runtime or production GitHub verification.
- Never contact AniDB with fabricated or unregistered client credentials during automated tests.

## Current validated baseline

At relocation time:

- `cargo fmt --all -- --check` passed.
- `cargo test --workspace` passed: 48 tests.
- `cargo clippy --workspace --all-targets -- -D warnings` passed.
- `cargo build --release -p animatic` passed on Windows.
- A native startup smoke and a complete private migration smoke had passed before repository sanitization; no private inputs or outputs were copied here.

Treat this as historical evidence, not a substitute for rerunning checks after changes.

## Change hygiene

- Update `README.md`, this file, and `docs/architecture.md` when commands, boundaries, modules, or user-visible workflows change.
- Keep `docs/README.md` navigation and the settings/feature references aligned with implementation. Label desktop workflows, Rust-only APIs and stored-only compatibility fields separately; do not turn historical verification or forum reports into current capability claims.
- Add schema changes idempotently and preserve existing user databases.
- A text file embedded with `include_str!` must be listed in `.gitattributes` with `eol=lf`, so Windows (`core.autocrlf=true`) and Linux builds embed the same bytes.
- Do not commit `target/` or archives. Releases come only from the tag-driven release workflow as unsigned draft GitHub Releases (Windows ZIP, Linux tar.gz, `SHA256SUMS`); do not bump versions by hand, tag, push release branches or publish without an explicit maintainer instruction. Publishing a draft also makes it visible to the in-app updater. Versions are CalVer `YYYY.M.PATCH` (optional `-rc.N`, tag `vYYYY.M.PATCH`; the numbers are Cargo-compatible SemVer syntax only); use `python scripts/release.py prepare --version VERSION` for bumps and keep internal dependency pins, `Cargo.lock` and `CHANGELOG.md` consistent. A release version bump never changes the registered AniDB `client_version` or the database schema version. Archives must include `LICENSE` and the generated `THIRD-PARTY-NOTICES.html` (`about.toml`, `about.hbs`, pinned `cargo-about`); a dependency license outside the policy needs a deliberate maintainer review, not a policy edit to pass CI. Commit subjects become changelog text: keep them free of private data.
- Update `docs/releases.md`, `docs/development.md`, `docs/getting-started.md` and `README.md` together when release, packaging, distribution or in-app update behavior changes. Do not claim signing, installers, attestations, Linux runtime verification or configured GitHub settings that the repository does not demonstrate.
- Do not weaken `.gitignore` privacy exclusions without an explicit reason.
- Keep the repository URL and remote on `RetroHazard/animatic`; Git operations use the SSH remote.
