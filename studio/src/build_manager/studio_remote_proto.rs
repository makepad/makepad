#[derive(Clone, Copy, Debug)]
struct PendingStudioRemoteScreenshot {
    web_socket_id: u64,
    build_id: LiveId,
    kind_id: u32,
}

#[derive(Clone, Debug)]
struct PendingStudioRemoteWidgetTreeDump {
    web_socket_id: u64,
    build_id: LiveId,
    emit_dump: bool,
    startup_query: Option<String>,
}

struct UniqueIdMap<T> {
    next_id: u64,
    entries: HashMap<u64, T>,
}

impl<T> Default for UniqueIdMap<T> {
    fn default() -> Self {
        Self {
            next_id: 0,
            entries: HashMap::new(),
        }
    }
}

impl<T> UniqueIdMap<T> {
    fn insert_unique(&mut self, value: T) -> u64 {
        let id = loop {
            self.next_id = self.next_id.wrapping_add(1);
            if self.next_id != 0 && !self.entries.contains_key(&self.next_id) {
                break self.next_id;
            }
        };
        self.entries.insert(id, value);
        id
    }

    fn remove(&mut self, id: &u64) -> Option<T> {
        self.entries.remove(id)
    }

    fn retain<F>(&mut self, f: F)
    where
        F: FnMut(&u64, &mut T) -> bool,
    {
        self.entries.retain(f);
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

#[derive(Debug, Clone, SerJson, DeJson)]
pub enum StudioRemoteRequest {
    ListBuilds,
    CargoRun {
        args: Vec<String>,
        root: Option<String>,
        startup_query: Option<String>,
        env: Option<HashMap<String, String>>,
    },
    Stop {
        build_id: u64,
    },
    StudioToApp {
        build_id: u64,
        msg: StudioToApp,
    },
    TypeText {
        build_id: u64,
        text: String,
        replace_last: Option<bool>,
        was_paste: Option<bool>,
        auto_dump: Option<bool>,
    },
    Return {
        build_id: u64,
        auto_dump: Option<bool>,
    },
    Click {
        build_id: u64,
        x: i64,
        y: i64,
        button: Option<u32>,
        auto_dump: Option<bool>,
    },
    Screenshot {
        build_id: u64,
        kind_id: Option<u32>,
    },
    WidgetTreeDump {
        build_id: u64,
    },
    WidgetQuery {
        build_id: u64,
        query: String,
    },
}

#[derive(Debug, Clone, SerJson, DeJson)]
pub enum StudioRemoteResponse {
    Builds {
        builds: Vec<StudioRemoteBuildInfo>,
    },
    Started {
        build_id: u64,
        root: String,
        package: String,
    },
    Stopped {
        build_id: u64,
    },
    Log {
        build_id: u64,
        level: String,
        line: String,
    },
    Screenshot {
        build_id: u64,
        request_id: u64,
        kind_id: u32,
        path: String,
        width: u32,
        height: u32,
    },
    WidgetTreeDump {
        build_id: u64,
        request_id: u64,
        dump: String,
    },
    WidgetQuery {
        build_id: u64,
        query: String,
        rects: Vec<String>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, SerJson, DeJson)]
pub struct StudioRemoteBuildInfo {
    pub build_id: u64,
    pub root: String,
    pub package: String,
    pub active: bool,
    pub has_web_socket: bool,
}

enum StudioRemoteSocket {
    Connected {
        web_socket_id: u64,
        sender: mpsc::Sender<Vec<u8>>,
    },
    Disconnected {
        web_socket_id: u64,
    },
    Request {
        web_socket_id: u64,
        request: StudioRemoteRequest,
    },
}

struct CompactF64(f64);

impl std::fmt::Display for CompactF64 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = self.0;
        let rounded = value.round();
        if (value - rounded).abs() < 1e-9 {
            write!(f, "{:.0}", rounded)
        } else {
            write!(f, "{}", value)
        }
    }
}

fn has_message_format_json(args: &[String]) -> bool {
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--message-format=json" {
            return true;
        }
        if arg == "--message-format" && iter.peek().is_some_and(|next| next.as_str() == "json") {
            return true;
        }
        if arg
            .strip_prefix("--message-format=")
            .is_some_and(|value| value == "json")
        {
            return true;
        }
    }
    false
}

fn normalize_studio_remote_cargo_run_args(raw_args: Vec<String>) -> Result<Vec<String>, String> {
    let mut args = raw_args;
    if args.first().is_some_and(|arg| arg == "run") {
        args.remove(0);
    }
    if args
        .first()
        .is_some_and(|arg| !arg.starts_with('-') && arg != "--")
    {
        return Err(
            "CargoRun expects args after `cargo run` (do not pass a different cargo subcommand)"
                .to_string(),
        );
    }

    let split_index = args
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len());
    let mut cargo_args = args[..split_index].to_vec();
    let mut app_args = if split_index < args.len() {
        args[(split_index + 1)..].to_vec()
    } else {
        Vec::new()
    };

    if !has_message_format_json(&cargo_args) {
        cargo_args.push("--message-format=json".to_string());
    }
    if !has_message_format_json(&app_args) {
        app_args.insert(0, "--message-format=json".to_string());
    }
    if !app_args.iter().any(|arg| arg == "--stdin-loop") {
        app_args.insert(0, "--stdin-loop".to_string());
    }

    let mut final_args = vec!["run".to_string()];
    final_args.extend(cargo_args);
    final_args.push("--".to_string());
    final_args.extend(app_args);
    Ok(final_args)
}

fn normalize_studio_remote_env_map(
    raw_env: Option<HashMap<String, String>>,
) -> Result<HashMap<String, String>, String> {
    let mut out = HashMap::new();
    let Some(raw_env) = raw_env else {
        return Ok(out);
    };
    for (key, value) in raw_env {
        let key = key.trim().to_string();
        if key.is_empty() {
            return Err("CargoRun.env contains an empty env var name".to_string());
        }
        if key.contains('=') || key.contains('\0') {
            return Err(format!(
                "CargoRun.env contains invalid env var name '{}'",
                key
            ));
        }
        out.insert(key, value);
    }
    Ok(out)
}

fn cargo_run_is_release(cargo_args: &[String]) -> bool {
    let run_args = if cargo_args.first().is_some_and(|arg| arg == "run") {
        &cargo_args[1..]
    } else {
        cargo_args
    };
    let split_index = run_args
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(run_args.len());
    run_args[..split_index].iter().any(|arg| arg == "--release")
}

fn parse_studio_remote_package_name(cargo_args: &[String]) -> Option<String> {
    let run_args = if cargo_args.first().is_some_and(|arg| arg == "run") {
        &cargo_args[1..]
    } else {
        cargo_args
    };
    let split_index = run_args
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(run_args.len());
    let args = &run_args[..split_index];
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "-p" | "--package" if i + 1 < args.len() => return Some(args[i + 1].clone()),
            "--bin" if i + 1 < args.len() => return Some(args[i + 1].clone()),
            arg if arg.starts_with("--package=") => {
                return arg.split_once('=').map(|(_, value)| value.to_string());
            }
            arg if arg.starts_with("--bin=") => {
                return arg.split_once('=').map(|(_, value)| value.to_string());
            }
            _ => {}
        }
        i += 1;
    }
    None
}

impl BuildManager {
    fn alloc_studio_remote_build_id(&mut self) -> LiveId {
        loop {
            self.studio_remote_build_counter = self.studio_remote_build_counter.wrapping_add(1);
            let build_id = LiveId::from_str("studio_remote")
                .bytes_append(&self.studio_remote_build_counter.to_be_bytes());
            if !self.running_processes.contains_key(&build_id)
                && !self.active.builds.contains_key(&build_id)
            {
                return build_id;
            }
        }
    }

    fn clear_studio_remote_screenshots_for_socket(&mut self, web_socket_id: u64) {
        self.studio_remote_screenshot_requests
            .retain(|_, pending| pending.web_socket_id != web_socket_id);
    }

    fn clear_studio_remote_screenshots_for_build(&mut self, build_id: LiveId) {
        self.studio_remote_screenshot_requests
            .retain(|_, pending| pending.build_id != build_id);
    }

    fn clear_studio_remote_widget_tree_dumps_for_socket(&mut self, web_socket_id: u64) {
        self.studio_remote_widget_tree_dump_requests
            .retain(|_, pending| pending.web_socket_id != web_socket_id);
    }

    fn clear_studio_remote_widget_tree_dumps_for_build(&mut self, build_id: LiveId) {
        self.studio_remote_widget_tree_dump_requests
            .retain(|_, pending| pending.build_id != build_id);
    }

    fn query_widget_dump_rects(dump: &str, query: &str) -> Vec<String> {
        let query = query.trim();
        let (mode, needle) = if let Some(v) = query.strip_prefix("id:") {
            ("id", v.trim())
        } else if let Some(v) = query.strip_prefix("type:") {
            ("type", v.trim())
        } else {
            ("any", query)
        };

        let mut rects = Vec::new();
        for line in dump.lines() {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 8 {
                continue;
            }
            if cols[0].starts_with('W') || cols[0] == "O" {
                continue;
            }
            let id = cols[2];
            let ty = cols[3];
            let is_match = match mode {
                "id" => id == needle,
                "type" => ty == needle,
                _ => needle.is_empty() || id.contains(needle) || ty.contains(needle),
            };
            if !is_match {
                continue;
            }
            rects.push(format!(
                "{} {} {} {} {} {} {}",
                cols[0], id, ty, cols[4], cols[5], cols[6], cols[7]
            ));
            if rects.len() >= 256 {
                break;
            }
        }
        rects
    }

    fn studio_remote_control_log_label(msg: &StudioToApp) -> String {
        match msg {
            StudioToApp::MouseDown(e) => {
                format!(
                    "MouseDown x={} y={} button={}",
                    CompactF64(e.x),
                    CompactF64(e.y),
                    e.button_raw_bits
                )
            }
            StudioToApp::MouseMove(e) => {
                format!("MouseMove x={} y={}", CompactF64(e.x), CompactF64(e.y))
            }
            StudioToApp::TweakRay(e) => {
                format!("TweakRay x={} y={}", CompactF64(e.x), CompactF64(e.y))
            }
            StudioToApp::MouseUp(e) => {
                format!(
                    "MouseUp x={} y={} button={}",
                    CompactF64(e.x),
                    CompactF64(e.y),
                    e.button_raw_bits
                )
            }
            StudioToApp::Scroll(e) => format!(
                "Scroll x={} y={} sx={} sy={}",
                CompactF64(e.x),
                CompactF64(e.y),
                CompactF64(e.sx),
                CompactF64(e.sy)
            ),
            StudioToApp::Tick => "Tick".to_string(),
            StudioToApp::TextCopy => "TextCopy".to_string(),
            StudioToApp::TextCut => "TextCut".to_string(),
            StudioToApp::TextInput(e) => {
                let mut text = e.input.clone();
                if text.len() > 48 {
                    text.truncate(48);
                    text.push_str("...");
                }
                format!("TextInput {:?}", text)
            }
            StudioToApp::KeyDown(e) => format!("KeyDown {:?}", e.key_code),
            StudioToApp::KeyUp(e) => format!("KeyUp {:?}", e.key_code),
            StudioToApp::Swapchain(_) => "Swapchain".to_string(),
            StudioToApp::WindowGeomChange {
                window_id,
                dpi_factor,
                left,
                top,
                width,
                height,
            } => format!(
                "WindowGeomChange window={} left={} top={} width={} height={} dpi={}",
                window_id,
                CompactF64(*left),
                CompactF64(*top),
                CompactF64(*width),
                CompactF64(*height),
                CompactF64(*dpi_factor)
            ),
            StudioToApp::Screenshot(_) => "Screenshot".to_string(),
            StudioToApp::WidgetTreeDump(_) => "WidgetTreeDump".to_string(),
            StudioToApp::KeepAlive => "KeepAlive".to_string(),
            StudioToApp::LiveChange { file_name, .. } => {
                format!("LiveChange file={}", file_name)
            }
            StudioToApp::None => "None".to_string(),
            StudioToApp::Kill => "Kill".to_string(),
        }
    }

    fn write_studio_remote_screenshot_png(
        &self,
        build_id: LiveId,
        kind_id: u32,
        request_id: u64,
        png: &[u8],
    ) -> Result<PathBuf, String> {
        let mut dir = std::env::temp_dir();
        dir.push("makepad_studio_remote");
        std::fs::create_dir_all(&dir).map_err(|err| {
            format!(
                "failed to create screenshot temp dir {}: {err}",
                dir.display()
            )
        })?;

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| format!("system time error: {err}"))?
            .as_millis();
        let file_name = format!(
            "build-{}-kind-{}-req-{}-{}.png",
            build_id.0, kind_id, request_id, now_ms
        );
        let path = dir.join(file_name);
        std::fs::write(&path, png)
            .map_err(|err| format!("failed to write screenshot png {}: {err}", path.display()))?;
        Ok(path)
    }

    fn send_studio_remote_response_to_sender(
        sender: &mpsc::Sender<Vec<u8>>,
        response: StudioRemoteResponse,
    ) {
        let _ = sender.send(response.serialize_json().into_bytes());
    }

    fn send_studio_remote_response(&self, web_socket_id: u64, response: StudioRemoteResponse) {
        if let Some(sender) = self.studio_remote_sockets.get(&web_socket_id) {
            Self::send_studio_remote_response_to_sender(sender, response);
        }
    }

    fn send_studio_remote_log(&self, build_id: LiveId, level: &str, line: String) {
        let Some(web_socket_id) = self.studio_remote_build_owners.get(&build_id).copied() else {
            return;
        };
        self.send_studio_remote_response(
            web_socket_id,
            StudioRemoteResponse::Log {
                build_id: build_id.0,
                level: level.to_string(),
                line,
            },
        );
    }

    fn send_studio_remote_error(&self, web_socket_id: u64, message: impl Into<String>) {
        self.send_studio_remote_response(
            web_socket_id,
            StudioRemoteResponse::Error {
                message: message.into(),
            },
        );
    }

    fn log_studio_remote_bridge_event(&mut self, cx: &mut Cx, line: String) {
        self.log.push((
            LiveId::from_str("studio_remote_bridge"),
            LogItem::Bare(LogItemBare {
                level: LogLevel::Log,
                line,
            }),
        ));
        cx.action(AppAction::RedrawLog);
    }

    fn request_studio_remote_screenshot(
        &mut self,
        cx: &mut Cx,
        web_socket_id: u64,
        build_id: LiveId,
        kind_id: u32,
    ) -> Result<(), String> {
        let request_id = self.studio_remote_screenshot_requests.insert_unique(
            PendingStudioRemoteScreenshot {
                web_socket_id,
                build_id,
                kind_id,
            },
        );

        let sent = if let Ok(sockets) = self.active_build_websockets.lock() {
            sockets.borrow_mut().send_studio_to_app(
                build_id,
                StudioToApp::Screenshot(ScreenshotRequest {
                    request_id,
                    kind_id,
                }),
            )
        } else {
            false
        };

        if sent {
            self.log_studio_remote_bridge_event(
                cx,
                format!(
                    "studio_remote -> child build={} Screenshot request_id={} kind_id={}",
                    build_id.0, request_id, kind_id
                ),
            );
            Ok(())
        } else {
            self.studio_remote_screenshot_requests.remove(&request_id);
            Err(format!(
                "build {} has no active studio websocket connection",
                build_id.0
            ))
        }
    }

    fn request_studio_remote_widget_tree_dump(
        &mut self,
        cx: &mut Cx,
        web_socket_id: u64,
        build_id: LiveId,
        startup_query: Option<String>,
        emit_dump: bool,
    ) -> Result<(), String> {
        let request_id = self.studio_remote_widget_tree_dump_requests.insert_unique(
            PendingStudioRemoteWidgetTreeDump {
                web_socket_id,
                build_id,
                emit_dump,
                startup_query,
            },
        );

        let sent = if let Ok(sockets) = self.active_build_websockets.lock() {
            sockets.borrow_mut().send_studio_to_app(
                build_id,
                StudioToApp::WidgetTreeDump(WidgetTreeDumpRequest { request_id }),
            )
        } else {
            false
        };

        if sent {
            self.log_studio_remote_bridge_event(
                cx,
                format!(
                    "studio_remote -> child build={} WidgetTreeDump request_id={}",
                    build_id.0, request_id
                ),
            );
            Ok(())
        } else {
            self.studio_remote_widget_tree_dump_requests.remove(&request_id);
            Err(format!(
                "build {} has no active studio websocket connection",
                build_id.0
            ))
        }
    }

    fn handle_studio_remote_screenshot_response(
        &mut self,
        _build_id: LiveId,
        screenshot: &ScreenshotResponse,
    ) {
        let pending: Vec<(u64, PendingStudioRemoteScreenshot)> = screenshot
            .request_ids
            .iter()
            .filter_map(|request_id| {
                self.studio_remote_screenshot_requests
                    .remove(request_id)
                    .map(|pending| (*request_id, pending))
            })
            .collect();

        if pending.is_empty() {
            return;
        }

        for (request_id, pending_request) in pending {
            match self.write_studio_remote_screenshot_png(
                pending_request.build_id,
                pending_request.kind_id,
                request_id,
                &screenshot.png,
            ) {
                Ok(path) => self.send_studio_remote_response(
                    pending_request.web_socket_id,
                    StudioRemoteResponse::Screenshot {
                        build_id: pending_request.build_id.0,
                        request_id,
                        kind_id: pending_request.kind_id,
                        path: path.to_string_lossy().into_owned(),
                        width: screenshot.width,
                        height: screenshot.height,
                    },
                ),
                Err(err) => self.send_studio_remote_error(pending_request.web_socket_id, err),
            }
        }
    }

    fn handle_studio_remote_widget_tree_dump_response(
        &mut self,
        build_id: LiveId,
        dump_response: WidgetTreeDumpResponse,
    ) {
        let Some(pending_request) = self
            .studio_remote_widget_tree_dump_requests
            .remove(&dump_response.request_id)
        else {
            return;
        };

        if pending_request.build_id != build_id {
            self.send_studio_remote_error(
                pending_request.web_socket_id,
                format!(
                    "widget tree dump request {} expected build {}, got {}",
                    dump_response.request_id, pending_request.build_id.0, build_id.0
                ),
            );
            return;
        }

        let dump = dump_response.dump;
        self.studio_remote_latest_widget_dumps
            .insert(build_id, dump.clone());
        if pending_request.emit_dump {
            self.send_studio_remote_response(
                pending_request.web_socket_id,
                StudioRemoteResponse::WidgetTreeDump {
                    build_id: pending_request.build_id.0,
                    request_id: dump_response.request_id,
                    dump: dump.clone(),
                },
            );
        }
        if let Some(query) = pending_request.startup_query {
            let rects = Self::query_widget_dump_rects(&dump, &query);
            self.send_studio_remote_response(
                pending_request.web_socket_id,
                StudioRemoteResponse::WidgetQuery {
                    build_id: pending_request.build_id.0,
                    query,
                    rects,
                },
            );
        }
    }

    fn studio_remote_now() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|v| v.as_secs_f64())
            .unwrap_or(0.0)
    }

    fn send_studio_remote_to_app(
        &mut self,
        cx: &mut Cx,
        web_socket_id: u64,
        build_id: LiveId,
        msg: StudioToApp,
        auto_dump: bool,
    ) -> Result<(), String> {
        let msg_label = Self::studio_remote_control_log_label(&msg);
        let sent = if let Ok(sockets) = self.active_build_websockets.lock() {
            sockets.borrow_mut().send_studio_to_app(build_id, msg)
        } else {
            false
        };
        if !sent {
            return Err(format!(
                "build {} has no active studio websocket connection",
                build_id.0
            ));
        }
        self.log_studio_remote_bridge_event(
            cx,
            format!("studio_remote -> child build={} {}", build_id.0, msg_label),
        );
        if auto_dump {
            self.request_studio_remote_widget_tree_dump(cx, web_socket_id, build_id, None, true)?;
        }
        Ok(())
    }

    fn handle_studio_remote_request(
        &mut self,
        cx: &mut Cx,
        web_socket_id: u64,
        request: StudioRemoteRequest,
    ) {
        match request {
            StudioRemoteRequest::ListBuilds => {
                let mut build_ids: BTreeSet<LiveId> = BTreeSet::new();
                build_ids.extend(self.running_processes.keys().copied());
                build_ids.extend(self.active.builds.keys().copied());
                let builds: Vec<StudioRemoteBuildInfo> = build_ids
                    .into_iter()
                    .map(|build_id| {
                        let process = self
                            .running_processes
                            .get(&build_id)
                            .or_else(|| self.active.builds.get(&build_id).map(|b| &b.process));
                        let (root, package) = if let Some(process) = process {
                            (process.root.clone(), process.binary.clone())
                        } else {
                            ("".to_string(), "".to_string())
                        };
                        let has_web_socket = if let Ok(sockets) = self.active_build_websockets.lock()
                        {
                            sockets.borrow().sockets.iter().any(|s| s.build_id == build_id)
                        } else {
                            false
                        };
                        StudioRemoteBuildInfo {
                            build_id: build_id.0,
                            root,
                            package,
                            active: self.active.builds.contains_key(&build_id),
                            has_web_socket,
                        }
                    })
                    .collect();
                self.send_studio_remote_response(web_socket_id, StudioRemoteResponse::Builds { builds });
            }
            StudioRemoteRequest::CargoRun {
                args,
                root,
                startup_query,
                env,
            } => match self.start_studio_remote_cargo_run(web_socket_id, args, root, startup_query, env) {
                Ok((build_id, root, package)) => {
                    self.send_studio_remote_response(
                        web_socket_id,
                        StudioRemoteResponse::Started {
                            build_id: build_id.0,
                            root,
                            package,
                        },
                    );
                }
                Err(message) => {
                    self.send_studio_remote_response(
                        web_socket_id,
                        StudioRemoteResponse::Error { message },
                    );
                }
            },
            StudioRemoteRequest::Stop { build_id } => {
                let build_id = LiveId(build_id);
                let removed_running = self.running_processes.remove(&build_id).is_some();
                let removed_active = self.active.builds.remove(&build_id).is_some();
                self.remove_profile_build(build_id);
                self.studio_remote_build_owners.remove(&build_id);
                self.studio_remote_latest_widget_dumps.remove(&build_id);
                self.studio_remote_startup_queries.remove(&build_id);
                self.studio_remote_startup_dump_pending.remove(&build_id);
                self.clear_studio_remote_screenshots_for_build(build_id);
                self.clear_studio_remote_widget_tree_dumps_for_build(build_id);
                self.clients[0].send_cmd_with_id(build_id, BuildCmd::Stop);
                if removed_active {
                    cx.action(AppAction::DestroyRunViews {
                        run_view_id: build_id,
                    });
                }
                if removed_running || removed_active {
                    self.send_studio_remote_response(
                        web_socket_id,
                        StudioRemoteResponse::Stopped {
                            build_id: build_id.0,
                        },
                    );
                } else {
                    self.send_studio_remote_response(
                        web_socket_id,
                        StudioRemoteResponse::Error {
                            message: format!("unknown build id {}", build_id.0),
                        },
                    );
                }
            }
            StudioRemoteRequest::StudioToApp { build_id, msg } => {
                let build_id = LiveId(build_id);
                let should_auto_dump = !matches!(
                    msg,
                    StudioToApp::MouseMove(_) | StudioToApp::TweakRay(_) | StudioToApp::Scroll(_)
                );
                if let Err(message) = self.send_studio_remote_to_app(
                    cx,
                    web_socket_id,
                    build_id,
                    msg,
                    should_auto_dump,
                ) {
                    self.send_studio_remote_error(web_socket_id, format!("build {}: {message}", build_id.0));
                }
            }
            StudioRemoteRequest::TypeText {
                build_id,
                text,
                replace_last,
                was_paste,
                auto_dump,
            } => {
                let build_id = LiveId(build_id);
                let msg = StudioToApp::TextInput(TextInputEvent {
                    input: text,
                    replace_last: replace_last.unwrap_or(false),
                    was_paste: was_paste.unwrap_or(false),
                    ..Default::default()
                });
                if let Err(message) = self.send_studio_remote_to_app(
                    cx,
                    web_socket_id,
                    build_id,
                    msg,
                    auto_dump.unwrap_or(false),
                ) {
                    self.send_studio_remote_error(web_socket_id, format!("build {}: {message}", build_id.0));
                }
            }
            StudioRemoteRequest::Return {
                build_id,
                auto_dump,
            } => {
                let build_id = LiveId(build_id);
                let now = Self::studio_remote_now();
                let modifiers = KeyModifiers::default();
                let auto_dump = auto_dump.unwrap_or(false);
                let msgs = [
                    (
                        StudioToApp::KeyDown(KeyEvent {
                            key_code: KeyCode::ReturnKey,
                            is_repeat: false,
                            modifiers,
                            time: now,
                        }),
                        false,
                    ),
                    (
                        StudioToApp::KeyUp(KeyEvent {
                            key_code: KeyCode::ReturnKey,
                            is_repeat: false,
                            modifiers,
                            time: now + 0.01,
                        }),
                        auto_dump,
                    ),
                ];
                for (msg, auto_dump) in msgs {
                    if let Err(message) =
                        self.send_studio_remote_to_app(cx, web_socket_id, build_id, msg, auto_dump)
                    {
                        self.send_studio_remote_error(web_socket_id, format!("build {}: {message}", build_id.0));
                        break;
                    }
                }
            }
            StudioRemoteRequest::Click {
                build_id,
                x,
                y,
                button,
                auto_dump,
            } => {
                let build_id = LiveId(build_id);
                cx.action(BuildManagerAction::AiClickViz {
                    build_id,
                    x: x as f64,
                    y: y as f64,
                    phase: AiClickVizPhase::Down,
                });
                let button_raw_bits = button.unwrap_or(1);
                let auto_dump = auto_dump.unwrap_or(false);
                let now = Self::studio_remote_now();
                let modifiers = RemoteKeyModifiers::default();
                let msgs = [
                    (
                        StudioToApp::MouseMove(RemoteMouseMove {
                            time: now,
                            x: x as f64,
                            y: y as f64,
                            modifiers,
                        }),
                        false,
                    ),
                    (
                        StudioToApp::MouseDown(RemoteMouseDown {
                            button_raw_bits,
                            x: x as f64,
                            y: y as f64,
                            time: now,
                            modifiers,
                        }),
                        false,
                    ),
                    (
                        StudioToApp::MouseUp(RemoteMouseUp {
                            time: now + 0.01,
                            button_raw_bits,
                            x: x as f64,
                            y: y as f64,
                            modifiers,
                        }),
                        auto_dump,
                    ),
                ];
                for (msg, auto_dump) in msgs {
                    if let Err(message) =
                        self.send_studio_remote_to_app(cx, web_socket_id, build_id, msg, auto_dump)
                    {
                        self.send_studio_remote_error(web_socket_id, format!("build {}: {message}", build_id.0));
                        break;
                    }
                }
                cx.action(BuildManagerAction::AiClickViz {
                    build_id,
                    x: x as f64,
                    y: y as f64,
                    phase: AiClickVizPhase::Up,
                });
            }
            StudioRemoteRequest::Screenshot { build_id, kind_id } => {
                let build_id = LiveId(build_id);
                if let Err(message) =
                    self.request_studio_remote_screenshot(cx, web_socket_id, build_id, kind_id.unwrap_or(0))
                {
                    self.send_studio_remote_error(web_socket_id, message);
                }
            }
            StudioRemoteRequest::WidgetTreeDump { build_id } => {
                let build_id = LiveId(build_id);
                if let Err(message) =
                    self.request_studio_remote_widget_tree_dump(cx, web_socket_id, build_id, None, true)
                {
                    self.send_studio_remote_error(web_socket_id, message);
                }
            }
            StudioRemoteRequest::WidgetQuery { build_id, query } => {
                let build_id = LiveId(build_id);
                let Some(dump) = self.studio_remote_latest_widget_dumps.get(&build_id) else {
                    self.send_studio_remote_error(
                        web_socket_id,
                        format!(
                            "build {} has no cached widget tree yet; wait for startup dump",
                            build_id.0
                        ),
                    );
                    return;
                };
                let rects = Self::query_widget_dump_rects(dump, &query);
                self.send_studio_remote_response(
                    web_socket_id,
                    StudioRemoteResponse::WidgetQuery {
                        build_id: build_id.0,
                        query,
                        rects,
                    },
                );
            }
        }
    }

    fn start_studio_remote_cargo_run(
        &mut self,
        web_socket_id: u64,
        args: Vec<String>,
        root: Option<String>,
        startup_query: Option<String>,
        env: Option<HashMap<String, String>>,
    ) -> Result<(LiveId, String, String), String> {
        let root = if let Some(root) = root {
            root
        } else {
            self.default_root_name()
                .ok_or_else(|| "studio has no configured roots".to_string())?
        };

        self.roots
            .find_root(&root)
            .map_err(|_| format!("unknown root '{root}'"))?;

        let cargo_args = normalize_studio_remote_cargo_run_args(args)?;
        let run_env = normalize_studio_remote_env_map(env)?;
        let build_id = self.alloc_studio_remote_build_id();
        let package = parse_studio_remote_package_name(&cargo_args)
            .unwrap_or_else(|| format!("cargo-run-{}", build_id.0));
        let target = if cargo_run_is_release(&cargo_args) {
            BuildTarget::ReleaseStudio
        } else {
            BuildTarget::DebugStudio
        };
        let process = BuildProcess {
            root: root.clone(),
            binary: package.clone(),
            target,
        };

        self.clients[0].send_cmd_with_id(
            build_id,
            BuildCmd::RunCargo(process.clone(), cargo_args, self.studio_addr(), run_env),
        );
        self.running_processes.insert(build_id, process);
        self.studio_remote_build_owners.insert(build_id, web_socket_id);
        self.studio_remote_startup_dump_pending.insert(build_id);
        if let Some(query) = startup_query.map(|q| q.trim().to_string()) {
            if !query.is_empty() {
                self.studio_remote_startup_queries.insert(build_id, query);
            }
        }
        Ok((build_id, root, package))
    }

    fn handle_studio_remote_socket(&mut self, cx: &mut Cx) {
        while let Ok(msg) = self.recv_studio_remote_msg.try_recv() {
            match msg {
                StudioRemoteSocket::Connected {
                    web_socket_id,
                    sender,
                } => {
                    self.studio_remote_sockets.insert(web_socket_id, sender);
                }
                StudioRemoteSocket::Disconnected { web_socket_id } => {
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
                StudioRemoteSocket::Request {
                    web_socket_id,
                    request,
                } => {
                    self.handle_studio_remote_request(cx, web_socket_id, request);
                }
            }
        }
    }
}
