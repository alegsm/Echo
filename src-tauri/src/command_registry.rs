macro_rules! for_each_echo_command {
    ($consumer:ident) => {
        $consumer! {
            deliver_transcript => crate::commands::clipboard::deliver_transcript,
            copy_transcript => crate::commands::clipboard::copy_transcript,
            position_overlay => crate::commands::overlay_position::position_overlay,
            set_fn_hotkey_capture_enabled => crate::commands::modifier::set_fn_hotkey_capture_enabled,
            get_desktop_permissions => crate::commands::permissions::get_desktop_permissions,
            request_desktop_permission => crate::commands::permissions::request_desktop_permission,
            open_desktop_permission_settings => crate::commands::permissions::open_desktop_permission_settings,
            change_fn_key_preference => crate::commands::permissions::change_fn_key_preference,
            get_fn_key_preference => crate::commands::permissions::get_fn_key_preference,
            start_audio_capture => crate::commands::audio_capture::start_audio_capture,
            stop_audio_capture => crate::commands::audio_capture::stop_audio_capture,
            cancel_audio_capture => crate::commands::audio_capture::cancel_audio_capture,
            get_desktop_runtime_snapshot => crate::commands::stt::get_desktop_runtime_snapshot,
            transcribe_audio_artifact => crate::commands::stt::transcribe_audio_artifact,
            probe_provider => crate::commands::stt::probe_provider,
            cancel_transcription_job => crate::commands::stt::cancel_transcription_job,
            report_transcript_delivery => crate::commands::stt::report_transcript_delivery,
            get_persisted_settings => crate::commands::storage::settings::get_persisted_settings,
            update_persisted_settings => crate::commands::storage::settings::update_persisted_settings,
            get_launch_at_login_status => crate::commands::login_item::get_launch_at_login_status,
            list_persisted_history => crate::commands::storage::history::list_persisted_history,
            get_persisted_transcript => crate::commands::storage::history::get_persisted_transcript,
            delete_persisted_transcript => crate::commands::storage::history::delete_persisted_transcript,
            clear_persisted_history => crate::commands::storage::history::clear_persisted_history,
            preview_history_retention => crate::commands::storage::history::preview_history_retention,
            set_provider_api_key => crate::commands::credentials::set_provider_api_key,
            delete_provider_api_key => crate::commands::credentials::delete_provider_api_key,
            find_provider_api_key => crate::commands::credentials::find_provider_api_key,
            provider_api_key_presence => crate::commands::credentials::provider_api_key_presence,
            get_diagnostic_context => crate::diagnostics::get_diagnostic_context,
            record_diagnostic_event => crate::diagnostics::record_diagnostic_event,
            probe_diagnostic_log => crate::diagnostics::probe_diagnostic_log,
        }
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn fn_key_mutation_is_dashboard_only() {
        let grant = "allow-change-fn-key-preference";
        let dashboard: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/dashboard.json")).unwrap();
        let overlay: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/overlay.json")).unwrap();
        assert_eq!(
            dashboard["permissions"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| *p == grant)
                .count(),
            1
        );
        assert!(!overlay["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == grant));
        let source = include_str!("commands/permissions.rs");
        let setter = source
            .split("pub fn change_fn_key_preference(")
            .nth(1)
            .unwrap()
            .split("fn desktop_permissions_response")
            .next()
            .unwrap();
        assert!(setter.contains("require_dashboard(&window)?"));
        assert!(!setter.contains("require_known_window"));
    }

    #[test]
    fn fn_key_never_uses_path_lookup_or_kills_processes() {
        let source = include_str!("platform/macos/fn_key.rs");
        assert!(!source.contains("Command::new(\"defaults\")"));
        assert!(!source.contains("Command::new(\"killall\")"));
    }

    // Keep: the read refreshes CoreFoundation's cache before querying a defaults-written domain.
    #[test]
    fn fn_key_synchronizes_before_reading() {
        let source = include_str!("platform/macos/fn_key.rs");
        let read = source.split("fn read_preference(").nth(1).unwrap();
        assert!(
            read.find("CFPreferencesAppSynchronize(").unwrap()
                < read.find("CFPreferencesCopyAppValue(").unwrap()
        );
    }

    #[test]
    fn retired_retry_request_is_rejected() {
        let proof = serde_json::json!({
            "artifactId": "artifact", "sessionId": "session", "intentId": "intent"
        });
        assert!(
            serde_json::from_value::<crate::commands::stt::TranscribeArtifactRequest>(
                proof.clone()
            )
            .is_ok()
        );
        let mut retired = proof;
        retired["retryOfIntentId"] = "old-intent".into();
        assert!(
            serde_json::from_value::<crate::commands::stt::TranscribeArtifactRequest>(retired)
                .is_err()
        );
    }

    #[test]
    fn retired_retry_paths_are_absent() {
        let pipeline = include_str!("commands/stt/pipeline.rs");
        let registry = include_str!("services/transcription_jobs.rs");
        for retired in ["execute_artifact_job", "prepare_retry", "lease_and_read"] {
            assert!(!pipeline.contains(retired), "retained {retired}");
        }
        for retired in [
            "RetryEntry",
            "RetryDisposition",
            "admit_retry",
            "retry_settings",
            "add_retry",
            "MAX_RETRY_RECORDS",
        ] {
            assert!(!registry.contains(retired), "retained {retired}");
        }
    }

    macro_rules! collect_command_names {
        ($( $name:ident => $handler:path, )*) => {
            vec![$(stringify!($name)),*]
        };
    }

    // Keep: only the dashboard may search the OS store, and only after native authorization.
    #[test]
    fn saved_key_search_is_dashboard_only() {
        let names = for_each_echo_command!(collect_command_names);
        assert!(names.contains(&"find_provider_api_key"));
        let grant = "allow-find-provider-api-key";
        let compiled: serde_json::Value =
            serde_json::from_str(include_str!("../gen/schemas/capabilities.json")).unwrap();
        for (source, surface, granted) in [
            (
                include_str!("../capabilities/dashboard.json"),
                "dashboard",
                true,
            ),
            (
                include_str!("../capabilities/overlay.json"),
                "overlay",
                false,
            ),
        ] {
            let capability: serde_json::Value = serde_json::from_str(source).unwrap();
            for grants in [
                &capability["permissions"],
                &compiled[surface]["permissions"],
            ] {
                let count = grants
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|p| *p == grant)
                    .count();
                assert_eq!(count, usize::from(granted), "{surface} grant");
            }
        }
        let handlers = include_str!("commands/credentials.rs");
        let search = handlers
            .split("pub async fn find_provider_api_key<")
            .nth(1)
            .unwrap()
            .split("#[derive(")
            .next()
            .unwrap();
        assert!(
            search.find("require_dashboard(&window)?").unwrap()
                < search.find("spawn_blocking").unwrap()
        );
        assert!(!search.contains("echo-credentials-updated"));
    }

    // Keep: only the dashboard may read login-item status, after native authorization,
    // and no capability carries the autostart plugin's JS enable, disable or status grants.
    #[test]
    fn launch_at_login_status_is_dashboard_only() {
        let names = for_each_echo_command!(collect_command_names);
        assert!(names.contains(&"get_launch_at_login_status"));
        let grant = "allow-get-launch-at-login-status";
        let compiled: serde_json::Value =
            serde_json::from_str(include_str!("../gen/schemas/capabilities.json")).unwrap();
        for (source, surface, granted) in [
            (
                include_str!("../capabilities/dashboard.json"),
                "dashboard",
                1,
            ),
            (include_str!("../capabilities/overlay.json"), "overlay", 0),
        ] {
            let capability: serde_json::Value = serde_json::from_str(source).unwrap();
            for grants in [
                &capability["permissions"],
                &compiled[surface]["permissions"],
            ] {
                let grants = grants.as_array().unwrap();
                assert_eq!(
                    grants.iter().filter(|p| *p == grant).count(),
                    granted,
                    "{surface}"
                );
                assert!(!grants.iter().any(|p| p
                    .as_str()
                    .is_some_and(|name| name.starts_with("autostart:"))));
            }
        }
        let handler = include_str!("commands/login_item.rs")
            .split("pub fn get_launch_at_login_status(")
            .nth(1)
            .unwrap();
        assert!(
            handler.find("require_dashboard(&window)?").unwrap()
                < handler.find("is_registered(").unwrap()
        );
        assert!(!handler.contains("require_known_window"));
    }

    // Keep: neither renderer may bypass native field filtering with plugin log strings.
    #[test]
    fn diagnostics_grants_exclude_generic_log_commands() {
        let names = for_each_echo_command!(collect_command_names);
        assert!(names.contains(&"record_diagnostic_event"));
        let compiled: serde_json::Value =
            serde_json::from_str(include_str!("../gen/schemas/capabilities.json")).unwrap();
        for surface in ["dashboard", "overlay"] {
            let grants = compiled[surface]["permissions"].as_array().unwrap();
            assert!(grants
                .iter()
                .any(|grant| grant == "allow-record-diagnostic-event"));
            assert!(!grants
                .iter()
                .any(|grant| grant.as_str().is_some_and(|s| s.starts_with("log:"))));
        }
        let overlay: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/overlay.json")).unwrap();
        assert!(!overlay["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|grant| grant == "allow-get-diagnostic-context"));
        for source in [
            include_str!("../capabilities/dashboard.json"),
            include_str!("../capabilities/overlay.json"),
        ] {
            let capability: serde_json::Value = serde_json::from_str(source).unwrap();
            let grants = capability["permissions"].as_array().unwrap();
            assert!(!grants
                .iter()
                .any(|grant| grant.as_str().is_some_and(|s| s.starts_with("log:"))));
            assert!(grants
                .iter()
                .any(|grant| grant == "allow-record-diagnostic-event"));
        }
    }
    // Keep: renderer clipboard authority is closed and delivery is limited to the overlay.
    #[test]
    fn clipboard_commands_and_capabilities_are_least_privilege() {
        let names = for_each_echo_command!(collect_command_names);
        assert!(names.contains(&"deliver_transcript"));
        assert!(names.contains(&"copy_transcript"));
        assert!(!names.contains(&"paste_clipboard"));
        for (source, overlay) in [
            (include_str!("../capabilities/overlay.json"), true),
            (include_str!("../capabilities/dashboard.json"), false),
        ] {
            let capability: serde_json::Value = serde_json::from_str(source).unwrap();
            let grants = capability["permissions"].as_array().unwrap();
            assert!(grants.iter().any(|p| p == "allow-copy-transcript"));
            assert_eq!(
                grants.iter().any(|p| p == "allow-deliver-transcript"),
                overlay
            );
            for grant in grants {
                let name = grant.as_str().unwrap();
                assert!(!name.starts_with("clipboard-manager:"));
                assert_ne!(name, "allow-paste-clipboard");
            }
        }
        let handlers = include_str!("commands/clipboard.rs");
        let delivery = handlers
            .split("pub async fn deliver_transcript(")
            .nth(1)
            .unwrap()
            .split("#[tauri::command]")
            .next()
            .unwrap();
        assert!(
            delivery.find("require_overlay(&window)?").unwrap()
                < delivery.find("spawn_blocking").unwrap()
        );
        let copy = handlers
            .split("pub async fn copy_transcript(")
            .nth(1)
            .unwrap();
        assert!(
            copy.find("require_known_window(&window)?").unwrap()
                < copy.find("spawn_blocking").unwrap()
        );
    }

    // Keep: host contract pin for Windows-only marker publication; OS behavior needs host proof.
    #[test]
    fn clipboard_windows_publication_requires_markers_before_text() {
        let source = include_str!("platform/windows/clipboard.rs");
        let publication = source
            .split("pub fn publish_protected(")
            .nth(1)
            .unwrap()
            .split("pub fn restore_if_owned(")
            .next()
            .unwrap();
        for marker in [
            "ExcludeClipboardContentFromMonitorProcessing",
            "CanIncludeInClipboardHistory",
            "CanUploadToCloudClipboard",
        ] {
            assert!(publication.contains(marker), "missing {marker}");
        }
        assert!(publication.contains("Memory::new(&0u32.to_le_bytes())?"));
        assert!(
            publication.find("memory.publish(format)?").unwrap()
                < publication.find("text.publish(CF_UNICODETEXT)?").unwrap()
        );
    }

    #[test]
    fn retired_commands_have_no_handlers() {
        let names = for_each_echo_command!(collect_command_names);
        for command in [
            "get_audio_capture_snapshot",
            "discard_audio_artifact",
            "legacy_migration_receipt",
            "import_legacy_state",
            "mark_legacy_migration_complete",
            "reveal_diagnostic_log",
            "get_microphone_permission_status",
            "request_microphone_permission",
            "set_globe_key_do_nothing",
        ] {
            assert!(!names.contains(&command), "registered {command}");
        }
    }

    #[test]
    fn retired_commands_have_no_window_grants() {
        for source in [
            include_str!("../capabilities/dashboard.json"),
            include_str!("../capabilities/overlay.json"),
        ] {
            let capability: serde_json::Value = serde_json::from_str(source).unwrap();
            for permission in capability["permissions"].as_array().unwrap() {
                let grant = permission
                    .as_str()
                    .or_else(|| permission["identifier"].as_str())
                    .unwrap();
                for retired in [
                    "allow-get-audio-capture-snapshot",
                    "allow-discard-audio-artifact",
                    "allow-legacy-migration-receipt",
                    "allow-import-legacy-state",
                    "allow-mark-legacy-migration-complete",
                    "allow-reveal-diagnostic-log",
                    "allow-get-microphone-permission-status",
                    "allow-request-microphone-permission",
                    "allow-set-globe-key-do-nothing",
                ] {
                    assert_ne!(grant, retired);
                }
                assert!(!grant.starts_with("opener:"));
            }
        }
    }

    #[test]
    fn retired_capture_commands_have_no_permissions_or_schemas() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for command in [
            "paste_clipboard",
            "get_audio_capture_snapshot",
            "discard_audio_artifact",
            "set_globe_key_do_nothing",
        ] {
            assert!(!root
                .join("permissions/autogenerated")
                .join(format!("{command}.toml"))
                .exists());
            for schema in [
                "acl-manifests.json",
                "capabilities.json",
                "desktop-schema.json",
                "macOS-schema.json",
                "windows-schema.json",
            ] {
                let source =
                    std::fs::read_to_string(root.join("gen/schemas").join(schema)).unwrap();
                assert!(!source.contains(command), "{schema} retains {command}");
                let permission = command.replace('_', "-");
                assert!(
                    !source.contains(&permission),
                    "{schema} retains {permission}"
                );
            }
        }
    }

    #[test]
    fn transcript_editing_is_not_registered() {
        let names = for_each_echo_command!(collect_command_names);
        assert!(!names.contains(&"update_persisted_transcript_text"));
    }

    #[test]
    fn command_names_are_unique() {
        let names = for_each_echo_command!(collect_command_names);
        let unique_names = names
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(unique_names.len(), names.len());
        assert!(!names.contains(&"compare_audio_artifact"));
        assert!(names.contains(&"probe_provider"));
    }

    #[test]
    fn dashboard_keeps_provider_probe_without_capture_or_compare_grants() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/dashboard.json")).unwrap();
        let permissions = capability["permissions"].as_array().unwrap();
        assert!(permissions
            .iter()
            .any(|value| value == "allow-probe-provider"));
        for grant in [
            "clipboard-manager:allow-read-text",
            "allow-compare-audio-artifact",
            "allow-get-audio-capture-snapshot",
            "allow-start-audio-capture",
            "allow-stop-audio-capture",
            "allow-cancel-audio-capture",
            "allow-discard-audio-artifact",
            "allow-cancel-transcription-job",
        ] {
            assert!(!permissions.iter().any(|value| value == grant), "{grant}");
        }
    }

    #[test]
    fn neither_window_can_forge_backend_events() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
        let mut paths: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        paths.sort();
        assert!(!paths.is_empty(), "no capability JSON files found");
        for required in ["dashboard.json", "overlay.json"] {
            assert!(
                paths.contains(&directory.join(required)),
                "missing {required}"
            );
        }

        for path in paths {
            let source = std::fs::read_to_string(&path).unwrap();
            let capability: serde_json::Value = serde_json::from_str(&source).unwrap();
            for permission in capability["permissions"].as_array().unwrap() {
                let grant = permission
                    .as_str()
                    .or_else(|| permission["identifier"].as_str())
                    .unwrap();
                assert!(
                    !matches!(
                        grant,
                        "core:event:allow-emit"
                            | "core:event:allow-emit-to"
                            | "core:event:default"
                            | "core:default"
                    ),
                    "{} grants {grant}, allowing forged echo-partial-transcript events",
                    path.display()
                );
            }
        }
    }
}
