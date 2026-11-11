use rusqlite::{params, Connection};

use super::models::{
    PersistedSettings, SettingsEnvelope, SettingsPatch, SettingsUpdate, StorageError,
    StorageResult, SETTINGS_SCHEMA_VERSION,
};
use super::EchoStorage;

pub fn ensure_default(connection: &Connection) -> StorageResult<()> {
    let mut settings = PersistedSettings::default();
    settings.validate()?;
    let value = serde_json::to_string(&settings).map_err(|_| StorageError::Database)?;
    connection.execute(
        "INSERT OR IGNORE INTO settings(id,schema_version,revision,value_json,updated_at)
         VALUES(1,?1,0,?2,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
        params![SETTINGS_SCHEMA_VERSION, value],
    )?;
    Ok(())
}

impl EchoStorage {
    pub(crate) fn set_provider_key_saved(
        &self,
        provider: crate::providers::ProviderId,
        saved: bool,
    ) -> StorageResult<Option<SettingsUpdate>> {
        self.set_provider_key_saved_with(provider, saved, |revision, patch| {
            self.update_settings(revision, patch)
        })
    }

    fn set_provider_key_saved_with(
        &self,
        provider: crate::providers::ProviderId,
        saved: bool,
        mut commit: impl FnMut(u64, SettingsPatch) -> StorageResult<SettingsUpdate>,
    ) -> StorageResult<Option<SettingsUpdate>> {
        loop {
            let current = self.read_settings()?;
            if current.settings.provider_key_saved.get(provider.as_str()) == Some(&saved) {
                return Ok(None);
            }
            match commit(
                current.revision,
                SettingsPatch {
                    provider_key_saved: Some(
                        [(provider.as_str().into(), saved)].into_iter().collect(),
                    ),
                    ..Default::default()
                },
            ) {
                Err(StorageError::RevisionConflict { .. }) => continue,
                result => return result.map(Some),
            }
        }
    }

    pub fn read_settings(&self) -> StorageResult<SettingsEnvelope> {
        let connection = self.lock()?;
        read(&connection)
    }

    pub fn update_settings(
        &self,
        expected_revision: u64,
        patch: SettingsPatch,
    ) -> StorageResult<SettingsUpdate> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let previous = read(&transaction)?;
        ensure_revision(expected_revision, previous.revision)?;
        let (mut next, changed_fields) = previous.settings.apply_patch(patch);
        next.validate()?;
        let current = write(&transaction, previous.revision + 1, next)?;
        if changed_fields.contains(&super::models::SettingsField::HistoryRetentionDays) {
            super::retention::purge_with_current_time(
                &transaction,
                current.settings.history_retention_days,
            )?;
        }
        transaction.commit()?;
        Ok(SettingsUpdate {
            cleanup_status: self.cleanup_deleted_pages(&connection),
            previous,
            current,
            changed_fields,
        })
    }
}

pub(super) fn read(connection: &Connection) -> StorageResult<SettingsEnvelope> {
    let (revision, value): (u64, String) = connection.query_row(
        "SELECT revision,value_json FROM settings WHERE id=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let mut value: serde_json::Value =
        serde_json::from_str(&value).map_err(|_| StorageError::Database)?;
    if let Some(fields) = value.as_object_mut() {
        fields.remove("turboModeEnabled");
        fields.remove("providerEnvVarNames");
        if !fields.contains_key("providerKeySaved") {
            let saved: std::collections::BTreeMap<_, _> = crate::providers::ProviderId::ALL
                .into_iter()
                .map(|provider| {
                    let id = provider.as_str();
                    let literal = fields
                        .get("providerCredentialModes")
                        .and_then(|modes| modes.get(id))
                        .and_then(serde_json::Value::as_str)
                        == Some("literal");
                    let recorded = fields
                        .get("providerKeyStatuses")
                        .and_then(|statuses| statuses.get(id))
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|status| matches!(status, "valid" | "unchecked" | "invalid"));
                    (id, literal && recorded)
                })
                .collect();
            fields.insert("providerKeySaved".into(), serde_json::json!(saved));
        }
        fields.remove("providerCredentialModes");
    }
    let mut settings: PersistedSettings =
        serde_json::from_value(value).map_err(|_| StorageError::Database)?;
    settings.validate()?;
    Ok(SettingsEnvelope { revision, settings })
}

pub(super) fn write(
    connection: &Connection,
    revision: u64,
    settings: PersistedSettings,
) -> StorageResult<SettingsEnvelope> {
    let value = serde_json::to_string(&settings).map_err(|_| StorageError::Database)?;
    connection.execute(
        "UPDATE settings SET schema_version=?1,revision=?2,value_json=?3,
         updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=1",
        params![SETTINGS_SCHEMA_VERSION, revision, value],
    )?;
    Ok(SettingsEnvelope { revision, settings })
}

fn ensure_revision(expected: u64, actual: u64) -> StorageResult<()> {
    if expected == actual {
        return Ok(());
    }
    Err(StorageError::RevisionConflict {
        resource: "settings",
        expected,
        actual,
    })
}

#[cfg(test)]
mod credential_tests {
    use super::*;

    // Keep: a competing real settings write forces the production metadata retry path.
    #[test]
    fn credentials_marker_retries_revision_conflicts_and_preserves_other_settings() {
        let path = super::super::test_support::test_database("marker-conflict");
        let storage = super::super::test_support::initialized(&path);
        let provider = crate::providers::ProviderId::GroqWhisperTurbo;
        let mut attempts = 0;
        let update = storage
            .set_provider_key_saved_with(provider, true, |revision, patch| {
                attempts += 1;
                if attempts == 1 {
                    storage.update_settings(
                        revision,
                        SettingsPatch {
                            debug_logging_enabled: Some(true),
                            ..Default::default()
                        },
                    )?;
                }
                storage.update_settings(revision, patch)
            })
            .unwrap()
            .unwrap();
        assert_eq!(attempts, 2);
        assert_eq!(update.current.revision, 2);
        assert!(update.current.settings.debug_logging_enabled);
        assert!(update.current.settings.provider_key_saved[provider.as_str()]);
        assert_eq!(
            update.changed_fields,
            [super::super::SettingsField::ProviderKeySaved]
        );
        drop(storage);
        let reopened = super::super::test_support::initialized(&path);
        assert_eq!(reopened.read_settings().unwrap(), update.current);
        drop(reopened);
        super::super::test_support::cleanup(&path);
    }
}
