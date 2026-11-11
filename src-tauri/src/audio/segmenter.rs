use std::collections::VecDeque;
use std::sync::Arc;

use crate::microphone_metrics::summarize_samples;

use super::types::{
    AudioCapturePolicy, AudioSegment, CaptureError, CaptureErrorCode, SegmentCutKind,
    MINIMUM_SEGMENT_SECONDS,
};
use super::wav::encode_wav;

const HALF_SECOND: usize = 0;
const QUARTER_SECOND: usize = 1;
const BOUNDARY_FRAME: usize = 2;
const QUIET_RATIO: f64 = 0.25;
const BOUNDARY_BLOCK_MS: usize = 100;
const SPEECH_BLOCK_MS: usize = 10;
const SPEECH_WINDOW_MS: usize = 200;
const MIN_SPEECH_MS: usize = 120;
/// Preserve word endings after the latest qualifying non-quiet sub-block on release.
const TRAILING_PAD_MS: usize = 400;

/// Device frames determine boundaries, regardless of callback size or Diagnostics.
#[cfg_attr(test, derive(Clone))]
pub(crate) struct Segmenter {
    sample_rate: u32,
    channels: usize,
    samples: Vec<f32>,
    windows: [QuietWindow; 3],
    recent: QuietWindow,
    last_speech_end: Option<usize>,
    speech_block: QuietWindow,
    speech_activity: VecDeque<bool>,
    boundary_frames: VecDeque<(usize, f64)>,
    segments: Vec<AudioSegment>,
    encoded_bytes: usize,
    maximum_bytes: usize,
    minimum_rms: f64,
    minimum_peak: f32,
}

impl Segmenter {
    pub(crate) fn new(sample_rate: u32, channels: u16, policy: &AudioCapturePolicy) -> Self {
        Self {
            sample_rate,
            channels: usize::from(channels),
            samples: Vec::new(),
            windows: [500, 250, BOUNDARY_BLOCK_MS]
                .map(|ms| QuietWindow::new((sample_rate as usize * ms).div_ceil(1_000))),
            recent: QuietWindow::new(sample_rate as usize * 3),
            last_speech_end: None,
            speech_block: QuietWindow::new(
                (sample_rate as usize * SPEECH_BLOCK_MS).div_ceil(1_000),
            ),
            speech_activity: VecDeque::new(),
            boundary_frames: VecDeque::new(),
            segments: Vec::new(),
            encoded_bytes: 0,
            maximum_bytes: policy.maximum_artifact_bytes,
            minimum_rms: policy.minimum_signal_rms,
            minimum_peak: policy.minimum_signal_peak,
        }
    }

    pub(crate) fn push(&mut self, samples: &[f32]) -> Result<(), CaptureError> {
        for sample in samples {
            self.samples.push(*sample);
            if !self.samples.len().is_multiple_of(self.channels) {
                continue;
            }
            let frame = &self.samples[self.samples.len() - self.channels..];
            let energy = frame.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
            let peak = frame.iter().map(|s| s.abs()).fold(0.0, f32::max);
            self.recent.push(energy, peak);
            self.observe_frame(energy, peak);
            let frames = self.samples.len() / self.channels;
            let rate = self.sample_rate as usize;
            let (quiet_energy, quiet_peak) = self.quiet_limits();
            let quiet =
                |index: usize| self.windows[index].quiet(self.channels, quiet_energy, quiet_peak);
            let boundary = if (frames >= rate * MINIMUM_SEGMENT_SECONDS && quiet(HALF_SECOND))
                || (frames >= rate * 6 && quiet(QUARTER_SECOND))
            {
                Some((frames, SegmentCutKind::QuietWindow))
            } else if frames >= rate * 12 {
                // Continuous speech still cuts at the quietest available frame.
                self.boundary_frames
                    .iter()
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(end, _)| (*end, SegmentCutKind::Valley))
            } else {
                None
            };
            if let Some((boundary, kind)) = boundary {
                self.close_at(boundary * self.channels, kind)?;
            }
        }
        Ok(())
    }

    fn quiet_limits(&self) -> (f64, f64) {
        (
            (self.minimum_rms * self.minimum_rms)
                .max(self.recent.mean_energy(self.channels) * QUIET_RATIO.powi(2)),
            f64::from(self.minimum_peak).max(self.recent.peak() * QUIET_RATIO),
        )
    }

    fn observe_frame(&mut self, energy: f64, peak: f32) {
        let frames = self.samples.len() / self.channels;
        for window in &mut self.windows {
            window.push(energy, peak);
        }
        let short = &self.windows[BOUNDARY_FRAME];
        if frames.is_multiple_of(short.frames) {
            self.boundary_frames
                .push_back((frames, short.energy.max(0.0)));
        }
        self.speech_block.push(energy, peak);
        if frames.is_multiple_of(self.speech_block.frames) {
            let (quiet_energy, quiet_peak) = self.quiet_limits();
            let non_quiet = !self
                .speech_block
                .quiet(self.channels, quiet_energy, quiet_peak);
            self.speech_activity.push_back(non_quiet);
            if self.speech_activity.len() > SPEECH_WINDOW_MS / SPEECH_BLOCK_MS {
                self.speech_activity.pop_front();
            }
            if non_quiet
                && self
                    .speech_activity
                    .iter()
                    .filter(|active| **active)
                    .count()
                    >= MIN_SPEECH_MS / SPEECH_BLOCK_MS
            {
                self.last_speech_end = Some(frames);
            }
        }
        let oldest_start = frames.saturating_sub(self.sample_rate as usize);
        while self
            .boundary_frames
            .front()
            .is_some_and(|(end, _)| *end < oldest_start + short.frames)
        {
            self.boundary_frames.pop_front();
        }
    }

    pub(crate) fn finish(&mut self) -> Result<(), CaptureError> {
        // A callback may end in a partial device frame. Encoding omits that frame.
        self.samples
            .truncate(self.samples.len() / self.channels * self.channels);
        if !self.samples.is_empty() {
            self.close_at(self.samples.len(), SegmentCutKind::ReleaseTail)?;
        }
        Ok(())
    }

    pub(crate) fn segments(&self) -> &[AudioSegment] {
        &self.segments
    }

    fn close_at(&mut self, boundary: usize, cut_kind: SegmentCutKind) -> Result<(), CaptureError> {
        let remainder = self.samples.split_off(boundary);
        let duration_ms =
            (self.samples.len() / self.channels) as u64 * 1_000 / u64::from(self.sample_rate);
        if cut_kind == SegmentCutKind::ReleaseTail {
            if let Some(end) = self.last_speech_end {
                let pad = (self.sample_rate as usize * TRAILING_PAD_MS).div_ceil(1_000);
                self.samples
                    .truncate(self.samples.len().min((end + pad) * self.channels));
            }
        }
        let signal = summarize_samples(&self.samples);
        let rms = (signal.sum_squares / signal.finite as f64).sqrt();
        let bytes = if self.last_speech_end.is_some()
            && (rms >= self.minimum_rms || signal.peak >= f64::from(self.minimum_peak))
        {
            let bytes = encode_wav(&self.samples, self.sample_rate, self.channels as u16);
            self.encoded_bytes = self.encoded_bytes.saturating_add(bytes.len());
            if self.encoded_bytes > self.maximum_bytes {
                return Err(CaptureError::new(
                    CaptureErrorCode::ArtifactSizeLimit,
                    "segment_encode",
                ));
            }
            Some(Arc::from(bytes))
        } else {
            None
        };
        self.segments.push(AudioSegment {
            cut_kind,
            ordinal: self.segments.len() as u32,
            duration_ms,
            bytes,
        });
        self.samples.clear();
        for window in &mut self.windows {
            window.reset();
        }
        self.boundary_frames.clear();
        self.last_speech_end = None;
        self.speech_block.reset();
        self.speech_activity.clear();
        // Rebuild piece windows without replaying the capture-wide recent reference.
        for frame in remainder.chunks_exact(self.channels) {
            self.samples.extend_from_slice(frame);
            let energy = frame.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
            let peak = frame.iter().map(|s| s.abs()).fold(0.0, f32::max);
            self.observe_frame(energy, peak);
        }
        Ok(())
    }
}

#[cfg_attr(test, derive(Clone))]
struct QuietWindow {
    frames: usize,
    energies: VecDeque<f64>,
    energy: f64,
    peaks: VecDeque<(usize, f32)>,
    observed: usize,
}

impl QuietWindow {
    fn new(frames: usize) -> Self {
        Self {
            frames,
            energies: VecDeque::new(),
            energy: 0.0,
            peaks: VecDeque::new(),
            observed: 0,
        }
    }

    fn push(&mut self, energy: f64, peak: f32) {
        self.observed += 1;
        self.energies.push_back(energy);
        self.energy += energy;
        if self.energies.len() > self.frames {
            self.energy -= self.energies.pop_front().unwrap();
        }
        // A decreasing deque keeps the exact rolling peak without rescanning PCM.
        while self.peaks.back().is_some_and(|(_, value)| *value <= peak) {
            self.peaks.pop_back();
        }
        self.peaks.push_back((self.observed, peak));
        let oldest = self.observed.saturating_sub(self.frames);
        while self
            .peaks
            .front()
            .is_some_and(|(index, _)| *index <= oldest)
        {
            self.peaks.pop_front();
        }
    }

    fn mean_energy(&self, channels: usize) -> f64 {
        self.energy.max(0.0) / (self.energies.len() * channels) as f64
    }

    fn peak(&self) -> f64 {
        self.peaks.front().map_or(0.0, |(_, peak)| f64::from(*peak))
    }

    fn quiet(&self, channels: usize, maximum_energy: f64, maximum_peak: f64) -> bool {
        self.energies.len() == self.frames
            && self.mean_energy(channels) <= maximum_energy
            && self.peak() <= maximum_peak
    }

    fn reset(&mut self) {
        self.energies.clear();
        self.energy = 0.0;
        self.peaks.clear();
        self.observed = 0;
    }
}
