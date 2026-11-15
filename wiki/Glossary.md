This page explains the words you meet in Animatic, in AniDB and in this guide, in alphabetical order.
Each entry says what the term means and links to the page that explains it. Menu paths and button
names are written as they appear on screen. If a word is missing, search the wiki or see the
[FAQ](FAQ.md).

- **Add queued**: what the **MyList** column of **Hashed files** shows for a file that is not in MyList
  yet but has an add command waiting in the command queue. See
  [AniDB synchronization](AniDB-Synchronization.md#mylist-editing).
- **aid**: the AniDB *anime id*, the number AniDB gives an anime. The detail window shows it as
  `AniDB <aid>`, and **Advanced search** accepts it in **Title or AniDB ID**. See
  [Library pages](Library-Pages.md#anime-detail-window).
- **AniDB**: the anime database at <https://anidb.net> that Animatic identifies your files against and
  whose MyList it edits. Animatic is not affiliated with AniDB's operators and sends only what the
  command queue holds. See [AniDB synchronization](AniDB-Synchronization.md).
- **AniDB client**: the name and version a program announces to AniDB. Animatic has two built in,
  `animatic` v1 (UDP API) and `animatichttp` v1 (HTTP API); you never enter or change them. See
  [AniDB client registration](First-Launch.md#anidb-client-registration).
- **Anime detail window**: the window that opens when you click an anime's title. It shows the cover,
  description, episodes, titles, genres, relations and reviews and has **Open on AniDB** and
  **Refresh from AniDB**. See [Library pages](Library-Pages.md#anime-detail-window).
- **Anime titles list** (title dump): AniDB's public list of every anime title. **Tools → Download
  anime titles** and **Tools → Full update…** queue its download (at most once per 24 hours); it
  creates a row for every anime id it does not know, so afterwards the **Anime** page and the sidebar
  count include every anime on AniDB (tens of thousands), not only the anime in your library. See
  [Request limits](AniDB-Synchronization.md#request-limits).
- **AOM** (AniDB O'Matic): the older Windows program that Animatic succeeds. Animatic can import an
  AOM databank and AOM settings without changing the originals. See
  [Migrating from AOM](Migrating-from-AOM.md).
- **API key**: an optional key from your AniDB profile. When set in **Options → AniDB account**, it
  turns on encrypted UDP sessions. It is stored in the system keystore when one works. See
  [First launch](First-Launch.md#anidb-client-registration).
- **Auto rename**: a per-folder mode (**Off**, **Dry run (log only)**, **On**) that renames identified
  files without asking, through the same verified rename as a manual one. Off by default and never
  implicit. See [Renaming files](Renaming-Files.md#automatic-rename-opt-in-per-managed-folder).
- **AVDump**: an external AniDB tool that Animatic can start on a selected file when you enable it in
  **Options → Players and tools**. See [External tools](External-Tools.md).
- **Backup** (library backup): **File → Back up library…** writes a single consistent copy of the
  library database. A *settings backup* is something else: the `.bak` copy of `settings.json` that a
  save keeps. See [Backups and recovery](Backups-and-Recovery.md).
- **Ban**: AniDB refusing a client for sending too many requests. Animatic reacts to a ban reply with a
  cooldown (see **Cooldown**) and stops the run. See
  [Cooldowns and bans](Network-and-Sync-Problems.md#cooldowns-and-bans).
- **Batch rename**: renaming several selected hashed files with one confirmation (**Rename N
  selected…**). Blocked files are listed and skipped. See
  [Renaming files](Renaming-Files.md#several-files-batch-rename).
- **Blocked** (command): a queued command that failed permanently (for example a wrong password) and
  will not be sent until you fix the cause and use **Retry failed**. The sidebar shows a red `N blocked`.
  See [Failure handling](AniDB-Synchronization.md#failure-handling).
- **Command line** (hash queue source): the label the hash queue panel shows for files that were given
  on Animatic's command line; other sources are `Add files`, `Scan` and `Other launch`. See
  [Running Animatic](Running-Animatic.md#command-line-reference).
- **Command queue**: the list of things waiting to be sent to AniDB (identify a file, edit a MyList
  entry, refresh an anime, …), saved in the library so it survives restarts. It is shown on the
  **Queue** page and is sent only by **Run command queue**. See
  [AniDB synchronization](AniDB-Synchronization.md#synchronization).
- **Confirm rename**: the window that shows the current path, the new path and any problems for one
  file before **Rename file** moves it. See [Renaming files](Renaming-Files.md#rename-one-file).
- **Cooldown**: a pause of all AniDB requests that Animatic saves after a ban or a network failure. It
  survives restarts, **Run command queue** is disabled meanwhile, and nothing can clear it early. See
  [Cooldowns and bans](Network-and-Sync-Problems.md#cooldowns-and-bans).
- **Cover**: the picture of an anime, downloaded once from AniDB's image server when you open its detail
  window (if downloads are on) and kept in the `covers` folder. See [Cover pictures](Data-Locations.md#cover-pictures).
- **CRC mismatch**: the **CRC** column of **Hashed files** reads `Mismatch` when AniDB knows a
  different CRC32 for the identified file than Animatic computed (a damaged or altered copy). The
  Overview counts these under **Needs attention**. See [Library pages](Library-Pages.md#hashed-files).
- **CRC32**: a short checksum of a file's content, one of the hashes Animatic computes. AniDB stores
  it for every known file. Written as eight hexadecimal digits; many release names end in
  `[CRC32]`. See [Hashing and identification](Hashing-and-Identification.md).
- **Credentials**: your AniDB password, API key and AVDump key. They are kept in the Windows Credential
  Manager or the Linux Secret Service, not in `settings.json`, while the system keystore works. See
  [Settings files](Settings-Files.md#credentials-and-the-system-keystore).
- **Custom pattern**: a rename pattern you write yourself instead of a built-in naming convention. See
  [Rename patterns](Rename-Patterns.md) and [Rename language](Rename-Language.md).
- **Database**: `animatic.sqlite3`, the file that holds your library, hashes and command queue. See
  [Data locations](Data-Locations.md#where-data-lives).
- **Deferred** (command): a command that could not be saved to the database yet because the database
  was busy. The Overview says `N commands not saved yet; Animatic keeps retrying`. See
  [Network and sync problems](Network-and-Sync-Problems.md#queue-states-blocked-deferred-and-unsaved).
- **Dry run**: show what would happen and change nothing. The **Dry run (log only)** automatic-rename
  mode writes the planned renames to the **Log**, and the confirmation for turning automatic rename
  **On** shows a dry run first. See [Renaming files](Renaming-Files.md#automatic-rename-opt-in-per-managed-folder).
- **ed2k** (ED2K hash): the file fingerprint AniDB identifies files by, together with the file size.
  Animatic computes the AniDB-compatible "red" variant. See
  [Hashing and identification](Hashing-and-Identification.md).
- **eid**: the AniDB *episode id*.
- **Episode number**: the number of an episode within its anime. Regular episodes are plain numbers
  (`01`); other kinds carry a letter prefix: `S` special, `C` credit, `T` trailer, `P` parody, `O` other.
  See [Library pages](Library-Pages.md).
- **ffprobe**: the external program Animatic can use, only if you enable it, to read media properties
  (duration, resolution, codecs) after hashing. See [External tools](External-Tools.md#media-properties-ffprobe).
- **fid**: the AniDB *file id*. **Hashed files** shows `fid <id>` in the **AniDB** column for an
  identified file. See [Library pages](Library-Pages.md#hashed-files).
- **File-menu entry**: the optional **Add "Hash with Animatic" to the file menu** (**Options → Hashing**),
  which adds a per-user entry to your file manager's context menu. See
  [File-menu entry](Options-Window.md#file-menu-entry).
- **Flood protection**: AniDB's rule against too many requests, and Animatic's compliance with it:
  UDP packets at least 2 seconds apart for the first five of a burst and 4 seconds afterwards, HTTP
  requests 2 seconds apart, and fixed intervals (24 hours for an anime or the title list, 7 days for a
  known file) before the same thing is requested again. See
  [Request limits](AniDB-Synchronization.md#request-limits).
- **Full update**: **Tools → Full update…** (or `--full-update`) queues the title list plus a refresh
  of every identified file and its anime, minus what the limits do not allow yet. Nothing is sent until
  you run the queue. See [AniDB synchronization](AniDB-Synchronization.md#synchronization).
- **Getting started box**: the checklist at the top of the Overview on a new profile, hidden with
  **Hide**. See [First launch](First-Launch.md#the-getting-started-box).
- **gid**: the AniDB *group id*, the id of a release group.
- **Group** (release group): the fansub or release team that made a file, such as Hanabi Subs in the
  sample library. The **Groups** page lists them; the built-in naming convention puts the short name in
  square brackets. See [Library pages](Library-Pages.md#groups).
- **Hash**, **hashing**: reading a file once and computing its ED2K, CRC32, MD5 and SHA-1 fingerprints
  in one pass. Hashing is local; it never contacts AniDB. See
  [Hashing and identification](Hashing-and-Identification.md).
- **Hash new files**: a managed-folder switch (off for a new rule): a scan hashes the files it finds that
  are not yet in the library. See [Managed folders and scans](Managed-Folders-and-Scans.md).
- **Hash queue**: the list of files waiting to be hashed, saved in the library so it survives restarts.
  Failed files stay in it with a reason until you retry or clear them. See
  [Hash queue](Hashing-and-Identification.md#hash-queue).
- **Hashed files**: the page that lists every file Animatic has hashed, with its AniDB, MyList and CRC
  status. See [Library pages](Library-Pages.md#hashed-files).
- **HTTP API**: AniDB's web interface that Animatic uses for full anime records (and, with the image
  server, covers). See [Request limits](AniDB-Synchronization.md#request-limits).
- **Identify**, **identified**: asking AniDB which file a hash belongs to. A file AniDB knows becomes
  *identified* (`fid N`); otherwise the **AniDB** column reads `Not identified`. Identification happens
  only when the queue runs. See [Hashing and identification](Hashing-and-Identification.md).
- **Include subfolders**: a managed-folder switch (on for a new rule): look in all folders below the
  rule's path; off means direct children only. See [Managed folders and scans](Managed-Folders-and-Scans.md).
- **Instance lock** (`animatic.sqlite3.instance`): the small file next to the database that lets a
  second launch hand its files to the running window. See
  [One window per database](Running-Animatic.md#one-window-per-database).
- **Kowai** (databank): the folder inside an AOM installation that holds its encrypted `.data` files.
  See [Migrating from AOM](Migrating-from-AOM.md#migrate-an-aom-databank).
- **lid**: the AniDB *MyList entry id*. **Hashed files** shows `lid <id>` for a file in MyList, and the
  MyList toolbar shows `Entry <lid>` for the selected row. See [Managing MyList](Managing-MyList.md).
- **local #N**: Animatic's own number for a hashed file, shown in the Log line `Hashed <path> (local
  #N, <ed2k>)`. It is not an AniDB id.
- **Log**: the page that lists this session's activity (hashing, renames, AniDB requests, errors). It
  is kept in memory and not written to disk. See [Library pages](Library-Pages.md#log).
- **Managed folder**: a folder rule in **Options → Managed folders** that combines a folder, scan
  behavior, a rename destination and the MyList settings for files found there. Rules are optional;
  a file that no rule covers is only identified, not added to MyList. See
  [Managed folders and scans](Managed-Folders-and-Scans.md).
- **MD5**, **SHA-1**: two more hashes computed in the same pass as the ED2K and CRC32. They are stored
  and available to rename patterns and exports. See [Hashing and identification](Hashing-and-Identification.md).
- **Media properties**: duration, resolution, codecs and bitrates read from a file by ffprobe after
  hashing. Opt-in and off by default. See [External tools](External-Tools.md#media-properties-ffprobe).
- **Move renamed files to another folder**: a managed-folder checkbox. Ticked, it shows a **Destination**
  field (with **Browse…**): the folder the folder rule's renamed files go to. Unticked, or with the
  field empty, a file is renamed in its current folder. See [Rename targets and apply](Rename-Targets-and-Apply.md).
- **MyList**: your personal list of files on AniDB. Animatic keeps a local copy, lets you edit it
  offline and sends the edits through the command queue. See [Managing MyList](Managing-MyList.md) and
  [Library pages](Library-Pages.md#mylist).
- **MyList entry**: one row of your MyList: a file you have, with its state, watched flag, view date,
  storage, source and other text. See [Managing MyList](Managing-MyList.md).
- **MyList export**: AniDB's way of handing you the whole MyList as one file. You request it with
  **Tools → Request MyList export…**, download it from AniDB's web page and import it with **File →
  Import → MyList export…**. See
  [Full-account MyList import](AniDB-Synchronization.md#full-account-mylist-import).
- **MyList state**: where a file physically is: `Unknown`, `On HDD`, `On CD/DVD`, `Deleted` or
  `Remote/Shared`. See [Managing MyList](Managing-MyList.md).
- **Naming convention**: one of the built-in rename patterns offered in **Options → Renaming**, for
  example `[Group] Title - episode [CRC]` (the default). Choose **Custom pattern** to write your own.
  See [Renaming files](Renaming-Files.md#choosing-how-files-are-named) and
  [Built-in naming conventions](Rename-Targets-and-Apply.md#built-in-naming-conventions).
- **Needs attention**: the Overview section that lists what wants action: the hash queue, CRC
  mismatches, unidentified files, blocked and waiting commands. See [Library pages](Library-Pages.md#overview).
- **Online** / **Offline**: the status-bar button that switches **Work offline** (see that entry).
- **Pending**: the status of a queued command that is waiting to be sent (or retried). A command that
  fails temporarily shows `N retry` or `N retries`. See [Library pages](Library-Pages.md#queue).
- **Plan**: the preview of a rename that Animatic computes from the local library: the target path and
  any errors or warnings. Planning never touches the file. See
  [Rename targets and apply](Rename-Targets-and-Apply.md).
- **Portable profile**: a library and settings kept in a folder of your choice with `--database` /
  `ANIMATIC_DB` and `ANIMATIC_SETTINGS`, so nothing depends on the default folders. See
  [Portable and rehearsal setups](Data-Locations.md#portable-and-rehearsal-setups).
- **Queue** (page): the page that shows the command queue. See [Library pages](Library-Pages.md#queue).
- **Rehash changed files**: a managed-folder switch (off for a new rule): a scan hashes known files whose
  modified time changed. See [Managed folders and scans](Managed-Folders-and-Scans.md).
- **Release channel**: **Stable** or **Beta** in **Options → Updates**; decides which published
  releases the in-app updater offers. See [Updating Animatic](Updating-Animatic.md).
- **Remove from list**: **Hashed files → Remove from list…** forgets Animatic's record of selected files
  (path, hash, media properties). It never deletes the media file or touches MyList. See
  [Library pages](Library-Pages.md#hashed-files).
- **Remove missing files from the library**: a managed-folder switch (off for a new rule): a scan forgets
  the records of files that are definitely gone. See [Managed folders and scans](Managed-Folders-and-Scans.md).
- **Rename pattern**: the text, written in the rename language, that turns a file's AniDB data into its
  new name. See [Rename language](Rename-Language.md).
- **Retry failed**: the button on the **Queue** page (and the hash queue panel) that unblocks failed
  commands (or queues failed files for hashing again). It does not bypass a cooldown. See
  [Failure handling](AniDB-Synchronization.md#failure-handling).
- **Scan**: **Tools → Scan managed folders** walks your folder rules and hashes new files. A scan never
  contacts AniDB, renames files or writes MyList. See [Managed folders and scans](Managed-Folders-and-Scans.md).
- **Session** (AniDB session): a login to AniDB's UDP API. A run logs in when its first command needs it
  and out when it ends; with automatic runs on, one login is kept between runs and ended after the idle
  time you set. See [Automatic runs](AniDB-Synchronization.md#automatic-runs-and-the-anidb-session).
- **Settings file** (`settings.json`): your options, folder rules and window layout. Its previous valid
  copy is `settings.json.bak`. See [Settings files](Settings-Files.md) and
  [Data locations](Data-Locations.md).
- **Settings recovered**: the window shown at start when `settings.json` was unusable and Animatic
  loaded the backup copy offline instead. See [Settings problems](Backups-and-Recovery.md#settings-problems).
- **Settle delay**: how long a found file must stay unchanged before a scan hashes it (500 ms by
  default; **Hash a file once it is unchanged for**). It keeps files that are still downloading out of
  the scan. See [Settle delay and pacing](Managed-Folders-and-Scans.md#settle-delay-and-pacing).
- **SHA256SUMS**: the file on a GitHub Release that lists the SHA-256 hash of each archive, used to
  check a download. See [Installation](Installation.md#option-a-download-a-release).
- **Single-instance handoff**: launching Animatic with media files while it already runs on the same
  database gives the files to that window instead of opening a second one. See
  [One window per database](Running-Animatic.md#one-window-per-database).
- **Skip system files**: a managed-folder switch (on for a new rule) that makes scans skip files and
  folders with the Windows system attribute. It has no effect on Linux. See
  [Managed folders and scans](Managed-Folders-and-Scans.md).
- **Skipped** (command): a queued command that finished without being sent because AniDB's limits do not
  allow that request yet (for example an anime requested within 24 hours). It is removed from the queue
  and logged with the time it is allowed again. See [Request limits](AniDB-Synchronization.md#request-limits).
- **Status colors**: the tint and marker on table rows that mark CRC mismatches, unidentified, in-MyList
  and watched entries, set in **Options → Appearance**. See [Main window](Main-Window.md).
- **Storage**, **Source**, **Other**: three free-text fields of a MyList entry (where you keep the file,
  where it came from, a note). A managed folder can set them for the files it adds. See
  [Managing MyList](Managing-MyList.md).
- **Stored-only setting**: a settings key that Animatic keeps (often for AOM compatibility) but that
  changes nothing. See [Settings reference](Settings-Reference.md) and [Feature parity](Feature-Parity.md).
- **System tray**: the notification-area icon (Windows) or status-notifier icon (Linux) with **Show
  Animatic**, **Run command queue** and **Quit**. See [System tray](Main-Window.md#system-tray).
- **UDP API**: AniDB's main interface that Animatic uses to log in, identify files and edit MyList. It
  uses `api.anidb.net:9000` and the local UDP port 29456 by default. See
  [Network and sync problems](Network-and-Sync-Problems.md#network-and-firewall).
- **Update check**: the anonymous look at GitHub for a newer release, at startup (unless disabled or
  offline) or through **Help → Check for updates…**. Nothing is downloaded or installed without
  your confirmation. See [Updating Animatic](Updating-Animatic.md).
- **Update known file information**: the option (off by default) that queues a refresh of an already
  identified file's AniDB record when you hash it again, at most once every 7 days per file. See
  [AniDB synchronization](AniDB-Synchronization.md#synchronization).
- **Watched**: whether you have seen an episode or file; a flag of the MyList entry with an optional
  view date. See [Managing MyList](Managing-MyList.md).
- **Work offline**: the **Work offline (keep commands queued)** setting. While on, **Run command
  queue** sends nothing and commands wait; hashing, browsing and local edits keep working, and the
  startup update check is skipped. See [First launch](First-Launch.md#safe-first-run-settings).

## See also

- [Quick start](Quick-Start.md): the terms in use, step by step.
- [Library pages](Library-Pages.md): the pages and columns named above.
- [FAQ](FAQ.md): short answers to common questions.
