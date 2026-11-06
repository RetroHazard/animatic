//! Application side of the system tray: the per-frame hook and the Options section.

use eframe::egui::{self, Color32, RichText};

use super::AnimaticApp;
use crate::tray::{Availability, TrayController, TrayRequest, unavailable_reason};

impl AnimaticApp {
    /// Once per frame: puts the icon up or removes it as the settings say, hides a minimized
    /// window to the tray and carries out what the tray menu asked for.
    pub(super) fn tray_frame(&mut self, ctx: &egui::Context) {
        let allowed = self.queue_run_allowed();
        let ui_settings = &self.settings.ui;
        let (show_icon, minimize) = (ui_settings.show_tray_icon, ui_settings.minimize_to_tray);
        let tray = self.tray.get_or_insert_with(|| TrayController::new(ctx));
        for request in tray.update(ctx, show_icon, minimize, allowed) {
            match request {
                // The menu item is only a request: the same gating as Tools applies now.
                TrayRequest::RunQueue if self.queue_run_allowed() => {
                    self.worker
                        .submit(super::Job::ProcessQueue(self.settings.anidb.clone()));
                }
                TrayRequest::RunQueue => {}
            }
        }
    }

    /// Brings the main window back, also from the tray, and focuses it (a launch that forwards
    /// files to this instance does this).
    pub(super) fn restore_main_window(&mut self, ctx: &egui::Context) {
        match &mut self.tray {
            Some(tray) => tray.show_window(ctx),
            None => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
    }

    /// Options → System tray.
    pub(super) fn tray_options_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("System tray");
        let availability = self
            .tray
            .as_ref()
            .map(|tray| tray.availability().clone())
            .unwrap_or(Availability::Checking);
        let usable = unavailable_reason(&availability).is_none();
        ui.add_enabled_ui(usable, |ui| {
            ui.checkbox(&mut self.settings.ui.show_tray_icon, "Show tray icon");
            ui.add_enabled_ui(self.settings.ui.show_tray_icon, |ui| {
                ui.checkbox(&mut self.settings.ui.minimize_to_tray, "Minimize to tray")
                    .on_hover_text(
                        "Minimizing hides the window; click the tray icon or choose Show \
                         Animatic to bring it back. Closing the window still quits.",
                    );
            });
        });
        match &availability {
            Availability::Unavailable(reason) => {
                ui.colored_label(
                    Color32::YELLOW,
                    format!(
                        "No system tray is available here: {reason}. The tray icon and minimize \
                         to tray stay off, so the window is never hidden without a way back."
                    ),
                );
            }
            Availability::Checking => {
                ui.label(RichText::new("Checking for a system tray…").weak());
            }
            Availability::Available => {
                ui.label(
                    RichText::new(
                        "Minimize to tray needs the icon. Closing the window still quits.",
                    )
                    .weak(),
                );
            }
        }
    }
}
