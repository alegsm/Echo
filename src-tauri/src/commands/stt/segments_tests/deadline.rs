use super::*;

fn deadline_event(outcome: &'static str, idle: bool) -> crate::audio::CaptureEvent {
    crate::audio::CaptureEvent {
        elapsed_ms: 0,
        limit_ms: 600_000,
        sequence: 1,
        phase: if idle {
            crate::audio::CapturePhase::Idle
        } else {
            crate::audio::CapturePhase::Recording
        },
        purpose: Some(CapturePurpose::PushToTalk),
        intent_id: (!idle).then(|| "intent".into()),
        session_id: (!idle).then(|| "session".into()),
        artifact_id: (!idle).then(|| "artifact".into()),
        outcome,
        error: None,
        has_observed_callback: true,
        callback_count: 1,
        sample_count: 1,
        last_error: None,
        artifact_count: 1,
        artifact_bytes: 1,
    }
}

struct DeadlineFixture {
    registry: CaptureDictations,
    entry: Arc<CaptureDictation>,
    runtime: crate::services::runtime::EchoRuntime,
    retired: Arc<Mutex<Vec<u64>>>,
}

impl DeadlineFixture {
    async fn new() -> Self {
        let registry = CaptureDictations::default();
        *registry.test_deadlines.lock().unwrap() = Some(TestDeadlines::default());
        let jobs = TranscriptionJobs::default();
        let runtime = crate::services::runtime::EchoRuntime::default();
        let admission = jobs.clone();
        let observed = runtime.clone();
        let entry = registry
            .get_with(
                request(),
                jobs,
                Arc::new(FakeProvider::default()),
                Arc::new(move |request| {
                    let job = admission.admit_initial(request.clone(), settings())?;
                    observed
                        .job_started_for_capture(job.job_sequence, &request.intent_id)
                        .unwrap();
                    Ok(job)
                }),
            )
            .unwrap();
        entry.run.submit(&audio(1, false, None)).unwrap();
        wait_until(|| entry.run.job().is_some()).await;
        Self {
            registry,
            entry,
            runtime,
            retired: Arc::default(),
        }
    }

    fn event(&self, event: &crate::audio::CaptureEvent, timeout: Duration) {
        let runtime = self.runtime.clone();
        let retired = self.retired.clone();
        self.registry.capture_deadline_event(
            event,
            timeout,
            Arc::new(move |jobs| {
                for job in jobs {
                    runtime.job_cancelled(job.job_sequence).unwrap();
                    retired.lock().unwrap().push(job.job_sequence);
                }
            }),
        );
    }

    fn registered(&self) -> bool {
        self.registry.entries.lock().unwrap().contains_key("intent")
    }
}

#[test]
fn release_renews_the_bundle_lease_until_a_late_partial_completion_discards_audio() {
    use crate::audio::tests::fixtures::{begin, owner, spawn_coordinator, FakeBackend};
    use crate::audio::{AudioCapturePolicy, StopCapture};
    use crate::commands::stt::pipeline::{finish_provider_work, read_bundle};
    tauri::async_runtime::block_on(async {
        let (backend, input) = FakeBackend::continuous();
        let capture = spawn_coordinator(
            backend,
            AudioCapturePolicy {
                minimum_capture_duration: Duration::ZERO,
                minimum_sample_count: 0,
                artifact_ttl: Duration::from_millis(800),
                lease_ttl: Duration::from_secs(1),
                ..Default::default()
            },
        );
        capture.freeze_time();
        let owner = owner("overlay");
        let session = begin(&capture, owner.clone(), "intent");
        input.feed(&capture, 0.2, 1_000);
        let metadata = capture
            .stop(StopCapture {
                owner: owner.clone(),
                intent_id: "intent".into(),
                session_id: session.session_id,
            })
            .unwrap();
        let request = JobRequest {
            artifact_id: metadata.artifact_id,
            session_id: metadata.session_id,
            ..request()
        };
        let jobs = TranscriptionJobs::default();
        let registry = jobs.clone();
        let (response, responses) = std::sync::mpsc::channel();
        let provider = Arc::new(FakeProvider {
            fail: Some(b'R'),
            delay_ms: Some(0),
            release: Some(Arc::new(Mutex::new(responses))),
            ..Default::default()
        });
        let run = SegmentedDictation::new(
            request.clone(),
            jobs.clone(),
            provider.clone(),
            Arc::new(move |request| registry.admit_initial(request.clone(), settings())),
        );
        let mut bundle = read_bundle(&capture, &owner, &request, true).await.unwrap();
        run.submit(&bundle.audio).unwrap();
        let initial_lease = bundle.lease_id.clone();
        let completion = async {
            wait_until(|| provider.calls.lock().unwrap().len() == 1).await;
            capture.advance_time(Duration::from_millis(750));
            // Renewal retires the original lease before its virtual expiry.
            wait_until(|| {
                capture
                    .read_artifact(
                        owner.clone(),
                        request.artifact_id.clone(),
                        initial_lease.clone(),
                    )
                    .is_err()
            })
            .await;
            capture.advance_time(Duration::from_millis(750));
            response.send(()).unwrap();
            response.send(()).unwrap();
            run.finish().await
        };
        let result = maintain_lease(
            completion,
            &capture,
            &owner,
            &request,
            &mut bundle,
            Duration::from_millis(250),
        )
        .await;
        let job = run.job().unwrap();
        let (_, result) = result.unwrap();
        assert!(result.partial);
        let completion = finish_provider_work(&capture, &owner, &jobs, &job, bundle, Ok(result))
            .await
            .unwrap();
        assert_eq!(
            completion.provider.provider.transcript,
            "[Transcription gap]"
        );
        let error = read_bundle(&capture, &owner, &request, true)
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, SttJobErrorCode::ArtifactExpired);
        assert_eq!(capture.snapshot().unwrap().artifact_count, 0);
    });
}

#[test]
fn expired_unreleased_capture_does_not_hold_registry_capacity() {
    use crate::audio::tests::fixtures::{begin, owner, spawn_coordinator, FakeBackend};
    use crate::audio::{AudioCapturePolicy, StopCapture};
    tauri::async_runtime::block_on(async {
        let (backend, input) = FakeBackend::continuous();
        let capture = spawn_coordinator(
            backend,
            AudioCapturePolicy {
                minimum_capture_duration: Duration::ZERO,
                minimum_sample_count: 0,
                artifact_ttl: Duration::from_millis(30),
                ..Default::default()
            },
        );
        capture.freeze_time();
        let owner = owner("overlay");
        let session = begin(&capture, owner.clone(), "intent");
        input.feed(&capture, 0.2, 8_000);
        let metadata = capture
            .stop(StopCapture {
                owner,
                intent_id: "intent".into(),
                session_id: session.session_id,
            })
            .unwrap();
        let request = JobRequest {
            artifact_id: metadata.artifact_id,
            session_id: metadata.session_id,
            ..request()
        };
        let jobs = TranscriptionJobs::default();
        let registry = CaptureDictations::default();
        *registry.test_deadlines.lock().unwrap() = Some(TestDeadlines::default());
        let admission = jobs.clone();
        let run = SegmentedDictation::new(
            request.clone(),
            jobs.clone(),
            Arc::new(FakeProvider::default()),
            Arc::new(move |request| admission.admit_initial(request.clone(), settings())),
        );
        let mut bundle = audio(1, false, None);
        bundle.metadata.artifact_id = request.artifact_id.clone();
        bundle.metadata.session_id = request.session_id.clone();
        run.submit(&bundle).unwrap();
        wait_until(|| run.job().is_some()).await;
        registry.entries.lock().unwrap().insert(
            "intent".into(),
            Arc::new(CaptureDictation {
                run: run.clone(),
                request: request.clone(),
                reads: tauri::async_runtime::Mutex::new(()),
                released: std::sync::atomic::AtomicBool::new(false),
                release_timer: Mutex::new(None),
            }),
        );
        capture.advance_time(Duration::from_millis(30));
        assert_eq!(capture.snapshot().unwrap().artifact_count, 0);
        // The release deadline retires the entry without a renderer transcription command.
        let retired = Arc::new(Mutex::new(Vec::new()));
        let observed = retired.clone();
        registry.arm_release_deadline(
            &request.artifact_id,
            Duration::from_millis(20),
            Arc::new(move |jobs| {
                *observed.lock().unwrap() = jobs;
            }),
        );
        registry.advance_deadlines(Duration::from_millis(20)).await;
        assert_eq!(retired.lock().unwrap().len(), 1);
        assert!(registry.entries.lock().unwrap().is_empty());
        assert_eq!(
            run.finish().await.unwrap_err().code,
            SttJobErrorCode::JobCanceled
        );
    });
}

#[test]
fn segment_deadline_renews_through_a_scaled_180_second_hold_then_retires_on_inactivity() {
    tauri::async_runtime::block_on(async {
        let fixture = DeadlineFixture::new().await;
        // One simulated second is 10 ms: 2 s pieces, 60 s inactivity, 180 s held.
        let timeout = Duration::from_millis(600);
        fixture.event(&deadline_event("artifact_created", false), timeout);
        for seconds in (2..=180).step_by(2) {
            fixture
                .registry
                .advance_deadlines(Duration::from_millis(20))
                .await;
            fixture.event(&deadline_event("segment_closed", false), timeout);
            assert!(
                fixture.registered(),
                "expired while speaking at simulated {seconds} s"
            );
            assert!(fixture.retired.lock().unwrap().is_empty());
        }
        fixture
            .registry
            .advance_deadlines(timeout - Duration::from_millis(1))
            .await;
        assert!(
            fixture.registered(),
            "retired before the full inactivity allowance"
        );
        fixture
            .registry
            .advance_deadlines(Duration::from_millis(1))
            .await;
        assert!(!fixture.registered());
        assert_eq!(
            fixture.retired.lock().unwrap().as_slice(),
            &[fixture.entry.run.job().unwrap().job_sequence]
        );
        assert_eq!(
            fixture.runtime.desktop_snapshot().unwrap().snapshot.state,
            crate::services::runtime::EchoState::Idle
        );
        assert_eq!(
            fixture.entry.run.finish().await.unwrap_err().code,
            SttJobErrorCode::JobCanceled
        );
    });
}

#[test]
fn capture_deadline_starts_on_a_segment_or_artifact_and_restarts_on_identityless_idle() {
    tauri::async_runtime::block_on(async {
        for outcome in ["segment_closed", "artifact_created"] {
            let fixture = DeadlineFixture::new().await;
            fixture.event(&deadline_event(outcome, false), Duration::from_millis(40));
            fixture
                .registry
                .advance_deadlines(Duration::from_millis(40))
                .await;
            assert!(!fixture.registered(), "{outcome} must arm a deadline");
        }

        let fixture = DeadlineFixture::new().await;
        let timeout = Duration::from_millis(600);
        fixture.event(&deadline_event("segment_closed", false), timeout);
        fixture.event(&deadline_event("artifact_created", false), timeout);
        fixture
            .registry
            .advance_deadlines(Duration::from_millis(400))
            .await;
        fixture.event(&deadline_event("stream_dropped", true), timeout);
        assert!(fixture.registry.capturing.lock().unwrap().is_none());
        fixture
            .registry
            .advance_deadlines(Duration::from_millis(350))
            .await;
        assert!(
            fixture.registered(),
            "successful idle must restart the full inactivity allowance"
        );
        fixture
            .registry
            .advance_deadlines(Duration::from_millis(250))
            .await;
        assert!(!fixture.registered());
    });
}

#[test]
fn release_cancel_and_error_idle_prevent_late_capture_deadlines() {
    tauri::async_runtime::block_on(async {
        for action in ["release", "cancel", "error"] {
            let fixture = DeadlineFixture::new().await;
            let timeout = Duration::from_millis(40);
            fixture.event(&deadline_event("segment_closed", false), timeout);
            match action {
                "release" => {
                    fixture.entry.release();
                }
                "cancel" => fixture.registry.cancel("intent"),
                _ => {
                    let mut event = deadline_event("capture_failed", true);
                    event.intent_id = Some("intent".into());
                    event.error = Some(crate::audio::CaptureErrorCode::DeviceUnavailable);
                    fixture.event(&event, timeout);
                }
            }
            fixture.event(&deadline_event("artifact_created", false), timeout);
            fixture.event(&deadline_event("stream_dropped", true), timeout);
            fixture
                .registry
                .advance_deadlines(Duration::from_millis(100))
                .await;
            assert!(fixture.retired.lock().unwrap().is_empty(), "{action}");
            assert_eq!(fixture.registered(), action == "release", "{action}");
            fixture.registry.cancel("intent");
        }
    });
}

// Keep: a delayed terminal event cannot retire a newer capture or its registry entry.
#[test]
fn terminal_error_only_cancels_its_own_intent() {
    tauri::async_runtime::block_on(async {
        for event_intent in [Some("old"), Some("intent"), None] {
            let fixture = DeadlineFixture::new().await;
            fixture.event(
                &deadline_event("segment_closed", false),
                Duration::from_secs(60),
            );
            let mut event = deadline_event("capture_failed", true);
            event.intent_id = event_intent.map(str::to_owned);
            event.error = Some(crate::audio::CaptureErrorCode::DeviceUnavailable);
            fixture.event(&event, Duration::from_secs(60));
            assert_eq!(fixture.registered(), event_intent != Some("intent"));
            assert_eq!(
                fixture.registry.capturing.lock().unwrap().as_deref(),
                if event_intent == Some("intent") {
                    None
                } else {
                    Some("intent")
                }
            );
            fixture.registry.cancel("intent");
        }
    });
}
