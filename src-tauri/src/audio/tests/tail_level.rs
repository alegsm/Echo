use crate::audio::{encode_wav, tail_level, TailLevel};

/// Square wave with exactly the requested RMS, so expected levels are exact.
fn level_wav(parts: &[(f64, f32)]) -> Vec<u8> {
    let mut samples = Vec::new();
    for &(seconds, rms) in parts {
        let count = (seconds * 16_000.0).round() as usize;
        samples.extend((0..count).map(|index| if index % 2 == 0 { rms } else { -rms }));
    }
    encode_wav(&samples, 16_000, 1)
}

/// Two seconds of speech at RMS 0.1 followed by `tail`, a list of (seconds, RMS) parts.
fn speech_then(tail: &[(f64, f32)]) -> Vec<u8> {
    let mut parts = vec![(2.0, 0.1)];
    parts.extend_from_slice(tail);
    level_wav(&parts)
}

/// Whether the window was judged quiet; `None` when it was not judged at all.
fn quiet_verdict(level: TailLevel) -> Option<bool> {
    matches!(level, TailLevel::Judged { .. }).then(|| level.is_quiet())
}

fn judged_quiet(wav: &[u8], end_s: f64) -> Option<bool> {
    quiet_verdict(tail_level(wav, 2.0, end_s))
}

// Keep: the duration rule, table form. The tail is quiet when it stays at speech level for
// under 120 ms (the segmenter's shortest speech run): a decaying word end or two clicks
// do not save it, while a short real word after a long pause, or a soft closer, does.
#[test]
fn tail_level_judges_how_long_the_tail_stays_at_speech_level() {
    let decay = [
        (0.02, 0.1),
        (0.02, 0.07),
        (0.02, 0.04),
        (0.02, 0.02),
        (0.02, 0.01),
        (0.4, 0.002),
    ];
    let clicks = [
        (0.2, 0.002),
        (0.02, 0.1),
        (0.16, 0.002),
        (0.02, 0.1),
        (0.1, 0.002),
    ];
    for (name, tail, end_s, expected) in [
        (
            "12a: 100 ms at speech level, then 400 ms quiet",
            speech_then(&[(0.1, 0.1), (0.4, 0.002)]),
            2.5,
            true,
        ),
        (
            "12b: 1.6 s quiet, then a 200 ms word at speech level",
            speech_then(&[(1.6, 0.002), (0.2, 0.1)]),
            3.8,
            false,
        ),
        (
            "12b with a 2.0 s pause, so the word is under a tenth of the window",
            speech_then(&[(2.0, 0.002), (0.2, 0.1)]),
            4.2,
            false,
        ),
        (
            "12c: 500 ms quiet with two 20 ms clicks",
            speech_then(&clicks),
            2.5,
            true,
        ),
        (
            "12d: 400 ms soft closer 12 dB down",
            speech_then(&[(0.4, 0.025)]),
            2.4,
            false,
        ),
        (
            "12e: 100 ms decay from 0.1 to 0.01, then 400 ms quiet",
            speech_then(&decay),
            2.5,
            true,
        ),
        (
            "120 ms at speech level is speech",
            speech_then(&[(0.12, 0.1), (0.4, 0.002)]),
            2.52,
            false,
        ),
        (
            "just under 0.1 x speech is not loud",
            speech_then(&[(0.5, 0.009)]),
            2.5,
            true,
        ),
        (
            "just over 0.1 x speech is loud",
            speech_then(&[(0.5, 0.011)]),
            2.5,
            false,
        ),
    ] {
        assert_eq!(judged_quiet(&tail, end_s), Some(expected), "{name}");
    }
}

// Keep: the Debug log record reports these numbers, so they must be the real values.
#[test]
fn tail_level_reports_times_and_levels() {
    let wav = level_wav(&[(2.0, 0.1), (0.3, 0.002), (0.2, 0.1), (0.5, 0.002)]);
    let level = tail_level(&wav, 2.1, 3.0);
    let TailLevel::Judged {
        inside_ms,
        loud_ms,
        speech_rms,
        tail_peak_rms,
    } = level
    else {
        panic!("{level:?}");
    };
    assert_eq!((inside_ms, loud_ms), (900, 200), "{level:?}");
    assert!((speech_rms - 0.1).abs() < 0.001, "{level:?}");
    assert!((tail_peak_rms - 0.1).abs() < 0.001, "{level:?}");
    assert!(!level.is_quiet());
}

// Keep: every guard that must leave the audio and text alone.
#[test]
fn tail_level_declines_to_judge_unsafe_inputs() {
    let speech_then_quiet = level_wav(&[(2.0, 0.1), (1.0, 0.002)]);
    let whisper = level_wav(&[(3.0, 0.01)]);
    let below_floor = level_wav(&[(3.0, 0.002)]);
    let mut wrong_rate = speech_then_quiet.clone();
    wrong_rate[24..28].copy_from_slice(&44_100u32.to_le_bytes());
    let mut stereo = speech_then_quiet.clone();
    stereo[22..24].copy_from_slice(&2u16.to_le_bytes());
    let mut data_length_lies = speech_then_quiet.clone();
    data_length_lies[40..44].copy_from_slice(&7u32.to_le_bytes());
    let odd_length = speech_then_quiet[..speech_then_quiet.len() - 1].to_vec();
    let header_only = speech_then_quiet[..44].to_vec();
    let quiet = Some(true);
    for (name, wav, start, end, expected) in [
        ("speech-level tail", &whisper, 2.0, 3.0, Some(false)),
        (
            "whole recording below the floor",
            &below_floor,
            2.0,
            3.0,
            None,
        ),
        ("empty audio", &Vec::new(), 2.0, 3.0, None),
        ("header only", &header_only, 2.0, 3.0, None),
        ("not a WAV", &vec![7; 100], 2.0, 3.0, None),
        ("wrong sample rate", &wrong_rate, 2.0, 3.0, None),
        ("stereo", &stereo, 2.0, 3.0, None),
        ("data length mismatch", &data_length_lies, 2.0, 3.0, None),
        ("odd data length", &odd_length, 2.0, 3.0, None),
        ("start NaN", &speech_then_quiet, f64::NAN, 3.0, None),
        ("end infinite", &speech_then_quiet, 2.0, f64::INFINITY, None),
        ("end NaN", &speech_then_quiet, 2.0, f64::NAN, None),
        ("negative start", &speech_then_quiet, -1.0, 3.0, None),
        ("start equals end", &speech_then_quiet, 2.5, 2.5, None),
        ("start after end", &speech_then_quiet, 2.9, 2.1, None),
        ("start past the audio", &speech_then_quiet, 3.5, 4.5, None),
        ("window under 300 ms", &speech_then_quiet, 2.0, 2.2, None),
        ("300 ms window", &speech_then_quiet, 2.0, 2.3, quiet),
        (
            "only 100 ms inside the audio",
            &speech_then_quiet,
            2.9,
            30.0,
            None,
        ),
        (
            "300 ms inside, end clamped",
            &speech_then_quiet,
            2.7,
            30.0,
            quiet,
        ),
    ] {
        let judged = quiet_verdict(tail_level(wav, start, end));
        assert_eq!(judged, expected, "{name}");
    }
}

// Keep: the speech level is a robust average, so pauses and room tone do not lower it.
#[test]
fn tail_level_speech_level_ignores_pauses_and_room_tone() {
    let wav = level_wav(&[
        (0.5, 0.1),
        (3.0, 0.0),
        (0.5, 0.1),
        (0.5, 0.001),
        (1.0, 0.02),
    ]);
    let level = tail_level(&wav, 4.5, 5.5);
    let TailLevel::Judged { speech_rms, .. } = level else {
        panic!("{level:?}");
    };
    assert!((speech_rms - 0.1).abs() < 0.001, "{speech_rms}");
}
