//! Client side of the app's `--remote` HTTP surface (`platform/src/remote.rs`).
//!
//! One connection per request, hand-rolled HTTP/1.1 on loopback, JSON decoded
//! with `makepad-micro-serde`. Every `{"err":...}` answer becomes a
//! [`TestError`] so callers never see a fake success.

use crate::error::{TestError, TestResult};
use makepad_micro_serde::{DeJson, JsonValue};
use makepad_studio_protocol::{KeyCode, KeyModifiers, WidgetSnapshot};
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Longer than every internal `ask` timeout in the app's remote thread (grabs
/// wait up to 10 s for the GPU), so a wedged app still answers with its own
/// diagnostic instead of a bare socket timeout.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq)]
pub struct WindowInfo {
    pub id: usize,
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub dpi: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LogTail {
    pub last_seq: u64,
    pub lines: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Move,
    Down,
    Up,
    Click,
    Scroll,
}

impl MouseKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Down => "down",
            Self::Up => "up",
            Self::Click => "click",
            Self::Scroll => "scroll",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    Press,
    Down,
    Up,
}

impl KeyKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Press => "press",
            Self::Down => "down",
            Self::Up => "up",
        }
    }
}

#[derive(Clone, Debug)]
pub struct MouseInput {
    pub kind: MouseKind,
    pub window: Option<usize>,
    pub x: f64,
    pub y: f64,
    pub button: u32,
    pub dx: f64,
    pub dy: f64,
    pub modifiers: KeyModifiers,
    pub wait: bool,
}

impl MouseInput {
    pub fn at(kind: MouseKind, window: Option<usize>, x: f64, y: f64) -> Self {
        Self {
            kind,
            window,
            x,
            y,
            button: 0,
            dx: 0.0,
            dy: 0.0,
            modifiers: KeyModifiers::default(),
            wait: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RemoteClient {
    host: String,
    port: u16,
}

impl RemoteClient {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }

    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// `GET /` — the running binary's own protocol sheet.
    pub fn help(&self) -> TestResult<String> {
        self.get_text("/", &[])
    }

    pub fn windows(&self) -> TestResult<Vec<WindowInfo>> {
        let json = self.get_json("/s", &[])?;
        parse_windows(&json)
    }

    /// Every widget, including invisible ones; callers filter.
    pub fn snapshot(&self) -> TestResult<Vec<WidgetSnapshot>> {
        let json = self.get_json("/snap", &[("all", "1")])?;
        parse_snapshot(&json)
    }

    pub fn dump(&self) -> TestResult<String> {
        self.get_text("/d", &[])
    }

    /// Grab one window to a PNG on disk and return its absolute path.
    pub fn grab(&self, window: Option<usize>, scale: f64) -> TestResult<PathBuf> {
        let window = window.map(|w| w.to_string());
        let scale = format_num(scale);
        let mut params: Vec<(&str, &str)> = vec![("scale", scale.as_str())];
        if let Some(window) = &window {
            params.push(("w", window.as_str()));
        }
        let json = self.get_json("/g", &params)?;
        let path = json
            .key("png")
            .and_then(JsonValue::string)
            .ok_or_else(|| TestError::new(format!("grab reply carries no png path: {json:?}")))?;
        Ok(PathBuf::from(path))
    }

    /// Log lines after `since` (sequence numbers start at 1; `0` is everything
    /// the ring still holds).
    pub fn log_since(&self, since: u64) -> TestResult<LogTail> {
        let since = since.to_string();
        let json = self.get_json("/log", &[("since", since.as_str())])?;
        parse_log(&json)
    }

    pub fn mouse(&self, input: &MouseInput) -> TestResult<()> {
        let route = if input.kind == MouseKind::Click {
            "/click"
        } else {
            "/m"
        };
        let (x, y, dx, dy, button) = (
            format_num(input.x),
            format_num(input.y),
            format_num(input.dx),
            format_num(input.dy),
            input.button.to_string(),
        );
        let window = input.window.map(|w| w.to_string());
        let mut params: Vec<(&str, &str)> = vec![
            ("k", input.kind.as_str()),
            ("x", x.as_str()),
            ("y", y.as_str()),
            ("b", button.as_str()),
        ];
        if input.kind == MouseKind::Scroll {
            params.push(("dx", dx.as_str()));
            params.push(("dy", dy.as_str()));
        }
        push_modifier_params(&mut params, &input.modifiers);
        if let Some(window) = &window {
            params.push(("w", window.as_str()));
        }
        if input.wait {
            params.push(("wait", "1"));
        }
        self.get_json(route, &params).map(|_| ())
    }

    pub fn key(
        &self,
        window: Option<usize>,
        kind: KeyKind,
        code: KeyCode,
        modifiers: KeyModifiers,
        wait: bool,
    ) -> TestResult<()> {
        let code = key_code_name(code);
        let window = window.map(|w| w.to_string());
        let mut params: Vec<(&str, &str)> = vec![("k", kind.as_str()), ("c", code.as_str())];
        push_modifier_params(&mut params, &modifiers);
        if let Some(window) = &window {
            params.push(("w", window.as_str()));
        }
        if wait {
            params.push(("wait", "1"));
        }
        self.get_json("/k", &params).map(|_| ())
    }

    pub fn text(&self, window: Option<usize>, text: &str, wait: bool) -> TestResult<()> {
        let window = window.map(|w| w.to_string());
        let mut params: Vec<(&str, &str)> = vec![("t", text)];
        if let Some(window) = &window {
            params.push(("w", window.as_str()));
        }
        if wait {
            params.push(("wait", "1"));
        }
        self.get_json("/t", &params).map(|_| ())
    }

    /// Graceful shutdown; the caller confirms the process actually exits.
    pub fn grab_and_quit(&self) -> TestResult<()> {
        self.get_json("/gq", &[]).map(|_| ())
    }

    /// Graceful shutdown without a grab, for backends without capture support.
    pub fn quit(&self) -> TestResult<()> {
        self.get_json("/quit", &[]).map(|_| ())
    }

    pub fn get_json(&self, path: &str, params: &[(&str, &str)]) -> TestResult<JsonValue> {
        let body = self.get_text(path, params)?;
        parse_json(&body)
    }

    /// Raw GET. A non-200 status or an `{"err":..}` body is an error.
    pub fn get_text(&self, path: &str, params: &[(&str, &str)]) -> TestResult<String> {
        let target = build_target(path, params);
        let response = self.request(&target)?;
        let err = response_error(&response.body);
        if response.status != 200 {
            return Err(TestError::new(format!(
                "{} {target}: {}",
                response.status,
                err.unwrap_or_else(|| response.body.trim().to_string())
            )));
        }
        if let Some(err) = err {
            return Err(TestError::new(format!("{target}: {err}")));
        }
        Ok(response.body)
    }

    fn request(&self, target: &str) -> TestResult<HttpResponse> {
        let addr = self.socket_addr()?;
        let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).map_err(|err| {
            TestError::new(format!("connect to app remote {addr} failed: {err}"))
        })?;
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(RESPONSE_TIMEOUT));
        let _ = stream.set_write_timeout(Some(CONNECT_TIMEOUT));
        let head = format!(
            "GET {target} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            self.endpoint()
        );
        stream
            .write_all(head.as_bytes())
            .map_err(|err| TestError::new(format!("write to app remote {addr} failed: {err}")))?;
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .map_err(|err| TestError::new(format!("read from app remote {addr} failed: {err}")))?;
        parse_response(&raw)
    }

    fn socket_addr(&self) -> TestResult<SocketAddr> {
        (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|err| TestError::new(format!("resolve {}: {err}", self.endpoint())))?
            .next()
            .ok_or_else(|| TestError::new(format!("resolve {}: no address", self.endpoint())))
    }
}

pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub fn parse_response(raw: &[u8]) -> TestResult<HttpResponse> {
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| TestError::new("malformed HTTP reply from app remote (no header end)"))?;
    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| TestError::new(format!("malformed HTTP status line: {status_line:?}")))?;
    let mut content_length = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let mut body = raw[head_end + 4..].to_vec();
    if let Some(len) = content_length {
        body.truncate(len);
    }
    Ok(HttpResponse {
        status,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

/// The `err` field of a JSON object body, if the body is one.
fn response_error(body: &str) -> Option<String> {
    let trimmed = body.trim_start();
    if !trimmed.starts_with('{') || !trimmed.contains("\"err\"") {
        return None;
    }
    let json = JsonValue::deserialize_json(trimmed).ok()?;
    json.key("err").and_then(JsonValue::string).cloned()
}

pub fn parse_json(body: &str) -> TestResult<JsonValue> {
    JsonValue::deserialize_json(body.trim())
        .map_err(|err| TestError::new(format!("bad JSON from app remote: {err:?}\n{body}")))
}

fn json_f64(value: Option<&JsonValue>) -> Option<f64> {
    match value? {
        JsonValue::U64(v) => Some(*v as f64),
        JsonValue::I64(v) => Some(*v as f64),
        JsonValue::F64(v) => Some(*v),
        JsonValue::U128(v) => Some(*v as f64),
        JsonValue::I128(v) => Some(*v as f64),
        _ => None,
    }
}

fn json_str(value: Option<&JsonValue>) -> Option<String> {
    value.and_then(JsonValue::string).cloned()
}

fn json_bool(value: Option<&JsonValue>) -> Option<bool> {
    match value? {
        JsonValue::Bool(value) => Some(*value),
        _ => None,
    }
}

fn json_array(value: Option<&JsonValue>) -> Option<&Vec<JsonValue>> {
    match value? {
        JsonValue::Array(items) => Some(items),
        _ => None,
    }
}

pub fn parse_windows(json: &JsonValue) -> TestResult<Vec<WindowInfo>> {
    let items = json_array(json.key("w"))
        .ok_or_else(|| TestError::new(format!("status reply carries no window list: {json:?}")))?;
    let mut windows = Vec::with_capacity(items.len());
    for item in items {
        let id = json_f64(item.key("i")).map(|v| v as usize);
        let size = json_array(item.key("sz"));
        let (Some(id), Some(size)) = (id, size) else {
            return Err(TestError::new(format!("malformed window entry: {item:?}")));
        };
        windows.push(WindowInfo {
            id,
            title: json_str(item.key("t")).unwrap_or_default(),
            width: json_f64(size.first()).unwrap_or(0.0),
            height: json_f64(size.get(1)).unwrap_or(0.0),
            dpi: json_f64(item.key("dpi")).unwrap_or(1.0),
        });
    }
    Ok(windows)
}

/// `/snap` rows carry window-local layout points and the widget's actual
/// window identity and state. Missing required state is a protocol error.
pub fn parse_snapshot(json: &JsonValue) -> TestResult<Vec<WidgetSnapshot>> {
    let items = json_array(json.key("s"))
        .ok_or_else(|| TestError::new(format!("snap reply carries no widget list: {json:?}")))?;
    let mut widgets = Vec::with_capacity(items.len());
    for item in items {
        let id = json_str(item.key("i"));
        let widget_type = json_str(item.key("ty"));
        let rect = json_array(item.key("r"));
        let window_id = json_str(item.key("window_id"));
        let enabled = json_bool(item.key("enabled"));
        let (Some(id), Some(widget_type), Some(rect), Some(window_id), Some(enabled)) =
            (id, widget_type, rect, window_id, enabled)
        else {
            return Err(TestError::new(format!("malformed snapshot entry: {item:?}")));
        };
        if rect.len() != 4 {
            return Err(TestError::new(format!("malformed snapshot rect: {item:?}")));
        }
        let coord = |index: usize| json_f64(rect.get(index)).unwrap_or(0.0).round() as i64;
        widgets.push(WidgetSnapshot {
            id,
            widget_type,
            window_id,
            window_index: json_f64(item.key("w")).unwrap_or(0.0) as usize,
            visible: json_f64(item.key("v")).map_or(true, |v| v != 0.0),
            enabled,
            x: coord(0),
            y: coord(1),
            width: coord(2),
            height: coord(3),
            text: json_str(item.key("t")),
            value: json_str(item.key("val")),
            checked: json_f64(item.key("c")).map(|c| c != 0.0),
            selected: json_str(item.key("selected")),
        });
    }
    Ok(widgets)
}

pub fn parse_log(json: &JsonValue) -> TestResult<LogTail> {
    let lines = json_array(json.key("l"))
        .ok_or_else(|| TestError::new(format!("log reply carries no lines: {json:?}")))?;
    Ok(LogTail {
        last_seq: json_f64(json.key("n")).unwrap_or(0.0) as u64,
        lines: lines
            .iter()
            .filter_map(|line| line.string().cloned())
            .collect(),
    })
}

/// The remote's key-code parser lowercases the variant name, so the `Debug`
/// spelling is the wire spelling (`KeyA`, `ReturnKey`, `ArrowLeft`, ...).
pub fn key_code_name(code: KeyCode) -> String {
    format!("{code:?}")
}

fn push_modifier_params<'a>(params: &mut Vec<(&'a str, &'a str)>, modifiers: &KeyModifiers) {
    if modifiers.shift {
        params.push(("shift", "1"));
    }
    if modifiers.control {
        params.push(("ctrl", "1"));
    }
    if modifiers.alt {
        params.push(("alt", "1"));
    }
    if modifiers.logo {
        params.push(("cmd", "1"));
    }
}

pub fn build_target(path: &str, params: &[(&str, &str)]) -> String {
    let mut target = String::from(path);
    for (index, (key, value)) in params.iter().enumerate() {
        target.push(if index == 0 { '?' } else { '&' });
        target.push_str(&percent_encode(key));
        target.push('=');
        target.push_str(&percent_encode(value));
    }
    target
}

pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => {
                let _ = write!(&mut out, "%{byte:02X}");
            }
        }
    }
    out
}

fn format_num(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1.0e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_percent_encoded() {
        assert_eq!(
            build_target("/t", &[("t", "hello world & \"x\""), ("wait", "1")]),
            "/t?t=hello%20world%20%26%20%22x%22&wait=1"
        );
        assert_eq!(build_target("/d", &[]), "/d");
    }

    #[test]
    fn key_names_match_the_remote_spelling() {
        assert_eq!(key_code_name(KeyCode::KeyA), "KeyA");
        assert_eq!(key_code_name(KeyCode::ReturnKey), "ReturnKey");
        assert_eq!(key_code_name(KeyCode::ArrowLeft), "ArrowLeft");
        assert_eq!(key_code_name(KeyCode::Backspace), "Backspace");
    }

    #[test]
    fn snapshot_rows_become_widget_snapshots_with_window_names() {
        let json = parse_json(
            r#"{"s":[{"i":"window_widget","ty":"Window","r":[0,0,800,600],"w":0,"window_id":"main_window","enabled":true},
                    {"i":"counter_label","ty":"Label","r":[10.5,20,100,30],"w":0,"window_id":"main_window","enabled":true,"t":"Count: 1"},
                    {"i":"input","ty":"TextInput","r":[0,0,50,20],"w":0,"window_id":"main_window","enabled":false,"val":""},
                    {"i":"hidden","ty":"View","r":[0,0,0,0],"w":0,"window_id":"main_window","enabled":true,"v":0},
                    {"i":"toggle","ty":"CheckBox","r":[1,2,3,4],"w":1,"window_id":"panel_window","enabled":true,"c":1,"selected":"second"}]}"#,
        )
        .unwrap();
        let widgets = parse_snapshot(&json).unwrap();
        assert_eq!(widgets.len(), 5);
        let label = &widgets[1];
        assert_eq!(label.window_id, "main_window");
        assert_eq!((label.x, label.y, label.width, label.height), (11, 20, 100, 30));
        assert_eq!(label.text.as_deref(), Some("Count: 1"));
        assert!(label.visible);
        assert_eq!(widgets[2].value.as_deref(), Some(""));
        assert!(!widgets[2].enabled);
        assert!(!widgets[3].visible);
        assert_eq!(widgets[4].checked, Some(true));
        assert_eq!(widgets[4].window_index, 1);
        assert_eq!(widgets[4].window_id, "panel_window");
        assert_eq!(widgets[4].selected.as_deref(), Some("second"));
    }

    #[test]
    fn snapshots_reject_missing_or_fabricated_required_state() {
        for row in [
            r#"{"i":"button","ty":"Button","r":[0,0,10,10],"w":0,"window_id":"main"}"#,
            r#"{"i":"button","ty":"Button","r":[0,0,10,10],"w":0,"enabled":true}"#,
            r#"{"i":"button","ty":"Button","r":[0,0,10,10],"w":0,"window_id":"main","enabled":1}"#,
        ] {
            let json = parse_json(&format!("{{\"s\":[{row}]}}")).unwrap();
            assert!(parse_snapshot(&json).is_err(), "accepted {row}");
        }
    }

    #[test]
    fn windows_and_logs_parse() {
        let json = parse_json(
            r#"{"app":"x","pid":1,"w":[{"i":0,"t":"Counter [remote]","sz":[800,600],"px":[1600,1200],"dpi":2,"pos":[0,0]}]}"#,
        )
        .unwrap();
        let windows = parse_windows(&json).unwrap();
        assert_eq!(
            windows,
            vec![WindowInfo {
                id: 0,
                title: "Counter [remote]".to_string(),
                width: 800.0,
                height: 600.0,
                dpi: 2.0
            }]
        );
        let json = parse_json(r#"{"n":7,"pool":"","l":["a","b"]}"#).unwrap();
        let tail = parse_log(&json).unwrap();
        assert_eq!(tail.last_seq, 7);
        assert_eq!(tail.lines, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn http_replies_split_status_and_body() {
        let raw = b"HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: 14\r\n\r\n{\"err\":\"nope\"}extra";
        let response = parse_response(raw).unwrap();
        assert_eq!(response.status, 404);
        assert_eq!(response.body, "{\"err\":\"nope\"}");
        assert_eq!(response_error(&response.body).as_deref(), Some("nope"));
        assert!(response_error("{\"ok\":1}").is_none());
        assert!(parse_response(b"garbage").is_err());
    }

    #[test]
    fn client_drives_the_remote_routes_over_loopback() {
        let server = crate::fixture::FixtureServer::start();
        server.route("/", 200, "makepad-remote  app=fake pid=1\n/ this sheet");
        server.route(
            "/s",
            200,
            r#"{"app":"fake","pid":1,"w":[{"i":0,"t":"Fake [remote]","sz":[640,480],"px":[1280,960],"dpi":2,"pos":[0,0]}]}"#,
        );
        server.route(
            "/snap",
            200,
            r#"{"s":[{"i":"main_window","ty":"Window","r":[0,0,640,480],"w":0,"window_id":"main_window","enabled":true},{"i":"go","ty":"Button","r":[10,10,80,24],"w":0,"window_id":"main_window","enabled":false,"t":"Go","selected":"item"}]}"#,
        );
        server.route("/click", 200, r#"{"ok":1,"f":12}"#);
        server.route("/k", 200, r#"{"ok":1,"f":13}"#);
        server.route("/t", 200, r#"{"ok":1,"f":14}"#);
        server.route("/m", 200, r#"{"ok":1}"#);
        server.route("/d", 200, "root View 0 0 640 480\n  go Button 10 10 80 24\n");
        server.route("/g", 200, r#"{"png":"/tmp/grab-w0-00001.png","w":0,"sz":[320,240]}"#);
        server.route("/log", 200, r#"{"n":3,"pool":"","l":["a","Count: 1"]}"#);
        server.route("/quit", 200, r#"{"ok":1}"#);
        server.route("/gq", 200, r#"{"quit":1,"png":[]}"#);

        let client = RemoteClient::new("127.0.0.1", server.port());
        assert!(client.help().unwrap().contains("this sheet"));
        let windows = client.windows().unwrap();
        assert_eq!(windows[0].dpi, 2.0);
        let widgets = client.snapshot().unwrap();
        assert_eq!(widgets[1].window_id, "main_window");
        assert_eq!(widgets[1].text.as_deref(), Some("Go"));
        assert!(!widgets[1].enabled);
        assert_eq!(widgets[1].selected.as_deref(), Some("item"));

        client
            .mouse(&MouseInput::at(MouseKind::Click, Some(0), 50.0, 22.0))
            .unwrap();
        client
            .key(
                Some(0),
                KeyKind::Press,
                KeyCode::KeyA,
                KeyModifiers {
                    logo: true,
                    ..Default::default()
                },
                true,
            )
            .unwrap();
        client.text(None, "hello world", true).unwrap();
        client
            .mouse(&MouseInput {
                dx: 0.0,
                dy: -3.5,
                wait: false,
                ..MouseInput::at(MouseKind::Scroll, Some(1), 5.0, 6.0)
            })
            .unwrap();
        assert!(client.dump().unwrap().contains("go Button"));
        assert_eq!(
            client.grab(Some(0), 0.5).unwrap(),
            PathBuf::from("/tmp/grab-w0-00001.png")
        );
        let tail = client.log_since(0).unwrap();
        assert_eq!(tail.last_seq, 3);
        assert!(tail.lines.iter().any(|line| line == "Count: 1"));
        client.grab_and_quit().unwrap();
        client.quit().unwrap();

        let requests = server.requests();
        assert_eq!(requests[0], "/");
        assert_eq!(requests[1], "/s");
        assert_eq!(requests[2], "/snap?all=1");
        assert_eq!(requests[3], "/click?k=click&x=50&y=22&b=0&w=0&wait=1");
        assert_eq!(requests[4], "/k?k=press&c=KeyA&cmd=1&w=0&wait=1");
        assert_eq!(requests[5], "/t?t=hello%20world&wait=1");
        assert_eq!(requests[6], "/m?k=scroll&x=5&y=6&b=0&dx=0&dy=-3.5&w=1");
        assert_eq!(requests[7], "/d");
        assert_eq!(requests[8], "/g?scale=0.5&w=0");
        assert_eq!(requests[9], "/log?since=0");
        assert_eq!(requests[10], "/gq");
        assert_eq!(requests[11], "/quit");
    }

    #[test]
    fn remote_errors_surface_as_test_errors() {
        let server = crate::fixture::FixtureServer::start();
        server.route("/s", 404, r#"{"err":"window 3 closed by user"}"#);
        server.route("/snap", 200, r#"{"err":"timeout (app busy or not running its event loop)"}"#);
        server.route("/d", 200, "not json");
        let client = RemoteClient::new("127.0.0.1", server.port());

        let err = client.windows().unwrap_err();
        assert!(err.message().contains("404"), "{}", err.message());
        assert!(err.message().contains("closed by user"), "{}", err.message());
        let err = client.snapshot().unwrap_err();
        assert!(err.message().contains("app busy"), "{}", err.message());
        assert_eq!(client.dump().unwrap(), "not json");
        let err = client.get_json("/d", &[]).unwrap_err();
        assert!(err.message().contains("bad JSON"), "{}", err.message());
        let err = client.grab(None, 1.0).unwrap_err();
        assert!(err.message().contains("no route"), "{}", err.message());

        drop(server);
        let err = client.help().unwrap_err();
        assert!(err.message().contains("connect"), "{}", err.message());
    }
}
