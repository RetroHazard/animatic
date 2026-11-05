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

The migration implementation necessarily contains clean-room format knowledge and constants required to read a user-supplied databank. That code is part of Animatic; original AOM files are not.

## Workspace structure

- `crates/animatic-core`: storage, schema, library queries, hashing, migration, settings, rename language, file-manager policy, statistics, external tools, and durable command model.
- `crates/animatic-anidb`: AniDB UDP and HTTP transports plus typed command/reply handling.
- `crates/animatic`: native eframe/egui application, background worker, UI state, and CLI entry point.
- `docs/architecture.md`: architecture, data flow, invariants, and extension points.
- `docs/feature-parity.md`: mapping from legacy workflows to Animatic equivalents.
- `.github/workflows/ci.yml`: Windows and Ubuntu formatting, lint, tests, release builds, and artifacts.

## Architectural rules

1. The UI thread owns egui state and must not perform hashing, network requests, migration, or long database work.
2. `worker.rs` runs background `Job`s and reports typed `Event`s over channels.
3. SQLite is the source of truth for library state and the persistent command queue. UI caches are reloadable projections.
4. User edits that require AniDB synchronization must survive restarts in `command_queue`.
5. Hash ED2K-red, CRC32, MD5, and SHA-1 in one file pass. Do not add redundant reads.
6. Legacy imports read source files only. Never rewrite or delete the selected AOM databank.
7. Preserve rename-language compatibility intentionally; document deliberate differences and add behavior tests.
8. Network code must retain AniDB flood spacing, authentication/session recovery, optional encryption, and offline behavior.
9. Prefer current AniDB web editors over reviving retired metadata-change protocols.
10. Avoid platform-specific behavior unless isolated behind a cross-platform abstraction and verified on Windows and Linux.
11. Automatic local-record removal requires affirmative file absence beneath a reachable, explicitly configured manager root. Unavailable drives and filesystem errors must preserve library records.

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
- Cross-platform changes: keep CI green on both `windows-latest` and `ubuntu-latest`.
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
- Add schema changes idempotently and preserve existing user databases.
- Do not commit `target/`; release artifacts come from CI.
- Do not weaken `.gitignore` privacy exclusions without an explicit reason.
- Keep the repository URL and remote on `RetroHazard/animatic`; Git operations use the SSH remote.
