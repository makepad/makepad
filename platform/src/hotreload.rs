use crate::thread::SignalToUI;
use dioxus_devtools_types::DevserverMsg;
use makepad_network::{
    HttpMethod, HttpRequest, ServerWebSocketError, ServerWebSocketMessage, SplitUrl,
    WebSocketParser, SERVER_WEB_SOCKET_PONG_MESSAGE,
};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

pub use subsecond;

static HOTRELOAD_CONNECT_ONCE: Once = Once::new();
const HOTRELOAD_RETRY_DELAY: Duration = Duration::from_millis(250);

pub type HotReloadFlag = Arc<AtomicBool>;

pub fn register_signal_handler(flag: &HotReloadFlag) {
    connect_once();

    let flag = flag.clone();
    subsecond::register_handler(Arc::new(move || {
        flag.store(true, Ordering::Release);
        SignalToUI::set_ui_signal();
    }));
}

pub fn connect_once() {
    HOTRELOAD_CONNECT_ONCE.call_once(|| {
        let Some(endpoint) = devserver_ws_endpoint_fallback() else {
            return;
        };

        std::thread::spawn(move || {
            run_hotreload_connection_loop(
                HOTRELOAD_RETRY_DELAY,
                || true,
                || run_hotreload_connection(&endpoint),
            );
        });
    });
}

fn run_hotreload_connection_loop<F, K>(retry_delay: Duration, mut keep_running: K, mut run_once: F)
where
    F: FnMut() -> Result<(), String>,
    K: FnMut() -> bool,
{
    while keep_running() {
        let _ = run_once();
        if keep_running() && !retry_delay.is_zero() {
            std::thread::sleep(retry_delay);
        }
    }
}

fn run_hotreload_connection(endpoint: &str) -> Result<(), String> {
    let request = hotreload_request(endpoint.to_string());
    let split = request.split_url();
    let mut stream = connect_stream(&split)?;

    write_websocket_handshake(&mut stream, &request, &split)?;
    let leftover = read_websocket_handshake_response(&mut stream)?;

    let mut parser = WebSocketParser::new();
    if !leftover.is_empty() && parse_incoming(&mut parser, &mut stream, &leftover) {
        return Err("hotreload websocket disconnected".to_string());
    }

    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    loop {
        let mut buffer = [0u8; 65535];
        match stream.read(&mut buffer) {
            Ok(0) => return Err("hotreload websocket connection closed".to_string()),
            Ok(bytes_read) => {
                if parse_incoming(&mut parser, &mut stream, &buffer[..bytes_read]) {
                    return Err("hotreload websocket disconnected".to_string());
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(err) => {
                return Err(format!("failed to read hotreload websocket message: {err}"));
            }
        }
    }
}

fn devserver_ws_endpoint_fallback() -> Option<String> {
    if let Some(endpoint) = dioxus_cli_config::devserver_ws_endpoint() {
        return Some(endpoint);
    }

    // On iOS/tvOS, the CLI environment variables are often not available at runtime.
    // Fall back to compile-time values that `dx` can set during the build.
    #[cfg(any(target_os = "ios", target_os = "tvos"))]
    return Some(format!(
        "ws://{}:{}/_dioxus",
        option_env!("DIOXUS_DEVSERVER_IP").unwrap_or("127.0.0.1"),
        option_env!("DIOXUS_DEVSERVER_PORT").unwrap_or("8080"),
    ));

    #[cfg(not(any(target_os = "ios", target_os = "tvos")))]
    None
}

fn hotreload_request(endpoint: String) -> HttpRequest {
    let build_id = dioxus_cli_config::build_id();
    #[cfg(any(target_os = "ios", target_os = "tvos"))]
    let build_id = if build_id != 0 {
        build_id
    } else {
        option_env!("DIOXUS_BUILD_ID")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    };

    let url = format!(
        "{endpoint}?aslr_reference={}&build_id={}&pid={}",
        subsecond::aslr_reference(),
        build_id,
        std::process::id()
    );
    HttpRequest::new(url, HttpMethod::GET)
}

fn connect_stream(split: &SplitUrl<'_>) -> Result<TcpStream, String> {
    if !matches!(split.proto, "ws" | "http") {
        return Err(format!(
            "unsupported hotreload websocket scheme: {}",
            split.proto
        ));
    }

    let stream = TcpStream::connect(format!("{}:{}", split.host, split.port))
        .map_err(|err| format!("failed to connect to hotreload websocket: {err}"))?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    Ok(stream)
}

fn write_websocket_handshake(
    stream: &mut TcpStream,
    request: &HttpRequest,
    split: &SplitUrl<'_>,
) -> Result<(), String> {
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

    stream
        .write_all(http_request.as_bytes())
        .map_err(|err| format!("failed to write hotreload websocket handshake: {err}"))?;
    stream
        .flush()
        .map_err(|err| format!("failed to flush hotreload websocket handshake: {err}"))
}

fn read_websocket_handshake_response(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let start = std::time::Instant::now();
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(end_of_headers) = find_header_terminator(&buffer) {
            let header_bytes = &buffer[..end_of_headers];
            let header_text = String::from_utf8_lossy(header_bytes);
            let first_line = header_text.lines().next().unwrap_or("");
            if !first_line.contains("101") {
                return Err(format!("websocket upgrade rejected: {first_line}"));
            }
            return Ok(buffer[end_of_headers + 4..].to_vec());
        }

        if start.elapsed() > Duration::from_secs(5) {
            return Err("timeout waiting for websocket upgrade response".to_string());
        }

        match stream.read(&mut chunk) {
            Ok(0) => return Err("connection closed during websocket handshake".to_string()),
            Ok(bytes_read) => buffer.extend_from_slice(&chunk[..bytes_read]),
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

fn find_header_terminator(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_incoming(parser: &mut WebSocketParser, stream: &mut TcpStream, data: &[u8]) -> bool {
    let mut disconnected = false;
    parser.parse(data, |result| match result {
        Ok(ServerWebSocketMessage::Ping(_)) => {
            let _ = send_pong(stream);
        }
        Ok(ServerWebSocketMessage::Pong(_)) => {}
        Ok(ServerWebSocketMessage::Text(text)) => {
            if let Ok(msg) = serde_json::from_str::<DevserverMsg>(text) {
                handle_devserver_msg(msg);
            }
        }
        Ok(ServerWebSocketMessage::Binary(_)) => {}
        Ok(ServerWebSocketMessage::Close) => {
            disconnected = true;
        }
        Err(ServerWebSocketError::OpcodeNotSupported(_))
        | Err(ServerWebSocketError::TextNotUTF8(_)) => {
            disconnected = true;
        }
    });
    disconnected
}

fn send_pong(stream: &mut TcpStream) -> Result<(), String> {
    stream
        .write_all(&SERVER_WEB_SOCKET_PONG_MESSAGE)
        .map_err(|err| format!("failed to write hotreload websocket pong: {err}"))?;
    stream
        .flush()
        .map_err(|err| format!("failed to flush hotreload websocket pong: {err}"))
}

fn handle_devserver_msg(msg: DevserverMsg) {
    if let DevserverMsg::HotReload(hot_reload_msg) = msg {
        if let Some(jump_table) = hot_reload_msg.jump_table {
            if hot_reload_msg.for_pid == Some(std::process::id()) {
                if let Err(err) = unsafe { subsecond::apply_patch(jump_table) } {
                    crate::error!("failed to apply hotreload patch: {err}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn hotreload_connection_loop_retries_after_disconnect() {
        let attempts = AtomicUsize::new(0);

        run_hotreload_connection_loop(
            Duration::ZERO,
            || attempts.load(Ordering::Relaxed) < 3,
            || {
                attempts.fetch_add(1, Ordering::Relaxed);
                Err("disconnected".to_string())
            },
        );

        assert_eq!(attempts.load(Ordering::Relaxed), 3);
    }
}
