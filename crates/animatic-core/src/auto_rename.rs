//! Opt-in automatic rename of identified files in managed folders (roadmap M6.3).
//!
//! A pass looks at every hashed library file whose path is inside a managed folder
//! (an explicit path match, never the built-in default rule) with [`AutoRename`] on or on dry
//! run, plans its rename with the current pattern through [`plan_batch`], and reports what to
//! do: the renames to carry out for folders set to [`AutoRename::On`] (the application hands
//! them to the verified rename job, [`crate::file_manager::move_file`]) and the log notes for
//! everything else. Nothing here changes the file system.
//!
//! * A file is only considered once AniDB identified it **and** its anime and episode records
//!   are cached; until then it is skipped without a note, because the identification reply
//!   that completes it triggers another pass.
//! * Files whose plan is blocked (pattern errors or warnings, a target that already exists, two
//!   files that would get the same name, missing metadata) are skipped and logged, never
//!   renamed and never overwritten.
//! * Notes are given **once per file and state** ([`AutoRenameMemory`]): repeating a pass
//!   without a change reports and renames nothing again, so scans cannot spam the log or retry
//!   a failed move forever.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::db::Db;
use crate::error::Result;
use crate::library::LocalFile;
use crate::rename::plan::{Candidate, check_pattern, plan_batch};
use crate::settings::{AutoRename, FileManagerSettings};

/// Most notes a single pass returns; the rest are summarized in one more note.
pub const MAX_NOTES: usize = 20;

/// What a pass did last for each file, so an unchanged state is neither reported nor retried.
#[derive(Debug, Default)]
pub struct AutoRenameMemory {
    seen: HashMap<i64, String>,
    pattern_problem: Option<String>,
}

impl AutoRenameMemory {
    /// Forget a file, so the next pass plans and reports it again (a cancelled rename).
    pub fn forget(&mut self, local_id: i64) {
        self.seen.remove(&local_id);
    }
}

/// One rename the application must carry out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRename {
    pub local_id: i64,
    pub old: PathBuf,
    pub new: PathBuf,
}

/// The outcome of [`plan_pass`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AutoRenamePass {
    /// Renames of files in folders set to [`AutoRename::On`].
    pub renames: Vec<PlannedRename>,
    /// Log lines: dry-run plans and skipped files, each new since the previous pass.
    pub notes: Vec<String>,
}

/// Identified files of `files` inside a managed folder, with the library row and the index of
/// that folder in `settings.managers`; `wanted` filters the folders.
fn identified_in_folders<'a>(
    files: &'a [LocalFile],
    settings: &FileManagerSettings,
    wanted: impl Fn(usize) -> bool,
) -> Vec<(usize, &'a LocalFile, Candidate)> {
    files
        .iter()
        .filter_map(|f| {
            let fid = f.fid?;
            let path = PathBuf::from(&f.path);
            let index = settings.matching_manager_index(&path)?;
            wanted(index).then(|| {
                let candidate = Candidate {
                    local_id: f.id,
                    path,
                    fid: Some(fid),
                    base_path: settings.managers[index].base_path.clone(),
                };
                (index, f, candidate)
            })
        })
        .collect()
}

/// Identified files of `files` inside the managed folder number `folder` of
/// `settings.managers`, as batch rename candidates (the dry-run preview shown before a folder is
/// turned on). No database is read, so a file whose anime or episode record is missing is still
/// offered and shows up as blocked in its plan.
pub fn folder_candidates(
    files: &[LocalFile],
    settings: &FileManagerSettings,
    folder: usize,
) -> Vec<Candidate> {
    identified_in_folders(files, settings, |index| index == folder)
        .into_iter()
        .map(|(_, _, candidate)| candidate)
        .collect()
}

/// Plan the automatic renames of the whole library with `pattern`.
///
/// Returns nothing when every folder is off. An unusable pattern (it does not compile) blocks
/// the pass with one note. Otherwise each eligible file (see the module documentation) is
/// planned; see [`AutoRenamePass`] for the result. `memory` makes the result incremental: a
/// file whose outcome equals the previous pass's is left out.
pub fn plan_pass(
    db: &Db,
    settings: &FileManagerSettings,
    pattern: &str,
    memory: &mut AutoRenameMemory,
) -> Result<AutoRenamePass> {
    let mut pass = AutoRenamePass::default();
    if settings.managers.iter().all(|m| m.auto_rename.is_off()) {
        memory.seen.clear();
        memory.pattern_problem = None;
        return Ok(pass);
    }
    if let Err(problem) = check_pattern(pattern) {
        if memory.pattern_problem.as_deref() != Some(problem.as_str()) {
            pass.notes.push(format!(
                "Automatic rename is paused: the renaming pattern cannot be used ({problem})"
            ));
            memory.pattern_problem = Some(problem);
        }
        return Ok(pass);
    }
    memory.pattern_problem = None;

    let files = db.local_files()?;
    let mut present = HashSet::new();
    let mut chosen: Vec<(AutoRename, Candidate)> = Vec::new();
    for (index, file, candidate) in identified_in_folders(&files, settings, |index| {
        !settings.managers[index].auto_rename.is_off()
    }) {
        let cached = file
            .aid
            .is_some_and(|aid| aid > 0 && db.anime(aid).ok().flatten().is_some())
            && file
                .eid
                .is_some_and(|eid| eid > 0 && db.episode(eid).ok().flatten().is_some());
        if !cached || !candidate.path.try_exists().unwrap_or(false) {
            continue;
        }
        present.insert(candidate.local_id);
        chosen.push((settings.managers[index].auto_rename, candidate));
    }
    memory.seen.retain(|id, _| present.contains(id));

    let candidates: Vec<Candidate> = chosen.iter().map(|(_, c)| c.clone()).collect();
    for ((mode, _), row) in chosen.iter().zip(plan_batch(db, pattern, &candidates)) {
        let old = row.old.display();
        let (key, note, rename) = match (&row.plan.target, row.plan.can_apply()) {
            (Some(new), true) => match mode {
                AutoRename::On => (
                    format!("on:{old}->{}", new.display()),
                    None,
                    Some(new.clone()),
                ),
                _ => (
                    format!("dry:{old}->{}", new.display()),
                    Some(format!(
                        "Automatic rename (dry run): {old} would be renamed to {}",
                        new.display()
                    )),
                    None,
                ),
            },
            _ if row.plan.already_named() => ("named".to_string(), None, None),
            _ => {
                let reasons = row
                    .plan
                    .errors
                    .iter()
                    .chain(&row.plan.warnings)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ");
                (
                    format!("skip:{}:{reasons}", mode.as_str()),
                    Some(format!("Automatic rename skipped {old}: {reasons}")),
                    None,
                )
            }
        };
        if memory.seen.get(&row.local_id) == Some(&key) {
            continue;
        }
        memory.seen.insert(row.local_id, key);
        pass.notes.extend(note);
        if let Some(new) = rename {
            pass.renames.push(PlannedRename {
                local_id: row.local_id,
                old: row.old.clone(),
                new,
            });
        }
    }
    if pass.notes.len() > MAX_NOTES {
        let more = pass.notes.len() - MAX_NOTES;
        pass.notes.truncate(MAX_NOTES);
        pass.notes
            .push(format!("Automatic rename: {more} more note(s) not shown"));
    }
    Ok(pass)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Hashes;
    use crate::settings::FileManager;
    use animatic_anidb::commands::{EpNo, FileInfo};
    use std::path::Path;

    const PATTERN: &str = "A.Name + ' ' + E.EpisodeNo + '.mkv'";

    struct Library {
        dir: tempfile::TempDir,
        db: Db,
    }

    impl Library {
        fn new() -> Library {
            Library {
                dir: tempfile::tempdir().unwrap(),
                db: Db::memory().unwrap(),
            }
        }

        fn folder(&self, sub: &str) -> PathBuf {
            let folder = self.dir.path().join(sub);
            std::fs::create_dir_all(&folder).unwrap();
            folder
        }

        /// A hashed file on disk; identified (anime and episode cached) when `episode` is set.
        fn file(&self, folder: &Path, id: i64, episode: Option<i64>) -> PathBuf {
            let path = folder.join(format!("source {id}.mkv"));
            std::fs::write(&path, format!("synthetic {id}")).unwrap();
            let ed2k = format!("{id:032x}");
            self.db
                .add_local_file(
                    &path.to_string_lossy(),
                    &Hashes {
                        size: 1000 + id as u64,
                        ed2k: ed2k.clone(),
                        crc32: String::new(),
                        md5: String::new(),
                        sha1: String::new(),
                    },
                    0,
                )
                .unwrap();
            if let Some(episode) = episode {
                self.db
                    .store_file_info(&FileInfo {
                        fid: id,
                        aid: 2,
                        eid: 20 + episode,
                        size: 1000 + id,
                        ed2k: Some(ed2k),
                        anime_romaji: Some("Synthetic Show".into()),
                        epno: EpNo {
                            kind: 1,
                            number: episode as u32,
                        },
                        ..FileInfo::default()
                    })
                    .unwrap();
            }
            path
        }
    }

    fn settings(folder: &Path, mode: AutoRename) -> FileManagerSettings {
        let mut settings = FileManagerSettings::default();
        settings.managers.push(FileManager {
            name: "Synthetic".into(),
            path: folder.to_string_lossy().into_owned(),
            auto_rename: mode,
            ..FileManager::default()
        });
        settings
    }

    #[test]
    fn off_plans_nothing() {
        let lib = Library::new();
        let folder = lib.folder("watched");
        lib.file(&folder, 1, Some(1));
        let mut memory = AutoRenameMemory::default();

        let pass = plan_pass(
            &lib.db,
            &settings(&folder, AutoRename::Off),
            PATTERN,
            &mut memory,
        )
        .unwrap();

        assert_eq!(pass, AutoRenamePass::default());
    }

    #[test]
    fn dry_run_notes_the_plan_and_renames_nothing() {
        let lib = Library::new();
        let folder = lib.folder("watched");
        lib.file(&folder, 1, Some(1));
        let mut memory = AutoRenameMemory::default();
        let settings = settings(&folder, AutoRename::DryRun);

        let pass = plan_pass(&lib.db, &settings, PATTERN, &mut memory).unwrap();

        assert!(pass.renames.is_empty());
        assert_eq!(pass.notes.len(), 1, "{:?}", pass.notes);
        assert!(pass.notes[0].contains("dry run"), "{:?}", pass.notes);
        assert!(pass.notes[0].contains("Synthetic Show 1.mkv"));
        let again = plan_pass(&lib.db, &settings, PATTERN, &mut memory).unwrap();
        assert_eq!(
            again,
            AutoRenamePass::default(),
            "an unchanged state is reported once"
        );
    }

    #[test]
    fn on_plans_the_rename_once() {
        let lib = Library::new();
        let folder = lib.folder("watched");
        let old = lib.file(&folder, 1, Some(1));
        let mut memory = AutoRenameMemory::default();
        let settings = settings(&folder, AutoRename::On);

        let pass = plan_pass(&lib.db, &settings, PATTERN, &mut memory).unwrap();

        assert_eq!(
            pass.renames,
            [PlannedRename {
                local_id: 1,
                old,
                new: folder.join("Synthetic Show 1.mkv"),
            }]
        );
        assert!(pass.notes.is_empty());
        let again = plan_pass(&lib.db, &settings, PATTERN, &mut memory).unwrap();
        assert!(
            again.renames.is_empty(),
            "a rename that was handed over (or failed) is not planned again unchanged"
        );
        memory.forget(1);
        let retried = plan_pass(&lib.db, &settings, PATTERN, &mut memory).unwrap();
        assert_eq!(retried.renames.len(), 1, "a cancelled rename is retried");
    }

    #[test]
    fn a_collision_is_skipped_and_logged_once() {
        let lib = Library::new();
        let folder = lib.folder("watched");
        lib.file(&folder, 1, Some(1));
        std::fs::write(folder.join("Synthetic Show 1.mkv"), b"another file").unwrap();
        let mut memory = AutoRenameMemory::default();
        let settings = settings(&folder, AutoRename::On);

        let pass = plan_pass(&lib.db, &settings, PATTERN, &mut memory).unwrap();

        assert!(pass.renames.is_empty());
        assert_eq!(pass.notes.len(), 1, "{:?}", pass.notes);
        assert!(pass.notes[0].contains("skipped"), "{:?}", pass.notes);
        assert!(pass.notes[0].contains("already exists"), "{:?}", pass.notes);
        let again = plan_pass(&lib.db, &settings, PATTERN, &mut memory).unwrap();
        assert_eq!(again, AutoRenamePass::default());
    }

    #[test]
    fn two_files_with_one_target_rename_only_the_first() {
        let lib = Library::new();
        let folder = lib.folder("watched");
        lib.file(&folder, 1, Some(1));
        lib.file(&folder, 2, Some(1));
        let mut memory = AutoRenameMemory::default();

        let pass = plan_pass(
            &lib.db,
            &settings(&folder, AutoRename::On),
            PATTERN,
            &mut memory,
        )
        .unwrap();

        assert_eq!(pass.renames.len(), 1);
        assert_eq!(pass.renames[0].local_id, 1);
        assert!(pass.notes[0].contains("same name"), "{:?}", pass.notes);
    }

    #[test]
    fn unidentified_files_and_missing_files_are_left_alone() {
        let lib = Library::new();
        let folder = lib.folder("watched");
        lib.file(&folder, 1, None);
        let gone = lib.file(&folder, 2, Some(2));
        std::fs::remove_file(gone).unwrap();
        let mut memory = AutoRenameMemory::default();

        let pass = plan_pass(
            &lib.db,
            &settings(&folder, AutoRename::On),
            PATTERN,
            &mut memory,
        )
        .unwrap();

        assert_eq!(pass, AutoRenamePass::default());
    }

    #[test]
    fn a_blocked_plan_is_skipped_and_logged_not_renamed() {
        let lib = Library::new();
        let folder = lib.folder("watched");
        lib.file(&folder, 1, Some(1));
        let mut memory = AutoRenameMemory::default();
        // Compiles, but evaluates to a truth value instead of a name.
        let pass = plan_pass(
            &lib.db,
            &settings(&folder, AutoRename::On),
            "if(1=2, 'x.mkv')",
            &mut memory,
        )
        .unwrap();

        assert!(pass.renames.is_empty());
        assert_eq!(pass.notes.len(), 1, "{:?}", pass.notes);
        assert!(pass.notes[0].contains("skipped"));
    }

    #[test]
    fn a_pattern_that_does_not_compile_pauses_the_pass_with_one_note() {
        let lib = Library::new();
        let folder = lib.folder("watched");
        lib.file(&folder, 1, Some(1));
        let mut memory = AutoRenameMemory::default();
        let settings = settings(&folder, AutoRename::On);

        let pass = plan_pass(&lib.db, &settings, "set('a', ", &mut memory).unwrap();

        assert!(pass.renames.is_empty());
        assert_eq!(pass.notes.len(), 1);
        assert!(pass.notes[0].contains("paused"), "{:?}", pass.notes);
        let again = plan_pass(&lib.db, &settings, "set('a', ", &mut memory).unwrap();
        assert_eq!(again, AutoRenamePass::default());
        let fixed = plan_pass(&lib.db, &settings, PATTERN, &mut memory).unwrap();
        assert_eq!(fixed.renames.len(), 1, "a fixed pattern resumes the pass");
    }

    #[test]
    fn only_files_inside_a_matching_managed_folder_are_renamed() {
        let lib = Library::new();
        let watched = lib.folder("watched");
        let elsewhere = lib.folder("elsewhere");
        let off = lib.folder("watched/off");
        lib.file(&watched, 1, Some(1));
        lib.file(&elsewhere, 2, Some(2));
        lib.file(&off, 3, Some(3));
        let mut settings = settings(&watched, AutoRename::On);
        settings.managers.push(FileManager {
            name: "Nested, off".into(),
            path: off.to_string_lossy().into_owned(),
            auto_rename: AutoRename::Off,
            ..FileManager::default()
        });
        let mut memory = AutoRenameMemory::default();

        let pass = plan_pass(&lib.db, &settings, PATTERN, &mut memory).unwrap();

        let renamed: Vec<i64> = pass.renames.iter().map(|r| r.local_id).collect();
        assert_eq!(
            renamed,
            [1],
            "not the file outside every folder (the built-in default rule never renames), \
             and not the one whose nearest folder rule is off"
        );
    }

    #[test]
    fn a_legacy_boolean_loads_as_dry_run_never_on() {
        let manager = |json: &str| -> FileManager { serde_json::from_str(json).unwrap() };

        assert_eq!(manager("{}").auto_rename, AutoRename::Off);
        assert_eq!(
            manager(r#"{"auto_rename":false}"#).auto_rename,
            AutoRename::Off
        );
        assert_eq!(
            manager(r#"{"auto_rename":true}"#).auto_rename,
            AutoRename::DryRun
        );
        assert_eq!(
            manager(r#"{"auto_rename":"on"}"#).auto_rename,
            AutoRename::On
        );
        assert!(serde_json::from_str::<FileManager>(r#"{"auto_rename":"sometimes"}"#).is_err());
        let saved = serde_json::to_string(&FileManager {
            auto_rename: AutoRename::On,
            ..FileManager::default()
        })
        .unwrap();
        assert!(saved.contains(r#""auto_rename":"on""#), "{saved}");
    }
}
