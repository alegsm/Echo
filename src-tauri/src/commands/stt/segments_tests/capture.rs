use super::*;

#[test]
fn dictation_user_loop_capture_provider_history_delivery_and_failure() {
    use crate::audio::tests::fixtures::{begin, owner, spawn_coordinator, FakeBackend};
    use crate::audio::{AudioCapturePolicy, StopCapture};
    use crate::commands::stt::persistence::build_record;
    use crate::commands::stt::pipeline::{finish_provider_work, read_bundle};
    use crate::storage::EchoStorage;

    tauri::async_runtime::block_on(async {
        let directory =
            std::env::temp_dir().join(format!("echo-dictation-loop-{}", uuid::Uuid::new_v4()));
        let storage = EchoStorage::open(directory.join("echo.sqlite3")).unwrap();
        storage.initialize().unwrap();
        for (duration, failure, automatic) in [
            (1_000, None, false),
            (1_000, None, true),
            (20_200, None, false),
            (20_200, None, true),
            (20_200, Some(SttJobErrorCode::ProviderTimeout), false),
            (20_200, Some(SttJobErrorCode::ProviderSilence), false),
        ] {
            let (backend, input) = FakeBackend::continuous();
            let capture = spawn_coordinator(
                backend,
                AudioCapturePolicy {
                    minimum_capture_duration: Duration::ZERO,
                    minimum_sample_count: 0,
                    ..Default::default()
                },
            );
            capture.freeze_time();
            let owner = owner("overlay");
            let session = begin(&capture, owner.clone(), "loop");
            for _ in 0..duration / 2_500 {
                input.feed(&capture, 0.2, 2_000);
                input.feed(&capture, 0.0, 500);
            }
            input.feed(&capture, 0.2, duration % 2_500);
            if automatic {
                capture.advance_time(Duration::from_secs(600));
                assert_eq!(
                    capture.snapshot().unwrap().phase,
                    crate::audio::CapturePhase::Idle
                );
                assert_eq!(capture.snapshot().unwrap().artifact_count, 1);
            }
            let metadata = capture
                .stop(StopCapture {
                    owner: owner.clone(),
                    intent_id: "loop".into(),
                    session_id: session.session_id,
                })
                .unwrap();
            let request = JobRequest {
                owner: "overlay".into(),
                artifact_id: metadata.artifact_id.clone(),
                session_id: metadata.session_id.clone(),
                intent_id: "loop".into(),
                artifact_intent_id: "loop".into(),
                created_at: storage.current_timestamp().unwrap(),
            };
            let jobs = TranscriptionJobs::default();
            let registry = jobs.clone();
            let provider = Arc::new(FakeProvider {
                whole_outcome: failure.map(|code| Err(fixture_error(code))),
                outcomes: failure
                    .map(|code| {
                        (
                            b'R',
                            Err(SttJobError::new(
                                code,
                                SttJobStage::Provider,
                                true,
                                "Fixture provider failure.",
                            )),
                        )
                    })
                    .into_iter()
                    .collect(),
                ..Default::default()
            });
            let published = Arc::new(Mutex::new(Vec::new()));
            let partials = published.clone();
            let run = SegmentedDictation::new_with_publisher(
                request.clone(),
                jobs.clone(),
                provider.clone(),
                Arc::new(move |request| registry.admit_initial(request.clone(), settings())),
                Arc::new(move |event| partials.lock().unwrap().push(event)),
            );
            let bundle = read_bundle(&capture, &owner, &request, true).await.unwrap();
            // Each 2 s burst plus its first 500 ms quiet window closes a piece.
            let expected_pieces = duration.div_ceil(2_500);
            assert_eq!(bundle.audio.segments.len(), expected_pieces);
            let result = run.release(&bundle.audio).await;
            let job = run.job().unwrap();
            let history_before = storage.list_history(10, None).unwrap().total_count;
            let result = result.map(|(_, result)| result);
            let completion =
                finish_provider_work(&capture, &owner, &jobs, &job, bundle, result).await;
            if failure == Some(SttJobErrorCode::ProviderSilence) {
                assert_eq!(
                    completion.err().unwrap().code,
                    SttJobErrorCode::InvalidAudio
                );
                jobs.complete_failure(&job).unwrap();
                assert_eq!(
                    storage.list_history(10, None).unwrap().total_count,
                    history_before
                );
                let error = read_bundle(&capture, &owner, &request, true)
                    .await
                    .err()
                    .unwrap();
                assert_eq!(error.code, SttJobErrorCode::ArtifactExpired);
                assert_eq!(capture.snapshot().unwrap().artifact_count, 0);
            } else {
                let completion = completion.unwrap();
                let record = build_record(&job, &completion.audio, completion.provider);
                let saved = storage
                    .append_transcript(record.clone())
                    .unwrap()
                    .transcript
                    .unwrap();
                if failure.is_some() {
                    assert_eq!(saved.record.insertion_mode, "failed");
                    assert_eq!(saved.record.error_code.as_deref(), Some("provider_failed"));
                    assert_eq!(
                        saved.record.text,
                        std::iter::repeat_n("[Transcription gap]", expected_pieces)
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                    assert_eq!(
                        storage.list_history(10, None).unwrap().total_count,
                        history_before + 1
                    );
                    assert_eq!(capture.snapshot().unwrap().artifact_count, 0);
                    continue;
                }
                jobs.authorize_delivery(&job, &record.id, saved.revision)
                    .unwrap();
                assert_eq!(
                    jobs.delivery_revision("overlay", &metadata.session_id, "loop", &record.id)
                        .unwrap(),
                    saved.revision
                );
                jobs.complete_delivery("overlay", "loop").unwrap();
                assert!(jobs
                    .delivery_revision("overlay", &metadata.session_id, "loop", &record.id)
                    .is_err());
                assert_eq!(
                    storage.list_history(10, None).unwrap().total_count,
                    history_before + 1
                );
                assert!(provider.whole_calls.lock().unwrap().is_empty());
                let expected = std::iter::repeat_n("piece-82", expected_pieces)
                    .collect::<Vec<_>>()
                    .join(" ");
                assert_eq!(saved.record.text, expected);
                assert_eq!(provider.calls.lock().unwrap().len(), expected_pieces);
                assert_eq!(capture.snapshot().unwrap().artifact_count, 0);
                assert!(run.finish().await.is_err());
            }
        }
        drop(storage);
        std::fs::remove_dir_all(directory).unwrap();
    });
}

#[test]
fn capture_accepts_callbacks_while_a_closed_native_segment_waits_for_its_provider() {
    use crate::audio::tests::fixtures::{begin, owner, spawn_coordinator, FakeBackend};
    use crate::audio::{AudioCapturePolicy, StopCapture};

    struct HoldingProvider {
        calls: AtomicUsize,
        releases: Arc<Mutex<std::sync::mpsc::Receiver<()>>>,
    }
    impl SegmentProvider for HoldingProvider {
        fn transcribe(
            &self,
            settings: FrozenJobSettings,
            bytes: Arc<[u8]>,
            _can_dispatch: crate::services::dictation::DispatchGate,
        ) -> Pin<Box<dyn Future<Output = Result<TranscriptionResult, SttJobError>> + Send + '_>>
        {
            Box::pin(async move {
                assert_eq!(&bytes[..4], b"RIFF");
                self.calls.fetch_add(1, Ordering::SeqCst);
                let releases = Arc::clone(&self.releases);
                tauri::async_runtime::spawn_blocking(move || {
                    releases.lock().unwrap().recv().unwrap()
                })
                .await
                .unwrap();
                Ok(TranscriptionResult {
                    edges: Some(confident_edges()),
                    transcript: "speech".into(),
                    provider_id: settings.provider_id,
                    provider_model: settings.provider_id.model(),
                    provider_ms: 1,
                })
            })
        }
        fn transcribe_pair(
            &self,
            settings: FrozenJobSettings,
            audio: Arc<[u8]>,
            can_dispatch: crate::services::dictation::DispatchGate,
        ) -> Pin<Box<dyn Future<Output = Result<TranscriptionResult, SttJobError>> + Send + '_>>
        {
            self.transcribe(settings, audio, can_dispatch)
        }
        fn transcribe_whole(
            &self,
            settings: FrozenJobSettings,
            audio: NativeAudioArtifact,
            can_dispatch: crate::services::dictation::DispatchGate,
        ) -> Pin<Box<dyn Future<Output = Result<TranscriptionResult, SttJobError>> + Send + '_>>
        {
            self.transcribe(settings, audio.whole_wav(), can_dispatch)
        }
    }

    tauri::async_runtime::block_on(async {
        let (backend, input) = FakeBackend::continuous();
        let capture = spawn_coordinator(
            backend,
            AudioCapturePolicy {
                minimum_capture_duration: Duration::ZERO,
                minimum_sample_count: 0,
                ..Default::default()
            },
        );
        let owner = owner("overlay");
        let session = begin(&capture, owner.clone(), "intent");
        input.feed(&capture, 0.2, 2_000);
        let closed = input.feed(&capture, 0.0, 500);
        assert_eq!(closed.len(), 1);
        let (release, releases) = std::sync::mpsc::channel();
        let provider = Arc::new(HoldingProvider {
            calls: AtomicUsize::new(0),
            releases: Arc::new(Mutex::new(releases)),
        });
        let jobs = TranscriptionJobs::default();
        let mut held = Vec::new();
        for _ in 0..crate::services::transcription_jobs::MAX_ACTIVE_JOBS {
            held.push(jobs.provider_permit().await);
        }
        let registry = jobs.clone();
        let run = SegmentedDictation::new(
            request(),
            jobs,
            provider.clone(),
            Arc::new(move |request| registry.admit_initial(request.clone(), settings())),
        );
        // Fixture identity isolates the service seam; the WAVs come from the real capture actor.
        let mut bundle = audio(1, false, None);
        bundle.segments = closed.into();
        run.submit(&bundle).unwrap();
        let before = capture.snapshot().unwrap().callback_count;
        input.feed(&capture, 0.3, 1_000);
        let queued = capture.snapshot().unwrap();
        assert_eq!(queued.phase, crate::audio::CapturePhase::Recording);
        assert_eq!(queued.callback_count, before + 50);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        drop(held);
        wait_until(|| provider.calls.load(Ordering::SeqCst) == 1).await;
        let before = capture.snapshot().unwrap().callback_count;
        input.feed(&capture, 0.3, 1_000);
        let after = capture.snapshot().unwrap();
        assert_eq!(after.phase, crate::audio::CapturePhase::Recording);
        assert_eq!(after.callback_count, before + 50);
        let metadata = capture
            .stop(StopCapture {
                owner: owner.clone(),
                intent_id: "intent".into(),
                session_id: session.session_id,
            })
            .unwrap();
        let mut actual_request = request();
        actual_request.artifact_id = metadata.artifact_id;
        actual_request.session_id = metadata.session_id;
        let stored =
            crate::commands::stt::pipeline::read_bundle(&capture, &owner, &actual_request, true)
                .await
                .unwrap();
        assert_eq!(stored.audio.segments[0].bytes, bundle.segments[0].bytes);
        bundle.is_complete = true;
        bundle.segments = stored.audio.segments;
        run.submit(&bundle).unwrap();
        release.send(()).unwrap();
        release.send(()).unwrap();
        let (_, text) = run.finish().await.unwrap();
        assert_eq!(text.provider.transcript, "speech speech");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    });
}
