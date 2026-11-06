**Adding files.** *File → Add files…*, the Hashed files page, the Overview button, or media paths on the
[command line](Running-Animatic.md#command-line-reference). The picker's extension filter is
only a convenience; command-line paths are never filtered. Files from the picker, the command
line, another launch (the [file-menu entry](Options-Window.md#file-menu-entry)) and
[scans](Managed-Folders-and-Scans.md) first join the persistent [hash queue](#hash-queue) and are
then hashed in order. Each file is read **once**; ED2K
(the AniDB-compatible "red" variant), CRC32, MD5 and SHA-1 are computed together on separate
threads. Progress shows the file name, `(n/total)` and the current file's percentage.

Result handling per file:

1. The local hash and file path are stored (the same path is updated in place; identical content at
   a different path shares one hash record). Paths are stored exactly as given, so use absolute
   paths.
2. The Log gets `Hashed <path> (local #N, <ed2k>)`.
3. If *Identify files with AniDB after hashing* is on (default), an **Identify** command is
   queued. If the file's [folder rule](Managed-Folders-and-Scans.md) has **Add to mylist**, the
   command also carries that rule's MyList settings.
4. The file leaves the hash queue only now, after its result is stored. A read error (missing
   file, permission, a directory) is logged as `<path>: <reason>` and the file stays in the queue
   as **failed** with that reason (see [Hash queue](#hash-queue)); the remaining files still run.
   Cancel stops the whole batch and leaves the files that were not hashed yet in the queue.

**Identification** happens only when the queue runs: AniDB is asked for the file by size and ED2K
hash. If AniDB knows it, the file record is stored (so Hashed files shows `fid N`); if the file
is not already in your MyList and the command carries an add policy, it is added with the rule's
state, watched flag, storage, source and other fields; if it is already in MyList the policy is
**not** applied and the MyList entry AniDB reports is stored locally. If AniDB explicitly reports
that the file is **not** in your MyList, local entries for that file are removed unless they have
queued edits. An unknown file logs `file unknown to AniDB` and nothing else changes.

With default settings, the built-in **Default** rule has *Add to mylist* on, *Set state* on with
*On HDD*, and *Set watched* on with *Watched* off, so identified files that match no custom rule are
added to MyList as "On HDD", unwatched, when you run the queue. Review this before online work.

## Hash queue

Files waiting to be hashed are saved in the library database, so they survive closing Animatic,
a crash and a restart. The same file is queued once, however its path is spelled (a relative
path, a different case on Windows, `..` segments or a link all resolve to one entry); the path
that is hashed and stored in the library is the absolute path as given.

- **Where it is shown.** The **Hashed files** page (and a compact summary on the Overview) show
  the queue while it is not empty: how many files wait, how many failed, and the list (up to 500
  files, failed ones first, each with where it came from and, for failed ones, why).
- **Cancel** stops the current run and leaves everything that was not hashed yet queued,
  including the file that was being read. A scan, **Add files** or **Resume hashing** later
  continues with the remaining files, oldest first, before the newly added ones.
- **Resume hashing** drains the queue now. **Retry failed** queues the failed files again and
  hashes them. **Clear hash queue** forgets every waiting and failed file; it never touches
  hashed files or the library. All three wait while another task runs.
- **Failed files.** A missing, unreadable or locked file is not retried on its own. It stays in
  the queue as *failed* with a reason ("file not found (moved, renamed or deleted, or its drive is
  unavailable)", or the read error) until you add it again, retry failed files or clear the queue.
- **Start hashing automatically** (Options, Hashing; on by default, imported from AOM's
  `autostartHashing`): when Animatic starts and files are still queued, hashing resumes by
  itself once nothing else blocks it (no import review, restore or settings recovery notice is
  open and no other task is running). With it off nothing hashes at start: the Hashed files page and Overview show the
  pending count with a **Resume hashing** button. Files given on the command line are always
  hashed, together with any older queued files. Resuming is local work and does not depend on
  *Work offline*.
- **Hashing thread priority** (`hashing.priority`) is kept only so AOM settings import and export
  round-trip; hashing always runs at the normal thread priority. There is no cross-platform
  thread-priority control, so none is applied.
