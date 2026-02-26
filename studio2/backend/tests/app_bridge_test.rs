use makepad_live_id::LiveId;
use makepad_micro_serde::{DeBin, SerBin};
use makepad_network::{
    HttpMethod, HttpRequest, NetworkConfig, NetworkResponse, NetworkRuntime, WsMessage, WsSend,
};
use makepad_studio_backend::{
    AppToStudioMsg, AppToStudioVec, BackendConfig, ClientId, MountConfig, QueryId, StudioBackend,
    StudioToAppMsg, StudioToAppVec, StudioToUI, UIToStudio, UIToStudioEnvelope,
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
fn websocket_app_bridge_widget_dump_roundtrip() {
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
    let ui_socket = LiveId::from_str("studio2.backend.ui");
    let ui_request = HttpRequest::new(format!("ws://127.0.0.1:{port}/$studio_ui"), HttpMethod::GET);
    if runtime.ws_open(ui_socket, ui_request).is_err() {
        return;
    }
    let ui_opened = wait_for_event(&runtime, Duration::from_secs(3), |event| {
        matches!(event, NetworkResponse::WsOpened { socket_id: id } if *id == ui_socket)
    });
    assert!(ui_opened.is_some(), "did not receive ui WsOpened");

    let hello_bin = wait_for_ws_binary(&runtime, ui_socket, Duration::from_secs(3));
    let hello = StudioToUI::deserialize_bin(&hello_bin).expect("decode hello");
    let client_id = match hello {
        StudioToUI::Hello { client_id } => client_id,
        other => panic!("expected hello, got {:?}", other),
    };
    assert_ne!(client_id, ClientId(u16::MAX));

    let build_id = QueryId::new(client_id, 100);
    let app_socket = LiveId::from_str("studio2.backend.app");
    let app_request = HttpRequest::new(
        format!("ws://127.0.0.1:{port}/$studio_app/{}", build_id.0),
        HttpMethod::GET,
    );
    runtime.ws_open(app_socket, app_request).expect("open app socket");
    let app_opened = wait_for_event(&runtime, Duration::from_secs(3), |event| {
        matches!(event, NetworkResponse::WsOpened { socket_id: id } if *id == app_socket)
    });
    assert!(app_opened.is_some(), "did not receive app WsOpened");

    let query_id = QueryId::new(client_id, 1);
    let ui_request = UIToStudioEnvelope {
        query_id,
        msg: UIToStudio::WidgetTreeDump { build_id },
    };
    runtime
        .ws_send(ui_socket, WsSend::Binary(ui_request.serialize_bin()))
        .expect("send widget dump request");

    let app_incoming = wait_for_ws_binary(&runtime, app_socket, Duration::from_secs(3));
    let app_msg = StudioToAppVec::deserialize_bin(&app_incoming).expect("decode app command");
    assert_eq!(app_msg.0.len(), 1);
    match &app_msg.0[0] {
        StudioToAppMsg::WidgetTreeDumpRequest { request_id } => {
            assert_eq!(*request_id, query_id.0);
        }
        other => panic!("unexpected app message: {:?}", other),
    }

    let response = AppToStudioVec(vec![AppToStudioMsg::WidgetTreeDump {
        request_id: query_id.0,
        dump: "W1 root View 0 0 10 10".to_string(),
    }]);
    runtime
        .ws_send(app_socket, WsSend::Binary(response.serialize_bin()))
        .expect("send app response");

    let ui_incoming = wait_for_ws_binary(&runtime, ui_socket, Duration::from_secs(3));
    let ui_msg = StudioToUI::deserialize_bin(&ui_incoming).expect("decode ui response");
    match ui_msg {
        StudioToUI::WidgetTreeDump {
            query_id: got_query,
            build_id: got_build,
            dump,
        } => {
            assert_eq!(got_query, query_id);
            assert_eq!(got_build, build_id);
            assert!(dump.contains("root"));
        }
        other => panic!("unexpected ui response: {:?}", other),
    }

    let _ = runtime.ws_close(app_socket);
    let _ = runtime.ws_close(ui_socket);
}
