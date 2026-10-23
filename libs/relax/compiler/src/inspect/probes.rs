//! Injected Rust probes (DESIGN.md §10a): an agent's Rust fn runs at a hook point of a running
//! program. The probe is the agent's own text, compiled as a new private child module of the
//! hooked fn's module (modules.rs load_in_module: front, type check, compile): the module's
//! private items, its `use`s and `crate::` paths are the probe's, as for any child module with
//! `use super::*`; it is not spliced into anything. Its `probe` fn takes the hooked fn's
//! parameters (same order, types modulo lifetimes; a non-Copy by-value parameter is declared
//! `ManuallyDrop<T>`: the same ABI and no drop),
//! so the instrumented build of the hooked slot calls it with the slot's own parameter values.
//! The call is guarded (a disabled probe is skipped) and an invoke whose unwind edge disables
//! the probe and continues: a panicking probe never unwinds into the program.
//!
//! In the probe, `std::relax::probe::{emit, count, hist, sample, pause}` record for that probe.

use super::{json_str, now_ms, short, Out, Req};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::{Relaxed, SeqCst}};
use std::sync::Arc;
use relax_session::sync::Mutex;

include!("../rt_abi.rs");

#[derive(Default)]
struct Hist {
    n: u64,
    min: f64,
    max: f64,
    sum: f64,
    /// log2 buckets of |x| (index 0: |x| < 1)
    buckets: [u64; 32],
}

#[derive(Default)]
struct Aggregates {
    counts: HashMap<String, u64>,
    hists: HashMap<String, Hist>,
    /// key -> (seen, kept values)
    samples: HashMap<String, (u64, Vec<String>)>,
    emitted: u64,
    emit_dropped: u64,
    /// the latest emits, oldest first: (ms, the emit's line in the probe, the value's Debug form)
    recent: std::collections::VecDeque<(u64, u32, String)>,
    /// emits in the current second (rate limit) and that second
    emit_window: (u64, u64),
}

pub(super) struct Probe {
    pub(super) id: u32,
    hook: String,
    /// the slots it is hooked into (empty: removed)
    slots: Mutex<Vec<u32>>,
    on: AtomicBool,
    hits: AtomicU64,
    /// ns spent in the probe over the timed hits (1 in 64)
    ns: AtomicU64,
    timed: AtomicU64,
    failed: Mutex<Option<String>>,
    agg: Mutex<Aggregates>,
    /// the probe's `emit(..)` calls: (line, the names of the emitted expression's elements)
    emits: Vec<(u32, Vec<String>)>,
}

/// How many emitted values a probe keeps for `probe read`.
const RECENT: usize = 256;

static PROBES: Mutex<Vec<Arc<Probe>>> = Mutex::new(Vec::new());
static NEXT: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// the probe running on this thread (index into PROBES) and its timed start (0: untimed)
    static CURRENT: std::cell::Cell<(usize, u64)> = const { std::cell::Cell::new((usize::MAX, 0)) };
}

/// Is a probe running on this thread? (its panics are the probe's, handled at its landing)
pub fn in_probe() -> bool {
    CURRENT.try_with(|c| c.get().0 != usize::MAX).unwrap_or(false)
}

fn probe_at(i: u64) -> Option<Arc<Probe>> {
    PROBES.lock().unwrap_or_else(|e| e.into_inner()).get(i as usize).cloned()
}

fn current() -> Option<Arc<Probe>> {
    let (i, _) = CURRENT.try_with(|c| c.get()).ok()?;
    if i == usize::MAX {
        return None;
    }
    probe_at(i as u64)
}

rt_fn! {
/// Before the probe call: 1 run it (this thread now runs probe `i`), 0 skip (disabled).
fn rt_probe_enter(i: u64) -> u64 {
    let Some(p) = probe_at(i) else { return 0 };
    if !p.on.load(Relaxed) {
        return 0;
    }
    let n = p.hits.fetch_add(1, Relaxed);
    let t = if n % 64 == 0 { crate::clock::wall_ns().max(1) } else { 0 };
    let _ = CURRENT.try_with(|c| c.set((i as usize, t)));
    1
}
}

rt_fn! {
fn rt_probe_leave() {
    let Ok((i, t)) = CURRENT.try_with(|c| c.replace((usize::MAX, 0))) else { return };
    if t != 0 {
        if let Some(p) = probe_at(i as u64) {
            p.ns.fetch_add(crate::clock::wall_ns().saturating_sub(t), Relaxed);
            p.timed.fetch_add(1, Relaxed);
        }
    }
}
}

rt_fn! {
/// The probe's unwind landing: the probe panicked. It is disabled, the panic ends here and the
/// hooked fn goes on.
fn rt_probe_failed(i: u64) {
    let _ = CURRENT.try_with(|c| c.set((usize::MAX, 0)));
    let msg = crate::jit::take_panic_message().unwrap_or_else(|| "panic".into());
    crate::jit::abandon_unwind();
    if let Some(p) = probe_at(i) {
        p.on.store(false, SeqCst);
        *p.failed.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg.clone());
        super::events::emit("probe", &format!("P{}|failed", p.id), &format!("\"probe\":\"P{}\",\"failed\":{},\"note\":\"the probe is disabled; the program went on\"", p.id, json_str(&short(&msg, 300))));
    }
}
}

// ------------------------------------------------------------------ probe:: entries (Relax std)

fn text<'a>(p: *const u8, n: usize) -> &'a str {
    unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(p, n)) }
}

pub extern "C" fn relax_probe_emit(at: u32, t: *const u8, n: usize) {
    let _k = crate::zone_alloc::kept();
    let Some(p) = current() else { return };
    let line = short(text(t, n), 1000);
    let send = {
        let mut a = p.agg.lock().unwrap_or_else(|e| e.into_inner());
        if a.recent.len() >= RECENT {
            a.recent.pop_front();
        }
        a.recent.push_back((now_ms(), at, line.clone()));
        let sec = now_ms() / 1000;
        if a.emit_window.1 != sec {
            a.emit_window = (0, sec);
        }
        a.emit_window.0 += 1;
        if a.emit_window.0 > 20 {
            a.emit_dropped += 1;
            false
        } else {
            a.emitted += 1;
            true
        }
    };
    if send {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Relaxed);
        super::events::emit("probe", &format!("P{}|{}", p.id, seq), &format!("\"probe\":\"P{}\",\"value\":{}{}", p.id, json_str(&line), named_fields(&p, at, &line)));
    }
}

pub extern "C" fn relax_probe_count(k: *const u8, n: usize) {
    let Some(p) = current() else { return };
    let _k = crate::zone_alloc::kept();
    *p.agg.lock().unwrap_or_else(|e| e.into_inner()).counts.entry(text(k, n).to_string()).or_default() += 1;
}

pub extern "C" fn relax_probe_hist(k: *const u8, n: usize, x: f64) {
    let _k = crate::zone_alloc::kept();
    let Some(p) = current() else { return };
    let mut a = p.agg.lock().unwrap_or_else(|e| e.into_inner());
    let h = a.hists.entry(text(k, n).to_string()).or_default();
    if h.n == 0 || x < h.min {
        h.min = x;
    }
    if h.n == 0 || x > h.max {
        h.max = x;
    }
    h.n += 1;
    h.sum += x;
    let b = if x.abs() < 1.0 { 0 } else { (x.abs().log2() as usize + 1).min(31) };
    h.buckets[b] += 1;
}

pub extern "C" fn relax_probe_sample(k: *const u8, n: usize, t: *const u8, tn: usize, keep: usize) {
    let _k = crate::zone_alloc::kept();
    let Some(p) = current() else { return };
    let mut a = p.agg.lock().unwrap_or_else(|e| e.into_inner());
    let e = a.samples.entry(text(k, n).to_string()).or_default();
    e.0 += 1;
    let keep = keep.clamp(1, 256);
    let v = short(text(t, tn), 300);
    if e.1.len() < keep {
        e.1.push(v);
    } else {
        // reservoir: replace with probability keep / seen
        let r = (e.0.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 11) % e.0;
        if (r as usize) < keep {
            e.1[r as usize] = v;
        }
    }
}

pub extern "C" fn relax_probe_pause() {
    let Some(p) = current() else { return };
    let saved = CURRENT.try_with(|c| c.get()).unwrap_or((usize::MAX, 0));
    super::instrument::park_here(u64::MAX, &format!("P{}", p.id), &format!("probe::pause in P{} at {}", p.id, p.hook), "");
    let _ = CURRENT.try_with(|c| c.set(saved));
}


// ------------------------------------------------------------------ emitted values, named

/// The `emit(..)` calls of a probe's source: (line, the names of the emitted value's elements --
/// the elements of a tuple expression `emit((a, b.c))` are `a`, `b.c`; any other expression is
/// one name). Read from the agent's own text (comments and literals skipped), never rewritten.
fn emit_exprs(src: &str) -> Vec<(u32, Vec<String>)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut line) = (0usize, 1u32);
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    while i < b.len() {
        match b[i] {
            b'\n' => line += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    } else if b[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
            b'e' if src[i..].starts_with("emit") && (i == 0 || !ident(b[i - 1])) => {
                let mut j = i + 4;
                while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                    j += 1;
                }
                if b.get(j) == Some(&b'(') {
                    if let Some(end) = close_of(b, j) {
                        let inner = src[j + 1..end].trim();
                        let names = match inner.strip_prefix('(').filter(|_| close_of(inner.as_bytes(), 0) == Some(inner.len() - 1)) {
                            Some(t) => split_top(&t[..t.len() - 1]),
                            None => vec![inner.to_string()],
                        };
                        out.push((line, names.into_iter().map(|n| n.split_whitespace().collect::<Vec<_>>().join(" ")).collect()));
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// The index of the bracket closing the one at `open` (string literals skipped).
fn close_of(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut k = open;
    while k < b.len() {
        match b[k] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            b'"' => {
                k += 1;
                while k < b.len() && b[k] != b'"' {
                    if b[k] == b'\\' {
                        k += 1;
                    }
                    k += 1;
                }
            }
            _ => {}
        }
        k += 1;
    }
    None
}

/// `a, f(b, c), [d]` as its top-level elements (a trailing comma gives none).
fn split_top(t: &str) -> Vec<String> {
    let b = t.as_bytes();
    let (mut out, mut depth, mut start, mut k) = (Vec::new(), 0i32, 0usize, 0usize);
    while k < b.len() {
        match b[k] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'<' if depth == 0 && k > 0 && b[k - 1] == b':' => depth += 1,
            b'"' => {
                k += 1;
                while k < b.len() && b[k] != b'"' {
                    if b[k] == b'\\' {
                        k += 1;
                    }
                    k += 1;
                }
            }
            b',' if depth == 0 => {
                out.push(t[start..k].trim().to_string());
                start = k + 1;
            }
            _ => {}
        }
        k += 1;
    }
    if !t[start..].trim().is_empty() {
        out.push(t[start..].trim().to_string());
    }
    out
}

/// The emitted value's elements under the emit expression's names: `,"fields":{"dist":"2.12",..}`
/// when the emit at `line` named as many elements as the value has, else an empty string.
fn named_fields(p: &Probe, line: u32, value: &str) -> String {
    match fields_of(p, line, value) {
        Some(f) => format!(",\"fields\":{{{}}}", f.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(v))).collect::<Vec<_>>().join(",")),
        None => String::new(),
    }
}

fn fields_of(p: &Probe, line: u32, value: &str) -> Option<Vec<(String, String)>> {
    let names = &p.emits.iter().find(|e| e.0 == line)?.1;
    let v = value.trim();
    let parts = if names.len() > 1 {
        let inner = v.strip_prefix('(')?.strip_suffix(')')?;
        split_top(inner)
    } else {
        vec![v.to_string()]
    };
    (parts.len() == names.len()).then(|| names.iter().cloned().zip(parts).collect())
}

// ------------------------------------------------------------------ adding a probe

/// The probe call at the hook (entry: line None) with the hooked fn's own parameters.
pub(super) fn edit(f: &mut relax_backend::rir::Func, u: &crate::jit::Unit, idx: u64, probe_slot: u32, line: Option<(u32, u32)>) -> bool {
    // where: block b before instruction k
    let (b, k) = match line {
        None => (0usize, 0usize),
        Some(l) => {
            let mut at = None;
            'f: for b in 0..f.blocks.len() {
                for k in 0..f.blocks[b].insts.len() {
                    if super::instrument::on_line(u, f.blocks[b].pos[k], l) {
                        at = Some((b, k));
                        break 'f;
                    }
                }
            }
            match at {
                Some(x) => x,
                None => return false,
            }
        }
    };
    let args: Vec<relax_backend::rir::VReg> = f.params.iter().copied().filter(|v| Some(*v) != f.sret).collect();
    place_call(f, u, idx, probe_slot, b, k, args);
    true
}

/// A by-name probe's calls, lowered into the body already (lower.rs ProbePoint): each becomes the
/// guarded invoke.
pub(super) fn wrap_calls(f: &mut relax_backend::rir::Func, u: &crate::jit::Unit, idx: u64, probe_slot: u32) -> bool {
    use relax_backend::rir::{Callee, Inst, Term};
    let mut found = false;
    let mut b = 0;
    while b < f.blocks.len() {
        let at = f.blocks[b].insts.iter().position(|i| matches!(i, Inst::Call(Callee::Fn(s), _, r) if *s == probe_slot && r.is_empty()));
        if let Some(k) = at {
            // a call already made an invoke by an earlier wrap is the last instruction of a block
            // ending in Invoke: skip it
            let last = k + 1 == f.blocks[b].insts.len();
            if !(last && matches!(f.blocks[b].term, Term::Invoke(..))) {
                let Inst::Call(_, args, _) = f.blocks[b].insts.remove(k) else { unreachable!() };
                f.blocks[b].pos.remove(k);
                place_call(f, u, idx, probe_slot, b, k, args);
                found = true;
                continue;
            }
        }
        b += 1;
    }
    found
}

/// In block `b` before instruction `k`: `enter` guard, the probe as an invoke with `args`, `leave`
/// on the normal path, `failed` on the unwind path; both go on with the rest of the block.
fn place_call(f: &mut relax_backend::rir::Func, u: &crate::jit::Unit, idx: u64, probe_slot: u32, b: usize, k: usize, args: Vec<relax_backend::rir::VReg>) {
    use relax_backend::rir::{Callee, Cls, Inst, Term};
    let pos = f.blocks[b].pos.get(k).copied().unwrap_or(f.blocks[b].term_pos);
    let sym = |name: &'static str, a: u64| u.syms.intern(crate::syms::SymKind::Rt(name), a);
    let (enter, leave, failed) = (
        sym("probe_enter", rt_probe_enter as *const () as u64),
        sym("probe_leave", rt_probe_leave as *const () as u64),
        sym("probe_failed", rt_probe_failed as *const () as u64),
    );
    // the rest of block b moves to `rest`
    let rest = f.block();
    let ri = f.blocks[b].insts.split_off(k);
    let rp = f.blocks[b].pos.split_off(k);
    let rt = std::mem::replace(&mut f.blocks[b].term, Term::Unreachable);
    f.blocks[rest as usize].insts = ri;
    f.blocks[rest as usize].pos = rp;
    f.blocks[rest as usize].term = rt;
    f.blocks[rest as usize].term_pos = f.blocks[b].term_pos;
    let call = f.block();
    let after = f.block();
    let pad = f.block();
    let (iv, d) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.blocks[b].insts.push(Inst::Iconst(iv, idx as i64));
    f.blocks[b].insts.push(Inst::Call(Callee::Host(enter), vec![iv], vec![d]));
    f.blocks[b].pos.extend([pos, pos]);
    f.blocks[b].term = Term::Branch(d, call, rest);
    f.blocks[call as usize].insts.push(Inst::Call(Callee::Fn(probe_slot), args, Vec::new()));
    f.blocks[call as usize].pos.push(pos);
    f.blocks[call as usize].term = Term::Invoke(after, pad);
    f.blocks[after as usize].insts.push(Inst::Call(Callee::Host(leave), Vec::new(), Vec::new()));
    f.blocks[after as usize].pos.push(pos);
    f.blocks[after as usize].term = Term::Jump(rest);
    let iv2 = f.vreg(Cls::I);
    f.blocks[pad as usize].insts.push(Inst::Iconst(iv2, idx as i64));
    f.blocks[pad as usize].insts.push(Inst::Call(Callee::Host(failed), vec![iv2], Vec::new()));
    f.blocks[pad as usize].pos.extend([pos, pos]);
    f.blocks[pad as usize].term = Term::Jump(rest);
}

/// The probe's parameters as (name, T) when every one is a shared reference `name: &T` (the by-name
/// form: locals and parameters in scope at a line).
fn by_name_params(u: &crate::jit::Unit, probe: crate::program::DefId) -> Option<Vec<(String, crate::types::TyId)>> {
    use crate::types::TyKind;
    let sig = u.tcx.fn_sig(&u.prog, probe)?;
    let def = u.prog.def(probe);
    let file = &u.prog.files[def.file as usize];
    let crate::ast::ItemKind::Fn(fsig, _, _) = &file.ast.item(def.item).kind else { return None };
    let names: Vec<String> = file
        .ast
        .list(file.ast.fn_sig(*fsig).params)
        .iter()
        .map(|p| match file.ast.pat(p.pat) {
            crate::ast::Pat::Ident { name, .. } => file.text(*name).to_string(),
            _ => "_".into(),
        })
        .collect();
    if names.len() != sig.params.len() || names.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for (n, t) in names.iter().zip(sig.params.iter()) {
        match u.tcx.tys.kind(u.tcx.tys.erase_all_regions(*t)) {
            TyKind::Ref(false, inner, _) => out.push((n.clone(), *inner)),
            _ => return None,
        }
    }
    Some(out)
}

/// Checks the probe fn's parameters against the hooked slot's: same count and types, a by-value
/// type that needs dropping declared `ManuallyDrop<T>`, `&mut` only with `mutate`.
fn check_params(u: &crate::jit::Unit, hooked: u32, probe: crate::program::DefId, mutate: bool) -> Result<(), String> {
    use crate::types::TyKind;
    // a SPEC-V twin (`[unverified]`) runs its base slot's fn: same signature
    let crate::jit::FnKey::Inst(d, gargs) = super::instrument::base_key(u, hooked) else { return Err("the hooked slot is not a fn instance".into()) };
    let hs = u.tcx.fn_sig(&u.prog, *d).ok_or("the hooked fn has no signature")?;
    let ps = u.tcx.fn_sig(&u.prog, probe).ok_or("the probe fn has no signature")?;
    if ps.n_generics != 0 {
        return Err("the probe fn must not be generic".into());
    }
    // types modulo regions (late-bound ones included: the probe's `&[u8]` is the hooked fn's
    // `&'a [u8]`), aliases normalized: the same values in the same registers
    let env = crate::tcx::TypingEnv::fully_monomorphized(&u.prog);
    let key = |t: crate::types::TyId| u.tcx.tys.erase_all_regions(u.tcx.normalize_erasing_late_bound_regions(&u.prog, env, t));
    let hp: Vec<crate::types::TyId> = hs.params.iter().map(|t| key(u.tcx.tys.subst(*t, gargs))).collect();
    let pp: Vec<crate::types::TyId> = ps.params.iter().map(|t| key(*t)).collect();
    let show = |t: crate::types::TyId| crate::typeck::ty_to_string(false, &u.prog, &u.tcx, t);
    // two types that print the same: where they differ (another crate revision of the same item,
    // ...), with crate and definition ids
    let differ = |h: crate::types::TyId, p: crate::types::TyId| {
        let (a, b) = (show(h), show(p));
        if a != b {
            return (a, b);
        }
        let (a, b) = (crate::typeck::ty_to_string_paths(&u.prog, &u.tcx, h), crate::typeck::ty_to_string_paths(&u.prog, &u.tcx, p));
        if a != b {
            return (a, b);
        }
        (format!("{} (type #{})", a, h.0), format!("{} (type #{})", b, p.0))
    };
    if hp.len() != pp.len() {
        let want: Vec<String> = hp.iter().map(|t| show(*t)).collect();
        return Err(format!("the probe takes {} parameter(s), the hooked fn {}: ({})", pp.len(), hp.len(), want.join(", ")));
    }
    for (k, (h, p)) in hp.iter().zip(pp.iter()).enumerate() {
        let same = h == p;
        let wrapped = match u.tcx.tys.kind(*p) {
            TyKind::Adt(ad, args, _) => u.prog.def_path(*ad).ends_with("ManuallyDrop") && u.tcx.tys.list(*args).first() == Some(h),
            _ => false,
        };
        if !same && !wrapped {
            let (a, b) = differ(*h, *p);
            return Err(format!("parameter {}: the hooked fn has `{}`, the probe `{}`", k + 1, a, b));
        }
        if same && u.needs_drop_glue(*h) {
            return Err(format!("parameter {}: `{}` is dropped by the hooked fn -- declare it `std::mem::ManuallyDrop<{}>` in the probe", k + 1, show(*h), show(*h)));
        }
        if !mutate && matches!(u.tcx.tys.kind(*h), TyKind::Ref(true, _, _)) {
            return Err(format!("parameter {}: `{}` gives the probe write access -- add mutate=1 to allow it", k + 1, show(*h)));
        }
    }
    Ok(())
}

fn add(hook: &str, src: Vec<u8>, mutate: bool, low: bool) -> Result<(Arc<Probe>, Vec<String>, Vec<String>), String> {
    let up = crate::jit::current_unit();
    if up.is_null() {
        return Err("no running unit".into());
    }
    let id = NEXT.fetch_add(1, SeqCst) as u32;
    let path = super::staged_path(&format!("probe_{}.rs", id));
    // (on disk too: an error's line is the agent's own text)
    let _ = std::fs::write(&path, &src);
    let hook = hook.to_string();
    let src_text = String::from_utf8_lossy(&src).into_owned();
    let r = crate::owner::call_within(crate::owner::Class::Program, 30_000, move || -> Result<(Arc<Probe>, Vec<String>, Vec<String>), String> {
        let u = unsafe { &mut *up };
        let found = super::instrument::find_all(u, &hook, 8);
        if found.is_empty() {
            return Err(format!("no compiled fn or line matches `{}`{}", hook, super::instrument::closest(u, &hook)));
        }
        // the probe is a child module of the hooked fn's module: its private items, its `use`s
        // and `crate::` paths are the probe's too
        let scope = match super::instrument::base_key(u, found[0].0) {
            crate::jit::FnKey::Inst(d, _) => crate::modules::scope_of(u, *d),
            _ => return Err("the hooked slot is not a fn instance".into()),
        };
        let name = format!("__relax_probe_{}", id);
        let v = u.prog.child_parse_snapshot(scope, &name, &path).parse(src).map_err(|e| format!("{}: {}", path, e))?;
        let (child, _) = unsafe { crate::modules::load_in_module(up, scope, &name, v) }.map_err(|e| e.join("\n"))?;
        let u = unsafe { &mut *up };
        let sym = u.prog.syms.get("probe").ok_or("the probe module has no `fn probe`")?;
        let pd = *u.prog.mods[child as usize].values.get(&sym).ok_or("the probe module has no `fn probe`")?;
        let probe_slot = u.fn_id(crate::jit::FnKey::Inst(pd, Vec::new()));
        let mut lines = Vec::new();
        let mut slots = Vec::new();
        let mut ok: Vec<(u32, Option<u32>)> = Vec::new();
        // by name: every probe parameter is `&T`, at a file:line -- locals and parameters in scope
        // there, by the probe parameters' names
        let by_name = by_name_params(u, pd);
        for (slot, line, fname) in found {
            match (&by_name, line) {
                (Some(_), Some(_)) => ok.push((slot, line)),
                _ => match check_params(u, slot, pd, mutate) {
                    Ok(()) => ok.push((slot, line)),
                    Err(e) => lines.push(format!(
                        "{}: not hooked: {}{}",
                        fname,
                        e,
                        if line.is_some() { " (or name locals in scope: every parameter `name: &T`)" } else { "" }
                    )),
                },
            }
        }
        // the probe's record and the edits that name it are kept state: they outlive the
        // compiler arena (a settle swap destroys it; the probe code keeps calling in)
        let _kept = crate::zone_alloc::kept();
        let (p, idx) = register_probe(id, &hook, emit_exprs(&src_text));
        super::patches::with_low_tier(low, || {
        let mut rows = Vec::new();
        let owner = super::patches::Owner::Probe(id);
        for &(slot, line) in &ok {
            let u = unsafe { &*up };
            let fname = u.fns[slot as usize].name.clone();
            let file = super::codever::fn_lines(u, slot).map_or(u32::MAX, |x| x.0);
            let edit = match (&by_name, line) {
                (Some(params), Some(l)) => super::patches::Edit::ProbeLocals {
                    idx,
                    point: Arc::new(crate::lower::ProbePoint { slot, line: l, probe_slot, params: params.clone() }),
                },
                _ => super::patches::Edit::Probe { idx, probe_slot, line: line.map(|l| (file, l)) },
            };
            let before = super::codever::snapshot(u, slot);
            let r = unsafe { super::patches::add(up, slot, owner, edit) };
            let u = unsafe { &*up };
            rows.push(format!("{{\"hooked_slot\":{},\"fn\":{},\"hooked\":{},{}}}", slot, json_str(&short(&fname, 160)), matches!(r, Ok(true)), super::codever::patched_row(u, slot, &before, &r)));
            match r {
                Ok(true) => {
                    slots.push(slot);
                    lines.push(format!("{}: hooked", fname));
                }
                Ok(false) => lines.push(format!("{}: line not found in its code", fname)),
                Err(e) => lines.push(format!("{}: not hooked: {}", fname, e)),
            }
        }
        // inlined copies of the hooked fn / line in other slots' code: rebuilt to call it
        if !slots.is_empty() {
            rows.extend(unsafe { super::codever::cover_inliners(up, owner, &ok, false) });
        }
        *p.slots.lock().unwrap_or_else(|e| e.into_inner()) = slots;
        Ok((p, lines, rows))
        })
    });
    r.unwrap_or_else(|| Err(format!("the owner did not take the request in 30 s ({})", crate::owner::serving_note())))
}

pub(crate) fn scan_kept(a: &mut crate::jit::zone_scan::Acc) {
    let Ok(ps) = PROBES.try_lock() else { return };
    a.field("inspect.probes");
    a.vec(&ps);
    for p in ps.iter() {
        a.arc(p);
        a.string(&p.hook);
        if let Ok(s) = p.slots.try_lock() {
            a.vec(&s);
        }
        a.vec(&p.emits);
        for (_, names) in &p.emits {
            a.vec(names);
            for n in names {
                a.string(n);
            }
        }
        if let Ok(g) = p.agg.try_lock() {
            let (x, y) = g.recent.as_slices();
            if g.recent.capacity() > 0 {
                a.ptr(if x.is_empty() { y.as_ptr() } else { x.as_ptr() });
            }
            for (_, _, v) in g.recent.iter() {
                a.string(v);
            }
            a.map(&g.counts, |a, k, _| a.string(k));
            a.map(&g.hists, |a, k, _| a.string(k));
            a.map(&g.samples, |a, k, v| {
                a.string(k);
                a.vec(&v.1);
                for x in &v.1 {
                    a.string(x);
                }
            });
        }
    }
}

/// A new probe's record (its index): kept state whatever the caller's phase.
fn register_probe(id: u32, hook: &str, emits: Vec<(u32, Vec<String>)>) -> (Arc<Probe>, u64) {
    let _k = crate::zone_alloc::kept();
    let p = Arc::new(Probe {
        id,
        hook: hook.to_string(),
        slots: Mutex::new(Vec::new()),
        on: AtomicBool::new(true),
        hits: AtomicU64::new(0),
        ns: AtomicU64::new(0),
        timed: AtomicU64::new(0),
        failed: Mutex::new(None),
        agg: Mutex::new(Aggregates::default()),
        emits,
    });
    let mut ps = PROBES.lock().unwrap_or_else(|e| e.into_inner());
    ps.push(p.clone());
    (p, ps.len() as u64 - 1)
}

#[cfg(test)]
pub(super) fn register_probe_for_test(id: u32, hook: &str) -> (Arc<Probe>, u64) {
    register_probe(id, hook, Vec::new())
}

/// The live probes: (id, hook).
pub fn live() -> Vec<(u32, String)> {
    PROBES.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|p| p.on.load(Relaxed) && !p.slots.lock().unwrap_or_else(|e| e.into_inner()).is_empty()).map(|p| (p.id, p.hook.clone())).collect()
}

/// Any live injected probe?
pub fn any() -> bool {
    PROBES.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|p| p.on.load(Relaxed))
}

/// The live probes' counters: ("P<n> <hook>", hits, ns estimated from the timed hits) and their
/// `count` keys ("P<n> count <key>", n, 0).
pub fn counters(out: &mut Vec<(String, u64, u64)>) {
    let ps: Vec<Arc<Probe>> = PROBES.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|p| p.on.load(Relaxed)).cloned().collect();
    for p in ps {
        let (hits, ns, timed) = (p.hits.load(Relaxed), p.ns.load(Relaxed), p.timed.load(Relaxed));
        out.push((format!("P{} {}", p.id, p.hook), hits, if timed > 0 { ns * hits / timed } else { 0 }));
        let a = p.agg.lock().unwrap_or_else(|e| e.into_inner());
        for (k, v) in a.counts.iter() {
            out.push((format!("P{} count {}", p.id, k), *v, 0));
        }
    }
}

fn remove(id: Option<u32>) -> usize {
    remove_rows(id).0
}

/// `remove`, with a row per slot saying which code is back.
fn remove_rows(id: Option<u32>) -> (usize, Vec<String>) {
    let up = crate::jit::current_unit();
    let take: Vec<Arc<Probe>> = PROBES.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|p| id.map_or(true, |k| k == p.id) && !p.slots.lock().unwrap_or_else(|e| e.into_inner()).is_empty()).cloned().collect();
    for p in &take {
        p.on.store(false, SeqCst);
    }
    let ids: Vec<u32> = take.iter().map(|p| p.id).collect();
    let rows = crate::owner::call_within(crate::owner::Class::Program, 10_000, move || {
        let mut rows = Vec::new();
        for id in ids {
            let owner = super::patches::Owner::Probe(id);
            let slots = super::patches::owner_slots(owner);
            unsafe { super::patches::remove_owner(up, owner) };
            let u = unsafe { &*up };
            for (slot, role, _) in slots {
                rows.push(format!("{{\"removed\":\"P{}\",\"slot\":{},\"role\":\"{}\",\"back\":{}}}", id, slot, role, super::codever::installed_json(u, slot)));
            }
        }
        rows
    }).unwrap_or_default();
    for p in &take {
        p.slots.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
    (take.len(), rows)
}

/// Removes every probe (the inspect session that added them ended).
pub fn remove_all() {
    remove(None);
}

fn read_json(p: &Probe, n_recent: usize) -> Vec<String> {
    let a = p.agg.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    // the latest emitted values first (newest last), each with its fields when the emit named them
    let now = now_ms();
    let skip = a.recent.len().saturating_sub(n_recent);
    for (t, line, v) in a.recent.iter().skip(skip) {
        out.push(format!("{{\"emit\":{},\"ago_ms\":{},\"line\":{}{}}}", json_str(v), now.saturating_sub(*t), line, named_fields(p, *line, v)));
    }
    if skip > 0 {
        out.push(format!("{{\"emits_not_shown\":{},\"kept\":{}}}", skip, a.recent.len()));
    }
    let mut counts: Vec<(&String, &u64)> = a.counts.iter().collect();
    counts.sort_by(|x, y| y.1.cmp(x.1).then(x.0.cmp(y.0)));
    for (k, v) in counts.iter().take(30) {
        out.push(format!("{{\"count\":{},\"n\":{}}}", json_str(k), v));
    }
    let mut hk: Vec<&String> = a.hists.keys().collect();
    hk.sort();
    for k in hk.iter().take(20) {
        let h = &a.hists[*k];
        let b: Vec<String> = h.buckets.iter().enumerate().filter(|x| *x.1 > 0).map(|(i, c)| format!("[{},{}]", if i == 0 { 0.0 } else { (2f64).powi(i as i32 - 1) }, c)).collect();
        out.push(format!("{{\"hist\":{},\"n\":{},\"min\":{},\"max\":{},\"mean\":{},\"log2_buckets\":[{}]}}", json_str(k), h.n, h.min, h.max, h.sum / h.n.max(1) as f64, b.join(",")));
    }
    let mut sk: Vec<&String> = a.samples.keys().collect();
    sk.sort();
    for k in sk.iter().take(20) {
        let (seen, v) = &a.samples[*k];
        let vs: Vec<String> = v.iter().take(32).map(|x| json_str(x)).collect();
        out.push(format!("{{\"sample\":{},\"seen\":{},\"values\":[{}]}}", json_str(k), seen, vs.join(",")));
    }
    out.push(format!("{{\"emitted\":{},\"emit_dropped\":{}}}", a.emitted, a.emit_dropped));
    out
}

/// `probe add <fn|file:line> <probe.rs | code> [mutate=1]` | `probe list` | `probe read P<n>` |
/// `probe rm <P<n>|all>`
pub fn cmd(req: &Req, out: &mut Out) {
    // a Splash line (`game.splash:76`) or a Splash probe (`SP3`): the script VM's line probes
    let sub = req.pos.first().map(String::as_str).unwrap_or("list");
    let target = req.pos.get(1).map(String::as_str).unwrap_or("");
    let splash = match sub {
        "add" => target.rsplit_once(':').is_some_and(|(f, _)| f.ends_with(".splash")),
        "read" | "rm" => target.starts_with("SP"),
        _ => false,
    };
    if splash {
        if !super::host_query("sprobe", req, out) {
            out.err("Splash line probes need a host with Splash VMs (makepad run of a makepad app)");
        }
        return;
    }
    match sub {
        "add" => {
            // the client sends the code (or a file's path) as `code`; a typed line has it as words
            let src = req.get("code").map(str::to_string).or_else(|| (req.pos.len() > 2).then(|| req.pos[2..].join(" ")));
            let (Some(hook), Some(src)) = (req.pos.get(1), src) else {
                out.err("usage: probe add <fn|file:line> <probe.rs file | code with `fn probe(..)`> [mutate=1]");
                return;
            };
            // a file path, else the code itself
            let code = match std::fs::read(src.trim()) {
                Ok(b) => b,
                Err(_) => src.into_bytes(),
            };
            match add(hook, code, req.get("mutate") == Some("1"), req.get("tier").is_some_and(|t| t.eq_ignore_ascii_case("o1"))) {
                Ok((p, lines, rows)) => {
                    out.measured(&format!("probe P{} at {}", p.id, p.hook), "owner: module load + instrumented builds", &[]);
                    for r in rows {
                        out.line(&r);
                    }
                    out.line(&format!("{{\"probe\":\"P{}\",\"hooked\":{},\"notes\":[{}],\"more\":[\"probe read P{}\",\"probe list\",\"probe rm P{}\",\"events follow=1 on=probe\"]}}", p.id, p.slots.lock().unwrap_or_else(|e| e.into_inner()).len(), lines.iter().map(|l| json_str(l)).collect::<Vec<_>>().join(","), p.id, p.id));
                }
                Err(e) => out.err(&e),
            }
        }
        "list" => {
            out.measured("probes", "probe counters", &[]);
            // the Splash line probes (SP<n>) first
            let _ = super::host_query("sprobe", req, out);
            for p in PROBES.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|p| !p.slots.lock().unwrap_or_else(|e| e.into_inner()).is_empty()) {
                let (hits, ns, timed) = (p.hits.load(Relaxed), p.ns.load(Relaxed), p.timed.load(Relaxed));
                let failed = p.failed.lock().unwrap_or_else(|e| e.into_inner()).clone();
                out.line(&format!(
                    "{{\"probe\":\"P{}\",\"at\":{},\"slots\":{},\"on\":{},\"hits\":{},\"us_per_hit\":{:.3}{}}}",
                    p.id,
                    json_str(&p.hook),
                    p.slots.lock().unwrap_or_else(|e| e.into_inner()).len(),
                    p.on.load(Relaxed),
                    hits,
                    ns as f64 / 1e3 / timed.max(1) as f64,
                    failed.map(|f| format!(",\"failed\":{}", json_str(&short(&f, 200)))).unwrap_or_default()
                ));
            }
        }
        "read" => {
            let Some(id) = req.pos.get(1).and_then(|s| s.trim_start_matches('P').parse::<u32>().ok()) else {
                out.err("usage: probe read P<n>");
                return;
            };
            let ps = PROBES.lock().unwrap_or_else(|e| e.into_inner());
            match ps.iter().find(|p| p.id == id && !p.slots.lock().unwrap_or_else(|e| e.into_inner()).is_empty()).cloned() {
                Some(p) => {
                    drop(ps);
                    out.measured(&format!("probe P{} aggregates ({} hits)", p.id, p.hits.load(Relaxed)), "probe tables", &[]);
                    let n = req.get("n").and_then(|v| v.parse::<usize>().ok()).unwrap_or(12).clamp(1, RECENT);
                    for l in read_json(&p, n) {
                        out.line(&l);
                    }
                }
                None => out.err(&format!("no probe P{}", id)),
            }
        }
        "rm" => {
            let w = req.pos.get(1).map(String::as_str).unwrap_or("all");
            let id = if w == "all" { None } else { w.trim_start_matches('P').parse::<u32>().ok() };
            let (n, rows) = remove_rows(id);
            for r in rows {
                out.line(&r);
            }
            out.line(&format!("{{\"removed\":{}}}", n));
        }
        _ => out.err("usage: probe add <fn|file:line> <probe.rs|code> [mutate=1] | list | read P<n> | rm <P<n>|all>"),
    }
}
