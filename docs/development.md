# Development

This guide covers building, checking, testing and changing Animatic. It describes what the
repository does today; where something is absent (an installer, signing) it says so.
Repository-specific engineering rules for coding agents live in
[`AGENTS.md`](../AGENTS.md); design and invariants are in [Architecture](architecture.md) and
[Storage and network contracts](storage-and-network.md).

## At a glance

| Item | Current state | Source |
|---|---|---|
| Language / edition | Rust, edition 2024, Cargo resolver 3 | [`Cargo.toml`](../Cargo.toml) |
| Toolchain | Current stable Rust. There is no `rust-toolchain` file and no `rust-version` field; CI uses the `stable` channel with `clippy` and `rustfmt` | [`ci.yml`](../.github/workflows/ci.yml) |
| Workspace members | `animatic-core`, `animatic-anidb`, `animatic` | [`Cargo.toml`](../Cargo.toml) |
| Version | One version in `[workspace.package]`, inherited by all three crates; internal dependency pins and the `Cargo.lock` local entries move with it (see [Releases](releases.md)) | [`Cargo.toml`](../Cargo.toml) |
| Targets | Windows (`x86_64-pc-windows-msvc`) and Linux (`x86_64-unknown-linux-gnu`, CI on Ubuntu 22.04) | [`ci.yml`](../.github/workflows/ci.yml) |
| CI | Pull requests and `main` only: a planner selects the Rust and license jobs, an always-on quality job covers Python tests, docs and workflow validation, and a stable `Required checks` job gates the result; reusable by the release workflows. No server-side merge requirement is configured while the repository is private (see [Continuous integration](#continuous-integration)) | [`ci.yml`](../.github/workflows/ci.yml) |
| Distribution | Tag-driven draft GitHub Releases with an unsigned Windows ZIP, an unsigned Linux tar.gz and `SHA256SUMS`. No installer, package or signing. Installed copies can update themselves from a *published* release through the explicit in-app flow (`animatic-core::update`) | [Releases](releases.md) |
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

`ubuntu-22.04` runners already carry a C compiler and `pkg-config`; on a minimal system also
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
.github/workflows/            CI, release preparation and release workflows
scripts/                      release tooling (release.py) and its tests
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

`Cargo.lock` is tracked. CI builds, tests and lints with `--locked`, so commit lockfile
changes together with any dependency change or the run fails.

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
| Cross-platform code | Keep the `Required checks` result green for both `windows-latest` and `ubuntu-22.04`; isolate platform behavior behind an abstraction |
| Rename language | Behavior tests in `rename/tests.rs` and a note in [Rename language](rename-language.md) for any deliberate difference |
| Documentation only | Re-read the cited source; run `python scripts/ci.py docs` (Markdown links, anchors, fences, tables) and the format and link review below |

Before handing work off, run the Rust checks below in this order (the full CI set, including
the Python and documentation checks, is under [Reproducing CI locally](#reproducing-ci-locally))
and report what you actually ran:

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
   test must never be one click from the network. The startup update check
   (`updates.auto_check`, default on) also contacts GitHub on a fresh profile; seed the isolated
   settings file with `{ "updates": { "auto_check": false }, "anidb": { "offline": true } }`
   before the first launch (see
   [Avoiding the startup update check](getting-started.md#avoiding-the-startup-update-check)).
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

### Update tests and smoke rehearsals

The same rule applies to the updater: **no automated test contacts GitHub or AniDB.**

- `animatic-core::update` tests serve synthetic release JSON, `SHA256SUMS` and synthetic
  archives from a loopback listener (with proxies disabled) and use fabricated future
  versions and fictitious tags. Build archives in the test from explicit bytes; never embed a real
  binary, a real checksum or data copied from a real release.
- Cover at least: CalVer parsing and ordering, highest-version selection independent of API
  order, draft/prerelease/equal/lower skipped, checksum mismatch or missing/duplicate lines,
  traversal and unexpected archive entries, size limits, truncated downloads and cancellation.
- **Never run `self-replace` against the test harness executable** (it would replace the running
  test binary). Exercise the real swap only from a throwaway helper process that copies itself to a
  new temporary folder and replaces that copy.
- Worker and UI tests for the updater use the `Updater` (`UpdateJob`/`UpdateEvent`) path and must
  not touch the normal worker's `cancel` flag semantics or the AniDB cooldown.
- A manual install rehearsal uses a synthetic "new version" archive staged under a temporary
  folder and a copy of a built executable in a disposable directory; it never replaces the
  checkout's `target/` binary and never needs a real GitHub release. A Linux run is not verified by the
  project until someone records one.

#### Observed verification record (2026-11-05, Windows only)

Reported by the implementing session; historical evidence, not a substitute for rerunning checks:

- **Native Windows UI rehearsal, passed.** In a disposable workspace outside the repository, two
  actual release builds (`2026.11.0` and a synthetic `2099.1.0`) were used, with the old one served
  an update through a minimal **loopback HTTP fixture** and a small **external, throwaway patch of
  the update origin** (a test-only substitution for this rehearsal, not a relaxation of production
  TLS or host rules; the shipped code still requires HTTPS to GitHub). Observed: Help → Check for
  updates found the version; Download and verify matched the SHA-256; the first **Install** click did
  not modify the executable; **Confirm installation** replaced it with a file whose hash matched the
  new build; **Restart now** exited the old process with status 0 and the new process showed
  `2099.1.0` in About.
- **Data safety observed.** A custom `--database` path was kept, a hash-cache row survived, and
  synthetic queue/local rows that had been cleared before the install stayed empty after the restart
  (no media-file or `--full-update` replay). SQLite integrity check reported `ok`. The fixture
  served three anonymous requests (metadata, `SHA256SUMS`, the archive).
- **Not production.** No production AniDB request and no real GitHub release was involved or created.
- **Automated checks at that time:** the full workspace tests passed (208 Rust tests: 22 app, 26
  AniDB, 160 core), strict Clippy, rustdoc and the release build passed, and an earlier test that ran
  the real installer in a child process against a **release-profile (LTO, stripped) Windows build**
  passed. Counts are a dated fact; do not quote them as current.
- **Not exercised:** the Linux GUI and the Linux install path, and any update against real GitHub
  releases.

## Continuous integration

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) is the one CI workflow. It checks
**pull requests and `main` only**. Ordinary pull request, `main` and manual runs only check
the code and build raw executables (`cargo build --release` is still a compile); a release
workflow calling it with `package: true` additionally has it package archives. `ci.yml` itself
never creates a GitHub Release, tag or push, so it cannot cut a release from a pull request or a development branch. A plain push
to a development branch starts nothing; open a pull request (a draft is enough) or run the
workflow manually to get CI on it. How releases are made is in [Releases](releases.md).

- **Triggers:** `push` to `main`; every `pull_request`; `merge_group` (only matters if a merge
  queue is enabled); manual `workflow_dispatch`, which always runs the full checks; and
  `workflow_call` with inputs `ref` (string, default empty) and `package` (boolean, default
  `false`), used by the release workflows to check an exact commit. There is no workflow-level
  path filter: the workflow always starts and its final job reports, so the check can be required
  without a PR ever waiting for a check that never runs.
- **Concurrency:** a newer run for the same pull request cancels the older one. Every other run
  (`main`, merge group, manual, release-called) has its own group and always finishes.
- **Jobs** (the aggregate check is **`Required checks`**; the job ID is shown first and the
  name GitHub displays after it where it differs):

  | Job | Runs | What it does |
  |---|---|---|
  | `plan` (*Plan checks*) | always | Runs `scripts/ci.py plan` on the Git diff, outputs whether the Rust jobs and the license-policy job are needed, and writes the selection to the run summary |
  | `quality` (*Quality*) | **always**, including docs-only changes | Python 3.12: all `scripts/test_*.py` behavior tests, `release.py validate`, `scripts/ci.py docs` (Markdown links, anchors, fences and tables), pinned `actionlint` over the workflows, and `cargo fmt --check` when Rust is selected |
  | `notices` (*Dependency license notices*) | when the plan selects licenses | Installs the pinned `cargo-about` (`--features cli`), verifies `cargo about --version`, runs the license-policy gate and uploads `dependency-notices` |
  | `rust` (*Rust (windows)*, *Rust (linux)*) | when the plan selects Rust | Matrix: `windows-latest` (MSVC) and `ubuntu-22.04` (GNU), `fail-fast: false`; locked Clippy with warnings denied, tests, rustdoc with warnings denied, the release build and a `--help` smoke run; uploads the raw executable as `animatic-windows-x86_64` / `animatic-linux-x86_64` for 14 days |
  | `package` (*Package (windows)*, *Package (linux)*) | only when called with `package: true` | Needs `rust` and `notices`; wraps their executables and notices with `scripts/release.py package` and uploads `release-windows-x86_64` / `release-linux-x86_64` |
  | `required` (**Required checks**) | always | Fails closed unless every job the plan selected succeeded (see below) |

  `notices` and `rust` run in parallel and do not depend on each other, so one failure does not
  hide the other's diagnostics. Only `package` waits for both.
- **Planner (`scripts/ci.py plan`):** reads `git diff --no-renames --name-only` between the pull
  request base and the merge commit (for `main`, the pushed range) and prints
  `rust=true|false` and `licenses=false|true`. Documentation-only changes (Markdown and
  documentation assets) select neither; Rust sources under `crates/` select Rust; `Cargo.toml`,
  `Cargo.lock` or a crate manifest selects both; `about.toml`, `about.hbs` and `LICENSE` select
  the license job; `scripts/release.py`, `scripts/test_release.py`, anything under `.github/`,
  `.cargo/` or a `rust-toolchain*` file select both because they affect packaging or CI itself;
  other Python under `scripts/` selects only `quality`. **Any path it does not recognize selects
  both**, so a new kind of file fails safe. A reusable call with a non-empty `ref`, `package: true`,
  a manual run, a merge-group run and an unusable base commit (missing or all zeros) all
  run everything. An unreadable base fails the job; it is never treated as "nothing changed".
- **The gate (`scripts/ci.py gate`):** `quality` and `plan` must always succeed. `rust`,
  `notices` and `package` must succeed when selected and must be skipped when not selected. A
  failed, cancelled or unexpectedly skipped job, a missing plan or malformed data all fail
  `Required checks` with a reason; a skipped job is acceptable **only** when the plan did not
  select it.
- **Tests:** `scripts/test_ci.py` covers behavior only (path selection, Markdown validation,
  the gate's decisions); it does not assert on workflow text.
- **Pinned inputs and caching:** every third-party action is pinned to a full commit SHA.
  `actionlint` is downloaded from its GitHub release at a pinned version and verified against a
  pinned SHA-256, and `cargo-about` is pinned to an exact version (see the workflow). Ordinary
  runs may restore a cached `cargo-about` binary and Rust dependency cache, but only runs on
  `main` save them, and release (`package: true`) runs never restore either: they compile the
  renderer and the application from source. The workflow only reads repository contents.

### Third-party license gate

The `notices` job installs `cargo-about` with the `cli` feature and generates
`THIRD-PARTY-NOTICES.html` from [`about.toml`](../about.toml) and [`about.hbs`](../about.hbs) for
the union of the Windows and Linux dependencies. Generation fails on a license outside the policy,
and the job also fails on any `cargo-about` warning on standard error (for example a changed
pinned license file). A new dependency license needs a deliberate maintainer review (see
[Releases](releases.md#third-party-notices)); the generated notices are not a legal audit.

To run it locally, use the version pinned in `ci.yml` (shown here as `0.9.3`). The
`--features cli` flag is **required**: the crate's executable is built only with that feature, and without it
`cargo install` finishes but installs no `cargo-about` command.

```sh
cargo install --locked --version 0.9.3 --features cli cargo-about
cargo about --version
cargo about generate --locked --fail about.hbs --output-file "${TMPDIR:-/tmp}/THIRD-PARTY-NOTICES.html"
```

```powershell
cargo install --locked --version 0.9.3 --features cli cargo-about
cargo about --version
cargo about generate --locked --fail about.hbs --output-file "$env:TEMP\THIRD-PARTY-NOTICES.html"
```

Write the output outside the repository. `--fail` alone does not reproduce the whole gate:
`cargo-about` only warns about a changed pinned license file, which CI turns into a failure by
rejecting standard-error output, so read the command's warnings yourself.

### Reproducing CI locally

Install the prerequisites above and run, from a clean checkout of the same commit:

```sh
python -m unittest discover -s scripts -p "test_*.py"
python scripts/release.py validate
python scripts/ci.py docs
python scripts/ci.py plan --base BASE_SHA --head HEAD_SHA
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps
cargo build --locked --release -p animatic
```

`plan` only reports what CI would select. `actionlint` and `gate` need CI's inputs and are not
reproduced by hand. A green run means those commands passed on that runner for that commit; it
says nothing about runtime behavior beyond the `--help` smoke run. A CI artifact is a build
product, not a release.

### Known failure: `cargo about` "no such command"

The first CI run of this pipeline (run `41999768685`) failed this way, taken from its logs and
not reproduced since: `cargo install --locked --version 0.9.3 cargo-about` compiled the crate
but installed no executable, warning that the binary requires the `cli` feature; the next step,
`cargo about generate`, then failed with ``no such command: `about` ``. The fix is the
`--features cli` install plus a `cargo about --version` check before generating, as above. The strict
standard-error guard on `generate` is unchanged: stderr is never suppressed.

**Observed remediation (2026-11-05, Windows only).** The fix was exercised locally, not by
rerunning the old failure and not on GitHub Actions or Linux: `cargo install --locked --version 0.9.3
--features cli cargo-about` into a throwaway `--root` and target directory installed a real
`cargo-about.exe`; `cargo about --version` reported `cargo-about 0.9.3`; and `cargo about generate
--locked --fail` for the then-current workspace produced `THIRD-PARTY-NOTICES.html`, exited 0 and
wrote nothing to standard error. Treat it as a dated observation; the workflow's own run remains the
authoritative gate.

### Pull request gate and branch protection

Requiring these checks is a GitHub setting, not repository content. **The repository is currently
private, and GitHub answers the branch
protection and ruleset APIs with HTTP 403 (they need GitHub Pro, or a public repository).
No server-side enforcement is configured, so nothing currently prevents merging a pull request
with failing checks, or pushing directly to `main`.** `Required checks` is a signal today. The
maintainer plans to make the repository public after the development branches are collapsed
into `main`; this repository's changes never alter visibility.

Activate enforcement once the repository is public (or on a plan that offers it), as a
branch-protection rule or ruleset targeting `main`:

1. **Require a pull request before merging.** Required approvals are optional: a sole
   maintainer cannot approve their own pull request.
2. **Require status checks to pass** and add exactly **`Required checks`** (GitHub Actions). It
   is the only check to require; GitHub lists a check in the picker only after it has run on the
   repository. Do not add the individual `rust`, `notices` or `quality` jobs: they are skipped
   when unselected and a skipped required check is treated as passing, which is exactly what the
   aggregate encodes explicitly.
3. **Require branches to be up to date before merging** (strict): recommended, optional.
4. **Do not allow bypassing** (no bypass list, include administrators) and block force pushes
   and deletion of `main`: recommended.
5. **Signed commits:** optional. The bot's release-preparation commit is unsigned (see
   [Releases](releases.md#workflows)), so requiring signed commits can block that release pull
   request unless the maintainer re-signs the branch; [INFERENCE] a squash or merge commit made
   in the GitHub web UI is signed by GitHub, so check this on the first release pull request
   before relying on it.
6. **Merge queue:** optional. `ci.yml` already answers `merge_group`.

Pull requests opened by the release-preparation bot do not start `ci.yml`, because events
created with the workflow token do not trigger other workflows. If `Required checks` is required,
run CI on that branch manually (see [Releases](releases.md#maintainer-procedure)); the
manual run reports a check named `Required checks` on the branch commit, which satisfies the
requirement. The reusable run inside Prepare release is reported under a different, prefixed
name and does not.

Also available to the maintainer, and equally unconfigured here: a `release` environment and a
`v*` tag ruleset ([Releases](releases.md#repository-settings-the-maintainer-must-provide)).

## Release and distribution

Releases are produced by the `release-prepare.yml` and `release.yml` workflows and the
[`scripts/release.py`](../scripts/release.py) tool. The full contract, maintainer procedure,
required GitHub settings, archive layout and installation/verification instructions are in
[Releases](releases.md). In short:

- A maintainer runs **Release prepare** (dry run first) to bump the version in lockstep,
  update the changelog and open a pull request from `release/vVERSION`.
- After merging, the maintainer pushes the tag `vVERSION` explicitly (or runs **Release**
  manually from `main` for an existing tag); **Release** verifies it, reruns CI with packaging
  on that commit and creates a **draft** GitHub Release with a Windows ZIP, a Linux tar.gz and
  `SHA256SUMS`. Publishing the draft is a separate manual act.
- Archives are unsigned. There are no installers, packages or attestations. The app can update
  *itself* from a published release (the in-app updater; see
  [Releases](releases.md#in-app-updates)), so the archive names, the single root folder
  and the `SHA256SUMS` format are a compatibility contract with `update.rs`. A release run is
  not evidence of Linux desktop behavior or production AniDB validation.
- Nothing tags, bumps or publishes from a pull request or a development-branch push, and a
  release is never cut by CI itself; see [Continuous integration](#continuous-integration). The
  workflows do not claim any GitHub environment or branch-protection setting is configured; see
  [the required settings](releases.md#repository-settings-the-maintainer-must-provide).

Change release tooling together with [Releases](releases.md), [Getting started](getting-started.md),
the README and [`AGENTS.md`](../AGENTS.md). Keep commit subjects free of private text: they become
changelog entries.

## Extending the system

[Architecture](architecture.md#extending-the-system) contains the design-level checklists for a
new persistent field, AniDB command and background operation; do not duplicate them here.
This table maps a change to the places you will touch and the documentation to update.

| Change | Where | Also update |
|---|---|---|
| New or changed setting | `Settings` types in [`settings.rs`](../crates/animatic-core/src/settings.rs) (keep `#[serde(default)]` so old files load), `Default`, the Options window in `app.rs`, and the consumer. Add the legacy-import mapping only if AOM had the setting | [Settings reference](settings-reference.md) must say whether the field is consumed or stored only |
| New table, column or index | [`db/schema.sql`](../crates/animatic-core/src/db/schema.sql) (idempotent) or a migration in [`db/mod.rs`](../crates/animatic-core/src/db/mod.rs); store writes and library read models | [Storage and network contracts](storage-and-network.md), [Architecture](architecture.md) |
| New AniDB command | Request/reply in `animatic-anidb` `commands.rs`; a serializable `Command` with a priority and `describe()` in [`sync.rs`](../crates/animatic-core/src/sync.rs); execution and reply application in `core::sync::execute`; error classification in `error.rs` and `CommandError`. The retry, block and cooldown **policy** lives in the desktop worker (`fail_command` and `process_queue` in `worker.rs`), while core supplies the durable primitives (`queue_block`, `queue_deferred`, `set_sync_cooldown` and so on) | [Storage and network contracts](storage-and-network.md); [User guide](user-guide.md) if reachable from the UI |
| New background job | `Job` and `Event` in [`worker.rs`](../crates/animatic/src/worker.rs); define progress, cancellation and failure; keep blocking work off the UI thread. An operation that must not share the normal worker's `cancel` flag or queue (the updater) gets its own thread, cancel flag and typed events | [Architecture](architecture.md) |
| New page, menu item or dialog | `app.rs`; consume only projections and events | [User guide](user-guide.md) |
| Updater, archive layout or release asset names | [`update.rs`](../crates/animatic-core/src/update.rs), `scripts/release.py` and `release.yml` together | [Releases](releases.md#in-app-updates), [Storage and network](storage-and-network.md#github-update-transport), [User guide](user-guide.md#updating-animatic) |
| Rename function, object or property | `rename/` (`parser`, `eval`, `objects`, `plan`); tests in `rename/tests.rs` | [Rename language](rename-language.md); record deliberate differences from AOM |
| Public API of a library crate | Rust doc comments on the item | The generated `cargo doc` output is the API reference |
| Version, changelog, packaging, CI or workflow | [`scripts/release.py`](../scripts/release.py), [`scripts/ci.py`](../scripts/ci.py), their tests and `.github/workflows/` | [Releases](releases.md) and the README, Getting started and `AGENTS.md` claims about distribution or CI |

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
