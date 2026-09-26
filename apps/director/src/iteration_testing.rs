// Included by iteration_worker.rs. AI input is restricted to its own hidden,
// retained-artifact child; requests advance on the existing worker reactor.

const TEST_JSON_LIMIT: u64 = 1024 * 1024;
const TEST_PNG_LIMIT: u64 = 64 * 1024 * 1024;
const TEST_MAX_OPERATIONS: usize = 128;
const TEST_MAX_CAPTURES: usize = 16;
const TEST_DEMO_MAX_CHARS: usize = 120;

fn validate_test_demo(title: &str) -> Result<(), String> {
    if title.trim().is_empty()
        || title.chars().count() > TEST_DEMO_MAX_CHARS
        || title.chars().any(char::is_control)
    {
        return Err(
            "demo must be a nonempty title of at most 120 characters without control characters"
                .into(),
        );
    }
    Ok(())
}

// Display metadata only. Its exact observed identities must match even when
// loaded after restart, independently of the bounded operation-report cache.
fn test_demo_title(
    root: &Path,
    flow: &str,
    run: &str,
    artifact: &str,
    commit: &str,
) -> Result<Option<String>, String> {
    let path = root.join("runs").join(run).join("demo.json");
    recording_owned_path(root, &path)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(err(error)),
        Ok(_) => {}
    }
    let value = json::parse(&recording_bytes(root, &path, 4096)?).map_err(str::to_owned)?;
    if value.get("version").and_then(Value::as_u64) != Some(1)
        || [
            ("kind", "studio_demo"),
            ("flow", flow),
            ("run", run),
            ("artifact", artifact),
            ("commit", commit),
        ]
        .iter()
        .any(|(field, expected)| value.get(field).and_then(Value::as_str) != Some(*expected))
    {
        return Err("Demonstration metadata does not match its owned flow/run/checkpoint".into());
    }
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .ok_or("Missing demonstration title")?;
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
    /// `mode` None asks for the Director-hosted path (the default); an
    /// explicit standalone request never tries hosting. `grant` names a
    /// parent-artifact grant issued to this lane.
    Start {
        artifact_id: String,
        demo: Option<String>,
        mode: Option<iteration::LaunchMode>,
        grant: Option<String>,
    },
    Input {
        run_id: String,
        input: TestInput,
    },
    Snapshot {
        run_id: String,
        window: Option<u32>,
        query: String,
    },
    Stop {
        run_id: String,
    },
    Status {
        run_id: String,
        operation_id: Option<String>,
    },
}

pub fn parse_test_request(args: &Value) -> Result<TestRequest, String> {
    let Value::Obj(fields) = args else {
        return Err("Test arguments must be an object".into());
    };
    let text = |key: &str, max: usize| -> Result<String, String> {
        let value = args
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{key} must be a string"))?;
        if value.is_empty() || value.len() > max || value.contains('\0') {
            return Err(format!("Invalid {key}"));
        }
        Ok(value.into())
    };
    let id = |key| -> Result<String, String> {
        let value = text(key, 256)?;
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(format!("Invalid {key}"));
        }
        Ok(value)
    };
    id("flow")?;
    let action = text("action", 16)?;
    let allowed: &[&str] = match action.as_str() {
        "start" => &["flow", "action", "artifact_id", "demo", "mode", "grant"],
        "input" => &[
            "flow", "action", "run_id", "input", "window", "x", "y", "button", "dx", "dy", "key",
            "text", "shift", "ctrl", "alt", "cmd",
        ],
        "snapshot" => &["flow", "action", "run_id", "window", "query"],
        "stop" => &["flow", "action", "run_id"],
        "status" => &["flow", "action", "run_id", "operation_id"],
        _ => return Err("Test action must be start, input, snapshot, stop or status".into()),
    };
    if fields
        .iter()
        .any(|(key, _)| !allowed.contains(&key.as_str()))
    {
        return Err("Unknown test argument".into());
    }
    let window = args
        .get("window")
        .map(|value| {
            value
                .as_u64()
                .filter(|value| *value <= u32::MAX as u64)
                .map(|value| value as u32)
                .ok_or("window must be a nonnegative 32-bit integer")
        })
        .transpose()?;
    Ok(match action.as_str() {
        "start" => TestRequest::Start {
            artifact_id: id("artifact_id")?,
            demo: args
                .get("demo")
                .map(|value| {
                    let title = value.as_str().ok_or("demo must be a string")?;
                    validate_test_demo(title)?;
                    Ok::<_, String>(title.to_owned())
                })
                .transpose()?,
            mode: match args
                .get("mode")
                .map(|_| text("mode", 16))
                .transpose()?
                .as_deref()
            {
                None => None,
                Some("embedded") => Some(iteration::LaunchMode::Embedded),
                Some("standalone") => Some(iteration::LaunchMode::Standalone),
                Some(_) => return Err("mode must be embedded or standalone".into()),
            },
            grant: args.get("grant").map(|_| id("grant")).transpose()?,
        },
        "snapshot" => TestRequest::Snapshot {
            run_id: id("run_id")?,
            window,
            query: match args.get("query") {
                None => String::new(),
                Some(_) => text("query", 256)?,
            },
        },
        "stop" => TestRequest::Stop {
            run_id: id("run_id")?,
        },
        "status" => TestRequest::Status {
            run_id: id("run_id")?,
            operation_id: args
                .get("operation_id")
                .map(|_| id("operation_id"))
                .transpose()?,
        },
        "input" => {
            let kind = text("input", 16)?;
            let extra: &[&str] = match kind.as_str() {
                "click" | "move" | "down" | "up" => &["x", "y", "button", "shift", "ctrl", "alt", "cmd"],
                "scroll" => &["x", "y", "dx", "dy", "shift", "ctrl", "alt", "cmd"],
                "key_press" | "key_down" | "key_up" => &["key", "shift", "ctrl", "alt", "cmd"],
                "text" => &["text"],
                _ => return Err("Unknown test input; use click/move/down/up/scroll/key_press/key_down/key_up/text".into()),
            };
            if fields.iter().any(|(key, _)| {
                !["flow", "action", "run_id", "input", "window"].contains(&key.as_str())
                    && !extra.contains(&key.as_str())
            }) {
                return Err("Argument does not apply to this input kind".into());
            }
            let mut params = vec![("wait".into(), "1".into())];
            if let Some(window) = window {
                params.push(("w".into(), window.to_string()));
            }
            for modifier in ["shift", "ctrl", "alt", "cmd"] {
                if let Some(value) = args.get(modifier) {
                    let value = value.as_bool().ok_or("Modifiers must be booleans")?;
                    params.push((modifier.into(), if value { "1" } else { "0" }.into()));
                }
            }
            let coordinate =
                |key: &str, signed: bool, required: bool| -> Result<Option<String>, String> {
                    let value = match args.get(key) {
                        Some(Value::Int(value)) => *value as f64,
                        Some(Value::F64(value)) => *value,
                        None if !required => return Ok(None),
                        _ => return Err(format!("{key} must be a finite number")),
                    };
                    if !value.is_finite() || value.abs() > 131072.0 || (!signed && value < 0.0) {
                        return Err(format!("{key} exceeds window-coordinate bounds"));
                    }
                    Ok(Some(value.to_string()))
                };
            let route = if kind == "text" {
                params.push(("t".into(), text("text", 4096)?));
                "t"
            } else if let Some(kind) = kind.strip_prefix("key_") {
                let key = text("key", 32)?;
                if !key.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
                    return Err("Key must be a Makepad key name such as KeyA or ReturnKey".into());
                }
                params.push(("k".into(), kind.into()));
                params.push(("c".into(), key));
                "k"
            } else {
                params.push(("k".into(), kind.clone()));
                for key in ["x", "y"] {
                    params.push((key.into(), coordinate(key, false, true)?.unwrap()));
                }
                if let Some(button) = args.get("button") {
                    let button = button
                        .as_u64()
                        .filter(|button| *button <= 2)
                        .ok_or("button must be 0, 1 or 2")?;
                    params.push(("b".into(), button.to_string()));
                }
                for key in ["dx", "dy"] {
                    if let Some(value) = coordinate(key, true, false)? {
                        params.push((key.into(), value));
                    }
                }
                if kind == "scroll" && args.get("dx").is_none() && args.get("dy").is_none() {
                    return Err("Scroll requires dx or dy".into());
                }
                "m"
            };
            TestRequest::Input {
                run_id: id("run_id")?,
                input: TestInput { route, params },
            }
        }
        _ => unreachable!(),
    })
}

/// Launching: a hosted run identity exists but its broker registration is
/// still pending, so there is no process yet. Fallback: a hosted attempt that
/// never became ready is being closed, and its exit and unregistration are
/// awaited before a standalone replacement gets a fresh run identity.
/// Surface: the hosted app answered its remote port, which does not prove a
/// connected, rendering surface; the host transport's first frame does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TestPhase {
    Launching,
    Fallback,
    Ready,
    Surface,
    Activity,
    Input,
    Inspect,
    Frame,
    Stop,
}
impl TestPhase {
    /// Until the app answered, presented a frame and the input guard was
    /// read, the test has not begun; a failure here may close the app.
    /// Afterwards nothing reruns.
    fn starting(self) -> bool {
        matches!(
            self,
            Self::Launching | Self::Fallback | Self::Ready | Self::Surface | Self::Activity
        )
    }
    /// What the pending operation is waiting for, as status reports it.
    fn waiting_for(self) -> &'static str {
        match self {
            Self::Launching => "host_registration",
            Self::Fallback => "hosted_attempt_exit",
            Self::Ready => "remote_endpoint",
            Self::Surface => "first_hosted_frame",
            Self::Activity => "input_guard",
            Self::Input => "input_reply",
            Self::Inspect => "widget_snapshot",
            Self::Frame => "frame_capture",
            Self::Stop => "process_exit",
        }
    }
}
struct TestOperation {
    id: String,
    run: String,
    artifact: String,
    /// Empty until the process exists; evidence lives under its run directory.
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
    requested: iteration::LaunchMode,
    /// The hosted attempt this run replaced, with the reason, if any.
    attempt: Option<String>,
    attempt_client: Option<u64>,
    fallback_reason: Option<String>,
    demo: Option<String>,
    grant: Option<String>,
    /// When the hosted surface presented its first frame, from test start.
    first_frame_ms: Option<u64>,
}
impl TestOperation {
    fn response(&self) -> PathBuf {
        self.directory.join(if self.phase == TestPhase::Frame {
            "frame.pending.png"
        } else {
            "response.json"
        })
    }
    fn stderr(&self) -> PathBuf {
        self.directory.join("request.stderr")
    }
    fn headers(&self) -> PathBuf {
        self.directory.join("response.headers")
    }
    fn status(&self) -> PathBuf {
        self.directory.join("response.code")
    }
}

/// The ending input counter every remote response carries.
fn test_user_seq_header(headers: &str) -> Option<u64> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("x-makepad-user-seq")
            .then(|| value.trim().parse().ok())
            .flatten()
    })
}

impl Host {
    /// Commit of the artifact a test run used: the lane's own, or a granted one.
    fn test_artifact_commit(&self, flow: &str, artifact: &str) -> Option<String> {
        self.flow(flow)
            .ok()?
            .artifacts
            .iter()
            .find(|item| item.id == artifact)
            .map(|item| item.commit.clone())
            .or_else(|| {
                self.engine
                    .agent_grant_for(flow, artifact)
                    .map(|grant| grant.commit.clone())
            })
    }

    fn test_demo(&self, flow: &str, run_id: &str) -> Result<Option<String>, String> {
        let state = self.flow(flow)?;
        // A hosted run that is still registering has no run row or files yet.
        let Some(run) = state
            .runs
            .iter()
            .find(|run| run.id == run_id && run.role == iteration::RunRole::AiTest)
        else {
            return Ok(None);
        };
        let commit = self
            .test_artifact_commit(flow, &run.artifact_id)
            .ok_or("Unknown demonstration artifact")?;
        test_demo_title(&self.directory, flow, run_id, &run.artifact_id, &commit)
    }

    fn test_pending(&self, flow: &str, run_id: &str) -> Option<&PendingEmbedded> {
        self.embedding_pending.values().find(|pending| {
            pending.flow == flow
                && pending.run == run_id
                && pending.role == iteration::RunRole::AiTest
        })
    }

    /// Finalized recordings and final frames actually found for a run. Absence
    /// is reported as such; nothing is claimed that was not observed.
    fn test_run_evidence(&self, flow: &str, run_id: &str) -> Value {
        let mut recordings = Vec::new();
        let mut errors = Vec::new();
        let mut finalized = false;
        for run in self
            .known_recording_runs()
            .iter()
            .filter(|run| run.run == run_id)
        {
            match recording_sidecars(&self.directory, run) {
                Ok(sidecars) => {
                    for sidecar in sidecars {
                        match recording_tile(&self.directory, run, &sidecar) {
                            Ok(tile) => {
                                // Usable means a finished MP4 with real frames and
                                // no encoder error; anything less is only listed.
                                let usable = tile.complete
                                    && !tile.active
                                    && tile.mp4.is_some()
                                    && tile.frames > 0
                                    && tile.error.is_none();
                                finalized |= usable;
                                recordings.push(json::obj(vec![
                                    ("id", s(&tile.id)),
                                    ("usable", Value::Bool(usable)),
                                    ("complete", Value::Bool(tile.complete)),
                                    ("active", Value::Bool(tile.active)),
                                    (
                                        "frames",
                                        Value::Int(tile.frames.min(i64::MAX as u64) as i64),
                                    ),
                                    (
                                        "mp4",
                                        tile.mp4
                                            .as_ref()
                                            .map(|path| s(path.to_string_lossy()))
                                            .unwrap_or(Value::Null),
                                    ),
                                    ("error", tile.error.as_ref().map(s).unwrap_or(Value::Null)),
                                    (
                                        "inspection_error",
                                        tile.inspection_error
                                            .as_ref()
                                            .map(s)
                                            .unwrap_or(Value::Null),
                                    ),
                                ]));
                            }
                            Err(error) => errors.push(s(error)),
                        }
                    }
                }
                Err(error) => errors.push(s(error)),
            }
        }
        let directory = self.directory.join("runs").join(run_id);
        let frames: Vec<Value> = (0..16)
            .map(|index| directory.join(format!("frozen-{index}.png")))
            .filter(|path| path.is_file())
            .map(|path| s(path.to_string_lossy()))
            .collect();
        json::obj(vec![("flow", s(flow)), ("run_id", s(run_id)), ("recordings", Value::Arr(recordings)), ("recording_finalized", Value::Bool(finalized)),
            ("final_frames", Value::Arr(frames)), ("errors", Value::Arr(errors)),
            ("note", s("Only files found on disk are listed; an empty list means no recording or final frame was observed"))])
    }

    fn test_request(&mut self, flow: &str, action: TestRequest) -> Result<Value, String> {
        if let TestRequest::Status {
            run_id,
            operation_id,
        } = &action
        {
            let operation_report = |host: &Host,
                                    operation: &str|
             -> Result<Option<Value>, String> {
                let owns = |report: &Value| {
                    report.get("flow").and_then(Value::as_str) == Some(flow)
                        && (report.get("run_id").and_then(Value::as_str) == Some(run_id)
                            || report.get("attempt_run_id").and_then(Value::as_str) == Some(run_id))
                };
                if let Some(report) = host
                    .reports
                    .get(&format!("test:{operation}"))
                    .filter(|report| owns(report))
                {
                    return Ok(Some(report.clone()));
                }
                for run in host
                    .flow(flow)?
                    .runs
                    .iter()
                    .filter(|run| run.role == iteration::RunRole::AiTest)
                {
                    let path = host
                        .directory
                        .join("runs")
                        .join(&run.id)
                        .join("testing")
                        .join(operation)
                        .join("operation.json");
                    if !path.is_file() {
                        continue;
                    }
                    let report = json::parse(&recording_bytes(&host.directory, &path, 128 * 1024)?)
                        .map_err(str::to_owned)?;
                    if owns(&report) {
                        return Ok(Some(report));
                    }
                }
                Ok(None)
            };
            if let Some(operation) = operation_id {
                return operation_report(self, operation)?
                    .ok_or_else(|| "Test operation does not belong to this run".into());
            }
            // A hosted run whose registration is still pending has an identity
            // but no process or run row yet; it is inspectable all the same.
            if let Some(pending) = self.test_pending(flow, run_id) {
                return Ok(json::obj(vec![
                    ("flow", s(flow)),
                    ("run_id", s(run_id)),
                    ("role", s("ai_test")),
                    ("artifact_id", s(&pending.artifact)),
                    ("phase", s("launching")),
                    ("pending", Value::Bool(true)),
                    ("requested_mode", s("embedded")),
                    ("observed_mode", Value::Null),
                    (
                        "operation_id",
                        pending.operation.as_deref().map(s).unwrap_or(Value::Null),
                    ),
                    (
                        "demo",
                        pending.demo.as_deref().map(s).unwrap_or(Value::Null),
                    ),
                    (
                        "grant",
                        pending.grant.as_deref().map(s).unwrap_or(Value::Null),
                    ),
                    (
                        "note",
                        s("Waiting for the Director host registration; no process exists yet"),
                    ),
                ]));
            }
            let attempt = self
                .reports
                .get(&format!("test-attempt:{run_id}"))
                .filter(|report| report.get("flow").and_then(Value::as_str) == Some(flow))
                .cloned();
            let Some(run) = self
                .flow(flow)?
                .runs
                .iter()
                .find(|run| run.id == *run_id && run.role == iteration::RunRole::AiTest)
            else {
                // A hosted attempt that never became a process: point at the run that replaced it.
                return attempt.ok_or_else(|| "Unknown AI test run for this flow".into());
            };
            let owned = self.apps.get(flow).filter(|app| app.run == *run_id);
            return Ok(json::obj(vec![("flow", s(flow)), ("run_id", s(run_id)), ("role", s("ai_test")), ("artifact_id", s(&run.artifact_id)),
                ("demo", self.test_demo(flow, run_id)?.as_deref().map(s).unwrap_or(Value::Null)),
                ("requested_mode", owned.map(|app| s(app.requested.as_str())).unwrap_or(Value::Null)), ("observed_mode", s(run.mode.as_str())),
                // From the run's own record, live or closed; never inferred.
                ("grant", owned.and_then(|app| app.grant.as_deref()).map(s)
                    .or_else(|| self.reports.get(&format!("run:{run_id}")).and_then(|report| report.get("grant")).cloned()).unwrap_or(Value::Null)),
                ("replaced_by", attempt.as_ref().and_then(|report| report.get("replaced_by")).cloned().unwrap_or(Value::Null)),
                ("closed", Value::Bool(run.closed)), ("owned", Value::Bool(owned.is_some())),
                ("remote_ready", Value::Bool(owned.is_some_and(|app| app.port.is_some()))),
                ("handed_off", Value::Bool(owned.is_some_and(|app| app.handed_off))),
                ("handoff_reason", owned.and_then(|app| app.handoff_reason.as_deref()).map(s).unwrap_or(Value::Null)),
                // The host transport's first presented frame; null when the run
                // is standalone and has no hosted surface to observe.
                ("first_frame", owned.filter(|app| app.mode == iteration::LaunchMode::Embedded).map(|app| Value::Bool(app.first_frame)).unwrap_or(Value::Null)),
                ("unobserved_windows", Value::Int(owned.map(|app| app.extra_windows).unwrap_or(0) as i64)),
                ("operation_id", self.test_operations.get(flow).map(|op| s(&op.id)).unwrap_or(Value::Null)),
                ("waiting_for", self.test_operations.get(flow).map(|op| s(op.phase.waiting_for())).unwrap_or(Value::Null)),
                ("operations", Value::Arr(self.reports.values().filter(|report| report.get("kind").and_then(Value::as_str) == Some("ai_test") && report.get("run_id").and_then(Value::as_str) == Some(run_id)).cloned().collect())),
                ("evidence", if run.closed { self.test_run_evidence(flow, run_id) } else { Value::Null }),
                ("video_directory", s(self.directory.join("runs").join(run_id).join("video").to_string_lossy())),
                ("test_result", s("Inspect actual input responses, captures and recordings; no pass is inferred"))]));
        }
        self.require_active_lane(flow)?;
        if self.test_operations.contains_key(flow) && !matches!(action, TestRequest::Stop { .. }) {
            return Err(
                "An AI test operation is still pending; poll its status before the next action"
                    .into(),
            );
        }
        let (id, _) = fresh_run_identity();
        let mut operation = TestOperation {
            id: format!("test-{id}"),
            run: String::new(),
            artifact: String::new(),
            directory: PathBuf::new(),
            phase: TestPhase::Ready,
            started: Instant::now(),
            phase_started: Instant::now(),
            child: None,
            input: None,
            window: None,
            query: String::new(),
            request: Value::Null,
            inspected: None,
            requested: iteration::LaunchMode::Standalone,
            attempt: None,
            attempt_client: None,
            fallback_reason: None,
            demo: None,
            grant: None,
            first_frame_ms: None,
        };
        match action {
            TestRequest::Start {
                artifact_id,
                demo,
                mode,
                grant,
            } => {
                if let Some(title) = &demo {
                    validate_test_demo(title)?;
                }
                let state = self.flow(flow)?;
                if state.runs.iter().any(|run| !run.closed) || self.apps.contains_key(flow) {
                    return Err("Close the existing app before starting an AI test; Studio will not close it for the agent".into());
                }
                if state
                    .runs
                    .iter()
                    .rev()
                    .find(|run| run.role == iteration::RunRole::Human)
                    .is_some_and(|run| !run.human_requested)
                {
                    return Err(
                        "The previous human app needs an explicit human close before testing"
                            .into(),
                    );
                }
                if self.builds.contains_key(flow)
                    || self
                        .embedding_pending
                        .values()
                        .any(|pending| pending.flow == flow)
                    || self
                        .effects
                        .iter()
                        .any(|effect| effect_flow(effect) == flow)
                    || state.job.as_ref().is_some_and(|job| {
                        matches!(
                            job.phase,
                            iteration::BuildPhase::WaitingForClose
                                | iteration::BuildPhase::CheckpointRequested
                                | iteration::BuildPhase::Ready
                                | iteration::BuildPhase::Building
                        )
                    })
                {
                    return Err(
                        "Finish or cancel the queued build/launch before starting an AI test"
                            .into(),
                    );
                }
                // Own artifact, or exactly the one a grant to this lane names.
                let retained = self.retained_artifact(flow, &artifact_id, grant.as_deref())?;
                // Full capacity is reported, never launched around: the budget
                // covers standalone test processes as well.
                self.app_budget()?;
                operation.requested = mode.unwrap_or(iteration::LaunchMode::Embedded);
                operation.artifact = artifact_id.clone();
                operation.demo = demo.clone();
                operation.grant = grant.clone();
                let mut request = vec![
                    ("action", s("start")),
                    ("artifact_id", s(artifact_id)),
                    ("mode", s(operation.requested.as_str())),
                ];
                if let Some(title) = demo {
                    request.push(("demo", s(title)));
                }
                if let Some(grant) = grant {
                    request.push(("grant", s(grant)));
                }
                operation.request = json::obj(request);
                if operation.requested == iteration::LaunchMode::Embedded
                    && self.embedding_port.is_some()
                {
                    // Hosted: the run identity exists now; the process starts
                    // only after the broker registered it with every host
                    // setting. A bare --stdin-loop launch is never attempted.
                    operation.run = self.queue_embedded_test(
                        flow,
                        &retained,
                        &operation.id,
                        operation.demo.as_deref(),
                    )?;
                    operation.phase = TestPhase::Launching;
                } else {
                    if operation.requested == iteration::LaunchMode::Embedded {
                        // Nothing was launched, so this bounded fallback cannot
                        // duplicate an app; its reason is retained.
                        operation.fallback_reason = Some(
                            "Director's embedding host is not available in this session".into(),
                        );
                    }
                    self.spawn_app(AppLaunch {
                        flow,
                        artifact: &retained.id,
                        path: &retained.path,
                        mode: iteration::LaunchMode::Standalone,
                        requested: operation.requested,
                        reopening: false,
                        embedded: None,
                        role: iteration::RunRole::AiTest,
                        grant: operation.grant.as_deref(),
                        demo: operation.demo.as_deref(),
                    })?;
                    operation.run = self
                        .apps
                        .get(flow)
                        .ok_or("Test child ownership was not retained")?
                        .run
                        .clone();
                    if let Err(error) = self.test_prepare_evidence(flow, &mut operation) {
                        let _ = self.close(flow);
                        return Err(error);
                    }
                }
            }
            TestRequest::Input { run_id, input } => {
                operation.request = json::obj(vec![
                    ("action", s("input")),
                    ("route", s(input.route)),
                    (
                        "parameters",
                        Value::Obj(
                            input
                                .params
                                .iter()
                                .map(|(key, value)| (key.clone(), s(value)))
                                .collect(),
                        ),
                    ),
                ]);
                operation.run = run_id;
                operation.phase = TestPhase::Input;
                operation.input = Some(input);
            }
            TestRequest::Snapshot {
                run_id,
                window,
                query,
            } => {
                if self
                    .flow(flow)?
                    .captures
                    .iter()
                    .filter(|capture| capture.run_id == run_id)
                    .count()
                    >= TEST_MAX_CAPTURES
                {
                    return Err("This test run has retained 16 captures; inspect its video or begin a new test run".into());
                }
                operation.request = json::obj(vec![
                    ("action", s("snapshot")),
                    (
                        "window",
                        window
                            .map(|value| Value::Int(value.into()))
                            .unwrap_or(Value::Null),
                    ),
                    ("query", s(&query)),
                ]);
                operation.run = run_id;
                operation.phase = TestPhase::Inspect;
                operation.window = window;
                operation.query = query;
            }
            TestRequest::Stop { run_id } => {
                operation.request = json::obj(vec![("action", s("stop"))]);
                operation.run = run_id.clone();
                operation.phase = TestPhase::Stop;
                if let Some(pending) = self
                    .test_pending(flow, &run_id)
                    .map(|pending| pending.operation.clone())
                {
                    // Stopping a hosted test that never got a process: cancel
                    // its registration; there is nothing to close.
                    self.cancel_test_operation(
                        flow,
                        "Canceled by explicit test stop before the hosted app launched",
                    );
                    self.cancel_pending_embedding(
                        flow,
                        "Test stopped before the hosted app launched",
                    );
                    if let Some(operation) = pending {
                        self.reports.remove(&format!("test-launch:{operation}"));
                    }
                    return Ok(json::obj(vec![
                        ("flow", s(flow)),
                        ("run_id", s(run_id)),
                        ("phase", s("canceled")),
                        ("complete", Value::Bool(true)),
                        ("launched", Value::Bool(false)),
                    ]));
                }
                if self.test_owned_app(flow, &run_id, false)?.handed_off {
                    return Err("The person took this test app over; it stays running until they close it and is not stopped for the agent".into());
                }
                self.cancel_test_operation(flow, "Canceled by explicit test stop");
                self.close(flow)?;
            }
            TestRequest::Status { .. } => unreachable!(),
        }
        if !matches!(operation.phase, TestPhase::Launching)
            && operation.directory.as_os_str().is_empty()
        {
            self.test_owned_app(flow, &operation.run, operation.phase != TestPhase::Stop)?;
            operation.artifact = self
                .apps
                .get(flow)
                .map(|app| app.artifact.clone())
                .unwrap_or_default();
            self.test_prepare_evidence(flow, &mut operation)?;
        }
        match self.publish_test(flow, &operation, "pending", None, None) {
            Ok(report) => {
                self.test_operations.insert(flow.into(), operation);
                Ok(report)
            }
            Err(error) => {
                // A start nobody can observe leaves no app or registration behind.
                if operation.phase.starting() {
                    let _ = self.close(flow);
                }
                Err(error)
            }
        }
    }

    /// The evidence directory lives under the run the process actually owns,
    /// so it can only be created once that process exists.
    fn test_prepare_evidence(
        &mut self,
        flow: &str,
        operation: &mut TestOperation,
    ) -> Result<(), String> {
        let run = self
            .apps
            .get(flow)
            .filter(|app| app.run == operation.run)
            .ok_or("No app process owned by this host for the test")?;
        let root = run.directory.join("testing");
        if let (Some(title), true) = (&operation.demo, operation.phase.starting()) {
            let commit = self
                .test_artifact_commit(flow, &operation.artifact)
                .ok_or("Unknown demonstration artifact")?;
            let path = self
                .directory
                .join("runs")
                .join(&operation.run)
                .join("demo.json");
            recording_owned_path(&self.directory, &path)?;
            if path.try_exists().map_err(err)? {
                return Err("Demonstration metadata already exists for this run".into());
            }
            let metadata = json::obj(vec![
                ("kind", s("studio_demo")),
                ("version", Value::Int(1)),
                ("flow", s(flow)),
                ("run", s(&operation.run)),
                ("artifact", s(&operation.artifact)),
                ("commit", s(commit)),
                ("title", s(title)),
            ]);
            atomic_write(&path, metadata.to_json().as_bytes())?;
        }
        fs::create_dir_all(&root).map_err(err)?;
        if fs::read_dir(&root)
            .map_err(err)?
            .take(TEST_MAX_OPERATIONS + 1)
            .count()
            >= TEST_MAX_OPERATIONS
        {
            return Err("This test run reached its 128-operation history bound".into());
        }
        let directory = root.join(&operation.id);
        fs::create_dir(&directory).map_err(err)?;
        operation.directory = directory;
        Ok(())
    }

    fn test_owned_app(
        &mut self,
        flow: &str,
        run_id: &str,
        require_running: bool,
    ) -> Result<&mut AppRun, String> {
        let run = self
            .apps
            .get_mut(flow)
            .ok_or("No app process owned by this host for the test")?;
        if run.run != run_id || run.role != iteration::RunRole::AiTest {
            return Err("AI input is restricted to the exact owned AI test run; human apps cannot be controlled".into());
        }
        if require_running && run.handed_off {
            return Err("The person interacted with this test app; automation stopped and the instance stays theirs until they close it".into());
        }
        if require_running
            && (run.closing.is_some()
                || run.exit.is_some()
                || run.child.try_wait().map_err(err)?.is_some())
        {
            return Err("The owned AI test is closing or has exited".into());
        }
        Ok(run)
    }

    fn publish_test(
        &mut self,
        flow: &str,
        operation: &TestOperation,
        phase: &str,
        result: Option<Value>,
        error: Option<&str>,
    ) -> Result<Value, String> {
        let observed = self
            .apps
            .get(flow)
            .filter(|app| app.run == operation.run)
            .map(|app| app.mode)
            .or_else(|| {
                self.flow(flow).ok().and_then(|state| {
                    state
                        .runs
                        .iter()
                        .find(|run| run.id == operation.run)
                        .map(|run| run.mode)
                })
            });
        let report = json::obj(vec![
            ("kind", s("ai_test")),
            ("flow", s(flow)),
            ("run_id", s(&operation.run)),
            ("artifact_id", s(&operation.artifact)),
            (
                "node",
                self.engine
                    .agent_node(flow)
                    .map(|node| s(&node.id))
                    .unwrap_or(Value::Null),
            ),
            (
                "demo",
                self.test_demo(flow, &operation.run)?
                    .as_deref()
                    .map(s)
                    .or_else(|| operation.demo.as_deref().map(s))
                    .unwrap_or(Value::Null),
            ),
            ("requested_mode", s(operation.requested.as_str())),
            (
                "observed_mode",
                observed.map(|mode| s(mode.as_str())).unwrap_or(Value::Null),
            ),
            (
                "attempt_run_id",
                operation.attempt.as_deref().map(s).unwrap_or(Value::Null),
            ),
            (
                "fallback_reason",
                operation
                    .fallback_reason
                    .as_deref()
                    .map(s)
                    .unwrap_or(Value::Null),
            ),
            (
                "grant",
                operation.grant.as_deref().map(s).unwrap_or(Value::Null),
            ),
            ("operation_id", s(&operation.id)),
            ("phase", s(phase)),
            (
                "complete",
                Value::Bool(matches!(phase, "completed" | "failed" | "canceled")),
            ),
            ("request", operation.request.clone()),
            ("result", result.unwrap_or(Value::Null)),
            ("error", error.map(s).unwrap_or(Value::Null)),
            (
                "elapsed_ms",
                Value::Int(
                    operation
                        .started
                        .elapsed()
                        .as_millis()
                        .min(i64::MAX as u128) as i64,
                ),
            ),
            (
                "evidence_directory",
                if operation.directory.as_os_str().is_empty() {
                    Value::Null
                } else {
                    s(operation.directory.to_string_lossy())
                },
            ),
            ("test_result", s("Not inferred from command completion")),
        ]);
        // Before the process exists there is no owned run directory to write to.
        if !operation.directory.as_os_str().is_empty() {
            if let Err(error) = atomic_write(
                &operation.directory.join("operation.json"),
                report.to_json().as_bytes(),
            ) {
                self.storage_error = Some(format!("AI test evidence could not be saved: {error}"));
                self.changed = true;
                return Err(error);
            }
        }
        self.reports
            .insert(format!("test:{}", operation.id), report.clone());
        let old: Vec<_> = self
            .reports
            .iter()
            .filter(|(key, value)| {
                key.starts_with("test:") && value.get("complete") == Some(&Value::Bool(true))
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in old.iter().take(old.len().saturating_sub(64)) {
            self.reports.remove(key);
        }
        self.changed = true;
        Ok(report)
    }

    fn cancel_test_operation(&mut self, flow: &str, reason: &str) {
        if let Some(mut operation) = self.test_operations.remove(flow) {
            if let Some(child) = &mut operation.child {
                let _ = child.kill();
                let _ = child.wait();
            }
            if let Err(error) = self.publish_test(flow, &operation, "canceled", None, Some(reason))
            {
                self.note = format!("Test cancellation evidence: {error}");
                self.changed = true;
            }
        }
    }

    fn poll_testing(&mut self) {
        let flows: Vec<_> = self.test_operations.keys().cloned().collect();
        for flow in flows {
            let Some(mut operation) = self.test_operations.remove(&flow) else {
                continue;
            };
            match self.advance_test(&flow, &mut operation) {
                Ok(Some(result)) => {
                    if let Err(error) =
                        self.publish_test(&flow, &operation, "completed", Some(result), None)
                    {
                        self.note = format!("Test completion evidence: {error}");
                        self.changed = true;
                    }
                }
                Ok(None) => {
                    self.test_operations.insert(flow, operation);
                }
                Err(error) => {
                    if let Some(child) = &mut operation.child {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    let _ = self.publish_test(&flow, &operation, "failed", None, Some(&error));
                    // A start that never began may close what it launched. An
                    // active test is never closed or rerun for the agent, and
                    // an app the person took over is never touched.
                    let owned = self
                        .apps
                        .get(&flow)
                        .is_some_and(|app| app.run == operation.run && !app.handed_off)
                        || self.test_pending(&flow, &operation.run).is_some();
                    if operation.phase.starting() && owned {
                        let _ = self.close(&flow);
                    }
                    self.note = format!("{flow}: AI test operation failed: {error}");
                    self.changed = true;
                }
            }
        }
    }

    /// A hosted attempt is over (it never launched, or it exited and was
    /// unregistered): start the observable standalone replacement under a
    /// fresh run identity and retain the attempt so the caller finds it.
    fn test_fallback_standalone(
        &mut self,
        flow: &str,
        operation: &mut TestOperation,
        reason: String,
    ) -> Result<(), String> {
        let retained =
            self.retained_artifact(flow, &operation.artifact, operation.grant.as_deref())?;
        self.spawn_app(AppLaunch {
            flow,
            artifact: &retained.id,
            path: &retained.path,
            mode: iteration::LaunchMode::Standalone,
            requested: iteration::LaunchMode::Embedded,
            reopening: false,
            embedded: None,
            role: iteration::RunRole::AiTest,
            grant: operation.grant.as_deref(),
            demo: operation.demo.as_deref(),
        })?;
        let replacement = self
            .apps
            .get(flow)
            .ok_or("Test child ownership was not retained")?
            .run
            .clone();
        let attempt = std::mem::replace(&mut operation.run, replacement.clone());
        self.reports.insert(
            format!("test-attempt:{attempt}"),
            json::obj(vec![
                ("kind", s("ai_test_attempt")),
                ("flow", s(flow)),
                ("run_id", s(&attempt)),
                ("replaced_by", s(&replacement)),
                ("operation_id", s(&operation.id)),
                ("requested_mode", s("embedded")),
                ("observed_mode", s("standalone")),
                ("reason", s(&reason)),
                (
                    "note",
                    s("The hosted attempt ended before the test began; poll the replacement run"),
                ),
            ]),
        );
        let old: Vec<_> = self
            .reports
            .keys()
            .filter(|key| key.starts_with("test-attempt:"))
            .cloned()
            .collect();
        for key in old.iter().take(old.len().saturating_sub(64)) {
            self.reports.remove(key);
        }
        // The pointer to the real run must survive a restart.
        self.persist()?;
        operation.attempt = Some(attempt);
        operation.attempt_client = None;
        operation.fallback_reason = Some(reason);
        operation.directory = PathBuf::new();
        self.test_prepare_evidence(flow, operation)?;
        operation.phase = TestPhase::Ready;
        operation.phase_started = Instant::now();
        Ok(())
    }

    fn advance_test(
        &mut self,
        flow: &str,
        operation: &mut TestOperation,
    ) -> Result<Option<Value>, String> {
        if operation.phase == TestPhase::Stop {
            if let Some(app) = self
                .apps
                .get(flow)
                .filter(|app| app.run == operation.run && app.handed_off)
            {
                return Err(format!(
                    "The test app was not closed and stays running: {}",
                    app.handoff_reason
                        .as_deref()
                        .unwrap_or("a person is using it")
                ));
            }
            if self
                .apps
                .get(flow)
                .is_some_and(|app| app.run == operation.run)
            {
                return Ok(None);
            }
            let run = self
                .flow(flow)?
                .runs
                .iter()
                .find(|run| run.id == operation.run && run.role == iteration::RunRole::AiTest)
                .ok_or("Test ownership history disappeared")?;
            if !run.closed {
                return Err("Test process exited without a durable closure observation".into());
            }
            return Ok(Some(json::obj(vec![
                ("closed", Value::Bool(true)),
                ("human_requested", Value::Bool(false)),
                (
                    "exit_code",
                    run.exit_code
                        .map(|code| Value::Int(code.into()))
                        .unwrap_or(Value::Null),
                ),
                ("observed_mode", s(run.mode.as_str())),
                ("evidence", self.test_run_evidence(flow, &operation.run)),
            ])));
        }
        if let Some(error) = &self.storage_error {
            return Err(format!(
                "Test paused because durable evidence is unavailable: {error}"
            ));
        }
        self.require_active_lane(flow)?;
        if operation.phase == TestPhase::Launching {
            if self.test_pending(flow, &operation.run).is_some() {
                return Ok(None);
            }
            if let Some(client) = self
                .apps
                .get(flow)
                .filter(|app| app.run == operation.run)
                .map(|app| app.embedding_client)
            {
                // Remembered so its unregistration can be verified even after
                // the process is gone.
                operation.attempt_client = client;
                self.test_prepare_evidence(flow, operation)?;
                operation.phase = TestPhase::Ready;
                operation.phase_started = Instant::now();
                return Ok(None);
            }
            if self
                .flow(flow)?
                .runs
                .iter()
                .any(|run| run.id == operation.run)
            {
                // It launched and already exited: await its closure below.
                operation.phase = TestPhase::Fallback;
                operation.phase_started = Instant::now();
                operation.fallback_reason =
                    Some("The hosted app exited before it became ready".into());
                return Ok(None);
            }
            // The registration ended without any process, so a replacement
            // cannot duplicate an app.
            let reason = self
                .reports
                .remove(&format!("test-launch:{}", operation.id))
                .and_then(|report| {
                    report
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "The hosted launch ended before its process started".into());
            self.test_fallback_standalone(flow, operation, reason)?;
            return Ok(None);
        }
        if operation.phase == TestPhase::Fallback {
            if let Some((handed_off, closing)) = self
                .apps
                .get(flow)
                .filter(|app| app.run == operation.run)
                .map(|app| (app.handed_off, app.closing.is_some()))
            {
                if handed_off {
                    return Err("A person took the hosted attempt over; it stays running and no replacement was started".into());
                }
                // Close exactly the owned attempt through the guarded automatic
                // path; its exit is awaited here.
                if !closing {
                    self.close(flow)?;
                }
                if operation.phase_started.elapsed() > Duration::from_secs(30) {
                    return Err(
                        "The hosted attempt did not exit; no replacement was started".into(),
                    );
                }
                return Ok(None);
            }
            let closed = self
                .flow(flow)?
                .runs
                .iter()
                .find(|run| run.id == operation.run)
                .is_some_and(|run| run.closed);
            let unregistered = operation
                .attempt_client
                .is_none_or(|client| !self.embedding_unregister.contains(&client));
            if !closed || !unregistered {
                if operation.phase_started.elapsed() > Duration::from_secs(30) {
                    return Err("The hosted attempt's exit or unregistration was not observed; no replacement was started".into());
                }
                return Ok(None);
            }
            let reason = operation
                .fallback_reason
                .clone()
                .unwrap_or_else(|| "The hosted app never became ready".into());
            self.test_fallback_standalone(flow, operation, reason)?;
            return Ok(None);
        }
        let starting_hosted = operation.phase.starting()
            && operation.attempt.is_none()
            && operation.requested == iteration::LaunchMode::Embedded;
        let owned = self.test_owned_app(flow, &operation.run, true).map(|run| {
            (
                run.child.id(),
                run.user_seq,
                run.mode == iteration::LaunchMode::Embedded,
                run.embedding_client,
                run.port,
            )
        });
        let (pid, expected_seq, hosted, client, port) = match owned {
            Ok(owned) => owned,
            Err(error) => {
                // A hosted app that died before the test began is replaced
                // once; an app the person took over is never replaced.
                let handed_off = self.apps.get(flow).is_some_and(|app| app.handed_off);
                let was_hosted = self.apps.get(flow).is_some_and(|app| {
                    app.run == operation.run && app.mode == iteration::LaunchMode::Embedded
                }) || self.flow(flow)?.runs.iter().any(|run| {
                    run.id == operation.run && run.mode == iteration::LaunchMode::Embedded
                });
                if !starting_hosted || handed_off || !was_hosted {
                    return Err(error);
                }
                operation.fallback_reason = Some(format!(
                    "The hosted app ended before the test began: {error}"
                ));
                operation.phase = TestPhase::Fallback;
                operation.phase_started = Instant::now();
                return Ok(None);
            }
        };
        let port = match port {
            Some(port) => port,
            None if operation.phase == TestPhase::Ready
                && operation.phase_started.elapsed() < Duration::from_secs(15) =>
            {
                return Ok(None)
            }
            None if starting_hosted && hosted => {
                // Hosting is unsupported by this app or never came up: close
                // exactly this process, await its exit, then fall back.
                operation.attempt_client = operation.attempt_client.or(client);
                operation.fallback_reason = Some(
                    "The hosted app did not publish its remote endpoint within 15 seconds".into(),
                );
                operation.phase = TestPhase::Fallback;
                operation.phase_started = Instant::now();
                return Ok(None);
            }
            None => {
                return Err(
                    "The owned test has not published its matching-PID remote endpoint".into(),
                )
            }
        };
        let limit = if operation.phase == TestPhase::Frame {
            TEST_PNG_LIMIT
        } else {
            TEST_JSON_LIMIT
        };
        let observed_files = [
            (operation.response(), limit),
            (operation.stderr(), 64 * 1024),
            (operation.headers(), 64 * 1024),
            (operation.status(), 64),
        ];
        if let Some(child) = &mut operation.child {
            for (path, limit) in observed_files {
                if let Ok(metadata) = fs::symlink_metadata(path) {
                    if !metadata.file_type().is_file() || metadata.len() > limit {
                        return Err(
                            "Test endpoint output exceeded its bounded regular-file limit".into(),
                        );
                    }
                }
            }
            if operation.phase_started.elapsed() > Duration::from_secs(20) {
                return Err("Owned test endpoint timed out".into());
            }
            let Some(status) = child.try_wait().map_err(err)? else {
                return Ok(None);
            };
            operation.child = None;
            // Ownership evidence is read before anything else, also when curl
            // itself failed: a partial or timed-out reply can still carry the
            // 409 or a moved input counter, and an app a person took over must
            // never reach the start-failure cleanup below.
            let code = bounded_read(&operation.status(), 64)
                .ok()
                .and_then(|code| code.trim().parse::<u16>().ok())
                .filter(|code| *code != 0);
            let ending = bounded_read(&operation.headers(), 64 * 1024)
                .ok()
                .and_then(|headers| test_user_seq_header(&headers));
            if code == Some(409)
                || matches!((expected_seq, ending), (Some(expected), Some(ending)) if expected != ending)
            {
                let reason = format!(
                    "a person interacted with this test app (HTTP {}, input counter {} then {})",
                    code.map(|code| code.to_string())
                        .unwrap_or_else(|| "none".into()),
                    expected_seq
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "unread".into()),
                    ending
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "unreported".into())
                );
                if let Some(app) = self
                    .apps
                    .get_mut(flow)
                    .filter(|app| app.run == operation.run)
                {
                    hand_off(app, reason.clone());
                }
                self.changed = true;
                return Err(format!("Automation stopped: {reason}. Nothing was retried and the instance is left running for them"));
            }
            if !status.success() {
                return Err(format!(
                    "Owned test endpoint failed: {}",
                    log_tail(&operation.stderr(), 4096).unwrap_or_else(|_| status.to_string())
                ));
            }
            let code = code.ok_or("Owned test endpoint returned no HTTP status")?;
            if code != 200 {
                return Err(format!(
                    "Owned test endpoint returned HTTP {code}: {}",
                    log_tail(&operation.response(), 1024).unwrap_or_default()
                ));
            }
            // With a guard established, the unchanged ending counter is the
            // evidence that this step was the test's. Its absence is not.
            if expected_seq.is_some() && ending.is_none() {
                return Err("The reply carried no X-Makepad-User-Seq, so this step cannot be attributed to the test; nothing was retried".into());
            }
            if operation.phase == TestPhase::Frame {
                let source = operation.response();
                let bytes = recording_bytes(&self.directory, &source, TEST_PNG_LIMIT as usize)?;
                if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                    return Err("Owned test snapshot returned no PNG".into());
                }
                let (width, height) =
                    makepad_widgets::image_cache::image_size_by_data(&bytes, &source)
                        .map_err(err)?;
                if width == 0 || height == 0 || width.saturating_mul(height) > 16 * 1024 * 1024 {
                    return Err("Test PNG exceeds 16 megapixels".into());
                }
                let _ =
                    makepad_widgets::image_cache::decode_image_from_data(&bytes).map_err(err)?;
                let path = operation.directory.join("frame.png");
                fs::rename(&source, &path).map_err(err)?;
                let capture_id = format!("capture-{}", operation.id);
                self.observe(Observation::Captured {
                    flow: flow.into(),
                    capture: iteration::Capture {
                        id: capture_id.clone(),
                        artifact_id: operation.artifact.clone(),
                        run_id: operation.run.clone(),
                        path: path.clone(),
                        width: width as u32,
                        height: height as u32,
                        timestamp_ms: now(),
                    },
                })?;
                return Ok(Some(json::obj(vec![
                    ("capture_id", s(capture_id)),
                    ("png", s(path.to_string_lossy())),
                    ("width", Value::Int(width as i64)),
                    ("height", Value::Int(height as i64)),
                    (
                        "widget_snapshot",
                        test_response_excerpt(operation.inspected.as_deref().unwrap_or("")),
                    ),
                    (
                        "widget_snapshot_path",
                        s(operation.directory.join("widgets.json").to_string_lossy()),
                    ),
                ])));
            }
            let response = String::from_utf8(recording_bytes(
                &self.directory,
                &operation.response(),
                TEST_JSON_LIMIT as usize,
            )?)
            .map_err(err)?;
            let parsed = json::parse(response.as_bytes()).map_err(str::to_owned)?;
            if let Some(error) = parsed.get("err") {
                return Err(format!("Owned test endpoint: {}", error.to_json()));
            }
            match operation.phase {
                TestPhase::Ready => {
                    if parsed.get("pid").and_then(Value::as_u64) != Some(pid.into()) {
                        return Err("Owned test endpoint PID does not match its process".into());
                    }
                    // A hosted app must also present a frame through the host
                    // transport; a standalone one has no hosted surface.
                    operation.phase = if hosted {
                        TestPhase::Surface
                    } else {
                        TestPhase::Activity
                    };
                    operation.phase_started = Instant::now();
                }
                TestPhase::Activity => {
                    let sequence = parsed.get("user_seq").and_then(Value::as_u64).ok_or("The test app's /activity reports no user_seq; refusing to drive it without the human-input guard")?;
                    // /activity is read exactly once. The guard is adopted only
                    // from a quiet reading whose body and ending header agree;
                    // an observed intervention, or missing ending evidence,
                    // stops automation here: the counter is never read again,
                    // nothing is retried and the app is left to the person.
                    let busy = parsed.get("user_active").and_then(Value::as_bool) != Some(false)
                        || parsed.get("held").and_then(Value::as_bool) != Some(false);
                    let refusal = if busy {
                        Some("a person is using the app (its input is active or held)".to_owned())
                    } else {
                        match ending {
                            None => Some("the /activity reply carried no ending X-Makepad-User-Seq, so ownership of the app is unproven".to_owned()),
                            Some(ending) if ending != sequence => Some(format!("the person's input counter moved from {sequence} to {ending} while /activity was read")),
                            Some(_) => None,
                        }
                    };
                    if let Some(reason) = refusal {
                        if let Some(app) = self
                            .apps
                            .get_mut(flow)
                            .filter(|app| app.run == operation.run)
                        {
                            hand_off(app, reason.clone());
                        }
                        self.changed = true;
                        return Err(format!("The test did not begin: {reason}. The counter was not read again and nothing was retried, closed or replaced; the instance is left running for the person, and a new test can start once they closed it"));
                    }
                    if let Some(app) = self.apps.get_mut(flow) {
                        app.user_seq = Some(sequence);
                    }
                    return Ok(Some(json::obj(vec![
                        ("remote_ready", Value::Bool(true)),
                        ("user_seq", Value::Int(sequence.min(i64::MAX as u64) as i64)),
                        (
                            "first_frame",
                            if hosted {
                                Value::Bool(true)
                            } else {
                                Value::Null
                            },
                        ),
                        (
                            "first_frame_ms",
                            operation
                                .first_frame_ms
                                .map(|ms| Value::Int(ms.min(i64::MAX as u64) as i64))
                                .unwrap_or(Value::Null),
                        ),
                        ("requested_mode", s(operation.requested.as_str())),
                        (
                            "observed_mode",
                            s(if hosted { "embedded" } else { "standalone" }),
                        ),
                        (
                            "attempt_run_id",
                            operation.attempt.as_deref().map(s).unwrap_or(Value::Null),
                        ),
                        (
                            "fallback_reason",
                            operation
                                .fallback_reason
                                .as_deref()
                                .map(s)
                                .unwrap_or(Value::Null),
                        ),
                        ("activity", test_response_excerpt(&response)),
                    ])));
                }
                TestPhase::Input => {
                    return Ok(Some(json::obj(vec![
                        ("response", test_response_excerpt(&response)),
                        ("frame_waited", Value::Bool(true)),
                    ])))
                }
                TestPhase::Inspect => {
                    fs::rename(
                        operation.response(),
                        operation.directory.join("widgets.json"),
                    )
                    .map_err(err)?;
                    operation.inspected = Some(response);
                    operation.phase = TestPhase::Frame;
                }
                _ => unreachable!(),
            }
        }
        if operation.phase == TestPhase::Surface {
            if self
                .apps
                .get(flow)
                .is_some_and(|app| app.run == operation.run && app.first_frame)
            {
                operation.first_frame_ms = Some(
                    operation
                        .started
                        .elapsed()
                        .as_millis()
                        .min(u64::MAX as u128) as u64,
                );
                operation.phase = TestPhase::Activity;
            } else if operation.phase_started.elapsed() < Duration::from_secs(20) {
                return Ok(None);
            } else if starting_hosted {
                // The port answers but nothing renders through the host. The
                // test has not begun: retire exactly this attempt, then fall
                // back once.
                operation.attempt_client = operation.attempt_client.or(client);
                operation.fallback_reason = Some(
                    "The hosted app presented no frame through the host within 20 seconds".into(),
                );
                operation.phase = TestPhase::Fallback;
                operation.phase_started = Instant::now();
                return Ok(None);
            } else {
                return Err(
                    "The hosted app presented no frame through the host within 20 seconds".into(),
                );
            }
        }
        let (route, mut params) = match operation.phase {
            TestPhase::Ready => ("s", vec![]),
            TestPhase::Activity => ("activity", vec![]),
            TestPhase::Input => {
                let input = operation.input.as_ref().ok_or("Missing typed test input")?;
                (input.route, input.params.clone())
            }
            TestPhase::Inspect => (
                "snap",
                if operation.query.is_empty() {
                    vec![]
                } else {
                    vec![("q".into(), operation.query.clone())]
                },
            ),
            TestPhase::Frame => ("g", vec![("raw".into(), "1".into())]),
            TestPhase::Launching | TestPhase::Fallback | TestPhase::Surface | TestPhase::Stop => {
                unreachable!()
            }
        };
        if matches!(operation.phase, TestPhase::Inspect | TestPhase::Frame) {
            if let Some(window) = operation.window {
                params.push(("w".into(), window.to_string()));
            }
        }
        if operation.phase == TestPhase::Input {
            // Every mutation carries the counter read at the start of this test.
            let sequence = expected_seq
                .ok_or("This test has no input guard; start the test again before sending input")?;
            params.push(("if_user_seq".into(), sequence.to_string()));
        }
        let _ = fs::remove_file(operation.response());
        // The reply's status and headers are the ownership evidence: when they
        // cannot be recorded, nothing is sent.
        File::create(operation.headers())
            .map_err(|error| format!("Test reply headers cannot be recorded: {error}"))?;
        let error_output = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(operation.stderr())
            .map_err(err)?;
        let status_output = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(operation.status())
            .map_err(err)?;
        let mut command = Command::new("curl");
        // Disable user curl configuration, proxies and redirects. Route and
        // address are derived exclusively from our live owned AppRun. The
        // status and headers are kept so a 409 and the input counter are seen.
        command
            .args([
                "-q",
                "--noproxy",
                "*",
                "--proto",
                "=http",
                "--silent",
                "--show-error",
                "--connect-timeout",
                "1",
                "--max-time",
                "16",
                "--max-filesize",
            ])
            .arg(
                if operation.phase == TestPhase::Frame {
                    TEST_PNG_LIMIT
                } else {
                    TEST_JSON_LIMIT
                }
                .to_string(),
            )
            .args(["--write-out", "%{http_code}", "--dump-header"])
            .arg(operation.headers())
            .args(["--get", "--output"])
            .arg(operation.response())
            .stdin(Stdio::null())
            .stdout(status_output)
            .stderr(error_output);
        for (key, value) in params {
            command
                .arg("--data-urlencode")
                .arg(format!("{key}={value}"));
        }
        command.arg(format!("http://127.0.0.1:{port}/{route}"));
        operation.child = Some(command.spawn().map_err(err)?);
        operation.phase_started = Instant::now();
        Ok(None)
    }
}

fn test_response_excerpt(response: &str) -> Value {
    let mut end = response.len().min(32 * 1024);
    while !response.is_char_boundary(end) {
        end -= 1;
    }
    json::obj(vec![
        ("json", s(&response[..end])),
        ("truncated", Value::Bool(end != response.len())),
    ])
}

fn retained_binary_hash(path: &Path) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path).map_err(err)?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > 1024 * 1024 * 1024
    {
        return Err("Retained executable must be a bounded regular file".into());
    }
    git::content_hash(path).map_err(|error| format!("Cannot hash the retained executable: {error}"))
}
