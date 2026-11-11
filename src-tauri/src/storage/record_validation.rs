use super::models::{StorageError, StorageResult};

pub(super) fn validate_language(value: Option<String>) -> StorageResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 35 {
        return Err(StorageError::Validation("language"));
    }
    let value = value.to_string();
    let mut parts = value.split('-');
    let first = parts.next().unwrap_or_default();
    let valid = (2..=8).contains(&first.len())
        && first.bytes().all(|byte| byte.is_ascii_alphabetic())
        && parts.all(|part| {
            !part.is_empty()
                && part.len() <= 8
                && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        });
    valid
        .then_some(Some(value))
        .ok_or(StorageError::Validation("language"))
}

pub(super) fn validate_rfc3339(value: &str) -> StorageResult<()> {
    if !value.is_ascii() {
        return Err(StorageError::Validation("transcript.createdAt"));
    }
    let zone_start = validate_timestamp_shape(value)?;
    validate_fraction(value, zone_start)?;
    validate_calendar(value)?;
    validate_timezone(value, zone_start)
}

fn validate_timestamp_shape(value: &str) -> StorageResult<usize> {
    let bytes = value.as_bytes();
    let shape = bytes.len() >= 20
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes.get(10) == Some(&b'T')
        && bytes.get(13) == Some(&b':')
        && bytes.get(16) == Some(&b':');
    let zone = value.ends_with('Z')
        || bytes.len() >= 25
            && matches!(bytes.get(bytes.len() - 6), Some(b'+' | b'-'))
            && bytes.get(bytes.len() - 3) == Some(&b':');
    if !shape || !zone {
        return Err(StorageError::Validation("transcript.createdAt"));
    }
    Ok(if value.ends_with('Z') {
        bytes.len() - 1
    } else {
        bytes.len() - 6
    })
}

fn validate_fraction(value: &str, zone_start: usize) -> StorageResult<()> {
    let fraction = &value[19..zone_start];
    if !fraction.is_empty()
        && (!fraction.starts_with('.')
            || fraction.len() == 1
            || !fraction[1..].bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(StorageError::Validation("transcript.createdAt"));
    }
    Ok(())
}

fn validate_calendar(value: &str) -> StorageResult<()> {
    let (Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        parse_component(value, 5..7),
        parse_component(value, 8..10),
        parse_component(value, 11..13),
        parse_component(value, 14..16),
        parse_component(value, 17..19),
    ) else {
        return Err(StorageError::Validation("transcript.createdAt"));
    };
    let year =
        parse_component(value, 0..4).ok_or(StorageError::Validation("transcript.createdAt"))?;
    if year == 0
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(StorageError::Validation("transcript.createdAt"));
    }
    Ok(())
}

fn validate_timezone(value: &str, zone_start: usize) -> StorageResult<()> {
    if value.ends_with('Z') {
        return Ok(());
    }
    let zone_hour = parse_component(value, zone_start + 1..zone_start + 3).unwrap_or(99);
    let zone_minute = parse_component(value, zone_start + 4..value.len()).unwrap_or(99);
    if zone_hour <= 23 && zone_minute <= 59 {
        Ok(())
    } else {
        Err(StorageError::Validation("transcript.createdAt"))
    }
}

fn parse_component(value: &str, range: std::ops::Range<usize>) -> Option<u32> {
    value.get(range)?.parse().ok()
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(400) || year.is_multiple_of(4) && !year.is_multiple_of(100) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    }
}

pub(super) fn valid_audio_mime(value: &str) -> bool {
    let mut parts = value.split(';');
    let base = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
    let parameters: Vec<_> = parts
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    match base.as_str() {
        "audio/webm" | "audio/ogg" => {
            parameters.is_empty()
                || parameters.len() == 1 && parameters[0].eq_ignore_ascii_case("codecs=opus")
        }
        "audio/mp4" => {
            parameters.is_empty()
                || parameters.len() == 1 && parameters[0].eq_ignore_ascii_case("codecs=mp4a.40.2")
        }
        "audio/wav" => parameters.is_empty(),
        _ => false,
    }
}

pub(super) fn valid_error_code(code: &str) -> bool {
    matches!(
        code,
        "shortcut_conflict"
            | "microphone_permission_denied"
            | "microphone_unavailable"
            | "capture_failed"
            | "empty_audio"
            | "missing_provider_key"
            | "provider_timeout"
            | "provider_failed"
            | "clipboard_failed"
            | "insertion_failed"
            | "history_write_failed"
            | "not_implemented"
    )
}
