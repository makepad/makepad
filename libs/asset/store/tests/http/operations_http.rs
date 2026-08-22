//! Typed asset operations over REAL sockets with the REAL shared client:
//! registry truthfulness, strict create parsing, idempotency join/conflict,
//! exact input pinning, worker execution + atomic finalize (lineage,
//! inherited rights, model facts, alias CAS), owner scoping, cancellation,
//! retry, and the finalize-only success guard.

mod common;
use common::*;

use makepad_asset_store::json::Value as SrvValue;
use makepad_asset_client::json::{self as cjson, Value};
use makepad_asset_client::{
    ApiEndpoints, AssetClient, ClientConfig, ClientError, OperationAliasExpect,
    OperationCreateRequest, OperationFinalizeRequest, OperationInputRef, OperationOutputFile,
    OperationPublicationRef, OperationStateDto, PublishFile, PublishRequest, PublishRights,
    PublishThumbnail,
};
use makepad_asset_data::{
    AssetAlias, AssetId, AssetKind, AssetRevisionId, DerivativePolicy, DeviceTier, FileRole,
    MediaType, Redistribution, ThumbnailMedia,
};
use std::str::FromStr;

const OP_KIND: &str = "mesh.from_image.v1";
const EXEC_KIND: &str = "op.mesh.from_image.v1";

fn real_client(ts: &TestServer, token: &str, leaf: &str) -> AssetClient {
    let mut cfg = ClientConfig::new(ts.root.join(leaf));
    cfg.token = Some(token.to_string());
    let endpoints = ApiEndpoints {
        control: ts.server.control_addr(),
        data: ts.server.data_addr(),
    };
    AssetClient::connect(cfg, endpoints, Some(ts.server.server_id())).expect("client connect")
}

/// Publish one PNG texture with a DISTINCT, attribution-bearing rights
/// record, so rights inheritance is observable (never the blanket grant).
fn publish_seed_texture(admin: &mut AssetClient, tag: &str) -> (AssetId, AssetRevisionId) {
    let mut request = PublishRequest::new(
        "gen",
        AssetKind::Texture,
        format!("seed image {tag}"),
        PublishFile {
            bytes: format!("png-bytes-{tag}").into_bytes(),
            media: MediaType::Png,
            role: FileRole::Texture,
            media_millis: 0,
            dims: Some((64, 64)),
        },
        PublishThumbnail {
            bytes: vec![0xAB; 1_500],
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
            views: Vec::new(),
        },
    );
    request.rights = PublishRights::declared(
        "CC-BY-4.0",
        "Seed Author",
        "https://example.com/seed",
        Redistribution::AttributionRequired,
        DerivativePolicy::AttributionRequired,
    );
    let published = admin.publish_artifact(&request).expect("seed publish");
    (published.asset_id, published.revision)
}

fn image_input(asset: AssetId, revision: AssetRevisionId) -> OperationInputRef {
    OperationInputRef {
        slot: "image".into(),
        asset,
        revision,
        role: FileRole::Texture,
        tier: None,
        lod: None,
        expected_media: Some(MediaType::Png),
    }
}

#[test]
fn full_slice_image_to_mesh_over_sockets() {
    let ts = start_server("op_full_slice");
    let admin_tok = ts.admin_token();
    let mut admin_http = ts.control(Some(&admin_tok));
    let creator_tok = principal_with(&mut admin_http, &[("operation_run", "gen")]);
    let worker_tok = principal_with(
        &mut admin_http,
        &[("job_worker", "gen"), ("blob_write", "gen")],
    );

    let mut admin = real_client(&ts, &admin_tok, "admin-cache");
    let (seed_asset, seed_rev) = publish_seed_texture(&mut admin, "hero");

    let creator = real_client(&ts, &creator_tok, "creator-cache");
    let worker = real_client(&ts, &worker_tok, "worker-cache");

    // (1) Truthful availability: no worker has offered the executor kind.
    let types = creator.operation_types().expect("types");
    let mesh_type = types.iter().find(|t| t.kind == OP_KIND).expect("registered");
    assert!(!mesh_type.available);
    assert!(mesh_type.unavailable_reason.is_some());

    // Creation refuses while unavailable — structured 409, not a silent queue.
    let request = OperationCreateRequest::new(
        "gen",
        OP_KIND,
        "slice-key-1",
        vec![image_input(seed_asset, seed_rev)],
    );
    match creator.operation_create(&request) {
        Err(ClientError::Server { status: 409, .. }) => {}
        other => panic!("expected 409 unavailable, got {other:?}"),
    }

    // (2) A worker poll (even on an empty queue) proves liveness.
    assert!(worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .expect("claim poll")
        .is_none());
    let types = creator.operation_types().expect("types");
    assert!(types.iter().find(|t| t.kind == OP_KIND).unwrap().available);

    // (3) Create pins the exact input.
    let status = creator.operation_create(&request).expect("create");
    assert_eq!(status.joined, Some(false));
    assert_eq!(status.state, OperationStateDto::Queued);
    assert_eq!(status.kind, OP_KIND);
    assert_eq!(status.round, 0);
    assert_eq!(status.inputs.len(), 1);
    assert_eq!(status.inputs[0].asset, seed_asset);
    assert_eq!(status.inputs[0].revision, seed_rev);
    assert_eq!(status.inputs[0].media, "png");
    let op = status.operation;

    // (4) Exact replay joins; a mismatched replay conflicts.
    let joined = creator.operation_create(&request).expect("replay");
    assert_eq!(joined.joined, Some(true));
    assert_eq!(joined.operation, op);
    let mut conflicting = request.clone();
    conflicting.params = cjson::obj(vec![("seed", Value::Int(7))]);
    match creator.operation_create(&conflicting) {
        Err(ClientError::Server { status: 409, .. }) => {}
        other => panic!("expected idempotency conflict, got {other:?}"),
    }

    // (5) The worker claims the executor job and reads its pinned inputs.
    let claimed = worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .expect("claim")
        .expect("job claimable");
    assert_eq!(claimed.kind, EXEC_KIND);
    assert_eq!(claimed.namespace, "gen");
    assert_eq!(
        claimed.body.get("operation").and_then(Value::as_str),
        Some(op.to_string()).as_deref()
    );
    let input = &claimed.body.get("inputs").and_then(Value::as_arr).expect("inputs")[0];
    let pinned_blob: makepad_asset_data::BlobId = input
        .get("blob")
        .and_then(Value::as_str)
        .expect("pinned blob")
        .parse()
        .expect("blob id");
    let pinned_len = input.get("byte_len").and_then(Value::as_u64).expect("len");

    // The worker fetches EXACTLY the pinned bytes (digest-verified).
    let mut worker_rw = real_client(&ts, &worker_tok, "worker-cache-rw");
    let bytes = worker_rw
        .fetch_blob_bytes(&pinned_blob, Some(pinned_len))
        .expect("input bytes");
    assert_eq!(bytes, b"png-bytes-hero".to_vec());

    // (6) Progress heartbeats surface on the owner's status.
    worker
        .worker_heartbeat(&claimed.job, 60_000, Some("w1"), Some((400, "meshing")))
        .expect("heartbeat");
    let live = creator.operation_get(&op).expect("get");
    assert_eq!(live.state, OperationStateDto::Running);
    let progress = live.progress.expect("progress");
    assert_eq!(progress.permille, 400);
    assert_eq!(progress.note, "meshing");

    // (7) Malicious facts refuse over the wire; nothing publishes.
    let glb = vec![0x11u8; 900];
    let thumb = vec![0xEEu8; 1_400];
    let glb_blob = upload_via(&ts, &worker_tok, &glb);
    let thumb_blob = upload_via(&ts, &worker_tok, &thumb);
    let good = OperationFinalizeRequest {
        job: claimed.job.clone(),
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
        seed: 0,
    };
    let mut lying_size = good.clone();
    lying_size.files[0].byte_len += 1;
    lying_size.metrics.0 += 1;
    match worker.operation_finalize(&op, &lying_size) {
        Err(ClientError::Server { status: 422, .. }) => {}
        other => panic!("expected size-lie refusal, got {other:?}"),
    }
    let mut lying_seed = good.clone();
    lying_seed.seed = 42;
    match worker.operation_finalize(&op, &lying_seed) {
        Err(ClientError::Server { status: 409, .. }) => {}
        other => panic!("expected seed-lie refusal, got {other:?}"),
    }
    let mut wrong_role = good.clone();
    wrong_role.files[0].role = FileRole::Video;
    wrong_role.files[0].media = MediaType::Mp4;
    match worker.operation_finalize(&op, &wrong_role) {
        Err(ClientError::Server { status: 400, .. }) => {}
        other => panic!("expected role refusal, got {other:?}"),
    }
    let after_refusals = creator.operation_get(&op).expect("get");
    assert_eq!(after_refusals.state, OperationStateDto::Running);
    assert!(after_refusals.result.is_none());

    // (8) The honest finalize lands atomically.
    let (out_asset, out_rev) = worker.operation_finalize(&op, &good).expect("finalize");
    let done = creator.operation_get(&op).expect("get");
    assert_eq!(done.state, OperationStateDto::Succeeded);
    assert_eq!(done.result, Some((out_asset, out_rev)));

    // The immutable manifest carries exact lineage, real model facts, the
    // server-computed spec digest, and the seed's FULL rights record.
    let mut reader = real_client(&ts, &creator_tok, "creator-cache-rw");
    let manifest = reader.fetch_asset_manifest(&out_rev).expect("mesh manifest");
    assert_eq!(manifest.kind, AssetKind::Mesh);
    let prov = manifest.provenance.as_ref().expect("provenance");
    assert_eq!(prov.parents, vec![seed_rev]);
    assert_eq!(prov.generator, "trellis");
    assert_eq!(prov.model, "trellis-image-large");
    assert_eq!(prov.seed, 0);
    assert_eq!(
        prov.params_digest.map(hex::to_hex_string),
        Some(done.spec_digest.clone()),
        "provenance digest must be the canonical spec digest"
    );
    let seed_manifest = reader.fetch_asset_manifest(&seed_rev).expect("seed manifest");
    assert_eq!(manifest.rights, seed_manifest.rights, "rights inherit verbatim");
    assert_eq!(manifest.rights.license, "CC-BY-4.0");
    assert_eq!(manifest.rights.credits, "Seed Author");

    // (9) Identical replay is idempotent; the durable events tell the story.
    let (again_asset, again_rev) = worker.operation_finalize(&op, &good).expect("replay");
    assert_eq!((again_asset, again_rev), (out_asset, out_rev));
    let events = creator.operation_events(&op, 0, 0, 64).expect("events");
    let kinds: Vec<&str> = events.events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, vec!["created", "succeeded"]);
    assert_eq!(events.cursor, events.events.last().unwrap().seq);

    // (10) The output is discoverable through search.
    let page = creator
        .catalog_search(
            &makepad_asset_client::CatalogQuery::text("mesh from image", 10),
            None,
        )
        .expect("search");
    assert!(
        page.hits.iter().any(|h| h.asset_id == out_asset),
        "published operation output must be searchable: {:?}",
        page.hits
    );
}

/// Upload bytes through the raw data-plane route (worker-side helper: the
/// typed Api is private inside AssetClient, and this test wants the exact
/// public wire a real worker binary uses).
fn upload_via(ts: &TestServer, token: &str, bytes: &[u8]) -> makepad_asset_data::BlobId {
    let mut data = ts.data(Some(token));
    let resp = data.post_bytes("/v1/blobs?ns=gen", bytes);
    assert_eq!(resp.status, 201, "blob upload: {:?}", String::from_utf8_lossy(&resp.body));
    resp.str_field("blob_id").parse().expect("blob id")
}

#[test]
fn strict_create_parsing_fails_closed() {
    let ts = start_server("op_strict_parse");
    let admin_tok = ts.admin_token();
    let mut admin_http = ts.control(Some(&admin_tok));
    let creator_tok = principal_with(&mut admin_http, &[("operation_run", "gen")]);
    let mut creator_http = ts.control(Some(&creator_tok));

    let base = |extra: Vec<(&str, SrvValue)>| {
        let mut pairs = vec![
            ("api_version", SrvValue::Int(1)),
            ("namespace", jstr("gen")),
            ("kind", jstr(OP_KIND)),
            ("idempotency_key", jstr("k1")),
            ("inputs", SrvValue::Arr(vec![jobj(vec![
                ("slot", jstr("image")),
                ("asset", jstr(&AssetId::from_bytes([1; 16]).to_string())),
                ("revision", jstr(&AssetRevisionId::from_bytes([2; 32]).to_string())),
                ("role", jstr("texture")),
            ])])),
        ];
        pairs.extend(extra);
        jobj(pairs)
    };

    // Unknown top-level field: refused BEFORE any state access.
    let resp = creator_http.post_json(
        "/v1/operations",
        &base(vec![("owner", jstr("prin_0000"))]),
    );
    assert_eq!(
        resp.status,
        400,
        "unknown field must refuse: {}",
        String::from_utf8_lossy(&resp.body)
    );

    // Unknown/unsupported api_version.
    let mut v2 = base(vec![]);
    if let SrvValue::Obj(pairs) = &mut v2 {
        pairs[0].1 = SrvValue::Int(2);
    }
    assert_eq!(creator_http.post_json("/v1/operations", &v2).status, 400);
    // Missing api_version entirely.
    let missing = jobj(vec![
        ("namespace", jstr("gen")),
        ("kind", jstr(OP_KIND)),
        ("idempotency_key", jstr("k1")),
        ("inputs", SrvValue::Arr(vec![])),
    ]);
    assert_eq!(creator_http.post_json("/v1/operations", &missing).status, 400);

    // Unknown input field.
    let bad_input = base(vec![]);
    let bad_input = match bad_input {
        SrvValue::Obj(mut pairs) => {
            let inputs = pairs.iter_mut().find(|(k, _)| k == "inputs").unwrap();
            inputs.1 = SrvValue::Arr(vec![jobj(vec![
                ("slot", jstr("image")),
                ("asset", jstr(&AssetId::from_bytes([1; 16]).to_string())),
                ("revision", jstr(&AssetRevisionId::from_bytes([2; 32]).to_string())),
                ("role", jstr("texture")),
                ("path", jstr("/etc/passwd")),
            ])]);
            SrvValue::Obj(pairs)
        }
        _ => unreachable!(),
    };
    assert_eq!(creator_http.post_json("/v1/operations", &bad_input).status, 400);

    // Unknown operation kind (unknown VERSION included) is a 404 even with
    // an otherwise valid body — checked before inputs resolve.
    let mut wrong_kind = base(vec![]);
    if let SrvValue::Obj(pairs) = &mut wrong_kind {
        pairs.iter_mut().find(|(k, _)| k == "kind").unwrap().1 = jstr("mesh.from_image.v9");
    }
    assert_eq!(creator_http.post_json("/v1/operations", &wrong_kind).status, 404);
}

#[test]
fn cross_owner_access_is_hidden_and_caps_enforced() {
    let ts = start_server("op_cross_owner");
    let admin_tok = ts.admin_token();
    let mut admin_http = ts.control(Some(&admin_tok));
    let creator_tok = principal_with(&mut admin_http, &[("operation_run", "gen")]);
    let other_tok = principal_with(&mut admin_http, &[("operation_run", "gen")]);
    let no_cap_tok = principal_with(&mut admin_http, &[]);
    let worker_tok = principal_with(&mut admin_http, &[("job_worker", "gen")]);

    let mut admin = real_client(&ts, &admin_tok, "admin-cache");
    let (seed_asset, seed_rev) = publish_seed_texture(&mut admin, "acl");
    let creator = real_client(&ts, &creator_tok, "creator-cache");
    let other = real_client(&ts, &other_tok, "other-cache");
    let no_cap = real_client(&ts, &no_cap_tok, "nocap-cache");
    let worker = real_client(&ts, &worker_tok, "worker-cache");

    // Liveness, then create as the owner.
    assert!(worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .unwrap()
        .is_none());
    let request = OperationCreateRequest::new(
        "gen",
        OP_KIND,
        "acl-key",
        vec![image_input(seed_asset, seed_rev)],
    );
    let status = creator.operation_create(&request).expect("create");
    let op = status.operation;

    // A different authenticated principal: the operation is INVISIBLE.
    for outcome in [
        other.operation_get(&op).err(),
        other.operation_events(&op, 0, 0, 16).err(),
        other.operation_cancel(&op).err(),
        other.operation_retry(&op).err(),
    ] {
        match outcome {
            Some(ClientError::NotFound { .. }) => {}
            other => panic!("cross-owner access must read as absent, got {other:?}"),
        }
    }

    // Same idempotency key under ANOTHER owner: an independent operation,
    // not a join and not a conflict.
    let foreign = other.operation_create(&request).expect("independent create");
    assert_eq!(foreign.joined, Some(false));
    assert_ne!(foreign.operation, op);

    // No operation_run capability: create refuses 403.
    match no_cap.operation_create(&request) {
        Err(ClientError::Denied) => {}
        other => panic!("expected Denied, got {other:?}"),
    }
    // ...and reads of foreign ids stay hidden for it too.
    match no_cap.operation_get(&op) {
        Err(ClientError::NotFound { .. }) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn cancel_stale_worker_and_raw_succeed_guard() {
    let ts = start_server("op_cancel_guard");
    let admin_tok = ts.admin_token();
    let mut admin_http = ts.control(Some(&admin_tok));
    let creator_tok = principal_with(&mut admin_http, &[("operation_run", "gen")]);
    let worker_tok = principal_with(
        &mut admin_http,
        &[("job_worker", "gen"), ("blob_write", "gen")],
    );
    let mut admin = real_client(&ts, &admin_tok, "admin-cache");
    let (seed_asset, seed_rev) = publish_seed_texture(&mut admin, "cg");
    let creator = real_client(&ts, &creator_tok, "creator-cache");
    let worker = real_client(&ts, &worker_tok, "worker-cache");

    assert!(worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .unwrap()
        .is_none());

    // (A) Cancellation beats the worker: heartbeat and finalize both refuse.
    let op_a = creator
        .operation_create(&OperationCreateRequest::new(
            "gen",
            OP_KIND,
            "cg-a",
            vec![image_input(seed_asset, seed_rev)],
        ))
        .expect("create")
        .operation;
    let claimed = worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .unwrap()
        .expect("claim");
    assert!(creator.operation_cancel(&op_a).expect("cancel"));
    assert!(!creator.operation_cancel(&op_a).expect("idempotent cancel"));
    match worker.worker_heartbeat(&claimed.job, 60_000, Some("w1"), None) {
        Err(ClientError::Server { status: 409, .. }) => {}
        other => panic!("cancelled job heartbeat must refuse, got {other:?}"),
    }
    let facts = finalize_request_via(&ts, &worker_tok, claimed.job.clone(), 0x21);
    match worker.operation_finalize(&op_a, &facts) {
        Err(ClientError::Server { status: 409, .. }) => {}
        other => panic!("cancelled finalize must refuse, got {other:?}"),
    }
    let got = creator.operation_get(&op_a).expect("get");
    assert_eq!(got.state, OperationStateDto::Cancelled);
    assert!(got.result.is_none());

    // (B) A worker cannot bypass the finalizer with a raw succeed.
    let op_b = creator
        .operation_create(&OperationCreateRequest::new(
            "gen",
            OP_KIND,
            "cg-b",
            vec![image_input(seed_asset, seed_rev)],
        ))
        .expect("create")
        .operation;
    let claimed = worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .unwrap()
        .expect("claim");
    match worker.worker_succeed(&claimed.job, Some("w1"), None) {
        Err(ClientError::Server { status: 409, .. }) => {}
        other => panic!("raw succeed on an operation job must refuse, got {other:?}"),
    }
    // The honest path still works afterwards.
    let facts = finalize_request_via(&ts, &worker_tok, claimed.job.clone(), 0x22);
    worker.operation_finalize(&op_b, &facts).expect("finalize");

    // (C) Compute failure -> operation failed -> retry -> next round lands.
    let op_c = creator
        .operation_create(&OperationCreateRequest::new(
            "gen",
            OP_KIND,
            "cg-c",
            vec![image_input(seed_asset, seed_rev)],
        ))
        .expect("create")
        .operation;
    let claimed = worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .unwrap()
        .expect("claim");
    let error = cjson::obj(vec![("error", cjson::s("gpu exploded"))]);
    worker
        .worker_fail(&claimed.job, Some("w1"), 0, Some(&error))
        .expect("fail");
    let failed = creator.operation_get(&op_c).expect("get");
    assert_eq!(failed.state, OperationStateDto::Failed);
    assert!(failed.error.is_some());
    let retried = creator.operation_retry(&op_c).expect("retry");
    assert_eq!(retried.state, OperationStateDto::Queued);
    assert_eq!(retried.round, 1);
    let claimed2 = worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .unwrap()
        .expect("round-1 claim");
    assert_ne!(claimed2.job, claimed.job, "round 1 is a fresh job");
    // The superseded round-0 job can never finalize.
    let stale = finalize_request_via(&ts, &worker_tok, claimed.job.clone(), 0x23);
    match worker.operation_finalize(&op_c, &stale) {
        Err(ClientError::Server { status: 409, .. }) => {}
        other => panic!("superseded finalize must refuse, got {other:?}"),
    }
    let facts = finalize_request_via(&ts, &worker_tok, claimed2.job.clone(), 0x24);
    worker.operation_finalize(&op_c, &facts).expect("round-1 finalize");
    let done = creator.operation_get(&op_c).expect("get");
    assert_eq!(done.state, OperationStateDto::Succeeded);
    let events = creator.operation_events(&op_c, 0, 0, 64).expect("events");
    let kinds: Vec<&str> = events.events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, vec!["created", "failed", "retried", "succeeded"]);
}

fn finalize_request_via(
    ts: &TestServer,
    worker_tok: &str,
    job: makepad_asset_client::JobId,
    tag: u8,
) -> OperationFinalizeRequest {
    let glb = vec![tag; 900];
    let thumb = vec![tag ^ 0xFF; 1_400];
    let glb_blob = upload_via(ts, worker_tok, &glb);
    let thumb_blob = upload_via(ts, worker_tok, &thumb);
    OperationFinalizeRequest {
        job,
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
        seed: 0,
    }
}

#[test]
fn alias_cas_over_sockets() {
    let ts = start_server("op_alias_http");
    let admin_tok = ts.admin_token();
    let mut admin_http = ts.control(Some(&admin_tok));
    let creator_tok = principal_with(&mut admin_http, &[("operation_run", "gen")]);
    let worker_tok = principal_with(
        &mut admin_http,
        &[("job_worker", "gen"), ("blob_write", "gen")],
    );
    let mut admin = real_client(&ts, &admin_tok, "admin-cache");
    let (seed_asset, seed_rev) = publish_seed_texture(&mut admin, "alias");
    let creator = real_client(&ts, &creator_tok, "creator-cache");
    let worker = real_client(&ts, &worker_tok, "worker-cache");
    assert!(worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .unwrap()
        .is_none());
    let alias = AssetAlias::from_str("gen/op-hero").unwrap();

    // First operation aliases into empty space.
    let mut request = OperationCreateRequest::new(
        "gen",
        OP_KIND,
        "alias-a",
        vec![image_input(seed_asset, seed_rev)],
    );
    request.publication = OperationPublicationRef::PublishAndAlias {
        alias: alias.clone(),
        expect: OperationAliasExpect::Absent,
    };
    let op_a = creator.operation_create(&request).expect("create").operation;
    let claimed = worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .unwrap()
        .expect("claim");
    let facts = finalize_request_via(&ts, &worker_tok, claimed.job, 0x31);
    let (_, rev_a) = worker.operation_finalize(&op_a, &facts).expect("finalize");
    let head = creator.resolve_alias(&alias).expect("alias resolves");
    assert_eq!(head.head_revision, rev_a);

    // Second operation with the same Absent expectation: the CAS refuses and
    // NOTHING publishes — the operation stays live for a corrected retry.
    let mut request_b = request.clone();
    request_b.idempotency_key = "alias-b".into();
    let op_b = creator.operation_create(&request_b).expect("create").operation;
    let claimed = worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .unwrap()
        .expect("claim");
    let facts_b = finalize_request_via(&ts, &worker_tok, claimed.job, 0x32);
    match worker.operation_finalize(&op_b, &facts_b) {
        Err(ClientError::Server { status: 409, .. }) => {}
        other => panic!("expected alias CAS conflict, got {other:?}"),
    }
    let status_b = creator.operation_get(&op_b).expect("get");
    assert_eq!(status_b.state, OperationStateDto::Running);
    assert!(status_b.result.is_none());
    assert_eq!(
        creator.resolve_alias(&alias).expect("alias").head_revision,
        rev_a,
        "alias must not move on a failed CAS"
    );

    // Third operation compare-and-sets against the CURRENT head: succeeds.
    let mut request_c = request.clone();
    request_c.idempotency_key = "alias-c".into();
    request_c.publication = OperationPublicationRef::PublishAndAlias {
        alias: alias.clone(),
        expect: OperationAliasExpect::Head(rev_a),
    };
    let op_c = creator.operation_create(&request_c).expect("create").operation;
    let claimed = worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .unwrap()
        .expect("claim");
    let facts_c = finalize_request_via(&ts, &worker_tok, claimed.job, 0x33);
    let (_, rev_c) = worker.operation_finalize(&op_c, &facts_c).expect("finalize");
    assert_eq!(
        creator.resolve_alias(&alias).expect("alias").head_revision,
        rev_c
    );
}

#[test]
fn liveness_cannot_be_forged_without_a_worker_grant() {
    let ts = start_server("op_liveness_forge");
    let admin_tok = ts.admin_token();
    let mut admin_http = ts.control(Some(&admin_tok));
    let creator_tok = principal_with(&mut admin_http, &[("operation_run", "gen")]);
    let no_grant_tok = principal_with(&mut admin_http, &[]);
    let real_worker_tok = principal_with(&mut admin_http, &[("job_worker", "gen")]);
    let creator = real_client(&ts, &creator_tok, "creator-cache");
    let no_grant = real_client(&ts, &no_grant_tok, "nogrant-cache");
    let real_worker = real_client(&ts, &real_worker_tok, "worker-cache");

    // A principal with NO job_worker grant polls the executor kind: on a
    // fresh server the namespace claim gate is vacuous, so the poll itself
    // succeeds — but it must NOT count as liveness.
    assert!(no_grant
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .expect("poll allowed")
        .is_none());
    let types = creator.operation_types().expect("types");
    assert!(
        !types.iter().find(|t| t.kind == OP_KIND).unwrap().available,
        "a grant-less poll must not forge operation availability"
    );

    // A real worker principal flips it.
    assert!(real_worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .unwrap()
        .is_none());
    assert!(creator.operation_types().unwrap().iter().find(|t| t.kind == OP_KIND).unwrap().available);
}

#[test]
fn operation_events_long_poll_answers_and_times_out_promptly() {
    let ts = start_server("op_events_wait");
    let admin_tok = ts.admin_token();
    let mut admin_http = ts.control(Some(&admin_tok));
    let creator_tok = principal_with(&mut admin_http, &[("operation_run", "gen")]);
    let worker_tok = principal_with(&mut admin_http, &[("job_worker", "gen")]);
    let mut admin = real_client(&ts, &admin_tok, "admin-cache");
    let (seed_asset, seed_rev) = publish_seed_texture(&mut admin, "wait");
    let creator = real_client(&ts, &creator_tok, "creator-cache");
    let worker = real_client(&ts, &worker_tok, "worker-cache");
    assert!(worker
        .worker_claim_kinds(60_000, Some("w1"), &[EXEC_KIND])
        .unwrap()
        .is_none());
    let op = creator
        .operation_create(&OperationCreateRequest::new(
            "gen",
            OP_KIND,
            "wait-key",
            vec![image_input(seed_asset, seed_rev)],
        ))
        .expect("create")
        .operation;

    // Events already exist: a long wait answers immediately.
    let t0 = std::time::Instant::now();
    let page = creator.operation_events(&op, 0, 5_000, 64).expect("events");
    assert!(!page.events.is_empty());
    assert!(
        t0.elapsed() < std::time::Duration::from_millis(2_000),
        "existing events must answer without waiting out the poll"
    );

    // Nothing new past the cursor: the wait times out empty, near the
    // requested bound, without erroring.
    let t1 = std::time::Instant::now();
    let page = creator
        .operation_events(&op, page.cursor, 1_200, 64)
        .expect("events wait");
    assert!(page.events.is_empty());
    let waited = t1.elapsed();
    assert!(
        waited >= std::time::Duration::from_millis(1_000),
        "empty wait must hold near the bound, waited {waited:?}"
    );
}

/// Hex sugar for digest comparison in assertions.
mod hex {
    pub fn to_hex_string(d: [u8; 32]) -> String {
        let mut out = String::with_capacity(64);
        for b in d {
            out.push_str(&format!("{b:02x}"));
        }
        out
    }
}
