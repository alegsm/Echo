//! Initialized native settings reads and dashboard-only revision-checked updates.

use tauri::{Manager, WebviewWindow};

use crate::services::login_item;
use crate::storage::{
    EchoStorage, SettingsEnvelope, SettingsField, SettingsPatch, SettingsUpdate, StorageError,
};

use super::super::window_authorization::{require_dashboard, require_known_window};
use super::{events, synchronization};

#[tauri::command]
pub fn get_persisted_settings(window: WebviewWindow) -> Result<SettingsEnvelope, String> {
    read_authorized_settings(
        require_known_window(&window),
        &window.state::<crate::storage::EchoStorage>(),
    )
}

fn read_authorized_settings(
    authorization: Result<(), String>,
    storage: &crate::storage::EchoStorage,
) -> Result<SettingsEnvelope, String> {
    authorization?;
    storage.read_settings().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn update_persisted_settings(
    expected_revision: u64,
    patch: SettingsPatch,
    window: WebviewWindow,
) -> Result<SettingsUpdate, String> {
    require_dashboard(&window)?;
    let app = window.app_handle();
    commit_settings(
        &window.state::<EchoStorage>(),
        expected_revision,
        patch,
        |on| login_item::set_registered(app, on),
        |update| {
            let synchronization = synchronization::apply_settings(&window, &update.current);
            events::emit_settings(&window, update);
            let retention_event = emit_retention_change(&window, update);
            synchronization?;
            retention_event
        },
    )
}

/// The renderer matches this prefix to show the Background card's login-item alert.
const LOGIN_ITEM_FAILED: &str = "login_item_failed";

/// Commits the patch, then hands the committed update to `publish` (runtime, tray and
/// settings event). A launchAtLogin change commits only if `set_login_item` succeeds,
/// and is otherwise refused with `login_item_failed: <detail>`, no change and nothing
/// published. `publish` runs after the storage lock is released because it reads storage.
fn commit_settings(
    storage: &EchoStorage,
    expected_revision: u64,
    patch: SettingsPatch,
    mut set_login_item: impl FnMut(bool) -> Result<(), String>,
    publish: impl FnOnce(&SettingsUpdate) -> Result<(), String>,
) -> Result<SettingsUpdate, String> {
    let update = storage
        .update_settings_with_login_item(expected_revision, patch, |on| {
            set_login_item(on)
                .map_err(|detail| SaveRefusal(format!("{LOGIN_ITEM_FAILED}: {detail}")))
        })
        .map_err(|SaveRefusal(message)| message)?;
    publish(&update)?;
    Ok(update)
}

/// A refused save as the renderer receives it.
struct SaveRefusal(String);

impl From<StorageError> for SaveRefusal {
    fn from(error: StorageError) -> Self {
        Self(error.to_string())
    }
}

impl std::fmt::Display for SaveRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn emit_retention_change(window: &WebviewWindow, update: &SettingsUpdate) -> Result<(), String> {
    if !update
        .changed_fields
        .contains(&SettingsField::HistoryRetentionDays)
    {
        return Ok(());
    }
    let preview = window
        .state::<crate::storage::EchoStorage>()
        .preview_retention(update.current.settings.history_retention_days)
        .map_err(|error| error.to_string())?;
    events::emit_history_revision(window, preview.history_revision, "purge", None);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::PersistedSettings;

    // Keep: launchAtLogin commits only when the OS accepts it; a refusal returns the
    // renderer's prefix, leaves the row and revision unchanged and publishes nothing to
    // the runtime, tray or settings event; a commit publishes exactly the committed
    // update; other fields never touch the OS (AC1 to AC5 at the command seam).
    #[test]
    fn login_item_changes_commit_only_when_registration_succeeds() {
        let other_field = SettingsPatch {
            debug_logging_enabled: Some(true),
            ..Default::default()
        };
        let launch = |on| SettingsPatch {
            launch_at_login: Some(on),
            ..Default::default()
        };
        for (start_on, patch, registration, expected_calls, expected) in [
            (false, launch(true), Ok(()), vec![true], Ok(true)),
            (true, launch(false), Ok(()), vec![false], Ok(false)),
            (
                false,
                launch(true),
                Err("denied"),
                vec![true],
                Err("login_item_failed: denied"),
            ),
            (
                true,
                launch(false),
                Err("busy"),
                vec![false],
                Err("login_item_failed: busy"),
            ),
            (false, other_field.clone(), Err("unused"), vec![], Ok(false)),
            (true, other_field, Err("unused"), vec![], Ok(true)),
            (true, launch(true), Err("unused"), vec![], Ok(true)),
        ] {
            let path = crate::storage::test_support::test_database("login-item-seam");
            let storage = crate::storage::test_support::initialized(&path);
            if start_on {
                commit_settings(&storage, 0, launch(true), |_| Ok(()), |_| Ok(())).unwrap();
            }
            let before = storage.read_settings().unwrap();
            let mut calls = Vec::new();
            let mut published = Vec::new();
            let result = commit_settings(
                &storage,
                before.revision,
                patch,
                |on| {
                    calls.push(on);
                    registration.map_err(str::to_owned)
                },
                |update| {
                    published.push(update.clone());
                    Ok(())
                },
            );
            assert_eq!(calls, expected_calls);
            let after = storage.read_settings().unwrap();
            match expected {
                Ok(on) => {
                    let update = result.unwrap();
                    assert_eq!(update.current, after);
                    assert_eq!(after.revision, before.revision + 1);
                    assert_eq!(after.settings.launch_at_login, on);
                    assert_eq!(published, vec![update]);
                }
                Err(message) => {
                    assert_eq!(result.unwrap_err(), message);
                    assert_eq!(after, before);
                    assert_eq!(published, Vec::new());
                }
            }
            drop(storage);
            crate::storage::test_support::cleanup(&path);
        }
    }

    // Keep: native settings persist independently of absent, incomplete or malformed receipts.
    #[test]
    fn settings_survive_reopen_without_changing_legacy_rows() {
        for status in ["missing", "state_imported", "complete", "malformed"] {
            let receipt = match status {
                "missing" => None,
                "malformed" => Some("{broken".to_owned()),
                _ => Some(
                    serde_json::json!({
                        "migrationId": "echo.localStorage.v1", "status": status,
                        "settingsRevision": 1, "historyRevision": 1, "importedCount": 1,
                        "skippedExpiredCount": 0, "skippedDuplicateCount": 0,
                        "sourceChecksum": "fnv1a64:0000000000000000", "persistedChecksum": "unchanged",
                        "sourcePreserved": true
                    })
                    .to_string(),
                ),
            };
            let directory =
                std::env::temp_dir().join(format!("echo-settings-{}", uuid::Uuid::new_v4()));
            let path = directory.join("echo.sqlite3");
            let storage = EchoStorage::open(&path).unwrap();
            storage.initialize().unwrap();
            let initial = read_authorized_settings(Ok(()), &storage).unwrap();
            assert_eq!(initial.revision, 0);
            assert_eq!(initial.settings, PersistedSettings::default());
            if let Some(receipt) = &receipt {
                rusqlite::Connection::open(&path).unwrap().execute(
                    "INSERT INTO legacy_migrations VALUES('echo.localStorage.v1',?1,'unchanged',?2,'original')",
                    [status, receipt.as_str()],
                ).unwrap();
            }
            let saved = storage
                .update_settings(
                    0,
                    SettingsPatch {
                        debug_logging_enabled: Some(true),
                        ..Default::default()
                    },
                )
                .unwrap();
            let updated = storage
                .update_settings(
                    saved.current.revision,
                    SettingsPatch {
                        language: crate::storage::PatchValue::Value(Some("en".into())),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert!(updated.current.settings.debug_logging_enabled);
            let record: crate::storage::TranscriptRecord = serde_json::from_value(serde_json::json!({
                "id": "retained", "text": "saved native record", "createdAt": storage.current_timestamp().unwrap(),
                "durationMs": 1000, "providerId": "groq-whisper-turbo", "source": "desktop",
                "insertionMode": "copied", "audio": {"mimeType": "audio/wav", "byteSize": 4096},
                "timings": {"recordingMs": 1000.0, "blobReadyMs": 0.0, "uploadMs": 10.0, "stopToTranscriptMs": 20.0}
            })).unwrap();
            let saved_record = storage.append_transcript(record).unwrap();
            drop(storage);
            let reopened = EchoStorage::open(&path).unwrap();
            reopened.initialize().unwrap();
            assert_eq!(
                read_authorized_settings(Ok(()), &reopened).unwrap(),
                updated.current
            );
            assert_eq!(
                reopened.get_transcript("retained").unwrap(),
                saved_record.transcript
            );
            let rows: Vec<(String, String, String, String, String)> = rusqlite::Connection::open(&path).unwrap()
                .prepare("SELECT migration_id,status,source_checksum,receipt_json,updated_at FROM legacy_migrations").unwrap()
                .query_map([], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).unwrap()
                .collect::<Result<_, _>>().unwrap();
            assert_eq!(
                rows,
                receipt
                    .into_iter()
                    .map(|value| (
                        "echo.localStorage.v1".into(),
                        status.into(),
                        "unchanged".into(),
                        value,
                        "original".into()
                    ))
                    .collect::<Vec<_>>()
            );
            drop(reopened);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
}
