//! Validated rename planning: turns a pattern and one file's metadata into either a target path
//! or explicit reasons why renaming must not happen. Nothing here changes the file system;
//! [`plan_batch`] only looks whether a target path is already taken.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::objects::{DbSource, TITLE_PENDING, bindings_for_file};
use super::{Bindings, DataSource, Pattern, Value, evaluate, target_path};
use crate::db::Db;
use crate::file_manager::normalize;
use crate::settings::DEFAULT_RENAME_PATTERN;

/// How a pattern relates to the built-in default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternKind {
    /// Blank: nothing can be renamed with it.
    Empty,
    /// The stable built-in default.
    StableDefault,
    /// Any other pattern; never altered by the application.
    Custom,
}

/// Recognize the built-in default by its text (line endings and surrounding blanks ignored).
pub fn classify(pattern: &str) -> PatternKind {
    let text = pattern.trim();
    if text.is_empty() {
        PatternKind::Empty
    } else if text.lines().eq(DEFAULT_RENAME_PATTERN.trim().lines()) {
        PatternKind::StableDefault
    } else {
        PatternKind::Custom
    }
}

/// Compile `source`, turning every reason it cannot be used into a readable message.
pub fn check_pattern(source: &str) -> Result<Pattern, String> {
    if source.trim().is_empty() {
        return Err(
            "The renaming pattern is empty. Use the stable default, or write a \
                    pattern whose last expression is the new file name."
                .into(),
        );
    }
    Pattern::compile(source).map_err(|e| {
        format!(
            "The renaming pattern has a syntax error: {}",
            e.message.replace("\r\n", "\n")
        )
    })
}

/// What a pattern would do to one file.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// Text the pattern evaluated to (empty if it never got that far).
    pub result: String,
    /// New path; `None` when blocked or when the file already has that name.
    pub target: Option<PathBuf>,
    /// Soft evaluation problems (wrong argument counts, unconvertible values). While any
    /// exist the output cannot be trusted, so the plan is not applicable.
    pub warnings: Vec<String>,
    /// Hard problems: bad pattern, missing metadata, unusable target.
    pub errors: Vec<String>,
}

impl Plan {
    /// A rename may be carried out: a target exists and nothing was reported.
    pub fn can_apply(&self) -> bool {
        self.target.is_some() && self.errors.is_empty() && self.warnings.is_empty()
    }

    /// The pattern is sound and the file already carries the name it produces.
    pub fn already_named(&self) -> bool {
        self.target.is_none() && self.errors.is_empty() && self.warnings.is_empty()
    }

    fn blocked(error: String) -> Plan {
        Plan {
            errors: vec![error],
            ..Plan::default()
        }
    }
}

/// Metadata the stable default builds its name from; while one is still a placeholder the
/// name would contain stand-in text.
fn stable_default_gaps(b: &Bindings) -> Vec<String> {
    let mut gaps = Vec::new();
    let title = b
        .anime
        .as_ref()
        .and_then(|a| a.prop("Name"))
        .map(Value::to_text)
        .unwrap_or_default();
    if title.trim().is_empty() || title == TITLE_PENDING {
        gaps.push(
            "The stable default needs the anime title, which has not been fetched yet. Fetch \
             the anime's information from AniDB first."
                .to_string(),
        );
    }
    let numbered = b
        .episode
        .as_ref()
        .is_some_and(|e| matches!(e.prop("EpisodeNo"), Some(Value::Int(n)) if *n > 0));
    if !numbered {
        gaps.push(
            "The stable default needs the episode number, which is not known yet. Fetch the \
             episode's information from AniDB first."
                .to_string(),
        );
    }
    gaps
}

/// Evaluate `pattern_source` once for the file at `old` and validate every step. Never panics.
/// Only the stable built-in default requires fetched title and episode metadata; any other
/// pattern is judged by its own result and warnings.
pub fn plan(pattern_source: &str, b: &Bindings, source: &dyn DataSource, old: &Path) -> Plan {
    let pattern = match check_pattern(pattern_source) {
        Ok(p) => p,
        Err(e) => return Plan::blocked(e),
    };
    if classify(pattern_source) == PatternKind::StableDefault {
        let gaps = stable_default_gaps(b);
        if !gaps.is_empty() {
            return Plan {
                errors: gaps,
                ..Plan::default()
            };
        }
    }
    let evaluation = match evaluate(&pattern, b, source) {
        Ok(v) => v,
        Err(e) => return Plan::blocked(format!("The renaming pattern failed to run: {e}")),
    };
    let mut plan = Plan {
        result: evaluation.result,
        warnings: evaluation.warnings,
        ..Plan::default()
    };
    if evaluation.boolean_result {
        plan.errors.push(format!(
            "The pattern ended with the truth value \"{}\" instead of a file name. Make its last \
             expression the new name (for example a variable holding it).",
            plan.result
        ));
        return plan;
    }
    match target_path(old, &plan.result, &b.base_path) {
        Ok(target) => plan.target = target,
        Err(e) => plan.errors.push(e),
    }
    plan
}

/// Plan the rename of the identified file `fid` stored at `path`, reading its metadata from `db`.
/// `base_path` is the file manager's rename destination (empty: the file's own folder).
pub fn plan_local_file(db: &Db, pattern: &str, fid: i64, path: &Path, base_path: &str) -> Plan {
    match bindings_for_file(db, fid, &path.to_string_lossy(), base_path) {
        Ok(bindings) => plan(pattern, &bindings, &DbSource { db }, path),
        Err(e) => Plan::blocked(e),
    }
}

/// One hashed library file offered to [`plan_batch`].
#[derive(Debug, Clone)]
pub struct Candidate {
    pub local_id: i64,
    pub path: PathBuf,
    /// AniDB file id; `None` for a file AniDB has not identified (it cannot be planned).
    pub fid: Option<i64>,
    /// Rename destination of the file's manager, as in [`plan_local_file`].
    pub base_path: String,
}

/// The planned rename of one [`Candidate`].
#[derive(Debug, Clone)]
pub struct BatchRow {
    pub local_id: i64,
    pub old: PathBuf,
    pub plan: Plan,
}

/// Plan every candidate with `pattern`, in order, one row each. Besides the diagnostics of
/// [`plan_local_file`], a row is blocked (an error is added, so [`Plan::can_apply`] is false) when
/// its target path is already taken by a file on disk, or when an earlier row of the batch
/// already claims the same target. Reads file metadata but changes nothing.
pub fn plan_batch(db: &Db, pattern: &str, files: &[Candidate]) -> Vec<BatchRow> {
    let mut claimed: HashMap<String, PathBuf> = HashMap::new();
    files
        .iter()
        .map(|file| {
            let mut plan = match file.fid {
                Some(fid) => plan_local_file(db, pattern, fid, &file.path, &file.base_path),
                None => Plan::blocked("The file is not identified by AniDB".into()),
            };
            if plan.can_apply()
                && let Some(target) = plan.target.clone()
            {
                if let Some(first) = claimed.get(&normalize(&target)) {
                    plan.errors.push(format!(
                        "{} would get the same name; only the first of the two can be renamed",
                        first.display()
                    ));
                } else if target.symlink_metadata().is_ok() {
                    plan.errors
                        .push("A file or folder already exists at the target path".into());
                } else {
                    claimed.insert(normalize(&target), file.path.clone());
                }
            }
            BatchRow {
                local_id: file.local_id,
                old: file.path.clone(),
                plan,
            }
        })
        .collect()
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    use animatic_anidb::commands::{EpNo, FileInfo};

    const PATTERN: &str = "A.Name + ' ' + E.EpisodeNo + '.mkv'";

    fn identified(db: &Db, dir: &Path, fid: i64, episode: i64) -> Candidate {
        let path = dir.join(format!("source {fid}.mkv"));
        db.store_file_info(&FileInfo {
            fid,
            aid: 2,
            eid: 20 + episode,
            size: 1000 + fid,
            ed2k: Some(format!("{fid:032x}")),
            anime_romaji: Some("Synthetic Show".into()),
            epno: EpNo {
                kind: 1,
                number: episode as u32,
            },
            ..FileInfo::default()
        })
        .unwrap();
        Candidate {
            local_id: fid,
            path,
            fid: Some(fid),
            base_path: String::new(),
        }
    }

    #[test]
    fn a_batch_plans_each_file_and_blocks_collisions_and_unidentified_files() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::memory().unwrap();
        let first = identified(&db, dir.path(), 1, 1);
        let second = identified(&db, dir.path(), 2, 2);
        let same_episode = identified(&db, dir.path(), 3, 1);
        let taken = identified(&db, dir.path(), 4, 4);
        std::fs::write(dir.path().join("Synthetic Show 4.mkv"), b"another file").unwrap();
        let unidentified = Candidate {
            local_id: 5,
            path: dir.path().join("unknown.mkv"),
            fid: None,
            base_path: String::new(),
        };
        let missing_data = Candidate {
            local_id: 6,
            path: dir.path().join("missing.mkv"),
            fid: Some(999),
            base_path: String::new(),
        };

        let rows = plan_batch(
            &db,
            PATTERN,
            &[
                first,
                second,
                same_episode,
                taken,
                unidentified,
                missing_data,
            ],
        );

        let ids: Vec<_> = rows.iter().map(|r| r.local_id).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5, 6], "one row per file, in order");
        let valid: Vec<_> = rows
            .iter()
            .filter(|r| r.plan.can_apply())
            .map(|r| r.local_id)
            .collect();
        assert_eq!(valid, [1, 2]);
        assert_eq!(
            rows[0].plan.target.as_deref(),
            Some(dir.path().join("Synthetic Show 1.mkv").as_path())
        );
        for blocked in &rows[2..] {
            assert!(!blocked.plan.errors.is_empty(), "{blocked:?}");
        }
        assert!(
            rows[2].plan.errors[0].contains("same name"),
            "{:?}",
            rows[2]
        );
        assert!(
            rows[3].plan.errors[0].contains("already exists"),
            "{:?}",
            rows[3]
        );
        assert!(
            rows[4].plan.errors[0].contains("not identified"),
            "{:?}",
            rows[4]
        );
    }

    #[test]
    fn a_blocked_pattern_blocks_every_row() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::memory().unwrap();
        let files = [
            identified(&db, dir.path(), 1, 1),
            identified(&db, dir.path(), 2, 2),
        ];

        let rows = plan_batch(&db, "set('a', ", &files);

        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .all(|r| !r.plan.can_apply() && !r.plan.errors.is_empty())
        );
    }
}
