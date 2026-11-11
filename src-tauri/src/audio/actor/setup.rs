use super::telemetry::{emit_device_snapshot, emit_stream_drop, emit_stream_dropped};
use super::*;

pub(super) fn finish_stream<Stream>(active: &mut ActiveCapture<Stream>) {
    if let Some(observer) = active.diagnostics.as_ref() {
        emit_device_snapshot(observer, "before_stop");
    }
    emit_stream_drop(active.diagnostics.as_ref(), "stop");
    drop(active.stream.take());
    emit_stream_dropped(active.diagnostics.as_ref());
}

pub(super) fn start_diagnostics(
    reservation: &CaptureReservation,
    session_id: &str,
) -> Option<crate::microphone_diagnostics::MicrophoneDiagnostics> {
    reservation
        .diagnostics_token
        .clone()
        .filter(crate::microphone_diagnostic_event::DiagnosticToken::is_active)
        .map(|token| {
            let (capture_path, purpose) = reservation.purpose.diagnostic_labels();
            let identity = crate::microphone_diagnostics::CaptureDiagnosticIdentity::new(
                capture_path,
                purpose,
                &reservation.intent_id,
            );
            let observer = crate::microphone_diagnostics::MicrophoneDiagnostics::start_with_token(
                session_id,
                identity,
                Some(token),
            );
            observer.emit(
                "native_capture_start_requested",
                log::Level::Info,
                Default::default(),
            );
            observer.emit(
                "native_stream_build_started",
                log::Level::Info,
                Default::default(),
            );
            emit_device_snapshot(&observer, "before_stream_build");
            observer
        })
}

pub(super) fn start_device_observation(
    reservation: &CaptureReservation,
    diagnostics: Option<&crate::microphone_diagnostics::MicrophoneDiagnostics>,
    events: std::sync::mpsc::SyncSender<BackendEvent>,
) -> Option<crate::audio::device_observation::DefaultInputDeviceObserver> {
    diagnostics
        .filter(|observer| observer.is_active())
        .and_then(|observer| {
            let registration = crate::audio::device_observation::DefaultInputDeviceObserver::start(
                events,
                reservation.generation,
            );
            if registration.is_some() {
                observer.emit(
                    "native_device_observer_registered",
                    log::Level::Info,
                    Default::default(),
                );
            } else {
                observer.emit(
                    "native_device_observation_unavailable",
                    log::Level::Info,
                    crate::microphone_diagnostics::MicrophoneFields {
                        reason: Some("device_notification_registration_failed"),
                        ..Default::default()
                    },
                );
            }
            registration
        })
}

pub(super) fn emit_opened<Stream>(
    diagnostics: Option<&crate::microphone_diagnostics::MicrophoneDiagnostics>,
    opened: &super::super::backend::OpenedCapture<Stream>,
) {
    if let Some(observer) = diagnostics {
        observer.emit(
            "native_input_config_result",
            log::Level::Info,
            crate::microphone_diagnostics::MicrophoneFields {
                outcome: Some("succeeded"),
                sample_format: Some(opened.sample_format),
                sample_rate: Some(opened.sample_rate),
                channel_count: Some(opened.channel_count),
                ..Default::default()
            },
        );
        observer.emit(
            "native_stream_build_succeeded",
            log::Level::Info,
            Default::default(),
        );
        observer.emit(
            "native_stream_play_started",
            log::Level::Info,
            Default::default(),
        );
    }
}

pub(super) fn emit_played(
    diagnostics: Option<&crate::microphone_diagnostics::MicrophoneDiagnostics>,
) {
    if let Some(observer) = diagnostics {
        observer.emit(
            "native_stream_play_succeeded",
            log::Level::Info,
            Default::default(),
        );
    }
}
