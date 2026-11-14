//! **Remove from list** on Hashed files: the explicit, confirmed removal of Animatic's records of
//! hashed files (path, hash and media properties). It is a user action, not automatic removal
//! (AGENTS rule 11): media files on disk and MyList entries are never touched, and a later scan
//! or **Add files** brings a file back. The removal itself is `Job::RemoveLocalFiles`, one
//! transaction on the worker.

use eframe::egui;

use super::{AnimaticApp, dialog};
use crate::style;
use crate::worker::Job;

impl AnimaticApp {
    /// Ask to remove the selected files from the list. Nothing happens without a selection.
    pub(super) fn open_remove_files(&mut self) {
        let ids: Vec<i64> = self.selected_locals().into_iter().collect();
        if !ids.is_empty() {
            self.remove_files_confirm = Some(ids);
        }
    }

    /// The confirmation window; Escape and Cancel close it without removing anything.
    pub(super) fn remove_files_window(&mut self, ctx: &egui::Context) {
        let Some(ids) = self.remove_files_confirm.as_ref() else {
            return;
        };
        let count = ids.len();
        let blocker = self.busy_reason();
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        let shown = dialog::window(ctx, "Remove from list")
            .open(&mut open)
            .resizable(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.label(format!(
                    "Remove {} from Animatic's list of hashed files?",
                    style::plural(count, "file", "files")
                ));
                ui.add_space(6.0);
                ui.label(
                    "Only Animatic's records of the files are removed: their hash, path and \
                     media properties.",
                );
                ui.label("The media files on disk are not touched.");
                ui.label(
                    "MyList entries, local and on AniDB, are not touched either, and neither \
                     are queued MyList changes.",
                );
                ui.label(
                    "A later folder scan or Add files lists a file again (it is hashed again).",
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let label = style::destructive(ui, "Remove from list");
                    let button = ui.add_enabled(blocker.is_none(), egui::Button::new(label));
                    if let Some(reason) = blocker {
                        button.on_disabled_hover_text(reason);
                    } else if button.clicked() {
                        confirm = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        let escape = dialog::escape_closes(ctx, shown.as_ref());
        if confirm {
            if let Some(ids) = self.remove_files_confirm.take() {
                self.worker.submit(Job::RemoveLocalFiles { ids });
            }
        } else if cancel || escape || !open {
            self.remove_files_confirm = None;
        }
    }

    /// The records of `ids` are gone: drop them from the selection and from a rename preview,
    /// and log the result. The library refresh follows from `Event::Finished`.
    pub(super) fn local_files_removed(&mut self, ids: &[i64], removed: usize) {
        let gone: std::collections::BTreeSet<i64> = ids.iter().copied().collect();
        self.mylist_ui.marked_locals.retain(|id| !gone.contains(id));
        if self.selected_local.is_some_and(|id| gone.contains(&id)) {
            self.selected_local = None;
        }
        if self
            .rename_preview
            .as_ref()
            .is_some_and(|preview| gone.contains(&preview.id))
        {
            self.rename_preview = None;
        }
        self.log(format!(
            "Removed {} from the list",
            style::plural(removed, "file", "files")
        ));
    }
}
