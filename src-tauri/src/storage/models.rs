use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::providers::ProviderId;

pub const SETTINGS_SCHEMA_VERSION: u32 = 1;
pub const MAX_HISTORY_PAGE: u32 = 200;
pub const MAX_AUDIO_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum StorageError {
    #[error("storage database operation failed")]
    Database,
    #[error("storage lock is unavailable")]
    LockUnavailable,
    #[error("invalid storage field: {0}")]
    Validation(&'static str),
    #[error("{resource} revision conflict: expected {expected}, actual {actual}")]
    RevisionConflict {
        resource: &'static str,
        expected: u64,
        actual: u64,
    },
    #[error("{0} was not found")]
    NotFound(&'static str),
    #[error("transcript id conflicts with different content")]
    TranscriptConflict,
}

impl From<rusqlite::Error> for StorageError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Database
    }
}

impl From<std::io::Error> for StorageError {
    fn from(_: std::io::Error) -> Self {
        Self::Database
    }
}

pub type StorageResult<T> = Result<T, StorageError>;

/// Whether committed deletions have also removed historical WAL frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupStatus {
    Complete,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersistedSettings {
    pub push_to_talk_shortcut: String,
    pub fallback_shortcut: Option<String>,
    pub provider_id: String,
    pub provider_key_statuses: BTreeMap<String, String>,
    pub provider_key_saved: BTreeMap<String, bool>,
    pub phrasebook_terms: Vec<String>,
    pub language: Option<String>,
    pub custom_prompt: Option<String>,
    pub paste_after_transcription: bool,
    pub restore_clipboard: bool,
    pub debug_logging_enabled: bool,
    pub history_retention_days: Option<u32>,
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            push_to_talk_shortcut: if cfg!(target_os = "windows") {
                "Control+Windows"
            } else {
                "Fn"
            }
            .into(),
            fallback_shortcut: Some("CommandOrControl+Shift+Space".into()),
            provider_id: "groq-whisper-turbo".into(),
            provider_key_statuses: BTreeMap::new(),
            provider_key_saved: ProviderId::ALL
                .iter()
                .map(|provider| (provider.as_str().into(), false))
                .collect(),
            phrasebook_terms: Vec::new(),
            language: None,
            custom_prompt: None,
            paste_after_transcription: true,
            restore_clipboard: true,
            debug_logging_enabled: false,
            history_retention_days: Some(30),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsPatch {
    pub push_to_talk_shortcut: Option<String>,
    #[serde(default, skip_serializing_if = "PatchValue::is_missing")]
    pub fallback_shortcut: PatchValue<String>,
    pub provider_id: Option<String>,
    pub provider_key_statuses: Option<BTreeMap<String, String>>,
    #[serde(skip)]
    pub provider_key_saved: Option<BTreeMap<String, bool>>,
    pub phrasebook_terms: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "PatchValue::is_missing")]
    pub language: PatchValue<String>,
    #[serde(default, skip_serializing_if = "PatchValue::is_missing")]
    pub custom_prompt: PatchValue<String>,
    pub paste_after_transcription: Option<bool>,
    pub restore_clipboard: Option<bool>,
    pub debug_logging_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "PatchValue::is_missing")]
    pub history_retention_days: PatchValue<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PatchValue<T> {
    #[default]
    Missing,
    Value(Option<T>),
}

impl<T> PatchValue<T> {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
    pub fn into_change(self) -> Option<Option<T>> {
        match self {
            Self::Missing => None,
            Self::Value(value) => Some(value),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for PatchValue<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer).map(Self::Value)
    }
}

impl<T: Serialize> Serialize for PatchValue<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Missing => serializer.serialize_none(),
            Self::Value(value) => value.serialize(serializer),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsEnvelope {
    pub revision: u64,
    pub settings: PersistedSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettingsField {
    PushToTalkShortcut,
    FallbackShortcut,
    ProviderId,
    ProviderKeyStatuses,
    ProviderKeySaved,
    PhrasebookTerms,
    Language,
    CustomPrompt,
    PasteAfterTranscription,
    RestoreClipboard,
    DebugLoggingEnabled,
    HistoryRetentionDays,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdate {
    pub cleanup_status: CleanupStatus,
    pub previous: SettingsEnvelope,
    pub current: SettingsEnvelope,
    pub changed_fields: Vec<SettingsField>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptTiming {
    pub recording_ms: f64,
    pub blob_ready_ms: f64,
    pub upload_ms: f64,
    pub stop_to_transcript_ms: f64,
    pub history_write_ms: Option<f64>,
    pub provider_ms: Option<f64>,
    pub backend_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptAudioMetadata {
    pub mime_type: String,
    pub byte_size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptRecord {
    pub id: String,
    pub text: String,
    pub created_at: String,
    pub duration_ms: u64,
    pub provider_id: String,
    pub provider_model: Option<String>,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub timings: TranscriptTiming,
    pub audio: TranscriptAudioMetadata,
    pub insertion_mode: String,
    pub source: String,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredTranscript {
    pub revision: u64,
    pub record: TranscriptRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryCursor {
    pub created_at: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub cleanup_status: CleanupStatus,
    #[serde(skip)]
    pub purged: bool,
    pub revision: u64,
    pub records: Vec<StoredTranscript>,
    pub next_cursor: Option<HistoryCursor>,
    pub total_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMutation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_status: Option<CleanupStatus>,
    pub history_revision: u64,
    pub transcript: Option<StoredTranscript>,
    pub record_id: Option<String>,
    pub affected_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPreview {
    pub history_revision: u64,
    pub retention_days: Option<u32>,
    pub delete_count: u64,
    pub retain_count: u64,
}
