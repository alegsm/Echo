use tauri::{AppHandle, Manager};

use crate::services::dictation::{ActiveJob, DictationCompletion, SttJobError};
use crate::services::runtime::{DesktopStateEnvelope, EchoErrorCode, EchoRuntime};
use crate::services::transcription_jobs::TranscriptionJobs;
use crate::storage::{EchoStorage, TranscriptAudioMetadata, TranscriptRecord, TranscriptTiming};

use super::errors::runtime_unavailable;
use super::events::{emit_history_committed, emit_job, emit_state, JobEvent};
use super::{DeliveryPlan, PersistenceStatus, TranscribeArtifactResult};

pub(super) struct SuccessfulTranscription {
    pub audio: crate::audio::AudioArtifactMetadata,
    pub provider: DictationCompletion,
    pub cleanup_warning: Option<EchoErrorCode>,
}

pub(super) fn persist_success(
    app: &AppHandle,
    jobs: &TranscriptionJobs,
    job: ActiveJob,
    completion: SuccessfulTranscription,
) -> Result<TranscribeArtifactResult, SttJobError> {
    let provider_status = if completion.provider.credential_rejected {
        Some("invalid")
    } else if completion.provider.has_speech {
        Some("valid")
    } else {
        None
    };
    if let Some(status) = provider_status {
        super::settings::persist_provider_status(app, &job.settings, status);
    }
    let has_speech = completion.provider.has_speech;
    let record = build_record(&job, &completion.audio, completion.provider);
    let persistence = app.state::<EchoStorage>().append_transcript(record.clone());
    let saved = persistence.is_ok();
    let runtime_completion = RuntimeCompletion {
        record_revision: persistence
            .as_ref()
            .ok()
            .and_then(|mutation| mutation.transcript.as_ref())
            .map(|stored| stored.revision),
        warning: completion_warning(
            saved,
            record.error_code.is_some(),
            completion.cleanup_warning,
        ),
    };
    let snapshot = complete_runtime(app, &job, &record, runtime_completion)?;
    emit_completion(
        app,
        jobs,
        CompletionEvent {
            job: &job,
            record: &record,
            persistence: persistence.as_ref().ok(),
            has_speech,
            snapshot: &snapshot,
        },
    );
    Ok(TranscribeArtifactResult {
        job_sequence: job.job_sequence,
        transcript: record,
        snapshot,
        delivery: DeliveryPlan {
            paste_after_transcription: job.settings.paste_after_transcription,
            restore_clipboard: job.settings.restore_clipboard,
        },
        persistence: PersistenceStatus {
            saved,
            error_code: (!saved).then_some("history_write_failed"),
        },
    })
}

pub(super) fn completion_warning(
    saved: bool,
    partial: bool,
    cleanup_warning: Option<EchoErrorCode>,
) -> Option<EchoErrorCode> {
    if cleanup_warning.is_some() {
        log::warn!("capture_cleanup_failed code=capture_failed");
    }
    if !saved {
        Some(EchoErrorCode::HistoryWriteFailed)
    } else if partial {
        Some(EchoErrorCode::ProviderFailed)
    } else {
        cleanup_warning
    }
}

#[derive(Clone, Copy)]
struct RuntimeCompletion {
    record_revision: Option<u64>,
    warning: Option<EchoErrorCode>,
}

fn complete_runtime(
    app: &AppHandle,
    job: &ActiveJob,
    record: &TranscriptRecord,
    completion: RuntimeCompletion,
) -> Result<DesktopStateEnvelope, SttJobError> {
    let runtime = app.state::<EchoRuntime>();
    let result = match completion.warning {
        Some(warning) => runtime.job_completed_with_warning(
            job.job_sequence,
            record.to_owned(),
            completion.record_revision,
            warning,
        ),
        None => runtime.job_completed(
            job.job_sequence,
            record.to_owned(),
            completion.record_revision,
        ),
    };
    result.map_err(|_| runtime_unavailable())
}

struct CompletionEvent<'a> {
    job: &'a ActiveJob,
    record: &'a TranscriptRecord,
    persistence: Option<&'a crate::storage::HistoryMutation>,
    snapshot: &'a DesktopStateEnvelope,
    has_speech: bool,
}

fn emit_completion(app: &AppHandle, jobs: &TranscriptionJobs, event: CompletionEvent<'_>) {
    emit_state(app, event.snapshot);
    emit_job(
        app,
        JobEvent {
            job_sequence: Some(event.job.job_sequence),
            session_id: &event.job.request.session_id,
            intent_id: &event.job.request.intent_id,
            state: "completed",
            error: None,
        },
    );
    if let Some(mutation) = event.persistence {
        emit_history_committed(app, mutation.history_revision, &event.record.id);
        if let Some(stored) = mutation.transcript.as_ref().filter(|_| event.has_speech) {
            let _ = jobs.authorize_delivery(event.job, &event.record.id, stored.revision);
        }
    }
}

pub(super) fn build_record(
    job: &ActiveJob,
    audio: &crate::audio::AudioArtifactMetadata,
    completion: DictationCompletion,
) -> TranscriptRecord {
    let provider = completion.provider;
    TranscriptRecord {
        id: uuid::Uuid::new_v4().to_string(),
        text: provider.transcript,
        created_at: job.request.created_at.clone(),
        duration_ms: audio.duration_ms,
        provider_id: provider.provider_id.as_str().to_string(),
        provider_model: Some(provider.provider_model.to_string()),
        language: job.settings.language.clone(),
        prompt: job.settings.prompt.clone(),
        timings: TranscriptTiming {
            recording_ms: audio.duration_ms as f64,
            blob_ready_ms: 0.0,
            upload_ms: 0.0,
            stop_to_transcript_ms: completion.stop_to_transcript_ms as f64,
            history_write_ms: None,
            provider_ms: Some(provider.provider_ms as f64),
            backend_ms: Some(provider.provider_ms as f64),
        },
        audio: TranscriptAudioMetadata {
            mime_type: audio.mime_type.to_string(),
            byte_size: audio.byte_size as u64,
        },
        insertion_mode: if completion.has_speech {
            "manual"
        } else {
            "failed"
        }
        .to_string(),
        source: "desktop".to_string(),
        error_code: completion.partial.then(|| "provider_failed".into()),
    }
}
