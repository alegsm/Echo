//! Background mode: whether a dashboard close hides or quits, whether the dashboard
//! shows at launch, and the tray icon that brings it back while it is hidden.

use tauri::image::Image;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::services::runtime::EchoRuntime;

/// Marks a launch started by the OS at login. Login registration must pass this exact
/// argument; it can only keep the dashboard hidden.
pub const LAUNCHED_AT_LOGIN_ARG: &str = "--launched-at-login";
const TRAY_ID: &str = "echo-background";
const OPEN_MENU_ID: &str = "tray-open";
const QUIT_MENU_ID: &str = "tray-quit";
const DASHBOARD_LABEL: &str = "dashboard";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseAction {
    Hide,
    Exit,
}

/// Reads the committed setting at close time. Echo quits unless the setting is known
/// to be on, so an unavailable runtime never leaves an invisible process behind.
pub fn close_action(runtime: &EchoRuntime) -> CloseAction {
    match runtime.run_in_background() {
        Ok(true) => CloseAction::Hide,
        Ok(false) | Err(_) => CloseAction::Exit,
    }
}

pub fn dashboard_visible_at_start(launched_at_login: bool, runtime: &EchoRuntime) -> bool {
    !(launched_at_login && runtime.run_in_background().unwrap_or(false))
}

/// The one way Echo brings the dashboard back: tray Open Echo, a Windows left-click,
/// a macOS Dock click, a second launch and a normal start all use it.
pub fn show_dashboard(app: &AppHandle) {
    let Some(dashboard) = app.get_webview_window(DASHBOARD_LABEL) else {
        return;
    };
    let shown = dashboard
        .show()
        .and_then(|()| dashboard.unminimize())
        .and_then(|()| dashboard.set_focus());
    if let Err(error) = shown {
        log::warn!("Could not show the dashboard: {error}");
    }
}

/// Adds the tray icon when the setting is on and removes it when off. Safe to call
/// with an unchanged value. A tray that cannot be built leaves the setting on, so a
/// close still hides; the Dock, a relaunch or a second launch brings the dashboard back.
pub fn apply_run_in_background(app: &AppHandle, on: bool) {
    if !on {
        // Tauri drops the icon with the last handle; none is kept here.
        app.remove_tray_by_id(TRAY_ID);
        return;
    }
    if app.tray_by_id(TRAY_ID).is_some() {
        return;
    }
    if let Err(error) = build_tray(app) {
        log::warn!("Could not create the background tray icon: {error}");
    }
}

/// Registered once on the app builder: Tauri adds a tray builder's menu handler to the
/// app-wide list on every build, so a per-tray handler would repeat after each toggle.
/// The handler also receives app-menu events, which it ignores.
pub fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        OPEN_MENU_ID => show_dashboard(app),
        QUIT_MENU_ID => app.exit(0),
        _ => {}
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, OPEN_MENU_ID, "Open Echo", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, QUIT_MENU_ID, "Quit Echo", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &separator, &quit])?;
    let tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_image(app))
        .tooltip("Echo")
        .menu(&menu);
    #[cfg(target_os = "macos")]
    let tray = tray.icon_as_template(true);
    #[cfg(target_os = "windows")]
    let tray = tray
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_dashboard(tray.app_handle());
            }
        });
    tray.build(app)?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn tray_image(_app: &AppHandle) -> Image<'static> {
    tauri::include_image!("icons/tray/macos-template.png")
}

/// Picks the frame once, from the dashboard's scale when the icon is built; a later
/// DPI change does not re-cut it.
#[cfg(target_os = "windows")]
fn tray_image(app: &AppHandle) -> Image<'static> {
    let scale = app
        .get_webview_window(DASHBOARD_LABEL)
        .and_then(|dashboard| dashboard.scale_factor().ok())
        .unwrap_or(1.0);
    if scale >= 1.5 {
        tauri::include_image!("icons/tray/windows-32.png")
    } else {
        tauri::include_image!("icons/tray/windows-16.png")
    }
}

#[cfg(test)]
#[path = "background_mode_tests.rs"]
mod tests;
