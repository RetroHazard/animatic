# Animatic

Animatic is a native Rust successor to **AniDB O'Matic (AOM)** for Windows and Linux.
It manages local media and AniDB metadata, preserves offline synchronization intent,
and imports user-supplied AOM databanks without modifying the originals.

## What works

- Read-only AOM databank import and reviewable, offline-first settings migration.
- Single-pass ED2K-red, CRC32, MD5 and SHA-1 hashing; managed-folder scans (manual, at start or
  on an interval).
- Searchable library views, anime details, local-file identification and MyList
  watched/state editing.
- Durable synchronization queue with latest-intent edits, retained failures and
  restart-persistent cooldowns; AniDB's request spacing and once-a-day limits hold across
  runs and restarts.
- HTTP anime metadata and bounded, validated public-title downloads.
- Anime covers in the detail window: downloaded once on demand, kept on disk for your own
  display and never downloaded again (switch off with `anidb.download_covers`).
- A user-requested MyList export from AniDB and a bounded, validated, all-or-nothing import of
  the downloaded file.
- Compatible rename expressions with diagnostics, preview and explicit confirmation.
- Configurable players with Open, Play and Enqueue actions and a chooser, a best-effort system
  tray with minimize-to-tray, AVDump integration and XML/INI statistics exports.
- Safe settings-backup recovery that starts offline and preserves the original files.
- Library backup, validated restore, integrity check and compaction from the File and Tools
  menus.
- Explicit, user-confirmed updates from published GitHub releases (check, download and
  verify, install, restart); nothing is downloaded or installed unattended.

Some functionality exists only in the Rust APIs or as persisted compatibility settings.
The whole MyList is loaded through AniDB's MyList export (request it from the Tools menu,
download the file from AniDB's web page, import it from the File menu), never entry by entry.
There is **no automatic
renaming, or media-deletion UI**. See the
[feature-parity matrix](https://github.com/RetroHazard/animatic/wiki/Feature-Parity) and
[settings reference](https://github.com/RetroHazard/animatic/wiki/Settings-Reference) before relying on an imported option.

## Install or build

### Download a release

Published [GitHub Releases](https://github.com/RetroHazard/animatic/releases) carry an
unsigned `animatic-vVERSION-windows-x86_64.zip`, an unsigned
`animatic-vVERSION-linux-x86_64.tar.gz` and a `SHA256SUMS` file. Each archive contains the
executable plus `LICENSE`, `THIRD-PARTY-NOTICES.html`, `README.md`, `CHANGELOG.md` and the user
guide from the [wiki](https://github.com/RetroHazard/animatic/wiki) as `docs/*.md` for offline
reading (developer and Rust API documentation are not included). There is no installer or code signing.
Verify the checksum before unpacking; the Windows PowerShell and Linux `sha256sum` commands
are in [Releases](docs/releases.md#install-a-release). Linux runtime behavior has not been
exercised by the project, and the Linux build is made on Ubuntu 22.04.

### Update an installed release

On Windows and Linux x86_64, Animatic can update **its own executable** from the latest stable
published GitHub release. Updates are always explicit: **Help → Check for updates…** (or the
optional startup check) only looks for a newer version; you then press **Download and
verify**, confirm the installation, and choose **Restart now** or **Later**. Nothing is
downloaded or installed unattended, and there is no privilege elevation: the folder that
holds the executable must be writable. The archive is checked against the release's
`SHA256SUMS`, which detects a corrupted or mismatched download but is **not** a signature and
does not prove who produced it; releases are unsigned. Contacting GitHub exposes your IP
address and the Animatic version to GitHub; no account, token or library data is sent. The
startup check is controlled by **Check for updates at startup** (`updates.auto_check`, on by
default) and is skipped while *Work offline* is set or settings were recovered from a backup.
See [Updating Animatic](https://github.com/RetroHazard/animatic/wiki/Updating-Animatic) and
[Releases](docs/releases.md#in-app-updates).

### Build from source

Install current stable Rust. Windows builds require the MSVC C++ build tools and Windows
SDK. On Debian/Ubuntu, install the desktop dependencies:

```sh
sudo apt-get update
sudo apt-get install -y libgtk-3-dev libxkbcommon-dev libwayland-dev libssl-dev
```

```sh
cargo build --release -p animatic
cargo run -p animatic
```

The executable is `target/release/animatic.exe` on Windows or
`target/release/animatic` on Linux.

Start **offline**. Before synchronizing, configure your AniDB credentials and a registered
client identifier. Do not test production AniDB with invented client credentials.
The [Installation](https://github.com/RetroHazard/animatic/wiki/Installation),
[Running Animatic](https://github.com/RetroHazard/animatic/wiki/Running-Animatic),
[Data locations](https://github.com/RetroHazard/animatic/wiki/Data-Locations) and
[First launch](https://github.com/RetroHazard/animatic/wiki/First-Launch) wiki pages cover
downloads, the complete CLI, portable paths and the first run. A source-checkout or package-manager executable
can run the updater but should normally be updated through its own workflow; see
[Updating Animatic](https://github.com/RetroHazard/animatic/wiki/Updating-Animatic).

## Documentation

The user guide is the [Animatic wiki](https://github.com/RetroHazard/animatic/wiki), published
from [`wiki/`](wiki) in this repository:

- [Getting started](https://github.com/RetroHazard/animatic/wiki#get-started): installation,
  command line, data locations and the first launch.
- [Using Animatic](https://github.com/RetroHazard/animatic/wiki#use-animatic): pages, hashing,
  scans, MyList, renaming, exports, updating and migrating from AOM.
- [Settings reference](https://github.com/RetroHazard/animatic/wiki/Settings-Reference): defaults, JSON, UI exposure and
  which compatibility fields actually affect runtime behavior.
- [Troubleshooting and recovery](https://github.com/RetroHazard/animatic/wiki/Troubleshooting): backups, startup failures,
  queue/network problems, renaming and privacy-safe diagnostics.

Developer documentation stays in [`docs/`](docs/README.md): architecture, storage/network
contracts, development, [releases](docs/releases.md) (versioning, changelog, release
workflows, archive contents, checksum verification and the in-app update contract) and
historical research.

## Development and verification

```sh
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p animatic
cargo doc --workspace --no-deps
```

See [development](docs/development.md) for the verification matrix, CI (pull requests
targeting `main` are checked, with stacked children deferred by a runtime base check; `main`
pushes only refresh caches and releases only build; one `Required checks` result, enforced
only through GitHub settings outside this repository) and contribution workflow, and
[releases](docs/releases.md) for the maintainer release procedure.
[`AGENTS.md`](AGENTS.md) is the coding-agent handoff.
The [forum audit](docs/aom-forum-audit.md) records historical verification evidence and
research limits; it is not a claim of complete AOM parity or exhaustive forum coverage.

## Privacy and license

The AniDB password and API keys are kept in the operating-system keystore (Windows Credential
Manager, Linux Secret Service); when none is available they stay in the settings file as plain
text, which Options says. Settings backups can then contain plaintext credentials. Library databases, exports and
logs can expose viewing history and filesystem paths. Keep runtime data outside this
repository; use synthetic fixtures for development and redact support reports.
Original AOM binaries, databanks and extraction artifacts are not part of this project.
Update checks and downloads contact GitHub (`github.com`, `api.github.com`), not AniDB; see
[Storage and network](docs/storage-and-network.md#github-update-transport) for exactly what is sent.

MIT; see [LICENSE](LICENSE). AniDB and AniDB O'Matic are independent projects.

