use makepad_live_id::LiveId;
use makepad_network::{
    HttpMethod, HttpRequest, HttpServer, HttpServerRequest, HttpServerResponse, NetworkConfig,
    NetworkResponse, NetworkRuntime, SocketStream, WebSocketTransport, WsMessage, WsSend,
    HTTP_BODY_LIMIT_ERROR,
};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
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
fn one_shot_http_response(
    response: Vec<u8>,
    expect_client_cancel: bool,
) -> Option<(
    SocketAddr,
    mpsc::Receiver<(String, Vec<u8>, bool)>,
    std::thread::JoinHandle<()>,
)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    let address = listener.local_addr().expect("read HTTP fixture address");
    let (capture_sender, capture_receiver) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept HTTP fixture request");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("set HTTP fixture read timeout");
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .expect("set HTTP fixture write timeout");

        let mut request = Vec::new();
        let mut scratch = [0u8; 4096];
        let head_end = loop {
            let read = stream.read(&mut scratch).expect("read HTTP fixture request");
            assert_ne!(read, 0, "client closed before sending request headers");
            request.extend_from_slice(&scratch[..read]);
            if let Some(end) = find_header_end(&request) {
                break end + 4;
            }
        };
        let head = String::from_utf8_lossy(&request[..head_end]).to_string();
        let body_len = parse_content_length(&head);
        while request.len() - head_end < body_len {
            let read = stream.read(&mut scratch).expect("read HTTP fixture body");
            assert_ne!(read, 0, "client closed before sending request body");
            request.extend_from_slice(&scratch[..read]);
        }
        let body = request[head_end..head_end + body_len].to_vec();

        let client_cancelled = if expect_client_cancel {
            if stream.write_all(&response).is_err() || stream.flush().is_err() {
                true
            } else {
                let chunk = [b'x'; 4096];
                (0..1024).any(|_| {
                    if stream.write_all(&chunk).is_err() || stream.flush().is_err() {
                        true
                    } else {
                        std::thread::sleep(Duration::from_millis(1));
                        false
                    }
                })
            }
        } else {
            stream.write_all(&response).expect("write HTTP fixture response");
            stream.flush().expect("flush HTTP fixture response");
            false
        };
        capture_sender
            .send((head, body, client_cancelled))
            .expect("send HTTP fixture capture");
    });
    Some((address, capture_receiver, server))
}

#[cfg(not(target_arch = "wasm32"))]
fn run_http_request(
    runtime: &NetworkRuntime,
    request_id: LiveId,
    address: SocketAddr,
    method: HttpMethod,
    body: &[u8],
    max_body: u64,
) -> NetworkResponse {
    let mut request = HttpRequest::new(format!("http://{address}/body-cap"), method);
    request.set_max_response_body_bytes(max_body);
    if !body.is_empty() || matches!(method, HttpMethod::POST | HttpMethod::PUT) {
        request.set_body(body.to_vec());
    }
    runtime
        .http_start(request_id, request)
        .expect("start native HTTP request");
    wait_for_event(runtime, Duration::from_secs(5), |event| {
        matches!(event, NetworkResponse::HttpResponse { request_id: id, .. } if *id == request_id)
            || matches!(event, NetworkResponse::HttpError { request_id: id, .. } if *id == request_id)
    })
    .expect("native HTTP request did not complete")
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_http_enforces_body_cap_and_preserves_methods() {
    let _guard = test_guard();
    let runtime = NetworkRuntime::new(NetworkConfig::default());

    // With no declared length, the server keeps sending until the backend observes
    // the first byte over the cap and closes the request. The peer-side disconnect
    // proves the backend did not buffer the rest of the response.
    let oversized = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
    let Some((address, capture, server)) = one_shot_http_response(oversized, true) else {
        eprintln!("native HTTP body-cap test skipped: cannot bind local fixture");
        return;
    };
    let event = run_http_request(
        &runtime,
        LiveId::from_str("native.http.cap.exceeded"),
        address,
        HttpMethod::GET,
        &[],
        8,
    );
    match event {
        NetworkResponse::HttpError { error, .. } => {
            assert_eq!(error.message, HTTP_BODY_LIMIT_ERROR);
        }
        other => panic!("oversized response did not return the body-cap error: {other:?}"),
    }
    let (_, _, client_cancelled) = capture
        .recv_timeout(Duration::from_secs(4))
        .expect("capture oversized request");
    assert!(client_cancelled, "backend did not cancel the oversized request");
    server.join().expect("join oversized HTTP fixture");

    let normal =
        b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\n12345678".to_vec();
    let (address, capture, server) =
        one_shot_http_response(normal, false).expect("bind within-cap HTTP fixture");
    let event = run_http_request(
        &runtime,
        LiveId::from_str("native.http.cap.accepted"),
        address,
        HttpMethod::GET,
        &[],
        8,
    );
    match event {
        NetworkResponse::HttpResponse { response, .. } => {
            assert_eq!(response.body(), Some(&b"12345678"[..]));
        }
        other => panic!("within-cap response failed: {other:?}"),
    }
    capture
        .recv_timeout(Duration::from_secs(4))
        .expect("capture within-cap request");
    server.join().expect("join within-cap HTTP fixture");

    for (name, method, request_body) in [
        ("head", HttpMethod::HEAD, &b""[..]),
        ("post", HttpMethod::POST, &b"post-body"[..]),
        ("put", HttpMethod::PUT, &b"put-body"[..]),
    ] {
        let response = if matches!(method, HttpMethod::HEAD) {
            b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\nConnection: close\r\n\r\n".to_vec()
        } else {
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_vec()
        };
        let (address, capture, server) =
            one_shot_http_response(response, false).expect("bind method HTTP fixture");
        let event = run_http_request(
            &runtime,
            LiveId::from_str(&format!("native.http.method.{name}")),
            address,
            method,
            request_body,
            8,
        );
        match event {
            NetworkResponse::HttpResponse { response, .. } => {
                if matches!(method, HttpMethod::HEAD) {
                    assert!(response.body().is_none_or(|body| body.is_empty()));
                } else {
                    assert_eq!(response.body(), Some(&b"ok"[..]));
                }
            }
            other => panic!("{method:?} request failed: {other:?}"),
        }
        let (head, body, _) = capture
            .recv_timeout(Duration::from_secs(4))
            .expect("capture method request");
        assert!(head.starts_with(method.as_str()));
        assert_eq!(body, request_body);
        server.join().expect("join method HTTP fixture");
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn http_server_routes_head_through_get_and_suppresses_body() {
    let _guard = test_guard();
    let port = find_free_port().expect("allocate local test port");
    let listen_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let runtime = NetworkRuntime::new(NetworkConfig::default());
    let (request_sender, request_receiver) = mpsc::channel::<HttpServerRequest>();
    runtime
        .start_http_server(HttpServer {
            listen_address,
            request: request_sender,
            post_max_size: 1024,
            post_max_size_overrides: Vec::new(),
        })
        .expect("start HTTP server");
    let handler = std::thread::spawn(move || {
        let request = request_receiver.recv_timeout(Duration::from_secs(3)).unwrap();
        let HttpServerRequest::Get { headers, response_sender } = request else {
            panic!("HEAD did not use GET-shaped dispatch");
        };
        assert_eq!(headers.verb, "HEAD");
        assert!(response_sender
            .send(HttpServerResponse {
                header: "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\n".into(),
                body: b"hidden!".to_vec(),
            })
            .is_ok());
    });
    let mut stream = TcpStream::connect(listen_address).unwrap();
    stream
        .write_all(b"HEAD /asset HTTP/1.1\r\nHost: test\r\n\r\n")
        .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let split = find_header_end(&response).unwrap() + 4;
    assert!(String::from_utf8_lossy(&response[..split]).contains("Content-Length: 7"));
    assert!(response[split..].is_empty());
    handler.join().unwrap();
}

#[cfg(not(target_arch = "wasm32"))]
fn raw_server_request(address: SocketAddr, request: &[u8]) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    stream.write_all(request).unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    String::from_utf8(response).unwrap()
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn http_server_rejects_ambiguous_framing_targets_and_get_shaped_mutations() {
    let _guard = test_guard();
    let port = find_free_port().expect("allocate local test port");
    let listen_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let runtime = NetworkRuntime::new(NetworkConfig::default());
    let (request_sender, request_receiver) = mpsc::channel::<HttpServerRequest>();
    runtime
        .start_http_server(HttpServer {
            listen_address,
            request: request_sender,
            post_max_size: 16,
            post_max_size_overrides: vec![("/small".into(), 2)],
        })
        .unwrap();

    for malformed in [
        &b"GET /x HTTP/1.1?q\r\nHost: x\r\n\r\n"[..],
        &b"GET  HTTP/1.1\r\nHost: x\r\n\r\n"[..],
        &b"GET https://example.test/x HTTP/1.1\r\nHost: x\r\n\r\n"[..],
        &b"POST /x HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx"[..],
        &b"POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"[..],
    ] {
        let response = raw_server_request(listen_address, malformed);
        assert!(response.starts_with("HTTP/1.1 400"), "{response:?}");
        assert!(response.contains("Cross-Origin-Opener-Policy: same-origin"));
        assert!(response.contains("Cross-Origin-Embedder-Policy: require-corp"));
    }
    for verb in ["PUT", "DELETE"] {
        let response = raw_server_request(
            listen_address,
            format!("{verb} /x HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes(),
        );
        assert!(response.starts_with("HTTP/1.1 405"), "{response:?}");
    }
    assert!(raw_server_request(
        listen_address,
        b"POST /x HTTP/1.1\r\nHost: x\r\n\r\n"
    )
    .starts_with("HTTP/1.1 411"));
    assert!(raw_server_request(
        listen_address,
        b"POST /small HTTP/1.1\r\nHost: x\r\nContent-Length: 3\r\n\r\nabc"
    )
    .starts_with("HTTP/1.1 413"));
    assert!(request_receiver.try_recv().is_err(), "rejected verbs reached application dispatch");
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
        post_max_size_overrides: Vec::new(),
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
fn http_get_range_header_is_sent_and_206_body_is_the_slice() {
    let _guard = test_guard();
    let runtime = NetworkRuntime::new(NetworkConfig::default());
    let Some(port) = find_free_port() else {
        eprintln!("http range test skipped: cannot allocate local test port");
        return;
    };

    let (capture_tx, capture_rx) = mpsc::channel::<String>();
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind local tcp listener");
    let server = std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut req = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            let Ok(n) = stream.read(&mut tmp) else {
                return;
            };
            if n == 0 {
                break;
            }
            req.extend_from_slice(&tmp[..n]);
            if find_header_end(&req).is_some() {
                break;
            }
        }
        let headers = String::from_utf8_lossy(&req).to_string();
        let _ = capture_tx.send(headers);
        let _ = stream.write_all(
            b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-7/16\r\nContent-Length: 4\r\nConnection: close\r\n\r\nWXYZ",
        );
        let _ = stream.flush();
    });

    let request_id = LiveId::from_str("http.get.range.test");
    let mut request = HttpRequest::new(format!("http://127.0.0.1:{port}/blob"), HttpMethod::GET);
    request.set_header("Range".to_string(), "bytes=4-7".to_string());
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
            assert_eq!(
                response.status_code, 206,
                "range GET must surface 206, got {}",
                response.status_code
            );
            assert_eq!(response.body(), Some(&b"WXYZ"[..]));
        }
        NetworkResponse::HttpError { error, .. } => {
            panic!("unexpected http error: {}", error.message);
        }
        other => panic!("unexpected network event: {other:?}"),
    }

    let headers = capture_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("did not capture local request");
    assert!(
        headers.to_ascii_lowercase().contains("range: bytes=4-7"),
        "Range header missing or rewritten: {headers}"
    );
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
