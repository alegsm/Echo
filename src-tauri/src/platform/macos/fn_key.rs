use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use crate::platform::{FnKeyAction, FnKeyPreference, FnKeyValue};
use core_foundation::base::{Boolean, CFType, CFTypeRef, TCFType};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFPreferencesAppSynchronize(application_id: CFStringRef) -> Boolean;
    fn CFPreferencesCopyAppValue(key: CFStringRef, application_id: CFStringRef) -> CFTypeRef;
}

const DEFAULTS_PATH: &str = "/usr/bin/defaults";
static PREFERENCE_LOCK: Mutex<()> = Mutex::new(());

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedPreference {
    previous: Option<i32>,
}

trait PreferenceAccess {
    fn read(&mut self) -> Result<Option<FnKeyValue>, String>;
    fn write(&mut self, value: Option<i32>) -> Result<(), String>;
}

struct SystemDefaults;

impl PreferenceAccess for SystemDefaults {
    fn read(&mut self) -> Result<Option<FnKeyValue>, String> {
        read_preference("com.apple.HIToolbox", "AppleFnUsageType")
    }

    fn write(&mut self, value: Option<i32>) -> Result<(), String> {
        let output = Command::new(DEFAULTS_PATH)
            .args(write_arguments(value)?)
            .output()
            .map_err(|_| "Could not change the Fn key setting.".to_string())?;
        // Deleting an already absent key is a successful restoration of system default.
        if output.status.success() || value.is_none() && self.read()?.is_none() {
            Ok(())
        } else {
            Err("Could not change the Fn key setting.".to_string())
        }
    }
}

fn read_preference(domain: &str, key: &str) -> Result<Option<FnKeyValue>, String> {
    let domain = CFString::new(domain);
    let key = CFString::new(key);
    // defaults runs in another process, so refresh this process's cached domain first.
    // SAFETY: domain is a live CFString borrowed only for this synchronous call.
    if unsafe { CFPreferencesAppSynchronize(domain.as_concrete_TypeRef()) } == 0 {
        return Err("Could not refresh the Fn key setting.".to_string());
    }
    // Copy returns an owned value, or NULL for an absent preference, without localized stderr.
    // SAFETY: key and domain are live CFStrings; Copy returns an owned CFType or NULL.
    let value = unsafe {
        CFPreferencesCopyAppValue(key.as_concrete_TypeRef(), domain.as_concrete_TypeRef())
    };
    if value.is_null() {
        return Ok(None);
    }
    // SAFETY: the non-null Copy result carries one owned reference, transferred once here.
    let value = unsafe { CFType::wrap_under_create_rule(value) };
    Ok(Some(preference_value(&value)))
}

fn preference_value(value: &CFType) -> FnKeyValue {
    match value
        .downcast::<CFNumber>()
        .and_then(|number| number.to_f64())
        .filter(|number| number.is_finite())
    {
        Some(number) => FnKeyValue::Number(number),
        None => FnKeyValue::Unrecognized,
    }
}

fn restorable_value(value: Option<FnKeyValue>) -> Result<Option<i32>, String> {
    match value {
        None => Ok(None),
        Some(FnKeyValue::Number(value)) if [0.0, 1.0, 2.0, 3.0].contains(&value) => {
            Ok(Some(value as i32))
        }
        _ => Err("This Fn key value cannot be changed safely by Echo.".to_string()),
    }
}

fn write_arguments(value: Option<i32>) -> Result<Vec<&'static str>, String> {
    let value = match value {
        None => return Ok(vec!["delete", "com.apple.HIToolbox", "AppleFnUsageType"]),
        Some(0) => "0",
        Some(1) => "1",
        Some(2) => "2",
        Some(3) => "3",
        _ => return Err("This Fn key value cannot be restored safely by Echo.".to_string()),
    };
    Ok(vec![
        "write",
        "com.apple.HIToolbox",
        "AppleFnUsageType",
        "-int",
        value,
    ])
}

pub fn status(backup: &Path) -> Result<FnKeyPreference, String> {
    let _guard = PREFERENCE_LOCK
        .lock()
        .map_err(|_| "Fn key setting is busy.".to_string())?;
    Ok(FnKeyPreference {
        supported: true,
        current: SystemDefaults.read()?,
        can_restore: read_backup(backup)?.is_some(),
    })
}

pub fn change(backup: &Path, action: FnKeyAction) -> Result<(), String> {
    let _guard = PREFERENCE_LOCK
        .lock()
        .map_err(|_| "Fn key setting is busy.".to_string())?;
    update_with_access(backup, action, &mut SystemDefaults)
}

fn read_backup(path: &Path) -> Result<Option<SavedPreference>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Could not read the previous Fn key setting.".to_string()),
    };
    let saved: SavedPreference = serde_json::from_slice(&bytes)
        .map_err(|_| "Could not read the previous Fn key setting.".to_string())?;
    write_arguments(saved.previous)?;
    Ok(Some(saved))
}

fn update_with_access(
    backup: &Path,
    action: FnKeyAction,
    access: &mut impl PreferenceAccess,
) -> Result<(), String> {
    let saved = read_backup(backup)?;
    match action {
        FnKeyAction::Restore => {
            let saved = saved
                .ok_or_else(|| "There is no previous Fn key setting to restore.".to_string())?;
            access.write(saved.previous)?;
            return std::fs::remove_file(backup).map_err(|_| {
                "Setting restored, but its restore record could not be cleared.".to_string()
            });
        }
        FnKeyAction::SystemDefault => {
            if saved.is_some() {
                return Err(
                    "Restore the previous Fn key setting before using system default.".to_string(),
                );
            }
            if restorable_value(access.read()?)?.is_none() {
                return Err("The Fn key already uses the system default.".to_string());
            }
            return access.write(None);
        }
        FnKeyAction::UseFn => {}
    }

    // Validate even with a restore record: another app may have changed the preference.
    let previous = restorable_value(access.read()?)?;
    if saved.is_none() {
        let bytes = serde_json::to_vec(&SavedPreference { previous })
            .map_err(|_| "Could not save the previous Fn key setting.".to_string())?;
        let parent = backup
            .parent()
            .ok_or_else(|| "Invalid Fn restore path.".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|_| "Could not save the previous Fn key setting.".to_string())?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(backup)
            .map_err(|_| "Could not save the previous Fn key setting.".to_string())?;
        // A failed or interrupted save must prevent the system preference write.
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| "Could not save the previous Fn key setting.".to_string())?;
    }
    access.write(Some(0))
}

#[cfg(test)]
#[path = "fn_key_tests.rs"]
mod tests;
