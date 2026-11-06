Every built-in rename function and every property of the objects a pattern can read. Syntax,
values and coercion are in [Rename language](Rename-Language.md).

## Built-in function reference

Function names are case-insensitive. "Count" is the accepted number of arguments; any other
count produces a warning (blocking) and a neutral return value. "Returns" gives the value type
because truthiness and later coercion depend on it.

### Control flow and variables

| Function | Count | Returns | Behavior |
|---|---|---|---|
| `if(cond, then[, else])` | 2–3 | the chosen branch's value, with its type; empty `AStr` when false and no `else` | Evaluates only the chosen branch |
| `in(x, a, b, …)` | ≥ 2 | Bool | `True` when `x` equals any later argument, using the [comparison](Rename-Language.md#comparison) rules; stops at the first match |
| `set(name, value)` | 2 | empty `AStr` | See [Variables](Rename-Language.md#variables) |
| `for(var, from, to, body)` | 4 | the bodies added together, starting from an empty string | Inclusive range of integers (`from`/`to` converted to integers). The body is evaluated once per value; its results are combined with the raw `+`, so **the body must produce a string**: `for('i', 1, 3, 'x' + i)` is `x1x2x3`, but a numeric body such as `for('i', 1, 3, i)` is a hard conversion error. Initialize any accumulator first: an unset variable reads as `False`. |
| `function(name, body, …)` | ≥ 2 | empty `AStr` | See [User functions](Rename-Language.md#user-functions) |

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
| `max(a, b, …)` / `min(a, b, …)` | ≥ 2 | `Int` | Arguments are [converted to integers](Rename-Language.md#argument-conversion-for-functions) first, so `max(2.6, 1)` is 3 and strings that are not numbers count as 0 |
| `strtoint(x)` | 1 | `Int64` (falsy) | Delphi `StrToIntDef(x, 0)`: trims, accepts a sign and a `$` hex prefix, otherwise 0 |

### Titles and extra objects

These use the `DataSource` ([custom data sources](https://github.com/RetroHazard/animatic/blob/main/docs/rename-api.md#custom-data-sources)). A language argument
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
no automatic renaming happens unless a managed folder is explicitly set to **On**.

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
| `EpisodeStrPadded` | WStr | Type prefix plus the number zero-padded to the digit width of the **highest stored episode number of that type for this anime** (width at least 1). It therefore changes as more episodes of that anime are fetched; see [Pitfalls](Rename-Patterns.md#pitfalls). |
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
