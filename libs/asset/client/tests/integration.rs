//! End-to-end integration over real sockets: discovery → authenticated
//! selection → paginated browse/search → digest-verified manifest and blob
//! fetch with Range resume → pinned, budgeted cache → resolver → runtime
//! states. The fixture serves REAL canonical manifests; every digest the
//! client checks is a genuine SHA-256 of genuine bytes.

mod common;

use common::{
    payload, response_head, test_root, write_bytes_resp, write_error, write_json_resp, write_raw,
    FixtureOptions, FixtureServer, FixtureStore, ParsedRequest, RawServer,
};
use makepad_asset_client::json::{obj, s, Value};
use makepad_asset_client::{
    AssetClient, CacheBudgets, CatalogQuery, ClientConfig, ClientError, ClientEvent, ClientOutput,
    ClientRequest, ClientRuntime, ClosureBudget, DiscoveryListener, HttpLimits, ResourceSlot,
    RuntimeConfig, SubmitOptions,
    SourceCollectionRegistered, TierPreference,
};
use makepad_asset_data::*;
use std::collections::HashMap;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn fast_limits() -> HttpLimits {
    HttpLimits {
        connect_timeout_ms: 2_000,
        read_timeout_ms: 1_000,
        write_timeout_ms: 2_000,
        head_deadline_ms: 2_000,
        body_deadline_ms: 5_000,
    }
}

fn config(name: &str) -> ClientConfig {
    let mut cfg = ClientConfig::new(test_root(name));
    cfg.http = fast_limits();
    cfg.blob_body_deadline_ms = 5_000;
    cfg
}

fn pin_marker(cache_root: &Path, blob: &BlobId) -> PathBuf {
    cache_root
        .join("pins")
        .join(makepad_asset_client::util::to_hex(blob.as_bytes()))
}

fn wait_runtime(
    runtime: &mut ClientRuntime,
    request: makepad_asset_client::RequestId,
) -> Result<ClientOutput, ClientError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "runtime request {request} never finished"
        );
        for event in runtime.poll() {
            match event {
                ClientEvent::Done { id, output } if id == request => return Ok(output),
                ClientEvent::Failed { id, error } if id == request => return Err(error),
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// A store with aliased props, a dependency chain, and a game revision.
fn seeded_store() -> (FixtureStore, Vec<AssetRevisionRef>) {
    let mut store = FixtureStore::default();
    let mut refs = Vec::new();
    // Ten rockets for pagination.
    for i in 0..10u8 {
        let r = store.add_prop(
            10 + i,
            "stock",
            (i == 0).then_some("stock/rocket-launcher"),
            &format!("Rocket {i}"),
            payload(100 + i as u64, 2_000 + i as usize * 100),
            vec![],
        );
        refs.push(r);
    }
    // A crate in another namespace.
    store.add_prop(40, "props", Some("props/crate"), "Wooden Crate", payload(200, 1_500), vec![]);
    (store, refs)
}

/// The VJ filter contract: `exclude_tag` travels on the wire and the SERVER
/// drops the rows — the client never post-filters a page, so `total`, the
/// page contents and the cursor walk all agree with the exclusion.
#[test]
fn catalog_search_exclude_tag_is_filtered_server_side() {
    let (mut store, refs) = seeded_store();
    // Rockets 1, 3, 5, 7, 9 are intermediates; 3 also carries `keep`.
    for (i, r) in refs.iter().enumerate() {
        if i % 2 == 1 {
            store.tag_asset(r, &["keep", "intermediate"]);
        } else {
            store.tag_asset(r, &["keep"]);
        }
    }
    let fixture = FixtureServer::start(store, FixtureOptions::default());
    let client =
        AssetClient::connect(config("exclude_tag"), fixture.endpoints(), None).unwrap();

    let mut q = CatalogQuery::text("rocket", 10);
    q.tag = Some("keep".into());
    assert_eq!(client.catalog_search(&q, None).unwrap().total, 10);
    q.exclude_tag = Some("intermediate".into());
    let page = client.catalog_search(&q, None).unwrap();
    assert_eq!(page.total, 5, "the server dropped the intermediates");
    assert_eq!(page.hits.len(), 5);
    assert!(page.next.is_none());
    let kept: Vec<AssetId> =
        refs.iter().step_by(2).map(|r| r.asset_id).collect();
    let mut got: Vec<AssetId> = page.hits.iter().map(|h| h.asset_id).collect();
    got.sort();
    let mut want = kept.clone();
    want.sort();
    assert_eq!(got, want);

    // Paging over the excluded set: excluded rows interleave the kept ones,
    // and the cursor walk still yields each kept row exactly once.
    q.page_size = 2;
    let mut seen = Vec::new();
    let mut cursor = None;
    for _ in 0..10 {
        let page = client.catalog_search(&q, cursor.as_ref()).unwrap();
        assert_eq!(page.total, 5);
        seen.extend(page.hits.iter().map(|h| h.asset_id));
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    seen.sort();
    assert_eq!(seen, want);
}

#[test]
fn discovery_select_connect_browse_paginate() {
    let token = format!("mpat_{}", "ab".repeat(32));
    let (store, _refs) = seeded_store();
    let options = FixtureOptions {
        server_id: [0x42; 16],
        auth_token: Some(token.clone()),
        ..FixtureOptions::default()
    };
    let fixture = FixtureServer::start(store, options);

    // ---- discovery on a loopback ephemeral port ----
    let listener = DiscoveryListener::start(0, 60_000, now_ms).unwrap();
    fixture.send_beacon(listener.port(), true);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let candidate = loop {
        if let Some(c) = listener.pick(makepad_asset_client::content_client_caps(), now_ms())
        {
            break c;
        }
        assert!(std::time::Instant::now() < deadline, "no beacon received");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(candidate.server_id, [0x42; 16]);
    assert!(candidate.auth_required);

    // ---- authenticated selection ----
    let mut cfg = config("discovery_connect");
    cfg.token = Some(token);
    let client = AssetClient::connect_discovered(cfg, &candidate).unwrap();
    assert_eq!(client.server_id(), [0x42; 16]);

    // ---- catalog search pagination: exact continuation, no dup/skip ----
    let query = CatalogQuery::text("rocket", 3);
    let mut seen = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = client.catalog_search(&query, cursor.as_ref()).unwrap();
        assert_eq!(page.total, 10);
        assert!(page.hits.len() <= 3);
        seen.extend(page.hits.iter().map(|h| h.asset_id));
        pages += 1;
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
        assert!(pages < 10, "pagination never terminated");
    }
    assert_eq!(pages, 4, "10 hits at page size 3");
    assert_eq!(seen.len(), 10);
    let mut dedup = seen.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), 10, "pagination duplicated a row");

    // ---- keyset listing with namespace filter ----
    let mut listed = Vec::new();
    let mut cursor = None;
    loop {
        let page = client.assets_page(Some("stock"), cursor.as_ref(), 4).unwrap();
        listed.extend(page.assets.iter().map(|a| a.asset_id));
        for a in &page.assets {
            assert_eq!(a.namespace, "stock");
        }
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(listed.len(), 10);
    let mut sorted = listed.clone();
    sorted.sort_by_key(|id| id.to_string());
    assert_eq!(listed, sorted, "keyset listing must be ordered");
}

#[test]
fn alias_to_verified_file_end_to_end() {
    let (store, _refs) = seeded_store();
    let glb_bytes = {
        // Original bytes of the aliased rocket's render blob.
        let a = store
            .assets
            .iter()
            .find(|a| a.alias.as_deref() == Some("stock/rocket-launcher"))
            .unwrap();
        let file = &a.manifest.files[0];
        store.blobs[file.blob.as_bytes()].clone()
    };
    let fixture = FixtureServer::start(store, FixtureOptions::default());
    let mut client = AssetClient::connect(config("alias_e2e"), fixture.endpoints(), None).unwrap();

    // alias → head revision → detail agrees
    let alias = AssetAlias::from_str("stock/rocket-launcher").unwrap();
    let head = client.resolve_alias(&alias).unwrap();
    let detail = client.asset_detail(&head.asset_id).unwrap();
    assert_eq!(detail.latest_published().unwrap().revision, head.head_revision);

    // manifest: typed, digest-verified, byte-cached
    let manifest = client.fetch_asset_manifest(&head.head_revision).unwrap();
    assert_eq!(manifest.asset_id, head.asset_id);

    // resolver: role-selected, digest-verified local file
    let resolved = client
        .resolve_file(
            &manifest,
            FileRole::RenderGlb,
            TierPreference::PreferWithAnyFallback(DeviceTier::High),
            7,
            None,
        )
        .unwrap();
    assert_eq!(std::fs::read(&resolved.path).unwrap(), glb_bytes);
    assert_eq!(resolved.byte_len, glb_bytes.len() as u64);

    // thumbnail is typed and materialized
    let thumb = client.resolve_thumbnail(&manifest).unwrap().expect("mesh thumbnail");
    assert!(thumb.path.exists());

    // HEAD probe reports the exact size and echoes the identity as a strong
    // ETag (the precondition for trusting a later If-Range resume).
    let head_probe = client.blob_head(&manifest.files[0].blob).unwrap();
    assert_eq!(head_probe.size, manifest.files[0].byte_len);
    assert!(head_probe.etag_matches);

    // Second resolve is served from cache: zero new blob requests.
    let gets_before = fixture.log.count("GET", "/v1/blobs/");
    let again = client
        .resolve_file(
            &manifest,
            FileRole::RenderGlb,
            TierPreference::PreferWithAnyFallback(DeviceTier::High),
            7,
            None,
        )
        .unwrap();
    assert_eq!(again.path, resolved.path);
    assert_eq!(fixture.log.count("GET", "/v1/blobs/"), gets_before, "cache miss on hot path");

    // Bytes path agrees with file path.
    let bytes = client
        .resolve_file_bytes(
            &manifest,
            FileRole::RenderGlb,
            TierPreference::PreferWithAnyFallback(DeviceTier::High),
            7,
        )
        .unwrap();
    assert_eq!(bytes, glb_bytes);

    // Manifest re-fetch is also cache-served.
    let revs_before = fixture.log.count("GET", "/v1/revisions/");
    let _ = client.fetch_asset_manifest(&head.head_revision).unwrap();
    assert_eq!(fixture.log.count("GET", "/v1/revisions/"), revs_before);
}

#[test]
fn range_resume_after_connection_kill() {
    let mut store = FixtureStore::default();
    let big = payload(555, 300_000);
    let r = store.add_prop(50, "stock", None, "Big Rocket", big.clone(), vec![]);
    let fixture = FixtureServer::start(store, FixtureOptions::default());
    let mut client = AssetClient::connect(config("resume"), fixture.endpoints(), None).unwrap();

    let manifest = client.fetch_asset_manifest(&r.revision).unwrap();
    let file = manifest.files[0].clone();

    // First attempt dies after ~90KB of body.
    *fixture.knobs.kill_blob_after.lock().unwrap() = Some(90_000);

    let mut progress_points = Vec::new();
    let mut progress = |bytes: u64, total: u64| progress_points.push((bytes, total));
    let path = client
        .fetch_blob(&file.blob, Some(file.byte_len), Some(&mut progress))
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), big);

    // Exactly two GETs: the killed one and the resume.
    assert_eq!(fixture.log.count("GET", "/v1/blobs/"), 2);
    let resume = fixture.log.last_matching("GET", "/v1/blobs/").unwrap();
    let range = resume.header("range").expect("resume must use Range");
    let start: u64 = range
        .strip_prefix("bytes=")
        .and_then(|r| r.strip_suffix('-'))
        .and_then(|r| r.parse().ok())
        .expect("open-ended range shape");
    assert!(start > 0 && start < big.len() as u64, "resume offset {start}");
    let if_range = resume.header("if-range").expect("resume must gate on If-Range");
    assert_eq!(if_range, format!("\"{}\"", file.blob));

    // Progress is monotonic and ends complete.
    assert!(progress_points.windows(2).all(|w| w[0].0 <= w[1].0));
    assert_eq!(progress_points.last().unwrap(), &(big.len() as u64, big.len() as u64));
}

#[test]
fn restart_resumes_partial_across_client_instances() {
    let mut store = FixtureStore::default();
    let big = payload(556, 200_000);
    let r = store.add_prop(51, "stock", None, "Interrupted Rocket", big.clone(), vec![]);
    let fixture = FixtureServer::start(store, FixtureOptions::default());

    let root = test_root("restart_resume");
    let file = {
        let mut cfg = config("unused");
        cfg.cache_root = root.clone();
        cfg.max_transfer_attempts = 1; // fail hard on the kill
        let mut client = AssetClient::connect(cfg, fixture.endpoints(), None).unwrap();
        let manifest = client.fetch_asset_manifest(&r.revision).unwrap();
        let file = manifest.files[0].clone();
        *fixture.knobs.kill_blob_after.lock().unwrap() = Some(60_000);
        let err = client.fetch_blob(&file.blob, Some(file.byte_len), None).unwrap_err();
        assert!(matches!(err, ClientError::Io { .. }), "{err:?}");
        file
        // client drops; partial stays on disk
    };

    // A NEW client on the same cache root resumes instead of restarting.
    let mut cfg = config("unused2");
    cfg.cache_root = root;
    let mut client = AssetClient::connect(cfg, fixture.endpoints(), None).unwrap();
    let path = client.fetch_blob(&file.blob, Some(file.byte_len), None).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), big);
    let resume = fixture.log.last_matching("GET", "/v1/blobs/").unwrap();
    assert!(resume.header("range").is_some(), "second process must resume, not restart");
}

#[test]
fn offline_resolution_from_cache_and_honest_miss() {
    let (store, refs) = seeded_store();
    let fixture = FixtureServer::start(store, FixtureOptions::default());
    let mut client = AssetClient::connect(config("offline"), fixture.endpoints(), None).unwrap();

    let manifest = client.fetch_asset_manifest(&refs[0].revision).unwrap();
    let resolved = client
        .resolve_file(
            &manifest,
            FileRole::RenderGlb,
            TierPreference::PreferWithAnyFallback(DeviceTier::Low),
            7,
            None,
        )
        .unwrap();

    // Server goes away entirely.
    let mut fixture = fixture;
    fixture.control.stop();
    fixture.data.stop();

    // Cached content still resolves, fully verified, with zero network.
    let again = client
        .resolve_file(
            &manifest,
            FileRole::RenderGlb,
            TierPreference::PreferWithAnyFallback(DeviceTier::Low),
            7,
            None,
        )
        .unwrap();
    assert_eq!(again.path, resolved.path);
    let cached = client.cached_blob(&manifest.files[0].blob).unwrap();
    assert!(cached.is_some());

    // Uncached content fails with an explicit transport error — no guess, no
    // fallback, no stale substitute.
    let other = client.fetch_asset_manifest(&refs[5].revision);
    match other {
        Err(ClientError::Io { .. }) | Err(ClientError::Timeout { .. }) => {}
        other => panic!("offline miss must be an explicit transport error: {other:?}"),
    }
}

#[test]
fn pinning_survives_eviction_pressure_end_to_end() {
    let mut store = FixtureStore::default();
    let precious = payload(700, 30_000);
    let precious_ref = store.add_prop(60, "stock", None, "Precious", precious.clone(), vec![]);
    for i in 0..4u8 {
        store.add_prop(61 + i, "stock", None, &format!("Filler {i}"), payload(710 + i as u64, 40_000), vec![]);
    }
    let fixture = FixtureServer::start(store, FixtureOptions::default());

    let mut cfg = config("pin_pressure");
    cfg.cache = CacheBudgets {
        // Room for ~two big blobs (plus manifests/thumbs): heavy pressure.
        max_total_bytes: 100_000,
        max_object_bytes: 60_000,
        max_partial_bytes: 100_000,
        stale_partial_ms: 1_000_000,
        max_ram_bytes: 512 * 1024,
    };
    let mut client = AssetClient::connect(cfg, fixture.endpoints(), None).unwrap();

    let manifest = client.fetch_asset_manifest(&precious_ref.revision).unwrap();
    let blob = manifest.files[0].blob;
    client.pin_blob(&blob).unwrap();
    client.fetch_blob(&blob, Some(manifest.files[0].byte_len), None).unwrap();
    let gets_for_precious = fixture.log.count("GET", &format!("/v1/blobs/{blob}"));

    // Churn: pull every filler through the small cache.
    for i in 0..4u8 {
        let asset_id = makepad_asset_data::AssetId::from_bytes([61 + i; 16]);
        let detail = client.asset_detail(&asset_id).unwrap();
        let rev = detail.latest_published().unwrap().revision;
        let m = client.fetch_asset_manifest(&rev).unwrap();
        client.fetch_blob(&m.files[0].blob, Some(m.files[0].byte_len), None).unwrap();
    }
    assert!(client.cache_stats().evictions > 0, "pressure never evicted anything");

    // The pinned blob is still local — no refetch happened.
    assert!(client.cached_blob(&blob).unwrap().is_some(), "pinned blob was evicted");
    assert_eq!(
        fixture.log.count("GET", &format!("/v1/blobs/{blob}")),
        gets_for_precious,
        "pinned blob was refetched"
    );
}

#[test]
fn dependency_closure_bounded_and_verified() {
    let mut store = FixtureStore::default();
    let leaf = store.add_prop(72, "stock", None, "Leaf", payload(900, 800), vec![]);
    let mid = store.add_prop(71, "stock", None, "Mid", payload(901, 800), vec![leaf]);
    let root_ref = store.add_prop(70, "stock", None, "Root", payload(902, 800), vec![mid]);
    let fixture = FixtureServer::start(store, FixtureOptions::default());
    let mut client = AssetClient::connect(config("closure"), fixture.endpoints(), None).unwrap();

    let closure = client.resolve_closure(&root_ref, ClosureBudget::default()).unwrap();
    let ids: Vec<_> = closure.iter().map(|(r, _)| r.asset_id).collect();
    assert_eq!(ids.len(), 3);
    assert_eq!(closure[0].0, root_ref, "BFS order starts at the root");
    // Every manifest's declared pair was proven.
    for (r, m) in &closure {
        assert_eq!(m.asset_id, r.asset_id);
    }

    // Asset budget refusal.
    let err = client
        .resolve_closure(&root_ref, ClosureBudget { max_assets: 2, max_depth: 8 })
        .unwrap_err();
    assert!(matches!(err, ClientError::OverBudget { what: "closure assets", .. }), "{err:?}");

    // Depth budget refusal.
    let err = client
        .resolve_closure(&root_ref, ClosureBudget { max_assets: 10, max_depth: 1 })
        .unwrap_err();
    assert!(matches!(err, ClientError::OverBudget { what: "closure depth", .. }), "{err:?}");
}

/// The bridge for tools that take a FILE rather than bytes (an AO bake, a
/// rig pass, an OS drag-out): a verified on-disk path for catalog content.
/// It stays thin-client-legal because the object is named by its digest and
/// re-hashed before the path is handed out — a materialisation of the
/// revision, never a second source of truth.
#[test]
fn blob_path_materialises_verified_content_and_re_fetches_a_corrupted_object() {
    let mut store = FixtureStore::default();
    let payload = vec![7u8; 60_000];
    let blob = store.add_blob(payload.clone());
    let other = store.add_blob(b"a second, different object".to_vec());
    let fixture = FixtureServer::start(store, FixtureOptions::default());
    let mut client = AssetClient::connect(config("blob_path"), fixture.endpoints(), None).unwrap();

    let path = client.blob_path(&blob, Some(payload.len() as u64)).unwrap();
    assert!(path.is_file(), "the object is on disk at {}", path.display());
    assert_eq!(std::fs::read(&path).unwrap(), payload, "and it is the real payload");
    // Digest-keyed: the same blob resolves to the same path, and a second
    // call is served from the cache rather than the network.
    assert_eq!(client.blob_path(&blob, None).unwrap(), path);
    assert_ne!(client.blob_path(&other, None).unwrap(), path, "distinct objects, distinct paths");

    // A path is only handed out for bytes that still hash to the digest: a
    // corrupted object is removed and re-fetched, never returned.
    std::fs::write(&path, b"tampered").unwrap();
    let again = client.blob_path(&blob, Some(payload.len() as u64)).unwrap();
    assert_eq!(std::fs::read(&again).unwrap(), payload, "corruption re-fetched, not served");

    // Asking for a path does not spend the RAM budget on a file the caller
    // is about to read from disk.
    client.clear_ram_cache();
    let cold = client.blob_path(&blob, None).unwrap();
    assert_eq!(std::fs::read(cold).unwrap(), payload);
    assert_eq!(client.ram_cache_bytes().0, 0, "a path fetch stays out of RAM");
}

#[test]
fn ram_cache_evicts_under_its_budget_and_refetches_verified_after_forget() {
    // Five blobs, a budget that fits two: the client must stay inside it
    // and still answer every fetch with verified bytes.
    let mut store = FixtureStore::default();
    let payloads: Vec<Vec<u8>> = (0..5u8).map(|i| vec![i + 1; 40_000]).collect();
    let blobs: Vec<BlobId> = payloads
        .iter()
        .map(|bytes| store.add_blob(bytes.clone()))
        .collect();
    let fixture = FixtureServer::start(store, FixtureOptions::default());
    let mut cfg = config("ram_budget");
    cfg.cache.max_ram_bytes = 100_000;
    let mut client = AssetClient::connect(cfg, fixture.endpoints(), None).unwrap();

    for (blob, expect) in blobs.iter().zip(&payloads) {
        let got = client.fetch_blob_bytes(blob, Some(expect.len() as u64)).unwrap();
        assert_eq!(&got, expect, "every fetch is the real, verified payload");
        let (used, budget) = client.ram_cache_bytes();
        assert!(used <= budget, "ram cache blew its budget: {used} > {budget}");
    }
    let (used, budget) = client.ram_cache_bytes();
    assert_eq!(budget, 100_000);
    assert!(used <= budget && used > 0, "some residency, under budget: {used}");

    // Forget drops residency; the next fetch re-materialises and re-verifies
    // from the server (or the disk cache) rather than serving a ghost.
    let hot = &blobs[4];
    assert!(client.forget_blob(hot), "the newest fetch was resident");
    let (after_forget, _) = client.ram_cache_bytes();
    assert!(after_forget < used, "forget freed its bytes: {after_forget} !< {used}");
    let again = client.fetch_blob_bytes(hot, Some(payloads[4].len() as u64)).unwrap();
    assert_eq!(&again, &payloads[4], "re-fetch is verified, not a ghost");

    // Clearing empties it without breaking any later fetch.
    client.clear_ram_cache();
    assert_eq!(client.ram_cache_bytes().0, 0);
    let cold = client.fetch_blob_bytes(&blobs[0], Some(payloads[0].len() as u64)).unwrap();
    assert_eq!(&cold, &payloads[0]);
}

#[test]
fn ram_cache_budget_holds_while_lanes_fetch_together() {
    // Lane clones share one RAM cache. Eight threads pulling the same six
    // blobs must never push it past the budget.
    let mut store = FixtureStore::default();
    let payloads: Vec<Vec<u8>> = (0..6u8).map(|i| vec![i + 9; 30_000]).collect();
    let blobs: Vec<BlobId> = payloads
        .iter()
        .map(|bytes| store.add_blob(bytes.clone()))
        .collect();
    let fixture = FixtureServer::start(store, FixtureOptions::default());
    let mut cfg = config("ram_lanes");
    cfg.cache.max_ram_bytes = 90_000;
    let client = AssetClient::connect(cfg, fixture.endpoints(), None).unwrap();

    std::thread::scope(|scope| {
        for lane in 0..8 {
            let mut lane_client = client.lane_clone();
            let blobs = &blobs;
            let payloads = &payloads;
            scope.spawn(move || {
                for round in 0..6 {
                    let i = (lane + round) % blobs.len();
                    let got = lane_client
                        .fetch_blob_bytes(&blobs[i], Some(payloads[i].len() as u64))
                        .unwrap();
                    assert_eq!(got, payloads[i]);
                    let (used, budget) = lane_client.ram_cache_bytes();
                    assert!(used <= budget, "lane saw {used} > {budget}");
                }
            });
        }
    });
    let (used, budget) = client.ram_cache_bytes();
    assert!(used <= budget, "after the lanes: {used} > {budget}");
}

#[test]
fn game_manifest_and_blob_fetch() {
    let mut store = FixtureStore::default();
    let game_rev = store.add_game(90, "Fixture Kart");
    let splash_len = {
        let bytes = store.game_manifests[game_rev.as_bytes()].clone();
        let m = makepad_asset_data::GameRevisionManifest::from_canonical_bytes(&bytes).unwrap();
        m.splash_byte_len
    };
    let fixture = FixtureServer::start(store, FixtureOptions::default());
    let mut client = AssetClient::connect(config("game"), fixture.endpoints(), None).unwrap();

    let game = client.fetch_game_manifest(&game_rev).unwrap();
    assert_eq!(game.name, "Fixture Kart");
    // The splash blob materializes with its declared length verified.
    let splash = client.fetch_blob_bytes(&game.splash_blob, Some(game.splash_byte_len)).unwrap();
    assert_eq!(splash.len() as u64, splash_len);
    // Lock blob has no declared length in the manifest: fetched under the
    // budget cap, still digest-verified.
    let lock = client.fetch_blob_bytes(&game.lock_blob, None).unwrap();
    assert!(!lock.is_empty());
    // Cached game manifest round-trips.
    let again = client.fetch_game_manifest(&game_rev).unwrap();
    assert_eq!(again, game);
}

#[test]
fn complete_partial_with_unknown_length_commits_via_416() {
    // A fully downloaded partial whose expected length is unknown: the resume
    // request lands at the server's size, gets 416, and the client PROVES the
    // partial by digest locally instead of refetching anything.
    let mut store = FixtureStore::default();
    let bytes = payload(660, 50_000);
    let r = store.add_prop(96, "stock", None, "Complete Partial", bytes.clone(), vec![]);
    let blob = store.assets.iter().find(|a| a.asset_id == r.asset_id).unwrap().manifest.files[0]
        .blob;
    let fixture = FixtureServer::start(store, FixtureOptions::default());

    let root = test_root("complete_partial");
    {
        let mut cache = makepad_asset_client::ContentCache::open(
            &root,
            CacheBudgets::default_v1(),
            now_ms(),
        )
        .unwrap();
        let mut w = cache.open_partial(blob.as_bytes()).unwrap();
        w.write(&bytes).unwrap();
    }

    let mut cfg = config("unused3");
    cfg.cache_root = root;
    let mut client = AssetClient::connect(cfg, fixture.endpoints(), None).unwrap();
    let path = client.fetch_blob(&blob, None, None).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    // One request (the 416 probe), zero body bytes refetched.
    assert_eq!(fixture.log.count("GET", "/v1/blobs/"), 1);
    let probe = fixture.log.last_matching("GET", "/v1/blobs/").unwrap();
    assert_eq!(
        probe.header("range").unwrap(),
        format!("bytes={}-", bytes.len()),
        "probe must resume at the partial's end"
    );
}

#[test]
fn runtime_states_are_explicit() {
    let (store, refs) = seeded_store();
    let fixture = FixtureServer::start(store, FixtureOptions::default());
    let client = AssetClient::connect(config("runtime"), fixture.endpoints(), None).unwrap();
    let mut runtime = ClientRuntime::start(client).unwrap();

    // A search slot and a failing alias slot.
    let search_id = runtime
        .submit(ClientRequest::CatalogSearch {
            query: CatalogQuery::text("rocket", 5),
            cursor: None,
        })
        .unwrap();
    let missing_alias = AssetAlias::from_str("stock/does-not-exist").unwrap();
    let fail_id = runtime.submit(ClientRequest::ResolveAlias { alias: missing_alias }).unwrap();
    let manifest_id = runtime
        .submit(ClientRequest::FetchAssetManifest { rev: refs[0].revision })
        .unwrap();

    let mut search_slot: ResourceSlot<u64> = ResourceSlot::default();
    search_slot.begin(search_id);
    assert!(search_slot.state.is_loading());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut fail_seen = false;
    let mut manifest_seen = false;
    let mut started_order = Vec::new();
    while !(search_slot.state.is_ready() && fail_seen && manifest_seen) {
        assert!(std::time::Instant::now() < deadline, "runtime never finished");
        for event in runtime.poll() {
            if let ClientEvent::Started { id } = &event {
                started_order.push(*id);
            }
            let consumed = search_slot.on_event(&event, |out| match out {
                makepad_asset_client::ClientOutput::CatalogPage(p) => Some(p.total),
                _ => None,
            });
            if consumed {
                continue;
            }
            match event {
                ClientEvent::Failed { id, error } if id == fail_id => {
                    assert!(matches!(error, ClientError::NotFound { .. }), "{error:?}");
                    fail_seen = true;
                }
                ClientEvent::Done { id, .. } if id == manifest_id => {
                    manifest_seen = true;
                }
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(search_slot.state.ready(), Some(&10u64));
    // Lanes run requests in parallel, so completion order across requests is
    // NOT a guarantee any more (tests/runtime_lanes.rs pins what is): every
    // submitted request starts exactly once, and its own events stay ordered.
    started_order.sort_unstable();
    assert_eq!(started_order, vec![search_id, fail_id, manifest_id]);

    runtime.shutdown();
}

#[test]
fn runtime_reports_blob_progress() {
    let mut store = FixtureStore::default();
    let big = payload(999, 1_500_000);
    let r = store.add_prop(95, "stock", None, "Huge", big.clone(), vec![]);
    let fixture = FixtureServer::start(store, FixtureOptions::default());
    let mut client = AssetClient::connect(config("runtime_prog"), fixture.endpoints(), None).unwrap();
    let manifest = client.fetch_asset_manifest(&r.revision).unwrap();
    let file = manifest.files[0].clone();
    let mut runtime = ClientRuntime::start(client).unwrap();

    let id = runtime
        .submit(ClientRequest::FetchBlob {
            blob: file.blob,
            expected_len: Some(file.byte_len),
            pin: true,
        })
        .unwrap();
    let mut progress = Vec::new();
    let mut done_path = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while done_path.is_none() {
        assert!(std::time::Instant::now() < deadline, "blob fetch never finished");
        for event in runtime.poll() {
            match event {
                ClientEvent::Progress { id: eid, bytes, total } if eid == id => {
                    progress.push((bytes, total));
                }
                ClientEvent::Done { id: eid, output } if eid == id => match output {
                    makepad_asset_client::ClientOutput::Blob { path, .. } => {
                        done_path = Some(path);
                    }
                    other => panic!("wrong output: {other:?}"),
                },
                ClientEvent::Failed { id: eid, error } if eid == id => {
                    panic!("blob fetch failed: {error}");
                }
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(done_path.unwrap()).unwrap(), big);
    assert!(progress.len() >= 2, "throttled progress still reports interior points");
    assert!(progress.windows(2).all(|w| w[0].0 <= w[1].0), "progress must be monotonic");
    assert_eq!(progress.last().unwrap(), &(big.len() as u64, big.len() as u64));
    runtime.shutdown();
}

#[test]
fn runtime_blob_pin_is_transactional_and_async_unpin_is_idempotent() {
    let bytes = payload(1001, 48_000);
    let mut store = FixtureStore::default();
    let blob = store.add_blob(bytes.clone());
    let fixture = FixtureServer::start(store, FixtureOptions::default());
    let cfg = config("runtime_blob_lease");
    let cache_root = cfg.cache_root.clone();
    let client = AssetClient::connect(cfg, fixture.endpoints(), None).unwrap();
    let mut runtime = ClientRuntime::start(client).unwrap();
    let marker = pin_marker(&cache_root, &blob);

    // A refusal must not leave the old pre-fetch pin marker behind.
    let failed = runtime
        .submit(ClientRequest::FetchBlob {
            blob,
            expected_len: Some(bytes.len() as u64 + 1),
            pin: true,
        })
        .unwrap();
    assert!(
        matches!(wait_runtime(&mut runtime, failed), Err(ClientError::SizeMismatch { .. })),
        "wrong declared length must refuse"
    );
    assert!(!marker.exists(), "failed fetch leaked a durable pin marker");

    let fetched = runtime
        .submit(ClientRequest::FetchBlob {
            blob,
            expected_len: Some(bytes.len() as u64),
            pin: true,
        })
        .unwrap();
    match wait_runtime(&mut runtime, fetched).expect("verified fetch") {
        ClientOutput::Blob { blob: got, path } => {
            assert_eq!(got, blob);
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
        other => panic!("wrong fetch output: {other:?}"),
    }
    assert!(marker.exists(), "successful pinned fetch did not pin its object");

    for _ in 0..2 {
        let unpin = runtime.submit(ClientRequest::UnpinBlob { blob }).unwrap();
        match wait_runtime(&mut runtime, unpin).expect("async unpin") {
            ClientOutput::BlobUnpinned { blob: got } => assert_eq!(got, blob),
            other => panic!("wrong unpin output: {other:?}"),
        }
        assert!(!marker.exists(), "unpin left a durable pin marker");
    }
    runtime.shutdown();
}

// ---------------------------------------------------------------------------
// artifact publication (write path)
// ---------------------------------------------------------------------------

#[test]
fn publish_artifact_roundtrips_manifest_blobs_alias_and_annotation() {
    use makepad_asset_client::{PublishFile, PublishRequest, PublishThumbnail};
    use makepad_asset_data::{AssetKind, MediaType, ThumbnailMedia};

    let token = format!("mpat_{}", "5c".repeat(32));
    let fx = FixtureServer::start(
        FixtureStore::default(),
        FixtureOptions { auth_token: Some(token.clone()), ..FixtureOptions::default() },
    );
    let mut cfg = config("publish_rt");
    cfg.token = Some(token);
    let mut client = AssetClient::connect(cfg, fx.endpoints(), None).expect("connect");

    let artifact = payload(31, 5_000);
    let thumb = payload(32, 1_200);
    let mut request = PublishRequest::new(
        "gen",
        AssetKind::Video,
        "Neon drift",
        PublishFile {
            bytes: artifact.clone(),
            media: MediaType::Mp4,
            role: FileRole::Video,
            media_millis: 5_200,
            dims: None,
        },
        PublishThumbnail {
            bytes: thumb.clone(),
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
        },
    );
    request.alias = Some(AssetAlias::from_str("gen/neon-drift").unwrap());
    request.categories = vec!["music-video".into()];
    request.prompt = "a neon drift over wet asphalt".into();

    let published = client.publish_artifact(&request).expect("publish");
    assert_eq!(published.alias.as_ref().unwrap().as_str(), "gen/neon-drift");

    // The published revision fetches back, digest-verified + decoded, with
    // the playable file and the mandatory thumbnail intact.
    let manifest = client.fetch_asset_manifest(&published.revision).expect("manifest");
    assert_eq!(manifest.asset_id, published.asset_id);
    assert_eq!(manifest.kind, AssetKind::Video);
    assert_eq!(manifest.files[0].blob, published.artifact_blob);
    assert_eq!(manifest.thumbnail.as_ref().unwrap().blob, published.thumbnail_blob);

    // The artifact bytes round-trip through the verified cache path.
    let bytes = client
        .fetch_blob_bytes(&published.artifact_blob, Some(artifact.len() as u64))
        .expect("blob");
    assert_eq!(bytes, artifact);

    // The alias resolves to the published head; the detail reports it as the
    // latest published candidate.
    let alias = client.resolve_alias(request.alias.as_ref().unwrap()).expect("alias");
    assert_eq!(alias.asset_id, published.asset_id);
    assert_eq!(alias.head_revision, published.revision);
    let detail = client.asset_detail(&published.asset_id).expect("detail");
    assert_eq!(detail.latest_published().unwrap().revision, published.revision);

    // The annotation landed (title recorded by the fixture's 204 route),
    // and it landed BEFORE the publish (kind-stamped publish events).
    assert_eq!(
        fx.published.lock().unwrap().annotations.get(&published.asset_id.to_string()),
        Some(&"Neon drift".to_string())
    );
    let log = fx.log.requests.lock().unwrap();
    let idx_of = |m: &str, frag: &str| {
        log.iter()
            .position(|r| r.method == m && r.target.contains(frag))
            .unwrap_or(usize::MAX)
    };
    assert!(
        idx_of("PUT", "/annotation") < idx_of("POST", "/publish"),
        "annotation must precede publish so the publish event carries the kind"
    );
    drop(log);

    // Re-publish onto the SAME asset id (register 409 path): new revision,
    // alias head moves.
    let mut again = request.clone();
    again.asset_id = Some(published.asset_id);
    again.artifact.bytes[0] ^= 0xff;
    let second = client.publish_artifact(&again).expect("re-publish");
    assert_eq!(second.asset_id, published.asset_id);
    assert_ne!(second.revision, published.revision);
    let alias = client.resolve_alias(request.alias.as_ref().unwrap()).expect("alias 2");
    assert_eq!(alias.head_revision, second.revision);
    let detail = client.asset_detail(&published.asset_id).expect("detail 2");
    assert_eq!(detail.latest_published().unwrap().revision, second.revision);
}

// ---------------------------------------------------------------------------
// runtime cancellation, cache-root locking, session, concurrent discovery
// ---------------------------------------------------------------------------

#[test]
fn runtime_cancel_skips_queued_and_aborts_in_flight() {
    let mut store = FixtureStore::default();
    // A blob large enough to drip for a while: 512KB in 32KB/25ms chunks
    // (~400ms total) — cancellation lands mid-transfer deterministically.
    let big = payload(700, 512 * 1024);
    let big_id = store.add_blob(big.clone());
    let small = payload(701, 20_000);
    let small_id = store.add_blob(small.clone());
    let fx = FixtureServer::start(store, FixtureOptions::default());
    *fx.knobs.drip_blob.lock().unwrap() = Some((32 * 1024, 25));

    let cfg = config("cancel_rt");
    let cache_root = cfg.cache_root.clone();
    let client = AssetClient::connect(cfg, fx.endpoints(), None).unwrap();
    // This test is about the QUEUE: one worker per lane and both fetches
    // pinned to the same lane, so the second is provably still queued when
    // it is cancelled. (With the default pool it would simply run in
    // parallel — which is the point of the lanes, proven in
    // tests/runtime_lanes.rs.)
    let mut runtime = ClientRuntime::start_with(
        client,
        RuntimeConfig { fast_workers: 1, bulk_workers: 1, ..RuntimeConfig::default_v1() },
    )
    .unwrap();
    let id_big = runtime
        .submit_with(
            ClientRequest::FetchBlob {
                blob: big_id,
                expected_len: Some(big.len() as u64),
                pin: true,
            },
            SubmitOptions::bulk(),
        )
        .unwrap();
    let id_queued = runtime
        .submit_with(
            ClientRequest::FetchBlob {
                blob: small_id,
                expected_len: Some(small.len() as u64),
                pin: true,
            },
            SubmitOptions::bulk(),
        )
        .unwrap();
    // Give the worker time to start the drip transfer, then cancel BOTH:
    // the in-flight one aborts between chunks, the queued one never starts.
    std::thread::sleep(std::time::Duration::from_millis(120));
    runtime.cancel(id_big);
    runtime.cancel(id_queued);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut failures = std::collections::HashMap::new();
    while failures.len() < 2 && std::time::Instant::now() < deadline {
        for event in runtime.poll() {
            if let ClientEvent::Failed { id, error } = event {
                failures.insert(id, error);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        failures.get(&id_big),
        Some(&ClientError::Cancelled),
        "in-flight transfer must abort as Cancelled"
    );
    assert_eq!(
        failures.get(&id_queued),
        Some(&ClientError::Cancelled),
        "queued request must be skipped as Cancelled"
    );
    assert!(
        !pin_marker(&cache_root, &big_id).exists(),
        "cancelled in-flight fetch leaked a durable pin marker"
    );
    assert!(
        !pin_marker(&cache_root, &small_id).exists(),
        "cancelled queued fetch leaked a durable pin marker"
    );

    // The aborted partial stays resumable: a fresh fetch (drip off)
    // completes and verifies.
    *fx.knobs.drip_blob.lock().unwrap() = None;
    let id_retry = runtime
        .submit(ClientRequest::FetchBlob {
            blob: big_id,
            expected_len: Some(big.len() as u64),
            pin: false,
        })
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut done = false;
    while !done && std::time::Instant::now() < deadline {
        for event in runtime.poll() {
            match event {
                ClientEvent::Done { id, .. } if id == id_retry => done = true,
                ClientEvent::Failed { id, error } if id == id_retry => {
                    panic!("retry after cancel failed: {error}")
                }
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(done, "cancelled partial must resume cleanly");
}

#[test]
fn cache_root_is_process_exclusive_and_restart_clean() {
    use makepad_asset_client::{CacheBudgets, ContentCache};
    let root = test_root("cache_lock");
    let first = ContentCache::open(&root, CacheBudgets::default_v1(), 1).expect("first open");
    // A second live cache on the SAME root refuses — the single-owner
    // contract is enforced, not just documented.
    match ContentCache::open(&root, CacheBudgets::default_v1(), 2) {
        Err(ClientError::CacheBusy) => {}
        Err(other) => panic!("second owner must refuse with CacheBusy, got {other:?}"),
        Ok(_) => panic!("second owner must refuse with CacheBusy, got Ok"),
    }
    // Releasing the first owner (clean or crashed process — the OS drops
    // the lock either way) makes reopening immediate.
    drop(first);
    let again = ContentCache::open(&root, CacheBudgets::default_v1(), 3).expect("reopen");
    drop(again);
}

#[test]
fn session_connector_hands_over_and_stops_cleanly() {
    use makepad_asset_client::{SessionConfig, SessionConnector, SessionMsg, SessionStatus};

    let fx = FixtureServer::start(FixtureStore::default(), FixtureOptions::default());
    let mut cfg = SessionConfig::new(test_root("session_up"));
    cfg.endpoints = Some(fx.endpoints());
    cfg.media_lanes = vec!["video-a".into(), "video-b".into(), "audio".into()];
    cfg.retry_min_ms = 100;
    cfg.retry_max_ms = 200;
    let mut connector = SessionConnector::start(cfg).expect("start");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut handles = None;
    while handles.is_none() && std::time::Instant::now() < deadline {
        for msg in connector.poll() {
            if let SessionMsg::Up(up) = msg {
                handles = Some(*up);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let handles = handles.expect("session must come up against the fixture");
    assert_eq!(handles.media.len(), 3, "one runtime per configured lane");
    assert_eq!(handles.server_id, fx.options.server_id);
    handles.shutdown();

    // Wrong pinned identity: the connector reports honest retries and stop()
    // ends it promptly.
    let mut cfg = SessionConfig::new(test_root("session_pin"));
    cfg.endpoints = Some(fx.endpoints());
    cfg.server_id = Some([0x99; 16]);
    cfg.retry_min_ms = 100;
    cfg.retry_max_ms = 200;
    let mut connector = SessionConnector::start(cfg).expect("start 2");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut retried = false;
    while !retried && std::time::Instant::now() < deadline {
        for msg in connector.poll() {
            if let SessionMsg::Status(SessionStatus::Retrying { .. }) = msg {
                retried = true;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(retried, "identity mismatch must surface as Retrying");
    connector.stop();

    // Config validation refuses duplicate/overlapping lanes.
    let mut bad = SessionConfig::new(test_root("session_bad"));
    bad.media_lanes = vec!["a".into(), "a".into()];
    assert!(SessionConnector::start(bad).is_err());
}

#[cfg(unix)]
#[test]
fn discovery_listeners_share_the_port_on_one_host() {
    // The regression this pins: two apps on one machine must BOTH be able
    // to bind the discovery port (SO_REUSEADDR + SO_REUSEPORT). Beacons are
    // broadcast on real LANs and reach every group member; loopback unicast
    // delivery lands on at least one listener, which is asserted loosely.
    let a = DiscoveryListener::start(0, 5_000, now_ms).expect("first listener");
    let port = a.port();
    let b = DiscoveryListener::start(port, 5_000, now_ms).expect(
        "second listener on the SAME port must bind (reuse group)",
    );
    let fx = FixtureServer::start(FixtureStore::default(), FixtureOptions::default());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut seen = false;
    while !seen && std::time::Instant::now() < deadline {
        fx.send_beacon(port, false);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let now = now_ms();
        seen = !a.snapshot(now).is_empty() || !b.snapshot(now).is_empty();
    }
    assert!(seen, "a beacon must reach the reuse group");
}

// ---------------------------------------------------------------------------
// import + immutable derived-variant routes
//
// Real-process coverage of these routes belongs in the Asset Server crate
// (`libs/asset/store` e2e). This suite stays hermetic: a local TCP fixture
// that speaks the wire contract. A clean-checkout `cargo test` must not
// require a prebuilt `makepad-asset-store` binary.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImportTamper {
    None,
    SourceDigest,
    ImportRevision,
    ImportGetId,
    ImportEntryRevision,
    VariantBytes,
    VariantEtag,
    VariantSetId,
    ResolveDigest,
    SourcePageCursor,
    SourcePageOrder,
}

struct ImportPlaneState {
    write_token: String,
    read_token: String,
    server_id: [u8; 16],
    sources: HashMap<String, (SourceCollectionId, Vec<u8>)>,
    imports: HashMap<[u8; 32], (Vec<u8>, ImportReportDtoStored)>,
    variants: HashMap<[u8; 32], Vec<u8>>,
    sets: HashMap<[u8; 32], Vec<u8>>,
    tamper: ImportTamper,
}

struct ImportReportDtoStored {
    entries: Vec<(PackEntryKey, AssetId, AssetRevisionId, AssetAlias)>,
}

struct ImportPlane {
    control: RawServer,
    data: RawServer,
    state: Arc<Mutex<ImportPlaneState>>,
    write_token: String,
    read_token: String,
}

impl ImportPlane {
    fn start() -> ImportPlane {
        Self::start_with_id([0x51; 16])
    }

    fn start_with_id(server_id: [u8; 16]) -> ImportPlane {
        let write_token = format!("mpat_{}", "aa".repeat(32));
        let read_token = format!("mpat_{}", "bb".repeat(32));
        let state = Arc::new(Mutex::new(ImportPlaneState {
            write_token: write_token.clone(),
            read_token: read_token.clone(),
            server_id,
            sources: HashMap::new(),
            imports: HashMap::new(),
            variants: HashMap::new(),
            sets: HashMap::new(),
            tamper: ImportTamper::None,
        }));
        let control = {
            let state = state.clone();
            RawServer::start(Arc::new(move |req, stream| {
                import_control(&state, &req, stream);
            }))
        };
        let data = RawServer::start(Arc::new(|_req, stream| {
            write_error(stream, 404, "not found");
        }));
        ImportPlane { control, data, state, write_token, read_token }
    }

    fn endpoints(&self) -> makepad_asset_client::ApiEndpoints {
        makepad_asset_client::ApiEndpoints {
            control: self.control.addr,
            data: self.data.addr,
        }
    }

    fn set_tamper(&self, tamper: ImportTamper) {
        self.state.lock().unwrap().tamper = tamper;
    }

    fn seed_variant(&self, bytes: Vec<u8>) -> DerivedVariantId {
        let id = DerivedVariantId::hash_of(&bytes);
        self.state.lock().unwrap().variants.insert(*id.as_bytes(), bytes);
        id
    }

    fn seed_source(&self, collection: SourceCollection) {
        let bytes = collection.to_canonical_bytes().unwrap();
        let digest = SourceCollectionId::hash_of(&bytes);
        self.state
            .lock()
            .unwrap()
            .sources
            .insert(collection.id, (digest, bytes));
    }
}

fn bearer_of(req: &ParsedRequest) -> Option<&str> {
    req.header("authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
}

fn import_auth(state: &ImportPlaneState, req: &ParsedRequest) -> Result<bool, u16> {
    // `true` = write principal, `false` = authenticated reader.
    match bearer_of(req) {
        Some(t) if t == state.write_token => Ok(true),
        Some(t) if t == state.read_token => Ok(false),
        _ => Err(401),
    }
}

fn import_control(state: &Mutex<ImportPlaneState>, req: &ParsedRequest, stream: &mut TcpStream) {
    let segs = req.segs();
    let seg = |i: usize| segs.get(i).map(String::as_str).unwrap_or("");
    if req.method == "GET" && seg(0) == "v1" && seg(1) == "health" {
        let server_id = state.lock().unwrap().server_id;
        write_json_resp(
            stream,
            200,
            &obj(vec![
                ("server_id", s(common::hex(&server_id))),
                ("protocol_version", Value::Int(1)),
            ]),
        );
        return;
    }
    let write = {
        let st = state.lock().unwrap();
        match import_auth(&st, req) {
            Ok(w) => w,
            Err(401) => {
                write_error(stream, 401, "unauthenticated");
                return;
            }
            Err(_) => unreachable!(),
        }
    };
    match (req.method.as_str(), seg(0), seg(1), segs.len()) {
        ("GET", "v1", "assets", 2) => {
            write_json_resp(
                stream,
                200,
                &obj(vec![("assets", Value::Arr(Vec::new())), ("cursor", Value::Null)]),
            );
        }
        ("PUT", "v1", "import-sources", 2) => {
            if !write {
                write_json_resp(
                    stream,
                    403,
                    &obj(vec![("error", s("denied")), ("capability", s("import_source"))]),
                );
                return;
            }
            let Ok(collection) = SourceCollection::from_canonical_bytes(&req.body) else {
                write_error(stream, 400, "malformed source collection");
                return;
            };
            let digest = SourceCollectionId::hash_of(&req.body);
            let mut st = state.lock().unwrap();
            if let Some((existing, _)) = st.sources.get(&collection.id) {
                if *existing != digest {
                    write_error(stream, 409, "conflict");
                    return;
                }
            } else {
                st.sources
                    .insert(collection.id.clone(), (digest, req.body.clone()));
            }
            let echoed = if st.tamper == ImportTamper::SourceDigest {
                SourceCollectionId::from_bytes({
                    let mut b = *digest.as_bytes();
                    b[31] ^= 0xff;
                    b
                })
            } else {
                digest
            };
            write_json_resp(
                stream,
                201,
                &obj(vec![
                    ("source_id", s(collection.id)),
                    ("digest", s(echoed.to_string())),
                ]),
            );
        }
        ("GET", "v1", "import-sources", 2) => {
            let explicit =
                req.query_get("limit").is_some() || req.query_get("cursor").is_some();
            let limit = if explicit {
                match req.query_get("limit") {
                    None => 100usize,
                    Some(t) => {
                        if t.is_empty()
                            || t.len() > 6
                            || !t.bytes().all(|b| b.is_ascii_digit())
                        {
                            write_error(stream, 400, "malformed limit");
                            return;
                        }
                        let n: u64 = match t.parse() {
                            Ok(n) if n > 0 => n,
                            _ => {
                                write_error(stream, 400, "malformed limit");
                                return;
                            }
                        };
                        n.min(500) as usize
                    }
                }
            } else {
                512
            };
            if let Some(c) = req.query_get("cursor") {
                if !makepad_asset_client::wire::source_cursor_ok(&c) {
                    write_error(stream, 400, "malformed source cursor");
                    return;
                }
            }
            let after = req.query_get("cursor");
            let st = state.lock().unwrap();
            let mut ids: Vec<_> = st.sources.keys().cloned().collect();
            ids.sort();
            if st.tamper == ImportTamper::SourcePageOrder {
                ids.reverse();
            }
            let mut rows = Vec::new();
            let mut last_source_id = None;
            let mut more = false;
            for id in ids {
                if after.as_ref().is_some_and(|cursor| id.as_str() <= cursor.as_str()) {
                    continue;
                }
                if rows.len() == limit {
                    more = true;
                    break;
                }
                last_source_id = Some(id.clone());
                let (digest, bytes) = st.sources.get(&id).unwrap();
                let c = SourceCollection::from_canonical_bytes(bytes).unwrap();
                rows.push(obj(vec![
                    ("source_id", s(id)),
                    ("title", s(c.title)),
                    ("license", s(c.terms.license)),
                    ("credits", s(c.terms.credits)),
                    ("digest", s(digest.to_string())),
                ]));
            }
            if more && !explicit {
                write_error(stream, 413, "source listing requires pagination");
                return;
            }
            let cursor = match (more, last_source_id, st.tamper) {
                (true, Some(id), ImportTamper::SourcePageCursor) => s(format!("{id}-x")),
                (true, Some(id), _) => s(id),
                _ => Value::Null,
            };
            write_json_resp(
                stream,
                200,
                &obj(vec![("sources", Value::Arr(rows)), ("cursor", cursor)]),
            );
        }
        ("POST", "v1", "imports", 2) => {
            if !write {
                write_json_resp(
                    stream,
                    403,
                    &obj(vec![("error", s("denied")), ("capability", s("import_run"))]),
                );
                return;
            }
            let Ok(manifest) = ImportManifest::from_canonical_bytes(&req.body) else {
                write_error(stream, 400, "malformed import");
                return;
            };
            let irev = ImportRevisionId::hash_of(&req.body);
            let mut st = state.lock().unwrap();
            let (created, entries) = if let Some((_, stored)) = st.imports.get(irev.as_bytes()) {
                (false, stored.entries.clone())
            } else {
                let Some((registered, registered_bytes)) = st.sources.get(&manifest.source_id) else {
                    write_error(stream, 404, "not found");
                    return;
                };
                if *registered != manifest.source_collection {
                    write_error(stream, 409, "conflict");
                    return;
                }
                let registered = SourceCollection::from_canonical_bytes(registered_bytes).unwrap();
                if manifest.rights != registered.terms {
                    write_error(stream, 409, "conflict");
                    return;
                }
                let mut entries = Vec::new();
                for asset in &manifest.assets {
                    let produced = manifest.asset_manifest_for(asset, &irev).unwrap();
                    let revision = produced.revision().unwrap();
                    entries.push((
                        asset.key.clone(),
                        produced.asset_id,
                        revision,
                        manifest.alias_for(&asset.key).unwrap(),
                    ));
                }
                st.imports.insert(
                    *irev.as_bytes(),
                    (
                        req.body.clone(),
                        ImportReportDtoStored { entries: entries.clone() },
                    ),
                );
                (true, entries)
            };
            let echoed = if st.tamper == ImportTamper::ImportRevision {
                ImportRevisionId::from_bytes({
                    let mut b = *irev.as_bytes();
                    b[0] ^= 0xff;
                    b
                })
            } else {
                irev
            };
            let rows: Vec<Value> = entries
                .into_iter()
                .map(|(key, asset_id, revision, alias)| {
                    let revision = if st.tamper == ImportTamper::ImportEntryRevision {
                        AssetRevisionId::from_bytes({
                            let mut b = *revision.as_bytes();
                            b[0] ^= 0xff;
                            b
                        })
                    } else {
                        revision
                    };
                    obj(vec![
                        ("key", s(key.as_str().to_string())),
                        ("asset_id", s(asset_id.to_string())),
                        ("revision", s(revision.to_string())),
                        ("alias", s(alias.as_str().to_string())),
                    ])
                })
                .collect();
            write_json_resp(
                stream,
                if created { 201 } else { 200 },
                &obj(vec![
                    ("import_revision", s(echoed.to_string())),
                    ("created", Value::Bool(created)),
                    ("entries", Value::Arr(rows)),
                ]),
            );
        }
        ("GET", "v1", "imports", 3) => {
            let Ok(irev) = seg(2).parse::<ImportRevisionId>() else {
                write_error(stream, 400, "malformed import revision");
                return;
            };
            let st = state.lock().unwrap();
            let Some((bytes, stored)) = st.imports.get(irev.as_bytes()) else {
                write_error(stream, 404, "not found");
                return;
            };
            let manifest = ImportManifest::from_canonical_bytes(bytes).unwrap();
            let echoed = if st.tamper == ImportTamper::ImportGetId {
                ImportRevisionId::from_bytes([0xff; 32])
            } else {
                irev
            };
            let rows: Vec<Value> = stored
                .entries
                .iter()
                .map(|(key, asset_id, revision, alias)| {
                    obj(vec![
                        ("key", s(key.as_str().to_string())),
                        ("asset_id", s(asset_id.to_string())),
                        ("revision", s(revision.to_string())),
                        ("alias", s(alias.as_str().to_string())),
                    ])
                })
                .collect();
            write_json_resp(
                stream,
                200,
                &obj(vec![
                    ("import_revision", s(echoed.to_string())),
                    ("source_id", s(manifest.source_id)),
                    ("pack_name", s(manifest.pack_name)),
                    ("pack_version", s(manifest.pack_version)),
                    ("license", s(manifest.rights.license)),
                    ("credits", s(manifest.rights.credits)),
                    ("entries", Value::Arr(rows)),
                ]),
            );
        }
        ("GET", "v1", "derived-variants", 3) => {
            let Ok(id) = seg(2).parse::<DerivedVariantId>() else {
                write_error(stream, 400, "malformed derived variant");
                return;
            };
            let st = state.lock().unwrap();
            let Some(mut bytes) = st.variants.get(id.as_bytes()).cloned() else {
                write_error(stream, 404, "not found");
                return;
            };
            if st.tamper == ImportTamper::VariantBytes {
                bytes[0] ^= 0xff;
            }
            let etag = if st.tamper == ImportTamper::VariantEtag {
                format!("\"{}\"", DerivedVariantId::from_bytes([0x00; 32]))
            } else {
                format!("\"{id}\"")
            };
            write_bytes_resp(
                stream,
                200,
                "application/octet-stream",
                &bytes,
                &[("ETag", &etag), ("Cache-Control", "private, max-age=31536000, immutable")],
            );
        }
        ("POST", "v1", "variant-sets", 2) => {
            if !write {
                write_json_resp(
                    stream,
                    403,
                    &obj(vec![("error", s("denied")), ("capability", s("asset_publish"))]),
                );
                return;
            }
            let Ok(body) = makepad_asset_client::json::parse(&req.body) else {
                write_error(stream, 400, "malformed json");
                return;
            };
            let Some(base_asset) = body
                .get("base_asset")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<AssetId>().ok())
            else {
                write_error(stream, 400, "malformed asset id");
                return;
            };
            let Some(base_revision) = body
                .get("base_revision")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<AssetRevisionId>().ok())
            else {
                write_error(stream, 400, "malformed asset revision");
                return;
            };
            let Some(arr) = body.get("variants").and_then(Value::as_arr) else {
                write_error(stream, 400, "variants must be an array");
                return;
            };
            let mut variants = Vec::new();
            for v in arr {
                let Some(id) = v.as_str().and_then(|t| t.parse::<DerivedVariantId>().ok()) else {
                    write_error(stream, 400, "malformed derived variant");
                    return;
                };
                variants.push(id);
            }
            let base = AssetRevisionRef { asset_id: base_asset, revision: base_revision };
            let mut set = VariantSetManifest {
                base,
                variants: variants.clone(),
                policy_version: RESOLUTION_POLICY_V1,
            };
            set.canonicalize();
            let Ok(set_bytes) = set.to_canonical_bytes() else {
                write_error(stream, 400, "invalid variant set");
                return;
            };
            let set_id = VariantSetId::hash_of(&set_bytes);
            let mut st = state.lock().unwrap();
            for v in &set.variants {
                let Some(bytes) = st.variants.get(v.as_bytes()) else {
                    write_error(stream, 404, "not found");
                    return;
                };
                let Ok(manifest) = DerivedVariantManifest::from_canonical_bytes(bytes) else {
                    write_error(stream, 400, "malformed derived variant");
                    return;
                };
                if manifest.base != base {
                    write_error(stream, 409, "conflict");
                    return;
                }
            }
            st.sets.insert(*set_id.as_bytes(), set_bytes);
            let echoed = if st.tamper == ImportTamper::VariantSetId {
                VariantSetId::from_bytes({
                    let mut b = *set_id.as_bytes();
                    b[0] ^= 0xff;
                    b
                })
            } else {
                set_id
            };
            write_json_resp(
                stream,
                201,
                &obj(vec![("variant_set", s(echoed.to_string()))]),
            );
        }
        ("GET", "v1", "variant-sets", 3) => {
            let Ok(id) = seg(2).parse::<VariantSetId>() else {
                write_error(stream, 400, "malformed variant set");
                return;
            };
            let st = state.lock().unwrap();
            let Some(bytes) = st.sets.get(id.as_bytes()) else {
                write_error(stream, 404, "not found");
                return;
            };
            let etag = format!("\"{id}\"");
            write_bytes_resp(
                stream,
                200,
                "application/octet-stream",
                bytes,
                &[("ETag", &etag), ("Cache-Control", "private, max-age=31536000, immutable")],
            );
        }
        ("POST", "v1", "variant-resolutions", 2) => {
            let Ok(body) = makepad_asset_client::json::parse(&req.body) else {
                write_error(stream, 400, "malformed json");
                return;
            };
            let Some(set_id) = body
                .get("variant_set")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<VariantSetId>().ok())
            else {
                write_error(stream, 400, "malformed variant set");
                return;
            };
            let Some(p) = body.get("profile") else {
                write_error(stream, 400, "missing profile");
                return;
            };
            let Some(tier) = p
                .get("tier")
                .and_then(Value::as_str)
                .and_then(parse_tier_name)
            else {
                write_error(stream, 400, "malformed profile tier");
                return;
            };
            let Some(accept) = p.get("accept").and_then(Value::as_arr) else {
                write_error(stream, 400, "profile accept must be an array");
                return;
            };
            let mut profile = ClientProfile {
                policy_version: p
                    .get("policy_version")
                    .and_then(Value::as_u64)
                    .unwrap_or(RESOLUTION_POLICY_V1 as u64) as u32,
                tier,
                max_texture_dim: p.get("max_texture_dim").and_then(Value::as_u64).unwrap_or(0) as u32,
                max_triangles: p.get("max_triangles").and_then(Value::as_u64).unwrap_or(0) as u32,
                max_variant_bytes: p.get("max_variant_bytes").and_then(Value::as_u64).unwrap_or(0),
                accept_png: false,
                accept_jpeg: false,
                accept_glb: false,
                accept_bin: false,
            };
            for a in accept {
                match a.as_str() {
                    Some("png") => profile.accept_png = true,
                    Some("jpeg") => profile.accept_jpeg = true,
                    Some("glb") => profile.accept_glb = true,
                    Some("bin") => profile.accept_bin = true,
                    _ => {
                        write_error(stream, 400, "malformed profile accept entry");
                        return;
                    }
                }
            }
            let st = state.lock().unwrap();
            let Some(set_bytes) = st.sets.get(set_id.as_bytes()) else {
                write_error(stream, 404, "not found");
                return;
            };
            let set = VariantSetManifest::from_canonical_bytes(set_bytes).unwrap();
            let mut variants = Vec::new();
            for v in &set.variants {
                let Some(bytes) = st.variants.get(v.as_bytes()) else {
                    write_error(stream, 404, "not found");
                    return;
                };
                variants.push(DerivedVariantManifest::from_canonical_bytes(bytes).unwrap());
            }
            let map = resolve_variants(&set, &variants, &profile).unwrap();
            let digest = if st.tamper == ImportTamper::ResolveDigest {
                ResolvedMapDigest::from_bytes({
                    let mut b = *map.digest().unwrap().as_bytes();
                    b[0] ^= 0xff;
                    b
                })
            } else {
                map.digest().unwrap()
            };
            let entries: Vec<Value> = map
                .entries
                .iter()
                .map(|e| {
                    obj(vec![
                        (
                            "role",
                            s(makepad_asset_client::dto::variant_role_name(e.role)),
                        ),
                        ("variant", s(e.variant.to_string())),
                        (
                            "blobs",
                            Value::Arr(e.blobs.iter().map(|b| s(b.to_string())).collect()),
                        ),
                    ])
                })
                .collect();
            write_json_resp(
                stream,
                200,
                &obj(vec![
                    ("digest", s(digest.to_string())),
                    ("variant_set", s(map.set.to_string())),
                    ("profile", s(map.profile.to_string())),
                    ("entries", Value::Arr(entries)),
                ]),
            );
        }
        _ => write_error(stream, 404, "not found"),
    }
}

fn parse_tier_name(s: &str) -> Option<DeviceTier> {
    Some(match s {
        "any" => DeviceTier::Any,
        "low" => DeviceTier::Low,
        "medium" => DeviceTier::Medium,
        "high" => DeviceTier::High,
        _ => return None,
    })
}

fn fixture_rights() -> Rights {
    Rights {
        license: "CC0-1.0".into(),
        license_revision: String::new(),
        terms_digest: Some(sha256(b"CC0-1.0 legal text")),
        terms_url: "https://creativecommons.org/publicdomain/zero/1.0/".into(),
        credits: "Kenney (kenney.nl)".into(),
        source: "https://kenney.nl/assets/space-kit".into(),
        source_archive: Some(sha256(b"space-kit-1.0.zip")),
        redistribution: Redistribution::Allowed,
        derivatives: DerivativePolicy::Allowed,
    }
}

fn fixture_collection() -> SourceCollection {
    SourceCollection {
        id: "kenney".into(),
        title: "Kenney game assets".into(),
        origin: SourceOrigin::Upload,
        terms: fixture_rights(),
    }
}

fn fixture_import() -> ImportManifest {
    let glb = b"watchtower-glb";
    let preview = b"watchtower-png";
    let mut manifest = ImportManifest {
        source_collection: fixture_collection().digest().unwrap(),
        source_id: "kenney".into(),
        pack_name: "space-kit".into(),
        pack_version: "1.0".into(),
        policy_version: IMPORT_ASSET_ID_POLICY_V1,
        assets: vec![ImportAsset {
            key: "models/watchtower".parse().unwrap(),
            kind: AssetKind::Prop,
            files: vec![ImportFile {
                path: "models/watchtower.glb".into(),
                file: AssetFile {
                    role: FileRole::RenderGlb,
                    tier: DeviceTier::Any,
                    lod: 0,
                    media: MediaType::Glb,
                    blob: BlobId::hash_of(glb),
                    byte_len: glb.len() as u64,
                    dims: None,
                },
            }],
            thumbnail: Some(ImportThumbnail {
                path: "previews/watchtower.png".into(),
                meta: ThumbnailMeta {
                    blob: BlobId::hash_of(preview),
                    media: ThumbnailMedia::Png,
                    width: 512,
                    height: 512,
                    byte_len: preview.len() as u64,
                },
            }),
            metrics: Metrics {
                total_bytes: (glb.len() + preview.len()) as u64,
                triangles: 12,
                vertices: 8,
                joints: 0,
                clips: 0,
                max_texture_dim: 0,
                media_millis: 0,
            },
            coordinate_system: CoordinateSystem {
                units_per_meter: 1.0,
                up: Axis::YPos,
                forward: Axis::ZNeg,
                pivot: Pivot::Origin,
            },
            bounds: Bounds {
                min: Vec3::new(-1.0, -1.0, -1.0),
                max: Vec3::new(1.0, 1.0, 1.0),
            },
            anchors: vec![],
            capabilities: Capabilities::default(),
            spawn_recipe: None,
        }],
        rights: fixture_rights(),
    };
    manifest.canonicalize();
    manifest
}

fn fixture_tool() -> ToolClosure {
    ToolClosure {
        processor: "mp_derive".into(),
        version: "1.0".into(),
        build: "deadbeef".into(),
        deterministic: true,
    }
}

fn fixture_thumb_variant(base: AssetRevisionRef) -> DerivedVariantManifest {
    DerivedVariantManifest {
        base,
        kind: RecipeKind::MeshThumbnail,
        recipe: ProcessingRecipe {
            settings: RecipeSettings::MeshThumbnail {
                width: 512,
                height: 512,
                media: ThumbnailMedia::Png,
            },
            tool: fixture_tool(),
            output_schema: OUTPUT_SCHEMA_V1,
        }
        .digest()
        .unwrap(),
        inputs: vec![DerivedInput {
            role: FileRole::RenderGlb,
            blob: BlobId::hash_of(b"watchtower-glb"),
        }],
        outputs: vec![],
        thumbnail: Some(ThumbnailMeta {
            blob: BlobId::hash_of(b"derived-thumb"),
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
            byte_len: 450,
        }),
        metrics: Metrics {
            total_bytes: 450,
            ..Default::default()
        },
        rights: fixture_rights(),
    }
}

fn fixture_profile() -> ClientProfile {
    ClientProfile {
        policy_version: RESOLUTION_POLICY_V1,
        tier: DeviceTier::High,
        max_texture_dim: 2048,
        max_triangles: 1_000_000,
        max_variant_bytes: 64 * 1024 * 1024,
        accept_png: true,
        accept_jpeg: true,
        accept_glb: true,
        accept_bin: true,
    }
}

fn import_config(name: &str, token: &str) -> ClientConfig {
    let mut cfg = config(name);
    cfg.token = Some(token.to_string());
    cfg
}

fn connect_import(name: &str, plane: &ImportPlane, token: &str) -> AssetClient {
    AssetClient::connect(import_config(name, token), plane.endpoints(), None).unwrap()
}

#[test]
fn import_source_register_list_and_idempotent_retry() {
    let plane = ImportPlane::start();
    let client = connect_import("src_reg", &plane, &plane.write_token);
    let bytes = fixture_collection().to_canonical_bytes().unwrap();
    let expected = SourceCollectionId::hash_of(&bytes);
    let first = client.register_source_collection(&bytes).unwrap();
    assert_eq!(first.source_id, "kenney");
    assert_eq!(first.digest, expected);
    let retry = client.register_source_collection(&bytes).unwrap();
    assert_eq!(retry, first);
    let listed = client.list_source_collections().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].source_id, "kenney");
    assert_eq!(listed[0].digest, expected);
    assert_eq!(listed[0].license, "CC0-1.0");
}

#[test]
fn import_run_status_and_idempotent_retry() {
    let plane = ImportPlane::start();
    let client = connect_import("imp_run", &plane, &plane.write_token);
    let collection = fixture_collection().to_canonical_bytes().unwrap();
    client.register_source_collection(&collection).unwrap();
    let manifest = fixture_import();
    let bytes = manifest.to_canonical_bytes().unwrap();
    let expected = ImportRevisionId::hash_of(&bytes);
    let first = client.run_import(&bytes).unwrap();
    assert!(first.created);
    assert_eq!(first.import_revision, expected);
    assert_eq!(first.entries.len(), 1);
    assert_eq!(first.entries[0].key.as_str(), "models/watchtower");
    assert_eq!(first.entries[0].asset_id, manifest.asset_id_for(&manifest.assets[0].key));
    let retry = client.run_import(&bytes).unwrap();
    assert!(!retry.created);
    assert_eq!(retry.import_revision, first.import_revision);
    assert_eq!(retry.entries, first.entries);
    let status = client.import_status(&expected).unwrap();
    assert_eq!(status.import_revision, expected);
    assert_eq!(status.source_id, "kenney");
    assert_eq!(status.pack_name, "space-kit");
    assert_eq!(status.entries[0].asset_id, first.entries[0].asset_id);
}

#[test]
fn derived_variant_and_variant_set_identities() {
    let plane = ImportPlane::start();
    let mut client = connect_import("dvar", &plane, &plane.write_token);
    let base = AssetRevisionRef {
        asset_id: AssetId::from_bytes([0x10; 16]),
        revision: AssetRevisionId::from_bytes([0x20; 32]),
    };
    let variant = fixture_thumb_variant(base);
    let variant_bytes = variant.to_canonical_bytes().unwrap();
    let variant_id = plane.seed_variant(variant_bytes);
    assert_eq!(variant_id, variant.id().unwrap());
    let fetched = client.fetch_derived_variant(&variant_id).unwrap();
    assert_eq!(fetched, variant);
    let set_id = client.freeze_variant_set(&base, &[variant_id]).unwrap();
    let retry = client.freeze_variant_set(&base, &[variant_id]).unwrap();
    assert_eq!(retry, set_id);
    let set = client.fetch_variant_set(&set_id).unwrap();
    assert_eq!(set.base, base);
    assert_eq!(set.variants, vec![variant_id]);
    let map = client.resolve_variant_set(&set_id, &fixture_profile()).unwrap();
    assert_eq!(map.set, set_id);
    assert_eq!(map.profile, fixture_profile().digest().unwrap());
    assert!(!map.entries.is_empty());
}

#[test]
fn import_derived_authorization() {
    let plane = ImportPlane::start();
    let collection = fixture_collection().to_canonical_bytes().unwrap();
    let import_bytes = fixture_import().to_canonical_bytes().unwrap();
    let writer = connect_import("auth_w", &plane, &plane.write_token);
    writer.register_source_collection(&collection).unwrap();
    writer.run_import(&import_bytes).unwrap();

    let reader = connect_import("auth_r", &plane, &plane.read_token);
    assert!(reader.list_source_collections().is_ok());
    let irev = ImportRevisionId::hash_of(&import_bytes);
    assert!(reader.import_status(&irev).is_ok());
    match reader.register_source_collection(&collection) {
        Err(ClientError::Denied) => {}
        other => panic!("register must deny reader, got {other:?}"),
    }
    match reader.run_import(&import_bytes) {
        Err(ClientError::Denied) => {}
        other => panic!("import run must deny reader, got {other:?}"),
    }
    let base = AssetRevisionRef {
        asset_id: AssetId::from_bytes([0x10; 16]),
        revision: AssetRevisionId::from_bytes([0x20; 32]),
    };
    match reader.freeze_variant_set(&base, &[DerivedVariantId::from_bytes([1; 32])]) {
        Err(ClientError::Denied) => {}
        other => panic!("freeze must deny reader, got {other:?}"),
    }

    let anon = makepad_asset_client::Api::new(
        plane.endpoints(),
        fast_limits(),
        None,
    )
    .unwrap();
    match anon.list_source_collections() {
        Err(ClientError::Unauthenticated) => {}
        other => panic!("anonymous list must 401, got {other:?}"),
    }
    match anon.run_import(&import_bytes) {
        Err(ClientError::Unauthenticated) => {}
        other => panic!("anonymous import must 401, got {other:?}"),
    }
}

#[test]
fn import_derived_tampered_identities_fail_closed() {
    let plane = ImportPlane::start();
    let client = connect_import("tamper", &plane, &plane.write_token);
    let collection = fixture_collection().to_canonical_bytes().unwrap();
    let import_bytes = fixture_import().to_canonical_bytes().unwrap();
    let base = AssetRevisionRef {
        asset_id: AssetId::from_bytes([0x10; 16]),
        revision: AssetRevisionId::from_bytes([0x20; 32]),
    };
    let variant = fixture_thumb_variant(base);
    let variant_id = plane.seed_variant(variant.to_canonical_bytes().unwrap());

    plane.set_tamper(ImportTamper::SourceDigest);
    match client.register_source_collection(&collection) {
        Err(ClientError::DigestMismatch { what, .. }) => {
            assert_eq!(what, "source collection digest")
        }
        other => panic!("tampered source digest must refuse, got {other:?}"),
    }
    plane.set_tamper(ImportTamper::None);
    client.register_source_collection(&collection).unwrap();

    plane.set_tamper(ImportTamper::ImportRevision);
    match client.run_import(&import_bytes) {
        Err(ClientError::DigestMismatch { what, .. }) => assert_eq!(what, "import revision"),
        other => panic!("tampered import revision must refuse, got {other:?}"),
    }
    plane.set_tamper(ImportTamper::ImportEntryRevision);
    match client.run_import(&import_bytes) {
        Err(ClientError::DigestMismatch { what, .. }) => {
            assert_eq!(what, "import entry revision")
        }
        other => panic!("tampered entry revision must refuse, got {other:?}"),
    }
    plane.set_tamper(ImportTamper::None);
    let report = client.run_import(&import_bytes).unwrap();

    plane.set_tamper(ImportTamper::ImportGetId);
    match client.import_status(&report.import_revision) {
        Err(ClientError::Protocol { what }) => assert_eq!(what, "import status id mismatch"),
        other => panic!("tampered import get id must refuse, got {other:?}"),
    }
    plane.set_tamper(ImportTamper::None);

    let mut fetch_client = connect_import("tamper_fetch", &plane, &plane.write_token);
    plane.set_tamper(ImportTamper::VariantBytes);
    match fetch_client.fetch_derived_variant(&variant_id) {
        Err(ClientError::DigestMismatch { what, .. }) => {
            assert_eq!(what, "derived variant bytes")
        }
        other => panic!("tampered variant bytes must refuse, got {other:?}"),
    }
    plane.set_tamper(ImportTamper::VariantEtag);
    match fetch_client.fetch_derived_variant(&variant_id) {
        Err(ClientError::Protocol { what }) => assert_eq!(what, "canonical etag mismatch"),
        other => panic!("tampered variant etag must refuse, got {other:?}"),
    }
    plane.set_tamper(ImportTamper::VariantSetId);
    match client.freeze_variant_set(&base, &[variant_id]) {
        Err(ClientError::DigestMismatch { what, .. }) => {
            assert_eq!(what, "variant set identity")
        }
        other => panic!("tampered variant set id must refuse, got {other:?}"),
    }
    plane.set_tamper(ImportTamper::None);
    let set_id = client.freeze_variant_set(&base, &[variant_id]).unwrap();
    plane.set_tamper(ImportTamper::ResolveDigest);
    match client.resolve_variant_set(&set_id, &fixture_profile()) {
        Err(ClientError::DigestMismatch { what, .. }) => {
            assert_eq!(what, "resolution map digest")
        }
        other => panic!("tampered resolve digest must refuse, got {other:?}"),
    }
}

#[test]
fn import_derived_runtime_request_output_mapping() {
    let plane = ImportPlane::start();
    let client = connect_import("rt_map", &plane, &plane.write_token);
    let collection = fixture_collection().to_canonical_bytes().unwrap();
    let import_bytes = fixture_import().to_canonical_bytes().unwrap();
    let base = AssetRevisionRef {
        asset_id: AssetId::from_bytes([0x10; 16]),
        revision: AssetRevisionId::from_bytes([0x20; 32]),
    };
    let variant = fixture_thumb_variant(base);
    let variant_id = plane.seed_variant(variant.to_canonical_bytes().unwrap());

    let mut runtime = ClientRuntime::start(client).unwrap();
    let id_reg = runtime
        .submit(ClientRequest::RegisterSourceCollection {
            bytes: collection.clone(),
        })
        .unwrap();
    match wait_runtime(&mut runtime, id_reg).unwrap() {
        ClientOutput::SourceCollectionRegistered(SourceCollectionRegistered { source_id, .. }) => {
            assert_eq!(source_id, "kenney")
        }
        other => panic!("unexpected register output {other:?}"),
    }
    let id_list = runtime.submit(ClientRequest::ListSourceCollections).unwrap();
    match wait_runtime(&mut runtime, id_list).unwrap() {
        ClientOutput::SourceCollections(rows) => assert_eq!(rows.len(), 1),
        other => panic!("unexpected list output {other:?}"),
    }
    let id_run = runtime
        .submit(ClientRequest::RunImport {
            bytes: import_bytes.clone(),
        })
        .unwrap();
    let report = match wait_runtime(&mut runtime, id_run).unwrap() {
        ClientOutput::ImportReport(r) => r,
        other => panic!("unexpected import output {other:?}"),
    };
    let id_status = runtime
        .submit(ClientRequest::FetchImport {
            revision: report.import_revision,
        })
        .unwrap();
    match wait_runtime(&mut runtime, id_status).unwrap() {
        ClientOutput::ImportStatus(s) => assert_eq!(s.import_revision, report.import_revision),
        other => panic!("unexpected status output {other:?}"),
    }
    let id_var = runtime
        .submit(ClientRequest::FetchDerivedVariant { id: variant_id })
        .unwrap();
    match wait_runtime(&mut runtime, id_var).unwrap() {
        ClientOutput::DerivedVariant(v) => assert_eq!(v.id().unwrap(), variant_id),
        other => panic!("unexpected derived output {other:?}"),
    }
    let id_freeze = runtime
        .submit(ClientRequest::FreezeVariantSet {
            base,
            variants: vec![variant_id],
        })
        .unwrap();
    let set_id = match wait_runtime(&mut runtime, id_freeze).unwrap() {
        ClientOutput::VariantSetFrozen(id) => id,
        other => panic!("unexpected freeze output {other:?}"),
    };
    let id_set = runtime
        .submit(ClientRequest::FetchVariantSet { id: set_id })
        .unwrap();
    match wait_runtime(&mut runtime, id_set).unwrap() {
        ClientOutput::VariantSet(set) => assert_eq!(set.id().unwrap(), set_id),
        other => panic!("unexpected set output {other:?}"),
    }
    let id_res = runtime
        .submit(ClientRequest::ResolveVariantSet {
            set: set_id,
            profile: fixture_profile(),
        })
        .unwrap();
    match wait_runtime(&mut runtime, id_res).unwrap() {
        ClientOutput::ResolvedVariants(map) => assert_eq!(map.set, set_id),
        other => panic!("unexpected resolve output {other:?}"),
    }
    runtime.shutdown();
}

fn seed_n_sources(plane: &ImportPlane, n: usize) {
    for i in 0..n {
        plane.seed_source(SourceCollection {
            id: format!("s{i:03}"),
            title: format!("Source {i:03}"),
            origin: SourceOrigin::Upload,
            terms: fixture_rights(),
        });
    }
}

#[test]
fn source_collections_page_cursor_order_and_limit() {
    let plane = ImportPlane::start();
    seed_n_sources(&plane, 5);
    let client = connect_import("src_page", &plane, &plane.write_token);

    match client.source_collections_page(None, 0) {
        Err(ClientError::InvalidInput { what }) => assert_eq!(what, "source page limit"),
        other => panic!("limit 0 must be local, got {other:?}"),
    }
    match client.source_collections_page(None, 501) {
        Err(ClientError::InvalidInput { what }) => assert_eq!(what, "source page limit"),
        other => panic!("limit 501 must be local, got {other:?}"),
    }

    let first = client.source_collections_page(None, 2).unwrap();
    assert_eq!(
        first.sources.iter().map(|r| r.source_id.as_str()).collect::<Vec<_>>(),
        ["s000", "s001"]
    );
    assert_eq!(first.next.as_ref().map(|c| c.server_id()), Some(client.server_id()).as_ref());
    let second = client
        .source_collections_page(first.next.as_ref(), 2)
        .unwrap();
    assert_eq!(
        second.sources.iter().map(|r| r.source_id.as_str()).collect::<Vec<_>>(),
        ["s002", "s003"]
    );
    let last = client
        .source_collections_page(second.next.as_ref(), 2)
        .unwrap();
    assert_eq!(
        last.sources.iter().map(|r| r.source_id.as_str()).collect::<Vec<_>>(),
        ["s004"]
    );
    assert!(last.next.is_none());

    let all = client.list_source_collections().unwrap();
    assert_eq!(all.len(), 5);
    let mut sorted = all.clone();
    sorted.sort_by(|a, b| a.source_id.cmp(&b.source_id));
    assert_eq!(all, sorted);
}

#[test]
fn source_collections_page_tamper_and_legacy_aggregate() {
    let plane = ImportPlane::start();
    seed_n_sources(&plane, 5);
    let client = connect_import("src_tamper", &plane, &plane.write_token);

    plane.set_tamper(ImportTamper::SourcePageOrder);
    match client.source_collections_page(None, 3) {
        Err(ClientError::Protocol { what }) => assert_eq!(what, "source collection order"),
        other => panic!("reversed page must refuse, got {other:?}"),
    }
    plane.set_tamper(ImportTamper::SourcePageCursor);
    match client.source_collections_page(None, 2) {
        Err(ClientError::Protocol { what }) => {
            assert!(what == "source page cursor" || what == "source page cursor mismatch")
        }
        other => panic!("tampered cursor must refuse, got {other:?}"),
    }
    plane.set_tamper(ImportTamper::None);

    seed_n_sources(&plane, 513);
    match client.list_source_collections() {
        Err(ClientError::OverBudget { what, limit, found }) => {
            assert_eq!(what, "source collections");
            assert_eq!(limit, 512);
            assert!(found > 512);
        }
        other => panic!("513 sources must fail closed, not partial, got {other:?}"),
    }
    let exact = ImportPlane::start();
    seed_n_sources(&exact, 512);
    let client512 = connect_import("src_512", &exact, &exact.write_token);
    assert_eq!(client512.list_source_collections().unwrap().len(), 512);
}

#[test]
fn source_collections_cursor_refuses_another_server() {
    let a = ImportPlane::start_with_id([0x11; 16]);
    let b = ImportPlane::start_with_id([0x22; 16]);
    seed_n_sources(&a, 3);
    seed_n_sources(&b, 3);
    let ca = connect_import("src_srv_a", &a, &a.write_token);
    let cb = connect_import("src_srv_b", &b, &b.write_token);
    assert_ne!(ca.server_id(), cb.server_id());
    let page = ca.source_collections_page(None, 2).unwrap();
    let cursor = page.next.expect("continuation cursor");
    assert_eq!(cursor.server_id(), &ca.server_id());
    match cb.source_collections_page(Some(&cursor), 2) {
        Err(ClientError::WrongServerCursor) => {}
        other => panic!("foreign source cursor must refuse, got {other:?}"),
    }
}

#[test]
fn source_page_over_ceiling_is_refused() {
    let cap = makepad_asset_client::wire::MAX_SOURCE_PAGE_JSON_RESPONSE_BYTES;
    let server = RawServer::start(Arc::new(move |_req, stream| {
        let head = response_head(200, "application/json", cap + 1, &[]);
        write_raw(stream, head.as_bytes());
    }));
    let api = makepad_asset_client::Api::new(
        makepad_asset_client::ApiEndpoints {
            control: server.addr,
            data: server.addr,
        },
        fast_limits(),
        None,
    )
    .unwrap();
    match api.source_collections_page(None, 1) {
        Err(ClientError::OverBudget { what, limit, found }) => {
            assert_eq!(what, "json response body");
            assert_eq!(limit, cap);
            assert_eq!(found, cap + 1);
        }
        other => panic!("over-ceiling source page must refuse, got {other:?}"),
    }
}
