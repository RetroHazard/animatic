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
