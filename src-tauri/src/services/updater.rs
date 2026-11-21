//! In-app updates. Rust owns every update request: it checks the configured endpoints at
//! launch and once a day, and on request downloads and verifies the release, waits until
//! Echo is idle, installs it and restarts. The dashboard reads the status and asks for the
//! install through `commands/update.rs`; the tray offers the same install.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

use crate::audio::DesktopAudioCapture;
use crate::services::background_mode;
use crate::services::runtime::{Activity, EchoRuntime};
use crate::services::transcript_delivery;

/// Emitted to the dashboard with an [`UpdateEnvelope`] on every status change.
pub const STATUS_EVENT: &str = "echo-update-status";
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Monotonic sleep pauses while the system sleeps, so each tick re-reads wall time
/// instead of sleeping for the whole interval.
const SCHEDULE_TICK: Duration = Duration::from_secs(5 * 60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MAX_REDIRECTS: usize = 5;
const IDLE_POLL: Duration = Duration::from_secs(1);
/// Covers the gap between a job completing and the renderer delivering its transcript:
/// a manual-mode transcript never calls delivery, so only a timeout can cover it.
const QUIET_PERIOD: Duration = Duration::from_secs(10);
const MIB: u64 = 1024 * 1024;

/// The wire status. `version` is the release's semver without a leading `v`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum UpdateStatus {
    None,
    Available {
        version: String,
    },
    Downloading {
        version: String,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    WaitingForIdle {
        version: String,
    },
    Installing {
        version: String,
    },
    Failed {
        version: String,
    },
}

/// `sequence` rises by one on every status change, so the dashboard can drop stale events.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UpdateEnvelope {
    pub sequence: u64,
    pub status: UpdateStatus,
}

/// How the tray shows the Update Echo item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrayOffer {
    Hidden,
    Enabled,
    Disabled,
}

pub(crate) enum CheckOutcome<P> {
    None,
    Available { version: String, pending: P },
    Failed(String),
}

/// Where releases come from. Production uses the Tauri updater plugin; tests use fakes.
pub(crate) trait UpdateSource {
    type Pending: Clone;
    fn check(&self) -> CheckOutcome<Self::Pending>;
    /// Returns the artifact only after its signature verified against the configured key.
    fn download(
        &self,
        pending: &Self::Pending,
        on_chunk: &mut dyn FnMut(usize, Option<u64>),
    ) -> Result<Vec<u8>, String>;
    /// Installs and restarts Echo. On Windows the installer exits the process instead of returning.
    fn install(&self, pending: &Self::Pending, bytes: &[u8]) -> Result<(), String>;
}

/// Admission to install, atomic with capture admission.
pub(crate) trait InstallGate {
    /// Sets the install barrier when Echo is idle; otherwise leaves it clear and returns false.
    fn try_close(&self) -> bool;
    fn reopen(&self);
}

pub struct UpdaterService<P = Update> {
    state: Arc<Mutex<ServiceState<P>>>,
    publish: Arc<dyn Fn(&UpdateEnvelope) + Send + Sync>,
}

impl<P> Clone for UpdaterService<P> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            publish: Arc::clone(&self.publish),
        }
    }
}

struct ServiceState<P> {
    sequence: u64,
    status: UpdateStatus,
    /// The release the current offer refers to, kept after a failure so a retry downloads it again.
    pending: Option<P>,
    /// An install request owns the status from admission until it fails or Echo restarts.
    busy: bool,
}

impl<P> ServiceState<P> {
    fn envelope(&self) -> UpdateEnvelope {
        UpdateEnvelope {
            sequence: self.sequence,
            status: self.status.clone(),
        }
    }

    fn change(&mut self, status: UpdateStatus) -> Option<UpdateEnvelope> {
        if self.status == status {
            return None;
        }
        self.status = status;
        self.sequence += 1;
        Some(self.envelope())
    }
}

impl<P: Clone> UpdaterService<P> {
    /// `publish` runs outside the state lock after every status change.
    pub fn new(publish: impl Fn(&UpdateEnvelope) + Send + Sync + 'static) -> Self {
        Self {
            state: Arc::new(Mutex::new(ServiceState {
                sequence: 0,
                status: UpdateStatus::None,
                pending: None,
                busy: false,
            })),
            publish: Arc::new(publish),
        }
    }

    pub fn status(&self) -> UpdateEnvelope {
        self.lock().envelope()
    }

    pub fn tray_offer(&self) -> TrayOffer {
        match self.lock().status {
            UpdateStatus::None => TrayOffer::Hidden,
            UpdateStatus::Available { .. } | UpdateStatus::Failed { .. } => TrayOffer::Enabled,
            UpdateStatus::Downloading { .. }
            | UpdateStatus::WaitingForIdle { .. }
            | UpdateStatus::Installing { .. } => TrayOffer::Disabled,
        }
    }

    fn is_busy(&self) -> bool {
        self.lock().busy
    }

    /// A failed check keeps the status exactly as it was; an install in progress ignores checks.
    fn apply_check(&self, outcome: CheckOutcome<P>) {
        let changed = {
            let mut state = self.lock();
            if state.busy {
                return;
            }
            match outcome {
                CheckOutcome::Failed(_) => return,
                CheckOutcome::None => {
                    state.pending = None;
                    state.change(UpdateStatus::None)
                }
                CheckOutcome::Available { version, pending } => {
                    state.pending = Some(pending);
                    state.change(UpdateStatus::Available { version })
                }
            }
        };
        self.publish(changed);
    }

    /// Admits one install from available or failed. Any other state ignores the request.
    fn admit_install(&self) -> Option<(String, P)> {
        let (admitted, changed) = {
            let mut state = self.lock();
            let version = match &state.status {
                UpdateStatus::Available { version } | UpdateStatus::Failed { version }
                    if !state.busy =>
                {
                    version.clone()
                }
                _ => return None,
            };
            let pending = state.pending.clone()?;
            state.busy = true;
            let changed = state.change(UpdateStatus::Downloading {
                version: version.clone(),
                downloaded_bytes: 0,
                total_bytes: None,
            });
            ((version, pending), changed)
        };
        self.publish(changed);
        Some(admitted)
    }

    fn set_status(&self, status: UpdateStatus) {
        let changed = self.lock().change(status);
        self.publish(changed);
    }

    fn fail(&self, version: String) {
        let changed = {
            let mut state = self.lock();
            state.busy = false;
            state.change(UpdateStatus::Failed { version })
        };
        self.publish(changed);
    }

    fn publish(&self, changed: Option<UpdateEnvelope>) {
        if let Some(envelope) = changed {
            (self.publish)(&envelope);
        }
    }

    fn lock(&self) -> MutexGuard<'_, ServiceState<P>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Manages the service. Every status change is emitted to the dashboard and rebuilds the tray menu.
pub fn manage(app: &AppHandle) {
    let handle = app.clone();
    app.manage(UpdaterService::<Update>::new(move |envelope| {
        if let Err(error) = handle.emit_to("dashboard", STATUS_EVENT, envelope) {
            log::warn!("Could not publish the update status: {error}");
        }
        background_mode::refresh_tray_menu(&handle);
    }));
}

/// Runs the launch check and the daily schedule in release builds only, so `tauri dev`
/// and other debug builds never contact the update endpoints.
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    if cfg!(debug_assertions) {
        return;
    }
    let service = app.state::<UpdaterService>().inner().clone();
    let source = PluginSource { app: app.clone() };
    let spawned = std::thread::Builder::new()
        .name("echo-update-schedule".to_owned())
        .spawn(move || {
            run_schedule(&service, &source, SystemTime::now, |tick| {
                std::thread::sleep(tick);
                true
            });
        });
    if let Err(error) = spawned {
        log::warn!("update_check outcome=failed error=schedule thread: {error}");
    }
}

pub fn tray_offer(app: &AppHandle) -> TrayOffer {
    app.try_state::<UpdaterService>()
        .map_or(TrayOffer::Hidden, |service| service.tray_offer())
}

/// The sidebar button and the tray item both land here. Returns as soon as the request
/// is admitted (status already downloading) or ignored (status unchanged).
pub fn request_install<R: Runtime>(app: &AppHandle<R>) -> UpdateEnvelope {
    let service = app.state::<UpdaterService>().inner().clone();
    let Some((version, pending)) = service.admit_install() else {
        return service.status();
    };
    let admitted = service.status();
    let source = PluginSource { app: app.clone() };
    let gate = IdleGate {
        capture: app.state::<DesktopAudioCapture>().inner().clone(),
        runtime: app.state::<EchoRuntime>().inner().clone(),
        deliveries: transcript_delivery::activity,
    };
    let worker = service.clone();
    let failed_version = version.clone();
    let spawned = std::thread::Builder::new()
        .name("echo-update-install".to_owned())
        .spawn(move || run_install(&worker, &source, &gate, IDLE_POLL, version, pending));
    if let Err(error) = spawned {
        log::warn!("update_install outcome=failed stage=start error={error}");
        service.fail(failed_version);
    }
    admitted
}

pub(crate) fn check_once<S: UpdateSource>(service: &UpdaterService<S::Pending>, source: &S) {
    if service.is_busy() {
        return;
    }
    let outcome = source.check();
    match &outcome {
        CheckOutcome::None => log::info!("update_check outcome=none"),
        CheckOutcome::Available { version, .. } => {
            log::info!("update_check outcome=available version={version}");
        }
        CheckOutcome::Failed(error) => log::warn!("update_check outcome=failed error={error}"),
    }
    service.apply_check(outcome);
}

/// Checks at once, then on every tick at which wall time is a day past the last check.
/// `tick` sleeps for the given time and returns false to stop.
pub(crate) fn run_schedule<S: UpdateSource>(
    service: &UpdaterService<S::Pending>,
    source: &S,
    now: impl Fn() -> SystemTime,
    mut tick: impl FnMut(Duration) -> bool,
) {
    let mut last_check: Option<SystemTime> = None;
    loop {
        let current = now();
        let due = last_check.is_none_or(|last| match current.duration_since(last) {
            Ok(elapsed) => elapsed >= CHECK_INTERVAL,
            // The clock moved back past the last check; waiting for it to catch up could take years.
            Err(_) => true,
        });
        if due {
            check_once(service, source);
            last_check = Some(current);
        }
        if !tick(SCHEDULE_TICK) {
            return;
        }
    }
}

/// Download, then the idle gate, then install. Any error leaves the status failed and
/// capture admission open.
pub(crate) fn run_install<S: UpdateSource>(
    service: &UpdaterService<S::Pending>,
    source: &S,
    gate: &impl InstallGate,
    poll: Duration,
    version: String,
    pending: S::Pending,
) {
    let mut downloaded: u64 = 0;
    let mut published_step: Option<u64> = None;
    let mut on_chunk = |chunk: usize, total: Option<u64>| {
        downloaded = downloaded.saturating_add(chunk as u64);
        // One status change per whole percent, or per MiB when the server sends no length.
        let step = match total {
            Some(total) if total > 0 => downloaded.saturating_mul(100) / total,
            _ => downloaded / MIB,
        };
        if published_step != Some(step) {
            published_step = Some(step);
            service.set_status(UpdateStatus::Downloading {
                version: version.clone(),
                downloaded_bytes: downloaded,
                total_bytes: total,
            });
        }
    };
    let bytes = match source.download(&pending, &mut on_chunk) {
        Ok(bytes) => bytes,
        Err(error) => {
            log::warn!("update_install outcome=failed stage=download error={error}");
            service.fail(version);
            return;
        }
    };
    log::info!("update_install outcome=downloaded version={version}");
    if !gate.try_close() {
        log::info!("update_install outcome=waiting_for_idle version={version}");
        service.set_status(UpdateStatus::WaitingForIdle {
            version: version.clone(),
        });
        loop {
            std::thread::sleep(poll);
            if gate.try_close() {
                break;
            }
        }
    }
    log::info!("update_install outcome=installing version={version}");
    service.set_status(UpdateStatus::Installing {
        version: version.clone(),
    });
    if let Err(error) = source.install(&pending, &bytes) {
        gate.reopen();
        log::warn!("update_install outcome=failed stage=install error={error}");
        service.fail(version);
        return;
    }
    log::info!("update_install outcome=restarting version={version}");
}

/// The one definition of idle for an install: no capture reservation or session, no
/// capture phase other than Idle, no active job, no pending delivery, and the quiet
/// period since the last of those ended.
fn is_idle(capture_reserved: bool, work: Activity, deliveries: Activity, now: Instant) -> bool {
    if capture_reserved {
        return false;
    }
    let (Activity::Quiet(work_ended), Activity::Quiet(delivery_ended)) = (work, deliveries) else {
        return false;
    };
    work_ended
        .max(delivery_ended)
        .is_none_or(|ended| now.saturating_duration_since(ended) >= QUIET_PERIOD)
}

pub(crate) struct IdleGate {
    capture: DesktopAudioCapture,
    runtime: EchoRuntime,
    deliveries: fn() -> Activity,
}

impl IdleGate {
    fn is_idle_at(&self, now: Instant) -> bool {
        let work = self.runtime.activity().unwrap_or(Activity::Running);
        is_idle(
            self.capture.has_reservation(),
            work,
            (self.deliveries)(),
            now,
        )
    }

    /// The first read keeps a busy Echo from refusing captures every poll; the read after
    /// setting the barrier is the one that makes admission atomic.
    fn try_close_at(&self, now: Instant) -> bool {
        if !self.is_idle_at(now) {
            return false;
        }
        self.capture.set_install_barrier(true);
        if self.is_idle_at(now) {
            return true;
        }
        self.capture.set_install_barrier(false);
        false
    }
}

impl InstallGate for IdleGate {
    fn try_close(&self) -> bool {
        self.try_close_at(Instant::now())
    }

    fn reopen(&self) {
        self.capture.set_install_barrier(false);
    }
}

struct PluginSource<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> PluginSource<R> {
    fn updater(&self) -> tauri_plugin_updater::Result<Updater> {
        let https_only = !self.insecure_transport_configured();
        let app = self.app.clone();
        self.app
            .updater_builder()
            .no_proxy()
            .timeout(CHECK_TIMEOUT)
            .configure_client(move |client| {
                client
                    .https_only(https_only)
                    .redirect(updater_reqwest::redirect::Policy::limited(MAX_REDIRECTS))
                    .connect_timeout(CONNECT_TIMEOUT)
            })
            .version_comparator(|current, release| {
                release.version > current && release.version.pre.is_empty()
            })
            .on_before_exit(move || {
                crate::app_setup::shutdown_application(&app);
                app.cleanup_before_exit();
            })
            .build()
    }

    /// Only the staging overlay sets this, to serve a loopback manifest over HTTP.
    fn insecure_transport_configured(&self) -> bool {
        self.app
            .config()
            .plugins
            .0
            .get("updater")
            .and_then(|updater| updater.get("dangerousInsecureTransportProtocol"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }
}

impl<R: Runtime> UpdateSource for PluginSource<R> {
    type Pending = Update;

    fn check(&self) -> CheckOutcome<Update> {
        let checked = self
            .updater()
            .and_then(|updater| tauri::async_runtime::block_on(updater.check()));
        match checked {
            // Every endpoint answered without a manifest, as before the first release carries one.
            Ok(None) | Err(tauri_plugin_updater::Error::ReleaseNotFound) => CheckOutcome::None,
            Ok(Some(update)) => CheckOutcome::Available {
                version: update.version.clone(),
                pending: update,
            },
            Err(error) => CheckOutcome::Failed(error.to_string()),
        }
    }

    fn download(
        &self,
        pending: &Update,
        on_chunk: &mut dyn FnMut(usize, Option<u64>),
    ) -> Result<Vec<u8>, String> {
        let mut update = pending.clone();
        update.timeout = Some(DOWNLOAD_TIMEOUT);
        tauri::async_runtime::block_on(update.download(on_chunk, || {}))
            .map_err(|error| error.to_string())
    }

    fn install(&self, pending: &Update, bytes: &[u8]) -> Result<(), String> {
        pending.install(bytes).map_err(|error| error.to_string())?;
        #[cfg(target_os = "macos")]
        self.app.request_restart();
        Ok(())
    }
}

#[cfg(test)]
#[path = "updater_tests.rs"]
mod tests;
