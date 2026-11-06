Get an Animatic executable: a published release archive or a build from source. Then see
[Running Animatic](Running-Animatic.md) for the command line, [Data locations](Data-Locations.md)
for where data is kept and [First launch](First-Launch.md) for safe first-run settings.

## Status and limits

- Windows and Linux (x86-64) are the supported targets. Published releases carry an unsigned
  Windows ZIP and an unsigned Linux tar.gz (see [Option A](#option-a-download-a-release)); CI
  builds and tests both platforms.
- Native Windows runs were exercised during development. **Linux runtime behavior and
  production AniDB traffic have not been exercised** (see the
  [v2 verification record](https://github.com/RetroHazard/animatic/blob/main/docs/aom-forum-audit.md#v2-verification-observed-only)). Treat
  Linux desktop launch and any real AniDB exchange as unverified.
- There is no installer or code signing. Releases are published as GitHub
  Releases when a maintainer merges a reviewed release pull request; see
  [Releases](https://github.com/RetroHazard/animatic/blob/main/docs/releases.md). A CI build artifact is not a release.
- Animatic never contacts AniDB at startup. Network traffic to AniDB happens only when you start
  **Tools → Run command queue** or **Queue → Synchronize now** (see
  [Synchronization](AniDB-Synchronization.md#synchronization)). The default settings still have
  *Work offline* **off**, so the first thing to do in a fresh profile is turn it on
  ([first launch](First-Launch.md#first-launch)).
- Separately, a startup **update check** (`updates.auto_check`, on by default) asks GitHub for
  a newer stable release unless *Work offline* is on or the settings were recovered from a
  backup. It only looks for a version: nothing is downloaded or installed until you choose
  **Download and verify** and confirm the installation
  ([Updating Animatic](Updating-Animatic.md)). On a brand-new profile that means
  one request to GitHub (your IP address and the Animatic version) at the very first start;
  see [Portable and rehearsal setups](Data-Locations.md#portable-and-rehearsal-setups) to avoid it.

## Get an executable

### Option A: download a release

Each GitHub Release lists three assets (if the project has published one):

| Asset | Content | Platform |
|---|---|---|
| `animatic-vVERSION-windows-x86_64.zip` | `animatic.exe` plus `LICENSE`, `THIRD-PARTY-NOTICES.html`, `README.md`, `CHANGELOG.md` and this user guide as `docs/*.md` | Windows |
| `animatic-vVERSION-linux-x86_64.tar.gz` | `animatic` (executable) plus the same license and documentation files | Linux |
| `SHA256SUMS` | SHA-256 of both archives | both |

Download the archive for your platform and `SHA256SUMS`, verify the checksum, then unpack. The
commands for PowerShell and for Linux `sha256sum` are in
[Releases](https://github.com/RetroHazard/animatic/blob/main/docs/releases.md#install-a-release). Each archive has a single root folder named after
the archive; the Linux binary keeps its executable bit. The archives are **unsigned**. Links
between the bundled guide pages work offline; links to developer documentation and source files
point to GitHub, and the Rust API documentation is not included.

Once running, an installed release can replace its own executable with a newer published
release through **Help → Check for updates…** (Windows and Linux x86_64, writable folder
required, no elevation). The bundled `LICENSE`, `THIRD-PARTY-NOTICES.html`, README, changelog
and guides stay at the version you unpacked; the new version's licenses are added as
`LICENSE-vVERSION` and `THIRD-PARTY-NOTICES-vVERSION.html` beside the executable. Details and
limits: [Updating Animatic](Updating-Animatic.md). A checkout built from source can run
the updater, but it only replaces the compiled binary, never the source; use `git pull` and
rebuild, and update package-manager installations with that manager.

The Linux binary links against the desktop libraries listed below, so those must be present at
run time as well [INFERENCE: the CI job installs them for the build; the workflows do not
verify a run on a machine without them].

### Option B: build from source

Prerequisites:

| Item | Requirement | Source |
|---|---|---|
| Rust toolchain | Current stable. The workspace uses edition 2024 and `let` chains, which need a recent stable compiler. No `rust-toolchain` file or `rust-version` is pinned. | [`Cargo.toml`](https://github.com/RetroHazard/animatic/blob/main/Cargo.toml), CI uses `dtolnay/rust-toolchain@stable` |
| C toolchain/linker | Whatever Rust needs on your platform (MSVC build tools on Windows). SQLite is compiled in (`rusqlite` with `bundled`). | [`crates/animatic-core/Cargo.toml`](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/Cargo.toml) |
| Debian/Ubuntu libraries | `libgtk-3-dev libxkbcommon-dev libwayland-dev libssl-dev` | CI workflow, README |

Linux (Debian/Ubuntu):

```sh
sudo apt-get update
sudo apt-get install -y libgtk-3-dev libxkbcommon-dev libwayland-dev libssl-dev
cargo build --release -p animatic
```

Windows (PowerShell) and any other platform with the toolchain installed:

```powershell
cargo build --release -p animatic
```

Run these from the repository root. The executable is
`target/release/animatic` (`target\release\animatic.exe` on Windows). Release builds use
optimization level 3, thin LTO and stripped symbols. Other distributions need the
equivalent packages; they are not documented or tested here.
