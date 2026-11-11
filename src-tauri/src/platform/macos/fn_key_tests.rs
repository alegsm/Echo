use super::*;

#[derive(Default)]
struct Preferences {
    current: Option<i32>,
    writes: Vec<Option<i32>>,
    fail_write: bool,
}

impl PreferenceAccess for Preferences {
    fn read(&mut self) -> Result<Option<FnKeyValue>, String> {
        Ok(self
            .current
            .map(|value| FnKeyValue::Number(f64::from(value))))
    }

    fn write(&mut self, value: Option<i32>) -> Result<(), String> {
        if self.fail_write {
            return Err("simulated defaults failure".into());
        }
        self.writes.push(value);
        self.current = value;
        Ok(())
    }
}

fn backup_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("echo-fn-key-{}.json", uuid::Uuid::new_v4()))
}

// Keep: only allowlisted, fixed arguments can reach the absolute executable.
#[test]
fn fn_key_command_arguments_are_fixed() {
    assert_eq!(DEFAULTS_PATH, "/usr/bin/defaults");
    for value in [None, Some(0), Some(1), Some(2), Some(3)] {
        let args = write_arguments(value).unwrap();
        let expected = match value {
            None => vec!["delete", "com.apple.HIToolbox", "AppleFnUsageType"],
            Some(0) => vec![
                "write",
                "com.apple.HIToolbox",
                "AppleFnUsageType",
                "-int",
                "0",
            ],
            Some(1) => vec![
                "write",
                "com.apple.HIToolbox",
                "AppleFnUsageType",
                "-int",
                "1",
            ],
            Some(2) => vec![
                "write",
                "com.apple.HIToolbox",
                "AppleFnUsageType",
                "-int",
                "2",
            ],
            Some(3) => vec![
                "write",
                "com.apple.HIToolbox",
                "AppleFnUsageType",
                "-int",
                "3",
            ],
            _ => unreachable!(),
        };
        assert_eq!(args, expected);
    }
    assert!(write_arguments(Some(4)).is_err());
    assert!(write_arguments(Some(-1)).is_err());
}

// Keep: the original value survives repeated changes and a recreated native accessor.
#[test]
fn fn_key_records_before_writing_and_restores_after_restart() {
    for previous in [None, Some(0), Some(1), Some(2), Some(3)] {
        let path = backup_path();
        let mut access = Preferences {
            current: previous,
            ..Preferences::default()
        };
        update_with_access(&path, FnKeyAction::UseFn, &mut access).unwrap();
        assert_eq!(access.current, Some(0));
        assert_eq!(read_backup(&path).unwrap().unwrap().previous, previous);
        update_with_access(&path, FnKeyAction::UseFn, &mut access).unwrap();
        assert_eq!(read_backup(&path).unwrap().unwrap().previous, previous);
        assert!(update_with_access(&path, FnKeyAction::SystemDefault, &mut access).is_err());
        assert_eq!(access.writes, vec![Some(0), Some(0)]);
        let mut restarted = Preferences {
            current: Some(0),
            ..Preferences::default()
        };
        update_with_access(&path, FnKeyAction::Restore, &mut restarted).unwrap();
        assert_eq!(restarted.writes, vec![previous]);
        assert!(!path.exists());
        assert!(update_with_access(&path, FnKeyAction::Restore, &mut restarted).is_err());
        assert_eq!(restarted.writes, vec![previous]);
        if previous.is_none() {
            assert!(update_with_access(&path, FnKeyAction::SystemDefault, &mut restarted).is_err());
        } else {
            update_with_access(&path, FnKeyAction::SystemDefault, &mut restarted).unwrap();
            assert_eq!(restarted.writes, vec![previous, None]);
            assert_eq!(restarted.current, None);
        }
    }
}

#[test]
fn fn_key_failed_write_or_restore_keeps_the_original_backup() {
    let path = backup_path();
    let mut access = Preferences {
        current: Some(2),
        fail_write: true,
        ..Preferences::default()
    };
    assert!(update_with_access(&path, FnKeyAction::UseFn, &mut access).is_err());
    assert_eq!(read_backup(&path).unwrap().unwrap().previous, Some(2));
    access.fail_write = false;
    update_with_access(&path, FnKeyAction::UseFn, &mut access).unwrap();
    access.fail_write = true;
    assert!(update_with_access(&path, FnKeyAction::Restore, &mut access).is_err());
    assert_eq!(read_backup(&path).unwrap().unwrap().previous, Some(2));
    access.fail_write = false;
    update_with_access(&path, FnKeyAction::Restore, &mut access).unwrap();
    assert_eq!(access.current, Some(2));
}

#[test]
fn fn_key_backup_failure_and_unknown_values_never_write() {
    let path = backup_path();
    std::fs::create_dir(&path).unwrap();
    let mut access = Preferences {
        current: Some(2),
        ..Preferences::default()
    };
    assert!(update_with_access(&path, FnKeyAction::UseFn, &mut access).is_err());
    assert!(access.writes.is_empty());
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, "invalid backup").unwrap();
    assert!(update_with_access(&path, FnKeyAction::UseFn, &mut access).is_err());
    assert!(update_with_access(&path, FnKeyAction::Restore, &mut access).is_err());
    assert!(access.writes.is_empty());
    std::fs::remove_file(&path).unwrap();
    access.current = Some(99);
    assert!(update_with_access(&path, FnKeyAction::UseFn, &mut access).is_err());
    assert!(!path.exists());
    assert!(access.writes.is_empty());
}

// Keep: a live, read-only CFPreferences call proves stock absence without touching HIToolbox.
#[test]
fn fn_key_reads_a_guaranteed_missing_preference() {
    let key = format!("missing-{}", uuid::Uuid::new_v4());
    assert_eq!(
        read_preference("app.echostt.desktop.test", &key).unwrap(),
        None
    );
}

// Keep: fractional and unknown numbers remain visible, but cannot be rounded into a writable value.
#[test]
fn fn_key_preserves_fractional_numbers_and_rejects_unrecognized_writes() {
    for number in [1.5, 99.0, -1.0] {
        let value = CFNumber::from(number).as_CFType();
        assert_eq!(preference_value(&value), FnKeyValue::Number(number));
        assert!(restorable_value(Some(FnKeyValue::Number(number))).is_err());
    }
}

// Keep: non-numeric values reach the safe Settings row and cannot be overwritten.
#[test]
fn fn_key_non_numeric_values_are_unrecognized() {
    for value in [
        CFString::new("custom").as_CFType(),
        core_foundation::boolean::CFBoolean::true_value().as_CFType(),
    ] {
        let current = preference_value(&value);
        assert_eq!(
            serde_json::to_value(&current).unwrap(),
            serde_json::json!("unrecognized")
        );
        assert!(restorable_value(Some(current)).is_err());
    }
}
