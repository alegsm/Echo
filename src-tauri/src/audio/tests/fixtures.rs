use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::audio::backend::{CaptureBackend, CaptureBuffer, OpenedCapture};
use crate::audio::*;

pub(super) enum PlayBehavior {
    Samples(Vec<f32>),
    NoCallback,
    /// Callback deliveries in order, run on the actor thread during play.
    Script(Vec<FakeCallback>),
}

/// One delivery a CPAL stream makes: a PCM block or an error-callback notification.
pub(super) enum FakeCallback {
    Pcm(Vec<f32>),
    Notify(cpal::ErrorKind),
}

pub(crate) struct FakeStream {
    buffer: CaptureBuffer,
    behavior: PlayBehavior,
    drops: Arc<AtomicUsize>,
}

impl Drop for FakeStream {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) struct FakeBackend {
    behaviors: VecDeque<PlayBehavior>,
    drops: Arc<AtomicUsize>,
    open_started: Option<Sender<()>>,
    open_release: Option<Receiver<()>>,
    open_error: Option<CaptureErrorCode>,
    input: Option<Arc<Mutex<Option<CaptureBuffer>>>>,
    format: (u32, u16),
}

impl FakeBackend {
    pub(super) fn with_format(mut self, rate: u32, channels: u16) -> Self {
        self.format = (rate, channels);
        self
    }

    /// Counts dropped streams for every capture this backend opens.
    pub(super) fn drops(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.drops)
    }

    pub(super) fn immediate(behavior: PlayBehavior) -> (Self, Arc<AtomicUsize>) {
        let drops = Arc::new(AtomicUsize::new(0));
        (
            Self {
                behaviors: VecDeque::from([behavior]),
                drops: Arc::clone(&drops),
                open_started: None,
                open_release: None,
                open_error: None,
                input: None,
                format: (48_000, 1),
            },
            drops,
        )
    }

    pub(super) fn blocked() -> (Self, Receiver<()>, Sender<()>, Arc<AtomicUsize>) {
        let (started_sender, started_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let drops = Arc::new(AtomicUsize::new(0));
        (
            Self {
                behaviors: VecDeque::from([PlayBehavior::Samples(vec![0.1; 32])]),
                drops: Arc::clone(&drops),
                open_started: Some(started_sender),
                open_release: Some(release_receiver),
                open_error: None,
                input: None,
                format: (48_000, 1),
            },
            started_receiver,
            release_sender,
            drops,
        )
    }

    pub(super) fn sequence(behaviors: impl IntoIterator<Item = PlayBehavior>) -> Self {
        Self {
            behaviors: behaviors.into_iter().collect(),
            drops: Arc::new(AtomicUsize::new(0)),
            open_started: None,
            open_release: None,
            open_error: None,
            input: None,
            format: (48_000, 1),
        }
    }

    pub(super) fn failing(code: CaptureErrorCode) -> Self {
        Self {
            behaviors: VecDeque::new(),
            drops: Arc::new(AtomicUsize::new(0)),
            open_started: None,
            open_release: None,
            open_error: Some(code),
            input: None,
            format: (48_000, 1),
        }
    }
}

impl CaptureBackend for FakeBackend {
    type Stream = FakeStream;

    fn open(&mut self, buffer: CaptureBuffer) -> Result<OpenedCapture<Self::Stream>, CaptureError> {
        if let Some(sender) = self.open_started.take() {
            let _ = sender.send(());
        }
        if let Some(receiver) = self.open_release.take() {
            let _ = receiver.recv();
        }
        if let Some(code) = self.open_error.take() {
            return Err(CaptureError::new(code, "fake_open"));
        }
        if let Some(input) = &self.input {
            *input.lock().unwrap() = Some(buffer.clone());
        }
        Ok(OpenedCapture {
            stream: FakeStream {
                buffer,
                behavior: match self.behaviors.pop_front() {
                    Some(behavior) => behavior,
                    None if self.input.is_some() => PlayBehavior::Samples(Vec::new()),
                    None => panic!("fake behavior available"),
                },
                drops: Arc::clone(&self.drops),
            },
            sample_rate: self.format.0,
            channel_count: self.format.1,
            sample_format: "f32",
        })
    }

    fn play(&mut self, stream: &Self::Stream) -> Result<(), CaptureError> {
        match &stream.behavior {
            PlayBehavior::Samples(samples) => stream.buffer.append(samples),
            PlayBehavior::NoCallback => {}
            PlayBehavior::Script(callbacks) => {
                for callback in callbacks {
                    match callback {
                        FakeCallback::Pcm(samples) => stream.buffer.append(samples),
                        FakeCallback::Notify(kind) => stream
                            .buffer
                            .report_stream_notification(cpal::Error::new(*kind)),
                    }
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn owner(label: &str) -> TrustedCaptureOwner {
    TrustedCaptureOwner::from_window_label(label)
}

pub(super) fn intent(id: &str, purpose: CapturePurpose) -> CaptureIntent {
    CaptureIntent {
        intent_id: id.to_owned(),
        purpose,
    }
}

pub(super) fn test_policy() -> AudioCapturePolicy {
    AudioCapturePolicy {
        minimum_capture_duration: Duration::ZERO,
        minimum_sample_count: 0,
        minimum_signal_rms: 0.0,
        minimum_signal_peak: 0.0,
        pcm_headroom: PcmHeadroom::Bytes(1024 * 1024),
        ..Default::default()
    }
}

pub(crate) fn spawn_coordinator(
    backend: FakeBackend,
    policy: AudioCapturePolicy,
) -> DesktopAudioCapture {
    DesktopAudioCapture::start_with_backend(backend, policy, Arc::new(|_| {})).unwrap()
}

pub(super) fn spawn_with_events(
    backend: FakeBackend,
    policy: AudioCapturePolicy,
) -> (DesktopAudioCapture, Arc<Mutex<Vec<CaptureEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    let coordinator = DesktopAudioCapture::start_with_backend(
        backend,
        policy,
        Arc::new(move |event| captured.lock().unwrap().push(event)),
    )
    .unwrap();
    (coordinator, events)
}

pub(crate) fn begin(
    coordinator: &DesktopAudioCapture,
    owner: TrustedCaptureOwner,
    id: &str,
) -> CaptureSessionMetadata {
    let reservation = coordinator
        .reserve(owner, intent(id, CapturePurpose::PushToTalk))
        .unwrap();
    coordinator.begin(reservation).unwrap()
}

pub(super) fn event_with_outcome(
    events: &Arc<Mutex<Vec<CaptureEvent>>>,
    outcome: &'static str,
) -> CaptureEvent {
    events
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|event| event.outcome == outcome)
        .cloned()
        .unwrap()
}

pub(crate) struct FakeInput(Arc<Mutex<Option<CaptureBuffer>>>);

impl FakeBackend {
    pub(crate) fn continuous() -> (Self, FakeInput) {
        Self::continuous_with(PlayBehavior::Samples(Vec::new()))
    }

    /// Like `continuous`, with `behavior` for the first stream's play.
    pub(super) fn continuous_with(behavior: PlayBehavior) -> (Self, FakeInput) {
        let (mut backend, _) = Self::immediate(behavior);
        let input = FakeInput(Arc::new(Mutex::new(None)));
        backend.input = Some(Arc::clone(&input.0));
        (backend, input)
    }
}

impl FakeInput {
    /// Keeps the current stream's callback after later captures reopen the backend.
    pub(super) fn detach(&self) -> FakeInput {
        FakeInput(Arc::new(Mutex::new(self.0.lock().unwrap().clone())))
    }

    /// Delivers CPAL error-callback notifications back to back, then waits until the actor
    /// has handled them.
    pub(super) fn notify(
        &self,
        coordinator: &DesktopAudioCapture,
        kinds: impl IntoIterator<Item = cpal::ErrorKind>,
    ) {
        self.report(kinds);
        // The actor drains backend events after each command, so the second reply
        // follows the drain that saw these notifications.
        for _ in 0..2 {
            coordinator.snapshot().unwrap();
        }
    }

    /// An event sink that answers every underrun warning with another `Xrun` from this
    /// input, and the count of those refills. Each slot the actor frees in the bounded event
    /// queue is refilled before its next read, so the queue never empties while the stream
    /// is current, as under sustained CoreAudio overload.
    pub(super) fn refilling_sink(&self) -> (CaptureEventSink, Arc<AtomicUsize>) {
        let input = Arc::clone(&self.0);
        let refills = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&refills);
        let sink = Arc::new(move |event: CaptureEvent| {
            if event.error == Some(CaptureErrorCode::BufferUnderrun) {
                if let Some(buffer) = input.lock().unwrap().as_ref() {
                    buffer.report_stream_notification(cpal::Error::new(cpal::ErrorKind::Xrun));
                    counted.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
        (sink, refills)
    }

    /// Delivers CPAL error-callback notifications back to back without waiting for the actor.
    pub(super) fn report(&self, kinds: impl IntoIterator<Item = cpal::ErrorKind>) {
        let buffer = self.0.lock().unwrap();
        for kind in kinds {
            buffer
                .as_ref()
                .unwrap()
                .report_stream_notification(cpal::Error::new(kind));
        }
    }

    pub(super) fn feed_samples(&self, coordinator: &DesktopAudioCapture, samples: &[f32]) {
        self.0.lock().unwrap().as_ref().unwrap().append(samples);
        coordinator
            .request(crate::audio::actor::ActorCommand::DrainSegments)
            .unwrap();
    }

    // Each block advances captured audio by 20 ms, then synchronizes with the actor.
    pub(crate) fn feed(
        &self,
        coordinator: &DesktopAudioCapture,
        value: f32,
        ms: usize,
    ) -> Vec<AudioSegment> {
        assert_eq!(ms % 20, 0);
        let mut segments = Vec::new();
        for _ in 0..ms / 20 {
            self.0
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .append(&[value; 960]);
            segments = coordinator
                .request(crate::audio::actor::ActorCommand::DrainSegments)
                .unwrap();
        }
        assert_eq!(
            self.0.lock().unwrap().as_ref().unwrap().stats().2,
            0,
            "continuous capture must not drop callbacks"
        );
        segments
    }
}

struct DeviceConfigLog(Mutex<Vec<serde_json::Value>>);
static DEVICE_CONFIG_LOG: DeviceConfigLog = DeviceConfigLog(Mutex::new(Vec::new()));
static LOGGER_INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

impl log::Log for DeviceConfigLog {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        let message = record.args().to_string();
        let Some(json) = message.strip_prefix("[echo-diagnostic] ") else {
            return;
        };
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        if value["event"] == "native_input_config_result" {
            self.0.lock().unwrap().push(value);
        }
    }
    fn flush(&self) {}
}

pub(super) fn install_device_config_logger() {
    LOGGER_INSTALLED.get_or_init(|| {
        log::set_logger(&DEVICE_CONFIG_LOG).unwrap();
        log::set_max_level(log::LevelFilter::Info);
    });
}

pub(super) fn device_config(session_id: &str) -> serde_json::Value {
    DEVICE_CONFIG_LOG
        .0
        .lock()
        .unwrap()
        .iter()
        .find(|record| record["sessionId"] == session_id)
        .expect("capture must report its device configuration")
        .clone()
}
