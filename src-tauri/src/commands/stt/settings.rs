use tauri::{AppHandle, Manager};

use crate::providers::ProviderId;
use crate::services::credentials;
use crate::services::dictation::{FrozenJobSettings, SttJobError, SttJobErrorCode, SttJobStage};
use crate::storage::{EchoStorage, PersistedSettings};

use super::errors::{credential_error, credential_unavailable, settings_unavailable};

pub(super) fn read_frozen_settings(app: &AppHandle) -> Result<FrozenJobSettings, SttJobError> {
    let settings = read_persisted_settings(app)?;
    freeze_settings(settings)
}

pub(super) fn read_persisted_settings(app: &AppHandle) -> Result<PersistedSettings, SttJobError> {
    let storage = app.state::<EchoStorage>();
    Ok(storage
        .read_settings()
        .map_err(|_| settings_unavailable())?
        .settings)
}

fn freeze_settings(settings: PersistedSettings) -> Result<FrozenJobSettings, SttJobError> {
    let provider_id = ProviderId::try_from(settings.provider_id.as_str()).map_err(|_| {
        SttJobError::new(
            SttJobErrorCode::UnsupportedProvider,
            SttJobStage::Settings,
            false,
            "Unsupported transcription provider.",
        )
    })?;
    freeze_provider_settings(&settings, provider_id)
}

pub(super) fn freeze_provider_settings(
    settings: &PersistedSettings,
    provider_id: ProviderId,
) -> Result<FrozenJobSettings, SttJobError> {
    let prompt = transcription_prompt(provider_id, settings);
    Ok(FrozenJobSettings {
        provider_id,
        language: settings.language.clone(),
        prompt,
        keyterms: settings.phrasebook_terms.clone(),
        paste_after_transcription: settings.paste_after_transcription,
        restore_clipboard: settings.restore_clipboard,
        diagnostics: settings.debug_logging_enabled,
    })
}

fn transcription_prompt(provider_id: ProviderId, settings: &PersistedSettings) -> Option<String> {
    if !provider_id.accepts_prompt() {
        return None;
    }
    let phrasebook = if settings.phrasebook_terms.is_empty() {
        None
    } else {
        let terms = settings.phrasebook_terms.join(", ");
        // The sentence is English, and Groq expects the prompt in the audio's language, so
        // it is sent only when English is the saved language. Auto-detect and every other
        // language get the bare terms, which carry no language of their own.
        Some(if saved_language_is_english(settings) {
            format!(
                "This is English dictation for software and product work. Use these exact spellings when heard: {terms}."
            )
        } else {
            format!("{terms}.")
        })
    };
    let prompt = [settings.custom_prompt.clone(), phrasebook]
        .into_iter()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!prompt.is_empty()).then_some(prompt)
}

/// True when the saved language's primary subtag is "en" ("en", "en-US", "en-GB").
fn saved_language_is_english(settings: &PersistedSettings) -> bool {
    settings
        .language
        .as_deref()
        .and_then(|tag| tag.split('-').next())
        .is_some_and(|primary| primary.eq_ignore_ascii_case("en"))
}

pub(super) async fn resolve_credential(
    app: &AppHandle,
    settings: FrozenJobSettings,
) -> Result<String, SttJobError> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        resolve_credential_with(
            &app.state::<credentials::CredentialService>(),
            &app.state::<EchoStorage>(),
            settings,
            |update| crate::commands::storage::events::emit_settings(&app, update),
        )
    })
    .await
    .map_err(|_| credential_unavailable())?
}

fn resolve_credential_with(
    service: &credentials::CredentialService,
    storage: &EchoStorage,
    settings: FrozenJobSettings,
    publish: impl FnOnce(&crate::storage::SettingsUpdate),
) -> Result<String, SttJobError> {
    service
        .resolve(storage, settings.provider_id, publish)
        .map_err(credential_error)
}

// Status writes belong to native STT: the overlay cannot call dashboard-only settings commands.
pub(super) fn persist_provider_status(app: &AppHandle, settings: &FrozenJobSettings, status: &str) {
    let storage = app.state::<EchoStorage>();
    match save_provider_status(&storage, settings, status) {
        Ok(Some(update)) => {
            crate::commands::storage::events::emit_settings(app, &update);
        }
        Ok(None) => {}
        Err(_) => eprintln!("Provider credential status could not be saved."),
    }
}

fn save_provider_status(
    storage: &EchoStorage,
    frozen: &FrozenJobSettings,
    status: &str,
) -> crate::storage::StorageResult<Option<crate::storage::SettingsUpdate>> {
    loop {
        let current = storage.read_settings()?;
        let id = frozen.provider_id.as_str();
        if current
            .settings
            .provider_key_statuses
            .get(id)
            .is_some_and(|saved| saved == status)
        {
            return Ok(None);
        }
        match storage.update_settings(
            current.revision,
            crate::storage::SettingsPatch {
                provider_key_statuses: Some(
                    [(id.to_owned(), status.to_owned())].into_iter().collect(),
                ),
                ..Default::default()
            },
        ) {
            Err(crate::storage::StorageError::RevisionConflict { .. }) => continue,
            result => return result.map(Some),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_support::{cleanup, initialized, test_database};

    // Keep: explicit check and transcription share missing-vs-denied reconciliation and public error codes.
    #[test]
    fn credentials_resolution_distinguishes_missing_items_from_access_denial() {
        use crate::services::credentials::{
            tests::CountingStore, CredentialService, CredentialStore,
        };
        for provider in ProviderId::ALL {
            for denied in [false, true] {
                let path = test_database("echo-resolve");
                let storage = initialized(&path);
                let store = CountingStore::default();
                let service = CredentialService::new(store.clone());
                service
                    .save(&storage, provider, "synthetic-fixture", |_| {})
                    .unwrap();
                if denied {
                    store.deny_reads(true);
                } else {
                    store.delete("app.echostt.desktop", provider).unwrap();
                }
                store.reset_calls();
                let updates = std::cell::Cell::new(0);
                let settings =
                    freeze_provider_settings(&storage.read_settings().unwrap().settings, provider)
                        .unwrap();
                let error = resolve_credential_with(&service, &storage, settings, |_| {
                    updates.set(updates.get() + 1);
                })
                .unwrap_err();
                assert_eq!(
                    error.code,
                    if denied {
                        SttJobErrorCode::CredentialUnavailable
                    } else {
                        SttJobErrorCode::MissingProviderKey
                    }
                );
                assert_eq!(updates.get(), if denied { 0 } else { 1 });
                assert_eq!(
                    storage.read_settings().unwrap().settings.provider_key_saved[provider.as_str()],
                    denied
                );
                assert_eq!(store.calls(), ["construct", "read"]);
                drop(storage);
                cleanup(&path);
            }
        }
    }

    #[test]
    fn provider_warnings_persist_and_repeated_status_does_not_write() {
        let path = test_database("echo-provider-status");
        let storage = initialized(&path);
        let id = ProviderId::GroqWhisperTurbo;
        let frozen =
            freeze_provider_settings(&storage.read_settings().unwrap().settings, id).unwrap();
        for status in ["invalid", "valid"] {
            let update = save_provider_status(&storage, &frozen, status)
                .unwrap()
                .unwrap();
            assert_eq!(
                update.current.settings.provider_key_statuses[id.as_str()],
                status
            );
            assert!(save_provider_status(&storage, &frozen, status)
                .unwrap()
                .is_none());
        }
        drop(storage);
        cleanup(&path);
    }

    // Keep: the English Phrasebook sentence is sent only for English. Auto-detect and every
    // other saved language get the terms alone, so the prompt never pulls non-English audio
    // towards English. The user's own prompt is passed through unchanged in every case.
    #[test]
    fn frozen_prompt_uses_the_english_sentence_only_for_english() {
        const ENGLISH: &str = "This is English dictation for software and product work. Use these exact spellings when heard: Sample, Echo.";
        const NEUTRAL: &str = "Sample, Echo.";
        for (language, phrasebook) in [
            (None, NEUTRAL),
            (Some(""), NEUTRAL),
            (Some("es"), NEUTRAL),
            (Some("pt-BR"), NEUTRAL),
            (Some("fil"), NEUTRAL),
            (Some("en"), ENGLISH),
            (Some("en-US"), ENGLISH),
            (Some("en-GB"), ENGLISH),
            (Some("EN"), ENGLISH),
        ] {
            for provider in [ProviderId::GroqWhisperTurbo, ProviderId::OpenAiTranscribe] {
                let settings = PersistedSettings {
                    language: language.map(str::to_owned),
                    phrasebook_terms: vec!["Sample".into(), "Echo".into()],
                    ..Default::default()
                };
                let frozen = freeze_provider_settings(&settings, provider).unwrap();
                assert_eq!(frozen.prompt.as_deref(), Some(phrasebook), "{language:?}");
                let settings = PersistedSettings {
                    custom_prompt: Some("Custom instruction".into()),
                    ..settings
                };
                let frozen = freeze_provider_settings(&settings, provider).unwrap();
                assert_eq!(
                    frozen.prompt,
                    Some(format!("Custom instruction\n\n{phrasebook}")),
                    "{language:?}"
                );
            }
        }
        // Without Phrasebook terms the custom prompt travels alone, whatever the language.
        let settings = PersistedSettings {
            custom_prompt: Some("Custom instruction".into()),
            language: None,
            ..Default::default()
        };
        let frozen = freeze_provider_settings(&settings, ProviderId::GroqWhisperTurbo).unwrap();
        assert_eq!(frozen.prompt.as_deref(), Some("Custom instruction"));
    }

    #[test]
    fn segments_frozen_prompt_keeps_custom_then_phrasebook_and_defaults_to_none() {
        for provider in [ProviderId::GroqWhisperTurbo, ProviderId::OpenAiTranscribe] {
            for terms in [vec![], vec!["Sample".into(), "Echo".into()]] {
                let settings = PersistedSettings {
                    custom_prompt: Some("Custom instruction".into()),
                    phrasebook_terms: terms.clone(),
                    language: Some("en".into()),
                    ..Default::default()
                };
                let frozen = freeze_provider_settings(&settings, provider).unwrap();
                assert_eq!(frozen.keyterms, terms);
                let phrasebook = if terms.is_empty() {
                    String::new()
                } else {
                    "\n\nThis is English dictation for software and product work. Use these exact spellings when heard: Sample, Echo.".into()
                };
                assert_eq!(
                    frozen.prompt.as_deref(),
                    Some(format!("Custom instruction{phrasebook}").as_str())
                );
            }
            let frozen = freeze_provider_settings(&PersistedSettings::default(), provider).unwrap();
            assert_eq!(frozen.prompt, None);
        }
        let settings = PersistedSettings {
            custom_prompt: Some("Custom instruction".into()),
            phrasebook_terms: vec!["Sample".into(), "Echo".into()],
            ..Default::default()
        };
        assert_eq!(
            freeze_provider_settings(&settings, ProviderId::XaiStt)
                .unwrap()
                .keyterms,
            settings.phrasebook_terms
        );
        assert_eq!(
            freeze_provider_settings(&settings, ProviderId::XaiStt)
                .unwrap()
                .prompt,
            None
        );
    }
}
