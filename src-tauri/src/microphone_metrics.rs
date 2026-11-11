use std::collections::VecDeque;

pub const RMS_SILENCE_THRESHOLD: f64 = 0.0001;
pub const PEAK_SILENCE_THRESHOLD: f64 = 0.001;
pub const ZERO_RATIO_THRESHOLD: f64 = 0.999;
const NEAR_ZERO_AMPLITUDE: f32 = 0.00001;

#[derive(Clone, Copy, Default)]
pub struct BlockMetrics {
    pub at_ms: u64,
    pub samples: u64,
    pub finite: u64,
    pub non_finite: u64,
    pub sum_squares: f64,
    pub peak: f64,
    pub near_zero: u64,
}

#[derive(Clone, Copy, Default)]
pub struct WindowMetrics {
    pub samples: u64,
    pub finite: u64,
    pub non_finite: u64,
    pub sum_squares: f64,
    pub peak: f64,
    pub near_zero: u64,
}

impl WindowMetrics {
    pub fn add(&mut self, block: BlockMetrics) {
        self.samples += block.samples;
        self.finite += block.finite;
        self.non_finite += block.non_finite;
        self.sum_squares += block.sum_squares;
        self.peak = self.peak.max(block.peak);
        self.near_zero += block.near_zero;
    }

    pub fn summary(&self) -> (f64, f64, bool) {
        if self.finite == 0 {
            return (0.0, 0.0, false);
        }
        let rms = (self.sum_squares / self.finite as f64).sqrt();
        let ratio = self.near_zero as f64 / self.finite as f64;
        let silent = rms < RMS_SILENCE_THRESHOLD
            && self.peak < PEAK_SILENCE_THRESHOLD
            && ratio >= ZERO_RATIO_THRESHOLD;
        (rms, ratio, silent)
    }
}

pub fn summarize_samples(samples: &[f32]) -> BlockMetrics {
    let mut block = BlockMetrics {
        samples: samples.len() as u64,
        ..Default::default()
    };
    for sample in samples {
        if !sample.is_finite() {
            block.non_finite += 1;
            continue;
        }
        let amplitude = sample.abs();
        block.finite += 1;
        block.sum_squares += f64::from(*sample) * f64::from(*sample);
        block.peak = block.peak.max(f64::from(amplitude));
        if amplitude <= NEAR_ZERO_AMPLITUDE {
            block.near_zero += 1;
        }
    }
    block
}

pub fn stall_threshold_ms(gaps: &VecDeque<u64>) -> Option<u64> {
    if gaps.len() < 5 {
        return None;
    }
    let mut sorted: Vec<_> = gaps.iter().copied().collect();
    sorted.sort_unstable();
    Some(250.max(sorted[sorted.len() / 2].saturating_mul(4)))
}
