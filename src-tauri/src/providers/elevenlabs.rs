//! ElevenLabs Scribe v2 transcription.

use super::{
    multipart_file, primary_language, sensitive_header, vocabulary, ProviderError, ProviderId,
    TranscriptionRequest, VocabularyRules,
};

pub(super) const ORIGIN: &str = "https://api.elevenlabs.io";
const PATH: &str = "/v1/speech-to-text";
pub(super) const VOCABULARY_RULES: VocabularyRules = VocabularyRules {
    max_terms: 100,
    max_chars: 50,
    max_words: 5,
    forbidden: &['<', '>', '{', '}', '[', ']', '\\'],
};

pub(super) fn build_request(
    client: &reqwest::Client,
    request: &TranscriptionRequest,
    api_key: &str,
    origin: &str,
) -> Result<reqwest::RequestBuilder, ProviderError> {
    let mut form =
        reqwest::multipart::Form::new().text("model_id", ProviderId::ElevenLabsScribeV2.model());
    if let Some(language) = primary_language(request) {
        form = form.text("language_code", language.to_owned());
    }
    for term in vocabulary(&request.keyterms, &VOCABULARY_RULES) {
        form = form.text("keyterms", term.to_owned());
    }
    form = form.part("file", multipart_file(request)?);
    Ok(client
        .post(format!("{origin}{PATH}"))
        .header("xi-api-key", sensitive_header(api_key)?)
        .multipart(form))
}
