# Development

This guide covers building, checking, testing and changing Animatic. It describes what the
repository does today; where something is absent (a release workflow, an installer, signing)
it says so. Repository-specific engineering rules for coding agents live in
[`AGENTS.md`](../AGENTS.md); design and invariants are in [Architecture](architecture.md) and
[Storage and network contracts](storage-and-network.md).

## At a glance

| Item | Current state | Source |
|---|---|---|
| Language / edition | Rust, edition 2024, Cargo resolver 3 | [`Cargo.toml`](../Cargo.toml) |
| Toolchain | Current stable Rust. There is no `rust-toolchain` file and no `rust-version` field; CI uses the `stable` channel with `clippy` and `rustfmt` | [`ci.yml`](../.github/workflows/ci.yml) |
| Workspace members | `animatic-core`, `animatic-anidb`, `animatic` | [`Cargo.toml`](../Cargo.toml) |
| Version | `1.0.0` for all three crates (shared through `[workspace.package]`) | [`Cargo.toml`](../Cargo.toml) |
| Targets | Windows and Linux (x86_64 builds in CI) | [`ci.yml`](../.github/workflows/ci.yml) |
| CI | Format, Clippy, tests and a release build on every push and pull request | [`ci.yml`](../.github/workflows/ci.yml) |
| Distribution | CI artifacts only. No release workflow, tags, installer, package or signature | see [Release and distribution status](#release-and-distribution-status) |
| License | MIT | [`LICENSE`](../LICENSE) |

Local runtime verification and CI coverage are different evidence. Linux runtime behavior and
production AniDB behavior have not been exercised by the project's recorded verification
(see the [README](../README.md) and the [forum audit](aom-forum-audit.md)); do not write
documentation or commit messages that claim otherwise.

## Prerequisites

### All platforms

- A current stable Rust toolchain with the `rustfmt` and `clippy` components
  (`rustup component add rustfmt clippy`).
- Git. The `origin` remote uses SSH (`git@github.com:RetroHazard/animatic.git`), so pushing
  needs an SSH key registered with your GitHub account. The `repository` field in `Cargo.toml`
  is the HTTPS URL of the same project. Keep both on `RetroHazard/animatic`.
- A C compiler. SQLite is compiled from source through `rusqlite`'s `bundled` feature, so no
  system SQLite is needed, but a C toolchain is.

Edition 2024 and the language features the source uses (including `let` chains) need a recent
stable compiler. The project does not pin a minimum version and CI only tests the newest
stable release, so if the build fails with an "unknown feature" or edition error, run
`rustup update stable` before investigating anything else.

### Windows (MSVC)

- Install Rust with the default `x86_64-pc-windows-msvc` host toolchain (the CI runner
  `windows-latest` builds this way; a GNU-host toolchain is not exercised).
- Install the Visual Studio Build Tools (or Visual Studio) with the **Desktop development
  with C++** workload so that `link.exe`, the MSVC C compiler and the Windows SDK are
  available. A typical symptom of their absence is a "linker `link.exe` not found" error.
- Run the commands below from PowerShell, Windows Terminal or a Developer prompt, in your
  local clone. No extra runtime libraries are required for the build.

### Linux

Install the native desktop build dependencies. On Debian or Ubuntu, CI uses:

```sh
sudo apt-get update
sudo apt-get install -y libgtk-3-dev libxkbcommon-dev libwayland-dev libssl-dev
```

`ubuntu-latest` runners already carry a C compiler and `pkg-config`; on a minimal system also
install them (for example `build-essential pkg-config`). Other distributions need the
equivalent development packages. A build failure that mentions a missing GTK, xkbcommon,
Wayland or OpenSSL library or a `pkg-config` lookup almost always means one of those
packages is absent.

Linux is a supported build target (CI compiles and tests it), but a native Linux run of the
desktop application has not been recorded; treat Linux UI behavior as unverified until you
have exercised it yourself.

## Workspace layout

```text
Cargo.toml                    workspace, shared package metadata, build profiles
crates/animatic-anidb/        AniDB UDP and HTTP transports, typed commands and errors
crates/animatic-core/         storage, schema, queue, hashing, import, rename, settings, ...
crates/animatic/              native egui/eframe app, worker thread, CLI entry point
docs/                         this documentation set
.github/workflows/ci.yml      the only workflow
```

Dependency direction: `animatic` depends on both libraries; `animatic-core` depends on
`animatic-anidb` (the command executor in `core::sync` drives the transport clients);
`animatic-anidb` depends on neither. The UI crate is the only one that links the GUI stack.
Module-level responsibilities are in [Architecture](architecture.md).

Build profiles ([`Cargo.toml`](../Cargo.toml)):

- `release`: `opt-level = 3`, thin LTO, 4 codegen units, symbols stripped.
- `dev`: `opt-level = 1`, with all dependencies at `opt-level = 3` (so hashing, SQLite and
  rendering are usable without a release build, at the cost of a slower first dependency
  build).

`Cargo.lock` is tracked. CI does not pass `--locked`, so commit lockfile changes together
with any dependency change.

## Everyday commands

Run everything from the repository root.

| Purpose | Command |
|---|---|
| Format check | `cargo fmt --all -- --check` |
| Apply formatting | `cargo fmt --all` |
| Lint (warnings are errors) | `cargo clippy --workspace --all-targets -- -D warnings` |
| All tests | `cargo test --workspace` |
| One crate | `cargo test -p animatic-core` |
| One test or module | `cargo test -p animatic-core settings::` or `cargo test -p animatic worker::` |
| Release build of the app | `cargo build --release -p animatic` |
| Run the app (debug profile) | `cargo run -p animatic` |
| API documentation | `cargo doc --workspace --no-deps` |

The release executable is `target/release/animatic` (`target\release\animatic.exe` on
Windows). Generated API pages start at `target/doc/animatic_core/index.html` and
`target/doc/animatic_anidb/index.html`. `target/` is ignored by Git and must never be
committed.

Tests are inline unit tests (`#[cfg(test)]` modules next to the code, plus
[`rename/tests.rs`](../crates/animatic-core/src/rename/tests.rs)); there is no `tests/`
directory. The app crate's worker tests start a local UDP fixture on `127.0.0.1`, so a
sandbox that blocks loopback sockets will fail them.

### Running with isolated data

Never develop against your real library. Point the application at throwaway paths:

```sh
ANIMATIC_DB=/tmp/animatic-dev.sqlite3 \
ANIMATIC_SETTINGS=/tmp/animatic-dev-settings.json \
cargo run -p animatic
```

```powershell
$env:ANIMATIC_DB = "$env:TEMP\animatic-dev.sqlite3"
$env:ANIMATIC_SETTINGS = "$env:TEMP\animatic-dev-settings.json"
cargo run -p animatic
# afterwards, so later launches in this terminal use the real locations:
Remove-Item Env:ANIMATIC_DB, Env:ANIMATIC_SETTINGS
```

`--database FILE` (also `-d` and the legacy `/database`) overrides `ANIMATIC_DB`; there is no
command-line option for the settings file. Portable overrides do not need the default
directory to exist, and `--help` prints one line and creates no files. Every launch in the
same environment uses the same overrides, so unset the variables when you want the default
locations again. The full option list and default per-user paths are in
[Getting started](getting-started.md).

Delete the temporary files (including the `-wal` and `-shm` companions of the database)
when you are finished.

### Migration smoke test

The `import-legacy` example imports a databank into a standalone SQLite file. It needs a
private databank that lives **outside** the repository:

```sh
cargo run -p animatic-core --example import-legacy -- /outside/repo/kowai /tmp/animatic-import.sqlite3
```

It writes decrypted intermediate files to a sibling directory named after the output
(`animatic-import.import-temp` here; `output.with_extension("import-temp")` in the example's
code) and removes it after a successful run. After a failed or interrupted run, plaintext
copies of private library data can remain there. The example does not require the directory
to be new, so inspect it, delete only the `<collection>.sqlite` files the importer created, and
remove the directory once it is empty; delete the output yourself. Only record the
observation ("imported N records from M collections"), never the content, in notes or
commits.

## Verification expectations

Which checks a change needs (from [`AGENTS.md`](../AGENTS.md)):

| Change | Minimum verification |
|---|---|
| Core, storage or network code | A targeted behavior test, then the full workspace tests and Clippy |
| UI behavior | Launch the native application and exercise the changed path; tests alone are insufficient |
| Migration | Synthetic unit fixtures; when a private databank is available, an out-of-tree smoke import and a SQLite integrity check |
| Cross-platform code | Keep CI green on both `windows-latest` and `ubuntu-latest`; isolate platform behavior behind an abstraction |
| Rename language | Behavior tests in `rename/tests.rs` and a note in [Rename language](rename-language.md) for any deliberate difference |
| Documentation only | Re-read the cited source; run the format and link review below |

Before handing work off, run the four CI commands in this order and report what you actually
ran:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p animatic
```

Test counts are historical facts about a past run. [`AGENTS.md`](../AGENTS.md) and the forum
audit record counts from earlier moments, and they disagree because they were captured at
different times. Never copy a count into documentation as "current"; quote one only if you
just ran the suite, and say when.

### Native UI verification

Automated tests cover the worker and core, not the egui surface. For a UI change:

1. Build or run with isolated paths (see above) and start from a clean temporary settings
   file.
2. **Tools → Options… → AniDB account:** tick **Work offline (keep commands queued)** and
   choose **Save options**. A fresh settings file has `offline` set to `false`, the
   built-in default folder rule adds identified files to MyList, and **Identify files with
   AniDB after hashing** is on by default, so hashing queues an Identify command. Nothing
   runs unless you choose **Tools → Run command queue** or **Synchronize now**, but a smoke
   test must never be one click from the network.
3. Create synthetic media under a fictitious test root. Contents need not be valid video,
   because hashing reads bytes:

   ```sh
   root="${TMPDIR:-/tmp}/animatic-test-root"
   mkdir -p "$root"
   head -c 4096 /dev/urandom > "$root/animatic-test-episode-01.mkv"
   ```

   ```powershell
   $root = Join-Path $env:TEMP 'animatic-test-root'
   New-Item -ItemType Directory -Force $root | Out-Null
   [IO.File]::WriteAllBytes("$root\animatic-test-episode-01.mkv", [byte[]](1..4096 | ForEach-Object { $_ % 251 }))
   ```

4. Exercise the changed path end to end (for example **File → Add files…**, the **Hashed
   files**, **Queue** and **Log** pages, **Tools → Scan managed folders**, the rename
   preview/confirm window, or **File → Import AOM settings…** with a hand-written synthetic
   XML file) and watch the status bar and the **Log** page for errors.
5. Close the application and reopen it against the same files to confirm persistence (queue
   rows, settings, library rows).
6. Write down exactly what you exercised and on which OS. Do not report an unexercised path
   as verified, and do not generalize a Windows observation to Linux.

Use the synthetic user name `animatic-test-user` and fictitious paths in anything you paste
into an issue, a commit or documentation.

## Test-writing principles

- **Build fixtures in code from explicit synthetic values.** Legacy parser tests construct
  records field by field; codec tests generate SQLite pages and encrypt them in the test. Never
  embed record dumps, ciphertext, header samples or hex copied from a real databank, even
  encoded. They are private artifacts, not format constants.
- **Use fictitious identities.** User name `animatic-test-user`, a client name that cannot
  be mistaken for a registered one, and paths under a clearly fictitious root (a temporary
  directory such as `<TEMP>/animatic-test-root`).
- **Isolate the filesystem.** Create per-test directories under `std::env::temp_dir()` named
  with the process id and a counter, and delete them in a `Drop` guard (the worker tests'
  `TempDb` and the settings tests' `TestDir` do this). Use `Db::memory()` when no file is
  needed. Remember SQLite's `-wal`/`-shm` companions when removing a database.
- **Generate hashes from generated bytes.** Compute expected ED2K/CRC32/MD5/SHA-1 values from
  data the test creates; do not paste hashes of real files.
- **Assert the consumer-visible result,** not just that code ran: the queue row, the stored
  record, the rendered text, the produced path.
- **Cover both a new and an existing database** for any schema or queue change, and keep
  schema changes idempotent (see `old_command_queue_is_rebuilt_idempotently_keeping_rows_and_ids`
  in [`db/mod.rs`](../crates/animatic-core/src/db/mod.rs)).

### Local API test isolation

No automated test may contact production AniDB, and none may use fabricated or unregistered
client credentials against it. The existing tests show the pattern:

- **UDP:** the worker tests bind a fixture `UdpSocket` to `127.0.0.1:0`, point
  `AnidbSettings::udp_server` at its address, set `local_port` to `0`, and reply with canned
  lines. `AnidbSettings::default()` points at `api.anidb.net:9000`, so a test that forgets
  the override targets the real server. Always set the server address and assert on the
  fixture's packet count.
- **HTTP:** `HttpClient::anime` and `HttpClient::titles_dump` use fixed production URLs
  (`HTTP_API`, `TITLES_DUMP`), and `process_queue` in the worker builds its clients from
  those. A worker-level test therefore must not queue `FetchAnime` or `TitlesDump`. HTTP
  behavior is tested inside `animatic-anidb` against a loopback `TcpListener` through the
  crate-private `titles_dump_from(url, …)`, with the proxy disabled (`.proxy(None)`, because
  the default agent can pick up a proxy from the environment), or against byte fixtures
  passed straight to the parsers.
- **No registered production client exists in this repository.** Do not invent one, and do not
  put real credentials, API keys or session keys in tests, fixtures, logs or documentation.
- Deliberate production contact is a manual, maintainer-run activity with that person's own
  account and registered client identifier; it is not part of any test or CI job, and nothing
  in this repository records it as done.

## Continuous integration

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml):

- **Triggers:** every `push` and every `pull_request`.
- **Matrix:** `windows-latest` and `ubuntu-latest`, `fail-fast: false`.
- **Setup:** `actions/checkout@v4`; `dtolnay/rust-toolchain@stable` with `clippy` and
  `rustfmt`; on Linux only, `sudo apt-get update && sudo apt-get install -y libgtk-3-dev
  libxkbcommon-dev libwayland-dev libssl-dev`.
- **Steps, in order:** `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`,
  `cargo build --release -p animatic`.
- **Artifacts** (`actions/upload-artifact@v4`, one per run and operating system):

  | Artifact name | Path in the job | Produced on |
  |---|---|---|
  | `animatic-windows-x86_64` | `target/release/animatic.exe` | `windows-latest` |
  | `animatic-linux-x86_64` | `target/release/animatic` | `ubuntu-latest` |

Each artifact contains only the single executable. A failing step stops the job before the
upload, so a downloadable artifact means all four commands passed on that runner for that
commit. That says nothing about runtime behavior on that platform.

To reproduce a CI failure locally, install the prerequisites above and run the same four
commands from a clean checkout of the same commit.

## Release and distribution status

There is currently **no release process** in the repository:

- no release or tag workflow, no Git tags, no changelog, no GitHub Release publishing;
- no installer, package (MSI, deb, rpm, Flatpak, AppImage and so on) or auto-update;
- no code signing, no notarization and no published checksums. The CI executables are
  unsigned and unverified beyond the CI steps above.

Binaries are CI artifacts or local `cargo build --release` output. Do not describe either as
a "release", "signed build" or "installer" in documentation. If a release process is added
later, update this section, [Getting started](getting-started.md) and the README in the same
change.

## Extending the system

[Architecture](architecture.md#extending-the-system) contains the design-level checklists for a
new persistent field, AniDB command and background operation; do not duplicate them here.
This table maps a change to the places you will touch and the documentation to update.

| Change | Where | Also update |
|---|---|---|
| New or changed setting | `Settings` types in [`settings.rs`](../crates/animatic-core/src/settings.rs) (keep `#[serde(default)]` so old files load), `Default`, the Options window in `app.rs`, and the consumer. Add the legacy-import mapping only if AOM had the setting | [Settings reference](settings-reference.md) must say whether the field is consumed or stored only |
| New table, column or index | [`db/schema.sql`](../crates/animatic-core/src/db/schema.sql) (idempotent) or a migration in [`db/mod.rs`](../crates/animatic-core/src/db/mod.rs); store writes and library read models | [Storage and network contracts](storage-and-network.md), [Architecture](architecture.md) |
| New AniDB command | Request/reply in `animatic-anidb` `commands.rs`; a serializable `Command` with a priority and `describe()` in [`sync.rs`](../crates/animatic-core/src/sync.rs); execution and reply application in `core::sync::execute`; error classification in `error.rs` and `CommandError`. The retry, block and cooldown **policy** lives in the desktop worker (`fail_command` and `process_queue` in `worker.rs`), while core supplies the durable primitives (`queue_block`, `queue_deferred`, `set_sync_cooldown` and so on) | [Storage and network contracts](storage-and-network.md); [User guide](user-guide.md) if reachable from the UI |
| New background job | `Job` and `Event` in [`worker.rs`](../crates/animatic/src/worker.rs); define progress, cancellation and failure; keep blocking work off the UI thread | [Architecture](architecture.md) |
| New page, menu item or dialog | `app.rs`; consume only projections and events | [User guide](user-guide.md) |
| Rename function, object or property | `rename/` (`parser`, `eval`, `objects`, `plan`); tests in `rename/tests.rs` | [Rename language](rename-language.md); record deliberate differences from AOM |
| Public API of a library crate | Rust doc comments on the item | The generated `cargo doc` output is the API reference |

Before changing an exported symbol, find its references (for example with rust-analyzer's
"find all references") and migrate every caller; this project does not keep compatibility
shims for superseded internal APIs.

A setting that is stored but not consumed, or a core capability with no desktop control, must
be documented as such. Do not let a new documentation page imply a feature exists because a
field or a function does. [Feature parity](feature-parity.md) is the place to record the
difference between a desktop workflow and a core-only capability.

## Privacy and provenance

Animatic is a clean-room successor to AniDB O'Matic (AOM). Never add any of the following to
the repository, in any form (including documentation examples, test fixtures and commit
messages):

- the original `AOM/` directory, executables, DLLs, resources or bundled text files;
- AOM `kowai/*.data` databanks, or decrypted or intermediate databases from them;
- the former `re/` reverse-engineering workspace, raw disassembly or extraction artifacts;
- real settings, user names, passwords, API keys, session keys, media paths, statistics,
  hashes, file names, logs or library records;
- generated SQLite databases or exported XML/INI/statistics files;
- real forum handles or private forum content (the [forum audit](aom-forum-audit.md) is a
  source-linked research record, not a place to add personal data).

The migration code necessarily contains clean-room format knowledge (constants needed to read
a user-supplied databank). That is part of Animatic; original AOM files are not. Do not
replace those constants with material copied from an installation.

Runtime data and where it lands:

- The default settings and database live in per-user directories outside the checkout (see
  [Getting started](getting-started.md)); the settings file holds the AniDB password, API key
  and AVDump key in plain text, and the database holds your viewing history and file paths.
  Statistics and hash exports are user-chosen files that reveal the same kind of information.
- The legacy import writes decrypted intermediates to an `import-temp` directory beside the
  database (or `<output>.import-temp` for the example).

[`.gitignore`](../.gitignore) is a defense, not permission to create private artifacts inside
the checkout. It excludes `/target/`, `settings.json`, `*.sqlite`, `*.sqlite3`,
`*.sqlite3-*`, `*.db`, `*.log`, `*.xml`, `*.ini`, `/AOM/`, `/re/`, `/kowai/`, `*.data`,
`local_settings.xml`, `animatic-hashes.txt` and `animatic-statistics.*`. It does **not**
match the settings backup and temporary names (`settings.json.bak`, `settings.json.tmp`,
`settings.json.bak.tmp`), so a settings directory placed inside the checkout would expose a
credential-bearing backup to `git add`. Do not weaken the existing exclusions without an
explicit reason, and keep runtime paths outside the working tree.

Before committing, review exactly what is staged:

```sh
git status --short
git diff --cached
```

and read the whole staged diff, not just file names: confirm there are no databases, exports,
settings files, logs, credentials, hashes or paths that reveal a personal directory layout,
including inside otherwise legitimate source or documentation files. If private material was
staged or committed, remove it from the index and history before pushing and treat any
credential in it as exposed.

## Documentation rules

- Update [`README.md`](../README.md), [`AGENTS.md`](../AGENTS.md) and
  [Architecture](architecture.md) whenever commands, boundaries, modules or user-visible
  workflows change. Update the matching page of this set (the table above) for settings,
  storage, rename, troubleshooting and parity changes.
- Document the implemented desktop behavior first, label core-only capabilities and stored
  but unused settings explicitly, and never turn a research note into a feature claim.
- Ground every claim in the source or in an observation you made, and say which. Mark an
  inference as an inference. Do not document behavior you have not read in the code.
- Keep one home for each topic: user tasks in the [User guide](user-guide.md),
  configuration in the [Settings reference](settings-reference.md), invariants in
  [Architecture](architecture.md) and [Storage and network contracts](storage-and-network.md),
  recovery and support in [Troubleshooting](troubleshooting.md). Link instead of copying
  procedures.
- Use relative links with exact file names, synthetic examples only, and no real credentials,
  paths or library data.
- Do not state new verification results unless you ran them. Historical counts stay
  historical.
- Review changed pages by re-reading the cited source and checking that every relative link
  resolves.

## Contribution checklist

- [ ] I read [`AGENTS.md`](../AGENTS.md), and the change respects its architectural rules.
- [ ] The change needs no private data; fixtures are synthetic and built in code.
- [ ] Tests assert behavior (and cover both a new and an existing database for schema or queue
      changes); no test contacts production AniDB.
- [ ] `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
      `cargo test --workspace` and `cargo build --release -p animatic` pass, and I report
      only what I ran.
- [ ] UI changes were exercised in the native application with isolated, offline settings.
- [ ] Platform-specific behavior is isolated, and nothing assumes a Windows-only or Linux-only
      path.
- [ ] Schema changes are idempotent and existing user databases keep working.
- [ ] Documentation affected by the change is updated (see the tables above).
- [ ] No runtime files, `target/`, exports or `.bak`/`.tmp` settings copies are staged.
- [ ] The Git remote and `repository` URL still point at `RetroHazard/animatic`.

## Build environment problems

| Symptom | Likely cause | Action |
|---|---|---|
| `linker 'link.exe' not found` (Windows) | MSVC Build Tools or the Windows SDK are missing | Install the C++ build tools workload, reopen the terminal |
| Compile errors about `edition2024`, an unstable feature or `let` chains | Toolchain is older than the source requires | `rustup update stable` |
| `pkg-config` or "library not found" for GTK, xkbcommon, Wayland or OpenSSL (Linux) | Development packages are missing | Install the packages listed under [Linux](#linux) |
| C compiler errors while building `libsqlite3-sys` | No usable C compiler | Install the platform C toolchain |
| Local tests fail to bind or send on `127.0.0.1` | The environment blocks loopback networking | Run the tests where loopback sockets are allowed |
| `cargo fmt --check` fails in CI but not locally | A different formatter version or unformatted files | `rustup update stable`, run `cargo fmt --all`, commit the result |
