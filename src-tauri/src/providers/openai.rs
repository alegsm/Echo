//! OpenAI gpt-transcribe transcription.

use super::{
    multipart_file, primary_language, vocabulary, ProviderError, ProviderId, TranscriptionRequest,
    VocabularyRules,
};

pub(super) const ORIGIN: &str = "https://api.openai.com";
const PATH: &str = "/v1/audio/transcriptions";
pub(super) const VOCABULARY_RULES: VocabularyRules = VocabularyRules {
    max_terms: usize::MAX,
    max_chars: usize::MAX,
    max_words: usize::MAX,
    forbidden: &['<', '>', '\r', '\n'],
};

pub(super) fn build_request(
    client: &reqwest::Client,
    request: &TranscriptionRequest,
    api_key: &str,
    origin: &str,
) -> Result<reqwest::RequestBuilder, ProviderError> {
    let mut form = reqwest::multipart::Form::new()
        .part("file", multipart_file(request)?)
        .text("model", ProviderId::OpenAiTranscribe.model());
    if let Some(language) = primary_language(request) {
        form = form.text("languages[]", language.to_owned());
    }
    for term in vocabulary(&request.keyterms, &VOCABULARY_RULES) {
        form = form.text("keywords[]", term.to_owned());
    }
    if let Some(prompt) = request
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        form = form.text("prompt", prompt.to_owned());
    }
    Ok(client
        .post(format!("{origin}{PATH}"))
        .bearer_auth(api_key)
        .multipart(form))
}
