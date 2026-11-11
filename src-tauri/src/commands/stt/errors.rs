use crate::audio::{CaptureError, CaptureErrorCode};
use crate::providers::{ProviderError, ProviderErrorCode};
use crate::services::dictation::{SttJobError, SttJobErrorCode, SttJobStage};
use crate::services::runtime::EchoErrorCode;
use crate::storage::StorageError;

pub(super) fn provider_error(error: ProviderError) -> SttJobError {
    let (code, retryable) = match error.code {
        ProviderErrorCode::UnsupportedProvider => (SttJobErrorCode::UnsupportedProvider, false),
        ProviderErrorCode::MissingCredential => (SttJobErrorCode::MissingProviderKey, false),
        ProviderErrorCode::CredentialRejected => {
            (SttJobErrorCode::ProviderCredentialRejected, false)
        }
        ProviderErrorCode::InvalidAudio => (SttJobErrorCode::InvalidAudio, false),
        ProviderErrorCode::Timeout => (SttJobErrorCode::ProviderTimeout, true),
        ProviderErrorCode::RequestFailed => (SttJobErrorCode::ProviderFailed, true),
        ProviderErrorCode::InvalidResponse => (SttJobErrorCode::ProviderInvalidResponse, true),
        ProviderErrorCode::EmptyTranscript => (SttJobErrorCode::ProviderEmptyTranscript, true),
        ProviderErrorCode::LowConfidence | ProviderErrorCode::Silence => {
            (SttJobErrorCode::ProviderLowConfidence, true)
        }
    };
    SttJobError::new(
        code,
        SttJobStage::Provider,
        retryable,
        provider_message(code),
    )
}

pub(super) fn artifact_error(error: CaptureError) -> SttJobError {
    let code = match error.code {
        CaptureErrorCode::ArtifactExpired => SttJobErrorCode::ArtifactExpired,
        CaptureErrorCode::InvalidOwner => SttJobErrorCode::ArtifactNotFound,
        CaptureErrorCode::ArtifactCapacityReached => SttJobErrorCode::ArtifactBusy,
        _ => SttJobErrorCode::ArtifactNotFound,
    };
    SttJobError::new(code, SttJobStage::Artifact, false, artifact_message(code))
}

pub(super) fn credential_error(message: String) -> SttJobError {
    if message.starts_with("Add and verify") {
        return SttJobError::new(
            SttJobErrorCode::MissingProviderKey,
            SttJobStage::Credential,
            false,
            "Add and verify an API key for the selected provider before recording.",
        );
    }
    credential_unavailable()
}

pub(super) fn history_error(_: StorageError) -> SttJobError {
    SttJobError::new(
        SttJobErrorCode::HistoryWriteFailed,
        SttJobStage::History,
        false,
        "The transcript could not be saved.",
    )
}

pub(super) fn public_error(code: SttJobErrorCode) -> EchoErrorCode {
    match code {
        SttJobErrorCode::MissingProviderKey | SttJobErrorCode::ProviderCredentialRejected => {
            EchoErrorCode::MissingProviderKey
        }
        SttJobErrorCode::ProviderTimeout => EchoErrorCode::ProviderTimeout,
        SttJobErrorCode::HistoryWriteFailed => EchoErrorCode::HistoryWriteFailed,
        SttJobErrorCode::InvalidAudio => EchoErrorCode::EmptyAudio,
        _ => EchoErrorCode::ProviderFailed,
    }
}

pub(super) const fn provider_message(code: SttJobErrorCode) -> &'static str {
    match code {
        SttJobErrorCode::ProviderCredentialRejected => {
            "The transcription provider rejected the API key."
        }
        SttJobErrorCode::ProviderTimeout => "The transcription provider timed out.",
        SttJobErrorCode::ProviderInvalidResponse => {
            "The transcription provider returned an invalid response."
        }
        SttJobErrorCode::ProviderEmptyTranscript => {
            "The transcription provider returned empty text."
        }
        SttJobErrorCode::ProviderLowConfidence => {
            "The recording contained too little confident speech."
        }
        _ => "The transcription provider failed.",
    }
}

pub(super) const fn artifact_message(code: SttJobErrorCode) -> &'static str {
    match code {
        SttJobErrorCode::ArtifactExpired => "The recording expired. Record again.",
        SttJobErrorCode::ArtifactBusy => "Recorded audio is temporarily busy.",
        _ => "Recorded audio is unavailable.",
    }
}

pub(super) const fn settings_unavailable() -> SttJobError {
    SttJobError::new(
        SttJobErrorCode::InvalidJob,
        SttJobStage::Settings,
        false,
        "Transcription settings are unavailable.",
    )
}

pub(super) const fn credential_unavailable() -> SttJobError {
    SttJobError::new(
        SttJobErrorCode::CredentialUnavailable,
        SttJobStage::Credential,
        false,
        "The selected provider credential is unavailable.",
    )
}

pub(super) const fn artifact_unavailable() -> SttJobError {
    SttJobError::new(
        SttJobErrorCode::ArtifactNotFound,
        SttJobStage::Artifact,
        false,
        "Recorded audio is unavailable.",
    )
}

pub(super) const fn runtime_unavailable() -> SttJobError {
    SttJobError::new(
        SttJobErrorCode::ProviderFailed,
        SttJobStage::Provider,
        true,
        "Desktop transcription state is unavailable.",
    )
}

pub(super) use crate::services::transcription_jobs::job_canceled;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_recording_requires_a_new_recording() {
        let error = artifact_error(CaptureError::new(
            CaptureErrorCode::ArtifactExpired,
            "artifact_read",
        ));
        assert_eq!(error.message, "The recording expired. Record again.");
        assert!(!error.retryable);
    }

    #[test]
    fn rejected_and_missing_credentials_keep_a_distinct_public_error() {
        for provider_code in [
            ProviderErrorCode::CredentialRejected,
            ProviderErrorCode::MissingCredential,
        ] {
            let error = provider_error(ProviderError::new(provider_code, "sanitized"));
            assert_eq!(public_error(error.code), EchoErrorCode::MissingProviderKey);
        }
        assert_eq!(
            public_error(SttJobErrorCode::ProviderFailed),
            EchoErrorCode::ProviderFailed
        );
        assert_eq!(
            public_error(SttJobErrorCode::ProviderTimeout),
            EchoErrorCode::ProviderTimeout
        );
    }
}
