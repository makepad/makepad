// Included by iteration_worker.rs. Loopback HTTP is another producer of the
// same durable per-flow CLI envelopes; it never executes an alternate command.

const FLOW_HTTP_CLIENTS: usize = 16;
const FLOW_HTTP_HEADER: usize = 8192;
const FLOW_HTTP_BODY: usize = 16 * 1024;
const FLOW_HTTP_OUTPUT_BUDGET: usize = 16 * 1024 * 1024;
const FLOW_HTTP_DEADLINE: Duration = Duration::from_secs(10);

struct FlowHttpCapability {
    flow: String,
    control: PathBuf,
    token: String,
    url: String,
}
struct FlowHttpPeer {
    stream: std::net::TcpStream,
    input: Vec<u8>,
    output: Vec<u8>,
    sent: usize,
    started: Instant,
    responding: bool,
    waiting: Option<FlowHttpFeedbackWait>,
}
struct FlowHttpFeedbackWait {
    flow: String,
    id: String,
    reply: PathBuf,
    started: Instant,
}
struct FlowHttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}
struct IterationHttp {
    listener: std::net::TcpListener,
    port: u16,
    peers: Vec<FlowHttpPeer>,
    capabilities: BTreeMap<String, FlowHttpCapability>,
    next_sync: Instant,
    next_peer: usize,
}

impl IterationHttp {
    fn start() -> Result<Self, String> {
        let listener =
            std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(err)?;
        listener.set_nonblocking(true).map_err(err)?;
        let port = listener.local_addr().map_err(err)?.port();
        Ok(Self {
            listener,
            port,
            peers: Vec::new(),
            capabilities: BTreeMap::new(),
            next_sync: Instant::now(),
            next_peer: 0,
        })
    }

    fn sync_capabilities(&mut self, host: &Host) -> Result<(), String> {
        self.capabilities.retain(|flow, capability| {
            if host.engine.flows.contains_key(flow) {
                return true;
            }
            flow_http_remove_discovery(capability);
            false
        });
        for flow in host.engine.flows.keys().take(iteration::MAX_STORED_FLOWS) {
            if self.capabilities.contains_key(flow) {
                continue;
            }
            let control = cli_control_dir(&host.directory, flow)?;
            let token = flow_http_token()?;
            if self
                .capabilities
                .values()
                .any(|existing| existing.token == token)
            {
                return Err("OS entropy returned a duplicate lane capability".into());
            }
            let url = format!("http://127.0.0.1:{}/v1/{token}", self.port);
            flow_http_publish_discovery(&control, &url)?;
            self.capabilities.insert(
                flow.clone(),
                FlowHttpCapability {
                    flow: flow.clone(),
                    control,
                    token,
                    url,
                },
            );
        }
        // The terminal belongs to its durable origin, while callbacks belong
        // to that origin's latest successor. Display mirrors never enter this
        // worker-owned map and cannot take over its capability.
        let directory = host.directory.canonicalize().map_err(err)?;
        if directory.file_name().and_then(|name| name.to_str()) != Some("iterations") {
            return Err("Lane session bindings require the Studio iterations directory".into());
        }
        let sessions = directory
            .parent()
            .ok_or("Iterations has no Studio state parent")?
            .join("agent_sessions");
        let mut bindings = BTreeMap::new();
        for flow in host
            .engine
            .flows
            .values()
            .filter(|flow| flow.successor.is_none())
            .take(iteration::MAX_STORED_FLOWS)
        {
            let session = cli_screen_session_id(host.engine.terminal_origin(&flow.id)?)?;
            let capability = self
                .capabilities
                .get(&flow.id)
                .ok_or("Current lane has no HTTP capability")?;
            let text = json::obj(vec![
                ("version", Value::Int(1)),
                ("session_id", s(&session)),
                ("flow", s(&flow.id)),
                (
                    "control_dir",
                    s(capability
                        .control
                        .to_str()
                        .ok_or("Lane control directory is not UTF-8")?),
                ),
            ])
            .to_json();
            if bindings.insert(session, text).is_some() {
                return Err("More than one lane claims the same persistent terminal origin".into());
            }
        }
        if !bindings.is_empty() {
            cli_private_dir(&sessions, true)?;
            for (session, text) in bindings {
                cli_publish_lane_binding(&sessions.join(format!("{session}.lane.json")), &text)?;
            }
        }
        Ok(())
    }

    fn poll(&mut self, host: &mut Host) {
        if Instant::now() >= self.next_sync {
            self.next_sync = Instant::now() + Duration::from_secs(1);
            if host.storage_error.is_none() {
                if let Err(error) = self.sync_capabilities(host) {
                    let note = format!("Lane HTTP discovery unavailable: {error}");
                    if host.note != note {
                        host.note = note;
                        host.changed = true;
                    }
                }
            }
        }
        for _ in 0..4 {
            let (stream, remote) = match self.listener.accept() {
                Ok(peer) => peer,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    host.note = format!("Lane HTTP accept failed: {error}");
                    host.changed = true;
                    break;
                }
            };
            if !remote.ip().is_loopback() || self.peers.len() >= FLOW_HTTP_CLIENTS {
                continue;
            }
            if stream.set_nonblocking(true).is_err() {
                continue;
            }
            let _ = stream.set_nodelay(true);
            self.peers.push(FlowHttpPeer {
                stream,
                input: Vec::new(),
                output: Vec::new(),
                sent: 0,
                started: Instant::now(),
                responding: false,
                waiting: None,
            });
        }
        let mut peers = std::mem::take(&mut self.peers);
        if !peers.is_empty() {
            let rotation = self.next_peer % peers.len();
            peers.rotate_left(rotation);
            self.next_peer = self.next_peer.wrapping_add(1);
        }
        let mut output_bytes: usize = peers.iter().map(|peer| peer.output.len()).sum();
        let mut admissions = 4usize;
        for mut peer in peers {
            if peer.started.elapsed() > FLOW_HTTP_DEADLINE {
                continue;
            }
            let mut alive = true;
            if let Some(wait) = &peer.waiting {
                let result = flow_http_feedback_reply(host, &wait.flow, &wait.id, &wait.reply);
                let response = match result {
                    Ok(Some(response)) => Some(response),
                    Err(error) => Some((500, flow_http_feedback_error(host, &wait.flow, &error))),
                    Ok(None) if wait.started.elapsed() >= Duration::from_secs(5) => {
                        Some((202, flow_http_feedback_pending(&wait.id)))
                    }
                    Ok(None) => None,
                };
                if let Some((status, body)) = response {
                    peer.output = flow_http_response(status, body);
                    output_bytes = output_bytes.saturating_add(peer.output.len());
                    peer.responding = true;
                    peer.waiting = None;
                }
            }
            if !peer.responding && peer.waiting.is_none() && admissions > 0 {
                let mut input = [0u8; 8192];
                for _ in 0..4 {
                    match peer.stream.read(&mut input) {
                        Ok(0) => {
                            alive = false;
                            break;
                        }
                        Ok(count) => {
                            peer.input.extend_from_slice(&input[..count]);
                            if peer.input.len() > FLOW_HTTP_HEADER + FLOW_HTTP_BODY {
                                break;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => {
                            alive = false;
                            break;
                        }
                    }
                }
                match flow_http_parse(&peer.input, self.port) {
                    Ok(Some(request)) => {
                        admissions -= 1;
                        let (status, body) = if output_bytes
                            > FLOW_HTTP_OUTPUT_BUDGET.saturating_sub(CLI_REPLY_LIMIT + 8192)
                        {
                            (503, flow_http_error("HTTP response capacity is busy; retain the request ID and retry later"))
                        } else {
                            self.route(host, request, &mut peer.waiting)
                        };
                        if peer.waiting.is_none() {
                            peer.output = flow_http_response(status, body);
                            peer.responding = true;
                        } else {
                            peer.input = Vec::new();
                            alive = true;
                        }
                    }
                    Ok(None) => {}
                    Err((status, message)) => {
                        peer.output = flow_http_response(status, flow_http_error(message));
                        peer.responding = true;
                    }
                }
                if peer.responding {
                    peer.input = Vec::new();
                    output_bytes = output_bytes.saturating_add(peer.output.len());
                    // A client may shut down its write half after one request.
                    alive = true;
                }
            }
            if alive && peer.responding {
                for _ in 0..4 {
                    let end = (peer.sent + 64 * 1024).min(peer.output.len());
                    match peer.stream.write(&peer.output[peer.sent..end]) {
                        Ok(0) => {
                            alive = false;
                            break;
                        }
                        Ok(count) => {
                            peer.sent += count;
                            if peer.sent == peer.output.len() {
                                alive = false;
                                break;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => {
                            alive = false;
                            break;
                        }
                    }
                }
            }
            if alive {
                self.peers.push(peer);
            } else {
                output_bytes = output_bytes.saturating_sub(peer.output.len());
            }
        }
    }

    fn route(
        &self,
        host: &Host,
        request: FlowHttpRequest,
        waiting: &mut Option<FlowHttpFeedbackWait>,
    ) -> (u16, String) {
        let parts: Vec<_> = request.path.split('/').collect();
        if parts.len() < 4 || parts[0] != "" || parts[1] != "v1" || parts[2].len() != 64 {
            return (404, flow_http_error("Unknown lane endpoint"));
        }
        let Some(capability) = self.capabilities.values().find(|capability| {
            flow_http_same_token(capability.token.as_bytes(), parts[2].as_bytes())
        }) else {
            return (404, flow_http_error("Unknown lane endpoint"));
        };
        if !host.engine.flows.contains_key(&capability.flow) {
            return (404, flow_http_error("Unknown lane"));
        }
        let resolved = match host.engine.resolve_active_flow(&capability.flow) {
            Ok(flow) => flow,
            Err(error) => return (409, flow_http_error(&error)),
        };
        let outcome: Result<(u16, String), (u16, String)> = (|| {
            match (request.method.as_str(), parts.as_slice()) {
                ("GET", [_, _, _, "tools" | "manifest"]) => Ok((200, flow_http_manifest(resolved)?)),
                ("GET", [_, _, _, "state"]) => Ok((200, flow_http_state(host, resolved)?)),
                ("GET", [_, _, _, "brief"]) => Ok((200, flow_http_brief(host, resolved)?)),
                ("POST", [_, _, _, "feedback"]) => {
                    if host.storage_error.is_some() { return Err((503, "History is unavailable; feedback was not accepted".into())); }
                    let body = json::parse_depth(&request.body, 20).map_err(|error| (400, error.to_owned()))?;
                    let Value::Obj(mut args) = body else { return Err((400, "Feedback must be {i,v,q?,k?,u}".into())); };
                    let id = args.iter().find(|(key, _)| key == "i").and_then(|(_, value)| value.as_str()).filter(|id| cli_identifier(id)).ok_or((400, "Feedback requires a unique i".into()))?.to_owned();
                    if args.iter().any(|(key, _)| !["i", "v", "q", "k", "u"].contains(&key.as_str())) { return Err((400, "Feedback accepts only i,v,q,k,u; lane identity comes from its capability".into())); }
                    args.retain(|(key, _)| key != "i");
                    let call = cli_service_call(&id, &capability.flow, "report", Value::Obj(args)).map_err(|error| (400, error))?;
                    let reply = match cli_submit(&capability.control, &capability.flow, &call) {
                        Ok(reply) => reply,
                        Err(error) => return Ok((409, flow_http_feedback_error(host, &capability.flow, &error))),
                    };
                    match flow_http_feedback_reply(host, &capability.flow, &id, &reply).map_err(|error| (500, error))? {
                        Some(response) => Ok(response),
                        None => {
                            *waiting = Some(FlowHttpFeedbackWait { flow: capability.flow.clone(), id: id.clone(), reply, started: Instant::now() });
                            Ok((202, flow_http_feedback_pending(&id)))
                        }
                    }
                },
                ("GET", [_, _, _, "feedback", id]) if cli_identifier(id) => {
                    if !flow_http_feedback_known(&capability.control, &capability.flow, id).map_err(|error| (500, error))? { return Err((404, "Unknown feedback callback ID for this lane".into())); }
                    let reply = capability.control.join("replies").join(format!("{id}.json"));
                    Ok(flow_http_feedback_reply(host, &capability.flow, id, &reply).map_err(|error| (500, error))?.unwrap_or_else(|| (202, flow_http_feedback_pending(id))))
                },
                ("GET", [_, _, _, "events", after]) => {
                    let after = after.parse::<u64>().map_err(|_| (400, "Event cursor must be an unsigned sequence number".into()))?;
                    Ok((200, flow_http_events(host, resolved, after)))
                },
                ("POST", [_, _, _, "call"]) => {
                    if host.storage_error.is_some() { return Err((503, "History is unavailable; no request was accepted".into())); }
                    let envelope = json::parse_depth(&request.body, 20).map_err(|error| (400, error.to_owned()))?;
                    let Value::Obj(fields) = &envelope else { return Err((400, "Call must be an object with id, tool and args".into())); };
                    if fields.len() != 3 || fields.iter().any(|(key, _)| !["id", "tool", "args"].contains(&key.as_str())) { return Err((400, "Call accepts exactly id, tool and args".into())); }
                    let id = envelope.get("id").and_then(Value::as_str).filter(|id| cli_identifier(id)).ok_or((400, "Invalid request id".into()))?;
                    let tool = envelope.get("tool").and_then(Value::as_str).ok_or((400, "tool must be a string".into()))?;
                    let args = envelope.get("args").cloned().ok_or((400, "args must be an object".into()))?;
                    let call = cli_service_call(id, &capability.flow, tool, args).map_err(|error| (400, error))?;
                    let reply = cli_submit(&capability.control, &capability.flow, &call).map_err(|error| (409, error))?;
                    let result = cli_poll_reply(&reply, &capability.flow, id).map_err(|error| (500, error))?;
                    Ok(match result { Some(result) => (200, result.to_json()), None => (202, flow_http_pending(&capability.flow, id)) })
                }
                ("GET", [_, _, _, "result", id]) if cli_identifier(id) => {
                    cli_validate_control(&capability.control, &capability.flow).map_err(|error| (500, error))?;
                    let reply = capability.control.join("replies").join(format!("{id}.json"));
                    if let Some(result) = cli_poll_reply(&reply, &capability.flow, id).map_err(|error| (500, error))? { return Ok((200, result.to_json())); }
                    let known = ["requests", "processing", "receipts"].iter().any(|directory| capability.control.join(directory).join(format!("{id}.json")).is_file());
                    if !known { return Err((404, "Unknown request ID for this lane".into())); }
                    if host.storage_error.is_some() { return Err((503, "History is unavailable; the recorded request remains pending, do not replace its ID".into())); }
                    Ok((202, flow_http_pending(&capability.flow, id)))
                }
                ("GET" | "POST", _) => Err((404, "Unknown lane route".into())),
                _ => Err((405, "Use GET brief/state/tools/events/sequence, POST feedback/call or GET feedback/id/result/id".into())),
            }
        })();
        match outcome {
            Ok(response) => response,
            Err((status, error)) => (status, flow_http_error(&error)),
        }
    }
}

impl Drop for IterationHttp {
    fn drop(&mut self) {
        for capability in self.capabilities.values() {
            flow_http_remove_discovery(capability);
        }
    }
}

fn flow_http_same_token(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

fn flow_http_parse(
    bytes: &[u8],
    port: u16,
) -> Result<Option<FlowHttpRequest>, (u16, &'static str)> {
    if bytes.len() > FLOW_HTTP_HEADER + FLOW_HTTP_BODY {
        return Err((413, "HTTP request exceeds its size limit"));
    }
    let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
        if bytes.len() > FLOW_HTTP_HEADER {
            return Err((431, "HTTP headers exceed 8 KiB"));
        }
        return Ok(None);
    };
    if end + 4 > FLOW_HTTP_HEADER {
        return Err((431, "HTTP headers exceed 8 KiB"));
    }
    let header =
        std::str::from_utf8(&bytes[..end]).map_err(|_| (400, "HTTP headers must be ASCII"))?;
    if !header.is_ascii() {
        return Err((400, "HTTP headers must be ASCII"));
    }
    let mut lines = header.split("\r\n");
    let line = lines.next().ok_or((400, "Missing HTTP request line"))?;
    let words: Vec<_> = line.split(' ').collect();
    if words.len() != 3
        || !matches!(words[2], "HTTP/1.1")
        || words[1].len() > 512
        || !words[0].bytes().all(|byte| byte.is_ascii_uppercase())
        || words[0].is_empty()
        || !words[1].starts_with('/')
        || !words[1]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_'))
    {
        return Err((400, "Expected HTTP/1.1 with an exact lane path"));
    }
    let mut headers = BTreeMap::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or((400, "Malformed HTTP header"))?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || value
                .bytes()
                .any(|byte| byte.is_ascii_control() && byte != b'\t')
        {
            return Err((400, "Malformed HTTP header"));
        }
        let name = name.to_ascii_lowercase();
        if headers.insert(name.clone(), value.trim()).is_some() {
            return Err((400, "Duplicate HTTP headers are refused"));
        }
        if name == "origin"
            || name == "referer"
            || name.starts_with("sec-fetch-")
            || name.starts_with("access-control-")
        {
            return Err((
                403,
                "Browser-origin requests are not accepted by the lane tool endpoint",
            ));
        }
        if matches!(name.as_str(), "transfer-encoding" | "expect" | "upgrade") {
            return Err((
                400,
                "Chunking, upgrades and HTTP expectations are unsupported",
            ));
        }
    }
    if headers.get("host").copied() != Some(format!("127.0.0.1:{port}").as_str()) {
        return Err((403, "Host must match the owned loopback listener"));
    }
    let body_len = match headers.get("content-length") {
        Some(value) if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) => {
            value
                .parse::<usize>()
                .map_err(|_| (413, "HTTP body exceeds its size limit"))?
        }
        Some(_) => return Err((400, "Invalid Content-Length")),
        None if words[0] == "POST" => return Err((411, "POST requires Content-Length")),
        None => 0,
    };
    if body_len > FLOW_HTTP_BODY {
        return Err((413, "HTTP JSON body exceeds 16 KiB"));
    }
    if words[0] == "POST"
        && !headers.get("content-type").is_some_and(|value| {
            value.eq_ignore_ascii_case("application/json")
                || value.eq_ignore_ascii_case("application/json; charset=utf-8")
        })
    {
        return Err((415, "POST requires application/json"));
    }
    if words[0] != "POST" && body_len != 0 {
        return Err((400, "Only POST call accepts a body"));
    }
    let total = end + 4 + body_len;
    if bytes.len() < total {
        return Ok(None);
    }
    if bytes.len() != total {
        return Err((400, "HTTP pipelining and trailing bytes are unsupported"));
    }
    Ok(Some(FlowHttpRequest {
        method: words[0].into(),
        path: words[1].into(),
        body: bytes[end + 4..].to_vec(),
    }))
}

fn flow_http_error(message: &str) -> String {
    let bounded: String = message.chars().take(1024).collect();
    json::obj(vec![("status", s("refused")), ("error", s(bounded))]).to_json()
}
fn flow_http_pending(flow: &str, id: &str) -> String {
    json::obj(vec![("schema_version", Value::Int(1)), ("flow_id", s(flow)), ("request_id", s(id)), ("status", s("pending")),
        ("result_path", s(format!("result/{id}"))), ("note", s("Queued intent only. Poll this ID; tool admission does not prove build, input, login or test completion."))]).to_json()
}
fn flow_http_response(mut status: u16, mut body: String) -> Vec<u8> {
    if body.len() > CLI_REPLY_LIMIT {
        status = 500;
        body = flow_http_error("Result exceeds the HTTP response bound; inspect the same durable request through the CLI");
    }
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    let header = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\n\r\n", body.len());
    let mut bytes = header.into_bytes();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}
fn flow_http_manifest(flow: &str) -> Result<String, (u16, String)> {
    let tools = crate::iteration_tools::tool_defs()
        .into_iter()
        .chain(std::iter::once(flow_split_tool_def()))
        .filter(|tool| cli_tool_name(&tool.name).is_ok())
        .map(|tool| {
            let mut schema = json::parse_depth(tool.parameters.as_bytes(), 20)
                .map_err(|error| (500, error.to_owned()))?;
            if let Value::Obj(fields) = &mut schema {
                for (key, value) in fields {
                    match (key.as_str(), value) {
                        ("properties", Value::Obj(properties)) => {
                            properties.retain(|(name, _)| !matches!(name.as_str(), "flow" | "f"))
                        }
                        ("required", Value::Arr(required)) => {
                            required.retain(|name| !matches!(name.as_str(), Some("flow" | "f")))
                        }
                        _ => {}
                    }
                }
            }
            Ok(json::obj(vec![
                ("tool", s(&tool.name)),
                ("description", s(&tool.description)),
                ("args", schema),
            ]))
        })
        .collect::<Result<Vec<_>, (u16, String)>>()?;
    Ok(json::obj(vec![("schema_version", Value::Int(1)), ("flow_id", s(flow)), ("tools", Value::Arr(tools)),
        ("call", s("POST call with application/json {id,tool,args}. Omit flow/f; the capability supplies it. Use a unique 1-96 character alphanumeric, hyphen or underscore ID for each distinct call.")),
        ("result", s("GET result/<id>. 202 is pending; 200 returns the durable CLI reply. An identical repeated POST reuses its ID; changed arguments reject. Uncertain results must be inspected, never blindly repeated.")),
        ("asynchronous_work", s("A successful tool reply may only admit work. flow_test status observes its operation; flow_inspect observes builds and feedback. The human must close the evaluation app before checks or compilation.")),
        ("feedback", s("GET brief on startup/resume. POST feedback {i:unique_id,v:todo_revision,q?:request_scope,k?:existing_requirement_id,u:[[todo_id,state,text?],...]}. Success is exactly {v,r}. Omit q for status-only deltas; k amends q. Stale v rejects both scope and todos. 202 {i,pending:true} means poll GET feedback/i. Same i/body reuses its original result.")),
        ("state", s("GET state reads this lane's requirements, todos, builds, artifacts, runs, captures, operation reports and recording paths/errors. GET events/0 pages durable activity; continue with next_after while more is true.")),
        ("scope", s("Only the listed per-lane tools are callable. Root lifecycle, account recovery, agent terminal controls and human close stay on Studio's UI/app service; this endpoint cannot forge those observations.")),
        ("discovery", s("Read control/http-url again after Studio restarts. Treat the URL as a local secret and do not share or log it."))]).to_json())
}

fn flow_http_feedback_pending(id: &str) -> String {
    json::obj(vec![("i", s(id)), ("pending", Value::Bool(true))]).to_json()
}
fn flow_http_feedback_error(host: &Host, flow: &str, error: &str) -> String {
    let flow = host.engine.resolve_active_flow(flow).unwrap_or(flow);
    let mut fields = vec![("error", s(error.chars().take(384).collect::<String>()))];
    if let Ok(state) = host.flow(flow) {
        fields.push(("v", Value::Int(state.todos_revision as i64)));
        fields.push(("r", Value::Int(state.requirements_revision as i64)));
    }
    json::obj(fields).to_json()
}
fn flow_http_feedback_known(control: &Path, flow: &str, id: &str) -> Result<bool, String> {
    cli_validate_control(control, flow)?;
    for directory in ["receipts", "processing", "requests"] {
        let path = control.join(directory).join(format!("{id}.json"));
        if !path.try_exists().map_err(err)? {
            continue;
        }
        let saved = json::parse_depth(cli_read(&path, CLI_REQUEST_LIMIT * 2)?.as_bytes(), 24)
            .map_err(str::to_owned)?;
        let envelope = saved.get("request").unwrap_or(&saved);
        return Ok(
            envelope.get("request_id").and_then(Value::as_str) == Some(id)
                && envelope.get("flow_id").and_then(Value::as_str) == Some(flow)
                && envelope.get("tool").and_then(Value::as_str) == Some("flow_report"),
        );
    }
    Ok(false)
}
fn flow_http_feedback_reply(
    host: &Host,
    flow: &str,
    id: &str,
    path: &Path,
) -> Result<Option<(u16, String)>, String> {
    let Some(reply) = cli_poll_reply(path, flow, id)? else {
        return Ok(None);
    };
    if reply.get("status").and_then(Value::as_str) == Some("ok") {
        let result = reply.get("result").ok_or("Feedback reply has no result")?;
        let v = result
            .get("v")
            .and_then(Value::as_u64)
            .ok_or("Feedback reply has no todo revision")?;
        let r = result
            .get("r")
            .and_then(Value::as_u64)
            .ok_or("Feedback reply has no request revision")?;
        return Ok(Some((
            200,
            json::obj(vec![
                ("v", Value::Int(v as i64)),
                ("r", Value::Int(r as i64)),
            ])
            .to_json(),
        )));
    }
    let error = reply.get("error").and_then(Value::as_str).unwrap_or(
        "Feedback request has no confirmed result; inspect brief before deciding the next action",
    );
    if reply.get("status").and_then(Value::as_str) == Some("uncertain") {
        return Ok(Some((
            409,
            json::obj(vec![
                ("i", s(id)),
                ("uncertain", Value::Bool(true)),
                ("error", s(error.chars().take(384).collect::<String>())),
            ])
            .to_json(),
        )));
    }
    Ok(Some((409, flow_http_feedback_error(host, flow, error))))
}
fn flow_http_brief(host: &Host, flow_id: &str) -> Result<String, (u16, String)> {
    let flow = host.flow(flow_id).map_err(|error| (404, error))?;
    let mut q = Vec::new();
    let mut omitted = Vec::new();
    let mut bytes = 0;
    for requirement in &flow.requirements {
        if bytes + requirement.text.len() > 8192 {
            omitted.push(s(&requirement.id));
            continue;
        }
        bytes += requirement.text.len();
        q.push(Value::Arr(vec![s(&requirement.id), s(&requirement.text)]));
    }
    let gate = if host.lifecycle_pending.contains_key(flow_id) {
        "stopping"
    } else if flow.lifecycle != iteration::FlowLifecycle::Active {
        flow.lifecycle.as_str()
    } else if flow
        .runs
        .iter()
        .any(|run| run.role == iteration::RunRole::AiTest && !run.closed)
    {
        "stop_ai_test_before_build"
    } else if flow
        .runs
        .iter()
        .rev()
        .find(|run| run.role == iteration::RunRole::Human)
        .is_some_and(|run| !run.closed || !run.human_requested)
    {
        "wait_for_human_close"
    } else if let Some(job) = &flow.job {
        job.phase.as_str()
    } else {
        "no_build"
    };
    let mut fields = vec![
        ("context", s(&flow.config.delegation_context)),
        ("v", Value::Int(flow.todos_revision as i64)),
        ("r", Value::Int(flow.requirements_revision as i64)),
        (
            "u",
            Value::Arr(
                flow.todos
                    .iter()
                    .filter(|todo| todo.state != iteration::TodoState::Implemented)
                    .map(|todo| {
                        Value::Arr(vec![s(&todo.id), s(todo.state.as_str()), s(&todo.text)])
                    })
                    .collect(),
            ),
        ),
        (
            "d",
            Value::Arr(
                flow.todos
                    .iter()
                    .filter(|todo| todo.state == iteration::TodoState::Implemented)
                    .map(|todo| s(&todo.id))
                    .collect(),
            ),
        ),
        ("q", Value::Arr(q)),
        ("gate", s(gate)),
    ];
    if let Some(previous) = &flow.predecessor {
        fields.push(("flow", s(flow_id)));
        fields.push(("from", s(previous)));
    }
    if !omitted.is_empty() {
        fields.push(("q_omitted", Value::Arr(omitted)));
        fields.push(("more", s("state")));
    }
    if let Some(error) = &host.storage_error {
        fields.push(("error", s(error)));
    }
    Ok(json::obj(fields).to_json())
}

fn flow_http_state(host: &Host, flow_id: &str) -> Result<String, (u16, String)> {
    let flow = host.flow(flow_id).map_err(|error| (404, error))?;
    let path = |path: &Path| s(path.to_string_lossy());
    let number = |value: u64| Value::Int(value.min(i64::MAX as u64) as i64);
    let captures = flow
        .captures
        .iter()
        .map(|capture| {
            json::obj(vec![
                ("id", s(&capture.id)),
                ("artifact_id", s(&capture.artifact_id)),
                ("run_id", s(&capture.run_id)),
                ("path", path(&capture.path)),
                ("width", number(capture.width.into())),
                ("height", number(capture.height.into())),
                ("timestamp_ms", number(capture.timestamp_ms)),
            ])
        })
        .collect();
    let runs = flow
        .runs
        .iter()
        .map(|run| {
            json::obj(vec![
                ("id", s(&run.id)),
                ("artifact_id", s(&run.artifact_id)),
                ("role", s(run.role.as_str())),
                ("mode", s(run.mode.as_str())),
                (
                    "pid",
                    run.pid.map(|pid| number(pid.into())).unwrap_or(Value::Null),
                ),
                ("closed", Value::Bool(run.closed)),
                ("observation_lost", Value::Bool(run.observation_lost)),
                ("human_requested", Value::Bool(run.human_requested)),
                (
                    "exit_code",
                    run.exit_code
                        .map(|code| Value::Int(code.into()))
                        .unwrap_or(Value::Null),
                ),
                (
                    "owned_now",
                    Value::Bool(
                        host.apps
                            .get(flow_id)
                            .is_some_and(|owned| owned.run == run.id),
                    ),
                ),
            ])
        })
        .collect();
    let artifacts = flow
        .artifacts
        .iter()
        .map(|artifact| {
            json::obj(vec![
                ("id", s(&artifact.id)),
                (
                    "origin_flow",
                    host.engine
                        .evidence_origin("artifact", &artifact.id)
                        .map(s)
                        .unwrap_or(Value::Null),
                ),
                (
                    "history_owner",
                    s(host
                        .engine
                        .history_owner(flow_id, &format!("artifact/{}", artifact.id))),
                ),
                ("job_id", s(&artifact.job_id)),
                ("commit", s(&artifact.commit)),
                ("path", path(&artifact.path)),
                ("source_revision", number(artifact.source_revision)),
                (
                    "requirements_revision",
                    number(artifact.requirements_revision),
                ),
                ("mode", s(artifact.mode.as_str())),
            ])
        })
        .collect();
    let reports: Vec<_> = host
        .reports
        .keys()
        .cloned()
        .zip(host.projected_reports())
        .filter(|(_, report)| report.get("flow").and_then(Value::as_str) == Some(flow_id))
        .collect();
    let mut bytes = 0usize;
    let mut operations = Vec::new();
    for (key, report) in reports.iter().rev().take(128) {
        let size = report.to_json().len();
        if bytes + size > 2 * 1024 * 1024 {
            continue;
        }
        bytes += size;
        operations.push(json::obj(vec![
            ("id", s(key)),
            ("report", (*report).clone()),
        ]));
    }
    let operations_omitted = reports.len().saturating_sub(operations.len());
    let mut bytes = 0usize;
    let mut feedback = Vec::new();
    for record in flow.feedback.iter().rev().filter(|record| {
        host.engine
            .history_visible(flow_id, &format!("feedback/{}", record.id))
    }) {
        let value = json::obj(vec![
            ("id", s(&record.id)),
            ("from_human", Value::Bool(record.from_human)),
            ("artifact_id", s(&record.feedback.artifact_id)),
            ("run_id", s(&record.feedback.run_id)),
            ("category", s(&record.feedback.category)),
            ("summary", s(&record.feedback.summary)),
            (
                "evidence",
                Value::Arr(
                    record
                        .feedback
                        .evidence
                        .iter()
                        .map(|evidence| {
                            json::obj(vec![
                                ("capture_id", s(&evidence.capture_id)),
                                (
                                    "region",
                                    evidence
                                        .region
                                        .as_ref()
                                        .map(|region| {
                                            json::obj(vec![
                                                ("x", Value::F64(region.x)),
                                                ("y", Value::F64(region.y)),
                                                ("width", Value::F64(region.width)),
                                                ("height", Value::F64(region.height)),
                                            ])
                                        })
                                        .unwrap_or(Value::Null),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]);
        let size = value.to_json().len();
        if bytes + size > 1024 * 1024 {
            break;
        }
        bytes += size;
        feedback.push(value);
    }
    let feedback_omitted = flow.feedback.len().saturating_sub(feedback.len());
    Ok(json::obj(vec![("schema_version", Value::Int(1)), ("flow_id", s(flow_id)), ("observed_at_ms", number(now())), ("revision", number(host.engine.revision)),
        ("flow", host.engine.inspect(flow_id).map_err(|error| (500, error))?),
        ("requirements", Value::Arr(flow.requirements.iter().map(|item| json::obj(vec![("id", s(&item.id)), ("text", s(&item.text)), ("revision", number(item.revision))])).collect())),
        ("todos", Value::Arr(flow.todos.iter().map(|todo| json::obj(vec![("id", s(&todo.id)), ("state", s(todo.state.as_str())), ("text", s(&todo.text)), ("revision", number(todo.revision)), ("source_revision", number(todo.source_revision)), ("requirements_revision", number(todo.requirements_revision))])).collect())),
        ("artifacts", Value::Arr(artifacts)), ("runs", Value::Arr(runs)), ("captures", Value::Arr(captures)),
        ("feedback", Value::Arr(feedback)), ("feedback_omitted", number(feedback_omitted as u64)),
        ("operations", Value::Arr(operations)), ("operations_omitted", number(operations_omitted as u64)),
        ("recordings", Value::Arr(host.projected_recordings().iter().filter(|tile| tile.flow == flow_id).map(RecordingTile::json).collect())),
        ("attachments", Value::Arr(host.projected_attachments().iter().filter(|attachment| attachment.flow == flow_id).map(Attachment::json).collect())),
        ("build_owned_now", Value::Bool(host.builds.contains_key(flow_id))), ("lifecycle_transition_pending", Value::Bool(host.lifecycle_pending.contains_key(flow_id))),
        ("storage_error", host.storage_error.as_deref().map(s).unwrap_or(Value::Null)),
        ("coverage", s("Worker-observed lane state only. Recording metadata is scanned about once per second with bounded historical rotation. Full durable activity is paged at events/0. Omitted operation evidence remains in local reports/result IDs. Agent terminal output and global UI state belong to Studio's app service. No test pass or human acceptance is inferred."))]).to_json())
}
fn flow_http_events(host: &Host, flow: &str, after: u64) -> String {
    let mut events = Vec::new();
    let mut bytes = 0usize;
    let mut next = after;
    let mut more = false;
    for event in host
        .engine
        .events(flow)
        .filter(|event| event.sequence > after)
    {
        let value = event.json();
        let size = value.to_json().len();
        if events.len() >= 32 || (!events.is_empty() && bytes + size > 1024 * 1024) {
            more = true;
            break;
        }
        bytes += size;
        next = event.sequence;
        events.push(value);
    }
    json::obj(vec![
        ("flow_id", s(flow)),
        ("events", Value::Arr(events)),
        ("next_after", Value::Int(next.min(i64::MAX as u64) as i64)),
        ("more", Value::Bool(more)),
    ])
    .to_json()
}

fn flow_http_token() -> Result<String, String> {
    #[cfg(any(unix, windows))]
    let mut bytes = [0u8; 32];
    #[cfg(unix)]
    {
        File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut bytes))
            .map_err(|_| "OS randomness is unavailable for lane HTTP capabilities")?;
    }
    #[cfg(windows)]
    {
        #[link(name = "bcrypt")]
        unsafe extern "system" {
            fn BCryptGenRandom(
                algorithm: *mut std::ffi::c_void,
                buffer: *mut u8,
                count: u32,
                flags: u32,
            ) -> i32;
        }
        if unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                2,
            )
        } != 0
        {
            return Err("OS randomness is unavailable for lane HTTP capabilities".into());
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        return Err("Lane HTTP requires a native OS random source".into());
    }
    #[cfg(any(unix, windows))]
    {
        Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
    }
}
#[cfg(windows)]
fn flow_http_publish_discovery(control: &Path, url: &str) -> Result<(), String> {
    cli_private_dir(control, false)?;
    if url.len() > 255 {
        return Err("Lane HTTP discovery exceeds its bound".into());
    }
    makepad_screen::protocol::write_private(
        &control.join("http-url"),
        format!("{url}\n").as_bytes(),
    )
}

#[cfg(not(windows))]
fn flow_http_publish_discovery(control: &Path, url: &str) -> Result<(), String> {
    cli_private_dir(control, false)?;
    let target = control.join("http-url");
    match fs::symlink_metadata(&target) {
        Ok(meta) => {
            if !meta.file_type().is_file() {
                return Err("Lane HTTP discovery must be a regular private file".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if meta.uid() != fs::metadata(control).map_err(err)?.uid()
                    || meta.mode() & 0o077 != 0
                {
                    return Err(
                        "Lane HTTP discovery must be owned by the lane user with mode 0600".into(),
                    );
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(err(error)),
    }
    let temp = control.join(format!(".http-{}", cli_request_id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(err)?;
    let result = (|| {
        writeln!(file, "{url}").map_err(err)?;
        file.sync_all().map_err(err)?;
        fs::rename(&temp, &target).map_err(err)?;
        #[cfg(unix)]
        File::open(control)
            .and_then(|directory| directory.sync_all())
            .map_err(err)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
fn flow_http_remove_discovery(capability: &FlowHttpCapability) {
    if cli_private_dir(&capability.control, false).is_err() {
        return;
    }
    let path = capability.control.join("http-url");
    if cli_read(&path, 256).is_ok_and(|value| value.trim() == capability.url) {
        let _ = fs::remove_file(path);
    }
}
