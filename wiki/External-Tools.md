**Players** (Options → External player): per player *Name*, *Executable*, *Open arguments*, *Play
arguments*, *Enqueue arguments* and *Default player*; **Add player** and **Remove player** manage the
list. On the Hashed files page, with a file selected:

- **Play** starts the chosen player with its *Play arguments* (always offered).
- **Open in player** and **Enqueue** start it with its *Open* or *Enqueue arguments*. They appear
  only for a player whose template for that action is not blank, so a player that only plays shows
  just **Play**.
- With **several players** configured, a chooser next to **Play** picks the player for all three
  actions (the *Name* of each player; an unnamed one is "Player N"). It starts on the player marked
  default, else the first, and goes back to that when the chosen player is removed. The choice
  lasts for the session and is not saved.
- Right-clicking a file name opens a menu with the same actions (**Open**, **Play with …**, **Open
  in …**, **Enqueue in …**) for the chosen player; a right click on a file outside the selection
  selects it first.

**Open** (the plain button and the menu entry of that name) and a double-click on a path use the OS
default program, not a configured player. An empty executable gives `Could not start player: player
executable is not configured`; an action whose template is blank gives `this player has no open
arguments configured`.

**AVDump** (Options → AVDump): *Enable AVDump integration*, *Executable*, *Arguments*, *AVDump API
key*. The **AVDump** button needs the integration enabled and a selected file. The default
arguments are:

```
--Consumers=ED2K --Auth={user}:{key} --LPort={port} --PrintEd2kLink "{path}"
```

Placeholders: `{path}` (the file), `{user}` (the AniDB username from Options), `{key}` (the
AVDump API key) and `{port}` (the AniDB local UDP port). Players also accept `%file%` for the path.

How arguments are built (`external.rs`, the `shell-words` crate): the template is split into
words with POSIX-shell rules first (spaces separate words; `'…'`, `"…"` and `\` quote; `#` starts
a comment), then the placeholders are substituted inside each word, and the program is started
directly, without a shell. For a player whose arguments do not contain a path placeholder the
file is **appended as one argument**. Consequences:

- A path, username or key with spaces, quotes or option-like text always arrives as part of the
  one argument that held its placeholder; it cannot add or split arguments. Quoting a
  placeholder is unnecessary.
- Backslashes typed into the template itself are escape characters unless inside double quotes;
  substituted values are never affected.
- Unbalanced quotes are rejected: `invalid external command arguments: …`.
- Animatic does **not** wait for, capture or import the child's output: the ED2K link
  AVDump prints is not shown or stored. The process inherits the application's output
  streams, and its arguments, including your API key, are visible in the process's command
  line to other local tools [INFERENCE from how processes are started].

More examples and the stored-field details: [settings reference](Settings-Reference.md#external-command-templates).

## Media properties (ffprobe)

Animatic can read the duration, resolution, frame rate, codecs, sample rate and bitrates of your
hashed files with **`ffprobe`**, the media-inspection tool of [FFmpeg](https://ffmpeg.org/). It is
**optional and off by default**: nothing is read and no process is started until you enable
*Read media properties after hashing* in Options → Media properties, and that checkbox stays
disabled until an `ffprobe` is found. The values fill the media tokens of the
[hash export](Exports.md#hash-export) (`%fps%`, `%length%`, `%height%`, `%width%`, `%vcodec%`,
`%vbitrate%`, `%acodec%`, `%asample%`, `%abitrate%`); they are not shown elsewhere and do not
change the [rename objects](Rename-Functions-and-Objects.md).

**Where Animatic looks** (Options shows the version it found, which of the places it came from,
and its path):

| Order on Windows | Order on Linux |
|---|---|
| 1. The *ffprobe path* in Options (an executable, or the folder that holds it) | 1. The *ffprobe path* in Options |
| 2. Animatic's own copy, `tools\ffprobe\ffprobe.exe` in the [data folder](Data-Locations.md#where-data-lives) | 2. An `ffprobe` on `PATH` (a distribution package is preferred when present) |
| 3. An `ffprobe` on `PATH` | 3. Animatic's own copy, `tools/ffprobe/ffprobe` |

A candidate counts only if `ffprobe -version` answers with an `ffprobe version …` line. A path in
Options that is not usable is reported under the version line and the search goes on. The search
runs in the background when Animatic starts, with **Detect**, and after **Save options** changed
the path.

**Install ffprobe…** downloads a pinned FFmpeg build for you, after you confirm. The
confirmation shows the build, the download size (about 161 MB on Windows and 130 MB on Linux),
the license, the source page and the install folder, and the install does the following:

- Contacts `github.com` and GitHub's download servers only (not AniDB), anonymously: GitHub sees
  your IP address and `Animatic/<version>`; nothing about your library is sent. It works in offline
  mode because you asked for it. The download is bounded in size and time, and **Cancel
  download** stops it.
- The build is [BtbN's FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds) static **LGPL** build
  (no GPL components; the license text is GNU LGPL version 3). Animatic checks the exact size
  and the SHA-256 checksum of the download against values built into the program and refuses
  anything else before it opens the archive. That proves it is the pinned file; it does not
  authenticate the publisher.
- It keeps only `ffprobe` (about 126 MB unpacked on Windows, 134 MB on Linux) and the FFmpeg
  license, `LICENSE-ffmpeg.txt`, in `tools/ffprobe/` of the data folder. It needs no administrator
  rights, and it never touches `PATH`. The release archives of Animatic do **not** contain `ffprobe`.
- Installing again replaces the two files. To remove the tool, delete the `tools/ffprobe` folder
  (and untick the option). A newer pinned build arrives with a newer Animatic; installing again
  then upgrades the copy.
- On Linux the button stays available even when a distribution `ffprobe` exists, which is
  preferred over the download.

**When files are read.** After a hashing or scan job finishes, a separate background job probes
the files that job hashed (progress shows as *Reading media properties*; **Cancel** stops it).
Hashing itself never reads the file a second time for this. Files that were hashed before you
enabled the option are read with **Read missing properties now**, which probes every hashed file
without figures for its current content. Each probe runs `ffprobe` directly (no shell) with a 60
second limit and a bounded output, and may open only local files: a playlist that points at a
network address is not followed.

- A file `ffprobe` cannot read is logged (*Media properties of …: ffprobe could not read the
  file*) and recorded as tried, so it is not tried again. A file that is not there (for example an
  unplugged drive), a timeout or an unstartable `ffprobe` are logged but not recorded.
- The figures belong to the file's content: renaming or moving a file keeps them, hashing the
  file again to different content discards them until it is read again, and removing the file
  from the library removes them.
- A value `ffprobe` does not report (no audio stream, unknown bitrate) stays empty; the export
  then keeps that token's constant. Cover art is not treated as video.
