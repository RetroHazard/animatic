//! The `media_info` table and the batch that fills it.
//!
//! One row per local file that was probed. `hash_id` records which content the figures belong
//! to: a file that is hashed again to different content keeps its `local_file` row (and id), so
//! its old figures are ignored by every read here until they are probed again. A row whose
//! value columns are all `NULL` records only that the file was probed without a usable result.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use rusqlite::{OptionalExtension, params};

use super::{MediaError, MediaInfo};
use crate::db::Db;
use crate::error::Result;

/// A local file to probe: its library id and where it is on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaTarget {
    pub local_id: i64,
    pub path: PathBuf,
}

fn from_row(r: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<MediaInfo> {
    Ok(MediaInfo {
        length: r.get(first)?,
        fps: r.get(first + 1)?,
        video_width: r.get(first + 2)?,
        video_height: r.get(first + 3)?,
        video_codec: r.get(first + 4)?,
        video_bitrate: r.get(first + 5)?,
        audio_codec: r.get(first + 6)?,
        audio_sample_rate: r.get(first + 7)?,
        audio_bitrate: r.get(first + 8)?,
    })
}

const COLUMNS: &str = "m.length, m.fps, m.video_width, m.video_height, m.video_codec, \
     m.video_bitrate, m.audio_codec, m.audio_samplerate, m.audio_bitrate";

impl Db {
    /// Content id (`local_hash.id`) of a local file, or `None` when the file is not in the
    /// library any more.
    pub fn local_file_hash_id(&self, local_id: i64) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT hash_id FROM local_file WHERE id = ?1",
                [local_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Save the figures of a probe of content `hash_id` of local file `local_id`, replacing
    /// earlier figures. Nothing is saved when the file now has other content (it was hashed
    /// again meanwhile) or is gone.
    pub fn set_media_info(&self, local_id: i64, hash_id: i64, info: &MediaInfo) -> Result<()> {
        self.conn().execute(
            "INSERT INTO media_info(local_file_id, hash_id, length, fps, video_width, video_height,
                 video_codec, video_bitrate, audio_codec, audio_samplerate, audio_bitrate, probed)
             SELECT l.id, l.hash_id, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12
             FROM local_file l WHERE l.id = ?1 AND l.hash_id = ?2
             ON CONFLICT(local_file_id) DO UPDATE SET
                 hash_id = excluded.hash_id, length = excluded.length, fps = excluded.fps,
                 video_width = excluded.video_width, video_height = excluded.video_height,
                 video_codec = excluded.video_codec, video_bitrate = excluded.video_bitrate,
                 audio_codec = excluded.audio_codec, audio_samplerate = excluded.audio_samplerate,
                 audio_bitrate = excluded.audio_bitrate, probed = excluded.probed",
            params![
                local_id,
                hash_id,
                info.length,
                info.fps,
                info.video_width,
                info.video_height,
                info.video_codec,
                info.video_bitrate,
                info.audio_codec,
                info.audio_sample_rate,
                info.audio_bitrate,
                crate::format::now(),
            ],
        )?;
        Ok(())
    }

    /// Figures of a local file's current content; `None` when it was not probed (or only before
    /// it was hashed to other content).
    pub fn media_info(&self, local_id: i64) -> Result<Option<MediaInfo>> {
        Ok(self
            .conn()
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM media_info m
                     JOIN local_file l ON l.id = m.local_file_id AND l.hash_id = m.hash_id
                     WHERE m.local_file_id = ?1"
                ),
                [local_id],
                |r| from_row(r, 0),
            )
            .optional()?)
    }

    /// Figures of every probed local file, by local file id, for the hash export.
    pub fn media_info_by_file(&self) -> Result<HashMap<i64, MediaInfo>> {
        let mut statement = self.conn().prepare(&format!(
            "SELECT m.local_file_id, {COLUMNS} FROM media_info m
             JOIN local_file l ON l.id = m.local_file_id AND l.hash_id = m.hash_id"
        ))?;
        let rows = statement
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, from_row(r, 1)?)))?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?;
        Ok(rows)
    }

    /// Hashed local files that have no figures for their current content, in path order.
    pub fn media_targets_missing(&self) -> Result<Vec<MediaTarget>> {
        let mut statement = self.conn().prepare(
            "SELECT l.id, l.path FROM local_file l
             WHERE NOT EXISTS (SELECT 1 FROM media_info m
                               WHERE m.local_file_id = l.id AND m.hash_id = l.hash_id)
             ORDER BY l.path",
        )?;
        let rows = statement
            .query_map([], |r| {
                Ok(MediaTarget {
                    local_id: r.get(0)?,
                    path: PathBuf::from(r.get::<_, String>(1)?),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

/// Progress of [`read_files`].
#[derive(Debug)]
pub enum ReadEvent<'a> {
    /// About to probe `target` (`index` is zero-based).
    Start {
        index: usize,
        total: usize,
        target: &'a MediaTarget,
    },
    /// The file could not be probed. Whether the attempt was saved depends on the error: a
    /// file `ffprobe` refused is recorded (and not tried again), a timeout is not.
    Failed {
        target: &'a MediaTarget,
        error: &'a MediaError,
    },
}

/// What a batch did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReadReport {
    /// Files whose figures were saved (including files without any usable figure).
    pub saved: usize,
    /// Files that failed; see [`ReadEvent::Failed`].
    pub failed: usize,
    /// The batch stopped early because it was cancelled.
    pub cancelled: bool,
    /// The batch stopped early because `ffprobe` cannot be started.
    pub tool_unavailable: bool,
}

/// Probe each target after its hash is in the library and save the figures. A failure of one
/// file never stops the others; a cancel does, and so does an `ffprobe` that cannot be started
/// (every further file would fail the same way).
pub fn read_files(
    db: &Db,
    ffprobe: &Path,
    targets: &[MediaTarget],
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(ReadEvent<'_>),
) -> ReadReport {
    let mut report = ReadReport::default();
    for (index, target) in targets.iter().enumerate() {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            report.cancelled = true;
            break;
        }
        on_event(ReadEvent::Start {
            index,
            total: targets.len(),
            target,
        });
        let hash_id = match db.local_file_hash_id(target.local_id) {
            Ok(Some(id)) => id,
            // Removed from the library while the batch ran.
            Ok(None) => continue,
            Err(e) => {
                report.failed += 1;
                on_event(ReadEvent::Failed {
                    target,
                    error: &MediaError::Db(e),
                });
                continue;
            }
        };
        // An unavailable drive or a moved file is not a verdict on the file: nothing is saved,
        // so it is probed again when it is back.
        if !target.path.is_file() {
            report.failed += 1;
            on_event(ReadEvent::Failed {
                target,
                error: &MediaError::FileMissing,
            });
            continue;
        }
        let outcome = super::probe_file(ffprobe, &target.path, cancel);
        let save = match &outcome {
            Ok(info) => Some(info.clone()),
            // ffprobe ran and refused the file: remember that, so it is not probed again.
            Err(MediaError::Unreadable(_)) => Some(MediaInfo::default()),
            Err(_) => None,
        };
        if let Some(info) = save {
            match db.set_media_info(target.local_id, hash_id, &info) {
                Ok(()) if outcome.is_ok() => {
                    report.saved += 1;
                    continue;
                }
                Ok(()) => report.saved += 1,
                Err(e) => {
                    report.failed += 1;
                    on_event(ReadEvent::Failed {
                        target,
                        error: &MediaError::Db(e),
                    });
                    continue;
                }
            }
        }
        if let Err(error) = &outcome {
            match error {
                MediaError::Cancelled => {
                    report.cancelled = true;
                    break;
                }
                MediaError::Spawn(_) => {
                    report.failed += 1;
                    report.tool_unavailable = true;
                    on_event(ReadEvent::Failed { target, error });
                    break;
                }
                _ => {
                    report.failed += 1;
                    on_event(ReadEvent::Failed { target, error });
                }
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::super::probe::tests::fake_ffprobe;
    use super::*;
    use std::sync::atomic::Ordering;

    const SAMPLE: &str = r#"{"streams":[
        {"codec_type":"video","codec_name":"h264","width":1280,"height":720,
         "avg_frame_rate":"25/1","bit_rate":"2000000"},
        {"codec_type":"audio","codec_name":"aac","sample_rate":"44100","bit_rate":"96000"}],
        "format":{"duration":"90.5"}}"#;

    fn db_with_files() -> Db {
        let db = Db::memory().unwrap();
        db.conn()
            .execute_batch(
                "INSERT INTO local_hash(id, ed2k, size, crc32, md5, sha1) VALUES
                    (1, 'aa', 1000, 'DEADBEEF', 'MD5A', 'SHA1A'), (2, 'bb', 2000, 'CAFEBABE', 'MD5B', 'SHA1B');
                 INSERT INTO local_file(id, hash_id, path) VALUES (1, 1, '/a/one.mkv'), (2, 2, '/a/two.mkv');",
            )
            .unwrap();
        db
    }

    fn full() -> MediaInfo {
        MediaInfo {
            length: Some(90.5),
            fps: Some(25.0),
            video_width: Some(1280),
            video_height: Some(720),
            video_codec: Some("h264".into()),
            video_bitrate: Some(2000.0),
            audio_codec: Some("aac".into()),
            audio_sample_rate: Some(44_100),
            audio_bitrate: Some(96.0),
        }
    }

    #[test]
    fn figures_round_trip_and_are_replaced_by_a_new_probe() {
        let db = db_with_files();
        assert_eq!(db.media_info(1).unwrap(), None);
        db.set_media_info(1, 1, &full()).unwrap();
        assert_eq!(db.media_info(1).unwrap(), Some(full()));
        let updated = MediaInfo {
            length: Some(1.0),
            ..MediaInfo::default()
        };
        db.set_media_info(1, 1, &updated).unwrap();
        assert_eq!(db.media_info(1).unwrap(), Some(updated));
        assert_eq!(db.media_info(2).unwrap(), None);
    }

    #[test]
    fn figures_of_other_content_are_ignored_and_never_saved() {
        let db = db_with_files();
        db.set_media_info(1, 1, &full()).unwrap();
        // The file is hashed again and now holds different content.
        db.conn()
            .execute("UPDATE local_file SET hash_id = 2 WHERE id = 1", [])
            .unwrap();
        assert_eq!(db.media_info(1).unwrap(), None);
        assert!(db.media_info_by_file().unwrap().is_empty());
        let missing: Vec<i64> = db
            .media_targets_missing()
            .unwrap()
            .iter()
            .map(|t| t.local_id)
            .collect();
        assert_eq!(missing, vec![1, 2]);
        // A probe of the old content cannot overwrite the row for the new one.
        db.set_media_info(1, 1, &MediaInfo::default()).unwrap();
        assert_eq!(db.media_info(1).unwrap(), None);
        // Probing the new content replaces the stale row.
        db.set_media_info(1, 2, &full()).unwrap();
        assert_eq!(db.media_info(1).unwrap(), Some(full()));
    }

    #[test]
    fn removing_a_file_removes_its_figures() {
        let db = db_with_files();
        db.set_media_info(1, 1, &full()).unwrap();
        db.remove_local_file(1).unwrap();
        let rows: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM media_info", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
        db.set_media_info(2, 2, &full()).unwrap();
        db.clear_local_files().unwrap();
        let rows: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM media_info", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn the_table_is_added_to_an_existing_database_without_touching_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite3");
        {
            let db = Db::open(&path).unwrap();
            db.conn()
                .execute_batch(
                    "INSERT INTO local_hash(id, ed2k, size) VALUES (1, 'aa', 1);
                     INSERT INTO local_file(id, hash_id, path) VALUES (1, 1, '/a/one.mkv');
                     DROP TABLE media_info;",
                )
                .unwrap();
        }
        let db = Db::open(&path).unwrap();
        db.set_media_info(1, 1, &full()).unwrap();
        drop(db);
        // Opening again keeps the saved figures.
        let db = Db::open(&path).unwrap();
        assert_eq!(db.media_info(1).unwrap(), Some(full()));
        assert_eq!(db.local_files().unwrap().len(), 1);
    }

    #[test]
    fn hash_export_uses_saved_figures_and_keeps_constants_without_them() {
        let db = db_with_files();
        db.set_media_info(1, 1, &full()).unwrap();
        let template = "%filename%|%fps%|%length%|%height%|%width%|%vcodec%|%vbitrate%|%acodec%|%asample%|%abitrate%";
        let (text, count) = db.local_hashes_export_text(template, false).unwrap();
        assert_eq!(count, 2);
        let lines: Vec<&str> = text.lines().collect();
        // 90.5 s is exported as 91 whole seconds.
        assert_eq!(
            lines[0],
            "one.mkv|25.00|91|720|1280|h264|2000.00|aac|44100|96.00"
        );
        // The second file was not probed: the constants stay.
        assert_eq!(lines[1], "two.mkv|0.00||0|0||0.00||0|0.00");
        let (minutes, _) = db.local_hashes_export_text("%length%", true).unwrap();
        assert_eq!(minutes.lines().collect::<Vec<_>>(), vec!["2", "0"]);
    }

    #[test]
    fn a_partly_known_file_keeps_constants_for_the_rest() {
        let db = db_with_files();
        let audio_only = MediaInfo {
            length: Some(125.0),
            audio_codec: Some("flac".into()),
            ..MediaInfo::default()
        };
        db.set_media_info(1, 1, &audio_only).unwrap();
        let (text, _) = db
            .local_hashes_export_text("%vcodec%/%acodec%/%width%/%length%", false)
            .unwrap();
        assert_eq!(text.lines().next().unwrap(), "/flac/0/125");
    }

    // ---- batch ------------------------------------------------------------------------------

    fn targets(db: &Db) -> Vec<MediaTarget> {
        db.media_targets_missing().unwrap()
    }

    #[test]
    fn the_batch_probes_every_target_and_saves_what_ffprobe_printed() {
        let db = db_with_files();
        let dir = tempfile::tempdir().unwrap();
        let ffprobe = fake_ffprobe(dir.path(), SAMPLE, 0);
        let mut started = Vec::new();
        let report = read_files(
            &db,
            &ffprobe,
            // Absolute paths are required; the fake ffprobe does not open them.
            &absolute_targets(&db, dir.path()),
            &AtomicBool::new(false),
            &mut |event| {
                if let ReadEvent::Start { index, total, .. } = event {
                    started.push((index, total));
                }
            },
        );
        assert_eq!(report.saved, 2);
        assert_eq!(report.failed, 0);
        assert_eq!(started, vec![(0, 2), (1, 2)]);
        let info = db.media_info(1).unwrap().unwrap();
        assert_eq!(info.video_codec.as_deref(), Some("h264"));
        assert_eq!(info.length, Some(90.5));
        assert!(targets(&db).is_empty(), "nothing is left to probe");
    }

    /// The library holds paths that do not exist; point the targets at (empty) files in the temp
    /// folder so that they exist and are absolute on every platform. The fake ffprobe does not
    /// open them.
    fn absolute_targets(db: &Db, dir: &Path) -> Vec<MediaTarget> {
        targets(db)
            .into_iter()
            .map(|t| {
                let path = dir.join(t.path.file_name().unwrap());
                std::fs::write(&path, b"").unwrap();
                MediaTarget { path, ..t }
            })
            .collect()
    }

    #[test]
    fn a_file_that_is_not_there_is_reported_and_not_recorded() {
        let db = db_with_files();
        let dir = tempfile::tempdir().unwrap();
        let ffprobe = fake_ffprobe(dir.path(), SAMPLE, 0);
        // Not created: an unavailable drive looks exactly like this.
        let absent: Vec<MediaTarget> = targets(&db)
            .into_iter()
            .map(|t| MediaTarget {
                path: dir.path().join(t.path.file_name().unwrap()),
                ..t
            })
            .collect();
        let mut errors = Vec::new();
        let report = read_files(
            &db,
            &ffprobe,
            &absent,
            &AtomicBool::new(false),
            &mut |event| {
                if let ReadEvent::Failed { error, .. } = event {
                    errors.push(error.to_string());
                }
            },
        );
        assert_eq!((report.saved, report.failed), (0, 2));
        assert!(!report.tool_unavailable);
        assert!(
            errors.iter().all(|e| e.contains("does not exist")),
            "{errors:?}"
        );
        assert_eq!(targets(&db).len(), 2, "tried again once the files are back");
    }

    #[test]
    fn a_file_ffprobe_refuses_is_recorded_once_and_reported() {
        let db = db_with_files();
        let dir = tempfile::tempdir().unwrap();
        let ffprobe = fake_ffprobe(dir.path(), "", 1);
        let mut failures = Vec::new();
        let report = read_files(
            &db,
            &ffprobe,
            &absolute_targets(&db, dir.path()),
            &AtomicBool::new(false),
            &mut |event| {
                if let ReadEvent::Failed { target, error } = event {
                    failures.push((target.local_id, error.to_string()));
                }
            },
        );
        assert_eq!((report.saved, report.failed), (2, 2));
        assert_eq!(failures.len(), 2);
        // The attempt is recorded as an empty row, so a later "read missing" skips it.
        assert_eq!(db.media_info(1).unwrap(), Some(MediaInfo::default()));
        assert!(targets(&db).is_empty());
    }

    #[test]
    fn an_ffprobe_that_cannot_start_stops_the_batch_and_saves_nothing() {
        let db = db_with_files();
        let dir = tempfile::tempdir().unwrap();
        let mut failures = 0;
        let report = read_files(
            &db,
            &dir.path().join("missing-ffprobe"),
            &absolute_targets(&db, dir.path()),
            &AtomicBool::new(false),
            &mut |event| {
                if matches!(event, ReadEvent::Failed { .. }) {
                    failures += 1;
                }
            },
        );
        assert!(report.tool_unavailable);
        assert_eq!((report.saved, report.failed, failures), (0, 1, 1));
        assert_eq!(targets(&db).len(), 2);
    }

    #[test]
    fn cancelling_stops_before_the_next_file() {
        let db = db_with_files();
        let dir = tempfile::tempdir().unwrap();
        let ffprobe = fake_ffprobe(dir.path(), SAMPLE, 0);
        let cancel = AtomicBool::new(false);
        let report = read_files(
            &db,
            &ffprobe,
            &absolute_targets(&db, dir.path()),
            &cancel,
            &mut |event| {
                if matches!(event, ReadEvent::Start { index: 0, .. }) {
                    cancel.store(true, Ordering::Release);
                }
            },
        );
        // The second file never starts, whether or not the first probe finished.
        assert!(report.cancelled);
        assert!(report.saved <= 1);
        assert!(db.media_info(2).unwrap().is_none());
    }

    #[test]
    fn files_removed_during_the_batch_are_skipped() {
        let db = db_with_files();
        let dir = tempfile::tempdir().unwrap();
        let ffprobe = fake_ffprobe(dir.path(), SAMPLE, 0);
        let list = absolute_targets(&db, dir.path());
        db.remove_local_file(2).unwrap();
        let report = read_files(&db, &ffprobe, &list, &AtomicBool::new(false), &mut |_| {});
        assert_eq!((report.saved, report.failed), (1, 0));
    }
}
