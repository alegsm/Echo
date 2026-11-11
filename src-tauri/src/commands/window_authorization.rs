use tauri::WebviewWindow;

pub(super) const WINDOW_NOT_AUTHORIZED: &str = "window_not_authorized";

pub fn require_known_window(window: &WebviewWindow) -> Result<(), String> {
    authorize(window.label(), WindowAccess::Known)
}

pub fn require_overlay(window: &WebviewWindow) -> Result<(), String> {
    authorize(window.label(), WindowAccess::Overlay)
}

pub fn require_dashboard<R: tauri::Runtime>(window: &WebviewWindow<R>) -> Result<(), String> {
    authorize(window.label(), WindowAccess::Dashboard)
}

#[derive(Clone, Copy)]
enum WindowAccess {
    Known,
    Overlay,
    Dashboard,
}

fn authorize(label: &str, access: WindowAccess) -> Result<(), String> {
    let is_authorized = match access {
        WindowAccess::Known => matches!(label, "overlay" | "dashboard"),
        WindowAccess::Overlay => label == "overlay",
        WindowAccess::Dashboard => label == "dashboard",
    };
    if is_authorized {
        Ok(())
    } else {
        Err(WINDOW_NOT_AUTHORIZED.to_string())
    }
}

pub(super) fn can_access_record(label: &str, current_id: Option<&str>, requested_id: &str) -> bool {
    authorize(label, WindowAccess::Dashboard).is_ok()
        || authorize(label, WindowAccess::Overlay).is_ok() && current_id == Some(requested_id)
}

#[cfg(test)]
mod tests {
    use super::{authorize, can_access_record, WindowAccess, WINDOW_NOT_AUTHORIZED};

    #[test]
    fn denials_use_renderer_matchable_code() {
        assert_eq!(
            authorize("unexpected", WindowAccess::Known),
            Err("window_not_authorized".to_string())
        );
    }

    #[test]
    fn known_window_access_rejects_untrusted_labels() {
        assert!(authorize("overlay", WindowAccess::Known).is_ok());
        assert!(authorize("dashboard", WindowAccess::Known).is_ok());
        assert_eq!(
            authorize("unexpected", WindowAccess::Known),
            Err(WINDOW_NOT_AUTHORIZED.to_string())
        );
    }

    #[test]
    fn overlay_access_rejects_dashboard() {
        assert!(authorize("overlay", WindowAccess::Overlay).is_ok());
        assert!(authorize("dashboard", WindowAccess::Overlay).is_err());
    }

    #[test]
    fn dashboard_access_rejects_overlay() {
        assert!(authorize("dashboard", WindowAccess::Dashboard).is_ok());
        assert!(authorize("overlay", WindowAccess::Dashboard).is_err());
        assert!(authorize("unexpected", WindowAccess::Dashboard).is_err());
    }

    #[test]
    fn record_access_is_dashboard_wide_and_overlay_current_only() {
        assert!(can_access_record("dashboard", None, "any"));
        assert!(can_access_record("overlay", Some("current"), "current"));
        assert!(!can_access_record("overlay", Some("current"), "other"));
        assert!(!can_access_record("other", Some("current"), "current"));
    }
}
