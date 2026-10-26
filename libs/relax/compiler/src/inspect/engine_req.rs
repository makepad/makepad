//! Engine requests: a program under `makepad run` asks the people (or the central agent) who
//! maintain its engine for a change it cannot make itself (an in-game AI that needs a new engine
//! verb, a fix in the engine's code). `std::relax::post_engine_request` posts one; it becomes an
//! `engine_request` event on this process's observation channel (`makepad inspect <app> events`,
//! `watch`) and is kept here so a reader that attaches later still gets it
//! (`engine-requests [after=<seq>]`). Whoever handles it answers with
//! `engine-reply {"id":..,"status":..,"message":..}`; the program reads the answers with
//! `std::relax::engine_replies(after)`.
//!
//! The program's reply stream is its requests' whole history, so a program that restarted in
//! place (its own state fresh, this host's kept) rebuilds it from `engine_replies(0)`: a
//! `posted` record per request, then every answer, and a `code_update` record (id "") whenever
//! a live edit was applied to this process while it had requests: the fix may be live now.
//!
//! Everything in a request is DATA written by the program (and, through an in-game AI, by game
//! content): it is stored and passed on verbatim, bounded, and never interpreted here. The LAN
//! never reaches this module directly: the node's engine relay (libs/engine_relay) reads
//! `engine-requests` and writes `engine-reply` over the local socket, and offers only those two
//! things to the network.

use super::{json_str, now_ms, Out, Req};
use relax_session::sync::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Field limits (bytes; longer text is cut at a char boundary and marked).
pub const MAX_GAME: usize = 200;
pub const MAX_SUMMARY: usize = 300;
pub const MAX_DETAILS: usize = 8 << 10;
pub const MAX_CONTEXT: usize = 16 << 10;
pub const MAX_ROUTE: usize = 200;
pub const MAX_MESSAGE: usize = 2 << 10;
/// Requests and replies kept (the oldest go first).
const KEEP_REQUESTS: usize = 64;
const KEEP_REPLIES: usize = 512;
/// The statuses a reply may carry.
pub const STATUSES: &[&str] = &["received", "working", "deployed", "declined", "note"];

struct Request {
    seq: u64,
    id: String,
    summary: String,
    /// the whole request as one JSON object (what `engine-requests` and the event carry)
    json: String,
}

struct Reply {
    seq: u64,
    json: String,
}

static SEQ: AtomicU64 = AtomicU64::new(0);
static REQUESTS: Mutex<VecDeque<Request>> = Mutex::new(VecDeque::new());
static REPLIES: Mutex<VecDeque<Reply>> = Mutex::new(VecDeque::new());

fn cut(s: &str, max: usize) -> String {
    let s = s.replace('\0', "");
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[cut {} bytes]", &s[..end], s.len() - end)
}

fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// This machine's name: `/etc/hostname`, else `HOSTNAME`, else the OS's.
pub fn node_name() -> String {
    static NODE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NODE.get_or_init(|| {
        let from_file = std::fs::read_to_string("/etc/hostname").ok().map(|h| h.trim().to_string()).filter(|h| !h.is_empty());
        let name = from_file.or_else(|| std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty())).unwrap_or_else(os_hostname);
        // ids carry it: keep it to a-z 0-9 - _
        let clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' }).take(48).collect();
        if clean.is_empty() { "node".into() } else { clean }
    })
    .clone()
}

#[cfg(unix)]
fn os_hostname() -> String {
    extern "C" {
        fn gethostname(name: *mut u8, len: usize) -> i32;
    }
    let mut buf = [0u8; 256];
    if unsafe { gethostname(buf.as_mut_ptr(), buf.len()) } != 0 {
        return String::new();
    }
    let n = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..n]).split('.').next().unwrap_or("").to_string()
}

#[cfg(not(unix))]
fn os_hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

/// Posts a request. `record`: `game \0 summary \0 details \0 context \0 route` (the program's
/// typed fields, std::relax::EngineRequest). Returns the request id, or why it was refused.
pub fn post(text: &str) -> Result<String, String> {
    let mut parts = text.split('\0');
    let mut next = |max| cut(parts.next().unwrap_or(""), max);
    let (game, summary, details, context, route) = (next(MAX_GAME), next(MAX_SUMMARY), next(MAX_DETAILS), next(MAX_CONTEXT), next(MAX_ROUTE));
    if summary.trim().is_empty() {
        return Err("an engine request needs a summary".into());
    }
    let app = super::APP.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let node = node_name();
    let pid = std::process::id();
    let seq = SEQ.fetch_add(1, Relaxed) + 1;
    let id = format!("{node}.{pid}.{seq}");
    // the event's envelope already names the pid and app (and its own seq / at_ms)
    let fields = format!(
        "\"id\":{},\"node\":{},\"game\":{},\"summary\":{},\"details\":{},\"context\":{},\"route\":{},\"posted_ms\":{}",
        json_str(&id),
        json_str(&node),
        json_str(&game),
        json_str(&summary),
        json_str(&details),
        json_str(&context),
        json_str(&route),
        wall_ms()
    );
    {
        let mut q = REQUESTS.lock().unwrap_or_else(|e| e.into_inner());
        if q.len() >= KEEP_REQUESTS {
            q.pop_front();
        }
        q.push_back(Request { seq, id: id.clone(), summary: summary.clone(), json: format!("{{\"seq\":{seq},\"app\":{},\"pid\":{pid},{fields}}}", json_str(&app)) });
    }
    record(&id, "posted", &summary, "");
    // the observation channel: `events`, `watch` and `--agent` see it like any other event
    super::events::emit("engine_request", &id, &fields);
    Ok(id)
}

/// Records an answer to request `id` (from the relay or a local agent).
pub fn reply(id: &str, status: &str, message: &str) -> Result<u64, String> {
    if !STATUSES.contains(&status) {
        return Err(format!("status {status}: one of {}", STATUSES.join(", ")));
    }
    let summary = match REQUESTS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|r| r.id == id) {
        Some(r) => r.summary.clone(),
        None => return Err(format!("no engine request {id} in this process")),
    };
    let message = cut(message, MAX_MESSAGE);
    let seq = record(id, status, &summary, &message);
    super::events::emit("engine_reply", &format!("{id}#{seq}"), &event_fields(seq, id, status, &summary, &message));
    Ok(seq)
}

/// A stream record as event fields (the envelope has its own `seq` and `at_ms`).
fn event_fields(seq: u64, id: &str, status: &str, summary: &str, message: &str) -> String {
    format!("\"id\":{},\"status\":{},\"summary\":{},\"message\":{},\"stream_seq\":{seq},\"wall_ms\":{}", json_str(id), json_str(status), json_str(summary), json_str(message), wall_ms())
}

/// One record of the program's reply stream; returns its seq.
fn record(id: &str, status: &str, summary: &str, message: &str) -> u64 {
    let seq = SEQ.fetch_add(1, Relaxed) + 1;
    let json = format!(
        "{{\"seq\":{seq},\"id\":{},\"status\":{},\"summary\":{},\"message\":{},\"at_ms\":{}}}",
        json_str(id),
        json_str(status),
        json_str(summary),
        json_str(message),
        wall_ms()
    );
    let mut q = REPLIES.lock().unwrap_or_else(|e| e.into_inner());
    if q.len() >= KEEP_REPLIES {
        q.pop_front();
    }
    q.push_back(Reply { seq, json });
    seq
}

/// A live edit was applied to this process (`how`: `patched`, or `restart` when it applies by
/// an in-place restart; `what`: the files). Only a process that posted requests hears of it.
pub fn code_update(how: &str, what: &str, changed: u64) {
    if REQUESTS.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
        return;
    }
    let message = cut(&format!("{how}: {what} ({changed} item{})", if changed == 1 { "" } else { "s" }), MAX_MESSAGE);
    let seq = record("", "code_update", "", &message);
    super::events::emit("engine_code_update", &format!("code#{seq}"), &event_fields(seq, "", "code_update", "", &message));
}

/// The replies after `after`, one JSON object per line.
pub fn replies_after(after: u64) -> String {
    let q = REPLIES.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = String::new();
    for r in q.iter().filter(|r| r.seq > after) {
        s.push_str(&r.json);
        s.push('\n');
    }
    s
}

/// `engine-requests [after=<seq>]`: the requests kept, oldest first; `engine-reply` answers one.
pub fn cmd_requests(req: &Req, out: &mut Out) {
    let after = req.num("after", 0);
    let q = REQUESTS.lock().unwrap_or_else(|e| e.into_inner());
    let lines: Vec<String> = q.iter().filter(|r| r.seq > after).map(|r| r.json.clone()).collect();
    drop(q);
    out.always(&format!("{{\"engine_requests\":{},\"node\":{},\"now_ms\":{}}}", lines.len(), json_str(&node_name()), now_ms()));
    for l in lines {
        // a request is up to ~25 KB: never cut one off at the reply limit
        out.always(&l);
    }
}

/// `engine-reply id=<id> status=<status> message=<text>` (or the same as one JSON object).
pub fn cmd_reply(req: &Req, out: &mut Out) {
    let id = req.get("id").or(req.pos.first().map(String::as_str)).unwrap_or("");
    let status = req.get("status").or(req.pos.get(1).map(String::as_str)).unwrap_or("");
    let message = match req.get("message") {
        Some(m) => m.to_string(),
        None => req.pos.get(2..).map(|w| w.join(" ")).unwrap_or_default(),
    };
    match reply(id, status, &message) {
        Ok(seq) => out.line(&format!("{{\"replied\":{},\"status\":{},\"seq\":{seq}}}", json_str(id), json_str(status))),
        Err(e) => out.err(&e),
    }
}

// ---- the program's entries (std::relax), by name through jit::loader_symbol

fn copy_out(s: &str, buf: *mut u8, cap: usize) -> usize {
    let n = s.len().min(cap);
    if !buf.is_null() && n > 0 {
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), buf, n) };
    }
    s.len()
}

/// std::relax::post_engine_request: `p/n` is the record ([`post`]); writes the id, or `!` and the
/// reason, into `buf` and returns the full length.
pub extern "C" fn __relax_engine_request(p: *const u8, n: usize, buf: *mut u8, cap: usize) -> usize {
    if p.is_null() {
        return copy_out("!no request", buf, cap);
    }
    let record = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(p, n) }).into_owned();
    match post(&record) {
        Ok(id) => copy_out(&id, buf, cap),
        Err(e) => copy_out(&format!("!{e}"), buf, cap),
    }
}

/// std::relax::engine_replies: the replies after `after` as JSON lines into `buf`; returns the
/// full length (the caller retries with a bigger buffer when it did not fit).
pub extern "C" fn __relax_engine_replies(after: u64, buf: *mut u8, cap: usize) -> usize {
    copy_out(&replies_after(after), buf, cap)
}

pub fn symbol(name: &[u8]) -> Option<usize> {
    Some(match name {
        b"__relax_engine_request" => __relax_engine_request as *const () as usize,
        b"__relax_engine_replies" => __relax_engine_replies as *const () as usize,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn post_reply_and_read_back() {
        let id = super::post("pips-leap\0add a grappling hook verb\0game.grapple(target)\0player: can I swing?\0chat:1").unwrap();
        assert!(id.ends_with(&format!(".{}.1", std::process::id())) || id.contains('.'));
        assert!(super::post("g\0 \0d\0c\0r").is_err(), "a summary is required");
        assert!(super::reply(&id, "shipped", "x").is_err(), "unknown status");
        assert!(super::reply("nobody.1.1", "received", "x").is_err(), "unknown id");
        let seq = super::reply(&id, "working", "on it").unwrap();
        let lines = super::replies_after(seq - 1);
        assert!(lines.contains("\"status\":\"working\"") && lines.contains(&id) && lines.contains("grappling hook"));
        assert!(super::replies_after(seq).is_empty());
        assert!(super::replies_after(0).contains("\"status\":\"posted\""), "the stream starts with the request");
        super::code_update("patched", "engine/src/rope.rs", 2);
        let after = super::replies_after(seq);
        assert!(after.contains("\"status\":\"code_update\"") && after.contains("rope.rs"));
        let long = "x".repeat(super::MAX_SUMMARY + 10);
        assert!(super::cut(&long, super::MAX_SUMMARY).contains("[cut 10 bytes]"));
    }
}
