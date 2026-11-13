use crate::audio::{AudioCapturePolicy, DesktopAudioCapture, TrustedCaptureOwner};
use crate::commands::stt::segments::CaptureDictations;
use crate::services::background_mode::{self, CloseAction};
use crate::services::login_item;
use crate::services::runtime::EchoRuntime;
use crate::services::transcription_jobs::TranscriptionJobs;
use crate::{commands, diagnostics, platform};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, WindowEvent};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FnHotkeyEvent {
    state: &'static str,
}

pub fn setup_application(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let audio_capture = initialize_runtime(app)?;
    configure_overlay(app, &audio_capture);
    configure_dashboard(app, &audio_capture);
    #[cfg(target_os = "windows")]
    for label in ["dashboard", "overlay"] {
        if let Some(window) = app.get_webview_window(label) {
            use_executable_icons(&window);
        }
    }
    start_modifier_monitor(app);
    if let Err(error) = diagnostics::harden_log_directory_permissions(app.handle()) {
        log::warn!("Could not harden diagnostic log permissions: {error}");
    }
    show_tray_and_dashboard(app);
    Ok(())
}

// tauri.conf.json creates the dashboard hidden, so a login launch never flashes it.
fn show_tray_and_dashboard(app: &tauri::App) {
    let runtime = app.state::<EchoRuntime>();
    background_mode::apply_run_in_background(
        app.handle(),
        runtime.run_in_background().unwrap_or(false),
    );
    let launched_at_login =
        std::env::args().any(|argument| argument == background_mode::LAUNCHED_AT_LOGIN_ARG);
    if background_mode::dashboard_visible_at_start(launched_at_login, &runtime) {
        background_mode::show_dashboard(app.handle());
    }
    login_item::refresh_at_startup(app.handle(), runtime.launch_at_login().unwrap_or(false));
}

pub fn shutdown_application(app: &AppHandle) {
    if let Some(dictations) = app.try_state::<CaptureDictations>() {
        dictations.cancel_all();
    }
    if let Some(monitor) = app.try_state::<platform::NativeModifierMonitor>() {
        monitor.shutdown();
    }
    if let Some(jobs) = app.try_state::<TranscriptionJobs>() {
        for owner in ["overlay", "dashboard"] {
            let _ = jobs.cancel_owner(owner);
        }
    }
    if let Some(capture) = app.try_state::<DesktopAudioCapture>() {
        capture.shutdown();
    }
}

fn initialize_runtime(
    app: &mut tauri::App,
) -> Result<DesktopAudioCapture, Box<dyn std::error::Error>> {
    let directory = commands::storage::setup::directory(app)?;
    let persisted = bootstrap(app, &directory)?;
    app.manage(CaptureDictations::with_partial_publisher(
        commands::stt::events::overlay_partial_publisher(app.handle().clone()),
    ));
    let event_handle = app.handle().clone();
    let runtime = app.state::<EchoRuntime>().inner().clone();
    let audio_capture = DesktopAudioCapture::start(
        AudioCapturePolicy::default(),
        std::sync::Arc::new(move |event| {
            commands::stt::segments::capture_event(&event_handle, &event);
            publish_capture_state(&event_handle, &runtime, &event);
            let _ = event_handle.emit("echo-audio-capture", event);
        }),
    )
    .map_err(|error| std::io::Error::other(error.to_string()))?;
    audio_capture
        .apply_diagnostics_setting(persisted.revision, persisted.settings.debug_logging_enabled)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    app.manage(audio_capture.clone());
    Ok(audio_capture)
}

fn publish_capture_state<R: tauri::Runtime>(
    app: &AppHandle<R>,
    runtime: &EchoRuntime,
    event: &crate::audio::CaptureEvent,
) {
    if event.outcome == "recording_progress" {
        return;
    }
    if let Ok(envelope) = runtime.apply_capture_event(event) {
        let _ = app.emit("echo-desktop-state", envelope);
    }
}

fn bootstrap<R: tauri::Runtime>(
    app: &impl Manager<R>,
    directory: &std::path::Path,
) -> std::io::Result<crate::storage::SettingsEnvelope> {
    // Tauri keeps the first managed value, so an injected test service stays in place.
    app.manage(crate::services::credentials::CredentialService::default());
    let persisted = commands::storage::setup::initialize(app, directory)?;
    app.state::<EchoRuntime>()
        .replace_persisted_settings(persisted.revision, &persisted.settings)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(persisted)
}

#[cfg(test)]
#[path = "app_setup_tests.rs"]
mod tests;

fn configure_overlay(app: &tauri::App, audio_capture: &DesktopAudioCapture) {
    let Some(overlay) = app.get_webview_window("overlay") else {
        return;
    };
    let (width, height) = (80.0, 80.0);
    let _ = overlay.set_size(LogicalSize::new(width, height));
    if let Err(error) =
        commands::overlay_position::place_at_bottom_center(&overlay, width, height, 0.0, Some(80.0))
    {
        log::warn!("Could not place overlay at startup: {error}");
    }
    let app_handle = app.handle().clone();
    let capture = audio_capture.clone();
    let jobs = app.state::<TranscriptionJobs>().inner().clone();
    overlay.on_window_event(move |event| {
        if matches!(event, WindowEvent::CloseRequested { .. }) {
            platform::set_fn_hotkey_capture_enabled(false);
            if let Err(error) = app_handle.global_shortcut().unregister_all() {
                log::warn!("Could not unregister overlay shortcuts: {error}");
            }
            let _ = capture.cleanup_owner(TrustedCaptureOwner::from_window_label("overlay"));
            app_handle.state::<CaptureDictations>().cancel_all();
            let _ = jobs.cancel_owner("overlay");
        }
    });
}

fn configure_dashboard(app: &tauri::App, audio_capture: &DesktopAudioCapture) {
    let Some(dashboard) = app.get_webview_window("dashboard") else {
        return;
    };
    let app_handle = app.handle().clone();
    let capture = audio_capture.clone();
    let jobs = app.state::<TranscriptionJobs>().inner().clone();
    let runtime = app.state::<EchoRuntime>().inner().clone();
    let window = dashboard.clone();
    dashboard.on_window_event(move |event| {
        let WindowEvent::CloseRequested { api, .. } = event else {
            return;
        };
        // Hiding skips owner cleanup: the dashboard holds no capture or job grant, and
        // the overlay keeps dictation running.
        if background_mode::close_action(&runtime) == CloseAction::Hide {
            api.prevent_close();
            if let Err(error) = window.hide() {
                log::warn!("Could not hide the dashboard: {error}");
            }
            return;
        }
        let _ = capture.cleanup_owner(TrustedCaptureOwner::from_window_label("dashboard"));
        let _ = jobs.cancel_owner("dashboard");
        app_handle.exit(0);
    });
    #[cfg(target_os = "windows")]
    let _ = dashboard.set_decorations(false);
}

// Tauri sets only a stretched small icon. The overlay skips the taskbar but still
// appears in Alt-Tab, so both windows use the executable's sized icon frames.
#[cfg(target_os = "windows")]
fn use_executable_icons(window: &tauri::WebviewWindow) {
    let Ok(handle) = window.hwnd() else {
        log::warn!("Window handle unavailable; keeping the default window icons");
        return;
    };
    let mut icons = platform::WindowIcons::new(handle.0);
    icons.apply();
    let icons = std::sync::Mutex::new(icons);
    window.on_window_event(move |event| {
        let is_rescaled = matches!(event, WindowEvent::ScaleFactorChanged { .. });
        if !is_rescaled && !matches!(event, WindowEvent::Destroyed) {
            return;
        }
        let mut icons = icons
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if is_rescaled {
            icons.apply();
        } else {
            icons.release();
        }
    });
}

fn start_modifier_monitor(app: &tauri::App) {
    let handle = app.handle().clone();
    let health_handle = handle.clone();
    let result = platform::start_modifier_monitor(
        move |pressed| {
            let _ = handle.emit(
                "echo-fn-hotkey",
                FnHotkeyEvent {
                    state: if pressed { "pressed" } else { "released" },
                },
            );
        },
        move |available| {
            publish_modifier_availability(&health_handle, available);
            let _ = health_handle.emit(
                "echo-fn-hotkey",
                FnHotkeyEvent {
                    state: if available {
                        "available"
                    } else {
                        "unavailable"
                    },
                },
            );
        },
    );
    match result {
        Ok(modifier_monitor) => {
            app.manage(modifier_monitor);
        }
        Err(error) => {
            log::warn!("Native modifier monitor unavailable: {error}");
            #[cfg(target_os = "windows")]
            publish_modifier_availability(app.handle(), false);
            #[cfg(target_os = "windows")]
            let _ = app.emit(
                "echo-fn-hotkey",
                FnHotkeyEvent {
                    state: "unavailable",
                },
            );
        }
    }
}

fn publish_modifier_availability(app: &AppHandle, available: bool) {
    if let Ok(envelope) = app
        .state::<EchoRuntime>()
        .set_native_modifier_available(available)
    {
        let _ = app.emit("echo-desktop-state", envelope);
    }
}
