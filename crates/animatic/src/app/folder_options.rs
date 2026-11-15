//! **Options → Managed folders**: one collapsible section per folder rule, grouped by what the
//! controls act on (the folder, scans, renames, MyList), then the scan schedule.
//!
//! Folder rules are optional and all alike: a file outside every rule is only identified (not
//! added to MyList, renamed in its own folder, never scanned). Any rule can be removed after an
//! inline confirmation.

use std::collections::HashSet;
use std::path::Path;

use animatic_core::library::mylist_state_name;
use animatic_core::settings::{FileManager, MYLIST_STATES};
use eframe::egui::{self, RichText};

use super::{AnimaticApp, FIELD_WIDTH, auto_rename, field, field_label, style};

/// Options-window state of this page; dropped when Options closes.
#[derive(Default)]
pub(super) struct FolderOptionsState {
    /// Rules whose **Move renamed files** box was ticked while the destination is still empty.
    moving: HashSet<usize>,
    /// The rule whose removal waits for confirmation.
    confirm_remove: Option<usize>,
}

/// Section header: the name and the path, so collapsed rules can be told apart.
fn header(manager: &FileManager) -> String {
    let name = if manager.name.is_empty() {
        "Unnamed folder"
    } else {
        manager.name.as_str()
    };
    if manager.path.trim().is_empty() {
        name.to_owned()
    } else {
        format!("{name} — {}", manager.path)
    }
}

/// A small bold heading of a group inside a rule.
fn group_heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).strong());
}

/// A path [`field`] with a **Browse…** folder picker that starts in the current folder.
fn path_field(ui: &mut egui::Ui, label: &str, value: &mut String, title: &str) {
    ui.horizontal(|ui| {
        let label = field_label(ui, label);
        ui.add(egui::TextEdit::singleline(value).desired_width(FIELD_WIDTH))
            .labelled_by(label.id);
        if ui.button("Browse…").clicked() {
            let mut dialog = rfd::FileDialog::new().set_title(title);
            if !value.trim().is_empty() && Path::new(value.trim()).is_dir() {
                dialog = dialog.set_directory(value.trim());
            }
            if let Some(folder) = dialog.pick_folder() {
                *value = folder.to_string_lossy().into_owned();
            }
        }
    });
}

/// What a rule's section asked for this frame.
#[derive(Default)]
struct RuleOutcome {
    remove: bool,
    enable_auto_rename: bool,
}

fn rule_ui(
    ui: &mut egui::Ui,
    index: usize,
    manager: &mut FileManager,
    state: &mut FolderOptionsState,
) -> RuleOutcome {
    let mut outcome = RuleOutcome::default();
    group_heading(ui, "Folder");
    field(ui, "Name", &mut manager.name);
    path_field(
        ui,
        "Path",
        &mut manager.path,
        "Choose the folder of this rule",
    );
    ui.checkbox(&mut manager.include_subdirs, "Include subfolders")
        .on_hover_text("Off: only files directly inside the folder belong to this rule.");

    group_heading(ui, "When a scan finds files");
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut manager.auto_hash, "Hash new files");
        ui.checkbox(&mut manager.auto_rehash, "Rehash changed files")
            .on_hover_text("Hash a known file again when its modification time changed.");
        ui.checkbox(&mut manager.exclude_system, "Skip system files")
            .on_hover_text(
                "Scans skip files and folders with the Windows system attribute. Linux has no \
                 such attribute, so this has no effect there.",
            );
    });
    ui.checkbox(
        &mut manager.auto_remove,
        style::destructive(ui, "Remove missing files from the library"),
    )
    .on_hover_text(
        "Removes Animatic's record of a file that is confirmed gone while the folder is \
         reachable. Never deletes media files; an unreachable drive or share keeps every record.",
    );

    group_heading(ui, "Renaming");
    let mut moving = !manager.base_path.is_empty() || state.moving.contains(&index);
    if ui
        .checkbox(&mut moving, "Move renamed files to another folder")
        .on_hover_text(
            "Off: files are renamed where they are. On: renamed files are moved under the \
             destination (BASEPATH in rename patterns).",
        )
        .changed()
    {
        if moving {
            state.moving.insert(index);
        } else {
            state.moving.remove(&index);
            manager.base_path.clear();
        }
    }
    if moving {
        path_field(
            ui,
            "Destination",
            &mut manager.base_path,
            "Choose where renamed files go",
        );
    }
    if auto_rename::mode_control(ui, index, manager) {
        outcome.enable_auto_rename = true;
    }
    ui.checkbox(
        &mut manager.clear_empty_folders,
        style::destructive(ui, "Remove folders a rename leaves empty"),
    )
    .on_hover_text(
        "After Animatic renames or moves a file out of a subfolder, remove the folder (and \
         emptied parents) if nothing at all is left in it. The rule's own folder is never \
         removed.",
    );

    group_heading(ui, "MyList");
    ui.checkbox(
        &mut manager.mylist_auto_add,
        "Add identified files to MyList",
    )
    .on_hover_text("Added when the queue runs online, with the values below.");
    ui.add_enabled_ui(manager.mylist_auto_add, |ui| {
        ui.horizontal(|ui| {
            ui.checkbox(&mut manager.mylist_watched_check, "Set watched");
            if manager.mylist_watched_check {
                ui.checkbox(&mut manager.mylist_watched, "Watched");
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut manager.mylist_state_check, "Set state");
            ui.add_enabled_ui(manager.mylist_state_check, |ui| {
                egui::ComboBox::from_id_salt(("manager-state", index))
                    .selected_text(mylist_state_name(manager.mylist_state))
                    .show_ui(ui, |ui| {
                        for (id, name) in MYLIST_STATES.iter().enumerate() {
                            ui.selectable_value(&mut manager.mylist_state, id as i64, *name);
                        }
                    });
            });
        });
        for (check, value, label) in [
            (
                &mut manager.mylist_storage_check,
                &mut manager.mylist_storage,
                "Set storage",
            ),
            (
                &mut manager.mylist_source_check,
                &mut manager.mylist_source,
                "Set source",
            ),
            (
                &mut manager.mylist_other_check,
                &mut manager.mylist_other,
                "Set other",
            ),
        ] {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(super::LABEL_WIDTH, ui.spacing().interact_size.y),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_width(super::LABEL_WIDTH);
                        ui.checkbox(check, label);
                    },
                );
                let on = *check;
                ui.add_enabled(
                    on,
                    egui::TextEdit::singleline(value).desired_width(FIELD_WIDTH),
                );
            });
        }
    });

    ui.add_space(6.0);
    if state.confirm_remove == Some(index) {
        ui.label(
            "Remove this folder rule? Files in the folder stay in the library; files no rule \
             covers are only identified.",
        );
        ui.horizontal(|ui| {
            if ui.button(style::destructive(ui, "Remove")).clicked() {
                outcome.remove = true;
            }
            if ui.button("Cancel").clicked() {
                state.confirm_remove = None;
            }
        });
    } else if ui.button("Remove rule…").clicked() {
        state.confirm_remove = Some(index);
    }
    outcome
}

impl AnimaticApp {
    pub(super) fn options_folders(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        let mut enable_auto_rename = None;
        let managers = &mut self.settings.file_managers.managers;
        if managers.is_empty() {
            ui.label(
                RichText::new(
                    "No folder rules. Add one to scan a folder, add its files to MyList or move \
                     renamed files. Files outside every rule are not added to MyList, and \
                     renaming one keeps it in its folder.",
                )
                .weak(),
            );
        }
        for (index, manager) in managers.iter_mut().enumerate() {
            // Keyed by position, not by the header: typing in Name must not close the section.
            egui::CollapsingHeader::new(header(manager))
                .id_salt(("options-managed-folder", index))
                .show(ui, |ui| {
                    let outcome = rule_ui(ui, index, manager, &mut self.folder_options);
                    if outcome.remove {
                        remove = Some(index);
                    }
                    if outcome.enable_auto_rename {
                        enable_auto_rename = Some(index);
                    }
                });
        }
        if let Some(index) = remove {
            managers.remove(index);
            // Positions moved: per-rule state no longer points at the same rules.
            self.folder_options = FolderOptionsState::default();
        }
        if let Some(index) = enable_auto_rename {
            self.begin_auto_rename_enable(index);
        }
        if ui.button("Add folder rule").clicked() {
            self.settings.file_managers.managers.push(FileManager {
                name: "New folder".into(),
                ..FileManager::default()
            });
        }
        self.options_folder_scanning(ui);
    }
}
