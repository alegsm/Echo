//! Start Echo at login: the per-user OS record that launches Echo with
//! `--launched-at-login`. On macOS it is `~/Library/LaunchAgents/Echo.plist`; on
//! Windows it is the `Echo` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//! The record is named after `productName`, which is also the NSIS uninstaller's
//! `${PRODUCTNAME}`, so the uninstaller removes the Windows value. Each call builds the
//! launcher from the running executable, so a moved Echo registers its new location.

use std::path::{Path, PathBuf};

use auto_launch::{AutoLaunchBuilder, MacOSLaunchMode, WindowsEnableMode};
use tauri::{AppHandle, Runtime};

use crate::services::background_mode::LAUNCHED_AT_LOGIN_ARG;

/// Adds the record for the running executable when `on`, removes it otherwise.
pub fn set_registered<R: Runtime>(app: &AppHandle<R>, on: bool) -> Result<(), String> {
    #[cfg(windows)]
    if !on {
        return user_run_entry::remove(&user_run_entry::CurrentUser, &app.package_info().name);
    }
    let launcher = launcher(app, on)?
        .build()
        .map_err(|error| error.to_string())?;
    let result = if on {
        launcher.enable()
    } else {
        launcher.disable()
    };
    result.map_err(|error| error.to_string())
}

/// Whether the OS will start Echo at login. On macOS this only checks that the plist
/// exists. On Windows it reads Echo's own HKCU entry, and a Task Manager disable reads
/// as not registered.
pub fn is_registered<R: Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    #[cfg(windows)]
    {
        user_run_entry::enabled(&user_run_entry::CurrentUser, &app.package_info().name)
    }
    #[cfg(not(windows))]
    {
        launcher(app, false)?
            .build()
            .and_then(|launcher| launcher.is_enabled())
            .map_err(|error| error.to_string())
    }
}

/// Rewrites the record at startup so a moved Echo keeps launching. It runs only while
/// the setting is on and the OS still lists Echo, so a removal in System Settings or a
/// Task Manager disable is never undone. Failures are logged and never block startup.
pub fn refresh_at_startup<R: Runtime>(app: &AppHandle<R>, saved_on: bool) {
    let refreshed = refresh(
        saved_on,
        || is_registered(app),
        || set_registered(app, true),
    );
    if let Err(error) = refreshed {
        log::warn!("Start Echo at login could not be refreshed: {error}");
    }
}

fn refresh(
    saved_on: bool,
    registered: impl FnOnce() -> Result<bool, String>,
    register: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if saved_on && registered()? {
        register()?;
    }
    Ok(())
}

fn launcher<R: Runtime>(app: &AppHandle<R>, for_enable: bool) -> Result<AutoLaunchBuilder, String> {
    launcher_for(
        &app.package_info().name,
        &app.config().identifier,
        &current_exe()?,
        for_enable,
    )
}

/// The launcher configuration for one executable path. Only enabling writes the path,
/// so only enabling refuses a path the macOS plist cannot hold; removal and status
/// work from any location.
fn launcher_for(
    name: &str,
    identifier: &str,
    exe: &Path,
    for_enable: bool,
) -> Result<AutoLaunchBuilder, String> {
    let exe = exe
        .to_str()
        .ok_or("Echo's location is not valid Unicode. Move Echo to another folder.")?;
    if for_enable && cfg!(target_os = "macos") {
        refuse_unescaped_plist_path(exe)?;
    }
    let path = if cfg!(windows) {
        run_value_path(exe)
    } else {
        exe.to_owned()
    };
    let mut builder = AutoLaunchBuilder::new();
    builder
        .set_app_name(name)
        .set_app_path(&path)
        .set_args(&[LAUNCHED_AT_LOGIN_ARG])
        .set_macos_launch_mode(MacOSLaunchMode::LaunchAgent)
        .set_bundle_identifiers(&[identifier])
        .set_windows_enable_mode(WindowsEnableMode::CurrentUser);
    Ok(builder)
}

/// auto-launch writes the plist's ProgramArguments without XML escaping, so these
/// characters would corrupt the file or inject keys.
fn refuse_unescaped_plist_path(exe: &str) -> Result<(), String> {
    if exe.contains(['&', '<', '>']) {
        return Err(
            "Echo's location contains &, < or >. Move Echo to a folder without these characters."
                .into(),
        );
    }
    Ok(())
}

/// auto-launch joins the Run value as `<path> <args>`; an unquoted path with spaces
/// lets Windows run an earlier partial path such as `C:\Program.exe`.
fn run_value_path(exe: &str) -> String {
    format!("\"{exe}\"")
}

/// macOS registers the resolved path so the plist names the real bundle executable.
/// Windows keeps the path as launched: canonicalizing adds a `\\?\` prefix.
fn current_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe();
    #[cfg(target_os = "macos")]
    let exe = exe.and_then(std::fs::canonicalize);
    exe.map_err(|error| format!("Echo's location could not be read: {error}"))
}

/// Echo's Windows entry is per user: auto-launch writes it to HKCU when enabling, but
/// its `disable` also deletes a same-named HKLM value and its `is_enabled` also reads
/// HKLM. Echo removes and reads only HKCU, so a machine-wide entry an administrator
/// created stays in place and never shows as a mismatch the switch cannot clear.
#[cfg(any(windows, test))]
mod user_run_entry {
    pub(super) const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    /// Task Manager's per-entry startup switch. auto-launch writes an enabled value here
    /// when the key exists.
    pub(super) const APPROVED_KEY: &str =
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

    /// Registry values addressed by full key path, hive first. A missing key or value
    /// reads as `None` and removes as a no-op.
    pub(super) trait Registry {
        fn value(&self, key: &str, name: &str) -> Result<Option<Vec<u8>>, String>;
        fn remove_value(&self, key: &str, name: &str) -> Result<(), String>;
    }

    /// Removes the Run value before Task Manager's switch, so a partial failure never
    /// leaves a Task Manager-disabled entry enabled again.
    pub(super) fn remove(registry: &impl Registry, name: &str) -> Result<(), String> {
        registry.remove_value(RUN_KEY, name)?;
        registry.remove_value(APPROVED_KEY, name)
    }

    pub(super) fn enabled(registry: &impl Registry, name: &str) -> Result<bool, String> {
        if registry.value(RUN_KEY, name)?.is_none() {
            return Ok(false);
        }
        Ok(registry
            .value(APPROVED_KEY, name)?
            .is_none_or(|value| task_manager_allows(&value)))
    }

    /// Task Manager stores a disable as a nonzero timestamp in the last eight bytes; this
    /// is the rule auto-launch applies.
    fn task_manager_allows(value: &[u8]) -> bool {
        value.len() < 8 || value.iter().rev().take(8).all(|byte| *byte == 0)
    }

    /// The HKCU subkey for a key path, refusing every other hive.
    pub(super) fn user_subkey(key: &str) -> Result<&str, String> {
        key.strip_prefix(r"HKCU\")
            .ok_or_else(|| format!("Echo only changes HKCU, not {key}."))
    }

    #[cfg(windows)]
    pub(super) struct CurrentUser;

    #[cfg(windows)]
    impl Registry for CurrentUser {
        fn value(&self, key: &str, name: &str) -> Result<Option<Vec<u8>>, String> {
            let read = windows_registry::CURRENT_USER
                .open(user_subkey(key)?)
                .and_then(|key| key.get_value(name));
            found(read.map(|value| value.to_vec()))
        }

        fn remove_value(&self, key: &str, name: &str) -> Result<(), String> {
            let removed = windows_registry::CURRENT_USER
                .options()
                .write()
                .open(user_subkey(key)?)
                .and_then(|key| key.remove_value(name));
            found(removed).map(drop)
        }
    }

    /// Maps a missing key or value (`ERROR_FILE_NOT_FOUND` as an HRESULT) to `None`.
    #[cfg(windows)]
    fn found<T>(result: windows_registry::Result<T>) -> Result<Option<T>, String> {
        const NOT_FOUND: i32 = 0x8007_0002_u32 as i32;
        match result {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.code().0 == NOT_FOUND => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
}

#[cfg(test)]
#[path = "login_item_tests.rs"]
mod tests;
