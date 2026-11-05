# Animatic

Animatic is a modern, cross-platform successor to **AniDB O'Matic (AOM)**. It manages an AniDB library, hashes and identifies local media, edits mylist entries, keeps an offline command queue, and imports AOM's encrypted `kowai/*.data` databank without modifying the original files.

## Current capabilities

- Direct migration of AOM anime, episode, file, group, genre, title, mylist, local hash, and local path collections.
- Simultaneous ED2K (AniDB-compatible red variant), CRC32, MD5, and SHA-1 hashing from one disk read, with pause and cancellation.
- AniDB UDP API client with authentication, encryption, compression, flood protection, automatic re-authentication, and typed commands.
- AniDB HTTP API anime records, title dump, and image downloads.
- Durable offline command queue with latest-intent MyList edits, retained failed commands, and persistent AniDB cooldowns.
- Background library snapshots and virtualized library, MyList, hashed-file, anime, group, genre, queue, and activity views.
- MyList watched/state editing, anime detail pages, cached search filters, and AniDB links.
- AOM-compatible file-renaming language and database-backed `A`, `E`, `F`, `G`, `H`, and `M` objects.
- Per-folder file-manager rules, discovery planning, automatic hash/rehash/removal decisions, and safe file moves.
- JSON settings plus reviewable, offline-first import of AOM's UTF-16 `local_settings.xml`.

The clean-room behavior mapping is documented in the [feature parity audit](docs/feature-parity.md). Original AOM binaries, extracted reverse-engineering work files, and imported library data are intentionally excluded from this repository.

Development context is captured in [`AGENTS.md`](AGENTS.md); system boundaries and
data flows are documented in [`docs/architecture.md`](docs/architecture.md).

The [AOM forum opportunity audit](docs/aom-forum-audit.md) separates verified topic
leads, blocked historical research, current implementation gaps, and shipped improvements.

## Build

Current stable Rust is required.

On Debian/Ubuntu, install the native desktop build dependencies first:

```sh
sudo apt-get install libgtk-3-dev libxkbcommon-dev libwayland-dev libssl-dev
```

```sh
cargo build --release
```

The executable is `target/release/animatic` (`animatic.exe` on Windows). The same source builds on Windows and Linux; eframe/egui provides the native desktop surface.
The CI workflow builds and uploads standalone `animatic-windows-x86_64` and
`animatic-linux-x86_64` artifacts on every push and pull request.

## Run

```sh
cargo run -p animatic
```

Application data uses the platform directory returned for `net.AniDB.Animatic`:

- Windows: `%APPDATA%` / `%LOCALAPPDATA%`
- Linux: `$XDG_CONFIG_HOME` / `$XDG_DATA_HOME`

For portable/testing use, override paths:

```sh
ANIMATIC_DB=/path/library.sqlite3 ANIMATIC_SETTINGS=/path/settings.json cargo run -p animatic
```

## Migrating from AniDB O'Matic

1. Keep AOM closed and make a backup of its `kowai` folder.
2. Start Animatic and choose **File → Import AOM databank…**.
3. Select the `kowai` directory containing `animes.data`, `mylist.data`, `local_files.data`, etc.
4. Wait for the progress bar to finish. Import is transactional per collection and can be cancelled.
5. Open **Tools → Options…**, enter AniDB credentials and your registered UDP/HTTP client name.
6. Review the queued work under **Queue**, then choose **Synchronize now**.

Import reads the encrypted source files and writes a new SQLite database. It never rewrites or deletes AOM data.

To migrate preferences, choose **File → Import AOM settings…** and select
`local_settings.xml`. A non-secret summary is shown before **Apply imported settings**
replaces your configuration. **Cancel** leaves it unchanged. Parsing happens in the
background; unrelated XML is rejected. Worker actions stay disabled until review ends.
Applied settings are saved offline; review credentials, registered client name,
managed folders and external players in **Tools → Options…** before enabling online work.
The selected XML is never modified. Parse/save failures leave current settings intact.

## AniDB client registration

AniDB requires API clients to use a registered client identifier. Configure the identifier and version under **Tools → Options…**. Keep offline mode enabled until this is set. The UDP client enforces AniDB's packet spacing and handles session expiry.

## Synchronization safety

MyList edits are saved atomically with their local display state. Repeated edits to the
same entry merge into a fresh queue generation; an older reply or acknowledgement cannot
erase newer intent. Malformed successful replies are errors, not evidence that an entry
was deleted. Existing databases receive an idempotent queue upgrade.

The **Queue** page retains permanent failures as **Blocked**. Correct the underlying
problem, then use **Retry failed**. This does not bypass a server cooldown. Bans without
an explicit duration pause synchronization for a conservative 30 minutes; temporary
connectivity failures pause for 60 seconds. **Clear queue** deliberately discards pending
work, including unsynchronized edits.

Library refreshes and statistics exports run in the background. During database contention,
the UI explicitly labels deferred identification/refresh requests as **NOT saved yet**;
they are not durable until written. A MyList edit that cannot be saved reports failure
instead of pretending it succeeded.

This release does not add full-account MyList bootstrap/reconciliation, scheduled folder
scanning, or automatic renaming.

If the primary settings file is missing or invalid and a valid backup exists, Animatic
recovers **offline** and shows a notice. Loading changes neither file; closing the recovery
session does not overwrite the primary until you explicitly **Save options**. Both files
missing means defaults. An unreadable primary or no usable copy of existing settings
reports an error rather than silently resetting preferences.

Public title downloads stream through a bounded temporary spool and merge atomically.
Failures preserve library titles and the completion timestamp. This is not a full-account
MyList import or resumable download.

The built-in rename pattern uses a fixed two-digit episode minimum, independent of cached
episode counts. Saved patterns remain unchanged; **Use stable default** is an explicit
replacement with an undo control. Syntax diagnostics and selected-file validation precede
the confirm preview; errors and evaluation warnings prevent applying a rename.

## Managed-folder safety

Automatic missing-file removal requires an explicit matching folder rule, an accessible
directory root, and a file lookup that reports absence. Unavailable drives, network roots,
and other filesystem errors preserve local records. A genuinely missing file or nested
folder under an available root can still be removed from the local inventory. This does
not delete media or remove entries from AniDB MyList.
An unmatched default rule cannot authorize removal outside its folder, and non-recursive
rules do not authorize removal from subfolders.

Under **Tools → Options… → Managed folders**, each rule can set MyList storage,
source and other fields. Unchecked means leave unchanged; checked with an empty value
means explicitly clear that field. Watched and state policies remain independent.

## Verification

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Legacy parser tests construct synthetic records, and codec tests encrypt generated SQLite
pages. No test fixture may contain bytes copied from a user's databank.

The legacy import smoke scenario can be run against an AOM folder with:

```sh
cargo run -p animatic-core --example import-legacy -- AOM/kowai /tmp/animatic-import.sqlite3
```

### v2 verification

Windows: **151 workspace tests**, formatting check, Clippy with warnings denied, and release
build passed. Native recovery, explicit save, pattern reset/restore, syntax diagnostics,
blocked empty-result preview and a successful Unicode file rename were exercised.
The [forum audit](docs/aom-forum-audit.md#v2-verification-observed-only) records the synthetic
streaming and before/after probes. Production AniDB and Linux runtime were not exercised.

## License

MIT. AniDB and AniDB O'Matic are independent projects; this repository contains no AOM source code.
