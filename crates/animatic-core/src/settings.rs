//! Application settings (JSON file) and import of the legacy `local_settings.xml`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The rename script AniDB O'Matic used when none was configured (literal at 0x7d4078).
pub const DEFAULT_RENAME_PATTERN: &str = include_str!("rename/default_pattern.txt");

/// Default extension filter of the legacy client (literal at 0x7d3a28).
pub const DEFAULT_EXTENSIONS: &str = "7z,aac,ac3,ace,asf,ass,avi,dts,dtshd,flac,flv,idx,js,lrc,m2ts,m4a,mk3d,mka,mks,mkv,mov,mp3,mp4,mpeg,mpg,ogg,ogm,ogv,pjs,qt,ra,rar,rm,rmvb,rt,smi,smil,srt,ssa,sub,swf,thd,tmp,ts,tts,txt,wav,webm,wma,wmv,xss,zip";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StatFormat {
    #[default]
    Xml,
    Ini,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}

/// Mylist state values used by AniDB (`state=` parameter).
pub const MYLIST_STATES: [&str; 5] = ["Unknown", "On HDD", "On CD/DVD", "Deleted", "Remote/Shared"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnidbSettings {
    pub username: String,
    pub password: String,
    /// UDP API key (Profile → Settings → Account on anidb.net). Enables encrypted sessions.
    pub api_key: String,
    /// Registered client name (https://anidb.net/software/add); both APIs refuse unregistered clients.
    pub client_name: String,
    pub client_version: u32,
    pub udp_server: String,
    pub local_port: u16,
    pub image_server: String,
    /// Refresh cached AniDB data of known files when they are touched ("Auto update known file data").
    pub update_file_information: bool,
    /// Work offline: commands are queued until this is switched off.
    pub offline: bool,
    /// Log out after this many idle minutes.
    pub idle_logout_minutes: u32,
}

impl Default for AnidbSettings {
    fn default() -> Self {
        AnidbSettings {
            username: String::new(),
            password: String::new(),
            api_key: String::new(),
            client_name: String::new(),
            client_version: 1,
            udp_server: "api.anidb.net:9000".into(),
            local_port: 29_456,
            image_server: "cdn.anidb.net".into(),
            update_file_information: false,
            offline: false,
            idle_logout_minutes: 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Player {
    pub name: String,
    pub path: String,
    pub open: String,
    pub play: String,
    pub enqueue: String,
    pub default: bool,
}

/// Per-folder behaviour ("file manager" in the legacy client).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileManager {
    pub name: String,
    pub path: String,
    pub include_subdirs: bool,
    pub auto_hash: bool,
    pub auto_rehash: bool,
    pub auto_remove: bool,
    pub auto_rename: bool,
    /// Destination folder for renamed files ("" = keep the current folder).
    pub base_path: String,
    pub exclude_hidden: bool,
    pub exclude_system: bool,
    pub clear_empty_folders: bool,
    pub mylist_auto_add: bool,
    pub mylist_watched: bool,
    pub mylist_state: i64,
    pub mylist_storage: String,
    pub mylist_source: String,
    pub mylist_other: String,
    pub mylist_watched_check: bool,
    pub mylist_state_check: bool,
    pub mylist_storage_check: bool,
    pub mylist_source_check: bool,
    pub mylist_other_check: bool,
    /// User-added entries can be edited; the built-in "Default" and network entries cannot.
    pub can_edit_path: bool,
    pub default: bool,
}

impl Default for FileManager {
    fn default() -> Self {
        FileManager {
            name: String::new(),
            path: String::new(),
            include_subdirs: true,
            auto_hash: false,
            auto_rehash: false,
            auto_remove: false,
            auto_rename: false,
            base_path: String::new(),
            exclude_hidden: true,
            exclude_system: true,
            clear_empty_folders: false,
            mylist_auto_add: false,
            mylist_watched: false,
            mylist_state: 1,
            mylist_storage: String::new(),
            mylist_source: String::new(),
            mylist_other: String::new(),
            mylist_watched_check: false,
            mylist_state_check: true,
            mylist_storage_check: false,
            mylist_source_check: false,
            mylist_other_check: false,
            can_edit_path: true,
            default: false,
        }
    }
}

impl FileManager {
    pub fn builtin_default() -> FileManager {
        FileManager {
            name: "Default".into(),
            can_edit_path: false,
            default: true,
            include_subdirs: false,
            mylist_auto_add: true,
            mylist_state_check: true,
            mylist_watched_check: true,
            ..FileManager::default()
        }
    }

    pub fn network_default() -> FileManager {
        FileManager {
            name: "\\\\ (Default network path)".into(),
            path: "\\\\".into(),
            can_edit_path: false,
            include_subdirs: false,
            exclude_hidden: false,
            exclude_system: false,
            mylist_state_check: false,
            mylist_state: 0,
            ..FileManager::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileManagerSettings {
    /// Delay between two file manager actions in milliseconds.
    pub action_delay_ms: u64,
    /// Pause between two scans in milliseconds.
    pub sleep_time_ms: u64,
    pub managers: Vec<FileManager>,
}

impl Default for FileManagerSettings {
    fn default() -> Self {
        FileManagerSettings {
            action_delay_ms: 500,
            sleep_time_ms: 900_000,
            managers: vec![
                FileManager::builtin_default(),
                FileManager::network_default(),
            ],
        }
    }
}

impl FileManagerSettings {
    pub fn default_manager(&self) -> &FileManager {
        self.managers
            .iter()
            .find(|m| m.default)
            .or_else(|| self.managers.first())
            .expect("at least one file manager")
    }

    /// The manager whose path is the longest prefix of `path`; the default manager otherwise.
    /// A manager without `include_subdirs` only matches files directly inside its folder.
    pub fn manager_for(&self, path: &Path) -> &FileManager {
        self.matching_manager_for(path)
            .unwrap_or_else(|| self.default_manager())
    }

    /// Explicit path match, without the fallback used for non-destructive folder policy.
    pub(crate) fn matching_manager_for(&self, path: &Path) -> Option<&FileManager> {
        let p = normalize_path(&path.to_string_lossy());
        let parent = path
            .parent()
            .map(|d| normalize_path(&d.to_string_lossy()))
            .unwrap_or_default();
        self.managers
            .iter()
            .filter(|m| !m.path.is_empty())
            .filter(|m| {
                let mp = normalize_path(&m.path);
                let mp_dir = if mp.ends_with('/') {
                    mp.clone()
                } else {
                    format!("{mp}/")
                };
                if m.include_subdirs || mp == "//" {
                    p.starts_with(&mp_dir)
                } else {
                    format!("{parent}/") == mp_dir
                }
            })
            .max_by_key(|m| normalize_path(&m.path).len())
    }
}

/// Lowercase on Windows, forward slashes, no trailing separator (except roots).
pub fn normalize_path(p: &str) -> String {
    let mut s = p.replace('\\', "/");
    if cfg!(windows) {
        s = s.to_lowercase();
    }
    while s.len() > 1 && s.ends_with('/') && !s.ends_with("//") {
        s.pop();
    }
    s
}

/// Colour slots of the legacy client (tag numbers kept for import): odd = background, even = text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
pub enum ColorSlot {
    LocalRoot = 1,
    LocalFolder = 3,
    LocalFile = 5,
    LocalFileInMylist = 7,
    LocalFileUnknown = 9,
    LocalFileInvalidCrc = 11,
    Anime = 13,
    Episode = 15,
    EpisodeInMylist = 17,
    File = 19,
    FileCrcOk = 21,
    FileCrcInvalid = 23,
    FileInMylist = 25,
    LocalFileDuplicate = 27,
}

impl ColorSlot {
    pub const ALL: [ColorSlot; 14] = [
        ColorSlot::LocalRoot,
        ColorSlot::LocalFolder,
        ColorSlot::LocalFile,
        ColorSlot::LocalFileInMylist,
        ColorSlot::LocalFileUnknown,
        ColorSlot::LocalFileInvalidCrc,
        ColorSlot::LocalFileDuplicate,
        ColorSlot::Anime,
        ColorSlot::Episode,
        ColorSlot::EpisodeInMylist,
        ColorSlot::File,
        ColorSlot::FileCrcOk,
        ColorSlot::FileCrcInvalid,
        ColorSlot::FileInMylist,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ColorSlot::LocalRoot => "Hashed files: root",
            ColorSlot::LocalFolder => "Hashed files: folder",
            ColorSlot::LocalFile => "Hashed files: file",
            ColorSlot::LocalFileInMylist => "Hashed files: file in mylist",
            ColorSlot::LocalFileUnknown => "Hashed files: file unknown",
            ColorSlot::LocalFileInvalidCrc => "Hashed files: invalid CRC",
            ColorSlot::LocalFileDuplicate => "Hashed files: duplicate",
            ColorSlot::Anime => "AniDB: anime",
            ColorSlot::Episode => "AniDB: episode",
            ColorSlot::EpisodeInMylist => "AniDB: episode in mylist",
            ColorSlot::File => "AniDB: file",
            ColorSlot::FileCrcOk => "AniDB: file CRC ok",
            ColorSlot::FileCrcInvalid => "AniDB: file CRC invalid",
            ColorSlot::FileInMylist => "AniDB: file in mylist",
        }
    }

    /// Legacy default background as 0xRRGGBB.
    pub fn default_background(self) -> u32 {
        match self {
            ColorSlot::LocalRoot | ColorSlot::Anime => 0xC9C9C9,
            ColorSlot::LocalFolder | ColorSlot::Episode => 0xDFDFDF,
            ColorSlot::LocalFile | ColorSlot::File => 0xF4F4F4,
            ColorSlot::LocalFileInMylist | ColorSlot::FileInMylist => 0x70A0FF,
            ColorSlot::LocalFileUnknown | ColorSlot::FileCrcInvalid => 0xFF7070,
            ColorSlot::LocalFileInvalidCrc => 0xCB97FF,
            ColorSlot::LocalFileDuplicate => 0xFF8000,
            ColorSlot::EpisodeInMylist => 0x7070D0,
            ColorSlot::FileCrcOk => 0x80FF80,
        }
    }

    fn from_tag(tag: u32) -> Option<(ColorSlot, bool)> {
        let bg = tag % 2 == 1;
        let base = if bg { tag } else { tag.checked_sub(1)? };
        ColorSlot::ALL
            .iter()
            .copied()
            .find(|s| *s as u32 == base)
            .map(|s| (s, bg))
    }
}

/// Background/text override for a colour slot (0xRRGGBB).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotColor {
    pub background: u32,
    pub text: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ColumnState {
    pub id: String,
    pub width: f32,
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub theme: Theme,
    pub zoom: f32,
    pub window: Option<[f32; 4]>,
    pub maximized: bool,
    pub last_directory: String,
    /// Tint rows with the configured colours.
    pub colored_rows: bool,
    pub colors: BTreeMap<String, SlotColor>,
    /// Column layout per table id.
    pub columns: BTreeMap<String, Vec<ColumnState>>,
    pub tree_auto_expand: bool,
    pub tree_auto_collapse: bool,
    pub show_local_folder_size: bool,
    pub hide_root_folder: bool,
    pub copy_length_as_minutes: bool,
    pub show_tray_icon: bool,
    pub minimize_to_tray: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        UiSettings {
            theme: Theme::Dark,
            zoom: 1.0,
            window: None,
            maximized: false,
            last_directory: String::new(),
            colored_rows: true,
            colors: BTreeMap::new(),
            columns: BTreeMap::new(),
            tree_auto_expand: false,
            tree_auto_collapse: false,
            show_local_folder_size: true,
            hide_root_folder: false,
            copy_length_as_minutes: false,
            show_tray_icon: true,
            minimize_to_tray: true,
        }
    }
}

impl UiSettings {
    pub fn color(&self, slot: ColorSlot) -> SlotColor {
        self.colors
            .get(&format!("{slot:?}"))
            .copied()
            .unwrap_or(SlotColor {
                background: slot.default_background(),
                text: None,
            })
    }

    pub fn set_color(&mut self, slot: ColorSlot, c: SlotColor) {
        if c.background == slot.default_background() && c.text.is_none() {
            self.colors.remove(&format!("{slot:?}"));
        } else {
            self.colors.insert(format!("{slot:?}"), c);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HashingSettings {
    pub autostart: bool,
    /// 0 idle .. 6 time critical (legacy thread priority index).
    pub priority: u8,
    pub use_extensions: bool,
    pub extensions: Vec<String>,
    /// Hash export template (`%ed2k%`, `%fulled2k%` ...).
    pub export_template: String,
    pub integrate_into_shell: bool,
    /// Identify hashed files on AniDB and add them to mylist using the file manager rules.
    pub identify_after_hashing: bool,
}

impl Default for HashingSettings {
    fn default() -> Self {
        HashingSettings {
            autostart: true,
            priority: 1,
            use_extensions: false,
            extensions: DEFAULT_EXTENSIONS.split(',').map(str::to_string).collect(),
            export_template: "%fulled2k%".into(),
            integrate_into_shell: false,
            identify_after_hashing: true,
        }
    }
}

impl HashingSettings {
    pub fn accepts(&self, path: &Path) -> bool {
        if !self.use_extensions {
            return true;
        }
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        self.extensions
            .iter()
            .any(|e| e.trim().trim_start_matches('.').eq_ignore_ascii_case(&ext))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatWhoreSettings {
    pub enabled: bool,
    pub format: StatFormat,
    pub file: String,
}

impl Default for StatWhoreSettings {
    fn default() -> Self {
        StatWhoreSettings {
            enabled: false,
            format: StatFormat::Xml,
            file: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AvdumpSettings {
    pub enabled: bool,
    /// Path to the AVDump executable (AVDump3CL).
    pub executable: String,
    /// Arguments; `{path}`, `{user}`, `{key}` and `{port}` are substituted.
    pub arguments: String,
    /// UDP API key used by AVDump (legacy "AVDump password").
    pub key: String,
}

impl Default for AvdumpSettings {
    fn default() -> Self {
        AvdumpSettings {
            enabled: false,
            executable: String::new(),
            arguments:
                "--Consumers=ED2K --Auth={user}:{key} --LPort={port} --PrintEd2kLink \"{path}\""
                    .into(),
            key: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub anidb: AnidbSettings,
    pub hashing: HashingSettings,
    pub ui: UiSettings,
    pub players: Vec<Player>,
    pub file_managers: FileManagerSettings,
    pub renaming_pattern: String,
    pub stat_whore: StatWhoreSettings,
    pub avdump: AvdumpSettings,
    /// Replace spaces in ed2k link file names with this string (None = keep spaces).
    pub ed2k_space_replacement: Option<String>,
    /// True until the first-run assistant has been completed.
    pub first_launch: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            anidb: AnidbSettings::default(),
            hashing: HashingSettings::default(),
            ui: UiSettings::default(),
            players: Vec::new(),
            file_managers: FileManagerSettings::default(),
            renaming_pattern: DEFAULT_RENAME_PATTERN.into(),
            stat_whore: StatWhoreSettings::default(),
            avdump: AvdumpSettings::default(),
            ed2k_space_replacement: None,
            first_launch: true,
        }
    }
}

/// Outcome of [`Settings::load_with_recovery`].
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsLoad {
    pub settings: Settings,
    /// True when the primary file was missing or invalid and the settings come from the backup.
    /// Recovered settings are forced offline and no file has been modified; callers should keep
    /// the primary file untouched until the user explicitly saves.
    pub recovered_from_backup: bool,
}

/// State of one settings file on disk. Descriptions never contain file contents.
enum Slot {
    Missing,
    Valid(Box<Settings>),
    Invalid(String),
    Unreadable(String),
}

/// Location-only description of a JSON failure: serde messages can quote the offending value,
/// which may be a credential, so only the failure class and position are reported.
fn describe_json_error(e: &serde_json::Error) -> String {
    use serde_json::error::Category;
    let kind = match e.classify() {
        Category::Io => "read error",
        Category::Syntax => "invalid JSON syntax",
        Category::Data => "unexpected field type or value",
        Category::Eof => "truncated JSON",
    };
    format!("{kind} at line {} column {}", e.line(), e.column())
}

/// Settings document root: only a JSON object is accepted. Derived struct deserializers also
/// accept sequences (`[]` would silently become all defaults), which must never count as a
/// valid settings file for loading or for deciding whether a primary may replace the backup.
struct RootObject(Settings);

impl<'de> Deserialize<'de> for RootObject {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = RootObject;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a settings object")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> std::result::Result<RootObject, A::Error> {
                Settings::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    .map(RootObject)
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

fn parse_settings(bytes: &[u8]) -> std::result::Result<Settings, String> {
    match serde_json::from_slice::<RootObject>(bytes) {
        Ok(RootObject(mut s)) => {
            s.fixup();
            Ok(s)
        }
        Err(e) => Err(describe_json_error(&e)),
    }
}

fn read_slot(path: &Path) -> Slot {
    match std::fs::read(path) {
        Ok(bytes) => match parse_settings(&bytes) {
            Ok(s) => Slot::Valid(Box::new(s)),
            Err(d) => Slot::Invalid(d),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Slot::Missing,
        Err(e) => Slot::Unreadable(e.to_string()),
    }
}

impl Settings {
    /// Location of the backup written by [`Settings::save`] next to the primary file.
    pub fn backup_path(path: &Path) -> std::path::PathBuf {
        path.with_extension("json.bak")
    }

    /// Load the primary file; when it is missing or invalid, fall back to a valid backup.
    ///
    /// Nothing on disk is modified. A recovered result is forced offline so a possibly stale
    /// configuration cannot start talking to AniDB before the user has reviewed it. Unreadable
    /// files (permissions, directories in the way) are errors, never silent resets. With neither
    /// file present the defaults are returned. Error messages name both paths but never include
    /// file contents.
    pub fn load_with_recovery(path: &Path) -> Result<SettingsLoad> {
        let primary = match read_slot(path) {
            Slot::Valid(settings) => {
                return Ok(SettingsLoad {
                    settings: *settings,
                    recovered_from_backup: false,
                });
            }
            Slot::Unreadable(e) => {
                return Err(Error::Settings(format!(
                    "cannot read settings file {}: {e}; the file was not modified",
                    path.display()
                )));
            }
            other => other,
        };
        let backup_path = Self::backup_path(path);
        let backup = read_slot(&backup_path);
        match (primary, backup) {
            (_, Slot::Valid(mut settings)) => {
                settings.anidb.offline = true;
                Ok(SettingsLoad {
                    settings: *settings,
                    recovered_from_backup: true,
                })
            }
            (Slot::Missing, Slot::Missing) => Ok(SettingsLoad {
                settings: Settings::default(),
                recovered_from_backup: false,
            }),
            (primary, backup) => {
                let primary_text = match primary {
                    Slot::Invalid(d) => format!("is not valid ({d})"),
                    _ => "does not exist".to_string(),
                };
                let backup_text = match backup {
                    Slot::Missing => "does not exist".to_string(),
                    Slot::Invalid(d) => format!("is not valid ({d})"),
                    Slot::Unreadable(e) => format!("cannot be read ({e})"),
                    Slot::Valid(_) => "is valid".to_string(),
                };
                Err(Error::Settings(format!(
                    "settings file {} {primary_text} and backup {} {backup_text}; \
                     neither file was modified. Repair or remove them to start with default settings",
                    path.display(),
                    backup_path.display()
                )))
            }
        }
    }

    /// Write the settings atomically (temp file + rename).
    ///
    /// An existing primary file that still parses is copied to the backup first; an invalid
    /// primary never replaces the backup. If writing or renaming fails the original primary stays
    /// in place.
    pub fn save(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        let written = std::fs::File::create(&tmp).and_then(|mut f| {
            f.write_all(json.as_bytes())?;
            f.sync_all()
        });
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        if let Ok(previous) = std::fs::read(path)
            && parse_settings(&previous).is_ok()
        {
            let backup = Self::backup_path(path);
            let backup_tmp = path.with_extension("json.bak.tmp");
            let copied = std::fs::write(&backup_tmp, &previous)
                .and_then(|()| std::fs::rename(&backup_tmp, &backup));
            if copied.is_err() {
                let _ = std::fs::remove_file(&backup_tmp);
            }
        }
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    fn fixup(&mut self) {
        if !self.file_managers.managers.iter().any(|m| m.default) {
            self.file_managers
                .managers
                .insert(0, FileManager::builtin_default());
        }
        self.file_managers.action_delay_ms = self.file_managers.action_delay_ms.max(1);
        self.file_managers.sleep_time_ms = self.file_managers.sleep_time_ms.max(60_000);
        if self.ui.zoom <= 0.3 {
            self.ui.zoom = 1.0;
        }
    }

    pub fn default_player(&self) -> Option<&Player> {
        self.players
            .iter()
            .find(|p| p.default)
            .or_else(|| self.players.first())
    }
}

// ---------------------------------------------------------------------------------------------
// Legacy local_settings.xml
// ---------------------------------------------------------------------------------------------

/// Flat view of a legacy settings document: "section/element" -> text, plus repeated records.
#[derive(Debug, Default)]
struct LegacyXml {
    values: BTreeMap<String, String>,
    players: Vec<BTreeMap<String, String>>,
    managers: Vec<BTreeMap<String, String>>,
    colors: Vec<(u32, i64)>,
}

fn decode_legacy_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xff, 0xfe]) || (bytes.len() > 1 && bytes[1] == 0 && bytes[0] != 0) {
        let start = if bytes.starts_with(&[0xff, 0xfe]) {
            2
        } else {
            0
        };
        let units: Vec<u16> = bytes[start..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes))
            .into_owned()
    }
}

fn parse_legacy_xml(text: &str) -> Result<LegacyXml> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;
    // The XML declaration claims UTF-16 but we already decoded it.
    let text = text.trim_start_matches('\u{feff}');
    let body = match text.find("?>") {
        Some(i) if text.starts_with("<?xml") => &text[i + 2..],
        _ => text,
    };
    let mut r = Reader::from_str(body);
    r.config_mut().trim_text(false);
    let mut out = LegacyXml::default();
    let mut path: Vec<String> = Vec::new();
    let mut text_buf = String::new();
    let mut record: Option<BTreeMap<String, String>> = None;
    let attr = |e: &quick_xml::events::BytesStart, name: &str| -> Option<String> {
        e.attributes()
            .flatten()
            .find(|a| a.key.0 == name)
            .and_then(|a| {
                a.normalized_value(quick_xml::XmlVersion::default())
                    .ok()
                    .map(|v| v.into_owned())
            })
    };
    loop {
        match r.read_event().map_err(|e| Error::Xml(e.to_string()))? {
            Event::Start(e) => {
                let name = e.local_name().as_ref().to_string();
                if name == "player" || name == "manager" {
                    record = Some(BTreeMap::new());
                }
                path.push(name);
                text_buf.clear();
            }
            Event::Empty(e) => {
                if e.local_name().as_ref() == "color"
                    && let (Some(tag), Some(value)) = (attr(&e, "tag"), attr(&e, "value"))
                    && let (Ok(t), Ok(v)) = (tag.parse(), value.parse())
                {
                    out.colors.push((t, v));
                }
            }
            Event::Text(t) => text_buf.push_str(&t),
            Event::GeneralRef(g) => {
                let s = match &*g {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "quot" => "\"",
                    "apos" => "'",
                    _ => "",
                };
                if let Ok(Some(c)) = g.resolve_char_ref() {
                    text_buf.push(c);
                } else {
                    text_buf.push_str(s);
                }
            }
            Event::CData(t) => text_buf.push_str(&t),
            Event::End(_) => {
                let name = path.pop().unwrap_or_default();
                let value = std::mem::take(&mut text_buf);
                match name.as_str() {
                    "player" => out.players.extend(record.take()),
                    "manager" => out.managers.extend(record.take()),
                    _ => {
                        if let Some(rec) = record.as_mut() {
                            rec.insert(name, value);
                        } else if path.len() >= 2 {
                            out.values
                                .insert(format!("{}/{}", path[path.len() - 1], name), value);
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

fn b(v: Option<&String>, default: bool) -> bool {
    v.map(|s| s.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(default)
}

fn tcolor_to_rgb(v: i64) -> u32 {
    let v = v as u32;
    ((v & 0xff) << 16) | (v & 0xff00) | ((v >> 16) & 0xff)
}

/// Import supported settings from a legacy `local_settings.xml` on top of the defaults.
/// XML without recognized settings is rejected rather than replacing a configuration with defaults.
pub fn import_legacy_settings(path: &Path) -> Result<Settings> {
    let bytes = std::fs::read(path)?;
    let x = parse_legacy_xml(&decode_legacy_text(&bytes))?;
    // Track fields actually consumed below without duplicating the import schema in an allowlist.
    let recognized = std::cell::Cell::new(false);
    let g = |k: &str| {
        let value = x.values.get(k);
        if value.is_some() {
            recognized.set(true);
        }
        value
    };
    let mut s = Settings::default();

    if let Some(v) = g("anidb/username") {
        s.anidb.username = v.clone();
    }
    if let Some(v) = g("anidb/password") {
        s.anidb.password = v.clone();
    }
    if let Some(v) = g("anidb/imageServer").filter(|v| !v.is_empty()) {
        s.anidb.image_server = v.clone();
    }
    s.anidb.update_file_information = b(g("anidb/updateFileInformation"), false);
    s.first_launch = s.anidb.username.is_empty();

    s.hashing.autostart = b(g("general/autostartHashing"), false);
    if let Some(p) = g("general/hashingThreadPriority").and_then(|v| v.trim().parse::<u8>().ok()) {
        s.hashing.priority = p.min(6);
    }
    s.hashing.integrate_into_shell = b(g("general/integrateIntoShell"), false);
    s.hashing.use_extensions = b(g("general/useExtensions"), false);
    if let Some(v) = g("general/extensions") {
        s.hashing.extensions = v
            .split(',')
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty())
            .collect();
    }
    if let Some(v) = g("general/exportTemplate").filter(|v| !v.is_empty()) {
        s.hashing.export_template = v.replace("\r\n", "\n");
    }
    if let Some(v) = g("general/renamingPattern").filter(|v| !v.trim().is_empty()) {
        s.renaming_pattern = v.replace("\r\n", "\n");
    }
    s.ui.show_local_folder_size = b(g("general/showLocalFolderSize"), true);
    s.ui.hide_root_folder = b(g("general/hideRootFolder"), false);
    s.ui.copy_length_as_minutes = b(g("general/copyLengthAsMinutes"), false);
    s.ui.tree_auto_expand = b(g("general/treeAutoExpand"), false);
    s.ui.tree_auto_collapse = b(g("general/treeAutoCollapse"), false);
    s.ui.show_tray_icon = b(g("general/showTrayIcon"), true);
    s.ui.minimize_to_tray = b(g("general/minimizeToTray"), true);
    s.stat_whore.enabled = b(g("general/statWhore"), false);
    s.stat_whore.format =
        if g("general/statWhoreFormat").is_some_and(|v| v.eq_ignore_ascii_case("INI")) {
            StatFormat::Ini
        } else {
            StatFormat::Xml
        };
    if let Some(v) = g("general/statWhoreFileName") {
        s.stat_whore.file = v.clone();
    }
    s.avdump.enabled = b(g("general/useAVDump"), false);
    if let Some(v) = g("general/avdumpPassword") {
        s.avdump.key = v.clone();
    }
    if let Some(v) = g("ui/lastDirectory") {
        s.ui.last_directory = v.clone();
    }
    s.ui.maximized = b(g("ui/mainMaximized"), false);
    let num = |k: &str| g(k).and_then(|v| v.trim().parse::<i64>().ok());
    if let (Some(l), Some(t), Some(w), Some(h)) = (
        num("ui/mainLeft"),
        num("ui/mainTop"),
        num("ui/mainWidth"),
        num("ui/mainHeight"),
    ) && l > -100_000
        && t > -100_000
        && w > 200
        && h > 200
    {
        s.ui.window = Some([l as f32, t as f32, w as f32, h as f32]);
    }
    for (tag, value) in &x.colors {
        if let Some((slot, bg)) = ColorSlot::from_tag(*tag) {
            recognized.set(true);
            let mut c = s.ui.color(slot);
            if bg {
                c.background = tcolor_to_rgb(*value);
            } else if *value != 0 {
                c.text = Some(tcolor_to_rgb(*value));
            }
            s.ui.set_color(slot, c);
        }
    }

    s.players = x
        .players
        .iter()
        .filter(|p| p.get("name").is_some_and(|n| !n.is_empty()))
        .map(|p| Player {
            name: p["name"].clone(),
            path: p.get("path").cloned().unwrap_or_default(),
            open: p.get("open").cloned().unwrap_or_default(),
            play: p.get("play").cloned().unwrap_or_default(),
            enqueue: p.get("enqueue").cloned().unwrap_or_default(),
            default: b(p.get("default"), false),
        })
        .collect();
    if !s.players.is_empty() {
        recognized.set(true);
    }

    if let Some(v) = num("filemanager/actionDelay") {
        s.file_managers.action_delay_ms = v.max(1) as u64;
    }
    if let Some(v) = num("filemanager/sleepTime") {
        s.file_managers.sleep_time_ms = v.max(60_000) as u64;
    }
    let managers: Vec<FileManager> = x
        .managers
        .iter()
        .filter(|m| m.get("name").is_some_and(|n| !n.is_empty()))
        .map(|m| FileManager {
            name: m["name"].clone(),
            path: m.get("path").cloned().unwrap_or_default(),
            include_subdirs: b(m.get("includesubdirs"), true),
            auto_hash: b(m.get("autohash"), false),
            auto_rehash: b(m.get("autorehash"), false),
            auto_remove: b(m.get("autoremove"), false),
            auto_rename: b(m.get("autorename"), false),
            base_path: m.get("basepath").cloned().unwrap_or_default(),
            exclude_hidden: b(m.get("excludehiddenfiles"), false),
            exclude_system: b(m.get("excludesystemfiles"), false),
            clear_empty_folders: b(m.get("clearemptyfolders"), false),
            mylist_auto_add: b(m.get("mylistautoadd"), false),
            mylist_watched: b(m.get("mylistwatched"), false),
            mylist_state: m
                .get("myliststate")
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0),
            mylist_storage: m.get("myliststorage").cloned().unwrap_or_default(),
            mylist_source: m.get("mylistsource").cloned().unwrap_or_default(),
            mylist_other: m.get("mylistother").cloned().unwrap_or_default(),
            mylist_watched_check: b(m.get("mylistwatchedcheck"), false),
            mylist_state_check: b(m.get("myliststatecheck"), false),
            mylist_storage_check: b(m.get("myliststoragecheck"), false),
            mylist_source_check: b(m.get("mylistsourcecheck"), false),
            mylist_other_check: b(m.get("mylistothercheck"), false),
            can_edit_path: b(m.get("caneditpath"), true),
            default: b(m.get("default"), false),
        })
        .collect();
    if !managers.is_empty() {
        recognized.set(true);
        s.file_managers.managers = managers;
    }
    if !recognized.get() {
        return Err(Error::Xml(
            "No supported AOM settings found in the selected XML".into(),
        ));
    }
    s.fixup();
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16_file(xml: &str) -> std::path::PathBuf {
        let mut bytes = vec![0xff, 0xfe];
        for u in xml.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let p = std::env::temp_dir().join(format!(
            "animatic-settings-{}-{}.xml",
            std::process::id(),
            xml.len()
        ));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn imports_legacy_utf16_settings() {
        let xml = r#"<?xml version="1.0" encoding="UTF-16"?>
<aomsettings>
	<general>
		<minimizeToTray>false</minimizeToTray>
		<renamingPattern>set('x', if(F.Version&gt;1, 'v', ''))
x</renamingPattern>
		<autostartHashing>true</autostartHashing>
		<hashingThreadPriority>4</hashingThreadPriority>
		<useExtensions>true</useExtensions>
		<extensions>avi,mkv,mov,mp4</extensions>
	</general>
	<ui><colors><color tag="7" value="16711680"/></colors><lastDirectory>Z:\AnimaticTestMedia\</lastDirectory></ui>
	<anidb><username>animatic-test-user</username><password>animatic-test-password</password><imageServer>cdn.anidb.net</imageServer></anidb>
	<players><player><name>mpv</name><default>true</default><path>Z:\Tools\mpv.exe</path><open>"%L"</open></player></players>
	<filemanager><sleepTime>450000</sleepTime>
		<manager><name>Default</name><path></path><autorename>true</autorename><mylistautoadd>true</mylistautoadd><myliststate>1</myliststate><caneditpath>false</caneditpath><default>true</default></manager>
		<manager><name>Test media</name><path>Z:\AnimaticTestMedia</path><includesubdirs>true</includesubdirs><autohash>true</autohash><default>false</default></manager>
	</filemanager>
</aomsettings>"#;
        let p = utf16_file(xml);
        let s = import_legacy_settings(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        assert_eq!(s.anidb.username, "animatic-test-user");
        assert!(!s.first_launch);
        assert!(!s.ui.minimize_to_tray);
        assert_eq!(s.renaming_pattern, "set('x', if(F.Version>1, 'v', ''))\nx");
        assert_eq!(s.hashing.priority, 4);
        assert_eq!(s.hashing.extensions, vec!["avi", "mkv", "mov", "mp4"]);
        assert!(s.hashing.accepts(Path::new("x/Y.MKV")));
        assert!(!s.hashing.accepts(Path::new("x/y.srt")));
        assert_eq!(
            s.ui.color(ColorSlot::LocalFileInMylist).background,
            0x0000FF,
            "TColor is BGR"
        );
        assert_eq!(s.ui.last_directory, "Z:\\AnimaticTestMedia\\");
        assert_eq!(s.players[0].open, "\"%L\"");
        assert!(s.players[0].default);
        assert_eq!(s.file_managers.sleep_time_ms, 450_000);
        assert_eq!(s.file_managers.managers.len(), 2);
        assert!(s.file_managers.default_manager().auto_rename);
    }

    #[test]
    fn rejects_unrelated_or_empty_legacy_settings() {
        for xml in [
            "",
            "<catalog><entry>synthetic media</entry></catalog>",
            "<aomsettings><general><unsupported>true</unsupported></general></aomsettings>",
        ] {
            let path = utf16_file(xml);
            let result = import_legacy_settings(&path);
            std::fs::remove_file(path).unwrap();
            assert!(result.is_err());
        }
    }

    #[test]
    fn accepts_recognized_legacy_settings_with_default_values() {
        let path = utf16_file(
            "<aomsettings><general><autostartHashing>false</autostartHashing></general></aomsettings>",
        );
        let settings = import_legacy_settings(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(!settings.hashing.autostart);
    }

    #[test]
    fn manager_selection_prefers_longest_matching_path() {
        let mut fm = FileManagerSettings::default();
        fm.managers.push(FileManager {
            name: "a".into(),
            path: "/media/anime".into(),
            ..FileManager::default()
        });
        fm.managers.push(FileManager {
            name: "b".into(),
            path: "/media/anime/new".into(),
            include_subdirs: false,
            ..FileManager::default()
        });
        assert_eq!(fm.manager_for(Path::new("/media/anime/x/y.mkv")).name, "a");
        assert_eq!(
            fm.manager_for(Path::new("/media/anime/new/y.mkv")).name,
            "b"
        );
        assert_eq!(
            fm.manager_for(Path::new("/media/anime/new/sub/y.mkv")).name,
            "a",
            "b excludes subdirectories"
        );
        assert_eq!(fm.manager_for(Path::new("/other/y.mkv")).name, "Default");
    }

    struct TestDir(std::path::PathBuf);

    impl TestDir {
        fn new(label: &str) -> TestDir {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "animatic-settings-recovery-{}-{}-{label}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TestDir(dir)
        }

        fn primary(&self) -> std::path::PathBuf {
            self.0.join("settings.json")
        }

        fn backup(&self) -> std::path::PathBuf {
            Settings::backup_path(&self.primary())
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const CUSTOM_PATTERN: &str = "set('x', F.Version)\n'custom ' + x";

    fn custom_json(user: &str, offline: bool) -> String {
        let mut s = Settings::default();
        s.anidb.username = user.into();
        s.anidb.offline = offline;
        s.renaming_pattern = CUSTOM_PATTERN.into();
        serde_json::to_string_pretty(&s).unwrap()
    }

    fn error_text(result: Result<SettingsLoad>) -> String {
        result.expect_err("load must fail").to_string()
    }

    #[test]
    fn valid_primary_loads_without_recovery() {
        let dir = TestDir::new("valid");
        std::fs::write(dir.primary(), custom_json("primary-user", false)).unwrap();
        std::fs::write(dir.backup(), custom_json("backup-user", false)).unwrap();
        let loaded = Settings::load_with_recovery(&dir.primary()).unwrap();
        assert!(!loaded.recovered_from_backup);
        assert_eq!(loaded.settings.anidb.username, "primary-user");
        assert!(!loaded.settings.anidb.offline);
        assert_eq!(
            loaded,
            Settings::load_with_recovery(&dir.primary()).unwrap()
        );
    }

    #[test]
    fn corrupted_primary_recovers_offline_from_backup_without_touching_files() {
        let dir = TestDir::new("corrupt");
        let corrupt = "{ this is not json";
        let backup = custom_json("backup-user", false);
        std::fs::write(dir.primary(), corrupt).unwrap();
        std::fs::write(dir.backup(), &backup).unwrap();
        let loaded = Settings::load_with_recovery(&dir.primary()).unwrap();
        assert!(loaded.recovered_from_backup);
        assert!(loaded.settings.anidb.offline, "recovery must force offline");
        assert_eq!(loaded.settings.anidb.username, "backup-user");
        assert_eq!(loaded.settings.renaming_pattern, CUSTOM_PATTERN);
        assert_eq!(std::fs::read_to_string(dir.primary()).unwrap(), corrupt);
        assert_eq!(std::fs::read_to_string(dir.backup()).unwrap(), backup);
        assert_eq!(
            loaded,
            Settings::load_with_recovery(&dir.primary()).unwrap()
        );
    }

    #[test]
    fn truncated_primary_recovers_from_backup() {
        let dir = TestDir::new("truncated");
        let full = custom_json("primary-user", false);
        let truncated = &full[..full.len() / 2];
        std::fs::write(dir.primary(), truncated).unwrap();
        std::fs::write(dir.backup(), custom_json("backup-user", false)).unwrap();
        let loaded = Settings::load_with_recovery(&dir.primary()).unwrap();
        assert!(loaded.recovered_from_backup);
        assert_eq!(loaded.settings.anidb.username, "backup-user");
        assert_eq!(std::fs::read_to_string(dir.primary()).unwrap(), truncated);
    }

    #[test]
    fn empty_primary_is_not_treated_as_valid() {
        let dir = TestDir::new("empty");
        std::fs::write(dir.primary(), "").unwrap();
        std::fs::write(dir.backup(), custom_json("backup-user", false)).unwrap();
        assert!(
            Settings::load_with_recovery(&dir.primary())
                .unwrap()
                .recovered_from_backup
        );
    }

    #[test]
    fn missing_primary_recovers_from_valid_backup() {
        let dir = TestDir::new("missing-primary");
        std::fs::write(dir.backup(), custom_json("backup-user", false)).unwrap();
        let loaded = Settings::load_with_recovery(&dir.primary()).unwrap();
        assert!(loaded.recovered_from_backup);
        assert!(loaded.settings.anidb.offline);
        assert_eq!(loaded.settings.anidb.username, "backup-user");
        assert!(
            !dir.primary().exists(),
            "load must not create the primary file"
        );
    }

    #[test]
    fn missing_primary_and_backup_yield_defaults_without_creating_files() {
        let dir = TestDir::new("missing-both");
        let loaded = Settings::load_with_recovery(&dir.primary()).unwrap();
        assert!(!loaded.recovered_from_backup);
        assert_eq!(loaded.settings, Settings::default());
        assert!(!dir.primary().exists());
        assert!(!dir.backup().exists());
    }

    #[test]
    fn corrupted_primary_without_backup_is_an_error() {
        let dir = TestDir::new("no-backup");
        std::fs::write(dir.primary(), "{").unwrap();
        let text = error_text(Settings::load_with_recovery(&dir.primary()));
        assert!(text.contains(&dir.primary().display().to_string()));
        assert!(text.contains(&dir.backup().display().to_string()));
        assert_eq!(std::fs::read_to_string(dir.primary()).unwrap(), "{");
    }

    #[test]
    fn missing_primary_with_invalid_backup_is_an_error() {
        let dir = TestDir::new("missing-primary-bad-backup");
        std::fs::write(dir.backup(), "[]").unwrap();
        let text = error_text(Settings::load_with_recovery(&dir.primary()));
        assert!(text.contains(&dir.backup().display().to_string()));
        assert_eq!(std::fs::read_to_string(dir.backup()).unwrap(), "[]");
    }

    #[test]
    fn both_files_bad_is_actionable_error_without_contents() {
        let dir = TestDir::new("both-bad");
        // Syntax error and field-type error; serde would quote the secret in the latter.
        let primary = r#"{"anidb":{"password":"secret-primary-value"#;
        let backup = r#"{"first_launch":"secret-backup-value"}"#;
        std::fs::write(dir.primary(), primary).unwrap();
        std::fs::write(dir.backup(), backup).unwrap();
        let text = error_text(Settings::load_with_recovery(&dir.primary()));
        assert!(text.contains(&dir.primary().display().to_string()));
        assert!(text.contains(&dir.backup().display().to_string()));
        assert!(!text.contains("secret-primary-value"));
        assert!(!text.contains("secret-backup-value"));
        assert_eq!(std::fs::read_to_string(dir.primary()).unwrap(), primary);
        assert_eq!(std::fs::read_to_string(dir.backup()).unwrap(), backup);
        // The failure is repeatable instead of resetting to defaults.
        assert!(Settings::load_with_recovery(&dir.primary()).is_err());
    }

    #[test]
    fn unreadable_primary_is_not_silently_reset_or_replaced_by_backup() {
        let dir = TestDir::new("unreadable");
        // A directory at the primary path cannot be read as a file.
        std::fs::create_dir(dir.primary()).unwrap();
        std::fs::write(dir.backup(), custom_json("backup-user", false)).unwrap();
        let text = error_text(Settings::load_with_recovery(&dir.primary()));
        assert!(text.contains(&dir.primary().display().to_string()));
        assert!(dir.primary().is_dir());
    }

    #[test]
    fn save_with_invalid_primary_keeps_good_backup() {
        let dir = TestDir::new("save-invalid-primary");
        let backup = custom_json("backup-user", false);
        std::fs::write(dir.primary(), "{ truncated").unwrap();
        std::fs::write(dir.backup(), &backup).unwrap();
        let mut recovered = Settings::load_with_recovery(&dir.primary())
            .unwrap()
            .settings;
        recovered.anidb.username = "edited-user".into();
        recovered.save(&dir.primary()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.backup()).unwrap(),
            backup,
            "invalid primary must not replace the backup"
        );
        let reloaded = Settings::load_with_recovery(&dir.primary()).unwrap();
        assert!(!reloaded.recovered_from_backup);
        assert_eq!(reloaded.settings.anidb.username, "edited-user");
        assert_eq!(reloaded.settings.renaming_pattern, CUSTOM_PATTERN);
        assert!(!dir.primary().with_extension("json.tmp").exists());
    }

    #[test]
    fn save_with_valid_primary_updates_backup() {
        let dir = TestDir::new("save-valid-primary");
        let previous = custom_json("old-user", false);
        std::fs::write(dir.primary(), &previous).unwrap();
        std::fs::write(dir.backup(), custom_json("stale-user", false)).unwrap();
        let mut s = Settings::load_with_recovery(&dir.primary())
            .unwrap()
            .settings;
        s.anidb.username = "new-user".into();
        s.save(&dir.primary()).unwrap();
        assert_eq!(std::fs::read_to_string(dir.backup()).unwrap(), previous);
        assert_eq!(
            Settings::load_with_recovery(&dir.primary())
                .unwrap()
                .settings
                .anidb
                .username,
            "new-user"
        );
    }

    #[test]
    fn failed_save_preserves_original_primary_and_backup() {
        let dir = TestDir::new("save-failure");
        let primary = custom_json("primary-user", false);
        let backup = custom_json("backup-user", false);
        std::fs::write(dir.primary(), &primary).unwrap();
        std::fs::write(dir.backup(), &backup).unwrap();
        // A directory occupying the temp path makes the write step fail.
        std::fs::create_dir(dir.primary().with_extension("json.tmp")).unwrap();
        assert!(Settings::default().save(&dir.primary()).is_err());
        assert_eq!(std::fs::read_to_string(dir.primary()).unwrap(), primary);
        assert_eq!(std::fs::read_to_string(dir.backup()).unwrap(), backup);
    }

    #[test]
    fn custom_rename_pattern_survives_recovery_and_save_round_trip() {
        let dir = TestDir::new("custom-pattern");
        std::fs::write(dir.primary(), "").unwrap();
        std::fs::write(dir.backup(), custom_json("backup-user", false)).unwrap();
        let recovered = Settings::load_with_recovery(&dir.primary())
            .unwrap()
            .settings;
        assert_eq!(recovered.renaming_pattern, CUSTOM_PATTERN);
        recovered.save(&dir.primary()).unwrap();
        let reloaded = Settings::load_with_recovery(&dir.primary())
            .unwrap()
            .settings;
        assert_eq!(reloaded.renaming_pattern, CUSTOM_PATTERN);
        assert_ne!(reloaded.renaming_pattern, DEFAULT_RENAME_PATTERN);
    }

    #[test]
    fn non_object_documents_are_invalid_for_load_and_backup_rotation() {
        for document in ["[]", "[1,2]", "null", "42", "\"text\"", "true"] {
            let dir = TestDir::new("non-object");
            let backup = custom_json("backup-user", false);
            std::fs::write(dir.primary(), document).unwrap();
            std::fs::write(dir.backup(), &backup).unwrap();
            let loaded = Settings::load_with_recovery(&dir.primary()).unwrap();
            assert!(loaded.recovered_from_backup, "{document}");
            assert_eq!(loaded.settings.anidb.username, "backup-user", "{document}");
            Settings::default().save(&dir.primary()).unwrap();
            assert_eq!(
                std::fs::read_to_string(dir.backup()).unwrap(),
                backup,
                "{document} must not replace the backup"
            );
        }
    }
}
