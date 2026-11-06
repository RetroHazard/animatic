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

How arguments are built (`external.rs`, the `shell-words` crate): placeholders are substituted
into the text **first**, then the whole string is split with POSIX-shell rules (spaces separate
words; `'…'`, `"…"` and `\` quote; `#` starts a comment) and the program is started directly,
without a shell. For a player whose arguments do not contain a path placeholder the file is
**appended as one safe argument**. Consequences:

- Prefer leaving the path placeholder out of player arguments (use flags only): the path is then
  appended as its own unparsed argument, the only fully intact route. AVDump has no such route; its
  arguments always go through substitution and splitting, so its `{path}`, `{user}` and `{key}` values
  are raw text.
- Quoting `{path}` is only a mitigation, not escaping: the value is substituted as raw text
  before splitting. On Windows an unquoted path loses its backslashes, because `\` is an escape
  character in this syntax; inside double quotes backslashes survive except before `"`, `\`, `$`
  and `` ` ``. A path with a double quote can break a quoted token and add arguments (no shell is
  involved, so it cannot run commands, but it can change what the program receives).
- Unbalanced quotes are rejected: `invalid external command arguments: …`.
- `{user}`, `{key}` are inserted unquoted; values with spaces, quotes or backslashes will be
  mis-split.
- Animatic does **not** wait for, capture or import the child's output: the ED2K link
  AVDump prints is not shown or stored. The process inherits the application's output
  streams, and its arguments, including your API key, are visible in the process's command
  line to other local tools [INFERENCE from how processes are started].

More examples and the stored-field details: [settings reference](Settings-Reference.md#external-command-templates).
