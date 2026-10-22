//! Registry of live script VMs for `makepad inspect` (DESIGN.md section 9).
//!
//! Each VM base registers itself lazily, at its first evaluation (a `ScriptVmBase` is built by
//! value and boxed afterwards, so its address is only stable once it runs), and unregisters in
//! `Drop`. Counters are plain atomics updated at points that already cost something (the end
//! of a GC, an evaluation entry, an error); readers only load atomics and never touch a heap.
use crate::heap::ScriptHeap;
use crate::vm::ScriptVmBase;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

struct Entry {
    id: u32,
    label: &'static str,
    thread: String,
    base: usize,
    heap: usize,
    objects: AtomicU64,
    strings: AtomicU64,
    arrays: AtomicU64,
    bytes: AtomicU64,
    gc_runs: AtomicU64,
    gc_ns_total: AtomicU64,
    gc_ns_last: AtomicU64,
    errors: AtomicU64,
    evals: AtomicU64,
    /// errors and host diagnostics by site (`script errors`)
    diags: Mutex<Vec<Diag>>,
}

/// One error site of a VM: what it said, where, how often.
struct Diag {
    /// "error" (a script error the VM reported) or "diagnostic" (host code: a wrong-typed option, ..)
    kind: &'static str,
    file: String,
    line: u32,
    text: String,
    n: u64,
    first_ms: u64,
    last_ms: u64,
}

/// distinct sites kept per VM
const DIAG_SITES: usize = 256;

pub(crate) fn now_ms_pub() -> u64 {
    now_ms()
}

/// The registry id (V<n>) of VM `base`.
pub(crate) fn vm_of(base: &ScriptVmBase) -> Option<u32> {
    find(base as *const ScriptVmBase as usize, false).map(|e| e.id)
}

/// VMs owned by the current thread that it is not evaluating now: (id, base address).
pub(crate) fn idle_here() -> Vec<(u32, usize)> {
    let name = std::thread::current().name().unwrap_or("?").to_owned();
    let key = thread_key();
    let busy = EVALS.lock().map(|v| v.iter().any(|e| e.0 == key)).unwrap_or(true);
    if busy {
        return Vec::new();
    }
    REGISTRY.lock().map(|r| r.iter().filter(|e| e.thread == name).map(|e| (e.id, e.base)).collect()).unwrap_or_default()
}

/// Every live VM: (id, label, evaluations).
pub(crate) fn vm_ids() -> Vec<(u32, &'static str, u64)> {
    REGISTRY.lock().map(|r| r.iter().map(|e| (e.id, e.label, e.evals.load(Relaxed))).collect()).unwrap_or_default()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn note_diag(e: &Entry, kind: &'static str, at: Option<(&str, u32)>, text: &str) {
    let Ok(mut d) = e.diags.lock() else { return };
    let (file, line) = at.unwrap_or(("", 0));
    let t = now_ms();
    if let Some(x) = d.iter_mut().find(|x| x.line == line && x.file == file && x.text == text) {
        x.n += 1;
        x.last_ms = t;
        return;
    }
    if d.len() >= DIAG_SITES {
        return;
    }
    d.push(Diag { kind, file: file.to_string(), line, text: text.to_string(), n: 1, first_ms: t, last_ms: t });
}

static REGISTRY: Mutex<Vec<Arc<Entry>>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static ERROR_HOOK: AtomicUsize = AtomicUsize::new(0);
/// set by the host that reads the registry (makepad inspect, through `set_error_hook`): until then
/// the hooks cost one relaxed load (a native build never pays more)
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Live counts (allocated slots) and the retained slot bytes of the object, string and array
/// tables: slots * size_of the table element (heap-owned buffers inside elements not counted).
fn counts(heap: &ScriptHeap) -> (u64, u64, u64, u64) {
    let objects = heap.objects.len() - heap.objects_free.len();
    let strings = heap.strings.len() - heap.strings_free.len();
    let arrays = heap.arrays.len() - heap.arrays_free.len();
    let bytes = heap.objects.len() * std::mem::size_of::<crate::object::ScriptObjectData>()
        + heap.strings.len() * std::mem::size_of::<Option<crate::string::ScriptStringData>>()
        + heap.arrays.len() * std::mem::size_of::<crate::array::ScriptArrayData>();
    (objects as u64, strings as u64, arrays as u64, bytes as u64)
}

fn store(e: &Entry, heap: &ScriptHeap) {
    let (o, s, a, b) = counts(heap);
    e.objects.store(o, Relaxed);
    e.strings.store(s, Relaxed);
    e.arrays.store(a, Relaxed);
    e.bytes.store(b, Relaxed);
}

fn find(addr: usize, by_heap: bool) -> Option<Arc<Entry>> {
    let reg = REGISTRY.lock().ok()?;
    reg.iter().find(|e| if by_heap { e.heap == addr } else { e.base == addr }).cloned()
}

/// Register a VM base (its address must be stable: it is boxed). A second call is a no-op.
pub fn register(base: *const ScriptVmBase, label: &'static str) {
    let Ok(mut reg) = REGISTRY.lock() else { return };
    if reg.iter().any(|e| e.base == base as usize) {
        return;
    }
    let heap = unsafe { &(*base).heap };
    let e = Entry {
        id: NEXT_ID.fetch_add(1, Relaxed) as u32,
        label,
        thread: std::thread::current().name().unwrap_or("?").to_owned(),
        base: base as usize,
        heap: heap as *const ScriptHeap as usize,
        objects: AtomicU64::new(0),
        strings: AtomicU64::new(0),
        arrays: AtomicU64::new(0),
        bytes: AtomicU64::new(0),
        gc_runs: AtomicU64::new(0),
        gc_ns_total: AtomicU64::new(0),
        gc_ns_last: AtomicU64::new(0),
        errors: AtomicU64::new(0),
        evals: AtomicU64::new(0),
        diags: Mutex::new(Vec::new()),
    };
    store(&e, heap);
    reg.push(Arc::new(e));
}

pub fn unregister(base: *const ScriptVmBase) {
    if !ENABLED.load(Relaxed) {
        return;
    }
    if let Ok(mut reg) = REGISTRY.lock() {
        reg.retain(|e| e.base != base as usize);
    }
}

/// An evaluation starts on this VM: registers it on first use and refreshes the counts.
pub(crate) fn eval_started(base: &ScriptVmBase) {
    if !ENABLED.load(Relaxed) {
        return;
    }
    let addr = base as *const ScriptVmBase as usize;
    let e = match find(addr, false) {
        Some(e) => e,
        None => {
            register(base, "vm");
            match find(addr, false) {
                Some(e) => e,
                None => return,
            }
        }
    };
    e.evals.fetch_add(1, Relaxed);
    store(&e, &base.heap);
}

/// A collection finished on `heap` after `ns` nanoseconds.
pub(crate) fn gc_done(heap: &ScriptHeap, ns: u64) {
    if !ENABLED.load(Relaxed) {
        return;
    }
    if let Some(e) = find(heap as *const ScriptHeap as usize, true) {
        e.gc_runs.fetch_add(1, Relaxed);
        e.gc_ns_total.fetch_add(ns, Relaxed);
        e.gc_ns_last.store(ns, Relaxed);
        store(&e, heap);
    }
}

/// An error was drained on this VM; `line` formats it only when a hook is set.
pub(crate) fn error(base: &ScriptVmBase, line: impl FnOnce() -> String) {
    if !ENABLED.load(Relaxed) {
        return;
    }
    let text = line();
    if let Some(e) = find(base as *const ScriptVmBase as usize, false) {
        e.errors.fetch_add(1, Relaxed);
        note_diag(&e, "error", None, &text);
    }
    let f = ERROR_HOOK.load(Relaxed);
    if f != 0 {
        let f: fn(&str) = unsafe { std::mem::transmute::<usize, fn(&str)>(f) };
        f(&text);
    }
}

/// A host diagnostic at script site `at` (vm_ctl ERR_PUSH_DIAGNOSTIC): counted on every call, the
/// error hook (the `scripterr` event) only at a site's `first` report.
pub(crate) fn diagnostic(base: &ScriptVmBase, at: Option<(&str, u32)>, text: &str, first: bool) {
    if !ENABLED.load(Relaxed) {
        return;
    }
    let addr = base as *const ScriptVmBase as usize;
    let e = match find(addr, false) {
        Some(e) => e,
        None => {
            register(base, "vm");
            match find(addr, false) {
                Some(e) => e,
                None => return,
            }
        }
    };
    e.errors.fetch_add(1, Relaxed);
    note_diag(&e, "diagnostic", at, text);
    let f = ERROR_HOOK.load(Relaxed);
    if f != 0 && first {
        let f: fn(&str) = unsafe { std::mem::transmute::<usize, fn(&str)>(f) };
        match at {
            Some((file, line)) => f(&format!("{}:{}: {}", file, line, text)),
            None => f(text),
        }
    }
}

/// `script errors [vm=V<n>]`: every VM's error sites, the most frequent first, one JSON line each.
pub fn script_errors(vm: Option<u32>) -> Vec<String> {
    let list: Vec<Arc<Entry>> = match REGISTRY.lock() {
        Ok(r) => r.iter().filter(|e| vm.map_or(true, |v| v == e.id)).cloned().collect(),
        Err(_) => return Vec::new(),
    };
    let t = now_ms();
    let mut rows: Vec<(u64, String)> = Vec::new();
    for e in &list {
        let Ok(d) = e.diags.lock() else { continue };
        for x in d.iter() {
            let at = if x.file.is_empty() { String::new() } else { format!(",\"at\":{}", json_str(&format!("{}:{}", x.file, x.line))) };
            rows.push((
                x.n,
                format!(
                    "{{\"vm\":\"V{}\",\"label\":{},\"kind\":\"{}\"{},\"n\":{},\"first_s_ago\":{:.1},\"last_s_ago\":{:.1},\"text\":{}}}",
                    e.id,
                    json_str(e.label),
                    x.kind,
                    at,
                    x.n,
                    t.saturating_sub(x.first_ms) as f64 / 1000.0,
                    t.saturating_sub(x.last_ms) as f64 / 1000.0,
                    json_str(&x.text)
                ),
            ));
        }
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    rows.into_iter().map(|r| r.1).collect()
}

/// Every uncaught error that is reported or captured is passed to `f` as one formatted line.
/// Setting it also switches the registry on.
pub fn set_error_hook(f: fn(&str)) {
    ERROR_HOOK.store(f as usize, Relaxed);
    ENABLED.store(true, Relaxed);
}

pub(crate) fn json_str(s: &str) -> String {
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

/// One compact JSON object per live VM (atomics only; safe from any thread).
pub fn summaries() -> Vec<String> {
    let list: Vec<Arc<Entry>> = match REGISTRY.lock() {
        Ok(r) => r.clone(),
        Err(_) => return Vec::new(),
    };
    list.iter()
        .map(|e| {
            format!(
                "{{\"vm\":\"V{}\",\"label\":{},\"thread\":{},\"objects\":{},\"strings\":{},\"arrays\":{},\"heap_bytes_est\":{},\"gc_runs\":{},\"gc_ms_total\":{:.3},\"gc_ms_last\":{:.3},\"errors\":{},\"evals\":{}}}",
                e.id,
                json_str(e.label),
                json_str(&e.thread),
                e.objects.load(Relaxed),
                e.strings.load(Relaxed),
                e.arrays.load(Relaxed),
                e.bytes.load(Relaxed),
                e.gc_runs.load(Relaxed),
                e.gc_ns_total.load(Relaxed) as f64 / 1e6,
                e.gc_ns_last.load(Relaxed) as f64 / 1e6,
                e.errors.load(Relaxed),
                e.evals.load(Relaxed),
            )
        })
        .collect()
}

// ------------------------------------------------------------------ script stacks

/// Per OS thread: (pthread handle, VM base address being evaluated, nesting depth).
static EVALS: Mutex<Vec<(u64, usize, u32)>> = Mutex::new(Vec::new());

#[cfg(unix)]
fn thread_key() -> u64 {
    extern "C" {
        fn pthread_self() -> usize;
    }
    unsafe { pthread_self() as u64 }
}
#[cfg(not(unix))]
fn thread_key() -> u64 {
    0
}

/// Held for the length of `eval_body`: records which VM this thread evaluates (only while the
/// registry is enabled) and removes it on every exit, early returns and unwinding included.
pub(crate) struct EvalGuard {
    key: u64,
}

pub(crate) fn eval_guard(base: &ScriptVmBase) -> EvalGuard {
    crate::inspect_query::safe_point(base);
    eval_started(base);
    if !ENABLED.load(Relaxed) {
        return EvalGuard { key: 0 };
    }
    let key = thread_key();
    if key != 0 {
        if let Ok(mut v) = EVALS.lock() {
            match v.iter_mut().find(|e| e.0 == key) {
                Some(e) => {
                    e.1 = base as *const ScriptVmBase as usize;
                    e.2 += 1;
                }
                None => v.push((key, base as *const ScriptVmBase as usize, 1)),
            }
        }
    }
    EvalGuard { key }
}

impl Drop for EvalGuard {
    fn drop(&mut self) {
        if self.key == 0 {
            return;
        }
        if let Ok(mut v) = EVALS.lock() {
            if let Some(i) = v.iter().position(|e| e.0 == self.key) {
                if v[i].2 <= 1 {
                    v.swap_remove(i);
                } else {
                    v[i].2 -= 1;
                }
            }
        }
    }
}

/// The script frames (innermost first, at most `max`, each "fn-header file:line:col") of the VM
/// that the OS thread `thread` (a pthread handle) is evaluating now; None when it is in no VM.
///
/// Only valid while that thread is stopped (parked at a panic or breakpoint, or paused): it
/// reads the VM's call frames and code bodies without any lock. The caller guarantees the
/// stop. Any panic while reading (a body borrowed mid-edit) gives None.
pub fn script_stack(thread: u64, max: usize) -> Option<Vec<String>> {
    let base = EVALS.lock().ok()?.iter().find(|e| e.0 == thread)?.1;
    if find(base, false).is_none() {
        return None;
    }
    let max = max.clamp(1, 64);
    std::panic::catch_unwind(|| {
        let bx = unsafe { &*(base as *const ScriptVmBase) };
        let th = bx.threads.cur_ref();
        let mut ips = vec![th.trap.cur_ip()];
        for frame in th.calls.iter().rev() {
            if ips.len() >= max {
                break;
            }
            if let Some(ip) = frame.return_ip {
                ips.push(crate::value::ScriptIp { body: ip.body, index: ip.index.saturating_sub(1) });
            }
        }
        ips.iter()
            .map(|&ip| {
                let name = bx
                    .code
                    .fn_text(ip)
                    .map(|(_, t)| {
                        let head = t.lines().next().unwrap_or("").trim().trim_end_matches('{').trim().to_owned();
                        head.chars().take(60).collect::<String>()
                    })
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "<body>".to_owned());
                match bx.code.ip_to_loc(ip) {
                    Some(l) => format!("{} {}:{}:{}", name, l.file, l.line, l.col),
                    None => format!("{} ?", name),
                }
            })
            .collect()
    })
    .ok()
}

/// The names of the modules of VM `vm_id` (the number of its "V<n>" label). Same caveat as
/// [`script_stack`]: only while the VM is stopped or idle; empty when unknown.
pub fn modules(vm_id: u32) -> Vec<String> {
    let heap = match REGISTRY.lock() {
        Ok(r) => match r.iter().find(|e| e.id == vm_id) {
            Some(e) => e.heap,
            None => return Vec::new(),
        },
        Err(_) => return Vec::new(),
    };
    std::panic::catch_unwind(|| {
        let heap = unsafe { &*(heap as *const ScriptHeap) };
        let slots = heap.object_slots();
        if !slots.is_valid(heap.modules) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (key, _) in slots[heap.modules].map.iter() {
            if out.len() >= 4096 {
                break;
            }
            if let Some(id) = key.as_id() {
                out.push(id.as_string(|s| s.map(|s| s.to_string())).unwrap_or_else(|| format!("{:?}", id)));
            }
        }
        out
    })
    .unwrap_or_default()
}
