//! Desktop microphone consent and the Windows privacy settings entry point.

fn consent_status(
    microphone: Option<&str>,
    desktop: Option<&str>,
    device: Option<&str>,
) -> &'static str {
    match (microphone, desktop, device) {
        (Some("Deny"), _, _) | (_, Some("Deny"), _) | (_, _, Some("Deny")) => "denied",
        (Some("Allow"), Some("Allow"), Some("Allow")) => "authorized",
        _ => "unknown",
    }
}

fn settings_open_result(result: isize) -> Result<(), String> {
    if result > 32 {
        Ok(())
    } else {
        Err(format!(
            "Could not open Windows microphone privacy settings (ShellExecuteW: {result})."
        ))
    }
}

#[cfg(target_os = "windows")]
pub(super) use native::{open_settings, permission_status};

#[cfg(target_os = "windows")]
mod native {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    // These two APIs use narrow declarations to avoid adding Windows features to the manifest.
    #[link(name = "advapi32")]
    extern "system" {
        fn RegGetValueW(
            key: *mut c_void,
            subkey: *const u16,
            value: *const u16,
            flags: u32,
            value_type: *mut u32,
            data: *mut c_void,
            size: *mut u32,
        ) -> i32;
    }

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            window: *mut c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> *mut c_void;
    }

    const MICROPHONE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";
    const HKEY_CURRENT_USER: *mut c_void = -2_147_483_647_isize as *mut c_void;
    const HKEY_LOCAL_MACHINE: *mut c_void = -2_147_483_646_isize as *mut c_void;
    const RRF_RT_REG_SZ: u32 = 0x0000_0002;
    const SW_SHOWNORMAL: i32 = 1;

    pub(crate) fn permission_status() -> String {
        let device = read_consent(HKEY_LOCAL_MACHINE, MICROPHONE_KEY);
        let microphone = read_consent(HKEY_CURRENT_USER, MICROPHONE_KEY);
        let desktop = read_consent(HKEY_CURRENT_USER, &format!(r"{MICROPHONE_KEY}\NonPackaged"));
        super::consent_status(microphone.as_deref(), desktop.as_deref(), device.as_deref())
            .to_string()
    }

    fn read_consent(key: *mut c_void, subkey: &str) -> Option<String> {
        let subkey = wide(subkey);
        let value = wide("Value");
        // Allow/Deny fit in this bounded buffer; larger or non-string values remain unknown.
        let mut data = [0_u16; 16];
        let mut size = std::mem::size_of_val(&data) as u32;
        // SAFETY: key is a predefined HKCU or HKLM handle; both names are NUL-terminated.
        // The writable buffer and byte count remain valid for the synchronous query.
        let status = unsafe {
            RegGetValueW(
                key,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                data.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != 0 {
            return None;
        }
        let end = data.iter().position(|unit| *unit == 0)?;
        String::from_utf16(&data[..end]).ok()
    }

    pub(crate) fn open_settings() -> Result<(), String> {
        let operation = wide("open");
        let uri = wide("ms-settings:privacy-microphone");
        // SAFETY: both strings are NUL-terminated and live until ShellExecuteW returns;
        // optional window, parameters and directory are null. The result is a status, not an owned handle.
        let result = unsafe {
            ShellExecuteW(
                null_mut(),
                operation.as_ptr(),
                uri.as_ptr(),
                null(),
                null(),
                SW_SHOWNORMAL,
            )
        };
        super::settings_open_result(result as isize)
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Keep: desktop access requires both consent switches; missing data is not consent.
    #[test]
    fn microphone_consent_requires_explicit_allow_for_desktop_apps() {
        for (microphone, desktop, expected) in [
            (Some("Allow"), Some("Allow"), "authorized"),
            (Some("Deny"), Some("Allow"), "denied"),
            (Some("Allow"), Some("Deny"), "denied"),
            (None, Some("Deny"), "denied"),
            (Some("Deny"), None, "denied"),
            (None, Some("Allow"), "unknown"),
            (Some("Allow"), None, "unknown"),
            (None, None, "unknown"),
            (Some("Unexpected"), Some("Allow"), "unknown"),
            (Some("Allow"), Some(""), "unknown"),
        ] {
            assert_eq!(
                consent_status(microphone, desktop, Some("Allow")),
                expected,
                "{microphone:?}, {desktop:?}"
            );
        }
    }

    // Keep: device-level Deny overrides user consent; unreadable device consent is not a grant.
    #[test]
    fn microphone_device_consent_overrides_user_allow() {
        for (device, expected) in [
            (Some("Deny"), "denied"),
            (None, "unknown"),
            (Some("Unexpected"), "unknown"),
            (Some("Allow"), "authorized"),
        ] {
            assert_eq!(
                consent_status(Some("Allow"), Some("Allow"), device),
                expected
            );
        }
        assert_eq!(consent_status(None, None, Some("Deny")), "denied");
        assert_eq!(consent_status(Some("Deny"), None, None), "denied");
    }

    // Keep: ShellExecute's values through 32 are failures, including a null result.
    #[test]
    fn microphone_settings_propagates_shell_execute_failure() {
        for result in [0, 2, 5, 31, 32] {
            assert!(settings_open_result(result).is_err(), "{result}");
        }
        for result in [33, 42, isize::MAX] {
            assert!(settings_open_result(result).is_ok(), "{result}");
        }
    }
}
