//! Adversarial integration battery for Studio's streamable-HTTP MCP endpoint.
//!
//! Real loopback sockets, `std::net::TcpStream` clients, no extra crates.

use makepad_director::mcp::*;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Fake {
    huge: bool,
    calls: Mutex<Vec<String>>,
}

impl ToolDispatcher for Fake {
    fn list(&self, caller: &LaneCaller) -> Vec<ToolDef> {
        vec![ToolDef::new(
            "code_outline",
            format!("outline for {}", caller.lane),
            r#"{"type":"object","properties":{"scope":{"type":"string"}},"additionalProperties":false}"#,
            Risk::Read,
        )]
    }

    fn call(&self, caller: &LaneCaller, name: &str, args: &Value, request_id: &str) -> Outcome {
        self.calls.lock().expect("fake dispatcher poisoned").push(name.to_string());
        if self.huge {
            return Outcome { text: "x".repeat(20 * 1024), is_error: false };
        }
        if name != "code_outline" {
            return Outcome { text: format!("unknown tool {name}"), is_error: true };
        }
        let scope = args.get("scope").and_then(Value::as_str).unwrap_or("");
        Outcome {
            text: format!(
                r#"{{"lane":"{}","tool":"{name}","scope":"{scope}","request_id":"{request_id}"}}"#,
                caller.lane
            ),
            is_error: false,
        }
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "studio-mcp-battery-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    dir
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    fn json(&self) -> Value {
        parse_json(&self.body).unwrap_or_else(|e| {
            panic!("body is not JSON ({e}): {:?}", String::from_utf8_lossy(&self.body))
        })
    }

    fn rpc_error_code(&self) -> i64 {
        self.json().get("error").and_then(|e| e.get("code")).and_then(Value::as_i64).unwrap_or_else(|| {
            panic!("missing error.code in {:?}", String::from_utf8_lossy(&self.body))
        })
    }

    fn rpc_id_i64(&self) -> Option<i64> {
        self.json().get("id").and_then(Value::as_i64)
    }
}

fn port_from_url(url: &str) -> u16 {
    let rest = url.strip_prefix("http://127.0.0.1:").expect("url() is loopback http");
    let port = rest.split('/').next().expect("url() has a path");
    port.parse().expect("url() port")
}

fn connect(port: u16, read_ms: u64) -> TcpStream {
    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream.set_nodelay(true).unwrap();
    stream.set_read_timeout(Some(Duration::from_millis(read_ms.max(1)))).unwrap();
    stream.set_write_timeout(Some(Duration::from_millis(read_ms.max(1)))).unwrap();
    stream
}

fn request_bytes(
    method: &str,
    path: &str,
    host: &str,
    extra: &[(&str, &str)],
    body: &[u8],
    close: bool,
) -> Vec<u8> {
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n");
    req.push_str(if close { "Connection: close\r\n" } else { "Connection: keep-alive\r\n" });
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() || method.eq_ignore_ascii_case("POST") {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("\r\n");
    let mut out = req.into_bytes();
    out.extend_from_slice(body);
    out
}

fn read_reply(stream: &mut TcpStream, leftover: &mut Vec<u8>) -> io::Result<Reply> {
    let mut data = std::mem::take(leftover);
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("eof with {} header bytes", data.len()),
            ));
        }
        data.extend_from_slice(&tmp[..n]);
        if data.len() > 128 * 1024 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "headers too large"));
        }
    };
    let header_text = String::from_utf8_lossy(&data[..header_end]).into_owned();
    let rest = data[header_end + 4..].to_vec();
    let mut lines = header_text.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status: u16 = status_line
        .split(' ')
        .nth(1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, status_line.to_string()))?
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e}: {status_line}")))?;
    let mut headers = Vec::new();
    let mut length = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (k, v) = line.split_once(':').ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, format!("malformed header {line:?}"))
        })?;
        if k.eq_ignore_ascii_case("content-length") {
            length = v.trim().parse().unwrap_or(0);
        }
        headers.push((k.to_ascii_lowercase(), v.trim().to_string()));
    }
    let mut body = rest;
    while body.len() < length {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    if body.len() < length {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short body"));
    }
    leftover.extend_from_slice(&body[length..]);
    body.truncate(length);
    Ok(Reply { status, headers, body })
}

fn http_host(
    port: u16,
    method: &str,
    path: &str,
    host: &str,
    extra: &[(&str, &str)],
    body: &[u8],
) -> Reply {
    let mut stream = connect(port, 4000);
    let bytes = request_bytes(method, path, host, extra, body, true);
    stream.write_all(&bytes).unwrap();
    let _ = stream.shutdown(Shutdown::Write);
    read_reply(&mut stream, &mut Vec::new()).unwrap()
}

fn http(port: u16, method: &str, path: &str, extra: &[(&str, &str)], body: &[u8]) -> Reply {
    http_host(port, method, path, &format!("127.0.0.1:{port}"), extra, body)
}

fn rpc(port: u16, token: &str, body: &str) -> Reply {
    let auth = format!("Bearer {token}");
    http(
        port,
        "POST",
        "/mcp",
        &[("Content-Type", "application/json"), ("Authorization", &auth)],
        body.as_bytes(),
    )
}

fn rpc_on(stream: &mut TcpStream, leftover: &mut Vec<u8>, port: u16, token: &str, body: &str, close: bool) -> Reply {
    let host = format!("127.0.0.1:{port}");
    let auth = format!("Bearer {token}");
    let bytes = request_bytes(
        "POST",
        "/mcp",
        &host,
        &[("Content-Type", "application/json"), ("Authorization", &auth)],
        body.as_bytes(),
        close,
    );
    stream.write_all(&bytes).unwrap();
    read_reply(stream, leftover).unwrap()
}

fn start(huge: bool) -> (McpServer, Arc<TokenStore>, Arc<Fake>, PathBuf) {
    let dir = temp_dir("srv");
    let tokens = Arc::new(TokenStore::open(&dir).unwrap());
    let fake = Arc::new(Fake { huge, calls: Mutex::new(Vec::new()) });
    let server = McpServer::start(tokens.clone(), fake.clone()).unwrap();
    (server, tokens, fake, dir)
}

fn ping_body() -> &'static str {
    r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#
}

#[test]
fn handshake_then_call_roundtrip_with_request_ids() {
    let (server, tokens, fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-a", "owner-1").unwrap();
    let t = token.as_str();

    let init = rpc(
        port,
        t,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
    );
    assert_eq!(init.status, 200);
    let v = init.json();
    assert_eq!(v.get("id").and_then(Value::as_i64), Some(1));
    assert_eq!(
        v.get("result").and_then(|r| r.get("protocolVersion")).and_then(Value::as_str),
        Some("2025-06-18")
    );

    let initialized = rpc(port, t, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    assert_eq!(initialized.status, 202);
    assert!(initialized.body.is_empty());

    let list = rpc(port, t, r#"{"jsonrpc":"2.0","id":"list-1","method":"tools/list"}"#);
    assert_eq!(list.status, 200);
    let v = list.json();
    assert_eq!(v.get("id").and_then(Value::as_str), Some("list-1"));
    let tools = v.get("result").and_then(|r| r.get("tools")).and_then(Value::as_arr).unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].get("name").and_then(Value::as_str), Some("code_outline"));

    let call = rpc(
        port,
        t,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"code_outline","arguments":{"scope":"widgets::dock"}}}"#,
    );
    assert_eq!(call.status, 200);
    let v = call.json();
    assert_eq!(v.get("id").and_then(Value::as_i64), Some(3));
    let text = v
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(Value::as_arr)
        .and_then(|c| c.first())
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .unwrap();
    let inner = parse_json(text.as_bytes()).unwrap();
    assert_eq!(inner.get("tool").and_then(Value::as_str), Some("code_outline"));
    assert_eq!(inner.get("request_id").and_then(Value::as_str), Some("mcp-lane-a-3"));
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.as_slice(), ["code_outline"]);

    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn wrong_bearer_401_with_www_authenticate() {
    let (server, _tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let reply = http(
        port,
        "POST",
        "/mcp",
        &[("Content-Type", "application/json"), ("Authorization", "Bearer 00")],
        b"{}",
    );
    assert_eq!(reply.status, 401);
    let www = reply.header("www-authenticate").unwrap_or("");
    assert!(www.contains("Bearer"), "WWW-Authenticate: {www:?}");
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn missing_bearer_401() {
    let (server, _tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let reply = http(port, "POST", "/mcp", &[("Content-Type", "application/json")], b"{}");
    assert_eq!(reply.status, 401);
    let www = reply.header("www-authenticate").unwrap_or("");
    assert!(www.contains("Bearer"), "WWW-Authenticate: {www:?}");
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn revoked_token_401_after_success() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-d", "owner-1").unwrap();
    assert_eq!(rpc(port, token.as_str(), ping_body()).status, 200);
    assert!(tokens.revoke("lane-d").unwrap());
    assert_eq!(rpc(port, token.as_str(), ping_body()).status, 401);
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn reminted_token_invalidates_old() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let first = tokens.mint("lane-d", "owner-1").unwrap();
    assert_eq!(rpc(port, first.as_str(), ping_body()).status, 200);
    let second = tokens.mint("lane-d", "owner-1").unwrap();
    assert_ne!(first.as_str(), second.as_str());
    assert_eq!(rpc(port, first.as_str(), ping_body()).status, 401);
    assert_eq!(rpc(port, second.as_str(), ping_body()).status, 200);
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn origin_foreign_403() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let auth = format!("Bearer {}", token.as_str());
    let reply = http(
        port,
        "POST",
        "/mcp",
        &[
            ("Content-Type", "application/json"),
            ("Authorization", &auth),
            ("Origin", "http://evil.example"),
        ],
        b"{}",
    );
    assert_eq!(reply.status, 403);
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn host_mismatch_403() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let auth = format!("Bearer {}", token.as_str());
    let extra = [("Content-Type", "application/json"), ("Authorization", auth.as_str())];

    let foreign = http_host(port, "POST", "/mcp", "evil.example", &extra, ping_body().as_bytes());
    assert_eq!(foreign.status, 403, "foreign Host must be refused");

    let wrong_port = http_host(
        port,
        "POST",
        "/mcp",
        &format!("127.0.0.1:{}", port.wrapping_add(1).max(1)),
        &extra,
        ping_body().as_bytes(),
    );
    assert_eq!(wrong_port.status, 403, "loopback Host with the wrong port must be refused");

    let ipv4 = http_host(port, "POST", "/mcp", "127.0.0.1", &extra, ping_body().as_bytes());
    assert_eq!(ipv4.status, 200, "Host 127.0.0.1 without a port is loopback");

    let localhost = http_host(port, "POST", "/mcp", "localhost", &extra, ping_body().as_bytes());
    assert_eq!(localhost.status, 200, "Host localhost without a port is loopback");

    let bracket = http_host(
        port,
        "POST",
        "/mcp",
        &format!("[::1]:{port}"),
        &extra,
        ping_body().as_bytes(),
    );
    assert_eq!(bracket.status, 200, "Host [::1]:<port> is listed as loopback");

    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn get_405_with_allow_post() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let auth = format!("Bearer {}", token.as_str());
    let reply = http(port, "GET", "/mcp", &[("Authorization", &auth)], b"");
    assert_eq!(reply.status, 405);
    assert_eq!(reply.header("allow"), Some("POST"));
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn delete_405() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let auth = format!("Bearer {}", token.as_str());
    let reply = http(port, "DELETE", "/mcp", &[("Authorization", &auth)], b"");
    assert_eq!(reply.status, 405);
    assert_eq!(reply.header("allow"), Some("POST"));
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn unknown_path_404() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let auth = format!("Bearer {}", token.as_str());
    let reply = http(
        port,
        "POST",
        "/other",
        &[("Content-Type", "application/json"), ("Authorization", &auth)],
        b"{}",
    );
    assert_eq!(reply.status, 404);
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn wrong_content_type_415() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let auth = format!("Bearer {}", token.as_str());
    let reply = http(
        port,
        "POST",
        "/mcp",
        &[("Content-Type", "text/plain"), ("Authorization", &auth)],
        b"{}",
    );
    assert_eq!(reply.status, 415);
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn oversized_body_413_exactly_at_boundary() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let auth = format!("Bearer {}", token.as_str());
    let extra = [("Content-Type", "application/json"), ("Authorization", auth.as_str())];

    let ping = ping_body().as_bytes();
    let mut at_cap = vec![b' '; MAX_BODY_BYTES as usize];
    at_cap[..ping.len()].copy_from_slice(ping);
    let ok = http(port, "POST", "/mcp", &extra, &at_cap);
    assert_ne!(ok.status, 413, "64 KiB body must be accepted");
    assert_eq!(ok.status, 200);
    assert_eq!(ok.rpc_id_i64(), Some(1));

    let mut over = vec![b'x'; MAX_BODY_BYTES as usize + 1];
    over[..ping.len()].copy_from_slice(ping);
    let too_large = http(port, "POST", "/mcp", &extra, &over);
    assert_eq!(too_large.status, 413);

    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn jsonrpc_parse_error_minus_32700() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let reply = rpc(port, token.as_str(), "{not json");
    assert_eq!(reply.status, 400);
    assert_eq!(reply.rpc_error_code(), PARSE_ERROR);
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn invalid_request_minus_32600() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let t = token.as_str();

    let missing = rpc(port, t, r#"{"id":1,"method":"ping"}"#);
    assert_eq!(missing.status, 400);
    assert_eq!(missing.rpc_error_code(), INVALID_REQUEST);

    let batch = rpc(port, t, r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#);
    assert_eq!(batch.status, 400);
    assert_eq!(batch.rpc_error_code(), INVALID_REQUEST);

    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn method_not_found_minus_32601() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let reply = rpc(port, token.as_str(), r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#);
    assert_eq!(reply.status, 200);
    assert_eq!(reply.rpc_error_code(), METHOD_NOT_FOUND);
    assert_eq!(reply.json().get("id").and_then(Value::as_i64), Some(4));
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn invalid_params_minus_32602() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let reply =
        rpc(port, token.as_str(), r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"arguments":{}}}"#);
    assert_eq!(reply.status, 200);
    assert_eq!(reply.rpc_error_code(), INVALID_PARAMS);
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn unknown_tool_error_result() {
    let (server, tokens, fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let reply = rpc(
        port,
        token.as_str(),
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"missing_tool","arguments":{}}}"#,
    );
    assert_eq!(reply.status, 200);
    let v = reply.json();
    assert!(v.get("error").is_none(), "unknown tool is a result, not a JSON-RPC error");
    assert_eq!(v.get("result").and_then(|r| r.get("isError")).and_then(Value::as_bool), Some(true));
    assert_eq!(fake.calls.lock().unwrap().as_slice(), ["missing_tool"]);
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn result_over_16kib_is_refused_not_truncated() {
    let (server, tokens, _fake, dir) = start(true);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-c", "owner-1").unwrap();
    let reply = rpc(
        port,
        token.as_str(),
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"code_outline","arguments":{}}}"#,
    );
    assert_eq!(reply.status, 200);
    assert!(
        parse_json(&reply.body).is_ok(),
        "oversized-result refusal must still be valid JSON, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
    assert!(
        reply.body.len() < 16 * 1024,
        "must refuse rather than emit the 20 KiB payload, body is {} bytes",
        reply.body.len()
    );
    assert!(!reply.body.windows(32).any(|w| w.iter().all(|&b| b == b'x')));
    let err = reply.json().get("error").cloned().expect("json-rpc error");
    assert_eq!(err.get("code").and_then(Value::as_i64), Some(INTERNAL_ERROR));
    let msg = err.get("message").and_then(Value::as_str).unwrap_or("");
    assert!(msg.contains("page"), "error should tell the caller to page: {msg:?}");
    assert!(msg.contains("cursor"), "error should mention the cursor: {msg:?}");
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn notification_returns_202_empty() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let t = token.as_str();

    let initialized = rpc(port, t, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    assert_eq!(initialized.status, 202);
    assert!(initialized.body.is_empty());

    let unknown = rpc(port, t, r#"{"jsonrpc":"2.0","method":"notifications/not-a-real-note"}"#);
    assert_eq!(unknown.status, 202);
    assert!(unknown.body.is_empty());

    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn concurrency_bounded_503_with_retry_after() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-g", "owner-1").unwrap();
    let t = token.as_str();

    // Workers take from a 16-slot channel. Connecting all 20 before any worker
    // has called recv overflows at 17 and 503s a connection we wanted to keep.
    // Park the four workers first, then fill the queue.
    std::thread::sleep(Duration::from_millis(50));
    let mut held = Vec::new();
    for _ in 0..WORKERS {
        held.push(connect(port, 4000));
    }
    std::thread::sleep(Duration::from_millis(200));
    for _ in 0..QUEUE {
        held.push(connect(port, 4000));
    }
    std::thread::sleep(Duration::from_millis(50));

    let deadline = Instant::now() + Duration::from_secs(4);
    let mut refused = None;
    let mut extras = Vec::new();
    while Instant::now() < deadline {
        let mut overflow = connect(port, 500);
        match read_reply(&mut overflow, &mut Vec::new()) {
            Ok(reply) if reply.status == 503 => {
                refused = Some(reply);
                break;
            }
            Ok(reply) => panic!("overflow connection got {} instead of 503", reply.status),
            Err(_) => {
                extras.push(overflow);
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    let refused = refused.expect("overflow connection got no 503 within 4 s");
    assert_eq!(refused.status, 503);
    assert_eq!(refused.header("retry-after"), Some("1"));

    let host = format!("127.0.0.1:{port}");
    let auth = format!("Bearer {t}");
    let ping = request_bytes(
        "POST",
        "/mcp",
        &host,
        &[("Content-Type", "application/json"), ("Authorization", &auth)],
        ping_body().as_bytes(),
        true,
    );
    for stream in held.iter_mut() {
        stream.write_all(&ping).unwrap();
    }
    for (i, stream) in held.iter_mut().enumerate() {
        let reply = read_reply(stream, &mut Vec::new())
            .unwrap_or_else(|e| panic!("held connection {i} did not answer: {e}"));
        assert_eq!(reply.status, 200, "held connection {i} status");
    }
    let _ = extras;

    drop(held);
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn keep_alive_serves_256_then_closes() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-k", "owner-1").unwrap();
    let mut stream = connect(port, 4000);
    let mut leftover = Vec::new();

    for i in 0..MAX_REQUESTS_PER_CONN {
        let reply = rpc_on(&mut stream, &mut leftover, port, token.as_str(), ping_body(), false);
        assert_eq!(reply.status, 200, "keep-alive request {} of {}", i + 1, MAX_REQUESTS_PER_CONN);
        if i + 1 < MAX_REQUESTS_PER_CONN {
            let conn = reply.header("connection").unwrap_or("");
            assert!(!conn.eq_ignore_ascii_case("close"), "request {} closed early ({conn})", i + 1);
        }
    }

    let host = format!("127.0.0.1:{port}");
    let auth = format!("Bearer {}", token.as_str());
    let extra = request_bytes(
        "POST",
        "/mcp",
        &host,
        &[("Content-Type", "application/json"), ("Authorization", &auth)],
        ping_body().as_bytes(),
        false,
    );
    let write_err = stream.write_all(&extra).err();
    match read_reply(&mut stream, &mut leftover) {
        Err(_) => {}
        Ok(reply) => panic!("257th request got HTTP {} instead of close", reply.status),
    }
    let _ = write_err;

    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn slowloris_partial_header_times_out() {
    let (server, _tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let budget = Duration::from_millis(HEAD_DEADLINE_MS + 1000);
    let mut stream = connect(port, HEAD_DEADLINE_MS + 1000);
    stream.write_all(b"POST /mc").unwrap();

    let start = Instant::now();
    let mut buf = [0u8; 256];
    let result = stream.read(&mut buf);
    let elapsed = start.elapsed();
    assert!(
        elapsed <= budget,
        "slowloris hang {elapsed:?} exceeded deadline {} ms + 1 s",
        HEAD_DEADLINE_MS
    );
    match result {
        Ok(0) => {}
        Ok(n) => {
            let text = String::from_utf8_lossy(&buf[..n]);
            assert!(
                text.contains("408") || text.contains("HTTP/1.1"),
                "partial-head close produced {text:?}"
            );
        }
        Err(e) => {
            assert!(
                matches!(
                    e.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::BrokenPipe
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                ),
                "unexpected slowloris error: {e:?}"
            );
        }
    }

    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn token_store_persistence_roundtrip_0600() {
    let dir = temp_dir("persist");
    let token = {
        let store = TokenStore::open(&dir).unwrap();
        store.mint("lane-e", "owner-9").unwrap()
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(dir.join("mcp").join("tokens.txt")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        let dmeta = std::fs::metadata(dir.join("mcp")).unwrap();
        assert_eq!(dmeta.permissions().mode() & 0o777, 0o700);
    }
    let store = TokenStore::open(&dir).unwrap();
    let caller = store.authorize(token.as_str()).unwrap();
    assert_eq!(caller, LaneCaller { lane: "lane-e".into(), owner: "owner-9".into() });
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn token_store_refuses_tampered_file() {
    let dir = temp_dir("tamper");
    {
        let store = TokenStore::open(&dir).unwrap();
        let _ = store.mint("lane-e", "owner-10").unwrap();
    }
    std::fs::write(dir.join("mcp").join("tokens.txt"), "lane-e owner-10 zz\n").unwrap();
    assert!(TokenStore::open(&dir).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn token_store_refuses_world_readable_file() {
    let dir = temp_dir("world");
    {
        let store = TokenStore::open(&dir).unwrap();
        let _ = store.mint("lane-e", "owner-10").unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("mcp").join("tokens.txt");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = match TokenStore::open(&dir) {
            Ok(_) => panic!("world-readable token file must be refused"),
            Err(e) => e,
        };
        assert!(
            err.contains("0600") || err.contains("owned"),
            "unexpected refusal: {err}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn malformed_utf8_body_400() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let auth = format!("Bearer {}", token.as_str());
    let body = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\xff";
    let reply = http(
        port,
        "POST",
        "/mcp",
        &[("Content-Type", "application/json"), ("Authorization", &auth)],
        body,
    );
    assert_eq!(reply.status, 400);
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn duplicate_authorization_header_400() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let mut stream = connect(port, 4000);
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\
         Authorization: Bearer {t}\r\nAuthorization: Bearer {t}\r\n\
         Content-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}",
        t = token.as_str()
    );
    stream.write_all(req.as_bytes()).unwrap();
    let reply = read_reply(&mut stream, &mut Vec::new()).unwrap();
    assert_eq!(reply.status, 400);
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn header_over_512_bytes_431() {
    let (server, tokens, _fake, dir) = start(false);
    let port = port_from_url(&server.url());
    let token = tokens.mint("lane-b", "owner-1").unwrap();
    let origin = "x".repeat(513);
    let auth = format!("Bearer {}", token.as_str());
    let reply = http(
        port,
        "POST",
        "/mcp",
        &[
            ("Content-Type", "application/json"),
            ("Authorization", &auth),
            ("Origin", &origin),
        ],
        ping_body().as_bytes(),
    );
    assert_eq!(reply.status, 431);
    drop(server);
    let _ = std::fs::remove_dir_all(dir);
}
