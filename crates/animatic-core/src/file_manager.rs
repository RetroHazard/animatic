//! File discovery and automatic file-manager operations.
//!
//! Discovery is side-effect free. The desktop worker chooses which planned actions to execute,
//! keeping hashing, network access and filesystem mutations off the UI thread.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

/// Size and modification time of a file, the two things that change while it is being written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

fn stamp(path: &Path) -> std::io::Result<Stamp> {
    let metadata = fs::metadata(path)?;
    Ok(Stamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

/// The outcome of [`settle_scan`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Settled {
    /// Actions to run, in their planned order.
    pub ready: Vec<ScanAction>,
    /// Files that are still being written (or were modified too recently); a later scan
    /// reconsiders them.
    pub deferred: Vec<PathBuf>,
}

enum Verdict {
    Ready(ScanAction),
    Deferred(PathBuf),
    Watching(ScanAction, PathBuf, Stamp),
}

/// Hold back files that may still be being written. A file is hashed only when its modification
/// time is at least `delay` in the past at `now` and its size and modification time did not change
/// during one further `delay` observed by this call: the first rule needs no earlier scan, the
/// second catches a copy that keeps the source's old modification time while it grows. With a
/// zero `delay` nothing is held back. `wait` sleeps for the given time and returns `false` when
/// the scan was cancelled, which defers every file still under observation.
///
/// Removals and files whose metadata cannot be read are passed on untouched: the first needs no
/// settling and the second reports its own error when hashed.
pub fn settle_scan(
    actions: Vec<ScanAction>,
    delay: Duration,
    now: SystemTime,
    wait: &mut dyn FnMut(Duration) -> bool,
) -> Settled {
    if delay.is_zero() {
        return Settled {
            ready: actions,
            deferred: Vec::new(),
        };
    }
    let mut verdicts = Vec::with_capacity(actions.len());
    let mut watching = false;
    for action in actions {
        let path = match &action {
            ScanAction::Hash { path, .. } | ScanAction::Rehash { path, .. } => path.clone(),
            ScanAction::RemoveMissing { .. } => {
                verdicts.push(Verdict::Ready(action));
                continue;
            }
        };
        match stamp(&path) {
            Err(_) => verdicts.push(Verdict::Ready(action)),
            // A modification time in the future (clock skew) has no age; observation decides.
            Ok(first) => match first.modified.and_then(|m| now.duration_since(m).ok()) {
                Some(age) if age < delay => verdicts.push(Verdict::Deferred(path)),
                _ => {
                    watching = true;
                    verdicts.push(Verdict::Watching(action, path, first));
                }
            },
        }
    }
    let observed = !watching || wait(delay);
    let mut settled = Settled::default();
    for verdict in verdicts {
        match verdict {
            Verdict::Ready(action) => settled.ready.push(action),
            Verdict::Deferred(path) => settled.deferred.push(path),
            Verdict::Watching(action, path, first) => {
                if observed && stamp(&path).is_ok_and(|second| second == first) {
                    settled.ready.push(action);
                } else {
                    settled.deferred.push(path);
                }
            }
        }
    }
    settled
}

/// Sleep for `total`, waking early when `cancel` is set. Returns `false` when cancelled.
pub fn interruptible_sleep(total: Duration, cancel: &AtomicBool) -> bool {
    const SLICE: Duration = Duration::from_millis(50);
    let end = Instant::now() + total;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(left.min(SLICE));
    }
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

    const DELAY: Duration = Duration::from_secs(5);

    fn hash_action(path: &Path) -> ScanAction {
        ScanAction::Hash {
            path: path.to_path_buf(),
            manager: 0,
        }
    }

    /// A file written `age` before `now` (its modification time is set explicitly).
    fn aged_file(root: &Path, name: &str, now: SystemTime, age: Duration) -> PathBuf {
        let path = root.join(name);
        fs::write(&path, b"synthetic media").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(now - age)
            .unwrap();
        path
    }

    #[test]
    fn settle_defers_files_modified_within_the_delay_and_hashes_settled_ones() {
        let root = test_root("settle-age");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let now = SystemTime::now();
        let young = aged_file(&root, "young.mkv", now, Duration::from_secs(1));
        let old = aged_file(&root, "old.mkv", now, Duration::from_secs(3600));
        let mut waited = Vec::new();

        let settled = settle_scan(
            vec![hash_action(&young), hash_action(&old)],
            DELAY,
            now,
            &mut |d| {
                waited.push(d);
                true
            },
        );

        assert_eq!(settled.ready, vec![hash_action(&old)]);
        assert_eq!(settled.deferred, vec![young]);
        assert_eq!(
            waited,
            vec![DELAY],
            "one observation window for the whole scan"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn settle_defers_a_file_that_grows_while_observed_even_with_an_old_modification_time() {
        let root = test_root("settle-growing");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let now = SystemTime::now();
        let growing = aged_file(&root, "growing.mkv", now, Duration::from_secs(3600));
        let still = aged_file(&root, "still.mkv", now, Duration::from_secs(3600));
        let target = growing.clone();

        let settled = settle_scan(
            vec![hash_action(&growing), hash_action(&still)],
            DELAY,
            now,
            &mut |_| {
                // A copy that keeps the source's old modification time but adds data.
                let mtime = fs::metadata(&target).unwrap().modified().unwrap();
                let mut file = fs::OpenOptions::new().append(true).open(&target).unwrap();
                std::io::Write::write_all(&mut file, b" more data").unwrap();
                file.set_modified(mtime).unwrap();
                true
            },
        );

        assert_eq!(settled.ready, vec![hash_action(&still)]);
        assert_eq!(settled.deferred, vec![growing]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn settle_defers_a_file_whose_modification_time_changes_while_observed() {
        let root = test_root("settle-touched");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let now = SystemTime::now();
        let touched = aged_file(&root, "touched.mkv", now, Duration::from_secs(3600));
        let target = touched.clone();

        let settled = settle_scan(vec![hash_action(&touched)], DELAY, now, &mut |_| {
            fs::File::options()
                .write(true)
                .open(&target)
                .unwrap()
                .set_modified(now - Duration::from_secs(60))
                .unwrap();
            true
        });

        assert!(settled.ready.is_empty());
        assert_eq!(settled.deferred, vec![touched]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn settle_with_zero_delay_hashes_everything_without_waiting() {
        let root = test_root("settle-zero");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let now = SystemTime::now();
        let fresh = aged_file(&root, "fresh.mkv", now, Duration::ZERO);

        let settled = settle_scan(vec![hash_action(&fresh)], Duration::ZERO, now, &mut |_| {
            panic!("no waiting without a delay")
        });

        assert_eq!(settled.ready, vec![hash_action(&fresh)]);
        assert!(settled.deferred.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn settle_defers_observed_files_when_the_scan_is_cancelled_but_keeps_removals() {
        let root = test_root("settle-cancel");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let now = SystemTime::now();
        let old = aged_file(&root, "old.mkv", now, Duration::from_secs(3600));
        let removal = ScanAction::RemoveMissing {
            local_id: 7,
            path: root.join("gone.mkv"),
        };

        let settled = settle_scan(
            vec![hash_action(&old), removal.clone()],
            DELAY,
            now,
            &mut |_| false,
        );

        assert_eq!(settled.ready, vec![removal]);
        assert_eq!(settled.deferred, vec![old]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interruptible_sleep_waits_the_time_and_stops_when_cancelled() {
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        assert!(interruptible_sleep(Duration::from_millis(120), &cancel));
        assert!(started.elapsed() >= Duration::from_millis(120));

        cancel.store(true, Ordering::Relaxed);
        let started = Instant::now();
        assert!(!interruptible_sleep(Duration::from_secs(30), &cancel));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
