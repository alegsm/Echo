use rusqlite::{params, Connection, Transaction};

use super::models::{
    PersistedSettings, SettingsEnvelope, SettingsField, SettingsPatch, SettingsUpdate,
    StorageError, StorageResult, SETTINGS_SCHEMA_VERSION,
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
        let staged = stage(&transaction, expected_revision, patch)?;
        transaction.commit()?;
        Ok(self.finish(&connection, staged))
    }

    /// Commits a dashboard patch. A patch that changes launchAtLogin commits only after
    /// `set_login_item(new value)` succeeds, inside the same transaction and under the
    /// storage lock, so no other settings or credential write can land in between and a
    /// refused registration leaves the row and its revision unchanged. If the COMMIT
    /// itself fails, `set_login_item(previous value)` undoes the OS change; a failed undo
    /// is logged, and the dashboard's status check then reports the mismatch.
    pub fn update_settings_with_login_item<E>(
        &self,
        expected_revision: u64,
        patch: SettingsPatch,
        set_login_item: impl FnMut(bool) -> Result<(), E>,
    ) -> Result<SettingsUpdate, E>
    where
        E: From<StorageError> + std::fmt::Display,
    {
        self.update_settings_with_login_item_using(
            expected_revision,
            patch,
            set_login_item,
            |transaction| transaction.commit(),
        )
    }

    /// `commit` is the COMMIT step, replaceable so tests can make it fail.
    pub(super) fn update_settings_with_login_item_using<E>(
        &self,
        expected_revision: u64,
        patch: SettingsPatch,
        mut set_login_item: impl FnMut(bool) -> Result<(), E>,
        commit: impl FnOnce(Transaction<'_>) -> rusqlite::Result<()>,
    ) -> Result<SettingsUpdate, E>
    where
        E: From<StorageError> + std::fmt::Display,
    {
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(StorageError::from)?;
        let staged = stage(&transaction, expected_revision, patch)?;
        let login_item_changed = staged
            .changed_fields
            .contains(&SettingsField::LaunchAtLogin);
        if login_item_changed {
            // Dropping the transaction on this error rolls the row back.
            set_login_item(staged.current.settings.launch_at_login)?;
        }
        if let Err(error) = commit(transaction) {
            if login_item_changed {
                if let Err(undo) = set_login_item(staged.previous.settings.launch_at_login) {
                    log::warn!(
                        "Login item could not be restored after a failed settings commit: {undo}"
                    );
                }
            }
            return Err(StorageError::from(error).into());
        }
        Ok(self.finish(&connection, staged))
    }

    fn finish(&self, connection: &Connection, staged: StagedUpdate) -> SettingsUpdate {
        SettingsUpdate {
            cleanup_status: self.cleanup_deleted_pages(connection),
            previous: staged.previous,
            current: staged.current,
            changed_fields: staged.changed_fields,
        }
    }
}

/// A settings write made inside an open transaction, not yet committed.
struct StagedUpdate {
    previous: SettingsEnvelope,
    current: SettingsEnvelope,
    changed_fields: Vec<SettingsField>,
}

fn stage(
    transaction: &Transaction<'_>,
    expected_revision: u64,
    patch: SettingsPatch,
) -> StorageResult<StagedUpdate> {
    let previous = read(transaction)?;
    ensure_revision(expected_revision, previous.revision)?;
    let (mut next, changed_fields) = previous.settings.apply_patch(patch);
    next.validate()?;
    let current = write(transaction, previous.revision + 1, next)?;
    if changed_fields.contains(&SettingsField::HistoryRetentionDays) {
        super::retention::purge_with_current_time(
            transaction,
            current.settings.history_retention_days,
        )?;
    }
    Ok(StagedUpdate {
        previous,
        current,
        changed_fields,
    })
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
        assert_eq!(update.changed_fields, [SettingsField::ProviderKeySaved]);
        drop(storage);
        let reopened = super::super::test_support::initialized(&path);
        assert_eq!(reopened.read_settings().unwrap(), update.current);
        drop(reopened);
        super::super::test_support::cleanup(&path);
    }
}
