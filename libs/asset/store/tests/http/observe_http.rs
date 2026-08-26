//! LIVECODING END TO END: an observed origin directory against a real
//! server on a real socket.
//!
//! What this proves that the unit tests cannot:
//! - a `.splash` dropped in an origin directory becomes a catalog asset
//!   under `vjfx/<stem>`, published BY REFERENCE — the store's own tree
//!   gains a thumbnail and a manifest, and no copy of the document,
//! - editing that file publishes a NEW REVISION of the SAME asset and
//!   re-points the alias, which is the whole hot-reload contract (a client
//!   holding the alias sees a republish; the old revision still serves the
//!   old bytes),
//! - re-observing an unchanged file writes nothing at all,
//! - and the running `run()` loop does all of that on its own, from the
//!   initial sweep through a live edit, and stops promptly when asked.

use makepad_asset_client::{ApiEndpoints, AssetClient, ClientConfig};
use makepad_asset_data::{AssetKind, BlobId, FileRole};
use makepad_asset_store::observe::{self, ObserveConfig, Outcome};
use makepad_asset_store::{AssetServer, BlobRefPolicy, ServerConfig};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn test_root(name: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("mp_observe_http_{}_{}_{}", std::process::id(), n, name))
}

fn start_server(name: &str) -> (AssetServer, String, PathBuf) {
    let root = test_root(name);
    let mut cfg = ServerConfig::new(root.clone());
    cfg.control_addr = "127.0.0.1:0".parse().unwrap();
    cfg.data_addr = "127.0.0.1:0".parse().unwrap();
    cfg.bootstrap_admin = true;
    cfg.log = false;
    // The privilege decision the observer depends on, made explicitly here
    // exactly as the hosting apps make it.
    cfg.blob_refs = BlobRefPolicy::local_host();
    let server = AssetServer::start(cfg).expect("server start");
    let token = std::fs::read_to_string(root.join("admin-token"))
        .expect("admin token")
        .trim()
        .to_string();
    (server, token, root)
}

fn connect(server: &AssetServer, token: &str, cache: &str) -> AssetClient {
    let mut cfg = ClientConfig::new(test_root(cache));
    cfg.token = Some(token.to_string());
    let endpoints = ApiEndpoints { control: server.control_addr(), data: server.data_addr() };
    AssetClient::connect(cfg, endpoints, Some(server.server_id())).expect("connect")
}

const DOC_V1: &str = "// A LOOK — the first version.\n{\n    name: \"Live One\"\n    engine: \"particles\"\n}\n";
const DOC_V2: &str = "// A LOOK — edited in place.\n{\n    name: \"Live One\"\n    engine: \"particles\"\n    p0: 0.75\n}\n";

/// The head revision's `Source` blob id, which IS the document's sha256.
fn head_source_digest(client: &mut AssetClient, alias: &str) -> BlobId {
    let alias = alias.parse().expect("alias");
    let dto = client.resolve_alias(&alias).expect("alias resolves");
    let manifest = client
        .fetch_asset_manifest(&dto.head_revision)
        .expect("head manifest");
    assert_eq!(manifest.kind, AssetKind::VjEffect);
    manifest
        .files
        .iter()
        .find(|f| f.role == FileRole::Source)
        .expect("a Source file")
        .blob
}

#[test]
fn a_dropped_document_publishes_by_reference_and_an_edit_republishes_the_alias() {
    let (server, token, server_root) = start_server("edit");
    let mut client = connect(&server, &token, "edit-cache");

    let origin = test_root("origin");
    std::fs::create_dir_all(&origin).unwrap();
    let doc = origin.join("42_live_one.splash");
    std::fs::write(&doc, DOC_V1).unwrap();

    let config = ObserveConfig::vjfx(vec![origin.clone()]);
    assert_eq!(
        observe::publish_doc(&mut client, &doc, &config),
        Outcome::Published { alias: "vjfx/42_live_one".to_string() }
    );
    assert_eq!(
        head_source_digest(&mut client, "vjfx/42_live_one"),
        BlobId::hash_of(DOC_V1.as_bytes()),
        "the head must carry exactly the bytes on disk"
    );

    // NO COPY: the document's bytes are nowhere in the server's own tree.
    let mut found_copy = false;
    let mut stack = vec![server_root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if std::fs::read(&path).map(|b| b == DOC_V1.as_bytes()).unwrap_or(false) {
                found_copy = true;
            }
        }
    }
    assert!(!found_copy, "the store copied the document it was told to reference");

    // Observing an unchanged file is free: no new revision, no catalog write.
    let first_asset = client
        .resolve_alias(&"vjfx/42_live_one".parse().unwrap())
        .unwrap()
        .asset_id;
    let first_rev = client
        .resolve_alias(&"vjfx/42_live_one".parse().unwrap())
        .unwrap()
        .head_revision;
    assert_eq!(
        observe::publish_doc(&mut client, &doc, &config),
        Outcome::Unchanged { alias: "vjfx/42_live_one".to_string() }
    );
    assert_eq!(
        client
            .resolve_alias(&"vjfx/42_live_one".parse().unwrap())
            .unwrap()
            .head_revision,
        first_rev
    );

    // THE HOT EDIT: same file, new bytes, new revision of the SAME asset.
    std::fs::write(&doc, DOC_V2).unwrap();
    assert_eq!(
        observe::publish_doc(&mut client, &doc, &config),
        Outcome::Republished { alias: "vjfx/42_live_one".to_string() }
    );
    let after = client
        .resolve_alias(&"vjfx/42_live_one".parse().unwrap())
        .unwrap();
    assert_eq!(after.asset_id, first_asset, "an edit must not mint a second asset");
    assert_ne!(after.head_revision, first_rev, "an edit must be a new revision");
    assert_eq!(
        head_source_digest(&mut client, "vjfx/42_live_one"),
        BlobId::hash_of(DOC_V2.as_bytes())
    );

    // The superseded revision is still exactly what it always was: identity
    // is the content, so nothing anybody pinned changed underneath them.
    let old = client.fetch_asset_manifest(&first_rev).expect("old manifest");
    assert_eq!(
        old.files.iter().find(|f| f.role == FileRole::Source).unwrap().blob,
        BlobId::hash_of(DOC_V1.as_bytes())
    );

    std::fs::remove_dir_all(&origin).ok();
}

#[test]
fn a_transition_document_lands_in_the_transition_lane() {
    let (server, token, _) = start_server("transition");
    let mut client = connect(&server, &token, "transition-cache");
    let origin = test_root("origin-trans");
    std::fs::create_dir_all(&origin).unwrap();
    let doc = origin.join("200_my_wipe.splash");
    std::fs::write(
        &doc,
        "// A WIPE.\n{\n    name: \"My Wipe\"\n    engine: \"transition\"\n}\n",
    )
    .unwrap();
    let config = ObserveConfig::vjfx(vec![origin.clone()]);
    assert!(matches!(
        observe::publish_doc(&mut client, &doc, &config),
        Outcome::Published { .. }
    ));

    let mut query = makepad_asset_client::CatalogQuery::browse(50);
    query.kind = Some(AssetKind::VjEffect);
    query.tag = Some("transition".to_string());
    let page = client.catalog_search(&query, None).expect("tag search");
    assert_eq!(page.hits.len(), 1, "the transition chip's lane must show it");
    std::fs::remove_dir_all(&origin).ok();
}

#[test]
fn the_running_loop_sweeps_at_start_follows_live_edits_and_stops_promptly() {
    let (server, token, _) = start_server("loop");
    let mut client = connect(&server, &token, "loop-cache");
    let mut probe = connect(&server, &token, "loop-probe");

    let origin = test_root("origin-loop");
    std::fs::create_dir_all(&origin).unwrap();
    // Present before the observer starts: the initial sweep's job.
    let doc = origin.join("77_swept.splash");
    std::fs::write(&doc, DOC_V1).unwrap();

    let mut config = ObserveConfig::vjfx(vec![origin.clone()]);
    config.log = false;
    config.debounce_ms = 20;
    config.stable_ms = 20;
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread = std::thread::spawn(move || {
        observe::run(&mut client, &config, &thread_stop);
    });

    let alias: makepad_asset_data::AssetAlias = "vjfx/77_swept".parse().unwrap();
    let wait_for = |probe: &mut AssetClient, want: BlobId| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(dto) = probe.resolve_alias(&alias) {
                if let Ok(manifest) = probe.fetch_asset_manifest(&dto.head_revision) {
                    let blob = manifest
                        .files
                        .iter()
                        .find(|f| f.role == FileRole::Source)
                        .map(|f| f.blob);
                    if blob == Some(want) {
                        return;
                    }
                }
            }
            assert!(Instant::now() < deadline, "the observer never published {want}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    wait_for(&mut probe, BlobId::hash_of(DOC_V1.as_bytes()));

    // A LIVE EDIT while the loop runs — the actual livecoding gesture.
    std::fs::write(&doc, DOC_V2).unwrap();
    wait_for(&mut probe, BlobId::hash_of(DOC_V2.as_bytes()));

    stop.store(true, Ordering::Release);
    let started = Instant::now();
    thread.join().expect("observer thread");
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "stop must be prompt, took {:?}",
        started.elapsed()
    );
    std::fs::remove_dir_all(&origin).ok();
}
