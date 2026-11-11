use std::collections::HashMap;
use std::future::Future;

use serde::Serialize;
use tauri::{Emitter, Manager, WebviewWindow};

use crate::commands::storage::events::emit_settings;
use crate::providers::ProviderId;
use crate::services::credentials;

#[tauri::command]
pub async fn set_provider_api_key(
    provider_id: String,
    api_key: String,
    window: WebviewWindow,
) -> Result<(), String> {
    let provider = provider_id.clone();
    let app = window.app_handle().clone();
    update_credential(
        super::window_authorization::require_dashboard(&window),
        CredentialsUpdated {
            action: "set",
            provider_id: &provider_id,
        },
        async move {
            tauri::async_runtime::spawn_blocking(move || {
                let provider =
                    ProviderId::try_from(provider.as_str()).map_err(|error| error.to_string())?;
                app.state::<credentials::CredentialService>().save(
                    &app.state::<crate::storage::EchoStorage>(),
                    provider,
                    &api_key,
                    |update| emit_settings(&app, update),
                )
            })
            .await
            .map_err(|_| "Keychain write failed.".to_string())?
        },
        |name, payload| {
            let _ = window.app_handle().emit(name, payload);
        },
    )
    .await
}

#[tauri::command]
pub async fn delete_provider_api_key(
    provider_id: String,
    window: WebviewWindow,
) -> Result<(), String> {
    let provider = provider_id.clone();
    let app = window.app_handle().clone();
    update_credential(
        super::window_authorization::require_dashboard(&window),
        CredentialsUpdated {
            action: "delete",
            provider_id: &provider_id,
        },
        async move {
            tauri::async_runtime::spawn_blocking(move || {
                let provider =
                    ProviderId::try_from(provider.as_str()).map_err(|error| error.to_string())?;
                app.state::<credentials::CredentialService>().remove(
                    &app.state::<crate::storage::EchoStorage>(),
                    provider,
                    |update| emit_settings(&app, update),
                )
            })
            .await
            .map_err(|_| "Keychain delete failed.".to_string())?
        },
        |name, payload| {
            let _ = window.app_handle().emit(name, payload);
        },
    )
    .await
}

/// The renderer learns only the search outcome; the key stays in Rust.
#[derive(Serialize)]
pub struct FoundCredential {
    outcome: credentials::FindOutcome,
}

#[tauri::command]
pub async fn find_provider_api_key<R: tauri::Runtime>(
    provider_id: String,
    window: WebviewWindow<R>,
) -> Result<FoundCredential, String> {
    super::window_authorization::require_dashboard(&window)?;
    let app = window.app_handle().clone();
    // The settings event refreshes presence; searches publish no credential notification.
    tauri::async_runtime::spawn_blocking(move || {
        let provider =
            ProviderId::try_from(provider_id.as_str()).map_err(|error| error.to_string())?;
        app.state::<credentials::CredentialService>().find(
            &app.state::<crate::storage::EchoStorage>(),
            provider,
            |update| emit_settings(&app, update),
        )
    })
    .await
    .map_err(|_| credentials::search_error())?
    .map(|outcome| FoundCredential { outcome })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CredentialsUpdated<'a> {
    action: &'static str,
    provider_id: &'a str,
}

async fn update_credential(
    authorization: Result<(), String>,
    notification: CredentialsUpdated<'_>,
    operation: impl Future<Output = Result<(), String>>,
    publish: impl FnOnce(&str, CredentialsUpdated<'_>),
) -> Result<(), String> {
    authorization?;
    operation.await?;
    publish("echo-credentials-updated", notification);
    Ok(())
}

#[tauri::command]
pub async fn provider_api_key_presence<R: tauri::Runtime>(
    window: WebviewWindow<R>,
) -> Result<HashMap<String, bool>, String> {
    super::window_authorization::require_dashboard(&window)?;
    let settings = window
        .state::<crate::storage::EchoStorage>()
        .read_settings()
        .map_err(|_| "Provider settings are unavailable.".to_string())?
        .settings;
    Ok(window
        .state::<credentials::CredentialService>()
        .presence(&settings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::window_authorization::WINDOW_NOT_AUTHORIZED;
    use crate::storage::test_support::{cleanup, initialized, test_database};
    use std::cell::Cell;

    // Keep: authorized command sequencing crosses the real credential service and SQLite boundary.
    #[test]
    fn credentials_commands_publish_settings_before_success_and_withhold_failed_metadata() {
        use crate::services::credentials::{tests::CountingStore, CredentialService};
        for authorized in [false, true] {
            for fails_metadata in [false, true] {
                for action in ["set", "delete"] {
                    let path = test_database("echo-credential-command");
                    let storage = initialized(&path);
                    let store = CountingStore::default();
                    let service = CredentialService::new(store.clone());
                    let provider = ProviderId::GroqWhisperTurbo;
                    if action == "delete" {
                        service
                            .save(&storage, provider, "synthetic-fixture", |_| {})
                            .unwrap();
                    }
                    if fails_metadata {
                        rusqlite::Connection::open(&path).unwrap().execute_batch("CREATE TRIGGER reject_marker BEFORE UPDATE ON settings BEGIN SELECT RAISE(FAIL,'private database detail'); END;").unwrap();
                    }
                    store.reset_calls();
                    let events = std::cell::RefCell::new(Vec::new());
                    let result = tauri::async_runtime::block_on(update_credential(
                        if authorized {
                            Ok(())
                        } else {
                            Err(WINDOW_NOT_AUTHORIZED.into())
                        },
                        CredentialsUpdated {
                            action,
                            provider_id: provider.as_str(),
                        },
                        async {
                            let publish = |_: &crate::storage::SettingsUpdate| {
                                assert_eq!(
                                    storage.read_settings().unwrap().settings.provider_key_saved
                                        [provider.as_str()],
                                    action == "set"
                                );
                                events.borrow_mut().push("settings");
                            };
                            if action == "set" {
                                service.save(&storage, provider, "synthetic-fixture", publish)
                            } else {
                                service.remove(&storage, provider, publish)
                            }
                        },
                        |_, _| {
                            events.borrow_mut().push("credentials");
                        },
                    ));
                    assert_eq!(result.is_ok(), authorized && !fails_metadata);
                    assert_eq!(
                        *events.borrow(),
                        if authorized && !fails_metadata {
                            vec!["settings", "credentials"]
                        } else {
                            vec![]
                        }
                    );
                    if !authorized {
                        assert!(store.calls().is_empty());
                    } else {
                        assert_eq!(
                            store.calls(),
                            if action == "set" {
                                vec!["construct", "write", "construct", "read"]
                            } else {
                                vec!["construct", "delete"]
                            }
                        );
                    }
                    drop(storage);
                    cleanup(&path);
                }
            }
        }
    }

    // Keep: mock windows cross the real command, credential service and SQLite boundary.
    #[test]
    fn find_command_requires_the_dashboard_and_publishes_only_settings() {
        use crate::services::credentials::{
            tests::CountingStore, CredentialService, CredentialStore,
        };
        use std::sync::{Arc, Mutex};
        use tauri::Listener;
        let path = test_database("echo-credential-find-command");
        let provider = ProviderId::GroqWhisperTurbo;
        let store = CountingStore::default();
        store
            .write("app.echostt.desktop", provider, "synthetic-fixture")
            .unwrap();
        store.reset_calls();
        let app = tauri::test::mock_builder()
            .manage(CredentialService::new(store.clone()))
            .manage(initialized(&path))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        for name in ["echo-settings-committed", "echo-credentials-updated"] {
            let events = Arc::clone(&events);
            app.listen(name, move |_| events.lock().unwrap().push(name));
        }
        let dashboard = tauri::WebviewWindowBuilder::new(&app, "dashboard", Default::default())
            .build()
            .unwrap();
        let overlay = tauri::WebviewWindowBuilder::new(&app, "overlay", Default::default())
            .build()
            .unwrap();
        let find = |window: &tauri::WebviewWindow<tauri::test::MockRuntime>, id: &str| {
            tauri::async_runtime::block_on(find_provider_api_key(id.into(), window.clone()))
                .map(|response| serde_json::to_value(response).unwrap())
        };
        assert_eq!(
            find(&overlay, provider.as_str()),
            Err(WINDOW_NOT_AUTHORIZED.into())
        );
        assert!(find(&dashboard, "retired-provider").is_err());
        assert!(store.calls().is_empty(), "denied search accessed the store");
        assert_eq!(
            find(&dashboard, provider.as_str()),
            Ok(serde_json::json!({ "outcome": "found" }))
        );
        assert_eq!(store.calls(), ["construct", "read"]);
        assert_eq!(*events.lock().unwrap(), ["echo-settings-committed"]);
        assert_eq!(
            find(&dashboard, provider.as_str()),
            Ok(serde_json::json!({ "outcome": "already_saved" }))
        );
        assert_eq!(
            find(&overlay, provider.as_str()),
            Err(WINDOW_NOT_AUTHORIZED.into())
        );
        assert_eq!(store.calls(), ["construct", "read"]);
        assert_eq!(*events.lock().unwrap(), ["echo-settings-committed"]);
        drop(dashboard);
        drop(overlay);
        drop(app);
        cleanup(&path);
    }

    #[test]
    fn find_responses_serialize_only_their_outcome() {
        use crate::services::credentials::FindOutcome;
        for (outcome, name) in [
            (FindOutcome::Found, "found"),
            (FindOutcome::NotFound, "not_found"),
            (FindOutcome::Invalid, "invalid"),
            (FindOutcome::Unavailable, "unavailable"),
            (FindOutcome::AlreadySaved, "already_saved"),
            (FindOutcome::SaveFailed, "save_failed"),
        ] {
            let response = serde_json::to_value(FoundCredential { outcome }).unwrap();
            assert_eq!(response, serde_json::json!({ "outcome": name }));
            assert_eq!(response.as_object().unwrap().len(), 1);
        }
    }

    #[test]
    fn credential_notifications_follow_success_and_never_follow_failure_or_denial() {
        tauri::async_runtime::block_on(async {
            for action in ["set", "delete"] {
                for authorized in [true, false] {
                    for succeeds in [true, false] {
                        let mutated = Cell::new(false);
                        let mut notifications = Vec::new();
                        let result = update_credential(
                            if authorized {
                                Ok(())
                            } else {
                                Err(WINDOW_NOT_AUTHORIZED.into())
                            },
                            CredentialsUpdated {
                                action,
                                provider_id: "groq-whisper-turbo",
                            },
                            async {
                                if !succeeds {
                                    return Err("Keychain operation failed.".into());
                                }
                                mutated.set(true);
                                Ok(())
                            },
                            |name, payload| {
                                assert!(mutated.get(), "notification preceded credential mutation");
                                notifications.push((
                                    name.to_owned(),
                                    serde_json::to_value(payload).unwrap(),
                                ));
                            },
                        )
                        .await;
                        assert_eq!(mutated.get(), authorized && succeeds);
                        assert_eq!(result.is_ok(), authorized && succeeds);
                        if authorized && succeeds {
                            assert_eq!(
                                notifications,
                                vec![(
                                    "echo-credentials-updated".into(),
                                    serde_json::json!({
                                        "action": action, "providerId": "groq-whisper-turbo"
                                    })
                                )]
                            );
                        } else {
                            assert!(notifications.is_empty());
                        }
                    }
                }
            }
        });
    }
}
