# Rename language Rust API

How to plan a rename from Rust. The language itself is documented in the
[Rename language](https://github.com/RetroHazard/animatic/wiki/Rename-Language) wiki page.

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

## Example: plan a rename against fixed objects

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

## Example: plan against the library database

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

## Custom data sources

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
