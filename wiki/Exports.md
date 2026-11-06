The two file exports: a library statistics report and per-file hash lines. Their stored options
and tokens are in the [Settings reference](Settings-Reference.md#hash-export-template-tokens).

## Statistics export

**File → Export statistics…** asks for a file and writes a library report in the format chosen in
Options → Statistics export (XML or INI). It runs on the background projection thread and logs
`Exported statistics to <path>` or `Statistics export failed: …`.

**Automatic export:** if *Write statistics after library updates* is on and *Output file* is not
empty, the file is rewritten every time a library snapshot is loaded (after hashing, syncing,
edits, refreshes, startup), overwriting it. Failures log `Automatic statistics export failed: …`.
Parent folders are created. Use a path outside the repository.

Content (from `stats.rs`): AniDB totals in your database (anime, episodes, files, groups, total
size, total length), MyList totals (anime, episodes, files, size, anime watched/partially watched,
episodes watched, files watched, watched size, length, watched length), seven anime-type groups
(`tvseries, tvspecials, movies, ovas, web, other, unknown` with total, in mylist, all watched,
part watched, size, length, watched length) and, only when MyList is not empty, four MyList-state
groups (`unknown, onhdd, oncd, deleted`; state *Remote/Shared* is not reported). Lengths are
seconds; sizes are bytes. XML uses `<stats><anidb>/<mylist>` with CRLF line breaks; INI uses
`[anidb]`, `[mylist]`, `[animetypeTVSeries]`…`[animetypeUnknown]` and `[statusUnknown]`…
`[statusDeleted]`. Both contain your **AniDB username** (XML-escaped; INI verbatim); treat the
file as personal data.

## Hash export

**File → Export hashes…** writes one line per hashed file, for *all* hashed files (search
filters and selection are ignored), using the *Hash export template* from Options
(empty template = `%fulled2k%`). It runs on the interface thread and logs `Exported N hashes to
<path>` or `Hash export failed: …`. Lines end with CRLF; a newline inside the template becomes CRLF.

Tokens are replaced sequentially in this order; unknown text is left as is:

| Token | Value |
|---|---|
| `%filename%` | File name |
| `%filepath%` | Full stored path |
| `%ed2k%`, `%md5%`, `%sha1%`, `%crc32%` | Hash (ED2K lower-case; MD5/SHA-1/CRC32 upper-case; CRC32 zero-padded to 8) |
| `%size%` | **Human-readable** size such as `1.00 MB`, not a byte count |
| `%fulled2k%` | `ed2k://\|file\|<name>\|<bytes>\|<ed2k>\|/` |
| `%fps%`, `%height%`, `%width%`, `%vbitrate%`, `%asample%`, `%abitrate%` | Constant placeholders `0.00`/`0` |
| `%vcodec%`, `%acodec%` | Always empty |
| `%length%` | Empty, or `0` when *Export %length% in minutes* is on |

Media tokens are not read from the files, so only the first group is meaningful. See also the
[settings reference](Settings-Reference.md#hash-export-template-tokens).
