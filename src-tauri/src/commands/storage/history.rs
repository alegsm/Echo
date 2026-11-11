//! Dashboard history commands plus current-transcript-only overlay access.

use tauri::{Emitter, Manager, WebviewWindow};

use crate::storage::{
    HistoryCursor, HistoryMutation, HistoryPage, RetentionPreview, StoredTranscript,
};

use super::super::window_authorization::{require_dashboard, require_known_window};
use super::{access, events};

#[tauri::command]
pub fn list_persisted_history(
    limit: u32,
    cursor: Option<HistoryCursor>,
    window: WebviewWindow,
) -> Result<HistoryPage, String> {
    require_dashboard(&window)?;
    let page = window
        .state::<crate::storage::EchoStorage>()
        .list_history(limit, cursor)
        .map_err(|error| error.to_string())?;
    if page.purged {
        let synchronization = reconcile_current_after_clear(&window);
        events::emit_history_revision(&window, page.revision, "purge", None);
        synchronization?;
    }
    Ok(page)
}

#[tauri::command]
pub fn get_persisted_transcript(
    id: String,
    window: WebviewWindow,
) -> Result<Option<StoredTranscript>, String> {
    require_known_window(&window)?;
    access::require_record_access(&window, &id)?;
    window
        .state::<crate::storage::EchoStorage>()
        .get_transcript(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn delete_persisted_transcript(
    id: String,
    expected_record_revision: u64,
    window: WebviewWindow,
) -> Result<HistoryMutation, String> {
    require_dashboard(&window)?;
    let mutation = window
        .state::<crate::storage::EchoStorage>()
        .delete_transcript(&id, expected_record_revision)
        .map_err(|error| error.to_string())?;
    let synchronization = clear_current_transcript(&window, &id);
    events::emit_history(&window, &mutation, "delete");
    synchronization?;
    Ok(mutation)
}

#[tauri::command]
pub fn clear_persisted_history(
    expected_history_revision: u64,
    window: WebviewWindow,
) -> Result<HistoryMutation, String> {
    require_dashboard(&window)?;
    let mutation = window
        .state::<crate::storage::EchoStorage>()
        .clear_history(expected_history_revision)
        .map_err(|error| error.to_string())?;
    let synchronization = reconcile_current_after_clear(&window);
    events::emit_history(&window, &mutation, "clear");
    synchronization?;
    Ok(mutation)
}

#[tauri::command]
pub fn preview_history_retention(
    retention_days: Option<u32>,
    window: WebviewWindow,
) -> Result<RetentionPreview, String> {
    require_dashboard(&window)?;
    window
        .state::<crate::storage::EchoStorage>()
        .preview_retention(retention_days)
        .map_err(|error| error.to_string())
}

fn clear_current_transcript(window: &WebviewWindow, id: &str) -> Result<(), String> {
    let envelope = window
        .state::<crate::services::runtime::EchoRuntime>()
        .clear_last_transcript_if_current(id)
        .map_err(|_| "runtime_unavailable_after_commit".to_string())?;
    emit_optional_state(window, envelope);
    Ok(())
}

fn reconcile_current_after_clear(window: &WebviewWindow) -> Result<(), String> {
    let runtime = window.state::<crate::services::runtime::EchoRuntime>();
    let current_id = runtime
        .desktop_snapshot()
        .map_err(|_| "runtime_unavailable_after_commit".to_string())?
        .snapshot
        .last_transcript
        .map(|record| record.id);
    let Some(id) = current_id else {
        return Ok(());
    };
    if window
        .state::<crate::storage::EchoStorage>()
        .get_transcript(&id)
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Ok(());
    }
    let envelope = runtime
        .clear_last_transcript_if_current(&id)
        .map_err(|_| "runtime_unavailable_after_commit".to_string())?;
    emit_optional_state(window, envelope);
    Ok(())
}

fn emit_optional_state(
    window: &WebviewWindow,
    envelope: Option<crate::services::runtime::DesktopStateEnvelope>,
) {
    if let Some(envelope) = envelope {
        let _ = window.app_handle().emit("echo-desktop-state", envelope);
    }
}
