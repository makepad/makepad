//! Hostile-server tests: every byte a server can send is adversarial until
//! proven otherwise. Each test runs a REAL TCP listener that misbehaves in
//! one specific way and asserts the client refuses with the right typed
//! error, keeps its budgets, and never lets bad bytes reach cache or caller.

mod common;

use common::{
    payload, response_head, test_root, write_bytes_resp, write_error, write_json_resp, write_raw,
    FixtureOptions, FixtureServer, FixtureStore, ParsedRequest, RawServer,
};
use makepad_asset_client::json::{obj, s, Value};
use makepad_asset_client::{
    Api, ApiEndpoints, AssetClient, Beacon, CatalogQuery, ClientConfig, ClientError,
    DiscoveryListener, HttpLimits,
};
use makepad_asset_data::{AssetRevisionId, BlobId};
use std::net::{TcpStream, UdpSocket};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn fast_limits() -> HttpLimits {
    HttpLimits {
        connect_timeout_ms: 2_000,
        read_timeout_ms: 500,
        write_timeout_ms: 2_000,
        head_deadline_ms: 700,
        body_deadline_ms: 700,
    }
}

fn api_at(addr: std::net::SocketAddr) -> Api {
    Api::new(ApiEndpoints { control: addr, data: addr }, fast_limits(), None).unwrap()
}

/// RawServer that answers every request with exactly `bytes` then closes.
fn canned(bytes: Vec<u8>) -> RawServer {
    RawServer::start(Arc::new(move |_req, stream: &mut TcpStream| {
        write_raw(stream, &bytes);
    }))
}

// ---------------------------------------------------------------------------
// discovery
// ---------------------------------------------------------------------------

#[test]
fn discovery_ignores_garbage_and_bounds_floods() {
    let listener = DiscoveryListener::start(0, 60_000, now_ms).unwrap();
    let port = listener.port();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let dest = ("127.0.0.1", port);

    // Garbage payloads: short, long, bad magic, zero version/ports.
    socket.send_to(b"nonsense", dest).unwrap();
    socket.send_to(&[0u8; 36], dest).unwrap();
    socket.send_to(&[0u8; 100], dest).unwrap();
    let mut beacon = Beacon {
        protocol_version: 1,
        server_id: [1; 16],
        control_port: 9,
        data_port: 9,
        auth_required: false,
        tls: false,
        capability_bits: 0xf,
    };
    let mut zero = beacon.encode();
    zero[8] = 0;
    zero[9] = 0; // version 0
    socket.send_to(&zero, dest).unwrap();

    // A flood of forged identities cannot grow the cache past its cap.
    for i in 0..400u32 {
        beacon.server_id[0] = (i & 0xff) as u8;
        beacon.server_id[1] = (i >> 8) as u8;
        socket.send_to(&beacon.encode(), dest).unwrap();
    }
    // One known-good beacon.
    beacon.server_id = [0x77; 16];
    socket.send_to(&beacon.encode(), dest).unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let snap = listener.snapshot(now_ms());
        let found = snap.iter().any(|d| d.server_id == [0x77; 16]);
        assert!(
            snap.len() <= makepad_asset_client::MAX_ENTRIES,
            "flood breached the cache cap"
        );
        if found {
            // Endpoints derive from the sender address, never the payload.
            let d = snap.iter().find(|d| d.server_id == [0x77; 16]).unwrap();
            assert_eq!(d.ip.to_string(), "127.0.0.1");
            assert_eq!(d.control_port, 9);
            break;
        }
        assert!(std::time::Instant::now() < deadline, "beacon never surfaced");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------------------
// transport-level hostility
// ---------------------------------------------------------------------------

#[test]
fn hostile_response_heads_refused() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("garbage status", b"WAT 200\r\nContent-Length: 0\r\n\r\n".to_vec()),
        ("http2", b"HTTP/2 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec()),
        ("redirect", b"HTTP/1.1 302 Found\r\nLocation: /elsewhere\r\nContent-Length: 0\r\n\r\n".to_vec()),
        ("1xx", b"HTTP/1.1 100 Continue\r\n\r\n".to_vec()),
        ("204", b"HTTP/1.1 204 No Content\r\n\r\n".to_vec()),
        ("chunked", b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n".to_vec()),
        ("no length", b"HTTP/1.1 200 OK\r\n\r\n{}".to_vec()),
        (
            "smuggled double length",
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}".to_vec(),
        ),
        ("bytes past body", b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}EXTRA".to_vec()),
    ];
    for (name, bytes) in cases {
        let server = canned(bytes);
        let api = api_at(server.addr);
        let err = api.health().expect_err(name);
        assert!(
            matches!(err, ClientError::Protocol { .. }),
            "{name}: wrong refusal {err:?}"
        );
    }
}

#[test]
fn oversized_head_refused_with_bounded_memory() {
    // 64KB of headers exceeds the 32KB head budget.
    let mut resp = b"HTTP/1.1 200 OK\r\n".to_vec();
    for i in 0..8000 {
        resp.extend_from_slice(format!("X-Flood-{i}: aaaaaaaa\r\n").as_bytes());
    }
    resp.extend_from_slice(b"Content-Length: 0\r\n\r\n");
    let server = canned(resp);
    let api = api_at(server.addr);
    assert!(matches!(
        api.health().unwrap_err(),
        ClientError::Protocol { what: "response head too large" }
    ));
}

#[test]
fn silent_server_hits_head_deadline() {
    // Accepts the connection, sends nothing.
    let server = RawServer::start(Arc::new(|_req, _stream: &mut TcpStream| {
        std::thread::sleep(std::time::Duration::from_millis(2_500));
    }));
    let api = api_at(server.addr);
    let start = std::time::Instant::now();
    let err = api.health().unwrap_err();
    assert!(matches!(err, ClientError::Timeout { .. }), "{err:?}");
    assert!(start.elapsed().as_millis() < 2_000, "deadline was not enforced");
}

#[test]
fn truncated_body_is_a_transport_error() {
    // Declares 100 bytes, sends 3, closes.
    let server = canned(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nabc".to_vec());
    let api = api_at(server.addr);
    let err = api.health().unwrap_err();
    assert!(
        matches!(err, ClientError::Io { kind: std::io::ErrorKind::UnexpectedEof, .. }),
        "{err:?}"
    );
}

#[test]
fn declared_json_body_over_budget_refused_without_download() {
    // Claims a 10MB JSON body; the client must refuse on the declaration.
    let server = canned(b"HTTP/1.1 200 OK\r\nContent-Length: 10485760\r\n\r\n".to_vec());
    let api = api_at(server.addr);
    let start = std::time::Instant::now();
    let err = api.health().unwrap_err();
    assert!(matches!(err, ClientError::OverBudget { .. }), "{err:?}");
    assert!(start.elapsed().as_millis() < 500, "refusal must not wait for bytes");
}

#[test]
fn refusal_details_are_bounded_and_sanitized() {
    // 422 with an 8KB error string full of control chars.
    let nasty = format!("bad\u{1}\u{2}input {}", "x".repeat(8000));
    let server = {
        let nasty = nasty.clone();
        RawServer::start(Arc::new(move |_req, stream: &mut TcpStream| {
            write_json_resp(stream, 422, &obj(vec![("error", s(nasty.clone()))]));
        }))
    };
    let api = api_at(server.addr);
    match api.health().unwrap_err() {
        ClientError::Server { status: 422, detail: Some(d) } => {
            assert!(d.len() <= 128, "detail not bounded: {}", d.len());
            assert!(!d.chars().any(char::is_control), "controls not stripped");
        }
        other => panic!("wrong refusal: {other:?}"),
    }
}

#[test]
fn auth_statuses_map_uniformly() {
    for (status, expect_unauth) in [(401u16, true), (403u16, false)] {
        let server = RawServer::start(Arc::new(move |_req, stream: &mut TcpStream| {
            write_error(stream, status, "refused");
        }));
        let api = api_at(server.addr);
        let err = api.health().unwrap_err();
        match (expect_unauth, err) {
            (true, ClientError::Unauthenticated) => {}
            (false, ClientError::Denied) => {}
            (_, other) => panic!("status {status}: wrong mapping {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// DTO-level hostility
// ---------------------------------------------------------------------------

#[test]
fn hostile_catalog_pages_refused() {
    let good_hit = |title: &str, kind: &str| {
        obj(vec![
            ("asset_id", s(makepad_asset_data::AssetId::from_bytes([1; 16]).to_string())),
            ("namespace", s("stock")),
            ("kind", s(kind)),
            ("title", s(title)),
            ("snippet", s("snip")),
            ("score", Value::Int(5)),
            ("live", Value::Bool(true)),
        ])
    };
    let cases: Vec<(&str, Value)> = vec![
        (
            "unknown kind",
            obj(vec![
                ("hits", Value::Arr(vec![good_hit("ok", "haunted")])),
                ("total", Value::Int(1)),
            ]),
        ),
        (
            "oversized title",
            obj(vec![
                ("hits", Value::Arr(vec![good_hit(&"x".repeat(4096), "mesh")])),
                ("total", Value::Int(1)),
            ]),
        ),
        ("missing total", obj(vec![("hits", Value::Arr(vec![]))])),
        (
            "hostile cursor",
            obj(vec![
                ("hits", Value::Arr(vec![])),
                ("total", Value::Int(0)),
                ("cursor", s("a b?c")),
            ]),
        ),
        (
            "negative score",
            obj(vec![
                ("hits", Value::Arr(vec![{
                    let mut h = good_hit("ok", "mesh");
                    if let Value::Obj(pairs) = &mut h {
                        for (k, v) in pairs.iter_mut() {
                            if k == "score" {
                                *v = Value::Int(-5);
                            }
                        }
                    }
                    h
                }])),
                ("total", Value::Int(1)),
            ]),
        ),
    ];
    for (name, body) in cases {
        let server = RawServer::start(Arc::new(move |_req, stream: &mut TcpStream| {
            write_json_resp(stream, 200, &body);
        }));
        let api = api_at(server.addr);
        let err = api.catalog_search(&CatalogQuery::browse(10), None).expect_err(name);
        assert!(matches!(err, ClientError::Protocol { .. }), "{name}: {err:?}");
    }
}

#[test]
fn oversized_hit_count_refused() {
    // 600 syntactically valid hits exceed the page entry cap.
    let hit = obj(vec![
        ("asset_id", s(makepad_asset_data::AssetId::from_bytes([1; 16]).to_string())),
        ("namespace", s("stock")),
        ("kind", Value::Null),
        ("title", s("t")),
        ("snippet", s("")),
        ("score", Value::Int(1)),
        ("live", Value::Bool(false)),
    ]);
    let body = obj(vec![
        ("hits", Value::Arr(vec![hit; 600])),
        ("total", Value::Int(600)),
    ]);
    let server = RawServer::start(Arc::new(move |_req, stream: &mut TcpStream| {
        write_json_resp(stream, 200, &body);
    }));
    let api = api_at(server.addr);
    let err = api.catalog_search(&CatalogQuery::browse(10), None).unwrap_err();
    assert!(matches!(err, ClientError::Protocol { what: "catalog page too large" }), "{err:?}");
}

// ---------------------------------------------------------------------------
// manifest integrity
// ---------------------------------------------------------------------------

#[test]
fn manifest_bytes_with_wrong_digest_refused() {
    // Server returns bytes that do NOT hash to the requested revision.
    let server = RawServer::start(Arc::new(|_req, stream: &mut TcpStream| {
        write_bytes_resp(stream, 200, "application/octet-stream", b"not the manifest", &[]);
    }));
    let api = api_at(server.addr);
    let rev = AssetRevisionId::from_bytes([9; 32]);
    let err = api.fetch_revision_bytes(&rev).unwrap_err();
    assert!(matches!(err, ClientError::DigestMismatch { .. }), "{err:?}");
}

/// Control plane that answers health + the connect probe honestly and serves
/// one fixed revision body; counts revision requests.
fn scripted_control(revision_body: Vec<u8>, hits: Arc<AtomicUsize>) -> RawServer {
    RawServer::start(Arc::new(move |req: ParsedRequest, stream: &mut TcpStream| {
        if req.target == "/v1/health" {
            write_json_resp(
                stream,
                200,
                &obj(vec![
                    ("server_id", s("ab".repeat(16))),
                    ("protocol_version", Value::Int(1)),
                ]),
            );
        } else if req.target.starts_with("/v1/assets") {
            write_json_resp(
                stream,
                200,
                &obj(vec![("assets", Value::Arr(vec![])), ("cursor", Value::Null)]),
            );
        } else if req.target.starts_with("/v1/revisions/") {
            hits.fetch_add(1, Ordering::Relaxed);
            write_bytes_resp(stream, 200, "application/octet-stream", &revision_body, &[]);
        } else {
            write_error(stream, 404, "not found");
        }
    }))
}

#[test]
fn digest_valid_but_non_canonical_manifest_refused_and_never_cached() {
    // Bytes whose digest MATCHES the requested revision but which are not a
    // canonical manifest: decode must refuse and the cache must stay clean.
    let garbage = payload(1234, 600);
    let rev = AssetRevisionId::hash_of(&garbage);
    let hits = Arc::new(AtomicUsize::new(0));
    let control = scripted_control(garbage, hits.clone());
    let endpoints = ApiEndpoints { control: control.addr, data: control.addr };
    let mut cfg = ClientConfig::new(test_root("noncanon"));
    cfg.http = fast_limits();
    let mut client = AssetClient::connect(cfg, endpoints, None).unwrap();

    let err = client.fetch_asset_manifest(&rev).unwrap_err();
    assert!(matches!(err, ClientError::Content(_)), "{err:?}");
    // Not cached: a second call must hit the network again.
    let before = hits.load(Ordering::Relaxed);
    let _ = client.fetch_asset_manifest(&rev).unwrap_err();
    assert_eq!(hits.load(Ordering::Relaxed), before + 1, "refused bytes were cached");
}

// ---------------------------------------------------------------------------
// blob integrity, size lies, resume hostility
// ---------------------------------------------------------------------------

/// Fixture control plane + scripted data plane.
fn client_with_data_plane(
    name: &str,
    data: &RawServer,
) -> (AssetClient, FixtureServer, std::path::PathBuf) {
    let fixture = FixtureServer::start(FixtureStore::default(), FixtureOptions::default());
    let endpoints = ApiEndpoints { control: fixture.control.addr, data: data.addr };
    let root = test_root(name);
    let mut cfg = ClientConfig::new(&root);
    cfg.http = fast_limits();
    cfg.max_transfer_attempts = 2;
    cfg.blob_body_deadline_ms = 700;
    let client = AssetClient::connect(cfg, endpoints, None).unwrap();
    (client, fixture, root)
}

#[test]
fn blob_wrong_bytes_never_committed() {
    let real = payload(77, 2_000);
    let blob = BlobId::hash_of(&real);
    let wrong = payload(78, 2_000);
    // Serves the WRONG bytes with a confident length.
    let data = {
        let wrong = wrong.clone();
        RawServer::start(Arc::new(move |_req, stream: &mut TcpStream| {
            write_bytes_resp(stream, 200, "application/octet-stream", &wrong, &[]);
        }))
    };
    let (mut client, _fixture, _root) = client_with_data_plane("wrongblob", &data);
    let err = client.fetch_blob(&blob, Some(2_000), None).unwrap_err();
    assert!(matches!(err, ClientError::DigestMismatch { .. }), "{err:?}");
    assert!(client.cached_blob(&blob).unwrap().is_none(), "bad bytes reached the cache");
}

/// `upload_blob_with_digest`/`upload_blob_batch_with_digests` let a caller
/// that already hashed the bytes (e.g. against an upload plan's expected
/// digest) skip a redundant local rehash on upload. That must never weaken
/// the identity guarantee: a server that echoes a DIFFERENT blob id than the
/// one the caller supplied is still refused, exactly like the always-hashes
/// `upload_blob`/`upload_blob_batch` path.
#[test]
fn upload_with_precomputed_digest_still_refuses_a_server_disagreement() {
    let bytes = payload(41, 3_000);
    let correct = BlobId::hash_of(&bytes);
    let wrong = BlobId::hash_of(&payload(42, 3_000));
    let data = RawServer::start(Arc::new(move |req: ParsedRequest, stream: &mut TcpStream| {
        // Echoes an identity that does NOT match what the caller sent — or
        // what it precomputed — on either the single or the batch route.
        if req.target.starts_with("/v1/blobs/batch") {
            let rows: Vec<Value> =
                (0..2).map(|_| obj(vec![("blob_id", s(wrong.to_string()))])).collect();
            write_json_resp(stream, 201, &obj(vec![("blobs", Value::Arr(rows))]));
        } else {
            write_json_resp(stream, 201, &obj(vec![("blob_id", s(wrong.to_string()))]));
        }
    }));
    let api = api_at(data.addr);

    // Single-blob path: the caller passes the CORRECT precomputed digest
    // (skipping its own local hash), but the server's reply still disagrees.
    let err = api.upload_blob_with_digest("ns", &bytes, correct).unwrap_err();
    assert!(matches!(err, ClientError::DigestMismatch { .. }), "{err:?}");

    // Batch path: same guarantee, N blobs at once.
    let bytes2 = payload(43, 1_000);
    let correct2 = BlobId::hash_of(&bytes2);
    let err = api
        .upload_blob_batch_with_digests(
            "ns",
            &[(correct, bytes.as_slice()), (correct2, bytes2.as_slice())],
        )
        .unwrap_err();
    assert!(matches!(err, ClientError::DigestMismatch { .. }), "{err:?}");
}

#[test]
fn blob_size_lie_refused_before_streaming() {
    let real = payload(80, 5_000);
    let blob = BlobId::hash_of(&real);
    let data = {
        let real = real.clone();
        RawServer::start(Arc::new(move |_req, stream: &mut TcpStream| {
            write_bytes_resp(stream, 200, "application/octet-stream", &real, &[]);
        }))
    };
    let (mut client, _fixture, _root) = client_with_data_plane("sizelie", &data);
    // The manifest says 4000; the server declares 5000: refuse on declaration.
    let err = client.fetch_blob(&blob, Some(4_000), None).unwrap_err();
    assert!(
        matches!(
            err,
            ClientError::SizeMismatch { what: "blob declared length", expected: 4_000, found: 5_000 }
        ),
        "{err:?}"
    );
}

#[test]
fn resume_with_wrong_content_range_start_refused() {
    let full = payload(90, 3_000);
    let blob = BlobId::hash_of(&full);
    let root = test_root("badresume");

    // Pre-seed a genuine 1000-byte partial (as if a prior run was cut off).
    {
        let mut cache = makepad_asset_client::ContentCache::open(
            &root,
            makepad_asset_client::CacheBudgets::default_v1(),
            now_ms(),
        )
        .unwrap();
        let mut w = cache.open_partial(blob.as_bytes()).unwrap();
        w.write(&full[..1000]).unwrap();
    }

    // Hostile 206: correct size but claims the range starts at 0.
    let data = {
        let full = full.clone();
        RawServer::start(Arc::new(move |_req, stream: &mut TcpStream| {
            let cr = format!("bytes 0-{}/{}", full.len() - 1, full.len());
            write_bytes_resp(
                stream,
                206,
                "application/octet-stream",
                &full,
                &[("Content-Range", cr.as_str())],
            );
        }))
    };
    let fixture = FixtureServer::start(FixtureStore::default(), FixtureOptions::default());
    let endpoints = ApiEndpoints { control: fixture.control.addr, data: data.addr };
    let mut cfg = ClientConfig::new(&root);
    cfg.http = fast_limits();
    let mut client = AssetClient::connect(cfg, endpoints, None).unwrap();

    let err = client.fetch_blob(&blob, Some(full.len() as u64), None).unwrap_err();
    assert!(
        matches!(err, ClientError::Protocol { what: "resume offset mismatch" }),
        "{err:?}"
    );
    assert!(client.cached_blob(&blob).unwrap().is_none());
}

#[test]
fn blob_stall_times_out_and_partial_survives() {
    let full = payload(95, 8_000);
    let blob = BlobId::hash_of(&full);
    // Sends the head plus 1000 bytes, then stalls well past the deadline.
    let data = {
        let full = full.clone();
        RawServer::start(Arc::new(move |_req, stream: &mut TcpStream| {
            let head = response_head(200, "application/octet-stream", full.len() as u64, &[]);
            write_raw(stream, head.as_bytes());
            write_raw(stream, &full[..1000]);
            std::thread::sleep(std::time::Duration::from_millis(2_500));
        }))
    };
    let root;
    {
        let (mut client, _fixture, r) = client_with_data_plane("stall", &data);
        root = r;
        let err = client.fetch_blob(&blob, Some(full.len() as u64), None).unwrap_err();
        assert!(matches!(err, ClientError::Timeout { .. }), "{err:?}");
        assert!(client.cached_blob(&blob).unwrap().is_none());
    }
    // The partial bytes survive for a later resume.
    let cache = makepad_asset_client::ContentCache::open(
        &root,
        makepad_asset_client::CacheBudgets::default_v1(),
        now_ms(),
    )
    .unwrap();
    assert!(cache.partial_len(blob.as_bytes()) > 0, "partial was lost");
}

#[test]
fn bogus_416_without_range_request_refused() {
    // Answers every blob GET with 416 even though no Range was sent.
    let data = RawServer::start(Arc::new(|_req, stream: &mut TcpStream| {
        let head = response_head(416, "application/json", 0, &[("Content-Range", "bytes */100")]);
        write_raw(stream, head.as_bytes());
    }));
    let (mut client, _fixture, _root) = client_with_data_plane("bogus416", &data);
    let blob = BlobId::hash_of(b"whatever");
    let err = client.fetch_blob(&blob, Some(100), None).unwrap_err();
    assert!(matches!(err, ClientError::Protocol { what: "unexpected 416" }), "{err:?}");
}

// ---------------------------------------------------------------------------
// identity, credentials, cursors
// ---------------------------------------------------------------------------

#[test]
fn server_identity_mismatch_refused() {
    let fixture = FixtureServer::start(FixtureStore::default(), FixtureOptions::default());
    let mut cfg = ClientConfig::new(test_root("identity"));
    cfg.http = fast_limits();
    let err = AssetClient::connect(cfg, fixture.endpoints(), Some([0x11; 16])).unwrap_err();
    assert!(matches!(err, ClientError::ServerIdentityMismatch { .. }), "{err:?}");
}

#[test]
fn unsupported_protocol_version_refused() {
    let options = FixtureOptions { health_protocol_version: 99, ..FixtureOptions::default() };
    let fixture = FixtureServer::start(FixtureStore::default(), options);
    let mut cfg = ClientConfig::new(test_root("protover"));
    cfg.http = fast_limits();
    let err = AssetClient::connect(cfg, fixture.endpoints(), None).unwrap_err();
    assert!(
        matches!(err, ClientError::Protocol { what: "server protocol version unsupported" }),
        "{err:?}"
    );
}

#[test]
fn auth_required_paths_refuse_up_front() {
    // TLS-flagged candidate refused.
    let d = makepad_asset_client::DiscoveredServer {
        server_id: [1; 16],
        protocol_version: 1,
        ip: "127.0.0.1".parse().unwrap(),
        control_port: 1,
        data_port: 2,
        auth_required: false,
        tls: true,
        capability_bits: makepad_asset_client::wire::caps::ALL_V1,
        last_seen_ms: 0,
    };
    let mut cfg = ClientConfig::new(test_root("tls"));
    cfg.http = fast_limits();
    assert!(matches!(
        AssetClient::connect_discovered(cfg, &d).unwrap_err(),
        ClientError::TlsUnsupported
    ));
    // Auth-required candidate without a token refused before any I/O.
    let d2 = makepad_asset_client::DiscoveredServer { tls: false, auth_required: true, ..d };
    let mut cfg = ClientConfig::new(test_root("notoken"));
    cfg.http = fast_limits();
    assert!(matches!(
        AssetClient::connect_discovered(cfg, &d2).unwrap_err(),
        ClientError::Unauthenticated
    ));
}

#[test]
fn wrong_credential_is_uniformly_unauthenticated() {
    let token = format!("mpat_{}", "cd".repeat(32));
    let options = FixtureOptions { auth_token: Some(token), ..FixtureOptions::default() };
    let fixture = FixtureServer::start(FixtureStore::default(), options);
    // No token configured → the connect probe hits 401.
    let mut cfg = ClientConfig::new(test_root("badcred"));
    cfg.http = fast_limits();
    let err = AssetClient::connect(cfg, fixture.endpoints(), None).unwrap_err();
    assert!(matches!(err, ClientError::Unauthenticated), "{err:?}");
    // A wrong (but well-shaped) token → same refusal.
    let mut cfg = ClientConfig::new(test_root("badcred2"));
    cfg.http = fast_limits();
    cfg.token = Some(format!("mpat_{}", "ee".repeat(32)));
    let err = AssetClient::connect(cfg, fixture.endpoints(), None).unwrap_err();
    assert!(matches!(err, ClientError::Unauthenticated), "{err:?}");
}

#[test]
fn cursor_from_another_server_refused_locally() {
    let mut store_a = FixtureStore::default();
    for i in 0..6u8 {
        store_a.add_prop(i + 1, "stock", None, &format!("Rocket {i}"), payload(i as u64, 300), vec![]);
    }
    let a = FixtureServer::start(store_a, FixtureOptions { server_id: [0xaa; 16], ..FixtureOptions::default() });
    let b = FixtureServer::start(FixtureStore::default(), FixtureOptions { server_id: [0xbb; 16], ..FixtureOptions::default() });

    let mut cfg = ClientConfig::new(test_root("cursor_a"));
    cfg.http = fast_limits();
    let client_a = AssetClient::connect(cfg, a.endpoints(), None).unwrap();
    let mut cfg = ClientConfig::new(test_root("cursor_b"));
    cfg.http = fast_limits();
    let client_b = AssetClient::connect(cfg, b.endpoints(), None).unwrap();

    let query = CatalogQuery::text("rocket", 2);
    let page = client_a.catalog_search(&query, None).unwrap();
    let cursor = page.next.expect("more pages exist");

    let baseline = b.log.count("POST", "/v1/catalog");
    let err = client_b.catalog_search(&query, Some(&cursor)).unwrap_err();
    assert!(matches!(err, ClientError::WrongServerCursor), "{err:?}");
    assert_eq!(
        b.log.count("POST", "/v1/catalog"),
        baseline,
        "foreign cursor must be refused before any request"
    );
}
