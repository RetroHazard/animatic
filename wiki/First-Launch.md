The first start of a fresh profile, what to set before anything talks to AniDB, and the AniDB
client registration that online work needs.

## First launch

On a fresh profile (a first launch with default settings also performs one automatic update
check, see [status](Installation.md#status-and-limits)):

1. **Startup.** The database is created and opened first, then settings are loaded:
   a missing primary and missing backup mean defaults; a valid primary loads; an
   invalid or missing primary with a valid `.bak` loads the backup **offline** and shows a
   "Settings recovered" window; unreadable settings or two unusable files abort startup.
   Details: [Settings problems](Backups-and-Recovery.md#settings-problems).
2. **Status bar** shows "Loading library…" and then "Ready". The left panel lists nine
   pages; **Overview** is shown.
3. **Overview first-run card** ("Bring your AniDB O'Matic library forward") appears
   because `first_launch` defaults to true. Use it to import a databank
   ([migration](Migrating-from-AOM.md#migrate-an-aom-databank)) or **Dismiss first-run card**.
   Dismissing writes the settings file immediately (a first save), unless the settings were
   recovered from a backup. A normal exit also saves the settings.
4. **Open Tools → Options…** and, before anything else:
   - tick **Work offline (keep commands queued)** and press **Save options**. The factory
     default is *not* offline;
   - leave **Username/Password/API key/Client name** empty until you have a registered
     client;
   - decide whether the managed-folder rule **Default** (which adds identified files to
     MyList as "On HDD", unwatched) is what you want. See
     [managed folders](Managed-Folders-and-Scans.md).
5. Add files (**File → Add files…**) or import an AOM library, then follow the
   [user guide pages](Home.md#use-animatic).

Defaults worth knowing (`settings.rs`, confirmed in source): theme dark, UI scale 1.0,
UDP server `api.anidb.net:9000`, local UDP port 29456, `client_version` 1, *Identify files
with AniDB after hashing* on, file-extension restriction off, hash export template
`%fulled2k%`, statistics export off.

## AniDB client registration

AniDB refuses unregistered API clients on both the UDP and HTTP APIs. The settings source
points registration at <https://anidb.net/software/add>. Rules that follow from the code:

- Options has **Client name** and **Client version** side by side. The version is the number
  you registered together with the name (`anidb.client_version`, default `1`, a whole number
  of 1 or more). It identifies your registered client, not the Animatic release, so updating
  Animatic never changes it; set it once to what your registration says
  ([settings reference](Settings-Reference.md)).
- The local UDP port (`anidb.local_port`, default 29456) and UDP server are likewise
  JSON-only. If the port cannot be bound, the run stops with an error that names the setting;
  Animatic never switches to another port by itself (AniDB bans many different source ports).
- The **API key** (set in your AniDB profile) is optional and turns on encrypted UDP sessions.
- An empty client name, user name or password makes the queue stop at the first command
  with an authentication error and blocks *every* pending command until you correct it and
  use **Retry failed** ([queue failure handling](AniDB-Synchronization.md#failure-handling)).
- Imported AOM settings do not contain a client name; set it again after applying an import.
- Never use another person's identifier or a made-up one, and never run automated
  tests against AniDB with fabricated credentials.

The HTTP API endpoint is plain `http://api.anidb.net:9001/httpapi`; the title list comes
from `https://anidb.net/api/anime-titles.xml.gz`; UDP uses `api.anidb.net:9000` by default.
Transport details: [storage and network contracts](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md).

## Next steps

- Hash and identify files, edit MyList, run scans: [Hashing and identification](Hashing-and-Identification.md),
  [AniDB synchronization](AniDB-Synchronization.md),
  [Managed folders and scans](Managed-Folders-and-Scans.md).
- Import an AOM library and its settings:
  [Migrating from AOM](Migrating-from-AOM.md#migrate-an-aom-databank).
- Understand or edit stored options: [settings reference](Settings-Reference.md).
- Write a rename pattern: [rename language](Rename-Language.md).
- Developing or verifying the build: [development](https://github.com/RetroHazard/animatic/blob/main/docs/development.md),
  [architecture](https://github.com/RetroHazard/animatic/blob/main/docs/architecture.md).
