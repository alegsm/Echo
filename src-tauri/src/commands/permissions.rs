use tauri::Manager;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPermissionItem {
    pub status: String,
    pub granted: bool,
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DesktopPermissionsResponse {
    pub platform: &'static str,
    pub microphone: DesktopPermissionItem,
    pub accessibility: DesktopPermissionItem,
    pub input_monitoring: DesktopPermissionItem,
}

#[tauri::command]
pub fn get_desktop_permissions(
    window: tauri::WebviewWindow,
) -> Result<DesktopPermissionsResponse, String> {
    super::window_authorization::require_known_window(&window)?;
    Ok(desktop_permissions_response())
}

#[tauri::command]
pub async fn request_desktop_permission(
    window: tauri::WebviewWindow,
    kind: String,
) -> Result<DesktopPermissionsResponse, String> {
    super::window_authorization::require_dashboard(&window)?;
    tauri::async_runtime::spawn_blocking(move || request_permission(&kind))
        .await
        .map_err(|_| "Desktop permission request failed.".to_string())??;
    Ok(desktop_permissions_response())
}

fn request_permission(kind: &str) -> Result<(), String> {
    match kind {
        "microphone" => {
            crate::platform::request_microphone_permission()?;
        }
        "accessibility" | "paste" => {
            let status = crate::platform::request_accessibility_permission();
            if status != "authorized" && status != "not_required" {
                crate::platform::open_privacy_settings("accessibility")?;
            }
        }
        "input_monitoring" | "shortcuts" => {
            if crate::platform::input_monitoring_permission_status() != "not_required" {
                crate::platform::open_privacy_settings("input_monitoring")?;
            }
        }
        _ => return Err(format!("Unknown permission kind: {kind}")),
    }
    Ok(())
}

#[tauri::command]
pub fn open_desktop_permission_settings(
    window: tauri::WebviewWindow,
    kind: String,
) -> Result<(), String> {
    super::window_authorization::require_dashboard(&window)?;
    crate::platform::open_privacy_settings(&kind)
}

#[tauri::command]
pub fn change_fn_key_preference(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    action: crate::platform::FnKeyAction,
) -> Result<(), String> {
    super::window_authorization::require_dashboard(&window)?;
    crate::platform::change_fn_key_preference(&fn_key_backup_path(&app)?, action)
}

#[tauri::command]
pub fn get_fn_key_preference(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<crate::platform::FnKeyPreference, String> {
    super::window_authorization::require_dashboard(&window)?;
    crate::platform::get_fn_key_preference(&fn_key_backup_path(&app)?)
}

fn fn_key_backup_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join("fn-key-previous.json"))
        .map_err(|_| "Could not locate the Fn key restore record.".to_string())
}

fn desktop_permissions_response() -> DesktopPermissionsResponse {
    let microphone_status = crate::platform::microphone_permission_status();
    let accessibility_status = crate::platform::accessibility_permission_status();
    let input_monitoring_status = crate::platform::input_monitoring_permission_status();

    DesktopPermissionsResponse {
        platform: std::env::consts::OS,
        microphone: permission_item(
            microphone_status,
            "Echo can capture audio.",
            "Echo needs Microphone access to record dictation.",
        ),
        accessibility: permission_item(
            accessibility_status,
            "Echo can paste completed transcripts.",
            "Echo needs Accessibility access for virtual paste.",
        ),
        input_monitoring: permission_item(
            input_monitoring_status,
            "Echo can capture the Fn hotkey globally.",
            "Echo needs Input Monitoring for Fn/global hotkey capture.",
        ),
    }
}

fn permission_item(
    status: String,
    ok_message: &str,
    blocked_message: &str,
) -> DesktopPermissionItem {
    let granted = status == "authorized";
    let message = if status == "not_required" {
        "Not required on this platform."
    } else if granted {
        ok_message
    } else {
        blocked_message
    };
    DesktopPermissionItem {
        status,
        granted,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Keep: a capability outside the platform's permission model is neither consent nor a blocker.
    #[test]
    fn permissions_not_required_are_explicit_without_claiming_a_grant() {
        let item = permission_item("not_required".into(), "ready", "needs attention");
        assert!(!item.granted);
        assert_eq!(item.message, "Not required on this platform.");
    }

    // Keep: neither unknown data nor the retired unsupported status implies consent.
    #[test]
    fn microphone_permission_response_only_grants_confirmed_authorization() {
        for status in [
            "unknown",
            "unsupported",
            "denied",
            "restricted",
            "not_determined",
            "authorized",
        ] {
            let item = permission_item(status.into(), "ready", "needs attention");
            assert_eq!(item.granted, status == "authorized", "{status}");
            assert_eq!(
                item.message,
                if item.granted {
                    "ready"
                } else {
                    "needs attention"
                }
            );
        }
    }
}
