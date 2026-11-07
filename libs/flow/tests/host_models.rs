#![cfg(not(target_arch = "wasm32"))]

use makepad_flow::client::{Endpoints, FlowClient};
use makepad_flow::host::{FlowServer, FlowServerConfig};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
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

/// What a hub serving `testpattern` (image) and `qwen-test` (an LLM, advertised for chat and
/// for text) tells the fleet probe.
const HEALTH: &str = r#"{"service":"makepad-ai-hub","version":"test","models_loaded":[]}"#;
const MODELS: &str = r#"{"models":[
    {"id":"testpattern","domain":"image","backend":"testpattern","available":true,"gated":false,"vram_gb":0.0,"state":"ready"},
    {"id":"qwen-test","domain":"chat","backend":"llm","available":true,"gated":false,"vram_gb":0.0,"state":"ready"},
    {"id":"qwen-test","domain":"text","backend":"llm","available":true,"gated":false,"vram_gb":0.0,"state":"ready"}
]}"#;

/// A hub node that answers the probe at once with [`HEALTH`] and [`MODELS`]: the probe's budget
/// never comes into it, however loaded the box (a whole hub's answer is work a loaded box can
/// take past the budget, and the node then reads as unhealthy). Its base url.
fn start_node() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || answer(stream));
        }
    });
    base_url
}

fn answer(mut stream: TcpStream) {
    let (mut request, mut buf) = (Vec::new(), [0u8; 1024]);
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => request.extend_from_slice(&buf[..n]),
        }
    }
    let head = String::from_utf8_lossy(&request);
    let (status, body) = match head.split_whitespace().nth(1) {
        Some("/health") => ("200 OK", HEALTH),
        Some("/models") => ("200 OK", MODELS),
        _ => ("404 Not Found", "{}"),
    };
    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
}

fn start_flow(root: &Path, fleet_hint: Vec<String>) -> FlowServer {
    let mut config = FlowServerConfig::new(root.join("flow"));
    config.watch_interval_ms = 25;
    config.fleet_hint = fleet_hint;
    config.log = Box::new(|_| {});
    match FlowServer::start(config) {
        Ok(server) => server,
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
    let base_url = start_node();
    let server = start_flow(&root.0, vec![base_url.clone()]);
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

    let text = client.models(Some("text")).unwrap();
    let qwen: Vec<_> = text.models.iter().filter(|model| model.id == "qwen-test").collect();
    assert_eq!(qwen.len(), 1, "text/chat aliases must collapse to one picker entry");
    assert_eq!(qwen[0].domain, "text");

    let catalog = client.nodes_catalog().unwrap();
    let image = catalog
        .types
        .iter()
        .find(|ty| ty.domain.as_deref() == Some("image"))
        .expect("image catalog type missing");
    assert!(image.models.iter().any(|model| model == "testpattern"));

    server.shutdown();
}

#[test]
fn dead_hint_is_reported_unhealthy_without_failing_the_route() {
    let root = TempRoot::new("dead");
    let dead_url = "http://127.0.0.1:1".to_string();

    let server = start_flow(&root.0, vec![dead_url.clone()]);
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
