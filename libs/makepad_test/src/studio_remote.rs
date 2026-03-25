use crate::error::{TestError, TestResult};
use makepad_micro_serde::{DeBin, DeJson, SerBin};
use makepad_network::{
    ServerWebSocketError, ServerWebSocketMessage, ServerWebSocketMessageFormat,
    ServerWebSocketMessageHeader, WebSocketParser, SERVER_WEB_SOCKET_PONG_MESSAGE,
};
use makepad_studio_protocol::hub_protocol::{
    ClientId, ClientToHub, ClientToHubEnvelope, HubToClient, QueryId,
};
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const STUDIO_UI_PATH: &str = "/ui";
const HELLO_TIMEOUT: Duration = Duration::from_secs(2);

pub struct StudioRemoteClient {
    client_id: ClientId,
    next_counter: u64,
    stream: Arc<Mutex<TcpStream>>,
    recv: Receiver<HubToClient>,
    _reader_thread: JoinHandle<()>,
}

impl StudioRemoteClient {
    pub fn connect(studio: &str) -> TestResult<Self> {
        let (host, port) = resolve_host_port(studio)?;
        let host_header = format!("{host}:{port}");
        let mut addrs = host_header.to_socket_addrs().map_err(|err| {
            TestError::new(format!(
                "failed to resolve studio address {host_header}: {err}"
            ))
        })?;
        let socket_addr = addrs.next().ok_or_else(|| {
            TestError::new(format!("failed to resolve studio address {host_header}"))
        })?;

        let (stream, leftover) = connect_websocket(socket_addr, &host_header, STUDIO_UI_PATH)?;
        let read_stream = stream.try_clone().map_err(|err| {
            TestError::new(format!("failed to clone studio websocket stream: {err}"))
        })?;
        let stream = Arc::new(Mutex::new(stream));
        let (tx, recv) = mpsc::channel();
        let reader_stream = stream.clone();
        let reader_client_id = Arc::new(Mutex::new(None));
        let client_id_slot = reader_client_id.clone();

        let reader_thread = thread::spawn(move || {
            let mut web_socket = WebSocketParser::new();
            if !leftover.is_empty() {
                let _ = parse_incoming_frames(
                    &reader_stream,
                    &mut web_socket,
                    &tx,
                    &client_id_slot,
                    &leftover,
                );
            }
            let mut read_stream = read_stream;
            let mut recv_buf = [0u8; 65535];
            loop {
                match read_stream.read(&mut recv_buf) {
                    Ok(0) => {
                        let _ = tx.send(HubToClient::Error {
                            message: "studio websocket closed".to_string(),
                        });
                        break;
                    }
                    Ok(n) => {
                        if parse_incoming_frames(
                            &reader_stream,
                            &mut web_socket,
                            &tx,
                            &client_id_slot,
                            &recv_buf[..n],
                        )
                        .is_err()
                        {
                            break;
                        }
                    }
                    Err(err)
                        if matches!(
                            err.kind(),
                            io::ErrorKind::WouldBlock
                                | io::ErrorKind::TimedOut
                                | io::ErrorKind::Interrupted
                        ) => {}
                    Err(err) => {
                        let _ = tx.send(HubToClient::Error {
                            message: format!("studio websocket read error: {err}"),
                        });
                        break;
                    }
                }
            }
        });

        let client_id = recv
            .recv_timeout(HELLO_TIMEOUT)
            .map_err(|err| TestError::new(format!("studio did not send hello: {err}")))
            .and_then(|msg| match msg {
                HubToClient::Hello { client_id } => {
                    *reader_client_id.lock().unwrap() = Some(client_id);
                    Ok(client_id)
                }
                HubToClient::Error { message } => Err(TestError::new(message)),
                other => Err(TestError::new(format!(
                    "expected studio hello, received unexpected message: {other:?}"
                ))),
            })?;

        Ok(Self {
            client_id,
            next_counter: 0,
            stream,
            recv,
            _reader_thread: reader_thread,
        })
    }

    pub fn send(&mut self, msg: ClientToHub) -> TestResult<QueryId> {
        let query_id = QueryId::new(self.client_id, self.next_counter);
        self.next_counter = self.next_counter.wrapping_add(1);
        let envelope = ClientToHubEnvelope { query_id, msg };
        send_binary_frame(&self.stream, &envelope.serialize_bin())
            .map_err(|err| TestError::new(format!("failed to send studio request: {err}")))?;
        Ok(query_id)
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<HubToClient> {
        self.recv.recv_timeout(timeout).ok()
    }
}

fn resolve_host_port(studio: &str) -> TestResult<(String, u16)> {
    let raw = studio.trim();
    if raw.is_empty() {
        return Err(TestError::new("studio address is empty"));
    }
    if raw.contains('/') || raw.contains("://") {
        return Err(TestError::new(format!(
            "invalid studio address `{raw}`, expected ip:port"
        )));
    }
    let (host, port) = raw.rsplit_once(':').ok_or_else(|| {
        TestError::new(format!("invalid studio address `{raw}`, expected ip:port"))
    })?;
    if host.trim().is_empty() {
        return Err(TestError::new(format!(
            "invalid studio address `{raw}`, missing host"
        )));
    }
    let port = port
        .parse::<u16>()
        .map_err(|_| TestError::new(format!("invalid studio address `{raw}`, invalid port")))?;
    Ok((host.to_string(), port))
}

fn parse_incoming_frames(
    stream: &Arc<Mutex<TcpStream>>,
    web_socket: &mut WebSocketParser,
    tx: &mpsc::Sender<HubToClient>,
    client_id: &Arc<Mutex<Option<ClientId>>>,
    bytes: &[u8],
) -> Result<(), ()> {
    web_socket.parse(bytes, |result| match result {
        Ok(ServerWebSocketMessage::Ping(_)) => {
            if let Ok(mut guard) = stream.lock() {
                let _ = write_all_no_error(&mut guard, &SERVER_WEB_SOCKET_PONG_MESSAGE);
            }
        }
        Ok(ServerWebSocketMessage::Pong(_)) => {}
        Ok(ServerWebSocketMessage::Text(text)) => {
            if let Ok(msg) = HubToClient::deserialize_json(text) {
                let current_client_id = record_client_id(client_id, &msg);
                emit_message(tx, current_client_id, msg);
            }
        }
        Ok(ServerWebSocketMessage::Binary(data)) => {
            if let Ok(msg) = HubToClient::deserialize_bin(data) {
                let current_client_id = record_client_id(client_id, &msg);
                emit_message(tx, current_client_id, msg);
            } else if let Ok(text) = std::str::from_utf8(data) {
                if let Ok(msg) = HubToClient::deserialize_json(text) {
                    let current_client_id = record_client_id(client_id, &msg);
                    emit_message(tx, current_client_id, msg);
                }
            }
        }
        Ok(ServerWebSocketMessage::Close) => {
            let _ = tx.send(HubToClient::Error {
                message: "studio websocket closed".to_string(),
            });
        }
        Err(ServerWebSocketError::OpcodeNotSupported(opcode)) => {
            let _ = tx.send(HubToClient::Error {
                message: format!("studio websocket opcode not supported: {opcode}"),
            });
        }
        Err(ServerWebSocketError::TextNotUTF8(_)) => {
            let _ = tx.send(HubToClient::Error {
                message: "studio websocket text payload was not utf8".to_string(),
            });
        }
    });
    Ok(())
}

fn emit_message(tx: &mpsc::Sender<HubToClient>, client_id: Option<ClientId>, msg: HubToClient) {
    if !should_emit_for_client(client_id, &msg) {
        return;
    }
    let _ = tx.send(msg);
}

fn record_client_id(
    client_id_slot: &Arc<Mutex<Option<ClientId>>>,
    msg: &HubToClient,
) -> Option<ClientId> {
    let mut client_id = client_id_slot.lock().unwrap();
    if let HubToClient::Hello {
        client_id: hello_id,
    } = msg
    {
        *client_id = Some(*hello_id);
    }
    *client_id
}

fn should_emit_for_client(client_id: Option<ClientId>, msg: &HubToClient) -> bool {
    let Some(client_id) = client_id else {
        return true;
    };
    let Some(query_id) = message_query_id(msg) else {
        return true;
    };
    query_id.client_id() == client_id
}

fn message_query_id(msg: &HubToClient) -> Option<QueryId> {
    match msg {
        HubToClient::Screenshot { query_id, .. }
        | HubToClient::WidgetTreeDump { query_id, .. }
        | HubToClient::WidgetSnapshot { query_id, .. }
        | HubToClient::WidgetQuery { query_id, .. }
        | HubToClient::FindFileResults { query_id, .. }
        | HubToClient::SearchFileResults { query_id, .. }
        | HubToClient::QueryLogResults { query_id, .. }
        | HubToClient::QueryProfilerResults { query_id, .. }
        | HubToClient::QueryCancelled { query_id } => Some(*query_id),
        _ => None,
    }
}

fn connect_websocket(
    socket_addr: std::net::SocketAddr,
    host_header: &str,
    path: &str,
) -> TestResult<(TcpStream, Vec<u8>)> {
    let mut stream = TcpStream::connect(socket_addr)
        .map_err(|err| TestError::new(format!("failed to connect to studio websocket: {err}")))?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: SxJdXBRtW7Q4awLDhflO0Q==\r\n\r\n",
        path, host_header
    );
    write_all_no_error(&mut stream, request.as_bytes())
        .map_err(|err| TestError::new(format!("failed to write websocket handshake: {err}")))?;
    let leftover = read_websocket_handshake_response(&mut stream)?;
    Ok((stream, leftover))
}

fn send_binary_frame(stream: &Arc<Mutex<TcpStream>>, bytes: &[u8]) -> io::Result<()> {
    let header = ServerWebSocketMessageHeader::from_len(
        bytes.len(),
        ServerWebSocketMessageFormat::Binary,
        true,
    );
    let frame = WebSocketParser::build_message(header, bytes);
    let mut guard = stream.lock().unwrap();
    write_all_no_error(&mut guard, &frame)
}

fn read_websocket_handshake_response(stream: &mut TcpStream) -> TestResult<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut data = Vec::with_capacity(4096);
    let mut buf = [0u8; 4096];

    loop {
        if let Some(end) = find_header_end(&data) {
            let head = String::from_utf8_lossy(&data[..end]);
            let status_line = head.lines().next().unwrap_or_default();
            if !(status_line.starts_with("HTTP/1.1 101") || status_line.starts_with("HTTP/1.0 101"))
            {
                return Err(TestError::new(format!(
                    "studio websocket upgrade rejected: {}",
                    status_line.trim()
                )));
            }
            return Ok(data[end..].to_vec());
        }

        if Instant::now() >= deadline {
            return Err(TestError::new(
                "timeout waiting for studio websocket upgrade response",
            ));
        }

        match stream.read(&mut buf) {
            Ok(0) => {
                return Err(TestError::new(
                    "studio connection closed during websocket handshake",
                ))
            }
            Ok(n) => data.extend_from_slice(&buf[..n]),
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(err) => {
                return Err(TestError::new(format!(
                    "failed to read websocket handshake: {err}"
                )))
            }
        }
    }
}

fn write_all_no_error(stream: &mut TcpStream, bytes: &[u8]) -> io::Result<()> {
    let mut offset = 0usize;
    while offset < bytes.len() {
        match stream.write(&bytes[offset..]) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "socket closed")),
            Ok(n) => offset += n,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    data.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}
