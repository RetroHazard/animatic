Animatic is a native Rust successor to **AniDB O'Matic (AOM)** for Windows and Linux. It manages
local media files and their AniDB records, keeps MyList edits safe while offline, and imports an
existing AOM library without modifying it.

These pages describe what the desktop application does today. A setting that is stored but has no
effect, or a library capability that has no screen, is labeled as such. Linux runtime behavior and
production AniDB traffic have not been exercised by the project; see
[Status and limits](Installation.md#status-and-limits).

## Get started

| Page | Read it for |
|---|---|
| [Installation](Installation.md) | Status and limits, downloading and verifying a release, building from source |
| [Running Animatic](Running-Animatic.md) | Starting the application and the complete command-line reference |
| [Data locations](Data-Locations.md) | Where the database and settings live, portable and rehearsal profiles |
| [First launch](First-Launch.md) | Safe first-run settings and AniDB client registration |

Coming from AOM? Read [Migrating from AOM](Migrating-from-AOM.md) before the first launch.

## Use Animatic

| Page | Read it for |
|---|---|
| [Main window](Main-Window.md) | Layout, status bar and progress, searching and tables, menus |
| [Library pages](Library-Pages.md) | Overview, My list, Anime, Advanced search, Hashed files, Groups, Genres, Queue, Log and the anime detail window |
| [Hashing and identification](Hashing-and-Identification.md) | Adding files, the single-pass hashes and identifying files on AniDB |
| [Managed folders and scans](Managed-Folders-and-Scans.md) | Folder rules, manual scans and safe removal of missing files |
| [AniDB synchronization](AniDB-Synchronization.md) | The command queue, MyList edits and failure handling |
| [Renaming files](Renaming-Files.md) | Previewing and applying a rename to one file |
| [Exports](Exports.md) | Statistics and hash exports |
| [External tools](External-Tools.md) | External players and AVDump |
| [Options window](Options-Window.md) | Every section of **Tools → Options…** |
| [Updating Animatic](Updating-Animatic.md) | Checking for, verifying and installing a newer release |
| [Migrating from AOM](Migrating-from-AOM.md) | Importing an AOM databank and AOM settings |

## Settings and renaming

| Page | Read it for |
|---|---|
| [Settings reference](Settings-Reference.md) | Every `settings.json` key: type, default, Options control and whether it does anything |
| [Settings files](Settings-Files.md) | Loading, saving, backup recovery, plain-text credentials and the AOM settings import |
| [Rename language](Rename-Language.md) | Syntax, values and coercion, variables and user functions |
| [Rename functions and objects](Rename-Functions-and-Objects.md) | Every built-in function and object property |
| [Rename targets and apply](Rename-Targets-and-Apply.md) | How a result becomes a path, blocking rules and the stable default pattern |
| [Rename patterns](Rename-Patterns.md) | Worked patterns, pitfalls, diagnostics and how to verify a pattern |

## Troubleshooting

| Page | Read it for |
|---|---|
| [Troubleshooting](Troubleshooting.md) | Limits, a symptom index, which files are in use, startup problems and recurring messages |
| [Backups and recovery](Backups-and-Recovery.md) | Backups, settings recovery, temporary and leftover files |
| [Network and sync problems](Network-and-Sync-Problems.md) | Network access, credentials, cooldowns and bans, queue states, update problems |
| [File and import problems](File-and-Import-Problems.md) | Scans, offline drives, hashing, renaming, AOM imports, external tools |
| [Reporting problems](Reporting-Problems.md) | Privacy-safe support requests and bug reports |

## Compared with AOM

[Feature parity](Feature-Parity.md) maps every AOM workflow to its Animatic equivalent and lists
what exists only in the Rust libraries, what is stored without effect and what is not
implemented.

## About this wiki

The wiki is published from the
[`wiki/` folder of the repository](https://github.com/RetroHazard/animatic/tree/main/wiki), and
release archives carry the same pages in `docs/` for offline reading. Corrections go through a
pull request; edits made in the wiki editor are overwritten by the next publish.
Developer documentation (architecture, storage and network contracts, development and releases)
stays in the repository: see the
[developer documentation index](https://github.com/RetroHazard/animatic/blob/main/docs/README.md).
