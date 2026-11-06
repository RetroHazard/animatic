//! Crash-safe file replacement for user-visible exports.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Write `bytes` to `path` so that readers see either the previous file or the complete new one.
///
/// See [`write_atomic_with`] for the procedure and failure behavior.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, |file| file.write_all(bytes))
}

/// Let `write` fill a temporary file in the target's directory, flush it to disk and rename it
/// over `path`.
///
/// The temporary file lives next to the target (a rename never crosses volumes) under a name
/// unique to this call, so concurrent exports cannot share it. If `write`, the flush or the
/// rename fails, the temporary file is removed and an existing `path` is left exactly as it was.
/// The parent directory must already exist; this function does not create it.
pub fn write_atomic_with(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let tmp = temp_path(path)?;
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut file| {
            write(&mut file)?;
            file.flush()?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&tmp, path));
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

fn temp_path(path: &Path) -> io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "export path has no file name")
    })?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    Ok(path.with_file_name(tmp_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "animatic-atomic-{tag}-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn replaces_the_target_and_leaves_no_temporary_file() {
        let dir = temp_dir("replace");
        let target = dir.join("export.txt");
        write_atomic(&target, b"first").unwrap();
        write_atomic(&target, b"second").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"second");
        assert_eq!(entries(&dir), ["export.txt"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_write_keeps_the_previous_file_and_removes_the_temporary_one() {
        let dir = temp_dir("failed-write");
        let target = dir.join("export.txt");
        fs::write(&target, b"previous").unwrap();
        let err = write_atomic_with(&target, |file| {
            file.write_all(b"partial new content")?;
            Err(io::Error::other("disk full"))
        })
        .unwrap_err();
        assert_eq!(err.to_string(), "disk full");
        assert_eq!(fs::read(&target).unwrap(), b"previous");
        assert_eq!(entries(&dir), ["export.txt"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_rename_keeps_the_target_and_removes_the_temporary_file() {
        let dir = temp_dir("failed-rename");
        // A directory cannot be replaced by a file on Windows or Linux, so the rename fails
        // after the new content was fully written.
        let target = dir.join("export.txt");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep.txt"), b"previous").unwrap();
        assert!(write_atomic(&target, b"new").is_err());
        assert!(target.is_dir());
        assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"previous");
        assert_eq!(entries(&dir), ["export.txt"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_directory_is_an_error_not_a_created_path() {
        let dir = temp_dir("missing-dir");
        let target = dir.join("absent").join("export.txt");
        assert!(write_atomic(&target, b"new").is_err());
        assert!(!dir.join("absent").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_path_without_a_file_name_is_rejected() {
        let err = write_atomic(Path::new(""), b"x").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
