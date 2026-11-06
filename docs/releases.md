# Releases

This page describes how Animatic is versioned, prepared, packaged and published, and how to
install and verify a published archive. It documents the automation in the repository; it does
**not** claim that any GitHub setting, environment, branch-protection rule or published release
exists. Those are maintainer-side configuration, listed in
[Repository settings the maintainer must provide](#repository-settings-the-maintainer-must-provide).

Contents: [What a release is](#what-a-release-is) ·
[Install a release](#install-a-release) · [In-app updates](#in-app-updates) ·
[Versions and the changelog](#versions-and-the-changelog) ·
[Release tool](#release-tool) · [Workflows](#workflows) ·
[Repository settings](#repository-settings-the-maintainer-must-provide) ·
[Maintainer procedure](#maintainer-procedure) · [Archive contents](#archive-contents) ·
[Release notes and privacy](#release-notes-and-privacy) · [Failures and recovery](#failures-and-recovery) ·
[Limits](#limits)

## What a release is

A release is a Git tag `vVERSION` on a commit reachable from `origin/main`, plus a **published**
GitHub Release holding exactly three assets:

| Asset | Platform |
|---|---|
| `animatic-vVERSION-windows-x86_64.zip` | Windows, `x86_64-pc-windows-msvc` |
| `animatic-vVERSION-linux-x86_64.tar.gz` | Linux, `x86_64-unknown-linux-gnu`, built on Ubuntu 22.04 |
| `SHA256SUMS` | SHA-256 of the two archives |

A release is gated behind a reviewed merge **and** a tag at that merge. The only automatic path is
merging the `release`-labelled pull request that **Prepare release** opens: **Auto release** then
dispatches **Release** with the merge commit, which validates it, builds the archives, creates the
tag at that exact commit and publishes. Nobody pushes release tags by hand, and a tag push starts
nothing. No other pull request, development branch or `main` CI run can create a release; a manual
**Release** run from `main` can rehearse (dry run) or retry a commit that has no release yet (see
[Workflows](#workflows)). Publishing matters beyond the page itself: installed copies of Animatic
offer **published** stable releases (see [In-app updates](#in-app-updates)), so merging a release
pull request is what makes a stable version an update candidate.

What is deliberately **not** shipped: installers, packages (MSI, deb, rpm, Flatpak, AppImage),
unattended or automatic updates, code signing, notarization or build attestations. Archives are
unsigned; the only integrity aid is the `SHA256SUMS` file, which proves the download matches what
the workflow produced, not who produced it. The in-app updater is an explicit, user-confirmed
download of these same assets and adds no signing. Other architectures, distributions and Rust
targets are not built.

Each archive does include license notices: Animatic's own `LICENSE` and a generated
`THIRD-PARTY-NOTICES.html` listing the licenses of the Rust dependencies (including the fonts
embedded by the UI toolkit). These are license-compliance material, not code signing, and they
say nothing about the origin or integrity of the binary. They are generated automatically and
are **not a legal audit**; see [Third-party notices](#third-party-notices) for their limits.

## Install a release

Use a published release from the repository's GitHub Releases page. Download the archive for your
platform **and** `SHA256SUMS` into the same folder, verify, then unpack. The examples use
`2026.11.0` purely as an illustration; replace it with the version you downloaded. An
already-installed copy can instead update itself from a published release (see
[In-app updates](#in-app-updates)); a manual archive install is also how you refresh the
bundled guides and notices, which an in-app update leaves at their old version.

### Windows (PowerShell)

```powershell
$archive = 'animatic-v2026.11.0-windows-x86_64.zip'
$lines = @(Get-Content .\SHA256SUMS | Where-Object { $_ -match '^\S+\s+\*?(.+)$' -and $Matches[1] -ceq $archive })
if ($lines.Count -ne 1) { throw "expected exactly one SHA256SUMS entry for $archive" }
$expected = ($lines[0] -split '\s+')[0]
$actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash
if ($actual -ieq $expected) { 'checksum OK' } else { throw "checksum mismatch for $archive" }
Expand-Archive -LiteralPath $archive -DestinationPath .
.\animatic-v2026.11.0-windows-x86_64\animatic.exe
```

### Linux

```sh
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf animatic-v2026.11.0-linux-x86_64.tar.gz
./animatic-v2026.11.0-linux-x86_64/animatic
```

`sha256sum --check` must print `OK` for the archive you downloaded; do not unpack a file that
fails. The executable bit is set inside the tarball, so no `chmod` is needed after extraction.

The Linux binary is built on Ubuntu 22.04 against the GTK 3, xkbcommon, Wayland and OpenSSL
libraries installed by the CI job (see [Getting started](https://github.com/RetroHazard/animatic/wiki/Installation#option-b-build-from-source)
for the package names). **Runtime compatibility on other distributions or on a machine without
those libraries is not verified by the workflow**, and no Linux desktop launch of a released
binary is recorded by the project.

Start Animatic **offline** and follow [Getting started](https://github.com/RetroHazard/animatic/wiki/First-Launch#first-launch).
Archives contain no settings, databases, media or credentials; Animatic creates its own data at
first run.

Inside the extracted folder, links between the guides in `docs/` and to `README.md`,
`CHANGELOG.md` and `LICENSE` work offline. Links to source files, `Cargo.toml`, `crates/`,
`scripts/`, `.github/` or `AGENTS.md` are **not** shipped and only resolve in a source checkout;
links to GitHub pages need a network, and the Rust API documentation (`cargo doc` output) is
not included.

## In-app updates

An installed Animatic can check for, download, verify and install a newer release of itself.
The user-facing flow is in the [User guide](https://github.com/RetroHazard/animatic/wiki/Updating-Animatic); the transport,
limits and install contract are in
[Storage and network](storage-and-network.md#github-update-transport); the threading model is in
[Architecture](architecture.md#updater-thread). This section records what the **release process
must keep true** for it to work. Everything here describes the implemented code; none of it has
been run against a real GitHub release or on Linux by the project.

- **Only stable, published releases are offered.** The updater lists GitHub releases
  (anonymously; `RetroHazard/animatic` only) and ignores drafts, prereleases (including any tag
  with a `-` suffix such as `-rc.1`) and tags that are not `vYYYY.M.PATCH`. It takes the **highest
  CalVer** across all pages, not the first or "latest" entry, and offers it only if it is strictly
  newer than the running version: equal or lower versions are never offered and there is no
  downgrade. A release candidate is therefore *not* an update; testers install it manually.
- **Asset contract.** The published release must contain the platform archive
  `animatic-vVERSION-windows-x86_64.zip` or `animatic-vVERSION-linux-x86_64.tar.gz` and
  `SHA256SUMS` with the standard `sha256sum` format (64 lowercase hex digits, two spaces, the file
  name). The archive must have exactly one root folder (the archive stem) containing the
  executable (`animatic.exe` or `animatic`), `LICENSE`, `THIRD-PARTY-NOTICES.html`, `README.md`,
  `CHANGELOG.md` and `docs/*.md`, as built by `release.py package` (see
  [Archive contents](#archive-contents)). Renaming assets, changing the folder layout or the
  checksum format breaks updates for every already-installed copy, so change
  `scripts/release.py` and `animatic-core/src/update.rs` together and keep the previous layout readable
  for as long as older versions are in use.
- **What gets installed.** Only the executable is swapped, using the maintained `self-replace`
  crate. The new `LICENSE` and `THIRD-PARTY-NOTICES.html` are written beside it as
  `LICENSE-vVERSION` and `THIRD-PARTY-NOTICES-vVERSION.html` before the swap and are never
  overwritten if a different file exists. The old `LICENSE`, `THIRD-PARTY-NOTICES.html`,
  `README.md`, `CHANGELOG.md` and `docs/` stay as they were until the user replaces them from a new
  archive by hand (so the bundled guides can lag the executable), and the About window shows the
  versioned license files for the running version. Profiles, databases, settings and media are never touched.
- **Limits.** Metadata 1 MiB, `SHA256SUMS` 64 KiB, archive 256 MiB, extracted executable 512 MiB,
  each license file 8 MiB. A release whose archive would exceed them cannot be installed by the
  updater; raise the limits in `update.rs` and document them when the binary grows.
- **Trust.** Verification compares the archive's SHA-256 to `SHA256SUMS` from the same release.
  That catches corruption and mismatched files only; it is not a signature, and anyone able to edit
  a published release can replace both files. Do not describe it as authenticity.
- **Privacy and consent.** Checks are anonymous: they send the user's IP address and
  `Animatic/VERSION` to GitHub; there is no account, token or library data and no credential
  configuration. Nothing is installed without the user's confirmation. See
  [Settings reference → Updates](https://github.com/RetroHazard/animatic/wiki/Settings-Reference#updates) for `updates.auto_check`.
- **Publishing consequences.** Because installed copies see a release as soon as it is published,
  and merging a release pull request publishes it, a maintainer should rehearse the release (a dry
  **Release** run, see [Maintainer procedure](#maintainer-procedure)) and check the archive layout
  and `SHA256SUMS` *before* merging, and should treat unpublishing or deleting a published release
  as a user-visible event (an update that was offered can disappear before install). Release notes
  are not shown in the app; the updater links to the release page.
- **Supported targets only.** Windows and Linux `x86_64`. Other targets have no archives; the updater
  is disabled there.
- **Feed visibility.** The updater reads the release feed anonymously, so it only sees releases that
  GitHub serves to anyone. An HTTP 404 (also what an anonymous client sees for a repository or
  releases that are not publicly readable) is an explicit "release feed unavailable / not found" error,
  never "up to date". The feature works once the releases are publicly readable; this repository does
  not configure or prove its own visibility, and these docs make no claim about it. Until the feed is
  readable, update by manual archive download ([Install a release](#install-a-release)).
- **Linux.** The updater's Linux install path (an atomic rename of the executable, the executable bit
  kept from the archive) has not been exercised by the project on a real Linux desktop.

## Versions and the changelog

- **Version source:** one version in `[workspace.package]` in [`Cargo.toml`](../Cargo.toml),
  inherited by all three crates. The internal dependency requirements between the crates and
  the local package entries in `Cargo.lock` carry the same version and are updated together by
  `prepare`; the tool does not rewrite runtime code. A release version does **not** change the
  registered AniDB client version (`anidb.client_version`) or the database schema version; those
  are separate contracts that change only for their own reasons. Versions shown in this page
  (`2026.11.0`, `2026.11.1`, `2026.12.0-rc.1`) are illustrative examples; the repository's
  `Cargo.toml` is authoritative, and automated bumps make any number quoted in prose stale.
- **Scheme (CalVer):** `YYYY.M.PATCH`, for example `2026.11.0`. `YYYY` is the four-digit year
  (2000 to 9999), `M` the month `1`-`12` **without zero padding**, and `PATCH` a counter of
  releases within that month. Starting each month at `0` is the recommended convention, not a
  checked rule; the only mandatory rule is that each release is strictly greater than the last. A
  prerelease may append any Cargo-style prerelease identifier (`-rc.1`, `-beta.2`, `-alpha`);
  `-rc.N` is the recommended form, for example `2026.12.0-rc.1`. Build metadata (`+...`) is not
  accepted. Tags are the version with a leading `v` (`vYYYY.M.PATCH`). The version carries no
  compatibility promise; it says when the release was prepared.
- **Cargo compatibility:** Cargo requires a `MAJOR.MINOR.PATCH` version in SemVer *syntax*, so a
  CalVer number is written the same way (hence no leading zeros). That is a technical
  compatibility constraint only; Cargo's SemVer compatibility rules are not meaningful for these
  numbers.
- **Preparation rules:** `prepare` requires the year and month to equal the **current UTC** year
  and month (not the past, not the future). By convention the first release of a month uses patch
  `0`, later releases in the same month use a higher `PATCH`, and a new month resets it to `0`;
  the tool does not require patch `0` for the first release of a month. The version must
  also be strictly greater than the latest released version, with a prerelease ordering before its
  final release (`2026.12.0-rc.1` < `2026.12.0`). Only "strictly greater" is enforced, so a
  skipped patch number is allowed. Downgrades, repeats, and a version whose changelog section or
  tag already exists are rejected. The very first release may equal the version already in
  `Cargo.toml` when no released section or `vYYYY.M.PATCH` tag exists yet. Tags that are not valid
  CalVer (for example an old `v1.0.0`) are ignored when finding the latest release.
- **Validation is not clock-checked:** `validate` accepts any valid CalVer version, past or
  future; only `prepare` applies the current-month rule.
- **Changelog:** [`CHANGELOG.md`](../CHANGELOG.md) keeps a curated `Unreleased` section at the top.
  Preparation moves its entries into a new dated section for the version, adds entries derived
  from commit subjects since the previous release, and leaves a fresh empty `Unreleased`.
  Commits come from the history since the latest reachable `vYYYY.M.PATCH` tag (or the whole history
  for the first release); merge commits and release-only bookkeeping commits are excluded.
  Curated entries are preserved, not rewritten. `main` takes squash merges, whose subjects GitHub
  ends with `(#N)`; a curated entry that cites its pull request the same way, as `(#N)`, replaces
  the bullet that would otherwise be generated from that squash commit, so a change is not listed
  twice. A bare `#N` without parentheses does not. Commit subjects are escaped when written into
  the changelog so they cannot inject HTML or Markdown; curated entries are left as written.
  `prepare` refuses a shallow clone (use `git fetch --unshallow` first), because the commit
  range would be incomplete.

Commit subjects are public text. Write them as release-note material and never place private
paths, usernames, credentials, library names or AOM data in them (see
[Release notes and privacy](#release-notes-and-privacy)).

## Release tool

[`scripts/release.py`](../scripts/release.py) uses only the Python 3.11+ standard library. It
infers the repository root from its own location. Tests live in
[`scripts/test_release.py`](../scripts/test_release.py) and run with:

```sh
python -m unittest discover -s scripts -p "test_release.py"
```

The parser also accepts `--root PATH` *before* the subcommand, which the tests use to work on
synthetic fixture repositories.

| Command | Purpose |
|---|---|
| `python scripts/release.py prepare --version VERSION` | Validate the version and update `workspace.package.version`, the explicit internal dependency versions and the `Cargo.lock` entries of the local crates; write the dated changelog section and a new empty `Unreleased`. Internal dependency pins are rewritten only in the one-line inline form (`{ version = "…", path = "…" }`); any other table form is refused before anything is written. Performs no `git commit` or push and does not require a globally clean worktree, but it rewrites those files, so run it where you can discard the result. Refuses a duplicate section, an existing tag or a version that is not an increase, and fails before changing files where possible. Prints the changed paths. There is no `--dry-run` or date option; rehearse in a disposable worktree (below). |
| `python scripts/release.py validate [--tag vVERSION]` | Check that the workspace, crates, internal dependency requirements and lock file agree. With `--tag`, additionally require the tag to equal the workspace version and require a non-empty matching changelog section. Without a tag, an `Unreleased`-only repository passes. Exits non-zero on any error. |
| `python scripts/release.py notes --version VERSION --output PATH` | Write exactly that version's changelog section (non-empty, with no `Unreleased` content) as the GitHub release notes. The output path may be anywhere, including outside the repository. |
| `python scripts/release.py package --platform windows\|linux --binary PATH --notices PATH --output DIRECTORY` | Validate metadata, then build the archive from an explicit binary, the generated third-party notices file and tracked `docs/*.md` selected by `git ls-files`. `--notices` is required. Refuses missing, empty, directory or symlinked binary/notices inputs, missing tracked docs, and repository documents redirected outside the checkout. The explicitly selected binary and notices may live outside it. See [Archive contents](#archive-contents). |
| `python scripts/release.py checksums --directory DIRECTORY` | Require exactly the two archives of the current version (plus an optional existing `SHA256SUMS`), hash them by streaming and write `SHA256SUMS`. Missing, symlinked or unexpected files are errors. |

Do not rehearse `prepare` in a working tree that holds your own changes: it rewrites version
files and `CHANGELOG.md`, and undoing it with a broad `git restore` could discard unrelated
work. Rehearse in a disposable worktree or fresh clone **outside** the repository instead
(use a path that does not exist yet):

```sh
git worktree add --detach ../animatic-release-rehearsal HEAD   # then use the version you intend to release
cd ../animatic-release-rehearsal
python scripts/release.py prepare --version 2026.11.1
python scripts/release.py validate
git diff --stat
cd -
git worktree remove --force ../animatic-release-rehearsal
```

`--force` is needed because the rehearsal leaves uncommitted changes; the path is the throwaway
worktree, so nothing of yours is touched. A dry run of **Prepare release** in GitHub performs
the same rehearsal without touching your machine.

## Workflows

Four workflows live in [`.github/workflows/`](../.github/workflows/): **CI** (`ci.yml`),
**Prepare release** (`release-prepare.yml`), **Auto release** (`auto-release.yml`) and **Release**
(`release.yml`). All run on GitHub-hosted runners, every third-party action is pinned to a full
commit SHA, and none uses a personal access token, signing key or other custom secret. The default
token permission is `contents: read`; the exceptions are named below.

### `ci.yml` (CI)

Runs on pushes to `main`, pull requests targeting `main`, merge groups, manual dispatch and `workflow_call`.
Pull requests, merge groups and manual runs check the code (Clippy, tests, rustdoc); a `main`
push only refreshes the dependency caches that pull requests restore. `ci.yml` never creates a
GitHub Release, tag or push. The release workflows reuse it through
`workflow_call` with `ref` (string, the exact commit to check out) and `package` (boolean,
default `false`). A call with a non-empty `ref` runs every check, never the planner's reduced
selection; release preparation uses this for its bot branch. A call with `package: true`
(Release) runs no Clippy, tests or rustdoc, because the tag must be on `main` history that was
checked through pull requests: it builds clean, uncached release executables and notices,
smoke-runs `--help`, packages the versioned archives with `scripts/release.py package` and
uploads them as `release-windows-x86_64` or `release-linux-x86_64`. A manual CI run with
**executables** ticked uploads the raw executables (`animatic-windows-x86_64`,
`animatic-linux-x86_64`, 14 days), which are build outputs, not releases. The `notices` job
generates `THIRD-PARTY-NOTICES.html` (artifact `dependency-notices`) after installing the pinned
`cargo-about` with `--features cli`, and doubles as the license-policy gate. The `quality` job
must pass before the Rust and license jobs start, and those then run in parallel. The stable aggregate
job **`Required checks`** is the check to require on pull requests; it fails closed. Pull
requests whose base is not `main` (stacked children) may start the workflow, but a cheap `scope` job checks
the actual base through the REST API and defers all code and expensive jobs under a separately named
*Stacked PR (deferred)* job until retargeted to `main`; a `scope` API error fails closed. Metadata-only (title/body) edits start the workflow but skip all code checks under a
separately named job, never emitting `Required checks`.

The job-by-job contract, the planner and gate rules, the local reproduction commands (including
the `cargo about` install and the required `actionlint` plus `shellcheck` pair) and the history
of the `cargo-about` "no such command" and ShellCheck SC2129 failures live
in [Development → Continuous integration](development.md#continuous-integration) and are not
repeated here.

### `release-prepare.yml` (Prepare release)

Manual (`workflow_dispatch`) with inputs `version` (string, no `v` prefix) and `dry_run`
(boolean, default `true`). It refuses to run from any ref other than `main`.

1. Run the Python release-tool tests, then `prepare --version`, `validate --tag vVERSION`,
   `cargo metadata --locked` and `notes` for the new version. The generated notes are copied to the
   run summary.
2. Upload the preview as the artifact `release-preview-vVERSION` (14 days): `CHANGELOG.md`, the
   root and crate `Cargo.toml` files and `Cargo.lock`.
3. A **dry run** stops here. It does not push anything, and it does not run the full Windows and
   Linux CI matrix.
4. A real run commits those files on a new branch `release/vVERSION` as `github-actions[bot]`
   (unsigned) and pushes it with an empty expected lease
   (`--force-with-lease="refs/heads/release/vVERSION:"`), so only a **new** branch is created and
   an existing branch of that name is never moved; it then calls the reusable CI on exactly that
   commit SHA, and opens a pull request **only after that CI succeeds**. If the branch already
   exists, the run fails: delete the stale branch deliberately and re-run.

Because the branch is pushed with the workflow token, GitHub does not start the ordinary
pull-request workflows for the bot's pull request. The reusable CI run against the exact SHA
before the PR is opened is what proves the branch, but its jobs are reported under the caller's
prefixed names, so it does **not** provide the `Required checks` result that branch protection
would look for. If that check is required, a maintainer must run CI manually on the branch
(**Actions → CI → Run workflow**, or `gh workflow run ci.yml --ref release/vVERSION`); the
manual run reports `Required checks` on the branch commit. The workflow does not approve its own
pull request; a maintainer reviews and merges it.

Token permissions: `contents: write` only in the preparation job that pushes the branch,
`pull-requests: write` only in the job that opens the PR.

The pull request is opened with the label `release` (the label must exist in the repository). Its
description says that merging publishes the version; removing the label before merging merges
without releasing.

### `auto-release.yml` (Auto release)

Runs when a pull request into `main` is closed. Only a **merged** pull request whose head branch
starts with `release/v` and which carries the `release` label dispatches **Release** on `main` with
`sha` = the merge commit and `dry_run` = `false`. Its only permission is `actions: write`; it builds
nothing. A workflow-token dispatch is exempt from GitHub's rule that workflow-token events start no
other workflows, so no personal access token is involved. A merge queue still produces the
`closed`/merged event this needs.

### `release.yml` (Release)

Trigger: `workflow_dispatch` only (from **Auto release**, or manually), with inputs `sha` (default
the tip of `main`) and `dry_run` (boolean, default `true`; **Auto release** passes `false`).
It is refused unless it runs from `main` (`github.ref` must be
`refs/heads/main`). There is no tag-push trigger. Publishing runs never overlap and are never
cancelled (one `release-publish` concurrency group); each dry run has its own group, so a
rehearsal can neither delay nor replace a queued publication.

1. **Validate the commit:** unless `dry_run`, it must be reachable from `origin/main` (a dry run
   may rehearse the head of an unmerged `release/vVERSION` branch); the tag is `v` + the
   workspace version in that commit's `Cargo.toml`, and `release.py validate --tag` must pass (all
   pins and the lock file agree, and the changelog has a non-empty section for the version, which
   only a merged **Prepare release** pull request adds). An existing tag of that name is accepted
   only if it already points at this commit (a retry after a failed publication), and an existing
   GitHub Release for it is refused.
2. **Build:** call `ci.yml` with `package: true` on exactly that commit: clean, uncached release
   executables with a `--help` smoke run on both platforms, the license notices, and the
   `release-windows-x86_64` and `release-linux-x86_64` archives. Clippy, tests and rustdoc are not
   rerun: they ran on the release pull request, and the commit must be on `main`.
3. **Bundle:** download both archives, run `release.py checksums` and upload the artifact
   `release-bundle-vVERSION` (14 days): the two archives and `SHA256SUMS`. This happens **even for a
   dry run**, so you can inspect the exact assets.
4. **Publish:** only when `dry_run` is `false`. It runs in the GitHub environment named `release`
   and extracts the notes with `release.py notes`. It re-checks that the tag, if it now exists,
   still points at the validated commit, then runs `gh release create --target <commit>`, which
   creates the tag at that commit and publishes the release, marking versions with a prerelease
   suffix as prereleases. Only this job has `contents: write`. An existing release is never
   overwritten (the command fails). The tag is created with the workflow token, so it starts no
   other workflow.

No signing, attestations or installers are produced, and a successful run does not show that
the Linux binary starts on any particular machine beyond the CI `--help` smoke run on the Ubuntu
22.04 runner.

## Repository settings the maintainer must provide

The workflows need these; nothing here asserts they are already configured.

- **Actions → General → Workflow permissions:** enable *Allow GitHub Actions to create and approve
  pull requests*. It is required for the preparation workflow to open its PR (the workflow never
  approves it). No personal access token is needed.
- **A `release` environment** with required reviewers (optional). Only the publishing job uses it,
  so reviewers approve the tag and publication after the build and bundle finish; without it,
  merging the release pull request publishes with no further step. Also restrict its **deployment
  branches** to `main`, so no other ref can reach the publishing job. Reviewer approval and the
  deployment-ref restriction are configured on GitHub by a maintainer; this repository does not
  configure or prove them.
- **The `release` label** must exist (Issues → Labels); Prepare release applies it to its pull
  request, and Auto release reacts only to it.
- **Branch protection / rulesets for `main`:** activation (require a pull request, require the
  single status check **`Required checks`**, require up-to-date branches or a merge queue, no
  bypass) is spelled out in
  [Development → Pull request gate and branch protection](development.md#pull-request-gate-and-branch-protection).
  That page also covers signed commits: the preparation bot's commit is unsigned.
  Publishing still needs the merge of a `release`-labelled `release/vVERSION` pull request, or a
  maintainer's manual non-dry **Release** run from `main`; no other pull request or
  development-branch CI can publish.
- **Tags** are created only by the Release workflow, at the validated merge commit. There is no
  manual tagging step.
- **Tag protection (recommended):** a ruleset for `v*` tags that blocks updates and deletion keeps
  a published version from being moved or recreated. If it also restricts tag *creation*, it must
  let the Release workflow (the `github-actions` app) create tags, or publishing fails. No such
  ruleset is configured by this repository.
- **Bootstrap:** `ci.yml`, `release-prepare.yml`, `auto-release.yml` and `release.yml` must already
  exist on `main` before they can run, so merge the workflow changes through a pull request first.

## Maintainer procedure

All commands run from your local clone; Git uses the SSH remote.

1. **Rehearse the preparation** (changes nothing remote): in **Actions → Prepare release → Run
   workflow** on `main`, enter the version without `v` and keep `dry_run` checked. Read the notes
   in the run summary and download the `release-preview-vVERSION` artifact (changelog,
   `Cargo.toml` files and `Cargo.lock`). A dry run does not run the full Windows and Linux matrix.
2. **Prepare for real:** run it again with `dry_run` unchecked. After CI passes on the new SHA
   the bot opens a pull request from `release/vVERSION` with the `release` label.
3. **Review the release notes** in that pull request before merging
   ([privacy review is mandatory](#release-notes-and-privacy)): they are published verbatim. Edit
   the changelog on the branch if needed; run `python scripts/release.py validate` and the CI
   commands again after editing. Run `CI` manually on the branch (**Actions → CI → Run
   workflow**, branch `release/vVERSION`) so the pull request gets a `Required checks` result: the
   bot's pull request starts no CI by itself.
4. **Rehearse the release** (recommended, because merging publishes): run **Release** manually
   from `main` with `sha` = the head commit of `release/vVERSION` and `dry_run` checked. It
   validates that commit, builds both archives and uploads `release-bundle-vVERSION` without
   tagging or publishing. Download it, verify `SHA256SUMS` as in
   [Install a release](#install-a-release), and launch offline with isolated data (see
   [Development](development.md#running-with-isolated-data)) on the platforms you can.
5. **Merge** the pull request into `main`. Auto release dispatches Release with the merge commit.
6. **Approve** the publishing job in the `release` environment if reviewers are configured, wait
   for the run to finish, and check the published release: notes, both archives and
   `SHA256SUMS`. Record only what you exercised.

If the Release run fails after validation, fix the cause and run **Release** manually from `main`
with `sha` = the merge commit and `dry_run` unchecked. A tag left by a failed publication is
accepted only when it points at that same commit.

The first release may use the version already in `Cargo.toml` if its year and month match the
current UTC month and no tag or released changelog section exists. Run the preparation workflow
as above with that same version: it turns the curated `Unreleased` entries and the history into
the release section without changing the version numbers.

## Archive contents

Every archive has exactly one root directory, `animatic-vVERSION-PLATFORM-x86_64` (the in-app
updater requires this layout; see [In-app updates](#in-app-updates)):

```text
animatic-v2026.11.0-windows-x86_64/
  animatic.exe            (animatic on Linux, mode 0755)
  LICENSE
  THIRD-PARTY-NOTICES.html   generated by cargo-about
  README.md
  CHANGELOG.md
  docs/*.md               every tracked Markdown file in docs/
```

Entry order, timestamps and ownership metadata are normalized where the formats allow, so the
same inputs aim to produce the same archive. Documentation comes from `git ls-files` only: there
is no fallback to files outside Git, and ignored or untracked files are never included, so
packaging needs a Git checkout. Not included: `AGENTS.md`, databases, settings,
media, build sources, Cargo manifests, scripts, workflows, `target/` output or any other file.
The packager takes the binary and notices from explicit paths and copies only those inputs and
the allowlisted repository documents. Review those inputs for privacy; tests use synthetic data.

A new or upgraded dependency whose license is not already accepted in `about.toml`, or whose
license files no longer match the checksum-pinned clarifications, makes CI fail until a maintainer
reviews it and updates the policy deliberately; do not add a license to the list just to turn CI
green.

### Third-party notices

`about.toml` accepts a fixed list of licenses, pins the license files of crates whose packaging
`cargo-about` cannot identify on its own (including the embedded fonts and some Windows-support
crates) by checksum, and covers only the two release targets. Known limits:

- Some crates do not package their license text. For those the generator falls back to the
  canonical SPDX license text **without a crate-specific copyright line**; the report notes the
  fallback visibly and gives the upstream URL so a reader can find the original. For three
  crates (`egui`, `profiling`, `pulp`) whose upstream copyright lines are not packaged,
  `about.hbs` carries a statically verified supplement with the upstream source URLs and
  pinned versions. Re-verify that supplement when those crates are upgraded. Public author
  attribution (names and contact addresses from upstream licenses) is intentionally kept.
- Generation is automatic and reflects what the crates declare and package. It is not a legal
  review or a guarantee of exhaustive compliance. A maintainer should read the generated file
  when dependencies change, especially after upgrades, and treat any change to a
  checksum-pinned clarification as needing re-review.
- The notices describe Rust dependencies resolved by Cargo for the release targets; they do not
  cover system libraries the Linux binary loads at run time or any other software on the
  machine.

## Release notes and privacy

The GitHub notes are generated from commit subjects plus the curated `Unreleased` entries, so
their accuracy and safety depend on what was committed, and they are **published as soon as the
release pull request is merged**. **A maintainer must read the whole changelog section in the
release pull request before merging** and edit or remove anything that exposes private text:
personal paths, names, hashes or filenames from a library, credentials, AOM binaries or databank
content, or claims the release does not support (Linux runtime verification, signing, production
AniDB validation). Fix the changelog on the release branch, so the published notes and the
archived `CHANGELOG.md` are both clean. If private text already reached a published release,
treat it as a disclosure: pull the release and rewrite or remove the text before republishing.

## Failures and recovery

| Symptom | Cause and action |
|---|---|
| `prepare` rejects the version | Not `YYYY.M.PATCH` CalVer (with an optional Cargo-style prerelease such as `-rc.N`), has zero padding or build metadata, is not the current UTC year and month, is not an increase over the latest released version, or the section or tag exists. It also refuses a shallow clone: run `git fetch --unshallow`. Nothing is written. |
| Preparation CI fails on the new branch | No PR is opened. Fix `release/vVERSION`, run CI on the new commit, then open the PR manually; retrying the failed job still checks its original SHA. Alternatively, deliberately delete the failed branch and run preparation again. A new preparation run never moves an existing release branch. |
| Bot PR shows no checks | Expected: events created with the workflow token do not start `ci.yml` on the pull request. Run `CI` manually on the branch (see [`release-prepare.yml`](#release-prepareyml-prepare-release) above); if `Required checks` is required, that manual run is what satisfies it. |
| Release run rejects the commit | The commit is not reachable from `origin/main`, the workspace version does not match a non-empty changelog section (the commit was not prepared), the pins or lock file disagree, a manual dispatch was not run from `main`, or the tag already exists at a different commit. Merge a correct **Prepare release** pull request instead. |
| A release for the tag already exists | The workflow refuses to overwrite. Releases are published directly, so deleting one is a user-visible event; never delete a published release without a deliberate decision. Release a new version instead. |
| Merging the release pull request started no release | The pull request lacked the `release` label, its head branch did not start with `release/v`, or it was closed without merging. Run **Release** manually from `main` with `sha` = the merge commit and `dry_run` unchecked. |
| Checksum mismatch after download | Do not run the archive. Download again; report it if it persists. |
| Linux binary fails to start | Install the GTK 3, xkbcommon, Wayland and OpenSSL runtime libraries for your distribution. Not verified by the workflow. |
| Users report an update that was offered but fails to download or verify | Check that the published release has exactly the expected archive name for the platform and a `SHA256SUMS` line for it in the standard `sha256sum` format, and that the archive stays within the [updater limits](#in-app-updates). Fix by publishing a corrected newer version; do not silently replace assets of a published release |

## Limits

- Windows `x86_64` MSVC and Linux `x86_64` GNU (Ubuntu 22.04 build) only; unsigned; no installer. In-app updates are
  explicit, stable-published-only and unverified against production GitHub or Linux.
- A passing release run shows the build, tests, packaging and checksums succeeded. It is not
  production AniDB validation and not evidence of Linux desktop behavior.
- Rust API stability across versions is not promised (see
  [Storage and network contracts](storage-and-network.md#public-api-map)).
