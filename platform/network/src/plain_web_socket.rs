use crate::types::{HttpRequest, WebSocketMessage};
use crate::web_socket_parser::{
    WebSocketMessage as ParsedWebSocketMessage, WebSocketMessageFormat, WebSocketMessageHeader,
    WebSocketParser, SERVER_WEB_SOCKET_PONG_MESSAGE,
};
use makepad_live_id::LiveId;
use std::{
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    sync::mpsc::{channel, Sender},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// The connection: plain TCP, or pinned TLS shared by the reader and the
/// writer thread (the reader holds the lock only for short timed reads).
enum Conn {
    Plain(TcpStream),
    Tls(Arc<Mutex<crate::SocketStream>>),
}

impl Conn {
    fn shutdown(&self) {
        match self {
            Conn::Plain(s) => {
                let _ = s.shutdown(Shutdown::Both);
            }
            Conn::Tls(s) => {
                if let Ok(mut s) = s.lock() {
                    s.shutdown();
                }
            }
        }
    }
}

pub struct PlainWebSocket {
    sender: Option<Sender<Outgoing>>,
    stream: Option<Conn>,
}

enum Outgoing {
    Message(WebSocketMessage),
    Pong,
}

impl Drop for PlainWebSocket {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(stream) = self.stream.take() {
            stream.shutdown();
        }
    }
}

impl PlainWebSocket {
    pub fn send_message(&mut self, message: WebSocketMessage) -> Result<(), ()> {
        if let Some(sender) = &mut self.sender {
            if sender.send(Outgoing::Message(message)).is_err() {
                return Err(());
            }
            return Ok(());
        }
        Err(())
    }

    pub fn close(&mut self) {
        self.sender.take();
        if let Some(stream) = self.stream.take() {
            stream.shutdown();
        }
    }

    pub fn open(
        _socket_id: LiveId,
        request: HttpRequest,
        rx_sender: Sender<WebSocketMessage>,
    ) -> PlainWebSocket {
        let split = request.split_url();
        match split.proto {
            "http" | "ws" => {}
            "https" | "wss" => return Self::open_pinned(request, rx_sender),
            _ => {
                let _ = rx_sender.send(WebSocketMessage::Error(format!(
                    "unsupported websocket scheme: {}",
                    split.proto
                )));
                return PlainWebSocket {
                    sender: None,
                    stream: None,
                };
            }
        }

        let mut stream = match TcpStream::connect(format!("{}:{}", split.host, split.port)) {
            Ok(stream) => stream,
            Err(err) => {
                let _ = rx_sender.send(WebSocketMessage::Error(format!(
                    "Error connecting websocket stream: {err}"
                )));
                return PlainWebSocket {
                    sender: None,
                    stream: None,
                };
            }
        };

        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

        let path = if split.file.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", split.file)
        };
        let host_header = if split.port == "80" {
            split.host.to_string()
        } else {
            format!("{}:{}", split.host, split.port)
        };

        let mut http_request = format!(
            "GET {path} HTTP/1.1\r\nHost: {host_header}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: SxJdXBRtW7Q4awLDhflO0Q==\r\n"
        );
        http_request.push_str(&request.get_headers_string());
        http_request.push_str("\r\n");

        if write_all_no_error(&mut stream, http_request.as_bytes()) {
            let _ = rx_sender.send(WebSocketMessage::Error(
                "Error writing request to websocket".into(),
            ));
            return PlainWebSocket {
                sender: None,
                stream: None,
            };
        }

        let leftover = match read_websocket_handshake_response(&mut stream) {
            Ok(leftover) => leftover,
            Err(err) => {
                let _ = rx_sender.send(WebSocketMessage::Error(err));
                return PlainWebSocket {
                    sender: None,
                    stream: None,
                };
            }
        };

        // The handshake needs a deadline, but an established connection may
        // idle indefinitely. Outgoing messages have their own worker and must
        // never wait for a read timeout or another message from the peer.
        if let Err(err) = stream.set_read_timeout(None) {
            let _ = rx_sender.send(WebSocketMessage::Error(format!(
                "Error clearing websocket read timeout: {err}"
            )));
            return PlainWebSocket {
                sender: None,
                stream: None,
            };
        }
        let mut read_stream = match stream.try_clone() {
            Ok(stream) => stream,
            Err(err) => {
                let _ = rx_sender.send(WebSocketMessage::Error(format!(
                    "Error cloning websocket stream: {err}"
                )));
                return PlainWebSocket {
                    sender: None,
                    stream: None,
                };
            }
        };
        let mut write_stream = match stream.try_clone() {
            Ok(stream) => stream,
            Err(err) => {
                let _ = rx_sender.send(WebSocketMessage::Error(format!(
                    "Error cloning websocket write stream: {err}"
                )));
                return PlainWebSocket {
                    sender: None,
                    stream: None,
                };
            }
        };

        let (sender, receiver) = channel();
        let writer_events = rx_sender.clone();
        let _write_thread = std::thread::spawn(move || {
            // All writes, including protocol replies, use this one stream so
            // a pong cannot split an application frame's header and payload.
            while let Ok(message) = receiver.recv() {
                let failed = match message {
                    Outgoing::Message(WebSocketMessage::Closed) => break,
                    Outgoing::Message(message) => {
                        handle_outgoing_message(&mut write_stream, message)
                    }
                    Outgoing::Pong => {
                        write_all_no_error(&mut write_stream, &SERVER_WEB_SOCKET_PONG_MESSAGE)
                    }
                };
                if failed {
                    let _ = writer_events.send(WebSocketMessage::Error(
                        "Failed to send websocket data".into(),
                    ));
                    break;
                }
            }
            // Also wakes a reader blocked after the peer stops receiving.
            let _ = write_stream.shutdown(Shutdown::Both);
        });
        let reply_sender = sender.clone();
        let _read_thread = std::thread::spawn(move || {
            let mut web_socket = WebSocketParser::new();
            let mut done = false;
            if !leftover.is_empty() {
                parse_incoming(
                    &mut web_socket,
                    &reply_sender,
                    &rx_sender,
                    &mut done,
                    &leftover,
                );
            }

            while !done {
                let mut buffer = [0u8; 65535];
                match read_stream.read(&mut buffer) {
                    Ok(0) => {
                        let _ = rx_sender.send(WebSocketMessage::Closed);
                        done = true;
                    }
                    Ok(bytes_read) => parse_incoming(
                        &mut web_socket,
                        &reply_sender,
                        &rx_sender,
                        &mut done,
                        &buffer[0..bytes_read],
                    ),
                    Err(err)
                        if matches!(
                            err.kind(),
                            std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(err) => {
                        let _ = rx_sender.send(WebSocketMessage::Error(format!(
                            "Failed to receive data: {err}"
                        )));
                        let _ = rx_sender.send(WebSocketMessage::Closed);
                        done = true;
                    }
                }
            }
            let _ = read_stream.shutdown(Shutdown::Both);
            // The public handle can outlive an EOF. Wake the writer even if
            // that handle still retains its sender.
            let _ = reply_sender.send(Outgoing::Message(WebSocketMessage::Closed));
        });

        PlainWebSocket {
            sender: Some(sender),
            stream: Some(Conn::Plain(stream)),
        }
    }

    /// `wss://` to a fleet endpoint ([`crate::tls::mark_fleet_endpoint`]):
    /// self-signed TLS checked against the fleet known-hosts record, with
    /// the fleet authorization bound to the certificate presented. Other
    /// TLS endpoints are refused (no CA validation here).
    fn open_pinned(request: HttpRequest, rx_sender: Sender<WebSocketMessage>) -> PlainWebSocket {
        let failed = |rx: &Sender<WebSocketMessage>, msg: String| {
            let _ = rx.send(WebSocketMessage::Error(msg));
            PlainWebSocket { sender: None, stream: None }
        };
        let split = request.split_url();
        let host_port = format!("{}:{}", split.host, split.port);
        if !crate::tls::is_fleet_endpoint(&host_port) {
            return failed(&rx_sender, format!("{host_port} is not a known fleet TLS endpoint"));
        }
        let (mut stream, authorization) = match crate::tls::connect_fleet(split.host, split.port) {
            Ok(s) => s,
            Err(err) => return failed(&rx_sender, format!("Error connecting websocket stream: {err}")),
        };
        let path = if split.file.is_empty() { "/".to_string() } else { format!("/{}", split.file) };
        let mut head = format!(
            "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: SxJdXBRtW7Q4awLDhflO0Q==\r\n"
        );
        let headers = request.get_headers_string();
        if !headers.to_ascii_lowercase().contains("authorization:") {
            if let Some(value) = authorization {
                head.push_str(&format!("Authorization: {value}\r\n"));
            }
        }
        head.push_str(&headers);
        head.push_str("\r\n");
        let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
        if write_all_no_error(&mut stream, head.as_bytes()) {
            return failed(&rx_sender, "Error writing request to websocket".into());
        }
        let leftover = match read_websocket_handshake_response(&mut stream) {
            Ok(l) => l,
            Err(err) => return failed(&rx_sender, err),
        };
        let shared = Arc::new(Mutex::new(stream));
        // The writer raises this while it wants the lock; the reader steps
        // aside between reads so outgoing frames never starve.
        let write_wanted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (sender, receiver) = channel();
        let writer = shared.clone();
        let wants = write_wanted.clone();
        let writer_events = rx_sender.clone();
        std::thread::spawn(move || {
            while let Ok(message) = receiver.recv() {
                wants.store(true, std::sync::atomic::Ordering::SeqCst);
                let lock = writer.lock();
                wants.store(false, std::sync::atomic::Ordering::SeqCst);
                let mut s = match lock {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let failed = match message {
                    Outgoing::Message(WebSocketMessage::Closed) => break,
                    Outgoing::Message(message) => handle_outgoing_message(&mut *s, message),
                    Outgoing::Pong => write_all_no_error(&mut *s, &SERVER_WEB_SOCKET_PONG_MESSAGE),
                };
                if failed {
                    let _ = writer_events.send(WebSocketMessage::Error("Failed to send websocket data".into()));
                    break;
                }
            }
            if let Ok(mut s) = writer.lock() {
                s.shutdown();
            }
        });
        let reader = shared.clone();
        let reply_sender = sender.clone();
        std::thread::spawn(move || {
            let mut web_socket = WebSocketParser::new();
            let mut done = false;
            if !leftover.is_empty() {
                parse_incoming(&mut web_socket, &reply_sender, &rx_sender, &mut done, &leftover);
            }
            let mut buffer = vec![0u8; 65535];
            while !done {
                while write_wanted.load(std::sync::atomic::Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_micros(100));
                }
                let result = match reader.lock() {
                    Ok(mut s) => s.read(&mut buffer),
                    Err(_) => break,
                };
                match result {
                    Ok(0) => {
                        let _ = rx_sender.send(WebSocketMessage::Closed);
                        done = true;
                    }
                    Ok(n) => parse_incoming(&mut web_socket, &reply_sender, &rx_sender, &mut done, &buffer[..n]),
                    Err(err)
                        if matches!(
                            err.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted
                        ) =>
                    {}
                    Err(err) => {
                        let _ = rx_sender.send(WebSocketMessage::Error(format!("Failed to receive data: {err}")));
                        let _ = rx_sender.send(WebSocketMessage::Closed);
                        done = true;
                    }
                }
            }
            let _ = reply_sender.send(Outgoing::Message(WebSocketMessage::Closed));
        });
        PlainWebSocket { sender: Some(sender), stream: Some(Conn::Tls(shared)) }
    }
}

fn handle_outgoing_message(stream: &mut dyn Write, msg: WebSocketMessage) -> bool {
    match msg {
        WebSocketMessage::Binary(data) => {
            let header =
                WebSocketMessageHeader::from_len(data.len(), WebSocketMessageFormat::Binary, false);
            write_all_no_error(stream, header.as_slice()) || write_all_no_error(stream, &data)
        }
        WebSocketMessage::String(data) => {
            let header =
                WebSocketMessageHeader::from_len(data.len(), WebSocketMessageFormat::Text, false);
            write_all_no_error(stream, header.as_slice())
                || write_all_no_error(stream, data.as_bytes())
        }
        WebSocketMessage::Closed => true,
        WebSocketMessage::Opened => false,
        WebSocketMessage::Error(_) => false,
    }
}

fn parse_incoming(
    web_socket: &mut WebSocketParser,
    reply_sender: &Sender<Outgoing>,
    rx_sender: &Sender<WebSocketMessage>,
    done: &mut bool,
    bytes: &[u8],
) {
    web_socket.parse(bytes, |result| match result {
        Ok(ParsedWebSocketMessage::Ping(_)) => {
            if reply_sender.send(Outgoing::Pong).is_err() {
                *done = true;
                let _ = rx_sender.send(WebSocketMessage::Error("Pong message send failed".into()));
            }
        }
        Ok(ParsedWebSocketMessage::Pong(_)) => {}
        Ok(ParsedWebSocketMessage::Text(text)) => {
            if rx_sender
                .send(WebSocketMessage::String(text.into()))
                .is_err()
            {
                *done = true;
            }
        }
        Ok(ParsedWebSocketMessage::Binary(data)) => {
            if rx_sender
                .send(WebSocketMessage::Binary(data.into()))
                .is_err()
            {
                *done = true;
            }
        }
        Ok(ParsedWebSocketMessage::Close) => {
            let _ = rx_sender.send(WebSocketMessage::Closed);
            *done = true;
        }
        Err(e) => {
            let _ = rx_sender.send(WebSocketMessage::Error(format!(
                "WebSocket parse error: {e:?}"
            )));
        }
    });
}

fn read_websocket_handshake_response(stream: &mut dyn Read) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut data = Vec::with_capacity(4096);
    let mut buf = [0u8; 4096];

    loop {
        if let Some(end) = find_header_end(&data) {
            let head = String::from_utf8_lossy(&data[..end]);
            let status_line = head.lines().next().unwrap_or_default();
            if !(status_line.starts_with("HTTP/1.1 101") || status_line.starts_with("HTTP/1.0 101"))
            {
                return Err(format!(
                    "websocket upgrade rejected: {}",
                    status_line.trim()
                ));
            }
            return Ok(data[end..].to_vec());
        }

        if Instant::now() >= deadline {
            return Err("timeout waiting for websocket upgrade response".to_string());
        }

        match stream.read(&mut buf) {
            Ok(0) => return Err("connection closed during websocket handshake".to_string()),
            Ok(n) => data.extend_from_slice(&buf[..n]),
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(err) => return Err(format!("failed to read websocket handshake: {err}")),
        }
    }
}

fn write_all_no_error(stream: &mut dyn Write, bytes: &[u8]) -> bool {
    let mut offset = 0usize;
    while offset < bytes.len() {
        match stream.write(&bytes[offset..]) {
            Ok(0) => return true,
            Ok(n) => offset += n,
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(_) => return true,
        }
    }
    false
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    data.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}
