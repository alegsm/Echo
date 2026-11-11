//! Trusted speech-to-text provider boundary.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::{RequestBuilder, Response, StatusCode};

pub mod deepgram;
pub mod elevenlabs;
pub mod groq;
pub mod mistral;
pub mod openai;
pub mod xai;

const PROVIDER_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PROVIDER_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
pub(crate) fn http_client() -> Result<reqwest::Client, reqwest::Error> {
    http_client_builder().build()
}

fn http_client_builder() -> reqwest::ClientBuilder {
    let settings = http_client_settings();
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .https_only(true)
        .no_proxy()
        .connect_timeout(settings.connect_timeout)
        .timeout(settings.timeout)
}

struct HttpClientSettings {
    connect_timeout: Duration,
    timeout: Duration,
}

fn http_client_settings() -> HttpClientSettings {
    HttpClientSettings {
        connect_timeout: Duration::from_secs(10),
        timeout: PROVIDER_TIMEOUT,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderId {
    GroqWhisperTurbo,
    OpenAiTranscribe,
    XaiStt,
    ElevenLabsScribeV2,
    DeepgramNova3,
    MistralVoxtral3,
}

impl ProviderId {
    pub const ALL: [Self; 6] = [
        Self::GroqWhisperTurbo,
        Self::OpenAiTranscribe,
        Self::XaiStt,
        Self::ElevenLabsScribeV2,
        Self::DeepgramNova3,
        Self::MistralVoxtral3,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GroqWhisperTurbo => "groq-whisper-turbo",
            Self::OpenAiTranscribe => "openai-gpt-4o-mini-transcribe",
            Self::XaiStt => "xai-stt",
            Self::ElevenLabsScribeV2 => "elevenlabs-scribe-v2",
            Self::DeepgramNova3 => "deepgram-nova-3",
            Self::MistralVoxtral3 => "mistral-voxtral-3",
        }
    }

    pub(crate) const fn reports_edge_confidence(self) -> bool {
        matches!(self, Self::GroqWhisperTurbo)
    }

    pub const fn accepts_prompt(self) -> bool {
        matches!(self, Self::GroqWhisperTurbo | Self::OpenAiTranscribe)
    }

    const fn transcript_pointer(self) -> &'static str {
        match self {
            Self::DeepgramNova3 => "/results/channels/0/alternatives/0/transcript",
            _ => "/text",
        }
    }

    const fn origin(self) -> &'static str {
        match self {
            Self::GroqWhisperTurbo => groq::TRANSCRIPTIONS_URL,
            Self::OpenAiTranscribe => openai::ORIGIN,
            Self::XaiStt => xai::ORIGIN,
            Self::ElevenLabsScribeV2 => elevenlabs::ORIGIN,
            Self::DeepgramNova3 => deepgram::ORIGIN,
            Self::MistralVoxtral3 => mistral::ORIGIN,
        }
    }

    pub const fn model(self) -> &'static str {
        match self {
            Self::GroqWhisperTurbo => "whisper-large-v3-turbo",
            Self::OpenAiTranscribe => "gpt-transcribe",
            Self::XaiStt => "grok-voice-transcribe-2.0",
            Self::ElevenLabsScribeV2 => "scribe_v2",
            Self::DeepgramNova3 => "nova-3",
            Self::MistralVoxtral3 => "voxtral-mini-transcribe-3-0",
        }
    }
}

impl TryFrom<&str> for ProviderId {
    type Error = ProviderError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "groq-whisper-turbo" => Ok(Self::GroqWhisperTurbo),
            "openai-gpt-4o-mini-transcribe" => Ok(Self::OpenAiTranscribe),
            "xai-stt" => Ok(Self::XaiStt),
            "elevenlabs-scribe-v2" => Ok(Self::ElevenLabsScribeV2),
            "deepgram-nova-3" => Ok(Self::DeepgramNova3),
            "mistral-voxtral-3" => Ok(Self::MistralVoxtral3),
            _ => Err(ProviderError::unsupported_provider()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TranscriptionRequest {
    pub audio: Arc<[u8]>,
    pub mime_type: String,
    pub file_name: String,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub keyterms: Vec<String>,
    /// Debug log is on: providers may write numeric diagnostics, never transcript text.
    pub diagnostics: bool,
}

pub(super) struct VocabularyRules {
    pub max_terms: usize,
    pub max_chars: usize,
    pub max_words: usize,
    pub forbidden: &'static [char],
}

pub(super) fn vocabulary<'a>(
    terms: &'a [String],
    rules: &'a VocabularyRules,
) -> impl Iterator<Item = &'a str> {
    terms
        .iter()
        .map(String::as_str)
        .filter(|term| {
            term.chars().count() <= rules.max_chars
                && term.split_whitespace().count() <= rules.max_words
                && !term.contains(rules.forbidden)
        })
        .take(rules.max_terms)
}

/// Trims a saved text value and treats blank as absent.
pub(super) fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// The two-letter primary subtag of the saved language ("pt-BR" -> "pt"). `None` for
/// Auto-detect and for tags without one: no adapter may substitute English, because
/// providers detect the spoken language when none is sent.
pub(super) fn primary_language(request: &TranscriptionRequest) -> Option<&str> {
    let language = request.language.as_deref()?;
    let primary = language
        .split_once('-')
        .map_or(language, |(primary, _)| primary);
    (primary.len() == 2 && primary.bytes().all(|ch| ch.is_ascii_alphabetic())).then_some(primary)
}

#[derive(Clone, Debug, PartialEq)]
pub struct TranscriptionResult {
    pub transcript: String,
    pub provider_id: ProviderId,
    pub provider_model: &'static str,
    pub provider_ms: u128,
    pub edges: Option<TranscriptionEdges>,
}

/// Native-only confidence at the two edges of a chunk. Never sent to the renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentConfidence {
    pub avg_logprob: f64,
    pub no_speech_prob: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TranscriptionEdges {
    pub first: Option<SegmentConfidence>,
    pub last: Option<SegmentConfidence>,
}

impl TranscriptionResult {
    pub(crate) fn usable_repair(&self) -> bool {
        !self.transcript.trim().is_empty()
            && (!self.provider_id.reports_edge_confidence()
                || self
                    .edges
                    .is_some_and(|edges| edges.first.is_some() && edges.last.is_some()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderErrorCode {
    UnsupportedProvider,
    MissingCredential,
    CredentialRejected,
    InvalidAudio,
    Timeout,
    RequestFailed,
    InvalidResponse,
    EmptyTranscript,
    LowConfidence,
    Silence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderError {
    pub code: ProviderErrorCode,
    message: &'static str,
}

impl ProviderError {
    pub const fn new(code: ProviderErrorCode, message: &'static str) -> Self {
        Self { code, message }
    }

    pub const fn unsupported_provider() -> Self {
        Self::new(
            ProviderErrorCode::UnsupportedProvider,
            "Unsupported transcription provider.",
        )
    }

    pub const fn missing_credential() -> Self {
        Self::new(
            ProviderErrorCode::MissingCredential,
            "Add and verify an API key for the selected provider before recording.",
        )
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for ProviderError {}

pub async fn transcribe(
    client: &reqwest::Client,
    provider_id: ProviderId,
    request: &TranscriptionRequest,
    api_key: &str,
) -> Result<TranscriptionResult, ProviderError> {
    transcribe_with_verdict(client, provider_id, request, api_key, VerdictMode::Strict).await
}

pub(crate) async fn transcribe_segment(
    client: &reqwest::Client,
    provider_id: ProviderId,
    request: &TranscriptionRequest,
    api_key: &str,
) -> Result<TranscriptionResult, ProviderError> {
    transcribe_with_verdict(
        client,
        provider_id,
        request,
        api_key,
        VerdictMode::CaptureSegment,
    )
    .await
}

#[derive(Clone, Copy)]
pub(super) enum VerdictMode {
    Strict,
    CaptureSegment,
}

async fn transcribe_with_verdict(
    client: &reqwest::Client,
    provider_id: ProviderId,
    request: &TranscriptionRequest,
    api_key: &str,
    verdict: VerdictMode,
) -> Result<TranscriptionResult, ProviderError> {
    if api_key.trim().is_empty() {
        return Err(ProviderError::missing_credential());
    }

    let started_at = Instant::now();
    let provider_request =
        build_request(provider_id, client, request, api_key, provider_id.origin())?;
    execute_with_verdict(provider_id, provider_request, request, started_at, verdict).await
}

fn build_request(
    provider_id: ProviderId,
    client: &reqwest::Client,
    request: &TranscriptionRequest,
    api_key: &str,
    origin: &str,
) -> Result<RequestBuilder, ProviderError> {
    match provider_id {
        ProviderId::GroqWhisperTurbo => groq::build_request(client, request, api_key, origin),
        ProviderId::OpenAiTranscribe => openai::build_request(client, request, api_key, origin),
        ProviderId::XaiStt => xai::build_request(client, request, api_key, origin),
        ProviderId::ElevenLabsScribeV2 => {
            elevenlabs::build_request(client, request, api_key, origin)
        }
        ProviderId::DeepgramNova3 => deepgram::build_request(client, request, api_key, origin),
        ProviderId::MistralVoxtral3 => mistral::build_request(client, request, api_key, origin),
    }
}

/// Builds a credential header value that reqwest redacts from its Debug output,
/// matching what `bearer_auth` does for the Bearer providers. A key that cannot be a
/// header value fails as a request failure, as it does at send time on those providers.
pub(super) fn sensitive_header(value: &str) -> Result<reqwest::header::HeaderValue, ProviderError> {
    let mut header = reqwest::header::HeaderValue::from_str(value).map_err(|_| request_failed())?;
    header.set_sensitive(true);
    Ok(header)
}

pub(super) fn multipart_file(
    request: &TranscriptionRequest,
) -> Result<reqwest::multipart::Part, ProviderError> {
    reqwest::multipart::Part::stream_with_length(
        bytes::Bytes::from_owner(Arc::clone(&request.audio)),
        request.audio.len() as u64,
    )
    .file_name(request.file_name.clone())
    .mime_str(&request.mime_type)
    .map_err(|_| {
        ProviderError::new(
            ProviderErrorCode::InvalidAudio,
            "Recorded audio has an invalid MIME type.",
        )
    })
}

pub(super) async fn execute_with_verdict(
    provider_id: ProviderId,
    provider_request: RequestBuilder,
    input: &TranscriptionRequest,
    started_at: Instant,
    verdict: VerdictMode,
) -> Result<TranscriptionResult, ProviderError> {
    let response = provider_request.send().await.map_err(|error| {
        if error.is_timeout() {
            ProviderError::new(
                ProviderErrorCode::Timeout,
                "The transcription provider timed out.",
            )
        } else {
            request_failed()
        }
    })?;

    read_response(provider_id, response, input, started_at, verdict).await
}

async fn read_response(
    provider_id: ProviderId,
    mut response: Response,
    input: &TranscriptionRequest,
    started_at: Instant,
    verdict: VerdictMode,
) -> Result<TranscriptionResult, ProviderError> {
    if !response.status().is_success() {
        return Err(status_error(response.status()));
    }

    let body = read_response_body(&mut response).await?;
    let payload =
        serde_json::from_slice::<serde_json::Value>(&body).map_err(|_| invalid_response())?;
    let transcript = parse_transcript(provider_id, &payload, input, verdict)?;

    Ok(TranscriptionResult {
        transcript,
        provider_id,
        provider_model: provider_id.model(),
        provider_ms: started_at.elapsed().as_millis(),
        edges: if provider_id == ProviderId::GroqWhisperTurbo {
            groq::edge_confidence(&payload)
        } else {
            None
        },
    })
}

async fn read_response_body(response: &mut Response) -> Result<Vec<u8>, ProviderError> {
    let declared_size = response.content_length().unwrap_or_default();
    if declared_size > MAX_PROVIDER_RESPONSE_BYTES as u64 {
        return Err(response_too_large());
    }

    let mut body = Vec::with_capacity(declared_size as usize);
    while let Some(chunk) = response.chunk().await.map_err(response_read_error)? {
        if body.len().saturating_add(chunk.len()) > MAX_PROVIDER_RESPONSE_BYTES {
            return Err(response_too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn response_read_error(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::new(
            ProviderErrorCode::Timeout,
            "The transcription provider timed out.",
        )
    } else {
        invalid_response()
    }
}

const fn response_too_large() -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::InvalidResponse,
        "Transcription provider returned an oversized response.",
    )
}

fn parse_transcript(
    provider_id: ProviderId,
    payload: &serde_json::Value,
    input: &TranscriptionRequest,
    verdict: VerdictMode,
) -> Result<String, ProviderError> {
    if provider_id == ProviderId::GroqWhisperTurbo {
        if let Err(error) = groq::reject_low_confidence(payload) {
            match (verdict, error.code) {
                (VerdictMode::CaptureSegment, ProviderErrorCode::LowConfidence) => {}
                (VerdictMode::Strict, ProviderErrorCode::Silence) => {
                    // Probes and whole-artifact callers retain their existing verdict.
                    return Err(ProviderError {
                        code: ProviderErrorCode::LowConfidence,
                        ..error
                    });
                }
                _ => return Err(error),
            }
        }
    }

    let trailing = (provider_id == ProviderId::GroqWhisperTurbo)
        .then(|| groq::trailing_speech(payload, &input.audio))
        .flatten();
    if let (Some(trailing), true) = (&trailing, input.diagnostics) {
        log_quiet_tail_check(trailing);
    }
    trailing
        .as_ref()
        .and_then(|trailing| trailing.text.as_deref())
        .or_else(|| {
            payload
                .pointer(provider_id.transcript_pointer())
                .and_then(serde_json::Value::as_str)
        })
        .map(str::trim)
        .filter(|transcript| !transcript.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCode::EmptyTranscript,
                "Transcription provider returned empty text.",
            )
        })
}

/// One record per reply of two or more segments, to tune the level rule from. It
/// describes the segment that settled the decision and carries numbers only, never text;
/// what could not be measured prints as "-".
fn log_quiet_tail_check(trailing: &groq::TrailingCheck) {
    use crate::audio::TailLevel;
    let (Some(decisive), true) = (&trailing.decisive, trailing.segments >= 2) else {
        return;
    };
    let seconds = |value: Option<f64>| value.map_or("-".into(), |value| format!("{value:.2}"));
    let (inside_ms, loud_ms, speech_rms, tail_peak_rms, ratio) = match decisive.level {
        Some(TailLevel::Short { inside_ms }) | Some(TailLevel::BelowFloor { inside_ms }) => {
            (Some(inside_ms), None, None, None, None)
        }
        Some(TailLevel::Judged {
            inside_ms,
            loud_ms,
            speech_rms,
            tail_peak_rms,
        }) => (
            Some(inside_ms),
            Some(loud_ms),
            Some(speech_rms),
            Some(tail_peak_rms),
            Some(tail_peak_rms / speech_rms),
        ),
        Some(TailLevel::Unusable) | None => (None, None, None, None, None),
    };
    let count = |value: Option<usize>| value.map_or("-".into(), |value| value.to_string());
    let level = |value: Option<f64>, places: usize| {
        value.map_or("-".into(), |value| format!("{value:.places$}"))
    };
    log::info!(
        "quiet_tail_check provider={} decision={} reason={} segments={} dropped={} start_s={} end_s={} inside_ms={} loud_ms={} speech_rms={} tail_peak_rms={} ratio={}",
        ProviderId::GroqWhisperTurbo.as_str(),
        if decisive.reason.dropped() { "dropped" } else { "kept" },
        reason_name(decisive.reason),
        trailing.segments,
        trailing.dropped,
        seconds(decisive.window.0),
        seconds(decisive.window.1),
        count(inside_ms),
        count(loud_ms),
        level(speech_rms, 4),
        level(tail_peak_rms, 4),
        level(ratio, 3),
    );
}

const fn reason_name(reason: groq::TailReason) -> &'static str {
    match reason {
        groq::TailReason::SkipRule => "skip_rule",
        groq::TailReason::Quiet => "quiet",
        groq::TailReason::FirstSegment => "first_segment",
        groq::TailReason::NoTimestamps => "no_timestamps",
        groq::TailReason::BelowFloor => "below_floor",
        groq::TailReason::Short => "short",
        groq::TailReason::Speech => "speech",
    }
}

fn status_error(status: StatusCode) -> ProviderError {
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return ProviderError::new(
            ProviderErrorCode::CredentialRejected,
            "The transcription provider rejected the API key.",
        );
    }

    ProviderError::new(
        ProviderErrorCode::RequestFailed,
        "The transcription provider rejected the request.",
    )
}

const fn invalid_response() -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::InvalidResponse,
        "Transcription provider returned an invalid response.",
    )
}

const fn request_failed() -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::RequestFailed,
        "The transcription provider failed.",
    )
}

#[cfg(test)]
mod tests;
