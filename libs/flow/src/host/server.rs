use super::config::{FlowServerConfig, SharedConfig};
use super::events::EventHub;
use super::backend::Backend;
use super::routes::{dispatch, Outcome, Plane, RouteCtx};
use super::util::{atomic_write, from_hex_16, log, random_16, random_32, random_u64, to_hex, write_secret_file};
use super::watcher::{spawn_watcher, watch_baseline};
use crate::engine::results::{FolderStore, ResultStore};
use makepad_bounded_http::{Conn, HeadError, Method, Resp};
use std::fs::File;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

const HEAD_DEADLINE_MS: u64 = 10_000;
const KEEPALIVE_IDLE_MS: u64 = 30_000;
const READ_TIMEOUT_MS: u64 = 10_000;
const WRITE_TIMEOUT_MS: u64 = 10_000;
const MAX_REQUESTS_PER_CONN: u32 = 256;

#[cfg(not(test))]
pub(super) type FlowConn = Conn<TcpStream>;
#[cfg(test)]
pub(super) type FlowConn = Conn<tests::ProbeTransport>;

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
    Results(String),
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
            Self::StateUnavailable => write!(formatter, "flow engine unavailable"),
            Self::Results(error) => write!(formatter, "flow result store: {error}"),
        }
    }
}

impl std::error::Error for ServerError {}

pub struct FlowServer {
    endpoints: Endpoints,
    stop: Arc<AtomicBool>,
    acceptors: Vec<JoinHandle<()>>,
    watcher: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
    /// The one engine behind both planes, and the thread that pumps it.
    backend: Option<Arc<Backend>>,
    backend_join: Option<JoinHandle<()>>,
    events: Arc<EventHub>,
    config: SharedConfig,
    _root_lock: File,
}

impl FlowServer {
    pub fn start(config: FlowServerConfig) -> Result<Self, ServerError> {
        Self::start_inner(
            config,
            #[cfg(test)]
            None,
        )
    }

    fn start_inner(
        config: FlowServerConfig,
        #[cfg(test)] mut control_probe: Option<tests::ShutdownProbe>,
    ) -> Result<Self, ServerError> {
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

        let results: Arc<dyn ResultStore> = match config.results.clone() {
            Some(results) => results,
            None => Arc::new(
                FolderStore::open(config.root.join("results"), "flows").map_err(ServerError::Results)?,
            ),
        };
        let config = Arc::new(config);
        let origin = (to_hex(&server_id), epoch);
        let baseline = watch_baseline(&config.root)?;
        let (backend, backend_join) =
            Backend::start(config.clone(), events.clone(), epoch, origin, results.clone())?;
        let stop = Arc::new(AtomicBool::new(false));
        let route = RouteCtx {
            backend: backend.clone(),
            config: config.clone(),
            server_id,
            token: token.clone(),
            events: events.clone(),
            results,
        };
        let mut watcher = match spawn_watcher(config.clone(), backend.clone(), baseline) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                drop(route);
                backend.stop();
                let _ = backend_join.join();
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
            #[cfg(test)]
            let probe = if plane == Plane::Control { control_probe.take() } else { None };
            let name = match plane {
                Plane::Control => "flow-server-control-accept",
                Plane::Data => "flow-server-data-accept",
            };
            match std::thread::Builder::new().name(name.to_string()).spawn(move || {
                accept_loop(
                    listener, thread_route, plane, cap, thread_stop,
                    #[cfg(test)]
                    probe,
                )
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
                    backend.stop();
                    let _ = backend_join.join();
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
            backend: Some(backend),
            backend_join: Some(backend_join),
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
        // Cancel every run and wait out the engine's stop grace, bounded so a
        // wedged executor cannot hang shutdown (§5.4/§14).
        if let Some(backend) = &self.backend {
            backend.stop();
        }
        self.events.shutdown();
        if let Some((stop, join)) = self.watcher.take() {
            let _ = stop.send(());
            let _ = join.join();
        }
        for acceptor in self.acceptors.drain(..) {
            let _ = acceptor.join();
        }
        drop(self.backend.take());
        if let Some(join) = self.backend_join.take() {
            let _ = join.join();
        }
        log(&self.config, "stopped");
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

struct ConnectionWorker {
    join: JoinHandle<()>,
    wake: Weak<TcpStream>,
}

fn accept_loop(
    listener: TcpListener,
    route: RouteCtx,
    plane: Plane,
    max_connections: usize,
    stop: Arc<AtomicBool>,
    #[cfg(test)] mut probe: Option<tests::ShutdownProbe>,
) {
    let active = Arc::new(AtomicUsize::new(0));
    let mut workers: Vec<ConnectionWorker> = Vec::new();
    let mut idle = 0u32;
    while !stop.load(Ordering::Relaxed) {
        let mut index = 0;
        while index < workers.len() {
            if workers[index].join.is_finished() {
                let join = workers.swap_remove(index).join;
                let _ = join.join();
            } else {
                index += 1;
            }
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
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
                let wake_stream = match stream.try_clone() {
                    Ok(socket) => Some(Arc::new(socket)),
                    Err(error) => {
                        log(&config, &format!("connection wake socket clone failed: {error}"));
                        None
                    }
                };
                // Only the worker keeps the duplicate alive, so an unreaped
                // join handle cannot delay ordinary EOF for the client.
                let wake = wake_stream.as_ref().map(Arc::downgrade).unwrap_or_default();
                #[cfg(test)]
                let connection_probe = probe.as_mut().and_then(tests::ShutdownProbe::next_connection);
                match std::thread::Builder::new()
                    .name("flow-server-connection".to_string())
                    .spawn(move || {
                        serve_connection(
                            stream, &route, plane, &stop,
                            #[cfg(test)]
                            connection_probe,
                        );
                        drop(wake_stream);
                        active_for_connection.fetch_sub(1, Ordering::Relaxed);
                    })
                {
                    Ok(join) => workers.push(ConnectionWorker { join, wake }),
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
    for worker in &workers {
        if let Some(stream) = worker.wake.upgrade() {
            match makepad_bounded_http::wake_idle_read(&stream) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotConnected => {}
                Err(error) => log(&route.config, &format!("connection read wake failed: {error}")),
            }
        }
    }
    #[cfg(test)]
    if let Some(probe) = probe {
        probe.wake_pass_finished();
    }
    for ConnectionWorker { join, .. } in workers {
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

fn serve_connection(
    stream: TcpStream,
    route: &RouteCtx,
    plane: Plane,
    stop: &AtomicBool,
    #[cfg(test)] probe: Option<tests::ConnectionProbe>,
) {
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    #[cfg(not(test))]
    let connection = FlowConn::new(stream, READ_TIMEOUT_MS, WRITE_TIMEOUT_MS);
    #[cfg(test)]
    let connection = tests::ProbeTransport::connection(stream, probe);
    let Ok(mut connection) = connection else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_bounded_http::Transport;
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::sync::mpsc::Sender;

    #[derive(Debug)]
    enum Event {
        IdlePending(Sender<()>),
        IdleResult(Result<usize, io::ErrorKind>),
        TailPending(Sender<()>),
        WakePassFinished,
    }

    enum Watch {
        Idle { response_written: bool },
        Tail,
        TailByte,
    }

    pub(super) struct ConnectionProbe {
        watch: Watch,
        events: Sender<Event>,
    }

    pub(super) struct ShutdownProbe {
        connections: std::vec::IntoIter<Option<ConnectionProbe>>,
        events: Sender<Event>,
    }

    impl ShutdownProbe {
        pub(super) fn next_connection(&mut self) -> Option<ConnectionProbe> {
            self.connections.next().flatten()
        }

        pub(super) fn wake_pass_finished(self) {
            self.events.send(Event::WakePassFinished).unwrap();
        }
    }

    pub(crate) struct ProbeTransport {
        stream: TcpStream,
        probe: Option<ConnectionProbe>,
    }

    impl ProbeTransport {
        pub(super) fn connection(stream: TcpStream, probe: Option<ConnectionProbe>) -> io::Result<FlowConn> {
            stream.set_nodelay(true)?;
            stream.set_write_timeout(Some(Duration::from_millis(WRITE_TIMEOUT_MS)))?;
            Ok(Conn::over(Self { stream, probe }, READ_TIMEOUT_MS))
        }
    }

    impl Read for ProbeTransport {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if let Some(ConnectionProbe { watch: Watch::Idle { response_written: true }, events }) = &self.probe {
                self.stream.set_nonblocking(true)?;
                assert_eq!(self.stream.read(buf).unwrap_err().kind(), io::ErrorKind::WouldBlock);
                let (resume, gate) = mpsc::channel();
                events.send(Event::IdlePending(resume)).unwrap();
                gate.recv().unwrap();
                let result = self.stream.read(buf);
                events.send(Event::IdleResult(result.as_ref().copied().map_err(io::Error::kind))).unwrap();
                self.stream.set_nonblocking(false)?;
                return result;
            }
            self.stream.read(buf)
        }
    }

    impl Write for ProbeTransport {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Some(probe) = &mut self.probe {
                match &mut probe.watch {
                    Watch::Idle { response_written } => {
                        let written = self.stream.write(buf)?;
                        *response_written = written == buf.len();
                        return Ok(written);
                    }
                    Watch::Tail => {
                        assert!(
                            buf.starts_with(b"HTTP/1.1 ") && buf.ends_with(b"\r\n\r\n[]"),
                            "tail hold needs head and body in one write"
                        );
                        let prefix = &buf[..buf.len() - 1];
                        self.stream.write_all(prefix)?;
                        probe.watch = Watch::TailByte;
                        return Ok(prefix.len());
                    }
                    Watch::TailByte => {
                        let (resume, gate) = mpsc::channel();
                        probe.events.send(Event::TailPending(resume)).unwrap();
                        gate.recv().unwrap();
                    }
                }
            }
            self.stream.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.stream.flush()
        }
    }

    impl Transport for ProbeTransport {
        fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            self.stream.set_read_timeout(timeout)
        }

        fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
            self.stream.set_nonblocking(nonblocking)
        }

        fn peer_addr(&self) -> io::Result<SocketAddr> {
            self.stream.peer_addr()
        }
    }

    fn request_flows(endpoints: &Endpoints) -> BufReader<TcpStream> {
        let mut client = TcpStream::connect(endpoints.control).unwrap();
        write!(client, "GET /v1/flows HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\n\r\n", endpoints.token).unwrap();
        let mut reader = BufReader::new(client);
        let headers: Vec<_> = (&mut reader).lines().map(Result::unwrap)
            .take_while(|line| !line.is_empty()).collect();
        assert_eq!(headers[0].split_whitespace().nth(1), Some("200"));
        assert!(headers.iter().any(|line| line.eq_ignore_ascii_case("Content-Length: 2")), "{headers:?}");
        reader
    }

    #[test]
    fn shutdown_wakes_idle_read_and_finishes_partial_response() {
        let root = std::env::temp_dir().join(format!("flow-shutdown-{}", random_u64().unwrap()));
        // On a setup assertion, drop the gate channels before the server joins its workers.
        let server;
        let (idle_tx, idle_events) = mpsc::channel();
        let (tail_tx, tail_events) = mpsc::channel();
        let (wake_tx, wake_events) = mpsc::channel();
        // Other platforms assert only the partial response. In particular, Windows
        // keeps bounded_http's idle read slice until the stop-event wake is implemented.
        let probe = ShutdownProbe {
            connections: vec![
                cfg!(unix).then_some(ConnectionProbe { watch: Watch::Idle { response_written: false }, events: idle_tx }),
                Some(ConnectionProbe { watch: Watch::Tail, events: tail_tx }),
            ].into_iter(),
            events: wake_tx,
        };
        server = FlowServer::start_inner(FlowServerConfig::new(root.clone()), Some(probe)).unwrap();
        let endpoints = server.endpoints();
        let mut idle = request_flows(&endpoints);
        let mut body = [0; 2];
        idle.read_exact(&mut body).unwrap();
        assert_eq!(&body, b"[]");
        let idle_gate = cfg!(unix).then(|| match idle_events.recv().unwrap() {
            Event::IdlePending(resume) => resume,
            event => panic!("expected a pending idle read, got {event:?}"),
        });

        let mut writer = request_flows(&endpoints);
        let mut body = Vec::from([0]);
        writer.read_exact(&mut body).unwrap();
        assert_eq!(&body, b"[");
        let Event::TailPending(tail_gate) = tail_events.recv().unwrap() else {
            panic!("expected a pending response tail");
        };
        let shutdown = std::thread::spawn(move || server.shutdown());
        assert!(matches!(wake_events.recv().unwrap(), Event::WakePassFinished));
        if let Some(resume) = idle_gate {
            resume.send(()).unwrap();
        }
        tail_gate.send(()).unwrap();
        shutdown.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();

        writer.read_to_end(&mut body).unwrap();
        assert_eq!(&body, b"[]", "shutdown interrupted the response");
        if cfg!(unix) {
            let event = idle_events.try_recv().expect("idle read exited without a result");
            assert!(matches!(event, Event::IdleResult(Ok(0) | Err(io::ErrorKind::NotConnected))),
                "expected shutdown to end the idle read, got {event:?}");
        }
    }
}
