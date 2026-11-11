use crate::audio::{CaptureErrorCode, CaptureEvent, CapturePhase, CapturePurpose};
use crate::storage::{TranscriptAudioMetadata, TranscriptRecord, TranscriptTiming};

use super::{EchoErrorCode, EchoRuntime, EchoState};

// Keep: asynchronous capture failures preserve the permission-specific renderer code.
#[test]
fn microphone_permission_denial_reaches_the_desktop_snapshot() {
    let runtime = EchoRuntime::default();
    let envelope = runtime
        .apply_capture_event(&capture_event(
            CapturePhase::Idle,
            Some(CaptureErrorCode::MicrophonePermissionDenied),
        ))
        .unwrap();
    assert_eq!(
        envelope.snapshot.last_error,
        Some(EchoErrorCode::MicrophonePermissionDenied)
    );
}

#[test]
fn current_capture_has_display_precedence_over_its_job_failure() {
    let runtime = EchoRuntime::default();
    runtime
        .apply_capture_event(&capture_event(CapturePhase::Recording, None))
        .expect("capture state should apply");
    runtime
        .job_started_for_capture(1, "intent-1")
        .expect("job should start");
    let envelope = runtime
        .job_failed(1, EchoErrorCode::ProviderFailed)
        .expect("job should fail");

    assert_eq!(envelope.snapshot.state, EchoState::Recording);
    assert_eq!(
        envelope.snapshot.last_error,
        Some(EchoErrorCode::ProviderFailed)
    );
}

#[test]
fn late_older_completion_does_not_regress_last_transcript() {
    let runtime = EchoRuntime::default();
    runtime.job_started_for_capture(1, "intent-1").unwrap();
    runtime.job_started_for_capture(2, "intent-1").unwrap();
    runtime
        .job_completed(2, transcript("newer"), Some(1))
        .unwrap();
    let envelope = runtime
        .job_completed(1, transcript("older"), Some(1))
        .unwrap();

    assert_eq!(envelope.snapshot.last_transcript.unwrap().id, "newer");
}

#[test]
fn older_failure_does_not_replace_newer_success() {
    let runtime = EchoRuntime::default();
    runtime.job_started_for_capture(1, "intent-1").unwrap();
    runtime.job_started_for_capture(2, "intent-1").unwrap();
    runtime
        .job_completed(2, transcript("newer"), Some(1))
        .unwrap();

    let envelope = runtime
        .job_failed(1, EchoErrorCode::ProviderFailed)
        .unwrap();

    assert_eq!(envelope.snapshot.state, EchoState::Done);
    assert_eq!(envelope.snapshot.last_error, None);
}

#[test]
fn older_cancel_does_not_clear_newer_warning() {
    let runtime = EchoRuntime::default();
    runtime.job_started_for_capture(1, "intent-1").unwrap();
    runtime.job_started_for_capture(2, "intent-1").unwrap();
    runtime
        .job_completed_with_warning(
            2,
            transcript("newer"),
            Some(1),
            EchoErrorCode::CaptureFailed,
        )
        .unwrap();

    let envelope = runtime.job_cancelled(1).unwrap();

    assert_eq!(envelope.snapshot.state, EchoState::Error);
    assert_eq!(
        envelope.snapshot.last_error,
        Some(EchoErrorCode::CaptureFailed)
    );
}

#[test]
fn older_success_does_not_clear_newer_warning() {
    let runtime = EchoRuntime::default();
    runtime.job_started_for_capture(1, "intent-1").unwrap();
    runtime.job_started_for_capture(2, "intent-1").unwrap();
    runtime
        .job_completed_with_warning(
            2,
            transcript("newer"),
            Some(1),
            EchoErrorCode::CaptureFailed,
        )
        .unwrap();

    let envelope = runtime
        .job_completed(1, transcript("older"), Some(1))
        .unwrap();

    assert_eq!(envelope.snapshot.state, EchoState::Error);
    assert_eq!(
        envelope.snapshot.last_error,
        Some(EchoErrorCode::CaptureFailed)
    );
    assert_eq!(envelope.snapshot.last_transcript.unwrap().id, "newer");
}

#[test]
fn opening_capture_is_not_reported_as_recording() {
    let runtime = EchoRuntime::default();
    let envelope = runtime
        .apply_capture_event(&capture_event(CapturePhase::Opening, None))
        .unwrap();

    assert_eq!(envelope.snapshot.state, EchoState::RequestingMicrophone);
}

#[test]
fn older_job_start_cannot_regress_the_active_provider_setting() {
    let runtime = EchoRuntime::default();
    let settings = crate::storage::PersistedSettings {
        provider_id: "xai-stt".to_string(),
        ..Default::default()
    };
    runtime.replace_persisted_settings(1, &settings).unwrap();

    let envelope = runtime.job_started_for_capture(1, "intent-1").unwrap();

    assert_eq!(envelope.snapshot.active_provider_id, "xai-stt");
}

#[test]
fn dictation_capture_advances_the_shared_desktop_state() {
    let runtime = EchoRuntime::default();
    let before = runtime.desktop_snapshot().unwrap();
    let event = capture_event(CapturePhase::Recording, None);
    let after = runtime.apply_capture_event(&event).unwrap();

    assert_eq!(after.sequence, before.sequence + 1);
    assert_eq!(after.snapshot.state, EchoState::Recording);
}

#[test]
fn terminal_event_without_identity_clears_recording_state() {
    let runtime = EchoRuntime::default();
    runtime
        .apply_capture_event(&capture_event(CapturePhase::Recording, None))
        .unwrap();
    let mut terminal = capture_event(CapturePhase::Idle, None);
    terminal.purpose = None;
    terminal.intent_id = None;
    terminal.session_id = None;

    let envelope = runtime.apply_capture_event(&terminal).unwrap();

    assert_eq!(envelope.snapshot.state, EchoState::Idle);
}

#[test]
fn capture_error_is_sanitized_to_public_error_code() {
    let runtime = EchoRuntime::default();
    let envelope = runtime
        .apply_capture_event(&capture_event(
            CapturePhase::Idle,
            Some(CaptureErrorCode::StreamBackendError),
        ))
        .unwrap();

    assert_eq!(envelope.snapshot.state, EchoState::Error);
    assert_eq!(
        envelope.snapshot.last_error,
        Some(EchoErrorCode::CaptureFailed)
    );
}

#[test]
fn stale_settings_revision_cannot_overwrite_runtime_state() {
    let runtime = EchoRuntime::default();
    let defaults = crate::storage::PersistedSettings::default();
    let initial = runtime.desktop_snapshot().unwrap();
    assert_eq!(initial.snapshot.active_provider_id, defaults.provider_id);
    assert_eq!(
        initial.snapshot.active_shortcut,
        defaults.push_to_talk_shortcut
    );
    let first = crate::storage::PersistedSettings {
        provider_id: "openai-gpt-4o-mini-transcribe".to_string(),
        ..Default::default()
    };
    runtime.replace_persisted_settings(0, &first).unwrap();

    let mut latest = first.clone();
    latest.provider_id = "xai-stt".to_string();
    latest.push_to_talk_shortcut = "Alt+Space".to_string();
    let committed = runtime.replace_persisted_settings(2, &latest).unwrap();
    assert_eq!(committed.snapshot.active_provider_id, latest.provider_id);
    assert_eq!(
        committed.snapshot.active_shortcut,
        latest.push_to_talk_shortcut
    );
    let stale = runtime.replace_persisted_settings(1, &first).unwrap();

    assert_eq!(stale.snapshot.active_provider_id, "xai-stt");
    assert_eq!(stale.snapshot.active_shortcut, "Alt+Space");
    let duplicate = runtime.replace_persisted_settings(2, &first).unwrap();
    assert_eq!(duplicate.sequence, stale.sequence);
    assert_eq!(duplicate.snapshot.active_shortcut, "Alt+Space");
    assert_eq!(duplicate.snapshot.active_provider_id, "xai-stt");
}

#[test]
fn delivery_revision_updates_only_the_current_transcript() {
    let runtime = EchoRuntime::default();
    runtime
        .job_completed(1, transcript("current"), Some(1))
        .unwrap();
    let mut delivered = transcript("current");
    delivered.text = "delivered".to_string();

    let envelope = runtime
        .replace_last_transcript_if_current(2, delivered)
        .unwrap()
        .expect("current transcript should update");
    assert_eq!(envelope.snapshot.last_transcript.unwrap().text, "delivered");
    assert!(runtime
        .replace_last_transcript_if_current(1, transcript("other"))
        .unwrap()
        .is_none());
}

#[test]
fn delayed_older_delivery_revision_cannot_regress_current_transcript() {
    let runtime = EchoRuntime::default();
    runtime
        .job_completed(1, transcript("current"), Some(1))
        .unwrap();
    let mut revision_three = transcript("current");
    revision_three.text = "latest".to_string();
    runtime
        .replace_last_transcript_if_current(3, revision_three)
        .unwrap();
    let mut revision_two = transcript("current");
    revision_two.text = "stale".to_string();

    assert!(runtime
        .replace_last_transcript_if_current(2, revision_two)
        .unwrap()
        .is_none());
    assert_eq!(
        runtime
            .desktop_snapshot()
            .unwrap()
            .snapshot
            .last_transcript
            .unwrap()
            .text,
        "latest"
    );
}

#[test]
fn transcript_clear_requires_the_current_id() {
    let runtime = EchoRuntime::default();
    runtime
        .job_completed(1, transcript("current"), Some(1))
        .unwrap();

    assert!(runtime
        .clear_last_transcript_if_current("other")
        .unwrap()
        .is_none());
    let cleared = runtime
        .clear_last_transcript_if_current("current")
        .unwrap()
        .expect("current transcript should clear");
    assert!(cleared.snapshot.last_transcript.is_none());
}

fn capture_event(phase: CapturePhase, error: Option<CaptureErrorCode>) -> CaptureEvent {
    CaptureEvent {
        elapsed_ms: 0,
        limit_ms: 600_000,
        sequence: 1,
        phase,
        purpose: Some(CapturePurpose::PushToTalk),
        intent_id: Some("intent-1".to_string()),
        session_id: Some("session-1".to_string()),
        artifact_id: None,
        outcome: "test",
        error,
        has_observed_callback: false,
        callback_count: 0,
        sample_count: 0,
        last_error: error,
        artifact_count: 0,
        artifact_bytes: 0,
    }
}

fn transcript(id: &str) -> TranscriptRecord {
    TranscriptRecord {
        id: id.to_string(),
        text: id.to_string(),
        created_at: "2026-09-09T12:00:00Z".to_string(),
        duration_ms: 800,
        provider_id: "groq-whisper-turbo".to_string(),
        provider_model: Some("whisper-large-v3-turbo".to_string()),
        language: Some("en".to_string()),
        prompt: None,
        timings: TranscriptTiming {
            recording_ms: 800.0,
            blob_ready_ms: 0.0,
            upload_ms: 0.0,
            stop_to_transcript_ms: 100.0,
            history_write_ms: Some(1.0),
            provider_ms: Some(95.0),
            backend_ms: Some(100.0),
        },
        audio: TranscriptAudioMetadata {
            mime_type: "audio/wav".to_string(),
            byte_size: 1_024,
        },
        insertion_mode: "manual".to_string(),
        source: "desktop".to_string(),
        error_code: None,
    }
}

#[test]
fn older_completion_during_newer_capture_cannot_replace_current_transcript() {
    let runtime = EchoRuntime::default();
    runtime.job_started_for_capture(1, "intent-1").unwrap();
    let mut event = capture_event(CapturePhase::Recording, None);
    event.outcome = "segment_closed";
    runtime.apply_capture_event(&event).unwrap();
    let after = runtime
        .job_completed(1, transcript("older"), Some(1))
        .unwrap();
    assert_eq!(after.snapshot.state, EchoState::Recording);
    assert!(after.snapshot.last_transcript.is_none());
}

#[test]
fn older_capture_admitted_after_newer_capture_cannot_own_current_state() {
    let runtime = EchoRuntime::default();
    let mut newer = capture_event(CapturePhase::Recording, None);
    newer.intent_id = Some("newer-capture".into());
    newer.outcome = "segment_closed";
    runtime.apply_capture_event(&newer).unwrap();
    runtime.job_started_for_capture(1, "older-capture").unwrap();
    let result = runtime
        .job_completed(1, transcript("older"), Some(1))
        .unwrap();
    assert!(result.snapshot.last_transcript.is_none());
}

#[test]
fn push_to_talk_capture_has_display_precedence_over_job_failure() {
    let runtime = EchoRuntime::default();
    runtime.job_started_for_capture(1, "older").unwrap();
    runtime
        .apply_capture_event(&capture_event(CapturePhase::Recording, None))
        .unwrap();
    let envelope = runtime
        .job_failed(1, EchoErrorCode::ProviderFailed)
        .unwrap();
    assert_eq!(envelope.snapshot.state, EchoState::Recording);
    assert_eq!(
        envelope.snapshot.last_error,
        Some(EchoErrorCode::ProviderFailed)
    );
}

#[test]
fn accidental_tap_preserves_the_older_jobs_outcome() {
    for error in [
        CaptureErrorCode::CaptureCancelled,
        CaptureErrorCode::RecordingTooShort,
    ] {
        let runtime = EchoRuntime::default();
        runtime.job_started_for_capture(1, "older").unwrap();
        runtime
            .apply_capture_event(&capture_event(CapturePhase::Recording, None))
            .unwrap();
        runtime
            .apply_capture_event(&capture_event(CapturePhase::Idle, Some(error)))
            .unwrap();
        let after = runtime
            .job_completed(1, transcript("older"), Some(1))
            .unwrap();
        assert_eq!(after.snapshot.state, EchoState::Done);
        assert_eq!(after.snapshot.last_error, None);
        assert_eq!(after.snapshot.last_transcript.unwrap().id, "older");
    }
}

#[test]
fn accidental_tap_keeps_a_job_that_completed_before_the_tap_ended() {
    for error in [
        CaptureErrorCode::CaptureCancelled,
        CaptureErrorCode::RecordingTooShort,
    ] {
        let runtime = EchoRuntime::default();
        runtime.job_started_for_capture(1, "older").unwrap();
        runtime
            .apply_capture_event(&capture_event(CapturePhase::Recording, None))
            .unwrap();
        runtime
            .job_completed(1, transcript("older"), Some(1))
            .unwrap();
        let after = runtime
            .apply_capture_event(&capture_event(CapturePhase::Idle, Some(error)))
            .unwrap();
        assert_eq!(after.snapshot.state, EchoState::Done);
        assert_eq!(after.snapshot.last_error, None);
        assert_eq!(after.snapshot.last_transcript.unwrap().id, "older");
    }
}

#[test]
fn cancelled_or_short_capture_after_a_finished_job_reports_capture_failed() {
    for error in [
        CaptureErrorCode::CaptureCancelled,
        CaptureErrorCode::RecordingTooShort,
    ] {
        let runtime = EchoRuntime::default();
        runtime.job_started_for_capture(1, "older").unwrap();
        runtime
            .job_completed(1, transcript("older"), Some(1))
            .unwrap();
        runtime
            .apply_capture_event(&capture_event(CapturePhase::Recording, None))
            .unwrap();
        let after = runtime
            .apply_capture_event(&capture_event(CapturePhase::Idle, Some(error)))
            .unwrap();
        assert_eq!(after.snapshot.state, EchoState::Error);
        assert_eq!(
            after.snapshot.last_error,
            Some(EchoErrorCode::CaptureFailed)
        );
    }
}

// Keep: a dashboard opened after failure or recovery reads the latest sequenced health.
#[test]
fn hook_health_snapshot_retains_lifecycle_changes_for_late_readers() {
    let runtime = EchoRuntime::default();
    let initial = runtime.desktop_snapshot().unwrap();
    let value = serde_json::to_value(&initial).unwrap();
    assert!(value["snapshot"]
        .as_object()
        .unwrap()
        .contains_key("nativeModifierAvailable"));
    assert!(value["snapshot"]["nativeModifierAvailable"].is_null());
    let mut sequence = initial.sequence;
    for available in [false, true, false] {
        let event = runtime.set_native_modifier_available(available).unwrap();
        assert!(event.sequence > sequence);
        sequence = event.sequence;
        let read = serde_json::to_value(runtime.desktop_snapshot().unwrap()).unwrap();
        assert_eq!(read["snapshot"]["nativeModifierAvailable"], available);
        assert_eq!(event.snapshot.native_modifier_available, Some(available));
    }
}
