use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::artifacts::{ArtifactRelease, ArtifactStore};
use super::backend::{BackendEvent, CaptureBackend, CaptureBuffer, CaptureBufferContext};
use super::gate::CaptureGate;
use super::types::{
    ArtifactReleaseDisposition, ArtifactReleaseRequest, AudioArtifactLease, AudioArtifactMetadata,
    AudioCapturePolicy, CancelCapture, CaptureError, CaptureErrorCode, CaptureEvent,
    CaptureEventSink, CapturePhase, CaptureReservation, CaptureSessionMetadata, CaptureSnapshot,
    NativeAudioArtifact, StopCapture, TrustedCaptureOwner,
};

mod lifecycle;
mod maintenance;
mod setup;
mod telemetry;

pub(crate) enum ActorCommand {
    Reserved(CaptureReservation),
    Begin(
        CaptureReservation,
        SyncSender<Result<CaptureSessionMetadata, CaptureError>>,
    ),
    Stop(
        StopCapture,
        SyncSender<Result<AudioArtifactMetadata, CaptureError>>,
    ),
    Cancel(CancelCapture),
    #[cfg(test)]
    Snapshot(SyncSender<CaptureSnapshot>),
    Lease(
        TrustedCaptureOwner,
        String,
        SyncSender<Result<AudioArtifactLease, CaptureError>>,
    ),
    Read(
        TrustedCaptureOwner,
        String,
        String,
        SyncSender<Result<NativeAudioArtifact, CaptureError>>,
    ),
    Release(ArtifactReleaseRequest, SyncSender<Result<(), CaptureError>>),
    CleanupOwner(TrustedCaptureOwner),
    DiagnosticsChanged,
    #[cfg(test)]
    DiagnosticsActive(SyncSender<bool>),
    #[cfg(test)]
    DrainSegments(SyncSender<Vec<super::types::AudioSegment>>),
    #[cfg(test)]
    FreezeTime(SyncSender<()>),
    #[cfg(test)]
    AdvanceTime(Duration, SyncSender<()>),
    Shutdown,
}

struct ActiveCapture<Stream> {
    reservation: CaptureReservation,
    session_id: String,
    phase: CapturePhase,
    stream: Option<Stream>,
    buffer: CaptureBuffer,
    segmenter: super::segmenter::Segmenter,
    artifact_id: String,
    stored_segment_count: usize,
    sample_rate: u32,
    channel_count: u16,
    sample_format: &'static str,
    capture_started_at: Instant,
    last_progress_second: u64,
    diagnostics: Option<crate::microphone_diagnostics::MicrophoneDiagnostics>,
    device_observer: Option<super::device_observation::DefaultInputDeviceObserver>,
}

impl<Stream> ActiveCapture<Stream> {
    fn duration_ms(&self) -> u64 {
        (self.buffer.stats().1 / usize::from(self.channel_count)) as u64 * 1_000
            / u64::from(self.sample_rate)
    }
}

pub(crate) struct AudioActor<Backend: CaptureBackend> {
    backend: Backend,
    gate: Arc<CaptureGate>,
    policy: AudioCapturePolicy,
    event_sink: CaptureEventSink,
    backend_events: Receiver<BackendEvent>,
    backend_event_sender: SyncSender<BackendEvent>,
    diagnostics: crate::microphone_diagnostic_event::DiagnosticControl,
    active: Option<ActiveCapture<Backend::Stream>>,
    artifacts: ArtifactStore,
    sequence: u64,
    last_error: Option<CaptureErrorCode>,
    #[cfg(test)]
    test_now: Option<Instant>,
}

pub(crate) struct ActorDependencies {
    pub(crate) gate: Arc<CaptureGate>,
    pub(crate) policy: AudioCapturePolicy,
    pub(crate) event_sink: CaptureEventSink,
    pub(crate) backend_events: Receiver<BackendEvent>,
    pub(crate) backend_event_sender: SyncSender<BackendEvent>,
    pub(crate) diagnostics: crate::microphone_diagnostic_event::DiagnosticControl,
}

impl<Backend: CaptureBackend> AudioActor<Backend> {
    pub(crate) fn new(backend: Backend, dependencies: ActorDependencies) -> Self {
        let artifacts = ArtifactStore::new(&dependencies.policy);
        Self {
            backend,
            gate: dependencies.gate,
            policy: dependencies.policy,
            event_sink: dependencies.event_sink,
            backend_events: dependencies.backend_events,
            backend_event_sender: dependencies.backend_event_sender,
            diagnostics: dependencies.diagnostics,
            active: None,
            artifacts,
            sequence: 0,
            last_error: None,
            #[cfg(test)]
            test_now: None,
        }
    }

    pub(crate) fn run(mut self, commands: Receiver<ActorCommand>) {
        loop {
            match commands.recv_timeout(Duration::from_millis(20)) {
                Ok(ActorCommand::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(command) => self.handle_command(command),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.process_backend_events();
            self.enforce_deadlines();
            self.drain_capture();
            self.reap_artifacts();
        }
        self.cancel_active(CaptureErrorCode::CaptureCancelled, "shutdown");
    }

    fn now(&self) -> Instant {
        #[cfg(test)]
        if let Some(now) = self.test_now {
            return now;
        }
        Instant::now()
    }

    fn handle_command(&mut self, command: ActorCommand) {
        match command {
            ActorCommand::Reserved(reservation) => {
                self.emit_reservation(&reservation, "reserved", None);
            }
            ActorCommand::Begin(reservation, reply) => {
                let _ = reply.send(self.begin(reservation));
            }
            ActorCommand::Stop(request, reply) => {
                let _ = reply.send(self.stop(request));
            }
            ActorCommand::Cancel(request) => self.cancel_request(request),
            #[cfg(test)]
            ActorCommand::Snapshot(reply) => {
                let _ = reply.send(self.snapshot());
            }
            ActorCommand::Lease(owner, artifact_id, reply) => {
                let _ = reply.send(self.artifacts.lease(&owner, &artifact_id, self.now()));
            }
            ActorCommand::Read(owner, artifact_id, lease_id, reply) => {
                let result = self
                    .artifacts
                    .read(&owner, &artifact_id, &lease_id, self.now());
                let _ = reply.send(result);
            }
            ActorCommand::Release(request, reply) => {
                let result = self.artifacts.release(ArtifactRelease {
                    owner: &request.owner,
                    artifact_id: &request.artifact_id,
                    lease_id: &request.lease_id,
                    retain_until_idle_expiry: matches!(
                        request.disposition,
                        ArtifactReleaseDisposition::RetainUntilIdleExpiry
                    ),
                    now: self.now(),
                });
                let _ = reply.send(result);
            }
            ActorCommand::CleanupOwner(owner) => self.cleanup_owner(owner),
            ActorCommand::DiagnosticsChanged => self.update_diagnostics(),
            #[cfg(test)]
            ActorCommand::DiagnosticsActive(reply) => {
                let is_active = self
                    .active
                    .as_ref()
                    .and_then(|active| active.diagnostics.as_ref())
                    .is_some_and(crate::microphone_diagnostics::MicrophoneDiagnostics::is_active);
                let _ = reply.send(is_active);
            }
            #[cfg(test)]
            ActorCommand::DrainSegments(reply) => {
                self.drain_capture();
                let segments = self
                    .active
                    .as_ref()
                    .map_or_else(Vec::new, |active| active.segmenter.segments().to_vec());
                let _ = reply.send(segments);
            }
            #[cfg(test)]
            ActorCommand::FreezeTime(reply) => {
                self.test_now = Some(Instant::now());
                let _ = reply.send(());
            }
            #[cfg(test)]
            ActorCommand::AdvanceTime(duration, reply) => {
                self.test_now = Some(self.now() + duration);
                self.enforce_deadlines();
                self.drain_capture();
                self.reap_artifacts();
                let _ = reply.send(());
            }
            ActorCommand::Shutdown => {}
        }
    }
}
