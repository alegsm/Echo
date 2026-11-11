mod capture;
mod deadline;
mod dispatch;
mod preview;
mod registry;
mod release;
mod repair_lifecycle;
mod seam;

use super::{maintain_lease, CaptureDictation, CaptureDictations, TestDeadlines};

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio::{AudioArtifactMetadata, AudioSegment, CapturePurpose, NativeAudioArtifact};
use crate::providers::{ProviderId, SegmentConfidence, TranscriptionEdges, TranscriptionResult};
use crate::services::dictation::{
    FrozenJobSettings, JobRequest, PartialTranscriptEvent, SegmentProvider, SegmentedDictation,
    SttJobError, SttJobErrorCode, SttJobStage,
};
use crate::services::transcription_jobs::TranscriptionJobs;

#[derive(Default)]
struct FakeProvider {
    pair_calls: Mutex<Vec<(u8, FrozenJobSettings)>>,
    pair_completed: AtomicUsize,
    pair_missing_evidence: bool,
    pair_outcomes: Vec<(u8, Result<String, SttJobError>)>,
    pair_release: Option<Arc<tauri::async_runtime::Mutex<tauri::async_runtime::Receiver<()>>>>,
    edges: Vec<(u8, Option<TranscriptionEdges>)>,
    calls: Mutex<Vec<(u8, FrozenJobSettings)>>,
    completed: AtomicUsize,
    background_completed: Arc<AtomicUsize>,
    background_accepted: Arc<AtomicUsize>,
    whole_calls: Mutex<Vec<FrozenJobSettings>>,
    whole_outcome: Option<Result<String, SttJobError>>,
    whole_release: Option<Arc<Mutex<std::sync::mpsc::Receiver<()>>>>,
    wav_ordinals: bool,
    fail: Option<u8>,
    fail_once: Option<u8>,
    outcomes: Vec<(u8, Result<String, SttJobError>)>,
    out_of_order: bool,
    delay_ms: Option<u64>,
    release: Option<Arc<Mutex<std::sync::mpsc::Receiver<()>>>>,
    release_ordinals: Option<Vec<u8>>,
    preflight_ms: Option<u64>,
    preflights: AtomicUsize,
    preflights_finished: AtomicUsize,
    active: AtomicUsize,
    peak_active: AtomicUsize,
}

impl SegmentProvider for FakeProvider {
    fn transcribe(
        &self,
        settings: FrozenJobSettings,
        audio: Arc<[u8]>,
        can_dispatch: crate::services::dictation::DispatchGate,
    ) -> Pin<Box<dyn Future<Output = Result<TranscriptionResult, SttJobError>> + Send + '_>> {
        Box::pin(async move {
            self.preflights.fetch_add(1, Ordering::SeqCst);
            if let Some(delay) = self.preflight_ms {
                tauri::async_runtime::spawn_blocking(move || {
                    std::thread::sleep(Duration::from_millis(delay))
                })
                .await
                .unwrap();
            }
            self.preflights_finished.fetch_add(1, Ordering::SeqCst);
            if !can_dispatch() {
                return Err(SttJobError::new(
                    SttJobErrorCode::JobCanceled,
                    SttJobStage::Provider,
                    false,
                    "Fixture canceled.",
                ));
            }
            let ordinal = if self.wav_ordinals {
                audio[44]
            } else {
                audio[0]
            };
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak_active.fetch_max(active, Ordering::SeqCst);
            let first_attempt = {
                let mut calls = self.calls.lock().unwrap();
                let first = !calls.iter().any(|(id, _)| *id == ordinal);
                calls.push((ordinal, settings.clone()));
                first
            };
            let release = self.release.clone().filter(|_| {
                self.release_ordinals
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&ordinal))
            });
            let delay = if let Some(delay) = self.delay_ms {
                delay
            } else if self.out_of_order {
                120 - u64::from(ordinal) * 30
            } else {
                80
            };
            let background_completed = self.background_completed.clone();
            let background_accepted = self.background_accepted.clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Some(release) = release {
                    release.lock().unwrap().recv().unwrap();
                }
                std::thread::sleep(Duration::from_millis(delay));
                if can_dispatch() {
                    background_accepted.fetch_add(1, Ordering::SeqCst);
                }
                background_completed.fetch_add(1, Ordering::SeqCst);
            })
            .await
            .unwrap();
            self.active.fetch_sub(1, Ordering::SeqCst);
            self.completed.fetch_add(1, Ordering::SeqCst);
            if self.fail == Some(ordinal) || (self.fail_once == Some(ordinal) && first_attempt) {
                return Err(SttJobError::new(
                    SttJobErrorCode::ProviderTimeout,
                    SttJobStage::Provider,
                    true,
                    "Fixture timeout.",
                ));
            }
            let transcript = match self.outcomes.iter().find(|(id, _)| *id == ordinal) {
                Some((_, result)) => result.clone()?,
                None => format!("  piece-{ordinal} \n"),
            };
            Ok(TranscriptionResult {
                edges: self
                    .edges
                    .iter()
                    .find(|(id, _)| *id == ordinal)
                    .map_or(Some(confident_edges()), |(_, edges)| *edges),
                transcript,
                provider_id: settings.provider_id,
                provider_model: settings.provider_id.model(),
                provider_ms: u128::from(delay),
            })
        })
    }
    fn transcribe_pair(
        &self,
        settings: FrozenJobSettings,
        audio: Arc<[u8]>,
        can_dispatch: crate::services::dictation::DispatchGate,
    ) -> Pin<Box<dyn Future<Output = Result<TranscriptionResult, SttJobError>> + Send + '_>> {
        Box::pin(async move {
            assert!(can_dispatch());
            let ordinal = audio[44];
            self.pair_calls
                .lock()
                .unwrap()
                .push((ordinal, settings.clone()));
            if let Some(release) = &self.pair_release {
                release.lock().await.recv().await;
            }
            self.pair_completed.fetch_add(1, Ordering::SeqCst);
            let text = self
                .pair_outcomes
                .iter()
                .find(|(id, _)| *id == ordinal)
                .map(|(_, outcome)| outcome.clone())
                .unwrap_or_else(|| Err(fixture_error(SttJobErrorCode::ProviderLowConfidence)))?;
            Ok(TranscriptionResult {
                transcript: text,
                edges: (!self.pair_missing_evidence).then_some(confident_edges()),
                provider_id: settings.provider_id,
                provider_model: settings.provider_id.model(),
                provider_ms: 1,
            })
        })
    }
    fn transcribe_whole(
        &self,
        settings: FrozenJobSettings,
        audio: NativeAudioArtifact,
        can_dispatch: crate::services::dictation::DispatchGate,
    ) -> Pin<Box<dyn Future<Output = Result<TranscriptionResult, SttJobError>> + Send + '_>> {
        Box::pin(async move {
            assert!(can_dispatch());
            self.whole_calls.lock().unwrap().push(settings.clone());
            if let Some(release) = self.whole_release.clone() {
                tauri::async_runtime::spawn_blocking(move || {
                    release.lock().unwrap().recv().unwrap();
                })
                .await
                .unwrap();
            }
            if let Some(outcome) = &self.whole_outcome {
                return Ok(TranscriptionResult {
                    edges: Some(confident_edges()),
                    transcript: outcome.clone()?,
                    provider_id: settings.provider_id,
                    provider_model: settings.provider_id.model(),
                    provider_ms: 1,
                });
            }
            self.transcribe(settings, audio.whole_wav(), can_dispatch)
                .await
                .map(|mut result| {
                    result.transcript = result.transcript.trim().to_string();
                    result
                })
        })
    }
}

fn settings() -> FrozenJobSettings {
    FrozenJobSettings {
        provider_id: ProviderId::GroqWhisperTurbo,
        language: Some("en".into()),
        prompt: Some("original".into()),
        keyterms: Vec::new(),
        paste_after_transcription: true,
        restore_clipboard: true,
        diagnostics: false,
    }
}

fn request() -> JobRequest {
    JobRequest {
        owner: "overlay".into(),
        artifact_id: "artifact".into(),
        session_id: "session".into(),
        intent_id: "intent".into(),
        artifact_intent_id: "intent".into(),
        created_at: "2026-11-01T00:00:00Z".into(),
    }
}

fn audio(count: u8, complete: bool, silent: Option<u8>) -> NativeAudioArtifact {
    NativeAudioArtifact {
        stopped_at: Some(Instant::now()),
        is_complete: complete,
        metadata: AudioArtifactMetadata {
            artifact_id: "artifact".into(),
            session_id: "session".into(),
            intent_id: "intent".into(),
            purpose: CapturePurpose::PushToTalk,
            mime_type: "audio/wav",
            byte_size: count as usize,
            duration_ms: u64::from(count) * 1000,
            sample_rate: 16_000,
            channel_count: 1,
        },
        segments: (0..count)
            .map(|ordinal| AudioSegment {
                ordinal: u32::from(ordinal),
                cut_kind: crate::audio::SegmentCutKind::QuietWindow,
                duration_ms: 1000,
                bytes: (silent != Some(ordinal)).then(|| Arc::from([ordinal])),
            })
            .collect::<Vec<_>>()
            .into(),
    }
}

fn dictation(
    jobs: &TranscriptionJobs,
    provider: Arc<FakeProvider>,
    current: Arc<Mutex<FrozenJobSettings>>,
) -> Arc<SegmentedDictation> {
    let registry = jobs.clone();
    SegmentedDictation::new(
        request(),
        jobs.clone(),
        provider,
        Arc::new(move |request| {
            registry.admit_initial(request.clone(), current.lock().unwrap().clone())
        }),
    )
}

fn dictation_with_publisher(
    jobs: &TranscriptionJobs,
    provider: Arc<FakeProvider>,
    published: Arc<Mutex<Vec<PartialTranscriptEvent>>>,
) -> Arc<SegmentedDictation> {
    let registry = jobs.clone();
    SegmentedDictation::new_with_publisher(
        request(),
        jobs.clone(),
        provider,
        Arc::new(move |request| registry.admit_initial(request.clone(), settings())),
        Arc::new(move |event| published.lock().unwrap().push(event)),
    )
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "condition timed out");
        tauri::async_runtime::spawn_blocking(|| std::thread::sleep(Duration::from_millis(2)))
            .await
            .unwrap();
    }
}

fn release_audio(duration_ms: u64, pieces: u8) -> NativeAudioArtifact {
    let mut bundle = audio(pieces, true, None);
    bundle.metadata.duration_ms = duration_ms;
    bundle.segments = (0..pieces)
        .map(|ordinal| AudioSegment {
            ordinal: ordinal.into(),
            cut_kind: crate::audio::SegmentCutKind::QuietWindow,
            duration_ms: duration_ms / u64::from(pieces),
            bytes: Some(Arc::from({
                let mut bytes = crate::audio::encode_wav(&[0.2; 16], 16_000, 1);
                bytes[44] = ordinal;
                bytes
            })),
        })
        .collect::<Vec<_>>()
        .into();
    bundle.metadata.byte_size = bundle
        .segments
        .iter()
        .filter_map(|p| p.bytes.as_ref())
        .map(|p| p.len())
        .sum();
    bundle
}

fn confident_edges() -> TranscriptionEdges {
    let edge = Some(SegmentConfidence {
        avg_logprob: -0.2,
        no_speech_prob: 0.1,
    });
    TranscriptionEdges {
        first: edge,
        last: edge,
    }
}

fn fixture_error(code: SttJobErrorCode) -> SttJobError {
    SttJobError::new(code, SttJobStage::Provider, true, "Fixture failure.")
}
