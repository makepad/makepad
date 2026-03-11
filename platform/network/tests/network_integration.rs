use makepad_live_id::LiveId;
use makepad_network::{
    HttpMethod, HttpRequest, HttpServer, HttpServerRequest, NetworkConfig, NetworkResponse,
    NetworkRuntime, SocketStream, WebSocketTransport, WsMessage, WsSend,
};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::sync::{mpsc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

fn find_free_port() -> Option<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    Some(listener.local_addr().ok()?.port())
}

fn test_guard() -> MutexGuard<'static, ()> {
    static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
    TEST_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn wait_for_event<F>(
    runtime: &NetworkRuntime,
    timeout: Duration,
    mut matcher: F,
) -> Option<NetworkResponse>
where
    F: FnMut(&NetworkResponse) -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(event) = runtime.recv_timeout(Duration::from_millis(50)) {
            if matcher(&event) {
                return Some(event);
            }
        }
    }
    None
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_content_length(headers: &str) -> usize {
    for line in headers.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("content-length:") {
            let value = line.split_once(':').map(|(_, v)| v.trim()).unwrap_or("0");
            return value.parse::<usize>().unwrap_or(0);
        }
    }
    0
}

#[cfg(not(target_arch = "wasm32"))]
fn websocket_roundtrip_via_http_server(transport: WebSocketTransport) {
    let runtime = NetworkRuntime::new(NetworkConfig::default());
    let Some(port) = find_free_port() else {
        eprintln!("websocket integration test skipped: cannot allocate local test port");
        return;
    };
    let listen_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let (request_sender, request_receiver) = mpsc::channel::<HttpServerRequest>();
    let Some(_http_thread) = runtime.start_http_server(HttpServer {
        listen_address,
        request: request_sender,
        post_max_size: 1024 * 1024,
    }) else {
        eprintln!("websocket integration test skipped: failed to start http server");
        return;
    };

    let server_thread = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            let Ok(request) = request_receiver.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            match request {
                HttpServerRequest::BinaryMessage {
                    response_sender,
                    data,
                    ..
                } => {
                    let _ = response_sender.send(data);
                    break;
                }
                HttpServerRequest::DisconnectWebSocket { .. } => break,
                _ => {}
            }
        }
    });

    let socket_id = match transport {
        WebSocketTransport::PlainTcp => LiveId::from_str("plain.ws.test"),
        WebSocketTransport::Platform => LiveId::from_str("platform.ws.test"),
        WebSocketTransport::Auto => LiveId::from_str("auto.ws.test"),
    };
    let mut request = HttpRequest::new(format!("ws://127.0.0.1:{port}/transport"), HttpMethod::GET);
    request.set_websocket_transport(transport);
    runtime
        .ws_open(socket_id, request)
        .expect("ws_open should succeed");

    let opened = wait_for_event(
        &runtime,
        Duration::from_secs(4),
        |event| matches!(event, NetworkResponse::WsOpened { socket_id: id } if *id == socket_id),
    );
    assert!(opened.is_some(), "did not receive WsOpened");

    let payload = vec![1u8, 2, 3, 4, 5];
    runtime
        .ws_send(socket_id, WsSend::Binary(payload.clone()))
        .expect("ws_send should succeed");

    let echoed = wait_for_event(&runtime, Duration::from_secs(4), |event| {
        matches!(
            event,
            NetworkResponse::WsMessage {
                socket_id: id,
                message: WsMessage::Binary(data)
            } if *id == socket_id && data == &payload
        )
    });
    assert!(echoed.is_some(), "did not receive echoed websocket payload");

    let _ = runtime.ws_close(socket_id);
    let _ = server_thread.join();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn plain_websocket_roundtrip_via_http_server() {
    let _guard = test_guard();
    websocket_roundtrip_via_http_server(WebSocketTransport::PlainTcp);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn platform_websocket_roundtrip_via_http_server() {
    let _guard = test_guard();
    websocket_roundtrip_via_http_server(WebSocketTransport::Platform);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn https_google_request_exercises_https_path() {
    let _guard = test_guard();
    let runtime = NetworkRuntime::new(NetworkConfig::default());
    let request_id = LiveId::from_str("https.makepad.test");
    let mut request = HttpRequest::new("https://makepad.nl/".to_string(), HttpMethod::GET);
    request.set_header("User-Agent".to_string(), "makepad-network-test".to_string());
    runtime
        .http_start(request_id, request)
        .expect("http_start should succeed");

    let event = wait_for_event(&runtime, Duration::from_secs(30), |event| {
        matches!(
            event,
            NetworkResponse::HttpResponse {
                request_id: id,
                ..
            } if *id == request_id
        ) || matches!(
            event,
            NetworkResponse::HttpError {
                request_id: id,
                ..
            } if *id == request_id
        )
    })
    .expect("no http result event received");

    match event {
        NetworkResponse::HttpResponse { response, .. } => {
            assert!(
                response.status_code >= 100 && response.status_code < 600,
                "unexpected status code: {}",
                response.status_code
            );
        }
        NetworkResponse::HttpError { error, .. } => {
            let msg = error.message.to_ascii_lowercase();
            assert!(
                !msg.contains("unsupported"),
                "https path reported unsupported transport: {}",
                error.message
            );
        }
        other => panic!("unexpected network event: {other:?}"),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn http_post_body_roundtrip_preserves_json_payload() {
    let _guard = test_guard();
    let runtime = NetworkRuntime::new(NetworkConfig::default());

    let Some(port) = find_free_port() else {
        eprintln!("http post body test skipped: cannot allocate local test port");
        return;
    };

    let (capture_tx, capture_rx) = mpsc::channel::<(String, Vec<u8>)>();
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind local tcp listener");
    let server = std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };

        let mut req = Vec::new();
        let mut tmp = [0u8; 4096];
        let mut target_len = None::<usize>;
        let mut header_end = None::<usize>;

        loop {
            let Ok(n) = stream.read(&mut tmp) else {
                return;
            };
            if n == 0 {
                break;
            }
            req.extend_from_slice(&tmp[..n]);

            if header_end.is_none() {
                header_end = find_header_end(&req);
                if let Some(end) = header_end {
                    let headers = String::from_utf8_lossy(&req[..end]).to_string();
                    target_len = Some(end + 4 + parse_content_length(&headers));
                }
            }
            if let Some(target) = target_len {
                if req.len() >= target {
                    break;
                }
            }
        }

        let Some(end) = find_header_end(&req) else {
            return;
        };
        let headers = String::from_utf8_lossy(&req[..end]).to_string();
        let body_len = parse_content_length(&headers);
        let body_start = end + 4;
        let body_end = body_start.saturating_add(body_len).min(req.len());
        let body = req[body_start..body_end].to_vec();
        let _ = capture_tx.send((headers, body));

        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK");
        let _ = stream.flush();
    });

    let request_id = LiveId::from_str("http.post.body.test");
    let mut request = HttpRequest::new(
        format!("http://127.0.0.1:{port}/v1/chat/completions"),
        HttpMethod::POST,
    );
    request.set_header("Content-Type".to_string(), "application/json".to_string());
    let body = r#"{"messages":[{"role":"user","content":"hello"}],"stream":false}"#;
    request.set_body_string(body);
    runtime
        .http_start(request_id, request)
        .expect("http_start should succeed");

    let event = wait_for_event(&runtime, Duration::from_secs(10), |event| {
        matches!(event, NetworkResponse::HttpResponse { request_id: id, .. } if *id == request_id)
            || matches!(event, NetworkResponse::HttpError { request_id: id, .. } if *id == request_id)
    })
    .expect("no http result event received");
    match event {
        NetworkResponse::HttpResponse { response, .. } => {
            assert_eq!(response.status_code, 200, "unexpected response status");
        }
        NetworkResponse::HttpError { error, .. } => {
            panic!("unexpected http error: {}", error.message);
        }
        other => panic!("unexpected network event: {other:?}"),
    }

    let (headers, captured_body) = capture_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("did not capture local request");
    let captured = String::from_utf8(captured_body).expect("request body must be utf8");
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("content-type: application/json"),
        "content-type header missing or wrong: {headers}"
    );
    assert_eq!(captured, body, "request body changed in transport layer");

    let _ = server.join();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn socket_stream_plain_tcp_large_roundtrip() {
    let _guard = test_guard();
    let Some(port) = find_free_port() else {
        eprintln!("socket stream test skipped: cannot allocate local test port");
        return;
    };

    let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind local tcp listener");
    let server = std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut total = 0usize;
        let mut buf = vec![0u8; 8192];
        loop {
            let Ok(n) = stream.read(&mut buf) else {
                return;
            };
            if n == 0 {
                break;
            }
            total += n;
            let _ = stream.write_all(&buf[..n]);
            if total >= 256 * 1024 {
                break;
            }
        }
        let _ = stream.flush();
    });

    let mut socket = SocketStream::connect("127.0.0.1", &port.to_string(), false, false)
        .expect("socket stream connect should succeed");
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("set_read_timeout");
    socket
        .set_write_timeout(Some(Duration::from_secs(3)))
        .expect("set_write_timeout");

    let mut payload = Vec::with_capacity(256 * 1024);
    for i in 0..(256 * 1024) {
        payload.push((i % 251) as u8);
    }

    socket.write_all(&payload).expect("write_all payload");
    socket.flush().expect("flush payload");

    let mut echoed = vec![0u8; payload.len()];
    let mut read = 0usize;
    while read < echoed.len() {
        let n = socket.read(&mut echoed[read..]).expect("read echoed bytes");
        if n == 0 {
            break;
        }
        read += n;
    }
    echoed.truncate(read);
    assert_eq!(echoed.len(), payload.len(), "echoed payload size mismatch");
    assert_eq!(echoed, payload, "echoed payload contents mismatch");

    socket.shutdown();
    let _ = server.join();
}
