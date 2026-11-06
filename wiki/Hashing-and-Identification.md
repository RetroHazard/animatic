**Adding files.** *File → Add files…*, the Hashed files page, the Overview button, or media paths on the
[command line](Running-Animatic.md#command-line-reference). The picker's extension filter is
only a convenience; command-line paths are never filtered. Each file is read **once**; ED2K
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
4. A read error (missing file, permission, a directory) is logged as `<path>: <error>`; the
   remaining files still run. Cancel stops the whole batch.

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

"Start hashing automatically" is stored only: hashing begins as soon as files are chosen or
passed on the command line.
