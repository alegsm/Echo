use super::*;

#[test]
fn segments_cancel_during_whole_request_delivers_no_fallback() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let (response, responses) = std::sync::mpsc::channel();
        let provider = Arc::new(FakeProvider {
            whole_outcome: Some(Err(SttJobError::new(
                SttJobErrorCode::ProviderTimeout,
                SttJobStage::Provider,
                true,
                "Fixture timeout.",
            ))),
            whole_release: Some(Arc::new(Mutex::new(responses))),
            fail: Some(0),
            wav_ordinals: true,
            delay_ms: Some(1),
            ..Default::default()
        });
        let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
        let release_run = run.clone();
        let release = tauri::async_runtime::spawn(async move {
            release_run.release(&release_audio(59_999, 1)).await
        });
        wait_until(|| !provider.whole_calls.lock().unwrap().is_empty()).await;
        run.cancel();
        assert_eq!(
            release.await.unwrap().unwrap_err().code,
            SttJobErrorCode::JobCanceled
        );
        response.send(()).unwrap();
        assert_eq!(provider.calls.lock().unwrap().len(), 2);
    });
}

// Keep: strict short recovery replaces a partial only on usable whole audio.
#[test]
fn segments_whole_success_or_failure_retains_partial_at_duration_boundary() {
    tauri::async_runtime::block_on(async {
        for duration in [59_999, 60_000] {
            for failed_ordinal in [0, 1] {
                for whole_succeeds in [true, false] {
                    let jobs = TranscriptionJobs::default();
                    let provider = Arc::new(FakeProvider {
                        wav_ordinals: true,
                        delay_ms: Some(1),
                        fail: Some(failed_ordinal),
                        whole_outcome: Some(if whole_succeeds {
                            Ok("recovered whole text".into())
                        } else {
                            Err(fixture_error(SttJobErrorCode::ProviderLowConfidence))
                        }),
                        ..Default::default()
                    });
                    let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
                    let (_, result) = run.release(&release_audio(duration, 2)).await.unwrap();
                    if duration < 60_000 && whole_succeeds {
                        assert_eq!(result.provider.transcript, "recovered whole text");
                        assert!(!result.partial);
                    } else {
                        let expected = if failed_ordinal == 0 {
                            "[Transcription gap] piece-1"
                        } else {
                            "piece-0 [Transcription gap]"
                        };
                        assert_eq!(result.provider.transcript, expected);
                        assert!(result.partial);
                    }
                    assert_eq!(
                        provider.whole_calls.lock().unwrap().len(),
                        usize::from(duration < 60_000)
                    );
                    assert_eq!(provider.calls.lock().unwrap().len(), 3);
                }
            }
        }
    });
}

#[test]
fn segments_cutoff_counts_signal_gated_duration() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let provider = Arc::new(FakeProvider {
            wav_ordinals: true,
            delay_ms: Some(1),
            whole_outcome: Some(Ok("wrong path".into())),
            ..Default::default()
        });
        let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
        let mut bundle = release_audio(60_000, 2);
        let mut pieces = bundle.segments.to_vec();
        pieces[0].duration_ms = 59_000;
        pieces[1].duration_ms = 1_000;
        pieces[1].bytes = None;
        bundle.segments = pieces.into();
        let (_, result) = run.release(&bundle).await.unwrap();
        assert_eq!(result.provider.transcript, "piece-0");
        assert!(provider.whole_calls.lock().unwrap().is_empty());
    });
}

#[test]
fn segments_cancel_during_final_chunk_delivers_nothing() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let (response, responses) = std::sync::mpsc::channel();
        let provider = Arc::new(FakeProvider {
            release: Some(Arc::new(Mutex::new(responses))),
            delay_ms: Some(1),
            whole_outcome: Some(Err(SttJobError::new(
                SttJobErrorCode::ProviderTimeout,
                SttJobStage::Provider,
                true,
                "Fixture timeout.",
            ))),
            ..Default::default()
        });
        let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
        let release_run = run.clone();
        let release = tauri::async_runtime::spawn(async move {
            release_run.release(&release_audio(59_999, 1)).await
        });
        wait_until(|| !provider.calls.lock().unwrap().is_empty()).await;
        run.cancel();
        assert_eq!(
            release.await.unwrap().unwrap_err().code,
            SttJobErrorCode::JobCanceled
        );
        response.send(()).unwrap();
        assert!(provider.whole_calls.lock().unwrap().is_empty());
    });
}

#[test]
fn segments_artifact_failure_while_whole_waits_for_capacity_stays_terminal() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let mut permits = Vec::new();
        for _ in 0..6 {
            permits.push(jobs.provider_permit().await);
        }
        let provider = Arc::new(FakeProvider {
            whole_outcome: Some(Ok("must not dispatch".into())),
            ..Default::default()
        });
        let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
        let release_run = run.clone();
        let release = tauri::async_runtime::spawn(async move {
            release_run.release(&release_audio(59_999, 1)).await
        });
        wait_until(|| run.job().is_some()).await;
        let failure = SttJobError::new(
            SttJobErrorCode::ArtifactExpired,
            SttJobStage::Artifact,
            false,
            "Fixture artifact expired.",
        );
        run.fail(failure.clone());
        assert_eq!(release.await.unwrap().unwrap_err(), failure);
        assert!(provider.calls.lock().unwrap().is_empty());
        assert!(provider.whole_calls.lock().unwrap().is_empty());
    });
}

#[test]
fn segments_release_stitches_successful_chunks_at_every_duration_without_whole_audio() {
    tauri::async_runtime::block_on(async {
        for duration in [59_000, 59_999, 60_000, 61_000] {
            let jobs = TranscriptionJobs::default();
            let provider = Arc::new(FakeProvider {
                wav_ordinals: true,
                delay_ms: Some(1),
                outcomes: vec![(0, Ok("...Complete cleaned stitch…".into()))],
                whole_outcome: Some(Ok("must not request".into())),
                ..Default::default()
            });
            let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
            let (_, result) = run.release(&release_audio(duration, 1)).await.unwrap();
            assert_eq!(result.provider.transcript, "Complete cleaned stitch");
            assert!(provider.whole_calls.lock().unwrap().is_empty());
            assert_eq!(provider.calls.lock().unwrap().len(), 1);
        }
    });
}

// Keep: a failed ordinal cannot discard later speech or let seam cleanup cross its gap.
#[test]
fn segments_failed_piece_retries_once_keeps_later_speech_and_separate_seams() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let provider = Arc::new(FakeProvider {
            wav_ordinals: true,
            delay_ms: Some(1),
            fail: Some(1),
            outcomes: vec![
                (0, Ok("Alpha beta gamma".into())),
                (2, Ok("Alpha beta gamma delta".into())),
            ],
            ..Default::default()
        });
        let published = Arc::new(Mutex::new(Vec::new()));
        let run = dictation_with_publisher(&jobs, provider.clone(), published.clone());
        let mut recording = release_audio(60_000, 3);
        recording.is_complete = false;
        run.submit(&recording).unwrap();
        wait_until(|| {
            published.lock().unwrap().iter().any(|event| {
                event.status == Some(crate::services::dictation::PartialTranscriptStatus::Gap)
            })
        })
        .await;
        assert!(published.lock().unwrap().iter().any(|event| event.status
            == Some(crate::services::dictation::PartialTranscriptStatus::Retrying)));
        let (_, result) = run.release(&release_audio(60_000, 3)).await.unwrap();
        assert_eq!(
            result.provider.transcript,
            "Alpha beta gamma [Transcription gap] Alpha beta gamma delta"
        );
        assert_eq!(
            provider
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(id, _)| *id == 1)
                .count(),
            2
        );
        assert!(provider.whole_calls.lock().unwrap().is_empty());
    });
}

// Keep: retry uses the frozen settings and contributes its successful text once.
#[test]
fn segments_successful_retry_recovers_once_without_gap() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let provider = Arc::new(FakeProvider {
            wav_ordinals: true,
            delay_ms: Some(1),
            fail_once: Some(0),
            ..Default::default()
        });
        let current = Arc::new(Mutex::new(settings()));
        let registry = jobs.clone();
        let frozen = current.clone();
        let changing = current.clone();
        let run = SegmentedDictation::new_with_publisher(
            request(),
            jobs,
            provider.clone(),
            Arc::new(move |request| {
                registry.admit_initial(request.clone(), frozen.lock().unwrap().clone())
            }),
            Arc::new(move |event| {
                if event.status
                    == Some(crate::services::dictation::PartialTranscriptStatus::Retrying)
                {
                    changing.lock().unwrap().prompt = Some("changed during retry".into());
                }
            }),
        );
        let (_, result) = run.release(&release_audio(60_000, 1)).await.unwrap();
        assert_eq!(result.provider.transcript, "piece-0");
        assert!(!result.partial);
        let calls = provider.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            current.lock().unwrap().prompt.as_deref(),
            Some("changed during retry")
        );
        assert_eq!(calls[0].1.prompt.as_deref(), Some("original"));
        assert_eq!(calls[0].1.prompt, calls[1].1.prompt);
        assert!(provider.whole_calls.lock().unwrap().is_empty());
    });
}

// Keep: every exhausted ordinal survives as a marker, with no speech to deliver.
#[test]
fn segments_all_failed_short_fallback_retains_marker_only_completion() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let provider = Arc::new(FakeProvider {
            wav_ordinals: true,
            delay_ms: Some(1),
            fail: Some(0),
            whole_outcome: Some(Err(fixture_error(SttJobErrorCode::ProviderTimeout))),
            ..Default::default()
        });
        let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
        let (_, result) = run.release(&release_audio(59_999, 1)).await.unwrap();
        assert_eq!(result.provider.transcript, "[Transcription gap]");
        assert!(result.partial);
        assert!(!result.has_speech);
        assert_eq!(provider.calls.lock().unwrap().len(), 2);
        assert_eq!(provider.whole_calls.lock().unwrap().len(), 1);
    });
}

// Keep: pieces completed during recording do not inflate the native-stop wall clock.
#[test]
fn segments_stop_timing_reaches_persistence_as_wall_time() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let provider = Arc::new(FakeProvider {
            wav_ordinals: true,
            delay_ms: Some(100),
            ..Default::default()
        });
        let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
        let mut recording = release_audio(60_000, 6);
        recording.is_complete = false;
        run.submit(&recording).unwrap();
        wait_until(|| provider.completed.load(Ordering::SeqCst) == 6).await;
        let mut stopped = release_audio(60_000, 7);
        stopped.stopped_at = Some(Instant::now());
        let (job, result) = run.release(&stopped).await.unwrap();
        assert_eq!(result.provider.provider_ms, 700);
        let row = crate::commands::stt::persistence::build_record(&job, &stopped.metadata, result);
        assert!(row.timings.stop_to_transcript_ms >= 90.0);
        assert!(row.timings.stop_to_transcript_ms < 650.0);
        assert_eq!(row.timings.provider_ms, Some(700.0));
    });
}

// Keep: saving a partial result must not turn a rejected credential into a valid one.
#[test]
fn segments_credential_errors_skip_piece_retry_and_persist_partial_or_marker_only_rows() {
    tauri::async_runtime::block_on(async {
        for code in [
            SttJobErrorCode::MissingProviderKey,
            SttJobErrorCode::ProviderCredentialRejected,
        ] {
            for (all_failed, duration_ms) in [
                (false, 30_000),
                (true, 30_000),
                (false, 90_000),
                (true, 90_000),
            ] {
                let jobs = TranscriptionJobs::default();
                let error = SttJobError::new(
                    code,
                    SttJobStage::Provider,
                    false,
                    "Fixture credential failure.",
                );
                let mut outcomes = vec![(0, Err(error.clone()))];
                if all_failed {
                    outcomes.push((1, Err(error)));
                }
                let provider = Arc::new(FakeProvider {
                    wav_ordinals: true,
                    delay_ms: Some(1),
                    outcomes,
                    whole_outcome: Some(Err(SttJobError::new(
                        code,
                        SttJobStage::Provider,
                        false,
                        "Fixture credential failure.",
                    ))),
                    ..Default::default()
                });
                let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
                let stopped = release_audio(duration_ms, 2);
                let (job, completion) = run.release(&stopped).await.unwrap();
                assert!(completion.credential_rejected);
                assert_eq!(completion.has_speech, !all_failed);
                assert!(completion.partial);
                assert_eq!(provider.calls.lock().unwrap().len(), 2);
                assert_eq!(
                    provider.whole_calls.lock().unwrap().len(),
                    usize::from(duration_ms < 60_000)
                );
                let row = crate::commands::stt::persistence::build_record(
                    &job,
                    &stopped.metadata,
                    completion,
                );
                assert_eq!(row.error_code.as_deref(), Some("provider_failed"));
                assert_eq!(
                    row.insertion_mode,
                    if all_failed { "failed" } else { "manual" }
                );
                assert_eq!(
                    row.text,
                    if all_failed {
                        "[Transcription gap] [Transcription gap]"
                    } else {
                        "[Transcription gap] piece-1"
                    }
                );
            }
        }
    });
}

// Keep: cancellation at the retry boundary prevents a second provider dispatch.
#[test]
fn segments_cancel_at_retry_notice_prevents_second_dispatch() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let registry = jobs.clone();
        let canceling = jobs.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = events.clone();
        let provider = Arc::new(FakeProvider {
            wav_ordinals: true,
            delay_ms: Some(1),
            fail: Some(0),
            ..Default::default()
        });
        let run = SegmentedDictation::new_with_publisher(
            request(),
            jobs,
            provider.clone(),
            Arc::new(move |request| registry.admit_initial(request.clone(), settings())),
            Arc::new(move |event| {
                if event.status
                    == Some(crate::services::dictation::PartialTranscriptStatus::Retrying)
                {
                    canceling.cancel("overlay", "session", "intent").unwrap();
                }
                observed.lock().unwrap().push(event);
            }),
        );
        assert_eq!(
            run.release(&release_audio(60_000, 1))
                .await
                .unwrap_err()
                .code,
            SttJobErrorCode::JobCanceled
        );
        assert_eq!(provider.calls.lock().unwrap().len(), 1);
        assert_eq!(events.lock().unwrap().len(), 1);
        assert!(provider.whole_calls.lock().unwrap().is_empty());
    });
}
