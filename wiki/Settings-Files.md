How `settings.json` is loaded, saved, backed up and recovered, what it holds in plain text, a
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
| `file_managers.action_delay_ms` < 1 | set to 1 |
| `file_managers.sleep_time_ms` < 60000 | set to 60000 |
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

`Settings::save` serializes to pretty-printed JSON and then:

1. Creates parent folders if needed.
2. Writes the new content to a sibling temporary file `settings.json.tmp` and flushes it to disk.
3. If the existing primary file still parses as a settings object, tries to copy it to
   `settings.json.bak.tmp` and rename that over `settings.json.bak`. An invalid existing primary
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
and holds the previous valid settings, including whatever credentials they contained.

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
`offline: true`, over the primary; untick the offline option afterwards if you want to go online.
Step-by-step recovery and support guidance: [Troubleshooting](Troubleshooting.md).

## Credentials are stored in plain text

`anidb.password`, `anidb.api_key` and `avdump.key` are written to `settings.json` as ordinary JSON
strings. Animatic does not encrypt them or use an operating-system credential store. Copies also
exist in `settings.json.bak` (see below) and, briefly, in the `.tmp` file during a save. Consequences:

- Anyone who can read your profile folder (or a backup of it) can read them.
- **The backup can hold a different secret than the current file.** A save copies the *previous*
  valid primary into `settings.json.bak`, so after you change or remove a credential the backup
  keeps the old value until a later save replaces it (and after a first save there may be no
  backup yet). Removing a secret from `settings.json` does not remove it from `settings.json.bak`;
  check and delete both files before sharing or discarding a configuration.
- Do not copy settings files into bug reports, forum posts, repositories or screenshots. If you
  need to share a configuration, remove the three keys first. The repository's `.gitignore`
  excludes `settings.json` but not `settings.json.bak`, so be careful with portable setups kept
  inside a checkout.
- The Options password field is masked, but the API key fields are not.
- An AOM settings import copies the AOM username, password and AVDump password verbatim and saves
  them.
- Animatic does not log credentials in its error messages for malformed settings files.

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
| `filemanager/actionDelay`, `sleepTime` | `file_managers.action_delay_ms`, `.sleep_time_ms` (with the fixups) |
| `manager` records (`name` required) | `file_managers.managers[]` (all rule keys; absent booleans become false except `includesubdirs` and `caneditpath`, which become true; absent `myliststate` becomes 0) |

Not touched by the import (they keep fresh-file defaults): `anidb.client_name`, `client_version`,
`api_key`, `udp_server`, `local_port`, `idle_logout_minutes`, `hashing.identify_after_hashing`,
`avdump.executable`, `avdump.arguments`, `ui.theme`, `ui.zoom`, `ui.columns`,
`ui.colored_rows`, and `ed2k_space_replacement`. After an import you therefore still have to
enter a registered client name and review every managed folder before going online or scanning.
Carriage returns are normalized in imported templates and patterns.
