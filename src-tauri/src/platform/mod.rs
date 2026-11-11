//! Platform-owned permissions, paste insertion, and native modifier monitoring.

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod fallback;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(any(target_os = "windows", test))]
mod windows_hook_health;
#[cfg(any(target_os = "windows", test))]
mod windows_hotkey_reducer;
#[cfg(any(target_os = "windows", test))]
mod windows_microphone;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use fallback as selected_platform;
#[cfg(target_os = "macos")]
use macos as selected_platform;
#[cfg(target_os = "windows")]
use windows as selected_platform;

#[cfg(any(target_os = "windows", test))]
mod windows_input;

pub type NativeModifierMonitor = selected_platform::NativeModifierMonitor;
#[cfg(target_os = "windows")]
pub use windows::window_icon::WindowIcons;

pub fn microphone_permission_status() -> String {
    selected_platform::microphone_permission_status()
}

pub fn request_microphone_permission() -> Result<String, String> {
    selected_platform::request_microphone_permission()
}

pub fn paste_clipboard() -> Result<(), String> {
    selected_platform::paste_clipboard()
}

pub fn accessibility_permission_status() -> String {
    selected_platform::accessibility_permission_status()
}

pub fn request_accessibility_permission() -> String {
    selected_platform::request_accessibility_permission()
}

pub fn input_monitoring_permission_status() -> String {
    selected_platform::input_monitoring_permission_status()
}

#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FnKeyValue {
    #[cfg(target_os = "macos")]
    Unrecognized,
    #[cfg(target_os = "macos")]
    #[serde(untagged)]
    Number(f64),
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FnKeyAction {
    UseFn,
    Restore,
    SystemDefault,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FnKeyPreference {
    pub supported: bool,
    pub current: Option<FnKeyValue>,
    pub can_restore: bool,
}

pub fn get_fn_key_preference(backup: &std::path::Path) -> Result<FnKeyPreference, String> {
    #[cfg(target_os = "macos")]
    return macos::fn_key::status(backup);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = backup;
        Ok(FnKeyPreference {
            supported: false,
            current: None,
            can_restore: false,
        })
    }
}

pub fn change_fn_key_preference(
    backup: &std::path::Path,
    action: FnKeyAction,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return macos::fn_key::change(backup, action);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (backup, action);
        Err("Fn key preferences are available only on macOS.".to_string())
    }
}

pub fn open_privacy_settings(kind: &str) -> Result<(), String> {
    selected_platform::open_privacy_settings(kind)
}

pub fn set_fn_hotkey_capture_enabled(enabled: bool) {
    selected_platform::set_fn_hotkey_capture_enabled(enabled);
}

pub fn native_modifier_available() -> bool {
    #[cfg(target_os = "windows")]
    return windows::native_modifier_available();
    #[cfg(not(target_os = "windows"))]
    true
}

pub fn start_modifier_monitor<F, H>(handler: F, health: H) -> Result<NativeModifierMonitor, String>
where
    F: Fn(bool) + Send + Sync + 'static,
    H: Fn(bool) + Send + Sync + 'static,
{
    #[cfg(target_os = "windows")]
    return windows::start_modifier_monitor(handler, health);
    #[cfg(not(target_os = "windows"))]
    {
        let _ = health;
        selected_platform::start_modifier_monitor(handler)
    }
}

#[derive(Default)]
struct ModifierState {
    is_capture_enabled: bool,
    is_pressed: bool,
}

#[derive(Debug, PartialEq)]
struct ModifierInputResult {
    should_consume: bool,
    transition: Option<bool>,
}

impl ModifierState {
    fn set_capture_enabled(&mut self, enabled: bool) -> Option<bool> {
        self.is_capture_enabled = enabled;
        if enabled || !self.is_pressed {
            return None;
        }

        self.is_pressed = false;
        Some(false)
    }

    fn update_pressed(&mut self, is_physically_pressed: bool) -> ModifierInputResult {
        let is_pressed = self.is_capture_enabled && is_physically_pressed;
        if is_pressed == self.is_pressed {
            return ModifierInputResult {
                should_consume: self.is_capture_enabled,
                transition: None,
            };
        }

        self.is_pressed = is_pressed;
        ModifierInputResult {
            should_consume: self.is_capture_enabled,
            transition: Some(is_pressed),
        }
    }

    fn release_pressed(&mut self) -> Option<bool> {
        if !self.is_pressed {
            return None;
        }

        self.is_pressed = false;
        Some(false)
    }
}

/// Ownership includes the OS generation so another application's identical copy also wins.
pub(crate) fn clipboard_is_owned(
    expected: u64,
    current: u64,
    text: &str,
    current_text: &str,
) -> bool {
    expected == current && text == current_text
}

// Both adapters prepare their representations before entering this publication boundary.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(super) fn publish_clipboard<C>(
    clipboard: C,
    previous: Option<&str>,
    publish: impl FnOnce(&C) -> Result<(), ()>,
    restore: impl FnOnce(&C, &str) -> Result<(), ()>,
    generation: impl FnOnce() -> u64,
) -> Result<u64, ()> {
    if let Err(error) = publish(&clipboard) {
        if let Some(previous) = previous {
            // A failed rollback cannot replace the original publication failure.
            let _ = restore(&clipboard, previous);
        }
        return Err(error);
    }
    // Windows synthesizes additional text formats at CloseClipboard, advancing its sequence.
    drop(clipboard);
    let generation = generation();
    if generation == 0 {
        return Err(());
    }
    Ok(generation)
}

pub struct TranscriptDeliveryPlatform;

impl crate::services::transcript_delivery::DeliveryPlatform for TranscriptDeliveryPlatform {
    fn snapshot(&self) -> Result<String, ()> {
        selected_platform::clipboard::snapshot()
    }
    fn publish_protected(&self, text: &str) -> Result<u64, ()> {
        selected_platform::clipboard::publish_protected(text)
    }
    fn paste(&self) -> Result<(), ()> {
        paste_clipboard().map_err(|_| ())
    }
    fn restore_if_owned(&self, previous: &str, text: &str, generation: u64) -> Result<(), ()> {
        selected_platform::clipboard::restore_if_owned(previous, text, generation)
    }
}

#[cfg(test)]
mod tests {
    use super::ModifierState;

    #[test]
    fn modifier_transitions_only_while_capture_is_enabled() {
        let mut state = ModifierState::default();

        assert_eq!(
            state.update_pressed(true),
            super::ModifierInputResult {
                should_consume: false,
                transition: None
            }
        );
        assert_eq!(state.set_capture_enabled(true), None);
        assert_eq!(state.update_pressed(true).transition, Some(true));
        let repeated_press = state.update_pressed(true);
        assert!(repeated_press.should_consume);
        assert_eq!(repeated_press.transition, None);
        assert_eq!(state.update_pressed(false).transition, Some(false));
    }

    #[test]
    fn disabling_capture_releases_an_active_modifier() {
        let mut state = ModifierState::default();
        state.set_capture_enabled(true);
        state.update_pressed(true);

        assert_eq!(state.set_capture_enabled(false), Some(false));
        assert_eq!(state.update_pressed(false).transition, None);
    }

    #[test]
    fn forced_release_preserves_capture_and_only_emits_once() {
        let mut state = ModifierState::default();
        state.set_capture_enabled(true);
        state.update_pressed(true);

        assert_eq!(state.release_pressed(), Some(false));
        assert_eq!(state.release_pressed(), None);
        assert!(state.update_pressed(false).should_consume);
    }
}
