use std::collections::{BTreeMap, HashMap};

use crate::providers::ProviderId;

const MAX_API_KEY_BYTES: usize = 4 * 1024;

fn service_name() -> &'static str {
    static IDENTITY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    IDENTITY.get_or_init(|| {
        let config: serde_json::Value = serde_json::from_str(include_str!("../../tauri.conf.json"))
            .expect("embedded Tauri configuration should be valid");
        config["identifier"]
            .as_str()
            .expect("Tauri identifier should be configured")
            .to_owned()
    })
}

/// Result of an explicit saved-key search. It never carries key material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindOutcome {
    Found,
    NotFound,
    Invalid,
    Unavailable,
    AlreadySaved,
    SaveFailed,
}

pub(crate) trait CredentialStore {
    fn read(&self, service: &str, provider: ProviderId) -> Result<Option<String>, String>;
    fn write(&self, service: &str, provider: ProviderId, key: &str) -> Result<(), String>;
    fn delete(&self, service: &str, provider: ProviderId) -> Result<(), String>;
}

struct NativeCredentialStore;

impl CredentialStore for NativeCredentialStore {
    fn read(&self, service: &str, provider: ProviderId) -> Result<Option<String>, String> {
        let entry = keychain_entry(service, provider)?;
        match entry.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err("Keychain read failed.".into()),
        }
    }

    fn write(&self, service: &str, provider: ProviderId, key: &str) -> Result<(), String> {
        keychain_entry(service, provider)?
            .set_password(key)
            .map_err(|_| "Keychain write failed.".to_string())
    }
    fn delete(&self, service: &str, provider: ProviderId) -> Result<(), String> {
        match keychain_entry(service, provider)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("Keychain delete failed.".into()),
        }
    }
}

fn write_verified(
    store: &(impl CredentialStore + ?Sized),
    provider: ProviderId,
    api_key: &str,
) -> Result<(), String> {
    let api_key = validate_api_key(api_key)?;
    store
        .write(service_name(), provider, api_key)
        .map_err(|_| "Keychain write failed.".to_string())?;
    let verified = store
        .read(service_name(), provider)
        .map_err(|_| "Keychain verification failed.".to_string())?;
    if verified.as_deref() != Some(api_key) {
        return Err("Keychain verification failed.".to_string());
    }
    Ok(())
}

/// The backend stays dormant until an explicit save, removal, saved-key search or saved-key use.
pub struct CredentialService {
    store: Box<dyn CredentialStore + Send + Sync>,
    operations: BTreeMap<&'static str, std::sync::Mutex<()>>,
}

impl Default for CredentialService {
    fn default() -> Self {
        Self::new(NativeCredentialStore)
    }
}

impl CredentialService {
    pub(crate) fn new(store: impl CredentialStore + Send + Sync + 'static) -> Self {
        Self {
            store: Box::new(store),
            operations: ProviderId::ALL
                .into_iter()
                .map(|id| (id.as_str(), std::sync::Mutex::new(())))
                .collect(),
        }
    }

    pub fn presence(&self, settings: &crate::storage::PersistedSettings) -> HashMap<String, bool> {
        list_presence(settings)
    }

    pub fn save(
        &self,
        storage: &crate::storage::EchoStorage,
        provider: ProviderId,
        key: &str,
        publish: impl FnOnce(&crate::storage::SettingsUpdate),
    ) -> Result<(), String> {
        let _operation = self.operations[provider.as_str()]
            .lock()
            .map_err(|_| "Credential operation unavailable. Try again.".to_string())?;
        write_verified(self.store.as_ref(), provider, key)?;
        self.commit_marker(storage, provider, true, publish)
    }

    pub fn remove(
        &self,
        storage: &crate::storage::EchoStorage,
        provider: ProviderId,
        publish: impl FnOnce(&crate::storage::SettingsUpdate),
    ) -> Result<(), String> {
        let _operation = self.operations[provider.as_str()]
            .lock()
            .map_err(|_| "Credential operation unavailable. Try again.".to_string())?;
        self.store
            .delete(service_name(), provider)
            .map_err(|_| "Credential removal failed. Try again.".to_string())?;
        self.commit_marker(storage, provider, false, publish)
    }

    pub fn resolve(
        &self,
        storage: &crate::storage::EchoStorage,
        provider: ProviderId,
        publish: impl FnOnce(&crate::storage::SettingsUpdate),
    ) -> Result<String, String> {
        let _operation = self.operations[provider.as_str()]
            .lock()
            .map_err(|_| "Credential operation unavailable. Try again.".to_string())?;
        let settings = storage
            .read_settings()
            .map_err(|_| metadata_error())?
            .settings;
        if settings.provider_key_saved.get(provider.as_str()) != Some(&true) {
            return Err(missing_key());
        }
        match self
            .store
            .read(service_name(), provider)
            .map_err(|_| "Keychain read failed.".to_string())?
        {
            Some(key) => validate_api_key(&key).map(ToOwned::to_owned),
            None => {
                self.commit_marker(storage, provider, false, publish)?;
                Err(missing_key())
            }
        }
    }

    /// Adopts a valid OS item already saved under Echo's identity. It never writes or deletes it.
    pub fn find(
        &self,
        storage: &crate::storage::EchoStorage,
        provider: ProviderId,
        publish: impl FnOnce(&crate::storage::SettingsUpdate),
    ) -> Result<FindOutcome, String> {
        let _operation = self.operations[provider.as_str()]
            .lock()
            .map_err(|_| "Credential operation unavailable. Try again.".to_string())?;
        let settings = storage
            .read_settings()
            .map_err(|_| search_error())?
            .settings;
        if settings.provider_key_saved.get(provider.as_str()) == Some(&true) {
            return Ok(FindOutcome::AlreadySaved);
        }
        // Read failures carry OS detail; the outcome alone leaves Rust.
        let Ok(item) = self.store.read(service_name(), provider) else {
            return Ok(FindOutcome::Unavailable);
        };
        let Some(key) = item else {
            return Ok(FindOutcome::NotFound);
        };
        let valid = validate_api_key(&key).is_ok();
        drop(key);
        if !valid {
            return Ok(FindOutcome::Invalid);
        }
        // A valid key was read, so a failed commit is an outcome rather than a search failure.
        if self
            .commit_marker(storage, provider, true, publish)
            .is_err()
        {
            return Ok(FindOutcome::SaveFailed);
        }
        Ok(FindOutcome::Found)
    }

    fn commit_marker(
        &self,
        storage: &crate::storage::EchoStorage,
        provider: ProviderId,
        saved: bool,
        publish: impl FnOnce(&crate::storage::SettingsUpdate),
    ) -> Result<(), String> {
        if let Some(update) = storage
            .set_provider_key_saved(provider, saved)
            .map_err(|_| metadata_error())?
        {
            publish(&update);
        }
        Ok(())
    }
}

/// Searches that fail before a result exists share one message that never claims a key was found.
pub(crate) fn search_error() -> String {
    "Saved key search failed. Try again.".into()
}

fn metadata_error() -> String {
    "Credential settings could not be saved. Try again.".into()
}

fn list_presence(settings: &crate::storage::PersistedSettings) -> HashMap<String, bool> {
    ProviderId::ALL
        .into_iter()
        .map(|provider| {
            let id = provider.as_str();
            (
                id.into(),
                settings.provider_key_saved.get(id) == Some(&true),
            )
        })
        .collect()
}

fn keychain_entry(service: &str, provider: ProviderId) -> Result<keyring::Entry, String> {
    keyring::Entry::new(service, provider.as_str())
        .map_err(|_| "Keychain is unavailable.".to_string())
}

fn validate_api_key(api_key: &str) -> Result<&str, String> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err("API key must not be empty.".to_string());
    }
    if api_key.len() > MAX_API_KEY_BYTES {
        return Err("API key is too large.".to_string());
    }
    Ok(api_key)
}

fn missing_key() -> String {
    "Add and verify an API key for the selected provider before recording.".to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::storage::test_support::{cleanup, initialized, test_database};
    use crate::test_support::clear_child_environment;

    #[derive(Clone, Default)]
    pub(crate) struct CountingStore(std::sync::Arc<std::sync::Mutex<CountingState>>);

    #[derive(Default)]
    struct CountingState {
        values: BTreeMap<(String, String), String>,
        calls: Vec<&'static str>,
        denied: bool,
        fail_write: bool,
        lose_write: bool,
        fail_delete: bool,
    }

    impl CountingStore {
        pub(crate) fn calls(&self) -> Vec<&'static str> {
            self.0.lock().unwrap().calls.clone()
        }
        pub(crate) fn deny_reads(&self, denied: bool) {
            self.0.lock().unwrap().denied = denied;
        }
        pub(crate) fn reset_calls(&self) {
            self.0.lock().unwrap().calls.clear();
        }
    }

    impl CredentialStore for CountingStore {
        fn read(&self, service: &str, provider: ProviderId) -> Result<Option<String>, String> {
            let mut state = self.0.lock().unwrap();
            state.calls.extend(["construct", "read"]);
            if state.denied {
                return Err("private OS detail".into());
            }
            Ok(state
                .values
                .get(&(service.into(), provider.as_str().into()))
                .cloned())
        }
        fn write(&self, service: &str, provider: ProviderId, key: &str) -> Result<(), String> {
            let mut state = self.0.lock().unwrap();
            state.calls.extend(["construct", "write"]);
            if state.fail_write {
                return Err("private OS detail".into());
            }
            if !state.lose_write {
                state
                    .values
                    .insert((service.into(), provider.as_str().into()), key.into());
            }
            Ok(())
        }
        fn delete(&self, service: &str, provider: ProviderId) -> Result<(), String> {
            let mut state = self.0.lock().unwrap();
            state.calls.extend(["construct", "delete"]);
            if state.fail_delete {
                return Err("private OS detail".into());
            }
            state
                .values
                .remove(&(service.into(), provider.as_str().into()));
            Ok(())
        }
    }

    // Keep: real SQLite commits and the injected backend prove native marker ordering for every provider.
    #[test]
    fn credentials_save_remove_and_missing_resolution_commit_before_publication() {
        for provider in ProviderId::ALL {
            let path = test_database("echo-credential");
            let storage = initialized(&path);
            let store = CountingStore::default();
            let service = CredentialService::new(store.clone());
            let marker =
                || storage.read_settings().unwrap().settings.provider_key_saved[provider.as_str()];
            let publications = std::cell::Cell::new(0);
            assert_eq!(
                service
                    .resolve(&storage, provider, |_| panic!("false marker published"))
                    .unwrap_err(),
                missing_key()
            );
            assert!(store.calls().is_empty());
            service
                .save(&storage, provider, "  synthetic-fixture  ", |update| {
                    assert!(marker());
                    assert!(update.current.settings.provider_key_saved[provider.as_str()]);
                    publications.set(publications.get() + 1);
                })
                .unwrap();
            assert_eq!(store.calls(), ["construct", "write", "construct", "read"]);
            // A legacy environment choice cannot hide an explicitly saved OS item.
            let mut row = serde_json::to_value(storage.read_settings().unwrap().settings).unwrap();
            row["providerCredentialModes"] = serde_json::json!({provider.as_str(): "env"});
            rusqlite::Connection::open(&path)
                .unwrap()
                .execute("UPDATE settings SET value_json=?1", [row.to_string()])
                .unwrap();
            assert_eq!(
                service
                    .resolve(&storage, provider, |_| panic!("unchanged marker published"))
                    .unwrap(),
                "synthetic-fixture"
            );
            store.0.lock().unwrap().denied = true;
            assert_eq!(
                service
                    .resolve(&storage, provider, |_| panic!("denial cleared metadata"))
                    .unwrap_err(),
                "Keychain read failed."
            );
            assert!(marker());
            store.0.lock().unwrap().denied = false;
            store.delete(service_name(), provider).unwrap();
            store.reset_calls();
            assert_eq!(
                service
                    .resolve(&storage, provider, |update| {
                        assert!(!marker());
                        assert!(!update.current.settings.provider_key_saved[provider.as_str()]);
                        publications.set(publications.get() + 1);
                    })
                    .unwrap_err(),
                missing_key()
            );
            assert!(!marker());
            assert_eq!(store.calls(), ["construct", "read"]);
            assert_eq!(publications.get(), 2);
            store.reset_calls();
            assert!(service
                .resolve(&storage, provider, |_| panic!("missing republished"))
                .is_err());
            assert!(store.calls().is_empty());
            for missing in [false, true] {
                service
                    .save(&storage, provider, "synthetic-fixture", |_| {})
                    .unwrap();
                if missing {
                    store.delete(service_name(), provider).unwrap();
                }
                store.reset_calls();
                service
                    .remove(&storage, provider, |_| {
                        assert!(!marker());
                    })
                    .unwrap();
                assert!(!marker());
                assert_eq!(store.calls(), ["construct", "delete"]);
            }
            drop(storage);
            cleanup(&path);
        }
    }

    #[test]
    fn credentials_failures_preserve_metadata_and_never_compensate_os_mutations() {
        for failure in [
            "write",
            "verification",
            "delete",
            "metadata-save",
            "metadata-delete",
            "metadata-missing",
        ] {
            let path = test_database("echo-credential-failure");
            let storage = initialized(&path);
            let provider = ProviderId::GroqWhisperTurbo;
            let store = CountingStore::default();
            let service = CredentialService::new(store.clone());
            let previous = matches!(failure, "delete" | "metadata-delete" | "metadata-missing");
            if previous {
                service
                    .save(&storage, provider, "synthetic-fixture", |_| {})
                    .unwrap();
            }
            if failure == "metadata-missing" {
                store.delete(service_name(), provider).unwrap();
            }
            {
                let mut state = store.0.lock().unwrap();
                state.fail_write = failure == "write";
                state.lose_write = failure == "verification";
                state.fail_delete = failure == "delete";
            }
            if failure.starts_with("metadata") {
                rusqlite::Connection::open(&path).unwrap().execute_batch("CREATE TRIGGER reject_metadata BEFORE UPDATE ON settings BEGIN SELECT RAISE(FAIL,'private database detail'); END;").unwrap();
            }
            store.reset_calls();
            let publish =
                |_: &crate::storage::SettingsUpdate| panic!("failed mutation published success");
            let error = if failure == "metadata-missing" {
                service.resolve(&storage, provider, publish).unwrap_err()
            } else if previous {
                service.remove(&storage, provider, publish).unwrap_err()
            } else {
                service
                    .save(&storage, provider, "synthetic-fixture", publish)
                    .unwrap_err()
            };
            assert!(!error.contains("private"));
            assert!(!error.contains("synthetic"));
            assert_eq!(
                storage.read_settings().unwrap().settings.provider_key_saved[provider.as_str()],
                previous
            );
            let expected: &[&str] = match failure {
                "write" => &["construct", "write"],
                "verification" | "metadata-save" => &["construct", "write", "construct", "read"],
                "metadata-missing" => &["construct", "read"],
                _ => &["construct", "delete"],
            };
            assert_eq!(store.calls(), expected);
            drop(storage);
            cleanup(&path);
        }
    }

    // Keep: a competing removal waits through settings publication without blocking other providers.
    #[test]
    fn credentials_provider_lock_covers_mutation_metadata_and_publication() {
        use std::sync::{mpsc, Arc};
        let path = test_database("echo-credential-lock");
        let storage = Arc::new(initialized(&path));
        let store = CountingStore::default();
        let service = Arc::new(CredentialService::new(store.clone()));
        let provider = ProviderId::GroqWhisperTurbo;
        let (published, publication) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let saving = {
            let service = Arc::clone(&service);
            let storage = Arc::clone(&storage);
            std::thread::spawn(move || {
                service.save(&storage, provider, "synthetic-fixture", |_| {
                    published.send(()).unwrap();
                    released.recv().unwrap();
                })
            })
        };
        publication.recv().unwrap();
        let (started, starting) = mpsc::channel();
        let (finished, completion) = mpsc::channel();
        let removing = {
            let service = Arc::clone(&service);
            let storage = Arc::clone(&storage);
            std::thread::spawn(move || {
                started.send(()).unwrap();
                let result = service.remove(&storage, provider, |_| {});
                finished.send(()).unwrap();
                result
            })
        };
        starting.recv().unwrap();
        service
            .save(
                &storage,
                ProviderId::OpenAiTranscribe,
                "synthetic-fixture",
                |_| {},
            )
            .unwrap();
        let removal_waited = completion
            .recv_timeout(std::time::Duration::from_millis(30))
            .is_err();
        let before_release = store.calls();
        release.send(()).unwrap();
        saving.join().unwrap().unwrap();
        removing.join().unwrap().unwrap();
        assert!(
            removal_waited,
            "removal overtook the pending save publication"
        );
        assert!(!before_release.contains(&"delete"));
        let markers = storage.read_settings().unwrap().settings.provider_key_saved;
        assert!(!markers[provider.as_str()]);
        assert!(markers[ProviderId::OpenAiTranscribe.as_str()]);
        drop(storage);
        cleanup(&path);
    }

    fn marker(storage: &crate::storage::EchoStorage, provider: ProviderId) -> bool {
        storage.read_settings().unwrap().settings.provider_key_saved[provider.as_str()]
    }

    fn stored(store: &CountingStore, provider: ProviderId) -> Option<String> {
        store
            .0
            .lock()
            .unwrap()
            .values
            .get(&(service_name().into(), provider.as_str().into()))
            .cloned()
    }

    // Keep: real SQLite commits prove adoption publishes only after the marker is durable.
    #[test]
    fn credentials_find_adopts_a_valid_item_and_commits_before_publication() {
        for provider in ProviderId::ALL {
            let path = test_database("echo-credential-find");
            let storage = initialized(&path);
            let store = CountingStore::default();
            let service = CredentialService::new(store.clone());
            store
                .write(service_name(), provider, "  synthetic-fixture  ")
                .unwrap();
            store.reset_calls();
            let publications = std::cell::Cell::new(0);
            assert_eq!(
                service.find(&storage, provider, |update| {
                    assert!(marker(&storage, provider));
                    assert!(update.current.settings.provider_key_saved[provider.as_str()]);
                    publications.set(publications.get() + 1);
                }),
                Ok(FindOutcome::Found)
            );
            assert_eq!(publications.get(), 1);
            assert_eq!(store.calls(), ["construct", "read"]);
            assert_eq!(
                stored(&store, provider).as_deref(),
                Some("  synthetic-fixture  ")
            );
            for other in ProviderId::ALL
                .into_iter()
                .filter(|other| *other != provider)
            {
                assert!(!marker(&storage, other));
            }
            assert_eq!(
                service
                    .resolve(&storage, provider, |_| panic!("adopted marker republished"))
                    .unwrap(),
                "synthetic-fixture"
            );
            drop(storage);
            cleanup(&path);
        }
    }

    #[test]
    fn credentials_find_without_an_adoptable_item_changes_neither_metadata_nor_store() {
        for item in ["missing", "empty", "oversize", "denied"] {
            let path = test_database("echo-credential-find-none");
            let storage = initialized(&path);
            let provider = ProviderId::GroqWhisperTurbo;
            let store = CountingStore::default();
            let service = CredentialService::new(store.clone());
            let value = match item {
                "empty" => Some(" \t\n ".to_string()),
                "oversize" => Some("x".repeat(MAX_API_KEY_BYTES + 1)),
                "denied" => Some("synthetic-fixture".to_string()),
                _ => None,
            };
            if let Some(value) = &value {
                store.write(service_name(), provider, value).unwrap();
            }
            store.deny_reads(item == "denied");
            store.reset_calls();
            let before = storage.read_settings().unwrap().revision;
            let outcome = service
                .find(&storage, provider, |_| {
                    panic!("{item} item published metadata")
                })
                .unwrap();
            assert_eq!(
                outcome,
                match item {
                    "missing" => FindOutcome::NotFound,
                    "denied" => FindOutcome::Unavailable,
                    _ => FindOutcome::Invalid,
                }
            );
            assert_eq!(store.calls(), ["construct", "read"]);
            assert_eq!(stored(&store, provider), value);
            assert_eq!(storage.read_settings().unwrap().revision, before);
            assert!(!marker(&storage, provider));
            drop(storage);
            cleanup(&path);
        }
    }

    #[test]
    fn credentials_find_commit_failure_is_save_failed_without_publication() {
        let path = test_database("echo-credential-find-metadata");
        let storage = initialized(&path);
        let provider = ProviderId::GroqWhisperTurbo;
        let store = CountingStore::default();
        let service = CredentialService::new(store.clone());
        store
            .write(service_name(), provider, "synthetic-fixture")
            .unwrap();
        rusqlite::Connection::open(&path).unwrap().execute_batch("CREATE TRIGGER reject_metadata BEFORE UPDATE ON settings BEGIN SELECT RAISE(FAIL,'private database detail'); END;").unwrap();
        store.reset_calls();
        let outcome = service.find(&storage, provider, |_| {
            panic!("failed adoption published success")
        });
        assert_eq!(outcome, Ok(FindOutcome::SaveFailed));
        assert_eq!(
            serde_json::to_value(outcome.unwrap()).unwrap(),
            serde_json::json!("save_failed")
        );
        assert!(!marker(&storage, provider));
        assert_eq!(store.calls(), ["construct", "read"]);
        assert_eq!(
            stored(&store, provider).as_deref(),
            Some("synthetic-fixture")
        );
        drop(storage);
        cleanup(&path);
    }

    #[test]
    fn credentials_find_settings_read_failure_is_a_distinct_error_before_any_store_access() {
        let path = test_database("echo-credential-find-settings");
        let storage = initialized(&path);
        let provider = ProviderId::GroqWhisperTurbo;
        let store = CountingStore::default();
        let service = CredentialService::new(store.clone());
        store
            .write(service_name(), provider, "synthetic-fixture")
            .unwrap();
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch("DROP TABLE settings;")
            .unwrap();
        store.reset_calls();
        let error = service
            .find(&storage, provider, |_| {
                panic!("unreadable settings published")
            })
            .unwrap_err();
        assert_eq!(error, search_error());
        assert_ne!(error, metadata_error());
        assert!(!error.contains("found"));
        assert!(store.calls().is_empty());
        drop(storage);
        cleanup(&path);
    }

    // Keep: a saved marker short-circuits before any entry is constructed, including after a lost item.
    #[test]
    fn credentials_find_respects_saved_markers_removal_and_lost_items() {
        let path = test_database("echo-credential-find-marker");
        let storage = initialized(&path);
        let provider = ProviderId::GroqWhisperTurbo;
        let store = CountingStore::default();
        let service = CredentialService::new(store.clone());
        service
            .save(&storage, provider, "synthetic-fixture", |_| {})
            .unwrap();
        store.deny_reads(true);
        store.reset_calls();
        assert_eq!(
            service.find(&storage, provider, |_| panic!("saved marker republished")),
            Ok(FindOutcome::AlreadySaved)
        );
        assert!(store.calls().is_empty());
        store.deny_reads(false);
        service.remove(&storage, provider, |_| {}).unwrap();
        store.reset_calls();
        assert_eq!(
            service.find(&storage, provider, |_| panic!("removal adopted metadata")),
            Ok(FindOutcome::NotFound)
        );
        assert_eq!(store.calls(), ["construct", "read"]);
        // A crash after OS deletion but before the marker commit leaves a true marker without an item.
        service
            .save(&storage, provider, "synthetic-fixture", |_| {})
            .unwrap();
        store.delete(service_name(), provider).unwrap();
        store.reset_calls();
        assert_eq!(
            service.find(&storage, provider, |_| panic!("lost item republished")),
            Ok(FindOutcome::AlreadySaved)
        );
        assert!(store.calls().is_empty());
        assert!(marker(&storage, provider));
        assert_eq!(
            service.resolve(&storage, provider, |_| {}).unwrap_err(),
            missing_key()
        );
        assert!(!marker(&storage, provider));
        store.reset_calls();
        assert_eq!(
            service.find(&storage, provider, |_| panic!("missing item published")),
            Ok(FindOutcome::NotFound)
        );
        assert_eq!(store.calls(), ["construct", "read"]);
        drop(storage);
        cleanup(&path);
    }

    // Keep: a search waits through a pending removal publication without blocking other providers.
    #[test]
    fn credentials_find_waits_on_the_provider_lock() {
        use std::sync::{mpsc, Arc};
        let path = test_database("echo-credential-find-lock");
        let storage = Arc::new(initialized(&path));
        let store = CountingStore::default();
        let service = Arc::new(CredentialService::new(store.clone()));
        let provider = ProviderId::GroqWhisperTurbo;
        service
            .save(&storage, provider, "synthetic-fixture", |_| {})
            .unwrap();
        store.reset_calls();
        let (published, publication) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let removing = {
            let service = Arc::clone(&service);
            let storage = Arc::clone(&storage);
            std::thread::spawn(move || {
                service.remove(&storage, provider, |_| {
                    published.send(()).unwrap();
                    released.recv().unwrap();
                })
            })
        };
        publication.recv().unwrap();
        let (started, starting) = mpsc::channel();
        let (finished, completion) = mpsc::channel();
        let finding = {
            let service = Arc::clone(&service);
            let storage = Arc::clone(&storage);
            std::thread::spawn(move || {
                started.send(()).unwrap();
                let result = service.find(&storage, provider, |_| {});
                finished.send(()).unwrap();
                result
            })
        };
        starting.recv().unwrap();
        assert_eq!(
            service.find(&storage, ProviderId::OpenAiTranscribe, |_| {}),
            Ok(FindOutcome::NotFound)
        );
        let search_waited = completion
            .recv_timeout(std::time::Duration::from_millis(30))
            .is_err();
        let before_release = store.calls();
        release.send(()).unwrap();
        removing.join().unwrap().unwrap();
        assert_eq!(finding.join().unwrap(), Ok(FindOutcome::NotFound));
        assert!(
            search_waited,
            "search overtook the pending removal publication"
        );
        assert_eq!(before_release, ["construct", "delete", "construct", "read"]);
        assert_eq!(
            store.calls(),
            [
                "construct",
                "delete",
                "construct",
                "read",
                "construct",
                "read"
            ]
        );
        drop(storage);
        cleanup(&path);
    }

    #[test]
    fn credentials_service_uses_configured_desktop_identity() {
        assert_eq!(service_name(), "app.echostt.desktop");
    }

    #[test]
    fn credentials_presence_never_queries_the_store() {
        let store = CountingStore::default();
        let service = CredentialService::new(store.clone());
        for saved in [false, true] {
            for os_item in [false, true] {
                let mut settings = crate::storage::PersistedSettings::default();
                for provider in ProviderId::ALL {
                    settings
                        .provider_key_saved
                        .insert(provider.as_str().into(), saved);
                    if os_item {
                        store
                            .write(service_name(), provider, "synthetic-fixture")
                            .unwrap();
                    }
                }
                store.reset_calls();
                let present = service.presence(&settings);
                assert_eq!(present.len(), ProviderId::ALL.len());
                assert!(present.values().all(|value| *value == saved));
                assert!(
                    store.calls().is_empty(),
                    "presence accessed the credential store"
                );
            }
        }
    }

    // Keep: isolated processes prove environment values cannot supply credentials or presence.
    #[test]
    fn credentials_ignore_provider_environment_variables() {
        const CHILD: &str = "ECHO_FIXED_ENV_TEST_CHILD";
        const NAMES: [&str; 6] = [
            "GROQ_API_KEY",
            "OPENAI_API_KEY",
            "XAI_API_KEY",
            "ELEVENLABS_API_KEY",
            "DEEPGRAM_API_KEY",
            "MISTRAL_API_KEY",
        ];
        if std::env::var_os(CHILD).is_some() {
            let path = test_database("ignore-env");
            let storage = initialized(&path);
            let store = CountingStore::default();
            let service = CredentialService::new(store.clone());
            let settings = storage.read_settings().unwrap().settings;
            assert!(service.presence(&settings).values().all(|present| !present));
            for provider in ProviderId::ALL {
                assert_eq!(
                    service.resolve(&storage, provider, |_| panic!(
                        "environment changed metadata"
                    )),
                    Err(missing_key())
                );
            }
            assert!(store.calls().is_empty());
            drop(storage);
            cleanup(&path);
            return;
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        clear_child_environment(&mut child).env(CHILD, "1").args([
            "--exact",
            "services::credentials::tests::credentials_ignore_provider_environment_variables",
        ]);
        for name in NAMES {
            child.env(name, "<redacted>");
        }
        let output = child.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "environment isolation failed: {stdout}"
        );
        assert!(stdout.contains("test result: ok. 1 passed;"));
    }

    #[test]
    fn api_keys_are_trimmed_and_bounded() {
        assert_eq!(validate_api_key("  test-key  "), Ok("test-key"));
        assert!(validate_api_key("   ").is_err());
        assert!(validate_api_key(&"x".repeat(MAX_API_KEY_BYTES + 1)).is_err());
    }
    // Keep: pin native backend selection without constructing an OS credential entry.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_build_uses_the_native_keychain_backend() {
        let store = CountingStore::default();
        let _service = CredentialService::new(store.clone());
        assert!(keyring::default::default_credential_builder()
            .as_any()
            .is::<keyring::macos::MacCredentialBuilder>());
        assert!(store.calls().is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_build_uses_the_native_credential_backend() {
        let store = CountingStore::default();
        let _service = CredentialService::new(store.clone());
        assert!(keyring::default::default_credential_builder()
            .as_any()
            .is::<keyring::windows::WinCredentialBuilder>());
        assert!(store.calls().is_empty());
    }
}
