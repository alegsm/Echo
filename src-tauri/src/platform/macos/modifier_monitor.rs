use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopRunResult};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult, EventField, KeyCode,
};

use super::super::ModifierState;

const MONITOR_READY_TIMEOUT: Duration = Duration::from_secs(3);
const MONITOR_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const MONITOR_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const RUN_LOOP_POLL_INTERVAL: Duration = Duration::from_millis(100);
static FN_EVENT_TAP_AVAILABLE: AtomicBool = AtomicBool::new(false);
static IS_MONITOR_STARTED: AtomicBool = AtomicBool::new(false);
type ModifierHandler = Arc<dyn Fn(bool) + Send + Sync>;
static HANDLER: OnceLock<Mutex<Option<ModifierHandler>>> = OnceLock::new();
static MODIFIER_STATE: OnceLock<Mutex<ModifierState>> = OnceLock::new();

pub fn input_monitoring_permission_status() -> String {
    if FN_EVENT_TAP_AVAILABLE.load(Ordering::SeqCst) {
        "authorized".to_string()
    } else {
        "unknown_or_denied".to_string()
    }
}

pub fn set_fn_hotkey_capture_enabled(enabled: bool) {
    let mut state = modifier_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(is_pressed) = state.set_capture_enabled(enabled) {
        eprintln!("Echo reset Fn hotkey state after capture was disabled.");
        dispatch_modifier_transition(is_pressed);
    }
}

/// Owns the native Fn event tap and stops its run loop when dropped.
pub struct NativeModifierMonitor {
    run_loop: Arc<Mutex<Option<CFRunLoop>>>,
    stop_signal: Arc<StopSignal>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl NativeModifierMonitor {
    pub fn shutdown(&self) {
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(thread) = thread else {
            return;
        };

        set_fn_hotkey_capture_enabled(false);
        self.stop_signal.request();
        if let Some(run_loop) = self
            .run_loop
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            run_loop.stop();
        }
        finish_monitor_thread(thread);
    }
}

impl Drop for NativeModifierMonitor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Default)]
struct StopSignal {
    is_requested: Mutex<bool>,
    wakeup: Condvar,
}

impl StopSignal {
    fn request(&self) {
        *self
            .is_requested
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.wakeup.notify_all();
    }

    fn is_requested(&self) -> bool {
        *self
            .is_requested
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn wait_for_retry(&self) -> bool {
        let requested = self
            .is_requested
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (requested, _) = self
            .wakeup
            .wait_timeout_while(requested, MONITOR_RETRY_INTERVAL, |value| !*value)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *requested
    }
}

pub fn start_modifier_monitor<F>(handler: F) -> Result<NativeModifierMonitor, String>
where
    F: Fn(bool) + Send + Sync + 'static,
{
    if IS_MONITOR_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("The Fn hotkey monitor is already running.".to_string());
    }

    let handler: ModifierHandler = Arc::new(handler);
    *HANDLER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handler);
    let run_loop = Arc::new(Mutex::new(None));
    let stop_signal = Arc::new(StopSignal::default());
    let (ready_sender, ready_receiver) = mpsc::sync_channel(0);
    let thread_run_loop = Arc::clone(&run_loop);
    let thread_stop_signal = Arc::clone(&stop_signal);
    let thread = match std::thread::Builder::new()
        .name("echo-fn-hotkey-monitor".to_string())
        .spawn(move || run_modifier_monitor(thread_run_loop, thread_stop_signal, ready_sender))
    {
        Ok(thread) => thread,
        Err(error) => {
            clear_monitor_state();
            return Err(format!("Could not start Fn hotkey monitor: {error}"));
        }
    };

    if ready_receiver.recv_timeout(MONITOR_READY_TIMEOUT).is_ok() {
        return Ok(NativeModifierMonitor {
            run_loop,
            stop_signal,
            thread: Mutex::new(Some(thread)),
        });
    }

    drop(ready_receiver);
    stop_signal.request();
    if let Some(run_loop) = run_loop
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
    {
        run_loop.stop();
    }
    finish_monitor_thread(thread);
    Err("Timed out while starting the Fn hotkey monitor.".to_string())
}

fn run_modifier_monitor(
    run_loop: Arc<Mutex<Option<CFRunLoop>>>,
    stop_signal: Arc<StopSignal>,
    ready_sender: mpsc::SyncSender<()>,
) {
    let ready_sender = Arc::new(Mutex::new(Some(ready_sender)));
    while !stop_signal.is_requested() {
        let current_run_loop = Arc::clone(&run_loop);
        let callback_stop_signal = Arc::clone(&stop_signal);
        let callback_ready_sender = Arc::clone(&ready_sender);
        let result = CGEventTap::with_enabled(
            CGEventTapLocation::HID,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
            vec![CGEventType::FlagsChanged],
            move |_proxy, event_type, event| handle_event(event_type, event),
            move || {
                let active_run_loop = CFRunLoop::get_current();
                *current_run_loop
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                    Some(active_run_loop.clone());
                FN_EVENT_TAP_AVAILABLE.store(true, Ordering::SeqCst);
                report_monitor_ready(&callback_ready_sender);
                if callback_stop_signal.is_requested() {
                    active_run_loop.stop();
                } else {
                    run_event_loop(&callback_stop_signal);
                }
                FN_EVENT_TAP_AVAILABLE.store(false, Ordering::SeqCst);
                current_run_loop
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take();
            },
        );

        report_monitor_ready(&ready_sender);
        if stop_signal.is_requested() {
            break;
        }
        release_active_modifier();
        log_event_tap_restart(result.is_ok());
        if stop_signal.wait_for_retry() {
            break;
        }
    }
    clear_monitor_state();
}

fn run_event_loop(stop_signal: &StopSignal) {
    while !stop_signal.is_requested() {
        let result = CFRunLoop::run_in_mode(
            // SAFETY: CoreFoundation exports an immutable CFString valid for the process lifetime.
            unsafe { kCFRunLoopDefaultMode },
            RUN_LOOP_POLL_INTERVAL,
            false,
        );
        if matches!(
            result,
            CFRunLoopRunResult::Finished | CFRunLoopRunResult::Stopped
        ) {
            return;
        }
    }
}

fn handle_event(event_type: CGEventType, event: &CGEvent) -> CallbackResult {
    match event_route(event_type) {
        EventRoute::RecoverTap => {
            FN_EVENT_TAP_AVAILABLE.store(false, Ordering::SeqCst);
            release_active_modifier();
            CFRunLoop::get_current().stop();
            return CallbackResult::Keep;
        }
        EventRoute::Ignore => return CallbackResult::Keep,
        EventRoute::ReadFlags => {}
    }

    let key_code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
    if key_code != KeyCode::FUNCTION as i64 {
        return CallbackResult::Keep;
    }

    let is_fn_down = event
        .get_flags()
        .contains(CGEventFlags::CGEventFlagSecondaryFn);
    let mut state = modifier_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let result = state.update_pressed(is_fn_down);
    if let Some(is_pressed) = result.transition {
        dispatch_modifier_transition(is_pressed);
    }
    if result.should_consume {
        CallbackResult::Drop
    } else {
        CallbackResult::Keep
    }
}

fn modifier_state() -> &'static Mutex<ModifierState> {
    MODIFIER_STATE.get_or_init(|| Mutex::new(ModifierState::default()))
}

#[derive(Debug, PartialEq)]
enum EventRoute {
    RecoverTap,
    ReadFlags,
    Ignore,
}

fn event_route(event_type: CGEventType) -> EventRoute {
    match event_type {
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
            EventRoute::RecoverTap
        }
        CGEventType::FlagsChanged => EventRoute::ReadFlags,
        _ => EventRoute::Ignore,
    }
}

fn release_active_modifier() {
    let mut state = modifier_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(is_pressed) = state.release_pressed() {
        dispatch_modifier_transition(is_pressed);
    }
}

fn dispatch_modifier_transition(is_pressed: bool) {
    let handler = HANDLER.get().and_then(|handler_store| {
        handler_store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    });
    if let Some(handler) = handler {
        handler(is_pressed);
    }
}

fn clear_monitor_state() {
    FN_EVENT_TAP_AVAILABLE.store(false, Ordering::SeqCst);
    IS_MONITOR_STARTED.store(false, Ordering::SeqCst);
    if let Some(handler_store) = HANDLER.get() {
        handler_store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }
}

fn report_monitor_ready(sender: &Mutex<Option<mpsc::SyncSender<()>>>) {
    if let Some(sender) = sender
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    {
        let _ = sender.send(());
    }
}

fn log_event_tap_restart(did_stop_after_start: bool) {
    if did_stop_after_start {
        eprintln!("Echo Fn event tap stopped. Retrying.");
        return;
    }
    eprintln!(
        "Echo could not start the Fn key event tap. Enable Accessibility/Input Monitoring for Echo."
    );
}

fn finish_monitor_thread(thread: JoinHandle<()>) {
    let deadline = Instant::now() + MONITOR_SHUTDOWN_TIMEOUT;
    while !thread.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if thread.is_finished() {
        let _ = thread.join();
    } else {
        eprintln!("Echo Fn hotkey monitor did not stop before the shutdown deadline.");
    }
}

#[cfg(test)]
#[path = "modifier_monitor_tests.rs"]
mod tests;
