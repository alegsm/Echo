use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::services::dictation::ActiveJob;

const MAX_DELIVERY_PROOFS: usize = 32;
const DELIVERY_PROOF_TTL: Duration = Duration::from_secs(300);

#[derive(Default)]
pub(super) struct DeliveryRegistry {
    proofs: HashMap<DeliveryKey, DeliveryProof>,
}

struct DeliveryProof {
    session_id: String,
    transcript_id: String,
    record_revision: u64,
    expires_at: Instant,
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct DeliveryKey {
    owner: String,
    intent_id: String,
}

impl DeliveryRegistry {
    pub(super) fn authorize(&mut self, job: &ActiveJob, transcript_id: &str, record_revision: u64) {
        if self.proofs.len() >= MAX_DELIVERY_PROOFS {
            if let Some(oldest) = self
                .proofs
                .iter()
                .min_by_key(|(_, proof)| proof.expires_at)
                .map(|(key, _)| key.to_owned())
            {
                self.proofs.remove(&oldest);
            }
        }
        self.proofs.insert(
            DeliveryKey::new(&job.request.owner, &job.request.intent_id),
            DeliveryProof {
                session_id: job.request.session_id.clone(),
                transcript_id: transcript_id.to_owned(),
                record_revision,
                expires_at: Instant::now() + DELIVERY_PROOF_TTL,
            },
        );
    }

    pub(super) fn revision(
        &self,
        owner: &str,
        intent_id: &str,
        session_id: &str,
        transcript_id: &str,
    ) -> Option<u64> {
        self.proofs
            .get(&DeliveryKey::new(owner, intent_id))
            .filter(|proof| proof.session_id == session_id && proof.transcript_id == transcript_id)
            .map(|proof| proof.record_revision)
    }

    pub(super) fn complete(&mut self, owner: &str, intent_id: &str) {
        self.proofs.remove(&DeliveryKey::new(owner, intent_id));
    }

    pub(super) fn remove_owner(&mut self, owner: &str) {
        self.proofs.retain(|key, _| key.owner != owner);
    }

    pub(super) fn reap(&mut self, now: Instant) {
        self.proofs.retain(|_, proof| proof.expires_at > now);
    }
}

impl DeliveryKey {
    fn new(owner: &str, intent_id: &str) -> Self {
        Self {
            owner: owner.to_owned(),
            intent_id: intent_id.to_owned(),
        }
    }
}
