Where the database and settings live, what else Animatic writes, and how to run a portable or
rehearsal profile that leaves your normal one untouched.

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
| `animatic-legacy-import-*/` | During a databank import | Created in the system temporary folder (not the database's folder), with a marker file `.animatic-legacy-import`. Holds one decrypted (plaintext) collection file such as `animes.sqlite` at a time and is removed when the import ends. After a crash the next start offers to remove marked folders that no running import uses; only those are removed. An older version's `import-temp/` folder beside the database is not offered. See [Migration](Migrating-from-AOM.md#migrate-an-aom-databank). |

Runtime files store your AniDB password and API key in plain text, and the `.bak` copy holds the
*previous* valid settings (possibly credentials you have since removed). Nothing is encrypted at
rest. Keep them out of version control, shared backups and support requests
([troubleshooting](Troubleshooting.md)). Do not run two Animatic processes on the same settings
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
[settings reference](Settings-Reference.md#document-layout)):

```json
{ "updates": { "auto_check": false }, "anidb": { "offline": true } }
```

Save it as the file named by `ANIMATIC_SETTINGS` (for the rehearsal above, `$ROOT/settings.json`
or `$root\settings.json`). Either key alone prevents the check; **Help → Check for updates…**
still works whenever you choose to use it.
