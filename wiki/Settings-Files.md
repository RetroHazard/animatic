How `settings.json` is loaded, saved, backed up and recovered, where your password and keys are
kept, a minimal offline settings file you can copy, and how AOM settings are imported. Read it when
Animatic starts in a "Settings recovered" state, when you move a profile between computers, when you
want to know what is stored in plain text, or before you edit the file by hand. Each key is described
in the [Settings reference](Settings-Reference.md); the controls that change them are in the
[Options window](Options-Window.md).

## The files involved

Next to `settings.json` (the **primary** file; its location is in [Data locations](Data-Locations.md)
and can be moved with `ANIMATIC_SETTINGS`) Animatic keeps up to three more things:

| Item | What it is | Lasts |
|---|---|---|
| `settings.json.bak` | The **backup**: a copy of the previous valid primary, made by each save. Used only when the primary is missing or invalid. | Always |
| `settings.json.tmp`, `settings.json.bak.tmp` | Temporary files used while saving. | Only during a save (a crash during a save can leave one behind; it is safe to delete) |
| Keystore entries | The AniDB password, the AniDB API key and the AVDump key, in the Windows Credential Manager or the Linux Secret Service. | Until you delete them |

For an override such as `ANIMATIC_SETTINGS=/x/custom.cfg` the names are derived by replacing the
extension: `/x/custom.json.bak` and `/x/custom.json.tmp`; for a path without an extension,
`name.json.bak`.

## Loading and saving

### Omitted keys, unknown keys and wrong types

- Every object in the model is read with defaults: a missing key takes the default shown in the
  tables, and the whole file may be as small as `{}` (all defaults).
- Unknown keys (at any level) are ignored on load and are **not preserved**: the next save writes
  only the keys Animatic knows.
- A key with the wrong JSON type, a number out of range for its type (for example a negative
  `local_port`, a `local_port` above 65535, or a non-integer where an integer is expected),
  an unknown `theme` / `format` / `auto_rename` / `row_style` / `channel` spelling, a `null` for a
  non-nullable key, or a `columns` entry missing `id`, `width` or `visible` (`sort` is optional)
  makes the **whole file invalid**. Invalid is handled by the recovery matrix below; Animatic does not
  repair a file field by field. The one exception is `ui.color_scheme`: a missing key or a name this
  version does not know loads as `Sakura` without affecting the rest of the file.
- Error messages report the kind of failure and the line and column only, never the offending text,
  so a credential in a malformed file is not echoed.

### Load-time fixups

After a successful parse (also after an AOM import) these corrections apply in memory:

| Condition | Correction |
|---|---|
| No manager has `default: true` | Built-in "Default" rule inserted at position 0 |
| `file_managers.action_delay_ms` > 600000 | set to 600000 (0 is valid) |
| `file_managers.sleep_time_ms` > 10000 (a value stored when the key meant a pause between scans) | set to 0 |
| `file_managers.scan_interval_minutes` < 5 or > 10080 | set to 5 or 10080 |
| `anidb.idle_logout_minutes` < 1 or > 30 | set to 1 or 30 |
| `ui.zoom` <= 0.3 | set to 1.0 |

Keys that no longer exist are ignored on load and are not written back, so the next save drops
them. This applies to `anidb.client_name`, `anidb.client_version`, `anidb.http_client_name` and
`anidb.http_client_version` from older versions: Animatic always uses its own registered AniDB
clients and never reads a client from the settings file.

No other value is validated. In particular Animatic adds no new validation of paths, ports,
templates or credentials on load or save.

### When settings are written

| Event | Writes `settings.json`? |
|---|---|
| **Save options** in the Options window | Yes; also applies theme and scale and closes Options |
| Applying reviewed AOM settings | Yes, immediately (see [AOM import](#aom-settings-import)) |
| The status bar's **Online** / **Offline** button, the Overview's **Go online** button | Yes, unless the [recovery guard](#recovery-matrix) holds. The whole current settings are written, including Options edits you have not saved |
| **Hide** on the Overview's **Getting started** card | Yes, unless the recovery guard holds |
| **Restart to finish updating** (after an update was installed) | Yes. If the save fails the restart does not happen. While settings are recovered and not saved, restarting is refused until you save in Options |
| Closing the application | Yes, unless the guard holds; a failed exit save is silent. This is also when the window size, position and maximized state and the table column layouts reach the file |
| **Help → Check for updates…**, **Download and verify**, **Install** | No; none of these writes the settings file. Changing `updates.auto_check` or the channel in Options reaches the file with the next save (**Save options**, a normal exit and so on) |
| Starting with neither file present | No; defaults are used and no file is created until the first save |
| `animatic --help` | No; creates no files |

Options edits change the in-memory settings as you type. Closing the Options window with **Close**,
`Esc` or the title-bar button does not revert them: the edited values stay in effect for the session
(for example the next queue run) and are written by the next of the events above, usually when you
quit. **Save options** is the only action in Options that writes at once.

### How a save works

When you save, Animatic first hands the credential fields to the keystore (see
[Credentials and the system keystore](#credentials-and-the-system-keystore); on failure they stay in
the document), then writes pretty-printed JSON and:

1. Creates parent folders if needed.
2. Writes the new content to a sibling temporary file `settings.json.tmp` and flushes it to disk.
3. If the existing primary file still parses as a settings object, tries to copy it to
   `settings.json.bak.tmp` and rename that over `settings.json.bak` (credentials that now live in
   the keystore are blanked in that copy). An invalid existing primary
   never replaces a good backup. **A failure in this step is ignored**: the save carries on, so the
   new primary can be installed while the backup stays stale or absent.
4. Renames the temporary file over `settings.json`.

If writing the new temporary file or the final rename fails, that temporary file is removed and the
previous primary stays in place; this guarantee does not extend to the backup (step 3). A failed save
shows `Could not save settings: …` in the status bar and the Log; the in-memory settings are
unaffected.

**Concurrent writers are unsupported.** There is no file locking, and every process uses the same
`settings.json.tmp` and `settings.json.bak.tmp` names. Two Animatic processes sharing one settings
file can remove or rename each other's temporary files, so a save may fail with an error or may
install content written by the other process; the outcome is not a predictable "last writer
wins". Run one instance per settings file (the [one window per database](Running-Animatic.md#one-window-per-database)
rule normally ensures that).

The `.bak` file persists and holds the previous valid settings, including whatever credentials they
contained when they were written (none once the keystore holds them; see
[Credentials and the system keystore](#credentials-and-the-system-keystore)). Because every save
rotates it, the backup is the state **before your latest save**, not a long history. To keep an older
state, copy the file yourself.

### Recovery matrix

Loading never modifies any file. Primary means `settings.json`; backup means `settings.json.bak`.

| Primary | Backup | Result |
|---|---|---|
| valid object | not read | Primary loaded normally |
| unreadable (permissions, a folder in the way, I/O error) | not read | Startup error naming the file; nothing is reset or replaced |
| missing or invalid | valid | **Backup loaded, `offline` forced to true**, a recovery window appears, nothing is written to the primary |
| missing | missing | Defaults; no file created |
| missing or invalid | missing, invalid or unreadable | Startup error naming both files and the problem class; neither file is modified. Repair or remove them to start with defaults |

"Invalid" means not valid JSON, truncated, empty, a wrong-typed value anywhere, or a JSON document
whose root is not an object (`[]`, `null`, `42`, `"text"`, `true` are all rejected, so they never
count as an empty-defaults file and never replace a backup).

A **startup error** is printed to the terminal when you started Animatic from one; started from a
shortcut or Explorer on Windows it appears in a message box titled `Animatic`, beginning
`Animatic could not start:`. The text names the database and settings paths and the problem class
(for example `invalid JSON syntax at line 12 column 3`), never a value from the file.

#### What a backup recovery looks like

After a backup load, a window titled **Settings recovered** opens with this text (your paths in place
of the placeholders):

> The settings file could not be used, so Animatic loaded the backup copy (`<backup path>`) and
> switched to offline mode.
>
> The original file (`<primary path>`) has not been changed. Animatic will not write settings when it
> closes unless you save them yourself in Options; saving replaces the original file.

Behind the window Animatic keeps these protections until you save:

- Animatic starts offline (`anidb.offline` is forced to true for this session). You may switch it off
  again; nothing is written until you save. The automatic startup update
  check does not run.
- The exit save, the status bar's **Online** / **Offline** button writes, the Overview's **Hide** and
  **Go online** writes and the update restart are all skipped, so the unreadable original is not
  replaced by a decision you did not make.
- Scheduled scans do not start while the notice window is open.

What to do:

1. Click **OK** on the notice, then open **Tools → Options…** (`Ctrl+,`).
2. Check each category. The values are those of the backup, which is the state before the save that
   preceded the damage; recent changes may be missing.
3. Click **Save options**. This writes the recovered values (including `offline: true`) over the
   primary, rotates the backup, and, when the keystore works, writes the recovered credentials to it.
   The protections end.
4. Untick **Work offline (keep commands queued)** (or click the status bar's **Offline** button) when
   you want to go online.

If you would rather inspect the damaged file first, quit without saving: the original stays as it was.
More support steps are in [Troubleshooting](Troubleshooting.md).

## Credentials and the system keystore

`anidb.password`, `anidb.api_key` and `avdump.key` are kept in the operating-system keystore: the
**Windows Credential Manager** or, on Linux, the **Secret Service** (GNOME Keyring, KWallet and
compatible). A successful save writes them there and stores them as empty strings in `settings.json`
together with `"secrets_in_keystore": true`; loading reads the empty fields back from the keystore.
(The Linux Secret Service has not been verified at runtime by the project; on other platforms there
is no supported keystore and the fallback below applies.)
Entries use the service name `animatic` and an account made of a short digest of the settings file's
absolute path plus the field name (for example `3f9c1a7d2b4e6a80/anidb.password`), so two portable
profiles ([`ANIMATIC_SETTINGS`](Data-Locations.md)) never share or overwrite each other's credentials.
Moving or renaming a settings file therefore also changes its keystore account: the stored credentials
are not found and must be entered again (the old entries remain in the keystore; delete them there).

- **Migration.** A settings file written by an older version still holds the three values in plain
  text. Loading it changes nothing on disk. The first successful **Save options** (or any other
  save) moves them into the keystore, writes empty strings, and also blanks them in the
  `settings.json.bak` copy it creates from the old file, so no plain-text copy stays behind. If
  that backup step fails (it is ignored, as for every save), an older plain-text backup can
  remain: delete it yourself.
- **Fallback, never a lost secret.** If the keystore is unavailable (for example a headless Linux
  session without a Secret Service) or refuses a write, Animatic keeps the values **in
  `settings.json` exactly as before** (all three, even those the keystore had accepted) and Options
  shows, under the AniDB account section, a yellow line: `Credentials are stored in the settings
  file because no system keystore is available: <reason>` (the reason is the keystore's own
  message). Every later save tries the keystore again and moves the values as soon as it works.
  In this state all the cautions below about plain text apply.
- **Unreadable credentials.** If the file says the credentials are in the keystore but reading them
  fails (locked or removed keystore), the fields stay empty and Options says so (`Saved credentials
  could not be read from the system keystore: …`). Saving without typing new values leaves the
  stored entries untouched; typing a value replaces it. Clearing a field you did read and saving
  deletes its entry.
- **Recovery stays offline and read-only.** Loading the backup after a damaged primary only *reads* the
  keystore; nothing is written to it (or to either file) until you use **Save options**. Error
  messages never include credential values.
- **Imports.** The AOM settings review shows only whether credentials were detected, never their
  values; applying the import saves through the same path, so imported credentials go to the
  keystore too.
- **A keystore prompt can pause the window.** A keystore that asks to be unlocked can pause the
  window during a save.

What a settings file can still expose:

- With the keystore working, `settings.json` and its backup contain no credentials. Anyone who can
  run programs as you can still read the keystore; it protects against copying files, not against
  malware running in your session. Credentials in the keystore are **not** part of a copy of your
  profile folder, so back them up with your operating system's own tools.
- In the fallback state, anyone who can read your profile folder (or a backup of it) can read the
  values. **The backup can hold a different secret than the current file.** A save copies the
  *previous* valid primary into `settings.json.bak`, so after you change or remove a credential
  the backup keeps the old value until a later save replaces it. Check and delete both files before
  sharing or discarding a configuration.
- Do not copy settings files into bug reports, forum posts, repositories or screenshots. The
  repository's `.gitignore` excludes `settings.json` but not `settings.json.bak`, so be careful with
  portable setups kept inside a checkout.
- The Options password field is masked, but the API key fields are not.
- To inspect or delete the entries: Windows Credential Manager (*Control Panel → Credential
  Manager → Windows Credentials*, or `cmdkey /list` and look for entries whose name contains
  `animatic`), or on Linux the Secret Service front end of your desktop (for example Seahorse or
  KWalletManager; the items carry the attributes `service` = `animatic` and `username` = the
  account). Names may differ slightly by Windows version and desktop.

## Minimal offline settings file

This synthetic file keeps Animatic offline, lists two extensions and defines one managed folder. The
user name and folder are fictitious. No AniDB client identifier belongs in a settings file: Animatic's
own registered clients are built in.

```json
{
  "anidb": {
    "username": "animatic-test-user",
    "offline": true
  },
  "hashing": {
    "use_extensions": true,
    "extensions": ["mkv", "mp4"]
  },
  "file_managers": {
    "managers": [
      {
        "name": "Synthetic library",
        "path": "/animatic-test-root/anime",
        "auto_hash": true,
        "mylist_auto_add": true
      }
    ]
  },
  "first_launch": false
}
```

Reading the file against the settings model, it loads as follows (this exact file was not run):

- All other `anidb` keys keep their defaults (UDP server `api.anidb.net:9000`, port 29456,
  empty password); with `offline: true` and no password, no network traffic is possible.
- The single listed folder rule gets the omitted-key defaults (`include_subdirs` true,
  `exclude_hidden` true, `mylist_state_check` true with state 1). Because no rule is flagged
  `default`, the built-in "Default" rule is inserted first, and the network entry is absent.
- All other sections (`ui`, `players`, `stat_whore`, `avdump`, `renaming_pattern`, `updates`,
  `media`) take their defaults.
- On Windows use an existing folder and escape backslashes, for example
  `"D:\\animatic-test-root\\anime"`.

Closing Animatic writes the complete file back, so the file then also contains every defaulted key.

## AOM settings import

**File → Import → AOM settings…** reads an AOM `local_settings.xml` (UTF-8 or UTF-16) on top of the
defaults and lets you review the result before anything changes. The migration workflow around it is
in [Import AOM settings](Migrating-from-AOM.md#import-aom-settings).

### Steps

1. Choose **File → Import → AOM settings…**. A file dialog titled `Choose AOM local_settings.xml`
   (filter `AOM settings XML`) opens in the folder you used last. The item is disabled while another
   import is in progress.
2. Pick the file. Animatic reads it in the background (the status bar shows `Reading AOM
   settings`). The file is only read; it is never changed.
3. If the XML contains no setting Animatic recognizes, the import stops with `Could not import AOM
   settings: XML: No supported AOM settings found in the selected XML`. Nothing changes; your
   your settings are not reset.
4. Otherwise the window **Review imported AOM settings** appears. It shows no values, only a summary:
   - **Applying will replace your current Animatic settings.**
   - A yellow warning: `Imported credentials will not connect to AniDB: offline mode remains enabled.`
   - `Review managed folders before scanning them.`
   - `Managed folders: N`, `External players: N`, `Hash file extensions: N`.
   - Either `Imported credentials were detected and will require review.` or `No imported credentials
     were detected.`
5. Click **Apply imported settings**, or **Cancel** (closing the window with its **✕** counts as
   Cancel).

### What happens next

- **Apply imported settings** saves the imported settings to `settings.json` at once (rotating the
  previous valid file into `settings.json.bak`), replaces the in-memory settings, forces
  `offline: true`, applies the theme, color scheme and scale, and logs `Settings saved`. Imported
  credentials go to the system keystore like any saved credential. The Overview's Getting started card
  reappears when the imported user name is empty (`first_launch`).
- If that save fails, `Could not save settings: …` is shown and the review window stays open so you can
  try again or cancel.
- **Cancel** changes nothing; the active settings are preserved and no file is touched.
- While the review window is open (and while the file is being read), other background work is
  blocked: **Add files…**, the other imports and **Run command queue** are disabled with the hover
  text `An import is in progress`, and scheduled scans and automatic queue runs wait. Installing an
  update and library tasks also wait (`Apply or cancel the imported settings first.`).
- After applying, review every managed folder before going online or scanning. Then untick **Work
  offline** when you are ready.

### What is replaced and what is kept

The import starts from the defaults of a fresh file, not from your current settings: **every value the
XML does not mention (including your current API key, UDP server and port, theme and scale) is
replaced by its default**, and the importer's own fallbacks for absent XML values (shown below) can
differ from those defaults. The exceptions are the preferences AOM cannot know: the apply step copies
your current `updates` object (`updates.auto_check`, `updates.channel`), your current `media` object
(`media.read_properties`, `media.ffprobe_path`) and your current `ui.color_scheme` into the imported
settings before saving. The previous settings otherwise survive only in the backup file. Column
layouts (`ui.columns`) are replaced by the defaults too.

| AOM element | Animatic key |
|---|---|
| `anidb/username`, `password` | `anidb.username`, `anidb.password` |
| `anidb/imageServer` (non-empty), `updateFileInformation` | `anidb.image_server`, `anidb.update_file_information` |
| `general/autostartHashing` (absent: false), `hashingThreadPriority` (0-6) | `hashing.autostart`, `hashing.priority` (stored only) |
| `general/useExtensions`, `extensions` (comma list) | `hashing.use_extensions`, `hashing.extensions` |
| `general/exportTemplate` (non-empty), `integrateIntoShell` | `hashing.export_template`, `hashing.integrate_into_shell` (stored only; no file-menu entry is registered by the import) |
| `general/renamingPattern` (non-blank) | `renaming_pattern` |
| `general/showLocalFolderSize`, `hideRootFolder`, `copyLengthAsMinutes`, `treeAutoExpand`, `treeAutoCollapse`, `showTrayIcon`, `minimizeToTray` | the matching `ui.*` keys |
| `general/statWhore`, `statWhoreFormat` (`INI` or else XML), `statWhoreFileName` | `stat_whore.enabled`, `.format`, `.file` |
| `general/useAVDump`, `avdumpPassword` | `avdump.enabled`, `avdump.key` |
| `ui/lastDirectory`, `mainMaximized`, `mainLeft/Top/Width/Height` | `ui.last_directory`, `ui.maximized`, `ui.window` (only when all four numbers are present, the width and height exceed 200 and left and top exceed -100000) |
| `color` elements (tag, value) | `ui.colors` (AOM's BGR integers converted to RGB; a text color of `0` means no override) |
| `player` records (`name` required) | `players[]` (`name`, `path`, `open`, `play`, `enqueue`, `default`) |
| `filemanager/actionDelay` | `file_managers.action_delay_ms` (with the fixup; negative becomes 0) |
| `filemanager/sleepTime` (AOM's pause between scans) | `file_managers.scan_interval_minutes` in whole minutes (with the fixup); `scheduled_scan` stays off |
| `manager` records (`name` required) | `file_managers.managers[]` (all rule keys; absent booleans become false except `includesubdirs` and `caneditpath`, which become true; absent `myliststate` becomes 0; `autorename` true becomes `dry_run`, never `on`) |

Not touched by the import (they keep fresh-file defaults): `anidb.api_key`, `udp_server`,
`local_port`, `download_covers`, `idle_logout_minutes`, `auto_run_queue` (so an import never
turns automatic queue runs on), `hashing.identify_after_hashing`, `avdump.executable`,
`avdump.arguments`, `ui.theme`, `ui.zoom`, `ui.colored_rows`, `ui.row_style`,
`file_managers.scan_at_start`, `scheduled_scan`, `sleep_time_ms` and `ed2k_space_replacement`. After
an import you therefore still have to review every managed folder before going online or scanning.
The AniDB client identifiers are not settings, so an import neither sets nor needs them. Carriage
returns are normalized in imported templates and patterns.

## See also

- [Options window](Options-Window.md): **Save options**, **Close** and when each change applies.
- [Settings reference](Settings-Reference.md): every key, type and default.
- [Migrating from AOM](Migrating-from-AOM.md): the whole AOM migration, including the databank.
- [Backups and recovery](Backups-and-Recovery.md): backing up the library database.
- [Troubleshooting](Troubleshooting.md): a settings file that will not load.
