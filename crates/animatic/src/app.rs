use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use animatic_core::db::Db;
use animatic_core::format;
use animatic_core::library::{Anime, AnimeDetail, LibraryProjection, mylist_state_name};
use animatic_core::rename::plan::{self, PatternKind, Plan};
use animatic_core::settings::{MYLIST_STATES, Settings, StatFormat, Theme};
use eframe::egui::{self, Align, Color32, Layout, RichText, Sense};
use egui_extras::{Column, TableBuilder};

use crate::worker::{Event, Job, Worker};

/// Minimum spacing between projection requests, so a long sync does not reload continuously.
const RELOAD_INTERVAL: Duration = Duration::from_millis(250);
/// Longest the UI thread waits on a database lock held by the worker before reporting it.
const UI_DB_BUSY_TIMEOUT: Duration = Duration::from_millis(1500);
/// How often queue pushes that hit a busy database are retried.
const DEFERRED_RETRY: Duration = Duration::from_millis(500);
/// Most commands held in memory while the database is busy; beyond this they are refused.
const DEFERRED_LIMIT: usize = 1000;
/// Final attempt to save unsaved commands when the window closes.
const EXIT_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);
const LOADING_STATUS: &str = "Loading library…";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Dashboard,
    Mylist,
    Anime,
    Search,
    Hashed,
    Groups,
    Genres,
    Queue,
    Log,
}

impl Page {
    const ALL: [(Page, &str); 9] = [
        (Page::Dashboard, "Overview"),
        (Page::Mylist, "My list"),
        (Page::Anime, "Anime"),
        (Page::Search, "Advanced search"),
        (Page::Hashed, "Hashed files"),
        (Page::Groups, "Groups"),
        (Page::Genres, "Genres"),
        (Page::Queue, "Queue"),
        (Page::Log, "Log"),
    ];
}

/// Request/response bookkeeping for background library projections: at most one request is
/// outstanding, further changes only mark the projection dirty.
struct ProjectionLoader {
    /// Generation the next request will carry; generations start at 1.
    next_generation: u64,
    outstanding: Option<u64>,
    dirty: bool,
    last_request: Option<Instant>,
    /// Bumped whenever a projection is swapped in; filtered indices are keyed on it.
    version: u64,
}

/// Filtered row indices into the projection, rebuilt only when their key changes.
#[derive(Default)]
struct IndexCache<K> {
    key: Option<K>,
    rows: Vec<usize>,
}

impl<K: PartialEq + Clone> IndexCache<K> {
    /// Rebuild with `build` unless `key` is unchanged; true when rebuilt.
    fn refresh(&mut self, key: &K, build: impl FnOnce() -> Vec<usize>) -> bool {
        if self.key.as_ref() == Some(key) {
            return false;
        }
        self.rows = build();
        self.key = Some(key.clone());
        true
    }
}

#[derive(Default)]
struct TableViews {
    anime: IndexCache<(u64, String)>,
    search: IndexCache<(u64, String, String, i64, bool, bool)>,
    mylist: IndexCache<(u64, String)>,
    local: IndexCache<(u64, String)>,
    groups: IndexCache<(u64, String)>,
    genres: IndexCache<(u64, String)>,
}

/// Anime detail shown in the detail window, fetched once per (anime, projection version).
struct DetailCache {
    aid: i64,
    version: u64,
    detail: Option<AnimeDetail>,
}

/// A mylist edit saved to the outbox that no loaded projection reflects yet.
struct MylistOverlay {
    state: i64,
    viewed: bool,
    /// Projections with `generation >= this` were requested after the edit was saved.
    generation: u64,
}

#[derive(Default)]
struct Activity {
    busy: bool,
    label: String,
    current: usize,
    total: usize,
    fraction: f32,
}

/// Decides whether the settings file may be rewritten without the user asking for it.
///
/// Settings recovered from a backup leave the unreadable original on disk untouched; the
/// implicit save when the window closes must not replace it, only an explicit save may.
struct SettingsGuard {
    recovered: bool,
    user_saved: bool,
}

impl SettingsGuard {
    fn allows_exit_save(&self) -> bool {
        !self.recovered || self.user_saved
    }
}

/// Text of the startup warning shown after settings were recovered from the backup copy.
fn recovery_notice(settings_path: &Path) -> String {
    format!(
        "The settings file could not be used, so Animatic loaded the backup copy ({}) and \
         switched to offline mode.\n\nThe original file ({}) has not been changed. Animatic \
         will not write settings when it closes unless you save them yourself in Options; \
         saving replaces the original file.",
        Settings::backup_path(settings_path).display(),
        settings_path.display()
    )
}

/// A planned rename of one file, shown for confirmation.
struct RenamePreview {
    id: i64,
    old: PathBuf,
    plan: Plan,
}

/// Diagnostics state of the renaming pattern editor in Options.
#[derive(Default)]
struct PatternEditor {
    /// Pattern text the diagnostics below were computed for.
    checked: Option<String>,
    syntax_error: Option<String>,
    /// Text replaced by "Use stable default", kept so the replacement can be undone.
    replaced: Option<String>,
    /// Outcome of "Validate on selected file" for the text in `checked`.
    test: Option<Result<RenamePreview, String>>,
}

pub struct AnimaticApp {
    db: Db,
    db_path: PathBuf,
    settings_path: PathBuf,
    settings: Settings,
    projection: LibraryProjection,
    loader: ProjectionLoader,
    views: TableViews,
    detail: Option<DetailCache>,
    mylist_overlay: HashMap<i64, MylistOverlay>,
    /// Queue pushes that hit a busy database, retried until saved.
    deferred: Vec<animatic_core::sync::Command>,
    last_deferred_retry: Instant,
    /// Sync cooldown as (epoch seconds until, reason).
    cooldown: Option<(i64, String)>,
    /// Projections requested before this generation predate the latest `Cooldown` event and
    /// must not overwrite the cooldown it set.
    cooldown_floor: u64,
    /// Likewise for `Blocked` events: until a projection with generation >= this arrives the
    /// queue's blocked state is known to be newer than the one on screen.
    blocked_floor: u64,
    /// Generation of the projection on screen (0 before the first load).
    projection_generation: u64,
    worker: Worker,
    page: Page,
    query: String,
    selected_aid: Option<i64>,
    settings_open: bool,
    about_open: bool,
    selected_local: Option<i64>,
    rename_preview: Option<RenamePreview>,
    pattern_editor: PatternEditor,
    settings_guard: SettingsGuard,
    /// Startup warning about recovered settings, shown until dismissed.
    settings_notice: Option<String>,
    import_source: String,
    imported_settings: Option<Box<Settings>>,
    import_pending: bool,
    search_year: String,
    search_type: i64,
    search_mylist_only: bool,
    search_watched_only: bool,
    activity: Activity,
    status: String,
    logs: Vec<String>,
}

impl AnimaticApp {
    pub fn new(
        db_path: PathBuf,
        settings_path: PathBuf,
        initial_files: Vec<PathBuf>,
        full_update: bool,
    ) -> anyhow::Result<Self> {
        let db = Db::open(&db_path)?;
        // The worker may hold the write lock for long stretches; small UI writes must fail
        // visibly instead of freezing the window for the default 30 seconds.
        db.conn().busy_timeout(UI_DB_BUSY_TIMEOUT)?;
        let loaded = Settings::load_with_recovery(&settings_path)?;
        let settings_guard = SettingsGuard {
            recovered: loaded.recovered_from_backup,
            user_saved: false,
        };
        let settings_notice = settings_guard
            .recovered
            .then(|| recovery_notice(&settings_path));
        let settings = loaded.settings;
        let worker = Worker::spawn(db_path.clone());
        let mut app = Self {
            db,
            db_path,
            settings_path,
            settings,
            projection: LibraryProjection::default(),
            loader: ProjectionLoader {
                next_generation: 1,
                outstanding: None,
                dirty: false,
                last_request: None,
                version: 0,
            },
            views: TableViews::default(),
            detail: None,
            mylist_overlay: HashMap::new(),
            deferred: Vec::new(),
            last_deferred_retry: Instant::now(),
            cooldown: None,
            cooldown_floor: 0,
            blocked_floor: 0,
            projection_generation: 0,
            worker,
            page: Page::Dashboard,
            query: String::new(),
            selected_aid: None,
            selected_local: None,
            rename_preview: None,
            pattern_editor: PatternEditor::default(),
            settings_guard,
            settings_notice,
            settings_open: false,
            about_open: false,
            import_source: String::new(),
            imported_settings: None,
            import_pending: false,
            search_year: String::new(),
            search_type: 0,
            search_mylist_only: false,
            search_watched_only: false,
            activity: Activity::default(),
            status: LOADING_STATUS.into(),
            logs: Vec::new(),
        };
        app.request_library_refresh();
        if app.settings_notice.is_some() {
            app.error(
                "Settings were recovered from a backup copy (offline mode); the original settings \
                 file was left unchanged",
            );
        }
        if full_update {
            app.worker.submit(Job::QueueFullUpdate);
        }
        if !initial_files.is_empty() {
            app.worker.submit(Job::Hash(initial_files));
        }
        Ok(app)
    }

    /// Ask for a fresh projection. Cheap: requests are coalesced and sent by `pump_library`.
    fn request_library_refresh(&mut self) {
        self.loader.dirty = true;
    }

    fn stats_export(&self) -> Option<(PathBuf, String, StatFormat)> {
        let stats = &self.settings.stat_whore;
        (stats.enabled && !stats.file.is_empty()).then(|| {
            (
                PathBuf::from(&stats.file),
                self.settings.anidb.username.clone(),
                stats.format,
            )
        })
    }

    /// Send the next projection request when none is outstanding and the interval elapsed.
    fn pump_library(&mut self, ctx: &egui::Context) {
        if !self.loader.dirty || self.loader.outstanding.is_some() {
            return;
        }
        if let Some(last) = self.loader.last_request {
            let elapsed = last.elapsed();
            if elapsed < RELOAD_INTERVAL {
                ctx.request_repaint_after(RELOAD_INTERVAL - elapsed);
                return;
            }
        }
        let generation = self.loader.next_generation;
        self.loader.next_generation += 1;
        self.loader.dirty = false;
        self.loader.outstanding = Some(generation);
        self.loader.last_request = Some(Instant::now());
        let export = self.stats_export();
        self.worker.submit(Job::LoadLibrary { generation, export });
    }

    fn apply_projection(&mut self, generation: u64, projection: LibraryProjection) {
        self.loader.outstanding = None;
        self.loader.version += 1;
        if generation >= self.cooldown_floor {
            self.cooldown = projection.cooldown.clone();
        }
        self.projection_generation = generation;
        self.projection = projection;
        self.mylist_overlay.retain(|_, o| o.generation > generation);
        if self.status == LOADING_STATUS {
            self.status = "Ready".into();
        }
    }

    /// A `Blocked` event is newer than the projection on screen.
    fn blocked_unconfirmed(&self) -> bool {
        self.projection_generation < self.blocked_floor
    }

    /// The sync cooldown still in force, as (epoch seconds until, reason).
    fn active_cooldown(&self) -> Option<(i64, &str)> {
        let (until, reason) = self.cooldown.as_ref()?;
        (*until > chrono::Utc::now().timestamp()).then_some((*until, reason.as_str()))
    }

    fn cooldown_text(until: i64, reason: &str) -> String {
        let when = chrono::DateTime::from_timestamp(until, 0)
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%H:%M:%S")
                    .to_string()
            })
            .unwrap_or_else(|| until.to_string());
        format!("AniDB sync paused until {when}: {reason}")
    }

    /// Persist a command. If the worker holds the database lock the command is only held in
    /// memory: it is shown as unsaved (bottom bar) and retried until the database accepts it.
    fn queue_command(&mut self, cmd: animatic_core::sync::Command) {
        if self.deferred.contains(&cmd) {
            return;
        }
        match self.db.queue_push(&cmd) {
            Ok(_) => self.request_library_refresh(),
            Err(e) if is_busy(&e) => {
                if self.deferred.len() >= DEFERRED_LIMIT {
                    self.error(format!(
                        "NOT queued: {} (database busy and {DEFERRED_LIMIT} commands are already unsaved)",
                        cmd.describe()
                    ));
                    return;
                }
                if self.deferred.is_empty() {
                    self.last_deferred_retry = Instant::now();
                    self.log("Database busy: queued command is NOT saved yet, retrying");
                }
                self.deferred.push(cmd);
            }
            Err(e) => self.error(format!("Could not queue {}: {e}", cmd.describe())),
        }
    }

    fn retry_deferred(&mut self) {
        if self.deferred.is_empty() || self.last_deferred_retry.elapsed() < DEFERRED_RETRY {
            return;
        }
        self.last_deferred_retry = Instant::now();
        let saved = self.flush_deferred();
        if saved > 0 {
            self.log(format!("Saved {saved} previously unsaved commands"));
        }
    }

    /// Try to save every deferred command, in order; stops at the first busy database.
    /// Returns how many were saved.
    fn flush_deferred(&mut self) -> usize {
        let pending = std::mem::take(&mut self.deferred);
        let mut pending = pending.into_iter();
        let mut saved = 0;
        while let Some(cmd) = pending.next() {
            match self.db.queue_push(&cmd) {
                Ok(_) => {
                    saved += 1;
                    self.request_library_refresh();
                }
                Err(e) if is_busy(&e) => {
                    // Still locked: keep the rest in order rather than stalling on each.
                    self.deferred.push(cmd);
                    self.deferred.extend(pending);
                    return saved;
                }
                Err(e) => self.error(format!("Could not queue {}: {e}", cmd.describe())),
            }
        }
        saved
    }

    fn db_write_error(&mut self, what: &str, e: &animatic_core::Error) {
        if is_busy(e) {
            self.error(format!(
                "{what}: the database is busy with a background write; nothing was changed, try again"
            ));
        } else {
            self.error(format!("{what}: {e}"));
        }
    }

    fn log(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.status = text.clone();
        self.logs.push(format!(
            "{}  {text}",
            chrono::Local::now().format("%H:%M:%S")
        ));
        if self.logs.len() > 2_000 {
            self.logs.drain(..500);
        }
    }

    fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.status = text.clone();
        self.logs.push(format!(
            "{}  ERROR  {text}",
            chrono::Local::now().format("%H:%M:%S")
        ));
    }

    fn process_events(&mut self, ctx: &egui::Context) {
        let events: Vec<_> = self.worker.events.try_iter().collect();
        for event in events {
            match event {
                Event::Started { label, total } => {
                    self.activity = Activity {
                        busy: true,
                        label: label.clone(),
                        current: 0,
                        total,
                        fraction: 0.0,
                    };
                    self.log(label);
                }
                Event::Progress {
                    label,
                    current,
                    total,
                    fraction,
                } => {
                    self.activity.label = label;
                    self.activity.current = current;
                    self.activity.total = total;
                    self.activity.fraction = fraction;
                }
                Event::Hashed {
                    path,
                    local_id,
                    size,
                    ed2k,
                } => {
                    self.log(format!(
                        "Hashed {} (local #{local_id}, {ed2k})",
                        path.display()
                    ));
                    if self.settings.hashing.identify_after_hashing {
                        let manager = self.settings.file_managers.manager_for(&path);
                        let add = manager
                            .mylist_auto_add
                            .then(|| animatic_core::sync::manager_mylist_edit(manager));
                        self.queue_command(animatic_core::sync::Command::Identify {
                            size,
                            ed2k,
                            add,
                        });
                    }
                    self.request_library_refresh();
                }
                Event::Imported { summary } => {
                    self.log(summary);
                    self.request_library_refresh();
                }
                Event::LegacySettingsImported(settings) => {
                    self.imported_settings = Some(settings);
                }
                Event::LegacySettingsImportFailed(e) => {
                    self.import_pending = false;
                    self.error(e);
                }
                Event::Synced {
                    description,
                    changed,
                } => {
                    self.log(description);
                    if changed {
                        self.request_library_refresh();
                    }
                }
                Event::Blocked { description } => {
                    self.error(format!("Command blocked: {description}"));
                    self.blocked_floor = self.loader.next_generation;
                    self.request_library_refresh();
                }
                Event::Cooldown { until, reason } => {
                    self.error(Self::cooldown_text(until, &reason));
                    self.cooldown = Some((until, reason));
                    self.cooldown_floor = self.loader.next_generation;
                    self.request_library_refresh();
                }
                // Not a dirty signal: statistics export failures arrive as errors and would
                // otherwise re-request the projection (and the export) forever.
                Event::Error(e) => self.error(e),
                Event::Finished => {
                    self.activity.busy = false;
                    self.status = "Ready".into();
                    self.request_library_refresh();
                }
                Event::LibraryLoaded {
                    generation,
                    projection,
                } => {
                    if self.loader.outstanding == Some(generation) {
                        self.apply_projection(generation, *projection);
                    }
                }
                Event::LibraryLoadFailed {
                    generation,
                    message,
                } => {
                    if self.loader.outstanding == Some(generation) {
                        self.loader.outstanding = None;
                        self.error(format!("Unable to refresh library: {message}"));
                    }
                }
            }
            ctx.request_repaint();
        }
    }

    fn submit_files(&mut self) {
        if self.import_pending {
            return;
        }
        let mut dialog = rfd::FileDialog::new().set_title("Add files to hash");
        if !self.settings.ui.last_directory.is_empty() {
            dialog = dialog.set_directory(&self.settings.ui.last_directory);
        }
        if self.settings.hashing.use_extensions {
            let ext: Vec<&str> = self
                .settings
                .hashing
                .extensions
                .iter()
                .map(String::as_str)
                .collect();
            dialog = dialog.add_filter("Configured media files", &ext);
        }
        if let Some(files) = dialog.pick_files() {
            if let Some(parent) = files.first().and_then(|p| p.parent()) {
                self.settings.ui.last_directory = parent.to_string_lossy().into_owned();
            }
            self.worker.submit(Job::Hash(files));
        }
    }
    fn import_legacy_settings(&mut self) {
        if self.import_pending {
            return;
        }
        let mut dialog = rfd::FileDialog::new()
            .set_title("Choose AOM local_settings.xml")
            .add_filter("AOM settings XML", &["xml"]);
        if !self.settings.ui.last_directory.is_empty() {
            dialog = dialog.set_directory(&self.settings.ui.last_directory);
        }
        let Some(source) = dialog.pick_file() else {
            return;
        };
        self.import_pending = true;
        self.worker.cancel.store(true, Ordering::Relaxed);
        let _ = self.worker.jobs.send(Job::ImportLegacySettings { source });
    }

    fn legacy_settings_review_window(&mut self, ctx: &egui::Context) {
        let Some(imported) = self.imported_settings.as_ref() else {
            return;
        };
        let managed_folders = imported.file_managers.managers.len();
        let players = imported.players.len();
        let extensions = imported.hashing.extensions.len();
        let credentials_detected = !imported.anidb.username.is_empty()
            || !imported.anidb.password.is_empty()
            || !imported.avdump.key.is_empty();
        let mut open = true;
        let mut apply = false;
        let mut cancelled = false;
        egui::Window::new("Review imported AOM settings")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.strong("Applying will replace your current Animatic settings.");
                ui.colored_label(
                    Color32::YELLOW,
                    "Imported credentials will not connect to AniDB: offline mode remains enabled.",
                );
                ui.label("Review managed folders before scanning them.");
                ui.separator();
                ui.label(format!("Managed folders: {managed_folders}"));
                ui.label(format!("External players: {players}"));
                ui.label(format!("Hash file extensions: {extensions}"));
                ui.label(if credentials_detected {
                    "Imported credentials were detected and will require review."
                } else {
                    "No imported credentials were detected."
                });
                ui.add_space(8.0);
                if ui.button("Apply imported settings").clicked() {
                    apply = true;
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
        if apply {
            let mut imported = *self.imported_settings.take().expect("reviewed settings");
            imported.anidb.offline = true;
            match imported.save(&self.settings_path) {
                Ok(()) => {
                    self.settings_guard.user_saved = true;
                    self.settings = imported;
                    self.worker.cancel.store(true, Ordering::Relaxed);
                    self.import_pending = false;
                    self.log("Settings saved");
                    self.apply_visuals(ctx);
                }
                Err(e) => {
                    self.error(format!("Could not save settings: {e}"));
                    self.imported_settings = Some(Box::new(imported));
                }
            }
        } else if cancelled || !open {
            self.imported_settings = None;
            self.import_pending = false;
        }
    }

    fn import_legacy(&mut self) {
        if self.import_pending {
            return;
        }
        let source = PathBuf::from(self.import_source.trim());
        if !animatic_core::legacy::import::is_databank(&source) {
            self.error(format!("No AOM databank found in {}", source.display()));
            return;
        }
        let temp = self
            .db_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("import-temp");
        self.worker.submit(Job::ImportLegacy { source, temp });
    }

    /// Plan the rename of local file `id` with `pattern` without touching the file system.
    fn preview_rename(&self, id: i64, pattern: &str) -> Result<RenamePreview, String> {
        let local = self
            .projection
            .local
            .iter()
            .find(|f| f.id == id)
            .ok_or("The selected file is no longer in the list")?;
        let fid = local
            .fid
            .ok_or("The selected file is not identified by AniDB")?;
        let old = PathBuf::from(&local.path);
        let manager = self.settings.file_managers.manager_for(&old);
        let plan = match animatic_core::rename::objects::bindings_for_file(
            &self.db,
            fid,
            &local.path,
            &manager.base_path,
        ) {
            Ok(bindings) => plan::plan(
                pattern,
                &bindings,
                &animatic_core::rename::objects::DbSource { db: &self.db },
                &old,
            ),
            Err(e) => Plan {
                errors: vec![e],
                ..Plan::default()
            },
        };
        Ok(RenamePreview { id, old, plan })
    }

    fn prepare_rename(&mut self) {
        let Some(id) = self.selected_local else {
            self.error("Select a hashed file first");
            return;
        };
        match self.preview_rename(id, &self.settings.renaming_pattern) {
            Ok(preview) => {
                if let Some(problem) = preview.plan.errors.first() {
                    let text = format!("Rename not possible: {problem}");
                    self.error(text);
                } else if !preview.plan.warnings.is_empty() {
                    self.error("Rename blocked: the renaming pattern reported warnings");
                } else if preview.plan.already_named() {
                    self.log("The file already has the name the pattern produces");
                }
                self.rename_preview = Some(preview);
            }
            Err(e) => self.error(e),
        }
    }

    fn rename_window(&mut self, ctx: &egui::Context) {
        let Some(preview) = self.rename_preview.as_ref() else {
            return;
        };
        let (id, old, plan) = (preview.id, preview.old.clone(), preview.plan.clone());
        let mut open = true;
        let mut apply = false;
        let mut edit_pattern = false;
        egui::Window::new("Confirm rename")
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Current path");
                ui.monospace(old.display().to_string());
                if let Some(new) = &plan.target {
                    ui.add_space(8.0);
                    ui.label(if plan.can_apply() {
                        "New path"
                    } else {
                        "Path the pattern would produce (rename blocked)"
                    });
                    ui.monospace(new.display().to_string());
                } else if plan.already_named() {
                    ui.add_space(8.0);
                    ui.label(
                        "The file already has the name the pattern produces; nothing to rename.",
                    );
                }
                show_plan(ui, &plan);
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(plan.can_apply(), egui::Button::new("Rename file"))
                        .clicked()
                    {
                        apply = true;
                    }
                    if (!plan.errors.is_empty() || !plan.warnings.is_empty())
                        && ui.button("Edit pattern in Options").clicked()
                    {
                        edit_pattern = true;
                    }
                });
            });
        if apply {
            if let Some(new) = plan.target.as_ref().filter(|_| plan.can_apply()) {
                match animatic_core::file_manager::move_file(&self.db, id, &old, new) {
                    Ok(()) => {
                        self.log(format!("Renamed {} to {}", old.display(), new.display()));
                        self.request_library_refresh();
                    }
                    Err(e) => self.error(format!("Rename failed: {e}")),
                }
            }
            self.rename_preview = None;
        } else if edit_pattern {
            self.settings_open = true;
            self.rename_preview = None;
        } else if !open {
            self.rename_preview = None;
        }
    }

    /// Options section of the renaming pattern: diagnostics, the stable default and a dry run.
    fn pattern_editor_ui(&mut self, ui: &mut egui::Ui) {
        let kind = plan::classify(&self.settings.renaming_pattern);
        match kind {
            PatternKind::StableDefault => {
                ui.label(
                    "Stable built-in default: episode numbers always have at least two digits \
                     (01, 02, … 100), whatever the cached episode counts.",
                );
            }
            PatternKind::Custom => {
                ui.label(
                    "Custom pattern: kept exactly as written. \"Use stable default\" would \
                     replace it with the built-in pattern, whose episode numbers have a fixed \
                     two-digit minimum.",
                );
            }
            PatternKind::Empty => {
                ui.colored_label(
                    Color32::LIGHT_RED,
                    "The pattern is empty, so no file can be renamed.",
                );
            }
        }
        let mut use_stable = false;
        let mut restore = false;
        let mut validate = false;
        ui.horizontal_wrapped(|ui| {
            use_stable = ui
                .add_enabled(
                    kind != PatternKind::StableDefault,
                    egui::Button::new("Use stable default"),
                )
                .on_hover_text(
                    "Replace the text below with the built-in default. Its episode numbers use a \
                     fixed two-digit minimum instead of the width of the cached episode total. \
                     Nothing is stored until you press Save options.",
                )
                .clicked();
            if self.pattern_editor.replaced.is_some() {
                restore = ui.button("Restore previous text").clicked();
            }
            validate = ui
                .button("Validate on selected file")
                .on_hover_text("Run the pattern on the file selected on the Hashed files page")
                .clicked();
        });
        if use_stable {
            let old = std::mem::replace(
                &mut self.settings.renaming_pattern,
                animatic_core::settings::DEFAULT_RENAME_PATTERN.to_string(),
            );
            self.pattern_editor.replaced = Some(old);
        } else if restore && let Some(old) = self.pattern_editor.replaced.take() {
            self.settings.renaming_pattern = old;
        }
        ui.add(
            egui::TextEdit::multiline(&mut self.settings.renaming_pattern)
                .font(egui::TextStyle::Monospace)
                .desired_rows(12),
        );
        if self.pattern_editor.checked.as_deref() != Some(self.settings.renaming_pattern.as_str()) {
            self.pattern_editor.syntax_error =
                plan::check_pattern(&self.settings.renaming_pattern).err();
            self.pattern_editor.checked = Some(self.settings.renaming_pattern.clone());
            self.pattern_editor.test = None;
        }
        if validate {
            let test = match self.selected_local {
                None => Err(
                    "Select a file identified by AniDB on the Hashed files page, then \
                             validate again."
                        .to_string(),
                ),
                Some(id) => self.preview_rename(id, &self.settings.renaming_pattern),
            };
            self.pattern_editor.test = Some(test);
        }
        if let Some(error) = &self.pattern_editor.syntax_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        match &self.pattern_editor.test {
            Some(Err(e)) => {
                ui.colored_label(Color32::LIGHT_RED, e);
            }
            Some(Ok(preview)) => {
                if !preview.plan.result.is_empty() {
                    ui.label("Result for the selected file:");
                    ui.monospace(&preview.plan.result);
                }
                show_plan(ui, &preview.plan);
                if preview.plan.can_apply() || preview.plan.already_named() {
                    ui.colored_label(Color32::LIGHT_GREEN, "The pattern is valid for this file.");
                }
            }
            None => {}
        }
    }

    fn settings_notice_window(&mut self, ctx: &egui::Context) {
        let Some(text) = self.settings_notice.as_ref() else {
            return;
        };
        let mut dismissed = false;
        egui::Window::new("Settings recovered")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.colored_label(Color32::YELLOW, text);
                ui.add_space(8.0);
                if ui.button("OK").clicked() {
                    dismissed = true;
                }
            });
        if dismissed {
            self.settings_notice = None;
        }
    }

    fn play_selected(&mut self, action: animatic_core::external::PlayerAction) {
        let Some(file) = self
            .projection
            .local
            .iter()
            .find(|f| Some(f.id) == self.selected_local)
        else {
            return;
        };
        let Some(player) = self
            .settings
            .players
            .iter()
            .find(|p| p.default)
            .or_else(|| self.settings.players.first())
        else {
            self.error("No external player is configured");
            return;
        };
        if let Err(e) =
            animatic_core::external::launch_player(player, action, Path::new(&file.path))
        {
            self.error(format!("Could not start player: {e}"));
        }
    }

    fn avdump_selected(&mut self) {
        let Some(file) = self
            .projection
            .local
            .iter()
            .find(|f| Some(f.id) == self.selected_local)
        else {
            return;
        };
        if let Err(e) = animatic_core::external::launch_avdump(
            &self.settings.avdump,
            Path::new(&file.path),
            &self.settings.anidb.username,
            self.settings.anidb.local_port,
        ) {
            self.error(format!("Could not start AVDump: {e}"));
        }
    }

    fn export_statistics(&mut self) {
        let extension = match self.settings.stat_whore.format {
            animatic_core::settings::StatFormat::Xml => "xml",
            animatic_core::settings::StatFormat::Ini => "ini",
        };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("animatic-stats.{extension}"))
            .save_file()
        else {
            return;
        };
        // The worker runs the library-wide queries and reports success or the typed error.
        self.worker.submit(Job::ExportStatistics {
            path,
            username: self.settings.anidb.username.clone(),
            format: self.settings.stat_whore.format,
        });
    }

    fn export_hashes(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("animatic-hashes.txt")
            .save_file()
        else {
            return;
        };
        let mut text = String::new();
        for f in &self.projection.local {
            let hashes = animatic_core::hash::Hashes {
                size: f.size as u64,
                ed2k: f.ed2k.clone(),
                crc32: f.crc32.clone().unwrap_or_default(),
                md5: f.md5.clone().unwrap_or_default(),
                sha1: f.sha1.clone().unwrap_or_default(),
            };
            text.push_str(&hashes.export(
                std::path::Path::new(&f.path),
                &self.settings.hashing.export_template,
                self.settings.ui.copy_length_as_minutes,
            ));
            text.push_str("\r\n");
        }
        match std::fs::write(&path, text) {
            Ok(()) => self.log(format!(
                "Exported {} hashes to {}",
                self.projection.local.len(),
                path.display()
            )),
            Err(e) => self.error(format!("Hash export failed: {e}")),
        }
    }

    fn save_settings(&mut self) {
        match self.settings.save(&self.settings_path) {
            Ok(()) => {
                self.settings_guard.user_saved = true;
                self.settings_notice = None;
                self.log("Settings saved");
            }
            Err(e) => self.error(format!("Could not save settings: {e}")),
        }
    }

    /// Store settings after a change the user did not ask to save (for example dismissing the
    /// first-run card). Recovered settings keep the unreadable original untouched, so the change
    /// then stays in memory until Options "Save" or an applied import; no save state changes.
    fn persist_settings_incidentally(&mut self) {
        if !self.settings_guard.allows_exit_save() {
            return;
        }
        if let Err(e) = self.settings.save(&self.settings_path) {
            self.error(format!("Could not save settings: {e}"));
        }
    }

    fn top_bar(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        let cooling = self
            .active_cooldown()
            .map(|(until, reason)| Self::cooldown_text(until, reason));
        egui::Panel::top("menu").show(root, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui
                        .add_enabled(!self.import_pending, egui::Button::new("Add files…"))
                        .clicked()
                    {
                        ui.close();
                        self.submit_files();
                    }
                    if ui
                        .add_enabled(
                            !self.import_pending,
                            egui::Button::new("Import AOM databank…"),
                        )
                        .clicked()
                    {
                        ui.close();
                        if let Some(dir) = rfd::FileDialog::new()
                            .set_title("Choose AOM kowai folder")
                            .pick_folder()
                        {
                            self.import_source = dir.to_string_lossy().into_owned();
                            self.import_legacy();
                        }
                    }
                    if ui
                        .add_enabled(
                            !self.import_pending,
                            egui::Button::new("Import AOM settings…"),
                        )
                        .clicked()
                    {
                        ui.close();
                        self.import_legacy_settings();
                    }
                    if ui.button("Export statistics…").clicked() {
                        ui.close();
                        self.export_statistics();
                    }
                    if ui.button("Export hashes…").clicked() {
                        ui.close();
                        self.export_hashes();
                    }
                    if ui.button("Refresh").clicked() {
                        ui.close();
                        self.request_library_refresh();
                    }
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Tools", |ui| {
                    if ui.button("Options…").clicked() {
                        self.settings_open = true;
                        ui.close();
                    }
                    ui.add_enabled_ui(!self.import_pending, |ui| {
                        if ui.button("Scan managed folders").clicked() {
                            self.worker.submit(Job::Scan {
                                settings: self.settings.file_managers.clone(),
                                extensions: self.settings.hashing.extensions.join(","),
                            });
                            ui.close();
                        }
                    });
                    if ui.button("Download anime titles").clicked() {
                        self.queue_command(animatic_core::sync::Command::TitlesDump);
                        ui.close();
                    }
                    if ui.button("Fetch mylist statistics").clicked() {
                        self.queue_command(animatic_core::sync::Command::MylistStats);
                        ui.close();
                    }
                    ui.menu_button("Edit AniDB data", |ui| {
                        for (label, url) in [
                            ("Add anime…", "https://anidb.net/anime/add"),
                            ("Add episode…", "https://anidb.net/episode/add"),
                            ("Add file…", "https://anidb.net/file/add"),
                            ("Submit change request…", "https://anidb.net/creq"),
                        ] {
                            if ui.button(label).clicked() {
                                if let Err(e) = open::that(url) {
                                    self.error(format!("Could not open AniDB: {e}"));
                                }
                                ui.close();
                            }
                        }
                    });
                    let run = ui
                        .add_enabled(
                            !self.import_pending && cooling.is_none(),
                            egui::Button::new("Run command queue"),
                        )
                        .on_disabled_hover_text(cooling.as_deref().unwrap_or_default());
                    if run.clicked() {
                        self.worker
                            .submit(Job::ProcessQueue(self.settings.anidb.clone()));
                        ui.close();
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("AniDB website").clicked() {
                        let _ = open::that("https://anidb.net/");
                        ui.close();
                    }
                    if ui.button("About Animatic").clicked() {
                        self.about_open = true;
                        ui.close();
                    }
                });
            });
        });
    }

    fn navigation(&mut self, root: &mut egui::Ui) {
        egui::Panel::left("navigation")
            .resizable(false)
            .default_size(165.0)
            .show(root, |ui| {
                ui.add_space(10.0);
                ui.heading(RichText::new("Animatic").size(24.0));
                ui.label(RichText::new("AniDB library manager").weak());
                ui.add_space(16.0);
                for (page, label) in Page::ALL {
                    if ui.selectable_label(self.page == page, label).clicked() {
                        self.page = page;
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    ui.label(format!("{} anime", self.projection.anime.len()));
                    ui.label(format!("{} mylist files", self.projection.mylist.len()));
                });
            });
    }

    fn page_header(&mut self, ui: &mut egui::Ui, title: &str, searchable: bool) {
        ui.horizontal(|ui| {
            ui.heading(title);
            if searchable {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_sized(
                        [260.0, 28.0],
                        egui::TextEdit::singleline(&mut self.query).hint_text("Search…"),
                    );
                });
            }
        });
        ui.separator();
    }

    fn dashboard(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "Library overview", false);
        let summary = self.projection.summary;
        let known = summary.local_known;
        let local_total = self.projection.local.len();
        ui.columns(4, |cols| {
            metric(
                &mut cols[0],
                "Anime",
                self.projection.anime.len().to_string(),
            );
            metric(
                &mut cols[1],
                "Mylist files",
                self.projection.mylist.len().to_string(),
            );
            metric(&mut cols[2], "Watched", summary.mylist_watched.to_string());
            metric(
                &mut cols[3],
                "Local size",
                format::size_short(summary.local_bytes),
            );
        });
        ui.add_space(20.0);
        ui.heading("Local library health");
        ui.label(format!(
            "{known} of {local_total} hashed files identified by AniDB"
        ));
        let bad = summary.local_crc_mismatch;
        if bad > 0 {
            ui.colored_label(
                Color32::from_rgb(235, 100, 100),
                format!("{bad} files have a CRC mismatch"),
            );
        }
        let unknown = local_total.saturating_sub(known);
        if unknown > 0 {
            ui.label(format!("{unknown} files are not identified yet"));
        }
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.import_pending, egui::Button::new("Add files"))
                .clicked()
            {
                self.submit_files();
            }
            if ui.button("Open hashed files").clicked() {
                self.page = Page::Hashed;
            }
            if ui.button("Refresh data").clicked() {
                self.request_library_refresh();
            }
        });
        if self.settings.first_launch {
            ui.add_space(24.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("Bring your AniDB O'Matic library forward");
                ui.label("Import the encrypted databank directly. Existing anime, mylist, hashes and local paths are preserved.");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.import_source);
                    ui.add_enabled_ui(!self.import_pending, |ui| {
                        if ui.button("Browse…").clicked()
                            && let Some(d) = rfd::FileDialog::new().pick_folder()
                        {
                            self.import_source = d.to_string_lossy().into_owned();
                        }
                        if ui.button("Import").clicked() {
                            self.import_legacy();
                        }
                    });
                });
                if ui.button("Dismiss first-run card").clicked() {
                    self.settings.first_launch = false;
                    self.persist_settings_incidentally();
                }
            });
        }
    }

    fn anime_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "Anime database", true);
        let key = (self.loader.version, self.query.to_ascii_lowercase());
        {
            let Self {
                projection,
                views,
                selected_aid,
                ..
            } = self;
            views
                .anime
                .refresh(&key, || filter_anime(projection, &key.1));
            let rows = &views.anime.rows;
            let row_height = row_height(ui);
            data_table(
                ui,
                "anime_table",
                &[
                    ("Title", 360.0),
                    ("Type", 100.0),
                    ("Year", 80.0),
                    ("Episodes", 90.0),
                    ("Rating", 80.0),
                    ("Mylist", 160.0),
                ],
            )
            .body(|body| {
                body.rows(row_height, rows.len(), |mut row| {
                    let a = &projection.anime[rows[row.index()]];
                    let state = projection.states.get(&a.aid);
                    row.col(|ui| {
                        let title = anime_title(a);
                        if ui
                            .selectable_label(*selected_aid == Some(a.aid), title.as_ref())
                            .clicked()
                        {
                            *selected_aid = Some(a.aid);
                        }
                    });
                    row.col(|ui| {
                        ui.label(a.type_name());
                    });
                    row.col(|ui| {
                        ui.label(&a.year);
                    });
                    row.col(|ui| {
                        ui.label(a.episodes.to_string());
                    });
                    row.col(|ui| {
                        ui.label(format::rating(a.rating));
                    });
                    row.col(|ui| {
                        ui.label(
                            state
                                .map(|s| format!("{} eps / {} files", s.eps, s.files))
                                .unwrap_or_default(),
                        );
                    });
                });
            });
        }
        self.anime_detail_window(ui.ctx());
    }

    fn search_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "Advanced anime search", false);
        ui.horizontal_wrapped(|ui| {
            ui.add_sized(
                [260.0, 28.0],
                egui::TextEdit::singleline(&mut self.query).hint_text("Title or AniDB ID"),
            );
            ui.label("Year");
            ui.add_sized(
                [80.0, 28.0],
                egui::TextEdit::singleline(&mut self.search_year).hint_text("e.g. 2012"),
            );
            egui::ComboBox::from_id_salt("advanced-type")
                .selected_text(match self.search_type {
                    2 => "TV Series",
                    3 => "OVA",
                    4 => "Movie",
                    5 => "Other",
                    6 => "Web",
                    7 => "TV Special",
                    8 => "Music Video",
                    _ => "Any type",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.search_type, 0, "Any type");
                    for (id, name) in [
                        (2, "TV Series"),
                        (3, "OVA"),
                        (4, "Movie"),
                        (5, "Other"),
                        (6, "Web"),
                        (7, "TV Special"),
                        (8, "Music Video"),
                    ] {
                        ui.selectable_value(&mut self.search_type, id, name);
                    }
                });
            ui.checkbox(&mut self.search_mylist_only, "In mylist");
            ui.checkbox(&mut self.search_watched_only, "Fully watched");
        });
        ui.separator();
        let key = (
            self.loader.version,
            self.query.to_ascii_lowercase(),
            self.search_year.trim().to_string(),
            self.search_type,
            self.search_mylist_only,
            self.search_watched_only,
        );
        {
            let Self {
                projection,
                views,
                selected_aid,
                ..
            } = self;
            views.search.refresh(&key, || {
                filter_search(
                    projection,
                    &SearchFilter {
                        query: &key.1,
                        year: &key.2,
                        type_id: key.3,
                        mylist_only: key.4,
                        watched_only: key.5,
                    },
                )
            });
            let rows = &views.search.rows;
            let row_height = row_height(ui);
            data_table(
                ui,
                "advanced-results",
                &[
                    ("Title", 360.0),
                    ("Type", 100.0),
                    ("Year", 80.0),
                    ("Episodes", 90.0),
                    ("Rating", 80.0),
                    ("Mylist", 160.0),
                ],
            )
            .body(|body| {
                body.rows(row_height, rows.len(), |mut row| {
                    let a = &projection.anime[rows[row.index()]];
                    let state = projection.states.get(&a.aid);
                    row.col(|ui| {
                        let title = anime_title(a);
                        if ui.link(title.as_ref()).clicked() {
                            *selected_aid = Some(a.aid);
                        }
                    });
                    row.col(|ui| {
                        ui.label(a.type_name());
                    });
                    row.col(|ui| {
                        ui.label(&a.year);
                    });
                    row.col(|ui| {
                        ui.label(a.episodes.to_string());
                    });
                    row.col(|ui| {
                        ui.label(format::rating(a.rating));
                    });
                    row.col(|ui| {
                        ui.label(
                            state
                                .map(|s| format!("{} eps / {} watched", s.eps, s.watched_eps))
                                .unwrap_or_default(),
                        );
                    });
                });
            });
        }
        self.anime_detail_window(ui.ctx());
    }

    fn mylist_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "My list", true);
        let key = (self.loader.version, self.query.to_ascii_lowercase());
        // (lid, edit, resulting state, resulting watched flag)
        let mut edits: Vec<(i64, animatic_anidb::commands::MylistEdit, i64, bool)> = Vec::new();
        {
            let Self {
                projection,
                views,
                selected_aid,
                mylist_overlay,
                ..
            } = self;
            views
                .mylist
                .refresh(&key, || filter_mylist(projection, &key.1));
            let rows = &views.mylist.rows;
            let row_height = row_height(ui);
            data_table(
                ui,
                "mylist_table",
                &[
                    ("Anime", 320.0),
                    ("Episode", 90.0),
                    ("Group", 200.0),
                    ("State", 120.0),
                    ("Watched", 80.0),
                    ("Size", 100.0),
                ],
            )
            .body(|body| {
                body.rows(row_height, rows.len(), |mut row| {
                    let i = rows[row.index()];
                    let item = &projection.mylist[i];
                    let lid = item.entry.lid;
                    // Show a toggle the user just saved even before a projection includes it.
                    let (shown_state, shown_viewed) = mylist_overlay
                        .get(&lid)
                        .map_or((item.entry.state, item.entry.viewed), |o| {
                            (o.state, o.viewed)
                        });
                    let mut state = shown_state;
                    let mut watched = shown_viewed;
                    row.col(|ui| {
                        if ui.link(projection.mylist_title(i)).clicked() {
                            *selected_aid = Some(item.entry.aid);
                        }
                    });
                    row.col(|ui| {
                        ui.label(
                            item.episode
                                .as_ref()
                                .map(|e| e.epno_str())
                                .unwrap_or_default(),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            item.file
                                .as_ref()
                                .map(|f| f.group_label())
                                .unwrap_or_default(),
                        );
                    });
                    row.col(|ui| {
                        egui::ComboBox::from_id_salt(("state", lid))
                            .selected_text(mylist_state_name(state))
                            .show_ui(ui, |ui| {
                                for (id, name) in MYLIST_STATES.iter().enumerate() {
                                    ui.selectable_value(&mut state, id as i64, *name);
                                }
                            });
                    });
                    row.col(|ui| {
                        ui.checkbox(&mut watched, "");
                    });
                    row.col(|ui| {
                        ui.label(
                            item.file
                                .as_ref()
                                .map(|f| format::size_short(f.size))
                                .unwrap_or_default(),
                        );
                    });
                    if state != shown_state || watched != shown_viewed {
                        edits.push((
                            lid,
                            animatic_anidb::commands::MylistEdit {
                                state: (state != shown_state).then_some(state),
                                viewed: (watched != shown_viewed).then_some(watched),
                                viewdate: None,
                                storage: None,
                                source: None,
                                other: None,
                            },
                            state,
                            watched,
                        ));
                    }
                });
            });
        }
        for (lid, edit, state, viewed) in edits {
            // One atomic outbox + optimistic-row transaction; the overlay only bridges the
            // time until a projection requested after this write arrives.
            match self.db.queue_edit_mylist(lid, &edit) {
                Ok(0) => self.log(format!(
                    "Mylist entry {lid} is pending removal; edit ignored"
                )),
                Ok(_) => {
                    self.mylist_overlay.insert(
                        lid,
                        MylistOverlay {
                            state,
                            viewed,
                            generation: self.loader.next_generation,
                        },
                    );
                    self.request_library_refresh();
                }
                Err(e) => self.db_write_error(&format!("Could not edit mylist entry {lid}"), &e),
            }
        }
        self.anime_detail_window(ui.ctx());
    }

    fn hashed_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "Hashed files", true);
        let key = (self.loader.version, self.query.to_ascii_lowercase());
        {
            let Self {
                projection,
                views,
                selected_local,
                ..
            } = self;
            if views
                .local
                .refresh(&key, || filter_local(projection, &key.1))
                && let Some(id) = *selected_local
                && !views
                    .local
                    .rows
                    .iter()
                    .any(|&i| projection.local[i].id == id)
            {
                // Never leave an invisible selection that Rename/Open/Play would act on.
                *selected_local = None;
            }
        }
        ui.horizontal(|ui| {
            if ui.button("Add files…").clicked() {
                self.submit_files();
            }
            if ui
                .add_enabled(self.selected_local.is_some(), egui::Button::new("Rename…"))
                .clicked()
            {
                self.prepare_rename();
            }
            if ui
                .add_enabled(self.selected_local.is_some(), egui::Button::new("Open"))
                .clicked()
                && let Some(f) = self
                    .projection
                    .local
                    .iter()
                    .find(|f| Some(f.id) == self.selected_local)
            {
                let _ = open::that(&f.path);
            }
            if ui
                .add_enabled(self.selected_local.is_some(), egui::Button::new("Play"))
                .clicked()
            {
                self.play_selected(animatic_core::external::PlayerAction::Play);
            }
            if ui
                .add_enabled(
                    self.selected_local.is_some() && self.settings.avdump.enabled,
                    egui::Button::new("AVDump"),
                )
                .clicked()
            {
                self.avdump_selected();
            }
            let shown = self.views.local.rows.len();
            let total = self.projection.local.len();
            ui.label(if shown == total {
                format!("{total} files")
            } else {
                format!("{shown} of {total} files")
            });
        });
        {
            let Self {
                projection,
                views,
                selected_local,
                ..
            } = self;
            let rows = &views.local.rows;
            let row_height = row_height(ui);
            data_table(
                ui,
                "local_table",
                &[
                    ("File", 300.0),
                    ("Size", 90.0),
                    ("AniDB", 90.0),
                    ("Mylist", 90.0),
                    ("CRC", 90.0),
                    ("Path", 400.0),
                ],
            )
            .body(|body| {
                body.rows(row_height, rows.len(), |mut row| {
                    let f = &projection.local[rows[row.index()]];
                    row.col(|ui| {
                        if ui
                            .selectable_label(*selected_local == Some(f.id), f.file_name())
                            .clicked()
                        {
                            *selected_local = Some(f.id);
                        }
                    });
                    row.col(|ui| {
                        ui.label(format::size_short(f.size));
                    });
                    row.col(|ui| {
                        ui.label(
                            f.fid
                                .map(|id| format!("fid {id}"))
                                .unwrap_or_else(|| "Unknown".into()),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            f.lid
                                .map(|id| format!("lid {id}"))
                                .unwrap_or_else(|| "—".into()),
                        );
                    });
                    row.col(|ui| {
                        ui.label(if f.crc_mismatch() {
                            "Mismatch"
                        } else {
                            f.crc32.as_deref().unwrap_or("—")
                        });
                    });
                    row.col(|ui| {
                        if ui
                            .add(egui::Label::new(&f.path).sense(Sense::click()))
                            .double_clicked()
                        {
                            let _ = open::that(&f.path);
                        }
                    });
                });
            });
        }
    }

    fn groups_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "Groups", true);
        let key = (self.loader.version, self.query.to_ascii_lowercase());
        let Self {
            projection, views, ..
        } = self;
        views
            .groups
            .refresh(&key, || filter_groups(projection, &key.1));
        let rows = &views.groups.rows;
        let row_height = row_height(ui);
        data_table(
            ui,
            "groups",
            &[
                ("Name", 300.0),
                ("Short name", 140.0),
                ("Rating", 80.0),
                ("Votes", 80.0),
                ("Website", 100.0),
            ],
        )
        .body(|body| {
            body.rows(row_height, rows.len(), |mut row| {
                let g = &projection.groups[rows[row.index()]];
                row.col(|ui| {
                    ui.label(&g.name);
                });
                row.col(|ui| {
                    ui.label(&g.short);
                });
                row.col(|ui| {
                    ui.label(format::rating(g.rating));
                });
                row.col(|ui| {
                    ui.label(g.votes.to_string());
                });
                row.col(|ui| {
                    if let Some(url) = &g.url {
                        if ui.link("Open").clicked() {
                            let _ = open::that(url);
                        }
                    } else {
                        ui.label("—");
                    }
                });
            });
        });
    }

    fn genres_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "Genres", true);
        let key = (self.loader.version, self.query.to_ascii_lowercase());
        let Self {
            projection, views, ..
        } = self;
        views
            .genres
            .refresh(&key, || filter_genres(projection, &key.1));
        let rows = &views.genres.rows;
        let row_height = row_height(ui);
        data_table(ui, "genres", &[("Genre", 300.0), ("Anime", 100.0)]).body(|body| {
            body.rows(row_height, rows.len(), |mut row| {
                let (_, name, count) = &projection.genres[rows[row.index()]];
                row.col(|ui| {
                    ui.label(name);
                });
                row.col(|ui| {
                    ui.label(count.to_string());
                });
            });
        });
    }

    fn queue_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "AniDB command queue", false);
        let total = self.projection.queue.len();
        let blocked = self.projection.summary.queue_blocked;
        let blocked_pending = self.blocked_unconfirmed();
        let cooling = self
            .active_cooldown()
            .map(|(until, reason)| Self::cooldown_text(until, reason));
        let mut clear = false;
        let mut retry = false;
        let mut sync = false;
        ui.horizontal(|ui| {
            ui.label(format!("{total} pending commands"));
            if blocked_pending {
                ui.colored_label(Color32::LIGHT_RED, "blocked commands (refreshing)");
            } else if blocked > 0 {
                ui.colored_label(Color32::LIGHT_RED, format!("{blocked} blocked"));
            }
            if ui.button("Clear queue").clicked() {
                clear = true;
            }
            if ui
                .add_enabled(
                    blocked > 0 || blocked_pending,
                    egui::Button::new("Retry failed"),
                )
                .on_hover_text("Unblock failed commands and try them again")
                .clicked()
            {
                retry = true;
            }
            let run = ui
                .add_enabled(
                    !self.import_pending && cooling.is_none(),
                    egui::Button::new("Synchronize now"),
                )
                .on_disabled_hover_text(cooling.as_deref().unwrap_or_default());
            if run.clicked() {
                sync = true;
            }
        });
        if let Some(text) = &cooling {
            ui.colored_label(Color32::YELLOW, text);
        }
        if clear {
            match self.db.queue_clear() {
                Ok(()) => self.request_library_refresh(),
                Err(e) => self.db_write_error("Could not clear the queue", &e),
            }
        }
        if retry {
            match self.db.queue_retry_failed() {
                Ok(n) => {
                    self.log(format!("Retrying {n} failed commands"));
                    self.request_library_refresh();
                }
                Err(e) => self.db_write_error("Could not retry failed commands", &e),
            }
        }
        if sync {
            self.worker
                .submit(Job::ProcessQueue(self.settings.anidb.clone()));
        }
        let queue = &self.projection.queue;
        let row_height = row_height(ui);
        data_table(
            ui,
            "queue_table",
            &[
                ("ID", 70.0),
                ("Command", 420.0),
                ("Status", 110.0),
                ("Last error", 300.0),
            ],
        )
        .body(|body| {
            body.rows(row_height, queue.len(), |mut row| {
                let entry = &queue[row.index()];
                row.col(|ui| {
                    ui.monospace(format!("#{}", entry.id));
                });
                row.col(|ui| {
                    ui.label(&entry.description);
                });
                row.col(|ui| {
                    if entry.blocked {
                        ui.colored_label(Color32::LIGHT_RED, "Blocked");
                    } else if entry.tries > 0 {
                        ui.colored_label(Color32::YELLOW, format!("{} retries", entry.tries));
                    } else {
                        ui.weak("Pending");
                    }
                });
                row.col(|ui| {
                    if let Some(err) = &entry.last_error {
                        ui.label(RichText::new(err).color(Color32::LIGHT_RED))
                            .on_hover_text(err);
                    }
                });
            });
        });
    }

    fn log_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "Activity log", false);
        ui.horizontal(|ui| {
            if ui.button("Clear").clicked() {
                self.logs.clear();
            }
        });
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show_rows(ui, row_height, self.logs.len(), |ui, range| {
                for line in &self.logs[range] {
                    ui.monospace(line);
                }
            });
    }

    fn anime_detail_window(&mut self, ctx: &egui::Context) {
        let Some(aid) = self.selected_aid else {
            self.detail = None;
            return;
        };
        // One detail read per (anime, projection version), not one per frame.
        let version = self.loader.version;
        let detail = match self.detail.take() {
            Some(cached) if cached.aid == aid && cached.version == version => cached.detail,
            _ => self.db.anime_detail(aid).ok().flatten(),
        };
        let mut refresh_requested = false;
        let mut open = true;
        egui::Window::new(format!("Anime #{aid}"))
            .open(&mut open)
            .default_width(520.0)
            .show(ctx, |ui| {
                if let Some(d) = &detail {
                    ui.heading(if d.anime.name.is_empty() {
                        format!("Anime {aid}")
                    } else {
                        d.anime.name.clone()
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(d.anime.type_name());
                        ui.separator();
                        ui.label(&d.anime.year);
                        ui.separator();
                        ui.label(format!("{} episodes", d.anime.episodes));
                    });
                    if let Some(text) = &d.anime.description {
                        ui.add_space(8.0);
                        ui.label(text);
                    }
                    ui.add_space(8.0);
                    ui.collapsing(format!("Titles ({})", d.titles.len()), |ui| {
                        for t in &d.titles {
                            ui.horizontal(|ui| {
                                ui.monospace(&t.lang);
                                ui.label(&t.name);
                            });
                        }
                    });
                    ui.collapsing(format!("Episodes ({})", d.episodes.len()), |ui| {
                        for e in &d.episodes {
                            ui.horizontal(|ui| {
                                ui.monospace(e.epno_str());
                                ui.label(&e.name);
                            });
                        }
                    });
                    ui.collapsing(format!("Genres ({})", d.genres.len()), |ui| {
                        ui.label(
                            d.genres
                                .iter()
                                .map(|g| g.1.as_str())
                                .collect::<Vec<_>>()
                                .join(", "),
                        );
                    });
                    ui.collapsing(format!("Relations ({})", d.relations.len()), |ui| {
                        for (related_aid, relation, name) in &d.relations {
                            if ui
                                .link(format!(
                                    "{} — {}",
                                    animatic_core::library::relation_name(*relation),
                                    name
                                ))
                                .clicked()
                            {
                                self.selected_aid = Some(*related_aid);
                            }
                        }
                    });
                    ui.collapsing(format!("Reviews ({})", d.reviews.len()), |ui| {
                        for review in &d.reviews {
                            ui.horizontal(|ui| {
                                ui.strong(if review.uname.is_empty() {
                                    "Anonymous"
                                } else {
                                    &review.uname
                                });
                                ui.label(format::rating(review.rating));
                            });
                            ui.label(&review.body);
                            ui.separator();
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Open on AniDB").clicked() {
                            let _ = open::that(format!("https://anidb.net/anime/{aid}"));
                        }
                        if ui.button("Refresh from AniDB").clicked() {
                            refresh_requested = true;
                        }
                    });
                } else {
                    ui.label("No detail record is available.");
                }
            });
        if open {
            self.detail = Some(DetailCache {
                aid,
                version,
                detail,
            });
        } else {
            self.selected_aid = None;
        }
        if refresh_requested {
            self.queue_command(animatic_core::sync::Command::FetchAnime { aid });
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let mut open = true;
        let mut save = false;
        egui::Window::new("Options")
            .open(&mut open)
            .resizable(true)
            .default_size([620.0, 600.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("AniDB account");
                    field(ui, "Username", &mut self.settings.anidb.username);
                    ui.horizontal(|ui| {
                        ui.label("Password");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.settings.anidb.password)
                                .password(true),
                        );
                    });
                    field(ui, "API key", &mut self.settings.anidb.api_key);
                    field(ui, "Client name", &mut self.settings.anidb.client_name);
                    ui.checkbox(
                        &mut self.settings.anidb.offline,
                        "Work offline (keep commands queued)",
                    );
                    ui.checkbox(
                        &mut self.settings.anidb.update_file_information,
                        "Update known file information",
                    );
                    ui.separator();
                    ui.heading("Appearance");
                    egui::ComboBox::from_label("Theme")
                        .selected_text(format!("{:?}", self.settings.ui.theme))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.settings.ui.theme, Theme::Dark, "Dark");
                            ui.selectable_value(&mut self.settings.ui.theme, Theme::Light, "Light");
                            ui.selectable_value(
                                &mut self.settings.ui.theme,
                                Theme::System,
                                "System",
                            );
                        });
                    ui.add(
                        egui::Slider::new(&mut self.settings.ui.zoom, 0.75..=2.0).text("UI scale"),
                    );
                    ui.checkbox(&mut self.settings.ui.colored_rows, "Use status colors");
                    ui.separator();
                    ui.heading("Hashing");
                    ui.checkbox(
                        &mut self.settings.hashing.autostart,
                        "Start hashing automatically",
                    );
                    ui.checkbox(
                        &mut self.settings.hashing.identify_after_hashing,
                        "Identify files with AniDB after hashing",
                    );
                    ui.checkbox(
                        &mut self.settings.hashing.use_extensions,
                        "Restrict file extensions",
                    );
                    let mut ext = self.settings.hashing.extensions.join(",");
                    if ui
                        .add(egui::TextEdit::multiline(&mut ext).desired_rows(3))
                        .changed()
                    {
                        self.settings.hashing.extensions = ext
                            .split(',')
                            .map(|x| x.trim().to_string())
                            .filter(|x| !x.is_empty())
                            .collect();
                    }
                    ui.label("Hash export template");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.settings.hashing.export_template)
                            .desired_rows(2)
                            .hint_text("%filename% %size% %fulled2k%"),
                    );
                    ui.checkbox(
                        &mut self.settings.ui.copy_length_as_minutes,
                        "Export %length% in minutes",
                    );
                    ui.separator();
                    ui.heading("Managed folders");
                    let mut remove_manager = None;
                    for (index, manager) in
                        self.settings.file_managers.managers.iter_mut().enumerate()
                    {
                        let label = if manager.name.is_empty() {
                            "Unnamed folder".to_string()
                        } else {
                            manager.name.clone()
                        };
                        ui.collapsing(label, |ui| {
                            field(ui, "Name", &mut manager.name);
                            field(ui, "Path", &mut manager.path);
                            field(ui, "Rename destination", &mut manager.base_path);
                            ui.horizontal_wrapped(|ui| {
                                ui.checkbox(&mut manager.include_subdirs, "Subfolders");
                                ui.checkbox(&mut manager.auto_hash, "Auto hash");
                                ui.checkbox(&mut manager.auto_rehash, "Rehash changed");
                                ui.checkbox(&mut manager.auto_remove, "Remove missing");
                                ui.checkbox(&mut manager.auto_rename, "Auto rename");
                                ui.checkbox(&mut manager.mylist_auto_add, "Add to mylist");
                            });
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut manager.mylist_watched_check, "Set watched");
                                if manager.mylist_watched_check {
                                    ui.checkbox(&mut manager.mylist_watched, "Watched");
                                }
                                ui.checkbox(&mut manager.mylist_state_check, "Set state");
                                if manager.mylist_state_check {
                                    egui::ComboBox::from_id_salt(("manager-state", index))
                                        .selected_text(mylist_state_name(manager.mylist_state))
                                        .show_ui(ui, |ui| {
                                            for (id, name) in MYLIST_STATES.iter().enumerate() {
                                                ui.selectable_value(
                                                    &mut manager.mylist_state,
                                                    id as i64,
                                                    *name,
                                                );
                                            }
                                        });
                                }
                            });
                            ui.checkbox(&mut manager.mylist_storage_check, "Set storage");
                            ui.add_enabled_ui(manager.mylist_storage_check, |ui| {
                                field(ui, "Storage", &mut manager.mylist_storage);
                            });
                            ui.checkbox(&mut manager.mylist_source_check, "Set source");
                            ui.add_enabled_ui(manager.mylist_source_check, |ui| {
                                field(ui, "Source", &mut manager.mylist_source);
                            });
                            ui.checkbox(&mut manager.mylist_other_check, "Set other");
                            ui.add_enabled_ui(manager.mylist_other_check, |ui| {
                                field(ui, "Other", &mut manager.mylist_other);
                            });
                            if manager.can_edit_path && ui.button("Remove folder rule").clicked() {
                                remove_manager = Some(index);
                            }
                        });
                    }
                    if let Some(index) = remove_manager {
                        self.settings.file_managers.managers.remove(index);
                    }
                    if ui.button("Add folder rule").clicked() {
                        self.settings.file_managers.managers.push(
                            animatic_core::settings::FileManager {
                                name: "New folder".into(),
                                ..Default::default()
                            },
                        );
                    }
                    ui.separator();
                    ui.heading("Statistics export");
                    ui.checkbox(
                        &mut self.settings.stat_whore.enabled,
                        "Write statistics after library updates",
                    );
                    egui::ComboBox::from_label("Format")
                        .selected_text(format!("{:?}", self.settings.stat_whore.format))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.settings.stat_whore.format,
                                animatic_core::settings::StatFormat::Xml,
                                "XML",
                            );
                            ui.selectable_value(
                                &mut self.settings.stat_whore.format,
                                animatic_core::settings::StatFormat::Ini,
                                "INI",
                            );
                        });
                    field(ui, "Output file", &mut self.settings.stat_whore.file);
                    ui.separator();
                    ui.heading("External player");
                    if self.settings.players.is_empty() && ui.button("Add player").clicked() {
                        self.settings
                            .players
                            .push(animatic_core::settings::Player::default());
                    }
                    for player in &mut self.settings.players {
                        field(ui, "Name", &mut player.name);
                        field(ui, "Executable", &mut player.path);
                        field(ui, "Play arguments", &mut player.play);
                        ui.checkbox(&mut player.default, "Default player");
                    }
                    ui.separator();
                    ui.heading("AVDump");
                    ui.checkbox(
                        &mut self.settings.avdump.enabled,
                        "Enable AVDump integration",
                    );
                    field(ui, "Executable", &mut self.settings.avdump.executable);
                    field(ui, "Arguments", &mut self.settings.avdump.arguments);
                    field(ui, "AVDump API key", &mut self.settings.avdump.key);
                    ui.separator();
                    ui.heading("Renaming pattern");
                    self.pattern_editor_ui(ui);
                    ui.separator();
                    ui.heading("Mylist states");
                    ui.label(MYLIST_STATES.join(" · "));
                    ui.add_space(12.0);
                    if ui.button("Save options").clicked() {
                        save = true;
                    }
                });
            });
        if save {
            self.save_settings();
            self.apply_visuals(ctx);
            self.settings_open = false;
        }
        if !open {
            self.settings_open = false;
        }
    }

    fn apply_visuals(&self, ctx: &egui::Context) {
        match self.settings.ui.theme {
            Theme::Dark => ctx.set_visuals(egui::Visuals::dark()),
            Theme::Light => ctx.set_visuals(egui::Visuals::light()),
            Theme::System => {}
        }
        ctx.set_zoom_factor(self.settings.ui.zoom.clamp(0.75, 2.0));
    }

    fn bottom_bar(&mut self, root: &mut egui::Ui) {
        let cooling = self
            .active_cooldown()
            .map(|(until, reason)| Self::cooldown_text(until, reason));
        egui::Panel::bottom("status").show(root, |ui| {
            ui.horizontal(|ui| {
                if self.activity.busy {
                    ui.add(
                        egui::ProgressBar::new(self.activity.fraction)
                            .desired_width(180.0)
                            .show_percentage(),
                    );
                    ui.label(format!(
                        "{} ({}/{})",
                        self.activity.label, self.activity.current, self.activity.total
                    ));
                    if ui
                        .button(if self.worker.pause.load(Ordering::Relaxed) {
                            "Resume"
                        } else {
                            "Pause"
                        })
                        .clicked()
                    {
                        let old = self.worker.pause.load(Ordering::Relaxed);
                        self.worker.pause.store(!old, Ordering::Relaxed);
                    }
                    if ui.button("Cancel").clicked() {
                        self.worker.cancel.store(true, Ordering::Relaxed);
                    }
                } else {
                    ui.label(&self.status);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(if self.settings.anidb.offline {
                        "Offline"
                    } else {
                        "Ready for AniDB"
                    });
                    if let Some(text) = &cooling {
                        ui.colored_label(Color32::YELLOW, text);
                    }
                    if self.projection.summary.queue_blocked > 0 || self.blocked_unconfirmed() {
                        ui.colored_label(
                            Color32::LIGHT_RED,
                            if self.blocked_unconfirmed() {
                                "blocked commands (refreshing)".to_string()
                            } else {
                                format!("{} blocked", self.projection.summary.queue_blocked)
                            },
                        );
                    }
                    if !self.deferred.is_empty() {
                        ui.colored_label(
                            Color32::YELLOW,
                            format!("{} commands NOT saved yet", self.deferred.len()),
                        );
                    }
                });
            });
        });
    }
}

impl eframe::App for AnimaticApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.process_events(&ctx);
        self.retry_deferred();
        self.pump_library(&ctx);
        self.top_bar(root);
        self.navigation(root);
        self.bottom_bar(root);
        egui::CentralPanel::default().show(root, |ui| match self.page {
            Page::Dashboard => self.dashboard(ui),
            Page::Mylist => self.mylist_page(ui),
            Page::Anime => self.anime_page(ui),
            Page::Search => self.search_page(ui),
            Page::Hashed => self.hashed_page(ui),
            Page::Groups => self.groups_page(ui),
            Page::Genres => self.genres_page(ui),
            Page::Queue => self.queue_page(ui),
            Page::Log => self.log_page(ui),
        });
        self.legacy_settings_review_window(&ctx);
        self.settings_window(&ctx);
        self.settings_notice_window(&ctx);
        self.rename_window(&ctx);
        if self.about_open {
            let mut open = true;
            egui::Window::new("About Animatic")
                .open(&mut open)
                .show(&ctx, |ui| {
                    ui.heading("Animatic");
                    ui.label("Modern cross-platform AniDB library manager");
                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                    ui.hyperlink_to("AniDB", "https://anidb.net/");
                });
            if !open {
                self.about_open = false;
            }
        }
        // Worker events are only seen while frames run, so poll while anything is in flight.
        if self.activity.busy
            || self.loader.outstanding.is_some()
            || self.loader.dirty
            || !self.deferred.is_empty()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else if self.active_cooldown().is_some() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    fn on_exit(&mut self) {
        if self.settings_guard.allows_exit_save() {
            let _ = self.settings.save(&self.settings_path);
        } else {
            log::warn!(
                "Settings were recovered from a backup and not saved by the user; the original settings file was left unchanged on exit"
            );
        }
        if !self.deferred.is_empty() {
            let _ = self.db.conn().busy_timeout(EXIT_FLUSH_TIMEOUT);
            self.flush_deferred();
            if !self.deferred.is_empty() {
                // Nothing can show this after exit, so make the loss explicit in the log.
                log::error!(
                    "{} queued commands could not be saved because the database stayed busy: {}",
                    self.deferred.len(),
                    self.deferred
                        .iter()
                        .map(|c| c.describe())
                        .collect::<Vec<_>>()
                        .join("; ")
                );
            }
        }
    }
}

fn metric(ui: &mut egui::Ui, label: &str, value: String) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new(value).size(24.0).strong());
        ui.label(RichText::new(label).weak());
    });
}

/// Height of every table row: tall enough for the combo boxes and checkboxes in the My list.
fn row_height(ui: &egui::Ui) -> f32 {
    ui.spacing().interact_size.y
}

/// Virtualized table: a fixed header and a body that only lays out the visible rows. Cells
/// truncate instead of wrapping so every row keeps the height the body was told. The columns
/// have a combined minimum width; when the page is narrower the whole table (header and rows
/// together) scrolls horizontally so every column stays reachable.
struct DataTable<'a, 'c> {
    ui: &'a mut egui::Ui,
    id: &'c str,
    columns: &'c [(&'c str, f32)],
}

fn data_table<'a, 'c>(
    ui: &'a mut egui::Ui,
    id: &'c str,
    columns: &'c [(&'c str, f32)],
) -> DataTable<'a, 'c> {
    DataTable { ui, id, columns }
}

impl DataTable<'_, '_> {
    fn body(self, add_body: impl FnOnce(egui_extras::TableBody<'_>)) {
        let Self { ui, id, columns } = self;
        let header_height = row_height(ui);
        let spacing = ui.spacing().item_spacing.x;
        let minimum: f32 = columns.iter().map(|(_, width)| width + spacing).sum();
        let width = ui.available_width().max(minimum);
        let last = columns.len().saturating_sub(1);
        egui::ScrollArea::horizontal()
            .id_salt((id, "horizontal"))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(width);
                let mut table = TableBuilder::new(ui)
                    .id_salt(id)
                    .striped(true)
                    .resizable(true)
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .auto_shrink([false, false])
                    .min_scrolled_height(0.0);
                for (i, (_, column_width)) in columns.iter().enumerate() {
                    let column = if i == last {
                        Column::remainder().at_least(*column_width)
                    } else {
                        Column::initial(*column_width).at_least(40.0)
                    };
                    table = table.column(column.clip(true));
                }
                table
                    .header(header_height, |mut header| {
                        for (name, _) in columns {
                            header.col(|ui| {
                                ui.strong(*name);
                            });
                        }
                    })
                    .body(add_body);
            });
    }
}

fn is_busy(e: &animatic_core::Error) -> bool {
    let text = e.to_string().to_ascii_lowercase();
    text.contains("locked") || text.contains("busy")
}

fn anime_title(a: &Anime) -> std::borrow::Cow<'_, str> {
    if a.name.is_empty() {
        std::borrow::Cow::Owned(format!("aid {}", a.aid))
    } else {
        std::borrow::Cow::Borrowed(&a.name)
    }
}

/// `Some(aid)` when the query is exactly a canonical AniDB id.
fn aid_query(q: &str) -> Option<i64> {
    q.parse::<i64>().ok().filter(|n| n.to_string() == q)
}

fn matches_anime(key: &str, aid: i64, q: &str, q_aid: Option<i64>) -> bool {
    q.is_empty() || key.contains(q) || q_aid == Some(aid)
}

/// Row indices of anime whose name contains the lower-cased query or whose id equals it.
fn filter_anime(p: &LibraryProjection, q: &str) -> Vec<usize> {
    let q_aid = aid_query(q);
    (0..p.anime.len())
        .filter(|&i| matches_anime(&p.keys.anime[i], p.anime[i].aid, q, q_aid))
        .collect()
}

struct SearchFilter<'a> {
    query: &'a str,
    year: &'a str,
    type_id: i64,
    mylist_only: bool,
    watched_only: bool,
}

fn filter_search(p: &LibraryProjection, f: &SearchFilter) -> Vec<usize> {
    let q_aid = aid_query(f.query);
    (0..p.anime.len())
        .filter(|&i| {
            let a = &p.anime[i];
            let state = p.states.get(&a.aid);
            matches_anime(&p.keys.anime[i], a.aid, f.query, q_aid)
                && (f.year.is_empty() || a.year.contains(f.year))
                && (f.type_id == 0 || a.type_id == f.type_id)
                && (!f.mylist_only || state.is_some())
                && (!f.watched_only || state.is_some_and(|s| s.all_watched(a)))
        })
        .collect()
}

fn filter_mylist(p: &LibraryProjection, q: &str) -> Vec<usize> {
    (0..p.mylist.len())
        .filter(|&i| q.is_empty() || p.keys.mylist[i].contains(q))
        .collect()
}

fn filter_local(p: &LibraryProjection, q: &str) -> Vec<usize> {
    (0..p.local.len())
        .filter(|&i| q.is_empty() || p.keys.local[i].contains(q))
        .collect()
}

fn filter_groups(p: &LibraryProjection, q: &str) -> Vec<usize> {
    (0..p.groups.len())
        .filter(|&i| {
            let (name, short) = &p.keys.groups[i];
            q.is_empty() || name.contains(q) || short.contains(q)
        })
        .collect()
}

fn filter_genres(p: &LibraryProjection, q: &str) -> Vec<usize> {
    (0..p.genres.len())
        .filter(|&i| q.is_empty() || p.keys.genres[i].contains(q))
        .collect()
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.text_edit_singleline(value);
    });
}

/// Warnings and errors of a rename plan; warnings block applying it just like errors do.
fn show_plan(ui: &mut egui::Ui, plan: &Plan) {
    for error in &plan.errors {
        ui.colored_label(Color32::LIGHT_RED, error);
    }
    if !plan.warnings.is_empty() {
        ui.colored_label(
            Color32::YELLOW,
            "The pattern reported warnings, so its result cannot be trusted. Renaming stays \
             disabled until they are fixed:",
        );
        for warning in &plan.warnings {
            ui.colored_label(Color32::YELLOW, format!("• {warning}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> LibraryProjection {
        let db = Db::memory().unwrap();
        db.conn()
            .execute_batch(
                "INSERT INTO category VALUES ('qual', 2, 'very high', 0, 2, NULL), ('type', 16, 'Blu-ray', 0, 13, NULL);
                 INSERT INTO anime(aid, type_id, year, episodes) VALUES (1, 2, '1999', 1), (2, 4, '2012', 1), (3, 2, '2012', 12);
                 INSERT INTO anime_title(aid, name, type, lang) VALUES (1, 'Fixture Alpha', 1, 'x-jat'),
                    (2, 'Fixture Movie', 1, 'x-jat'), (3, 'Third Series', 1, 'x-jat');
                 INSERT INTO episode(eid, aid, epno, type, length) VALUES (10, 1, 1, 1, 25);
                 INSERT INTO grp(gid, name, short) VALUES (7, 'Fixture Group', 'FG');
                 INSERT INTO file(fid, aid, eid, gid, size, ed2k, crc32, quality_id, source_id, state, user_count)
                    VALUES (100, 1, 10, 7, 1000, 'aa', 'deadbeef', 2, 16, 0, 5);
                 INSERT INTO mylist(lid, fid, eid, aid, gid, state, viewed, viewdate) VALUES (1000, 100, 10, 1, 7, 1, 1, 5);
                 INSERT INTO local_hash(id, ed2k, size, crc32) VALUES (1, 'aa', 1000, 'DEADBEEF');
                 INSERT INTO local_file(id, hash_id, path) VALUES (1, 1, '/Fixture/Root/One.mkv');",
            )
            .unwrap();
        LibraryProjection::load(&db).unwrap()
    }

    #[test]
    fn anime_filter_matches_name_or_exact_id() {
        let p = fixture();
        assert_eq!(filter_anime(&p, "").len(), 3);
        assert_eq!(filter_anime(&p, "fixture").len(), 2);
        let by_id = filter_anime(&p, "3");
        assert_eq!(by_id.len(), 1);
        assert_eq!(p.anime[by_id[0]].aid, 3);
        assert!(
            filter_anime(&p, "03").is_empty(),
            "ids match exactly, not by padding"
        );
    }

    #[test]
    fn search_filter_combines_year_type_and_mylist() {
        let p = fixture();
        let all_2012 = SearchFilter {
            query: "",
            year: "2012",
            type_id: 0,
            mylist_only: false,
            watched_only: false,
        };
        assert_eq!(filter_search(&p, &all_2012).len(), 2);
        let series_2012 = SearchFilter {
            type_id: 2,
            ..all_2012
        };
        let rows = filter_search(&p, &series_2012);
        assert_eq!(rows.len(), 1);
        assert_eq!(p.anime[rows[0]].aid, 3);
        let mylist = SearchFilter {
            query: "",
            year: "",
            type_id: 0,
            mylist_only: true,
            watched_only: false,
        };
        let rows = filter_search(&p, &mylist);
        assert_eq!(rows.len(), 1);
        assert_eq!(p.anime[rows[0]].aid, 1);
    }

    #[test]
    fn table_filters_use_projection_keys() {
        let p = fixture();
        assert_eq!(filter_mylist(&p, "alpha").len(), 1);
        assert!(filter_mylist(&p, "third").is_empty());
        assert_eq!(filter_local(&p, "/fixture/root").len(), 1);
        assert_eq!(filter_groups(&p, "fg").len(), 1);
        assert_eq!(filter_groups(&p, "fixture g").len(), 1);
        assert!(filter_groups(&p, "zzz").is_empty());
    }

    #[test]
    fn index_cache_rebuilds_only_when_key_changes() {
        let mut cache: IndexCache<(u64, String)> = IndexCache::default();
        let mut builds = 0;
        let key = (1, "a".to_string());
        assert!(cache.refresh(&key, || {
            builds += 1;
            vec![0, 2]
        }));
        assert!(!cache.refresh(&key, || {
            builds += 1;
            vec![]
        }));
        assert_eq!(cache.rows, vec![0, 2]);
        let next = (2, "a".to_string());
        assert!(cache.refresh(&next, || {
            builds += 1;
            vec![1]
        }));
        assert_eq!((builds, cache.rows), (2, vec![1]));
    }

    #[test]
    fn recovered_settings_are_only_rewritten_after_an_explicit_save() {
        let guard = |recovered, user_saved| SettingsGuard {
            recovered,
            user_saved,
        };
        assert!(guard(false, false).allows_exit_save());
        assert!(!guard(true, false).allows_exit_save());
        assert!(guard(true, true).allows_exit_save());
    }
}
