//! Initialized native settings reads and dashboard-only revision-checked updates.

use tauri::{Manager, WebviewWindow};

use crate::storage::{SettingsEnvelope, SettingsField, SettingsPatch, SettingsUpdate};

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
    let update = window
        .state::<crate::storage::EchoStorage>()
        .update_settings(expected_revision, patch)
        .map_err(|error| error.to_string())?;
    let synchronization = synchronization::apply_settings(&window, &update.current);
    events::emit_settings(&window, &update);
    let retention_event = emit_retention_change(&window, &update);
    synchronization?;
    retention_event?;
    Ok(update)
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
    use crate::storage::{EchoStorage, PersistedSettings};

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
