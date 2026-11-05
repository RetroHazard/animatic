//! The AniDB O'Matic renaming language, re-implemented.
//!
//! Implements the clean-room behavior specification recovered for AOM's rename language. Patterns
//! written for the legacy client produce the same names; deliberate differences are that
//! strings are Unicode throughout (no ANSI round trips) and relative results resolve
//! against the file's own folder.

pub mod eval;
pub mod objects;
pub mod parser;
pub mod value;

use std::path::{Path, PathBuf};

pub use eval::{DataSource, Evaluator, Object};
pub use parser::{CompileError, Node};
pub use value::Value;

/// Hard evaluation error (aborts the rename).
#[derive(Debug, Clone, PartialEq)]
pub struct EvalError {
    pub message: String,
}

impl EvalError {
    pub fn new(message: String) -> Self {
        EvalError { message }
    }
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// A compiled pattern.
#[derive(Debug, Clone)]
pub struct Pattern {
    pub source: String,
    pub preprocessed: String,
    tree: Option<Node>,
}

impl Pattern {
    pub fn compile(source: &str) -> Result<Pattern, CompileError> {
        let preprocessed = parser::preprocess(source);
        let tree = parser::compile(&preprocessed)?;
        Ok(Pattern {
            source: source.to_string(),
            preprocessed,
            tree,
        })
    }
}

/// Objects bound for one evaluation (`A`, `E`, `F`, `G`, `H`, `M`).
#[derive(Debug, Clone, Default)]
pub struct Bindings {
    pub anime: Option<Object>,
    pub episode: Option<Object>,
    pub file: Option<Object>,
    pub group: Option<Object>,
    pub hash: Option<Object>,
    pub mylist: Option<Object>,
    /// Current full path of the file ("" for ed2k links).
    pub path: String,
    /// File manager base path ("" if none).
    pub base_path: String,
}

#[derive(Debug, Clone)]
pub struct Evaluation {
    pub result: String,
    /// Variables after evaluation (for "Test auto rename").
    pub variables: Vec<(String, String)>,
    /// Soft errors (wrong argument counts, unconvertible values).
    pub warnings: Vec<String>,
}

pub fn evaluate(
    pattern: &Pattern,
    b: &Bindings,
    source: &dyn DataSource,
) -> Result<Evaluation, EvalError> {
    let mut ev = Evaluator::new(source);
    ev.set_variable("ACTIVEPATTERN", Value::WStr(pattern.preprocessed.clone()));
    ev.set_variable("PATH", Value::WStr(b.path.clone()));
    ev.set_variable("BASEPATH", Value::WStr(b.base_path.clone()));
    ev.set_object("A", b.anime.clone());
    ev.set_object("E", b.episode.clone());
    ev.set_object("F", b.file.clone());
    ev.set_object("G", b.group.clone());
    ev.set_object("H", b.hash.clone());
    ev.set_object("M", b.mylist.clone());
    let v = ev.run(pattern.tree.as_ref())?;
    let result = v.to_text().replace(['\r', '\n'], "").replace("\\\\", "\\");
    Ok(Evaluation {
        result,
        variables: ev
            .variables()
            .iter()
            .filter(|(n, _)| n != "ACTIVEPATTERN")
            .map(|(n, v)| (n.clone(), v.to_text()))
            .collect(),
        warnings: ev.errors.clone(),
    })
}

/// Turn an evaluation result into the new path of `old` (legacy file-manager rules):
/// base path join, plain names stay in the current folder, every component is trimmed of
/// trailing dots/spaces and must not be empty. Returns `Ok(None)` when there is nothing to do.
pub fn target_path(old: &Path, result: &str, base_path: &str) -> Result<Option<PathBuf>, String> {
    if result.is_empty() {
        return Ok(None);
    }
    let is_sep = |c: char| c == '\\' || c == '/';
    let mut name = result.to_string();
    if !base_path.is_empty() {
        name = if name.starts_with(is_sep) {
            format!("{}{}", base_path.trim_end_matches(is_sep), name)
        } else {
            format!(
                "{}{}{}",
                base_path.trim_end_matches(is_sep),
                std::path::MAIN_SEPARATOR,
                name
            )
        };
    }
    let invalid = || {
        format!(
            "Renaming \"{}\" to \"{}\" would result in an invalid file name, rename skipped!",
            old.display(),
            name
        )
    };
    let (mut out, rest) = match split_root(&name, old) {
        Some((root, rest)) => (root, rest),
        // Relative results (plain names included) stay next to the current file.
        None => (
            old.parent().map(Path::to_path_buf).unwrap_or_default(),
            name.as_str(),
        ),
    };
    for comp in rest.split(is_sep) {
        let c = comp
            .trim_end_matches(['.', ' '])
            .trim_matches(|ch: char| ch <= ' ');
        if c.is_empty() {
            return Err(invalid());
        }
        out.push(c);
    }
    if out == old {
        return Ok(None);
    }
    Ok(Some(out))
}

/// Split an absolute name into its root (drive, UNC share or `/`) and the remainder.
/// A single leading backslash means the root of the current file's drive (Windows semantics).
fn split_root<'a>(name: &'a str, old: &Path) -> Option<(PathBuf, &'a str)> {
    let b = name.as_bytes();
    let is_sep = |c: char| c == '\\' || c == '/';
    if let Some(unc) = name.strip_prefix("\\\\") {
        let mut parts = unc.splitn(3, is_sep);
        let server = parts.next().unwrap_or("");
        let share = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        return Some((PathBuf::from(format!("\\\\{server}\\{share}\\")), rest));
    }
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return Some((
            PathBuf::from(format!("{}{}", &name[..2], std::path::MAIN_SEPARATOR)),
            name[2..].trim_start_matches(is_sep),
        ));
    }
    if name.starts_with(is_sep) {
        let root = old
            .ancestors()
            .last()
            .filter(|r| !r.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR_STR));
        return Some((root, name.trim_start_matches(is_sep)));
    }
    None
}

#[cfg(test)]
mod tests;
