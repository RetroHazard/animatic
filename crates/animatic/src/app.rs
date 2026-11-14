use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::style;
use crate::table::{
    SortKey, data_table, row_height, sort_anime, sort_genres, sort_groups, sort_key, sort_local,
    sort_mylist,
};
use crate::worker::{
    Event, Job, RenameItem, RestoreResult, UpdateEvent, UpdateJob, Updater, Worker, spawn_restore,
};
use animatic_anidb::{HTTP_CLIENT, HTTP_CLIENT_VERSION, UDP_CLIENT, UDP_CLIENT_VERSION};
use animatic_core::db::Db;
use animatic_core::format;
use animatic_core::hash_queue::HashSource;
use animatic_core::library::{Anime, AnimeDetail, LibraryProjection, mylist_state_name};
use animatic_core::maintenance::{BackupInfo, IntegrityReport};
use animatic_core::rename::plan::{self, BatchRow, Candidate, PatternKind, Plan};
use animatic_core::rename::{presets, reference};
use animatic_core::secrets::{CredentialStorage, Credentials};
use animatic_core::settings::{MYLIST_STATES, RowStyle, Settings, StatFormat, Theme};
use animatic_core::ui_state::{
    self, anime_slot, local_file_slot, mylist_slot, row_colors, window_to_save,
};
use animatic_core::update::{InstallationTarget, Platform, ReleaseInfo, VerifiedUpdate};
use crossbeam_channel::{Receiver, TryRecvError};
use eframe::egui::{self, Align, Color32, Layout, RichText, Sense};

mod anidb_markup;
mod auto_rename;
mod covers;
mod dialog;
mod hash_queue_panel;
mod media_options;
mod mylist_editor;
mod mylist_export;
mod player_actions;
mod refresh;
mod shell_menu;
mod tray_ui;

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
    /// Sidebar groups in order: an unnamed group for the overview, then the library pages, then
    /// the activity pages. The order is the documented one; only the grouping is visual.
    const GROUPS: [(Option<&'static str>, &'static [Page]); 3] = [
        (None, &[Page::Dashboard]),
        (
            Some("Library"),
            &[
                Page::Mylist,
                Page::Anime,
                Page::Search,
                Page::Hashed,
                Page::Groups,
                Page::Genres,
            ],
        ),
        (Some("Activity"), &[Page::Queue, Page::Log]),
    ];

    /// The sidebar label, which is also the page title.
    fn label(self) -> &'static str {
        match self {
            Page::Dashboard => "Overview",
            Page::Mylist => "MyList",
            Page::Anime => "Anime",
            Page::Search => "Advanced search",
            Page::Hashed => "Hashed files",
            Page::Groups => "Groups",
            Page::Genres => "Genres",
            Page::Queue => "Queue",
            Page::Log => "Log",
        }
    }

    /// Pages in sidebar order (`Ctrl+1` … `Ctrl+9`).
    fn all() -> impl Iterator<Item = Page> {
        Page::GROUPS
            .into_iter()
            .flat_map(|(_, pages)| pages.iter().copied())
    }
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
    anime: IndexCache<(u64, String, Option<i64>, SortKey)>,
    search: IndexCache<(u64, String, String, i64, bool, bool, SortKey)>,
    mylist: IndexCache<(u64, String, SortKey)>,
    local: IndexCache<(u64, String, SortKey)>,
    groups: IndexCache<(u64, String, SortKey)>,
    genres: IndexCache<(u64, String, SortKey)>,
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

/// Release archives carry no signature; this is shown wherever an install can be started.
const UNSIGNED_WARNING: &str = "Release archives are not code-signed. Animatic compares the \
     downloaded archive with the SHA-256 checksum published in the same GitHub release. That \
     detects a damaged or mismatched download, but it does not prove who published the release. \
     Install only if you trust the release on GitHub.";
const UPDATE_PRIVACY: &str = "Checking contacts github.com. GitHub sees your IP address and \
     the Animatic version; no AniDB credentials and no library data are sent.";

/// Where the update flow stands. Only the check can start by itself; downloading and \
/// installing each need an explicit action, and installing a confirmation as well.
#[derive(Debug, Clone, PartialEq)]
enum UpdateStage {
    Idle,
    /// `manual`: the user asked (or joined an automatic check), so a failure is shown.
    Checking {
        manual: bool,
        cancelling: bool,
    },
    UpToDate,
    /// A newer release is known; nothing has been downloaded.
    Available,
    Downloading {
        done: u64,
        total: u64,
        cancelling: bool,
    },
    /// Downloaded, verified against its checksum and staged.
    Ready {
        confirming: bool,
    },
    Installing,
    /// The executable on disk is the new version; this process is not. Final.
    Installed,
    /// An installation was attempted and ended in an error. The executable on disk may be in an
    /// unknown state, so this is final too: no further check, download, install or restart.
    InstallFailed,
}

impl UpdateStage {
    /// True when a check job must be submitted. A running automatic check is adopted by a
    /// manual request instead of being repeated.
    fn begin_check(&mut self, manual: bool) -> bool {
        match self {
            UpdateStage::Idle | UpdateStage::UpToDate => {
                *self = UpdateStage::Checking {
                    manual,
                    cancelling: false,
                };
                true
            }
            UpdateStage::Checking { manual: known, .. } => {
                *known |= manual;
                false
            }
            _ => false,
        }
    }

    fn check_finished(&mut self, found: bool) -> bool {
        if matches!(self, UpdateStage::Checking { .. }) {
            *self = if found {
                UpdateStage::Available
            } else {
                UpdateStage::UpToDate
            };
            return true;
        }
        false
    }

    /// `Some(true)` when the user is waiting for this check and must see the failure.
    fn check_failed(&mut self) -> Option<bool> {
        if let UpdateStage::Checking { manual, .. } = *self {
            *self = UpdateStage::Idle;
            return Some(manual);
        }
        None
    }

    /// True when the cancel flag of the updater must be raised.
    fn cancel(&mut self) -> bool {
        match self {
            UpdateStage::Checking { cancelling, .. }
            | UpdateStage::Downloading { cancelling, .. } => {
                *cancelling = true;
                true
            }
            _ => false,
        }
    }

    fn cancelled(&mut self) {
        match self {
            UpdateStage::Checking { .. } => *self = UpdateStage::Idle,
            UpdateStage::Downloading { .. } => *self = UpdateStage::Available,
            _ => {}
        }
    }

    fn begin_download(&mut self) -> bool {
        if matches!(self, UpdateStage::Available) {
            *self = UpdateStage::Downloading {
                done: 0,
                total: 0,
                cancelling: false,
            };
            return true;
        }
        false
    }

    fn progress(&mut self, done_now: u64, total_now: u64) {
        if let UpdateStage::Downloading { done, total, .. } = self {
            (*done, *total) = (done_now, total_now);
        }
    }

    fn download_ready(&mut self) -> bool {
        if matches!(self, UpdateStage::Downloading { .. }) {
            *self = UpdateStage::Ready { confirming: false };
            return true;
        }
        false
    }

    /// A failed download keeps the release, so it can be tried again.
    fn download_failed(&mut self) -> bool {
        if matches!(self, UpdateStage::Downloading { .. }) {
            *self = UpdateStage::Available;
            return true;
        }
        false
    }

    fn set_confirming(&mut self, value: bool) {
        if let UpdateStage::Ready { confirming } = self {
            *confirming = value;
        }
    }

    /// Dropping the staged update means downloading it again.
    fn discard_ready(&mut self) -> bool {
        if matches!(self, UpdateStage::Ready { .. }) {
            *self = UpdateStage::Available;
            return true;
        }
        false
    }

    /// Only a confirmed, staged update may be installed.
    fn begin_install(&mut self) -> bool {
        if matches!(self, UpdateStage::Ready { confirming: true }) {
            *self = UpdateStage::Installing;
            return true;
        }
        false
    }

    fn installed(&mut self) -> bool {
        if matches!(self, UpdateStage::Installing) {
            *self = UpdateStage::Installed;
            return true;
        }
        false
    }

    /// `attempted`: the executable may have been changed, which ends the update flow for this
    /// session (see `InstallFailed`). Otherwise nothing was replaced; the staged files are gone,
    /// so the update has to be downloaded again.
    fn install_failed(&mut self, attempted: bool) -> bool {
        if matches!(self, UpdateStage::Installing) {
            *self = if attempted {
                UpdateStage::InstallFailed
            } else {
                UpdateStage::Available
            };
            return true;
        }
        false
    }

    /// An operation is in flight and its events must be polled for.
    fn is_active(&self) -> bool {
        matches!(
            self,
            UpdateStage::Checking { .. }
                | UpdateStage::Downloading { .. }
                | UpdateStage::Installing
        )
    }
}

struct UpdateState {
    stage: UpdateStage,
    release: Option<ReleaseInfo>,
    ready: Option<Box<VerifiedUpdate>>,
    /// Failure of the last action the user took, shown in the Updates window.
    error: Option<String>,
    installed_version: Option<String>,
    open: bool,
}

#[derive(Clone, Copy)]
enum UpdateAction {
    Check,
    Download,
    Cancel,
    Confirm,
    Back,
    Install,
    Discard,
    Restart,
    Close,
}

/// Everything `AnimaticApp::new` needs from the command line and the process.
pub struct Startup {
    pub db_path: PathBuf,
    pub settings_path: PathBuf,
    pub files: Vec<PathBuf>,
    pub full_update: bool,
    /// The executable the process was started from, identified at startup before an update can
    /// replace it. Without it updates are disabled.
    pub installation: Option<Arc<InstallationTarget>>,
    /// Raised when the user restarts after an update; `main` relaunches once the window closed.
    pub restart: Arc<AtomicBool>,
    /// Keystore for the AniDB password and API keys; tests pass an in-memory one.
    pub credentials: Credentials,
    /// Files other launches forward while this window runs; `None` when the single-instance
    /// lock could not be claimed (or is held by another instance).
    pub incoming: Option<animatic_core::instance::Incoming>,
}

/// Longest the window waits for a running library restore when it closes.
const EXIT_RESTORE_WAIT: Duration = Duration::from_secs(60);

/// Steps of **File > Restore library…**, from the validated backup to the swapped files.
enum RestoreStage {
    /// The worker is checking the chosen backup; a modal shows progress and blocks the rest of
    /// the window. Cancel drops the result when it arrives.
    Validating { source: PathBuf },
    /// The worker validated the backup; waiting for the user's decision.
    Confirm { source: PathBuf, info: BackupInfo },
    /// The worker and projection threads are stopped and the UI connection is closed while
    /// the restore thread swaps the files.
    Running {
        source: PathBuf,
        result: Receiver<RestoreResult>,
    },
    /// The library could not be opened again after the swap. The application must not go on
    /// (its own connection is an empty placeholder), so only quitting is offered.
    Broken(String),
}

/// The automatic check is the only update step that runs unasked. It follows the user's choice,
/// never runs in offline mode, and never after a settings recovery (which forces offline mode,
/// so even the in-memory state must not make network requests the user did not enable).
fn startup_check_allowed(settings: &Settings, recovered: bool) -> bool {
    settings.updates.auto_check && !settings.anidb.offline && !recovered
}

/// Folder picker for the AOM databank. Windows folder pickers never list files, so the title
/// says what to choose; `find_databank` accepts the AOM folder or its `kowai` folder.
fn pick_aom_databank_folder() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Choose the AOM folder or its kowai folder (files are not listed)")
        .pick_folder()
}

/// A planned rename of one file, shown for confirmation.
struct RenamePreview {
    id: i64,
    old: PathBuf,
    plan: Plan,
}

/// Batch rename window state: the plan of every ticked file, then the outcome of applying it.
struct BatchRename {
    /// Answers to older plan requests are ignored.
    request: u64,
    /// Number of files being planned.
    files: usize,
    /// `None` while the projection thread is planning.
    rows: Option<Vec<BatchRow>>,
    /// The confirmation was given and the rows that can be applied are with the worker.
    applied: bool,
    /// Outcome per local file id once it is known: how it was moved, or why it was not.
    results: HashMap<i64, Result<String, String>>,
    /// Files renamed and not renamed, once the worker finished the job.
    summary: Option<(usize, usize)>,
}

/// Diagnostics state of the renaming pattern editor in Options.
#[derive(Default)]
struct PatternEditor {
    /// Pattern text the diagnostics below were computed for.
    checked: Option<String>,
    syntax_error: Option<String>,
    /// What the checked text names the fictitious sample file.
    sample: Option<Plan>,
    /// "Custom pattern" was chosen while the text still equals a built-in convention, so the
    /// editor stays open for the user to change it.
    custom: bool,
    /// Custom text replaced by choosing a built-in convention, kept so it can be restored.
    replaced: Option<String>,
    /// Outcome of "Validate on selected file" for the text in `checked`.
    test: Option<Result<RenamePreview, String>>,
    /// The pattern reference window is shown.
    reference_open: bool,
    /// Sample results of the reference's code, per section and entry; computed once.
    reference_values: Vec<Vec<Result<String, String>>>,
}

pub struct AnimaticApp {
    db: Db,
    db_path: PathBuf,
    settings_path: PathBuf,
    settings: Settings,
    projection: LibraryProjection,
    loader: ProjectionLoader,
    views: TableViews,
    /// Zoom factor the last tracked window geometry was measured with.
    window_zoom: f32,
    detail: Option<DetailCache>,
    covers: covers::CoverUi,
    mylist_overlay: HashMap<i64, MylistOverlay>,
    mylist_ui: mylist_editor::MylistUi,
    export_ui: mylist_export::ExportUi,
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
    /// The Anime page lists only anime of this genre (set from the Genres page or a genre in
    /// the anime detail window).
    genre_filter: Option<i64>,
    settings_open: bool,
    about_open: bool,
    /// The license text window opened from About.
    license_open: bool,
    /// The third-party notices window opened from About.
    notices_open: bool,
    /// Ctrl+F asked for the page's search field to take the keyboard focus.
    focus_search: bool,
    /// The status line holds an error that stays until the next message replaces it.
    status_error: bool,
    options_page: OptionsPage,
    clear_queue_confirm: bool,
    selected_local: Option<i64>,
    rename_preview: Option<RenamePreview>,
    batch_rename: Option<BatchRename>,
    /// Automatic rename (roadmap M6.3): when a pass is due and the preview before a folder is
    /// turned on.
    auto_rename: auto_rename::AutoRenameState,
    rename_requests: u64,
    pattern_editor: PatternEditor,
    settings_guard: SettingsGuard,
    /// Startup warning about recovered settings, shown until dismissed.
    settings_notice: Option<String>,
    import_source: String,
    imported_settings: Option<Box<Settings>>,
    import_pending: bool,
    /// Databank waiting for the user to confirm an import into a database that holds data.
    import_confirm: Option<PathBuf>,
    /// Temporary root and the unused marked import folders found there at startup.
    leftover_imports: Option<(PathBuf, Vec<PathBuf>)>,
    search_year: String,
    search_type: i64,
    search_mylist_only: bool,
    search_watched_only: bool,
    activity: Activity,
    status: String,
    logs: Vec<String>,
    updater: Updater,
    update: UpdateState,
    installation: Option<Arc<InstallationTarget>>,
    restart: Arc<AtomicBool>,
    /// Library restore in progress, if any; see [`RestoreStage`].
    restore: Option<RestoreStage>,
    /// The user asked to compact the library and has not answered the confirmation yet.
    compact_confirm: bool,
    full_update: refresh::FullUpdateStage,
    /// Latest backup made in this session, named when a library check fails.
    last_backup: Option<PathBuf>,
    /// Keystore the settings' credential fields are saved to and loaded from.
    credentials: Credentials,
    /// Where the credentials are kept, as of the last load or save; shown in Options.
    credential_storage: CredentialStorage,
    /// File-menu entry state shown in Options.
    shell: shell_menu::ShellUi,
    incoming: Option<animatic_core::instance::Incoming>,
    /// The repaint hook was handed to `incoming` (it needs the egui context).
    incoming_wired: bool,
    /// Files forwarded by another launch, waiting for the worker to accept hashing.
    pending_handoff: Vec<PathBuf>,
    /// System tray icon and hidden-to-tray state; created on the first frame, when an egui
    /// context exists.
    tray: Option<crate::tray::TrayController>,
    /// Player the file actions use, as an index into `settings.players`; `None` means the
    /// default player (see `animatic_core::external::chosen_player`).
    selected_player: Option<usize>,
    /// Opt-in media properties through `ffprobe`: detection, install and the read after hashing.
    media_ui: media_options::MediaUi,
}

impl AnimaticApp {
    pub fn new(startup: Startup) -> anyhow::Result<Self> {
        let Startup {
            db_path,
            settings_path,
            files: initial_files,
            full_update,
            installation,
            restart,
            credentials,
            incoming,
        } = startup;
        let db = Db::open(&db_path)?;
        // The worker may hold the write lock for long stretches; small UI writes must fail
        // visibly instead of freezing the window for the default 30 seconds.
        db.conn().busy_timeout(UI_DB_BUSY_TIMEOUT)?;
        let (loaded, credential_storage) =
            Settings::load_with_credentials(&settings_path, &credentials)?;
        let settings_guard = SettingsGuard {
            recovered: loaded.recovered_from_backup,
            user_saved: false,
        };
        let settings_notice = settings_guard
            .recovered
            .then(|| recovery_notice(&settings_path));
        let settings = loaded.settings;
        let worker = Worker::spawn(db_path.clone());
        let updater = Updater::spawn(installation.clone());
        let media_ui = media_options::MediaUi::new(
            db_path.parent().map(Path::to_path_buf).unwrap_or_default(),
            &settings.media.ffprobe_path,
        );
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
            window_zoom: 1.0,
            detail: None,
            covers: covers::CoverUi::default(),
            mylist_overlay: HashMap::new(),
            mylist_ui: mylist_editor::MylistUi::default(),
            export_ui: mylist_export::ExportUi::default(),
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
            genre_filter: None,
            selected_local: None,
            rename_preview: None,
            batch_rename: None,
            auto_rename: auto_rename::AutoRenameState::default(),
            rename_requests: 0,
            pattern_editor: PatternEditor::default(),
            settings_guard,
            settings_notice,
            credentials,
            credential_storage,
            shell: shell_menu::ShellUi::system(),
            incoming,
            incoming_wired: false,
            pending_handoff: Vec::new(),
            tray: None,
            selected_player: None,
            settings_open: false,
            about_open: false,
            license_open: false,
            notices_open: false,
            focus_search: false,
            status_error: false,
            options_page: OptionsPage::Account,
            clear_queue_confirm: false,
            import_source: String::new(),
            imported_settings: None,
            import_pending: false,
            import_confirm: None,
            leftover_imports: None,
            search_year: String::new(),
            search_type: 0,
            search_mylist_only: false,
            search_watched_only: false,
            activity: Activity::default(),
            status: LOADING_STATUS.into(),
            logs: Vec::new(),
            updater,
            update: UpdateState {
                stage: UpdateStage::Idle,
                release: None,
                ready: None,
                error: None,
                installed_version: None,
                open: false,
            },
            installation,
            restart,
            restore: None,
            compact_confirm: false,
            full_update: refresh::FullUpdateStage::default(),
            last_backup: None,
            media_ui,
        };
        app.request_library_refresh();
        // Off the UI thread: look for decrypted copies an interrupted import left behind.
        app.worker.submit(Job::FindLegacyLeftovers {
            temp_root: animatic_core::legacy::workdir::default_temp_root(),
        });
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
            let pause_between = app.settings.file_managers.file_pause();
            app.worker.submit(Job::Hash {
                paths: initial_files,
                source: HashSource::CommandLine,
                pause_between,
            });
        }
        // Resume files queued before the last quit; local only, so it is not tied to Work offline.
        // The scheduler thread submits it once nothing blocks it (an empty queue does nothing).
        if app.settings.hashing.autostart {
            app.sync_scan_schedule();
            app.worker.scan_control().request_hash_resume();
        }
        // Local only (scans never contact AniDB): the scheduler thread starts it as soon as
        // nothing blocks a scan.
        if app.settings.file_managers.scan_at_start {
            app.sync_scan_schedule();
            app.worker.scan_control().request_scan();
        }
        // The check is the only thing that runs unasked. It never runs against the user's
        // offline choice, nor after a settings recovery (which forces offline mode).
        if startup_check_allowed(&app.settings, app.settings_guard.recovered) {
            app.start_update_check(false);
        }
        Ok(app)
    }

    /// Path of the executable identified at startup.
    fn executable_path(&self) -> Option<&Path> {
        self.installation.as_deref().map(InstallationTarget::path)
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
            self.status_error = false;
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

    /// Whether the command queue may be started now. The Tools menu, the Queue page and the
    /// tray menu all use this one rule.
    fn queue_run_allowed(&self) -> bool {
        !self.import_pending && self.active_cooldown().is_none()
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

    /// Queue a title dump unless AniDB's once-a-day rule says to wait, and then say when it is
    /// next allowed. The queue executor applies the same rule again when the command runs.
    fn queue_titles_dump(&mut self) {
        match self.db.titles_dump_next_allowed(format::now()) {
            Ok(Some(until)) => self.log(animatic_core::limits::titles_dump_wait_message(until)),
            Ok(None) => self.queue_command(animatic_core::sync::Command::TitlesDump),
            Err(e) => {
                log::warn!("cannot read the title dump attempt time: {e}");
                self.queue_command(animatic_core::sync::Command::TitlesDump);
            }
        }
    }

    /// Queue a refresh of one anime unless it was requested within the last day, and then say
    /// when it is next allowed. The queue executor applies the same rule again.
    fn queue_anime_refresh(&mut self, aid: i64) {
        use animatic_core::limits::{RequestKind, anime_wait_message};
        match self
            .db
            .request_next_allowed(RequestKind::Anime, aid, format::now())
        {
            Ok(Some(until)) => self.log(anime_wait_message(aid, until)),
            Ok(None) => self.queue_command(animatic_core::sync::Command::FetchAnime { aid }),
            Err(e) => {
                log::warn!("cannot read the attempt time of anime {aid}: {e}");
                self.queue_command(animatic_core::sync::Command::FetchAnime { aid });
            }
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

    /// Adds to the log page without replacing the status line (background results nobody
    /// waits for).
    fn note(&mut self, text: impl Into<String>) {
        self.logs.push(format!(
            "{}  {}",
            chrono::Local::now().format("%H:%M:%S"),
            text.into()
        ));
    }

    fn log(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.status = text.clone();
        self.status_error = false;
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
        self.status_error = true;
        self.logs.push(format!(
            "{}  ERROR  {text}",
            chrono::Local::now().format("%H:%M:%S")
        ));
    }

    fn process_update_events(&mut self, ctx: &egui::Context) {
        let events: Vec<_> = self.updater.events.try_iter().collect();
        for event in events {
            match event {
                UpdateEvent::Checked(release) => {
                    let found = release.is_some();
                    if self.update.stage.check_finished(found) {
                        match release {
                            Some(release) => {
                                self.note(format!(
                                    "Animatic {} is available (Help → Check for updates…)",
                                    release.version
                                ));
                                self.update.release = Some(*release);
                            }
                            None => {
                                self.update.release = None;
                                self.note("No newer Animatic release found");
                            }
                        }
                    }
                }
                UpdateEvent::CheckFailed(message) => match self.update.stage.check_failed() {
                    Some(true) => {
                        self.error(format!("Update check failed: {message}"));
                        self.update.error = Some(message);
                    }
                    Some(false) => self.note(format!("Update check failed: {message}")),
                    None => {}
                },
                UpdateEvent::Progress { done, total } => {
                    self.update.stage.progress(done, total);
                }
                UpdateEvent::Ready(update) => {
                    if self.update.stage.download_ready() {
                        self.log(format!(
                            "Update {} downloaded and verified",
                            update.version()
                        ));
                        self.update.ready = Some(update);
                    }
                }
                UpdateEvent::DownloadFailed(message) => {
                    if self.update.stage.download_failed() {
                        self.error(format!("Update download failed: {message}"));
                        self.update.error = Some(message);
                    }
                }
                UpdateEvent::Cancelled => {
                    self.update.stage.cancelled();
                    self.note("Update check or download cancelled");
                }
                UpdateEvent::Installed(version) => {
                    if self.update.stage.installed() {
                        self.log(format!(
                            "Animatic {version} installed; restart to start using it"
                        ));
                        self.update.installed_version = Some(version);
                    }
                }
                UpdateEvent::InstallFailed { message, attempted } => {
                    if self.update.stage.install_failed(attempted) {
                        self.error(format!("Update installation failed: {message}"));
                        self.update.error = Some(message);
                    }
                }
            }
            ctx.request_repaint();
        }
    }

    /// Why installing or restarting must wait, from the state of the application itself (not
    /// from events the window may not have processed yet).
    fn update_blocker(&self) -> Option<&'static str> {
        if self.installation.is_none() {
            Some("Animatic cannot tell where it is installed.")
        } else if self.restore.is_some() {
            Some("A library restore is in progress.")
        } else if !self.worker.is_idle() || self.activity.busy {
            Some("Wait for the running task to finish, or cancel it, first.")
        } else if self.import_confirm.is_some() {
            Some("Confirm or cancel the import first.")
        } else if self.import_pending {
            Some("An import is in progress.")
        } else if self.imported_settings.is_some() {
            Some("Apply or cancel the imported settings first.")
        } else if !self.deferred.is_empty() {
            Some("Some changes are not saved to the database yet.")
        } else {
            None
        }
    }

    fn start_update_check(&mut self, manual: bool) {
        if Platform::current().is_none() {
            return;
        }
        if self.update.stage.begin_check(manual) {
            self.update.error = None;
            self.updater.submit(UpdateJob::Check);
        }
    }

    fn start_update_download(&mut self) {
        let Some(release) = self.update.release.clone() else {
            return;
        };
        if self.update.stage.begin_download() {
            self.update.error = None;
            self.updater.submit(UpdateJob::Download(Box::new(release)));
        }
    }

    fn install_update(&mut self) {
        if let Some(reason) = self.update_blocker() {
            self.update.error = Some(reason.into());
            return;
        }
        if self.update.ready.is_some() && self.update.stage.begin_install() {
            self.update.error = None;
            if let Some(update) = self.update.ready.take() {
                self.log("Installing the update…");
                self.updater.submit(UpdateJob::Install(update));
            }
        }
    }

    /// Restarting also needs the settings to be savable, since the window closing for it saves
    /// them: a recovered settings file stays untouched until the user saves it explicitly.
    fn restart_blocker(&self) -> Option<&'static str> {
        self.update_blocker().or_else(|| {
            (!self.settings_guard.allows_exit_save()).then_some(
                "The settings were recovered from a backup. Save them first in Tools → Options…, \
                 so that restarting does not decide about the damaged settings file.",
            )
        })
    }

    /// Saves the settings, then closes the window; `main` starts the new executable once
    /// `run_native` has returned and the application (worker, database connections) is gone.
    /// A failed save keeps the application open: closing cannot report it any more.
    fn restart_now(&mut self, ctx: &egui::Context) {
        if self.update.stage != UpdateStage::Installed {
            return;
        }
        if let Some(reason) = self.restart_blocker() {
            self.update.error = Some(reason.into());
            return;
        }
        if let Err(e) = self.write_settings() {
            let message = format!("Could not save settings, so Animatic was not restarted: {e}");
            self.error(message.clone());
            self.update.error = Some(message);
            return;
        }
        self.update.error = None;
        self.restart.store(true, Ordering::SeqCst);
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Text of the nonblocking top-bar notice, if the update flow has news.
    fn update_notice(&self) -> Option<String> {
        match (&self.update.stage, &self.update.release) {
            (UpdateStage::Available, Some(release)) => {
                Some(format!("Update available: {}", release.version))
            }
            (UpdateStage::Ready { .. }, _) => Some("Update ready to install".into()),
            (UpdateStage::Installed, _) => Some("Restart to finish updating".into()),
            (UpdateStage::InstallFailed, _) => Some("Update failed: see details".into()),
            _ => None,
        }
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
                    self.media_note_hashed(local_id, &path);
                    self.log(format!(
                        "Hashed {} (local #{local_id}, {ed2k})",
                        path.display()
                    ));
                    self.queue_after_hash(&path, size, ed2k);
                    self.auto_rename.mark_due();
                    self.request_library_refresh();
                }
                Event::FullUpdatePreview(report) => {
                    if matches!(self.full_update, refresh::FullUpdateStage::Computing) {
                        self.full_update = refresh::FullUpdateStage::Confirm(report);
                    }
                }
                Event::Imported { summary } => {
                    self.log(summary);
                    self.auto_rename.mark_due();
                    self.request_library_refresh();
                }
                Event::LegacyImportNeedsConfirmation { source } => {
                    self.import_confirm = Some(source);
                }
                Event::LegacyLeftovers { temp_root, folders } => {
                    self.leftover_imports = Some((temp_root, folders));
                }
                Event::LibraryBackedUp { path, bytes } => {
                    self.log(format!(
                        "Library backed up to {} ({})",
                        path.display(),
                        format::size_short(bytes as i64)
                    ));
                    self.last_backup = Some(path);
                }
                Event::RestoreCandidate { source, info } => {
                    // Only the backup still being checked; a cancelled check is ignored.
                    if matches!(&self.restore, Some(RestoreStage::Validating { source: s }) if *s == source)
                    {
                        self.restore = Some(RestoreStage::Confirm { source, info });
                    }
                }
                Event::LibraryChecked(report) => self.library_checked(&report),
                Event::LibraryCompacted(report) => self.log(format!(
                    "Library compacted: {} → {}",
                    format::size_short(report.before as i64),
                    format::size_short(report.after as i64)
                )),
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
                        self.auto_rename.mark_due();
                        self.request_library_refresh();
                    }
                }
                Event::Renamed {
                    local_id,
                    old,
                    new,
                    copied,
                    note,
                } => {
                    self.log(format!(
                        "Renamed {} to {}{}",
                        old.display(),
                        new.display(),
                        if copied {
                            " (copied to the other volume and verified)"
                        } else {
                            ""
                        }
                    ));
                    if let Some(note) = &note {
                        self.error(note.clone());
                    }
                    self.mylist_ui.marked_locals.remove(&local_id);
                    if let Some(batch) = &mut self.batch_rename {
                        let how = match (copied, note) {
                            (_, Some(note)) => note,
                            (true, None) => "Renamed (copied and verified)".to_string(),
                            (false, None) => "Renamed".to_string(),
                        };
                        batch.results.insert(local_id, Ok(how));
                    }
                    self.request_library_refresh();
                }
                Event::RenameFailed {
                    local_id,
                    old,
                    new,
                    message,
                } => {
                    self.error(format!(
                        "Rename failed: {}: {message} (target {})",
                        old.display(),
                        new.display()
                    ));
                    if let Some(batch) = &mut self.batch_rename {
                        batch.results.insert(local_id, Err(message));
                    }
                }
                Event::RenameSummary {
                    renamed,
                    not_renamed,
                } => {
                    if let Some(batch) = &mut self.batch_rename {
                        batch.summary = Some((renamed, not_renamed));
                    }
                    self.log(format!(
                        "Rename finished: {renamed} renamed, {not_renamed} not renamed"
                    ));
                }
                Event::RenamesPlanned { request, rows } => {
                    if let Some(rows) = self.auto_rename.accept_plan(request, rows)
                        && let Some(batch) = &mut self.batch_rename
                        && batch.request == request
                    {
                        batch.rows = Some(rows);
                    }
                }
                Event::Info(text) => self.log(text),
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
                Event::CoverLoaded { picname, outcome } => {
                    self.covers.on_loaded(ctx, picname, outcome);
                }
                // Not a dirty signal: statistics export failures arrive as errors and would
                // otherwise re-request the projection (and the export) forever.
                Event::Error(e) => {
                    if matches!(self.full_update, refresh::FullUpdateStage::Computing) {
                        self.full_update = refresh::FullUpdateStage::Closed;
                    }
                    self.error(e)
                }
                Event::HashQueueChanged => self.request_library_refresh(),
                Event::Finished => {
                    // A check that ended without a candidate was refused (the error is logged).
                    if matches!(self.restore, Some(RestoreStage::Validating { .. })) {
                        self.restore = None;
                    }
                    self.activity.busy = false;
                    // An error from the job stays readable; otherwise the line returns to Ready.
                    if !self.status_error {
                        self.status = "Ready".into();
                    }
                    self.request_library_refresh();
                    self.media_after_job();
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
            self.worker.submit(Job::Hash {
                paths: files,
                source: HashSource::AddFiles,
                pause_between: self.settings.file_managers.file_pause(),
            });
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
        self.worker.submit(Job::ImportLegacySettings { source });
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
        dialog::window(ctx, "Review imported AOM settings")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.strong("Applying will replace your current Animatic settings.");
                ui.colored_label(
                    style::warn(ui.visuals()),
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
            imported.keep_preferences_unknown_to_legacy(&self.settings);
            imported.anidb.offline = true;
            match imported.save(&self.settings_path, &self.credentials) {
                Ok(storage) => {
                    self.credential_storage = storage;
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
        if self.import_pending || self.import_confirm.is_some() {
            return;
        }
        let picked = PathBuf::from(self.import_source.trim());
        let Some(source) = animatic_core::legacy::import::find_databank(&picked) else {
            self.error(format!(
                "No AOM databank found in {} (choose the AOM folder or its kowai folder)",
                picked.display()
            ));
            return;
        };
        self.submit_legacy_import(source, false);
    }

    /// The worker decides whether the database needs confirmation (it looks at the database,
    /// which the UI thread must not): an unconfirmed import into a database that holds data
    /// answers with `Event::LegacyImportNeedsConfirmation` instead of writing.
    fn submit_legacy_import(&mut self, source: PathBuf, confirmed: bool) {
        self.worker.submit(Job::ImportLegacy {
            source,
            temp_root: animatic_core::legacy::workdir::default_temp_root(),
            confirmed,
        });
    }

    /// Asks before an AOM databank is imported into a database that already holds data.
    fn legacy_import_confirm_window(&mut self, ctx: &egui::Context) {
        let Some(source) = self.import_confirm.clone() else {
            return;
        };
        let mut proceed = false;
        let mut cancelled = false;
        let mut open = true;
        dialog::window(ctx, "Import into a database that has data?")
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(format!(
                    "The database {} already holds library data.",
                    self.db_path.display()
                ));
                ui.label(
                    "Importing the AOM databank replaces every row that has the same id as a row \
                     in the databank: anime, episodes, files, MyList entries and the hashed \
                     local files. Rows with other ids are kept.",
                );
                ui.label(
                    "To keep this database as it is, cancel and start Animatic with \
                     --database FILE (a new file) to import into an empty database instead.",
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Continue").clicked() {
                        proceed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancelled = true;
                    }
                });
            });
        if proceed {
            self.import_confirm = None;
            self.submit_legacy_import(source, true);
        } else if cancelled || !open {
            self.import_confirm = None;
            self.log("Import cancelled; the database was not changed");
        }
    }

    /// Offers to remove decrypted temporary copies that an interrupted import left behind.
    fn leftover_import_window(&mut self, ctx: &egui::Context) {
        let Some((temp_root, folders)) = self.leftover_imports.as_ref() else {
            return;
        };
        let mut remove = false;
        let mut keep = false;
        dialog::window(ctx, "Leftover import files")
            .resizable(false)
            .show(ctx, |ui| {
                ui.colored_label(
                    style::warn(ui.visuals()),
                    "An earlier import of an AOM databank did not finish cleanly. It left \
                 decrypted copies of the databank in these temporary folders:",
                );
                for folder in folders {
                    ui.monospace(folder.display().to_string());
                }
                ui.label(
                    "Removing them deletes only these Animatic import folders; nothing else in \
                     the temporary folder and not your AOM databank. If you keep them, \
                     Animatic asks again at the next start.",
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Remove").clicked() {
                        remove = true;
                    }
                    if ui.button("Keep for now").clicked() {
                        keep = true;
                    }
                });
            });
        if remove {
            let (temp_root, folders) = (temp_root.clone(), folders.clone());
            self.leftover_imports = None;
            self.worker
                .submit(Job::RemoveLegacyLeftovers { temp_root, folders });
        } else if keep {
            self.leftover_imports = None;
        }
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
        let plan = plan::plan_local_file(&self.db, pattern, fid, &old, &manager.base_path);
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
        let shown = dialog::window(ctx, "Confirm rename")
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
        if dialog::escape_closes(ctx, shown.as_ref()) {
            open = false;
        }
        if apply {
            if let Some(new) = plan.target.clone().filter(|_| plan.can_apply()) {
                self.worker.submit(Job::RenameFiles {
                    items: vec![RenameItem {
                        local_id: id,
                        old,
                        new,
                    }],
                    managers: self.settings.file_managers.clone(),
                });
            }
            self.rename_preview = None;
        } else if edit_pattern {
            self.settings_open = true;
            self.options_page = OptionsPage::Renaming;
            self.rename_preview = None;
        } else if !open {
            self.rename_preview = None;
        }
    }

    /// Every hashed file selected on **Hashed files** (Ctrl/Shift selection plus the primary
    /// one): the one selection that Add to MyList… and Rename N selected… act on.
    fn selected_locals(&self) -> BTreeSet<i64> {
        let mut selected = self.mylist_ui.marked_locals.clone();
        selected.extend(self.selected_local);
        selected
    }

    /// Plan the rename of every selected file on the projection thread; its answer opens the
    /// preview. Nothing is read or planned on the UI thread.
    fn prepare_batch_rename(&mut self) {
        if self.batch_rename.is_some() {
            return;
        }
        let wanted = self.selected_locals();
        let files: Vec<Candidate> = self
            .views
            .local
            .rows
            .iter()
            .map(|&i| &self.projection.local[i])
            .filter(|f| wanted.contains(&f.id))
            .map(|f| {
                let path = PathBuf::from(&f.path);
                let base_path = self
                    .settings
                    .file_managers
                    .manager_for(&path)
                    .base_path
                    .clone();
                Candidate {
                    local_id: f.id,
                    path,
                    fid: f.fid,
                    base_path,
                }
            })
            .collect();
        if files.is_empty() {
            self.error("Select hashed files for the batch rename first");
            return;
        }
        self.rename_requests += 1;
        let request = self.rename_requests;
        self.batch_rename = Some(BatchRename {
            request,
            files: files.len(),
            rows: None,
            applied: false,
            results: HashMap::new(),
            summary: None,
        });
        self.worker.submit(Job::PlanRenames {
            request,
            pattern: self.settings.renaming_pattern.clone(),
            files,
        });
    }

    /// Preview of a batch rename: one line per file with its planned target or its problem.
    /// "Rename N files" is the one explicit confirmation; it applies only the valid rows.
    fn batch_rename_window(&mut self, ctx: &egui::Context) {
        let Some(batch) = self.batch_rename.as_ref() else {
            return;
        };
        let mut open = true;
        let mut close = false;
        let mut apply = false;
        let mut edit_pattern = false;
        let shown = dialog::window(ctx, "Batch rename")
            .open(&mut open)
            .default_width(760.0)
            .show(ctx, |ui| {
                let Some(rows) = &batch.rows else {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Planning the renames of {} files…", batch.files));
                    });
                    return;
                };
                let valid = rows.iter().filter(|r| r.plan.can_apply()).count();
                let named = rows.iter().filter(|r| r.plan.already_named()).count();
                let blocked = rows.len() - valid - named;
                ui.label(format!(
                    "{valid} of {} files can be renamed, {blocked} blocked, {named} already \
                     named as the pattern says.",
                    rows.len()
                ));
                if blocked > 0 && !batch.applied {
                    ui.label(
                        "Blocked files are skipped; hover a line for every problem. Only the \
                         ready files are renamed.",
                    );
                }
                ui.add_space(6.0);
                let line = ui.text_style_height(&egui::TextStyle::Body);
                egui::ScrollArea::vertical()
                    .max_height(340.0)
                    .auto_shrink([false, true])
                    .show_rows(ui, line, rows.len(), |ui, range| {
                        for row in &rows[range] {
                            batch_rename_line(
                                ui,
                                row,
                                batch.applied,
                                batch.results.get(&row.local_id),
                            );
                        }
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if batch.applied {
                        match batch.summary {
                            Some((renamed, not_renamed)) => {
                                ui.label(format!(
                                    "Renamed {renamed} files, {not_renamed} not renamed."
                                ));
                            }
                            None => {
                                ui.spinner();
                                ui.label("Renaming…");
                            }
                        }
                    } else {
                        let label = if valid == 1 {
                            "Rename 1 file".to_string()
                        } else {
                            format!("Rename {valid} files")
                        };
                        if ui
                            .add_enabled(valid > 0, egui::Button::new(label))
                            .clicked()
                        {
                            apply = true;
                        }
                        if blocked > 0 && ui.button("Edit pattern in Options").clicked() {
                            edit_pattern = true;
                        }
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        if dialog::escape_closes(ctx, shown.as_ref()) {
            close = true;
        }
        if apply
            && let Some(batch) = self.batch_rename.as_mut()
            && let Some(rows) = &batch.rows
        {
            let items: Vec<RenameItem> = rows
                .iter()
                .filter(|r| r.plan.can_apply())
                .filter_map(|r| {
                    r.plan.target.clone().map(|new| RenameItem {
                        local_id: r.local_id,
                        old: r.old.clone(),
                        new,
                    })
                })
                .collect();
            batch.applied = true;
            self.worker.submit(Job::RenameFiles {
                items,
                managers: self.settings.file_managers.clone(),
            });
        } else if edit_pattern {
            self.settings_open = true;
            self.options_page = OptionsPage::Renaming;
            self.batch_rename = None;
        } else if close || !open {
            self.batch_rename = None;
        }
    }

    /// Options section of the renaming pattern: a built-in naming convention or a custom
    /// pattern, a live example, the reference and a dry run on the selected file.
    fn pattern_editor_ui(&mut self, ui: &mut egui::Ui) {
        let kind = plan::classify(&self.settings.renaming_pattern);
        let preset = match kind {
            PatternKind::Preset(preset) if !self.pattern_editor.custom => Some(preset),
            _ => None,
        };
        let mut chosen = None;
        ui.horizontal(|ui| {
            ui.label("Naming convention");
            egui::ComboBox::from_id_salt("naming-convention")
                .selected_text(preset.map_or("Custom pattern", |p| p.name))
                .width(340.0)
                .show_ui(ui, |ui| {
                    for p in presets::PRESETS {
                        if ui
                            .selectable_label(preset == Some(p), p.name)
                            .on_hover_text(p.description)
                            .clicked()
                        {
                            chosen = Some(Some(p));
                        }
                    }
                    if ui
                        .selectable_label(preset.is_none(), "Custom pattern")
                        .on_hover_text("Write your own pattern in Animatic's renaming language")
                        .clicked()
                    {
                        chosen = Some(None);
                    }
                });
        });
        match chosen {
            Some(Some(p)) if preset != Some(p) => {
                let old = std::mem::replace(&mut self.settings.renaming_pattern, p.source.into());
                if plan::classify(&old) == PatternKind::Custom {
                    self.pattern_editor.replaced = Some(old);
                }
                self.pattern_editor.custom = false;
            }
            // Start from the convention that was selected; editing it makes it custom.
            Some(None) => self.pattern_editor.custom = true,
            _ => {}
        }
        let preset = match plan::classify(&self.settings.renaming_pattern) {
            PatternKind::Preset(preset) if !self.pattern_editor.custom => Some(preset),
            _ => None,
        };
        match preset {
            Some(p) => {
                ui.label(p.description);
            }
            None => {
                ui.label(
                    "Your own pattern in Animatic's renaming language, kept exactly as written. \
                     The reference explains how to write one; a built-in convention is a good \
                     starting point.",
                );
            }
        }
        if self.pattern_editor.checked.as_deref() != Some(self.settings.renaming_pattern.as_str()) {
            self.pattern_editor.syntax_error =
                plan::check_pattern(&self.settings.renaming_pattern).err();
            self.pattern_editor.sample =
                Some(presets::sample_plan(&self.settings.renaming_pattern));
            self.pattern_editor.checked = Some(self.settings.renaming_pattern.clone());
            self.pattern_editor.test = None;
        }
        if let Some(sample) = &self.pattern_editor.sample
            && !sample.result.is_empty()
        {
            ui.horizontal_wrapped(|ui| {
                ui.label("Example:")
                    .on_hover_text("A fictitious episode 6, release version 2, by a group");
                ui.monospace(shortened(&sample.result, 300));
            });
        }
        let mut restore = false;
        let mut validate = false;
        ui.horizontal_wrapped(|ui| {
            if preset.is_none()
                && ui
                    .button("Pattern reference")
                    .on_hover_text("Values, functions and recipes for writing a pattern")
                    .clicked()
            {
                self.pattern_editor.reference_open = true;
            }
            if self.pattern_editor.replaced.is_some() {
                restore = ui
                    .button("Restore my custom pattern")
                    .on_hover_text("Put back the custom pattern the convention replaced")
                    .clicked();
            }
            validate = ui
                .button("Validate on selected file")
                .on_hover_text("Run the pattern on the file selected on the Hashed files page")
                .clicked();
        });
        if restore && let Some(old) = self.pattern_editor.replaced.take() {
            self.settings.renaming_pattern = old;
            self.pattern_editor.custom = true;
        }
        if preset.is_none() {
            ui.add(
                egui::TextEdit::multiline(&mut self.settings.renaming_pattern)
                    // A fixed id: the example line above appears and disappears while typing,
                    // which would otherwise change the automatic id and drop the focus.
                    .id_salt("renaming-pattern")
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .desired_rows(12),
            );
            if let Some(error) = &self.pattern_editor.syntax_error {
                ui.colored_label(style::error(ui.visuals()), error);
            } else if let Some(sample) = &self.pattern_editor.sample {
                show_plan(ui, sample);
            }
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
        match &self.pattern_editor.test {
            Some(Err(e)) => {
                ui.colored_label(style::error(ui.visuals()), e);
            }
            Some(Ok(preview)) => {
                if !preview.plan.result.is_empty() {
                    ui.label("Result for the selected file:");
                    ui.monospace(&preview.plan.result);
                }
                show_plan(ui, &preview.plan);
                if preview.plan.can_apply() || preview.plan.already_named() {
                    ui.colored_label(
                        style::ok(ui.visuals()),
                        "The pattern is valid for this file.",
                    );
                }
            }
            None => {}
        }
    }

    /// How to write a custom pattern, with every sample's result for the fictitious file.
    fn pattern_reference_window(&mut self, ctx: &egui::Context) {
        if !self.pattern_editor.reference_open {
            return;
        }
        if self.pattern_editor.reference_values.is_empty() {
            self.pattern_editor.reference_values = reference::SECTIONS
                .iter()
                .map(|s| {
                    s.entries
                        .iter()
                        .map(|e| presets::sample_value(e.code))
                        .collect()
                })
                .collect();
        }
        let mut open = true;
        let shown = dialog::window(ctx, "Renaming pattern reference")
            .open(&mut open)
            .default_width(720.0)
            .default_height(560.0)
            .vscroll(true)
            .show(ctx, |ui| {
                ui.label(
                    "Results are for a fictitious file: episode 6 of \"Reibun no Tabi\", \
                     release version 2, by the group ExGroup.",
                );
                ui.hyperlink_to(
                    "Complete language reference (wiki)",
                    "https://github.com/RetroHazard/animatic/wiki/Rename-Language",
                );
                for (index, section) in reference::SECTIONS.iter().enumerate() {
                    egui::CollapsingHeader::new(section.title)
                        .default_open(index == 0)
                        .show(ui, |ui| {
                            for note in section.notes {
                                ui.label(format!("• {note}"));
                            }
                            for (entry, value) in section
                                .entries
                                .iter()
                                .zip(&self.pattern_editor.reference_values[index])
                            {
                                ui.separator();
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(RichText::new(entry.meaning).strong());
                                    if ui
                                        .small_button("Copy")
                                        .on_hover_text("Copy the code to the clipboard")
                                        .clicked()
                                    {
                                        ui.ctx().copy_text(entry.code.to_string());
                                    }
                                });
                                ui.add(egui::Label::new(RichText::new(entry.code).monospace()));
                                match value {
                                    Ok(v) => {
                                        ui.label(RichText::new(format!("→ {v}")).monospace().weak())
                                    }
                                    Err(e) => ui.colored_label(style::error(ui.visuals()), e),
                                };
                            }
                        });
                }
            });
        if !open || dialog::escape_closes(ctx, shown.as_ref()) {
            self.pattern_editor.reference_open = false;
        }
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let repository = env!("CARGO_PKG_REPOSITORY");
        let mut open = true;
        let shown = dialog::window(ctx, "About Animatic")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("Animatic")
                        .text_style(egui::TextStyle::Heading)
                        .color(style::accent(ui.visuals())),
                );
                ui.label("Hash, identify, rename and track your anime library with AniDB.");
                ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).weak());
                ui.label(format!(
                    "Created by {}",
                    env!("CARGO_PKG_AUTHORS").replace(':', ", ")
                ));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    for (label, url) in [
                        ("Source code", repository.to_string()),
                        ("Documentation", format!("{repository}/wiki")),
                        ("AniDB website", "https://anidb.net/".to_string()),
                    ] {
                        if ui.button(label).on_hover_text(&url).clicked()
                            && let Err(e) = open::that(&url)
                        {
                            self.error(format!("Could not open {url}: {e}"));
                        }
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("View license").clicked() {
                        self.license_open = true;
                    }
                    if ui
                        .button("Third-party notices")
                        .on_hover_text(
                            "Licenses of the open-source components built into this version",
                        )
                        .clicked()
                    {
                        self.notices_open = true;
                    }
                });
            });
        if !open || dialog::escape_closes(ctx, shown.as_ref()) {
            self.about_open = false;
        }
    }

    /// Animatic's own license, shown in the app. The text is the one this build was made with,
    /// so it needs no file next to the program and no program to open an extensionless file.
    fn license_window(&mut self, ctx: &egui::Context) {
        const LICENSE: &str = include_str!("../../../LICENSE");
        if !self.license_open {
            return;
        }
        // Wide enough for the longest line, so the license does not wrap at its default size;
        // narrower screens and a resized window still wrap.
        let style = ctx.global_style();
        let font = egui::TextStyle::Monospace.resolve(&style);
        let text_width = ctx.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(LICENSE.to_owned(), font, Color32::PLACEHOLDER)
                .size()
                .x
        });
        // `default_width` is the outer width: add the window frame and the scroll bar.
        let width = text_width
            + egui::Frame::window(&style).total_margin().sum().x
            + style.spacing.scroll.allocated_width()
            + 8.0;
        let mut open = true;
        let shown = dialog::window(ctx, "Animatic license")
            .open(&mut open)
            .default_width(width.min(ctx.content_rect().width() * 0.9))
            .max_height(ctx.content_rect().height() * 0.8)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(LICENSE).monospace()).selectable(true));
                });
            });
        if !open || dialog::escape_closes(ctx, shown.as_ref()) {
            self.license_open = false;
        }
    }

    /// The third-party notices (the root `NOTICES` file) built into this version. The text is
    /// long, so only the visible lines are laid out, without wrapping.
    fn notices_window(&mut self, ctx: &egui::Context) {
        static LINES: std::sync::LazyLock<Vec<&'static str>> =
            std::sync::LazyLock::new(|| include_str!("../../../NOTICES").lines().collect());
        if !self.notices_open {
            return;
        }
        // Room for the notices' 100-column prose; longer lines scroll sideways.
        let style = ctx.global_style();
        let font = egui::TextStyle::Monospace.resolve(&style);
        let text_width = ctx.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap("M".repeat(100), font, Color32::PLACEHOLDER)
                .size()
                .x
        });
        let width = text_width
            + egui::Frame::window(&style).total_margin().sum().x
            + style.spacing.scroll.allocated_width()
            + 8.0;
        let mut open = true;
        let shown = dialog::window(ctx, "Third-party notices")
            .open(&mut open)
            .default_width(width.min(ctx.content_rect().width() * 0.9))
            .default_height(ctx.content_rect().height() * 0.7)
            .show(ctx, |ui| {
                let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
                egui::ScrollArea::both().auto_shrink(false).show_rows(
                    ui,
                    row_height,
                    LINES.len(),
                    |ui, rows| {
                        for line in &LINES[rows] {
                            ui.add(
                                egui::Label::new(RichText::new(*line).monospace())
                                    .extend()
                                    .selectable(true),
                            );
                        }
                    },
                );
            });
        if !open || dialog::escape_closes(ctx, shown.as_ref()) {
            self.notices_open = false;
        }
    }

    fn settings_notice_window(&mut self, ctx: &egui::Context) {
        let Some(text) = self.settings_notice.as_ref() else {
            return;
        };
        let mut dismissed = false;
        dialog::window(ctx, "Settings recovered")
            .resizable(false)
            .show(ctx, |ui| {
                ui.colored_label(style::warn(ui.visuals()), text);
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
        let Some(index) =
            animatic_core::external::chosen_player(&self.settings.players, self.selected_player)
        else {
            self.error("No external player is configured");
            return;
        };
        let player = &self.settings.players[index];
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
        // The projection thread reads the hashed files, expands the template and writes the file
        // atomically; the outcome returns as a log or error event.
        self.worker.submit(Job::ExportHashes {
            path,
            template: self.settings.hashing.export_template.clone(),
            copy_length_as_minutes: self.settings.ui.copy_length_as_minutes,
        });
    }

    /// Saves the settings file; the credential fields go to the system keystore when it works.
    /// Records where they ended up for Options.
    fn write_settings(&mut self) -> animatic_core::Result<()> {
        self.credential_storage = self.settings.save(&self.settings_path, &self.credentials)?;
        Ok(())
    }

    fn save_settings(&mut self) {
        match self.write_settings() {
            Ok(()) => {
                self.settings_guard.user_saved = true;
                self.settings_notice = None;
                self.log("Settings saved");
            }
            Err(e) => self.error(format!("Could not save settings: {e}")),
        }
    }

    /// An Options save applies the file-menu toggle; nothing else ever registers the entry
    /// (an AOM settings import only stores the flag).
    fn apply_shell_integration(&mut self, exe: Option<&Path>) {
        let enabled = self.settings.hashing.integrate_into_shell;
        self.shell.apply(enabled, exe);
    }

    fn poll_shell(&mut self, ctx: &egui::Context) {
        if let Some(finished) = self.shell.poll() {
            match finished.change {
                Some(Ok(message)) => self.log(message),
                Some(Err(e)) => self.error(format!("File-menu entry: {e}")),
                None => {}
            }
        }
        if self.shell.is_pending() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// Hashes files another launch forwarded, like the files given on the command line, and
    /// brings the window forward. Held back while an import review blocks the worker.
    fn process_handoff(&mut self, ctx: &egui::Context) {
        if let Some(incoming) = &self.incoming {
            if !self.incoming_wired {
                let ctx = ctx.clone();
                incoming.set_waker(move || ctx.request_repaint());
                self.incoming_wired = true;
            }
            let mut arrived = false;
            while let Some(files) = incoming.try_recv() {
                self.pending_handoff.extend(files);
                arrived = true;
            }
            if arrived {
                self.restore_main_window(ctx);
            }
        }
        if !self.pending_handoff.is_empty() && !self.import_pending {
            let files = std::mem::take(&mut self.pending_handoff);
            self.log(format!(
                "Hashing {} file(s) from another launch",
                files.len()
            ));
            self.worker.submit(Job::Hash {
                paths: files,
                source: HashSource::Handoff,
                pause_between: self.settings.file_managers.file_pause(),
            });
        }
    }

    /// Store settings after a change the user did not ask to save (for example dismissing the
    /// first-run card). Recovered settings keep the unreadable original untouched, so the change
    /// then stays in memory until Options "Save" or an applied import; no save state changes.
    fn persist_settings_incidentally(&mut self) {
        if !self.settings_guard.allows_exit_save() {
            return;
        }
        if let Err(e) = self.write_settings() {
            self.error(format!("Could not save settings: {e}"));
        }
    }

    fn top_bar(&mut self, root: &mut egui::Ui) {
        const IMPORTING: &str = "An import is in progress";
        const QUEUED_ONLY: &str = "Adds a command to the queue; nothing is sent to AniDB until \
                                   the command queue runs.";
        let ctx = root.ctx().clone();
        let cooling = self
            .active_cooldown()
            .map(|(until, reason)| Self::cooldown_text(until, reason));
        let library_blocker = self.library_task_blocker();
        let restore_blocker = self.restore_start_blocker();
        egui::Panel::top("menu").show(root, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui
                        .add_enabled(
                            !self.import_pending,
                            menu_entry(&ctx, "Add files…", &shortcuts::ADD_FILES),
                        )
                        .on_disabled_hover_text(IMPORTING)
                        .clicked()
                    {
                        ui.close();
                        self.submit_files();
                    }
                    ui.separator();
                    ui.menu_button("Import", |ui| {
                        if ui
                            .add_enabled(!self.import_pending, egui::Button::new("AOM databank…"))
                            .on_disabled_hover_text(IMPORTING)
                            .clicked()
                        {
                            ui.close();
                            if let Some(dir) = pick_aom_databank_folder() {
                                self.import_source = dir.to_string_lossy().into_owned();
                                self.import_legacy();
                            }
                        }
                        if ui
                            .add_enabled(!self.import_pending, egui::Button::new("AOM settings…"))
                            .on_disabled_hover_text(IMPORTING)
                            .clicked()
                        {
                            ui.close();
                            self.import_legacy_settings();
                        }
                        if ui
                            .add_enabled(
                                library_blocker.is_none(),
                                egui::Button::new("MyList export…"),
                            )
                            .on_disabled_hover_text(library_blocker.unwrap_or_default())
                            .clicked()
                        {
                            ui.close();
                            self.import_mylist_export();
                        }
                    });
                    ui.menu_button("Export", |ui| {
                        if ui.button("Statistics…").clicked() {
                            ui.close();
                            self.export_statistics();
                        }
                        if ui.button("Hashes…").clicked() {
                            ui.close();
                            self.export_hashes();
                        }
                    });
                    ui.separator();
                    if ui
                        .add_enabled(
                            library_blocker.is_none(),
                            egui::Button::new("Back up library…"),
                        )
                        .on_disabled_hover_text(library_blocker.unwrap_or_default())
                        .clicked()
                    {
                        ui.close();
                        self.backup_library();
                    }
                    if ui
                        .add_enabled(
                            restore_blocker.is_none(),
                            egui::Button::new("Restore library…"),
                        )
                        .on_disabled_hover_text(restore_blocker.unwrap_or_default())
                        .clicked()
                    {
                        ui.close();
                        self.restore_library();
                    }
                    ui.separator();
                    if ui
                        .add(menu_entry(&ctx, "Reload", &shortcuts::RELOAD))
                        .on_hover_text("Re-read the library from disk. Nothing is sent to AniDB.")
                        .clicked()
                    {
                        ui.close();
                        self.request_library_refresh();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Tools", |ui| {
                    if ui
                        .add(menu_entry(&ctx, "Options…", &shortcuts::OPTIONS))
                        .clicked()
                    {
                        self.settings_open = true;
                        ui.close();
                    }
                    ui.separator();
                    let run = ui
                        .add_enabled(
                            self.queue_run_allowed(),
                            menu_entry(&ctx, "Run command queue", &shortcuts::RUN_QUEUE),
                        )
                        .on_disabled_hover_text(cooling.as_deref().unwrap_or_default());
                    if run.clicked() {
                        self.run_command_queue();
                        ui.close();
                    }
                    if ui
                        .button("Download anime titles")
                        .on_hover_text(QUEUED_ONLY)
                        .clicked()
                    {
                        self.queue_titles_dump();
                        ui.close();
                    }
                    if ui
                        .button("Fetch MyList statistics")
                        .on_hover_text(QUEUED_ONLY)
                        .clicked()
                    {
                        self.queue_command(animatic_core::sync::Command::MylistStats);
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            !self.import_pending
                                && matches!(self.full_update, refresh::FullUpdateStage::Closed),
                            egui::Button::new("Full update…"),
                        )
                        .on_hover_text(
                            "Queue a refresh of the title dump and of every anime and file \
                             in the library; nothing is sent until the queue runs.",
                        )
                        .clicked()
                    {
                        self.start_full_update_preview();
                        ui.close();
                    }
                    if ui.button("Request MyList export…").clicked() {
                        self.open_export_request();
                        ui.close();
                    }
                    ui.menu_button("Edit AniDB data", |ui| {
                        for (label, url) in [
                            ("Add anime", "https://anidb.net/anime/add"),
                            ("Add episode", "https://anidb.net/episode/add"),
                            ("Add file", "https://anidb.net/file/add"),
                            ("Submit change request", "https://anidb.net/creq"),
                        ] {
                            if ui
                                .button(label)
                                .on_hover_text("Opens the AniDB website")
                                .clicked()
                            {
                                if let Err(e) = open::that(url) {
                                    self.error(format!("Could not open AniDB: {e}"));
                                }
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    if ui
                        .add_enabled(
                            !self.import_pending,
                            egui::Button::new("Scan managed folders"),
                        )
                        .on_disabled_hover_text(IMPORTING)
                        .clicked()
                    {
                        self.worker.submit(Job::Scan {
                            settings: self.settings.file_managers.clone(),
                            extensions: self.settings.hashing.extensions.join(","),
                        });
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            library_blocker.is_none(),
                            egui::Button::new("Check library"),
                        )
                        .on_disabled_hover_text(library_blocker.unwrap_or_default())
                        .clicked()
                    {
                        ui.close();
                        self.worker.submit(Job::CheckLibrary);
                    }
                    if ui
                        .add_enabled(
                            library_blocker.is_none(),
                            egui::Button::new("Compact library…"),
                        )
                        .on_disabled_hover_text(library_blocker.unwrap_or_default())
                        .clicked()
                    {
                        ui.close();
                        self.compact_confirm = true;
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("AniDB website").clicked() {
                        let _ = open::that("https://anidb.net/");
                        ui.close();
                    }
                    ui.menu_button("Keyboard shortcuts", |ui| {
                        for (what, shortcut) in [
                            ("Add files", ctx.format_shortcut(&shortcuts::ADD_FILES)),
                            ("Search this page", ctx.format_shortcut(&shortcuts::SEARCH)),
                            ("Options", ctx.format_shortcut(&shortcuts::OPTIONS)),
                            ("Reload", ctx.format_shortcut(&shortcuts::RELOAD)),
                            (
                                "Run command queue",
                                ctx.format_shortcut(&shortcuts::RUN_QUEUE),
                            ),
                            (
                                "Open page 1–9 of the sidebar",
                                format!("{}1…9", ctx.format_modifiers(egui::Modifiers::COMMAND)),
                            ),
                            ("Close the topmost window", "Esc".to_string()),
                        ] {
                            ui.horizontal(|ui| {
                                ui.label(what);
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(RichText::new(shortcut).weak());
                                });
                            });
                        }
                    });
                    ui.separator();
                    if ui
                        .add_enabled(
                            Platform::current().is_some(),
                            egui::Button::new("Check for updates…"),
                        )
                        .on_disabled_hover_text(
                            "Updates are only available for 64-bit Windows and Linux builds",
                        )
                        .clicked()
                    {
                        self.update.open = true;
                        self.start_update_check(true);
                        ui.close();
                    }
                    if ui.button("About Animatic").clicked() {
                        self.about_open = true;
                        ui.close();
                    }
                });
                if let Some(notice) = self.update_notice() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button(notice).clicked() {
                            self.update.open = true;
                        }
                    });
                }
            });
        });
    }

    /// Sends the command queue to AniDB now (menu, Queue page and shortcut).
    fn run_command_queue(&mut self) {
        self.worker
            .submit(Job::ProcessQueue(self.settings.anidb.clone()));
    }

    /// The main window's keyboard shortcuts. None act while a modal dialog is open.
    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.top_modal_layer().is_some()) {
            return;
        }
        let pressed =
            |shortcut: &egui::KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(shortcut));
        if pressed(&shortcuts::ADD_FILES) && !self.import_pending {
            self.submit_files();
        }
        if pressed(&shortcuts::SEARCH) {
            self.focus_search = true;
        }
        if pressed(&shortcuts::OPTIONS) {
            self.settings_open = true;
        }
        if pressed(&shortcuts::RELOAD) {
            self.request_library_refresh();
        }
        if pressed(&shortcuts::RUN_QUEUE) && self.queue_run_allowed() {
            self.run_command_queue();
        }
        for (key, page) in shortcuts::PAGES.into_iter().zip(Page::all()) {
            if pressed(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, key)) {
                self.page = page;
            }
        }
    }

    fn navigation(&mut self, root: &mut egui::Ui) {
        let fill = style::chrome_fill(root.visuals());
        let pending = self.projection.queue.len();
        let blocked = self.projection.summary.queue_blocked;
        egui::Panel::left("navigation")
            .resizable(false)
            .exact_size(184.0)
            .frame(
                egui::Frame::side_top_panel(root.style())
                    .fill(fill)
                    .inner_margin(egui::Margin::symmetric(10, 12)),
            )
            .show(root, |ui| {
                ui.label(
                    RichText::new("Animatic")
                        .size(19.0)
                        .color(style::accent(ui.visuals())),
                );
                for (group, pages) in Page::GROUPS {
                    match group {
                        Some(name) => {
                            ui.add_space(14.0);
                            ui.label(RichText::new(name.to_uppercase()).small().weak());
                        }
                        None => ui.add_space(10.0),
                    }
                    for &page in pages {
                        let badge = match page {
                            Page::Queue if blocked > 0 => {
                                Some((format!("{blocked} blocked"), style::error(ui.visuals())))
                            }
                            Page::Queue if pending > 0 => {
                                Some((pending.to_string(), ui.visuals().weak_text_color()))
                            }
                            // Only the queue: pending work is the one state the other pages hide.
                            _ => None,
                        };
                        if nav_item(ui, self.page == page, page.label(), badge).clicked() {
                            self.page = page;
                        }
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    if nav_item(ui, self.settings_open, "⚙ Options", None)
                        .on_hover_text(shortcut_hint(ui.ctx(), &shortcuts::OPTIONS))
                        .clicked()
                    {
                        self.settings_open = !self.settings_open;
                    }
                    // One count per row so neither wraps in the narrow sidebar. The layout
                    // grows upwards: Options is the bottom row, the counts sit above it with the
                    // MyList line added first so it ends up under the anime line.
                    for text in [
                        style::plural(
                            self.projection.mylist.len(),
                            "MyList entry",
                            "MyList entries",
                        ),
                        style::plural(self.projection.anime.len(), "anime", "anime"),
                    ] {
                        ui.label(RichText::new(text).small().weak());
                    }
                });
            });
    }

    /// The page title (the sidebar label), an optional one-line explanation of what the page
    /// lists, and the search field shared by the searchable pages.
    fn page_header(&mut self, ui: &mut egui::Ui, subtitle: Option<&str>, searchable: bool) {
        ui.horizontal(|ui| {
            ui.heading(self.page.label());
            if let Some(subtitle) = subtitle {
                ui.label(RichText::new(subtitle).weak());
            }
            if searchable {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let search = ui.add(
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text("Search…")
                            .desired_width(240.0),
                    );
                    if std::mem::take(&mut self.focus_search) {
                        search.request_focus();
                    }
                    if !self.query.is_empty()
                        && ui
                            .small_button("✕")
                            .on_hover_text("Clear the search")
                            .clicked()
                    {
                        self.query.clear();
                    }
                });
            }
        });
        ui.separator();
    }

    fn dashboard(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, None, false);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.settings.first_launch {
                    self.getting_started(ui);
                }
                self.attention(ui);
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    let accent = style::accent(ui.visuals());
                    if ui
                        .add_enabled(
                            !self.import_pending,
                            egui::Button::new(RichText::new("Add files…").color(Color32::WHITE))
                                .fill(accent),
                        )
                        .on_hover_text(shortcut_hint(ui.ctx(), &shortcuts::ADD_FILES))
                        .clicked()
                    {
                        self.submit_files();
                    }
                    if ui
                        .add_enabled(
                            !self.import_pending,
                            egui::Button::new("Scan managed folders"),
                        )
                        .clicked()
                    {
                        self.worker.submit(Job::Scan {
                            settings: self.settings.file_managers.clone(),
                            extensions: self.settings.hashing.extensions.join(","),
                        });
                    }
                    if ui.button("Open hashed files").clicked() {
                        self.page = Page::Hashed;
                    }
                });
                let summary = self.projection.summary;
                style::section(ui, "Your library");
                ui.columns(4, |cols| {
                    metric(
                        &mut cols[0],
                        "Anime",
                        self.projection.anime.len().to_string(),
                    );
                    metric(
                        &mut cols[1],
                        "MyList entries",
                        self.projection.mylist.len().to_string(),
                    );
                    metric(&mut cols[2], "Watched", summary.mylist_watched.to_string());
                    metric(
                        &mut cols[3],
                        "Size on disk",
                        format::size_short(summary.local_bytes),
                    );
                });
                ui.label(
                    RichText::new(format!(
                        "{} of {} identified by AniDB",
                        summary.local_known,
                        style::plural(self.projection.local.len(), "hashed file", "hashed files"),
                    ))
                    .weak(),
                );
                self.account_figures(ui);
            });
    }

    /// What needs the user now, each with the action that resolves it; one calm line otherwise.
    fn attention(&mut self, ui: &mut egui::Ui) {
        let summary = self.projection.summary;
        let unidentified = self
            .projection
            .local
            .len()
            .saturating_sub(summary.local_known);
        let pending = self.projection.queue.len();
        let blocked = summary.queue_blocked;
        let hashing = self.projection.hash_queue.pending + self.projection.hash_queue.failed;
        let offline = self.settings.anidb.offline;
        style::section(ui, "Needs attention");
        let nothing = summary.local_crc_mismatch == 0
            && unidentified == 0
            && pending == 0
            && blocked == 0
            && hashing == 0
            && self.deferred.is_empty();
        if nothing {
            ui.colored_label(style::ok(ui.visuals()), "✓ Nothing needs your attention.");
            return;
        }
        self.hash_queue_panel(ui, true);
        let row = |ui: &mut egui::Ui, color: Option<Color32>, text: String, action: &str| {
            ui.horizontal(|ui| {
                match color {
                    Some(color) => ui.colored_label(color, text),
                    None => ui.label(text),
                };
                ui.button(action).clicked()
            })
            .inner
        };
        if summary.local_crc_mismatch > 0
            && row(
                ui,
                Some(style::error(ui.visuals())),
                format!(
                    "⚠ {} the CRC in {} name",
                    style::plural(
                        summary.local_crc_mismatch,
                        "file does not match",
                        "files do not match"
                    ),
                    if summary.local_crc_mismatch == 1 {
                        "its"
                    } else {
                        "their"
                    }
                ),
                "Open hashed files",
            )
        {
            self.page = Page::Hashed;
        }
        if unidentified > 0
            && row(
                ui,
                None,
                format!(
                    "{} not identified by AniDB yet",
                    style::plural(unidentified, "hashed file is", "hashed files are"),
                ),
                "Open hashed files",
            )
        {
            self.page = Page::Hashed;
        }
        if blocked > 0
            && row(
                ui,
                Some(style::error(ui.visuals())),
                format!(
                    "⚠ {} could not be sent",
                    style::plural(blocked, "command", "commands")
                ),
                "Open queue",
            )
        {
            self.page = Page::Queue;
        }
        let waiting = pending.saturating_sub(blocked);
        if waiting > 0 {
            let text = format!(
                "{} to be sent to AniDB{}",
                style::plural(waiting, "command waits", "commands wait"),
                if offline { " (working offline)" } else { "" }
            );
            let (action, allowed) = if offline {
                ("Go online", true)
            } else {
                ("Run command queue", self.queue_run_allowed())
            };
            let clicked = ui
                .horizontal(|ui| {
                    ui.label(text);
                    ui.add_enabled(allowed, egui::Button::new(action)).clicked()
                })
                .inner;
            if clicked {
                if offline {
                    self.settings.anidb.offline = false;
                    self.persist_settings_incidentally();
                    self.log("Working online");
                } else {
                    self.run_command_queue();
                }
            }
        }
        if !self.deferred.is_empty() {
            ui.colored_label(
                style::warn(ui.visuals()),
                format!(
                    "{} not saved yet; Animatic keeps retrying",
                    style::plural(self.deferred.len(), "command", "commands")
                ),
            );
        }
    }

    /// First-launch checklist: account, folders, and the optional AOM import.
    fn getting_started(&mut self, ui: &mut egui::Ui) {
        let account = !self.settings.anidb.username.trim().is_empty();
        let folders = self
            .settings
            .file_managers
            .managers
            .iter()
            .any(|m| !m.path.trim().is_empty());
        let visuals = ui.visuals().clone();
        egui::Frame::new()
            .fill(visuals.faint_bg_color)
            .stroke(egui::Stroke::new(1.0, style::accent(&visuals)))
            .corner_radius(8)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Getting started")
                            .text_style(style::section_style())
                            .color(visuals.strong_text_color()),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Hide").clicked() {
                            self.settings.first_launch = false;
                            self.persist_settings_incidentally();
                        }
                    });
                });
                let step = |ui: &mut egui::Ui, done: bool, text: &str, action: &str| {
                    ui.horizontal(|ui| {
                        if done {
                            ui.colored_label(style::ok(ui.visuals()), "✓");
                        } else {
                            ui.weak("○");
                        }
                        ui.label(text);
                        ui.button(action).clicked()
                    })
                    .inner
                };
                if step(
                    ui,
                    account,
                    "Enter your AniDB account, or keep working offline for now",
                    "Open account options",
                ) {
                    self.options_page = OptionsPage::Account;
                    self.settings_open = true;
                }
                if step(
                    ui,
                    folders,
                    "Choose the folders Animatic watches and renames into",
                    "Open folder options",
                ) {
                    self.options_page = OptionsPage::Library;
                    self.settings_open = true;
                }
                ui.add_space(6.0);
                ui.label(RichText::new("Coming from AniDB O'Matic (AOM)?").strong());
                ui.label(
                    RichText::new(
                        "Import its databank: anime, MyList entries, hashes and file paths are \
                         kept. Choose the AOM folder or its kowai folder; the picker does not \
                         list the .data files inside.",
                    )
                    .weak(),
                );
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.import_source)
                            .hint_text("AOM folder")
                            .desired_width(320.0),
                    );
                    ui.add_enabled_ui(!self.import_pending, |ui| {
                        if ui.button("Browse…").clicked()
                            && let Some(d) = pick_aom_databank_folder()
                        {
                            self.import_source = d.to_string_lossy().into_owned();
                        }
                        if ui.button("Import").clicked() {
                            self.import_legacy();
                        }
                    });
                });
            });
        ui.add_space(6.0);
    }

    /// Show the Anime page filtered to one genre. The shared search text is cleared so it does
    /// not hide anime of the genre, and an open detail window is closed so the list shows.
    fn browse_genre(&mut self, genre: i64) {
        self.genre_filter = Some(genre);
        self.query.clear();
        self.selected_aid = None;
        self.page = Page::Anime;
    }

    fn anime_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, Some("every anime stored locally"), true);
        if let Some(genre) = self.genre_filter {
            let name = self
                .projection
                .genres
                .iter()
                .find(|g| g.0 == genre)
                .map_or_else(|| format!("genre {genre}"), |g| g.1.clone());
            ui.horizontal(|ui| {
                ui.label("Genre:");
                ui.strong(name);
                if ui
                    .small_button("✕")
                    .on_hover_text("Show every anime again")
                    .clicked()
                {
                    self.genre_filter = None;
                }
            });
        }
        let sort = sort_key(&self.settings.ui.columns, "anime_table");
        let key = (
            self.loader.version,
            self.query.to_ascii_lowercase(),
            self.genre_filter,
            sort,
        );
        {
            let Self {
                projection,
                views,
                selected_aid,
                settings,
                ..
            } = self;
            views.anime.refresh(&key, || {
                let mut rows = filter_anime(projection, &key.1, key.2);
                sort_anime(projection, &mut rows, &key.3);
                rows
            });
            let rows = &views.anime.rows;
            let row_height = row_height(ui);
            let (colored, overrides) = (settings.ui.tint_mode(), &settings.ui.colors);
            data_table(
                ui,
                "anime_table",
                &[
                    ("Title", 360.0),
                    ("Type", 100.0),
                    ("Year", 80.0),
                    ("Episodes", 90.0),
                    ("Rating", 80.0),
                    ("MyList", 160.0),
                ],
                &mut settings.ui.columns,
            )
            .body(|body| {
                body.rows(row_height, rows.len(), |mut row| {
                    let a = &projection.anime[rows[row.index()]];
                    let state = projection.states.get(&a.aid);
                    row.tint(row_colors(colored, overrides, anime_slot(a, state)));
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
                                .map(|s| {
                                    format!(
                                        "{} eps / {} files{}",
                                        s.eps,
                                        s.files,
                                        if s.all_watched(a) {
                                            " · all watched"
                                        } else {
                                            ""
                                        }
                                    )
                                })
                                .unwrap_or_default(),
                        );
                    });
                });
            });
        }
        self.anime_detail_window(ui.ctx());
    }

    fn search_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, None, false);
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
            ui.checkbox(&mut self.search_mylist_only, "In MyList");
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
            sort_key(&self.settings.ui.columns, "advanced-results"),
        );
        {
            let Self {
                projection,
                views,
                selected_aid,
                settings,
                ..
            } = self;
            views.search.refresh(&key, || {
                let mut rows = filter_search(
                    projection,
                    &SearchFilter {
                        query: &key.1,
                        year: &key.2,
                        type_id: key.3,
                        mylist_only: key.4,
                        watched_only: key.5,
                    },
                );
                sort_anime(projection, &mut rows, &key.6);
                rows
            });
            let rows = &views.search.rows;
            let row_height = row_height(ui);
            let (colored, overrides) = (settings.ui.tint_mode(), &settings.ui.colors);
            data_table(
                ui,
                "advanced-results",
                &[
                    ("Title", 360.0),
                    ("Type", 100.0),
                    ("Year", 80.0),
                    ("Episodes", 90.0),
                    ("Rating", 80.0),
                    ("MyList", 160.0),
                ],
                &mut settings.ui.columns,
            )
            .body(|body| {
                body.rows(row_height, rows.len(), |mut row| {
                    let a = &projection.anime[rows[row.index()]];
                    let state = projection.states.get(&a.aid);
                    row.tint(row_colors(colored, overrides, anime_slot(a, state)));
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
                                .map(|s| {
                                    format!(
                                        "{} eps / {} watched{}",
                                        s.eps,
                                        s.watched_eps,
                                        if s.all_watched(a) {
                                            " · all watched"
                                        } else {
                                            ""
                                        }
                                    )
                                })
                                .unwrap_or_default(),
                        );
                    });
                });
            });
        }
        self.anime_detail_window(ui.ctx());
    }

    fn mylist_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, None, true);
        self.mylist_toolbar(ui);
        let key = (
            self.loader.version,
            self.query.to_ascii_lowercase(),
            sort_key(&self.settings.ui.columns, "mylist_table"),
        );
        // (lid, edit, resulting state, resulting watched flag)
        let mut edits: Vec<(i64, animatic_anidb::commands::MylistEdit, i64, bool)> = Vec::new();
        {
            let Self {
                projection,
                views,
                selected_aid,
                mylist_overlay,
                mylist_ui,
                settings,
                ..
            } = self;
            views.mylist.refresh(&key, || {
                let mut rows = filter_mylist(projection, &key.1);
                sort_mylist(projection, &mut rows, &key.2);
                rows
            });
            let rows = &views.mylist.rows;
            let row_height = row_height(ui);
            let (colored, overrides) = (settings.ui.tint_mode(), &settings.ui.colors);
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
                    ("Queue", 140.0),
                ],
                &mut settings.ui.columns,
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
                    row.tint(row_colors(colored, overrides, mylist_slot(shown_viewed)));
                    row.col(|ui| {
                        if ui.link(projection.mylist_title(i)).clicked() {
                            *selected_aid = Some(item.entry.aid);
                        }
                    });
                    row.col(|ui| {
                        let epno = item
                            .episode
                            .as_ref()
                            .map_or_else(|| "—".to_string(), |e| e.epno_str());
                        if ui
                            .selectable_label(mylist_ui.selected_lid == Some(lid), epno)
                            .clicked()
                        {
                            mylist_ui.selected_lid = Some(lid);
                        }
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
                    row.col(|ui| {
                        if let Some(p) = projection.mylist_pending.get(&lid) {
                            let response = ui.label(p.label());
                            if let Some(error) = &p.error {
                                response.on_hover_text(error);
                            }
                        }
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
        self.page_header(ui, None, true);
        let key = (
            self.loader.version,
            self.query.to_ascii_lowercase(),
            sort_key(&self.settings.ui.columns, "local_table"),
        );
        {
            let Self {
                projection,
                views,
                selected_local,
                mylist_ui,
                ..
            } = self;
            if views.local.refresh(&key, || {
                let mut rows = filter_local(projection, &key.1);
                sort_local(projection, &mut rows, &key.2);
                rows
            }) {
                if let Some(id) = *selected_local
                    && !views
                        .local
                        .rows
                        .iter()
                        .any(|&i| projection.local[i].id == id)
                {
                    // Never leave an invisible selection that Rename/Open/Play would act on.
                    *selected_local = None;
                }
                // Nor an invisible tick that a batch rename would act on.
                let listed: BTreeSet<i64> = views
                    .local
                    .rows
                    .iter()
                    .map(|&i| projection.local[i].id)
                    .collect();
                mylist_ui.marked_locals.retain(|id| listed.contains(id));
            }
        }
        const SELECT_FIRST: &str = "Select a file in the table first";
        ui.horizontal_wrapped(|ui| {
            let selected = self.selected_local.is_some();
            let marked = self.selected_locals().len();
            let group_gap = |ui: &mut egui::Ui| {
                ui.add(egui::Separator::default().vertical().spacing(10.0));
            };
            if ui
                .add_enabled(!self.import_pending, egui::Button::new("Add files…"))
                .on_hover_text(shortcut_hint(ui.ctx(), &shortcuts::ADD_FILES))
                .on_disabled_hover_text("An import is in progress")
                .clicked()
            {
                self.submit_files();
            }
            group_gap(ui);
            if ui
                .add_enabled(selected, egui::Button::new("Rename…"))
                .on_hover_text("Preview and rename the selected file")
                .on_disabled_hover_text(SELECT_FIRST)
                .clicked()
            {
                self.prepare_rename();
            }
            if ui
                .add_enabled(
                    marked > 0,
                    egui::Button::new(if marked > 0 {
                        format!("Rename {marked} selected…")
                    } else {
                        "Rename selected…".to_string()
                    }),
                )
                .on_hover_text("Preview and rename the selected files together")
                .on_disabled_hover_text("Ctrl+click or Shift+click selects several files")
                .clicked()
            {
                self.prepare_batch_rename();
            }
            if ui
                .add_enabled(
                    selected || !self.mylist_ui.marked_locals.is_empty(),
                    egui::Button::new("Add to MyList…"),
                )
                .on_hover_text("Ctrl+click or Shift+click selects several files")
                .on_disabled_hover_text(SELECT_FIRST)
                .clicked()
            {
                self.open_add_dialog();
            }
            group_gap(ui);
            if ui
                .add_enabled(selected, egui::Button::new("Play"))
                .on_disabled_hover_text(SELECT_FIRST)
                .clicked()
            {
                self.play_selected(animatic_core::external::PlayerAction::Play);
            }
            if ui
                .add_enabled(selected, egui::Button::new("Open file"))
                .on_hover_text("Open with the program your system uses for this file type")
                .on_disabled_hover_text(SELECT_FIRST)
                .clicked()
                && let Some(f) = self
                    .projection
                    .local
                    .iter()
                    .find(|f| Some(f.id) == self.selected_local)
            {
                let _ = open::that(&f.path);
            }
            self.player_toolbar(ui);
            if ui
                .add_enabled(
                    selected && self.settings.avdump.enabled,
                    egui::Button::new("AVDump"),
                )
                .on_disabled_hover_text(if self.settings.avdump.enabled {
                    SELECT_FIRST
                } else {
                    "Turn on AVDump in Options, Players and tools"
                })
                .clicked()
            {
                self.avdump_selected();
            }
            group_gap(ui);
            if ui
                .button("Select all")
                .on_hover_text("Select every listed file (Rename and Add to MyList act on all)")
                .clicked()
            {
                self.mylist_ui.marked_locals.extend(
                    self.views
                        .local
                        .rows
                        .iter()
                        .map(|&i| self.projection.local[i].id),
                );
            }
            if marked > 0 && ui.button("Clear selection").clicked() {
                self.mylist_ui.marked_locals.clear();
                self.selected_local = None;
            }
            let shown = self.views.local.rows.len();
            let total = self.projection.local.len();
            let count = if shown == total {
                style::plural(total, "file", "files")
            } else {
                format!("{shown} of {}", style::plural(total, "file", "files"))
            };
            ui.label(
                RichText::new(if marked > 0 {
                    format!("{count} · {marked} selected")
                } else {
                    count
                })
                .weak(),
            );
        });
        self.hash_queue_panel(ui, false);
        let file_menu = self.file_menu();
        let mut file_action = None;
        {
            let Self {
                projection,
                views,
                selected_local,
                mylist_ui,
                settings,
                ..
            } = self;
            let rows = &views.local.rows;
            let row_height = row_height(ui);
            let (colored, overrides) = (settings.ui.tint_mode(), &settings.ui.colors);
            data_table(
                ui,
                "local_table",
                &[
                    ("File", 300.0),
                    ("Size", 90.0),
                    ("AniDB", 90.0),
                    ("MyList", 140.0),
                    ("CRC", 90.0),
                    ("Path", 400.0),
                ],
                &mut settings.ui.columns,
            )
            .body(|body| {
                body.rows(row_height, rows.len(), |mut row| {
                    let f = &projection.local[rows[row.index()]];
                    row.tint(row_colors(colored, overrides, local_file_slot(f)));
                    row.col(|ui| {
                        let chosen = mylist_ui.marked_locals.contains(&f.id)
                            || *selected_local == Some(f.id);
                        let response = ui.selectable_label(chosen, f.file_name());
                        let (toggle, extend) =
                            ui.input(|i| (i.modifiers.command, i.modifiers.shift));
                        // A right click on a row outside the selection selects that row alone,
                        // so the menu never acts on a file the user cannot see highlighted.
                        let clicked = response.clicked();
                        let context_select = response.secondary_clicked() && !chosen;
                        if clicked || context_select {
                            mylist_editor::click_selection(
                                selected_local,
                                &mut mylist_ui.marked_locals,
                                f.id,
                                toggle && clicked,
                                extend && clicked,
                                || rows.iter().map(|&i| projection.local[i].id).collect(),
                            );
                        }
                        response.context_menu(|ui| {
                            if let Some(action) = player_actions::file_menu_ui(ui, &file_menu) {
                                file_action = Some(action);
                            }
                        });
                    });
                    row.col(|ui| {
                        ui.label(format::size_short(f.size));
                    });
                    row.col(|ui| {
                        ui.label(
                            f.fid
                                .map(|id| format!("fid {id}"))
                                .unwrap_or_else(|| "Not identified".into()),
                        );
                    });
                    row.col(|ui| {
                        ui.label(match (f.lid, f.mylist_add_pending) {
                            (Some(id), _) if f.viewed => format!("Watched · lid {id}"),
                            (Some(id), _) => format!("In MyList · lid {id}"),
                            (None, true) => "Add queued".to_string(),
                            (None, false) => "—".to_string(),
                        });
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
        if let Some(action) = file_action {
            self.run_file_action(action);
        }
    }

    fn groups_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, None, true);
        let key = (
            self.loader.version,
            self.query.to_ascii_lowercase(),
            sort_key(&self.settings.ui.columns, "groups"),
        );
        let Self {
            projection,
            views,
            settings,
            ..
        } = self;
        let mut refresh_groups: Vec<i64> = Vec::new();
        views.groups.refresh(&key, || {
            let mut rows = filter_groups(projection, &key.1);
            sort_groups(projection, &mut rows, &key.2);
            rows
        });
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
                ("Refresh", 90.0),
            ],
            &mut settings.ui.columns,
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
                row.col(|ui| {
                    if ui
                        .link("Refresh")
                        .on_hover_text("Queues a request that re-reads this group from AniDB")
                        .clicked()
                    {
                        refresh_groups.push(g.gid);
                    }
                });
            });
        });
        for gid in refresh_groups {
            self.queue_command(animatic_core::sync::Command::FetchGroup { gid });
        }
    }

    fn genres_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, None, true);
        let key = (
            self.loader.version,
            self.query.to_ascii_lowercase(),
            sort_key(&self.settings.ui.columns, "genres"),
        );
        let Self {
            projection,
            views,
            settings,
            ..
        } = self;
        views.genres.refresh(&key, || {
            let mut rows = filter_genres(projection, &key.1);
            sort_genres(projection, &mut rows, &key.2);
            rows
        });
        let rows = &views.genres.rows;
        let row_height = row_height(ui);
        let mut browse = None;
        data_table(
            ui,
            "genres",
            &[("Genre", 300.0), ("Anime", 100.0)],
            &mut settings.ui.columns,
        )
        .body(|body| {
            body.rows(row_height, rows.len(), |mut row| {
                let (id, name, count) = &projection.genres[rows[row.index()]];
                row.col(|ui| {
                    if ui
                        .link(name)
                        .on_hover_text("Show the anime of this genre")
                        .clicked()
                    {
                        browse = Some(*id);
                    }
                });
                row.col(|ui| {
                    ui.label(count.to_string());
                });
            });
        });
        if let Some(genre) = browse {
            self.browse_genre(genre);
        }
    }

    fn queue_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, Some("AniDB commands waiting to be sent"), false);
        let total = self.projection.queue.len();
        let blocked = self.projection.summary.queue_blocked;
        let blocked_pending = self.blocked_unconfirmed();
        let cooling = self
            .active_cooldown()
            .map(|(until, reason)| Self::cooldown_text(until, reason));
        let mut clear = false;
        let mut retry = false;
        let mut sync = false;
        ui.horizontal_wrapped(|ui| {
            let run = ui
                .add_enabled(
                    self.queue_run_allowed(),
                    egui::Button::new("Run command queue"),
                )
                .on_hover_text(shortcut_hint(ui.ctx(), &shortcuts::RUN_QUEUE))
                .on_disabled_hover_text(cooling.as_deref().unwrap_or("An import is in progress"));
            if run.clicked() {
                sync = true;
            }
            if ui
                .add_enabled(
                    blocked > 0 || blocked_pending,
                    egui::Button::new("Retry failed"),
                )
                .on_hover_text("Unblock failed commands and try them again")
                .on_disabled_hover_text("No command has failed")
                .clicked()
            {
                retry = true;
            }
            ui.label(style::plural(total, "pending command", "pending commands"));
            if blocked_pending {
                ui.colored_label(style::error(ui.visuals()), "blocked commands (refreshing)");
            } else if blocked > 0 {
                ui.colored_label(style::error(ui.visuals()), format!("{blocked} blocked"));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        total > 0,
                        egui::Button::new(style::destructive(ui, "Clear queue…")),
                    )
                    .on_hover_text("Discard every pending command")
                    .on_disabled_hover_text("The queue is empty")
                    .clicked()
                {
                    self.clear_queue_confirm = true;
                }
            });
        });
        if let Some(text) = &cooling {
            ui.colored_label(style::warn(ui.visuals()), text);
        }
        if self.settings.anidb.auto_run_queue {
            let removals = self
                .projection
                .queue
                .iter()
                .filter(|e| {
                    !e.blocked
                        && matches!(
                            e.command,
                            Some(animatic_core::sync::Command::MylistDelete { .. })
                        )
                })
                .count();
            ui.weak(
                "Automatic runs are on. They never remove MyList entries: removals stay queued \
                 until you click Run command queue.",
            );
            if removals > 0 {
                ui.colored_label(
                    style::warn(ui.visuals()),
                    format!(
                        "{} {} for you to click Run command queue.",
                        style::plural(removals, "MyList removal", "MyList removals"),
                        if removals == 1 { "waits" } else { "wait" }
                    ),
                );
            }
        }
        if self.clear_queue_confirm {
            let mut close = false;
            let modal = egui::Modal::new(egui::Id::unique("clear_queue")).show(ui.ctx(), |ui| {
                ui.set_max_width(420.0);
                style::section(ui, "Clear the command queue?");
                ui.label(format!(
                    "{} will be discarded and never sent to AniDB. MyList edits that are \
                     still waiting are lost. This cannot be undone.",
                    style::plural(total, "pending command", "pending commands")
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    if ui
                        .button(style::destructive(ui, "Discard commands"))
                        .clicked()
                    {
                        clear = true;
                        close = true;
                    }
                });
            });
            if close || modal.should_close() {
                self.clear_queue_confirm = false;
            }
        }
        if clear && total > 0 {
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
            self.run_command_queue();
        }
        if total == 0 {
            empty_state(
                ui,
                "Nothing is waiting for AniDB",
                if self.settings.anidb.offline {
                    "MyList edits and lookups wait here until they are sent. You are working \
                     offline, so they are kept."
                } else {
                    "MyList edits and lookups wait here until they are sent."
                },
            );
            return;
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
            &mut self.settings.ui.columns,
        )
        .fixed_order()
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
                        ui.colored_label(style::error(ui.visuals()), "Blocked");
                    } else if entry.tries > 0 {
                        ui.colored_label(
                            style::warn(ui.visuals()),
                            style::plural(entry.tries as usize, "retry", "retries"),
                        );
                    } else {
                        ui.weak("Pending");
                    }
                });
                row.col(|ui| {
                    if let Some(err) = &entry.last_error {
                        ui.label(RichText::new(err).color(style::error(ui.visuals())))
                            .on_hover_text(err);
                    }
                });
            });
        });
    }

    fn log_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, Some("this session's activity"), false);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.logs.is_empty(), egui::Button::new("Clear"))
                .on_hover_text("Empty the log; nothing else changes")
                .clicked()
            {
                self.logs.clear();
            }
        });
        if self.logs.is_empty() {
            empty_state(
                ui,
                "No activity yet",
                "Hashing, renaming, AniDB requests and errors are listed here as they happen.",
            );
            return;
        }
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show_rows(ui, row_height, self.logs.len(), |ui, range| {
                for line in &self.logs[range] {
                    if line.contains("  ERROR  ") {
                        ui.label(
                            RichText::new(line)
                                .monospace()
                                .color(style::error(ui.visuals())),
                        );
                    } else {
                        ui.monospace(line);
                    }
                }
            });
    }

    fn anime_detail_window(&mut self, ctx: &egui::Context) {
        let Some(aid) = self.selected_aid else {
            self.detail = None;
            self.covers.window_closed();
            return;
        };
        // One detail read per (anime, projection version), not one per frame.
        let version = self.loader.version;
        let detail = match self.detail.take() {
            Some(cached) if cached.aid == aid && cached.version == version => cached.detail,
            _ => self.db.anime_detail(aid).ok().flatten(),
        };
        let mut refresh_requested = false;
        let mut genre_clicked = None;
        let mut episode_refresh: Vec<i64> = Vec::new();
        let mut open = true;
        let title = match &detail {
            Some(d) if !d.anime.name.is_empty() => d.anime.name.clone(),
            _ => format!("Anime {aid}"),
        };
        let shown = dialog::window(ctx, title)
            // The title changes with the anime shown; the window keeps its place.
            .id(egui::Id::unique("anime_detail"))
            .open(&mut open)
            .default_width(560.0)
            .max_height(ctx.content_rect().height() * 0.85)
            .vscroll(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Open on AniDB").clicked() {
                        let _ = open::that(format!("https://anidb.net/anime/{aid}"));
                    }
                    if ui
                        .button("Refresh from AniDB")
                        .on_hover_text("Queues a request that re-reads this anime from AniDB")
                        .clicked()
                    {
                        refresh_requested = true;
                    }
                });
                ui.add_space(4.0);
                let Some(d) = &detail else {
                    ui.label("No detail record is available.");
                    return;
                };
                ui.horizontal_top(|ui| {
                    self.covers.show(
                        ui,
                        &self.worker,
                        &self.settings.anidb,
                        aid,
                        d.anime.picname.as_deref(),
                    );
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(d.anime.type_name());
                            ui.separator();
                            ui.label(&d.anime.year);
                            ui.separator();
                            ui.label(style::plural(
                                d.anime.episodes.max(0) as usize,
                                "episode",
                                "episodes",
                            ));
                            ui.separator();
                            ui.label(RichText::new(format!("AniDB {aid}")).weak());
                        });
                        if let Some(text) = &d.anime.description {
                            ui.add_space(6.0);
                            ui.label(anidb_markup::layout(ui, text));
                        }
                    });
                });
                ui.add_space(8.0);
                // Sections without entries stay listed (with their zero) but cannot open.
                let section = |ui: &mut egui::Ui,
                               name: &str,
                               count: usize,
                               open: bool,
                               body: &mut dyn FnMut(&mut egui::Ui)| {
                    let header = egui::CollapsingHeader::new(format!("{name} ({count})"))
                        .id_salt(("anime_detail_section", name))
                        .default_open(open);
                    ui.add_enabled_ui(count > 0, |ui| {
                        header.show(ui, |ui| body(ui));
                    });
                };
                section(ui, "Episodes", d.episodes.len(), true, &mut |ui| {
                    egui::Grid::new("anime_detail_episodes")
                        .num_columns(3)
                        .striped(true)
                        .show(ui, |ui| {
                            for e in &d.episodes {
                                ui.monospace(e.epno_str());
                                ui.label(&e.name);
                                if ui
                                    .small_button("Refresh")
                                    .on_hover_text(
                                        "Queues a request that re-reads this episode from AniDB",
                                    )
                                    .clicked()
                                {
                                    episode_refresh.push(e.eid);
                                }
                                ui.end_row();
                            }
                        });
                });
                section(ui, "Titles", d.titles.len(), false, &mut |ui| {
                    for t in &d.titles {
                        ui.horizontal(|ui| {
                            ui.monospace(&t.lang);
                            ui.label(&t.name);
                        });
                    }
                });
                section(ui, "Genres", d.genres.len(), false, &mut |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (id, name) in &d.genres {
                            if ui
                                .link(name)
                                .on_hover_text("Show the anime of this genre")
                                .clicked()
                            {
                                genre_clicked = Some(*id);
                            }
                        }
                    });
                });
                let mut related = None;
                section(ui, "Relations", d.relations.len(), false, &mut |ui| {
                    for (related_aid, relation, name) in &d.relations {
                        if ui
                            .link(format!(
                                "{} — {}",
                                animatic_core::library::relation_name(*relation),
                                name
                            ))
                            .clicked()
                        {
                            related = Some(*related_aid);
                        }
                    }
                });
                if related.is_some() {
                    self.selected_aid = related;
                }
                section(ui, "Reviews", d.reviews.len(), false, &mut |ui| {
                    for review in &d.reviews {
                        ui.horizontal(|ui| {
                            ui.strong(if review.uname.is_empty() {
                                "Anonymous"
                            } else {
                                &review.uname
                            });
                            ui.label(format::rating(review.rating));
                        });
                        ui.label(anidb_markup::layout(ui, &review.body));
                        ui.separator();
                    }
                });
            });
        if dialog::escape_closes(ctx, shown.as_ref()) {
            open = false;
        }
        if open {
            self.detail = Some(DetailCache {
                aid,
                version,
                detail,
            });
        } else {
            self.selected_aid = None;
            self.covers.window_closed();
        }
        if refresh_requested {
            self.queue_anime_refresh(aid);
        }
        for eid in episode_refresh {
            self.queue_command(animatic_core::sync::Command::FetchEpisode { eid });
        }
        if let Some(genre) = genre_clicked {
            self.browse_genre(genre);
            self.detail = None;
            self.covers.window_closed();
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let mut open = true;
        let exe = self.executable_path().map(Path::to_path_buf);
        self.shell.ensure_probed(exe.as_deref());
        let mut save = false;
        let mut close = false;
        // Never larger than the main window, so the footer with Save stays on screen.
        let room = ctx.content_rect().size() - egui::vec2(48.0, 72.0);
        let shown = dialog::window(ctx, "Options")
            .open(&mut open)
            .resizable(true)
            .default_size([820.0_f32.min(room.x), 560.0_f32.min(room.y)])
            .min_size([520.0_f32.min(room.x), 300.0_f32.min(room.y)])
            .max_size(room)
            .show(ctx, |ui| {
                let body = (ui.available_height() - 52.0).max(160.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(170.0);
                        for page in OptionsPage::ALL {
                            if nav_item(ui, self.options_page == page, page.label(), None).clicked()
                            {
                                self.options_page = page;
                            }
                        }
                    });
                    ui.separator();
                    // A scroll area takes the layout of its parent: without this vertical
                    // wrapper the page would be laid out left to right.
                    ui.vertical(|ui| {
                        egui::ScrollArea::vertical()
                            .id_salt(("options-page", self.options_page))
                            .max_height(body)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(self.options_page.label())
                                        .text_style(style::section_style())
                                        .color(ui.visuals().strong_text_color()),
                                );
                                ui.add_space(4.0);
                                match self.options_page {
                                    OptionsPage::Account => self.options_account(ui),
                                    OptionsPage::Appearance => self.options_appearance(ui),
                                    OptionsPage::Library => self.options_folders(ui),
                                    OptionsPage::Hashing => {
                                        self.options_hashing(ui, exe.as_deref())
                                    }
                                    OptionsPage::Renaming => self.pattern_editor_ui(ui),
                                    OptionsPage::Tools => self.options_tools(ui),
                                    OptionsPage::Exports => self.options_exports(ui),
                                    OptionsPage::Updates => self.options_updates(ui),
                                }
                            });
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(
                            "Changes are used right away. Save options writes them to the \
                             settings file.",
                        )
                        .small()
                        .weak(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let accent = style::accent(ui.visuals());
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Save options").color(Color32::WHITE),
                                )
                                .fill(accent),
                            )
                            .clicked()
                        {
                            save = true;
                        }
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                });
            });
        if save {
            self.save_settings();
            self.apply_visuals(ctx);
            self.media_settings_saved();
            self.settings_open = false;
            self.apply_shell_integration(exe.as_deref());
        }
        if !open || close || dialog::escape_closes(ctx, shown.as_ref()) {
            self.settings_open = false;
        }
        if !self.settings_open {
            self.shell.options_closed();
        }
    }

    fn options_account(&mut self, ui: &mut egui::Ui) {
        field(ui, "Username", &mut self.settings.anidb.username);
        ui.horizontal(|ui| {
            field_label(ui, "Password");
            ui.add(
                egui::TextEdit::singleline(&mut self.settings.anidb.password)
                    .password(true)
                    .desired_width(FIELD_WIDTH),
            );
        });
        ui.horizontal(|ui| {
            let label = field_label(ui, "API key (optional)").on_hover_text(
                "The UDP API key from your AniDB profile. When set, the AniDB session is \
                 encrypted; leave it empty for an unencrypted session.",
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.settings.anidb.api_key)
                    .desired_width(FIELD_WIDTH),
            )
            .labelled_by(label.id);
        });
        if let Some(notice) = self.credential_storage.notice() {
            ui.colored_label(style::warn(ui.visuals()), notice);
        }
        ui.label(
            RichText::new(format!(
                "AniDB client: {UDP_CLIENT} v{UDP_CLIENT_VERSION} (UDP), \
                             {HTTP_CLIENT} v{HTTP_CLIENT_VERSION} (HTTP)"
            ))
            .weak(),
        )
        .on_hover_text(
            "Animatic's own registered AniDB clients. They are built in and fixed: \
                         you only need your AniDB username and password.",
        );
        style::section(ui, "Synchronization");
        ui.checkbox(
            &mut self.settings.anidb.offline,
            "Work offline (keep commands queued)",
        );
        ui.checkbox(
            &mut self.settings.anidb.auto_run_queue,
            "Run the command queue automatically",
        )
        .on_hover_text(
            "Sends the queued commands 30 seconds after new ones arrive and every 15 \
                         minutes, and keeps one AniDB login between those runs. Never while Work \
                         offline is on or AniDB has paused syncing, and MyList removals are never \
                         sent automatically.",
        );
        ui.add_enabled_ui(self.settings.anidb.auto_run_queue, |ui| {
            ui.horizontal(|ui| {
                ui.label("Log out of AniDB when idle for");
                ui.add(
                    egui::DragValue::new(&mut self.settings.anidb.idle_logout_minutes)
                        .range(
                            animatic_core::settings::MIN_IDLE_LOGOUT_MINUTES
                                ..=animatic_core::settings::MAX_IDLE_LOGOUT_MINUTES,
                        )
                        .suffix(" min"),
                );
            });
        });
        style::section(ui, "Downloads");
        ui.checkbox(
            &mut self.settings.anidb.update_file_information,
            "Update known file information",
        )
        .on_hover_text(
            "When an already identified file is hashed again, queue a refresh of its \
                         AniDB record (at most once every 7 days per file).",
        );
        ui.checkbox(
            &mut self.settings.anidb.download_covers,
            "Download anime covers when a detail window opens",
        )
        .on_hover_text(
            "Each cover is downloaded once from AniDB's image server and kept on \
                         disk for your own display; covers already on disk are always shown",
        );
    }

    fn options_appearance(&mut self, ui: &mut egui::Ui) {
        let theme_name = |theme: Theme| match theme {
            Theme::Dark => "Dark",
            Theme::Light => "Light",
            Theme::System => "Same as the system",
        };
        let mut restyle = false;
        egui::Grid::new("options-appearance")
            .num_columns(2)
            .spacing([14.0, 8.0])
            .show(ui, |ui| {
                ui.label("Theme");
                egui::ComboBox::from_id_salt("options-theme")
                    .selected_text(theme_name(self.settings.ui.theme))
                    .show_ui(ui, |ui| {
                        for theme in [Theme::Dark, Theme::Light, Theme::System] {
                            restyle |= ui
                                .selectable_value(
                                    &mut self.settings.ui.theme,
                                    theme,
                                    theme_name(theme),
                                )
                                .changed();
                        }
                    });
                ui.end_row();
                ui.label("UI scale");
                let scale = ui.add(
                    egui::Slider::new(&mut self.settings.ui.zoom, 0.75..=2.0)
                        .step_by(0.05)
                        .suffix("×"),
                );
                // Rescaling while dragging would move the slider under the pointer.
                restyle |= scale.drag_stopped() || (scale.changed() && !scale.dragged());
                ui.end_row();
                ui.label("Status colors");
                ui.checkbox(
                    &mut self.settings.ui.colored_rows,
                    "Tint table rows by status",
                )
                .on_hover_text(
                    "Tint table rows by status: CRC mismatch, unidentified, watched and in \
                         MyList. Each status is also written in the row, so the color is never \
                         the only cue.",
                );
                ui.end_row();
                ui.label("Row color style");
                ui.add_enabled_ui(self.settings.ui.colored_rows, |ui| {
                    egui::ComboBox::from_id_salt("options-row-style")
                        .selected_text(self.settings.ui.row_style.label())
                        .show_ui(ui, |ui| {
                            for row_style in RowStyle::ALL {
                                ui.selectable_value(
                                    &mut self.settings.ui.row_style,
                                    row_style,
                                    row_style.label(),
                                );
                            }
                        })
                        .response
                        .on_hover_text(
                            "Soft: a light tint and a colored bar at the start of the row. \
                             Solid: the whole row in the status color, as in AniDB O'Matic.",
                        );
                });
                ui.end_row();
            });
        if restyle {
            self.apply_visuals(ui.ctx());
        }
        self.tray_options_ui(ui);
    }

    fn options_hashing(&mut self, ui: &mut egui::Ui, exe: Option<&Path>) {
        ui.checkbox(
            &mut self.settings.hashing.autostart,
            "Start hashing automatically",
        )
        .on_hover_text("Resume the files still waiting in the hash queue when Animatic starts");
        ui.checkbox(
            &mut self.settings.hashing.identify_after_hashing,
            "Identify files with AniDB after hashing",
        );
        self.shell
            .options_ui(ui, &mut self.settings.hashing.integrate_into_shell, exe);
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
    }

    fn options_folders(&mut self, ui: &mut egui::Ui) {
        let mut remove_manager = None;
        let mut enable_auto_rename = None;
        for (index, manager) in self.settings.file_managers.managers.iter_mut().enumerate() {
            let label = if manager.name.is_empty() {
                "Unnamed folder".to_string()
            } else {
                manager.name.clone()
            };
            // Keyed by position, not by the name: typing in Name must not close the section.
            egui::CollapsingHeader::new(label)
                .id_salt(("options-managed-folder", index))
                .show(ui, |ui| {
                            field(ui, "Name", &mut manager.name);
                            field(ui, "Path", &mut manager.path);
                            field(ui, "Rename destination", &mut manager.base_path);
                            ui.horizontal_wrapped(|ui| {
                                ui.checkbox(&mut manager.include_subdirs, "Subfolders");
                                ui.checkbox(&mut manager.auto_hash, "Auto hash");
                                ui.checkbox(&mut manager.auto_rehash, "Rehash changed");
                                ui.checkbox(&mut manager.auto_remove, "Remove missing");
                                if auto_rename::mode_control(ui, index, manager) {
                                    enable_auto_rename = Some(index);
                                }
                                ui.checkbox(&mut manager.exclude_system, "Skip system files")
                                    .on_hover_text(
                                        "Scans skip files and folders with the Windows system attribute. \
                                         Linux has no such attribute, so this has no effect there.",
                                    );
                                ui.checkbox(&mut manager.clear_empty_folders, "Remove emptied folders")
                                    .on_hover_text(
                                        "After Animatic renames or moves a file out of a subfolder, remove \
                                         the folder (and emptied parents) if nothing at all is left in it. \
                                         The managed folder itself is never removed.",
                                    );
                                ui.checkbox(&mut manager.mylist_auto_add, "Add to MyList")
                                    .on_hover_text(
                                        "Add files identified in this folder to your AniDB MyList",
                                    );
                            });
                            ui.label(
                                RichText::new("For files this folder adds to MyList")
                                    .small()
                                    .weak(),
                            );
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
        if let Some(index) = enable_auto_rename {
            self.begin_auto_rename_enable(index);
        }
        if ui.button("Add folder rule").clicked() {
            self.settings
                .file_managers
                .managers
                .push(animatic_core::settings::FileManager {
                    name: "New folder".into(),
                    ..Default::default()
                });
        }
        style::section(ui, "Folder scanning");
        ui.label(
            RichText::new(
                "Scans read local folders only and never contact AniDB. Schedule changes apply \
                 when Options is closed.",
            )
            .weak(),
        );
        {
            use animatic_core::settings::{
                MAX_ACTION_DELAY_MS, MAX_SCAN_INTERVAL_MINUTES, MAX_SLEEP_TIME_MS,
                MIN_SCAN_INTERVAL_MINUTES,
            };
            let scanning = &mut self.settings.file_managers;
            ui.checkbox(
                &mut scanning.scan_at_start,
                "Scan managed folders when Animatic starts",
            );
            ui.checkbox(
                &mut scanning.scheduled_scan,
                "Scan managed folders on a schedule",
            )
            .on_hover_text(
                "A scheduled scan waits while another task, an import, a settings review or a \
                 restore is running.",
            );
            ui.add_enabled_ui(scanning.scheduled_scan, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Scan every");
                    ui.add(
                        egui::DragValue::new(&mut scanning.scan_interval_minutes)
                            .range(MIN_SCAN_INTERVAL_MINUTES..=MAX_SCAN_INTERVAL_MINUTES)
                            .suffix(" minutes"),
                    );
                });
            });
            ui.horizontal(|ui| {
                ui.label("Hash a file once it is unchanged for")
                    .on_hover_text("A file still being written is left for the next scan.");
                ui.add(
                    egui::DragValue::new(&mut scanning.action_delay_ms)
                        .range(0..=MAX_ACTION_DELAY_MS)
                        .suffix(" ms"),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Pause between hashed files");
                ui.add(
                    egui::DragValue::new(&mut scanning.sleep_time_ms)
                        .range(0..=MAX_SLEEP_TIME_MS)
                        .suffix(" ms"),
                );
            });
        }
    }

    fn options_tools(&mut self, ui: &mut egui::Ui) {
        style::section(ui, "External players");
        let mut remove_player = None;
        let mut default_change = None;
        let default_index = animatic_core::external::default_player(&self.settings.players);
        for (index, player) in self.settings.players.iter_mut().enumerate() {
            if index > 0 {
                ui.separator();
            }
            field_hint(
                ui,
                "Name",
                &mut player.name,
                "Shown in the player chooser on Hashed files when several players are configured.",
            );
            field(ui, "Executable", &mut player.path);
            field_hint(
                ui,
                "Open arguments",
                &mut player.open,
                "Arguments for Open in player. Leave empty to hide that action for this player.",
            );
            field_hint(
                ui,
                "Play arguments",
                &mut player.play,
                "Arguments for Play. Without {path} or %file% the file is appended.",
            );
            field_hint(
                ui,
                "Enqueue arguments",
                &mut player.enqueue,
                "Arguments for Enqueue. Leave empty to hide that action for this player.",
            );
            let mut is_default = default_index == Some(index);
            if ui.checkbox(&mut is_default, "Default player").changed() {
                default_change = Some((index, is_default));
            }
            if ui.button("Remove player").clicked() {
                remove_player = Some(index);
            }
        }
        if let Some((index, on)) = default_change {
            animatic_core::external::set_default_player(&mut self.settings.players, index, on);
        }
        if let Some(index) = remove_player {
            self.settings.players.remove(index);
            self.selected_player = None;
        }
        if ui.button("Add player").clicked() {
            self.settings
                .players
                .push(animatic_core::settings::Player::default());
        }
        style::section(ui, "AVDump");
        ui.checkbox(
            &mut self.settings.avdump.enabled,
            "Enable AVDump integration",
        );
        field(ui, "Executable", &mut self.settings.avdump.executable);
        field(ui, "Arguments", &mut self.settings.avdump.arguments);
        field(ui, "AVDump API key", &mut self.settings.avdump.key);
        style::section(ui, "Media properties");
        self.media_options_ui(ui);
    }

    fn options_exports(&mut self, ui: &mut egui::Ui) {
        style::section(ui, "Hash export");
        ui.label(RichText::new("The text written for each file by File → Export → Hashes…").weak());
        ui.add(
            egui::TextEdit::multiline(&mut self.settings.hashing.export_template)
                .desired_rows(2)
                .hint_text("%filename% %size% %fulled2k%"),
        );
        ui.checkbox(
            &mut self.settings.ui.copy_length_as_minutes,
            "Export %length% in minutes",
        );
        style::section(ui, "Statistics export");
        ui.checkbox(
            &mut self.settings.stat_whore.enabled,
            "Write statistics after library updates",
        );
        ui.horizontal(|ui| {
            field_label(ui, "Format");
            egui::ComboBox::from_id_salt("options-stat-format")
                .selected_text(match self.settings.stat_whore.format {
                    StatFormat::Xml => "XML",
                    StatFormat::Ini => "INI",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.settings.stat_whore.format,
                        StatFormat::Xml,
                        "XML",
                    );
                    ui.selectable_value(
                        &mut self.settings.stat_whore.format,
                        StatFormat::Ini,
                        "INI",
                    );
                });
        });
        field(ui, "Output file", &mut self.settings.stat_whore.file);
    }

    fn options_updates(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(
            &mut self.settings.updates.auto_check,
            "Check for updates at startup",
        );
        let repository = env!("CARGO_PKG_REPOSITORY");
        ui.label(
            RichText::new(
                "Looks for a newer stable Animatic release on Animatic's GitHub releases page. \
                 Only GitHub (github.com and its download servers) is contacted, anonymously; \
                 never AniDB. Skipped in offline mode. Nothing is downloaded or installed \
                 without your confirmation.",
            )
            .weak(),
        );
        let releases = format!("{repository}/releases");
        ui.hyperlink_to(
            releases.trim_start_matches("https://").to_string(),
            &releases,
        );
        style::section(ui, "MyList states");
        ui.label(RichText::new("The states a MyList entry can have on AniDB:").weak());
        ui.label(MYLIST_STATES.join(" · "));
    }

    fn update_window(&mut self, ctx: &egui::Context) {
        if !self.update.open {
            return;
        }
        let blocker = self.update_blocker();
        let restart_blocker = self.restart_blocker();
        let stage = self.update.stage.clone();
        let mut action = None;
        let mut open = true;
        let shown = dialog::window(ctx, "Software updates")
            .open(&mut open)
            .resizable(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.label(format!("Installed version: {}", env!("CARGO_PKG_VERSION")));
                if Platform::current().is_none() {
                    ui.label("Updates are only available for 64-bit Windows and Linux builds.");
                    return;
                }
                ui.add_space(4.0);
                if let Some(error) = &self.update.error {
                    ui.colored_label(style::error(ui.visuals()), error);
                    ui.add_space(4.0);
                }
                let version = self.update.release.as_ref().map(|r| r.version.as_str());
                match stage {
                    UpdateStage::Idle => {
                        if ui.button("Check now").clicked() {
                            action = Some(UpdateAction::Check);
                        }
                    }
                    UpdateStage::Checking { cancelling, .. } => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Checking GitHub for a newer release…");
                        });
                        if ui
                            .add_enabled(!cancelling, egui::Button::new("Cancel"))
                            .clicked()
                        {
                            action = Some(UpdateAction::Cancel);
                        }
                    }
                    UpdateStage::UpToDate => {
                        ui.label("Animatic is up to date.");
                        if ui.button("Check again").clicked() {
                            action = Some(UpdateAction::Check);
                        }
                    }
                    UpdateStage::Available => {
                        if let Some(release) = &self.update.release {
                            ui.label(format!(
                                "Version {} is available ({} download).",
                                release.version,
                                format::size_short(
                                    i64::try_from(release.archive_size).unwrap_or(i64::MAX)
                                )
                            ));
                            ui.hyperlink_to("Release notes on GitHub", &release.page_url);
                        }
                        ui.add_space(4.0);
                        ui.colored_label(style::warn(ui.visuals()), UNSIGNED_WARNING);
                        ui.add_space(4.0);
                        if ui.button("Download and verify").clicked() {
                            action = Some(UpdateAction::Download);
                        }
                    }
                    UpdateStage::Downloading {
                        done,
                        total,
                        cancelling,
                    } => {
                        ui.label(format!("Downloading version {}…", version.unwrap_or("?")));
                        ui.add(
                            egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                                .show_percentage(),
                        );
                        if ui
                            .add_enabled(!cancelling, egui::Button::new("Cancel"))
                            .clicked()
                        {
                            action = Some(UpdateAction::Cancel);
                        }
                    }
                    UpdateStage::Ready { confirming } => {
                        if let Some(update) = &self.update.ready {
                            ui.label(format!(
                                "Version {} was downloaded and matches the published checksum.",
                                update.version()
                            ));
                            ui.horizontal(|ui| {
                                ui.label("SHA-256:");
                                ui.label(RichText::new(update.sha256()).monospace());
                            });
                        }
                        ui.add_space(4.0);
                        ui.colored_label(style::warn(ui.visuals()), UNSIGNED_WARNING);
                        ui.add_space(4.0);
                        if confirming {
                            ui.label(match self.executable_path() {
                                Some(path) => format!(
                                    "Animatic will replace {} with the new version. Your \
                                     database, settings, profiles and media are not touched; \
                                     the new license files are added next to the program.",
                                    path.display()
                                ),
                                None => "Animatic cannot tell where it is installed.".into(),
                            });
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(
                                        blocker.is_none(),
                                        egui::Button::new("Confirm installation"),
                                    )
                                    .on_disabled_hover_text(blocker.unwrap_or_default())
                                    .clicked()
                                {
                                    action = Some(UpdateAction::Install);
                                }
                                if ui.button("Back").clicked() {
                                    action = Some(UpdateAction::Back);
                                }
                            });
                        } else {
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(blocker.is_none(), egui::Button::new("Install"))
                                    .on_disabled_hover_text(blocker.unwrap_or_default())
                                    .clicked()
                                {
                                    action = Some(UpdateAction::Confirm);
                                }
                                if ui.button("Discard download").clicked() {
                                    action = Some(UpdateAction::Discard);
                                }
                            });
                        }
                        if let Some(reason) = blocker {
                            ui.label(RichText::new(reason).weak());
                        }
                    }
                    UpdateStage::InstallFailed => {
                        ui.label(
                            "The installation did not finish. The program files may be in an \
                             unknown state, so Animatic will not update or restart itself in \
                             this session.",
                        );
                        ui.label(match self.executable_path() {
                            Some(path) => format!(
                                "Check that {} still starts. If it does not, download the \
                                 release from GitHub and replace the program files manually.",
                                path.display()
                            ),
                            None => "Download the release from GitHub and replace the program \
                                     files manually."
                                .into(),
                        });
                        if let Some(release) = &self.update.release {
                            ui.hyperlink_to("Release page on GitHub", &release.page_url);
                        }
                    }
                    UpdateStage::Installing => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Installing… do not close Animatic.");
                        });
                    }
                    UpdateStage::Installed => {
                        ui.label(format!(
                            "Version {} is installed. Restart Animatic to start using it.",
                            self.update.installed_version.as_deref().unwrap_or("?")
                        ));
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    restart_blocker.is_none(),
                                    egui::Button::new("Restart now"),
                                )
                                .on_disabled_hover_text(restart_blocker.unwrap_or_default())
                                .clicked()
                            {
                                action = Some(UpdateAction::Restart);
                            }
                            if ui.button("Later").clicked() {
                                action = Some(UpdateAction::Close);
                            }
                        });
                        if let Some(reason) = restart_blocker {
                            ui.label(RichText::new(reason).weak());
                        }
                    }
                }
                ui.add_space(6.0);
                ui.label(RichText::new(UPDATE_PRIVACY).weak());
            });
        match action {
            Some(UpdateAction::Check) => self.start_update_check(true),
            Some(UpdateAction::Download) => self.start_update_download(),
            Some(UpdateAction::Cancel) => {
                if self.update.stage.cancel() {
                    self.updater.cancel();
                }
            }
            Some(UpdateAction::Confirm) => self.update.stage.set_confirming(true),
            Some(UpdateAction::Back) => self.update.stage.set_confirming(false),
            Some(UpdateAction::Install) => self.install_update(),
            Some(UpdateAction::Discard) => {
                if self.update.stage.discard_ready() {
                    self.update.ready = None;
                }
            }
            Some(UpdateAction::Restart) => self.restart_now(ctx),
            Some(UpdateAction::Close) => open = false,
            None => {}
        }
        if !open || dialog::escape_closes(ctx, shown.as_ref()) {
            self.update.open = false;
        }
    }

    pub fn apply_visuals(&self, ctx: &egui::Context) {
        // `System` follows the operating system's light or dark mode, also while running.
        ctx.set_theme(match self.settings.ui.theme {
            Theme::Dark => egui::ThemePreference::Dark,
            Theme::Light => egui::ThemePreference::Light,
            Theme::System => egui::ThemePreference::System,
        });
        ctx.set_zoom_factor(self.settings.ui.zoom.clamp(0.75, 2.0));
    }

    /// The window to open: restored size, position and maximized state from `ui.window` and
    /// `ui.maximized` (validated, see `ui_state::placement_from_saved`).
    pub fn viewport(&self) -> egui::ViewportBuilder {
        let placement = ui_state::placement_from_saved(self.settings.ui.window);
        let mut viewport = egui::ViewportBuilder::default()
            .with_title("Animatic — AniDB library manager")
            .with_icon(std::sync::Arc::new(crate::icon::window_icon()))
            .with_inner_size(placement.size)
            .with_min_inner_size(ui_state::MIN_WINDOW_SIZE)
            .with_maximized(self.settings.ui.maximized);
        if let Some(position) = placement.position {
            viewport = viewport.with_position(position);
        }
        viewport
    }

    /// Keeps `ui.window` and `ui.maximized` current in memory. They reach the file with the
    /// normal settings save (Options, exit), which the recovery guard already controls.
    ///
    /// The window's normal size and position are only recorded while it is neither maximized,
    /// minimized nor fullscreen, so restoring a maximized window keeps its normal geometry.
    /// Values are stored at UI scale 1, so they stay valid when the UI scale changes.
    fn track_window(&mut self, ctx: &egui::Context) {
        let zoom = ctx.zoom_factor();
        if (zoom - self.window_zoom).abs() > f32::EPSILON {
            // Measured with the old scale for this frame; wait for the next one.
            self.window_zoom = zoom;
            return;
        }
        let (outer, inner, content, minimized, maximized, fullscreen) = ctx.input(|i| {
            let viewport = i.viewport();
            (
                viewport.outer_rect,
                viewport.inner_rect,
                i.content_rect(),
                viewport.minimized,
                viewport.maximized,
                viewport.fullscreen,
            )
        });
        if minimized == Some(true) || fullscreen == Some(true) {
            return;
        }
        let maximized = maximized.unwrap_or(false);
        self.settings.ui.maximized = maximized;
        if maximized {
            return;
        }
        let size = inner.map_or(content.size(), |rect| rect.size()) * zoom;
        // Where the platform hides the position (Wayland), keep the one that was saved.
        let position = match outer {
            Some(rect) => [rect.min.x * zoom, rect.min.y * zoom],
            None => self
                .settings
                .ui
                .window
                .map_or([0.0, 0.0], |window| [window[0], window[1]]),
        };
        if let Some(window) = window_to_save(position, [size.x, size.y])
            && self
                .settings
                .ui
                .window
                .is_none_or(|old| old.iter().zip(window).any(|(a, b)| (a - b).abs() > 0.5))
        {
            self.settings.ui.window = Some(window);
        }
    }

    fn bottom_bar(&mut self, root: &mut egui::Ui) {
        let cooling = self
            .active_cooldown()
            .map(|(until, reason)| Self::cooldown_text(until, reason));
        let frame = egui::Frame::side_top_panel(root.style())
            .fill(style::chrome_fill(root.visuals()))
            .inner_margin(egui::Margin::symmetric(10, 4));
        egui::Panel::bottom("status").frame(frame).show(root, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(24.0);
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
                        .small_button(if self.worker.pause.load(Ordering::Relaxed) {
                            "Resume"
                        } else {
                            "Pause"
                        })
                        .clicked()
                    {
                        let old = self.worker.pause.load(Ordering::Relaxed);
                        self.worker.pause.store(!old, Ordering::Relaxed);
                    }
                    if ui.small_button("Cancel").clicked() {
                        self.worker.cancel.store(true, Ordering::Relaxed);
                    }
                } else if self.status_error {
                    // An error is marked by a sign as well as its colour and leads to the log.
                    let text = RichText::new(format!("⚠ {}", self.status))
                        .color(style::error(ui.visuals()));
                    if ui
                        .add(egui::Label::new(text).truncate().sense(Sense::click()))
                        .on_hover_text("Open the log")
                        .clicked()
                    {
                        self.page = Page::Log;
                    }
                } else {
                    ui.add(egui::Label::new(&self.status).truncate());
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let offline = self.settings.anidb.offline;
                    let (text, color, hint) = if offline {
                        (
                            "Offline",
                            style::warn(ui.visuals()),
                            "Work offline is on: commands stay queued and nothing is sent to \
                             AniDB. Click to go online.",
                        )
                    } else {
                        (
                            "Online",
                            style::ok(ui.visuals()),
                            "Work offline is off: queued commands are sent when the command \
                             queue runs. Click to work offline.",
                        )
                    };
                    // The dot is painted, not a "●" glyph: the glyph sits above the text's
                    // centre in the UI font.
                    let dot = egui::Atom::paint(egui::Vec2::splat(8.0), move |ui, args| {
                        ui.painter().circle_filled(args.rect.center(), 4.0, color);
                    });
                    if ui
                        .add(egui::Button::new((dot, RichText::new(text).color(color))).small())
                        .on_hover_text(hint)
                        .clicked()
                    {
                        self.settings.anidb.offline = !offline;
                        self.persist_settings_incidentally();
                        self.log(if offline {
                            "Working online"
                        } else {
                            "Working offline"
                        });
                    }
                    if let Some(text) = &cooling {
                        ui.colored_label(style::warn(ui.visuals()), text);
                    }
                    if self.projection.summary.queue_blocked > 0 || self.blocked_unconfirmed() {
                        let text = if self.blocked_unconfirmed() {
                            "blocked commands (refreshing)".to_string()
                        } else {
                            style::plural(
                                self.projection.summary.queue_blocked,
                                "blocked command",
                                "blocked commands",
                            )
                        };
                        if ui
                            .link(RichText::new(text).color(style::error(ui.visuals())))
                            .on_hover_text("Open the queue")
                            .clicked()
                        {
                            self.page = Page::Queue;
                        }
                    }
                    if !self.deferred.is_empty() {
                        ui.colored_label(
                            style::warn(ui.visuals()),
                            format!(
                                "{} not saved yet",
                                style::plural(self.deferred.len(), "command", "commands")
                            ),
                        );
                    }
                });
            });
        });
    }
}

/// Library backup, restore, check and compaction (File and Tools menus). Pickers and dialogs
/// run here on the UI thread; every database and file operation runs on the worker, or for the
/// file swap of a restore on the `animatic-restore` thread.
impl AnimaticApp {
    /// Why no worker-side library task can start: the worker is busy, or an import or the
    /// review of imported settings is open.
    fn busy_reason(&self) -> Option<&'static str> {
        if !self.worker.is_idle() || self.activity.busy {
            Some("Wait for the running task to finish, or cancel it, first.")
        } else if self.import_confirm.is_some() {
            Some("Confirm or cancel the import first.")
        } else if self.import_pending {
            Some("An import is in progress.")
        } else if self.imported_settings.is_some() {
            Some("Apply or cancel the imported settings first.")
        } else {
            None
        }
    }

    /// Why a backup, check or compaction cannot start now.
    fn library_task_blocker(&self) -> Option<&'static str> {
        if self.restore.is_some() {
            Some("A library restore is in progress.")
        } else {
            self.busy_reason()
        }
    }

    /// Why a restore cannot start now: besides the busy states, queued commands that are not
    /// saved yet would be lost or land in the wrong library.
    fn restore_start_blocker(&self) -> Option<&'static str> {
        self.library_task_blocker().or_else(|| {
            (!self.deferred.is_empty()).then_some("Some changes are not saved to the database yet.")
        })
    }

    fn backup_library(&mut self) {
        if let Some(reason) = self.library_task_blocker() {
            self.error(format!("Library not backed up: {reason}"));
            return;
        }
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let Some(dest) = rfd::FileDialog::new()
            .set_title("Back up library")
            .set_file_name(format!("animatic-backup-{stamp}.sqlite3"))
            .add_filter("SQLite database", &["sqlite3", "sqlite", "db"])
            .save_file()
        else {
            return;
        };
        self.worker.submit(Job::BackupLibrary { dest });
    }

    fn restore_library(&mut self) {
        if let Some(reason) = self.restore_start_blocker() {
            self.error(format!("Library not restored: {reason}"));
            return;
        }
        let Some(source) = rfd::FileDialog::new()
            .set_title("Choose a library backup to restore")
            .add_filter("SQLite database", &["sqlite3", "sqlite", "db"])
            .add_filter("All files", &["*"])
            .pick_file()
        else {
            return;
        };
        self.restore = Some(RestoreStage::Validating {
            source: source.clone(),
        });
        self.worker.submit(Job::ValidateBackup { source });
    }

    fn library_checked(&mut self, report: &IntegrityReport) {
        if report.is_ok() {
            self.log(report.summary());
            return;
        }
        for line in report.lines().take(20) {
            self.note(format!("  {line}"));
        }
        let mut message = format!(
            "{}. Nothing was repaired; restore a backup with File → Restore library…",
            report.summary()
        );
        if let Some(backup) = &self.last_backup {
            message.push_str(&format!(
                " (latest backup made in this session: {})",
                backup.display()
            ));
        }
        self.error(message);
    }

    /// Confirmed restore: stop the worker and projection threads, close the UI connection,
    /// then let the restore thread swap the files. The state machine continues in
    /// [`Self::finish_restore`] when the thread reports.
    fn begin_restore(&mut self, source: PathBuf) {
        if let Some(reason) = self.restore_start_blocker() {
            self.error(format!("Library not restored: {reason}"));
            return;
        }
        let placeholder = match Db::memory() {
            Ok(db) => db,
            Err(e) => {
                self.error(format!("Library not restored: {e}"));
                return;
            }
        };
        self.worker.stop_and_join();
        drop(std::mem::replace(&mut self.db, placeholder));
        self.log("Restoring library…");
        let result = spawn_restore(self.db_path.clone(), source.clone());
        self.restore = Some(RestoreStage::Running { source, result });
    }

    /// The restore thread reported (or died): reopen the library, restart the worker and
    /// reload every projection, whatever the outcome.
    fn finish_restore(&mut self, result: RestoreResult) {
        self.restore = None;
        let restored = match &result {
            Ok(outcome) => {
                let kept = outcome
                    .previous_copy
                    .as_ref()
                    .map(|p| format!("; the previous library was kept as {}", p.display()))
                    .unwrap_or_default();
                self.log(format!("Library restored{kept}"));
                true
            }
            Err(e) => {
                self.error(format!("Library restore failed: {e}"));
                false
            }
        };
        match Db::open(&self.db_path).and_then(|db| {
            db.conn().busy_timeout(UI_DB_BUSY_TIMEOUT)?;
            Ok(db)
        }) {
            Ok(db) => {
                self.db = db;
                self.worker = Worker::spawn(self.db_path.clone());
                self.reset_library_views();
            }
            Err(e) => {
                self.restore = Some(RestoreStage::Broken(format!(
                    "The library could not be opened after {}: {e}. Close Animatic; \
                     the previous library, if any, is next to the database file as \
                     .before-restore-*.",
                    if restored {
                        "the restore"
                    } else {
                        "the failed restore"
                    }
                )));
            }
        }
    }

    /// Forget everything shown from the previous library and request a fresh projection.
    fn reset_library_views(&mut self) {
        self.projection = LibraryProjection::default();
        self.detail = None;
        self.mylist_overlay.clear();
        self.cooldown = None;
        self.cooldown_floor = 0;
        self.blocked_floor = 0;
        self.projection_generation = 0;
        self.selected_aid = None;
        self.selected_local = None;
        self.rename_preview = None;
        self.mylist_ui.marked_locals.clear();
        self.batch_rename = None;
        self.activity = Activity::default();
        self.loader.outstanding = None;
        self.loader.last_request = None;
        self.loader.version += 1;
        self.request_library_refresh();
    }

    fn library_safety_windows(&mut self, ctx: &egui::Context) {
        self.restore_window(ctx);
        self.compact_window(ctx);
    }

    fn compact_window(&mut self, ctx: &egui::Context) {
        if !self.compact_confirm {
            return;
        }
        let mut go = None;
        let response = egui::Modal::new(egui::Id::unique("compact_library")).show(ctx, |ui| {
            style::section(ui, "Compact library");
            ui.label(
                "Rewrites the library file without its unused space (SQLite VACUUM). It needs \
                 free disk space about the size of the library and blocks other background \
                 work until it is done. Nothing is lost.",
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Compact").clicked() {
                    go = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    go = Some(false);
                }
            });
        });
        if response.should_close() && go.is_none() {
            go = Some(false);
        }
        let Some(go) = go else { return };
        self.compact_confirm = false;
        if !go {
            return;
        }
        if let Some(reason) = self.library_task_blocker() {
            self.error(format!("Library not compacted: {reason}"));
        } else {
            self.worker.submit(Job::CompactLibrary);
        }
    }

    fn restore_window(&mut self, ctx: &egui::Context) {
        let id = egui::Id::unique("restore_library");
        let mut start = None;
        let mut cancelled = false;
        let mut quit = false;
        let mut finished = None;
        match &self.restore {
            None => return,
            Some(RestoreStage::Validating { source }) => {
                let response = egui::Modal::new(id).show(ctx, |ui| {
                    style::section(ui, "Restore library");
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Checking {}…", source.display()));
                    });
                    ui.label(
                        "The backup is read and its integrity checked before anything changes. \
                         A large library can take a while.",
                    );
                    ui.add_space(8.0);
                    if ui.button("Cancel").clicked() {
                        cancelled = true;
                    }
                });
                // Escape or a click outside does not dismiss the check by accident.
                let _ = response;
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            Some(RestoreStage::Confirm { source, info }) => {
                let response = egui::Modal::new(id).show(ctx, |ui| {
                    style::section(ui, "Restore library");
                    ui.label(format!(
                        "Replace the current library with {}?",
                        source.display()
                    ));
                    ui.label(format!(
                        "The backup holds {} anime, {} My list entries, {} hashed files and {} \
                         queued commands ({}).",
                        format::thousands(info.anime),
                        format::thousands(info.mylist),
                        format::thousands(info.local_files),
                        format::thousands(info.queued_commands),
                        format::size_short(info.size as i64)
                    ));
                    ui.colored_label(
                        style::warn(ui.visuals()),
                        "The current library is kept next to its file as \
                     <name>.before-restore-<date>-<time>. Background work stops while the \
                     files are swapped.",
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Restore").clicked() {
                            start = Some(source.clone());
                        }
                        if ui.button("Cancel").clicked() {
                            cancelled = true;
                        }
                    });
                });
                cancelled |= response.should_close() && start.is_none();
            }
            Some(RestoreStage::Running { source, result }) => {
                egui::Modal::new(id).show(ctx, |ui| {
                    style::section(ui, "Restoring library");
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Replacing the library with {}…", source.display()));
                    });
                    ui.label("Do not close Animatic until this is finished.");
                });
                match result.try_recv() {
                    Ok(outcome) => finished = Some(outcome),
                    Err(TryRecvError::Empty) => {
                        ctx.request_repaint_after(Duration::from_millis(100));
                    }
                    Err(TryRecvError::Disconnected) => {
                        finished = Some(Err("the restore thread stopped unexpectedly".into()));
                    }
                }
            }
            Some(RestoreStage::Broken(message)) => {
                egui::Modal::new(id).show(ctx, |ui| {
                    style::section(ui, "Library unavailable");
                    ui.colored_label(style::error(ui.visuals()), message);
                    if ui.button("Quit").clicked() {
                        quit = true;
                    }
                });
            }
        }
        if let Some(outcome) = finished {
            self.finish_restore(outcome);
        } else if let Some(source) = start {
            self.restore = None;
            self.begin_restore(source);
        } else if cancelled {
            self.restore = None;
        }
        if quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// A restore still swapping files when the window closes is waited for, so that the files
    /// are never left half swapped by the process ending.
    fn finish_restore_before_exit(&mut self) {
        if let Some(RestoreStage::Running { result, .. }) = &self.restore {
            let _ = result.recv_timeout(EXIT_RESTORE_WAIT);
        }
    }
}

impl AnimaticApp {
    /// Why a scheduled scan must not start now (the worker's own busy state is checked by the
    /// scheduler thread, atomically with the submission).
    fn scan_blocker(&self) -> Option<&'static str> {
        if self.restore.is_some() {
            Some("a library restore is in progress")
        } else if self.import_confirm.is_some() || self.import_pending {
            Some("an import is in progress")
        } else if self.imported_settings.is_some() {
            Some("imported settings are waiting for review")
        } else if self.settings_notice.is_some() {
            Some("the settings recovery notice is open")
        } else if !self.deferred.is_empty() {
            Some("some changes are not saved to the database yet")
        } else {
            None
        }
    }

    /// Give the scheduler thread the current settings and blocker. While Options is open the
    /// settings it holds stay as they were, so half-edited values never start a scan.
    fn sync_scan_schedule(&self) {
        let control = self.worker.scan_control();
        if self.settings_open {
            control.set_blocker(self.scan_blocker());
        } else {
            control.sync(
                &self.settings.file_managers,
                &self.settings.hashing.extensions,
                self.scan_blocker(),
            );
        }
    }

    /// Give the automatic queue scheduler and the worker's session upkeep the current AniDB
    /// settings and the same blocker as the scan scheduler. While Options is open the settings
    /// they hold stay as they were, so half-edited values never start a run.
    fn sync_queue_schedule(&self) {
        let control = self.worker.queue_control();
        if self.settings_open {
            control.set_blocker(self.scan_blocker());
        } else {
            control.sync(&self.settings.anidb, self.scan_blocker());
        }
    }
}

impl eframe::App for AnimaticApp {
    /// Also runs while the window is minimized or hidden to the tray, when eframe runs no UI
    /// pass at all. Files forwarded by another launch are hashed (and the window restored)
    /// without waiting for the user, and the tray keeps working: it is what brings the window
    /// back.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tray_frame(ctx);
        self.process_handoff(ctx);
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.track_window(&ctx);
        self.process_events(&ctx);
        self.pump_auto_rename(&ctx);
        self.process_update_events(&ctx);
        self.poll_shell(&ctx);
        self.process_media_events(&ctx);
        self.retry_deferred();
        self.sync_scan_schedule();
        self.sync_queue_schedule();
        self.pump_library(&ctx);
        self.handle_shortcuts(&ctx);
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
        self.pattern_reference_window(&ctx);
        self.update_window(&ctx);
        self.settings_notice_window(&ctx);
        self.legacy_import_confirm_window(&ctx);
        self.leftover_import_window(&ctx);
        self.library_safety_windows(&ctx);
        self.rename_window(&ctx);
        self.mylist_windows(&ctx);
        self.full_update_window(&ctx);
        self.mylist_export_window(&ctx);
        self.batch_rename_window(&ctx);
        self.auto_rename_enable_window(&ctx);
        self.media_install_window(&ctx);
        self.about_window(&ctx);
        self.license_window(&ctx);
        self.notices_window(&ctx);
        // Worker events are only seen while frames run, so poll while anything is in flight.
        // A job the scheduler thread started (autostart, scheduled scan) has no UI action that
        // triggers a frame, so the worker's own pending count keeps the polling going too.
        if self.activity.busy
            || !self.worker.is_idle()
            || self.worker.scan_control().hash_resume_pending()
            || self.loader.outstanding.is_some()
            || self.loader.dirty
            || !self.deferred.is_empty()
            || self.update.stage.is_active()
            || self.media_ui.working()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else if self.active_cooldown().is_some() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    fn on_exit(&mut self) {
        self.finish_restore_before_exit();
        if self.settings_guard.allows_exit_save() {
            let _ = self.write_settings();
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
        if self.restart.load(Ordering::SeqCst) {
            // The window is closing for a restart: close the worker's database connection too
            // before `main` relaunches. Everything was idle when the restart was requested.
            self.worker.stop_and_join();
        }
    }
}

/// What an empty page shows instead of a bare table: what would be here, and how it gets here.
fn empty_state(ui: &mut egui::Ui, title: &str, detail: &str) {
    ui.add_space((ui.available_height() * 0.2).min(120.0));
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(title)
                .text_style(style::section_style())
                .color(ui.visuals().strong_text_color()),
        );
        ui.add_space(4.0);
        ui.label(RichText::new(detail).weak());
    });
}

/// A figure card: the value large, its meaning small underneath.
fn metric(ui: &mut egui::Ui, label: &str, value: String) {
    metric_card(ui, label, value, None);
}

/// A figure card; `accent` colours the value of a figure that needs attention.
fn metric_card(
    ui: &mut egui::Ui,
    label: &str,
    value: String,
    accent: Option<Color32>,
) -> egui::Response {
    let visuals = ui.visuals().clone();
    egui::Frame::new()
        .fill(visuals.faint_bg_color)
        .stroke(visuals.widgets.noninteractive.bg_stroke)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                RichText::new(value)
                    .size(24.0)
                    .color(accent.unwrap_or_else(|| visuals.strong_text_color())),
            );
            ui.label(RichText::new(label).small().weak());
        })
        .response
}

/// One sidebar entry: full width, the selected one marked by the accent bar as well as its fill,
/// with an optional count on the right.
fn nav_item(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    badge: Option<(String, Color32)>,
) -> egui::Response {
    let text = if selected {
        RichText::new(label).strong()
    } else {
        RichText::new(label)
    };
    let mut button = egui::Button::selectable(selected, text)
        .frame_when_inactive(false)
        .min_size(egui::vec2(ui.available_width(), 28.0));
    if let Some((badge, color)) = badge {
        button = button.right_text(RichText::new(badge).small().color(color));
    }
    let response = ui.add(button);
    if selected {
        let rect = response.rect;
        let bar = egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height()));
        ui.painter()
            .rect_filled(bar, 2.0, style::accent(ui.visuals()));
    }
    response
}

/// Keyboard shortcuts of the main window. `COMMAND` is Ctrl on Windows and Linux.
mod shortcuts {
    use eframe::egui::{Key, KeyboardShortcut, Modifiers};

    pub const ADD_FILES: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
    pub const SEARCH: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::F);
    pub const OPTIONS: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Comma);
    pub const RELOAD: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F5);
    pub const RUN_QUEUE: KeyboardShortcut =
        KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
    /// `Ctrl+1` … `Ctrl+9` open the pages in sidebar order.
    pub const PAGES: [Key; 9] = [
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
    ];
}

/// `Shortcut: Ctrl+,` for a hover text.
fn shortcut_hint(ctx: &egui::Context, shortcut: &egui::KeyboardShortcut) -> String {
    format!("Shortcut: {}", ctx.format_shortcut(shortcut))
}

/// A menu entry that shows its keyboard shortcut.
fn menu_entry<'a>(
    ctx: &egui::Context,
    text: &'a str,
    shortcut: &egui::KeyboardShortcut,
) -> egui::Button<'a> {
    egui::Button::new(text).shortcut_text(ctx.format_shortcut(shortcut))
}

/// Categories of the Options window, in the order of its list.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum OptionsPage {
    Account,
    Appearance,
    Library,
    Hashing,
    Renaming,
    Tools,
    Exports,
    Updates,
}

impl OptionsPage {
    const ALL: [OptionsPage; 8] = [
        OptionsPage::Account,
        OptionsPage::Appearance,
        OptionsPage::Library,
        OptionsPage::Hashing,
        OptionsPage::Renaming,
        OptionsPage::Tools,
        OptionsPage::Exports,
        OptionsPage::Updates,
    ];

    fn label(self) -> &'static str {
        match self {
            OptionsPage::Account => "AniDB account",
            OptionsPage::Appearance => "Appearance",
            OptionsPage::Library => "Managed folders",
            OptionsPage::Hashing => "Hashing",
            OptionsPage::Renaming => "Renaming",
            OptionsPage::Tools => "Players and tools",
            OptionsPage::Exports => "Exports",
            OptionsPage::Updates => "Updates",
        }
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

/// Row indices of anime whose name contains the lower-cased query or whose id equals it, and
/// that have `genre` when one is given.
fn filter_anime(p: &LibraryProjection, q: &str, genre: Option<i64>) -> Vec<usize> {
    let q_aid = aid_query(q);
    (0..p.anime.len())
        .filter(|&i| {
            let a = &p.anime[i];
            matches_anime(&p.keys.anime[i], a.aid, q, q_aid)
                && genre.is_none_or(|g| {
                    p.anime_genres
                        .get(&a.aid)
                        .is_some_and(|ids| ids.contains(&g))
                })
        })
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

/// Width of the label column of forms, so the fields of a section line up.
const LABEL_WIDTH: f32 = 140.0;
/// Width of single-line text fields in forms.
const FIELD_WIDTH: f32 = 340.0;

/// A form label in the fixed-width label column.
fn field_label(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let height = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(
        egui::vec2(LABEL_WIDTH, height),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_width(LABEL_WIDTH);
            ui.label(label)
        },
    )
    .inner
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        let label = field_label(ui, label);
        ui.add(egui::TextEdit::singleline(value).desired_width(FIELD_WIDTH))
            .labelled_by(label.id);
    });
}

/// One line of the batch rename preview: status, the file and its planned target or problem.
/// The tooltip carries the full paths and every diagnostic.
fn batch_rename_line(
    ui: &mut egui::Ui,
    row: &BatchRow,
    applied: bool,
    result: Option<&Result<String, String>>,
) {
    let name = row
        .old
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let visuals = ui.visuals();
    let (color, status, text) = match result {
        Some(Ok(how)) => (
            style::ok(visuals),
            how.clone(),
            row.plan.target.as_ref().map(|t| t.display().to_string()),
        ),
        Some(Err(why)) => (
            style::error(visuals),
            format!("Not renamed: {why}"),
            row.plan.target.as_ref().map(|t| t.display().to_string()),
        ),
        None if row.plan.can_apply() => (
            style::ok(visuals),
            if applied { "Waiting" } else { "Ready" }.to_string(),
            row.plan.target.as_ref().map(|t| t.display().to_string()),
        ),
        None if row.plan.already_named() => {
            (visuals.weak_text_color(), "Already named".to_string(), None)
        }
        None => (
            style::error(visuals),
            "Blocked".to_string(),
            Some(
                row.plan
                    .errors
                    .first()
                    .or(row.plan.warnings.first())
                    .cloned()
                    .unwrap_or_default(),
            ),
        ),
    };
    let response = ui
        .horizontal(|ui| {
            ui.colored_label(color, status);
            let line = match text {
                Some(text) if row.plan.can_apply() => format!("{name} → {text}"),
                Some(text) => format!("{name}: {text}"),
                None => name.clone(),
            };
            ui.add(egui::Label::new(line).truncate());
        })
        .response;
    response.on_hover_ui(|ui| {
        ui.label(format!("Current path: {}", row.old.display()));
        if let Some(target) = &row.plan.target {
            ui.label(format!("Planned path: {}", target.display()));
        }
        show_plan(ui, &row.plan);
    });
}

/// A text field with a hover explanation.
fn field_hint(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    ui.horizontal(|ui| {
        let label = field_label(ui, label);
        ui.add(egui::TextEdit::singleline(value).desired_width(FIELD_WIDTH))
            .labelled_by(label.id)
            .on_hover_text(hint);
    });
}

/// Warnings and errors of a rename plan; warnings block applying it just like errors do.
/// `text` cut to `max` characters with an ellipsis, so a runaway pattern result stays cheap
/// to lay out every frame.
fn shortened(text: &str, max: usize) -> std::borrow::Cow<'_, str> {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]).into(),
        None => text.into(),
    }
}

fn show_plan(ui: &mut egui::Ui, plan: &Plan) {
    for error in &plan.errors {
        ui.colored_label(style::error(ui.visuals()), error);
    }
    if !plan.warnings.is_empty() {
        ui.colored_label(
            style::warn(ui.visuals()),
            "The pattern reported warnings, so its result cannot be trusted. Renaming stays \
         disabled until they are fixed:",
        );
        for warning in &plan.warnings {
            ui.colored_label(style::warn(ui.visuals()), format!("• {warning}"));
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
        assert_eq!(filter_anime(&p, "", None).len(), 3);
        assert_eq!(filter_anime(&p, "fixture", None).len(), 2);
        let by_id = filter_anime(&p, "3", None);
        assert_eq!(by_id.len(), 1);
        assert_eq!(p.anime[by_id[0]].aid, 3);
        assert!(
            filter_anime(&p, "03", None).is_empty(),
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
    fn genre_filter_keeps_only_anime_of_that_genre() {
        let mut p = fixture();
        let aids: Vec<i64> = p.anime.iter().map(|a| a.aid).collect();
        p.anime_genres.insert(aids[0], vec![7, 8]);
        p.anime_genres.insert(aids[1], vec![8]);
        let picked = |rows: Vec<usize>| rows.iter().map(|&i| p.anime[i].aid).collect::<Vec<_>>();
        assert_eq!(picked(filter_anime(&p, "", Some(7))), vec![aids[0]]);
        assert_eq!(picked(filter_anime(&p, "", Some(8))).len(), 2);
        assert!(filter_anime(&p, "", Some(9)).is_empty());
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

    #[test]
    fn automatic_check_failures_are_silent_unless_the_user_joined_the_check() {
        let mut stage = UpdateStage::Idle;
        assert!(stage.begin_check(false));
        assert_eq!(stage.check_failed(), Some(false));
        assert_eq!(stage, UpdateStage::Idle, "a later check is possible");

        assert!(stage.begin_check(false));
        assert!(!stage.begin_check(true), "the running check is adopted");
        assert_eq!(stage.check_failed(), Some(true));
        assert_eq!(stage.check_failed(), None, "a stale failure is ignored");
    }

    #[test]
    fn cancelling_returns_to_the_last_stable_stage_and_allows_a_retry() {
        let mut stage = UpdateStage::Idle;
        stage.begin_check(true);
        assert!(stage.cancel());
        stage.cancelled();
        assert_eq!(stage, UpdateStage::Idle);

        stage.begin_check(true);
        assert!(stage.check_finished(true));
        assert!(stage.begin_download());
        stage.progress(5, 10);
        assert!(stage.cancel());
        stage.cancelled();
        assert_eq!(stage, UpdateStage::Available);
        assert!(stage.begin_download(), "the release is kept for a retry");
        assert!(stage.download_failed());
        assert_eq!(stage, UpdateStage::Available);
    }

    #[test]
    fn installing_needs_a_staged_update_and_confirmation() {
        let mut stage = UpdateStage::Available;
        assert!(!stage.begin_install(), "nothing is staged");
        stage.begin_download();
        assert!(stage.download_ready());
        assert!(!stage.begin_install(), "not confirmed");
        stage.set_confirming(true);
        stage.set_confirming(false);
        assert!(!stage.begin_install(), "confirmation withdrawn");
        stage.set_confirming(true);
        assert!(stage.begin_install());
        assert!(!stage.begin_install(), "one installation at a time");

        // Refused before anything was replaced: only the staged files are lost.
        assert!(stage.install_failed(false));
        assert_eq!(stage, UpdateStage::Available);
        stage.begin_download();
        stage.download_ready();
        stage.set_confirming(true);
        assert!(stage.begin_install());

        // After the replacement was entered nothing else follows: the file may be changed.
        assert!(stage.install_failed(true));
        assert_eq!(stage, UpdateStage::InstallFailed);
        assert!(!stage.begin_check(true));
        assert!(!stage.begin_download());
        assert!(!stage.begin_install());
        assert!(!stage.check_finished(true));
    }

    #[test]
    fn after_an_installation_the_running_old_build_never_checks_again() {
        let mut stage = UpdateStage::Installing;
        assert!(stage.installed());
        assert_eq!(stage, UpdateStage::Installed);
        assert!(!stage.begin_check(true));
        assert!(!stage.begin_check(false));
        assert!(!stage.begin_download());
        assert!(!stage.check_finished(true), "stale replies are ignored");
        assert!(!stage.download_ready());
        assert_eq!(stage, UpdateStage::Installed);
    }

    #[test]
    fn the_startup_check_respects_the_preference_offline_mode_and_recovery() {
        let mut settings = Settings::default();
        assert!(startup_check_allowed(&settings, false));
        assert!(!startup_check_allowed(&settings, true));
        settings.anidb.offline = true;
        assert!(!startup_check_allowed(&settings, false));
        settings.anidb.offline = false;
        settings.updates.auto_check = false;
        assert!(!startup_check_allowed(&settings, false));
    }
}
