use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SizedSample};

use super::gate::CaptureGate;
use super::types::{CaptureError, CaptureErrorCode};

const PCM_BLOCK_SAMPLES: usize = 4_096;

pub(crate) enum BackendEvent {
    FirstCallback(u64),
    PcmLimitReached(u64),
    StreamError(u64, CaptureErrorCode),
    DefaultInputChanged(u64, &'static str),
}

#[derive(Clone)]
pub(crate) struct CaptureBuffer {
    inner: Arc<Mutex<PcmBlocks>>,
    sample_count: Arc<AtomicUsize>,
    callback_count: Arc<AtomicU64>,
    dropped_callback_count: Arc<AtomicU64>,
    has_reported_first_callback: Arc<AtomicBool>,
    has_reported_limit: Arc<AtomicBool>,
    terminal_error: Arc<OnceLock<CaptureErrorCode>>,
    generation: u64,
    gate: Arc<CaptureGate>,
    events: SyncSender<BackendEvent>,
    diagnostics: Option<crate::microphone_diagnostics::CallbackDiagnostics>,
}

#[derive(Default)]
struct PcmBlocks {
    maximum_samples: usize,
    capture_limit: usize,
    outstanding_samples: usize,
    queued: VecDeque<Vec<f32>>,
    free: Vec<Vec<f32>>,
    spare: VecDeque<Vec<f32>>,
}

pub(crate) struct CaptureBufferContext {
    pub(crate) generation: u64,
    pub(crate) gate: Arc<CaptureGate>,
    pub(crate) events: SyncSender<BackendEvent>,
    pub(crate) diagnostics: Option<crate::microphone_diagnostics::CallbackDiagnostics>,
}

impl PcmBlocks {
    fn new(maximum_samples: usize, capture_limit: usize) -> Result<Self, CaptureError> {
        // One extra block covers a partial block held by the actor during a drain.
        let block_count = maximum_samples.div_ceil(PCM_BLOCK_SAMPLES) + 1;
        let reservation_error =
            |_| CaptureError::new(CaptureErrorCode::PcmMemoryLimit, "pcm_reservation");
        let mut queued = VecDeque::new();
        queued.try_reserve(block_count).map_err(reservation_error)?;
        let mut spare = VecDeque::new();
        spare.try_reserve(block_count).map_err(reservation_error)?;
        let mut free = Vec::new();
        free.try_reserve(block_count).map_err(reservation_error)?;
        for _ in 0..block_count {
            let mut block = Vec::new();
            block
                .try_reserve_exact(PCM_BLOCK_SAMPLES)
                .map_err(reservation_error)?;
            free.push(block);
        }
        Ok(Self {
            maximum_samples,
            capture_limit,
            outstanding_samples: 0,
            queued,
            free,
            spare,
        })
    }
}

impl CaptureBuffer {
    pub(crate) fn new(context: CaptureBufferContext) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PcmBlocks::default())),
            sample_count: Arc::new(AtomicUsize::new(0)),
            callback_count: Arc::new(AtomicU64::new(0)),
            dropped_callback_count: Arc::new(AtomicU64::new(0)),
            has_reported_first_callback: Arc::new(AtomicBool::new(false)),
            has_reported_limit: Arc::new(AtomicBool::new(false)),
            terminal_error: Arc::new(OnceLock::new()),
            generation: context.generation,
            gate: context.gate,
            events: context.events,
            diagnostics: context.diagnostics,
        }
    }

    /// Reserve the complete callback pool after format selection and before stream play.
    pub(crate) fn configure(
        &self,
        sample_rate: u32,
        channels: u16,
        policy: &super::types::AudioCapturePolicy,
    ) -> Result<(), CaptureError> {
        let samples_per_second = sample_rate as usize * usize::from(channels);
        let maximum_samples = match policy.pcm_headroom {
            super::types::PcmHeadroom::StallBudget(budget) => (samples_per_second as f64
                * budget.min(policy.maximum_capture_duration).as_secs_f64())
            .ceil() as usize,
            #[cfg(test)]
            super::types::PcmHeadroom::Bytes(bytes) => bytes / std::mem::size_of::<f32>(),
        };
        let capture_limit = (samples_per_second as f64
            * policy.maximum_capture_duration.as_secs_f64())
        .ceil() as usize;
        let reserved = PcmBlocks::new(maximum_samples, capture_limit)?;
        let mut blocks = self.inner.lock().map_err(|_| {
            CaptureError::new(CaptureErrorCode::CoordinatorUnavailable, "pcm_configure")
        })?;
        *blocks = reserved;
        Ok(())
    }

    pub(crate) fn append<T>(&self, samples: &[T])
    where
        T: Sample + Copy,
        f32: FromSample<T>,
    {
        if !self.gate.is_current(self.generation) {
            return;
        }
        self.callback_count.fetch_add(1, Ordering::Relaxed);
        let Ok(mut buffer) = self.inner.lock() else {
            self.dropped_callback_count.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let accepted = self.sample_count.load(Ordering::Relaxed);
        let wanted = samples
            .len()
            .min(buffer.capture_limit.saturating_sub(accepted));
        let available = buffer
            .maximum_samples
            .saturating_sub(buffer.outstanding_samples);
        let count = wanted.min(available);
        let mut remaining = &samples[..count];
        while !remaining.is_empty() {
            let mut block = match buffer.queued.back() {
                Some(block) if block.len() < PCM_BLOCK_SAMPLES => buffer.queued.pop_back().unwrap(),
                _ => match buffer.free.pop() {
                    Some(block) => block,
                    None => {
                        let appended = count - remaining.len();
                        buffer.outstanding_samples += appended;
                        self.sample_count.fetch_add(appended, Ordering::Relaxed);
                        self.dropped_callback_count.fetch_add(1, Ordering::Relaxed);
                        drop(buffer);
                        self.report_pcm_limit();
                        return;
                    }
                },
            };
            let count = remaining.len().min(PCM_BLOCK_SAMPLES - block.len());
            let start = block.len();
            block.extend(remaining[..count].iter().map(|sample| {
                let sample = f32::from_sample(*sample);
                if sample.is_finite() {
                    sample
                } else {
                    0.0
                }
            }));
            if let Some(diagnostics) = self.diagnostics.as_ref() {
                diagnostics.observe(&block[start..]);
            }
            buffer.queued.push_back(block);
            remaining = &remaining[count..];
        }
        buffer.outstanding_samples += count;
        self.sample_count.fetch_add(count, Ordering::Relaxed);
        drop(buffer);
        // A start response may only follow data becoming observable to stop().
        self.report_first_callback();
        if wanted > available {
            self.dropped_callback_count.fetch_add(1, Ordering::Relaxed);
            self.report_pcm_limit();
        }
    }

    /// Forwards one CPAL error-callback notification for this generation to the actor.
    pub(crate) fn report_stream_notification(&self, error: cpal::Error) {
        if !self.gate.is_current(self.generation) {
            return;
        }
        let code = match stream_notification(&error) {
            StreamNotification::Fatal(code) => {
                // The first terminal error ends the session; later ones change nothing.
                if self.terminal_error.set(code).is_err() {
                    return;
                }
                code
            }
            StreamNotification::Underrun => CaptureErrorCode::BufferUnderrun,
            StreamNotification::RouteChanged => {
                let _ = self.events.try_send(BackendEvent::DefaultInputChanged(
                    self.generation,
                    "audio_route_changed",
                ));
                return;
            }
            StreamNotification::Ignored => return,
        };
        if let Some(diagnostics) = self.diagnostics.as_ref() {
            diagnostics.stream_error(stream_error_name(code));
        }
        let _ = self
            .events
            .try_send(BackendEvent::StreamError(self.generation, code));
    }

    /// The terminal error, or else the readiness, this capture has latched. The event queue
    /// is bounded and drops on full, so a warning burst can crowd out either event; the
    /// actor reads this after draining the queue so neither is lost.
    pub(crate) fn latched_event(&self) -> Option<BackendEvent> {
        if let Some(code) = self.terminal_error.get() {
            return Some(BackendEvent::StreamError(self.generation, *code));
        }
        self.has_reported_first_callback
            .load(Ordering::Acquire)
            .then_some(BackendEvent::FirstCallback(self.generation))
    }

    pub(crate) fn take(&self) -> Result<DrainedPcmBlocks, CaptureError> {
        let mut blocks = self
            .inner
            .lock()
            .map_err(|_| CaptureError::new(CaptureErrorCode::CoordinatorUnavailable, "pcm"))?;
        let mut drained = std::mem::take(&mut blocks.spare);
        std::mem::swap(&mut blocks.queued, &mut drained);
        Ok(DrainedPcmBlocks {
            queued: drained,
            buffer: self.clone(),
        })
    }

    pub(crate) fn recycle(&self, mut block: Vec<f32>) -> Result<(), CaptureError> {
        let mut blocks = self
            .inner
            .lock()
            .map_err(|_| CaptureError::new(CaptureErrorCode::CoordinatorUnavailable, "pcm"))?;
        blocks.outstanding_samples -= block.len();
        block.clear();
        blocks.free.push(block);
        Ok(())
    }

    pub(crate) fn stats(&self) -> (u64, usize, u64) {
        let sample_count = self.sample_count.load(Ordering::Relaxed);
        (
            self.callback_count.load(Ordering::Relaxed),
            sample_count,
            self.dropped_callback_count.load(Ordering::Relaxed),
        )
    }

    fn report_first_callback(&self) {
        if !self
            .has_reported_first_callback
            .swap(true, Ordering::AcqRel)
        {
            let _ = self
                .events
                .try_send(BackendEvent::FirstCallback(self.generation));
        }
    }

    fn report_pcm_limit(&self) {
        if !self.has_reported_limit.swap(true, Ordering::AcqRel) {
            let _ = self
                .events
                .try_send(BackendEvent::PcmLimitReached(self.generation));
        }
    }
}

/// Returns the emptied queue even when segmentation exits early with an error.
pub(crate) struct DrainedPcmBlocks {
    queued: VecDeque<Vec<f32>>,
    buffer: CaptureBuffer,
}

impl Iterator for DrainedPcmBlocks {
    type Item = Vec<f32>;

    fn next(&mut self) -> Option<Self::Item> {
        self.queued.pop_front()
    }
}

impl std::ops::Deref for DrainedPcmBlocks {
    type Target = VecDeque<Vec<f32>>;

    fn deref(&self) -> &Self::Target {
        &self.queued
    }
}

impl Drop for DrainedPcmBlocks {
    fn drop(&mut self) {
        if let Ok(mut blocks) = self.buffer.inner.lock() {
            for mut block in self.queued.drain(..) {
                blocks.outstanding_samples -= block.len();
                block.clear();
                blocks.free.push(block);
            }
            blocks.spare = std::mem::take(&mut self.queued);
        }
    }
}

pub(crate) struct OpenedCapture<Stream> {
    pub(crate) stream: Stream,
    pub(crate) sample_rate: u32,
    pub(crate) channel_count: u16,
    pub(crate) sample_format: &'static str,
}

pub(crate) trait CaptureBackend: Send + 'static {
    type Stream;

    fn open(&mut self, buffer: CaptureBuffer) -> Result<OpenedCapture<Self::Stream>, CaptureError>;
    fn play(&mut self, stream: &Self::Stream) -> Result<(), CaptureError>;
}

pub(crate) struct CpalCaptureBackend;

impl CaptureBackend for CpalCaptureBackend {
    type Stream = cpal::Stream;

    fn open(&mut self, buffer: CaptureBuffer) -> Result<OpenedCapture<Self::Stream>, CaptureError> {
        let device = cpal::default_host().default_input_device().ok_or_else(|| {
            CaptureError::new(CaptureErrorCode::MicrophoneUnavailable, "default_device")
        })?;
        let config = device.default_input_config().map_err(|error| {
            CaptureError::new(
                capture_error_code(&error, CaptureErrorCode::DeviceConfigFailed),
                "default_config",
            )
        })?;
        let metadata = (
            config.sample_rate(),
            config.channels(),
            format_name(config.sample_format()),
        );
        let stream_config = config.config();
        let stream = match config.sample_format() {
            cpal::SampleFormat::I8 => build_stream::<i8>(&device, stream_config, buffer),
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, stream_config, buffer),
            cpal::SampleFormat::I24 => build_stream::<cpal::I24>(&device, stream_config, buffer),
            cpal::SampleFormat::I32 => build_stream::<i32>(&device, stream_config, buffer),
            cpal::SampleFormat::I64 => build_stream::<i64>(&device, stream_config, buffer),
            cpal::SampleFormat::U8 => build_stream::<u8>(&device, stream_config, buffer),
            cpal::SampleFormat::U16 => build_stream::<u16>(&device, stream_config, buffer),
            cpal::SampleFormat::U24 => build_stream::<cpal::U24>(&device, stream_config, buffer),
            cpal::SampleFormat::U32 => build_stream::<u32>(&device, stream_config, buffer),
            cpal::SampleFormat::U64 => build_stream::<u64>(&device, stream_config, buffer),
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, stream_config, buffer),
            cpal::SampleFormat::F64 => build_stream::<f64>(&device, stream_config, buffer),
            _ => Err(CaptureError::new(
                CaptureErrorCode::StreamBuildFailed,
                "unsupported_sample_format",
            )),
        }?;
        Ok(OpenedCapture {
            stream,
            sample_rate: metadata.0,
            channel_count: metadata.1,
            sample_format: metadata.2,
        })
    }

    fn play(&mut self, stream: &Self::Stream) -> Result<(), CaptureError> {
        stream.play().map_err(|error| {
            CaptureError::new(
                capture_error_code(&error, CaptureErrorCode::StreamPlayFailed),
                "stream_play",
            )
        })
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    buffer: CaptureBuffer,
) -> Result<cpal::Stream, CaptureError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let error_buffer = buffer.clone();
    device
        .build_input_stream(
            config,
            move |input: &[T], _| {
                buffer.append(input);
            },
            move |error| error_buffer.report_stream_notification(error),
            Some(Duration::from_secs(3)),
        )
        .map_err(|error| {
            CaptureError::new(
                capture_error_code(&error, CaptureErrorCode::StreamBuildFailed),
                "stream_build",
            )
        })
}

/// What one CPAL error-callback notification means for the capture that received it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamNotification {
    Fatal(CaptureErrorCode),
    Underrun,
    RouteChanged,
    Ignored,
}

fn stream_notification(error: &cpal::Error) -> StreamNotification {
    match error.kind() {
        cpal::ErrorKind::Xrun => StreamNotification::Underrun,
        // The stream keeps running: CoreAudio reports default-output route changes and
        // WASAPI 0.18.2 reports default-input changes as StreamInvalidated instead.
        cpal::ErrorKind::DeviceChanged => StreamNotification::RouteChanged,
        // Audio continues at normal thread priority.
        cpal::ErrorKind::RealtimeDenied => StreamNotification::Ignored,
        _ => StreamNotification::Fatal(capture_error_code(
            error,
            CaptureErrorCode::StreamBackendError,
        )),
    }
}

/// Classifies a failed config, build, play or callback; `stage` is the code for kinds
/// without an owner-visible meaning of their own.
fn capture_error_code(error: &cpal::Error, stage: CaptureErrorCode) -> CaptureErrorCode {
    classify_capture_error(error, stage, cfg!(target_os = "windows"))
}

fn classify_capture_error(
    error: &cpal::Error,
    stage: CaptureErrorCode,
    is_windows: bool,
) -> CaptureErrorCode {
    match error.kind() {
        cpal::ErrorKind::PermissionDenied => CaptureErrorCode::MicrophonePermissionDenied,
        cpal::ErrorKind::DeviceNotAvailable => CaptureErrorCode::DeviceUnavailable,
        cpal::ErrorKind::StreamInvalidated => CaptureErrorCode::StreamInvalidated,
        // CPAL 0.18.2 WASAPI maps no HRESULT to PermissionDenied; E_ACCESSDENIED arrives as
        // BackendError whose message is the std::io::Error text with its OS code.
        cpal::ErrorKind::BackendError
            if is_windows && is_windows_access_denied(&error.to_string()) =>
        {
            CaptureErrorCode::MicrophonePermissionDenied
        }
        _ => stage,
    }
}

fn is_windows_access_denied(description: &str) -> bool {
    let description = description.to_ascii_lowercase();
    let denied_code = description
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .any(|token| {
            matches!(
                token,
                "e_accessdenied" | "0x80070005" | "2147024891" | "2147942405"
            )
        });
    let denied_message = [
        "access is denied",
        "access denied",
        "permission denied",
        "blocked by privacy settings",
        "disabled by privacy settings",
    ]
    .iter()
    .any(|message| description.contains(message));
    denied_code || denied_message
}

fn stream_error_name(code: CaptureErrorCode) -> &'static str {
    match code {
        CaptureErrorCode::DeviceUnavailable => "device_not_available",
        CaptureErrorCode::StreamInvalidated => "stream_invalidated",
        CaptureErrorCode::BufferUnderrun => "buffer_underrun",
        _ => "backend_stream_error",
    }
}

fn format_name(format: cpal::SampleFormat) -> &'static str {
    match format {
        cpal::SampleFormat::I8 => "i8",
        cpal::SampleFormat::I16 => "i16",
        cpal::SampleFormat::I24 => "i24",
        cpal::SampleFormat::I32 => "i32",
        cpal::SampleFormat::I64 => "i64",
        cpal::SampleFormat::U8 => "u8",
        cpal::SampleFormat::U16 => "u16",
        cpal::SampleFormat::U24 => "u24",
        cpal::SampleFormat::U32 => "u32",
        cpal::SampleFormat::U64 => "u64",
        cpal::SampleFormat::F32 => "f32",
        cpal::SampleFormat::F64 => "f64",
        _ => "unsupported",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{CaptureIntent, CapturePurpose, TrustedCaptureOwner};

    use cpal::{Error, ErrorKind};

    // Keep: WASAPI leaves E_ACCESSDENIED as a BackendError whose text carries the HRESULT
    // (`std::io::Error` formatting), so only that kind is read for denial text on Windows.
    #[test]
    fn microphone_windows_backend_denial_keeps_permission_identity() {
        for description in [
            "E_ACCESSDENIED",
            "WASAPI: HRESULT 0x80070005",
            "Access is denied. (os error -2147024891)",
            "Localized error (os error 2147942405)",
            "Access denied",
            "Permission denied",
            "Microphone blocked by privacy settings",
            "Device disabled by privacy settings",
        ] {
            let error = Error::with_message(ErrorKind::BackendError, description);
            assert_eq!(
                classify_capture_error(&error, CaptureErrorCode::StreamBuildFailed, true),
                CaptureErrorCode::MicrophonePermissionDenied,
                "{description}"
            );
            assert_eq!(
                classify_capture_error(&error, CaptureErrorCode::StreamBuildFailed, false),
                CaptureErrorCode::StreamBuildFailed,
                "{description} outside Windows"
            );
        }
        for description in [
            "Device unavailable",
            "HRESULT 0x88890004",
            "Check privacy settings",
            "0x800700050",
        ] {
            let error = Error::with_message(ErrorKind::BackendError, description);
            assert_eq!(
                classify_capture_error(&error, CaptureErrorCode::StreamBuildFailed, true),
                CaptureErrorCode::StreamBuildFailed,
                "{description}"
            );
        }
    }

    // Keep: one typed classification serves config, build, play and callback (AC3-AC5, AC18).
    #[test]
    fn typed_cpal_errors_classify_identically_at_every_stage() {
        let stages = [
            CaptureErrorCode::DeviceConfigFailed,
            CaptureErrorCode::StreamBuildFailed,
            CaptureErrorCode::StreamPlayFailed,
            CaptureErrorCode::StreamBackendError,
        ];
        let denial = "Access is denied. (os error -2147024891)";
        for stage in stages {
            for is_windows in [false, true] {
                let typed = [
                    (
                        Error::new(ErrorKind::PermissionDenied),
                        CaptureErrorCode::MicrophonePermissionDenied,
                    ),
                    (
                        Error::new(ErrorKind::DeviceNotAvailable),
                        CaptureErrorCode::DeviceUnavailable,
                    ),
                    (
                        Error::new(ErrorKind::StreamInvalidated),
                        CaptureErrorCode::StreamInvalidated,
                    ),
                    // Typed identity wins over misleading denial text.
                    (
                        Error::with_message(ErrorKind::DeviceNotAvailable, denial),
                        CaptureErrorCode::DeviceUnavailable,
                    ),
                    (
                        Error::with_message(ErrorKind::StreamInvalidated, "E_ACCESSDENIED"),
                        CaptureErrorCode::StreamInvalidated,
                    ),
                    (
                        Error::with_message(ErrorKind::UnsupportedConfig, denial),
                        stage,
                    ),
                    (
                        Error::with_message(ErrorKind::DeviceBusy, "permission denied"),
                        stage,
                    ),
                    // Synchronous operations stay failures at their stage, whatever the kind.
                    (Error::new(ErrorKind::Xrun), stage),
                    (Error::new(ErrorKind::DeviceChanged), stage),
                    (Error::new(ErrorKind::RealtimeDenied), stage),
                    (Error::new(ErrorKind::HostUnavailable), stage),
                    (Error::new(ErrorKind::BackendError), stage),
                    (Error::new(ErrorKind::Other), stage),
                ];
                for (error, expected) in typed {
                    assert_eq!(
                        classify_capture_error(&error, stage, is_windows),
                        expected,
                        "{error:?} at {stage:?}, windows {is_windows}"
                    );
                }
            }
        }
    }

    // Keep: the callback disposition per CPAL kind; only recoverable kinds keep the stream.
    #[test]
    fn stream_notification_kinds_map_to_one_disposition() {
        use StreamNotification::*;
        for (kind, expected) in [
            (ErrorKind::Xrun, Underrun),
            (ErrorKind::DeviceChanged, RouteChanged),
            (ErrorKind::RealtimeDenied, Ignored),
            (
                ErrorKind::DeviceNotAvailable,
                Fatal(CaptureErrorCode::DeviceUnavailable),
            ),
            (
                ErrorKind::StreamInvalidated,
                Fatal(CaptureErrorCode::StreamInvalidated),
            ),
            (
                ErrorKind::PermissionDenied,
                Fatal(CaptureErrorCode::MicrophonePermissionDenied),
            ),
            (
                ErrorKind::BackendError,
                Fatal(CaptureErrorCode::StreamBackendError),
            ),
            (
                ErrorKind::DeviceBusy,
                Fatal(CaptureErrorCode::StreamBackendError),
            ),
            (
                ErrorKind::ResourceExhausted,
                Fatal(CaptureErrorCode::StreamBackendError),
            ),
            (
                ErrorKind::Other,
                Fatal(CaptureErrorCode::StreamBackendError),
            ),
        ] {
            assert_eq!(stream_notification(&Error::new(kind)), expected, "{kind:?}");
        }
    }

    // Keep: I24 and I32 callbacks keep polarity, full-scale range and interleaved channel order.
    #[test]
    fn integer_callback_samples_keep_polarity_scale_and_channel_order() {
        let expected = [-1.0, 0.5, -0.5, 0.25, 0.0, -0.25];
        let i24 = [-(1 << 23), 1 << 22, -(1 << 22), 1 << 21, 0, -(1 << 21)]
            .map(|value| cpal::I24::new(value).unwrap());
        let i32 = [i32::MIN, 1 << 30, -(1 << 30), 1 << 29, 0, -(1 << 29)];
        let (buffer, _) = test_buffer();
        buffer.append(&i24);
        buffer.append(&i32);
        buffer.append(&[cpal::I24::new((1 << 23) - 1).unwrap()]);
        buffer.append(&[i32::MAX]);
        let samples: Vec<f32> = buffer.take().unwrap().flatten().collect();
        assert_eq!(&samples[..6], &expected, "i24");
        assert_eq!(&samples[6..12], &expected, "i32");
        for (format, sample) in [("i24", samples[12]), ("i32", samples[13])] {
            assert!(
                (0.999_99..=1.0).contains(&sample),
                "{format} full scale: {sample}"
            );
        }
    }

    #[test]
    fn drained_queue_is_returned_without_allocating_on_the_next_tick() {
        let (buffer, _) = test_buffer();
        buffer.append(&[0.25_f32; 960]);
        let first = buffer.take().unwrap();
        let allocation = first.as_slices().0.as_ptr();
        let capacity = first.capacity();
        drop(first);
        // Two queues alternate between the callback and the actor.
        drop(buffer.take().unwrap());
        buffer.append(&[0.5_f32; 960]);
        let third = buffer.take().unwrap();
        assert_eq!(third.capacity(), capacity);
        assert_eq!(third.as_slices().0.as_ptr(), allocation);
        assert_eq!(third.iter().flatten().count(), 960);
    }

    #[test]
    fn exhausted_pcm_pool_reports_limit_without_panicking() {
        let (buffer, events) = test_buffer();
        buffer.inner.lock().unwrap().free.clear();
        buffer.append(&[0.25_f32; 960]);
        assert_eq!(buffer.stats(), (1, 0, 1));
        assert!(matches!(
            events.try_recv().unwrap(),
            BackendEvent::PcmLimitReached(_)
        ));
    }

    // Keep: recycling releases capacity while telemetry remains cumulative.
    #[test]
    fn pcm_capacity_counts_outstanding_blocks_including_actor_and_early_cleanup() {
        let (buffer, events) = test_buffer();
        for _ in 0..10 {
            buffer.append(&[0.25_f32; 960]);
            for block in buffer.take().unwrap() {
                buffer.recycle(block).unwrap();
            }
        }
        assert_eq!(buffer.stats().1, 9_600);
        assert!(!events
            .try_iter()
            .any(|event| matches!(event, BackendEvent::PcmLimitReached(_))));
        buffer.append(&[0.25_f32; 960]);
        drop(buffer.take().unwrap());
        buffer.append(&[0.25_f32; 960]);
        let held = buffer.take().unwrap();
        buffer.append(&[0.25_f32; 960]);
        assert_eq!(buffer.stats().1, 12_480);
        buffer.append(&[0.25_f32; 960]);
        assert_eq!(buffer.stats().1, 9_600 + 960 + 2_048);
        assert_eq!(
            buffer.stats().2,
            1,
            "overflow is counted without discarding accepted samples"
        );
        assert!(events
            .try_iter()
            .any(|event| matches!(event, BackendEvent::PcmLimitReached(_))));
        drop(held);
        drop(buffer.take().unwrap());
        buffer.append(&[0.25_f32; 960]);
        assert_eq!(buffer.stats().1, 9_600 + 960 + 2_048 + 960);
    }

    // Keep: 599 seconds of each input format can recycle one reserved pool.
    #[test]
    fn device_scaled_pcm_pool_accepts_599_seconds_without_cumulative_exhaustion() {
        for (rate, channels) in [(16_000, 1), (96_000, 2), (96_000, 8)] {
            let (buffer, events) = test_buffer();
            buffer
                .configure(
                    rate,
                    channels,
                    &super::super::types::AudioCapturePolicy::default(),
                )
                .unwrap();
            let samples = vec![0.2_f32; rate as usize * usize::from(channels)];
            for _ in 0..599 {
                buffer.append(&samples);
                for block in buffer.take().unwrap() {
                    buffer.recycle(block).unwrap();
                }
            }
            assert_eq!(buffer.stats().1, samples.len() * 599, "{rate}/{channels}");
            buffer.append(&samples);
            for block in buffer.take().unwrap() {
                buffer.recycle(block).unwrap();
            }
            buffer.append(&samples);
            assert_eq!(
                buffer.stats().1,
                samples.len() * 600,
                "callbacks beyond the cap are bounded"
            );
            assert_eq!(buffer.stats().2, 0);
            assert!(!events
                .try_iter()
                .any(|event| matches!(event, BackendEvent::PcmLimitReached(_))));
        }
    }

    // Keep: device format is unknown at construction; pool allocation waits for configure.
    #[test]
    fn unconfigured_capture_buffer_has_no_pcm_allocation() {
        let (buffer, _) = unconfigured_buffer();
        let blocks = buffer.inner.lock().unwrap();
        assert_eq!(blocks.queued.capacity(), 0);
        assert_eq!(blocks.spare.capacity(), 0);
        assert_eq!(blocks.free.capacity(), 0);
    }

    // Keep: a single delayed callback must fit the independently sized stall pool.
    #[test]
    fn stall_headroom_accepts_three_seconds_at_device_formats() {
        for (rate, channels) in [(48_000, 1), (96_000, 2), (192_000, 8)] {
            let (buffer, events) = test_buffer();
            buffer
                .configure(
                    rate,
                    channels,
                    &super::super::types::AudioCapturePolicy::default(),
                )
                .unwrap();
            let samples = vec![0.2_f32; rate as usize * usize::from(channels) * 3];
            buffer.append(&samples);
            assert_eq!(buffer.stats(), (1, samples.len(), 0), "{rate}/{channels}");
            assert!(!events
                .try_iter()
                .any(|event| matches!(event, BackendEvent::PcmLimitReached(_))));
        }
    }

    fn test_buffer() -> (CaptureBuffer, std::sync::mpsc::Receiver<BackendEvent>) {
        let (buffer, events) = unconfigured_buffer();
        buffer
            .configure(
                48_000,
                1,
                &super::super::types::AudioCapturePolicy {
                    pcm_headroom: super::super::types::PcmHeadroom::Bytes(8192),
                    ..Default::default()
                },
            )
            .unwrap();
        (buffer, events)
    }

    fn unconfigured_buffer() -> (CaptureBuffer, std::sync::mpsc::Receiver<BackendEvent>) {
        let gate = Arc::new(CaptureGate::default());
        let reservation = gate
            .reserve(
                TrustedCaptureOwner::from_window_label("overlay"),
                CaptureIntent {
                    intent_id: "pool-test".into(),
                    purpose: CapturePurpose::PushToTalk,
                },
                None,
            )
            .unwrap();
        let (events, receiver) = std::sync::mpsc::sync_channel(32);
        (
            CaptureBuffer::new(CaptureBufferContext {
                generation: reservation.generation,
                gate,
                events,
                diagnostics: None,
            }),
            receiver,
        )
    }

    #[test]
    fn contended_callback_retains_all_samples() {
        let (buffer, _) = test_buffer();
        let held = buffer.inner.lock().unwrap();
        let callback = buffer.clone();
        let (started, ready) = std::sync::mpsc::channel();
        let (finished, done) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            callback.append(&[0.25_f32; 960]);
            finished.send(()).unwrap();
        });
        ready.recv().unwrap();
        let _ = done.recv_timeout(Duration::from_millis(50));
        drop(held);
        worker.join().unwrap();
        assert_eq!(buffer.stats(), (1, 960, 0));
        let blocks = buffer.take().unwrap();
        assert_eq!(
            blocks.iter().flatten().copied().collect::<Vec<_>>(),
            vec![0.25; 960]
        );
    }
}
