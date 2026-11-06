Bring an AniDB O'Matic (AOM) library and its settings into Animatic. Both imports only read the
AOM files. Problems are covered in
[File and import problems](File-and-Import-Problems.md#aom-import-problems).

## Migrate an AOM databank

AOM keeps its library in encrypted `kowai/*.data` SQLite files. Animatic reads them without
modifying or deleting them.

Before you start: close AOM, **back up the `kowai` folder**, and decide the target. The import
writes into the database Animatic currently has open (default location or `--database`). For a
clean result start with an empty database (a fresh `--database` file) and keep a copy of any
existing database made while Animatic is closed (the database file plus its `-wal`/`-shm` files).
Animatic asks for confirmation when that database already holds data, but it cannot undo an
import.

1. **File → Import AOM databank…** (or the Overview first-run card). Choose the `kowai` folder or
   the AOM folder directly above it; the folder picker does not list the `.data` files inside.
   The `kowai` folder must directly contain `mylist.data`, `local_files.data` or `animes.data`,
   else Animatic says `No AOM databank found in <folder> (choose the AOM folder or its kowai
   folder)`.
2. Watch the progress bar (`Decrypting <name>.data`, `Importing <name>`, `Imported <name>`,
   `Computing derived data`, `Import complete`). Use **Cancel** to stop.
3. The Log ends with `Imported N records from M collections`.
4. Continue with [settings import](#import-aom-settings), then Options (client name!) and the
   [first sync](AniDB-Synchronization.md#synchronization).

What happens (`legacy::import::import_databank`):

- Up to 23 collections are processed **in this order**, skipping any `.data` file that is absent:
  `catcat, catlang, catqual, cattype, catvcodec, catacodec, catres, catsub, genres, groups,
  animes, animetitles, animegenres, animerelations, episodes, episodetitles, files,
  fileepisoderelations, filerelations, reviews, mylist, local_hashes, local_files`.
  If none exist the import fails with `no legacy databank files found`.
- For each collection one file is decrypted into a plain SQLite file in the temporary work
  folder (see **Temporary files** below), attached, imported inside **one transaction per
  collection**, committed, detached and the temporary file deleted. Progress is weighted by file
  size.
- **Atomicity is per collection, not for the whole import.** A collection that fails is rolled back
  and skipped while later collections still run. The Log summary reports only how many
  collections were imported, and each skipped collection is also logged as an error line
  `Legacy import warning: <collection>: <error>`, so compare `M` with the number of recognized
  `.data` files you have. Unreadable individual records inside a collection are dropped silently.
  A failure while *decrypting* (for example a truncated file) aborts the entire import with
  `Legacy import failed: …`; collections already committed stay in the database.
- **Cancel** is only checked while a collection is being decrypted and at the "Importing <name>"
  step before it. It is *not* checked inside a collection's import transaction, during the final
  derived-data processing or the closing bookkeeping. A decryption in progress finishes first. A
  Cancel that arrives late therefore may still finish with **success** (`Imported N records…`). A Cancel
  that is honored reports `Legacy import failed: cancelled`; collections already committed
  **remain**, the remaining ones are not imported, and the derived-data step and import bookkeeping
  are skipped, so run the import again (it is safe to repeat) rather than using a half-imported
  library.
- **Existing database.** Rows are written with *insert or replace* keyed by AniDB identifiers: a row
  with the same id (anime, file, group, MyList entry `lid`, local hash/file id, …) is replaced as
  a whole by the AOM copy, even if you fetched newer data from AniDB since; columns the
  import does not supply return to their defaults (so imported anime have no "fetched" marker
  and a later title-list merge may replace their titles). Rows that exist only in your database,
  and the command queue, are left alone. Re-importing the same databank converges to the same
  result. Because of that, **File → Import AOM databank…** asks first when the database already
  holds anime, MyList entries or hashed local files: **Import into a database that has data?**
  explains that rows with the same ids will be replaced. **Continue** starts the import,
  **Cancel** changes nothing. To keep the current database untouched, cancel and start Animatic
  with `--database FILE` (a new file) instead. An empty database imports without the question.
- After the last collection the highest/special episode counts are recomputed, missing MyList
  group ids are filled from file records, and the source folder plus date are recorded in the
  database.
- Nothing is queued, sent to AniDB or deleted. The import does not create queue entries; the
  Queue only fills as you hash/identify or refresh. Imported MyList entries stay as the local copy
  until an identify reply for their file says otherwise (see [Hashing and identification](Hashing-and-Identification.md)).
- Local file paths are imported verbatim (AOM's Windows paths). On Linux those paths do not
  exist; *Remove missing* only acts under an explicit folder rule whose directory exists, so
  they remain as stale records until you configure rules or remove them.
- **Temporary files.** Decrypted collections are plaintext copies of your library. They are
  written to a folder that only Animatic creates, directly in the system temporary folder: its
  name starts with `animatic-legacy-import-` and it contains a marker file named
  `.animatic-legacy-import`, created before any decrypted data is written (on Linux the folder
  is private to your user). Each copy is deleted after its collection, and the whole
  folder is removed when the import ends, whether it succeeded, failed or was cancelled; if the
  folder cannot be removed the Log says so. A crash, a kill or a power loss can still leave it
  behind. At the **next start** Animatic looks for such folders in the temporary folder and, if
  it finds any that no running import is using, shows **Leftover import files** with their paths.
  **Remove** deletes only those folders (a folder counts only if its name has the prefix **and** it
  holds the marker); **Keep for now** leaves them and Animatic asks again at the next start.
  Nothing else in the temporary folder, and never the AOM databank, is touched. A folder that
  lacks the marker (for example one left by an older version, which used an `import-temp` folder
  next to the database) is not offered and has to be inspected and removed by hand.
- The `import-legacy` example in `animatic-core` (`cargo run -p animatic-core --example
  import-legacy -- <kowai> <output.sqlite3>`) runs the same import against a database file with the
  system temporary folder as the temporary root (the same marked folder, removed the same way);
  it does not ask before writing into an existing file. Use it only with a copy of the data
  outside the repository and delete outputs afterwards
  ([development](https://github.com/RetroHazard/animatic/blob/main/docs/development.md)).

## Import AOM settings

Imports AOM's `local_settings.xml` (UTF-16 or UTF-8). It is read-only for the source file.

1. **File → Import AOM settings…**, choose the XML. **Starting an import cancels whatever job is
   currently running**, then reads the file on the worker. All worker actions (add files, import
   databank, scan, queue) stay disabled until the review ends.
2. **Review imported AOM settings** shows counts (managed folders, players, hash extensions) and
   whether credentials were detected, never the values. It states that applying **replaces** your
   Animatic settings and that offline mode stays on.
3. **Apply imported settings** saves immediately and activates the result **offline**. **Cancel**, or
   closing the window, discards it. If saving fails the error is logged and the review stays open.
   Unrelated or empty XML is rejected (`No supported AOM settings found in the selected XML`) and
   your settings are unchanged.

Important: the imported settings are built on the Animatic **defaults**, not on your current
configuration. Everything the XML does not mention (client name, client version, API key, UDP
server and port, theme, scale, current folder rules when the XML has none, …) reverts to the default,
and applying overwrites your settings file. A save first tries to copy your previous valid file to
`settings.json.bak` (a failed copy is ignored, so the backup can be stale or absent); the backup
holds the *previous* settings, including credentials you may have since removed, and is only loaded
automatically when the primary is missing or invalid. Copy your settings file yourself first if you
may want it back. The one exception is `updates.auto_check`: AOM has no such preference, so applying an
import keeps your current value.

What is read: username and password, image server, file-information flag, hashing flags and extension
list, hash export template, rename pattern, statistics export settings, AVDump enable and key, last
folder, folder rules (with their MyList policy), players, window/tray/tree preferences and row
colors, folder-scan delays. Several of those have no effect in Animatic (see the
[settings reference](Settings-Files.md#aom-settings-import)). Credentials are stored in plain text. After
applying: open Options, set the **Client name**, review folder rules and players, check
*Remove missing*/*Add to mylist*, then clear **Work offline** only when ready.
