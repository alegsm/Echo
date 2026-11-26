#![warn(clippy::undocumented_unsafe_blocks)]

mod app_setup;
mod audio;
mod bounded_log;
#[macro_use]
mod command_registry;
mod commands;
mod diagnostics;
mod microphone_diagnostic_event;
mod microphone_diagnostics;
mod microphone_metrics;
mod panic_log;
mod platform;
mod providers;
mod services;
mod storage;
#[cfg(test)]
mod test_support;

use services::background_mode;
use services::runtime::EchoRuntime;
use services::transcription_jobs::TranscriptionJobs;

macro_rules! generate_echo_handlers {
    ($( $name:ident => $handler:path, )*) => {
        tauri::generate_handler![$($handler),*]
    };
}

/// Wayland lets neither a window place itself nor stay above other apps, which the overlay
/// needs. Under XWayland GTK can do both, so Echo's own windows use X11 there unless the
/// user chose a GDK backend. Input and paste still follow the real session (platform/linux).
#[cfg(target_os = "linux")]
fn prefer_xwayland() {
    let has = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    if has("WAYLAND_DISPLAY") && has("DISPLAY") && !has("GDK_BACKEND") {
        // Called first in run(), before GTK or any other thread starts.
        std::env::set_var("GDK_BACKEND", "x11");
    }
}

/// WebKitGTK's DMA-BUF renderer cannot allocate buffers on the proprietary NVIDIA driver and
/// leaves every window blank.
#[cfg(target_os = "linux")]
fn disable_webkit_dmabuf_renderer() {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    prefer_xwayland();
    #[cfg(target_os = "linux")]
    disable_webkit_dmabuf_renderer();
    panic_log::install();
    let context = tauri::generate_context!();
    let log_directory = bounded_log::platform_log_directory(&context.config().identifier)
        .expect("Echo log directory should be available");
    let log_dispatch =
        bounded_log::dispatch(log_directory).expect("Echo log writer should initialize");
    let builder = tauri::Builder::default()
        // Registered first so a second launch hands off before any other plugin starts.
        // Its arguments and working directory are ignored: it can only show the dashboard.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            background_mode::show_dashboard(app)
        }));
    // Only builds that install their own updates register the updater plugin (the switch is
    // in build.rs). Rust-only: no capability grants an `updater:` permission.
    // services/updater.rs owns every check, and on these builds every download and install.
    #[cfg(echo_update_install)]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    let application = builder
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .clear_targets()
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Dispatch(log_dispatch),
                ))
                .build(),
        )
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(providers::http_client().expect("Provider HTTP client should initialize"))
        .manage(EchoRuntime::default())
        .manage(TranscriptionJobs::default())
        .on_menu_event(background_mode::handle_menu_event)
        .setup(app_setup::setup_application)
        .invoke_handler(for_each_echo_command!(generate_echo_handlers))
        .build(context)
        .expect("error while building Echo");

    application.run(|app, event| match event {
        tauri::RunEvent::Exit => app_setup::shutdown_application(app),
        // The overlay is always visible, so has_visible_windows says nothing about the dashboard.
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => background_mode::show_dashboard(app),
        _ => {}
    });
}
