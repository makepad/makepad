//! The dispatcher's surviving surface against a REAL Asset Server over real
//! sockets (aicore §9): catalog reads work, the capability doc is honest,
//! and everything the store no longer executes — generation, transforms —
//! answers a typed refusal that names where that work lives now. The
//! generation/operations era of this suite left with the store's queue.

use makepad_asset_chat::dispatch::AssetServerTools;
use makepad_asset_chat::session::{CancelFlag, ExecCtx, Origin, SessionId, ToolExecutor};
use makepad_asset_chat::tools::{ContentGenerateKind, ContentToolCall, InspectTarget};
use makepad_asset_chat::wire::ToolOutcome;
use makepad_asset_client::json::Value;
use makepad_asset_client::{
    ApiEndpoints, AssetClient, ClientConfig, PublishFile, PublishRequest, PublishRights,
    PublishThumbnail,
};
use makepad_asset_data::{AssetAlias, AssetKind, FileRole, MediaType, ThumbnailMedia};
use std::collections::HashSet;
use makepad_asset_store::{AssetServer, ServerConfig};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn test_root(name: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("mp_chat_e2e_{}_{}_{}", std::process::id(), n, name))
}

fn start_server(name: &str) -> (AssetServer, String) {
    let root = test_root(name);
    let mut cfg = ServerConfig::new(root.clone());
    cfg.control_addr = "127.0.0.1:0".parse().unwrap();
    cfg.data_addr = "127.0.0.1:0".parse().unwrap();
    cfg.bootstrap_admin = true;
    cfg.log = false;
    let server = AssetServer::start(cfg).expect("server start");
    let token = std::fs::read_to_string(root.join("admin-token"))
        .expect("admin token")
        .trim()
        .to_string();
    (server, token)
}

fn endpoints(server: &AssetServer) -> ApiEndpoints {
    ApiEndpoints { control: server.control_addr(), data: server.data_addr() }
}

fn tools_for(server: &AssetServer, token: &str) -> AssetServerTools {
    AssetServerTools::connect(endpoints(server), Some(token.to_string()), "gen")
        .expect("dispatcher connect")
}

fn run(tools: &mut AssetServerTools, call: &ContentToolCall) -> ToolOutcome {
    let origin = Origin {
        principal: "prin_admin".to_string(),
        session: SessionId::parse("chat_0123456789abcdef").expect("session"),
    };
    let known = HashSet::new();
    let ctx = ExecCtx { origin: &origin, known: &known };
    let cancel = CancelFlag::default();
    tools.execute(call, &ctx, &mut |_, _| {}, &cancel)
}

fn publish_seed(server: &AssetServer, token: &str) {
    let mut cfg = ClientConfig::new(test_root("seed_cache"));
    cfg.token = Some(token.to_string());
    let mut client =
        AssetClient::connect(cfg, endpoints(server), Some(server.server_id())).expect("connect");
    let mut request = PublishRequest::new(
        "gen",
        AssetKind::Texture,
        "a weathered brick texture".to_string(),
        PublishFile {
            bytes: b"seed-bytes-brick".to_vec(),
            media: MediaType::Png,
            role: FileRole::Texture,
            media_millis: 0,
            dims: Some((64, 64)),
        },
        PublishThumbnail {
            bytes: vec![0xCD; 1_500],
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
            views: Vec::new(),
        },
    );
    request.rights = PublishRights::generated_cc0();
    request.alias = AssetAlias::from_str("gen/brick").ok();
    client.publish_artifact(&request).expect("seed publish");
}

/// Catalog reads work over real sockets: search finds the published row and
/// inspect resolves its alias to a typed summary.
#[test]
fn catalog_reads_work_over_real_sockets() {
    let (server, token) = start_server("reads");
    publish_seed(&server, &token);
    let mut tools = tools_for(&server, &token);

    match run(
        &mut tools,
        &ContentToolCall::AssetSearch { query: "brick".to_string(), limit: 8 },
    ) {
        ToolOutcome::Ok { value } => {
            let hits = value.get("hits").and_then(Value::as_arr).expect("hits");
            assert!(
                hits.iter().any(|h| {
                    h.get("title").and_then(Value::as_str).is_some_and(|t| t.contains("brick"))
                }),
                "search finds the seed row"
            );
        }
        other => panic!("search failed: {other:?}"),
    }

    let alias = AssetAlias::from_str("gen/brick").expect("alias");
    match run(&mut tools, &ContentToolCall::AssetInspect { target: InspectTarget::Alias(alias) }) {
        ToolOutcome::Ok { value } => {
            assert_eq!(value.get("alias").and_then(Value::as_str), Some("gen/brick"));
            assert!(value.get("revision").and_then(Value::as_str).is_some());
        }
        other => panic!("inspect failed: {other:?}"),
    }
}

/// Everything the store no longer executes answers a typed refusal naming
/// where the work lives now — never an error string, never a 404 surprise.
#[test]
fn moved_work_is_refused_by_name() {
    let (server, token) = start_server("refusals");
    let mut tools = tools_for(&server, &token);

    match run(
        &mut tools,
        &ContentToolCall::ContentGenerate {
            kind: ContentGenerateKind::Prop,
            prompt: "a wooden cart".to_string(),
            dim_height: None,
        },
    ) {
        ToolOutcome::Unavailable { reason } => {
            assert!(reason.contains("creator"), "{reason}");
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }

    match run(&mut tools, &ContentToolCall::OperationCapabilities) {
        ToolOutcome::Unavailable { reason } => {
            assert!(reason.contains("creator apps"), "{reason}");
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }

    let doc = tools.capability_doc();
    assert!(doc.contains("asset.search"), "{doc}");
    assert!(doc.contains("creator apps"), "{doc}");
}
