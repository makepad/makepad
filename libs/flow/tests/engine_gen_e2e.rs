mod support;

use makepad_ai_hub::discovery::DEFAULT_FLEET;
use makepad_ai_hub::download::Downloader;
use makepad_ai_hub::peer_serve::PeerOptions;
use makepad_ai_hub::registry::{Domain, ModelSpec, Registry};
use makepad_ai_hub::server::{start_service, ServiceConfig};
use makepad_flow::engine::FixedGen;
use std::sync::Arc;
use std::time::SystemTime;
use support::{output, FakeChat, FakeHttp};

#[test]
fn fixed_gen_runs_real_testpattern_service_and_is_deterministic() {
    let cache_dir = std::env::temp_dir().join(format!(
        "makepad-flow-gen-e2e-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&cache_dir).unwrap();
    let service = start_service(ServiceConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        cache_dir: cache_dir.clone(),
        registry: Registry {
            models: vec![ModelSpec {
                id: "testpattern".to_string(),
                domain: Domain::Image,
                backend: "testpattern".to_string(),
                available: true,
                gated: false,
                vram_gb: Some(0.0),
                min_vram_gb: None,
                min_compute_cap: None,
                note: None,
                license: None,
                files: Vec::new(),
            }],
        },
        downloader: Downloader::new("http://127.0.0.1:1", None).unwrap(),
        peer: PeerOptions {
            serve: Some(false),
            sources: Some(Vec::new()),
            ..Default::default()
        },
        fleet: DEFAULT_FLEET.to_string(),
    });
    let handle = match service {
        Ok(handle) => handle,
        Err(error) if error.to_string().contains("Operation not permitted") => {
            let _ = std::fs::remove_dir_all(&cache_dir);
            eprintln!("skipping real hub service: loopback bind is forbidden by this sandbox");
            return;
        }
        Err(error) => panic!("start real hub service: {error}"),
    };
    let base_url = format!("http://{}", handle.addr);
    // The service has no stop message; the integration-test process owns all
    // of its threads and drops them together at process exit.
    std::mem::forget(handle);

    let source = r#"use mod.flow.*
let prompt = Input{default: "same prompt"}
let image = Image{prompt: prompt.text() width: 64 height: 48 steps: 4 seed: 42 model: "testpattern"}
let picture = Output{type: @image value: image.image()}
Flow{prompt, image, picture}
"#;
    let run_once = || {
        support::run(
            source,
            makepad_flow::engine::Seams {
                chat: Arc::new(FakeChat::done("unused")),
                gen: Arc::new(FixedGen(base_url.clone())),
                http: Arc::new(FakeHttp::json(200, "{}")),
            },
            None,
        )
    };
    let first = run_once();
    let second = run_once();
    let first = output(&first, "picture");
    let second = output(&second, "picture");
    assert_eq!(first.content_type, "image/png");
    assert_eq!(&first.bytes[..8], &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']);
    assert_eq!(first.digest, second.digest);
}
