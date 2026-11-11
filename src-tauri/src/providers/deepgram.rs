//! Deepgram Nova-3 transcription.

use super::{
    non_empty, sensitive_header, vocabulary, ProviderError, ProviderId, TranscriptionRequest,
    VocabularyRules,
};
use std::sync::Arc;

pub(super) const ORIGIN: &str = "https://api.deepgram.com";
const PATH: &str = "/v1/listen";
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
    let mut query = vec![
        ("model", ProviderId::DeepgramNova3.model()),
        ("smart_format", "true"),
        // Deepgram assumes English when `language` is omitted; `multi` makes it detect.
        (
            "language",
            non_empty(request.language.as_deref()).unwrap_or("multi"),
        ),
    ];
    query.extend(vocabulary(&request.keyterms, &VOCABULARY_RULES).map(|term| ("keyterm", term)));
    Ok(client
        .post(format!("{origin}{PATH}"))
        .query(&query)
        .header(
            reqwest::header::AUTHORIZATION,
            sensitive_header(&format!("Token {api_key}"))?,
        )
        .header(reqwest::header::CONTENT_TYPE, &request.mime_type)
        .body(bytes::Bytes::from_owner(Arc::clone(&request.audio))))
}
