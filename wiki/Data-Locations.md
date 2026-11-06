Where the database and settings live, what else Animatic writes, and how to run a portable or
rehearsal profile that leaves your normal one untouched.

## Where data lives

Animatic stores a SQLite **database** (library, queue, hashes), a JSON
**settings** file and, once you open anime detail windows, a [folder of cover pictures](#cover-pictures). The parent directories are created when needed. When no override is
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
| `animatic-legacy-import-*/` | During a databank import | Created in the system temporary folder (not the database's folder), with a marker file `.animatic-legacy-import`. Holds one decrypted (plaintext) collection file such as `animes.sqlite` at a time and is removed when the import ends. After a crash the next start offers to remove marked folders that no running import uses; only those are removed. An older version's `import-temp/` folder beside the database is not offered. See [Migration](Migrating-from-AOM.md#migrate-an-aom-databank). |
| `animatic.sqlite3.before-restore-YYYYMMDD-HHMMSS` | After **File → Restore library…** | The library that was replaced, kept on purpose and never deleted by Animatic. Delete it yourself once the restored library is confirmed. |
| `animatic.sqlite3.restore-staging`, `.animatic-copy-*.tmp` | Briefly during a restore or backup | Removed when the step ends; an interrupted one can leave them behind. Safe to delete with Animatic closed. See [leftover files](Backups-and-Recovery.md#leftover-files-and-what-is-safe-to-delete). |
| `animatic.sqlite3.instance` | While Animatic runs | The single-instance lock, next to the database (the canonical database path plus `.instance`). Holds a loopback port and a random token; removed when the window closes. A file left by a crash is detected and replaced; it is safe to delete with Animatic closed. See [Shell entries and the lock file](#shell-entries-and-the-lock-file). |
| `covers/` | When an anime detail window downloads a cover | Same folder as the database. One file per cover, see [Cover pictures](#cover-pictures). Not part of library backups. |
| `tools/ffprobe/` (`ffprobe` or `ffprobe.exe`, `LICENSE-ffmpeg.txt`) | After **Install ffprobe…** in Options → Media properties | Inside the database's folder (`tools` next to `animatic.sqlite3`, so `--database` or `ANIMATIC_DB` moves it too). Only these two files are kept (about 130 MB). Delete the folder to remove the tool; not included in backups or the library file. See [External tools](External-Tools.md#media-properties-ffprobe). |
| `tools/.ffprobe-install-*/` | Briefly during that install | A private staging folder on the same drive, removed when the install ends; an interrupted one can be left behind and is safe to delete with Animatic closed. |

Credentials are not in these files while the system keystore works: the AniDB password, API key and
AVDump key live in the **Windows Credential Manager** or the Linux **Secret Service** under the
service name `animatic`, in entries named after a short digest of the settings file's absolute path
(so each `ANIMATIC_SETTINGS` profile has its own), and `settings.json` stores them empty. A copy of
the profile folder therefore does not carry them. When no keystore is available, Options says so and
the values stay in `settings.json` in plain text; the `.bak` copy holds the *previous* valid settings
(possibly credentials in that state that you have since removed). Keep the files out of version
control, shared backups and support requests
([troubleshooting](Troubleshooting.md); [details](Settings-Files.md#credentials-and-the-system-keystore)).
Do not run two Animatic processes on the same settings
file; they share temporary file names and are unsupported. Window size and position are not
remembered between launches.

## Cover pictures

Anime covers live in a `covers` folder next to the database (so with the default locations
`%APPDATA%\AniDB\Animatic\data\covers\` on Windows and `$XDG_DATA_HOME/animatic/covers/` or
`$HOME/.local/share/animatic/covers/` on Linux; with `ANIMATIC_DB` or `--database`, beside that
file). The folder is created when the first cover is saved.

- One file per picture, named exactly like AniDB's picture name (for example `123456.jpg`).
  [Animatic downloads a cover once](Library-Pages.md#cover), when you open that anime's detail
  window, and never downloads the same file again. A new picture name is a new file; Animatic never
  deletes the old one, so remove files you no longer want yourself (with Animatic closed or not;
  a removed cover is fetched again the next time its anime is opened, if downloads are on).
- A file is written whole or not at all. A hidden `.<name>.*` temporary file can remain after a
  crash and is safe to delete.
- The covers are **not** in the database, so [library backups](Backups-and-Recovery.md) neither
  contain nor restore them, and moving to another computer means copying the folder if you want
  them (or letting Animatic download them again).
- Covers are for your own display only. AniDB does not license the artwork: do not share or
  re-host the folder.
- You can put your own picture in the folder under the anime's picture name (shown in the
  `picname` column of the `anime` table) and it is shown instead of being downloaded; JPEG, PNG,
  WebP and GIF are read.
- To stop downloads altogether, turn off **Download anime covers when a detail window opens** in
  Options ([setting `anidb.download_covers`](Settings-Reference.md#anidb-account)); covers already
  in the folder are still shown.

## Shell entries and the lock file

Two things live outside the database and settings folders, and only when you use the matching
feature:

- **Lock file.** `<database file>.instance` next to the database, created when Animatic starts and
  removed when it closes. A launch with media files that finds a live lock for the same database
  hands the files over and exits ([One window per database](Running-Animatic.md#one-window-per-database)).
  It contains the port of a `127.0.0.1` listener, a random token, the process id and the database
  path, and no library data. On Linux the file is readable only by you. `--help` creates nothing.
- **File-menu entry** (only after you save the option on, see [File-menu entry](Options-Window.md#file-menu-entry)):
  - Windows: the key `HKEY_CURRENT_USER\Software\Classes\*\shell\Animatic.Hash` with a `command`
    subkey. View it with `reg query "HKCU\Software\Classes\*\shell\Animatic.Hash" /s`.
  - Linux: `animatic-hash.desktop` in `$XDG_DATA_HOME/applications` (default
    `~/.local/share/applications`).

  Saving the option off removes exactly these, and only when they carry Animatic's ownership
  mark (registry value `AnimaticManaged`, desktop-file line `X-Animatic-Managed=true`). Deleting the
  program does not remove the entry; turn the option off first. If you did not, delete the key or
  file by hand.

## Portable and rehearsal setups

Both overrides together give a self-contained profile anywhere. The examples below create a
**brand-new, uniquely named** scratch folder in the system temporary directory and put a synthetic
one-megabyte media file in it, so no real library, credentials or repository file is involved.
They never contact AniDB unless *you* run the queue (or, with *Work offline* off, open an anime
detail window whose cover is not on disk). A brand-new settings file has
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

A rehearsal profile that you save credentials into also creates **system keystore** entries (see
below). Deleting the folder does not remove them; delete them in the Windows Credential Manager or
your Linux Secret Service front end, or save no credentials in a rehearsal profile.

### Avoiding the startup update check

The update check runs at startup only when `updates.auto_check` is `true` (the default), *Work
offline* is off and the settings were not recovered from a backup. To keep a new or rehearsal
profile from contacting GitHub, create the settings file before the first launch with a minimal
document (omitted keys take their defaults, see
[settings reference](Settings-Reference.md#document-layout)):

```json
{ "updates": { "auto_check": false }, "anidb": { "offline": true } }
```

Save it as the file named by `ANIMATIC_SETTINGS` (for the rehearsal above, `$ROOT/settings.json`
or `$root\settings.json`). Either key alone prevents the check; **Help → Check for updates…**
still works whenever you choose to use it.
