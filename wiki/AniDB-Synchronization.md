How local work reaches AniDB: the synchronization job, MyList edits and what happens when a
command fails. Error messages and recovery steps are in
[Network and sync problems](Network-and-Sync-Problems.md).

## Synchronization

Animatic works **offline-first**. Hashing, browsing, searching and local MyList edits never
touch the network. Network traffic happens only in the *Synchronizing with AniDB* job, started
by **Tools → Run command queue** or **Queue → Synchronize now** (nothing runs on a timer).

Before the first run: set **Client name** (registered with AniDB), **Username** and
**Password**, optionally an **API key** (encrypted UDP sessions), clear **Work offline**, press
**Save options**. See [client registration](First-Launch.md#anidb-client-registration). Imported AOM
settings never contain a client name.

The job (`process_queue`):

1. If *Work offline* is on, it logs `Offline mode is enabled; commands remain queued` and stops.
   The queue is unchanged.
2. If a persisted cooldown is active it logs how long remains and stops (this also refuses to
   start the job, via the disabled buttons).
3. Otherwise it builds UDP and HTTP clients from the current in-memory settings and repeatedly runs
   the next unblocked command until the queue is empty, you cancel, a command stops the run or a
   cooldown begins. Every successful command is removed from the queue and logged
   (`known file F (anime A), mylist entry L`, `mylist entry updated`, `anime A updated`,
   `N titles downloaded (M new)`, …). It logs out of UDP at the end.
4. Requests are paced: UDP packets at least 2 s apart (4 s after a burst of five; idle for 10
   minutes resets that), HTTP requests 2 s apart. UDP login is lazy (first UDP command).

What the user-reachable commands do:

| Command | Effect on the local database |
|---|---|
| Identify | Stores AniDB's file record; optionally adds the file to MyList (see above) |
| Edit mylist entry | Sends the merged state, watched, view date, storage, source and other changes; on success updates the local entry (keeping any newer pending edit on top) |
| Add to mylist | From **Hashed files → Add to MyList…**. An identified file is added by file id; an unidentified one by size and ED2K hash, which AniDB looks up first (a file it does not know is dropped with the log line `file unknown to AniDB`). A new entry is stored locally with the values you set; if the file is already in MyList the entry is refreshed and your values are applied as an edit. Until then **Hashed files** shows `Add queued`. |
| Remove mylist entry | From **My list → Remove from MyList…**. Deletes the entry on AniDB. The local row is removed only after AniDB acknowledges (or says the entry no longer exists), and not at all if you queued something for the entry afterwards. Never touches your media file. |
| Refresh mylist entry | From **My list → Refresh entry**. Re-reads the entry and replaces the cached row, keeping edits that are still queued on top. If AniDB says the entry is gone, the row is removed unless changes for it are queued. |
| Fetch anime N | Downloads the HTTP-API anime record: titles, episodes, genres, relations, reviews, metadata |
| Fetch file N | Updates a file record (from a full update) |
| Fetch episode N | From the **Refresh** button of an episode in the [anime detail window](Library-Pages.md#anime-detail-window). Replaces the cached episode record |
| Fetch group N | From the **Refresh** link on the [Groups page](Library-Pages.md#groups). Replaces the cached group record |
| Download anime title list | Streams `anime-titles.xml.gz` into a temporary store, validates it, then merges atomically in one transaction. Creates an anime row for every unknown id and replaces titles only of anime without a full HTTP record (imported AOM anime count as such). Failure leaves titles and the completion time untouched. Limits: 32 MiB compressed, 128 MiB decoded, 250 000 anime, 2 000 000 titles, 2048 bytes per name. |
| Fetch mylist statistics | Stores AniDB's account figures with the time they arrived (replacing the previous ones) and shows them on the [Overview](Library-Pages.md#overview) as **AniDB account figures**, apart from the local figures. The log line `N anime, N episodes, N files in mylist` is still written |

A full update (`--full-update`) queues the title list plus one file lookup and one anime refresh for
each file id and anime id that your **identified hashed local files** resolve to (not the whole local
cache or the title catalog); it does not run them.

## MyList editing

On **My list**, changing **State** or toggling **Watched**, or saving the **Edit…** dialog (state,
watched, view date, storage, source, other), is a *local-first* edit:

1. In one database transaction the edit is saved to the outbox **and** the stored MyList entry is
   updated, so the row shows the new value immediately (even before the next snapshot) and
   the change survives restarts.
2. The edit is sent to AniDB only when the queue runs (priority 0, ahead of lookups).
3. Further edits of the same entry before that are **merged** into one fresh queue row with a new,
   never-reused id: unset fields keep the older value, and the last watched/unwatched choice wins
   (watched → unwatched → watched ends watched at the latest time). Marking watched stamps the
   time of your click, not the time AniDB hears of it; unmarking clears the date. A field the
   editor sent as an empty string (ticked, empty) is a real value that clears it on AniDB and
   stays in the merge; a field you did not tick is simply absent.
4. When AniDB acknowledges, the local entry is rewritten with the acknowledged values **plus** any
   newer pending edit. An acknowledgement or reply for an older command cannot erase newer
   intent. Queued edits that can be read are re-applied over AniDB data fetched meanwhile, so it
   cannot overwrite an entry that still has them. A queue row this version cannot read is kept
   (blocked) but cannot be overlaid, so remote values may replace the local ones for its entry.
5. A malformed "success" reply is a protocol error, never evidence that something was deleted.

Failures and edge cases:

- *No such entry on AniDB:* the edit is kept and **blocked** with an explanation (the local entry
  stays). Refresh the data or discard the row, then **Retry failed**.
- Permanent errors block the one edit; transient errors pause the queue ([below](#failure-handling));
  authentication problems block everything until fixed.
- **Clear queue** deletes the pending edit rows only. Nothing restores the previous local values, so
  the screen keeps showing the edit even though AniDB will never receive it. A later AniDB
  refresh of that entry may overwrite it [INFERENCE: no pending intent remains to protect it].
- If the database is locked by a background write the edit is refused with a visible message;
  repeat it.
- An entry with a queued removal ignores edits (`Mylist entry N is pending removal; edit
  ignored`); the editor says so and its **Save** is disabled. **Remove from MyList…** queues the
  removal (and discards the entry's queued edits, which are moot). The local row stays and shows
  `Removal queued` until AniDB acknowledges; a removal that fails permanently stays queued and
  blocked (`Removal blocked`, last error on hover) and the row is kept, until you **Retry failed**
  or clear it. If the removal left the queue while it was being sent (for example **Clear queue**
  during a run) and you then edit the entry, that newer edit wins: the acknowledgement does not
  delete the row.

## Failure handling

How the queue reacts. The retry, cooldown and blocking *policy* lives in the desktop worker
(`fail_command`, `process_queue`); the core library supplies the durable queue operations and a single
`execute` per command, and `AnidbError` classifies the failures:

| Situation | Reaction |
|---|---|
| Ban (UDP 510/555/504; HTTP ban text) | Entry kept, **no attempt counted**, queue stops, cooldown **30 minutes** (persisted) |
| Rejected credentials, missing client name/user/password, outdated client (UDP 503) or HTTP client rejection | **Every** unblocked command is blocked with the reason; the queue stops; no cooldown. Fix the setting, **Save options**, **Retry failed**, run again |
| Network/DNS/timeout, AniDB "unavailable" | Queue stops; cooldown 60 s (or the server's stated wait); not counted as an attempt |
| UDP server errors 600/601/602/604 | 604 → 60 s cooldown; the others 30 minutes. Only error 600 and HTTP responses with an error status count toward the attempt limit |
| Storage I/O or database errors while applying a reply | Treated as transient: 60 s cooldown, counted |
| Attempt limit | After 5 counted failures a command is blocked (`blocked after 5 failed attempts`) |
| Permanent errors (illegal input, protocol/XML problem, unexpected reply, unknown command) | Only that command is blocked; the queue continues |

Cooldowns are stored in the database (`sync_cooldown`) and survive restarts. **Retry failed** does
not clear them; Run/Synchronize are disabled until the shown time passes. Blocked rows stay
visible with their last error until you retry, clear or the underlying problem is fixed.
