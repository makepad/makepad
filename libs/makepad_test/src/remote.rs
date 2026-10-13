//! Client side of the app's `--remote` endpoint (`platform/src/remote_core.rs`).
//!
//! Every call is one `cmd` request over the app's makepad-remote Unix socket;
//! the answer lines are JSON decoded with `makepad-micro-serde`. Every
//! `{"err":...}` line becomes a [`TestError`] so callers never see a fake
//! success. Images are written by this client (`makepad_remote::app`), never by
//! the app.

use crate::error::{TestError, TestResult};
use makepad_micro_serde::{DeJson, JsonValue};
use makepad_studio_protocol::{KeyCode, KeyModifiers, WidgetSnapshot};
use makepad_remote::conn::Event;
use makepad_remote::Client;
use std::path::{Path, PathBuf};
use std::time::Duration;

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

#[derive(Clone)]
pub struct RemoteClient {
    socket: PathBuf,
    client: Client,
}

impl std::fmt::Debug for RemoteClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteClient").field("socket", &self.socket).finish()
    }
}

impl RemoteClient {
    /// Dials the app's makepad-remote Unix socket (the path of its startup line).
    pub fn connect(socket: impl AsRef<Path>) -> TestResult<Self> {
        let socket = socket.as_ref().to_path_buf();
        let client = Client::connect(&socket.display().to_string()).map_err(|err| {
            TestError::new(format!(
                "connect to app remote {} failed: {err}",
                socket.display()
            ))
        })?;
        Ok(Self { socket, client })
    }

    /// The socket path this client dials, for diagnostics.
    pub fn socket(&self) -> String {
        self.socket.display().to_string()
    }

    /// `help` -- the running binary's own command sheet.
    pub fn help(&self) -> TestResult<String> {
        self.get_text("help", &[])
    }

    pub fn windows(&self) -> TestResult<Vec<WindowInfo>> {
        let json = self.get_json("status", &[])?;
        parse_windows(&json)
    }

    /// Every widget, including invisible ones; callers filter.
    pub fn snapshot(&self) -> TestResult<Vec<WidgetSnapshot>> {
        let json = self.get_json("snap", &[("all", "1")])?;
        parse_snapshot(&json)
    }

    pub fn dump(&self) -> TestResult<String> {
        self.get_text("dump", &[])
    }

    /// Grab one window to a PNG on disk (written by this client) and return
    /// its absolute path.
    pub fn grab(&self, window: Option<usize>, scale: f64) -> TestResult<PathBuf> {
        let window = window.map(|w| w.to_string());
        let scale = format_num(scale);
        let mut params: Vec<(&str, &str)> = vec![("scale", scale.as_str())];
        if let Some(window) = &window {
            params.push(("w", window.as_str()));
        }
        let lines = self.run_client_side("grab", &params)?;
        let json = parse_json(lines.first().map(String::as_str).unwrap_or_default())?;
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
        let json = self.get_json("log", &[("since", since.as_str())])?;
        parse_log(&json)
    }

    pub fn mouse(&self, input: &MouseInput) -> TestResult<()> {
        let command = if input.kind == MouseKind::Click {
            "click"
        } else {
            "mouse"
        };
        let (x, y, dx, dy, button) = (
            format_num(input.x),
            format_num(input.y),
            format_num(input.dx),
            format_num(input.dy),
            input.button.to_string(),
        );
        let window = input.window.map(|w| w.to_string());
        let mut params: Vec<(&str, &str)> = Vec::new();
        if input.kind != MouseKind::Click {
            params.push(("k", input.kind.as_str()));
        }
        params.push(("x", x.as_str()));
        params.push(("y", y.as_str()));
        params.push(("b", button.as_str()));
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
        self.get_json(command, &params).map(|_| ())
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
        self.get_json("key", &params).map(|_| ())
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
        self.get_json("text", &params).map(|_| ())
    }

    /// Graceful shutdown (`gq`: a frame of every window, written by this
    /// client, then `quit`); the caller confirms the process actually exits.
    pub fn grab_and_quit(&self) -> TestResult<()> {
        self.run_client_side("gq", &[]).map(|_| ())
    }

    /// Graceful shutdown without a grab, for backends without capture support.
    pub fn quit(&self) -> TestResult<()> {
        self.get_json("quit", &[]).map(|_| ())
    }

    /// One command; the first answer line decoded as JSON.
    pub fn get_json(&self, command: &str, params: &[(&str, &str)]) -> TestResult<JsonValue> {
        let lines = self.command_lines(command, params)?;
        parse_json(lines.first().map(String::as_str).unwrap_or_default())
    }

    /// One command; every answer line, newline-terminated. An `{"err":..}`
    /// line is an error.
    pub fn get_text(&self, command: &str, params: &[(&str, &str)]) -> TestResult<String> {
        let lines = self.command_lines(command, params)?;
        let mut text = String::new();
        for line in &lines {
            text.push_str(line);
            text.push('\n');
        }
        Ok(text)
    }

    /// The answer lines of one `cmd` request, with the response timeout. The
    /// answer must end with its `end` line; an `{"err":..}` line is an error.
    fn command_lines(&self, command: &str, params: &[(&str, &str)]) -> TestResult<Vec<String>> {
        let fail = |what: String| TestError::new(format!("app remote {command}: {what}"));
        let words = [command.to_string()];
        let kv: Vec<(String, String)> = params
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let line = makepad_remote::app::request_line(&words, &kv);
        let channel = self.client.open("cmd", "").map_err(|err| fail(err.to_string()))?;
        channel
            .send(line.into_bytes())
            .map_err(|err| fail(format!("link lost: {err}")))?;
        let mut pending = String::new();
        let mut lines = Vec::new();
        loop {
            match channel.recv_timeout(RESPONSE_TIMEOUT) {
                Ok(Event::Data(data)) => {
                    pending.push_str(&String::from_utf8_lossy(&data));
                    let mut ended = false;
                    while let Some(newline) = pending.find('\n') {
                        let answer: String = pending.drain(..=newline).collect();
                        let answer = answer.trim_end();
                        if answer.starts_with("{\"end\":") {
                            ended = true;
                            break;
                        }
                        if !answer.is_empty() {
                            lines.push(answer.to_string());
                        }
                    }
                    if ended {
                        break;
                    }
                }
                Ok(Event::Closed(code, why)) => {
                    return Err(fail(format!("connection closed without an answer ({code}: {why})")));
                }
                Ok(Event::Opened(_)) => {}
                Err(true) => {
                    return Err(fail(format!(
                        "no answer within {} s (app busy or not running its event loop)",
                        RESPONSE_TIMEOUT.as_secs()
                    )));
                }
                Err(false) => return Err(fail("link lost: channel closed".to_string())),
            }
        }
        check_answer(command, lines)
    }

    /// A command answered client side (`grab`, `gq`): `makepad_remote::app`
    /// opens the `frames` service and writes the PNG files. Returns the
    /// answer lines; an `{"err":..}` first line is an error.
    fn run_client_side(&self, command: &str, params: &[(&str, &str)]) -> TestResult<Vec<String>> {
        let argv: Vec<String> = std::iter::once(command.to_string())
            .chain(params.iter().map(|(k, v)| format!("{k}={v}")))
            .collect();
        let mut lines = Vec::new();
        makepad_remote::app::run(&self.client, &argv, &mut |line| {
            lines.push(line.to_string());
            true
        })
        .map_err(|err| TestError::new(format!("app remote {command}: {err}")))?;
        check_answer(command, lines)
    }
}

fn check_answer(command: &str, lines: Vec<String>) -> TestResult<Vec<String>> {
    for line in &lines {
        if let Some(err) = response_error(line) {
            return Err(TestError::new(format!("app remote {command}: {err}")));
        }
    }
    Ok(lines)
}

/// The `err` field of a JSON object line, if the line is an error answer.
fn response_error(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("{\"err\":") {
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
            path: json_str(item.key("p")),
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
        // Not `cmd`: a JSON request line already uses that key for the command.
        params.push(("logo", "1"));
    }
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
    fn error_lines_are_recognised_and_other_lines_are_not() {
        assert_eq!(
            response_error("{\"err\":\"nope\"}").as_deref(),
            Some("nope")
        );
        assert!(response_error("{\"ok\":1}").is_none());
        assert!(response_error("{\"s\":[{\"t\":\"{\\\"err\\\":1}\"}]}").is_none());
        assert!(response_error("root View 0 0 640 480").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn client_drives_the_remote_commands_over_the_socket() {
        let server = crate::fixture::FixtureServer::start();
        server.route("help", "makepad-remote  app=fake pid=1\nstatus  this sheet");
        server.route(
            "status",
            r#"{"app":"fake","pid":1,"w":[{"i":0,"t":"Fake [remote]","sz":[640,480],"px":[1280,960],"dpi":2,"pos":[0,0]}]}"#,
        );
        server.route(
            "snap",
            r#"{"s":[{"i":"main_window","ty":"Window","r":[0,0,640,480],"w":0,"window_id":"main_window","enabled":true},{"i":"go","ty":"Button","r":[10,10,80,24],"w":0,"window_id":"main_window","enabled":false,"t":"Go","selected":"item"}]}"#,
        );
        server.route("click", r#"{"ok":1,"f":12}"#);
        server.route("key", r#"{"ok":1,"f":13}"#);
        server.route("text", r#"{"ok":1,"f":14}"#);
        server.route("mouse", r#"{"ok":1}"#);
        server.route("dump", "root View 0 0 640 480\n  go Button 10 10 80 24");
        server.route("log", r#"{"n":3,"pool":"","l":["a","Count: 1"]}"#);
        server.route("quit", r#"{"ok":1}"#);

        let client = RemoteClient::connect(server.socket()).unwrap();
        assert_eq!(client.socket(), server.socket().display().to_string());
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
        assert_eq!(client.dump().unwrap(), "root View 0 0 640 480\n  go Button 10 10 80 24\n");

        // Images never come from the app as files: this client writes the PNG.
        let png = client.grab(Some(0), 0.5).unwrap();
        assert!(png.starts_with(std::env::temp_dir().join("makepad-remote")));
        assert_eq!(std::fs::read(&png).unwrap(), b"PNG0-0");
        assert!(png.file_name().unwrap().to_string_lossy().starts_with("grab-w0-"));

        let tail = client.log_since(0).unwrap();
        assert_eq!(tail.last_seq, 3);
        assert!(tail.lines.iter().any(|line| line == "Count: 1"));
        client.grab_and_quit().unwrap();
        client.quit().unwrap();

        let requests = server.requests();
        assert_eq!(requests[0], "help");
        assert_eq!(requests[1], "status");
        assert_eq!(requests[2], "snap all=1");
        assert_eq!(requests[3], "click x=50 y=22 b=0 w=0 wait=1");
        assert_eq!(requests[4], "key k=press c=KeyA logo=1 w=0 wait=1");
        assert_eq!(requests[5], "text t=hello world wait=1");
        assert_eq!(requests[6], "mouse k=scroll x=5 y=6 b=0 dx=0 dy=-3.5 w=1");
        assert_eq!(requests[7], "dump");
        assert_eq!(requests[8], "frames window=0 scale=0.5 codec=png once=1");
        assert_eq!(requests[9], "ping");
        assert_eq!(requests[10], "log since=0");
        // gq: the window list, a frame of each window (client side), then quit.
        assert_eq!(requests[11], "status");
        assert_eq!(requests[12], "frames window=0 scale=1 codec=png once=1");
        assert_eq!(requests[13], "ping");
        assert_eq!(requests[14], "quit");
        assert_eq!(requests[15], "quit");
        assert_eq!(requests.len(), 16);
        let _ = std::fs::remove_file(png);
    }

    #[cfg(unix)]
    #[test]
    fn remote_errors_surface_as_test_errors() {
        let server = crate::fixture::FixtureServer::start();
        server.route("status", r#"{"err":"window 3 closed by user"}"#);
        server.route("snap", r#"{"err":"timeout (app busy or not running its event loop)"}"#);
        server.route("dump", "not json");
        server.disconnect("log");
        let client = RemoteClient::connect(server.socket()).unwrap();

        let err = client.windows().unwrap_err();
        assert!(err.message().contains("closed by user"), "{}", err.message());
        let err = client.snapshot().unwrap_err();
        assert!(err.message().contains("app busy"), "{}", err.message());
        assert_eq!(client.dump().unwrap(), "not json\n");
        let err = client.get_json("dump", &[]).unwrap_err();
        assert!(err.message().contains("bad JSON"), "{}", err.message());
        let err = client.get_json("nope", &[]).unwrap_err();
        assert!(err.message().contains("unknown command"), "{}", err.message());
        // A command cut off without its end line is an error, never a result.
        let err = client.log_since(0).unwrap_err();
        assert!(err.message().contains("without an answer"), "{}", err.message());
        // The link survives a command that was cut off.
        assert_eq!(client.dump().unwrap(), "not json\n");

        drop(server);
        let err = client.help().unwrap_err();
        assert!(err.message().contains("link lost"), "{}", err.message());
        assert!(RemoteClient::connect("/nonexistent/makepad-remote.sock")
            .unwrap_err()
            .message()
            .contains("connect"));
    }
}
