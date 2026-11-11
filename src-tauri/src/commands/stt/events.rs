use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use crate::commands::events::HistoryCommittedEvent;
use crate::services::dictation::{PartialTranscriptEvent, SegmentPublisher, SttJobErrorCode};
use crate::services::runtime::DesktopStateEnvelope;

const DESKTOP_STATE_EVENT: &str = "echo-desktop-state";
const STT_JOB_EVENT: &str = "echo-stt-job";
const HISTORY_COMMITTED_EVENT: &str = "echo-history-committed";
const PARTIAL_TRANSCRIPT_EVENT: &str = "echo-partial-transcript";

pub(crate) fn overlay_partial_publisher(app: AppHandle) -> SegmentPublisher {
    Arc::new(move |event: PartialTranscriptEvent| {
        emit_partial_to_overlay(
            |target, name, payload| app.emit_to(target, name, payload),
            event,
        );
    })
}

fn emit_partial_to_overlay(
    emit: impl FnOnce(&str, &str, PartialTranscriptEvent) -> tauri::Result<()>,
    event: PartialTranscriptEvent,
) {
    let _ = emit("overlay", PARTIAL_TRANSCRIPT_EVENT, event);
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SttJobEvent {
    job_sequence: Option<u64>,
    session_id: String,
    intent_id: String,
    state: &'static str,
    error: Option<SttJobErrorCode>,
}

pub(super) struct JobEvent<'a> {
    pub job_sequence: Option<u64>,
    pub session_id: &'a str,
    pub intent_id: &'a str,
    pub state: &'static str,
    pub error: Option<SttJobErrorCode>,
}

pub(super) fn emit_state(app: &AppHandle, envelope: &DesktopStateEnvelope) {
    let _ = app.emit(DESKTOP_STATE_EVENT, envelope);
}

pub(super) fn emit_job(app: &AppHandle, event: JobEvent<'_>) {
    let _ = app.emit(
        STT_JOB_EVENT,
        SttJobEvent {
            job_sequence: event.job_sequence,
            session_id: event.session_id.to_string(),
            intent_id: event.intent_id.to_string(),
            state: event.state,
            error: event.error,
        },
    );
}

pub(super) fn emit_history_committed(app: &AppHandle, history_revision: u64, record_id: &str) {
    emit_history_mutation(app, history_revision, record_id, "append");
}

pub(super) fn emit_history_updated(app: &AppHandle, history_revision: u64, record_id: &str) {
    emit_history_mutation(app, history_revision, record_id, "update");
}

fn emit_history_mutation(
    app: &AppHandle,
    history_revision: u64,
    record_id: &str,
    action: &'static str,
) {
    let _ = app.emit(
        HISTORY_COMMITTED_EVENT,
        HistoryCommittedEvent {
            history_revision,
            record_id: Some(record_id),
            action,
            affected_count: Some(1),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_transcript_events_target_only_the_overlay_window() {
        let emitted = std::sync::Mutex::new(Vec::new());
        emit_partial_to_overlay(
            |target, name, payload| {
                emitted
                    .lock()
                    .unwrap()
                    .push((target.to_string(), name.to_string(), payload));
                Ok(())
            },
            PartialTranscriptEvent {
                status: None,
                dictation_id: "intent".into(),
                ordinal: 2,
                text: "segment text".into(),
            },
        );
        assert_eq!(
            *emitted.lock().unwrap(),
            vec![(
                "overlay".to_string(),
                PARTIAL_TRANSCRIPT_EVENT.to_string(),
                PartialTranscriptEvent {
                    status: None,
                    dictation_id: "intent".into(),
                    ordinal: 2,
                    text: "segment text".into(),
                },
            )]
        );
    }
}
