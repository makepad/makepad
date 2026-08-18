//! Security-focused tests: credential uniformity, capability scoping and
//! delegation limits, worker identity binding, the namespace claim gate,
//! hostile HTTP input, upload refusals, and discovery-plane hostility.

mod common;

use common::*;
use makepad_asset_store::discovery::{caps, Beacon, DiscoveryListener, PROTOCOL_VERSION};
use makepad_asset_store::json::Value;
use makepad_asset_data::BlobId;
use std::net::UdpSocket;

// ---------------------------------------------------------------------------
// credentials
// ---------------------------------------------------------------------------

#[test]
fn refused_credentials_are_uniform() {
    let ts = start_server("uniform401");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));

    // A real token that will be expired, and one that will be revoked.
    let victim = {
        let r = admin.post_json("/v1/auth/principals", &jobj(vec![("name", jstr("victim"))]));
        r.str_field("principal")
    };
    let short = admin
        .post_json(
            "/v1/auth/tokens",
            &jobj(vec![("principal", jstr(victim.clone())), ("ttl_ms", Value::Int(50))]),
        )
        .str_field("token");
    let revoked = admin
        .post_json("/v1/auth/tokens", &jobj(vec![("principal", jstr(victim))]))
        .str_field("token");
    let r = admin.post_json("/v1/auth/tokens/revoke", &jobj(vec![("token", jstr(revoked.clone()))]));
    assert_eq!(r.status, 204);
    std::thread::sleep(std::time::Duration::from_millis(80)); // expire `short`

    let unknown = format!("mpat_{}", "ab".repeat(32));
    let cases: Vec<Option<String>> = vec![
        None,                                  // no header
        Some("not-a-token".into()),            // malformed shape
        Some("mpat_zz".into()),                // malformed hex
        Some(unknown),                         // valid shape, unknown
        Some(short),                           // expired
        Some(revoked),                         // revoked
    ];
    let mut bodies = Vec::new();
    for case in cases {
        let mut c = ts.control(case.as_deref());
        let r = c.get("/v1/auth/whoami");
        assert_eq!(r.status, 401);
        assert_eq!(r.header("WWW-Authenticate"), Some("Bearer"));
        bodies.push(r.body);
    }
    // Every refusal is byte-identical: no oracle distinguishes unknown,
    // malformed, expired and revoked credentials.
    for b in &bodies[1..] {
        assert_eq!(b, &bodies[0]);
    }
}

// ---------------------------------------------------------------------------
// capability scoping
// ---------------------------------------------------------------------------

#[test]
fn capability_scoping_and_revocation() {
    let ts = start_server("scoping");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    let scoped = principal_with(&mut admin, &[("asset_register", "demo")]);
    let mut c = ts.control(Some(&scoped));

    // In-scope namespace works; any other namespace is denied with the
    // capability named.
    let r = c.post_json("/v1/assets", &jobj(vec![("namespace", jstr("demo"))]));
    assert_eq!(r.status, 201);
    let asset_id = r.str_field("asset_id");
    let r = c.post_json("/v1/assets", &jobj(vec![("namespace", jstr("other"))]));
    assert_eq!(r.status, 403);
    assert_eq!(r.json().get("capability").unwrap().as_str(), Some("asset_register"));

    // A capability the principal never had is denied even in-scope.
    let fake_rev = format!("arev_{}", "0".repeat(64));
    let r = c.post_json(
        &format!("/v1/assets/{asset_id}/revisions/{fake_rev}/publish"),
        &jobj(vec![]),
    );
    assert_eq!(r.status, 403);
    assert_eq!(r.json().get("capability").unwrap().as_str(), Some("asset_publish"));

    // Revocation takes effect on the next request.
    let whoami = c.get("/v1/auth/whoami").str_field("principal");
    let r = admin.post_json(
        "/v1/auth/grants/revoke",
        &jobj(vec![
            ("principal", jstr(whoami)),
            ("capability", jstr("asset_register")),
            ("scope", jstr("demo")),
        ]),
    );
    assert_eq!(r.status, 204);
    let r = c.post_json("/v1/assets", &jobj(vec![("namespace", jstr("demo"))]));
    assert_eq!(r.status, 403);
}

#[test]
fn auth_admin_delegation_is_namespace_bounded() {
    let ts = start_server("delegation");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    let ns_admin = principal_with(&mut admin, &[("auth_admin", "demo")]);
    let subject = principal_with(&mut admin, &[]);
    let mut c = ts.control(Some(&ns_admin));
    let subject_principal = {
        let mut s = ts.control(Some(&subject));
        s.get("/v1/auth/whoami").str_field("principal")
    };

    // A namespace-scoped auth admin may grant within its namespace...
    let r = c.post_json(
        "/v1/auth/grants",
        &jobj(vec![
            ("principal", jstr(subject_principal.clone())),
            ("capability", jstr("blob_write")),
            ("scope", jstr("demo")),
        ]),
    );
    assert_eq!(r.status, 204);
    // ...but not in another namespace, not with wildcard scope, and it may
    // not mint principals or tokens (root-only).
    let r = c.post_json(
        "/v1/auth/grants",
        &jobj(vec![
            ("principal", jstr(subject_principal.clone())),
            ("capability", jstr("blob_write")),
            ("scope", jstr("other")),
        ]),
    );
    assert_eq!(r.status, 403);
    let r = c.post_json(
        "/v1/auth/grants",
        &jobj(vec![
            ("principal", jstr(subject_principal.clone())),
            ("capability", jstr("blob_write")),
            ("scope", jstr("*")),
        ]),
    );
    assert_eq!(r.status, 403);
    assert_eq!(
        c.post_json("/v1/auth/principals", &jobj(vec![("name", jstr("sneaky"))])).status,
        403
    );
    let r = c.post_json(
        "/v1/auth/tokens",
        &jobj(vec![("principal", jstr(subject_principal))]),
    );
    assert_eq!(r.status, 403);

    // The bootstrap admin principal cannot be disabled.
    let root_principal = admin.get("/v1/auth/whoami").str_field("principal");
    let r = admin.post_json(&format!("/v1/auth/principals/{root_principal}/disable"), &jobj(vec![]));
    assert_eq!(r.status, 409);
}

// ---------------------------------------------------------------------------
// jobs: claim gate + identity binding + visibility
// ---------------------------------------------------------------------------

#[test]
fn worker_claim_gate_covers_every_namespace() {
    let ts = start_server("claimgate");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    let enq = principal_with(&mut admin, &[("job_enqueue", "demo"), ("job_enqueue", "other")]);
    let mut enqueuer = ts.control(Some(&enq));
    for ns in ["demo", "other"] {
        let r = enqueuer.post_json(
            "/v1/jobs",
            &jobj(vec![
                ("namespace", jstr(ns)),
                ("kind", jstr("work")),
                ("body", jobj(vec![])),
            ]),
        );
        assert_eq!(r.status, 201);
    }
    // A worker scoped to one namespace cannot claim while another namespace
    // has jobs: the core claim is namespace-blind, so the gate must cover
    // the whole routed set.
    let scoped_worker = principal_with(&mut admin, &[("job_worker", "demo")]);
    let mut w = ts.control(Some(&scoped_worker));
    let r = w.post_json("/v1/worker/claim", &jobj(vec![("lease_ms", Value::Int(10_000))]));
    assert_eq!(r.status, 403);
    assert_eq!(r.json().get("capability").unwrap().as_str(), Some("job_worker"));
    // Granting the missing namespace opens the gate.
    let wp = w.get("/v1/auth/whoami").str_field("principal");
    let r = admin.post_json(
        "/v1/auth/grants",
        &jobj(vec![
            ("principal", jstr(wp)),
            ("capability", jstr("job_worker")),
            ("scope", jstr("other")),
        ]),
    );
    assert_eq!(r.status, 204);
    let r = w.post_json("/v1/worker/claim", &jobj(vec![("lease_ms", Value::Int(10_000))]));
    assert_eq!(r.status, 200);
    assert!(r.json().get("job").unwrap().as_str().is_some());
}

#[test]
fn worker_identity_binds_to_principal() {
    let ts = start_server("workerident");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    let enq = principal_with(&mut admin, &[("job_enqueue", "demo")]);
    let w1 = principal_with(&mut admin, &[("job_worker", "demo")]);
    let w2 = principal_with(&mut admin, &[("job_worker", "demo")]);
    let mut enqueuer = ts.control(Some(&enq));
    let mut worker1 = ts.control(Some(&w1));
    let mut worker2 = ts.control(Some(&w2));

    let r = enqueuer.post_json(
        "/v1/jobs",
        &jobj(vec![("namespace", jstr("demo")), ("kind", jstr("work")), ("body", jobj(vec![]))]),
    );
    let job = r.str_field("job");
    let r = worker1.post_json(
        "/v1/worker/claim",
        &jobj(vec![("lease_ms", Value::Int(60_000)), ("suffix", jstr("t0"))]),
    );
    assert_eq!(r.json().get("job").unwrap().as_str(), Some(job.as_str()));

    // The worker string is derived from the authenticated principal: another
    // principal cannot succeed, heartbeat, or fail this lease even with the
    // same suffix, and the holder must present the same suffix.
    let refuse = |client: &mut Client, suffix: Option<&str>| {
        let mut pairs = vec![("job", jstr(job.clone()))];
        if let Some(sfx) = suffix {
            pairs.push(("suffix", jstr(sfx)));
        }
        let r = client.post_json("/v1/worker/succeed", &jobj(pairs));
        assert_eq!(r.status, 409, "foreign lease report must refuse");
    };
    refuse(&mut worker2, Some("t0"));
    refuse(&mut worker1, Some("t1"));
    refuse(&mut worker1, None);
    let r = worker1.post_json(
        "/v1/worker/succeed",
        &jobj(vec![("job", jstr(job.clone())), ("suffix", jstr("t0"))]),
    );
    assert_eq!(r.status, 200);
}

#[test]
fn job_visibility_is_tenant_scoped() {
    let ts = start_server("jobvis");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    let enq = principal_with(&mut admin, &[("job_enqueue", "demo")]);
    let outsider = principal_with(&mut admin, &[("job_enqueue", "elsewhere")]);
    let mut enqueuer = ts.control(Some(&enq));
    let mut out = ts.control(Some(&outsider));
    let r = enqueuer.post_json(
        "/v1/jobs",
        &jobj(vec![("namespace", jstr("demo")), ("kind", jstr("secret")), ("body", jobj(vec![]))]),
    );
    let job = r.str_field("job");
    // The outsider gets the same 404 an unknown job id gets — existence is
    // not disclosed across tenants.
    let r = out.get(&format!("/v1/jobs/{job}"));
    assert_eq!(r.status, 404);
    let bogus = format!("job_{}", "00".repeat(16));
    let r2 = out.get(&format!("/v1/jobs/{bogus}"));
    assert_eq!(r2.status, 404);
    assert_eq!(r.body, r2.body);
    // Cancel without the capability refuses too (as a 404, same shape).
    let r = out.post_json(&format!("/v1/jobs/{job}/cancel"), &jobj(vec![]));
    assert_eq!(r.status, 403);
}

// ---------------------------------------------------------------------------
// hostile HTTP
// ---------------------------------------------------------------------------

#[test]
fn hostile_http_input_is_refused() {
    let ts = start_server("hostile");
    let addr = ts.server.control_addr();

    // Request smuggling and framing hostility.
    let cases: Vec<(&[u8], u16)> = vec![
        (b"POST /v1/catalog HTTP/1.1\r\nHost: x\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n", 400),
        (b"POST /v1/catalog HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: gzip, chunked\r\n\r\n", 501),
        (b"GET /v1/health HTTP/1.0\r\nHost: x\r\n\r\n", 505),
        (b"BREW /v1/health HTTP/1.1\r\nHost: x\r\n\r\n", 501),
        (b"GET /v1/health HTTP/1.1\r\n\r\n", 400),
        (b"GET /v1/health HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n", 400),
        (b"GET /v1/%2e%2e/secrets HTTP/1.1\r\nHost: x\r\n\r\n", 400),
        (b"GET /v1/../etc/passwd HTTP/1.1\r\nHost: x\r\n\r\n", 400),
        (b"GET /v1//health HTTP/1.1\r\nHost: x\r\n\r\n", 400),
        (b"GET /v1/health?a=1&a=2 HTTP/1.1\r\nHost: x\r\n\r\n", 400),
        (b"GET /v1/health HTTP/1.1\r\nHost: x\r\nBad: a\x01b\r\n\r\n", 400),
        (b"GET /v1/health HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\n", 400),
    ];
    for (raw, want) in cases {
        let resp = raw_roundtrip(addr, raw);
        assert_eq!(status_of(&resp), want, "{}", String::from_utf8_lossy(raw));
        let text = String::from_utf8_lossy(&resp);
        assert!(text.contains("Connection: close"), "hostile input closes");
        assert!(text.contains("X-Content-Type-Options: nosniff"));
    }

    // Oversized head: refused with 431.
    let mut big = b"GET /v1/health HTTP/1.1\r\nHost: x\r\n".to_vec();
    big.extend_from_slice(format!("X-Filler: {}\r\n", "a".repeat(40 * 1024)).as_bytes());
    big.extend_from_slice(b"\r\n");
    assert_eq!(status_of(&raw_roundtrip(addr, &big)), 431);

    // Chunked body over the JSON budget: 413, connection closed.
    let token = ts.admin_token();
    let mut chunked = format!(
        "POST /v1/catalog HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nTransfer-Encoding: chunked\r\n\r\n"
    )
    .into_bytes();
    let chunk = vec![b'a'; 64 * 1024];
    for _ in 0..6 {
        chunked.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        chunked.extend_from_slice(&chunk);
        chunked.extend_from_slice(b"\r\n");
    }
    chunked.extend_from_slice(b"0\r\n\r\n");
    assert_eq!(status_of(&raw_roundtrip(addr, &chunked)), 413);

    // Chunk-extension and non-hex size lines are malformed.
    let bad_chunk = format!(
        "POST /v1/catalog HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nTransfer-Encoding: chunked\r\n\r\n5;ext=1\r\nhello\r\n0\r\n\r\n"
    );
    assert_eq!(status_of(&raw_roundtrip(addr, bad_chunk.as_bytes())), 400);

    // Pipelined requests on one connection are answered in order.
    let two = b"GET /v1/health HTTP/1.1\r\nHost: x\r\n\r\nGET /v1/health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n";
    let resp = raw_roundtrip(addr, two);
    let text = String::from_utf8_lossy(&resp);
    assert_eq!(text.matches("HTTP/1.1 200").count(), 2);
}

// ---------------------------------------------------------------------------
// upload refusals
// ---------------------------------------------------------------------------

#[test]
fn blob_upload_refusals_leave_no_state() {
    // Shrunken blob budget makes over-budget cheap to hit.
    let ts = start_server_with("uploadrefuse", |cfg| {
        cfg.budgets.max_blob_bytes = 4096;
    });
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    let writer = principal_with(&mut admin, &[("blob_write", "demo")]);
    let mut data = ts.data(Some(&writer));

    // Over budget: refused, and the digest stays unknown afterwards.
    let big = vec![7u8; 8192];
    let r = data.post_bytes("/v1/blobs?ns=demo", &big);
    assert_eq!(r.status, 413);
    let id = BlobId::hash_of(&big);
    assert_eq!(data.get(&format!("/v1/blobs/{id}")).status, 404);

    // Declared-digest mismatch: refused, nothing recorded.
    let payload = b"honest bytes".to_vec();
    let wrong = "1".repeat(64);
    let r = data.request(
        "POST",
        &format!("/v1/blobs?ns=demo&sha256={wrong}"),
        &[],
        Some(&payload),
    );
    assert_eq!(r.status, 422);
    let id = BlobId::hash_of(&payload);
    assert_eq!(data.get(&format!("/v1/blobs/{id}")).status, 404);

    // Missing namespace and missing capability refuse before any byte is
    // admitted.
    assert_eq!(data.post_bytes("/v1/blobs", &payload).status, 400);
    let outsider = principal_with(&mut admin, &[]);
    let mut noperm = ts.data(Some(&outsider));
    let r = noperm.post_bytes("/v1/blobs?ns=demo", &payload);
    assert_eq!(r.status, 403);

    // Unauthenticated data plane: refused uniformly.
    let mut anon = ts.data(None);
    assert_eq!(anon.post_bytes("/v1/blobs?ns=demo", &payload).status, 401);
    assert_eq!(anon.get(&format!("/v1/blobs/{id}")).status, 401);
}

// ---------------------------------------------------------------------------
// connection capacity
// ---------------------------------------------------------------------------

#[test]
fn over_capacity_connections_get_an_explicit_503() {
    let ts = start_server_with("capacity", |cfg| {
        cfg.control_max_conns = 2;
    });
    // Two idle keep-alive connections occupy the whole plane.
    let mut c1 = ts.control(None);
    let mut c2 = ts.control(None);
    assert_eq!(c1.get("/v1/health").status, 200);
    assert_eq!(c2.get("/v1/health").status, 200);
    // The third connection is refused immediately and explicitly — no silent
    // backlog starvation behind idle keep-alives.
    let resp = raw_roundtrip(
        ts.server.control_addr(),
        b"GET /v1/health HTTP/1.1\r\nHost: x\r\n\r\n",
    );
    assert_eq!(status_of(&resp), 503);
    // The occupied connections still work, and freeing one readmits others.
    assert_eq!(c1.get("/v1/health").status, 200);
    drop(c2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let resp = raw_roundtrip(
            ts.server.control_addr(),
            b"GET /v1/health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        );
        if status_of(&resp) == 200 {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "slot never freed");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------------------
// discovery hostility
// ---------------------------------------------------------------------------

#[test]
fn discovery_listener_survives_hostile_floods() {
    let mut listener = DiscoveryListener::start(0, 60_000).expect("listener");
    let port = listener.port();
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let dest = ("127.0.0.1", port);

    // Garbage of every shape: ignored.
    sock.send_to(b"", dest).unwrap();
    sock.send_to(b"short", dest).unwrap();
    sock.send_to(&[0u8; 36], dest).unwrap();
    sock.send_to(&[0xffu8; 512], dest).unwrap();
    let mut zero_port = Beacon {
        protocol_version: PROTOCOL_VERSION,
        server_id: [1; 16],
        control_port: 0,
        data_port: 1,
        auth_required: true,
        tls: false,
        capability_bits: caps::ALL_V1,
    };
    sock.send_to(&zero_port.encode(), dest).unwrap();
    zero_port.control_port = 1;
    zero_port.protocol_version = 0;
    sock.send_to(&zero_port.encode(), dest).unwrap();

    // A flood of distinct identities cannot grow the cache past its bound.
    for i in 0..300u32 {
        let mut id = [0u8; 16];
        id[..4].copy_from_slice(&i.to_be_bytes());
        let b = Beacon {
            protocol_version: PROTOCOL_VERSION,
            server_id: id,
            control_port: 9701,
            data_port: 9702,
            auth_required: true,
            tls: false,
            capability_bits: caps::ALL_V1,
        };
        sock.send_to(&b.encode(), dest).unwrap();
    }
    // Give the receive thread time to drain the socket.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let n = listener.snapshot(makepad_asset_store::util::now_ms()).len();
        assert!(n <= DiscoveryListener::MAX_ENTRIES, "cache bounded");
        if n == DiscoveryListener::MAX_ENTRIES || std::time::Instant::now() >= deadline {
            assert!(n > 0, "well-formed beacons were received");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    listener.stop();
}
