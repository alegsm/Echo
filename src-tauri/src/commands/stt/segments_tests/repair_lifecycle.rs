use super::*;

#[test]
fn segments_release_retires_older_repair_and_awaits_only_final_seam() {
    tauri::async_runtime::block_on(async {
        for (duration, cancel, expected_pairs) in
            [(59_999, false, 2), (60_000, false, 1), (59_999, true, 2)]
        {
            let jobs = TranscriptionJobs::default();
            let (response, responses) = tauri::async_runtime::channel(2);
            let provider = Arc::new(FakeProvider {
                wav_ordinals: true,
                delay_ms: Some(1),
                pair_release: Some(Arc::new(tauri::async_runtime::Mutex::new(responses))),
                pair_outcomes: vec![
                    (0, Ok("older must retire".into())),
                    (1, Ok("final pair".into())),
                ],
                ..Default::default()
            });
            let published = Arc::new(Mutex::new(Vec::new()));
            let run = dictation_with_publisher(&jobs, provider.clone(), published.clone());
            let mut bundle = release_audio(10_000, 2);
            bundle.is_complete = false;
            for piece in Arc::make_mut(&mut bundle.segments) {
                piece.cut_kind = crate::audio::SegmentCutKind::Valley;
            }
            run.submit(&bundle).unwrap();
            wait_until(|| provider.pair_calls.lock().unwrap().len() == 1).await;
            let mut sealed = release_audio(duration, 3);
            for piece in Arc::make_mut(&mut sealed.segments) {
                piece.cut_kind = crate::audio::SegmentCutKind::Valley;
            }
            let releasing = run.clone();
            let release =
                tauri::async_runtime::spawn(async move { releasing.release(&sealed).await });
            if duration < 60_000 {
                wait_until(|| provider.pair_calls.lock().unwrap().len() == 2).await;
                assert_eq!(
                    provider.pair_completed.load(Ordering::SeqCst),
                    0,
                    "old request must be dropped to release the slot"
                );
                assert!(
                    !release.inner().is_finished(),
                    "eligible final seam must be awaited"
                );
                if cancel {
                    run.cancel();
                } else {
                    response.send(()).await.unwrap();
                }
            }
            let result = release.await.unwrap();
            if cancel {
                assert_eq!(result.unwrap_err().code, SttJobErrorCode::JobCanceled);
            } else {
                assert_eq!(
                    result.unwrap().1.provider.transcript,
                    if duration < 60_000 {
                        "piece-0 final pair"
                    } else {
                        "piece-0 piece-1 piece-2"
                    }
                );
            }
            assert_eq!(provider.pair_calls.lock().unwrap().len(), expected_pairs);
            assert!(provider.whole_calls.lock().unwrap().is_empty());
            // Both kinds of permit must be back after retirement/cancellation.
            let slot = jobs.repair_slot().await;
            let mut permits = Vec::new();
            for _ in 0..6 {
                permits.push(jobs.provider_permit().await);
            }
            drop((slot, permits));
        }
    });
}

#[test]
fn segments_shared_repair_slot_does_not_block_capture_in_another_dictation() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let (response, responses) = tauri::async_runtime::channel(2);
        let provider = Arc::new(FakeProvider {
            wav_ordinals: true,
            delay_ms: Some(1),
            pair_release: Some(Arc::new(tauri::async_runtime::Mutex::new(responses))),
            pair_outcomes: vec![(0, Ok("paired text".into()))],
            ..Default::default()
        });
        let first = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
        let registry = jobs.clone();
        let second = SegmentedDictation::new(
            JobRequest {
                intent_id: "second".into(),
                ..request()
            },
            jobs.clone(),
            provider.clone(),
            Arc::new(move |request| registry.admit_initial(request.clone(), settings())),
        );
        let mut bundle = release_audio(2_000, 2);
        bundle.is_complete = false;
        Arc::make_mut(&mut bundle.segments)[0].cut_kind = crate::audio::SegmentCutKind::Valley;
        first.submit(&bundle).unwrap();
        wait_until(|| provider.pair_calls.lock().unwrap().len() == 1).await;
        second.submit(&bundle).unwrap();
        wait_until(|| provider.completed.load(Ordering::SeqCst) == 4).await;
        assert_eq!(
            provider.pair_calls.lock().unwrap().len(),
            1,
            "second repair must wait outside the provider pool"
        );
        let mut permits = Vec::new();
        for _ in 0..5 {
            permits.push(jobs.provider_permit().await);
        }
        assert_eq!(provider.pair_completed.load(Ordering::SeqCst), 0);
        drop(permits);
        response.send(()).await.unwrap();
        wait_until(|| provider.pair_calls.lock().unwrap().len() == 2).await;
        assert_eq!(provider.pair_completed.load(Ordering::SeqCst), 1);
        response.send(()).await.unwrap();
        wait_until(|| provider.pair_completed.load(Ordering::SeqCst) == 2).await;
        bundle.is_complete = true;
        assert_eq!(
            first.release(&bundle).await.unwrap().1.provider.transcript,
            "paired text"
        );
        assert_eq!(
            second.release(&bundle).await.unwrap().1.provider.transcript,
            "paired text"
        );
    });
}

#[test]
fn segments_release_does_not_wait_for_a_final_pair_consumed_by_an_earlier_repair() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let (response, responses) = tauri::async_runtime::channel(1);
        let provider = Arc::new(FakeProvider {
            wav_ordinals: true,
            delay_ms: Some(1),
            pair_release: Some(Arc::new(tauri::async_runtime::Mutex::new(responses))),
            pair_outcomes: vec![
                (0, Ok("...accepted earlier pair…".into())),
                (1, Ok("must retire".into())),
            ],
            ..Default::default()
        });
        let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
        let mut bundle = release_audio(3_000, 3);
        bundle.is_complete = false;
        for piece in Arc::make_mut(&mut bundle.segments) {
            piece.cut_kind = crate::audio::SegmentCutKind::Valley;
        }
        let mut prefix = bundle.clone();
        prefix.segments = bundle.segments[..2].to_vec().into();
        run.submit(&prefix).unwrap();
        wait_until(|| provider.pair_calls.lock().unwrap().len() == 1).await;
        response.send(()).await.unwrap();
        wait_until(|| provider.pair_completed.load(Ordering::SeqCst) == 1).await;
        run.submit(&bundle).unwrap();
        wait_until(|| provider.pair_calls.lock().unwrap().len() == 2).await;
        bundle.is_complete = true;
        let released = run.clone();
        let result = Arc::new(Mutex::new(None));
        let completed = result.clone();
        let release = tauri::async_runtime::spawn(async move {
            *completed.lock().unwrap() = Some(
                released
                    .release(&bundle)
                    .await
                    .unwrap()
                    .1
                    .provider
                    .transcript,
            );
        });
        wait_until(|| result.lock().unwrap().is_some()).await;
        release.await.unwrap();
        assert_eq!(
            result.lock().unwrap().as_deref(),
            Some("accepted earlier pair piece-2")
        );
        assert_eq!(provider.pair_completed.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn segments_final_speech_seam_is_awaited_before_a_silent_release_tail() {
    tauri::async_runtime::block_on(async {
        for gated in [true, false] {
            let jobs = TranscriptionJobs::default();
            let (response, responses) = tauri::async_runtime::channel(1);
            let provider = Arc::new(FakeProvider {
                wav_ordinals: true,
                delay_ms: Some(1),
                outcomes: if gated {
                    vec![]
                } else {
                    vec![(2, Ok(" ".into()))]
                },
                pair_release: Some(Arc::new(tauri::async_runtime::Mutex::new(responses))),
                pair_outcomes: vec![(0, Ok("repaired speech".into()))],
                ..Default::default()
            });
            let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
            let mut bundle = release_audio(3_000, 3);
            bundle.is_complete = false;
            Arc::make_mut(&mut bundle.segments)[0].cut_kind = crate::audio::SegmentCutKind::Valley;
            if gated {
                Arc::make_mut(&mut bundle.segments)[2].bytes = None;
            }
            run.submit(&bundle).unwrap();
            wait_until(|| provider.pair_calls.lock().unwrap().len() == 1).await;
            wait_until(|| provider.completed.load(Ordering::SeqCst) == if gated { 2 } else { 3 })
                .await;
            bundle.is_complete = true;
            let releasing = run.clone();
            let release =
                tauri::async_runtime::spawn(async move { releasing.release(&bundle).await });
            // A retired repair would finish release without a provider response.
            tauri::async_runtime::spawn_blocking(|| std::thread::sleep(Duration::from_millis(30)))
                .await
                .unwrap();
            assert!(
                !release.inner().is_finished(),
                "last speech seam must wait, gated={gated}"
            );
            response.send(()).await.unwrap();
            assert_eq!(
                release.await.unwrap().unwrap().1.provider.transcript,
                "repaired speech"
            );
            assert!(provider.whole_calls.lock().unwrap().is_empty());
        }
    });
}

#[test]
fn segments_fallback_waits_for_successful_chunk_and_retires_obsolete_repair() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let (chunk_response, chunk_responses) = std::sync::mpsc::channel();
        let (_pair_response, pair_responses) = tauri::async_runtime::channel(1);
        let provider = Arc::new(FakeProvider {
            wav_ordinals: true,
            delay_ms: Some(1),
            release: Some(Arc::new(Mutex::new(chunk_responses))),
            release_ordinals: Some(vec![2]),
            pair_release: Some(Arc::new(tauri::async_runtime::Mutex::new(pair_responses))),
            fail: Some(3),
            whole_outcome: Some(Ok("whole fallback".into())),
            ..Default::default()
        });
        let published = Arc::new(Mutex::new(Vec::new()));
        let run = dictation_with_publisher(&jobs, provider.clone(), published.clone());
        let mut bundle = release_audio(4_000, 3);
        bundle.is_complete = false;
        Arc::make_mut(&mut bundle.segments)[0].cut_kind = crate::audio::SegmentCutKind::Valley;
        run.submit(&bundle).unwrap();
        wait_until(|| {
            provider.pair_calls.lock().unwrap().len() == 1
                && provider.calls.lock().unwrap().len() == 3
        })
        .await;
        assert_eq!(provider.background_completed.load(Ordering::SeqCst), 2);
        let releasing = run.clone();
        let release = tauri::async_runtime::spawn(async move {
            releasing.release(&release_audio(4_000, 4)).await
        });
        wait_until(|| provider.completed.load(Ordering::SeqCst) == 4).await;
        assert!(
            provider.whole_calls.lock().unwrap().is_empty(),
            "the blocked successful ordinal must not be discarded"
        );
        chunk_response.send(()).unwrap();
        wait_until(|| provider.whole_calls.lock().unwrap().len() == 1).await;
        assert_eq!(
            release.await.unwrap().unwrap().1.provider.transcript,
            "whole fallback"
        );
        assert_eq!(provider.background_completed.load(Ordering::SeqCst), 5);
        assert_eq!(provider.pair_completed.load(Ordering::SeqCst), 0);
        let capacity = tauri::async_runtime::spawn(async move {
            let slot = jobs.repair_slot().await;
            let mut permits = Vec::new();
            for _ in 0..6 {
                permits.push(jobs.provider_permit().await);
            }
            drop((slot, permits));
        });
        wait_until(|| capacity.inner().is_finished()).await;
        capacity.await.unwrap();
        assert_eq!(provider.calls.lock().unwrap().len(), 5);
        assert_eq!(provider.pair_calls.lock().unwrap().len(), 1);
        assert!(published.lock().unwrap().iter().any(|event| event.status
            == Some(crate::services::dictation::PartialTranscriptStatus::Recovered)));
    });
}

#[test]
fn segments_terminal_artifact_failure_retires_an_inflight_repair() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let (_response, responses) = tauri::async_runtime::channel(1);
        let provider = Arc::new(FakeProvider {
            wav_ordinals: true,
            delay_ms: Some(1),
            pair_release: Some(Arc::new(tauri::async_runtime::Mutex::new(responses))),
            pair_outcomes: vec![(0, Ok("must never deliver".into()))],
            ..Default::default()
        });
        let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
        let mut bundle = release_audio(2_000, 2);
        bundle.is_complete = false;
        Arc::make_mut(&mut bundle.segments)[0].cut_kind = crate::audio::SegmentCutKind::Valley;
        run.submit(&bundle).unwrap();
        wait_until(|| provider.pair_calls.lock().unwrap().len() == 1).await;
        let failure = SttJobError::new(
            SttJobErrorCode::ArtifactExpired,
            SttJobStage::Artifact,
            false,
            "Fixture artifact expired.",
        );
        run.fail(failure.clone());
        bundle.is_complete = true;
        assert_eq!(run.release(&bundle).await.unwrap_err(), failure);
        let capacity = tauri::async_runtime::spawn(async move {
            let slot = jobs.repair_slot().await;
            let mut permits = Vec::new();
            for _ in 0..6 {
                permits.push(jobs.provider_permit().await);
            }
            drop((slot, permits));
        });
        wait_until(|| capacity.inner().is_finished()).await;
        capacity.await.unwrap();
        assert_eq!(provider.pair_completed.load(Ordering::SeqCst), 0);
        assert!(provider.whole_calls.lock().unwrap().is_empty());
    });
}
