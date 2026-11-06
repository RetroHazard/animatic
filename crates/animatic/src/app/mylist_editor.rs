//! MyList management in the desktop application: the entry editor, **Add to MyList…**,
//! **Remove from MyList…**, the **Refresh** actions and the AniDB account figures on the
//! overview.
//!
//! Every change is stored in the command queue first (`Db::queue_edit_mylist` for edits, which
//! also writes the optimistic local row in the same transaction; `queue_command` for adds,
//! removals and refreshes). Nothing is sent until the queue runs, and a removal only drops the
//! local row once AniDB acknowledged it (`animatic_core::sync`).

use std::collections::BTreeSet;

use animatic_anidb::commands::{FileRef, MylistEdit};
use animatic_core::library::{LocalFile, MylistEntry};
use animatic_core::sync::Command;

use super::*;

/// One text field of the form. Left alone it means "keep what is there"; ticked it is sent,
/// and an empty ticked field clears the value (the same rule as the folder policy).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct TextChoice {
    pub(super) change: bool,
    pub(super) text: String,
}

impl TextChoice {
    fn value(&self) -> Option<String> {
        self.change.then(|| self.text.trim().to_string())
    }

    fn with_text(text: Option<&str>) -> Self {
        Self {
            change: false,
            text: text.unwrap_or_default().to_string(),
        }
    }
}

/// The fields of an add or edit, each with a "change it" tick.
#[derive(Clone, Debug)]
pub(super) struct MylistForm {
    change_state: bool,
    state: i64,
    change_watched: bool,
    watched: bool,
    change_viewdate: bool,
    viewdate: String,
    storage: TextChoice,
    source: TextChoice,
    other: TextChoice,
}

impl MylistForm {
    /// An editor for an existing entry: it shows the current values and changes nothing yet.
    pub(super) fn for_entry(entry: &MylistEntry) -> Self {
        Self {
            change_state: false,
            state: entry.state,
            change_watched: false,
            watched: entry.viewed,
            change_viewdate: false,
            viewdate: format::datetime_local(entry.viewdate),
            storage: TextChoice::with_text(entry.storage.as_deref()),
            source: TextChoice::with_text(entry.source.as_deref()),
            other: TextChoice::with_text(entry.other.as_deref()),
        }
    }

    /// A form for new entries: nothing is set, so AniDB's defaults apply.
    pub(super) fn for_add() -> Self {
        Self {
            change_state: false,
            state: 1,
            change_watched: false,
            watched: false,
            change_viewdate: false,
            viewdate: String::new(),
            storage: TextChoice::default(),
            source: TextChoice::default(),
            other: TextChoice::default(),
        }
    }

    /// Whether the entry counts as watched once the form is applied to an entry that is
    /// `watched_now`.
    fn watched_after(&self, watched_now: bool) -> bool {
        if self.change_watched {
            self.watched
        } else {
            watched_now
        }
    }

    /// The edit this form stands for. `watched_now` is the entry's current state (false for a
    /// new entry); a watched mark that repeats it changes nothing and so keeps the view date.
    /// A watched entry without an explicit date is stamped `now`: the time the user marked it,
    /// not the time AniDB hears of it.
    pub(super) fn build(&self, watched_now: bool, now: i64) -> Result<MylistEdit, String> {
        let mut edit = MylistEdit {
            state: self.change_state.then_some(self.state),
            storage: self.storage.value(),
            source: self.source.value(),
            other: self.other.value(),
            ..Default::default()
        };
        let watched = self.watched_after(watched_now);
        let date_edited = watched && self.change_viewdate;
        if (self.change_watched && self.watched != watched_now) || date_edited {
            edit.viewed = Some(watched);
            if watched {
                let text = self.viewdate.trim();
                edit.viewdate = Some(if date_edited && !text.is_empty() {
                    parse_view_date(text)?
                } else {
                    now
                });
            }
        }
        Ok(edit)
    }
}

/// "YYYY-MM-DD HH:MM" or "YYYY-MM-DD", read as local time (the format the tables show).
fn parse_view_date(text: &str) -> Result<i64, String> {
    use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
    let invalid = || format!("View date \"{text}\" must look like 2025-03-04 18:30 (local time)");
    let naive = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M")
        .or_else(|_| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d").map(|date| date.and_time(NaiveTime::MIN))
        })
        .map_err(|_| invalid())?;
    let local = naive
        .and_local_timezone(chrono::Local)
        .earliest()
        .ok_or_else(|| format!("View date \"{text}\" does not exist in your time zone"))?;
    let stamp = local.timestamp();
    if stamp <= 0 {
        return Err(invalid());
    }
    Ok(stamp)
}

/// Apply a click on a hashed-file row to the selection. A plain click selects only that
/// file; Ctrl toggles it; Shift selects every visible row from the first selected file to it.
/// `selected` is the primary file the single-file actions (rename, play, ...) act on.
pub(super) fn click_selection(
    selected: &mut Option<i64>,
    marked: &mut BTreeSet<i64>,
    id: i64,
    toggle: bool,
    extend: bool,
    visible: impl FnOnce() -> Vec<i64>,
) {
    if extend && let Some(anchor) = *selected {
        let order = visible();
        let position = |wanted: i64| order.iter().position(|&x| x == wanted);
        if let (Some(a), Some(b)) = (position(anchor), position(id)) {
            marked.clear();
            marked.extend(order[a.min(b)..=a.max(b)].iter().copied());
            return;
        }
    }
    if toggle {
        if marked.remove(&id) {
            if *selected == Some(id) {
                *selected = marked.iter().next().copied();
            }
        } else {
            marked.insert(id);
            *selected = Some(id);
        }
        return;
    }
    marked.clear();
    marked.insert(id);
    *selected = Some(id);
}

/// A hashed file an add would queue.
struct AddTarget {
    name: String,
    file: FileRef,
}

/// The files of a selection that **Add to MyList** would queue, and what it left out.
#[derive(Default)]
struct AddPlan {
    targets: Vec<AddTarget>,
    in_mylist: usize,
    already_queued: usize,
}

impl AddPlan {
    /// An identified file is added by file id; an unidentified one by size and ED2K hash, which
    /// AniDB resolves first. Files in MyList, files with an add already waiting, and further
    /// copies of the same content are left out.
    fn new<'a>(files: impl IntoIterator<Item = &'a LocalFile>) -> Self {
        let mut plan = AddPlan::default();
        for file in files {
            if file.lid.is_some() {
                plan.in_mylist += 1;
                continue;
            }
            if file.mylist_add_pending {
                plan.already_queued += 1;
                continue;
            }
            let reference = match file.fid {
                Some(fid) => FileRef::Fid(fid),
                None => FileRef::Hash {
                    size: file.size.max(0) as u64,
                    ed2k: file.ed2k.clone(),
                },
            };
            if plan.targets.iter().all(|t| t.file != reference) {
                plan.targets.push(AddTarget {
                    name: file.file_name().to_string(),
                    file: reference,
                });
            }
        }
        plan
    }

    fn unidentified(&self) -> usize {
        self.targets
            .iter()
            .filter(|t| matches!(t.file, FileRef::Hash { .. }))
            .count()
    }
}

pub(super) struct EntryEditor {
    lid: i64,
    title: String,
    entry: MylistEntry,
    form: MylistForm,
    error: Option<String>,
}

pub(super) struct AddDialog {
    plan: AddPlan,
    form: MylistForm,
}

struct RemoveConfirm {
    lid: i64,
    title: String,
}

/// UI state of the MyList management features.
#[derive(Default)]
pub(super) struct MylistUi {
    /// The My list row the toolbar acts on.
    pub(super) selected_lid: Option<i64>,
    /// Every selected hashed file (the primary one included).
    pub(super) marked_locals: BTreeSet<i64>,
    editor: Option<EntryEditor>,
    add: Option<AddDialog>,
    remove: Option<RemoveConfirm>,
}

const VIEW_DATE_HINT: &str = "YYYY-MM-DD HH:MM, empty = now";

fn text_row(ui: &mut egui::Ui, verb: &str, label: &str, choice: &mut TextChoice) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut choice.change, format!("{verb} {label}"));
        if ui
            .add(egui::TextEdit::singleline(&mut choice.text).desired_width(260.0))
            .changed()
        {
            choice.change = true;
        }
    });
}

/// The shared fields of the editor and the add dialog. Editing a value ticks its box.
fn form_ui(ui: &mut egui::Ui, form: &mut MylistForm, verb: &str, watched_now: bool, salt: &str) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut form.change_state, format!("{verb} state"));
        let before = form.state;
        egui::ComboBox::from_id_salt((salt, "state"))
            .selected_text(mylist_state_name(form.state))
            .show_ui(ui, |ui| {
                for (id, name) in MYLIST_STATES.iter().enumerate() {
                    ui.selectable_value(&mut form.state, id as i64, *name);
                }
            });
        if form.state != before {
            form.change_state = true;
        }
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut form.change_watched, format!("{verb} watched"));
        let before = form.watched;
        ui.checkbox(&mut form.watched, "Watched");
        if form.watched != before {
            form.change_watched = true;
        }
    });
    if form.watched_after(watched_now) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut form.change_viewdate, format!("{verb} view date"));
            if ui
                .add(
                    egui::TextEdit::singleline(&mut form.viewdate)
                        .hint_text(VIEW_DATE_HINT)
                        .desired_width(200.0),
                )
                .changed()
            {
                form.change_viewdate = true;
            }
        });
    }
    text_row(ui, verb, "storage", &mut form.storage);
    text_row(ui, verb, "source", &mut form.source);
    text_row(ui, verb, "other", &mut form.other);
}

impl AnimaticApp {
    /// `Title — episode` of a My list row, for dialogs.
    fn mylist_row_title(&self, lid: i64) -> String {
        let projection = &self.projection;
        let Some(i) = projection.mylist.iter().position(|m| m.entry.lid == lid) else {
            return format!("Entry {lid}");
        };
        let item = &projection.mylist[i];
        match &item.episode {
            Some(e) => format!("{} — episode {}", projection.mylist_title(i), e.epno_str()),
            None => projection.mylist_title(i),
        }
    }

    /// Edit, remove and refresh buttons for the selected My list row.
    pub(super) fn mylist_toolbar(&mut self, ui: &mut egui::Ui) {
        let selected = self.mylist_ui.selected_lid;
        let removing = selected
            .and_then(|lid| self.projection.mylist_pending.get(&lid))
            .is_some_and(|p| p.delete);
        let (mut edit, mut remove, mut refresh) = (false, false, false);
        ui.horizontal(|ui| {
            edit = ui
                .add_enabled(selected.is_some() && !removing, egui::Button::new("Edit…"))
                .on_disabled_hover_text(
                    "Select a row (click its episode) that is not being removed",
                )
                .clicked();
            remove = ui
                .add_enabled(
                    selected.is_some() && !removing,
                    egui::Button::new("Remove from MyList…"),
                )
                .clicked();
            refresh = ui
                .add_enabled(selected.is_some(), egui::Button::new("Refresh entry"))
                .on_hover_text("Queues a request that re-reads this entry from AniDB")
                .clicked();
            ui.label(match selected {
                Some(lid) if removing => format!("Entry {lid}: removal queued"),
                Some(lid) => format!("Entry {lid} selected"),
                None => "Click a row's episode to select it".to_string(),
            });
        });
        let Some(lid) = selected else { return };
        if edit {
            self.open_entry_editor(lid);
        }
        if remove {
            self.mylist_ui.remove = Some(RemoveConfirm {
                lid,
                title: self.mylist_row_title(lid),
            });
        }
        if refresh {
            self.queue_command(Command::MylistRefresh { lid });
            self.log(format!("Queued a refresh of mylist entry {lid}"));
        }
    }

    fn open_entry_editor(&mut self, lid: i64) {
        match self.db.mylist_entry(lid) {
            Ok(Some(entry)) => {
                self.mylist_ui.editor = Some(EntryEditor {
                    lid,
                    title: self.mylist_row_title(lid),
                    form: MylistForm::for_entry(&entry),
                    entry,
                    error: None,
                });
            }
            Ok(None) => {
                self.mylist_ui.selected_lid = None;
                self.error(format!("Mylist entry {lid} no longer exists locally"));
            }
            Err(e) => self.error(format!("Could not read mylist entry {lid}: {e}")),
        }
    }

    /// Open **Add to MyList…** for the selected hashed files that are currently listed.
    pub(super) fn open_add_dialog(&mut self) {
        let listed: BTreeSet<i64> = self
            .views
            .local
            .rows
            .iter()
            .map(|&i| self.projection.local[i].id)
            .collect();
        let mut wanted = self.mylist_ui.marked_locals.clone();
        wanted.extend(self.selected_local);
        let plan = AddPlan::new(
            self.projection
                .local
                .iter()
                .filter(|f| wanted.contains(&f.id) && listed.contains(&f.id)),
        );
        self.mylist_ui.add = Some(AddDialog {
            plan,
            form: MylistForm::for_add(),
        });
    }

    /// The dialogs of the MyList features; call once per frame.
    pub(super) fn mylist_windows(&mut self, ctx: &egui::Context) {
        self.entry_editor_window(ctx);
        self.add_window(ctx);
        self.remove_window(ctx);
    }

    fn entry_editor_window(&mut self, ctx: &egui::Context) {
        let Some(mut editor) = self.mylist_ui.editor.take() else {
            return;
        };
        let removing = self
            .projection
            .mylist_pending
            .get(&editor.lid)
            .is_some_and(|p| p.delete);
        let mut open = true;
        let mut cancel = false;
        let mut to_save: Option<MylistEdit> = None;
        egui::Window::new(format!("Edit MyList entry {}", editor.lid))
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.strong(&editor.title);
                ui.label(
                    RichText::new(
                        "Tick a field to change it. Unticked fields stay as they are on AniDB; \
                         a ticked empty field clears the value.",
                    )
                    .weak(),
                );
                if removing {
                    ui.colored_label(
                        Color32::from_rgb(235, 160, 90),
                        "This entry is waiting to be removed from MyList; edits are refused.",
                    );
                }
                ui.add_space(6.0);
                ui.add_enabled_ui(!removing, |ui| {
                    form_ui(
                        ui,
                        &mut editor.form,
                        "Change",
                        editor.entry.viewed,
                        "entry-editor",
                    );
                });
                let built = editor.form.build(editor.entry.viewed, format::now());
                match &built {
                    Err(message) => {
                        ui.colored_label(Color32::LIGHT_RED, message);
                    }
                    Ok(edit) if *edit == MylistEdit::default() => {
                        ui.label(RichText::new("Nothing is ticked yet.").weak());
                    }
                    Ok(_) => {}
                }
                if let Some(message) = &editor.error {
                    ui.colored_label(Color32::LIGHT_RED, message);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let ready = built.as_ref().is_ok_and(|e| *e != MylistEdit::default());
                    if ui
                        .add_enabled(ready && !removing, egui::Button::new("Save"))
                        .on_hover_text(
                            "Queues the change and shows it here at once; it is sent to AniDB \
                             when the command queue runs",
                        )
                        .clicked()
                        && let Ok(edit) = built
                    {
                        to_save = Some(edit);
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        let mut keep = open && !cancel;
        if let Some(edit) = to_save {
            let lid = editor.lid;
            match self.db.queue_edit_mylist(lid, &edit) {
                Ok(0) => {
                    editor.error = Some(
                        "Nothing was queued: the entry is waiting to be removed from MyList."
                            .into(),
                    );
                }
                Ok(_) => {
                    self.mylist_overlay.insert(
                        lid,
                        MylistOverlay {
                            state: edit.state.unwrap_or(editor.entry.state),
                            viewed: edit.viewed.unwrap_or(editor.entry.viewed),
                            generation: self.loader.next_generation,
                        },
                    );
                    self.log(format!("Queued an edit of mylist entry {lid}"));
                    self.request_library_refresh();
                    keep = false;
                }
                Err(e) => {
                    self.db_write_error(&format!("Could not edit mylist entry {lid}"), &e);
                    editor.error = Some(e.to_string());
                }
            }
        }
        if keep {
            self.mylist_ui.editor = Some(editor);
        }
    }

    fn add_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.mylist_ui.add.take() else {
            return;
        };
        let mut open = true;
        let mut cancel = false;
        let mut to_queue: Option<MylistEdit> = None;
        egui::Window::new("Add to MyList")
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                let plan = &dialog.plan;
                if plan.targets.is_empty() {
                    ui.label("None of the selected files can be added.");
                } else {
                    ui.strong(format!(
                        "{} file{} will be queued for your MyList",
                        plan.targets.len(),
                        if plan.targets.len() == 1 { "" } else { "s" }
                    ));
                    if plan.unidentified() > 0 {
                        ui.label(format!(
                            "{} of them {} not identified yet: AniDB looks them up by hash \
                             first, and a file it does not know is skipped.",
                            plan.unidentified(),
                            if plan.unidentified() == 1 {
                                "is"
                            } else {
                                "are"
                            }
                        ));
                    }
                    egui::ScrollArea::vertical()
                        .max_height(110.0)
                        .id_salt("mylist-add-files")
                        .show(ui, |ui| {
                            for target in &plan.targets {
                                ui.label(&target.name);
                            }
                        });
                }
                if plan.in_mylist > 0 {
                    ui.label(
                        RichText::new(format!(
                            "{} already in MyList (edit them on the My list page)",
                            plan.in_mylist
                        ))
                        .weak(),
                    );
                }
                if plan.already_queued > 0 {
                    ui.label(
                        RichText::new(format!(
                            "{} already waiting to be added",
                            plan.already_queued
                        ))
                        .weak(),
                    );
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Tick a field to set it for every file; unticked fields keep AniDB's \
                         defaults.",
                    )
                    .weak(),
                );
                form_ui(ui, &mut dialog.form, "Set", false, "mylist-add");
                let built = dialog.form.build(false, format::now());
                if let Err(message) = &built {
                    ui.colored_label(Color32::LIGHT_RED, message);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !dialog.plan.targets.is_empty() && built.is_ok(),
                            egui::Button::new("Queue add"),
                        )
                        .clicked()
                        && let Ok(edit) = built
                    {
                        to_queue = Some(edit);
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if let Some(edit) = to_queue {
            let count = dialog.plan.targets.len();
            for target in std::mem::take(&mut dialog.plan.targets) {
                self.queue_command(Command::MylistAdd {
                    file: target.file,
                    edit: edit.clone(),
                });
            }
            self.log(format!(
                "Queued {count} file{} for MyList; run the command queue to send them",
                if count == 1 { "" } else { "s" }
            ));
        } else if open && !cancel {
            self.mylist_ui.add = Some(dialog);
        }
    }

    fn remove_window(&mut self, ctx: &egui::Context) {
        let Some(confirm) = self.mylist_ui.remove.take() else {
            return;
        };
        let mut open = true;
        let mut decision = None;
        egui::Window::new("Remove from MyList")
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label("Remove this entry from your AniDB MyList?");
                ui.strong(&confirm.title);
                ui.add_space(6.0);
                ui.label(
                    "Your media file on disk is not touched. The request is queued and sent \
                     when the command queue runs; this list keeps the row until AniDB confirms \
                     the removal. Edits of this entry that are still queued are discarded.",
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Remove from MyList").clicked() {
                        decision = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        decision = Some(false);
                    }
                });
            });
        match decision {
            Some(true) => {
                self.queue_command(Command::MylistDelete { lid: confirm.lid });
                self.log(format!(
                    "Queued removal of mylist entry {}; run the command queue to send it",
                    confirm.lid
                ));
            }
            None if open => self.mylist_ui.remove = Some(confirm),
            _ => {}
        }
    }

    /// The statistics AniDB reported for the whole account, labelled apart from the local
    /// figures above them.
    pub(super) fn account_figures(&self, ui: &mut egui::Ui) {
        ui.heading("AniDB account figures");
        let Some(stored) = &self.projection.account_stats else {
            ui.label(
                "None received yet. Tools → Fetch mylist statistics queues the request; AniDB \
                 answers when the command queue runs.",
            );
            return;
        };
        let s = &stored.stats;
        ui.label(
            RichText::new(format!(
                "Reported by AniDB on {} for your whole account. They are not the local library \
                 figures above: they include entries this database has never cached.",
                format::datetime_local(stored.fetched)
            ))
            .weak(),
        );
        ui.columns(4, |cols| {
            metric(&mut cols[0], "Anime (AniDB)", format::thousands(s.animes));
            metric(
                &mut cols[1],
                "Episodes (AniDB)",
                format::thousands(s.episodes),
            );
            metric(&mut cols[2], "Files (AniDB)", format::thousands(s.files));
            metric(
                &mut cols[3],
                "Size (AniDB)",
                format::size_short(s.size_mb.saturating_mul(1024 * 1024)),
            );
        });
        ui.columns(4, |cols| {
            metric(
                &mut cols[0],
                "Watched episodes (AniDB)",
                format::thousands(s.viewed_episodes),
            );
            metric(
                &mut cols[1],
                "Watch time (AniDB)",
                format::length_compact(s.viewed_minutes.saturating_mul(60)),
            );
            metric(
                &mut cols[2],
                "Watched of MyList (AniDB)",
                format!("{}%", s.viewed_pct_of_mylist),
            );
            metric(
                &mut cols[3],
                "Votes / reviews (AniDB)",
                format!("{} / {}", s.votes, s.reviews),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> MylistEntry {
        MylistEntry {
            lid: 1,
            state: 1,
            storage: Some("Shelf".into()),
            ..Default::default()
        }
    }

    #[test]
    fn an_untouched_form_changes_nothing() {
        let form = MylistForm::for_entry(&entry());
        assert_eq!(form.build(false, 100).unwrap(), MylistEdit::default());
    }

    #[test]
    fn only_ticked_fields_are_sent_and_an_empty_ticked_text_clears_it() {
        let mut form = MylistForm::for_entry(&entry());
        form.change_state = true;
        form.state = 2;
        form.storage.change = true;
        form.storage.text = String::new();
        form.other.change = true;
        form.other.text = "  A note ".into();
        // Source is edited but not ticked: it stays out of the edit.
        form.source.text = "Ignored".into();
        assert_eq!(
            form.build(false, 100).unwrap(),
            MylistEdit {
                state: Some(2),
                storage: Some(String::new()),
                other: Some("A note".into()),
                ..Default::default()
            }
        );
    }

    #[test]
    fn marking_watched_stamps_the_moment_and_a_typed_date_wins() {
        let mut form = MylistForm::for_entry(&entry());
        form.change_watched = true;
        form.watched = true;
        let edit = form.build(false, 1_700_000_000).unwrap();
        assert_eq!(
            (edit.viewed, edit.viewdate),
            (Some(true), Some(1_700_000_000))
        );

        form.change_viewdate = true;
        form.viewdate = "2024-03-05 18:30".into();
        let edit = form.build(false, 1_700_000_000).unwrap();
        assert_eq!(edit.viewed, Some(true));
        assert_eq!(
            format::datetime_local(edit.viewdate.unwrap()),
            "2024-03-05 18:30"
        );

        form.viewdate = "yesterday".into();
        assert!(form.build(false, 1).unwrap_err().contains("yesterday"));

        // Date-only input is midnight local time; an empty ticked date means "now".
        form.viewdate = "2024-03-05".into();
        assert_eq!(
            format::datetime_local(form.build(false, 1).unwrap().viewdate.unwrap()),
            "2024-03-05 00:00"
        );
        form.viewdate = String::new();
        assert_eq!(form.build(false, 77).unwrap().viewdate, Some(77));
    }

    #[test]
    fn correcting_the_date_of_a_watched_entry_keeps_it_watched_and_repeating_the_mark_is_a_no_op() {
        let watched = MylistEntry {
            viewed: true,
            viewdate: 1_700_000_000,
            ..entry()
        };
        let mut form = MylistForm::for_entry(&watched);
        form.change_viewdate = true;
        form.viewdate = "2024-03-05 18:30".into();
        let edit = form.build(true, 5).unwrap();
        assert_eq!(edit.viewed, Some(true));
        assert!(edit.viewdate.is_some_and(|d| d != 5));

        let mut repeat = MylistForm::for_entry(&watched);
        repeat.change_watched = true;
        repeat.watched = true;
        assert_eq!(
            repeat.build(true, 5).unwrap(),
            MylistEdit::default(),
            "marking an already watched entry must not reset its date"
        );

        // A date typed for an entry that stays unwatched is meaningless and is dropped.
        let mut unwatched = MylistForm::for_entry(&entry());
        unwatched.change_viewdate = true;
        unwatched.viewdate = "2024-03-05 18:30".into();
        assert_eq!(unwatched.build(false, 5).unwrap(), MylistEdit::default());

        let mut unmark = MylistForm::for_entry(&watched);
        unmark.change_watched = true;
        unmark.watched = false;
        assert_eq!(
            unmark.build(true, 5).unwrap(),
            MylistEdit {
                viewed: Some(false),
                ..Default::default()
            }
        );
    }

    fn local(id: i64, fid: Option<i64>, lid: Option<i64>, pending: bool) -> LocalFile {
        LocalFile {
            id,
            path: format!("/synthetic/file{id}.mkv"),
            size: 1000 + id,
            ed2k: format!("hash{id}"),
            fid,
            lid,
            mylist_add_pending: pending,
            ..Default::default()
        }
    }

    #[test]
    fn add_targets_use_the_file_id_or_the_hash_and_skip_what_is_already_handled() {
        let files = [
            local(1, Some(11), None, false),
            local(2, None, None, false),
            local(3, Some(13), Some(500), false),
            local(4, Some(14), None, true),
            LocalFile {
                path: "/synthetic/copy/file1.mkv".into(),
                ..local(5, Some(11), None, false)
            },
        ];
        let plan = AddPlan::new(&files);
        let refs: Vec<_> = plan.targets.iter().map(|t| t.file.clone()).collect();
        assert_eq!(
            refs,
            vec![
                FileRef::Fid(11),
                FileRef::Hash {
                    size: 1002,
                    ed2k: "hash2".into()
                }
            ]
        );
        assert_eq!(
            (plan.in_mylist, plan.already_queued, plan.unidentified()),
            (1, 1, 1)
        );
    }

    #[test]
    fn clicking_selects_toggles_and_extends_a_range() {
        let order = || vec![10, 11, 12, 13, 14];
        let mut selected = None;
        let mut marked = BTreeSet::new();

        click_selection(&mut selected, &mut marked, 11, false, false, order);
        assert_eq!((selected, marked.clone()), (Some(11), BTreeSet::from([11])));

        click_selection(&mut selected, &mut marked, 13, true, false, order);
        assert_eq!(marked, BTreeSet::from([11, 13]));
        assert_eq!(selected, Some(13));

        // Toggling the primary file off hands the primary role to a remaining file.
        click_selection(&mut selected, &mut marked, 13, true, false, order);
        assert_eq!((selected, marked.clone()), (Some(11), BTreeSet::from([11])));

        click_selection(&mut selected, &mut marked, 14, false, true, order);
        assert_eq!(marked, BTreeSet::from([11, 12, 13, 14]));
        assert_eq!(selected, Some(11), "the anchor stays the primary file");

        // A plain click starts over.
        click_selection(&mut selected, &mut marked, 10, false, false, order);
        assert_eq!((selected, marked), (Some(10), BTreeSet::from([10])));
    }
}
