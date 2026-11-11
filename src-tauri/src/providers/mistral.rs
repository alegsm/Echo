//! Mistral Voxtral Mini Transcribe 3 transcription.

use super::{
    multipart_file, primary_language, vocabulary, ProviderError, ProviderId, TranscriptionRequest,
    VocabularyRules,
};

pub(super) const ORIGIN: &str = "https://api.mistral.ai";
const PATH: &str = "/v1/audio/transcriptions";
pub(super) const VOCABULARY_RULES: VocabularyRules = VocabularyRules {
    max_terms: 100,
    max_chars: usize::MAX,
    max_words: usize::MAX,
    forbidden: &[],
};

pub(super) fn build_request(
    client: &reqwest::Client,
    request: &TranscriptionRequest,
    api_key: &str,
    origin: &str,
) -> Result<reqwest::RequestBuilder, ProviderError> {
    let mut form =
        reqwest::multipart::Form::new().text("model", ProviderId::MistralVoxtral3.model());
    if let Some(language) = primary_language(request) {
        form = form.text("language", language.to_owned());
    }
    for term in vocabulary(&request.keyterms, &VOCABULARY_RULES) {
        form = form.text("context_bias", term.to_owned());
    }
    form = form.part("file", multipart_file(request)?);
    Ok(client
        .post(format!("{origin}{PATH}"))
        .bearer_auth(api_key)
        .multipart(form))
}
