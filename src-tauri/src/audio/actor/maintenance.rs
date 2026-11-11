use super::telemetry::*;
use super::*;

/// Events handled per drain; matches the backend event queue's capacity (`mpsc::sync_channel(32)`
/// in `DesktopAudioCapture::start_with_backend`), so one drain empties a queue nobody refills.
const BACKEND_EVENT_BATCH: usize = 32;

impl<Backend: CaptureBackend> AudioActor<Backend> {
    pub(super) fn drain_capture(&mut self) {
        let Some(mut active) = self.active.take() else {
            return;
        };
        let before = active.stored_segment_count;
        let result = self.drain_active(&mut active);
        let artifact_id = active.artifact_id.clone();
        let closed = active.stored_segment_count > before;
        self.active = Some(active);
        match result {
            Err(error) => self.cancel_active(error.code, "segment_failed"),
            Ok(()) if closed => self.emit_active("segment_closed", None, Some(&artifact_id)),
            Ok(()) => {}
        }
    }

    pub(super) fn update_diagnostics(&mut self) {
        let token = self.diagnostics.token();
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if token.is_some()
            && active
                .diagnostics
                .as_ref()
                .is_some_and(crate::microphone_diagnostics::MicrophoneDiagnostics::is_active)
        {
            return;
        }
        self.retire_invalid_diagnostics();
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if let Some(token) = token {
            let (capture_path, purpose) = active.reservation.purpose.diagnostic_labels();
            crate::microphone_diagnostics::CaptureDiagnosticIdentity::new(
                capture_path,
                purpose,
                &active.reservation.intent_id,
            )
            .with_token(Some(token))
            .emit(
                &active.session_id,
                "native_observation_unavailable",
                log::Level::Warn,
                crate::microphone_diagnostics::MicrophoneFields {
                    reason: Some("enabled_mid_capture_next_session"),
                    ..Default::default()
                },
            );
        }
    }

    pub(super) fn retire_invalid_diagnostics(&mut self) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active
            .diagnostics
            .as_ref()
            .is_none_or(crate::microphone_diagnostics::MicrophoneDiagnostics::is_active)
        {
            return;
        }
        if let Some(observer) = active.diagnostics.take() {
            observer.shutdown();
        }
        active.device_observer.take();
    }

    /// Applies one backend event to the current capture, in either capture phase.
    /// Returns readiness or the terminal code that `wait_for_first_callback` reports.
    pub(super) fn handle_backend_event(
        &mut self,
        event: BackendEvent,
    ) -> Option<Result<CaptureSessionMetadata, CaptureErrorCode>> {
        let (BackendEvent::FirstCallback(generation)
        | BackendEvent::PcmLimitReached(generation)
        | BackendEvent::StreamError(generation, _)
        | BackendEvent::DefaultInputChanged(generation, _)) = event;
        if self.active_generation() != Some(generation) {
            return None;
        }
        match event {
            BackendEvent::FirstCallback(_) => self
                .active
                .as_ref()
                .is_some_and(|active| active.phase == CapturePhase::WaitingForFirstCallback)
                .then(|| Ok(self.mark_callback_observed())),
            BackendEvent::PcmLimitReached(_) => {
                self.emit_active("pcm_limit", Some(CaptureErrorCode::PcmMemoryLimit), None);
                None
            }
            BackendEvent::StreamError(_, CaptureErrorCode::BufferUnderrun) => {
                self.emit_active("stream_error", Some(CaptureErrorCode::BufferUnderrun), None);
                None
            }
            BackendEvent::StreamError(_, code) => {
                self.cancel_active(code, "stream_error");
                Some(Err(code))
            }
            BackendEvent::DefaultInputChanged(_, outcome) => {
                self.emit_active(outcome, None, None);
                if let Some(observer) = self
                    .active
                    .as_ref()
                    .and_then(|active| active.diagnostics.as_ref())
                {
                    emit_device_snapshot(observer, outcome);
                }
                None
            }
        }
    }

    /// Handles at most one batch of queued events, then the active capture's latched terminal
    /// error or readiness in case a full queue dropped it. Stops at the first readiness or
    /// terminal result, leaving later events queued for the next drain.
    ///
    /// The batch bound lets callers check cancellation, deadlines and commands between
    /// batches: callbacks can refill the queue as fast as the actor drains it (CoreAudio
    /// reports `Xrun` on every overload notification), so draining until empty could
    /// postpone the first-callback deadline indefinitely.
    pub(super) fn drain_backend_events(
        &mut self,
    ) -> Option<Result<CaptureSessionMetadata, CaptureErrorCode>> {
        for _ in 0..BACKEND_EVENT_BATCH {
            let Ok(event) = self.backend_events.try_recv() else {
                break;
            };
            if let Some(result) = self.handle_backend_event(event) {
                return Some(result);
            }
        }
        let latched = self.active.as_ref()?.buffer.latched_event()?;
        self.handle_backend_event(latched)
    }

    pub(super) fn process_backend_events(&mut self) {
        // Outside the first-callback wait nobody takes the result; handling has applied it.
        let _ = self.drain_backend_events();
    }

    pub(super) fn enforce_deadlines(&mut self) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        let elapsed = self.now().duration_since(active.capture_started_at);
        if active.phase == CapturePhase::WaitingForFirstCallback
            && elapsed >= self.policy.first_callback_timeout
        {
            self.cancel_active(
                CaptureErrorCode::FirstCallbackTimeout,
                "first_callback_timeout",
            );
            return;
        }
        if elapsed >= self.policy.maximum_capture_duration {
            let request = StopCapture {
                owner: active.reservation.owner.clone(),
                intent_id: active.reservation.intent_id.clone(),
                session_id: active.session_id.clone(),
            };
            let _ = self.stop(request);
            return;
        }
        let second = elapsed.as_secs();
        if let Some(active) = self.active.as_mut() {
            if active.phase == CapturePhase::Recording && second > active.last_progress_second {
                active.last_progress_second = second;
                self.emit_active("recording_progress", None, None);
            }
        }
    }

    pub(super) fn cancel_request(&mut self, request: CancelCapture) {
        let should_cancel = self.active.as_ref().is_some_and(|active| {
            active.reservation.owner == request.owner
                && active.reservation.intent_id == request.intent_id
                && request
                    .session_id
                    .as_deref()
                    .is_none_or(|id| id == active.session_id)
        });
        if should_cancel {
            self.cancel_active(CaptureErrorCode::CaptureCancelled, request.reason);
        }
    }

    pub(super) fn cleanup_owner(&mut self, owner: TrustedCaptureOwner) {
        self.gate.cancel_owner(&owner);
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.reservation.owner == owner)
        {
            self.cancel_active(CaptureErrorCode::CaptureCancelled, "owner_exit");
        }
        self.artifacts.discard_owner(&owner);
    }

    pub(super) fn cancel_active(&mut self, code: CaptureErrorCode, outcome: &'static str) {
        let Some(mut active) = self.active.take() else {
            return;
        };
        let _ = self
            .artifacts
            .discard(&active.reservation.owner, &active.artifact_id);
        self.gate.release(active.reservation.generation);
        self.last_error = Some(code);
        self.emit_terminal(&active, outcome, Some(code), CapturePhase::Idle);
        if let Some(observer) = active.diagnostics.as_ref() {
            emit_device_snapshot(observer, "before_cancel");
        }
        emit_stream_drop(active.diagnostics.as_ref(), outcome);
        drop(active.stream.take());
        emit_stream_dropped(active.diagnostics.as_ref());
        if let Some(observer) = active.diagnostics {
            observer.shutdown();
        }
    }
}
