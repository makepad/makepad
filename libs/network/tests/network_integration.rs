use makepad_live_id::LiveId;
use makepad_network::{
    HttpMethod, HttpRequest, HttpServer, HttpServerRequest, NetworkConfig, NetworkResponse,
    NetworkRuntime, WebSocketTransport, WsMessage, WsSend,
};
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
    })
    else {
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

    let opened = wait_for_event(&runtime, Duration::from_secs(4), |event| {
        matches!(event, NetworkResponse::WsOpened { socket_id: id } if *id == socket_id)
    });
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
