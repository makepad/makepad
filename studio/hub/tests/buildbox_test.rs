use makepad_live_id::LiveId;
use makepad_micro_serde::{DeBin, SerBin};
use makepad_script_std::makepad_network::{
    HttpMethod, HttpRequest, NetworkConfig, NetworkResponse, NetworkRuntime, WsMessage, WsSend,
};
use makepad_studio_hub::{HubConfig, MountConfig, StudioHub};
use makepad_studio_protocol::hub_protocol::{
    BuildBoxToHub, BuildBoxToHubVec, ClientId, ClientToHub, ClientToHubEnvelope, HubToBuildBox,
    HubToBuildBoxVec, HubToClient, QueryId,
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

fn wait_for_ui_message<F>(
    runtime: &NetworkRuntime,
    socket_id: LiveId,
    timeout: Duration,
    mut matcher: F,
) -> Option<HubToClient>
where
    F: FnMut(&HubToClient) -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let event = runtime.recv_timeout(Duration::from_millis(50))?;
        if let NetworkResponse::WsMessage {
            socket_id: id,
            message: WsMessage::Binary(data),
        } = event
        {
            if id != socket_id {
                continue;
            }
            if let Ok(msg) = HubToClient::deserialize_bin(&data) {
                if matcher(&msg) {
                    return Some(msg);
                }
            }
        }
    }
    None
}

fn wait_for_buildbox_message(
    runtime: &NetworkRuntime,
    socket_id: LiveId,
    timeout: Duration,
) -> Option<HubToBuildBoxVec> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let event = runtime.recv_timeout(Duration::from_millis(50))?;
        if let NetworkResponse::WsMessage {
            socket_id: id,
            message: WsMessage::Binary(data),
        } = event
        {
            if id != socket_id {
                continue;
            }
            if let Ok(msgs) = HubToBuildBoxVec::deserialize_bin(&data) {
                return Some(msgs);
            }
        }
    }
    None
}

#[test]
fn websocket_buildbox_remote_build_roundtrip() {
    let dir = makepad_studio_hub::test_support::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "fn main() {}\n").unwrap();

    let Some(port) = find_free_port() else {
        return;
    };
    let config = HubConfig {
        listen_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        post_max_size: 1024 * 1024,
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        enable_in_process_gateway: false,
    };

    let _backend = match StudioHub::start_headless(config) {
        Ok(v) => v,
        Err(err) => {
            if err.contains("failed to bind") {
                return;
            }
            panic!("start backend failed: {}", err);
        }
    };

    let runtime = NetworkRuntime::new(NetworkConfig::default());

    let ui_socket = LiveId::from_str("studio2.backend.buildbox.ui");
    let ui_request = HttpRequest::new(format!("ws://127.0.0.1:{port}/$studio_ui"), HttpMethod::GET);
    if runtime.ws_open(ui_socket, ui_request).is_err() {
        return;
    }
    let ui_opened = wait_for_event(
        &runtime,
        Duration::from_secs(3),
        |event| matches!(event, NetworkResponse::WsOpened { socket_id: id } if *id == ui_socket),
    );
    assert!(ui_opened.is_some(), "did not receive ui WsOpened");

    let hello = wait_for_ui_message(&runtime, ui_socket, Duration::from_secs(3), |msg| {
        matches!(msg, HubToClient::Hello { .. })
    })
    .expect("did not receive hello");
    let client_id = match hello {
        HubToClient::Hello { client_id } => client_id,
        _ => unreachable!(),
    };
    assert_ne!(client_id, ClientId(u16::MAX));

    let buildbox_socket = LiveId::from_str("studio2.backend.buildbox.remote");
    let buildbox_request = HttpRequest::new(
        format!("ws://127.0.0.1:{port}/$studio_buildbox"),
        HttpMethod::GET,
    );
    runtime
        .ws_open(buildbox_socket, buildbox_request)
        .expect("open buildbox socket");
    let bb_opened = wait_for_event(
        &runtime,
        Duration::from_secs(3),
        |event| matches!(event, NetworkResponse::WsOpened { socket_id: id } if *id == buildbox_socket),
    );
    assert!(bb_opened.is_some(), "did not receive buildbox WsOpened");

    let hello = BuildBoxToHubVec(vec![BuildBoxToHub::Hello {
        name: "linux".to_string(),
        platform: "linux".to_string(),
        arch: "x86_64".to_string(),
        tree_hash: "abc".to_string(),
    }]);
    runtime
        .ws_send(buildbox_socket, WsSend::Binary(hello.serialize_bin()))
        .expect("send buildbox hello");

    let connected = wait_for_ui_message(&runtime, ui_socket, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            HubToClient::BuildBoxConnected { info } if info.name == "linux"
        )
    });
    assert!(connected.is_some(), "did not receive BuildBoxConnected");

    let list_query = ClientToHubEnvelope {
        query_id: QueryId::new(client_id, 0),
        msg: ClientToHub::ListBuildBoxes,
    };
    runtime
        .ws_send(ui_socket, WsSend::Binary(list_query.serialize_bin()))
        .expect("send list buildboxes");
    let boxes = wait_for_ui_message(&runtime, ui_socket, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            HubToClient::BuildBoxes { boxes } if boxes.iter().any(|b| b.name == "linux")
        )
    })
    .expect("did not receive BuildBoxes");
    match boxes {
        HubToClient::BuildBoxes { boxes } => {
            let linux = boxes.iter().find(|b| b.name == "linux").unwrap();
            assert_eq!(linux.platform, "linux");
        }
        _ => unreachable!(),
    }

    let sync_query = ClientToHubEnvelope {
        query_id: QueryId::new(client_id, 1),
        msg: ClientToHub::BuildBoxSyncNow {
            name: "linux".to_string(),
        },
    };
    runtime
        .ws_send(ui_socket, WsSend::Binary(sync_query.serialize_bin()))
        .expect("send buildbox sync now");
    let sync_cmd = wait_for_buildbox_message(&runtime, buildbox_socket, Duration::from_secs(3))
        .expect("did not receive buildbox sync command");
    assert_eq!(sync_cmd.0.len(), 1);
    assert!(matches!(sync_cmd.0[0], HubToBuildBox::RequestTreeHash));

    let run_query_id = QueryId::new(client_id, 2);
    let run_query = ClientToHubEnvelope {
        query_id: run_query_id,
        msg: ClientToHub::Cargo {
            mount: "repo".to_string(),
            args: vec!["-p".to_string(), "remote-app".to_string()],
            env: None,
            buildbox: Some("linux".to_string()),
        },
    };
    runtime
        .ws_send(ui_socket, WsSend::Binary(run_query.serialize_bin()))
        .expect("send remote cargo run");

    let started = wait_for_ui_message(
        &runtime,
        ui_socket,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::BuildStarted { mount, .. } if mount == "repo"),
    );
    let build_id = match started {
        Some(HubToClient::BuildStarted { build_id, .. }) => build_id,
        _ => panic!("did not receive BuildStarted"),
    };

    let cargo_cmd = wait_for_buildbox_message(&runtime, buildbox_socket, Duration::from_secs(3))
        .expect("did not receive buildbox cargo command");
    assert_eq!(cargo_cmd.0.len(), 1);
    match &cargo_cmd.0[0] {
        HubToBuildBox::CargoBuild {
            build_id: id,
            mount,
            args,
            ..
        } => {
            assert_eq!(*id, build_id);
            assert_eq!(mount, "repo");
            assert_eq!(args, &vec!["-p".to_string(), "remote-app".to_string()]);
        }
        other => panic!("unexpected buildbox command: {:?}", other),
    }

    let output = BuildBoxToHubVec(vec![BuildBoxToHub::BuildOutput {
        build_id,
        line: "remote build line".to_string(),
    }]);
    runtime
        .ws_send(buildbox_socket, WsSend::Binary(output.serialize_bin()))
        .expect("send buildbox output");

    let log_query_id = QueryId::new(client_id, 3);
    let query_logs = ClientToHubEnvelope {
        query_id: log_query_id,
        msg: ClientToHub::QueryLogs {
            build_id: Some(build_id),
            level: None,
            source: None,
            file: None,
            pattern: Some("remote build".to_string()),
            is_regex: None,
            since_index: None,
            live: Some(false),
        },
    };
    runtime
        .ws_send(ui_socket, WsSend::Binary(query_logs.serialize_bin()))
        .expect("send log query");
    let log_result = wait_for_ui_message(&runtime, ui_socket, Duration::from_secs(3), |msg| {
        matches!(msg, HubToClient::QueryLogResults { query_id, .. } if *query_id == log_query_id)
    })
    .expect("did not receive QueryLogResults");
    match log_result {
        HubToClient::QueryLogResults { entries, .. } => {
            assert!(entries
                .iter()
                .any(|(_, e)| e.message.contains("remote build line")));
        }
        _ => unreachable!(),
    }

    let stopped = BuildBoxToHubVec(vec![BuildBoxToHub::BuildStopped {
        build_id,
        exit_code: Some(0),
    }]);
    runtime
        .ws_send(buildbox_socket, WsSend::Binary(stopped.serialize_bin()))
        .expect("send buildbox stop");
    let stopped = wait_for_ui_message(&runtime, ui_socket, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            HubToClient::BuildStopped {
                build_id: id,
                exit_code: Some(0)
            } if *id == build_id
        )
    });
    assert!(stopped.is_some(), "did not receive BuildStopped");

    let _ = runtime.ws_close(buildbox_socket);
    let _ = runtime.ws_close(ui_socket);
}
