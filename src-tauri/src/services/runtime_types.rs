use serde::Serialize;

pub use crate::storage::TranscriptRecord;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EchoState {
    Idle,
    RequestingMicrophone,
    Recording,
    Stopping,
    Transcribing,
    Done,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EchoErrorCode {
    MicrophonePermissionDenied,
    MicrophoneUnavailable,
    CaptureFailed,
    EmptyAudio,
    MissingProviderKey,
    ProviderTimeout,
    ProviderFailed,
    HistoryWriteFailed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EchoSnapshot {
    pub mode: &'static str,
    pub native_modifier_available: Option<bool>,
    pub state: EchoState,
    pub active_provider_id: String,
    pub active_shortcut: String,
    pub last_error: Option<EchoErrorCode>,
    pub last_transcript: Option<TranscriptRecord>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopStateEnvelope {
    pub sequence: u64,
    pub snapshot: EchoSnapshot,
}
