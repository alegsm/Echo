use std::sync::Arc;
use std::time::Duration;

use core_graphics::event::CGEventType;

use super::{event_route, run_event_loop, EventRoute, StopSignal};

#[test]
fn event_tap_disable_notifications_route_to_recovery() {
    assert_eq!(
        event_route(CGEventType::TapDisabledByTimeout),
        EventRoute::RecoverTap
    );
    assert_eq!(
        event_route(CGEventType::TapDisabledByUserInput),
        EventRoute::RecoverTap
    );
    assert_eq!(
        event_route(CGEventType::FlagsChanged),
        EventRoute::ReadFlags
    );
    assert_eq!(event_route(CGEventType::KeyDown), EventRoute::Ignore);
}

#[test]
fn event_loop_does_not_enter_after_shutdown_was_requested() {
    let signal = StopSignal::default();
    signal.request();

    run_event_loop(&signal);
}

#[test]
fn shutdown_interrupts_the_monitor_retry_wait() {
    let signal = Arc::new(StopSignal::default());
    let thread_signal = Arc::clone(&signal);
    let thread = std::thread::spawn(move || thread_signal.wait_for_retry());

    signal.request();

    assert!(thread.join().expect("retry thread should stop"));
}

#[test]
fn monitor_retry_waits_until_shutdown() {
    let signal = Arc::new(StopSignal::default());
    let thread_signal = Arc::clone(&signal);
    let (started_sender, started_receiver) = std::sync::mpsc::sync_channel(1);
    let (result_sender, result_receiver) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || {
        started_sender.send(()).unwrap();
        result_sender.send(thread_signal.wait_for_retry())
    });

    started_receiver.recv().unwrap();
    assert!(result_receiver
        .recv_timeout(Duration::from_millis(30))
        .is_err());
    signal.request();
    assert!(result_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap());
    thread.join().unwrap().unwrap();
}
