How a pattern's result becomes a target path, what blocks a rename, how a rename is applied,
and the stable built-in pattern.

## From result to target path

### Result normalization

`evaluate` converts the final value to text, then **removes every `\r` and `\n`** and turns
every double backslash `\\` into a single `\`. The `Evaluation` also reports:

- `boolean_result`: the final value was a Boolean (`result` then holds `True` or `False`),
- `variables`: every variable after the run, as text (not shown by the application),
- `warnings`: the soft errors.

### Planning rules

`plan::plan` applies these checks in order and stops at the first failure:

1. The pattern must not be blank and must compile. Otherwise one error, no result.
2. If the pattern is the [stable default](#the-stable-default-pattern), the anime title must be
   known and the episode number positive (see below).
3. The pattern runs. A hard error becomes `The renaming pattern failed to run: <message>`.
4. A **Boolean final value is an error**: `The pattern ended with the truth value "True"
   instead of a file name. Make its last expression the new name (for example a variable
   holding it).` Examples: `1 = 1`, a comment-only pattern, a bare unknown name.
5. An **empty or blank result is an error**: `The pattern produced an empty file name;
   nothing would be renamed. Its last expression must evaluate to the new name.` Examples:
   `''`, `set('x', 'y')`, `if(1 = 2, 'x.mkv')`.
6. The text is turned into a path (next sections). A bad component is an error.
7. A path equal to the current path means "already named": no target, no error.

Warnings do not stop the steps above, but any warning makes the plan non-applicable.

**Metadata rule.** Only the stable default insists on fetched metadata: an anime title that is
non-blank and not the `[Title not yet available]` stand-in, and an episode number greater than
zero. Failing either gives
`The stable default needs the anime title, which has not been fetched yet. Fetch the anime's
information from AniDB first.` or
`The stable default needs the episode number, which is not known yet. Fetch the episode's
information from AniDB first.` Every other pattern is judged only by its own result. **A
custom pattern that uses `A.Name` will happily produce `[Title not yet available]` in a file
name.** Guard it if that matters:

```
set('title', A.Name)
// ... build 'filename' from title ...
if(title = '[Title not yet available]', '', filename)
```

The last line yields an empty string for a pending title, and the plan reports the empty-name
error instead of renaming.

### Base path and absolute or relative results

`rename::target_path(old, result, base_path)` is a pure function over strings. Both `\` and
`/` are separators on every platform.

1. A blank result is an error (above).
2. If `base_path` is not empty, the result is **always appended to it**: the base's trailing
   separators are trimmed, then the result is joined. A result that starts with a separator is
   joined as is; otherwise the platform's separator is inserted.
3. The joined text is split into a root and components:
   - `\\server\share\rest`: a UNC root,
   - `X:` followed by anything (`X` any single ASCII letter, so also `a:`): treated as a drive root on every platform, with `rest` as components. **A result such as `a:b.mkv` is therefore accepted on Windows as drive `a:` plus the file `b.mkv`** (target `a:\b.mkv`), not rejected for its colon. Use a longer prefix (`bad:name.mkv`) to see the colon rule.
   - a single leading `\` or `/`: the root of the **current file's** drive (on POSIX, `/`),
   - anything else: **relative**, resolved against the file's own folder.
4. Every component is trimmed of trailing dots and spaces, then of all characters at or below
   U+0020 at both ends, then validated.
5. The components are pushed onto the root. If the path equals the current path, the result is
   "already named".

`base_path` comes from `manager_for`: the file manager whose folder is the longest match for the
file (subfolder inclusion respected), otherwise the default manager. This longest-match rule
is specific to the rename destination and post-hash MyList policy; scan discovery of
hash/rehash candidates uses a different rule (first matching rule in list order). The default
entry has an empty base path, and an empty base path keeps the file in its current folder.
How managers match is in the [Settings reference](Settings-Reference.md).

| Base path | Result | Target |
|---|---|---|
| *(empty)* | `new.mkv` | `<folder of the file>\new.mkv` |
| *(empty)* | `Show\new.mkv` or `Show/new.mkv` | `<folder of the file>\Show\new.mkv` (the subfolder is created on apply) |
| *(empty)* | `D:\Anime\new.mkv` | `D:\Anime\new.mkv` |
| *(empty)* | `\Anime\new.mkv` | `<drive of the file>\Anime\new.mkv` |
| *(empty)* | `\\server\share\dir\new.mkv` | that UNC path **on Windows**. On POSIX the backslashes are ordinary characters, so the "root" `\\server\share\` is a relative path and the target resolves against the process's current working directory, not a network share or the file's folder. |
| `D:\Anime` or `D:\Anime\` | `Show\new.mkv` | `D:\Anime\Show\new.mkv` |
| `D:\Anime` | `\Show\new.mkv` | `D:\Anime\Show\new.mkv` |
| `Anime` (relative) | `new.mkv` | `<folder of the file>\Anime\new.mkv` |
| `D:\Anime` | `E:\Other\new.mkv` | **error**: the joined text contains the component `E:`, which has a colon |

An absolute result and a non-empty base path therefore do not combine; with a base path every
result must be relative. A relative base path is relative to the file's folder, not to the
program's working directory. Drive-letter roots are recognized on every platform; on POSIX
such a name is not meaningful (reading of the code: it would become a relative path whose first
component is `D:`).

### Component validation

Checked for each component, on **every platform**, so a library stays portable. The first
problem found is reported as
`Renaming "<old path>" to "<joined name>" would result in an invalid file name (<reason>);
rename skipped!`

| Rule | Detail |
|---|---|
| Not empty | `it contains an empty folder or file name`: doubled separators (`a//b`), a trailing separator, and the components `.` and `..` (they trim to empty) are all rejected |
| Forbidden characters | `<`, `>`, `:`, `"`, `?`, `*` and the vertical bar are rejected (`"<char>" is not allowed in a name`). The colon of a leading single-letter drive prefix is exempt because the root is split off first, which is why `a:b.mkv` is accepted as drive `a:` on Windows while `bad:name.mkv` is rejected. |
| Control characters | Characters below U+0020 inside a name are rejected (`it contains a control character`); at the ends they are trimmed first. U+007F is not checked. |
| Reserved device names | `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`, compared case-insensitively against the text before the **first dot** with trailing spaces removed, so `NUL.mkv` and `Com3.tar.gz` fail while `COM10.mkv` and `COM0` do not. The superscript digits `¹ ² ³` count as the digit for `COM`/`LPT`. |
| Length | At most **255 per component**: in **UTF-16 code units** when the build targets Windows, in **UTF-8 bytes** otherwise. 130 `é` characters pass on a Windows build (134 units with `.mkv`) and fail on a POSIX build (264 bytes). The platform is fixed when the program is compiled. |

Not checked: the total path length (Windows `MAX_PATH`), name collisions that differ only in
case, Unicode normalization, or whether a folder exists. Nothing is replaced silently: a
character that is not allowed makes the rename fail with a message. Sanitizing is the
pattern's job, as the stable default does. A trailing dot or space is the exception: it is
removed from each component, so `Show.` becomes `Show`.

Because the default pattern limits titles by **characters** (`limit(…, 90)`, `200`, `235`),
a title made of multi-byte characters can still exceed 255 **bytes** on a POSIX build, and the
plan then reports a length problem. This is a reading of the code; Linux was not exercised.

## Plan, blocking rules and apply

| Field of `Plan` | Meaning |
|---|---|
| `result` | The evaluated text, even when the plan is blocked later (empty when the pattern never ran) |
| `target` | The new path, or `None` when an error stopped planning before a path was built or when the file already has that name. **A target can be present while warnings exist** (for example `lc(5) + '.mkv'`); it is then only the path the pattern would produce. Treat `can_apply()` as the approval, never the presence of a target. |
| `warnings` | Soft evaluation problems |
| `errors` | Hard problems: bad pattern, metadata gap, failed run, boolean/empty result, invalid target, missing database rows |

| Method | True when |
|---|---|
| `can_apply()` | a target exists, **and** there are no errors, **and** there are no warnings |
| `already_named()` | no target, no errors and no warnings: the pattern is sound and the file already has that name |

What each outcome looks like in the application:

| Situation | Preview dialog | **Rename file** |
|---|---|---|
| Target, no errors or warnings | `New path` | Enabled |
| Target but warnings | `Path the pattern would produce (rename blocked)` plus the warnings | Disabled |
| Errors (any) | The errors in red; no target path is shown | Disabled |
| Already named | `The file already has the name the pattern produces; nothing to rename.` | Disabled |

With errors or warnings the dialog offers **Edit pattern in Options**. When **Rename…** is used
the log also records `Rename not possible: <first error>` or
`Rename blocked: the renaming pattern reported warnings`. Before the dialog opens, the selected
row must still exist and be identified by AniDB (`The selected file is not identified by
AniDB`, `The selected file is no longer in the list`); otherwise nothing is shown.

The **Rename…** dialog always uses the pattern text currently held in memory by the Options
editor. Options edits take effect as you type: they are not held back until **Save options**,
and the edited text is written to `settings.json` at the next explicit save or on a normal
exit (the exit save is skipped only while the settings-recovery guard is active). See the
[Settings reference](Settings-Reference.md) for the exact persistence rules.

On confirmation the application submits a **rename job** to the background worker with the
planned path (see [The three stages](Rename-Language.md#the-three-stages)); nothing is moved on
the interface thread. For each file the job calls `move_file(old, target)`: on success it logs
`Renamed <old> to <new>` and refreshes the library list; on failure it logs `Rename failed: <old>:
<reason> (target <new>)` and the file and its library path are unchanged.

### How a file is moved

`move_file` first checks that the library still records `<old>` for that file and that
`<target>` does not exist (a dangling link counts as existing), and creates missing parent
folders. Then:

| Case | Behavior |
|---|---|
| Same volume | One operating-system rename, then the stored path is updated. If the update fails the rename is reverted, so the library never names a missing file. |
| The rename fails because the volumes differ (`EXDEV` on Linux, `ERROR_NOT_SAME_DEVICE` on Windows) | The file is copied to `.animatic-move-*.tmp` in the target folder (keeping its modification time and permissions); the copy's size is compared; the copy is **re-hashed in one pass with the same ED2K variant the library uses and compared with the stored ED2K and size**; the temporary file is renamed into place without replacing anything; the library path is updated; only then is the source deleted. |

The verification read is deliberate: it doubles the I/O of a cross-volume move but is the only
proof that the copy is the file the library knows. A file without a stored hash, or whose size
differs from the stored size, is refused before anything is copied. Cancel acts while copying and
verifying. Every failure before the swap removes the temporary file and leaves source and library
untouched; a failed library update removes the new copy again. If the source cannot be deleted
after a successful swap, the move still counts, the library points at the copy, and the Log says
the old file remains.

### Batch rename

The batch preview calls `plan_local_file` for every selected file on the background (projection)
thread, in table order, and adds two checks that need the whole list: a target that already
exists on disk is an error (`A file or folder already exists at the target path`), and a target
an earlier row already claims is an error (`<path> would get the same name; only the first of
the two can be renamed`; compared case-insensitively on Windows). A file that AniDB has not
identified is blocked with `The file is not identified by AniDB`. One confirmation submits only the rows for which
`can_apply()` is true; blocked and already-named rows are listed but never sent. A row's outcome
is `Renamed`, `Renamed (copied and verified)` or `Not renamed: <reason>`; a collision with a
file created after the preview is reported as `destination already exists`. Chains inside one
batch (file A to the current name of file B) are blocked, not ordered.

## The stable default pattern

The text is the file `crates/animatic-core/src/rename/default_pattern.txt`; the constant
`settings::DEFAULT_RENAME_PATTERN` includes it. This listing is a copy for reading; the file is
the source of truth.

```
set('atitle', at('x-jat'))
if(length(atitle)=0, set('atitle', at('en')))
if(length(atitle)=0, set('atitle', A.Name))
set('atitle', replace(atitle, '\', ' '))
set('etitle', et('en'))
if(length(etitle)=0, set('etitle', et('x-jat')))
if(length(etitle)=0, set('etitle', et('ja')))
if(length(etitle)=0, set('etitle', E.Name))
set('etitle', replace(etitle, '\', ' '))
set('version', if(F.Version>1, 'v'+F.Version, ''))
set('epno', padl(E.EpisodeNo, 2, '0') + version)
if(E.TypeId=2, set('epno', 'S'+epno))
if(E.TypeId=3, set('epno', 'C'+epno))
if(E.TypeId=4, set('epno', 'T'+epno))
if(E.TypeId=5, set('epno', 'P'+epno))
if(E.TypeId=6, set('epno', 'O'+epno))
set('groupname', if(G, if(length(G.Shortname)>0, G.Shortname, G.Name), 'no group'))
set('groupname', if(length(groupname)>0, '['+groupname+']'))
set('groupname', replace(groupname, '\', ' '))
set('crc', if(H, H.Crc32, F.Crc))
if(length(crc)>0, set('crc', '('+uc(crc)+')'))
set('filename', limit(limit(limit(atitle, 90) + ' - ' + epno + ' - ' + etitle, 200) + ' ' + groupname, 235) + crc + '.' + F.FileType)
set('filename', replace(filename, '*', ' '))
set('filename', replace(filename, '/', ' '))
set('filename', replace(filename, '?', ' '))
set('filename', replace(filename, ':', ' '))
set('filename', replace(filename, '"', ' '))
set('filename', replace(filename, '<', ' '))
set('filename', replace(filename, '>', ' '))
set('filename', replace(filename, '|', ' '))
set('filename', replace(filename, '`', "'"))
set('filename', replace(filename, '  ', ' '))
filename
```

What it does:

| Part | Behavior |
|---|---|
| Series title | `x-jat` title, else English, else `A.Name`; a backslash becomes a space |
| Episode title | English, else `x-jat`, else Japanese, else `E.Name`; backslash becomes a space |
| Episode number | `E.EpisodeNo` left-padded with `0` to **at least 2 digits**, followed by `vN` when the file version is above 1; special types add a prefix `S`, `C`, `T`, `P` or `O` in front (for example `S01v2`) |
| Group | `[Shortname]`, else `[Name]`; with no group the name is `no group`, giving `[no group]`; a group whose short and full names are both empty contributes no group text |
| CRC | `H.Crc32` when a local hash exists, else `F.Crc`, upper-cased in parentheses; none when both are empty |
| Name | `<title> - <number> - <episode title> <group><crc>.<extension>`, limited to 90 characters for the title, 200 up to the episode title, and 235 up to the group |
| Clean-up | `* / ? : " < >` and the vertical bar become spaces, `` ` `` becomes `'`, and each pair of adjacent spaces becomes one space |

Two details of the clean-up are easy to miss. The double-space replacement is a single pass, so
three consecutive spaces become two, not one. Titles containing `\` are turned into spaces
because the result would otherwise create folders.

For the synthetic sample used in [Working patterns](Rename-Patterns.md#working-patterns) the default produces:

```
Example Series - 03v2 - The Long Road [ExGroup](EB420C97).mkv
```

### Fixed two-digit counter

The built-in pattern pads episode numbers to a **fixed minimum of two digits**
(`padl(E.EpisodeNo, 2, '0')`), independent of any cached episode totals or per-type counts.
Numbers of 100 or more keep all their digits (`100`, `1234`) and type prefixes are preserved
(`S01`, `C12`, `O120`). The same episode therefore keeps the same name when AniDB totals change.
The rename unit tests check this for several totals and counts.

### New patterns versus saved patterns

| Situation | Pattern text |
|---|---|
| New settings (no settings file) | The stable default |
| A saved `settings.json` that contains `renaming_pattern` | Kept exactly as saved. The application never rewrites a saved pattern. |
| A saved file without `renaming_pattern` | The default fills the missing field |
| Imported AOM settings with a non-blank `general/renamingPattern` | That text, with `\r\n` converted to `\n`, replaces the pattern |
| Imported AOM settings with a blank pattern | The existing/default pattern stays |
| **Use stable default** in Options | Replaces the editor text with the default; the replaced text is kept in memory |
| **Restore previous text** (appears after **Use stable default**) | Puts the replaced text back; used once |

**Use stable default** is enabled only when the current text is not already the default. The
hover text in the application says nothing is stored until **Save options**; in the
implementation the edit changes the in-memory settings immediately, like every other Options
field, and is written by **Save options**, by a normal exit, or later. The editor classifies
the text with `classify`:

| Kind | When |
|---|---|
| Empty | blank |
| Stable default | the lines equal the default's lines (the default is compared after trimming the whole text; CRLF versus LF makes no difference) |
| Custom | anything else, including a one-character edit of the default |

A **Custom** pattern, including an old default whose counter width depended on cached totals,
is never altered by the application.
