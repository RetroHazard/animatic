# Animatic

Animatic is a modern, cross-platform successor to **AniDB O'Matic (AOM)**. It manages an AniDB library, hashes and identifies local media, edits mylist entries, keeps an offline command queue, and imports AOM's encrypted `kowai/*.data` databank without modifying the original files.

## Current capabilities

- Direct migration of AOM anime, episode, file, group, genre, title, mylist, local hash, and local path collections.
- Simultaneous ED2K (AniDB-compatible red variant), CRC32, MD5, and SHA-1 hashing from one disk read, with pause and cancellation.
- AniDB UDP API client with authentication, encryption, compression, flood protection, automatic re-authentication, and typed commands.
- AniDB HTTP API anime records, title dump, and image downloads.
- Durable offline command queue for identification, mylist add/edit/delete/refresh, and record updates.
- Library, mylist, hashed-file, anime, group, genre, queue, and activity views.
- Mylist watched/state editing, anime detail pages, full-text filtering, and AniDB links.
- AOM-compatible file-renaming language and database-backed `A`, `E`, `F`, `G`, `H`, and `M` objects.
- Per-folder file-manager rules, discovery planning, automatic hash/rehash/removal decisions, and safe file moves.
- JSON settings plus import of AOM's UTF-16 `local_settings.xml`.

The clean-room behavior mapping is documented in the [feature parity audit](docs/feature-parity.md). Original AOM binaries, extracted reverse-engineering work files, and imported library data are intentionally excluded from this repository.

Development context is captured in [`AGENTS.md`](AGENTS.md); system boundaries and
data flows are documented in [`docs/architecture.md`](docs/architecture.md).

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

## AniDB client registration

AniDB requires API clients to use a registered client identifier. Configure the identifier and version under **Tools → Options…**. Keep offline mode enabled until this is set. The UDP client enforces AniDB's packet spacing and handles session expiry.

## Managed-folder safety

Automatic missing-file removal requires an explicit matching folder rule, an accessible
directory root, and a file lookup that reports absence. Unavailable drives, network roots,
and other filesystem errors preserve local records. A genuinely missing file or nested
folder under an available root can still be removed from the local inventory. This does
not delete media or remove entries from AniDB MyList.

## Verification

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The legacy import smoke scenario can be run against an AOM folder with:

```sh
cargo run -p animatic-core --example import-legacy -- AOM/kowai /tmp/animatic-import.sqlite3
```

## License

MIT. AniDB and AniDB O'Matic are independent projects; this repository contains no AOM source code.
