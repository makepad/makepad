// this webserver is serving our site. Why? WHYYY. Because it was fun to write. And MUCH faster and MUCH simpler than anything else imaginable.

use crate::utils::*;
pub use crate::web_socket_parser::{
    WebSocketMessage, WebSocketMessageFormat, WebSocketMessageHeader, WebSocketParser,
    SERVER_WEB_SOCKET_PING_MESSAGE, SERVER_WEB_SOCKET_PONG_MESSAGE,
};
use std::io::prelude::*;
use std::fs::File;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, mpsc::RecvTimeoutError, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Sockets whose peer vanished (a client retry storm timing out and
/// abandoning connects, a NAT dropping the flow) used to pin their
/// per-connection thread FOREVER in a blocking read — thousands of leaked
/// threads eventually slow `thread::spawn` on the ACCEPT loop itself, the
/// listen backlog fills, and new SYNs get dropped: the service looks dead
/// from outside while established flows stay fine. Bound every socket wait.
const API_CONNECTION_TIMEOUT: Duration = Duration::from_secs(60);
const STATIC_CONNECTION_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Concurrent connection threads (process-wide). Over the cap, new
/// connections are shed with an inline 503 — cheap and honest — instead of
/// growing the thread pile.
const MAX_CONNS: usize = 256;
const MAX_CONNS_PER_IP: usize = 16;
const MAX_IN_FLIGHT_BODY_BYTES: usize = 32 * 1024 * 1024;
static ACTIVE_CONNS: AtomicUsize = AtomicUsize::new(0);

struct ConnGuard;
impl ConnGuard {
    fn try_acquire() -> Option<ConnGuard> {
        let prev = ACTIVE_CONNS.fetch_add(1, Ordering::SeqCst);
        if prev >= MAX_CONNS {
            ACTIVE_CONNS.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(ConnGuard)
    }
}
impl Drop for ConnGuard {
    fn drop(&mut self) {
        ACTIVE_CONNS.fetch_sub(1, Ordering::SeqCst);
    }
}

struct IpConnGuard {
    counts: Arc<Mutex<HashMap<IpAddr, usize>>>,
    ip: IpAddr,
}

impl IpConnGuard {
    fn try_acquire(counts: Arc<Mutex<HashMap<IpAddr, usize>>>, ip: IpAddr) -> Option<Self> {
        let ip = normalize_ip(ip);
        let mut locked = counts.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = locked.entry(ip).or_default();
        if *count >= MAX_CONNS_PER_IP {
            return None;
        }
        *count += 1;
        drop(locked);
        Some(Self { counts, ip })
    }
}

impl Drop for IpConnGuard {
    fn drop(&mut self) {
        let mut counts = self.counts.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = counts.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.ip);
            }
        }
    }
}

fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(ip) => IpAddr::V6(Ipv6Addr::from(u128::from(ip) & (!0u128 << 64))),
    }
}

struct BodyBudgetGuard {
    used: Arc<AtomicUsize>,
    bytes: usize,
}

impl BodyBudgetGuard {
    fn try_acquire(used: Arc<AtomicUsize>, bytes: usize, limit: usize) -> Option<Self> {
        let mut current = used.load(Ordering::Acquire);
        loop {
            let next = current.checked_add(bytes)?;
            if next > limit {
                return None;
            }
            match used.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(Self { used, bytes }),
                Err(changed) => current = changed,
            }
        }
    }
}

impl Drop for BodyBudgetGuard {
    fn drop(&mut self) {
        self.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[cfg(feature = "script")]
use makepad_script::*;

#[derive(Clone)]
pub struct HttpServer {
    pub listen_address: SocketAddr,
    pub request: mpsc::Sender<HttpServerRequest>,
    pub post_max_size: u64,
    /// Exact-path POST limits checked before the request body is allocated.
    /// The smallest matching limit wins over `post_max_size`.
    pub post_max_size_overrides: Vec<(String, u64)>,
    /// Dispatch POST headers first, allowing the application to reserve a
    /// bounded worker/queue slot before any declared body buffer is allocated.
    pub pre_admit_posts: bool,
    /// Resolves the client identity used by the per-IP connection cap. A
    /// reverse-proxy deployment must validate its trusted peers here.
    pub client_ip_resolver: Option<fn(&HttpServerHeaders) -> IpAddr>,
}

#[cfg_attr(feature = "script", derive(Script, ScriptHook))]
#[derive(Clone)]
pub struct HttpServerResponse {
    #[cfg_attr(feature = "script", live)]
    pub header: String,
    #[cfg_attr(feature = "script", live)]
    pub body: Vec<u8>,
}

struct HttpServerFileResponse {
    file: File,
    offset: u64,
    len: u64,
}

const FILE_RESPONSE_MARKER: &[u8] = b"\0makepad-http-file\0";
static NEXT_FILE_RESPONSE: AtomicU64 = AtomicU64::new(1);
static FILE_RESPONSES: OnceLock<Mutex<HashMap<u64, HttpServerFileResponse>>> = OnceLock::new();

impl HttpServerResponse {
    /// Builds a response whose already-open file region is streamed by the
    /// connection thread. Ordinary struct literals and response channels are
    /// unchanged, so this is additive for existing HTTP server users.
    pub fn from_file(header: String, file: File, offset: u64, len: u64) -> Self {
        let id = NEXT_FILE_RESPONSE.fetch_add(1, Ordering::Relaxed);
        let mut responses = FILE_RESPONSES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        responses.insert(id, HttpServerFileResponse { file, offset, len });
        drop(responses);
        let mut body = FILE_RESPONSE_MARKER.to_vec();
        body.extend_from_slice(&id.to_le_bytes());
        Self { header, body }
    }

    /// Closes a registered file payload when its response could not be sent.
    pub fn discard_file(&self) {
        let _ = take_file_response(&self.body);
    }
}

pub type HttpServerResponseSender = mpsc::Sender<HttpServerResponse>;

enum PostAdmission {
    Read,
    Respond(HttpServerResponse),
}

/// A body whose allocation and socket read are waiting on application-level
/// admission. `receive` should only be called by an already-admitted worker.
pub struct HttpServerPendingBody {
    pub content_length: usize,
    admission: Option<mpsc::SyncSender<PostAdmission>>,
    body: mpsc::Receiver<Result<Vec<u8>, ()>>,
}

impl HttpServerPendingBody {
    pub fn receive(mut self) -> Result<Vec<u8>, ()> {
        self.admission.take().ok_or(())?.send(PostAdmission::Read).map_err(|_| ())?;
        self.body.recv().map_err(|_| ())?
    }

    pub fn reject(mut self, response: HttpServerResponse) {
        if let Some(admission) = self.admission.take() {
            let _ = admission.send(PostAdmission::Respond(response));
        }
    }
}

fn take_file_response(body: &[u8]) -> Option<HttpServerFileResponse> {
    if body.len() != FILE_RESPONSE_MARKER.len() + 8 || !body.starts_with(FILE_RESPONSE_MARKER) {
        return None;
    }
    let id = u64::from_le_bytes(body[FILE_RESPONSE_MARKER.len()..].try_into().ok()?);
    FILE_RESPONSES
        .get()?
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&id)
}

pub enum HttpServerRequest {
    ConnectWebSocket {
        web_socket_id: u64,
        headers: HttpServerHeaders,
        response_sender: mpsc::Sender<Vec<u8>>,
    },
    DisconnectWebSocket {
        web_socket_id: u64,
    },
    BinaryMessage {
        web_socket_id: u64,
        response_sender: mpsc::Sender<Vec<u8>>,
        data: Vec<u8>,
    },
    TextMessage {
        web_socket_id: u64,
        response_sender: mpsc::Sender<Vec<u8>>,
        string: String,
    },
    Get {
        headers: HttpServerHeaders,
        response_sender: HttpServerResponseSender,
    },
    Post {
        headers: HttpServerHeaders,
        body: Vec<u8>,
        response: HttpServerResponseSender,
    },
    PostPending {
        headers: HttpServerHeaders,
        body: HttpServerPendingBody,
        response: HttpServerResponseSender,
    },
}

pub fn start_http_server(http_server: HttpServer) -> Option<std::thread::JoinHandle<()>> {
    let listener = if let Ok(listener) = TcpListener::bind(http_server.listen_address) {
        listener
    } else {
        println!("Cannot bind http server port");
        return None;
    };

    let ip_connections = Arc::new(Mutex::new(HashMap::new()));
    let in_flight_body_bytes = Arc::new(AtomicUsize::new(0));
    let body_budget_limit = MAX_IN_FLIGHT_BODY_BYTES;
    let listen_thread = {
        std::thread::spawn(move || {
            let mut connection_counter = 0u64;
            for tcp_stream in listener.incoming() {
                let mut tcp_stream = if let Ok(tcp_stream) = tcp_stream {
                    tcp_stream
                } else {
                    println!("Incoming stream failure");
                    continue;
                };
                let http_server = http_server.clone();
                let ip_connections = ip_connections.clone();
                let in_flight_body_bytes = in_flight_body_bytes.clone();
                connection_counter += 1;
                // Shed over the cap INLINE (no thread): the pile of leaked
                // connection threads is what used to stall this accept loop.
                let Some(guard) = ConnGuard::try_acquire() else {
                    http_error_out(tcp_stream, 503);
                    continue;
                };
                let peer_ip = match tcp_stream.peer_addr() {
                    Ok(address) => address.ip(),
                    Err(_) => {
                        http_error_out(tcp_stream, 400);
                        continue;
                    }
                };
                let Some(peer_guard) = IpConnGuard::try_acquire(ip_connections.clone(), peer_ip) else {
                    http_error_out(tcp_stream, 503);
                    continue;
                };
                let _read_thread = std::thread::spawn(move || {
                    let _guard = guard;
                    let started = Instant::now();
                    let head_deadline = started + API_CONNECTION_TIMEOUT;
                    let head = match HttpServerHeaders::from_tcp_stream_until(
                        &mut tcp_stream,
                        head_deadline,
                    ) {
                        Ok(head) => head,
                        Err(error) => return http_error_out(tcp_stream, error.status()),
                    };
                    // Whatever arrived in the same segment as the headers is
                    // already off the socket and has to be handed onward.
                    let (headers, body_prefix) = head;
                    let client_ip = http_server
                        .client_ip_resolver
                        .map(|resolve| resolve(&headers))
                        .unwrap_or(peer_ip);
                    let _ip_guard = if normalize_ip(client_ip) == peer_guard.ip {
                        peer_guard
                    } else {
                        let Some(client_guard) = IpConnGuard::try_acquire(ip_connections, client_ip) else {
                            return http_error_out(tcp_stream, 503);
                        };
                        drop(peer_guard);
                        client_guard
                    };

                    if headers.sec_websocket_key.is_some() {
                        return handle_web_socket(
                            http_server,
                            tcp_stream,
                            headers,
                            body_prefix,
                            connection_counter,
                        );
                    }
                    let deadline = started
                        + if headers.path.starts_with("/api/") {
                            API_CONNECTION_TIMEOUT
                        } else {
                            STATIC_CONNECTION_TIMEOUT
                        };
                    if headers.verb == "POST" {
                        return handle_post(
                            http_server,
                            tcp_stream,
                            headers,
                            body_prefix,
                            deadline,
                            in_flight_body_bytes,
                            body_budget_limit,
                        );
                    }
                    if matches!(headers.verb.as_str(), "GET" | "HEAD" | "OPTIONS" | "PUT" | "DELETE") {
                        return handle_get(http_server, tcp_stream, headers, deadline);
                    }
                    http_error_out(tcp_stream, 400)
                });
            }
        })
    };
    Some(listen_thread)
}

fn handle_post(
    http_server: HttpServer,
    mut tcp_stream: TcpStream,
    headers: HttpServerHeaders,
    body_prefix: Vec<u8>,
    deadline: Instant,
    in_flight_body_bytes: Arc<AtomicUsize>,
    body_budget_limit: usize,
) {
    // we have to have a content-length or bust
    let Some(content_length) = headers.content_length else {
        return http_error_out(tcp_stream, 411);
    };
    let path_limit = http_server
        .post_max_size_overrides
        .iter()
        .filter(|(path, _)| path == &headers.path)
        .map(|(_, limit)| *limit)
        .min()
        .unwrap_or(http_server.post_max_size);
    if content_length > path_limit {
        return http_error_out(tcp_stream, 413);
    }
    let Ok(bytes_total) = usize::try_from(content_length) else {
        return http_error_out(tcp_stream, 413);
    };
    if body_prefix.len() > bytes_total {
        return http_error_out(tcp_stream, 400);
    }
    if headers
        .lines
        .iter()
        .skip(1)
        .filter_map(|line| split_header_line(line, "Content-Encoding"))
        .any(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return http_error_out(tcp_stream, 415);
    }

    let (tx_socket, rx_socket) = mpsc::channel::<HttpServerResponse>();
    let mut pending_body_sender = None;
    if http_server.pre_admit_posts {
        let (admission_sender, admission_receiver) = mpsc::sync_channel(1);
        let (body_sender, body_receiver) = mpsc::sync_channel(1);
        if http_server
            .request
            .send(HttpServerRequest::PostPending {
                headers: headers.clone(),
                body: HttpServerPendingBody {
                    content_length: bytes_total,
                    admission: Some(admission_sender),
                    body: body_receiver,
                },
                response: tx_socket.clone(),
            })
            .is_err()
        {
            return http_error_out(tcp_stream, 500);
        }
        let Some(remaining) = remaining(deadline) else {
            return http_error_out(tcp_stream, 408);
        };
        match admission_receiver.recv_timeout(remaining) {
            Ok(PostAdmission::Read) => pending_body_sender = Some(body_sender),
            Ok(PostAdmission::Respond(response)) => {
                write_response(&mut tcp_stream, response, false, deadline);
                let _ = tcp_stream.shutdown(Shutdown::Both);
                return;
            }
            Err(_) => return http_error_out(tcp_stream, 503),
        }
    }

    let Some(_body_budget) = BodyBudgetGuard::try_acquire(
        in_flight_body_bytes,
        bytes_total,
        body_budget_limit,
    ) else {
        if let Some(sender) = pending_body_sender {
            let _ = sender.send(Err(()));
        }
        return http_error_out(tcp_stream, 503);
    };
    let mut body = Vec::new();
    body.resize(bytes_total, 0u8);

    // Body bytes that came in with the headers are already read; reading them
    // from the socket again would block until the peer or the timeout gives up.
    let prefix_len = body_prefix.len().min(bytes_total);
    body[..prefix_len].copy_from_slice(&body_prefix[..prefix_len]);

    let mut bytes_left = bytes_total - prefix_len;
    while bytes_left > 0 {
        let Some(remaining) = remaining(deadline) else {
            if let Some(sender) = pending_body_sender {
                let _ = sender.send(Err(()));
            }
            return http_error_out(tcp_stream, 408);
        };
        if tcp_stream.set_read_timeout(Some(remaining)).is_err() {
            return http_error_out(tcp_stream, 400);
        }
        let buf = &mut body[(bytes_total - bytes_left)..bytes_total];
        let bytes_read = match tcp_stream.read(buf) {
            Ok(bytes_read) => bytes_read,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                return http_error_out(tcp_stream, 408)
            }
            Err(_) => return http_error_out(tcp_stream, 400),
        };
        if bytes_read == 0 {
            return http_error_out(tcp_stream, 400);
        }
        bytes_left -= bytes_read;
    }

    if let Some(sender) = pending_body_sender {
        if sender.send(Ok(body)).is_err() {
            return http_error_out(tcp_stream, 500);
        }
    } else if http_server
        .request
        .send(HttpServerRequest::Post { headers, body, response: tx_socket })
        .is_err()
    {
        return http_error_out(tcp_stream, 500);
    }

    let Some(wait) = remaining(deadline) else {
        return http_error_out(tcp_stream, 504);
    };
    match rx_socket.recv_timeout(wait) {
        Ok(response) => write_response(&mut tcp_stream, response, false, deadline),
        Err(_) => return http_error_out(tcp_stream, 504),
    }
    let _ = tcp_stream.shutdown(Shutdown::Both);
}

fn handle_web_socket(
    http_server: HttpServer,
    mut tcp_stream: TcpStream,
    headers: HttpServerHeaders,
    body_prefix: Vec<u8>,
    web_socket_id: u64,
) {
    // Low-latency control traffic (e.g. studio Tick messages) benefits from
    // disabling Nagle on loopback websocket links.
    let _ = tcp_stream.set_nodelay(true);
    // A websocket idles legitimately (the write thread pings it); the
    // accept-time socket timeout would kill it, so clear it here.
    let _ = tcp_stream.set_read_timeout(None);
    let _ = tcp_stream.set_write_timeout(None);

    let upgrade_response =
        WebSocketParser::create_upgrade_response(headers.sec_websocket_key.as_ref().unwrap());

    write_bytes_to_tcp_stream_no_error(&mut tcp_stream, upgrade_response.as_bytes());

    let mut write_tcp_stream = tcp_stream.try_clone().unwrap();
    let _ = write_tcp_stream.set_nodelay(true);
    let (tx_socket, rx_socket) = mpsc::channel::<Vec<u8>>();

    let _write_thread = std::thread::spawn(move || {
        // xx
        loop {
            match rx_socket.recv_timeout(Duration::from_millis(2000)) {
                Ok(data) => {
                    if data.is_empty() {
                        break;
                    }
                    let header = WebSocketMessageHeader::from_len(
                        data.len(),
                        WebSocketMessageFormat::Binary,
                        false,
                    );
                    write_bytes_to_tcp_stream_no_error(&mut write_tcp_stream, header.as_slice());
                    write_bytes_to_tcp_stream_no_error(&mut write_tcp_stream, &data);
                }
                Err(RecvTimeoutError::Timeout) => {
                    write_bytes_to_tcp_stream_no_error(
                        &mut write_tcp_stream,
                        &SERVER_WEB_SOCKET_PING_MESSAGE,
                    );
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = write_tcp_stream.shutdown(Shutdown::Both);
    });

    if http_server
        .request
        .send(HttpServerRequest::ConnectWebSocket {
            headers,
            web_socket_id,
            response_sender: tx_socket.clone(),
        })
        .is_err()
    {
        let _ = tcp_stream.shutdown(Shutdown::Both);
        return;
    };

    let mut web_socket = WebSocketParser::new();
    // A client may pipeline its first frames into the same segment as the
    // upgrade request; those bytes came off the socket with the headers and
    // must be parsed before we block waiting for more.
    let mut pending = body_prefix;
    loop {
        let mut data = [0u8; 65535];
        let pipelined;
        let read = if pending.is_empty() {
            tcp_stream.read(&mut data)
        } else {
            pipelined = std::mem::take(&mut pending);
            data[..pipelined.len()].copy_from_slice(&pipelined);
            Ok(pipelined.len())
        };
        match read {
            Ok(n) => {
                if n == 0 {
                    let _ = tcp_stream.shutdown(Shutdown::Both);
                    let _ = tx_socket.send(Vec::new());
                    break;
                }
                web_socket.parse(&data[0..n], |result| match result {
                    Ok(WebSocketMessage::Ping(_)) => {
                        let _ = tx_socket.send(SERVER_WEB_SOCKET_PONG_MESSAGE.to_vec());
                    }
                    Ok(WebSocketMessage::Pong(_)) => {}
                    Ok(WebSocketMessage::Text(text)) => {
                        if http_server
                            .request
                            .send(HttpServerRequest::TextMessage {
                                web_socket_id,
                                response_sender: tx_socket.clone(),
                                string: text.into(),
                            })
                            .is_err()
                        {
                            eprintln!("Websocket message deserialize error");
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                            let _ = tx_socket.send(Vec::new());
                        };
                    }
                    Ok(WebSocketMessage::Binary(data)) => {
                        if http_server
                            .request
                            .send(HttpServerRequest::BinaryMessage {
                                web_socket_id,
                                response_sender: tx_socket.clone(),
                                data: data.to_vec(),
                            })
                            .is_err()
                        {
                            eprintln!("Websocket message deserialize error");
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                            let _ = tx_socket.send(Vec::new());
                        };
                    }
                    Ok(WebSocketMessage::Close) => {
                        let _ = tcp_stream.shutdown(Shutdown::Both);
                    }
                    Err(e) => {
                        eprintln!("Websocket error {:?}", e);
                        let _ = tcp_stream.shutdown(Shutdown::Both);
                        let _ = tx_socket.send(Vec::new());
                    }
                });
            }
            Err(_) => {
                println!("Websocket closed");
                let _ = tcp_stream.shutdown(Shutdown::Both);
                let _ = tx_socket.send(Vec::new());
                break;
            }
        }
    }

    let _ = http_server
        .request
        .send(HttpServerRequest::DisconnectWebSocket { web_socket_id });
}

fn handle_get(
    http_server: HttpServer,
    mut tcp_stream: TcpStream,
    headers: HttpServerHeaders,
    deadline: Instant,
) {
    // send our channel the post
    let suppress_body = headers.verb == "HEAD";
    let (tx_socket, rx_socket) = mpsc::channel::<HttpServerResponse>();
    if http_server
        .request
        .send(HttpServerRequest::Get {
            headers,
            response_sender: tx_socket,
        })
        .is_err()
    {
        return http_error_out(tcp_stream, 500);
    };

    let Some(wait) = remaining(deadline) else {
        return http_error_out(tcp_stream, 504);
    };
    match rx_socket.recv_timeout(wait) {
        Ok(response) => write_response(&mut tcp_stream, response, suppress_body, deadline),
        Err(_) => return http_error_out(tcp_stream, 504),
    }
    let _ = tcp_stream.shutdown(Shutdown::Both);
}

fn write_response(
    tcp_stream: &mut TcpStream,
    response: HttpServerResponse,
    suppress_body: bool,
    deadline: Instant,
) {
    let file_response = take_file_response(&response.body);
    if !write_bytes_until(tcp_stream, response.header.as_bytes(), deadline) {
        return;
    }
    if suppress_body {
        return;
    }
    if let Some(HttpServerFileResponse { mut file, offset, len }) = file_response {
        if file.seek(std::io::SeekFrom::Start(offset)).is_ok() {
            let mut left = len;
            let mut buffer = [0u8; 64 * 1024];
            while left > 0 {
                let amount = left.min(buffer.len() as u64) as usize;
                let Ok(read) = file.read(&mut buffer[..amount]) else { break };
                if read == 0 || !write_bytes_until(tcp_stream, &buffer[..read], deadline) {
                    break;
                }
                left -= read as u64;
            }
        }
    } else {
        let _ = write_bytes_until(tcp_stream, &response.body, deadline);
    }
}

fn remaining(deadline: Instant) -> Option<Duration> {
    deadline.checked_duration_since(Instant::now()).filter(|duration| !duration.is_zero())
}

fn write_bytes_until(tcp_stream: &mut TcpStream, bytes: &[u8], deadline: Instant) -> bool {
    let mut written = 0;
    while written < bytes.len() {
        let Some(wait) = remaining(deadline) else { return false };
        if tcp_stream.set_write_timeout(Some(wait)).is_err() {
            return false;
        }
        match tcp_stream.write(&bytes[written..]) {
            Ok(0) => return false,
            Ok(count) => written += count,
            Err(_) => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn aggregate_body_budget_is_bounded_and_released() {
        let used = Arc::new(AtomicUsize::new(0));
        let first = BodyBudgetGuard::try_acquire(used.clone(), 7, 10).unwrap();
        assert!(BodyBudgetGuard::try_acquire(used.clone(), 4, 10).is_none());
        let second = BodyBudgetGuard::try_acquire(used.clone(), 3, 10).unwrap();
        assert_eq!(used.load(Ordering::Acquire), 10);
        drop((first, second));
        assert_eq!(used.load(Ordering::Acquire), 0);
    }

    #[test]
    fn per_ip_cap_groups_ipv6_by_64_and_releases_entries() {
        let counts = Arc::new(Mutex::new(HashMap::new()));
        let first: IpAddr = "2001:db8::1".parse().unwrap();
        let same_64: IpAddr = "2001:db8::ffff".parse().unwrap();
        let mut guards = Vec::new();
        for _ in 0..MAX_CONNS_PER_IP {
            guards.push(IpConnGuard::try_acquire(counts.clone(), first).unwrap());
        }
        assert!(IpConnGuard::try_acquire(counts.clone(), same_64).is_none());
        drop(guards);
        assert!(counts.lock().unwrap().is_empty());
    }

    #[test]
    fn abandoned_file_response_registration_is_removed() {
        let path = std::env::temp_dir().join(format!(
            "makepad-http-file-response-{}-{}",
            std::process::id(),
            NEXT_FILE_RESPONSE.load(Ordering::Relaxed)
        ));
        fs::write(&path, b"fixture").unwrap();
        let before = FILE_RESPONSES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap()
            .len();
        let response = HttpServerResponse::from_file(String::new(), File::open(&path).unwrap(), 0, 7);
        assert_eq!(FILE_RESPONSES.get().unwrap().lock().unwrap().len(), before + 1);
        response.discard_file();
        assert_eq!(FILE_RESPONSES.get().unwrap().lock().unwrap().len(), before);
        fs::remove_file(path).unwrap();
    }
}
