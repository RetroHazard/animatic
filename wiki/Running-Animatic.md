How to start Animatic from a checkout or an extracted archive, and every command-line flag and
environment variable with its precedence. Where the files end up is in
[Data locations](Data-Locations.md).

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
  [Options window](Options-Window.md)).
- The source sets no Windows GUI-subsystem attribute, so a console window is expected
  next to the GUI when the executable is started from Explorer [not observed]. No logger is
  installed; the in-app **Log** page is the activity record and is not written to disk.
- If startup fails (for example unreadable settings) the process prints
  `Animatic could not start (database: …, settings: …)` with the cause to standard error and
  exits with failure; no window appears. See
  [troubleshooting](Troubleshooting.md).

## Command-line reference

The parser is a single loop over the arguments in order (`main.rs`). Matching is exact and
case-sensitive.

| Argument | Meaning |
|---|---|
| `--database FILE`, `-d FILE`, `/database FILE` | Use `FILE` as the SQLite library database. The next argument is taken as the file name even if it starts with `-`. Missing value → error `--database requires a file name`. May repeat; the last one wins. |
| `--full-update`, `/fullupdate` | After startup, queue a full update: the title-list download plus one file lookup and one anime refresh for each file id and anime id that your **identified hashed local files** resolve to (not the whole local cache or title catalog). It leaves out what AniDB's limits do not allow yet (the title list and an anime requested within 24 hours, a file within 7 days; see [Request limits](AniDB-Synchronization.md#request-limits)) and the log says how many. Nothing is sent to AniDB until you run the queue. There is no menu equivalent. Queueing it twice adds nothing. |
| `--help`, `-h`, `/?` | Print `Animatic [--database FILE] [--full-update] [MEDIA FILES...]` and exit. Does not create directories, files or a window. |
| `/utf8`, `/ucs2` | Accepted and ignored (legacy compatibility). |
| any other argument starting with `-` | Error: `unknown option: <argument>`. Includes `--version`, `-v`, `--database=FILE` (there is no `=` form) and `--settings`. |
| anything else | A media file path. It is hashed in the background right after startup, as if added with **File → Add files…** If Animatic is already running with the same database, the paths go to that window instead; see [One window per database](#one-window-per-database). |

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
- **Media paths** given to the first window are stored exactly as given, relative paths
  included, and are resolved against the process working directory. Paths forwarded to a
  running window are made absolute first. Use absolute paths. Directories are not expanded;
  list files explicitly. `--database`/`--full-update` and file arguments can be combined;
  the full update is queued first, then the files are hashed.
- **Settings have no flag.** The settings file can only be redirected with the
  `ANIMATIC_SETTINGS` environment variable.
- **Failure output.** Parse errors are returned from `main`, so Rust prints
  `Error: <message>` to standard error and exits with status 1 [standard behavior of a
  `main` that returns `Err`].

### One window per database

Starting Animatic with media files while another Animatic already has **the same database**
open does not open a second window. The new process gives the files to the running one and
exits; the running window comes to the front and hashes them exactly as if they had been given
on its own command line (identification and MyList rules included). This is what makes the
optional **Hash with Animatic** file-menu entry (see [Options window](Options-Window.md#file-menu-entry))
usable when you select several files.

- **Same database** means the same file after resolving the directory and links, so
  `--database ./a.sqlite3` and `--database /full/path/a.sqlite3` match. A different
  `--database` (or `ANIMATIC_DB`) starts a separate window as before.
- **No files, or `--full-update`:** nothing is forwarded. A second launch without files
  opens another window as before.
- **Mechanism.** The running window listens on `127.0.0.1` only, on a random port, and records
  the port and a random token in `<database file>.instance` next to the database (see
  [Data locations](Data-Locations.md#shell-entries-and-the-lock-file)). The new process needs that token to be
  accepted. The file is removed when the window closes. A file left behind by a crash is
  detected (nobody answers) and replaced.
- **If forwarding fails** for any reason (the lock directory is not writable, the window does
  not answer, a path is not valid Unicode) the new process starts normally.
- **Bringing the window forward** is best effort. Windows and some Linux window managers may
  only flash the taskbar entry instead of taking focus.

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
