use tauri::{AppHandle, Manager};

use crate::audio::{
    ArtifactReleaseDisposition, ArtifactReleaseRequest, DesktopAudioCapture, TrustedCaptureOwner,
};
use crate::providers::{self, TranscriptionRequest, TranscriptionResult};
use crate::services::dictation::{
    ActiveJob, DictationCompletion, DispatchGate, FrozenJobSettings, JobRequest, SttJobError,
};
use crate::services::runtime::{EchoErrorCode, EchoRuntime};
use crate::services::transcription_jobs::TranscriptionJobs;

use super::errors::*;
use super::events::{emit_job, emit_state, JobEvent};
use super::persistence::{persist_success, SuccessfulTranscription};
use super::settings::resolve_credential;
use super::TranscribeArtifactResult;

pub(super) async fn transcribe_strict_wav(
    app: &AppHandle,
    client: &reqwest::Client,
    settings: FrozenJobSettings,
    request: TranscriptionRequest,
    can_dispatch: DispatchGate,
) -> Result<TranscriptionResult, SttJobError> {
    if !can_dispatch() {
        return Err(job_canceled());
    }
    let api_key = resolve_credential(app, settings.clone()).await?;
    if !can_dispatch() {
        return Err(job_canceled());
    }
    providers::transcribe(client, settings.provider_id, &request, &api_key)
        .await
        .map_err(provider_error)
}

pub(super) fn build_transcription_request(
    audio: &crate::audio::NativeAudioArtifact,
    settings: &FrozenJobSettings,
) -> TranscriptionRequest {
    build_wav_request(audio.whole_wav(), settings)
}

pub(super) fn build_wav_request(
    audio: std::sync::Arc<[u8]>,
    settings: &FrozenJobSettings,
) -> TranscriptionRequest {
    TranscriptionRequest {
        audio,
        mime_type: "audio/wav".to_string(),
        file_name: "echo-dictation.wav".to_string(),
        language: settings.language.clone(),
        prompt: settings.prompt.clone(),
        keyterms: settings.keyterms.clone(),
        diagnostics: settings.diagnostics,
    }
}

pub(super) struct TranscriptionContext<'a> {
    pub app: &'a AppHandle,
    pub jobs: &'a TranscriptionJobs,
    pub capture: &'a DesktopAudioCapture,
    pub owner: &'a TrustedCaptureOwner,
}

pub(super) struct LeasedArtifact {
    pub lease_id: String,
    pub audio: crate::audio::NativeAudioArtifact,
}

pub(super) async fn read_bundle(
    capture: &DesktopAudioCapture,
    owner: &TrustedCaptureOwner,
    request: &JobRequest,
    require_complete: bool,
) -> Result<LeasedArtifact, SttJobError> {
    let capture = capture.clone();
    let owner = owner.clone();
    let artifact_id = request.artifact_id.clone();
    let request = request.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let lease = capture
            .lease_artifact(owner.clone(), artifact_id.clone())
            .map_err(artifact_error)?;
        let audio = capture
            .read_artifact(owner.clone(), artifact_id.clone(), lease.lease_id.clone())
            .map_err(|error| {
                let _ = capture.release_artifact(ArtifactReleaseRequest {
                    owner: owner.clone(),
                    artifact_id: artifact_id.clone(),
                    lease_id: lease.lease_id.clone(),
                    disposition: ArtifactReleaseDisposition::RetainUntilIdleExpiry,
                });
                artifact_error(error)
            })?;
        if !(artifact_matches_job(&audio, &request)
            || (!require_complete && artifact_identity_matches(&audio, &request)))
        {
            let _ = capture.release_artifact(ArtifactReleaseRequest {
                owner,
                artifact_id,
                lease_id: lease.lease_id,
                disposition: ArtifactReleaseDisposition::RetainUntilIdleExpiry,
            });
            return Err(SttJobError::invalid());
        }
        Ok(LeasedArtifact {
            lease_id: lease.lease_id,
            audio,
        })
    })
    .await
    .map_err(|_| artifact_unavailable())?
}

fn artifact_matches_job(audio: &crate::audio::NativeAudioArtifact, request: &JobRequest) -> bool {
    audio.is_complete && artifact_identity_matches(audio, request)
}

fn artifact_identity_matches(
    audio: &crate::audio::NativeAudioArtifact,
    request: &JobRequest,
) -> bool {
    audio.metadata.artifact_id == request.artifact_id
        && audio.metadata.session_id == request.session_id
        && audio.metadata.intent_id == request.artifact_intent_id
}

pub(super) async fn finish_transcription(
    context: &TranscriptionContext<'_>,
    job: ActiveJob,
    artifact: LeasedArtifact,
    result: Result<DictationCompletion, SttJobError>,
) -> Result<TranscribeArtifactResult, SttJobError> {
    let completion = match finish_provider_work(
        context.capture,
        context.owner,
        context.jobs,
        &job,
        artifact,
        result,
    )
    .await
    {
        Ok(completion) => completion,
        Err(error) => return finish_failure(context.app, context.jobs, &job, error),
    };
    persist_success(context.app, context.jobs, job, completion)
}

pub(super) async fn finish_provider_work(
    capture: &DesktopAudioCapture,
    owner: &TrustedCaptureOwner,
    jobs: &TranscriptionJobs,
    job: &ActiveJob,
    artifact: LeasedArtifact,
    result: Result<DictationCompletion, SttJobError>,
) -> Result<SuccessfulTranscription, SttJobError> {
    let provider = match result {
        Ok(result) => result,
        Err(error) => {
            let _ = release(
                ArtifactCleanup::new(capture, owner, job, &artifact),
                ArtifactReleaseDisposition::Discard,
            )
            .await;
            return Err(error);
        }
    };
    let cleanup_warning = release(
        ArtifactCleanup::new(capture, owner, job, &artifact),
        ArtifactReleaseDisposition::Discard,
    )
    .await
    .err()
    .map(|_| EchoErrorCode::CaptureFailed);
    if !jobs.complete_success(job)? {
        return Err(job_canceled());
    }
    Ok(SuccessfulTranscription {
        audio: artifact.audio.metadata,
        provider,
        cleanup_warning,
    })
}

struct ArtifactCleanup<'a> {
    capture: &'a DesktopAudioCapture,
    owner: &'a TrustedCaptureOwner,
    job: &'a ActiveJob,
    artifact: &'a LeasedArtifact,
}

impl<'a> ArtifactCleanup<'a> {
    fn new(
        capture: &'a DesktopAudioCapture,
        owner: &'a TrustedCaptureOwner,
        job: &'a ActiveJob,
        artifact: &'a LeasedArtifact,
    ) -> Self {
        Self {
            capture,
            owner,
            job,
            artifact,
        }
    }
}

async fn release(
    cleanup: ArtifactCleanup<'_>,
    disposition: ArtifactReleaseDisposition,
) -> Result<(), SttJobError> {
    let capture = cleanup.capture.clone();
    let owner = cleanup.owner.clone();
    let artifact_id = cleanup.job.request.artifact_id.clone();
    let lease_id = cleanup.artifact.lease_id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        capture
            .release_artifact(ArtifactReleaseRequest {
                owner,
                artifact_id,
                lease_id,
                disposition,
            })
            .map_err(artifact_error)
    })
    .await
    .map_err(|_| artifact_unavailable())?
}

pub(super) fn emit_started(app: &AppHandle, job: &ActiveJob) -> Result<(), SttJobError> {
    let snapshot = app
        .state::<EchoRuntime>()
        .job_started_for_capture(job.job_sequence, &job.request.artifact_intent_id)
        .map_err(|_| runtime_unavailable())?;
    emit_state(app, &snapshot);
    emit_job(
        app,
        JobEvent {
            job_sequence: Some(job.job_sequence),
            session_id: &job.request.session_id,
            intent_id: &job.request.intent_id,
            state: "transcribing",
            error: None,
        },
    );
    Ok(())
}

pub(super) fn finish_failure<T>(
    app: &AppHandle,
    jobs: &TranscriptionJobs,
    job: &ActiveJob,
    error: SttJobError,
) -> Result<T, SttJobError> {
    if !jobs.complete_failure(job)? {
        return finish_canceled(app, job);
    }
    if matches!(
        error.code,
        crate::services::dictation::SttJobErrorCode::MissingProviderKey
            | crate::services::dictation::SttJobErrorCode::ProviderCredentialRejected
    ) {
        super::settings::persist_provider_status(app, &job.settings, "invalid");
    }
    let snapshot = app
        .state::<EchoRuntime>()
        .job_failed(job.job_sequence, public_error(error.code))
        .map_err(|_| runtime_unavailable())?;
    emit_state(app, &snapshot);
    emit_job(
        app,
        JobEvent {
            job_sequence: Some(job.job_sequence),
            session_id: &job.request.session_id,
            intent_id: &job.request.intent_id,
            state: "failed",
            error: Some(error.code),
        },
    );
    Err(error)
}

fn finish_canceled<T>(app: &AppHandle, job: &ActiveJob) -> Result<T, SttJobError> {
    let snapshot = app
        .state::<EchoRuntime>()
        .job_cancelled(job.job_sequence)
        .map_err(|_| runtime_unavailable())?;
    emit_state(app, &snapshot);
    emit_job(
        app,
        JobEvent {
            job_sequence: Some(job.job_sequence),
            session_id: &job.request.session_id,
            intent_id: &job.request.intent_id,
            state: "canceled",
            error: None,
        },
    );
    Err(job_canceled())
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;
