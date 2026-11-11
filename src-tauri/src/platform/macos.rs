use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use objc2::runtime::Bool;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaType, AVMediaTypeAudio};

pub(super) mod clipboard;
pub mod fn_key;
mod modifier_monitor;

pub use modifier_monitor::{
    input_monitoring_permission_status, set_fn_hotkey_capture_enabled, start_modifier_monitor,
};
pub type NativeModifierMonitor = modifier_monitor::NativeModifierMonitor;

const V_KEY_CODE: CGKeyCode = 0x09;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
}

pub fn microphone_permission_status() -> String {
    status_label(current_status()).to_string()
}

pub fn request_microphone_permission() -> Result<String, String> {
    match current_status() {
        AVAuthorizationStatus::Authorized => Ok("authorized".to_string()),
        AVAuthorizationStatus::Denied => Ok("denied".to_string()),
        AVAuthorizationStatus::Restricted => Ok("restricted".to_string()),
        AVAuthorizationStatus::NotDetermined => request_access(),
        _ => Ok("unknown".to_string()),
    }
}

fn current_status() -> AVAuthorizationStatus {
    let media_type = audio_media_type();
    // SAFETY: media_type is the immutable AVFoundation constant and this status query has no thread affinity.
    unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) }
}

fn request_access() -> Result<String, String> {
    let media_type = audio_media_type();
    let (sender, receiver) = mpsc::channel();
    let callback = RcBlock::new(move |granted: Bool| {
        let _ = sender.send(granted.as_bool());
    });

    // SAFETY: media_type is a process-lifetime constant. AVFoundation copies the
    // completion block, whose owned sender stays valid even after a timeout.
    unsafe {
        AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &callback);
    }

    let granted = receiver
        .recv_timeout(Duration::from_secs(120))
        .map_err(|_| "Timed out waiting for the macOS microphone permission prompt.".to_string())?;

    Ok(if granted { "authorized" } else { "denied" }.to_string())
}

fn audio_media_type() -> &'static AVMediaType {
    // SAFETY: AVFoundation exports an immutable NSString with process lifetime.
    unsafe { AVMediaTypeAudio.expect("AVMediaTypeAudio is available on macOS") }
}

fn status_label(status: AVAuthorizationStatus) -> &'static str {
    match status {
        AVAuthorizationStatus::Authorized => "authorized",
        AVAuthorizationStatus::Denied => "denied",
        AVAuthorizationStatus::Restricted => "restricted",
        AVAuthorizationStatus::NotDetermined => "not_determined",
        _ => "unknown",
    }
}

pub fn paste_clipboard() -> Result<(), String> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| accessibility_error())?;
    let key_down = CGEvent::new_keyboard_event(source.clone(), V_KEY_CODE, true)
        .map_err(|_| accessibility_error())?;
    let key_up = CGEvent::new_keyboard_event(source, V_KEY_CODE, false)
        .map_err(|_| accessibility_error())?;

    key_down.set_flags(CGEventFlags::CGEventFlagCommand);
    key_down.set_type(CGEventType::KeyDown);
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);
    key_up.set_type(CGEventType::KeyUp);

    key_down.post(CGEventTapLocation::HID);
    key_up.post(CGEventTapLocation::HID);

    Ok(())
}

pub fn accessibility_permission_status() -> String {
    if is_accessibility_trusted() {
        "authorized".to_string()
    } else {
        "denied".to_string()
    }
}

pub fn request_accessibility_permission() -> String {
    let prompt_key = CFString::new("AXTrustedCheckOptionPrompt");
    let prompt_value = CFBoolean::true_value();
    let options = CFDictionary::from_CFType_pairs(&[(prompt_key, prompt_value)]);

    // SAFETY: options is a live CFDictionary of a CFString key and CFBoolean
    // value, as required by Accessibility; the synchronous call borrows it.
    unsafe {
        AXIsProcessTrustedWithOptions(options.as_CFTypeRef());
    }

    accessibility_permission_status()
}

const SETTINGS_OPEN_PATH: &str = "/usr/bin/open";

fn settings_url(kind: &str) -> String {
    if matches!(kind, "keyboard" | "globe") {
        return "x-apple.systempreferences:com.apple.Keyboard-Settings.extension".to_string();
    }
    let pane = match kind {
        "accessibility" | "paste" => "Privacy_Accessibility",
        "input_monitoring" | "shortcuts" => "Privacy_ListenEvent",
        "microphone" => "Privacy_Microphone",
        _ => "Privacy",
    };
    format!("x-apple.systempreferences:com.apple.preference.security?{pane}")
}

pub fn open_privacy_settings(kind: &str) -> Result<(), String> {
    std::process::Command::new(SETTINGS_OPEN_PATH)
        .arg(settings_url(kind))
        .spawn()
        .map_err(|error| format!("Could not open System Settings: {error}"))?;
    Ok(())
}

fn is_accessibility_trusted() -> bool {
    // SAFETY: the linked Accessibility query takes no pointers and is thread-safe.
    unsafe { AXIsProcessTrusted() }
}

fn accessibility_error() -> String {
    "Echo could not synthesize paste. Enable Accessibility permission for Echo in System Settings."
        .to_string()
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    // Keep: the Fn Settings action targets Keyboard, while permission links retain Privacy.
    #[test]
    fn fn_key_settings_uses_keyboard_pane_and_absolute_open() {
        assert_eq!(SETTINGS_OPEN_PATH, "/usr/bin/open");
        for kind in ["globe", "keyboard"] {
            assert_eq!(
                settings_url(kind),
                "x-apple.systempreferences:com.apple.Keyboard-Settings.extension"
            );
        }
        assert_eq!(
            settings_url("microphone"),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        );
    }
}
