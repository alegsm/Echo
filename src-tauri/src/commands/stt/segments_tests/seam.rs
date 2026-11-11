use super::*;

#[test]
fn segments_seam_classification_and_capture_repair_preserve_original_preview() {
    tauri::async_runtime::block_on(async {
        let mut low_log = confident_edges();
        low_log.last.as_mut().unwrap().avg_logprob = -0.75;
        let mut high_silence = confident_edges();
        high_silence.first.as_mut().unwrap().no_speech_prob = 0.60;
        let mut nonfinite = confident_edges();
        nonfinite.last.as_mut().unwrap().avg_logprob = f64::NAN;
        for (name, left_edges, right_edges, cut, left, right, doubtful) in [
            (
                "sound",
                Some(confident_edges()),
                Some(confident_edges()),
                crate::audio::SegmentCutKind::QuietWindow,
                "first",
                "second",
                false,
            ),
            (
                "left threshold",
                Some(low_log),
                Some(confident_edges()),
                crate::audio::SegmentCutKind::QuietWindow,
                "first",
                "second",
                true,
            ),
            (
                "right threshold",
                Some(confident_edges()),
                Some(high_silence),
                crate::audio::SegmentCutKind::QuietWindow,
                "first",
                "second",
                true,
            ),
            (
                "missing evidence",
                None,
                Some(confident_edges()),
                crate::audio::SegmentCutKind::QuietWindow,
                "first",
                "second",
                true,
            ),
            (
                "nonfinite",
                Some(nonfinite),
                Some(confident_edges()),
                crate::audio::SegmentCutKind::QuietWindow,
                "first",
                "second",
                true,
            ),
            (
                "valley",
                Some(confident_edges()),
                Some(confident_edges()),
                crate::audio::SegmentCutKind::Valley,
                "first",
                "second",
                true,
            ),
            (
                "overlap",
                Some(confident_edges()),
                Some(confident_edges()),
                crate::audio::SegmentCutKind::QuietWindow,
                "one two three",
                "one two three four",
                true,
            ),
        ] {
            for out_of_order in [false, true] {
                let jobs = TranscriptionJobs::default();
                let published = Arc::new(Mutex::new(Vec::new()));
                let provider = Arc::new(FakeProvider {
                    wav_ordinals: true,
                    delay_ms: (!out_of_order).then_some(1),
                    out_of_order,
                    outcomes: vec![(0, Ok(left.into())), (1, Ok(right.into()))],
                    edges: vec![(0, left_edges), (1, right_edges)],
                    pair_outcomes: vec![(0, Ok("repaired pair".into()))],
                    ..Default::default()
                });
                let run = dictation_with_publisher(&jobs, provider.clone(), published.clone());
                let mut bundle = release_audio(2_000, 2);
                bundle.is_complete = false;
                Arc::make_mut(&mut bundle.segments)[0].cut_kind = cut;
                run.submit(&bundle).unwrap();
                if doubtful {
                    wait_until(|| provider.pair_completed.load(Ordering::SeqCst) == 1).await;
                }
                wait_until(|| published.lock().unwrap().len() == 2).await;
                run.submit(&bundle).unwrap(); // duplicate snapshot cannot reserve the seam twice
                let original_preview = published
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|e| e.text.clone())
                    .collect::<Vec<_>>()
                    .join(" ");
                bundle.is_complete = true;
                let (_, result) = run.release(&bundle).await.unwrap();
                assert_eq!(
                    provider.pair_calls.lock().unwrap().len(),
                    usize::from(doubtful),
                    "{name}"
                );
                assert_eq!(
                    result.provider.transcript,
                    if doubtful {
                        "repaired pair"
                    } else {
                        &original_preview
                    },
                    "{name}"
                );
                assert_eq!(
                    published
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|e| e.text.clone())
                        .collect::<Vec<_>>()
                        .join(" "),
                    original_preview
                );
                assert!(provider.whole_calls.lock().unwrap().is_empty());
            }
        }
    });
}

#[test]
fn segments_repairs_cap_overlap_failure_and_long_discard_matrix() {
    tauri::async_runtime::block_on(async {
        for (name, pair0, duration, expected) in [
            (
                "left wins",
                Ok("repair zero one".into()),
                59_999,
                "repair zero one repair two three piece-4",
            ),
            (
                "left fails",
                Err(fixture_error(SttJobErrorCode::ProviderTimeout)),
                59_999,
                "piece-0 repair one two piece-3 piece-4",
            ),
            (
                "empty",
                Ok("  ".into()),
                59_999,
                "piece-0 repair one two piece-3 piece-4",
            ),
            (
                "low confidence",
                Err(fixture_error(SttJobErrorCode::ProviderLowConfidence)),
                59_999,
                "piece-0 repair one two piece-3 piece-4",
            ),
            (
                "silent repair",
                Err(fixture_error(SttJobErrorCode::ProviderSilence)),
                59_999,
                "piece-0 repair one two piece-3 piece-4",
            ),
            (
                "long discards",
                Ok("repair zero one".into()),
                60_000,
                "piece-0 piece-1 piece-2 piece-3 piece-4",
            ),
        ] {
            let jobs = TranscriptionJobs::default();
            let provider = Arc::new(FakeProvider {
                wav_ordinals: true,
                delay_ms: Some(1),
                pair_outcomes: vec![
                    (0, pair0),
                    (1, Ok("repair one two".into())),
                    (2, Ok("repair two three".into())),
                ],
                ..Default::default()
            });
            let current = Arc::new(Mutex::new(settings()));
            let run = dictation(&jobs, provider.clone(), current.clone());
            let mut bundle = release_audio(10_000, 5);
            bundle.is_complete = false;
            for piece in Arc::make_mut(&mut bundle.segments) {
                piece.cut_kind = crate::audio::SegmentCutKind::Valley;
            }
            // Admit chunks in sequence to make the cap's ordinal choice observable.
            for count in 1..=5 {
                let mut prefix = bundle.clone();
                prefix.segments = bundle.segments[..count].to_vec().into();
                run.submit(&prefix).unwrap();
                wait_until(|| provider.completed.load(Ordering::SeqCst) == count).await;
                if count > 1 && count <= 4 {
                    wait_until(|| provider.pair_completed.load(Ordering::SeqCst) == count - 1)
                        .await;
                }
                current.lock().unwrap().prompt = Some("changed".into());
            }
            bundle.is_complete = true;
            bundle.metadata.duration_ms = duration;
            let (_, result) = run.release(&bundle).await.unwrap();
            assert_eq!(result.provider.transcript, expected, "{name}");
            let pairs = provider.pair_calls.lock().unwrap();
            assert_eq!(
                pairs.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
                [0, 1, 2]
            );
            assert!(pairs
                .iter()
                .all(|(_, frozen)| frozen.prompt == settings().prompt
                    && frozen.language.as_deref() == Some("en")
                    && frozen.provider_id == settings().provider_id));
            assert!(provider.whole_calls.lock().unwrap().is_empty());
        }
    });
}

#[test]
fn segments_overlapping_pairs_choose_left_seam_in_either_completion_order() {
    tauri::async_runtime::block_on(async {
        for reversed in [false, true] {
            let jobs = TranscriptionJobs::default();
            let provider = Arc::new(FakeProvider {
                wav_ordinals: true,
                out_of_order: reversed,
                delay_ms: (!reversed).then_some(1),
                pair_outcomes: vec![(0, Ok("left pair".into())), (1, Ok("right pair".into()))],
                ..Default::default()
            });
            let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
            let mut bundle = release_audio(3_000, 3);
            bundle.is_complete = false;
            for piece in Arc::make_mut(&mut bundle.segments) {
                piece.cut_kind = crate::audio::SegmentCutKind::Valley;
            }
            if reversed {
                run.submit(&bundle).unwrap();
            } else {
                let mut prefix = bundle.clone();
                prefix.segments = bundle.segments[..2].to_vec().into();
                run.submit(&prefix).unwrap();
                wait_until(|| provider.pair_completed.load(Ordering::SeqCst) == 1).await;
                run.submit(&bundle).unwrap();
            }
            wait_until(|| provider.pair_completed.load(Ordering::SeqCst) == 2).await;
            bundle.is_complete = true;
            let (_, result) = run.release(&bundle).await.unwrap();
            assert_eq!(result.provider.transcript, "left pair piece-2");
            assert_eq!(provider.pair_calls.lock().unwrap()[0].0, u8::from(reversed));
        }
    });
}

#[test]
fn segments_unusable_pair_and_silent_audio_ordinals_keep_originals() {
    tauri::async_runtime::block_on(async {
        for silent in [false, true] {
            let jobs = TranscriptionJobs::default();
            let provider = Arc::new(FakeProvider {
                wav_ordinals: true,
                delay_ms: Some(1),
                pair_missing_evidence: true,
                pair_outcomes: vec![(0, Ok("must reject missing evidence".into()))],
                ..Default::default()
            });
            let run = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
            let mut bundle = release_audio(3_000, 3);
            bundle.is_complete = false;
            for piece in Arc::make_mut(&mut bundle.segments) {
                piece.cut_kind = crate::audio::SegmentCutKind::Valley;
            }
            if silent {
                Arc::make_mut(&mut bundle.segments)[1].bytes = None;
            }
            run.submit(&bundle).unwrap();
            wait_until(|| provider.completed.load(Ordering::SeqCst) == if silent { 2 } else { 3 })
                .await;
            if !silent {
                wait_until(|| provider.pair_completed.load(Ordering::SeqCst) == 2).await;
            }
            bundle.is_complete = true;
            assert_eq!(
                run.release(&bundle).await.unwrap().1.provider.transcript,
                if silent {
                    "piece-0 piece-2"
                } else {
                    "piece-0 piece-1 piece-2"
                }
            );
            if silent {
                assert!(provider.pair_calls.lock().unwrap().is_empty());
            }
            assert!(provider.whole_calls.lock().unwrap().is_empty());
        }
    });
}

#[test]
fn segments_overlap_spanning_a_short_left_chunk_repairs_the_join() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let provider = Arc::new(FakeProvider {
            wav_ordinals: true,
            delay_ms: Some(1),
            outcomes: vec![
                (0, Ok("one two three".into())),
                (1, Ok("four five".into())),
                (2, Ok("three four five six".into())),
            ],
            pair_outcomes: vec![(1, Ok("four five six repaired".into()))],
            ..Default::default()
        });
        let published = Arc::new(Mutex::new(Vec::new()));
        let run = dictation_with_publisher(&jobs, provider.clone(), published.clone());
        let mut bundle = release_audio(3_000, 3);
        bundle.is_complete = false;
        for count in 1..=3 {
            let mut prefix = bundle.clone();
            prefix.segments = bundle.segments[..count].to_vec().into();
            run.submit(&prefix).unwrap();
            wait_until(|| published.lock().unwrap().len() == count).await;
        }
        wait_until(|| provider.pair_completed.load(Ordering::SeqCst) == 1).await;
        bundle.is_complete = true;
        assert_eq!(
            run.release(&bundle).await.unwrap().1.provider.transcript,
            "one two three four five six repaired"
        );
        assert_eq!(provider.pair_calls.lock().unwrap()[0].0, 1);
        assert_eq!(published.lock().unwrap()[2].text, "six");
    });
}
