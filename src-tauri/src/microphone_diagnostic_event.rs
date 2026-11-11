use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

static RUN_ID: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().to_string());
static STARTED_AT: LazyLock<Instant> = LazyLock::new(Instant::now);
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Default)]
pub struct DiagnosticControl {
    inner: Arc<DiagnosticControlInner>,
}

#[derive(Clone, Debug)]
pub struct DiagnosticToken {
    inner: Arc<DiagnosticControlInner>,
    expected: u64,
}

#[derive(Debug, Default)]
struct DiagnosticControlInner {
    update: Mutex<DiagnosticSetting>,
    state: AtomicU64,
}

#[derive(Debug, Default)]
struct DiagnosticSetting {
    revision: Option<u64>,
    generation: u64,
    is_enabled: bool,
}

impl DiagnosticControl {
    pub fn apply(&self, revision: u64, enabled: bool) -> bool {
        let Ok(mut setting) = self.inner.update.lock() else {
            return false;
        };
        if setting.revision.is_some_and(|current| current >= revision) {
            return false;
        }
        setting.revision = Some(revision);
        if setting.is_enabled == enabled {
            return false;
        }
        setting.is_enabled = enabled;
        setting.generation = setting.generation.saturating_add(1);
        let state = setting.generation.saturating_mul(2) | u64::from(enabled);
        self.inner.state.store(state, Ordering::Release);
        true
    }

    pub fn token(&self) -> Option<DiagnosticToken> {
        let expected = self.inner.state.load(Ordering::Acquire);
        (expected & 1 == 1).then(|| DiagnosticToken {
            inner: Arc::clone(&self.inner),
            expected,
        })
    }
}

impl DiagnosticToken {
    pub fn is_active(&self) -> bool {
        self.inner.state.load(Ordering::Acquire) == self.expected
    }
}

#[derive(Clone)]
pub struct CaptureDiagnosticIdentity {
    pub capture_path: &'static str,
    pub purpose: &'static str,
    pub intent_id: String,
    token: Option<DiagnosticToken>,
}

impl CaptureDiagnosticIdentity {
    pub fn new(capture_path: &'static str, purpose: &'static str, intent_id: &str) -> Self {
        Self {
            capture_path,
            purpose,
            intent_id: normalize_session_id(intent_id).to_owned(),
            token: None,
        }
    }

    pub fn with_token(mut self, token: Option<DiagnosticToken>) -> Self {
        self.token = token;
        self
    }

    pub fn emit(
        &self,
        session_id: &str,
        event: &'static str,
        level: log::Level,
        fields: MicrophoneFields,
    ) {
        if self.token.as_ref().is_some_and(|token| !token.is_active()) {
            return;
        }
        let record = serde_json::json!({
            "timeUnixMs": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
            "monotonicMs": STARTED_AT.elapsed().as_millis(),
            "sequence": SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1,
            "runId": RUN_ID.as_str(),
            "surface": "native",
            "version": env!("CARGO_PKG_VERSION"),
            "gitCommit": option_env!("ECHO_GIT_COMMIT").unwrap_or("unavailable"),
            "area": "microphone",
            "event": event,
            "sessionId": normalize_session_id(session_id),
            "intentId": self.intent_id,
            "capturePath": self.capture_path,
            "purpose": self.purpose,
            "fields": fields,
        });
        write_record(level, record);
    }
}

pub struct PermissionDiagnostics {
    is_enabled: bool,
    session_id: String,
    started_at: Instant,
    identity: CaptureDiagnosticIdentity,
    token: Option<DiagnosticToken>,
}

impl PermissionDiagnostics {
    pub fn start_with_token(
        session_id: Option<&str>,
        identity: CaptureDiagnosticIdentity,
        token: Option<DiagnosticToken>,
    ) -> Self {
        let diagnostics = Self {
            is_enabled: token.is_some(),
            session_id: normalize_session_id(session_id.unwrap_or("permission")).to_string(),
            started_at: Instant::now(),
            identity: identity.with_token(token.clone()),
            token,
        };
        diagnostics.emit(
            "native_permission_check_started",
            log::Level::Info,
            Default::default(),
        );
        diagnostics
    }

    pub fn checked(&self, status: &str) {
        self.emit(
            "native_permission_check_result",
            log::Level::Info,
            MicrophoneFields {
                outcome: Some(permission_status_name(status)),
                elapsed_ms: Some(self.elapsed_ms()),
                ..Default::default()
            },
        );
        if status == "not_determined" {
            self.emit(
                "native_permission_request_started",
                log::Level::Info,
                Default::default(),
            );
        }
    }

    pub fn request_succeeded(&self, initial_status: &str, status: &str) {
        if initial_status != "not_determined" {
            return;
        }
        self.emit(
            "native_permission_request_result",
            log::Level::Info,
            MicrophoneFields {
                outcome: Some(permission_status_name(status)),
                elapsed_ms: Some(self.elapsed_ms()),
                ..Default::default()
            },
        );
    }

    pub fn request_failed(&self) {
        self.emit(
            "native_permission_request_result",
            log::Level::Error,
            MicrophoneFields {
                outcome: Some("failed"),
                error_name: Some("permission_request_failed"),
                elapsed_ms: Some(self.elapsed_ms()),
                ..Default::default()
            },
        );
    }

    fn emit(&self, event: &'static str, level: log::Level, fields: MicrophoneFields) {
        if self.is_enabled && self.token.as_ref().is_none_or(DiagnosticToken::is_active) {
            self.identity.emit(&self.session_id, event, level, fields);
        }
    }

    fn elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrophoneFields {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_name: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_format: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_count: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer_size_kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer_frames: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_default_audio_input: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_input_connected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_input_in_use_by_another_application: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_input_suspended: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_gap_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub longest_callback_gap_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_callback_age_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finite_sample_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub non_finite_sample_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub near_zero_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rms_threshold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_threshold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub near_zero_ratio_threshold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropout_duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telemetry_dropped_count: Option<u64>,
}

fn write_record(level: log::Level, record: serde_json::Value) {
    let message = format!("[echo-diagnostic] {record}");
    match level {
        log::Level::Error => log::error!("{message}"),
        log::Level::Warn => log::warn!("{message}"),
        _ => log::info!("{message}"),
    }
}

fn normalize_session_id(session_id: &str) -> &str {
    let safe = !session_id.is_empty()
        && session_id.len() <= 64
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if safe {
        session_id
    } else {
        "invalid"
    }
}

fn permission_status_name(status: &str) -> &'static str {
    match status {
        "authorized" => "authorized",
        "denied" => "denied",
        "restricted" => "restricted",
        "not_determined" => "not_determined",
        "unsupported" => "unsupported",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_session_id, DiagnosticControl};

    #[test]
    fn session_identifiers_are_bounded_and_normalized() {
        assert_eq!(normalize_session_id("session_123-abc"), "session_123-abc");
        assert_eq!(normalize_session_id("Sample's microphone"), "invalid");
        assert_eq!(normalize_session_id(&"a".repeat(65)), "invalid");
    }

    #[test]
    fn committed_revision_invalidates_tokens_and_rejects_stale_toggles() {
        let control = DiagnosticControl::default();
        assert!(control.apply(4, true));
        let token = control.token().unwrap();
        assert!(token.is_active());

        assert!(control.apply(5, false));
        assert!(!token.is_active());
        assert!(!control.apply(4, true));
        assert!(!control.apply(5, true));
        assert!(control.token().is_none());
    }

    #[test]
    fn enabled_setting_survives_an_unrelated_settings_revision() {
        let control = DiagnosticControl::default();
        assert!(control.apply(4, true));
        let token = control.token().unwrap();

        assert!(!control.apply(5, true));
        assert!(token.is_active());
        assert!(control.apply(6, false));
        assert!(!token.is_active());
    }
}
