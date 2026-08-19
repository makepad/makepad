//! End-to-end test: starts the real service on a localhost socket and drives
//! it through the `LocalService` provider (which exercises the same
//! http_client the sandbox will use): health, models, generate with the
//! testpattern backend, job polling, artifact fetch, queue/reject policy.

use makepad_asset_ai::client::{ContentProvider, LocalService};
use makepad_asset_ai::download::Downloader;
use makepad_asset_ai::error::AssetAiError;
use makepad_asset_ai::http_client::{http_fetch, HttpClientRequest};
use makepad_asset_ai::protocol::{GenerateRequestJson, RealtimeRequestJson, RealtimeResponseJson};
use makepad_asset_ai::realtime_wire::{self, FrameHeader, FrameKind};
use makepad_asset_ai::registry::{Domain, Registry};
use makepad_asset_ai::server::{start_service, ServiceConfig};
use makepad_live_id::LiveId;
use makepad_micro_serde::{DeJson, SerJson};
use makepad_network::plain_web_socket::PlainWebSocket;
use makepad_network::{HttpMethod, HttpRequest, WebSocketMessage};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "makepad-asset-ai-e2e-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn peer_off() -> makepad_asset_ai::peer_serve::PeerOptions {
    // Tests must not race on process-global env: pin the peer lane off with
    // explicit options unless a test opts in.
    makepad_asset_ai::peer_serve::PeerOptions {
        serve: Some(false),
        sources: Some(Vec::new()),
        ..Default::default()
    }
}

fn start_test_service(name: &str) -> LocalService {
    let handle = start_service(ServiceConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        cache_dir: test_dir(name),
        registry: Registry::embedded().unwrap(),
        downloader: Downloader::new("http://127.0.0.1:1", None).unwrap(),
        peer: peer_off(),
        fleet: makepad_asset_ai::discovery::DEFAULT_FLEET.to_string(),
    })
    .unwrap();
    let provider = LocalService::new(&format!("http://{}", handle.addr));
    // The service outlives the test; keep the singleton lock with it.
    std::mem::forget(handle);
    provider
}

fn poll_until_done(provider: &LocalService, job_id: &str) -> Vec<(String, String)> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = provider.poll(job_id).unwrap();
        match status.state.as_str() {
            "done" => {
                return status
                    .artifacts
                    .iter()
                    .map(|a| (a.id.clone(), a.content_type.clone()))
                    .collect()
            }
            "error" => panic!("job failed: {:?}", status.error),
            _ => {
                assert!(Instant::now() < deadline, "job did not finish in time");
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

fn poll_until_terminal(provider: &LocalService, job_id: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = provider.poll(job_id).unwrap();
        if matches!(status.state.as_str(), "done" | "error" | "cancelled") {
            return status.state;
        }
        assert!(Instant::now() < deadline, "job did not finish in time");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn health_and_models() {
    let provider = start_test_service("health");

    let health = provider.health().unwrap();
    assert_eq!(health.service, "makepad-asset-ai");
    assert_eq!(health.version, env!("CARGO_PKG_VERSION"));
    assert!(health.models_loaded.is_empty());

    let models = provider.list_models().unwrap();
    let testpattern = models.iter().find(|m| m.id == "testpattern").unwrap();
    assert!(testpattern.available);
    assert_eq!(testpattern.state, "ready");
    assert_eq!(testpattern.domain, "image");

    // flux1-schnell: registered, no cached files -> absent. Availability
    // requires BOTH the compiled `flux` feature AND real FP8 execution
    // capability (a CUDA device): the canonical combined-FP8 tier has no
    // CPU/Metal fallback, so a mac/CI test run with `--features flux` must
    // still list flux unavailable, fail-closed.
    let expect_flux =
        cfg!(feature = "flux") && makepad_asset_ai::backend::backend_provisioned("flux");
    let schnell = models.iter().find(|m| m.id == "flux1-schnell").unwrap();
    assert_eq!(schnell.state, "absent");
    assert_eq!(schnell.available, expect_flux);

    let dev = models.iter().find(|m| m.id == "flux1-dev").unwrap();
    assert_eq!(dev.available, expect_flux);
    assert!(dev.gated);

    let trellis = models.iter().find(|m| m.id == "trellis-2").unwrap();
    assert_eq!(trellis.domain, "mesh");
    // Availability tracks the compiled backend: the default (stub) build has
    // no mesh feature, a `--features mesh` test run does.
    assert_eq!(trellis.available, cfg!(feature = "mesh"));
}

#[test]
fn generate_testpattern_end_to_end() {
    let provider = start_test_service("generate");

    let job_id = provider
        .request(
            Domain::Image,
            &GenerateRequestJson {
                model: "testpattern".to_string(),
                prompt: Some("a red fox jumping a fence".to_string()),
                width: Some(64),
                height: Some(48),
                seed: Some(42),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(job_id.starts_with("job-"));

    let artifacts = poll_until_done(&provider, &job_id);
    assert_eq!(artifacts.len(), 1);
    let (artifact_id, content_type) = &artifacts[0];
    assert_eq!(content_type, "image/png");

    let artifact = provider.fetch_artifact(artifact_id).unwrap();
    assert_eq!(artifact.content_type, "image/png");
    assert_eq!(
        &artifact.bytes[..8],
        &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']
    );
    let width = u32::from_be_bytes(artifact.bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(artifact.bytes[20..24].try_into().unwrap());
    assert_eq!((width, height), (64, 48));

    // Determinism: the same request renders identical bytes.
    let job2 = provider
        .request(
            Domain::Image,
            &GenerateRequestJson {
                model: "testpattern".to_string(),
                prompt: Some("a red fox jumping a fence".to_string()),
                width: Some(64),
                height: Some(48),
                seed: Some(42),
                ..Default::default()
            },
        )
        .unwrap();
    let artifacts2 = poll_until_done(&provider, &job2);
    let artifact2 = provider.fetch_artifact(&artifacts2[0].0).unwrap();
    assert_eq!(artifact.bytes, artifact2.bytes);

    // Testpattern has no resident runtime: generation must not falsely turn
    // an on-disk/callable model into `loaded` fleet affinity.
    let health = provider.health().unwrap();
    assert!(!health.models_loaded.contains(&"testpattern".to_string()));
    let models = provider.list_models().unwrap();
    assert_eq!(
        models.iter().find(|model| model.id == "testpattern").unwrap().state,
        "ready"
    );
}

#[test]
fn model_picked_by_domain_when_unnamed() {
    let provider = start_test_service("bydomain");
    // Empty model + image domain -> the provider picks testpattern (the only
    // available image model in a no-flux build).
    let job_id = provider
        .request(
            Domain::Image,
            &GenerateRequestJson {
                prompt: Some("anything".to_string()),
                width: Some(16),
                height: Some(16),
                ..Default::default()
            },
        )
        .unwrap();
    let artifacts = poll_until_done(&provider, &job_id);
    assert_eq!(artifacts.len(), 1);

    // Mesh domain has no available model in the default (stub) build ->
    // clean error. (A `--features mesh` build compiles the real trellis
    // backend, so the domain IS routable there and this check doesn't apply.)
    #[cfg(not(feature = "mesh"))]
    {
        let err = provider
            .request(Domain::Mesh, &GenerateRequestJson::default())
            .unwrap_err();
        assert!(matches!(err, AssetAiError::Unavailable(_)));
    }
}

#[test]
fn queue_and_reject_policies() {
    let provider = start_test_service("policies");

    // A slow job (the delay_ms test hook) occupies the single GPU slot.
    let slow = provider
        .request(
            Domain::Image,
            &GenerateRequestJson {
                model: "testpattern".to_string(),
                prompt: Some("slow".to_string()),
                width: Some(16),
                height: Some(16),
                delay_ms: Some(1500),
                ..Default::default()
            },
        )
        .unwrap();

    // Wait until it is actually running.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = provider.poll(&slow).unwrap();
        if status.state == "running" {
            break;
        }
        assert!(Instant::now() < deadline, "slow job never started");
        std::thread::sleep(Duration::from_millis(10));
    }

    // Reject policy while busy -> Busy error (http 409).
    let err = provider
        .request(
            Domain::Image,
            &GenerateRequestJson {
                model: "testpattern".to_string(),
                prompt: Some("rejected".to_string()),
                width: Some(16),
                height: Some(16),
                queue_policy: Some("reject".to_string()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err, AssetAiError::Busy);

    // Queue policy while busy -> accepted, runs after the slow one.
    let queued = provider
        .request(
            Domain::Image,
            &GenerateRequestJson {
                model: "testpattern".to_string(),
                prompt: Some("queued".to_string()),
                width: Some(16),
                height: Some(16),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(provider.poll(&queued).unwrap().state, "queued");

    poll_until_done(&provider, &slow);
    poll_until_done(&provider, &queued);
}

#[test]
fn error_paths() {
    let provider = start_test_service("errors");

    // Unknown model -> http 404 with a message.
    let err = provider
        .request(
            Domain::Image,
            &GenerateRequestJson {
                model: "does-not-exist".to_string(),
                ..Default::default()
            },
        )
        .unwrap_err();
    match err {
        AssetAiError::Http(message) => assert!(message.contains("unknown model")),
        other => panic!("expected Http error, got {other:?}"),
    }

    // Unavailable model -> http 503. Pick one dynamically so the assertion
    // holds under every feature combination: any model /models reports as
    // unavailable must refuse generation with an explicit 503.
    let models = provider.list_models().unwrap();
    if let Some(unavailable) = models.iter().find(|m| !m.available) {
        let err = provider
            .request(
                Domain::Image,
                &GenerateRequestJson {
                    model: unavailable.id.clone(),
                    ..Default::default()
                },
            )
            .unwrap_err();
        match err {
            AssetAiError::Http(message) => assert!(message.contains("503"), "got: {message}"),
            other => panic!("expected Http error, got {other:?}"),
        }
        // The same model must say WHY on /models.
        assert!(unavailable.unavailable_reason.is_some());
    }

    // Unknown job and artifact ids -> clean 404s.
    assert!(provider.poll("job-999").is_err());
    assert!(provider.fetch_artifact("nope-0").is_err());
}

#[test]
fn pull_only_job_downloads_and_stops() {
    let provider = start_test_service("pull");

    // A pull job prepares artifacts (a no-op for testpattern) and completes
    // before resident loading/generation: done, zero artifacts, still Ready.
    let job_id = provider
        .request(
            Domain::Image,
            &GenerateRequestJson {
                model: "testpattern".to_string(),
                pull_only: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    let artifacts = poll_until_done(&provider, &job_id);
    assert!(artifacts.is_empty());
    let models = provider.list_models().unwrap();
    assert_eq!(
        models.iter().find(|model| model.id == "testpattern").unwrap().state,
        "ready"
    );
}

#[test]
fn cancelled_generation_keeps_truthful_state_and_next_job_succeeds() {
    let provider = start_test_service("cancel-recover");
    let slow = provider
        .request(
            Domain::Image,
            &GenerateRequestJson {
                model: "testpattern".to_string(),
                prompt: Some("slow".into()),
                width: Some(16),
                height: Some(16),
                delay_ms: Some(2_000),
                ..Default::default()
            },
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if provider.poll(&slow).unwrap().state == "running" {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    provider.cancel(&slow).unwrap();
    assert_eq!(poll_until_terminal(&provider, &slow), "cancelled");
    let models = provider.list_models().unwrap();
    assert_eq!(
        models.iter().find(|model| model.id == "testpattern").unwrap().state,
        "ready"
    );

    let next = provider
        .request(
            Domain::Image,
            &GenerateRequestJson {
                model: "testpattern".to_string(),
                prompt: Some("recovered".into()),
                width: Some(16),
                height: Some(16),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(poll_until_done(&provider, &next).len(), 1);
}

#[test]
fn artifact_handoff_is_hash_verified() {
    let provider = start_test_service("hash");
    let job = provider
        .request(
            Domain::Image,
            &GenerateRequestJson {
                model: "testpattern".to_string(),
                prompt: Some("hash me".into()),
                width: Some(32),
                height: Some(32),
                seed: Some(7),
                ..Default::default()
            },
        )
        .unwrap();
    // Wait for done, then inspect the raw status: every artifact ref must
    // carry its digest and exact length.
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        let status = provider.poll(&job).unwrap();
        match status.state.as_str() {
            "done" => break status,
            "error" => panic!("job failed: {:?}", status.error),
            _ => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    };
    assert!(!status.artifacts.is_empty());
    for artifact in &status.artifacts {
        let sha = artifact.sha256.as_deref().expect("artifact sha256 present");
        assert_eq!(sha.len(), 64);
        assert!(sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        let len = artifact.byte_len.expect("artifact byte_len present");
        assert!(len > 0);

        // fetch_artifact verifies the X-Artifact-Sha256 header internally;
        // then the JSON-level verifier must agree on the same bytes.
        let fetched = provider.fetch_artifact(&artifact.id).unwrap();
        assert_eq!(fetched.bytes.len() as u64, len);
        makepad_asset_ai::client::verify_artifact_bytes(&fetched.bytes, artifact).unwrap();

        // A corrupted relay is refused explicitly.
        let mut corrupted = fetched.bytes.clone();
        corrupted[0] ^= 0xff;
        let refused = makepad_asset_ai::client::verify_artifact_bytes(&corrupted, artifact);
        assert!(refused.is_err(), "corrupted bytes must not verify");
    }

    // Job metadata: model + lifecycle timestamps + bounded stage log.
    assert_eq!(status.model.as_deref(), Some("testpattern"));
    let queued = status.queued_ms.expect("queued_ms");
    let started = status.started_ms.expect("started_ms");
    let finished = status.finished_ms.expect("finished_ms");
    assert!(queued <= started && started <= finished);
    let log = status.log.expect("stage log present");
    assert!(!log.is_empty());
    assert!(log.iter().any(|line| line.contains("render") || line.contains("done")));
}

#[test]
fn node_identity_is_durable_and_capabilities_are_honest() {
    // Two service starts over the SAME cache dir: the durable node_key must
    // survive the restart while the per-start node_id rotates.
    let dir = test_dir("identity");
    let start = |dir: &PathBuf| {
        let handle = start_service(ServiceConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            cache_dir: dir.clone(),
            registry: Registry::embedded().unwrap(),
            downloader: Downloader::new("http://127.0.0.1:1", None).unwrap(),
            peer: peer_off(),
            fleet: makepad_asset_ai::discovery::DEFAULT_FLEET.to_string(),
        })
        .unwrap();
        // Dropping the handle releases the per-cache-dir singleton lock —
        // exactly the "old process gone" restart this test simulates.
        LocalService::new(&format!("http://{}", handle.addr))
    };
    let first = start(&dir).health().unwrap();
    let second = start(&dir).health().unwrap();
    let key_a = first.node_key.expect("node_key present");
    let key_b = second.node_key.expect("node_key present");
    assert_eq!(key_a, key_b, "node_key is durable across restarts");
    assert_eq!(key_a.len(), 32);
    assert_ne!(first.node_id, second.node_id, "node_id rotates per start");
    assert!(first.started_ms.unwrap() > 0);
    assert!(first.queue_limit.unwrap() >= 1);
    assert!(first.vram_reserve_mb.is_some());

    // Capabilities list only what THIS build+machine can serve: the
    // default-feature test build serves image (testpattern) but must not
    // claim e.g. video (backend not compiled).
    let caps = first.capabilities.expect("capabilities present");
    assert!(caps.contains(&"image".to_string()));
    // Domains whose backend is not compiled must not be claimed. (With the
    // video feature on, h3 is compiled and the claim is legitimate.)
    if !cfg!(feature = "video") {
        assert!(!caps.contains(&"video".to_string()));
    }
}

#[test]
fn models_report_revision_and_explicit_unavailable_reason() {
    let provider = start_test_service("reasons");
    let models = provider.list_models().unwrap();

    let testpattern = models.iter().find(|m| m.id == "testpattern").unwrap();
    assert!(testpattern.available);
    assert!(testpattern.unavailable_reason.is_none());

    // The model must say WHY it is unavailable instead of a bare false. The
    // canonical FP8 tier needs the compiled feature AND a CUDA device; the
    // reason names whichever gate failed (feature -> "not compiled",
    // capability -> "not provisioned"). Only a CUDA box with the feature
    // shows it available with no reason.
    let flux = models.iter().find(|m| m.id == "flux1-schnell").unwrap();
    if cfg!(feature = "flux") && makepad_asset_ai::backend::backend_provisioned("flux") {
        assert!(flux.available);
        assert!(flux.unavailable_reason.is_none());
    } else {
        assert!(!flux.available);
        let reason = flux.unavailable_reason.as_deref().expect("explicit reason");
        assert!(
            reason.contains("not compiled") || reason.contains("not provisioned"),
            "got: {reason}"
        );
    }

    // Pinned registry revisions surface for provenance-tracking consumers.
    let trellis = models.iter().find(|m| m.id == "trellis-2").unwrap();
    let revision = trellis.revision.as_deref().expect("trellis revisions pinned");
    assert!(revision.len() >= 40);
}

// ---------------------------------------------------------------------------
// Realtime / live session (POST /realtime + GET /realtime/<id> websocket)
// ---------------------------------------------------------------------------

fn post_realtime(base_url: &str, request: &RealtimeRequestJson) -> (u16, RealtimeResponseJson) {
    let url = format!("{base_url}/realtime");
    let response = http_fetch(&HttpClientRequest::post(
        &url,
        "application/json",
        request.serialize_json().as_bytes(),
    ))
    .unwrap();
    let status = response.status;
    let bytes = response.read_body_to_vec(4 * 1024 * 1024).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let parsed = RealtimeResponseJson::deserialize_json_lenient(text)
        .unwrap_or_else(|e| panic!("bad realtime response json {text:?}: {e:?}"));
    (status, parsed)
}

/// Opens a plain-TCP websocket to `<base_url><ws_path>` (base_url is
/// "http://host:port" — rewritten to "ws://host:port" here, matching the
/// scheme `PlainWebSocket::open` accepts).
fn open_realtime_socket(base_url: &str, ws_path: &str) -> (PlainWebSocket, Receiver<WebSocketMessage>) {
    let ws_url = format!("{}{}", base_url.replacen("http://", "ws://", 1), ws_path);
    let (tx, rx) = std::sync::mpsc::channel::<WebSocketMessage>();
    let socket = PlainWebSocket::open(LiveId::empty(), HttpRequest::new(ws_url, HttpMethod::GET), tx);
    (socket, rx)
}

/// Drains `rx` until the socket reports `Closed` (or its channel disconnects
/// — the io thread dropping its sender after `Closed` is also a valid
/// "closed" signal), or `timeout` elapses.
fn wait_for_socket_close(rx: &Receiver<WebSocketMessage>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(WebSocketMessage::Closed) => return true,
            Ok(_) => {} // drain frames/stats/stopped/etc while waiting
            Err(RecvTimeoutError::Disconnected) => return true,
            Err(RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    return false;
                }
            }
        }
    }
}

fn poll_realtime_terminal(provider: &LocalService, job_id: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let status = provider.poll(job_id).unwrap();
        if matches!(status.state.as_str(), "done" | "cancelled" | "error") {
            return status.state;
        }
        assert!(
            Instant::now() < deadline,
            "live session did not reach a terminal state in time (state={})",
            status.state
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn realtime_feed_mode_streams_frames_and_stops_cleanly() {
    let provider = start_test_service("realtime-feed");
    let base_url = provider.base_url().to_string();

    let (status, response) = post_realtime(
        &base_url,
        &RealtimeRequestJson {
            model: "testpattern".to_string(),
            width: Some(16),
            height: Some(16),
            prompt: Some("a red fox".to_string()),
            strength: Some(0.7),
            steps: Some(2),
            loop_mode: Some("feed".to_string()),
            output_encoding: Some("raw".to_string()),
            idle_timeout_s: Some(5),
            ..Default::default()
        },
    );
    assert_eq!(status, 200, "realtime response: {response:?}");
    let job_id = response.job_id.clone().expect("job_id");
    let ws_path = response.ws_path.clone().expect("ws_path");
    assert_eq!(ws_path, format!("/realtime/{job_id}"));
    assert!(provider.poll(&job_id).unwrap().state == "queued" || provider.poll(&job_id).unwrap().state == "live");

    let (mut socket, rx) = open_realtime_socket(&base_url, &ws_path);

    // A control update (must not crash/derail the session) ...
    socket
        .send_message(WebSocketMessage::String(
            r#"{"type":"control","prompt":"a blue whale","strength":0.9}"#.to_string(),
        ))
        .unwrap();

    // Then raw RGB8 input frames — sent one at a time, interleaved with
    // receiving, not as an upfront burst: the session mailbox keeps only the
    // LATEST unconsumed frame (see protocol.rs's backpressure law), so a
    // burst sent faster than the worker's loop cadence would mostly
    // coalesce away and starve loop_mode="feed" (which blocks waiting for a
    // genuinely NEW frame each iteration). A real camera-feed client would
    // push continuously the same way. Keep sending fresh frames until at
    // least 5 outputs and one stats message have arrived.
    let mut frames_received = 0usize;
    let mut stats_received = false;
    let mut next_input_index = 0u32;
    let deadline = Instant::now() + Duration::from_secs(15);
    while (frames_received < 5 || !stats_received) && Instant::now() < deadline {
        if next_input_index < 200 {
            let payload = vec![((next_input_index * 37) % 255) as u8; 16 * 16 * 3];
            let header = FrameHeader {
                kind: FrameKind::Raw,
                width: 16,
                height: 16,
                frame_index: next_input_index,
            };
            let bytes = realtime_wire::encode_frame(header, &payload);
            socket.send_message(WebSocketMessage::Binary(bytes)).unwrap();
            next_input_index += 1;
        }
        match rx.recv_timeout(Duration::from_millis(30)) {
            Ok(WebSocketMessage::Binary(data)) => {
                if realtime_wire::is_frame_message(&data) {
                    let (header, payload) = realtime_wire::decode_frame(&data).unwrap();
                    assert_eq!(header.kind, FrameKind::Raw);
                    assert_eq!((header.width, header.height), (16, 16));
                    assert_eq!(payload.len(), 16 * 16 * 3);
                    frames_received += 1;
                } else {
                    let text = std::str::from_utf8(&data).expect("non-frame push must be utf-8 json");
                    if text.contains("\"type\":\"stats\"") {
                        stats_received = true;
                        assert!(text.contains("\"frames_out\""));
                    }
                }
            }
            Ok(WebSocketMessage::Closed) => panic!("socket closed before {frames_received} frames arrived"),
            Ok(WebSocketMessage::Error(e)) => panic!("websocket error: {e}"),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => panic!("socket channel disconnected early"),
        }
    }
    assert!(frames_received >= 5, "only received {frames_received} frames");
    assert!(stats_received, "never received a stats message");

    // GET /job/<id> carries live counters whenever it reports state=live
    // (the counters themselves are only refreshed at ~10 Hz — see
    // `run_live`'s doc — so they may still read frames_out=0 a few ms into
    // the session; only the field's presence is a stable contract here).
    let live_status = provider.poll(&job_id).unwrap();
    if live_status.state == "live" {
        live_status.live.expect("live counters present while state=live");
    }

    socket
        .send_message(WebSocketMessage::String(r#"{"type":"stop"}"#.to_string()))
        .unwrap();

    let state = poll_realtime_terminal(&provider, &job_id);
    assert_eq!(state, "done", "stop must end the session as done, not error/cancelled");

    assert!(wait_for_socket_close(&rx, Duration::from_secs(10)), "socket never closed after stop");
    socket.close();
}

#[test]
fn realtime_feedback_mode_produces_frames_without_any_input_then_cancels() {
    let provider = start_test_service("realtime-feedback");
    let base_url = provider.base_url().to_string();

    let (status, response) = post_realtime(
        &base_url,
        &RealtimeRequestJson {
            model: "testpattern".to_string(),
            // 16 is LiveParams::from_request's clamp floor for width/height.
            width: Some(16),
            height: Some(16),
            loop_mode: Some("feedback".to_string()),
            idle_timeout_s: Some(5),
            ..Default::default()
        },
    );
    assert_eq!(status, 200, "realtime response: {response:?}");
    let job_id = response.job_id.clone().expect("job_id");
    let ws_path = response.ws_path.clone().expect("ws_path");

    let (mut socket, rx) = open_realtime_socket(&base_url, &ws_path);

    // No input frames are ever sent — feedback mode must still produce
    // output (its own previous output, camera-warped, seeds the next step;
    // the very first frame has no previous output either, and testpattern's
    // live_step handles `init: None` by rendering the pure pattern).
    let mut frames_received = 0usize;
    let deadline = Instant::now() + Duration::from_secs(15);
    while frames_received < 3 && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(WebSocketMessage::Binary(data)) if realtime_wire::is_frame_message(&data) => {
                let (header, payload) = realtime_wire::decode_frame(&data).unwrap();
                assert_eq!((header.width, header.height), (16, 16));
                assert_eq!(payload.len(), 16 * 16 * 3);
                frames_received += 1;
            }
            Ok(WebSocketMessage::Error(e)) => panic!("websocket error: {e}"),
            _ => {}
        }
    }
    assert!(frames_received >= 3, "only received {frames_received} feedback-mode frames");

    // Cancel via the ordinary job-cancel endpoint (not the ws "stop"
    // message) — the live session must honor POST /job/<id>/cancel exactly
    // like a running generate job.
    provider.cancel(&job_id).unwrap();
    let state = poll_realtime_terminal(&provider, &job_id);
    assert_eq!(state, "cancelled");

    assert!(wait_for_socket_close(&rx, Duration::from_secs(10)), "socket never closed after cancel");
    socket.close();
}

#[test]
fn realtime_post_admission_errors_match_generate_semantics() {
    let provider = start_test_service("realtime-admission-errors");
    let base_url = provider.base_url().to_string();

    let (status, response) = post_realtime(
        &base_url,
        &RealtimeRequestJson {
            model: "does-not-exist".to_string(),
            ..Default::default()
        },
    );
    assert_eq!(status, 404, "unknown model: {response:?}");

    // A registered-but-unavailable model 503s before the live-support check
    // even runs — same admission order as POST /generate.
    let expect_flux = cfg!(feature = "flux") && makepad_asset_ai::backend::backend_provisioned("flux");
    let (status, response) = post_realtime(
        &base_url,
        &RealtimeRequestJson {
            model: "flux1-schnell".to_string(),
            ..Default::default()
        },
    );
    if !expect_flux {
        assert_eq!(status, 503, "unavailable model: {response:?}");
    }
}
