//! `makepad-remote mcp <target>`: an app's tools as an MCP server on stdio, generated from its
//! `app` service. One implementation for every app: no listening socket, no token, no discovery
//! file of its own. Newline-delimited JSON-RPC in, one line out per request (anything else goes
//! to stderr). `initialize`, `ping` and notifications are answered here; `tools/list` is the
//! target's manifests (its AI services and the built-in ui/edit/logs/inspect, unless
//! `--no-builtins`) as `<service>__<tool>`; `tools/call` is one `app` call. The client-side
//! commands (`ui__grab`, `ui__gseq`, `edit__tweak_grab`) run here and return the PNG as an
//! image. The client sees the risk in the description; the app confirms destructive tools on screen.
//!
//! `--caller-env NAME` passes the capability in that environment variable with every call (an
//! agent lane's `MAKEPAD_SCOPE_MCP_TOKEN`); the app checks it.
//!
//! The target is a pid, an app name, `<host>:<pid>` (through the hub) or an endpoint file an app
//! published in a directory it owns (`discovery::publish`): the instance of that home, found again
//! when it restarts. With `--launch <program> [args..]` an app that is not running is started
//! (detached) when a request needs it.

use crate::conn::{ChannelTx, Event, CLOSE_LINK_LOST, CLOSE_NORMAL};
use crate::link::{Client, Error};
use makepad_strict_json::{self as json, Value};
use std::io::{BufRead, Write};
use std::time::{Duration, Instant};
use std::sync::{Arc, Mutex, Weak, mpsc};

/// Protocol versions spoken, newest first.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// How long a launched app has to open its endpoint.
const LAUNCH_WAIT: Duration = Duration::from_secs(30);
/// A call may wait for a person to confirm it.
const CALL_WAIT: Duration = Duration::from_secs(15 * 60);
/// JSON nesting the adapter reads (tool arguments may nest).
const DEPTH: u32 = 32;
/// The built-in services every app has.
const BUILT_IN: [&str; 4] = ["ui", "edit", "logs", "inspect"];
/// Built-in tools run here: they read the `frames` service.
const CLIENT_SIDE: [(&str, &str); 4] = [("ui", "grab"), ("ui", "gseq"), ("ui", "rec"), ("edit", "tweak_grab")];

pub struct Options {
    pub target: String,
    pub builtins: bool,
    pub launch: Vec<String>,
    /// A capability passed with every call (`caller=` of the `app` request): the app that
    /// issued it (an app, per agent lane) decides what the caller may do.
    pub caller: Option<String>,
}

/// One tool as listed.
struct Tool {
    service: String,
    name: String,
    description: String,
    parameters: String,
    risk: String,
}

struct Adapter {
    opts: Options,
    client: Option<Client>,
    active: Option<Arc<Mutex<Cancellation>>>,
}

#[derive(Default)]
struct Cancellation {
    cancelled: bool,
    channel: Option<ChannelTx>,
}

impl Cancellation {
    fn cancel(token: &Mutex<Self>) {
        let channel = {
            let mut state = token.lock().unwrap();
            state.cancelled = true;
            state.channel.take()
        };
        if let Some(ch) = channel { ch.close(CLOSE_NORMAL, "the caller cancelled"); }
    }
}

type RequestLine = (String, Option<Arc<Mutex<Cancellation>>>);

// Only this reader touches stdin. A cancellation can close its own channel while the
// dispatcher is waiting. Ordinary requests keep their input order, including queued calls.
fn read_requests(input: impl BufRead, tx: mpsc::Sender<RequestLine>) {
    let mut calls: Vec<(Value, Weak<Mutex<Cancellation>>)> = Vec::new();
    for line in input.lines().map_while(Result::ok) {
        if line.trim().is_empty() { continue }
        calls.retain(|(_, token)| token.strong_count() != 0);
        let msg = json::parse_depth(line.as_bytes(), DEPTH).ok();
        let method = msg.as_ref().and_then(|m| m.get("method")).and_then(Value::as_str);
        let id = msg.as_ref().and_then(|m| m.get("id"));
        if method == Some("notifications/cancelled") && id.is_none() {
            let target = msg.as_ref().and_then(|m| m.get("params")?.get("requestId"));
            if let Some(token) = calls.iter().find(|(id, _)| Some(id) == target).and_then(|(_, t)| t.upgrade()) {
                Cancellation::cancel(&token);
            }
            continue
        }
        let token = id.filter(|_| method == Some("tools/call")).map(|id| {
            let token = Arc::new(Mutex::new(Cancellation::default()));
            calls.push((id.clone(), Arc::downgrade(&token)));
            token
        });
        if tx.send((line, token)).is_err() { break }
    }
    for (_, token) in calls {
        if let Some(token) = token.upgrade() { Cancellation::cancel(&token); }
    }
}

pub fn serve(opts: Options) -> i32 {
    let (tx, rx) = mpsc::channel();
    if let Err(err) = std::thread::Builder::new().name("mcp-stdin".into())
        .spawn(move || read_requests(std::io::stdin().lock(), tx)) {
        eprintln!("cannot read MCP requests: {err}");
        return 1;
    }
    let mut adapter = Adapter { opts, client: None, active: None };
    let mut out = std::io::stdout();
    for (line, token) in rx {
        adapter.active = token;
        let reply = adapter.handle(&line);
        adapter.active = None;
        if let Some(reply) = reply {
            if writeln!(out, "{reply}").and_then(|_| out.flush()).is_err() { break }
        }
    }
    0
}

fn rpc_result(id: &Value, result: Value) -> String {
    json::obj(vec![("jsonrpc", json::s("2.0")), ("id", id.clone()), ("result", result)]).to_json()
}

fn rpc_error(id: &Value, code: i64, message: &str) -> String {
    json::obj(vec![("jsonrpc", json::s("2.0")), ("id", id.clone()), ("error", json::obj(vec![("code", Value::Int(code)), ("message", json::s(message))]))]).to_json()
}

impl Adapter {
    /// One JSON-RPC message; `None` for notifications.
    fn handle(&mut self, line: &str) -> Option<String> {
        let msg = match json::parse_depth(line.as_bytes(), DEPTH) {
            Ok(m) => m,
            Err(e) => return Some(rpc_error(&Value::Null, -32700, &format!("parse error: {e}"))),
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("").to_string();
        let id = msg.get("id")?.clone();
        let params = msg.get("params").cloned().unwrap_or(Value::Obj(Vec::new()));
        Some(match method.as_str() {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
                let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(PROTOCOL_VERSIONS[0]);
                rpc_result(
                    &id,
                    json::obj(vec![
                        ("protocolVersion", json::s(version)),
                        ("capabilities", json::obj(vec![("tools", json::obj(vec![]))])),
                        ("serverInfo", json::obj(vec![("name", json::s(format!("makepad-remote {}", self.opts.target))), ("version", json::s(env!("CARGO_PKG_VERSION")))])),
                    ]),
                )
            }
            "ping" => rpc_result(&id, json::obj(vec![])),
            "tools/list" => match self.tools() {
                Ok(tools) => rpc_result(&id, json::obj(vec![("tools", Value::Arr(tools.iter().map(tool_json).collect()))])),
                Err(e) => rpc_error(&id, -32603, &e),
            },
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let args = params.get("arguments").map(Value::to_json).unwrap_or_else(|| "{}".into());
                match self.call(&name, &args) {
                    Ok(result) => rpc_result(&id, result),
                    Err(e) => rpc_error(&id, -32603, &e),
                }
            }
            other => rpc_error(&id, -32601, &format!("method not found: {other}")),
        })
    }

    /// The target's client, launching the app first when asked to and it is not running.
    fn client(&mut self) -> Result<Client, String> {
        if let Some(c) = &self.client {
            return Ok(c.clone());
        }
        let target = self.opts.target.clone();
        let c = match connect(&target) {
            Ok(c) => c,
            Err(e) if self.opts.launch.is_empty() => return Err(e),
            Err(_) => {
                let (program, args) = self.opts.launch.split_first().ok_or("empty --launch")?;
                let mut cmd = std::process::Command::new(program);
                cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
                crate::group::detach(&mut cmd);
                let child = cmd.spawn().map_err(|e| format!("cannot start {program}: {e}"))?;
                let pid = child.id().to_string();
                let end = Instant::now() + LAUNCH_WAIT;
                let c = loop {
                    if let Ok(c) = connect(&pid) {
                        break c;
                    }
                    if Instant::now() >= end {
                        return Err(format!("{program} (pid {pid}) did not open its endpoint within {} s", LAUNCH_WAIT.as_secs()));
                    }
                    std::thread::sleep(Duration::from_millis(200));
                };
                // the endpoint opens with the process; its window and AI ports come with startup
                while Instant::now() < end && !(has_window(&c) && answers(&c, "settled", "\"settled\":true")) {
                    std::thread::sleep(Duration::from_millis(200));
                }
                c
            }
        };
        self.client = Some(c.clone());
        Ok(c)
    }

    fn tools(&mut self) -> Result<Vec<Tool>, String> {
        let client = self.client()?;
        drop(client);
        let lines = self.app("list")?;
        let mut tools: Vec<Tool> = Vec::new();
        for l in lines {
            let Some(manifest) = l.get("Instance").and_then(|i| i.get("manifest")) else { continue };
            let service = manifest.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            if !self.opts.builtins && BUILT_IN.contains(&service.as_str()) {
                continue;
            }
            // one entry per service: several instances of one app share its tools
            if tools.iter().any(|t| t.service == service) {
                continue;
            }
            let Some(Value::Arr(list)) = manifest.get("tools") else { continue };
            for t in list {
                let risk = match t.get("risk") {
                    Some(Value::Obj(pairs)) => pairs.first().map(|(k, _)| k.clone()).unwrap_or_default(),
                    _ => String::new(),
                };
                tools.push(Tool {
                    service: service.clone(),
                    name: t.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    description: t.get("description").and_then(Value::as_str).unwrap_or("").to_string(),
                    parameters: t.get("parameters").and_then(Value::as_str).unwrap_or("{\"type\":\"object\"}").to_string(),
                    risk,
                });
            }
        }
        Ok(tools)
    }

    fn call(&mut self, name: &str, args: &str) -> Result<Value, String> {
        let (service, tool) = name.split_once("__").ok_or_else(|| format!("unknown tool `{name}`"))?;
        let client = self.client()?;
        if CLIENT_SIDE.contains(&(service, tool)) {
            return client_side(&client, &tool.replace('_', " "), args);
        }
        let caller = match &self.opts.caller {
            Some(c) => format!(" caller={c}"),
            None => String::new(),
        };
        drop(client);
        let lines = self.app(&format!("call {service}.{tool}{caller}\n{args}"))?;
        let result = lines.iter().find_map(|l| l.get("Result").and_then(|r| match r {
            Value::Arr(a) => a.first().cloned(),
            other => Some(other.clone()),
        }));
        let Some(r) = result else { return Err("the app ended the call without a result".into()) };
        let outcome = match r.get("outcome") {
            Some(Value::Obj(pairs)) => pairs.first().map(|(k, _)| k.clone()).unwrap_or_default(),
            _ => String::new(),
        };
        let mut text = r.get("text").and_then(Value::as_str).unwrap_or("").to_string();
        let data = r.get("data").and_then(Value::as_str).unwrap_or("");
        if !data.is_empty() {
            text.push_str(&format!("\n{data}"));
        }
        if outcome != "Ok" {
            text = format!("{}: {text}", outcome.to_lowercase());
        }
        let mut content = vec![json::obj(vec![("type", json::s("text")), ("text", json::s(text))])];
        if let Some(images) = r.get("images").and_then(Value::as_arr) {
            for image in images.iter().filter_map(Value::as_str) {
                content.push(json::obj(vec![("type", json::s("image")), ("data", json::s(image)), ("mimeType", json::s(picture_media_type(image)?))]));
            }
        }
        Ok(json::obj(vec![("content", Value::Arr(content)), ("isError", Value::Bool(outcome != "Ok"))]))
    }
}

/// A local endpoint by pid / app name, or `<host>:<app|pid>` through the hub.
fn connect(target: &str) -> Result<Client, String> {
    if target.contains(':') && !crate::discovery::is_endpoint_file(target) {
        #[cfg(unix)]
        {
            let relax = std::env::var_os("RELAX_BIN").map(std::path::PathBuf::from);
            let hub = crate::hub::connect(relax.as_deref())?;
            return Ok(Client::routed(&hub, target));
        }
        #[cfg(not(unix))]
        return Err("remote hosts are not available on this platform yet".into());
    }
    let c = Client::connect_as(target, &crate::discovery::Caller::set_up_for_app()).map_err(|e| e.to_string())?;
    c.set_timeout(Some(CALL_WAIT));
    Ok(c)
}

/// Whether the answer to `line` contains `needle`.
fn answers(client: &Client, line: &str, needle: &str) -> bool {
    client.cmd(line).map(|lines| lines.filter_map(Result::ok).any(|l| l.contains(needle))).unwrap_or(false)
}

/// Whether the app has a window yet (`status` lists one).
fn has_window(client: &Client) -> bool {
    client.cmd("status").map(|lines| lines.filter_map(Result::ok).any(|l| l.contains("\"w\":[{"))).unwrap_or(false)
}

/// Why an `app` request failed.
enum Failure {
    /// The channel never opened: the link is gone (the app quit or restarted), nothing ran.
    NotOpened(String),
    /// The link dropped while the request was out: it may have run.
    LinkLost(String),
    Other(String),
}

/// One `app` request: its answer lines, parsed.
fn app_request(client: &Client, args: &str, token: Option<&Mutex<Cancellation>>) -> Result<Vec<Value>, Failure> {
    if token.is_some_and(|t| t.lock().unwrap().cancelled) {
        return Err(Failure::Other("the caller cancelled".into()));
    }
    let ch = client.open("app", args).map_err(|e| match e {
        Error::Refused(_, why) if why.contains("unsupported service") => Failure::Other("this process serves no `app` service".to_string()),
        Error::Refused(_, why) => Failure::Other(why),
        e => Failure::NotOpened(e.to_string()),
    })?;
    if let Some(token) = token {
        let mut state = token.lock().unwrap();
        if state.cancelled {
            drop(state);
            ch.close(CLOSE_NORMAL, "the caller cancelled");
            return Err(Failure::Other("the caller cancelled".into()));
        }
        state.channel = Some(ch.tx());
    }
    let mut lines = Vec::new();
    loop {
        let event = ch.recv_timeout(CALL_WAIT);
        if token.is_some_and(|t| t.lock().unwrap().cancelled) {
            return Err(Failure::Other("the caller cancelled".into()));
        }
        match event {
            Ok(Event::Data(d)) => {
                for l in String::from_utf8_lossy(&d).lines().filter(|l| !l.is_empty()) {
                    let line = json::parse_depth(l.as_bytes(), DEPTH).map_err(|e| Failure::Other(format!("an answer line I cannot read ({e})")))?;
                    // Progress renews the silence wait. Only results and manifests are returned.
                    if line.get("Progress").is_none() { lines.push(line); }
                }
            }
            Ok(Event::Closed(CLOSE_NORMAL, _)) => return Ok(lines),
            Ok(Event::Closed(CLOSE_LINK_LOST, why)) => return Err(Failure::LinkLost(why)),
            Ok(Event::Closed(_, why)) => return Err(Failure::Other(why)),
            Ok(Event::Opened(_)) => {}
            Err(true) => return Err(Failure::Other("no answer in time".into())),
            Err(false) => return Err(Failure::LinkLost("the channel closed".into())),
        }
    }
}

impl Adapter {
    /// One `app` request on the target. A link that is gone is dropped and the target found
    /// again, once: an app name or an endpoint file then reaches the instance that runs now (an
    /// app restarted in place). A request cut off mid-way is not repeated, since it may have run;
    /// the next request reconnects.
    fn app(&mut self, args: &str) -> Result<Vec<Value>, String> {
        let client = self.client()?;
        let failure = match app_request(&client, args, self.active.as_deref()) {
            Ok(lines) => return Ok(lines),
            Err(f) => f,
        };
        match failure {
            Failure::Other(why) => Err(why),
            Failure::LinkLost(why) => {
                self.client = None;
                Err(format!("link lost: {why}"))
            }
            Failure::NotOpened(_) => {
                self.client = None;
                let client = self.client()?;
                app_request(&client, args, self.active.as_deref()).map_err(|f| match f {
                    Failure::NotOpened(why) | Failure::LinkLost(why) => {
                        self.client = None;
                        format!("link lost: {why}")
                    }
                    Failure::Other(why) => why,
                })
            }
        }
    }
}

/// A grab-type command: run as `makepad-remote app` runs it; the PNG comes back as an image.
fn client_side(client: &Client, words: &str, args: &str) -> Result<Value, String> {
    let mut argv: Vec<String> = words.split(' ').map(str::to_string).collect();
    if let Ok(Value::Obj(pairs)) = json::parse_depth(args.as_bytes(), DEPTH) {
        for (k, v) in pairs {
            let v = match v {
                Value::Str(s) => s,
                Value::Bool(b) => if b { "1".into() } else { "0".into() },
                other => other.to_json(),
            };
            argv.push(format!("{k}={v}"));
        }
    }
    let mut lines = Vec::new();
    let had_err = crate::app::run(client, &argv, &mut |l| {
        lines.push(l.to_string());
        true
    })
    .map_err(|e| e.to_string())?;
    let text = lines.join("\n");
    let mut content = vec![json::obj(vec![("type", json::s("text")), ("text", json::s(text.clone()))])];
    // one image per PNG path the answer names
    for l in &lines {
        let Ok(v) = json::parse_depth(l.as_bytes(), DEPTH) else { continue };
        let paths: Vec<String> = match v.get("png") {
            Some(Value::Str(p)) => vec![p.clone()],
            Some(Value::Arr(a)) => a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect(),
            _ => Vec::new(),
        };
        for p in paths.iter().take(4) {
            if let Ok(bytes) = std::fs::read(p) {
                let data = base64(&bytes);
                let media_type = picture_media_type(&data)?;
                content.push(json::obj(vec![("type", json::s("image")), ("data", json::s(data)), ("mimeType", json::s(media_type))]));
            }
        }
    }
    Ok(json::obj(vec![("content", Value::Arr(content)), ("isError", Value::Bool(had_err))]))
}

fn tool_json(t: &Tool) -> Value {
    let schema = json::parse_depth(t.parameters.as_bytes(), DEPTH).unwrap_or_else(|_| json::obj(vec![("type", json::s("object"))]));
    let risk = match t.risk.as_str() {
        "Read" => "reads only",
        "Act" => "changes the app's own state",
        "Request" => "makes something; confirm with the person",
        "Destructive" => "deletes, sends or reaches outside the app; confirm with the person",
        _ => "",
    };
    let description = if risk.is_empty() { t.description.clone() } else { format!("{} [{risk}]", t.description) };
    json::obj(vec![("name", json::s(format!("{}__{}", t.service, t.name))), ("description", json::s(description)), ("inputSchema", schema)])
}

// This adapter sits below the AI hub (platform depends on remote), so sniff
// the wire prefix here without linking a provider or decoding any pixels.
pub(crate) fn picture_media_type(data: &str) -> Result<&'static str, String> {
    let head = data.get(..12).ok_or("tool picture is too short")?;
    let bytes = makepad_base64::base64_decode(head.as_bytes())
        .map_err(|_| "tool picture is not base64")?;
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        Ok("image/jpeg")
    } else {
        Err("tool picture must be PNG or JPEG".into())
    }
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(picture_media_type(&base64(b"\x89PNG\r\n\x1a\n")), Ok("image/png"), "PNG tool results keep their MIME type");
        assert_eq!(picture_media_type(&base64(&[0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff])), Ok("image/jpeg"), "JPEG tool results use the JPEG MIME type");
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn the_handshake_and_errors_need_no_app() {
        let mut a = Adapter { opts: Options { target: "nobody-runs-this-app".into(), builtins: true, launch: Vec::new(), caller: None }, client: None, active: None };
        let init = a.handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#).unwrap();
        assert!(init.contains(r#""protocolVersion":"2025-03-26""#) && init.contains(r#""tools":{}"#), "{init}");
        assert!(a.handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        assert!(a.handle(r#"{"jsonrpc":"2.0","id":"p","method":"ping"}"#).unwrap().contains(r#""id":"p","result":{}"#));
        let list = a.handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        assert!(list.contains(r#""error""#) && list.contains("nobody-runs-this-app"), "{list}");
        assert!(a.handle(r#"{"jsonrpc":"2.0","id":3,"method":"bogus"}"#).unwrap().contains("-32601"));
    }

    #[cfg(unix)]
    #[test]
    fn mcp_cancels_the_held_channel_and_keeps_the_next_request_in_order() {
        use crate::{endpoint::Endpoint, link::Target};
        use std::{io::BufReader, os::unix::net::UnixStream, thread};

        let app = Endpoint::new("confirmation-test");
        let (opened, channel) = mpsc::channel();
        app.register("app", 1, Arc::new(move |_, open| {
            opened.send(open.channel).unwrap();
            Ok(())
        }));
        let key = crate::carrier::loopback::new_token().unwrap();
        let addr = crate::carrier::loopback::serve(&app, &key).unwrap().to_string();
        let client = Client::new(Target::Loopback(addr, key), "test");
        let (mut input, reader) = UnixStream::pair().unwrap();
        let (tx, rx) = mpsc::channel();
        let read = thread::spawn(move || read_requests(BufReader::new(reader), tx));
        writeln!(input, r#"{{"id":1,"method":"tools/call","params":{{"name":"example__change"}}}}"#).unwrap();
        let (line, token) = rx.recv().unwrap();
        let call = thread::spawn(move || {
            let mut adapter = Adapter { opts: Options { target: String::new(), builtins: false,
                launch: vec![], caller: None }, client: Some(client), active: token };
            let reply = adapter.handle(&line).unwrap();
            adapter.active = None;
            (adapter, reply)
        });
        let held = channel.recv().unwrap();
        held.send(b"{\"Progress\":{\"note\":\"waiting\"}}\n".to_vec()).unwrap();
        writeln!(input, r#"{{"id":"next","method":"ping"}}"#).unwrap();
        writeln!(input, r#"{{"method":"notifications/cancelled","params":{{"requestId":"other"}}}}"#).unwrap();
        writeln!(input, r#"{{"method":"notifications/cancelled","params":{{"requestId":1}}}}"#).unwrap();
        let end = held.recv();
        assert!(matches!(&end, Some(Event::Closed(_, why)) if why == "the caller cancelled"), "app saw {end:?}");
        let (mut adapter, reply) = call.join().unwrap();
        assert!(reply.contains("the caller cancelled"));
        let (next, _) = rx.recv().unwrap();
        assert!(adapter.handle(&next).unwrap().contains(r#""id":"next","result":{}"#));
        drop(input);
        read.join().unwrap();
    }

    #[test]
    fn tools_carry_schema_and_risk() {
        let t = Tool { service: "calculator".into(), name: "eval".into(), description: "Evaluate.".into(), parameters: r#"{"type":"object","properties":{"expression":{"type":"string"}}}"#.into(), risk: "Read".into() };
        let j = tool_json(&t).to_json();
        assert!(j.contains(r#""name":"calculator__eval""#) && j.contains("[reads only]") && j.contains(r#""inputSchema":{"type":"object""#), "{j}");
    }
}
