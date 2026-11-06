//! Finding the `ffprobe` executable.
//!
//! Order of the search ([`Order`]):
//!
//! * Windows: the configured path, then Animatic's own copy (installed from Options into
//!   `<data folder>/tools/ffprobe/`), then `PATH`.
//! * Linux: the configured path, then `PATH` (a distribution-packaged `ffprobe` is preferred
//!   when present), then Animatic's own copy.
//!
//! A candidate counts only when it is a file that answers `ffprobe -version` with a first line
//! of `ffprobe version ...`. An unusable configured path is reported in
//! [`Detection::notes`] and the search continues, so a stale setting does not hide a working
//! copy.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::Result;

/// File name of the executable on this platform.
pub const FILE_NAME: &str = if cfg!(windows) {
    "ffprobe.exe"
} else {
    "ffprobe"
};

/// File name of the FFmpeg license text kept beside the installed executable.
pub const LICENSE_NAME: &str = "LICENSE-ffmpeg.txt";

/// Where a found `ffprobe` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSource {
    /// The path in Options.
    Configured,
    /// The copy Animatic installed into its data folder.
    Installed,
    /// An `ffprobe` on `PATH`.
    Path,
}

impl ToolSource {
    pub fn describe(self) -> &'static str {
        match self {
            ToolSource::Configured => "configured path",
            ToolSource::Installed => "installed by Animatic",
            ToolSource::Path => "found on PATH",
        }
    }
}

/// Position of `PATH` relative to Animatic's own copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Own copy before `PATH` (Windows, where no distribution packages exist).
    OwnCopyFirst,
    /// `PATH` before the own copy (Linux).
    PathFirst,
}

impl Order {
    pub fn for_platform() -> Order {
        if cfg!(windows) {
            Order::OwnCopyFirst
        } else {
            Order::PathFirst
        }
    }
}

/// A working `ffprobe`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ffprobe {
    pub path: PathBuf,
    /// The version token, for example `n9.0.2-39-g7adfbe40dc` or `6.1.1-3ubuntu5`.
    pub version: String,
    pub source: ToolSource,
}

/// Result of a search: the tool that was chosen and what was wrong with earlier candidates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detection {
    pub tool: Option<Ffprobe>,
    /// Problems with candidates that were skipped (only the configured path is reported; a
    /// missing copy on `PATH` is the normal case).
    pub notes: Vec<String>,
}

/// Folder Animatic installs its own `ffprobe` into: `<data folder>/tools/ffprobe`.
pub fn install_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("tools").join("ffprobe")
}

/// The candidates in search order. Only existing regular files are listed; `PATH` entries that are
/// not absolute are ignored (they would depend on the working directory).
pub fn candidates(
    configured: &str,
    install_dir: &Path,
    path_var: Option<&OsStr>,
    order: Order,
) -> Vec<(PathBuf, ToolSource)> {
    let mut found = Vec::new();
    let configured = configured.trim();
    if !configured.is_empty() {
        let mut path = PathBuf::from(configured);
        if path.is_dir() {
            path.push(FILE_NAME);
        }
        found.push((path, ToolSource::Configured));
    }
    let own = || {
        let path = install_dir.join(FILE_NAME);
        path.is_file().then_some((path, ToolSource::Installed))
    };
    let on_path = || {
        path_var.into_iter().flat_map(|var| {
            std::env::split_paths(var)
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join(FILE_NAME))
                .filter(|path| path.is_file())
                .map(|path| (path, ToolSource::Path))
        })
    };
    match order {
        Order::OwnCopyFirst => {
            found.extend(own());
            found.extend(on_path());
        }
        Order::PathFirst => {
            found.extend(on_path());
            found.extend(own());
        }
    }
    found
}

/// Search with `check` deciding whether a candidate is a working `ffprobe` (it returns the
/// version). Separate from [`detect`] so that the order can be tested without processes.
pub fn detect_with(
    configured: &str,
    install_dir: &Path,
    path_var: Option<&OsStr>,
    order: Order,
    check: &dyn Fn(&Path) -> Result<String>,
) -> Detection {
    let mut detection = Detection::default();
    for (path, source) in candidates(configured, install_dir, path_var, order) {
        if source == ToolSource::Configured && !path.is_file() {
            detection.notes.push(format!(
                "The configured path {} is not a file",
                path.display()
            ));
            continue;
        }
        match check(&path) {
            Ok(version) => {
                detection.tool = Some(Ffprobe {
                    path,
                    version,
                    source,
                });
                return detection;
            }
            Err(e) if source == ToolSource::Configured => detection.notes.push(format!(
                "The configured path {} is not usable: {e}",
                path.display()
            )),
            Err(_) => {}
        }
    }
    detection
}

/// Search the real system: the process environment's `PATH` and `ffprobe -version` on each
/// candidate. Runs processes; call it from a worker thread.
pub fn detect(configured: &str, data_dir: &Path, cancel: &AtomicBool) -> Detection {
    let path_var = std::env::var_os("PATH");
    detect_with(
        configured,
        &install_dir(data_dir),
        path_var.as_deref(),
        Order::for_platform(),
        &|path| super::probe::ffprobe_version(path, cancel),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Layout {
        _root: tempfile::TempDir,
        configured: PathBuf,
        install: PathBuf,
        path_dir: PathBuf,
        path_var: std::ffi::OsString,
    }

    fn layout() -> Layout {
        let root = tempfile::tempdir().unwrap();
        let configured = root.path().join("custom").join("my-ffprobe");
        let install = install_dir(&root.path().join("data"));
        let path_dir = root.path().join("bin");
        let other = root.path().join("other-bin");
        for dir in [configured.parent().unwrap(), &install, &path_dir, &other] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(&configured, "x").unwrap();
        fs::write(install.join(FILE_NAME), "x").unwrap();
        fs::write(path_dir.join(FILE_NAME), "x").unwrap();
        let path_var = std::env::join_paths([&other, &path_dir]).unwrap();
        Layout {
            _root: root,
            configured,
            install,
            path_dir,
            path_var,
        }
    }

    fn accept_all(_: &Path) -> Result<String> {
        Ok("9.9".into())
    }

    fn run(layout: &Layout, configured: &str, order: Order) -> Detection {
        detect_with(
            configured,
            &layout.install,
            Some(&layout.path_var),
            order,
            &accept_all,
        )
    }

    #[test]
    fn a_configured_path_wins_over_everything() {
        let l = layout();
        for order in [Order::OwnCopyFirst, Order::PathFirst] {
            let d = run(&l, l.configured.to_str().unwrap(), order);
            let tool = d.tool.unwrap();
            assert_eq!(tool.path, l.configured);
            assert_eq!(tool.source, ToolSource::Configured);
            assert_eq!(tool.version, "9.9");
        }
    }

    #[test]
    fn own_copy_comes_before_path_on_windows_and_after_it_on_linux() {
        let l = layout();
        let own = run(&l, "", Order::OwnCopyFirst).tool.unwrap();
        assert_eq!(own.source, ToolSource::Installed);
        assert_eq!(own.path, l.install.join(FILE_NAME));
        let path = run(&l, "", Order::PathFirst).tool.unwrap();
        assert_eq!(path.source, ToolSource::Path);
        assert_eq!(path.path, l.path_dir.join(FILE_NAME));
    }

    #[test]
    fn the_other_location_is_used_when_the_first_has_nothing() {
        let l = layout();
        fs::remove_file(l.install.join(FILE_NAME)).unwrap();
        assert_eq!(
            run(&l, "", Order::OwnCopyFirst).tool.unwrap().source,
            ToolSource::Path
        );
        fs::remove_file(l.path_dir.join(FILE_NAME)).unwrap();
        assert_eq!(run(&l, "", Order::OwnCopyFirst).tool, None);
        assert_eq!(run(&l, "", Order::PathFirst).tool, None);
    }

    #[test]
    fn an_unusable_configured_path_is_reported_and_the_search_goes_on() {
        let l = layout();
        let missing = l.configured.with_file_name("missing");
        let d = run(&l, missing.to_str().unwrap(), Order::OwnCopyFirst);
        assert_eq!(d.tool.unwrap().source, ToolSource::Installed);
        assert_eq!(d.notes.len(), 1);
        assert!(d.notes[0].contains("is not a file"), "{:?}", d.notes);

        // A file that does not behave like ffprobe is skipped with the reason.
        let refuse = |path: &Path| -> Result<String> {
            if path == l.configured {
                Err(super::super::MediaError::NotFfprobe(
                    path.display().to_string(),
                ))
            } else {
                Ok("1".into())
            }
        };
        let d = detect_with(
            l.configured.to_str().unwrap(),
            &l.install,
            Some(&l.path_var),
            Order::PathFirst,
            &refuse,
        );
        assert_eq!(d.tool.unwrap().source, ToolSource::Path);
        assert!(d.notes[0].contains("is not usable"), "{:?}", d.notes);
    }

    #[test]
    fn a_configured_folder_means_the_executable_inside_it() {
        let l = layout();
        let folder = l.install.clone();
        let d = run(&l, folder.to_str().unwrap(), Order::PathFirst);
        let tool = d.tool.unwrap();
        assert_eq!(tool.source, ToolSource::Configured);
        assert_eq!(tool.path, folder.join(FILE_NAME));
    }

    #[test]
    fn relative_path_entries_are_ignored() {
        let l = layout();
        let var = std::env::join_paths([Path::new("."), Path::new("relative-bin")]).unwrap();
        let found = candidates("", &l.install, Some(&var), Order::PathFirst);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, ToolSource::Installed);
    }

    #[test]
    fn the_platform_order_follows_the_documented_one() {
        assert_eq!(
            Order::for_platform(),
            if cfg!(windows) {
                Order::OwnCopyFirst
            } else {
                Order::PathFirst
            }
        );
        assert_eq!(
            install_dir(Path::new("data")),
            Path::new("data").join("tools").join("ffprobe")
        );
    }

    #[test]
    fn version_detection_requires_the_ffprobe_banner() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let good = super::super::probe::tests::fake_ffprobe(
            dir.path(),
            "ffprobe version n9.0.2-39-gabc Copyright (c) the FFmpeg developers\nbuilt with gcc\n",
            0,
        );
        assert_eq!(
            super::super::probe::ffprobe_version(&good, &cancel).unwrap(),
            "n9.0.2-39-gabc"
        );
        let other = tempfile::tempdir().unwrap();
        let bad = super::super::probe::tests::fake_ffprobe(other.path(), "hello world\n", 0);
        assert!(matches!(
            super::super::probe::ffprobe_version(&bad, &cancel),
            Err(super::super::MediaError::NotFfprobe(_))
        ));
        let failing = tempfile::tempdir().unwrap();
        let fail =
            super::super::probe::tests::fake_ffprobe(failing.path(), "ffprobe version 1\n", 2);
        assert!(super::super::probe::ffprobe_version(&fail, &cancel).is_err());
    }
}
