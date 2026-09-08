// Included by iteration_worker.rs: CLI requests run on the existing host worker.

const CLI_REQUEST_LIMIT: usize = 64 * 1024;
const CLI_REPLY_LIMIT: usize = 8 * 1024 * 1024;
const CLI_HISTORY_LIMIT: usize = 4096;
const CLI_REPLY_BYTES: u64 = 32 * 1024 * 1024;

fn cli_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// Only operations scoped to one flow are offered by the terminal bridge.
pub fn cli_tool_name(name: &str) -> Result<&str, String> {
    Ok(match name {
        "inspect" | "flow_inspect" => "flow_inspect",
        "rename" | "flow_rename" => "flow_rename",
        "context" | "flow_context" => "flow_context",
        "report" | "flow_report" => "flow_report",
        "split" | "flow_lane" => "flow_lane",
        "todos" | "flow_todos" => "flow_todos",
        "requirement" | "flow_requirement" => "flow_requirement",
        "prepared" | "flow_prepared" => "flow_prepared",
        "build" | "flow_build" => "flow_build",
        "freeze" | "flow_freeze" => "flow_freeze",
        "feedback" | "flow_feedback" => "flow_feedback",
        "git_inspect" | "flow_git_inspect" => "flow_git_inspect",
        "promotion_preview" | "flow_promotion_preview" => "flow_promotion_preview",
        "promote" | "flow_promote" => "flow_promote",
        "sync_preview" | "flow_sync_preview" => "flow_sync_preview",
        "sync" | "flow_sync" => "flow_sync",
        "fetch" | "flow_fetch" => "flow_fetch",
        "diff" | "flow_diff" => "flow_diff",
        "test" | "flow_test" => "flow_test",
        // Code-intelligence tools the lane may see (never the view Act tools).
        _ if crate::atlas::registry::lane_offers(name, false) => name,
        _ => return Err("Tool is unavailable through the per-flow terminal bridge".into()),
    })
}

/// Whether a bridged tool is a code-intelligence tool: its scope is the lane
/// identity itself, never an argument.
pub fn cli_code_tool(tool: &str) -> bool {
    crate::atlas::registry::lane_offers(tool, false)
}

/// Scope is supplied by Studio's launch environment, never by tool contents.
pub fn cli_service_call(
    id: &str,
    flow: &str,
    tool: &str,
    args: Value,
) -> Result<makepad_ai_services::wire::ServiceCall, String> {
    if !cli_identifier(id) || !cli_identifier(flow) {
        return Err("Invalid request or flow identity".into());
    }
    let tool = cli_tool_name(tool)?;
    let Value::Obj(mut fields) = args else {
        return Err("Tool arguments must be a JSON object".into());
    };
    for (key, value) in &fields {
        if matches!(key.as_str(), "f" | "flow") && value.as_str() != Some(flow) {
            return Err("The terminal may only address its assigned flow".into());
        }
    }
    fields.retain(|(key, _)| !matches!(key.as_str(), "f" | "flow"));
    if cli_code_tool(tool) {
        // The lane identity travels out of band; the registry validates the
        // arguments exactly as the manifest advertises them.
        let args = Value::Obj(fields);
        crate::atlas::registry::parse_lane_call(tool, &args).map_err(|error| error.message)?;
        return Ok(makepad_ai_services::wire::ServiceCall {
            call_id: id.into(),
            tool: tool.into(),
            args: args.to_json(),
        });
    }
    fields.push((
        if tool == "flow_todos" { "f" } else { "flow" }.into(),
        s(flow),
    ));
    let call = makepad_ai_services::wire::ServiceCall {
        call_id: id.into(),
        tool: tool.into(),
        args: Value::Obj(fields).to_json(),
    };
    // Share the real tool schemas/validators with F10; the CLI invents no API.
    cli_parse_request(&call)?;
    Ok(call)
}

/// Write a `--full` export into the lane's scratch (`<control>/exports`):
/// a bare file name, never over an existing file, never through a symlink,
/// at most 8 MiB. On unix the file is created relative to verified
/// directory handles (`openat`), so a directory swapped for a symlink
/// between the check and the creation cannot redirect the write.
pub fn cli_export(control: &Path, name: &str, body: &str) -> Result<PathBuf, String> {
    let valid = !name.is_empty()
        && name.len() <= 96
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if !valid || name.contains("..") {
        return Err("Export name must be a bare file name (letters, digits, - _ .)".into());
    }
    if body.len() > CODE_EXPORT_LIMIT {
        return Err("Export exceeds 8 MiB".into());
    }
    let file_name = if name.ends_with(".json") { name.to_owned() } else { format!("{name}.json") };
    let path = control.join("exports").join(&file_name);
    let mut file = cli_export_create(control, &file_name)?;
    file.write_all(body.as_bytes()).map_err(err)?;
    file.sync_all().map_err(err)?;
    Ok(path)
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "linux", target_os = "android"))]
fn cli_export_create(control: &Path, file_name: &str) -> Result<File, String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::os::unix::io::{AsRawFd, FromRawFd};
    // openat is variadic in C; the mode must travel as a variadic argument
    // (on Apple arm64 variadic arguments go on the stack, not in registers).
    extern "C" {
        fn openat(dirfd: std::os::raw::c_int, path: *const std::os::raw::c_char, flags: std::os::raw::c_int, ...) -> std::os::raw::c_int;
        fn mkdirat(dirfd: std::os::raw::c_int, path: *const std::os::raw::c_char, mode: u16) -> std::os::raw::c_int;
    }
    const O_WRONLY: i32 = 0x1;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const O_CREAT: i32 = 0x200;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const O_EXCL: i32 = 0x800;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const O_NOFOLLOW: i32 = 0x100;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const O_DIRECTORY: i32 = 0x100000;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const O_CLOEXEC: i32 = 0x1000000;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const O_CREAT: i32 = 0x40;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const O_EXCL: i32 = 0x80;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const O_NOFOLLOW: i32 = 0x20000;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const O_DIRECTORY: i32 = 0x10000;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const O_CLOEXEC: i32 = 0x80000;
    // the control directory: a private directory, opened without following links
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    let control_dir = options.open(control).map_err(err)?;
    let meta = control_dir.metadata().map_err(err)?;
    if !meta.is_dir() || meta.mode() & 0o077 != 0 {
        return Err("Control directory is not a private directory".into());
    }
    let exports = CString::new("exports").map_err(err)?;
    // create (or reuse) the exports directory relative to that handle
    let made = unsafe { mkdirat(control_dir.as_raw_fd(), exports.as_ptr(), 0o700u16) };
    if made != 0 {
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(format!("Cannot create the export directory: {e}"));
        }
    }
    let exports_fd = unsafe { openat(control_dir.as_raw_fd(), exports.as_ptr(), O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC) };
    if exports_fd < 0 {
        return Err(format!("The export directory cannot be opened as a directory: {}", std::io::Error::last_os_error()));
    }
    let exports_dir = unsafe { File::from_raw_fd(exports_fd) };
    let meta = exports_dir.metadata().map_err(err)?;
    if !meta.is_dir() || meta.mode() & 0o077 != 0 {
        return Err("The export directory is not a private directory".into());
    }
    let leaf = CString::new(Path::new(file_name).as_os_str().as_bytes()).map_err(err)?;
    let fd = unsafe { openat(exports_dir.as_raw_fd(), leaf.as_ptr(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600 as std::os::raw::c_uint) };
    if fd < 0 {
        let e = std::io::Error::last_os_error();
        return Err(if e.kind() == std::io::ErrorKind::AlreadyExists { "Export refuses to overwrite an existing file".into() } else { format!("Export cannot be created: {e}") });
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "linux", target_os = "android")))]
fn cli_export_create(control: &Path, file_name: &str) -> Result<File, String> {
    let exports = control.join("exports");
    if let Ok(meta) = fs::symlink_metadata(&exports) {
        if !meta.file_type().is_dir() {
            return Err("The export directory is not a directory".into());
        }
    } else {
        cli_private_dir(&exports, true)?;
    }
    let path = exports.join(file_name);
    if fs::symlink_metadata(&path).is_ok() {
        return Err("Export refuses to overwrite an existing file".into());
    }
    OpenOptions::new().write(true).create_new(true).open(&path).map_err(err)
}

// Compact terminal feedback combines an optional request scope with todo
// deltas. Reuse model parsers so IDs, text, tuples and bounds stay identical.
fn cli_parse_request(call: &makepad_ai_services::wire::ServiceCall) -> Result<Request, String> {
    if cli_code_tool(&call.tool) {
        return Err("Code tools carry their lane identity out of band; the transports dispatch them directly".into());
    }
    if call.tool == "flow_lane" {
        if call.args.len() > 16 * 1024 {
            return Err("Split request exceeds its bound".into());
        }
        let args = json::parse_depth(call.args.as_bytes(), 16).map_err(str::to_owned)?;
        return if args.get("state").and_then(Value::as_str) == Some("clear_history") {
            parse_clear_history_request(&args)
        } else {
            parse_split_request(&args)
        };
    }
    if call.tool != "flow_report" {
        return crate::iteration_tools::parse(call);
    }
    if call.args.len() > 16 * 1024 {
        return Err("Feedback callback exceeds 16 KiB".into());
    }
    let args = json::parse_depth(call.args.as_bytes(), 16).map_err(str::to_owned)?;
    let Value::Obj(fields) = &args else {
        return Err("Feedback callback must be an object".into());
    };
    if fields
        .iter()
        .any(|(name, _)| !["flow", "v", "q", "k", "u"].contains(&name.as_str()))
    {
        return Err("Feedback accepts v, optional q/k, and u".into());
    }
    let flow = args
        .get("flow")
        .and_then(Value::as_str)
        .filter(|flow| cli_identifier(flow))
        .ok_or("Invalid callback flow")?
        .to_owned();
    let todo_args = json::obj(vec![
        ("f", s(&flow)),
        ("v", args.get("v").cloned().ok_or("Feedback requires v")?),
        ("u", args.get("u").cloned().ok_or("Feedback requires u")?),
    ]);
    let todos = iteration::parse(&makepad_ai_services::wire::ServiceCall {
        call_id: call.call_id.clone(),
        tool: "flow_todos".into(),
        args: todo_args.to_json(),
    })?;
    let request = if let Some(text) = args.get("q") {
        let mut fields = vec![("flow", s(&flow)), ("text", text.clone())];
        if let Some(id) = args.get("k") {
            fields.push(("id", id.clone()));
        }
        Some(iteration::parse(&makepad_ai_services::wire::ServiceCall {
            call_id: call.call_id.clone(),
            tool: "flow_requirement".into(),
            args: json::obj(fields).to_json(),
        })?)
    } else {
        if args.get("k").is_some() {
            return Err("k requires the amended scope q".into());
        }
        None
    };
    Ok(Request::FeedbackReport {
        flow,
        requirement: request,
        todos,
    })
}

impl Host {
    fn feedback_report(
        &mut self,
        flow: &str,
        requirement: Option<FlowCommand>,
        todos: FlowCommand,
    ) -> Result<Value, String> {
        self.require_active_lane(flow)?;
        if !matches!(&todos, FlowCommand::Todos { flow: owner, .. } if owner == flow)
            || requirement.as_ref().is_some_and(|command| !matches!(command, FlowCommand::Requirement { flow: owner, .. } if owner == flow)) {
            return Err("Feedback commands do not belong to this lane".into());
        }
        let previous = self.engine.clone();
        let mut next = previous.clone();
        let mut events = Vec::new();
        let mut effects = Vec::new();
        let timestamp = now();
        if let Some(requirement) = requirement {
            let transition = next.apply(requirement, timestamp)?;
            events.extend(transition.events);
            effects.extend(transition.effects);
        }
        let transition = next.apply(todos, timestamp)?;
        events.extend(transition.events);
        effects.extend(transition.effects);
        let state = next.flows.get(flow).ok_or("Unknown callback lane")?;
        let result = json::obj(vec![
            ("v", Value::Int(state.todos_revision as i64)),
            ("r", Value::Int(state.requirements_revision as i64)),
        ]);
        // Publish the two model events in one atomic state write. A stale todo
        // version, invalid scope or persistence failure leaves both untouched.
        self.engine = next;
        self.transition(
            previous,
            Transition {
                result,
                effects,
                events,
            },
        )
    }
}

#[cfg(windows)]
fn cli_private_dir(path: &Path, create: bool) -> Result<(), String> {
    makepad_screen::protocol::private_directory(path, create)
}

#[cfg(windows)]
fn cli_read(path: &Path, limit: usize) -> Result<String, String> {
    String::from_utf8(makepad_screen::protocol::read_private(path, limit)?)
        .map_err(|_| "Control input must be valid UTF-8".into())
}

#[cfg(windows)]
fn cli_private_read(path: &Path, limit: usize) -> Result<String, String> {
    cli_read(path, limit)
}

#[cfg(windows)]
fn cli_publish_lane_binding(path: &Path, text: &str) -> Result<(), String> {
    if text.len() > 8192 {
        return Err("Screen lane binding exceeds 8 KiB".into());
    }
    cli_private_dir(
        path.parent().ok_or("Screen lane binding has no parent")?,
        false,
    )?;
    match fs::symlink_metadata(path) {
        Ok(_) => {
            if cli_private_read(path, 8192)? == text {
                return Ok(());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(err(error)),
    }
    makepad_screen::protocol::write_private(path, text.as_bytes())
}

#[cfg(windows)]
fn cli_publish(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing control directory")?;
    cli_private_dir(parent, false)?;
    if bytes.len() > CLI_REPLY_LIMIT {
        return Err("Control publication exceeds its bound".into());
    }
    let temporary = parent.join(format!(".tmp-{}", cli_request_id()));
    makepad_screen::protocol::create_private(&temporary, bytes)?;
    // A completed private file is linked into place exactly once. Never
    // replace another caller's request or a durable outcome under the same ID.
    let result = fs::hard_link(&temporary, path).map_err(err);
    let removed = fs::remove_file(&temporary).map_err(err);
    result.and(removed)
}

#[cfg(not(windows))]
fn cli_private_dir(path: &Path, create: bool) -> Result<(), String> {
    if create {
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        match builder.create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    let meta = fs::symlink_metadata(path).map_err(err)?;
    if !meta.file_type().is_dir() {
        return Err("Control paths must be real directories".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        extern "C" {
            fn geteuid() -> u32;
        }
        if meta.uid() != unsafe { geteuid() } || meta.mode() & 0o077 != 0 {
            return Err("Control directories must be owned by this user with mode 0700".into());
        }
    }
    Ok(())
}

/// Worker-only setup. Export the returned absolute directory to that flow's CLI.
pub fn cli_control_dir(directory: &Path, flow: &str) -> Result<PathBuf, String> {
    if !cli_identifier(flow) {
        return Err("Invalid flow identity".into());
    }
    let root = directory.canonicalize().map_err(err)?.join("control");
    cli_private_dir(&root, true)?;
    let control = root.join(flow);
    cli_private_dir(&control, true)?;
    for name in ["requests", "processing", "receipts", "replies"] {
        cli_private_dir(&control.join(name), true)?;
    }
    Ok(control)
}

fn cli_validate_control(control: &Path, flow: &str) -> Result<PathBuf, String> {
    if !control.is_absolute()
        || !cli_identifier(flow)
        || control.file_name().and_then(|v| v.to_str()) != Some(flow)
        || control
            .parent()
            .and_then(Path::file_name)
            .and_then(|v| v.to_str())
            != Some("control")
    {
        return Err("Control directory does not match the assigned flow".into());
    }
    cli_private_dir(control.parent().unwrap(), false)?;
    cli_private_dir(control, false)?;
    for name in ["requests", "processing", "receipts", "replies"] {
        cli_private_dir(&control.join(name), false)?;
    }
    control.canonicalize().map_err(err)
}

fn cli_screen_session_id(origin: &str) -> Result<String, String> {
    if !cli_identifier(origin) {
        return Err("Invalid terminal origin".into());
    }
    let id = makepad_widgets::LiveId::from_str(&format!("studio-flow-terminal:{origin}"));
    Ok(format!("term-{:016x}", id.0))
}

#[cfg(not(windows))]
fn cli_private_read(path: &Path, limit: usize) -> Result<String, String> {
    let before = fs::symlink_metadata(path).map_err(err)?;
    if !before.is_file() || before.len() > limit as u64 {
        return Err("Lane discovery must be a bounded regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        if before.uid() != unsafe { geteuid() } || before.mode() & 0o777 != 0o600 {
            return Err("Lane discovery must be owned by this user with mode 0600".into());
        }
    }
    let text = cli_read(path, limit)?;
    let after = fs::symlink_metadata(path).map_err(err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.uid() != after.uid()
            || before.mode() != after.mode()
        {
            return Err(
                "Lane discovery changed while reading; retry against its current owner".into(),
            );
        }
    }
    if !after.is_file() || after.len() > limit as u64 {
        return Err("Lane discovery changed while reading".into());
    }
    Ok(text)
}

/// Resolve every invocation through the stable daemon identity. A mirrored
/// presentation cannot retarget the session by changing inherited flow vars.
pub fn cli_environment_scope() -> Result<(String, PathBuf), String> {
    let session = std::env::var_os("MAKEPAD_SCREEN_SESSION");
    let state = std::env::var_os("MAKEPAD_SCREEN_STATE_DIR");
    if session.is_some() || state.is_some() {
        let session = session
            .and_then(|value| value.into_string().ok())
            .ok_or("Managed screen session identity is missing or not UTF-8")?;
        if session.len() != 21
            || !session.starts_with("term-")
            || !session.as_bytes()[5..]
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        {
            return Err("This screen session is not a Studio terminal identity".into());
        }
        let state = PathBuf::from(state.ok_or("Managed screen state directory is missing")?);
        if !state.is_absolute() {
            return Err("Managed screen state directory must be absolute".into());
        }
        cli_private_dir(&state, false)?;
        let state = state.canonicalize().map_err(err)?;
        if state.file_name().and_then(|name| name.to_str()) != Some("agent_sessions") {
            return Err("Managed screen state directory is outside Studio agent_sessions".into());
        }
        let path = state.join(format!("{session}.lane.json"));
        let text = cli_private_read(&path, 8192)
            .map_err(|error| format!("Current screen lane binding is unavailable: {error}; no legacy lane fallback was used"))?;
        let value = json::parse_depth(text.as_bytes(), 4).map_err(str::to_owned)?;
        let Value::Obj(fields) = &value else {
            return Err("Invalid screen lane binding".into());
        };
        if fields.len() != 4
            || fields.iter().any(|(key, _)| {
                !["version", "session_id", "flow", "control_dir"].contains(&key.as_str())
            })
            || value.get("version").and_then(Value::as_u64) != Some(1)
            || value.get("session_id").and_then(Value::as_str) != Some(session.as_str())
        {
            return Err("Screen lane binding does not match this session".into());
        }
        let flow = value
            .get("flow")
            .and_then(Value::as_str)
            .filter(|flow| cli_identifier(flow))
            .ok_or("Screen lane binding has an invalid owner")?
            .to_owned();
        let control = PathBuf::from(
            value
                .get("control_dir")
                .and_then(Value::as_str)
                .ok_or("Screen lane binding has no control directory")?,
        );
        let control = cli_validate_control(&control, &flow)?;
        let expected = state
            .parent()
            .ok_or("Screen state has no Studio parent")?
            .join("iterations")
            .join("control")
            .join(&flow)
            .canonicalize()
            .map_err(err)?;
        if control != expected {
            return Err("Screen lane binding crosses its Studio state scope".into());
        }
        return Ok((flow, control));
    }
    let flow = std::env::var("MAKEPAD_STUDIO_FLOW_ID")
        .map_err(|_| "This terminal has no Studio flow identity")?;
    let control = PathBuf::from(
        std::env::var_os("MAKEPAD_STUDIO_CONTROL_DIR")
            .ok_or("This terminal has no Studio control directory")?,
    );
    let control = cli_validate_control(&control, &flow)?;
    Ok((flow, control))
}

/// This is a local capability: print only for an explicit --url invocation.
pub fn cli_capability_url(control: &Path, flow: &str) -> Result<String, String> {
    let control = cli_validate_control(control, flow)?;
    let raw = cli_private_read(&control.join("http-url"), 256)?;
    let url = raw.trim();
    let parts = url
        .strip_prefix("http://127.0.0.1:")
        .and_then(|value| value.split_once("/v1/"));
    if !parts.is_some_and(|(port, token)| {
        !port.is_empty()
            && port.bytes().all(|byte| byte.is_ascii_digit())
            && port.parse::<u16>().is_ok_and(|port| port > 0)
            && token.len() == 64
            && token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err("Lane discovery contains an invalid loopback capability URL".into());
    }
    Ok(url.to_owned())
}

// Worker-only, atomic, private publication. Repeated polling does not rewrite
// an unchanged binding or leave per-poll temporary files behind.
#[cfg(not(windows))]
fn cli_publish_lane_binding(path: &Path, text: &str) -> Result<(), String> {
    if text.len() > 8192 {
        return Err("Screen lane binding exceeds 8 KiB".into());
    }
    let parent = path.parent().ok_or("Screen lane binding has no parent")?;
    cli_private_dir(parent, false)?;
    match fs::symlink_metadata(path) {
        Ok(_) => {
            if cli_private_read(path, 8192)? == text {
                return Ok(());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(err(error)),
    }
    let temporary = parent.join(format!(".lane-{}", cli_request_id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(err)?;
    let result = (|| {
        file.write_all(text.as_bytes()).map_err(err)?;
        file.sync_all().map_err(err)?;
        fs::rename(&temporary, path).map_err(err)?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(err)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(not(windows))]
fn cli_read(path: &Path, limit: usize) -> Result<String, String> {
    let before = fs::symlink_metadata(path).map_err(err)?;
    if !before.file_type().is_file() || before.len() > limit as u64 {
        return Err("Control input must be a bounded regular file".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    // Refuse a symlink replacement, and never wait if a regular input was
    // replaced with a FIFO between metadata inspection and open.
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "linux",
        target_os = "android"
    ))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        options.custom_flags(0x100 | 0x4); // O_NOFOLLOW | O_NONBLOCK
        #[cfg(any(target_os = "linux", target_os = "android"))]
        options.custom_flags(0x20000 | 0x800); // O_NOFOLLOW | O_NONBLOCK
    }
    let file = options.open(path).map_err(err)?;
    let opened = file.metadata().map_err(err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Atomic publication briefly has the staging and final hard links.
        // Identity must remain exact; link count is not a readiness signal.
        if before.dev() != opened.dev() || before.ino() != opened.ino() {
            return Err("Control input changed identity while opening".into());
        }
    }
    if !opened.is_file() || opened.len() > limit as u64 {
        return Err("Control input grew beyond its limit".into());
    }
    let mut text = String::new();
    file.take(limit as u64 + 1)
        .read_to_string(&mut text)
        .map_err(err)?;
    if text.len() > limit {
        return Err("Control input grew beyond its limit".into());
    }
    Ok(text)
}

/// Publish once without replacing an existing receipt or reply.
#[cfg(not(windows))]
fn cli_publish(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing control directory")?;
    let temp = parent.join(format!(".tmp-{}", cli_request_id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(err)?;
    let result = (|| {
        file.write_all(bytes).map_err(err)?;
        file.sync_all().map_err(err)?;
        fs::hard_link(&temp, path).map_err(err)?;
        fs::remove_file(&temp).map_err(err)?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(err)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

pub fn cli_request_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "cli-{nanos}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn cli_budget(control: &Path) -> Result<(), String> {
    for (name, count) in [
        ("requests", 64),
        ("processing", 64),
        ("receipts", CLI_HISTORY_LIMIT - 1),
        ("replies", CLI_HISTORY_LIMIT - 1),
    ] {
        let mut bytes = 0u64;
        for (index, entry) in fs::read_dir(control.join(name)).map_err(err)?.enumerate() {
            if index >= count {
                return Err(format!(
                    "Flow control {name} budget is full; inspect retained receipts before cleanup"
                ));
            }
            let entry = entry.map_err(err)?;
            let meta = fs::symlink_metadata(entry.path()).map_err(err)?;
            if !meta.file_type().is_file() {
                return Err("Unexpected non-file in flow control spool".into());
            }
            bytes = bytes.saturating_add(meta.len());
            if name == "replies" && bytes > CLI_REPLY_BYTES - CLI_REPLY_LIMIT as u64 {
                return Err("Flow control cannot reserve an 8 MiB reply within its 32 MiB budget; archive reviewed replies before cleanup".into());
            }
        }
    }
    Ok(())
}

fn cli_canonical(value: &mut Value) {
    match value {
        Value::Obj(fields) => {
            for (_, value) in fields.iter_mut() {
                cli_canonical(value);
            }
            fields.sort_by(|left, right| left.0.cmp(&right.0));
        }
        Value::Arr(values) => {
            for value in values {
                cli_canonical(value);
            }
        }
        _ => {}
    }
}

// A request ID identifies one exact normalized tool call across both HTTP
// and file transport. Old receipts without arguments remain non-replayable.
fn cli_existing_request(
    control: &Path,
    flow: &str,
    call: &makepad_ai_services::wire::ServiceCall,
) -> Result<bool, String> {
    let mut expected = json::parse_depth(call.args.as_bytes(), 16).map_err(str::to_owned)?;
    cli_canonical(&mut expected);
    for directory in ["receipts", "processing", "requests"] {
        let path = control
            .join(directory)
            .join(format!("{}.json", call.call_id));
        if !path.try_exists().map_err(err)? {
            continue;
        }
        let saved = json::parse_depth(cli_read(&path, CLI_REQUEST_LIMIT * 2)?.as_bytes(), 24)
            .map_err(str::to_owned)?;
        let envelope = saved.get("request").unwrap_or(&saved);
        if envelope.get("request_id").and_then(Value::as_str) != Some(call.call_id.as_str())
            || envelope.get("flow_id").and_then(Value::as_str) != Some(flow)
        {
            return Err("Request ID already exists without matching durable call identity; inspect its result before continuing".into());
        }
        let tool = envelope.get("tool").and_then(Value::as_str).ok_or("Request ID was previously claimed without retained arguments; inspect its result, never replay it")?;
        let args = envelope
            .get("args")
            .cloned()
            .ok_or("Request ID has no retained arguments; inspect its result")?;
        let prior = cli_service_call(&call.call_id, flow, tool, args)?;
        let mut actual = json::parse_depth(prior.args.as_bytes(), 16).map_err(str::to_owned)?;
        cli_canonical(&mut actual);
        if prior.tool != call.tool || actual != expected {
            return Err("Request ID already belongs to a different tool or arguments".into());
        }
        return Ok(true);
    }
    if control
        .join("replies")
        .join(format!("{}.json", call.call_id))
        .try_exists()
        .map_err(err)?
    {
        return Err("Request ID has a retained result without its original call; inspect the result instead of replaying".into());
    }
    Ok(false)
}

pub fn cli_submit(
    control: &Path,
    flow: &str,
    call: &makepad_ai_services::wire::ServiceCall,
) -> Result<PathBuf, String> {
    let control = cli_validate_control(control, flow)?;
    let args = json::parse_depth(call.args.as_bytes(), 16).map_err(str::to_owned)?;
    let call = cli_service_call(&call.call_id, flow, &call.tool, args)?;
    let reply = control
        .join("replies")
        .join(format!("{}.json", call.call_id));
    if cli_existing_request(&control, flow, &call)? {
        return Ok(reply);
    }
    cli_budget(&control)?;
    let args = json::parse_depth(call.args.as_bytes(), 16).map_err(str::to_owned)?;
    let envelope = json::obj(vec![
        ("schema_version", Value::Int(1)),
        ("request_id", s(&call.call_id)),
        ("flow_id", s(flow)),
        ("tool", s(&call.tool)),
        ("args", args),
        ("created_at_ms", Value::Int(now() as i64)),
    ]);
    let body = envelope.to_json();
    if body.len() > CLI_REQUEST_LIMIT {
        return Err("CLI request exceeds 64 KiB".into());
    }
    if let Err(error) = cli_publish(
        &control
            .join("requests")
            .join(format!("{}.json", call.call_id)),
        body.as_bytes(),
    ) {
        // Another transport may have published the same ID after our check.
        if !cli_existing_request(&control, flow, &call)? {
            return Err(error);
        }
    }
    Ok(reply)
}

pub fn cli_poll_reply(path: &Path, flow: &str, id: &str) -> Result<Option<Value>, String> {
    if !path.try_exists().map_err(err)? {
        return Ok(None);
    }
    let body = cli_read(path, CLI_REPLY_LIMIT)?;
    let value = json::parse_depth(body.as_bytes(), 32).map_err(str::to_owned)?;
    if value.get("request_id").and_then(Value::as_str) != Some(id)
        || value.get("flow_id").and_then(Value::as_str) != Some(flow)
    {
        return Err("Control reply identity does not match the request".into());
    }
    Ok(Some(value))
}

fn cli_answer(
    control: &Path,
    flow: &str,
    id: &str,
    status: &str,
    result: Result<Value, String>,
) -> Result<(), String> {
    let mut fields = vec![
        ("schema_version", Value::Int(1)),
        ("request_id", s(id)),
        ("flow_id", s(flow)),
        ("status", s(status)),
    ];
    match result {
        Ok(value) => fields.push(("result", value)),
        Err(error) => fields.push(("error", s(error))),
    }
    let mut body = json::obj(fields).to_json();
    if body.len() > CLI_REPLY_LIMIT {
        body = json::obj(vec![("schema_version", Value::Int(1)), ("request_id", s(id)), ("flow_id", s(flow)),
            ("status", s("uncertain")), ("error", s("Tool completed but its reply exceeded 8 MiB. Inspect the flow; do not repeat a mutation blindly."))]).to_json();
    }
    cli_publish(
        &control.join("replies").join(format!("{id}.json")),
        body.as_bytes(),
    )
}

fn cli_pending(directory: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).map_err(err)?.take(65) {
        let entry = entry.map_err(err)?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .filter(|id| cli_identifier(id))
        else {
            continue;
        };
        files.push((id.to_owned(), entry.path()));
    }
    if files.len() > 64 {
        return Err("Control request queue exceeds 64 files".into());
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// Any processing file found at a poll belongs to an interrupted attempt
/// (a Studio restart between claim and reply). Its receipt is preserved and
/// its reply says so; its contents are never executed again.
fn cli_recover_interrupted(control: &Path, flow: &str, id: &str, path: &Path) -> Result<(), String> {
    let receipt = control.join("receipts").join(format!("{id}.json"));
    if !receipt.try_exists().map_err(err)? {
        let recovered = json::obj(vec![
            ("request_id", s(id)),
            ("flow_id", s(flow)),
            ("interrupted", Value::Bool(true)),
            (
                "request",
                cli_read(path, CLI_REQUEST_LIMIT)
                    .ok()
                    .and_then(|raw| json::parse_depth(raw.as_bytes(), 20).ok())
                    .unwrap_or(Value::Null),
            ),
            ("claimed_at_ms", Value::Int(now() as i64)),
        ])
        .to_json();
        cli_publish(&receipt, recovered.as_bytes())?;
    }
    if cli_poll_reply(&control.join("replies").join(format!("{id}.json")), flow, id)?.is_none() {
        cli_answer(control, flow, id, "uncertain", Err("Studio was interrupted after claiming this request. Its effects may already exist. Run studio-flow inspect before deciding the next action; this request will not be replayed.".into()))?;
    }
    fs::remove_file(path).map_err(err)
}

impl Host {
    /// Non-UI, bounded work shared by terminal and loopback HTTP requests.
    fn poll_cli(&mut self) {
        let mut flows: Vec<_> = self
            .engine
            .flows
            .values()
            .filter(|flow| {
                flow.successor.is_none() && flow.lifecycle != iteration::FlowLifecycle::Archived
            })
            .take(4)
            .map(|flow| flow.id.clone())
            .collect();
        let aliases: Vec<_> = self
            .engine
            .flows
            .values()
            .filter(|flow| flow.successor.is_some())
            .map(|flow| flow.id.clone())
            .collect();
        if !aliases.is_empty() {
            flows.push(aliases[self.cli_archive_cursor % aliases.len()].clone());
        }
        let archived: Vec<_> = self
            .engine
            .flows
            .values()
            .filter(|flow| {
                flow.successor.is_none() && flow.lifecycle == iteration::FlowLifecycle::Archived
            })
            .map(|flow| flow.id.clone())
            .collect();
        if !archived.is_empty() {
            flows.push(archived[self.cli_archive_cursor % archived.len()].clone());
        }
        self.cli_archive_cursor = self.cli_archive_cursor.wrapping_add(1);
        for flow in flows {
            // One busy agent must not consume another flow's polling budget.
            let mut remaining = 2;
            let result = (|| {
                let control = cli_control_dir(&self.directory, &flow)?;
                for (id, path) in cli_pending(&control.join("processing"))? {
                    // A deferred code call keeps its claimed file until its
                    // answer is published; it is in flight, not interrupted.
                    if self.code.deferred.contains_key(&(flow.clone(), id.clone())) {
                        continue;
                    }
                    if remaining == 0 {
                        break;
                    }
                    remaining -= 1;
                    cli_recover_interrupted(&control, &flow, &id, &path)?;
                }
                for (id, path) in cli_pending(&control.join("requests"))? {
                    if remaining == 0 {
                        break;
                    }
                    remaining -= 1;
                    let reply = control.join("replies").join(format!("{id}.json"));
                    if cli_poll_reply(&reply, &flow, &id)?.is_some() {
                        fs::remove_file(path).map_err(err)?;
                        continue;
                    }
                    cli_budget(&control)?;
                    let claimed = control.join("processing").join(format!("{id}.json"));
                    if claimed.try_exists().map_err(err)? {
                        continue;
                    }
                    fs::rename(&path, &claimed).map_err(err)?;
                    #[cfg(unix)]
                    File::open(control.join("processing"))
                        .and_then(|directory| directory.sync_all())
                        .map_err(err)?;
                    let receipt = control.join("receipts").join(format!("{id}.json"));
                    if receipt.try_exists().map_err(err)? {
                        cli_answer(&control, &flow, &id, "uncertain", Err("This request ID was already claimed. Inspect the flow; it will not be executed again.".into()))?;
                        fs::remove_file(claimed).map_err(err)?;
                        continue;
                    }
                    let receipt_body = json::obj(vec![
                        (
                            "request",
                            cli_read(&claimed, CLI_REQUEST_LIMIT)
                                .ok()
                                .and_then(|raw| json::parse_depth(raw.as_bytes(), 20).ok())
                                .unwrap_or(Value::Null),
                        ),
                        ("request_id", s(&id)),
                        ("flow_id", s(&flow)),
                        ("claimed_at_ms", Value::Int(now() as i64)),
                    ])
                    .to_json();
                    cli_publish(&receipt, receipt_body.as_bytes())?;
                    let scoped = (|| {
                        let raw = cli_read(&claimed, CLI_REQUEST_LIMIT)?;
                        let envelope =
                            json::parse_depth(raw.as_bytes(), 20).map_err(str::to_owned)?;
                        if envelope.get("schema_version").and_then(Value::as_i64) != Some(1)
                            || envelope.get("request_id").and_then(Value::as_str)
                                != Some(id.as_str())
                            || envelope.get("flow_id").and_then(Value::as_str)
                                != Some(flow.as_str())
                        {
                            return Err("Invalid CLI request envelope or flow identity".into());
                        }
                        let tool = envelope
                            .get("tool")
                            .and_then(Value::as_str)
                            .ok_or("Missing CLI tool")?;
                        let args = envelope.get("args").ok_or("Missing CLI arguments")?.clone();
                        let call = cli_service_call(&id, &flow, tool, args)?;
                        // Receipts stay in their original capability namespace.
                        // Only execution follows the durable successor link.
                        let successor = self.engine.resolve_active_flow(&flow)?.to_owned();
                        let call = if successor != flow {
                            let Value::Obj(mut fields) =
                                json::parse(call.args.as_bytes()).map_err(str::to_owned)?
                            else {
                                return Err("Invalid scoped arguments".into());
                            };
                            fields.retain(|(key, _)| !matches!(key.as_str(), "flow" | "f"));
                            cli_service_call(&id, &successor, tool, Value::Obj(fields))?
                        } else {
                            call
                        };
                        Ok::<_, String>((call, successor))
                    })();
                    let result = match scoped {
                        Ok((call, successor)) if cli_code_tool(&call.tool) => {
                            // Code calls answer with an envelope even when they
                            // fail; a deferred one keeps its claimed file and
                            // answers from poll_code.
                            let args = json::parse_depth(call.args.as_bytes(), 16)
                                .map_err(str::to_owned)?;
                            match self.code_call(&successor, &flow, &id, &control, Some(claimed.clone()), &call.tool, &args) {
                                CodeOutcome::Deferred => continue,
                                CodeOutcome::Answered(envelope) => {
                                    let status = if envelope.is_error() { "error" } else { "ok" };
                                    cli_answer(&control, &flow, &id, status, Ok(envelope.json()))?;
                                    fs::remove_file(claimed).map_err(err)?;
                                    continue;
                                }
                            }
                        }
                        Ok((call, _)) => cli_parse_request(&call).and_then(|request| self.request(request)),
                        Err(error) => Err(error),
                    };
                    let status = if result.is_ok() { "ok" } else { "error" };
                    cli_answer(&control, &flow, &id, status, result)?;
                    fs::remove_file(claimed).map_err(err)?;
                }
                Ok::<_, String>(())
            })();
            if let Err(error) = result {
                let note = format!("{flow}: terminal tool bridge: {error}");
                if self.note != note {
                    self.note = note;
                    self.changed = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn scratch(name: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("studio-cli-{}-{}-{}", name, std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        dir
    }

    fn args(text: &str) -> Value {
        json::parse(text.as_bytes()).unwrap()
    }

    #[test]
    fn code_tools_are_bridged_without_a_flow_argument_and_view_tools_are_not() {
        assert_eq!(cli_tool_name("code_brief").unwrap(), "code_brief");
        assert!(cli_tool_name("atlas_show").is_err(), "view Act tools are never bridged");
        assert!(cli_tool_name("code_callers").is_err(), "gated tools are not advertised");
        let call = cli_service_call("req-1", "lane-a", "code_brief", args(r#"{"scope":"widgets::dock"}"#)).unwrap();
        assert_eq!(call.args, r#"{"scope":"widgets::dock"}"#, "no flow is injected into a code call");
        assert!(cli_service_call("req-2", "lane-a", "code_brief", args(r#"{"scope":"x","flow":"lane-b"}"#)).is_err(), "another lane cannot be addressed");
        assert!(cli_service_call("req-3", "lane-a", "code_brief", args(r#"{"scope":"x","bogus":1}"#)).is_err(), "unknown fields are refused at the transport");
        assert!(cli_service_call("req-4", "lane-a", "code_impact", args(r#"{"depth":2}"#)).is_err());
        assert!(cli_parse_request(&call).is_err(), "code calls never reach the flow request parser");
    }

    #[test]
    fn receipts_round_trip_and_survive_a_simulated_restart() {
        let state = scratch("receipts");
        let flow = "lane-a";
        let control = cli_control_dir(&state, flow).unwrap();
        let call = cli_service_call("req-1", flow, "code_size", args(r#"{"scope":"atlasfix"}"#)).unwrap();
        let reply = cli_submit(&control, flow, &call).unwrap();
        assert!(cli_poll_reply(&reply, flow, "req-1").unwrap().is_none(), "nothing answered yet");
        // an identical repeat reuses the request; different arguments conflict
        assert_eq!(cli_submit(&control, flow, &call).unwrap(), reply);
        let other = cli_service_call("req-1", flow, "code_size", args(r#"{"scope":"other"}"#)).unwrap();
        let conflict = cli_submit(&control, flow, &other).unwrap_err();
        assert!(conflict.contains("different tool or arguments"), "{conflict}");
        let tool = cli_service_call("req-1", flow, "code_brief", args(r#"{"scope":"atlasfix"}"#)).unwrap();
        assert!(cli_submit(&control, flow, &tool).is_err());
        // the host claims it: requests -> processing, receipt published
        let pending = cli_pending(&control.join("requests")).unwrap();
        assert_eq!(pending.len(), 1);
        let claimed = control.join("processing").join("req-1.json");
        fs::rename(&pending[0].1, &claimed).unwrap();
        let request = json::parse(cli_read(&claimed, CLI_REQUEST_LIMIT).unwrap().as_bytes()).unwrap();
        let receipt = json::obj(vec![("request", request), ("request_id", s("req-1")), ("flow_id", s(flow)), ("claimed_at_ms", Value::Int(now() as i64))]);
        cli_publish(&control.join("receipts").join("req-1.json"), receipt.to_json().as_bytes()).unwrap();
        // ... and answers with an envelope
        let envelope = json::obj(vec![("request_id", s("req-1")), ("tool", s("code_size")), ("rows", Value::Arr(vec![]))]);
        cli_answer(&control, flow, "req-1", "ok", Ok(envelope.clone())).unwrap();
        fs::remove_file(&claimed).unwrap();
        let answered = cli_poll_reply(&reply, flow, "req-1").unwrap().expect("reply published");
        assert_eq!(answered.get("status").and_then(Value::as_str), Some("ok"));
        assert_eq!(answered.get("result"), Some(&envelope));
        // a repeat after the answer returns the retained reply without a new request
        assert_eq!(cli_submit(&control, flow, &call).unwrap(), reply);
        assert!(cli_pending(&control.join("requests")).unwrap().is_empty());
        // simulated restart: a second request was claimed but never answered
        let second = cli_service_call("req-2", flow, "code_size", args(r#"{"scope":"atlasfix"}"#)).unwrap();
        let reply2 = cli_submit(&control, flow, &second).unwrap();
        let claimed2 = control.join("processing").join("req-2.json");
        fs::rename(control.join("requests").join("req-2.json"), &claimed2).unwrap();
        cli_recover_interrupted(&control, flow, "req-2", &claimed2).unwrap();
        let recovered = cli_poll_reply(&reply2, flow, "req-2").unwrap().expect("uncertain reply");
        assert_eq!(recovered.get("status").and_then(Value::as_str), Some("uncertain"));
        assert!(!claimed2.exists(), "the interrupted claim is retired");
        let receipt = cli_read(&control.join("receipts").join("req-2.json"), CLI_REQUEST_LIMIT).unwrap();
        assert!(receipt.contains("\"interrupted\":true"), "{receipt}");
        // the id is never replayed: a repeat returns the uncertain reply
        assert_eq!(cli_submit(&control, flow, &second).unwrap(), reply2);
        let _ = fs::remove_dir_all(&state);
    }

    #[test]
    fn exports_are_bounded_named_and_never_replace_or_follow_links() {
        let state = scratch("exports");
        let control = cli_control_dir(&state, "lane-a").unwrap();
        assert!(cli_export(&control, "../escape", "{}").is_err());
        assert!(cli_export(&control, "a/b", "{}").is_err());
        assert!(cli_export(&control, ".hidden", "{}").is_err());
        assert!(cli_export(&control, "", "{}").is_err());
        let path = cli_export(&control, "brief", "{\"rows\":[]}").unwrap();
        assert_eq!(path, control.join("exports").join("brief.json"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"rows\":[]}");
        assert!(cli_export(&control, "brief.json", "{}").is_err(), "no overwrite");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(state.join("elsewhere.json"), control.join("exports").join("link.json")).unwrap();
            assert!(cli_export(&control, "link", "{}").is_err(), "a symlink is never followed");
            assert!(!state.join("elsewhere.json").exists());
        }
        let huge = "x".repeat(CODE_EXPORT_LIMIT + 1);
        assert!(cli_export(&control, "huge", &huge).is_err());
        let _ = fs::remove_dir_all(&state);
    }
}
