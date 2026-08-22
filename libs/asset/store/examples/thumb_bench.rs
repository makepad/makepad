//! What a thumbnail grid costs, measured three ways against a real server.
//!
//! Serves a COPY of a server root on ephemeral ports, picks N thumbnails out
//! of the catalog, and fetches them with a cold client cache under three
//! transport configurations:
//!
//!   1. `connect-per-request`  — one TCP connect + request + close per blob
//!      (what this client did before keep-alive existed)
//!   2. `keep-alive`           — one connection, N requests
//!   3. `keep-alive + batch`   — one connection, ONE ordered request
//!
//! Point it at a copy, never at a root a server is using:
//!
//!   cargo run --release -p makepad-asset-store --example thumb_bench -- <root> [count]

use makepad_asset_client::{
    ApiEndpoints, AssetClient, ClientConfig, ClientEvent, ClientOutput, ClientRequest,
    ClientRuntime, HttpLimits, RuntimeConfig, SubmitOptions,
};
use makepad_asset_data::{AssetManifest, BlobId};
use makepad_asset_store::{AssetServer, ServerConfig};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let root = match args.next() {
        Some(r) => PathBuf::from(r),
        None => {
            eprintln!("usage: thumb_bench <server-root-copy> [count]");
            std::process::exit(2);
        }
    };
    let count: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(30);

    let mut cfg = ServerConfig::new(root.clone());
    cfg.control_addr = "127.0.0.1:0".parse().unwrap();
    cfg.data_addr = "127.0.0.1:0".parse().unwrap();
    cfg.bootstrap_admin = true;
    cfg.log = false;
    let mut server = AssetServer::start(cfg).expect("server start");
    let token = std::fs::read_to_string(root.join("admin-token"))
        .expect("admin token")
        .trim()
        .to_string();
    let endpoints = ApiEndpoints { control: server.control_addr(), data: server.data_addr() };

    // Collect N real thumbnails (blob + declared length) from the catalog.
    let mut probe = connect(&endpoints, &token, "probe", true);
    let page = probe
        .catalog_search(
            &makepad_asset_client::CatalogQuery::browse(
                (count as u32).saturating_mul(3).clamp(8, makepad_asset_client::MAX_SEARCH_LIMIT),
            ),
            None,
        )
        .expect("browse");
    let mut thumbs: Vec<(BlobId, u64)> = Vec::new();
    let (mut no_alias, mut no_head, mut no_manifest, mut no_thumb) = (0, 0, 0, 0);
    for hit in &page.hits {
        // Resolve through the alias head: that is the revision a UI shows.
        let Some(alias) = hit.alias.as_ref() else {
            no_alias += 1;
            continue;
        };
        let head = match probe.resolve_alias(alias) {
            Ok(h) => h,
            Err(_) => {
                no_head += 1;
                continue;
            }
        };
        let manifest = match probe.fetch_asset_manifest(&head.head_revision) {
            Ok(m) => m,
            Err(_) => {
                no_manifest += 1;
                continue;
            }
        };
        match thumbnail_of(&manifest) {
            Some(t) => thumbs.push(t),
            None => no_thumb += 1,
        }
        if thumbs.len() == count {
            break;
        }
    }
    if thumbs.is_empty() {
        eprintln!(
            "no thumbnails found in {} (hits: {}, with alias: {})",
            root.display(),
            page.hits.len(),
            page.hits.iter().filter(|h| h.alias.is_some()).count()
        );
        eprintln!(
            "skipped: no_alias {no_alias}, alias unresolved {no_head}, manifest {no_manifest}, no thumbnail {no_thumb}"
        );
        std::process::exit(1);
    }
    let total_bytes: u64 = thumbs.iter().map(|(_, len)| *len).sum();
    println!(
        "{} thumbnails, {} bytes total ({} avg)",
        thumbs.len(),
        total_bytes,
        total_bytes / thumbs.len() as u64
    );

    // Baseline without the runtime in the picture: sequential fetches on one
    // handle, so a per-request cost cannot hide behind worker scheduling.
    for (label, keep_alive) in [("sequential, connect-per-request", false), ("sequential, keep-alive", true)] {
        let before = server.data_connections_accepted();
        let before_reqs = server.data_requests_served();
        let mut client = connect(&endpoints, &token, label, keep_alive);
        let t0 = Instant::now();
        for (blob, len) in &thumbs {
            client.fetch_blob(blob, Some(*len), None).expect("fetch");
        }
        let elapsed = t0.elapsed();
        println!(
            "{label:>32}: {:>8.2} ms total, {:>6.3} ms each, {} conn, {} requests",
            elapsed.as_secs_f64() * 1000.0,
            elapsed.as_secs_f64() * 1000.0 / thumbs.len() as f64,
            server.data_connections_accepted() - before,
            server.data_requests_served() - before_reqs,
        );
    }

    for (label, keep_alive, batch) in [
        ("connect-per-request", false, 1usize),
        ("keep-alive", true, 1),
        ("keep-alive + batch", true, 16),
    ] {
        let before = server.data_connections_accepted();
        let before_reqs = server.data_requests_served();
        let client = connect(&endpoints, &token, label, keep_alive);
        let mut runtime = ClientRuntime::start_with(
            client,
            RuntimeConfig { fast_batch_max_items: batch, ..RuntimeConfig::default_v1() },
        )
        .expect("runtime");
        let t0 = Instant::now();
        let mut ids = Vec::with_capacity(thumbs.len());
        for (blob, len) in &thumbs {
            ids.push(
                runtime
                    .submit_with(
                        ClientRequest::FetchBlob {
                            blob: *blob,
                            expected_len: Some(*len),
                            pin: false,
                        },
                        SubmitOptions::fast(),
                    )
                    .expect("submit"),
            );
        }
        let mut done = 0usize;
        let deadline = Instant::now() + Duration::from_secs(120);
        while done < ids.len() {
            assert!(Instant::now() < deadline, "{label}: never finished");
            for event in runtime.poll() {
                match event {
                    ClientEvent::Done { output: ClientOutput::Blob { .. }, .. } => done += 1,
                    ClientEvent::Failed { id, error } => panic!("{label}: {id} failed: {error}"),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let elapsed = t0.elapsed();
        let conns = server.data_connections_accepted() - before;
        let reqs = server.data_requests_served() - before_reqs;
        println!(
            "{label:>32}: {:>8.2} ms total, {:>6.3} ms each, {conns} conn, {reqs} requests",
            elapsed.as_secs_f64() * 1000.0,
            elapsed.as_secs_f64() * 1000.0 / thumbs.len() as f64,
        );
        runtime.shutdown();
    }
    server.shutdown();
}

/// The bytes a grid actually renders for one asset: its typed thumbnail
/// when it has one, otherwise the smallest thumbnail-sized file it carries
/// (sprite/billboard content has no separate thumbnail — the sprite IS it).
fn thumbnail_of(manifest: &AssetManifest) -> Option<(BlobId, u64)> {
    if let Some(t) = manifest.thumbnail.as_ref() {
        return Some((t.blob, t.byte_len));
    }
    manifest
        .files
        .iter()
        .filter(|f| f.byte_len <= 512 * 1024)
        .min_by_key(|f| f.byte_len)
        .map(|f| (f.blob, f.byte_len))
}

fn connect(endpoints: &ApiEndpoints, token: &str, tag: &str, keep_alive: bool) -> AssetClient {
    let dir = std::env::temp_dir().join(format!(
        "mp_thumb_bench_{}_{}_{}",
        std::process::id(),
        tag.replace(' ', "_").replace('+', ""),
        Instant::now().elapsed().as_nanos()
    ));
    let mut cfg = ClientConfig::new(dir);
    cfg.token = Some(token.to_string());
    cfg.http_keep_alive = keep_alive;
    cfg.http = HttpLimits {
        connect_timeout_ms: 2_000,
        read_timeout_ms: 10_000,
        write_timeout_ms: 10_000,
        head_deadline_ms: 10_000,
        body_deadline_ms: 60_000,
    };
    AssetClient::connect(cfg, *endpoints, None).expect("connect")
}
