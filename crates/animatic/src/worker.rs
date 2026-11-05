use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use animatic_anidb::http::HttpClient;
use animatic_anidb::udp::{Config as UdpConfig, UdpClient};
use animatic_core::db::Db;
use animatic_core::file_manager::hash_and_register;
use animatic_core::hash::Ed2kVariant;
use animatic_core::settings::{AnidbSettings, FileManagerSettings, Settings};
use crossbeam_channel::{Receiver, Sender};

#[derive(Debug)]
pub enum Job {
    Hash(Vec<PathBuf>),
    Scan {
        settings: FileManagerSettings,
        extensions: String,
    },
    ImportLegacy {
        source: PathBuf,
        temp: PathBuf,
    },
    ImportLegacySettings {
        source: PathBuf,
    },
    ProcessQueue(AnidbSettings),
    Stop,
}
#[derive(Debug, Clone)]
pub enum Event {
    Started {
        label: String,
        total: usize,
    },
    Progress {
        label: String,
        current: usize,
        total: usize,
        fraction: f32,
    },
    Hashed {
        path: PathBuf,
        local_id: i64,
        size: u64,
        ed2k: String,
    },
    Imported {
        summary: String,
    },
    LegacySettingsImported(Box<Settings>),
    LegacySettingsImportFailed(String),
    Synced {
        description: String,
        changed: bool,
    },
    Error(String),
    Finished,
}
pub struct Worker {
    pub jobs: Sender<Job>,
    pub events: Receiver<Event>,
    pub cancel: Arc<AtomicBool>,
    pub pause: Arc<AtomicBool>,
}

impl Worker {
    pub fn spawn(db_path: PathBuf) -> Worker {
        let (job_tx, job_rx) = crossbeam_channel::unbounded();
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let p = pause.clone();
        std::thread::Builder::new()
            .name("animatic-worker".into())
            .spawn(move || run(db_path, job_rx, event_tx, c, p))
            .expect("start worker");
        Worker {
            jobs: job_tx,
            events: event_rx,
            cancel,
            pause,
        }
    }

    pub fn submit(&self, job: Job) {
        self.cancel.store(false, Ordering::Relaxed);
        let _ = self.jobs.send(job);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.jobs.send(Job::Stop);
    }
}

fn run(
    db_path: PathBuf,
    jobs: Receiver<Job>,
    events: Sender<Event>,
    cancel: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
) {
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Stop => break,
            Job::Hash(paths) => {
                let total = paths.len();
                let _ = events.send(Event::Started {
                    label: "Hashing files".into(),
                    total,
                });
                for (index, path) in paths.into_iter().enumerate() {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let label = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let result = Db::open(&db_path).and_then(|db| {
                        hash_and_register(
                            &db,
                            &path,
                            Ed2kVariant::Red,
                            &cancel,
                            &pause,
                            &mut |done, size| {
                                let _ = events.send(Event::Progress {
                                    label: label.clone(),
                                    current: index + 1,
                                    total,
                                    fraction: done as f32 / size.max(1) as f32,
                                });
                            },
                        )
                    });
                    match result {
                        Ok((local_id, hashes)) => {
                            let _ = events.send(Event::Hashed {
                                path,
                                local_id,
                                size: hashes.size,
                                ed2k: hashes.ed2k,
                            });
                        }
                        Err(e) => {
                            let _ = events.send(Event::Error(format!("{}: {e}", path.display())));
                        }
                    }
                }
                let _ = events.send(Event::Finished);
            }
            Job::ProcessQueue(settings) => {
                if settings.offline {
                    let _ = events.send(Event::Error(
                        "Offline mode is enabled; commands remain queued".into(),
                    ));
                    continue;
                }
                let _ = events.send(Event::Started {
                    label: "Synchronizing with AniDB".into(),
                    total: 0,
                });
                let mut udp = UdpClient::new(UdpConfig {
                    server: settings.udp_server.clone(),
                    local_port: settings.local_port,
                    client: settings.client_name.clone(),
                    client_version: settings.client_version,
                    username: settings.username.clone(),
                    password: settings.password.clone(),
                    api_key: (!settings.api_key.is_empty()).then(|| settings.api_key.clone()),
                    ..Default::default()
                });
                let mut http = HttpClient::new(&settings.client_name, settings.client_version);
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let db = match Db::open(&db_path) {
                        Ok(db) => db,
                        Err(e) => {
                            let _ = events.send(Event::Error(e.to_string()));
                            break;
                        }
                    };
                    let next = match db.queue_peek() {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = events.send(Event::Error(e.to_string()));
                            break;
                        }
                    };
                    let Some((id, command, _tries)) = next else {
                        break;
                    };
                    let _ = events.send(Event::Progress {
                        label: command.describe(),
                        current: 0,
                        total: 0,
                        fraction: 0.0,
                    });
                    match animatic_core::sync::execute(&command, &db, &mut udp, &mut http) {
                        Ok(outcome) => {
                            let changed = outcome.changes_library();
                            let description = outcome.describe();
                            if let Err(e) = db.queue_remove(id) {
                                let _ = events.send(Event::Error(e.to_string()));
                                break;
                            }
                            let _ = events.send(Event::Synced {
                                description,
                                changed,
                            });
                        }
                        Err(e) => {
                            let _ = db.queue_failed(id, &e.message);
                            let _ = events.send(Event::Error(format!(
                                "{}: {}",
                                command.describe(),
                                e.message
                            )));
                            if e.fatal || e.transient {
                                break;
                            }
                        }
                    }
                }
                let _ = udp.logout();
                let _ = events.send(Event::Finished);
            }
            Job::Scan {
                settings,
                extensions,
            } => {
                let db = match Db::open(&db_path) {
                    Ok(db) => db,
                    Err(e) => {
                        let _ = events.send(Event::Error(e.to_string()));
                        continue;
                    }
                };
                let actions =
                    match animatic_core::file_manager::plan_scan(&db, &settings, &extensions) {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = events.send(Event::Error(e.to_string()));
                            continue;
                        }
                    };
                let total = actions.len();
                let _ = events.send(Event::Started {
                    label: "Scanning managed folders".into(),
                    total,
                });
                for (index, action) in actions.into_iter().enumerate() {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    match action {
                        animatic_core::file_manager::ScanAction::Hash { path, .. }
                        | animatic_core::file_manager::ScanAction::Rehash { path, .. } => {
                            let label = path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned();
                            match hash_and_register(
                                &db,
                                &path,
                                Ed2kVariant::Red,
                                &cancel,
                                &pause,
                                &mut |done, size| {
                                    let _ = events.send(Event::Progress {
                                        label: label.clone(),
                                        current: index + 1,
                                        total,
                                        fraction: done as f32 / size.max(1) as f32,
                                    });
                                },
                            ) {
                                Ok((local_id, hashes)) => {
                                    let _ = events.send(Event::Hashed {
                                        path,
                                        local_id,
                                        size: hashes.size,
                                        ed2k: hashes.ed2k,
                                    });
                                }
                                Err(e) => {
                                    let _ = events
                                        .send(Event::Error(format!("{}: {e}", path.display())));
                                }
                            }
                        }
                        animatic_core::file_manager::ScanAction::RemoveMissing {
                            local_id,
                            path,
                        } => match animatic_core::file_manager::remove_missing(&db, local_id) {
                            Ok(()) => {
                                let _ = events.send(Event::Synced {
                                    description: format!("Removed missing path {}", path.display()),
                                    changed: true,
                                });
                            }
                            Err(e) => {
                                let _ = events.send(Event::Error(e.to_string()));
                            }
                        },
                    }
                }
                let _ = events.send(Event::Finished);
            }
            Job::ImportLegacySettings { source } => {
                let _ = events.send(Event::Started {
                    label: "Reading AOM settings".into(),
                    total: 1,
                });
                match animatic_core::settings::import_legacy_settings(&source) {
                    Ok(settings) => {
                        let _ = events.send(Event::LegacySettingsImported(Box::new(settings)));
                    }
                    Err(e) => {
                        let _ = events.send(Event::LegacySettingsImportFailed(format!(
                            "Could not import AOM settings: {e}"
                        )));
                    }
                }
                let _ = events.send(Event::Finished);
            }
            Job::ImportLegacy { source, temp } => {
                let _ = events.send(Event::Started {
                    label: "Importing legacy databank".into(),
                    total: 1,
                });
                let result = Db::open(&db_path).and_then(|mut db| {
                    animatic_core::legacy::import::import_databank(
                        &mut db,
                        &source,
                        &temp,
                        &mut |label, fraction| {
                            let _ = events.send(Event::Progress {
                                label: label.into(),
                                current: 1,
                                total: 1,
                                fraction,
                            });
                            !cancel.load(Ordering::Relaxed)
                        },
                    )
                });
                match result {
                    Ok(report) => {
                        let records: usize = report.imported.iter().map(|(_, n)| *n).sum();
                        let _ = events.send(Event::Imported {
                            summary: format!(
                                "Imported {records} records from {} collections",
                                report.imported.len()
                            ),
                        });
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(format!("Legacy import failed: {e}")));
                    }
                }
                let _ = events.send(Event::Finished);
            }
        }
    }
}
