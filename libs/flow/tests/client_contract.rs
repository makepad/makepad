#![cfg(not(target_arch = "wasm32"))]

use makepad_flow::client::{
    ClientError, Endpoints, FlowClient, FlowSubscriber, FlowSubscriberConfig, SessionConfig,
    SessionConnector, SessionStatus, SubscriptionEvent,
};
use makepad_flow::embed::{resolve, EmbedPolicy, Resolved};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SERVER_ID: [u8; 16] = [0x11; 16];

#[derive(Clone, Debug)]
struct Request {
    method: String,
    target: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    fn json(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }
}

struct FixtureServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl FixtureServer {
    fn start(
        handler: impl Fn(&Request) -> Reply + Send + Sync + 'static,
    ) -> FixtureServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        Self::from_listener(listener, handler)
    }

    fn start_at(
        addr: SocketAddr,
        handler: impl Fn(&Request) -> Reply + Send + Sync + 'static,
    ) -> FixtureServer {
        let listener = TcpListener::bind(addr).unwrap();
        Self::from_listener(listener, handler)
    }

    fn from_listener(
        listener: TcpListener,
        handler: impl Fn(&Request) -> Reply + Send + Sync + 'static,
    ) -> FixtureServer {
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let worker_requests = requests.clone();
        let handler = Arc::new(handler);
        let join = std::thread::spawn(move || {
            let mut connections = Vec::new();
            while !worker_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        let handler = handler.clone();
                        let requests = worker_requests.clone();
                        connections.push(std::thread::spawn(move || {
                            serve_connection(stream, handler, requests)
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            for connection in connections {
                let _ = connection.join();
            }
        });
        FixtureServer {
            addr,
            stop,
            join: Some(join),
            requests,
        }
    }

    fn endpoints(&self) -> Endpoints {
        Endpoints {
            control: self.addr,
            data: self.addr,
        }
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.addr);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn serve_connection(
    stream: TcpStream,
    handler: Arc<dyn Fn(&Request) -> Reply + Send + Sync>,
    requests: Arc<Mutex<Vec<Request>>>,
) {
    let mut reader = BufReader::new(stream);
    loop {
        let mut request_line = String::new();
        match reader.read_line(&mut request_line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let mut parts = request_line.trim_end().split_whitespace();
        let Some(method) = parts.next() else { return };
        let Some(target) = parts.next() else { return };
        if parts.next() != Some("HTTP/1.1") {
            return;
        }
        let mut content_length = 0usize;
        let mut authorization = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).ok().filter(|count| *count > 0).is_none() {
                return;
            }
            if line == "\r\n" {
                break;
            }
            let Some((name, value)) = line.trim_end().split_once(':') else {
                return;
            };
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap();
            }
            if name.eq_ignore_ascii_case("authorization") {
                authorization = Some(value.trim().to_string());
            }
        }
        let mut body = vec![0; content_length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let request = Request {
            method: method.to_string(),
            target: target.to_string(),
            authorization,
            body,
        };
        requests.lock().unwrap().push(request.clone());
        let reply = handler(&request);
        let reason = match reply.status {
            200 => "OK",
            401 => "Unauthorized",
            422 => "Unprocessable Content",
            _ => "Error",
        };
        let response = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{}",
            reply.status,
            reason,
            reply.body.len(),
            reply.body
        );
        if reader.get_mut().write_all(response.as_bytes()).is_err()
            || reader.get_mut().flush().is_err()
        {
            return;
        }
    }
}

fn health(protocol_version: u16, server_id: [u8; 16]) -> String {
    format!(
        "{{\"service\":\"makepad-flow\",\"server_id\":\"{}\",\"protocol_version\":{protocol_version},\"revision_epoch\":7}}",
        hex(&server_id)
    )
}

fn normal_handler(request: &Request) -> Reply {
    match request.target.as_str() {
        "/v1/health" => Reply::json(200, health(1, SERVER_ID)),
        "/v1/flows" if request.authorization.as_deref() == Some("Bearer token") => {
            Reply::json(200, "[]")
        }
        _ => Reply::json(401, "{\"error\":\"unauthorized\"}"),
    }
}

#[test]
fn connect_fail_closes_identity_protocol_and_auth() {
    let _serial = SerialTest::acquire();
    let server = FixtureServer::start(normal_handler);
    assert_eq!(
        FlowClient::connect(server.endpoints(), "token".into(), Some([0x22; 16])).unwrap_err(),
        ClientError::ServerIdentityMismatch
    );

    let server = FixtureServer::start(|request| match request.target.as_str() {
        "/v1/health" => Reply::json(200, health(2, SERVER_ID)),
        _ => Reply::json(200, "[]"),
    });
    assert!(matches!(
        FlowClient::connect(server.endpoints(), "token".into(), None),
        Err(ClientError::Protocol(_))
    ));

    let server = FixtureServer::start(|request| match request.target.as_str() {
        "/v1/health" => Reply::json(200, health(1, SERVER_ID)),
        _ => Reply::json(401, "{\"error\":\"no\"}"),
    });
    assert_eq!(
        FlowClient::connect(server.endpoints(), "wrong".into(), None).unwrap_err(),
        ClientError::Unauthorized
    );
}

#[test]
fn source_put_maps_422_to_eval_error() {
    let _serial = SerialTest::acquire();
    let server = FixtureServer::start(|request| match request.target.as_str() {
        "/v1/health" => Reply::json(200, health(1, SERVER_ID)),
        "/v1/flows" if request.method == "GET" => Reply::json(200, "[]"),
        "/v1/flows/broken" if request.method == "PUT" => Reply::json(
            422,
            "{\"error\":{\"line\":12,\"col\":7,\"message\":\"expected expression\"}}",
        ),
        _ => Reply::json(500, "{}"),
    });
    let client = FlowClient::connect(server.endpoints(), "token".into(), None).unwrap();
    match client.put_source("broken", "Flow{") {
        Err(ClientError::Eval(error)) => {
            assert_eq!((error.line, error.col), (12, 7));
            assert_eq!(error.message, "expected expression");
        }
        other => panic!("expected Eval, got {other:?}"),
    }
    let requests = server.requests.lock().unwrap();
    let put = requests.iter().find(|request| request.method == "PUT").unwrap();
    assert!(std::str::from_utf8(&put.body).unwrap().contains("Flow{"));
}

#[test]
fn subscriber_preserves_cursor_and_reports_gap() {
    let _serial = SerialTest::acquire();
    let polls = Arc::new(AtomicUsize::new(0));
    let worker_polls = polls.clone();
    let server = FixtureServer::start(move |request| match request.target.as_str() {
        "/v1/health" => Reply::json(200, health(1, SERVER_ID)),
        "/v1/flows" => Reply::json(200, "[]"),
        target if target.starts_with("/v1/events?") => {
            let poll = worker_polls.fetch_add(1, Ordering::SeqCst);
            match poll {
                0 => {
                    assert!(!target.contains("cursor="));
                    Reply::json(200, event_page(1, false))
                }
                1 => {
                    assert!(target.contains("cursor=0123456789abcdef-1"));
                    Reply::json(200, event_page(2, false))
                }
                _ => {
                    assert!(target.contains("cursor=0123456789abcdef-2"));
                    Reply::json(200, "{\"events\":[],\"cursor\":\"0123456789abcdef-3\",\"gap\":true}")
                }
            }
        }
        _ => Reply::json(500, "{}"),
    });
    let client = FlowClient::connect(server.endpoints(), "token".into(), None).unwrap();
    let subscriber = FlowSubscriber::start(
        Arc::new(Mutex::new(client)),
        FlowSubscriberConfig {
            wait_ms: 20,
            limit: 2,
            topic: Some("flows".into()),
        },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut received = Vec::new();
    while Instant::now() < deadline
        && !received
            .iter()
            .any(|event| matches!(event, SubscriptionEvent::ResyncRequired))
    {
        received.extend(subscriber.poll());
        std::thread::sleep(Duration::from_millis(10));
    }
    subscriber.request_stop();
    assert!(matches!(received.first(), Some(SubscriptionEvent::Ready)));
    assert_eq!(
        received
            .iter()
            .filter(|event| matches!(event, SubscriptionEvent::Events(_)))
            .count(),
        2
    );
    assert!(received
        .iter()
        .any(|event| matches!(event, SubscriptionEvent::ResyncRequired)));
}

#[test]
fn session_retries_then_connects_when_hint_appears() {
    let _serial = SerialTest::acquire();
    let reserved = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = reserved.local_addr().unwrap();
    drop(reserved);
    let endpoints = Endpoints {
        control: addr,
        data: addr,
    };
    let mut session = SessionConnector::start(SessionConfig {
        hint: Some(endpoints),
        token: Some("token".into()),
        retry_min_ms: 30,
        retry_max_ms: 80,
        ..SessionConfig::default()
    });
    wait_until(Duration::from_secs(2), || {
        matches!(session.status(), SessionStatus::Retrying { .. })
    });
    let server = FixtureServer::start_at(addr, normal_handler);
    wait_until(Duration::from_secs(3), || {
        matches!(session.status(), SessionStatus::Connected { .. })
    });
    assert!(session.client().is_some());
    session.stop();
    drop(session);
    drop(server);
}

#[test]
fn embed_policy_parses_and_reachable_root_attaches() {
    let _serial = SerialTest::acquire();
    let original = std::env::var_os("FLOW_UI_FLOW_EMBED");
    for value in ["never", "no", "off", "0", "false", "attach", "client"] {
        std::env::set_var("FLOW_UI_FLOW_EMBED", value);
        assert_eq!(EmbedPolicy::from_env(), EmbedPolicy::Never);
    }
    for value in ["always", "host"] {
        std::env::set_var("FLOW_UI_FLOW_EMBED", value);
        assert_eq!(EmbedPolicy::from_env(), EmbedPolicy::Always);
    }
    std::env::set_var("FLOW_UI_FLOW_EMBED", "something-else");
    assert_eq!(EmbedPolicy::from_env(), EmbedPolicy::Auto);
    match original {
        Some(value) => std::env::set_var("FLOW_UI_FLOW_EMBED", value),
        None => std::env::remove_var("FLOW_UI_FLOW_EMBED"),
    }

    let empty = TempDir::new("embed-empty");
    assert_eq!(resolve(EmbedPolicy::Auto, &empty.path, None), Resolved::Host);

    let server = FixtureServer::start(normal_handler);
    let root = TempDir::new("embed-live");
    std::fs::write(
        root.path.join("listen"),
        format!("127.0.0.1:{}:{}\n", server.addr.port(), server.addr.port()),
    )
    .unwrap();
    std::fs::write(root.path.join("token"), "token\n").unwrap();
    std::fs::write(root.path.join("server-id"), format!("{}\n", hex(&SERVER_ID))).unwrap();
    match resolve(EmbedPolicy::Auto, &root.path, None) {
        Resolved::Attach(Some(endpoints), Some(token), Some(server_id)) => {
            assert_eq!(endpoints, server.endpoints());
            assert_eq!(token, "token");
            assert_eq!(server_id, SERVER_ID);
        }
        other => panic!("expected attach, got {other:?}"),
    }
}

fn event_page(cursor: u64, gap: bool) -> String {
    format!(
        "{{\"events\":[{{\"seq\":{cursor},\"topic\":\"flows\",\"kind\":\"flow.changed\",\"name\":\"demo\",\"revision\":{cursor},\"canonical\":true}}],\"cursor\":\"0123456789abcdef-{cursor}\",\"gap\":{gap}}}"
    )
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("condition did not become true within {timeout:?}");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "makepad-flow-client-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct SerialTest(std::fs::File);

impl SerialTest {
    fn acquire() -> Self {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(std::env::temp_dir().join("makepad-flow-client-contract-tests.lock"))
            .unwrap();
        file.lock().unwrap();
        Self(file)
    }
}

impl Drop for SerialTest {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
