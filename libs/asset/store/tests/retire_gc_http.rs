//! The deletion contract over real sockets: the real `makepad-asset-client`
//! against a real `AssetServer`, publishing, deleting, and collecting.
//!
//! This is the proof the Asset UI's "Delete from store" and "Collect
//! garbage" buttons stand on: after `retire_asset` the alias 404s, search
//! and the listing no longer carry the asset, detail says `retired`, a
//! subscriber is told, and a GC run reclaims exactly the bytes that
//! retirement unlinked — over HTTP, with the same DTOs the UI parses.

use makepad_asset_client::{
    Api, ApiEndpoints, AssetClient, CandidateStateDto, CatalogEventKind, CatalogQuery,
    CatalogSubscriberConfig, CatalogSubscriptionEvent, ClientConfig, ClientError, GcPhaseDto,
    GcRequest, HttpLimits, PublishFile, PublishRequest, PublishThumbnail,
};
use makepad_asset_data::{AssetAlias, AssetKind, FileRole, MediaType, ThumbnailMedia};
use makepad_asset_store::{AssetServer, ServerConfig};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn test_root(name: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("mp_asset_del_{}_{}_{}", std::process::id(), n, name))
}

fn start_server(name: &str) -> (AssetServer, String) {
    let root = test_root(name);
    let mut cfg = ServerConfig::new(root.clone());
    cfg.control_addr = "127.0.0.1:0".parse().unwrap();
    cfg.data_addr = "127.0.0.1:0".parse().unwrap();
    cfg.bootstrap_admin = true;
    cfg.log = false;
    // Tests drive GC explicitly; a background janitor advancing the same run
    // would make the step accounting non-deterministic.
    cfg.gc_janitor_steps = 0;
    cfg.gc_grace_ms = 0;
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

fn publish(client: &mut AssetClient, alias: &str, title: &str, fill: u8, len: usize)
    -> makepad_asset_client::Published
{
    publish_rev(client, alias, title, fill, len, None)
}

/// Publish a new REVISION of an existing asset when `asset_id` is given
/// (that is what makes a superseded revision to retire).
fn publish_rev(
    client: &mut AssetClient,
    alias: &str,
    title: &str,
    fill: u8,
    len: usize,
    asset_id: Option<makepad_asset_data::AssetId>,
) -> makepad_asset_client::Published {
    let artifact = vec![fill; len];
    let thumb = vec![fill ^ 0xFF; 1_024];
    let mut request = PublishRequest::new(
        "gen",
        AssetKind::Video,
        title,
        PublishFile {
            bytes: artifact,
            media: MediaType::Mp4,
            role: FileRole::Video,
            media_millis: 1_000,
            dims: None,
        },
        PublishThumbnail {
            bytes: thumb,
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
        },
    );
    request.alias = Some(AssetAlias::from_str(alias).unwrap());
    request.asset_id = asset_id;
    client.publish_artifact(&request).expect("publish")
}

/// Drive a GC run to completion through the HTTP route, exactly as a UI
/// polling the button would.
fn gc_to_completion(client: &AssetClient, request: GcRequest) -> makepad_asset_client::GcStatusDto {
    let mut status = client.gc_blobs(&request).expect("gc start");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !status.done {
        assert!(Instant::now() < deadline, "gc did not converge: {status:?}");
        status = client.gc_blobs(&request).expect("gc advance");
    }
    status
}

#[test]
fn delete_from_store_then_collect_garbage_over_http() {
    let (mut server, token) = start_server("delete_e2e");
    let mut client = connect(&server, &token, "delete_e2e_cache");

    let mut sub_cfg = CatalogSubscriberConfig::default_v1();
    sub_cfg.wait_ms = 1_000;
    let mut subscriber = client.subscribe_catalog(sub_cfg).expect("subscribe");

    let keep = publish(&mut client, "gen/keeper", "Keeper clip", 0x11, 4_000);
    let drop = publish(&mut client, "gen/doomed", "Doomed clip", 0x22, 6_000);

    // Both are visible: listing, search, alias.
    let page = client.assets_page(Some("gen"), None, 10).expect("listing");
    assert_eq!(page.assets.len(), 2);
    let hits = client
        .catalog_search(&CatalogQuery::browse(10), None)
        .expect("browse");
    assert_eq!(hits.hits.len(), 2);
    let doomed_alias = AssetAlias::from_str("gen/doomed").unwrap();
    assert_eq!(
        client.resolve_alias(&doomed_alias).expect("alias").asset_id,
        drop.asset_id
    );

    // ---- delete ----------------------------------------------------------
    let report = client.retire_asset(&drop.asset_id).expect("retire");
    assert!(!report.already_retired);
    assert_eq!(report.revisions_retired, 1);
    assert_eq!(report.aliases_dropped, 1);
    assert!(report.annotation_cleared);
    // Idempotent over the wire too.
    assert!(client.retire_asset(&drop.asset_id).expect("retire again").already_retired);

    // The alias is gone (indistinguishable from never existing).
    assert!(matches!(
        client.resolve_alias(&doomed_alias),
        Err(ClientError::NotFound { .. })
    ));
    // Search and listing carry only the survivor.
    let hits = client
        .catalog_search(&CatalogQuery::browse(10), None)
        .expect("browse after delete");
    assert_eq!(hits.hits.len(), 1);
    assert_eq!(hits.hits[0].asset_id, keep.asset_id);
    let page = client.assets_page(Some("gen"), None, 10).expect("listing after delete");
    assert_eq!(page.assets.len(), 1);
    assert_eq!(page.assets[0].asset_id, keep.asset_id);
    // Detail still answers, and says deleted.
    let detail = client.asset_detail(&drop.asset_id).expect("detail after delete");
    assert!(detail.retired);
    assert!(detail.retired_ms.is_some());
    assert_eq!(detail.candidates.len(), 1);
    assert_eq!(detail.candidates[0].state, CandidateStateDto::Retired);
    assert!(detail.candidates[0].retired_ms.is_some());
    assert!(detail.latest_published().is_none());
    // The manifest of a deleted revision reads exactly like an absent one.
    assert!(matches!(
        client.fetch_asset_manifest(&drop.revision),
        Err(ClientError::NotFound { .. })
    ));

    // The subscriber is told, with the retirement kind (this client asks for
    // the vocabulary that has one).
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut saw_retire = false;
    while Instant::now() < deadline && !saw_retire {
        for event in subscriber.poll() {
            if let CatalogSubscriptionEvent::Events { events, .. } = event {
                for ev in events {
                    if ev.asset_id == Some(drop.asset_id)
                        && ev.kind == CatalogEventKind::AssetRetired
                    {
                        assert!(ev.kind.removes_content());
                        saw_retire = true;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(saw_retire, "asset_retired must reach the subscriber");
    subscriber.shutdown();

    // ---- collect garbage -------------------------------------------------
    // Dry run first: the exact bytes the real run will free.
    let dry = gc_to_completion(&client, GcRequest::dry_run());
    assert!(dry.dry_run);
    assert_eq!(dry.phase, GcPhaseDto::Done);
    assert_eq!(dry.unreferenced_blobs, 2, "the deleted asset's file + thumbnail");
    assert_eq!(dry.unreferenced_bytes, 6_000 + 1_024);
    assert_eq!(dry.deleted_blobs, 0);
    // Nothing was deleted by the preview.
    assert!(client.blob_head(&drop.artifact_blob).is_ok());

    let real = gc_to_completion(&client, GcRequest::collect());
    assert!(!real.dry_run);
    assert_eq!(real.deleted_blobs, dry.unreferenced_blobs);
    assert_eq!(real.deleted_bytes, dry.unreferenced_bytes);

    // The deleted asset's bytes are unreachable; the survivor's are intact.
    assert!(matches!(
        client.blob_head(&drop.artifact_blob),
        Err(ClientError::NotFound { .. })
    ));
    assert_eq!(
        client
            .fetch_blob_bytes(&keep.artifact_blob, Some(4_000))
            .expect("survivor bytes")
            .len(),
        4_000
    );

    // Status without starting a run reports the finished one; a further
    // collection finds nothing.
    let status = client.gc_status().expect("gc status");
    assert!(status.done);
    assert_eq!(status.deleted_blobs, 2);
    let again = gc_to_completion(&client, GcRequest::collect());
    assert_eq!(again.deleted_blobs, 0);
    assert!(!client.gc_cancel().expect("cancel idle"));

    server.shutdown();
}

#[test]
fn retiring_a_superseded_revision_keeps_the_head_serving() {
    let (mut server, token) = start_server("retire_revision_e2e");
    let mut client = connect(&server, &token, "retire_revision_cache");

    let v1 = publish(&mut client, "gen/clip", "Clip v1", 0x33, 3_000);
    let v2 = publish_rev(&mut client, "gen/clip", "Clip v2", 0x44, 3_500, Some(v1.asset_id));
    assert_eq!(v1.asset_id, v2.asset_id);
    assert_ne!(v1.revision, v2.revision);
    let alias = AssetAlias::from_str("gen/clip").unwrap();
    assert_eq!(client.resolve_alias(&alias).expect("alias").head_revision, v2.revision);

    let report = client.retire_revision(&v1.asset_id, &v1.revision).expect("retire v1");
    assert!(!report.already_retired);
    assert!(client
        .retire_revision(&v1.asset_id, &v1.revision)
        .expect("retire v1 again")
        .already_retired);

    // The head still serves and the asset is still listed.
    assert_eq!(client.resolve_alias(&alias).expect("alias").head_revision, v2.revision);
    let page = client.assets_page(Some("gen"), None, 10).expect("listing");
    assert_eq!(page.assets.len(), 1);
    let detail = client.asset_detail(&v1.asset_id).expect("detail");
    assert!(!detail.retired);
    let states: Vec<_> = detail
        .candidates
        .iter()
        .map(|c| (c.revision, c.state))
        .collect();
    assert!(states.contains(&(v1.revision, CandidateStateDto::Retired)));
    assert!(states.contains(&(v2.revision, CandidateStateDto::Published)));

    // Collection frees the superseded revision's bytes only.
    let status = gc_to_completion(&client, GcRequest::collect());
    assert_eq!(status.deleted_blobs, 2, "v1's file + thumbnail");
    assert!(matches!(
        client.blob_head(&v1.artifact_blob),
        Err(ClientError::NotFound { .. })
    ));
    assert!(client.blob_head(&v2.artifact_blob).is_ok());

    server.shutdown();
}

#[test]
fn retention_over_http_trims_history_without_touching_heads() {
    let (mut server, token) = start_server("retention_e2e");
    let mut client = connect(&server, &token, "retention_cache");
    let mut published = Vec::new();
    for i in 0..3u8 {
        let asset_id = published.last().map(|p: &makepad_asset_client::Published| p.asset_id);
        published.push(publish_rev(
            &mut client,
            "gen/series",
            &format!("Series v{i}"),
            0x50 + i,
            2_000 + i as usize,
            asset_id,
        ));
    }
    let request = GcRequest { retain_per_asset: Some(1), ..GcRequest::default() };
    let status = gc_to_completion(&client, request);
    // Two superseded revisions retired; the head survives.
    assert_eq!(status.retired_revisions, 2);
    assert_eq!(status.deleted_blobs, 4);
    let head = published.last().unwrap();
    assert!(client.blob_head(&head.artifact_blob).is_ok());
    for old in &published[..2] {
        assert!(matches!(
            client.blob_head(&old.artifact_blob),
            Err(ClientError::NotFound { .. })
        ));
    }
    let alias = AssetAlias::from_str("gen/series").unwrap();
    assert_eq!(client.resolve_alias(&alias).expect("alias").head_revision, head.revision);

    // Retention retires revisions inside the core, without any route being
    // called: detail must still report the real lifecycle (it reads the
    // catalog, not a mirror).
    let detail = client.asset_detail(&head.asset_id).expect("detail");
    assert!(!detail.retired);
    let retired: Vec<_> = detail
        .candidates
        .iter()
        .filter(|c| c.state == CandidateStateDto::Retired)
        .map(|c| c.revision)
        .collect();
    assert_eq!(retired.len(), 2);
    assert!(!retired.contains(&head.revision));
    assert_eq!(
        detail.latest_published().expect("head candidate").revision,
        head.revision
    );

    server.shutdown();
}

#[test]
fn deletion_and_collection_require_credentials() {
    let (mut server, token) = start_server("delete_auth");
    let mut client = connect(&server, &token, "delete_auth_cache");
    let published = publish(&mut client, "gen/guarded", "Guarded", 0x66, 2_000);

    // A well-shaped but unknown token is refused uniformly on every one of
    // the new routes — no oracle, and nothing is deleted.
    let stranger = Api::new(
        ApiEndpoints { control: server.control_addr(), data: server.data_addr() },
        HttpLimits::default_v1(),
        Some(format!("mpat_{}", "ab".repeat(32))),
    )
    .expect("api");
    assert!(matches!(
        stranger.retire_asset(&published.asset_id),
        Err(ClientError::Unauthenticated)
    ));
    assert!(matches!(
        stranger.retire_revision(&published.asset_id, &published.revision),
        Err(ClientError::Unauthenticated)
    ));
    assert!(matches!(
        stranger.gc_blobs(&GcRequest::dry_run()),
        Err(ClientError::Unauthenticated)
    ));
    assert!(matches!(stranger.gc_status(), Err(ClientError::Unauthenticated)));
    assert!(matches!(stranger.gc_cancel(), Err(ClientError::Unauthenticated)));

    // The asset is untouched.
    let detail = client.asset_detail(&published.asset_id).expect("detail");
    assert!(!detail.retired);

    server.shutdown();
}
