#[tauri::command]
pub fn set_fn_hotkey_capture_enabled(
    window: tauri::WebviewWindow,
    enabled: bool,
) -> Result<bool, String> {
    super::window_authorization::require_overlay(&window)?;
    let available = crate::platform::native_modifier_available();
    crate::platform::set_fn_hotkey_capture_enabled(enabled);
    Ok(available)
}
