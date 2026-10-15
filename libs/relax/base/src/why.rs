//! Opt-in, first-observation provenance. MAKEPAD_RUN_WHY=/path/events writes
//! /path/events.<pid>.jsonl (separate files across child processes and forks) and records
//! reads separately from collection, lowering, body checking and code generation.
//! A cause is a compiler request, not evidence that an application called a function.
use std::{cell::RefCell, collections::BTreeMap, fs::File, io::Write, sync::OnceLock};
use relax_session::sync::Mutex;

#[derive(Clone, Default)]
pub struct Cause {
    pub file: String,
    pub line: usize,
    pub kind: String,
    pub symbol: String,
}
impl Cause {
    pub fn new(file: &str, line: usize, kind: &str, symbol: &str) -> Self {
        Self { file: file.into(), line, kind: kind.into(), symbol: symbol.into() }
    }
}
struct Trace {
    file: File,
    path: std::ffi::OsString,
    pid: u32,
    seq: u64,
    first: BTreeMap<(String, String, usize, String), Cause>,
    edges: BTreeMap<String, Cause>,
}
static TRACE: OnceLock<Option<Mutex<Trace>>> = OnceLock::new();
fn trace() -> Option<&'static Mutex<Trace>> {
    TRACE.get_or_init(|| std::env::var_os("MAKEPAD_RUN_WHY").map(|path| {
        let pid = std::process::id();
        let file = open(&path, pid);
        Mutex::new(Trace { file, path, pid, seq: 0, first: BTreeMap::new(), edges: BTreeMap::new() })
    })).as_ref()
}
fn open(path: &std::ffi::OsStr, pid: u32) -> File {
    let mut name = path.to_os_string(); name.push(format!(".{pid}.jsonl"));
    File::create(&name).unwrap_or_else(|e| panic!("MAKEPAD_RUN_WHY {:?}: {e}", name))
}
impl Trace {
    fn process(&mut self) {
        let pid = std::process::id();
        if self.pid != pid {
            self.file = open(&self.path, pid); self.pid = pid; self.seq = 0;
            self.first.clear(); self.edges.clear();
        }
    }
}
pub fn enabled() -> bool { trace().is_some() }
crate::gen_local! { static CAUSE: RefCell<Option<Cause>>; }
pub struct Scope { previous: Option<Option<Cause>> }
pub fn enter(cause: Cause) -> Scope {
    Scope { previous: enabled().then(|| CAUSE.with(|c| c.replace(Some(cause)))) }
}
impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() { let _ = CAUSE.try_with(|c| c.replace(previous)); }
    }
}
fn quoted(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""), '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"), '\r' => out.push_str("\\r"), '\t' => out.push_str("\\t"),
            c if c < ' ' => { use std::fmt::Write; let _ = write!(out, "\\u{:04x}", c as u32); },
            c => out.push(c),
        }
    }
    out.push('"'); out
}
fn write_event(t: &mut Trace, phase: &str, target: &str, line: usize, symbol: &str, cause: &Cause) {
    t.seq += 1;
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let line = format!("{{\"seq\":{},\"pid\":{},\"at_ns\":{},\"phase\":{},\"target\":{},\"line\":{},\"symbol\":{},\"from\":{},\"from_line\":{},\"kind\":{},\"caller\":{}}}",
        t.seq, t.pid, at, quoted(phase), quoted(target), line, quoted(symbol), quoted(&cause.file), cause.line, quoted(&cause.kind), quoted(&cause.symbol));
    t.file.write_all(line.as_bytes()).expect("write MAKEPAD_RUN_WHY");
    t.file.write_all(b"\n").expect("write MAKEPAD_RUN_WHY newline");
}
/// Remember the first scheduling edge before a worker reads the file.
pub fn edge(target: &str, cause: Cause) {
    let Some(t) = trace() else { return; };
    let mut t = t.lock().unwrap_or_else(|e| e.into_inner());
    t.process();
    let key = crate::vfs::normalize_path(target);
    if t.edges.contains_key(&key) { return; }
    t.edges.insert(key, cause.clone());
    write_event(&mut t, "edge", target, 0, "", &cause);
}
/// Explicit causes are used by the lazy closure, whose workers run later.
pub fn event(phase: &str, target: &str, line: usize, symbol: &str, cause: Option<Cause>) {
    let Some(t) = trace() else { return; };
    let mut t = t.lock().unwrap_or_else(|e| e.into_inner());
    t.process();
    let path = crate::vfs::normalize_path(target);
    let key = (phase.into(), path.clone(), line, symbol.into());
    if t.first.contains_key(&key) { return; }
    let demand = matches!(phase, "typeck" | "lower-fn" | "compile")
        .then(|| t.first.get(&("fn-demand".into(), path.clone(), line, symbol.into())).cloned()).flatten();
    let cause = cause.or(demand).or_else(|| CAUSE.try_with(|c| c.borrow().clone()).ok().flatten().filter(|c| c.file != target || c.line != line || c.symbol != symbol))
        .or_else(|| t.first.get(&("fn-demand".into(), path.clone(), line, symbol.into())).cloned())
        .or_else(|| t.first.get(&("select".into(), path.clone(), 0, String::new())).cloned())
        .or_else(|| t.edges.get(&path).cloned())
        .unwrap_or_else(|| Cause::new("", 0, "compiler-root", ""));
    write_event(&mut t, phase, target, line, symbol, &cause);
    t.first.insert(key, cause);
}
pub fn read(path: &std::path::Path) {
    if !enabled() || !matches!(path.extension().and_then(|s| s.to_str()), Some("rs" | "rx")) { return; }
    event("read", &path.to_string_lossy(), 0, "", None);
}
