use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

const LOG_FILE_NAME: &str = "Echo.log";
const PROBE_FILE_PREFIX: &str = ".echo-diagnostic-write-probe";

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    #[default]
    Info,
    Warn,
    Error,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticRequest {
    area: String,
    event: String,
    attempt_id: Option<String>,
    #[serde(default)]
    level: DiagnosticLevel,
    #[serde(default)]
    fields: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Renderer requests carry no trusted provenance or preformatted log text.
#[tauri::command]
pub fn record_diagnostic_event(
    window: tauri::WebviewWindow,
    request: DiagnosticRequest,
) -> Result<bool, String> {
    write_diagnostic(
        window.label(),
        &window.state::<crate::storage::EchoStorage>(),
        request,
        |level, record| log::log!(target: "echo::renderer_diagnostics", level, "[echo-diagnostic] {record}"),
    )
}

fn write_diagnostic(
    surface: &str,
    storage: &crate::storage::EchoStorage,
    request: DiagnosticRequest,
    mut sink: impl FnMut(log::Level, &str),
) -> Result<bool, String> {
    if !matches!(surface, "dashboard" | "overlay") {
        return Err("window_not_authorized".into());
    }
    if !storage
        .read_settings()
        .map_err(|_| "diagnostic_settings_unavailable")?
        .settings
        .debug_logging_enabled
    {
        return Ok(false);
    }
    let allowed_event = match request.area.as_str() {
        "diagnostics" => matches!(request.event.as_str(), "enabled" | "disabled"),
        "recording" => matches!(
            request.event.as_str(),
            "clipboard_copy_completed"
                | "clipboard_copy_failed"
                | "paste_completed"
                | "paste_failed"
                | "paste_suppressed"
        ),
        _ => false,
    };
    if !allowed_event || request.fields.len() > 64 {
        return Err("invalid_diagnostic_event".into());
    }
    let fields: serde_json::Map<_, _> = request
        .fields
        .into_iter()
        .filter(|(name, value)| allowed_field(name, value))
        .collect();
    let attempt_id = request.attempt_id.filter(|id| valid_diagnostic_id(id));
    let record = serde_json::json!({
        "timeMs": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "diagnostic_clock_unavailable")?.as_millis(),
        "surface": surface,
        "version": env!("CARGO_PKG_VERSION"),
        "gitCommit": option_env!("ECHO_GIT_COMMIT").unwrap_or("unavailable"),
        "area": request.area,
        "event": request.event,
        "attemptId": attempt_id,
        "fields": fields,
    })
    .to_string();
    if record.len() > 4096 {
        return Err("invalid_diagnostic_event".into());
    }
    let level = match request.level {
        DiagnosticLevel::Info => log::Level::Info,
        DiagnosticLevel::Warn => log::Level::Warn,
        DiagnosticLevel::Error => log::Level::Error,
    };
    sink(level, &record);
    Ok(true)
}

fn valid_diagnostic_id(id: &str) -> bool {
    id.len() == 36 && uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id)
}

// Each string field has a closed value vocabulary. Identifier-shaped private
// text is not safe merely because it lacks punctuation. Nested values are dropped.
fn allowed_field(name: &str, value: &serde_json::Value) -> bool {
    match name {
        "stage" => matches!(value.as_str(), Some("clipboard" | "paste")),
        "outcome" => matches!(value.as_str(), Some("success" | "failed" | "disabled")),
        "code" => matches!(
            value.as_str(),
            Some("clipboard_failed" | "insertion_failed")
        ),
        _ => false,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticContext {
    version: &'static str,
    git_commit: &'static str,
    log_file_path: String,
    log_file_exists: bool,
}

#[tauri::command]
pub fn get_diagnostic_context(app: AppHandle) -> Result<DiagnosticContext, String> {
    let log_file_path = diagnostic_log_path(&app)?;
    Ok(DiagnosticContext {
        version: env!("CARGO_PKG_VERSION"),
        git_commit: option_env!("ECHO_GIT_COMMIT").unwrap_or("unavailable"),
        log_file_exists: log_file_path.is_file(),
        log_file_path: log_file_path.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
pub fn probe_diagnostic_log(app: AppHandle) -> Result<(), String> {
    let log_file_path = diagnostic_log_path(&app)?;
    let log_directory = log_file_path
        .parent()
        .ok_or_else(|| "diagnostic_log_directory_unavailable".to_string())?;
    probe_log_directory(log_directory)
}

fn diagnostic_log_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_log_dir()
        .map(|directory| directory.join(LOG_FILE_NAME))
        .map_err(|error| error.to_string())
}

pub fn harden_log_directory_permissions(app: &AppHandle) -> Result<(), String> {
    let log_file_path = diagnostic_log_path(app)?;
    let directory = log_file_path
        .parent()
        .ok_or_else(|| "diagnostic_log_directory_unavailable".to_string())?;
    fs::create_dir_all(directory).map_err(|_| "diagnostic_log_directory_unwritable".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| "diagnostic_log_permission_update_failed".to_string())?;
        for entry in fs::read_dir(directory)
            .map_err(|_| "diagnostic_log_directory_unreadable".to_string())?
        {
            let path = entry
                .map_err(|_| "diagnostic_log_entry_unreadable".to_string())?
                .path();
            if path.is_file()
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("Echo"))
            {
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                    .map_err(|_| "diagnostic_log_permission_update_failed".to_string())?;
            }
        }
    }
    Ok(())
}

fn probe_log_directory(log_directory: &Path) -> Result<(), String> {
    fs::create_dir_all(log_directory)
        .map_err(|_| "diagnostic_log_directory_unwritable".to_string())?;
    let probe_path = log_directory.join(format!("{PROBE_FILE_PREFIX}-{}", uuid::Uuid::new_v4()));
    write_flush_and_remove_probe(&probe_path)
}

fn write_flush_and_remove_probe(probe_path: &Path) -> Result<(), String> {
    let mut probe = create_private_probe(probe_path)?;
    let write_result = probe
        .write_all(b"echo-diagnostic-probe")
        .map_err(|_| "diagnostic_log_probe_write_failed".to_string())
        .and_then(|()| {
            probe
                .sync_all()
                .map_err(|_| "diagnostic_log_probe_flush_failed".to_string())
        });
    drop(probe);
    let cleanup_result =
        fs::remove_file(probe_path).map_err(|_| "diagnostic_log_probe_cleanup_failed".to_string());
    write_result.and(cleanup_result)
}

fn create_private_probe(probe_path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(probe_path)
        .map_err(|_| "diagnostic_log_probe_create_failed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::{probe_log_directory, PROBE_FILE_PREFIX};

    // Keep: route the native request through the retained bounded file backend.
    #[test]
    fn diagnostics_native_request_reaches_one_real_bounded_log_record() {
        let directory = temporary_test_path("native-file");
        let storage = crate::storage::EchoStorage::open(directory.join("echo.sqlite3")).unwrap();
        storage.initialize().unwrap();
        storage
            .update_settings(
                0,
                crate::storage::SettingsPatch {
                    debug_logging_enabled: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        let (_, logger) = crate::bounded_log::dispatch(directory.join("logs"))
            .unwrap()
            .into_log();
        let request = serde_json::from_value(serde_json::json!({
            "area":"diagnostics", "event":"enabled", "fields":{"code":"privateAlphanumericText"}
        }))
        .unwrap();
        assert!(
            write_diagnostic("dashboard", &storage, request, |level, record| {
                logger.log(
                    &log::Record::builder()
                        .level(level)
                        .target("echo::renderer_diagnostics")
                        .args(format_args!("[echo-diagnostic] {record}"))
                        .build(),
                );
            })
            .unwrap()
        );
        logger.flush();
        let output = fs::read_to_string(directory.join("logs/Echo.log")).unwrap();
        let lines: Vec<_> = output.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(!output.contains("privateAlphanumericText"));
        let record: serde_json::Value =
            serde_json::from_str(lines[0].strip_prefix("[echo-diagnostic] ").unwrap()).unwrap();
        assert_eq!(record["event"], "enabled");
        assert_eq!(record["surface"], "dashboard");
        assert_eq!(record["fields"], serde_json::json!({}));
        drop(logger);
        drop(storage);
        fs::remove_dir_all(directory).unwrap();
    }

    // Keep: exercise the command's native policy with real committed SQLite settings.
    #[test]
    fn diagnostics_enforce_settings_window_identity_and_closed_values() {
        let directory = temporary_test_path("native-policy");
        let storage = crate::storage::EchoStorage::open(directory.join("echo.sqlite3")).unwrap();
        storage.initialize().unwrap();
        let request = || {
            serde_json::from_value::<DiagnosticRequest>(serde_json::json!({
            "area": "recording", "event": "clipboard_copy_failed", "level": "warn",
            "attemptId": "privateAlphanumericText",
            "fields": {
                "code": "clipboard_failed", "stage": "clipboard", "outcome": "failed",
                "rms": 0.004, "framesSeen": 5, "sessionId": "privateAlphanumericText",
                "reason": "privateAlphanumericText", "providerId": "privateAlphanumericText",
                "durationMs": "privateAlphanumericText", "peak": {"secret": "privateAlphanumericText"},
                "transcript": "privateAlphanumericText"
            }
        })).unwrap()
        };
        let mut records = Vec::new();
        assert!(
            !write_diagnostic("dashboard", &storage, request(), |_, r| records
                .push(r.to_owned()))
            .unwrap()
        );
        assert!(records.is_empty());
        storage
            .update_settings(
                0,
                crate::storage::SettingsPatch {
                    debug_logging_enabled: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            write_diagnostic("unknown", &storage, request(), |_, r| records
                .push(r.to_owned())),
            Err("window_not_authorized".into())
        );
        assert!(records.is_empty());
        for surface in ["dashboard", "overlay"] {
            assert!(write_diagnostic(surface, &storage, request(), |level, r| {
                assert_eq!(level, log::Level::Warn);
                records.push(r.to_owned());
            })
            .unwrap());
            let line = records.last().unwrap();
            assert!(!line.contains("privateAlphanumericText"));
            assert!(!line.contains('\n'));
            assert!(line.len() <= 4096);
            let record: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(record["surface"], surface);
            assert_eq!(record["version"], env!("CARGO_PKG_VERSION"));
            assert_eq!(
                record["gitCommit"],
                option_env!("ECHO_GIT_COMMIT").unwrap_or("unavailable")
            );
            assert!(record["timeMs"].as_u64().unwrap() > 0);
            assert_eq!(
                record["fields"],
                serde_json::json!({"code":"clipboard_failed","stage":"clipboard","outcome":"failed"})
            );
        }
        assert_eq!(records.len(), 2);
        for value in [
            serde_json::json!(uuid::Uuid::new_v4().to_string()),
            serde_json::json!("privateAlphanumericText"),
        ] {
            assert!(!allowed_field("sessionId", &value));
        }
        storage
            .update_settings(
                1,
                crate::storage::SettingsPatch {
                    debug_logging_enabled: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            !write_diagnostic("overlay", &storage, request(), |_, _| panic!(
                "disabled write"
            ))
            .unwrap()
        );
        drop(storage);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn diagnostics_reject_forged_metadata_and_bound_requests() {
        for (key, value) in [
            ("surface", serde_json::json!("dashboard")),
            ("version", serde_json::json!("forged")),
            ("gitCommit", serde_json::json!("forged")),
            ("time", serde_json::json!(0)),
            ("message", serde_json::json!("privateText")),
            ("unknown", serde_json::json!({})),
        ] {
            let mut value_request = serde_json::json!({"area":"diagnostics","event":"enabled"});
            value_request[key] = value;
            assert!(serde_json::from_value::<DiagnosticRequest>(value_request).is_err());
        }
        let directory = temporary_test_path("request-bounds");
        let storage = crate::storage::EchoStorage::open(directory.join("echo.sqlite3")).unwrap();
        storage.initialize().unwrap();
        storage
            .update_settings(
                0,
                crate::storage::SettingsPatch {
                    debug_logging_enabled: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        for value in [
            serde_json::json!({"area":"privateAlphanumericText","event":"enabled"}),
            serde_json::json!({"area":"diagnostics","event":"privateAlphanumericText"}),
            serde_json::json!({"area":"diagnostics","event":"probe"}),
            serde_json::json!({"area":"recording","event":"capture.started"}),
            serde_json::json!({"area":"recording","event":"capture.failed"}),
            serde_json::json!({"area":"microphone","event":"audio_window"}),
            serde_json::json!({"area":"diagnostics","event":"enabled", "fields": (0..65).map(|i| (format!("unknown{i}"), true)).collect::<std::collections::BTreeMap<_,_>>()}),
        ] {
            let request = serde_json::from_value(value).unwrap();
            assert_eq!(
                write_diagnostic("dashboard", &storage, request, |_, _| panic!(
                    "invalid write"
                )),
                Err("invalid_diagnostic_event".into())
            );
        }
        drop(storage);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn probe_flushes_and_removes_its_private_file() {
        let directory = temporary_test_path("success");
        probe_log_directory(&directory).expect("probe should succeed");
        assert!(!has_probe_file(&directory));
        std::fs::remove_dir(directory).expect("test directory should be removable");
    }

    #[test]
    fn probe_reports_a_path_that_cannot_be_a_directory() {
        let path = temporary_test_path("failure");
        std::fs::write(&path, b"file").expect("test file should be created");
        assert_eq!(
            probe_log_directory(&path),
            Err("diagnostic_log_directory_unwritable".to_string())
        );
        std::fs::remove_file(path).expect("test file should be removable");
    }

    #[test]
    fn simultaneous_surface_probes_do_not_collide() {
        let directory = temporary_test_path("concurrent");
        let probes: Vec<_> = (0..4)
            .map(|_| {
                let directory = directory.clone();
                std::thread::spawn(move || probe_log_directory(&directory))
            })
            .collect();
        for probe in probes {
            probe
                .join()
                .expect("probe thread should finish")
                .expect("probe should succeed");
        }
        assert!(!has_probe_file(&directory));
        std::fs::remove_dir(directory).expect("test directory should be removable");
    }

    fn temporary_test_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("echo-diagnostic-{label}-{}", uuid::Uuid::new_v4()))
    }

    fn has_probe_file(directory: &PathBuf) -> bool {
        std::fs::read_dir(directory)
            .expect("test directory should be readable")
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(PROBE_FILE_PREFIX)
            })
    }

    use std::path::PathBuf;
}
