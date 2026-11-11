use super::*;

#[test]
fn global_request_cap_is_six_across_dictations_including_canceled_requests() {
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let provider = Arc::new(FakeProvider::default());
        let first = dictation(&jobs, provider.clone(), Arc::new(Mutex::new(settings())));
        first.submit(&audio(8, false, None)).unwrap();
        wait_until(|| provider.calls.lock().unwrap().len() == 6).await;
        assert_eq!(provider.active.load(Ordering::SeqCst), 6);
        first.cancel();
        let mut next = request();
        next.intent_id = "second".into();
        let registry = jobs.clone();
        let second = SegmentedDictation::new(
            next,
            jobs,
            provider.clone(),
            Arc::new(move |request| registry.admit_initial(request.clone(), settings())),
        );
        second.submit(&audio(2, true, None)).unwrap();
        assert_eq!(provider.calls.lock().unwrap().len(), 6);
        let (_, result) = second.finish().await.unwrap();
        assert_eq!(result.provider.transcript, "piece-0 piece-1");
        assert_eq!(provider.calls.lock().unwrap().len(), 8);
        assert_eq!(provider.peak_active.load(Ordering::SeqCst), 6);
    });
}

#[test]
fn capture_cancellation_requires_the_registered_session() {
    let jobs = TranscriptionJobs::default();
    let run = dictation(
        &jobs,
        Arc::new(FakeProvider::default()),
        Arc::new(Mutex::new(settings())),
    );
    let registry = CaptureDictations::default();
    registry.entries.lock().unwrap().insert(
        "intent".into(),
        Arc::new(CaptureDictation {
            run,
            request: request(),
            reads: tauri::async_runtime::Mutex::new(()),
            released: std::sync::atomic::AtomicBool::new(false),
            release_timer: Mutex::new(None),
        }),
    );
    assert_eq!(
        registry
            .validate_session("intent", Some("foreign"))
            .unwrap_err()
            .code,
        SttJobErrorCode::InvalidJob
    );
    assert!(registry.validate_session("intent", Some("session")).is_ok());
    assert!(registry.validate_session("intent", None).is_ok());
}

#[test]
fn registry_limits_dictations_and_rejects_other_windows() {
    let registry = CaptureDictations::default();
    let jobs = TranscriptionJobs::default();
    let provider = Arc::new(FakeProvider::default());
    let admission = jobs.clone();
    let start: crate::services::dictation::StartJob =
        Arc::new(move |request| admission.admit_initial(request.clone(), settings()));
    for owner in ["dashboard", "unknown"] {
        let foreign = JobRequest {
            owner: owner.into(),
            ..request()
        };
        assert_eq!(
            registry
                .get_with(foreign, jobs.clone(), provider.clone(), start.clone())
                .err()
                .unwrap()
                .code,
            SttJobErrorCode::InvalidJob
        );
    }
    for index in 0..6 {
        let request = JobRequest {
            intent_id: format!("intent-{index}"),
            ..request()
        };
        registry
            .get_with(request, jobs.clone(), provider.clone(), start.clone())
            .unwrap();
    }
    assert_eq!(
        registry
            .get_with(request(), jobs.clone(), provider.clone(), start.clone())
            .err()
            .unwrap()
            .code,
        SttJobErrorCode::TooManyJobs
    );
    registry.cancel("intent-0");
    registry.get_with(request(), jobs, provider, start).unwrap();
}

#[test]
fn released_entry_rejects_a_delayed_snapshot_before_submission() {
    use crate::audio::tests::fixtures::{begin, owner, spawn_coordinator, FakeBackend};
    use crate::audio::{AudioCapturePolicy, StopCapture};
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
        input.feed(&capture, 0.2, 1_000);
        let metadata = capture
            .stop(StopCapture {
                owner,
                intent_id: "intent".into(),
                session_id: session.session_id,
            })
            .unwrap();
        let jobs = TranscriptionJobs::default();
        let mut held = Vec::new();
        for _ in 0..crate::services::transcription_jobs::MAX_ACTIVE_JOBS {
            held.push(jobs.provider_permit().await);
        }
        let admission = jobs.clone();
        let registry = CaptureDictations::default();
        let entry = registry
            .get_with(
                JobRequest {
                    artifact_id: metadata.artifact_id,
                    session_id: metadata.session_id,
                    ..request()
                },
                jobs,
                Arc::new(FakeProvider::default()),
                Arc::new(move |request| admission.admit_initial(request.clone(), settings())),
            )
            .unwrap();
        entry.released.store(true, Ordering::Release);
        entry.snapshot(&capture).await;
        assert!(
            entry.run.job().is_none(),
            "a late reader must not submit after release"
        );
        registry.cancel("intent");
        drop(held);
    });
}

#[test]
fn release_and_cancel_stop_deadlines_even_when_stop_event_arrives_late() {
    let registry = CaptureDictations::default();
    let jobs = TranscriptionJobs::default();
    let admission = jobs.clone();
    let entry = registry
        .get_with(
            request(),
            jobs,
            Arc::new(FakeProvider::default()),
            Arc::new(move |request| admission.admit_initial(request.clone(), settings())),
        )
        .unwrap();
    let timer = entry.release_deadline();
    registry.cancel("intent");
    assert_eq!(
        timer.recv_timeout(Duration::ZERO),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    );
    let late_timer = entry.release_deadline();
    assert_eq!(
        late_timer.recv_timeout(Duration::ZERO),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    );
}
