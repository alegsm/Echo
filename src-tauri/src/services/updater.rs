//! In-app updates. Rust owns every update request: it checks the configured endpoints at
//! launch and once a day. The dashboard reads the status and asks for the update through
//! `commands/update.rs`; the tray offers the same request, and both land in
//! [`request_update`]. What a request does depends on the build. Where build.rs sets
//! `echo_update_install` (macOS), the `install` module downloads and verifies the release,
//! waits until Echo is idle, installs it and restarts. Every other build (Windows, until its
//! builds are code-signed) downloads and runs nothing: it reads only the manifest's version
//! and opens [`DOWNLOAD_PAGE_URL`] in the browser.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::services::background_mode;

#[cfg(echo_update_install)]
mod install;
#[cfg(echo_update_install)]
pub use install::ProductionPending;
/// What an offer carries on builds that only open the download page: nothing.
#[cfg(not(echo_update_install))]
pub type ProductionPending = ();

/// The one page a notify build opens. Compiled into Echo, never read from a manifest.
#[cfg_attr(echo_update_install, allow(dead_code))]
pub const DOWNLOAD_PAGE_URL: &str = "https://github.com/alegsm/Echo/releases/latest";

/// Emitted to the dashboard with an [`UpdateEnvelope`] on every status change.
pub const STATUS_EVENT: &str = "echo-update-status";
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Monotonic sleep pauses while the system sleeps, so each tick re-reads wall time
/// instead of sleeping for the whole interval.
const SCHEDULE_TICK: Duration = Duration::from_secs(5 * 60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REDIRECTS: usize = 5;

/// The wire status. `version` is the release's semver without a leading `v`. Builds that
/// install use `available` through `failed`; notify builds use `download_page` and
/// `download_page_failed`, the latter after the browser could not be opened.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum UpdateStatus {
    None,
    #[cfg_attr(not(echo_update_install), allow(dead_code))]
    Available {
        version: String,
    },
    #[cfg_attr(not(echo_update_install), allow(dead_code))]
    Downloading {
        version: String,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    #[cfg_attr(not(echo_update_install), allow(dead_code))]
    WaitingForIdle {
        version: String,
    },
    #[cfg_attr(not(echo_update_install), allow(dead_code))]
    Installing {
        version: String,
    },
    #[cfg_attr(not(echo_update_install), allow(dead_code))]
    Failed {
        version: String,
    },
    #[cfg_attr(echo_update_install, allow(dead_code))]
    DownloadPage {
        version: String,
    },
    #[cfg_attr(echo_update_install, allow(dead_code))]
    DownloadPageFailed {
        version: String,
    },
}

/// `sequence` rises by one on every status change, so the dashboard can drop stale events.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UpdateEnvelope {
    pub sequence: u64,
    pub status: UpdateStatus,
}

/// How the tray shows the update item: Update Echo (enabled or disabled) on builds that
/// install, Download Echo Update on notify builds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrayOffer {
    Hidden,
    Enabled,
    Disabled,
    DownloadPage,
}

pub(crate) enum CheckOutcome<P> {
    None,
    Available { version: String, pending: P },
    Failed(String),
}

/// Where releases come from. Builds that install use the Tauri updater plugin, notify
/// builds `ManifestSource`; tests use fakes. Only builds that install can download.
pub(crate) trait UpdateSource {
    type Pending: Clone;
    fn check(&self) -> CheckOutcome<Self::Pending>;
    /// Returns the artifact only after its signature verified against the configured key.
    #[cfg(echo_update_install)]
    fn download(
        &self,
        pending: &Self::Pending,
        on_chunk: &mut dyn FnMut(usize, Option<u64>),
    ) -> Result<Vec<u8>, String>;
    /// Installs and restarts Echo. On Windows the installer exits the process instead of returning.
    #[cfg(echo_update_install)]
    fn install(&self, pending: &Self::Pending, bytes: &[u8]) -> Result<(), String>;
}

pub struct UpdaterService<P = ProductionPending> {
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
    #[cfg_attr(not(echo_update_install), allow(dead_code))]
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
            UpdateStatus::DownloadPage { .. } | UpdateStatus::DownloadPageFailed { .. } => {
                TrayOffer::DownloadPage
            }
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
                    state.change(offer(version))
                }
            }
        };
        self.publish(changed);
    }

    #[cfg_attr(not(echo_update_install), allow(dead_code))]
    fn set_status(&self, status: UpdateStatus) {
        let changed = self.lock().change(status);
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
    app.manage(UpdaterService::<ProductionPending>::new(move |envelope| {
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
    // No Linux release is published, so a check would offer a page with nothing to download.
    if cfg!(target_os = "linux") {
        return;
    }
    let service = app.state::<UpdaterService>().inner().clone();
    #[cfg(echo_update_install)]
    let source = install::PluginSource::new(app.clone());
    #[cfg(not(echo_update_install))]
    let source = ManifestSource::new(app.clone());
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

/// The sidebar button and the tray item both land here. Builds that install start the
/// download and return once it is admitted or ignored; notify builds open the download page.
pub fn request_update<R: Runtime>(app: &AppHandle<R>) -> UpdateEnvelope {
    #[cfg(echo_update_install)]
    return install::request_install(app);
    #[cfg(not(echo_update_install))]
    request_download_page(app)
}

/// The status a check offers: an install where Echo installs, the download page elsewhere.
fn offer(version: String) -> UpdateStatus {
    #[cfg(echo_update_install)]
    return UpdateStatus::Available { version };
    #[cfg(not(echo_update_install))]
    UpdateStatus::DownloadPage { version }
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

/// Opens the download page in the browser through the platform shell.
#[cfg(not(echo_update_install))]
fn request_download_page<R: Runtime>(app: &AppHandle<R>) -> UpdateEnvelope {
    let service = app.state::<UpdaterService>().inner().clone();
    open_download_page(&service, crate::platform::open_url)
}

/// Opens [`DOWNLOAD_PAGE_URL`], and nothing else, from `download_page` or
/// `download_page_failed`; any other state returns the status unchanged without calling
/// `open`. A failure to open reads `download_page_failed`, so the dashboard shows the address.
#[cfg(any(test, not(echo_update_install)))]
pub(crate) fn open_download_page<P: Clone>(
    service: &UpdaterService<P>,
    open: impl FnOnce(&str) -> Result<(), String>,
) -> UpdateEnvelope {
    let offered = match &service.lock().status {
        UpdateStatus::DownloadPage { version } | UpdateStatus::DownloadPageFailed { version } => {
            Some(version.clone())
        }
        _ => None,
    };
    let Some(version) = offered else {
        return service.status();
    };
    // The browser opens outside the lock, so a check landing meanwhile is not blocked on it.
    let status = match open(DOWNLOAD_PAGE_URL) {
        Ok(()) => {
            log::info!("update_open outcome=opened version={version}");
            UpdateStatus::DownloadPage { version }
        }
        Err(error) => {
            log::warn!("update_open outcome=failed error={error}");
            UpdateStatus::DownloadPageFailed { version }
        }
    };
    let changed = {
        let mut state = service.lock();
        // A check that replaced the offer meanwhile wins over this result.
        let still_offered = match (&state.status, &status) {
            (
                UpdateStatus::DownloadPage { version: current }
                | UpdateStatus::DownloadPageFailed { version: current },
                UpdateStatus::DownloadPage { version }
                | UpdateStatus::DownloadPageFailed { version },
            ) => current == version,
            _ => false,
        };
        if still_offered {
            state.change(status)
        } else {
            None
        }
    };
    service.publish(changed);
    service.status()
}

/// The most of a manifest body a notify build reads.
#[cfg(any(test, not(echo_update_install)))]
const MANIFEST_LIMIT: usize = 64 * 1024;

/// The release manifest as a notify build reads it: only the version.
#[cfg(any(test, not(echo_update_install)))]
#[derive(serde::Deserialize)]
struct ManifestVersion {
    version: String,
}

/// The check on notify builds. It reads the same `plugins.updater` endpoints as the plugin,
/// in order and under the same client rules, but takes only `version` from the manifest.
/// Nothing is downloaded, so no signature is verified; an offer can only open
/// [`DOWNLOAD_PAGE_URL`].
#[cfg(any(test, not(echo_update_install)))]
pub(crate) struct ManifestSource<R: Runtime> {
    app: AppHandle<R>,
    /// The limit on a whole check request. Tests shorten it to exercise a server that stalls.
    check_timeout: Duration,
}

#[cfg(any(test, not(echo_update_install)))]
impl<R: Runtime> ManifestSource<R> {
    pub(crate) fn new(app: AppHandle<R>) -> Self {
        Self {
            app,
            check_timeout: CHECK_TIMEOUT,
        }
    }

    fn updater_config(&self, key: &str) -> Option<serde_json::Value> {
        self.app
            .config()
            .plugins
            .0
            .get("updater")
            .and_then(|updater| updater.get(key))
            .cloned()
    }

    /// The configured endpoints in order. Like the plugin, a missing list, an empty list or
    /// an endpoint that is not a URL fails the whole check.
    fn endpoints(&self) -> Result<Vec<reqwest::Url>, String> {
        let configured = self.updater_config("endpoints");
        let endpoints = configured
            .as_ref()
            .and_then(serde_json::Value::as_array)
            .filter(|endpoints| !endpoints.is_empty())
            .ok_or("no update endpoints configured")?;
        endpoints
            .iter()
            .map(|endpoint| {
                let endpoint = endpoint.as_str().ok_or("update endpoint is not a string")?;
                reqwest::Url::parse(endpoint).map_err(|error| format!("update endpoint: {error}"))
            })
            .collect()
    }

    /// Only the staging overlay sets this, to serve a loopback manifest over HTTP.
    fn insecure_transport_configured(&self) -> bool {
        self.updater_config("dangerousInsecureTransportProtocol")
            .as_ref()
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }

    /// The plugin's client rules: no proxy, HTTPS only, at most five redirects, 10-second
    /// connect and `check_timeout` request limits.
    fn client(&self) -> reqwest::Result<reqwest::Client> {
        reqwest::Client::builder()
            .no_proxy()
            .https_only(!self.insecure_transport_configured())
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(self.check_timeout)
            .user_agent("Echo")
            .build()
    }
}

#[cfg(any(test, not(echo_update_install)))]
impl<R: Runtime> UpdateSource for ManifestSource<R> {
    type Pending = ();

    fn check(&self) -> CheckOutcome<()> {
        let endpoints = match self.endpoints() {
            Ok(endpoints) => endpoints,
            Err(error) => return CheckOutcome::Failed(error),
        };
        let client = match self.client() {
            Ok(client) => client,
            Err(error) => return CheckOutcome::Failed(error.to_string()),
        };
        let version = &self.app.package_info().version;
        let current = (version.major, version.minor, version.patch);
        tauri::async_runtime::block_on(check_endpoints(&client, &endpoints, current))
    }

    // Install builds compile this source only for tests. A manifest offer carries no
    // artifact, so there is nothing to download or install.
    #[cfg(echo_update_install)]
    fn download(
        &self,
        _pending: &(),
        _on_chunk: &mut dyn FnMut(usize, Option<u64>),
    ) -> Result<Vec<u8>, String> {
        Err("a manifest offer has no artifact".into())
    }

    #[cfg(echo_update_install)]
    fn install(&self, _pending: &(), _bytes: &[u8]) -> Result<(), String> {
        Err("a manifest offer has no artifact".into())
    }
}

/// Walks the endpoints as the plugin does: 204 means no release; any other non-2xx moves on
/// without an error; a request, size, JSON or version error is kept and moves on; the first
/// readable manifest stops the walk and clears any kept error. A kept error fails the check.
#[cfg(any(test, not(echo_update_install)))]
async fn check_endpoints(
    client: &reqwest::Client,
    endpoints: &[reqwest::Url],
    current: (u64, u64, u64),
) -> CheckOutcome<()> {
    let mut last_error: Option<String> = None;
    let mut release: Option<(String, bool)> = None;
    for endpoint in endpoints {
        let response = client
            .get(endpoint.clone())
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                last_error = Some(request_error(endpoint, error));
                continue;
            }
        };
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return CheckOutcome::None;
        }
        if !response.status().is_success() {
            continue;
        }
        match read_release(endpoint, response, current).await {
            Ok(found) => {
                last_error = None;
                release = Some(found);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    if let Some(error) = last_error {
        return CheckOutcome::Failed(error);
    }
    match release {
        Some((version, true)) => CheckOutcome::Available {
            version,
            pending: (),
        },
        _ => CheckOutcome::None,
    }
}

/// A request error without the URL reqwest attaches, which after a redirect is the server's
/// `Location`. The configured endpoint names the request instead.
#[cfg(any(test, not(echo_update_install)))]
fn request_error(endpoint: &reqwest::Url, error: reqwest::Error) -> String {
    format!("{endpoint}: {}", error.without_url())
}

/// Reads at most [`MANIFEST_LIMIT`] bytes and returns the canonical version and whether it
/// is offered. No error carries text from the body.
#[cfg(any(test, not(echo_update_install)))]
async fn read_release(
    endpoint: &reqwest::Url,
    mut response: reqwest::Response,
    current: (u64, u64, u64),
) -> Result<(String, bool), String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| request_error(endpoint, error))?
    {
        if body.len() + chunk.len() > MANIFEST_LIMIT {
            return Err(format!(
                "{endpoint}: manifest larger than {MANIFEST_LIMIT} bytes"
            ));
        }
        body.extend_from_slice(&chunk);
    }
    let manifest: ManifestVersion = serde_json::from_slice(&body).map_err(|error| {
        format!(
            "{endpoint}: manifest has no readable version ({:?} error at line {} column {})",
            error.classify(),
            error.line(),
            error.column()
        )
    })?;
    newer_stable(current, &manifest.version).map_err(|error| format!("{endpoint}: {error}"))
}

/// The longest release version a notify build accepts; longer ones fail the check.
#[cfg(any(test, not(echo_update_install)))]
const VERSION_LIMIT: usize = 128;

/// The plugin's semver rules without semver: after one optional leading `v`, exactly
/// `major.minor.patch` of numbers without leading zeros, then an optional `-prerelease` and
/// an optional `+build`, each dot-separated identifiers of ASCII letters, digits and `-`.
/// Anything else is an error. Returns the canonical `major.minor.patch`, rebuilt from the
/// numbers so no server text leaves the check, and whether the release is offered: higher
/// than `current` and not a prerelease. Build metadata is dropped.
#[cfg(any(test, not(echo_update_install)))]
fn newer_stable(current: (u64, u64, u64), release: &str) -> Result<(String, bool), String> {
    let invalid = || "release version is not valid semver".to_owned();
    if release.len() > VERSION_LIMIT {
        return Err(invalid());
    }
    let release = release.strip_prefix('v').unwrap_or(release);
    let (release, build) = match release.split_once('+') {
        Some((release, build)) => (release, Some(build)),
        None => (release, None),
    };
    let (core, prerelease) = match release.split_once('-') {
        Some((core, prerelease)) => (core, Some(prerelease)),
        None => (release, None),
    };
    if !build.is_none_or(identifiers) || !prerelease.is_none_or(identifiers) {
        return Err(invalid());
    }
    let numbers = core
        .split('.')
        .map(version_number)
        .collect::<Option<Vec<u64>>>()
        .ok_or_else(invalid)?;
    let [major, minor, patch] = numbers[..] else {
        return Err(invalid());
    };
    let offered = prerelease.is_none() && (major, minor, patch) > current;
    Ok((format!("{major}.{minor}.{patch}"), offered))
}

/// A non-empty dot-separated list of non-empty identifiers of ASCII letters, digits and `-`.
#[cfg(any(test, not(echo_update_install)))]
fn identifiers(list: &str) -> bool {
    list.split('.').all(|identifier| {
        !identifier.is_empty()
            && identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    })
}

#[cfg(any(test, not(echo_update_install)))]
fn version_number(part: &str) -> Option<u64> {
    let digits = !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let leading_zero = part.len() > 1 && part.starts_with('0');
    if !digits || leading_zero {
        return None;
    }
    part.parse().ok()
}

#[cfg(test)]
#[path = "updater_tests.rs"]
mod tests;
