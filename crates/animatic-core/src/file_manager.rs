//! File discovery and automatic file-manager operations.
//!
//! Discovery is side-effect free. The desktop worker chooses which planned actions to execute,
//! keeping hashing, network access and filesystem mutations off the UI thread.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::UNIX_EPOCH;

use walkdir::{DirEntry, WalkDir};

use crate::db::Db;
use crate::error::{Error, Result};
use crate::hash::{self, Control, Ed2kVariant, Hashes};
use crate::settings::{FileManager, FileManagerSettings};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanAction {
    Hash {
        path: PathBuf,
        manager: usize,
    },
    Rehash {
        local_id: i64,
        path: PathBuf,
        manager: usize,
    },
    RemoveMissing {
        local_id: i64,
        path: PathBuf,
    },
}

fn modified_seconds(path: &Path) -> Result<i64> {
    Ok(fs::metadata(path)?
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64)
}

fn hidden(entry: &DirEntry) -> bool {
    entry.file_name().to_string_lossy().starts_with('.')
}

fn extension_allowed(path: &Path, extensions: &HashSet<String>) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| extensions.contains(&e.to_ascii_lowercase()))
        .unwrap_or(false)
}

pub fn extension_set(csv: &str) -> HashSet<String> {
    csv.split([',', ';', ' '])
        .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Plan one complete scan. Existing paths are compared by normalized, platform-native paths.
pub fn plan_scan(
    db: &Db,
    settings: &FileManagerSettings,
    extensions_csv: &str,
) -> Result<Vec<ScanAction>> {
    let extensions = extension_set(extensions_csv);
    let existing = db.local_files()?;
    let mut known = std::collections::HashMap::new();
    for f in &existing {
        known.insert(normalize(Path::new(&f.path)), (f.id, f.mtime));
    }

    let mut actions = Vec::new();
    let mut seen = HashSet::new();
    for (manager_index, manager) in settings.managers.iter().enumerate() {
        if manager.path.is_empty() || !(manager.auto_hash || manager.auto_rehash) {
            continue;
        }
        let root = Path::new(&manager.path);
        if !root.is_dir() {
            continue;
        }
        let depth = if manager.include_subdirs {
            usize::MAX
        } else {
            1
        };
        let walker = WalkDir::new(root)
            .follow_links(false)
            .max_depth(depth)
            .into_iter()
            .filter_entry(|entry| !(manager.exclude_hidden && entry.depth() > 0 && hidden(entry)));
        for entry in walker
            .filter_map(|r| r.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            if !extension_allowed(path, &extensions) {
                continue;
            }
            let key = normalize(path);
            if !seen.insert(key.clone()) {
                continue;
            }
            match known.get(&key).copied() {
                None if manager.auto_hash => actions.push(ScanAction::Hash {
                    path: path.to_path_buf(),
                    manager: manager_index,
                }),
                Some((id, old_mtime))
                    if manager.auto_rehash
                        && modified_seconds(path).unwrap_or(old_mtime) != old_mtime =>
                {
                    actions.push(ScanAction::Rehash {
                        local_id: id,
                        path: path.to_path_buf(),
                        manager: manager_index,
                    });
                }
                _ => {}
            }
        }
    }
    for f in existing {
        if !Path::new(&f.path).exists() {
            let manager = settings.manager_for(Path::new(&f.path));
            if manager.auto_remove {
                actions.push(ScanAction::RemoveMissing {
                    local_id: f.id,
                    path: PathBuf::from(f.path),
                });
            }
        }
    }
    Ok(actions)
}

/// Hash and persist one file, reporting bytes processed. The caller owns cancellation/pause flags.
pub fn hash_and_register(
    db: &Db,
    path: &Path,
    variant: Ed2kVariant,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(i64, Hashes)> {
    let hashes = hash::hash_file(
        path,
        variant,
        Control {
            cancel,
            pause,
            progress,
        },
    )?;
    let mtime = modified_seconds(path)?;
    let id = db.add_local_file(&path.to_string_lossy(), &hashes, mtime)?;
    Ok((id, hashes))
}

/// Move a local file and update its database path only after the filesystem operation succeeds.
pub fn move_file(db: &Db, local_id: i64, old: &Path, new: &Path) -> Result<()> {
    if old == new {
        return Ok(());
    }
    if new.exists() {
        return Err(Error::Other(format!(
            "destination already exists: {}",
            new.display()
        )));
    }
    if let Some(parent) = new.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(old, new)?;
    db.move_local_file(local_id, &new.to_string_lossy())?;
    Ok(())
}

pub fn remove_missing(db: &Db, local_id: i64) -> Result<()> {
    db.remove_local_file(local_id)
}

fn normalize(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('\\', "/");
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    if cfg!(windows) {
        s.to_ascii_lowercase()
    } else {
        s
    }
}

/// Manager parameters relevant to a discovered file.
pub fn manager_for_path<'a>(settings: &'a FileManagerSettings, path: &Path) -> &'a FileManager {
    settings.manager_for(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_parser_is_case_insensitive_and_accepts_dots() {
        let set = extension_set(".MKV, avi; mp4");
        assert!(extension_allowed(Path::new("x.MkV"), &set));
        assert!(!extension_allowed(Path::new("x.exe"), &set));
    }

    #[test]
    fn scan_plans_new_file_and_ignores_hidden_and_nested_when_disabled() {
        let root = std::env::temp_dir().join(format!("animatic-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("one.mkv"), b"one").unwrap();
        fs::write(root.join(".hidden.mkv"), b"hidden").unwrap();
        fs::write(root.join("sub/two.mkv"), b"two").unwrap();
        let db = Db::memory().unwrap();
        let m = FileManager {
            path: root.to_string_lossy().into_owned(),
            auto_hash: true,
            include_subdirs: false,
            ..Default::default()
        };
        let settings = FileManagerSettings {
            managers: vec![m],
            ..Default::default()
        };
        let actions = plan_scan(&db, &settings, "mkv").unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], ScanAction::Hash { path, .. } if path.ends_with("one.mkv")));
        fs::remove_dir_all(root).unwrap();
    }
}
