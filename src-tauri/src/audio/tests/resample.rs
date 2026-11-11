use std::f64::consts::{PI, TAU};

use crate::audio::encode_wav;

fn pcm(bytes: &[u8]) -> Vec<f64> {
    bytes[44..]
        .chunks_exact(2)
        .map(|sample| f64::from(i16::from_le_bytes(sample.try_into().unwrap())) / 32767.0)
        .collect()
}

fn tone(rate: u32, frequency: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|frame| (0.5 * (TAU * frequency * frame as f64 / f64::from(rate)).sin()) as f32)
        .collect()
}

fn rms(samples: &[f64]) -> f64 {
    (samples.iter().map(|sample| sample * sample).sum::<f64>() / samples.len() as f64).sqrt()
}

#[test]
fn wav_normalization_preserves_duration_and_pcm_contract_across_device_formats() {
    for rate in [8_000, 16_000, 44_100, 48_000, 96_000] {
        for channels in [1, 2] {
            let mono = tone(rate, 1_000.0, rate as usize / 10 + 1);
            let input: Vec<f32> = mono
                .iter()
                .flat_map(|sample| std::iter::repeat_n(*sample, channels as usize))
                .collect();
            let wav = encode_wav(&input, rate, channels);
            let expected_frames = (mono.len() as f64 * 16_000.0 / f64::from(rate)).round() as usize;
            assert_eq!(
                wav.len(),
                44 + 2 * expected_frames,
                "{rate} Hz, {channels} channels"
            );
            assert_eq!(&wav[..4], b"RIFF");
            assert_eq!(&wav[8..16], b"WAVEfmt ");
            assert_eq!(
                u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize,
                wav.len() - 8
            );
            assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1);
            assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
            assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
            assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 32_000);
            assert_eq!(u16::from_le_bytes(wav[32..34].try_into().unwrap()), 2);
            assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
            assert_eq!(&wav[36..40], b"data");
            assert_eq!(
                u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize,
                wav.len() - 44
            );
            let output = pcm(&wav);
            let interior = &output[100..output.len() - 100];
            let error: Vec<f64> = interior
                .iter()
                .enumerate()
                .map(|(index, sample)| {
                    sample - 0.5 * (TAU * 1_000.0 * (index + 100) as f64 / 16_000.0).sin()
                })
                .collect();
            assert!(
                rms(&error) < 0.001,
                "passband changed at {rate} Hz, {channels} channels"
            );
        }
    }
}

#[test]
fn above_nyquist_tones_are_attenuated_before_decimation() {
    for rate in [44_100, 48_000, 96_000] {
        for frequency in [8_500.0, 12_000.0, 19_000.0] {
            let wav = encode_wav(&tone(rate, frequency, rate as usize / 5), rate, 1);
            let output = pcm(&wav);
            let level = rms(&output[200..output.len() - 200]);
            // At least 40 dB below the input RMS of 0.5 / sqrt(2).
            assert!(
                level < 0.0035,
                "alias at {frequency} Hz from {rate} Hz: {level}"
            );
        }
    }
}

#[test]
fn speech_band_chirp_survives_stereo_downmix_and_resampling() {
    let rate = 44_100;
    let chirp = |time: f64| 0.6 * (TAU * (300.0 * time + 0.5 * 5_000.0 * time * time)).sin();
    let input: Vec<f32> = (0..rate)
        .flat_map(|frame| {
            let value = chirp(f64::from(frame) / f64::from(rate)) as f32;
            [value, value / 3.0]
        })
        .collect();
    let wav = encode_wav(&input, rate, 2);
    let output = pcm(&wav);
    assert_eq!(output.len(), 16_000);
    let error: Vec<f64> = output[100..15_900]
        .iter()
        .enumerate()
        .map(|(index, value)| value - chirp((index + 100) as f64 / 16_000.0) * 2.0 / 3.0)
        .collect();
    assert!(rms(&error) < 0.001);
}

#[test]
fn silence_non_finite_values_and_overload_produce_bounded_pcm() {
    for rate in [16_000, 44_100, 48_000] {
        let silence = encode_wav(&vec![0.0; rate as usize * 2], rate, 2);
        assert_eq!(silence.len(), 32_044);
        assert!(pcm(&silence).iter().all(|sample| *sample == 0.0));
        let non_finite = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY].repeat(rate as usize);
        assert!(pcm(&encode_wav(&non_finite, rate, 1))
            .iter()
            .all(|sample| *sample == 0.0));
        let overload: Vec<f32> = (0..rate).flat_map(|_| [f32::MAX, f32::MAX]).collect();
        let output = pcm(&encode_wav(&overload, rate, 2));
        assert!(output
            .iter()
            .all(|sample| sample.is_finite() && *sample == 1.0));
    }
}

#[test]
fn mono_16khz_probe_keeps_its_duration_and_pcm_values() {
    let samples = [-1.0, -0.5, 0.0, 0.5, 1.0];
    let wav = encode_wav(&samples, 16_000, 1);
    assert_eq!(wav.len(), 54);
    assert_eq!(&wav[44..], &[1, 128, 1, 192, 0, 0, 255, 63, 255, 127]);
    let probe = encode_wav(&vec![0.0; 12_800], 16_000, 1);
    assert_eq!(probe.len(), 25_644);
}

#[test]
fn downmix_cancels_opposite_channels_before_resampling() {
    let input: Vec<f32> = (0..48_000)
        .flat_map(|frame| {
            let sample = (PI * f64::from(frame) / 7.0).sin() as f32;
            [sample, -sample]
        })
        .collect();
    let wav = encode_wav(&input, 48_000, 2);
    assert_eq!(wav.len(), 32_044);
    assert!(pcm(&wav).iter().all(|sample| *sample == 0.0));
}
