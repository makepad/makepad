//! Alias-scoped publication: atomic conflicts, current-target retry, and
//! unchanged legacy rebinding. All stores and sockets are test-owned.
mod common;
#[path = "http/common/mod.rs"]
mod http_common;

use common::{asset_id_n, prop_manifest, NOW};
use makepad_asset_client::{
    Api, ApiEndpoints, AssetClient, ClientConfig, ClientError, HttpLimits,
    PublishBundle, PublishExpectedHead as ClientHead,
    PublishRights, PublishStage, PublishThumbnail,
};
use makepad_asset_data::{
    AssetKind, AssetRevisionId, AssetRevisionRef, BlobId,
    FileRole, MediaType, ThumbnailMedia,
};
use makepad_asset_store::{
    json::{obj, s, Value}, AssetAnnotation, Budgets, CatalogCore, PublishBatchItem,
    PublishExpectedHead as Head, ServerError, Visibility,
};
use std::sync::{Arc, Barrier};

fn annotation(title: &str) -> AssetAnnotation {
    AssetAnnotation {
        title: title.into(), description: title.into(), kind: Some(AssetKind::Prop),
        categories: vec![], tags: vec![], creator: String::new(), artist: String::new(),
        artist_url: String::new(), album: String::new(), source_url: String::new(),
        license: String::new(), license_url: String::new(), owner: None,
        generator: String::new(), backend: String::new(), model: String::new(),
        prompt: String::new(), provenance: String::new(), visibility: Visibility::Public,
    }
}

fn item(core: &CatalogCore, id: u8, version: u8, alias: &str) -> PublishBatchItem {
    let glb = [version; 20];
    let thumb = [9; 40];
    for bytes in [glb.as_slice(), thumb.as_slice()] {
        core.catalog().record_blob(&BlobId::hash_of(bytes), bytes.len() as u64, NOW).unwrap();
    }
    PublishBatchItem {
        namespace: "rik2".into(),
        manifest_bytes: prop_manifest(asset_id_n(id), &glb, &thumb).to_canonical_bytes().unwrap(),
        annotation: annotation(&format!("version {version}")),
        alias: Some(alias.parse().unwrap()),
    }
}

fn target(item: &PublishBatchItem, id: u8) -> AssetRevisionRef {
    AssetRevisionRef {
        asset_id: asset_id_n(id), revision: AssetRevisionId::hash_of(&item.manifest_bytes),
    }
}

fn snapshot(core: &CatalogCore, ids: &[u8]) -> (u64, Vec<String>) {
    (
        core.search().generation().unwrap(),
        ids.iter().map(|id| format!("{:?}", core.detail(&asset_id_n(*id)).unwrap())).collect(),
    )
}

#[test]
fn stale_page_and_current_target_replay_do_not_mutate_catalog_or_search() {
    let core = CatalogCore::open_memory(Budgets::default_v1()).unwrap();
    let first = item(&core, 1, 1, "rik2/model");
    let base = target(&first, 1);
    core.publish_batch_guarded(&[first.clone()], &[Head::Absent], NOW).unwrap();
    let before = snapshot(&core, &[1]);
    // A lost success response is recovered without rewriting even metadata.
    let mut retry = first.clone();
    retry.annotation.title = "must not replace original metadata".into();
    let outcome = core.publish_batch_guarded(&[retry], &[Head::Absent], NOW + 1).unwrap();
    assert!(outcome[0].unchanged && outcome[0].already_published);
    assert_eq!(snapshot(&core, &[1]), before);

    let next = item(&core, 1, 2, "rik2/model");
    core.publish_batch_guarded(&[next.clone()], &[Head::Exact(base)], NOW + 2).unwrap();
    let next_target = target(&next, 1);
    let losing = item(&core, 1, 3, "rik2/model");
    let fresh = item(&core, 2, 4, "rik2/other");
    let before = snapshot(&core, &[1, 2]);
    let error = core.publish_batch_guarded(
        &[fresh.clone(), losing.clone()], &[Head::Absent, Head::Exact(base)], NOW + 3,
    ).unwrap_err();
    assert!(matches!(error, ServerError::Conflict { .. }));
    assert_eq!(snapshot(&core, &[1, 2]), before);
    assert!(core.catalog().asset_revision_manifest(&target(&losing, 1).revision).unwrap().is_none());
    assert!(core.catalog().asset_revision_manifest(&target(&fresh, 2).revision).unwrap().is_none());

    // Old successful requests cannot roll the alias back after a successor.
    assert!(core.publish_batch_guarded(&[first], &[Head::Absent], NOW + 4).is_err());
    assert_eq!(snapshot(&core, &[1, 2]), before);
    let retry = core.publish_batch_guarded(&[next], &[Head::Exact(base)], NOW + 5).unwrap();
    assert!(retry[0].unchanged);
    assert_eq!(snapshot(&core, &[1, 2]), before);
    // The rejected candidate can be explicitly retried against the observed head.
    core.publish_batch_guarded(&[losing], &[Head::Exact(next_target)], NOW + 6).unwrap();
}

#[test]
fn absent_covers_unaliased_and_previously_published_identity_and_forbids_rebinding() {
    let core = CatalogCore::open_memory(Budgets::default_v1()).unwrap();
    core.catalog().register_asset(&asset_id_n(1), "rik2", NOW).unwrap();
    let first = item(&core, 1, 1, "rik2/model");
    core.publish_batch_guarded(&[first.clone()], &[Head::Absent], NOW).unwrap();
    let other_id = item(&core, 2, 2, "rik2/model");
    let before = snapshot(&core, &[1, 2]);
    assert!(matches!(core.publish_batch_guarded(
        &[other_id.clone()], &[Head::Exact(target(&first, 1))], NOW + 1,
    ), Err(ServerError::Conflict { what: "publish expected asset identity" })));
    assert_eq!(snapshot(&core, &[1, 2]), before);
    core.catalog().clear_asset_alias(first.alias.as_ref().unwrap()).unwrap();
    let before = snapshot(&core, &[1]);
    assert!(core.publish_batch_guarded(&[first.clone()], &[Head::Absent], NOW + 2).is_err());
    assert_eq!(snapshot(&core, &[1]), before);
    core.catalog().quarantine_asset(&asset_id_n(1), &target(&first, 1).revision, NOW + 3).unwrap();
    let newer = item(&core, 1, 3, "rik2/new_alias");
    assert!(core.publish_batch_guarded(&[newer], &[Head::Absent], NOW + 4).is_err());

    // Legacy rebinding and annotation refresh still work unchanged.
    core.publish_batch(&[other_id.clone()], NOW + 5).unwrap();
    let rebind = item(&core, 3, 4, "rik2/model");
    core.publish_batch(&[rebind.clone()], NOW + 6).unwrap();
    assert_eq!(core.catalog().resolve_asset_alias(rebind.alias.as_ref().unwrap()).unwrap(), Some(target(&rebind, 3)));
    let mut update = rebind;
    update.annotation.title = "legacy metadata update".into();
    assert!(!core.publish_batch(&[update], NOW + 7).unwrap()[0].unchanged);
    assert_eq!(core.search().annotation(&asset_id_n(3)).unwrap().unwrap().title, "legacy metadata update");
}

#[test]
fn guarded_page_rollback_also_covers_failure_after_an_earlier_item_was_written() {
    let core = CatalogCore::open_memory(Budgets::default_v1()).unwrap();
    let first = item(&core, 1, 1, "rik2/a");
    let second = item(&core, 2, 2, "rik2/b");
    let mut bad = second.clone();
    let mut manifest = makepad_asset_data::AssetManifest::from_canonical_bytes(&bad.manifest_bytes).unwrap();
    manifest.files[0].blob = BlobId::hash_of(b"not admitted");
    bad.manifest_bytes = manifest.to_canonical_bytes().unwrap();
    let before = snapshot(&core, &[1, 2]);
    assert!(core.publish_batch_guarded(&[first.clone(), bad], &[Head::Absent, Head::Absent], NOW).is_err());
    assert_eq!(snapshot(&core, &[1, 2]), before);
    core.publish_batch_guarded(&[first.clone(), second], &[Head::Absent, Head::Absent], NOW + 1).unwrap();
    // Conflicting multiple writes to one alias or identity in a page are malformed.
    let duplicate = item(&core, 3, 3, "rik2/a");
    assert!(matches!(core.publish_batch_guarded(&[first.clone(), duplicate], &[Head::Any, Head::Any], NOW + 2),
        Err(ServerError::InvalidInput { .. })));
    assert!(core.publish_batch_guarded(&[first], &[], NOW + 3).is_err());
}

fn connect(server: &http_common::TestServer, cache: &str) -> AssetClient {
    let mut cfg = ClientConfig::new(http_common::test_root(cache));
    cfg.token = Some(server.admin_token());
    AssetClient::connect(cfg, endpoints(server), Some(server.server.server_id())).unwrap()
}

fn endpoints(server: &http_common::TestServer) -> ApiEndpoints {
    ApiEndpoints { control: server.server.control_addr(), data: server.server.data_addr() }
}

fn api(server: &http_common::TestServer) -> Api {
    Api::new(endpoints(server), HttpLimits::default_v1(), Some(server.admin_token())).unwrap()
}

fn bundle(id: u8, version: u8) -> PublishBundle {
    let mut bundle = PublishBundle::editable_model(
        "rik2", format!("model {version}"), vec![version + 1; 40], vec![version; 20],
        PublishThumbnail::plain(vec![9; 40], ThumbnailMedia::Png, 512, 512),
        PublishRights::generated_cc0(),
    );
    bundle.asset_id = Some(asset_id_n(id));
    bundle.stats.vertices = 8;
    bundle.stats.triangles = 12;
    bundle.alias = Some("rik2/model".parse().unwrap());
    bundle.expected_head = ClientHead::Absent;
    bundle
}

fn assert_conflict<T: std::fmt::Debug>(result: Result<T, ClientError>) {
    assert!(matches!(result, Err(ClientError::Server { status: 409, .. })), "{result:?}");
}

#[test]
fn two_socket_clients_race_from_one_base_and_only_one_writer_publishes() {
    let mut server = http_common::start_server_with("alias_cas_race", |cfg| cfg.gc_janitor_steps = 0);
    let mut client_a = connect(&server, "cas_a");
    let mut client_b = connect(&server, "cas_b");
    let first = client_a.publish_bundle(&bundle(1, 1)).unwrap();
    let expected = ClientHead::Exact(AssetRevisionRef { asset_id: first.asset_id, revision: first.revision });
    let mut a = bundle(1, 2);
    a.expected_head = expected;
    let mut b = bundle(1, 3);
    b.expected_head = expected;
    let barrier = Arc::new(Barrier::new(2));
    let (ra, rb) = std::thread::scope(|scope| {
        let ready = barrier.clone();
        let request_a = &a;
        let writer_a = &mut client_a;
        let task_a = scope.spawn(move || {
            let mut progress = |stage: &PublishStage| { if matches!(stage, PublishStage::Publishing) { ready.wait(); } };
            writer_a.publish_bundle_with(request_a, Some(&mut progress), &|| false)
        });
        let task_b = scope.spawn(|| {
            let mut progress = |stage: &PublishStage| { if matches!(stage, PublishStage::Publishing) { barrier.wait(); } };
            client_b.publish_bundle_with(&b, Some(&mut progress), &|| false)
        });
        (task_a.join().unwrap(), task_b.join().unwrap())
    });
    let (winner, winner_bundle, loser_bundle) = match (ra, rb) {
        (Ok(winner), rejected) => { assert_conflict(rejected); (winner, a, b) }
        (rejected, Ok(winner)) => { assert_conflict(rejected); (winner, b, a) }
        other => panic!("expected one winning writer: {other:?}"),
    };
    let api = api(&server);
    assert_eq!(api.asset_detail(&first.asset_id).unwrap().candidates.len(), 2);
    let head = api.resolve_alias(winner.alias.as_ref().unwrap()).unwrap();
    assert_eq!(head.head_revision, winner.revision);
    assert_eq!(api.get_annotation(&winner.asset_id).unwrap().title, winner_bundle.title);
    let cursor = api.events_page(None, 0, 1, None).unwrap().cursor;
    // Simulate lost-success recovery by discarding and repeating the same request.
    assert_eq!(client_a.publish_bundle(&winner_bundle).unwrap().revision, winner.revision);
    assert!(api.events_page(Some(&cursor), 0, 100, None).unwrap().events.is_empty());
    assert_conflict(client_b.publish_bundle(&loser_bundle));

    let mut successor = bundle(1, 4);
    successor.expected_head = ClientHead::Exact(AssetRevisionRef { asset_id: winner.asset_id, revision: winner.revision });
    let next = client_b.publish_bundle(&successor).unwrap();
    assert_conflict(client_a.publish_bundle(&winner_bundle));
    assert_eq!(api.resolve_alias(next.alias.as_ref().unwrap()).unwrap().head_revision, next.revision);
    // An independent client receives every immutable source/runtime blob.
    let manifest = client_a.fetch_asset_manifest(&next.revision).unwrap();
    assert_eq!(manifest.files.len(), 2);
    for file in &manifest.files {
        let bytes = client_a.fetch_blob_bytes(&file.blob, Some(file.byte_len)).unwrap();
        assert_eq!(BlobId::hash_of(&bytes), file.blob);
        let original = successor.files.iter().find(|original| original.role == file.role).unwrap();
        assert_eq!(bytes, original.bytes);
    }
    assert!(manifest.files.iter().any(|file| file.role == FileRole::Source && file.media == MediaType::Bin));
    server.server.shutdown();
    // Reopen the durable store and use a fresh cache, so no source or runtime
    // bytes can be satisfied from the publishing clients' memory or cache.
    let root = server.root.clone();
    drop(server);
    let mut config = http_common::base_config(root.clone());
    config.gc_janitor_steps = 0;
    let mut reopened_server = http_common::TestServer {
        server: makepad_asset_store::AssetServer::start(config).unwrap(),
        root,
    };
    let mut late = connect(&reopened_server, "cas_late_reopen");
    let reopened = late.fetch_asset_manifest(&next.revision).unwrap();
    for file in reopened.files {
        let bytes = late.fetch_blob_bytes(&file.blob, Some(file.byte_len)).unwrap();
        assert_eq!(bytes, successor.files.iter().find(|original| original.role == file.role).unwrap().bytes);
    }
    reopened_server.server.shutdown();
}

#[test]
fn expect_absent_race_owns_one_identity_and_invalid_client_inputs_send_nothing() {
    let mut server = http_common::start_server_with("alias_cas_absent", |cfg| cfg.gc_janitor_steps = 0);
    let mut a = connect(&server, "absent_a");
    let mut b = connect(&server, "absent_b");
    let barrier = Barrier::new(2);
    let (ra, rb) = std::thread::scope(|scope| {
        let task_a = scope.spawn(|| {
            let mut progress = |stage: &PublishStage| { if matches!(stage, PublishStage::Publishing) { barrier.wait(); } };
            a.publish_bundle_with(&bundle(1, 1), Some(&mut progress), &|| false)
        });
        let task_b = scope.spawn(|| {
            let mut progress = |stage: &PublishStage| { if matches!(stage, PublishStage::Publishing) { barrier.wait(); } };
            b.publish_bundle_with(&bundle(2, 2), Some(&mut progress), &|| false)
        });
        (task_a.join().unwrap(), task_b.join().unwrap())
    });
    let (first, loser) = match (ra, rb) {
        (Ok(first), rejected) => { assert_conflict(rejected); (first, 2) },
        (rejected, Ok(first)) => { assert_conflict(rejected); (first, 1) },
        other => panic!("expected one absent-head winner: {other:?}"),
    };
    assert!(matches!(api(&server).asset_detail(&asset_id_n(loser)), Err(ClientError::NotFound { .. })));
    let counts = (server.server.control_requests_served(), server.server.data_requests_served());
    let mut missing_id = bundle(3, 3);
    missing_id.asset_id = None;
    assert!(matches!(b.publish_bundle(&missing_id), Err(ClientError::InvalidInput { .. })));
    let mut missing_alias = bundle(3, 3);
    missing_alias.alias = None;
    assert!(matches!(b.publish_bundle(&missing_alias), Err(ClientError::InvalidInput { .. })));
    let mut mismatched_id = bundle(3, 3);
    mismatched_id.expected_head = ClientHead::Exact(AssetRevisionRef { asset_id: first.asset_id, revision: first.revision });
    assert!(matches!(b.publish_bundle(&mismatched_id), Err(ClientError::InvalidInput { .. })));
    assert_eq!(counts, (server.server.control_requests_served(), server.server.data_requests_served()));
    server.server.shutdown();
}

#[test]
fn cancellation_from_publishing_callback_prevents_guarded_and_legacy_batch_commit() {
    let mut server = http_common::start_server_with("alias_cas_cancel", |cfg| cfg.gc_janitor_steps = 0);
    let mut client = connect(&server, "cancel");
    for (id, guard, batch) in [(1, ClientHead::Absent, false), (2, ClientHead::Absent, true), (3, ClientHead::Any, true)] {
        let cancelled = std::cell::Cell::new(false);
        let mut progress = |stage: &PublishStage| { if matches!(stage, PublishStage::Publishing) { cancelled.set(true); } };
        let mut request = bundle(id, id);
        request.expected_head = guard;
        let result = if batch {
            client.publish_bundles_with(&[request], Some(&mut progress), &|| cancelled.get()).map(|_| ())
        } else {
            client.publish_bundle_with(&request, Some(&mut progress), &|| cancelled.get()).map(|_| ())
        };
        assert!(matches!(result, Err(ClientError::Cancelled)), "{result:?}");
        assert!(matches!(api(&server).asset_detail(&asset_id_n(id)), Err(ClientError::NotFound { .. })));
    }
    server.server.shutdown();
}

#[test]
fn malformed_unknown_or_misrouted_guards_fail_closed_over_http() {
    let mut server = http_common::start_server("alias_cas_wire");
    let token = server.admin_token();
    let mut client = server.control(Some(&token));
    let absent = obj(vec![("state", s("absent"))]);
    let guards = [
        None, Some(Value::Null), Some(s("absent")),
        Some(obj(vec![("state", s("future"))])),
        Some(obj(vec![("state", s("absent")), ("revision", s("unexpected"))])),
        Some(obj(vec![("state", s("exact"))])),
        Some(obj(vec![("state", s("exact")), ("asset_id", s(asset_id_n(1).to_string())), ("revision", s("bad"))])),
    ];
    for guard in guards {
        let mut fields = vec![];
        if let Some(guard) = guard { fields.push(("expected_head", guard)); }
        let body = obj(vec![("items", Value::Arr(vec![obj(fields)]))]);
        assert_eq!(client.post_json("/v1/publish/batch/guarded", &body).status, 400);
    }
    let body = obj(vec![("items", Value::Arr(vec![obj(vec![("expected_head", absent)])]))]);
    assert_eq!(client.post_json("/v1/publish/batch", &body).status, 400);
    server.server.shutdown();
}

#[test]
fn an_older_server_cannot_silently_drop_the_guard() {
    use makepad_asset_client::api::{AnnotationUpload, PublishBatchWireItem};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let item = PublishBatchWireItem {
        namespace: "rik2".into(), manifest: vec![1], alias: Some("rik2/model".parse().unwrap()),
        annotation: AnnotationUpload { title: "test".into(), ..Default::default() },
    };
    let mut limits = HttpLimits::default_v1();
    limits.read_timeout_ms = 1_000;
    let api = Api::new(ApiEndpoints { control: addr, data: addr }, limits, None).unwrap();
    std::thread::scope(|scope| {
        let stub = scope.spawn(|| {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0; 1024];
            let body_start = loop {
                let n = socket.read(&mut chunk).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                assert!(bytes.len() < 64 * 1024);
                if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") { break pos + 4; }
            };
            let headers = std::str::from_utf8(&bytes[..body_start]).unwrap();
            assert!(headers.starts_with("POST /v1/publish/batch/guarded HTTP/1.1\r\n"));
            let len: usize = headers.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
            }).unwrap();
            while bytes.len() < body_start + len {
                let n = socket.read(&mut chunk).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            let body = makepad_asset_store::json::parse(&bytes[body_start..]).unwrap();
            assert_eq!(body.get("items").unwrap().as_arr().unwrap()[0]
                .get("expected_head").unwrap().get("state").unwrap().as_str(), Some("absent"));
            let reply = b"{\"error\":\"not found\"}";
            write!(socket, "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n", reply.len()).unwrap();
            socket.write_all(reply).unwrap();
        });
        assert!(matches!(api.publish_batch_guarded(&[item], &[ClientHead::Absent]), Err(ClientError::NotFound { .. })));
        stub.join().unwrap();
    });
    listener.set_nonblocking(true).unwrap();
    assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}

#[test]
fn a_committed_request_with_its_response_discarded_retries_without_side_effects() {
    use makepad_asset_client::api::{AnnotationUpload, PublishBatchWireItem};
    use std::io::Write;
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    let mut server = http_common::start_server_with("alias_cas_lost_reply", |cfg| cfg.gc_janitor_steps = 0);
    let api = api(&server);
    let glb = [3; 20];
    let thumbnail = [7; 40];
    api.upload_blob("rik2", &glb).unwrap();
    api.upload_blob("rik2", &thumbnail).unwrap();
    let manifest = prop_manifest(asset_id_n(7), &glb, &thumbnail);
    let canonical = manifest.to_canonical_bytes().unwrap();
    let alias = "rik2/lost_reply".parse().unwrap();
    let hex: String = canonical.iter().map(|byte| format!("{byte:02x}")).collect();
    let body = obj(vec![("items", Value::Arr(vec![obj(vec![
        ("namespace", s("rik2")), ("manifest", s(hex)), ("alias", s("rik2/lost_reply")),
        ("annotation", obj(vec![("title", s("lost reply")), ("kind", s("prop"))])),
        ("expected_head", obj(vec![("state", s("absent"))])),
    ])]))]).to_json();
    let mut socket = TcpStream::connect(server.server.control_addr()).unwrap();
    write!(socket, "POST /v1/publish/batch/guarded HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", server.admin_token(), body.len(), body).unwrap();
    // Close without reading any response: the client cannot know the commit result.
    drop(socket);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if api.resolve_alias(&alias).is_ok() { break; }
        assert!(Instant::now() < deadline, "discarded-response request did not commit");
        std::thread::sleep(Duration::from_millis(10));
    }
    let cursor = api.events_page(None, 0, 1, None).unwrap().cursor;
    let wire_item = PublishBatchWireItem {
        namespace: "rik2".into(), manifest: canonical, alias: Some(alias.clone()),
        annotation: AnnotationUpload { title: "lost reply".into(), kind: Some(AssetKind::Prop), ..Default::default() },
    };
    let retried = api.publish_batch_guarded(&[wire_item], &[ClientHead::Absent]).unwrap();
    assert_eq!(retried[0], (asset_id_n(7), manifest.revision().unwrap(), true));
    assert_eq!(api.asset_detail(&asset_id_n(7)).unwrap().candidates.len(), 1);
    assert!(api.events_page(Some(&cursor), 0, 100, None).unwrap().events.is_empty());
    assert_eq!(api.resolve_alias(&alias).unwrap().head_revision, manifest.revision().unwrap());
    server.server.shutdown();
}

#[test]
fn rejected_http_page_preserves_browse_index_events_annotation_and_alias() {
    use makepad_asset_client::api::{AnnotationUpload, PublishBatchWireItem};
    let mut server = http_common::start_server_with("alias_cas_http_page", |cfg| cfg.gc_janitor_steps = 0);
    let mut client = connect(&server, "http_page");
    let initial = client.publish_bundle(&bundle(1, 1)).unwrap();
    let expected = ClientHead::Exact(AssetRevisionRef { asset_id: initial.asset_id, revision: initial.revision });
    let mut next = bundle(1, 2);
    next.expected_head = expected;
    let published = client.publish_bundle(&next).unwrap();
    let mut stale = bundle(1, 3);
    stale.expected_head = expected;
    let mut fresh = bundle(2, 4);
    fresh.alias = Some("rik2/new_model".parse().unwrap());
    let api = api(&server);
    let index = api.assets_page(Some("rik2"), None, 100).unwrap();
    let detail = api.asset_detail(&initial.asset_id).unwrap();
    let annotation = api.get_annotation(&initial.asset_id).unwrap();
    let cursor = api.events_page(None, 0, 1, None).unwrap().cursor;
    assert_conflict(client.publish_bundles(&[fresh, stale]));
    // Bypass the convenience client's local identity check: the wire endpoint
    // must enforce identity ownership itself before any catalog writes.
    let mismatched = PublishBatchWireItem {
        namespace: "rik2".into(),
        manifest: prop_manifest(asset_id_n(3), &[3; 20], &[7; 40]).to_canonical_bytes().unwrap(),
        alias: Some("rik2/model".parse().unwrap()),
        annotation: AnnotationUpload { title: "rebind attempt".into(), ..Default::default() },
    };
    assert_conflict(api.publish_batch_guarded(&[mismatched], &[expected]));
    assert_eq!(api.assets_page(Some("rik2"), None, 100).unwrap(), index);
    assert_eq!(api.asset_detail(&initial.asset_id).unwrap(), detail);
    assert_eq!(api.get_annotation(&initial.asset_id).unwrap(), annotation);
    assert!(api.events_page(Some(&cursor), 0, 100, None).unwrap().events.is_empty());
    assert_eq!(api.resolve_alias(published.alias.as_ref().unwrap()).unwrap().head_revision, published.revision);
    assert!(matches!(api.asset_detail(&asset_id_n(2)), Err(ClientError::NotFound { .. })));
    assert!(matches!(api.asset_detail(&asset_id_n(3)), Err(ClientError::NotFound { .. })));
    server.server.shutdown();
}
