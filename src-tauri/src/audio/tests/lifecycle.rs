use super::fixtures::*;
use crate::audio::*;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[test]
fn reports_busy_with_the_active_dictation_purpose() {
    let (backend, _) = FakeBackend::immediate(PlayBehavior::NoCallback);
    let coordinator = spawn_coordinator(backend, test_policy());
    coordinator
        .reserve(owner("orb"), intent("one", CapturePurpose::PushToTalk))
        .unwrap();

    let error = coordinator
        .reserve(owner("overlay"), intent("two", CapturePurpose::PushToTalk))
        .unwrap_err();

    assert_eq!(error.code, CaptureErrorCode::MicrophoneBusy);
    assert_eq!(error.active_purpose, Some(CapturePurpose::PushToTalk));
    assert!(error.retryable);
}

#[test]
fn cancel_before_reserve_tombstones_the_intent() {
    let (backend, _) = FakeBackend::immediate(PlayBehavior::NoCallback);
    let coordinator = spawn_coordinator(backend, test_policy());
    let capture_owner = owner("orb");
    coordinator
        .cancel(CancelCapture {
            owner: capture_owner.clone(),
            intent_id: "released".to_owned(),
            session_id: None,
            reason: "hotkey_released",
        })
        .unwrap();

    let error = coordinator
        .reserve(
            capture_owner,
            intent("released", CapturePurpose::PushToTalk),
        )
        .unwrap_err();

    assert_eq!(error.code, CaptureErrorCode::CaptureCancelled);
    assert_eq!(
        coordinator
            .reserve(owner("orb"), intent("released", CapturePurpose::PushToTalk),)
            .unwrap_err()
            .code,
        CaptureErrorCode::CaptureCancelled
    );
}

#[test]
fn cancel_before_reserve_survives_another_active_owner() {
    let (backend, _) = FakeBackend::immediate(PlayBehavior::NoCallback);
    let coordinator = spawn_coordinator(backend, test_policy());
    let active_owner = owner("overlay");
    coordinator
        .reserve(
            active_owner.clone(),
            intent("active", CapturePurpose::PushToTalk),
        )
        .unwrap();
    coordinator
        .cancel(CancelCapture {
            owner: owner("other-window"),
            intent_id: "delayed".to_owned(),
            session_id: None,
            reason: "user_cancelled",
        })
        .unwrap();
    coordinator
        .cancel(CancelCapture {
            owner: active_owner,
            intent_id: "active".to_owned(),
            session_id: None,
            reason: "user_cancelled",
        })
        .unwrap();

    assert_eq!(
        coordinator
            .reserve(
                owner("other-window"),
                intent("delayed", CapturePurpose::PushToTalk),
            )
            .unwrap_err()
            .code,
        CaptureErrorCode::CaptureCancelled
    );
}

#[test]
fn stale_backend_events_cannot_advance_or_fail_a_new_capture() {
    let backend = FakeBackend::sequence([
        PlayBehavior::Samples(vec![0.1; 32]),
        PlayBehavior::NoCallback,
    ]);
    let coordinator = spawn_coordinator(backend, test_policy());
    let capture_owner = owner("orb");
    let first = coordinator
        .reserve(
            capture_owner.clone(),
            intent("first", CapturePurpose::PushToTalk),
        )
        .unwrap();
    let stale_generation = first.generation;
    coordinator.begin(first).unwrap();
    coordinator
        .cancel(CancelCapture {
            owner: capture_owner.clone(),
            intent_id: "first".to_owned(),
            session_id: None,
            reason: "hotkey_released",
        })
        .unwrap();
    let second = coordinator
        .reserve(
            capture_owner.clone(),
            intent("second", CapturePurpose::PushToTalk),
        )
        .unwrap();
    let current_generation = second.generation;
    let pending_coordinator = coordinator.clone();
    let pending = std::thread::spawn(move || pending_coordinator.begin(second));
    std::thread::sleep(Duration::from_millis(20));

    coordinator.inject_backend_event(crate::audio::backend::BackendEvent::FirstCallback(
        stale_generation,
    ));
    coordinator.inject_backend_event(crate::audio::backend::BackendEvent::StreamError(
        stale_generation,
        CaptureErrorCode::StreamInvalidated,
    ));
    std::thread::sleep(Duration::from_millis(20));
    assert!(!pending.is_finished());

    coordinator.inject_backend_event(crate::audio::backend::BackendEvent::FirstCallback(
        current_generation,
    ));
    assert_eq!(pending.join().unwrap().unwrap().intent_id, "second");
}

#[test]
fn cancel_during_open_invalidates_late_stream() {
    let (backend, open_started, release_open, drops) = FakeBackend::blocked();
    let coordinator = spawn_coordinator(backend, test_policy());
    let capture_owner = owner("orb");
    let reservation = coordinator
        .reserve(
            capture_owner.clone(),
            intent("slow", CapturePurpose::PushToTalk),
        )
        .unwrap();
    let begin_coordinator = coordinator.clone();
    let pending = std::thread::spawn(move || begin_coordinator.begin(reservation));
    open_started.recv_timeout(Duration::from_secs(10)).unwrap();

    coordinator
        .cancel(CancelCapture {
            owner: capture_owner,
            intent_id: "slow".to_owned(),
            session_id: None,
            reason: "hotkey_released",
        })
        .unwrap();
    release_open.send(()).unwrap();

    assert_eq!(
        pending.join().unwrap().unwrap_err().code,
        CaptureErrorCode::CaptureCancelled
    );
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test]
fn first_callback_timeout_releases_capture() {
    let (backend, drops) = FakeBackend::immediate(PlayBehavior::NoCallback);
    let mut policy = test_policy();
    policy.first_callback_timeout = Duration::from_millis(30);
    let coordinator = spawn_coordinator(backend, policy);
    let reservation = coordinator
        .reserve(owner("orb"), intent("timeout", CapturePurpose::PushToTalk))
        .unwrap();

    let error = coordinator.begin(reservation).unwrap_err();

    assert_eq!(error.code, CaptureErrorCode::FirstCallbackTimeout);
    assert_eq!(coordinator.snapshot().unwrap().phase, CapturePhase::Idle);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

// Keep: the injected native clock seals retained audio at the cap and preserves identity.
#[test]
fn ten_minute_deadline_seals_and_manual_stop_retrieves_the_same_bundle() {
    let backend = FakeBackend::sequence([
        PlayBehavior::Samples(vec![0.2; 14_400]),
        PlayBehavior::Samples(vec![0.2; 14_400]),
    ]);
    let (capture, events) = spawn_with_events(backend, test_policy());
    capture.freeze_time();
    let session = begin(&capture, owner("overlay"), "deadline");
    capture.advance_time(Duration::from_secs(599));
    assert_eq!(capture.snapshot().unwrap().phase, CapturePhase::Recording);
    capture.advance_time(Duration::from_secs(1));
    let terminal = event_with_outcome(&events, "artifact_created");
    assert_eq!(terminal.intent_id.as_deref(), Some("deadline"));
    assert_eq!(
        terminal.session_id.as_deref(),
        Some(session.session_id.as_str())
    );
    assert_eq!(terminal.error, None);
    let json = serde_json::to_value(&terminal).unwrap();
    assert_eq!(json["elapsedMs"], 600_000);
    assert_eq!(json["limitMs"], 600_000);
    let next = begin(&capture, owner("overlay"), "next");
    let metadata = capture
        .stop(StopCapture {
            owner: owner("overlay"),
            intent_id: "deadline".into(),
            session_id: session.session_id.clone(),
        })
        .unwrap();
    assert_eq!(Some(metadata.artifact_id.clone()), terminal.artifact_id);
    let lease = capture
        .lease_artifact(owner("overlay"), metadata.artifact_id.clone())
        .unwrap();
    let audio = capture
        .read_artifact(owner("overlay"), metadata.artifact_id, lease.lease_id)
        .unwrap();
    assert!(audio.is_complete);
    assert!(audio.whole_wav().len() > 44);
    assert_eq!(
        capture.snapshot().unwrap().session_id,
        Some(next.session_id)
    );
    assert!(capture
        .stop(StopCapture {
            owner: owner("overlay"),
            intent_id: "foreign".into(),
            session_id: session.session_id,
        })
        .is_err());
}

// Keep: protected artifact capacity is rejected before hardware starts.
#[test]
fn exhausted_artifact_capacity_fails_before_play() {
    let (backend, drops) = FakeBackend::immediate(PlayBehavior::Samples(vec![0.2; 14_400]));
    let policy = AudioCapturePolicy {
        maximum_artifact_count: 0,
        ..test_policy()
    };
    let capture = spawn_coordinator(backend, policy);
    let reservation = capture
        .reserve(owner("overlay"), intent("full", CapturePurpose::PushToTalk))
        .unwrap();
    assert_eq!(
        capture.begin(reservation).unwrap_err().code,
        CaptureErrorCode::ArtifactCapacityReached
    );
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    assert_eq!(capture.snapshot().unwrap().phase, CapturePhase::Idle);
}

#[test]
fn operation_timeout_and_shutdown_are_bounded_during_stuck_open() {
    let (backend, open_started, release_open, _) = FakeBackend::blocked();
    let mut policy = test_policy();
    policy.operation_timeout = Duration::from_secs(2);
    let coordinator = spawn_coordinator(backend, policy);
    let reservation = coordinator
        .reserve(owner("orb"), intent("stuck", CapturePurpose::PushToTalk))
        .unwrap();
    let pending = {
        let coordinator = coordinator.clone();
        std::thread::spawn(move || coordinator.begin(reservation))
    };
    open_started.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(pending.join().unwrap().unwrap_err().stage, "actor_timeout");
    assert_eq!(
        coordinator
            .reserve(owner("overlay"), intent("next", CapturePurpose::PushToTalk))
            .unwrap_err()
            .code,
        CaptureErrorCode::MicrophoneBusy
    );

    let started = std::time::Instant::now();
    coordinator.shutdown();
    assert!(started.elapsed() < Duration::from_secs(1));
    release_open.send(()).unwrap();
}

#[test]
fn failed_start_event_is_idle_and_carries_the_terminal_error() {
    let (coordinator, events) = spawn_with_events(
        FakeBackend::failing(CaptureErrorCode::StreamBuildFailed),
        test_policy(),
    );
    let reservation = coordinator
        .reserve(owner("orb"), intent("failure", CapturePurpose::PushToTalk))
        .unwrap();

    assert_eq!(
        coordinator.begin(reservation).unwrap_err().code,
        CaptureErrorCode::StreamBuildFailed
    );
    let failed = events.lock().unwrap().last().cloned().unwrap();
    assert_eq!(failed.outcome, "start_failed");
    assert_eq!(failed.phase, CapturePhase::Idle);
    assert_eq!(failed.error, Some(CaptureErrorCode::StreamBuildFailed));
    assert_eq!(failed.last_error, failed.error);
}

#[test]
fn disabling_diagnostics_does_not_cancel_the_active_capture() {
    let (backend, _) = FakeBackend::immediate(PlayBehavior::Samples(vec![0.2; 64]));
    let coordinator = spawn_coordinator(backend, test_policy());
    coordinator.apply_diagnostics_setting(1, true).unwrap();
    let session = begin(&coordinator, owner("overlay"), "diagnostics-off");

    coordinator.apply_diagnostics_setting(2, false).unwrap();
    assert_eq!(
        coordinator.snapshot().unwrap().phase,
        CapturePhase::Recording
    );

    coordinator
        .cancel(CancelCapture {
            owner: owner("overlay"),
            intent_id: "diagnostics-off".to_owned(),
            session_id: Some(session.session_id),
            reason: "user_cancelled",
        })
        .unwrap();
}

#[test]
fn delayed_diagnostics_refresh_reads_the_latest_committed_setting() {
    let (backend, _) = FakeBackend::immediate(PlayBehavior::Samples(vec![0.2; 64]));
    let coordinator = spawn_coordinator(backend, test_policy());
    coordinator.apply_diagnostics_setting(1, true).unwrap();
    let session = begin(&coordinator, owner("overlay"), "latest-toggle");

    coordinator.inject_diagnostics_refresh();
    assert!(coordinator.diagnostics_active());
    coordinator
        .cancel(CancelCapture {
            owner: owner("overlay"),
            intent_id: "latest-toggle".to_owned(),
            session_id: Some(session.session_id),
            reason: "user_cancelled",
        })
        .unwrap();
}

#[test]
fn diagnostics_disabled_during_permission_wait_does_not_start_an_observer() {
    let (backend, _) = FakeBackend::immediate(PlayBehavior::Samples(vec![0.2; 64]));
    let coordinator = spawn_coordinator(backend, test_policy());
    coordinator.apply_diagnostics_setting(1, true).unwrap();
    let reservation = coordinator
        .reserve(
            owner("overlay"),
            intent("pending-permission", CapturePurpose::PushToTalk),
        )
        .unwrap();

    coordinator.apply_diagnostics_setting(2, false).unwrap();
    let session = coordinator.begin(reservation).unwrap();

    assert!(!coordinator.diagnostics_active());
    coordinator
        .cancel(CancelCapture {
            owner: owner("overlay"),
            intent_id: "pending-permission".to_owned(),
            session_id: Some(session.session_id),
            reason: "user_cancelled",
        })
        .unwrap();
}

// Keep: elapsed deadlines are independent of device rate and channel count.
#[test]
fn device_formats_remain_recording_until_600_seconds() {
    for (rate, channels) in [(16_000, 1), (96_000, 2), (96_000, 8)] {
        let (backend, input) = FakeBackend::continuous();
        let (capture, events) = spawn_with_events(
            backend.with_format(rate, channels),
            AudioCapturePolicy {
                minimum_capture_duration: Duration::ZERO,
                minimum_sample_count: 0,
                ..Default::default()
            },
        );
        capture.freeze_time();
        let session = begin(&capture, owner("overlay"), "format-cap");
        input.feed_samples(
            &capture,
            &vec![0.2; rate as usize * usize::from(channels) / 5],
        );
        capture.advance_time(Duration::from_secs(599));
        assert_eq!(
            capture.snapshot().unwrap().phase,
            CapturePhase::Recording,
            "{rate}/{channels}"
        );
        let progress = event_with_outcome(&events, "recording_progress");
        let json = serde_json::to_value(progress).unwrap();
        assert_eq!(json["elapsedMs"], 599_000);
        capture.advance_time(Duration::from_secs(1));
        let artifact = capture
            .stop(StopCapture {
                owner: owner("overlay"),
                intent_id: "format-cap".into(),
                session_id: session.session_id,
            })
            .unwrap();
        assert!(artifact.byte_size > 44);
    }
}

// Keep: overflow retains every accepted sample and the recording remains transcribable.
#[test]
fn pcm_pool_exhaustion_preserves_audio_through_normal_stop() {
    for initial_overflow in [false, true] {
        let (backend, input) = FakeBackend::continuous();
        let (capture, events) = spawn_with_events(
            backend,
            AudioCapturePolicy {
                pcm_headroom: PcmHeadroom::Bytes(48_000 * 4),
                ..test_policy()
            },
        );
        capture.freeze_time();
        let session = begin(&capture, owner("overlay"), "overflow");
        if !initial_overflow {
            for _ in 0..540 {
                input.feed_samples(&capture, &vec![0.2; 48_000]);
            }
            capture.advance_time(Duration::from_secs(540));
        }
        input.feed_samples(&capture, &vec![0.2; 48_000 * 3]);
        assert_eq!(capture.snapshot().unwrap().phase, CapturePhase::Recording);
        let overflow = event_with_outcome(&events, "pcm_limit");
        assert_eq!(
            overflow.sample_count,
            if initial_overflow {
                48_000
            } else {
                48_000 * 541
            }
        );
        let metadata = capture
            .stop(StopCapture {
                owner: owner("overlay"),
                intent_id: "overflow".into(),
                session_id: session.session_id,
            })
            .unwrap();
        assert_eq!(
            metadata.duration_ms,
            if initial_overflow { 1_000 } else { 541_000 }
        );
        let lease = capture
            .lease_artifact(owner("overlay"), metadata.artifact_id.clone())
            .unwrap();
        let bundle = capture
            .read_artifact(owner("overlay"), metadata.artifact_id, lease.lease_id)
            .unwrap();
        assert_eq!(
            bundle
                .segments
                .iter()
                .map(|segment| segment.duration_ms)
                .sum::<u64>(),
            metadata.duration_ms
        );
        assert!(bundle
            .segments
            .iter()
            .any(|segment| segment.bytes.is_some()));
    }
}

#[derive(Clone, Copy, Debug)]
enum StreamEventPolicy {
    /// Capture continues; the optional nonterminal event names its outcome and error.
    Continues(Option<(&'static str, Option<CaptureErrorCode>)>),
    /// Capture ends with this code, releases the microphone and drops the stream.
    Ends(CaptureErrorCode),
}

// Keep: the owner-visible policy for each CPAL error-callback kind, shared by both phases.
const STREAM_EVENT_POLICY: [(cpal::ErrorKind, StreamEventPolicy); 7] = [
    (
        cpal::ErrorKind::Xrun,
        StreamEventPolicy::Continues(Some((
            "stream_error",
            Some(CaptureErrorCode::BufferUnderrun),
        ))),
    ),
    (
        cpal::ErrorKind::DeviceChanged,
        StreamEventPolicy::Continues(Some(("audio_route_changed", None))),
    ),
    (
        cpal::ErrorKind::RealtimeDenied,
        StreamEventPolicy::Continues(None),
    ),
    (
        cpal::ErrorKind::DeviceNotAvailable,
        StreamEventPolicy::Ends(CaptureErrorCode::DeviceUnavailable),
    ),
    (
        cpal::ErrorKind::StreamInvalidated,
        StreamEventPolicy::Ends(CaptureErrorCode::StreamInvalidated),
    ),
    (
        cpal::ErrorKind::PermissionDenied,
        StreamEventPolicy::Ends(CaptureErrorCode::MicrophonePermissionDenied),
    ),
    (
        cpal::ErrorKind::BackendError,
        StreamEventPolicy::Ends(CaptureErrorCode::StreamBackendError),
    ),
];

const NOTIFICATION_OUTCOMES: [&str; 3] = ["stream_error", "audio_route_changed", "pcm_limit"];

fn assert_nonterminal_event(
    events: &[CaptureEvent],
    note: Option<(&'static str, Option<CaptureErrorCode>)>,
    phase: CapturePhase,
    case: &str,
) {
    let observed: Vec<_> = events
        .iter()
        .filter(|event| NOTIFICATION_OUTCOMES.contains(&event.outcome))
        .map(|event| (event.outcome, event.error, event.phase))
        .collect();
    let expected: Vec<_> = note
        .into_iter()
        .map(|(outcome, error)| (outcome, error, phase))
        .collect();
    assert_eq!(observed, expected, "{case}");
}

fn assert_terminal_cleanup(
    capture: &DesktopAudioCapture,
    events: &[CaptureEvent],
    drops: &std::sync::atomic::AtomicUsize,
    code: CaptureErrorCode,
    case: &str,
) {
    // Underrun warnings share the outcome; the terminal event is the last one.
    let terminal = events
        .iter()
        .rfind(|event| event.outcome == "stream_error")
        .unwrap_or_else(|| panic!("{case}: terminal stream_error event"));
    assert_eq!(terminal.phase, CapturePhase::Idle, "{case}");
    assert_eq!(terminal.error, Some(code), "{case}");
    let snapshot = capture.snapshot().unwrap();
    assert_eq!(snapshot.phase, CapturePhase::Idle, "{case}");
    assert_eq!(snapshot.last_error, Some(code), "{case}");
    assert_eq!(
        snapshot.artifact_count, 0,
        "{case}: recording bundle discarded"
    );
    assert_eq!(drops.load(Ordering::Relaxed), 1, "{case}: stream dropped");
    capture
        .reserve(owner("next"), intent("after", CapturePurpose::PushToTalk))
        .unwrap_or_else(|error| panic!("{case}: microphone ownership released: {error:?}"));
}

// Keep: AC6, AC8-AC12 while the actor waits for actual PCM.
#[test]
fn stream_notifications_awaiting_first_pcm_follow_the_event_policy() {
    for (kind, policy) in STREAM_EVENT_POLICY {
        let case = format!("{kind:?} while awaiting PCM");
        let (backend, drops) = FakeBackend::immediate(PlayBehavior::Script(vec![
            FakeCallback::Notify(kind),
            FakeCallback::Pcm(vec![0.1; 32]),
        ]));
        let (capture, events) = spawn_with_events(backend, test_policy());
        let reservation = capture
            .reserve(
                owner("overlay"),
                intent("waiting", CapturePurpose::PushToTalk),
            )
            .unwrap();
        let result = capture.begin(reservation);
        match policy {
            StreamEventPolicy::Continues(note) => {
                let session = result.unwrap_or_else(|error| panic!("{case}: {error:?}"));
                assert_eq!(session.phase, CapturePhase::Recording, "{case}");
                assert_nonterminal_event(
                    &events.lock().unwrap(),
                    note,
                    CapturePhase::WaitingForFirstCallback,
                    &case,
                );
                assert_eq!(drops.load(Ordering::Relaxed), 0, "{case}");
            }
            StreamEventPolicy::Ends(code) => {
                assert_eq!(result.unwrap_err().code, code, "{case}");
                let events = events.lock().unwrap().clone();
                assert_terminal_cleanup(&capture, &events, &drops, code, &case);
            }
        }
    }
}

// Keep: AC9 and AC14; recoverable notifications neither mark readiness nor extend the deadline.
#[test]
fn recoverable_notifications_without_pcm_time_out_at_the_original_deadline() {
    let recoverable = [
        cpal::ErrorKind::Xrun,
        cpal::ErrorKind::DeviceChanged,
        cpal::ErrorKind::RealtimeDenied,
    ];
    let script = recoverable
        .iter()
        .cycle()
        .take(12)
        .map(|kind| FakeCallback::Notify(*kind))
        .collect();
    let (backend, drops) = FakeBackend::immediate(PlayBehavior::Script(script));
    let mut policy = test_policy();
    policy.first_callback_timeout = Duration::from_millis(60);
    let capture = spawn_coordinator(backend, policy);
    let reservation = capture
        .reserve(owner("orb"), intent("no-pcm", CapturePurpose::PushToTalk))
        .unwrap();

    let started = std::time::Instant::now();
    let error = capture.begin(reservation).unwrap_err();

    assert_eq!(error.code, CaptureErrorCode::FirstCallbackTimeout);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(capture.snapshot().unwrap().phase, CapturePhase::Idle);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

// Keep: AC7-AC12 during steady capture; accepted audio survives recoverable notifications.
#[test]
fn stream_notifications_while_recording_follow_the_event_policy() {
    for (kind, policy) in STREAM_EVENT_POLICY {
        let case = format!("{kind:?} while recording");
        let (backend, input) = FakeBackend::continuous();
        let drops = backend.drops();
        let (capture, events) = spawn_with_events(backend, test_policy());
        let session = begin(&capture, owner("overlay"), "recording");
        input.feed(&capture, 0.2, 200);
        input.notify(&capture, [kind]);
        match policy {
            StreamEventPolicy::Continues(note) => {
                assert_eq!(
                    capture.snapshot().unwrap().phase,
                    CapturePhase::Recording,
                    "{case}"
                );
                assert_nonterminal_event(
                    &events.lock().unwrap(),
                    note,
                    CapturePhase::Recording,
                    &case,
                );
                input.feed(&capture, 0.2, 200);
                let artifact = capture
                    .stop(StopCapture {
                        owner: owner("overlay"),
                        intent_id: "recording".into(),
                        session_id: session.session_id,
                    })
                    .unwrap_or_else(|error| panic!("{case}: {error:?}"));
                assert_eq!(artifact.duration_ms, 400, "{case}");
                assert_eq!(drops.load(Ordering::Relaxed), 1, "{case}: stopped stream");
            }
            StreamEventPolicy::Ends(code) => {
                let recorded = events.lock().unwrap().clone();
                assert_terminal_cleanup(&capture, &recorded, &drops, code, &case);
                assert_eq!(
                    capture
                        .stop(StopCapture {
                            owner: owner("overlay"),
                            intent_id: "recording".into(),
                            session_id: session.session_id,
                        })
                        .unwrap_err()
                        .code,
                    CaptureErrorCode::SessionNotFound,
                    "{case}"
                );
            }
        }
    }
}

/// More notifications than the bounded backend event queue holds.
const WARNING_BURST: usize = 64;

fn xrun_burst() -> impl Iterator<Item = FakeCallback> {
    (0..WARNING_BURST).map(|_| FakeCallback::Notify(cpal::ErrorKind::Xrun))
}

// Keep: F1 regression; a warning burst that fills the event queue loses neither readiness
// nor the terminal error, in either capture phase.
#[test]
fn terminal_errors_and_readiness_survive_a_saturated_event_queue() {
    let pcm = || FakeCallback::Pcm(vec![0.1; 32]);
    let unavailable = || FakeCallback::Notify(cpal::ErrorKind::DeviceNotAvailable);
    let cases: [(&str, Vec<FakeCallback>, Option<CaptureErrorCode>, bool); 3] = [
        (
            "PCM, burst, device lost",
            std::iter::once(pcm())
                .chain(xrun_burst())
                .chain([unavailable()])
                .collect(),
            Some(CaptureErrorCode::DeviceUnavailable),
            true,
        ),
        (
            "burst, PCM",
            xrun_burst().chain([pcm()]).collect(),
            None,
            true,
        ),
        (
            "burst, PCM, device lost",
            xrun_burst().chain([pcm(), unavailable()]).collect(),
            Some(CaptureErrorCode::DeviceUnavailable),
            false,
        ),
    ];
    for (case, script, terminal, becomes_ready) in cases {
        let (backend, drops) = FakeBackend::immediate(PlayBehavior::Script(script));
        let mut policy = test_policy();
        policy.first_callback_timeout = Duration::from_millis(300);
        let (capture, events) = spawn_with_events(backend, policy);
        let reservation = capture
            .reserve(
                owner("overlay"),
                intent("burst", CapturePurpose::PushToTalk),
            )
            .unwrap();
        let result = capture.begin(reservation);
        if becomes_ready {
            let session = result.unwrap_or_else(|error| panic!("{case}: {error:?}"));
            assert_eq!(session.phase, CapturePhase::Recording, "{case}");
        } else {
            assert_eq!(result.unwrap_err().code, terminal.unwrap(), "{case}");
        }
        let snapshot = capture.snapshot().unwrap();
        match terminal {
            Some(code) => {
                let events = events.lock().unwrap().clone();
                assert_terminal_cleanup(&capture, &events, &drops, code, case);
            }
            None => {
                assert_eq!(snapshot.phase, CapturePhase::Recording, "{case}");
                assert_eq!(drops.load(Ordering::Relaxed), 0, "{case}");
            }
        }
    }

    let (backend, input) = FakeBackend::continuous();
    let drops = backend.drops();
    let (capture, events) = spawn_with_events(backend, test_policy());
    begin(&capture, owner("overlay"), "recording");
    input.feed(&capture, 0.2, 200);
    input.notify(
        &capture,
        std::iter::repeat_n(cpal::ErrorKind::Xrun, WARNING_BURST)
            .chain([cpal::ErrorKind::DeviceNotAvailable]),
    );
    let recorded = events.lock().unwrap().clone();
    assert_terminal_cleanup(
        &capture,
        &recorded,
        &drops,
        CaptureErrorCode::DeviceUnavailable,
        "recording, burst, device lost",
    );
}

const REFILL_FIRST_CALLBACK_TIMEOUT: Duration = Duration::from_millis(100);

/// A capture whose event sink refills the backend event queue as fast as the actor drains it.
struct RefillingCapture {
    capture: DesktopAudioCapture,
    input: FakeInput,
    drops: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    refills: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

fn refilling_capture(behavior: PlayBehavior) -> RefillingCapture {
    let mut policy = test_policy();
    policy.first_callback_timeout = REFILL_FIRST_CALLBACK_TIMEOUT;
    policy.operation_timeout = Duration::from_secs(3);
    let (backend, input) = FakeBackend::continuous_with(behavior);
    let drops = backend.drops();
    let (sink, refills) = input.refilling_sink();
    let capture = DesktopAudioCapture::start_with_backend(backend, policy, sink).unwrap();
    RefillingCapture {
        capture,
        input,
        drops,
        refills,
    }
}

// Keep: F3 regression for AC14; an event queue that never empties cannot postpone the
// first-callback deadline.
#[test]
fn a_refilled_event_queue_cannot_postpone_the_first_callback_deadline() {
    let RefillingCapture { capture, drops, .. } =
        refilling_capture(PlayBehavior::Script(xrun_burst().collect()));
    let reservation = capture
        .reserve(owner("orb"), intent("overload", CapturePurpose::PushToTalk))
        .unwrap();

    let started = std::time::Instant::now();
    let result = capture.begin(reservation);
    let elapsed = started.elapsed();

    assert_eq!(
        result.unwrap_err().code,
        CaptureErrorCode::FirstCallbackTimeout
    );
    assert!(
        elapsed < REFILL_FIRST_CALLBACK_TIMEOUT + Duration::from_millis(900),
        "timed out after {elapsed:?}"
    );
    assert_eq!(capture.snapshot().unwrap().phase, CapturePhase::Idle);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

// Keep: F3 regression; while recording, an event queue that never empties cannot keep the
// actor from answering stop, and accepted audio survives.
#[test]
fn a_refilled_event_queue_cannot_starve_commands_while_recording() {
    let RefillingCapture {
        capture,
        input,
        drops,
        refills,
    } = refilling_capture(PlayBehavior::Samples(Vec::new()));
    let session = begin(&capture, owner("overlay"), "recording");
    input.feed(&capture, 0.2, 200);
    input.report(std::iter::repeat_n(cpal::ErrorKind::Xrun, WARNING_BURST));
    // Send stop only once the actor is draining the refilled queue, not before it starts.
    let draining = std::time::Instant::now();
    while refills.load(Ordering::Relaxed) < 2 * WARNING_BURST {
        assert!(
            draining.elapsed() < Duration::from_secs(1),
            "refill stalled"
        );
        std::thread::yield_now();
    }

    let started = std::time::Instant::now();
    let result = capture.stop(StopCapture {
        owner: owner("overlay"),
        intent_id: "recording".into(),
        session_id: session.session_id,
    });
    let elapsed = started.elapsed();

    assert_eq!(result.unwrap().duration_ms, 200);
    assert!(
        elapsed < Duration::from_secs(1),
        "stop answered after {elapsed:?}"
    );
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

// Keep: AC13; an older stream's callbacks and queued events cannot touch the current capture.
#[test]
fn stale_stream_notifications_cannot_affect_a_newer_recording() {
    let (backend, input) = FakeBackend::continuous();
    let drops = backend.drops();
    let (capture, events) = spawn_with_events(backend, test_policy());
    let first = capture
        .reserve(
            owner("overlay"),
            intent("first", CapturePurpose::PushToTalk),
        )
        .unwrap();
    let stale_generation = first.generation;
    capture.begin(first).unwrap();
    let stale = input.detach();
    capture
        .cancel(CancelCapture {
            owner: owner("overlay"),
            intent_id: "first".to_owned(),
            session_id: None,
            reason: "user_cancelled",
        })
        .unwrap();
    let session = begin(&capture, owner("overlay"), "second");
    let current = events.lock().unwrap().len();

    for kind in STREAM_EVENT_POLICY.map(|(kind, _)| kind) {
        stale.notify(&capture, [kind]);
    }
    for event in [
        crate::audio::backend::BackendEvent::StreamError(
            stale_generation,
            CaptureErrorCode::DeviceUnavailable,
        ),
        crate::audio::backend::BackendEvent::StreamError(
            stale_generation,
            CaptureErrorCode::BufferUnderrun,
        ),
        crate::audio::backend::BackendEvent::DefaultInputChanged(
            stale_generation,
            "audio_route_changed",
        ),
        crate::audio::backend::BackendEvent::PcmLimitReached(stale_generation),
    ] {
        capture.inject_backend_event(event);
    }
    input.feed(&capture, 0.2, 200);
    capture.snapshot().unwrap();

    assert_eq!(capture.snapshot().unwrap().phase, CapturePhase::Recording);
    assert_nonterminal_event(
        &events.lock().unwrap()[current..],
        None,
        CapturePhase::Recording,
        "stale generation",
    );
    assert_eq!(
        drops.load(Ordering::Relaxed),
        1,
        "only the cancelled stream"
    );
    capture
        .stop(StopCapture {
            owner: owner("overlay"),
            intent_id: "second".into(),
            session_id: session.session_id,
        })
        .unwrap();
}
