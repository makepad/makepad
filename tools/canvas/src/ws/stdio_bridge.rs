use makepad_widgets::SignalToUI;
use serde_json::Value;
use std::collections::VecDeque;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use super::types::*;

/// Canvas server — Splash rendering + event bridge.
///
/// Supports both WebSocket and HTTP:
///
/// WS:  ws://localhost:PORT
///   Send: {"splash": "..."} or {"splash_stream": "begin/append/end"}
///   Recv: {"event": "click", "widget": "btn_name"}
///
/// HTTP: http://localhost:PORT
///   POST /splash          body=Splash code     → render
///   POST /splash/stream   body=chunk           → stream append (first call = begin)
///   POST /splash/end                           → stream end
///   GET  /event           → blocking wait for next event (JSON)
///   POST /clear                                → clear panel
pub struct StdioBridge {
    pub commands: Arc<Mutex<VecDeque<CanvasCommand>>>,
    pub signal: SignalToUI,
    /// Channels for broadcasting events to all connected WS clients.
    /// Each entry is (sender, id) for explicit cleanup on disconnect.
    event_senders: Arc<Mutex<Vec<(std::sync::mpsc::Sender<String>, u64)>>>,
    /// Monotonic ID for sender registration
    next_sender_id: Arc<Mutex<u64>>,
    /// Event queue for HTTP polling
    event_queue: Arc<Mutex<VecDeque<String>>>,
    /// Notify for HTTP event waiters
    event_notify: Arc<tokio::sync::Notify>,
    port: Arc<Mutex<u16>>,
}

impl StdioBridge {
    pub fn new(signal: SignalToUI) -> Self {
        Self {
            commands: Arc::new(Mutex::new(VecDeque::new())),
            signal,
            event_senders: Arc::new(Mutex::new(Vec::new())),
            next_sender_id: Arc::new(Mutex::new(0)),
            event_queue: Arc::new(Mutex::new(VecDeque::new())),
            event_notify: Arc::new(tokio::sync::Notify::new()),
            port: Arc::new(Mutex::new(0)),
        }
    }

    fn push_cmd(&self, cmd: CanvasCommand) {
        if let Ok(mut q) = self.commands.lock() {
            q.push_back(cmd);
        }
        self.signal.set();
    }

    /// Register a new event sender, returning its ID for later removal.
    fn register_sender(&self, tx: std::sync::mpsc::Sender<String>) -> u64 {
        let id = {
            let mut next = self.next_sender_id.lock().unwrap();
            let id = *next;
            *next += 1;
            id
        };
        if let Ok(mut senders) = self.event_senders.lock() {
            senders.push((tx, id));
        }
        id
    }

    /// Remove a sender by ID (called on WS disconnect).
    fn unregister_sender(&self, id: u64) {
        if let Ok(mut senders) = self.event_senders.lock() {
            senders.retain(|(_, sid)| *sid != id);
        }
    }

    /// Send a widget event back to connected clients (WS + HTTP queue).
    pub fn send_event(&self, widget_name: &str) {
        let msg = serde_json::json!({"event": "click", "widget": widget_name});
        let json = serde_json::to_string(&msg).unwrap_or_default();

        // WS broadcast: send to all connected clients, remove dead ones
        if let Ok(mut senders) = self.event_senders.lock() {
            senders.retain(|(tx, _)| tx.send(json.clone()).is_ok());
        }

        // HTTP event queue
        if let Ok(mut q) = self.event_queue.lock() {
            q.push_back(json);
        }
        self.event_notify.notify_one();
    }

    pub fn start(self: &Arc<Self>) {
        let bridge = self.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.block_on(async move {
                bridge.run_server().await;
            });
        });
    }

    async fn run_server(self: &Arc<Self>) {
        use tokio::net::TcpListener as TokioTcpListener;

        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
            listener.local_addr().unwrap().port()
        };
        {
            let mut p = self.port.lock().unwrap();
            *p = port;
        }

        let listener = TokioTcpListener::bind(format!("127.0.0.1:{}", port))
            .await
            .expect("Failed to bind server");

        makepad_widgets::log!("[CANVAS] Server listening on 127.0.0.1:{}", port);

        if let Err(e) = std::fs::write("/tmp/makepad-canvas.port", port.to_string()) {
            makepad_widgets::log!("[CANVAS] Failed to write port file: {}", e);
        }

        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let bridge = self.clone();
                    tokio::spawn(async move {
                        bridge.handle_tcp(stream).await;
                    });
                }
                Err(e) => {
                    makepad_widgets::log!("[CANVAS] Accept error: {}", e);
                }
            }
        }
    }

    /// Detect if incoming TCP is HTTP or WebSocket upgrade, route accordingly.
    async fn handle_tcp(self: &Arc<Self>, stream: tokio::net::TcpStream) {
        // Peek at first bytes WITHOUT consuming them (important for WS handshake).
        // Use a large buffer to avoid truncating WS upgrade headers.
        let mut peek_buf = [0u8; 8192];
        let n = match stream.peek(&mut peek_buf).await {
            Ok(n) => n,
            Err(_) => return,
        };

        let is_post = n >= 4 && &peek_buf[..4] == b"POST";
        let is_get = n >= 3 && &peek_buf[..3] == b"GET";

        if is_get {
            let peeked = String::from_utf8_lossy(&peek_buf[..n]);
            // Case-insensitive check for WebSocket upgrade header
            let peeked_lower = peeked.to_ascii_lowercase();
            if peeked_lower.contains("upgrade: websocket") {
                // WS: pass stream untouched — accept_async reads the handshake itself
                self.handle_ws(stream).await;
            } else {
                let (request, stream) = self.read_http_request(stream).await;
                self.handle_http(stream, &request).await;
            }
        } else if is_post {
            let (request, stream) = self.read_http_request(stream).await;
            self.handle_http(stream, &request).await;
        } else {
            // Unknown protocol, try as WebSocket
            self.handle_ws(stream).await;
        }
    }

    /// Read a full HTTP request from the stream (consuming bytes).
    async fn read_http_request(&self, mut stream: tokio::net::TcpStream) -> (String, tokio::net::TcpStream) {
        use tokio::io::AsyncReadExt;

        let mut raw = Vec::with_capacity(8192);
        let mut read_buf = vec![0u8; 65536];
        // First read: wait up to 2s for initial data
        match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            stream.read(&mut read_buf),
        ).await {
            Ok(Ok(0)) => {},
            Ok(Ok(n)) => raw.extend_from_slice(&read_buf[..n]),
            _ => {},
        }
        // Subsequent reads: short timeout (data already flowing)
        loop {
            match tokio::time::timeout(
                std::time::Duration::from_millis(50),
                stream.read(&mut read_buf),
            ).await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => raw.extend_from_slice(&read_buf[..n]),
                _ => break,
            }
            if raw.len() > 512 * 1024 { break; }
        }
        let request = String::from_utf8_lossy(&raw).to_string();
        (request, stream)
    }

    async fn handle_ws(self: &Arc<Self>, stream: tokio::net::TcpStream) {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let ws_stream = match tokio_tungstenite::accept_async(stream).await {
            Ok(ws) => ws,
            Err(e) => {
                makepad_widgets::log!("[CANVAS] WS handshake failed: {}", e);
                return;
            }
        };
        self.push_cmd(CanvasCommand::ConnectionState { connected: true });

        let (mut ws_write, mut ws_read) = ws_stream.split();

        // Event channel: UI → this WS client
        let (std_tx, std_rx) = std::sync::mpsc::channel::<String>();
        let sender_id = self.register_sender(std_tx);

        tokio::spawn(async move {
            let (tokio_tx, mut tokio_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            std::thread::spawn(move || {
                while let Ok(msg) = std_rx.recv() {
                    if tokio_tx.send(msg).is_err() { break; }
                }
            });
            while let Some(msg) = tokio_rx.recv().await {
                if ws_write.send(Message::Text(msg)).await.is_err() { break; }
            }
        });

        while let Some(msg) = ws_read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    for line in text.split('\n') {
                        let line = line.trim();
                        if line.is_empty() { continue; }
                        match serde_json::from_str::<Value>(line) {
                            Ok(json) => self.handle_message(&json),
                            Err(_) => self.push_cmd(CanvasCommand::SplashRender { code: line.to_string() }),
                        }
                    }
                }
                Ok(Message::Close(_)) => break,
                Err(_) => break,
                _ => {}
            }
        }

        // Explicitly remove sender on disconnect (no leak)
        self.unregister_sender(sender_id);
        self.push_cmd(CanvasCommand::ConnectionState { connected: false });
    }

    async fn handle_http(self: &Arc<Self>, stream: tokio::net::TcpStream, request: &str) {
        use tokio::io::AsyncWriteExt;

        // Mark as connected on any HTTP activity (not just WS)
        self.push_cmd(CanvasCommand::ConnectionState { connected: true });

        let (method, path, body) = parse_http_request(request);

        let response = match (method.as_str(), path.as_str()) {
            ("POST", "/splash") => {
                if !body.is_empty() {
                    self.push_cmd(CanvasCommand::SplashRender { code: body.clone() });
                }
                http_response(200, r#"{"ok":true}"#)
            }
            ("POST", "/splash/stream") => {
                if !body.is_empty() {
                    self.push_cmd(CanvasCommand::SplashStreamAppend { code: body.clone() });
                } else {
                    self.push_cmd(CanvasCommand::SplashStreamBegin);
                }
                http_response(200, r#"{"ok":true}"#)
            }
            ("POST", "/splash/end") => {
                self.push_cmd(CanvasCommand::SplashStreamEnd);
                http_response(200, r#"{"ok":true}"#)
            }
            ("POST", "/eval") => {
                if !body.is_empty() {
                    self.push_cmd(CanvasCommand::SplashEval { code: body.clone() });
                }
                http_response(200, r#"{"ok":true}"#)
            }
            ("POST", "/clear") => {
                self.push_cmd(CanvasCommand::SplashRender { code: String::new() });
                http_response(200, r#"{"ok":true}"#)
            }
            ("GET", "/event") => {
                let event = self.wait_event().await;
                match event {
                    Some(e) => http_response(200, &e),
                    None => http_response(204, ""),
                }
            }
            ("POST", "/audio/play") => {
                if !body.is_empty() {
                    self.push_cmd(CanvasCommand::AudioPlay { url: body.clone() });
                }
                http_response(200, r#"{"ok":true}"#)
            }
            ("POST", "/audio/pause") => {
                self.push_cmd(CanvasCommand::AudioPause);
                http_response(200, r#"{"ok":true}"#)
            }
            ("POST", "/audio/stop") => {
                self.push_cmd(CanvasCommand::AudioStop);
                http_response(200, r#"{"ok":true}"#)
            }
            ("POST", "/save") => {
                // Body is the app name. If empty, auto-extract from current splash.
                let name = if body.is_empty() { String::new() } else { body.clone() };
                self.push_cmd(CanvasCommand::SaveApp { name });
                http_response(200, r#"{"ok":true}"#)
            }
            ("GET", "/ping") => {
                http_response(200, r#"{"ok":true}"#)
            }
            _ => {
                http_response(404, r#"{"error":"not found"}"#)
            }
        };

        // Use write_all for complete response delivery
        let mut stream = stream;
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.flush().await;
    }

    async fn wait_event(&self) -> Option<String> {
        // Check queue first
        if let Ok(mut q) = self.event_queue.lock() {
            if let Some(evt) = q.pop_front() {
                return Some(evt);
            }
        }
        // Wait up to 30s
        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.event_notify.notified(),
        ).await {
            Ok(()) => {
                if let Ok(mut q) = self.event_queue.lock() {
                    q.pop_front()
                } else {
                    None
                }
            }
            Err(_) => None, // timeout
        }
    }

    fn handle_message(&self, msg: &Value) {
        if let Some(code) = msg.get("splash").and_then(|v| v.as_str()) {
            self.push_cmd(CanvasCommand::SplashRender { code: code.to_string() });
        } else if let Some(action) = msg.get("splash_stream").and_then(|v| v.as_str()) {
            match action {
                "begin" => self.push_cmd(CanvasCommand::SplashStreamBegin),
                "append" => {
                    if let Some(code) = msg.get("code").and_then(|v| v.as_str()) {
                        self.push_cmd(CanvasCommand::SplashStreamAppend { code: code.to_string() });
                    }
                }
                "end" => self.push_cmd(CanvasCommand::SplashStreamEnd),
                _ => {}
            }
        } else if let Some(code) = msg.get("eval").and_then(|v| v.as_str()) {
            self.push_cmd(CanvasCommand::SplashEval { code: code.to_string() });
        } else if let Some(audio) = msg.get("audio") {
            if let Some(obj) = audio.as_object() {
                if let Some(url) = obj.get("play").and_then(|v| v.as_str()) {
                    self.push_cmd(CanvasCommand::AudioPlay { url: url.to_string() });
                }
            } else if let Some(action) = audio.as_str() {
                match action {
                    "pause" => self.push_cmd(CanvasCommand::AudioPause),
                    "stop" => self.push_cmd(CanvasCommand::AudioStop),
                    "toggle" => self.push_cmd(CanvasCommand::AudioToggle),
                    _ => {}
                }
            }
        } else if msg.get("clear").and_then(|v| v.as_bool()) == Some(true) {
            self.push_cmd(CanvasCommand::SplashRender { code: String::new() });
        }
    }

    /// Broadcast a message string to all connected WS clients.
    pub fn broadcast_message(&self, msg: &str) {
        if let Ok(mut senders) = self.event_senders.lock() {
            senders.retain(|(tx, _)| tx.send(msg.to_string()).is_ok());
        }
    }
}

fn parse_http_request(raw: &str) -> (String, String, String) {
    let mut lines = raw.split("\r\n");
    let first = lines.next().unwrap_or("");
    let parts: Vec<&str> = first.split_whitespace().collect();
    let method = parts.first().unwrap_or(&"GET").to_string();
    let path = parts.get(1).unwrap_or(&"/").to_string();

    // Find body after empty line
    let body = if let Some(pos) = raw.find("\r\n\r\n") {
        raw[pos + 4..].to_string()
    } else if let Some(pos) = raw.find("\n\n") {
        raw[pos + 2..].to_string()
    } else {
        String::new()
    };

    (method, path, body)
}

fn http_response(status: u16, body: &str) -> String {
    let status_text = match status {
        200 => "OK",
        204 => "No Content",
        404 => "Not Found",
        _ => "OK",
    };
    format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
        status, status_text, body.len(), body
    )
}
