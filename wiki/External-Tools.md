**Players** (Options → External player): *Name*, *Executable*, *Play arguments*, *Default player*.
**Play** uses the player marked default, else the first. *Add player* appears only while the list
is empty; imported settings may supply more players. The stored *open* and *enqueue* templates
have no screen and are never used. **Open** on the Hashed files page and a double-click on a path
use the OS default program, not a configured player. An empty executable gives `Could not start
player: player executable is not configured`.

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
