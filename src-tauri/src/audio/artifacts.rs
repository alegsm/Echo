use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::resample::{CHANNEL_COUNT, SAMPLE_RATE};
use super::types::{
    AudioArtifactLease, AudioArtifactMetadata, AudioCapturePolicy, CaptureError, CaptureErrorCode,
    NativeAudioArtifact, TrustedCaptureOwner,
};
use super::{AudioSegment, CapturePurpose};

pub(crate) struct BundleIdentity {
    pub(crate) owner: TrustedCaptureOwner,
    pub(crate) artifact_id: String,
    pub(crate) session_id: String,
    pub(crate) intent_id: String,
    pub(crate) purpose: CapturePurpose,
}

pub(crate) struct ArtifactRelease<'a> {
    pub(crate) owner: &'a TrustedCaptureOwner,
    pub(crate) artifact_id: &'a str,
    pub(crate) lease_id: &'a str,
    pub(crate) retain_until_idle_expiry: bool,
    pub(crate) now: Instant,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum BundleState {
    Recording,
    Sealed,
    Abandoned,
}

struct StoredArtifact {
    stopped_at: Option<Instant>,
    owner: TrustedCaptureOwner,
    metadata: AudioArtifactMetadata,
    segments: Vec<AudioSegment>,
    expires_at: Instant,
    leases: HashMap<String, Instant>,
    is_available: bool,
    state: BundleState,
}

pub(crate) struct ArtifactStore {
    entries: HashMap<String, StoredArtifact>,
    maximum_count: usize,
    maximum_bundle_bytes: usize,
    maximum_total_bytes: usize,
    artifact_ttl: Duration,
    lease_ttl: Duration,
    release_ttl: Duration,
}

impl ArtifactStore {
    pub(crate) fn new(policy: &AudioCapturePolicy) -> Self {
        Self {
            entries: HashMap::new(),
            maximum_count: policy.maximum_artifact_count,
            maximum_bundle_bytes: policy.maximum_artifact_bytes,
            maximum_total_bytes: policy.maximum_total_artifact_bytes,
            artifact_ttl: policy.artifact_ttl,
            lease_ttl: policy.lease_ttl,
            release_ttl: policy.release_ttl,
        }
    }

    pub(crate) fn create(
        &mut self,
        identity: BundleIdentity,
        now: Instant,
    ) -> Result<(), CaptureError> {
        self.reap(now);
        self.make_capacity(1, self.maximum_bundle_bytes)?;
        let metadata = AudioArtifactMetadata {
            artifact_id: identity.artifact_id.clone(),
            session_id: identity.session_id,
            intent_id: identity.intent_id,
            purpose: identity.purpose,
            mime_type: "audio/wav",
            byte_size: 0,
            duration_ms: 0,
            sample_rate: SAMPLE_RATE,
            channel_count: CHANNEL_COUNT,
        };
        self.entries.insert(
            identity.artifact_id,
            StoredArtifact {
                owner: identity.owner,
                metadata,
                segments: Vec::new(),
                expires_at: now + self.artifact_ttl,
                leases: HashMap::new(),
                is_available: true,
                state: BundleState::Recording,
                stopped_at: None,
            },
        );
        Ok(())
    }

    pub(crate) fn append(
        &mut self,
        owner: &TrustedCaptureOwner,
        artifact_id: &str,
        segment: AudioSegment,
        now: Instant,
    ) -> Result<(), CaptureError> {
        self.reap(now);
        let incoming_bytes = segment.bytes.as_ref().map_or(0, |bytes| bytes.len());
        let maximum_bytes = self.maximum_bundle_bytes;
        let entry = self.entry_for_owner(owner, artifact_id, "segment_append")?;
        if entry.metadata.byte_size.saturating_add(incoming_bytes) > maximum_bytes {
            return Err(CaptureError::new(
                CaptureErrorCode::ArtifactSizeLimit,
                "segment_append",
            ));
        }
        entry.metadata.byte_size += incoming_bytes;
        entry.metadata.duration_ms += segment.duration_ms;
        entry.segments.push(segment);
        Ok(())
    }

    pub(crate) fn seal(
        &mut self,
        owner: &TrustedCaptureOwner,
        artifact_id: &str,
        duration_ms: u64,
        stopped_at: Instant,
        now: Instant,
    ) -> Result<AudioArtifactMetadata, CaptureError> {
        let ttl = self.artifact_ttl;
        let entry = self.entry_for_owner(owner, artifact_id, "artifact_seal")?;
        entry.state = BundleState::Sealed;
        entry.stopped_at = Some(stopped_at);
        entry.metadata.duration_ms = duration_ms;
        entry.expires_at = now + ttl;
        Ok(entry.metadata.clone())
    }

    pub(crate) fn find_session(
        &self,
        owner: &TrustedCaptureOwner,
        session_id: &str,
        intent_id: &str,
    ) -> Option<AudioArtifactMetadata> {
        self.entries
            .values()
            .find(|entry| {
                entry.owner == *owner
                    && entry.metadata.session_id == session_id
                    && entry.metadata.intent_id == intent_id
                    && entry.state == BundleState::Sealed
                    && entry.is_available
            })
            .map(|entry| entry.metadata.clone())
    }

    pub(crate) fn lease(
        &mut self,
        owner: &TrustedCaptureOwner,
        artifact_id: &str,
        now: Instant,
    ) -> Result<AudioArtifactLease, CaptureError> {
        self.reap(now);
        let lease_ttl = self.lease_ttl;
        let entry = self.entry_for_owner(owner, artifact_id, "artifact_lease")?;
        if !entry.is_available {
            return Err(CaptureError::new(
                CaptureErrorCode::ArtifactExpired,
                "artifact_lease",
            ));
        }
        let lease_id = uuid::Uuid::new_v4().to_string();
        entry.leases.insert(lease_id.clone(), now + lease_ttl);
        Ok(AudioArtifactLease { lease_id })
    }

    pub(crate) fn read(
        &mut self,
        owner: &TrustedCaptureOwner,
        artifact_id: &str,
        lease_id: &str,
        now: Instant,
    ) -> Result<NativeAudioArtifact, CaptureError> {
        self.reap(now);
        let entry = self.entry_for_owner(owner, artifact_id, "artifact_read")?;
        let is_valid = entry
            .leases
            .get(lease_id)
            .is_some_and(|expires_at| *expires_at > now);
        if !is_valid {
            return Err(CaptureError::new(
                CaptureErrorCode::ArtifactExpired,
                "artifact_read",
            ));
        }
        Ok(NativeAudioArtifact {
            stopped_at: entry.stopped_at,
            is_complete: entry.state == BundleState::Sealed,
            metadata: entry.metadata.clone(),
            segments: Arc::from(entry.segments.as_slice()),
        })
    }

    pub(crate) fn release(&mut self, request: ArtifactRelease<'_>) -> Result<(), CaptureError> {
        let ArtifactRelease {
            owner,
            artifact_id,
            lease_id,
            retain_until_idle_expiry,
            now,
        } = request;
        let release_ttl = self.release_ttl;
        let entry = self.entry_for_owner(owner, artifact_id, "artifact_release")?;
        let is_valid = entry
            .leases
            .remove(lease_id)
            .is_some_and(|expires_at| expires_at > now);
        if !is_valid {
            return Err(CaptureError::new(
                CaptureErrorCode::ArtifactExpired,
                "artifact_release",
            ));
        }
        if retain_until_idle_expiry && entry.leases.is_empty() {
            entry.expires_at = now + release_ttl;
        }
        if !retain_until_idle_expiry {
            entry.is_available = false;
        }
        if entry.leases.is_empty() && !entry.is_available {
            self.entries.remove(artifact_id);
        }
        Ok(())
    }

    pub(crate) fn discard(
        &mut self,
        owner: &TrustedCaptureOwner,
        artifact_id: &str,
    ) -> Result<(), CaptureError> {
        let entry = self.entry_for_owner(owner, artifact_id, "artifact_discard")?;
        entry.is_available = false;
        if entry.state == BundleState::Recording {
            entry.state = BundleState::Abandoned;
        }
        if entry.leases.is_empty() {
            self.entries.remove(artifact_id);
        }
        Ok(())
    }

    pub(crate) fn discard_owner(&mut self, owner: &TrustedCaptureOwner) {
        self.entries.retain(|_, entry| entry.owner != *owner);
    }

    pub(crate) fn reap(&mut self, now: Instant) {
        self.entries.retain(|_, entry| {
            entry.leases.retain(|_, expires_at| *expires_at > now);
            if entry.state == BundleState::Recording || !entry.leases.is_empty() {
                return true;
            }
            entry.is_available && entry.expires_at > now
        });
    }

    pub(crate) fn usage(&self) -> (usize, usize) {
        (
            self.entries.len(),
            self.entries
                .values()
                .map(|entry| entry.metadata.byte_size)
                .sum(),
        )
    }

    fn entry_for_owner(
        &mut self,
        owner: &TrustedCaptureOwner,
        artifact_id: &str,
        stage: &'static str,
    ) -> Result<&mut StoredArtifact, CaptureError> {
        let entry = self
            .entries
            .get_mut(artifact_id)
            .ok_or_else(|| CaptureError::new(CaptureErrorCode::ArtifactExpired, stage))?;
        if entry.owner != *owner {
            return Err(CaptureError::new(CaptureErrorCode::InvalidOwner, stage));
        }
        Ok(entry)
    }

    fn make_capacity(
        &mut self,
        incoming_count: usize,
        incoming_bytes: usize,
    ) -> Result<(), CaptureError> {
        while self.entries.len() + incoming_count > self.maximum_count
            || self
                .entries
                .values()
                .map(|entry| {
                    if entry.state == BundleState::Recording {
                        self.maximum_bundle_bytes
                    } else {
                        entry.metadata.byte_size
                    }
                })
                .sum::<usize>()
                .saturating_add(incoming_bytes)
                > self.maximum_total_bytes
        {
            let candidate = self
                .entries
                .iter()
                .filter(|(_, entry)| {
                    entry.leases.is_empty() && entry.state != BundleState::Recording
                })
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(id, _)| id.clone());
            let Some(artifact_id) = candidate else {
                return Err(CaptureError::new(
                    CaptureErrorCode::ArtifactCapacityReached,
                    "artifact_store",
                ));
            };
            self.entries.remove(&artifact_id);
        }
        Ok(())
    }
}

impl NativeAudioArtifact {
    pub(crate) fn whole_wav(&self) -> Arc<[u8]> {
        assemble_wav(
            self.segments
                .iter()
                .filter_map(|segment| segment.bytes.as_ref()),
        )
    }

    /// Select exactly two adjacent, encoded pieces, including during capture.
    pub(crate) fn pair_wav(&self, left: u32) -> Option<Arc<[u8]>> {
        let pair = self.segments.get(left as usize..left as usize + 2)?;
        if pair[0].ordinal != left || pair[1].ordinal != left + 1 {
            return None;
        }
        let first = pair[0].bytes.as_ref().filter(|bytes| bytes.len() > 44)?;
        let second = pair[1].bytes.as_ref().filter(|bytes| bytes.len() > 44)?;
        Some(assemble_wav([first, second].into_iter()))
    }
}

fn assemble_wav<'a>(mut pieces: impl Iterator<Item = &'a Arc<[u8]>> + Clone) -> Arc<[u8]> {
    let Some(first) = pieces.next() else {
        return Arc::from([]);
    };
    let mut pieces = pieces.peekable();
    if pieces.peek().is_none() {
        return Arc::clone(first);
    }
    let capacity = first.len() + pieces.clone().map(|piece| piece.len() - 44).sum::<usize>();
    let mut bytes = Vec::with_capacity(capacity);
    bytes.extend_from_slice(first);
    for piece in pieces {
        bytes.extend_from_slice(&piece[44..]);
    }
    let data_length = (bytes.len() - 44) as u32;
    bytes[4..8].copy_from_slice(&(36 + data_length).to_le_bytes());
    bytes[40..44].copy_from_slice(&data_length.to_le_bytes());
    Arc::from(bytes)
}
