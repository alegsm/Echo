use super::test_support::{cleanup, initialized, test_database, transcript};
use super::*;

// Keep: opening current storage must not follow redirected live artifacts.
#[cfg(unix)]
#[test]
fn storage_open_refuses_symlinked_directory_database_and_sidecars() {
    use std::os::unix::fs::symlink;
    for leaf in [
        "directory",
        "echo.sqlite3",
        "echo.sqlite3-wal",
        "echo.sqlite3-shm",
        "echo.sqlite3-journal",
    ] {
        for dangling in [false, true] {
            let path = test_database("redirected-storage");
            let root = path.parent().unwrap();
            std::fs::create_dir_all(root).unwrap();
            let target = root.join("target");
            if !dangling {
                if leaf == "directory" {
                    std::fs::create_dir(&target).unwrap();
                } else {
                    std::fs::write(&target, b"").unwrap();
                }
            }
            let database = if leaf == "directory" {
                symlink(&target, root.join("current")).unwrap();
                root.join("current/echo.sqlite3")
            } else {
                symlink(&target, root.join(leaf)).unwrap();
                path.clone()
            };
            assert!(
                EchoStorage::open(database).is_err(),
                "followed {leaf}, dangling={dangling}"
            );
            if !dangling && leaf != "directory" {
                assert!(std::fs::read(&target).unwrap().is_empty());
            }
            cleanup(&path);
        }
    }
}

// Keep: scan real database and WAL bytes while the production connection is open.
#[test]
fn secure_deletion_overwrites_pages_and_truncates_wal_for_every_purge_path() {
    for action in [
        "clear",
        "delete",
        "startup",
        "list",
        "settings",
        "empty-clear",
    ] {
        let path = test_database(action);
        let storage = initialized(&path);
        let marker = format!("private-transcript-{action}-marker");
        let mut old = transcript("expired", "2000-01-01T00:00:00Z");
        old.text = marker.repeat(500);
        storage.append_transcript(old).unwrap();
        let retained = transcript("retained", "9998-01-01T00:00:00Z");
        if !matches!(action, "clear" | "delete" | "empty-clear") {
            storage.append_transcript(retained.clone()).unwrap();
        }
        // Put private text in database pages as well as WAL frames.
        storage
            .lock()
            .unwrap()
            .execute_batch("PRAGMA wal_checkpoint(FULL)")
            .unwrap();
        assert!(std::fs::read(&path)
            .unwrap()
            .windows(marker.len())
            .any(|w| w == marker.as_bytes()));
        match action {
            "clear" => {
                storage.clear_history(1).unwrap();
            }
            "delete" => {
                storage.delete_transcript("expired", 1).unwrap();
            }
            "empty-clear" => {
                storage.delete_transcript("expired", 1).unwrap();
                let result = storage.clear_history(2).unwrap();
                assert_eq!(result.history_revision, 2);
                assert_eq!(result.affected_count, 0);
            }
            "startup" => {
                storage.initialize().unwrap();
            }
            "list" => {
                storage.list_history(10, None).unwrap();
            }
            "settings" => {
                let result = storage
                    .update_settings(
                        0,
                        SettingsPatch {
                            history_retention_days: PatchValue::Value(Some(7)),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                assert_eq!(result.current.revision, 1);
            }
            _ => unreachable!(),
        }
        for suffix in ["", "-wal"] {
            let bytes = std::fs::read(format!("{}{suffix}", path.display())).unwrap();
            assert!(
                !bytes.windows(marker.len()).any(|w| w == marker.as_bytes()),
                "{action}: {suffix}"
            );
            if suffix == "-wal" {
                assert!(bytes.is_empty(), "{action}: WAL not truncated");
            }
        }
        if matches!(action, "startup" | "list" | "settings") {
            assert_eq!(
                storage.get_transcript("retained").unwrap().unwrap().record,
                retained
            );
            assert_eq!(storage.list_history(10, None).unwrap().revision, 3);
        }
        drop(storage);
        cleanup(&path);
    }
}

#[test]
fn secure_delete_is_enabled_before_initialization_on_new_and_existing_databases() {
    let path = test_database("secure-open");
    for _ in 0..2 {
        let storage = EchoStorage::open(&path).unwrap();
        let enabled: u32 = storage
            .lock()
            .unwrap()
            .pragma_query_value(None, "secure_delete", |r| r.get(0))
            .unwrap();
        assert_eq!(enabled, 1);
        storage.initialize().unwrap();
    }
    cleanup(&path);
}

// Keep: SQLite itself rejects or ignores the setting, rather than a mocked open.
#[test]
fn secure_delete_failure_blocks_initialization_even_when_sqlite_silently_ignores_it() {
    use rusqlite::ffi;
    use std::ffi::{c_char, c_int, c_void, CStr};
    unsafe extern "C" fn intercept(
        data: *mut c_void,
        action: c_int,
        name: *const c_char,
        value: *const c_char,
        _: *const c_char,
        _: *const c_char,
    ) -> c_int {
        if action == ffi::SQLITE_PRAGMA && !name.is_null() && !value.is_null() {
            // SAFETY: SQLite provides NUL-terminated pragma names valid during the callback.
            if unsafe { CStr::from_ptr(name) }.to_bytes() == b"secure_delete" {
                // SAFETY: the test keeps this c_int alive until the authorizer is removed.
                return unsafe { *data.cast::<c_int>() };
            }
        }
        ffi::SQLITE_OK
    }
    for mut mode in [ffi::SQLITE_DENY, ffi::SQLITE_IGNORE] {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "secure_delete", "OFF")
            .unwrap();
        // SAFETY: the connection is live and used synchronously on this thread.
        // mode outlives all callbacks and is detached before leaving the iteration.
        let result = unsafe {
            ffi::sqlite3_set_authorizer(
                connection.handle(),
                Some(intercept),
                (&mut mode as *mut c_int).cast(),
            )
        };
        assert_eq!(result, ffi::SQLITE_OK);
        let result = database::enable_secure_delete(&connection);
        // SAFETY: removing the authorizer from the same live connection ends its borrow of mode.
        unsafe {
            ffi::sqlite3_set_authorizer(connection.handle(), None, std::ptr::null_mut());
        }
        assert_eq!(result, Err(StorageError::Database));
    }
}

// Keep: a real second reader blocks TRUNCATE after commit, then the next purge retries.
#[test]
fn checkpoint_busy_returns_committed_state_and_retries_without_revision_changes() {
    for action in ["clear", "delete", "settings", "list", "startup"] {
        let path = test_database("checkpoint-busy");
        let storage = initialized(&path);
        storage
            .lock()
            .unwrap()
            .busy_timeout(std::time::Duration::from_millis(1))
            .unwrap();
        storage
            .append_transcript(transcript("expired", "2000-01-01T00:00:00Z"))
            .unwrap();
        let reader = rusqlite::Connection::open(&path).unwrap();
        reader
            .execute_batch("BEGIN; SELECT * FROM transcripts;")
            .unwrap();
        let result = match action {
            "clear" => serde_json::to_value(storage.clear_history(1).unwrap()).unwrap(),
            "delete" => {
                serde_json::to_value(storage.delete_transcript("expired", 1).unwrap()).unwrap()
            }
            "settings" => serde_json::to_value(
                storage
                    .update_settings(
                        0,
                        SettingsPatch {
                            history_retention_days: PatchValue::Value(Some(7)),
                            ..Default::default()
                        },
                    )
                    .unwrap(),
            )
            .unwrap(),
            "list" => serde_json::to_value(storage.list_history(10, None).unwrap()).unwrap(),
            "startup" => serde_json::json!({"cleanupStatus": storage.initialize().unwrap()}),
            _ => unreachable!(),
        };
        assert_eq!(result["cleanupStatus"], "pending", "{action}");
        assert!(storage.get_transcript("expired").unwrap().is_none());
        let empty = storage.clear_history(2).unwrap();
        assert_eq!(empty.affected_count, 0);
        assert_eq!(empty.history_revision, 2);
        assert_eq!(
            serde_json::to_value(empty).unwrap()["cleanupStatus"],
            "pending"
        );
        reader.execute_batch("COMMIT").unwrap();
        let page = storage.list_history(10, None).unwrap();
        assert_eq!(page.revision, 2);
        assert_eq!(
            serde_json::to_value(page).unwrap()["cleanupStatus"],
            "complete"
        );
        assert_eq!(
            std::fs::metadata(format!("{}-wal", path.display()))
                .unwrap()
                .len(),
            0
        );
        drop(reader);
        drop(storage);
        cleanup(&path);
    }
}

// Keep: each Background combination commits one field at a time, reports only real changes and survives reopening.
#[test]
fn settings_persist_across_restart_and_reject_stale_windows() {
    for (run_in_background, launch_at_login) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        let path = test_database("settings");
        let expected = {
            let storage = initialized(&path);
            let initial = storage.read_settings().unwrap();
            assert_eq!(initial.revision, 0);
            assert!(!initial.settings.run_in_background);
            assert!(!initial.settings.launch_at_login);
            let update = storage
                .update_settings(
                    0,
                    SettingsPatch {
                        debug_logging_enabled: Some(true),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert_eq!(update.current.revision, 1);
            assert!(update.current.settings.debug_logging_enabled);
            assert_eq!(
                storage
                    .update_settings(0, SettingsPatch::default())
                    .unwrap_err(),
                StorageError::RevisionConflict {
                    resource: "settings",
                    expected: 0,
                    actual: 1
                }
            );
            let mut current = update.current;
            for (field, value) in [
                (SettingsField::RunInBackground, run_in_background),
                (SettingsField::LaunchAtLogin, launch_at_login),
            ] {
                current = commit_background(&storage, current, field, value);
            }
            current
        };
        let reopened = initialized(&path);
        let mut current = reopened.read_settings().unwrap();
        assert_eq!(current, expected);
        assert_eq!(
            (
                current.settings.run_in_background,
                current.settings.launch_at_login
            ),
            (run_in_background, launch_at_login)
        );
        assert!(current.settings.debug_logging_enabled);
        for field in [SettingsField::RunInBackground, SettingsField::LaunchAtLogin] {
            current = commit_background(&reopened, current, field, false);
        }
        drop(reopened);
        cleanup(&path);
    }
}

/// Commits a one-field Background patch, as the dashboard does, and checks that
/// only that field changed and that changedFields names it only when it changed.
fn commit_background(
    storage: &EchoStorage,
    current: SettingsEnvelope,
    field: SettingsField,
    value: bool,
) -> SettingsEnvelope {
    let mut patch = SettingsPatch::default();
    let mut expected = current.settings.clone();
    let previous = match field {
        SettingsField::RunInBackground => {
            patch.run_in_background = Some(value);
            std::mem::replace(&mut expected.run_in_background, value)
        }
        SettingsField::LaunchAtLogin => {
            patch.launch_at_login = Some(value);
            std::mem::replace(&mut expected.launch_at_login, value)
        }
        _ => unreachable!(),
    };
    let update = storage.update_settings(current.revision, patch).unwrap();
    assert_eq!(update.current.revision, current.revision + 1);
    assert_eq!(update.current.settings, expected);
    let changed = if previous == value {
        vec![]
    } else {
        vec![field]
    };
    assert_eq!(update.changed_fields, changed);
    update.current
}

#[test]
fn history_actions_use_collection_and_record_revisions() {
    let path = test_database("history-crud");
    let storage = initialized(&path);
    let first = storage
        .append_transcript(transcript("first", "9998-09-09T10:00:00Z"))
        .unwrap();
    assert_eq!(first.history_revision, 1);
    let second = storage
        .append_transcript(transcript("second", "9998-09-09T11:00:00Z"))
        .unwrap();
    assert_eq!(second.history_revision, 2);
    let page = storage.list_history(10, None).unwrap();
    assert_eq!(page.total_count, 2);
    assert_eq!(
        page.records
            .iter()
            .map(|value| value.record.id.as_str())
            .collect::<Vec<_>>(),
        ["second", "first"]
    );

    let delivered = storage
        .update_transcript_delivery("first", 1, "pasted", None)
        .unwrap();
    let delivered = delivered.transcript.unwrap();
    assert_eq!(delivered.revision, 2);
    assert_eq!(delivered.record.text, "hello");
    assert_eq!(
        storage
            .update_transcript_delivery("first", 2, "failed", Some("provider_failed"))
            .unwrap_err(),
        StorageError::Validation("transcript.delivery")
    );
    assert!(matches!(
        storage.delete_transcript("first", 1),
        Err(StorageError::RevisionConflict { .. })
    ));
    let deletion = storage.delete_transcript("first", 2).unwrap();
    assert_eq!(deletion.record_id.as_deref(), Some("first"));
    assert_eq!(deletion.affected_count, 1);
    assert!(matches!(
        storage.clear_history(2),
        Err(StorageError::RevisionConflict { .. })
    ));
    let current = storage.list_history(10, None).unwrap().revision;
    storage.clear_history(current).unwrap();
    assert!(storage.list_history(10, None).unwrap().records.is_empty());
    cleanup(&path);
}

#[test]
fn identical_append_is_idempotent_and_different_duplicate_conflicts() {
    let path = test_database("idempotency");
    let storage = initialized(&path);
    let record = transcript("same", "2026-09-09T10:00:00Z");
    storage.append_transcript(record.clone()).unwrap();
    let repeated = storage.append_transcript(record.clone()).unwrap();
    assert_eq!(repeated.history_revision, 1);
    let mut conflicting = record;
    conflicting.text = "different".into();
    assert_eq!(
        storage.append_transcript(conflicting).unwrap_err(),
        StorageError::TranscriptConflict
    );
    cleanup(&path);
}

#[test]
fn retention_purges_only_expired_records_and_increments_once() {
    let path = test_database("retention");
    let storage = initialized(&path);
    storage
        .append_transcript(transcript("old", "2000-01-01T00:00:00Z"))
        .unwrap();
    storage
        .append_transcript(transcript("new", "9998-09-08T00:00:00Z"))
        .unwrap();
    let preview = storage.preview_retention(Some(3650)).unwrap();
    assert_eq!((preview.delete_count, preview.retain_count), (1, 1));
    let revision = storage
        .purge_history(Some(30), "2026-09-09T00:00:00Z")
        .unwrap();
    assert_eq!(revision, 3);
    let records = storage.list_history(10, None).unwrap().records;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].record.id, "new");
    cleanup(&path);
}

#[test]
fn startup_applies_persisted_retention_before_the_first_read() {
    let path = test_database("startup-retention");
    {
        let storage = initialized(&path);
        storage
            .append_transcript(transcript("old", "2000-01-01T00:00:00Z"))
            .unwrap();
        storage
            .append_transcript(transcript("new", "9998-01-01T00:00:00Z"))
            .unwrap();
    }
    let reopened = initialized(&path);
    let page = reopened.list_history(10, None).unwrap();
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].record.id, "new");
    cleanup(&path);
}

#[test]
fn validation_rejects_secrets_unknown_providers_and_oversized_audio() {
    let secret_settings = serde_json::json!({
        "pushToTalkShortcut":"Fn","providerId":"groq-whisper-turbo",
        "providerApiKeys":{"groq-whisper-turbo":"secret"}
    });
    assert!(serde_json::from_value::<PersistedSettings>(secret_settings).is_err());
    let mut settings = PersistedSettings {
        provider_id: "unknown".into(),
        ..Default::default()
    };
    assert_eq!(
        settings.validate().unwrap_err(),
        StorageError::Validation("providerId")
    );
    let mut record = transcript("large", "2026-09-09T10:00:00Z");
    record.audio.byte_size = MAX_AUDIO_BYTES + 1;
    assert_eq!(
        record.validate().unwrap_err(),
        StorageError::Validation("transcript.audio.byteSize")
    );
}

#[test]
fn settings_patch_distinguishes_missing_null_and_empty_optional_values() {
    let patch: SettingsPatch = serde_json::from_value(serde_json::json!({
        "language": null,
        "customPrompt": "   ",
        "fallbackShortcut": "",
        "historyRetentionDays": null
    }))
    .unwrap();
    let initial = PersistedSettings {
        language: Some("en".into()),
        custom_prompt: Some("prompt".into()),
        fallback_shortcut: Some("shortcut".into()),
        ..Default::default()
    };
    let (mut settings, changed) = initial.apply_patch(patch);
    settings.validate().unwrap();
    assert_eq!(settings.language, None);
    assert_eq!(settings.custom_prompt, None);
    assert_eq!(settings.fallback_shortcut, None);
    assert_eq!(settings.history_retention_days, None);
    assert!(changed.contains(&SettingsField::Language));
    let missing: SettingsPatch = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(missing.language.is_missing());
}

#[cfg(unix)]
#[test]
fn database_directory_and_file_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let path = test_database("permissions");
    let storage = initialized(&path);
    assert_eq!(
        std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    drop(storage);
    cleanup(&path);
}

#[test]
fn initialization_rejects_unknown_schema_versions() {
    let path = test_database("future-schema");
    let storage = initialized(&path);
    drop(storage);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE schema_meta SET value='2' WHERE key='schema_version'",
            [],
        )
        .unwrap();
    drop(connection);
    let reopened = EchoStorage::open(&path).unwrap();
    assert_eq!(
        reopened.initialize().unwrap_err(),
        StorageError::Validation("schemaVersion")
    );
    cleanup(&path);
}

// Keep: real saved rows tolerate only the retired fields, without read-time rewrites.
#[test]
fn retired_settings_rows_load_save_and_reopen_without_resetting_live_values() {
    for retired in [
        serde_json::json!({}),
        serde_json::json!({"turboModeEnabled": true}),
        serde_json::json!({"providerEnvVarNames": {"groq-whisper-turbo": "GROQ_API_KEY"}}),
        serde_json::json!({"turboModeEnabled": [null, {}], "providerEnvVarNames": 42}),
        serde_json::json!({"turboModeEnabled": null, "providerEnvVarNames": [false]}),
    ] {
        let path = test_database("retired-settings");
        let storage = initialized(&path);
        let expected = PersistedSettings {
            provider_id: "xai-stt".into(),
            push_to_talk_shortcut: "Alt+Space".into(),
            phrasebook_terms: vec!["Echo".into()],
            language: Some("es".into()),
            custom_prompt: Some("Saved instruction".into()),
            restore_clipboard: false,
            ..Default::default()
        };
        let mut old = serde_json::to_value(&expected).unwrap();
        old.as_object_mut().unwrap().remove("turboModeEnabled");
        old.as_object_mut().unwrap().remove("providerEnvVarNames");
        // A 0.3.2 row predates the Background settings, which must read as off.
        old.as_object_mut().unwrap().remove("runInBackground");
        old.as_object_mut().unwrap().remove("launchAtLogin");
        old.as_object_mut()
            .unwrap()
            .extend(retired.as_object().unwrap().clone());
        let original = old.to_string();
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE settings SET revision=7,value_json=?1 WHERE id=1",
                [&original],
            )
            .unwrap();
        drop(storage);
        let storage = initialized(&path);
        let loaded = storage.read_settings().unwrap();
        let mut normalized = expected;
        normalized.validate().unwrap();
        assert_eq!(loaded.revision, 7);
        assert_eq!(loaded.settings, normalized);
        let stored: String = connection
            .query_row("SELECT value_json FROM settings WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(stored, original);
        let saved = storage
            .update_settings(
                7,
                SettingsPatch {
                    debug_logging_enabled: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(saved.current.revision, 8);
        assert!(saved.current.settings.debug_logging_enabled);
        assert_eq!(saved.changed_fields, [SettingsField::DebugLoggingEnabled]);
        let stored: String = connection
            .query_row("SELECT value_json FROM settings WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert!(json.get("turboModeEnabled").is_none());
        assert!(json.get("providerEnvVarNames").is_none());
        let version: u32 = connection
            .query_row(
                "SELECT schema_version FROM settings WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 1);
        drop(storage);
        let reopened = initialized(&path);
        assert_eq!(reopened.read_settings().unwrap(), saved.current);
        drop(reopened);
        drop(connection);
        cleanup(&path);
    }
}

// Keep: compatibility filtering must preserve storage and command trust boundaries.
#[test]
fn retired_settings_filter_keeps_live_validation_and_patch_rejection_strict() {
    let path = test_database("strict-settings");
    let storage = initialized(&path);
    let connection = rusqlite::Connection::open(&path).unwrap();
    for invalid in [
        serde_json::json!({"unrelatedUnknown": true}),
        serde_json::json!({"providerApiKeys": {}}),
        serde_json::json!({"providerId": "unknown"}),
        serde_json::json!({"pushToTalkShortcut": ""}),
        serde_json::json!({"providerKeySaved": {"unknown": true}}),
        serde_json::json!({"providerKeySaved": {"xai-stt": "true"}}),
        serde_json::json!({"providerKeyStatuses": {"unknown": "valid"}}),
        serde_json::json!({"providerKeyStatuses": {"xai-stt": "unknown"}}),
        serde_json::json!({"runInBackground": "true"}),
        serde_json::json!({"runInBackground": null}),
        serde_json::json!({"launchAtLogin": 1}),
        serde_json::json!({"launchAtLogin": null}),
    ] {
        let mut row = serde_json::to_value(PersistedSettings::default()).unwrap();
        row["turboModeEnabled"] = serde_json::json!(false);
        row["providerEnvVarNames"] = serde_json::json!({});
        row.as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        connection
            .execute(
                "UPDATE settings SET value_json=?1 WHERE id=1",
                [row.to_string()],
            )
            .unwrap();
        assert!(storage.read_settings().is_err());
    }
    for patch in [
        serde_json::json!({"turboModeEnabled": false}),
        serde_json::json!({"providerEnvVarNames": {}}),
        serde_json::json!({"turboModeEnabled": null, "providerEnvVarNames": null}),
        serde_json::json!({"runInBackground": "true"}),
        serde_json::json!({"launchAtLogin": 1}),
    ] {
        assert!(serde_json::from_value::<SettingsPatch>(patch).is_err());
    }
    // As for the other booleans, a null patch value means the field is omitted.
    assert_eq!(
        serde_json::from_value::<SettingsPatch>(
            serde_json::json!({"runInBackground": null, "launchAtLogin": null})
        )
        .unwrap(),
        SettingsPatch::default()
    );
    drop(connection);
    drop(storage);
    cleanup(&path);
}

#[test]
fn legacy_webm_mime_and_calendar_validity_match_installed_data() {
    let mut legacy = transcript("legacy", "2026-09-09T10:00:00.123-05:00");
    legacy.audio.mime_type = "audio/webm; codecs=opus".into();
    legacy.validate().unwrap();
    assert_eq!(legacy.audio.mime_type, "audio/webm; codecs=opus");
    for mime in ["audio/ogg;codecs=opus", "audio/mp4; codecs=mp4a.40.2"] {
        legacy.audio.mime_type = mime.into();
        legacy.validate().unwrap();
    }
    let mut invalid = transcript("invalid-date", "2026-02-31T10:00:00Z");
    assert_eq!(
        invalid.validate().unwrap_err(),
        StorageError::Validation("transcript.createdAt")
    );
    for created_at in ["2026-09-09T10:00:60Z", "2026-09-09T10:00:00Zé"] {
        let mut invalid = transcript("invalid-time", created_at);
        assert_eq!(
            invalid.validate().unwrap_err(),
            StorageError::Validation("transcript.createdAt")
        );
    }
}

// Keep: schema v1 receipts remain opaque through initialization and native writes.
#[test]
fn initialization_and_native_writes_preserve_every_legacy_receipt_column() {
    let path = test_database("inert-receipts");
    let storage = initialized(&path);
    let rows = [
        (
            "complete",
            "complete",
            "old-checksum",
            "{ \"status\": \"complete\" }",
            "original",
        ),
        (
            "incomplete",
            "state_imported",
            "unfinished",
            "{}",
            "earlier",
        ),
        (
            "malformed",
            "unknown",
            "invalid",
            "{broken",
            "not-a-timestamp",
        ),
    ];
    for row in rows {
        storage
            .lock()
            .unwrap()
            .execute("INSERT INTO legacy_migrations VALUES(?1,?2,?3,?4,?5)", row)
            .unwrap();
    }
    let settings = storage
        .update_settings(
            0,
            SettingsPatch {
                custom_prompt: PatchValue::Value(Some("native settings".into())),
                ..Default::default()
            },
        )
        .unwrap()
        .current;
    let record = storage
        .append_transcript(transcript("retained", "9998-09-09T10:00:00Z"))
        .unwrap()
        .transcript;
    drop(storage);
    let reopened = initialized(&path);
    assert_eq!(reopened.read_settings().unwrap(), settings);
    assert_eq!(reopened.get_transcript("retained").unwrap(), record);
    let connection = reopened.lock().unwrap();
    let version: String = connection
        .query_row(
            "SELECT value FROM schema_meta WHERE key='schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, "1");
    let actual: Vec<(String, String, String, String, String)> = connection
        .prepare("SELECT migration_id,status,source_checksum,receipt_json,updated_at FROM legacy_migrations ORDER BY migration_id")
        .unwrap().query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
        .unwrap().collect::<Result<_, _>>().unwrap();
    let expected: Vec<_> = rows
        .into_iter()
        .map(|(id, status, checksum, receipt, updated)| {
            (
                id.to_owned(),
                status.to_owned(),
                checksum.to_owned(),
                receipt.to_owned(),
                updated.to_owned(),
            )
        })
        .collect();
    assert_eq!(actual, expected);
    drop(connection);
    drop(reopened);
    cleanup(&path);
}

// Keep: ordinary history reads leave live WAL frames alone when no cleanup is owed.
#[test]
fn history_pages_without_purges_preserve_wal() {
    let path = test_database("list-no-purge");
    let storage = initialized(&path);
    storage
        .append_transcript(transcript("retained", "9998-01-01T00:00:00Z"))
        .unwrap();
    let wal = format!("{}-wal", path.display());
    let before = std::fs::read(&wal).unwrap();
    assert!(!before.is_empty());
    for _ in 0..2 {
        let page = storage.list_history(10, None).unwrap();
        assert!(!page.purged);
        assert_eq!(page.cleanup_status, CleanupStatus::Complete);
        assert_eq!(page.records.len(), 1);
        assert!(
            std::fs::read(&wal).unwrap() == before,
            "history read changed the WAL"
        );
    }
    drop(storage);
    cleanup(&path);
}

// Keep: real legacy rows infer presence once; explicit metadata survives status changes and restart.
#[test]
fn credentials_saved_marker_upgrades_legacy_rows_and_preserves_explicit_false() {
    use crate::providers::ProviderId;
    for mode in ["literal", "env"] {
        for status in [None, Some("valid"), Some("unchecked"), Some("invalid")] {
            let path = test_database("saved-marker");
            let storage = initialized(&path);
            let connection = rusqlite::Connection::open(&path).unwrap();
            let mut row = serde_json::to_value(PersistedSettings::default()).unwrap();
            row.as_object_mut().unwrap().remove("providerKeySaved");
            row["providerCredentialModes"] = serde_json::json!({});
            for provider in ProviderId::ALL {
                row["providerCredentialModes"][provider.as_str()] = serde_json::json!(mode);
                if let Some(status) = status {
                    row["providerKeyStatuses"][provider.as_str()] = serde_json::json!(status);
                }
            }
            let original = row.to_string();
            connection
                .execute("UPDATE settings SET revision=7,value_json=?1", [&original])
                .unwrap();
            let loaded = storage.read_settings().unwrap();
            let json = serde_json::to_value(&loaded.settings).unwrap();
            for provider in ProviderId::ALL {
                assert_eq!(
                    json["providerKeySaved"][provider.as_str()],
                    mode == "literal" && status.is_some()
                );
            }
            assert_eq!(loaded.revision, 7);
            let raw: String = connection
                .query_row("SELECT value_json FROM settings", [], |r| r.get(0))
                .unwrap();
            assert_eq!(raw, original, "read must not rewrite the legacy row");
            let saved = storage
                .update_settings(
                    7,
                    SettingsPatch {
                        debug_logging_enabled: Some(true),
                        ..Default::default()
                    },
                )
                .unwrap();
            let mut explicit = serde_json::to_value(&saved.current.settings).unwrap();
            for provider in ProviderId::ALL {
                explicit["providerKeySaved"][provider.as_str()] = false.into();
            }
            connection
                .execute("UPDATE settings SET value_json=?1", [explicit.to_string()])
                .unwrap();
            drop(storage);
            let reopened = initialized(&path);
            let json = serde_json::to_value(reopened.read_settings().unwrap().settings).unwrap();
            assert_eq!(json["providerKeySaved"], explicit["providerKeySaved"]);
            drop(reopened);
            drop(connection);
            cleanup(&path);
        }
    }
    assert!(serde_json::from_value::<SettingsPatch>(
        serde_json::json!({"providerKeySaved": {"groq-whisper-turbo": true}})
    )
    .is_err());
}

// Keep: the owner's revision-11 row and malformed retired data survive without a read-time write.
#[test]
fn credentials_retire_modes_after_legacy_inference_and_reject_renderer_modes() {
    for modes in [
        serde_json::json!({"groq-whisper-turbo": "literal", "xai-stt": "env"}),
        serde_json::json!({"unknown": "invalid"}),
        serde_json::json!([null, 42]),
    ] {
        let path = test_database("retire-modes");
        let storage = initialized(&path);
        let connection = rusqlite::Connection::open(&path).unwrap();
        let mut row = serde_json::to_value(PersistedSettings::default()).unwrap();
        row["providerCredentialModes"] = modes;
        row["providerKeySaved"]["groq-whisper-turbo"] = true.into();
        row["providerKeyStatuses"]["groq-whisper-turbo"] = "valid".into();
        let original = row.to_string();
        connection
            .execute("UPDATE settings SET revision=11,value_json=?1", [&original])
            .unwrap();
        let loaded = storage.read_settings().unwrap();
        assert_eq!(loaded.revision, 11);
        assert!(loaded.settings.provider_key_saved["groq-whisper-turbo"]);
        assert_eq!(
            loaded.settings.provider_key_statuses["groq-whisper-turbo"],
            "valid"
        );
        assert!(serde_json::to_value(&loaded.settings)
            .unwrap()
            .get("providerCredentialModes")
            .is_none());
        let raw: String = connection
            .query_row("SELECT value_json FROM settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(raw, original);
        let saved = storage
            .update_settings(
                11,
                SettingsPatch {
                    debug_logging_enabled: Some(true),
                    ..Default::default()
                },
            )
            .unwrap()
            .current;
        let raw: String = connection
            .query_row("SELECT value_json FROM settings", [], |r| r.get(0))
            .unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&raw)
            .unwrap()
            .get("providerCredentialModes")
            .is_none());
        drop(storage);
        let reopened = initialized(&path);
        assert_eq!(reopened.read_settings().unwrap(), saved);
        drop(reopened);
        drop(connection);
        cleanup(&path);
    }
}

#[test]
fn credentials_renderer_patch_rejects_retired_modes_and_private_fields() {
    for patch in [
        serde_json::json!({"providerCredentialModes": {"groq-whisper-turbo": "literal"}}),
        serde_json::json!({"providerKeySaved": {"groq-whisper-turbo": true}}),
        serde_json::json!({"providerApiKeys": {}}),
        serde_json::json!({"unknown": true}),
    ] {
        assert!(serde_json::from_value::<SettingsPatch>(patch).is_err());
    }
}

// Keep: delivery outcomes retain the provider failure attached to partial speech.
#[test]
fn partial_history_preserves_provider_error_through_delivery() {
    let path = test_database("partial-delivery");
    let storage = initialized(&path);
    let mut row = transcript("partial", "9998-09-09T10:00:00Z");
    row.text = "before [Transcription gap] after".into();
    row.error_code = Some("provider_failed".into());
    storage.append_transcript(row).unwrap();
    for (revision, mode, error) in [(1, "pasted", None), (2, "failed", Some("insertion_failed"))] {
        let updated = storage
            .update_transcript_delivery("partial", revision, mode, error)
            .unwrap()
            .transcript
            .unwrap();
        assert_eq!(
            updated.record.error_code.as_deref(),
            Some("provider_failed")
        );
        assert_eq!(updated.record.insertion_mode, mode);
    }
    drop(storage);
    cleanup(&path);
}

/// The error a login-item closure returns in these tests; storage errors pass through it.
#[derive(Debug, PartialEq)]
enum LoginItemTestError {
    Storage(StorageError),
    Refused(&'static str),
}

impl From<StorageError> for LoginItemTestError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl std::fmt::Display for LoginItemTestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

fn launch_at_login_patch(on: bool) -> SettingsPatch {
    SettingsPatch {
        launch_at_login: Some(on),
        ..Default::default()
    }
}

// Keep: a refused registration rolls back the whole transaction, including the
// retention purge staged with it, so the row, its revision and History are unchanged.
#[test]
fn login_item_refusal_rolls_back_the_row_and_its_retention_purge() {
    let path = test_database("login-item-rollback");
    let storage = initialized(&path);
    storage
        .append_transcript(transcript("expired", "2000-01-01T00:00:00Z"))
        .unwrap();
    let before = storage.read_settings().unwrap();
    let mut calls = Vec::new();
    let error = storage
        .update_settings_with_login_item(
            0,
            SettingsPatch {
                launch_at_login: Some(true),
                history_retention_days: PatchValue::Value(Some(7)),
                ..Default::default()
            },
            |on| {
                calls.push(on);
                Err(LoginItemTestError::Refused("denied"))
            },
        )
        .unwrap_err();
    assert_eq!(error, LoginItemTestError::Refused("denied"));
    assert_eq!(calls, [true]);
    assert_eq!(storage.read_settings().unwrap(), before);
    assert!(storage.get_transcript("expired").unwrap().is_some());
    drop(storage);
    cleanup(&path);
}

// Keep: a credential commit issued while registration runs waits on the storage lock
// and lands after the login-item save, whichever way registration ends.
#[test]
fn competing_credential_commit_waits_for_login_item_registration() {
    let provider = crate::providers::ProviderId::GroqWhisperTurbo;
    for registration_succeeds in [true, false] {
        let path = test_database("login-item-competing");
        let storage = initialized(&path);
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let result =
                storage.update_settings_with_login_item(0, launch_at_login_patch(true), |_| {
                    let sender = sender.clone();
                    let storage = &storage;
                    scope.spawn(move || {
                        let _ = sender.send(storage.set_provider_key_saved(provider, true));
                    });
                    assert!(
                        receiver
                            .recv_timeout(std::time::Duration::from_millis(200))
                            .is_err(),
                        "credential commit landed while registration held the row open"
                    );
                    if registration_succeeds {
                        Ok(())
                    } else {
                        Err(LoginItemTestError::Refused("denied"))
                    }
                });
            assert_eq!(result.is_ok(), registration_succeeds);
        });
        let credential = receiver.recv().unwrap().unwrap().unwrap();
        let expected_revision = if registration_succeeds { 2 } else { 1 };
        assert_eq!(credential.current.revision, expected_revision);
        assert_eq!(
            credential.current.settings.launch_at_login,
            registration_succeeds
        );
        assert!(credential.current.settings.provider_key_saved[provider.as_str()]);
        assert_eq!(storage.read_settings().unwrap(), credential.current);
        drop(storage);
        cleanup(&path);
    }
}

// Keep: when COMMIT fails after registration succeeded, the OS change is undone with
// the previous value; a failed undo still returns the storage error, not the undo's.
#[test]
fn failed_commit_undoes_the_login_item_change() {
    for (start_on, undo_succeeds) in [(false, true), (false, false), (true, true)] {
        let path = test_database("login-item-commit-fails");
        let storage = initialized(&path);
        if start_on {
            storage
                .update_settings_with_login_item(0, launch_at_login_patch(true), |_| {
                    Ok::<(), LoginItemTestError>(())
                })
                .unwrap();
        }
        let before = storage.read_settings().unwrap();
        let mut calls = Vec::new();
        let error = storage
            .update_settings_with_login_item_using(
                before.revision,
                launch_at_login_patch(!start_on),
                |on| {
                    calls.push(on);
                    if calls.len() == 2 && !undo_succeeds {
                        return Err(LoginItemTestError::Refused("undo denied"));
                    }
                    Ok(())
                },
                |transaction| {
                    drop(transaction);
                    Err(rusqlite::Error::ExecuteReturnedResults)
                },
            )
            .unwrap_err();
        assert_eq!(error, LoginItemTestError::Storage(StorageError::Database));
        assert_eq!(calls, [!start_on, start_on]);
        assert_eq!(storage.read_settings().unwrap(), before);
        drop(storage);
        cleanup(&path);
    }
}
