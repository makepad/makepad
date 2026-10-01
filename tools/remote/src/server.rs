//! The tunnel server: TLS (pinned self-signed identity), a pre-shared key
//! per box for client authentication, LAN-only bind and peers, auth-failure
//! throttling, an audit log per key, a time limit on foreground commands
//! and a fixed admin allowlist. It refuses to start without keys.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use makepad_network::tls::{TlsIdentity, TlsServer, TlsStream};
use makepad_network::tunnel::*;

use crate::procs::{self, SpawnRecord};

/// Authentication failures from one address that trigger a ban.
pub const MAX_FAILURES: usize = 5;
pub const FAILURE_WINDOW: Duration = Duration::from_secs(10 * 60);
pub const BAN_TIME: Duration = Duration::from_secs(15 * 60);
/// Simultaneous unauthenticated connections (TLS + auth in progress).
const MAX_PENDING: usize = 8;
/// A peer must finish TLS and authentication within this.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// Silence allowed between requests before the run starts.
const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub struct ServerOptions {
    pub port: u16,
    pub bind: String,
    pub allow_all: bool,
    pub keys: PathBuf,
    pub identity: PathBuf,
    pub audit_log: PathBuf,
    pub max_run: Duration,
}

impl ServerOptions {
    pub fn defaults() -> Self {
        let dir = tunnel_dir();
        Self {
            port: DEFAULT_PORT,
            bind: "lan".into(),
            allow_all: false,
            keys: dir.join("server-keys"),
            identity: dir.join("identity"),
            audit_log: dir.join("audit.log"),
            max_run: Duration::from_secs(6 * 3600),
        }
    }
}

// --- process groups -----------------------------------------------------------

#[cfg(windows)]
mod process_group {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::{Child, Command};

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> *mut c_void;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn TerminateJobObject(job: *mut c_void, exit_code: u32) -> i32;
        fn CloseHandle(object: *mut c_void) -> i32;
    }

    pub struct JobHandle(*mut c_void);
    unsafe impl Send for JobHandle {}

    impl JobHandle {
        pub fn new() -> io::Result<Self> {
            let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(JobHandle(job))
        }

        pub fn assign(&mut self, child: &Child) -> io::Result<()> {
            if unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn terminate(&self) {
            unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    impl Drop for JobHandle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    pub fn configure_command(cmd: &mut Command) {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

#[cfg(unix)]
mod process_group {
    use std::io;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    pub struct JobHandle(u32);

    impl JobHandle {
        pub fn new() -> io::Result<Self> {
            Ok(JobHandle(0))
        }

        pub fn assign(&mut self, child: &Child) -> io::Result<()> {
            self.0 = child.id();
            Ok(())
        }

        pub fn terminate(&self) {
            if self.0 == 0 {
                return;
            }
            extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
            }
            unsafe { kill(-(self.0 as i32), 9) };
        }
    }

    pub fn configure_command(cmd: &mut Command) {
        unsafe {
            cmd.pre_exec(|| {
                extern "C" {
                    fn setpgid(pid: i32, pgid: i32) -> i32;
                }
                setpgid(0, 0);
                Ok(())
            });
        }
    }
}

// --- shared state -----------------------------------------------------------

struct RunningCommand {
    id: u64,
    child: Child,
    job: process_group::JobHandle,
    stream: TcpStream,
}

type RunState = Arc<Mutex<Option<RunningCommand>>>;
type SpawnState = Arc<Mutex<Vec<SpawnRecord>>>;
type Conn = Arc<Mutex<TlsStream>>;

static RUN_SEQ: AtomicU64 = AtomicU64::new(1);
static RESTART: AtomicBool = AtomicBool::new(false);

fn kill_running(state: &RunState) {
    let mut lock = state.lock().unwrap();
    if let Some(mut running) = lock.take() {
        eprintln!("server: stopping previous command (pid {})", running.child.id());
        running.job.terminate();
        let _ = running.child.wait();
        let _ = running.stream.shutdown(Shutdown::Both);
    }
}

// --- audit log --------------------------------------------------------------

pub struct Audit {
    file: Mutex<Option<fs::File>>,
}

impl Audit {
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = fs::OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self { file: Mutex::new(Some(file)) })
    }

    /// One line: UTC time, peer, key id, event, detail (control characters
    /// escaped so a command cannot forge lines).
    pub fn log(&self, peer: &str, key: &str, event: &str, detail: &str) {
        let line = format!("{} peer={peer} key={key} {event} {}\n", utc_now(), escape(detail));
        eprint!("audit: {line}");
        if let Ok(mut guard) = self.file.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.write_all(line.as_bytes());
                let _ = f.flush();
            }
        }
    }
}

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().take(2000) {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn utc_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

// --- throttling -------------------------------------------------------------

/// Per-address authentication failures and bans.
pub struct Limiter {
    peers: Mutex<HashMap<IpAddr, (Vec<Instant>, Option<Instant>)>>,
    pending: AtomicUsize,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Limiter {
    pub fn new() -> Self {
        Self { peers: Mutex::new(HashMap::new()), pending: AtomicUsize::new(0) }
    }

    pub fn banned(&self, ip: IpAddr, now: Instant) -> bool {
        let mut peers = self.peers.lock().unwrap();
        match peers.get_mut(&ip) {
            Some((_, Some(until))) if *until > now => true,
            Some((fails, ban)) => {
                if ban.is_some() {
                    *ban = None;
                    fails.clear();
                }
                false
            }
            None => false,
        }
    }

    /// Records a failure; true when it starts a ban.
    pub fn fail(&self, ip: IpAddr, now: Instant) -> bool {
        let mut peers = self.peers.lock().unwrap();
        let (fails, ban) = peers.entry(ip).or_default();
        fails.retain(|t| now.duration_since(*t) < FAILURE_WINDOW);
        fails.push(now);
        if fails.len() >= MAX_FAILURES {
            *ban = Some(now + BAN_TIME);
            fails.clear();
            return true;
        }
        false
    }

    pub fn succeed(&self, ip: IpAddr) {
        self.peers.lock().unwrap().remove(&ip);
    }

    fn try_begin(&self) -> bool {
        if self.pending.fetch_add(1, Ordering::SeqCst) >= MAX_PENDING {
            self.pending.fetch_sub(1, Ordering::SeqCst);
            return false;
        }
        true
    }

    fn end(&self) {
        self.pending.fetch_sub(1, Ordering::SeqCst);
    }
}

// --- LAN scope --------------------------------------------------------------

pub fn is_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_lan(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/// `lan`: the address of the interface that holds the default route (no
/// packet is sent); it must be a private address. An explicit address must
/// be loopback or private; the wildcard is refused.
pub fn resolve_bind(bind: &str) -> io::Result<IpAddr> {
    let ip = if bind == "lan" {
        let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        probe.connect(("192.0.2.1", 9))?;
        probe.local_addr()?.ip()
    } else {
        bind.parse::<IpAddr>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("bad --bind address {bind}")))?
    };
    if ip.is_unspecified() || !is_lan(ip) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("refusing to listen on {ip}: the tunnel binds a LAN (private) address only"),
        ));
    }
    Ok(ip)
}

// --- admin allowlist ----------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum AdminAction {
    /// A fixed command line (no client input in it).
    Run(&'static [&'static str]),
    /// Restart this server (picks up a replaced executable).
    RestartTunnel,
}

pub const ADMIN_ACTIONS: &[&str] = &["node-status", "node-stop", "node-start", "node-restart", "tunnel-restart"];

/// The admin allowlist. Node actions drive the AI node's service and need
/// the rights the installer grants the tunnel's account on exactly that
/// service (start, stop, query); nothing else is reachable through here.
pub fn admin_action(name: &str) -> Result<AdminAction, String> {
    const PS: &str = "powershell";
    match name.trim() {
        "node-status" if cfg!(windows) => Ok(AdminAction::Run(&[PS, "-NoProfile", "-NonInteractive", "-Command", "Get-Service MakepadAiNode | Format-List Name,Status,StartType"])),
        "node-stop" if cfg!(windows) => Ok(AdminAction::Run(&[PS, "-NoProfile", "-NonInteractive", "-Command", "Stop-Service MakepadAiNode -Force; Get-Service MakepadAiNode | Format-List Name,Status"])),
        "node-start" if cfg!(windows) => Ok(AdminAction::Run(&[PS, "-NoProfile", "-NonInteractive", "-Command", "Start-Service MakepadAiNode; Get-Service MakepadAiNode | Format-List Name,Status"])),
        "node-restart" if cfg!(windows) => Ok(AdminAction::Run(&[PS, "-NoProfile", "-NonInteractive", "-Command", "Restart-Service MakepadAiNode -Force; Get-Service MakepadAiNode | Format-List Name,Status"])),
        "tunnel-restart" => Ok(AdminAction::RestartTunnel),
        other if ADMIN_ACTIONS.contains(&other) => Err(format!("admin action {other} is not available on this OS")),
        other => Err(format!("admin action {other:?} is not on the allowlist ({})", ADMIN_ACTIONS.join(", "))),
    }
}

// --- server -----------------------------------------------------------------

pub fn init_identity(dir: &Path) -> io::Result<TlsIdentity> {
    let host = env::var("COMPUTERNAME").or_else(|_| env::var("HOSTNAME")).unwrap_or_else(|_| "box".into());
    TlsIdentity::load_or_create(dir, &format!("makepad tunnel {host}"))
}

pub fn run_server(opts: ServerOptions) -> io::Result<()> {
    // Fail closed: no keys, no server.
    let keys = load_server_keys(&opts.keys).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "no usable tunnel keys in {} ({e}); create them with `makepad-remote keygen <host>` on the client",
                opts.keys.display()
            ),
        )
    })?;
    let identity = init_identity(&opts.identity)?;
    let fingerprint = identity.fingerprint;
    let tls = Arc::new(TlsServer::new(identity)?);
    let audit = Arc::new(Audit::open(&opts.audit_log)?);
    let ip = resolve_bind(&opts.bind)?;
    let cwd = env::current_dir()?.canonicalize()?;
    let listener = TcpListener::bind(SocketAddr::new(ip, opts.port))?;
    eprintln!("server: cwd = {}", cwd.display());
    eprintln!("server: listening on {} (TLS)", listener.local_addr()?);
    eprintln!("server: certificate sha256 {}", makepad_network::tls::to_hex(&fingerprint));
    eprintln!("server: {} key(s) in {}", keys.len(), opts.keys.display());
    if opts.allow_all {
        eprintln!("server: --all: shell, spawn, ps and kill enabled for authenticated clients");
    }
    audit.log("-", "-", "start", &format!("listen={} keys={}", listener.local_addr()?, keys.len()));

    let ctx = Arc::new(Ctx {
        cwd,
        allow_all: opts.allow_all,
        keys_path: opts.keys.clone(),
        fingerprint,
        max_run: opts.max_run,
        audit: audit.clone(),
        limiter: Limiter::new(),
        run: Arc::new(Mutex::new(None)),
        spawns: Arc::new(Mutex::new(Vec::new())),
        listen: listener.local_addr()?,
    });

    for stream in listener.incoming() {
        if RESTART.load(Ordering::SeqCst) {
            break;
        }
        let tcp = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("server: accept error: {e}");
                continue;
            }
        };
        let Ok(peer) = tcp.peer_addr() else { continue };
        let now = Instant::now();
        if !is_lan(peer.ip()) {
            audit.log(&peer.to_string(), "-", "refused", "not a LAN address");
            continue;
        }
        if ctx.limiter.banned(peer.ip(), now) {
            continue;
        }
        if !ctx.limiter.try_begin() {
            audit.log(&peer.to_string(), "-", "refused", "too many pending handshakes");
            continue;
        }
        let ctx = ctx.clone();
        let tls = tls.clone();
        thread::spawn(move || {
            let session = authenticate(&ctx, &tls, tcp, peer);
            ctx.limiter.end();
            if let Some((conn, key)) = session {
                let key_id = key.id_hex();
                if let Err(e) = handle_session(&ctx, conn, &key, peer) {
                    ctx.audit.log(&peer.to_string(), &key_id, "error", &e.to_string());
                }
            }
        });
    }
    if RESTART.load(Ordering::SeqCst) {
        restart_self()?;
    }
    Ok(())
}

struct Ctx {
    cwd: PathBuf,
    allow_all: bool,
    keys_path: PathBuf,
    fingerprint: [u8; 32],
    max_run: Duration,
    audit: Arc<Audit>,
    limiter: Limiter,
    run: RunState,
    spawns: SpawnState,
    listen: SocketAddr,
}

fn authenticate(ctx: &Ctx, tls: &TlsServer, tcp: TcpStream, peer: SocketAddr) -> Option<(Conn, Psk)> {
    let peer_s = peer.to_string();
    let _ = tcp.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let _ = tcp.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
    let fail = |why: &str| {
        let banned = ctx.limiter.fail(peer.ip(), Instant::now());
        ctx.audit.log(&peer_s, "-", "auth-fail", why);
        if banned {
            ctx.audit.log(&peer_s, "-", "ban", &format!("{} s after {MAX_FAILURES} failures", BAN_TIME.as_secs()));
        }
    };
    // Port probes (connect, close) and pre-TLS clients are not credential
    // guesses: they are dropped without counting toward a ban.
    let mut first = [0u8; 1];
    match tcp.peek(&mut first) {
        Ok(1) if first[0] == 0x16 => {}
        Ok(1) if (TAG_FILE_DATA..=TAG_SET_KEYS).contains(&first[0]) => {
            let msg = b"this tunnel requires TLS and a key: update makepad-remote / cargo-makepad (see tools/remote/TUNNEL.md)";
            let mut frame = vec![TAG_ERROR];
            frame.extend_from_slice(&(msg.len() as u32).to_be_bytes());
            frame.extend_from_slice(msg);
            let mut tcp = tcp;
            let _ = tcp.write_all(&frame);
            let _ = tcp.shutdown(Shutdown::Both);
            ctx.audit.log(&peer_s, "-", "refused", "pre-TLS client");
            return None;
        }
        Ok(1) => {}
        _ => return None,
    }
    let mut stream = match tls.accept(tcp) {
        Ok(s) => s,
        Err(e) => {
            fail(&format!("tls: {e}"));
            return None;
        }
    };
    // Keys are re-read per connection, so a rotation needs no restart.
    let keys = match load_server_keys(&ctx.keys_path) {
        Ok(k) => k,
        Err(e) => {
            ctx.audit.log(&peer_s, "-", "auth-fail", &format!("server keys unreadable: {e}"));
            return None;
        }
    };
    match server_authenticate(&mut stream, &keys, &ctx.fingerprint) {
        Ok(key) => {
            ctx.limiter.succeed(peer.ip());
            ctx.audit.log(&peer_s, &key.id_hex(), "auth-ok", "");
            let _ = stream.set_read_timeout(Some(IDLE_TIMEOUT));
            let _ = stream.set_write_timeout(None);
            Some((Arc::new(Mutex::new(stream)), key))
        }
        Err(e) => {
            fail(&e.to_string());
            None
        }
    }
}

fn send(conn: &Conn, tag: u8, payload: &[u8]) -> io::Result<()> {
    let mut guard = conn.lock().unwrap();
    write_msg(&mut *guard, tag, payload)
}

fn reply_text(conn: &Conn, text: &str) -> io::Result<()> {
    let mut payload = Vec::with_capacity(1 + text.len());
    payload.push(STREAM_STDOUT);
    payload.extend_from_slice(text.as_bytes());
    send(conn, TAG_OUTPUT, &payload)?;
    send(conn, TAG_EXIT_CODE, &0i32.to_be_bytes())
}

fn reply_error(conn: &Conn, msg: &str) {
    let _ = send(conn, TAG_ERROR, msg.as_bytes());
}

fn validate_and_resolve_path(cwd: &Path, rel_path: &str) -> io::Result<PathBuf> {
    let rel = Path::new(rel_path);
    if rel.is_absolute() || rel_path.contains(':') {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "absolute paths not allowed"));
    }
    if rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, ".. not allowed in paths"));
    }
    let full = cwd.join(rel);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)?;
        if !parent.canonicalize()?.starts_with(cwd) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("path escapes working directory: {rel_path}"),
            ));
        }
    }
    Ok(full)
}

fn handle_session(ctx: &Ctx, conn: Conn, key: &Psk, peer: SocketAddr) -> io::Result<()> {
    let peer_s = peer.to_string();
    let key_id = key.id_hex();
    let audit = |event: &str, detail: &str| ctx.audit.log(&peer_s, &key_id, event, detail);
    let need_all = |what: &str| -> bool {
        if !ctx.allow_all {
            audit("denied", &format!("{what} (server not started with --all)"));
            reply_error(&conn, &format!("{what} not allowed (server not started with --all)"));
            return false;
        }
        true
    };
    let (run_args, is_shell): (Vec<String>, bool) = loop {
        let (tag, payload) = {
            let mut guard = conn.lock().unwrap();
            match read_msg(&mut *guard) {
                Ok(m) => m,
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
                Err(e) => return Err(e),
            }
        };
        match tag {
            TAG_FILE_DATA => {
                let (rel_path, data) = decode_file_data(&payload)?;
                let full_path = validate_and_resolve_path(&ctx.cwd, rel_path)?;
                fs::write(&full_path, data)?;
                audit("push", &format!("{rel_path} ({} bytes)", data.len()));
            }
            TAG_FILE_PULL => {
                let rel_path = String::from_utf8_lossy(&payload).to_string();
                let full_path = validate_and_resolve_path(&ctx.cwd, &rel_path)?;
                audit("pull", &rel_path);
                match fs::read(&full_path) {
                    Ok(data) => {
                        send(&conn, TAG_FILE_DATA, &encode_file_data(&rel_path, &data))?;
                        send(&conn, TAG_EXIT_CODE, &0i32.to_be_bytes())?;
                    }
                    Err(e) => reply_error(&conn, &format!("pull {rel_path}: {e}")),
                }
                return Ok(());
            }
            TAG_SPAWN => {
                if !need_all("spawn") {
                    return Ok(());
                }
                let command = String::from_utf8_lossy(&payload).trim().to_string();
                if command.is_empty() {
                    reply_error(&conn, "spawn requires a command");
                    return Ok(());
                }
                audit("spawn", &command);
                match spawn_detached(&ctx.cwd, &command) {
                    Ok((pid, log)) => {
                        if let Ok(mut list) = ctx.spawns.lock() {
                            list.retain(|j| procs::is_alive(j.pid));
                            list.push(SpawnRecord { pid, command: command.clone(), log: log.clone() });
                        }
                        reply_text(&conn, &format!("pid={pid}\nlog={}\n", log.display()))?;
                    }
                    Err(e) => reply_error(&conn, &format!("spawn failed: {e}")),
                }
                return Ok(());
            }
            TAG_PS => {
                if !need_all("ps") {
                    return Ok(());
                }
                let filter = String::from_utf8_lossy(&payload).trim().to_string();
                audit("ps", &filter);
                match format_process_list(&ctx.spawns, &filter) {
                    Ok(text) => reply_text(&conn, &text)?,
                    Err(e) => reply_error(&conn, &format!("ps failed: {e}")),
                }
                return Ok(());
            }
            TAG_KILL => {
                if !need_all("kill") {
                    return Ok(());
                }
                audit("kill", &String::from_utf8_lossy(&payload));
                match parse_kill_payload(&payload) {
                    Ok((pid, tree)) => match do_kill(&ctx.spawns, pid, tree) {
                        Ok(text) => reply_text(&conn, &text)?,
                        Err(e) => reply_error(&conn, &format!("kill failed: {e}")),
                    },
                    Err(e) => reply_error(&conn, &e),
                }
                return Ok(());
            }
            TAG_ADMIN => {
                let name = String::from_utf8_lossy(&payload).trim().to_string();
                match admin_action(&name) {
                    Err(e) => {
                        audit("admin-denied", &name);
                        reply_error(&conn, &e);
                    }
                    Ok(AdminAction::RestartTunnel) => {
                        audit("admin", &name);
                        reply_text(&conn, "tunnel restarting\n")?;
                        RESTART.store(true, Ordering::SeqCst);
                        // Wake the accept loop so it sees the flag.
                        let _ = TcpStream::connect_timeout(&ctx.listen, Duration::from_secs(2));
                    }
                    Ok(AdminAction::Run(argv)) => {
                        audit("admin", &name);
                        let out = Command::new(argv[0]).args(&argv[1..]).current_dir(&ctx.cwd).output();
                        match out {
                            Ok(out) => {
                                let mut text = String::from_utf8_lossy(&out.stdout).to_string();
                                text.push_str(&String::from_utf8_lossy(&out.stderr));
                                let mut payload = vec![STREAM_STDOUT];
                                payload.extend_from_slice(text.as_bytes());
                                send(&conn, TAG_OUTPUT, &payload)?;
                                send(&conn, TAG_EXIT_CODE, &out.status.code().unwrap_or(1).to_be_bytes())?;
                            }
                            Err(e) => reply_error(&conn, &format!("{name}: {e}")),
                        }
                    }
                }
                return Ok(());
            }
            TAG_SET_KEYS => {
                let text = String::from_utf8_lossy(&payload).to_string();
                match set_keys(&ctx.keys_path, &text, key) {
                    Ok(ids) => {
                        audit("set-keys", &ids);
                        reply_text(&conn, &format!("keys now: {ids}\n"))?;
                    }
                    Err(e) => {
                        audit("set-keys-denied", &e);
                        reply_error(&conn, &e);
                    }
                }
                return Ok(());
            }
            TAG_CARGO_RUN | TAG_SHELL_RUN => {
                let is_shell = tag == TAG_SHELL_RUN;
                if is_shell && !need_all("shell") {
                    return Ok(());
                }
                let args: Vec<String> = String::from_utf8_lossy(&payload)
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect();
                audit(if is_shell { "shell" } else { "cargo" }, &args.join(" "));
                break (args, is_shell);
            }
            _ => {
                reply_error(&conn, &format!("unknown tag: 0x{tag:02x}"));
                return Ok(());
            }
        }
    };
    run_foreground(ctx, &conn, run_args, is_shell, &peer_s, &key_id)
}

/// Replaces the server's key file; the new set must keep the caller's key.
pub fn set_keys(path: &Path, text: &str, current: &Psk) -> Result<String, String> {
    let keys = parse_key_file(text).map_err(|e| format!("rejected key file: {e}"))?;
    if keys.len() > 4 {
        return Err("rejected key file: more than 4 keys".into());
    }
    if !keys.contains(current) {
        return Err("rejected key file: it must contain the key of this connection (no lock-out)".into());
    }
    makepad_network::tls::write_private(path, key_file_text(&keys).as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(keys.iter().map(|k| k.id_hex()).collect::<Vec<_>>().join(","))
}

fn run_foreground(ctx: &Ctx, conn: &Conn, run_args: Vec<String>, is_shell: bool, peer: &str, key_id: &str) -> io::Result<()> {
    kill_running(&ctx.run);
    let mut cmd = if is_shell {
        let line = run_args.join(" ");
        #[cfg(unix)]
        let mut c = Command::new("sh");
        #[cfg(unix)]
        c.arg("-c").arg(&line);
        #[cfg(windows)]
        let mut c = Command::new("cmd");
        #[cfg(windows)]
        c.arg("/C").arg(&line);
        c
    } else {
        let mut c = Command::new("cargo");
        c.args(&run_args);
        c
    };
    cmd.current_dir(&ctx.cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    process_group::configure_command(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            reply_error(conn, &format!("failed to start: {e}"));
            return Ok(());
        }
    };
    let mut job = process_group::JobHandle::new()?;
    job.assign(&child)?;
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let id = RUN_SEQ.fetch_add(1, Ordering::SeqCst);
    let tcp = conn.lock().unwrap().try_clone_tcp()?;
    *ctx.run.lock().unwrap() = Some(RunningCommand { id, child, job, stream: tcp });

    // The time limit: the same run still going after max_run is stopped.
    {
        let run = ctx.run.clone();
        let max = ctx.max_run;
        let audit = ctx.audit.clone();
        let (peer, key_id) = (peer.to_string(), key_id.to_string());
        thread::spawn(move || {
            thread::sleep(max);
            let mut lock = run.lock().unwrap();
            if lock.as_ref().is_some_and(|r| r.id == id) {
                let r = lock.as_mut().unwrap();
                r.job.terminate();
                audit.log(&peer, &key_id, "timeout", &format!("stopped after {} s", max.as_secs()));
            }
        });
    }

    let c1 = conn.clone();
    let c2 = conn.clone();
    let t1 = thread::spawn(move || pipe_output(out, c1, STREAM_STDOUT));
    let t2 = thread::spawn(move || pipe_output(err, c2, STREAM_STDERR));
    let _ = t1.join();
    let _ = t2.join();

    let exit_code = {
        let mut lock = ctx.run.lock().unwrap();
        match lock.as_mut() {
            Some(r) if r.id == id => {
                let code = r.child.wait().ok().and_then(|s| s.code()).unwrap_or(1);
                *lock = None;
                code
            }
            _ => 137,
        }
    };
    ctx.audit.log(peer, key_id, "exit", &exit_code.to_string());
    let _ = send(conn, TAG_EXIT_CODE, &exit_code.to_be_bytes());
    conn.lock().unwrap().shutdown();
    Ok(())
}

fn pipe_output(reader: impl Read, conn: Conn, stream_id: u8) {
    let mut reader = BufReader::new(reader);
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut payload = Vec::with_capacity(1 + n);
                payload.push(stream_id);
                payload.extend_from_slice(&buf[..n]);
                if send(&conn, TAG_OUTPUT, &payload).is_err() {
                    break;
                }
            }
        }
    }
}

fn format_process_list(spawns: &SpawnState, filter: &str) -> io::Result<String> {
    let procs = procs::list_processes()?;
    let spawned = spawns
        .lock()
        .map(|mut list| {
            list.retain(|j| procs::is_alive(j.pid));
            list.clone()
        })
        .unwrap_or_default();
    let filter = filter.to_ascii_lowercase();
    let mut lines = Vec::new();
    for proc in procs {
        let matches_spawn = spawned
            .iter()
            .any(|j| j.pid == proc.pid && j.command.to_ascii_lowercase().contains(&filter));
        if !filter.is_empty() && !proc.name.to_ascii_lowercase().contains(&filter) && !matches_spawn {
            continue;
        }
        let mut line = format!("pid={} ppid={} name={}", proc.pid, proc.ppid, proc.name);
        if let Some(rec) = spawned.iter().find(|j| j.pid == proc.pid) {
            line.push_str(&format!(
                " spawned=yes log={} cmd={}",
                rec.log.display(),
                rec.command.replace('\n', " ")
            ));
        } else {
            line.push_str(" spawned=no");
        }
        lines.push(line);
    }
    lines.sort();
    if lines.is_empty() {
        lines.push("(none)".into());
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn parse_kill_payload(payload: &[u8]) -> Result<(u32, bool), String> {
    let text = std::str::from_utf8(payload).map_err(|_| "kill pid must be utf-8".to_string())?;
    let mut lines = text.lines().filter(|l| !l.is_empty());
    let pid = lines
        .next()
        .ok_or_else(|| "kill requires a pid".to_string())?
        .trim()
        .parse::<u32>()
        .map_err(|_| "kill pid must be a number".to_string())?;
    let tree = lines.any(|l| l.trim().eq_ignore_ascii_case("tree"));
    Ok((pid, tree))
}

fn do_kill(spawns: &SpawnState, pid: u32, tree: bool) -> io::Result<String> {
    let killed = if tree {
        procs::kill_tree(pid)?
    } else {
        procs::kill_pid(pid)?;
        vec![pid]
    };
    if let Ok(mut list) = spawns.lock() {
        list.retain(|j| !killed.contains(&j.pid) && procs::is_alive(j.pid));
    }
    Ok(format!("killed {}\n", killed.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(" ")))
}

static SPAWN_SEQ: AtomicU64 = AtomicU64::new(0);

fn spawn_detached(cwd: &Path, command: &str) -> io::Result<(u32, PathBuf)> {
    let job_dir = cwd.join("remote-jobs");
    fs::create_dir_all(&job_dir)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let seq = SPAWN_SEQ.fetch_add(1, Ordering::Relaxed);
    let log_path = job_dir.join(format!("{stamp}-{seq}.log"));
    let log = fs::File::create(&log_path)?;
    {
        let mut header = log.try_clone()?;
        let _ = writeln!(header, "spawn {stamp}\ncmd: {command}\n---");
    }
    let build = |log: fs::File| -> io::Result<Command> {
        let log_err = log.try_clone()?;
        #[cfg(unix)]
        let mut c = {
            let mut c = Command::new("sh");
            c.arg("-c").arg(command);
            c
        };
        #[cfg(windows)]
        let mut c = {
            let mut c = Command::new("powershell");
            c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", command]);
            c
        };
        c.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::from(log)).stderr(Stdio::from(log_err));
        Ok(c)
    };
    let mut cmd = build(log)?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000 | 0x0000_0200 | 0x0100_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                extern "C" {
                    fn setpgid(pid: i32, pgid: i32) -> i32;
                }
                setpgid(0, 0);
                Ok(())
            });
        }
    }
    let child = match cmd.spawn() {
        Ok(child) => child,
        #[cfg(windows)]
        Err(first) => {
            // Some hosts reject BREAKAWAY when the parent is not in a job.
            use std::os::windows::process::CommandExt;
            let log = fs::OpenOptions::new().append(true).open(&log_path)?;
            let mut retry = build(log)?;
            retry.creation_flags(0x0800_0000 | 0x0000_0200);
            retry
                .spawn()
                .map_err(|e2| io::Error::new(e2.kind(), format!("spawn retry without breakaway: {e2} (first: {first})")))?
        }
        #[cfg(not(windows))]
        Err(e) => return Err(e),
    };
    let pid = child.id();
    // The process outlives this connection on purpose.
    std::mem::forget(child);
    Ok((pid, log_path))
}

/// After `tunnel-restart`: under a supervisor (MAKEPAD_REMOTE_SUPERVISED=1,
/// set by the service config) just exit and be restarted; otherwise start a
/// fresh copy of this executable with the same arguments, detached.
fn restart_self() -> io::Result<()> {
    if env::var_os("MAKEPAD_REMOTE_SUPERVISED").is_some() {
        std::process::exit(0);
    }
    let exe = env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.args(env::args_os().skip(1)).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000 | 0x0000_0200 | 0x0100_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                extern "C" {
                    fn setsid() -> i32;
                }
                setsid();
                Ok(())
            });
        }
    }
    // Give the reply time to flush, and the listener time to close.
    thread::sleep(Duration::from_millis(300));
    let child = cmd.spawn()?;
    eprintln!("server: restarted as pid {}", child.id());
    std::mem::forget(child);
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_is_fixed() {
        assert_eq!(admin_action("tunnel-restart"), Ok(AdminAction::RestartTunnel));
        assert!(admin_action("shell").is_err());
        assert!(admin_action("node-restart; del C:\\").is_err());
        assert!(admin_action("Restart-Service MakepadAiNode").is_err());
        if cfg!(windows) {
            assert!(matches!(admin_action("node-restart"), Ok(AdminAction::Run(_))));
        } else {
            let e = admin_action("node-restart").unwrap_err();
            assert!(e.contains("not available"));
        }
        // No action takes client text into its command line.
        for name in ADMIN_ACTIONS {
            if let Ok(AdminAction::Run(argv)) = admin_action(name) {
                assert!(argv.iter().all(|a| !a.contains("{}")));
            }
        }
    }

    #[test]
    fn limiter_bans_after_repeated_failures() {
        let l = Limiter::new();
        let ip: IpAddr = "10.0.0.9".parse().unwrap();
        let other: IpAddr = "10.0.0.10".parse().unwrap();
        let t0 = Instant::now();
        for i in 0..MAX_FAILURES - 1 {
            assert!(!l.fail(ip, t0 + Duration::from_secs(i as u64)));
        }
        assert!(!l.banned(ip, t0));
        assert!(l.fail(ip, t0 + Duration::from_secs(10)));
        assert!(l.banned(ip, t0 + Duration::from_secs(11)));
        assert!(!l.banned(other, t0 + Duration::from_secs(11)));
        assert!(!l.banned(ip, t0 + BAN_TIME + Duration::from_secs(11)));
        // Failures spread beyond the window never add up to a ban.
        let l = Limiter::new();
        for i in 0..20u64 {
            assert!(!l.fail(ip, t0 + FAILURE_WINDOW * (i as u32)));
        }
    }

    #[test]
    fn lan_scope() {
        for ok in ["10.0.0.5", "192.168.1.2", "172.16.4.4", "127.0.0.1", "::1", "fd00::1", "fe80::1"] {
            assert!(is_lan(ok.parse().unwrap()), "{ok}");
        }
        for bad in ["8.8.8.8", "172.32.0.1", "2001:db8::1", "::ffff:8.8.8.8"] {
            assert!(!is_lan(bad.parse().unwrap()), "{bad}");
        }
        assert!(resolve_bind("0.0.0.0").is_err());
        assert!(resolve_bind("8.8.8.8").is_err());
        assert_eq!(resolve_bind("127.0.0.1").unwrap(), "127.0.0.1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn audit_escapes_control_characters() {
        assert_eq!(escape("a\nb\rc\\"), "a\\nb\\rc\\\\");
        assert_eq!(escape("x\u{1b}y"), "x\\u{1b}y");
    }

    #[test]
    fn set_keys_never_locks_out() {
        let dir = std::env::temp_dir().join(format!("mkrt-keys-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keys");
        let current = Psk::generate().unwrap();
        let next = Psk::generate().unwrap();
        assert!(set_keys(&path, &key_file_text(&[next.clone()]), &current).is_err());
        assert!(set_keys(&path, "garbage", &current).is_err());
        assert!(set_keys(&path, &key_file_text(&[next.clone(), current.clone()]), &current).is_ok());
        assert_eq!(load_server_keys(&path).unwrap(), vec![next, current]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths_stay_in_cwd() {
        let dir = std::env::temp_dir().join(format!("mkrt-cwd-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let cwd = dir.canonicalize().unwrap();
        assert!(validate_and_resolve_path(&cwd, "a/b.txt").is_ok());
        assert!(validate_and_resolve_path(&cwd, "../x").is_err());
        assert!(validate_and_resolve_path(&cwd, "/etc/passwd").is_err());
        assert!(validate_and_resolve_path(&cwd, "C:\\x").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn utc_format() {
        let s = utc_now();
        assert_eq!(s.len(), 20);
        assert!(s.ends_with('Z'));
    }
}
