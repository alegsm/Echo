use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tauri::test::MockRuntime;
use tauri::Manager;

use super::*;
use crate::audio::tests::fixtures::{owner, spawn_coordinator, FakeBackend};
use crate::audio::{
    AudioCapturePolicy, CancelCapture, CaptureErrorCode, CaptureEvent, CaptureIntent, CapturePhase,
    CapturePurpose,
};
use crate::services::runtime::EchoErrorCode;

const ARTIFACT: &[u8] = include_bytes!("updater_fixtures/artifact.bin");
const ARTIFACT_SIGNATURE: &str = include_str!("updater_fixtures/artifact.bin.sig");
const TRUSTED_KEY: &str = include_str!("updater_fixtures/trusted.pub");
const OTHER_KEY: &str = include_str!("updater_fixtures/other.pub");

type Published = Arc<Mutex<Vec<UpdateEnvelope>>>;

fn service<P: Clone>() -> (UpdaterService<P>, Published) {
    let published: Published = Arc::default();
    let sink = Arc::clone(&published);
    let service = UpdaterService::new(move |envelope| sink.lock().unwrap().push(envelope.clone()));
    (service, published)
}

fn available(version: &str) -> UpdateStatus {
    UpdateStatus::Available {
        version: version.to_owned(),
    }
}

fn downloading(downloaded_bytes: u64, total_bytes: Option<u64>) -> UpdateStatus {
    UpdateStatus::Downloading {
        version: "0.5.1".to_owned(),
        downloaded_bytes,
        total_bytes,
    }
}

fn version_state(state: &str) -> UpdateStatus {
    let version = "0.5.1".to_owned();
    match state {
        "waiting_for_idle" => UpdateStatus::WaitingForIdle { version },
        "installing" => UpdateStatus::Installing { version },
        "failed" => UpdateStatus::Failed { version },
        _ => unreachable!("{state}"),
    }
}

#[derive(Default)]
struct FakeSource {
    checks: Mutex<VecDeque<CheckOutcome<&'static str>>>,
    check_calls: AtomicUsize,
    chunks: Vec<usize>,
    total: Option<u64>,
    download_errors: Mutex<VecDeque<String>>,
    download_calls: AtomicUsize,
    install_error: Option<String>,
    installs: Mutex<Vec<(&'static str, Vec<u8>)>>,
}

impl FakeSource {
    fn offering(version: &str) -> Self {
        let source = Self {
            chunks: vec![1000; 4],
            total: Some(4000),
            ..Self::default()
        };
        source.queue_check(CheckOutcome::Available {
            version: version.to_owned(),
            pending: "release",
        });
        source
    }

    fn queue_check(&self, outcome: CheckOutcome<&'static str>) {
        self.checks.lock().unwrap().push_back(outcome);
    }
}

impl UpdateSource for FakeSource {
    type Pending = &'static str;

    fn check(&self) -> CheckOutcome<&'static str> {
        self.check_calls.fetch_add(1, Ordering::SeqCst);
        self.checks
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(CheckOutcome::None)
    }

    fn download(
        &self,
        _pending: &&'static str,
        on_chunk: &mut dyn FnMut(usize, Option<u64>),
    ) -> Result<Vec<u8>, String> {
        self.download_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.download_errors.lock().unwrap().pop_front() {
            return Err(error);
        }
        for &chunk in &self.chunks {
            on_chunk(chunk, self.total);
        }
        Ok(vec![7; self.chunks.iter().sum()])
    }

    fn install(&self, pending: &&'static str, bytes: &[u8]) -> Result<(), String> {
        self.installs
            .lock()
            .unwrap()
            .push((pending, bytes.to_vec()));
        self.install_error.clone().map_or(Ok(()), Err)
    }
}

/// Refuses `refusals` times, then closes. `hold` keeps it refusing until released.
#[derive(Default)]
struct FakeGate {
    refusals: AtomicUsize,
    hold: AtomicBool,
    attempts: AtomicUsize,
    closed: AtomicBool,
    reopened: AtomicUsize,
}

impl InstallGate for FakeGate {
    fn try_close(&self) -> bool {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        if self.hold.load(Ordering::SeqCst) {
            return false;
        }
        let refused = self
            .refusals
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        self.closed.store(!refused, Ordering::SeqCst);
        !refused
    }

    fn reopen(&self) {
        self.reopened.fetch_add(1, Ordering::SeqCst);
        self.closed.store(false, Ordering::SeqCst);
    }
}

fn install<S: UpdateSource>(service: &UpdaterService<S::Pending>, source: &S, gate: &FakeGate) {
    let (version, pending) = service.admit_install().expect("install admitted");
    run_install(
        service,
        source,
        gate,
        Duration::from_millis(1),
        version,
        pending,
    );
}

fn statuses(published: &Published) -> Vec<UpdateStatus> {
    published
        .lock()
        .unwrap()
        .iter()
        .map(|envelope| envelope.status.clone())
        .collect()
}

fn assert_sequences_rise_by_one(published: &Published) {
    let sequences: Vec<u64> = published
        .lock()
        .unwrap()
        .iter()
        .map(|envelope| envelope.sequence)
        .collect();
    let expected: Vec<u64> = (1..=sequences.len() as u64).collect();
    assert_eq!(sequences, expected);
}

// Keep: contract pin for the dashboard's command responses and echo-update-status payload.
#[test]
fn status_envelope_matches_the_dashboard_contract() {
    let cases = [
        (UpdateStatus::None, serde_json::json!({"state": "none"})),
        (
            available("0.5.1"),
            serde_json::json!({"state": "available", "version": "0.5.1"}),
        ),
        (
            downloading(1024, Some(4096)),
            serde_json::json!({"state": "downloading", "version": "0.5.1",
                "downloadedBytes": 1024, "totalBytes": 4096}),
        ),
        (
            downloading(0, None),
            serde_json::json!({"state": "downloading", "version": "0.5.1",
                "downloadedBytes": 0, "totalBytes": null}),
        ),
        (
            version_state("waiting_for_idle"),
            serde_json::json!({"state": "waiting_for_idle", "version": "0.5.1"}),
        ),
        (
            version_state("installing"),
            serde_json::json!({"state": "installing", "version": "0.5.1"}),
        ),
        (
            version_state("failed"),
            serde_json::json!({"state": "failed", "version": "0.5.1"}),
        ),
    ];
    for (status, expected) in cases {
        let envelope = UpdateEnvelope {
            sequence: 3,
            status,
        };
        assert_eq!(
            serde_json::to_value(envelope).unwrap(),
            serde_json::json!({"sequence": 3, "status": expected})
        );
    }
}

// Keep: none and failed checks show nothing, and a failed check keeps an existing offer.
#[test]
fn checks_offer_a_release_and_failures_keep_the_status_exactly() {
    let (service, published) = service();
    let source = FakeSource::default();
    for outcome in [CheckOutcome::None, CheckOutcome::Failed("timeout".into())] {
        source.queue_check(outcome);
        check_once(&service, &source);
        assert_eq!(service.status().status, UpdateStatus::None);
        assert_eq!(service.tray_offer(), TrayOffer::Hidden);
    }
    assert!(published.lock().unwrap().is_empty(), "nothing to show");

    source.queue_check(CheckOutcome::Available {
        version: "0.5.1".into(),
        pending: "release",
    });
    check_once(&service, &source);
    assert_eq!(service.status().status, available("0.5.1"));
    assert_eq!(service.tray_offer(), TrayOffer::Enabled);

    source.queue_check(CheckOutcome::Failed("malformed".into()));
    check_once(&service, &source);
    assert_eq!(
        service.status(),
        UpdateEnvelope {
            sequence: 1,
            status: available("0.5.1")
        }
    );

    source.queue_check(CheckOutcome::None);
    check_once(&service, &source);
    assert_eq!(
        statuses(&published),
        [available("0.5.1"), UpdateStatus::None]
    );
    assert_sequences_rise_by_one(&published);
}

// Keep: an admitted install reports rising progress, then installing, and installs once.
#[test]
fn install_reports_rising_progress_then_installs_once() {
    for (chunks, total, expected) in [
        (
            vec![1000; 4],
            Some(4000),
            vec![
                downloading(0, None),
                downloading(1000, Some(4000)),
                downloading(2000, Some(4000)),
                downloading(3000, Some(4000)),
                downloading(4000, Some(4000)),
            ],
        ),
        // Without a length, one change per MiB rather than per chunk.
        (
            vec![512 * 1024; 5],
            None,
            vec![
                downloading(0, None),
                downloading(512 * 1024, None),
                downloading(1024 * 1024, None),
                downloading(2 * 1024 * 1024, None),
            ],
        ),
    ] {
        let (service, published) = service();
        let source = FakeSource {
            chunks,
            total,
            ..FakeSource::offering("0.5.1")
        };
        check_once(&service, &source);
        published.lock().unwrap().clear();
        let gate = FakeGate::default();

        install(&service, &source, &gate);

        let mut expected = expected;
        expected.push(version_state("installing"));
        assert_eq!(statuses(&published), expected);
        let installs = source.installs.lock().unwrap();
        assert_eq!(installs.len(), 1);
        assert_eq!(installs[0].0, "release");
        assert_eq!(installs[0].1.len(), source.chunks.iter().sum::<usize>());
        assert!(gate.closed.load(Ordering::SeqCst));
    }
}

// Keep: a download or verification failure installs nothing, and a retry downloads again.
#[test]
fn failed_download_installs_nothing_and_a_retry_downloads_again() {
    let (service, _) = service();
    let source = FakeSource::offering("0.5.1");
    source
        .download_errors
        .lock()
        .unwrap()
        .push_back("signature mismatch".into());
    check_once(&service, &source);
    let gate = FakeGate::default();

    install(&service, &source, &gate);
    assert_eq!(service.status().status, version_state("failed"));
    assert_eq!(service.tray_offer(), TrayOffer::Enabled);
    assert!(source.installs.lock().unwrap().is_empty());
    assert_eq!(gate.attempts.load(Ordering::SeqCst), 0);

    install(&service, &source, &gate);
    assert_eq!(source.download_calls.load(Ordering::SeqCst), 2);
    assert_eq!(source.installs.lock().unwrap().len(), 1);
}

// Keep: while one install runs, further requests and scheduled checks change nothing.
#[test]
fn repeated_requests_during_an_install_start_one_download() {
    let (service, published) = service();
    let source = Arc::new(FakeSource::offering("0.5.1"));
    check_once(&service, source.as_ref());
    let gate = Arc::new(FakeGate {
        hold: AtomicBool::new(true),
        ..FakeGate::default()
    });
    let (version, pending) = service.admit_install().unwrap();
    assert_eq!(service.tray_offer(), TrayOffer::Disabled);
    assert!(service.admit_install().is_none(), "downloading");

    let worker = {
        let (service, source, gate) = (service.clone(), Arc::clone(&source), Arc::clone(&gate));
        std::thread::spawn(move || {
            run_install(
                &service,
                source.as_ref(),
                gate.as_ref(),
                Duration::from_millis(1),
                version,
                pending,
            )
        })
    };
    while service.status().status != version_state("waiting_for_idle") {
        std::thread::yield_now();
    }
    let waiting = service.status();
    assert!(service.admit_install().is_none(), "waiting for idle");
    source.queue_check(CheckOutcome::None);
    check_once(&service, source.as_ref());
    assert_eq!(
        source.check_calls.load(Ordering::SeqCst),
        1,
        "no check while busy"
    );
    assert_eq!(service.status(), waiting);
    assert_eq!(service.tray_offer(), TrayOffer::Disabled);

    gate.hold.store(false, Ordering::SeqCst);
    worker.join().unwrap();
    assert_eq!(service.status().status, version_state("installing"));
    assert_eq!(service.tray_offer(), TrayOffer::Disabled);
    assert!(service.admit_install().is_none(), "installing");
    assert_eq!(source.download_calls.load(Ordering::SeqCst), 1);
    assert_eq!(source.installs.lock().unwrap().len(), 1);
    assert_sequences_rise_by_one(&published);
}

// Keep: a busy Echo gets waiting_for_idle and no install until the gate closes; an install
// failure reopens capture admission and leaves the offer retryable.
#[test]
fn install_waits_for_idle_and_a_failed_install_reopens_capture() {
    let (service, published) = service();
    let source = FakeSource {
        install_error: Some("disk full".into()),
        ..FakeSource::offering("0.5.1")
    };
    check_once(&service, &source);
    let gate = FakeGate {
        refusals: AtomicUsize::new(3),
        ..FakeGate::default()
    };

    install(&service, &source, &gate);

    assert_eq!(gate.attempts.load(Ordering::SeqCst), 4);
    let statuses = statuses(&published);
    let waiting = statuses
        .iter()
        .position(|status| *status == version_state("waiting_for_idle"))
        .expect("waiting_for_idle published");
    assert_eq!(statuses[waiting + 1], version_state("installing"));
    assert_eq!(statuses.last(), Some(&version_state("failed")));
    assert_eq!(source.installs.lock().unwrap().len(), 1);
    assert_eq!(gate.reopened.load(Ordering::SeqCst), 1);
    assert!(!gate.closed.load(Ordering::SeqCst));
    assert!(service.admit_install().is_some(), "failed is retryable");
}

// Keep: idle needs no reservation, no running work or delivery, and ten quiet seconds
// after the last of them, so a paste delivered 5 s after its job still lands first.
#[test]
fn idle_requires_no_work_and_ten_quiet_seconds() {
    let job_done = Instant::now();
    let pasted = job_done + Duration::from_secs(6);
    let quiet = Activity::Quiet;
    let cases = [
        (
            "fresh launch",
            false,
            quiet(None),
            quiet(None),
            job_done,
            true,
        ),
        (
            "reservation",
            true,
            quiet(None),
            quiet(None),
            job_done,
            false,
        ),
        (
            "capture or job",
            false,
            Activity::Running,
            quiet(None),
            job_done,
            false,
        ),
        (
            "delivery",
            false,
            quiet(None),
            Activity::Running,
            job_done,
            false,
        ),
        (
            "9 s after a job",
            false,
            quiet(Some(job_done)),
            quiet(None),
            job_done + Duration::from_secs(9),
            false,
        ),
        (
            "10 s after a job",
            false,
            quiet(Some(job_done)),
            quiet(None),
            job_done + Duration::from_secs(10),
            true,
        ),
        (
            "renderer delivering 5 s after the job",
            false,
            quiet(Some(job_done)),
            Activity::Running,
            job_done + Duration::from_secs(5),
            false,
        ),
        (
            "12 s after the job, 6 s after its paste",
            false,
            quiet(Some(job_done)),
            quiet(Some(pasted)),
            job_done + Duration::from_secs(12),
            false,
        ),
        (
            "10 s after the paste",
            false,
            quiet(Some(job_done)),
            quiet(Some(pasted)),
            pasted + Duration::from_secs(10),
            true,
        ),
    ];
    for (case, reserved, work, deliveries, now, expected) in cases {
        assert_eq!(is_idle(reserved, work, deliveries, now), expected, "{case}");
    }
}

fn quiet_deliveries() -> Activity {
    Activity::Quiet(None)
}

fn idle_gate(capture: &DesktopAudioCapture, runtime: &EchoRuntime) -> IdleGate {
    IdleGate {
        capture: capture.clone(),
        runtime: runtime.clone(),
        deliveries: quiet_deliveries,
    }
}

fn reserve(capture: &DesktopAudioCapture, intent_id: &str) -> Result<(), CaptureErrorCode> {
    capture
        .reserve(
            owner("overlay"),
            CaptureIntent {
                intent_id: intent_id.to_owned(),
                purpose: CapturePurpose::PushToTalk,
            },
        )
        .map(|_| ())
        .map_err(|error| error.code)
}

fn cancel(capture: &DesktopAudioCapture, intent_id: &str) {
    capture
        .cancel(CancelCapture {
            owner: owner("overlay"),
            intent_id: intent_id.to_owned(),
            session_id: None,
            reason: "user_cancelled",
        })
        .unwrap();
}

fn capture_event(phase: CapturePhase) -> CaptureEvent {
    CaptureEvent {
        elapsed_ms: 0,
        limit_ms: 0,
        sequence: 1,
        phase,
        purpose: Some(CapturePurpose::PushToTalk),
        intent_id: Some("intent".into()),
        session_id: None,
        artifact_id: None,
        outcome: "test",
        error: None,
        has_observed_callback: false,
        callback_count: 0,
        sample_count: 0,
        last_error: None,
        artifact_count: 0,
        artifact_bytes: 0,
    }
}

// Keep: the gate reads the real reservation and runtime state and waits out the quiet
// period after a capture or job ends.
#[test]
fn idle_gate_waits_for_reservations_captures_jobs_and_the_quiet_period() {
    let (backend, _, _, _) = FakeBackend::blocked();
    let capture = spawn_coordinator(backend, AudioCapturePolicy::default());
    let runtime = EchoRuntime::default();
    let gate = idle_gate(&capture, &runtime);
    let later = || Instant::now() + Duration::from_secs(11);

    reserve(&capture, "held").unwrap();
    assert!(!gate.try_close_at(later()), "reservation");
    cancel(&capture, "held");

    runtime
        .apply_capture_event(&capture_event(CapturePhase::Recording))
        .unwrap();
    assert!(!gate.try_close_at(later()), "recording");
    runtime
        .apply_capture_event(&capture_event(CapturePhase::Idle))
        .unwrap();
    assert!(!gate.try_close_at(Instant::now()), "capture just ended");

    runtime.job_started_for_capture(1, "intent").unwrap();
    assert!(!gate.try_close_at(later()), "job");
    runtime
        .job_failed(1, EchoErrorCode::ProviderFailed)
        .unwrap();
    assert!(!gate.try_close_at(Instant::now()), "job just settled");

    assert!(gate.try_close_at(later()), "quiet period over");
    gate.reopen();
}

// Keep: a closed barrier refuses capture before the microphone opens; reopening it after a
// failed install lets the next capture start normally.
#[test]
fn closed_barrier_refuses_capture_without_opening_the_microphone() {
    let (backend, open_started, release_open, _) = FakeBackend::blocked();
    let capture = spawn_coordinator(backend, AudioCapturePolicy::default());
    let runtime = EchoRuntime::default();
    let gate = idle_gate(&capture, &runtime);
    assert!(gate.try_close_at(Instant::now()));

    let refused = capture
        .reserve(
            owner("overlay"),
            CaptureIntent {
                intent_id: "during-install".into(),
                purpose: CapturePurpose::PushToTalk,
            },
        )
        .unwrap_err();
    assert_eq!(refused.code, CaptureErrorCode::UpdateInstalling);
    assert!(refused.retryable);
    assert_eq!(
        serde_json::to_value(refused.code).unwrap(),
        "update_installing"
    );
    assert!(!capture.has_reservation());
    assert!(open_started.try_recv().is_err(), "microphone opened");

    gate.reopen();
    release_open.send(()).unwrap();
    let session = crate::audio::tests::fixtures::begin(&capture, owner("overlay"), "after-failure");
    assert_eq!(session.phase, CapturePhase::Recording);
    open_started
        .try_recv()
        .expect("microphone opened after reopening");
}

static INTERLEAVED_CAPTURE: Mutex<Option<DesktopAudioCapture>> = Mutex::new(None);

/// Reserves on the shared capture during the gate's first idle read, as a capture start
/// landing between that read and the barrier would.
fn deliveries_while_a_capture_starts() -> Activity {
    if let Some(capture) = INTERLEAVED_CAPTURE.lock().unwrap().take() {
        reserve(&capture, "interleaved").unwrap();
    }
    Activity::Quiet(None)
}

// Keep: the barrier race, the order closed_barrier_refuses_capture does not cover. A
// capture reserved after the gate's first idle read but before the barrier is seen by
// the read after the barrier, so the install keeps waiting and the capture runs.
#[test]
fn capture_reserved_before_the_barrier_keeps_the_install_waiting() {
    let (backend, _, _, _) = FakeBackend::blocked();
    let capture = spawn_coordinator(backend, AudioCapturePolicy::default());
    *INTERLEAVED_CAPTURE.lock().unwrap() = Some(capture.clone());
    let gate = IdleGate {
        capture: capture.clone(),
        runtime: EchoRuntime::default(),
        deliveries: deliveries_while_a_capture_starts,
    };

    assert!(!gate.try_close_at(Instant::now()));
    assert!(capture.has_reservation(), "the capture proceeds");
    cancel(&capture, "interleaved");
    assert_eq!(reserve(&capture, "next"), Ok(()), "barrier left set");
}

// Keep: checks run at launch and on the first 5-minute tick at which wall time is a day
// past the last check, including after system sleep and a clock moved back.
#[test]
fn schedule_checks_at_launch_and_once_wall_time_passes_a_day() {
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let minutes = |value: u64| Duration::from_secs(value * 60);
    const HOUR: i64 = 3600;
    // Wall seconds a tick adds beyond its own five minutes, keyed by tick number, and the
    // expected check times in seconds from launch.
    let cases = [
        ("awake", Vec::<(usize, i64)>::new(), 300, vec![0, 24 * HOUR]),
        (
            "system asleep for 30 h during the tenth tick",
            vec![(10, 30 * HOUR)],
            12,
            vec![0, 50 * 60 + 30 * HOUR],
        ),
        (
            "clock moved back a day",
            vec![(2, -24 * HOUR)],
            4,
            vec![0, 10 * 60 - 24 * HOUR],
        ),
    ];
    for (case, jumps, ticks, expected) in cases {
        let (service, _) = service();
        let source = FakeSource::default();
        let clock = Mutex::new((start, 0usize));
        let checked = Mutex::new(Vec::new());
        let tracking = TrackingSource {
            inner: &source,
            on_check: || checked.lock().unwrap().push(clock.lock().unwrap().0),
        };
        run_schedule(
            &service,
            &tracking,
            || clock.lock().unwrap().0,
            |tick| {
                assert_eq!(tick, minutes(5));
                let mut clock = clock.lock().unwrap();
                clock.1 += 1;
                let extra = jumps
                    .iter()
                    .find(|(at, _)| *at == clock.1)
                    .map_or(0, |(_, seconds)| *seconds);
                let advanced = clock.0 + tick;
                clock.0 = if extra >= 0 {
                    advanced + Duration::from_secs(extra as u64)
                } else {
                    advanced - Duration::from_secs(extra.unsigned_abs())
                };
                clock.1 < ticks
            },
        );
        let offsets: Vec<i64> = checked
            .lock()
            .unwrap()
            .iter()
            .map(|time| match time.duration_since(start) {
                Ok(after) => after.as_secs() as i64,
                Err(before) => -(before.duration().as_secs() as i64),
            })
            .collect();
        assert_eq!(offsets, expected, "{case}");
    }
}

struct TrackingSource<'a, F: Fn()> {
    inner: &'a FakeSource,
    on_check: F,
}

impl<F: Fn()> UpdateSource for TrackingSource<'_, F> {
    type Pending = &'static str;

    fn check(&self) -> CheckOutcome<&'static str> {
        (self.on_check)();
        self.inner.check()
    }

    fn download(
        &self,
        pending: &&'static str,
        on_chunk: &mut dyn FnMut(usize, Option<u64>),
    ) -> Result<Vec<u8>, String> {
        self.inner.download(pending, on_chunk)
    }

    fn install(&self, pending: &&'static str, bytes: &[u8]) -> Result<(), String> {
        self.inner.install(pending, bytes)
    }
}

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn status(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    fn body(body: impl Into<Vec<u8>>) -> Self {
        Self {
            body: body.into(),
            ..Self::status(200)
        }
    }

    fn redirect(location: String) -> Self {
        Self {
            headers: vec![("Location", location)],
            ..Self::status(302)
        }
    }
}

/// A plain HTTP server on 127.0.0.1 that records each request path. Needs permission
/// to bind a loopback listener.
struct Loopback {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Loopback {
    fn start(route: impl Fn(&str, &str) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback listener");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let (served_base, recorded) = (base.clone(), Arc::clone(&requests));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut header = String::new();
                while reader.read_line(&mut header).is_ok_and(|read| read > 2) {
                    header.clear();
                }
                let path = request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_owned();
                recorded.lock().unwrap().push(path.clone());
                let reply = route(&served_base, &path);
                let mut head = format!(
                    "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nConnection: close\r\n",
                    reply.status,
                    reply.body.len()
                );
                for (name, value) in reply.headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&reply.body);
            }
        });
        Self { base, requests }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

fn manifest(version: &str, artifact_url: &str) -> Vec<u8> {
    let target = tauri_plugin_updater::target().unwrap();
    serde_json::json!({
        "version": version,
        "platforms": {
            target: {"url": artifact_url, "signature": ARTIFACT_SIGNATURE.trim()}
        }
    })
    .to_string()
    .into_bytes()
}

/// A mock app running the real updater plugin as an installed 0.5.0 would.
fn plugin_app(endpoints: &[String], pubkey: &str, insecure: bool) -> tauri::App<MockRuntime> {
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.package_info_mut().version = "0.5.0".parse().unwrap();
    let mut updater = serde_json::json!({"pubkey": pubkey.trim(), "endpoints": endpoints});
    if insecure {
        updater["dangerousInsecureTransportProtocol"] = true.into();
    }
    context
        .config_mut()
        .plugins
        .0
        .insert("updater".into(), updater);
    tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)
        .unwrap()
}

fn plugin_check(endpoints: &[String], insecure: bool) -> CheckOutcome<Update> {
    let app = plugin_app(endpoints, TRUSTED_KEY, insecure);
    PluginSource::new(app.handle().clone()).check()
}

fn offered_version(outcome: &CheckOutcome<Update>) -> Option<&str> {
    match outcome {
        CheckOutcome::Available { version, .. } => Some(version),
        _ => None,
    }
}

// Keep: 404s from both endpoints read as no update, checked in configured order, and the
// second endpoint is used when the first has no manifest.
#[test]
fn missing_manifests_read_as_no_update() {
    let server = Loopback::start(|base, path| match path {
        "/fallback/latest.json" => Reply::body(manifest("0.5.1", &format!("{base}/artifact"))),
        _ => Reply::status(404),
    });
    let endpoints = [
        server.url("/first/latest.json"),
        server.url("/second/latest.json"),
    ];
    assert!(matches!(plugin_check(&endpoints, true), CheckOutcome::None));
    assert_eq!(
        server.requests(),
        ["/first/latest.json", "/second/latest.json"]
    );

    let fallback = [
        server.url("/first/latest.json"),
        server.url("/fallback/latest.json"),
    ];
    assert_eq!(
        offered_version(&plugin_check(&fallback, true)),
        Some("0.5.1")
    );
}

// Keep: the real comparator offers only higher stable versions, never a downgrade,
// the same version or a prerelease.
#[test]
fn only_a_higher_stable_release_is_offered() {
    let server = Loopback::start(|base, path| {
        let version = path
            .trim_start_matches('/')
            .trim_end_matches("/latest.json");
        Reply::body(manifest(version, &format!("{base}/artifact")))
    });
    for (version, offered) in [
        ("0.5.0", None),
        ("0.4.9", None),
        ("0.5.1-beta.1", None),
        ("0.5.1", Some("0.5.1")),
        ("1.0.0", Some("1.0.0")),
    ] {
        let endpoint = [server.url(&format!("/{version}/latest.json"))];
        assert_eq!(
            offered_version(&plugin_check(&endpoint, true)),
            offered,
            "{version}"
        );
    }
}

/// Serves `/redirect/<n>` (n more redirects, then a 0.5.1 offer) and `/malformed`; every
/// other path is 404.
fn release_server() -> Loopback {
    Loopback::start(|base, path| {
        if let Some(hops) = path.strip_prefix("/redirect/") {
            let hops: u32 = hops.parse().unwrap();
            return match hops {
                0 => Reply::body(manifest("0.5.1", &format!("{base}/artifact"))),
                _ => Reply::redirect(format!("{base}/redirect/{}", hops - 1)),
            };
        }
        match path {
            "/malformed" => Reply::body("{\"version\": "),
            _ => Reply::status(404),
        }
    })
}

/// An https:// endpoint whose server answers each TLS ClientHello with a fatal
/// handshake_failure alert, so the TCP connection succeeds and the TLS handshake fails.
/// The counter records the ClientHellos it received.
fn tls_refusing_endpoint() -> (String, Arc<AtomicUsize>) {
    const HANDSHAKE_RECORD: u8 = 22;
    // Content type alert (21), TLS 1.2, length 2, level fatal (2), handshake_failure (40).
    const FATAL_HANDSHAKE_FAILURE: [u8; 7] = [21, 3, 3, 0, 2, 2, 40];
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback listener");
    let endpoint = format!("https://{}/latest.json", listener.local_addr().unwrap());
    let hellos = Arc::new(AtomicUsize::new(0));
    let received = Arc::clone(&hellos);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut header = [0u8; 5];
            if stream.read_exact(&mut header).is_err() || header[0] != HANDSHAKE_RECORD {
                continue;
            }
            // Read the whole record first: closing with unread bytes resets the connection
            // before the client reads the alert.
            let mut hello = vec![0u8; usize::from(u16::from_be_bytes([header[3], header[4]]))];
            if stream.read_exact(&mut hello).is_err() {
                continue;
            }
            received.fetch_add(1, Ordering::SeqCst);
            let _ = stream.write_all(&FATAL_HANDSHAKE_FAILURE);
            let _ = stream.shutdown(std::net::Shutdown::Write);
            let _ = stream.read_to_end(&mut Vec::new());
        }
    });
    (endpoint, hellos)
}

/// The check timeout the failure cases use, so a timeout costs this instead of 30 s.
const SHORT_CHECK_TIMEOUT: Duration = Duration::from_millis(250);

struct FailingCheck {
    case: &'static str,
    endpoint: String,
    insecure: bool,
}

/// One check failure of each kind a check must survive without changing the status.
/// `release` is a [`release_server`]. The timeout case is a server that offers 0.5.1 only
/// after 3 s, so a check that ignored its timeout would read the offer instead of failing.
fn failing_checks(release: &Loopback) -> (Vec<FailingCheck>, Arc<AtomicUsize>) {
    let slow = Loopback::start(|base, _| {
        std::thread::sleep(Duration::from_secs(3));
        Reply::body(manifest("0.5.1", &format!("{base}/artifact")))
    });
    let (tls_endpoint, tls_hellos) = tls_refusing_endpoint();
    let closed_port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/latest.json", listener.local_addr().unwrap())
    };
    let case = |case, endpoint, insecure| FailingCheck {
        case,
        endpoint,
        insecure,
    };
    let cases = vec![
        case("timeout", slow.url("/latest.json"), true),
        case("TLS handshake failure", tls_endpoint, false),
        case("malformed JSON", release.url("/malformed"), true),
        case("six redirects", release.url("/redirect/6"), true),
        case(
            "http under the shipped config",
            release.url("/redirect/0"),
            false,
        ),
        case("connection refused", closed_port, true),
    ];
    (cases, tls_hellos)
}

/// The real plugin source for `failing`, with the short check timeout.
fn failing_source(failing: &FailingCheck) -> (tauri::App<MockRuntime>, PluginSource<MockRuntime>) {
    let app = plugin_app(
        std::slice::from_ref(&failing.endpoint),
        TRUSTED_KEY,
        failing.insecure,
    );
    let source = PluginSource {
        check_timeout: SHORT_CHECK_TIMEOUT,
        ..PluginSource::new(app.handle().clone())
    };
    (app, source)
}

// Keep: a timeout, a TLS handshake failure, malformed JSON, a sixth redirect, an http:// hop
// under the shipped config and a refused connection each fail the check through the real
// plugin and leave the envelope unchanged, so nothing is published: none stays none, and an
// existing offer stays available with its version. Five redirects still reach the manifest.
#[test]
fn check_failures_keep_the_status_and_redirects_stop_after_five() {
    let release = release_server();
    assert_eq!(
        offered_version(&plugin_check(&[release.url("/redirect/5")], true)),
        Some("0.5.1")
    );
    let offering = plugin_app(&[release.url("/redirect/0")], TRUSTED_KEY, true);
    let (cases, tls_hellos) = failing_checks(&release);
    for failing in &cases {
        for offered in [false, true] {
            let (service, _) = service::<Update>();
            if offered {
                check_once(&service, &PluginSource::new(offering.handle().clone()));
                assert_eq!(service.status().status, available("0.5.1"));
            }
            let before = service.status();
            let requests_before = release.requests().len();

            let (_app, source) = failing_source(failing);
            check_once(&service, &source);

            let case = failing.case;
            assert_eq!(service.status(), before, "{case}, offered: {offered}");
            if case == "http under the shipped config" {
                assert_eq!(
                    release.requests().len(),
                    requests_before,
                    "{case} sent a request"
                );
            }
        }
    }
    assert_eq!(
        tls_hellos.load(Ordering::SeqCst),
        2,
        "TLS handshakes attempted"
    );
}

// Keep: a shipped check gives up after 30 s; the failure cases above shorten it.
#[test]
fn shipped_checks_time_out_after_30_seconds() {
    let app = plugin_app(&[], TRUSTED_KEY, false);
    assert_eq!(
        PluginSource::new(app.handle().clone()).check_timeout,
        Duration::from_secs(30)
    );
}

/// Delegates to the real plugin but never installs, so a test cannot replace anything.
struct NeverInstalls<S> {
    inner: S,
    installs: AtomicUsize,
}

impl<S: UpdateSource> UpdateSource for NeverInstalls<S> {
    type Pending = S::Pending;

    fn check(&self) -> CheckOutcome<S::Pending> {
        self.inner.check()
    }

    fn download(
        &self,
        pending: &S::Pending,
        on_chunk: &mut dyn FnMut(usize, Option<u64>),
    ) -> Result<Vec<u8>, String> {
        self.inner.download(pending, on_chunk)
    }

    fn install(&self, _pending: &S::Pending, _bytes: &[u8]) -> Result<(), String> {
        self.installs.fetch_add(1, Ordering::SeqCst);
        Err("tests never install".into())
    }
}

// Keep: tampered bytes, a foreign signing key or an http:// artifact under the shipped
// config fail the install at download, with nothing installed; the untampered artifact
// verifies against the configured key.
#[test]
fn tampered_or_foreign_artifacts_fail_without_installing() {
    let server = Loopback::start(|base, path| match path {
        "/latest.json" => Reply::body(manifest("0.5.1", &format!("{base}/artifact"))),
        "/tampered/latest.json" => Reply::body(manifest("0.5.1", &format!("{base}/tampered"))),
        "/artifact" => Reply::body(ARTIFACT),
        "/tampered" => {
            let mut bytes = ARTIFACT.to_vec();
            bytes[0] ^= 1;
            Reply::body(bytes)
        }
        _ => Reply::status(404),
    });
    let check = |path: &str, key: &str| {
        let app = plugin_app(&[server.url(path)], key, true);
        let source = PluginSource::new(app.handle().clone());
        let CheckOutcome::Available { pending, .. } = source.check() else {
            panic!("{path} offers 0.5.1");
        };
        (app, source, pending)
    };

    let (_app, source, pending) = check("/latest.json", TRUSTED_KEY);
    assert_eq!(source.download(&pending, &mut |_, _| {}).unwrap(), ARTIFACT);

    for (case, path, key) in [
        ("tampered bytes", "/tampered/latest.json", TRUSTED_KEY),
        ("foreign signing key", "/latest.json", OTHER_KEY),
    ] {
        let (_app, source, _) = check(path, key);
        let guarded = NeverInstalls {
            inner: source,
            installs: AtomicUsize::new(0),
        };
        let (service, _) = service::<Update>();
        check_once(&service, &guarded);
        let gate = FakeGate::default();
        install(&service, &guarded, &gate);
        assert_eq!(service.status().status, version_state("failed"), "{case}");
        assert_eq!(guarded.installs.load(Ordering::SeqCst), 0, "{case}");
        assert_eq!(gate.attempts.load(Ordering::SeqCst), 0, "{case}");
    }

    // An http:// artifact under the shipped config: restore_update makes no request, so the
    // offer exists without a TLS loopback, and the download must refuse the hop.
    let shipped = plugin_app(&[server.url("/latest.json")], TRUSTED_KEY, false);
    let source = PluginSource::new(shipped.handle().clone());
    let offer = serde_json::from_slice(&manifest("0.5.1", &server.url("/artifact"))).unwrap();
    let update = source
        .updater()
        .unwrap()
        .restore_update(offer)
        .unwrap()
        .expect("0.5.1 offered");
    let artifact_requests = || {
        server
            .requests()
            .iter()
            .filter(|p| *p == "/artifact")
            .count()
    };
    let before = artifact_requests();
    assert!(source.download(&update, &mut |_, _| {}).is_err());
    assert_eq!(artifact_requests(), before, "http artifact requested");
}

// Keep: a debug build, as `bun run tauri:dev` runs, never contacts an endpoint.
#[test]
fn debug_builds_never_contact_the_endpoints() {
    let server = Loopback::start(|_, _| Reply::status(404));
    let app = plugin_app(&[server.url("/latest.json")], TRUSTED_KEY, true);
    app.manage(UpdaterService::<Update>::new(|_| {}));

    start(app.handle());
    std::thread::sleep(Duration::from_millis(500));

    assert!(server.requests().is_empty());
    assert_eq!(
        app.state::<UpdaterService>().status().status,
        UpdateStatus::None
    );
}

// Keep: the log lines Alex reviews. The log facade accepts one logger per process, so
// this runs in a child process.
#[test]
fn checks_log_none_for_missing_manifests_and_failed_for_errors() {
    const CHILD: &str = "ECHO_TEST_UPDATE_CHECK_LOG_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        let status = crate::test_support::clear_child_environment(&mut command)
            .env(CHILD, "1")
            .args([
                "--exact",
                "services::updater::tests::checks_log_none_for_missing_manifests_and_failed_for_errors",
                "--nocapture",
            ])
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    struct Records(Mutex<Vec<String>>);
    static RECORDS: Records = Records(Mutex::new(Vec::new()));
    impl log::Log for Records {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, record: &log::Record<'_>) {
            let line = record.args().to_string();
            if line.starts_with("update_check") {
                self.0.lock().unwrap().push(line);
            }
        }
        fn flush(&self) {}
    }
    log::set_logger(&RECORDS).unwrap();
    log::set_max_level(log::LevelFilter::Info);

    let release = release_server();
    let missing = plugin_app(&[release.url("/latest.json")], TRUSTED_KEY, true);
    check_once(
        &service::<Update>().0,
        &PluginSource::new(missing.handle().clone()),
    );
    let (cases, _) = failing_checks(&release);
    for failing in &cases {
        let (_app, source) = failing_source(failing);
        check_once(&service::<Update>().0, &source);
    }
    let records = RECORDS.0.lock().unwrap().clone();
    assert_eq!(records.len(), 1 + cases.len(), "{records:?}");
    assert_eq!(records[0], "update_check outcome=none");
    for (failing, record) in cases.iter().zip(&records[1..]) {
        assert!(
            record.starts_with("update_check outcome=failed error="),
            "{}: {record}",
            failing.case
        );
    }
}

// Keep: the native half of the loop across layers. A dashboard reads none over real IPC, a
// check against a loopback release offers 0.5.1, the install downloads and verifies the
// signed artifact with progress, waits while a capture is reserved, then installs; the
// stand-in installer fails, so capture admission reopens and the offer reads failed.
#[test]
fn update_loop_walks_from_offer_to_install_over_loopback() {
    let server = Loopback::start(|base, path| match path {
        "/latest.json" => Reply::body(manifest("0.5.1", &format!("{base}/artifact"))),
        "/artifact" => Reply::body(ARTIFACT),
        _ => Reply::status(404),
    });
    let mut context = tauri::generate_context!(test = true);
    context.package_info_mut().version = "0.5.0".parse().unwrap();
    context.config_mut().plugins.0.insert(
        "updater".into(),
        serde_json::json!({
            "pubkey": TRUSTED_KEY.trim(),
            "endpoints": [server.url("/latest.json")],
            "dangerousInsecureTransportProtocol": true
        }),
    );
    let (service, published) = service::<Update>();
    let app = tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(service.clone())
        .invoke_handler(tauri::generate_handler![
            crate::commands::update::get_update_status,
            crate::commands::update::install_update
        ])
        .build(context)
        .unwrap();
    let read_status = |label: &str| {
        let window = app.get_webview_window(label).unwrap_or_else(|| {
            tauri::WebviewWindowBuilder::new(&app, label, Default::default())
                .build()
                .unwrap()
        });
        tauri::test::get_ipc_response(
            &window,
            tauri::webview::InvokeRequest {
                cmd: "get_update_status".into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: if cfg!(windows) {
                    "http://tauri.localhost"
                } else {
                    "tauri://localhost"
                }
                .parse()
                .unwrap(),
                body: Default::default(),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
        .map(|body| body.deserialize::<serde_json::Value>().unwrap())
    };
    println!(
        "e2e: launch, dashboard reads {}",
        read_status("dashboard").unwrap()
    );
    assert!(read_status("overlay").is_err(), "overlay denied");
    println!("e2e: overlay get_update_status denied by the capability layer");

    let source = NeverInstalls {
        inner: PluginSource::new(app.handle().clone()),
        installs: AtomicUsize::new(0),
    };
    check_once(&service, &source);
    println!(
        "e2e: check, dashboard reads {}",
        read_status("dashboard").unwrap()
    );
    assert_eq!(service.tray_offer(), TrayOffer::Enabled);
    println!("e2e: tray offers Update Echo");

    let (backend, _, release_open, _) = FakeBackend::blocked();
    let capture = spawn_coordinator(backend, AudioCapturePolicy::default());
    reserve(&capture, "dictating").unwrap();
    println!("e2e: a dictation holds the microphone reservation");
    let gate = IdleGate {
        capture: capture.clone(),
        runtime: EchoRuntime::default(),
        deliveries: quiet_deliveries,
    };
    let (version, pending) = service.admit_install().expect("install admitted");
    assert!(service.admit_install().is_none(), "second request ignored");
    let worker = {
        let service = service.clone();
        std::thread::spawn(move || {
            run_install(
                &service,
                &source,
                &gate,
                Duration::from_millis(5),
                version,
                pending,
            );
            source.installs.load(Ordering::SeqCst)
        })
    };
    while service.status().status != version_state("waiting_for_idle") {
        std::thread::sleep(Duration::from_millis(5));
    }
    println!(
        "e2e: downloaded and verified, dashboard reads {}",
        read_status("dashboard").unwrap()
    );
    cancel(&capture, "dictating");
    println!("e2e: the dictation ends");
    assert_eq!(worker.join().unwrap(), 1, "one install");
    println!(
        "e2e: stand-in installer ran once and failed, dashboard reads {}",
        read_status("dashboard").unwrap()
    );
    release_open.send(()).unwrap();
    crate::audio::tests::fixtures::begin(&capture, owner("overlay"), "after-update");
    println!("e2e: capture starts again after the failed install");
    for envelope in published.lock().unwrap().iter() {
        println!("e2e: event {}", serde_json::to_string(envelope).unwrap());
    }
    assert_sequences_rise_by_one(&published);
    let statuses = statuses(&published);
    assert_eq!(statuses.first(), Some(&available("0.5.1")));
    assert!(statuses.contains(&downloading(
        ARTIFACT.len() as u64,
        Some(ARTIFACT.len() as u64)
    )));
    assert_eq!(
        &statuses[statuses.len() - 3..],
        [
            version_state("waiting_for_idle"),
            version_state("installing"),
            version_state("failed")
        ]
    );
    assert!(
        server
            .requests()
            .iter()
            .filter(|p| *p == "/artifact")
            .count()
            == 1
    );
}
