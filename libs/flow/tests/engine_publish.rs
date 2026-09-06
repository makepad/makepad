#![cfg(not(target_arch = "wasm32"))]

use makepad_asset_client::ApiEndpoints;
use makepad_flow::engine::executors::publish::{
    AssetListQuery, AssetStoreConfig, AssetWorker, PublishExecutor,
};
use makepad_flow::engine::executors::{Executor, Poll};
use makepad_flow::{Literal, Loc, Node, Port, PortType, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Roots(Vec<PathBuf>);

impl Roots {
    fn new() -> Self { Self(Vec::new()) }

    fn make(&mut self, label: &str) -> PathBuf {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("flow-publish-{}-{label}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        self.0.push(path.clone());
        path
    }
}

impl Drop for Roots {
    fn drop(&mut self) {
        for path in &self.0 { let _ = std::fs::remove_dir_all(path); }
    }
}

fn start_store(root: PathBuf) -> (makepad_asset_store::AssetServer, String) {
    let mut config = makepad_asset_store::ServerConfig::new(root.clone());
    config.control_addr = "127.0.0.1:0".parse().unwrap();
    config.data_addr = "127.0.0.1:0".parse().unwrap();
    config.bootstrap_admin = true;
    config.discovery = None;
    config.log = false;
    let server = makepad_asset_store::AssetServer::start(config).unwrap();
    let token = std::fs::read_to_string(root.join("admin-token")).unwrap().trim().to_string();
    (server, token)
}

fn node() -> Node {
    Node {
        id: "library".into(),
        kind: "publish".into(),
        type_name: "Publish".into(),
        params: vec![
            ("title".into(), Literal::Str("Sunset result".into())),
            ("namespace".into(), Literal::Str("flows".into())),
            ("tags".into(), Literal::Arr(vec![Literal::Str("flow".into()), Literal::Str("sunset".into())])),
            ("description".into(), Literal::Str("painted from a prompt".into())),
            ("alias".into(), Literal::Str("flows/sunset-result".into())),
        ],
        inputs: Vec::new(),
        outputs: vec![Port { name: "asset".into(), ty: PortType::Json }],
        at: None,
        size: None,
        flip: false,
        loc: Loc::default(),
        fn_src: None,
        face_src: None,
        on_fail: "fail".into(),
        label: None,
        domain: None,
        doc: None,
    }
}

fn picture(fill: u8) -> Value {
    let mut pixels = vec![fill; 256 * 256 * 4];
    for alpha in pixels.iter_mut().skip(3).step_by(4) { *alpha = 255; }
    let png = makepad_ai_hub::testpattern::encode_png_rgba(&pixels, 256, 256).unwrap();
    Value::media(PortType::Image, "image/png", png)
}

fn publish(worker: &AssetWorker, value: Value) -> makepad_strict_json::Value {
    let mut executor = PublishExecutor::new(
        Some(worker.handle()),
        "prompt-to-library".into(),
        "instance-1".into(),
        String::new(),
    );
    executor.start(&node(), &[("value".into(), value)]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match executor.poll() {
            Poll::Done(outputs) => {
                let value = outputs.into_iter().find(|(port, _)| port == "asset").unwrap().1;
                return makepad_strict_json::parse(&value.bytes).unwrap();
            }
            Poll::Failed(error) => panic!("publish failed: {error}"),
            _ if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            other => panic!("publish timed out after {other:?}"),
        }
    }
}

#[test]
fn picture_publish_and_alias_republish_create_revisions() {
    let mut roots = Roots::new();
    let (mut server, token) = start_store(roots.make("store"));
    let config = AssetStoreConfig {
        archive_outputs: false,
        cache_dir: roots.make("cache"),
        token: Some(token),
        endpoints: Some(ApiEndpoints { control: server.control_addr(), data: server.data_addr() }),
        server_id: Some(server.server_id()),
        discovery_port: 0,
        discovery_wait_ms: 0,
    };
    let mut worker = AssetWorker::start(config).unwrap();
    let first = publish(&worker, picture(30));
    let second = publish(&worker, picture(90));
    assert_eq!(first.get("id").and_then(|v| v.as_str()), second.get("id").and_then(|v| v.as_str()));
    assert_ne!(first.get("revision").and_then(|v| v.as_str()), second.get("revision").and_then(|v| v.as_str()));
    assert_eq!(second.get("alias").and_then(|v| v.as_str()), Some("flows/sunset-result"));
    assert_eq!(second.get("namespace").and_then(|v| v.as_str()), Some("flows"));
    assert_eq!(second.get("title").and_then(|v| v.as_str()), Some("Sunset result"));
    assert_eq!(second.get("kind").and_then(|v| v.as_str()), Some("texture"));

    let rows = worker.handle().list(AssetListQuery { text: String::new(), namespace: Some("flows".into()), limit: 10, cursor: None }).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Sunset result");
    assert_eq!(rows[0].namespace, "flows");
    assert_eq!(rows[0].tags, vec!["flow", "sunset"]);
    worker.stop();
    server.shutdown();
}

#[test]
fn missing_store_failure_names_discovery() {
    let mut roots = Roots::new();
    let mut config = AssetStoreConfig::new(roots.make("cache"));
    config.discovery_wait_ms = 0;
    let mut worker = AssetWorker::start(config).unwrap();
    let mut executor = PublishExecutor::new(Some(worker.handle()), "demo".into(), "one".into(), String::new());
    executor.start(&node(), &[("value".into(), picture(10))]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match executor.poll() {
            Poll::Failed(error) => {
                assert!(error.contains("no asset server discovered on this LAN"), "{error}");
                break;
            }
            _ if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => panic!("missing store failure timed out"),
        }
    }
    worker.stop();
}
