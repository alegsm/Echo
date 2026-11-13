use crate::storage::PersistedSettings;

use super::{close_action, dashboard_visible_at_start, CloseAction};
use crate::services::runtime::EchoRuntime;

fn runtime_with(revisions: &[(u64, bool)]) -> EchoRuntime {
    let runtime = EchoRuntime::default();
    for &(revision, run_in_background) in revisions {
        let settings = PersistedSettings {
            run_in_background,
            ..PersistedSettings::default()
        };
        runtime
            .replace_persisted_settings(revision, &settings)
            .expect("settings should reach the runtime");
    }
    runtime
}

fn poisoned_runtime() -> EchoRuntime {
    let runtime = runtime_with(&[(1, true)]);
    runtime.poison_lock();
    runtime
}

// Keep: closing reads the committed setting at close time and quits unless it is
// known to be on, including a fresh or 0.3.2 database and an unavailable runtime lock.
#[test]
fn close_action_hides_only_when_the_committed_setting_is_on() {
    let cases = [
        ("fresh database", EchoRuntime::default(), CloseAction::Exit),
        ("off", runtime_with(&[(1, false)]), CloseAction::Exit),
        ("on", runtime_with(&[(1, true)]), CloseAction::Hide),
        (
            "turned off by a newer revision",
            runtime_with(&[(1, true), (2, false)]),
            CloseAction::Exit,
        ),
        (
            "turned on by a newer revision",
            runtime_with(&[(1, false), (2, true)]),
            CloseAction::Hide,
        ),
        (
            "stale revision ignored",
            runtime_with(&[(2, true), (1, false)]),
            CloseAction::Hide,
        ),
        (
            "runtime lock unavailable",
            poisoned_runtime(),
            CloseAction::Exit,
        ),
    ];
    for (case, runtime, expected) in cases {
        assert_eq!(close_action(&runtime), expected, "{case}");
    }
}

// Keep: only a login launch with the setting on starts with the dashboard hidden.
#[test]
fn dashboard_is_hidden_at_start_only_for_a_login_launch_with_the_setting_on() {
    let cases = [
        (
            "normal launch, off",
            false,
            runtime_with(&[(1, false)]),
            true,
        ),
        ("normal launch, on", false, runtime_with(&[(1, true)]), true),
        ("login launch, off", true, runtime_with(&[(1, false)]), true),
        ("login launch, on", true, runtime_with(&[(1, true)]), false),
        (
            "login launch, lock unavailable",
            true,
            poisoned_runtime(),
            true,
        ),
    ];
    for (case, launched_at_login, runtime, expected) in cases {
        assert_eq!(
            dashboard_visible_at_start(launched_at_login, &runtime),
            expected,
            "{case}"
        );
    }
}
