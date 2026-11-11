use super::fixtures::*;
use crate::audio::*;
use std::time::Duration;

fn speech_capture() -> PlayBehavior {
    PlayBehavior::Samples(vec![0.2; 48_000 * 300 / 1_000])
}

#[test]
fn capture_artifact_is_16khz_mono_while_session_keeps_device_format() {
    install_device_config_logger();
    for channels in [1, 2] {
        check_normalized_capture(channels);
    }
}

struct DeviceChannels {
    backend: FakeBackend,
    channels: u16,
}

impl crate::audio::backend::CaptureBackend for DeviceChannels {
    type Stream = super::fixtures::FakeStream;

    fn open(
        &mut self,
        buffer: crate::audio::backend::CaptureBuffer,
    ) -> Result<crate::audio::backend::OpenedCapture<Self::Stream>, CaptureError> {
        let mut opened = self.backend.open(buffer)?;
        opened.channel_count = self.channels;
        Ok(opened)
    }

    fn play(&mut self, stream: &Self::Stream) -> Result<(), CaptureError> {
        self.backend.play(stream)
    }
}

fn check_normalized_capture(channels: u16) {
    let mut samples = vec![0.25; 480_000 * usize::from(channels)];
    // After 2 s, a complete 500 ms quiet window closes the first piece at 5 s.
    samples[216_000 * usize::from(channels)..240_000 * usize::from(channels)].fill(0.0);
    let (backend, _) = FakeBackend::immediate(PlayBehavior::Samples(samples));
    let policy = AudioCapturePolicy {
        pcm_headroom: PcmHeadroom::Bytes(4 * 1024 * 1024),
        ..test_policy()
    };
    let coordinator = DesktopAudioCapture::start_with_backend(
        DeviceChannels { backend, channels },
        policy,
        std::sync::Arc::new(|_| {}),
    )
    .unwrap();
    coordinator.apply_diagnostics_setting(1, true).unwrap();
    let capture_owner = owner("overlay");
    let session = begin(&coordinator, capture_owner.clone(), "normalized");
    assert_eq!(session.sample_rate, 48_000);
    assert_eq!(session.channel_count, channels);
    let config = device_config(&session.session_id);
    assert_eq!(config["fields"]["sampleRate"], 48_000);
    assert_eq!(config["fields"]["channelCount"], channels);
    let artifact = coordinator
        .stop(StopCapture {
            owner: capture_owner.clone(),
            intent_id: "normalized".to_owned(),
            session_id: session.session_id,
        })
        .unwrap();
    assert_eq!(artifact.sample_rate, 16_000);
    assert_eq!(artifact.channel_count, 1);
    // The quiet boundary and release produce two WAVs, each with a 44-byte header.
    assert_eq!(artifact.byte_size, 320_088);
    let lease = coordinator
        .lease_artifact(capture_owner.clone(), artifact.artifact_id.clone())
        .unwrap();
    let audio = coordinator
        .read_artifact(capture_owner, artifact.artifact_id, lease.lease_id)
        .unwrap();
    assert_eq!(audio.whole_wav().len(), 320_044);
    assert_eq!(
        u32::from_le_bytes(audio.whole_wav()[24..28].try_into().unwrap()),
        16_000
    );
    assert_eq!(
        u16::from_le_bytes(audio.whole_wav()[22..24].try_into().unwrap()),
        1
    );
}

#[test]
fn stop_creates_opaque_artifact_with_shared_leases() {
    let (backend, _) = FakeBackend::immediate(speech_capture());
    let coordinator = spawn_coordinator(backend, test_policy());
    let capture_owner = owner("overlay");
    let session = begin(&coordinator, capture_owner.clone(), "dictation");
    let artifact = coordinator
        .stop(StopCapture {
            owner: capture_owner.clone(),
            intent_id: "dictation".to_owned(),
            session_id: session.session_id,
        })
        .unwrap();
    let first = coordinator
        .lease_artifact(capture_owner.clone(), artifact.artifact_id.clone())
        .unwrap();
    let second = coordinator
        .lease_artifact(capture_owner.clone(), artifact.artifact_id.clone())
        .unwrap();

    let audio = coordinator
        .read_artifact(
            capture_owner.clone(),
            artifact.artifact_id.clone(),
            first.lease_id.clone(),
        )
        .unwrap();
    assert_eq!(&audio.whole_wav()[..4], b"RIFF");
    coordinator
        .release_artifact(ArtifactReleaseRequest {
            owner: capture_owner.clone(),
            artifact_id: artifact.artifact_id.clone(),
            lease_id: first.lease_id,
            disposition: ArtifactReleaseDisposition::Discard,
        })
        .unwrap();
    coordinator
        .read_artifact(
            capture_owner.clone(),
            artifact.artifact_id.clone(),
            second.lease_id.clone(),
        )
        .unwrap();
    assert_eq!(
        coordinator
            .lease_artifact(capture_owner.clone(), artifact.artifact_id.clone())
            .unwrap_err()
            .code,
        CaptureErrorCode::ArtifactExpired
    );
    coordinator
        .release_artifact(ArtifactReleaseRequest {
            owner: capture_owner,
            artifact_id: artifact.artifact_id,
            lease_id: second.lease_id,
            disposition: ArtifactReleaseDisposition::Discard,
        })
        .unwrap();
}

#[test]
fn artifact_expiry_and_owner_exit_cleanup_need_no_later_capture() {
    let (backend, _) = FakeBackend::immediate(speech_capture());
    let mut policy = test_policy();
    policy.artifact_ttl = Duration::from_millis(25);
    let coordinator = spawn_coordinator(backend, policy);
    coordinator.freeze_time();
    let capture_owner = owner("overlay");
    let session = begin(&coordinator, capture_owner.clone(), "expiring");
    coordinator
        .stop(StopCapture {
            owner: capture_owner.clone(),
            intent_id: "expiring".to_owned(),
            session_id: session.session_id,
        })
        .unwrap();
    coordinator.advance_time(Duration::from_millis(60));
    assert_eq!(coordinator.snapshot().unwrap().artifact_count, 0);

    coordinator.cleanup_owner(capture_owner).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().phase, CapturePhase::Idle);
}

#[test]
fn expired_lease_cannot_be_revived_on_release() {
    let (backend, _) = FakeBackend::immediate(speech_capture());
    let mut policy = test_policy();
    policy.lease_ttl = Duration::from_millis(15);
    let coordinator = spawn_coordinator(backend, policy);
    coordinator.freeze_time();
    let capture_owner = owner("overlay");
    let session = begin(&coordinator, capture_owner.clone(), "lease-expiry");
    let artifact = coordinator
        .stop(StopCapture {
            owner: capture_owner.clone(),
            intent_id: "lease-expiry".to_owned(),
            session_id: session.session_id,
        })
        .unwrap();
    let lease = coordinator
        .lease_artifact(capture_owner.clone(), artifact.artifact_id.clone())
        .unwrap();
    coordinator.advance_time(Duration::from_millis(30));

    assert_eq!(
        coordinator
            .release_artifact(ArtifactReleaseRequest {
                owner: capture_owner,
                artifact_id: artifact.artifact_id,
                lease_id: lease.lease_id,
                disposition: ArtifactReleaseDisposition::RetainUntilIdleExpiry,
            })
            .unwrap_err()
            .code,
        CaptureErrorCode::ArtifactExpired
    );
}

#[test]
fn events_publish_the_current_post_transition_snapshot() {
    let backend = FakeBackend::sequence([speech_capture(), speech_capture()]);
    let mut policy = test_policy();
    policy.artifact_ttl = Duration::from_millis(20);
    let (coordinator, events) = spawn_with_events(backend, policy);
    coordinator.freeze_time();
    let capture_owner = owner("overlay");
    let first = begin(&coordinator, capture_owner.clone(), "first-event");
    coordinator
        .stop(StopCapture {
            owner: capture_owner.clone(),
            intent_id: "first-event".to_owned(),
            session_id: first.session_id,
        })
        .unwrap();
    let artifact_event = event_with_outcome(&events, "artifact_created");
    assert_eq!(artifact_event.phase, CapturePhase::Idle);
    assert_eq!(artifact_event.artifact_count, 1);
    assert_eq!(artifact_event.intent_id.as_deref(), Some("first-event"));

    let second_owner = owner("overlay");
    let second = begin(&coordinator, second_owner.clone(), "second-event");
    coordinator.advance_time(Duration::from_millis(50));
    let expiry_event = event_with_outcome(&events, "artifact_expired");
    assert_eq!(expiry_event.phase, CapturePhase::Recording);
    assert_eq!(expiry_event.intent_id.as_deref(), Some("second-event"));
    assert_eq!(expiry_event.artifact_count, 1);

    coordinator
        .cancel(CancelCapture {
            owner: second_owner,
            intent_id: "second-event".to_owned(),
            session_id: Some(second.session_id),
            reason: "user_cancelled",
        })
        .unwrap();
    coordinator.advance_time(Duration::from_millis(20));
    let cancel_event = events.lock().unwrap().last().cloned().unwrap();
    assert_eq!(cancel_event.phase, CapturePhase::Idle);
    assert_eq!(cancel_event.intent_id.as_deref(), Some("second-event"));
    assert!(cancel_event.artifact_id.is_some());
}

#[test]
fn reserving_a_new_capture_preserves_an_older_artifact_until_lease() {
    let backend = FakeBackend::sequence([speech_capture(), PlayBehavior::NoCallback]);
    let coordinator = spawn_coordinator(backend, test_policy());
    let capture_owner = owner("overlay");
    let first = begin(&coordinator, capture_owner.clone(), "artifact-a");
    let artifact = coordinator
        .stop(StopCapture {
            owner: capture_owner.clone(),
            intent_id: "artifact-a".to_owned(),
            session_id: first.session_id,
        })
        .unwrap();
    coordinator
        .reserve(
            capture_owner.clone(),
            intent("capture-b", CapturePurpose::PushToTalk),
        )
        .unwrap();

    assert_eq!(artifact.intent_id, "artifact-a");
    let _lease = coordinator
        .lease_artifact(capture_owner.clone(), artifact.artifact_id.clone())
        .unwrap();
    coordinator
        .cancel(CancelCapture {
            owner: capture_owner,
            intent_id: "capture-b".to_owned(),
            session_id: None,
            reason: "user_cancelled",
        })
        .unwrap();
}

#[test]
fn normalized_size_admits_device_pcm_above_artifact_cap() {
    let (backend, _) = FakeBackend::immediate(PlayBehavior::Samples(vec![0.25; 48_000]));
    let policy = AudioCapturePolicy {
        maximum_artifact_bytes: 32_100,
        ..test_policy()
    };
    let coordinator = spawn_coordinator(backend, policy);
    let session = begin(&coordinator, owner("overlay"), "normalized-cap");
    let artifact = coordinator
        .stop(StopCapture {
            owner: owner("overlay"),
            intent_id: "normalized-cap".into(),
            session_id: session.session_id,
        })
        .expect("one second normalizes below the cap");
    assert_eq!(artifact.byte_size, 32_044);
}
