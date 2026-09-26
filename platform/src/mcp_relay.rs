//! `--mcp`: the app as an MCP server on stdio, for Claude Desktop.
//!
//! Claude Desktop starts `<app> --mcp` once per session and keeps it
//! running. This process opens no window, no GPU and no audio, and writes
//! nothing but JSON-RPC to stdout (anything else goes to stderr): it is a
//! relay. Each newline-delimited JSON-RPC message from stdin goes as one
//! HTTP POST to the running app's loopback MCP endpoint — the one the F10
//! panel serves while "Claude Desktop" is its provider — and the reply
//! goes back as one line. It exits when stdin closes.
//!
//! The running app is found through its discovery file,
//! `~/.makepad/mcp/<app-id>.json` ([`McpDiscovery`]: pid, port, bearer
//! token, title), written privately by the app and trusted only while its
//! pid is alive. `<app-id>` is the executable's stem, so this process and
//! the app agree on it without being told.
//!
//! The relay answers `initialize`, `ping` and notifications itself, and
//! `tools/list` from the list the app last served (`<app-id>.tools.json`)
//! while the app is not running, so a Claude Desktop start does not start
//! every connected app. The first request that needs the app itself (a
//! tool call, or a list never served before) starts it: the same
//! executable, without `--mcp`, detached, with Claude Desktop chosen as its
//! AI provider, and the relay waits up to [`LAUNCH_WAIT`] for its file.

use crate::makepad_micro_serde::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The command-line flag that makes an app this relay.
pub const MCP_FLAG: &str = "--mcp";
/// The provider slug of the F10 panel's "Claude Desktop" row.
pub const CLAUDE_DESKTOP: &str = "claude-desktop";
/// The environment variable that picks the F10 panel's provider.
pub const PROVIDER_ENV: &str = "MAKEPAD_AI_PROVIDER";
/// How long a started app has to write its discovery file.
pub const LAUNCH_WAIT: Duration = Duration::from_secs(20);
/// Protocol versions the relay negotiates, newest first.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// A tool call may wait for the person to confirm it in the app.
const CALL_READ_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Largest reply body the relay passes on.
const MAX_REPLY_BYTES: usize = 1024 * 1024;

/// Where a running app's MCP endpoint is: `~/.makepad/mcp/<app-id>.json`.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct McpDiscovery {
    pub pid: u32,
    pub port: u16,
    pub token: String,
    pub title: String,
}

/// The flag is on the command line.
pub fn requested() -> bool {
    std::env::args().skip(1).any(|arg| arg == MCP_FLAG)
}

/// This executable's stem, as a file-name-safe id: what the app and its
/// `--mcp` relay both call it.
pub fn app_id() -> String {
    let stem = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let id: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' })
        .collect();
    let id = id.trim_matches('.').to_string();
    if id.is_empty() {
        "app".into()
    } else {
        id
    }
}

/// `~/.makepad/mcp`.
pub fn discovery_dir() -> PathBuf {
    crate::home::makepad_home().join("mcp")
}

pub fn discovery_path(app_id: &str) -> PathBuf {
    discovery_dir().join(format!("{app_id}.json"))
}

/// The `tools` array the app last answered to `tools/list`.
pub fn tools_cache_path(app_id: &str) -> PathBuf {
    discovery_dir().join(format!("{app_id}.tools.json"))
}

/// Write a file only this user can read (mode 0600 in a 0700 directory on
/// Unix), replacing it whole.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or("no directory")?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let temp = dir.join(format!(".{}.{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("file"), std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options
        .open(&temp)
        .and_then(|mut file| file.write_all(bytes))
        .and_then(|_| std::fs::rename(&temp, path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("{}: {e}", path.display()));
    }
    Ok(())
}

/// The app's discovery file, when the app that wrote it is still running.
pub fn read_live(app_id: &str) -> Option<McpDiscovery> {
    let text = std::fs::read_to_string(discovery_path(app_id)).ok()?;
    let found = McpDiscovery::deserialize_json(&text).ok()?;
    process_alive(found.pid).then_some(found)
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const EPERM: i32 = 1;
    pid != 0
        && (unsafe { kill(pid as i32, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(EPERM))
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    type Handle = *mut std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn GetExitCodeProcess(process: Handle, code: *mut u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(process, &mut code) != 0;
        CloseHandle(process);
        ok && code == STILL_ACTIVE
    }
}

#[cfg(not(any(unix, windows)))]
fn process_alive(_pid: u32) -> bool {
    false
}

/// Run the relay until stdin closes. Returns the process exit code.
pub fn run() -> i32 {
    let mut relay = Relay { app_id: app_id(), app: None };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(reply) = relay.handle(line) {
            if writeln!(stdout, "{reply}").and_then(|_| stdout.flush()).is_err() {
                break;
            }
        }
    }
    0
}

struct Relay {
    app_id: String,
    /// The endpoint in use, while it answers.
    app: Option<McpDiscovery>,
}

enum Posted {
    /// A reply body to pass on.
    Reply(String),
    /// Accepted with nothing to say (a notification).
    Accepted,
}

impl Relay {
    fn handle(&mut self, line: &str) -> Option<String> {
        let message = match JsonValue::deserialize_json(line) {
            Ok(message) => message,
            Err(_) => return Some(error_line("null", -32700, "parse error")),
        };
        // A reply of the client's own (to a request we never send) or a
        // notification needs no answer; notifications stay here.
        let method = message.get("method").and_then(|m| m.as_str())?.to_string();
        let id = message.get("id")?.serialize_json();
        match method.as_str() {
            "initialize" => {
                let requested = message.get("params").and_then(|p| p.get("protocolVersion")).and_then(|v| v.as_str());
                let version = requested.filter(|v| PROTOCOL_VERSIONS.contains(v)).unwrap_or(PROTOCOL_VERSIONS[0]);
                Some(result_line(&id, &self.initialize_result(version)))
            }
            "ping" => Some(result_line(&id, "{}")),
            "tools/list" if self.live().is_none() => match std::fs::read_to_string(tools_cache_path(&self.app_id)) {
                Ok(tools) if JsonValue::deserialize_json(&tools).is_ok_and(|t| matches!(t, JsonValue::Array(_))) => {
                    Some(result_line(&id, &format!("{{\"tools\":{tools}}}")))
                }
                _ => Some(self.forward(&id, line)),
            },
            _ => Some(self.forward(&id, line)),
        }
    }

    fn initialize_result(&self, version: &str) -> String {
        let title = self.live().map(|app| app.title).filter(|t| !t.is_empty()).unwrap_or_else(|| self.app_id.clone());
        let instructions = format!(
            "The tools of {title}, a Makepad app on this computer. Each call shows in the app's AI panel (F10); \
             a call that deletes or changes things outside the app waits there for the person to confirm it. \
             The app starts by itself when a tool is called."
        );
        format!(
            "{{\"protocolVersion\":{},\"capabilities\":{{\"tools\":{{\"listChanged\":false}}}},\"serverInfo\":{{\"name\":{},\"title\":{},\"version\":{}}},\"instructions\":{}}}",
            version.serialize_json(),
            self.app_id.serialize_json(),
            title.serialize_json(),
            env!("CARGO_PKG_VERSION").serialize_json(),
            instructions.serialize_json(),
        )
    }

    /// The endpoint in use, or the discovery file's, when the app is up.
    fn live(&self) -> Option<McpDiscovery> {
        self.app.clone().or_else(|| read_live(&self.app_id))
    }

    /// Send one message to the app, starting it when it is not running.
    /// A refused connection or a stale token drops the endpoint and tries
    /// once more with a fresh one.
    fn forward(&mut self, id: &str, line: &str) -> String {
        for _ in 0..2 {
            let app = match self.app.clone().or_else(|| read_live(&self.app_id)) {
                Some(app) => app,
                None => match self.launch() {
                    Ok(app) => app,
                    Err(e) => return error_line(id, -32603, &e),
                },
            };
            match post(&app, line) {
                Ok(Posted::Reply(body)) => {
                    self.app = Some(app);
                    return body;
                }
                Ok(Posted::Accepted) => {
                    self.app = Some(app);
                    return result_line(id, "{}");
                }
                Err(e) => {
                    eprintln!("mcp relay: {} did not answer ({e})", self.app_id);
                    self.app = None;
                }
            }
        }
        error_line(id, -32603, &format!("{} is not answering", self.app_id))
    }

    /// Start this executable as the app, with Claude Desktop as its AI
    /// provider, and wait for its discovery file.
    fn launch(&mut self) -> Result<McpDiscovery, String> {
        let exe = std::env::current_exe().map_err(|e| format!("cannot find the app to start: {e}"))?;
        let mut command = std::process::Command::new(&exe);
        command
            .env(PROVIDER_ENV, CLAUDE_DESKTOP)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Its own process group: the app outlives this relay and the
            // signals Claude Desktop sends it.
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }
        let mut child = command.spawn().map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
        let pid = child.id();
        // Reap it whenever it exits, so it never lingers as a zombie of ours.
        std::thread::spawn(move || child.wait());
        let deadline = Instant::now() + LAUNCH_WAIT;
        while Instant::now() < deadline {
            if let Some(app) = read_live(&self.app_id).filter(|app| app.pid == pid) {
                return Ok(app);
            }
            if !process_alive(pid) {
                return Err(format!("{} exited while starting", self.app_id));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(format!("{} did not open its Claude Desktop connection within {} s", self.app_id, LAUNCH_WAIT.as_secs()))
    }
}

fn result_line(id: &str, result: &str) -> String {
    format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result}}}")
}

fn error_line(id: &str, code: i64, message: &str) -> String {
    format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":{code},\"message\":{}}}}}", message.serialize_json())
}

/// One HTTP/1.1 POST to the app's `/mcp`, connection closed after.
fn post(app: &McpDiscovery, body: &str) -> Result<Posted, String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, app.port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(CALL_READ_TIMEOUT)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
    let head = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        app.port,
        app.token,
        body.len()
    );
    stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body.as_bytes())).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).map_err(|e| e.to_string())?;
    let status: u16 = status_line.split(' ').nth(1).and_then(|s| s.parse().ok()).ok_or("no HTTP status")?;
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).map_err(|e| e.to_string())? == 0 {
            return Err("the reply ended early".into());
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().map_err(|_| "bad Content-Length")?;
            }
        }
    }
    if length > MAX_REPLY_BYTES {
        return Err("reply too large".into());
    }
    let mut reply = vec![0u8; length];
    reader.read_exact(&mut reply).map_err(|e| e.to_string())?;
    match status {
        // A token the app no longer holds: it restarted; read its new file.
        401 => Err("the app refused the token".into()),
        _ if reply.is_empty() => Ok(Posted::Accepted),
        _ => String::from_utf8(reply).map(Posted::Reply).map_err(|_| "reply is not UTF-8".into()),
    }
}
