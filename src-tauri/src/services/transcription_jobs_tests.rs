use crate::providers::ProviderId;

use super::*;

#[test]
fn cancellation_before_admission_is_remembered() {
    let jobs = TranscriptionJobs::default();
    assert!(jobs
        .cancel("dashboard", "session-1", "intent-1")
        .unwrap()
        .is_none());

    let error = jobs
        .admit_initial(request("intent-1", "session-1", "artifact-1"), settings())
        .expect_err("tombstone must reject late admission");

    assert_eq!(error.code, SttJobErrorCode::JobCanceled);
    let duplicate = jobs
        .admit_initial(request("intent-1", "session-1", "artifact-1"), settings())
        .expect_err("tombstone must reject every duplicate until expiry");
    assert_eq!(duplicate.code, SttJobErrorCode::JobCanceled);
}

#[test]
fn canceled_running_work_remains_inside_the_global_cap() {
    let jobs = TranscriptionJobs::default();
    // Six active jobs exhaust admission even when one is canceled but still running.
    let active: Vec<_> = (0..6)
        .map(|index| {
            jobs.admit_initial(
                request(
                    &format!("intent-{index}"),
                    &format!("session-{index}"),
                    &format!("artifact-{index}"),
                ),
                settings(),
            )
            .unwrap()
        })
        .collect();
    jobs.cancel("dashboard", "session-0", "intent-0").unwrap();

    let error = jobs
        .admit_initial(request("intent-6", "session-6", "artifact-6"), settings())
        .expect_err("canceled provider work must still count as active");
    assert_eq!(error.code, SttJobErrorCode::TooManyJobs);

    assert!(!jobs.complete_success(&active[0]).unwrap());
    jobs.admit_initial(request("intent-6", "session-6", "artifact-6"), settings())
        .expect("capacity should return after provider completion");
}

#[test]
fn delivery_proof_is_bound_and_consumed_once() {
    let jobs = TranscriptionJobs::default();
    let active = jobs
        .admit_initial(request("intent-1", "session-1", "artifact-1"), settings())
        .unwrap();
    jobs.authorize_delivery(&active, "transcript-1", 7).unwrap();

    let invalid = jobs.delivery_revision("dashboard", "session-other", "intent-1", "transcript-1");
    assert_eq!(invalid.unwrap_err().code, SttJobErrorCode::InvalidJob);
    assert_eq!(
        jobs.delivery_revision("dashboard", "session-1", "intent-1", "transcript-1")
            .unwrap(),
        7
    );
    jobs.complete_delivery("dashboard", "intent-1").unwrap();
    assert!(jobs
        .delivery_revision("dashboard", "session-1", "intent-1", "transcript-1")
        .is_err());
}

fn request(intent_id: &str, session_id: &str, artifact_id: &str) -> JobRequest {
    JobRequest {
        owner: "dashboard".to_string(),
        artifact_id: artifact_id.to_string(),
        session_id: session_id.to_string(),
        intent_id: intent_id.to_string(),
        artifact_intent_id: intent_id.to_string(),
        created_at: "2026-09-09T12:00:00Z".to_string(),
    }
}

fn settings() -> FrozenJobSettings {
    FrozenJobSettings {
        provider_id: ProviderId::GroqWhisperTurbo,
        language: Some("en".to_string()),
        prompt: Some("Echo, Sample".to_string()),
        keyterms: Vec::new(),
        paste_after_transcription: true,
        restore_clipboard: true,
        diagnostics: false,
    }
}

#[test]
fn failed_initial_intent_cannot_be_readmitted() {
    let jobs = TranscriptionJobs::default();
    let original = request("one", "session", "artifact");
    let job = jobs.admit_initial(original.clone(), settings()).unwrap();
    jobs.complete_failure(&job).unwrap();
    assert_eq!(
        jobs.admit_initial(original, settings()).unwrap_err().code,
        SttJobErrorCode::JobCanceled
    );
    assert!(jobs
        .admit_initial(request("new", "session-new", "artifact-new"), settings())
        .is_ok());
}

#[test]
fn segments_repair_slot_leaves_five_permits_and_returns_on_drop() {
    use std::future::Future;
    tauri::async_runtime::block_on(async {
        let jobs = TranscriptionJobs::default();
        let repair = jobs.repair_slot().await;
        let permit = jobs.provider_permit().await;
        let mut another = Box::pin(jobs.repair_slot());
        std::future::poll_fn(|cx| {
            assert!(another.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        let mut chunks = Vec::new();
        for _ in 0..5 {
            chunks.push(jobs.provider_permit().await);
        }
        let mut seventh = Box::pin(jobs.provider_permit());
        std::future::poll_fn(|cx| {
            assert!(seventh.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        drop(permit);
        let returned = seventh.await;
        drop(repair);
        let next_repair = another.await;
        drop((returned, next_repair, chunks));
    });
}
