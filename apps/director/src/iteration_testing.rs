// Included by iteration_worker.rs. AI input is restricted to its own hidden,
// retained-artifact child; requests advance on the existing worker reactor.

const TEST_JSON_LIMIT: u64 = 1024 * 1024;
const TEST_PNG_LIMIT: u64 = 64 * 1024 * 1024;
const TEST_MAX_OPERATIONS: usize = 128;
const TEST_MAX_CAPTURES: usize = 16;
const TEST_DEMO_MAX_CHARS: usize = 120;

fn validate_test_demo(title: &str) -> Result<(), String> {
    if title.trim().is_empty() || title.chars().count() > TEST_DEMO_MAX_CHARS || title.chars().any(char::is_control) {
        return Err("demo must be a nonempty title of at most 120 characters without control characters".into());
    }
    Ok(())
}

// Display metadata only. Its exact observed identities must match even when
// loaded after restart, independently of the bounded operation-report cache.
fn test_demo_title(root: &Path, flow: &str, run: &str, artifact: &str, commit: &str) -> Result<Option<String>, String> {
    let path = root.join("runs").join(run).join("demo.json");
    recording_owned_path(root, &path)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(err(error)),
        Ok(_) => {}
    }
    let value = json::parse(&recording_bytes(root, &path, 4096)?).map_err(str::to_owned)?;
    if value.get("version").and_then(Value::as_u64) != Some(1)
        || [("kind", "studio_demo"), ("flow", flow), ("run", run), ("artifact", artifact), ("commit", commit)]
            .iter().any(|(field, expected)| value.get(field).and_then(Value::as_str) != Some(*expected)) {
        return Err("Demonstration metadata does not match its owned flow/run/checkpoint".into());
    }
    let title = value.get("title").and_then(Value::as_str).ok_or("Missing demonstration title")?;
    validate_test_demo(title)?;
    Ok(Some(title.into()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct TestInput {
    route: &'static str,
    params: Vec<(String, String)>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum TestRequest {
    Start { artifact_id: String, demo: Option<String> },
    Input { run_id: String, input: TestInput },
    Snapshot { run_id: String, window: Option<u32>, query: String },
    Stop { run_id: String },
    Status { run_id: String, operation_id: Option<String> },
}

pub fn parse_test_request(args: &Value) -> Result<TestRequest, String> {
    let Value::Obj(fields) = args else { return Err("Test arguments must be an object".into()); };
    let text = |key: &str, max: usize| -> Result<String, String> {
        let value = args.get(key).and_then(Value::as_str).ok_or_else(|| format!("{key} must be a string"))?;
        if value.is_empty() || value.len() > max || value.contains('\0') { return Err(format!("Invalid {key}")); }
        Ok(value.into())
    };
    let id = |key| -> Result<String, String> {
        let value = text(key, 256)?;
        if !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) { return Err(format!("Invalid {key}")); }
        Ok(value)
    };
    id("flow")?;
    let action = text("action", 16)?;
    let allowed: &[&str] = match action.as_str() {
        "start" => &["flow", "action", "artifact_id", "demo"],
        "input" => &["flow", "action", "run_id", "input", "window", "x", "y", "button", "dx", "dy", "key", "text", "shift", "ctrl", "alt", "cmd"],
        "snapshot" => &["flow", "action", "run_id", "window", "query"],
        "stop" => &["flow", "action", "run_id"],
        "status" => &["flow", "action", "run_id", "operation_id"],
        _ => return Err("Test action must be start, input, snapshot, stop or status".into()),
    };
    if fields.iter().any(|(key, _)| !allowed.contains(&key.as_str())) { return Err("Unknown test argument".into()); }
    let window = args.get("window").map(|value| value.as_u64().filter(|value| *value <= u32::MAX as u64).map(|value| value as u32).ok_or("window must be a nonnegative 32-bit integer")).transpose()?;
    Ok(match action.as_str() {
        "start" => TestRequest::Start { artifact_id: id("artifact_id")?, demo: args.get("demo").map(|value| {
            let title = value.as_str().ok_or("demo must be a string")?;
            validate_test_demo(title)?;
            Ok::<_, String>(title.to_owned())
        }).transpose()? },
        "snapshot" => TestRequest::Snapshot { run_id: id("run_id")?, window, query: match args.get("query") { None => String::new(), Some(_) => text("query", 256)? } },
        "stop" => TestRequest::Stop { run_id: id("run_id")? },
        "status" => TestRequest::Status { run_id: id("run_id")?, operation_id: args.get("operation_id").map(|_| id("operation_id")).transpose()? },
        "input" => {
            let kind = text("input", 16)?;
            let extra: &[&str] = match kind.as_str() {
                "click" | "move" | "down" | "up" => &["x", "y", "button", "shift", "ctrl", "alt", "cmd"],
                "scroll" => &["x", "y", "dx", "dy", "shift", "ctrl", "alt", "cmd"],
                "key_press" | "key_down" | "key_up" => &["key", "shift", "ctrl", "alt", "cmd"],
                "text" => &["text"],
                _ => return Err("Unknown test input; use click/move/down/up/scroll/key_press/key_down/key_up/text".into()),
            };
            if fields.iter().any(|(key, _)| !["flow", "action", "run_id", "input", "window"].contains(&key.as_str()) && !extra.contains(&key.as_str())) { return Err("Argument does not apply to this input kind".into()); }
            let mut params = vec![("wait".into(), "1".into())];
            if let Some(window) = window { params.push(("w".into(), window.to_string())); }
            for modifier in ["shift", "ctrl", "alt", "cmd"] {
                if let Some(value) = args.get(modifier) {
                    let value = value.as_bool().ok_or("Modifiers must be booleans")?;
                    params.push((modifier.into(), if value { "1" } else { "0" }.into()));
                }
            }
            let coordinate = |key: &str, signed: bool, required: bool| -> Result<Option<String>, String> {
                let value = match args.get(key) {
                    Some(Value::Int(value)) => *value as f64,
                    Some(Value::F64(value)) => *value,
                    None if !required => return Ok(None),
                    _ => return Err(format!("{key} must be a finite number")),
                };
                if !value.is_finite() || value.abs() > 131072.0 || (!signed && value < 0.0) { return Err(format!("{key} exceeds window-coordinate bounds")); }
                Ok(Some(value.to_string()))
            };
            let route = if kind == "text" {
                params.push(("t".into(), text("text", 4096)?));
                "t"
            } else if let Some(kind) = kind.strip_prefix("key_") {
                let key = text("key", 32)?;
                if !key.bytes().all(|byte| byte.is_ascii_alphanumeric()) { return Err("Key must be a Makepad key name such as KeyA or ReturnKey".into()); }
                params.push(("k".into(), kind.into())); params.push(("c".into(), key));
                "k"
            } else {
                params.push(("k".into(), kind.clone()));
                for key in ["x", "y"] { params.push((key.into(), coordinate(key, false, true)?.unwrap())); }
                if let Some(button) = args.get("button") {
                    let button = button.as_u64().filter(|button| *button <= 2).ok_or("button must be 0, 1 or 2")?;
                    params.push(("b".into(), button.to_string()));
                }
                for key in ["dx", "dy"] { if let Some(value) = coordinate(key, true, false)? { params.push((key.into(), value)); } }
                if kind == "scroll" && args.get("dx").is_none() && args.get("dy").is_none() { return Err("Scroll requires dx or dy".into()); }
                "m"
            };
            TestRequest::Input { run_id: id("run_id")?, input: TestInput { route, params } }
        },
        _ => unreachable!(),
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TestPhase { Ready, Input, Inspect, Frame, Stop }
struct TestOperation {
    id: String,
    run: String,
    artifact: String,
    directory: PathBuf,
    phase: TestPhase,
    started: Instant,
    phase_started: Instant,
    child: Option<Child>,
    input: Option<TestInput>,
    window: Option<u32>,
    query: String,
    request: Value,
    inspected: Option<String>,
}
impl TestOperation {
    fn response(&self) -> PathBuf { self.directory.join(if self.phase == TestPhase::Frame { "frame.pending.png" } else { "response.json" }) }
    fn stderr(&self) -> PathBuf { self.directory.join("request.stderr") }
}

impl Host {
    fn test_demo(&self, flow: &str, run_id: &str) -> Result<Option<String>, String> {
        let state = self.flow(flow)?;
        let run = state.runs.iter().find(|run| run.id == run_id && run.role == iteration::RunRole::AiTest).ok_or("Unknown AI test run for demonstration metadata")?;
        let artifact = state.artifacts.iter().find(|artifact| artifact.id == run.artifact_id).ok_or("Unknown demonstration artifact")?;
        test_demo_title(&self.directory, flow, run_id, &artifact.id, &artifact.commit)
    }

    fn test_request(&mut self, flow: &str, action: TestRequest) -> Result<Value, String> {
        if let TestRequest::Status { run_id, operation_id } = &action {
            let run = self.flow(flow)?.runs.iter().find(|run| run.id == *run_id && run.role == iteration::RunRole::AiTest).ok_or("Unknown AI test run for this flow")?;
            if let Some(operation) = operation_id {
                let report = self.reports.get(&format!("test:{operation}")).filter(|report| report.get("flow").and_then(Value::as_str) == Some(flow) && report.get("run_id").and_then(Value::as_str) == Some(run_id));
                if let Some(report) = report { return Ok(report.clone()); }
                let path = self.directory.join("runs").join(run_id).join("testing").join(operation).join("operation.json");
                let report = json::parse(&recording_bytes(&self.directory, &path, 128 * 1024)?).map_err(str::to_owned)?;
                if report.get("flow").and_then(Value::as_str) != Some(flow) || report.get("run_id").and_then(Value::as_str) != Some(run_id) { return Err("Test operation does not belong to this run".into()); }
                return Ok(report);
            }
            return Ok(json::obj(vec![("flow", s(flow)), ("run_id", s(run_id)), ("role", s("ai_test")), ("artifact_id", s(&run.artifact_id)),
                ("demo", self.test_demo(flow, run_id)?.as_deref().map(s).unwrap_or(Value::Null)),
                ("closed", Value::Bool(run.closed)), ("owned", Value::Bool(self.apps.get(flow).is_some_and(|app| app.run == *run_id))),
                ("remote_ready", Value::Bool(self.apps.get(flow).is_some_and(|app| app.run == *run_id && app.port.is_some()))),
                ("operation_id", self.test_operations.get(flow).map(|op| s(&op.id)).unwrap_or(Value::Null)),
                ("operations", Value::Arr(self.reports.values().filter(|report| report.get("kind").and_then(Value::as_str) == Some("ai_test") && report.get("run_id").and_then(Value::as_str) == Some(run_id)).cloned().collect())),
                ("video_directory", s(self.directory.join("runs").join(run_id).join("video").to_string_lossy())),
                ("test_result", s("Inspect actual input responses, captures and recordings; no pass is inferred"))]));
        }
        self.require_active_lane(flow)?;
        if self.test_operations.contains_key(flow) && !matches!(action, TestRequest::Stop { .. }) { return Err("An AI test operation is still pending; poll its status before the next action".into()); }
        let demo = match &action { TestRequest::Start { demo, .. } => demo.clone(), _ => None };
        let (run_id, phase, input, window, query, request) = match action {
            TestRequest::Start { artifact_id, demo } => {
                if let Some(title) = &demo { validate_test_demo(title)?; }
                let state = self.flow(flow)?;
                if state.runs.iter().any(|run| !run.closed) || self.apps.contains_key(flow) { return Err("Close the existing app before starting an AI test; Studio will not close it for the agent".into()); }
                if state.runs.iter().rev().find(|run| run.role == iteration::RunRole::Human).is_some_and(|run| !run.human_requested) { return Err("The previous human app needs an explicit human close before testing".into()); }
                if self.builds.contains_key(flow) || self.embedding_pending.values().any(|pending| pending.flow == flow)
                    || self.effects.iter().any(|effect| effect_flow(effect) == flow)
                    || state.job.as_ref().is_some_and(|job| matches!(job.phase, iteration::BuildPhase::WaitingForClose | iteration::BuildPhase::CheckpointRequested | iteration::BuildPhase::Ready | iteration::BuildPhase::Building)) {
                    return Err("Finish or cancel the queued build/launch before starting an AI test".into());
                }
                let artifact = state.artifacts.iter().find(|artifact| artifact.id == artifact_id).ok_or("Unknown retained artifact")?.clone();
                self.spawn_artifact_role(flow, &artifact_id, &artifact.path, iteration::LaunchMode::Standalone, false, None, iteration::RunRole::AiTest)?;
                let run = self.apps.get(flow).ok_or("Test child ownership was not retained")?.run.clone();
                let mut request = vec![("action", s("start")), ("artifact_id", s(artifact_id))];
                if let Some(title) = demo { request.push(("demo", s(title))); }
                (run, TestPhase::Ready, None, None, String::new(), json::obj(request))
            },
            TestRequest::Input { run_id, input } => {
                let request = json::obj(vec![("action", s("input")), ("route", s(input.route)), ("parameters", Value::Obj(input.params.iter().map(|(key, value)| (key.clone(), s(value))).collect()))]);
                (run_id, TestPhase::Input, Some(input), None, String::new(), request)
            },
            TestRequest::Snapshot { run_id, window, query } => {
                if self.flow(flow)?.captures.iter().filter(|capture| capture.run_id == run_id).count() >= TEST_MAX_CAPTURES { return Err("This test run has retained 16 captures; inspect its video or begin a new test run".into()); }
                let request = json::obj(vec![("action", s("snapshot")), ("window", window.map(|value| Value::Int(value.into())).unwrap_or(Value::Null)), ("query", s(&query))]);
                (run_id, TestPhase::Inspect, None, window, query, request)
            },
            TestRequest::Stop { run_id } => {
                self.test_owned_app(flow, &run_id, false)?;
                self.cancel_test_operation(flow, "Canceled by explicit test stop");
                self.close(flow)?;
                (run_id, TestPhase::Stop, None, None, String::new(), json::obj(vec![("action", s("stop"))]))
            },
            TestRequest::Status { .. } => unreachable!(),
        };
        let result = (|| {
            let run = self.test_owned_app(flow, &run_id, phase != TestPhase::Stop)?;
            let artifact = run.artifact.clone();
            let root = run.directory.join("testing");
            if let Some(title) = &demo {
                let state = self.flow(flow)?;
                let checkpoint = state.artifacts.iter().find(|item| item.id == artifact).ok_or("Unknown demonstration artifact")?;
                let path = self.directory.join("runs").join(&run_id).join("demo.json");
                recording_owned_path(&self.directory, &path)?;
                if path.try_exists().map_err(err)? { return Err("Demonstration metadata already exists for this run".into()); }
                let metadata = json::obj(vec![("kind", s("studio_demo")), ("version", Value::Int(1)), ("flow", s(flow)),
                    ("run", s(&run_id)), ("artifact", s(&artifact)), ("commit", s(&checkpoint.commit)), ("title", s(title))]);
                atomic_write(&path, metadata.to_json().as_bytes())?;
            }
            fs::create_dir_all(&root).map_err(err)?;
            if fs::read_dir(&root).map_err(err)?.take(TEST_MAX_OPERATIONS + 1).count() >= TEST_MAX_OPERATIONS { return Err("This test run reached its 128-operation history bound".into()); }
            let (id, _) = fresh_run_identity();
            let id = format!("test-{id}");
            let directory = root.join(&id);
            fs::create_dir(&directory).map_err(err)?;
            let operation = TestOperation { id, run: run_id.clone(), artifact, directory, phase, started: Instant::now(), phase_started: Instant::now(), child: None, input, window, query, request, inspected: None };
            let report = self.publish_test(flow, &operation, "pending", None, None)?;
            self.test_operations.insert(flow.into(), operation);
            Ok(report)
        })();
        if result.is_err() && phase == TestPhase::Ready { let _ = self.close(flow); }
        result
    }

    fn test_owned_app(&mut self, flow: &str, run_id: &str, require_running: bool) -> Result<&mut AppRun, String> {
        let run = self.apps.get_mut(flow).ok_or("No app process owned by this host for the test")?;
        if run.run != run_id || run.role != iteration::RunRole::AiTest { return Err("AI input is restricted to the exact owned AI test run; human apps cannot be controlled".into()); }
        if require_running && (run.closing.is_some() || run.exit.is_some() || run.child.try_wait().map_err(err)?.is_some()) { return Err("The owned AI test is closing or has exited".into()); }
        Ok(run)
    }

    fn publish_test(&mut self, flow: &str, operation: &TestOperation, phase: &str, result: Option<Value>, error: Option<&str>) -> Result<Value, String> {
        let report = json::obj(vec![("kind", s("ai_test")), ("flow", s(flow)), ("run_id", s(&operation.run)), ("artifact_id", s(&operation.artifact)),
            ("demo", self.test_demo(flow, &operation.run)?.as_deref().map(s).unwrap_or(Value::Null)),
            ("operation_id", s(&operation.id)), ("phase", s(phase)), ("complete", Value::Bool(matches!(phase, "completed" | "failed" | "canceled"))),
            ("request", operation.request.clone()), ("result", result.unwrap_or(Value::Null)), ("error", error.map(s).unwrap_or(Value::Null)),
            ("elapsed_ms", Value::Int(operation.started.elapsed().as_millis().min(i64::MAX as u128) as i64)),
            ("evidence_directory", s(operation.directory.to_string_lossy())), ("test_result", s("Not inferred from command completion"))]);
        if let Err(error) = atomic_write(&operation.directory.join("operation.json"), report.to_json().as_bytes()) {
            self.storage_error = Some(format!("AI test evidence could not be saved: {error}"));
            self.changed = true;
            return Err(error);
        }
        self.reports.insert(format!("test:{}", operation.id), report.clone());
        let old: Vec<_> = self.reports.iter().filter(|(key, value)| key.starts_with("test:") && value.get("complete") == Some(&Value::Bool(true))).map(|(key, _)| key.clone()).collect();
        for key in old.iter().take(old.len().saturating_sub(64)) { self.reports.remove(key); }
        self.changed = true;
        Ok(report)
    }

    fn cancel_test_operation(&mut self, flow: &str, reason: &str) {
        if let Some(mut operation) = self.test_operations.remove(flow) {
            if let Some(child) = &mut operation.child { let _ = child.kill(); let _ = child.wait(); }
            if let Err(error) = self.publish_test(flow, &operation, "canceled", None, Some(reason)) { self.note = format!("Test cancellation evidence: {error}"); self.changed = true; }
        }
    }

    fn poll_testing(&mut self) {
        let flows: Vec<_> = self.test_operations.keys().cloned().collect();
        for flow in flows {
            let Some(mut operation) = self.test_operations.remove(&flow) else { continue; };
            match self.advance_test(&flow, &mut operation) {
                Ok(Some(result)) => {
                    if let Err(error) = self.publish_test(&flow, &operation, "completed", Some(result), None) { self.note = format!("Test completion evidence: {error}"); self.changed = true; }
                },
                Ok(None) => { self.test_operations.insert(flow, operation); },
                Err(error) => {
                    if let Some(child) = &mut operation.child { let _ = child.kill(); let _ = child.wait(); }
                    let _ = self.publish_test(&flow, &operation, "failed", None, Some(&error));
                    if operation.phase == TestPhase::Ready { let _ = self.close(&flow); }
                    self.note = format!("{flow}: AI test operation failed: {error}"); self.changed = true;
                },
            }
        }
    }

    fn advance_test(&mut self, flow: &str, operation: &mut TestOperation) -> Result<Option<Value>, String> {
        if operation.phase == TestPhase::Stop {
            if self.apps.get(flow).is_some_and(|app| app.run == operation.run) { return Ok(None); }
            let run = self.flow(flow)?.runs.iter().find(|run| run.id == operation.run && run.role == iteration::RunRole::AiTest).ok_or("Test ownership history disappeared")?;
            if !run.closed { return Err("Test process exited without a durable closure observation".into()); }
            return Ok(Some(json::obj(vec![("closed", Value::Bool(true)), ("human_requested", Value::Bool(false)), ("exit_code", run.exit_code.map(|code| Value::Int(code.into())).unwrap_or(Value::Null))])));
        }
        if let Some(error) = &self.storage_error { return Err(format!("Test paused because durable evidence is unavailable: {error}")); }
        self.require_active_lane(flow)?;
        let run = self.test_owned_app(flow, &operation.run, true)?;
        let pid = run.child.id();
        let port = match run.port {
            Some(port) => port,
            None if operation.phase == TestPhase::Ready && operation.started.elapsed() < Duration::from_secs(15) => return Ok(None),
            None => return Err("The owned test has not published its matching-PID remote endpoint".into()),
        };
        let limit = if operation.phase == TestPhase::Frame { TEST_PNG_LIMIT } else { TEST_JSON_LIMIT };
        let observed_files = [(operation.response(), limit), (operation.stderr(), 64 * 1024)];
        if let Some(child) = &mut operation.child {
            for (path, limit) in observed_files {
                if let Ok(metadata) = fs::symlink_metadata(path) { if !metadata.file_type().is_file() || metadata.len() > limit { return Err("Test endpoint output exceeded its bounded regular-file limit".into()); } }
            }
            if operation.phase_started.elapsed() > Duration::from_secs(20) { return Err("Owned test endpoint timed out".into()); }
            let Some(status) = child.try_wait().map_err(err)? else { return Ok(None); };
            operation.child = None;
            if !status.success() { return Err(format!("Owned test endpoint failed: {}", log_tail(&operation.stderr(), 4096).unwrap_or_else(|_| status.to_string()))); }
            if operation.phase == TestPhase::Frame {
                let source = operation.response();
                let bytes = recording_bytes(&self.directory, &source, TEST_PNG_LIMIT as usize)?;
                if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") { return Err("Owned test snapshot returned no PNG".into()); }
                let (width, height) = makepad_widgets::image_cache::image_size_by_data(&bytes, &source).map_err(err)?;
                if width == 0 || height == 0 || width.saturating_mul(height) > 16 * 1024 * 1024 { return Err("Test PNG exceeds 16 megapixels".into()); }
                let _ = makepad_widgets::image_cache::decode_image_from_data(&bytes).map_err(err)?;
                let path = operation.directory.join("frame.png");
                fs::rename(&source, &path).map_err(err)?;
                let capture_id = format!("capture-{}", operation.id);
                self.observe(Observation::Captured { flow: flow.into(), capture: iteration::Capture { id: capture_id.clone(), artifact_id: operation.artifact.clone(), run_id: operation.run.clone(), path: path.clone(), width: width as u32, height: height as u32, timestamp_ms: now() } })?;
                return Ok(Some(json::obj(vec![("capture_id", s(capture_id)), ("png", s(path.to_string_lossy())), ("width", Value::Int(width as i64)), ("height", Value::Int(height as i64)),
                    ("widget_snapshot", test_response_excerpt(operation.inspected.as_deref().unwrap_or(""))), ("widget_snapshot_path", s(operation.directory.join("widgets.json").to_string_lossy()))])));
            }
            let response = String::from_utf8(recording_bytes(&self.directory, &operation.response(), TEST_JSON_LIMIT as usize)?).map_err(err)?;
            let parsed = json::parse(response.as_bytes()).map_err(str::to_owned)?;
            if let Some(error) = parsed.get("err") { return Err(format!("Owned test endpoint: {}", error.to_json())); }
            match operation.phase {
                TestPhase::Ready => {
                    if parsed.get("pid").and_then(Value::as_u64) != Some(pid.into()) { return Err("Owned test endpoint PID does not match its process".into()); }
                    return Ok(Some(json::obj(vec![("remote_ready", Value::Bool(true)), ("status", test_response_excerpt(&response))])));
                },
                TestPhase::Input => return Ok(Some(json::obj(vec![("response", test_response_excerpt(&response)), ("frame_waited", Value::Bool(true))]))),
                TestPhase::Inspect => {
                    fs::rename(operation.response(), operation.directory.join("widgets.json")).map_err(err)?;
                    operation.inspected = Some(response); operation.phase = TestPhase::Frame;
                },
                _ => unreachable!(),
            }
        }
        let (route, mut params) = match operation.phase {
            TestPhase::Ready => ("s", vec![]),
            TestPhase::Input => { let input = operation.input.as_ref().ok_or("Missing typed test input")?; (input.route, input.params.clone()) },
            TestPhase::Inspect => ("snap", if operation.query.is_empty() { vec![] } else { vec![("q".into(), operation.query.clone())] }),
            TestPhase::Frame => ("g", vec![("raw".into(), "1".into())]),
            TestPhase::Stop => unreachable!(),
        };
        if matches!(operation.phase, TestPhase::Inspect | TestPhase::Frame) { if let Some(window) = operation.window { params.push(("w".into(), window.to_string())); } }
        let error_output = OpenOptions::new().write(true).create(true).truncate(true).open(operation.stderr()).map_err(err)?;
        let mut command = Command::new("curl");
        // Disable user curl configuration, proxies and redirects. Route and
        // address are derived exclusively from our live owned AppRun.
        command.args(["-q", "--noproxy", "*", "--proto", "=http", "--silent", "--show-error", "--fail", "--connect-timeout", "1", "--max-time", "16", "--max-filesize"])
            .arg(if operation.phase == TestPhase::Frame { TEST_PNG_LIMIT } else { TEST_JSON_LIMIT }.to_string())
            .args(["--get", "--output"]).arg(operation.response())
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(error_output);
        for (key, value) in params { command.arg("--data-urlencode").arg(format!("{key}={value}")); }
        command.arg(format!("http://127.0.0.1:{port}/{route}"));
        operation.child = Some(command.spawn().map_err(err)?);
        operation.phase_started = Instant::now();
        Ok(None)
    }
}

fn test_response_excerpt(response: &str) -> Value {
    let mut end = response.len().min(32 * 1024);
    while !response.is_char_boundary(end) { end -= 1; }
    json::obj(vec![("json", s(&response[..end])), ("truncated", Value::Bool(end != response.len()))])
}

fn retained_binary_hash(path: &Path) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path).map_err(err)?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > 1024 * 1024 * 1024 { return Err("Retained executable must be a bounded regular file".into()); }
    git::content_hash(path).map_err(|error| format!("Cannot hash the retained executable: {error}"))
}
