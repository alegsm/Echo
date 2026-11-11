use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn initialized(path: &std::path::Path) -> EchoStorage {
    let storage = EchoStorage::open(path).unwrap();
    storage.initialize().unwrap();
    storage
}

pub fn transcript(id: &str, created_at: &str) -> TranscriptRecord {
    TranscriptRecord {
        id: id.into(),
        text: "hello".into(),
        created_at: created_at.into(),
        duration_ms: 1_000,
        provider_id: "groq-whisper-turbo".into(),
        provider_model: Some("whisper-large-v3-turbo".into()),
        language: Some("en".into()),
        prompt: Some("approved prompt".into()),
        timings: TranscriptTiming {
            recording_ms: 1_000.0,
            blob_ready_ms: 10.0,
            upload_ms: 20.0,
            stop_to_transcript_ms: 500.0,
            history_write_ms: Some(2.0),
            provider_ms: Some(450.0),
            backend_ms: Some(470.0),
        },
        audio: TranscriptAudioMetadata {
            mime_type: "audio/wav".into(),
            byte_size: 4_096,
        },
        insertion_mode: "pasted".into(),
        source: "desktop".into(),
        error_code: None,
    }
}

pub fn test_database(label: &str) -> std::path::PathBuf {
    let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir()
        .join(format!(
            "echo-storage-{label}-{}-{sequence}",
            std::process::id()
        ))
        .join("echo.sqlite3")
}

pub fn cleanup(path: &std::path::Path) {
    if let Some(directory) = path.parent() {
        let _ = std::fs::remove_dir_all(directory);
    }
}
