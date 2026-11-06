Problems with files on disk: scans, offline drives, hashing, renaming, AOM imports, external
tools and exports.

## Scans and managed folders

A scan runs only when you choose **Tools → Scan managed folders** (it is disabled while an AOM
import or settings review is pending). It reads the managed-folder rules from the current
in-memory settings and plans three possible actions:

| Action | Condition |
|---|---|
| Hash | A file under a rule with **auto hash** whose extension is allowed and whose path is not yet in the library. If several rules cover the same path, the **first rule in list order** that takes part in discovery wins (longest-path matching applies to MyList defaults, the rename base path and removal, not to discovery) |
| Rehash | A known file under a rule with **auto rehash** whose modification time (in seconds) changed |
| Remove missing | A library path under a rule with **auto remove** that is affirmatively absent; see [Offline drives](#offline-drives-and-automatic-removal) |

If a scan hashes nothing:

1. **No rule has auto hash or auto rehash**, or its folder path is empty or not an existing
   directory. A rule with neither flag is skipped for hashing.
2. **Extensions:** scans always filter by the extension list in **Tools → Options… → Hashing**,
   whatever **Restrict file extensions** is set to. A file without an extension never matches
   and an empty list hashes nothing. Entries may include a leading dot and are matched
   case-insensitively. (Files passed on the command line are not filtered.)
3. **Hidden files** are skipped when the rule's **exclude hidden** is on, and names starting
   with `.` are hidden by this definition. Symbolic links are not followed. **Include
   subdirectories** controls depth: a non-recursive rule sees only its own folder.
4. Unreadable subfolders are skipped silently; they produce no hash actions.
5. Other settings in the rule (`auto_rename`, clear empty folders, exclude system files, the
   delay fields) are stored but not used by scans; nothing renames or deletes automatically.

Per-file hashing errors appear in the Log as `<path>: <error>` and the scan continues with the
next file. After a scan, each hashed file queues an **Identify** command when **Identify files
with AniDB after hashing** is on; with the default folder rule that command may also add the
file to MyList when the queue later runs online. Keep **Work offline** ticked until you have
reviewed the queue.

Moving or renaming files outside Animatic does not update library paths. The old record keeps
its path until a scan with **auto remove** drops it and **auto hash** adds the new location
(which hashes that file again), or until you add the file again with **File → Add files…**.

## Offline drives and automatic removal

Automatic removal only ever deletes **local inventory rows** (the path record and a stored hash
that no other path uses). It does not delete media, change MyList or touch AniDB. A removed
record means that file must be **hashed again** if you want it back, which re-reads the file.

A path is planned for removal only when all of these hold:

- a matching folder rule exists and has **auto remove** on (a non-matching path never falls
  back to the default rule, and a non-recursive rule does not cover its subfolders);
- looking up the file reports that it **does not exist** (a permission or I/O error preserves
  the record);
- the rule's root folder is **currently a directory** (an unmounted Windows drive letter or an
  unreachable network root fails this root check, so records are preserved).

Remaining risk to plan for: the root check only asks whether the folder exists. A folder that
**exists but is empty because its storage is not mounted** (a Linux mount point, a placeholder
for a network mount, a cloud "online-only" folder) looks like an available root that contains
no files, so every record under it can be removed by a scan. Likewise, storage can disconnect
between planning and execution; the check is a conservative plan, not an atomic guarantee.

Recommendations:

- Leave **auto remove** off for removable, network or cloud-backed folders.
- Back up the database before a scan with **auto remove** enabled, and check the drive is
  mounted and shows its files first.
- If records were removed because storage was absent, restore the database from a backup made
  before the scan, or remount the storage and let a scan hash the files again.

## Hashing large files

Hashing computes ED2K (the AniDB-compatible red variant), CRC32, MD5 and SHA-1 in **one pass**
over each file; nothing is read twice. Memory use does not grow with file size (4 MiB blocks).
Speed is limited by the disk, network share, USB bridge or security scanner reading the file.

- Files are hashed one after another on the background worker. The status bar shows the file
  and a progress bar; **Pause** and **Cancel** act between 4 MiB blocks.
- A cancelled file is not recorded; the remaining files in that job are skipped. The Log shows
  the file with an error such as `cancelled`. Quitting also cancels.
- A file that cannot be read (permissions, a lock by another program, a vanished network share)
  logs `<path>: <error>` and the job continues with the next file.
- A hash is stored only after the whole file was read. Nothing is saved for a crash or
  power loss mid-file; hash it again.
- Hashing does not delay the interface; the library views read their own snapshot (WAL).
  The queue runs separately and is not started by hashing.

## Rename problems

Renaming is manual and previewed: choose a file on **Hashed files**, **Rename…**, review the
**Confirm rename** window, then **Rename file**. Nothing renames automatically, and there is
no undo.

| Message | Meaning and action |
|---|---|
| `Select a hashed file first` | No file is selected |
| `The selected file is no longer in the list` / `The selected file is not identified by AniDB` | The preview needs the file's AniDB identification. Run identification online first |
| `Rename not possible: <problem>` | The pattern or the data produced an error. The window shows details |
| `Rename blocked: the renaming pattern reported warnings` | Evaluation warnings block **Rename file**. Use **Edit pattern in Options** |
| `The renaming pattern is empty. …` | Use **Use stable default** or write a pattern |
| `The renaming pattern has a syntax error: …` | Fix the pattern; **Validate on selected file** in Options dry-runs it |
| `The stable default needs the anime title, which has not been fetched yet. …` / `… needs the episode number, which is not known yet. …` | The default pattern needs fetched title and episode data. Use the anime detail window's **Refresh from AniDB**, make sure the file is identified, and run the queue online, then retry |
| `The pattern ended with the truth value "…" instead of a file name. …` | The last expression must evaluate to the new name |
| `The pattern produced an empty file name; nothing would be renamed. …` | Same cause; an empty target is an error, never a silent no-op |
| `Renaming "<old>" to "<new>" would result in an invalid file name (<why>); rename skipped!` | A path component contains one of `<>:"\|?*` or a control character, is empty, is longer than 255 (UTF-16 units on Windows, UTF-8 bytes elsewhere), or is a reserved device name such as `CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`. Trailing dots and spaces are trimmed first. The rules apply on every platform |
| `The file already has the name the pattern produces` | Nothing to do |
| `Rename failed: destination already exists: <path>` | A collision. The preview does **not** check for an existing target; the check happens when you apply it. Change the pattern or move the other file |
| `Rename failed: I/O error: <os error>` | The filesystem operation failed (see below) |
| `Rename failed: database error: …` | See the warning below |

Details:

- **Collisions** are detected only at apply time. Two files that map to the same name will
  succeed once and fail the second time.
- **Cross-volume moves fail.** A rename is a filesystem rename, not copy and delete. If the
  pattern or the rule's base path puts the target on a different volume or drive, the
  operating system refuses and nothing is moved. There is no partial copy.
- **Missing folders are created before the rename.** If the rename then fails, the new empty
  folders remain; Animatic does not remove them.
- **Open files and permissions:** a file held open by a player or scanner, a read-only
  destination, or an operating-system limit on total path length (the planner limits each
  name component but not the whole path) causes an I/O error.
- The library path is updated only **after** a successful rename. If the file moved but the
  database update then failed (`Rename failed: database error: …`, for example from a busy
  database), the library still shows the old path. Add the file again at its new location with
  **File → Add files…**, or let a scan hash it, and remove the stale record with a scan that
  has **auto remove** or accept it as stale.
- The existence check and the rename are two separate steps, so a file created at the target in
  between is not detected by Animatic. On Linux the rename can then **replace** that newly
  created file instead of failing with a collision, so do not let other programs write to
  the destination folder while you rename.

For syntax and functions see [Rename language](Rename-Language.md).

## AOM import problems

### Databank import

**File → Import AOM databank…** and the first-run card's **Browse…** open a folder picker titled
**Choose the AOM folder or its kowai folder (files are not listed)**. Folder pickers never show
files, so an empty-looking `kowai` folder is normal: select it and confirm. Either the `kowai`
folder (containing `mylist.data`, `local_files.data` or `animes.data`) or the AOM folder directly
above it is accepted; anything else is rejected with `No AOM databank found in <path> (choose
the AOM folder or its kowai folder)`.

The import reads the source files only (they are never modified), decrypts each collection
into `import-temp/`, imports it in **one transaction per collection**, and tries to remove the
plaintext (a failure can leave it behind). Results to be aware of:

- **Partial results.** A collection that fails to import is rolled back and skipped, and the
  import continues. The interface reports only `Imported N records from M collections`; the
  per-collection warnings and skipped list are not shown. If M is smaller than the number of
  recognized `.data` files in the folder, some collections were skipped.
- **A decryption error aborts the run.** The text is `Legacy import failed: <error>`.
  Collections already committed stay in the database; the remaining ones and the final
  derived-data step (highest episode, special counts, group fix-up) did not run, and the
  `legacy_import_source`/`legacy_import_date` markers were not written.
- **Cancel.** **Cancel** is cooperative and is not guaranteed to stop the import at once. It is
  honored while a collection decrypts and just before each collection's transaction; the log
  then shows `Legacy import failed: cancelled`. Decrypting a collection cannot be interrupted
  midway, so cancelling can take until the current collection finishes decrypting. A Cancel
  that arrives during the final collection's transaction or the derived-data step is ignored:
  the import can complete and report success. Already committed collections remain.
- **Recovering.** Restore a backup taken before the import, or rerun the import after fixing
  the cause. Most collections are written with replace-by-key statements, so a rerun replaces
  rows instead of duplicating them; that also means a rerun overwrites rows with the
  databank's older values, so import into a fresh database, or one you have backed up, rather
  than into a library with newer synchronized data.
- Remove leftover decrypted files as described in
  [Disk space, temporary files and leftovers](Backups-and-Recovery.md#disk-space-temporary-files-and-leftovers).
- The import menu items are disabled while an import or settings review is pending, and other
  job submissions are blocked until the review ends.

### Settings import

**File → Import AOM settings…** parses the chosen XML on the worker and shows **Review imported
AOM settings**. Errors appear as `Could not import AOM settings: …` (for example `XML: No
supported AOM settings found in the selected XML` for unrelated XML). Failure or **Cancel**
leaves your active settings unchanged and releases the lock on other actions. **Apply imported
settings** saves first and forces offline mode; if the save fails you see `Could not save
settings: …`, your old settings stay active, and the review remains open. The import starts
from defaults and replaces whole sections, so settings the XML does not mention are replaced
by the importer's fallbacks rather than kept. The imported values include the AOM password
in plain text; review managed folders, players, credentials and the registered client name
before going online (see the [Settings reference](Settings-Reference.md)).

## External tools and exports

- `player executable is not configured`: set a player in **Options → External player**.
- `invalid external command arguments: …`: the argument template could not be split (for
  example an unbalanced quote). Templates are split into an argument list before placeholder
  substitution, with no shell involved; see the [Settings reference](Settings-Reference.md)
  for placeholders.
- `AVDump integration is disabled` / `external executable is not configured`: enable and
  configure it in **Options → AVDump**. Each placeholder value (user name, key, path) is
  substituted inside its own argument, so spaces or quotes in it cannot split it. A template
  that uses `{key}` places that key on the child process's command line.
- `Hash export failed: …`, `Automatic statistics export failed: …` or a manual statistics
  export error: the destination is not writable, is a folder, the disk is full or the path is
  invalid, or (statistics) a database query failed. The previous file is left unchanged; see
  [Exports](Exports.md#safe-replacement). These write to the path you chose; they reveal your library, so keep them out of the repository.
