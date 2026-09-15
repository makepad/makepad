//! MCP streamable-HTTP endpoint on loopback for the AI lanes.
//!
//! Studio serves `POST http://127.0.0.1:<port>/mcp` (JSON-RPC 2.0) so a
//! lane's CLI (Claude, Codex, later Grok) can discover and call the
//! code-intelligence tools natively. The transport implements `initialize`,
//! `notifications/initialized`, `ping`, `tools/list` and `tools/call`; `GET`
//! answers 405 because server-initiated SSE is not offered; notifications
//! answer an empty 202.
//!
//! Every request that reaches the JSON-RPC layer carries a per-lane bearer
//! token; the transport rejects an unknown path (404), a foreign Host or
//! Origin (403) and a non-POST method (405) before looking at credentials, so
//! those replies never reveal whether a token was valid. Tokens are minted at
//! spawn ([`TokenStore::mint`]), OS-random, persisted privately under the state
//! directory (mode 0600 inside a 0700 directory) so a PTY that survives a
//! Studio restart keeps its credential, and revoked when the terminal's
//! lifecycle ends ([`TokenStore::revoke`]), never on UI detach.
//!
//! Execution is not here. The server calls a [`ToolDispatcher`] which the app
//! binds at startup to the shared tool registry (`McpServer::start(.., dispatcher)`).
//! The dispatcher runs on the server's worker threads: it may wait for a
//! receipt there, and it must never be called from the UI thread.
//!
//! Threading: one acceptor thread (non-blocking `accept` polled every 20 ms
//! so it observes `stop`) hands connections to a fixed pool of `WORKERS`
//! threads through a bounded queue of `QUEUE` slots; a connection arriving
//! over capacity is refused with 503 and closed. No thread per connection.

use makepad_bounded_http::{Conn, HeadError, Method, Resp};
use makepad_strict_json as json;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub use makepad_ai_services::wire::Risk;
pub use makepad_strict_json::{parse as parse_json, Value};

/// Protocol versions this server negotiates; the first is what it answers
/// with when the client's requested version is unknown.
pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// Largest request body accepted.
pub const MAX_BODY_BYTES: u64 = 64 * 1024;
/// Largest `tools/call` result text the transport emits in one reply. A
/// dispatcher that has more pages the rest (its envelope carries a cursor).
pub const MAX_RESULT_BYTES: usize = makepad_ai_services::wire::MAX_RESULT_BYTES;
/// Worker threads serving connections.
pub const WORKERS: usize = 4;
/// Accepted connections waiting for a worker.
pub const QUEUE: usize = 16;
/// Requests served on one keep-alive connection before it is closed.
pub const MAX_REQUESTS_PER_CONN: u32 = 256;
/// Slowloris budget for completing a request head once the first byte arrives.
pub const HEAD_DEADLINE_MS: u64 = 10_000;
const KEEPALIVE_IDLE_MS: u64 = 30_000;
const READ_TIMEOUT_MS: u64 = 10_000;
const WRITE_TIMEOUT_MS: u64 = 10_000;
const BODY_DEADLINE_MS: u64 = 10_000;
/// Longest lane / owner identity accepted by the token store.
const MAX_ID: usize = 64;
/// Longest tool name the transport forwards.
const MAX_TOOL_NAME: usize = makepad_ai_services::wire::MAX_TOOL_NAME;
/// Largest `arguments` object (encoded) the transport forwards.
const MAX_ARGS_BYTES: usize = makepad_ai_services::wire::MAX_ARGS_BYTES;

/// JSON-RPC error codes the transport uses.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

/// Who is calling, as established by the bearer token. Never taken from
/// request arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneCaller {
    pub lane: String,
    pub owner: String,
}

/// One tool definition as advertised over `tools/list`.
pub type ToolDef = makepad_ai_services::wire::ToolDef;

/// Result of executing a tool. `text` is the model-facing envelope (JSON
/// text); `is_error` marks a tool-level failure the model should read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub text: String,
    pub is_error: bool,
}

/// The execution seam. Studio binds its shared tool registry here.
pub trait ToolDispatcher: Send + Sync {
    /// Tools this caller may see, already filtered by capability.
    fn list(&self, caller: &LaneCaller) -> Vec<ToolDef>;
    /// Execute one tool. May block the server worker thread waiting for a
    /// receipt; must never be invoked on the UI thread.
    fn call(&self, caller: &LaneCaller, name: &str, args: &Value, request_id: &str) -> Outcome;
}

/// A minted bearer credential: 32 OS-random bytes as lowercase hex.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

struct TokenRecord {
    lane: String,
    owner: String,
    token: String,
}

/// Per-lane bearer tokens, persisted privately so PTYs that outlive Studio
/// keep their credential. Every access is constant-time over the token
/// bytes; the store never logs or returns a token except from `mint`.
pub struct TokenStore {
    path: PathBuf,
    records: Mutex<Vec<TokenRecord>>,
}

impl TokenStore {
    /// Open (or create) the store at `<state_dir>/mcp/tokens.txt`.
    pub fn open(state_dir: &Path) -> Result<Self, String> {
        let dir = state_dir.join("mcp");
        private_dir(&dir, true)?;
        let path = dir.join("tokens.txt");
        let records = match std::fs::symlink_metadata(&path) {
            Ok(meta) => {
                check_private_file(&path, &meta)?;
                let text = read_bounded(&path, 256 * 1024)?;
                parse_records(&text)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.to_string()),
        };
        Ok(Self { path, records: Mutex::new(records) })
    }

    /// Mint a fresh token for `lane`, owned by `owner` (the durable session
    /// owner). A lane already holding a token is re-minted: the old
    /// credential stops working at once.
    pub fn mint(&self, lane: &str, owner: &str) -> Result<Token, String> {
        if !identifier(lane) || !identifier(owner) {
            return Err("Invalid lane or owner identity for an MCP token".into());
        }
        let token = random_hex()?;
        let mut records = self.records.lock().map_err(|_| "MCP token store poisoned")?;
        if records.iter().any(|r| r.token == token) {
            return Err("OS entropy returned a duplicate MCP token".into());
        }
        records.retain(|r| r.lane != lane);
        records.push(TokenRecord { lane: lane.into(), owner: owner.into(), token: token.clone() });
        persist(&self.path, &records)?;
        Ok(Token(token))
    }

    /// Transfer a lane's token to a new owner (a split changes the
    /// authorised successor; the credential itself stays).
    pub fn reassign(&self, lane: &str, owner: &str) -> Result<bool, String> {
        if !identifier(lane) || !identifier(owner) {
            return Err("Invalid lane or owner identity for an MCP token".into());
        }
        let mut records = self.records.lock().map_err(|_| "MCP token store poisoned")?;
        let Some(record) = records.iter_mut().find(|r| r.lane == lane) else {
            return Ok(false);
        };
        record.owner = owner.into();
        persist(&self.path, &records)?;
        Ok(true)
    }

    /// Revoke a lane's token. Call this when the terminal's lifecycle ends,
    /// never on UI detach.
    pub fn revoke(&self, lane: &str) -> Result<bool, String> {
        let mut records = self.records.lock().map_err(|_| "MCP token store poisoned")?;
        let before = records.len();
        records.retain(|r| r.lane != lane);
        let removed = records.len() != before;
        if removed {
            persist(&self.path, &records)?;
        }
        Ok(removed)
    }

    /// Resolve a presented bearer credential to its lane, constant-time over
    /// the credential bytes.
    pub fn authorize(&self, token: &str) -> Option<LaneCaller> {
        let records = self.records.lock().ok()?;
        let mut found: Option<LaneCaller> = None;
        for record in records.iter() {
            if same_token(record.token.as_bytes(), token.as_bytes()) {
                found = Some(LaneCaller { lane: record.lane.clone(), owner: record.owner.clone() });
            }
        }
        found
    }

    pub fn lanes(&self) -> Vec<String> {
        self.records.lock().map(|r| r.iter().map(|x| x.lane.clone()).collect()).unwrap_or_default()
    }
}

fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ID
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

fn same_token(a: &[u8], b: &[u8]) -> bool {
    // Fixed-length compare over the longer operand so length leaks nothing
    // beyond "not 64 hex bytes".
    if a.len() != b.len() {
        let mut acc = 0u8;
        for (x, y) in a.iter().zip(b.iter().chain(std::iter::repeat(&0))) {
            acc |= x ^ y;
        }
        let _ = acc;
        return false;
    }
    let mut acc = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    acc == 0
}

fn random_hex() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    #[cfg(unix)]
    {
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .map_err(|_| "OS randomness is unavailable for MCP tokens")?;
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
        if unsafe { BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), bytes.len() as u32, 2) }
            != 0
        {
            return Err("OS randomness is unavailable for MCP tokens".into());
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        return Err("MCP tokens require a native OS random source".into());
    }
    #[allow(unreachable_code)]
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn parse_records(text: &str) -> Result<Vec<TokenRecord>, String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split(' ');
        let (Some(lane), Some(owner), Some(token), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err("MCP token store is malformed".into());
        };
        if !identifier(lane)
            || !identifier(owner)
            || token.len() != 64
            || !token.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("MCP token store holds an invalid record".into());
        }
        out.push(TokenRecord { lane: lane.into(), owner: owner.into(), token: token.into() });
    }
    Ok(out)
}

fn persist(path: &Path, records: &[TokenRecord]) -> Result<(), String> {
    let mut text = String::new();
    for r in records {
        text.push_str(&r.lane);
        text.push(' ');
        text.push_str(&r.owner);
        text.push(' ');
        text.push_str(&r.token);
        text.push('\n');
    }
    let dir = path.parent().ok_or("MCP token store has no directory")?;
    let temp = dir.join(format!(".tokens-{}.tmp", random_hex()?));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(|e| e.to_string())?;
    let result = (|| {
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        std::fs::rename(&temp, path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        std::fs::File::open(dir).and_then(|d| d.sync_all()).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn private_dir(path: &Path, create: bool) -> Result<(), String> {
    if create {
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut b = builder;
            b.mode(0o700);
            b
        };
        match builder.create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.file_type().is_dir() {
        return Err("MCP token directory must be a real directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        extern "C" {
            fn geteuid() -> u32;
        }
        if meta.uid() != unsafe { geteuid() } || meta.mode() & 0o077 != 0 {
            return Err("MCP token directory must be owned by this user with mode 0700".into());
        }
    }
    Ok(())
}

fn check_private_file(path: &Path, meta: &std::fs::Metadata) -> Result<(), String> {
    if !meta.file_type().is_file() {
        return Err(format!("{} must be a regular private file", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        extern "C" {
            fn geteuid() -> u32;
        }
        if meta.uid() != unsafe { geteuid() } || meta.mode() & 0o077 != 0 {
            return Err("MCP token store must be owned by this user with mode 0600".into());
        }
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: usize) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file).take(limit as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("MCP token store exceeds its bound".into());
    }
    String::from_utf8(bytes).map_err(|_| "MCP token store is not UTF-8".into())
}

/// The running endpoint. Dropping it stops the acceptor and the workers.
pub struct McpServer {
    port: u16,
    stop: Arc<AtomicBool>,
    acceptor: Option<JoinHandle<()>>,
    workers: Vec<JoinHandle<()>>,
}

struct Shared {
    tokens: Arc<TokenStore>,
    dispatcher: Arc<dyn ToolDispatcher>,
    stop: Arc<AtomicBool>,
    port: u16,
}

impl McpServer {
    /// Bind an ephemeral loopback port and start serving. This is the
    /// binding point for the real dispatcher: pass the shared tool registry.
    pub fn start(tokens: Arc<TokenStore>, dispatcher: Arc<dyn ToolDispatcher>) -> Result<Self, String> {
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| format!("MCP bind: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared { tokens, dispatcher, stop: stop.clone(), port });
        let (tx, rx) = mpsc::sync_channel::<TcpStream>(QUEUE);
        let rx = Arc::new(Mutex::new(rx));
        let mut workers = Vec::with_capacity(WORKERS);
        for i in 0..WORKERS {
            let rx = rx.clone();
            let shared = shared.clone();
            let join = std::thread::Builder::new()
                .name(format!("studio-mcp-{i}"))
                .spawn(move || worker_loop(rx, shared))
                .map_err(|e| format!("MCP worker spawn: {e}"))?;
            workers.push(join);
        }
        let acceptor_shared = shared.clone();
        let acceptor = std::thread::Builder::new()
            .name("studio-mcp-accept".into())
            .spawn(move || accept_loop(listener, tx, acceptor_shared))
            .map_err(|e| format!("MCP acceptor spawn: {e}"))?;
        Ok(Self { port, stop, acceptor: Some(acceptor), workers })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.acceptor.take() {
            let _ = join.join();
        }
        for join in self.workers.drain(..) {
            let _ = join.join();
        }
    }
}

fn accept_loop(listener: TcpListener, tx: mpsc::SyncSender<TcpStream>, shared: Arc<Shared>) {
    let _ = listener.set_nonblocking(true);
    while !shared.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, remote)) => {
                if !remote.ip().is_loopback() {
                    drop(stream);
                    continue;
                }
                let _ = stream.set_nonblocking(false);
                match tx.try_send(stream) {
                    Ok(()) => {}
                    Err(mpsc::TrySendError::Full(stream)) => refuse_capacity(stream),
                    Err(mpsc::TrySendError::Disconnected(_)) => return,
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    // Dropping `tx` here ends the workers' receive loops.
}

fn refuse_capacity(mut stream: TcpStream) {
    // Write the refusal straight to the socket and close the write side
    // gracefully (FIN, not RST) so the client reads the 503 before EOF.
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let body = rpc_error_body(Value::Null, INTERNAL_ERROR, "server at connection capacity; retry");
    let mut out = Vec::with_capacity(256 + body.len());
    out.extend_from_slice(b"HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nX-Content-Type-Options: nosniff\r\nRetry-After: 1\r\n");
    out.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    out.extend_from_slice(b"Connection: close\r\n\r\n");
    out.extend_from_slice(&body);
    let _ = stream.write_all(&out);
    let _ = stream.shutdown(std::net::Shutdown::Write);
    // Give the peer a moment to read before the socket is dropped.
    let mut sink = [0u8; 256];
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let _ = stream.read(&mut sink);
}

fn worker_loop(rx: Arc<Mutex<mpsc::Receiver<TcpStream>>>, shared: Arc<Shared>) {
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        let next = {
            let Ok(guard) = rx.lock() else { return };
            guard.recv_timeout(Duration::from_millis(250))
        };
        match next {
            Ok(stream) => serve_connection(stream, &shared),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn serve_connection(stream: TcpStream, shared: &Shared) {
    let mut conn = match Conn::new(stream, READ_TIMEOUT_MS, WRITE_TIMEOUT_MS) {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut served = 0u32;
    loop {
        let mut head = match conn.next_request(HEAD_DEADLINE_MS, KEEPALIVE_IDLE_MS, &shared.stop) {
            Ok(h) => h,
            Err(HeadError::Closed) | Err(HeadError::Io) => return,
            Err(HeadError::Timeout) => {
                let _ = conn.write_resp(false, &Resp::empty(408).closing());
                return;
            }
            Err(HeadError::Bad(status, message)) => {
                let body = rpc_error_body(Value::Null, INVALID_REQUEST, message);
                let _ = conn.write_resp(false, &Resp::bytes(status, "application/json", body).closing());
                return;
            }
        };
        served += 1;
        let is_head = head.method == Method::Head;
        let (resp, close_after) = handle_request(&mut conn, &mut head, shared);
        let mut resp = resp;
        if close_after || served >= MAX_REQUESTS_PER_CONN {
            resp.close = true;
        }
        if conn.write_resp(is_head, &resp).is_err() {
            return;
        }
        if resp.close || !conn.finish_request(&mut head) {
            return;
        }
    }
}

/// Route one request. Returns the response and whether the connection must
/// close afterwards (a refused body that was not consumed).
fn handle_request(conn: &mut Conn, head: &mut makepad_bounded_http::Head, shared: &Shared) -> (Resp, bool) {
    if head.segs.as_slice() != ["mcp"] {
        return (json_status(404, Value::Null, INVALID_REQUEST, "unknown endpoint"), false);
    }
    // Host/Origin: only this machine's loopback origin (or no Origin at all,
    // as CLI clients send) may talk to a lane endpoint.
    if !host_is_local(&head.host, shared.port) {
        return (json_status(403, Value::Null, INVALID_REQUEST, "host not accepted"), true);
    }
    if let Some(origin) = &head.origin {
        if !origin_is_local(origin, shared.port) {
            return (json_status(403, Value::Null, INVALID_REQUEST, "origin not accepted"), true);
        }
    }
    match head.method {
        Method::Post => {}
        Method::Get | Method::Head => {
            return (
                json_status(405, Value::Null, INVALID_REQUEST, "server-initiated streams are not offered")
                    .with_header("Allow", "POST".into()),
                false,
            );
        }
        _ => return (json_status(405, Value::Null, INVALID_REQUEST, "method not allowed").with_header("Allow", "POST".into()), true),
    }
    // Bearer before body: an unauthenticated client never gets to spend
    // our body budget.
    let caller = head
        .authorization
        .as_deref()
        .and_then(|a| a.strip_prefix("Bearer ").or_else(|| a.strip_prefix("bearer ")))
        .map(str::trim)
        .and_then(|t| shared.tokens.authorize(t));
    let Some(caller) = caller else {
        let deadline = Instant::now() + Duration::from_millis(500);
        conn.drain_remaining(head, deadline);
        return (
            json_status(401, Value::Null, INVALID_REQUEST, "bearer token required")
                .with_header("WWW-Authenticate", "Bearer realm=\"studio-lane\"".into()),
            true,
        );
    };
    if let Some(ct) = &head.content_type {
        let mime = ct.split(';').next().unwrap_or("").trim();
        if !mime.eq_ignore_ascii_case("application/json") {
            let deadline = Instant::now() + Duration::from_millis(500);
            conn.drain_remaining(head, deadline);
            return (json_status(415, Value::Null, INVALID_REQUEST, "application/json required"), true);
        }
    }
    let body = match conn.read_body_full(head, MAX_BODY_BYTES, BODY_DEADLINE_MS) {
        Ok(b) => b,
        Err(makepad_bounded_http::BodyError::TooLarge) => {
            let deadline = Instant::now() + Duration::from_millis(500);
            conn.drain_remaining(head, deadline);
            return (json_status(413, Value::Null, INVALID_REQUEST, "request body too large"), true);
        }
        Err(makepad_bounded_http::BodyError::Timeout) => return (Resp::empty(408).closing(), true),
        Err(_) => return (json_status(400, Value::Null, PARSE_ERROR, "malformed request body"), true),
    };
    let message = match json::parse_depth(&body, 32) {
        Ok(v) => v,
        Err(_) => return (json_status(400, Value::Null, PARSE_ERROR, "parse error"), false),
    };
    // Batches are not accepted: one message per request keeps the result
    // cap and receipt attribution exact.
    let Value::Obj(_) = &message else {
        return (json_status(400, Value::Null, INVALID_REQUEST, "one JSON-RPC message per request"), false);
    };
    (dispatch_message(&message, &caller, shared), false)
}

fn host_is_local(host: &str, port: u16) -> bool {
    // A bracketed IPv6 literal carries colons of its own: split the port off
    // only after the closing bracket, so `[::1]` (no port) and `[::1]:8080`
    // are both read correctly.
    let (name, p) = if let Some(rest) = host.strip_prefix('[') {
        match rest.split_once(']') {
            Some((inner, after)) => {
                let port = after.strip_prefix(':').and_then(|p| p.parse::<u16>().ok());
                if !after.is_empty() && port.is_none() {
                    return false;
                }
                (format!("[{inner}]"), port)
            }
            None => return false,
        }
    } else {
        match host.rsplit_once(':') {
            Some((n, p)) => (n.to_string(), p.parse::<u16>().ok()),
            None => (host.to_string(), None),
        }
    };
    matches!(name.as_str(), "127.0.0.1" | "localhost" | "[::1]") && p.is_none_or(|p| p == port)
}

/// Only a plain-HTTP loopback origin is local: the server speaks HTTP, so an
/// `https://` origin can never be this endpoint and is refused on purpose.
fn origin_is_local(origin: &str, port: u16) -> bool {
    let Some(rest) = origin.strip_prefix("http://") else { return false };
    host_is_local(rest.trim_end_matches('/'), port)
}

fn json_status(status: u16, id: Value, code: i64, message: &str) -> Resp {
    Resp::bytes(status, "application/json", rpc_error_body(id, code, message))
}

fn rpc_error_body(id: Value, code: i64, message: &str) -> Vec<u8> {
    json::obj(vec![
        ("jsonrpc", json::s("2.0")),
        ("id", id),
        ("error", json::obj(vec![("code", Value::Int(code)), ("message", json::s(message))])),
    ])
    .to_json()
    .into_bytes()
}

fn rpc_result(id: Value, result: Value) -> Resp {
    let body = json::obj(vec![("jsonrpc", json::s("2.0")), ("id", id), ("result", result)])
        .to_json()
        .into_bytes();
    Resp::bytes(200, "application/json", body)
}

fn dispatch_message(message: &Value, caller: &LaneCaller, shared: &Shared) -> Resp {
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return json_status(400, Value::Null, INVALID_REQUEST, "jsonrpc 2.0 required");
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return json_status(400, Value::Null, INVALID_REQUEST, "method required");
    };
    let id = message.get("id").cloned();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    // A message without an id is a notification: accept, do nothing, 202.
    let Some(id) = id else {
        return match method {
            "notifications/initialized" | "notifications/cancelled" | "notifications/progress" => {
                Resp::empty(202)
            }
            _ => Resp::empty(202),
        };
    };
    if !matches!(id, Value::Int(_) | Value::Str(_)) {
        return json_status(400, Value::Null, INVALID_REQUEST, "id must be a string or integer");
    }
    match method {
        "initialize" => {
            let requested = params.get("protocolVersion").and_then(Value::as_str);
            let version = requested
                .filter(|v| PROTOCOL_VERSIONS.contains(v))
                .unwrap_or(PROTOCOL_VERSIONS[0]);
            rpc_result(
                id,
                json::obj(vec![
                    ("protocolVersion", json::s(version)),
                    ("capabilities", json::obj(vec![("tools", json::obj(vec![("listChanged", Value::Bool(false))]))])),
                    (
                        "serverInfo",
                        json::obj(vec![
                            ("name", json::s("makepad-studio")),
                            ("version", json::s(env!("CARGO_PKG_VERSION"))),
                        ]),
                    ),
                    (
                        "instructions",
                        json::s(INSTRUCTIONS),
                    ),
                ]),
            )
        }
        "ping" => rpc_result(id, json::obj(vec![])),
        "tools/list" => {
            let tools = shared
                .dispatcher
                .list(caller)
                .into_iter()
                .map(|t| {
                    let schema = json::parse(t.parameters.as_bytes())
                        .unwrap_or_else(|_| json::obj(vec![("type", json::s("object"))]));
                    json::obj(vec![
                        ("name", json::s(&t.name)),
                        ("description", json::s(&t.description)),
                        ("inputSchema", schema),
                    ])
                })
                .collect();
            rpc_result(id, json::obj(vec![("tools", Value::Arr(tools))]))
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return rpc_error(id, INVALID_PARAMS, "params.name required");
            };
            if name.is_empty()
                || name.len() > MAX_TOOL_NAME
                || !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                return rpc_error(id, INVALID_PARAMS, "invalid tool name");
            }
            let args = params.get("arguments").cloned().unwrap_or_else(|| json::obj(vec![]));
            let Value::Obj(_) = &args else {
                return rpc_error(id, INVALID_PARAMS, "params.arguments must be an object");
            };
            if args.to_json().len() > MAX_ARGS_BYTES {
                return rpc_error(id, INVALID_PARAMS, "arguments exceed their bound");
            }
            let request_id = match &id {
                Value::Int(n) => format!("mcp-{}-{n}", caller.lane),
                Value::Str(s) => format!("mcp-{}-{s}", caller.lane),
                _ => unreachable!(),
            };
            let outcome = shared.dispatcher.call(caller, name, &args, &request_id);
            let text = if outcome.text.len() > MAX_RESULT_BYTES {
                // The dispatcher's envelope is expected to page; a reply over
                // the cap is a dispatcher bug, reported rather than truncated
                // mid-JSON.
                return rpc_error(id, INTERNAL_ERROR, "tool result exceeds the transport cap; page with the cursor");
            } else {
                outcome.text
            };
            rpc_result(
                id,
                json::obj(vec![
                    (
                        "content",
                        Value::Arr(vec![json::obj(vec![("type", json::s("text")), ("text", json::s(text))])]),
                    ),
                    ("isError", Value::Bool(outcome.is_error)),
                ]),
            )
        }
        _ => rpc_error(id, METHOD_NOT_FOUND, "method not found"),
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Resp {
    Resp::bytes(200, "application/json", rpc_error_body(id, code, message))
}

const INSTRUCTIONS: &str = "Studio's code-intelligence tools for this lane. Read code_brief for your scope and code_impact for your files before editing; check code_references, code_impact and code_coverage before deleting; call code_arch_diff against your baseline before claiming a refactor done. Candidates are possible resolutions, not dependencies. Truncation and \"no path\" never prove absence.";

#[cfg(test)]
mod host_tests {
    use super::host_is_local;
    #[test]
    fn host_ipv6_literal_with_and_without_port() {
        assert!(host_is_local("[::1]", 4242));
        assert!(host_is_local("[::1]:4242", 4242));
        assert!(!host_is_local("[::1]:4243", 4242));
        assert!(!host_is_local("[::1]:x", 4242));
        assert!(!host_is_local("[::2]", 4242));
        assert!(host_is_local("127.0.0.1", 4242));
        assert!(host_is_local("localhost:4242", 4242));
        assert!(!host_is_local("evil.example", 4242));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    struct Fake {
        big: bool,
    }

    impl ToolDispatcher for Fake {
        fn list(&self, caller: &LaneCaller) -> Vec<ToolDef> {
            vec![ToolDef::new(
                "code_outline",
                format!("outline for {}", caller.lane),
                r#"{"type":"object","properties":{"scope":{"type":"string"}},"additionalProperties":false}"#,
                makepad_ai_services::wire::Risk::Read,
            )]
        }
        fn call(&self, caller: &LaneCaller, name: &str, args: &Value, request_id: &str) -> Outcome {
            if self.big {
                return Outcome { text: "x".repeat(MAX_RESULT_BYTES + 1), is_error: false };
            }
            Outcome {
                text: json::obj(vec![
                    ("lane", json::s(&caller.lane)),
                    ("tool", json::s(name)),
                    ("scope", args.get("scope").cloned().unwrap_or(Value::Null)),
                    ("request_id", json::s(request_id)),
                ])
                .to_json(),
                is_error: false,
            }
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "studio-mcp-{tag}-{}-{}",
            std::process::id(),
            random_hex().unwrap()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        dir
    }

    struct Reply {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    /// A minimal HTTP/1.1 client over std::net for the tests: one request,
    /// one reply, connection closed.
    fn http(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Reply {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        if !body.is_empty() || method == "POST" {
            req.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let status: u16 = line.split(' ').nth(1).unwrap().parse().unwrap();
        let mut headers = Vec::new();
        let mut length = 0usize;
        loop {
            let mut h = String::new();
            reader.read_line(&mut h).unwrap();
            let h = h.trim_end().to_string();
            if h.is_empty() {
                break;
            }
            let (k, v) = h.split_once(':').unwrap();
            if k.eq_ignore_ascii_case("content-length") {
                length = v.trim().parse().unwrap();
            }
            headers.push((k.to_ascii_lowercase(), v.trim().to_string()));
        }
        let mut body = vec![0u8; length];
        if length > 0 {
            reader.read_exact(&mut body).unwrap();
        }
        Reply { status, headers, body }
    }

    fn rpc(port: u16, token: &str, body: &str) -> Reply {
        http(
            port,
            "POST",
            "/mcp",
            &[("Content-Type", "application/json"), ("Authorization", &format!("Bearer {token}"))],
            body.as_bytes(),
        )
    }

    fn start(big: bool) -> (McpServer, Arc<TokenStore>, PathBuf) {
        let dir = temp_dir("srv");
        let tokens = Arc::new(TokenStore::open(&dir).unwrap());
        let server = McpServer::start(tokens.clone(), Arc::new(Fake { big })).unwrap();
        (server, tokens, dir)
    }

    #[test]
    fn handshake_list_and_call_round_trip() {
        let (server, tokens, dir) = start(false);
        let token = tokens.mint("lane-a", "owner-1").unwrap();
        let t = token.as_str();
        let init = rpc(server.port(), t, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#);
        assert_eq!(init.status, 200);
        let v = json::parse(&init.body).unwrap();
        assert_eq!(v.get("id").and_then(Value::as_i64), Some(1));
        assert_eq!(v.get("result").unwrap().get("protocolVersion").and_then(Value::as_str), Some("2025-06-18"));
        let initialized = rpc(server.port(), t, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert_eq!(initialized.status, 202);
        assert!(initialized.body.is_empty());
        let ping = rpc(server.port(), t, r#"{"jsonrpc":"2.0","id":"p","method":"ping"}"#);
        assert_eq!(ping.status, 200);
        let list = rpc(server.port(), t, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = json::parse(&list.body).unwrap();
        let tools = v.get("result").unwrap().get("tools").unwrap().as_arr().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].get("name").and_then(Value::as_str), Some("code_outline"));
        assert_eq!(tools[0].get("description").and_then(Value::as_str), Some("outline for lane-a"));
        assert!(tools[0].get("inputSchema").unwrap().get("properties").is_some());
        let call = rpc(server.port(), t, r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"code_outline","arguments":{"scope":"widgets::dock"}}}"#);
        assert_eq!(call.status, 200);
        let v = json::parse(&call.body).unwrap();
        let content = v.get("result").unwrap().get("content").unwrap().as_arr().unwrap();
        let text = content[0].get("text").and_then(Value::as_str).unwrap();
        let inner = json::parse(text.as_bytes()).unwrap();
        assert_eq!(inner.get("lane").and_then(Value::as_str), Some("lane-a"));
        assert_eq!(inner.get("tool").and_then(Value::as_str), Some("code_outline"));
        assert_eq!(inner.get("scope").and_then(Value::as_str), Some("widgets::dock"));
        assert_eq!(inner.get("request_id").and_then(Value::as_str), Some("mcp-lane-a-3"));
        assert_eq!(v.get("result").unwrap().get("isError").and_then(Value::as_bool), Some(false));
        let unknown = rpc(server.port(), t, r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#);
        let v = json::parse(&unknown.body).unwrap();
        assert_eq!(v.get("error").unwrap().get("code").and_then(Value::as_i64), Some(METHOD_NOT_FOUND));
        drop(server);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_bad_auth_origin_method_and_bodies() {
        let (server, tokens, dir) = start(false);
        let token = tokens.mint("lane-b", "owner-1").unwrap();
        let t = token.as_str();
        let p = server.port();
        let missing = http(p, "POST", "/mcp", &[("Content-Type", "application/json")], b"{}");
        assert_eq!(missing.status, 401);
        assert!(missing.headers.iter().any(|(k, v)| k == "www-authenticate" && v.contains("Bearer")));
        let wrong = http(p, "POST", "/mcp", &[("Content-Type", "application/json"), ("Authorization", "Bearer 00")], b"{}");
        assert_eq!(wrong.status, 401);
        let origin = http(
            p,
            "POST",
            "/mcp",
            &[("Content-Type", "application/json"), ("Authorization", &format!("Bearer {t}")), ("Origin", "http://evil.example")],
            b"{}",
        );
        assert_eq!(origin.status, 403);
        let get = http(p, "GET", "/mcp", &[("Authorization", &format!("Bearer {t}"))], b"");
        assert_eq!(get.status, 405);
        assert!(get.headers.iter().any(|(k, v)| k == "allow" && v == "POST"));
        let big = vec![b' '; MAX_BODY_BYTES as usize + 1];
        let too_large = http(p, "POST", "/mcp", &[("Content-Type", "application/json"), ("Authorization", &format!("Bearer {t}"))], &big);
        assert_eq!(too_large.status, 413);
        let malformed = rpc(p, t, "{not json");
        assert_eq!(malformed.status, 400);
        let v = json::parse(&malformed.body).unwrap();
        assert_eq!(v.get("error").unwrap().get("code").and_then(Value::as_i64), Some(PARSE_ERROR));
        let no_version = rpc(p, t, r#"{"id":1,"method":"ping"}"#);
        let v = json::parse(&no_version.body).unwrap();
        assert_eq!(v.get("error").unwrap().get("code").and_then(Value::as_i64), Some(INVALID_REQUEST));
        let batch = rpc(p, t, r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#);
        assert_eq!(batch.status, 400);
        let bad_params = rpc(p, t, r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"arguments":{}}}"#);
        let v = json::parse(&bad_params.body).unwrap();
        assert_eq!(v.get("error").unwrap().get("code").and_then(Value::as_i64), Some(INVALID_PARAMS));
        let wrong_type = http(p, "POST", "/mcp", &[("Content-Type", "text/plain"), ("Authorization", &format!("Bearer {t}"))], b"{}");
        assert_eq!(wrong_type.status, 415);
        let elsewhere = rpc_path(p, t, "/other");
        assert_eq!(elsewhere.status, 404);
        drop(server);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn rpc_path(port: u16, token: &str, path: &str) -> Reply {
        http(port, "POST", path, &[("Content-Type", "application/json"), ("Authorization", &format!("Bearer {token}"))], b"{}")
    }

    #[test]
    fn oversized_tool_result_is_an_error_not_a_truncation() {
        let (server, tokens, dir) = start(true);
        let token = tokens.mint("lane-c", "owner-1").unwrap();
        let call = rpc(server.port(), token.as_str(), r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"code_outline","arguments":{}}}"#);
        assert_eq!(call.status, 200);
        let v = json::parse(&call.body).unwrap();
        let err = v.get("error").unwrap();
        assert_eq!(err.get("code").and_then(Value::as_i64), Some(INTERNAL_ERROR));
        assert!(err.get("message").and_then(Value::as_str).unwrap().contains("cursor"));
        drop(server);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn revoke_and_remint_invalidate_old_tokens() {
        let (server, tokens, dir) = start(false);
        let first = tokens.mint("lane-d", "owner-1").unwrap();
        assert_eq!(rpc(server.port(), first.as_str(), r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).status, 200);
        let second = tokens.mint("lane-d", "owner-1").unwrap();
        assert_ne!(first, second);
        assert_eq!(rpc(server.port(), first.as_str(), r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).status, 401);
        assert_eq!(rpc(server.port(), second.as_str(), r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).status, 200);
        assert!(tokens.revoke("lane-d").unwrap());
        assert!(!tokens.revoke("lane-d").unwrap());
        assert_eq!(rpc(server.port(), second.as_str(), r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).status, 401);
        drop(server);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tokens_persist_privately_and_reload() {
        let dir = temp_dir("persist");
        let token = {
            let store = TokenStore::open(&dir).unwrap();
            let t = store.mint("lane-e", "owner-9").unwrap();
            assert!(store.reassign("lane-e", "owner-10").unwrap());
            assert!(!store.reassign("lane-x", "owner-10").unwrap());
            t
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(dir.join("mcp").join("tokens.txt")).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
            let dmeta = std::fs::metadata(dir.join("mcp")).unwrap();
            assert_eq!(dmeta.permissions().mode() & 0o777, 0o700);
        }
        let store = TokenStore::open(&dir).unwrap();
        let caller = store.authorize(token.as_str()).unwrap();
        assert_eq!(caller, LaneCaller { lane: "lane-e".into(), owner: "owner-10".into() });
        assert!(store.authorize("nope").is_none());
        assert_eq!(store.lanes(), vec!["lane-e".to_string()]);
        // A tampered store is refused, not partially trusted.
        std::fs::write(dir.join("mcp").join("tokens.txt"), "lane-e owner-10 zz\n").unwrap();
        assert!(TokenStore::open(&dir).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn identity_and_token_rules() {
        let dir = temp_dir("ids");
        let store = TokenStore::open(&dir).unwrap();
        assert!(store.mint("", "o").is_err());
        assert!(store.mint("lane a", "o").is_err());
        assert!(store.mint(&"l".repeat(MAX_ID + 1), "o").is_err());
        let t = store.mint("lane-f", "o").unwrap();
        assert_eq!(t.as_str().len(), 64);
        assert!(t.as_str().bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(same_token(b"abc", b"abc"));
        assert!(!same_token(b"abc", b"abd"));
        assert!(!same_token(b"abc", b"ab"));
        assert!(host_is_local("127.0.0.1:5", 5));
        assert!(host_is_local("localhost", 5));
        assert!(!host_is_local("127.0.0.1:6", 5));
        assert!(!host_is_local("example.com:5", 5));
        assert!(origin_is_local("http://127.0.0.1:5", 5));
        assert!(!origin_is_local("https://127.0.0.1:5", 5));
        assert!(!origin_is_local("http://127.0.0.1:7", 5));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_clients_are_bounded_not_unbounded_threads() {
        let (server, tokens, dir) = start(false);
        let token = tokens.mint("lane-g", "owner-1").unwrap();
        let p = server.port();
        // Park WORKERS idle keep-alive connections on the workers and QUEUE
        // more in the hand-off queue. Nothing beyond that may get a thread:
        // the acceptor must answer the overflow with 503 and close.
        let mut held = Vec::new();
        for _ in 0..(WORKERS + QUEUE) {
            held.push(TcpStream::connect((Ipv4Addr::LOCALHOST, p)).unwrap());
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut refused = None;
        while Instant::now() < deadline {
            let overflow = TcpStream::connect((Ipv4Addr::LOCALHOST, p)).unwrap();
            overflow.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
            let mut reader = BufReader::new(overflow);
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(n) if n > 0 => {
                    refused = Some(line);
                    break;
                }
                // Not yet drained from the backlog into the queue: the
                // acceptor has not seen this connection; keep it parked and
                // try again with a fresh one.
                _ => {
                    held.push(reader.into_inner());
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        let line = refused.expect("overflow connection got no refusal within 5 s");
        assert!(line.starts_with("HTTP/1.1 503"), "expected 503, got {line:?}");
        drop(held);
        // Once capacity frees, ordinary service resumes on the same server.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let ping = rpc(p, token.as_str(), r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
            if ping.status == 200 {
                break;
            }
            assert!(Instant::now() < deadline, "service did not resume after capacity freed: {}", ping.status);
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(server);
        let _ = std::fs::remove_dir_all(dir);
    }
}
