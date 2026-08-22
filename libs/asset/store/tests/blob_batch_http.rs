//! Keep-alive and the ordered batch pull, over real sockets.
//!
//! The thing being proven is what a thumbnail grid feels: many small blobs
//! should cost ONE connection and ONE round trip, arrive in the order the UI
//! asked for, and be abandonable the moment the user scrolls somewhere else
//! without throwing away what already landed.

use makepad_asset_client::{
    Api, ApiEndpoints, AssetClient, BatchFlow, BatchFrame, BatchItem, ClientConfig, ClientError,
    ClientRequest, ClientRuntime, HttpLimits, RuntimeConfig, SubmitOptions,
};
use makepad_asset_data::BlobId;
use makepad_asset_store::{AssetServer, ServerConfig};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn test_root(name: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("mp_asset_batch_{}_{}_{}", std::process::id(), n, name))
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

fn config(name: &str) -> ClientConfig {
    let mut cfg = ClientConfig::new(test_root(name));
    cfg.http = HttpLimits {
        connect_timeout_ms: 2_000,
        read_timeout_ms: 5_000,
        write_timeout_ms: 5_000,
        head_deadline_ms: 5_000,
        body_deadline_ms: 20_000,
    };
    cfg.blob_body_deadline_ms = 20_000;
    cfg
}

/// A bare API handle on the same server: the batch route is exercised
/// directly here, without the cache layer in the way.
fn api(server: &AssetServer, token: &str) -> Api {
    Api::new(
        ApiEndpoints { control: server.control_addr(), data: server.data_addr() },
        HttpLimits::default_v1(),
        Some(token.to_string()),
    )
    .expect("api")
}

fn connect(server: &AssetServer, token: &str, cache: &str) -> AssetClient {
    let mut cfg = config(cache);
    cfg.token = Some(token.to_string());
    let endpoints = ApiEndpoints { control: server.control_addr(), data: server.data_addr() };
    AssetClient::connect(cfg, endpoints, Some(server.server_id())).expect("connect")
}

/// Upload `count` distinct small blobs (thumbnail-sized) and return them in
/// upload order.
fn seed_blobs(client: &mut AssetClient, count: usize, len: usize) -> Vec<(BlobId, Vec<u8>)> {
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let bytes: Vec<u8> = (0..len).map(|b| ((b + i * 7) % 251) as u8).collect();
        let blob = client.upload_blob("gen", &bytes).expect("upload");
        assert_eq!(blob, BlobId::hash_of(&bytes));
        out.push((blob, bytes));
    }
    out
}

#[test]
fn keep_alive_serves_many_fetches_over_one_connection() {
    let (mut server, token) = start_server("keepalive");
    let mut client = connect(&server, &token, "keepalive_cache");
    let blobs = seed_blobs(&mut client, 20, 3_000);

    let uploads = server.data_connections_accepted();
    // Twenty single GETs on one client handle: with keep-alive that is ONE
    // more accepted connection, not twenty.
    let mut fresh = connect(&server, &token, "keepalive_cache2");
    let before = server.data_connections_accepted();
    for (blob, bytes) in &blobs {
        let got = fresh.fetch_blob_bytes(blob, Some(bytes.len() as u64)).expect("fetch");
        assert_eq!(&got, bytes);
    }
    let opened = server.data_connections_accepted() - before;
    assert_eq!(opened, 1, "20 fetches opened {opened} data connections (uploads: {uploads})");

    server.shutdown();
}

#[test]
fn a_pooled_socket_the_server_closed_is_retried_transparently() {
    // A server that hangs up on idle connections is the normal case (every
    // keep-alive server does); the client must not surface that as an error.
    let root = test_root("keepalive_stale");
    let mut cfg = ServerConfig::new(root.clone());
    cfg.control_addr = "127.0.0.1:0".parse().unwrap();
    cfg.data_addr = "127.0.0.1:0".parse().unwrap();
    cfg.bootstrap_admin = true;
    cfg.log = false;
    cfg.keepalive_idle_ms = 150;
    // Also prove the request-count rollover: after this many requests the
    // server closes the connection itself and the client reconnects.
    cfg.max_requests_per_conn = 8;
    let mut server = AssetServer::start(cfg).expect("server start");
    let token = std::fs::read_to_string(root.join("admin-token"))
        .expect("admin token")
        .trim()
        .to_string();

    let mut client = connect(&server, &token, "keepalive_stale_cache");
    let blobs = seed_blobs(&mut client, 24, 1_000);
    let mut reader = connect(&server, &token, "keepalive_stale_reader");
    // More requests than one connection is allowed to serve.
    for (blob, bytes) in &blobs {
        let got = reader.fetch_blob_bytes(blob, Some(bytes.len() as u64)).expect("fetch");
        assert_eq!(&got, bytes);
    }
    // Let the pooled socket go stale, then use it again.
    std::thread::sleep(Duration::from_millis(400));
    let (blob, bytes) = &blobs[0];
    let got = reader.fetch_blob_bytes(blob, Some(bytes.len() as u64)).expect("fetch after idle");
    assert_eq!(&got, bytes);

    server.shutdown();
}

#[test]
fn batch_frames_arrive_in_the_requested_order() {
    let (mut server, token) = start_server("batch_order");
    let mut client = connect(&server, &token, "batch_order_cache");
    let blobs = seed_blobs(&mut client, 6, 2_048);

    // Ask in a deliberately scrambled order: the response must follow it.
    let order = [4usize, 0, 5, 1, 3, 2];
    let items: Vec<BatchItem> = order
        .iter()
        .map(|i| BatchItem { blob: blobs[*i].0, max_bytes: Some(64 * 1024) })
        .collect();
    let mut seen: Vec<(BlobId, usize)> = Vec::new();
    api(&server, &token)
        .fetch_blob_batch(&items, 10_000, &mut |blob, frame, bytes| {
            assert_eq!(frame, BatchFrame::Ok);
            seen.push((blob, bytes.len()));
            BatchFlow::Continue
        })
        .expect("batch");
    let got: Vec<BlobId> = seen.iter().map(|(b, _)| *b).collect();
    let want: Vec<BlobId> = order.iter().map(|i| blobs[*i].0).collect();
    assert_eq!(got, want, "frames must follow the requested order");
    assert!(seen.iter().all(|(_, len)| *len == 2_048));

    server.shutdown();
}

#[test]
fn batch_reports_missing_and_over_cap_items_without_dropping_the_rest() {
    let (mut server, token) = start_server("batch_refusals");
    let mut client = connect(&server, &token, "batch_refusals_cache");
    let blobs = seed_blobs(&mut client, 2, 4_000);
    let absent = BlobId::hash_of(b"never uploaded");

    let items = vec![
        BatchItem { blob: blobs[0].0, max_bytes: Some(64 * 1024) },
        BatchItem { blob: absent, max_bytes: Some(64 * 1024) },
        // A cap below the real size: refused, not truncated.
        BatchItem { blob: blobs[1].0, max_bytes: Some(100) },
    ];
    let mut frames: Vec<(BlobId, BatchFrame, usize)> = Vec::new();
    api(&server, &token)
        .fetch_blob_batch(&items, 10_000, &mut |blob, frame, bytes| {
            frames.push((blob, frame, bytes.len()));
            BatchFlow::Continue
        })
        .expect("batch");
    assert_eq!(frames.len(), 3);
    assert_eq!((frames[0].1, frames[0].2), (BatchFrame::Ok, 4_000));
    assert_eq!((frames[1].1, frames[1].2), (BatchFrame::Missing, 0));
    assert_eq!((frames[2].1, frames[2].2), (BatchFrame::OverItemCap, 0));

    server.shutdown();
}

#[test]
fn mid_stream_abort_keeps_what_landed_and_a_reissue_gets_the_rest() {
    let (mut server, token) = start_server("batch_abort");
    let mut client = connect(&server, &token, "batch_abort_cache");
    let blobs = seed_blobs(&mut client, 6, 8_000);
    // A second client so the cache under test starts empty.
    let mut reader = connect(&server, &token, "batch_abort_reader");

    // Abort after the second item: the UI scrolled somewhere else.
    let wanted: Vec<(BlobId, Option<u64>)> =
        blobs.iter().map(|(b, v)| (*b, Some(v.len() as u64))).collect();
    let mut delivered: Vec<BlobId> = Vec::new();
    let cancel_from = 2usize;
    let cancelled: Vec<BlobId> = blobs[cancel_from..].iter().map(|(b, _)| *b).collect();
    reader
        .fetch_blobs_ordered(
            &wanted,
            &|blob| cancelled.contains(blob),
            &mut |blob, outcome| {
                if outcome.is_ok() {
                    delivered.push(blob);
                }
            },
        )
        .expect("batch");
    assert_eq!(delivered.len(), cancel_from, "only the uncancelled prefix lands");

    // Everything delivered is committed and verified on disk; the rest is
    // simply absent — no half-written objects.
    for (i, (blob, bytes)) in blobs.iter().enumerate() {
        let cached = reader.cached_blob(blob).expect("cache read");
        if i < cancel_from {
            let path = cached.unwrap_or_else(|| panic!("item {i} must be cached"));
            assert_eq!(&std::fs::read(path).unwrap(), bytes);
        } else {
            assert!(cached.is_none(), "cancelled item {i} must not be cached");
        }
    }

    // Re-issue with the NEW priority order; the already-cached prefix costs
    // no bytes, and the rest arrives.
    let reissue: Vec<(BlobId, Option<u64>)> = blobs
        .iter()
        .rev()
        .map(|(b, v)| (*b, Some(v.len() as u64)))
        .collect();
    let mut second: Vec<BlobId> = Vec::new();
    reader
        .fetch_blobs_ordered(&reissue, &|_| false, &mut |blob, outcome| {
            outcome.expect("reissued item");
            second.push(blob);
        })
        .expect("reissue");
    assert_eq!(second.len(), blobs.len());
    for (blob, bytes) in &blobs {
        let path = reader.cached_blob(blob).expect("cache").expect("cached after reissue");
        assert_eq!(&std::fs::read(path).unwrap(), bytes);
    }

    server.shutdown();
}

#[test]
fn a_batch_and_a_single_get_can_race_on_one_digest() {
    let (mut server, token) = start_server("batch_race");
    let mut client = connect(&server, &token, "batch_race_cache");
    let blobs = seed_blobs(&mut client, 4, 200_000);
    let shared = blobs[1].0;

    // Two lane handles on ONE cache, exactly as the runtime builds them.
    let mut reader = connect(&server, &token, "batch_race_reader");
    let mut lane = reader.lane_clone();
    let expected = blobs[1].1.clone();
    let single = std::thread::spawn(move || {
        lane.fetch_blob(&shared, Some(expected.len() as u64), None)
            .expect("single get")
    });
    let wanted: Vec<(BlobId, Option<u64>)> =
        blobs.iter().map(|(b, v)| (*b, Some(v.len() as u64))).collect();
    let mut ok = 0usize;
    reader
        .fetch_blobs_ordered(&wanted, &|_| false, &mut |_, outcome| {
            if outcome.is_ok() {
                ok += 1;
            }
        })
        .expect("batch");
    let single_path = single.join().expect("single thread");
    assert_eq!(ok, blobs.len());
    assert_eq!(std::fs::read(single_path).unwrap(), blobs[1].1);
    // Every blob verifies from the shared cache, including the contested one.
    for (blob, bytes) in &blobs {
        let path = reader.cached_blob(blob).expect("cache").expect("cached");
        assert_eq!(&std::fs::read(path).unwrap(), bytes);
    }

    server.shutdown();
}

#[test]
fn the_runtime_fast_lane_coalesces_queued_thumb_fetches() {
    let (mut server, token) = start_server("batch_runtime");
    let mut seeder = connect(&server, &token, "batch_runtime_seed");
    let blobs = seed_blobs(&mut seeder, 24, 6_000);
    let client = connect(&server, &token, "batch_runtime_cache");
    // One fast worker so the coalescing is deterministic: everything queued
    // behind the first request rides with it.
    let mut runtime = ClientRuntime::start_with(
        client,
        RuntimeConfig { fast_workers: 1, bulk_workers: 1, ..RuntimeConfig::default_v1() },
    )
    .expect("runtime");

    let before_conns = server.data_connections_accepted();
    let before_reqs = server.data_requests_served();
    let mut ids = Vec::new();
    for (blob, bytes) in &blobs {
        ids.push(
            runtime
                .submit_with(
                    ClientRequest::FetchBlob {
                        blob: *blob,
                        expected_len: Some(bytes.len() as u64),
                        pin: false,
                    },
                    SubmitOptions::fast(),
                )
                .expect("submit"),
        );
    }

    let mut done: HashMap<u64, PathBuf> = HashMap::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while done.len() < ids.len() {
        assert!(Instant::now() < deadline, "runtime never finished: {}/{}", done.len(), ids.len());
        for event in runtime.poll() {
            match event {
                makepad_asset_client::ClientEvent::Done { id, output } => {
                    let makepad_asset_client::ClientOutput::Blob { path, .. } = output else {
                        panic!("wrong output");
                    };
                    done.insert(id, path);
                }
                makepad_asset_client::ClientEvent::Failed { id, error } => {
                    panic!("request {id} failed: {error}");
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(std::fs::read(&done[id]).unwrap(), blobs[i].1);
    }
    // 24 thumbnails: a couple of batched requests over one keep-alive
    // connection — not 24 requests, and not 24 connections.
    let opened = server.data_connections_accepted() - before_conns;
    let requests = server.data_requests_served() - before_reqs;
    assert!(opened <= 2, "24 coalesced fetches opened {opened} connections");
    assert!(
        requests <= 4,
        "24 coalesced fetches cost {requests} requests (batching did not happen)"
    );

    runtime.shutdown();
    server.shutdown();
}

#[test]
fn cancelling_the_whole_queue_mid_batch_reports_every_item() {
    let (mut server, token) = start_server("batch_cancel");
    let mut seeder = connect(&server, &token, "batch_cancel_seed");
    let blobs = seed_blobs(&mut seeder, 8, 120_000);
    let client = connect(&server, &token, "batch_cancel_cache");
    let mut runtime = ClientRuntime::start_with(
        client,
        RuntimeConfig { fast_workers: 1, bulk_workers: 1, ..RuntimeConfig::default_v1() },
    )
    .expect("runtime");

    let mut ids = Vec::new();
    for (blob, bytes) in &blobs {
        ids.push(
            runtime
                .submit_with(
                    ClientRequest::FetchBlob {
                        blob: *blob,
                        expected_len: Some(bytes.len() as u64),
                        pin: false,
                    },
                    SubmitOptions::fast(),
                )
                .expect("submit"),
        );
    }
    // Cancel the tail immediately: some may already be in flight, some still
    // queued — both must end as Cancelled, and none may go silent.
    for id in &ids[4..] {
        runtime.cancel(*id);
    }

    let mut terminal: HashMap<u64, bool> = HashMap::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while terminal.len() < ids.len() {
        assert!(Instant::now() < deadline, "runtime never finished: {:?}", terminal.len());
        for event in runtime.poll() {
            match event {
                makepad_asset_client::ClientEvent::Done { id, .. } => {
                    terminal.insert(id, true);
                }
                makepad_asset_client::ClientEvent::Failed { id, error } => {
                    assert!(
                        matches!(error, ClientError::Cancelled),
                        "unexpected failure for {id}: {error}"
                    );
                    terminal.insert(id, false);
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // Every request ended exactly once, and the uncancelled head succeeded.
    assert_eq!(terminal.len(), ids.len());
    for id in &ids[..4] {
        assert_eq!(terminal.get(id), Some(&true), "uncancelled request {id} must succeed");
    }

    runtime.shutdown();
    server.shutdown();
}
