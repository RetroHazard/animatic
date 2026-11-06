Open with **Tools → Options…**. It is one scrolling window; sections in order: AniDB account,
Appearance, System tray, Hashing, Managed folders, Statistics export, External player, AVDump, Renaming pattern,
Updates, and a read-only list of MyList state names. **Save options** at the bottom writes the file, applies
theme and scale and closes the window.

**Edits are live.** Controls change the in-memory settings as you type. Closing the window with
its title-bar button does not revert them: the edited values stay in effect (for example for the next
queue run) and are written when the application exits normally. Only **Save options** writes
immediately. Exceptions: when settings were **recovered from a backup**, exit and the Overview
dismiss button do not write; only **Save options** or an applied import do (a recovery notice says so).
Theme and scale are applied at launch, on **Save options** and on an applied import (`Theme: System`
follows the operating system's light or dark mode). The window's size, position and maximized state
and the table column layouts are not Options controls: they are kept as you use the window and
written with the settings.

| Section | Controls | Status |
|---|---|---|
| AniDB account | Username, Password (masked), API key (shown in clear), Client name, Client version | Used by sync and AVDump. *Client version* is the version registered with AniDB for your client name: a whole number of 1 or more, shown red with an explanation (and **Save options** disabled) while the text is not one. Animatic updates never change it |
| | Work offline | Active |
| | Download anime covers when a detail window opens | Active (default on); see [Cover](Library-Pages.md#cover) |
| | Update known file information | Active: re-hashing an identified file queues a refresh of its AniDB record (at most every 7 days per file); off by default |
| Appearance | Theme (Dark/Light/System), UI scale 0.75–2.0 | Active; applied at launch and on Save |
| | Use status colors | Active: tints table rows by status (CRC mismatch, unidentified, watched, in MyList); each status is also written in the row. Applies immediately. See [Main window](Main-Window.md#sorting-columns-and-row-colors) |
| System tray | Show tray icon, Minimize to tray | Active. Both are disabled, with a yellow explanation, where the system has no tray (see [System tray](Main-Window.md#system-tray)) |
| Hashing | Start hashing automatically | Shown disabled ("Not active yet"); value kept |
| | Identify files with AniDB after hashing | Active |
| | Add "Hash with Animatic" to the file menu | Active, off by default; applied by **Save options**, see [File-menu entry](#file-menu-entry) |
| | Restrict file extensions + list | The picker filter; the list also always drives [scans](Managed-Folders-and-Scans.md) |
| | Hash export template, Export %length% in minutes | Active for [hash export](Exports.md#hash-export) |
| Managed folders | Per-rule fields (including *Skip system files* and *Remove emptied folders*), Add folder rule, Remove folder rule (user-created rules only) | See [scans](Managed-Folders-and-Scans.md); *Skip system files* applies on Windows only; *Remove emptied folders* acts after a [rename](Managed-Folders-and-Scans.md#removing-folders-a-rename-emptied); *Auto rename* is shown disabled ("Not active yet") |
| | Folder scanning: scan at start, scan on a schedule every N minutes (5–10080), hash a file once unchanged for N ms, pause between hashed files | Active; see [scheduled scans](Managed-Folders-and-Scans.md#scheduled-scans) and [settle delay and pacing](Managed-Folders-and-Scans.md#settle-delay-and-pacing). Scan settings reach the background timer when Options is closed, not while you edit |
| Statistics export | Enable, Format, Output file | Active |
| External player | Per player: Name, Executable, Open/Play/Enqueue arguments, Default player, Remove player; Add player | Active; see [External tools](External-Tools.md) |
| AVDump | Enable, Executable, Arguments, API key | Active |
| Renaming pattern | Editor, Use stable default, Restore previous text, Validate on selected file | Active |
| Updates | Check for updates at startup (`updates.auto_check`, default on) | Active; see [Updating Animatic](Updating-Animatic.md) |

Not in Options (JSON only): UDP server, local port, image server (the fallback host for covers), idle logout,
row colors, `ed2k_space_replacement`.
The full list with active/stored-only status is the [settings reference](Settings-Reference.md).
The password, API key and AVDump key are saved to the system keystore (Windows Credential Manager,
Linux Secret Service) when one is available, and then are not in `settings.json` or its backup. When
none is, a yellow line under the AniDB account section says so ("Credentials are stored in the
settings file because no system keystore is available: …") and the values stay in the file in plain
text. See [Credentials and the system keystore](Settings-Files.md#credentials-and-the-system-keystore).

If a **"Settings recovered"** window appears at startup, the primary file was missing or invalid and
the backup was loaded **offline**; the original file was not touched. Review the values, then
**Save options** to replace it. See the [recovery matrix](Settings-Files.md#recovery-matrix) and
[troubleshooting](Troubleshooting.md).

## File-menu entry

**Add "Hash with Animatic" to the file menu** (`hashing.integrate_into_shell`) puts a per-user entry
in your system's file menu so you can send files to Animatic from the file manager. It is off by
default and needs no administrator rights.

- **Only Save options changes the system.** Ticking the box does nothing yet. **Save options**
  registers the entry when the box is ticked and removes it when it is not. Closing the window with its
  title-bar button, quitting, loading settings and **importing AOM settings** never register or remove
  anything; an imported `integrateIntoShell = true` is only stored, and Options then reads
  "Not registered yet; Save options to add it."
- **Options shows the real state** under the checkbox, read from the system each time the window
  opens: registered, not registered, registered for another copy of Animatic (for example after you
  moved the program; saving with the box ticked points the entry at this copy), or an entry with the
  same name that Animatic did not create. The last one is never changed or removed.
- **Windows.** The entry is added to the current user's registry classes
  (`HKEY_CURRENT_USER\Software\Classes\*\shell\Animatic.Hash`) as **Hash with Animatic** on the context
  menu of files (on Windows 11 under **Show more options**). It runs `"<animatic.exe>" "%1"`. Removing
  it deletes that key and any parent key that is left empty; other entries are untouched.
- **Linux.** The entry is the file `animatic-hash.desktop` in `$XDG_DATA_HOME/applications` (default
  `~/.local/share/applications`), hidden from application menus (`NoDisplay=true`) and offered for
  common video and audio types, so it appears under **Open With → Hash with Animatic**. It runs
  `animatic %F`. `update-desktop-database` is run when it is installed; the entry also works without it.
- **Only files, only the program's path.** The command passes the clicked files and nothing else, so
  it opens the default database (or `ANIMATIC_DB`/`ANIMATIC_SETTINGS` if those are set in the
  environment the file manager runs in). Folders are not offered because Animatic does not expand
  folders; use [Managed folders](Managed-Folders-and-Scans.md).
- **Several files at once.** Windows starts one process per selected file; they hand their files to
  the one running window ([One window per database](Running-Animatic.md#one-window-per-database)).
- **Ownership.** Animatic marks its entry (registry value `AnimaticManaged`, line
  `X-Animatic-Managed=true` in the desktop file) and removes only entries that carry the mark. See
  [Data locations](Data-Locations.md#shell-entries-and-the-lock-file).
