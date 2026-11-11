use tauri::{Emitter, Manager, WebviewWindow};

use crate::audio::DesktopAudioCapture;
use crate::services::runtime::{DesktopStateEnvelope, EchoRuntime};
use crate::storage::SettingsEnvelope;

pub(super) fn apply_settings(
    window: &WebviewWindow,
    settings: &SettingsEnvelope,
) -> Result<DesktopStateEnvelope, String> {
    let app = window.app_handle();
    let envelope = app
        .state::<EchoRuntime>()
        .replace_persisted_settings(settings.revision, &settings.settings)
        .map_err(|_| "runtime_unavailable_after_commit".to_string())?;
    if let Some(capture) = app.try_state::<DesktopAudioCapture>() {
        if capture
            .apply_diagnostics_setting(settings.revision, settings.settings.debug_logging_enabled)
            .is_err()
        {
            log::warn!("Native diagnostics setting could not reach the capture actor");
        }
    }
    let _ = app.emit("echo-desktop-state", &envelope);
    Ok(envelope)
}
