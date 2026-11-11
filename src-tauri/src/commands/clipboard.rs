use crate::services::transcript_delivery::{self, DeliveryError, DeliveryOutcome};
use tauri_plugin_clipboard_manager::ClipboardExt;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliverTranscriptRequest {
    text: String,
    paste_after_transcription: bool,
    restore_clipboard: bool,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyTranscriptRequest {
    text: String,
}

// This bounds clipboard work after IPC deserialization, not memory used to receive the request.
fn validate_text(text: &str) -> Result<(), DeliveryError> {
    if text.len() > 2 * 1024 * 1024 || text.contains('\0') {
        return Err(DeliveryError::ClipboardFailed);
    }
    Ok(())
}

#[tauri::command]
pub async fn deliver_transcript(
    window: tauri::WebviewWindow,
    request: DeliverTranscriptRequest,
) -> Result<DeliveryOutcome, serde_json::Value> {
    super::window_authorization::require_overlay(&window)?;
    validate_text(&request.text).map_err(|error| serde_json::json!(error))?;
    let permit = transcript_delivery::admit().map_err(|error| serde_json::json!(error))?;
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        transcript_delivery::serialized(|| {
            transcript_delivery::deliver(
                &crate::platform::TranscriptDeliveryPlatform,
                std::thread::sleep,
                &request.text,
                request.paste_after_transcription,
                request.restore_clipboard,
            )
        })?
    })
    .await
    .map_err(|_| serde_json::json!(DeliveryError::ClipboardFailed))?
    .map_err(|error| serde_json::json!(error))
}

#[tauri::command]
pub async fn copy_transcript(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    request: CopyTranscriptRequest,
) -> Result<(), serde_json::Value> {
    super::window_authorization::require_known_window(&window)?;
    validate_text(&request.text).map_err(|error| serde_json::json!(error))?;
    let permit = transcript_delivery::admit().map_err(|error| serde_json::json!(error))?;
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        transcript_delivery::serialized(|| {
            app.clipboard()
                .write_text(request.text)
                .map_err(|_| DeliveryError::ClipboardFailed)
        })?
    })
    .await
    .map_err(|_| serde_json::json!(DeliveryError::ClipboardFailed))?
    .map_err(|error| serde_json::json!(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Keep: malformed requests cannot smuggle caller identity or unbounded clipboard contents.
    #[test]
    fn clipboard_requests_are_closed_and_bounded() {
        let valid = serde_json::json!({"text": "hello", "pasteAfterTranscription": true, "restoreClipboard": false});
        assert!(serde_json::from_value::<DeliverTranscriptRequest>(valid.clone()).is_ok());
        for field in ["window", "snapshot", "label"] {
            let mut invalid = valid.clone();
            invalid[field] = "forged".into();
            assert!(serde_json::from_value::<DeliverTranscriptRequest>(invalid).is_err());
            assert!(serde_json::from_value::<CopyTranscriptRequest>(
                serde_json::json!({"text": "hello", field: "forged"})
            )
            .is_err());
        }
        assert!(serde_json::from_value::<DeliverTranscriptRequest>(
            serde_json::json!({"text": "hello"})
        )
        .is_err());
        assert!(validate_text("hello").is_ok());
        assert!(validate_text("embedded\0nul").is_err());
        assert!(validate_text(&"x".repeat(2 * 1024 * 1024 + 1)).is_err());
    }
}
