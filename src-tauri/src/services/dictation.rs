mod stitching;

use stitching::{clean_seam, clean_seam_details, stitch, strip_edge_ellipses};

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;

use super::transcription_jobs::{job_canceled, TranscriptionJobs};
use crate::audio::{NativeAudioArtifact, SegmentCutKind};
use crate::providers::{ProviderId, TranscriptionResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SttJobStage {
    Settings,
    Credential,
    Artifact,
    Provider,
    History,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SttJobErrorCode {
    InvalidJob,
    JobCanceled,
    TooManyJobs,
    UnsupportedProvider,
    MissingProviderKey,
    CredentialUnavailable,
    ProviderCredentialRejected,
    ArtifactNotFound,
    ArtifactExpired,
    ArtifactBusy,
    InvalidAudio,
    ProviderTimeout,
    ProviderFailed,
    ProviderInvalidResponse,
    ProviderEmptyTranscript,
    ProviderLowConfidence,
    ProviderSilence,
    HistoryWriteFailed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SttJobError {
    pub code: SttJobErrorCode,
    pub stage: SttJobStage,
    pub retryable: bool,
    pub message: &'static str,
}

impl SttJobError {
    pub const fn new(
        code: SttJobErrorCode,
        stage: SttJobStage,
        retryable: bool,
        message: &'static str,
    ) -> Self {
        Self {
            code,
            stage,
            retryable,
            message,
        }
    }

    pub const fn invalid() -> Self {
        Self::new(
            SttJobErrorCode::InvalidJob,
            SttJobStage::Settings,
            false,
            "Transcription job identifiers are invalid.",
        )
    }
}

impl fmt::Display for SttJobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for SttJobError {}

#[derive(Clone, Debug)]
pub struct FrozenJobSettings {
    pub provider_id: ProviderId,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub keyterms: Vec<String>,
    pub paste_after_transcription: bool,
    pub restore_clipboard: bool,
    /// Frozen from the Debug log setting; lets the provider layer write level records.
    pub diagnostics: bool,
}

#[derive(Clone, Debug)]
pub struct JobRequest {
    pub owner: String,
    pub artifact_id: String,
    pub session_id: String,
    pub intent_id: String,
    pub artifact_intent_id: String,
    pub created_at: String,
}

#[derive(Clone, Debug)]
pub struct ActiveJob {
    pub job_sequence: u64,
    pub request: JobRequest,
    pub settings: FrozenJobSettings,
}

const TRANSCRIPTION_GAP: &str = "[Transcription gap]";
const MAX_SEGMENTS: usize = crate::audio::MAXIMUM_SEGMENTS;
const MAX_SEAM_REPAIRS: usize = 3;
const REPAIR_DURATION_LIMIT_MS: u64 = 60_000;

type ProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<TranscriptionResult, SttJobError>> + Send + 'a>>;
pub(crate) type DispatchGate = Arc<dyn Fn() -> bool + Send + Sync>;
pub(crate) type SegmentPublisher = Arc<dyn Fn(PartialTranscriptEvent) + Send + Sync>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartialTranscriptEvent {
    pub dictation_id: String,
    pub ordinal: u32,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<PartialTranscriptStatus>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PartialTranscriptStatus {
    Retrying,
    Gap,
    Recovered,
}

#[derive(Clone, Debug)]
pub(crate) struct DictationCompletion {
    pub provider: TranscriptionResult,
    pub partial: bool,
    pub credential_rejected: bool,
    pub has_speech: bool,
    pub stop_to_transcript_ms: u128,
}

pub(crate) trait SegmentProvider: Send + Sync {
    fn transcribe(
        &self,
        settings: FrozenJobSettings,
        audio: Arc<[u8]>,
        can_dispatch: DispatchGate,
    ) -> ProviderFuture<'_>;

    fn transcribe_pair(
        &self,
        settings: FrozenJobSettings,
        audio: Arc<[u8]>,
        can_dispatch: DispatchGate,
    ) -> ProviderFuture<'_>;

    fn transcribe_whole(
        &self,
        settings: FrozenJobSettings,
        audio: NativeAudioArtifact,
        can_dispatch: DispatchGate,
    ) -> ProviderFuture<'_>;
}

pub(crate) type StartJob = Arc<dyn Fn(&JobRequest) -> Result<ActiveJob, SttJobError> + Send + Sync>;

struct PairRepair {
    pending: bool,
    result: Option<TranscriptionResult>,
}

struct SegmentState {
    audio: Option<NativeAudioArtifact>,
    duration_ms: u64,
    repairs: BTreeMap<u32, PairRepair>,
    repair_tasks: usize,
    repairs_closed: bool,
    seen: usize,
    pending: usize,
    sealed: bool,
    finishing: bool,
    canceled: bool,
    job: Option<ActiveJob>,
    error: Option<SttJobError>,
    failures: BTreeSet<u32>,
    provider_ms: u128,
    credential_rejected: bool,
    results: BTreeMap<u32, TranscriptionResult>,
    next_published_ordinal: u32,
    silent_ordinals: BTreeSet<u32>,
    dispatchers: Vec<std::task::Waker>,
}

/// One capture's provider work. Only release may consume its final result.
pub(crate) struct SegmentedDictation {
    request: JobRequest,
    jobs: TranscriptionJobs,
    provider: Arc<dyn SegmentProvider>,
    start_job: StartJob,
    publish_segment: SegmentPublisher,
    state: Mutex<SegmentState>,
    changed: tauri::async_runtime::Sender<()>,
    changes: tauri::async_runtime::Mutex<tauri::async_runtime::Receiver<()>>,
}

impl SegmentedDictation {
    #[cfg(test)]
    pub(crate) fn new(
        request: JobRequest,
        jobs: TranscriptionJobs,
        provider: Arc<dyn SegmentProvider>,
        start_job: StartJob,
    ) -> Arc<Self> {
        Self::new_with_publisher(request, jobs, provider, start_job, Arc::new(|_| {}))
    }

    pub(crate) fn new_with_publisher(
        request: JobRequest,
        jobs: TranscriptionJobs,
        provider: Arc<dyn SegmentProvider>,
        start_job: StartJob,
        publish_segment: SegmentPublisher,
    ) -> Arc<Self> {
        let (changed, changes) = tauri::async_runtime::channel(1);
        Arc::new(Self {
            request,
            jobs,
            provider,
            start_job,
            publish_segment,
            state: Mutex::new(SegmentState {
                audio: None,
                duration_ms: 0,
                repairs: BTreeMap::new(),
                repair_tasks: 0,
                repairs_closed: false,
                seen: 0,
                pending: 0,
                sealed: false,
                finishing: false,
                canceled: false,
                job: None,
                error: None,
                failures: BTreeSet::new(),
                provider_ms: 0,
                credential_rejected: false,
                results: BTreeMap::new(),
                next_published_ordinal: 0,
                silent_ordinals: BTreeSet::new(),
                dispatchers: Vec::new(),
            }),
            changed,
            changes: tauri::async_runtime::Mutex::new(changes),
        })
    }

    pub(crate) fn submit(self: &Arc<Self>, audio: &NativeAudioArtifact) -> Result<(), SttJobError> {
        self.validate_audio(audio)?;
        let mut state = self.state.lock().map_err(|_| SttJobError::invalid())?;
        if state.canceled {
            return Err(job_canceled());
        }
        if state.sealed {
            return Ok(());
        }
        state.duration_ms = state.duration_ms.max(audio.metadata.duration_ms);
        if state.duration_ms >= REPAIR_DURATION_LIMIT_MS {
            Self::retire_repairs(&mut state, None);
        }
        if state.error.is_some() {
            state.sealed = audio.is_complete;
            return Ok(());
        }
        if audio.is_complete && state.job.is_none() {
            match (self.start_job)(&self.request) {
                Ok(job) => state.job = Some(job),
                Err(error) => {
                    state.error = Some(error);
                    Self::wake_dispatchers(&mut state);
                    state.sealed = true;
                    return Ok(());
                }
            }
        }
        // A delayed capture snapshot may precede one that already included its pieces.
        if audio.segments.len() < state.seen {
            return Ok(());
        }
        for segment in &audio.segments[state.seen..] {
            if let Some(bytes) = &segment.bytes {
                state.pending += 1;
                let run = Arc::clone(self);
                let bytes = Arc::clone(bytes);
                let ordinal = segment.ordinal;
                tauri::async_runtime::spawn(async move {
                    run.transcribe(ordinal, bytes).await;
                });
            } else {
                state.silent_ordinals.insert(segment.ordinal);
            }
        }
        state.audio = Some(audio.clone());
        state.seen = audio.segments.len();
        state.sealed = audio.is_complete;
        self.schedule_repairs(&mut state);
        let _ = self.changed.try_send(());
        Ok(())
    }

    fn validate_audio(&self, audio: &NativeAudioArtifact) -> Result<(), SttJobError> {
        if audio.metadata.artifact_id != self.request.artifact_id
            || audio.metadata.session_id != self.request.session_id
            || audio.metadata.intent_id != self.request.artifact_intent_id
            || audio.segments.len() > MAX_SEGMENTS
            || audio
                .segments
                .iter()
                .enumerate()
                .any(|(index, piece)| piece.ordinal as usize != index)
        {
            return Err(SttJobError::invalid());
        }
        Ok(())
    }

    async fn transcribe(self: &Arc<Self>, ordinal: u32, bytes: Arc<[u8]>) {
        let Some(_permit) = self.wait_for_capacity().await else {
            let mut state = self.state.lock().expect("dictation state lock");
            state.pending -= 1;
            self.cleanup_canceled(&state);
            let _ = self.changed.try_send(());
            return;
        };
        let (job, settings) = {
            let mut state = self.state.lock().expect("dictation state lock");
            if !Self::accepts_chunk(&state) {
                state.pending -= 1;
                self.cleanup_canceled(&state);
                let _ = self.changed.try_send(());
                return;
            }
            if state.job.is_none() {
                match (self.start_job)(&self.request) {
                    Ok(job) => state.job = Some(job),
                    Err(error) => {
                        state.error = Some(error);
                        Self::wake_dispatchers(&mut state);
                        state.pending -= 1;
                        let _ = self.changed.try_send(());
                        return;
                    }
                }
            }
            let job = state.job.clone().expect("first dispatch admitted the job");
            let settings = job.settings.clone();
            (job, settings)
        };
        if self.jobs.is_cancelled(&job).unwrap_or(true) {
            self.cancel();
            let mut state = self.state.lock().expect("dictation state lock");
            state.pending -= 1;
            self.cleanup_canceled(&state);
            return;
        }
        let run = Arc::clone(self);
        let dispatch_job = job.clone();
        let can_dispatch: DispatchGate = Arc::new(move || {
            let state = run.state.lock().expect("dictation state lock");
            Self::accepts_chunk(&state) && !run.jobs.is_cancelled(&dispatch_job).unwrap_or(true)
        });
        let mut result = None;
        let mut retried = false;
        for attempt in 0..2 {
            if !can_dispatch() {
                break;
            }
            retried = attempt == 1;
            let started = Instant::now();
            result = Some(
                self.provider
                    .transcribe(settings.clone(), bytes.clone(), can_dispatch.clone())
                    .await,
            );
            {
                let mut state = self.state.lock().expect("dictation state lock");
                state.provider_ms += match &result {
                    Some(Ok(result)) => result.provider_ms,
                    _ => started.elapsed().as_millis(),
                };
            }
            match &result {
                Some(Err(error))
                    if matches!(
                        error.code,
                        SttJobErrorCode::ProviderSilence | SttJobErrorCode::ProviderEmptyTranscript
                    ) =>
                {
                    result = Some(Ok(TranscriptionResult {
                        transcript: String::new(),
                        provider_id: settings.provider_id,
                        provider_model: settings.provider_id.model(),
                        provider_ms: 0,
                        edges: None,
                    }));
                    break;
                }
                Some(Err(error)) if error.code == SttJobErrorCode::JobCanceled => break,
                Some(Err(error)) if attempt == 0 && error.retryable => {
                    let state = self.state.lock().expect("dictation state lock");
                    if Self::accepts_chunk(&state) && !self.jobs.is_cancelled(&job).unwrap_or(true)
                    {
                        self.publish_status(ordinal, PartialTranscriptStatus::Retrying);
                    }
                }
                _ => break,
            }
        }
        {
            let mut state = self.state.lock().expect("dictation state lock");
            state.pending -= 1;
            let published = if state.canceled || self.jobs.is_cancelled(&job).unwrap_or(true) {
                state.canceled = true;
                state.results.clear();
                Self::wake_dispatchers(&mut state);
                Vec::new()
            } else if Self::accepts_chunk(&state) {
                match result {
                    Some(Ok(result)) => {
                        if retried {
                            self.publish_status(ordinal, PartialTranscriptStatus::Recovered);
                        }
                        state.results.insert(ordinal, result);
                        self.take_contiguous_results(&mut state)
                    }
                    Some(Err(error)) => {
                        if error.code == SttJobErrorCode::JobCanceled {
                            state.canceled = true;
                            Self::wake_dispatchers(&mut state);
                            Vec::new()
                        } else {
                            state.credential_rejected |= matches!(
                                error.code,
                                SttJobErrorCode::MissingProviderKey
                                    | SttJobErrorCode::ProviderCredentialRejected
                            );
                            state.failures.insert(ordinal);
                            self.publish_status(ordinal, PartialTranscriptStatus::Gap);
                            self.take_contiguous_results(&mut state)
                        }
                    }
                    None => Vec::new(),
                }
            } else {
                Vec::new()
            };
            self.schedule_repairs(&mut state);
            for event in published {
                (self.publish_segment)(event);
            }
            self.cleanup_canceled(&state);
            let _ = self.changed.try_send(());
        }
    }

    fn publish_status(&self, ordinal: u32, status: PartialTranscriptStatus) {
        (self.publish_segment)(PartialTranscriptEvent {
            dictation_id: self.request.intent_id.clone(),
            ordinal,
            text: String::new(),
            status: Some(status),
        });
    }

    fn take_contiguous_results(&self, state: &mut SegmentState) -> Vec<PartialTranscriptEvent> {
        let mut published = Vec::new();
        loop {
            let ordinal = state.next_published_ordinal;
            let text = if state.failures.contains(&ordinal) {
                TRANSCRIPTION_GAP.to_string()
            } else if state.silent_ordinals.contains(&ordinal) {
                state.next_published_ordinal += 1;
                continue;
            } else if let Some(result) = state.results.get(&ordinal) {
                let start = state
                    .failures
                    .range(..ordinal)
                    .next_back()
                    .map_or(0, |gap| gap + 1);
                let previous = stitch(
                    state
                        .results
                        .range(start..ordinal)
                        .map(|(_, result)| result),
                );
                clean_seam(&previous, &result.transcript)
            } else {
                break;
            };
            if !text.is_empty() {
                published.push(PartialTranscriptEvent {
                    dictation_id: self.request.intent_id.clone(),
                    ordinal,
                    text,
                    status: None,
                });
            }
            state.next_published_ordinal += 1;
        }
        published
    }

    async fn wait_for_capacity(&self) -> Option<super::transcription_jobs::ProviderPermit> {
        let mut capacity = Box::pin(self.jobs.provider_permit());
        std::future::poll_fn(|context| {
            let mut state = self.state.lock().expect("dictation state lock");
            if !Self::accepts_chunk(&state) {
                return std::task::Poll::Ready(None);
            }
            match capacity.as_mut().poll(context) {
                std::task::Poll::Ready(permit) => std::task::Poll::Ready(Some(permit)),
                std::task::Poll::Pending => {
                    Self::register_dispatcher(&mut state, context.waker());
                    std::task::Poll::Pending
                }
            }
        })
        .await
    }

    fn accepts_chunk(state: &SegmentState) -> bool {
        !state.canceled && state.error.is_none()
    }

    fn register_dispatcher(state: &mut SegmentState, waker: &std::task::Waker) {
        if !state.dispatchers.iter().any(|saved| saved.will_wake(waker)) {
            state.dispatchers.push(waker.clone());
        }
    }

    fn wake_dispatchers(state: &mut SegmentState) {
        for waker in state.dispatchers.drain(..) {
            waker.wake();
        }
    }

    fn cleanup_canceled(&self, state: &SegmentState) {
        if state.canceled && state.pending == 0 && state.repair_tasks == 0 {
            if let Some(job) = &state.job {
                let _ = self.jobs.complete_success(job);
            }
        }
    }

    pub(crate) fn fail(&self, error: SttJobError) {
        let mut state = self.state.lock().expect("dictation state lock");
        if state.error.is_none() && !state.canceled {
            state.error = Some(error);
            state.audio = None;
            state.results.clear();
            Self::wake_dispatchers(&mut state);
        }
        let _ = self.changed.try_send(());
    }

    pub(crate) fn cancel(&self) {
        let mut state = self.state.lock().expect("dictation state lock");
        state.canceled = true;
        state.audio = None;
        state.results.clear();
        Self::wake_dispatchers(&mut state);
        if let Some(job) = &state.job {
            let _ = self.jobs.cancel(
                &job.request.owner,
                &job.request.session_id,
                &job.request.intent_id,
            );
        }
        self.cleanup_canceled(&state);
        let _ = self.changed.try_send(());
    }

    pub(crate) fn job(&self) -> Option<ActiveJob> {
        self.state.lock().ok()?.job.clone()
    }

    /// Failed short dictations retain their stitch if strict whole-audio recovery fails.
    pub(crate) async fn release(
        self: &Arc<Self>,
        audio: &NativeAudioArtifact,
    ) -> Result<(ActiveJob, DictationCompletion), SttJobError> {
        if !audio.is_complete {
            return Err(SttJobError::invalid());
        }
        self.submit(audio)?;
        let (job, mut completion) = self.finish().await?;
        if completion.partial && audio.metadata.duration_ms < REPAIR_DURATION_LIMIT_MS {
            let (whole, provider_elapsed) = self
                .until_canceled(async {
                    let _permit = self.jobs.provider_permit().await;
                    let run = Arc::clone(self);
                    let dispatch_job = job.clone();
                    let can_dispatch: DispatchGate = Arc::new(move || {
                        let state = run.state.lock().expect("dictation state lock");
                        Self::accepts_chunk(&state)
                            && !run.jobs.is_cancelled(&dispatch_job).unwrap_or(true)
                    });
                    if !can_dispatch() {
                        return (Err(job_canceled()), 0);
                    }
                    let started = Instant::now();
                    let result = self
                        .provider
                        .transcribe_whole(job.settings.clone(), audio.clone(), can_dispatch)
                        .await;
                    (result, started.elapsed().as_millis())
                })
                .await?;
            let mut state = self.state.lock().map_err(|_| SttJobError::invalid())?;
            if self.jobs.is_cancelled(&job)? {
                return Err(job_canceled());
            }
            state.provider_ms += whole
                .as_ref()
                .map_or(provider_elapsed, |result| result.provider_ms);
            if let Err(error) = &whole {
                completion.credential_rejected |= matches!(
                    error.code,
                    SttJobErrorCode::MissingProviderKey
                        | SttJobErrorCode::ProviderCredentialRejected
                );
            }
            if let Ok(result) = whole {
                if !result.transcript.trim().is_empty() {
                    completion.provider = result;
                    completion.partial = false;
                    completion.credential_rejected = false;
                    completion.has_speech = true;
                    for ordinal in &state.failures {
                        self.publish_status(*ordinal, PartialTranscriptStatus::Recovered);
                    }
                }
            }
            completion.provider.provider_ms = state.provider_ms;
        }
        completion.stop_to_transcript_ms = audio
            .stopped_at
            .map_or(0, |stop| stop.elapsed().as_millis());
        Ok((job, completion))
    }

    fn schedule_repairs(self: &Arc<Self>, state: &mut SegmentState) {
        if state.repairs_closed
            || state.duration_ms >= REPAIR_DURATION_LIMIT_MS
            || state.repairs.len() >= MAX_SEAM_REPAIRS
            || !Self::accepts_chunk(state)
        {
            return;
        }
        let Some(audio) = &state.audio else {
            return;
        };
        for pair in audio.segments.windows(2) {
            if state.repairs.len() >= MAX_SEAM_REPAIRS {
                break;
            }
            let left = pair[0].ordinal;
            if state.repairs.contains_key(&left) {
                continue;
            }
            let (Some(first), Some(second)) =
                (state.results.get(&left), state.results.get(&(left + 1)))
            else {
                continue;
            };
            if strip_edge_ellipses(&first.transcript).is_empty()
                || strip_edge_ellipses(&second.transcript).is_empty()
            {
                continue;
            }
            let prefix = stitch(state.results.range(..=left).map(|(_, result)| result));
            let overlap = clean_seam_details(&prefix, &second.transcript).1;
            let missing_or_uncertain_edges = first.provider_id.reports_edge_confidence()
                && (!first
                    .edges
                    .and_then(|edges| edges.last)
                    .is_some_and(|edge| edge.confident())
                    || !second
                        .edges
                        .and_then(|edges| edges.first)
                        .is_some_and(|edge| edge.confident()));
            if !overlap && pair[0].cut_kind != SegmentCutKind::Valley && !missing_or_uncertain_edges
            {
                continue;
            }
            let Some(bytes) = audio.pair_wav(left) else {
                continue;
            };
            let Some(job) = state.job.clone() else {
                continue;
            };
            state.repairs.insert(
                left,
                PairRepair {
                    pending: true,
                    result: None,
                },
            );
            state.repair_tasks += 1;
            let run = Arc::clone(self);
            tauri::async_runtime::spawn(async move {
                run.repair(left, bytes, job).await;
            });
        }
    }

    fn accepts_repair(state: &SegmentState, left: u32) -> bool {
        !state.canceled
            && state.error.is_none()
            && state.duration_ms < REPAIR_DURATION_LIMIT_MS
            && state
                .repairs
                .get(&left)
                .is_some_and(|repair| repair.pending)
    }

    async fn while_repair_pending<T>(
        &self,
        left: u32,
        future: impl Future<Output = T>,
    ) -> Option<T> {
        let mut future = Box::pin(future);
        std::future::poll_fn(|context| {
            let mut state = self.state.lock().expect("dictation state lock");
            if !Self::accepts_repair(&state, left) {
                return std::task::Poll::Ready(None);
            }
            Self::register_dispatcher(&mut state, context.waker());
            drop(state);
            future.as_mut().poll(context).map(Some)
        })
        .await
    }

    async fn repair(self: Arc<Self>, left: u32, bytes: Arc<[u8]>, job: ActiveJob) {
        let result = self
            .while_repair_pending(left, async {
                let _slot = self.jobs.repair_slot().await;
                let _permit = self.jobs.provider_permit().await;
                let run = Arc::clone(&self);
                let dispatch_job = job.clone();
                let can_dispatch: DispatchGate = Arc::new(move || {
                    let state = run.state.lock().expect("dictation state lock");
                    Self::accepts_repair(&state, left)
                        && !run.jobs.is_cancelled(&dispatch_job).unwrap_or(true)
                });
                if !can_dispatch() {
                    return (Err(job_canceled()), 0);
                }
                let started = Instant::now();
                let result = self
                    .provider
                    .transcribe_pair(job.settings.clone(), bytes, can_dispatch)
                    .await;
                (result, started.elapsed().as_millis())
            })
            .await;
        let mut state = self.state.lock().expect("dictation state lock");
        if let Some((outcome, elapsed)) = &result {
            state.provider_ms += outcome
                .as_ref()
                .map_or(*elapsed, |result| result.provider_ms);
        }
        if Self::accepts_repair(&state, left) && !self.jobs.is_cancelled(&job).unwrap_or(true) {
            let repair = state.repairs.get_mut(&left).expect("reserved repair");
            repair.result = result
                .and_then(|(outcome, _)| outcome.ok())
                .filter(TranscriptionResult::usable_repair);
            repair.pending = false;
        }
        state.repair_tasks -= 1;
        self.cleanup_canceled(&state);
        let _ = self.changed.try_send(());
    }

    fn retire_repairs(state: &mut SegmentState, keep: Option<u32>) {
        for (left, repair) in &mut state.repairs {
            if Some(*left) != keep {
                repair.pending = false;
            }
        }
        Self::wake_dispatchers(state);
    }

    /// Completed disjoint repairs consume two originals; earlier seams win.
    fn final_pieces(state: &SegmentState) -> Vec<Option<&TranscriptionResult>> {
        let mut pieces = Vec::new();
        let mut ordinal = 0;
        while ordinal < state.seen as u32 {
            let repair = if state.duration_ms < REPAIR_DURATION_LIMIT_MS {
                state
                    .repairs
                    .get(&ordinal)
                    .and_then(|repair| repair.result.as_ref())
            } else {
                None
            };
            if let Some(repair) = repair {
                pieces.push(Some(repair));
                ordinal += 2;
            } else {
                if state.failures.contains(&ordinal) {
                    pieces.push(None);
                } else if let Some(original) = state.results.get(&ordinal) {
                    pieces.push(Some(original));
                }
                ordinal += 1;
            }
        }
        pieces
    }

    fn final_seam(state: &SegmentState) -> Option<u32> {
        if state.duration_ms >= REPAIR_DURATION_LIMIT_MS {
            return None;
        }
        let final_left = state.results.iter().rev().find_map(|(&left, first)| {
            let second = state.results.get(&(left + 1))?;
            (!strip_edge_ellipses(&first.transcript).is_empty()
                && !strip_edge_ellipses(&second.transcript).is_empty())
            .then_some(left)
        })?;
        let mut ordinal = 0;
        while ordinal < final_left {
            ordinal += if state
                .repairs
                .get(&ordinal)
                .is_some_and(|repair| repair.result.is_some())
            {
                2
            } else {
                1
            };
        }
        (ordinal == final_left).then_some(final_left)
    }

    async fn until_canceled<T>(&self, future: impl Future<Output = T>) -> Result<T, SttJobError> {
        let mut future = Box::pin(future);
        std::future::poll_fn(|context| {
            let mut state = self.state.lock().expect("dictation state lock");
            if state.canceled {
                return std::task::Poll::Ready(Err(job_canceled()));
            }
            if let Some(error) = &state.error {
                return std::task::Poll::Ready(Err(error.clone()));
            }
            Self::register_dispatcher(&mut state, context.waker());
            drop(state);
            future.as_mut().poll(context).map(Ok)
        })
        .await
    }

    pub(crate) async fn finish(
        self: &Arc<Self>,
    ) -> Result<(ActiveJob, DictationCompletion), SttJobError> {
        {
            let mut state = self.state.lock().map_err(|_| SttJobError::invalid())?;
            if state.finishing {
                return Err(SttJobError::invalid());
            }
            state.finishing = true;
        }
        self.await_stitch().await
    }

    async fn await_stitch(
        self: &Arc<Self>,
    ) -> Result<(ActiveJob, DictationCompletion), SttJobError> {
        let mut changes = self.changes.lock().await;
        loop {
            {
                let mut state = self.state.lock().map_err(|_| SttJobError::invalid())?;
                if state.canceled {
                    return Err(job_canceled());
                }
                if let Some(error) = &state.error {
                    return Err(error.clone());
                }
                if !state.sealed {
                    return Err(SttJobError::invalid());
                }
                if state.pending == 0 && !state.repairs_closed {
                    self.schedule_repairs(&mut state);
                    state.repairs_closed = true;
                    let keep = Self::final_seam(&state);
                    Self::retire_repairs(&mut state, keep);
                }
                if state.pending == 0 && state.repair_tasks == 0 {
                    let job = state.job.clone().ok_or_else(SttJobError::invalid)?;
                    let mut result = TranscriptionResult {
                        transcript: String::new(),
                        provider_id: job.settings.provider_id,
                        provider_model: job.settings.provider_id.model(),
                        provider_ms: state.provider_ms,
                        edges: None,
                    };
                    let pieces = Self::final_pieces(&state);
                    let has_speech = pieces
                        .iter()
                        .flatten()
                        .any(|piece| !strip_edge_ellipses(&piece.transcript).is_empty());
                    let mut run = Vec::new();
                    let mut text = Vec::new();
                    for piece in pieces {
                        if let Some(piece) = piece {
                            run.push(piece);
                        } else {
                            text.push(stitch(run.drain(..)));
                            text.push(TRANSCRIPTION_GAP.into());
                        }
                    }
                    text.push(stitch(run.into_iter()));
                    result.transcript = text
                        .into_iter()
                        .filter(|text| !text.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                    if result.transcript.is_empty() {
                        return Err(SttJobError::new(
                            SttJobErrorCode::InvalidAudio,
                            SttJobStage::Provider,
                            true,
                            "The recording contained no confident speech.",
                        ));
                    }
                    return Ok((
                        job,
                        DictationCompletion {
                            provider: result,
                            partial: !state.failures.is_empty(),
                            credential_rejected: state.credential_rejected,
                            has_speech,
                            stop_to_transcript_ms: state
                                .audio
                                .as_ref()
                                .and_then(|audio| audio.stopped_at)
                                .map_or(0, |stop| stop.elapsed().as_millis()),
                        },
                    ));
                }
            }
            changes.recv().await;
        }
    }
}
