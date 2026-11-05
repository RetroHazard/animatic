//! Validated rename planning: turns a pattern and one file's metadata into either a target path
//! or explicit reasons why renaming must not happen. Nothing here touches the file system.

use std::path::{Path, PathBuf};

use super::objects::TITLE_PENDING;
use super::{Bindings, DataSource, Pattern, Value, evaluate, target_path};
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
