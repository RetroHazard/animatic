Animatic names and moves hashed files with a small expression language that follows the
AniDB O'Matic (AOM) rename script. These pages document the language **as implemented in
this repository**: syntax, values and coercion, every built-in function, every object
property, how a result becomes a path, and what blocks a rename: this page (syntax, values, variables),
[Functions and objects](Rename-Functions-and-Objects.md),
[Targets and apply](Rename-Targets-and-Apply.md) and [Patterns](Rename-Patterns.md).

Statements about behavior in these pages were read from the sources listed in
[Source map](#source-map) and from the rename unit tests. Expected results in the examples
and in [Self-test patterns](Rename-Patterns.md#self-test-patterns) are derived from that reading; they were
not re-executed while writing them. Use **Validate on selected file** (see
[Verify a pattern](Rename-Patterns.md#verify-a-pattern)) to confirm any pattern on your own build and data.

Compatibility with the legacy client is the implementation's stated goal, and the code keeps
the legacy interpreter's quirks on purpose. Where these pages say "legacy", it only repeats
what the source comments say; no AOM binary was run for them. Windows is the platform
the implementation was exercised on. Linux runtime behavior has not been verified, so
Linux-specific statements are reading of the code.

## What renaming does and does not do

| Capability | Status |
|---|---|
| Rename one selected, AniDB-identified hashed file after a preview and explicit confirmation (**Hashed files → Rename…**) | Implemented |
| Edit the pattern, reset it to the stable default, restore the replaced text and validate it on the selected file (**Tools → Options → Renaming pattern**) | Implemented |
| Per-folder rename destination (`base_path`, shown as **Rename destination** in Options) | Implemented, see [Base path](Rename-Targets-and-Apply.md#base-path-and-absolute-or-relative-results) |
| Rename several files in one action | Not implemented; the dialog handles the one selected file |
| Automatic renaming during or after a scan | **Not implemented.** The per-folder "Auto rename" checkbox is stored and shown but nothing reads it. Every rename is a manual, confirmed action. |
| Renaming files that AniDB has not identified, or whose anime/episode rows are missing | Blocked with a message |
| Cleaning up folders left empty by a move | Not implemented (`clear_empty_folders` is stored only) |

User-interface steps (which page, which button) are in [Renaming files](Renaming-Files.md); the
stored field and its neighbors are in the [Settings reference](Settings-Reference.md). These
pages cover the language and the rules behind the preview.

## The three stages

Renaming is deliberately split so that nothing touches the file system until the last step.

| Stage | Entry points | Reads | Writes |
|---|---|---|---|
| **1. Evaluate** | `Pattern::compile`, `rename::evaluate` | Pattern text, the bound objects, and (through `DataSource`) title tables for `at`, `et`, `atr`, `etr`, `getanime`, `getepisode` | Nothing. Produces a text result, variables and warnings. |
| **2. Plan** | `rename::plan::plan` (preceded by `check_pattern` and `classify`) | Everything stage 1 reads; the current path and the manager base path as strings | Nothing. No existence checks, no directory creation. Produces a `Plan`: result text, optional target path, warnings, errors. |
| **3. Apply** | `file_manager::move_file`, called by the **Rename file** button | The file system (destination exists?) | Creates missing parent folders, renames the file, then updates the library path |

- **Evaluate** is pure with respect to the file system. The database-backed data source only
  reads.
- **Plan** adds validation: a syntax-checked pattern, the metadata rule for the stable
  default, the empty/boolean result rules, warning blocking and path validation. A plan that
  `can_apply()` is *allowed* to be applied; it has not been applied and the destination may
  still exist.
- **Apply** is the only stage that changes anything. `move_file` returns `Ok` immediately
  when old and new are equal, refuses when the destination already exists
  (`destination already exists: <path>`), runs `create_dir_all` on the destination's parent,
  calls `std::fs::rename`, and **only after that succeeds** updates the database path. A
  failed rename leaves the database path unchanged.

Details worth knowing about apply (read from `move_file`; not exercised):

- Folders created by `create_dir_all` stay even if the following `fs::rename` fails.
- **The existence check and the rename are two separate steps, not one atomic operation.** If
  another process creates a file at the destination between them, `std::fs::rename` can
  replace it: on Linux/POSIX `rename(2)` overwrites an existing file, so the check protects
  against ordinary collisions only, not against this race. Do not run other tools that write
  to the destination folder while confirming a rename.
- It is a rename, not copy-and-delete, so a destination on another volume can fail with the
  operating system's error, shown as `Rename failed: …`.
- The existence check compares the destination path, so a change that differs only in letter
  case on a case-insensitive file system may report the destination as existing.

## Language syntax

A pattern is text. It is turned into **one expression** and evaluated left to right. There
are no statements, loops over lines or jumps; "lines" are a way to lay out one big
concatenation.

### Preprocessing

Implemented by `preprocess` in [parser.rs](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/src/rename/parser.rs).
Steps, in order:

1. Split the text into lines on `\r\n`, `\r` or `\n`.
2. Trim each line of every character at or below U+0020 (spaces, tabs and control characters)
   at both ends.
3. Drop lines that are empty after trimming, and lines that start with `//`.
4. Continuation: working from the last line backwards, a line whose last character is `_` has
   that `_` removed and the **next** line appended directly (no space is inserted). Chains of
   continuation lines work. A `_` at the end of the very last line is left in place.
5. Wrap every remaining line in parentheses and join the lines with ` + `.

So these three lines

```
// build the name
set('a', 'x')
'a' + _
'b'
```

become `(set('a', 'x')) + ('a' + 'b')`.

Consequences:

- **Comments are whole lines only**, starting with `//` after trimming. `//` in the middle of
  a line is not a comment; it is two division operators and a syntax error.
- Comment lines are removed *before* continuation, so a comment between two continued lines
  disappears and the first line joins the next real line.
- Because lines are concatenated with `+`, **every line contributes its value to the result**.
  `set(…)` returns an empty string, and the two-argument `if(cond, action)` returns an empty
  string when the condition is false, which is why those idioms are invisible. A line that
  evaluates to text, a number or a truth value is added to the name. See
  [Pitfalls](Rename-Patterns.md#pitfalls).
- A pattern that is only comments and blank lines compiles to nothing and evaluates to the
  truth value `False`; the plan then reports the boolean-result error.
- The preprocessed text is stored on the compiled pattern and exposed to the pattern as the
  variable `ACTIVEPATTERN`.
- Error positions refer to the preprocessed one-line text, not to your original line numbers.
  An unbalanced quote swallows following lines, so the reported position can be much later than
  the mistake.

### Tokens

Whitespace between tokens is **space, line feed and form feed only**. A **tab inside a line**
(not at its ends, where trimming removes it) is an unknown character and ends the token
stream, which produces a syntax error. Tabs are fine inside string literals.

Any character the scanner does not recognise ends the token stream without being consumed, so
the parser reports a syntax error near it. This covers `&`, `%`, `!`, `;`, `[`, `{`, `#`, `@`,
`==`, `!=`, `&&`, `||` and every non-ASCII character that is not inside a string literal.

| Token | Spelling |
|---|---|
| Number | A run of ASCII digits and dots, for example `2`, `0.5`, `5.`. Text that does not parse as a number (for example `1.2.3`) becomes `0`. No sign, exponent, hex or leading `.5`. Always a `Double`. |
| String | `'…'` or `"…"`. See [String literals](#string-literals). |
| Boolean | `True`, `False` (case-insensitive) |
| Identifier | An ASCII letter followed by ASCII letters, digits or `_`. Case-insensitive everywhere (variables, objects, properties, functions). |
| Word operators | `and`, `or`, `xor`, `not`, `div`, `mod` (case-insensitive, reserved; they cannot be variable names) |
| Symbol operators | `+ - * / = <> < <= > >=` |
| Punctuation | `( ) , .` |

There is no `==`, `!=`, `&&`, `||`, `%` or `!`. Equality is a single `=`; inequality is `<>`.

### String literals

- Delimited by matching single or double quotes.
- **There are no escape sequences.** A backslash is an ordinary character: `'\'` is a one
  character string. This is why Windows paths and the default pattern's `replace(x, '\', ' ')`
  work as written.
- A literal cannot contain its own delimiter. To produce both quote kinds, concatenate:
  `"It's " + '"quoted"'`. The default pattern uses `"'"` for a single quote.
- A string that is never closed runs to the end of the text and is accepted by the lexer;
  the missing delimiter then usually breaks the parenthesis structure and surfaces as a
  syntax error.
- Literal strings are `WStr` values, which matters for truthiness. See
  [Values](#values-truthiness-and-coercion).

### Operators and precedence

From lowest to highest binding power:

| Level | Operators | Notes |
|---|---|---|
| 1 | `or` | Short-circuits; result is always a Boolean |
| 2 | `and` | Short-circuits; Boolean |
| 3 | `xor` | **Binds tighter than `and`**; evaluates both sides; Boolean |
| 4 | `=` `<>` `<` `<=` `>` `>=` | Boolean |
| 5 | `+` `-` | `+` concatenates or adds, see [coercion](#values-truthiness-and-coercion) |
| 6 | `*` `/` `div` `mod` | `/` gives a Double; `div` and `mod` give integers |
| 7 (unary) | `-x`, `+x`, `not x` | Apply to the *next primary only*, see below |

Two properties surprise most people:

1. **Every binary level is right-associative.** `10 - 3 - 2` is `10 - (3 - 2)` = `9`, and
   `a + b + c` is `a + (b + c)`. For string building this is harmless as long as a string sits
   somewhere to the right, but **two numbers at the end of a `+` chain are added first**:
   `'x' + 1 + 2` is `x3`, while `'x' + 1 + 2 + '.mkv'` is `x12.mkv`.
2. **Unary operators bind to the next primary only.** `not 1 = 2` is `(not 1) = 2`, not
   `not (1 = 2)`. Write `not (1 = 2)`. `-` is parsed as `False - x`, so `-5` is the number
   `-5`.

Parentheses group as usual. A parenthesized or called expression may not be followed by `.`:
`getanime('B', 1).Name` is a syntax error.

### Calls and property access

- `name(arg, arg, …)` calls a built-in or user function. `name()` with no arguments parses.
- `A.Name` reads a property of a bound object. Only a bare object name can be the parent:
  `A.B.C` fails at evaluation with a "parent object not found" error.
- A call written with a parent (`x.fn(…)`) parses and the parent is ignored.
- `name` without parentheses is a **variable, object or property reference**, never a call.

### Compile errors

Compile errors carry the legacy messages, including the original spelling "parentesis", and
the context decoration `In "<31 characters around the position>..." near "<next 16>"`.

| Message | When |
|---|---|
| `TRuleEvaluator.GetExpression7: tkParLeft: Missing right parentesis!` | `(` without matching `)`. Because every line is wrapped in parentheses, this is also the message for leftover tokens inside a line (`5abc`, `a b`) and for an unknown character, which ends the token stream |
| `TRuleEvaluator.GetExpression7: tkSymbol: Missing right parentesis!` | A call whose argument list does not end with `)` |
| `TRuleEvaluator.GetExpression7: tkSymbol: Property or method name missing!` | `.` not followed by a name |
| `TRuleEvaluator.GetExpression7: Right part of expression missing!` | An operand is missing (`'a' //`, `+` at the end of a line) |
| `TRuleManager.Compile: Syntax error!` | Tokens left over after the whole wrapped expression, for example an extra `)` as in `(1))` |

In the application the text is shown as `The renaming pattern has a syntax error: <message>`
with line breaks normalized. A blank pattern is reported separately as
`The renaming pattern is empty. Use the stable default, or write a pattern whose last
expression is the new file name.`

## Values, truthiness and coercion

The legacy interpreter used Delphi Variants, and its behavior depends on the exact variant
type, so Animatic keeps the types distinct. In Animatic all strings are Unicode (UTF-8 in
memory); the `AStr`/`WStr` distinction survives only for its effect on truthiness and
coercion.

| Type | Examples | Truthy when |
|---|---|---|
| `Int` | `length(x)`, `E.EpisodeNo`, `F.Version`, `div`/`mod` results, object references | not `0` |
| `Double` | every number literal, results of `/`, `+ - *` that involve a Double | not `0` |
| `Bool` | comparisons, `not`, `and`, `or`, `xor`, `contains`, `start`, `numeric`, `in`, `E.IsSpecial` | `True` |
| `AStr` | `lc`, `uc`, `replace*`, a false two-argument `if`, string properties such as `F.Crc`, `A.Year` | not empty |
| `WStr` | string literals, `limit`, `pad*`, `substr`, `split` part, `trim`, `sc`, `tc`, `join`, `at`, `et`, user function parameters, text properties such as `A.Name` | **never** |
| `Int64` | `strtoint`, `A.Size`, `A.AirDate`, `F.Size`, `E.AirDate` | **never** |
| `Byte` | `count` | **never** |
| `Empty` | internal identity of `+` for user functions | never |

Practical rules that follow:

- Do not test a literal or a `limit`/`substr`/`trim` result with `if(x, …)`; it is always
  false. Test `length(x) > 0` instead. The same goes for `count(…)` and the Int64 properties:
  write `count(a, b) > 0`.
- An object name used as a value (`G`, `H`, `M`) evaluates to the non-zero integer `1` when
  the object is bound, so `if(G, …)` means "a group is known".
- Anything unknown evaluates to the Boolean `False`, see [Pitfalls](Rename-Patterns.md#pitfalls).

### Text conversion (`to_text`)

Used for the final result and whenever `+` concatenates with a string:

| Value | Text |
|---|---|
| Integers | Decimal digits |
| `Double` | Delphi-style general format with up to 15 significant digits; whole numbers print without a fractional part (`2`), others trim trailing zeros (`0.333333333333333`); very large or small values use `E` notation (`1E20`) |
| `Bool` | `True` or `False` |
| `Empty` | empty string |

### `+`

1. If either operand is a string, the result is the concatenation of both operands' text.
   The result is an `AStr` if either operand was one, otherwise a `WStr`. Numbers and truth
   values are converted with the table above.
2. Otherwise the operands are added numerically (`Empty` is the identity). Integers stay
   integers (`Int64` if either is), anything else becomes a `Double`.

### `-`, `*`, `/`, `div`, `mod`

- `-` and `*`: integers (including `Bool`, treated as `-1` for `True` and `0` for `False`, and
  `Empty` as `0`) stay integers; anything else becomes a `Double`. Strings are first parsed as
  numbers; text that is not a number is a **hard error**
  (`Could not convert variant of type (String) into type (Double): '…'`).
- `/` always gives a `Double`; division by zero is a hard error `Division by zero`.
- `div` and `mod` round both operands to the nearest integer (ties to even), then divide
  integers; the result is an `Int`. A zero divisor is the same hard error.

### Comparison

`=`, `<>`, `<`, `<=`, `>`, `>=` and `in(…)` use the same comparison:

| Operands | How they are compared |
|---|---|
| two integers (`Int`, `Byte`, `Int64`) | numerically |
| two strings | by their text, **case-sensitively**, in code-point order |
| two Booleans | `False` < `True` |
| Boolean and string | the string is parsed as `true`/`false` (any case) or a number; otherwise a **hard error** |
| string and number | the string is parsed as a number; text that is not numeric is a **hard error** |
| anything else | both sides converted to `Double`, `Bool` counting as `-1`/`0` |

Compare strings with strings and numbers with numbers. `A.Year = 2010` raises a hard error
when the year text is empty or like `2010-2012`; compare the Int `A.StartYear` instead. A
mistyped property name evaluates to `False`, so `F.Nme = 'x'` is a hard error rather than a
silent mismatch.

### Argument conversion for functions

Functions convert their arguments in two ways. A failed conversion is a **warning**, not an
error (see the next section), and the value `""` or `0` is used.

| Conversion | Rules |
|---|---|
| To string | `Bool` → `True`/`False`; integers → digits; strings unchanged. **`Double` and `Empty` are not convertible** (warning `PARSERUN: Syntax error, value can not be converted to a string.`) |
| To integer | `Bool` → `1`/`0` (note: not `-1`); integers unchanged; `Double` rounds to nearest, ties to even; strings parse like Delphi `StrToIntDef` (trimmed, optional sign, `$` prefix for hex, otherwise `0`); `Empty` is a warning |

The most common consequence: **a number literal or the result of arithmetic is a `Double`, so
it cannot be passed where a function expects text.** `padl(5, 2, '0')` and
`padl(E.EpisodeNo + 1, 2, '0')` both produce a warning and block the rename, while
`padl(E.EpisodeNo, 2, '0')` is fine because `E.EpisodeNo` is an `Int`. Convert a Double to text
by concatenating with a string, `'' + (E.EpisodeNo + 1)`, or avoid the Double with
`(E.EpisodeNo + 1) div 1`. Number literals in *integer* positions (`limit(x, 90)`,
`padl(x, 2, '0')`) are fine.

## Errors and warnings during evaluation

| Kind | Effect | Examples |
|---|---|---|
| **Hard error** | Evaluation stops; the plan has one error `The renaming pattern failed to run: <message>` and no result | `Division by zero`; a non-numeric string in arithmetic or in a comparison with a number; a Boolean/string comparison that cannot be parsed; reading a property of an object that is not bound; a bare name with no default object; `at`/`et` without the `A`/`E` object |
| **Warning** (soft error) | Evaluation continues with a neutral value; the message is kept. **While any warning exists the plan cannot be applied**, even though a target path may still be shown | Wrong number of function arguments; `Double`/`Empty` passed as text; `Empty` passed as an integer; `set` with an invalid variable name |

Warnings use the legacy format
`PARSERUN: Function <name>: <text>, found <n> parameters.` (for example
`PARSERUN: Function lc: Lc must have 1 parameter, found 2 parameters.`). The function name and
text come from the branch that detected the problem, so `min` reports as `max`, and a
`getepisode` count error mentions `getaanime`.

Two evaluation failures that are **not** errors are the main source of bad names:

- A name that is not a variable, bound object, or property of the default object evaluates to
  `False` without any message.
- A call to an undefined function evaluates to `False` without any message.

## Variables, scope and user functions

### Variables

- `set('name', value)` creates or overwrites a variable and returns an empty string. The
  value keeps its type (a stored literal stays a falsy `WStr`).
- The first argument may be any string-valued expression. It must start with an ASCII letter,
  otherwise the call produces a warning and sets nothing. Only the first character is checked,
  so a name such as `a b` can be stored but never referenced.
- Names are case-insensitive. There are no scopes and nothing persists between files or
  between evaluations: every `evaluate` call starts with fresh variables and functions.
- A bare name is resolved in this order: variable; bound object (evaluates to `1`); property of
  the **default object**; otherwise `False`. The default object is the object named `self` if
  one was bound, otherwise the first bound object, which is `A`. Consequently a bare
  `Name`, `Year` or `Size` that you never set reads `A.Name`, `A.Year` or `A.Size`.
- `A.Name` always means the object property; a variable called `A` does not hide the object for
  property access.

Predefined variables (all `WStr`):

| Variable | Value |
|---|---|
| `PATH` | The file's current full path as recorded in the library |
| `BASEPATH` | The matching file manager's rename destination; empty when none |
| `ACTIVEPATTERN` | The preprocessed pattern text |

They are ordinary variables and can be overwritten with `set`. `ACTIVEPATTERN` is left out of
the variable list returned by `evaluate`.

### Control variables

`for('i', 1, 3, body)` sets `i` to each integer in turn, as an `Int`, and the variable remains
set after the loop. `function(…)` parameters are stored as `param1`, `param2`, … and also remain
set after the call.

### User functions

```
function('label', '[' + param1 + ']')
set('x', label('ok'))
x
```

- `function('name', body1, body2, …)` needs a name and at least one body; the message
  `Function can only have 2 parameters` is misleading, more bodies are accepted.
- The bodies are **stored unevaluated** and evaluated at each call. The function line itself
  returns an empty string.
- A name that is already defined is ignored: the **first definition wins**.
- Built-in names are matched first, so a user function can never replace a built-in.
- A call evaluates its arguments, converts each to **text** (so a `Double` argument warns), and
  stores them in `param1`…`paramN` as `WStr`. Because they are `WStr`, **`if(param1, …)` is
  always false**; use `length(param1) > 0`.
- The result is the bodies added together with the raw `+` of the `for` loop: strings
  concatenate, and numbers are added.
- A function must be defined on an **earlier line** than its first call. Calling an undefined
  function yields `False`, silently.
- There is no recursion-depth or iteration limit in the evaluator. Do not write a function
  that calls itself or a `for` with an enormous range.

## Differences from AOM and limits

Deliberate differences stated in the source and project rules:

- Strings are Unicode throughout, with no ANSI round trips.
- A relative result resolves against the **file's own folder**.
- The stable default uses a fixed two-digit counter; counts stored in the database do not
  influence it.
- Empty results, Boolean-only results, hard errors and any evaluation warning block the rename
  instead of producing or guessing a name.
- File-name validity uses the strictest rules on every platform (Windows reserved characters
  and device names including `¹ ² ³`), with a length unit that depends on the build platform.
- The stable default needs fetched title and episode data; a pending-title stand-in is never
  used to name a file with it.
- Renaming is manual and confirmed; automatic renaming is not implemented and must never be
  enabled implicitly.

Limits:

- One file per confirmation; no batch or dry run over many files.
- No variable list, step trace or debugger in the application; `Evaluation::variables` is
  available through the Rust API only.
- No collision handling beyond the non-atomic "destination already exists" check (which can lose
  a race and overwrite a file created in between), and no folder clean-up.
- The evaluator has no execution budget (loops, recursion).
- Production AniDB behavior and Linux runtime were not exercised while documenting.

## Source map

| Topic | File |
|---|---|
| Preprocessing, lexer, precedence, compile errors | [parser.rs](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/src/rename/parser.rs) |
| Values, truthiness, coercion, arithmetic, comparison | [value.rs](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/src/rename/value.rs) |
| Evaluator, functions, variables, user functions, title selection | [eval.rs](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/src/rename/eval.rs) |
| Bound objects, property values, language resolution | [objects.rs](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/src/rename/objects.rs) |
| Result handling, target path, name validation | [mod.rs](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/src/rename/mod.rs) |
| Plan, classification, stable-default metadata rule | [plan.rs](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/src/rename/plan.rs) |
| Default pattern | [default_pattern.txt](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/src/rename/default_pattern.txt) |
| Behavior tests | [tests.rs](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/src/rename/tests.rs) |
| Apply | [file_manager.rs](https://github.com/RetroHazard/animatic/blob/main/crates/animatic-core/src/file_manager.rs) (`move_file`) |
| Preview and editor UI | [app.rs](https://github.com/RetroHazard/animatic/blob/main/crates/animatic/src/app.rs) (`preview_rename`, `rename_window`, `pattern_editor_ui`) |
