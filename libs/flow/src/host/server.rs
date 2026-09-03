use super::config::{FlowServerConfig, SharedConfig};
use super::events::EventHub;
use super::routes::{dispatch, Outcome, Plane, RouteCtx};
use super::state::{spawn_state, StateHandle};
use super::util::{atomic_write, from_hex_16, log, random_16, random_32, random_u64, to_hex, write_secret_file};
use super::watcher::spawn_watcher;
use makepad_bounded_http::{Conn, HeadError, Method, Resp};
use std::fs::File;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

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
        }
    }
}

impl std::error::Error for ServerError {}

pub struct FlowServer {
    endpoints: Endpoints,
    stop: Arc<AtomicBool>,
    acceptors: Vec<JoinHandle<()>>,
    watcher: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
    state: Option<StateHandle>,
    state_join: Option<JoinHandle<()>>,
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

        let config = Arc::new(config);
        let (state, state_join) = spawn_state(config.clone(), events.clone(), epoch)?;
        let stop = Arc::new(AtomicBool::new(false));
        let route = RouteCtx {
            state: state.clone(),
            config: config.clone(),
            server_id,
            token: token.clone(),
            events: events.clone(),
        };
        let mut watcher = match spawn_watcher(config.clone(), state.clone()) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                drop(route);
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
            state: Some(state),
            state_join: Some(state_join),
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
        self.events.shutdown();
        if let Some((stop, join)) = self.watcher.take() {
            let _ = stop.send(());
            let _ = join.join();
        }
        for acceptor in self.acceptors.drain(..) {
            let _ = acceptor.join();
        }
        drop(self.state.take());
        if let Some(join) = self.state_join.take() {
            let _ = join.join();
            log(&self.config, "stopped");
        }
    }
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
