How `settings.json` is loaded, saved, backed up and recovered, where credentials are kept, a
minimal offline configuration, and how AOM settings are imported. Each key is described in the
[Settings reference](Settings-Reference.md).

## Loading and saving

### Omitted keys, unknown keys and wrong types

- Every object in the model is read with defaults: a missing key takes the default shown in the
  tables, and the whole file may be as small as `{}` (all defaults).
- Unknown keys (at any level) are ignored on load and are **not preserved**: the next save writes
  only the keys Animatic knows.
- A key with the wrong JSON type, a number out of range for its type (for example a negative
  `local_port`, a `local_port` above 65535, or a non-integer where an integer is expected),
  an unknown `theme` / `format` spelling, a `null` for a non-nullable key, or a `columns` entry
  missing one of its three fields makes the **whole file invalid**. Invalid is handled by the
  recovery matrix below; Animatic does not repair a file field by field.
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
| `ui.zoom` <= 0.3 | set to 1.0 |

No other value is validated. In particular Animatic adds no new validation of paths, ports,
templates or credentials on load or save.

### When settings are written

| Event | Writes `settings.json`? |
|---|---|
| Options → **Save options** | Yes; also applies theme and scale and closes Options |
| Applying reviewed AOM settings | Yes, immediately (see [AOM import](#aom-settings-import)) |
| Help → Check for updates…, Download and verify, Install | No; none of these writes the settings file. Only **Save options** (including the `updates.auto_check` checkbox) does |
| Overview → "Dismiss first-run card" | Yes, unless the [recovery guard](#recovery-matrix) holds |
| Closing the application | Yes, unless the guard holds; a failed exit save is silent |
| Starting with neither file present | No; defaults are used and no file is created until the first save |

Options edits change the in-memory settings as you type. Closing the Options window with the title
bar button does not revert them: the edited values stay in effect for the session (for example
the next queue run) and are written at exit. "Save options" is the only action that writes
immediately.

### How a save works

`Settings::save` first hands the credential fields to the keystore (see
[Credentials and the system keystore](#credentials-and-the-system-keystore); on failure they stay in
the document), then serializes to pretty-printed JSON and:

1. Creates parent folders if needed.
2. Writes the new content to a sibling temporary file `settings.json.tmp` and flushes it to disk.
3. If the existing primary file still parses as a settings object, tries to copy it to
   `settings.json.bak.tmp` and rename that over `settings.json.bak` (credentials that now live in
   the keystore are blanked in that copy). An invalid existing primary
   never replaces a good backup. **A failure in this step is ignored**: the save carries on, so the
   new primary can be installed while the backup stays stale or absent.
4. Renames the temporary file over `settings.json`.

If writing the new temporary file or the final rename fails, that temporary file is removed and the
previous primary stays in place; this guarantee does not extend to the backup (step 3).

**Concurrent writers are unsupported.** There is no file locking, and every process uses the same
`settings.json.tmp` and `settings.json.bak.tmp` names. Two Animatic processes sharing one settings
file can remove or rename each other's temporary files, so a save may fail with an error or may
install content written by the other process; the outcome is not a predictable "last writer
wins". Run one instance per settings file.

File names are derived with "replace the extension": for `settings.json` they are `settings.json.bak`
and `settings.json.tmp`. For an override such as `ANIMATIC_SETTINGS=/x/custom.cfg` they become
`/x/custom.json.bak` and `/x/custom.json.tmp`; for a path without extension, `name.json.bak`.
The `.tmp` files are transient (a crash during a save can leave one behind); the `.bak` file persists
and holds the previous valid settings, including whatever credentials they contained when they were
written (none once the keystore holds them; see
[Credentials and the system keystore](#credentials-and-the-system-keystore)).

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

After a backup recovery the guard keeps the unreadable original untouched: the exit save and the
first-run-card save are skipped, so recovered settings survive only in memory until you use
**Save options** (or apply an AOM import). That explicit save writes the recovered values, including
`offline: true`, over the primary (and, when the keystore works, writes the recovered credentials to
it); untick the offline option afterwards if you want to go online.
Step-by-step recovery and support guidance: [Troubleshooting](Troubleshooting.md).

## Credentials and the system keystore

`anidb.password`, `anidb.api_key` and `avdump.key` are kept in the operating-system keystore: the
**Windows Credential Manager** or, on Linux, the **Secret Service** (GNOME Keyring, KWallet and
compatible). A successful save writes them there and stores them as empty strings in `settings.json`
together with `"secrets_in_keystore": true`; loading reads the empty fields back from the keystore.
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
  `settings.json` exactly as before** and Options shows, under the AniDB account section, a yellow
  line: *Credentials are stored in the settings file because no system keystore is available:
  `<reason>`* (the reason is the keystore's own message). Every later save tries the keystore
  again and moves the values as soon as it works.
  In this state all the cautions below about plain text apply.
- **Unreadable credentials.** If the file says the credentials are in the keystore but reading them
  fails (locked or removed keystore), the fields stay empty and Options says so. Saving without
  typing new values leaves the stored entries untouched; typing a value replaces it. Clearing a
  field you did read and saving deletes its entry.
- **Recovery stays offline and read-only.** Loading the backup after a damaged primary only *reads* the
  keystore; nothing is written to it (or to either file) until you use **Save options**. Error
  messages never include credential values.
- **Imports.** The AOM settings review shows only whether credentials were detected, never their
  values; applying the import saves through the same path, so imported credentials go to the
  keystore too.
- **Writes happen on the interface thread.** A keystore that asks to be unlocked can pause the
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
  Manager → Windows Credentials*, or `cmdkey /list` and look for targets ending in `.animatic`),
  or on Linux the Secret Service front end of your desktop (for example Seahorse or KWalletManager;
  the items carry the attributes `service` = `animatic` and `username` = the account).

## Minimal offline configuration

This synthetic file keeps Animatic offline, lists two extensions and defines one managed folder. The
user name and folder are fictitious; no client identifier is included because none ships with
Animatic.

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

Read against the model, this loads as follows (not executed as part of writing this page):

- All other `anidb` keys keep their defaults (UDP server `api.anidb.net:9000`, port 29456,
  empty password and client name); with `offline: true` and no client name, no network traffic is
  possible.
- The single listed folder rule gets the omitted-key defaults (`include_subdirs` true,
  `exclude_hidden` true, `mylist_state_check` true with state 1). Because no rule is flagged
  `default`, the built-in "Default" rule is inserted first, and the network entry is absent.
- All other sections (`ui`, `players`, `stat_whore`, `avdump`, `renaming_pattern`) take their
  defaults.
- On Windows use an existing folder and escape backslashes, for example
  `"D:\\animatic-test-root\\anime"`.

Closing Animatic writes the complete file back, so the file then also contains every defaulted key.

## AOM settings import

File → Import AOM settings… reads an AOM `local_settings.xml` (UTF-8 or UTF-16) on top of the
defaults, shows a review window and, on **Apply imported settings**, saves it immediately
(rotating the previous valid file into `settings.json.bak`), replaces the in-memory settings,
forces `offline: true` and applies the theme and scale. Cancel changes nothing. XML containing no
recognized setting is rejected instead of resetting your configuration. The import starts from the
defaults of a fresh file, not from your current settings: **every value the XML does not mention
(including your current client name, API key, UDP server and port, theme and scale) is replaced
by its default**, and the importer's own fallbacks for absent XML values (shown below) can differ
from those defaults. The one exception is `updates.auto_check`: the apply step copies your current
`updates` object into the imported settings before saving, because AOM has no such preference.
The previous settings otherwise survive only in the backup file. Migration workflow:
[Import AOM settings](Migrating-from-AOM.md#import-aom-settings).

| AOM element | Animatic key |
|---|---|
| `anidb/username`, `password` | `anidb.username`, `anidb.password` |
| `anidb/imageServer` (non-empty), `updateFileInformation` | `anidb.image_server`, `anidb.update_file_information` |
| `general/autostartHashing` (absent: false), `hashingThreadPriority` (0-6) | `hashing.autostart`, `hashing.priority` |
| `general/useExtensions`, `extensions` (comma list) | `hashing.use_extensions`, `hashing.extensions` |
| `general/exportTemplate` (non-empty), `integrateIntoShell` | `hashing.export_template`, `hashing.integrate_into_shell` |
| `general/renamingPattern` (non-blank) | `renaming_pattern` |
| `general/showLocalFolderSize`, `hideRootFolder`, `copyLengthAsMinutes`, `treeAutoExpand`, `treeAutoCollapse`, `showTrayIcon`, `minimizeToTray` | the matching `ui.*` keys |
| `general/statWhore`, `statWhoreFormat` (`INI` or else XML), `statWhoreFileName` | `stat_whore.enabled`, `.format`, `.file` |
| `general/useAVDump`, `avdumpPassword` | `avdump.enabled`, `avdump.key` |
| `ui/lastDirectory`, `mainMaximized`, `mainLeft/Top/Width/Height` | `ui.last_directory`, `ui.maximized`, `ui.window` |
| `color` elements (tag, value) | `ui.colors` |
| `player` records (`name` required) | `players[]` (`name`, `path`, `open`, `play`, `enqueue`, `default`) |
| `filemanager/actionDelay` | `file_managers.action_delay_ms` (with the fixup) |
| `filemanager/sleepTime` (AOM's pause between scans) | `file_managers.scan_interval_minutes` in whole minutes (with the fixup); `scheduled_scan` stays off |
| `manager` records (`name` required) | `file_managers.managers[]` (all rule keys; absent booleans become false except `includesubdirs` and `caneditpath`, which become true; absent `myliststate` becomes 0) |

Not touched by the import (they keep fresh-file defaults): `anidb.client_name`, `client_version`,
`api_key`, `udp_server`, `local_port`, `idle_logout_minutes`, `hashing.identify_after_hashing`,
`avdump.executable`, `avdump.arguments`, `ui.theme`, `ui.zoom`, `ui.columns`,
`ui.colored_rows`, and `ed2k_space_replacement`. After an import you therefore still have to
enter a registered client name and review every managed folder before going online or scanning.
Carriage returns are normalized in imported templates and patterns.
