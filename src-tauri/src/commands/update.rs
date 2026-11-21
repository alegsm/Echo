//! Dashboard-only update commands. Neither takes a payload; both return the status
//! envelope that `echo-update-status` carries. Update requests stay in Rust.

use tauri::{Manager, Runtime, WebviewWindow};

use super::window_authorization::require_dashboard;
use crate::services::updater::{self, UpdateEnvelope, UpdaterService};

#[tauri::command]
pub fn get_update_status<R: Runtime>(window: WebviewWindow<R>) -> Result<UpdateEnvelope, String> {
    require_dashboard(&window)?;
    Ok(window.state::<UpdaterService>().status())
}

/// Returns once the request is admitted or ignored; the download continues natively.
#[tauri::command]
pub fn install_update<R: Runtime>(window: WebviewWindow<R>) -> Result<UpdateEnvelope, String> {
    require_dashboard(&window)?;
    Ok(updater::request_install(window.app_handle()))
}

#[cfg(test)]
mod tests {
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{get_ipc_response, mock_builder, MockRuntime, INVOKE_KEY};
    use tauri::webview::InvokeRequest;
    use tauri::Manager;

    use super::{get_update_status, install_update};
    use crate::services::updater::UpdaterService;

    /// Built from the real tauri.conf.json and capabilities, so the real ACL decides.
    fn app() -> tauri::App<MockRuntime> {
        mock_builder()
            .manage(UpdaterService::<tauri_plugin_updater::Update>::new(|_| {}))
            .invoke_handler(tauri::generate_handler![get_update_status, install_update])
            .build(tauri::generate_context!(test = true))
            .unwrap()
    }

    fn invoke(
        app: &tauri::App<MockRuntime>,
        label: &str,
        command: &str,
    ) -> Result<serde_json::Value, String> {
        let window = app.get_webview_window(label).unwrap_or_else(|| {
            tauri::WebviewWindowBuilder::new(app, label, Default::default())
                .build()
                .unwrap()
        });
        get_ipc_response(
            &window,
            InvokeRequest {
                cmd: command.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: if cfg!(windows) {
                    "http://tauri.localhost"
                } else {
                    "tauri://localhost"
                }
                .parse()
                .unwrap(),
                body: InvokeBody::default(),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|body| body.deserialize::<serde_json::Value>().unwrap())
        .map_err(|error| error.to_string())
    }

    // Keep: over real IPC the capability layer admits both commands for the dashboard only.
    #[test]
    fn update_commands_are_denied_to_the_overlay_over_ipc() {
        let app = app();
        for command in ["get_update_status", "install_update"] {
            assert_eq!(
                invoke(&app, "dashboard", command),
                Ok(serde_json::json!({"sequence": 0, "status": {"state": "none"}})),
                "dashboard {command}"
            );
            let denied = invoke(&app, "overlay", command).unwrap_err();
            assert!(
                denied.contains("not allowed"),
                "overlay {command}: {denied}"
            );
        }
    }

    // Keep: the handlers authorize the native window label before reading any state.
    #[test]
    fn update_handlers_refuse_other_window_labels() {
        let app = mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        for label in ["overlay", "unexpected"] {
            let window = tauri::WebviewWindowBuilder::new(&app, label, Default::default())
                .build()
                .unwrap();
            assert_eq!(
                get_update_status(window.clone()).unwrap_err(),
                "window_not_authorized"
            );
            assert_eq!(install_update(window).unwrap_err(), "window_not_authorized");
        }
    }
}
