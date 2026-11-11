use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::types::{
    CancelCapture, CaptureError, CaptureErrorCode, CaptureIntent, CapturePurpose,
    CaptureReservation, TrustedCaptureOwner,
};

#[derive(Clone)]
struct ActiveIntent {
    owner: TrustedCaptureOwner,
    intent_id: String,
    purpose: CapturePurpose,
    phase: super::types::CapturePhase,
    generation: u64,
    session_id: Option<String>,
}

#[derive(Default)]
pub(crate) struct CaptureGate {
    state: Mutex<GateState>,
    next_generation: AtomicU64,
    current_generation: AtomicU64,
}

#[derive(Default)]
struct GateState {
    active: Option<ActiveIntent>,
    cancelled: Vec<CancelledIntent>,
}

struct CancelledIntent {
    owner: TrustedCaptureOwner,
    intent_id: String,
    expires_at: Instant,
}

const CANCEL_TOMBSTONE_TTL: Duration = Duration::from_secs(5);
const MAX_CANCEL_TOMBSTONES: usize = 32;

impl CaptureGate {
    pub(crate) fn reserve(
        &self,
        owner: TrustedCaptureOwner,
        intent: CaptureIntent,
        diagnostics_token: Option<crate::microphone_diagnostic_event::DiagnosticToken>,
    ) -> Result<CaptureReservation, CaptureError> {
        let mut state = self.lock_state()?;
        state
            .cancelled
            .retain(|value| value.expires_at > Instant::now());
        if state
            .cancelled
            .iter()
            .any(|value| value.owner == owner && value.intent_id == intent.intent_id)
        {
            return Err(CaptureError::new(
                CaptureErrorCode::CaptureCancelled,
                "reserve",
            ));
        }
        if let Some(intent) = state.active.as_ref() {
            return Err(CaptureError::busy(intent.phase, intent.purpose));
        }
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        state.active = Some(ActiveIntent {
            owner: owner.clone(),
            intent_id: intent.intent_id.clone(),
            purpose: intent.purpose,
            phase: super::types::CapturePhase::PermissionPending,
            generation,
            session_id: None,
        });
        self.current_generation.store(generation, Ordering::Release);
        Ok(CaptureReservation {
            owner,
            intent_id: intent.intent_id,
            purpose: intent.purpose,
            diagnostics_token,
            generation,
        })
    }

    pub(crate) fn attach_session(
        &self,
        reservation: &CaptureReservation,
        session_id: &str,
    ) -> Result<(), CaptureError> {
        let mut state = self.lock_state()?;
        let intent = state.active.as_mut().ok_or_else(invalid_reservation)?;
        if !matches_reservation(intent, reservation) {
            return Err(invalid_reservation());
        }
        intent.session_id = Some(session_id.to_owned());
        Ok(())
    }

    pub(crate) fn set_phase(&self, generation: u64, phase: super::types::CapturePhase) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(intent) = state
                .active
                .as_mut()
                .filter(|intent| intent.generation == generation)
            {
                intent.phase = phase;
            }
        }
    }

    pub(crate) fn cancel(&self, request: &CancelCapture) -> Result<bool, CaptureError> {
        let mut state = self.lock_state()?;
        let Some(intent) = state.active.as_ref() else {
            add_tombstone(&mut state, request);
            return Ok(false);
        };
        if intent.owner != request.owner || intent.intent_id != request.intent_id {
            add_tombstone(&mut state, request);
            return Ok(false);
        }
        if request
            .session_id
            .as_deref()
            .is_some_and(|session_id| intent.session_id.as_deref() != Some(session_id))
        {
            return Err(CaptureError::new(
                CaptureErrorCode::InvalidReservation,
                "cancel",
            ));
        }
        add_tombstone(&mut state, request);
        state.active = None;
        self.current_generation.store(0, Ordering::Release);
        Ok(true)
    }

    pub(crate) fn release(&self, generation: u64) {
        if let Ok(mut state) = self.state.lock() {
            if state
                .active
                .as_ref()
                .is_some_and(|intent| intent.generation == generation)
            {
                state.active = None;
                self.current_generation.store(0, Ordering::Release);
            }
        }
    }

    pub(crate) fn invalidate(&self, generation: u64) {
        if self.current_generation.load(Ordering::Acquire) == generation {
            self.current_generation.store(0, Ordering::Release);
        }
    }

    pub(crate) fn invalidate_current(&self) {
        self.current_generation.store(0, Ordering::Release);
    }

    pub(crate) fn cancel_owner(&self, owner: &TrustedCaptureOwner) {
        if let Ok(mut state) = self.state.lock() {
            if state
                .active
                .as_ref()
                .is_some_and(|intent| intent.owner == *owner)
            {
                state.active = None;
                self.current_generation.store(0, Ordering::Release);
            }
            state.cancelled.retain(|intent| intent.owner != *owner);
        }
    }

    pub(crate) fn is_current(&self, generation: u64) -> bool {
        self.current_generation.load(Ordering::Acquire) == generation
    }

    pub(crate) fn pending(
        &self,
    ) -> Option<(
        super::types::CapturePhase,
        CapturePurpose,
        String,
        Option<String>,
    )> {
        self.state.lock().ok().and_then(|state| {
            state.active.as_ref().map(|intent| {
                (
                    intent.phase,
                    intent.purpose,
                    intent.intent_id.clone(),
                    intent.session_id.clone(),
                )
            })
        })
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, GateState>, CaptureError> {
        self.state.lock().map_err(|_| {
            CaptureError::new(CaptureErrorCode::CoordinatorUnavailable, "capture_gate")
        })
    }
}

fn add_tombstone(state: &mut GateState, request: &CancelCapture) {
    state
        .cancelled
        .retain(|value| value.expires_at > Instant::now());
    if state.cancelled.len() >= MAX_CANCEL_TOMBSTONES {
        state.cancelled.remove(0);
    }
    state.cancelled.push(CancelledIntent {
        owner: request.owner.clone(),
        intent_id: request.intent_id.clone(),
        expires_at: Instant::now() + CANCEL_TOMBSTONE_TTL,
    });
}

fn matches_reservation(intent: &ActiveIntent, reservation: &CaptureReservation) -> bool {
    intent.owner == reservation.owner
        && intent.intent_id == reservation.intent_id
        && intent.purpose == reservation.purpose
        && intent.generation == reservation.generation
}

fn invalid_reservation() -> CaptureError {
    CaptureError::new(CaptureErrorCode::InvalidReservation, "begin")
}
