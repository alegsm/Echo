use super::setup::*;
use super::telemetry::*;
use super::*;
use crate::audio::artifacts::BundleIdentity;

struct CapturePreparation {
    buffer: CaptureBuffer,
    diagnostics: Option<crate::microphone_diagnostics::MicrophoneDiagnostics>,
    device_observer: Option<crate::audio::device_observation::DefaultInputDeviceObserver>,
}

struct PlayedCapture<Stream> {
    opened: super::super::backend::OpenedCapture<Stream>,
    preparation: CapturePreparation,
    started_at: Instant,
}

impl<Backend: CaptureBackend> AudioActor<Backend> {
    pub(super) fn begin(
        &mut self,
        reservation: CaptureReservation,
    ) -> Result<CaptureSessionMetadata, CaptureError> {
        let session_id = self.initialize_reservation(&reservation)?;
        let artifact_id = uuid::Uuid::new_v4().to_string();
        if let Err(error) = self.artifacts.create(
            BundleIdentity {
                owner: reservation.owner.clone(),
                artifact_id: artifact_id.clone(),
                session_id: session_id.clone(),
                intent_id: reservation.intent_id.clone(),
                purpose: reservation.purpose,
            },
            self.now(),
        ) {
            self.fail_start(&reservation, None, error.clone());
            return Err(error);
        }
        let owner = reservation.owner.clone();
        let result = (|| {
            let preparation = self.prepare_capture(&reservation, &session_id);
            let played = self.open_and_play(&reservation, preparation)?;
            self.activate_capture(reservation, session_id, artifact_id.clone(), played);
            self.wait_for_first_callback()
        })();
        if result.is_err() {
            let _ = self.artifacts.discard(&owner, &artifact_id);
        }
        result
    }

    fn initialize_reservation(
        &mut self,
        reservation: &CaptureReservation,
    ) -> Result<String, CaptureError> {
        if !self.gate.is_current(reservation.generation) {
            self.gate.release(reservation.generation);
            return Err(CaptureError::new(
                CaptureErrorCode::CaptureCancelled,
                "begin",
            ));
        }
        let session_id = uuid::Uuid::new_v4().to_string();
        self.gate.attach_session(reservation, &session_id)?;
        self.gate
            .set_phase(reservation.generation, CapturePhase::Opening);
        self.emit_reservation(reservation, "opening", None);
        Ok(session_id)
    }

    fn prepare_capture(
        &mut self,
        reservation: &CaptureReservation,
        session_id: &str,
    ) -> CapturePreparation {
        let diagnostics = start_diagnostics(reservation, session_id);
        let buffer = CaptureBuffer::new(CaptureBufferContext {
            generation: reservation.generation,
            gate: Arc::clone(&self.gate),
            events: self.backend_event_sender.clone(),
            diagnostics: diagnostics.as_ref().map(|observer| observer.callback()),
        });
        CapturePreparation {
            buffer,
            diagnostics,
            device_observer: None,
        }
    }

    fn open_and_play(
        &mut self,
        reservation: &CaptureReservation,
        mut preparation: CapturePreparation,
    ) -> Result<PlayedCapture<Backend::Stream>, CaptureError> {
        let opened = match self.backend.open(preparation.buffer.clone()) {
            Ok(opened) => opened,
            Err(error) => {
                self.fail_start(reservation, preparation.diagnostics.take(), error.clone());
                return Err(error);
            }
        };
        if let Err(error) =
            preparation
                .buffer
                .configure(opened.sample_rate, opened.channel_count, &self.policy)
        {
            self.fail_start(reservation, preparation.diagnostics.take(), error.clone());
            return Err(error);
        }
        emit_opened(preparation.diagnostics.as_ref(), &opened);
        if !self.gate.is_current(reservation.generation) {
            return self.drop_late_stream(reservation, preparation, opened.stream, "open");
        }
        preparation.device_observer = start_device_observation(
            reservation,
            preparation.diagnostics.as_ref(),
            self.backend_event_sender.clone(),
        );
        let started_at = self.now();
        if let Some(observer) = preparation.diagnostics.as_ref() {
            observer.mark_play_started();
        }
        if let Err(error) = self.backend.play(&opened.stream) {
            self.fail_start(reservation, preparation.diagnostics.take(), error.clone());
            return Err(error);
        }
        emit_played(preparation.diagnostics.as_ref());
        if !self.gate.is_current(reservation.generation) {
            return self.drop_late_stream(reservation, preparation, opened.stream, "play");
        }
        Ok(PlayedCapture {
            opened,
            preparation,
            started_at,
        })
    }

    fn drop_late_stream<T>(
        &mut self,
        reservation: &CaptureReservation,
        mut preparation: CapturePreparation,
        stream: Backend::Stream,
        stage: &'static str,
    ) -> Result<T, CaptureError> {
        emit_stream_drop(preparation.diagnostics.as_ref(), "cancelled_during_start");
        drop(stream);
        emit_stream_dropped(preparation.diagnostics.as_ref());
        if let Some(observer) = preparation.diagnostics.take() {
            observer.shutdown();
        }
        self.gate.release(reservation.generation);
        self.last_error = Some(CaptureErrorCode::CaptureCancelled);
        self.emit_current(EventOccurrence::error(
            "cancelled_during_start",
            CaptureErrorCode::CaptureCancelled,
        ));
        Err(CaptureError::new(CaptureErrorCode::CaptureCancelled, stage))
    }

    fn activate_capture(
        &mut self,
        reservation: CaptureReservation,
        session_id: String,
        artifact_id: String,
        played: PlayedCapture<Backend::Stream>,
    ) {
        self.gate.set_phase(
            reservation.generation,
            CapturePhase::WaitingForFirstCallback,
        );
        self.active = Some(ActiveCapture {
            reservation,
            session_id,
            phase: CapturePhase::WaitingForFirstCallback,
            stream: Some(played.opened.stream),
            buffer: played.preparation.buffer,
            segmenter: super::super::segmenter::Segmenter::new(
                played.opened.sample_rate,
                played.opened.channel_count,
                &self.policy,
            ),
            artifact_id,
            stored_segment_count: 0,
            sample_rate: played.opened.sample_rate,
            channel_count: played.opened.channel_count,
            sample_format: played.opened.sample_format,
            capture_started_at: played.started_at,
            last_progress_second: 0,
            diagnostics: played.preparation.diagnostics,
            device_observer: played.preparation.device_observer,
        });
        self.emit_active("play_returned", None, None);
    }

    pub(super) fn wait_for_first_callback(
        &mut self,
    ) -> Result<CaptureSessionMetadata, CaptureError> {
        let deadline = Instant::now() + self.policy.first_callback_timeout;
        loop {
            self.retire_invalid_diagnostics();
            let generation = self
                .active
                .as_ref()
                .map(|active| active.reservation.generation);
            if generation.is_none_or(|value| !self.gate.is_current(value)) {
                self.cancel_active(
                    CaptureErrorCode::CaptureCancelled,
                    "cancelled_before_callback",
                );
                return Err(CaptureError::new(
                    CaptureErrorCode::CaptureCancelled,
                    "first_callback",
                ));
            }
            if let Some(result) = self.drain_backend_events() {
                return result.map_err(|code| CaptureError::new(code, "first_callback"));
            }
            if Instant::now() >= deadline {
                self.cancel_active(
                    CaptureErrorCode::FirstCallbackTimeout,
                    "first_callback_timeout",
                );
                return Err(CaptureError::new(
                    CaptureErrorCode::FirstCallbackTimeout,
                    "first_callback",
                ));
            }
            match self.backend_events.recv_timeout(Duration::from_millis(20)) {
                Ok(event) => {
                    if let Some(result) = self.handle_backend_event(event) {
                        return result.map_err(|code| CaptureError::new(code, "first_callback"));
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(CaptureError::new(
                        CaptureErrorCode::CoordinatorUnavailable,
                        "first_callback",
                    ));
                }
            }
        }
    }

    pub(super) fn mark_callback_observed(&mut self) -> CaptureSessionMetadata {
        let metadata = {
            let active = self
                .active
                .as_mut()
                .expect("active capture exists while awaiting callback");
            active.phase = CapturePhase::Recording;
            self.gate
                .set_phase(active.reservation.generation, CapturePhase::Recording);
            CaptureSessionMetadata {
                session_id: active.session_id.clone(),
                intent_id: active.reservation.intent_id.clone(),
                purpose: active.reservation.purpose,
                phase: active.phase,
                sample_rate: active.sample_rate,
                channel_count: active.channel_count,
                sample_format: active.sample_format,
            }
        };
        self.emit_active("first_callback", None, None);
        if let Some(observer) = self
            .active
            .as_ref()
            .and_then(|active| active.diagnostics.as_ref())
        {
            emit_device_snapshot(observer, "first_callback");
        }
        metadata
    }

    pub(super) fn stop(
        &mut self,
        request: StopCapture,
    ) -> Result<AudioArtifactMetadata, CaptureError> {
        let stopped_at = std::time::Instant::now();
        if let Some(metadata) =
            self.artifacts
                .find_session(&request.owner, &request.session_id, &request.intent_id)
        {
            return Ok(metadata);
        }
        let mut active = self.take_matching_capture(&request)?;
        self.gate
            .set_phase(active.reservation.generation, CapturePhase::Stopping);
        self.emit_terminal(&active, "stop_requested", None, CapturePhase::Stopping);
        let duration = self.now().duration_since(active.capture_started_at);
        finish_stream(&mut active);
        let result = self.finish_audio(&mut active, duration, stopped_at);
        let sample_count = active.buffer.stats().1;
        let duration_ms = active.duration_ms();
        let bytes = active
            .segmenter
            .segments()
            .iter()
            .filter_map(|segment| segment.bytes.as_ref())
            .map(|bytes| bytes.len())
            .sum();
        self.finish_diagnostics(
            active.diagnostics.take(),
            capture_metrics(duration_ms, sample_count, bytes, active.channel_count),
        );
        self.gate.release(active.reservation.generation);
        match result {
            Ok(metadata) => {
                self.emit_terminal(&active, "artifact_created", None, CapturePhase::Idle);
                Ok(metadata)
            }
            Err(error) => {
                let _ = self
                    .artifacts
                    .discard(&active.reservation.owner, &active.artifact_id);
                self.last_error = Some(error.code);
                self.emit_terminal(&active, "stop_failed", Some(error.code), CapturePhase::Idle);
                Err(error)
            }
        }
    }

    fn finish_audio(
        &mut self,
        active: &mut ActiveCapture<Backend::Stream>,
        duration: Duration,
        stopped_at: std::time::Instant,
    ) -> Result<AudioArtifactMetadata, CaptureError> {
        let samples = active.buffer.stats().1;
        if duration < self.policy.minimum_capture_duration
            || samples < self.policy.minimum_sample_count
        {
            return Err(CaptureError::new(
                CaptureErrorCode::RecordingTooShort,
                "stop",
            ));
        }
        self.drain_active(active)?;
        active.segmenter.finish()?;
        self.store_segments(active)?;
        if !self.gate.is_current(active.reservation.generation) {
            return Err(CaptureError::new(
                CaptureErrorCode::CaptureCancelled,
                "stop",
            ));
        }
        if active
            .segmenter
            .segments()
            .iter()
            .all(|segment| segment.bytes.is_none())
        {
            return Err(CaptureError::new(CaptureErrorCode::LowSignal, "stop"));
        }
        self.artifacts.seal(
            &active.reservation.owner,
            &active.artifact_id,
            active.duration_ms(),
            stopped_at,
            self.now(),
        )
    }

    pub(super) fn drain_active(
        &mut self,
        active: &mut ActiveCapture<Backend::Stream>,
    ) -> Result<(), CaptureError> {
        // take() releases the callback lock before segmentation, resampling or encoding.
        for block in active.buffer.take()? {
            let result = active.segmenter.push(&block);
            active.buffer.recycle(block)?;
            result?;
            self.store_segments(active)?;
        }
        Ok(())
    }

    fn store_segments(
        &mut self,
        active: &mut ActiveCapture<Backend::Stream>,
    ) -> Result<(), CaptureError> {
        let pending = &active.segmenter.segments()[active.stored_segment_count..];
        if pending.is_empty() {
            return Ok(());
        }
        for segment in pending {
            self.artifacts.append(
                &active.reservation.owner,
                &active.artifact_id,
                segment.clone(),
                self.now(),
            )?;
            active.stored_segment_count = segment.ordinal as usize + 1;
        }
        Ok(())
    }

    fn take_matching_capture(
        &mut self,
        request: &StopCapture,
    ) -> Result<ActiveCapture<Backend::Stream>, CaptureError> {
        let Some(active) = self.active.take() else {
            return Err(CaptureError::new(CaptureErrorCode::SessionNotFound, "stop"));
        };
        let matches = active.reservation.owner == request.owner
            && active.reservation.intent_id == request.intent_id
            && active.session_id == request.session_id;
        if matches {
            Ok(active)
        } else {
            self.active = Some(active);
            Err(CaptureError::new(CaptureErrorCode::InvalidOwner, "stop"))
        }
    }
}
