use makepad_http::websocket::{
    ServerWebSocket, ServerWebSocketError, ServerWebSocketMessage, ServerWebSocketMessageFormat,
    ServerWebSocketMessageHeader, SERVER_WEB_SOCKET_PONG_MESSAGE,
};
use makepad_micro_serde::*;
use std::collections::HashMap;
use std::env;
use std::io::{self, BufRead, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_STUDIO_HOST_PORT: &str = "127.0.0.1:8001";
const DEFAULT_STUDIO_REMOTE_PATH: &str = "/$studio_remote";

#[derive(Debug, Clone, SerJson, DeJson)]
enum StudioRemoteRequest {
    CargoRun {
        args: Vec<String>,
        root: Option<String>,
        startup_query: Option<String>,
        env: Option<HashMap<String, String>>,
    },
    Stop {
        build_id: u64,
    },
}

fn show_studio_help() {
    eprintln!("Studio websocket remote");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  cargo makepad studio [terminal|studio_remote] [--studio=IP:PORT]");
    eprintln!("  cargo makepad studio run [--studio=IP:PORT] [--root=ROOT] [cargo run args]");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  cargo makepad studio");
    eprintln!("  cargo makepad studio --studio=127.0.0.1:8001");
    eprintln!("  cargo makepad studio run -p makepad-example-splash --release");
    eprintln!("  cargo makepad studio run --root=makepad -- -p makepad-example-splash");
    eprintln!(
        "  echo '{{\"Screenshot\":{{\"build_id\":1234,\"kind_id\":0}}}}' | cargo makepad studio"
    );
    eprintln!("  echo '{{\"WidgetTreeDump\":{{\"build_id\":1234}}}}' | cargo makepad studio");
}

pub fn handle_studio(args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        show_studio_help();
        return Ok(());
    }

    let mut mode_run = false;
    let mut index = 0usize;
    if let Some(first) = args.first() {
        match first.as_str() {
            "terminal" | "studio_remote" => {
                index = 1;
            }
            "run" => {
                mode_run = true;
                index = 1;
            }
            _ => {}
        }
    }

    let mut studio: Option<String> = None;
    let mut root: Option<String> = None;
    let mut cargo_run_args = Vec::new();

    while index < args.len() {
        let arg = &args[index];
        if let Some(v) = arg.strip_prefix("--studio=") {
            studio = Some(v.to_string());
        } else if arg == "--studio" {
            index += 1;
            if index >= args.len() {
                return Err("missing value after --studio".to_string());
            }
            studio = Some(args[index].clone());
        } else if let Some(v) = arg.strip_prefix("--root=") {
            if !mode_run {
                return Err("--root is only supported with 'studio run'".to_string());
            }
            root = Some(v.to_string());
        } else if arg == "--root" {
            if !mode_run {
                return Err("--root is only supported with 'studio run'".to_string());
            }
            index += 1;
            if index >= args.len() {
                return Err("missing value after --root".to_string());
            }
            root = Some(args[index].clone());
        } else if mode_run {
            cargo_run_args.push(arg.clone());
        } else if !arg.starts_with('-') && studio.is_none() {
            studio = Some(arg.to_string());
        } else {
            return Err(format!("unsupported studio argument: '{arg}'"));
        }
        index += 1;
    }

    let target = resolve_host_port(studio)?;
    if mode_run {
        let request = StudioRemoteRequest::CargoRun {
            args: cargo_run_args,
            root,
            startup_query: None,
            env: None,
        };
        run_studio_remote(target, vec![request.serialize_json()])
    } else {
        run_studio_remote(target, Vec::new())
    }
}

fn resolve_host_port(studio_override: Option<String>) -> Result<(String, u16), String> {
    let raw = if let Some(studio) = studio_override {
        studio
    } else if let Ok(studio) = env::var("STUDIO") {
        studio
    } else {
        DEFAULT_STUDIO_HOST_PORT.to_string()
    };

    let raw = raw.trim();
    if raw.is_empty() {
        return Err("studio ip:port is empty".to_string());
    }
    if raw.contains('/') || raw.contains("://") {
        return Err(format!("invalid studio address '{raw}', expected ip:port"));
    }

    let (host, port) = raw
        .rsplit_once(':')
        .ok_or_else(|| format!("invalid studio address '{raw}', expected ip:port"))?;
    if host.trim().is_empty() {
        return Err(format!("invalid studio address '{raw}', missing host"));
    }
    let port = port
        .parse::<u16>()
        .map_err(|_| format!("invalid studio address '{raw}', invalid port"))?;

    Ok((host.to_string(), port))
}

fn run_studio_remote(
    target: (String, u16),
    initial_messages: Vec<String>,
) -> Result<(), String> {
    let (host, port) = target;
    let host_header = format!("{host}:{port}");
    let addr = host_header.clone();
    let mut addrs = addr
        .to_socket_addrs()
        .map_err(|e| format!("failed to resolve studio address {addr}: {e}"))?;
    let socket_addr = addrs
        .next()
        .ok_or_else(|| format!("failed to resolve studio address {addr}"))?;

    let mut stream = TcpStream::connect(socket_addr)
        .map_err(|e| format!("failed to connect to studio websocket at {addr}: {e}"))?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: SxJdXBRtW7Q4awLDhflO0Q==\r\n\r\n",
        DEFAULT_STUDIO_REMOTE_PATH, host_header
    );
    write_all_no_error(&mut stream, request.as_bytes())
        .map_err(|e| format!("failed to write websocket handshake request: {e}"))?;

    let leftover = read_websocket_handshake_response(&mut stream)?;
    let mut read_stream = stream
        .try_clone()
        .map_err(|e| format!("failed to clone websocket stream for reading: {e}"))?;
    let write_stream = Arc::new(Mutex::new(stream));
    let is_done = Arc::new(AtomicBool::new(false));

    for message in initial_messages {
        let message = message.trim();
        if message.is_empty() {
            continue;
        }
        send_text_frame(&write_stream, message)
            .map_err(|e| format!("failed to send initial studio request: {e}"))?;
    }

    {
        let write_stream = write_stream.clone();
        let is_done = is_done.clone();
        thread::spawn(move || {
            let stdin = io::stdin();
            let mut stdin = stdin.lock();
            let mut line = String::new();
            while !is_done.load(Ordering::Relaxed) {
                line.clear();
                match stdin.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        let text = line.trim_end_matches(&['\r', '\n'][..]);
                        if text.is_empty() {
                            continue;
                        }
                        if let Err(err) = JsonValue::deserialize_json(text) {
                            eprintln!("studio remote: invalid json request: {err:?}");
                            continue;
                        }
                        if let Err(err) = send_text_frame(&write_stream, text) {
                            eprintln!("studio remote: failed to send websocket text frame: {err}");
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            is_done.store(true, Ordering::Relaxed);
        });
    }

    let mut web_socket = ServerWebSocket::new();
    if !leftover.is_empty() {
        parse_incoming_frames(&write_stream, &mut web_socket, &is_done, &leftover)?;
    }

    let mut recv_buf = [0u8; 65535];
    while !is_done.load(Ordering::Relaxed) {
        let read = match read_stream.read(&mut recv_buf) {
            Ok(0) => {
                is_done.store(true, Ordering::Relaxed);
                break;
            }
            Ok(n) => n,
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
            Err(err) => {
                return Err(format!("studio websocket read error: {err}"));
            }
        };

        parse_incoming_frames(&write_stream, &mut web_socket, &is_done, &recv_buf[..read])?;
    }

    Ok(())
}

fn send_text_frame(stream: &Arc<Mutex<TcpStream>>, text: &str) -> io::Result<()> {
    let header = ServerWebSocketMessageHeader::from_len(
        text.len(),
        ServerWebSocketMessageFormat::Text,
        true,
    );
    let frame = ServerWebSocket::build_message(header, text.as_bytes());
    let mut guard = stream.lock().unwrap();
    write_all_no_error(&mut guard, &frame)
}

fn parse_incoming_frames(
    stream: &Arc<Mutex<TcpStream>>,
    web_socket: &mut ServerWebSocket,
    is_done: &Arc<AtomicBool>,
    bytes: &[u8],
) -> Result<(), String> {
    let mut out = io::stdout();
    web_socket.parse(bytes, |result| match result {
        Ok(ServerWebSocketMessage::Ping(_)) => {
            if let Ok(mut guard) = stream.lock() {
                let _ = write_all_no_error(&mut guard, &SERVER_WEB_SOCKET_PONG_MESSAGE);
            }
        }
        Ok(ServerWebSocketMessage::Pong(_)) => {}
        Ok(ServerWebSocketMessage::Text(text)) => {
            let _ = out.write_all(text.as_bytes());
            let _ = out.write_all(b"\n");
            let _ = out.flush();
        }
        Ok(ServerWebSocketMessage::Binary(data)) => {
            if let Ok(text) = std::str::from_utf8(data) {
                let _ = out.write_all(text.as_bytes());
                let _ = out.write_all(b"\n");
                let _ = out.flush();
            } else {
                eprintln!("studio remote: ignoring non-utf8 binary websocket message");
            }
        }
        Ok(ServerWebSocketMessage::Close) => {
            is_done.store(true, Ordering::Relaxed);
        }
        Err(ServerWebSocketError::OpcodeNotSupported(opcode)) => {
            eprintln!("studio remote: websocket opcode not supported: {opcode}");
        }
        Err(ServerWebSocketError::TextNotUTF8(_)) => {
            eprintln!("studio remote: non-utf8 text websocket message");
        }
    });
    Ok(())
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
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(err) => return Err(format!("failed to read websocket handshake: {err}")),
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
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
}
