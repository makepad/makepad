//! The vision-annotation queue over real sockets.
//!
//! What this proves that a unit test cannot:
//! - publishing an annotatable asset QUEUES its description automatically,
//!   whoever published it — the property the whole design rests on, because
//!   an import, a generation and a game agent all reach the store the same
//!   way and none of them knows this queue exists,
//! - a kind with no turntable sheet (a vjeffect) queues nothing,
//! - the backlog sweep is idempotent: a second sweep over the same assets
//!   enqueues nothing, because the job id is derived from the asset and the
//!   annotator version rather than minted at random,
//! - a worker claims those jobs by kind, and the annotation it PUTs back
//!   takes the asset out of the backlog and makes it findable by the words
//!   in its description,
//! - the summary counts what an operator bar draws.

use makepad_asset_client::{
    Api, ApiEndpoints, AnnotationUpload, AssetClient, ClientConfig, HttpLimits, PublishBundle,
    PublishBundleFile, PublishRights, PublishStats, PublishThumbnail,
};
use makepad_asset_data::{AssetKind, DeviceTier, FileRole, MediaType, ThumbnailMedia};
use makepad_asset_store::host::annotate;
use makepad_asset_store::{AssetServer, ServerConfig};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn test_root(name: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("mp_annotate_http_{}_{}_{}", std::process::id(), n, name))
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

/// One kit piece, exactly as a Kenney import publishes it: a mesh with an
/// alias, a kit category, and NO description — which is the whole problem.
fn piece(i: usize, kind: AssetKind, kit: &str) -> PublishBundle {
    let source = format!("mesh {i} of {kit}\n");
    let thumb: Vec<u8> = (0..1024).map(|b| ((b * 17 + i * 3) % 251) as u8).collect();
    let mut b = PublishBundle::new(
        "kenney",
        kind,
        format!("piece {i}"),
        vec![PublishBundleFile {
            role: mesh_role(kind),
            tier: DeviceTier::Any,
            lod: 0,
            media: mesh_media(kind),
            bytes: source.into_bytes(),
            reference: None,
            dims: None,
        }],
        PublishThumbnail::plain(thumb, ThumbnailMedia::Jpeg, 512, 512),
        PublishRights::generated_cc0(),
    );
    b.alias = format!("kenney/{kit}/piece-{i:03}").parse().ok();
    b.categories = vec!["kenney".into(), kit.into()];
    if kind.has_mesh() {
        b.stats = PublishStats { triangles: 12, vertices: 8, ..PublishStats::default() };
    }
    b
}

/// A mesh kind must publish a render GLB; everything else is a source file.
fn mesh_role(kind: AssetKind) -> FileRole {
    if kind.has_mesh() {
        FileRole::RenderGlb
    } else {
        FileRole::Source
    }
}

fn mesh_media(kind: AssetKind) -> MediaType {
    if kind.has_mesh() {
        MediaType::Glb
    } else {
        MediaType::Text
    }
}

#[test]
fn publishing_an_annotatable_asset_queues_its_description() {
    let (server, token) = start_server("publish");
    let mut client = connect(&server, &token, "publish_cache");
    let api = api(&server, &token);

    let bundles = vec![
        piece(0, AssetKind::Mesh, "nature-kit"),
        piece(1, AssetKind::Character, "nature-kit"),
        // A vjeffect has no turntable sheet; describing sixteen views of it
        // would be describing something that is not there.
        {
            let mut b = piece(2, AssetKind::VjEffect, "nature-kit");
            b.alias = "kenney/nature-kit/not-a-mesh".parse().ok();
            b
        },
    ];
    let published = client.publish_bundles(&bundles).expect("publish");

    // Two of the three owe a description, and the server queued both
    // without anybody asking it to.
    let summary = api.annotate_summary(None).expect("summary");
    assert_eq!(summary.version_tag, annotate::version_tag());
    assert_eq!(summary.owed, 2, "mesh + character owe, vjeffect does not");
    assert_eq!(summary.annotated, 0);
    assert_eq!(summary.pending, 2, "publishing IS the enqueue");

    // A NEW REVISION of the same asset must not queue a second job: the job
    // id is derived from the asset and the annotator version, so the second
    // publish mints exactly the same id and the enqueue is refused. (A new
    // asset would rightly get its own job, which is why the id is pinned.)
    let again: Vec<PublishBundle> = published
        .iter()
        .zip(bundles.iter())
        .map(|(done, b)| {
            let mut next = b.clone();
            next.asset_id = Some(done.asset_id);
            next.title = format!("{} v2", b.title);
            next
        })
        .collect();
    client.publish_bundles(&again).expect("republish");
    let summary = api.annotate_summary(None).expect("summary");
    assert_eq!(summary.pending, 2, "a new revision is not a second job");
    assert_eq!(summary.owed, 2);
}

#[test]
fn a_backlog_sweep_is_idempotent_and_scoped_to_a_kit() {
    let (server, token) = start_server("sweep");
    let mut client = connect(&server, &token, "sweep_cache");
    let api = api(&server, &token);

    let mut bundles: Vec<PublishBundle> = (0..4)
        .map(|i| piece(i, AssetKind::Mesh, "nature-kit"))
        .collect();
    bundles.extend((10..13).map(|i| piece(i, AssetKind::Mesh, "brick-kit")));
    client.publish_bundles(&bundles).expect("publish");

    // The publish already queued all seven; a sweep over them is a no-op,
    // which is what makes the button safe to press at any time.
    let r = api.annotate_backlog(1000, None, 0).expect("sweep");
    assert_eq!(r.enqueued, 0, "everything was already queued by the publish");
    assert_eq!(r.skipped, 7);
    assert_eq!(r.remaining, 7, "queued is not described");

    // Scoping to one kit counts only that kit.
    let r = api.annotate_backlog(1000, Some("brick-kit"), 0).expect("kit sweep");
    assert_eq!(r.remaining, 3);
    let kit = api.annotate_summary(Some("nature-kit")).expect("kit summary");
    assert_eq!(kit.owed, 4);
    // Job counts are catalog-wide even when the asset counts are scoped:
    // the queue is one queue.
    assert_eq!(kit.pending, 7);
}

#[test]
fn a_worker_claims_by_kind_and_its_annotation_leaves_the_backlog() {
    let (server, token) = start_server("worker");
    let mut client = connect(&server, &token, "worker_cache");
    let api = api(&server, &token);

    let bundles: Vec<PublishBundle> = (0..3)
        .map(|i| piece(i, AssetKind::Mesh, "nature-kit"))
        .collect();
    let published = client.publish_bundles(&bundles).expect("publish");

    // The worker claims only its own kind — a generation coordinator
    // sharing this server never swallows an annotate job, and this one
    // never swallows a video render.
    let claimed = api
        .worker_claim_kinds(60_000, Some("annotate"), &[makepad_asset_annotate_kind()])
        .expect("claim")
        .expect("a queued annotate job");
    assert_eq!(claimed.kind, annotate::JOB_KIND);
    assert_eq!(claimed.namespace, "kenney");
    let alias = claimed
        .body
        .get("alias")
        .and_then(|v| v.as_str())
        .expect("the body names the alias the sheet is fetched by");
    assert!(alias.starts_with("kenney/nature-kit/piece-"), "{alias}");
    let asset = claimed
        .body
        .get("asset")
        .and_then(|v| v.as_str())
        .and_then(|t| t.parse::<makepad_asset_data::AssetId>().ok())
        .expect("the body names the asset");
    assert!(published.iter().any(|p| p.asset_id == asset));

    // What the pass does with it: read the record, add the description and
    // the facets it owns, write the whole record back.
    let current = api.get_annotation(&asset).expect("read annotation");
    assert!(current.description.is_empty(), "an import writes no description");
    let mut tags = current.tags.clone();
    tags.push(annotate::version_tag());
    tags.push("vlm-cat-tree".into());
    api.put_annotation(
        &asset,
        &AnnotationUpload {
            title: current.title.clone(),
            description: "pine tree; standalone; tall; green/brown; conical needled canopy"
                .into(),
            kind: current.kind,
            categories: current.categories.clone(),
            tags,
            creator: current.creator.clone(),
            generator: current.generator.clone(),
            backend: current.backend.clone(),
            model: current.model.clone(),
            prompt: current.prompt.clone(),
            provenance: current.provenance.clone(),
            private: current.private,
        },
    )
    .expect("put annotation");
    api.worker_succeed(&claimed.job, Some("annotate"), None)
        .expect("succeed");

    // The count moved, and the words are now retrievable — which is the
    // entire point: before this, "a conical needled tree" matched nothing.
    let summary = api.annotate_summary(None).expect("summary");
    assert_eq!(summary.annotated, 1);
    assert_eq!(summary.owed, 2);
    assert_eq!(summary.succeeded, 1);
    // Writing an annotation is itself an enqueue trigger (the split publish
    // flow has no kind or alias until the annotation lands). The pass's own
    // write must not therefore re-queue what it just described: it carries
    // the version tag, so the asset is no longer in the backlog at all.
    assert_eq!(summary.pending, 2, "the pass does not re-queue its own work");

    let mut query = makepad_asset_client::CatalogQuery::browse(10);
    query.text = "needled canopy".to_string();
    let page = client.catalog_search(&query, None).expect("search");
    assert_eq!(page.hits.len(), 1, "the description is what search hits");
    assert_eq!(page.hits[0].asset_id, asset);

    // And the annotated asset is out of the backlog for good: a later sweep
    // does not queue it again.
    let r = api.annotate_backlog(1000, None, 0).expect("sweep");
    assert_eq!(r.annotated, 1);
    assert_eq!(r.remaining, 2);
}

#[test]
fn work_undone_is_queued_again_rather_than_tombstoned() {
    // A re-import replaces the annotation, which erases the description the
    // pass wrote. The asset owes one again — and because the job id is
    // DERIVED from the asset, the already-succeeded job must be revived
    // rather than blocking the work for ever.
    let (server, token) = start_server("revive");
    let mut client = connect(&server, &token, "revive_cache");
    let api = api(&server, &token);

    let bundles = vec![piece(0, AssetKind::Mesh, "nature-kit")];
    let published = client.publish_bundles(&bundles).expect("publish");
    let asset = published[0].asset_id;

    let claimed = api
        .worker_claim_kinds(60_000, Some("annotate"), &[makepad_asset_annotate_kind()])
        .expect("claim")
        .expect("a queued job");
    let current = api.get_annotation(&asset).expect("annotation");
    let mut tags = current.tags.clone();
    tags.push(annotate::version_tag());
    api.put_annotation(
        &asset,
        &AnnotationUpload {
            title: current.title.clone(),
            description: "pine tree; standalone; tall".into(),
            kind: current.kind,
            categories: current.categories.clone(),
            tags,
            creator: current.creator.clone(),
            generator: current.generator.clone(),
            backend: current.backend.clone(),
            model: current.model.clone(),
            prompt: current.prompt.clone(),
            provenance: current.provenance.clone(),
            private: current.private,
        },
    )
    .expect("put annotation");
    api.worker_succeed(&claimed.job, Some("annotate"), None).expect("succeed");
    let summary = api.annotate_summary(None).expect("summary");
    assert_eq!(summary.annotated, 1);
    assert_eq!(summary.succeeded, 1);
    assert_eq!(summary.pending, 0);

    // The re-import: the same annotation an importer writes, with no vlm
    // tags and no description.
    api.put_annotation(
        &asset,
        &AnnotationUpload {
            title: current.title.clone(),
            description: String::new(),
            kind: current.kind,
            categories: current.categories.clone(),
            tags: current.tags.clone(),
            creator: current.creator.clone(),
            generator: current.generator.clone(),
            backend: current.backend.clone(),
            model: current.model.clone(),
            prompt: current.prompt.clone(),
            provenance: current.provenance.clone(),
            private: current.private,
        },
    )
    .expect("re-import annotation");

    let summary = api.annotate_summary(None).expect("summary");
    assert_eq!(summary.owed, 1, "the description is gone, so it is owed again");
    assert_eq!(summary.pending, 1, "and the job was revived, not tombstoned");
    assert_eq!(summary.succeeded, 0, "the same job, back in the queue");

    // And it really is claimable again, with the attempt numbered after the
    // first one rather than colliding with it.
    let again = api
        .worker_claim_kinds(60_000, Some("annotate"), &[makepad_asset_annotate_kind()])
        .expect("claim")
        .expect("the revived job");
    assert_eq!(again.job, claimed.job, "the derived id is stable");
    assert_eq!(again.attempt, 2);
}

/// The job kind, spelled where the worker crate spells it. The store and
/// the annotate crate do not depend on each other; this string is the wire.
fn makepad_asset_annotate_kind() -> &'static str {
    "annotate.asset"
}
