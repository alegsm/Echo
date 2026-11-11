mod errors;
pub(crate) mod events;
mod persistence;
mod pipeline;
mod probe;
pub(crate) mod segments;
mod settings;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, WebviewWindow};

use crate::audio::TrustedCaptureOwner;
use crate::services::dictation::SttJobError;
use crate::services::runtime::{DesktopStateEnvelope, EchoRuntime};
use crate::services::transcription_jobs::TranscriptionJobs;
use crate::storage::{EchoStorage, StoredTranscript, TranscriptRecord};

pub use probe::{ProbeProviderRequest, ProbeProviderResponse};

use errors::runtime_unavailable;
use events::{emit_job, emit_state, JobEvent};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscribeArtifactRequest {
    pub(super) artifact_id: String,
    pub(super) session_id: String,
    pub(super) intent_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscribeArtifactResult {
    pub(super) job_sequence: u64,
    pub(super) transcript: TranscriptRecord,
    pub(super) snapshot: DesktopStateEnvelope,
    pub(super) delivery: DeliveryPlan,
    pub(super) persistence: PersistenceStatus,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryPlan {
    paste_after_transcription: bool,
    restore_clipboard: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistenceStatus {
    saved: bool,
    error_code: Option<&'static str>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelTranscriptionRequest {
    session_id: String,
    intent_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelTranscriptionResult {
    canceled: bool,
    snapshot: DesktopStateEnvelope,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportTranscriptDeliveryRequest {
    session_id: String,
    intent_id: String,
    transcript_id: String,
    insertion_mode: String,
    error_code: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportTranscriptDeliveryResult {
    history_revision: u64,
    transcript: StoredTranscript,
}

#[tauri::command]
pub fn get_desktop_runtime_snapshot(app: AppHandle) -> Result<DesktopStateEnvelope, SttJobError> {
    app.state::<EchoRuntime>()
        .desktop_snapshot()
        .map_err(|_| runtime_unavailable())
}

#[tauri::command]
pub async fn transcribe_audio_artifact(
    request: TranscribeArtifactRequest,
    window: WebviewWindow,
    app: AppHandle,
) -> Result<TranscribeArtifactResult, SttJobError> {
    let owner = TrustedCaptureOwner::from_window_label(window.label());
    segments::execute(&app, &owner, request).await
}

#[tauri::command]
pub async fn probe_provider(
    request: ProbeProviderRequest,
    app: AppHandle,
) -> Result<ProbeProviderResponse, SttJobError> {
    let jobs = app.state::<TranscriptionJobs>();
    let _permit = jobs.provider_permit().await;
    probe::probe_provider(request, app.clone()).await
}

#[tauri::command]
pub fn cancel_transcription_job(
    request: CancelTranscriptionRequest,
    window: WebviewWindow,
    app: AppHandle,
) -> Result<CancelTranscriptionResult, SttJobError> {
    let jobs = app.state::<TranscriptionJobs>();
    if window.label() == "overlay" {
        app.state::<segments::CaptureDictations>()
            .validate_session(&request.intent_id, Some(&request.session_id))?;
    }
    let sequence = jobs.cancel(window.label(), &request.session_id, &request.intent_id)?;
    if window.label() == "overlay" {
        app.state::<segments::CaptureDictations>()
            .cancel(&request.intent_id);
    }
    let runtime = app.state::<EchoRuntime>();
    let snapshot = match sequence {
        Some(value) => runtime.job_cancelled(value),
        None => runtime.desktop_snapshot(),
    }
    .map_err(|_| runtime_unavailable())?;
    emit_state(&app, &snapshot);
    emit_job(
        &app,
        JobEvent {
            job_sequence: sequence,
            session_id: &request.session_id,
            intent_id: &request.intent_id,
            state: "canceled",
            error: None,
        },
    );
    Ok(CancelTranscriptionResult {
        canceled: true,
        snapshot,
    })
}

#[tauri::command]
pub fn report_transcript_delivery(
    request: ReportTranscriptDeliveryRequest,
    window: WebviewWindow,
    app: AppHandle,
) -> Result<ReportTranscriptDeliveryResult, SttJobError> {
    let jobs = app.state::<TranscriptionJobs>();
    let revision = jobs.delivery_revision(
        window.label(),
        &request.session_id,
        &request.intent_id,
        &request.transcript_id,
    )?;
    let mutation = app
        .state::<EchoStorage>()
        .update_transcript_delivery(
            &request.transcript_id,
            revision,
            &request.insertion_mode,
            request.error_code.as_deref(),
        )
        .map_err(errors::history_error)?;
    let transcript = mutation
        .transcript
        .ok_or_else(|| errors::history_error(crate::storage::StorageError::Database))?;
    jobs.complete_delivery(window.label(), &request.intent_id)?;
    if let Some(snapshot) = app
        .state::<EchoRuntime>()
        .replace_last_transcript_if_current(transcript.revision, transcript.record.clone())
        .map_err(|_| runtime_unavailable())?
    {
        emit_state(&app, &snapshot);
    }
    events::emit_history_updated(&app, mutation.history_revision, &request.transcript_id);
    Ok(ReportTranscriptDeliveryResult {
        history_revision: mutation.history_revision,
        transcript,
    })
}
