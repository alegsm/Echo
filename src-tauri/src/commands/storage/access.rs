use tauri::{Manager, WebviewWindow};

use crate::commands::window_authorization::{
    can_access_record, require_known_window, WINDOW_NOT_AUTHORIZED,
};
use crate::services::runtime::EchoRuntime;

pub(super) fn require_record_access(window: &WebviewWindow, id: &str) -> Result<(), String> {
    require_known_window(window)?;
    let snapshot = if window.label() == "overlay" {
        Some(
            window
                .state::<EchoRuntime>()
                .desktop_snapshot()
                .map_err(|_| "runtime_unavailable".to_string())?,
        )
    } else {
        None
    };
    let current_id = snapshot
        .as_ref()
        .and_then(|state| state.snapshot.last_transcript.as_ref())
        .map(|record| record.id.as_str());
    can_access_record(window.label(), current_id, id)
        .then_some(())
        .ok_or_else(|| WINDOW_NOT_AUTHORIZED.to_string())
}
