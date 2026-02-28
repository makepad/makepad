use makepad_live_id::LiveId;
use makepad_micro_serde::{DeBin, SerBin};
use makepad_network::{
    HttpMethod, HttpRequest, NetworkConfig, NetworkResponse, NetworkRuntime, WsMessage, WsSend,
};
use makepad_studio_backend::{
    BackendConfig, ClientId, MountConfig, QueryId, StudioBackend, StudioToUI, UIToStudio,
    UIToStudioEnvelope,
};
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::time::{Duration, Instant};

fn find_free_port() -> Option<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    Some(listener.local_addr().ok()?.port())
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

fn wait_for_ws_binary(runtime: &NetworkRuntime, socket_id: LiveId, timeout: Duration) -> Vec<u8> {
    let event = wait_for_event(runtime, timeout, |event| {
        matches!(
            event,
            NetworkResponse::WsMessage {
                socket_id: id,
                message: WsMessage::Binary(_)
            } if *id == socket_id
        )
    })
    .expect("did not receive ws binary message");

    match event {
        NetworkResponse::WsMessage {
            message: WsMessage::Binary(data),
            ..
        } => data,
        _ => unreachable!(),
    }
}

#[test]
fn websocket_ui_hello_and_load_file_tree_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "fn main() {}\n").unwrap();

    let Some(port) = find_free_port() else {
        return;
    };
    let config = BackendConfig {
        listen_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        post_max_size: 1024 * 1024,
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        enable_in_process_gateway: false,
    };

    let _backend = match StudioBackend::start_headless(config) {
        Ok(v) => v,
        Err(err) => {
            if err.contains("failed to bind") {
                return;
            }
            panic!("start backend failed: {}", err);
        }
    };

    let runtime = NetworkRuntime::new(NetworkConfig::default());
    let socket_id = LiveId::from_str("studio2.backend.gateway.test");
    let request = HttpRequest::new(format!("ws://127.0.0.1:{port}/$studio_ui"), HttpMethod::GET);
    if runtime.ws_open(socket_id, request).is_err() {
        return;
    }

    let opened = wait_for_event(
        &runtime,
        Duration::from_secs(3),
        |event| matches!(event, NetworkResponse::WsOpened { socket_id: id } if *id == socket_id),
    );
    assert!(opened.is_some(), "did not receive WsOpened");

    let hello_bin = wait_for_ws_binary(&runtime, socket_id, Duration::from_secs(3));
    let hello = StudioToUI::deserialize_bin(&hello_bin).expect("decode hello");
    let client_id = match hello {
        StudioToUI::Hello { client_id } => client_id,
        other => panic!("expected hello, got {:?}", other),
    };
    assert_ne!(client_id, ClientId(u16::MAX));

    let envelope = UIToStudioEnvelope {
        query_id: QueryId::new(client_id, 0),
        msg: UIToStudio::LoadFileTree {
            mount: "repo".to_string(),
        },
    };
    runtime
        .ws_send(socket_id, WsSend::Binary(envelope.serialize_bin()))
        .expect("ws_send");

    let tree_bin = wait_for_ws_binary(&runtime, socket_id, Duration::from_secs(3));
    let tree_msg = StudioToUI::deserialize_bin(&tree_bin).expect("decode tree");
    match tree_msg {
        StudioToUI::FileTree { mount, data } => {
            assert_eq!(mount, "repo");
            assert!(data.nodes.iter().any(|n| n.path == "repo/src/lib.rs"));
        }
        other => panic!("expected FileTree, got {:?}", other),
    }

    let _ = runtime.ws_close(socket_id);
}
