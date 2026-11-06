//! Refreshing what AniDB already told us: **Tools > Full update…** and the *Update known file
//! information* option.
//!
//! Both only add commands to the persistent queue; nothing is sent until the queue runs, and
//! the executor applies the AniDB request limits again (title dump once a day, anime once a
//! day, a known file seven days after the last attempt; `animatic_core::limits`).

use animatic_core::limits::{KNOWN_FILE_MIN_AGE, RequestKind};

use super::*;
use crate::worker::FullUpdateReport;

/// Where **Full update…** is: waiting for the worker's preview, or showing it for confirmation.
#[derive(Debug, Default)]
pub(super) enum FullUpdateStage {
    #[default]
    Closed,
    Computing,
    Confirm(FullUpdateReport),
}

/// What to queue for a file that was just hashed.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum AfterHash {
    Nothing,
    /// Look the file up by hash (and add it to MyList when the folder policy says so).
    Identify,
    /// Refresh the record of an identified file.
    Refresh(i64),
    /// An identified file whose last request is too recent; refreshable after this time.
    TooRecent {
        fid: i64,
        until: i64,
    },
}

/// Decide what hashing a file leads to.
///
/// An unidentified file keeps the identify-after-hashing path. An identified one (the library
/// has its AniDB record) is not looked up again by hash; instead, with `update_known`, a
/// `FetchFile` refreshes its record unless one was requested within `KNOWN_FILE_MIN_AGE`.
/// Adding to MyList (`auto_add`) still goes through `Identify`, which refreshes the record too.
pub(super) fn after_hash(
    db: &Db,
    identify_after_hashing: bool,
    update_known: bool,
    auto_add: bool,
    size: u64,
    ed2k: &str,
    now: i64,
) -> animatic_core::Result<AfterHash> {
    let Some(known) = db.file_by_hash(ed2k, size as i64)? else {
        return Ok(if identify_after_hashing {
            AfterHash::Identify
        } else {
            AfterHash::Nothing
        });
    };
    if identify_after_hashing && auto_add {
        return Ok(AfterHash::Identify);
    }
    if !update_known {
        return Ok(AfterHash::Nothing);
    }
    Ok(
        match db.request_next_allowed(RequestKind::File, known.fid, now)? {
            Some(until) => AfterHash::TooRecent {
                fid: known.fid,
                until,
            },
            None => AfterHash::Refresh(known.fid),
        },
    )
}

impl AnimaticApp {
    /// Queue what the settings ask for after `path` was hashed.
    pub(super) fn queue_after_hash(&mut self, path: &Path, size: u64, ed2k: String) {
        let manager = self.settings.file_managers.manager_for(path);
        let auto_add = manager.mylist_auto_add;
        let decision = after_hash(
            &self.db,
            self.settings.hashing.identify_after_hashing,
            self.settings.anidb.update_file_information,
            auto_add,
            size,
            &ed2k,
            format::now(),
        );
        let decision = decision.unwrap_or_else(|e| {
            log::warn!("cannot tell whether {} is identified: {e}", path.display());
            if self.settings.hashing.identify_after_hashing {
                AfterHash::Identify
            } else {
                AfterHash::Nothing
            }
        });
        match decision {
            AfterHash::Nothing => {}
            AfterHash::Identify => {
                let add = auto_add.then(|| animatic_core::sync::manager_mylist_edit(manager));
                self.queue_command(animatic_core::sync::Command::Identify { size, ed2k, add });
            }
            AfterHash::Refresh(fid) => {
                self.queue_command(animatic_core::sync::Command::FetchFile { fid });
            }
            AfterHash::TooRecent { fid, until } => {
                self.log(animatic_core::limits::file_wait_message(fid, until));
            }
        }
    }

    /// **Tools > Full update…**: ask the worker what would be queued.
    pub(super) fn start_full_update_preview(&mut self) {
        self.full_update = FullUpdateStage::Computing;
        self.worker.submit(Job::PreviewFullUpdate);
    }

    pub(super) fn full_update_window(&mut self, ctx: &egui::Context) {
        let FullUpdateStage::Confirm(report) = &self.full_update else {
            return;
        };
        let report = report.clone();
        let mut go = None;
        let response = egui::Modal::new(egui::Id::unique("full_update")).show(ctx, |ui| {
            ui.heading("Full update");
            ui.label(format!(
                "Queues {} commands: {} title dump, {} file refreshes and {} anime refreshes for \
                 the hashed files in the library.",
                report.queued, report.new_titles_dump, report.new_files, report.new_anime
            ));
            if let Some(left_out) = report.left_out() {
                ui.label(format!(
                    "Left out because AniDB's limits do not allow them yet: {left_out}."
                ));
            }
            ui.label(format!(
                "Nothing is sent to AniDB until you run the command queue. A file is refreshed \
                 at most every {} days, an anime and the title dump once a day.",
                KNOWN_FILE_MIN_AGE.as_secs() / 86_400
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(report.queued > 0, egui::Button::new("Queue"))
                    .clicked()
                {
                    go = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    go = Some(false);
                }
            });
        });
        if response.should_close() && go.is_none() {
            go = Some(false);
        }
        let Some(go) = go else { return };
        self.full_update = FullUpdateStage::Closed;
        if go {
            self.worker.submit(Job::QueueFullUpdate);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use animatic_core::sync::Command;

    use super::*;

    struct TempDb(PathBuf);

    impl TempDb {
        fn new() -> TempDb {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "animatic-refresh-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TempDb(dir)
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A library that knows file 10 (hash `aa`, 1 byte) from AniDB.
    fn library(dir: &TempDb) -> Db {
        let db = Db::open(&dir.0.join("library.db")).unwrap();
        db.conn()
            .execute_batch("INSERT INTO file(fid, aid, ed2k, size) VALUES (10,100,'aa',1);")
            .unwrap();
        db
    }

    #[test]
    fn a_known_file_hashed_again_is_refreshed_only_when_the_option_is_on() {
        let dir = TempDb::new();
        let db = library(&dir);
        let now = format::now();
        // Option off: nothing, even with identify-after-hashing on.
        assert_eq!(
            after_hash(&db, true, false, false, 1, "aa", now).unwrap(),
            AfterHash::Nothing
        );
        // Option on: one refresh of the known file.
        assert_eq!(
            after_hash(&db, true, true, false, 1, "aa", now).unwrap(),
            AfterHash::Refresh(10)
        );
        // An unidentified file keeps the identify path, whatever the option says.
        assert_eq!(
            after_hash(&db, true, true, false, 1, "bb", now).unwrap(),
            AfterHash::Identify
        );
        assert_eq!(
            after_hash(&db, false, true, false, 1, "bb", now).unwrap(),
            AfterHash::Nothing
        );
    }

    #[test]
    fn no_refresh_inside_the_known_file_window() {
        let dir = TempDb::new();
        let db = library(&dir);
        let now = format::now();
        let week = KNOWN_FILE_MIN_AGE.as_secs() as i64;
        db.record_request_attempt(RequestKind::File, 10, now - week + 3600)
            .unwrap();
        assert_eq!(
            after_hash(&db, false, true, false, 1, "aa", now).unwrap(),
            AfterHash::TooRecent {
                fid: 10,
                until: now + 3600
            }
        );
        db.record_request_attempt(RequestKind::File, 10, now - week - 1)
            .unwrap();
        assert_eq!(
            after_hash(&db, false, true, false, 1, "aa", now).unwrap(),
            AfterHash::Refresh(10)
        );
        // The refresh queued for it is a plain FetchFile.
        db.queue_push(&Command::FetchFile { fid: 10 }).unwrap();
        assert_eq!(db.queue_len().unwrap(), 1);
    }
}
