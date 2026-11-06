Renaming is **manual and always confirmed**: one file, or several files at once with a single
confirmation. The only exception is the opt-in
[automatic rename of a managed folder](#automatic-rename-opt-in-per-managed-folder), which is
off unless you switch it on for that folder.

## One file

1. Open **Hashed files** and select a file that AniDB has identified (`AniDB` column shows `fid`).
   Unidentified files report `The selected file is not identified by AniDB`.
2. Optionally edit or validate the pattern in **Tools → Options… → Renaming pattern**
   (see [Options window](Options-Window.md)). The language is documented in the [rename language](Rename-Language.md).
3. Click **Rename…**. Animatic evaluates the pattern against the local database (no network, no
   file access) and opens **Confirm rename** with the current path, the proposed new path, and any
   errors (red) or warnings (yellow).
4. **Rename file** is enabled only when a new path exists and there are no errors and no
   warnings. Otherwise you can click **Edit pattern in Options** (closes the preview and opens
   Options).

## Several files (batch rename)

1. On **Hashed files** select the files you want to rename: click one, Ctrl+click to add or remove
   files, Shift+click to select a range. **Select all shown** selects every listed file, so it
   follows the search box; **Clear selection** clears the selection. It is the same selection
   that **Add to MyList…** uses, and a file that a new search hides drops out of it.
2. Click **Rename N selected…**. The window **Batch rename** opens at once and shows
   `Planning the renames of N files…` while the plans are made in the background (the interface
   stays responsive however many files are selected).
3. The preview lists one line per file, in table order, with a status:

   | Status | Meaning |
   |---|---|
   | `Ready` (green) | The pattern gives a valid new path: `<file name> → <new path>` |
   | `Blocked` (red) | The first problem is shown after the file name; hover the line for the full paths and every error and warning |
   | `Already named` | The file already has the name the pattern produces |

   Besides the problems a single rename reports, a line is blocked when its target path **already
   exists** on disk, or when an earlier line of the batch would get the same name (the first one
   keeps it) and when the file is not identified by AniDB.
4. **Rename N files** is the one explicit confirmation. Only the `Ready` lines are renamed;
   blocked lines are skipped and nothing about them changes. **Edit pattern in Options** closes the
   preview and opens Options.
5. The window then shows the outcome per file (`Renamed`, `Renamed (copied and verified)` or
   `Not renamed: <reason>`) and a summary. The same lines go to the **Log**. **Cancel** in the
   status bar stops before the next file (and during a copy); files not reached are reported as
   `Skipped: cancelled`.

## What a plan blocks

A plan is blocked when: the pattern is empty or has a syntax error; the evaluation fails; it ends
with a true/false value instead of text; it yields an empty name or a name with an unusable part
(characters `< > : " | ? *`, control characters, empty folder names, reserved device names
such as `CON`/`NUL`, or a component over 255 characters – UTF-16 units on Windows, bytes
elsewhere); any evaluation warning occurred. The **stable default** pattern additionally requires the
anime title and a positive episode number to be known locally, so fetch the anime ("Refresh from
AniDB" and run the queue) first. If the file already has the produced name, the dialog says
nothing needs renaming.

The destination is the matching folder rule's **Rename destination** joined with the pattern's result; a
plain name stays in the file's current folder, while path separators in the result create or use
subfolders (missing parents are created).

## What applying does

Applying runs on the background worker, so the window stays usable and the status bar shows
progress with **Cancel**. For every file Animatic checks that the library still records the old
path and that the destination does not exist, then:

- **Same drive or filesystem:** the file is renamed in place and the stored path updated. If the
  update fails the rename is undone.
- **Another drive or filesystem** (the operating system refuses a plain rename): the file is
  **copied** to a hidden temporary file (`.animatic-move-*.tmp`) beside the destination, the
  copy is **verified** — its size, then its ED2K hash, which reads the whole copy once more and
  is compared with the hash stored when the file was hashed — and only then moved into place,
  recorded in the library and finally removed from the old location. This takes about twice the
  time a copy alone would, which is deliberate: the extra read is the proof that the copy is the
  file your library knows. The file's modification time is kept so a scan does not rehash it.
  The file must have a stored hash and must not have changed since it was hashed; otherwise the
  move is refused with a message asking you to hash it again.

Any failure or **Cancel** while copying or verifying removes the temporary file and leaves the
original and its library entry untouched: the library never points at a missing file and the
only copy is never deleted. If the verified copy is in place and recorded but the old file cannot
be removed (for example a player holds it open), the rename counts as done and the log tells you
that the old file remains as an extra copy you can delete yourself.

The check for an existing destination and a same-drive rename are two separate steps, not one
atomic operation: if another program creates a file at the destination in between, the rename
can **overwrite** it (on Linux the system call replaces an existing file; Rust's rename also
replaces existing files on Windows). A move to another drive never replaces a file that appears
in between. A failure reports `Rename failed: <file>: <reason> (target <path>)` in the Log and may leave newly
created empty folders. There is no undo and no AniDB notification. If the matching managed folder
has **Remove emptied folders** on, the folders a successful rename left empty are removed
afterwards ([details](Managed-Folders-and-Scans.md#removing-folders-a-rename-emptied)).

Pattern tools in Options (nothing is saved until **Save options**, but see
[Options window](Options-Window.md)): *Use stable default* (replace the text; the earlier text is
kept so *Restore previous text* can undo it), *Validate on selected
file* (dry run on the file selected on Hashed files; shows the result, diagnostics and `The pattern is
valid for this file.`) and live syntax diagnostics. The built-in default uses a fixed two-digit
episode minimum (`01`, `02`, … `100`); an existing custom pattern is never changed unless you
choose *Use stable default*.

## Automatic rename (opt-in per managed folder)

Each managed folder rule has an **Auto rename** setting with three modes (**Tools → Options… →
Managed folders**, see [Options window](Options-Window.md)):

| Mode | What happens |
|---|---|
| **Off** (default) | Nothing is renamed automatically. |
| **Dry run (log only)** | The renames that *would* happen are written to the **Log** (`Automatic rename (dry run): <old> would be renamed to <new>`); no file is moved. |
| **On** | Identified files in that folder are renamed with the current pattern, through the same worker job, verified cross-drive move and **Cancel** as a manual rename. |

**Turning a folder On is confirmed.** Choosing **On** opens a window that lists, as a read-only
dry run, what would be renamed *now* in that folder (the same lines as **Batch rename**: `Ready`,
`Blocked`, `Already named`) and says that nothing has been renamed. Only **Turn on automatic
rename** switches the folder to On; **Cancel** (or Esc) leaves the mode unchanged. The choice is
kept when you **Save options**. The window appears every time a folder changes to On, not only the
first time, so a preview is always seen before any automatic move. A stored or AOM-imported
`auto_rename` flag that was simply *true* loads as **Dry run**, never On; see
[Settings reference](Settings-Reference.md#per-folder-keys).

**When it runs.** A pass is made when something may have made a file renameable: a file was
hashed (a [scan](Managed-Folders-and-Scans.md) found it, including a file the library already
knows as identified), an AniDB reply changed the library (the identify reply of a queued file was
applied), an AOM import finished, or the folder modes, paths or pattern changed — and once at
start-up. It runs only while no other task is running, and not while an import, a restore, a
settings review or the settings recovery notice is open, or while the Options window is open.
*Work offline* does not matter: planning and moving are local.

**What it renames.** Only a file that

- lies inside a managed folder with a non-empty path (the built-in **Default** rule, which
  matches everything else, never renames; the folder with the longest matching path decides, so
  a subfolder rule set to Off excludes its files),
- AniDB has identified **and** whose anime and episode records are cached (until then it is
  left alone without a note; the reply that completes it triggers a new pass),
- still exists on disk, and
- has a plan that is valid for **Apply** with the current pattern: a pattern that does not
  compile pauses the whole pass with one `Automatic rename is paused` log line; warnings, a
  missing title or episode number (stable default), an empty or true/false result or an unusable
  name block the file.

**Collisions and blocked plans are skipped and logged, never overwritten.** A file whose target
already exists, or that would get the same name as another file of the same pass, is skipped with
`Automatic rename skipped <path>: <reason>`. Each outcome is logged **once per file and state**: a
scan or queue run that changes nothing neither repeats a note nor retries a failed move. A file
is planned again when its path, its metadata, its folder mode or the pattern changes (or after
Animatic restarts); a rename stopped by **Cancel** is tried again by the next pass. At most 20
notes are logged per pass, followed by a count of the rest. As with a manual rename, the
existing-target check and a same-drive rename are separate steps; the [limit described
above](#what-applying-does) applies.

Moves are reported like manual ones (`Renamed <old> to <new>`, `Rename failed: …`) followed by
`Automatic rename finished: N renamed, M not renamed`. There is no undo and no AniDB notification.
