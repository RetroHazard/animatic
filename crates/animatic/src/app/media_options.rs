//! Options and workflow of the opt-in media properties (see `animatic_core::media`): the
//! detected `ffprobe`, the confirmed one-click install, and the read after hashing.
//!
//! Everything that starts a process or touches the network runs on the media tools thread or the
//! main worker; this module only keeps the state the window shows.

use std::path::{Path, PathBuf};

use animatic_core::format;
use animatic_core::media::install::current_pin;
use animatic_core::media::store::MediaTarget;
use animatic_core::media::tool::{self, Ffprobe};
use eframe::egui::{self, Color32, RichText};

use super::AnimaticApp;
use crate::media::{MediaScope, MediaTools, ToolEvent, ToolJob};
use crate::worker::Job;

/// Where the install of `ffprobe` stands.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Install {
    Idle,
    /// The confirmation window is open.
    Confirm,
    Running {
        done: u64,
        total: u64,
        cancelling: bool,
    },
}

/// State of the media properties feature, owned by the application.
pub struct MediaUi {
    tools: MediaTools,
    data_dir: PathBuf,
    /// The `ffprobe` of the last search; `None` when none was found (or no search has finished).
    tool: Option<Ffprobe>,
    /// Problems with the configured path found by the last search.
    notes: Vec<String>,
    detecting: bool,
    /// The configured path the last search was started with.
    searched_path: String,
    install: Install,
    error: Option<String>,
    /// Files hashed by the job that is running, probed once it has finished.
    hashed: Vec<MediaTarget>,
}

impl MediaUi {
    /// Starts the first search at once, on the tools thread. `data_dir` is the folder that holds
    /// the database.
    pub fn new(data_dir: PathBuf, configured: &str) -> Self {
        let mut ui = MediaUi {
            tools: MediaTools::spawn(data_dir.clone()),
            data_dir,
            tool: None,
            notes: Vec::new(),
            detecting: false,
            searched_path: String::new(),
            install: Install::Idle,
            error: None,
            hashed: Vec::new(),
        };
        ui.detect(configured);
        ui
    }

    fn detect(&mut self, configured: &str) {
        self.detecting = true;
        self.searched_path = configured.to_owned();
        self.tools.submit(ToolJob::Detect {
            configured: configured.to_owned(),
        });
    }

    fn installing(&self) -> bool {
        matches!(self.install, Install::Running { .. })
    }

    /// A search or an install is in flight: the window must keep polling for its answer.
    pub fn working(&self) -> bool {
        self.detecting || self.installing()
    }
}

impl AnimaticApp {
    /// Answers of the tools thread. Called once per frame.
    pub(super) fn process_media_events(&mut self, ctx: &egui::Context) {
        let events: Vec<_> = self.media_ui.tools.events.try_iter().collect();
        for event in events {
            match event {
                ToolEvent::Detected(found) => {
                    self.media_ui.detecting = false;
                    self.media_ui.tool = found.tool;
                    self.media_ui.notes = found.notes;
                    // Files hashed while the search was still running (for example paths given
                    // on the command line) were kept; probe them now unless a job is running.
                    if !self.activity.busy {
                        self.media_after_job();
                    }
                }
                ToolEvent::InstallProgress { done, total } => {
                    if let Install::Running { cancelling, .. } = self.media_ui.install {
                        self.media_ui.install = Install::Running {
                            done,
                            total,
                            cancelling,
                        };
                    }
                }
                ToolEvent::Installed(found) => {
                    self.media_ui.install = Install::Idle;
                    self.media_ui.detecting = false;
                    self.media_ui.error = None;
                    if let Some(tool) = &found.tool {
                        self.log(format!(
                            "ffprobe {} is ready ({})",
                            tool.version,
                            tool.path.display()
                        ));
                    }
                    self.media_ui.tool = found.tool;
                    self.media_ui.notes = found.notes;
                }
                ToolEvent::InstallFailed(message) => {
                    self.media_ui.install = Install::Idle;
                    self.error(format!("ffprobe was not installed: {message}"));
                    self.media_ui.error = Some(message);
                }
                ToolEvent::InstallCancelled => {
                    self.media_ui.install = Install::Idle;
                    self.note("ffprobe download cancelled");
                }
            }
            ctx.request_repaint();
        }
    }

    /// A file was hashed: remember it for the probe that follows the job, when the feature is on
    /// and `ffprobe` is available.
    pub(super) fn media_note_hashed(&mut self, local_id: i64, path: &Path) {
        let searching = self.media_ui.detecting && self.media_ui.tool.is_none();
        if !self.settings.media.read_properties || (self.media_ui.tool.is_none() && !searching) {
            return;
        }
        // ffprobe is only given absolute paths (a relative name could be read as an option).
        if let Ok(path) = std::path::absolute(path) {
            self.media_ui.hashed.push(MediaTarget { local_id, path });
        }
    }

    /// A worker job finished: probe the files it hashed, as a job of its own. While the first
    /// search for `ffprobe` is still running the files wait for its answer.
    pub(super) fn media_after_job(&mut self) {
        if self.media_ui.hashed.is_empty() || self.media_ui.detecting {
            return;
        }
        let files = std::mem::take(&mut self.media_ui.hashed);
        if !self.settings.media.read_properties {
            return;
        }
        if let Some(tool) = &self.media_ui.tool {
            self.worker.submit(Job::ReadMedia {
                ffprobe: tool.path.clone(),
                scope: MediaScope::Files(files),
            });
        }
    }

    /// Options were saved: search again when the configured path changed.
    pub(super) fn media_settings_saved(&mut self) {
        let configured = self.settings.media.ffprobe_path.clone();
        if configured != self.media_ui.searched_path
            && !self.media_ui.detecting
            && !self.media_ui.installing()
        {
            self.media_ui.detect(&configured);
        }
    }

    /// The "Media properties" section of Options.
    pub(super) fn media_options_ui(&mut self, ui: &mut egui::Ui) {
        let busy = self.media_ui.detecting || self.media_ui.installing();
        if self.media_ui.detecting {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Looking for ffprobe…");
            });
        } else if let Some(tool) = &self.media_ui.tool {
            ui.label(format!(
                "ffprobe {} ({})",
                tool.version,
                tool.source.describe()
            ));
            ui.label(RichText::new(tool.path.display().to_string()).weak());
        } else {
            ui.colored_label(Color32::YELLOW, "ffprobe was not found.");
        }
        for note in &self.media_ui.notes {
            ui.label(RichText::new(note).weak());
        }
        ui.horizontal(|ui| {
            ui.label("ffprobe path");
            ui.text_edit_singleline(&mut self.settings.media.ffprobe_path)
                .on_hover_text("Optional: an ffprobe executable, or the folder that holds it");
            if ui
                .add_enabled(!busy, egui::Button::new("Detect"))
                .on_hover_text("Search again with this path")
                .clicked()
            {
                let configured = self.settings.media.ffprobe_path.clone();
                self.media_ui.detect(&configured);
            }
        });
        let available = self.media_ui.tool.is_some();
        ui.add_enabled(
            available,
            egui::Checkbox::new(
                &mut self.settings.media.read_properties,
                "Read media properties after hashing",
            ),
        )
        .on_disabled_hover_text("Needs ffprobe: install it below or set its path");
        ui.label(
            RichText::new(
                "Off by default. Reads duration, resolution, codecs and bitrates of each hashed \
                 file once, in a separate step after hashing, and fills the media tokens of the \
                 hash export.",
            )
            .weak(),
        );
        if let Install::Running {
            done,
            total,
            cancelling,
        } = self.media_ui.install
        {
            let fraction = done as f32 / total.max(1) as f32;
            ui.add(egui::ProgressBar::new(fraction).text(format!(
                "Downloading ffprobe: {} of {}",
                format::size_short(done as i64),
                format::size_short(total as i64)
            )));
            if ui
                .add_enabled(!cancelling, egui::Button::new("Cancel download"))
                .clicked()
            {
                self.media_ui.tools.cancel();
                self.media_ui.install = Install::Running {
                    done,
                    total,
                    cancelling: true,
                };
            }
        } else if current_pin().is_some() {
            if ui
                .add_enabled(!busy, egui::Button::new("Install ffprobe…"))
                .on_hover_text("Download FFmpeg's ffprobe into Animatic's data folder")
                .clicked()
            {
                self.media_ui.install = Install::Confirm;
            }
        } else {
            ui.label(
                RichText::new(
                    "A download is only offered for 64-bit Windows and Linux. Install ffprobe \
                     yourself and set its path.",
                )
                .weak(),
            );
        }
        if let Some(error) = &self.media_ui.error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        let can_read = available && self.settings.media.read_properties;
        if ui
            .add_enabled(can_read, egui::Button::new("Read missing properties now"))
            .on_hover_text("Probe the hashed files that have no media properties yet")
            .clicked()
            && let Some(tool) = &self.media_ui.tool
        {
            self.worker.submit(Job::ReadMedia {
                ffprobe: tool.path.clone(),
                scope: MediaScope::Missing,
            });
        }
    }

    /// The confirmation of the install: download size, license, source and what is kept.
    pub(super) fn media_install_window(&mut self, ctx: &egui::Context) {
        if self.media_ui.install != Install::Confirm {
            return;
        }
        let Some(pin) = current_pin() else {
            self.media_ui.install = Install::Idle;
            return;
        };
        let folder = tool::install_dir(&self.media_ui.data_dir);
        let mut go = None;
        let response = egui::Modal::new(egui::Id::unique("install_ffprobe")).show(ctx, |ui| {
            ui.heading("Install ffprobe");
            ui.label(
                "Animatic will download FFmpeg's ffprobe tool, check it against a fixed \
                 SHA-256 checksum and keep only ffprobe and the FFmpeg license in its own \
                 data folder. No administrator rights are needed.",
            );
            ui.add_space(6.0);
            egui::Grid::new("install_ffprobe_facts")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Build");
                    ui.label(format!("FFmpeg {} {}", pin.version, pin.build));
                    ui.end_row();
                    ui.label("Download size");
                    ui.label(format::size_short(pin.size as i64));
                    ui.end_row();
                    ui.label("License");
                    ui.label(pin.license);
                    ui.end_row();
                    ui.label("Source");
                    ui.hyperlink_to(pin.page, pin.page);
                    ui.end_row();
                    ui.label("Install folder");
                    ui.label(folder.display().to_string());
                    ui.end_row();
                });
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "The download contacts github.com and GitHub's download servers, not \
                     AniDB, and sends nothing but the request itself (your IP address and \
                     \"Animatic/version\"). It does not depend on offline mode.",
                )
                .weak(),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Download and install").clicked() {
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
        match go {
            Some(true) => {
                self.media_ui.error = None;
                self.media_ui.install = Install::Running {
                    done: 0,
                    total: pin.size,
                    cancelling: false,
                };
                self.media_ui.tools.submit(ToolJob::Install {
                    configured: self.settings.media.ffprobe_path.clone(),
                });
                self.note("Downloading ffprobe…");
            }
            Some(false) => self.media_ui.install = Install::Idle,
            None => {}
        }
    }
}
