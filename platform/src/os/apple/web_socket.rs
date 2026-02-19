use crate::event::HttpRequest;
use crate::web_socket::WebSocketMessage;
use makepad_http::websocket::{
    ServerWebSocket, ServerWebSocketMessage, ServerWebSocketMessageFormat,
    ServerWebSocketMessageHeader, SERVER_WEB_SOCKET_PONG_MESSAGE,
};
use std::{
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    sync::mpsc::{channel, Sender, TryRecvError},
    time::{Duration, Instant},
};

pub struct OsWebSocket {
    sender: Option<Sender<WebSocketMessage>>,
    stream: Option<TcpStream>,
}

impl Drop for OsWebSocket {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(stream) = self.stream.take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

impl OsWebSocket {
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
        if let Some(stream) = self.stream.take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    pub fn open(
        _socket_id: u64,
        request: HttpRequest,
        rx_sender: Sender<WebSocketMessage>,
    ) -> OsWebSocket {
        let split = request.split_url();
        match split.proto {
            "http" | "ws" => {}
            "https" | "wss" => {
                let _ = rx_sender.send(WebSocketMessage::Error(
                    "TLS websocket is not supported by this client; use ws/http".to_string(),
                ));
                return OsWebSocket {
                    sender: None,
                    stream: None,
                };
            }
            _ => {
                let _ = rx_sender.send(WebSocketMessage::Error(format!(
                    "unsupported websocket scheme: {}",
                    split.proto
                )));
                return OsWebSocket {
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
                return OsWebSocket {
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
            return OsWebSocket {
                sender: None,
                stream: None,
            };
        }

        let leftover = match read_websocket_handshake_response(&mut stream) {
            Ok(leftover) => leftover,
            Err(err) => {
                let _ = rx_sender.send(WebSocketMessage::Error(err));
                return OsWebSocket {
                    sender: None,
                    stream: None,
                };
            }
        };

        let mut io_stream = match stream.try_clone() {
            Ok(stream) => stream,
            Err(err) => {
                let _ = rx_sender.send(WebSocketMessage::Error(format!(
                    "Error cloning websocket stream: {err}"
                )));
                return OsWebSocket {
                    sender: None,
                    stream: None,
                };
            }
        };

        let (sender, receiver) = channel();
        let _io_thread = std::thread::spawn(move || {
            let mut web_socket = ServerWebSocket::new();
            let mut done = false;
            if !leftover.is_empty() {
                parse_incoming(
                    &mut web_socket,
                    &mut io_stream,
                    &rx_sender,
                    &mut done,
                    &leftover,
                );
            }

            while !done {
                loop {
                    match receiver.try_recv() {
                        Ok(msg) => {
                            if handle_outgoing_message(&mut io_stream, msg) {
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
                match io_stream.read(&mut buffer) {
                    Ok(0) => {
                        let _ = rx_sender.send(WebSocketMessage::Closed);
                        done = true;
                    }
                    Ok(bytes_read) => parse_incoming(
                        &mut web_socket,
                        &mut io_stream,
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
            let _ = io_stream.shutdown(Shutdown::Both);
        });

        OsWebSocket {
            sender: Some(sender),
            stream: Some(stream),
        }
    }
}

fn handle_outgoing_message(stream: &mut TcpStream, msg: WebSocketMessage) -> bool {
    match msg {
        WebSocketMessage::Binary(data) => {
            let header = ServerWebSocketMessageHeader::from_len(
                data.len(),
                ServerWebSocketMessageFormat::Binary,
                false,
            );
            write_all_no_error(stream, header.as_slice()) || write_all_no_error(stream, &data)
        }
        WebSocketMessage::String(data) => {
            let header = ServerWebSocketMessageHeader::from_len(
                data.len(),
                ServerWebSocketMessageFormat::Text,
                false,
            );
            write_all_no_error(stream, header.as_slice())
                || write_all_no_error(stream, data.as_bytes())
        }
        WebSocketMessage::Closed => true,
        WebSocketMessage::Opened => false,
        WebSocketMessage::Error(_) => false,
    }
}

fn parse_incoming(
    web_socket: &mut ServerWebSocket,
    stream: &mut TcpStream,
    rx_sender: &Sender<WebSocketMessage>,
    done: &mut bool,
    bytes: &[u8],
) {
    web_socket.parse(bytes, |result| match result {
        Ok(ServerWebSocketMessage::Ping(_)) => {
            if write_all_no_error(stream, &SERVER_WEB_SOCKET_PONG_MESSAGE) {
                *done = true;
                let _ = rx_sender.send(WebSocketMessage::Error("Pong message send failed".into()));
            }
        }
        Ok(ServerWebSocketMessage::Pong(_)) => {}
        Ok(ServerWebSocketMessage::Text(text)) => {
            if rx_sender
                .send(WebSocketMessage::String(text.into()))
                .is_err()
            {
                *done = true;
            }
        }
        Ok(ServerWebSocketMessage::Binary(data)) => {
            if rx_sender
                .send(WebSocketMessage::Binary(data.into()))
                .is_err()
            {
                *done = true;
            }
        }
        Ok(ServerWebSocketMessage::Close) => {
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

fn read_websocket_handshake_response(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
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

fn write_all_no_error(stream: &mut TcpStream, bytes: &[u8]) -> bool {
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
