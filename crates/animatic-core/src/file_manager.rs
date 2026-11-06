//! File discovery and automatic file-manager operations.
//!
//! Discovery is side-effect free. The desktop worker chooses which planned actions to execute,
//! keeping hashing, network access and filesystem mutations off the UI thread.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use walkdir::{DirEntry, WalkDir};

use crate::db::Db;
use crate::error::{Error, Result};
use crate::hash::{self, Control, Ed2kVariant, Hashes};
use crate::settings::{FileManager, FileManagerSettings};

/// ED2K variant of every hash in the library (the worker hashes with it); the verification of a
/// cross-volume copy uses the same one so the results are comparable.
const LIBRARY_ED2K: Ed2kVariant = Ed2kVariant::Red;

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
/// Automatic removals require both a missing file and a reachable configured manager directory.
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
        let path = Path::new(&f.path);
        let Some(manager) = settings.matching_manager_for(path) else {
            continue;
        };
        if !manager.auto_remove {
            continue;
        }
        if !matches!(
            fs::metadata(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ) {
            continue;
        }
        if !matches!(fs::metadata(&manager.path), Ok(metadata) if metadata.is_dir()) {
            continue;
        }
        actions.push(ScanAction::RemoveMissing {
            local_id: f.id,
            path: PathBuf::from(f.path),
        });
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

/// How [`move_file`] carried a move out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveMethod {
    /// One `rename` on the same volume.
    Renamed,
    /// The volumes differ: copied to a temporary name beside the target, verified against the
    /// stored hash, renamed into place and only then removed from the old location.
    Copied,
}

/// Result of a successful move. The library already points at the new path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveOutcome {
    pub method: MoveMethod,
    /// Set when the verified copy is in place and recorded but the old file could not be
    /// removed (for example because a player holds it open). Nothing was lost; the old file is
    /// an extra copy the user may delete.
    pub source_left: Option<String>,
}

/// Cancellation, pause and progress hooks for [`move_file`]. `progress(done, total)` counts the
/// copy and the verification read together, so it only advances for a cross-volume move.
pub struct MoveControl<'a> {
    pub cancel: &'a AtomicBool,
    pub pause: &'a AtomicBool,
    pub progress: &'a mut dyn FnMut(u64, u64),
}

/// Move a local file to `new` and record the new path. Never overwrites an existing target.
///
/// Same volume: one `rename`. When it fails because the volumes differ the file is copied to a
/// temporary name in the target folder, the copy is verified (size, then its ED2K re-hashed with
/// the single-pass hasher and compared with the hash stored for the library row — a deliberate
/// extra read of the whole file, because only that proves the copy is the file the library
/// knows), renamed into place without replacing anything, the database is updated and only then
/// is the source removed.
///
/// On any failure or cancellation before that last step the temporary copy is deleted and the
/// source and the library record are untouched; a failure to update the library removes the
/// copy again. The library therefore never points at a missing file and the only copy is never
/// deleted. Cancellation is checked while copying and verifying, not during the final swap.
pub fn move_file(
    db: &Db,
    local_id: i64,
    old: &Path,
    new: &Path,
    ctl: MoveControl,
) -> Result<MoveOutcome> {
    move_file_with(db, local_id, old, new, ctl, &|from, to| {
        fs::rename(from, to)
    })
}

/// [`move_file`] with the same-volume step injected, so the cross-volume path can be driven on a
/// single volume by a `rename` that fails with [`std::io::ErrorKind::CrossesDevices`].
pub fn move_file_with(
    db: &Db,
    local_id: i64,
    old: &Path,
    new: &Path,
    ctl: MoveControl,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<MoveOutcome> {
    let same = MoveOutcome {
        method: MoveMethod::Renamed,
        source_left: None,
    };
    if old == new {
        return Ok(same);
    }
    let (recorded_path, stored) = db
        .local_file_record(local_id)?
        .ok_or_else(|| Error::Other("the file is no longer in the library".into()))?;
    if Path::new(&recorded_path) != old {
        return Err(Error::Other(format!(
            "the library now records this file at {recorded_path}, not {}",
            old.display()
        )));
    }
    if new.symlink_metadata().is_ok() {
        return Err(Error::Other(format!(
            "destination already exists: {}",
            new.display()
        )));
    }
    if let Some(parent) = new.parent() {
        fs::create_dir_all(parent)?;
    }
    match rename(old, new) {
        Ok(()) => {
            if let Err(e) = db.move_local_file(local_id, &new.to_string_lossy()) {
                // The record must not point at a path that no longer exists.
                let _ = rename(new, old);
                return Err(e);
            }
            Ok(same)
        }
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_verify_swap(db, local_id, old, new, &stored, ctl)
        }
        Err(e) => Err(e.into()),
    }
}

/// Cross-volume move: see [`move_file`].
fn copy_verify_swap(
    db: &Db,
    local_id: i64,
    old: &Path,
    new: &Path,
    stored: &Hashes,
    ctl: MoveControl,
) -> Result<MoveOutcome> {
    use std::io::{Read, Write};

    let MoveControl {
        cancel,
        pause,
        progress,
    } = ctl;
    if stored.ed2k.is_empty() {
        return Err(Error::Other(
            "the library has no stored hash for this file, so a copy to another volume cannot \
             be verified"
                .into(),
        ));
    }
    let mut source = fs::File::open(old)?;
    let metadata = source.metadata()?;
    let size = metadata.len();
    if size != stored.size {
        return Err(Error::Other(
            "the file changed since it was hashed; hash it again before moving it".into(),
        ));
    }
    let total = size.saturating_mul(2).max(1);
    let folder = new
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    // Dropped (deleted) on every early return below; hidden and not a media extension, so a
    // scan never registers it.
    let mut temp = tempfile::Builder::new()
        .prefix(".animatic-move-")
        .suffix(".tmp")
        .tempfile_in(folder)?;
    let mut buffer = vec![0u8; 4 << 20];
    let mut copied = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        while pause.load(Ordering::Relaxed) && !cancel.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let n = match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        temp.as_file_mut().write_all(&buffer[..n])?;
        copied += n as u64;
        progress(copied.min(size), total);
    }
    drop(source);
    drop(buffer);
    // Keep the modification time: a different one would make the next scan "rehash" the file.
    if let Ok(modified) = metadata.modified() {
        let _ = temp.as_file().set_modified(modified);
    }
    let _ = fs::set_permissions(temp.path(), metadata.permissions());
    temp.as_file().sync_all()?;
    let temp = temp.into_temp_path();
    if fs::metadata(&temp)?.len() != size || copied != size {
        return Err(Error::Other(
            "the copy has a different size than the original (the file may have changed while \
             it was copied); nothing was moved"
                .into(),
        ));
    }
    let verified = hash::hash_file(
        &temp,
        LIBRARY_ED2K,
        Control {
            cancel,
            pause,
            progress: &mut |done, _| progress(size + done.min(size), total),
        },
    )?;
    if verified.size != stored.size || !verified.ed2k.eq_ignore_ascii_case(&stored.ed2k) {
        return Err(Error::Other(
            "the copy does not match the hash stored for this file; nothing was moved".into(),
        ));
    }
    // Refuses to replace anything that appeared since the check in `move_file`.
    temp.persist_noclobber(new)
        .map_err(|e| match e.error.kind() {
            std::io::ErrorKind::AlreadyExists => {
                Error::Other(format!("destination already exists: {}", new.display()))
            }
            _ => Error::Io(e.error),
        })?;
    if let Err(e) = db.move_local_file(local_id, &new.to_string_lossy()) {
        // We created `new` a moment ago and the source is intact: undo.
        let _ = fs::remove_file(new);
        return Err(e);
    }
    let source_left = fs::remove_file(old).err().map(|e| {
        format!(
            "the copy is in place and recorded, but {} could not be removed: {e}",
            old.display()
        )
    });
    Ok(MoveOutcome {
        method: MoveMethod::Copied,
        source_left,
    })
}

pub fn remove_missing(db: &Db, local_id: i64) -> Result<()> {
    db.remove_local_file(local_id)
}

pub(crate) fn normalize(path: &Path) -> String {
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

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "animatic-file-manager-{name}-{}",
            std::process::id()
        ))
    }

    fn removal_scan(root: &Path, path: &Path, auto_remove: bool) -> (i64, Vec<ScanAction>) {
        let db = Db::memory().unwrap();
        let local_id = db
            .add_local_file(&path.to_string_lossy(), &Hashes::default(), 0)
            .unwrap();
        let settings = FileManagerSettings {
            managers: vec![FileManager {
                path: root.to_string_lossy().into_owned(),
                include_subdirs: true,
                auto_remove,
                ..Default::default()
            }],
            ..Default::default()
        };
        (local_id, plan_scan(&db, &settings, "mkv").unwrap())
    }

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

    #[test]
    fn scan_preserves_missing_records_when_matching_root_is_unavailable() {
        let root = test_root("unavailable-root");
        let _ = fs::remove_dir_all(&root);

        let (_, actions) = removal_scan(&root, &root.join("missing.mkv"), true);

        assert!(actions.is_empty());
    }

    #[test]
    fn scan_removes_missing_record_under_available_root() {
        let root = test_root("available-root");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("missing.mkv");

        let (local_id, actions) = removal_scan(&root, &path, true);

        assert_eq!(
            actions,
            vec![ScanAction::RemoveMissing {
                local_id,
                path: path.clone(),
            }]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scan_removes_missing_record_when_nested_parent_is_absent() {
        let root = test_root("nested-parent");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("missing-parent").join("missing.mkv");

        let (local_id, actions) = removal_scan(&root, &path, true);

        assert_eq!(
            actions,
            vec![ScanAction::RemoveMissing {
                local_id,
                path: path.clone(),
            }]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scan_preserves_record_when_file_metadata_fails() {
        let root = test_root("metadata-error");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("invalid\0.mkv");
        assert_ne!(
            fs::metadata(&path).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );

        let (_, actions) = removal_scan(&root, &path, true);

        assert!(actions.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scan_preserves_record_when_matching_root_is_not_a_directory() {
        let root = test_root("file-root");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&root);
        fs::write(&root, b"not a directory").unwrap();

        let (_, actions) = removal_scan(&root, &root.join("missing.mkv"), true);

        assert!(actions.is_empty());
        fs::remove_file(root).unwrap();
    }

    #[test]
    fn scan_default_rule_only_removes_explicitly_matching_paths() {
        let root = test_root("default-membership");
        let managed = root.join("managed");
        fs::create_dir_all(&managed).unwrap();
        let db = Db::memory().unwrap();
        for path in [
            root.join("unmanaged/missing.mkv"),
            root.join("managed-sibling/missing.mkv"),
            managed.join("nested/missing.mkv"),
        ] {
            db.add_local_file(&path.to_string_lossy(), &Hashes::default(), 0)
                .unwrap();
        }
        let matching_path = managed.join("missing.mkv");
        let local_id = db
            .add_local_file(&matching_path.to_string_lossy(), &Hashes::default(), 0)
            .unwrap();
        let settings = FileManagerSettings {
            managers: vec![FileManager {
                path: managed.to_string_lossy().into_owned(),
                include_subdirs: false,
                auto_remove: true,
                default: true,
                ..Default::default()
            }],
            ..Default::default()
        };

        let actions = plan_scan(&db, &settings, "mkv").unwrap();

        assert_eq!(
            actions,
            vec![ScanAction::RemoveMissing {
                local_id,
                path: matching_path,
            }]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scan_preserves_record_for_empty_default_manager_path() {
        let path = test_root("empty-default").join("missing.mkv");
        let db = Db::memory().unwrap();
        db.add_local_file(&path.to_string_lossy(), &Hashes::default(), 0)
            .unwrap();
        let settings = FileManagerSettings {
            managers: vec![FileManager {
                auto_remove: true,
                default: true,
                ..Default::default()
            }],
            ..Default::default()
        };

        let actions = plan_scan(&db, &settings, "mkv").unwrap();

        assert!(actions.is_empty());
    }

    #[test]
    fn scan_respects_auto_remove_and_manager_precedence() {
        let root = test_root("manager-precedence");
        let nested_root = root.join("nested");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&nested_root).unwrap();
        let path = nested_root.join("missing.mkv");
        let db = Db::memory().unwrap();
        db.add_local_file(&path.to_string_lossy(), &Hashes::default(), 0)
            .unwrap();
        let settings = FileManagerSettings {
            managers: vec![
                FileManager {
                    path: root.to_string_lossy().into_owned(),
                    include_subdirs: true,
                    auto_remove: true,
                    ..Default::default()
                },
                FileManager {
                    path: nested_root.to_string_lossy().into_owned(),
                    include_subdirs: true,
                    auto_remove: false,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let actions = plan_scan(&db, &settings, "mkv").unwrap();

        assert!(actions.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    fn crosses_volumes(_: &Path, _: &Path) -> std::io::Result<()> {
        Err(std::io::ErrorKind::CrossesDevices.into())
    }

    fn register(db: &Db, path: &Path, data: &[u8]) -> i64 {
        fs::write(path, data).unwrap();
        let hashes = hash::hash_bytes(data, Ed2kVariant::Red);
        db.add_local_file(
            &path.to_string_lossy(),
            &hashes,
            modified_seconds(path).unwrap(),
        )
        .unwrap()
    }

    fn move_with(
        db: &Db,
        id: i64,
        old: &Path,
        new: &Path,
        rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<MoveOutcome> {
        let (cancel, pause) = (AtomicBool::new(false), AtomicBool::new(false));
        move_file_with(
            db,
            id,
            old,
            new,
            MoveControl {
                cancel: &cancel,
                pause: &pause,
                progress: &mut |_, _| {},
            },
            rename,
        )
    }

    fn recorded_path(db: &Db, id: i64) -> String {
        db.local_file_record(id).unwrap().unwrap().0
    }

    fn temp_copies(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".animatic-move-"))
            .collect()
    }

    #[test]
    fn same_volume_move_renames_and_updates_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::memory().unwrap();
        let old = dir.path().join("old name.mkv");
        let new = dir.path().join("sub").join("new name.mkv");
        let id = register(&db, &old, b"synthetic episode");

        let (cancel, pause) = (AtomicBool::new(false), AtomicBool::new(false));
        let outcome = move_file(
            &db,
            id,
            &old,
            &new,
            MoveControl {
                cancel: &cancel,
                pause: &pause,
                progress: &mut |_, _| {},
            },
        )
        .unwrap();

        assert_eq!(outcome.method, MoveMethod::Renamed);
        assert!(!old.exists());
        assert_eq!(fs::read(&new).unwrap(), b"synthetic episode");
        assert_eq!(recorded_path(&db, id), new.to_string_lossy());
    }

    #[test]
    fn cross_volume_move_copies_verifies_swaps_records_and_removes_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::memory().unwrap();
        let old = dir.path().join("old.mkv");
        let new = dir.path().join("target").join("new.mkv");
        let data: Vec<u8> = (0..(5 << 20)).map(|i| (i % 251) as u8).collect();
        let id = register(&db, &old, &data);
        let mtime_before = fs::metadata(&old).unwrap().modified().unwrap();

        let (cancel, pause) = (AtomicBool::new(false), AtomicBool::new(false));
        let mut reported = Vec::new();
        let outcome = move_file_with(
            &db,
            id,
            &old,
            &new,
            MoveControl {
                cancel: &cancel,
                pause: &pause,
                progress: &mut |done, total| reported.push((done, total)),
            },
            &crosses_volumes,
        )
        .unwrap();

        assert_eq!(outcome.method, MoveMethod::Copied);
        assert_eq!(outcome.source_left, None);
        assert!(!old.exists(), "the source goes only after the swap");
        assert_eq!(fs::read(&new).unwrap(), data);
        assert_eq!(recorded_path(&db, id), new.to_string_lossy());
        assert_eq!(
            fs::metadata(&new).unwrap().modified().unwrap(),
            mtime_before,
            "a new modification time would make the next scan rehash the file"
        );
        assert!(temp_copies(new.parent().unwrap()).is_empty());
        let total = 2 * data.len() as u64;
        assert!(reported.iter().all(|&(_, t)| t == total));
        assert_eq!(
            reported.last().unwrap().0,
            total,
            "copy and verification both reported"
        );
    }

    #[test]
    fn a_copy_that_does_not_match_the_stored_hash_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::memory().unwrap();
        let old = dir.path().join("old.mkv");
        let new = dir.path().join("new.mkv");
        // The library knows other bytes of the same size: the copy cannot verify.
        fs::write(&old, b"actual content").unwrap();
        let stored = hash::hash_bytes(b"recorded bytes", Ed2kVariant::Red);
        let id = db
            .add_local_file(&old.to_string_lossy(), &stored, 0)
            .unwrap();

        let error = move_with(&db, id, &old, &new, &crosses_volumes).unwrap_err();

        assert!(error.to_string().contains("does not match"), "{error}");
        assert_eq!(fs::read(&old).unwrap(), b"actual content");
        assert!(!new.exists());
        assert_eq!(recorded_path(&db, id), old.to_string_lossy());
        assert!(temp_copies(dir.path()).is_empty());
    }

    #[test]
    fn a_file_changed_since_hashing_or_without_a_stored_hash_is_not_copied() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::memory().unwrap();
        let old = dir.path().join("old.mkv");
        let new = dir.path().join("new.mkv");
        let id = register(&db, &old, b"hashed bytes");
        fs::write(&old, b"longer than the hashed bytes").unwrap();

        let error = move_with(&db, id, &old, &new, &crosses_volumes).unwrap_err();
        assert!(error.to_string().contains("changed since"), "{error}");

        let unhashed = dir.path().join("unhashed.mkv");
        fs::write(&unhashed, b"x").unwrap();
        let id = db
            .add_local_file(&unhashed.to_string_lossy(), &Hashes::default(), 0)
            .unwrap();
        let error = move_with(&db, id, &unhashed, &new, &crosses_volumes).unwrap_err();
        assert!(error.to_string().contains("no stored hash"), "{error}");

        assert!(old.exists() && unhashed.exists() && !new.exists());
        assert_eq!(recorded_path(&db, id), unhashed.to_string_lossy());
        assert!(temp_copies(dir.path()).is_empty());
    }

    #[test]
    fn a_copy_that_cannot_be_made_leaves_the_library_alone() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::memory().unwrap();
        let old = dir.path().join("old.mkv");
        let new = dir.path().join("target").join("new.mkv");
        let id = register(&db, &old, b"synthetic episode");
        // The target folder vanishes after the rename failed: no temporary copy can be made.
        let vanishing = |_: &Path, to: &Path| {
            fs::remove_dir(to.parent().unwrap()).unwrap();
            Err(std::io::ErrorKind::CrossesDevices.into())
        };

        assert!(move_with(&db, id, &old, &new, &vanishing).is_err());

        assert_eq!(fs::read(&old).unwrap(), b"synthetic episode");
        assert_eq!(recorded_path(&db, id), old.to_string_lossy());
        // The source is gone before the copy starts: the copy fails reading it.
        let new = dir.path().join("target2").join("new.mkv");
        let disappearing = |from: &Path, _: &Path| {
            fs::remove_file(from).unwrap();
            Err(std::io::ErrorKind::CrossesDevices.into())
        };
        assert!(move_with(&db, id, &old, &new, &disappearing).is_err());
        assert_eq!(recorded_path(&db, id), old.to_string_lossy());
        assert!(temp_copies(new.parent().unwrap()).is_empty());
    }

    #[test]
    fn an_existing_target_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::memory().unwrap();
        let old = dir.path().join("old.mkv");
        let new = dir.path().join("new.mkv");
        let id = register(&db, &old, b"synthetic episode");
        fs::write(&new, b"somebody else's file").unwrap();

        let error = move_with(&db, id, &old, &new, &crosses_volumes).unwrap_err();
        assert!(error.to_string().contains("already exists"), "{error}");
        let error = move_with(&db, id, &old, &new, &|a, b| fs::rename(a, b)).unwrap_err();
        assert!(error.to_string().contains("already exists"), "{error}");
        // A target that appears after the check, while the copy is being made.
        let appearing = |_: &Path, to: &Path| {
            fs::write(to, b"appeared meanwhile").unwrap();
            Err(std::io::ErrorKind::CrossesDevices.into())
        };
        let late = dir.path().join("late.mkv");
        let error = move_with(&db, id, &old, &late, &appearing).unwrap_err();
        assert!(error.to_string().contains("already exists"), "{error}");

        assert_eq!(fs::read(&new).unwrap(), b"somebody else's file");
        assert_eq!(fs::read(&late).unwrap(), b"appeared meanwhile");
        assert_eq!(fs::read(&old).unwrap(), b"synthetic episode");
        assert_eq!(recorded_path(&db, id), old.to_string_lossy());
        assert!(temp_copies(dir.path()).is_empty());
    }

    #[test]
    fn cancelling_while_copying_or_verifying_keeps_the_source_and_leaves_no_copy() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::memory().unwrap();
        let old = dir.path().join("old.mkv");
        let new = dir.path().join("new.mkv");
        let data = vec![7u8; 6 << 20];
        let id = register(&db, &old, &data);
        let size = data.len() as u64;

        // Once during the copy (first half of the progress range), once during verification.
        for cancel_after in [size / 2, size + size / 2] {
            let (cancel, pause) = (AtomicBool::new(false), AtomicBool::new(false));
            let result = move_file_with(
                &db,
                id,
                &old,
                &new,
                MoveControl {
                    cancel: &cancel,
                    pause: &pause,
                    progress: &mut |done, _| {
                        if done >= cancel_after {
                            cancel.store(true, Ordering::Relaxed);
                        }
                    },
                },
                &crosses_volumes,
            );

            assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
            assert_eq!(fs::read(&old).unwrap(), data);
            assert!(!new.exists());
            assert_eq!(recorded_path(&db, id), old.to_string_lossy());
            assert!(temp_copies(dir.path()).is_empty());
        }
    }

    #[test]
    fn a_move_for_a_stale_or_missing_library_entry_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::memory().unwrap();
        let old = dir.path().join("old.mkv");
        let other = dir.path().join("other.mkv");
        let id = register(&db, &old, b"synthetic episode");
        fs::write(&other, b"synthetic episode").unwrap();
        let new = dir.path().join("new.mkv");

        let error = move_with(&db, id, &other, &new, &|a, b| fs::rename(a, b)).unwrap_err();
        assert!(error.to_string().contains("now records"), "{error}");
        let error = move_with(&db, id + 100, &old, &new, &|a, b| fs::rename(a, b)).unwrap_err();
        assert!(
            error.to_string().contains("no longer in the library"),
            "{error}"
        );

        assert!(old.exists() && other.exists() && !new.exists());
    }
}
