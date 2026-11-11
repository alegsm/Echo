use super::*;

impl<Backend: CaptureBackend> AudioActor<Backend> {
    pub(super) fn snapshot(&self) -> CaptureSnapshot {
        let (artifact_count, artifact_bytes) = self.artifacts.usage();
        if let Some(active) = self.active.as_ref() {
            let stats = active.buffer.stats();
            return CaptureSnapshot {
                sequence: self.sequence,
                phase: active.phase,
                purpose: Some(active.reservation.purpose),
                intent_id: Some(active.reservation.intent_id.clone()),
                session_id: Some(active.session_id.clone()),
                has_observed_callback: stats.0 > 0,
                callback_count: stats.0,
                sample_count: stats.1,
                last_error: self.last_error,
                artifact_count,
                artifact_bytes,
            };
        }
        let pending = self.gate.pending();
        CaptureSnapshot {
            sequence: self.sequence,
            phase: pending.as_ref().map_or(CapturePhase::Idle, |value| value.0),
            purpose: pending.as_ref().map(|value| value.1),
            intent_id: pending.as_ref().map(|value| value.2.clone()),
            session_id: pending.and_then(|value| value.3),
            has_observed_callback: false,
            callback_count: 0,
            sample_count: 0,
            last_error: self.last_error,
            artifact_count,
            artifact_bytes,
        }
    }

    pub(super) fn fail_start(
        &mut self,
        reservation: &CaptureReservation,
        diagnostics: Option<crate::microphone_diagnostics::MicrophoneDiagnostics>,
        error: CaptureError,
    ) {
        let mut snapshot = self.snapshot();
        self.gate.release(reservation.generation);
        self.last_error = Some(error.code);
        snapshot.phase = CapturePhase::Idle;
        snapshot.intent_id = Some(reservation.intent_id.clone());
        snapshot.purpose = Some(reservation.purpose);
        snapshot.last_error = self.last_error;
        self.emit_snapshot(
            snapshot,
            EventOccurrence::error("start_failed", error.code),
            0,
        );
        if let Some(observer) = diagnostics {
            observer.emit(
                "native_capture_start_failed",
                log::Level::Error,
                crate::microphone_diagnostics::MicrophoneFields {
                    stage: Some(error.stage),
                    error_name: Some(capture_error_name(error.code)),
                    ..Default::default()
                },
            );
            observer.shutdown();
        }
    }

    pub(super) fn finish_diagnostics(
        &self,
        diagnostics: Option<crate::microphone_diagnostics::MicrophoneDiagnostics>,
        metrics: crate::microphone_diagnostics::FinalCaptureMetrics,
    ) {
        if let Some(observer) = diagnostics {
            observer.finish(metrics);
        }
    }

    pub(super) fn reap_artifacts(&mut self) {
        let before = self.artifacts.usage().0;
        self.artifacts.reap(self.now());
        if self.artifacts.usage().0 < before {
            self.emit_current(EventOccurrence::new("artifact_expired"));
        }
    }

    pub(super) fn emit_active(
        &mut self,
        outcome: &'static str,
        error: Option<CaptureErrorCode>,
        artifact_id: Option<&str>,
    ) {
        if self.active.is_some() {
            self.emit_current(EventOccurrence {
                outcome,
                error,
                artifact_id: artifact_id.map(str::to_owned),
            });
        }
    }

    pub(super) fn emit_reservation(
        &mut self,
        reservation: &CaptureReservation,
        outcome: &'static str,
        artifact_id: Option<&str>,
    ) {
        let _ = reservation;
        self.emit_current(EventOccurrence {
            outcome,
            error: None,
            artifact_id: artifact_id.map(str::to_owned),
        });
    }

    pub(super) fn emit_current(&mut self, occurrence: EventOccurrence) {
        let elapsed = self.active.as_ref().map_or(0, |active| {
            self.now()
                .duration_since(active.capture_started_at)
                .as_millis() as u64
        });
        self.emit_snapshot(self.snapshot(), occurrence, elapsed);
    }

    pub(super) fn emit_terminal(
        &mut self,
        active: &ActiveCapture<Backend::Stream>,
        outcome: &'static str,
        error: Option<CaptureErrorCode>,
        phase: CapturePhase,
    ) {
        let mut snapshot = self.snapshot();
        snapshot.phase = phase;
        snapshot.purpose = Some(active.reservation.purpose);
        snapshot.intent_id = Some(active.reservation.intent_id.clone());
        snapshot.session_id = Some(active.session_id.clone());
        let stats = active.buffer.stats();
        snapshot.has_observed_callback = stats.0 > 0;
        snapshot.callback_count = stats.0;
        snapshot.sample_count = stats.1;
        self.emit_snapshot(
            snapshot,
            EventOccurrence {
                outcome,
                error,
                artifact_id: Some(active.artifact_id.clone()),
            },
            self.now()
                .duration_since(active.capture_started_at)
                .as_millis() as u64,
        );
    }

    fn emit_snapshot(
        &mut self,
        snapshot: CaptureSnapshot,
        occurrence: EventOccurrence,
        elapsed_ms: u64,
    ) {
        self.sequence += 1;
        (self.event_sink)(CaptureEvent {
            elapsed_ms,
            limit_ms: self.policy.maximum_capture_duration.as_millis() as u64,
            sequence: self.sequence,
            phase: snapshot.phase,
            purpose: snapshot.purpose,
            intent_id: snapshot.intent_id,
            session_id: snapshot.session_id,
            artifact_id: occurrence.artifact_id,
            outcome: occurrence.outcome,
            error: occurrence.error,
            has_observed_callback: snapshot.has_observed_callback,
            callback_count: snapshot.callback_count,
            sample_count: snapshot.sample_count,
            last_error: snapshot.last_error,
            artifact_count: snapshot.artifact_count,
            artifact_bytes: snapshot.artifact_bytes,
        });
    }

    pub(super) fn active_generation(&self) -> Option<u64> {
        self.active
            .as_ref()
            .map(|active| active.reservation.generation)
    }
}

pub(super) fn capture_metrics(
    duration_ms: u64,
    sample_count: usize,
    audio_bytes: usize,
    channel_count: u16,
) -> crate::microphone_diagnostics::FinalCaptureMetrics {
    crate::microphone_diagnostics::FinalCaptureMetrics {
        duration_ms,
        sample_count,
        frame_count: sample_count / usize::from(channel_count.max(1)),
        audio_bytes,
    }
}
pub(super) struct EventOccurrence {
    outcome: &'static str,
    error: Option<CaptureErrorCode>,
    artifact_id: Option<String>,
}

impl EventOccurrence {
    pub(super) fn new(outcome: &'static str) -> Self {
        Self {
            outcome,
            error: None,
            artifact_id: None,
        }
    }

    pub(super) fn error(outcome: &'static str, error: CaptureErrorCode) -> Self {
        Self {
            outcome,
            error: Some(error),
            artifact_id: None,
        }
    }
}

pub(super) fn emit_stream_drop(
    diagnostics: Option<&crate::microphone_diagnostics::MicrophoneDiagnostics>,
    reason: &'static str,
) {
    if let Some(observer) = diagnostics {
        observer.emit(
            "native_stream_drop_started",
            log::Level::Info,
            crate::microphone_diagnostics::MicrophoneFields {
                reason: Some(reason),
                ..Default::default()
            },
        );
    }
}

pub(super) fn emit_stream_dropped(
    diagnostics: Option<&crate::microphone_diagnostics::MicrophoneDiagnostics>,
) {
    if let Some(observer) = diagnostics {
        observer.emit(
            "native_stream_drop_completed",
            log::Level::Info,
            crate::microphone_diagnostics::MicrophoneFields {
                outcome: Some("rust_stream_handle_dropped"),
                ..Default::default()
            },
        );
    }
}

pub(super) fn capture_error_name(code: CaptureErrorCode) -> &'static str {
    match code {
        CaptureErrorCode::MicrophoneUnavailable => "microphone_unavailable",
        CaptureErrorCode::DeviceConfigFailed => "default_input_config_failed",
        CaptureErrorCode::StreamBuildFailed => "stream_build_failed",
        CaptureErrorCode::StreamPlayFailed => "stream_play_failed",
        CaptureErrorCode::FirstCallbackTimeout => "first_callback_timeout",
        CaptureErrorCode::PcmMemoryLimit => "pcm_memory_limit",
        CaptureErrorCode::RecordingTooShort => "recording_too_short",
        CaptureErrorCode::LowSignal => "low_signal",
        _ => "capture_failed",
    }
}

pub(super) fn emit_device_snapshot(
    diagnostics: &crate::microphone_diagnostics::MicrophoneDiagnostics,
    stage: &'static str,
) {
    let state = crate::audio::device_observation::default_input_device_snapshot();
    diagnostics.emit(
        "native_default_input_state",
        log::Level::Info,
        state.diagnostic_fields(stage),
    );
}
