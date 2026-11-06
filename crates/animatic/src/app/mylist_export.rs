//! Full-account MyList import through a MyList export: **Tools → Request MyList export…** and
//! **File → Import MyList export…**.
//!
//! No AniDB API returns a whole MyList, and walking it entry by entry is the bulk extraction
//! AniDB forbids. AniDB instead prepares an export on request (UDP `MYLISTEXPORT`, which can take
//! up to 24 hours), announces it as a message and offers the file on its web export page. The
//! request is a queued command like any other (`Command::MylistExport`: sent only when the
//! command queue runs, through the shared UDP limiter, at most once a day and never scheduled);
//! the file is imported by a worker job (`Job::ImportMylistExport`), which validates all of it
//! before it writes anything. Pickers and dialogs run here on the UI thread.

use animatic_anidb::commands::MYLIST_EXPORT_PAGE;
use animatic_core::limits::mylist_export_wait_message;
use animatic_core::sync::Command;

use super::*;

/// State of the request dialog.
#[derive(Default)]
pub(super) struct ExportUi {
    open: bool,
    /// When a new request is allowed again, read when the dialog opened.
    wait_until: Option<i64>,
}

enum Action {
    Request,
    OpenPage,
    Cancel,
}

impl AnimaticApp {
    /// Open the request dialog.
    pub(super) fn open_export_request(&mut self) {
        self.export_ui.wait_until = self.export_wait();
        self.export_ui.open = true;
    }

    fn export_wait(&mut self) -> Option<i64> {
        match self.db.mylist_export_next_allowed(format::now()) {
            Ok(until) => until,
            Err(e) => {
                // Without the saved attempt the request is allowed; the executor checks again.
                log::warn!("cannot read the MyList export attempt time: {e}");
                None
            }
        }
    }

    pub(super) fn mylist_export_window(&mut self, ctx: &egui::Context) {
        if !self.export_ui.open {
            return;
        }
        let wait_until = self.export_ui.wait_until;
        let mut open = true;
        let mut action = None;
        egui::Window::new("Request MyList export")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_max_width(480.0);
                ui.label(
                    "AniDB can prepare an export of your whole MyList, the supported way to \
                     load all of it here. Preparing it may take up to 24 hours.",
                );
                ui.add_space(6.0);
                ui.label(
                    "1. Request the export. The request is queued and sent when the command \
                     queue runs; AniDB prepares one export at a time.",
                );
                ui.label(
                    "2. AniDB announces the finished export as a message in your AniDB inbox.",
                );
                ui.label(
                    "3. Download the file from AniDB's export page in your browser (AniDB \
                     offers no way to fetch it from here).",
                );
                ui.label("4. Choose File → Import MyList export… and pick the downloaded file.");
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "Animatic requests the \"xml-plain-new\" template and never requests an \
                         export by itself.",
                    )
                    .weak(),
                );
                if let Some(until) = wait_until {
                    ui.add_space(4.0);
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        mylist_export_wait_message(until),
                    );
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(wait_until.is_none(), egui::Button::new("Request export"))
                        .on_disabled_hover_text("An export was requested recently.")
                        .clicked()
                    {
                        action = Some(Action::Request);
                    }
                    if ui.button("Open export page").clicked() {
                        action = Some(Action::OpenPage);
                    }
                    if ui
                        .button("Cancel pending export")
                        .on_hover_text("Ask AniDB to cancel the export it has queued")
                        .clicked()
                    {
                        action = Some(Action::Cancel);
                    }
                });
            });
        match action {
            Some(Action::Request) => {
                match self.db.mylist_export_next_allowed(format::now()) {
                    Ok(Some(until)) => self.log(mylist_export_wait_message(until)),
                    _ => {
                        self.queue_command(Command::MylistExport);
                        self.log(
                            "Queued a MyList export request; run the command queue to send it. \
                             AniDB may take up to 24 hours and announces the export as a message",
                        );
                    }
                }
                open = false;
            }
            Some(Action::OpenPage) => {
                if let Err(e) = open::that(MYLIST_EXPORT_PAGE) {
                    self.error(format!("Could not open the AniDB export page: {e}"));
                }
            }
            Some(Action::Cancel) => {
                self.queue_command(Command::MylistExportCancel);
                self.log(
                    "Queued the cancellation of the pending MyList export; run the command queue \
                     to send it",
                );
                open = false;
            }
            None => {}
        }
        self.export_ui.open = open;
    }

    /// **File → Import MyList export…**: pick the downloaded file and import it on the worker.
    pub(super) fn import_mylist_export(&mut self) {
        if let Some(reason) = self.library_task_blocker() {
            self.error(format!("MyList export not imported: {reason}"));
            return;
        }
        let Some(source) = rfd::FileDialog::new()
            .set_title("Choose the MyList export downloaded from AniDB")
            .add_filter("MyList export", &["tgz", "gz", "xml", "zip", "tar"])
            .add_filter("All files", &["*"])
            .pick_file()
        else {
            return;
        };
        self.worker.submit(Job::ImportMylistExport { source });
    }
}
