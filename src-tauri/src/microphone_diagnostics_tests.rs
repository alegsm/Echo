use super::*;

fn test_identity() -> CaptureDiagnosticIdentity {
    CaptureDiagnosticIdentity::new("test", "test", "test")
}

#[test]
fn sample_summary_excludes_non_finite_values() {
    let block = summarize_samples(&[0.0, 0.5, -0.5, f32::NAN, f32::INFINITY]);
    assert_eq!((block.samples, block.finite, block.non_finite), (5, 3, 2));
    assert_eq!(block.near_zero, 1);
    assert!((block.peak - 0.5).abs() < f64::EPSILON);
    assert!((block.sum_squares - 0.5).abs() < f64::EPSILON);
}

#[test]
fn silence_summary_uses_documented_thresholds() {
    let mut window = WindowMetrics::default();
    window.add(summarize_samples(&[0.0; 32]));
    let (rms, ratio, silent) = window.summary();
    assert_eq!(rms, 0.0);
    assert_eq!(ratio, 1.0);
    assert!(silent);

    let mut audible = WindowMetrics::default();
    audible.add(summarize_samples(&[0.01; 32]));
    assert!(!audible.summary().2);
}

#[test]
fn stall_threshold_uses_four_times_the_observed_median() {
    assert_eq!(stall_threshold_ms(&VecDeque::from([10, 11, 12, 13])), None);
    assert_eq!(
        stall_threshold_ms(&VecDeque::from([10, 11, 12, 13, 14])),
        Some(250)
    );
    assert_eq!(
        stall_threshold_ms(&VecDeque::from([70, 75, 80, 85, 90])),
        Some(320)
    );
}

#[test]
fn signal_windows_are_emitted_at_most_twice_per_second() {
    let mut state = WorkerState::new(test_identity());
    let play_started = AtomicU64::new(0);
    for at_ms in [0, 100, 499, 500, 750, 999, 1_000] {
        let mut block = summarize_samples(&[0.25; 8]);
        block.at_ms = at_ms;
        process_block("test-session", &mut state, block, &play_started);
    }
    assert_eq!(state.completed_window_count, 2);
}

#[test]
fn terminal_metrics_include_starvation_after_the_last_callback() {
    let mut state = WorkerState::new(test_identity());
    state.last_callback_ms = Some(100);
    state.longest_gap_ms = 0;
    assert_eq!(terminal_callback_gaps(&state, 5_100), (5_000, Some(5_000)));
}

#[test]
fn disabled_callback_does_no_telemetry_work() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let callback = test_callback(sender, false);
    callback.observe(&[0.5]);
    assert!(receiver.try_recv().is_err());
    assert_eq!(callback.dropped.load(Ordering::Relaxed), 0);
}

#[test]
fn bounded_channel_counts_dropped_summaries() {
    let (sender, _receiver) = mpsc::sync_channel(1);
    let callback = test_callback(sender, true);
    callback.observe(&[0.25]);
    callback.observe(&[0.25]);
    assert_eq!(callback.dropped.load(Ordering::Relaxed), 1);
}

fn test_callback(sender: SyncSender<Message>, enabled: bool) -> CallbackDiagnostics {
    CallbackDiagnostics {
        sender,
        enabled: Arc::new(AtomicBool::new(enabled)),
        dropped: Arc::new(AtomicU64::new(0)),
        play_started_ms: Arc::new(AtomicU64::new(PLAY_NOT_STARTED)),
        origin: Instant::now(),
        token: None,
    }
}
