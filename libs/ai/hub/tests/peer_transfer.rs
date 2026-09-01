//! Hermetic tests of the peer-assisted model-cache lane: the authenticated
//! digest-addressed blob endpoint on the one service port, the receiver's
//! peer-first resumable download, quarantine + next-peer + Hugging Face
//! fallback, atomic install, and inventory registration. Everything runs on
//! localhost sockets with per-test cache dirs; no network, no GPUs.

use makepad_ai_hub::backend::CancelToken;
use makepad_ai_hub::client::{ContentProvider, LocalService};
use makepad_ai_hub::download::{part_path, source_file_is_verified, Downloader};
use makepad_ai_hub::http_client::{http_fetch, HttpClientRequest};
use makepad_ai_hub::peer::{build_inventory, now_unix, PeerPlan, PeerTicket, TransferSecret};
use makepad_ai_hub::peer_serve::PeerOptions;
use makepad_ai_hub::protocol::{GenerateRequestJson, ModelInventoryJson};
use makepad_ai_hub::registry::{Domain, FileSpec, Registry};
use makepad_ai_hub::server::{start_service, ServiceConfig, ServiceHandle};
use makepad_micro_serde::DeJson;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FLEET_SECRET: &str = "fleet-transfer-secret-0123456789abcdef";
const DEAD_HF: &str = "http://127.0.0.1:1";

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "makepad-asset-ai-peer-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn test_bytes(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 7 + i / 251) % 251) as u8).collect()
}

/// Registry with one pinned peer-eligible model file (+ an optional second
/// pinned file for allow-list negative cases).
fn registry_json(digest: &str, size: u64, second: Option<(&str, u64)>) -> String {
    let revision = "ab".repeat(20);
    let mut files = format!(
        r#"{{"repo":"test-org/models","path":"weights.bin","revision":"{revision}","cache_as":"unet/peer-test.bin","size":{size},"sha256":"{digest}"}}"#
    );
    if let Some((digest2, size2)) = second {
        files.push_str(&format!(
            r#",{{"repo":"test-org/models","path":"second.bin","revision":"{revision}","cache_as":"unet/peer-second.bin","size":{size2},"sha256":"{digest2}"}}"#
        ));
    }
    format!(
        r#"{{"models":[{{"id":"peer-model","domain":"image","backend":"testpattern","available":true,"gated":false,"vram_gb":null,"note":null,"files":[{files}]}}]}}"#
    )
}

fn secret() -> TransferSecret {
    TransferSecret::new(FLEET_SECRET.as_bytes()).unwrap()
}

/// Seeds `bytes` as the VERIFIED artifact for `spec` in `cache` (receipt
/// written by the downloader's existing-file verification, zero network).
fn seed_verified(cache: &PathBuf, spec: &FileSpec, bytes: &[u8]) {
    let dest = spec.dest_path(cache);
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, bytes).unwrap();
    Downloader::new(DEAD_HF, None)
        .unwrap()
        .ensure_file(spec, cache, &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert!(source_file_is_verified(spec, cache));
}

struct PeerBox {
    base: String,
    node_key: String,
    cache: PathBuf,
    _handle: ServiceHandle,
}

/// Starts a full service over `registry` with the fleet secret and small
/// serve chunks; peer sources for its OWN downloads are empty unless given.
fn start_box(
    name: &str,
    registry: &Registry,
    chunk: u64,
    max_serves: usize,
    mbps: Option<f64>,
    sources: Vec<String>,
) -> PeerBox {
    let cache = test_dir(name);
    let handle = start_service(ServiceConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        cache_dir: cache.clone(),
        registry: registry.clone(),
        downloader: Downloader::new(DEAD_HF, None).unwrap(),
        peer: PeerOptions {
            secret: Some(FLEET_SECRET.to_string()),
            serve: Some(true),
            chunk_max_bytes: Some(chunk),
            max_serves: Some(max_serves),
            mbps,
            sources: Some(sources),
        },
        fleet: makepad_ai_hub::discovery::DEFAULT_FLEET.to_string(),
    })
    .unwrap();
    let base = format!("http://{}", handle.addr);
    let health = LocalService::new(&base).health().unwrap();
    PeerBox {
        base,
        node_key: health.node_key.unwrap(),
        cache,
        _handle: handle,
    }
}

struct BlobReply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl BlobReply {
    fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }
}

fn blob_get(
    base: &str,
    digest_path: &str,
    ticket: Option<&str>,
    receiver: Option<&str>,
    range_from: Option<u64>,
    extra: &[(String, String)],
) -> BlobReply {
    let url = format!("{base}/v1/model_blob/{digest_path}");
    let mut headers: Vec<(String, String)> = Vec::new();
    if let Some(ticket) = ticket {
        headers.push(("Authorization".to_string(), format!("Bearer {ticket}")));
    }
    if let Some(receiver) = receiver {
        headers.push(("X-Peer-Receiver".to_string(), receiver.to_string()));
    }
    headers.extend(extra.iter().cloned());
    let request = HttpClientRequest {
        method: "GET",
        url: &url,
        range_from,
        range_to: None,
        bearer: None,
        body: None,
        extra_headers: &headers,
    };
    let response = http_fetch(&request).unwrap();
    let status = response.status;
    let head: Vec<(String, String)> = response.headers.clone();
    let body = response.read_body_to_vec(64 * 1024 * 1024).unwrap();
    BlobReply {
        status,
        headers: head,
        body,
    }
}

fn receiver_key() -> String {
    "1234567890abcdef1234567890abcdef".to_string()
}

fn plan_with_secret(sources: Vec<String>, receiver: &str) -> Arc<PeerPlan> {
    // Shared-secret self-minting is deliberately restricted to sources from
    // trusted operator configuration, never arbitrary request URLs.
    PeerPlan::for_job(&[], &[], &sources, receiver, Some(secret())).expect("plan")
}

// ---------------------------------------------------------------------------
// Source-side endpoint: auth, allow-list, ranges
// ---------------------------------------------------------------------------

#[test]
fn blob_endpoint_auth_allowlist_and_ranges() {
    let bytes = test_bytes(200_000);
    let digest = makepad_ai_hub::sha256::sha256_hex(&bytes);
    let unverified = test_bytes(500);
    let unverified_digest = makepad_ai_hub::sha256::sha256_hex(&unverified);
    let registry = Registry::parse(&registry_json(
        &digest,
        bytes.len() as u64,
        Some((&unverified_digest, unverified.len() as u64)),
    ))
    .unwrap();

    // Seed + verify the first file; put WRONG bytes (no receipt possible)
    // at the second file's path.
    let cache = test_dir("serve-src-pre");
    seed_verified(&cache, &registry.find("peer-model").unwrap().files[0], &bytes);
    let second = &registry.find("peer-model").unwrap().files[1];
    let second_dest = second.dest_path(&cache);
    std::fs::create_dir_all(second_dest.parent().unwrap()).unwrap();
    std::fs::write(&second_dest, b"not the pinned bytes").unwrap();

    // Move the seeded cache under the service (same dir, service starts on it).
    let source = {
        let handle = start_service(ServiceConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            cache_dir: cache.clone(),
            registry: registry.clone(),
            downloader: Downloader::new(DEAD_HF, None).unwrap(),
            peer: PeerOptions {
                secret: Some(FLEET_SECRET.to_string()),
                serve: Some(true),
                chunk_max_bytes: Some(64 * 1024),
                max_serves: Some(4),
                mbps: None,
                sources: Some(Vec::new()),
            },
            fleet: makepad_ai_hub::discovery::DEFAULT_FLEET.to_string(),
        })
        .unwrap();
        let base = format!("http://{}", handle.addr);
        let node_key = LocalService::new(&base).health().unwrap().node_key.unwrap();
        PeerBox {
            base,
            node_key,
            cache,
            _handle: handle,
        }
    };
    let receiver = receiver_key();
    let now = now_unix();
    let ticket = PeerTicket::mint(&secret(), &source.node_key, &receiver, &digest, now + 120);

    // -- denials --
    let reply = blob_get(&source.base, &digest, None, Some(&receiver), None, &[]);
    assert_eq!(reply.status, 401, "missing ticket");
    let reply = blob_get(&source.base, &digest, Some("garbage"), Some(&receiver), None, &[]);
    assert_eq!(reply.status, 401, "malformed ticket");
    let reply = blob_get(&source.base, &digest, Some(&ticket), None, None, &[]);
    assert_eq!(reply.status, 401, "missing receiver claim");
    let wrong_receiver = "feedfacefeedfacefeedfacefeedface";
    let reply = blob_get(&source.base, &digest, Some(&ticket), Some(wrong_receiver), None, &[]);
    assert_eq!(reply.status, 403, "receiver scope mismatch");
    let expired = PeerTicket::mint(&secret(), &source.node_key, &receiver, &digest, now - 5);
    let reply = blob_get(&source.base, &digest, Some(&expired), Some(&receiver), None, &[]);
    assert_eq!(reply.status, 403, "expired ticket");
    let other_digest = "9".repeat(64);
    let scoped_elsewhere =
        PeerTicket::mint(&secret(), &source.node_key, &receiver, &other_digest, now + 120);
    let reply = blob_get(&source.base, &digest, Some(&scoped_elsewhere), Some(&receiver), None, &[]);
    assert_eq!(reply.status, 403, "digest scope mismatch");
    let wrong_source =
        PeerTicket::mint(&secret(), &"a".repeat(32), &receiver, &digest, now + 120);
    let reply = blob_get(&source.base, &digest, Some(&wrong_source), Some(&receiver), None, &[]);
    assert_eq!(reply.status, 403, "source scope mismatch");

    // -- allow-list --
    let absent = PeerTicket::mint(&secret(), &source.node_key, &receiver, &other_digest, now + 120);
    let reply = blob_get(&source.base, &other_digest, Some(&absent), Some(&receiver), None, &[]);
    assert_eq!(reply.status, 404, "unknown digest");
    let unverified_ticket = PeerTicket::mint(
        &secret(),
        &source.node_key,
        &receiver,
        &unverified_digest,
        now + 120,
    );
    let reply = blob_get(
        &source.base,
        &unverified_digest,
        Some(&unverified_ticket),
        Some(&receiver),
        None,
        &[],
    );
    assert_eq!(reply.status, 404, "on disk but NOT verified must never serve");

    // -- path shapes: no traversal surface, digests only --
    let a63 = "a".repeat(63);
    let g64 = "g".repeat(64);
    for bad in ["../../etc/passwd", "abc", a63.as_str(), g64.as_str()] {
        let reply = blob_get(&source.base, bad, Some(&ticket), Some(&receiver), None, &[]);
        assert_eq!(reply.status, 400, "path {bad:?}");
    }
    let reply = blob_get(
        &source.base,
        &format!("{digest}?x=1"),
        Some(&ticket),
        Some(&receiver),
        None,
        &[],
    );
    assert_eq!(reply.status, 400, "query refused");

    // -- happy chunked reads --
    let reply = blob_get(&source.base, &digest, Some(&ticket), Some(&receiver), None, &[]);
    assert_eq!(reply.status, 206, "big file without Range = bounded 206");
    assert_eq!(
        reply.header("content-range"),
        Some(format!("bytes 0-65535/{}", bytes.len()).as_str())
    );
    assert_eq!(reply.header("etag"), Some(format!("\"sha256:{digest}\"").as_str()));
    assert_eq!(reply.header("accept-ranges"), Some("bytes"));
    assert_eq!(reply.body, &bytes[..65536]);

    let reply = blob_get(&source.base, &digest, Some(&ticket), Some(&receiver), Some(100_000), &[]);
    assert_eq!(reply.status, 206);
    assert_eq!(
        reply.header("content-range"),
        Some(format!("bytes 100000-165535/{}", bytes.len()).as_str())
    );
    assert_eq!(reply.body, &bytes[100_000..165_536]);

    let reply = blob_get(&source.base, &digest, Some(&ticket), Some(&receiver), Some(199_999), &[]);
    assert_eq!(reply.status, 206);
    assert_eq!(reply.body, &bytes[199_999..]);

    // -- 416 past the end --
    let reply = blob_get(
        &source.base,
        &digest,
        Some(&ticket),
        Some(&receiver),
        Some(bytes.len() as u64),
        &[],
    );
    assert_eq!(reply.status, 416);
    assert_eq!(
        reply.header("content-range"),
        Some(format!("bytes */{}", bytes.len()).as_str())
    );

    // -- If-Range mismatch = serve from zero --
    let reply = blob_get(
        &source.base,
        &digest,
        Some(&ticket),
        Some(&receiver),
        Some(100_000),
        &[("If-Range".to_string(), "\"sha256:deadbeef\"".to_string())],
    );
    assert_eq!(reply.status, 206);
    assert!(reply
        .header("content-range")
        .unwrap()
        .starts_with("bytes 0-"));

    // -- inventory reports exactly the verified artifact --
    let inventory = fetch_inventory(&source.base);
    assert!(inventory.peer_serving);
    assert_eq!(inventory.node_key, source.node_key);
    assert!(inventory.artifacts.iter().any(|a| a.digest == digest
        && a.size == bytes.len() as u64
        && a.cache_as == "unet/peer-test.bin"
        && a.models == vec!["peer-model".to_string()]));
    assert!(
        !inventory.artifacts.iter().any(|a| a.digest == unverified_digest),
        "unverified files must not be advertised"
    );
    drop(source);
}

fn fetch_inventory(base: &str) -> ModelInventoryJson {
    let url = format!("{base}/v1/model_inventory");
    let response = http_fetch(&HttpClientRequest::get(&url)).unwrap();
    assert_eq!(response.status, 200);
    let body = response.read_body_to_vec(4 * 1024 * 1024).unwrap();
    ModelInventoryJson::deserialize_json_lenient(std::str::from_utf8(&body).unwrap()).unwrap()
}

#[test]
fn serving_fails_closed_without_a_secret() {
    let bytes = test_bytes(4_000);
    let digest = makepad_ai_hub::sha256::sha256_hex(&bytes);
    let registry = Registry::parse(&registry_json(&digest, bytes.len() as u64, None)).unwrap();
    let cache = test_dir("no-secret");
    seed_verified(&cache, &registry.find("peer-model").unwrap().files[0], &bytes);
    let handle = start_service(ServiceConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        cache_dir: cache,
        registry,
        downloader: Downloader::new(DEAD_HF, None).unwrap(),
        peer: PeerOptions {
            // Explicit empty override so an env/file secret can never leak
            // into this test; empty = too short = absent.
            secret: Some(String::new()),
            serve: Some(true),
            chunk_max_bytes: Some(64 * 1024),
            max_serves: Some(2),
            mbps: None,
            sources: Some(Vec::new()),
        },
        fleet: makepad_ai_hub::discovery::DEFAULT_FLEET.to_string(),
    })
    .unwrap();
    let base = format!("http://{}", handle.addr);
    let node_key = LocalService::new(&base).health().unwrap().node_key.unwrap();
    let ticket = PeerTicket::mint(&secret(), &node_key, &receiver_key(), &digest, now_unix() + 60);
    let reply = blob_get(&base, &digest, Some(&ticket), Some(&receiver_key()), None, &[]);
    assert_eq!(reply.status, 403, "no secret -> nothing serves");
    // The inventory endpoint still reports holdings (it is how the
    // coordinator learns this box could serve once provisioned)…
    let inventory = fetch_inventory(&base);
    assert!(!inventory.peer_serving);
    assert!(inventory.artifacts.iter().any(|a| a.digest == digest));
}

// ---------------------------------------------------------------------------
// Receiver: peer-first, resume, verify, atomic install, fallbacks
// ---------------------------------------------------------------------------

#[test]
fn receiver_fetches_from_peer_resumes_and_installs_atomically() {
    let bytes = test_bytes(300_000);
    let digest = makepad_ai_hub::sha256::sha256_hex(&bytes);
    let registry = Registry::parse(&registry_json(&digest, bytes.len() as u64, None)).unwrap();
    let spec = registry.find("peer-model").unwrap().files[0].clone();

    let source = start_box("recv-src-svc", &registry, 64 * 1024, 4, None, Vec::new());
    seed_verified(&source.cache, &spec, &bytes);

    let receiver_cache = test_dir("recv-dst");
    // Pre-seed a resumable partial + a stale garbage dest that must be
    // replaced atomically (the Windows-safe remove+rename path).
    let dest = spec.dest_path(&receiver_cache);
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(part_path(&dest), &bytes[..12_345]).unwrap();
    std::fs::write(&dest, b"stale garbage from an interrupted deploy").unwrap();

    let downloader = Downloader::new(DEAD_HF, None)
        .unwrap()
        .with_peer_plan(Some(plan_with_secret(
            vec![source.base.clone()],
            &receiver_key(),
        )));
    let mut reports = Vec::new();
    let out = downloader
        .ensure_file(&spec, &receiver_cache, &mut |p| reports.push(p.done), &CancelToken::new())
        .unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), bytes);
    assert!(!part_path(&dest).exists(), ".part renamed away");
    assert!(source_file_is_verified(&spec, &receiver_cache), "receipt written");
    // Resume actually resumed: the first progress report continues past the
    // pre-seeded prefix rather than restarting at the chunk size.
    assert!(reports.iter().all(|done| *done >= 12_345), "{reports:?}");
    // The receiver now registers the digest in its own inventory.
    let inventory = build_inventory(&registry, &receiver_cache);
    assert!(inventory.iter().any(|entry| entry.digest == digest));

    // A second ensure is a no-op (receipt short-circuits — and would fail
    // loudly if it hit the dead HF base or the peer again with bad state).
    downloader
        .ensure_file(&spec, &receiver_cache, &mut |_| {}, &CancelToken::new())
        .unwrap();
}

/// Minimal hostile peer: answers /health with a plausible node identity and
/// serves CORRUPT bytes with correct sizes/headers for any blob request.
/// Also used (without corruption) as a dead-stall stand-in via `close_early`.
struct LyingPeer {
    addr: SocketAddr,
    hits: Arc<Mutex<usize>>,
}

fn spawn_lying_peer(total: u64, digest: &str, close_early: bool) -> LyingPeer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(Mutex::new(0usize));
    let hits_thread = hits.clone();
    let digest = digest.to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let head = read_head(&mut stream);
            if head.is_empty() {
                continue; // receiver's TCP reachability preflight
            }
            let path = head
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();
            if path == "/health" {
                let body = format!(
                    r#"{{"service":"makepad-asset-ai","version":"0","gpu":null,"vram_free_mb":null,"vram_total_mb":null,"models_loaded":[],"jobs_pending":null,"node_id":null,"node_key":"{}","started_ms":null,"capabilities":null,"vram_reserve_mb":null,"queue_limit":null}}"#,
                    "c0ffee00c0ffee00c0ffee00c0ffee00"
                );
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
                continue;
            }
            *hits_thread.lock().unwrap() += 1;
            let offset = parse_range_from(&head).unwrap_or(0).min(total);
            let len = total - offset;
            if close_early {
                let header = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {offset}-{}/{total}\r\nContent-Length: {len}\r\nETag: \"sha256:{digest}\"\r\nConnection: close\r\n\r\n",
                    total - 1
                );
                let _ = stream.write_all(header.as_bytes());
                // Send a fragment, then die mid-chunk.
                let _ = stream.write_all(&vec![0xa5u8; (len / 4).max(1) as usize]);
                continue;
            }
            let header = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {offset}-{}/{total}\r\nContent-Length: {len}\r\nETag: \"sha256:{digest}\"\r\nConnection: close\r\n\r\n",
                total - 1
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&vec![0x5au8; len as usize]); // corrupt
        }
    });
    LyingPeer { addr, hits }
}

fn read_head(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => buf.push(byte[0]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

fn parse_range_from(head: &str) -> Option<u64> {
    for line in head.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("range: bytes=") {
            return rest.split('-').next()?.trim().parse().ok();
        }
    }
    None
}

/// Fixture Hugging Face endpoint: plain 200 with the full body, counting hits.
struct HfFixture {
    base: String,
    hits: Arc<Mutex<usize>>,
}

fn spawn_hf_fixture(data: Vec<u8>) -> HfFixture {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(Mutex::new(0usize));
    let hits_thread = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let _ = read_head(&mut stream);
            *hits_thread.lock().unwrap() += 1;
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    data.len()
                )
                .as_bytes(),
            );
            let _ = stream.write_all(&data);
        }
    });
    HfFixture { base, hits }
}

/// Canonical fixture that honors resume ranges. This catches poisoned peer
/// prefixes: a canonical server is allowed to continue the requested offset,
/// so the peer phase must roll its own unverified bytes back first.
fn spawn_ranged_hf_fixture(data: Vec<u8>) -> HfFixture {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(Mutex::new(0usize));
    let hits_thread = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let head = read_head(&mut stream);
            *hits_thread.lock().unwrap() += 1;
            let offset = parse_range_from(&head).unwrap_or(0);
            let offset = usize::try_from(offset).unwrap().min(data.len());
            let body = &data[offset..];
            let header = if offset == 0 {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
            } else {
                format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {offset}-{}/{total}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    data.len() - 1,
                    body.len(),
                    total = data.len()
                )
            };
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    HfFixture { base, hits }
}

/// A peer that gets one corrupt but syntactically valid full chunk committed,
/// then fails the next request. Those bytes are unauthenticated until the
/// whole-file digest passes and must not flow into another source.
fn spawn_poison_then_fail_peer(total: u64, digest: &str, chunk_len: u64) -> LyingPeer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(Mutex::new(0usize));
    let hits_thread = hits.clone();
    let digest = digest.to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let head = read_head(&mut stream);
            if head.is_empty() {
                continue; // receiver's TCP reachability preflight
            }
            let path = head
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            if path == "/health" {
                let body = format!(
                    r#"{{"service":"makepad-asset-ai","version":"0","gpu":null,"vram_free_mb":null,"vram_total_mb":null,"models_loaded":[],"jobs_pending":null,"node_id":null,"node_key":"{}","started_ms":null,"capabilities":null,"vram_reserve_mb":null,"queue_limit":null}}"#,
                    "c0ffee00c0ffee00c0ffee00c0ffee00"
                );
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
                continue;
            }
            let mut count = hits_thread.lock().unwrap();
            *count += 1;
            let hit = *count;
            drop(count);
            if hit == 1 {
                let end = chunk_len - 1;
                let header = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-{end}/{total}\r\nContent-Length: {chunk_len}\r\nETag: \"sha256:{digest}\"\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&vec![0x5au8; chunk_len as usize]);
            } else {
                let _ = stream.write_all(
                    b"HTTP/1.1 500 Broken Peer\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        }
    });
    LyingPeer { addr, hits }
}

fn spawn_oversized_range_peer(total: u64, digest: &str) -> LyingPeer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(Mutex::new(0usize));
    let hits_thread = hits.clone();
    let digest = digest.to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let head = read_head(&mut stream);
            if head.is_empty() {
                continue; // receiver's TCP reachability preflight
            }
            let path = head
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            if path == "/health" {
                let body = format!(
                    r#"{{"service":"makepad-asset-ai","version":"0","gpu":null,"vram_free_mb":null,"vram_total_mb":null,"models_loaded":[],"jobs_pending":null,"node_id":null,"node_key":"{}","started_ms":null,"capabilities":null,"vram_reserve_mb":null,"queue_limit":null}}"#,
                    "c0ffee00c0ffee00c0ffee00c0ffee00"
                );
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
                continue;
            }
            *hits_thread.lock().unwrap() += 1;
            let end = total - 1;
            let header = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-{end}/{total}\r\nContent-Length: {total}\r\nETag: \"sha256:{digest}\"\r\nConnection: close\r\n\r\n"
            );
            // The header alone is enough: a bounded receiver rejects the
            // oversized declared range before allocating or reading a body.
            let _ = stream.write_all(header.as_bytes());
        }
    });
    LyingPeer { addr, hits }
}

#[test]
fn corrupt_peer_quarantines_then_next_peer_serves() {
    let bytes = test_bytes(150_000);
    let digest = makepad_ai_hub::sha256::sha256_hex(&bytes);
    let registry = Registry::parse(&registry_json(&digest, bytes.len() as u64, None)).unwrap();
    let spec = registry.find("peer-model").unwrap().files[0].clone();

    let liar = spawn_lying_peer(bytes.len() as u64, &digest, false);
    let good = start_box("good-src", &registry, 64 * 1024, 4, None, Vec::new());
    seed_verified(&good.cache, &spec, &bytes);

    let receiver_cache = test_dir("quarantine-next");
    let downloader = Downloader::new(DEAD_HF, None)
        .unwrap()
        .with_peer_plan(Some(plan_with_secret(
            vec![format!("http://{}", liar.addr), good.base.clone()],
            &receiver_key(),
        )));
    let out = downloader
        .ensure_file(&spec, &receiver_cache, &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(std::fs::read(out).unwrap(), bytes);
    assert!(*liar.hits.lock().unwrap() >= 1, "liar was actually tried");
    assert!(source_file_is_verified(&spec, &receiver_cache));
}

#[test]
fn corrupt_peer_falls_back_to_hugging_face() {
    let bytes = test_bytes(90_000);
    let digest = makepad_ai_hub::sha256::sha256_hex(&bytes);
    let registry = Registry::parse(&registry_json(&digest, bytes.len() as u64, None)).unwrap();
    let spec = registry.find("peer-model").unwrap().files[0].clone();

    let liar = spawn_lying_peer(bytes.len() as u64, &digest, false);
    let hf = spawn_hf_fixture(bytes.clone());
    let receiver_cache = test_dir("fallback-hf");
    let downloader = Downloader::new(&hf.base, None)
        .unwrap()
        .with_peer_plan(Some(plan_with_secret(
            vec![format!("http://{}", liar.addr)],
            &receiver_key(),
        )));
    let out = downloader
        .ensure_file(&spec, &receiver_cache, &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(std::fs::read(out).unwrap(), bytes);
    assert!(*liar.hits.lock().unwrap() >= 1);
    assert_eq!(*hf.hits.lock().unwrap(), 1, "canonical path finished the job");
    assert!(source_file_is_verified(&spec, &receiver_cache));
}

#[test]
fn failed_peer_full_chunk_is_rolled_back_before_ranged_hf_fallback() {
    let bytes = test_bytes(180_000);
    let digest = makepad_ai_hub::sha256::sha256_hex(&bytes);
    let registry = Registry::parse(&registry_json(&digest, bytes.len() as u64, None)).unwrap();
    let spec = registry.find("peer-model").unwrap().files[0].clone();

    let poison = spawn_poison_then_fail_peer(bytes.len() as u64, &digest, 64 * 1024);
    let hf = spawn_ranged_hf_fixture(bytes.clone());
    let receiver_cache = test_dir("rollback-before-hf");
    let downloader = Downloader::new(&hf.base, None)
        .unwrap()
        .with_peer_plan(Some(plan_with_secret(
            vec![format!("http://{}", poison.addr)],
            &receiver_key(),
        )));
    let out = downloader
        .ensure_file(&spec, &receiver_cache, &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(std::fs::read(out).unwrap(), bytes);
    assert_eq!(*poison.hits.lock().unwrap(), 2, "peer appended then failed");
    assert_eq!(*hf.hits.lock().unwrap(), 1);
    assert!(source_file_is_verified(&spec, &receiver_cache));
}

#[test]
fn cross_origin_redirect_never_forwards_a_ticket() {
    let sink = TcpListener::bind("127.0.0.1:0").unwrap();
    let sink_addr = sink.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured_thread = captured.clone();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = sink.accept() {
            captured_thread.lock().unwrap().push(read_head(&mut stream));
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });

    let redirector = TcpListener::bind("127.0.0.1:0").unwrap();
    let redirect_base = format!("http://{}", redirector.local_addr().unwrap());
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = redirector.accept() {
            let _ = read_head(&mut stream);
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{sink_addr}/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let headers = [("Authorization".to_string(), "Bearer secret-ticket".to_string())];
    let request = HttpClientRequest {
        method: "GET",
        url: &redirect_base,
        range_from: None,
        range_to: None,
        bearer: None,
        body: None,
        extra_headers: &headers,
    };
    assert_eq!(http_fetch(&request).unwrap().status, 200);
    let deadline = Instant::now() + Duration::from_secs(2);
    while captured.lock().unwrap().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let head = captured.lock().unwrap().first().cloned().unwrap();
    assert!(!head.to_ascii_lowercase().contains("authorization:"), "{head}");
}

#[test]
fn hostile_content_range_is_rejected_before_body_allocation() {
    let total = makepad_ai_hub::peer_fetch::MAX_RECEIVE_CHUNK + 1;
    let digest = "a".repeat(64);
    let registry = Registry::parse(&registry_json(&digest, total, None)).unwrap();
    let spec = registry.find("peer-model").unwrap().files[0].clone();
    let hostile = spawn_oversized_range_peer(total, &digest);
    let receiver_cache = test_dir("oversized-range");
    let started = Instant::now();
    let err = Downloader::new(DEAD_HF, None)
        .unwrap()
        .with_peer_plan(Some(plan_with_secret(
            vec![format!("http://{}", hostile.addr)],
            &receiver_key(),
        )))
        .ensure_file(&spec, &receiver_cache, &mut |_| {}, &CancelToken::new())
        .unwrap_err();
    assert!(err.to_string().contains("http"), "{err}");
    assert_eq!(*hostile.hits.lock().unwrap(), 1);
    assert!(started.elapsed() < Duration::from_secs(5));
    let part = part_path(&spec.dest_path(&receiver_cache));
    assert!(std::fs::metadata(part).map(|m| m.len()).unwrap_or(0) == 0);
}

#[test]
fn broken_and_dead_peers_are_skipped_fast() {
    let bytes = test_bytes(120_000);
    let digest = makepad_ai_hub::sha256::sha256_hex(&bytes);
    let registry = Registry::parse(&registry_json(&digest, bytes.len() as u64, None)).unwrap();
    let spec = registry.find("peer-model").unwrap().files[0].clone();

    // Peer 1: nothing listens (connect refused). Peer 2: dies mid-chunk
    // with junk bytes (quarantined at hash time). Peer 3: good.
    let broken = spawn_lying_peer(bytes.len() as u64, &digest, true);
    let good = start_box("skip-src", &registry, 64 * 1024, 4, None, Vec::new());
    seed_verified(&good.cache, &spec, &bytes);

    let receiver_cache = test_dir("skip-dead");
    let downloader = Downloader::new(DEAD_HF, None)
        .unwrap()
        .with_peer_plan(Some(plan_with_secret(
            vec![
                "http://127.0.0.1:9".to_string(),
                format!("http://{}", broken.addr),
                good.base.clone(),
            ],
            &receiver_key(),
        )));
    let started = Instant::now();
    let out = downloader
        .ensure_file(&spec, &receiver_cache, &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(std::fs::read(out).unwrap(), bytes);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "dead peers must not eat client timeouts"
    );
}

// ---------------------------------------------------------------------------
// Service-level end to end: pull job rides the peer lane, inventory follows
// ---------------------------------------------------------------------------

#[test]
fn pull_job_uses_coordinator_peer_source_and_registers_inventory() {
    let bytes = test_bytes(250_000);
    let digest = makepad_ai_hub::sha256::sha256_hex(&bytes);
    let registry = Registry::parse(&registry_json(&digest, bytes.len() as u64, None)).unwrap();
    let spec = registry.find("peer-model").unwrap().files[0].clone();

    let source = start_box("e2e-src", &registry, 64 * 1024, 4, None, Vec::new());
    seed_verified(&source.cache, &spec, &bytes);
    let receiver = start_box("e2e-dst", &registry, 64 * 1024, 4, None, Vec::new());

    // Coordinator-shaped request: pull the model, naming the source box and
    // carrying a ticket minted for this exact receiver/source/digest tuple.
    let provider = LocalService::new(&receiver.base);
    let ticket = PeerTicket::mint(
        &secret(),
        &source.node_key,
        &receiver.node_key,
        &digest,
        now_unix() + 120,
    );
    let job = provider
        .request(
            Domain::Image,
            &GenerateRequestJson {
                model: "peer-model".to_string(),
                pull_only: Some(true),
                peer_sources: Some(vec![source.base.clone()]),
                peer_tickets: Some(vec![ticket]),
                ..Default::default()
            },
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = provider.poll(&job).unwrap();
        match status.state.as_str() {
            "done" => break,
            "error" => panic!("pull failed: {:?}", status.error),
            _ => {
                assert!(Instant::now() < deadline, "pull did not finish");
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
    // Bytes landed at the canonical path, receipt-verified.
    let dest = spec.dest_path(&receiver.cache);
    assert_eq!(std::fs::read(&dest).unwrap(), bytes);
    assert!(source_file_is_verified(&spec, &receiver.cache));
    // The receiver's inventory now advertises the digest: the coordinator
    // can select THIS box as a source for the rest of the fleet.
    let inventory = fetch_inventory(&receiver.base);
    assert!(inventory.peer_serving);
    assert!(inventory.artifacts.iter().any(|a| a.digest == digest));

    // Full circle: a third party can now pull the blob FROM the receiver.
    let ticket = PeerTicket::mint(
        &secret(),
        &inventory.node_key,
        &receiver_key(),
        &digest,
        now_unix() + 60,
    );
    let reply = blob_get(&receiver.base, &digest, Some(&ticket), Some(&receiver_key()), None, &[]);
    assert_eq!(reply.status, 206);
    assert_eq!(reply.body, &bytes[..64 * 1024]);
}

#[test]
fn coordinator_tickets_work_without_a_receiver_secret() {
    // Coordinator mode: the receiver holds NO transfer secret; it can only
    // use explicitly minted tickets (and cannot serve).
    let bytes = test_bytes(80_000);
    let digest = makepad_ai_hub::sha256::sha256_hex(&bytes);
    let registry = Registry::parse(&registry_json(&digest, bytes.len() as u64, None)).unwrap();
    let spec = registry.find("peer-model").unwrap().files[0].clone();

    let source = start_box("tickets-src", &registry, 64 * 1024, 4, None, Vec::new());
    seed_verified(&source.cache, &spec, &bytes);

    let receiver = receiver_key();
    let ticket = PeerTicket::mint(&secret(), &source.node_key, &receiver, &digest, now_unix() + 120);
    let plan = PeerPlan::for_job(
        &[source.base.clone()],
        &[ticket],
        &[],
        &receiver,
        None, // no secret on the receiver
    )
    .unwrap();
    let receiver_cache = test_dir("tickets-dst");
    let out = Downloader::new(DEAD_HF, None)
        .unwrap()
        .with_peer_plan(Some(plan))
        .ensure_file(&spec, &receiver_cache, &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(std::fs::read(out).unwrap(), bytes);

    // Without a ticket AND without a secret the peer lane is unusable and
    // the dead HF base surfaces as the error — never a silent success.
    let no_auth_plan = PeerPlan::for_job(&[source.base.clone()], &[], &[], &receiver, None).unwrap();
    let bare_cache = test_dir("tickets-none");
    let err = Downloader::new(DEAD_HF, None)
        .unwrap()
        .with_peer_plan(Some(no_auth_plan))
        .ensure_file(&spec, &bare_cache, &mut |_| {}, &CancelToken::new())
        .unwrap_err();
    assert!(err.to_string().contains("http"), "{err}");
}

// ---------------------------------------------------------------------------
// Concurrency bound
// ---------------------------------------------------------------------------

#[test]
fn concurrent_serves_beyond_the_bound_get_503() {
    let bytes = test_bytes(128 * 1024);
    let digest = makepad_ai_hub::sha256::sha256_hex(&bytes);
    let registry = Registry::parse(&registry_json(&digest, bytes.len() as u64, None)).unwrap();
    let spec = registry.find("peer-model").unwrap().files[0].clone();

    // max_serves 1 + 0.25 MB/s pacing: the second request holds the single
    // slot for ~0.5s of pacing, during which a third gets an immediate 503.
    let source = start_box("bound-src", &registry, 128 * 1024, 1, Some(0.25), Vec::new());
    seed_verified(&source.cache, &spec, &bytes);
    let receiver = receiver_key();
    let ticket = PeerTicket::mint(&secret(), &source.node_key, &receiver, &digest, now_unix() + 60);

    // Prime the throttle (first reservation is free). The whole file fits
    // one chunk, so a no-Range GET is a plain 200.
    let reply = blob_get(&source.base, &digest, Some(&ticket), Some(&receiver), None, &[]);
    assert_eq!(reply.status, 200);

    let base = source.base.clone();
    let ticket2 = ticket.clone();
    let receiver2 = receiver.clone();
    let slow = std::thread::spawn(move || {
        blob_get(&base, &digest, Some(&ticket2), Some(&receiver2), None, &[]).status
    });
    std::thread::sleep(Duration::from_millis(200));
    let reply = blob_get(
        &source.base,
        &makepad_ai_hub::sha256::sha256_hex(&bytes),
        Some(&ticket),
        Some(&receiver),
        None,
        &[],
    );
    assert_eq!(reply.status, 503, "at capacity -> immediate honest refusal");
    assert_eq!(reply.header("retry-after"), Some("1"));
    assert_eq!(slow.join().unwrap(), 200, "the in-flight serve completes");
}
