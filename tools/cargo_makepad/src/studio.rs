use makepad_micro_serde::*;
use makepad_network::{
    ServerWebSocketError, ServerWebSocketMessage, ServerWebSocketMessageFormat,
    ServerWebSocketMessageHeader, WebSocketParser, SERVER_WEB_SOCKET_PONG_MESSAGE,
};
use makepad_studio_protocol::backend_protocol::{
    ClientId, QueryId, StudioToUI, UIToStudio, UIToStudioEnvelope,
};
use std::collections::VecDeque;
use std::env;
use std::io::{self, BufRead, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_STUDIO_HOST_PORT: &str = "127.0.0.1:8001";
const STUDIO_UI_PATH: &str = "/$studio_ui";
const LEGACY_STUDIO_REMOTE_PATH: &str = "/$studio_remote";

struct BridgeState {
    client_id: Option<ClientId>,
    next_counter: u64,
}

fn show_studio_help() {
    eprintln!("Studio websocket bridge (filtered protocol passthrough)");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  cargo makepad studio [studio_remote] [--studio=IP:PORT]");
    eprintln!("  cargo makepad studio run [--studio=IP:PORT] [--root=ROOT] [cargo run args]");
    eprintln!();
    eprintln!("Stdin JSON lines accepted:");
    eprintln!("  UIToStudio");
    eprintln!();
    eprintln!("Stdout JSON lines emitted:");
    eprintln!("  StudioToUI");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  echo '{{\"ListBuilds\":[]}}' | cargo makepad studio");
    eprintln!(
        "  echo '{{\"CargoRun\":{{\"mount\":\"makepad\",\"args\":[\"run\",\"-p\",\"makepad-example-splash\"],\"startup_query\":null,\"env\":null,\"buildbox\":null}}}}' | cargo makepad studio"
    );
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
            "studio_remote" => {
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
        } else if !mode_run && arg == "terminal" {
            return Err(format!("unsupported studio argument: '{arg}'"));
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

    let mut initial_messages = Vec::new();
    if mode_run {
        let mount = root.unwrap_or_else(default_mount_from_env);
        initial_messages.push(UIToStudio::CargoRun {
            mount,
            args: normalize_cargo_run_args(cargo_run_args)?,
            startup_query: None,
            env: None,
            buildbox: None,
        });
    }

    run_studio_remote(target, initial_messages)
}

fn default_mount_from_env() -> String {
    env::var("STUDIO_ROOT")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            env::var("STUDIO_MOUNT")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .unwrap_or_else(|| "makepad".to_string())
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

fn run_studio_remote(target: (String, u16), initial_messages: Vec<UIToStudio>) -> Result<(), String> {
    let (host, port) = target;
    let host_header = format!("{host}:{port}");
    let addr = host_header.clone();
    let mut addrs = addr
        .to_socket_addrs()
        .map_err(|e| format!("failed to resolve studio address {addr}: {e}"))?;
    let socket_addr = addrs
        .next()
        .ok_or_else(|| format!("failed to resolve studio address {addr}"))?;

    let mut last_err = None;
    let mut selected_path = None;
    let mut selected_stream = None;
    let mut selected_leftover = Vec::new();
    for path in [LEGACY_STUDIO_REMOTE_PATH, STUDIO_UI_PATH] {
        match connect_websocket(socket_addr, &host_header, path) {
            Ok((stream, leftover)) => {
                selected_path = Some(path);
                selected_stream = Some(stream);
                selected_leftover = leftover;
                break;
            }
            Err(err) => {
                last_err = Some(format!("{path}: {err}"));
            }
        }
    }

    let path = selected_path.ok_or_else(|| {
        format!(
            "failed to connect to studio websocket at {addr} (tried {}, {}): {}",
            LEGACY_STUDIO_REMOTE_PATH,
            STUDIO_UI_PATH,
            last_err.unwrap_or_else(|| "unknown error".to_string())
        )
    })?;
    let stream = selected_stream.expect("selected stream exists");
    let mut read_stream = stream
        .try_clone()
        .map_err(|e| format!("failed to clone websocket stream for reading: {e}"))?;
    let write_stream = Arc::new(Mutex::new(stream));

    let mut state = BridgeState {
        client_id: None,
        next_counter: 0,
    };

    let mut out = io::stdout();
    let mut web_socket = WebSocketParser::new();
    if !selected_leftover.is_empty() {
        parse_incoming_frames(
            &write_stream,
            &mut web_socket,
            &mut state,
            &mut out,
            &selected_leftover,
        )?;
    }

    if state.client_id.is_none() {
        let hello_deadline = Instant::now() + Duration::from_secs(3);
        let mut recv_buf = [0u8; 65535];
        while state.client_id.is_none() {
            if Instant::now() >= hello_deadline {
                return Err(format!(
                    "studio did not send Hello on {} (expected StudioToUI Hello handshake)",
                    path
                ));
            }
            let read = match read_stream.read(&mut recv_buf) {
                Ok(0) => return Err("connection closed before Hello".to_string()),
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
                    return Err(format!(
                        "studio websocket read error while waiting for Hello: {err}"
                    ))
                }
            };
            parse_incoming_frames(
                &write_stream,
                &mut web_socket,
                &mut state,
                &mut out,
                &recv_buf[..read],
            )?;
        }
    }

    let mut pending_envelopes = VecDeque::new();
    for msg in initial_messages {
        pending_envelopes.push_back(make_envelope(&mut state, msg)?);
    }

    let (stdin_tx, stdin_rx) = mpsc::channel::<Option<String>>();
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) => {
                    let _ = stdin_tx.send(None);
                    break;
                }
                Ok(_) => {
                    let text = line.trim_end_matches(&['\r', '\n'][..]).to_string();
                    if text.is_empty() {
                        continue;
                    }
                    let _ = stdin_tx.send(Some(text));
                }
                Err(_) => {
                    let _ = stdin_tx.send(None);
                    break;
                }
            }
        }
    });

    let mut stdin_closed = false;
    let mut shutdown_deadline: Option<Instant> = None;
    let mut recv_buf = [0u8; 65535];

    loop {
        while let Ok(line) = stdin_rx.try_recv() {
            match line {
                Some(line) => {
                    match UIToStudio::deserialize_json(&line) {
                        Ok(msg) => match make_envelope(&mut state, msg) {
                            Ok(envelope) => pending_envelopes.push_back(envelope),
                            Err(err) => {
                                eprintln!("studio remote: {err}");
                            }
                        },
                        Err(err) => {
                            eprintln!("studio remote: invalid request json (expected UIToStudio): {err:?}");
                        }
                    }
                    shutdown_deadline = None;
                }
                None => {
                    stdin_closed = true;
                    if shutdown_deadline.is_none() {
                        shutdown_deadline = Some(Instant::now() + Duration::from_millis(700));
                    }
                }
            }
        }

        while let Some(envelope) = pending_envelopes.pop_front() {
            send_ui_envelope(&write_stream, envelope)?;
        }

        let read = match read_stream.read(&mut recv_buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                0
            }
            Err(err) => return Err(format!("studio websocket read error: {err}")),
        };
        if read > 0 {
            parse_incoming_frames(
                &write_stream,
                &mut web_socket,
                &mut state,
                &mut out,
                &recv_buf[..read],
            )?;
            if stdin_closed {
                shutdown_deadline = Some(Instant::now() + Duration::from_millis(700));
            }
        }

        if stdin_closed
            && pending_envelopes.is_empty()
            && shutdown_deadline.is_some_and(|deadline| Instant::now() >= deadline)
        {
            break;
        }
    }

    Ok(())
}

fn make_envelope(state: &mut BridgeState, msg: UIToStudio) -> Result<UIToStudioEnvelope, String> {
    let client_id = state
        .client_id
        .ok_or_else(|| "missing studio hello/client_id".to_string())?;
    let query_id = QueryId::new(client_id, state.next_counter);
    state.next_counter = state.next_counter.wrapping_add(1);
    Ok(UIToStudioEnvelope { query_id, msg })
}

fn send_ui_envelope(
    write_stream: &Arc<Mutex<TcpStream>>,
    envelope: UIToStudioEnvelope,
) -> Result<(), String> {
    send_binary_frame(write_stream, &envelope.serialize_bin())
        .map_err(|e| format!("failed to send studio request: {e}"))
}

fn has_message_format_json(args: &[String]) -> bool {
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--message-format=json" {
            return true;
        }
        if arg == "--message-format" && iter.peek().is_some_and(|next| next.as_str() == "json") {
            return true;
        }
        if arg
            .strip_prefix("--message-format=")
            .is_some_and(|value| value == "json")
        {
            return true;
        }
    }
    false
}

fn normalize_cargo_run_args(raw_args: Vec<String>) -> Result<Vec<String>, String> {
    let mut args = raw_args;
    if args.first().is_some_and(|arg| arg == "run") {
        args.remove(0);
    }
    if args
        .first()
        .is_some_and(|arg| !arg.starts_with('-') && arg != "--")
    {
        return Err(
            "CargoRun expects args after `cargo run` (do not pass a different cargo subcommand)"
                .to_string(),
        );
    }

    let split_index = args
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len());
    let mut cargo_args = args[..split_index].to_vec();
    let mut app_args = if split_index < args.len() {
        args[(split_index + 1)..].to_vec()
    } else {
        Vec::new()
    };

    if !has_message_format_json(&cargo_args) {
        cargo_args.push("--message-format=json".to_string());
    }
    if !has_message_format_json(&app_args) {
        app_args.insert(0, "--message-format=json".to_string());
    }
    if !app_args.iter().any(|arg| arg == "--stdin-loop") {
        app_args.insert(0, "--stdin-loop".to_string());
    }

    let mut final_args = vec!["run".to_string()];
    final_args.extend(cargo_args);
    final_args.push("--".to_string());
    final_args.extend(app_args);
    Ok(final_args)
}

fn parse_incoming_frames(
    stream: &Arc<Mutex<TcpStream>>,
    web_socket: &mut WebSocketParser,
    state: &mut BridgeState,
    out: &mut io::Stdout,
    bytes: &[u8],
) -> Result<(), String> {
    web_socket.parse(bytes, |result| match result {
        Ok(ServerWebSocketMessage::Ping(_)) => {
            if let Ok(mut guard) = stream.lock() {
                let _ = write_all_no_error(&mut guard, &SERVER_WEB_SOCKET_PONG_MESSAGE);
            }
        }
        Ok(ServerWebSocketMessage::Pong(_)) => {}
        Ok(ServerWebSocketMessage::Text(text)) => {
            if let Ok(msg) = StudioToUI::deserialize_json(text) {
                let _ = emit_protocol_response(out, state, msg);
            }
        }
        Ok(ServerWebSocketMessage::Binary(data)) => {
            if let Ok(msg) = StudioToUI::deserialize_bin(data) {
                let _ = emit_protocol_response(out, state, msg);
            } else if let Ok(text) = std::str::from_utf8(data) {
                if let Ok(msg) = StudioToUI::deserialize_json(text) {
                    let _ = emit_protocol_response(out, state, msg);
                } else {
                    eprintln!("studio remote: unrecognized utf8 binary websocket payload");
                }
            } else {
                eprintln!("studio remote: unrecognized binary websocket payload");
            }
        }
        Ok(ServerWebSocketMessage::Close) => {}
        Err(ServerWebSocketError::OpcodeNotSupported(opcode)) => {
            eprintln!("studio remote: websocket opcode not supported: {opcode}");
        }
        Err(ServerWebSocketError::TextNotUTF8(_)) => {
            eprintln!("studio remote: non-utf8 text websocket message");
        }
    });
    Ok(())
}

fn emit_protocol_response(
    out: &mut io::Stdout,
    state: &mut BridgeState,
    msg: StudioToUI,
) -> Result<(), String> {
    if let StudioToUI::Hello { client_id } = msg {
        state.client_id = Some(client_id);
        state.next_counter = 0;
        write_protocol_response(out, StudioToUI::Hello { client_id })
    } else {
        write_protocol_response(out, msg)
    }
}

fn write_protocol_response(out: &mut io::Stdout, msg: StudioToUI) -> Result<(), String> {
    if !should_emit_protocol_response(&msg) {
        return Ok(());
    }

    let json = msg.serialize_json();
    out.write_all(json.as_bytes())
        .map_err(|e| format!("failed to write response: {e}"))?;
    out.write_all(b"\n")
        .map_err(|e| format!("failed to write response newline: {e}"))?;
    out.flush()
        .map_err(|e| format!("failed to flush response: {e}"))?;
    Ok(())
}

fn should_emit_protocol_response(msg: &StudioToUI) -> bool {
    matches!(
        msg,
        StudioToUI::Hello { .. }
            | StudioToUI::Error { .. }
            | StudioToUI::Builds { .. }
            | StudioToUI::RunnableBuilds { .. }
            | StudioToUI::BuildStarted { .. }
            | StudioToUI::BuildStopped { .. }
            | StudioToUI::Screenshot { .. }
            | StudioToUI::WidgetTreeDump { .. }
            | StudioToUI::WidgetQuery { .. }
            | StudioToUI::QueryCancelled { .. }
    )
}

fn connect_websocket(
    socket_addr: std::net::SocketAddr,
    host_header: &str,
    path: &str,
) -> Result<(TcpStream, Vec<u8>), String> {
    let mut stream = TcpStream::connect(socket_addr).map_err(|e| format!("connect failed: {e}"))?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: SxJdXBRtW7Q4awLDhflO0Q==\r\n\r\n",
        path, host_header
    );
    write_all_no_error(&mut stream, request.as_bytes())
        .map_err(|e| format!("failed to write websocket handshake request: {e}"))?;
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
