use crate::providers::TranscriptionResult;

pub(super) fn stitch<'a>(results: impl Iterator<Item = &'a TranscriptionResult>) -> String {
    let mut text = String::new();
    for result in results {
        let delta = clean_seam(&text, &result.transcript);
        if !delta.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&delta);
        }
    }
    text
}

pub(super) fn strip_edge_ellipses(mut text: &str) -> &str {
    text = text.trim();
    loop {
        let stripped = text.strip_prefix('…').or_else(|| text.strip_prefix("..."));
        match stripped {
            Some(rest) => text = rest.trim_start(),
            None => break,
        }
    }
    loop {
        let stripped = text.strip_suffix('…').or_else(|| text.strip_suffix("..."));
        match stripped {
            Some(rest) => text = rest.trim_end(),
            None => break,
        }
    }
    text
}

/// Only seam edits are allowed: edge ellipses, 3+ boundary words and joining case.
pub(super) fn clean_seam(previous: &str, chunk: &str) -> String {
    clean_seam_details(previous, chunk).0
}

pub(super) fn clean_seam_details(previous: &str, chunk: &str) -> (String, bool) {
    let mut collapsed_overlap = false;
    let mut chunk = strip_edge_ellipses(chunk);
    let previous_words = word_spans(previous);
    let next_words = word_spans(chunk);
    for count in (3..=previous_words.len().min(next_words.len())).rev() {
        let overlap = &next_words[..count];
        let first_word = overlap[0].1.to_lowercase();
        if overlap
            .iter()
            .any(|(_, word)| word.to_lowercase() != first_word)
            && previous_words[previous_words.len() - count..]
                .iter()
                .zip(&next_words[..count])
                .all(|((_, left), (_, right))| left.to_lowercase() == right.to_lowercase())
        {
            let end = if count == next_words.len() {
                chunk.len()
            } else {
                next_words[count].0
            };
            chunk = &chunk[end..];
            collapsed_overlap = true;
            break;
        }
    }
    if !previous.is_empty() && !previous.ends_with(['.', '!', '?']) {
        if let Some(token) = chunk.split_whitespace().next() {
            let word =
                token.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’');
            let lower = word.to_ascii_lowercase();
            if !word.is_empty()
                && word.as_bytes()[0].is_ascii_uppercase()
                && word.as_bytes()[1..].iter().all(u8::is_ascii_lowercase)
                && CLIP_START_FUNCTION_WORDS.contains(&lower.as_str())
            {
                return (format!("{}{}", &lower[..1], &chunk[1..]), collapsed_overlap);
            }
        }
    }
    (chunk.to_string(), collapsed_overlap)
}

const CLIP_START_FUNCTION_WORDS: &[&str] = &[
    "the", "a", "an", "and", "but", "or", "so", "then", "that", "this", "these", "those", "it",
    "its", "we", "you", "they", "he", "she", "to", "of", "in", "on", "at", "for", "with", "from",
    "by", "as", "if", "when", "because", "which", "what", "there", "here", "my", "our", "your",
    "their", "is", "are", "was", "were", "be", "been", "have", "has", "had", "do", "does", "did",
    "not", "just", "also", "very", "really", "now", "like", "about", "into", "out", "up", "over",
    "after", "before", "than", "more", "some", "any", "all", "one", "two",
];

fn word_spans(text: &str) -> Vec<(usize, &str)> {
    text.split_whitespace()
        .filter_map(|word| {
            let comparison = word.trim_matches(|c: char| !c.is_alphanumeric());
            (!comparison.is_empty())
                .then_some((word.as_ptr() as usize - text.as_ptr() as usize, comparison))
        })
        .collect()
}
