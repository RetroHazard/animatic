//! Credentials in the operating-system keystore, with a visible fallback to the settings file.
//!
//! The AniDB password, the UDP API key and the AVDump key (see [`FIELDS`]) live in the Windows
//! Credential Manager or the Linux Secret Service. [`Credentials`] wraps a [`SecretStore`] and is
//! used by [`crate::settings::Settings::save`] and
//! [`crate::settings::Settings::load_with_credentials`]:
//!
//! * A successful save writes the secrets to the keystore and stores empty strings plus
//!   `secrets_in_keystore: true` in the settings JSON.
//! * A failing or missing keystore never loses a secret: the values stay in the JSON and the
//!   returned [`CredentialStorage`] says why, so the application can show it.
//! * Loading only reads. Recovery from a backup never writes to the keystore; the first write
//!   happens at the next explicit save.
//!
//! Entries are keyed by service [`SERVICE`] and an account made of a short digest of the settings
//! file path plus the field name, so portable profiles (`ANIMATIC_SETTINGS`) do not share
//! credentials. Tests use [`MemorySecretStore`] and never reach the real keystore.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use crate::settings::{Settings, normalize_path};

/// Service name of every keystore entry written by Animatic.
pub const SERVICE: &str = "animatic";

/// Names of the credential fields, in the order [`Settings::secrets`] lists them.
pub const FIELDS: [&str; 3] = ["anidb.password", "anidb.api_key", "avdump.key"];

/// A failed keystore operation. The text names the platform failure and never a secret value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SecretStoreError(pub String);

/// A flat account-to-secret store. Implementations are the system keystore and the in-memory
/// store used by tests.
pub trait SecretStore: Send + Sync {
    /// `Ok(None)` when no entry exists for the account.
    fn get(&self, account: &str) -> Result<Option<String>, SecretStoreError>;
    fn set(&self, account: &str, value: &str) -> Result<(), SecretStoreError>;
    /// Removing an absent entry succeeds.
    fn delete(&self, account: &str) -> Result<(), SecretStoreError>;
}

/// In-memory [`SecretStore`] for tests; it can be made to fail like an unavailable keystore.
#[derive(Default)]
pub struct MemorySecretStore {
    entries: Mutex<BTreeMap<String, String>>,
    failure: Mutex<Option<String>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every operation fails with `reason` until [`MemorySecretStore::recover`].
    pub fn fail_with(&self, reason: &str) {
        *self.failure.lock() = Some(reason.to_string());
    }

    pub fn recover(&self) {
        *self.failure.lock() = None;
    }

    /// Stored accounts, for assertions.
    pub fn accounts(&self) -> Vec<String> {
        self.entries.lock().keys().cloned().collect()
    }

    /// Stored value, bypassing the failure switch.
    pub fn peek(&self, account: &str) -> Option<String> {
        self.entries.lock().get(account).cloned()
    }

    /// Stores a value, bypassing the failure switch.
    pub fn put(&self, account: &str, value: &str) {
        self.entries.lock().insert(account.into(), value.into());
    }

    fn check(&self) -> Result<(), SecretStoreError> {
        match self.failure.lock().as_ref() {
            Some(reason) => Err(SecretStoreError(reason.clone())),
            None => Ok(()),
        }
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, account: &str) -> Result<Option<String>, SecretStoreError> {
        self.check()?;
        Ok(self.entries.lock().get(account).cloned())
    }

    fn set(&self, account: &str, value: &str) -> Result<(), SecretStoreError> {
        self.check()?;
        self.entries.lock().insert(account.into(), value.into());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), SecretStoreError> {
        self.check()?;
        self.entries.lock().remove(account);
        Ok(())
    }
}

impl<S: SecretStore + ?Sized> SecretStore for std::sync::Arc<S> {
    fn get(&self, account: &str) -> Result<Option<String>, SecretStoreError> {
        (**self).get(account)
    }

    fn set(&self, account: &str, value: &str) -> Result<(), SecretStoreError> {
        (**self).set(account, value)
    }

    fn delete(&self, account: &str) -> Result<(), SecretStoreError> {
        (**self).delete(account)
    }
}

/// The Windows Credential Manager or the Linux Secret Service, through `keyring-core`.
pub struct OsSecretStore {
    store: std::sync::Arc<keyring_core::CredentialStore>,
}

impl OsSecretStore {
    /// Connects to the platform keystore; fails when none is reachable (for example no Secret
    /// Service on a headless Linux session) or the platform has no supported keystore.
    pub fn open() -> Result<Self, SecretStoreError> {
        #[cfg(windows)]
        let store: std::sync::Arc<keyring_core::CredentialStore> =
            windows_native_keyring_store::Store::new()?;
        #[cfg(all(
            unix,
            not(any(target_os = "macos", target_os = "ios", target_os = "android"))
        ))]
        let store: std::sync::Arc<keyring_core::CredentialStore> =
            zbus_secret_service_keyring_store::Store::new()?;
        #[cfg(any(
            windows,
            all(
                unix,
                not(any(target_os = "macos", target_os = "ios", target_os = "android"))
            )
        ))]
        return Ok(OsSecretStore { store });
        #[cfg(not(any(
            windows,
            all(
                unix,
                not(any(target_os = "macos", target_os = "ios", target_os = "android"))
            )
        )))]
        Err(SecretStoreError(
            "this platform has no supported system keystore".into(),
        ))
    }

    fn entry(&self, account: &str) -> Result<keyring_core::Entry, SecretStoreError> {
        Ok(self.store.build(SERVICE, account, None)?)
    }
}

impl From<keyring_core::Error> for SecretStoreError {
    fn from(e: keyring_core::Error) -> Self {
        SecretStoreError(e.to_string())
    }
}

impl SecretStore for OsSecretStore {
    fn get(&self, account: &str) -> Result<Option<String>, SecretStoreError> {
        match self.entry(account)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set(&self, account: &str, value: &str) -> Result<(), SecretStoreError> {
        Ok(self.entry(account)?.set_password(value)?)
    }

    fn delete(&self, account: &str) -> Result<(), SecretStoreError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Where the credentials of a settings file are kept, as far as the last load or save knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialStorage {
    /// In the system keystore; the settings file holds none.
    Keystore,
    /// In the settings file because the keystore is unavailable or refused the write.
    SettingsFile { reason: String },
    /// The settings file defers to the keystore but the saved credentials could not be read.
    /// Saving leaves the unread entries untouched.
    Unreadable { reason: String },
}

impl CredentialStorage {
    /// The line Options shows, or `None` when everything is as expected.
    pub fn notice(&self) -> Option<String> {
        match self {
            CredentialStorage::Keystore => None,
            CredentialStorage::SettingsFile { reason } => Some(format!(
                "Credentials are stored in the settings file because no system keystore is available: {reason}"
            )),
            CredentialStorage::Unreadable { reason } => Some(format!(
                "Saved credentials could not be read from the system keystore: {reason}. \
                 Enter them again or fix the keystore; saving keeps unread entries unchanged."
            )),
        }
    }
}

enum Backend {
    Store(Box<dyn SecretStore>),
    Unavailable(String),
}

/// A keystore plus the bookkeeping that keeps unread secrets from being deleted.
pub struct Credentials {
    backend: Backend,
    /// Accounts whose stored value could not be read at load; an empty field for such an
    /// account says "unknown", not "cleared", so saving must not delete the entry.
    unread: Mutex<BTreeSet<String>>,
}

impl Credentials {
    pub fn with_store(store: impl SecretStore + 'static) -> Self {
        Credentials {
            backend: Backend::Store(Box::new(store)),
            unread: Mutex::default(),
        }
    }

    /// No keystore: secrets stay in the settings file and `reason` is shown to the user.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Credentials {
            backend: Backend::Unavailable(reason.into()),
            unread: Mutex::default(),
        }
    }

    /// The platform keystore, or the unavailable fallback with the failure as its reason.
    pub fn system() -> Self {
        match OsSecretStore::open() {
            Ok(store) => Self::with_store(store),
            Err(e) => Self::unavailable(e.to_string()),
        }
    }

    /// Account of one field of the settings file at `path`.
    pub fn account(path: &Path, field: &str) -> String {
        format!("{}/{field}", profile_key(path))
    }

    /// Fills the credential fields the settings file left empty from the keystore.
    ///
    /// Reads only. Fields that already hold a value (a file saved while the keystore was
    /// unavailable) win over the keystore.
    pub fn restore(&self, settings: &mut Settings, path: &Path) -> CredentialStorage {
        let usable = |reason: &str| CredentialStorage::SettingsFile {
            reason: reason.to_string(),
        };
        if !settings.secrets_in_keystore {
            return match &self.backend {
                Backend::Store(_) => CredentialStorage::Keystore,
                Backend::Unavailable(reason) => usable(reason),
            };
        }
        let mut failure: Option<String> = None;
        for (field, value) in FIELDS.into_iter().zip(settings.secrets_mut()) {
            if !value.is_empty() {
                continue;
            }
            let account = Self::account(path, field);
            let read = match &self.backend {
                Backend::Store(store) => store.get(&account),
                Backend::Unavailable(reason) => Err(SecretStoreError(reason.clone())),
            };
            match read {
                Ok(found) => {
                    self.unread.lock().remove(&account);
                    if let Some(found) = found {
                        *value = found;
                    }
                }
                Err(e) => {
                    self.unread.lock().insert(account);
                    failure.get_or_insert(e.0);
                }
            }
        }
        match (failure, &self.backend) {
            (Some(reason), _) => CredentialStorage::Unreadable { reason },
            (None, Backend::Store(_)) => CredentialStorage::Keystore,
            (None, Backend::Unavailable(reason)) => usable(reason),
        }
    }

    /// Prepares the document to write for `settings`: moves the secrets into the keystore and
    /// blanks them in the returned copy, or keeps them in the copy when the keystore cannot take
    /// all of them.
    pub(crate) fn seal(&self, settings: &Settings, path: &Path) -> (Settings, CredentialStorage) {
        let mut document = settings.clone();
        let store = match &self.backend {
            Backend::Store(store) => store,
            Backend::Unavailable(reason) => {
                return (
                    document,
                    CredentialStorage::SettingsFile {
                        reason: reason.clone(),
                    },
                );
            }
        };
        for (field, value) in FIELDS.into_iter().zip(settings.secrets()) {
            let account = Self::account(path, field);
            let outcome = if value.is_empty() {
                if self.unread.lock().contains(&account) {
                    continue;
                }
                store.delete(&account)
            } else {
                store.set(&account, value)
            };
            match outcome {
                Ok(()) => {
                    self.unread.lock().remove(&account);
                }
                Err(e) => {
                    return (document, CredentialStorage::SettingsFile { reason: e.0 });
                }
            }
        }
        document.clear_secrets();
        document.secrets_in_keystore = true;
        (document, CredentialStorage::Keystore)
    }
}

/// Short stable digest of the settings file location. The path is made absolute without touching
/// the file system and normalized like other paths (case-insensitive on Windows).
pub fn profile_key(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let digest = Sha256::digest(normalize_path(&absolute.to_string_lossy()).as_bytes());
    hex::encode(&digest[..8])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const PASSWORD: &str = "animatic-test-password";
    const API_KEY: &str = "animatic-test-api-key";
    const AVDUMP_KEY: &str = "animatic-test-avdump-key";

    struct Profile {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
    }

    impl Profile {
        fn new() -> Profile {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.json");
            Profile { _dir: dir, path }
        }

        fn text(&self) -> String {
            std::fs::read_to_string(&self.path).unwrap()
        }

        fn backup_text(&self) -> String {
            std::fs::read_to_string(Settings::backup_path(&self.path)).unwrap()
        }
    }

    fn memory() -> (Arc<MemorySecretStore>, Credentials) {
        let store = Arc::new(MemorySecretStore::new());
        (store.clone(), Credentials::with_store(store))
    }

    fn with_secrets() -> Settings {
        let mut s = Settings::default();
        s.anidb.username = "animatic-test-user".into();
        s.anidb.password = PASSWORD.into();
        s.anidb.api_key = API_KEY.into();
        s.avdump.key = AVDUMP_KEY.into();
        s
    }

    fn assert_no_secret(text: &str) {
        for secret in [PASSWORD, API_KEY, AVDUMP_KEY] {
            assert!(!text.contains(secret), "{secret} leaked into: {text}");
        }
    }

    #[test]
    fn save_moves_secrets_to_the_keystore_and_load_restores_them() {
        let profile = Profile::new();
        let (store, credentials) = memory();
        let storage = with_secrets().save(&profile.path, &credentials).unwrap();
        assert_eq!(storage, CredentialStorage::Keystore);
        assert_no_secret(&profile.text());
        assert_eq!(storage.notice(), None);
        assert_eq!(store.accounts().len(), 3);

        // A fresh process: new Credentials over the same keystore.
        let (loaded, storage) =
            Settings::load_with_credentials(&profile.path, &Credentials::with_store(store))
                .unwrap();
        assert_eq!(storage, CredentialStorage::Keystore);
        assert_eq!(loaded.settings.anidb.username, "animatic-test-user");
        assert_eq!(loaded.settings.anidb.password, PASSWORD);
        assert_eq!(loaded.settings.anidb.api_key, API_KEY);
        assert_eq!(loaded.settings.avdump.key, AVDUMP_KEY);
    }

    #[test]
    fn keystore_failure_keeps_plaintext_and_reports_the_fallback() {
        let profile = Profile::new();
        let (store, credentials) = memory();
        store.fail_with("the keystore is locked");
        let storage = with_secrets().save(&profile.path, &credentials).unwrap();
        let CredentialStorage::SettingsFile { reason } = &storage else {
            panic!("expected the settings-file fallback, got {storage:?}");
        };
        assert_eq!(reason, "the keystore is locked");
        let notice = storage.notice().unwrap();
        assert!(notice.starts_with("Credentials are stored in the settings file because"));
        assert!(notice.contains("the keystore is locked"));
        assert!(store.accounts().is_empty());

        let (loaded, _) =
            Settings::load_with_credentials(&profile.path, &Credentials::with_store(store))
                .unwrap();
        assert_eq!(loaded.settings.anidb.password, PASSWORD);
        assert_eq!(loaded.settings.anidb.api_key, API_KEY);
        assert_eq!(loaded.settings.avdump.key, AVDUMP_KEY);
    }

    #[test]
    fn missing_keystore_keeps_secrets_in_the_settings_file() {
        let profile = Profile::new();
        let credentials = Credentials::unavailable("no Secret Service on the session bus");
        let storage = with_secrets().save(&profile.path, &credentials).unwrap();
        assert!(
            storage
                .notice()
                .unwrap()
                .contains("no Secret Service on the session bus")
        );
        assert!(profile.text().contains(PASSWORD));

        let (loaded, storage) =
            Settings::load_with_credentials(&profile.path, &credentials).unwrap();
        assert_eq!(loaded.settings.anidb.password, PASSWORD);
        assert!(matches!(storage, CredentialStorage::SettingsFile { .. }));
    }

    #[test]
    fn a_failure_after_some_writes_still_loses_nothing() {
        struct FailsOnSecondWrite {
            inner: MemorySecretStore,
            writes: std::sync::atomic::AtomicUsize,
        }
        impl SecretStore for FailsOnSecondWrite {
            fn get(&self, a: &str) -> Result<Option<String>, SecretStoreError> {
                self.inner.get(a)
            }
            fn set(&self, a: &str, v: &str) -> Result<(), SecretStoreError> {
                if self
                    .writes
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    == 1
                {
                    return Err(SecretStoreError("quota exceeded".into()));
                }
                self.inner.set(a, v)
            }
            fn delete(&self, a: &str) -> Result<(), SecretStoreError> {
                self.inner.delete(a)
            }
        }
        let profile = Profile::new();
        let credentials = Credentials::with_store(FailsOnSecondWrite {
            inner: MemorySecretStore::new(),
            writes: Default::default(),
        });
        let storage = with_secrets().save(&profile.path, &credentials).unwrap();
        assert!(matches!(storage, CredentialStorage::SettingsFile { .. }));
        let text = profile.text();
        for secret in [PASSWORD, API_KEY, AVDUMP_KEY] {
            assert!(text.contains(secret));
        }
    }

    #[test]
    fn plaintext_settings_migrate_on_the_first_successful_save() {
        let profile = Profile::new();
        // A file written by a version that kept the secrets in the JSON.
        std::fs::write(
            &profile.path,
            serde_json::to_string_pretty(&with_secrets()).unwrap(),
        )
        .unwrap();
        let (store, credentials) = memory();

        let (loaded, storage) =
            Settings::load_with_credentials(&profile.path, &credentials).unwrap();
        assert_eq!(storage, CredentialStorage::Keystore);
        assert_eq!(loaded.settings.anidb.password, PASSWORD);
        // Loading alone changes nothing.
        assert!(profile.text().contains(PASSWORD));
        assert!(store.accounts().is_empty());

        loaded.settings.save(&profile.path, &credentials).unwrap();
        assert_no_secret(&profile.text());
        // The backup of the plaintext file is blanked as well; it restores from the keystore.
        assert_no_secret(&profile.backup_text());
        assert_eq!(store.accounts().len(), 3);
        let (restored, _) = Settings::load_with_credentials(&profile.path, &credentials).unwrap();
        assert_eq!(restored.settings.anidb.password, PASSWORD);
    }

    #[test]
    fn a_failed_migration_leaves_the_plaintext_backup_untouched() {
        let profile = Profile::new();
        let plaintext = serde_json::to_string_pretty(&with_secrets()).unwrap();
        std::fs::write(&profile.path, &plaintext).unwrap();
        let (store, credentials) = memory();
        store.fail_with("denied");
        let loaded = Settings::load_with_recovery(&profile.path).unwrap();
        loaded.settings.save(&profile.path, &credentials).unwrap();
        assert_eq!(profile.backup_text(), plaintext);
        assert!(profile.text().contains(PASSWORD));
    }

    #[test]
    fn recovery_from_the_backup_never_writes_to_the_keystore() {
        let profile = Profile::new();
        let (store, credentials) = memory();
        with_secrets().save(&profile.path, &credentials).unwrap();
        // Second save so a valid backup exists, then damage the primary.
        with_secrets().save(&profile.path, &credentials).unwrap();
        std::fs::write(&profile.path, "{ not json").unwrap();
        let before: Vec<_> = store
            .accounts()
            .into_iter()
            .map(|a| (store.peek(&a), a))
            .collect();
        let primary = profile.text();
        let backup = profile.backup_text();

        let (loaded, storage) =
            Settings::load_with_credentials(&profile.path, &credentials).unwrap();
        assert!(loaded.recovered_from_backup);
        assert!(loaded.settings.anidb.offline);
        assert_eq!(storage, CredentialStorage::Keystore);
        assert_eq!(loaded.settings.anidb.password, PASSWORD);
        let after: Vec<_> = store
            .accounts()
            .into_iter()
            .map(|a| (store.peek(&a), a))
            .collect();
        assert_eq!(before, after);
        assert_eq!(profile.text(), primary);
        assert_eq!(profile.backup_text(), backup);

        // A recovered plaintext backup is not migrated by loading either.
        let (empty_store, empty_credentials) = memory();
        std::fs::write(
            Settings::backup_path(&profile.path),
            serde_json::to_string_pretty(&with_secrets()).unwrap(),
        )
        .unwrap();
        let (loaded, _) =
            Settings::load_with_credentials(&profile.path, &empty_credentials).unwrap();
        assert!(loaded.recovered_from_backup);
        assert!(empty_store.accounts().is_empty());
    }

    #[test]
    fn load_errors_expose_no_secret_values() {
        let profile = Profile::new();
        let (_, credentials) = memory();
        std::fs::write(
            &profile.path,
            format!(r#"{{"anidb":{{"password":"{PASSWORD}"#),
        )
        .unwrap();
        std::fs::write(
            Settings::backup_path(&profile.path),
            format!(r#"{{"anidb":"{API_KEY}"}}"#),
        )
        .unwrap();
        let error = Settings::load_with_credentials(&profile.path, &credentials)
            .unwrap_err()
            .to_string();
        assert_no_secret(&error);
    }

    #[test]
    fn settings_files_at_different_paths_do_not_share_credentials() {
        let first = Profile::new();
        let second = Profile::new();
        let (store, credentials) = memory();
        let mut a = with_secrets();
        a.anidb.password = "animatic-first-password".into();
        let mut b = with_secrets();
        b.anidb.password = "animatic-second-password".into();
        a.save(&first.path, &credentials).unwrap();
        b.save(&second.path, &credentials).unwrap();
        assert_eq!(store.accounts().len(), 6);

        let (a, _) = Settings::load_with_credentials(&first.path, &credentials).unwrap();
        let (b, _) = Settings::load_with_credentials(&second.path, &credentials).unwrap();
        assert_eq!(a.settings.anidb.password, "animatic-first-password");
        assert_eq!(b.settings.anidb.password, "animatic-second-password");
        assert_ne!(profile_key(&first.path), profile_key(&second.path));
    }

    #[test]
    fn clearing_a_credential_removes_its_keystore_entry() {
        let profile = Profile::new();
        let (store, credentials) = memory();
        let mut settings = with_secrets();
        settings.save(&profile.path, &credentials).unwrap();
        settings.avdump.key.clear();
        settings.save(&profile.path, &credentials).unwrap();
        assert_eq!(store.accounts().len(), 2);
        let (loaded, _) = Settings::load_with_credentials(&profile.path, &credentials).unwrap();
        assert_eq!(loaded.settings.avdump.key, "");
        assert_eq!(loaded.settings.anidb.password, PASSWORD);
    }

    #[test]
    fn credentials_that_could_not_be_read_are_not_deleted_by_a_later_save() {
        let profile = Profile::new();
        let (store, credentials) = memory();
        with_secrets().save(&profile.path, &credentials).unwrap();

        let (restarted_store, restarted) = (store.clone(), Credentials::with_store(store.clone()));
        restarted_store.fail_with("keystore is locked");
        let (mut loaded, storage) =
            Settings::load_with_credentials(&profile.path, &restarted).unwrap();
        assert!(matches!(storage, CredentialStorage::Unreadable { .. }));
        assert!(storage.notice().unwrap().contains("keystore is locked"));
        assert_eq!(loaded.settings.anidb.password, "");

        // The keystore comes back; the user saves without having re-entered anything.
        restarted_store.recover();
        loaded.settings.anidb.username = "animatic-test-user-2".into();
        loaded.settings.save(&profile.path, &restarted).unwrap();
        assert_eq!(store.accounts().len(), 3);
        let (again, _) = Settings::load_with_credentials(&profile.path, &restarted).unwrap();
        assert_eq!(again.settings.anidb.password, PASSWORD);
        assert_eq!(again.settings.anidb.username, "animatic-test-user-2");
    }

    #[test]
    fn values_in_the_file_win_over_the_keystore() {
        let profile = Profile::new();
        let (store, credentials) = memory();
        with_secrets().save(&profile.path, &credentials).unwrap();
        // The file was saved later while the keystore was down.
        let mut newer = with_secrets();
        newer.anidb.password = "animatic-newer-password".into();
        newer.secrets_in_keystore = true;
        std::fs::write(&profile.path, serde_json::to_string_pretty(&newer).unwrap()).unwrap();
        let (loaded, _) =
            Settings::load_with_credentials(&profile.path, &Credentials::with_store(store))
                .unwrap();
        assert_eq!(loaded.settings.anidb.password, "animatic-newer-password");
    }
}
