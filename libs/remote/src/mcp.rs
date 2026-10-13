//! `makepad-remote mcp <target>`: an app's tools as an MCP server on stdio, generated from its
//! `app` service. One implementation for every app: no listening socket, no token, no discovery
//! file of its own. Newline-delimited JSON-RPC in, one line out per request (anything else goes
//! to stderr). `initialize`, `ping` and notifications are answered here; `tools/list` is the
//! target's manifests (its AI services and the built-in ui/edit/logs/inspect, unless
//! `--no-builtins`) as `<service>__<tool>`; `tools/call` is one `app` call. The client-side
//! commands (`ui__grab`, `ui__gseq`, `edit__tweak_grab`) run here and return the PNG as an
//! image. Approvals are the MCP client's: it sees each tool's risk in its description.
//!
//! `--caller-env NAME` passes the capability in that environment variable with every call (an
//! agent lane's `MAKEPAD_SCOPE_MCP_TOKEN`); the app checks it.
//!
//! The target is a pid, an app name or `<host>:<app|pid>` (through the hub). With `--launch
//! <program> [args..]` an app that is not running is started (detached) when a request needs it.

use crate::conn::{Event, CLOSE_LINK_LOST, CLOSE_NORMAL};
use crate::link::{Client, Error};
use makepad_strict_json::{self as json, Value};
use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

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
    /// issued it (Scope, per agent lane) decides what the caller may do.
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
}

pub fn serve(opts: Options) -> i32 {
    let mut a = Adapter { opts, client: None };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = a.handle(&line) {
            if writeln!(out, "{reply}").and_then(|_| out.flush()).is_err() {
                break;
            }
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
                #[cfg(unix)]
                {
                    use std::os::unix::process::CommandExt;
                    cmd.process_group(0);
                }
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
        let lines = app_request(&client, "list")?;
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
        let lines = app_request(&client, &format!("call {service}.{tool}{caller}\n{args}"))?;
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
                content.push(json::obj(vec![("type", json::s("image")), ("data", json::s(image)), ("mimeType", json::s("image/png"))]));
            }
        }
        Ok(json::obj(vec![("content", Value::Arr(content)), ("isError", Value::Bool(outcome != "Ok"))]))
    }
}

/// A local endpoint by pid / app name, or `<host>:<app|pid>` through the hub.
fn connect(target: &str) -> Result<Client, String> {
    if target.contains(':') && !target.contains('/') {
        #[cfg(unix)]
        {
            let relax = std::env::var_os("MAKEPAD_RUN_BIN").map(std::path::PathBuf::from);
            let hub = crate::hub::connect(relax.as_deref())?;
            return Ok(Client::routed(&hub, target));
        }
        #[cfg(not(unix))]
        return Err("remote hosts are not available on this platform yet".into());
    }
    let c = Client::connect(target).map_err(|e| e.to_string())?;
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

/// One `app` request: its answer lines, parsed.
fn app_request(client: &Client, args: &str) -> Result<Vec<Value>, String> {
    let ch = client.open("app", args).map_err(|e| match e {
        Error::Refused(_, why) if why.contains("unsupported service") => "this process serves no `app` service".to_string(),
        e => e.to_string(),
    })?;
    let mut lines = Vec::new();
    loop {
        match ch.recv_timeout(CALL_WAIT) {
            Ok(Event::Data(d)) => {
                for l in String::from_utf8_lossy(&d).lines().filter(|l| !l.is_empty()) {
                    lines.push(json::parse_depth(l.as_bytes(), DEPTH).map_err(|e| format!("an answer line I cannot read ({e})"))?);
                }
            }
            Ok(Event::Closed(CLOSE_NORMAL, _)) => return Ok(lines),
            Ok(Event::Closed(CLOSE_LINK_LOST, why)) => return Err(format!("link lost: {why}")),
            Ok(Event::Closed(_, why)) => return Err(why),
            Ok(Event::Opened(_)) => {}
            Err(true) => return Err("no answer in time".into()),
            Err(false) => return Err("the channel closed".into()),
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
                content.push(json::obj(vec![("type", json::s("image")), ("data", json::s(base64(&bytes))), ("mimeType", json::s("image/png"))]));
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
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn the_handshake_and_errors_need_no_app() {
        let mut a = Adapter { opts: Options { target: "nobody-runs-this-app".into(), builtins: true, launch: Vec::new(), caller: None }, client: None };
        let init = a.handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#).unwrap();
        assert!(init.contains(r#""protocolVersion":"2025-03-26""#) && init.contains(r#""tools":{}"#), "{init}");
        assert!(a.handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        assert!(a.handle(r#"{"jsonrpc":"2.0","id":"p","method":"ping"}"#).unwrap().contains(r#""id":"p","result":{}"#));
        let list = a.handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        assert!(list.contains(r#""error""#) && list.contains("nobody-runs-this-app"), "{list}");
        assert!(a.handle(r#"{"jsonrpc":"2.0","id":3,"method":"bogus"}"#).unwrap().contains("-32601"));
    }

    #[test]
    fn tools_carry_schema_and_risk() {
        let t = Tool { service: "calculator".into(), name: "eval".into(), description: "Evaluate.".into(), parameters: r#"{"type":"object","properties":{"expression":{"type":"string"}}}"#.into(), risk: "Read".into() };
        let j = tool_json(&t).to_json();
        assert!(j.contains(r#""name":"calculator__eval""#) && j.contains("[reads only]") && j.contains(r#""inputSchema":{"type":"object""#), "{j}");
    }
}
