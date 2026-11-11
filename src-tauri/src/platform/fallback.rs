//! Fallback behavior for desktop targets without native platform support.

pub struct NativeModifierMonitor;

impl NativeModifierMonitor {
    pub fn shutdown(&self) {}
}

pub fn microphone_permission_status() -> String {
    "unknown".to_string()
}

pub fn request_microphone_permission() -> Result<String, String> {
    Err("Microphone access cannot be confirmed on this platform.".to_string())
}

pub fn paste_clipboard() -> Result<(), String> {
    Err("Automatic paste is not supported on this platform yet.".to_string())
}

pub fn accessibility_permission_status() -> String {
    "not_required".to_string()
}

pub fn request_accessibility_permission() -> String {
    accessibility_permission_status()
}

pub fn input_monitoring_permission_status() -> String {
    "not_required".to_string()
}

pub fn open_privacy_settings(_kind: &str) -> Result<(), String> {
    Err("Privacy settings are unavailable on this platform.".to_string())
}

pub fn set_fn_hotkey_capture_enabled(_enabled: bool) {}

pub fn start_modifier_monitor<F>(_handler: F) -> Result<NativeModifierMonitor, String>
where
    F: Fn(bool) + Send + Sync + 'static,
{
    Ok(NativeModifierMonitor)
}

pub(super) mod clipboard {
    pub fn snapshot() -> Result<String, ()> {
        Err(())
    }
    pub fn publish_protected(_text: &str) -> Result<u64, ()> {
        Err(())
    }
    pub fn restore_if_owned(_previous: &str, _text: &str, _generation: u64) -> Result<(), ()> {
        Err(())
    }
}
