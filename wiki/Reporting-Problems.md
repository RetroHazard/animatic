How to ask for help or report a bug without leaking credentials, file names or library data.
Sanitize first, then use the
[issue forms](https://github.com/RetroHazard/animatic/issues/new/choose).

## What is sensitive

- **Settings and backups:** `settings.json`, `settings.json.bak` and any `.tmp` copy hold the
  AniDB password, API key and AVDump key in plain text. Never attach or paste them.
- **The database** (`animatic.sqlite3` and its `-wal`/`-shm`), exports (statistics XML/INI,
  hash lists) and `import-temp` contents are personal data: viewing history, file paths and
  names, hashes, AniDB identifiers.
- **The Log page** is the only diagnostic channel, and it is kept in memory only (older lines
  are discarded as it grows, **Clear** empties it, and it is lost on exit; there is no log
  file and no export button). Lines include full file paths (which can contain your user and
  drive names and series titles), ED2K hashes, local and AniDB identifiers, server reply text
  and cooldown reasons. Animatic does not intentionally write credentials into log lines, but
  review every line you share.
- **Startup errors** print the database and settings paths, which often contain your user name.

## How to sanitize

1. Copy only the lines you need and replace: user and drive names in paths with `<user>`,
   `<root>`; titles and file names with `<file>`; ED2K hashes, `lid`/`fid`/`aid`/`gid`
   numbers and session text with `<id>`; any user name, password, API key or client name with
   placeholders.
2. Keep exact message wording (for example `AniDB sync paused until <time>: <reason>`), the
   status-bar indicators (**Offline**, **N blocked**, **N commands NOT saved yet**), and what
   you clicked. Those usually identify the problem without any personal data.
3. Never post settings files or databases. If a settings detail matters, describe the field
   and value type (for example "`anidb.offline` is true"), not the file.
4. Include: the version shown in **Help → About Animatic**, the operating system, whether you
   ran a release archive, a CI artifact or a local build, and whether the settings were recovered.
5. To reproduce, build a **synthetic** case instead of sharing yours: temporary database and
   settings paths (see [Portable and rehearsal setups](Data-Locations.md#portable-and-rehearsal-setups)),
   **Work offline** ticked, small generated files under a fictitious root such as
   `<temp>/animatic-test-root`, and the user name `animatic-test-user`.
6. Do not try to reproduce a problem by sending requests to AniDB, and never use made-up client
   names or credentials against it.

Example of a sanitized report:

```text
Animatic X.Y.Z, Windows (local build). Settings loaded normally.
Steps: Tools -> Run command queue, offline unticked.
Result: Log "<time>  ERROR  <command>: authentication: login failed: wrong username or password"
        then "Command blocked: 3 queued commands blocked: authentication: ..."
Status bar: "3 blocked". Retry failed + Synchronize now repeats the same error.
```

The repository's bug report form asks for these fields and requires you to confirm that the
report is sanitized. Issues are public.

This guide defines no security-reporting process, and the repository contains none. If you
believe you found a vulnerability, share sanitized details through your normal channel with the
maintainers, and do not publish credentials or private library data anywhere public.
