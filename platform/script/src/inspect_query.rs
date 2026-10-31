//! `makepad inspect` queries over live Splash VMs (read-only): the running script source with
//! line numbers (`src`), a VM's top-level names and any value by path (`vm globals`, `vm get`),
//! the script->host call trace (`verbs`) and the error sites (`script errors`).
//!
//! A query that reads a heap runs ON THE VM'S OWN THREAD, at its next entry into script (an
//! evaluation or a host call into script: `safe_point`), never from the inspect thread while the
//! VM runs: the inspect thread queues the job and waits for its answer. When nothing is pending a
//! safe point costs one relaxed load. Answers are JSON lines (the inspect client renders text).
use crate::heap::ScriptHeap;
use crate::inspect_registry::{json_str, vm_of};
use crate::value::*;
use crate::vm::{ScriptSource, ScriptVmBase};
use crate::makepad_live_id::*;
use crate::trap::*;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;

// ------------------------------------------------------------------ safe points

type Job = Box<dyn FnOnce(&ScriptVmBase) -> Vec<String> + Send>;

static PENDING: Mutex<Vec<(u32, u64, Job, Sender<Vec<String>>)>> = Mutex::new(Vec::new());
static NEXT_JOB: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static ANY_PENDING: AtomicBool = AtomicBool::new(false);

/// The VM `base` is about to run script on its own thread: answer the queries waiting for it.
#[inline(always)]
pub(crate) fn safe_point(base: &ScriptVmBase) {
    if ANY_PENDING.load(Relaxed) {
        serve(base);
    }
}

#[cold]
#[inline(never)]
fn serve(base: &ScriptVmBase) {
    let Some(id) = vm_of(base) else { return };
    let mine: Vec<(u32, u64, Job, Sender<Vec<String>>)> = {
        let Ok(mut p) = PENDING.lock() else { return };
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *p).into_iter().partition(|j| j.0 == id);
        *p = rest;
        ANY_PENDING.store(!p.is_empty(), Relaxed);
        mine
    };
    for (_, _, job, tx) in mine {
        let lines = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(base))).unwrap_or_else(|_| vec![err("the query panicked while reading the VM")]);
        let _ = tx.send(lines);
    }
}

/// The app's event loop is between events (makepad-platform calls this every loop turn): the
/// queries waiting for a VM this thread owns and is not evaluating are answered now, so an idle
/// VM (a game between ticks, a paused UI) answers too. One relaxed load when nothing waits.
#[makepad_relax_api::relax_api]
pub fn serve_idle() {
    if !ANY_PENDING.load(Relaxed) {
        return;
    }
    for (id, base) in crate::inspect_registry::idle_here() {
        let has = PENDING.lock().map(|p| p.iter().any(|j| j.0 == id)).unwrap_or(false);
        if has {
            // SAFETY: the registry holds live, boxed bases; this thread owns the VM and is not
            // inside one of its evaluations (idle_here), so nothing mutates its heap meanwhile
            serve(unsafe { &*(base as *const ScriptVmBase) });
        }
    }
}

/// Runs `job` on VM `id`'s thread at its next safe point, waiting up to `wait_ms`.
fn run_on(id: u32, wait_ms: u64, job: Job) -> Vec<String> {
    let (tx, rx) = channel();
    let me = NEXT_JOB.fetch_add(1, Relaxed);
    if let Ok(mut p) = PENDING.lock() {
        p.push((id, me, job, tx));
        ANY_PENDING.store(true, Relaxed);
    }
    match rx.recv_timeout(std::time::Duration::from_millis(wait_ms)) {
        Ok(lines) => lines,
        Err(_) => {
            // withdraw the job: the VM never reached a safe point
            if let Ok(mut p) = PENDING.lock() {
                p.retain(|j| j.1 != me);
                ANY_PENDING.store(!p.is_empty(), Relaxed);
            }
            vec![err(&format!(
                "V{} ran no script in {} ms (paused, idle or stopped at a breakpoint?): its heap is only read on its own thread; try: wait=10000, or `vms` for its eval count",
                id, wait_ms
            ))]
        }
    }
}

fn err(s: &str) -> String {
    format!("{{\"err\":{}}}", json_str(s))
}

// ------------------------------------------------------------------ values as text

/// A value as Splash would write it, short: `{id: 12, near: 2, teleport_lost: true}`,
/// `[3 items: {..}, ..]`; at most `depth` levels and about `budget` characters.
pub fn short_value(heap: &ScriptHeap, v: ScriptValue, depth: usize, budget: usize) -> String {
    let mut out = String::new();
    let mut left = budget;
    write_value(heap, v, depth, &mut left, &mut out, &mut Vec::new());
    out
}

fn write_value(heap: &ScriptHeap, v: ScriptValue, depth: usize, left: &mut usize, out: &mut String, seen: &mut Vec<ScriptObject>) {
    let start = out.len();
    if v.is_nil() {
        out.push_str("nil");
    } else if let Some(b) = v.as_bool() {
        let _ = write!(out, "{}", b);
    } else if v.is_number() {
        let x = v.as_number().unwrap_or(f64::NAN);
        if x.fract() == 0.0 && x.abs() < 1e15 {
            let _ = write!(out, "{}", x as i64);
        } else {
            let _ = write!(out, "{}", (x * 1e4).round() / 1e4);
        }
    } else if let Some(s) = heap.string_with(v, |_, s| s.chars().take(80).collect::<String>()) {
        out.push_str(&json_str(&s));
    } else if let Some(id) = v.as_id() {
        let _ = write!(out, "#{}", id);
    } else if let Some(c) = v.as_color() {
        let _ = write!(out, "#x{:08x}", c);
    } else if let Some(a) = v.as_array() {
        let n = heap.array_len(a);
        if depth == 0 {
            let _ = write!(out, "[{} items]", n);
        } else {
            out.push('[');
            for i in 0..n {
                if *left < out.len() - start || i >= 16 {
                    let _ = write!(out, "+{} more", n - i);
                    break;
                }
                if i > 0 {
                    out.push_str(", ");
                }
                let x = heap.array_index(a, i, NoTrap);
                write_value(heap, x, depth - 1, left, out, seen);
            }
            out.push(']');
        }
    } else if let Some(o) = v.as_object() {
        if heap.is_fn(o) {
            out.push_str("<fn>");
        } else if seen.contains(&o) {
            out.push_str("<recur>");
        } else {
            let n = heap.iter_len(o);
            if depth == 0 {
                let _ = write!(out, "{{{} fields}}", n);
            } else {
                seen.push(o);
                out.push('{');
                for i in 0..n {
                    if *left < out.len() - start || i >= 24 {
                        let _ = write!(out, "+{} more", n - i);
                        break;
                    }
                    let kv = heap.iter_key_value(o, i, NoTrap);
                    if kv.key.as_id() == Some(id!(self)) {
                        continue;
                    }
                    if i > 0 {
                        out.push_str(", ");
                    }
                    if let Some(k) = kv.key.as_id() {
                        let _ = write!(out, "{}: ", k);
                    } else if !kv.key.is_nil() {
                        write_value(heap, kv.key, 0, left, out, seen);
                        out.push_str(": ");
                    }
                    write_value(heap, kv.value, depth - 1, left, out, seen);
                }
                out.push('}');
                seen.pop();
            }
        }
    } else if v.is_handle() {
        out.push_str("<handle>");
    } else if v.is_err() {
        out.push_str("<error value>");
    } else {
        let _ = write!(out, "<{:?}>", v.value_type());
    }
    *left = left.saturating_sub(out.len() - start);
}

// ------------------------------------------------------------------ names and paths

/// A path `companions[0].near` as its root name and steps.
enum Step {
    Field(LiveId),
    Index(usize),
}

fn parse_path(p: &str) -> Result<(LiveId, Vec<Step>), String> {
    let mut steps = Vec::new();
    let b = p.trim();
    let root_end = b.find(|c: char| c == '.' || c == '[').unwrap_or(b.len());
    let root = &b[..root_end];
    if root.is_empty() {
        return Err(format!("`{}`: a path starts with a name (`companions[0].near`)", p));
    }
    let mut rest = &b[root_end..];
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('.') {
            let end = r.find(|c: char| c == '.' || c == '[').unwrap_or(r.len());
            steps.push(Step::Field(LiveId::from_str(&r[..end])));
            rest = &r[end..];
        } else if let Some(r) = rest.strip_prefix('[') {
            let end = r.find(']').ok_or_else(|| format!("`{}`: unclosed [", p))?;
            let k = r[..end].trim().parse::<usize>().map_err(|_| format!("`{}`: an index is a number", p))?;
            steps.push(Step::Index(k));
            rest = &r[end + 1..];
        } else {
            return Err(format!("`{}`: unexpected `{}`", p, rest));
        }
    }
    Ok((LiveId::from_str(root), steps))
}

/// The bodies of `base` that hold names (ran, not free), newest first, filtered by `file`.
fn scopes(base: &ScriptVmBase, file: Option<&str>) -> Vec<(u16, String, ScriptObject)> {
    let Some(bodies) = base.code.bodies.try_borrow() else { return Vec::new() };
    let mut out = Vec::new();
    for (i, b) in bodies.iter().enumerate().rev() {
        if matches!(b.source, ScriptSource::Free) || b.transient {
            continue;
        }
        let name = body_file(b);
        if file.is_some_and(|f| !name.contains(f)) {
            continue;
        }
        let obj = b.end_scope.as_ref().map(|r| r.obj).unwrap_or(b.scope.obj);
        out.push((i as u16, name, obj));
    }
    out
}

fn body_file(b: &crate::vm::ScriptBody) -> String {
    match (&b.shown, b.real_file.is_empty(), &b.source) {
        (Some(s), _, _) => s.clone(),
        (None, false, _) => b.real_file.clone(),
        (None, true, ScriptSource::Mod(m)) => m.file.clone(),
        _ => "streaming".into(),
    }
}

fn lookup(base: &ScriptVmBase, name: LiveId, file: Option<&str>) -> Option<(ScriptValue, String)> {
    let heap = &base.heap;
    for (_, f, obj) in scopes(base, file) {
        let v = heap.value(obj, name.into(), NoTrap);
        if !v.is_err() && !v.is_nil() {
            return Some((v, f));
        }
    }
    if let Some(v) = base.injected_globals.get(&name) {
        return Some((*v, "injected global".into()));
    }
    let v = heap.value(heap.modules, name.into(), NoTrap);
    (!v.is_err() && !v.is_nil()).then(|| (v, "module".into()))
}

fn walk(heap: &ScriptHeap, mut v: ScriptValue, steps: &[Step]) -> Result<ScriptValue, String> {
    for (k, s) in steps.iter().enumerate() {
        v = match s {
            Step::Index(i) => match (v.as_array(), v.as_object()) {
                (Some(a), _) => {
                    let n = heap.array_len(a);
                    if *i >= n {
                        return Err(format!("step {}: index {} out of range (len {})", k + 1, i, n));
                    }
                    heap.array_index(a, *i, NoTrap)
                }
                (_, Some(o)) => heap.vec_value(o, *i, NoTrap),
                _ => return Err(format!("step {}: not a list ({})", k + 1, short_value(heap, v, 0, 40))),
            },
            Step::Field(f) => match v.as_object() {
                Some(o) => heap.value(o, (*f).into(), NoTrap),
                None => return Err(format!("step {}: `{}` of a non-object ({})", k + 1, f, short_value(heap, v, 0, 40))),
            },
        };
        if v.is_err() {
            return Err(format!("step {}: no such field / item", k + 1));
        }
    }
    Ok(v)
}

/// The top-level names of one scope (its own map, not its protos).
fn names_of(heap: &ScriptHeap, obj: ScriptObject, cap: usize) -> Vec<(String, ScriptValue)> {
    let n = heap.iter_len(obj);
    let mut out = Vec::new();
    for i in 0..n.min(cap) {
        let kv = heap.iter_key_value(obj, i, NoTrap);
        if let Some(k) = kv.key.as_id() {
            out.push((k.to_string(), kv.value));
        }
    }
    out
}

// ------------------------------------------------------------------ source

fn src_job(file: Option<String>, lines: Option<(usize, usize)>, q: Option<String>) -> Job {
    Box::new(move |base: &ScriptVmBase| {
        let Some(bodies) = base.code.bodies.try_borrow() else { return vec![err("the VM's code is being edited; ask again")] };
        let mut out = Vec::new();
        let live: Vec<(usize, &crate::vm::ScriptBody)> = bodies.iter().enumerate().filter(|(_, b)| !matches!(b.source, ScriptSource::Free)).collect();
        let Some(file) = file else {
            // the list: newest first
            for (i, b) in live.iter().rev().take(40) {
                let text = b.code_text();
                out.push(format!("{{\"body\":\"B{}\",\"file\":{},\"lines\":{},\"chars\":{}{}}}", i, json_str(&body_file(b)), text.lines().count(), text.len(), if b.transient { ",\"transient\":true" } else { "" }));
            }
            if live.len() > 40 {
                out.push(format!("{{\"bodies_not_shown\":{}}}", live.len() - 40));
            }
            return out;
        };
        let pick = file.strip_prefix('B').and_then(|n| n.parse::<usize>().ok());
        let found: Vec<&(usize, &crate::vm::ScriptBody)> = live.iter().rev().filter(|(i, b)| pick.map_or_else(|| body_file(b).contains(file.as_str()), |p| p == *i)).collect();
        let Some((i, b)) = found.first() else {
            let names: Vec<String> = live.iter().rev().take(12).map(|(_, b)| body_file(b)).collect();
            return vec![err(&format!("no script body named like `{}`; newest: {}", file, names.join(", ")))];
        };
        let first = match &b.source {
            ScriptSource::Mod(m) => m.line.max(1),
            _ => 1,
        };
        let text = b.code_text();
        let all: Vec<&str> = text.lines().collect();
        let (lo, hi) = match lines {
            Some((a, z)) => (a.max(first), z.min(first + all.len().saturating_sub(1))),
            None => (first, first + all.len().saturating_sub(1)),
        };
        let mut shown = 0;
        out.push(format!("{{\"src\":{},\"body\":\"B{}\",\"lines\":{},\"from\":{},\"to\":{}{}}}", json_str(&body_file(b)), i, all.len(), lo, hi, if found.len() > 1 { format!(",\"also\":{}", found.len() - 1) } else { String::new() }));
        for (k, l) in all.iter().enumerate() {
            let n = k + first;
            if n < lo || n > hi {
                continue;
            }
            if q.as_deref().is_some_and(|q| !l.contains(q)) {
                continue;
            }
            if shown >= 400 {
                out.push("{\"lines_not_shown\":true,\"note\":\"give line=a-b or q=text\"}".into());
                break;
            }
            out.push(format!("{{\"n\":{},\"line\":{}}}", n, json_str(l)));
            shown += 1;
        }
        out
    })
}

// ------------------------------------------------------------------ verb trace

struct TraceCfg {
    /// the verbs traced (`follower`, `game.follower`); empty: all
    verbs: Vec<String>,
}

struct Call {
    ms: u64,
    vm: u32,
    verb: String,
    args: String,
    ret: String,
    at: String,
}

static TRACE_ON: AtomicBool = AtomicBool::new(false);
static TRACE: Mutex<Option<TraceCfg>> = Mutex::new(None);
static CALLS: Mutex<VecDeque<Call>> = Mutex::new(VecDeque::new());
/// per (verb, site): calls since `trace verbs on`
static SITES: Mutex<Vec<(String, String, u64)>> = Mutex::new(Vec::new());
const CALLS_KEPT: usize = 2048;

#[inline(always)]
pub(crate) fn trace_on() -> bool {
    TRACE_ON.load(Relaxed)
}

/// The host is about to run script->host call `type_name.method(args)` at the current ip:
/// Some(verb label) when it is traced.
#[cold]
pub(crate) fn trace_wants(type_name: &str, method: LiveId) -> Option<String> {
    let verb = format!("{}.{}", type_name, method);
    let cfg = TRACE.lock().ok()?;
    let cfg = cfg.as_ref()?;
    let m = method.to_string();
    (cfg.verbs.is_empty() || cfg.verbs.iter().any(|v| *v == verb || *v == m)).then_some(verb)
}

/// A traced call returned: its arguments and result, formatted on the VM's thread.
#[cold]
pub(crate) fn trace_record(base: &ScriptVmBase, verb: String, ip: ScriptIp, args: ScriptObject, ret: ScriptValue) {
    let heap = &base.heap;
    // positional arguments, the `self` key left out
    let mut a = String::new();
    let mut left = 600usize;
    let n = heap.iter_len(args);
    let mut first = true;
    for i in 0..n {
        let kv = heap.iter_key_value(args, i, NoTrap);
        if kv.key.as_id() == Some(id!(self)) {
            continue;
        }
        if !first {
            a.push_str(", ");
        }
        first = false;
        if let Some(k) = kv.key.as_id() {
            let _ = write!(a, "{}: ", k);
        }
        write_value(heap, kv.value, 3, &mut left, &mut a, &mut Vec::new());
    }
    let at = base.code.ip_to_loc(ip).map(|l| format!("{}:{}", l.file, l.line)).unwrap_or_default();
    let call = Call {
        ms: crate::inspect_registry::now_ms_pub(),
        vm: vm_of(base).unwrap_or(0),
        ret: short_value(heap, ret, 1, 120),
        verb: verb.clone(),
        args: a,
        at: at.clone(),
    };
    if let Ok(mut s) = SITES.lock() {
        match s.iter().position(|x| x.0 == verb && x.1 == at) {
            Some(k) => s[k].2 += 1,
            None if s.len() < 4096 => s.push((verb, at, 1)),
            None => {}
        }
    }
    if let Ok(mut c) = CALLS.lock() {
        if c.len() >= CALLS_KEPT {
            c.pop_front();
        }
        c.push_back(call);
    }
}

/// The name a handle type was registered under (`game`), for the trace's verb labels.
pub(crate) fn handle_type_name(native: &crate::native::ScriptNative, ty: ScriptTypeRedux) -> String {
    native.handle_type.iter().find(|(_, t)| t.to_redux() == ty).map(|(id, _)| id.to_string()).unwrap_or_else(|| "host".into())
}

fn trace_cmd(words: &[String], kv: &[(String, String)]) -> Vec<String> {
    let get = |k: &str| kv.iter().rev().find(|x| x.0 == k).map(|x| x.1.as_str());
    let verbs: Vec<String> = get("verb").or_else(|| get("verbs")).map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
    match words.get(1).map(String::as_str) {
        Some("on") => {
            *TRACE.lock().unwrap_or_else(|e| e.into_inner()) = Some(TraceCfg { verbs: verbs.clone() });
            CALLS.lock().unwrap_or_else(|e| e.into_inner()).clear();
            SITES.lock().unwrap_or_else(|e| e.into_inner()).clear();
            TRACE_ON.store(true, Relaxed);
            vec![format!("{{\"trace\":\"on\",\"verbs\":{},\"note\":\"every script->host call of {} is recorded with its arguments; `verbs` reads them, `verbs by=site` counts per line, `verbs off` stops\"}}", json_list(&verbs), if verbs.is_empty() { "every verb".to_string() } else { verbs.join(", ") })]
        }
        Some("off") => {
            TRACE_ON.store(false, Relaxed);
            *TRACE.lock().unwrap_or_else(|e| e.into_inner()) = None;
            let n = CALLS.lock().unwrap_or_else(|e| e.into_inner()).len();
            vec![format!("{{\"trace\":\"off\",\"kept\":{}}}", n)]
        }
        _ => {
            let n = get("n").and_then(|v| v.parse::<usize>().ok()).unwrap_or(20).clamp(1, CALLS_KEPT);
            let now = crate::inspect_registry::now_ms_pub();
            let calls = CALLS.lock().unwrap_or_else(|e| e.into_inner());
            let mut out = Vec::new();
            let on = TRACE_ON.load(Relaxed);
            let matching: Vec<&Call> = calls.iter().filter(|c| verbs.is_empty() || verbs.iter().any(|v| c.verb == *v || c.verb.ends_with(&format!(".{}", v)))).collect();
            out.push(format!("{{\"trace\":\"{}\",\"calls\":{},\"shown\":{}}}", if on { "on" } else { "off" }, matching.len(), matching.len().min(n)));
            if get("by") == Some("site") {
                let sites = SITES.lock().unwrap_or_else(|e| e.into_inner());
                let mut s: Vec<&(String, String, u64)> = sites.iter().filter(|x| verbs.is_empty() || verbs.iter().any(|v| x.0 == *v || x.0.ends_with(&format!(".{}", v)))).collect();
                s.sort_by(|a, b| b.2.cmp(&a.2));
                for x in s.iter().take(n) {
                    let last = matching.iter().rev().find(|c| c.verb == x.0 && c.at == x.1).map(|c| c.args.clone()).unwrap_or_default();
                    out.push(format!("{{\"site\":{},\"verb\":{},\"calls\":{},\"last_args\":{}}}", json_str(&x.1), json_str(&x.0), x.2, json_str(&last)));
                }
                return out;
            }
            for c in matching.iter().skip(matching.len().saturating_sub(n)) {
                out.push(format!(
                    "{{\"call\":{},\"vm\":\"V{}\",\"args\":{},\"ret\":{},\"at\":{},\"ago_ms\":{}}}",
                    json_str(&c.verb),
                    c.vm,
                    json_str(&c.args),
                    json_str(&c.ret),
                    json_str(&c.at),
                    now.saturating_sub(c.ms)
                ));
            }
            if !on && calls.is_empty() {
                out.push("{\"note\":\"no trace recorded; start one: verbs on [verb=follower,character]\"}".into());
            }
            out
        }
    }
}

fn json_list(v: &[String]) -> String {
    format!("[{}]", v.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","))
}


fn get_job(path: String, file: Option<String>, depth: usize) -> Job {
    Box::new(move |base: &ScriptVmBase| {
    let (root, steps) = match parse_path(&path) {
        Ok(x) => x,
        Err(e) => return vec![err(&e)],
    };
    let Some((v, from)) = lookup(base, root, file.as_deref()) else {
        let near: Vec<String> = scopes(base, file.as_deref()).into_iter().take(3).flat_map(|(_, _, o)| names_of(&base.heap, o, 64).into_iter().map(|x| x.0)).take(24).collect();
        return vec![err(&format!("no name `{}` in V{}'s scripts; some names: {}", root, vm_of(base).unwrap_or(0), near.join(", ")))];
    };
    match walk(&base.heap, v, &steps) {
        Ok(v) => {
            let mut rows = vec![format!("{{\"path\":{},\"from\":{},\"value\":{}}}", json_str(&path), json_str(&from), json_str(&short_value(&base.heap, v, depth, 4000)))];
            // a list of objects: one row per item, so a table can show them
            if let Some(a) = v.as_array() {
                for i in 0..base.heap.array_len(a).min(50) {
                    let x = base.heap.array_index(a, i, NoTrap);
                    rows.push(format!("{{\"item\":{},\"value\":{}}}", i, json_str(&short_value(&base.heap, x, depth.saturating_sub(1).max(1), 400))));
                }
            }
            rows
        }
        Err(e) => vec![err(&format!("{}: {}", path, e))],
    }
})
}

// ------------------------------------------------------------------ the commands

/// The commands this module answers (first word), for the host's command table.
// ------------------------------------------------------------------ line probes

/// `probe add game.splash:76 [dist,config.near]`: every time a VM starts running line 76 of a
/// file ending in game.splash, the values of the names (or, without names, the frame's locals and
/// the innermost scope's own names) are recorded, newest kept; `probe read SP<n>` shows them as
/// the Rust probes' table. The VM loop pays one relaxed load per instruction while no line probe
/// exists; while one does, each instruction checks the (thread-cached, sorted) target ips.
struct LineProbe {
    id: u32,
    file: String,
    line: u32,
    names: Vec<String>,
    on: bool,
    hits: u64,
    /// (ms, vm, line it ran at, (name, value) pairs), oldest first
    recent: VecDeque<(u64, u32, Vec<(String, String)>)>,
}

const PROBE_KEPT: usize = 256;
static LINE_PROBES_ON: AtomicBool = AtomicBool::new(false);
static LINE_PROBES: Mutex<Vec<LineProbe>> = Mutex::new(Vec::new());
static LINE_PROBE_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static NEXT_LINE_PROBE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// One VM thread's view of the probe targets: (base, bodies epoch, probe generation) it was
/// computed for, the bodies holding a target, and the sorted (body, ip, probe id) targets.
#[derive(Default)]
struct Targets {
    base: usize,
    epoch: u64,
    generation: u64,
    bodies: Vec<u16>,
    at: Vec<(u16, u32, u32)>,
}

thread_local! {
    static TARGETS: std::cell::RefCell<Targets> = std::cell::RefCell::new(Targets::default());
}

#[inline(always)]
pub(crate) fn line_probes_on() -> bool {
    LINE_PROBES_ON.load(Relaxed)
}

/// The VM loop is about to run `ip` of `body` while a line probe exists.
#[inline(never)]
pub(crate) fn line_probe_check(base: &ScriptVmBase, body: usize, ip: usize) {
    let hit: Vec<u32> = TARGETS
        .try_with(|t| {
            let Ok(mut t) = t.try_borrow_mut() else { return Vec::new() };
            let key = (base as *const ScriptVmBase as usize, base.code.bodies.epoch(), LINE_PROBE_GEN.load(Relaxed));
            if (t.base, t.epoch, t.generation) != key {
                *t = targets_of(base);
                (t.base, t.epoch, t.generation) = key;
            }
            if !t.bodies.contains(&(body as u16)) {
                return Vec::new();
            }
            let k = t.at.partition_point(|x| (x.0, x.1) < (body as u16, ip as u32));
            t.at[k..].iter().take_while(|x| x.0 == body as u16 && x.1 == ip as u32).map(|x| x.2).collect()
        })
        .unwrap_or_default();
    for id in hit {
        record_line_probe(base, id, body, ip);
    }
}

/// Where the probes' lines start in `base`'s bodies: the first ip of each run of ips on the line.
fn targets_of(base: &ScriptVmBase) -> Targets {
    let mut t = Targets::default();
    let probes: Vec<(u32, String, u32)> = match LINE_PROBES.lock() {
        Ok(p) => p.iter().filter(|p| p.on).map(|p| (p.id, p.file.clone(), p.line)).collect(),
        Err(_) => return t,
    };
    let Some(bodies) = base.code.bodies.try_borrow() else { return t };
    for (bi, b) in bodies.iter().enumerate() {
        if matches!(b.source, ScriptSource::Free) {
            continue;
        }
        let name = body_file(b);
        let mine: Vec<&(u32, String, u32)> = probes.iter().filter(|p| name.ends_with(p.1.as_str()) || p.1.ends_with(name.as_str()) && !name.is_empty()).collect();
        if mine.is_empty() {
            continue;
        }
        let n = b.parser.opcodes.len();
        let mut prev = u32::MAX;
        let mut lines = Vec::with_capacity(n);
        for ip in 0..n {
            // (a second shared borrow of the bodies: ip_to_loc reads them too)
            let l = base.code.ip_to_loc_plain(ScriptIp { body: bi as u16, index: ip as u32 }).map_or(0, |l| l.line);
            lines.push(l);
        }
        for ip in 0..n {
            let l = lines[ip];
            if l != prev {
                for p in &mine {
                    if p.2 == l {
                        t.at.push((bi as u16, ip as u32, p.0));
                        if !t.bodies.contains(&(bi as u16)) {
                            t.bodies.push(bi as u16);
                        }
                    }
                }
            }
            prev = l;
        }
    }
    t.at.sort_unstable();
    t
}

/// The frame's names at a probe hit: the slot-compiled locals (their names from the body's slot
/// table: the nearest SLOTS_FRAME before `ip`, when the slab's top frame has its size), then the
/// innermost scope's own names.
fn frame_names(base: &ScriptVmBase, body: usize, ip: usize, cap: usize) -> Vec<(String, ScriptValue)> {
    let mut out = Vec::new();
    let th = base.threads.cur_ref();
    if let Some(bodies) = base.code.bodies.try_borrow() {
        if let Some(b) = bodies.get(body) {
            let frame = b.parser.slot_frames.iter().filter(|f| f.0 as usize <= ip).max_by_key(|f| f.0);
            if let Some((_, names)) = frame {
                if th.slot_base + names.len() == th.slots.len() {
                    for (i, n) in names.iter().enumerate() {
                        out.push((n.to_string(), th.slots[th.slot_base + i]));
                    }
                }
            }
        }
    }
    if let Some(&scope) = th.scopes.last() {
        for (k, v) in names_of(&base.heap, scope, cap) {
            if !out.iter().any(|x| x.0 == k) {
                out.push((k, v));
            }
        }
    }
    out.truncate(cap);
    out
}

fn record_line_probe(base: &ScriptVmBase, id: u32, body: usize, ip: usize) {
    let names: Vec<String> = match LINE_PROBES.lock() {
        Ok(p) => match p.iter().find(|p| p.id == id && p.on) {
            Some(p) => p.names.clone(),
            None => return,
        },
        Err(_) => return,
    };
    let heap = &base.heap;
    let frame = frame_names(base, body, ip, 24);
    let vals: Vec<(String, String)> = if names.is_empty() {
        frame.iter().map(|(k, v)| (k.clone(), short_value(heap, *v, 1, 80))).collect()
    } else {
        names
            .iter()
            .map(|n| {
                let v = match parse_path(n) {
                    Ok((root, steps)) => {
                        let rs = root.to_string();
                        let v0 = frame.iter().find(|x| x.0 == rs).map(|x| x.1).or_else(|| {
                            let th = base.threads.cur_ref();
                            th.scopes.last().map(|&s| heap.value(s, root.into(), NoTrap)).filter(|v| !v.is_err() && !v.is_nil())
                        }).or_else(|| lookup(base, root, None).map(|x| x.0));
                        match v0 {
                            Some(v) => match walk(heap, v, &steps) {
                                Ok(v) => short_value(heap, v, 1, 80),
                                Err(e) => format!("({})", e),
                            },
                            None => "(not in scope)".to_string(),
                        }
                    }
                    Err(e) => format!("({})", e),
                };
                (n.clone(), v)
            })
            .collect()
    };
    let vm = vm_of(base).unwrap_or(0);
    if let Ok(mut p) = LINE_PROBES.lock() {
        if let Some(p) = p.iter_mut().find(|p| p.id == id) {
            p.hits += 1;
            if p.recent.len() >= PROBE_KEPT {
                p.recent.pop_front();
            }
            p.recent.push_back((crate::inspect_registry::now_ms_pub(), vm, vals));
        }
    }
}

fn line_probe_cmd(words: &[String], kv: &[(String, String)]) -> Vec<String> {
    let get = |k: &str| kv.iter().rev().find(|x| x.0 == k).map(|x| x.1.as_str());
    let set_on = |p: &Vec<LineProbe>| {
        LINE_PROBES_ON.store(p.iter().any(|x| x.on), Relaxed);
        LINE_PROBE_GEN.fetch_add(1, Relaxed);
    };
    match words.get(1).map(String::as_str).unwrap_or("list") {
        "add" => {
            let Some((file, line)) = words.get(2).and_then(|w| w.rsplit_once(':')).and_then(|(f, l)| l.parse::<u32>().ok().map(|l| (f.to_string(), l))) else {
                return vec![err("usage: probe add <file>.splash:<line> [name,path.field,...]")];
            };
            // the inspect client sends what follows the line as `code` (the Rust probe's code)
            let named = get("code").map(str::to_string).unwrap_or_else(|| words[3..].join(" "));
            let names: Vec<String> = named.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            let id = NEXT_LINE_PROBE.fetch_add(1, Relaxed);
            let Ok(mut p) = LINE_PROBES.lock() else { return vec![err("probe table poisoned")] };
            p.push(LineProbe { id, file: file.clone(), line, names: names.clone(), on: true, hits: 0, recent: VecDeque::new() });
            set_on(&p);
            vec![format!(
                "{{\"probe\":\"SP{}\",\"at\":{},\"records\":{},\"note\":\"records each time a VM starts running the line (before it runs); no hit yet means the line has not run since\",\"more\":[\"probe read SP{}\",\"probe rm SP{}\",\"src {} line={}\"]}}",
                id,
                json_str(&format!("{}:{}", file, line)),
                json_str(&if names.is_empty() { "the frame's locals and the innermost scope's names".to_string() } else { names.join(", ") }),
                id,
                id,
                json_str(&file),
                line
            )]
        }
        "read" => {
            let Some(id) = words.get(2).and_then(|w| w.trim_start_matches("SP").parse::<u32>().ok()) else { return vec![err("usage: probe read SP<n>")] };
            let n = get("n").and_then(|v| v.parse::<usize>().ok()).unwrap_or(12).clamp(1, PROBE_KEPT);
            let Ok(p) = LINE_PROBES.lock() else { return vec![err("probe table poisoned")] };
            let Some(p) = p.iter().find(|p| p.id == id) else { return vec![err(&format!("no Splash probe SP{}", id))] };
            let now = crate::inspect_registry::now_ms_pub();
            // the Rust probes' answer shape: the client renders both alike
            let mut out = vec![format!("{{\"measured\":{},\"at_ms\":{},\"source\":\"Splash line probe\",\"partial\":[]}}", json_str(&format!("probe SP{} at {}:{} ({} hits)", p.id, p.file, p.line, p.hits)), now)];
            let skip = p.recent.len().saturating_sub(n);
            for (t, vm, vals) in p.recent.iter().skip(skip) {
                let all = vals.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join(", ");
                let fields = vals.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(v))).collect::<Vec<_>>().join(",");
                out.push(format!("{{\"emit\":{},\"ago_ms\":{},\"line\":{},\"vm\":\"V{}\",\"fields\":{{{}}}}}", json_str(&all), now.saturating_sub(*t), p.line, vm, fields));
            }
            if skip > 0 {
                out.push(format!("{{\"emits_not_shown\":{},\"kept\":{}}}", skip, p.recent.len()));
            }
            out.push(format!("{{\"emitted\":{},\"emit_dropped\":0}}", p.hits));
            out
        }
        "rm" => {
            let w = words.get(2).map(String::as_str).unwrap_or("all");
            let id = if w == "all" { None } else { w.trim_start_matches("SP").parse::<u32>().ok() };
            let Ok(mut p) = LINE_PROBES.lock() else { return vec![err("probe table poisoned")] };
            let before = p.len();
            p.retain(|x| id.is_some_and(|id| x.id != id));
            let n = before - p.len();
            set_on(&p);
            vec![format!("{{\"removed\":{}}}", n)]
        }
        _ => {
            let Ok(p) = LINE_PROBES.lock() else { return vec![err("probe table poisoned")] };
            p.iter().map(|p| format!("{{\"probe\":\"SP{}\",\"at\":{},\"on\":{},\"hits\":{}}}", p.id, json_str(&format!("{}:{}", p.file, p.line)), p.on, p.hits)).collect()
        }
    }
}

pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("src", "[<file part>|B<n>] [line=a-b|n] [q=text] [vm=V<n>]", "the running Splash source of a VM with line numbers (no file: its script bodies, newest first); line=76 shows 66..86"),
    ("vm", "globals [in=<file part>] [vm=V<n>] | get <path> [in=<file part>] [depth=3] [vm=V<n>]", "a live Splash VM's top-level names with short values, or one value by path (`companions[0].near`): the names of the newest script bodies, then injected globals, then modules; read on the VM's own thread"),
    ("verbs", "on [verb=a,b] | off | [verb=a] [n=20] [by=site]", "script->host call trace: every `game.<verb>(..)` call with its arguments as written values, its result and its script line; by=site counts calls per verb and line"),
    ("script", "errors [vm=V<n>]", "every VM's script error and host diagnostic sites (wrong-typed options, verbs on missing entities): count, script line, last seen"),
    ("sprobe", "add <file>.splash:<line> [name,path.field] | read SP<n> [n=12] | rm <SP<n>|all> | list", "Splash line probes (what `probe add game.splash:76 ..` uses): each run of the line records the names' values (default: the frame's locals), on the VM's own thread"),
];

/// Answers `words` (the command first) with `kv`; None when the command is not one of ours.
pub fn query(words: &[String], kv: &[(String, String)]) -> Option<Vec<String>> {
    let get = |k: &str| kv.iter().rev().find(|x| x.0 == k).map(|x| x.1.clone());
    let vm_arg = get("vm").and_then(|v| v.trim_start_matches('V').parse::<u32>().ok());
    let wait = get("wait").and_then(|v| v.parse::<u64>().ok()).unwrap_or(3000).clamp(100, 60_000);
    let cmd = words.first()?.as_str();
    // without vm=: every VM is asked in turn (newest first) until one answers without an error
    let search = vm_arg.is_none();
    let all_vms: Vec<u32> = {
        let mut v: Vec<u32> = crate::inspect_registry::vm_ids().into_iter().map(|x| x.0).collect();
        v.reverse();
        v
    };
    let first_ok = |make: &dyn Fn() -> Job| -> Vec<String> {
        let mut last = vec![err("no live Splash VM")];
        for &id in &all_vms {
            let mut out = vec![format!("{{\"vm\":\"V{}\"}}", id)];
            out.extend(run_on(id, wait, make()));
            if !out.iter().skip(1).any(|l| l.starts_with("{\"err\":")) {
                return out;
            }
            last = out;
        }
        last
    };
    let pick = || -> Result<u32, String> {
        let list = crate::inspect_registry::vm_ids();
        match vm_arg {
            Some(v) if list.iter().any(|x| x.0 == v) => Ok(v),
            Some(v) => Err(format!("no VM V{} (have {})", v, list.iter().map(|x| format!("V{} {}", x.0, x.1)).collect::<Vec<_>>().join(", "))),
            // the VM that evaluated most: the app's (a game shares it or has its own)
            None => list.iter().max_by_key(|x| x.2).map(|x| x.0).ok_or_else(|| "no live Splash VM".to_string()),
        }
    };
    Some(match cmd {
        "script" => match words.get(1).map(String::as_str) {
            Some("errors") | None => {
                let rows = crate::inspect_registry::script_errors(vm_arg);
                if rows.is_empty() {
                    vec!["{\"script_errors\":0,\"note\":\"no script error or host diagnostic recorded since the VMs started (while inspect is attached)\"}".into()]
                } else {
                    let mut v = vec![format!("{{\"script_errors\":{}}}", rows.len())];
                    v.extend(rows);
                    v
                }
            }
            _ => vec![err("usage: script errors [vm=V<n>]")],
        },
        "verbs" => trace_cmd(words, kv),
        "sprobe" => line_probe_cmd(words, kv),
        "src" => {
            let lines = get("line").and_then(|l| match l.split_once('-') {
                Some((a, b)) => Some((a.trim().parse().ok()?, b.trim().parse().ok()?)),
                None => l.trim().parse::<usize>().ok().map(|n| (n.saturating_sub(10), n + 10)),
            });
            let (file, q) = (words.get(1).cloned(), get("q"));
            if search && file.is_some() {
                return Some(first_ok(&|| src_job(file.clone(), lines, q.clone())));
            }
            let id = match pick() {
                Ok(id) => id,
                Err(e) => return Some(vec![err(&e)]),
            };
            let mut out = vec![format!("{{\"vm\":\"V{}\"}}", id)];
            out.extend(run_on(id, wait, src_job(file, lines, q)));
            out
        }
        "vm" => {
            let id = match pick() {
                Ok(id) => id,
                Err(e) => return Some(vec![err(&e)]),
            };
            // (a search asks every VM; `id` is the default for the rest)
            let file = get("in");
            match words.get(1).map(String::as_str) {
                Some("get") => {
                    let Some(path) = words.get(2).cloned() else { return Some(vec![err("usage: vm get <path> (`companions`, `companions[0].near`)")]) };
                    let depth = get("depth").and_then(|v| v.parse::<usize>().ok()).unwrap_or(3).clamp(0, 8);
                    if search {
                        return Some(first_ok(&|| get_job(path.clone(), file.clone(), depth)));
                    }
                    let mut out = vec![format!("{{\"vm\":\"V{}\"}}", id)];
                    out.extend(run_on(id, wait, get_job(path, file, depth)));
                    out
                }
                Some("globals") | None => {
                    let mut out = vec![format!("{{\"vm\":\"V{}\"}}", id)];
                    out.extend(run_on(id, wait, Box::new(move |base: &ScriptVmBase| {
                        let mut rows = Vec::new();
                        for (b, f, o) in scopes(base, file.as_deref()).into_iter().take(if file.is_some() { 1 } else { 2 }) {
                            let names = names_of(&base.heap, o, 400);
                            rows.push(format!("{{\"scope\":{},\"body\":\"B{}\",\"names\":{}}}", json_str(&f), b, names.len()));
                            for (k, v) in names {
                                rows.push(format!("{{\"name\":{},\"value\":{}}}", json_str(&k), json_str(&short_value(&base.heap, v, 1, 160))));
                            }
                        }
                        if rows.is_empty() {
                            rows.push(err("no script body has run in this VM (in=<file part> filters)"));
                        }
                        rows
                    })));
                    out
                }
                _ => vec![err("usage: vm globals [in=<file>] | vm get <path> [in=<file>] [depth=3]")],
            }
        }
        _ => return None,
    })
}
