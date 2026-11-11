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
mod platform;
mod providers;
mod services;
mod storage;
#[cfg(test)]
mod test_support;

use services::runtime::EchoRuntime;
use services::transcription_jobs::TranscriptionJobs;

macro_rules! generate_echo_handlers {
    ($( $name:ident => $handler:path, )*) => {
        tauri::generate_handler![$($handler),*]
    };
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();
    let log_directory = bounded_log::platform_log_directory(&context.config().identifier)
        .expect("Echo log directory should be available");
    let log_dispatch =
        bounded_log::dispatch(log_directory).expect("Echo log writer should initialize");
    let application = tauri::Builder::default()
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
        .setup(app_setup::setup_application)
        .invoke_handler(for_each_echo_command!(generate_echo_handlers))
        .build(context)
        .expect("error while building Echo");

    application.run(|app, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            app_setup::shutdown_application(app);
        }
    });
}
