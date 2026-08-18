//! THE dispatch proof: the compact operation tool surface against a REAL
//! Asset Server over real sockets. A deterministic in-test worker stands in
//! for the GPU fleet (it claims the armed executor job, uploads typed
//! output blobs, and reports facts to the atomic finalizer), so the whole
//! tool contract is exercised end to end:
//!
//! - honest operation availability (registered kinds x live workers),
//! - typed inputs pinned to session-known revisions,
//! - idempotent creation (dispatcher-derived keys join replays),
//! - progress observation through operation.wait, cancellation, retry,
//! - outputs as NEW immutable revisions with exact parent lineage,
//!   inherited rights, and real model facts — while the source revision
//!   stays byte-identical,
//! - and the structural absence of raw job/publish/alias tools.

use makepad_asset_store::{AssetServer, ServerConfig};
use makepad_asset_chat::dispatch::AssetServerTools;
use makepad_asset_chat::fleet_http;
use makepad_asset_chat::session::{CancelFlag, ExecCtx, Origin, SessionId, ToolExecutor};
use makepad_asset_chat::tools::{ContentToolCall, InspectTarget, OperationInputArg, PublicationArg};
use makepad_asset_chat::wire::ToolOutcome;
use makepad_asset_client::json::{self, Value};
use makepad_asset_client::{
    Api, ApiEndpoints, AssetClient, ClientConfig, HttpLimits, OperationFinalizeRequest,
    OperationId, OperationOutputFile, PublishFile, PublishRequest, PublishRights,
    PublishThumbnail,
};
use makepad_asset_data::{
    AssetKind, AssetRevisionId, DerivativePolicy, DeviceTier, FileRole, MediaType,
    Redistribution, ThumbnailMedia,
};
use std::collections::HashSet;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

const OP_KIND: &str = "mesh.from_image.v1";
const EXEC_KIND: &str = "op.mesh.from_image.v1";

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

fn connect(server: &AssetServer, token: &str, cache: &str) -> AssetClient {
    let mut cfg = ClientConfig::new(test_root(cache));
    cfg.token = Some(token.to_string());
    AssetClient::connect(cfg, endpoints(server), Some(server.server_id())).expect("client connect")
}

fn tools_for(server: &AssetServer, token: &str) -> AssetServerTools {
    AssetServerTools::connect(endpoints(server), Some(token.to_string()), "gen")
        .expect("dispatcher connect")
}

/// Raw typed API for the in-test worker (claim, upload, finalize).
fn worker_api(server: &AssetServer, token: &str) -> Api {
    Api::new(endpoints(server), HttpLimits::default_v1(), Some(token.to_string()))
        .expect("worker api")
}

fn stable_session() -> SessionId {
    SessionId::parse("chat_0123456789abcdef").expect("stable session")
}

fn sibling_session() -> SessionId {
    SessionId::parse("chat_fedcba9876543210").expect("sibling session")
}

fn origin_of(session: SessionId) -> Origin {
    Origin { principal: "prin_admin".to_string(), session }
}

/// Publish a deterministic seed image with a DISTINCT attribution-bearing
/// rights record so inheritance is observable.
fn seed_image(client: &mut AssetClient, tag: &str) -> (makepad_asset_data::AssetId, AssetRevisionId) {
    let mut request = PublishRequest::new(
        "gen",
        AssetKind::Texture,
        format!("seed {tag}"),
        PublishFile {
            bytes: format!("seed-bytes-{tag}").into_bytes(),
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
        },
    );
    request.rights = PublishRights::declared(
        "CC-BY-4.0",
        "Seed Author",
        "https://example.com/seed",
        Redistribution::AttributionRequired,
        DerivativePolicy::AttributionRequired,
    );
    request.prompt = format!("seed prompt {tag}");
    let published = client.publish_artifact(&request).expect("seed publish");
    (published.asset_id, published.revision)
}

fn execute(
    tools: &mut AssetServerTools,
    known: &HashSet<AssetRevisionId>,
    call: &ContentToolCall,
) -> ToolOutcome {
    execute_with_progress(tools, known, call, &mut |_p, _n| {}, &CancelFlag::default())
}

fn execute_with_progress(
    tools: &mut AssetServerTools,
    known: &HashSet<AssetRevisionId>,
    call: &ContentToolCall,
    progress: &mut dyn FnMut(u16, &str),
    cancel: &CancelFlag,
) -> ToolOutcome {
    execute_as(tools, &origin_of(stable_session()), known, call, progress, cancel)
}

fn execute_as(
    tools: &mut AssetServerTools,
    origin: &Origin,
    known: &HashSet<AssetRevisionId>,
    call: &ContentToolCall,
    progress: &mut dyn FnMut(u16, &str),
    cancel: &CancelFlag,
) -> ToolOutcome {
    let ctx = ExecCtx { origin, known };
    tools.execute(call, &ctx, progress, cancel)
}

fn ok_value(outcome: &ToolOutcome) -> &Value {
    match outcome {
        ToolOutcome::Ok { value } => value,
        other => panic!("expected Ok outcome, got {other:?}"),
    }
}

fn op_of(outcome: &ToolOutcome) -> OperationId {
    OperationId::parse(
        ok_value(outcome).get("operation").and_then(Value::as_str).expect("operation id"),
    )
    .expect("operation id parse")
}

fn create_call(
    asset: makepad_asset_data::AssetId,
    revision: AssetRevisionId,
) -> ContentToolCall {
    ContentToolCall::OperationCreate {
        kind: OP_KIND.to_string(),
        inputs: vec![OperationInputArg {
            slot: "image".into(),
            asset,
            revision,
            role: "texture".into(),
            tier: None,
            lod: None,
            media: Some("png".into()),
        }],
        params: Value::Obj(vec![]),
        publication: PublicationArg::Publish,
        idempotency_key: None,
    }
}

/// The deterministic stand-in for the GPU fleet: claim one armed executor
/// job, verify its pinned input, upload derived output blobs, heartbeat
/// progress, and report typed facts to the atomic finalizer. Never touches
/// publish/alias routes itself.
fn run_worker_once(api: &Api, tag: u8) -> u32 {
    let Some(job) = api
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .expect("claim")
    else {
        return 0;
    };
    assert_eq!(job.kind, EXEC_KIND);
    let op = OperationId::parse(
        job.body.get("operation").and_then(Value::as_str).expect("operation in body"),
    )
    .expect("operation id");
    let input = &job.body.get("inputs").and_then(Value::as_arr).expect("inputs")[0];
    let pinned_blob: makepad_asset_data::BlobId = input
        .get("blob")
        .and_then(Value::as_str)
        .expect("pinned blob")
        .parse()
        .expect("blob id");
    let _ = pinned_blob;
    let seed = job
        .body
        .get("params")
        .and_then(|p| p.get("seed"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    api.worker_heartbeat(&job.job, 60_000, Some("w1"), Some((500, "meshing")))
        .expect("heartbeat");

    let glb = vec![tag; 900];
    let thumb = vec![tag ^ 0xFF; 1_400];
    let glb_blob = api.upload_blob("gen", &glb).expect("glb upload");
    let thumb_blob = api.upload_blob("gen", &thumb).expect("thumb upload");
    api.operation_finalize(
        &op,
        &OperationFinalizeRequest {
            job: job.job,
            suffix: Some("w1".into()),
            output_name: "mesh".into(),
            files: vec![OperationOutputFile {
                role: FileRole::RenderGlb,
                tier: DeviceTier::Any,
                lod: 0,
                media: MediaType::Glb,
                blob: glb_blob,
                byte_len: glb.len() as u64,
                dims: None,
            }],
            thumbnail: Some((thumb_blob, "png", 512, 512, thumb.len() as u64)),
            metrics: (glb.len() as u64 + thumb.len() as u64, 240, 128, 0, 0, 512, 0),
            bounds: None,
            generator: "trellis".into(),
            model: "trellis-image-large".into(),
            version: "1".into(),
            seed,
        },
    )
    .expect("finalize");
    1
}

/// Mark the executor kind live (an empty claim poll is the liveness signal).
fn worker_alive(api: &Api) {
    assert!(api
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .expect("liveness poll")
        .is_none());
}

// ---------------------------------------------------------------- the tests

#[test]
fn operation_tools_publish_mesh_with_lineage_and_inherited_rights() {
    let (mut server, token) = start_server("op_flow");
    let mut client = connect(&server, &token, "flow_seed");
    let mut tools = tools_for(&server, &token);
    let worker = worker_api(&server, &token);

    let (seed_asset, seed_rev) = seed_image(&mut client, "hero");
    let source_bytes_before = client
        .fetch_asset_manifest(&seed_rev)
        .and_then(|m| client.fetch_blob_bytes(&m.files[0].blob, Some(m.files[0].byte_len)))
        .expect("source bytes");

    // Availability is honest end to end: unavailable before any worker...
    let outcome = execute(
        &mut tools,
        &[seed_rev].into_iter().collect(),
        &create_call(seed_asset, seed_rev),
    );
    match &outcome {
        ToolOutcome::Unavailable { reason } => {
            assert!(reason.contains(OP_KIND), "{reason}")
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
    // ...and the capability doc says so, then flips once a worker polls.
    let doc = tools.capability_doc();
    assert!(doc.contains("UNAVAILABLE"), "{doc}");
    worker_alive(&worker);
    let doc = tools.capability_doc();
    assert!(doc.contains("[available]"), "{doc}");
    assert!(doc.contains(OP_KIND), "{doc}");

    // operation.capabilities is the structured twin of the doc.
    let outcome = execute(&mut tools, &HashSet::new(), &ContentToolCall::OperationCapabilities);
    let caps = ok_value(&outcome);
    let ops = caps.get("operations").and_then(Value::as_arr).expect("operations");
    assert!(ops.iter().any(|o| {
        o.get("kind").and_then(Value::as_str) == Some(OP_KIND)
            && o.get("available").and_then(Value::as_bool) == Some(true)
    }));

    // Create through the tool (input bound to the session).
    let known: HashSet<_> = [seed_rev].into_iter().collect();
    let outcome = execute(&mut tools, &known, &create_call(seed_asset, seed_rev));
    let op = op_of(&outcome);
    assert_eq!(
        ok_value(&outcome).get("state").and_then(Value::as_str),
        Some("queued")
    );
    assert_eq!(
        ok_value(&outcome).get("joined").and_then(Value::as_bool),
        Some(false)
    );

    // An identical replay JOINS the same operation (dispatcher-derived key).
    let outcome = execute(&mut tools, &known, &create_call(seed_asset, seed_rev));
    assert_eq!(op_of(&outcome), op);
    assert_eq!(
        ok_value(&outcome).get("joined").and_then(Value::as_bool),
        Some(true)
    );

    // The worker executes; operation.wait streams progress and returns the
    // terminal truth with the durable events.
    assert_eq!(run_worker_once(&worker, 0x11), 1);
    let mut notes = Vec::new();
    let outcome = execute_with_progress(
        &mut tools,
        &known,
        &ContentToolCall::OperationWait { operation: op, after: 0, timeout_ms: 30_000 },
        &mut |permille, note| notes.push((permille, note.to_string())),
        &CancelFlag::default(),
    );
    let status = ok_value(&outcome);
    assert_eq!(status.get("state").and_then(Value::as_str), Some("succeeded"));
    let events = status.get("events").and_then(Value::as_arr).expect("events");
    let kinds: Vec<&str> = events
        .iter()
        .filter_map(|e| e.get("kind").and_then(Value::as_str))
        .collect();
    assert_eq!(kinds, vec!["created", "succeeded"]);

    // A NEW immutable revision with exact parent lineage, real model facts,
    // and the seed's FULL rights record.
    let new_rev = AssetRevisionId::from_str(
        status
            .get("result")
            .and_then(|r| r.get("revision"))
            .and_then(Value::as_str)
            .expect("result revision"),
    )
    .unwrap();
    assert_ne!(new_rev, seed_rev);
    let manifest = client.fetch_asset_manifest(&new_rev).expect("mesh manifest");
    assert_eq!(manifest.kind, AssetKind::Mesh);
    assert!(manifest.files.iter().any(|f| f.role == FileRole::RenderGlb));
    let prov = manifest.provenance.as_ref().expect("provenance");
    assert_eq!(prov.parents, vec![seed_rev]);
    assert_eq!(prov.generator, "trellis");
    let seed_manifest = client.fetch_asset_manifest(&seed_rev).expect("seed manifest");
    assert_eq!(manifest.rights, seed_manifest.rights, "rights inherit verbatim");

    // The source revision is untouched, byte for byte.
    let source_bytes_after = client
        .fetch_asset_manifest(&seed_rev)
        .and_then(|m| client.fetch_blob_bytes(&m.files[0].blob, Some(m.files[0].byte_len)))
        .expect("source bytes after");
    assert_eq!(source_bytes_before, source_bytes_after);

    // asset.inspect on the new revision surfaces the lineage; asset.search
    // finds the published output.
    let outcome = execute(
        &mut tools,
        &known,
        &ContentToolCall::AssetInspect { target: InspectTarget::Revision(new_rev) },
    );
    let summary = ok_value(&outcome);
    assert_eq!(summary.get("kind").and_then(Value::as_str), Some("mesh"));
    let parents = summary.get("parents").and_then(Value::as_arr).expect("parents");
    assert_eq!(parents[0].as_str(), Some(seed_rev.to_string()).as_deref());
    let outcome = execute(
        &mut tools,
        &known,
        &ContentToolCall::AssetSearch { query: "mesh from image".into(), limit: 10 },
    );
    let hits = ok_value(&outcome).get("hits").and_then(Value::as_arr).unwrap();
    assert!(!hits.is_empty(), "operation output must be searchable");

    server.shutdown();
}

#[test]
fn create_refusals_are_typed_and_precede_any_job() {
    let (mut server, token) = start_server("op_refusals");
    let mut client = connect(&server, &token, "refusals_seed");
    let mut tools = tools_for(&server, &token);
    let worker = worker_api(&server, &token);
    let (seed_asset, seed_rev) = seed_image(&mut client, "ref");
    worker_alive(&worker);

    // (1) Unregistered operation kind -> structured Unavailable.
    let mut call = create_call(seed_asset, seed_rev);
    if let ContentToolCall::OperationCreate { kind, .. } = &mut call {
        *kind = "video.upscale.v1".into();
    }
    let known: HashSet<_> = [seed_rev].into_iter().collect();
    match execute(&mut tools, &known, &call) {
        ToolOutcome::Unavailable { reason } => assert!(reason.contains("video.upscale.v1")),
        other => panic!("expected Unavailable, got {other:?}"),
    }

    // (2) Unpinned (fabricated) input -> Refused BEFORE any server call.
    let fabricated = AssetRevisionId::from_bytes([0x5A; 32]);
    match execute(&mut tools, &known, &create_call(seed_asset, fabricated)) {
        ToolOutcome::Refused { what } => assert!(what.contains("not bound"), "{what}"),
        other => panic!("expected Refused, got {other:?}"),
    }

    // (3) Unknown role vocabulary -> Refused before the server sees it.
    let mut call = create_call(seed_asset, seed_rev);
    if let ContentToolCall::OperationCreate { inputs, .. } = &mut call {
        inputs[0].role = "sourcefile".into();
    }
    match execute(&mut tools, &known, &call) {
        ToolOutcome::Refused { what } => assert!(what.contains("role"), "{what}"),
        other => panic!("expected Refused, got {other:?}"),
    }

    // (4) A media guard that contradicts the pinned file -> server Refused.
    let mut call = create_call(seed_asset, seed_rev);
    if let ContentToolCall::OperationCreate { inputs, .. } = &mut call {
        inputs[0].media = Some("jpeg".into());
    }
    match execute(&mut tools, &known, &call) {
        ToolOutcome::Refused { what } => assert!(what.contains("conflict") || what.contains("media"), "{what}"),
        other => panic!("expected Refused, got {other:?}"),
    }

    // Nothing above armed a job.
    assert!(worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .unwrap()
        .is_none());

    server.shutdown();
}

#[test]
fn unprivileged_principal_is_denied_typed() {
    let (mut server, token) = start_server("op_acl");
    let mut client = connect(&server, &token, "acl_seed");
    let (seed_asset, seed_rev) = seed_image(&mut client, "acl");
    let worker = worker_api(&server, &token);
    worker_alive(&worker);

    // Mint a principal with NO grants via the raw auth routes (root token).
    let control = format!("http://{}", server.control_addr());
    let (status, created) = fleet_http::request_json(
        "POST",
        &format!("{control}/v1/auth/principals"),
        Some(&json::obj(vec![("name", json::s("chat-visitor"))])),
        Some(&token),
    )
    .expect("principal create");
    assert_eq!(status, 201);
    let principal = created.get("principal").and_then(Value::as_str).unwrap().to_string();
    let (status, minted) = fleet_http::request_json(
        "POST",
        &format!("{control}/v1/auth/tokens"),
        Some(&json::obj(vec![("principal", json::s(principal)), ("ttl_ms", Value::Int(600_000))])),
        Some(&token),
    )
    .expect("token create");
    assert_eq!(status, 201);
    let visitor_token = minted.get("token").and_then(Value::as_str).unwrap().to_string();

    let mut tools = tools_for(&server, &visitor_token);
    // Reads are authenticated-only: inspection works...
    let outcome = execute(
        &mut tools,
        &HashSet::new(),
        &ContentToolCall::AssetInspect { target: InspectTarget::Revision(seed_rev) },
    );
    assert!(matches!(outcome, ToolOutcome::Ok { .. }), "read should pass: {outcome:?}");
    // ...but creating operations is Denied.
    let known: HashSet<_> = [seed_rev].into_iter().collect();
    let outcome = execute(&mut tools, &known, &create_call(seed_asset, seed_rev));
    assert!(matches!(outcome, ToolOutcome::Denied { .. }), "expected Denied: {outcome:?}");

    server.shutdown();
}

#[test]
fn cancel_via_tool_kills_the_operation_and_no_revision_appears() {
    let (mut server, token) = start_server("op_cancel");
    let mut client = connect(&server, &token, "cancel_seed");
    let mut tools = tools_for(&server, &token);
    let worker = worker_api(&server, &token);
    let (seed_asset, seed_rev) = seed_image(&mut client, "c");
    worker_alive(&worker);

    let known: HashSet<_> = [seed_rev].into_iter().collect();
    let outcome = execute(&mut tools, &known, &create_call(seed_asset, seed_rev));
    let op = op_of(&outcome);

    // No worker runs. The user cancels; wait honors the cancel flag by
    // propagating it server-side and reporting the terminal truth.
    let cancel = CancelFlag::default();
    cancel.cancel();
    let outcome = execute_with_progress(
        &mut tools,
        &known,
        &ContentToolCall::OperationWait { operation: op, after: 0, timeout_ms: 30_000 },
        &mut |_p, _n| {},
        &cancel,
    );
    let status = ok_value(&outcome);
    assert_eq!(status.get("state").and_then(Value::as_str), Some("cancelled"));
    assert!(status.get("result").is_none());

    // A late worker cannot resurrect it: nothing claimable remains.
    assert!(worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .unwrap()
        .is_none());

    server.shutdown();
}

#[test]
fn retry_via_tool_arms_the_next_round() {
    let (mut server, token) = start_server("op_retry");
    let mut client = connect(&server, &token, "retry_seed");
    let mut tools = tools_for(&server, &token);
    let worker = worker_api(&server, &token);
    let (seed_asset, seed_rev) = seed_image(&mut client, "r");
    worker_alive(&worker);

    let known: HashSet<_> = [seed_rev].into_iter().collect();
    let outcome = execute(&mut tools, &known, &create_call(seed_asset, seed_rev));
    let op = op_of(&outcome);

    // The worker claims and fails terminally.
    let claimed = worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .unwrap()
        .expect("claim");
    let error = json::obj(vec![("error", json::s("gpu exploded"))]);
    worker
        .worker_fail(&claimed.job, Some("w1"), 0, Some(&error))
        .expect("fail");

    let outcome = execute(
        &mut tools,
        &known,
        &ContentToolCall::OperationGet { operation: op },
    );
    let status = ok_value(&outcome);
    assert_eq!(status.get("state").and_then(Value::as_str), Some("failed"));
    assert!(status.get("error").is_some());

    // Retry through the tool; the next round completes.
    let outcome = execute(
        &mut tools,
        &known,
        &ContentToolCall::OperationRetry { operation: op },
    );
    let status = ok_value(&outcome);
    assert_eq!(status.get("state").and_then(Value::as_str), Some("queued"));
    assert_eq!(status.get("round").and_then(Value::as_i64), Some(1));

    assert_eq!(run_worker_once(&worker, 0x22), 1);
    let outcome = execute(
        &mut tools,
        &known,
        &ContentToolCall::OperationWait { operation: op, after: 0, timeout_ms: 30_000 },
    );
    let status = ok_value(&outcome);
    assert_eq!(status.get("state").and_then(Value::as_str), Some("succeeded"));

    server.shutdown();
}

#[test]
fn sibling_session_cannot_control_foreign_operation() {
    let (mut server, token) = start_server("op_session");
    let mut client = connect(&server, &token, "session_seed");
    let mut tools = tools_for(&server, &token);
    let worker = worker_api(&server, &token);
    let (seed_asset, seed_rev) = seed_image(&mut client, "s");
    worker_alive(&worker);
    let known: HashSet<_> = [seed_rev].into_iter().collect();
    let owner = origin_of(stable_session());
    let sibling = origin_of(sibling_session());
    let noop = CancelFlag::default();

    let outcome = execute_as(
        &mut tools,
        &owner,
        &known,
        &create_call(seed_asset, seed_rev),
        &mut |_p, _n| {},
        &noop,
    );
    let op = op_of(&outcome);

    for call in [
        ContentToolCall::OperationGet { operation: op },
        ContentToolCall::OperationWait { operation: op, after: 0, timeout_ms: 1 },
        ContentToolCall::OperationCancel { operation: op },
        ContentToolCall::OperationRetry { operation: op },
    ] {
        let outcome = execute_as(&mut tools, &sibling, &known, &call, &mut |_p, _n| {}, &noop);
        assert!(
            matches!(outcome, ToolOutcome::Denied { .. }),
            "sibling must be denied for {call:?}: {outcome:?}"
        );
    }

    let owner_get = execute_as(
        &mut tools,
        &owner,
        &known,
        &ContentToolCall::OperationGet { operation: op },
        &mut |_p, _n| {},
        &noop,
    );
    assert!(matches!(owner_get, ToolOutcome::Ok { .. }), "{owner_get:?}");

    server.shutdown();
}

#[test]
fn create_idempotency_is_session_bound_and_canonical() {
    let (mut server, token) = start_server("op_idemp");
    let mut client = connect(&server, &token, "idemp_seed");
    let mut tools = tools_for(&server, &token);
    let worker = worker_api(&server, &token);
    let (seed_asset, seed_rev) = seed_image(&mut client, "i");
    worker_alive(&worker);
    let known: HashSet<_> = [seed_rev].into_iter().collect();
    let owner = origin_of(stable_session());
    let sibling = origin_of(sibling_session());
    let noop = CancelFlag::default();

    let mut a = create_call(seed_asset, seed_rev);
    if let ContentToolCall::OperationCreate { params, .. } = &mut a {
        *params = json::obj(vec![("seed", Value::Int(3))]);
    }
    let mut b = create_call(seed_asset, seed_rev);
    if let ContentToolCall::OperationCreate { params, .. } = &mut b {
        *params = json::obj(vec![("seed", Value::Int(3))]);
    }

    let first = execute_as(&mut tools, &owner, &known, &a, &mut |_p, _n| {}, &noop);
    let op = op_of(&first);
    let joined = execute_as(&mut tools, &owner, &known, &b, &mut |_p, _n| {}, &noop);
    assert_eq!(op_of(&joined), op);
    assert_eq!(ok_value(&joined).get("joined").and_then(Value::as_bool), Some(true));

    let other = execute_as(&mut tools, &sibling, &known, &a, &mut |_p, _n| {}, &noop);
    assert_ne!(op_of(&other), op, "different sessions must not join");

    let mut keyed = create_call(seed_asset, seed_rev);
    if let ContentToolCall::OperationCreate { idempotency_key, .. } = &mut keyed {
        *idempotency_key = Some("retry-1".into());
    }
    let k1 = execute_as(&mut tools, &owner, &known, &keyed, &mut |_p, _n| {}, &noop);
    let k2 = execute_as(&mut tools, &sibling, &known, &keyed, &mut |_p, _n| {}, &noop);
    assert_ne!(op_of(&k1), op_of(&k2), "supplied keys are session-hashed");

    server.shutdown();
}

#[test]
fn origin_principal_does_not_scope_ops_or_idempotency() {
    // Same SessionId, different caller principal: join and control still
    // work. Principal is not hashed and is not sent to the Asset Server.
    let (mut server, token) = start_server("op_principal");
    let mut client = connect(&server, &token, "principal_seed");
    let mut tools = tools_for(&server, &token);
    let worker = worker_api(&server, &token);
    let (seed_asset, seed_rev) = seed_image(&mut client, "p");
    worker_alive(&worker);
    let known: HashSet<_> = [seed_rev].into_iter().collect();
    let session = stable_session();
    let a = Origin { principal: "prin_one".into(), session: session.clone() };
    let b = Origin { principal: "prin_two".into(), session };
    let noop = CancelFlag::default();

    let first = execute_as(&mut tools, &a, &known, &create_call(seed_asset, seed_rev), &mut |_p, _n| {}, &noop);
    let op = op_of(&first);
    let joined = execute_as(&mut tools, &b, &known, &create_call(seed_asset, seed_rev), &mut |_p, _n| {}, &noop);
    assert_eq!(op_of(&joined), op);
    assert_eq!(ok_value(&joined).get("joined").and_then(Value::as_bool), Some(true));

    let got = execute_as(
        &mut tools,
        &b,
        &known,
        &ContentToolCall::OperationGet { operation: op },
        &mut |_p, _n| {},
        &noop,
    );
    assert!(matches!(got, ToolOutcome::Ok { .. }), "{got:?}");

    server.shutdown();
}

#[test]
fn retire_session_denies_later_control() {
    let (mut server, token) = start_server("op_retire");
    let mut client = connect(&server, &token, "retire_seed");
    let mut tools = tools_for(&server, &token);
    let worker = worker_api(&server, &token);
    let (seed_asset, seed_rev) = seed_image(&mut client, "t");
    worker_alive(&worker);
    let known: HashSet<_> = [seed_rev].into_iter().collect();
    let owner = origin_of(stable_session());
    let noop = CancelFlag::default();

    let outcome = execute_as(
        &mut tools,
        &owner,
        &known,
        &create_call(seed_asset, seed_rev),
        &mut |_p, _n| {},
        &noop,
    );
    let op = op_of(&outcome);
    assert!(tools.retire_session(&stable_session()));
    assert!(!tools.retire_session(&stable_session()));

    for call in [
        ContentToolCall::OperationGet { operation: op },
        ContentToolCall::OperationWait { operation: op, after: 0, timeout_ms: 1 },
        ContentToolCall::OperationCancel { operation: op },
        ContentToolCall::OperationRetry { operation: op },
    ] {
        let outcome = execute_as(&mut tools, &owner, &known, &call, &mut |_p, _n| {}, &noop);
        assert!(
            matches!(outcome, ToolOutcome::Denied { .. }),
            "retired session must be denied for {call:?}: {outcome:?}"
        );
    }

    server.shutdown();
}
