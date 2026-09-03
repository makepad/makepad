#![cfg(not(target_arch = "wasm32"))]

use makepad_ai_hub::download::Downloader;
use makepad_ai_hub::registry::{Domain, ModelSpec, Registry};
use makepad_ai_hub::server::{start_service, ServiceConfig, ServiceHandle};
use makepad_flow::client::{Endpoints, FlowClient};
use makepad_flow::host::{FlowServer, FlowServerConfig, ServerError};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "makepad-flow-models-{}-{label}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn peer_off() -> makepad_ai_hub::peer_serve::PeerOptions {
    makepad_ai_hub::peer_serve::PeerOptions {
        serve: Some(false),
        sources: Some(Vec::new()),
        ..Default::default()
    }
}

fn start_hub(root: &Path) -> Option<ServiceHandle> {
    let service = start_service(ServiceConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        cache_dir: root.join("hub"),
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
        peer: peer_off(),
        fleet: makepad_ai_hub::discovery::DEFAULT_FLEET.to_string(),
    });
    match service {
        Ok(handle) => Some(handle),
        Err(error) if error.to_string().contains("Operation not permitted") => {
            eprintln!("skipping host model test: loopback bind is forbidden by this sandbox");
            None
        }
        Err(error) => panic!("start test hub: {error}"),
    }
}

fn start_flow(root: &Path, fleet_hint: Vec<String>) -> Option<FlowServer> {
    let mut config = FlowServerConfig::new(root.join("flow"));
    config.watch_interval_ms = 25;
    config.fleet_hint = fleet_hint;
    config.log = Box::new(|_| {});
    match FlowServer::start(config) {
        Ok(server) => Some(server),
        Err(ServerError::Io {
            kind: std::io::ErrorKind::PermissionDenied,
            ..
        }) => {
            eprintln!("skipping host model test: loopback bind is forbidden by this sandbox");
            None
        }
        Err(error) => panic!("start flow server: {error}"),
    }
}

fn client(server: &FlowServer) -> FlowClient {
    let endpoints = server.endpoints();
    FlowClient::connect(
        Endpoints {
            control: endpoints.control,
            data: endpoints.data,
        },
        endpoints.token.clone(),
        Some(endpoints.server_id),
    )
    .unwrap()
}

#[test]
fn models_lists_testpattern_and_filters_by_domain() {
    let root = TempRoot::new("live");
    let Some(hub) = start_hub(&root.0) else {
        return;
    };
    let base_url = format!("http://{}", hub.addr);
    let Some(server) = start_flow(&root.0, vec![base_url.clone()]) else {
        return;
    };
    let client = client(&server);

    let response = client.models(None).unwrap();
    assert!(response.snapshot_ms > 0);
    let node = response
        .nodes
        .iter()
        .find(|node| node.base_url == base_url)
        .expect("hinted hub node missing");
    assert!(node.healthy);
    let testpattern = response
        .models
        .iter()
        .find(|model| model.id == "testpattern" && model.node == base_url)
        .expect("testpattern model missing");
    assert_eq!(testpattern.domain, "image");
    assert!(testpattern.available);

    let video = client.models(Some("video")).unwrap();
    assert!(video.models.is_empty());
    assert_eq!(video.snapshot_ms, response.snapshot_ms);

    let catalog = client.nodes_catalog().unwrap();
    let image = catalog
        .types
        .iter()
        .find(|ty| ty.domain.as_deref() == Some("image"))
        .expect("image catalog type missing");
    assert!(image.models.iter().any(|model| model == "testpattern"));

    server.shutdown();
    std::mem::forget(hub);
}

#[test]
fn dead_hint_is_reported_unhealthy_without_failing_the_route() {
    let root = TempRoot::new("dead");
    let dead_url = "http://127.0.0.1:1".to_string();

    let Some(server) = start_flow(&root.0, vec![dead_url.clone()]) else {
        return;
    };
    let response = client(&server).models(None).unwrap();
    let node = response
        .nodes
        .iter()
        .find(|node| node.base_url == dead_url)
        .expect("dead hinted node missing");
    assert!(!node.healthy);
    assert!(response
        .models
        .iter()
        .all(|model| model.node != dead_url));
    server.shutdown();
}
