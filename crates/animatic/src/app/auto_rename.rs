//! Opt-in automatic rename per managed folder (roadmap M6.3), the application side.
//!
//! Planning and the once-per-state bookkeeping live in `animatic_core::auto_rename`; the worker's
//! `Job::AutoRename` carries a pass out through the same verified move as a manual rename. This
//! module decides *when* a pass is submitted and owns the two pieces of UI: the per-folder mode
//! control in Options and the dry-run preview that must be confirmed before a folder is turned
//! **On**.
//!
//! A pass is due when something happened that can make a file renameable: a file was hashed
//! (a scan found an already identified file), an AniDB reply changed the library (the identify
//! reply of a queued file was applied), a legacy import finished, or the folders' modes, paths or
//! the pattern changed. It is submitted only while the worker is idle, no import, restore or
//! settings review is open, and the Options window is closed (half-edited settings never rename).

use animatic_core::settings::{AutoRename, FileManager};

use super::*;

/// Dry-run preview shown before a managed folder is turned on.
pub(super) struct EnableRequest {
    folder: usize,
    name: String,
    /// Answers to older plan requests are ignored.
    request: u64,
    /// Plans of the folder's identified files; `None` while the worker is planning.
    rows: Option<Vec<BatchRow>>,
}

#[derive(Default)]
pub(super) struct AutoRenameState {
    /// Something that can make a file renameable happened since the last submitted pass.
    due: bool,
    /// The settings a pass depends on, as of the last time they were looked at.
    signature: String,
    pub(super) enable: Option<EnableRequest>,
}

impl AutoRenameState {
    /// A file was hashed, the library changed or an import finished.
    pub(super) fn mark_due(&mut self) {
        self.due = true;
    }

    /// The answer to a plan request. Taken for the preview when it is its own; otherwise the
    /// rows are handed back.
    pub(super) fn accept_plan(
        &mut self,
        request: u64,
        rows: Vec<BatchRow>,
    ) -> Option<Vec<BatchRow>> {
        match self.enable.as_mut() {
            Some(enable) if enable.request == request => {
                enable.rows = Some(rows);
                None
            }
            _ => Some(rows),
        }
    }
}

fn mode_label(mode: AutoRename) -> &'static str {
    match mode {
        AutoRename::Off => "Off",
        AutoRename::DryRun => "Dry run (log only)",
        AutoRename::On => "On",
    }
}

/// The per-folder **Auto rename** selector. Off and Dry run take effect at once; choosing On is
/// returned instead, so the caller can show the preview first. `true`: On was chosen.
pub(super) fn mode_control(ui: &mut egui::Ui, index: usize, manager: &mut FileManager) -> bool {
    let mut chose_on = false;
    ui.horizontal(|ui| {
        ui.label("Auto rename");
        egui::ComboBox::from_id_salt(("manager-auto-rename", index))
            .selected_text(mode_label(manager.auto_rename))
            .show_ui(ui, |ui| {
                for mode in [AutoRename::Off, AutoRename::DryRun] {
                    if ui
                        .selectable_label(manager.auto_rename == mode, mode_label(mode))
                        .clicked()
                    {
                        manager.auto_rename = mode;
                    }
                }
                if ui
                    .selectable_label(
                        manager.auto_rename == AutoRename::On,
                        mode_label(AutoRename::On),
                    )
                    .clicked()
                    && manager.auto_rename != AutoRename::On
                {
                    chose_on = true;
                }
            })
            .response
            .on_hover_text(
                "Off: never rename. Dry run: log the renames that would happen, move nothing. \
                 On: rename identified files in this folder with the current pattern; choosing \
                 On shows what would be renamed now and asks you to confirm.",
            );
    });
    chose_on
}

impl AnimaticApp {
    /// The settings a pass depends on (only folders that are not off).
    fn auto_rename_signature(&self) -> String {
        let mut signature = String::new();
        for m in &self.settings.file_managers.managers {
            if !m.auto_rename.is_off() {
                signature.push_str(&format!(
                    "{}|{}|{}|{};",
                    m.path,
                    m.base_path,
                    m.include_subdirs,
                    m.auto_rename.as_str()
                ));
            }
        }
        signature.push_str(&self.settings.renaming_pattern);
        signature
    }

    /// Submit an automatic rename pass when one is due and nothing prevents it; see the module
    /// documentation. Cheap when no folder has automatic rename switched on.
    pub(super) fn pump_auto_rename(&mut self, ctx: &egui::Context) {
        if self.settings_open {
            return;
        }
        let signature = self.auto_rename_signature();
        if signature != self.auto_rename.signature {
            self.auto_rename.signature = signature;
            self.auto_rename.due = true;
        }
        if !self.auto_rename.due {
            return;
        }
        let managers = &self.settings.file_managers;
        if managers.managers.iter().all(|m| m.auto_rename.is_off()) {
            self.auto_rename.due = false;
            return;
        }
        if self.scan_blocker().is_none()
            && self.auto_rename.enable.is_none()
            && self.worker.submit_if_idle(Job::AutoRename {
                managers: managers.clone(),
                pattern: self.settings.renaming_pattern.clone(),
            })
        {
            self.auto_rename.due = false;
        } else {
            // Nothing wakes the UI when the blocker goes away, so look again soon.
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    /// Plan, on the projection thread, what turning folder `index` **On** would rename now.
    pub(super) fn begin_auto_rename_enable(&mut self, index: usize) {
        let Some(manager) = self.settings.file_managers.managers.get(index) else {
            return;
        };
        let name = if manager.name.is_empty() {
            manager.path.clone()
        } else {
            manager.name.clone()
        };
        let files = animatic_core::auto_rename::folder_candidates(
            &self.projection.local,
            &self.settings.file_managers,
            index,
        );
        self.rename_requests += 1;
        let request = self.rename_requests;
        let rows = files.is_empty().then(Vec::new);
        if rows.is_none() {
            self.worker.submit(Job::PlanRenames {
                request,
                pattern: self.settings.renaming_pattern.clone(),
                files,
            });
        }
        self.auto_rename.enable = Some(EnableRequest {
            folder: index,
            name,
            request,
            rows,
        });
    }

    /// The dry-run preview and its explicit confirmation. Nothing is renamed by it: turning the
    /// folder on only changes its mode; the pass that follows is the usual one.
    pub(super) fn auto_rename_enable_window(&mut self, ctx: &egui::Context) {
        let Some(enable) = self.auto_rename.enable.as_ref() else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("auto_rename_enable")).show(ctx, |ui| {
            ui.set_width(720.0);
            ui.heading(format!("Turn on automatic rename for “{}”?", enable.name));
            ui.add_space(6.0);
            ui.label(
                "From now on, identified files in this folder are renamed with the current \
                 pattern when AniDB identifies them or a scan finds them. A file whose new \
                 name is taken, or whose rename is blocked, is skipped and logged; nothing is \
                 overwritten.",
            );
            ui.add_space(6.0);
            match &enable.rows {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Planning what would be renamed now…");
                    });
                }
                Some(rows) => {
                    let valid = rows.iter().filter(|r| r.plan.can_apply()).count();
                    let named = rows.iter().filter(|r| r.plan.already_named()).count();
                    let blocked = rows.len() - valid - named;
                    ui.label(format!(
                        "Dry run: {valid} of {} identified files would be renamed now, \
                         {blocked} blocked, {named} already named as the pattern says. \
                         Nothing has been renamed.",
                        rows.len()
                    ));
                    ui.add_space(4.0);
                    let line = ui.text_style_height(&egui::TextStyle::Body);
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .auto_shrink([false, true])
                        .show_rows(ui, line, rows.len(), |ui, range| {
                            for row in &rows[range] {
                                batch_rename_line(ui, row, false, None);
                            }
                        });
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        enable.rows.is_some(),
                        egui::Button::new("Turn on automatic rename"),
                    )
                    .clicked()
                {
                    confirm = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if confirm {
            if let Some(enable) = self.auto_rename.enable.take()
                && let Some(manager) = self.settings.file_managers.managers.get_mut(enable.folder)
            {
                manager.auto_rename = AutoRename::On;
                self.log(format!(
                    "Automatic rename is on for “{}”; save the options to keep it",
                    enable.name
                ));
            }
        } else if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.auto_rename.enable = None;
        }
    }
}
