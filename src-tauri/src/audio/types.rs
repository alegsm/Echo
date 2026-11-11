use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// All production recording duration, piece and encoded-cache budgets derive from this cap.
pub const MAXIMUM_CAPTURE_DURATION: Duration = Duration::from_secs(600);
pub(crate) const MINIMUM_SEGMENT_SECONDS: usize = 2;
pub(crate) const MAXIMUM_SEGMENTS: usize =
    (MAXIMUM_CAPTURE_DURATION.as_secs() as usize).div_ceil(MINIMUM_SEGMENT_SECONDS) + 1;
// PCM16 mono at the normalized rate, plus a header and rounding sample per piece.
const MAXIMUM_BUNDLE_BYTES: usize =
    MAXIMUM_CAPTURE_DURATION.as_secs() as usize * super::resample::SAMPLE_RATE as usize * 2
        + MAXIMUM_SEGMENTS * (44 + 2);
const MAXIMUM_BUNDLES: usize = 4;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapturePurpose {
    PushToTalk,
}

impl CapturePurpose {
    pub(crate) fn diagnostic_labels(self) -> (&'static str, &'static str) {
        match self {
            Self::PushToTalk => ("native_push_to_talk", "push_to_talk"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapturePhase {
    Idle,
    PermissionPending,
    Opening,
    WaitingForFirstCallback,
    Recording,
    Stopping,
}

#[derive(Clone, Debug)]
pub struct CaptureIntent {
    pub intent_id: String,
    pub purpose: CapturePurpose,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustedCaptureOwner(String);

impl TrustedCaptureOwner {
    /// The Tauri adapter calls this with its injected `WebviewWindow::label()`.
    /// Renderer request payloads must never supply this value.
    pub(crate) fn from_window_label(label: &str) -> Self {
        Self(label.to_owned())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub struct CaptureReservation {
    pub(crate) owner: TrustedCaptureOwner,
    pub(crate) intent_id: String,
    pub(crate) purpose: CapturePurpose,
    pub(crate) diagnostics_token: Option<crate::microphone_diagnostic_event::DiagnosticToken>,
    pub(crate) generation: u64,
}

impl CaptureReservation {
    pub(crate) fn diagnostics_token(
        &self,
    ) -> Option<crate::microphone_diagnostic_event::DiagnosticToken> {
        self.diagnostics_token.clone()
    }
}

#[derive(Clone, Debug)]
pub struct StopCapture {
    pub owner: TrustedCaptureOwner,
    pub intent_id: String,
    pub session_id: String,
}

#[derive(Clone, Debug)]
pub struct CancelCapture {
    pub owner: TrustedCaptureOwner,
    pub intent_id: String,
    pub session_id: Option<String>,
    pub reason: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSessionMetadata {
    pub session_id: String,
    pub intent_id: String,
    pub purpose: CapturePurpose,
    pub phase: CapturePhase,
    /// Device format, also used by capture diagnostics.
    pub sample_rate: u32,
    pub channel_count: u16,
    pub sample_format: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioArtifactMetadata {
    pub artifact_id: String,
    pub session_id: String,
    pub intent_id: String,
    pub purpose: CapturePurpose,
    pub mime_type: &'static str,
    pub byte_size: usize,
    pub duration_ms: u64,
    /// Encoded WAV format after normalization, independent of the device format.
    pub sample_rate: u32,
    pub channel_count: u16,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSnapshot {
    pub sequence: u64,
    pub phase: CapturePhase,
    pub purpose: Option<CapturePurpose>,
    pub intent_id: Option<String>,
    pub session_id: Option<String>,
    pub has_observed_callback: bool,
    pub callback_count: u64,
    pub sample_count: usize,
    pub last_error: Option<CaptureErrorCode>,
    pub artifact_count: usize,
    pub artifact_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureErrorCode {
    MicrophoneBusy,
    MicrophonePermissionDenied,
    MicrophoneUnavailable,
    DeviceConfigFailed,
    StreamBuildFailed,
    StreamPlayFailed,
    DeviceUnavailable,
    StreamInvalidated,
    BufferUnderrun,
    StreamBackendError,
    FirstCallbackTimeout,
    PcmMemoryLimit,
    ArtifactSizeLimit,
    ArtifactCapacityReached,
    ArtifactExpired,
    RecordingTooShort,
    LowSignal,
    InvalidOwner,
    InvalidReservation,
    SessionNotFound,
    CaptureCancelled,
    CoordinatorUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureError {
    pub code: CaptureErrorCode,
    pub phase: CapturePhase,
    pub active_purpose: Option<CapturePurpose>,
    pub retryable: bool,
    pub stage: &'static str,
}

impl CaptureError {
    pub(crate) fn new(code: CaptureErrorCode, stage: &'static str) -> Self {
        Self {
            code,
            phase: CapturePhase::Idle,
            active_purpose: None,
            retryable: is_retryable(code),
            stage,
        }
    }

    pub(crate) fn busy(phase: CapturePhase, purpose: CapturePurpose) -> Self {
        Self {
            code: CaptureErrorCode::MicrophoneBusy,
            phase,
            active_purpose: Some(purpose),
            retryable: true,
            stage: "reserve",
        }
    }

    pub(crate) fn at_phase(mut self, phase: CapturePhase) -> Self {
        self.phase = phase;
        self
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?} during {}", self.code, self.stage)
    }
}

impl std::error::Error for CaptureError {}

fn is_retryable(code: CaptureErrorCode) -> bool {
    !matches!(
        code,
        CaptureErrorCode::InvalidOwner
            | CaptureErrorCode::InvalidReservation
            | CaptureErrorCode::ArtifactSizeLimit
            | CaptureErrorCode::PcmMemoryLimit
            | CaptureErrorCode::LowSignal
    )
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureEvent {
    pub elapsed_ms: u64,
    pub limit_ms: u64,
    pub sequence: u64,
    pub phase: CapturePhase,
    pub purpose: Option<CapturePurpose>,
    pub intent_id: Option<String>,
    pub session_id: Option<String>,
    pub artifact_id: Option<String>,
    pub outcome: &'static str,
    pub error: Option<CaptureErrorCode>,
    pub has_observed_callback: bool,
    pub callback_count: u64,
    pub sample_count: usize,
    pub last_error: Option<CaptureErrorCode>,
    pub artifact_count: usize,
    pub artifact_bytes: usize,
}

pub type CaptureEventSink = Arc<dyn Fn(CaptureEvent) + Send + Sync>;

#[derive(Clone, Debug)]
pub struct AudioArtifactLease {
    pub lease_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactReleaseDisposition {
    Discard,
    RetainUntilIdleExpiry,
}

pub struct ArtifactReleaseRequest {
    pub owner: TrustedCaptureOwner,
    pub artifact_id: String,
    pub lease_id: String,
    pub disposition: ArtifactReleaseDisposition,
}

#[derive(Clone)]
pub struct NativeAudioArtifact {
    pub stopped_at: Option<std::time::Instant>,
    pub is_complete: bool,
    pub metadata: AudioArtifactMetadata,
    pub segments: Arc<[AudioSegment]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SegmentCutKind {
    QuietWindow,
    Valley,
    ReleaseTail,
}

/// One closed piece. A dropped silent piece keeps its ordinal and has no bytes.
#[derive(Clone, Debug)]
pub struct AudioSegment {
    pub cut_kind: SegmentCutKind,
    pub ordinal: u32,
    pub duration_ms: u64,
    pub bytes: Option<Arc<[u8]>>,
}

/// Scheduling headroom is independent of the segment cut duration.
const PCM_STALL_BUDGET: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug)]
pub enum PcmHeadroom {
    StallBudget(Duration),
    #[cfg(test)]
    Bytes(usize),
}

/// Below this RMS a recording or frame counts as silence rather than speech.
pub(crate) const MINIMUM_SIGNAL_RMS: f64 = 0.003;

#[derive(Clone, Debug)]
pub struct AudioCapturePolicy {
    pub operation_timeout: Duration,
    pub minimum_capture_duration: Duration,
    pub minimum_sample_count: usize,
    pub minimum_signal_rms: f64,
    pub minimum_signal_peak: f32,
    pub first_callback_timeout: Duration,
    pub maximum_capture_duration: Duration,
    pub pcm_headroom: PcmHeadroom,
    pub maximum_artifact_bytes: usize,
    pub maximum_artifact_count: usize,
    pub maximum_total_artifact_bytes: usize,
    pub artifact_ttl: Duration,
    pub lease_ttl: Duration,
    pub release_ttl: Duration,
}

impl Default for AudioCapturePolicy {
    fn default() -> Self {
        Self {
            operation_timeout: Duration::from_secs(if cfg!(test) { 60 } else { 8 }),
            minimum_capture_duration: Duration::from_millis(300),
            minimum_sample_count: 512,
            minimum_signal_rms: MINIMUM_SIGNAL_RMS,
            minimum_signal_peak: 0.02,
            first_callback_timeout: Duration::from_secs(3),
            maximum_capture_duration: MAXIMUM_CAPTURE_DURATION,
            pcm_headroom: PcmHeadroom::StallBudget(PCM_STALL_BUDGET),
            maximum_artifact_bytes: MAXIMUM_BUNDLE_BYTES,
            maximum_artifact_count: MAXIMUM_BUNDLES,
            maximum_total_artifact_bytes: MAXIMUM_BUNDLES * MAXIMUM_BUNDLE_BYTES,
            artifact_ttl: Duration::from_secs(60),
            lease_ttl: Duration::from_secs(45),
            release_ttl: Duration::from_secs(60),
        }
    }
}
