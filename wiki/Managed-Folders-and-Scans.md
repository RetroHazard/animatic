A **managed folder rule** (Options → Managed folders) combines a folder, scan behavior and
MyList policy. **Scans are manual-only**: run **Tools → Scan managed folders**. There is no
scheduler, tray icon or background watcher; `action_delay_ms` and `sleep_time_ms` are stored
but never read. The scan itself never deletes media files, never renames files and never writes
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
  consulted, and *exclude system files* is stored but unused.
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

Hashes found by a scan follow the normal [identification](Hashing-and-Identification.md) path.
The rule fields `auto_rename`, `clear_empty_folders` and `exclude_system` have no effect; the
**Auto rename** checkbox exists but renaming is only manual
([Manual renaming](Renaming-Files.md)).

**Which rule applies to a file** (`FileManagerSettings::manager_for`): rules with a non-empty
path are candidates; one applies if the file is below its folder (any depth when *Subfolders*
is on, direct children only otherwise); the longest matching path wins; no match falls back to the
rule marked default (else the first). The fallback supplies the MyList policy for identify-after-hash
and the rename destination, but never authorizes removal.

**MyList policy fields** (apply to newly added files): each of *Set watched*, *Set state*, *Set
storage*, *Set source*, *Set other* is a checkbox. Unchecked = leave that field unchanged;
checked with an empty text = explicitly clear it. **Rename destination** is the base folder
for [manual renames](Renaming-Files.md); empty keeps files where they are.
