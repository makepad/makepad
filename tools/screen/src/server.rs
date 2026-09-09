use crate::{
    protocol::*,
    snapshot::Projection,
    terminal::HostedTerminal,
    unix::{self, Pty},
};
use makepad_strict_json::{self as json, Value};
use std::{
    collections::VecDeque,
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

const MAX_CLIENTS: usize = 16;
const MAX_QUEUE: usize = 16 * 1024 * 1024 + 64 * 1024;
const MAX_INPUT: usize = 256 * 1024;
#[derive(Clone)]
pub struct StartOptions {
    pub state_dir: PathBuf,
    pub session_id: String,
    pub cwd: PathBuf,
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cols: u16,
    pub rows: u16,
    pub theme: crate::theme::Theme,
}
impl StartOptions {
    fn validate(&mut self) -> Result<(), String> {
        if !self.cwd.is_absolute() || !self.cwd.is_dir() {
            return Err("Screen cwd must be an existing absolute directory".into());
        }
        self.cwd = self.cwd.canonicalize().map_err(error)?;
        if self.cwd.to_str().is_none()
            || self.program.is_empty()
            || self.args.len() > 256
            || self.program.as_bytes().len()
                + self
                    .args
                    .iter()
                    .map(|arg| arg.as_bytes().len())
                    .sum::<usize>()
                > 64 * 1024
        {
            return Err("Screen command or cwd exceeds its supported bounds".into());
        }
        if !(2..=400).contains(&self.cols) || !(2..=200).contains(&self.rows) {
            return Err("Screen dimensions exceed 400 columns by 200 rows".into());
        }
        Ok(())
    }
    fn command_hash(&self) -> String {
        let mut bytes = Vec::new();
        for part in std::iter::once(self.cwd.as_os_str())
            .chain(std::iter::once(self.program.as_os_str()))
            .chain(self.args.iter().map(OsString::as_os_str))
        {
            bytes.extend_from_slice(&(part.as_bytes().len() as u64).to_be_bytes());
            bytes.extend_from_slice(part.as_bytes());
        }
        crate::digest::hex(&crate::digest::sha256(&bytes))
    }
}
fn claim(location: &SessionLocation) -> Result<Option<Value>, String> {
    match fs::symlink_metadata(&location.claim_path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(error(e)),
        Ok(_) => {
            let value = parse_object(&read_private(&location.claim_path, 16 * 1024)?)?;
            if value.get("version").and_then(Value::as_u64) != Some(VERSION)
                || value.get("session_id").and_then(Value::as_str) != Some(&location.session_id)
                || value.get("state_dir").and_then(Value::as_str) != location.state_dir.to_str()
                || !value
                    .get("instance")
                    .and_then(Value::as_str)
                    .is_some_and(|v| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(
                    "Screen claim identity is corrupt or belongs to another session".into(),
                );
            }
            Ok(Some(value))
        }
    }
}
fn stopped_status(session: &str, previous: Option<&Value>) -> Value {
    json::obj(vec![
        ("running", Value::Bool(false)),
        (
            "title",
            previous
                .and_then(|v| v.get("title"))
                .cloned()
                .unwrap_or(Value::Null),
        ),
        ("version", Value::Int(VERSION as i64)),
        ("session_id", json::s(session)),
        ("pid", Value::Null),
        ("child_pid", Value::Null),
        ("cols", Value::Int(0)),
        ("rows", Value::Int(0)),
        ("clients", Value::Int(0)),
        (
            "cwd",
            previous
                .and_then(|v| v.get("cwd"))
                .cloned()
                .unwrap_or(Value::Null),
        ),
        (
            "program",
            previous
                .and_then(|v| v.get("program"))
                .cloned()
                .unwrap_or(Value::Null),
        ),
    ])
}
fn running_status(location: &SessionLocation, previous: &Value) -> Result<Value, String> {
    let frame = exchange(location, STATUS, b"{}", Duration::from_secs(2))?;
    if frame.kind == ERROR {
        return Err(String::from_utf8_lossy(&frame.payload).into_owned());
    }
    if frame.kind != STATUS_REPLY {
        return Err("Invalid screen status frame".into());
    }
    let mut value = parse_object(&frame.payload)?;
    if value.get("running").and_then(Value::as_bool) != Some(true)
        || value.get("version").and_then(Value::as_u64) != Some(VERSION)
        || value.get("session_id").and_then(Value::as_str) != Some(&location.session_id)
        || value.get("instance") != previous.get("instance")
        || value
            .get("pid")
            .and_then(Value::as_u64)
            .is_none_or(|pid| pid == 0)
    {
        return Err("Screen endpoint identity differs from its saved claim".into());
    }
    if value.get("title").is_none() {
        // Existing version-1 hosts already project their OSC title. Read it
        // without resizing, stopping or replacing their running session.
        let hello = json::obj(vec![
            ("version", Value::Int(VERSION as i64)),
            ("session_id", json::s(&location.session_id)),
            ("cols", Value::Int(80)),
            ("rows", Value::Int(24)),
            ("read_only", Value::Bool(true)),
        ]);
        if let Ok(frame) = exchange(
            location,
            HELLO,
            hello.to_json().as_bytes(),
            Duration::from_secs(2),
        ) {
            if frame.kind == SNAPSHOT {
                let mut terminal = HostedTerminal::new(80, 24, 0);
                terminal.process(&frame.payload);
                if !terminal.title().is_empty() {
                    if let Value::Obj(fields) = &mut value {
                        fields.push(("title".into(), json::s(terminal.title())));
                    }
                }
            }
        }
    }
    Ok(value)
}
pub fn status(state_dir: &Path, session: &str) -> Result<Value, String> {
    if !state_dir.is_absolute() || !valid_session(session) {
        return Err("Invalid screen state directory or session ID".into());
    }
    if matches!(fs::symlink_metadata(state_dir), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(stopped_status(session, None));
    }
    let location = SessionLocation::open(state_dir, session, false)?;
    let previous = claim(&location)?;
    if location.validate_socket()? {
        if let Some(previous) = &previous {
            match running_status(&location, previous) {
                Ok(status) => return Ok(status),
                Err(error) => {
                    if SessionLock::acquire(&location)?.is_none() {
                        return Err(error);
                    }
                    match unix::connect(&location.socket_path, Duration::from_millis(200)) {
                        Ok(_) => return Err(error),
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::ConnectionRefused
                                    | std::io::ErrorKind::NotFound
                            ) => {}
                        Err(e) => return Err(e.to_string()),
                    }
                }
            }
        } else {
            return Err("Screen endpoint has no matching startup claim".into());
        }
    }
    if previous.is_none() {
        return Ok(stopped_status(session, None));
    }
    if SessionLock::acquire(&location)?.is_none() {
        return Err("Screen session startup or shutdown is still in progress".into());
    }
    if previous.as_ref().is_some_and(|v| {
        !matches!(
            v.get("phase").and_then(Value::as_str),
            Some("ended" | "failed")
        )
    }) {
        return Err(
            "Screen startup was interrupted; identity is retained and requires explicit --restart"
                .into(),
        );
    }
    Ok(stopped_status(session, previous.as_ref()))
}
pub fn name(state_dir: &Path, session: &str, title: &str) -> Result<Value, String> {
    let location = SessionLocation::open(state_dir, session, false)?;
    let frame = exchange(
        &location,
        NAME,
        json::obj(vec![("title", json::s(title))])
            .to_json()
            .as_bytes(),
        Duration::from_secs(2),
    )?;
    if frame.kind != STATUS_REPLY {
        return Err(String::from_utf8_lossy(&frame.payload).into_owned());
    }
    parse_object(&frame.payload)
}

pub fn list(state_dir: &Path) -> Result<Value, String> {
    if !state_dir.is_absolute() {
        return Err("Screen state directory must be absolute".into());
    }
    if matches!(fs::symlink_metadata(state_dir), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(Value::Arr(vec![]));
    }
    private_directory(state_dir, false)?;
    let mut names = Vec::new();
    for (index, entry) in fs::read_dir(state_dir)
        .map_err(error)?
        .take(4097)
        .enumerate()
    {
        if index >= 4096 {
            return Err("Screen state directory listing exceeds its bound".into());
        }
        let entry = entry.map_err(error)?;
        let name = entry.file_name();
        if let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".claim"))
            .filter(|id| valid_session(id))
        {
            names.push(id.to_owned());
        }
        if names.len() > 1024 {
            return Err("Screen session listing exceeds its bound".into());
        }
    }
    names.sort();
    let mut sessions = Vec::new();
    for name in names {
        let location = SessionLocation::open(state_dir, &name, false)?;
        let Some(previous) = claim(&location)? else {
            continue;
        };
        if !location.validate_socket()? {
            continue;
        }
        match running_status(&location, &previous) {
            Ok(mut value) => {
                if let Value::Obj(fields) = &mut value {
                    fields.retain(|(key, _)| key != "state_dir");
                    fields.push((
                        "state_dir".into(),
                        json::s(location.state_dir.to_string_lossy()),
                    ));
                }
                sessions.push(value);
            }
            Err(_) if SessionLock::acquire(&location)?.is_some() => {}
            Err(error) => return Err(error),
        }
    }
    Ok(Value::Arr(sessions))
}
pub fn stop(state_dir: &Path, session: &str) -> Result<Value, String> {
    stop_selected(state_dir, session, None)
}

pub fn stop_instance(state_dir: &Path, session: &str, instance: &str) -> Result<Value, String> {
    stop_selected(state_dir, session, Some(instance))
}

fn stop_selected(state_dir: &Path, session: &str, expected: Option<&str>) -> Result<Value, String> {
    let current = status(state_dir, session)?;
    if current.get("running").and_then(Value::as_bool) != Some(true) {
        return Ok(current);
    }
    if expected
        .is_some_and(|instance| current.get("instance").and_then(Value::as_str) != Some(instance))
    {
        return Err("Session changed since selection; refresh and confirm again".into());
    }
    let location = SessionLocation::open(state_dir, session, false)?;
    let request = json::obj(
        expected
            .map(|instance| vec![("instance", json::s(instance))])
            .unwrap_or_default(),
    )
    .to_json();
    let frame = exchange(&location, STOP, request.as_bytes(), Duration::from_secs(2))?;
    if frame.kind == ERROR {
        return Err(String::from_utf8_lossy(&frame.payload).into_owned());
    }
    if frame.kind != STATUS_REPLY {
        return Err("Invalid screen stop reply".into());
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if let Ok(value) = status(state_dir, session) {
            if value.get("running").and_then(Value::as_bool) == Some(false) {
                return Ok(value);
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    Err("Screen stop is still pending; inspect its exact session before retrying".into())
}
pub fn start(mut options: StartOptions, restart: bool) -> Result<Value, String> {
    options.validate()?;
    let location = SessionLocation::open(&options.state_dir, &options.session_id, true)?;
    options.state_dir = location.state_dir.clone();
    let previous = claim(&location)?;
    let hash = options.command_hash();
    let Some(lock) = SessionLock::acquire(&location)? else {
        let previous = previous.ok_or("An in-flight screen session has no readable claim")?;
        if previous.get("command_hash").and_then(Value::as_str) != Some(&hash) {
            return Err("This running or starting session belongs to a different command".into());
        }
        return running_status(&location, &previous).map_err(|_| "Screen startup is already claimed; attach or inspect later, do not rerun the command".into());
    };
    if location.validate_socket()? {
        match unix::connect(&location.socket_path, Duration::from_millis(200)) {
            Ok(_) => {
                return Err(
                    "A live screen endpoint exists without its expected ownership lock".into(),
                )
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                ) =>
            {
                fs::remove_file(&location.socket_path).map_err(error)?;
            }
            Err(e) => return Err(error(e)),
        }
    }
    if previous.is_some() && !restart {
        return Err("This screen startup identity was already used; only an explicit --restart may launch it again".into());
    }
    let instance = random_token()?;
    let initial = json::obj(vec![
        ("version", Value::Int(VERSION as i64)),
        ("session_id", json::s(&options.session_id)),
        ("state_dir", json::s(options.state_dir.to_string_lossy())),
        ("cwd", json::s(options.cwd.to_string_lossy())),
        ("program", json::s(options.program.to_string_lossy())),
        ("command_hash", json::s(&hash)),
        (
            "title",
            previous
                .as_ref()
                .and_then(|v| v.get("title"))
                .cloned()
                .unwrap_or(Value::Null),
        ),
        ("instance", json::s(&instance)),
        ("phase", json::s("starting")),
        ("launcher_pid", Value::Int(std::process::id().into())),
        ("pid", Value::Null),
    ]);
    if previous.is_none() {
        create_private(&location.claim_path, initial.to_json().as_bytes())?;
    } else {
        write_private(&location.claim_path, initial.to_json().as_bytes())?;
    }
    let log_path = location.log_path();
    if log_path.exists() {
        read_private(&log_path, 1024 * 1024)?;
    }
    let mut log_options = OpenOptions::new();
    log_options.create(true).append(true).mode(0o600);
    #[cfg(target_os = "macos")]
    log_options.custom_flags(0x100);
    #[cfg(target_os = "linux")]
    log_options.custom_flags(0x20000);
    let log = log_options.open(&log_path).map_err(error)?;
    if log.metadata().map_err(error)?.len() > 512 * 1024 {
        log.set_len(0).map_err(error)?;
    }
    let fd = lock.inherit(true)?;
    let mut command = Command::new(std::env::current_exe().map_err(error)?);
    command
        .arg("serve")
        .arg("--state-dir")
        .arg(&options.state_dir)
        .arg("--session")
        .arg(&options.session_id)
        .arg("--cwd")
        .arg(&options.cwd)
        .arg("--cols")
        .arg(options.cols.to_string())
        .arg("--rows")
        .arg(options.rows.to_string())
        .arg("--theme")
        .arg(options.theme.encode())
        .arg("--instance")
        .arg(&instance)
        .arg("--lock-fd")
        .arg(fd.to_string())
        .arg("--")
        .arg(&options.program)
        .args(&options.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    // Detach in the new helper at serve entry, keeping this launch on the
    // standard library's posix_spawn path (no pre_exec/fork).
    let spawned = command.spawn();
    lock.inherit(false)?;
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            finish_claim(&location, &instance, "failed", None)?;
            return Err(format!(
                "Screen daemon could not start; identity retained: {error}"
            ));
        }
    };
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if let Ok(value) = running_status(&location, &initial) {
            return Ok(value);
        }
        if let Some(exit) = child.try_wait().map_err(error)? {
            finish_claim(&location, &instance, "failed", exit.code())?;
            return Err("Screen command ended or failed during startup; identity retained, command was not retried".into());
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    Err("Screen startup is still claimed; inspect/attach later. The daemon was not killed and the command was not retried".into())
}
fn finish_claim(
    location: &SessionLocation,
    instance: &str,
    phase: &str,
    code: Option<i32>,
) -> Result<(), String> {
    let mut value = claim(location)?.ok_or("Screen startup claim disappeared")?;
    if value.get("instance").and_then(Value::as_str) != Some(instance) {
        return Err("Screen startup claim was replaced".into());
    }
    if let Value::Obj(fields) = &mut value {
        fields.retain(|(name, _)| !matches!(name.as_str(), "phase" | "exit_code"));
        fields.push(("phase".into(), json::s(phase)));
        fields.push((
            "exit_code".into(),
            code.map(|code| Value::Int(code.into()))
                .unwrap_or(Value::Null),
        ));
    }
    write_private(&location.claim_path, value.to_json().as_bytes())
}
include!("host.rs");
pub fn serve(mut options: StartOptions, instance: &str, lock_fd: i32) -> Result<(), String> {
    unix::detach_session().map_err(error)?;
    options.validate()?;
    let location = SessionLocation::open(&options.state_dir, &options.session_id, false)?;
    let _lock = unsafe { SessionLock::from_inherited(&location, lock_fd)? };
    let previous = claim(&location)?.ok_or("Screen serve requires an existing startup claim")?;
    if previous.get("instance").and_then(Value::as_str) != Some(instance)
        || previous.get("command_hash").and_then(Value::as_str)
            != Some(options.command_hash().as_str())
    {
        return Err("Screen serve does not match its claimed command".into());
    }
    let state = location.state_dir.as_os_str();
    let pty = match Pty::spawn_with_env(
        &options.program,
        &options.args,
        &options.cwd,
        options.cols,
        options.rows,
        &[
            (
                OsStr::new("MAKEPAD_SCREEN_SESSION"),
                OsStr::new(&options.session_id),
            ),
            (OsStr::new("MAKEPAD_SCREEN_STATE_DIR"), state),
        ],
    ) {
        Ok(pty) => pty,
        Err(e) => {
            finish_claim(&location, instance, "failed", None)?;
            return Err(format!("Screen PTY startup failed: {e}"));
        }
    };
    let listener = UnixListener::bind(&location.socket_path)
        .map_err(|e| format!("Cannot bind the screen session socket: {e}"))?;
    fs::set_permissions(&location.socket_path, fs::Permissions::from_mode(0o600)).map_err(error)?;
    listener.set_nonblocking(true).map_err(error)?;
    let socket_identity = fs::symlink_metadata(&location.socket_path).map_err(error)?;
    let mut host = Host {
        location: location.clone(),
        terminal: HostedTerminal::with_theme(
            options.cols as usize,
            options.rows as usize,
            5000,
            &options.theme,
        ),
        options,
        instance: instance.into(),
        pty,
        peers: Vec::new(),
        input: VecDeque::new(),
        generation: 0,
        client_count: 0,
        stopping: None,
        killed: false,
        started_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        last_output: Instant::now(),
    };
    let default_title = Path::new(&host.options.program)
        .file_name()
        .unwrap_or(&host.options.program)
        .to_string_lossy()
        .into_owned();
    host.terminal.set_title(
        previous
            .get("title")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(&default_title),
    );
    host.persist_name()?;
    let result = host.run(listener);
    let phase = if result.is_ok() { "ended" } else { "failed" };
    let code = result.as_ref().ok().copied().flatten();
    let finalized = finish_claim(&location, instance, phase, code);
    if fs::symlink_metadata(&location.socket_path)
        .is_ok_and(|m| m.dev() == socket_identity.dev() && m.ino() == socket_identity.ino())
    {
        let _ = fs::remove_file(&location.socket_path);
    }
    if read_private(&location.metadata_path, 16 * 1024)
        .ok()
        .and_then(|bytes| parse_object(&bytes).ok())
        .is_some_and(|v| v.get("instance").and_then(Value::as_str) == Some(instance))
    {
        let _ = fs::remove_file(&location.metadata_path);
    }
    finalized?;
    result.map(|_| ())
}
