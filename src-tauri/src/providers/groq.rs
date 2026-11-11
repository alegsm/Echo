//! Groq Whisper Large V3 Turbo transcription.

use super::{
    multipart_file, non_empty, ProviderError, ProviderErrorCode, ProviderId, TranscriptionRequest,
};

const MIN_AVG_LOGPROB: f64 = -0.75;
const MAX_NO_SPEECH_PROB: f64 = 0.60;
// Whisper skips non-speech only when both scores cross these strict limits.
const SKIP_NO_SPEECH_PROB: f64 = 0.6;
const SKIP_AVG_LOGPROB: f64 = -1.0;

impl super::SegmentConfidence {
    pub(crate) fn confident(self) -> bool {
        self.avg_logprob.is_finite()
            && self.no_speech_prob.is_finite()
            && self.avg_logprob > MIN_AVG_LOGPROB
            && self.no_speech_prob < MAX_NO_SPEECH_PROB
    }
}

pub(super) const TRANSCRIPTIONS_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";

pub(super) fn build_request(
    client: &reqwest::Client,
    request: &TranscriptionRequest,
    api_key: &str,
    url: &str,
) -> Result<reqwest::RequestBuilder, ProviderError> {
    let mut form = reqwest::multipart::Form::new()
        .part("file", multipart_file(request)?)
        .text("model", ProviderId::GroqWhisperTurbo.model())
        .text("temperature", "0")
        .text("response_format", "verbose_json");

    // Without a `language` part Groq detects the spoken language.
    if let Some(language) = non_empty(request.language.as_deref()) {
        form = form.text("language", language.to_owned());
    }
    if let Some(prompt) = non_empty(request.prompt.as_deref()) {
        form = form.text("prompt", prompt.to_owned());
    }

    Ok(client.post(url).bearer_auth(api_key).multipart(form))
}

/// Why the trailing scan kept or dropped one segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum TailReason {
    /// Dropped: Whisper marks it as non-speech.
    SkipRule,
    /// Dropped: its audio stays far below speech level.
    Quiet,
    /// Kept: the first segment is never judged by level.
    FirstSegment,
    /// Kept: a timestamp is missing or invalid, or the WAV is not the upload format.
    NoTimestamps,
    /// Kept: the whole recording is below the signal floor.
    BelowFloor,
    /// Kept: less of the window lies inside the audio than can be judged.
    Short,
    /// Kept: its audio stays at speech level for too long to be a phantom.
    Speech,
}

impl TailReason {
    pub(super) const fn dropped(self) -> bool {
        matches!(self, Self::SkipRule | Self::Quiet)
    }
}

/// One trailing-scan decision with the numbers behind it.
pub(super) struct TailDecision {
    pub reason: TailReason,
    /// The segment's `start` and `end` in seconds, when the reply carries them.
    pub window: (Option<f64>, Option<f64>),
    pub level: Option<crate::audio::TailLevel>,
}

/// What the trailing-segment rules decided for one Groq reply.
pub(super) struct TrailingCheck {
    /// Text of the kept segments; `None` when no trailing segment was dropped.
    pub text: Option<String>,
    pub segments: usize,
    /// Trailing segments dropped, by either rule.
    pub dropped: usize,
    /// The last segment dropped (the one nearest the speech), or, when nothing was
    /// dropped, the last segment and why it was kept.
    pub decisive: Option<TailDecision>,
}

/// Drops trailing segments Whisper invented. Scanning from the end, a segment goes when
/// Whisper itself marks it as non-speech, or when its audio is far quieter than the
/// speech in `wav` (see `audio::tail_level`). The first kept segment stops the scan, and
/// the first segment is never dropped for its level. Missing timestamps or PCM keep it.
pub(super) fn trailing_speech(payload: &serde_json::Value, wav: &[u8]) -> Option<TrailingCheck> {
    let segments = payload.get("segments")?.as_array()?;
    let mut dropped = 0;
    let mut last_dropped = None;
    let mut kept = 0;
    let mut stopped_at = None;
    for (index, segment) in segments.iter().enumerate().rev() {
        let decision = judge(index, segment, wav);
        if !decision.reason.dropped() {
            kept = index + 1;
            stopped_at = Some(decision);
            break;
        }
        dropped += 1;
        last_dropped = Some(decision);
    }
    Some(TrailingCheck {
        text: (dropped > 0).then(|| {
            segments[..kept]
                .iter()
                .filter_map(|segment| segment.get("text")?.as_str())
                .collect::<String>()
        }),
        segments: segments.len(),
        dropped,
        decisive: last_dropped.or(stopped_at),
    })
}

fn judge(index: usize, segment: &serde_json::Value, wav: &[u8]) -> TailDecision {
    let number = |key: &str| segment.get(key).and_then(serde_json::Value::as_f64);
    let window = (number("start"), number("end"));
    let (reason, level) = if marked_non_speech(segment) {
        (TailReason::SkipRule, None)
    } else if index == 0 {
        (TailReason::FirstSegment, None)
    } else if let (Some(start), Some(end)) = window {
        let level = crate::audio::tail_level(wav, start, end);
        let reason = match level {
            crate::audio::TailLevel::Unusable => TailReason::NoTimestamps,
            crate::audio::TailLevel::Short { .. } => TailReason::Short,
            crate::audio::TailLevel::BelowFloor { .. } => TailReason::BelowFloor,
            crate::audio::TailLevel::Judged { .. } if level.is_quiet() => TailReason::Quiet,
            crate::audio::TailLevel::Judged { .. } => TailReason::Speech,
        };
        (reason, Some(level))
    } else {
        (TailReason::NoTimestamps, None)
    };
    TailDecision {
        reason,
        window,
        level,
    }
}

fn marked_non_speech(segment: &serde_json::Value) -> bool {
    segment
        .get("no_speech_prob")
        .and_then(serde_json::Value::as_f64)
        .zip(
            segment
                .get("avg_logprob")
                .and_then(serde_json::Value::as_f64),
        )
        .is_some_and(|(no_speech, avg_logprob)| {
            no_speech.is_finite()
                && avg_logprob.is_finite()
                && no_speech > SKIP_NO_SPEECH_PROB
                && avg_logprob < SKIP_AVG_LOGPROB
        })
}

pub(super) fn reject_low_confidence(payload: &serde_json::Value) -> Result<(), ProviderError> {
    let Some(segments) = payload
        .get("segments")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(());
    };
    if segments.is_empty() {
        return Err(silence());
    }

    let no_speech = finite_values(segments, "no_speech_prob");
    let log_probability = finite_values(segments, "avg_logprob");
    let all_silent =
        !no_speech.is_empty() && no_speech.iter().all(|value| *value >= MAX_NO_SPEECH_PROB);
    let average_silent = average(&no_speech).is_some_and(|value| value >= 0.75);
    let all_uncertain = !log_probability.is_empty()
        && log_probability
            .iter()
            .all(|value| *value <= MIN_AVG_LOGPROB);

    if all_silent || average_silent {
        return Err(silence());
    }
    if all_uncertain {
        return Err(low_confidence());
    }

    Ok(())
}

fn finite_values(segments: &[serde_json::Value], field: &str) -> Vec<f64> {
    segments
        .iter()
        .filter_map(|segment| segment.get(field).and_then(serde_json::Value::as_f64))
        .filter(|value| value.is_finite())
        .collect()
}

fn average(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

const fn low_confidence() -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::LowConfidence,
        "Groq marked this audio as low-confidence speech.",
    )
}

const fn silence() -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::Silence,
        "Groq marked this audio as likely silence.",
    )
}

/// Keep exact edge evidence; aggregate scores cannot stand in for a missing edge.
pub(super) fn edge_confidence(payload: &serde_json::Value) -> Option<super::TranscriptionEdges> {
    let segments = payload.get("segments")?.as_array()?;
    fn read(segment: &serde_json::Value) -> Option<super::SegmentConfidence> {
        let avg_logprob = segment.get("avg_logprob")?.as_f64()?;
        let no_speech_prob = segment.get("no_speech_prob")?.as_f64()?;
        (avg_logprob.is_finite() && no_speech_prob.is_finite()).then_some(
            super::SegmentConfidence {
                avg_logprob,
                no_speech_prob,
            },
        )
    }
    Some(super::TranscriptionEdges {
        first: segments.first().and_then(read),
        last: segments.last().and_then(read),
    })
}
