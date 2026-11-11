//! Level of a transcript segment's audio relative to the speech in the same recording.
//!
//! Whisper sometimes invents a closing phrase ("Thank you.") from the quiet end of a
//! recording. Such a segment stays far below the speech level for almost all of its
//! window, whatever the words or language, so the rule measures how long the window is
//! at speech level and never reads text. The comparison is relative to the same
//! recording, so a whispered recording is not touched.

use super::resample::SAMPLE_RATE;
use super::types::MINIMUM_SIGNAL_RMS;
use super::wav::pcm16_mono_data;

/// Short enough that one frame holds less than a syllable, long enough for a steady RMS.
const FRAME_MS: usize = 20;
const FRAME_SAMPLES: usize = SAMPLE_RATE as usize / 1000 * FRAME_MS;
/// Speech level: the RMS that a quarter of the voiced frames reach, so pauses and room
/// tone between words do not pull it down.
const SPEECH_PERCENTILE: f64 = 0.75;
/// A frame is at speech level when its RMS is at least this fraction of the speech level
/// (-20 dB). The segmenter already calls audio quiet at a ratio of 0.25 (-12 dB), so a
/// soft but real closing word sits well above this bar.
const TAIL_RATIO: f64 = 0.1;
/// Shorter windows are too brief to judge, so they are never dropped.
const MIN_TAIL_MS: usize = 300;
/// A tail that stays at speech level for less than this is quiet. This is the segmenter's
/// MIN_SPEECH_MS, the shortest run it counts as speech: a decaying word end, a reverb
/// tail or a click is shorter, while a real word is not.
const MAX_TAIL_SPEECH_MS: usize = 120;

/// What `tail_level` could say about one segment's audio window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TailLevel {
    /// The window or the WAV is unusable: timestamps not finite and ordered, or `wav` is
    /// not the upload format.
    Unusable,
    /// Less than `MIN_TAIL_MS` of the window lies inside the audio.
    Short { inside_ms: usize },
    /// The whole recording is below the signal floor, so there is no speech level.
    BelowFloor { inside_ms: usize },
    Judged {
        inside_ms: usize,
        /// Time in the window spent at or above `TAIL_RATIO` x `speech_rms`.
        loud_ms: usize,
        speech_rms: f64,
        /// The loudest frame in the window.
        tail_peak_rms: f64,
    },
}

impl TailLevel {
    /// True when the window was judged and stayed at speech level for too short a time
    /// to be speech.
    pub(crate) fn is_quiet(&self) -> bool {
        matches!(self, Self::Judged { loud_ms, .. } if *loud_ms < MAX_TAIL_SPEECH_MS)
    }
}

/// Judges the audio between `start_s` and `end_s` (seconds from the start of `wav`)
/// against the speech level of the whole of `wav`: the tail is quiet when it stays at
/// speech level for under `MAX_TAIL_SPEECH_MS`. An `end_s` past the end of the audio is
/// clamped to it. Anything but `Judged` means "change nothing".
pub(crate) fn tail_level(wav: &[u8], start_s: f64, end_s: f64) -> TailLevel {
    let Some(pcm) = pcm16_mono_data(wav) else {
        return TailLevel::Unusable;
    };
    if !(start_s.is_finite() && end_s.is_finite() && start_s >= 0.0 && start_s < end_s) {
        return TailLevel::Unusable;
    }
    let rate = f64::from(SAMPLE_RATE);
    let start = (start_s * rate).round() as usize;
    let end = ((end_s * rate).round() as usize).min(pcm.len() / 2);
    let inside_ms = end.saturating_sub(start) * 1000 / SAMPLE_RATE as usize;
    if inside_ms < MIN_TAIL_MS {
        return TailLevel::Short { inside_ms };
    }

    let samples: Vec<f32> = pcm
        .chunks_exact(2)
        .map(|bytes| f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / f32::from(i16::MAX))
        .collect();
    let mut voiced: Vec<f64> = frame_levels(&samples)
        .filter(|rms| *rms >= MINIMUM_SIGNAL_RMS)
        .collect();
    if voiced.is_empty() {
        return TailLevel::BelowFloor { inside_ms };
    }
    let speech_rms = speech_level(&mut voiced);
    let tail: Vec<f64> = frame_levels(&samples[start..end]).collect();
    let loud_frames = tail
        .iter()
        .filter(|rms| **rms >= TAIL_RATIO * speech_rms)
        .count();
    TailLevel::Judged {
        inside_ms,
        loud_ms: loud_frames * FRAME_MS,
        speech_rms,
        tail_peak_rms: tail.iter().copied().fold(0.0, f64::max),
    }
}

fn frame_levels(samples: &[f32]) -> impl Iterator<Item = f64> + '_ {
    samples.chunks_exact(FRAME_SAMPLES).map(|frame| {
        let energy: f64 = frame.iter().map(|sample| f64::from(*sample).powi(2)).sum();
        (energy / FRAME_SAMPLES as f64).sqrt()
    })
}

/// The level that the loudest `1 - SPEECH_PERCENTILE` share of `levels` reaches (at
/// least one frame).
fn speech_level(levels: &mut [f64]) -> f64 {
    levels.sort_unstable_by(|a, b| b.total_cmp(a));
    let loudest = ((1.0 - SPEECH_PERCENTILE) * levels.len() as f64).ceil() as usize;
    levels[loudest.clamp(1, levels.len()) - 1]
}
