use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde_json::json;

use crate::test_support::clear_child_environment;

use super::{
    execute_with_verdict, multipart_file, parse_transcript, status_error, ProviderErrorCode,
    ProviderId, TranscriptionRequest, MAX_PROVIDER_RESPONSE_BYTES,
};

fn test_client() -> reqwest::Client {
    super::http_client_builder()
        .https_only(false)
        .build()
        .expect("test client should build")
}

// Keep: child isolation catches ambient routing through the production builder.
#[test]
fn production_client_ignores_system_proxies() {
    const ORIGIN: &str = "ECHO_TEST_PROXY_ORIGIN";
    if let Ok(url) = std::env::var(ORIGIN) {
        let client = super::http_client_builder()
            .https_only(false)
            .build()
            .unwrap();
        let response = tauri::async_runtime::block_on(async {
            client
                .get(url)
                .bearer_auth("synthetic-fixture")
                .send()
                .await
                .unwrap()
        });
        assert_eq!(response.status(), StatusCode::OK);
        return;
    }
    let origin = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    origin.set_nonblocking(true).unwrap();
    proxy.set_nonblocking(true).unwrap();
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    clear_child_environment(&mut command)
        .env(ORIGIN, format!("http://{}", origin.local_addr().unwrap()));
    for name in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        command.env(name, &proxy_url);
    }
    let mut child = command
        .args([
            "--exact",
            "providers::tests::production_client_ignores_system_proxies",
            "--nocapture",
        ])
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut origin_request = None;
    let mut proxy_connected = false;
    let status = loop {
        for (listener, is_proxy) in [(&origin, false), (&proxy, true)] {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let request = read_http_request(&mut stream);
                    if is_proxy {
                        proxy_connected = true;
                    } else {
                        origin_request = Some(request);
                    }
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .unwrap();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                Err(error) => panic!("listener failed: {error}"),
            }
        }
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("proxy test child exceeded five seconds");
        }
        thread::sleep(Duration::from_millis(5));
    };
    assert!(
        !proxy_connected,
        "production client connected to the system proxy"
    );
    assert!(status.success(), "proxy test child failed");
    let request = origin_request.expect("origin must receive the provider request");
    assert!(String::from_utf8_lossy(&request).contains("authorization: Bearer synthetic-fixture"));
}

#[test]
fn provider_client_settings_require_timeouts() {
    let settings = super::http_client_settings();
    assert_eq!(settings.connect_timeout, Duration::from_secs(10));
    assert_eq!(settings.timeout, Duration::from_secs(30));
}

#[test]
fn production_client_rejects_http_before_connecting() {
    let client = super::http_client().unwrap();
    let error =
        tauri::async_runtime::block_on(async { client.get("http://127.0.0.1:9").send().await })
            .expect_err("production requests require HTTPS");
    assert!(error.is_builder(), "HTTP must fail before connecting");
}

#[test]
fn provider_redirects_never_send_credentials_to_a_second_host() {
    for status in [301, 302, 303, 307, 308] {
        let target = TestServer::start(200, br#"{"text":"redirect followed"}"#, Duration::ZERO);
        let redirect =
            TestServer::start_redirect(status, target.url.replace("127.0.0.1", "localhost"));
        let provider = ProviderId::ElevenLabsScribeV2;
        let request = super::build_request(
            provider,
            &test_client(),
            &vocabulary_request(None),
            "test-only-key",
            &redirect.url,
        )
        .unwrap();
        let result = tauri::async_runtime::block_on(super::execute_with_verdict(
            provider,
            request,
            &vocabulary_request(None),
            Instant::now(),
            super::VerdictMode::Strict,
        ));
        let original = redirect.received();
        assert!(String::from_utf8_lossy(&original).contains("xi-api-key:"));
        assert!(matches!(
            target.request.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        let error = result.expect_err("provider redirects must fail");
        assert_eq!(error.code, ProviderErrorCode::RequestFailed);
        assert_eq!(
            error.to_string(),
            "The transcription provider rejected the request."
        );
    }
}

#[test]
fn provider_ids_are_strict() {
    let expected = [
        "groq-whisper-turbo",
        "openai-gpt-4o-mini-transcribe",
        "xai-stt",
        "elevenlabs-scribe-v2",
        "deepgram-nova-3",
        "mistral-voxtral-3",
    ];

    for provider_id in expected {
        let parsed = ProviderId::try_from(provider_id).expect("provider should be supported");
        assert_eq!(parsed.as_str(), provider_id);
    }

    let error = ProviderId::try_from("groq-secret-alias").expect_err("alias must be rejected");
    assert_eq!(error.code, ProviderErrorCode::UnsupportedProvider);
}

#[test]
fn provider_http_errors_never_include_the_response_body() {
    let unauthorized = status_error(StatusCode::UNAUTHORIZED);
    assert_eq!(unauthorized.code, ProviderErrorCode::CredentialRejected);
    assert_eq!(
        unauthorized.to_string(),
        "The transcription provider rejected the API key."
    );

    let failure = status_error(StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(failure.code, ProviderErrorCode::RequestFailed);
    assert_eq!(
        failure.to_string(),
        "The transcription provider rejected the request."
    );
}

#[test]
fn groq_silent_segments_are_rejected() {
    let payload = json!({
        "text": "hallucinated phrase",
        "segments": [
            { "no_speech_prob": 0.91, "avg_logprob": -0.82 },
            { "no_speech_prob": 0.86, "avg_logprob": -0.79 }
        ]
    });

    for (verdict, expected) in [
        (super::VerdictMode::Strict, ProviderErrorCode::LowConfidence),
        (
            super::VerdictMode::CaptureSegment,
            ProviderErrorCode::Silence,
        ),
    ] {
        let error = parse_transcript(
            ProviderId::GroqWhisperTurbo,
            &payload,
            &vocabulary_request(None),
            verdict,
        )
        .expect_err("silent segments must be rejected");
        assert_eq!(error.code, expected);
    }
}

#[test]
fn groq_drops_only_trailing_skip_rule_segments_in_both_verdict_modes() {
    let speech = json!({"text": " Ship it now.", "no_speech_prob": 0.1, "avg_logprob": -0.2});
    let next = json!({"text": " Then test.", "no_speech_prob": 0.2, "avg_logprob": -0.3});
    let tail = json!({"text": " Thank you.", "no_speech_prob": 0.8, "avg_logprob": -1.2});
    let full_text = "Ship it now. Then test. Thank you.";
    for (name, provider, segments, expected) in [
        (
            "skip tail",
            ProviderId::GroqWhisperTurbo,
            vec![speech.clone(), next.clone(), tail.clone()],
            Some("Ship it now. Then test."),
        ),
        (
            "keep confident tail",
            ProviderId::GroqWhisperTurbo,
            vec![
                speech.clone(),
                next.clone(),
                json!({"text": " Thank you.", "no_speech_prob": 0.8, "avg_logprob": -0.3}),
            ],
            Some(full_text),
        ),
        (
            "keep threshold equality",
            ProviderId::GroqWhisperTurbo,
            vec![
                speech.clone(),
                next.clone(),
                json!({"text": " Thank you.", "no_speech_prob": 0.6, "avg_logprob": -1.0}),
            ],
            Some(full_text),
        ),
        (
            "keep interior",
            ProviderId::GroqWhisperTurbo,
            vec![speech.clone(), tail.clone(), next.clone()],
            Some(full_text),
        ),
        (
            "all skipped",
            ProviderId::GroqWhisperTurbo,
            vec![tail.clone(), tail.clone()],
            None,
        ),
        (
            "OpenAI unchanged",
            ProviderId::OpenAiTranscribe,
            vec![speech.clone(), next.clone(), tail.clone()],
            Some(full_text),
        ),
        (
            "skip multiple tails",
            ProviderId::GroqWhisperTurbo,
            vec![speech, next, tail.clone(), tail],
            Some("Ship it now. Then test."),
        ),
    ] {
        let payload = json!({"text": full_text, "segments": segments});
        for verdict in [
            super::VerdictMode::CaptureSegment,
            super::VerdictMode::Strict,
        ] {
            let result = parse_transcript(provider, &payload, &vocabulary_request(None), verdict);
            if let Some(expected) = expected {
                assert_eq!(result.unwrap(), expected, "{name}");
            } else {
                let code = match verdict {
                    super::VerdictMode::CaptureSegment => ProviderErrorCode::Silence,
                    super::VerdictMode::Strict => ProviderErrorCode::LowConfidence,
                };
                assert_eq!(result.unwrap_err().code, code, "{name}");
            }
        }
    }
}

/// A Groq reply whose top-level text is every segment's text, as the real API sends.
fn reply(segments: Vec<serde_json::Value>) -> serde_json::Value {
    let text: String = segments
        .iter()
        .filter_map(|segment| segment["text"].as_str())
        .collect();
    json!({"text": text, "segments": segments})
}

/// Square wave at the given RMS levels, as 16 kHz mono PCM16 WAV.
fn level_wav(parts: &[(f64, f32)]) -> Arc<[u8]> {
    let mut samples = Vec::new();
    for &(seconds, rms) in parts {
        let count = (seconds * 16_000.0).round() as usize;
        samples.extend((0..count).map(|index| if index % 2 == 0 { rms } else { -rms }));
    }
    Arc::from(crate::audio::encode_wav(&samples, 16_000, 1))
}

fn request_with_audio(audio: Arc<[u8]>, diagnostics: bool) -> TranscriptionRequest {
    TranscriptionRequest {
        audio,
        diagnostics,
        ..vocabulary_request(None)
    }
}

fn timed(text: &str, start: serde_json::Value, end: serde_json::Value) -> serde_json::Value {
    json!({"text": text, "start": start, "end": end, "no_speech_prob": 0.1, "avg_logprob": -0.2})
}

// Keep: the level rule on a Groq reply, in both verdict modes. Audio is synthetic PCM and
// the replies are canned, so this proves the rule, not the live Groq timestamps.
#[test]
fn groq_drops_a_trailing_segment_far_quieter_than_the_speech() {
    let ship = timed(" Ship it now.", json!(0.0), json!(2.0));
    let thanks = |start: f64, end: f64| timed(" Thank you.", json!(start), json!(end));
    let quiet_tail = level_wav(&[(2.0, 0.1), (1.0, 0.002)]);
    let loud_tail = level_wav(&[(2.0, 0.1), (1.0, 0.1)]);
    let whisper = level_wav(&[(3.0, 0.01)]);
    let below_floor = level_wav(&[(3.0, 0.002)]);
    let quiet_middle = level_wav(&[(1.0, 0.1), (1.1, 0.002), (0.9, 0.1)]);
    let middle_piece = level_wav(&[(2.0, 0.1), (0.5, 0.002)]);
    // A short real word after a long pause, stamped from the end of the previous segment.
    let after_pause = level_wav(&[(0.3, 0.1), (1.6, 0.002), (0.2, 0.1), (0.4, 0.002)]);
    // Speech followed by the segmenter's 400 ms quiet pad.
    let trimmed = level_wav(&[(2.0, 0.1), (0.4, 0.002)]);
    let kept = "Ship it now. Thank you.";
    let dropped = "Ship it now.";
    for (name, provider, audio, segments, expected) in [
        (
            "quiet tail after speech",
            ProviderId::GroqWhisperTurbo,
            quiet_tail.clone(),
            vec![ship.clone(), thanks(2.1, 3.0)],
            dropped,
        ),
        (
            "a soft but real closing word, 10 dB below speech",
            ProviderId::GroqWhisperTurbo,
            level_wav(&[(2.0, 0.1), (1.0, 0.03)]),
            vec![ship.clone(), thanks(2.1, 3.0)],
            kept,
        ),
        (
            "the same words at speech level",
            ProviderId::GroqWhisperTurbo,
            loud_tail,
            vec![ship.clone(), thanks(2.1, 3.0)],
            kept,
        ),
        (
            "uniformly quiet recording (whisper)",
            ProviderId::GroqWhisperTurbo,
            whisper,
            vec![ship.clone(), thanks(2.1, 3.0)],
            kept,
        ),
        (
            "recording below the signal floor",
            ProviderId::GroqWhisperTurbo,
            below_floor,
            vec![ship.clone(), thanks(2.1, 3.0)],
            kept,
        ),
        (
            "interior segments are never dropped",
            ProviderId::GroqWhisperTurbo,
            quiet_middle,
            vec![
                timed(" Ship it now.", json!(0.0), json!(1.0)),
                thanks(1.1, 2.0),
                timed(" Then test.", json!(2.1), json!(3.0)),
            ],
            "Ship it now. Thank you. Then test.",
        ),
        (
            "middle piece ending in its quiet window",
            ProviderId::GroqWhisperTurbo,
            middle_piece,
            vec![ship.clone(), thanks(2.0, 2.5)],
            dropped,
        ),
        (
            "only 200 ms of the window inside the audio",
            ProviderId::GroqWhisperTurbo,
            quiet_tail.clone(),
            vec![ship.clone(), thanks(2.8, 3.0)],
            kept,
        ),
        (
            "end past the audio is clamped to 300 ms inside",
            ProviderId::GroqWhisperTurbo,
            quiet_tail.clone(),
            vec![ship.clone(), thanks(2.7, 30.0)],
            dropped,
        ),
        (
            "the first segment is never dropped by level",
            ProviderId::GroqWhisperTurbo,
            quiet_tail.clone(),
            vec![timed(" Thank you.", json!(2.1), json!(3.0))],
            "Thank you.",
        ),
        (
            "two quiet trailing segments",
            ProviderId::GroqWhisperTurbo,
            quiet_tail.clone(),
            vec![ship.clone(), thanks(2.1, 2.5), thanks(2.5, 3.0)],
            dropped,
        ),
        (
            "a missing timestamp keeps the segment",
            ProviderId::GroqWhisperTurbo,
            quiet_tail.clone(),
            vec![ship.clone(), timed(" Thank you.", json!(null), json!(3.0))],
            kept,
        ),
        (
            "a short real word after a long pause is kept",
            ProviderId::GroqWhisperTurbo,
            after_pause,
            vec![
                timed(" Okay.", json!(0.0), json!(0.3)),
                timed(" Yes.", json!(0.3), json!(2.5)),
            ],
            "Okay. Yes.",
        ),
        (
            "a phantom overlapping the last word by 100 ms is dropped",
            ProviderId::GroqWhisperTurbo,
            trimmed.clone(),
            vec![ship.clone(), thanks(1.9, 3.0)],
            dropped,
        ),
        (
            // Documented limit: the rule cannot tell this phantom from a real closer.
            "a phantom overlapping the last word by 250 ms is kept",
            ProviderId::GroqWhisperTurbo,
            trimmed,
            vec![ship.clone(), thanks(1.75, 3.0)],
            kept,
        ),
        (
            "other providers keep their text",
            ProviderId::OpenAiTranscribe,
            quiet_tail,
            vec![ship, thanks(2.1, 3.0)],
            kept,
        ),
    ] {
        let payload = reply(segments);
        for verdict in [
            super::VerdictMode::CaptureSegment,
            super::VerdictMode::Strict,
        ] {
            let request = request_with_audio(Arc::clone(&audio), false);
            let result = parse_transcript(provider, &payload, &request, verdict);
            assert_eq!(result.unwrap(), expected, "{name}");
        }
    }
}

// Keep: the rule works through the real response path, not only parse_transcript.
#[test]
fn groq_reply_over_loopback_loses_its_quiet_trailing_segment() {
    let server = TestServer::start(
        200,
        br#"{"text":" Ship it now. Thank you.","segments":[
            {"text":" Ship it now.","start":0.0,"end":2.0,"no_speech_prob":0.1,"avg_logprob":-0.2},
            {"text":" Thank you.","start":2.1,"end":3.0,"no_speech_prob":0.1,"avg_logprob":-0.2}]}"#,
        Duration::ZERO,
    );
    let request = request_with_audio(level_wav(&[(2.0, 0.1), (1.0, 0.002)]), false);
    let provider_request =
        super::groq::build_request(&test_client(), &request, "test-only-key", &server.url).unwrap();
    let result = tauri::async_runtime::block_on(execute_with_verdict(
        ProviderId::GroqWhisperTurbo,
        provider_request,
        &request,
        Instant::now(),
        super::VerdictMode::CaptureSegment,
    ))
    .unwrap();
    assert_eq!(result.transcript, "Ship it now.");
}

// Keep: the Debug log record carries numbers only and appears only when Debug log is on.
// The log facade accepts one logger per process, so this runs in a child process.
#[test]
fn quiet_tail_check_is_logged_only_with_diagnostics_and_never_with_text() {
    const CHILD: &str = "ECHO_TEST_QUIET_TAIL_LOG_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        let status = clear_child_environment(&mut command)
            .env(CHILD, "1")
            .args([
                "--exact",
                "providers::tests::quiet_tail_check_is_logged_only_with_diagnostics_and_never_with_text",
                "--nocapture",
            ])
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    struct Records(std::sync::Mutex<Vec<String>>);
    static RECORDS: Records = Records(std::sync::Mutex::new(Vec::new()));
    impl log::Log for Records {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, record: &log::Record<'_>) {
            self.0.lock().unwrap().push(record.args().to_string());
        }
        fn flush(&self) {}
    }
    log::set_logger(&RECORDS).unwrap();
    log::set_max_level(log::LevelFilter::Info);

    let quiet_tail = level_wav(&[(2.0, 0.1), (1.0, 0.002)]);
    let loud_tail = level_wav(&[(2.0, 0.1), (1.0, 0.1)]);
    let below_floor = level_wav(&[(3.0, 0.002)]);
    let ship = timed(" Ship it now.", json!(0.0), json!(2.0));
    let thanks = |start: f64, end: f64| timed(" Thank you.", json!(start), json!(end));
    let skipped = json!({"text": " Thank you.", "start": 2.1, "end": 3.0,
        "no_speech_prob": 0.9, "avg_logprob": -2.0});
    let parse = |diagnostics: bool, audio: &Arc<[u8]>, segments: Vec<serde_json::Value>| {
        let request = request_with_audio(Arc::clone(audio), diagnostics);
        parse_transcript(
            ProviderId::GroqWhisperTurbo,
            &reply(segments),
            &request,
            super::VerdictMode::CaptureSegment,
        )
        .unwrap();
    };
    let records = || RECORDS.0.lock().unwrap().clone();
    let head = "quiet_tail_check provider=groq-whisper-turbo";

    parse(false, &quiet_tail, vec![ship.clone(), thanks(2.1, 3.0)]);
    assert!(records().is_empty(), "diagnostics off must not log");

    // One record per reply of two or more segments; each describes the segment that
    // settled the decision, with "-" for what could not be measured.
    for (name, audio, segments, expected) in [
        (
            "quiet tail dropped",
            &quiet_tail,
            vec![ship.clone(), thanks(2.1, 3.0)],
            "decision=dropped reason=quiet segments=2 dropped=1 start_s=2.10 end_s=3.00 \
             inside_ms=900 loud_ms=0 speech_rms=0.1000 tail_peak_rms=0.0020 ratio=0.020",
        ),
        (
            "speech-level tail kept",
            &loud_tail,
            vec![ship.clone(), thanks(2.1, 3.0)],
            "decision=kept reason=speech segments=2 dropped=0 start_s=2.10 end_s=3.00 \
             inside_ms=900 loud_ms=900 speech_rms=0.1000 tail_peak_rms=0.1000 ratio=1.000",
        ),
        (
            "window too short",
            &quiet_tail,
            vec![ship.clone(), thanks(2.8, 3.0)],
            "decision=kept reason=short segments=2 dropped=0 start_s=2.80 end_s=3.00 \
             inside_ms=200 loud_ms=- speech_rms=- tail_peak_rms=- ratio=-",
        ),
        (
            "no timestamps",
            &quiet_tail,
            vec![ship.clone(), timed(" Thank you.", json!(null), json!(3.0))],
            "decision=kept reason=no_timestamps segments=2 dropped=0 start_s=- end_s=3.00 \
             inside_ms=- loud_ms=- speech_rms=- tail_peak_rms=- ratio=-",
        ),
        (
            "recording below the floor",
            &below_floor,
            vec![ship.clone(), thanks(2.1, 3.0)],
            "decision=kept reason=below_floor segments=2 dropped=0 start_s=2.10 end_s=3.00 \
             inside_ms=900 loud_ms=- speech_rms=- tail_peak_rms=- ratio=-",
        ),
        (
            "skip rule",
            &quiet_tail,
            vec![ship.clone(), skipped],
            "decision=dropped reason=skip_rule segments=2 dropped=1 start_s=2.10 end_s=3.00 \
             inside_ms=- loud_ms=- speech_rms=- tail_peak_rms=- ratio=-",
        ),
    ] {
        let before = records().len();
        parse(true, audio, segments);
        let new = records().split_off(before);
        let [record] = new.try_into().expect("exactly one record");
        assert_eq!(record, format!("{head} {expected}"), "{name}");
        assert!(
            !record.contains("Thank") && !record.contains("Ship"),
            "{name}"
        );
    }

    // A single segment has no trailing scan to report.
    let before = records().len();
    parse(true, &quiet_tail, vec![thanks(2.1, 3.0)]);
    assert_eq!(records().len(), before);
}

#[test]
fn provider_response_requires_non_empty_text() {
    let empty = parse_transcript(
        ProviderId::OpenAiTranscribe,
        &json!({ "text": " " }),
        &vocabulary_request(None),
        super::VerdictMode::Strict,
    )
    .expect_err("blank transcript must fail");
    assert_eq!(empty.code, ProviderErrorCode::EmptyTranscript);

    let transcript = parse_transcript(
        ProviderId::XaiStt,
        &json!({ "text": "  hello  " }),
        &vocabulary_request(None),
        super::VerdictMode::Strict,
    )
    .expect("text should parse");
    assert_eq!(transcript, "hello");
}

#[test]
fn invalid_audio_mime_is_rejected_before_network_use() {
    let request = TranscriptionRequest {
        audio: Arc::from([1_u8, 2, 3]),
        mime_type: "not a mime".to_string(),
        file_name: "capture.wav".to_string(),
        language: None,
        prompt: None,
        keyterms: Vec::new(),
        diagnostics: false,
    };

    let error = multipart_file(&request).expect_err("invalid MIME type must fail");
    assert_eq!(error.code, ProviderErrorCode::InvalidAudio);
}

#[test]
fn segments_pair_wav_multipart_preserves_selected_pcm_and_frozen_fields() {
    use crate::audio::tests::fixtures::{begin, owner, spawn_coordinator, FakeBackend};
    use crate::audio::{
        ArtifactReleaseDisposition, ArtifactReleaseRequest, AudioCapturePolicy, StopCapture,
    };

    let (backend, input) = FakeBackend::continuous();
    let capture = spawn_coordinator(
        backend,
        AudioCapturePolicy {
            minimum_capture_duration: Duration::ZERO,
            minimum_sample_count: 0,
            ..Default::default()
        },
    );
    let owner = owner("overlay");
    let session = begin(&capture, owner.clone(), "multipart-whole");
    input.feed(&capture, 0.2, 2_000);
    input.feed(&capture, 0.0, 500);
    input.feed(&capture, 0.3, 2_000);
    input.feed(&capture, 0.0, 500);
    input.feed(&capture, 0.4, 500);
    let metadata = capture
        .stop(StopCapture {
            owner: owner.clone(),
            intent_id: "multipart-whole".into(),
            session_id: session.session_id,
        })
        .unwrap();
    let lease = capture
        .lease_artifact(owner.clone(), metadata.artifact_id.clone())
        .unwrap();
    let audio = capture
        .read_artifact(
            owner.clone(),
            metadata.artifact_id.clone(),
            lease.lease_id.clone(),
        )
        .unwrap();
    assert_eq!(audio.segments.len(), 3);
    for (wav, selected) in [
        (audio.pair_wav(1).unwrap(), &audio.segments[1..3]),
        (audio.whole_wav(), &audio.segments[..]),
    ] {
        let expected_pcm = selected
            .iter()
            .filter_map(|p| p.bytes.as_ref())
            .flat_map(|p| p[44..].iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(&wav[44..], expected_pcm);
        assert_eq!(
            u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize,
            wav.len() - 8
        );
        assert_eq!(
            u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize,
            expected_pcm.len()
        );
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        let server = TestServer::start(200, b"{\"text\":\"whole transcript\"}", Duration::ZERO);
        let prompt =
            "Custom instruction\n\nThis is English dictation for software and product work. Use these exact spellings when heard: Sample, Echo.";
        let request = TranscriptionRequest {
            audio: Arc::clone(&wav),
            mime_type: "audio/wav".into(),
            file_name: "echo-dictation.wav".into(),
            language: Some("en".into()),
            prompt: Some(prompt.into()),
            keyterms: Vec::new(),
            diagnostics: false,
        };
        let provider_request =
            super::groq::build_request(&test_client(), &request, "test-only-key", &server.url)
                .unwrap();
        let result = tauri::async_runtime::block_on(execute_with_verdict(
            ProviderId::GroqWhisperTurbo,
            provider_request,
            &request,
            Instant::now(),
            super::VerdictMode::Strict,
        ))
        .unwrap();
        assert_eq!(result.transcript, "whole transcript");
        let received = server.received();
        let start = find_bytes(&received, b"RIFF").expect("whole WAV must reach multipart upload");
        assert_eq!(&received[start..start + wav.len()], wav.as_ref());
        assert_eq!(
            received.windows(4).filter(|part| *part == b"RIFF").count(),
            1
        );
        let multipart = String::from_utf8_lossy(&received);
        for field in [
            "filename=\"echo-dictation.wav\"",
            "Content-Type: audio/wav",
            "name=\"model\"\r\n\r\nwhisper-large-v3-turbo",
            "name=\"language\"\r\n\r\nen",
            prompt,
        ] {
            assert!(
                multipart.contains(field),
                "missing multipart field: {field}"
            );
        }
    }
    capture
        .release_artifact(ArtifactReleaseRequest {
            owner,
            artifact_id: metadata.artifact_id,
            lease_id: lease.lease_id,
            disposition: ArtifactReleaseDisposition::Discard,
        })
        .unwrap();
}

#[test]
fn response_body_timeout_keeps_timeout_classification() {
    let server = TestServer::start(200, b"{\"text\":\"late\"}", Duration::from_millis(250));
    let client = test_client();

    let error = tauri::async_runtime::block_on(execute_with_verdict(
        ProviderId::OpenAiTranscribe,
        client.get(&server.url).timeout(Duration::from_millis(75)),
        &vocabulary_request(None),
        Instant::now(),
        super::VerdictMode::Strict,
    ))
    .expect_err("delayed body must time out");

    assert_eq!(error.code, ProviderErrorCode::Timeout);
}

#[test]
fn declared_oversized_response_is_rejected_without_buffering() {
    let server =
        TestServer::start_with_length(200, b"", MAX_PROVIDER_RESPONSE_BYTES + 1, Duration::ZERO);

    let error = tauri::async_runtime::block_on(execute_with_verdict(
        ProviderId::XaiStt,
        test_client().get(&server.url),
        &vocabulary_request(None),
        Instant::now(),
        super::VerdictMode::Strict,
    ))
    .expect_err("oversized response must fail");

    assert_eq!(error.code, ProviderErrorCode::InvalidResponse);
}

#[test]
fn loopback_status_errors_do_not_expose_response_body() {
    let server = TestServer::start(500, b"private provider body", Duration::ZERO);
    let error = tauri::async_runtime::block_on(execute_with_verdict(
        ProviderId::GroqWhisperTurbo,
        test_client().get(&server.url),
        &vocabulary_request(None),
        Instant::now(),
        super::VerdictMode::Strict,
    ))
    .expect_err("provider status must fail");

    assert_eq!(error.code, ProviderErrorCode::RequestFailed);
    assert!(!error.to_string().contains("private provider body"));
}

fn vocabulary_request(language: Option<&str>) -> TranscriptionRequest {
    TranscriptionRequest {
        audio: Arc::from(*b"ECHO_AUDIO"),
        mime_type: "audio/wav".into(),
        file_name: "capture.wav".into(),
        language: language.map(str::to_owned),
        prompt: Some("Custom instruction".into()),
        keyterms: vec!["Sample".into(), "a<b".into(), "x\ny".into()],
        diagnostics: false,
    }
}

// Keep: caps apply after invalid terms are dropped, and Unicode length counts characters.
#[test]
fn vocabulary_rules_drop_only_invalid_terms_and_preserve_order() {
    for (rules, terms, expected) in [
        (
            &super::openai::VOCABULARY_RULES,
            vec![
                "Sample".into(),
                "a<b".into(),
                "a>b".into(),
                "x\ny".into(),
                "x\ry".into(),
                "Echo".into(),
            ],
            vec!["Sample".into(), "Echo".into()],
        ),
        (
            &super::xai::VOCABULARY_RULES,
            vec!["a".repeat(51), "é".repeat(50), "a".repeat(50), "z".into()],
            vec!["é".repeat(50), "a".repeat(50), "z".into()],
        ),
        (
            &super::xai::VOCABULARY_RULES,
            std::iter::once("a".repeat(51))
                .chain((0..101).map(|i| format!("term{i}")))
                .collect(),
            (0..100).map(|i| format!("term{i}")).collect(),
        ),
        (
            &super::elevenlabs::VOCABULARY_RULES,
            vec![
                "one two three four five six".into(),
                "a<b".into(),
                "a>b".into(),
                "a{b".into(),
                "a}b".into(),
                "a[b".into(),
                "a]b".into(),
                "a\\b".into(),
                "a".repeat(51),
                "é".repeat(50),
                "one two three four five".into(),
            ],
            vec!["é".repeat(50), "one two three four five".into()],
        ),
    ] {
        assert_eq!(
            super::vocabulary(&terms, rules).collect::<Vec<_>>(),
            expected.iter().map(String::as_str).collect::<Vec<_>>()
        );
    }
    let terms: Vec<_> = (0..101).map(|i| format!("term{i}")).collect();
    for rules in [
        &super::elevenlabs::VOCABULARY_RULES,
        &super::deepgram::VOCABULARY_RULES,
        &super::mistral::VOCABULARY_RULES,
    ] {
        assert_eq!(
            super::vocabulary(&terms, rules).collect::<Vec<_>>(),
            terms[..100].iter().map(String::as_str).collect::<Vec<_>>()
        );
    }
}

/// Sends `request` to a loopback server as `provider` and returns the raw bytes received.
fn sent_request(provider: ProviderId, request: &TranscriptionRequest) -> Vec<u8> {
    let server = TestServer::start(200, b"{\"text\":\"ok\"}", Duration::ZERO);
    // The reply only has to be accepted; Deepgram's transcript sits at another path, so
    // its verdict is ignored here.
    let _ = tauri::async_runtime::block_on(execute_with_verdict(
        provider,
        super::build_request(
            provider,
            &test_client(),
            request,
            "test-only-key",
            &server.url,
        )
        .unwrap(),
        request,
        Instant::now(),
        super::VerdictMode::Strict,
    ));
    server.received()
}

/// The values a request carries in its language fields: the multipart parts `language`,
/// `languages[]` and `language_code`, or Deepgram's `language` query parameter.
fn language_values(provider: ProviderId, received: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(received);
    if provider == ProviderId::DeepgramNova3 {
        let target = text
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap();
        let url = reqwest::Url::parse(&format!("http://localhost{target}")).unwrap();
        return url
            .query_pairs()
            .filter(|(key, _)| key == "language")
            .map(|(_, value)| value.into_owned())
            .collect();
    }
    ["language", "languages[]", "language_code"]
        .into_iter()
        .flat_map(|field| {
            let marker = format!("name=\"{field}\"\r\n\r\n");
            text.match_indices(&marker)
                .map(|(at, _)| {
                    let rest = &text[at + marker.len()..];
                    rest[..rest.find("\r\n").unwrap()].to_owned()
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

// Keep: Auto-detect (no saved language) sends no language to any provider, never English,
// and every saved tag reaches each provider as it did before. Deepgram alone, which
// assumes English when the language is omitted, gets its multilingual mode instead.
#[test]
fn adapter_language_parts_follow_saved_tags_and_auto_detect() {
    const NONE: [&[&str]; 5] = [&[], &[], &[], &[], &[]];
    // Saved language, then what Groq, OpenAI, xAI, ElevenLabs and Mistral send, then
    // Deepgram, then whether xAI also sends `format` (it requires a language).
    #[allow(clippy::type_complexity)]
    let rows: [(Option<&str>, [&[&str]; 5], &[&str], bool); 10] = [
        (None, NONE, &["multi"], false),
        (Some(""), NONE, &["multi"], false),
        (Some("  "), NONE, &["multi"], false),
        (
            Some("en"),
            [&["en"], &["en"], &["en"], &["en"], &["en"]],
            &["en"],
            true,
        ),
        (
            Some("en-US"),
            [&["en-US"], &["en"], &["en"], &["en"], &["en"]],
            &["en-US"],
            true,
        ),
        (
            Some("pt-BR"),
            [&["pt-BR"], &["pt"], &["pt"], &["pt"], &["pt"]],
            &["pt-BR"],
            true,
        ),
        (
            Some("es"),
            [&["es"], &["es"], &["es"], &["es"], &["es"]],
            &["es"],
            true,
        ),
        // No two-letter primary subtag: the primary-subtag providers send nothing.
        (Some("fil"), [&["fil"], &[], &[], &[], &[]], &["fil"], false),
        (Some("12"), [&["12"], &[], &[], &[], &[]], &["12"], false),
        // Two bytes long but not ASCII letters: not a usable primary subtag either.
        (Some("é"), [&["é"], &[], &[], &[], &[]], &["é"], false),
    ];
    assert_eq!(super::primary_language(&vocabulary_request(None)), None);
    for (language, [groq, openai, xai, elevenlabs, mistral], deepgram, xai_format) in rows {
        let request = vocabulary_request(language);
        for (provider, expected) in [
            (ProviderId::GroqWhisperTurbo, groq),
            (ProviderId::OpenAiTranscribe, openai),
            (ProviderId::XaiStt, xai),
            (ProviderId::ElevenLabsScribeV2, elevenlabs),
            (ProviderId::DeepgramNova3, deepgram),
            (ProviderId::MistralVoxtral3, mistral),
        ] {
            let received = sent_request(provider, &request);
            let values = language_values(provider, &received);
            assert_eq!(values, expected, "{language:?} to {}", provider.as_str());
            if language.is_none_or(|tag| tag.trim().is_empty()) {
                assert!(
                    !values
                        .iter()
                        .any(|value| value.eq_ignore_ascii_case("en") || value.starts_with("en-")),
                    "{} sent English for Auto-detect",
                    provider.as_str()
                );
            }
            if provider == ProviderId::XaiStt {
                let text = String::from_utf8_lossy(&received);
                if xai_format {
                    assert!(
                        text.contains("name=\"format\"\r\n\r\ntrue\r\n"),
                        "{language:?} xAI must send format=true"
                    );
                } else {
                    assert!(
                        !text.contains("name=\"format\""),
                        "{language:?} xAI must not send format"
                    );
                }
                // The rest of the request still goes out.
                assert!(text.contains("name=\"keyterm\"\r\n\r\nSample"));
            }
        }
    }
}

#[test]
fn upgraded_adapters_preserve_empty_and_credential_errors_without_body_text() {
    for provider in ProviderId::ALL
        .into_iter()
        .filter(|id| *id != ProviderId::GroqWhisperTurbo)
    {
        for (status, body, code) in [
            (
                401,
                b"private provider body".as_slice(),
                ProviderErrorCode::CredentialRejected,
            ),
            (
                200,
                b"{\"text\":\" \"}".as_slice(),
                ProviderErrorCode::EmptyTranscript,
            ),
        ] {
            let server = TestServer::start(status, body, Duration::ZERO);
            let error = tauri::async_runtime::block_on(execute_with_verdict(
                provider,
                super::build_request(
                    provider,
                    &test_client(),
                    &vocabulary_request(None),
                    "test-only-key",
                    &server.url,
                )
                .unwrap(),
                &vocabulary_request(None),
                Instant::now(),
                super::VerdictMode::Strict,
            ))
            .unwrap_err();
            assert_eq!(error.code, code);
            assert!(!error.to_string().contains("private provider body"));
        }
    }
}

struct TestServer {
    url: String,
    request: mpsc::Receiver<Vec<u8>>,
}

impl TestServer {
    fn start(status: u16, body: &'static [u8], body_delay: Duration) -> Self {
        Self::start_with_fixture(TestResponse {
            status,
            body,
            content_length: body.len(),
            body_delay,
            location: None,
        })
    }

    fn start_redirect(status: u16, location: String) -> Self {
        Self::start_with_fixture(TestResponse {
            status,
            body: b"",
            content_length: 0,
            body_delay: Duration::ZERO,
            location: Some(location),
        })
    }

    fn start_with_length(
        status: u16,
        body: &'static [u8],
        content_length: usize,
        body_delay: Duration,
    ) -> Self {
        Self::start_with_fixture(TestResponse {
            status,
            body,
            content_length,
            body_delay,
            location: None,
        })
    }

    fn start_with_fixture(response: TestResponse) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener should bind");
        let address = listener
            .local_addr()
            .expect("listener should have an address");
        let (send_request, request) = mpsc::sync_channel(1);
        thread::spawn(move || serve_once(listener, response, send_request));
        Self {
            url: format!("http://{address}"),
            request,
        }
    }

    fn received(self) -> Vec<u8> {
        self.request
            .recv_timeout(Duration::from_secs(1))
            .expect("server should capture request")
    }
}

struct TestResponse {
    status: u16,
    body: &'static [u8],
    content_length: usize,
    body_delay: Duration,
    location: Option<String>,
}

fn serve_once(
    listener: TcpListener,
    response: TestResponse,
    request_sink: mpsc::SyncSender<Vec<u8>>,
) {
    let (mut stream, _) = listener.accept().expect("loopback request should connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("read timeout should apply");
    let request = read_http_request(&mut stream);
    let reason = if response.status == 200 {
        "OK"
    } else {
        "Error"
    };
    write!(
        stream,
        "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n",
        response.status,
        response.content_length,
        response
            .location
            .as_ref()
            .map(|url| format!("Location: {url}\r\n"))
            .unwrap_or_default()
    )
    .expect("response headers should write");
    stream.flush().expect("response headers should flush");
    thread::sleep(response.body_delay);
    let _ = stream.write_all(response.body);
    let _ = request_sink.send(request);
}

fn read_http_request(stream: &mut impl Read) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let Ok(read) = stream.read(&mut buffer) else {
            break;
        };
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = find_bytes(&request, b"\r\n\r\n") else {
            continue;
        };
        let content_length = parse_content_length(&request[..header_end]);
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    request
}

fn parse_content_length(headers: &[u8]) -> usize {
    String::from_utf8_lossy(headers)
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .map(str::to_owned)
        })
        .and_then(|value| value.parse().ok())
        .unwrap_or_default()
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[test]
fn segments_groq_no_speech_is_distinct_from_low_confidence() {
    for segments in [
        json!([]),
        json!([{ "no_speech_prob": 0.6 }]),
        json!([{ "no_speech_prob": 1.0 }, { "no_speech_prob": 0.5 }]),
    ] {
        let error =
            super::groq::reject_low_confidence(&json!({ "text": "silence", "segments": segments }))
                .unwrap_err();
        assert_eq!(error.code, ProviderErrorCode::Silence);
    }
}

#[test]
fn segments_groq_capture_keeps_uncertain_text_but_whole_dictation_stays_strict() {
    let payload =
        json!({"text":"uncertain speech", "segments":[{"no_speech_prob":0.1, "avg_logprob":-1.0}]});
    assert_eq!(
        super::parse_transcript(
            ProviderId::GroqWhisperTurbo,
            &payload,
            &vocabulary_request(None),
            super::VerdictMode::CaptureSegment
        )
        .unwrap(),
        "uncertain speech"
    );
    let error = parse_transcript(
        ProviderId::GroqWhisperTurbo,
        &payload,
        &vocabulary_request(None),
        super::VerdictMode::Strict,
    )
    .unwrap_err();
    assert_eq!(error.code, ProviderErrorCode::LowConfidence);
}

#[test]
fn segments_groq_edge_evidence_uses_only_first_and_last_segments() {
    let payload = json!({"avg_logprob": -0.1, "no_speech_prob": 0.0, "segments": [
        {"avg_logprob": -0.2, "no_speech_prob": 0.1},
        {"avg_logprob": -3.0, "no_speech_prob": 1.0},
        {"avg_logprob": -0.3, "no_speech_prob": 0.2}
    ]});
    let edges = super::groq::edge_confidence(&payload).unwrap();
    assert!(edges.first.unwrap().confident());
    assert!(edges.last.unwrap().confident());
    for segment in [
        json!({}),
        json!({"avg_logprob":"bad", "no_speech_prob":0.1}),
        json!({"avg_logprob":-0.75,"no_speech_prob":0.1}),
        json!({"avg_logprob":-0.1,"no_speech_prob":0.60}),
    ] {
        let edges = super::groq::edge_confidence(&json!({"segments":[segment]})).unwrap();
        assert!(!edges.first.is_some_and(|edge| edge.confident()));
        assert!(!edges.last.is_some_and(|edge| edge.confident()));
    }
    assert!(super::groq::edge_confidence(&json!({})).is_none());
}

#[test]
fn deepgram_reads_only_its_transcript_path_and_preserves_empty_verdict() {
    for (text, expected) in [
        (" hi ", Ok("hi".to_owned())),
        ("", Err(ProviderErrorCode::EmptyTranscript)),
    ] {
        let payload = json!({"text": "wrong path", "results": {"channels": [{"alternatives": [{"transcript": text}]}]}});
        assert_eq!(
            parse_transcript(
                ProviderId::DeepgramNova3,
                &payload,
                &vocabulary_request(None),
                super::VerdictMode::Strict
            )
            .map_err(|error| error.code),
            expected
        );
    }
}

struct AdapterCase {
    provider: ProviderId,
    request: TranscriptionRequest,
    model: &'static str,
    verify: fn(&[u8], &TranscriptionRequest),
}

#[test]
fn adapters_send_each_provider_request_contract() {
    let cases = [
        {
            let language = Some("en-US");
            let request = vocabulary_request(language);
            AdapterCase {
                provider: ProviderId::OpenAiTranscribe,
                request,
                model: "gpt-transcribe",
                verify: openai_adapter_sends_transcribe_language_and_keywords,
            }
        },
        {
            let language = None;
            let mut request = vocabulary_request(language);
            request.keyterms = vec!["Sample".into(), "a".repeat(51)];
            AdapterCase {
                provider: ProviderId::XaiStt,
                request,
                model: "grok-voice-transcribe-2.0",
                verify: xai_adapter_sends_model_vocabulary_and_file_last_without_prompt_or_language,
            }
        },
        {
            let language = Some("pt-BR");
            let mut request = vocabulary_request(language);
            request.keyterms = vec!["Sample".into(), "a".repeat(51)];
            AdapterCase {
                provider: ProviderId::XaiStt,
                request,
                model: "grok-voice-transcribe-2.0",
                verify: xai_adapter_sends_format_language_vocabulary_and_file_last_without_prompt,
            }
        },
        {
            let request = TranscriptionRequest {
                audio: Arc::from(*b"ECHO_AUDIO"),
                mime_type: "audio/wav".to_string(),
                file_name: "capture.wav".to_string(),
                language: None,
                prompt: Some("Sample Echo".to_string()),
                keyterms: Vec::new(),
                diagnostics: false,
            };
            AdapterCase {
                provider: ProviderId::GroqWhisperTurbo,
                request,
                model: "whisper-large-v3-turbo",
                verify: groq_adapter_serializes_prompt_without_language,
            }
        },
        {
            let language = Some("pt-BR");
            let mut request = vocabulary_request(language);
            request.keyterms = vec![
                "Sample".into(),
                "a{b}".into(),
                "one two three four five six".into(),
            ];
            AdapterCase {
                provider: ProviderId::ElevenLabsScribeV2,
                request,
                model: "scribe_v2",
                verify: elevenlabs_adapter_sends_filtered_keyterms_and_primary_language,
            }
        },
        {
            let language = Some("en-US");
            let mut request = vocabulary_request(language);
            request.keyterms = vec!["Sample Echo".into(), "a&b+#?".into()];
            AdapterCase {
                provider: ProviderId::DeepgramNova3,
                request,
                model: "nova-3",
                verify: deepgram_adapter_sends_encoded_query_and_exact_raw_audio,
            }
        },
        {
            let language = None;
            let mut request = vocabulary_request(language);
            request.keyterms = vec!["Sample Echo".into(), "a&b+#?".into()];
            AdapterCase {
                provider: ProviderId::DeepgramNova3,
                request,
                model: "nova-3",
                verify: deepgram_adapter_sends_encoded_query_and_exact_raw_audio,
            }
        },
        {
            let language = Some("pt-BR");
            let mut request = vocabulary_request(language);
            request.keyterms = vec!["Sample".into(), "Echo".into()];
            AdapterCase {
                provider: ProviderId::MistralVoxtral3,
                request,
                model: "voxtral-mini-transcribe-3-0",
                verify: mistral_adapter_sends_context_bias_and_omits_non_two_letter_language,
            }
        },
        {
            let language = Some("fil");
            let mut request = vocabulary_request(language);
            request.keyterms = vec!["Sample".into(), "Echo".into()];
            AdapterCase {
                provider: ProviderId::MistralVoxtral3,
                request,
                model: "voxtral-mini-transcribe-3-0",
                verify: mistral_adapter_sends_context_bias_and_omits_non_two_letter_language,
            }
        },
    ];
    for case in cases {
        let (body, transcript): (&'static [u8], _) = match case.provider {
            ProviderId::DeepgramNova3 => (
                br#"{"results":{"channels":[{"alternatives":[{"transcript":"hi"}]}]}}"#,
                "hi",
            ),
            ProviderId::ElevenLabsScribeV2 | ProviderId::MistralVoxtral3 => {
                (br#"{"text":"hi"}"#, "hi")
            }
            _ => (br#"{"text":"ok"}"#, "ok"),
        };
        let server = TestServer::start(200, body, Duration::ZERO);
        let result = tauri::async_runtime::block_on(execute_with_verdict(
            case.provider,
            super::build_request(
                case.provider,
                &test_client(),
                &case.request,
                "test-only-key",
                &server.url,
            )
            .unwrap(),
            &case.request,
            Instant::now(),
            super::VerdictMode::Strict,
        ))
        .unwrap();
        assert_eq!(result.transcript, transcript);
        assert_eq!(result.provider_model, case.model);
        (case.verify)(&server.received(), &case.request);
    }
}

fn openai_adapter_sends_transcribe_language_and_keywords(
    bytes: &[u8],
    _request: &TranscriptionRequest,
) {
    let received = String::from_utf8_lossy(bytes);
    assert!(received.starts_with("POST /v1/audio/transcriptions HTTP/1.1"));
    assert!(received.contains("authorization: Bearer test-only-key\r\n"));
    for field in [
        "name=\"model\"\r\n\r\ngpt-transcribe",
        "name=\"languages[]\"\r\n\r\nen",
        "name=\"keywords[]\"\r\n\r\nSample",
        "name=\"prompt\"\r\n\r\nCustom instruction",
        "filename=\"capture.wav\"",
        "Content-Type: audio/wav",
        "ECHO_AUDIO",
    ] {
        assert!(received.contains(field), "missing field {field}");
    }
    assert_eq!(received.matches("name=\"keywords[]\"").count(), 1);
    assert_eq!(
        received
            .matches("name=\"keywords[]\"\r\n\r\nSample\r\n")
            .count(),
        1
    );
    assert!(!received.contains("name=\"language\""));
    assert!(!received.contains("a<b"));
    assert!(!received.contains("x\ny"));
}

/// Checks what every xAI request carries: endpoint, credential, the given parts in this
/// order with the file last, no prompt, and only the valid keyterm.
fn assert_xai_parts_in_order(bytes: &[u8], fields: &[&str]) -> String {
    let received = String::from_utf8_lossy(bytes).into_owned();
    assert!(received.starts_with("POST /v1/stt HTTP/1.1"));
    assert!(received.contains("authorization: Bearer test-only-key\r\n"));
    let mut previous = 0;
    for field in fields {
        let position = received
            .find(field)
            .unwrap_or_else(|| panic!("missing field {field}"));
        assert!(position > previous);
        previous = position;
    }
    assert!(!received[previous..].contains("\r\nContent-Disposition:"));
    assert!(!received.contains("name=\"prompt\""));
    assert!(!received.contains(&"a".repeat(51)));
    assert_eq!(received.matches("name=\"keyterm\"").count(), 1);
    received
}

fn xai_adapter_sends_model_vocabulary_and_file_last_without_prompt_or_language(
    bytes: &[u8],
    _request: &TranscriptionRequest,
) {
    let received = assert_xai_parts_in_order(
        bytes,
        &[
            "name=\"model\"\r\n\r\ngrok-voice-transcribe-2.0",
            "name=\"keyterm\"\r\n\r\nSample",
            "name=\"file\"; filename=\"capture.wav\"",
        ],
    );
    // Auto-detect: xAI rejects `format` without `language`, so neither is sent.
    assert!(!received.contains("name=\"format\""));
    assert!(!received.contains("name=\"language\""));
}

fn xai_adapter_sends_format_language_vocabulary_and_file_last_without_prompt(
    bytes: &[u8],
    _request: &TranscriptionRequest,
) {
    assert_xai_parts_in_order(
        bytes,
        &[
            "name=\"model\"\r\n\r\ngrok-voice-transcribe-2.0",
            "name=\"format\"\r\n\r\ntrue",
            "name=\"language\"\r\n\r\npt",
            "name=\"keyterm\"\r\n\r\nSample",
            "name=\"file\"; filename=\"capture.wav\"",
        ],
    );
}

fn groq_adapter_serializes_prompt_without_language(bytes: &[u8], _request: &TranscriptionRequest) {
    let received = String::from_utf8_lossy(bytes);
    assert!(!received.contains("name=\"language\""));
    assert!(received.contains("name=\"prompt\"\r\n\r\nSample Echo"));
    assert!(received.contains("name=\"response_format\"\r\n\r\nverbose_json"));
}

fn elevenlabs_adapter_sends_filtered_keyterms_and_primary_language(
    bytes: &[u8],
    _request: &TranscriptionRequest,
) {
    let received = String::from_utf8_lossy(bytes);
    assert!(received.starts_with("POST /v1/speech-to-text HTTP/1.1"));
    assert!(received.contains("xi-api-key: test-only-key\r\n"));
    assert!(!received.contains("authorization:"));
    for field in [
        "name=\"model_id\"\r\n\r\nscribe_v2",
        "name=\"language_code\"\r\n\r\npt",
        "name=\"keyterms\"\r\n\r\nSample",
        "ECHO_AUDIO",
    ] {
        assert!(received.contains(field), "missing {field}");
    }
    assert_eq!(received.matches("name=\"keyterms\"").count(), 1);
    assert!(!received.contains("a{b}"));
    assert!(!received.contains("one two three four five six"));
    assert!(!received.contains("name=\"prompt\""));
}

fn deepgram_adapter_sends_encoded_query_and_exact_raw_audio(
    bytes: &[u8],
    request: &TranscriptionRequest,
) {
    let language = request.language.as_deref();
    let header_end = find_bytes(bytes, b"\r\n\r\n").unwrap();
    assert_eq!(&bytes[header_end + 4..], request.audio.as_ref());
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let target = headers
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap();
    let url = reqwest::Url::parse(&format!("http://localhost{target}")).unwrap();
    assert_eq!(url.path(), "/v1/listen");
    let pairs = url.query_pairs().collect::<Vec<_>>();
    assert_eq!(
        pairs
            .iter()
            .map(|(key, value)| (key.as_ref(), value.as_ref()))
            .collect::<Vec<_>>(),
        [
            ("model", "nova-3"),
            ("smart_format", "true"),
            ("language", language.unwrap_or("multi")),
            ("keyterm", "Sample Echo"),
            ("keyterm", "a&b+#?")
        ]
    );
    assert!(!target.contains("test-only-key"));
    assert!(headers.contains("authorization: Token test-only-key"));
    assert!(headers.contains("content-type: audio/wav"));
    assert!(!headers.contains("multipart"));
}

fn mistral_adapter_sends_context_bias_and_omits_non_two_letter_language(
    bytes: &[u8],
    request: &TranscriptionRequest,
) {
    let received = String::from_utf8_lossy(bytes);
    assert!(received.starts_with("POST /v1/audio/transcriptions HTTP/1.1"));
    assert!(received.contains("authorization: Bearer test-only-key\r\n"));
    for field in [
        "name=\"model\"\r\n\r\nvoxtral-mini-transcribe-3-0",
        "name=\"context_bias\"\r\n\r\nSample",
        "name=\"context_bias\"\r\n\r\nEcho",
        "ECHO_AUDIO",
    ] {
        assert!(received.contains(field), "missing {field}");
    }
    assert_eq!(received.matches("name=\"context_bias\"").count(), 2);
    if request.language.as_deref() == Some("pt-BR") {
        assert!(received.contains("name=\"language\"\r\n\r\npt"));
    } else {
        assert!(!received.contains("name=\"language\""));
    }
    assert!(!received.contains("name=\"prompt\""));
}
