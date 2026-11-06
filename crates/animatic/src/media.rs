//! Background side of the opt-in media properties (see `animatic_core::media`).
//!
//! Two pieces, both off the UI thread:
//!
//! * [`MediaTools`], a small thread that finds `ffprobe` (`ffprobe -version` is a process, so
//!   never the UI thread) and runs the confirmed one-click install. It has nothing to do with
//!   the AniDB worker, the AniDB limiter or the update thread; its only network use is the
//!   pinned download.
//! * [`read_media`], the body of the main worker's `Job::ReadMedia`: after files were hashed, a
//!   separate job probes them one by one. It never touches the hash read (rule 5).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use animatic_core::db::Db;
use animatic_core::media::install::{Installer, current_pin};
use animatic_core::media::store::{MediaTarget, ReadEvent};
use animatic_core::media::tool::{self, Detection};
use animatic_core::media::{self, MediaError};
use crossbeam_channel::{Receiver, Sender};

use crate::worker::Event;

/// A request for the tools thread.
#[derive(Debug)]
pub enum ToolJob {
    /// Search for `ffprobe`: the configured path, Animatic's own copy and `PATH`.
    Detect { configured: String },
    /// Download, verify and install the pinned build, then search again.
    Install { configured: String },
}

/// Answer of the tools thread.
#[derive(Debug)]
pub enum ToolEvent {
    Detected(Detection),
    InstallProgress { done: u64, total: u64 },
    Installed(Detection),
    InstallFailed(String),
    InstallCancelled,
}

/// Owner of the tools thread. Dropping it cancels the running job and waits for the thread;
/// every wait inside a job is bounded (process timeouts, network timeouts, cancel checks).
pub struct MediaTools {
    jobs: Option<Sender<ToolJob>>,
    pub events: Receiver<ToolEvent>,
    cancel: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MediaTools {
    /// `data_dir` is the folder that holds the database; Animatic's own `ffprobe` lives in
    /// `<data_dir>/tools/ffprobe/`.
    pub fn spawn(data_dir: PathBuf) -> MediaTools {
        let (job_tx, job_rx) = crossbeam_channel::unbounded();
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("animatic-media-tools".into())
            .spawn(move || run_tools(&data_dir, job_rx, event_tx, &flag))
            .expect("start media tools thread");
        MediaTools {
            jobs: Some(job_tx),
            events: event_rx,
            cancel,
            thread: Some(thread),
        }
    }

    /// The UI submits only while no job is in flight, so the cancel flag is cleared here.
    pub fn submit(&self, job: ToolJob) {
        if let Some(jobs) = &self.jobs {
            self.cancel.store(false, Ordering::SeqCst);
            let _ = jobs.send(job);
        }
    }

    /// Cancels the running download or search.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

impl Drop for MediaTools {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_tools(
    data_dir: &Path,
    jobs: Receiver<ToolJob>,
    events: Sender<ToolEvent>,
    cancel: &AtomicBool,
) {
    while let Ok(job) = jobs.recv() {
        let event = match job {
            ToolJob::Detect { configured } => {
                let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    tool::detect(&configured, data_dir, cancel)
                }));
                ToolEvent::Detected(found.unwrap_or_default())
            }
            ToolJob::Install { configured } => {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    install(&configured, data_dir, &events, cancel)
                }))
                .unwrap_or_else(|_| {
                    ToolEvent::InstallFailed("the installation stopped unexpectedly".into())
                })
            }
        };
        if events.send(event).is_err() {
            break;
        }
    }
}

fn install(
    configured: &str,
    data_dir: &Path,
    events: &Sender<ToolEvent>,
    cancel: &AtomicBool,
) -> ToolEvent {
    let Some(pin) = current_pin() else {
        return ToolEvent::InstallFailed(MediaError::UnsupportedPlatform.to_string());
    };
    let mut last_percent = None;
    let result = Installer::new().install(
        pin,
        &tool::install_dir(data_dir),
        cancel,
        &mut |done, total| {
            // At most one event per percent: the UI repaints for each of them.
            let percent = done * 100 / total.max(1);
            if last_percent != Some(percent) {
                last_percent = Some(percent);
                let _ = events.send(ToolEvent::InstallProgress { done, total });
            }
        },
    );
    match result {
        // The new copy must actually run on this system before the install counts.
        Ok(installed) => match media::probe::ffprobe_version(&installed.exe, cancel) {
            Ok(_) => ToolEvent::Installed(tool::detect(configured, data_dir, cancel)),
            Err(MediaError::Cancelled) => ToolEvent::InstallCancelled,
            Err(e) => ToolEvent::InstallFailed(format!(
                "the downloaded ffprobe does not start on this system ({e})"
            )),
        },
        Err(MediaError::Cancelled) => ToolEvent::InstallCancelled,
        Err(e) => ToolEvent::InstallFailed(e.to_string()),
    }
}

/// Which files a `Job::ReadMedia` probes.
#[derive(Debug, Clone)]
pub enum MediaScope {
    /// Exactly these files (just hashed).
    Files(Vec<MediaTarget>),
    /// Every hashed file that has no figures for its current content.
    Missing,
}

/// Body of `Job::ReadMedia`: probe the files, save the figures, report through `events` (a
/// `Started` first and a `Finished` last, like every counted job).
pub fn read_media(
    db_path: &Path,
    ffprobe: &Path,
    scope: MediaScope,
    events: &Sender<Event>,
    cancel: &AtomicBool,
) {
    let outcome = Db::open(db_path).map_err(|e| e.to_string()).and_then(|db| {
        let targets = match scope {
            MediaScope::Files(files) => files,
            MediaScope::Missing => db.media_targets_missing().map_err(|e| e.to_string())?,
        };
        let total = targets.len();
        let _ = events.send(Event::Started {
            label: "Reading media properties".into(),
            total,
        });
        let report = media::read_files(&db, ffprobe, &targets, cancel, &mut |event| match event {
            ReadEvent::Start {
                index,
                total,
                target,
            } => {
                let _ = events.send(Event::Progress {
                    label: target
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    current: index + 1,
                    total,
                    fraction: index as f32 / total.max(1) as f32,
                });
            }
            ReadEvent::Failed { target, error } => {
                let _ = events.send(Event::Error(format!(
                    "Media properties of {}: {error}",
                    target.path.display()
                )));
            }
        });
        Ok((total, report))
    });
    match outcome {
        Ok((total, report)) => {
            let mut description =
                format!("Read media properties of {} of {total} files", report.saved);
            if report.cancelled {
                description.push_str(" (cancelled)");
            }
            if report.tool_unavailable {
                description.push_str("; ffprobe could not be started");
            }
            let _ = events.send(Event::Synced {
                description,
                changed: false,
            });
        }
        Err(e) => {
            let _ = events.send(Event::Error(format!("Media properties: {e}")));
        }
    }
    let _ = events.send(Event::Finished);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    const SAMPLE: &str = r#"{"streams":[
        {"codec_type":"video","codec_name":"h264","width":640,"height":360,
         "avg_frame_rate":"30/1","bit_rate":"1000000"},
        {"codec_type":"audio","codec_name":"aac","sample_rate":"48000","bit_rate":"64000"}],
        "format":{"duration":"12.5"}}"#;

    /// A stand-in `ffprobe`: a banner for `-version`, `SAMPLE` for everything else. A shell
    /// script on Unix and a batch file on Windows.
    fn fake_ffprobe(dir: &Path) -> PathBuf {
        fs::write(dir.join("out.json"), SAMPLE).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join("fake-ffprobe");
            fs::write(
                &path,
                "#!/bin/sh\nif [ \"$1\" = \"-version\" ]; then echo 'ffprobe version 9.9-test'; \
                 else cat \"$(dirname \"$0\")/out.json\"; fi\n",
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
        #[cfg(windows)]
        {
            let path = dir.join("fake-ffprobe.cmd");
            fs::write(
                &path,
                "@echo off\r\nif \"%1\"==\"-version\" (echo ffprobe version 9.9-test) else \
                 (type \"%~dp0out.json\")\r\n",
            )
            .unwrap();
            path
        }
    }

    /// A library of two hashed files that exist (empty) in `dir`; the stand-in `ffprobe` does
    /// not open them.
    fn library(dir: &Path) -> (PathBuf, Vec<MediaTarget>) {
        let db_path = dir.join("library.sqlite3");
        let db = Db::open(&db_path).unwrap();
        db.conn()
            .execute_batch(
                "INSERT INTO local_hash(id, ed2k, size) VALUES (1, 'aa', 10), (2, 'bb', 20);",
            )
            .unwrap();
        let targets: Vec<MediaTarget> = ["one.mkv", "two.mkv"]
            .iter()
            .enumerate()
            .map(|(i, name)| MediaTarget {
                local_id: i as i64 + 1,
                path: dir.join(name),
            })
            .collect();
        for target in &targets {
            fs::write(&target.path, b"").unwrap();
            db.conn()
                .execute(
                    "INSERT INTO local_file(id, hash_id, path) VALUES (?1, ?1, ?2)",
                    (target.local_id, target.path.to_string_lossy().into_owned()),
                )
                .unwrap();
        }
        (db_path, targets)
    }

    fn drain(events: &Receiver<Event>) -> Vec<Event> {
        let mut all = Vec::new();
        while let Ok(event) = events.recv_timeout(Duration::from_secs(30)) {
            let done = matches!(event, Event::Finished);
            all.push(event);
            if done {
                break;
            }
        }
        all
    }

    #[test]
    fn a_media_job_saves_figures_and_reports_like_every_counted_job() {
        let dir = tempfile::tempdir().unwrap();
        let ffprobe = fake_ffprobe(dir.path());
        let (db_path, targets) = library(dir.path());
        let (tx, rx) = crossbeam_channel::unbounded();
        read_media(
            &db_path,
            &ffprobe,
            MediaScope::Files(targets),
            &tx,
            &AtomicBool::new(false),
        );
        let events = drain(&rx);
        assert!(matches!(
            events.first(),
            Some(Event::Started { total: 2, .. })
        ));
        assert!(matches!(events.last(), Some(Event::Finished)));
        let described = events.iter().find_map(|e| match e {
            Event::Synced { description, .. } => Some(description.clone()),
            _ => None,
        });
        assert_eq!(
            described.as_deref(),
            Some("Read media properties of 2 of 2 files")
        );
        let db = Db::open(&db_path).unwrap();
        let info = db.media_info(2).unwrap().unwrap();
        assert_eq!(info.video_codec.as_deref(), Some("h264"));
        assert_eq!(info.length, Some(12.5));
        assert!(db.media_targets_missing().unwrap().is_empty());
    }

    #[test]
    fn the_missing_scope_probes_only_files_without_figures() {
        let dir = tempfile::tempdir().unwrap();
        let ffprobe = fake_ffprobe(dir.path());
        let (db_path, _) = library(dir.path());
        let db = Db::open(&db_path).unwrap();
        db.set_media_info(1, 1, &animatic_core::media::MediaInfo::default())
            .unwrap();
        drop(db);
        let (tx, rx) = crossbeam_channel::unbounded();
        read_media(
            &db_path,
            &ffprobe,
            MediaScope::Missing,
            &tx,
            &AtomicBool::new(false),
        );
        let events = drain(&rx);
        assert!(matches!(
            events.first(),
            Some(Event::Started { total: 1, .. })
        ));
        assert!(events.iter().all(|e| !matches!(e, Event::Error(_))));
        // `one.mkv` keeps its recorded attempt (an empty row); `two.mkv` was probed now.
        let db = Db::open(&db_path).unwrap();
        assert!(db.media_info(1).unwrap().unwrap().is_empty());
        assert_eq!(db.media_info(2).unwrap().unwrap().length, Some(12.5));
    }

    #[test]
    fn a_broken_ffprobe_is_reported_once_and_ends_the_job() {
        let dir = tempfile::tempdir().unwrap();
        let (db_path, targets) = library(dir.path());
        let (tx, rx) = crossbeam_channel::unbounded();
        read_media(
            &db_path,
            &dir.path().join("no-such-ffprobe"),
            MediaScope::Files(targets),
            &tx,
            &AtomicBool::new(false),
        );
        let events = drain(&rx);
        let errors = events
            .iter()
            .filter(|e| matches!(e, Event::Error(_)))
            .count();
        assert_eq!(errors, 1);
        assert!(matches!(events.last(), Some(Event::Finished)));
        assert!(Db::open(&db_path).unwrap().media_info(1).unwrap().is_none());
    }

    #[test]
    fn the_tools_thread_reports_a_configured_ffprobe_and_a_bad_path() {
        let dir = tempfile::tempdir().unwrap();
        let ffprobe = fake_ffprobe(dir.path());
        let tools = MediaTools::spawn(dir.path().join("data"));
        tools.submit(ToolJob::Detect {
            configured: ffprobe.display().to_string(),
        });
        let Ok(ToolEvent::Detected(found)) = tools.events.recv_timeout(Duration::from_secs(30))
        else {
            panic!("no detection answer");
        };
        let tool = found.tool.expect("the configured fake is accepted");
        assert_eq!(tool.version, "9.9-test");
        assert_eq!(tool.source, tool::ToolSource::Configured);

        tools.submit(ToolJob::Detect {
            configured: dir.path().join("missing").display().to_string(),
        });
        let Ok(ToolEvent::Detected(found)) = tools.events.recv_timeout(Duration::from_secs(30))
        else {
            panic!("no detection answer");
        };
        assert!(found.notes.iter().any(|n| n.contains("is not a file")));
    }
}
