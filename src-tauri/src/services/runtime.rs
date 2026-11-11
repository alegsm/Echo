use std::collections::BTreeSet;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::audio::{CaptureErrorCode, CaptureEvent, CapturePhase};
use crate::storage::PersistedSettings;
#[path = "runtime_types.rs"]
mod types;
pub use types::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeError {
    LockUnavailable,
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Desktop runtime state is unavailable.")
    }
}

impl std::error::Error for RuntimeError {}

struct RuntimeInner {
    sequence: u64,
    native_modifier_available: Option<bool>,
    settings_revision: Option<u64>,
    settings: PersistedSettings,
    capture_phase: CapturePhase,
    capture_intent_id: Option<String>,
    capture_overlaps_older_job: bool,
    newest_capture_intent_id: Option<String>,
    superseded_jobs: BTreeSet<u64>,
    active_jobs: BTreeSet<u64>,
    newest_started_job_sequence: u64,
    settled_state: EchoState,
    last_error: Option<EchoErrorCode>,
    last_transcript: Option<TranscriptRecord>,
    last_transcript_record_revision: Option<u64>,
    last_transcript_job_sequence: u64,
    capture_job_floor: u64,
}

#[derive(Clone)]
pub struct EchoRuntime {
    inner: Arc<Mutex<RuntimeInner>>,
}

impl Default for EchoRuntime {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RuntimeInner {
                sequence: 0,
                native_modifier_available: None,
                settings_revision: None,
                settings: PersistedSettings::default(),
                capture_phase: CapturePhase::Idle,
                capture_intent_id: None,
                capture_overlaps_older_job: false,
                newest_capture_intent_id: None,
                superseded_jobs: BTreeSet::new(),
                active_jobs: BTreeSet::new(),
                newest_started_job_sequence: 0,
                settled_state: EchoState::Idle,
                last_error: None,
                last_transcript: None,
                last_transcript_record_revision: None,
                last_transcript_job_sequence: 0,
                capture_job_floor: 0,
            })),
        }
    }
}

impl EchoRuntime {
    pub fn desktop_snapshot(&self) -> Result<DesktopStateEnvelope, RuntimeError> {
        let inner = self.lock()?;
        Ok(inner.envelope())
    }

    pub fn set_native_modifier_available(
        &self,
        available: bool,
    ) -> Result<DesktopStateEnvelope, RuntimeError> {
        let mut inner = self.lock()?;
        inner.native_modifier_available = Some(available);
        inner.advance();
        Ok(inner.envelope())
    }

    pub fn apply_capture_event(
        &self,
        event: &CaptureEvent,
    ) -> Result<DesktopStateEnvelope, RuntimeError> {
        let mut inner = self.lock()?;
        if event.intent_id.is_some() {
            if event.intent_id != inner.capture_intent_id {
                inner.capture_overlaps_older_job = !inner.active_jobs.is_empty();
            }
            inner.capture_intent_id = event.intent_id.clone();
        }
        if matches!(event.outcome, "segment_closed" | "artifact_created") {
            if let Some(intent) = inner.capture_intent_id.clone() {
                inner.claim_capture(&intent);
            }
        }
        let preserve_job_outcome = inner.capture_overlaps_older_job
            && inner.capture_intent_id != inner.newest_capture_intent_id
            && matches!(
                event.error,
                Some(CaptureErrorCode::CaptureCancelled | CaptureErrorCode::RecordingTooShort)
            );
        inner.capture_phase = event.phase;
        if event.phase == CapturePhase::Idle && event.outcome != "stop_requested" {
            inner.capture_intent_id = None;
        }
        if let Some(error) = event.error.filter(|_| !preserve_job_outcome) {
            inner.last_error = Some(capture_error(error));
            inner.settled_state = EchoState::Error;
        } else if event.phase != CapturePhase::Idle {
            inner.last_error = None;
        }
        inner.advance();
        Ok(inner.envelope())
    }

    pub fn job_started_for_capture(
        &self,
        job_sequence: u64,
        intent_id: &str,
    ) -> Result<DesktopStateEnvelope, RuntimeError> {
        let mut inner = self.lock()?;
        if inner.capture_intent_id.as_deref() == Some(intent_id) {
            inner.claim_capture(intent_id);
        } else if inner.capture_intent_id.is_some() {
            inner.capture_overlaps_older_job = true;
        }
        inner.active_jobs.insert(job_sequence);
        let superseded = inner
            .newest_capture_intent_id
            .as_deref()
            .is_some_and(|newest| intent_id != newest);
        if superseded {
            inner.superseded_jobs.insert(job_sequence);
        }
        if !superseded && job_sequence >= inner.newest_started_job_sequence {
            inner.newest_started_job_sequence = job_sequence;
            inner.last_error = None;
        }
        inner.advance();
        Ok(inner.envelope())
    }

    pub fn job_failed(
        &self,
        job_sequence: u64,
        error: EchoErrorCode,
    ) -> Result<DesktopStateEnvelope, RuntimeError> {
        let mut inner = self.lock()?;
        inner.active_jobs.remove(&job_sequence);
        if inner.owns_settled_outcome(job_sequence) {
            inner.last_error = Some(error);
            inner.settled_state = EchoState::Error;
        }
        inner.superseded_jobs.remove(&job_sequence);
        inner.advance();
        Ok(inner.envelope())
    }

    pub fn job_completed(
        &self,
        job_sequence: u64,
        transcript: TranscriptRecord,
        record_revision: Option<u64>,
    ) -> Result<DesktopStateEnvelope, RuntimeError> {
        self.finish_job(job_sequence, transcript, record_revision, None)
    }

    pub fn job_completed_with_warning(
        &self,
        job_sequence: u64,
        transcript: TranscriptRecord,
        record_revision: Option<u64>,
        warning: EchoErrorCode,
    ) -> Result<DesktopStateEnvelope, RuntimeError> {
        self.finish_job(job_sequence, transcript, record_revision, Some(warning))
    }

    fn finish_job(
        &self,
        job_sequence: u64,
        transcript: TranscriptRecord,
        record_revision: Option<u64>,
        warning: Option<EchoErrorCode>,
    ) -> Result<DesktopStateEnvelope, RuntimeError> {
        let mut inner = self.lock()?;
        inner.active_jobs.remove(&job_sequence);
        if job_sequence >= inner.last_transcript_job_sequence
            && job_sequence >= inner.capture_job_floor
            && !inner.superseded_jobs.contains(&job_sequence)
        {
            inner.last_transcript_job_sequence = job_sequence;
            inner.last_transcript = Some(transcript.clone());
            inner.last_transcript_record_revision = record_revision;
        }
        if inner.owns_settled_outcome(job_sequence) {
            inner.last_error = warning;
            inner.settled_state = if warning.is_some() {
                EchoState::Error
            } else {
                EchoState::Done
            };
        }
        inner.superseded_jobs.remove(&job_sequence);
        inner.advance();
        Ok(inner.envelope())
    }

    pub fn job_cancelled(&self, job_sequence: u64) -> Result<DesktopStateEnvelope, RuntimeError> {
        let mut inner = self.lock()?;
        inner.active_jobs.remove(&job_sequence);
        if inner.owns_settled_outcome(job_sequence) {
            inner.settled_state = EchoState::Idle;
            inner.last_error = None;
        }
        inner.superseded_jobs.remove(&job_sequence);
        inner.advance();
        Ok(inner.envelope())
    }

    pub fn replace_persisted_settings(
        &self,
        revision: u64,
        settings: &PersistedSettings,
    ) -> Result<DesktopStateEnvelope, RuntimeError> {
        let mut inner = self.lock()?;
        if inner
            .settings_revision
            .is_some_and(|current| revision <= current)
        {
            return Ok(inner.envelope());
        }
        inner.settings_revision = Some(revision);
        inner.settings = settings.clone();
        inner.advance();
        Ok(inner.envelope())
    }

    pub fn replace_last_transcript_if_current(
        &self,
        record_revision: u64,
        transcript: TranscriptRecord,
    ) -> Result<Option<DesktopStateEnvelope>, RuntimeError> {
        let mut inner = self.lock()?;
        let is_current = inner
            .last_transcript
            .as_ref()
            .is_some_and(|current| current.id == transcript.id);
        if !is_current {
            return Ok(None);
        }
        if inner
            .last_transcript_record_revision
            .is_some_and(|current| record_revision <= current)
        {
            return Ok(None);
        }
        inner.last_transcript = Some(transcript);
        inner.last_transcript_record_revision = Some(record_revision);
        inner.advance();
        Ok(Some(inner.envelope()))
    }

    pub fn clear_last_transcript_if_current(
        &self,
        transcript_id: &str,
    ) -> Result<Option<DesktopStateEnvelope>, RuntimeError> {
        let mut inner = self.lock()?;
        let is_current = inner
            .last_transcript
            .as_ref()
            .is_some_and(|current| current.id == transcript_id);
        if !is_current {
            return Ok(None);
        }
        inner.last_transcript = None;
        inner.last_transcript_record_revision = None;
        inner.advance();
        Ok(Some(inner.envelope()))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, RuntimeInner>, RuntimeError> {
        self.inner.lock().map_err(|_| RuntimeError::LockUnavailable)
    }
}

impl RuntimeInner {
    fn claim_capture(&mut self, intent_id: &str) {
        if self.newest_capture_intent_id.as_deref() != Some(intent_id) {
            self.capture_job_floor = self.newest_started_job_sequence.saturating_add(1);
            self.newest_capture_intent_id = Some(intent_id.to_owned());
            self.superseded_jobs
                .extend(self.active_jobs.iter().copied());
        }
    }

    fn owns_settled_outcome(&self, job_sequence: u64) -> bool {
        job_sequence == self.newest_started_job_sequence
            && job_sequence >= self.capture_job_floor
            && !self.superseded_jobs.contains(&job_sequence)
    }

    fn advance(&mut self) {
        self.sequence = self.sequence.saturating_add(1);
    }

    fn envelope(&self) -> DesktopStateEnvelope {
        DesktopStateEnvelope {
            sequence: self.sequence,
            snapshot: self.snapshot(),
        }
    }

    fn snapshot(&self) -> EchoSnapshot {
        EchoSnapshot {
            mode: "desktop",
            native_modifier_available: self.native_modifier_available,
            state: self.display_state(),
            active_provider_id: self.settings.provider_id.clone(),
            active_shortcut: self.settings.push_to_talk_shortcut.clone(),
            last_error: self.last_error,
            last_transcript: self.last_transcript.clone(),
        }
    }

    fn display_state(&self) -> EchoState {
        match self.capture_phase {
            CapturePhase::PermissionPending
            | CapturePhase::Opening
            | CapturePhase::WaitingForFirstCallback => EchoState::RequestingMicrophone,
            CapturePhase::Recording => EchoState::Recording,
            CapturePhase::Stopping => EchoState::Stopping,
            CapturePhase::Idle if !self.active_jobs.is_empty() => EchoState::Transcribing,
            CapturePhase::Idle => self.settled_state,
        }
    }
}

fn capture_error(error: CaptureErrorCode) -> EchoErrorCode {
    match error {
        CaptureErrorCode::MicrophonePermissionDenied => EchoErrorCode::MicrophonePermissionDenied,
        CaptureErrorCode::MicrophoneUnavailable | CaptureErrorCode::DeviceUnavailable => {
            EchoErrorCode::MicrophoneUnavailable
        }
        _ => EchoErrorCode::CaptureFailed,
    }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
