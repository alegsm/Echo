use super::*;

#[test]
fn quiet_segment_results_preserve_speech_and_ordered_nonempty_partials() {
    tauri::async_runtime::block_on(async {
        let silence = SttJobError::new(
            SttJobErrorCode::ProviderSilence,
            SttJobStage::Provider,
            true,
            "Fixture silence.",
        );
        let empty = SttJobError::new(
            SttJobErrorCode::ProviderEmptyTranscript,
            SttJobStage::Provider,
            true,
            "Fixture empty transcript.",
        );
        for (name, quiet_ordinal, outcome, initial_count, count, expected_ordinals) in [
            ("silent middle", 1, Err(silence.clone()), 2, 3, vec![0, 2]),
            ("silent first", 0, Err(silence.clone()), 1, 2, vec![1]),
            ("empty middle", 1, Err(empty), 2, 3, vec![0, 2]),
            (
                "whitespace middle",
                1,
                Ok(" \n\t ".to_string()),
                2,
                3,
                vec![0, 2],
            ),
            ("silent tail", 1, Err(silence), 1, 2, vec![0]),
        ] {
            let jobs = TranscriptionJobs::default();
            let published = Arc::new(Mutex::new(Vec::new()));
            let provider = Arc::new(FakeProvider {
                outcomes: vec![(quiet_ordinal, outcome)],
                out_of_order: true,
                ..Default::default()
            });
            let run = dictation_with_publisher(&jobs, provider.clone(), published.clone());
            run.submit(&audio(initial_count, false, None)).unwrap();
            wait_until(|| provider.completed.load(Ordering::SeqCst) == usize::from(initial_count))
                .await;
            let (job, result) = run
                .release(&audio(count, true, None))
                .await
                .unwrap_or_else(|error| panic!("{name}: {error:?}"));
            let expected = expected_ordinals
                .iter()
                .map(|ordinal| format!("piece-{ordinal}"))
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(result.provider.transcript, expected, "{name}");
            assert!(provider.pair_calls.lock().unwrap().is_empty());
            assert!(provider.whole_calls.lock().unwrap().is_empty());
            assert_eq!(result.provider.provider_id, job.settings.provider_id);
            assert_eq!(
                result.provider.provider_model,
                job.settings.provider_id.model()
            );
            assert_eq!(
                provider.calls.lock().unwrap().len(),
                usize::from(count),
                "{name}"
            );
            let events = published.lock().unwrap();
            assert_eq!(
                events.iter().map(|event| event.ordinal).collect::<Vec<_>>(),
                expected_ordinals,
                "{name}"
            );
            assert!(events.iter().all(|event| !event.text.is_empty()), "{name}");
            let expected_ms: u128 = (0..count)
                .map(|ordinal| u128::from(120 - u64::from(ordinal) * 30))
                .sum();
            assert!(
                (expected_ms..=expected_ms + 30).contains(&result.provider.provider_ms),
                "{name}: aggregate provider time must be {expected_ms}..={}, observed {}",
                expected_ms + 30,
                result.provider.provider_ms
            );
        }
    });
}

#[test]
fn all_quiet_segment_results_fail_with_empty_audio_only_at_release() {
    tauri::async_runtime::block_on(async {
        for code in [
            Some(SttJobErrorCode::ProviderSilence),
            Some(SttJobErrorCode::ProviderEmptyTranscript),
            None,
        ] {
            let outcomes = (0..3)
                .map(|ordinal| {
                    let outcome = match code {
                        Some(code) => Err(SttJobError::new(
                            code,
                            SttJobStage::Provider,
                            true,
                            "Fixture quiet audio.",
                        )),
                        None => Ok(" \n\t ".to_string()),
                    };
                    (ordinal, outcome)
                })
                .collect();
            let provider = Arc::new(FakeProvider {
                outcomes,
                delay_ms: Some(1),
                ..Default::default()
            });
            let jobs = TranscriptionJobs::default();
            let published = Arc::new(Mutex::new(Vec::new()));
            let run = dictation_with_publisher(&jobs, provider.clone(), published.clone());
            run.submit(&audio(1, false, None)).unwrap();
            wait_until(|| provider.completed.load(Ordering::SeqCst) == 1).await;
            run.submit(&audio(2, false, None)).unwrap();
            wait_until(|| provider.completed.load(Ordering::SeqCst) == 2).await;
            let error = run.release(&audio(3, true, None)).await.unwrap_err();
            assert!(provider.pair_calls.lock().unwrap().is_empty());
            assert!(provider.whole_calls.lock().unwrap().is_empty());
            assert_eq!(error.code, SttJobErrorCode::InvalidAudio, "{code:?}");
            assert_eq!(error.stage, SttJobStage::Provider);
            assert!(error.retryable);
            assert_eq!(
                crate::commands::stt::errors::public_error(error.code),
                crate::services::runtime::EchoErrorCode::EmptyAudio
            );
            assert_eq!(provider.calls.lock().unwrap().len(), 3);
            assert!(published.lock().unwrap().is_empty());
        }
    });
}

#[test]
fn segment_error_retryability_preserves_queued_ordinals_under_shared_capacity() {
    tauri::async_runtime::block_on(async {
        for (code, retryable) in [
            (SttJobErrorCode::ProviderFailed, true),
            (SttJobErrorCode::ProviderTimeout, true),
            (SttJobErrorCode::ProviderCredentialRejected, false),
            (SttJobErrorCode::MissingProviderKey, false),
            (SttJobErrorCode::InvalidAudio, false),
            (SttJobErrorCode::ProviderInvalidResponse, true),
            (SttJobErrorCode::ProviderLowConfidence, true),
            (SttJobErrorCode::ProviderFailed, false),
        ] {
            let error =
                SttJobError::new(code, SttJobStage::Provider, retryable, "Fixture failure.");
            let provider = Arc::new(FakeProvider {
                outcomes: vec![(0, Err(error))],
                delay_ms: Some(1),
                ..Default::default()
            });
            let jobs = TranscriptionJobs::default();
            let mut held = Vec::new();
            for _ in 0..crate::services::transcription_jobs::MAX_ACTIVE_JOBS - 1 {
                held.push(jobs.provider_permit().await);
            }
            let published = Arc::new(Mutex::new(Vec::new()));
            let run = dictation_with_publisher(&jobs, provider.clone(), published.clone());
            run.submit(&audio(1, false, None)).unwrap();
            wait_until(|| !provider.calls.lock().unwrap().is_empty()).await;
            run.submit(&audio(6, false, None)).unwrap();
            let mut sealed = audio(7, true, None);
            sealed.metadata.duration_ms = 60_000;
            let (job, completion) = run.release(&sealed).await.unwrap();
            assert_eq!(
                completion.provider.transcript,
                "[Transcription gap] piece-1 piece-2 piece-3 piece-4 piece-5 piece-6"
            );
            assert!(completion.partial);
            assert_eq!(
                provider.calls.lock().unwrap().len(),
                7 + usize::from(retryable),
                "{code:?}"
            );
            assert_eq!(provider.peak_active.load(Ordering::SeqCst), 1);
            let statuses: Vec<_> = published
                .lock()
                .unwrap()
                .iter()
                .filter_map(|event| event.status)
                .collect();
            let mut expected = Vec::new();
            if retryable {
                expected.push(crate::services::dictation::PartialTranscriptStatus::Retrying);
            }
            expected.push(crate::services::dictation::PartialTranscriptStatus::Gap);
            assert_eq!(statuses, expected, "{code:?}");
            assert!(jobs.complete_success(&job).unwrap());
        }
    });
}

#[test]
fn short_dictation_dispatches_exactly_one_request_and_finalizes_once() {
    tauri::async_runtime::block_on(async {
        let provider = Arc::new(FakeProvider {
            wav_ordinals: true,
            ..Default::default()
        });
        let jobs = TranscriptionJobs::default();
        let current = Arc::new(Mutex::new(settings()));
        let run = dictation(&jobs, provider.clone(), current.clone());
        current.lock().unwrap().prompt = Some("at release".into());
        let (job, result) = run.release(&release_audio(1_000, 1)).await.unwrap();
        assert_eq!(result.provider.transcript, "piece-0");
        assert!(provider.whole_calls.lock().unwrap().is_empty());
        assert_eq!(provider.calls.lock().unwrap().len(), 1);
        assert_eq!(job.settings.prompt.as_deref(), Some("at release"));
        assert_eq!(
            run.finish().await.unwrap_err().code,
            SttJobErrorCode::InvalidJob
        );
    });
}

#[test]
fn long_release_sends_only_tail_with_one_delay_and_stitches_out_of_order_results() {
    tauri::async_runtime::block_on(async {
        let (response, responses) = std::sync::mpsc::channel();
        let provider = Arc::new(FakeProvider {
            delay_ms: Some(0),
            release: Some(Arc::new(Mutex::new(responses))),
            release_ordinals: Some(vec![2]),
            ..Default::default()
        });
        let jobs = TranscriptionJobs::default();
        let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
        run.submit(&audio(2, false, None)).unwrap();
        wait_until(|| provider.completed.load(Ordering::SeqCst) == 2).await;
        let mut sealed = audio(3, true, None);
        sealed.metadata.duration_ms = 60_000;
        let releasing = run.clone();
        let release = tauri::async_runtime::spawn(async move { releasing.release(&sealed).await });
        wait_until(|| provider.calls.lock().unwrap().len() == 3).await;
        assert_eq!(provider.completed.load(Ordering::SeqCst), 2);
        assert!(
            !release.inner().is_finished(),
            "release must await the tail response"
        );
        response.send(()).unwrap();
        let (_, result) = release.await.unwrap().unwrap();
        assert!(provider.whole_calls.lock().unwrap().is_empty());
        assert_eq!(result.provider.transcript, "piece-0 piece-1 piece-2");
        assert_eq!(provider.calls.lock().unwrap().len(), 3);

        let provider = Arc::new(FakeProvider {
            out_of_order: true,
            ..Default::default()
        });
        let jobs = TranscriptionJobs::default();
        let run = dictation(&jobs, provider, Arc::new(Mutex::new(settings())));
        run.submit(&audio(4, true, Some(1))).unwrap();
        let (_, result) = run.finish().await.unwrap();
        assert_eq!(result.provider.transcript, "piece-0 piece-2 piece-3");
    });
}

#[test]
fn partial_events_publish_contiguous_segment_results_once_and_in_ordinal_order() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let published = Arc::new(Mutex::new(Vec::new()));
        let run = dictation_with_publisher(
            &jobs,
            Arc::new(FakeProvider {
                out_of_order: true,
                ..Default::default()
            }),
            published.clone(),
        );
        run.submit(&audio(4, true, None)).unwrap();
        run.finish().await.unwrap();
        wait_until(|| published.lock().unwrap().len() == 4).await;
        assert_eq!(
            *published.lock().unwrap(),
            vec![
                PartialTranscriptEvent {
                    status: None,
                    dictation_id: "intent".into(),
                    ordinal: 0,
                    text: "piece-0".into()
                },
                PartialTranscriptEvent {
                    status: None,
                    dictation_id: "intent".into(),
                    ordinal: 1,
                    text: "piece-1".into()
                },
                PartialTranscriptEvent {
                    status: None,
                    dictation_id: "intent".into(),
                    ordinal: 2,
                    text: "piece-2".into()
                },
                PartialTranscriptEvent {
                    status: None,
                    dictation_id: "intent".into(),
                    ordinal: 3,
                    text: "piece-3".into()
                },
            ]
        );

        let jobs = TranscriptionJobs::default();
        let published = Arc::new(Mutex::new(Vec::new()));
        let run =
            dictation_with_publisher(&jobs, Arc::new(FakeProvider::default()), published.clone());
        run.submit(&audio(3, true, Some(1))).unwrap();
        run.finish().await.unwrap();
        wait_until(|| !published.lock().unwrap().is_empty()).await;
        assert_eq!(
            *published.lock().unwrap(),
            vec![
                PartialTranscriptEvent {
                    status: None,
                    dictation_id: "intent".into(),
                    ordinal: 0,
                    text: "piece-0".into()
                },
                PartialTranscriptEvent {
                    status: None,
                    dictation_id: "intent".into(),
                    ordinal: 2,
                    text: "piece-2".into()
                }
            ]
        );
    });
}

#[test]
fn cancellation_suppresses_partials_while_failed_ordinals_keep_later_partials() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let published = Arc::new(Mutex::new(Vec::new()));
        let mut held = Vec::new();
        for _ in 0..crate::services::transcription_jobs::MAX_ACTIVE_JOBS {
            held.push(jobs.provider_permit().await);
        }
        let run =
            dictation_with_publisher(&jobs, Arc::new(FakeProvider::default()), published.clone());
        run.submit(&audio(2, false, None)).unwrap();
        run.cancel();
        drop(held);
        assert_eq!(
            run.finish().await.unwrap_err().code,
            SttJobErrorCode::JobCanceled
        );
        assert!(published.lock().unwrap().is_empty());

        let jobs = TranscriptionJobs::default();
        let published = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(FakeProvider {
            fail: Some(1),
            out_of_order: true,
            ..Default::default()
        });
        let run = dictation_with_publisher(&jobs, provider.clone(), published.clone());
        run.submit(&audio(3, true, None)).unwrap();
        let (_, result) = run.finish().await.unwrap();
        assert_eq!(
            result.provider.transcript,
            "piece-0 [Transcription gap] piece-2"
        );
        assert_eq!(provider.calls.lock().unwrap().len(), 4);
        let text = published
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.status.is_none())
            .map(|event| event.text.clone())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(text, result.provider.transcript);
    });
}

#[test]
fn cancellation_suppresses_a_successful_inflight_partial_result() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let published = Arc::new(Mutex::new(Vec::new()));
        let (release, released) = std::sync::mpsc::channel();
        let provider = Arc::new(FakeProvider {
            delay_ms: Some(0),
            release: Some(Arc::new(Mutex::new(released))),
            ..Default::default()
        });
        let run = dictation_with_publisher(&jobs, provider.clone(), published.clone());
        run.submit(&audio(1, false, None)).unwrap();
        wait_until(|| provider.calls.lock().unwrap().len() == 1).await;
        assert_eq!(provider.completed.load(Ordering::SeqCst), 0);
        run.cancel();
        release.send(()).unwrap();
        assert_eq!(
            run.finish().await.unwrap_err().code,
            SttJobErrorCode::JobCanceled
        );
        let pending = Arc::downgrade(&run);
        drop(run);
        wait_until(|| pending.upgrade().is_none()).await;
        assert_eq!(provider.completed.load(Ordering::SeqCst), 1);
        assert!(published.lock().unwrap().is_empty());
    });
}

#[test]
fn settings_freeze_at_first_dispatch_after_capacity_becomes_available() {
    tauri::async_runtime::block_on(async {
        let provider = Arc::new(FakeProvider::default());
        let jobs = TranscriptionJobs::default();
        let mut permits = Vec::new();
        for _ in 0..crate::services::transcription_jobs::MAX_ACTIVE_JOBS {
            permits.push(jobs.provider_permit().await);
        }
        let current = Arc::new(Mutex::new(settings()));
        let run = dictation(&jobs, provider.clone(), current.clone());
        run.submit(&audio(1, false, None)).unwrap();
        current.lock().unwrap().prompt = Some("first dispatch".into());
        drop(permits.pop());
        wait_until(|| provider.calls.lock().unwrap().len() == 1).await;
        {
            let mut changed = current.lock().unwrap();
            changed.prompt = Some("later".into());
            changed.provider_id = ProviderId::XaiStt;
            changed.language = Some("es".into());
            changed.paste_after_transcription = false;
            changed.restore_clipboard = false;
        }
        drop(permits);
        run.submit(&audio(2, true, None)).unwrap();
        let (job, _) = run.finish().await.unwrap();
        assert!(job.settings.paste_after_transcription && job.settings.restore_clipboard);
        for (_, frozen) in provider.calls.lock().unwrap().iter() {
            assert_eq!(frozen.prompt.as_deref(), Some("first dispatch"));
            assert_eq!(frozen.provider_id, ProviderId::GroqWhisperTurbo);
            assert_eq!(frozen.language.as_deref(), Some("en"));
        }
    });
}

#[test]
fn cancel_discards_queued_segments_and_inflight_completions() {
    tauri::async_runtime::block_on(async {
        let provider = Arc::new(FakeProvider::default());
        let jobs = TranscriptionJobs::default();
        let mut held = Vec::new();
        for _ in 0..crate::services::transcription_jobs::MAX_ACTIVE_JOBS - 1 {
            held.push(jobs.provider_permit().await);
        }
        let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
        run.submit(&audio(6, false, None)).unwrap();
        wait_until(|| provider.calls.lock().unwrap().len() == 1).await;
        run.cancel();
        assert_eq!(
            run.finish().await.unwrap_err().code,
            SttJobErrorCode::JobCanceled
        );
        drop(held);
        wait_until(|| provider.completed.load(Ordering::SeqCst) == 1).await;
        assert_eq!(provider.calls.lock().unwrap().len(), 1);
        assert_eq!(
            run.submit(&audio(7, true, None)).unwrap_err().code,
            SttJobErrorCode::JobCanceled
        );
    });
}

#[test]
fn short_dictation_freezes_settings_at_release_even_when_capacity_is_exhausted() {
    tauri::async_runtime::block_on(async {
        let provider = Arc::new(FakeProvider::default());
        let jobs = TranscriptionJobs::default();
        let mut held = Vec::new();
        for _ in 0..crate::services::transcription_jobs::MAX_ACTIVE_JOBS {
            held.push(jobs.provider_permit().await);
        }
        let current = Arc::new(Mutex::new(settings()));
        let run = dictation(&jobs, provider, current.clone());
        let release_run = run.clone();
        let release = tauri::async_runtime::spawn(async move {
            release_run.release(&release_audio(2_000, 2)).await
        });
        wait_until(|| run.job().is_some()).await;
        current.lock().unwrap().prompt = Some("after release".into());
        drop(held);
        let (job, _) = release.await.unwrap().unwrap();
        assert_eq!(job.settings.prompt.as_deref(), Some("original"));
    });
}

#[test]
fn foreign_or_noncontiguous_bundles_are_rejected() {
    let jobs = TranscriptionJobs::default();
    let run = dictation(
        &jobs,
        Arc::new(FakeProvider::default()),
        Arc::new(Mutex::new(settings())),
    );
    let mut foreign = audio(1, false, None);
    foreign.metadata.session_id = "foreign".into();
    assert_eq!(
        run.submit(&foreign).unwrap_err().code,
        SttJobErrorCode::InvalidJob
    );
    let mut gap = audio(1, false, None);
    Arc::make_mut(&mut gap.segments)[0].ordinal = 1;
    assert_eq!(
        run.submit(&gap).unwrap_err().code,
        SttJobErrorCode::InvalidJob
    );
}

#[test]
fn cancellation_releases_queued_audio_without_waiting_for_inflight_capacity() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let mut held = Vec::new();
        for _ in 0..crate::services::transcription_jobs::MAX_ACTIVE_JOBS {
            held.push(jobs.provider_permit().await);
        }
        let provider = Arc::new(FakeProvider::default());
        let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
        let bundle = audio(2, false, None);
        let queued = Arc::downgrade(bundle.segments[0].bytes.as_ref().unwrap());
        run.submit(&bundle).unwrap();
        drop(bundle);
        run.cancel();
        wait_until(|| queued.upgrade().is_none()).await;
        assert!(provider.calls.lock().unwrap().is_empty());
        drop(held);
    });
}

#[test]
fn cancellation_while_resolving_credentials_prevents_the_http_dispatch() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let provider = Arc::new(FakeProvider {
            preflight_ms: Some(80),
            ..Default::default()
        });
        let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
        run.submit(&audio(1, false, None)).unwrap();
        wait_until(|| provider.preflights.load(Ordering::SeqCst) == 1).await;
        run.cancel();
        assert_eq!(
            run.finish().await.unwrap_err().code,
            SttJobErrorCode::JobCanceled
        );
        wait_until(|| provider.preflights_finished.load(Ordering::SeqCst) == 1).await;
        assert!(provider.calls.lock().unwrap().is_empty());
    });
}

fn ordinal_bundle(count: u32) -> NativeAudioArtifact {
    let mut bundle = audio(1, false, None);
    bundle.segments = (0..count)
        .map(|ordinal| AudioSegment {
            ordinal,
            duration_ms: 2_000,
            cut_kind: crate::audio::SegmentCutKind::QuietWindow,
            bytes: None,
        })
        .collect::<Vec<_>>()
        .into();
    bundle
}

// Merge: the ordinal boundary admits all 301 entries and rejects the next one.
#[test]
fn ten_minute_dictation_ordinal_boundary() {
    for (count, admitted) in [(301, true), (302, false)] {
        let jobs = TranscriptionJobs::default();
        let run = dictation(
            &jobs,
            Arc::new(FakeProvider::default()),
            Arc::new(Mutex::new(settings())),
        );
        let result = run.submit(&ordinal_bundle(count));
        if admitted {
            result.unwrap();
        } else {
            assert_eq!(result.unwrap_err().code, SttJobErrorCode::InvalidJob);
        }
        run.cancel();
    }
}
