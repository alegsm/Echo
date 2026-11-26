//! The Control+Super hold-to-talk chord on Linux.
//!
//! A supervisor thread runs one input backend at a time: XInput2 raw key events on X11, or
//! `/dev/input` event devices when X11 is unavailable (Wayland). Neither backend grabs or
//! consumes keys; they only observe them, so the chord still reaches the focused app.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

use super::super::ModifierState;
use super::{evdev, x11, Session};

/// Left and right Control, then left and right Super.
pub(super) type ChordKeys = [bool; 4];

type ModifierHandler = Arc<dyn Fn(bool) + Send + Sync>;
static HANDLER: Mutex<Option<ModifierHandler>> = Mutex::new(None);
static STATE: OnceLock<Mutex<ChordState>> = OnceLock::new();
static AVAILABLE: AtomicBool = AtomicBool::new(false);
static IS_MONITOR_STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct ChordState {
    keys: ChordKeys,
    chord: ModifierState,
}

impl ChordState {
    fn apply(&mut self, keys: ChordKeys) -> Option<bool> {
        self.keys = keys;
        let pressed = (keys[0] || keys[1]) && (keys[2] || keys[3]);
        self.chord.update_pressed(pressed).transition
    }

    fn reset(&mut self) -> Option<bool> {
        self.keys = [false; 4];
        self.chord.release_pressed()
    }
}

fn state() -> &'static Mutex<ChordState> {
    STATE.get_or_init(|| Mutex::new(ChordState::default()))
}

fn dispatch(transition: Option<bool>) {
    let Some(is_pressed) = transition else {
        return;
    };
    let handler = HANDLER.lock().unwrap_or_else(|p| p.into_inner()).clone();
    if let Some(handler) = handler {
        handler(is_pressed);
    }
}

/// Backends call this with the full chord key state after every relevant key edge.
pub(super) fn report(keys: ChordKeys) {
    let transition = state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .apply(keys);
    dispatch(transition);
}

pub fn native_modifier_available() -> bool {
    AVAILABLE.load(Ordering::SeqCst)
}

pub fn set_fn_hotkey_capture_enabled(enabled: bool) {
    let transition = state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .chord
        .set_capture_enabled(enabled && native_modifier_available());
    dispatch(transition);
}

pub struct NativeModifierMonitor {
    stop: Arc<AtomicBool>,
    wake: OwnedFd,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl NativeModifierMonitor {
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let thread = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(thread) = thread {
            // SAFETY: wake is this monitor's open pipe write end and the byte is valid memory.
            unsafe {
                libc::write(self.wake.as_raw_fd(), [1u8].as_ptr().cast(), 1);
            }
            let _ = thread.join();
        }
    }
}

impl Drop for NativeModifierMonitor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn start_modifier_monitor<F, H>(handler: F, health: H) -> Result<NativeModifierMonitor, String>
where
    F: Fn(bool) + Send + Sync + 'static,
    H: Fn(bool) + Send + Sync + 'static,
{
    if IS_MONITOR_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("The Linux hotkey monitor is already running.".to_string());
    }
    *HANDLER.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(handler));
    let (wake_read, wake_write) = match pipe() {
        Ok(pipe) => pipe,
        Err(error) => {
            IS_MONITOR_STARTED.store(false, Ordering::SeqCst);
            return Err(error);
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("echo-hotkey-monitor".into())
        .spawn(move || {
            supervise(wake_read.as_raw_fd(), &stopping, &health);
            drop(wake_read);
        })
        .map_err(|error| {
            IS_MONITOR_STARTED.store(false, Ordering::SeqCst);
            format!("Could not start the Linux hotkey monitor: {error}")
        })?;
    Ok(NativeModifierMonitor {
        stop,
        wake: wake_write,
        thread: Mutex::new(Some(thread)),
    })
}

fn supervise(wake: RawFd, stop: &AtomicBool, health: &dyn Fn(bool)) {
    let ready = || {
        AVAILABLE.store(true, Ordering::SeqCst);
        health(true);
    };
    let mut errors = Vec::new();
    // XWayland only reports keys typed into X11 windows, so Wayland goes straight to evdev.
    if super::session() == Session::X11 {
        match x11::run_hotkeys(wake, &ready) {
            Ok(()) => return finish(health, None),
            Err(error) => errors.push(format!("X11: {error}")),
        }
    }
    if !stop.load(Ordering::SeqCst) {
        match evdev::run_hotkeys(wake, &ready) {
            Ok(()) => return finish(health, None),
            Err(error) => errors.push(format!("input devices: {error}")),
        }
    }
    finish(health, Some(errors.join("; ")));
}

fn finish(health: &dyn Fn(bool), error: Option<String>) {
    let was_available = AVAILABLE.swap(false, Ordering::SeqCst);
    let release = state().lock().unwrap_or_else(|p| p.into_inner()).reset();
    dispatch(release);
    if let Some(error) = error {
        log::warn!("Linux hotkey monitor unavailable: {error}");
        health(false);
    } else if was_available {
        health(false);
    }
    *HANDLER.lock().unwrap_or_else(|p| p.into_inner()) = None;
    IS_MONITOR_STARTED.store(false, Ordering::SeqCst);
}

fn pipe() -> Result<(OwnedFd, OwnedFd), String> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: fds is valid writable storage for the two descriptors pipe2 returns.
    let result = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
    if result != 0 {
        return Err(format!(
            "Could not create the hotkey wake pipe: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: pipe2 succeeded, so both descriptors are open and owned only by this function.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Waits until any descriptor is readable, or the timeout passes. Returns readability per fd.
pub(super) fn wait_readable(fds: &[RawFd], timeout_ms: i32) -> std::io::Result<Vec<bool>> {
    let mut polls: Vec<libc::pollfd> = fds
        .iter()
        .map(|&fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    loop {
        // SAFETY: polls is a live, correctly sized pollfd array for this synchronous call.
        let result =
            unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, timeout_ms) };
        if result >= 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(polls
        .iter()
        .map(|poll| {
            poll.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::ChordState;

    #[test]
    fn either_control_with_either_super_is_the_chord() {
        let mut state = ChordState::default();
        state.chord.set_capture_enabled(true);
        assert_eq!(state.apply([true, false, false, false]), None);
        assert_eq!(state.apply([true, false, false, true]), Some(true));
        assert_eq!(state.apply([true, true, false, true]), None);
        assert_eq!(state.apply([false, true, false, true]), None);
        assert_eq!(state.apply([false, true, false, false]), Some(false));
        assert_eq!(state.apply([false, true, true, false]), Some(true));
    }

    #[test]
    fn chord_is_ignored_while_capture_is_disabled_and_reset_releases() {
        let mut state = ChordState::default();
        assert_eq!(state.apply([true, false, true, false]), None);
        state.chord.set_capture_enabled(true);
        assert_eq!(state.apply([true, false, true, false]), Some(true));
        assert_eq!(state.reset(), Some(false));
        assert_eq!(state.reset(), None);
    }
}
