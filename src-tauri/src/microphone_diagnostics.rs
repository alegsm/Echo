use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const WINDOW_MS: u64 = 500;
const CHANNEL_CAPACITY: usize = 32;
const PLAY_NOT_STARTED: u64 = u64::MAX;

mod callback;
pub use callback::FinalCaptureMetrics;

pub use crate::microphone_diagnostic_event::{
    CaptureDiagnosticIdentity, DiagnosticToken, MicrophoneFields,
};
use crate::microphone_metrics::{
    stall_threshold_ms, summarize_samples, BlockMetrics, WindowMetrics, PEAK_SILENCE_THRESHOLD,
    RMS_SILENCE_THRESHOLD, ZERO_RATIO_THRESHOLD,
};

#[derive(Clone)]
pub struct CallbackDiagnostics {
    sender: SyncSender<Message>,
    enabled: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
    play_started_ms: Arc<AtomicU64>,
    origin: Instant,
    token: Option<DiagnosticToken>,
}

pub struct MicrophoneDiagnostics {
    session_id: String,
    identity: CaptureDiagnosticIdentity,
    callback: CallbackDiagnostics,
    worker: Option<JoinHandle<()>>,
    token: Option<DiagnosticToken>,
}

struct WorkerControl {
    enabled: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
    play_started_ms: Arc<AtomicU64>,
    origin: Instant,
    identity: CaptureDiagnosticIdentity,
    token: Option<DiagnosticToken>,
}

enum Message {
    Block(BlockMetrics),
    StreamError(&'static str),
    Finish(FinalCaptureMetrics),
    Shutdown,
}

impl MicrophoneDiagnostics {
    pub fn start_with_token(
        session_id: &str,
        identity: CaptureDiagnosticIdentity,
        token: Option<DiagnosticToken>,
    ) -> Self {
        let identity = identity.with_token(token.clone());
        let (sender, receiver) = mpsc::sync_channel(CHANNEL_CAPACITY);
        let enabled = Arc::new(AtomicBool::new(true));
        let dropped = Arc::new(AtomicU64::new(0));
        let play_started_ms = Arc::new(AtomicU64::new(PLAY_NOT_STARTED));
        let callback = CallbackDiagnostics {
            sender,
            enabled: Arc::clone(&enabled),
            dropped: Arc::clone(&dropped),
            play_started_ms: Arc::clone(&play_started_ms),
            origin: Instant::now(),
            token: token.clone(),
        };
        let worker_control = WorkerControl {
            enabled: Arc::clone(&enabled),
            dropped: Arc::clone(&dropped),
            play_started_ms,
            origin: callback.origin,
            identity: identity.clone(),
            token: token.clone(),
        };
        let worker_session_id = session_id.to_string();
        let worker = std::thread::Builder::new()
            .name("echo-microphone-diagnostics".to_string())
            .spawn(move || run_worker(worker_session_id, receiver, worker_control))
            .ok();
        if worker.is_none() {
            callback.enabled.store(false, Ordering::Release);
            identity.emit(
                session_id,
                "native_observer_worker_failed",
                log::Level::Error,
                MicrophoneFields {
                    error_name: Some("observer_thread_unavailable"),
                    ..Default::default()
                },
            );
        }
        Self {
            session_id: session_id.to_string(),
            identity,
            callback,
            worker,
            token,
        }
    }

    pub fn callback(&self) -> CallbackDiagnostics {
        self.callback.clone()
    }

    pub fn is_active(&self) -> bool {
        self.token.as_ref().is_none_or(DiagnosticToken::is_active)
    }

    pub fn mark_play_started(&self) {
        self.callback.play_started_ms.store(
            self.callback.origin.elapsed().as_millis() as u64,
            Ordering::Release,
        );
    }

    pub fn emit(&self, event: &'static str, level: log::Level, fields: MicrophoneFields) {
        if self.token.as_ref().is_some_and(|token| !token.is_active()) {
            return;
        }
        self.identity.emit(&self.session_id, event, level, fields);
    }

    pub fn finish(mut self, metrics: FinalCaptureMetrics) {
        let _ = self.callback.sender.send(Message::Finish(metrics));
        self.join();
    }

    pub fn shutdown(mut self) {
        self.stop_worker();
    }

    fn stop_worker(&mut self) {
        self.callback.enabled.store(false, Ordering::Release);
        let _ = self.callback.sender.try_send(Message::Shutdown);
        self.join();
    }

    fn join(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for MicrophoneDiagnostics {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

struct WorkerState {
    identity: CaptureDiagnosticIdentity,
    window: WindowMetrics,
    total: WindowMetrics,
    window_started_ms: Option<u64>,
    completed_window_count: u64,
    callback_count: u64,
    longest_gap_ms: u64,
    last_callback_ms: Option<u64>,
    recent_gaps: VecDeque<u64>,
    silence_windows: u8,
    is_silent: bool,
    stall_started_ms: Option<u64>,
}

impl WorkerState {
    fn new(identity: CaptureDiagnosticIdentity) -> Self {
        Self {
            identity,
            window: WindowMetrics::default(),
            total: WindowMetrics::default(),
            window_started_ms: None,
            completed_window_count: 0,
            callback_count: 0,
            longest_gap_ms: 0,
            last_callback_ms: None,
            recent_gaps: VecDeque::new(),
            silence_windows: 0,
            is_silent: false,
            stall_started_ms: None,
        }
    }
}

fn run_worker(session_id: String, receiver: Receiver<Message>, control: WorkerControl) {
    let mut state = WorkerState::new(control.identity.clone());
    loop {
        if control
            .token
            .as_ref()
            .is_some_and(|token| !token.is_active())
        {
            break;
        }
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(Message::Block(block)) => {
                process_block(&session_id, &mut state, block, &control.play_started_ms)
            }
            Ok(Message::StreamError(error_name)) => state.identity.emit(
                &session_id,
                "native_stream_error",
                log::Level::Error,
                MicrophoneFields {
                    error_name: Some(error_name),
                    ..Default::default()
                },
            ),
            Ok(Message::Finish(metrics)) => {
                emit_final(&session_id, &state, &control, metrics);
                break;
            }
            Ok(Message::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => check_stall(
                &session_id,
                &mut state,
                control.origin.elapsed().as_millis() as u64,
            ),
        }
        if !control.enabled.load(Ordering::Acquire) {
            break;
        }
    }
}

fn process_block(
    session_id: &str,
    state: &mut WorkerState,
    block: BlockMetrics,
    play_started_ms: &AtomicU64,
) {
    let window_started = state.window_started_ms.get_or_insert(block.at_ms);
    if block.at_ms.saturating_sub(*window_started) >= WINDOW_MS {
        flush_window(session_id, state);
        state.window_started_ms = Some(block.at_ms);
    }
    state.callback_count += 1;
    if state.callback_count == 1 {
        let play_started = play_started_ms.load(Ordering::Acquire);
        state.identity.emit(
            session_id,
            "native_first_callback",
            log::Level::Info,
            MicrophoneFields {
                elapsed_ms: (play_started != PLAY_NOT_STARTED)
                    .then(|| block.at_ms.saturating_sub(play_started)),
                ..Default::default()
            },
        );
    }
    if let Some(previous) = state.last_callback_ms {
        let gap = block.at_ms.saturating_sub(previous);
        state.longest_gap_ms = state.longest_gap_ms.max(gap);
        if state.recent_gaps.len() == 31 {
            state.recent_gaps.pop_front();
        }
        state.recent_gaps.push_back(gap);
        if let Some(started) = state.stall_started_ms.take() {
            state.identity.emit(
                session_id,
                "native_callback_stall_ended",
                log::Level::Info,
                MicrophoneFields {
                    dropout_duration_ms: Some(block.at_ms.saturating_sub(started)),
                    ..Default::default()
                },
            );
        }
    }
    state.last_callback_ms = Some(block.at_ms);
    state.window.add(block);
    state.total.add(block);
}

fn flush_window(session_id: &str, state: &mut WorkerState) {
    if state.window.samples == 0 {
        return;
    }
    let (rms, ratio, is_near_silent) = state.window.summary();
    state.silence_windows = if is_near_silent {
        state.silence_windows.saturating_add(1)
    } else {
        0
    };
    if state.silence_windows >= 2 && !state.is_silent {
        state.is_silent = true;
        state.identity.emit(
            session_id,
            "native_near_silence_started",
            log::Level::Warn,
            MicrophoneFields {
                reason: Some("observed_near_silence"),
                ..Default::default()
            },
        );
    }
    if !is_near_silent && state.is_silent {
        state.is_silent = false;
        state.identity.emit(
            session_id,
            "native_near_silence_ended",
            log::Level::Info,
            MicrophoneFields {
                reason: Some("observed_near_silence"),
                ..Default::default()
            },
        );
    }
    state.identity.emit(
        session_id,
        "native_audio_window",
        log::Level::Info,
        MicrophoneFields {
            sample_count: Some(state.window.samples),
            finite_sample_count: Some(state.window.finite),
            non_finite_sample_count: Some(state.window.non_finite),
            rms: Some(rms),
            peak: Some(state.window.peak),
            near_zero_ratio: Some(ratio),
            rms_threshold: Some(RMS_SILENCE_THRESHOLD),
            peak_threshold: Some(PEAK_SILENCE_THRESHOLD),
            near_zero_ratio_threshold: Some(ZERO_RATIO_THRESHOLD),
            window_ms: Some(WINDOW_MS),
            ..Default::default()
        },
    );
    state.completed_window_count += 1;
    state.window = WindowMetrics::default();
}

fn check_stall(session_id: &str, state: &mut WorkerState, now_ms: u64) {
    let Some(last) = state.last_callback_ms else {
        return;
    };
    if state.stall_started_ms.is_some() {
        return;
    }
    let Some(threshold) = stall_threshold_ms(&state.recent_gaps) else {
        return;
    };
    if now_ms.saturating_sub(last) > threshold {
        state.stall_started_ms = Some(last);
        state.identity.emit(
            session_id,
            "native_callback_stall_started",
            log::Level::Warn,
            MicrophoneFields {
                callback_gap_ms: Some(now_ms.saturating_sub(last)),
                ..Default::default()
            },
        );
    }
}

fn emit_final(
    session_id: &str,
    state: &WorkerState,
    control: &WorkerControl,
    metrics: FinalCaptureMetrics,
) {
    let (rms, ratio, _) = state.total.summary();
    let ended_at_ms = control.origin.elapsed().as_millis() as u64;
    let (longest_callback_gap_ms, last_callback_age_ms) =
        terminal_callback_gaps(state, ended_at_ms);
    state.identity.emit(
        session_id,
        "native_capture_finalized",
        log::Level::Info,
        MicrophoneFields {
            duration_ms: Some(metrics.duration_ms),
            callback_count: Some(state.callback_count),
            longest_callback_gap_ms: Some(longest_callback_gap_ms),
            last_callback_age_ms,
            sample_count: Some(metrics.sample_count as u64),
            frame_count: Some(metrics.frame_count as u64),
            finite_sample_count: Some(state.total.finite),
            non_finite_sample_count: Some(state.total.non_finite),
            rms: Some(rms),
            peak: Some(state.total.peak),
            near_zero_ratio: Some(ratio),
            audio_bytes: Some(metrics.audio_bytes),
            telemetry_dropped_count: Some(control.dropped.load(Ordering::Relaxed)),
            ..Default::default()
        },
    );
}

fn terminal_callback_gaps(state: &WorkerState, ended_at_ms: u64) -> (u64, Option<u64>) {
    let age = state
        .last_callback_ms
        .map(|last_callback| ended_at_ms.saturating_sub(last_callback));
    (state.longest_gap_ms.max(age.unwrap_or_default()), age)
}

#[cfg(test)]
#[path = "microphone_diagnostics_tests.rs"]
mod tests;
