use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager};

use crate::audio::{
    ArtifactReleaseDisposition, ArtifactReleaseRequest, CaptureEvent, CapturePhase,
    DesktopAudioCapture, TrustedCaptureOwner,
};
use crate::providers::{self, TranscriptionResult};
use crate::services::dictation::{
    ActiveJob, DispatchGate, FrozenJobSettings, JobRequest, SegmentProvider, SegmentPublisher,
    SegmentedDictation, StartJob, SttJobError, SttJobErrorCode, SttJobStage,
};
use crate::services::transcription_jobs::{too_many_jobs, TranscriptionJobs, MAX_ACTIVE_JOBS};
use crate::storage::EchoStorage;

use super::errors::{
    artifact_error, artifact_unavailable, history_error, job_canceled, provider_error,
};
use super::pipeline::{self, TranscriptionContext};
use super::settings::{read_frozen_settings, resolve_credential};
use super::{TranscribeArtifactRequest, TranscribeArtifactResult};

#[derive(Clone)]
pub(crate) struct CaptureDictations {
    entries: Arc<Mutex<HashMap<String, Arc<CaptureDictation>>>>,
    capturing: Arc<Mutex<Option<String>>>,
    partial_publisher: SegmentPublisher,
    #[cfg(test)]
    test_deadlines: Arc<Mutex<Option<TestDeadlines>>>,
}

#[cfg(test)]
impl Default for CaptureDictations {
    fn default() -> Self {
        Self::with_partial_publisher(Arc::new(|_| {}))
    }
}

struct CaptureDictation {
    run: Arc<SegmentedDictation>,
    request: JobRequest,
    reads: tauri::async_runtime::Mutex<()>,
    released: AtomicBool,
    release_timer: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

#[cfg(test)]
#[derive(Default)]
struct TestDeadlines {
    now: std::time::Duration,
    pending: Vec<TestDeadline>,
}

#[cfg(test)]
struct TestDeadline {
    at: std::time::Duration,
    canceled: std::sync::mpsc::Receiver<()>,
    artifact_id: String,
    on_retired: Arc<dyn Fn(Vec<ActiveJob>) + Send + Sync>,
}

struct NativeProvider {
    app: AppHandle,
    client: reqwest::Client,
}

impl SegmentProvider for NativeProvider {
    fn transcribe(
        &self,
        settings: FrozenJobSettings,
        audio: Arc<[u8]>,
        can_dispatch: DispatchGate,
    ) -> Pin<Box<dyn Future<Output = Result<TranscriptionResult, SttJobError>> + Send + '_>> {
        Box::pin(async move {
            let key = resolve_credential(&self.app, settings.clone()).await?;
            if !can_dispatch() {
                return Err(job_canceled());
            }
            providers::transcribe_segment(
                &self.client,
                settings.provider_id,
                &pipeline::build_wav_request(audio, &settings),
                &key,
            )
            .await
            .map_err(|error| {
                if error.code == providers::ProviderErrorCode::Silence {
                    SttJobError::new(
                        SttJobErrorCode::ProviderSilence,
                        SttJobStage::Provider,
                        true,
                        "The segment contained no speech.",
                    )
                } else {
                    provider_error(error)
                }
            })
        })
    }
    fn transcribe_pair(
        &self,
        settings: FrozenJobSettings,
        audio: Arc<[u8]>,
        can_dispatch: DispatchGate,
    ) -> Pin<Box<dyn Future<Output = Result<TranscriptionResult, SttJobError>> + Send + '_>> {
        Box::pin(async move {
            let request = pipeline::build_wav_request(audio, &settings);
            pipeline::transcribe_strict_wav(
                &self.app,
                &self.client,
                settings,
                request,
                can_dispatch,
            )
            .await
        })
    }

    fn transcribe_whole(
        &self,
        settings: FrozenJobSettings,
        audio: crate::audio::NativeAudioArtifact,
        can_dispatch: DispatchGate,
    ) -> Pin<Box<dyn Future<Output = Result<TranscriptionResult, SttJobError>> + Send + '_>> {
        Box::pin(async move {
            let request = pipeline::build_transcription_request(&audio, &settings);
            pipeline::transcribe_strict_wav(
                &self.app,
                &self.client,
                settings,
                request,
                can_dispatch,
            )
            .await
        })
    }
}

impl CaptureDictations {
    pub(crate) fn with_partial_publisher(partial_publisher: SegmentPublisher) -> Self {
        Self {
            entries: Arc::default(),
            capturing: Arc::default(),
            partial_publisher,
            #[cfg(test)]
            test_deadlines: Arc::default(),
        }
    }

    fn get(
        &self,
        app: &AppHandle,
        request: JobRequest,
    ) -> Result<Arc<CaptureDictation>, SttJobError> {
        let jobs = app.state::<TranscriptionJobs>().inner().clone();
        let handle = app.clone();
        let registry = jobs.clone();
        let start_job = Arc::new(move |request: &JobRequest| {
            let settings = read_frozen_settings(&handle)?;
            let mut request = request.clone();
            request.created_at = handle
                .state::<EchoStorage>()
                .current_timestamp()
                .map_err(history_error)?;
            let job = registry.admit_initial(request, settings)?;
            pipeline::emit_started(&handle, &job)?;
            Ok(job)
        });
        self.get_with(
            request,
            jobs,
            Arc::new(NativeProvider {
                app: app.clone(),
                client: app.state::<reqwest::Client>().inner().clone(),
            }),
            start_job,
        )
    }

    fn get_with(
        &self,
        request: JobRequest,
        jobs: TranscriptionJobs,
        provider: Arc<dyn SegmentProvider>,
        start_job: StartJob,
    ) -> Result<Arc<CaptureDictation>, SttJobError> {
        if request.owner != "overlay" {
            return Err(SttJobError::invalid());
        }
        let mut entries = self.entries.lock().map_err(|_| SttJobError::invalid())?;
        if let Some(entry) = entries.get(&request.intent_id) {
            if entry.request.session_id != request.session_id
                || entry.request.artifact_id != request.artifact_id
            {
                return Err(SttJobError::invalid());
            }
            return Ok(Arc::clone(entry));
        }
        if entries.len() >= MAX_ACTIVE_JOBS {
            return Err(too_many_jobs());
        }
        let entry = Arc::new(CaptureDictation {
            run: SegmentedDictation::new_with_publisher(
                request.clone(),
                jobs,
                provider,
                start_job,
                self.partial_publisher.clone(),
            ),
            request,
            reads: tauri::async_runtime::Mutex::new(()),
            released: AtomicBool::new(false),
            release_timer: Mutex::new(None),
        });
        entries.insert(entry.request.intent_id.clone(), Arc::clone(&entry));
        Ok(entry)
    }

    async fn expire_unreleased(&self, artifact_id: &str) -> Vec<ActiveJob> {
        let entry = self.entries.lock().ok().and_then(|entries| {
            entries
                .values()
                .find(|entry| entry.request.artifact_id == artifact_id)
                .cloned()
        });
        let Some(entry) = entry else {
            return Vec::new();
        };
        let _read = entry.reads.lock().await;
        if entry.released.load(Ordering::Acquire) {
            return Vec::new();
        }
        self.cancel(&entry.request.intent_id);
        entry.run.job().into_iter().collect()
    }

    fn arm_release_deadline(
        &self,
        artifact_id: &str,
        timeout: std::time::Duration,
        on_retired: Arc<dyn Fn(Vec<ActiveJob>) + Send + Sync>,
    ) {
        let entry = self.entries.lock().ok().and_then(|entries| {
            entries
                .values()
                .find(|entry| entry.request.artifact_id == artifact_id)
                .cloned()
        });
        let Some(entry) = entry else { return };
        let timer = entry.release_deadline();
        #[cfg(test)]
        if let Some(clock) = self.test_deadlines.lock().unwrap().as_mut() {
            clock.pending.push(TestDeadline {
                at: clock.now + timeout,
                canceled: timer,
                artifact_id: artifact_id.to_owned(),
                on_retired,
            });
            return;
        }
        let registry = self.clone();
        let artifact_id = artifact_id.to_owned();
        tauri::async_runtime::spawn(async move {
            let expired = tauri::async_runtime::spawn_blocking(move || {
                matches!(
                    timer.recv_timeout(timeout),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                )
            })
            .await
            .unwrap_or(false);
            if expired {
                on_retired(registry.expire_unreleased(&artifact_id).await);
            }
        });
    }

    #[cfg(test)]
    async fn advance_deadlines(&self, duration: std::time::Duration) {
        let expired = {
            let mut clock = self.test_deadlines.lock().unwrap();
            let clock = clock.as_mut().expect("manual deadlines enabled");
            clock.now += duration;
            let pending = std::mem::take(&mut clock.pending);
            let (expired, pending): (Vec<TestDeadline>, Vec<TestDeadline>) =
                pending.into_iter().partition(|timer| timer.at <= clock.now);
            clock.pending = pending;
            expired
        };
        for timer in expired {
            if matches!(
                timer.canceled.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ) {
                (timer.on_retired)(self.expire_unreleased(&timer.artifact_id).await);
            }
        }
    }

    fn capture_deadline_event(
        &self,
        event: &CaptureEvent,
        timeout: std::time::Duration,
        on_retired: Arc<dyn Fn(Vec<ActiveJob>) + Send + Sync>,
    ) {
        let terminal_idle = event.phase == CapturePhase::Idle && event.outcome != "stop_requested";
        let stopped_intent = self.capturing.lock().ok().and_then(|mut current| {
            if terminal_idle {
                let stopped = event
                    .intent_id
                    .clone()
                    .or_else(|| event.error.is_none().then(|| current.clone()).flatten());
                if stopped == *current {
                    current.take();
                }
                stopped
            } else {
                if event.intent_id.is_some() {
                    *current = event.intent_id.clone();
                }
                None
            }
        });
        if terminal_idle && event.error.is_some() {
            if let Some(intent_id) = &event.intent_id {
                self.cancel(intent_id);
            }
            return;
        }
        let artifact_id = if matches!(event.outcome, "segment_closed" | "artifact_created") {
            event.artifact_id.clone()
        } else if terminal_idle {
            stopped_intent.and_then(|intent_id| {
                self.entries
                    .lock()
                    .ok()?
                    .get(&intent_id)
                    .map(|entry| entry.request.artifact_id.clone())
            })
        } else {
            None
        };
        if let Some(artifact_id) = artifact_id {
            self.arm_release_deadline(&artifact_id, timeout, on_retired);
        }
    }

    pub(crate) fn validate_session(
        &self,
        intent_id: &str,
        session_id: Option<&str>,
    ) -> Result<(), SttJobError> {
        let entries = self.entries.lock().map_err(|_| SttJobError::invalid())?;
        if let Some(entry) = entries.get(intent_id) {
            if session_id.is_some_and(|session| session != entry.request.session_id) {
                return Err(SttJobError::invalid());
            }
        }
        Ok(())
    }

    pub(crate) fn cancel(&self, intent_id: &str) {
        if let Some(entry) = self
            .entries
            .lock()
            .ok()
            .and_then(|mut entries| entries.remove(intent_id))
        {
            entry.release();
            entry.run.cancel();
        }
    }

    pub(crate) fn cancel_all(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            for (_, entry) in entries.drain() {
                entry.release();
                entry.run.cancel();
            }
        }
    }
}

pub(crate) fn capture_event(app: &AppHandle, event: &CaptureEvent) {
    let dictations = app.state::<CaptureDictations>();
    let entry = if event.outcome == "segment_closed" {
        match (&event.artifact_id, &event.session_id, &event.intent_id) {
            (Some(artifact_id), Some(session_id), Some(intent_id)) => dictations
                .get(
                    app,
                    JobRequest {
                        owner: "overlay".into(),
                        artifact_id: artifact_id.clone(),
                        session_id: session_id.clone(),
                        intent_id: intent_id.clone(),
                        artifact_intent_id: intent_id.clone(),
                        created_at: String::new(),
                    },
                )
                .ok(),
            _ => None,
        }
    } else {
        None
    };
    let handle = app.clone();
    dictations.capture_deadline_event(
        event,
        crate::audio::AudioCapturePolicy::default().artifact_ttl,
        Arc::new(move |jobs| emit_retired(&handle, jobs)),
    );
    let Some(entry) = entry else { return };
    let capture = app.state::<DesktopAudioCapture>().inner().clone();
    tauri::async_runtime::spawn(async move {
        entry.snapshot(&capture).await;
    });
}

fn emit_retired(app: &AppHandle, retired: Vec<ActiveJob>) {
    for job in retired {
        if let Ok(state) = app
            .state::<crate::services::runtime::EchoRuntime>()
            .job_cancelled(job.job_sequence)
        {
            super::events::emit_state(app, &state);
        }
    }
}

impl CaptureDictation {
    fn release_deadline(&self) -> std::sync::mpsc::Receiver<()> {
        let (cancel, timer) = std::sync::mpsc::channel();
        let mut pending = self.release_timer.lock().expect("release timer lock");
        if !self.released.load(Ordering::Acquire) {
            *pending = Some(cancel);
        }
        timer
    }

    fn release(&self) -> bool {
        let already_released = self.released.swap(true, Ordering::AcqRel);
        self.release_timer
            .lock()
            .expect("release timer lock")
            .take();
        already_released
    }

    async fn snapshot(&self, capture: &DesktopAudioCapture) {
        let _read = self.reads.lock().await;
        if self.released.load(Ordering::Acquire) {
            return;
        }
        let owner = TrustedCaptureOwner::from_window_label("overlay");
        match pipeline::read_bundle(capture, &owner, &self.request, false).await {
            Ok(bundle) => {
                if let Err(error) = self.run.submit(&bundle.audio) {
                    self.run.fail(error);
                }
                let _ =
                    release_snapshot(capture.clone(), owner, &self.request, bundle.lease_id).await;
            }
            Err(error) => self.run.fail(error),
        }
    }
}

async fn release_snapshot(
    capture: DesktopAudioCapture,
    owner: TrustedCaptureOwner,
    request: &JobRequest,
    lease_id: String,
) -> Result<(), SttJobError> {
    let artifact_id = request.artifact_id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        capture
            .release_artifact(ArtifactReleaseRequest {
                owner,
                artifact_id,
                lease_id,
                disposition: ArtifactReleaseDisposition::RetainUntilIdleExpiry,
            })
            .map_err(artifact_error)
    })
    .await
    .map_err(|_| artifact_unavailable())?
}

pub(super) async fn execute(
    app: &AppHandle,
    owner: &TrustedCaptureOwner,
    request: TranscribeArtifactRequest,
) -> Result<TranscribeArtifactResult, SttJobError> {
    let dictations = app.state::<CaptureDictations>();
    let entry = dictations.get(
        app,
        JobRequest {
            owner: owner.as_str().into(),
            artifact_id: request.artifact_id,
            session_id: request.session_id,
            artifact_intent_id: request.intent_id.clone(),
            intent_id: request.intent_id,
            created_at: String::new(),
        },
    )?;
    {
        let _read = entry.reads.lock().await;
        if entry.release() {
            return Err(SttJobError::invalid());
        }
    }
    let outcome = execute_released(app, owner, &entry).await;
    dictations
        .entries
        .lock()
        .map_err(|_| SttJobError::invalid())?
        .remove(&entry.request.intent_id);
    outcome
}

async fn execute_released(
    app: &AppHandle,
    owner: &TrustedCaptureOwner,
    entry: &CaptureDictation,
) -> Result<TranscribeArtifactResult, SttJobError> {
    let capture = app.state::<DesktopAudioCapture>().inner().clone();
    let mut bundle = match pipeline::read_bundle(&capture, owner, &entry.request, true).await {
        Ok(bundle) => bundle,
        Err(error) => {
            entry.run.fail(error.clone());
            return match entry.run.job() {
                Some(job) => {
                    pipeline::finish_failure(app, &app.state::<TranscriptionJobs>(), &job, error)
                }
                None => Err(error),
            };
        }
    };
    let audio = bundle.audio.clone();
    let result = maintain_lease(
        entry.run.release(&audio),
        &capture,
        owner,
        &entry.request,
        &mut bundle,
        std::time::Duration::from_secs(20),
    )
    .await;
    if let Err(error) = &result {
        entry.run.fail(error.clone());
    }
    let jobs = app.state::<TranscriptionJobs>();
    match result {
        Ok((job, result)) => {
            pipeline::finish_transcription(
                &TranscriptionContext {
                    app,
                    jobs: &jobs,
                    capture: &capture,
                    owner,
                },
                job,
                bundle,
                Ok(result),
            )
            .await
        }
        Err(error) => match entry.run.job() {
            Some(job) => {
                pipeline::finish_transcription(
                    &TranscriptionContext {
                        app,
                        jobs: &jobs,
                        capture: &capture,
                        owner,
                    },
                    job,
                    bundle,
                    Err(error),
                )
                .await
            }
            None => {
                let artifact_id = entry.request.artifact_id.clone();
                let owner = owner.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    capture.release_artifact(ArtifactReleaseRequest {
                        owner,
                        artifact_id,
                        lease_id: bundle.lease_id,
                        disposition: ArtifactReleaseDisposition::Discard,
                    })
                })
                .await;
                Err(error)
            }
        },
    }
}

pub(super) async fn maintain_lease<F, T>(
    completion: F,
    capture: &DesktopAudioCapture,
    owner: &TrustedCaptureOwner,
    request: &JobRequest,
    bundle: &mut pipeline::LeasedArtifact,
    interval: std::time::Duration,
) -> Result<T, SttJobError>
where
    F: Future<Output = Result<T, SttJobError>>,
{
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    let (tick, mut ticks) = tauri::async_runtime::channel(1);
    // Dropping stop also retires the timer if the command future is abandoned.
    let clock = tauri::async_runtime::spawn_blocking(move || {
        while matches!(
            stopped.recv_timeout(interval),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ) {
            let _ = tick.try_send(());
        }
    });
    let mut finished = Box::pin(completion);
    let result = loop {
        let mut tick = Box::pin(ticks.recv());
        let completion = std::future::poll_fn(|context| {
            if let std::task::Poll::Ready(result) = finished.as_mut().poll(context) {
                return std::task::Poll::Ready(Some(result));
            }
            tick.as_mut().poll(context).map(|_| None)
        })
        .await;
        if let Some(result) = completion {
            break result;
        }
        let capture = capture.clone();
        let owner = owner.clone();
        let artifact_id = request.artifact_id.clone();
        let previous_lease = bundle.lease_id.clone();
        let renewed = tauri::async_runtime::spawn_blocking(move || {
            let lease = capture
                .lease_artifact(owner.clone(), artifact_id.clone())
                .map_err(artifact_error)?;
            let _ = capture.release_artifact(ArtifactReleaseRequest {
                owner,
                artifact_id,
                lease_id: previous_lease,
                disposition: ArtifactReleaseDisposition::RetainUntilIdleExpiry,
            });
            Ok::<_, SttJobError>(lease.lease_id)
        })
        .await
        .map_err(|_| artifact_unavailable())
        .and_then(|result| result);
        match renewed {
            Ok(lease_id) => bundle.lease_id = lease_id,
            Err(error) => break Err(error),
        }
    };
    drop(stop);
    let _ = clock.await;
    result
}

#[cfg(test)]
#[path = "segments_tests/mod.rs"]
mod tests;
