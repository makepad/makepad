//! The platform WebSocket (NSURLSessionWebSocketTask on Apple) must stay
//! safe to send to and close after the connection has failed or closed.
//! The app's run loop drains an autorelease pool after every event, so a
//! task the session has finished with is freed unless the socket holds
//! its own reference; these tests open the socket inside a pool that is
//! drained before the send, as the run loop would.

#![cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]

use makepad_live_id::LiveId;
use makepad_network::{
    HttpMethod, HttpRequest, NetworkConfig, NetworkResponse, NetworkRuntime, WebSocketTransport,
    WsSend,
};
use std::ffi::c_void;
use std::net::TcpListener;
use std::time::{Duration, Instant};

#[link(name = "objc")]
extern "C" {
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

fn wait_for<F: FnMut(&NetworkResponse) -> bool>(net: &NetworkRuntime, timeout: Duration, mut f: F) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(event) = net.recv_timeout(Duration::from_millis(50)) {
            if f(&event) {
                return true;
            }
        }
    }
    false
}

/// A port nothing listens on: the connection is refused at once.
fn closed_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.local_addr().unwrap().port()
}

#[test]
fn sending_and_closing_after_a_refused_connection_do_not_crash() {
    let net = NetworkRuntime::new(NetworkConfig::default());
    let id = LiveId::from_str("apple.ws.refused");
    let mut request = HttpRequest::new(format!("ws://127.0.0.1:{}/x", closed_port()), HttpMethod::GET);
    request.set_websocket_transport(WebSocketTransport::Platform);
    unsafe {
        let pool = objc_autoreleasePoolPush();
        net.ws_open(id, request).expect("ws_open");
        objc_autoreleasePoolPop(pool);
    }
    assert!(
        wait_for(&net, Duration::from_secs(10), |e| matches!(e, NetworkResponse::WsError { socket_id, .. } | NetworkResponse::WsClosed { socket_id } if *socket_id == id)),
        "the refused connection is reported"
    );
    // Give NSURLSession time to let go of the finished task.
    std::thread::sleep(Duration::from_millis(500));
    for _ in 0..5 {
        let _ = net.ws_send(id, WsSend::Text("hello".into()));
    }
    let _ = net.ws_close(id);
    let _ = net.ws_send(id, WsSend::Text("after close".into()));
}
