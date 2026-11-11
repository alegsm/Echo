use super::resample::{normalize_mono, CHANNEL_COUNT, SAMPLE_RATE};

/// Encode device PCM as 16 kHz mono signed PCM16 for provider upload.
pub(crate) fn encode_wav(samples: &[f32], sample_rate: u32, channel_count: u16) -> Vec<u8> {
    let samples = normalize_mono(samples, sample_rate, channel_count);
    let data_bytes = samples.len().saturating_mul(2);
    let data_length = u32::try_from(data_bytes).unwrap_or(u32::MAX);
    let block_align = CHANNEL_COUNT * 2;
    let byte_rate = SAMPLE_RATE * u32::from(block_align);
    let mut bytes = Vec::with_capacity(44usize.saturating_add(data_bytes));
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&36u32.saturating_add(data_length).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&CHANNEL_COUNT.to_le_bytes());
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&byte_rate.to_le_bytes());
    bytes.extend_from_slice(&block_align.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_length.to_le_bytes());
    for sample in samples {
        let value = (sample * f32::from(i16::MAX)) as i16;
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

const HEADER_BYTES: usize = 44;

/// The PCM bytes of a WAV in exactly the format `encode_wav` writes: 16 kHz, mono,
/// signed 16-bit, with a data length that matches the buffer. Anything else is `None`,
/// so callers never guess at a layout they did not produce.
pub(super) fn pcm16_mono_data(wav: &[u8]) -> Option<&[u8]> {
    let (header, data) = wav.split_at_checked(HEADER_BYTES)?;
    let word = |offset: usize| u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
    let half = |offset: usize| u16::from_le_bytes(header[offset..offset + 2].try_into().unwrap());
    let expected_header = &header[0..4] == b"RIFF"
        && &header[8..16] == b"WAVEfmt "
        && word(16) == 16
        && half(20) == 1
        && half(22) == CHANNEL_COUNT
        && word(24) == SAMPLE_RATE
        && half(34) == 16
        && &header[36..40] == b"data"
        && usize::try_from(word(40)) == Ok(data.len());
    (expected_header && data.len() % 2 == 0).then_some(data)
}
