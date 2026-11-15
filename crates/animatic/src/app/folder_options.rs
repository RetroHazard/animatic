//! **Options → Managed folders**: one collapsible section per folder rule, grouped by what the
//! controls act on (the folder, scans, renames, MyList), then the scan schedule.
//!
//! Three kinds of rule share `FileManager`: the Default rule (no folder; the policy for files
//! outside every folder rule), the built-in `\\` network rule (every UNC path) and folder rules.
//! Controls that cannot affect a kind are not shown for it. Any rule can be removed, after an
//! inline confirmation, except the last one; a removed built-in rule can be restored.

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

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// The Default rule: no folder, applies to files outside every folder rule.
    Fallback,
    /// The built-in `\\` rule.
    Network,
    Folder,
}

fn kind(manager: &FileManager) -> Kind {
    if manager.default {
        Kind::Fallback
    } else if manager.is_network_builtin() {
        Kind::Network
    } else {
        Kind::Folder
    }
}

/// Section header: the name, and for a folder rule its path, so collapsed rules can be told
/// apart.
fn header(manager: &FileManager) -> String {
    let name = if manager.name.is_empty() {
        "Unnamed folder"
    } else {
        manager.name.as_str()
    };
    match kind(manager) {
        Kind::Fallback => format!("{name} — files outside every folder"),
        Kind::Folder if !manager.path.trim().is_empty() => format!("{name} — {}", manager.path),
        _ => name.to_owned(),
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
    last_rule: bool,
) -> RuleOutcome {
    let mut outcome = RuleOutcome::default();
    let kind = kind(manager);
    match kind {
        Kind::Fallback => {
            ui.label(
                RichText::new(
                    "Applies to files outside every folder rule, such as files added by hand. \
                     It has no folder, so scans never use it.",
                )
                .weak(),
            );
        }
        Kind::Network => {
            ui.label(
                RichText::new(
                    "Applies to every network path (\\\\server\\share\\…) that no folder rule \
                     covers.",
                )
                .weak(),
            );
        }
        Kind::Folder => {}
    }

    group_heading(ui, "Folder");
    field(ui, "Name", &mut manager.name);
    if kind == Kind::Folder {
        path_field(
            ui,
            "Path",
            &mut manager.path,
            "Choose the folder of this rule",
        );
        ui.checkbox(&mut manager.include_subdirs, "Include subfolders")
            .on_hover_text("Off: only files directly inside the folder belong to this rule.");
    }

    if kind != Kind::Fallback {
        group_heading(ui, "When a scan finds files");
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut manager.auto_hash, "Hash new files");
            ui.checkbox(&mut manager.auto_rehash, "Rehash changed files")
                .on_hover_text("Hash a known file again when its modification time changed.");
            ui.checkbox(&mut manager.exclude_system, "Skip system files")
                .on_hover_text(
                    "Scans skip files and folders with the Windows system attribute. Linux has \
                     no such attribute, so this has no effect there.",
                );
        });
        ui.checkbox(
            &mut manager.auto_remove,
            style::destructive(ui, "Remove missing files from the library"),
        )
        .on_hover_text(
            "Removes Animatic's record of a file that is confirmed gone while the folder is \
             reachable. Never deletes media files; an unreachable drive or share keeps every \
             record.",
        );
    }

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
    if kind != Kind::Fallback {
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
    }

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
        ui.label(match kind {
            Kind::Fallback => {
                "Remove the Default rule? Files outside your folders will no longer be added \
                 to MyList, and renaming them keeps them in their folder."
            }
            Kind::Network => {
                "Remove the network rule? Network files outside your folders will use the \
                 Default rule."
            }
            Kind::Folder => "Remove this folder rule? Files in the folder stay in the library.",
        });
        ui.horizontal(|ui| {
            if ui.button(style::destructive(ui, "Remove")).clicked() {
                outcome.remove = true;
            }
            if ui.button("Cancel").clicked() {
                state.confirm_remove = None;
            }
        });
    } else if ui
        .add_enabled(!last_rule, egui::Button::new("Remove rule…"))
        .on_disabled_hover_text("The last rule cannot be removed. Add another rule first.")
        .clicked()
    {
        state.confirm_remove = Some(index);
    }
    outcome
}

impl AnimaticApp {
    pub(super) fn options_folders(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        let mut enable_auto_rename = None;
        let managers = &mut self.settings.file_managers.managers;
        let last_rule = managers.len() <= 1;
        for (index, manager) in managers.iter_mut().enumerate() {
            // Keyed by position, not by the header: typing in Name must not close the section.
            egui::CollapsingHeader::new(header(manager))
                .id_salt(("options-managed-folder", index))
                .show(ui, |ui| {
                    let outcome = rule_ui(ui, index, manager, &mut self.folder_options, last_rule);
                    if outcome.remove {
                        remove = Some(index);
                    }
                    if outcome.enable_auto_rename {
                        enable_auto_rename = Some(index);
                    }
                });
        }
        if let Some(index) = remove {
            if managers.len() > 1 {
                managers.remove(index);
            }
            // Positions moved: per-rule state no longer points at the same rules.
            self.folder_options = FolderOptionsState::default();
        }
        if let Some(index) = enable_auto_rename {
            self.begin_auto_rename_enable(index);
        }
        ui.horizontal(|ui| {
            if ui.button("Add folder rule").clicked() {
                self.settings.file_managers.managers.push(FileManager {
                    name: "New folder".into(),
                    ..FileManager::default()
                });
            }
            let missing = self.settings.file_managers.missing_builtins();
            if !missing.is_empty() {
                let names: Vec<&str> = missing.iter().map(|m| m.name.as_str()).collect();
                if ui
                    .button("Restore built-in rules")
                    .on_hover_text(format!("Adds back: {}", names.join(", ")))
                    .clicked()
                {
                    for rule in missing {
                        if rule.default {
                            self.settings.file_managers.managers.insert(0, rule);
                        } else {
                            self.settings.file_managers.managers.push(rule);
                        }
                    }
                    self.folder_options = FolderOptionsState::default();
                }
            }
        });
        self.options_folder_scanning(ui);
    }
}
