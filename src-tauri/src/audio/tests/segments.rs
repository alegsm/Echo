use crate::audio::segmenter::Segmenter;
use crate::audio::*;

fn signal_policy() -> AudioCapturePolicy {
    AudioCapturePolicy {
        minimum_capture_duration: std::time::Duration::ZERO,
        minimum_sample_count: 0,
        ..Default::default()
    }
}

#[test]
fn release_tail_trims_only_after_speech_and_keeps_the_available_pad() {
    for (name, blocks, encoded_ms) in [
        ("room noise", vec![(1_000, 0.2), (900, 0.006)], 1_400),
        (
            "soft last word",
            vec![(1_000, 0.2), (300, 0.06), (600, 0.006)],
            1_700,
        ),
        (
            "interior pause",
            vec![(600, 0.2), (400, 0.006), (600, 0.2), (200, 0.006)],
            1_800,
        ),
    ] {
        let samples = blocks
            .iter()
            .flat_map(|(ms, amplitude)| {
                (0..ms * 16).map(move |i| if i % 2 == 0 { *amplitude } else { -*amplitude })
            })
            .collect::<Vec<f32>>();
        let mut segmenter = Segmenter::new(16_000, 1, &signal_policy());
        segmenter.push(&samples).unwrap();
        segmenter.finish().unwrap();
        let segments = segmenter.segments();
        assert_eq!(segments.len(), 1, "{name}");
        assert_eq!(segments[0].cut_kind, SegmentCutKind::ReleaseTail, "{name}");
        assert_eq!(
            segments[0].duration_ms,
            (samples.len() / 16) as u64,
            "{name}"
        );
        let wav = segments[0].bytes.as_ref().expect("speech must be encoded");
        assert_eq!(wav.len(), 44 + encoded_ms * 32, "{name}");
        let expected = samples[..encoded_ms * 16]
            .iter()
            .flat_map(|sample| ((sample * f32::from(i16::MAX)) as i16).to_le_bytes())
            .collect::<Vec<_>>();
        assert_eq!(&wav[44..], expected, "{name}");
    }
}

#[test]
fn segments_relative_room_noise_tail_has_no_audio_in_the_sealed_bundle() {
    use super::fixtures::*;
    let (backend, input) = FakeBackend::continuous();
    let capture = spawn_coordinator(backend, signal_policy());
    let owner = owner("overlay");
    let session = begin(&capture, owner.clone(), "room-noise-tail");
    input.feed(&capture, 0.2, 2_000);
    input.feed(&capture, 0.006, 2_000);
    let metadata = capture
        .stop(StopCapture {
            owner: owner.clone(),
            intent_id: "room-noise-tail".into(),
            session_id: session.session_id,
        })
        .unwrap();
    let lease = capture
        .lease_artifact(owner.clone(), metadata.artifact_id.clone())
        .unwrap();
    let audio = capture
        .read_artifact(owner, metadata.artifact_id, lease.lease_id)
        .unwrap();
    assert_eq!(audio.segments.len(), 2);
    assert_eq!(audio.segments[0].cut_kind, SegmentCutKind::QuietWindow);
    assert_eq!(audio.segments[0].duration_ms, 2_500);
    assert!(audio.segments[0].bytes.is_some());
    assert_eq!(audio.segments[1].cut_kind, SegmentCutKind::ReleaseTail);
    assert_eq!(audio.segments[1].duration_ms, 1_500);
    assert!(audio.segments[1].bytes.is_none());
    assert_eq!(audio.whole_wav().len(), 44 + 2_500 * 32);
}

#[test]
fn segments_relative_room_noise_between_speech_has_no_audio_in_the_sealed_bundle() {
    use super::fixtures::*;
    let (backend, input) = FakeBackend::continuous();
    let capture = spawn_coordinator(backend, signal_policy());
    let owner = owner("overlay");
    let session = begin(&capture, owner.clone(), "room-noise-middle");
    input.feed(&capture, 0.2, 2_000);
    input.feed(&capture, 0.006, 3_000);
    input.feed(&capture, 0.2, 1_000);
    let metadata = capture
        .stop(StopCapture {
            owner: owner.clone(),
            intent_id: "room-noise-middle".into(),
            session_id: session.session_id,
        })
        .unwrap();
    let lease = capture
        .lease_artifact(owner.clone(), metadata.artifact_id.clone())
        .unwrap();
    let audio = capture
        .read_artifact(owner, metadata.artifact_id, lease.lease_id)
        .unwrap();
    assert_eq!(audio.segments.len(), 3);
    assert_eq!(audio.segments[0].cut_kind, SegmentCutKind::QuietWindow);
    assert_eq!(audio.segments[1].cut_kind, SegmentCutKind::QuietWindow);
    assert_eq!(audio.segments[1].duration_ms, 2_000);
    assert!(audio.segments[1].bytes.is_none());
    assert_eq!(audio.segments[2].cut_kind, SegmentCutKind::ReleaseTail);
    let first = audio.segments[0].bytes.as_ref().expect("first speech");
    let last = audio.segments[2].bytes.as_ref().expect("final speech");
    let expected_pcm = [&first[44..], &last[44..]].concat();
    let whole = audio.whole_wav();
    assert_eq!(whole.len(), 44 + expected_pcm.len());
    assert_eq!(&whole[44..], expected_pcm);
}

#[test]
fn segment_boundaries_use_first_quiet_window_from_two_seconds_and_shorter_gap_after_six() {
    for noise in [0.0, 0.002, 0.006] {
        for (speech_ms, quiet_ms, expected_ms) in [
            (1_500, 500, 2_000),
            (2_000, 500, 2_500),
            (5_750, 250, 6_000),
            (6_000, 250, 6_250),
        ] {
            let mut segmenter = Segmenter::new(48_000, 1, &signal_policy());
            let mut samples = (0..speech_ms * 48)
                .map(|i| if i % 2 == 0 { 0.05 } else { -0.05 })
                .collect::<Vec<_>>();
            samples.extend((0..quiet_ms * 48).map(|i| if i % 2 == 0 { noise } else { -noise }));
            segmenter.push(&samples[..samples.len() - 1]).unwrap();
            assert!(
                segmenter.segments().is_empty(),
                "early close: {speech_ms}/{quiet_ms}"
            );
            segmenter.push(&samples[samples.len() - 1..]).unwrap();
            assert_eq!(
                segmenter.segments().len(),
                1,
                "missing close: {speech_ms}/{quiet_ms}"
            );
            assert_eq!(segmenter.segments()[0].duration_ms, expected_ms);
        }
    }
}

#[test]
fn interrupted_quiet_window_does_not_close_until_a_full_window_finishes() {
    let mut segmenter = Segmenter::new(16_000, 2, &signal_policy());
    segmenter.push(&vec![0.2; 5 * 16_000 * 2]).unwrap();
    segmenter.push(&vec![0.0; 499 * 16 * 2]).unwrap();
    segmenter.push(&[0.0, 0.2]).unwrap();
    segmenter.push(&vec![0.0; 500 * 16 * 2 - 2]).unwrap();
    assert!(segmenter.segments().is_empty());
    segmenter.push(&[0.0, 0.0]).unwrap();
    assert_eq!(segmenter.segments()[0].duration_ms, 5_999);
}

#[test]
fn silent_pieces_and_release_tail_keep_ordinals_without_audio() {
    let mut segmenter = Segmenter::new(16_000, 1, &signal_policy());
    segmenter.push(&vec![0.0; 2 * 16_000]).unwrap();
    segmenter.push(&vec![0.2; 2 * 16_000]).unwrap();
    segmenter.push(&vec![0.0; 8_000]).unwrap();
    segmenter.push(&vec![0.0; 100]).unwrap();
    segmenter.finish().unwrap();
    let segments = segmenter.segments();
    assert_eq!(
        segments.iter().map(|s| s.ordinal).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert!(segments[0].bytes.is_none());
    assert!(segments[1].bytes.is_some());
    assert!(segments[2].bytes.is_none());
}

#[test]
fn continuous_capture_closes_before_release_and_keeps_cumulative_counters() {
    use super::fixtures::*;
    let (backend, input) = FakeBackend::continuous();
    let coordinator = spawn_coordinator(backend, signal_policy());
    let session = begin(&coordinator, owner("overlay"), "continuous");
    assert!(input.feed(&coordinator, 0.2, 2_000).is_empty());
    assert!(input.feed(&coordinator, 0.0, 480).is_empty());
    let closed = input.feed(&coordinator, 0.0, 20);
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].duration_ms, 2_500);
    assert_eq!(closed[0].cut_kind, SegmentCutKind::QuietWindow);
    assert!(closed[0].bytes.is_some());
    assert_eq!(input.feed(&coordinator, 0.3, 4_000).len(), 1);
    let closed = input.feed(&coordinator, 0.0, 500);
    assert_eq!(closed.len(), 2);
    assert_eq!(closed[1].duration_ms, 4_500);
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.sample_count, 7_000 * 48);
    assert_eq!(snapshot.callback_count, 7_000 / 20 + 1);
    assert_eq!(snapshot.phase, CapturePhase::Recording);
    assert_eq!(snapshot.artifact_count, 1);
    assert_eq!(
        snapshot.artifact_bytes,
        closed
            .iter()
            .filter_map(|s| s.bytes.as_ref())
            .map(|b| b.len())
            .sum::<usize>()
    );
    input.feed(&coordinator, 0.0, 100);
    let metadata = coordinator
        .stop(StopCapture {
            owner: owner("overlay"),
            intent_id: "continuous".into(),
            session_id: session.session_id.clone(),
        })
        .unwrap();
    let lease = coordinator
        .lease_artifact(owner("overlay"), metadata.artifact_id.clone())
        .unwrap();
    let audio = coordinator
        .read_artifact(
            owner("overlay"),
            metadata.artifact_id.clone(),
            lease.lease_id,
        )
        .unwrap();
    assert_eq!(audio.segments.len(), 3);
    assert_eq!(audio.segments[0].cut_kind, SegmentCutKind::QuietWindow);
    assert_eq!(audio.segments[2].cut_kind, SegmentCutKind::ReleaseTail);
    assert!(audio.segments[2].bytes.is_none());
    assert_eq!(audio.metadata.session_id, session.session_id);
    assert_eq!(audio.metadata.intent_id, "continuous");
    assert_eq!(coordinator.snapshot().unwrap().artifact_count, 1);
    assert!(coordinator
        .lease_artifact(owner("other"), metadata.artifact_id)
        .is_err());
}

#[test]
fn all_silent_pieces_fail_with_existing_low_signal_outcome() {
    use super::fixtures::*;
    let (backend, input) = FakeBackend::continuous();
    let coordinator = spawn_coordinator(backend, signal_policy());
    let session = begin(&coordinator, owner("overlay"), "silent");
    input.feed(&coordinator, 0.0, 5_100);
    let error = coordinator
        .stop(StopCapture {
            owner: owner("overlay"),
            intent_id: "silent".into(),
            session_id: session.session_id,
        })
        .unwrap_err();
    assert_eq!(error.code, CaptureErrorCode::LowSignal);
    assert_eq!(coordinator.snapshot().unwrap().artifact_count, 0);
}

// Keep: admission reserves the entire bundle before accepting new audio.
#[test]
fn open_bundle_counts_against_cache_bounds_without_evicting_leased_audio() {
    use super::fixtures::*;
    for (maximum_artifact_count, maximum_total_artifact_bytes, leased) in [
        (1, 1_000_000, false),
        (1, 1_000_000, true),
        (4, 264_044, false),
        (4, 264_044, true),
    ] {
        let (backend, input) = FakeBackend::continuous();
        let mut policy = signal_policy();
        policy.maximum_artifact_count = maximum_artifact_count;
        policy.maximum_artifact_bytes = 264_044;
        policy.maximum_total_artifact_bytes = maximum_total_artifact_bytes;
        let coordinator = spawn_coordinator(backend, policy);
        let session = begin(&coordinator, owner("overlay"), "older");
        input.feed(&coordinator, 0.2, 1_000);
        let old = coordinator
            .stop(StopCapture {
                owner: owner("overlay"),
                intent_id: "older".into(),
                session_id: session.session_id,
            })
            .unwrap();
        let lease = leased.then(|| {
            coordinator
                .lease_artifact(owner("overlay"), old.artifact_id.clone())
                .unwrap()
        });
        let reservation = coordinator
            .reserve(
                owner("overlay"),
                intent("newer", CapturePurpose::PushToTalk),
            )
            .unwrap();
        let admission = coordinator.begin(reservation);
        if let Some(lease) = lease {
            assert_eq!(
                admission.unwrap_err().code,
                CaptureErrorCode::ArtifactCapacityReached
            );
            assert_eq!(coordinator.snapshot().unwrap().phase, CapturePhase::Idle);
            assert!(
                coordinator
                    .read_artifact(owner("overlay"), old.artifact_id, lease.lease_id)
                    .unwrap()
                    .is_complete
            );
        } else {
            admission.unwrap();
            input.feed(&coordinator, 0.2, 8_000);
            input.feed(&coordinator, 0.0, 260);
            let snapshot = coordinator.snapshot().unwrap();
            assert_eq!(snapshot.artifact_count, 1);
            assert_eq!(snapshot.phase, CapturePhase::Recording);
            assert_eq!(snapshot.artifact_bytes, 264_044);
            assert!(coordinator
                .lease_artifact(owner("overlay"), old.artifact_id)
                .is_err());
        }
    }
}

#[test]
fn open_bundle_survives_idle_ttl_and_cleans_up_on_cancel_or_owner_exit() {
    use super::fixtures::*;
    for owner_exit in [false, true] {
        let (backend, input) = FakeBackend::continuous();
        let mut policy = signal_policy();
        policy.artifact_ttl = std::time::Duration::from_millis(1);
        let (coordinator, events) = spawn_with_events(backend, policy);
        coordinator.freeze_time();
        let session = begin(&coordinator, owner("overlay"), "cleanup");
        input.feed(&coordinator, 0.2, 8_000);
        input.feed(&coordinator, 0.0, 260);
        coordinator.advance_time(std::time::Duration::from_millis(25));
        assert_eq!(coordinator.snapshot().unwrap().artifact_count, 1);
        let artifact_id = event_with_outcome(&events, "segment_closed")
            .artifact_id
            .unwrap();
        let lease = coordinator
            .lease_artifact(owner("overlay"), artifact_id.clone())
            .unwrap();
        assert!(
            !coordinator
                .read_artifact(
                    owner("overlay"),
                    artifact_id.clone(),
                    lease.lease_id.clone()
                )
                .unwrap()
                .is_complete
        );
        if owner_exit {
            coordinator.cleanup_owner(owner("overlay")).unwrap();
        } else {
            coordinator
                .cancel(CancelCapture {
                    owner: owner("overlay"),
                    intent_id: "cleanup".into(),
                    session_id: Some(session.session_id),
                    reason: "user_cancelled",
                })
                .unwrap();
            assert_eq!(
                coordinator
                    .lease_artifact(owner("overlay"), artifact_id.clone())
                    .unwrap_err()
                    .code,
                CaptureErrorCode::ArtifactExpired
            );
            assert!(
                !coordinator
                    .read_artifact(
                        owner("overlay"),
                        artifact_id.clone(),
                        lease.lease_id.clone()
                    )
                    .unwrap()
                    .is_complete
            );
            coordinator
                .release_artifact(ArtifactReleaseRequest {
                    owner: owner("overlay"),
                    artifact_id,
                    lease_id: lease.lease_id,
                    disposition: ArtifactReleaseDisposition::Discard,
                })
                .unwrap();
        }
        let snapshot = coordinator.snapshot().unwrap();
        assert_eq!(snapshot.phase, CapturePhase::Idle);
        assert_eq!(snapshot.artifact_count, 0);
    }
}

#[test]
fn normalized_segment_bytes_enforce_the_artifact_limit_during_capture() {
    use super::fixtures::*;
    for (maximum_artifact_bytes, pieces) in [(80_000, 1), (160_060, 2)] {
        let (backend, input) = FakeBackend::continuous();
        let mut policy = signal_policy();
        policy.maximum_artifact_bytes = maximum_artifact_bytes;
        let coordinator = spawn_coordinator(backend, policy);
        begin(&coordinator, owner("overlay"), "encoded-limit");
        for _ in 0..pieces {
            input.feed(&coordinator, 0.2, 2_000);
            input.feed(&coordinator, 0.0, 500);
        }
        let snapshot = coordinator.snapshot().unwrap();
        assert_eq!(
            snapshot.last_error,
            Some(CaptureErrorCode::ArtifactSizeLimit)
        );
        assert_eq!(snapshot.phase, CapturePhase::Idle);
        assert_eq!(snapshot.artifact_bytes, 0);
    }
}

#[test]
fn quiet_window_requires_both_rms_and_peak_thresholds() {
    for (sample_rate, channels) in [(16_000, 1), (16_000, 2), (16_001, 1), (16_001, 2)] {
        for (age_seconds, window_ms) in [(2, 500), (6, 250)] {
            for (amplitude, peak_spike, closes) in [
                (0.002, false, true),
                (0.06, false, false),
                (0.0, true, false),
            ] {
                let mut segmenter = Segmenter::new(sample_rate, channels, &signal_policy());
                let rate = sample_rate as usize;
                let channels = usize::from(channels);
                let window_frames = (rate * window_ms).div_ceil(1_000);
                let speech_frames = (age_seconds * rate).div_ceil(window_frames) * window_frames;
                segmenter
                    .push(&vec![0.2; speech_frames * channels])
                    .unwrap();
                let mut quiet = vec![amplitude; window_frames * channels];
                if peak_spike {
                    *quiet.last_mut().unwrap() = 0.06;
                }
                segmenter.push(&quiet).unwrap();
                assert_eq!(
                    !segmenter.segments().is_empty(), closes,
                    "{sample_rate} Hz, {channels} channels, age {age_seconds} s, window {window_ms} ms, amplitude {amplitude}, peak spike {peak_spike}"
                );
            }
        }
    }
}

#[test]
fn actor_tick_closes_a_segment_without_stop_or_test_drain_commands() {
    use super::fixtures::*;
    let mut samples = vec![0.2; 96_000];
    samples.extend(vec![0.0; 24_000]);
    let (backend, _) = FakeBackend::immediate(PlayBehavior::Samples(samples));
    // This fake delivers 2.5 seconds in one callback rather than hardware-sized blocks.
    let policy = AudioCapturePolicy {
        pcm_headroom: PcmHeadroom::Bytes(120_000 * 4),
        ..signal_policy()
    };
    let (coordinator, events) = spawn_with_events(backend, policy);
    begin(&coordinator, owner("overlay"), "autonomous-segment");
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.phase, CapturePhase::Recording);
    assert_eq!(snapshot.artifact_count, 1);
    assert_eq!(snapshot.artifact_bytes, 80_044);
    assert!(event_with_outcome(&events, "segment_closed")
        .artifact_id
        .is_some());
}

#[test]
fn quiet_window_waits_a_full_half_second_at_odd_device_rates() {
    let mut segmenter = Segmenter::new(16_001, 1, &signal_policy());
    segmenter.push(&vec![0.2; 5 * 16_001]).unwrap();
    segmenter.push(&vec![0.0; 8_000]).unwrap();
    assert!(segmenter.segments().is_empty());
    segmenter.push(&[0.0]).unwrap();
    assert_eq!(segmenter.segments()[0].duration_ms, 5_500);
}

#[test]
fn segments_valley_cut_retains_every_sample_in_the_next_piece() {
    let mut segmenter = Segmenter::new(16_000, 1, &signal_policy());
    let mut samples = vec![0.2; 11 * 16_000];
    samples.extend(vec![0.00005; 1_600]);
    samples.extend(vec![0.3; 1_600]);
    samples.extend(vec![0.0; 1_600]);
    samples.extend(vec![0.4; 11_200]);
    segmenter.push(&samples).unwrap();
    assert_eq!(segmenter.segments().len(), 1);
    assert_eq!(segmenter.segments()[0].duration_ms, 11_300);
    assert_eq!(segmenter.segments()[0].cut_kind, SegmentCutKind::Valley);
    let mut following = vec![0.25; 20_800];
    following.extend(vec![0.0; 8_000]);
    following.extend(vec![0.3; 16_000]);
    samples.extend_from_slice(&following);
    segmenter.push(&following).unwrap();
    assert_eq!(segmenter.segments()[1].duration_ms, 2_500);
    segmenter.finish().unwrap();
    assert_eq!(segmenter.segments()[2].duration_ms, 1_000);
    assert_eq!(
        segmenter.segments()[1].cut_kind,
        SegmentCutKind::QuietWindow
    );
    assert_eq!(
        segmenter.segments()[2].cut_kind,
        SegmentCutKind::ReleaseTail
    );
    let actual: Vec<u8> = segmenter
        .segments()
        .iter()
        .flat_map(|piece| piece.bytes.as_ref().unwrap()[44..].iter().copied())
        .collect();
    let expected: Vec<u8> = samples
        .iter()
        .flat_map(|sample| ((sample * f32::from(i16::MAX)) as i16).to_le_bytes())
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn segments_continuous_speech_uses_the_quietest_available_frame_and_stays_bounded() {
    let mut segmenter = Segmenter::new(16_000, 1, &signal_policy());
    let mut samples = vec![0.05; 11 * 16_000];
    samples.extend(vec![0.03; 1_600]);
    samples.extend(vec![0.04; 14_400]);
    segmenter.push(&samples).unwrap();
    assert_eq!(segmenter.segments().len(), 1);
    assert_eq!(segmenter.segments()[0].duration_ms, 11_100);
    samples.extend(vec![0.05; 19 * 16_000]);
    segmenter.push(&samples[12 * 16_000..]).unwrap();
    segmenter.finish().unwrap();
    assert!(segmenter
        .segments()
        .iter()
        .all(|piece| piece.duration_ms <= 30_000));
    assert_eq!(
        segmenter
            .segments()
            .iter()
            .map(|piece| piece.duration_ms)
            .sum::<u64>(),
        31_000
    );
    let actual: Vec<u8> = segmenter
        .segments()
        .iter()
        .flat_map(|piece| piece.bytes.as_ref().unwrap()[44..].iter().copied())
        .collect();
    let expected: Vec<u8> = samples
        .iter()
        .flat_map(|sample| ((sample * f32::from(i16::MAX)) as i16).to_le_bytes())
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn segments_relative_reference_carries_across_valley_cuts() {
    let mut segmenter = Segmenter::new(16_000, 1, &signal_policy());
    let mut samples = vec![0.05; 11 * 16_000];
    samples.extend(vec![0.0; 1_600]);
    samples.extend(vec![0.05; 12_800]);
    samples.extend(vec![0.006; 1_600]);
    segmenter.push(&samples).unwrap();
    assert_eq!(segmenter.segments()[0].duration_ms, 11_100);
    segmenter.push(&vec![0.006; 17_599]).unwrap();
    assert_eq!(segmenter.segments().len(), 1);
    segmenter.push(&[0.006]).unwrap();
    assert_eq!(segmenter.segments()[1].duration_ms, 2_000);
}

// Reconstruct the logged 500 ms RMS/peak windows with deterministic room noise.
fn logged_profile_samples(profile: &[(f32, f32)]) -> (Vec<f32>, Vec<f32>) {
    let mut seed = 12345u32;
    let mut noise = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    };
    let mut samples = Vec::new();
    for (rms, peak) in profile {
        for i in 0..24_000 {
            let value = noise();
            samples.push(if i == 12_000 {
                *peak
            } else {
                value * rms * 3f32.sqrt()
            });
        }
    }
    let tail_noise = (0..14_400).map(|_| noise()).collect();
    (samples, tail_noise)
}

#[test]
fn speech_run_stop_tap_does_not_keep_the_logged_silent_wait() {
    let profiles: [(&str, &[(f32, f32)]); 2] = [
        (
            "session-1",
            &[
                (0.003, 0.025),
                (0.067, 0.248),
                (0.023, 0.134),
                (0.043, 0.197),
                (0.016, 0.171),
                (0.032, 0.266),
                (0.043, 0.22),
                (0.054, 0.269),
                (0.027, 0.228),
                (0.007, 0.082),
                (0.038, 0.161),
                (0.034, 0.16),
                (0.046, 0.295),
                (0.005, 0.057),
                (0.042, 0.283),
                (0.027, 0.239),
                (0.004, 0.033),
                (0.038, 0.175),
                (0.025, 0.152),
                (0.018, 0.116),
                (0.017, 0.108),
                (0.002, 0.018),
                (0.017, 0.196),
                (0.027, 0.128),
                (0.044, 0.224),
                (0.03, 0.177),
                (0.022, 0.154),
                (0.017, 0.124),
                (0.035, 0.158),
                (0.037, 0.217),
                (0.033, 0.224),
                (0.024, 0.215),
                (0.032, 0.176),
                (0.018, 0.132),
                (0.003, 0.036),
                (0.002, 0.008),
                (0.002, 0.008),
                (0.002, 0.007),
                (0.002, 0.007),
            ],
        ),
        (
            "session-2",
            &[
                (0.002, 0.012),
                (0.059, 0.298),
                (0.027, 0.147),
                (0.003, 0.033),
                (0.039, 0.297),
                (0.049, 0.19),
                (0.035, 0.194),
                (0.003, 0.021),
                (0.035, 0.217),
                (0.039, 0.18),
                (0.022, 0.105),
                (0.002, 0.009),
                (0.006, 0.05),
                (0.036, 0.164),
                (0.018, 0.15),
                (0.044, 0.311),
                (0.041, 0.227),
                (0.03, 0.131),
                (0.015, 0.127),
                (0.003, 0.011),
                (0.003, 0.011),
                (0.004, 0.025),
            ],
        ),
    ];
    let mut failures = Vec::new();
    for ((name, profile), expected_sizes) in profiles.into_iter().zip([
        [Some(216_042), Some(134_674), Some(200_316), None, None],
        [
            Some(64_044),
            Some(64_044),
            Some(64_044),
            Some(124_608),
            None,
        ],
    ]) {
        let (samples, tail_noise) = logged_profile_samples(profile);
        let mut before_tail = Segmenter::new(48_000, 1, &signal_policy());
        for callback in samples.chunks(512) {
            before_tail.push(callback).unwrap();
        }
        let mut baseline = None;
        let taps = std::iter::once(None).chain([15, 50].into_iter().flat_map(|ms| {
            // Keep the observed leaks and cross a 480-sample sub-block edge.
            [8_720, 8_960, 9_200, 10_000, 9_119]
                .into_iter()
                .map(move |start| Some((start, ms * 48)))
        }));
        for tap in taps {
            let tail: Vec<_> = tail_noise
                .iter()
                .enumerate()
                .map(|(i, noise)| {
                    let amplitude = if tap
                        .is_some_and(|(start, length)| (start..start + length).contains(&i))
                    {
                        0.3
                    } else {
                        0.0035
                    };
                    noise * amplitude
                })
                .collect();
            // Preserve every rolling window and already encoded piece for each stop.
            let mut segmenter = before_tail.clone();
            for callback in tail.chunks(512) {
                segmenter.push(callback).unwrap();
            }
            segmenter.finish().unwrap();
            let pieces = segmenter.segments();
            let sizes: Vec<_> = pieces
                .iter()
                .map(|s| s.bytes.as_ref().map(|b| b.len()))
                .collect();
            if sizes != expected_sizes {
                failures.push(format!(
                    "{name}: encoded sizes changed, tap={tap:?}: {sizes:?}"
                ));
            }
            if pieces.last().unwrap().bytes.is_some() {
                failures.push(format!("{name}: silent release tail, tap={tap:?}"));
            }
            let output = pieces.iter().map(|s| s.bytes.clone()).collect::<Vec<_>>();
            if let Some(baseline) = &baseline {
                if baseline != &output {
                    failures.push(format!("{name}: tap={tap:?} added silent-wait audio"));
                }
            } else {
                baseline = Some(output);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn speech_run_rejects_isolated_bursts_and_keeps_a_short_final_word() {
    let mut failures = Vec::new();
    for burst_ms in [10, 15, 50, 100, 300] {
        // Bursts cover every 160-sample sub-block phase once. The word covers
        // aligned, partial and boundary-crossing starts without repeating phases.
        let starts: Vec<_> = if burst_ms <= 100 {
            (0..160).collect()
        } else {
            vec![0, 79, 159, 1_441]
        };
        for start in starts {
            let sound_end = start + 16 * burst_ms;
            // Complete the next 100 ms boundary block for burst mutations; a word
            // also needs enough quiet to exercise its entire release pad.
            let quiet_ms = if burst_ms <= 100 { 100 } else { 410 };
            let mut samples = vec![0.0; sound_end + 16 * quiet_ms];
            samples[start..sound_end].fill(0.2);
            let mut segmenter = Segmenter::new(16_000, 1, &signal_policy());
            for callback in samples.chunks(137) {
                segmenter.push(callback).unwrap();
            }
            segmenter.finish().unwrap();
            let piece = &segmenter.segments()[0];
            if burst_ms == 300 {
                let wav = piece.bytes.as_ref().expect("300 ms word must survive");
                let word_end = start + 16 * burst_ms;
                assert!(wav.len() >= 44 + word_end * 2);
                assert!(wav.len() <= 44 + (word_end + 16 * 410) * 2);
                let expected: Vec<_> = samples[..word_end]
                    .iter()
                    .flat_map(|s| ((s * f32::from(i16::MAX)) as i16).to_le_bytes())
                    .collect();
                assert_eq!(&wav[44..44 + expected.len()], expected);
            } else if piece.bytes.is_some() {
                failures.push((burst_ms, start));
            }
        }
        assert!(
            failures.is_empty(),
            "{burst_ms} ms burst encoded at {} offsets, first crossings: {:?}",
            failures.len(),
            &failures[..failures.len().min(10)]
        );
    }
}

#[test]
fn speech_run_all_burst_pieces_fail_low_signal() {
    use super::fixtures::*;
    let (backend, input) = FakeBackend::continuous();
    let capture = spawn_coordinator(backend, signal_policy());
    let session = begin(&capture, owner("overlay"), "bursts");
    for _ in 0..2 {
        input.feed(&capture, 0.2, 100);
        let pieces = input.feed(&capture, 0.0, 1_900);
        assert!(pieces.iter().all(|s| s.bytes.is_none()));
    }
    input.feed(&capture, 0.2, 100);
    let error = capture
        .stop(StopCapture {
            owner: owner("overlay"),
            intent_id: "bursts".into(),
            session_id: session.session_id,
        })
        .unwrap_err();
    assert_eq!(error.code, CaptureErrorCode::LowSignal);
}

#[test]
fn speech_duration_tolerates_syllable_dips_but_requires_120_ms_in_200_ms() {
    for (sound_ms, expected) in [(50, false), (60, true)] {
        let mut segmenter = Segmenter::new(16_000, 1, &signal_policy());
        for _ in 0..3 {
            segmenter.push(&vec![0.2; sound_ms * 16]).unwrap();
            segmenter.push(&vec![0.0; (100 - sound_ms) * 16]).unwrap();
        }
        segmenter.push(&vec![0.0; 500 * 16]).unwrap();
        segmenter.finish().unwrap();
        assert_eq!(
            segmenter.segments()[0].bytes.is_some(),
            expected,
            "{sound_ms} ms per syllable"
        );
    }
}

// Keep: every minimum-length cut, including WAV headers, fits the cap budget.
#[test]
fn ten_minute_normalized_wav_budget_keeps_every_two_second_piece() {
    let policy = AudioCapturePolicy::default();
    let mut segmenter = Segmenter::new(16_000, 1, &policy);
    let speech = vec![0.2; 24_000];
    let quiet = vec![0.0; 8_000];
    for _ in 0..300 {
        segmenter.push(&speech).unwrap();
        segmenter.push(&quiet).unwrap();
    }
    segmenter.finish().unwrap();
    assert_eq!(segmenter.segments().len(), 300);
    assert!(segmenter
        .segments()
        .iter()
        .all(|piece| piece.bytes.is_some()));
    let bytes: usize = segmenter
        .segments()
        .iter()
        .filter_map(|piece| piece.bytes.as_ref())
        .map(|wav| wav.len())
        .sum();
    assert_eq!(bytes, 300 * (64_000 + 44));
    assert!(bytes <= policy.maximum_artifact_bytes);
}
