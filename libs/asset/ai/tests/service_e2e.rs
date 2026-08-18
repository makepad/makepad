//! End-to-end test: starts the real service on a localhost socket and drives
//! it through the `LocalService` provider (which exercises the same
//! http_client the sandbox will use): health, models, generate with the
//! testpattern backend, job polling, artifact fetch, queue/reject policy.

use makepad_asset_ai::client::{ContentProvider, LocalService};
use makepad_asset_ai::download::Downloader;
use makepad_asset_ai::error::AssetAiError;
use makepad_asset_ai::protocol::GenerateRequestJson;
use makepad_asset_ai::registry::{Domain, Registry};
use makepad_asset_ai::server::{start_service, ServiceConfig};
use std::path::PathBuf;
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
