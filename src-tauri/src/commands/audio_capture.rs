use serde::Deserialize;
use tauri::{State, WebviewWindow};

use super::stt::segments::CaptureDictations;
use crate::audio::{
    AudioArtifactMetadata, CancelCapture, CaptureError, CaptureErrorCode, CaptureIntent,
    CapturePhase, CapturePurpose, CaptureReservation, CaptureSessionMetadata, DesktopAudioCapture,
    StopCapture, TrustedCaptureOwner,
};
use crate::services::transcription_jobs::TranscriptionJobs;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartAudioCaptureRequest {
    purpose: CapturePurpose,
    intent_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopAudioCaptureRequest {
    session_id: String,
    intent_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelAudioCaptureRequest {
    intent_id: String,
    session_id: Option<String>,
    reason: String,
}

struct PendingCapture {
    owner: TrustedCaptureOwner,
    intent_id: String,
    purpose: CapturePurpose,
    reservation: CaptureReservation,
}

#[tauri::command]
pub async fn start_audio_capture(
    request: StartAudioCaptureRequest,
    window: WebviewWindow,
    capture: State<'_, DesktopAudioCapture>,
) -> Result<CaptureSessionMetadata, CaptureError> {
    let pending = prepare_capture(&request, &window, &capture)?;
    authorize_capture(&pending, &capture).await?;
    begin_capture(pending, &capture).await
}

fn prepare_capture(
    request: &StartAudioCaptureRequest,
    window: &WebviewWindow,
    capture: &DesktopAudioCapture,
) -> Result<PendingCapture, CaptureError> {
    validate_identifier(&request.intent_id, "intent_id")?;
    let owner = trusted_owner(window);
    validate_purpose_owner(&owner, request.purpose)?;
    let reservation = capture.reserve(
        owner.clone(),
        CaptureIntent {
            intent_id: request.intent_id.clone(),
            purpose: request.purpose,
        },
    )?;
    Ok(PendingCapture {
        owner,
        intent_id: request.intent_id.clone(),
        purpose: request.purpose,
        reservation,
    })
}

async fn authorize_capture(
    pending: &PendingCapture,
    capture: &DesktopAudioCapture,
) -> Result<(), CaptureError> {
    let (capture_path, purpose) = pending.purpose.diagnostic_labels();
    let permission_diagnostics =
        crate::microphone_diagnostic_event::PermissionDiagnostics::start_with_token(
            None,
            crate::microphone_diagnostic_event::CaptureDiagnosticIdentity::new(
                capture_path,
                purpose,
                &pending.intent_id,
            ),
            pending.reservation.diagnostics_token(),
        );
    let initial_permission = crate::platform::microphone_permission_status();
    permission_diagnostics.checked(&initial_permission);
    let permission_result =
        tauri::async_runtime::spawn_blocking(crate::platform::request_microphone_permission).await;
    let permission = match permission_result {
        Err(_) => {
            cancel_reservation(
                capture,
                &pending.owner,
                &pending.intent_id,
                "permission_task_failed",
            );
            return Err(unavailable("permission_task"));
        }
        Ok(permission) => permission,
    };
    let permission = match permission {
        Ok(permission) => permission,
        Err(_) => {
            permission_diagnostics.request_failed();
            cancel_reservation(
                capture,
                &pending.owner,
                &pending.intent_id,
                "permission_request_failed",
            );
            return Err(permission_error("unknown", cfg!(target_os = "windows")));
        }
    };
    permission_diagnostics.request_succeeded(&initial_permission, &permission);
    if !permission_allows_native_capture(&permission) {
        cancel_reservation(
            capture,
            &pending.owner,
            &pending.intent_id,
            "permission_unavailable",
        );
        return Err(permission_error(&permission, cfg!(target_os = "windows")));
    }
    Ok(())
}

async fn begin_capture(
    pending: PendingCapture,
    capture: &DesktopAudioCapture,
) -> Result<CaptureSessionMetadata, CaptureError> {
    let coordinator = capture.clone();
    let begin_owner = pending.owner;
    let begin_intent = pending.intent_id;
    let reservation = pending.reservation;
    match tauri::async_runtime::spawn_blocking(move || coordinator.begin(reservation)).await {
        Ok(result) => result,
        Err(_) => {
            cancel_reservation(capture, &begin_owner, &begin_intent, "begin_task_failed");
            Err(unavailable("begin_task"))
        }
    }
}

#[tauri::command]
pub async fn stop_audio_capture(
    request: StopAudioCaptureRequest,
    window: WebviewWindow,
    capture: State<'_, DesktopAudioCapture>,
) -> Result<AudioArtifactMetadata, CaptureError> {
    validate_identifier(&request.intent_id, "intent_id")?;
    validate_identifier(&request.session_id, "session_id")?;
    let coordinator = capture.inner().clone();
    let stop = StopCapture {
        owner: trusted_owner(&window),
        intent_id: request.intent_id,
        session_id: request.session_id,
    };
    tauri::async_runtime::spawn_blocking(move || coordinator.stop(stop))
        .await
        .map_err(|_| unavailable("stop_task"))?
}

#[tauri::command]
pub fn cancel_audio_capture(
    request: CancelAudioCaptureRequest,
    window: WebviewWindow,
    capture: State<'_, DesktopAudioCapture>,
    dictations: State<'_, CaptureDictations>,
    jobs: State<'_, TranscriptionJobs>,
) -> Result<(), CaptureError> {
    validate_identifier(&request.intent_id, "intent_id")?;
    if let Some(session_id) = request.session_id.as_deref() {
        validate_identifier(session_id, "session_id")?;
    }
    if window.label() == "overlay" {
        dictations
            .validate_session(&request.intent_id, request.session_id.as_deref())
            .map_err(|_| CaptureError::new(CaptureErrorCode::InvalidReservation, "cancel"))?;
    }
    capture.cancel(CancelCapture {
        owner: trusted_owner(&window),
        intent_id: request.intent_id.clone(),
        session_id: request.session_id.clone(),
        reason: cancel_reason(&request.reason),
    })?;
    if window.label() == "overlay" {
        dictations.cancel(&request.intent_id);
        let _ = jobs.cancel(
            "overlay",
            request.session_id.as_deref().unwrap_or("pending"),
            &request.intent_id,
        );
    }
    Ok(())
}

fn trusted_owner(window: &WebviewWindow) -> TrustedCaptureOwner {
    TrustedCaptureOwner::from_window_label(window.label())
}

fn cancel_reason(reason: &str) -> &'static str {
    match reason {
        "hotkey_released" => "hotkey_released",
        "window_closed" => "window_closed",
        "user_cancelled" => "user_cancelled",
        "superseded" => "superseded",
        _ => "unspecified",
    }
}

fn permission_error(permission: &str, is_windows: bool) -> CaptureError {
    let code = if is_windows && permission == "denied" {
        CaptureErrorCode::MicrophonePermissionDenied
    } else {
        CaptureErrorCode::MicrophoneUnavailable
    };
    let stage = if is_windows && permission != "denied" {
        "permission_unconfirmed"
    } else {
        "permission"
    };
    CaptureError::new(code, stage).at_phase(CapturePhase::PermissionPending)
}

fn permission_allows_native_capture(permission: &str) -> bool {
    permission == "authorized"
}

fn unavailable(stage: &'static str) -> CaptureError {
    CaptureError::new(CaptureErrorCode::CoordinatorUnavailable, stage)
}

fn validate_identifier(value: &str, stage: &'static str) -> Result<(), CaptureError> {
    let is_valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if is_valid {
        Ok(())
    } else {
        Err(CaptureError::new(
            CaptureErrorCode::InvalidReservation,
            stage,
        ))
    }
}

fn validate_purpose_owner(
    owner: &TrustedCaptureOwner,
    purpose: CapturePurpose,
) -> Result<(), CaptureError> {
    let is_allowed = matches!(
        (owner.as_str(), purpose),
        ("overlay", CapturePurpose::PushToTalk)
    );
    if is_allowed {
        Ok(())
    } else {
        Err(CaptureError::new(
            CaptureErrorCode::InvalidOwner,
            "purpose_owner",
        ))
    }
}

fn cancel_reservation(
    capture: &DesktopAudioCapture,
    owner: &TrustedCaptureOwner,
    intent_id: &str,
    reason: &'static str,
) {
    let _ = capture.cancel(CancelCapture {
        owner: owner.clone(),
        intent_id: intent_id.to_owned(),
        session_id: None,
        reason,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_overlay_may_start_dictation_capture() {
        for label in ["overlay", "dashboard", "unknown"] {
            let result = validate_purpose_owner(
                &TrustedCaptureOwner::from_window_label(label),
                CapturePurpose::PushToTalk,
            );
            if label == "overlay" {
                assert!(result.is_ok());
            } else {
                assert_eq!(result.unwrap_err().code, CaptureErrorCode::InvalidOwner);
            }
        }
    }

    #[test]
    fn retired_and_unknown_capture_purposes_are_rejected() {
        let dictation = serde_json::json!({"purpose": "push_to_talk", "intentId": "intent-a"});
        assert!(serde_json::from_value::<StartAudioCaptureRequest>(dictation).is_ok());
        for purpose in ["compare", "unknown"] {
            let request = serde_json::json!({"purpose": purpose, "intentId": "intent-a"});
            assert!(serde_json::from_value::<StartAudioCaptureRequest>(request).is_err());
        }
    }

    // Keep: capture cannot start from unknown consent or the retired unsupported status.
    #[test]
    fn microphone_capture_requires_confirmed_permission() {
        assert!(permission_allows_native_capture("authorized"));
        assert!(!permission_allows_native_capture("unsupported"));
        assert!(!permission_allows_native_capture("unknown"));
        assert!(!permission_allows_native_capture("denied"));
        assert!(!permission_allows_native_capture("restricted"));
        assert!(!permission_allows_native_capture("not_determined"));
    }

    // Keep: unreadable consent is distinct from an explicit Windows denial.
    #[test]
    fn microphone_unconfirmed_access_is_not_reported_as_denied() {
        let error = permission_error("unknown", true);
        assert_eq!(error.code, CaptureErrorCode::MicrophoneUnavailable);
        assert_eq!(error.stage, "permission_unconfirmed");
        assert_eq!(error.phase, CapturePhase::PermissionPending);
        assert_eq!(
            permission_error("denied", true).code,
            CaptureErrorCode::MicrophonePermissionDenied
        );
        assert_eq!(
            permission_error("denied", false).code,
            CaptureErrorCode::MicrophoneUnavailable
        );
    }
}
