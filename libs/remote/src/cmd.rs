//! The `cmd` service: the inspect line protocol of `makepad inspect` and native apps. One request per
//! DATA message (a message may hold several lines); the answer is JSON lines sent as DATA, ending
//! with an `end` line. The host plugs in `dispatch`. Several cmd channels run at once, one thread
//! each. `Req`, `Out`, `answer` and the size limit are the inspect protocol's own, unchanged.

use crate::conn::{ChannelTx, Event, CLOSE_NORMAL};
use crate::endpoint::Factory;
use std::io::Write;
use std::sync::Arc;

static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

pub(crate) fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// ms since the first use of this crate in the process (the clock every reply uses)
pub fn now_ms() -> u64 {
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

/// Answers one request line in memory: the reply's JSON lines (no `end` line) and whether it was
/// cut at the size limit. For hosts that call a program's commands directly (`makepad inspect`).
pub fn answer(dispatch: fn(&Req, &mut Out), line: &str) -> (Vec<u8>, bool) {
    let mut buf: Vec<u8> = Vec::new();
    let truncated = answer_to(dispatch, line, &mut buf);
    (buf, truncated)
}

/// As [`answer`], writing the reply's lines to `w` as they are produced (a host streams them on).
/// Returns whether the reply was cut at the size limit.
pub fn answer_to(dispatch: fn(&Req, &mut Out), line: &str, w: &mut dyn Write) -> bool {
    let req = Req::parse(line);
    let mut out = Out { w, bytes: 0, limit: if req.get("full") == Some("1") { usize::MAX } else { LIMIT }, truncated: false, ok: true };
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dispatch(&req, &mut out)));
    if r.is_err() {
        out.err("internal error in this command (inspect bug)");
    }
    out.truncated
}

/// Host-provided fields for the `end` line of every answer: the function appends `,"key":value`
/// text (the platform reports the user sequence this way). Runs on the command's thread, right
/// after its dispatch.
pub type EndFields = Arc<std::sync::Mutex<Option<fn(&mut String)>>>;


/// A reply being written: JSON lines, bounded (a reply never grows past `LIMIT` bytes unless the
/// request said `full=1`).
pub struct Out<'a> {
    w: &'a mut dyn Write,
    bytes: usize,
    pub limit: usize,
    pub truncated: bool,
    ok: bool,
}

const LIMIT: usize = 16 << 10;

impl<'a> Out<'a> {
    pub fn line(&mut self, s: &str) {
        if self.bytes + s.len() > self.limit {
            self.truncated = true;
            return;
        }
        self.bytes += s.len() + 1;
        self.ok &= self.w.write_all(s.as_bytes()).is_ok() && self.w.write_all(b"\n").is_ok();
    }
    /// A line that is always written (headers, errors, the end line).
    pub fn always(&mut self, s: &str) {
        self.ok &= self.w.write_all(s.as_bytes()).is_ok() && self.w.write_all(b"\n").is_ok();
    }
    pub fn err(&mut self, msg: &str) {
        self.always(&format!("{{\"err\":{}}}", json_str(msg)));
    }
    pub fn flush(&mut self) -> bool {
        self.ok &= self.w.flush().is_ok();
        self.ok
    }
    /// The measurement header every reply starts with.
    pub fn measured(&mut self, what: &str, source: &str, partial: &[String]) {
        let p: Vec<String> = partial.iter().map(|s| json_str(s)).collect();
        self.always(&format!("{{\"measured\":{},\"at_ms\":{},\"source\":{},\"partial\":[{}]}}", json_str(what), now_ms(), json_str(source), p.join(",")));
    }
}

/// A parsed request: the command word, positional words and k=v arguments.
pub struct Req {
    pub cmd: String,
    pub pos: Vec<String>,
    pub kv: Vec<(String, String)>,
}

impl Req {
    /// `cmd word k=v ...`, or one flat JSON object `{"cmd":"..","k":"v","n":1}` (the cmd may
    /// be several words; values keep their spaces).
    pub fn parse(line: &str) -> Req {
        let line = line.trim();
        if line.starts_with('{') {
            if let Some(mut r) = parse_json_request(line) {
                // `{"cmd":"tweak apply",...}`: the words after the first are positional, as typed
                if r.cmd.contains(char::is_whitespace) {
                    let words: Vec<String> = r.cmd.split_whitespace().map(str::to_string).collect();
                    r.cmd = words[0].clone();
                    r.pos = words[1..].to_vec();
                }
                return r;
            }
        }
        let mut words = line.split_whitespace();
        let cmd = words.next().unwrap_or("").to_string();
        let mut r = Req { cmd, pos: Vec::new(), kv: Vec::new() };
        for w in words {
            let w = w.trim_start_matches("--");
            match w.split_once('=') {
                Some((k, v)) => r.kv.push((k.to_string(), v.to_string())),
                None => r.pos.push(w.to_string()),
            }
        }
        r
    }
    pub fn get(&self, k: &str) -> Option<&str> {
        self.kv.iter().rev().find(|x| x.0 == k).map(|x| x.1.as_str())
    }
    pub fn num(&self, k: &str, d: u64) -> u64 {
        self.get(k).and_then(|v| v.parse().ok()).unwrap_or(d)
    }
}

/// A flat object of string / number / bool values (other values are skipped).
fn parse_json_request(s: &str) -> Option<Req> {
    let b = s.as_bytes();
    let mut i = 1;
    let mut r = Req { cmd: String::new(), pos: Vec::new(), kv: Vec::new() };
    fn ws(b: &[u8], i: &mut usize) {
        while *i < b.len() && b[*i].is_ascii_whitespace() {
            *i += 1;
        }
    }
    fn string(b: &[u8], i: &mut usize) -> Option<String> {
        if b.get(*i) != Some(&b'"') {
            return None;
        }
        *i += 1;
        let mut out: Vec<u8> = Vec::new();
        while *i < b.len() {
            match b[*i] {
                b'"' => {
                    *i += 1;
                    return String::from_utf8(out).ok();
                }
                b'\\' => {
                    *i += 1;
                    match *b.get(*i)? {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let h = std::str::from_utf8(b.get(*i + 1..*i + 5)?).ok()?;
                            let c = char::from_u32(u32::from_str_radix(h, 16).ok()?).unwrap_or('\u{fffd}');
                            out.extend_from_slice(c.to_string().as_bytes());
                            *i += 4;
                        }
                        c => out.push(c),
                    }
                    *i += 1;
                }
                c => {
                    out.push(c);
                    *i += 1;
                }
            }
        }
        None
    }
    loop {
        ws(b, &mut i);
        match b.get(i)? {
            b'}' => return Some(r),
            b',' => {
                i += 1;
                continue;
            }
            _ => {}
        }
        let key = string(b, &mut i)?;
        ws(b, &mut i);
        if b.get(i)? != &b':' {
            return None;
        }
        i += 1;
        ws(b, &mut i);
        let val = if b.get(i)? == &b'"' {
            string(b, &mut i)?
        } else {
            let st = i;
            while i < b.len() && !matches!(b[i], b',' | b'}') {
                i += 1;
            }
            let t = s[st..i].trim();
            match t {
                "true" => "1".into(),
                "false" => "0".into(),
                t if t.parse::<f64>().is_ok() => t.to_string(),
                _ => continue,
            }
        };
        if key == "cmd" {
            r.cmd = val;
        } else {
            r.kv.push((key, val));
        }
    }
}

pub fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// A writer that buffers and sends the buffer as one DATA message on flush (or past 64 KiB).
/// It waits for queue room, so a slow client slows only this command thread.
struct ChanWriter {
    tx: ChannelTx,
    buf: Vec<u8>,
}

impl Write for ChanWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(b);
        if self.buf.len() >= 64 << 10 {
            self.flush()?;
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        self.tx.send(std::mem::take(&mut self.buf))
    }
}

/// The `cmd` service served by `dispatch`.
pub fn service(dispatch: fn(&Req, &mut Out), end_fields: EndFields) -> Factory {
    Arc::new(move |conn, open| {
        let hooks = conn.hooks();
        let end_fields = end_fields.clone();
        std::thread::Builder::new()
            .name("makepad-remote-cmd".into())
            .spawn(move || {
                (hooks.enter)();
                serve(open.channel, dispatch, end_fields);
                (hooks.exit)();
            })
            .map(|_| ())
            .map_err(|e| (crate::conn::CLOSE_REFUSED, format!("cannot start the command thread: {e}")))
    })
}

fn serve(ch: crate::conn::Channel, dispatch: fn(&Req, &mut Out), end_fields: EndFields) {
    let mut w = ChanWriter { tx: ch.tx(), buf: Vec::new() };
    while let Some(ev) = ch.recv() {
        let data = match ev {
            Event::Data(d) => d,
            Event::Closed(..) => break,
            Event::Opened(_) => continue,
        };
        for line in String::from_utf8_lossy(&data).lines() {
            if line.trim().is_empty() {
                continue;
            }
            let req = Req::parse(line);
            let t = std::time::Instant::now();
            let mut out = Out { w: &mut w, bytes: 0, limit: if req.get("full") == Some("1") { usize::MAX } else { LIMIT }, truncated: false, ok: true };
            let cmd = req.cmd.clone();
            // a panic in a command (a bug in the host) answers as an error, the session goes on
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dispatch(&req, &mut out)));
            if r.is_err() {
                out.err("internal error in this command (inspect bug)");
            }
            let trunc = out.truncated;
            let mut extra = String::new();
            if let Some(fields) = *end_fields.lock().unwrap_or_else(|e| e.into_inner()) {
                fields(&mut extra);
            }
            out.always(&format!("{{\"end\":{},\"ms\":{}{}{}}}", json_str(&cmd), t.elapsed().as_millis(), if trunc { ",\"truncated\":true" } else { "" }, extra));
            if !out.flush() {
                ch.close(CLOSE_NORMAL, "");
                return;
            }
        }
    }
}
