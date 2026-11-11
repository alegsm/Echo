use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::providers::{self, ProviderErrorCode, ProviderId, TranscriptionRequest};
use crate::services::dictation::SttJobError;

use super::errors::provider_error;
use super::settings::{freeze_provider_settings, read_persisted_settings, resolve_credential};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProbeProviderRequest {
    provider_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeProviderResponse {
    ok: bool,
    provider_id: String,
    provider_model: String,
    timings: ProbeTimings,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeTimings {
    provider_ms: u128,
    backend_ms: u128,
}

pub async fn probe_provider(
    request: ProbeProviderRequest,
    app: AppHandle,
) -> Result<ProbeProviderResponse, SttJobError> {
    let started_at = Instant::now();
    let provider_id =
        ProviderId::try_from(request.provider_id.as_str()).map_err(|_| SttJobError::invalid())?;
    let settings = freeze_provider_settings(&read_persisted_settings(&app)?, provider_id)?;
    let api_key = resolve_credential(&app, settings).await?;
    let provider_started_at = Instant::now();
    let probe = crate::audio::encode_wav(&vec![0.0; 12_800], 16_000, 1);
    let client = app.state::<reqwest::Client>();
    let result = providers::transcribe(
        &client,
        provider_id,
        &TranscriptionRequest {
            audio: Arc::from(probe),
            mime_type: "audio/wav".to_string(),
            file_name: "echo-provider-probe.wav".to_string(),
            language: Some("en".to_string()),
            prompt: None,
            keyterms: Vec::new(),
            diagnostics: false,
        },
        &api_key,
    )
    .await;
    let provider_ms = provider_started_at.elapsed().as_millis();
    match result {
        Ok(result) => Ok(response(
            result.provider_id,
            result.provider_model,
            result.provider_ms,
            started_at,
        )),
        Err(error)
            if matches!(
                error.code,
                ProviderErrorCode::EmptyTranscript | ProviderErrorCode::LowConfidence
            ) =>
        {
            Ok(response(
                provider_id,
                provider_id.model(),
                provider_ms,
                started_at,
            ))
        }
        Err(error) => Err(provider_error(error)),
    }
}

fn response(
    provider_id: ProviderId,
    provider_model: &str,
    provider_ms: u128,
    started_at: Instant,
) -> ProbeProviderResponse {
    ProbeProviderResponse {
        ok: true,
        provider_id: provider_id.as_str().to_string(),
        provider_model: provider_model.to_string(),
        timings: ProbeTimings {
            provider_ms,
            backend_ms: started_at.elapsed().as_millis(),
        },
    }
}
