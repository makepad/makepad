use crate::types::{HttpRequest, WebSocketMessage};
use crate::web_socket_parser::{
    WebSocketMessage as ParsedWebSocketMessage, WebSocketMessageFormat, WebSocketMessageHeader,
    WebSocketParser, SERVER_WEB_SOCKET_PONG_MESSAGE,
};
use makepad_live_id::LiveId;
use std::{
    io::{Read, Write},
    sync::mpsc::{channel, Sender, TryRecvError},
    time::{Duration, Instant},
};

use super::socket_stream::SocketStream;

pub struct LinuxWebSocket {
    sender: Option<Sender<WebSocketMessage>>,
}

impl Drop for LinuxWebSocket {
    fn drop(&mut self) {
        self.sender.take();
    }
}

impl LinuxWebSocket {
    pub fn send_message(&mut self, message: WebSocketMessage) -> Result<(), ()> {
        if let Some(sender) = &mut self.sender {
            if sender.send(message).is_err() {
                return Err(());
            }
            return Ok(());
        }
        Err(())
    }

    pub fn close(&mut self) {
        self.sender.take();
    }

    pub fn open(
        _socket_id: LiveId,
        request: HttpRequest,
        rx_sender: Sender<WebSocketMessage>,
    ) -> LinuxWebSocket {
        let split = request.split_url();
        let is_tls = match split.proto {
            "ws" | "http" => false,
            "wss" | "https" => true,
            _ => {
                let _ = rx_sender.send(WebSocketMessage::Error(format!(
                    "unsupported websocket scheme: {}",
                    split.proto
                )));
                return LinuxWebSocket { sender: None };
            }
        };

        let mut stream =
            match SocketStream::connect(split.host, split.port, is_tls, request.ignore_ssl_cert) {
                Ok(stream) => stream,
                Err(err) => {
                    let _ = rx_sender.send(WebSocketMessage::Error(format!(
                        "Error connecting websocket stream: {err}"
                    )));
                    return LinuxWebSocket { sender: None };
                }
            };

        let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

        let path = if split.file.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", split.file)
        };
        let default_port = if is_tls { "443" } else { "80" };
        let host_header = if split.port == default_port {
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
            return LinuxWebSocket { sender: None };
        }

        let leftover = match read_websocket_handshake_response(&mut stream) {
            Ok(leftover) => leftover,
            Err(err) => {
                let _ = rx_sender.send(WebSocketMessage::Error(err));
                return LinuxWebSocket { sender: None };
            }
        };

        let (sender, receiver) = channel();

        let _io_thread = std::thread::spawn(move || {
            let mut web_socket = WebSocketParser::new();
            let mut done = false;
            if !leftover.is_empty() {
                parse_incoming(
                    &mut web_socket,
                    &mut stream,
                    &rx_sender,
                    &mut done,
                    &leftover,
                );
            }

            while !done {
                loop {
                    match receiver.try_recv() {
                        Ok(msg) => {
                            if handle_outgoing_message(&mut stream, msg) {
                                done = true;
                                break;
                            }
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            done = true;
                            break;
                        }
                    }
                }

                if done {
                    break;
                }

                let mut buffer = [0u8; 65535];
                match stream.read(&mut buffer) {
                    Ok(0) => {
                        let _ = rx_sender.send(WebSocketMessage::Closed);
                        done = true;
                    }
                    Ok(bytes_read) => parse_incoming(
                        &mut web_socket,
                        &mut stream,
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
            stream.shutdown();
        });

        LinuxWebSocket {
            sender: Some(sender),
        }
    }
}

fn handle_outgoing_message(stream: &mut SocketStream, msg: WebSocketMessage) -> bool {
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
    stream: &mut SocketStream,
    rx_sender: &Sender<WebSocketMessage>,
    done: &mut bool,
    bytes: &[u8],
) {
    web_socket.parse(bytes, |result| match result {
        Ok(ParsedWebSocketMessage::Ping(_)) => {
            if write_all_no_error(stream, &SERVER_WEB_SOCKET_PONG_MESSAGE) {
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

fn read_websocket_handshake_response(stream: &mut SocketStream) -> Result<Vec<u8>, String> {
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

fn write_all_no_error(stream: &mut SocketStream, bytes: &[u8]) -> bool {
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
                continue;
            }
            Err(_) => return true,
        }
    }
    false
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    data.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
}
