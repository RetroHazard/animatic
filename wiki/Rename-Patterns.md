Worked patterns, common pitfalls, every diagnostic, self-test patterns, and how to check a
pattern against one of your own files before applying it.

## Working patterns

The examples use fictitious data only. **Reference sample:**

| Item | Value |
|---|---|
| `A.Name` | `Example Series` (also its `x-jat` main title) |
| `E` | regular episode (`TypeId` 1), `EpisodeNo` 3, English title `The Long Road` |
| `F` | `Version` 2, `Crc` `eb420c97`, `FileType` `mkv` |
| `G` | `Shortname` `ExGroup`, `Name` `Example Group` |
| `H` | `Crc32` `EB420C97` |

Results below are the text the pattern evaluates to; the path rules then apply. All examples
are meant to be validated with the steps in [Verify a pattern](#verify-a-pattern).

### Unpadded, custom

```
A.Name + ' - ' + E.EpisodeNo + '.' + F.FileType
```

`Example Series - 3.mkv`. Padding to a fixed width is `padl(E.EpisodeNo, 3, '0')`, giving
`Example Series - 003.mkv`.

### Dynamic version suffix

```
set('ver', '')
if(F.Version > 1, set('ver', ' v' + F.Version))
A.Name + ' - ' + padl(E.EpisodeNo, 2, '0') + ver + '.' + F.FileType
```

`Example Series - 03 v2.mkv`; with `Version` 1 it is `Example Series - 03.mkv`. Initializing
`ver` on the first line matters: an unset variable would read as `False` and add the word
`False` to the name. The default pattern uses the three-argument `if` for the same reason.

### Unicode

```
A.Name + ' 第' + padl(E.EpisodeNo, 2, '0') + '話 ' + E.Name + '.' + F.FileType
```

With a fictitious anime title `星空の旅` and episode title `長い道`, the result is
`星空の旅 第03話 長い道.mkv`. Non-ASCII characters are fine **inside string literals and in
data**; they are never valid as bare identifiers or operators. Names are Unicode all the way
(no code page conversion), and length rules count UTF-16 units or UTF-8 bytes, see
[Component validation](Rename-Targets-and-Apply.md#component-validation).

### Folder, sanitizer and a user function

For an anime titled `Example Series: Voyage`:

```
function('clean', replace(replace(replace(param1, ':', ' -'), '/', ' '), '?', ''))
set('series', clean(A.Name))
set('folder', series + '\')
set('ep', padl(E.EpisodeNo, 2, '0'))
folder + series + ' - ' + ep + '.' + F.FileType
```

`Example Series - Voyage\Example Series - Voyage - 03.mkv`. With no base path the new folder
is created inside the file's current folder; with a base path of `D:\Anime` it is
`D:\Anime\Example Series - Voyage\…`. Without `clean`, the colon in the title would be rejected
(`":" is not allowed in a name`).

### Optional group and CRC

```
set('grp', '')
if(G, set('grp', ' [' + if(length(G.Shortname) > 0, G.Shortname, G.Name) + ']'))
set('crc', '')
if(H, set('crc', ' (' + uc(H.Crc32) + ')'))
A.Name + ' - ' + padl(E.EpisodeNo, 2, '0') + grp + crc + '.' + F.FileType
```

`Example Series - 03 [ExGroup] (EB420C97).mkv`. Without a group the group part vanishes; without
a local hash the CRC part vanishes. `G.Shortname` is only read when `G` is bound.

### Specials in a subfolder

```
set('sub', '')
if(E.IsSpecial, set('sub', 'Specials\'))
sub + A.Name + ' - ' + E.EpisodeStr + '.' + F.FileType
```

A regular episode 3 gives `Example Series - 3.mkv`; special 1 gives
`Specials\Example Series - S1.mkv`. (`'Specials\'` is a valid literal because strings have no
escapes.) `E.EpisodeStr` is unpadded; `E.EpisodeStrPadded` would change with the stored episode
counts.

### Using extra titles and a pending-title guard

```
set('t', at('en'))
if(length(t) = 0, set('t', at('x-jat')))
set('filename', t + ' - ' + padl(E.EpisodeNo, 2, '0') + '.' + F.FileType)
if(length(t) = 0, '', filename)
```

Prefers the English title, falls back to the romaji title, and returns an empty result (which
blocks the rename with the empty-name error) when neither exists.

## Pitfalls

Each item below follows from the implementation.

1. **Unknown names evaluate to `False`.** A misspelled variable, property or function does not
   fail; it yields the Boolean `False`, and joined with text it becomes the word `False`:
   `filenmae + '.mkv'` evaluates to `False.mkv`. The plan **accepts** that name, because the
   result is text. Only a result that is *entirely* a Boolean is rejected (`1 = 1`, or a final
   line such as `filenmae`). Always validate on a real file and read the result.
2. **Truthiness depends on type.** Literals, `limit`, `padl`, `substr`, `trim`, `split`, `join`,
   `sc`, `tc`, the `at`/`et` titles, user function parameters and text properties such as
   `A.Name` are falsy. `if('abc', 'a', 'b')` is `b`. Test `length(x) > 0` or compare
   explicitly. `count`, `strtoint` and Int64 properties (`A.Size`, dates) are falsy as well.
3. **`+` is right-associative.** Trailing numbers are added: `'S' + E.EpisodeNo + F.Version`
   (episode 3, version 2) is `S5`, not `S32`. Put a string at the end, or convert one number
   to text first (`'S' + padl(E.EpisodeNo, 1, '0') + F.Version` is `S32`).
4. **`not` binds to the next operand only**: `not 1 = 2` is `(not 1) = 2`. Write
   `not (1 = 2)`.
5. **Doubles cannot be text arguments.** Number literals and arithmetic results warn when
   passed to `lc`, `uc`, `padl(x, …)`, `replace` and similar, and any warning blocks the rename.
6. **Comparisons across types fail hard.** `A.Year = 2010` and comparing a mistyped property
   (`False`) with a string stop the pattern. Compare same-typed values.
7. **Whole-line comments only.** A trailing `// note` is a syntax error. A line ending with `_`
   is joined to the next line with no space; a name ending in `_` at the end of a line will
   be swallowed as a continuation.
8. **Tabs between tokens are syntax errors.** Indent with spaces, or not at all (leading tabs
   are trimmed).
9. **Every line is part of the name.** A stray line like `E.EpisodeNo > 1`, a lone
   `A.Name` or an unset variable adds `True`, `False` or text to the result. Use `set(…)` or
   `if(cond, set(…))` for work, and let only the last line produce the name.
10. **Unbound objects are hard errors.** `G.Shortname` for an ungrouped file stops the pattern
    (`…Attribute Shortname parent object not found!`). Guard with `if(G, …)`.
11. **User function parameters are falsy `WStr`s**, and a function must be defined before it is
    called; a call before the definition yields `False`.
12. **`replace` runs once.** Three spaces become two when replacing `'  '` with `' '`;
    `replacerepeat` does not change that.
13. **`lc` and `uc` are ASCII-only**, and `tc` capitalizes after spaces only.
14. **Sanitizing is manual.** `: ? * " < > |` in a result make the plan fail, with no automatic
    substitution. A backslash or slash in a *title* creates folders.
15. **An empty `F.FileType` loses the extension.** The result ends with `.`, which is trimmed.
16. **Custom patterns accept the pending-title stand-in.** See the guard under
    [Planning rules](Rename-Targets-and-Apply.md#planning-rules).
17. **`E.EpisodeStrPadded` is data dependent.** Its width follows the highest stored episode
    number of the type, so a fixed `padl(E.EpisodeNo, 2, '0')` is more stable.
18. **`H` media properties are mostly empty** for files Animatic hashed itself.
19. **`getanime` rebinding is ignored**; the first binding of a name wins.
20. **`for` bodies must produce strings**, and unset accumulator variables read as `False`.
21. **No iteration or recursion limit.** A runaway loop or self-calling function is not stopped
    by the evaluator.

## Diagnostics reference

| Where it appears | Text (abridged) | Cause | Fix |
|---|---|---|---|
| Editor and preview | `The renaming pattern is empty. …` | Blank pattern | Write a pattern or use **Use stable default** |
| Editor and preview | `The renaming pattern has a syntax error: …` | See [Compile errors](Rename-Language.md#compile-errors) | Fix the construct near the quoted position |
| Preview | `The stable default needs the anime title, which has not been fetched yet. …` | Title missing or the stand-in | Fetch the anime from AniDB |
| Preview | `The stable default needs the episode number, which is not known yet. …` | Episode number 0 or unknown | Fetch the episode |
| Preview | `The renaming pattern failed to run: <message>` | Hard error (division by zero, bad conversion, unbound object, …) | Fix the expression; guard optional objects |
| Preview | `The pattern ended with the truth value "True"/"False" instead of a file name. …` | The whole value is a Boolean | Make the last line the name |
| Preview | `The pattern produced an empty file name; nothing would be renamed. …` | Empty result | Make the last line produce the name |
| Preview | `Renaming "<old>" to "<name>" would result in an invalid file name (<reason>); rename skipped!` | [Component validation](Rename-Targets-and-Apply.md#component-validation) | Sanitize in the pattern |
| Preview (yellow) | `The pattern reported warnings, so its result cannot be trusted. …` followed by `PARSERUN: …` lines | Wrong argument count, `Double` as text, bad `set` name | Fix each listed call |
| Preview | `No file entry for file <fid>!`, `Can't autorename file, Episode <eid> is missing for File <fid>!`, `Can't autorename file, Anime <aid> is missing for File <fid>!` | Library rows missing | Identify the file; fetch the anime/episode |
| Preview | `destination already exists: <path>` (as `Rename failed: …`) | Apply found an existing destination | Change the pattern or move the other file |

## Self-test patterns

These patterns do not depend on the selected file's metadata. Paste one into the pattern
editor, choose an AniDB-identified hashed file and press **Validate on selected file**. The
expected values come from reading the implementation. **Pasting replaces your pattern in the
live settings**, and a normal exit saves them, so copy your own pattern text somewhere first
and put it back (or use **Restore previous text** if you pressed **Use stable default**)
before closing the application.

| Pattern | Expected result | Plan |
|---|---|---|
| `'x' + 1 + 2` | `x3` | valid (no extension) |
| `'x' + 1 + 2 + '.mkv'` | `x12.mkv` | valid |
| `10 - 3 - 2` | `9` | valid |
| `'a' + 5 div 2` and `'n' + 7 mod 4` | `a2` and `n3` | valid |
| `if('abc', 'yes', 'no') + '.mkv'` | `no.mkv` | valid |
| `if(lc('abc'), 'yes', 'no') + '.mkv'` | `yes.mkv` | valid |
| `if(not 1 = 2, 'a', 'b') + '.mkv'` | `b.mkv` | valid |
| `if(not (1 = 2), 'a', 'b') + '.mkv'` | `a.mkv` | valid |
| `if(count('aaa', 'aa') = 2, 'ok', 'no') + '.mkv'` | `ok.mkv` | valid |
| `if(count('aaa', 'aa'), 'y', 'n') + '.mkv'` | `n.mkv` | valid |
| `replace('a  b   c', '  ', ' ') + '.mkv'` | `a b  c.mkv` | valid |
| `limit('abcdefgh', 5, '~') + '.mkv'` | `abcd~.mkv` | valid |
| `padl('7', 3, '0') + '.mkv'` | `007.mkv` | valid |
| `tc('hELLO wORLD') + ' ' + sc('hELLO wORLD') + '.mkv'` | `Hello World Hello world.mkv` | valid |
| `uc('abcé') + '.mkv'` | `ABCé.mkv` | valid |
| `for('i', 1, 3, 'x' + i) + '.mkv'` | `x1x2x3.mkv` | valid |
| `unknownname + '.mkv'` | `False.mkv` | **valid**: the engine cannot tell |
| `'ok.'` | `ok.` | valid; the target has no trailing dot |
| `1 = 1` | `True` | error: truth value |
| `if(1 = 2, 'x.mkv')` | empty | error: empty file name |
| `set('x', 'y')` | empty | error: empty file name |
| `lc('A', 'B') + '.mkv'` | warning `PARSERUN: Function lc: Lc must have 1 parameter, found 2 parameters.` | blocked |
| `lc(5) + '.mkv'` | warning `PARSERUN: Syntax error, value can not be converted to a string.` | blocked |
| `'bad:name.mkv'` | `bad:name.mkv` | error: `":" is not allowed in a name`. (`'a:b.mkv'` is **not** an error on Windows: `a:` is read as a drive root and the target is `a:\b.mkv`.) |
| `'NUL.mkv'` | `NUL.mkv` | error: reserved device name |
| `'a//b.mkv'` | `a//b.mkv` | error: empty folder or file name |
| `1 / 0` | none | error: `The renaming pattern failed to run: Division by zero` |
| `'a' // note` | none | syntax error: `Right part of expression missing!` |
| `'x' + 'y' & 'z'` | none | syntax error: `Missing right parentesis!` |

Multi-line patterns (enter each on separate lines):

```
function('f', param1 + '!')
f('hi') + '.mkv'
```

Expected `hi!.mkv`.

```
// comment
'a' + _
'b' + '.mkv'
```

Expected `ab.mkv`.

## Verify a pattern

1. Open **Hashed files** and select a file that AniDB has identified (an identified file with
   its anime, episode and file rows in the library). Anime and episode details must be
   fetched for the stable default to be usable.
2. Open **Tools → Options → Renaming pattern**. The status line shows whether the text is
   Empty, Stable default or Custom, and a red syntax error appears while the text does not
   compile.
3. Press **Validate on selected file**. It runs the same planning as **Rename…** without
   touching any file: the evaluated result, red errors, yellow warnings, and, when the plan
   could be applied or the file already has that name, `The pattern is valid for this file.`
4. Only **Hashed files → Rename…** followed by **Rename file** moves the file. Check the
   path in the dialog first; the new path and `Current path` are both shown in full.

Editing the text clears the previous validation result. Validation uses the pattern text
currently in the editor, saved or not.

The rename unit tests (`crates/animatic-core/src/rename/tests.rs`, plus tests in `parser.rs`
and `value.rs`) cover the default pattern output, counter widths, Unicode, `classify`,
user functions, target paths, reserved names and length limits. Run them with the commands in
[Development](https://github.com/RetroHazard/animatic/blob/main/docs/development.md).
