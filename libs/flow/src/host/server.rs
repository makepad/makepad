use super::config::{FlowServerConfig, SharedConfig};
use super::events::EventHub;
use super::routes::{dispatch, Outcome, Plane, RouteCtx};
use super::state::{spawn_state, RunRegistration, StateHandle};
use super::util::{atomic_write, from_hex_16, log, random_16, random_32, random_u64, to_hex, write_secret_file};
use super::watcher::spawn_watcher;
use crate::engine;
use crate::engine::executors::publish::AssetWorker;
use makepad_bounded_http::{Conn, HeadError, Method, Resp};
use std::fs::File;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const HEAD_DEADLINE_MS: u64 = 10_000;
const KEEPALIVE_IDLE_MS: u64 = 30_000;
const READ_TIMEOUT_MS: u64 = 10_000;
const WRITE_TIMEOUT_MS: u64 = 10_000;
const MAX_REQUESTS_PER_CONN: u32 = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    pub control: SocketAddr,
    pub data: SocketAddr,
    pub server_id: [u8; 16],
    pub token: String,
}

#[derive(Debug)]
pub enum ServerError {
    Locked,
    InvalidConfig(&'static str),
    InvalidFile(&'static str),
    Io { op: &'static str, kind: std::io::ErrorKind },
    Prelude(crate::EvalError),
    StateUnavailable,
    Asset(String),
}

impl ServerError {
    pub(crate) fn io(op: &'static str, error: std::io::Error) -> Self {
        Self::Io { op, kind: error.kind() }
    }
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Locked => write!(formatter, "flow root is locked by another server"),
            Self::InvalidConfig(message) => write!(formatter, "invalid flow-server config: {message}"),
            Self::InvalidFile(message) => write!(formatter, "invalid flow-server file: {message}"),
            Self::Io { op, kind } => write!(formatter, "{op}: {kind:?}"),
            Self::Prelude(error) => write!(formatter, "flow prelude: {error}"),
            Self::StateUnavailable => write!(formatter, "flow state thread unavailable"),
            Self::Asset(error) => write!(formatter, "flow asset worker: {error}"),
        }
    }
}

impl std::error::Error for ServerError {}

pub struct FlowServer {
    endpoints: Endpoints,
    stop: Arc<AtomicBool>,
    acceptors: Vec<JoinHandle<()>>,
    watcher: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
    run_events: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
    janitor: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
    state: Option<StateHandle>,
    state_join: Option<JoinHandle<()>>,
    asset_worker: Option<AssetWorker>,
    events: Arc<EventHub>,
    config: SharedConfig,
    _root_lock: File,
}

impl FlowServer {
    pub fn start(config: FlowServerConfig) -> Result<Self, ServerError> {
        config.validate()?;
        std::fs::create_dir_all(&config.root)
            .map_err(|error| ServerError::io("create flow root", error))?;
        std::fs::create_dir_all(config.root.join("flows"))
            .map_err(|error| ServerError::io("create flows directory", error))?;
        std::fs::create_dir_all(config.root.join("log"))
            .map_err(|error| ServerError::io("create log directory", error))?;
        let root_lock = lock_root(&config)?;
        let server_id = load_or_create_server_id(&config)?;
        let token = load_or_create_token(&config)?;
        let epoch = random_u64()?;
        let events = Arc::new(EventHub::new(
            epoch,
            config.event_journal_cap,
            config.event_max_waiters,
        ));
        let control = bind(&config.control_addr, "bind control plane")?;
        let data = bind(&config.data_addr, "bind data plane")?;
        let control_addr = control
            .local_addr()
            .map_err(|error| ServerError::io("read control address", error))?;
        let data_addr = data
            .local_addr()
            .map_err(|error| ServerError::io("read data address", error))?;
        write_listen(&config, control_addr, data_addr)?;

        let asset_worker = AssetWorker::start(config.asset.clone()).map_err(ServerError::Asset)?;
        let asset_handle = asset_worker.handle();
        let config = Arc::new(config);
        let origin = (to_hex(&server_id), epoch);
        let (run_register_tx, run_register_rx) = mpsc::channel::<RunRegistration>();
        let (state, state_join) =
            spawn_state(
                config.clone(),
                events.clone(),
                epoch,
                origin,
                run_register_tx,
                asset_handle.clone(),
            )?;
        let run_events = spawn_run_events(run_register_rx, state.clone());
        let janitor = spawn_janitor(&config, state.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let route = RouteCtx {
            state: state.clone(),
            config: config.clone(),
            server_id,
            token: token.clone(),
            events: events.clone(),
            assets: asset_handle,
        };
        let mut watcher = match spawn_watcher(config.clone(), state.clone()) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                drop(route);
                stop_thread(janitor);
                stop_thread(run_events);
                drop(state);
                let _ = state_join.join();
                return Err(error);
            }
        };
        let mut acceptors = Vec::with_capacity(2);
        for (plane, listener, cap) in [
            (Plane::Control, control, config.control_max_conns),
            (Plane::Data, data, config.data_max_conns),
        ] {
            let thread_route = route.clone();
            let thread_stop = stop.clone();
            let name = match plane {
                Plane::Control => "flow-server-control-accept",
                Plane::Data => "flow-server-data-accept",
            };
            match std::thread::Builder::new().name(name.to_string()).spawn(move || {
                accept_loop(listener, thread_route, plane, cap, thread_stop)
            }) {
                Ok(join) => acceptors.push(join),
                Err(error) => {
                    stop.store(true, Ordering::SeqCst);
                    events.shutdown();
                    if let Some((stop_watcher, join)) = watcher.take() {
                        let _ = stop_watcher.send(());
                        let _ = join.join();
                    }
                    for join in acceptors {
                        let _ = join.join();
                    }
                    drop(route);
                    stop_thread(janitor);
                    stop_thread(run_events);
                    drop(state);
                    let _ = state_join.join();
                    return Err(ServerError::io("spawn acceptor thread", error));
                }
            }
        }
        // TODO(flow-discovery): `config.discovery` is intentionally inert until the beacon lane.
        log(
            &config,
            &format!(
                "up: control {control_addr}, data {data_addr}, server_id {}",
                to_hex(&server_id)
            ),
        );
        Ok(Self {
            endpoints: Endpoints { control: control_addr, data: data_addr, server_id, token },
            stop,
            acceptors,
            watcher,
            run_events: Some(run_events),
            janitor: Some(janitor),
            state: Some(state),
            state_join: Some(state_join),
            asset_worker: Some(asset_worker),
            events,
            config,
            _root_lock: root_lock,
        })
    }

    pub fn endpoints(&self) -> Endpoints {
        self.endpoints.clone()
    }

    pub fn shutdown(mut self) {
        self.shutdown_inner();
    }

    fn shutdown_inner(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(state) = &self.state {
            cancel_and_join_runs(state);
        }
        self.events.shutdown();
        if let Some((stop, join)) = self.watcher.take() {
            let _ = stop.send(());
            let _ = join.join();
        }
        for acceptor in self.acceptors.drain(..) {
            let _ = acceptor.join();
        }
        if let Some(pair) = self.run_events.take() {
            stop_thread(pair);
        }
        if let Some(pair) = self.janitor.take() {
            stop_thread(pair);
        }
        drop(self.state.take());
        if let Some(join) = self.state_join.take() {
            let _ = join.join();
        }
        if let Some(mut worker) = self.asset_worker.take() {
            worker.stop();
        }
        log(&self.config, "stopped");
    }
}

/// Cancel every in-flight run and join its thread, bounded so a wedged
/// executor cannot hang shutdown forever (§5.4/§14: "cancel every run
/// (flag + join with a bound)").
const RUN_JOIN_BOUND: Duration = Duration::from_secs(5);

fn cancel_and_join_runs(state: &StateHandle) {
    let Some(handles) = state.call(|state| state.take_all_run_handles()) else {
        return;
    };
    for handle in &handles {
        handle.cancel.store(true, Ordering::SeqCst);
    }
    let deadline = Instant::now() + RUN_JOIN_BOUND;
    for handle in handles {
        while !handle.join.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        // Past the bound the thread is left to finish on its own; dropping
        // the (unjoined) JoinHandle just detaches it, no resource leak.
    }
}

fn stop_thread(pair: (mpsc::Sender<()>, JoinHandle<()>)) {
    let (stop, join) = pair;
    let _ = stop.send(());
    let _ = join.join();
}

fn spawn_run_events(
    receiver: mpsc::Receiver<RunRegistration>,
    state: StateHandle,
) -> (mpsc::Sender<()>, JoinHandle<()>) {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let join = std::thread::Builder::new()
        .name("flow-server-run-events".to_string())
        .spawn(move || {
            let mut active: Vec<RunRegistration> = Vec::new();
            while matches!(stop_rx.recv_timeout(Duration::from_millis(20)), Err(mpsc::RecvTimeoutError::Timeout))
            {
                while let Ok(registration) = receiver.try_recv() {
                    active.push(registration);
                }
                drain_run_events(&mut active, &state);
            }
            // Final pass: forward whatever already landed in a run's
            // channel (an mpsc sender's buffered sends outlive a dropped
            // sender) before this thread exits.
            while let Ok(registration) = receiver.try_recv() {
                active.push(registration);
            }
            let mut progress = true;
            while progress {
                progress = drain_run_events(&mut active, &state);
            }
        })
        .expect("spawn flow run-events thread");
    (stop_tx, join)
}

/// One poll pass over every registered run's event channel; forwards each
/// event to the state thread via `StateHandle::call`, drops a registration
/// once its run thread's sender disconnects. Returns whether any event was
/// forwarded, so the caller can decide whether to keep draining.
fn drain_run_events(active: &mut Vec<RunRegistration>, state: &StateHandle) -> bool {
    let mut progressed = false;
    active.retain_mut(|registration| loop {
        match registration.receiver.try_recv() {
            Ok(event) => {
                progressed = true;
                let run_id = registration.run_id.clone();
                let instance = registration.instance.clone();
                let flow = registration.flow.clone();
                let _ = state.call(move |state| state.apply_run_event(run_id, instance, flow, event));
            }
            Err(mpsc::TryRecvError::Empty) => return true,
            Err(mpsc::TryRecvError::Disconnected) => return false,
        }
    });
    progressed
}

/// Janitor thread: a fast (100 ms) tick starts any due `trigger: @input`
/// debounced run, a slow (`config.janitor_sweep_secs`, default 30 s) tick
/// sweeps values/runs/instances (§5.2).
fn spawn_janitor(config: &FlowServerConfig, state: StateHandle) -> (mpsc::Sender<()>, JoinHandle<()>) {
    const FAST_TICK: Duration = Duration::from_millis(100);
    let sweep_every = Duration::from_secs(config.janitor_sweep_secs);
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let join = std::thread::Builder::new()
        .name("flow-server-janitor".to_string())
        .spawn(move || {
            let mut since_sweep = Duration::ZERO;
            while matches!(stop_rx.recv_timeout(FAST_TICK), Err(mpsc::RecvTimeoutError::Timeout)) {
                let now_ms = engine::unix_ms();
                let _ = state.call(move |state| state.run_debounced_inputs(now_ms));
                since_sweep += FAST_TICK;
                if since_sweep >= sweep_every {
                    since_sweep = Duration::ZERO;
                    let _ = state.call(|state| state.janitor_sweep());
                }
            }
        })
        .expect("spawn flow janitor thread");
    (stop_tx, join)
}

impl Drop for FlowServer {
    fn drop(&mut self) {
        self.shutdown_inner();
    }
}

fn lock_root(config: &FlowServerConfig) -> Result<File, ServerError> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(config.root.join("server.lock"))
        .map_err(|error| ServerError::io("open server.lock", error))?;
    file.try_lock().map_err(|_| ServerError::Locked)?;
    Ok(file)
}

fn load_or_create_server_id(config: &FlowServerConfig) -> Result<[u8; 16], ServerError> {
    let path = config.root.join("server-id");
    match std::fs::read_to_string(&path) {
        Ok(text) => from_hex_16(text.trim()).ok_or(ServerError::InvalidFile("server-id")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let id = random_16()?;
            atomic_write(&path, format!("{}\n", to_hex(&id)).as_bytes(), 0)?;
            Ok(id)
        }
        Err(error) => Err(ServerError::io("read server-id", error)),
    }
}

fn load_or_create_token(config: &FlowServerConfig) -> Result<String, ServerError> {
    let path = config.root.join("token");
    let token = match std::fs::read_to_string(&path) {
        Ok(text) => text.trim().to_string(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let token = format!("mpft_{}", to_hex(&random_32()?));
            write_secret_file(&path, &token)?;
            token
        }
        Err(error) => return Err(ServerError::io("read token", error)),
    };
    if token.len() != 69
        || !token.starts_with("mpft_")
        || !token[5..].bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ServerError::InvalidFile("token"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| ServerError::io("chmod token", error))?;
    }
    Ok(token)
}

fn bind(text: &str, op: &'static str) -> Result<TcpListener, ServerError> {
    let address: SocketAddr = text.parse().map_err(|_| ServerError::InvalidConfig("invalid bind address"))?;
    let listener = TcpListener::bind(address).map_err(|error| ServerError::io(op, error))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| ServerError::io("make listener nonblocking", error))?;
    Ok(listener)
}

fn write_listen(config: &FlowServerConfig, control: SocketAddr, data: SocketAddr) -> Result<(), ServerError> {
    let ip = if control.ip().is_unspecified() {
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    } else {
        control.ip()
    };
    atomic_write(
        &config.root.join("listen"),
        format!("{ip}:{}:{}\n", control.port(), data.port()).as_bytes(),
        1,
    )
}

fn accept_loop(
    listener: TcpListener,
    route: RouteCtx,
    plane: Plane,
    max_connections: usize,
    stop: Arc<AtomicBool>,
) {
    let active = Arc::new(AtomicUsize::new(0));
    let mut joins: Vec<JoinHandle<()>> = Vec::new();
    let mut idle = 0u32;
    while !stop.load(Ordering::Relaxed) {
        let mut index = 0;
        while index < joins.len() {
            if joins[index].is_finished() {
                let join = joins.swap_remove(index);
                let _ = join.join();
            } else {
                index += 1;
            }
        }
        match listener.accept() {
            Ok((stream, _)) => {
                idle = 0;
                if active.load(Ordering::Relaxed) >= max_connections {
                    refuse_capacity(stream);
                    continue;
                }
                active.fetch_add(1, Ordering::Relaxed);
                let route = route.clone();
                let stop = stop.clone();
                let active_for_connection = active.clone();
                let config = route.config.clone();
                match std::thread::Builder::new()
                    .name("flow-server-connection".to_string())
                    .spawn(move || {
                        serve_connection(stream, &route, plane, &stop);
                        active_for_connection.fetch_sub(1, Ordering::Relaxed);
                    })
                {
                    Ok(join) => joins.push(join),
                    Err(error) => {
                        active.fetch_sub(1, Ordering::Relaxed);
                        log(&config, &format!("connection thread spawn failed: {error}"));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                idle = idle.saturating_add(1);
                if idle < 200 {
                    std::thread::sleep(Duration::from_micros(200));
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    for join in joins {
        let _ = join.join();
    }
}

fn refuse_capacity(stream: TcpStream) {
    let _ = stream.set_nonblocking(false);
    let Ok(mut connection) = Conn::new(stream, 1_000, 1_000) else {
        return;
    };
    let _ = connection.write_resp(
        false,
        &Resp::bytes(503, "text/plain; charset=utf-8", b"connection capacity".to_vec()).closing(),
    );
}

fn serve_connection(stream: TcpStream, route: &RouteCtx, plane: Plane, stop: &AtomicBool) {
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let Ok(mut connection) = Conn::new(stream, READ_TIMEOUT_MS, WRITE_TIMEOUT_MS) else {
        return;
    };
    let mut served = 0;
    loop {
        let mut head = match connection.next_request(HEAD_DEADLINE_MS, KEEPALIVE_IDLE_MS, stop) {
            Ok(head) => head,
            Err(HeadError::Closed | HeadError::Io) => return,
            Err(HeadError::Timeout) => {
                let _ = connection.write_resp(false, &plain_error(408, "head timeout").closing());
                return;
            }
            Err(HeadError::Bad(status, message)) => {
                let _ = connection.write_resp(false, &plain_error(status, message).closing());
                return;
            }
        };
        served += 1;
        let force_close = served >= MAX_REQUESTS_PER_CONN || stop.load(Ordering::Relaxed);
        match dispatch(&mut connection, &mut head, route, plane) {
            Outcome::Hangup => return,
            Outcome::Resp(mut response) => {
                let keep = connection.finish_request(&mut head) && !response.close && !force_close;
                response.close = !keep;
                let is_head = head.method == Method::Head;
                if connection.write_resp(is_head, &response).is_err() || response.close {
                    return;
                }
            }
        }
    }
}

fn plain_error(status: u16, message: &str) -> Resp {
    Resp::bytes(status, "text/plain; charset=utf-8", message.as_bytes().to_vec())
}
