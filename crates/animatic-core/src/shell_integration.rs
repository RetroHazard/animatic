//! Per-user "Hash with Animatic" entry in the operating system's file menu.
//!
//! The entry is created only by an explicit Options save (never by an AOM settings import) and
//! needs no elevation:
//!
//! * **Windows** — `HKEY_CURRENT_USER\Software\Classes\*\shell\Animatic.Hash` with a `command`
//!   subkey running `"<exe>" "%1"`.
//! * **Linux and other Unix** — `animatic-hash.desktop` in `$XDG_DATA_HOME/applications` (default
//!   `~/.local/share/applications`) with `Exec=<exe> %F`, `NoDisplay=true` and the common video
//!   and audio MIME types, so it appears in "Open With". `update-desktop-database` is run on a
//!   best-effort basis.
//!
//! Every entry carries an ownership marker ([`MARKER_NAME`] in the registry, the
//! [`DESKTOP_MARKER`] line in the desktop file). [`ShellIntegration::unregister`] and
//! [`ShellIntegration::register`] never change an entry without the marker, even when it has
//! the same name, and report it as [`Status::Foreign`].
//!
//! The location is injectable (`ShellIntegration::registry` on Windows,
//! [`ShellIntegration::desktop_entry`])
//! so the tests use a temporary registry subkey or directory, never the real shell
//! configuration.

use std::io;
use std::path::{Path, PathBuf};

/// Registry key name and desktop file stem of the entry.
pub const VERB: &str = "Animatic.Hash";
/// Text shown in the file menu.
pub const LABEL: &str = "Hash with Animatic";
/// Registry value that marks a key as created by Animatic.
pub const MARKER_NAME: &str = "AnimaticManaged";
/// Line that marks a desktop file as created by Animatic.
pub const DESKTOP_MARKER: &str = "X-Animatic-Managed=true";
/// File name of the Linux entry.
pub const DESKTOP_FILE: &str = "animatic-hash.desktop";

/// MIME types the desktop entry is offered for.
const MIME_TYPES: &[&str] = &[
    "video/x-matroska",
    "video/mp4",
    "video/x-msvideo",
    "video/mpeg",
    "video/webm",
    "video/quicktime",
    "video/x-flv",
    "video/ogg",
    "video/x-ms-wmv",
    "video/mp2t",
    "audio/mpeg",
    "audio/flac",
    "audio/x-flac",
    "audio/ogg",
    "audio/mp4",
    "audio/x-wav",
    "audio/aac",
    "audio/opus",
    "audio/x-matroska",
];

/// What is actually registered, as seen by [`ShellIntegration::status`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// No entry exists.
    NotRegistered,
    /// Animatic's entry exists and starts the given executable.
    Registered,
    /// Animatic's entry exists but starts something else (another copy, or the program was
    /// moved). `command` is the stored command text.
    Elsewhere { command: String },
    /// An entry with the same name exists that Animatic did not create. It is never changed.
    Foreign,
}

/// Result of [`ShellIntegration::unregister`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// Animatic's entry was removed.
    Removed,
    /// There was nothing to remove.
    NotRegistered,
    /// An entry with the same name exists that Animatic did not create; it was left alone.
    LeftForeign,
}

#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("shell integration is not available: {0}")]
    Unavailable(String),
    #[error(
        "an entry named {0} already exists that Animatic did not create; it was left unchanged"
    )]
    Foreign(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// The platform's registration target. See the module documentation.
pub struct ShellIntegration {
    backend: Result<Backend, String>,
}

enum Backend {
    #[cfg(windows)]
    Registry(registry::Registry),
    Desktop(desktop::Desktop),
}

impl ShellIntegration {
    /// The real per-user location of this platform.
    pub fn system() -> Self {
        #[cfg(windows)]
        let backend = Ok(Backend::Registry(registry::Registry::new(
            r"Software\Classes",
        )));
        #[cfg(all(unix, not(windows)))]
        let backend = directories::BaseDirs::new()
            .map(|dirs| {
                Backend::Desktop(desktop::Desktop::new(
                    dirs.data_dir().join("applications"),
                    true,
                ))
            })
            .ok_or_else(|| "the user's data directory could not be determined".to_string());
        #[cfg(not(any(windows, unix)))]
        let backend = Err("this platform has no supported file-menu integration".to_string());
        Self { backend }
    }

    /// Windows registry entry below `base` (a path under `HKEY_CURRENT_USER`, normally
    /// `Software\Classes`). Tests pass a temporary subkey.
    #[cfg(windows)]
    pub fn registry(base: &str) -> Self {
        Self {
            backend: Ok(Backend::Registry(registry::Registry::new(base))),
        }
    }

    /// Desktop-file entry in `dir`. `refresh` runs `update-desktop-database` afterwards;
    /// tests pass `false`.
    pub fn desktop_entry(dir: impl Into<PathBuf>, refresh: bool) -> Self {
        Self {
            backend: Ok(Backend::Desktop(desktop::Desktop::new(dir.into(), refresh))),
        }
    }

    /// Why the platform cannot register an entry, if it cannot.
    pub fn unavailable(&self) -> Option<&str> {
        self.backend.as_ref().err().map(String::as_str)
    }

    /// Human-readable location of the entry (registry key or desktop file).
    pub fn location(&self) -> String {
        match &self.backend {
            #[cfg(windows)]
            Ok(Backend::Registry(r)) => r.location(),
            Ok(Backend::Desktop(d)) => d.file().display().to_string(),
            Err(reason) => reason.clone(),
        }
    }

    fn backend(&self) -> Result<&Backend, ShellError> {
        self.backend
            .as_ref()
            .map_err(|reason| ShellError::Unavailable(reason.clone()))
    }

    /// What is registered now, compared with starting `exe`.
    pub fn status(&self, exe: &Path) -> Result<Status, ShellError> {
        let command = Command::new(exe)?;
        match self.backend()? {
            #[cfg(windows)]
            Backend::Registry(r) => Ok(r.status(&command.windows())?),
            Backend::Desktop(d) => Ok(d.status(&command)?),
        }
    }

    /// Creates the entry, or replaces Animatic's own entry that started something else.
    /// Fails with [`ShellError::Foreign`] when an entry without the ownership marker exists.
    pub fn register(&self, exe: &Path) -> Result<(), ShellError> {
        let command = Command::new(exe)?;
        match self.backend()? {
            #[cfg(windows)]
            Backend::Registry(r) => r.register(&command.windows()),
            Backend::Desktop(d) => d.register(&command),
        }
    }

    /// Removes only an entry Animatic created, and the empty parent keys it needed.
    pub fn unregister(&self) -> Result<Removal, ShellError> {
        match self.backend()? {
            #[cfg(windows)]
            Backend::Registry(r) => Ok(r.unregister()?),
            Backend::Desktop(d) => Ok(d.unregister()?),
        }
    }
}

/// The validated executable.
struct Command {
    exe: String,
}

impl Command {
    fn new(exe: &Path) -> Result<Self, ShellError> {
        if !exe.is_absolute() {
            return Err(ShellError::Invalid(format!(
                "the executable path is not absolute: {}",
                exe.display()
            )));
        }
        let text = exe.to_str().ok_or_else(|| {
            ShellError::Invalid("the executable path is not valid Unicode".into())
        })?;
        if text.chars().any(|c| c.is_control() || c == '"') {
            return Err(ShellError::Invalid(
                "the executable path contains characters that cannot be quoted".into(),
            ));
        }
        // `std::fs::canonicalize` style prefixes are accepted by CreateProcess but look odd in
        // the registry.
        let text = text
            .strip_prefix(r"\\?\")
            .filter(|rest| !rest.starts_with("UNC\\"))
            .unwrap_or(text);
        Ok(Self { exe: text.into() })
    }

    /// Command line stored in the registry: the clicked file is `%1`.
    #[cfg(windows)]
    fn windows(&self) -> String {
        format!("\"{}\" \"%1\"", self.exe)
    }

    /// Value of the desktop file's `Exec` key; `%F` expands to the selected files.
    fn exec(&self) -> String {
        format!("{} %F", desktop::quote_exec_argument(&self.exe))
    }
}

mod desktop {
    use super::*;
    use std::fs;

    pub(super) struct Desktop {
        dir: PathBuf,
        refresh: bool,
    }

    impl Desktop {
        pub(super) fn new(dir: PathBuf, refresh: bool) -> Self {
            Self { dir, refresh }
        }

        pub(super) fn file(&self) -> PathBuf {
            self.dir.join(DESKTOP_FILE)
        }

        fn read(&self) -> io::Result<Option<String>> {
            match fs::read_to_string(self.file()) {
                Ok(text) => Ok(Some(text)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                // Not UTF-8: certainly not ours.
                Err(e) if e.kind() == io::ErrorKind::InvalidData => Ok(Some(String::new())),
                Err(e) => Err(e),
            }
        }

        pub(super) fn status(&self, command: &Command) -> io::Result<Status> {
            Ok(match self.read()? {
                None => Status::NotRegistered,
                Some(text) if !is_ours(&text) => Status::Foreign,
                Some(text) => {
                    let exec = command.exec();
                    if text.lines().any(|line| line == format!("Exec={exec}")) {
                        Status::Registered
                    } else {
                        Status::Elsewhere {
                            command: text
                                .lines()
                                .find_map(|line| line.strip_prefix("Exec="))
                                .unwrap_or_default()
                                .to_string(),
                        }
                    }
                }
            })
        }

        pub(super) fn register(&self, command: &Command) -> Result<(), ShellError> {
            if matches!(self.read()?, Some(text) if !is_ours(&text)) {
                return Err(ShellError::Foreign(DESKTOP_FILE.into()));
            }
            fs::create_dir_all(&self.dir)?;
            crate::atomic::write_atomic(&self.file(), render(command).as_bytes())?;
            self.refresh();
            Ok(())
        }

        pub(super) fn unregister(&self) -> io::Result<Removal> {
            match self.read()? {
                None => Ok(Removal::NotRegistered),
                Some(text) if !is_ours(&text) => Ok(Removal::LeftForeign),
                Some(_) => {
                    fs::remove_file(self.file())?;
                    self.refresh();
                    Ok(Removal::Removed)
                }
            }
        }

        /// Best effort: desktop environments pick the file up without it as well.
        fn refresh(&self) {
            if !self.refresh {
                return;
            }
            let _ = std::process::Command::new("update-desktop-database")
                .arg(&self.dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    fn is_ours(text: &str) -> bool {
        text.lines().any(|line| line.trim() == DESKTOP_MARKER)
    }

    fn render(command: &Command) -> String {
        let mut mime = MIME_TYPES.join(";");
        mime.push(';');
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name={LABEL}\n\
             Comment=Hash media files with Animatic\n\
             Exec={}\n\
             Terminal=false\n\
             NoDisplay=true\n\
             MimeType={mime}\n\
             {DESKTOP_MARKER}\n",
            command.exec()
        )
    }

    /// Quotes `argument` for an `Exec` key: reserved characters force double quotes, and the
    /// specification's two escaping layers (argument, then string value) both apply.
    pub(in crate::shell_integration) fn quote_exec_argument(argument: &str) -> String {
        let plain = !argument.is_empty()
            && argument
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "/._-+:,@=".contains(c));
        if plain {
            return argument.to_string();
        }
        let mut quoted = String::from("\"");
        for c in argument.chars() {
            match c {
                // Argument layer, then the value layer doubles every backslash.
                '"' | '`' | '$' => {
                    quoted.push_str("\\\\");
                    quoted.push(c);
                }
                '\\' => quoted.push_str("\\\\\\\\"),
                // A literal percent sign is `%%`.
                '%' => quoted.push_str("%%"),
                _ => quoted.push(c),
            }
        }
        quoted.push('"');
        quoted
    }
}

#[cfg(windows)]
mod registry {
    use super::*;
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    pub(super) struct Registry {
        base: String,
    }

    impl Registry {
        pub(super) fn new(base: &str) -> Self {
            Self {
                base: base.trim_matches('\\').to_string(),
            }
        }

        fn shell_path(&self) -> String {
            format!(r"{}\*\shell", self.base)
        }

        fn verb_path(&self) -> String {
            format!(r"{}\{VERB}", self.shell_path())
        }

        pub(super) fn location(&self) -> String {
            format!(r"HKEY_CURRENT_USER\{}", self.verb_path())
        }

        fn open_verb(&self) -> io::Result<Option<RegKey>> {
            match RegKey::predef(HKEY_CURRENT_USER).open_subkey(self.verb_path()) {
                Ok(key) => Ok(Some(key)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            }
        }

        fn is_ours(key: &RegKey) -> bool {
            key.get_value::<String, _>(MARKER_NAME)
                .is_ok_and(|value| value == "1")
        }

        pub(super) fn status(&self, command: &str) -> io::Result<Status> {
            let Some(key) = self.open_verb()? else {
                return Ok(Status::NotRegistered);
            };
            if !Self::is_ours(&key) {
                return Ok(Status::Foreign);
            }
            let stored = key
                .open_subkey("command")
                .and_then(|k| k.get_value::<String, _>(""))
                .unwrap_or_default();
            Ok(if stored == command {
                Status::Registered
            } else {
                Status::Elsewhere { command: stored }
            })
        }

        pub(super) fn register(&self, command: &str) -> Result<(), ShellError> {
            if matches!(self.open_verb()?, Some(key) if !Self::is_ours(&key)) {
                return Err(ShellError::Foreign(self.location()));
            }
            let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(self.verb_path())?;
            // The marker goes first, so a half-written entry can still be removed.
            key.set_value(MARKER_NAME, &"1")?;
            key.set_value("", &LABEL)?;
            let exe = command.split('"').nth(1).unwrap_or_default();
            key.set_value("Icon", &exe)?;
            let (command_key, _) = key.create_subkey("command")?;
            command_key.set_value("", &command)?;
            Ok(())
        }

        pub(super) fn unregister(&self) -> io::Result<Removal> {
            let Some(key) = self.open_verb()? else {
                return Ok(Removal::NotRegistered);
            };
            if !Self::is_ours(&key) {
                return Ok(Removal::LeftForeign);
            }
            drop(key);
            let root = RegKey::predef(HKEY_CURRENT_USER);
            root.delete_subkey_all(self.verb_path())?;
            // The parents may have been created for the entry; drop them only when empty.
            let shell = self.shell_path();
            let star = shell.trim_end_matches(r"\shell").to_string();
            for path in [shell, star] {
                Self::remove_if_empty(&root, &path);
            }
            Ok(Removal::Removed)
        }

        fn remove_if_empty(root: &RegKey, path: &str) {
            let empty = root
                .open_subkey(path)
                .and_then(|key| key.query_info())
                .is_ok_and(|info| info.sub_keys == 0 && info.values == 0);
            if empty {
                let _ = root.delete_subkey(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe() -> PathBuf {
        #[cfg(windows)]
        return PathBuf::from(r"C:\animatic-test-root\Program Files\animatic.exe");
        #[cfg(not(windows))]
        PathBuf::from("/animatic-test-root/program files/animatic")
    }

    fn other_exe() -> PathBuf {
        #[cfg(windows)]
        return PathBuf::from(r"C:\animatic-test-root\other\animatic.exe");
        #[cfg(not(windows))]
        PathBuf::from("/animatic-test-root/other/animatic")
    }

    /// Names (not contents) of everything below `dir`.
    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = walkdir::WalkDir::new(dir)
            .into_iter()
            .map(|e| {
                e.unwrap()
                    .path()
                    .strip_prefix(dir)
                    .unwrap()
                    .display()
                    .to_string()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn desktop_round_trip_restores_the_directory() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("applications");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("other.desktop"), "[Desktop Entry]\nName=Other\n").unwrap();
        let before = listing(root.path());
        let shell = ShellIntegration::desktop_entry(&dir, false);

        assert_eq!(shell.status(&exe()).unwrap(), Status::NotRegistered);
        shell.register(&exe()).unwrap();
        assert_eq!(shell.status(&exe()).unwrap(), Status::Registered);

        let text = std::fs::read_to_string(dir.join(DESKTOP_FILE)).unwrap();
        assert!(text.contains("NoDisplay=true"), "{text}");
        assert!(text.contains("video/x-matroska;"), "{text}");
        assert!(text.contains(DESKTOP_MARKER), "{text}");
        assert!(text.contains(&format!("Name={LABEL}")), "{text}");
        // The path has a space, so it is quoted for the Exec key.
        assert!(
            text.lines()
                .any(|l| l.starts_with("Exec=\"") && l.ends_with("\" %F")),
            "{text}"
        );

        assert_eq!(shell.unregister().unwrap(), Removal::Removed);
        assert_eq!(shell.status(&exe()).unwrap(), Status::NotRegistered);
        assert_eq!(listing(root.path()), before);
        assert_eq!(shell.unregister().unwrap(), Removal::NotRegistered);
    }

    #[test]
    fn desktop_never_touches_a_foreign_file() {
        let root = tempfile::tempdir().unwrap();
        let foreign = root.path().join(DESKTOP_FILE);
        std::fs::write(
            &foreign,
            "[Desktop Entry]\nName=Somebody else\nExec=other %F\n",
        )
        .unwrap();
        let shell = ShellIntegration::desktop_entry(root.path(), false);

        assert_eq!(shell.status(&exe()).unwrap(), Status::Foreign);
        assert!(matches!(
            shell.register(&exe()),
            Err(ShellError::Foreign(_))
        ));
        assert_eq!(shell.unregister().unwrap(), Removal::LeftForeign);
        assert_eq!(
            std::fs::read_to_string(&foreign).unwrap(),
            "[Desktop Entry]\nName=Somebody else\nExec=other %F\n"
        );
    }

    #[test]
    fn desktop_entry_for_another_executable_is_reported_and_replaced() {
        let root = tempfile::tempdir().unwrap();
        let shell = ShellIntegration::desktop_entry(root.path(), false);
        shell.register(&other_exe()).unwrap();

        match shell.status(&exe()).unwrap() {
            Status::Elsewhere { command } => assert!(command.contains("other"), "{command}"),
            status => panic!("unexpected {status:?}"),
        }
        shell.register(&exe()).unwrap();
        assert_eq!(shell.status(&exe()).unwrap(), Status::Registered);
        assert_eq!(shell.unregister().unwrap(), Removal::Removed);
    }

    #[test]
    fn register_creates_a_missing_desktop_directory() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("share").join("applications");
        let shell = ShellIntegration::desktop_entry(&dir, false);
        shell.register(&exe()).unwrap();
        assert!(dir.join(DESKTOP_FILE).is_file());
    }

    #[test]
    fn exec_quoting_follows_the_desktop_entry_specification() {
        use super::desktop::quote_exec_argument as quote;
        assert_eq!(quote("/usr/bin/animatic"), "/usr/bin/animatic");
        assert_eq!(quote("/my apps/animatic"), "\"/my apps/animatic\"");
        assert_eq!(quote("/100%/a"), "\"/100%%/a\"");
        assert_eq!(quote("/a$b"), "\"/a\\\\$b\"");
        assert_eq!(quote("/a\\b"), "\"/a\\\\\\\\b\"");
    }

    #[test]
    fn relative_and_unquotable_executables_are_rejected() {
        let shell = ShellIntegration::desktop_entry(std::env::temp_dir(), false);
        assert!(matches!(
            shell.status(Path::new("animatic")),
            Err(ShellError::Invalid(_))
        ));
        let mut bad = exe().into_os_string();
        bad.push("\n");
        assert!(matches!(
            shell.register(Path::new(&bad)),
            Err(ShellError::Invalid(_))
        ));
    }

    #[cfg(windows)]
    mod registry {
        use super::*;
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;

        /// A throwaway `Software\Classes` replacement removed on drop.
        struct TestHive {
            top: String,
            base: String,
        }

        impl TestHive {
            fn new(name: &str) -> Self {
                let top = format!(r"Software\animatic-test-{}-{name}", std::process::id());
                let base = format!(r"{top}\Classes");
                let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&top);
                RegKey::predef(HKEY_CURRENT_USER)
                    .create_subkey(&base)
                    .unwrap();
                Self { top, base }
            }

            fn key(&self, path: &str) -> Option<RegKey> {
                let full = if path.is_empty() {
                    self.base.clone()
                } else {
                    format!(r"{}\{path}", self.base)
                };
                RegKey::predef(HKEY_CURRENT_USER).open_subkey(full).ok()
            }

            fn dump(&self) -> Vec<String> {
                fn walk(key: &RegKey, prefix: &str, out: &mut Vec<String>) {
                    for (name, value) in key.enum_values().map(Result::unwrap) {
                        out.push(format!("{prefix}[{name}]={value:?}"));
                    }
                    for sub in key.enum_keys().map(Result::unwrap) {
                        let path = format!("{prefix}\\{sub}");
                        out.push(path.clone());
                        walk(&key.open_subkey(&sub).unwrap(), &path, out);
                    }
                }
                let mut out = Vec::new();
                walk(&self.key("").unwrap(), "", &mut out);
                out.sort();
                out
            }
        }

        impl Drop for TestHive {
            fn drop(&mut self) {
                let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.top);
            }
        }

        #[test]
        fn registry_round_trip_stores_the_command_and_leaves_no_trace() {
            let hive = TestHive::new("round-trip");
            let before = hive.dump();
            let shell = ShellIntegration::registry(&hive.base);

            assert_eq!(shell.status(&exe()).unwrap(), Status::NotRegistered);
            shell.register(&exe()).unwrap();
            assert_eq!(shell.status(&exe()).unwrap(), Status::Registered);

            let verb = hive.key(r"*\shell\Animatic.Hash").unwrap();
            assert_eq!(verb.get_value::<String, _>("").unwrap(), LABEL);
            assert_eq!(verb.get_value::<String, _>(MARKER_NAME).unwrap(), "1");
            let command: String = verb.open_subkey("command").unwrap().get_value("").unwrap();
            assert_eq!(command, format!("\"{}\" \"%1\"", exe().display()));

            assert_eq!(shell.unregister().unwrap(), Removal::Removed);
            assert_eq!(hive.dump(), before);
            assert_eq!(shell.status(&exe()).unwrap(), Status::NotRegistered);
            assert_eq!(shell.unregister().unwrap(), Removal::NotRegistered);
        }

        #[test]
        fn registry_unregister_keeps_unrelated_shell_entries() {
            let hive = TestHive::new("neighbours");
            let root = RegKey::predef(HKEY_CURRENT_USER);
            let (neighbour, _) = root
                .create_subkey(format!(r"{}\*\shell\Other.Verb\command", hive.base))
                .unwrap();
            neighbour.set_value("", &"other.exe %1").unwrap();
            let before = hive.dump();

            let shell = ShellIntegration::registry(&hive.base);
            shell.register(&exe()).unwrap();
            assert_eq!(shell.unregister().unwrap(), Removal::Removed);
            assert_eq!(hive.dump(), before);
        }

        #[test]
        fn registry_never_touches_a_foreign_key() {
            let hive = TestHive::new("foreign");
            let root = RegKey::predef(HKEY_CURRENT_USER);
            let (foreign, _) = root
                .create_subkey(format!(r"{}\*\shell\Animatic.Hash\command", hive.base))
                .unwrap();
            foreign.set_value("", &"someone-else.exe").unwrap();
            let before = hive.dump();
            let shell = ShellIntegration::registry(&hive.base);

            assert_eq!(shell.status(&exe()).unwrap(), Status::Foreign);
            assert!(matches!(
                shell.register(&exe()),
                Err(ShellError::Foreign(_))
            ));
            assert_eq!(shell.unregister().unwrap(), Removal::LeftForeign);
            assert_eq!(hive.dump(), before);
        }

        #[test]
        fn registry_entry_for_another_executable_is_reported_and_replaced() {
            let hive = TestHive::new("elsewhere");
            let shell = ShellIntegration::registry(&hive.base);
            shell.register(&other_exe()).unwrap();
            assert!(matches!(
                shell.status(&exe()).unwrap(),
                Status::Elsewhere { .. }
            ));
            shell.register(&exe()).unwrap();
            assert_eq!(shell.status(&exe()).unwrap(), Status::Registered);
        }
    }
}
