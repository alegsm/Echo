//! Dashboard-only status of the Start Echo at login record.

use serde::Serialize;
use tauri::{Manager, WebviewWindow};

use super::window_authorization::require_dashboard;
use crate::services::login_item;

#[derive(Serialize)]
pub struct LaunchAtLoginStatus {
    registered: bool,
}

/// Lets the Background card show a line when the OS and the saved setting disagree.
#[tauri::command]
pub fn get_launch_at_login_status(window: WebviewWindow) -> Result<LaunchAtLoginStatus, String> {
    require_dashboard(&window)?;
    login_item::is_registered(window.app_handle())
        .map(|registered| LaunchAtLoginStatus { registered })
}
