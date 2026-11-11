//! Opens the private app database and registers its initialized facade with Tauri.

use tauri::Manager;

use crate::storage::{EchoStorage, SettingsEnvelope};

pub fn directory<R: tauri::Runtime>(app: &impl Manager<R>) -> std::io::Result<std::path::PathBuf> {
    #[cfg(not(windows))]
    let directory = app.path().app_data_dir().map_err(setup_error)?;
    #[cfg(windows)]
    let directory =
        windows_local_directory(std::env::var_os("LOCALAPPDATA"), &app.config().identifier)?;
    Ok(directory)
}

pub fn initialize<R: tauri::Runtime>(
    app: &impl Manager<R>,
    directory: &std::path::Path,
) -> std::io::Result<SettingsEnvelope> {
    let storage = EchoStorage::open(directory.join("echo.sqlite3")).map_err(setup_error)?;
    let cleanup = storage.initialize().map_err(setup_error)?;
    if cleanup == crate::storage::CleanupStatus::Pending {
        log::warn!("history_cleanup_pending");
    }
    let settings = storage.read_settings().map_err(setup_error)?;
    if !app.manage(storage) {
        return Err(std::io::Error::other("storage_already_initialized"));
    }
    Ok(settings)
}

#[cfg(any(windows, test))]
fn windows_local_directory(
    value: Option<std::ffi::OsString>,
    identifier: &str,
) -> std::io::Result<std::path::PathBuf> {
    let base = value
        .map(std::path::PathBuf::from)
        .filter(|path| {
            path.is_absolute()
                && !path
                    .components()
                    .any(|part| part == std::path::Component::ParentDir)
        })
        .ok_or_else(|| std::io::Error::other("local_app_data_unavailable"))?;
    Ok(base.join(identifier))
}

fn setup_error(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_local_data_requires_an_absolute_localappdata_without_fallback() {
        for invalid in [
            None,
            Some("".into()),
            Some("relative".into()),
            Some(std::env::temp_dir().join("..")),
        ] {
            assert!(windows_local_directory(
                invalid.map(|path: std::path::PathBuf| path.into_os_string()),
                "custom.echo"
            )
            .is_err());
        }
        let local = std::env::temp_dir();
        assert_eq!(
            windows_local_directory(Some(local.clone().into_os_string()), "custom.echo").unwrap(),
            local.join("custom.echo")
        );
    }
}
