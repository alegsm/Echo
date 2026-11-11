use crate::providers::ProviderId;

use super::models::{
    PersistedSettings, SettingsField, SettingsPatch, StorageError, StorageResult, TranscriptRecord,
    TranscriptTiming, MAX_AUDIO_BYTES,
};

impl PersistedSettings {
    pub fn apply_patch(&self, patch: SettingsPatch) -> (Self, Vec<SettingsField>) {
        let mut next = self.clone();
        let mut changed = Vec::new();
        apply_capture_settings(&mut next, &patch, &mut changed);
        apply_provider_settings(&mut next, &patch, &mut changed);
        apply_output_settings(&mut next, patch, &mut changed);
        (next, changed)
    }

    pub fn validate(&mut self) -> StorageResult<()> {
        self.push_to_talk_shortcut =
            bounded_trimmed(&self.push_to_talk_shortcut, 128, "pushToTalkShortcut")?;
        self.fallback_shortcut =
            optional_bounded(self.fallback_shortcut.take(), 128, "fallbackShortcut")?;
        validate_provider_id(&self.provider_id)?;
        for provider in ProviderId::ALL {
            self.provider_key_saved
                .entry(provider.as_str().into())
                .or_insert(false);
        }
        validate_provider_maps(self)?;
        normalize_phrasebook(&mut self.phrasebook_terms)?;
        self.language = super::record_validation::validate_language(self.language.take())?;
        self.custom_prompt = optional_bounded(self.custom_prompt.take(), 8_000, "customPrompt")?;
        if self
            .history_retention_days
            .is_some_and(|days| !(1..=3650).contains(&days))
        {
            return Err(StorageError::Validation("historyRetentionDays"));
        }
        Ok(())
    }
}

impl TranscriptRecord {
    pub fn validate(&mut self) -> StorageResult<()> {
        self.id = bounded_trimmed(&self.id, 128, "transcript.id")?;
        if self.text.len() > 1_000_000 {
            return Err(StorageError::Validation("transcript.text"));
        }
        if self.duration_ms > i64::MAX as u64 {
            return Err(StorageError::Validation("transcript.durationMs"));
        }
        super::record_validation::validate_rfc3339(&self.created_at)?;
        validate_provider_id(&self.provider_id)?;
        self.provider_model =
            optional_bounded(self.provider_model.take(), 128, "transcript.providerModel")?;
        self.language = super::record_validation::validate_language(self.language.take())?;
        self.prompt = optional_bounded(self.prompt.take(), 8_000, "transcript.prompt")?;
        self.timings.validate()?;
        if self.audio.byte_size > MAX_AUDIO_BYTES {
            return Err(StorageError::Validation("transcript.audio.byteSize"));
        }
        if !super::record_validation::valid_audio_mime(&self.audio.mime_type) {
            return Err(StorageError::Validation("transcript.audio.mimeType"));
        }
        if !matches!(
            self.insertion_mode.as_str(),
            "pasted" | "copied" | "manual" | "failed"
        ) {
            return Err(StorageError::Validation("transcript.insertionMode"));
        }
        if !matches!(self.source.as_str(), "web" | "desktop") {
            return Err(StorageError::Validation("transcript.source"));
        }
        if self
            .error_code
            .as_deref()
            .is_some_and(|code| !super::record_validation::valid_error_code(code))
        {
            return Err(StorageError::Validation("transcript.errorCode"));
        }
        Ok(())
    }
}

impl TranscriptTiming {
    fn validate(&self) -> StorageResult<()> {
        let values = [
            Some(self.recording_ms),
            Some(self.blob_ready_ms),
            Some(self.upload_ms),
            Some(self.stop_to_transcript_ms),
            self.history_write_ms,
            self.provider_ms,
            self.backend_ms,
        ];
        if values
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite() || value < 0.0)
        {
            return Err(StorageError::Validation("transcript.timings"));
        }
        Ok(())
    }
}

fn apply_capture_settings(
    next: &mut PersistedSettings,
    patch: &SettingsPatch,
    changed: &mut Vec<SettingsField>,
) {
    apply(
        &mut next.push_to_talk_shortcut,
        patch.push_to_talk_shortcut.clone(),
        SettingsField::PushToTalkShortcut,
        changed,
    );
    apply(
        &mut next.fallback_shortcut,
        patch.fallback_shortcut.clone().into_change(),
        SettingsField::FallbackShortcut,
        changed,
    );
    apply(
        &mut next.language,
        patch.language.clone().into_change(),
        SettingsField::Language,
        changed,
    );
    apply(
        &mut next.custom_prompt,
        patch.custom_prompt.clone().into_change(),
        SettingsField::CustomPrompt,
        changed,
    );
}

fn apply_provider_settings(
    next: &mut PersistedSettings,
    patch: &SettingsPatch,
    changed: &mut Vec<SettingsField>,
) {
    apply(
        &mut next.provider_id,
        patch.provider_id.clone(),
        SettingsField::ProviderId,
        changed,
    );
    merge_map(
        &mut next.provider_key_saved,
        patch.provider_key_saved.clone(),
        SettingsField::ProviderKeySaved,
        changed,
    );
    merge_map(
        &mut next.provider_key_statuses,
        patch.provider_key_statuses.clone(),
        SettingsField::ProviderKeyStatuses,
        changed,
    );
}

fn merge_map<T: PartialEq>(
    target: &mut std::collections::BTreeMap<String, T>,
    patch: Option<std::collections::BTreeMap<String, T>>,
    field: SettingsField,
    changed: &mut Vec<SettingsField>,
) {
    let Some(patch) = patch else {
        return;
    };
    let mut changed_value = false;
    for (key, value) in patch {
        if target.get(&key) != Some(&value) {
            target.insert(key, value);
            changed_value = true;
        }
    }
    if changed_value {
        changed.push(field);
    }
}

fn apply_output_settings(
    next: &mut PersistedSettings,
    patch: SettingsPatch,
    changed: &mut Vec<SettingsField>,
) {
    apply(
        &mut next.phrasebook_terms,
        patch.phrasebook_terms,
        SettingsField::PhrasebookTerms,
        changed,
    );
    apply(
        &mut next.paste_after_transcription,
        patch.paste_after_transcription,
        SettingsField::PasteAfterTranscription,
        changed,
    );
    apply(
        &mut next.restore_clipboard,
        patch.restore_clipboard,
        SettingsField::RestoreClipboard,
        changed,
    );
    apply(
        &mut next.debug_logging_enabled,
        patch.debug_logging_enabled,
        SettingsField::DebugLoggingEnabled,
        changed,
    );
    apply(
        &mut next.history_retention_days,
        patch.history_retention_days.into_change(),
        SettingsField::HistoryRetentionDays,
        changed,
    );
}

fn apply<T: PartialEq>(
    target: &mut T,
    value: Option<T>,
    field: SettingsField,
    changed: &mut Vec<SettingsField>,
) {
    if let Some(value) = value {
        if *target != value {
            *target = value;
            changed.push(field);
        }
    }
}

fn validate_provider_id(value: &str) -> StorageResult<()> {
    ProviderId::try_from(value)
        .map(|_| ())
        .map_err(|_| StorageError::Validation("providerId"))
}

fn validate_provider_maps(settings: &PersistedSettings) -> StorageResult<()> {
    if settings
        .provider_key_saved
        .keys()
        .any(|id| validate_provider_id(id).is_err())
    {
        return Err(StorageError::Validation("providerKeySaved"));
    }
    if settings.provider_key_statuses.iter().any(|(id, status)| {
        validate_provider_id(id).is_err()
            || !matches!(status.as_str(), "unchecked" | "valid" | "invalid")
    }) {
        return Err(StorageError::Validation("providerKeyStatuses"));
    }
    Ok(())
}

fn normalize_phrasebook(terms: &mut Vec<String>) -> StorageResult<()> {
    let mut normalized = Vec::new();
    for term in terms.drain(..) {
        let term = bounded_trimmed(&term, 128, "phrasebookTerms")?;
        if !normalized.contains(&term) {
            normalized.push(term);
        }
    }
    if normalized.len() > 100 {
        return Err(StorageError::Validation("phrasebookTerms"));
    }
    *terms = normalized;
    Ok(())
}

fn bounded_trimmed(value: &str, max: usize, field: &'static str) -> StorageResult<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > max {
        return Err(StorageError::Validation(field));
    }
    Ok(value.to_string())
}

fn optional_bounded(
    value: Option<String>,
    max: usize,
    field: &'static str,
) -> StorageResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Ok(None);
    }
    bounded_trimmed(&value, max, field).map(Some)
}
