use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use animatic_core::db::Db;
use animatic_core::format;
use animatic_core::library::{
    Anime, AnimeMylistState, Group, LocalFile, MylistItem, mylist_state_name,
};
use animatic_core::settings::{MYLIST_STATES, Settings, Theme};
use eframe::egui::{self, Align, Color32, Layout, RichText, Sense};

use crate::worker::{Event, Job, Worker};

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

#[derive(Default)]
struct Cache {
    anime: Vec<Anime>,
    mylist: Vec<MylistItem>,
    states: HashMap<i64, AnimeMylistState>,
    local: Vec<LocalFile>,
    groups: Vec<Group>,
    genres: Vec<(i64, String, i64)>,
    queue: Vec<(i64, animatic_core::sync::Command, i64, Option<String>)>,
}

#[derive(Default)]
struct Activity {
    busy: bool,
    label: String,
    current: usize,
    total: usize,
    fraction: f32,
}

pub struct AnimaticApp {
    db: Db,
    db_path: PathBuf,
    settings_path: PathBuf,
    settings: Settings,
    cache: Cache,
    worker: Worker,
    page: Page,
    query: String,
    selected_aid: Option<i64>,
    settings_open: bool,
    about_open: bool,
    selected_local: Option<i64>,
    rename_preview: Option<(i64, PathBuf, PathBuf)>,
    import_source: String,
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
        let settings = Settings::load(&settings_path)?;
        let worker = Worker::spawn(db_path.clone());
        let mut app = Self {
            db,
            db_path,
            settings_path,
            settings,
            cache: Cache::default(),
            worker,
            page: Page::Dashboard,
            query: String::new(),
            selected_aid: None,
            selected_local: None,
            rename_preview: None,
            settings_open: false,
            about_open: false,
            import_source: String::new(),
            search_year: String::new(),
            search_type: 0,
            search_mylist_only: false,
            search_watched_only: false,
            activity: Activity::default(),
            status: "Ready".into(),
            logs: Vec::new(),
        };
        app.reload();
        if full_update {
            app.db
                .queue_push(&animatic_core::sync::Command::TitlesDump)?;
            let mut fids = std::collections::BTreeSet::new();
            let mut aids = std::collections::BTreeSet::new();
            for file in &app.cache.local {
                if let Some(fid) = file.fid {
                    fids.insert(fid);
                }
                if let Some(aid) = file.aid {
                    aids.insert(aid);
                }
            }
            for fid in fids {
                app.db
                    .queue_push(&animatic_core::sync::Command::FetchFile { fid })?;
            }
            for aid in aids {
                app.db
                    .queue_push(&animatic_core::sync::Command::FetchAnime { aid })?;
            }
            app.reload();
        }
        if !initial_files.is_empty() {
            app.worker.submit(Job::Hash(initial_files));
        }
        Ok(app)
    }
    fn reload(&mut self) {
        let result = (|| -> animatic_core::Result<Cache> {
            let anime = self.db.all_anime()?;
            let mylist = self.db.mylist_items()?;
            let states = self.db.mylist_state_by_anime(&mylist);
            Ok(Cache {
                anime,
                mylist,
                states,
                local: self.db.local_files()?,
                groups: self.db.all_groups()?,
                genres: self.db.genres()?,
                queue: self.db.queue_list()?,
            })
        })();
        match result {
            Ok(cache) => {
                self.cache = cache;
                if self.settings.stat_whore.enabled && !self.settings.stat_whore.file.is_empty() {
                    let path = Path::new(&self.settings.stat_whore.file);
                    if let Err(e) = self.db.library_stats().and_then(|s| {
                        s.write(
                            path,
                            &self.settings.anidb.username,
                            self.settings.stat_whore.format,
                        )
                    }) {
                        self.error(format!("Automatic statistics export failed: {e}"));
                    }
                }
            }
            Err(e) => self.error(format!("Unable to refresh library: {e}")),
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
                        if let Err(e) = self
                            .db
                            .queue_push(&animatic_core::sync::Command::Identify { size, ed2k, add })
                        {
                            self.error(format!("Could not queue AniDB identification: {e}"));
                        }
                    }
                }
                Event::Imported { summary } => self.log(summary),
                Event::Synced {
                    description,
                    changed,
                } => {
                    self.log(description);
                    if changed {
                        self.reload();
                    }
                }
                Event::Error(e) => self.error(e),
                Event::Finished => {
                    self.activity.busy = false;
                    self.status = "Ready".into();
                    self.reload();
                }
            }
            ctx.request_repaint();
        }
    }

    fn submit_files(&mut self) {
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

    fn import_legacy(&mut self) {
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

    fn prepare_rename(&mut self) {
        let Some(id) = self.selected_local else {
            self.error("Select a hashed file first");
            return;
        };
        let Some(local) = self.cache.local.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        let Some(fid) = local.fid else {
            self.error("The selected file is not identified by AniDB");
            return;
        };
        let path = PathBuf::from(&local.path);
        let manager = self.settings.file_managers.manager_for(&path);
        let result = (|| -> Result<PathBuf, String> {
            let bindings = animatic_core::rename::objects::bindings_for_file(
                &self.db,
                fid,
                &local.path,
                &manager.base_path,
            )?;
            let pattern = animatic_core::rename::Pattern::compile(&self.settings.renaming_pattern)
                .map_err(|e| e.to_string())?;
            let evaluation = animatic_core::rename::evaluate(
                &pattern,
                &bindings,
                &animatic_core::rename::objects::DbSource { db: &self.db },
            )
            .map_err(|e| e.to_string())?;
            animatic_core::rename::target_path(&path, &evaluation.result, &manager.base_path)?
                .ok_or_else(|| "The pattern produces the current path; no rename is needed".into())
        })();
        match result {
            Ok(target) => self.rename_preview = Some((id, path, target)),
            Err(e) => self.error(e),
        }
    }

    fn rename_window(&mut self, ctx: &egui::Context) {
        let Some((id, old, new)) = self.rename_preview.clone() else {
            return;
        };
        let mut open = true;
        let mut apply = false;
        egui::Window::new("Confirm rename")
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Current path");
                ui.monospace(old.display().to_string());
                ui.add_space(8.0);
                ui.label("New path");
                ui.monospace(new.display().to_string());
                ui.add_space(12.0);
                if ui.button("Rename file").clicked() {
                    apply = true;
                }
            });
        if apply {
            match animatic_core::file_manager::move_file(&self.db, id, &old, &new) {
                Ok(()) => {
                    self.log(format!("Renamed {} to {}", old.display(), new.display()));
                    self.reload();
                }
                Err(e) => self.error(format!("Rename failed: {e}")),
            }
            self.rename_preview = None;
        } else if !open {
            self.rename_preview = None;
        }
    }

    fn play_selected(&mut self, action: animatic_core::external::PlayerAction) {
        let Some(file) = self
            .cache
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
            .cache
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
        match self.db.library_stats().and_then(|s| {
            s.write(
                &path,
                &self.settings.anidb.username,
                self.settings.stat_whore.format,
            )
        }) {
            Ok(()) => self.log(format!("Exported statistics to {}", path.display())),
            Err(e) => self.error(format!("Statistics export failed: {e}")),
        }
    }

    fn export_hashes(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("animatic-hashes.txt")
            .save_file()
        else {
            return;
        };
        let mut text = String::new();
        for f in &self.cache.local {
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
                self.cache.local.len(),
                path.display()
            )),
            Err(e) => self.error(format!("Hash export failed: {e}")),
        }
    }

    fn save_settings(&mut self) {
        match self.settings.save(&self.settings_path) {
            Ok(()) => self.log("Settings saved"),
            Err(e) => self.error(format!("Could not save settings: {e}")),
        }
    }

    fn top_bar(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        egui::Panel::top("menu").show(root, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Add files…").clicked() {
                        ui.close();
                        self.submit_files();
                    }
                    if ui.button("Import AOM databank…").clicked() {
                        ui.close();
                        if let Some(dir) = rfd::FileDialog::new()
                            .set_title("Choose AOM kowai folder")
                            .pick_folder()
                        {
                            self.import_source = dir.to_string_lossy().into_owned();
                            self.import_legacy();
                        }
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
                        self.reload();
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
                    if ui.button("Scan managed folders").clicked() {
                        self.worker.submit(Job::Scan {
                            settings: self.settings.file_managers.clone(),
                            extensions: self.settings.hashing.extensions.join(","),
                        });
                        ui.close();
                    }
                    if ui.button("Download anime titles").clicked() {
                        if let Err(e) = self
                            .db
                            .queue_push(&animatic_core::sync::Command::TitlesDump)
                        {
                            self.error(e.to_string());
                        }
                        self.reload();
                        ui.close();
                    }
                    if ui.button("Fetch mylist statistics").clicked() {
                        if let Err(e) = self
                            .db
                            .queue_push(&animatic_core::sync::Command::MylistStats)
                        {
                            self.error(e.to_string());
                        }
                        self.reload();
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
                    if ui.button("Run command queue").clicked() {
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
                    ui.label(format!("{} anime", self.cache.anime.len()));
                    ui.label(format!("{} mylist files", self.cache.mylist.len()));
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
        let known = self.cache.local.iter().filter(|f| f.known()).count();
        let watched = self.cache.mylist.iter().filter(|m| m.entry.viewed).count();
        let bytes: i64 = self.cache.local.iter().map(|f| f.size).sum();
        ui.columns(4, |cols| {
            metric(&mut cols[0], "Anime", self.cache.anime.len().to_string());
            metric(
                &mut cols[1],
                "Mylist files",
                self.cache.mylist.len().to_string(),
            );
            metric(&mut cols[2], "Watched", watched.to_string());
            metric(&mut cols[3], "Local size", format::size_short(bytes));
        });
        ui.add_space(20.0);
        ui.heading("Local library health");
        ui.label(format!(
            "{known} of {} hashed files identified by AniDB",
            self.cache.local.len()
        ));
        let bad = self.cache.local.iter().filter(|f| f.crc_mismatch()).count();
        if bad > 0 {
            ui.colored_label(
                Color32::from_rgb(235, 100, 100),
                format!("{bad} files have a CRC mismatch"),
            );
        }
        let unknown = self.cache.local.len().saturating_sub(known);
        if unknown > 0 {
            ui.label(format!("{unknown} files are not identified yet"));
        }
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            if ui.button("Add files").clicked() {
                self.submit_files();
            }
            if ui.button("Open hashed files").clicked() {
                self.page = Page::Hashed;
            }
            if ui.button("Refresh data").clicked() {
                self.reload();
            }
        });
        if self.settings.first_launch {
            ui.add_space(24.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("Bring your AniDB O'Matic library forward");
                ui.label("Import the encrypted databank directly. Existing anime, mylist, hashes and local paths are preserved.");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.import_source);
                    if ui.button("Browse…").clicked()
                        && let Some(d) = rfd::FileDialog::new().pick_folder() { self.import_source = d.to_string_lossy().into_owned(); }
                    if ui.button("Import").clicked() { self.import_legacy(); }
                });
                if ui.button("Dismiss first-run card").clicked() { self.settings.first_launch = false; self.save_settings(); }
            });
        }
    }

    fn anime_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "Anime database", true);
        let q = self.query.to_ascii_lowercase();
        let rows: Vec<_> = self
            .cache
            .anime
            .iter()
            .filter(|a| {
                q.is_empty() || a.name.to_ascii_lowercase().contains(&q) || a.aid.to_string() == q
            })
            .cloned()
            .collect();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("anime_table")
                .striped(true)
                .min_col_width(80.0)
                .show(ui, |ui| {
                    header(
                        ui,
                        &["Title", "Type", "Year", "Episodes", "Rating", "Mylist"],
                    );
                    for a in rows {
                        let selected = self.selected_aid == Some(a.aid);
                        if ui
                            .selectable_label(
                                selected,
                                if a.name.is_empty() {
                                    format!("aid {}", a.aid)
                                } else {
                                    a.name.clone()
                                },
                            )
                            .clicked()
                        {
                            self.selected_aid = Some(a.aid);
                        }
                        ui.label(a.type_name());
                        ui.label(&a.year);
                        ui.label(a.episodes.to_string());
                        ui.label(format::rating(a.rating));
                        let state = self.cache.states.get(&a.aid);
                        ui.label(
                            state
                                .map(|s| format!("{} eps / {} files", s.eps, s.files))
                                .unwrap_or_default(),
                        );
                        ui.end_row();
                    }
                });
        });
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
        let q = self.query.to_ascii_lowercase();
        let year = self.search_year.trim();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("advanced-results")
                .striped(true)
                .show(ui, |ui| {
                    header(
                        ui,
                        &["Title", "Type", "Year", "Episodes", "Rating", "Mylist"],
                    );
                    for a in &self.cache.anime {
                        let state = self.cache.states.get(&a.aid);
                        if !q.is_empty()
                            && !a.name.to_ascii_lowercase().contains(&q)
                            && a.aid.to_string() != q
                        {
                            continue;
                        }
                        if !year.is_empty() && !a.year.contains(year) {
                            continue;
                        }
                        if self.search_type != 0 && a.type_id != self.search_type {
                            continue;
                        }
                        if self.search_mylist_only && state.is_none() {
                            continue;
                        }
                        if self.search_watched_only && !state.is_some_and(|s| s.all_watched(a)) {
                            continue;
                        }
                        if ui
                            .link(if a.name.is_empty() {
                                format!("aid {}", a.aid)
                            } else {
                                a.name.clone()
                            })
                            .clicked()
                        {
                            self.selected_aid = Some(a.aid);
                        }
                        ui.label(a.type_name());
                        ui.label(&a.year);
                        ui.label(a.episodes.to_string());
                        ui.label(format::rating(a.rating));
                        ui.label(
                            state
                                .map(|s| format!("{} eps / {} watched", s.eps, s.watched_eps))
                                .unwrap_or_default(),
                        );
                        ui.end_row();
                    }
                });
        });
        self.anime_detail_window(ui.ctx());
    }

    fn mylist_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "My list", true);
        let q = self.query.to_ascii_lowercase();
        let names: HashMap<_, _> = self
            .cache
            .anime
            .iter()
            .map(|a| (a.aid, a.name.as_str()))
            .collect();
        let mut edits = Vec::new();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("mylist_table")
                .striped(true)
                .show(ui, |ui| {
                    header(
                        ui,
                        &["Anime", "Episode", "Group", "State", "Watched", "Size"],
                    );
                    for item in &self.cache.mylist {
                        let title = names
                            .get(&item.entry.aid)
                            .copied()
                            .unwrap_or("Unknown anime");
                        if !q.is_empty() && !title.to_ascii_lowercase().contains(&q) {
                            continue;
                        }
                        if ui.link(title).clicked() {
                            self.selected_aid = Some(item.entry.aid);
                        }
                        ui.label(
                            item.episode
                                .as_ref()
                                .map(|e| e.epno_str())
                                .unwrap_or_default(),
                        );
                        ui.label(
                            item.file
                                .as_ref()
                                .map(|f| f.group_label())
                                .unwrap_or_default(),
                        );
                        let mut state = item.entry.state;
                        egui::ComboBox::from_id_salt(("state", item.entry.lid))
                            .selected_text(mylist_state_name(state))
                            .show_ui(ui, |ui| {
                                for (id, name) in MYLIST_STATES.iter().enumerate() {
                                    ui.selectable_value(&mut state, id as i64, *name);
                                }
                            });
                        let mut watched = item.entry.viewed;
                        ui.checkbox(&mut watched, "");
                        if state != item.entry.state || watched != item.entry.viewed {
                            edits.push((
                                item.entry.lid,
                                animatic_anidb::commands::MylistEdit {
                                    state: (state != item.entry.state).then_some(state),
                                    viewed: (watched != item.entry.viewed).then_some(watched),
                                    viewdate: None,
                                    storage: None,
                                    source: None,
                                    other: None,
                                },
                            ));
                        }
                        ui.label(
                            item.file
                                .as_ref()
                                .map(|f| format::size_short(f.size))
                                .unwrap_or_default(),
                        );
                        ui.end_row();
                    }
                });
        });
        let had_edits = !edits.is_empty();
        for (lid, edit) in edits {
            if let Err(e) =
                animatic_core::sync::apply_edit_locally(&self.db, lid, &edit).and_then(|_| {
                    self.db
                        .queue_push(&animatic_core::sync::Command::MylistEdit { lid, edit })
                        .map(|_| ())
                })
            {
                self.error(format!("Could not edit mylist entry {lid}: {e}"));
            }
        }
        if had_edits {
            self.reload();
        }
        self.anime_detail_window(ui.ctx());
    }

    fn hashed_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "Hashed files", true);
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
                    .cache
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
            ui.label(format!("{} files", self.cache.local.len()));
        });
        let q = self.query.to_ascii_lowercase();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("local_table").striped(true).show(ui, |ui| {
                header(ui, &["File", "Size", "AniDB", "Mylist", "CRC", "Path"]);
                for f in &self.cache.local {
                    if !q.is_empty() && !f.path.to_ascii_lowercase().contains(&q) {
                        continue;
                    }
                    if ui
                        .selectable_label(self.selected_local == Some(f.id), f.file_name())
                        .clicked()
                    {
                        self.selected_local = Some(f.id);
                    }
                    ui.label(format::size_short(f.size));
                    ui.label(
                        f.fid
                            .map(|id| format!("fid {id}"))
                            .unwrap_or_else(|| "Unknown".into()),
                    );
                    ui.label(
                        f.lid
                            .map(|id| format!("lid {id}"))
                            .unwrap_or_else(|| "—".into()),
                    );
                    ui.label(if f.crc_mismatch() {
                        "Mismatch"
                    } else {
                        f.crc32.as_deref().unwrap_or("—")
                    });
                    if ui
                        .add(egui::Label::new(&f.path).sense(Sense::click()))
                        .double_clicked()
                    {
                        let _ = open::that(&f.path);
                    }
                    ui.end_row();
                }
            });
        });
    }

    fn groups_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "Groups", true);
        let q = self.query.to_ascii_lowercase();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("groups").striped(true).show(ui, |ui| {
                header(ui, &["Name", "Short name", "Rating", "Votes", "Website"]);
                for g in &self.cache.groups {
                    if !q.is_empty()
                        && !g.name.to_ascii_lowercase().contains(&q)
                        && !g.short.to_ascii_lowercase().contains(&q)
                    {
                        continue;
                    }
                    ui.label(&g.name);
                    ui.label(&g.short);
                    ui.label(format::rating(g.rating));
                    ui.label(g.votes.to_string());
                    if let Some(url) = &g.url {
                        if ui.link("Open").clicked() {
                            let _ = open::that(url);
                        }
                    } else {
                        ui.label("—");
                    }
                    ui.end_row();
                }
            })
        });
    }

    fn genres_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "Genres", true);
        let q = self.query.to_ascii_lowercase();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("genres").striped(true).show(ui, |ui| {
                header(ui, &["Genre", "Anime"]);
                for (_, name, count) in &self.cache.genres {
                    if q.is_empty() || name.to_ascii_lowercase().contains(&q) {
                        ui.label(name);
                        ui.label(count.to_string());
                        ui.end_row();
                    }
                }
            })
        });
    }

    fn queue_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "AniDB command queue", false);
        ui.horizontal(|ui| {
            ui.label(format!("{} pending commands", self.cache.queue.len()));
            if ui.button("Clear queue").clicked() {
                if let Err(e) = self.db.queue_clear() {
                    self.error(e.to_string());
                }
                self.reload();
            }
            if ui.button("Synchronize now").clicked() {
                self.worker
                    .submit(Job::ProcessQueue(self.settings.anidb.clone()));
            }
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (id, cmd, tries, err) in &self.cache.queue {
                ui.horizontal(|ui| {
                    ui.monospace(format!("#{id}"));
                    ui.label(cmd.describe());
                    if *tries > 0 {
                        ui.colored_label(Color32::YELLOW, format!("{tries} retries"));
                    }
                });
                if let Some(e) = err {
                    ui.small(RichText::new(e).color(Color32::LIGHT_RED));
                }
                ui.separator();
            }
        });
    }

    fn log_page(&mut self, ui: &mut egui::Ui) {
        self.page_header(ui, "Activity log", false);
        ui.horizontal(|ui| {
            if ui.button("Clear").clicked() {
                self.logs.clear();
            }
        });
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for line in &self.logs {
                    ui.monospace(line);
                }
            });
    }

    fn anime_detail_window(&mut self, ctx: &egui::Context) {
        let Some(aid) = self.selected_aid else {
            return;
        };
        let detail = self.db.anime_detail(aid).ok().flatten();
        let mut open = true;
        egui::Window::new(format!("Anime #{aid}"))
            .open(&mut open)
            .default_width(520.0)
            .show(ctx, |ui| {
                if let Some(d) = detail {
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
                            let _ = self
                                .db
                                .queue_push(&animatic_core::sync::Command::FetchAnime { aid });
                            self.reload();
                        }
                    });
                } else {
                    ui.label("No detail record is available.");
                }
            });
        if !open {
            self.selected_aid = None;
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
                    ui.add(
                        egui::TextEdit::multiline(&mut self.settings.renaming_pattern)
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(12),
                    );
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
                });
            });
        });
    }
}

impl eframe::App for AnimaticApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.process_events(&ctx);
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
        self.settings_window(&ctx);
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
    }

    fn on_exit(&mut self) {
        let _ = self.settings.save(&self.settings_path);
    }
}

fn metric(ui: &mut egui::Ui, label: &str, value: String) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new(value).size(24.0).strong());
        ui.label(RichText::new(label).weak());
    });
}

fn header(ui: &mut egui::Ui, names: &[&str]) {
    for name in names {
        ui.label(RichText::new(*name).strong());
    }
    ui.end_row();
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.text_edit_singleline(value);
    });
}
