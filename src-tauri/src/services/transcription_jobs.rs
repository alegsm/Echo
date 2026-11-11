use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) const MAX_ACTIVE_JOBS: usize = 6;
const MAX_TOMBSTONES: usize = 32;
const TOMBSTONE_TTL: Duration = Duration::from_secs(60);
const INTENT_ID_LIMIT: usize = 128;

use super::dictation::{
    ActiveJob, FrozenJobSettings, JobRequest, SttJobError, SttJobErrorCode, SttJobStage,
};
use delivery::DeliveryRegistry;

mod delivery;

#[derive(Clone)]
pub struct TranscriptionJobs {
    inner: Arc<Mutex<JobRegistry>>,
    provider_capacity: Arc<ProviderCapacity>,
    repair_capacity: Arc<ProviderCapacity>,
}

struct ProviderCapacity {
    returned: tauri::async_runtime::Sender<()>,
    available: tauri::async_runtime::Mutex<tauri::async_runtime::Receiver<()>>,
}

impl ProviderCapacity {
    fn new(size: usize) -> Self {
        let (returned, available) = tauri::async_runtime::channel(size);
        for _ in 0..size {
            returned
                .try_send(())
                .expect("new capacity channel is empty");
        }
        Self {
            returned,
            available: tauri::async_runtime::Mutex::new(available),
        }
    }
}

impl Default for TranscriptionJobs {
    fn default() -> Self {
        Self {
            inner: Arc::default(),
            provider_capacity: Arc::new(ProviderCapacity::new(MAX_ACTIVE_JOBS)),
            repair_capacity: Arc::new(ProviderCapacity::new(1)),
        }
    }
}

/// Owned by the request future until it exits, including after cancellation.
pub(crate) struct ProviderPermit(Arc<ProviderCapacity>);

impl Drop for ProviderPermit {
    fn drop(&mut self) {
        let _ = self.0.returned.try_send(());
    }
}

#[derive(Default)]
struct JobRegistry {
    next_sequence: u64,
    active: HashMap<JobKey, JobEntry>,
    tombstones: HashMap<JobKey, Instant>,
    deliveries: DeliveryRegistry,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct JobKey {
    owner: String,
    intent_id: String,
}

struct JobEntry {
    active: ActiveJob,
    canceled: bool,
}

impl TranscriptionJobs {
    /// Acquire before a provider permit so queued repairs consume no request capacity.
    pub(crate) async fn repair_slot(&self) -> ProviderPermit {
        self.repair_capacity.available.lock().await.recv().await;
        ProviderPermit(Arc::clone(&self.repair_capacity))
    }

    pub(crate) async fn provider_permit(&self) -> ProviderPermit {
        self.provider_capacity.available.lock().await.recv().await;
        ProviderPermit(Arc::clone(&self.provider_capacity))
    }

    pub fn admit_initial(
        &self,
        request: JobRequest,
        settings: FrozenJobSettings,
    ) -> Result<ActiveJob, SttJobError> {
        validate_request(&request)?;
        let mut registry = self.lock()?;
        registry.reap();
        registry.admit(request, settings)
    }

    pub fn cancel(
        &self,
        owner: &str,
        session_id: &str,
        intent_id: &str,
    ) -> Result<Option<u64>, SttJobError> {
        validate_identifier(owner)?;
        validate_identifier(session_id)?;
        validate_identifier(intent_id)?;
        let mut registry = self.lock()?;
        registry.reap();
        let key = JobKey::new(owner, intent_id);
        if let Some(job) = registry.active.get_mut(&key) {
            if job.active.request.session_id != session_id {
                return Err(SttJobError::invalid());
            }
            job.canceled = true;
            return Ok(Some(job.active.job_sequence));
        }
        registry.add_tombstone(key);
        Ok(None)
    }

    pub fn is_cancelled(&self, job: &ActiveJob) -> Result<bool, SttJobError> {
        let registry = self.lock()?;
        Ok(registry
            .active
            .get(&JobKey::from_request(&job.request))
            .is_none_or(|entry| entry.canceled || entry.active.job_sequence != job.job_sequence))
    }

    pub fn complete_success(&self, job: &ActiveJob) -> Result<bool, SttJobError> {
        let mut registry = self.lock()?;
        let key = JobKey::from_request(&job.request);
        let removed = registry.remove_matching(&key, job);
        if removed.is_some() {
            registry.add_tombstone(key);
        }
        Ok(removed.is_some_and(|entry| !entry.canceled))
    }

    pub fn complete_failure(&self, job: &ActiveJob) -> Result<bool, SttJobError> {
        let mut registry = self.lock()?;
        let key = JobKey::from_request(&job.request);
        let Some(entry) = registry.remove_matching(&key, job) else {
            return Ok(false);
        };
        registry.add_tombstone(key);
        if entry.canceled {
            return Ok(false);
        }
        Ok(true)
    }

    pub fn cancel_owner(&self, owner: &str) -> Result<usize, SttJobError> {
        validate_identifier(owner)?;
        let mut registry = self.lock()?;
        let mut canceled = 0;
        for (key, entry) in &mut registry.active {
            if key.owner == owner && !entry.canceled {
                entry.canceled = true;
                canceled += 1;
            }
        }
        registry.deliveries.remove_owner(owner);
        Ok(canceled)
    }

    pub fn authorize_delivery(
        &self,
        job: &ActiveJob,
        transcript_id: &str,
        record_revision: u64,
    ) -> Result<(), SttJobError> {
        validate_identifier(transcript_id)?;
        let mut registry = self.lock()?;
        registry.reap();
        registry
            .deliveries
            .authorize(job, transcript_id, record_revision);
        Ok(())
    }

    pub fn delivery_revision(
        &self,
        owner: &str,
        session_id: &str,
        intent_id: &str,
        transcript_id: &str,
    ) -> Result<u64, SttJobError> {
        for value in [owner, session_id, intent_id, transcript_id] {
            validate_identifier(value)?;
        }
        let mut registry = self.lock()?;
        registry.reap();
        let proof = registry
            .deliveries
            .revision(owner, intent_id, session_id, transcript_id)
            .ok_or_else(SttJobError::invalid)?;
        Ok(proof)
    }

    pub fn complete_delivery(&self, owner: &str, intent_id: &str) -> Result<(), SttJobError> {
        validate_identifier(owner)?;
        validate_identifier(intent_id)?;
        self.lock()?.deliveries.complete(owner, intent_id);
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, JobRegistry>, SttJobError> {
        self.inner.lock().map_err(|_| {
            SttJobError::new(
                SttJobErrorCode::ProviderFailed,
                SttJobStage::Provider,
                true,
                "Transcription job state is unavailable.",
            )
        })
    }
}

impl JobRegistry {
    fn admit(
        &mut self,
        request: JobRequest,
        settings: FrozenJobSettings,
    ) -> Result<ActiveJob, SttJobError> {
        let key = JobKey::from_request(&request);
        if self.tombstones.contains_key(&key) {
            return Err(job_canceled());
        }
        if self.active.len() >= MAX_ACTIVE_JOBS {
            return Err(too_many_jobs());
        }
        if self.active.contains_key(&key) {
            return Err(SttJobError::invalid());
        }
        self.next_sequence = self.next_sequence.saturating_add(1);
        let active = ActiveJob {
            job_sequence: self.next_sequence,
            request,
            settings,
        };
        self.active.insert(
            key,
            JobEntry {
                active: active.clone(),
                canceled: false,
            },
        );
        Ok(active)
    }

    fn add_tombstone(&mut self, key: JobKey) {
        trim_oldest(&mut self.tombstones, MAX_TOMBSTONES);
        self.tombstones.insert(key, Instant::now() + TOMBSTONE_TTL);
    }

    fn reap(&mut self) {
        let now = Instant::now();
        self.tombstones.retain(|_, expires_at| *expires_at > now);
        self.deliveries.reap(now);
    }

    fn remove_matching(&mut self, key: &JobKey, job: &ActiveJob) -> Option<JobEntry> {
        let matches = self
            .active
            .get(key)
            .is_some_and(|entry| entry.active.job_sequence == job.job_sequence);
        matches.then(|| self.active.remove(key)).flatten()
    }
}

impl JobKey {
    fn new(owner: &str, intent_id: &str) -> Self {
        Self {
            owner: owner.to_owned(),
            intent_id: intent_id.to_owned(),
        }
    }

    fn from_request(request: &JobRequest) -> Self {
        Self::new(&request.owner, &request.intent_id)
    }
}

fn trim_oldest(entries: &mut HashMap<JobKey, Instant>, limit: usize) {
    if entries.len() < limit {
        return;
    }
    if let Some(oldest) = entries
        .iter()
        .min_by_key(|(_, expires_at)| **expires_at)
        .map(|(key, _)| key.clone())
    {
        entries.remove(&oldest);
    }
}

fn validate_request(request: &JobRequest) -> Result<(), SttJobError> {
    validate_identifier(&request.owner)?;
    validate_identifier(&request.artifact_id)?;
    validate_identifier(&request.session_id)?;
    validate_identifier(&request.intent_id)?;
    validate_identifier(&request.artifact_intent_id)
}

fn validate_identifier(value: &str) -> Result<(), SttJobError> {
    let valid = !value.trim().is_empty() && value.len() <= INTENT_ID_LIMIT;
    valid.then_some(()).ok_or_else(SttJobError::invalid)
}

pub(crate) const fn too_many_jobs() -> SttJobError {
    SttJobError::new(
        SttJobErrorCode::TooManyJobs,
        SttJobStage::Provider,
        true,
        "Too many transcription jobs are already running.",
    )
}

pub(crate) const fn job_canceled() -> SttJobError {
    SttJobError::new(
        SttJobErrorCode::JobCanceled,
        SttJobStage::Provider,
        false,
        "Transcription was canceled.",
    )
}

#[cfg(test)]
#[path = "transcription_jobs_tests.rs"]
mod tests;
