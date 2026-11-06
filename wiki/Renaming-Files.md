Renaming is **manual and per file**. There is no automatic renaming of any kind.

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

On **Rename file** Animatic checks that the destination does not exist and then asks the operating
system to rename the file. The check and the rename are two separate steps, not one atomic
operation: if another program creates a file at the destination in between, the rename can **overwrite**
it (on Linux the system call replaces an existing file; Rust's rename also replaces existing files
on Windows). Only after the move succeeds is the stored path updated; a failure
reports `Rename failed: …`, leaves the record unchanged and may leave newly created empty
folders. There is no undo, no AniDB notification, and renaming across drives/filesystems is not
specially handled [behavior of the OS rename; not tested here].

Pattern tools in Options (nothing is saved until **Save options**, but see
[Options window](Options-Window.md)): *Use stable default* (replace the text; the earlier text is
kept so *Restore previous text* can undo it), *Validate on selected
file* (dry run on the file selected on Hashed files; shows the result, diagnostics and `The pattern is
valid for this file.`) and live syntax diagnostics. The built-in default uses a fixed two-digit
episode minimum (`01`, `02`, … `100`); an existing custom pattern is never changed unless you
choose *Use stable default*.
