//! The Animatic-owned temporary folder of a legacy import.
//!
//! The importer decrypts each `kowai/*.data` collection to a plain SQLite copy before it reads
//! it. Those copies hold the user's library in the clear, so they live in a folder that only
//! Animatic creates and recognises: a name starting with [`WORKDIR_PREFIX`] directly below the
//! temporary root, with a [`WORKDIR_MARKER`] file inside that is written before any decrypted
//! byte. The importer removes the folder when it ends, whether it succeeded or failed. A crash
//! or power loss can still leave it behind, and [`find_leftover_workdirs`] finds such folders on
//! the next start so that [`remove_leftover_workdirs`] can offer to remove them.
//!
//! An import holds an exclusive lock on its marker file for as long as it runs. The operating
//! system drops that lock when the process ends, however it ends, so a folder whose marker can be
//! locked belongs to no running import. Nothing here ever removes a path that is not such a
//! folder.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Name prefix of every import work folder.
pub const WORKDIR_PREFIX: &str = "animatic-legacy-import-";
/// File inside a work folder that marks it as Animatic's, created before any decrypted data.
pub const WORKDIR_MARKER: &str = ".animatic-legacy-import";

/// The temporary root used by the application: the operating system's temporary folder.
pub fn default_temp_root() -> PathBuf {
    std::env::temp_dir()
}

/// A work folder of the running import. Removed on [`ImportWorkdir::remove`] and on drop.
#[derive(Debug)]
pub struct ImportWorkdir {
    dir: PathBuf,
    /// The marker, locked exclusively while the import runs. `None` once released.
    marker: Option<File>,
}

impl ImportWorkdir {
    /// Create a new work folder below `temp_root` (created when missing) and lock its marker.
    /// The marker exists before this returns, so before any decrypted data can be written.
    pub fn create(temp_root: &Path) -> Result<ImportWorkdir> {
        std::fs::create_dir_all(temp_root)?;
        let dir = tempfile::Builder::new()
            .prefix(WORKDIR_PREFIX)
            .tempdir_in(temp_root)?
            .keep();
        let marker = match create_marker(&dir) {
            Ok(file) => file,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
        };
        Ok(ImportWorkdir {
            dir,
            marker: Some(marker),
        })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Release the lock and remove the folder with everything in it.
    pub fn remove(mut self) -> std::io::Result<()> {
        self.release()
    }

    fn release(&mut self) -> std::io::Result<()> {
        // The marker must be closed first: Windows cannot delete a file that is still open.
        self.marker = None;
        match std::fs::remove_dir_all(&self.dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

impl Drop for ImportWorkdir {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

fn create_marker(dir: &Path) -> Result<File> {
    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(WORKDIR_MARKER))?;
    marker.write_all(
        b"Temporary folder of an Animatic legacy databank import. It holds decrypted copies \
          of the databank and is safe to delete while no import is running.\n",
    )?;
    marker.try_lock().map_err(|e| match e {
        TryLockError::Error(e) => Error::Io(e),
        TryLockError::WouldBlock => {
            Error::Other("legacy import work folder is already locked".into())
        }
    })?;
    Ok(marker)
}

/// True when `dir` is a work folder (a real directory named with the prefix, holding a real
/// marker file) that no running import holds. Symbolic links never qualify.
fn is_idle_workdir(dir: &Path) -> bool {
    let named = dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(WORKDIR_PREFIX));
    if !named || !std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) {
        return false;
    }
    let marker = dir.join(WORKDIR_MARKER);
    if !std::fs::symlink_metadata(&marker).is_ok_and(|m| m.is_file()) {
        return false;
    }
    // Unreadable or locked markers are treated as in use: the safe answer is to keep the folder.
    OpenOptions::new()
        .write(true)
        .open(&marker)
        .is_ok_and(|file| file.try_lock().is_ok())
}

/// Work folders directly below `temp_root` that carry the marker and are not in use by a running
/// import (in this or another process), sorted by path. A missing `temp_root` has none.
pub fn find_leftover_workdirs(temp_root: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(temp_root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| is_idle_workdir(path))
        .collect();
    found.sort();
    Ok(found)
}

/// Outcome of [`remove_leftover_workdirs`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LeftoverRemoval {
    pub removed: Vec<PathBuf>,
    /// Folders that were kept, with the reason: no longer a work folder, in use, or not removable.
    pub kept: Vec<(PathBuf, String)>,
}

/// Remove the given folders, each only if it is still an idle work folder directly below
/// `temp_root`. Anything else in `dirs` is kept and reported; nothing else is touched.
pub fn remove_leftover_workdirs(temp_root: &Path, dirs: &[PathBuf]) -> LeftoverRemoval {
    let mut outcome = LeftoverRemoval::default();
    for dir in dirs {
        if dir.parent() != Some(temp_root) || !is_idle_workdir(dir) {
            outcome.kept.push((
                dir.clone(),
                "not an unused Animatic import folder".to_string(),
            ));
            continue;
        }
        match std::fs::remove_dir_all(dir) {
            Ok(()) => outcome.removed.push(dir.clone()),
            Err(e) => outcome.kept.push((dir.clone(), e.to_string())),
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leftover(root: &Path, name: &str, marked: bool) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("animes.sqlite"), b"synthetic plain copy").unwrap();
        if marked {
            std::fs::write(dir.join(WORKDIR_MARKER), b"").unwrap();
        }
        dir
    }

    #[test]
    fn a_new_workdir_is_marked_below_the_root_and_removed_with_its_contents() {
        let root = tempfile::tempdir().unwrap();
        let work = ImportWorkdir::create(root.path()).unwrap();
        let dir = work.path().to_path_buf();

        assert_eq!(dir.parent(), Some(root.path()));
        assert!(
            dir.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(WORKDIR_PREFIX)
        );
        assert!(dir.join(WORKDIR_MARKER).is_file());
        std::fs::write(dir.join("mylist.sqlite"), b"synthetic").unwrap();

        work.remove().unwrap();
        assert!(!dir.exists());
        assert!(root.path().exists());
    }

    #[test]
    fn a_dropped_workdir_removes_its_folder() {
        let root = tempfile::tempdir().unwrap();
        let dir = {
            let work = ImportWorkdir::create(root.path()).unwrap();
            work.path().to_path_buf()
        };
        assert!(!dir.exists());
    }

    #[test]
    fn discovery_returns_only_marked_idle_folders() {
        let root = tempfile::tempdir().unwrap();
        let marked = leftover(root.path(), "animatic-legacy-import-aaaa", true);
        let unmarked = leftover(root.path(), "animatic-legacy-import-bbbb", false);
        let other_name = leftover(root.path(), "animatic-test-cccc", true);
        std::fs::write(root.path().join("animatic-legacy-import-file"), b"x").unwrap();
        std::fs::write(root.path().join("unrelated.txt"), b"x").unwrap();

        assert_eq!(find_leftover_workdirs(root.path()).unwrap(), vec![marked]);
        assert!(unmarked.exists() && other_name.exists());
    }

    #[test]
    fn discovery_skips_the_folder_of_a_running_import() {
        let root = tempfile::tempdir().unwrap();
        let crashed = leftover(root.path(), "animatic-legacy-import-dead", true);
        let running = ImportWorkdir::create(root.path()).unwrap();

        assert_eq!(
            find_leftover_workdirs(root.path()).unwrap(),
            vec![crashed.clone()]
        );
        let outcome = remove_leftover_workdirs(
            root.path(),
            &[crashed.clone(), running.path().to_path_buf()],
        );
        assert_eq!(outcome.removed, vec![crashed]);
        assert_eq!(outcome.kept.len(), 1);
        assert!(running.path().join(WORKDIR_MARKER).is_file());
    }

    #[test]
    fn a_missing_temp_root_has_no_leftovers() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            find_leftover_workdirs(&root.path().join("absent"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn removal_leaves_unmarked_folders_other_paths_and_unrelated_files_alone() {
        let root = tempfile::tempdir().unwrap();
        let marked = leftover(root.path(), "animatic-legacy-import-aaaa", true);
        let unmarked = leftover(root.path(), "animatic-legacy-import-bbbb", false);
        let other_name = leftover(root.path(), "animatic-test-cccc", true);
        let unrelated = root.path().join("unrelated.txt");
        std::fs::write(&unrelated, b"keep").unwrap();
        // A folder outside the root, even one that looks right, is never removed.
        let elsewhere_root = tempfile::tempdir().unwrap();
        let elsewhere = leftover(elsewhere_root.path(), "animatic-legacy-import-dddd", true);
        let nested = {
            let inner = marked.join("animatic-legacy-import-nested");
            std::fs::create_dir_all(&inner).unwrap();
            std::fs::write(inner.join(WORKDIR_MARKER), b"").unwrap();
            inner
        };

        let requested = vec![
            marked.clone(),
            unmarked.clone(),
            other_name.clone(),
            unrelated.clone(),
            elsewhere.clone(),
            nested,
            root.path().to_path_buf(),
        ];
        let outcome = remove_leftover_workdirs(root.path(), &requested);

        assert_eq!(outcome.removed, vec![marked.clone()]);
        assert_eq!(outcome.kept.len(), requested.len() - 1);
        assert!(!marked.exists());
        assert!(unmarked.join("animes.sqlite").is_file());
        assert!(other_name.join("animes.sqlite").is_file());
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"keep");
        assert!(elsewhere.join("animes.sqlite").is_file());
        assert!(root.path().is_dir());
    }
}
