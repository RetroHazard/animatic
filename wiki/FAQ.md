Short answers to the questions people ask most, with links to the pages that explain them in
full. If your problem is a specific message or symptom, start with
[Troubleshooting](Troubleshooting.md) instead. Terms such as *hash*, *identify*, *managed folder*
and *cooldown* are defined in the [Glossary](Glossary.md).

## Your data and your files

### Is my AOM databank changed when I import it?

No. The import only reads the files you point it at; it never modifies or deletes the AOM
databank. Animatic decrypts each collection into a temporary work folder (a name starting
`animatic-legacy-import-`), imports it into its own library database and removes the folder when
the import ends. If an import is interrupted, the next start offers to remove leftovers. See
[Migrating from AOM](Migrating-from-AOM.md) and
[AOM import problems](File-and-Import-Problems.md#aom-import-problems). The same goes for AOM
settings: **File → Import → AOM settings…** reads the XML and lets you review it before anything
is applied.

### Does Animatic delete my media files?

No. It never deletes a media file on its own account. Two things come close, and both are
explicit:

- A **rename** moves a file. When the new place is on another drive, Animatic copies the file,
  verifies the copy against the stored hash and only then removes the original. There is no undo.
  See [Renaming files](Renaming-Files.md#what-applying-does).
- Empty folders that your own rename emptied inside a managed folder are removed only when that
  folder's **Remove folders a rename leaves empty** is on (off by default).

**Remove from list…** and a scan with **Remove missing files from the library** drop only Animatic's *records* of files, never
the files. See [Remove from list](File-and-Import-Problems.md#remove-from-list).

### Where are my settings and my library stored?

In two places: a JSON settings file (`settings.json`) and a SQLite database
(`animatic.sqlite3`). The default folders on Windows and Linux, the environment variables and
the `--database` option that change them are in [Data locations](Data-Locations.md#where-data-lives).
Cover pictures have their own folder.

### Are my AniDB password and API key safe?

They are kept in the operating-system keystore when one exists (Windows Credential Manager, Linux
Secret Service), not in `settings.json`. If **Tools → Options…** shows the notice that no keystore
is available, they are stored in plain text in the settings file and its backup instead. The
database and library backups are not encrypted. See
[Settings files](Settings-Files.md#credentials-and-the-system-keystore) and
[What to back up](Backups-and-Recovery.md#what-to-back-up-and-why-it-is-sensitive).

### What does Animatic send over the network?

Only to the destinations listed in [Network and firewall](Network-and-Sync-Problems.md#network-and-firewall):
AniDB (UDP and HTTP API, the public title list and cover pictures), GitHub for update checks and
downloads, and, if you ask for it, the pinned `ffprobe` download. The update requests are
anonymous: your IP address and the user agent `Animatic/VERSION`, no credentials and no library
data. Nothing is sent to AniDB while **Work offline** is on.

### Can Animatic repair a damaged database?

No. **Tools → Check library** tells you whether the library is damaged, and **File → Restore
library…** puts a backup back, but there is no repair. Back up before large operations. See
[Backups and recovery](Backups-and-Recovery.md#damaged-database).

### Does Animatic back up my library automatically?

No. Backups are manual (**File → Back up library…**), and a backup holds the library and queue but
not the settings. See [Back up and restore](Backups-and-Recovery.md#back-up-and-restore-inside-animatic).

## Working with AniDB

### Why are my commands not sent?

Because something is holding the queue. The usual reasons, in the order to check them: **Work
offline** is on (the status bar shows **Offline**), nothing started the queue (it runs only when
you choose **Run command queue**, unless you switched on **Run the command queue
automatically**), a cooldown is active, or the commands are blocked. The checklist is in
[Why are my commands not sent?](Network-and-Sync-Problems.md#why-are-my-commands-not-sent).

### Why does Animatic say it is offline?

The status bar's **Offline** indicator reflects the **Work offline (keep commands queued)** setting. It is on because you
ticked it or clicked the indicator, because Animatic recovered your settings from the backup, or
because you applied imported AOM settings (both force offline until you untick it). Click the
indicator to switch. While it is on, queued work waits and nothing is sent. See
[Credentials and client registration](Network-and-Sync-Problems.md#credentials-and-client-registration).

### Can I use Animatic without an AniDB account?

Yes, for the local parts: hashing files, the Hashed files page, scans, and anything built from
data already in your library (for example after an AOM import). Identifying files, adding to
MyList and refreshing from AniDB need an account (user name and password, and optionally an API
key). Tick **Work offline** and the commands stay queued until you are ready. See
[First launch](First-Launch.md).

### Do I have to register an AniDB client?

No. Animatic sends its own registered clients (`animatic` on UDP, `animatichttp` on HTTP), built
in and not configurable. If AniDB refuses it (reply 503 or 504), update Animatic. See
[Credentials and client registration](Network-and-Sync-Problems.md#credentials-and-client-registration).

### What is a cooldown, and can I skip it?

When the network fails, AniDB is busy or AniDB bans you, Animatic stops all requests for a while
(60 seconds for a network failure, 30 minutes for a ban or server error) and shows `AniDB sync paused until …`.
It is stored in the database, so a restart does not skip it, and **Retry failed** does not bypass
it. Sending sooner risks a longer ban. See
[Cooldowns and bans](Network-and-Sync-Problems.md#cooldowns-and-bans).

### Why is a download or refresh "skipped" or "allowed after …"?

AniDB asks clients not to repeat requests, so Animatic requests the title list at most once per
24 hours, an anime at most once per 24 hours and a known file at most once per 7 days. See
[Request limits](AniDB-Synchronization.md#request-limits).

### Can I download my whole MyList from AniDB?

Not entry by entry: AniDB forbids that. Use **Tools → Request MyList export…**, wait for AniDB's
message (up to 24 hours), download the file from AniDB's export page and import it with
**File → Import → MyList export…**. See
[Full-account MyList import](AniDB-Synchronization.md#full-account-mylist-import) and
[Managing MyList](Managing-MyList.md).

### Why is a removal from MyList not sent automatically?

By design: even with **Run the command queue automatically** on, MyList removals wait until you
click **Run command queue**, because a removal is destructive on AniDB. The Queue page shows how
many wait.

### How do I add an anime, episode or file that AniDB does not know?

Use **Tools → Edit AniDB data**, which opens AniDB's own web editors in your browser. Animatic
does not edit AniDB metadata itself. See [Feature parity](Feature-Parity.md).

### A file shows "Not identified". Why?

Either the Identify command has not been sent yet (offline, or the queue has not run), it is
blocked, or AniDB does not know the file (`file unknown to AniDB`, a normal answer). See
[Files that are not identified](File-and-Import-Problems.md#files-that-are-not-identified-or-show-a-crc-mismatch).

## Files, scans and renaming

### Why did hashing not start by itself?

Files wait in the persistent hash queue. They resume at start only when **Start hashing
automatically** is on (**Tools → Options… → Hashing**; it is on by default in a new
settings file); otherwise click **Resume hashing**. See
[Hashing and identification](Hashing-and-Identification.md#hash-queue) and
[Hashing large files](File-and-Import-Problems.md#hashing-large-files).

### Why did a scan hash nothing?

Usually no managed folder rule has **Hash new files**, the extension list does not include the file
type, or the file was still being written. The checklist is in
[Scans and managed folders](File-and-Import-Problems.md#scans-and-managed-folders).

### Records disappeared after a scan. What happened?

A rule with **Remove missing files from the library** removed the records of files it found missing under a folder that
exists. A folder that exists but is empty because its drive is not mounted looks like missing
files. Restore a backup or remount and rescan. See
[Offline drives and automatic removal](File-and-Import-Problems.md#offline-drives-and-automatic-removal).

### Why was a file not added to my MyList?

A fresh profile has no folder rules, and only a rule with **Add identified files to MyList** on adds the
files it covers. A file that no rule covers is identified (when **Identify files with AniDB after
hashing** is on) but not added to MyList, and it is renamed in its own folder. See
[Which rule applies to a file](Managed-Folders-and-Scans.md#which-rule-applies-to-a-file).

### Can I undo a rename?

No. Review the preview before you apply it, and use **Dry run (log only)** before you switch a managed folder
to automatic rename. A cross-drive move is verified against the stored hash before the original is
removed. See [Renaming files](Renaming-Files.md) and
[Rename problems](File-and-Import-Problems.md#rename-problems).

### Why is renaming blocked for my file?

The file must be identified by AniDB and its anime and episode data must be in the library, the
pattern must evaluate without warnings, the new name must be valid on every platform and must not
exist yet. The message in the rename window says which. See
[Rename problems](File-and-Import-Problems.md#rename-problems).

### Can I keep my library on a network drive or a cloud-synced folder?

Media files on a network share work (hashing is slower), but turn **Remove missing files from the library** off for
removable or network folders. Do not put the **database** on a network share or in a
cloud-synchronized folder: SQLite's write-ahead log needs a local filesystem. See
[Cloud sync, roaming profiles and network drives](Backups-and-Recovery.md#cloud-sync-roaming-profiles-and-network-drives).

## Using Animatic on more than one computer

### Can I use Animatic on two PCs?

Each computer has its own library, settings and queue; Animatic does not synchronize them. Your
AniDB MyList is the shared record: both libraries can talk to it. Do not synchronize the
database or the settings file through a cloud folder, and do not run two instances on the same
database. See [Cloud sync, roaming profiles and network drives](Backups-and-Recovery.md#cloud-sync-roaming-profiles-and-network-drives)
and [One window per database](Running-Animatic.md#one-window-per-database).

### How do I move my library to another computer?

1. On the old computer choose **File → Back up library…** and copy the backup file over. If you
   also want your managed folders and options, close Animatic on the old computer and copy
   `settings.json` too: Animatic writes its settings when it exits, so a copy made (or pasted)
   while it runs can be overwritten.
2. Install Animatic on the new computer ([Installation](Installation.md)). To keep your settings,
   put `settings.json` into the new computer's settings folder **before the first start** (see
   [Data locations](Data-Locations.md#where-data-lives)), then start Animatic once.
3. Choose **File → Restore library…**, pick the backup and confirm.
4. Tick **Work offline (keep commands queued)** in **Tools → Options… → AniDB account**, choose
   **Save options** and look at **Queue** before going online, because a restored queue can hold
   commands you already ran.
5. Enter your AniDB account again: credentials live in the operating-system keystore and do not
   travel with the library or the settings file.
6. Check the folder paths. Library records store full file paths, so if your media is under
   different paths on the new computer, scan the new folders with **Hash new files** and clear the
   stale records with **Remove from list…** (see [Remove from list](File-and-Import-Problems.md#remove-from-list)).

### Can I run two libraries on one computer?

Yes. Give each its own database with `--database FILE` or `ANIMATIC_DB`, and its own settings with
`ANIMATIC_SETTINGS`. See [Running Animatic](Running-Animatic.md) and
[Portable and rehearsal setups](Data-Locations.md#portable-and-rehearsal-setups).

## The application

### Does Animatic run on Linux or macOS?

Windows and Linux are the supported targets, and CI builds and tests both. The project's recorded
runtime checks on Linux are limited (a WSLg run with synthetic data), so treat Linux as less
exercised than Windows. No macOS build is published. See
[Installation](Installation.md) and [Feature parity](Feature-Parity.md#verification-status).

### Is there an installer? Are the downloads signed?

Releases are a Windows ZIP and a Linux tar.gz with a `SHA256SUMS` file. There is no installer, and
the archives are not code-signed, so your operating system or security software may warn. Verify
the download against `SHA256SUMS`. See [Installation](Installation.md).

### How do I update Animatic?

**Help → Check for updates…** opens the Software updates window; nothing downloads or installs
without your confirmation. The default **Stable** channel offers published releases; choose
**Beta** in **Tools → Options… → Updates** to see release candidates. See
[Updating Animatic](Updating-Animatic.md) and, if it fails,
[Update problems](Network-and-Sync-Problems.md#update-problems).

### Can I turn off the startup update check?

Yes: untick **Check for updates at startup** in **Tools → Options… → Updates**. It is on by
default, only *checks* and installs nothing, and is skipped while you work offline. See
[Avoiding the startup update check](Data-Locations.md#avoiding-the-startup-update-check).

### Where is the log, and does it keep history?

The **Log** page shows this session's activity. It lives in memory only, older lines are dropped
as it grows, **Clear** empties it, and it is lost when you quit. There is no log file. See
[Reporting problems](Reporting-Problems.md#what-is-sensitive).

### How do I reset my settings?

Close Animatic, move `settings.json` and `settings.json.bak` into a safe folder and start again:
with neither file present Animatic uses defaults. See
[Resetting settings deliberately](Backups-and-Recovery.md#resetting-settings-deliberately).

### How do I report a bug without leaking my library?

Sanitize first: no settings files, databases or exports, and placeholders for paths, titles,
hashes and ids. The steps, the version lookup (**Help → About Animatic**) and the issue forms are
in [Reporting problems](Reporting-Problems.md).

## See also

- [Troubleshooting](Troubleshooting.md) for the symptom index and the index of messages.
- [Quick start](Quick-Start.md) for a first-hour tutorial.
- [Glossary](Glossary.md) for the terms used here.
