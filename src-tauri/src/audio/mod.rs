//! Process-wide native microphone capture authority.
//!
//! The Tauri adapter reserves an intent before awaiting permission, then calls
//! `begin`. All blocking methods must run through `spawn_blocking`; the actor
//! thread is the only owner of the CPAL stream.

mod actor;
mod artifacts;
mod backend;
mod device_observation;
mod gate;
mod resample;
mod segmenter;
mod tail_level;
mod types;
mod wav;

use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use actor::{ActorCommand, ActorDependencies, AudioActor};
use backend::{CaptureBackend, CpalCaptureBackend};
use gate::CaptureGate;
pub(crate) use tail_level::{tail_level, TailLevel};
pub(crate) use types::MAXIMUM_SEGMENTS;
pub(crate) use wav::encode_wav;

#[cfg(test)]
use types::{CaptureSnapshot, PcmHeadroom};

pub use types::{
    ArtifactReleaseDisposition, ArtifactReleaseRequest, AudioArtifactLease, AudioArtifactMetadata,
    AudioCapturePolicy, AudioSegment, CancelCapture, CaptureError, CaptureErrorCode, CaptureEvent,
    CaptureEventSink, CaptureIntent, CapturePhase, CapturePurpose, CaptureReservation,
    CaptureSessionMetadata, NativeAudioArtifact, SegmentCutKind, StopCapture, TrustedCaptureOwner,
};

#[derive(Clone)]
pub struct DesktopAudioCapture {
    inner: Arc<CoordinatorInner>,
}

struct CoordinatorInner {
    gate: Arc<CaptureGate>,
    commands: mpsc::Sender<ActorCommand>,
    worker: Mutex<Option<JoinHandle<()>>>,
    operation_timeout: Duration,
    diagnostics: crate::microphone_diagnostic_event::DiagnosticControl,
    #[cfg(test)]
    backend_events: SyncSender<backend::BackendEvent>,
}

impl DesktopAudioCapture {
    pub fn start(
        policy: AudioCapturePolicy,
        event_sink: CaptureEventSink,
    ) -> Result<Self, CaptureError> {
        Self::start_with_backend(CpalCaptureBackend, policy, event_sink)
    }

    fn start_with_backend<Backend: CaptureBackend>(
        backend: Backend,
        policy: AudioCapturePolicy,
        event_sink: CaptureEventSink,
    ) -> Result<Self, CaptureError> {
        let gate = Arc::new(CaptureGate::default());
        let (commands, command_receiver) = mpsc::channel();
        let (backend_events, backend_event_receiver) = mpsc::sync_channel(32);
        #[cfg(test)]
        let test_backend_events = backend_events.clone();
        let actor_gate = Arc::clone(&gate);
        let diagnostics = crate::microphone_diagnostic_event::DiagnosticControl::default();
        let actor_diagnostics = diagnostics.clone();
        let operation_timeout = policy.operation_timeout;
        let worker = std::thread::Builder::new()
            .name("echo-audio-capture".to_owned())
            .spawn(move || {
                AudioActor::new(
                    backend,
                    ActorDependencies {
                        gate: actor_gate,
                        policy,
                        event_sink,
                        backend_events: backend_event_receiver,
                        backend_event_sender: backend_events,
                        diagnostics: actor_diagnostics,
                    },
                )
                .run(command_receiver);
            })
            .map_err(|_| {
                CaptureError::new(CaptureErrorCode::CoordinatorUnavailable, "actor_start")
            })?;
        Ok(Self {
            inner: Arc::new(CoordinatorInner {
                gate,
                commands,
                worker: Mutex::new(Some(worker)),
                operation_timeout,
                diagnostics,
                #[cfg(test)]
                backend_events: test_backend_events,
            }),
        })
    }

    pub fn reserve(
        &self,
        owner: TrustedCaptureOwner,
        intent: CaptureIntent,
    ) -> Result<CaptureReservation, CaptureError> {
        let reservation = self
            .inner
            .gate
            .reserve(owner, intent, self.inner.diagnostics.token())?;
        if self
            .inner
            .commands
            .send(ActorCommand::Reserved(reservation.clone()))
            .is_err()
        {
            self.inner.gate.release(reservation.generation);
            return Err(unavailable("reserve"));
        }
        Ok(reservation)
    }

    pub fn begin(
        &self,
        reservation: CaptureReservation,
    ) -> Result<CaptureSessionMetadata, CaptureError> {
        let generation = reservation.generation;
        match self.request(|reply| ActorCommand::Begin(reservation, reply)) {
            Err(error) if error.stage == "actor_timeout" => {
                self.inner.gate.invalidate(generation);
                Err(error)
            }
            result => result?,
        }
    }

    pub fn stop(&self, request: StopCapture) -> Result<AudioArtifactMetadata, CaptureError> {
        self.request(|reply| ActorCommand::Stop(request, reply))?
    }

    pub fn cancel(&self, request: CancelCapture) -> Result<(), CaptureError> {
        let had_active_intent = self.inner.gate.cancel(&request)?;
        if had_active_intent {
            self.inner
                .commands
                .send(ActorCommand::Cancel(request))
                .map_err(|_| unavailable("cancel"))?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn snapshot(&self) -> Result<CaptureSnapshot, CaptureError> {
        self.request(ActorCommand::Snapshot)
    }

    pub fn lease_artifact(
        &self,
        owner: TrustedCaptureOwner,
        artifact_id: String,
    ) -> Result<AudioArtifactLease, CaptureError> {
        self.request(|reply| ActorCommand::Lease(owner, artifact_id, reply))?
    }

    pub fn read_artifact(
        &self,
        owner: TrustedCaptureOwner,
        artifact_id: String,
        lease_id: String,
    ) -> Result<NativeAudioArtifact, CaptureError> {
        self.request(|reply| ActorCommand::Read(owner, artifact_id, lease_id, reply))?
    }

    pub fn release_artifact(&self, request: ArtifactReleaseRequest) -> Result<(), CaptureError> {
        self.request(|reply| ActorCommand::Release(request, reply))?
    }

    pub fn cleanup_owner(&self, owner: TrustedCaptureOwner) -> Result<(), CaptureError> {
        self.inner.gate.cancel_owner(&owner);
        self.inner
            .commands
            .send(ActorCommand::CleanupOwner(owner))
            .map_err(|_| unavailable("owner_cleanup"))
    }

    pub fn apply_diagnostics_setting(
        &self,
        revision: u64,
        enabled: bool,
    ) -> Result<(), CaptureError> {
        if !self.inner.diagnostics.apply(revision, enabled) {
            return Ok(());
        }
        self.inner
            .commands
            .send(ActorCommand::DiagnosticsChanged)
            .map_err(|_| unavailable("diagnostics_setting"))
    }

    pub fn shutdown(&self) {
        self.inner.shutdown();
    }

    fn request<T>(
        &self,
        command: impl FnOnce(SyncSender<T>) -> ActorCommand,
    ) -> Result<T, CaptureError> {
        let (reply, response) = mpsc::sync_channel(1);
        self.inner
            .commands
            .send(command(reply))
            .map_err(|_| unavailable("actor_request"))?;
        response
            .recv_timeout(self.inner.operation_timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => unavailable("actor_timeout"),
                mpsc::RecvTimeoutError::Disconnected => unavailable("actor_response"),
            })
    }

    #[cfg(test)]
    pub(crate) fn freeze_time(&self) {
        self.request(ActorCommand::FreezeTime).unwrap();
    }

    #[cfg(test)]
    pub(crate) fn advance_time(&self, duration: Duration) {
        self.request(|reply| ActorCommand::AdvanceTime(duration, reply))
            .unwrap();
    }

    #[cfg(test)]
    fn inject_backend_event(&self, event: backend::BackendEvent) {
        self.inner.backend_events.try_send(event).unwrap();
    }

    #[cfg(test)]
    fn inject_diagnostics_refresh(&self) {
        self.inner
            .commands
            .send(ActorCommand::DiagnosticsChanged)
            .unwrap();
    }

    #[cfg(test)]
    fn diagnostics_active(&self) -> bool {
        self.request(ActorCommand::DiagnosticsActive).unwrap()
    }
}

impl CoordinatorInner {
    fn shutdown(&self) {
        let worker = self.worker.lock().ok().and_then(|mut value| value.take());
        if let Some(worker) = worker {
            self.gate.invalidate_current();
            let _ = self.commands.send(ActorCommand::Shutdown);
            let deadline = Instant::now() + Duration::from_millis(250);
            while !worker.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}

impl Drop for CoordinatorInner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn unavailable(stage: &'static str) -> CaptureError {
    CaptureError::new(CaptureErrorCode::CoordinatorUnavailable, stage)
}

#[cfg(test)]
pub(crate) mod tests;
