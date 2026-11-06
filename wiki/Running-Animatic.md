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
