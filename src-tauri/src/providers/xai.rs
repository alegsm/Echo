//! xAI speech-to-text transcription.

use super::{
    multipart_file, primary_language, vocabulary, ProviderError, ProviderId, TranscriptionRequest,
    VocabularyRules,
};

pub(super) const ORIGIN: &str = "https://api.x.ai";
const PATH: &str = "/v1/stt";
pub(super) const VOCABULARY_RULES: VocabularyRules = VocabularyRules {
    max_terms: 100,
    max_chars: 50,
    max_words: usize::MAX,
    forbidden: &[],
};

pub(super) fn build_request(
    client: &reqwest::Client,
    request: &TranscriptionRequest,
    api_key: &str,
    origin: &str,
) -> Result<reqwest::RequestBuilder, ProviderError> {
    let mut form = reqwest::multipart::Form::new().text("model", ProviderId::XaiStt.model());
    // xAI detects the language when none is sent, and rejects `format` without one.
    if let Some(language) = primary_language(request) {
        form = form
            .text("format", "true")
            .text("language", language.to_owned());
    }
    for term in vocabulary(&request.keyterms, &VOCABULARY_RULES) {
        form = form.text("keyterm", term.to_owned());
    }
    // xAI may ignore metadata sent after the audio part.
    form = form.part("file", multipart_file(request)?);
    Ok(client
        .post(format!("{origin}{PATH}"))
        .bearer_auth(api_key)
        .multipart(form))
}
