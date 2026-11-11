use std::f64::consts::{PI, TAU};

pub(super) const SAMPLE_RATE: u32 = 16_000;
pub(super) const CHANNEL_COUNT: u16 = 1;

/// Input is interleaved PCM with a positive device rate and channel count.
/// A trailing incomplete frame is omitted.
pub(super) fn normalize_mono(samples: &[f32], sample_rate: u32, channels: u16) -> Vec<f32> {
    let mono: Vec<f32> = samples
        .chunks_exact(usize::from(channels))
        .map(|frame| {
            let sum: f64 = frame
                .iter()
                .map(|sample| {
                    if sample.is_finite() {
                        f64::from(sample.clamp(-1.0, 1.0))
                    } else {
                        0.0
                    }
                })
                .sum();
            (sum / f64::from(channels)) as f32
        })
        .collect();
    if sample_rate == SAMPLE_RATE || mono.is_empty() {
        return mono;
    }

    let ratio = (f64::from(SAMPLE_RATE) / f64::from(sample_rate)).min(1.0);
    // Leave a transition band below 8 kHz. Scale the window with the input rate
    // so downsampling retains the same stopband rejection on every device.
    let cutoff = 0.94 * ratio;
    let radius = (32.0 / ratio).ceil() as isize;
    let mut divisor = sample_rate;
    let mut remainder = SAMPLE_RATE;
    while remainder != 0 {
        (divisor, remainder) = (remainder, divisor % remainder);
    }
    let phase_count = SAMPLE_RATE / divisor;
    // Rational device rates reuse a finite set of fractional positions. Build
    // the sinc weights once per phase, rather than evaluating trig per sample.
    let kernels: Vec<Vec<f64>> = (0..phase_count)
        .map(|phase| sinc_kernel(f64::from(phase) / f64::from(phase_count), cutoff, radius))
        .collect();
    let frame_count = (mono.len() as u64 * u64::from(SAMPLE_RATE) + u64::from(sample_rate) / 2)
        / u64::from(sample_rate);
    (0..frame_count)
        .map(|frame| {
            let position = frame * u64::from(sample_rate);
            let center = (position / u64::from(SAMPLE_RATE)) as isize;
            let phase = (position % u64::from(SAMPLE_RATE)) as usize / divisor as usize;
            kernels[phase]
                .iter()
                .enumerate()
                .map(|(tap, weight)| {
                    let source = (center + tap as isize - radius).clamp(0, mono.len() as isize - 1);
                    f64::from(mono[source as usize]) * weight
                })
                .sum::<f64>()
                .clamp(-1.0, 1.0) as f32
        })
        .collect()
}

fn sinc_kernel(fraction: f64, cutoff: f64, radius: isize) -> Vec<f64> {
    let mut weights: Vec<f64> = (-radius..=radius)
        .map(|tap| {
            let distance = tap as f64 - fraction;
            let window_position = distance / radius as f64;
            if window_position.abs() > 1.0 {
                return 0.0;
            }
            let argument = PI * cutoff * distance;
            let sinc = if argument.abs() < 1e-12 {
                1.0
            } else {
                argument.sin() / argument
            };
            let window =
                0.42 + 0.5 * (PI * window_position).cos() + 0.08 * (TAU * window_position).cos();
            cutoff * sinc * window
        })
        .collect();
    let sum: f64 = weights.iter().sum();
    for weight in &mut weights {
        *weight /= sum;
    }
    weights
}
