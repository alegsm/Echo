//! Linux platform support.
//!
//! X11 sessions get a full native path: XInput2 raw key events for the Control+Super
//! hold-to-talk chord and XTest for the paste keystroke. Wayland does not let ordinary
//! clients observe or synthesize global input, so there Echo reads `/dev/input` when the
//! user may (membership in the `input` group) and pastes through `wtype` or `ydotool`
//! when either is installed. Linux has no per-app microphone, accessibility or input
//! monitoring consent outside sandboxes, so those report `not_required`.

pub(super) mod clipboard;
mod evdev;
mod hotkey;
mod paste;
mod x11;

pub use hotkey::{
    native_modifier_available, set_fn_hotkey_capture_enabled, start_modifier_monitor,
};
pub type NativeModifierMonitor = hotkey::NativeModifierMonitor;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Session {
    X11,
    Wayland,
}

/// The display protocol of the user's session, independent of the GDK backend Echo's own
/// windows use (they may run through XWayland).
pub(super) fn session() -> Session {
    session_from(
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty()),
    )
}

fn session_from(kind: Option<&str>, has_wayland_display: bool) -> Session {
    match kind.map(str::to_ascii_lowercase).as_deref() {
        Some("wayland") => Session::Wayland,
        Some("x11") => Session::X11,
        _ if has_wayland_display => Session::Wayland,
        _ => Session::X11,
    }
}

pub fn microphone_permission_status() -> String {
    "not_required".to_string()
}

pub fn request_microphone_permission() -> Result<String, String> {
    Ok(microphone_permission_status())
}

pub fn paste_clipboard() -> Result<(), String> {
    paste::paste(session())
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

pub fn open_privacy_settings(kind: &str) -> Result<(), String> {
    Err(format!(
        "Linux has no system privacy settings for {kind}. Check your desktop's sound settings or PipeWire/PulseAudio configuration."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_prefers_the_declared_type_then_wayland_display() {
        assert_eq!(session_from(Some("wayland"), false), Session::Wayland);
        assert_eq!(session_from(Some("X11"), true), Session::X11);
        assert_eq!(session_from(Some("tty"), true), Session::Wayland);
        assert_eq!(session_from(None, false), Session::X11);
    }

    /// Run with an X server, xdotool and xclip:
    /// `xvfb-run -a cargo test linux_x11 -- --ignored --test-threads=1`
    #[test]
    #[ignore = "needs an X server, xdotool and xclip"]
    fn linux_x11_chord_clipboard_and_paste_work_end_to_end() {
        use std::process::Command;
        use std::sync::mpsc;
        use std::time::Duration;

        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::set_var("XDG_SESSION_TYPE", "x11");
        x11::ACCEPT_SYNTHETIC.store(true, std::sync::atomic::Ordering::SeqCst);
        let xdotool = |args: &[&str]| {
            assert!(Command::new("xdotool")
                .args(args)
                .status()
                .unwrap()
                .success());
        };
        let xclip = |args: &[&str]| {
            let output = Command::new("xclip").args(args).output().unwrap();
            String::from_utf8(output.stdout).unwrap()
        };

        // Chord: either Control with either Super, observed through XInput2 raw events.
        let (pressed_tx, pressed) = mpsc::channel();
        let (health_tx, health) = mpsc::channel();
        let monitor = start_modifier_monitor(
            move |is_pressed| pressed_tx.send(is_pressed).unwrap(),
            move |available| health_tx.send(available).unwrap(),
        )
        .unwrap();
        assert_eq!(health.recv_timeout(Duration::from_secs(5)), Ok(true));
        assert!(native_modifier_available());
        set_fn_hotkey_capture_enabled(true);
        xdotool(&["keydown", "Control_L", "keydown", "Super_L"]);
        assert_eq!(pressed.recv_timeout(Duration::from_secs(2)), Ok(true));
        xdotool(&["keyup", "Super_L"]);
        assert_eq!(pressed.recv_timeout(Duration::from_secs(2)), Ok(false));
        xdotool(&[
            "keyup",
            "Control_L",
            "keydown",
            "Super_R",
            "keydown",
            "Control_R",
        ]);
        assert_eq!(pressed.recv_timeout(Duration::from_secs(2)), Ok(true));
        xdotool(&["keyup", "Control_R", "keyup", "Super_R"]);
        assert_eq!(pressed.recv_timeout(Duration::from_secs(2)), Ok(false));
        // A lone modifier is not the chord.
        xdotool(&["keydown", "Control_L", "keyup", "Control_L"]);
        assert!(pressed.recv_timeout(Duration::from_millis(300)).is_err());
        // Echo's own XTest paste must never look like the chord, even while capture is on.
        x11::ACCEPT_SYNTHETIC.store(false, std::sync::atomic::Ordering::SeqCst);
        xdotool(&[
            "keydown",
            "Control_L",
            "keydown",
            "Super_L",
            "keyup",
            "Super_L",
            "keyup",
            "Control_L",
        ]);
        assert!(pressed.recv_timeout(Duration::from_millis(300)).is_err());
        monitor.shutdown();
        assert_eq!(health.recv_timeout(Duration::from_secs(2)), Ok(false));
        assert!(!native_modifier_available());

        // Clipboard: protected publication, history hint, and owned restoration.
        let previous = "what the user had copied";
        clipboard::publish_protected(previous).unwrap();
        let generation = clipboard::publish_protected("dictated text").unwrap();
        assert_eq!(xclip(&["-o", "-selection", "clipboard"]), "dictated text");
        assert!(xclip(&["-o", "-selection", "clipboard", "-t", "TARGETS"])
            .contains("x-kde-passwordManagerHint"));
        assert_eq!(clipboard::snapshot(), Ok("dictated text".to_string()));
        clipboard::restore_if_owned(previous, "dictated text", generation).unwrap();
        assert_eq!(xclip(&["-o", "-selection", "clipboard"]), previous);
        // A stale generation leaves a newer clipboard alone.
        let newer = clipboard::publish_protected("newer").unwrap();
        clipboard::restore_if_owned(previous, "newer", newer - 1).unwrap();
        assert_eq!(xclip(&["-o", "-selection", "clipboard"]), "newer");

        // Paste: XTest accepts the full Control+V batch, including held-modifier recovery.
        xdotool(&["keydown", "Shift_L"]);
        paste_clipboard().unwrap();
        xdotool(&["keyup", "Shift_L"]);
    }
}
