Problems reaching AniDB or GitHub: network access, credentials, cooldowns and bans, queue
states, missing library entries and failed updates.

## Network and firewall

What the code uses (source: `animatic-anidb`):

| Purpose | Endpoint | Notes |
|---|---|---|
| UDP API | `api.anidb.net:9000` by default (**AniDB account** settings can change the server and local port) | IPv4 only. The client binds `0.0.0.0` on the configured local port (default 29456) and falls back to a random port if binding fails. Each exchange waits up to 20 s and a first timeout is retried once |
| HTTP API | `http://api.anidb.net:9001/httpapi` | Plain HTTP; requests are spaced at least 2 s apart; 60 s timeout |
| Titles dump | `https://anidb.net/api/anime-titles.xml.gz` | Public download, bounded as described in [Disk space, temporary files and leftovers](Backups-and-Recovery.md#disk-space-temporary-files-and-leftovers) |
| Update check and download | `https://api.github.com/` and `https://github.com/` (plus the CDN hosts GitHub redirects asset downloads to), HTTPS only | Not AniDB: no flood spacing and no cooldown. Anonymous: sends your IP address and `User-Agent: Animatic/VERSION`, no credentials or library data. Bounded sizes and timeouts. Used only for update checks and downloads you start, plus the optional startup check |

The UDP client enforces AniDB packet spacing (2 s between packets for the first five, then
4 s, resetting after 10 idle minutes) and re-authenticates when a session expires. A long
queue therefore takes a long time; that is not a hang. The HTTP client is built with the
HTTP library's defaults and Animatic sets no proxy of its own; the UDP API cannot use an HTTP
proxy.

| Message | Cause and action |
|---|---|
| `network error: cannot resolve <server>: …` | DNS failure or a wrong server name in the account settings |
| `network error: no IPv4 address for <server>` | The name resolves to IPv6 only. The UDP client needs an IPv4 address |
| `network error: cannot bind UDP socket: …` | Neither the configured local port nor a random port could be opened. Check security software |
| `no reply from AniDB (timeout)` | Outbound UDP to the server is blocked or lost. Allow outbound UDP to the server's port, and replies back to the local port (NAT and router behavior is outside Animatic) |
| `HTTP error: HTTP status 5xx` | A server-side failure from the HTTP API; counted toward the command's retry budget |
| `AniDB unavailable, retry in N min` | The server asked you to wait (or the HTTP API returned status 429, treated as a 30-minute wait) |

Transient network failures never lose queued work: the command stays in the queue and syncing
pauses for 60 seconds (see [Cooldowns and bans](#cooldowns-and-bans)).

## Credentials and client registration

Both AniDB APIs require a registered **client name**; the project ships none and its tests use
only synthetic local fixtures. Credentials and the client name live in **Tools → Options… →
AniDB account**. The Options window has no client-version field; `anidb.client_version`
defaults to `1` and can be changed only by editing the JSON file; AOM settings import does not
set it (see the [Settings reference](Settings-Reference.md)). For registration rules see
[AniDB client registration](First-Launch.md#anidb-client-registration).

The queue never runs on its own. **Tools → Run command queue** and **Queue → Synchronize now**
take a snapshot of the **current in-memory options** (saved or not) at the moment you click.
With **Work offline** ticked they send nothing, log `Offline mode is enabled; commands remain
queued`, and leave the queue alone.

| Message (in the **Last error** column, the **Log** or the status bar) | Meaning | Effect | Action |
|---|---|---|---|
| `authentication: no AniDB client name configured` | The client name is empty | Fatal: **every** non-blocked command is blocked | Enter the registered client name, then **Retry failed** |
| `authentication: username and password required` | Missing user name or password | Fatal, as above | Enter them, then **Retry failed** |
| `authentication: login failed: wrong username or password` | Login code 500 | Fatal, as above | Correct the credentials, then **Retry failed** |
| `authentication: illegal input or access denied (is the client registered?)` | Login code 505 | Fatal, as above | Verify the client registration |
| `authentication: API key not defined in your AniDB profile` / `no such user` | Encrypted-session setup failed (codes 309, 394) | Fatal, as above | Fix or clear the API key; check the user name |
| `authentication: access denied` | A later command was refused (code 502) | Fatal, as above | Check the account |
| `client version outdated - check the AniDB client registration` | Code 503 | Fatal, as above | Correct the client name or version registration |
| `authentication: HTTP API rejected the client registration (…)` | The HTTP API refused the client | Fatal, as above | Verify the client name and version |

After a fatal credential or client error the status bar shows **N blocked** and the Log shows
`Command blocked: N queued commands blocked: <message>`. Correct the cause, optionally **Save
options**, then **Queue → Retry failed**, then **Synchronize now**. A wrong password that you
keep retrying will keep failing; stop and fix it first.

Settings credentials are stored in plain text. Do not paste them into bug reports; see
[Privacy-safe support](Reporting-Problems.md).

## Cooldowns and bans

When AniDB refuses the client or the network fails, Animatic **stops the whole queue run** and
stores a cooldown so that it holds across restarts and new jobs:

- It is stored in the database's `meta` table (key `sync_cooldown`: an expiry time and the
  reason), so it belongs to **that database**. Another database or machine does not know about
  it; waiting still makes sense because the server-side condition remains.
- While it is active, **Run command queue** and **Synchronize now** are disabled (hover for the
  text) and the status bar and **Queue** page show `AniDB sync paused until HH:MM:SS: <reason>`
  in local time. A run started through another path logs `AniDB sync is paused for another …`
  and sends nothing.
- **Retry failed does not bypass a cooldown.** It only unblocks entries; the run still checks the
  stored cooldown first, and a stored value that cannot be read never locks syncing.
- There is no control to clear a cooldown, and editing the database to remove it is not
  supported. The cooldown exists to avoid immediate repeat requests after a refusal.

| Condition | Pause | What happens to the command |
|---|---|---|
| Connectivity failure: DNS, connect, TLS, timeout, no reply | 60 s | Stays queued, no attempt is counted |
| UDP server code 604 | 60 s | Stays queued, no attempt counted |
| UDP server codes 600, 601, 602 | 30 min | Stays queued; code 600 counts one attempt |
| `AniDB unavailable, retry in N min` | The duration given (HTTP 429: 30 min) | Stays queued |
| Ban: UDP codes 504, 510, 555; HTTP 403; HTTP `banned` / error 500 | 30 min (Animatic's conservative default when no duration is known) | Stays queued; a ban never blocks entries |
| HTTP status ≥ 500 or malformed server answers | 60 s | The command **counts an attempt**; after 5 counted attempts it is blocked |
| Local storage errors while applying a reply | 60 s | Counts an attempt |
| Code 598 (unknown command), 505 on a command (`illegal input: …`), unexpected reply codes, protocol errors, XML errors | none | That command is **blocked** and the run continues with the next |

Notes:

- The ban text may include the server's wording. If you see `client banned by AniDB: …`, stop
  starting more runs and wait.
- A run that finishes sends a logout. Cancel acts **between commands**: a command already in
  flight can take up to its timeout (20 s for UDP, 60 s for HTTP) to end. Closing the window
  does not wait for it.
- Durations are Animatic's own conservative choices unless AniDB supplied one; the project
  has no recorded production AniDB observation to cite for AniDB's actual ban lengths.

## Queue states: blocked, deferred and unsaved

The **Queue** page lists every saved command with its **Status** and **Last error**. There are
three different situations, and they are handled differently.

| State | Where you see it | Meaning | What to do |
|---|---|---|---|
| **Pending** | Status `Pending`, no retries | Waiting to run | Nothing; run the queue |
| **N retries** | Yellow status | Failed `N` times with errors that count toward the budget (at most 5) | Wait; it becomes **Blocked** at 5 |
| **Deferred** (not a separate label) | `Pending` or `N retries`, with a **Last error** such as a timeout or ban | Not attempted again because of connectivity or a cooldown, which do not count as attempts | Wait out the cooldown, fix connectivity, run again |
| **Blocked** | Red **Blocked**, red **N blocked** in the toolbar and status bar | A permanent failure, a protocol/format error, rejected credentials, or an unreadable stored command. It is skipped by runs and stays visible | Read **Last error**, fix the cause, then **Retry failed** |
| **blocked commands (refreshing)** | Red text | A newer blocked event arrived than the data on screen; the view is refreshing | Wait a moment |
| **NOT saved yet** | Yellow `N commands NOT saved yet` in the status bar | The command is only in memory | See below |

Rules that matter:

- **Retry failed** is enabled only when something is blocked. It unblocks and resets the
  attempt counter of every blocked command, merges multiple edits of the same MyList entry into
  the latest, and does not touch the cooldown. It does not clear **Last error** text; a stale
  message can remain until the command is retried or removed.
- **Clear queue** deletes **every** queued command, including unsynchronized MyList edits and
  blocked commands. It asks for no confirmation. It does not undo the optimistic local state
  those edits already applied, so the local view can then disagree with AniDB. It does not
  clear the cooldown. There is no per-command discard in the interface; to drop one command you
  must clear the whole queue, so make a backup first when other work is pending.
- A blocked MyList edit whose entry no longer exists on AniDB says `mylist entry N no longer
  exists on AniDB; the edit is kept (blocked): discard it from the queue or refresh the entry`.
  Retrying it fails again; only **Clear queue** discards it from the interface.
- An unreadable stored command is blocked with `unreadable queued command: …` rather than being
  deleted. Because Animatic cannot read it, such a row cannot overlay its pending intent onto
  values received from AniDB; readable queued edits are preserved over remote values.
- `file unknown to AniDB` after identification is a normal answer for a file AniDB does not
  know, not an error, and is not queued again.
- Command identifiers are never reused, so an older reply cannot erase newer intent.

### Database busy and unsaved commands

Two connections write to the database: the background worker and the interface. A long write
(for example an import, a title merge or a large registration) makes the other side wait.

| Side | Wait before giving up | What you see |
|---|---|---|
| Worker | 30 s | A failed job reports the database error |
| Interface | 1.5 s | The behavior below |

- **A MyList edit that cannot be saved** reports `<action>: the database is busy with a
  background write; nothing was changed, try again`. The edit is not applied; repeat it.
- **A queued command (identify, title download, refresh)** that hits a busy database is kept in
  memory: the Log says `Database busy: queued command is NOT saved yet, retrying` and the
  status bar shows `N commands NOT saved yet`. Animatic retries every half second and logs
  `Saved N previously unsaved commands` when it succeeds. At most 1000 are held; beyond that
  you get `NOT queued: <command> (database busy and 1000 commands are already unsaved)`.
- **Closing the window** makes one final attempt of up to 5 s. If the database is still busy,
  the unsaved commands are lost, and the only record is a `log` message that nothing prints
  (the application installs no logger). **Do not quit while the unsaved counter is non-zero.**
- Causes of long busy periods: a legacy import (each collection is one transaction), the
  titles merge, another program touching the files (backup tool, antivirus, sync client) or a
  second Animatic instance. Wait for the running job to finish or cancel it.
- `Unable to refresh library: …` means the background snapshot failed; a later refresh
  (**File → Refresh**) usually succeeds once the contention ends.

## The library is missing entries

- **A full-account bootstrap is not implemented.** Animatic never downloads your whole AniDB
  MyList. **Tools → Fetch mylist statistics** only fetches counts. **Tools → Download anime
  titles** fetches the public titles dump only. `--full-update` (command-line only, no menu
  item) queues the titles dump plus lookups for the files and anime your **hashed local files**
  already reference. Library content comes from an AOM import, from hashing and identifying
  files, from the anime detail window's **Refresh from AniDB**, and from your own edits.
- Not every hashed file is known to AniDB. A file AniDB does not know logs `file unknown to
  AniDB`.
- **Download anime titles** failures leave your existing titles and the "last updated"
  timestamp unchanged: the download is validated (XML, gzip end-of-stream and CRC, size and
  count limits) and merged in a single transaction only when complete. Errors you may see
  include `titles dump larger than the N byte download limit`, `… decompressed limit`,
  `unexpected data after the compressed titles dump`, `invalid compressed body: …` and `title
  longer than N bytes`. Format and size errors block the `TitlesDump` command; connectivity
  errors pause syncing instead (see [Cooldowns and bans](#cooldowns-and-bans)). After a block,
  use **Retry failed** once the cause is gone, or queue the download again from the Tools
  menu. A download is not resumable.
- Cached **MyList** displays include optimistic local state for queued edits; a blocked or
  cleared edit can leave the local view different from AniDB.
- A file lookup that **explicitly** reports "not in MyList" removes the local MyList rows for
  that file, unless a queued local edit for the entry exists. A malformed successful reply is
  a protocol error, never treated as absence. If an entry vanished after an online run,
  check the **Log** for the identification result and compare with AniDB's website before
  assuming data loss; a backup taken before the run is the way back.

## Update problems

How updating works and what it changes: [Updating Animatic](Updating-Animatic.md). The
update path is separate from AniDB: an update failure never touches the command queue or an
AniDB cooldown, and an AniDB cooldown never blocks it.

| Symptom | Cause and action |
|---|---|
| No check happens at startup | By design when `updates.auto_check` is off, *Work offline* is on, or settings were recovered from a backup. **Help → Check for updates…** still works |
| Help → Check for updates… is disabled or the platform is reported unsupported | Only Windows and Linux on x86_64 are supported. Download releases manually elsewhere (none are built) |
| The check fails, or reports a rate limit | Unauthenticated GitHub API requests are limited per IP address, and a firewall, proxy or offline machine also fails. Nothing is retried automatically; try again later. A manual check always shows the error, a startup check writes it only to the **Log** |
| The check reports "not found" or the release feed as unavailable | GitHub answers HTTP 404 when the release data is not visible to an anonymous client: the repository's releases are not publicly readable, or nothing is published. This is an error, never "up to date". Animatic has no token or sign-in setting, so until the releases are publicly readable, update manually ([Releases](https://github.com/RetroHazard/animatic/blob/main/docs/releases.md#install-a-release)). This guide does not state whether the repository is currently public |
| "Animatic is up to date" although a newer tag exists on GitHub | Only published, non-draft, non-prerelease releases with a valid CalVer tag (no `-` suffix), and only when strictly newer than the running version, are offered; the platform archive and `SHA256SUMS` must be present and correctly named. Drafts, release candidates and older versions are never offered. If the newest stable release is incomplete the check reports an error instead of offering an older one |
| Download or verify fails (size, checksum or archive error) | Nothing was installed. The staged files are in a private temporary folder that is removed when the update is dropped or Animatic exits. Retry; if the checksum keeps failing, do not install and download manually from the [releases page](https://github.com/RetroHazard/animatic/releases) as described in [Releases](https://github.com/RetroHazard/animatic/blob/main/docs/releases.md#install-a-release), and report it |
| Install or Restart now is disabled | They are blocked while a job is running, an AOM import is in progress, imported settings are waiting to be applied, or some changes are not yet saved to the database; Restart is also blocked while settings recovered from a backup are unsaved (use **Save options**). The reason is shown in the window. Finish or cancel that work first |
| Install fails with a permission or read-only error | The folder holding the executable must be writable by your account. There is no elevation. Update the copy the way it was installed (administrator copy, package manager) or move Animatic to a user-writable folder |
| Install refuses because the executable does not match | The running file is not the one Animatic started from (moved, replaced or launched through an unusual path). Restart Animatic normally and try again |
| `LICENSE-vVERSION` and `THIRD-PARTY-NOTICES-vVERSION.html` appear beside the executable | Expected after an install, even a failed one. They are the new release's license files, are never overwritten, and are harmless to keep. A different pre-existing file with the same name makes the install stop; inspect or rename it |
| An empty `.animatic-update.lock` file beside the executable, or a message that another install is running | Created by the install step; the OS holds an advisory lock on it only while an install runs. The empty file is harmless and may remain. The message means another Animatic process holds the lock: wait for it or close that instance |
| Dot-prefixed files next to the executable (`.animatic-recovery-*.exe`, `.animatic-restore-*`, or helper files from `self-replace` such as `.__selfdelete__.exe`) or a renamed old executable | `.animatic-recovery-*.exe` is the Windows copy of the previous executable made before the swap; it is removed after a successful install and **kept after any failed one**. Other files are left if the machine lost power or Animatic was killed mid-install. Animatic never deletes them automatically. Check which executable starts, and delete leftovers by hand only after confirming a working copy |
| The window says the installation did not finish and the program files may be in an unknown state | The swap itself failed. Animatic stops offering updates and does not restart by itself. Check that the executable still starts; the message names the recovery copy and, if the old file was not restored, which file to copy to which name. Otherwise install the release manually ([Releases](https://github.com/RetroHazard/animatic/blob/main/docs/releases.md#install-a-release)). Restart Animatic by hand before trying again |
| Animatic does not start after an update | Download the archive for your platform from the releases page, verify `SHA256SUMS`, and run or copy the executable from it ([Releases](https://github.com/RetroHazard/animatic/blob/main/docs/releases.md#install-a-release)). Your database and settings were not changed by the update |
| The documentation or notices beside the executable are older than the program | Expected: only the executable and versioned license files are updated. Replace the whole folder from a newly downloaded archive to refresh the bundled documents |
| Restart did not start a new window | The new process is started only after the old one exits, with only your `--database` path (and `ANIMATIC_SETTINGS` when it was set). Start Animatic yourself; nothing is lost because settings were saved before closing |

The check, download and install are bounded by byte limits and timeouts and run on their own
thread, so a slow or stuck GitHub connection never freezes the window; use **Cancel** during
a download, or close Animatic (which cancels it). Closing during the short install step waits for it.
