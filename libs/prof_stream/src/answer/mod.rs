//! The text answers of `makepad inspect`: what an agent (or a person) reads by default; `as=json`
//! prints the protocol's JSON lines instead (programs; Scope reads the binary `prof` stream).
//!
//! Rendered on the client from the answer's JSON lines, so every transport (local socket, hub,
//! native apps' endpoints) answers in the same words. The one renderer: the inspect client
//! (relax-compiler) and Scope's Live console both call [`render`]. The rules (profproto.md §5),
//! for LLM readers first, cheap in tokens and hard to misread:
//! - answer first: line 1 is the finding with its key number; evidence follows;
//! - the smallest useful answer; `more:` at the end names the drill-down commands with the ids
//!   this answer printed (T3 thread, S35 fn slot, H7 heap site, P1 panic, F140 frame, V2 VM);
//! - markdown tables with one header row, units in the header, 2-3 significant digits;
//! - short names (the last two path segments), file paths relative to the workspace;
//! - what is left out is said (`+41 smaller, 0.3 MB`); errors say what to ask instead.
//!
//! Each command's renderer reads only fields it knows and falls back to the generic table for
//! the rest, so a new JSON field never breaks an answer.

mod cpu;
mod frames;
mod hang;
mod jit;
mod live;
mod memory;
mod misc;
#[cfg(test)]
mod tests;

use makepad_strict_json::{self as json, Value};
use std::fmt::Write as _;

pub use frames::Frames;

/// A request as the client parsed it (makepad-remote's `Req`: `cmd word k=v ..`).
pub struct Request {
    pub cmd: String,
    pub pos: Vec<String>,
    pub kv: Vec<(String, String)>,
}

impl Request {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.kv.iter().rev().find(|x| x.0 == k).map(|x| x.1.as_str())
    }
}

/// Renders the answer to request `req` from its JSON lines (the `end` line left out).
pub fn render(req: &Request, lines: &[String]) -> String {
    let mut a = Answer::parse(req, lines);
    let sub = req.pos.first().map(String::as_str).unwrap_or("");
    match (req.cmd.as_str(), sub) {
        _ if a.only_errors() => {}
        ("help", _) => misc::help(&mut a),
        ("prof", _) => cpu::prof(&mut a),
        ("top", _) if matches!(req.get("by").unwrap_or("time"), "time" | "cpu") => cpu::prof(&mut a),
        ("top", _) | ("heap", "top") => memory::heap_top(&mut a),
        ("why-slow", _) => cpu::why_slow(&mut a),
        ("why-hung" | "hang", _) => hang::why_hung(&mut a),
        ("threads", _) => hang::threads(&mut a),
        ("waits", _) => hang::waits(&mut a),
        ("owner", _) => hang::owner(&mut a),
        ("stacks", _) => hang::stacks(&mut a),
        ("panics" | "events" | "watch", _) => misc::events(&mut a),
        ("locals", _) => misc::locals(&mut a),
        ("memory", _) => memory::memory(&mut a),
        ("heap", "path") | ("path", _) => memory::heap_path(&mut a),
        ("heap", "diff") => memory::heap_top(&mut a),
        ("heap", "start" | "stop" | "snap" | "status") => memory::heap_state(&mut a),
        ("vms", _) => memory::vms(&mut a),
        ("mem", "timeline") => memory::mem_timeline(&mut a),
        ("versions", _) => jit::versions(&mut a),
        ("probes", _) => jit::probes(&mut a),
        ("probe", "read") => jit::probe_read(&mut a),
        ("src", _) => live::src(&mut a),
        ("script", _) => live::script_errors(&mut a),
        ("vm", _) => live::vm(&mut a),
        ("verbs", _) => live::verbs(&mut a),
        ("game", _) => live::game(&mut a),
        ("writes", _) => live::writes(&mut a),
        ("timeline" | "frames", "frames") | ("frames", _) => misc::frames(&mut a),
        ("timeline" | "frame", "frame") | ("frame", _) => misc::frame(&mut a),
        ("hitch", _) => misc::hitch(&mut a),
        _ => {}
    }
    a.finish()
}

/// An answer being written: the parsed lines, which of them a renderer used, and the text.
pub struct Answer {
    pub cmd: String,
    pub req: Request,
    /// the answer's lines, parsed (non-JSON lines as strings)
    pub lines: Vec<Value>,
    /// lines a renderer used (the rest go to the generic part)
    pub used: Vec<bool>,
    /// the measurement header's text and partial notes
    pub measured: String,
    pub partial: Vec<String>,
    /// the finding (line 1), the body, the drill-downs
    pub head: String,
    pub body: String,
    pub more: Vec<String>,
    pub names: Names,
    pub errors: Vec<String>,
    pub notes: Vec<String>,
}

impl Answer {
    fn parse(req: &Request, lines: &[String]) -> Answer {
        let mut a = Answer {
            cmd: req.cmd.clone(),
            req: Request { cmd: req.cmd.clone(), pos: req.pos.clone(), kv: req.kv.clone() },
            lines: Vec::new(),
            used: Vec::new(),
            measured: String::new(),
            partial: Vec::new(),
            head: String::new(),
            body: String::new(),
            more: Vec::new(),
            names: Names::default(),
            errors: Vec::new(),
            notes: Vec::new(),
        };
        for l in lines {
            let v = json::parse_depth(l.as_bytes(), 64).unwrap_or_else(|_| Value::Str(l.clone()));
            let mut used = false;
            if let Some(m) = v.get("measured").and_then(Value::as_str) {
                a.measured = m.to_string();
                if let Some(p) = v.get("partial").and_then(Value::as_arr) {
                    a.partial.extend(p.iter().filter_map(Value::as_str).map(str::to_string));
                }
                used = true;
            } else if let Some(e) = v.get("err").and_then(Value::as_str) {
                a.errors.push(e.to_string());
                used = true;
            } else if v.get("verified").and_then(Value::as_bool) == Some(false) && obj_len(&v) <= 2 {
                a.notes.push(format!("unverified: {}", v.get("why").and_then(Value::as_str).unwrap_or("a patched fn runs before its borrow-check verdict")));
                used = true;
            }
            a.names.collect_paths(&v);
            a.lines.push(v);
            a.used.push(used);
        }
        a
    }

    fn only_errors(&self) -> bool {
        !self.errors.is_empty() && self.used.iter().all(|u| *u)
    }

    /// The lines that carry `key`, marked used.
    pub fn take(&mut self, key: &str) -> Vec<Value> {
        let mut out = Vec::new();
        for (i, l) in self.lines.iter().enumerate() {
            if !self.used[i] && l.get(key).is_some() {
                self.used[i] = true;
                out.push(l.clone());
            }
        }
        out
    }

    /// The first line that carries `key`, marked used.
    pub fn take1(&mut self, key: &str) -> Option<Value> {
        self.take_where(|l| l.get(key).is_some())
    }

    pub fn take_where(&mut self, f: impl Fn(&Value) -> bool) -> Option<Value> {
        for (i, l) in self.lines.iter().enumerate() {
            if !self.used[i] && f(l) {
                self.used[i] = true;
                return Some(l.clone());
            }
        }
        None
    }

    pub fn line(&mut self, s: &str) {
        self.body.push_str(s);
        if !s.ends_with('\n') {
            self.body.push('\n');
        }
    }

    pub fn more(&mut self, s: impl Into<String>) {
        let s = s.into();
        if !self.more.contains(&s) {
            self.more.push(s);
        }
    }

    /// The `more` hints the server sent (a `more` field: string or array), unless a renderer
    /// wrote its own.
    fn server_more(&mut self) {
        for (i, l) in self.lines.iter().enumerate() {
            let m = match l.get("more") {
                Some(Value::Str(s)) => vec![s.clone()],
                Some(Value::Arr(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
                _ => continue,
            };
            if obj_len(l) == 1 {
                self.used[i] = true;
            }
            if self.more.is_empty() {
                for s in m {
                    if !self.more.contains(&s) {
                        self.more.push(s);
                    }
                }
            }
        }
    }

    fn finish(mut self) -> String {
        self.server_more();
        let mut out = String::new();
        for e in &self.errors {
            let _ = writeln!(out, "error: {}", self.names.text(e));
        }
        // the lines no renderer used: a generic rendering (tables for runs of like objects)
        let rest: Vec<Value> = self.lines.iter().zip(&self.used).filter(|(_, u)| !**u).map(|(l, _)| l.clone()).collect();
        let generic = generic(&rest, &mut self.names);
        let head = if !self.head.is_empty() {
            self.head.clone()
        } else if !self.measured.is_empty() && (self.errors.is_empty() || !generic.is_empty() || !self.body.is_empty()) {
            self.measured.clone()
        } else {
            String::new()
        };
        if !head.is_empty() {
            out.push_str(head.trim_end());
            out.push('\n');
        }
        if !self.body.is_empty() {
            out.push_str(&self.body);
        }
        out.push_str(&generic);
        for n in &self.notes {
            let _ = writeln!(out, "note: {}", n);
        }
        if !self.partial.is_empty() {
            let _ = writeln!(out, "partial: {}", self.partial.join("; "));
        }
        out.push_str(&self.names.legend());
        if !self.more.is_empty() {
            let _ = writeln!(out, "more: {}", self.more.join(" · "));
        }
        out
    }
}

pub fn obj_len(v: &Value) -> usize {
    match v {
        Value::Obj(p) => p.len(),
        _ => 0,
    }
}

// ------------------------------------------------------------------ values

pub fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Int(i) => Some(*i as f64),
        Value::F64(f) => Some(*f),
        Value::Str(s) => s.parse().ok(),
        _ => None,
    }
}

pub fn f(v: &Value, k: &str) -> f64 {
    num(v.get(k)).unwrap_or(0.0)
}

pub fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

pub fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k).and_then(Value::as_arr).unwrap_or(&[])
}

/// 2-3 significant digits, no trailing zeros: 1234 -> "1234", 12.34 -> "12.3", 0.3141 -> "0.31".
pub fn sig(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return "0".into();
    }
    let a = x.abs();
    let dec = if a >= 100.0 {
        0
    } else if a >= 10.0 {
        1
    } else if a >= 1.0 {
        2
    } else {
        // two significant digits below 1
        ((-a.log10()).floor() as usize + 2).min(6)
    };
    let mut t = format!("{:.*}", dec, x);
    if t.contains('.') {
        while t.ends_with('0') {
            t.pop();
        }
        if t.ends_with('.') {
            t.pop();
        }
    }
    t
}

/// A duration in ms: "12.4 ms", "0.31 ms", under 0.1 ms in µs.
pub fn ms(x: f64) -> String {
    if x.abs() >= 10_000.0 {
        format!("{} s", sig(x / 1000.0))
    } else if x.abs() < 0.1 && x != 0.0 {
        format!("{} µs", sig(x * 1000.0))
    } else {
        format!("{} ms", sig(x))
    }
}

/// A size in MB: "12.4 MB", "1.67 GB", "310 KB".
pub fn mb(x: f64) -> String {
    if x.abs() >= 1024.0 {
        format!("{} GB", sig(x / 1024.0))
    } else if x.abs() < 1.0 && x != 0.0 {
        format!("{} KB", sig(x * 1024.0))
    } else {
        format!("{} MB", sig(x))
    }
}

pub fn pct(x: f64) -> String {
    if x >= 10.0 || x == 0.0 {
        format!("{:.0}%", x)
    } else {
        format!("{}%", sig(x))
    }
}

/// `▁▂▃▄▅▆▇█` of `vals` (min..max), at most `width` columns (the max of each bucket).
pub fn spark(vals: &[f64], width: usize) -> String {
    if vals.is_empty() {
        return String::new();
    }
    const B: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let per = vals.len().div_ceil(width.max(1));
    let cols: Vec<f64> = vals.chunks(per).map(|c| c.iter().cloned().fold(f64::MIN, f64::max)).collect();
    let lo = cols.iter().cloned().fold(f64::MAX, f64::min);
    let hi = cols.iter().cloned().fold(f64::MIN, f64::max);
    cols.iter().map(|v| if hi <= lo { B[0] } else { B[(((v - lo) / (hi - lo)) * 7.0).round() as usize] }).collect()
}

// ------------------------------------------------------------------ tables

pub struct Table {
    head: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(head: &[&str]) -> Table {
        Table { head: head.iter().map(|s| s.to_string()).collect(), rows: Vec::new() }
    }
    pub fn row(&mut self, cells: Vec<String>) {
        self.rows.push(cells);
    }
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    /// Markdown: one header row, a rule, then the rows; columns that are empty in every row go.
    pub fn md(&self) -> String {
        let keep: Vec<usize> = (0..self.head.len()).filter(|&c| self.rows.iter().any(|r| r.get(c).is_some_and(|x| !x.is_empty()))).collect();
        let mut out = String::new();
        let cell = |s: &str| s.replace('|', "\\|").replace('\n', " ");
        out.push_str(&format!("| {} |\n", keep.iter().map(|&c| cell(&self.head[c])).collect::<Vec<_>>().join(" | ")));
        out.push_str(&format!("|{}|\n", keep.iter().map(|_| "---").collect::<Vec<_>>().join("|")));
        for r in &self.rows {
            out.push_str(&format!("| {} |\n", keep.iter().map(|&c| cell(r.get(c).map(String::as_str).unwrap_or(""))).collect::<Vec<_>>().join(" | ")));
        }
        out
    }
}

// ------------------------------------------------------------------ names and paths

/// Short names and the workspace root of file paths (said once).
#[derive(Default)]
pub struct Names {
    /// the common directory of the answer's absolute file paths (stripped from every path)
    root: String,
    paths: Vec<String>,
    root_used: bool,
}

impl Names {
    fn collect_paths(&mut self, v: &Value) {
        match v {
            Value::Str(s) => {
                for w in s.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',' || c == '"') {
                    if w.starts_with('/') && w.contains(".rs") {
                        self.paths.push(w.to_string());
                    }
                }
            }
            Value::Arr(a) => a.iter().for_each(|x| self.collect_paths(x)),
            Value::Obj(p) => p.iter().for_each(|(_, x)| self.collect_paths(x)),
            _ => {}
        }
        if self.paths.len() >= 2 || (self.paths.len() == 1 && self.root.is_empty()) {
            self.root = common_dir(&self.paths);
        }
    }

    /// `path:line:col` relative to the root, the column dropped; Relax's std as `std/...`.
    pub fn at(&mut self, at: &str) -> String {
        let mut p = at.to_string();
        if !self.root.is_empty() {
            if let Some(r) = p.strip_prefix(self.root.as_str()) {
                p = r.to_string();
                self.root_used = true;
            }
        }
        if let Some(i) = p.find("libs/relax/compiler/std/") {
            p = format!("std/{}", &p[i + "libs/relax/compiler/std/".len()..]);
        }
        // drop the column (`file.rs:89:9` -> `file.rs:89`)
        let parts: Vec<&str> = p.rsplitn(3, ':').collect();
        if parts.len() == 3 && parts[0].chars().all(|c| c.is_ascii_digit()) && parts[1].chars().all(|c| c.is_ascii_digit()) {
            p = format!("{}:{}", parts[2], parts[1]);
        }
        p
    }

    /// A free text with its absolute paths made relative.
    pub fn text(&mut self, t: &str) -> String {
        if self.root.is_empty() || !t.contains(self.root.as_str()) {
            return t.to_string();
        }
        self.root_used = true;
        let mut out = t.replace(self.root.as_str(), "");
        out = out.replace("libs/relax/compiler/std/", "std/");
        out
    }

    fn legend(&self) -> String {
        if self.root_used && !self.root.is_empty() {
            format!("paths: under {}\n", self.root)
        } else {
            String::new()
        }
    }
}

fn common_dir(paths: &[String]) -> String {
    // the workspace root: where Relax's std lives (`<root>libs/relax/compiler/std/`)
    if let Some(i) = paths.iter().find_map(|p| p.find("libs/relax/compiler/std/").map(|i| (p, i))).map(|(p, i)| p[..i].to_string()) {
        if paths.iter().all(|p| p.starts_with(&i)) {
            return i;
        }
    }
    let Some(first) = paths.first() else { return String::new() };
    let mut pre: &str = first.as_str();
    for p in &paths[1..] {
        let n = pre.bytes().zip(p.bytes()).take_while(|(a, b)| a == b).count();
        pre = &pre[..n];
    }
    match pre.rfind('/') {
        // a root of at least two directories (never `/` or `/Users/`)
        Some(i) if pre[..i].matches('/').count() >= 2 => pre[..=i].to_string(),
        _ => String::new(),
    }
}

/// A fn path's last two segments (outside `<..>`): `core::slice::iter::<Iter as Iterator>::next`
/// -> `<Iter as Iterator>::next`, `relax_inspect_eval::score_items` stays.
pub fn short_fn(name: &str) -> String {
    let name = name.trim();
    if let Some(rest) = name.strip_prefix("{closure} in ") {
        return format!("{{closure}} in {}", short_fn(rest));
    }
    let mut segs: Vec<&str> = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'<' | b'(' | b'[' => depth += 1,
            b'>' | b')' | b']' => depth -= 1,
            b':' if depth == 0 && i + 1 < b.len() && b[i + 1] == b':' => {
                segs.push(&name[start..i]);
                start = i + 2;
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    segs.push(&name[start..]);
    let n = segs.len();
    let s = if n <= 2 { name.to_string() } else { format!("{}::{}", segs[n - 2], segs[n - 1]) };
    if s.len() > 90 {
        format!("{}..", &s[..s.char_indices().take_while(|(i, _)| *i < 88).last().map_or(0, |(i, c)| i + c.len_utf8())])
    } else {
        s
    }
}

// ------------------------------------------------------------------ generic

/// Lines no renderer knows: runs of objects with the same scalar keys become one table; other
/// objects `key: value` lines (nested values compact).
fn generic(lines: &[Value], names: &mut Names) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        let l = &lines[i];
        let Value::Obj(p) = l else {
            if let Value::Str(s) = l {
                out.push_str(s);
                out.push('\n');
            }
            i += 1;
            continue;
        };
        let keys: Vec<&str> = p.iter().map(|x| x.0.as_str()).collect();
        let flat = p.iter().all(|(_, v)| !matches!(v, Value::Obj(_)) && !matches!(v, Value::Arr(a) if a.iter().any(|x| matches!(x, Value::Obj(_)))));
        let mut j = i + 1;
        while flat && j < lines.len() && matches!(&lines[j], Value::Obj(q) if q.iter().map(|x| x.0.as_str()).collect::<Vec<_>>() == keys) {
            j += 1;
        }
        if j - i >= 2 {
            let mut t = Table::new(&keys.iter().map(|k| head_of(k)).collect::<Vec<_>>().iter().map(String::as_str).collect::<Vec<_>>());
            for r in &lines[i..j] {
                if let Value::Obj(q) = r {
                    t.row(q.iter().map(|(k, v)| cell(k, v, names)).collect());
                }
            }
            out.push_str(&t.md());
            i = j;
            continue;
        }
        for (k, v) in p {
            if k == "more" {
                continue;
            }
            match v {
                Value::Arr(a) if a.iter().all(|x| matches!(x, Value::Obj(_))) && !a.is_empty() => {
                    let _ = writeln!(out, "{}:", k);
                    out.push_str(&generic(a, names));
                }
                _ => {
                    let _ = writeln!(out, "{}: {}", k, cell(k, v, names));
                }
            }
        }
        i += 1;
    }
    out
}

/// A key's column title: `_mb` / `_ms` / `_pct` / `_bytes` suffixes become units.
fn head_of(k: &str) -> String {
    for (suf, unit) in [("_mb", "MB"), ("_ms", "ms"), ("_pct", "%"), ("_bytes", "bytes"), ("_ns", "ns"), ("_s", "s")] {
        if let Some(b) = k.strip_suffix(suf) {
            return format!("{} {}", b.replace('_', " "), unit);
        }
    }
    k.replace('_', " ")
}

/// A scalar as text.
pub fn cell_of(v: &Value) -> String {
    match v {
        Value::Bool(b) => if *b { "yes".into() } else { "no".into() },
        Value::Int(i) => i.to_string(),
        Value::F64(x) => sig(*x),
        Value::Str(s) => s.clone(),
        other => other.to_json(),
    }
}

fn cell(k: &str, v: &Value, names: &mut Names) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => if *b { "yes".into() } else { "no".into() },
        Value::Int(i) => if k.ends_with("_bytes") && *i >= 1 << 20 { mb(*i as f64 / 1048576.0) } else { i.to_string() },
        Value::F64(x) => sig(*x),
        Value::Str(s) => {
            if k == "at" || k == "location" || k.ends_with("_line") {
                names.at(s)
            } else if k == "fn" {
                short_fn(s)
            } else {
                names.text(s)
            }
        }
        Value::Arr(a) => a.iter().map(|x| cell(k, x, names)).collect::<Vec<_>>().join(", "),
        Value::Obj(p) => p.iter().map(|(k2, v2)| format!("{}={}", k2, cell(k2, v2, names))).collect::<Vec<_>>().join(" "),
    }
}
