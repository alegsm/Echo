use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetLastInputInfo, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYEVENTF_KEYUP, LASTINPUTINFO,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer,
    PeekMessageW, PostThreadMessageW, SetTimer, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, HC_ACTION, HWND_MESSAGE, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED,
    MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_KEYDOWN, WM_KEYUP,
    WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_WTSSESSION_CHANGE,
};

// These small API boundaries avoid enabling unrelated windows-sys feature groups.
#[link(name = "wtsapi32")]
unsafe extern "system" {
    fn WTSRegisterSessionNotification(window: HWND, flags: u32) -> i32;
    fn WTSUnRegisterSessionNotification(window: HWND) -> i32;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn OpenInputDesktop(flags: u32, inherit: i32, access: u32) -> HANDLE;
    fn GetUserObjectInformationW(
        object: HANDLE,
        index: i32,
        info: *mut std::ffi::c_void,
        length: u32,
        needed: *mut u32,
    ) -> i32;
    fn CloseDesktop(desktop: HANDLE) -> i32;
}

pub(super) mod clipboard;
pub(super) mod window_icon;
use super::windows_hook_health::{HookHealth, HookObservation, HookPlatform};
use super::windows_input;

use super::windows_hotkey_reducer::{HotkeyReducer, KeyInput};

const MONITOR_READY_TIMEOUT: Duration = Duration::from_secs(3);
const MONITOR_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
static WINDOWS_HOTKEY_HOOK_AVAILABLE: AtomicBool = AtomicBool::new(false);
static LAST_CALLBACK_TICK: AtomicU32 = AtomicU32::new(0);
static SESSION_INACTIVE: AtomicBool = AtomicBool::new(false);
static UNLOCK_GENERATION: AtomicU32 = AtomicU32::new(0);
static IS_MONITOR_STARTED: AtomicBool = AtomicBool::new(false);
type ModifierHandler = Arc<dyn Fn(bool) + Send + Sync>;
static HANDLER: OnceLock<Mutex<Option<ModifierHandler>>> = OnceLock::new();
static MODIFIER_STATE: OnceLock<Mutex<HotkeyReducer>> = OnceLock::new();

pub fn microphone_permission_status() -> String {
    super::windows_microphone::permission_status()
}

pub fn request_microphone_permission() -> Result<String, String> {
    Ok(microphone_permission_status())
}

pub fn paste_clipboard() -> Result<(), String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, KEYEVENTF_EXTENDEDKEY,
    };
    let held: Vec<_> = windows_input::MODIFIERS
        .iter()
        .copied()
        .filter(|&key| {
            // SAFETY: each value is a valid left/right modifier virtual key.
            (unsafe { GetAsyncKeyState(i32::from(key)) }) < 0
        })
        .collect();
    windows_input::insert(&held, |batch| {
        let inputs: Vec<_> = batch
            .iter()
            .map(|event| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: event.key,
                        wScan: 0,
                        dwFlags: if event.up { KEYEVENTF_KEYUP } else { 0 }
                            | if matches!(event.key, 0xA3 | 0xA5 | 0x5B | 0x5C) {
                                KEYEVENTF_EXTENDEDKEY
                            } else {
                                0
                            },
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            })
            .collect();
        // SAFETY: every initialized INPUT stays valid for this synchronous call.
        unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            ) as usize
        }
    })
    .map_err(|()| {
        "Echo could not synthesize Ctrl+V on Windows. Another process may be blocking input."
            .to_string()
    })
}

pub fn accessibility_permission_status() -> String {
    "not_required".to_string()
}

pub fn request_accessibility_permission() -> String {
    accessibility_permission_status()
}

pub fn input_monitoring_permission_status() -> String {
    "not_required".to_string()
}

pub fn open_privacy_settings(kind: &str) -> Result<(), String> {
    match kind {
        "microphone" => super::windows_microphone::open_settings(),
        _ => Err(format!(
            "Windows privacy settings are unavailable for {kind}."
        )),
    }
}

pub fn set_fn_hotkey_capture_enabled(enabled: bool) {
    let transition = modifier_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .set_capture_enabled(enabled && native_modifier_available());
    if let Some(is_pressed) = transition {
        dispatch_modifier_transition(is_pressed);
    }
}

pub fn native_modifier_available() -> bool {
    WINDOWS_HOTKEY_HOOK_AVAILABLE.load(Ordering::SeqCst)
}

/// Owns a supervisor; hook workers are replaced only after their predecessor exits.
pub struct NativeModifierMonitor {
    stop: Arc<AtomicBool>,
    wake: SupervisorWake,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl NativeModifierMonitor {
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let thread = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(thread) = thread {
            self.wake.post(WM_APP);
            finish_monitor_thread(thread);
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
        return Err("The Windows hotkey monitor is already running.".to_string());
    }
    *HANDLER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(handler));
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = Arc::clone(&stop);
    let wake = SupervisorWake(Arc::new(AtomicU32::new(0)));
    let worker_wake = wake.clone();
    let thread = std::thread::Builder::new()
        .name("echo-hook-supervisor".into())
        .spawn(move || {
            let notifications = SessionNotifications::new();
            worker_wake
                .0
                .store(current_message_thread(), Ordering::SeqCst);
            let mut platform = WindowsHook {
                worker: None,
                health,
                wake: worker_wake,
            };
            let mut policy = HookHealth::default();
            let start = Instant::now();
            // SAFETY: Zero is valid for every MSG field.
            let mut message = unsafe { std::mem::zeroed::<MSG>() };
            while !stopping.load(Ordering::SeqCst) {
                policy.tick(start.elapsed().as_millis() as u64, &mut platform);
                let delay = policy.next_delay_ms(start.elapsed().as_millis() as u64) as u32;
                // SAFETY: a thread timer with no callback delivers WM_TIMER to this queue.
                let timer = unsafe { SetTimer(null_mut(), 0, delay, None) };
                if timer == 0 {
                    break;
                }
                // SAFETY: message is valid writable storage; this blocks until a message or timer.
                let received = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
                // SAFETY: this supervisor owns the timer and no longer needs it.
                unsafe {
                    KillTimer(null_mut(), timer);
                }
                if received <= 0 || message.message == WM_APP {
                    break;
                }
                if message.message == WM_WTSSESSION_CHANGE {
                    match message.wParam {
                        // WTS console/remote disconnect, logoff and lock.
                        2 | 4 | 6 | 7 => SESSION_INACTIVE.store(true, Ordering::SeqCst),
                        // WTS console/remote connect, logon and unlock.
                        1 | 3 | 5 | 8 => {
                            SESSION_INACTIVE.store(false, Ordering::SeqCst);
                            UNLOCK_GENERATION.fetch_add(1, Ordering::SeqCst);
                        }
                        _ => {}
                    }
                }
                // SAFETY: GetMessage initialized message; both calls borrow its live storage.
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            platform.wake.0.store(0, Ordering::SeqCst);
            drop(notifications);
            platform.lifecycle(false);
            platform.stop();
            if let Some(worker) = platform.worker.take() {
                finish_monitor_thread(worker.thread);
            }
            clear_monitor_state();
        });
    match thread {
        Ok(thread) => Ok(NativeModifierMonitor {
            stop,
            wake,
            thread: Mutex::new(Some(thread)),
        }),
        Err(error) => {
            clear_monitor_state();
            Err(format!("Could not start Windows hotkey monitor: {error}"))
        }
    }
}

#[derive(Clone)]
struct SupervisorWake(Arc<AtomicU32>);

impl SupervisorWake {
    fn post(&self, message: u32) {
        let id = self.0.load(Ordering::SeqCst);
        if id != 0 {
            // SAFETY: the supervisor publishes its ID only after creating its message queue.
            unsafe {
                PostThreadMessageW(id, message, 0, 0);
            }
        }
    }
}

struct HookWorker {
    stop: Arc<AtomicBool>,
    thread_id: Arc<AtomicU32>,
    thread: JoinHandle<()>,
}

struct WindowsHook<H> {
    worker: Option<HookWorker>,
    health: H,
    wake: SupervisorWake,
}

impl<H: Fn(bool)> HookPlatform for WindowsHook<H> {
    fn install(&mut self) -> bool {
        self.stop();
        if self.worker.is_some() {
            return false;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let thread_id = Arc::new(AtomicU32::new(0));
        let worker_id = Arc::clone(&thread_id);
        let wake = self.wake.clone();
        let (ready_sender, ready_receiver) = std::sync::mpsc::sync_channel(1);
        let thread = match std::thread::Builder::new()
            .name("echo-windows-hotkey-monitor".into())
            .spawn(move || {
                run_modifier_monitor(ready_sender, stopping, Arc::clone(&worker_id));
                worker_id.store(0, Ordering::SeqCst);
                wake.post(WM_APP + 1);
            }) {
            Ok(thread) => thread,
            Err(_) => return false,
        };
        self.worker = Some(HookWorker {
            stop,
            thread_id,
            thread,
        });
        // Installation health depends only on hook registration, never synthesized input.
        ready_receiver.recv_timeout(MONITOR_READY_TIMEOUT) == Ok(true)
    }

    fn observe(&mut self) -> HookObservation {
        HookObservation {
            alive: self
                .worker
                .as_ref()
                .is_some_and(|worker| !worker.thread.is_finished()),
            session_active: !SESSION_INACTIVE.load(Ordering::SeqCst) && input_desktop_is_active(),
            unlock_generation: UNLOCK_GENERATION.load(Ordering::SeqCst),
            last_input_tick: last_input_tick(),
            last_callback_tick: LAST_CALLBACK_TICK.load(Ordering::SeqCst),
        }
    }

    fn stop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.stop.store(true, Ordering::SeqCst);
            if !worker.thread.is_finished() {
                let thread_id = worker.thread_id.load(Ordering::SeqCst);
                if thread_id != 0 {
                    // SAFETY: the worker creates its message queue before publishing this ID.
                    unsafe {
                        PostThreadMessageW(thread_id, WM_QUIT, 0, 0);
                    }
                }
                return;
            }
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.thread.join();
        }
    }

    fn resync(&mut self) {
        let release = {
            // Keep callbacks from applying newer edges between sampling and resync.
            let mut state = modifier_state().lock().unwrap_or_else(|p| p.into_inner());
            let snapshot = windows_input::MODIFIERS.map(|key| {
                // SAFETY: key is one of the eight valid side-specific modifier virtual keys.
                unsafe { GetAsyncKeyState(i32::from(key)) < 0 }
            });
            state.resync(snapshot)
        };
        if let Some(pressed) = release {
            dispatch_modifier_transition(pressed);
        }
    }

    fn lifecycle(&mut self, available: bool) {
        WINDOWS_HOTKEY_HOOK_AVAILABLE.store(available, Ordering::SeqCst);
        if !available {
            set_fn_hotkey_capture_enabled(false);
        }
        (self.health)(available);
    }
}

fn last_input_tick() -> Option<u32> {
    let mut input = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: input is initialized with the API's required size and writable storage.
    (unsafe { GetLastInputInfo(&mut input) } != 0).then_some(input.dwTime)
}

fn input_desktop_is_active() -> bool {
    // DESKTOP_READOBJECTS permits reading the name without switching desktops.
    // SAFETY: no pointers are passed; the returned desktop handle is closed below.
    let desktop = unsafe { OpenInputDesktop(0, 0, 0x0001) };
    if desktop.is_null() {
        return false;
    }
    let mut name = [0u16; 64];
    let mut needed = 0;
    // SAFETY: name and needed are valid output buffers; UOI_NAME is 2.
    let read = unsafe {
        GetUserObjectInformationW(
            desktop,
            2,
            name.as_mut_ptr().cast(),
            std::mem::size_of_val(&name) as u32,
            &mut needed,
        )
    };
    // SAFETY: this function exclusively owns the handle returned by OpenInputDesktop.
    unsafe {
        CloseDesktop(desktop);
    }
    read != 0 && name[..8] == [68, 101, 102, 97, 117, 108, 116, 0]
}

fn current_message_thread() -> u32 {
    // SAFETY: Zero is valid for MSG and PeekMessage creates the calling thread's queue.
    let mut message = unsafe { std::mem::zeroed::<MSG>() };
    // SAFETY: message is writable storage and the remaining arguments are valid filters.
    unsafe {
        PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
    }
    // SAFETY: GetCurrentThreadId takes no arguments.
    unsafe { GetCurrentThreadId() }
}

struct SessionNotifications {
    window: HWND,
    registered: bool,
}

impl SessionNotifications {
    fn new() -> Self {
        // Built-in STATIC window class.
        let class = [83u16, 84, 65, 84, 73, 67, 0];
        // SAFETY: class is nul-terminated; this thread owns the hidden message-only window.
        let window = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                null_mut(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        // SAFETY: registration borrows a live window; flag 0 limits events to this session.
        let registered =
            !window.is_null() && unsafe { WTSRegisterSessionNotification(window, 0) } != 0;
        if !registered {
            log::warn!("Windows session notifications unavailable; desktop transitions will be checked on the health timer.");
        }
        Self { window, registered }
    }
}

impl Drop for SessionNotifications {
    fn drop(&mut self) {
        if self.registered {
            // SAFETY: unregister before destroying the live window owned by this thread.
            unsafe {
                WTSUnRegisterSessionNotification(self.window);
            }
        }
        if !self.window.is_null() {
            // SAFETY: the message-only window was created on this thread and is no longer used.
            unsafe {
                DestroyWindow(self.window);
            }
        }
    }
}

fn run_modifier_monitor(
    ready: mpsc::SyncSender<bool>,
    stop: Arc<AtomicBool>,
    thread_id: Arc<AtomicU32>,
) {
    thread_id.store(current_message_thread(), Ordering::SeqCst);
    LAST_CALLBACK_TICK.store(last_input_tick().unwrap_or(0), Ordering::SeqCst);
    // SAFETY: callbacks have the required ABI and remain live until unhooking below.
    let keyboard = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), null_mut(), 0) };
    // Observe mouse activity too, so mouse-only use cannot imply a dead keyboard hook.
    // SAFETY: same lifetime and callback guarantees as the keyboard hook.
    let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), null_mut(), 0) };
    let installed = !keyboard.is_null() && !mouse.is_null();
    let _ = ready.send(installed);
    if installed {
        // SAFETY: Zero is valid for every MSG field.
        let mut message = unsafe { std::mem::zeroed::<MSG>() };
        while !stop.load(Ordering::SeqCst) {
            // SAFETY: message is writable storage. GetMessage blocks until input or WM_QUIT.
            if unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } <= 0 {
                break;
            }
            // SAFETY: GetMessage initialized message; both calls borrow its live storage.
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
    for hook in [keyboard, mouse] {
        if !hook.is_null() {
            // SAFETY: this worker owns both registrations, even if Windows already removed one.
            unsafe {
                UnhookWindowsHookEx(hook);
            }
        }
    }
}

unsafe extern "system" fn mouse_proc(code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: Windows supplies a live MSLLHOOKSTRUCT for an HC_ACTION callback.
        let event = unsafe { &*(l_param as *const MSLLHOOKSTRUCT) };
        LAST_CALLBACK_TICK.store(event.time, Ordering::SeqCst);
    }
    // SAFETY: forward the unchanged callback parameters to the next hook.
    unsafe { CallNextHookEx(null_mut(), code, w_param, l_param) }
}

unsafe extern "system" fn keyboard_proc(code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: Windows supplies a valid KBDLLHOOKSTRUCT for an HC_ACTION callback.
        let event = unsafe { *(l_param as *const KBDLLHOOKSTRUCT) };
        LAST_CALLBACK_TICK.store(event.time, Ordering::SeqCst);
        let is_key_down = w_param as u32 == WM_KEYDOWN || w_param as u32 == WM_SYSKEYDOWN;
        let is_key_up = w_param as u32 == WM_KEYUP || w_param as u32 == WM_SYSKEYUP;

        if is_key_down || is_key_up {
            let result = {
                let mut state = modifier_state()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.reduce(KeyInput {
                    virtual_key: event.vkCode as u16,
                    scan_code: event.scanCode,
                    extended: event.flags & LLKHF_EXTENDED != 0,
                    is_down: is_key_down,
                    injected: event.flags & LLKHF_INJECTED != 0,
                })
            };
            // The reducer lock is released before SendInput can reenter the hook.
            if result.mask_start_menu {
                mask_start_menu();
            }
            if let Some(is_pressed) = result.transition {
                dispatch_modifier_transition(is_pressed);
            }
            if result.consume {
                return 1;
            }
        }
    }

    // SAFETY: Forward the unchanged callback parameters to the next hook.
    unsafe { CallNextHookEx(null_mut(), code, w_param, l_param) }
}

fn mask_start_menu() {
    // VK_E8 is unassigned. A balanced pair cancels a bare-Win menu activation.
    let inputs = [0, KEYEVENTF_KEYUP].map(|flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0xE8,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    });
    // SAFETY: Both fully initialized INPUT values remain valid throughout SendInput.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent == 1 {
        // SAFETY: The second INPUT is the initialized key-up for a partially sent pair.
        let released =
            unsafe { SendInput(1, inputs[1..].as_ptr(), std::mem::size_of::<INPUT>() as i32) };
        if released == 1 {
            return;
        }
    }
    if sent != inputs.len() as u32 {
        eprintln!("Echo could not suppress the Windows Start menu for the hotkey.");
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

fn modifier_state() -> &'static Mutex<HotkeyReducer> {
    MODIFIER_STATE.get_or_init(|| Mutex::new(HotkeyReducer::default()))
}

fn clear_monitor_state() {
    SESSION_INACTIVE.store(false, Ordering::SeqCst);
    IS_MONITOR_STARTED.store(false, Ordering::SeqCst);
    if let Some(handler_store) = HANDLER.get() {
        handler_store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }
}

fn finish_monitor_thread(thread: JoinHandle<()>) {
    let deadline = Instant::now() + MONITOR_SHUTDOWN_TIMEOUT;
    while !thread.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if thread.is_finished() {
        let _ = thread.join();
    } else {
        eprintln!("Echo Windows hotkey monitor did not stop before the shutdown deadline.");
    }
}
