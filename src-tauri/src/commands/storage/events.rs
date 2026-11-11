use serde::Serialize;
use tauri::{Emitter, Manager, WebviewWindow};

use crate::commands::events::HistoryCommittedEvent;
use crate::storage::{HistoryMutation, SettingsUpdate};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsCommittedEvent<'a> {
    revision: u64,
    changed_fields: &'a [crate::storage::SettingsField],
}

pub(crate) fn emit_settings<R: tauri::Runtime>(emitter: &impl Emitter<R>, update: &SettingsUpdate) {
    let _ = emitter.emit(
        "echo-settings-committed",
        SettingsCommittedEvent {
            revision: update.current.revision,
            changed_fields: &update.changed_fields,
        },
    );
}

pub(super) fn emit_history(
    window: &WebviewWindow,
    mutation: &HistoryMutation,
    action: &'static str,
) {
    let _ = window.app_handle().emit(
        "echo-history-committed",
        HistoryCommittedEvent {
            history_revision: mutation.history_revision,
            record_id: mutation.record_id.as_deref().or_else(|| {
                mutation
                    .transcript
                    .as_ref()
                    .map(|value| value.record.id.as_str())
            }),
            action,
            affected_count: Some(mutation.affected_count),
        },
    );
}

pub(super) fn emit_history_revision(
    window: &WebviewWindow,
    history_revision: u64,
    action: &'static str,
    affected_count: Option<u64>,
) {
    let _ = window.app_handle().emit(
        "echo-history-committed",
        HistoryCommittedEvent {
            history_revision,
            record_id: None,
            action,
            affected_count,
        },
    );
}
