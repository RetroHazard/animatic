Open with **Tools → Options…**. It is one scrolling window; sections in order: AniDB account,
Appearance, Hashing, Managed folders, Statistics export, External player, AVDump, Renaming pattern,
Updates, and a read-only list of MyList state names. **Save options** at the bottom writes the file, applies
theme and scale and closes the window.

**Edits are live.** Controls change the in-memory settings as you type. Closing the window with
its title-bar button does not revert them: the edited values stay in effect (for example for the next
queue run) and are written when the application exits normally. Only **Save options** writes
immediately. Exceptions: when settings were **recovered from a backup**, exit and the Overview
dismiss button do not write; only **Save options** or an applied import do (a recovery notice says so).
Theme and scale are applied on **Save options** or an applied import; at launch the window
is always dark at normal scale (`Theme: System` never changes the look).

| Section | Controls | Status |
|---|---|---|
| AniDB account | Username, Password (masked), API key (shown in clear), Client name | Used by sync and AVDump |
| | Work offline | Active |
| | Update known file information | Shown disabled ("Not active yet"); value kept |
| Appearance | Theme (Dark/Light/System), UI scale 0.75–2.0 | Dark/Light and scale apply on Save; System does nothing |
| | Use status colors | Shown disabled ("Not active yet"); value kept, no row coloring exists |
| Hashing | Start hashing automatically | Shown disabled ("Not active yet"); value kept |
| | Identify files with AniDB after hashing | Active |
| | Restrict file extensions + list | The picker filter; the list also always drives [scans](Managed-Folders-and-Scans.md) |
| | Hash export template, Export %length% in minutes | Active for [hash export](Exports.md#hash-export) |
| Managed folders | Per-rule fields, Add folder rule, Remove folder rule (user-created rules only) | See [scans](Managed-Folders-and-Scans.md); *Auto rename* is shown disabled ("Not active yet") |
| Statistics export | Enable, Format, Output file | Active |
| External player | Executable, Play arguments, Default (*Name* is shown disabled, "Not active yet") | Active; open/enqueue have no UI |
| AVDump | Enable, Executable, Arguments, API key | Active |
| Renaming pattern | Editor, Use stable default, Restore previous text, Validate on selected file | Active |
| Updates | Check for updates at startup (`updates.auto_check`, default on) | Active; see [Updating Animatic](Updating-Animatic.md) |

Not in Options (JSON only): client version, UDP server, local port, image server, idle logout,
tray, window and column state, row colors, `ed2k_space_replacement`, folder-scan delays.
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
