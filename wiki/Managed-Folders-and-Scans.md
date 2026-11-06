A **managed folder rule** (Options → Managed folders) combines a folder, scan behavior and
MyList policy. Scans start from **Tools → Scan managed folders**, optionally also
[when Animatic starts and on a schedule](#scheduled-scans); there is no tray icon or filesystem
watcher. The scan itself never deletes media files, never renames files and never writes
MyList; it can hash files and forget missing local records. Hashing is not the end of it, though:
every file the scan hashes raises the same event as any other hashing, so with *Identify files with
AniDB after hashing* on and the folder rule's **Add to mylist** on, an Identify command with an add
policy is queued, and **running the queue later adds the file to your AniDB MyList**.

Planning (`file_manager::plan_scan`), per rule in list order:

- A rule is walked only if its **Path** is a directory that exists *and* **Auto hash** or
  **Rehash changed** is on.
- **Subfolders** on = unlimited depth; off = direct children only. Symbolic links are not followed.
- With **exclude_hidden** (the default for new rules; not shown in Options), entries whose name
  starts with `.` are skipped, directories included. The Windows hidden attribute is not
  consulted.
- With **Skip system files** (`exclude_system`, on for new rules), files and folders that carry the
  Windows *system* attribute are skipped; a skipped folder is not entered. Linux has no such
  attribute, so there the option changes nothing. The attribute is read from the entry itself, never
  from the target of a link.
- Only files whose extension is in the **extension list** are considered. This list is always
  applied during a scan, even when *Restrict file extensions* is unchecked (that checkbox only
  filters the file picker). An empty list means a scan finds nothing. Matching is
  case-insensitive; the list is comma-separated in Options.
- A file whose path is not in the database is hashed (**Auto hash**). A known path whose
  modified time differs from the stored one is re-hashed (**Rehash changed**).
- Discovery is **first rule wins** in the order of the rules list: each discovered path is
  recorded once, so a file inside two overlapping rules is planned under the earlier one. (Longest-path
  matching applies only to the post-hash MyList policy, the rename destination and *Remove
  missing*, described below.) Windows compares paths case-insensitively and with `\` and `/`
  unified; Linux compares case-sensitively.

**Remove missing** is evaluated independently of walking. A stored path is removed from the
local inventory only when **all** hold: the path is under an explicit rule that has *Remove
missing* on (subfolders count only for rules with *Subfolders*; the unmatched Default rule never
authorizes removal), the filesystem reports the file **not found** (any other error, such as an
unavailable drive or permission failure, keeps the record), and the rule's own folder exists as a
directory. It then removes only the local record (and a hash record nothing else uses). Media
files and MyList entries are untouched.

## Scheduled scans

Options → Managed folders → *Folder scanning* has three controls, all off by default:

- **Scan managed folders when Animatic starts** runs one scan right after the application opens.
- **Scan managed folders on a schedule** with **Scan every N minutes** (5 minutes to 10080, one
  week; the default is 60) runs a scan every N minutes, counted from the previous scheduled scan's
  start. Changing the setting restarts the count.
- They are saved as `scan_at_start`, `scheduled_scan` and `scan_interval_minutes` in the
  [settings file](Settings-Reference.md#managed-folders-file_managers). A change reaches the
  background timer when Options is closed, not while you edit.

A timer on the background worker submits the same scan as the menu item, so everything above
applies. The UI thread never waits for it. The timer polls once a second; it uses no filesystem
watcher, which keeps it identical on Windows and Linux and on network drives.

A scheduled scan **never overlaps other work**. It waits (and is retried every second, with one
log line "Scheduled folder scan postponed") while any background task or manual scan is running,
an import or its confirmation is open, imported settings are waiting for review, the settings
recovery notice is open, a library restore is running, or queue commands are not saved to the
database yet. It then starts, and the next interval counts from that start. **Work offline** does
not matter to a scan: scans never contact AniDB. The queued `Identify` commands described above
still wait for you to run the queue, offline or not. Closing Animatic cancels a running scan at
the next file.

## Settle delay and pacing

A file still being copied or downloaded must not be hashed half-written. A discovered file is
hashed only when both hold:

1. its modification time is at least **Hash a file once unchanged for** (`action_delay_ms`,
   default 500 ms, 0 to 600000) in the past, and
2. its size and modification time did not change during one further wait of that length, which
   the scan observes once for all candidate files.

The first rule needs no earlier scan; the second catches a copy that keeps the source file's old
modification time while it grows. A file that fails either rule is **deferred**: it is not hashed
and not recorded, the log lists it ("Scan deferred N file(s) that may still be written"), and the
next scan looks at it again. With a delay of 0 nothing is held back. A scan with candidate files
therefore takes at least the delay.

**Pause between hashed files** (`sleep_time_ms`, default 0, 0 to 10000 ms) waits between two
hashed files of one scan, so a large folder does not keep the disk busy continuously; there is no
pause before the first or after the last file. Cancelling interrupts both waits. Earlier versions
stored `sleep_time_ms` as a pause between scans (default 900000); a stored value above 10000 is
reset to 0 on load, and the scan interval above replaces the old meaning.

Hashes found by a scan follow the normal [identification](Hashing-and-Identification.md) path.
The rule field `auto_rename` has no effect; the **Auto rename** checkbox exists but renaming is
only manual ([Manual renaming](Renaming-Files.md)).

**Which rule applies to a file** (`FileManagerSettings::manager_for`): rules with a non-empty
path are candidates; one applies if the file is below its folder (any depth when *Subfolders*
is on, direct children only otherwise); the longest matching path wins; no match falls back to the
rule marked default (else the first). The fallback supplies the MyList policy for identify-after-hash
and the rename destination, but never authorizes removal.

**MyList policy fields** (apply to newly added files): each of *Set watched*, *Set state*, *Set
storage*, *Set source*, *Set other* is a checkbox. Unchecked = leave that field unchanged;
checked with an empty text = explicitly clear it. **Rename destination** is the base folder
for [manual renames](Renaming-Files.md); empty keeps files where they are.

## Removing folders a rename emptied

**Remove emptied folders** (`clear_empty_folders`, off by default) is the only place Animatic
deletes a folder, and it is not part of a scan. After a [manual rename](Renaming-Files.md) (single
or batch) has moved files, each folder a moved file left is removed when **all** hold:

- the old path is under an explicit rule with the option on (the same longest-path matching as
  *Remove missing*; a rule without *Subfolders* only covers its own folder, which is never removed),
  and the rule's folder exists as a directory;
- the folder is **strictly below** the rule's folder; the rule's own folder is never removed;
- the folder is **completely empty**: any file, folder or hidden file keeps it. Nothing is deleted
  recursively and links are not followed or removed.

Parents are then checked in turn, upward, under the same conditions. Only folders that one of the
files moved by that job came from are considered; an empty folder somewhere else is left alone,
and so is a folder whose file could not be moved (or whose old copy remains after a cross-drive
copy). The Log notes "Removed N empty folders"; a folder that cannot be removed is only written to
the diagnostic log, and the rename itself is not affected.
