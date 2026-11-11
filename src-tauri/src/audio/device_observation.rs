#[derive(Clone, Copy, Default)]
pub(crate) struct DefaultInputDeviceSnapshot {
    pub(crate) has_default_device: Option<bool>,
    pub(crate) is_connected: Option<bool>,
    pub(crate) is_in_use_by_another_application: Option<bool>,
    pub(crate) is_suspended: Option<bool>,
    diagnostic_reason: Option<&'static str>,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy)]
enum EndpointNotification {
    DefaultChanged(i32),
    Added,
    Removed,
    StateChanged(u32),
}

#[cfg(any(windows, test))]
fn endpoint_event(
    generation: u64,
    is_capture: bool,
    notification: EndpointNotification,
) -> Option<super::backend::BackendEvent> {
    if !is_capture {
        return None;
    }
    let outcome = match notification {
        // ERole: eConsole = 0, eCommunications = 2; eMultimedia is excluded.
        EndpointNotification::DefaultChanged(0 | 2) => "default_input_changed",
        EndpointNotification::DefaultChanged(_) => return None,
        EndpointNotification::Added => "device_connected",
        EndpointNotification::Removed => "device_disconnected",
        EndpointNotification::StateChanged(1) => "device_connected",
        EndpointNotification::StateChanged(4 | 8) => "device_disconnected",
        EndpointNotification::StateChanged(_) => "device_state_changed",
    };
    Some(super::backend::BackendEvent::DefaultInputChanged(
        generation, outcome,
    ))
}

#[cfg(any(windows, test))]
fn windows_device_snapshot(
    present: Option<bool>,
    state: Option<u32>,
) -> DefaultInputDeviceSnapshot {
    // DEVICE_STATE values are fixed by the Windows API. A disabled endpoint
    // does not establish physical connection or ownership.
    let (connected, reason) = match (present, state) {
        (Some(false), _) => (None, "wasapi_no_default_input"),
        (None, _) => (None, "wasapi_default_query_failed"),
        (Some(true), Some(1)) => (Some(true), "wasapi_default_active"),
        (Some(true), Some(2)) => (None, "wasapi_default_disabled"),
        (Some(true), Some(4)) => (Some(false), "wasapi_default_not_present"),
        (Some(true), Some(8)) => (Some(false), "wasapi_default_unplugged"),
        (Some(true), _) => (None, "wasapi_default_state_unknown"),
    };
    DefaultInputDeviceSnapshot {
        has_default_device: present,
        is_connected: connected,
        diagnostic_reason: Some(reason),
        ..Default::default()
    }
}

impl DefaultInputDeviceSnapshot {
    pub(crate) fn diagnostic_fields(
        &self,
        stage: &'static str,
    ) -> crate::microphone_diagnostics::MicrophoneFields {
        crate::microphone_diagnostics::MicrophoneFields {
            stage: Some(stage),
            reason: self
                .diagnostic_reason
                .or(Some("default_input_state_unavailable")),
            has_default_audio_input: self.has_default_device,
            default_input_connected: self.is_connected,
            default_input_in_use_by_another_application: self.is_in_use_by_another_application,
            default_input_suspended: self.is_suspended,
            ..Default::default()
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) struct DefaultInputDeviceObserver {
    center: objc2::rc::Retained<objc2_foundation::NSNotificationCenter>,
    connected:
        objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2::runtime::NSObjectProtocol>>,
    disconnected:
        objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2::runtime::NSObjectProtocol>>,
}

#[cfg(target_os = "macos")]
impl DefaultInputDeviceObserver {
    pub(crate) fn start(
        events: std::sync::mpsc::SyncSender<super::backend::BackendEvent>,
        generation: u64,
    ) -> Option<Self> {
        use std::ptr::NonNull;

        use block2::RcBlock;
        use objc2_av_foundation::{
            AVCaptureDeviceWasConnectedNotification, AVCaptureDeviceWasDisconnectedNotification,
        };
        use objc2_foundation::{NSNotification, NSNotificationCenter};

        let center = NSNotificationCenter::defaultCenter();
        let connected_events = events.clone();
        let connected_block = RcBlock::new(move |notification: NonNull<NSNotification>| {
            if is_audio_device_notification(notification) {
                let _ =
                    connected_events.try_send(super::backend::BackendEvent::DefaultInputChanged(
                        generation,
                        "device_connected",
                    ));
            }
        });
        let disconnected_block = RcBlock::new(move |notification: NonNull<NSNotification>| {
            if is_audio_device_notification(notification) {
                let _ = events.try_send(super::backend::BackendEvent::DefaultInputChanged(
                    generation,
                    "device_disconnected",
                ));
            }
        });
        // SAFETY: the framework name is a process-lifetime NSString. The center
        // copies the owned Send-safe block; retained tokens stay live until removal.
        let connected = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(AVCaptureDeviceWasConnectedNotification),
                None,
                None,
                &connected_block,
            )
        };
        // SAFETY: the framework name and center are live. Registration copies the
        // block and its owned sender; the returned token is retained until removal.
        let disconnected = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(AVCaptureDeviceWasDisconnectedNotification),
                None,
                None,
                &disconnected_block,
            )
        };
        Some(Self {
            center,
            connected,
            disconnected,
        })
    }
}

#[cfg(target_os = "macos")]
fn is_audio_device_notification(
    notification: std::ptr::NonNull<objc2_foundation::NSNotification>,
) -> bool {
    use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};

    // SAFETY: NSNotificationCenter supplies a non-null notification valid for this
    // synchronous callback. No borrowed object escapes the callback.
    let notification = unsafe { notification.as_ref() };
    let object = notification.object();
    let Some(device) = object
        .as_deref()
        .and_then(|object| object.downcast_ref::<AVCaptureDevice>())
    else {
        return false;
    };
    // SAFETY: AVFoundation exports this immutable process-lifetime NSString.
    let Some(media_type) = (unsafe { AVMediaTypeAudio }) else {
        return false;
    };
    // SAFETY: the checked downcast is an AVCaptureDevice, borrowed from the live
    // notification; media_type is the framework constant and the call is synchronous.
    unsafe { device.hasMediaType(media_type) }
}

#[cfg(target_os = "macos")]
impl Drop for DefaultInputDeviceObserver {
    fn drop(&mut self) {
        remove_observer(&self.center, &self.connected);
        remove_observer(&self.center, &self.disconnected);
    }
}

#[cfg(target_os = "macos")]
fn remove_observer(
    center: &objc2_foundation::NSNotificationCenter,
    token: &objc2::rc::Retained<
        objc2::runtime::ProtocolObject<dyn objc2::runtime::NSObjectProtocol>,
    >,
) {
    let pointer = objc2::rc::Retained::as_ptr(token).cast::<objc2::runtime::AnyObject>();
    // SAFETY: the retained NSObjectProtocol token is an Objective-C object and
    // remains live through synchronous removal from its retained registering center.
    unsafe {
        center.removeObserver(&*pointer);
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) struct DefaultInputDeviceObserver;

#[cfg(not(any(target_os = "macos", windows)))]
impl DefaultInputDeviceObserver {
    pub(crate) fn start(
        _events: std::sync::mpsc::SyncSender<super::backend::BackendEvent>,
        _generation: u64,
    ) -> Option<Self> {
        None
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn default_input_device_snapshot() -> DefaultInputDeviceSnapshot {
    use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};

    let snapshot = DefaultInputDeviceSnapshot {
        diagnostic_reason: Some("avfoundation_default_not_verified_cpal_identity"),
        ..Default::default()
    };
    // SAFETY: AVFoundation exports this immutable process-lifetime NSString.
    let Some(media_type) = (unsafe { AVMediaTypeAudio }) else {
        return snapshot;
    };
    // SAFETY: the framework media constant is live; AVFoundation returns a
    // retained AVCaptureDevice and permits this query from the observer thread.
    let Some(device) = (unsafe { AVCaptureDevice::defaultDeviceWithMediaType(media_type) }) else {
        return DefaultInputDeviceSnapshot {
            has_default_device: Some(false),
            ..snapshot
        };
    };
    DefaultInputDeviceSnapshot {
        has_default_device: Some(true),
        // SAFETY: device is retained for this scope; this property is a thread-safe AVFoundation query.
        is_connected: Some(unsafe { device.isConnected() }),
        // SAFETY: the retained device remains live through this thread-safe property query.
        is_in_use_by_another_application: Some(unsafe { device.isInUseByAnotherApplication() }),
        // SAFETY: the retained device remains live through this thread-safe property query.
        is_suspended: Some(unsafe { device.isSuspended() }),
        ..snapshot
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) fn default_input_device_snapshot() -> DefaultInputDeviceSnapshot {
    DefaultInputDeviceSnapshot::default()
}

#[cfg(windows)]
pub(crate) use windows_observer::{default_input_device_snapshot, DefaultInputDeviceObserver};

#[cfg(windows)]
mod windows_observer {
    use std::collections::HashSet;
    use std::sync::mpsc::{self, SyncSender};
    use std::thread::{self, JoinHandle};

    use windows::core::{Interface, PCWSTR};
    use windows::Win32::Foundation::{ERROR_NOT_FOUND, PROPERTYKEY, RPC_E_CHANGED_MODE};
    use windows::Win32::Media::Audio::{
        eCapture, eConsole, EDataFlow, ERole, IMMDeviceEnumerator, IMMEndpoint,
        IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator, DEVICE_STATE,
        DEVICE_STATEMASK_ALL,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED,
    };

    use super::{
        endpoint_event, windows_device_snapshot, DefaultInputDeviceSnapshot, EndpointNotification,
    };
    use crate::audio::backend::BackendEvent;

    pub(crate) struct DefaultInputDeviceObserver {
        messages: SyncSender<Message>,
        worker: Option<JoinHandle<()>>,
    }

    enum Message {
        DefaultChanged {
            is_capture: bool,
            role: i32,
        },
        EndpointChanged {
            kind: EndpointNotification,
            id: Vec<u16>,
        },
        Stop,
    }

    // The callback owns only a bounded sender. COM queries and registration
    // teardown stay on the worker; Windows notification threads never wait for it.
    #[windows::core::implement(IMMNotificationClient)]
    struct NotificationClient {
        messages: SyncSender<Message>,
    }

    impl NotificationClient {
        fn endpoint_changed(&self, kind: EndpointNotification, id: &PCWSTR) {
            // SAFETY: Windows supplies a NUL-terminated ID valid for this callback.
            // Only an owned copy leaves the callback, and IDs never enter diagnostics.
            let Some(id) = (unsafe { copy_endpoint_id(id) }) else {
                return;
            };
            let _ = self
                .messages
                .try_send(Message::EndpointChanged { kind, id });
        }
    }

    #[allow(non_snake_case)]
    impl IMMNotificationClient_Impl for NotificationClient_Impl {
        fn OnDefaultDeviceChanged(
            &self,
            flow: EDataFlow,
            role: ERole,
            _: &PCWSTR,
        ) -> windows::core::Result<()> {
            let _ = self.messages.try_send(Message::DefaultChanged {
                is_capture: flow == eCapture,
                role: role.0,
            });
            Ok(())
        }

        fn OnDeviceAdded(&self, id: &PCWSTR) -> windows::core::Result<()> {
            self.endpoint_changed(EndpointNotification::Added, id);
            Ok(())
        }

        fn OnDeviceRemoved(&self, id: &PCWSTR) -> windows::core::Result<()> {
            self.endpoint_changed(EndpointNotification::Removed, id);
            Ok(())
        }

        fn OnDeviceStateChanged(
            &self,
            id: &PCWSTR,
            state: DEVICE_STATE,
        ) -> windows::core::Result<()> {
            self.endpoint_changed(EndpointNotification::StateChanged(state.0), id);
            Ok(())
        }

        fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> windows::core::Result<()> {
            Ok(())
        }
    }

    impl DefaultInputDeviceObserver {
        pub(crate) fn start(events: SyncSender<BackendEvent>, generation: u64) -> Option<Self> {
            let (messages, receiver) = mpsc::sync_channel(64);
            let (ready, started) = mpsc::sync_channel(1);
            let callback_messages = messages.clone();
            let worker = thread::Builder::new()
                .name("echo-device-observation".into())
                .spawn(move || {
                    let Some(_apartment) = ComApartment::initialize() else {
                        return;
                    };
                    let Some(enumerator) = device_enumerator() else {
                        return;
                    };
                    let client: IMMNotificationClient = NotificationClient {
                        messages: callback_messages,
                    }
                    .into();
                    // SAFETY: the worker is COM-initialized. Both COM objects remain
                    // owned here until synchronous unregistration before apartment teardown.
                    if unsafe { enumerator.RegisterEndpointNotificationCallback(&client) }.is_err()
                    {
                        return;
                    }
                    if let Some(mut capture_ids) = capture_endpoint_ids(&enumerator) {
                        if ready.send(()).is_err() {
                            // The starter left; still unregister before releasing the client.
                            // SAFETY: the same live pair was successfully registered above.
                            let _ = unsafe {
                                enumerator.UnregisterEndpointNotificationCallback(&client)
                            };
                            return;
                        }
                        while let Ok(message) = receiver.recv() {
                            let (kind, is_capture) = match message {
                                Message::Stop => break,
                                Message::DefaultChanged { is_capture, role } => {
                                    (EndpointNotification::DefaultChanged(role), is_capture)
                                }
                                Message::EndpointChanged { kind, id } => {
                                    let is_capture =
                                        if matches!(kind, EndpointNotification::Removed) {
                                            // Removed endpoints may no longer be queryable. Cache all
                                            // capture states, including disabled/unplugged endpoints.
                                            capture_ids.remove(&id)
                                        } else {
                                            let capture = is_capture_endpoint(&enumerator, &id)
                                                .unwrap_or_else(|| capture_ids.contains(&id));
                                            if capture {
                                                capture_ids.insert(id);
                                            }
                                            capture
                                        };
                                    (kind, is_capture)
                                }
                            };
                            if let Some(event) = endpoint_event(generation, is_capture, kind) {
                                let _ = events.try_send(event);
                            }
                        }
                    }
                    // SAFETY: this is the same live enumerator/client pair registered
                    // above. Teardown runs on the worker, never inside a callback.
                    let _ = unsafe { enumerator.UnregisterEndpointNotificationCallback(&client) };
                })
                .ok()?;
            // A stalled COM start cannot be reproduced on the macOS test host.
            if started
                .recv_timeout(std::time::Duration::from_secs(2))
                .is_err()
            {
                // COM may still be blocked. Detach rather than joining on the
                // capture owner; a late worker sees Stop or the dropped ready receiver.
                let _ = messages.try_send(Message::Stop);
                return None;
            }
            Some(Self {
                messages,
                worker: Some(worker),
            })
        }
    }

    impl Drop for DefaultInputDeviceObserver {
        fn drop(&mut self) {
            let _ = self.messages.send(Message::Stop);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    struct ComApartment(bool);

    impl ComApartment {
        fn initialize() -> Option<Self> {
            // SAFETY: initializes only the calling thread, with no reserved pointer.
            let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            if result.is_ok() {
                Some(Self(true))
            } else if result == RPC_E_CHANGED_MODE {
                // An existing apartment can service the synchronous snapshot query.
                Some(Self(false))
            } else {
                None
            }
        }
    }

    impl Drop for ComApartment {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: balances this thread's successful CoInitializeEx after
                // every COM interface owned in its scope has been released.
                unsafe { CoUninitialize() };
            }
        }
    }

    fn device_enumerator() -> Option<IMMDeviceEnumerator> {
        // SAFETY: callers hold a live COM apartment; no aggregation is requested.
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.ok()
    }

    unsafe fn copy_endpoint_id(id: &PCWSTR) -> Option<Vec<u16>> {
        if id.is_null() {
            return None;
        }
        // SAFETY: callers guarantee a live NUL-terminated Windows endpoint ID.
        let mut owned = unsafe { id.as_wide() }.to_vec();
        owned.push(0);
        Some(owned)
    }

    fn capture_endpoint_ids(enumerator: &IMMDeviceEnumerator) -> Option<HashSet<Vec<u16>>> {
        // SAFETY: enumerator is live in the worker's COM apartment; output
        // interfaces are owned and released before that apartment ends.
        unsafe {
            let devices = enumerator
                .EnumAudioEndpoints(eCapture, DEVICE_STATE(DEVICE_STATEMASK_ALL))
                .ok()?;
            let mut ids = HashSet::new();
            for index in 0..devices.GetCount().ok()? {
                let device = devices.Item(index).ok()?;
                let id = device.GetId().ok()?;
                let owned = copy_endpoint_id(&PCWSTR(id.0));
                // Free the API allocation even when copying returned no ID.
                CoTaskMemFree(Some(id.0.cast()));
                ids.insert(owned?);
            }
            Some(ids)
        }
    }

    fn is_capture_endpoint(enumerator: &IMMDeviceEnumerator, id: &[u16]) -> Option<bool> {
        // SAFETY: callback IDs are owned NUL-terminated copies. The queried COM
        // interfaces remain in the worker's apartment and never escape it.
        unsafe {
            let device = enumerator.GetDevice(PCWSTR(id.as_ptr())).ok()?;
            let endpoint = device.cast::<IMMEndpoint>().ok()?;
            Some(endpoint.GetDataFlow().ok()? == eCapture)
        }
    }

    pub(crate) fn default_input_device_snapshot() -> DefaultInputDeviceSnapshot {
        let Some(_apartment) = ComApartment::initialize() else {
            return windows_device_snapshot(None, None);
        };
        let Some(enumerator) = device_enumerator() else {
            return windows_device_snapshot(None, None);
        };
        // SAFETY: enumerator and returned endpoint are owned in this thread's
        // live apartment. Query failures stay unknown rather than claiming absence.
        match unsafe { enumerator.GetDefaultAudioEndpoint(eCapture, eConsole) } {
            Ok(device) => windows_device_snapshot(
                Some(true),
                // SAFETY: device is owned in this thread's live COM apartment.
                unsafe { device.GetState() }.ok().map(|state| state.0),
            ),
            Err(error) if error.code() == windows::core::HRESULT::from_win32(ERROR_NOT_FOUND.0) => {
                windows_device_snapshot(Some(false), None)
            }
            Err(_) => windows_device_snapshot(None, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::backend::BackendEvent;
    use super::*;

    // Keep: snapshots carry their source reason; an unknown snapshot stays platform-neutral.
    #[test]
    fn device_snapshot_unknown_reason_is_platform_neutral() {
        assert_eq!(
            DefaultInputDeviceSnapshot::default()
                .diagnostic_fields("snapshot")
                .reason,
            Some("default_input_state_unavailable")
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            default_input_device_snapshot()
                .diagnostic_fields("snapshot")
                .reason,
            Some("avfoundation_default_not_verified_cpal_identity")
        );
    }

    // Keep: capture notifications preserve generation and outcomes; playback and
    // the multimedia-only default role cannot produce capture events.
    #[test]
    fn device_notifications_map_capture_changes_to_echo_events() {
        for (kind, expected) in [
            (
                EndpointNotification::DefaultChanged(0),
                Some("default_input_changed"),
            ),
            (
                EndpointNotification::DefaultChanged(2),
                Some("default_input_changed"),
            ),
            (EndpointNotification::DefaultChanged(1), None),
            (
                EndpointNotification::StateChanged(1),
                Some("device_connected"),
            ),
            (
                EndpointNotification::StateChanged(4),
                Some("device_disconnected"),
            ),
            (
                EndpointNotification::StateChanged(8),
                Some("device_disconnected"),
            ),
            (EndpointNotification::Added, Some("device_connected")),
            (EndpointNotification::Removed, Some("device_disconnected")),
            (
                EndpointNotification::StateChanged(2),
                Some("device_state_changed"),
            ),
        ] {
            for is_capture in [false, true] {
                match (
                    endpoint_event(42, is_capture, kind),
                    expected.filter(|_| is_capture),
                ) {
                    (
                        Some(BackendEvent::DefaultInputChanged(generation, outcome)),
                        Some(expected),
                    ) => {
                        assert_eq!(generation, 42);
                        assert_eq!(outcome, expected);
                    }
                    (None, None) => {}
                    _ => panic!("incorrect capture notification mapping"),
                }
            }
        }
    }

    // Keep: format the actual diagnostic fields, including every Windows state
    // and failed API queries, with no endpoint names or identifiers.
    #[test]
    fn device_snapshots_format_only_known_presence_and_state() {
        for (present, state, connected, reason) in [
            (Some(true), Some(1), Some(true), "wasapi_default_active"),
            (Some(true), Some(2), None, "wasapi_default_disabled"),
            (
                Some(true),
                Some(4),
                Some(false),
                "wasapi_default_not_present",
            ),
            (Some(true), Some(8), Some(false), "wasapi_default_unplugged"),
            (Some(true), Some(16), None, "wasapi_default_state_unknown"),
            (Some(true), None, None, "wasapi_default_state_unknown"),
            (Some(false), None, None, "wasapi_no_default_input"),
            (None, None, None, "wasapi_default_query_failed"),
        ] {
            let snapshot = windows_device_snapshot(present, state);
            assert_eq!(snapshot.has_default_device, present);
            assert_eq!(snapshot.is_connected, connected);
            assert_eq!(snapshot.is_in_use_by_another_application, None);
            assert_eq!(snapshot.is_suspended, None);
            let fields = snapshot.diagnostic_fields("device_state_changed");
            let formatted = serde_json::to_value(fields).unwrap();
            let mut expected = serde_json::json!({"stage":"device_state_changed", "reason":reason});
            if let Some(present) = present {
                expected["hasDefaultAudioInput"] = present.into();
            }
            if let Some(connected) = connected {
                expected["defaultInputConnected"] = connected.into();
            }
            assert_eq!(formatted, expected);
        }
    }
}
