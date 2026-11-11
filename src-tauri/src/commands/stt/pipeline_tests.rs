use std::sync::Arc;

use crate::audio::{AudioArtifactMetadata, AudioSegment, CapturePurpose, NativeAudioArtifact};
use crate::services::dictation::JobRequest;

use super::artifact_matches_job;

#[test]
fn artifact_provenance_requires_artifact_session_and_original_intent() {
    let request = request();
    let audio = artifact();
    assert!(artifact_matches_job(&audio, &request));

    let mut incomplete = audio.clone();
    incomplete.is_complete = false;
    assert!(!artifact_matches_job(&incomplete, &request));

    let mut wrong_artifact = audio.clone();
    wrong_artifact.metadata.artifact_id = "other-artifact".to_string();
    assert!(!artifact_matches_job(&wrong_artifact, &request));

    let mut wrong_session = audio.clone();
    wrong_session.metadata.session_id = "other-session".to_string();
    assert!(!artifact_matches_job(&wrong_session, &request));

    let mut wrong_intent = audio.clone();
    wrong_intent.metadata.intent_id = "other-intent".to_string();
    assert!(!artifact_matches_job(&wrong_intent, &request));
}

fn request() -> JobRequest {
    JobRequest {
        owner: "overlay".to_string(),
        artifact_id: "artifact-1".to_string(),
        session_id: "session-1".to_string(),
        intent_id: "capture-intent".to_string(),
        artifact_intent_id: "capture-intent".to_string(),
        created_at: "2026-09-09T12:00:00Z".to_string(),
    }
}

fn artifact() -> NativeAudioArtifact {
    NativeAudioArtifact {
        stopped_at: Some(std::time::Instant::now()),
        is_complete: true,
        metadata: AudioArtifactMetadata {
            artifact_id: "artifact-1".to_string(),
            session_id: "session-1".to_string(),
            intent_id: "capture-intent".to_string(),
            purpose: CapturePurpose::PushToTalk,
            mime_type: "audio/wav",
            byte_size: 4,
            duration_ms: 800,
            sample_rate: 16_000,
            channel_count: 1,
        },
        segments: Arc::from([AudioSegment {
            cut_kind: crate::audio::SegmentCutKind::ReleaseTail,
            ordinal: 0,
            duration_ms: 800,
            bytes: Some(Arc::from([0_u8; 4])),
        }]),
    }
}

#[test]
fn release_builds_one_whole_request_from_short_capture_or_multiple_segments() {
    use super::{build_transcription_request, read_bundle};
    use crate::audio::tests::fixtures::*;
    use crate::audio::{ArtifactReleaseDisposition, ArtifactReleaseRequest, StopCapture};
    use crate::providers::ProviderId;
    use crate::services::dictation::ActiveJob;

    for duration_ms in [1_000, 18_100] {
        let (backend, input) = FakeBackend::continuous();
        let policy = crate::audio::AudioCapturePolicy {
            minimum_capture_duration: std::time::Duration::ZERO,
            minimum_sample_count: 0,
            ..Default::default()
        };
        let capture = spawn_coordinator(backend, policy);
        let owner = owner("overlay");
        let session = begin(&capture, owner.clone(), "release-request");
        if duration_ms > 10_000 {
            // After 2 s, each 500 ms quiet window closes a piece; release seals the tail.
            for _ in 0..2 {
                input.feed(&capture, 0.2, 4_500);
                input.feed(&capture, 0.0, 500);
            }
            input.feed(&capture, 0.3, duration_ms - 10_000);
        } else {
            input.feed(&capture, 0.2, duration_ms);
        }
        let metadata = capture
            .stop(StopCapture {
                owner: owner.clone(),
                intent_id: "release-request".into(),
                session_id: session.session_id.clone(),
            })
            .unwrap();
        let job = ActiveJob {
            job_sequence: 1,
            request: JobRequest {
                owner: owner.as_str().into(),
                artifact_id: metadata.artifact_id.clone(),
                session_id: session.session_id,
                intent_id: "release-request".into(),
                artifact_intent_id: "release-request".into(),
                created_at: String::new(),
            },
            settings: super::super::settings::freeze_provider_settings(
                &crate::storage::PersistedSettings {
                    custom_prompt: Some("Fixture prompt".into()),
                    phrasebook_terms: vec!["Sample".into()],
                    language: Some("en".into()),
                    ..Default::default()
                },
                ProviderId::GroqWhisperTurbo,
            )
            .unwrap(),
        };
        let artifact =
            tauri::async_runtime::block_on(read_bundle(&capture, &owner, &job.request, true))
                .unwrap();
        assert_eq!(
            artifact.audio.segments.len(),
            if duration_ms == 1_000 { 1 } else { 3 }
        );
        let request = build_transcription_request(&artifact.audio, &job.settings);
        assert_eq!(&request.audio[..4], b"RIFF");
        assert_eq!(request.audio.len(), 44 + duration_ms * 32);
        assert_eq!(
            u32::from_le_bytes(request.audio[40..44].try_into().unwrap()) as usize,
            duration_ms * 32
        );
        assert_eq!(
            u32::from_le_bytes(request.audio[4..8].try_into().unwrap()) as usize,
            request.audio.len() - 8
        );
        assert_eq!(
            i16::from_le_bytes(request.audio[44..46].try_into().unwrap()),
            (0.2 * f32::from(i16::MAX)) as i16
        );
        assert_eq!(
            i16::from_le_bytes(request.audio[request.audio.len() - 2..].try_into().unwrap()),
            ((if duration_ms > 10_000 { 0.3 } else { 0.2 }) * f32::from(i16::MAX)) as i16
        );
        assert_eq!(request.language.as_deref(), Some("en"));
        assert_eq!(request.prompt, job.settings.prompt);
        assert_eq!(request.keyterms, vec!["Sample"]);
        assert_eq!(request.file_name, "echo-dictation.wav");
        assert!(request
            .prompt
            .as_deref()
            .unwrap()
            .ends_with("This is English dictation for software and product work. Use these exact spellings when heard: Sample."));
        assert_eq!(request.mime_type, "audio/wav");
        capture
            .release_artifact(ArtifactReleaseRequest {
                owner,
                artifact_id: metadata.artifact_id,
                lease_id: artifact.lease_id,
                disposition: ArtifactReleaseDisposition::Discard,
            })
            .unwrap();
        assert_eq!(capture.snapshot().unwrap().artifact_count, 0);
    }
}

#[test]
fn segments_pair_wav_rejects_missing_silent_or_nonadjacent_pieces() {
    let mut audio = artifact();
    let wav: Arc<[u8]> = crate::audio::encode_wav(&[0.2; 16], 16_000, 1).into();
    let mut pieces = Vec::new();
    for ordinal in 0..3 {
        pieces.push(AudioSegment {
            ordinal,
            duration_ms: 1,
            bytes: Some(wav.clone()),
            cut_kind: crate::audio::SegmentCutKind::QuietWindow,
        });
    }
    audio.segments = pieces.clone().into();
    assert_eq!(audio.pair_wav(1).unwrap().len(), 44 + 64);
    assert!(audio.pair_wav(2).is_none());
    pieces[1].bytes = None;
    audio.segments = pieces.clone().into();
    assert!(audio.pair_wav(0).is_none());
    assert!(audio.pair_wav(1).is_none());
    pieces[1].bytes = Some(wav);
    pieces[1].ordinal = 9;
    audio.segments = pieces.into();
    assert!(audio.pair_wav(0).is_none());
}

// Keep: the native stop entry survives artifact leases before provider work starts.
#[test]
fn native_stop_entry_is_retained_in_the_sealed_artifact() {
    use crate::audio::tests::fixtures::*;
    use crate::audio::{AudioCapturePolicy, StopCapture};
    let (backend, input) = FakeBackend::continuous();
    let capture = spawn_coordinator(
        backend,
        AudioCapturePolicy {
            minimum_capture_duration: std::time::Duration::ZERO,
            minimum_sample_count: 0,
            ..Default::default()
        },
    );
    let owner = owner("overlay");
    let session = begin(&capture, owner.clone(), "clock");
    input.feed(&capture, 0.2, 1_000);
    let before = std::time::Instant::now();
    let metadata = capture
        .stop(StopCapture {
            owner: owner.clone(),
            intent_id: "clock".into(),
            session_id: session.session_id,
        })
        .unwrap();
    let after = std::time::Instant::now();
    let lease = capture
        .lease_artifact(owner.clone(), metadata.artifact_id.clone())
        .unwrap();
    let artifact = capture
        .read_artifact(
            owner.clone(),
            metadata.artifact_id.clone(),
            lease.lease_id.clone(),
        )
        .unwrap();
    let stop = artifact
        .stopped_at
        .expect("sealed artifacts retain stop entry");
    assert!(stop >= before && stop <= after);
    let again = capture
        .read_artifact(owner, metadata.artifact_id, lease.lease_id)
        .unwrap();
    assert_eq!(again.stopped_at, Some(stop));
}

// Keep: partial history owns provider_failed, while cleanup failure reaches diagnostics.
#[test]
fn partial_completion_keeps_provider_error_and_reports_capture_cleanup_warning() {
    use crate::commands::stt::persistence::{build_record, completion_warning};
    use crate::providers::{ProviderId, TranscriptionResult};
    use crate::services::dictation::{ActiveJob, DictationCompletion};
    use crate::services::runtime::EchoErrorCode;
    use std::sync::Mutex;

    const CHILD: &str = "ECHO_TEST_CLEANUP_WARNING_CHILD";
    if std::env::var_os(CHILD).is_none() {
        assert!(std::process::Command::new(std::env::current_exe().unwrap())
            .env(CHILD, "1")
            .args(["--exact", "commands::stt::pipeline::tests::partial_completion_keeps_provider_error_and_reports_capture_cleanup_warning", "--nocapture"])
            .status().unwrap().success());
        return;
    }
    struct Diagnostics(Mutex<Vec<String>>);
    static DIAGNOSTICS: Diagnostics = Diagnostics(Mutex::new(Vec::new()));
    impl log::Log for Diagnostics {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, record: &log::Record<'_>) {
            self.0.lock().unwrap().push(record.args().to_string());
        }
        fn flush(&self) {}
    }
    log::set_logger(&DIAGNOSTICS).unwrap();
    log::set_max_level(log::LevelFilter::Warn);
    let job = ActiveJob {
        job_sequence: 1,
        request: request(),
        settings: super::super::settings::freeze_provider_settings(
            &crate::storage::PersistedSettings::default(),
            ProviderId::GroqWhisperTurbo,
        )
        .unwrap(),
    };
    let row = build_record(
        &job,
        &artifact().metadata,
        DictationCompletion {
            provider: TranscriptionResult {
                transcript: "speech [Transcription gap]".into(),
                provider_id: ProviderId::GroqWhisperTurbo,
                provider_model: ProviderId::GroqWhisperTurbo.model(),
                provider_ms: 1,
                edges: None,
            },
            partial: true,
            has_speech: true,
            credential_rejected: false,
            stop_to_transcript_ms: 1,
        },
    );
    assert_eq!(row.error_code.as_deref(), Some("provider_failed"));
    for (saved, expected) in [
        (true, EchoErrorCode::ProviderFailed),
        (false, EchoErrorCode::HistoryWriteFailed),
    ] {
        DIAGNOSTICS.0.lock().unwrap().clear();
        assert_eq!(
            completion_warning(
                saved,
                row.error_code.is_some(),
                Some(EchoErrorCode::CaptureFailed)
            ),
            Some(expected)
        );
        assert_eq!(
            *DIAGNOSTICS.0.lock().unwrap(),
            vec!["capture_cleanup_failed code=capture_failed"]
        );
    }
    DIAGNOSTICS.0.lock().unwrap().clear();
    assert_eq!(completion_warning(true, false, None), None);
    assert!(DIAGNOSTICS.0.lock().unwrap().is_empty());
}
