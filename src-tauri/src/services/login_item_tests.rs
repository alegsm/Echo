//! These tests never call enable, disable or is_enabled, nor the Windows registry:
//! they would write or read the real login item of the user running them.

use std::cell::RefCell;
use std::collections::BTreeMap;

use super::user_run_entry::{self, Registry, APPROVED_KEY, RUN_KEY};
use super::*;

/// Name and identifier exactly as the app reads them from tauri.conf.json.
fn configured_identity() -> (String, String) {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
    (
        config["productName"].as_str().unwrap().to_owned(),
        config["identifier"].as_str().unwrap().to_owned(),
    )
}

// Keep: the record is named "Echo" (the NSIS ${PRODUCTNAME} and README's Echo.plist),
// passes exactly the slice 2 argument, stays per-user, and only enabling refuses a
// path the plist cannot hold.
#[test]
fn launcher_config_matches_the_registered_record() {
    let (name, identifier) = configured_identity();
    assert_eq!(name, "Echo");
    for (exe, for_enable) in [
        ("/Applications/Echo.app/Contents/MacOS/echo", true),
        ("/Applications/Echo.app/Contents/MacOS/echo", false),
        (r"C:\Users\Alex Doe\AppData\Local\Echo\echo.exe", true),
        ("/Users/alex/R&D <test>/Echo.app/Contents/MacOS/echo", true),
        ("/Users/alex/R&D <test>/Echo.app/Contents/MacOS/echo", false),
    ] {
        let result = launcher_for(&name, &identifier, Path::new(exe), for_enable);
        let refused = for_enable && cfg!(target_os = "macos") && exe.contains('&');
        if refused {
            let detail = result.unwrap_err();
            assert!(detail.contains("Move Echo"), "{detail}");
            continue;
        }
        let builder = result.unwrap();
        assert_eq!(builder.app_name.as_deref(), Some("Echo"));
        let expected_path = if cfg!(windows) {
            format!("\"{exe}\"")
        } else {
            exe.to_owned()
        };
        assert_eq!(builder.app_path, Some(expected_path), "{exe}");
        assert_eq!(builder.args, Some(vec![LAUNCHED_AT_LOGIN_ARG.to_owned()]));
        assert_eq!(builder.macos_launch_mode, MacOSLaunchMode::LaunchAgent);
        assert_eq!(builder.bundle_identifiers, Some(vec![identifier.clone()]));
        assert_eq!(builder.windows_enable_mode, WindowsEnableMode::CurrentUser);
    }
}

// Keep: the Windows Run value quotes the executable so a path with spaces cannot run
// an earlier partial path; the plist check refuses each character auto-launch leaves
// unescaped.
#[test]
fn run_value_quotes_the_path_and_plist_check_refuses_unescaped_characters() {
    assert_eq!(
        run_value_path(r"C:\Program Files\Echo\echo.exe"),
        r#""C:\Program Files\Echo\echo.exe""#
    );
    for (exe, refused) in [
        ("/Applications/Echo.app/Contents/MacOS/echo", false),
        ("/Users/alex/Echo (1)/Echo.app/Contents/MacOS/echo", false),
        ("/Users/alex/R&D/Echo.app/Contents/MacOS/echo", true),
        ("/Users/alex/<a>/Echo.app/Contents/MacOS/echo", true),
        ("/Users/alex/a>b/Echo.app/Contents/MacOS/echo", true),
    ] {
        assert_eq!(refuse_unescaped_plist_path(exe).is_err(), refused, "{exe}");
    }
}

// Keep: startup rewrites only when the setting is on and the OS still lists Echo; a
// removal, a Task Manager disable or a failed status read is never overridden.
#[test]
fn startup_refresh_rewrites_only_a_listed_record() {
    for (saved_on, status, rewrites, status_read) in [
        (true, Ok(true), true, true),
        (true, Ok(false), false, true),
        (true, Err("denied"), false, true),
        (false, Ok(true), false, false),
        (false, Ok(false), false, false),
        (false, Err("denied"), false, false),
    ] {
        let (mut read, mut written) = (false, false);
        let result = refresh(
            saved_on,
            || {
                read = true;
                status.map_err(str::to_owned)
            },
            || {
                written = true;
                Ok(())
            },
        );
        assert_eq!(
            (read, written),
            (status_read, rewrites),
            "{saved_on} {status:?}"
        );
        assert_eq!(result.is_err(), saved_on && status.is_err());
    }
}

const MACHINE_RUN_KEY: &str = r"HKLM\Software\Microsoft\Windows\CurrentVersion\Run";
const MACHINE_APPROVED_KEY: &str =
    r"HKLM\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const TASK_MANAGER_ON: &[u8] = &[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
const TASK_MANAGER_OFF: &[u8] = &[3, 0, 0, 0, 0x5a, 0x1c, 0x3e, 0x01, 0xd2, 0x9f, 0xda, 0x01];

/// Every hive of one Windows machine, so a test can see an HKLM value touched.
#[derive(Default)]
struct FakeRegistry {
    values: RefCell<BTreeMap<(String, String), Vec<u8>>>,
    failing_key: Option<&'static str>,
}

impl FakeRegistry {
    fn with(entries: &[(&str, &str, &[u8])]) -> Self {
        let registry = Self::default();
        for (key, name, value) in entries {
            registry
                .values
                .borrow_mut()
                .insert((key.to_string(), name.to_string()), value.to_vec());
        }
        registry
    }

    fn entries(&self) -> Vec<(String, String)> {
        self.values.borrow().keys().cloned().collect()
    }
}

impl Registry for FakeRegistry {
    fn value(&self, key: &str, name: &str) -> Result<Option<Vec<u8>>, String> {
        if self.failing_key == Some(key) {
            return Err("Access is denied.".into());
        }
        Ok(self
            .values
            .borrow()
            .get(&(key.to_owned(), name.to_owned()))
            .cloned())
    }

    fn remove_value(&self, key: &str, name: &str) -> Result<(), String> {
        if self.failing_key == Some(key) {
            return Err("Access is denied.".into());
        }
        self.values
            .borrow_mut()
            .remove(&(key.to_owned(), name.to_owned()));
        Ok(())
    }
}

// Keep: turning the switch off on Windows deletes only Echo's HKCU Run value and its
// Task Manager switch, never an HKLM value or another app's entry, and deletes the Run
// value first so a failure cannot re-enable an entry Task Manager disabled.
#[test]
fn windows_removal_deletes_only_the_users_echo_entry() {
    let machine = [
        (
            RUN_KEY,
            "Echo",
            b"\"C:\\Echo\\echo.exe\" --launched-at-login".as_slice(),
        ),
        (APPROVED_KEY, "Echo", TASK_MANAGER_OFF),
        (RUN_KEY, "Other", b"other.exe".as_slice()),
        (MACHINE_RUN_KEY, "Echo", b"C:\\Echo\\echo.exe".as_slice()),
        (MACHINE_APPROVED_KEY, "Echo", TASK_MANAGER_ON),
    ];
    let registry = FakeRegistry::with(&machine);
    user_run_entry::remove(&registry, "Echo").unwrap();
    let left = |entries: &[(&str, &str, &[u8])]| {
        entries
            .iter()
            .map(|(key, name, _)| (key.to_string(), name.to_string()))
            .collect::<Vec<_>>()
    };
    let mut expected = left(&machine[2..]);
    expected.sort();
    assert_eq!(registry.entries(), expected);
    user_run_entry::remove(&FakeRegistry::default(), "Echo").unwrap();

    let failing = FakeRegistry {
        failing_key: Some(APPROVED_KEY),
        ..FakeRegistry::with(&machine[..2])
    };
    assert_eq!(
        user_run_entry::remove(&failing, "Echo"),
        Err("Access is denied.".into())
    );
    assert_eq!(failing.entries(), left(&machine[1..2]));
}

// Keep: Windows status reads only Echo's HKCU entry, so a machine-wide entry Echo did
// not write never shows a mismatch, and a Task Manager disable reads as off.
#[test]
fn windows_status_reads_only_the_users_echo_entry() {
    let run = (RUN_KEY, "Echo", b"echo.exe".as_slice());
    let machine_run = (MACHINE_RUN_KEY, "Echo", b"echo.exe".as_slice());
    for (entries, expected) in [
        (vec![], false),
        (vec![machine_run], false),
        (
            vec![machine_run, (APPROVED_KEY, "Echo", TASK_MANAGER_ON)],
            false,
        ),
        (vec![(RUN_KEY, "Other", b"other.exe".as_slice())], false),
        (vec![run], true),
        (vec![run, (APPROVED_KEY, "Echo", TASK_MANAGER_ON)], true),
        (vec![run, (APPROVED_KEY, "Echo", TASK_MANAGER_OFF)], false),
        (
            vec![run, (MACHINE_APPROVED_KEY, "Echo", TASK_MANAGER_OFF)],
            true,
        ),
    ] {
        let registry = FakeRegistry::with(&entries);
        assert_eq!(
            user_run_entry::enabled(&registry, "Echo"),
            Ok(expected),
            "{entries:?}"
        );
    }
    let failing = FakeRegistry {
        failing_key: Some(RUN_KEY),
        ..FakeRegistry::default()
    };
    assert!(user_run_entry::enabled(&failing, "Echo").is_err());
}

// Keep: the real registry adapter opens HKCU subkeys only and refuses any other hive,
// so a wrong key path fails instead of reaching HKLM.
#[test]
fn windows_registry_adapter_refuses_other_hives() {
    assert_eq!(
        user_run_entry::user_subkey(RUN_KEY),
        Ok(r"Software\Microsoft\Windows\CurrentVersion\Run")
    );
    for key in [
        MACHINE_RUN_KEY,
        r"Software\Microsoft\Windows\CurrentVersion\Run",
    ] {
        assert!(user_run_entry::user_subkey(key).is_err(), "{key}");
    }
}
