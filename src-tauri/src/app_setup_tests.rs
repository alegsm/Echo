use super::*;
use crate::providers::ProviderId;
use crate::services::credentials::{tests::CountingStore, CredentialService, CredentialStore};
use crate::storage::test_support::{cleanup, initialized, test_database, transcript};
use crate::storage::{EchoStorage, PersistedSettings};
use crate::test_support::clear_child_environment;

fn app(store: CountingStore) -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .manage(EchoRuntime::default())
        .manage(CredentialService::new(store))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap()
}

fn stale_receipts(directory: &std::path::Path) {
    for words in [
        ["identity", "migration", "v1"],
        ["local", "relocation", "v1"],
    ] {
        std::fs::write(
            directory.join(format!(".{}.json", words.join("-"))),
            b"invalid JSON",
        )
        .unwrap();
    }
}

// Keep: production bootstrap, SQLite, runtime settings and authorized command use no secrets.
#[test]
fn credentials_bootstrap_and_authorized_presence_never_access_the_store() {
    const CHILD: &str = "ECHO_TEST_STARTUP_CHILD";
    if std::env::var_os(CHILD).is_none() {
        for environment_present in [false, true] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            clear_child_environment(&mut child).env(CHILD, "1").args([
                "--exact",
                "app_setup::tests::credentials_bootstrap_and_authorized_presence_never_access_the_store",
                "--nocapture",
            ]);
            if environment_present {
                for name in [
                    "GROQ_API_KEY",
                    "OPENAI_API_KEY",
                    "XAI_API_KEY",
                    "ELEVENLABS_API_KEY",
                    "DEEPGRAM_API_KEY",
                    "MISTRAL_API_KEY",
                ] {
                    child.env(name, "<redacted>");
                }
            }
            assert!(child.status().unwrap().success(), "startup child failed");
        }
        return;
    }
    for mode in ["literal", "env"] {
        for marker in [None, Some(false), Some(true)] {
            for item in ["missing", "present", "denied"] {
                let path = test_database("bootstrap");
                let directory = path.parent().unwrap();
                let storage = initialized(&path);
                storage
                    .append_transcript(transcript("retained", "9998-01-01T00:00:00Z"))
                    .unwrap();
                let mut row = serde_json::to_value(PersistedSettings::default()).unwrap();
                row["providerCredentialModes"] = serde_json::json!({});
                row["providerId"] = ProviderId::OpenAiTranscribe.as_str().into();
                for provider in ProviderId::ALL {
                    row["providerCredentialModes"][provider.as_str()] = mode.into();
                    row["providerKeyStatuses"][provider.as_str()] = "valid".into();
                    if let Some(saved) = marker {
                        row["providerKeySaved"][provider.as_str()] = saved.into();
                    }
                }
                if marker.is_none() {
                    row.as_object_mut().unwrap().remove("providerKeySaved");
                }
                let original = row.to_string();
                let connection = rusqlite::Connection::open(&path).unwrap();
                connection
                    .execute("UPDATE settings SET revision=7,value_json=?1", [&original])
                    .unwrap();
                drop(connection);
                drop(storage);
                stale_receipts(directory);
                let store = CountingStore::default();
                for provider in ProviderId::ALL {
                    if item == "present" {
                        store
                            .write("app.echostt.desktop", provider, "synthetic-fixture")
                            .unwrap();
                    }
                }
                store.deny_reads(item == "denied");
                store.reset_calls();
                let app = app(store.clone());
                let injected = app.state::<CredentialService>().inner() as *const CredentialService;
                let boot = bootstrap(&app, directory).unwrap();
                assert!(
                    store.calls().is_empty(),
                    "startup accessed the store: {:?}",
                    store.calls()
                );
                assert!(std::ptr::eq(
                    injected,
                    app.state::<CredentialService>().inner()
                ));
                assert_eq!(boot.revision, 7);
                assert_eq!(
                    app.state::<EchoRuntime>()
                        .desktop_snapshot()
                        .unwrap()
                        .snapshot
                        .active_provider_id,
                    ProviderId::OpenAiTranscribe.as_str()
                );
                let dashboard =
                    tauri::WebviewWindowBuilder::new(&app, "dashboard", Default::default())
                        .build()
                        .unwrap();
                let overlay = tauri::WebviewWindowBuilder::new(&app, "overlay", Default::default())
                    .build()
                    .unwrap();
                assert_eq!(
                    tauri::async_runtime::block_on(
                        commands::credentials::provider_api_key_presence(overlay)
                    ),
                    Err("window_not_authorized".into())
                );
                for _ in 0..2 {
                    let presence = tauri::async_runtime::block_on(
                        commands::credentials::provider_api_key_presence(dashboard.clone()),
                    )
                    .unwrap();
                    for provider in ProviderId::ALL {
                        let expected = marker.unwrap_or(mode == "literal");
                        assert_eq!(presence[provider.as_str()], expected);
                    }
                }
                assert!(
                    store.calls().is_empty(),
                    "startup or presence accessed the store: {:?}",
                    store.calls()
                );
                assert_eq!(
                    app.state::<EchoStorage>()
                        .list_history(10, None)
                        .unwrap()
                        .records
                        .len(),
                    1
                );
                let connection = rusqlite::Connection::open(&path).unwrap();
                let raw: String = connection
                    .query_row("SELECT value_json FROM settings", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(raw, original, "bootstrap must not rewrite saved metadata");
                drop(connection);
                drop(dashboard);
                drop(app);
                cleanup(&path);
            }
        }
    }
}

// Keep: unrelated source databases never restore absent current data.
#[test]
fn bootstrap_opens_only_current_storage_without_restoring_other_directories() {
    let root_path = test_database("current-only");
    let root = root_path.parent().unwrap();
    let sources = [
        root.join("roaming/app.echostt.desktop/echo.sqlite3"),
        root.join("previous-install/echo.sqlite3"),
    ];
    for source_path in &sources {
        let source = initialized(source_path);
        source
            .append_transcript(transcript("source", "9998-01-01T00:00:00Z"))
            .unwrap();
    }
    let directory = root.join("local/app.echostt.desktop");
    std::fs::create_dir_all(&directory).unwrap();
    stale_receipts(&directory);
    let store = CountingStore::default();
    let app = app(store.clone());
    bootstrap(&app, &directory).unwrap();
    assert!(app
        .state::<EchoStorage>()
        .list_history(10, None)
        .unwrap()
        .records
        .is_empty());
    assert!(store.calls().is_empty());
    drop(app);
    for source_path in &sources {
        assert_eq!(
            initialized(source_path)
                .list_history(10, None)
                .unwrap()
                .records
                .len(),
            1
        );
    }
    cleanup(&root_path);
}

// Keep: per-second progress neither advances state nor broadcasts desktop envelopes.
#[test]
fn progress_uses_only_the_capture_channel() {
    use crate::audio::{CaptureEvent, CapturePhase, CapturePurpose};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tauri::Listener;
    let app = app(CountingStore::default());
    let runtime = app.state::<EchoRuntime>();
    let broadcasts = Arc::new(AtomicUsize::new(0));
    let seen = broadcasts.clone();
    app.listen("echo-desktop-state", move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    });
    let mut event = CaptureEvent {
        sequence: 1,
        phase: CapturePhase::Recording,
        purpose: Some(CapturePurpose::PushToTalk),
        intent_id: Some("intent".into()),
        session_id: Some("session".into()),
        artifact_id: None,
        outcome: "first_callback",
        error: None,
        has_observed_callback: true,
        callback_count: 1,
        sample_count: 960,
        last_error: None,
        artifact_count: 1,
        artifact_bytes: 0,
        elapsed_ms: 0,
        limit_ms: 600_000,
    };
    publish_capture_state(app.handle(), &runtime, &event);
    let before = runtime.desktop_snapshot().unwrap();
    assert_eq!(broadcasts.load(Ordering::SeqCst), 1);
    event.outcome = "recording_progress";
    for second in 1..=3 {
        event.elapsed_ms = second * 1_000;
        publish_capture_state(app.handle(), &runtime, &event);
    }
    assert_eq!(
        runtime.desktop_snapshot().unwrap().sequence,
        before.sequence
    );
    assert_eq!(broadcasts.load(Ordering::SeqCst), 1);
}
