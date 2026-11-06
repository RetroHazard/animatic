The first start of a fresh profile, what to set before anything talks to AniDB, and the AniDB
clients Animatic uses for online work.

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
   - leave **Username/Password/API key** empty until you have your AniDB account at hand;
   - decide whether the managed-folder rule **Default** (which adds identified files to
     MyList as "On HDD", unwatched) is what you want. See
     [managed folders](Managed-Folders-and-Scans.md).
5. Add files (**File → Add files…**) or import an AOM library, then follow the
   [user guide pages](Home.md#use-animatic).

Defaults worth knowing (`settings.rs`, confirmed in source): theme dark, UI scale 1.0,
UDP server `api.anidb.net:9000`, local UDP port 29456, *Identify files
with AniDB after hashing* on, file-extension restriction off, hash export template
`%fulled2k%`, statistics export off.

## AniDB client registration

AniDB refuses unregistered API clients, and AniDB registers a client **per API**. Animatic has
its own registered clients, built into the program: `animatic` version 1 for the UDP API (login,
file identification, MyList) and `animatichttp` version 1 for the HTTP API (full anime records).
You do not register, enter or change anything: Options shows them read-only under the account
fields ("AniDB client: …"), and there is no setting for them. Updating Animatic never changes
them; they change only in a release that ships a newly registered client version. Rules that
follow from the code:

- You only need your AniDB **Username** and **Password** (an AniDB account; sign up at
  <https://anidb.net>). The **API key** (set in your AniDB profile) is optional and turns on
  encrypted UDP sessions.
- The local UDP port (`anidb.local_port`, default 29456) and UDP server are JSON-only. If the
  port cannot be bound, the run stops with an error that names the setting; Animatic never
  switches to another port by itself (AniDB bans many different source ports).
- An empty user name or password makes the queue stop at the first command with an
  authentication error and blocks *every* pending command until you correct it and use
  **Retry failed** ([queue failure handling](AniDB-Synchronization.md#failure-handling)).
- Settings files from older versions may still hold `client_name`, `client_version`,
  `http_client_name` and `http_client_version`. They are ignored and dropped the next time the
  settings are saved.
- If AniDB refuses the built-in client (reply 503 or 504, or an HTTP client rejection), the
  commands are blocked with that reason; update Animatic, and report it if you already run the
  latest release ([Network and sync problems](Network-and-Sync-Problems.md#credentials-and-client-registration)).
- Never run automated tests against AniDB with fabricated credentials.

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
