Animatic can replace **its own executable** with a newer stable release published at
<https://github.com/RetroHazard/animatic/releases>. It is supported on Windows x86_64 and
Linux x86_64 only; elsewhere the update controls are disabled. Nothing is ever downloaded or
installed unattended. Linux runtime behavior of the updater has not been exercised by the project.

**What counts as an update.** Only a *published, non-draft, non-prerelease* GitHub release whose
tag is a valid `vYYYY.M.PATCH` CalVer version. Animatic reads the release list (not just the
API's first or "latest" entry) and takes the **highest CalVer version**; it is offered only when
it is strictly newer than the running version, so an equal version or a downgrade is never
offered. A release that lacks the expected archive or `SHA256SUMS` asset for your platform is not
offered.

**The steps** (each needs a click; the **Software updates** window shows the installed version
and the current step):

1. **Check.** Help → **Check for updates…** opens the window and starts a check (**Check now** /
   **Check again** repeat it). The startup check does the same work but does not open the
   window: if it finds a newer release, the Log gets a note and an **Update available: VERSION**
   button appears at the right of the menu bar to open the window. A startup check that finds
   nothing or fails stays quiet (a failure is only written to the Log); a manual check always
   reports its result in the window. The window shows the version, the download size, a link to the
   release page on GitHub and the integrity warning described below.
2. **Download and verify.** The archive and `SHA256SUMS` are downloaded with size limits and
   checked against each other (SHA-256), then unpacked into a private temporary folder. **Cancel**
   stops the download. Nothing in the installation folder changes yet. The verified SHA-256 is
   shown, and **Discard download** throws the staged files away.
3. **Install.** **Install** first shows which file will be replaced; **Confirm installation**
   (or **Back**) is the confirmation. Only then are the staged files installed; see below.
4. **Restart now or Later.** **Restart now** saves your settings, closes the window normally and
   starts the new executable afterwards; **Later** leaves the running version untouched and the
   new one starts the next time you launch Animatic. Until you restart, the running (older)
   program does not check for, download or install further updates.

Only one update operation runs at a time, and it runs on its own background thread, so the
AniDB queue, hashing and library refresh are unaffected. Closing Animatic during a check or
download cancels it; closing during the short install step waits for it to finish.

**Automatic startup check.** *Options → Updates → Check for updates at startup*
(`updates.auto_check`, **on by default**) makes one background check when Animatic starts. It
is skipped when *Work offline* is on, when the settings were recovered from a backup (recovery
always starts offline), or when you turned the option off. Because the factory default for
*Work offline* is **off**, a fresh profile contacts GitHub at its first start. To prevent that,
place a settings file that sets `updates.auto_check` to `false` (or `anidb.offline` to
`true`) at `ANIMATIC_SETTINGS` before the first launch; see the
[settings reference](Settings-Reference.md#updates). The check only looks for a version and
never downloads an archive. The Help menu check ignores *Work offline* because you asked for
it explicitly.

**What the install changes.** The executable is replaced using the maintained
[`self-replace`](https://crates.io/crates/self-replace) crate, which performs the operating-system
specific swap (on Windows the running executable is renamed aside and removed after it exits).
Before that swap Animatic writes the new release's license files **next to the executable** as
`LICENSE-vVERSION` and `THIRD-PARTY-NOTICES-vVERSION.html` (for example
`LICENSE-v2026.12.0`). An existing different file of that name, or a symlink, is never
overwritten; an identical file is fine. Everything else is left alone:

- The bundled `LICENSE`, `THIRD-PARTY-NOTICES.html`, `README.md`, `CHANGELOG.md` and `docs/`
  stay at the **old** version. They change only if you replace them from a newly downloaded
  archive yourself. The documentation you are reading may therefore describe an older version
  than the executable.
- Your database, settings, backups, profiles, queue, media and any files you have hashed are
  never touched; an update replaces the program only.
- The new version's licenses are the versioned files next to the executable. After the next
  start, **About Animatic → View license / Third-party notices** opens the versioned file for the
  version you run (`LICENSE-vVERSION`, `THIRD-PARTY-NOTICES-vVERSION.html`) and falls back to the
  older bundled copy only when that file does not exist.
- Installing also creates an empty `.animatic-update.lock` file next to the executable. The
  operating system holds an advisory lock on it only while an install runs, so two Animatic
  processes cannot install at once; the file is harmless, may remain afterwards and can be
  deleted by hand while no Animatic is installing.

**What the installer refuses.** **Install**, **Confirm installation** and **Restart now** are
disabled, with the reason shown, while a task is running (hashing, scan, queue run, import), an
AOM databank import is in progress, imported AOM settings are waiting to be applied or cancelled,
or some changes are not yet saved to the database. **Restart now** is also disabled while the
settings were recovered from a backup and not yet saved (use **Save options** first), and a
failed settings save keeps Animatic open instead of restarting. Animatic also needs to know where it
was installed (the executable path captured at startup) and refuses on a mismatch. Its folder must
be writable by your account: there is **no privilege elevation** and no
administrator or `sudo` prompt. If the folder is read-only (for example a system-wide
installation), update that copy the way you installed it. A copy run from a source
checkout (`target/debug` or `target/release`) can be replaced, but that only changes the compiled
binary, never the Rust source; rebuild or `git pull` instead. A copy installed by a package manager
should be updated by that manager, otherwise the next package upgrade will overwrite or
conflict with the replaced file.

**Restart safety.** Restarting happens only after the window has closed and the library
database and worker threads have been released, so two Animatic processes never use the
same files. The new process is started with only the current `--database` path (as an
absolute path). Original media paths and `--full-update` are **not** repeated, so nothing is
re-hashed or re-queued. The environment is inherited; if you started Animatic with `ANIMATIC_SETTINGS`, the new process receives it as the absolute path that was in use.
If the new executable cannot be started, Animatic exits with an error telling you to start it
manually; nothing is retried.

**Integrity and trust.** The check is that the archive's SHA-256 matches the `SHA256SUMS`
file published in the same release. This detects a corrupted or truncated download and a
mismatched file; it is **not** a digital signature and does not prove who published the
release, because both files come from the same GitHub release and are unsigned. Animatic
displays the verified SHA-256 so you can compare it with another source if you want to. Only
install updates from a release page you trust.

**Privacy.** Update traffic goes to GitHub, not AniDB, and is anonymous. GitHub sees your IP address and
the request's `User-Agent`, which names Animatic and its version. No account, token, credentials,
library data or file names are sent, and there is no token or sign-in setting. See
[Storage and network](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#github-update-transport).

**If something fails.** The window shows the reason. Check, download, verification and most
install refusals (another instance installing, the installed file is not the one this process
started from, a read-only folder, a conflicting different file in place of a versioned license
file) happen **before** the executable is touched: nothing is replaced, the staged download
is dropped and you must press **Download and verify** again. Versioned license files may already
be present next to the executable after such a refusal or a later failure; they are harmless.

If the actual replacement of the executable was entered and failed, Animatic treats the
installation as possibly damaged: it stops offering check, download, install and restart for
the rest of the session, never restarts by itself, and the window says the program files may be in
an unknown state. First check that the executable still starts. On **Windows**, where
`self-replace` is not transactional, Animatic first makes a recovery copy
`.animatic-recovery-<random>.exe` beside the executable and, after a failed swap, restores the old
executable to its original name **only if nothing exists there** (it never overwrites a file
that something else put there). The recovery copy is always kept after a failure; the message
names it and, if the restore did not happen, tells you which file to copy to which name. On
**Linux** the swap is a single atomic rename and the old executable should be unchanged. A
power loss or crash during the swap is not covered: Animatic promises no rollback, and it never
deletes dot-prefixed leftovers (`.animatic-recovery-*`, `.animatic-restore-*`, or helper files
created by `self-replace`) automatically, because one of them may be the only intact copy.
Delete them by hand only after Animatic starts normally from its usual path. If it does not
start, see [troubleshooting](Network-and-Sync-Problems.md#update-problems) and install the archive from the
release page manually as described in [Releases](https://github.com/RetroHazard/animatic/blob/main/docs/releases.md#install-a-release).

## When the release feed is unavailable

Animatic reads GitHub's release feed **anonymously**; there is no token, sign-in or other credential
setting. Only releases that GitHub serves to anyone can be found. If GitHub answers HTTP 404 (which
is also what an anonymous client sees for a repository or releases that are not publicly readable,
and for a repository with nothing published), a manual check reports that the release feed is
unavailable or was not found. It never says "up to date" in that case. The update feature works once
the releases are publicly readable; until then, update by downloading the archive manually
([Releases](https://github.com/RetroHazard/animatic/blob/main/docs/releases.md#install-a-release)). This page does not state whether the repository is
currently public. A rate limit (HTTP 403 or 429) is reported separately and is not retried.
