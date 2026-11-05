# Getting started

This guide takes you from nothing to a running Animatic window with safe settings. It
covers obtaining an executable, prerequisites, the command line, where data is stored,
portable and rehearsal setups, and the first launch. Day-to-day use is in the
[user guide](user-guide.md); every stored setting is in the
[settings reference](settings-reference.md); problems are in
[troubleshooting](troubleshooting.md).

Contents: [Status and limits](#status-and-limits) ·
[Get an executable](#get-an-executable) · [Run it](#run-it) ·
[Command-line reference](#command-line-reference) · [Where data lives](#where-data-lives) ·
[Portable and rehearsal setups](#portable-and-rehearsal-setups) ·
[First launch](#first-launch) · [AniDB client registration](#anidb-client-registration) ·
[Next steps](#next-steps)

## Status and limits

- Windows and Linux (x86-64) are the supported targets. Published releases carry an unsigned
  Windows ZIP and an unsigned Linux tar.gz (see [Option A](#option-a-download-a-release)); CI
  builds and tests both platforms.
- Native Windows runs were exercised during development. **Linux runtime behavior and
  production AniDB traffic have not been exercised** (see the
  [v2 verification record](aom-forum-audit.md#v2-verification-observed-only)). Treat
  Linux desktop launch and any real AniDB exchange as unverified.
- There is no installer or code signing. Releases are published as GitHub
  Releases only after a maintainer reviews a draft; see [Releases](releases.md). A CI build
  artifact is not a release.
- Animatic never contacts AniDB at startup. Network traffic to AniDB happens only when you start
  **Tools → Run command queue** or **Queue → Synchronize now** (see
  [Synchronization](user-guide.md#synchronization)). The default settings still have
  *Work offline* **off**, so the first thing to do in a fresh profile is turn it on
  ([first launch](#first-launch)).
- Separately, a startup **update check** (`updates.auto_check`, on by default) asks GitHub for
  a newer stable release unless *Work offline* is on or the settings were recovered from a
  backup. It only looks for a version: nothing is downloaded or installed until you choose
  **Download and verify** and confirm the installation
  ([Updating Animatic](user-guide.md#updating-animatic)). On a brand-new profile that means
  one request to GitHub (your IP address and the Animatic version) at the very first start;
  see [Portable and rehearsal setups](#portable-and-rehearsal-setups) to avoid it.

## Get an executable

### Option A: download a release

Each GitHub Release lists three assets (if the project has published one):

| Asset | Content | Platform |
|---|---|---|
| `animatic-vVERSION-windows-x86_64.zip` | `animatic.exe` plus `LICENSE`, `THIRD-PARTY-NOTICES.html`, `README.md`, `CHANGELOG.md` and the `docs/*.md` guides | Windows |
| `animatic-vVERSION-linux-x86_64.tar.gz` | `animatic` (executable) plus the same license and documentation files | Linux |
| `SHA256SUMS` | SHA-256 of both archives | both |

Download the archive for your platform and `SHA256SUMS`, verify the checksum, then unpack. The
commands for PowerShell and for Linux `sha256sum` are in
[Releases](releases.md#install-a-release). Each archive has a single root folder named after
the archive; the Linux binary keeps its executable bit. The archives are **unsigned** and the
bundled documentation works offline, but the Rust API documentation is not included.

Once running, an installed release can replace its own executable with a newer published
release through **Help → Check for updates…** (Windows and Linux x86_64, writable folder
required, no elevation). The bundled `LICENSE`, `THIRD-PARTY-NOTICES.html`, README, changelog
and guides stay at the version you unpacked; the new version's licenses are added as
`LICENSE-vVERSION` and `THIRD-PARTY-NOTICES-vVERSION.html` beside the executable. Details and
limits: [Updating Animatic](user-guide.md#updating-animatic). A checkout built from source can run
the updater, but it only replaces the compiled binary, never the source; use `git pull` and
rebuild, and update package-manager installations with that manager.

The Linux binary links against the desktop libraries listed below, so those must be present at
run time as well [INFERENCE: the CI job installs them for the build; the workflows do not
verify a run on a machine without them].

### Option B: build from source

Prerequisites:

| Item | Requirement | Source |
|---|---|---|
| Rust toolchain | Current stable. The workspace uses edition 2024 and `let` chains, which need a recent stable compiler. No `rust-toolchain` file or `rust-version` is pinned. | [`Cargo.toml`](../Cargo.toml), CI uses `dtolnay/rust-toolchain@stable` |
| C toolchain/linker | Whatever Rust needs on your platform (MSVC build tools on Windows). SQLite is compiled in (`rusqlite` with `bundled`). | [`crates/animatic-core/Cargo.toml`](../crates/animatic-core/Cargo.toml) |
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

## Run it

From a source checkout:

```sh
cargo run -p animatic
```

Arguments for the application go after `--`:

```sh
cargo run -p animatic -- --database /path/to/scratch/library.sqlite3
```

From a built executable:

```sh
./target/release/animatic
```

```powershell
.\target\release\animatic.exe
```

Facts about the window, taken from `crates/animatic/src/main.rs`:

- Title: "Animatic — AniDB library manager". Initial size 1280×800, minimum 900×600.
- The window starts with egui's dark visuals. A stored light theme or UI scale is applied
  when you press **Save options** or apply imported settings, not at launch (see
  [Options window](user-guide.md#options-window)).
- The source sets no Windows GUI-subsystem attribute, so a console window is expected
  next to the GUI when the executable is started from Explorer [not observed]. No logger is
  installed; the in-app **Log** page is the activity record and is not written to disk.
- If startup fails (for example unreadable settings) the process prints
  `Animatic could not start (database: …, settings: …)` with the cause to standard error and
  exits with failure; no window appears. See
  [troubleshooting](troubleshooting.md).

## Command-line reference

The parser is a single loop over the arguments in order (`main.rs`). Matching is exact and
case-sensitive.

| Argument | Meaning |
|---|---|
| `--database FILE`, `-d FILE`, `/database FILE` | Use `FILE` as the SQLite library database. The next argument is taken as the file name even if it starts with `-`. Missing value → error `--database requires a file name`. May repeat; the last one wins. |
| `--full-update`, `/fullupdate` | After startup, queue a full update: the title-list download plus one file lookup and one anime refresh for each file id and anime id that your **identified hashed local files** resolve to (not the whole local cache or title catalog). Nothing is sent to AniDB until you run the queue. There is no menu equivalent. Queueing it twice adds nothing. |
| `--help`, `-h`, `/?` | Print `Animatic [--database FILE] [--full-update] [MEDIA FILES...]` and exit. Does not create directories, files or a window. |
| `/utf8`, `/ucs2` | Accepted and ignored (legacy compatibility). |
| any other argument starting with `-` | Error: `unknown option: <argument>`. Includes `--version`, `-v`, `--database=FILE` (there is no `=` form) and `--settings`. |
| anything else | A media file path. It is hashed in the background right after startup, as if added with **File → Add files…** |

Details that follow from the code:

- **Order matters.** Arguments are processed left to right, so `--bogus --help` reports
  the unknown option instead of printing help; a media path before `--help` is simply
  discarded.
- **Case.** `/FullUpdate` or `--Database` are not options. Anything not starting with `-`
  (including `/FullUpdate`) is treated as a media file path and then fails to hash with an
  error in the **Log**.
- **No `--` separator.** A media file whose name begins with `-` cannot be passed
  directly. Prefix it with a directory (`./-clip.mkv`, `.\-clip.mkv`) so it no longer starts
  with `-`.
- **Media paths** are stored exactly as given, relative paths included, and are resolved
  against the process working directory. Use absolute paths. Directories are not expanded;
  list files explicitly. `--database`/`--full-update` and file arguments can be combined;
  the full update is queued first, then the files are hashed.
- **Settings have no flag.** The settings file can only be redirected with the
  `ANIMATIC_SETTINGS` environment variable.
- **Failure output.** Parse errors are returned from `main`, so Rust prints
  `Error: <message>` to standard error and exits with status 1 [standard behavior of a
  `main` that returns `Err`].

### Precedence

| Setting | Highest → lowest |
|---|---|
| Database file | `--database`/`-d`/`/database` → `ANIMATIC_DB` → `data_dir()/animatic.sqlite3` |
| Settings file | `ANIMATIC_SETTINGS` → `config_dir()/settings.json` |

The platform default directories are computed lazily, only for a path that has no override.
With both overridden, no default directory is consulted at all, so a portable launch does
not depend on a home directory. An empty environment variable is not treated as unset;
unset the variable instead of setting it to an empty string.

### Quoting examples

Quote any path with spaces; the quoting is your shell's, not Animatic's.

```sh
# POSIX shell
animatic -d "$HOME/animatic demo/library.sqlite3" "/data/media/Example Show - 01.mkv"
```

```powershell
# PowerShell
.\animatic.exe -d "$env:TEMP\animatic demo\library.sqlite3" "D:\Media\Example Show - 01.mkv"
```

```bat
:: cmd.exe, legacy switch spelling
animatic.exe /database "%TEMP%\animatic demo\library.sqlite3" "D:\Media\Example Show - 01.mkv"
```

## Where data lives

Animatic stores two things: a SQLite **database** (library, queue, hashes) and a JSON
**settings** file. Both parent directories are created when needed. When no override is
given, the locations come from `directories::ProjectDirs::from("net", "AniDB", "Animatic")`
(`directories` 6.0.0, pinned in `Cargo.lock`):

| | Windows | Linux |
|---|---|---|
| Settings | `%APPDATA%\AniDB\Animatic\config\settings.json` | `$XDG_CONFIG_HOME/animatic/settings.json`, else `$HOME/.config/animatic/settings.json` |
| Database | `%APPDATA%\AniDB\Animatic\data\animatic.sqlite3` | `$XDG_DATA_HOME/animatic/animatic.sqlite3`, else `$HOME/.local/share/animatic/animatic.sqlite3` |

- Windows uses the **Roaming** application-data folder (`{FOLDERID_RoamingAppData}`) for
  both, not Local. The `%APPDATA%` suffixes above were confirmed by running the
  `directories` 6.0.0 API on Windows and match its source.
- Linux paths were verified from the `directories` 6.0.0 source, not by running Animatic
  on Linux: `XDG_CONFIG_HOME`/`XDG_DATA_HOME` are used only when they hold an absolute
  path; the application folder is the lower-cased application name `animatic` (the organization is ignored).
- If the platform provides no home directory, startup fails with a message naming
  `--database` and `ANIMATIC_SETTINGS` as the fix.
- The README only says "platform directory"; the table above is the precise rule.

Companion files you will meet:

| File | When | Rule |
|---|---|---|
| `animatic.sqlite3-wal`, `animatic.sqlite3-shm` | While the database is open (SQLite WAL mode) | Same folder as the database. Copy all three together, and only with Animatic closed. |
| `settings.json.bak` | Written by **Save** when the previous primary parsed as valid | The backup name replaces the settings file's last extension with `.json.bak`. `my-settings.json` → `my-settings.json.bak`; a name without `.json` such as `custom.cfg` → `custom.json.bak`. |
| `settings.json.tmp`, `settings.json.bak.tmp` | Briefly during a save | Same replacement rule; removed after the atomic rename. |
| `import-temp/` | During a databank import | Created in the database's folder. Holds one decrypted (plaintext) collection file such as `animes.sqlite` at a time and is normally removed. After a failed import, inspect it and delete only the confirmed collection files; the folder may contain unrelated files. See [Migration](user-guide.md#migrate-an-aom-databank). |

Runtime files store your AniDB password and API key in plain text, and the `.bak` copy holds the
*previous* valid settings (possibly credentials you have since removed). Nothing is encrypted at
rest. Keep them out of version control, shared backups and support requests
([troubleshooting](troubleshooting.md)). Do not run two Animatic processes on the same settings
file; they share temporary file names and are unsupported. Window size and position are not
remembered between launches.

## Portable and rehearsal setups

Both overrides together give a self-contained profile anywhere. The examples below create a
**brand-new, uniquely named** scratch folder in the system temporary directory and put a synthetic
one-megabyte media file in it, so no real library, credentials or repository file is involved.
They never contact AniDB unless *you* run the queue. A brand-new settings file has
`updates.auto_check` on, so the first launch of a scratch profile would contact GitHub for an
update check; [keep a rehearsal local](#avoiding-the-startup-update-check) by pre-seeding the
settings file as shown below.

Creation fails instead of reusing a folder that already exists (`mktemp -d` picks a fresh random
name; the PowerShell example uses a new GUID and `New-Item` without `-Force`). Only that folder is
ever deleted, and never a fixed, predictable path.

POSIX shell (Linux):

```sh
ROOT=$(mktemp -d "${TMPDIR:-/tmp}/animatic-rehearsal.XXXXXX") || exit 1
mkdir "$ROOT/media"
head -c 1048576 /dev/zero > "$ROOT/media/synthetic-sample.mkv"
# keep the rehearsal local: no startup update check, offline AniDB
printf '%s\n' '{ "updates": { "auto_check": false }, "anidb": { "offline": true } }' > "$ROOT/settings.json"

ANIMATIC_DB="$ROOT/library.sqlite3" \
ANIMATIC_SETTINGS="$ROOT/settings.json" \
  cargo run -p animatic -- "$ROOT/media/synthetic-sample.mkv"
```

PowerShell:

```powershell
$root = Join-Path ([IO.Path]::GetTempPath()) ("animatic-rehearsal-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root -ErrorAction Stop | Out-Null
New-Item -ItemType Directory -Path (Join-Path $root 'media') -ErrorAction Stop | Out-Null
$sample = Join-Path $root 'media\synthetic-sample.mkv'
[IO.File]::WriteAllBytes($sample, (New-Object byte[] 1048576))
# keep the rehearsal local: no startup update check, offline AniDB
Set-Content -LiteralPath (Join-Path $root 'settings.json') -Value '{ "updates": { "auto_check": false }, "anidb": { "offline": true } }'

$env:ANIMATIC_DB = Join-Path $root 'library.sqlite3'
$env:ANIMATIC_SETTINGS = Join-Path $root 'settings.json'
cargo run -p animatic -- $sample
```

Keep the same shell session open (or note the printed folder name: `echo "$ROOT"` /
`$root`) and use the same pair of overrides on every launch of the rehearsal profile. In the
window, the sample is hashed, appears on **Hashed files** as `Unknown`, and (with the default
*Identify files with AniDB after hashing*) an identify command is waiting on **Queue**.
Do not run the queue with fabricated credentials or an unregistered client name.

Clean up when finished (close Animatic first). The commands remove only the folder created above and
refuse to run if the variable is empty or does not look like it:

```sh
case "$ROOT" in
  */animatic-rehearsal.??????) rm -rf -- "$ROOT" ;;
  *) echo "ROOT is not the rehearsal folder; nothing deleted" >&2 ;;
esac
```

```powershell
if ($root -and (Split-Path -Leaf $root) -like 'animatic-rehearsal-*') {
    Remove-Item -Recurse -LiteralPath $root
}
Remove-Item Env:ANIMATIC_DB, Env:ANIMATIC_SETTINGS
```

Environment variables set with `$env:` last for the PowerShell session; remove them as
shown or start a new session, otherwise a later launch silently reuses the rehearsal
profile.

### Avoiding the startup update check

The update check runs at startup only when `updates.auto_check` is `true` (the default), *Work
offline* is off and the settings were not recovered from a backup. To keep a new or rehearsal
profile from contacting GitHub, create the settings file before the first launch with a minimal
document (omitted keys take their defaults, see
[settings reference](settings-reference.md#document-layout)):

```json
{ "updates": { "auto_check": false }, "anidb": { "offline": true } }
```

Save it as the file named by `ANIMATIC_SETTINGS` (for the rehearsal above, `$ROOT/settings.json`
or `$root\settings.json`). Either key alone prevents the check; **Help → Check for updates…**
still works whenever you choose to use it.

## First launch

On a fresh profile (a first launch with default settings also performs one automatic update
check, see [status](#status-and-limits)):

1. **Startup.** The database is created and opened first, then settings are loaded:
   a missing primary and missing backup mean defaults; a valid primary loads; an
   invalid or missing primary with a valid `.bak` loads the backup **offline** and shows a
   "Settings recovered" window; unreadable settings or two unusable files abort startup.
   Details: [recovery](troubleshooting.md).
2. **Status bar** shows "Loading library…" and then "Ready". The left panel lists nine
   pages; **Overview** is shown.
3. **Overview first-run card** ("Bring your AniDB O'Matic library forward") appears
   because `first_launch` defaults to true. Use it to import a databank
   ([migration](user-guide.md#migrate-an-aom-databank)) or **Dismiss first-run card**.
   Dismissing writes the settings file immediately (a first save), unless the settings were
   recovered from a backup. A normal exit also saves the settings.
4. **Open Tools → Options…** and, before anything else:
   - tick **Work offline (keep commands queued)** and press **Save options**. The factory
     default is *not* offline;
   - leave **Username/Password/API key/Client name** empty until you have a registered
     client;
   - decide whether the managed-folder rule **Default** (which adds identified files to
     MyList as "On HDD", unwatched) is what you want. See
     [managed folders](user-guide.md#managed-folders-and-scans).
5. Add files (**File → Add files…**) or import an AOM library, then follow the
   [user guide](user-guide.md).

Defaults worth knowing (`settings.rs`, confirmed in source): theme dark, UI scale 1.0,
UDP server `api.anidb.net:9000`, local UDP port 29456, `client_version` 1, *Identify files
with AniDB after hashing* on, file-extension restriction off, hash export template
`%fulled2k%`, statistics export off.

## AniDB client registration

AniDB refuses unregistered API clients on both the UDP and HTTP APIs. The settings source
points registration at <https://anidb.net/software/add>. Rules that follow from the code:

- Options shows **Client name** only. The **client version** number sent to AniDB is the
  `anidb.client_version` JSON field (default `1`); Options has no control for it. If your
  registration uses another version, edit the settings file while Animatic is closed
  ([settings reference](settings-reference.md)).
- The local UDP port (`anidb.local_port`, default 29456) and UDP server are likewise
  JSON-only. If the port cannot be bound, the client falls back to an ephemeral port.
- The **API key** (set in your AniDB profile) is optional and turns on encrypted UDP sessions.
- An empty client name, user name or password makes the queue stop at the first command
  with an authentication error and blocks *every* pending command until you correct it and
  use **Retry failed** ([queue failure handling](user-guide.md#failure-handling)).
- Imported AOM settings do not contain a client name; set it again after applying an import.
- Never use another person's identifier or a made-up one, and never run automated
  tests against AniDB with fabricated credentials.

The HTTP API endpoint is plain `http://api.anidb.net:9001/httpapi`; the title list comes
from `https://anidb.net/api/anime-titles.xml.gz`; UDP uses `api.anidb.net:9000` by default.
Transport details: [storage and network contracts](storage-and-network.md).

## Next steps

- Hash and identify files, edit MyList, run scans: [user guide](user-guide.md).
- Import an AOM library and its settings:
  [user guide: migration](user-guide.md#migrate-an-aom-databank).
- Understand or edit stored options: [settings reference](settings-reference.md).
- Write a rename pattern: [rename language](rename-language.md).
- Developing or verifying the build: [development](development.md),
  [architecture](architecture.md).
