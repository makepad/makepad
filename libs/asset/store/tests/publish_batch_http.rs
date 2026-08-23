//! Batch publication over real sockets: N complete bundles land in TWO
//! round trips (one bulk blob upload, one batch publish), one catalog
//! transaction each — and everything the split flow guarantees still holds:
//! digest-verified identities, alias heads, annotations, catalog events,
//! idempotent replay, and the rights-immutability guard.

use makepad_asset_client::{
    Api, ApiEndpoints, AssetClient, CatalogEventKind, ClientConfig, HttpLimits, PublishBundle,
    PublishBundleFile, PublishRights, PublishThumbnail,
};
use makepad_asset_data::{
    AssetKind, BlobId, DeviceTier, FileRole, MediaType, ThumbnailMedia,
};
use makepad_asset_store::{AssetServer, ServerConfig};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn test_root(name: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("mp_publish_batch_{}_{}_{}", std::process::id(), n, name))
}

fn start_server(name: &str) -> (AssetServer, String) {
    let root = test_root(name);
    let mut cfg = ServerConfig::new(root.clone());
    cfg.control_addr = "127.0.0.1:0".parse().unwrap();
    cfg.data_addr = "127.0.0.1:0".parse().unwrap();
    cfg.bootstrap_admin = true;
    cfg.log = false;
    cfg.gc_janitor_steps = 0;
    let server = AssetServer::start(cfg).expect("server start");
    let token = std::fs::read_to_string(root.join("admin-token"))
        .expect("admin token")
        .trim()
        .to_string();
    (server, token)
}

fn connect(server: &AssetServer, token: &str, cache: &str) -> AssetClient {
    let mut cfg = ClientConfig::new(test_root(cache));
    cfg.token = Some(token.to_string());
    let endpoints = ApiEndpoints { control: server.control_addr(), data: server.data_addr() };
    AssetClient::connect(cfg, endpoints, Some(server.server_id())).expect("connect")
}

fn api(server: &AssetServer, token: &str) -> Api {
    Api::new(
        ApiEndpoints { control: server.control_addr(), data: server.data_addr() },
        HttpLimits::default_v1(),
        Some(token.to_string()),
    )
    .expect("api")
}

/// A preset-shaped bundle: one Source text file + a JPEG-tagged thumbnail.
fn bundle(i: usize) -> PublishBundle {
    let source = format!("// preset {i}\nname: \"preset {i}\"\nengine: \"screen\"\n");
    let thumb: Vec<u8> = (0..2048).map(|b| ((b * 31 + i * 7) % 251) as u8).collect();
    let mut b = PublishBundle::new(
        "vjfx",
        AssetKind::VjEffect,
        format!("preset {i}"),
        vec![PublishBundleFile {
            role: FileRole::Source,
            tier: DeviceTier::Any,
            lod: 0,
            media: MediaType::Text,
            bytes: source.into_bytes(),
            reference: None,
            dims: None,
        }],
        PublishThumbnail::plain(thumb, ThumbnailMedia::Jpeg, 512, 320),
        PublishRights::generated_cc0(),
    );
    b.alias = format!("vjfx/batch_{i:03}").parse().ok();
    b.description = format!("batch preset {i}");
    b.tags = vec!["vjeffect".into(), "builtin".into()];
    b.generator = "batch test".into();
    b
}

#[test]
fn a_page_of_bundles_publishes_in_two_round_trips_and_lands_whole() {
    let (mut server, token) = start_server("page");
    let mut client = connect(&server, &token, "page_cache");

    let bundles: Vec<PublishBundle> = (0..32).map(bundle).collect();
    let api = api(&server, &token);
    // Event feeds resume from a cursor; take one BEFORE publishing so the
    // page below is exactly what the batch emitted.
    let start_cursor = api.events_page(None, 0, 1, None).expect("cursor").cursor;
    let control_before = server.control_requests_served();
    let data_before = server.data_requests_served();
    let published = client.publish_bundles(&bundles).expect("batch publish");
    assert_eq!(published.len(), 32);

    // The whole page (32 bundles = 64 blobs, exactly one upload page)
    // cost TWO requests: one bulk blob upload (data plane), one batch
    // publish (control plane).
    let control_spent = server.control_requests_served() - control_before;
    let data_spent = server.data_requests_served() - data_before;
    assert!(
        control_spent <= 1 && data_spent <= 1,
        "a 32-bundle page cost {control_spent} control + {data_spent} data requests"
    );

    // Every alias resolves to exactly the published head, and the manifest
    // round-trips with the declared Source blob.
    for (i, done) in published.iter().enumerate() {
        let alias = format!("vjfx/batch_{i:03}").parse().expect("alias");
        let head = api.resolve_alias(&alias).expect("alias resolves");
        assert_eq!(head.asset_id, done.asset_id);
        assert_eq!(head.head_revision, done.revision);
        let manifest = client.fetch_asset_manifest(&done.revision).expect("manifest");
        assert_eq!(manifest.asset_id, done.asset_id);
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].role, FileRole::Source);
        assert_eq!(manifest.files[0].blob, done.files[0].blob);
        // The bytes are really there, digest-verified on the way out.
        let bytes = client
            .fetch_blob_bytes(&done.files[0].blob, Some(manifest.files[0].byte_len))
            .expect("source bytes");
        assert_eq!(BlobId::hash_of(&bytes), done.files[0].blob);
    }

    // Catalog events carried the publishes (kind-tagged from the annotation
    // that landed in the same transaction).
    let events = api.events_page(Some(&start_cursor), 0, 200, None).expect("events");
    let published_events = events
        .events
        .iter()
        .filter(|e| e.kind == CatalogEventKind::AssetPublished)
        .count();
    assert_eq!(published_events, 32, "one publish event per bundle");
    let alias_events = events
        .events
        .iter()
        .filter(|e| e.kind == CatalogEventKind::AliasSet)
        .count();
    assert_eq!(alias_events, 32, "one alias event per bundle");

    server.shutdown();
}

#[test]
fn replaying_a_landed_page_is_idempotent_and_mints_nothing() {
    let (mut server, token) = start_server("replay");
    let mut client = connect(&server, &token, "replay_cache");

    let mut bundles: Vec<PublishBundle> = (0..8).map(bundle).collect();
    let first = client.publish_bundles(&bundles).expect("first publish");
    // The replay must target the SAME asset identities (a real retry knows
    // its ids; a fresh seed pass discovers them via alias_status).
    for (b, done) in bundles.iter_mut().zip(&first) {
        b.asset_id = Some(done.asset_id);
    }
    let second = client.publish_bundles(&bundles).expect("replayed publish");
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(a.asset_id, b.asset_id, "replay minted a new asset");
        assert_eq!(a.revision, b.revision, "replay minted a new revision");
    }
    server.shutdown();
}

#[test]
fn the_batch_refuses_rights_changes_and_reference_slots() {
    let (mut server, token) = start_server("guards");
    let mut client = connect(&server, &token, "guards_cache");

    let bundles: Vec<PublishBundle> = (0..2).map(bundle).collect();
    let published = client.publish_bundles(&bundles).expect("publish");

    // Re-publishing an existing asset with DIFFERENT terms must refuse —
    // the rights-immutability law, now enforced server-side for the batch.
    let mut changed = bundle(0);
    changed.asset_id = Some(published[0].asset_id);
    changed.description = "new revision, new terms".into();
    changed.files[0].bytes = b"// changed source\nname: \"changed\"\n".to_vec();
    changed.rights = PublishRights::declared(
        "CC-BY-4.0",
        "someone",
        "https://example.com",
        makepad_asset_data::Redistribution::AttributionRequired,
        makepad_asset_data::DerivativePolicy::Allowed,
    );
    let err = client.publish_bundles(std::slice::from_ref(&changed));
    assert!(err.is_err(), "a rights change re-publication must refuse");

    // Reference slots need the split flow; the batch refuses them up front.
    let mut by_ref = bundle(1);
    by_ref.files[0].bytes = Vec::new();
    by_ref.files[0].reference = Some(PathBuf::from("/tmp/nonexistent"));
    let err = client.publish_bundles(std::slice::from_ref(&by_ref));
    assert!(err.is_err(), "reference slots must refuse in the batch lane");

    server.shutdown();
}

#[test]
fn bulk_blob_upload_dedups_and_verifies_identities() {
    let (mut server, token) = start_server("blobs");
    let _client = connect(&server, &token, "blobs_cache");
    let api = api(&server, &token);

    let blobs: Vec<Vec<u8>> = (0..10u8).map(|i| vec![i; 1000 + i as usize]).collect();
    let refs: Vec<&[u8]> = blobs.iter().map(Vec::as_slice).collect();
    let ids = api.upload_blob_batch("vjfx", &refs).expect("bulk upload");
    assert_eq!(ids.len(), 10);
    for (id, bytes) in ids.iter().zip(&blobs) {
        assert_eq!(*id, BlobId::hash_of(bytes));
    }
    // A replay dedups everything and answers the same identities.
    let again = api.upload_blob_batch("vjfx", &refs).expect("replayed upload");
    assert_eq!(ids, again);

    server.shutdown();
}
