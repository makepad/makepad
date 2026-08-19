//! The assembled runtime and its embedding boundary.
//!
//! `AssetServer::start(config)` brings up a complete standalone service —
//! state thread over the headless core, bounded HTTP control and data
//! planes, lease janitor, optional UDP discovery beacon — and returns a
//! handle whose `shutdown()` (also run on drop) tears everything down and
//! joins every thread. Nothing is global: embedders (the Asset Store, tests)
//! run several instances in one process on distinct roots.
//!
//! Single-writer law: one process serves one root. A `server.lock` advisory
//! file lock enforces it — the transport's job-routing metadata assumes it is
//! the only enqueuer, and two processes over one WAL catalog would be two
//! sources of recovery truth.

use super::config::ServerConfig;
use super::discovery::{load_or_create_server_id, spawn_beacon, Beacon, PROTOCOL_VERSION};
use super::http::{Conn, HeadError, Method, Resp};
use super::routes::{serve_request, Outcome, Plane, RouteCtx};
use super::state::{spawn_state, StateHandle};
use super::util::{log, now_ms, rand32, to_hex};
use crate::{
    token_hash, Capability, PrincipalId, RecoverReport, Scope, ServerError, ServerResult,
};
use std::fs::File;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

pub struct AssetServer {
    control_addr: SocketAddr,
    data_addr: SocketAddr,
    /// Connections accepted per plane since start. A keep-alive client
    /// serves many requests per connection, so this is how an operator (and
    /// the tests) can SEE that reuse is really happening rather than assume
    /// it: thirty thumbnails should cost one accept, not thirty.
    accepted: [Arc<std::sync::atomic::AtomicU64>; 2],
    server_id: [u8; 16],
    recover: RecoverReport,
    stop: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
    janitor: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
    beacon: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
    state: Option<StateHandle>,
    state_join: Option<JoinHandle<()>>,
    events: Arc<super::events::EventHub>,
    chat: Option<(super::chat::ChatHandle, JoinHandle<()>)>,
    /// Held for the server's whole life; the OS releases it on process death.
    _root_lock: File,
    log_enabled: bool,
}

impl AssetServer {
    pub fn start(cfg: ServerConfig) -> ServerResult<AssetServer> {
        cfg.validate()?;
        std::fs::create_dir_all(&cfg.root)
            .map_err(|e| ServerError::Io { op: "create server root", kind: e.kind() })?;
        let root_lock = lock_root(&cfg)?;

        let (state, recover, state_join) =
            spawn_state(cfg.root.clone(), cfg.budgets, cfg.log)?;
        if cfg.bootstrap_admin {
            bootstrap_admin(&state, &cfg)?;
        }
        let server_id = load_or_create_server_id(&cfg.root, cfg.log)?;

        let control =
            bind(cfg.control_addr).map_err(|e| ServerError::Io { op: "bind control plane", kind: e.kind() })?;
        let data =
            bind(cfg.data_addr).map_err(|e| ServerError::Io { op: "bind data plane", kind: e.kind() })?;
        let control_addr = control
            .local_addr()
            .map_err(|e| ServerError::Io { op: "control local addr", kind: e.kind() })?;
        let data_addr = data
            .local_addr()
            .map_err(|e| ServerError::Io { op: "data local addr", kind: e.kind() })?;

        let stop = Arc::new(AtomicBool::new(false));
        let cfg = Arc::new(cfg);
        // The journal epoch is fresh randomness per process life: a resume
        // cursor from a previous life can only ever produce an explicit
        // `gap`, never silently alias onto this life's sequences.
        let epoch: [u8; 8] = rand32()?[..8].try_into().expect("8 bytes");
        let events = Arc::new(super::events::EventHub::new(
            epoch,
            cfg.event_journal_cap,
            cfg.event_max_waiters,
        ));
        let endpoints = makepad_asset_client::ApiEndpoints {
            control: control_addr,
            data: data_addr,
        };
        let (chat_handle, chat_join) = super::chat::spawn_broker(
            endpoints,
            cfg.chat.clone(),
            cfg.log,
            stop.clone(),
        )?;
        let rc = RouteCtx {
            state: state.clone(),
            cfg: cfg.clone(),
            server_id,
            events: events.clone(),
            op_event_waiters: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            chat: Some(chat_handle.clone()),
            chat_event_waiters: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            profiles: std::sync::Arc::new(super::profiles::ProfileRegistry::new()),
        };

        // One acceptor per plane; each connection gets its own thread up to
        // the plane's hard cap. The acceptor joins its connection threads
        // before exiting, so shutdown only has to join the acceptors.
        let mut workers = Vec::with_capacity(2);
        let accepted: [Arc<std::sync::atomic::AtomicU64>; 2] = [
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
        ];
        for (n, (plane, listener, max_conns)) in [
            (Plane::Control, control, cfg.control_max_conns),
            (Plane::Data, data, cfg.data_max_conns),
        ]
        .into_iter()
        .enumerate()
        {
            let rc = rc.clone();
            let stop = stop.clone();
            let counter = accepted[n].clone();
            let name = format!(
                "asset-server-{}-accept",
                if plane == Plane::Control { "control" } else { "data" }
            );
            workers.push(
                std::thread::Builder::new()
                    .name(name)
                    .spawn(move || accept_loop(listener, rc, plane, max_conns, &stop, counter))
                    .map_err(|e| ServerError::Io { op: "spawn plane acceptor", kind: e.kind() })?,
            );
        }

        let janitor = Some(spawn_janitor(
            state.clone(),
            cfg.janitor_interval_ms,
            cfg.gc_janitor_steps,
        )?);

        let beacon = match &cfg.discovery {
            None => None,
            Some(d) => {
                let payload = Beacon {
                    protocol_version: PROTOCOL_VERSION,
                    server_id,
                    control_port: control_addr.port(),
                    data_port: data_addr.port(),
                    // Every route except /v1/health requires a bearer token,
                    // and this transport speaks plain HTTP.
                    auth_required: true,
                    tls: false,
                    capability_bits: d.capability_bits,
                };
                Some(spawn_beacon(payload, d.target_ip, d.port, d.interval_ms, cfg.log)?)
            }
        };

        write_listen_file(&cfg.root, control_addr, data_addr);
        log(
            cfg.log,
            &format!(
                "asset server up: control {control_addr}, data {data_addr}, server_id {}, recovered {} cas temps / {} leases",
                to_hex(&server_id),
                recover.cas_temps_removed,
                recover.leases_expired
            ),
        );
        Ok(AssetServer {
            control_addr,
            data_addr,
            accepted,
            server_id,
            recover,
            stop,
            workers,
            janitor,
            beacon,
            state: Some(state),
            state_join: Some(state_join),
            events,
            chat: Some((chat_handle, chat_join)),
            _root_lock: root_lock,
            log_enabled: cfg.log,
        })
    }

    pub fn control_addr(&self) -> SocketAddr {
        self.control_addr
    }

    pub fn data_addr(&self) -> SocketAddr {
        self.data_addr
    }

    /// Connections accepted on the control plane since start.
    pub fn control_connections_accepted(&self) -> u64 {
        self.accepted[0].load(Ordering::Relaxed)
    }

    /// Connections accepted on the data plane since start.
    pub fn data_connections_accepted(&self) -> u64 {
        self.accepted[1].load(Ordering::Relaxed)
    }

    pub fn server_id(&self) -> [u8; 16] {
        self.server_id
    }

    /// What restart recovery found when this instance opened its root.
    pub fn recover_report(&self) -> RecoverReport {
        self.recover
    }

    /// Stop accepting, finish in-flight requests, and join every thread.
    /// Idempotent; also runs on drop.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake parked `/v1/events` long-polls FIRST: their connection
        // threads must answer and exit before the acceptor joins them.
        self.events.shutdown();
        for j in self.workers.drain(..) {
            let _ = j.join();
        }
        if let Some((tx, j)) = self.janitor.take() {
            let _ = tx.send(());
            let _ = j.join();
        }
        if let Some((tx, j)) = self.beacon.take() {
            let _ = tx.send(());
            let _ = j.join();
        }
        if let Some((handle, join)) = self.chat.take() {
            handle.shutdown();
            let _ = join.join();
        }
        // Workers are gone, so this drop releases the last handle clones and
        // the state thread's task channel closes.
        drop(self.state.take());
        if let Some(j) = self.state_join.take() {
            let _ = j.join();
            log(self.log_enabled, "asset server stopped");
        }
    }
}

impl Drop for AssetServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Bind addresses for a live server (`ip:control:data`). Clients that lose
/// the root lock read this instead of treating the catalog as unusable.
pub const LISTEN_FILE: &str = "listen";

fn write_listen_file(root: &std::path::Path, control: SocketAddr, data: SocketAddr) {
    let ip = if control.ip().is_unspecified() {
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    } else {
        control.ip()
    };
    let _ = std::fs::write(
        root.join(LISTEN_FILE),
        format!("{ip}:{}:{}\n", control.port(), data.port()),
    );
}

/// Take the exclusive advisory lock on `<root>/server.lock`.
fn lock_root(cfg: &ServerConfig) -> ServerResult<File> {
    let path = cfg.root.join("server.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| ServerError::Io { op: "open root lock", kind: e.kind() })?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(_) => Err(ServerError::InvalidState {
            what: "server root",
            state: "locked by another server process",
        }),
    }
}

fn bind(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let listener = TcpListener::bind(addr)?;
    // Nonblocking accept + short sleep lets workers notice the stop flag
    // without a wake-up connection dance.
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn accept_loop(
    listener: TcpListener,
    rc: RouteCtx,
    plane: Plane,
    max_conns: usize,
    stop: &Arc<AtomicBool>,
    accepted: Arc<std::sync::atomic::AtomicU64>,
) {
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut joins: Vec<JoinHandle<()>> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        // Reap finished connection threads so the handle list stays bounded
        // by the live-connection cap.
        joins.retain(|j| !j.is_finished());
        match listener.accept() {
            Ok((stream, _peer)) => {
                if active.load(Ordering::Relaxed) >= max_conns {
                    refuse_over_capacity(stream);
                    continue;
                }
                active.fetch_add(1, Ordering::Relaxed);
                accepted.fetch_add(1, Ordering::Relaxed);
                let rc = rc.clone();
                let stop = stop.clone();
                let conn_active = active.clone();
                let spawned = std::thread::Builder::new()
                    .name("asset-server-conn".into())
                    .spawn(move || {
                        serve_conn(stream, &rc, plane, &stop);
                        conn_active.fetch_sub(1, Ordering::Relaxed);
                    });
                match spawned {
                    Ok(j) => joins.push(j),
                    Err(_) => {
                        active.fetch_sub(1, Ordering::Relaxed);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                // Transient accept failure (fd pressure, aborted handshake):
                // back off instead of spinning.
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    // Stopping: connection threads notice the flag between requests and
    // within an idle-wait slice; join them all before the acceptor exits.
    for j in joins {
        let _ = j.join();
    }
}

/// The over-capacity refusal: explicit and immediate, never a silent queue.
fn refuse_over_capacity(stream: TcpStream) {
    let _ = stream.set_nonblocking(false);
    let mut conn = match Conn::new(stream, 1000, 1000) {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = conn.write_resp(false, &Resp::error(503, "connection capacity").closing());
}

fn serve_conn(stream: TcpStream, rc: &RouteCtx, plane: Plane, stop: &AtomicBool) {
    // The accepted socket may inherit the listener's nonblocking mode on some
    // platforms; timeouts require blocking mode.
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let cfg = &rc.cfg;
    let mut conn = match Conn::new(stream, cfg.read_timeout_ms, cfg.write_timeout_ms) {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut served: u32 = 0;
    loop {
        let mut head = match conn.next_request(cfg.head_deadline_ms, cfg.keepalive_idle_ms, stop) {
            Ok(head) => head,
            Err(HeadError::Closed) | Err(HeadError::Io) => return,
            Err(HeadError::Timeout) => {
                let _ = conn.write_resp(false, &Resp::error(408, "head timeout").closing());
                return;
            }
            Err(HeadError::Bad(status, msg)) => {
                let _ = conn.write_resp(false, &Resp::error(status, msg).closing());
                return;
            }
        };
        served += 1;
        let force_close =
            served >= cfg.max_requests_per_conn || stop.load(Ordering::Relaxed);
        match serve_request(&mut conn, &mut head, rc, plane, force_close) {
            Outcome::Hangup => return,
            Outcome::Streamed { close } => {
                // Streamed responses only exist on GET/HEAD, whose requests
                // carry no body; the boundary is already clean.
                if close || !conn.finish_request(&mut head) {
                    return;
                }
            }
            Outcome::Resp(mut resp) => {
                let keep = conn.finish_request(&mut head) && !resp.close && !force_close;
                resp.close = !keep;
                let is_head = head.method == Method::Head;
                if conn.write_resp(is_head, &resp).is_err() || resp.close {
                    return;
                }
            }
        }
    }
}

fn spawn_janitor(
    state: StateHandle,
    interval_ms: u64,
    gc_steps: u32,
) -> ServerResult<(mpsc::Sender<()>, JoinHandle<()>)> {
    let (tx, rx) = mpsc::channel::<()>();
    let interval = Duration::from_millis(interval_ms.max(1));
    let join = std::thread::Builder::new()
        .name("asset-server-janitor".into())
        .spawn(move || loop {
            match rx.recv_timeout(interval) {
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let now = now_ms();
                    // Poisoned state returns None; keep ticking, health
                    // reports the outage.
                    let _ = state.call(move |ctx| ctx.core.jobs().expire_leases(now));
                    // A GC run finishes even if nobody polls it: a bounded
                    // number of steps per tick, each one transaction over
                    // one batch, interleaved with every other state call.
                    if gc_steps > 0 {
                        let _ = state.call(move |ctx| ctx.core.gc_advance(gc_steps, now));
                    }
                }
                _ => break,
            }
        })
        .map_err(|e| ServerError::Io { op: "spawn janitor", kind: e.kind() })?;
    Ok((tx, join))
}

/// Ensure the bootstrap admin principal exists with a wildcard `auth_admin`
/// grant, and that `<root>/admin-token` holds a working secret for it. The
/// secret only ever exists in that 0600 file and in issuance responses; the
/// stores hold hashes.
fn bootstrap_admin(state: &StateHandle, cfg: &ServerConfig) -> ServerResult<()> {
    let now = now_ms();
    let minted = PrincipalId(super::util::rand16()?);
    let root = state
        .call(move |ctx| -> ServerResult<PrincipalId> {
            if let Some(existing) = ctx.root_admin_get()? {
                return Ok(existing);
            }
            ctx.core.auth().create_principal(&minted, "root-admin", now)?;
            ctx.core.auth().grant(&minted, Capability::AuthAdmin, Scope::All, now)?;
            ctx.root_admin_set(&minted)?;
            Ok(minted)
        })
        .ok_or(ServerError::InvalidState { what: "state thread", state: "unavailable" })??;

    let token_path = cfg.root.join("admin-token");
    if let Ok(existing) = std::fs::read_to_string(&token_path) {
        let candidate = existing.trim().to_string();
        if !candidate.is_empty() {
            let probe = candidate.clone();
            let still_valid = state
                .call(move |ctx| -> ServerResult<bool> {
                    match ctx.core.auth().authenticate(probe.as_bytes(), now) {
                        Ok(p) => Ok(p == root),
                        Err(ServerError::Unauthenticated) => Ok(false),
                        Err(e) => Err(e),
                    }
                })
                .ok_or(ServerError::InvalidState { what: "state thread", state: "unavailable" })??;
            if still_valid {
                return Ok(());
            }
            log(cfg.log, "admin-token no longer valid; minting a replacement");
        }
    }

    let secret = format!("{}{}", super::api::TOKEN_PREFIX, to_hex(&rand32()?));
    let hash = token_hash(secret.as_bytes());
    let expires = now
        .checked_add(cfg.admin_token_ttl_ms)
        .ok_or(ServerError::InvalidInput { what: "admin token ttl overflow" })?;
    state
        .call(move |ctx| ctx.core.auth().register_token(&root, &hash, expires, now))
        .ok_or(ServerError::InvalidState { what: "state thread", state: "unavailable" })??;
    write_secret_file(&token_path, &secret)?;
    log(cfg.log, &format!("admin token written to {}", token_path.display()));
    Ok(())
}

/// Write a secret to a file only the owner can read.
fn write_secret_file(path: &std::path::Path, secret: &str) -> ServerResult<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .map_err(|e| ServerError::Io { op: "open admin-token", kind: e.kind() })?;
    f.write_all(format!("{secret}\n").as_bytes())
        .map_err(|e| ServerError::Io { op: "write admin-token", kind: e.kind() })?;
    f.sync_all()
        .map_err(|e| ServerError::Io { op: "sync admin-token", kind: e.kind() })
}
