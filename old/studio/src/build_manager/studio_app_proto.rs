const STUDIO_REMOTE_PATH: &str = "/$studio_remote";
const STUDIO_WEBSOCKET_PATH: &str = "/$studio_web_socket";

// Cross-platform
// Able to dynamically adapt to the current network environment
// whether it is a wired connection, Wi-Fi or VPN.
// But it requires the ability to access external networks.
fn get_local_ip() -> String {
    /*let ipv6 = UdpSocket::bind("[::]:0")
            .and_then(|socket| {
                socket.connect("[2001:4860:4860::8888]:80")?;
                socket.local_addr()
            })
            .ok();
    */
    let ipv4 = UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect("8.8.8.8:80")?;
            socket.local_addr()
        })
        .ok();

    match ipv4 {
        Some(SocketAddr::V4(addr)) if !addr.ip().is_loopback() => addr.ip().to_string(),
        _ => "127.0.0.1".to_string(),
    }
}

impl BuildManager {
    fn handle_external_ip_signal(&mut self) {
        if let Ok(mut addr) = self.recv_external_ip.try_recv() {
            addr.set_port(self.http_port as u16);
            self.studio_http = format!("http://{}{}", addr, STUDIO_WEBSOCKET_PATH);
        }
    }

    fn handle_studio_network_messages(
        &mut self,
        cx: &mut Cx,
        file_system: &mut FileSystem,
        pending_studio_remote_logs: &mut Vec<(LiveId, String, String)>,
    ) {
        let mut needs_redraw_profiler = false;
        while let Ok(message) = self.recv_studio_network_msg.try_recv() {
            match message {
                StudioNetworkMessage::AppDisconnected { build_id } => {
                    cx.action(AppAction::WebsocketDisconnect(build_id));
                    if self.recompiling_builds.contains(&build_id) {
                        continue;
                    }
                    let had_local_process = self.running_processes.remove(&build_id).is_some();
                    self.remove_profile_build(build_id);
                    if let Some(web_socket_id) = self.studio_remote_build_owners.remove(&build_id)
                    {
                        self.send_studio_remote_response(
                            web_socket_id,
                            StudioRemoteResponse::Stopped {
                                build_id: build_id.0,
                            },
                        );
                    }
                    self.studio_remote_latest_widget_dumps.remove(&build_id);
                    self.studio_remote_startup_queries.remove(&build_id);
                    self.studio_remote_startup_dump_pending.remove(&build_id);
                    self.clear_studio_remote_screenshots_for_build(build_id);
                    self.clear_studio_remote_widget_tree_dumps_for_build(build_id);
                    if had_local_process {
                        self.clients[0].send_cmd_with_id(build_id, BuildCmd::Stop);
                    }
                }
                StudioNetworkMessage::RemoteConnected { socket } => {
                    self.studio_remote_sockets
                        .insert(socket.web_socket_id, socket);
                }
                StudioNetworkMessage::RemoteDisconnected { web_socket_id } => {
                    self.studio_remote_sockets.remove(&web_socket_id);
                    let owned_builds: Vec<LiveId> = self
                        .studio_remote_build_owners
                        .iter()
                        .filter_map(|(build_id, owner)| {
                            (*owner == web_socket_id).then_some(*build_id)
                        })
                        .collect();
                    self.studio_remote_build_owners
                        .retain(|_, owner| *owner != web_socket_id);
                    for build_id in owned_builds {
                        self.studio_remote_latest_widget_dumps.remove(&build_id);
                        self.studio_remote_startup_queries.remove(&build_id);
                        self.studio_remote_startup_dump_pending.remove(&build_id);
                    }
                    self.clear_studio_remote_screenshots_for_socket(web_socket_id);
                    self.clear_studio_remote_widget_tree_dumps_for_socket(web_socket_id);
                }
                StudioNetworkMessage::RemoteRequest {
                    web_socket_id,
                    request,
                } => {
                    self.handle_studio_remote_request(cx, web_socket_id, request);
                }
                StudioNetworkMessage::AppToStudio { build_id, msgs } => {
                    self.recompiling_builds.remove(&build_id);
                    cx.action(AppAction::WebsocketReconnect(build_id));
                    self.ensure_active_build(build_id);
                    for msg in msgs.0 {
                        let auto_request_widget_tree_dump = self
                            .studio_remote_startup_dump_pending
                            .contains(&build_id)
                            && matches!(&msg, AppToStudio::DrawCompleteAndFlip(_));
                        if matches!(
                            &msg,
                            AppToStudio::CreateWindow { .. }
                                | AppToStudio::SetCursor(_)
                                | AppToStudio::ReadyToStart
                                | AppToStudio::DrawCompleteAndFlip(_)
                                | AppToStudio::SetClipboard(_)
                                | AppToStudio::RequestAnimationFrame
                                | AppToStudio::TweakHits(_)
                        ) {
                            cx.action(BuildManagerAction::AppToStudio {
                                build_id,
                                msg: msg.clone(),
                            });
                        }
                        if auto_request_widget_tree_dump {
                            if let Some(web_socket_id) =
                                self.studio_remote_build_owners.get(&build_id).copied()
                            {
                                let startup_query =
                                    self.studio_remote_startup_queries.get(&build_id).cloned();
                                let emit_dump = startup_query.is_none();
                                if let Err(message) = self.request_studio_remote_widget_tree_dump(
                                    cx,
                                    web_socket_id,
                                    build_id,
                                    startup_query,
                                    emit_dump,
                                ) {
                                    self.send_studio_remote_error(web_socket_id, message);
                                } else {
                                    self.studio_remote_startup_dump_pending.remove(&build_id);
                                }
                            }
                        }
                        match msg {
                            AppToStudio::LogItem(item) => {
                                let studio_remote_level = log_level_name(item.level);
                                let studio_remote_line = item.message.clone();
                                let file_name = if let Some(build) = self.active.builds.get(&build_id)
                                {
                                    self.roots.map_path(&build.root, &item.file_name)
                                } else {
                                    self.roots.map_path("", &item.file_name)
                                };

                                let start = text::Position {
                                    line_index: item.line_start as usize,
                                    byte_index: item.column_start as usize,
                                };
                                let end = text::Position {
                                    line_index: item.line_end as usize,
                                    byte_index: item.column_end as usize,
                                };
                                if let Some(file_id) = file_system.path_to_file_node_id(&file_name) {
                                    match item.level {
                                        LogLevel::Warning => {
                                            file_system.add_decoration(
                                                file_id,
                                                Decoration::new(
                                                    0,
                                                    start,
                                                    end,
                                                    DecorationType::Warning,
                                                ),
                                            );
                                            cx.action(AppAction::RedrawFile(file_id))
                                        }
                                        LogLevel::Error => {
                                            file_system.add_decoration(
                                                file_id,
                                                Decoration::new(
                                                    0,
                                                    start,
                                                    end,
                                                    DecorationType::Error,
                                                ),
                                            );
                                            cx.action(AppAction::RedrawFile(file_id))
                                        }
                                        _ => (),
                                    }
                                }
                                self.log.push((
                                    build_id,
                                    LogItem::Location(LogItemLocation {
                                        level: item.level,
                                        file_name,
                                        start,
                                        end,
                                        message: item.message,
                                        explanation: item.explanation,
                                    }),
                                ));
                                pending_studio_remote_logs.push((
                                    build_id,
                                    studio_remote_level.to_string(),
                                    studio_remote_line,
                                ));
                                cx.action(AppAction::RedrawLog)
                            }
                            AppToStudio::Screenshot(screenshot) => {
                                self.handle_studio_remote_screenshot_response(build_id, &screenshot);

                                // Keep legacy snapshot path for studio snapshots.
                                if let Some(build) = self.active.builds.get(&build_id) {
                                    file_system.save_snapshot_image(
                                        cx,
                                        &build.root,
                                        "qtest",
                                        screenshot.width as _,
                                        screenshot.height as _,
                                        screenshot.png,
                                    )
                                }
                            }
                            AppToStudio::WidgetTreeDump(dump_response) => {
                                self.handle_studio_remote_widget_tree_dump_response(
                                    build_id,
                                    dump_response,
                                );
                            }
                            AppToStudio::EventSample(sample) => {
                                if self.profiler_running {
                                    self.push_event_profile_sample(build_id, sample);
                                    needs_redraw_profiler = true;
                                }
                            }
                            AppToStudio::GPUSample(sample) => {
                                if self.profiler_running {
                                    self.push_gpu_profile_sample(build_id, sample);
                                    needs_redraw_profiler = true;
                                }
                            }
                            AppToStudio::GCSample(sample) => {
                                if self.profiler_running {
                                    self.push_gc_profile_sample(build_id, sample);
                                    needs_redraw_profiler = true;
                                }
                            }
                            AppToStudio::PatchFile(ef) => cx.action(AppAction::PatchFile(ef)),
                            AppToStudio::EditFile(ef) => cx.action(AppAction::EditFile(ef)),
                            AppToStudio::JumpToFile(jt) => {
                                cx.action(AppAction::JumpTo(jt));
                            }
                            AppToStudio::SelectInFile(jt) => {
                                cx.action(AppAction::SelectInFile(jt));
                            }
                            AppToStudio::SwapSelection(ss) => {
                                cx.action(AppAction::SwapSelection(ss));
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        if needs_redraw_profiler {
            cx.action(AppAction::RedrawProfiler);
        }
    }

    pub fn start_http_server(&mut self, cx: &mut Cx) {
        let (tx_request, rx_request) = mpsc::channel::<HttpServerRequest>();
        const MAX_HTTP_PORT_RETRIES: u16 = 32;

        let mut bound_port = None;
        for offset in 0..MAX_HTTP_PORT_RETRIES {
            let Some(port) = (self.http_port as u16).checked_add(offset) else {
                break;
            };
            let addr = SocketAddr::new("0.0.0.0".parse().unwrap(), port);
            if cx
                .net
                .start_http_server(HttpServer {
                listen_address: addr,
                post_max_size: 1024 * 1024,
                request: tx_request.clone(),
            })
            .is_some()
            {
                bound_port = Some(port as usize);
                break;
            }
        }

        let Some(bound_port) = bound_port else {
            println!(
                "Cannot bind studio http server on ports {}..{}",
                self.http_port,
                self.http_port + (MAX_HTTP_PORT_RETRIES as usize).saturating_sub(1)
            );
            return;
        };

        if bound_port != self.http_port {
            self.http_port = bound_port;
            let local_ip = get_local_ip();
            self.studio_http = format!(
                "http://{}:{}{}",
                local_ip, self.http_port, STUDIO_WEBSOCKET_PATH
            );
            println!("Studio http fallback : {:?}", self.studio_http);
        }

        let studio_network_sender = self.recv_studio_network_msg.sender();
        let active_build_websockets = self.active_build_websockets.clone();
        std::thread::spawn(move || {
            // TODO fix this proper:
            let makepad_path = "./".to_string();
            let abs_makepad_path = std::env::current_dir()
                .unwrap()
                .join(makepad_path.clone())
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let mut root = "./".to_string();
            for arg in std::env::args() {
                if let Some(prefix) = arg.strip_prefix("--root=") {
                    root = prefix.to_string();
                    break;
                }
            }
            let remaps = [
                (
                    format!("/makepad/{}/", abs_makepad_path),
                    makepad_path.clone(),
                ),
                (
                    format!("/makepad/{}/", std::env::current_dir().unwrap().display()),
                    "".to_string(),
                ),
                (
                    "/makepad//".to_string(),
                    format!("{}/{}", root, makepad_path.clone()),
                ),
                (
                    "/makepad/".to_string(),
                    format!("{}/{}", root, makepad_path.clone()),
                ),
                ("/".to_string(), "".to_string()),
            ];
            enum SocketKind {
                App(LiveId),
                StudioRemote,
            }
            let mut socket_kinds: HashMap<u64, SocketKind> = HashMap::new();
            while let Ok(message) = rx_request.recv() {
                match message {
                    HttpServerRequest::ConnectWebSocket {
                        web_socket_id,
                        response_sender,
                        headers,
                    } => {
                        if headers.path == STUDIO_REMOTE_PATH {
                            socket_kinds.insert(web_socket_id, SocketKind::StudioRemote);
                            let _ = studio_network_sender.send(StudioNetworkMessage::RemoteConnected {
                                socket: StudioWebSocket {
                                    web_socket_id,
                                    sender: response_sender,
                                },
                            });
                            continue;
                        }

                        let build_id = headers
                            .path
                            .rsplit('/')
                            .next()
                            .and_then(|id| id.parse::<u64>().ok())
                            .map(LiveId)
                            .unwrap_or(LiveId(web_socket_id));
                        socket_kinds.insert(web_socket_id, SocketKind::App(build_id));
                        active_build_websockets
                            .lock()
                            .unwrap()
                            .borrow_mut()
                            .sockets
                            .push(ActiveBuildSocket {
                                build_id,
                                socket: StudioWebSocket {
                                    web_socket_id,
                                    sender: response_sender,
                                },
                            });
                    }
                    HttpServerRequest::DisconnectWebSocket { web_socket_id } => {
                        if let Some(kind) = socket_kinds.remove(&web_socket_id) {
                            match kind {
                                SocketKind::App(build_id) => {
                                    let still_connected = socket_kinds.values().any(|kind| {
                                        matches!(kind, SocketKind::App(id) if *id == build_id)
                                    });
                                    if !still_connected {
                                        let _ = studio_network_sender
                                            .send(StudioNetworkMessage::AppDisconnected { build_id });
                                    }
                                }
                                SocketKind::StudioRemote => {
                                    let _ = studio_network_sender.send(
                                        StudioNetworkMessage::RemoteDisconnected { web_socket_id },
                                    );
                                }
                            }
                        }
                        active_build_websockets
                            .lock()
                            .unwrap()
                            .borrow_mut()
                            .sockets
                            .retain(|v| v.socket.web_socket_id != web_socket_id);
                    }
                    HttpServerRequest::TextMessage {
                        web_socket_id,
                        response_sender,
                        string,
                    } => {
                        if matches!(socket_kinds.get(&web_socket_id), Some(SocketKind::StudioRemote)) {
                            match StudioRemoteRequest::deserialize_json(&string) {
                                Ok(request) => {
                                    let _ = studio_network_sender.send(
                                        StudioNetworkMessage::RemoteRequest {
                                            web_socket_id,
                                            request,
                                        },
                                    );
                                }
                                Err(err) => {
                                    let message =
                                        format!("invalid studio_remote request: {err:?} json={string}");
                                    BuildManager::send_studio_remote_response_to_sender(
                                        &response_sender,
                                        StudioRemoteResponse::Error {
                                            message: message.clone(),
                                        },
                                    );
                                }
                            }
                        }
                    }
                    HttpServerRequest::BinaryMessage {
                        web_socket_id,
                        response_sender: _,
                        data,
                    } => {
                        if let Some(SocketKind::App(id)) = socket_kinds.get(&web_socket_id) {
                            if let Ok(msg) = AppToStudioVec::deserialize_bin(&data) {
                                let _ = studio_network_sender.send(
                                    StudioNetworkMessage::AppToStudio {
                                        build_id: *id,
                                        msgs: msg,
                                    },
                                );
                            }
                        }
                    }
                    HttpServerRequest::Get {
                        headers,
                        response_sender,
                    } => {
                        let path = &headers.path;
                        if path == "/$watch" {
                            let header = "HTTP/1.1 200 OK\r\n\
                                Cache-Control: max-age:0\r\n\
                                Connection: close\r\n\r\n"
                                .to_string();
                            let _ = response_sender.send(HttpServerResponse {
                                header,
                                body: vec![],
                            });
                            continue;
                        }
                        if path == "/favicon.ico" {
                            let header = "HTTP/1.1 200 OK\r\n\r\n".to_string();
                            let _ = response_sender.send(HttpServerResponse {
                                header,
                                body: vec![],
                            });
                            continue;
                        }

                        let mime_type = if path.ends_with(".html") {
                            "text/html"
                        } else if path.ends_with(".wasm") {
                            "application/wasm"
                        } else if path.ends_with(".css") {
                            "text/css"
                        } else if path.ends_with(".js") {
                            "text/javascript"
                        } else if path.ends_with(".ttf") {
                            "application/ttf"
                        } else if path.ends_with(".png") {
                            "image/png"
                        } else if path.ends_with(".jpg") {
                            "image/jpg"
                        } else if path.ends_with(".svg") {
                            "image/svg+xml"
                        } else if path.ends_with(".md") {
                            "text/markdown"
                        } else {
                            continue;
                        };

                        if path.contains("..") || path.contains('\\') {
                            continue;
                        }

                        let mut strip = None;
                        for remap in &remaps {
                            if let Some(s) = path.strip_prefix(&remap.0) {
                                strip = Some(format!("{}{}", remap.1, s));
                                break;
                            }
                        }
                        if let Some(base) = strip {
                            if let Ok(mut file_handle) = File::open(base) {
                                let mut body = Vec::<u8>::new();
                                if file_handle.read_to_end(&mut body).is_ok() {
                                    let header = format!(
                                        "HTTP/1.1 200 OK\r\n\
                                            Content-Type: {}\r\n\
                                            Cross-Origin-Embedder-Policy: require-corp\r\n\
                                            Cross-Origin-Opener-Policy: same-origin\r\n\
                                            Content-encoding: none\r\n\
                                            Cache-Control: max-age:0\r\n\
                                            Content-Length: {}\r\n\
                                            Connection: close\r\n\r\n",
                                        mime_type,
                                        body.len()
                                    );
                                    let _ = response_sender.send(HttpServerResponse { header, body });
                                }
                            }
                        }
                    }
                    HttpServerRequest::Post { .. } => {}
                }
            }
        });
    }

    pub fn discover_external_ip(&mut self, _cx: &mut Cx) {
        let studio_uid = LiveId::from_str(&format!(
            "{:?}{:?}",
            Instant::now(),
            std::time::SystemTime::now()
        ));
        let http_port = self.http_port as u16;
        let write_discovery = UdpSocket::bind(SocketAddr::new(
            "0.0.0.0".parse().unwrap(),
            http_port * 2 as u16 + 1,
        ));
        if write_discovery.is_err() {
            return;
        }
        let write_discovery = write_discovery.unwrap();
        write_discovery
            .set_read_timeout(Some(Duration::new(0, 1)))
            .unwrap();
        write_discovery.set_broadcast(true).unwrap();
        std::thread::spawn(move || {
            let dummy = studio_uid.0.to_be_bytes();
            loop {
                let _ = write_discovery.send_to(
                    &dummy,
                    SocketAddr::new("0.0.0.0".parse().unwrap(), http_port * 2 as u16),
                );
                thread::sleep(time::Duration::from_millis(100));
            }
        });

        let ip_sender = self.recv_external_ip.sender();
        std::thread::spawn(move || {
            let discovery = UdpSocket::bind(SocketAddr::new(
                "0.0.0.0".parse().unwrap(),
                http_port * 2 as u16,
            ))
            .unwrap();
            discovery
                .set_read_timeout(Some(Duration::new(0, 1)))
                .unwrap();
            discovery.set_broadcast(true).unwrap();

            let mut other_uid = [0u8; 8];
            'outer: loop {
                while let Ok((_, addr)) = discovery.recv_from(&mut other_uid) {
                    let recv_uid = u64::from_be_bytes(other_uid);
                    if studio_uid.0 == recv_uid {
                        let _ = ip_sender.send(addr);
                        break 'outer;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
    }
}
