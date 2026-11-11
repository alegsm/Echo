use super::*;

#[test]
fn segments_dispatch_uses_frozen_prompt_without_predecessor_text() {
    tauri::async_runtime::block_on(async {
        let composed = crate::commands::stt::settings::freeze_provider_settings(
            &crate::storage::PersistedSettings {
                custom_prompt: Some("Custom instruction".into()),
                phrasebook_terms: vec!["Sample".into(), "Echo".into()],
                ..Default::default()
            },
            ProviderId::GroqWhisperTurbo,
        )
        .unwrap()
        .prompt;
        for base_prompt in [None, composed, Some("b".repeat(1_000))] {
            let jobs = TranscriptionJobs::default();
            let provider = Arc::new(FakeProvider {
                delay_ms: Some(1),
                ..Default::default()
            });
            let current = Arc::new(Mutex::new(FrozenJobSettings {
                prompt: base_prompt.clone(),
                ..settings()
            }));
            let registry = jobs.clone();
            let published = Arc::new(Mutex::new(Vec::new()));
            let events = published.clone();
            let frozen = current.clone();
            let run = SegmentedDictation::new_with_publisher(
                request(),
                jobs,
                provider.clone(),
                Arc::new(move |request| {
                    registry.admit_initial(request.clone(), frozen.lock().unwrap().clone())
                }),
                Arc::new(move |event| events.lock().unwrap().push(event)),
            );
            run.submit(&audio(1, false, None)).unwrap();
            wait_until(|| published.lock().unwrap().len() == 1).await;
            current.lock().unwrap().prompt = Some("changed after freeze".into());
            run.submit(&audio(2, true, None)).unwrap();
            let (job, _) = run.finish().await.unwrap();
            assert_eq!(job.settings.prompt, base_prompt);
            let calls = provider.calls.lock().unwrap();
            assert_eq!(calls[0].1.prompt, base_prompt);
            assert_eq!(calls[1].1.prompt, base_prompt);
        }
    });
}

#[test]
fn segments_five_minute_capture_accepts_two_second_ordinals() {
    let jobs = TranscriptionJobs::default();
    let run = dictation(
        &jobs,
        Arc::new(FakeProvider::default()),
        Arc::new(Mutex::new(settings())),
    );
    let mut bundle = audio(151, false, None);
    for piece in Arc::make_mut(&mut bundle.segments) {
        piece.bytes = None;
    }
    run.submit(&bundle).unwrap();
}

#[test]
fn segments_whole_dictation_low_confidence_and_silence_keep_existing_public_error() {
    for code in [
        crate::providers::ProviderErrorCode::LowConfidence,
        crate::providers::ProviderErrorCode::Silence,
    ] {
        let mapped = crate::commands::stt::errors::provider_error(
            crate::providers::ProviderError::new(code, "Fixture verdict."),
        );
        assert_eq!(mapped.code, SttJobErrorCode::ProviderLowConfidence);
        assert!(mapped.retryable);
    }
}

#[test]
fn segments_silence_is_empty_while_uncertain_speech_publishes() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let published = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(FakeProvider {
            outcomes: vec![
                (
                    0,
                    Err(SttJobError::new(
                        SttJobErrorCode::ProviderSilence,
                        SttJobStage::Provider,
                        true,
                        "Fixture silence.",
                    )),
                ),
                (1, Ok("uncertain speech".into())),
            ],
            delay_ms: Some(1),
            ..Default::default()
        });
        let run = dictation_with_publisher(&jobs, provider, published.clone());
        run.submit(&audio(2, true, None)).unwrap();
        let (_, result) = run.finish().await.unwrap();
        assert_eq!(result.provider.transcript, "uncertain speech");
        let events = published.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].ordinal, 1);
        assert_eq!(events[0].text, "uncertain speech");
    });
}

#[test]
fn segments_seams_match_ordered_preview_and_final_stitch() {
    tauri::async_runtime::block_on(async {
        let cases = [
            (vec!["and then", "I think so"], "and then I think so"),
            (vec!["and then", "I'm ready"], "and then I'm ready"),
            (vec!["and then", "NASA said"], "and then NASA said"),
            (vec!["and then", "Paris in May"], "and then Paris in May"),
            (vec!["and then", "Dr. Smith"], "and then Dr. Smith"),
            (vec!["and then", "2026 Plans"], "and then 2026 Plans"),
            (vec!["and then", "The next step"], "and then the next step"),
            (vec!["and then", "A plan"], "and then a plan"),
            (vec!["and then", "The2 plans"], "and then The2 plans"),
            (vec!["and then", "I’m ready"], "and then I’m ready"),
            (vec!["and then", "THE next step"], "and then THE next step"),
            (vec!["and then", "ThE next step"], "and then ThE next step"),
            (vec!["no no no", "no no no way"], "no no no no no no way"),
            (vec!["No no NO", "no NO no way"], "No no NO no NO no way"),
            (vec!["...Hello…", "…World..."], "Hello World"),
            (vec!["Hello… there", "Again"], "Hello… there Again"),
            (vec!["Wait... here", "Again"], "Wait... here Again"),
            (vec!["One", "One two"], "One one two"),
            (
                vec![
                    "One two three one two three",
                    "One two three one two three Again",
                ],
                "One two three one two three Again",
            ),
            (
                vec!["We need the release notes.", "THE release notes. Next"],
                "We need the release notes. Next",
            ),
            (
                vec!["We need the release notes", "THE RELEASE NOTES Today"],
                "We need the release notes Today",
            ),
            (
                vec!["One two three one two three", "One two three Again"],
                "One two three one two three Again",
            ),
            (vec!["One two", "One two three"], "One two one two three"),
            (
                vec![
                    "Repeat these words here",
                    "Next repeat these words here later",
                ],
                "Repeat these words here Next repeat these words here later",
            ),
            (
                vec!["Done.", "Next!", "Yes?", "Fine"],
                "Done. Next! Yes? Fine",
            ),
            (
                vec!["Keep  internal, um… punctuation", "  And   spacing  "],
                "Keep  internal, um… punctuation and   spacing",
            ),
            (
                vec!["...", "Alpha beta gamma", "ALPHA BETA GAMMA", "", "Delta"],
                "Alpha beta gamma Delta",
            ),
            (
                vec!["We need release notes.", "Release notes. For tomorrow"],
                "We need release notes. Release notes. For tomorrow",
            ),
            (
                vec!["Alpha beta gamma", "Alpha beta gamma delta"],
                "Alpha beta gamma delta",
            ),
        ];
        for (chunks, expected) in cases {
            let jobs = TranscriptionJobs::default();
            let published = Arc::new(Mutex::new(Vec::new()));
            let provider = Arc::new(FakeProvider {
                outcomes: chunks
                    .iter()
                    .enumerate()
                    .map(|(i, s)| (i as u8, Ok((*s).into())))
                    .collect(),
                out_of_order: true,
                ..Default::default()
            });
            let run = dictation_with_publisher(&jobs, provider, published.clone());
            run.submit(&audio(chunks.len() as u8, true, None)).unwrap();
            let (_, result) = run.finish().await.unwrap();
            let preview = published
                .lock()
                .unwrap()
                .iter()
                .map(|e| e.text.clone())
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(result.provider.transcript, expected, "chunks: {chunks:?}");
            assert_eq!(preview, expected, "preview: {chunks:?}");
        }
    });
}
