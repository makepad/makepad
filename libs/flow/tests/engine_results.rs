#![cfg(not(target_arch = "wasm32"))]

//! Publish, archive-free result storage and the `/v1/assets` routes over the
//! plain-files `FolderStore`: what a bare open-source host gets.

use makepad_flow::client::{Endpoints, FlowClient};
use makepad_flow::engine::executors::lineage::{GeneratorRecord, Lineage};
use makepad_flow::engine::executors::publish::{PublishExecutor, PublishRun};
use makepad_flow::engine::executors::{Executor, Poll};
use makepad_flow::engine::results::{FolderStore, ResultKind, ResultQuery, ResultRef, ResultStore, PRESET_TAG};
use makepad_flow::host::{FlowServer, FlowServerConfig};
use makepad_flow::{Literal, Loc, Node, PortValue, Port, PortType, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Root(PathBuf);

impl Root {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("flow-results-{}-{label}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
            ("alias".into(), Literal::Str("flows/sunset-result".into())),
        ],
        inputs: Vec::new(),
        outputs: vec![Port { name: "asset".into(), ty: PortType::Json, many: None }],
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
    for alpha in pixels.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    let png = makepad_ai_hub::testpattern::encode_png_rgba(&pixels, 256, 256).unwrap();
    Value::media(PortType::Image, "image/png", png)
}

fn publish(store: &Arc<dyn ResultStore>, node: &Node, run: PublishRun, value: Value) -> makepad_strict_json::Value {
    let mut executor = PublishExecutor::new(Some(store.clone()), run);
    executor.start(node, &[("value".into(), PortValue::One(value))]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match executor.poll() {
            Poll::Done(outputs) => {
                let value = outputs.into_iter().find(|(port, _)| port == "asset").unwrap().1.one().cloned().unwrap();
                return makepad_strict_json::parse(&value.bytes).unwrap();
            }
            Poll::Failed(error) => panic!("publish failed: {error}"),
            _ if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            other => panic!("publish timed out after {other:?}"),
        }
    }
}

fn query(namespace: Option<&str>) -> ResultQuery {
    ResultQuery { text: String::new(), namespace: namespace.map(str::to_string), kind: None, limit: 10, cursor: None }
}

fn hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn publish_lists_reads_and_survives_a_reopen() {
    let root = Root::new("folder");
    let store: Arc<dyn ResultStore> = Arc::new(FolderStore::open(&root.0, "flows").unwrap());
    let run = PublishRun { flow: "prompt-to-library".into(), instance: "i1".into(), ..Default::default() };
    let first = publish(&store, &node(), run.clone(), picture(30));
    assert_eq!(first.get("alias").and_then(|v| v.as_str()), Some("flows/sunset-result"));
    assert_eq!(first.get("kind").and_then(|v| v.as_str()), Some("texture"));
    // A second picture under the same alias: the alias names the newest.
    std::thread::sleep(Duration::from_millis(5));
    let second = publish(&store, &node(), run, picture(90));
    assert_ne!(first.get("id"), second.get("id"));

    let page = store.list(query(Some("flows"))).unwrap();
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items.iter().filter(|item| item.alias.is_some()).count(), 1);
    assert_eq!(page.items[0].tags, vec!["flow", "sunset"]);
    assert!(store.list(query(Some("elsewhere"))).unwrap().items.is_empty());

    let by_alias = ResultRef::alias("flows/sunset-result");
    let content = store.read(&by_alias).unwrap();
    assert_eq!(content.content_type, "image/png");
    assert_eq!(content.bytes, picture(90).bytes.to_vec());
    let thumb = store.thumbnail(&by_alias).unwrap().expect("an image has a card");
    assert_eq!(thumb.content_type, "image/png");
    assert!(store.read(&ResultRef::id("missing")).unwrap_err().contains("not found"));
    let info = store.info(&by_alias).unwrap();
    assert_eq!((info.title.as_str(), info.kind), ("Sunset result", ResultKind::Texture));

    // Paging walks every result once.
    let mut small = query(None);
    small.limit = 1;
    let mut seen = Vec::new();
    loop {
        let page = store.list(small.clone()).unwrap();
        seen.extend(page.items.iter().map(|item| item.id.clone()));
        let Some(cursor) = page.cursor else { break };
        small.cursor = Some(cursor);
    }
    assert_eq!(seen.len(), 2);

    // A new process finds the same results.
    drop(store);
    let reopened = FolderStore::open(&root.0, "flows").unwrap();
    assert_eq!(reopened.list(query(None)).unwrap().items.len(), 2);
}

#[test]
fn a_derived_publish_is_found_by_digest_and_names_its_parent() {
    let root = Root::new("derived");
    let store: Arc<dyn ResultStore> = Arc::new(FolderStore::open(&root.0, "flows").unwrap());
    let source = picture(40);
    let source_ref = {
        let mut plain = node();
        plain.params.retain(|(name, _)| name != "alias");
        let published = publish(&store, &plain, PublishRun { flow: "f".into(), instance: "i".into(), ..Default::default() }, source.clone());
        ResultRef::id(published.get("id").and_then(|v| v.as_str()).unwrap())
    };
    // A host feeding a run library media notes where it came from.
    store.note_origin(source.digest, &source_ref);

    let recipe = [7u8; 32];
    let mut derived = node();
    derived.params.retain(|(name, _)| name != "alias" && name != "tags");
    derived.params.push(("recipe_digest".into(), Literal::Str(hex(&recipe))));
    let run = PublishRun {
        flow: "studio.Upscale".into(),
        instance: "i".into(),
        run: "run-1".into(),
        revision: 3,
        description_default: String::new(),
        lineage: Lineage {
            generators: vec![GeneratorRecord {
                node: "upscale".into(),
                type_name: "Upscale".into(),
                domain: "upscale".into(),
                model: "realesrgan".into(),
                seed: Some(42),
                params: vec![("factor".into(), Literal::Num(2.0))],
            }],
            prompt: "a sunset".into(),
            sources: vec![source.digest],
        },
    };
    let made = publish(&store, &derived, run, picture(200));
    let id = made.get("id").and_then(|v| v.as_str()).unwrap().to_string();

    let found = store.find_by_digest(&hex(&recipe)).expect("found by its reuse key");
    assert_eq!(found.id, id);
    assert!(store.find_by_digest(&hex(&[8u8; 32])).is_none());
    let info = store.info(&found).unwrap();
    let mut tags = info.tags.clone();
    tags.sort();
    assert_eq!(tags, ["flow", "studio-upscale", "upscaled"]);

    let sidecar = std::fs::read_to_string(root.0.join(format!("{id}.json"))).unwrap();
    let json = makepad_strict_json::parse(sidecar.as_bytes()).unwrap();
    let parents = json.get("parents").and_then(|v| v.as_arr()).unwrap();
    assert_eq!(parents.len(), 1);
    assert_eq!(parents[0].as_str(), Some(source_ref.id.as_str()));
    let provenance = json.get("provenance").unwrap();
    assert_eq!(provenance.get("flow").and_then(|v| v.as_str()), Some("studio.Upscale"));
    assert!(provenance.get("parents").is_some());

    // The digest is scoped by the store's namespace.
    let other = FolderStore::open(&root.0, "elsewhere").unwrap();
    assert!(other.find_by_digest(&hex(&recipe)).is_none());
}

#[test]
fn presets_are_flow_kind_results() {
    use makepad_flow::engine::results::{ResultFile, ResultFiles, ResultMedia, ResultRole};
    let root = Root::new("presets");
    let store = FolderStore::open(&root.0, "flows").unwrap();
    let source = b"use mod.flow.*\nFlow{}\n".to_vec();
    let files = ResultFiles {
        kind: ResultKind::Flow,
        namespace: "flows".into(),
        title: "My preset".into(),
        description: String::new(),
        alias: Some("flows/my-preset".into()),
        category: None,
        tags: vec![PRESET_TAG.into()],
        files: vec![ResultFile { role: ResultRole::Source, media: ResultMedia::Text, bytes: source.clone(), media_millis: 0, dims: None }],
        thumbnail: None,
        mesh_stats: None,
        creator: String::new(),
        artist: String::new(),
        generator: String::new(),
        model: String::new(),
        prompt: String::new(),
        seed: None,
        digest: None,
        generator_version: String::new(),
        provenance: Vec::new(),
        value_digest: [3u8; 32],
        sources: Vec::new(),
    };
    let events = store.publish(files).unwrap();
    let done = events.iter().find_map(|event| match event {
        makepad_flow::engine::results::PublishEvent::Done(result) => Some(result),
        _ => None,
    });
    done.unwrap().unwrap();
    let mut presets = query(None);
    presets.kind = Some(ResultKind::Flow);
    assert_eq!(store.list(presets).unwrap().items.len(), 1);
    let mut images = query(None);
    images.kind = Some(ResultKind::Texture);
    assert!(store.list(images).unwrap().items.is_empty());
    assert_eq!(store.read(&ResultRef::alias("flows/my-preset")).unwrap().bytes, source);
}

/// A bare server (no store configured) keeps results in `<root>/results`
/// and serves them on the `/v1/assets` routes.
#[test]
fn a_bare_server_serves_its_results_on_the_asset_routes() {
    let flow_root = Root::new("server");
    let mut config = FlowServerConfig::new(flow_root.0.clone());
    config.log = Box::new(|_| {});
    let server = FlowServer::start(config).unwrap();
    // Publish through a second handle on the same folder: what a run does.
    let store: Arc<dyn ResultStore> = Arc::new(FolderStore::open(flow_root.0.join("results"), "flows").unwrap());
    let published = publish(&store, &node(), PublishRun { flow: "f".into(), instance: "i".into(), ..Default::default() }, picture(60));
    let id = published.get("id").and_then(|v| v.as_str()).unwrap().to_string();
    drop(store);
    server.shutdown();

    // A restart reads what the folder holds.
    let mut config = FlowServerConfig::new(flow_root.0.clone());
    config.log = Box::new(|_| {});
    let server = FlowServer::start(config).unwrap();
    let served = server.endpoints();
    let client = FlowClient::connect(
        Endpoints { control: served.control, data: served.data },
        served.token.clone(),
        Some(served.server_id),
    )
    .unwrap();
    let rows = client.assets("Sunset", Some("flows"), 10).unwrap().assets;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].alias.as_deref(), Some("flows/sunset-result"));
    assert_eq!(client.asset_content(&id).unwrap().content_type, "image/png");
    let thumb = client.asset_thumbnail("flows/sunset-result").unwrap();
    assert_eq!(thumb.content_type, "image/png");
    client.asset_thumbnail(&id).unwrap();
    assert!(client.asset_preview(&id).unwrap().bytes.starts_with(b"\x89PNG"));
    assert!(client.asset_content("not/an-asset-id").is_err());
    server.shutdown();
}
