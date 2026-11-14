//! MyList management in the desktop application: the entry editor, **Add to MyList…**,
//! **Remove from MyList…**, the **Refresh** actions and the AniDB account figures on the
//! overview.
//!
//! Every change is stored in the command queue first (`Db::queue_edit_mylist` for edits, and
//! `Db::queue_edit_mylist_many` for all entries of an anime in one transaction; both also
//! write the optimistic local rows in the same transaction; `queue_command` for adds,
//! removals and refreshes). Nothing is sent until the queue runs, and a removal only drops the
//! local row once AniDB acknowledged it (`animatic_core::sync`).

use std::collections::{BTreeSet, HashSet};

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

    /// The edit this form stands for when it is applied to many entries at once. They differ
    /// in whether they are watched, so a ticked watched box is a value to set, never "no
    /// change" (`Db::queue_edit_mylist_many` leaves entries that already match alone). Only a
    /// view date typed into the form is a value for every entry; a plain "watched" carries no
    /// date, so entries that already are watched keep theirs and the others are stamped when
    /// the batch is queued.
    pub(super) fn build_many(&self, now: i64) -> Result<MylistEdit, String> {
        let mut edit = self.build(self.change_watched && !self.watched, now)?;
        let date_typed = self.change_viewdate && !self.viewdate.trim().is_empty();
        if edit.viewed == Some(true) && !date_typed {
            edit.viewdate = None;
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
    /// The entry being edited, or every entry of the anime being edited together.
    lids: Vec<i64>,
    title: String,
    /// The entry's stored values when a single entry is edited (the form shows them);
    /// `None` for an anime, whose entries differ.
    entry: Option<MylistEntry>,
    form: MylistForm,
    error: Option<String>,
}

pub(super) struct AddDialog {
    plan: AddPlan,
    form: MylistForm,
}

struct RemoveConfirm {
    lids: Vec<i64>,
    title: String,
}

/// UI state of the MyList management features.
#[derive(Default)]
pub(super) struct MylistUi {
    /// The My list entry the toolbar acts on. Never set together with `selected_anime`.
    pub(super) selected_lid: Option<i64>,
    /// The anime whose entries are all selected (a click on its top-level row), for the
    /// toolbar actions.
    pub(super) selected_anime: Option<i64>,
    /// Every selected hashed file (the primary one included).
    pub(super) marked_locals: BTreeSet<i64>,
    editor: Option<EntryEditor>,
    add: Option<AddDialog>,
    remove: Option<RemoveConfirm>,
}

impl MylistUi {
    pub(super) fn select_entry(&mut self, lid: i64) {
        self.selected_lid = Some(lid);
        self.selected_anime = None;
    }

    pub(super) fn select_anime(&mut self, aid: i64) {
        self.selected_lid = None;
        self.selected_anime = Some(aid);
    }
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

    /// The entries the toolbar acts on: the selected entry, or every entry of the selected
    /// anime (empty when that anime is filtered out of the table).
    fn selected_mylist_lids(&self) -> Vec<i64> {
        if let Some(lid) = self.mylist_ui.selected_lid {
            return vec![lid];
        }
        let Some(aid) = self.mylist_ui.selected_anime else {
            return Vec::new();
        };
        self.views
            .mylist
            .group_of(aid)
            .map_or_else(Vec::new, |group| {
                group
                    .members
                    .iter()
                    .map(|&i| self.projection.mylist[i].entry.lid)
                    .collect()
            })
    }

    /// Title of the selected anime for dialogs and the status line.
    fn selected_anime_title(&self, aid: i64) -> String {
        self.views.mylist.group_of(aid).map_or_else(
            || format!("aid {aid}"),
            |group| self.projection.mylist_title(group.members[0]),
        )
    }

    /// Edit, remove and refresh buttons for the selected My list row (an entry, or every
    /// entry of an anime), and the expand/collapse buttons of the anime rows.
    pub(super) fn mylist_toolbar(&mut self, ui: &mut egui::Ui) {
        let lids = self.selected_mylist_lids();
        // Entries whose removal is queued refuse edits and cannot be removed twice.
        let actionable: Vec<i64> = lids
            .iter()
            .copied()
            .filter(|lid| {
                !self
                    .projection
                    .mylist_pending
                    .get(lid)
                    .is_some_and(|p| p.delete)
            })
            .collect();
        let removing = lids.len() - actionable.len();
        let anime = self.mylist_ui.selected_anime.filter(|_| !lids.is_empty());
        let status = match (self.mylist_ui.selected_lid, anime) {
            (Some(lid), _) if removing > 0 => Some(format!("Entry {lid}: removal queued")),
            (Some(lid), _) => Some(format!("Entry {lid} · {}", self.mylist_row_title(lid))),
            (None, Some(aid)) => Some(format!(
                "{} · all {} selected{}",
                self.selected_anime_title(aid),
                crate::style::plural(lids.len(), "entry", "entries"),
                if removing > 0 {
                    format!(" ({removing} being removed)")
                } else {
                    String::new()
                }
            )),
            (None, None) => None,
        };
        let (mut edit, mut remove, mut refresh) = (false, false, false);
        let (mut expand, mut collapse) = (false, false);
        let any_anime = self.views.mylist.anime_count() > 0;
        ui.horizontal(|ui| {
            edit = ui
                .add_enabled(!actionable.is_empty(), egui::Button::new("Edit…"))
                .on_hover_text("Edit the selected entry, or every entry of the selected anime")
                .on_disabled_hover_text(
                    "Select a row (click its episode) that is not being removed",
                )
                .clicked();
            remove = ui
                .add_enabled(
                    !actionable.is_empty(),
                    egui::Button::new("Remove from MyList…"),
                )
                .clicked();
            refresh = ui
                .add_enabled(!lids.is_empty(), egui::Button::new("Refresh from AniDB"))
                .on_hover_text("Queues a request that re-reads the selected entries from AniDB")
                .clicked();
            expand = ui
                .add_enabled(any_anime, egui::Button::new("Expand all"))
                .on_hover_text("Show the entries of every anime")
                .clicked();
            collapse = ui
                .add_enabled(any_anime, egui::Button::new("Collapse all"))
                .on_hover_text("Show one row per anime")
                .clicked();
            match status {
                Some(text) => ui.label(text),
                None => ui.label(
                    RichText::new(
                        "Click a row's episode to select it; an anime's episode count selects \
                         all of its entries",
                    )
                    .weak()
                    .small(),
                ),
            };
        });
        if expand {
            self.views.mylist.expand_all();
        }
        if collapse {
            self.views.mylist.collapse_all();
        }
        let Some(&first) = lids.first() else { return };
        let title = match anime {
            Some(aid) => format!(
                "{} — {}",
                self.selected_anime_title(aid),
                crate::style::plural(actionable.len(), "entry", "entries")
            ),
            None => self.mylist_row_title(first),
        };
        if edit {
            match anime {
                Some(_) => {
                    self.mylist_ui.editor = Some(EntryEditor {
                        lids: actionable.clone(),
                        title: title.clone(),
                        entry: None,
                        form: MylistForm::for_add(),
                        error: None,
                    });
                }
                None => self.open_entry_editor(first),
            }
        }
        if remove {
            self.mylist_ui.remove = Some(RemoveConfirm {
                lids: actionable,
                title,
            });
        }
        if refresh {
            for &lid in &lids {
                self.queue_command(Command::MylistRefresh { lid });
            }
            self.log(match lids.as_slice() {
                [lid] => format!("Queued a refresh of MyList entry {lid}"),
                _ => format!("Queued a refresh of {} MyList entries", lids.len()),
            });
        }
    }

    /// Queue `edit` for every entry of `lids` in one transaction and show the result at once
    /// (the overlay bridges the time until a projection requested after the write arrives).
    /// Entries that already have the requested values are left alone, entries with a queued
    /// removal are refused (see `Db::queue_edit_mylist_many`).
    ///
    /// This runs on the UI thread like the single-entry edit does, deliberately: it is one
    /// transaction with one commit, bounded by the entries of one anime, that waits for no
    /// network and no hashing; moving it to the worker would queue the click behind a running
    /// job and would show an optimistic state before it is saved (rule 13). A busy database
    /// changes nothing and is reported, like the single-entry edit.
    pub(super) fn queue_mylist_edit_many(
        &mut self,
        lids: &[i64],
        edit: &MylistEdit,
    ) -> animatic_core::Result<animatic_core::sync::BatchEdit> {
        let wanted: HashSet<i64> = lids.iter().copied().collect();
        let shown: HashMap<i64, (i64, bool)> = self
            .projection
            .mylist
            .iter()
            .filter(|m| wanted.contains(&m.entry.lid))
            .map(|m| {
                let lid = m.entry.lid;
                let current = self
                    .mylist_overlay
                    .get(&lid)
                    .map_or((m.entry.state, m.entry.viewed), |o| (o.state, o.viewed));
                (lid, current)
            })
            .collect();
        let done = self.db.queue_edit_mylist_many(lids, edit)?;
        for &lid in &done.queued {
            let (state, viewed) = shown.get(&lid).copied().unwrap_or((0, false));
            self.mylist_overlay.insert(
                lid,
                MylistOverlay {
                    state: edit.state.unwrap_or(state),
                    viewed: edit.viewed.unwrap_or(viewed),
                    generation: self.loader.next_generation,
                },
            );
        }
        if done.skipped > 0 {
            self.log(format!(
                "{} MyList {} pending removal or missing; edit ignored",
                done.skipped,
                if done.skipped == 1 {
                    "entry is"
                } else {
                    "entries are"
                }
            ));
        }
        if !done.queued.is_empty() {
            self.request_library_refresh();
        }
        Ok(done)
    }

    fn open_entry_editor(&mut self, lid: i64) {
        match self.db.mylist_entry(lid) {
            Ok(Some(entry)) => {
                self.mylist_ui.editor = Some(EntryEditor {
                    lids: vec![lid],
                    title: self.mylist_row_title(lid),
                    form: MylistForm::for_entry(&entry),
                    entry: Some(entry),
                    error: None,
                });
            }
            Ok(None) => {
                self.mylist_ui.selected_lid = None;
                self.error(format!("MyList entry {lid} no longer exists locally"));
            }
            Err(e) => self.error(format!("Could not read MyList entry {lid}: {e}")),
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
        let removing = editor.lids.iter().all(|lid| {
            self.projection
                .mylist_pending
                .get(lid)
                .is_some_and(|p| p.delete)
        });
        let many = editor.entry.is_none();
        let mut open = true;
        let mut cancel = false;
        let mut to_save: Option<MylistEdit> = None;
        let shown = super::dialog::window(ctx, "Edit MyList entry")
            .id(egui::Id::unique("mylist_entry_editor"))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.strong(&editor.title);
                ui.label(
                    RichText::new(if many {
                        "Tick a field to change it for every one of these entries; entries \
                         that already have the value are left alone. A ticked empty field \
                         clears the value."
                    } else {
                        "Tick a field to change it. Unticked fields stay as they are on \
                         AniDB; a ticked empty field clears the value."
                    })
                    .weak(),
                );
                if removing {
                    ui.colored_label(
                        crate::style::warn(ui.visuals()),
                        "This entry is waiting to be removed from MyList; edits are refused.",
                    );
                }
                ui.add_space(6.0);
                let watched_now = editor.entry.as_ref().is_some_and(|e| e.viewed);
                ui.add_enabled_ui(!removing, |ui| {
                    form_ui(ui, &mut editor.form, "Change", watched_now, "entry-editor");
                });
                let built = match &editor.entry {
                    Some(entry) => editor.form.build(entry.viewed, format::now()),
                    None => editor.form.build_many(format::now()),
                };
                match &built {
                    Err(message) => {
                        ui.colored_label(crate::style::error(ui.visuals()), message);
                    }
                    Ok(edit) if *edit == MylistEdit::default() => {
                        ui.label(RichText::new("Nothing is ticked yet.").weak());
                    }
                    Ok(_) => {}
                }
                if let Some(message) = &editor.error {
                    ui.colored_label(crate::style::error(ui.visuals()), message);
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
        let escape = super::dialog::escape_closes(ctx, shown.as_ref());
        let mut keep = open && !cancel && !escape;
        if let Some(edit) = to_save {
            if let Some(entry) = editor.entry.clone() {
                let lid = editor.lids[0];
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
                                state: edit.state.unwrap_or(entry.state),
                                viewed: edit.viewed.unwrap_or(entry.viewed),
                                generation: self.loader.next_generation,
                            },
                        );
                        self.log(format!("Queued an edit of MyList entry {lid}"));
                        self.request_library_refresh();
                        keep = false;
                    }
                    Err(e) => {
                        self.db_write_error(&format!("Could not edit MyList entry {lid}"), &e);
                        editor.error = Some(e.to_string());
                    }
                }
            } else {
                match self.queue_mylist_edit_many(&editor.lids, &edit) {
                    Ok(done) if done.queued.is_empty() && done.unchanged == 0 => {
                        editor.error = Some(
                            "Nothing was queued: the entries are waiting to be removed from \
                             MyList."
                                .into(),
                        );
                    }
                    Ok(done) => {
                        self.log(format!(
                            "Queued an edit of {} ({} already as asked)",
                            crate::style::plural(
                                done.queued.len(),
                                "MyList entry",
                                "MyList entries"
                            ),
                            done.unchanged
                        ));
                        keep = false;
                    }
                    Err(e) => {
                        self.db_write_error("Could not edit the MyList entries", &e);
                        editor.error = Some(e.to_string());
                    }
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
        let shown = super::dialog::window(ctx, "Add to MyList")
            .open(&mut open)
            .show(ctx, |ui| {
                let plan = &dialog.plan;
                if plan.targets.is_empty() {
                    ui.label("None of the selected files can be added.");
                } else {
                    ui.strong(format!(
                        "{} will be queued for your MyList",
                        crate::style::plural(plan.targets.len(), "file", "files")
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
                            "{} already in MyList (edit them on the MyList page)",
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
                    ui.colored_label(crate::style::error(ui.visuals()), message);
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
        let escape = super::dialog::escape_closes(ctx, shown.as_ref());
        if let Some(edit) = to_queue {
            let count = dialog.plan.targets.len();
            for target in std::mem::take(&mut dialog.plan.targets) {
                self.queue_command(Command::MylistAdd {
                    file: target.file,
                    edit: edit.clone(),
                });
            }
            self.log(format!(
                "Queued {} for MyList; run the command queue to send them",
                crate::style::plural(count, "file", "files")
            ));
        } else if open && !cancel && !escape {
            self.mylist_ui.add = Some(dialog);
        }
    }

    fn remove_window(&mut self, ctx: &egui::Context) {
        let Some(confirm) = self.mylist_ui.remove.take() else {
            return;
        };
        let mut decision = None;
        let several = confirm.lids.len() > 1;
        let modal = egui::Modal::new(egui::Id::unique("mylist_remove_confirm")).show(ctx, |ui| {
            ui.set_max_width(440.0);
            crate::style::section(ui, "Remove from MyList");
            ui.label(if several {
                "Remove all of these entries from your AniDB MyList?"
            } else {
                "Remove this entry from your AniDB MyList?"
            });
            ui.strong(&confirm.title);
            ui.add_space(6.0);
            ui.label(format!(
                "Your media {} on disk {} not touched. The request is queued and sent \
                 when the command queue runs; this list keeps the {} until AniDB confirms \
                 the removal. Edits of {} that are still queued are discarded.",
                if several { "files" } else { "file" },
                if several { "are" } else { "is" },
                if several { "rows" } else { "row" },
                if several {
                    "these entries"
                } else {
                    "this entry"
                },
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = crate::style::destructive(ui, "Remove from MyList");
                if ui.button(label).clicked() {
                    decision = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
            });
        });
        if decision.is_none() && modal.should_close() {
            decision = Some(false);
        }
        match decision {
            Some(true) => {
                for &lid in &confirm.lids {
                    self.queue_command(Command::MylistDelete { lid });
                }
                self.log(match confirm.lids.as_slice() {
                    [lid] => format!(
                        "Queued removal of MyList entry {lid}; run the command queue to send it"
                    ),
                    lids => format!(
                        "Queued removal of {} MyList entries; run the command queue to send them",
                        lids.len()
                    ),
                });
            }
            None => self.mylist_ui.remove = Some(confirm),
            Some(false) => {}
        }
    }

    /// The statistics AniDB reported for the whole account, labelled apart from the local
    /// figures above them.
    pub(super) fn account_figures(&mut self, ui: &mut egui::Ui) {
        crate::style::section(ui, "Your AniDB account");
        let Some(stored) = &self.projection.account_stats else {
            let mut fetch = false;
            ui.horizontal(|ui| {
                ui.label(RichText::new("No account statistics yet.").weak());
                fetch = ui
                    .button("Fetch MyList statistics")
                    .on_hover_text(
                        "Queues the request; AniDB answers when you run the command queue",
                    )
                    .clicked();
            });
            if fetch {
                self.queue_command(Command::MylistStats);
            }
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
            metric(&mut cols[0], "Anime", format::thousands(s.animes));
            metric(&mut cols[1], "Episodes", format::thousands(s.episodes));
            metric(&mut cols[2], "Files", format::thousands(s.files));
            metric(
                &mut cols[3],
                "Size",
                format::size_short(s.size_mb.saturating_mul(1024 * 1024)),
            );
        });
        ui.columns(4, |cols| {
            metric(
                &mut cols[0],
                "Watched episodes",
                format::thousands(s.viewed_episodes),
            );
            metric(
                &mut cols[1],
                "Watch time",
                format::length_compact(s.viewed_minutes.saturating_mul(60)),
            );
            metric(
                &mut cols[2],
                "MyList watched",
                format!("{}%", s.viewed_pct_of_mylist),
            );
            metric(
                &mut cols[3],
                "Votes / reviews",
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

    #[test]
    fn a_form_for_many_entries_sets_watched_either_way_and_ignores_unticked_fields() {
        let mut form = MylistForm::for_add();
        assert_eq!(form.build_many(5).unwrap(), MylistEdit::default());
        form.change_watched = true;
        form.watched = true;
        assert_eq!(
            (
                form.build_many(5).unwrap().viewed,
                form.build_many(5).unwrap().viewdate
            ),
            (Some(true), None),
            "a plain mark carries no date: watched entries keep theirs, the batch stamps the rest"
        );
        form.change_viewdate = true;
        form.viewdate = "2025-03-04 18:30".into();
        let dated = form.build_many(5).unwrap();
        assert_eq!(dated.viewed, Some(true));
        assert!(
            dated.viewdate.is_some_and(|d| d != 5),
            "a typed date is a value for every entry"
        );
        form.change_viewdate = false;
        form.watched = false;
        assert_eq!(
            form.build_many(5).unwrap(),
            MylistEdit {
                viewed: Some(false),
                ..Default::default()
            },
            "unmarking applies to entries that are watched"
        );
        form.change_watched = false;
        form.change_state = true;
        form.state = 3;
        assert_eq!(
            form.build_many(5).unwrap(),
            MylistEdit {
                state: Some(3),
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
