#![cfg(not(target_arch = "wasm32"))]

use makepad_flow::host::{FlowServer, FlowServerConfig, ServerError};
use makepad_flow::{
    graph, EventsResponse, FlowMutationResponse, FlowResponse, FlowSummary, HealthResponse,
    NodesResponse, PutGraphRequest, PutSourceRequest, RevertRequest,
};
use makepad_micro_serde::{DeJson, JsonValue, SerJson};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "makepad-flow-host-{}-{}-{nonce}",
            std::process::id(),
            label
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct HttpResponse {
    status: u16,
    headers: String,
    body: String,
}

fn request(
    address: SocketAddr,
    method: &str,
    target: &str,
    token: Option<&str>,
    body: &str,
) -> HttpResponse {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(35))).unwrap();
    let authorization = token
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    write!(
        stream,
        "{method} {target} HTTP/1.1\r\nHost: localhost\r\n{authorization}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let status = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    HttpResponse { status, headers: head.to_string(), body: body.to_string() }
}

fn start(root: &Path) -> FlowServer {
    let mut config = FlowServerConfig::new(root.to_path_buf());
    config.watch_interval_ms = 25;
    config.log = Box::new(|_| {});
    FlowServer::start(config).unwrap()
}

fn source_request(source: &str) -> String {
    PutSourceRequest { source: source.to_string() }.serialize_json()
}

fn cursor(address: SocketAddr, token: &str) -> String {
    let response = request(address, "GET", "/v1/events", Some(token), "");
    assert_eq!(response.status, 200, "{}", response.body);
    EventsResponse::deserialize_json(&response.body).unwrap().cursor
}

fn poll(address: SocketAddr, token: &str, cursor: &str, suffix: &str) -> EventsResponse {
    let response = request(
        address,
        "GET",
        &format!("/v1/events?cursor={cursor}{suffix}"),
        Some(token),
        "",
    );
    assert_eq!(response.status, 200, "{}", response.body);
    EventsResponse::deserialize_json(&response.body).unwrap()
}

#[test]
fn startup_metadata_lock_and_restart() {
    let root = TempRoot::new("startup");
    let server = start(&root.0);
    let endpoints = server.endpoints();

    let health = request(endpoints.control, "GET", "/v1/health", None, "");
    assert_eq!(health.status, 200);
    assert!(health.headers.contains("X-Content-Type-Options: nosniff"));
    let health = HealthResponse::deserialize_json(&health.body).unwrap();
    assert_eq!(health.protocol_version, 1);
    assert_eq!(health.server_id.len(), 32);

    let mut second_config = FlowServerConfig::new(root.0.clone());
    second_config.log = Box::new(|_| {});
    assert!(matches!(FlowServer::start(second_config), Err(ServerError::Locked)));
    assert!(root.0.join("server.lock").is_file());
    assert_eq!(std::fs::read_to_string(root.0.join("listen")).unwrap().trim().split(':').count(), 3);
    assert_eq!(std::fs::read_to_string(root.0.join("server-id")).unwrap().trim().len(), 32);
    let token = std::fs::read_to_string(root.0.join("token")).unwrap();
    assert!(token.trim().starts_with("mpft_"));
    assert_eq!(token.trim().len(), 69);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(root.0.join("token")).unwrap().permissions().mode() & 0o777, 0o600);
    }

    server.shutdown();
    let restarted = start(&root.0);
    assert_eq!(restarted.endpoints().server_id, endpoints.server_id);
    restarted.shutdown();
}

#[test]
fn bearer_auth_and_empty_data_plane() {
    let root = TempRoot::new("auth");
    let server = start(&root.0);
    let endpoints = server.endpoints();
    assert_eq!(request(endpoints.control, "GET", "/v1/flows", None, "").status, 401);
    assert_eq!(
        request(endpoints.control, "GET", "/v1/flows", Some("mpft_wrong"), "").status,
        401
    );
    let ok = request(endpoints.control, "GET", "/v1/flows", Some(&endpoints.token), "");
    assert_eq!(ok.status, 200);
    assert_eq!(ok.body, "[]");
    assert_eq!(request(endpoints.data, "GET", "/v1/values/x", None, "").status, 401);
    // "x" is not a 64-hex sha256 digest; F2's value route rejects the shape
    // before it ever asks the state thread whether the value exists.
    assert_eq!(
        request(endpoints.data, "GET", "/v1/values/x", Some(&endpoints.token), "").status,
        400
    );
    let missing_digest = "0".repeat(64);
    assert_eq!(
        request(
            endpoints.data,
            "GET",
            &format!("/v1/values/{missing_digest}"),
            Some(&endpoints.token),
            "",
        )
        .status,
        404
    );
    server.shutdown();
}

#[test]
fn put_error_keeps_last_good_and_revert_restores_source() {
    let root = TempRoot::new("put");
    let server = start(&root.0);
    let endpoints = server.endpoints();
    let source = include_str!("fixtures/prompt_image.splash");
    let put = request(
        endpoints.control,
        "PUT",
        "/v1/flows/demo",
        Some(&endpoints.token),
        &source_request(source),
    );
    assert_eq!(put.status, 200, "{}", put.body);
    let put = FlowMutationResponse::deserialize_json(&put.body).unwrap();
    assert_eq!(put.revision, 1);
    assert_eq!(put.graph.nodes.len(), 5);

    let rows = request(endpoints.control, "GET", "/v1/flows", Some(&endpoints.token), "");
    let rows = Vec::<FlowSummary>::deserialize_json(&rows.body).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, "ok");

    let broken_source = "use mod.flow.*\nlet nope = Image{ width: }\nFlow{nope}\n";
    let broken = request(
        endpoints.control,
        "PUT",
        "/v1/flows/demo",
        Some(&endpoints.token),
        &source_request(broken_source),
    );
    assert_eq!(broken.status, 422, "{}", broken.body);
    assert!(broken.body.contains("\"line\":"));
    assert!(broken.body.contains("\"col\":"));

    let get = request(endpoints.control, "GET", "/v1/flows/demo", Some(&endpoints.token), "");
    let get = FlowResponse::deserialize_json(&get.body).unwrap();
    assert_eq!(get.source, broken_source);
    assert_eq!(get.revision, 1);
    assert_eq!(get.graph.unwrap().nodes.len(), 5);
    assert!(get.error.is_some());

    let revert = request(
        endpoints.control,
        "POST",
        "/v1/flows/demo/revert",
        Some(&endpoints.token),
        &RevertRequest { revision: 1 }.serialize_json(),
    );
    assert_eq!(revert.status, 200, "{}", revert.body);
    let revert = FlowMutationResponse::deserialize_json(&revert.body).unwrap();
    assert_eq!(revert.revision, 2);
    assert_eq!(revert.graph.nodes.len(), 5);
    server.shutdown();
}

#[test]
fn watcher_emits_changed_changed_removed() {
    let root = TempRoot::new("watcher");
    let server = start(&root.0);
    let endpoints = server.endpoints();
    let mut next = cursor(endpoints.control, &endpoints.token);
    let path = root.0.join("flows/watched.splash");

    std::fs::write(&path, "use mod.flow.*\nFlow{label: \"one\"}\n").unwrap();
    let first = poll(endpoints.control, &endpoints.token, &next, "&wait=1000&topic=flows");
    assert_eq!(first.events.len(), 1);
    assert_eq!(first.events[0].key("kind").and_then(JsonValue::string).map(String::as_str), Some("flow.changed"));
    next = first.cursor;

    std::fs::write(&path, "use mod.flow.*\nFlow{label: \"second label\"}\n").unwrap();
    let second = poll(endpoints.control, &endpoints.token, &next, "&wait=1000&topic=flows");
    assert_eq!(second.events.len(), 1);
    assert_eq!(second.events[0].key("kind").and_then(JsonValue::string).map(String::as_str), Some("flow.changed"));
    next = second.cursor;

    std::fs::remove_file(path).unwrap();
    let removed = poll(endpoints.control, &endpoints.token, &next, "&wait=1000&topic=flows");
    assert_eq!(removed.events.len(), 1);
    assert_eq!(removed.events[0].key("kind").and_then(JsonValue::string).map(String::as_str), Some("flow.removed"));
    server.shutdown();
}

#[test]
fn events_resume_limit_wait_cap_and_restart_gap() {
    let root = TempRoot::new("events");
    let mut config = FlowServerConfig::new(root.0.clone());
    config.watch_interval_ms = 25;
    config.event_max_waiters = 1;
    config.log = Box::new(|_| {});
    let server = FlowServer::start(config).unwrap();
    let endpoints = server.endpoints();
    let initial = cursor(endpoints.control, &endpoints.token);
    for name in ["a", "b", "c"] {
        let source = format!("use mod.flow.*\nFlow{{label: \"{name}\"}}\n");
        let response = request(
            endpoints.control,
            "PUT",
            &format!("/v1/flows/{name}"),
            Some(&endpoints.token),
            &source_request(&source),
        );
        assert_eq!(response.status, 200);
    }
    let first = poll(endpoints.control, &endpoints.token, &initial, "&limit=2");
    assert_eq!(first.events.len(), 2);
    let second = poll(endpoints.control, &endpoints.token, &first.cursor, "&limit=2");
    assert_eq!(second.events.len(), 1);
    let third = poll(endpoints.control, &endpoints.token, &second.cursor, "&limit=2");
    assert!(third.events.is_empty());

    let wait_cursor = third.cursor.clone();
    let wait_address = endpoints.control;
    let wait_token = endpoints.token.clone();
    let first_waiter = std::thread::spawn(move || {
        poll(wait_address, &wait_token, &wait_cursor, "&wait=2000")
    });
    std::thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    let over_cap = poll(endpoints.control, &endpoints.token, &third.cursor, "&wait=2000");
    assert!(over_cap.events.is_empty());
    assert!(started.elapsed() < Duration::from_millis(750));

    let wake = request(
        endpoints.control,
        "PUT",
        "/v1/flows/wake",
        Some(&endpoints.token),
        &source_request("use mod.flow.*\nFlow{label: \"wake\"}\n"),
    );
    assert_eq!(wake.status, 200);
    let woke = first_waiter.join().unwrap();
    assert_eq!(woke.events.len(), 1);
    let stale = woke.cursor;
    server.shutdown();

    let restarted = start(&root.0);
    let endpoints = restarted.endpoints();
    let gap = poll(endpoints.control, &endpoints.token, &stale, "");
    assert!(gap.gap);
    restarted.shutdown();
}

#[test]
fn nodes_catalog_uses_documented_image_range() {
    let root = TempRoot::new("nodes");
    let server = start(&root.0);
    let endpoints = server.endpoints();
    let response = request(endpoints.control, "GET", "/v1/nodes", Some(&endpoints.token), "");
    assert_eq!(response.status, 200, "{}", response.body);
    let catalog = NodesResponse::deserialize_json(&response.body).unwrap();
    let image = catalog.types.iter().find(|node| node.type_name == "Image").unwrap();
    let width = image.params.iter().find(|param| param.name == "width").unwrap();
    let range = width.range.as_ref().unwrap();
    assert_eq!(range.min, 256.0);
    assert_eq!(range.max, 2048.0);
    assert_eq!(range.step, Some(64.0));
    server.shutdown();
}

#[test]
fn graph_put_and_delete_routes_round_trip() {
    let root = TempRoot::new("graph");
    let server = start(&root.0);
    let endpoints = server.endpoints();
    let graph = graph::evaluate("use mod.flow.*\nFlow{label: \"graph\"}\n", "graph.splash").unwrap();
    let put = request(
        endpoints.control,
        "PUT",
        "/v1/flows/from-graph/graph",
        Some(&endpoints.token),
        &PutGraphRequest { graph }.serialize_json(),
    );
    assert_eq!(put.status, 200, "{}", put.body);
    assert!(root.0.join("flows/from-graph.splash").is_file());
    let delete = request(
        endpoints.control,
        "DELETE",
        "/v1/flows/from-graph",
        Some(&endpoints.token),
        "",
    );
    assert_eq!(delete.status, 204, "{}", delete.body);
    assert!(!root.0.join("flows/from-graph.splash").exists());
    server.shutdown();
}
