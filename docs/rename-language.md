# Rename language

Animatic names and moves hashed files with a small expression language that follows the
AniDB O'Matic (AOM) rename script. This guide documents the language **as implemented in
this repository**: syntax, values and coercion, every built-in function, every object
property, how a result becomes a path, and what blocks a rename.

Statements about behavior below were read from the sources listed in
[Source map](#source-map) and from the rename unit tests. Expected results in the examples
and in [Self-test patterns](#self-test-patterns) are derived from that reading; they were
not re-executed while writing this guide. Use **Validate on selected file** (see
[Verify a pattern](#verify-a-pattern)) to confirm any pattern on your own build and data.

Compatibility with the legacy client is the implementation's stated goal, and the code keeps
the legacy interpreter's quirks on purpose. Where this guide says "legacy", it only repeats
what the source comments say; no AOM binary was run for this guide. Windows is the platform
the implementation was exercised on. Linux runtime behavior has not been verified, so
Linux-specific statements are reading of the code.

## Contents

- [What renaming does and does not do](#what-renaming-does-and-does-not-do)
- [The three stages](#the-three-stages)
- [Language syntax](#language-syntax)
- [Values, truthiness and coercion](#values-truthiness-and-coercion)
- [Errors and warnings during evaluation](#errors-and-warnings-during-evaluation)
- [Variables, scope and user functions](#variables-scope-and-user-functions)
- [Built-in function reference](#built-in-function-reference)
- [Objects and properties](#objects-and-properties)
- [From result to target path](#from-result-to-target-path)
- [Plan, blocking rules and apply](#plan-blocking-rules-and-apply)
- [The stable default pattern](#the-stable-default-pattern)
- [Working patterns](#working-patterns)
- [Pitfalls](#pitfalls)
- [Diagnostics reference](#diagnostics-reference)
- [Self-test patterns](#self-test-patterns)
- [Verify a pattern](#verify-a-pattern)
- [Rust API](#rust-api)
- [Differences from AOM and limits](#differences-from-aom-and-limits)
- [Source map](#source-map)

## What renaming does and does not do

| Capability | Status |
|---|---|
| Rename one selected, AniDB-identified hashed file after a preview and explicit confirmation (**Hashed files → Rename…**) | Implemented |
| Edit the pattern, reset it to the stable default, restore the replaced text and validate it on the selected file (**Tools → Options → Renaming pattern**) | Implemented |
| Per-folder rename destination (`base_path`, shown as **Rename destination** in Options) | Implemented, see [Base path](#base-path-and-absolute-or-relative-results) |
| Rename several files in one action | Not implemented; the dialog handles the one selected file |
| Automatic renaming during or after a scan | **Not implemented.** The per-folder "Auto rename" checkbox is stored and shown but nothing reads it. Every rename is a manual, confirmed action. |
| Renaming files that AniDB has not identified, or whose anime/episode rows are missing | Blocked with a message |
| Cleaning up folders left empty by a move | Not implemented (`clear_empty_folders` is stored only) |

User-interface steps (which page, which button) are in the [User guide](user-guide.md); the
stored field and its neighbors are in the [Settings reference](settings-reference.md). This
guide covers the language and the rules behind the preview.

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

Implemented by `preprocess` in [parser.rs](../crates/animatic-core/src/rename/parser.rs).
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
  [Pitfalls](#pitfalls).
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
- Anything unknown evaluates to the Boolean `False`, see [Pitfalls](#pitfalls).

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

## Built-in function reference

Function names are case-insensitive. "Count" is the accepted number of arguments; any other
count produces a warning (blocking) and a neutral return value. "Returns" gives the value type
because truthiness and later coercion depend on it.

### Control flow and variables

| Function | Count | Returns | Behavior |
|---|---|---|---|
| `if(cond, then[, else])` | 2–3 | the chosen branch's value, with its type; empty `AStr` when false and no `else` | Evaluates only the chosen branch |
| `in(x, a, b, …)` | ≥ 2 | Bool | `True` when `x` equals any later argument, using the [comparison](#comparison) rules; stops at the first match |
| `set(name, value)` | 2 | empty `AStr` | See [Variables](#variables) |
| `for(var, from, to, body)` | 4 | the bodies added together, starting from an empty string | Inclusive range of integers (`from`/`to` converted to integers). The body is evaluated once per value; its results are combined with the raw `+`, so **the body must produce a string**: `for('i', 1, 3, 'x' + i)` is `x1x2x3`, but a numeric body such as `for('i', 1, 3, i)` is a hard conversion error. Initialize any accumulator first: an unset variable reads as `False`. |
| `function(name, body, …)` | ≥ 2 | empty `AStr` | See [User functions](#user-functions) |

### Text functions

| Function | Count | Returns | Behavior |
|---|---|---|---|
| `lc(x)` / `uc(x)` | 1 | `AStr` | Lower/upper case for **ASCII letters only**; other letters are unchanged (`uc('é')` is `é`) |
| `sc(x)` | 1 | `WStr` | Lower-cases everything, then upper-cases the first character (Unicode aware) |
| `tc(x)` | 1 | `WStr` | Lower-cases everything and upper-cases the first character of each **space-separated** word |
| `trim(x)` | 1 | `WStr` | Removes characters at or below U+0020 from both ends |
| `length(x)` | 1 | `Int` | Number of **UTF-16 code units** (characters outside the Basic Multilingual Plane count as 2). On failure: warning and `Byte` 0 |
| `limit(text, max[, suffix])` | 2–3 | `WStr` | Counts **characters** (code points). If longer than `max`, keeps `max − length(suffix)` characters (never below 0) and appends the suffix. **When the suffix itself is longer than `max`, no source characters are kept and the whole suffix is appended, so the result can be longer than `max`** (`limit('abcdef', 2, '....')` is `....`). With a suffix no longer than `max` the result is at most `max` characters. Unchanged when it fits. |
| `padl(text, width[, fill])` | 2–3 | `WStr` | Pads on the left to `width` characters; `fill` defaults to a space and only its **first character** is used; unchanged when already long enough (never truncates) |
| `padr(text, width[, fill])` | 2–3 | `WStr` | Pads on the right |
| `pad(text, width[, fill])` | 2–3 | `WStr` | Centers: `⌊missing/2⌋` fill characters on the left, the rest on the right |
| `replace(text, find[, with])` | 2–3 | `AStr` | Replaces every occurrence of `find` (case-sensitive) in a **single left-to-right pass**; `with` defaults to empty; an empty `find` leaves the text unchanged |
| `replacei(text, find[, with])` | 2–3 | `AStr` | Like `replace` but intended to ignore case. The search text is lower-cased with full Unicode expansion, while each input character is lower-cased using only the first character of its lower-case form. Characters whose lower case expands (for example `İ`) therefore may not match even an identical search text: `replacei('İ', 'İ', 'x')` returns `İ`. For ASCII and ordinary letters it behaves as expected; do not rely on it for such characters when sanitizing. |
| `replacerepeat`, `replacerepeati` | 2–3 | `AStr` | **Recognized, but behave exactly like `replace` / `replacei`.** They do not repeat until the text stops changing. |
| `substr(text, start[, count])` / `copy(…)` | 2–3 | `WStr` | `start` is 1-based in characters. A `start` below 1 or past the end gives an empty string; `count` defaults to the rest and is clamped; a negative count gives an empty string |
| `split(text, delimiter, index)` | 3 | `WStr` (empty `AStr` when missing) | Splits on the delimiter and returns the 1-based part. An empty delimiter means the whole text is one part. An empty or out-of-range part gives an empty string. |
| `join(separator, a, b, …)` | ≥ 1 | `WStr` (empty `AStr` when nothing remains) | Concatenates the **non-empty** arguments with the separator |
| `count(text, sub)` | 2 | `Byte` (falsy) | Number of occurrences of `sub` in `text`, **counting overlaps** (`count('aaa', 'aa')` is 2). An empty `sub` gives 0. Compare it: `count(…) > 0`. |

### Tests

| Function | Count | Returns | Behavior |
|---|---|---|---|
| `contains(needle, text)` | 2 | Bool | `True` when `text` contains `needle`, case-sensitive. **The needle is the first argument.** An empty needle is `False`. |
| `start(text, prefix)` | 2 | Bool | Case-sensitive prefix test |
| `starti(text, prefix)` | 2 | Bool | Case-insensitive prefix test |
| `numeric(x)` | 1 | Bool | `True` when the text is non-empty and consists only of ASCII digits, with an optional `-` as the first character (a lone `-` counts). A wrong argument count returns `True` with a warning. |

### Numbers

| Function | Count | Returns | Behavior |
|---|---|---|---|
| `max(a, b, …)` / `min(a, b, …)` | ≥ 2 | `Int` | Arguments are [converted to integers](#argument-conversion-for-functions) first, so `max(2.6, 1)` is 3 and strings that are not numbers count as 0 |
| `strtoint(x)` | 1 | `Int64` (falsy) | Delphi `StrToIntDef(x, 0)`: trims, accepts a sign and a `$` hex prefix, otherwise 0 |

### Titles and extra objects

These use the `DataSource` ([custom data sources](#custom-data-sources)). A language argument
that cannot be resolved yields an empty string.

| Function | Count | Returns | Behavior |
|---|---|---|---|
| `at(lang)` | 1 | `WStr` | Title of the bound anime `A` in that language. Needs `A`; otherwise a hard error. |
| `et(lang)` | 1 | `WStr` | Title of the bound episode `E` in that language. Needs `E`. |
| `atr(aid, lang)` | 2 | `WStr` | Anime title for an explicit anime id |
| `etr(eid, lang)` | 2 | `WStr` | Episode title for an explicit episode id |
| `getanime(name, aid)` | 2 | empty `AStr` | Binds the anime object `aid` under `name` (for example `B`), so `B.Name` works afterwards. When the anime is not in the library the name is bound to nothing. |
| `getepisode(name, eid)` | 2 | empty `AStr` | The same for an episode |

Rules for these functions:

- **Anime title selection** (`at`, `atr`): among stored titles of the requested language, the
  first **official** title (type 4) wins; otherwise the **main** title (type 1) of that
  language; synonyms (2) and short titles (3) are never returned. With the application
  database, `x-jat` carries the main (romaji) title, `en` and `ja` carry official titles when
  AniDB supplied them.
- **Episode title selection** (`et`, `etr`): the first stored title in that language.
- **Language argument** (`DbSource::language`): a positive number is looked up as a legacy
  language id; otherwise the text is matched against language names and codes in the category
  table; finally any code used by a stored anime or episode title is accepted as is. Typical
  arguments are codes such as `'x-jat'`, `'en'`, `'ja'`.
- **`getanime`/`getepisode` binding is first-wins**, even when the earlier binding was
  nothing. A name that is already bound (including the standard `A`, `E`, `F`, `G`, `H`, `M`)
  cannot be rebound, so calling `getanime('B', n)` repeatedly in a loop keeps the first
  result. Binding the name `self` changes the default object for bare names.

## Objects and properties

The application binds six objects for every evaluation. Property names are case-insensitive.
A property that does not exist on a bound object evaluates to `False` (no message). Reading a
property of an object that is **not bound** is a hard error.

| Object | Class | Bound when | Unbound means |
|---|---|---|---|
| `A` | anime | Always (the anime row must exist, else `Can't autorename file, Anime <aid> is missing for File <fid>!`) | — |
| `E` | episode | Always (else `Can't autorename file, Episode <eid> is missing for File <fid>!`) | — |
| `F` | AniDB file record | Always (else `No file entry for file <fid>!`) | — |
| `G` | release group | The file has a group id and a group row exists | `G` is false, `G.…` is a hard error |
| `H` | local hash row | The file's ed2k and size match a hashed local file | `H` is false, `H.…` is a hard error |
| `M` | MyList entry | The file is in the local MyList | `M` is false, `M.…` is a hard error |

Guard optional objects with `if(G, …)`, `if(H, …)`, `if(M, …)`, and use the lazy three-argument
form so the unbound branch never runs. The "Can't autorename" wording is inherited legacy text;
no automatic renaming exists.

The tables list each property with its value type. Strings marked `AStr` are truthy when
non-empty; `WStr`, `Int64` and `Byte` values are never truthy.

### `A` — anime

| Property | Type | Value |
|---|---|---|
| `Id` | Int | Anime id |
| `Name` | WStr | Display title: the main title when one exists, preferring `x-jat`, then `en`, then the lowest title type. When no title is stored it is the stand-in text `[Title not yet available]`, which is not real metadata. |
| `Category` | Int | Anime type id |
| `CategoryStr` | WStr | `TV Series`, `OVA`, `Movie`, `Other`, `Web`, `TV Special`, `Music Video` or `Unknown` |
| `Restricted` | Bool | Age-restricted flag |
| `Year` | AStr | Year text as stored (may be empty or a range) |
| `StartYear`, `EndYear` | Int | Four-digit numbers parsed from `Year` (split on `-` and `?`); `0` when absent; `EndYear` defaults to `StartYear` |
| `AirDate`, `EndDate` | Int64 | Stored Unix times |
| `TotalEpisodeCount` | Int | Total regular episodes from AniDB (0 when unknown) |
| `CurrentEpisodeCount` | Int | Highest known episode number |
| `EpisodeCount` | Int | Regular episodes stored locally |
| `SpecialEpisodeCount` | Int | Special-type episodes stored locally |
| `Length` | Int | Sum of stored regular episode lengths (minutes) |
| `Size` | Int64 | Sum of stored file sizes of this anime (bytes) |
| `VoteAverage`, `VoteCount` | Int | Stored vote rating (the schema stores ratings ×100) and count |
| `ReviewAverage`, `ReviewCount` | Int | Stored review rating and count |
| `MylistCurrentEpisodeCount` | Int | Distinct regular episodes in the MyList |
| `MylistSpecialEpisodeCount` | Int | Distinct non-regular episodes in the MyList |
| `MylistLength` | Int | Sum of episode lengths over MyList rows (minutes) |
| `MylistWatchedLength` | Int | The same for watched rows |
| `MylistSize` | Int64 | Sum of file sizes over MyList rows |
| `WatchedCount` | Int | Distinct watched regular episodes |
| `SpecialWatchedCount` | Int | Distinct watched non-regular episodes |
| `DownloadState` | AStr | Computed from **regular episodes only** (specials in the MyList are ignored): `adsHaveNone` (no regular episode in the MyList); `adsComplete` (total known and MyList regular episodes ≥ total); `adsHaveAll` (MyList regular episodes ≥ the larger of the effective total and 1); otherwise `adsIncomplete` |
| `WatchedState` | AStr | Computed from **regular episodes only**: `awsNone` (no watched regular episode); `awsAll` (watched regular episodes ≥ the larger of MyList regular episodes and the effective total); otherwise `awsPartial`. An anime with only watched specials reports `adsHaveNone` and `awsNone`. |

The effective total is `TotalEpisodeCount` when positive, otherwise `CurrentEpisodeCount`.

### `E` — episode

| Property | Type | Value |
|---|---|---|
| `Id` | Int | Episode id |
| `EpisodeNo` | Int | Episode number within its type |
| `TypeId` | Int | `1` regular, `2` special, `3` credit, `4` trailer, `5` parody, `6` other |
| `EpisodeStr` | WStr | Type prefix plus the unpadded number: `3`, `S1`, `C2`, `T1`, `P1`, `O1` |
| `EpisodeStrPadded` | WStr | Type prefix plus the number zero-padded to the digit width of the **highest stored episode number of that type for this anime** (width at least 1). It therefore changes as more episodes of that anime are fetched; see [Pitfalls](#pitfalls). |
| `EpisodeTypeCount` | Int | That highest stored episode number of the same type |
| `IsSpecial` | Bool | `True` for every type except regular |
| `IsRecap` | Bool | Recap flag |
| `Name` | WStr | English title when stored, else `x-jat`, else the first stored title; empty when none |
| `Length` | Int | Minutes |
| `AirDate` | Int64 | Stored air date |
| `Other` | WStr | Free text |

### `F` — file (AniDB record)

| Property | Type | Value |
|---|---|---|
| `Id` | Int | File id |
| `Version` | Int | `1` to `5`, from the AniDB state bits (v2 to v5) |
| `Crc` | AStr | AniDB's CRC32, **lower case**; empty when unknown |
| `CrcStatus` | AStr | `crcOk`, `crcInvalid` or `crcUnknown` |
| `Ed2k`, `Md5`, `Sha1` | AStr | Hashes from the AniDB record; empty when unknown |
| `FileType` | AStr | Extension as AniDB reports it (for example `mkv`); **empty when unknown**, which leaves a trailing dot that planning then trims, dropping the extension |
| `Size` | Int64 | Bytes |
| `Length` | Int | Seconds |
| `Quality`, `Source` | WStr | Category names (for example a quality or source label); empty when unknown |
| `Released` | Int | Stored release date |
| `ResolutionString` | WStr | Resolution text as stored (`WxH`) |
| `ResolutionWidth`, `ResolutionHeight` | Int | Parsed from the resolution text; `0` when absent |
| `State` | Int | Raw AniDB state bit field |
| `TypeState` | Int | Legacy file type code (`10` video, `20` audio, `30` subtitle, …) |
| `UserCount` | Int | AniDB user count |
| `EpisodeStart`, `StartPercent` | Int | Start percentage for files covering part of an episode |
| `EpisodeEnd`, `EndPercent` | Int | End percentage |
| `GroupShort`, `GroupLong` | WStr | Group names stored on the file record |
| `SubtitleId` | Int | Always `0` |
| `Other` | WStr | Free text |

### `G` — group

| Property | Type | Value |
|---|---|---|
| `Id` | Int | Group id |
| `Name` | WStr | Full name (may be empty) |
| `Shortname` | WStr | Short name (may be empty) |
| `Rating`, `Votes` | Int | Stored rating and votes |
| `Other` | WStr | Free text |

### `H` — local hash

`H` describes the hash row Animatic recorded when it hashed the file. Animatic's own hashing
stores only ed2k, size, CRC32, MD5 and SHA-1 (CRC32, MD5 and SHA-1 are stored in upper case).
**The media properties below are filled only for rows imported from an AOM databank**; for a
file Animatic hashed itself they read `0`, `0.0`, empty or `False`.

| Property | Type | Value |
|---|---|---|
| `Id` | Int | Row id |
| `Crc32`, `Md5`, `Sha1`, `Ed2k` | AStr | Hash texts; `Crc32` is what the stable default prefers over `F.Crc` |
| `Size` | Int64 | Bytes |
| `FPS`, `Length` | Double | Frames per second; length (a `Double`, so it cannot be passed to string functions) |
| `VideoFourCC`, `VideoCompression`, `VideoCodec` | AStr | Video identifiers |
| `VideoWidth`, `VideoHeight` | Int | Pixels |
| `VideoBitrate` | Double | Bitrate |
| `VideoVBR` | Bool | Variable bitrate flag |
| `AudioIdentCodec` | Int | Legacy audio identifier |
| `AudioCodec` | AStr | Audio codec |
| `AudioSamplerate` | Int | Sample rate |
| `AudioBitrate` | Double | Bitrate |
| `AudioVBR` | Bool | Variable bitrate flag |

### `M` — MyList entry

| Property | Type | Value |
|---|---|---|
| `Id`, `MylistId` | Int | MyList entry id |
| `FileId`, `EpisodeId` | Int | Linked file and episode ids |
| `State` | Int | `0` unknown, `1` on HDD, `2` on CD, `3` deleted, `4` shared |
| `Watched` | Bool | Watched flag |
| `WatchedDate`, `Date` | Int | Stored watched date and MyList add date |
| `Storage`, `Source`, `Other` | WStr | The MyList text fields |

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
How managers match is in the [Settings reference](settings-reference.md).

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
[Settings reference](settings-reference.md) for the exact persistence rules.

On confirmation the application calls `move_file(old, target)` with the planned path (see
[The three stages](#the-three-stages)). On success it logs `Renamed <old> to <new>` and
refreshes the library list; on failure it logs `Rename failed: <error>` and the library path
is unchanged.

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

For the synthetic sample used in [Working patterns](#working-patterns) the default produces:

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
[Component validation](#component-validation).

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
    [Planning rules](#planning-rules).
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
| Editor and preview | `The renaming pattern has a syntax error: …` | See [Compile errors](#compile-errors) | Fix the construct near the quoted position |
| Preview | `The stable default needs the anime title, which has not been fetched yet. …` | Title missing or the stand-in | Fetch the anime from AniDB |
| Preview | `The stable default needs the episode number, which is not known yet. …` | Episode number 0 or unknown | Fetch the episode |
| Preview | `The renaming pattern failed to run: <message>` | Hard error (division by zero, bad conversion, unbound object, …) | Fix the expression; guard optional objects |
| Preview | `The pattern ended with the truth value "True"/"False" instead of a file name. …` | The whole value is a Boolean | Make the last line the name |
| Preview | `The pattern produced an empty file name; nothing would be renamed. …` | Empty result | Make the last line produce the name |
| Preview | `Renaming "<old>" to "<name>" would result in an invalid file name (<reason>); rename skipped!` | [Component validation](#component-validation) | Sanitize in the pattern |
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
[Development](development.md).

## Rust API

The language lives in `animatic_core::rename`. Generate API documentation with
`cargo doc --workspace --no-deps` and open `target/doc/animatic_core/rename/index.html`;
the source-level entry points are:

| Item | Source |
|---|---|
| `Pattern::compile`, `Bindings`, `Evaluation`, `evaluate`, `target_path`, `EvalError` | [mod.rs](../crates/animatic-core/src/rename/mod.rs) |
| `preprocess`, `compile`, `Node`, `CompileError` | [parser.rs](../crates/animatic-core/src/rename/parser.rs) |
| `Evaluator`, `Object`, `DataSource` and the built-in functions | [eval.rs](../crates/animatic-core/src/rename/eval.rs) |
| `Value` and the arithmetic/comparison helpers | [value.rs](../crates/animatic-core/src/rename/value.rs) |
| `bindings_for_file`, `DbSource`, `anime_object`, `episode_object`, `file_object`, `group_object`, `hash_object`, `mylist_object`, `TITLE_PENDING` | [objects.rs](../crates/animatic-core/src/rename/objects.rs) |
| `PatternKind`, `classify`, `check_pattern`, `Plan`, `plan` | [plan.rs](../crates/animatic-core/src/rename/plan.rs) |
| `file_manager::move_file` | [file_manager.rs](../crates/animatic-core/src/file_manager.rs) |
| `DEFAULT_RENAME_PATTERN` | [settings.rs](../crates/animatic-core/src/settings.rs) and [default_pattern.txt](../crates/animatic-core/src/rename/default_pattern.txt) |

### Example: plan a rename against fixed objects

This uses only public items and needs no database. Put it in a scratch binary that depends on
`animatic-core` by path. It was written from the signatures in the sources.

```rust
use std::path::Path;

use animatic_core::rename::plan::{self, Plan};
use animatic_core::rename::{evaluate, Bindings, DataSource, Object, Pattern, Value};

/// A data source with no titles; enough for patterns that use only A, E and F.
struct NoTitles;

impl DataSource for NoTitles {
    fn anime(&self, _aid: i64) -> Option<Object> {
        None
    }
    fn episode(&self, _eid: i64) -> Option<Object> {
        None
    }
    fn language(&self, lang: &str) -> Option<String> {
        Some(lang.to_string())
    }
    fn anime_titles(&self, _aid: i64) -> Vec<(i64, String, String)> {
        Vec::new()
    }
    fn episode_titles(&self, _eid: i64) -> Vec<(String, String)> {
        Vec::new()
    }
}

fn sample() -> Bindings {
    Bindings {
        anime: Some(
            Object::new("TAniDBAnime")
                .with("Id", Value::Int(1))
                .with("Name", Value::WStr("Example Series".into())),
        ),
        episode: Some(
            Object::new("TAniDBEpisode")
                .with("Id", Value::Int(2))
                .with("EpisodeNo", Value::Int(3)),
        ),
        file: Some(Object::new("TAniDBFile").with("FileType", Value::AStr("mkv".into()))),
        ..Bindings::default()
    }
}

fn evaluate_only(bindings: &Bindings) -> Result<String, String> {
    let pattern = Pattern::compile("A.Name + '.mkv'").map_err(|e| e.message)?;
    let evaluation = evaluate(&pattern, bindings, &NoTitles).map_err(|e| e.message)?;
    Ok(evaluation.result)
}

fn main() {
    let bindings = sample();
    println!("{:?}", evaluate_only(&bindings)); // Ok("Example Series.mkv")

    let planned: Plan = plan::plan(
        "A.Name + ' - ' + padl(E.EpisodeNo, 2, '0') + '.' + F.FileType",
        &bindings,
        &NoTitles,
        Path::new("library/show/old.mkv"),
    );
    assert!(planned.can_apply(), "{planned:?}");
    // The file name is "Example Series - 03.mkv", placed beside the old file.
    println!("{}", planned.target.unwrap().display());
}
```

### Example: plan against the library database

`bindings_for_file` builds all six objects for a file id, and `DbSource` answers the title
functions from the same database. The snippet only plans; it never moves a file.

```rust
use std::path::Path;

use animatic_core::db::Db;
use animatic_core::rename::objects::{bindings_for_file, DbSource};
use animatic_core::rename::plan::plan;

fn preview(
    db: &Db,
    fid: i64,
    current_path: &str,
    base_path: &str,
    pattern: &str,
) -> Result<(), String> {
    let bindings = bindings_for_file(db, fid, current_path, base_path)?;
    let planned = plan(pattern, &bindings, &DbSource { db }, Path::new(current_path));
    for error in &planned.errors {
        println!("error: {error}");
    }
    for warning in &planned.warnings {
        println!("warning: {warning}");
    }
    match (&planned.target, planned.can_apply()) {
        (Some(target), true) => println!("would rename to {}", target.display()),
        (_, _) if planned.already_named() => println!("already named"),
        _ => println!("blocked"),
    }
    Ok(())
}
```

To apply a plan that `can_apply()`, call
`animatic_core::file_manager::move_file(&db, local_id, old_path, &target)`; the `local_id` is
the local file row id, which is not the AniDB file id.

### Custom data sources

`DataSource` is the only way the language reaches data other than the six bound objects. Its
five methods back `getanime`, `getepisode`, `at`, `et`, `atr`, `etr`:

| Method | Used by | Contract |
|---|---|---|
| `anime(aid) -> Option<Object>` | `getanime` | Anime object, or `None` if unknown |
| `episode(eid) -> Option<Object>` | `getepisode` | Episode object, or `None` |
| `language(lang) -> Option<String>` | `at`, `et`, `atr`, `etr` | Resolve a numeric id, name or code to the code stored on titles; `None` yields an empty title |
| `anime_titles(aid) -> Vec<(i64, String, String)>` | `at`, `atr` | `(type, language code, name)` in storage order; type 1 main, 4 official |
| `episode_titles(eid) -> Vec<(String, String)>` | `et`, `etr` | `(language code, name)` in storage order |

`DbSource` implements them over the library database. Objects are plain property lists:
`Object::new(class).with(name, Value)`, looked up case-insensitively.

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
| Preprocessing, lexer, precedence, compile errors | [parser.rs](../crates/animatic-core/src/rename/parser.rs) |
| Values, truthiness, coercion, arithmetic, comparison | [value.rs](../crates/animatic-core/src/rename/value.rs) |
| Evaluator, functions, variables, user functions, title selection | [eval.rs](../crates/animatic-core/src/rename/eval.rs) |
| Bound objects, property values, language resolution | [objects.rs](../crates/animatic-core/src/rename/objects.rs) |
| Result handling, target path, name validation | [mod.rs](../crates/animatic-core/src/rename/mod.rs) |
| Plan, classification, stable-default metadata rule | [plan.rs](../crates/animatic-core/src/rename/plan.rs) |
| Default pattern | [default_pattern.txt](../crates/animatic-core/src/rename/default_pattern.txt) |
| Behavior tests | [tests.rs](../crates/animatic-core/src/rename/tests.rs) |
| Apply | [file_manager.rs](../crates/animatic-core/src/file_manager.rs) (`move_file`) |
| Preview and editor UI | [app.rs](../crates/animatic/src/app.rs) (`preview_rename`, `rename_window`, `pattern_editor_ui`) |

Related guides: [User guide](user-guide.md), [Settings reference](settings-reference.md),
[Architecture](architecture.md), [Feature parity](feature-parity.md) and
[Troubleshooting](troubleshooting.md).
