//! The pending hash queue as the user sees it (roadmap M5.4): how many files wait, which ones,
//! which failed and why, and the **Resume hashing**, **Retry failed** and **Clear hash queue**
//! actions. The numbers come from the library projection, which the worker refreshes whenever
//! the queue changes, so nothing here reads the database.

use animatic_core::hash_queue::{HashQueueEntry, HashState};
use eframe::egui::{self, Color32, RichText};

use super::AnimaticApp;
use crate::worker::Job;

impl AnimaticApp {
    /// Draw the queue summary and its actions; nothing at all when the queue is empty. With
    /// `compact` the list of files is left out (dashboard).
    pub(super) fn hash_queue_panel(&mut self, ui: &mut egui::Ui, compact: bool) {
        let pending = self.projection.hash_queue.pending;
        let failed = self.projection.hash_queue.failed;
        if pending == 0 && failed == 0 {
            return;
        }
        let blocker = self.busy_reason();
        let mut resume = false;
        let mut retry = false;
        let mut clear = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Hash queue").strong());
                ui.label(match (pending, failed) {
                    (p, 0) => format!("{p} file(s) waiting to be hashed"),
                    (0, f) => format!("{f} file(s) failed"),
                    (p, f) => format!("{p} file(s) waiting to be hashed, {f} failed"),
                });
                if pending > 0 && !self.settings.hashing.autostart {
                    ui.weak("Hashing does not resume on its own (Options, Hashing).");
                }
            });
            ui.horizontal_wrapped(|ui| {
                let button = |ui: &mut egui::Ui, enabled: bool, text: &str, hint: &str| {
                    let response =
                        ui.add_enabled(enabled && blocker.is_none(), egui::Button::new(text));
                    match blocker {
                        Some(reason) if enabled => response.on_disabled_hover_text(reason),
                        _ => response.on_hover_text(hint).on_disabled_hover_text(hint),
                    }
                    .clicked()
                };
                resume = button(
                    ui,
                    pending > 0,
                    "Resume hashing",
                    "Hash the waiting files in order. Cancel stops and keeps the rest queued",
                );
                retry = button(
                    ui,
                    failed > 0,
                    "Retry failed",
                    "Queue the failed files again and hash them",
                );
                clear = button(
                    ui,
                    true,
                    "Clear hash queue",
                    "Forget every waiting and failed file. Hashed files stay in the library",
                );
            });
            if !compact {
                self.hash_queue_list(ui);
            }
        });
        let pause_between = self.settings.file_managers.file_pause();
        if resume || retry {
            self.worker.submit(Job::ResumeHashQueue {
                retry_failed: retry,
                pause_between,
            });
        } else if clear {
            self.worker.submit(Job::ClearHashQueue);
        }
    }

    fn hash_queue_list(&self, ui: &mut egui::Ui) {
        let view = &self.projection.hash_queue;
        let listed = view.entries.len();
        egui::CollapsingHeader::new("Files in the hash queue")
            .id_salt("hash_queue_list")
            .default_open(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("hash_queue_scroll")
                    .max_height(150.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for entry in &view.entries {
                            entry_row(ui, entry);
                        }
                        let unlisted = (view.pending + view.failed).saturating_sub(listed);
                        if unlisted > 0 {
                            ui.weak(format!("… and {unlisted} more"));
                        }
                    });
            });
    }
}

fn entry_row(ui: &mut egui::Ui, entry: &HashQueueEntry) {
    ui.horizontal(|ui| match (&entry.state, &entry.reason) {
        (HashState::Failed, reason) => {
            ui.colored_label(Color32::LIGHT_RED, "Failed");
            ui.label(entry.path.display().to_string());
            ui.weak(reason.as_deref().unwrap_or("unknown reason"));
        }
        (HashState::Queued, _) => {
            ui.weak(entry.source.label());
            ui.label(entry.path.display().to_string());
        }
    });
}
