//! Injected Rust probes (DESIGN.md §10a): an agent's Rust fn runs at a hook point of a running
//! program. The probe is the agent's own text, loaded as a trusted run-time module (modules.rs:
//! front, type check, compile) that depends on the hooked fn's crate and that crate's deps; it
//! is not spliced into anything. Its `probe` fn takes the hooked fn's parameters (same order and
//! types; a non-Copy by-value parameter is declared `ManuallyDrop<T>`: the same ABI and no drop),
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
    /// emits in the current second (rate limit) and that second
    emit_window: (u64, u64),
}

struct Probe {
    id: u32,
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
}

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

pub extern "C" fn relax_probe_emit(t: *const u8, n: usize) {
    let Some(p) = current() else { return };
    let line = short(text(t, n), 1000);
    let send = {
        let mut a = p.agg.lock().unwrap_or_else(|e| e.into_inner());
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
        super::events::emit("probe", &format!("P{}|{}", p.id, seq), &format!("\"probe\":\"P{}\",\"value\":{}", p.id, json_str(&line)));
    }
}

pub extern "C" fn relax_probe_count(k: *const u8, n: usize) {
    let Some(p) = current() else { return };
    *p.agg.lock().unwrap_or_else(|e| e.into_inner()).counts.entry(text(k, n).to_string()).or_default() += 1;
}

pub extern "C" fn relax_probe_hist(k: *const u8, n: usize, x: f64) {
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

// ------------------------------------------------------------------ adding a probe

/// The probe call at the hook (entry: line None) with the hooked fn's own parameters.
pub(super) fn edit(f: &mut crate::rir::Func, u: &crate::jit::Unit, idx: u64, probe_slot: u32, line: Option<u32>) -> bool {
    // where: block b before instruction k
    let (b, k) = match line {
        None => (0usize, 0usize),
        Some(l) => {
            let mut at = None;
            'f: for b in 0..f.blocks.len() {
                for k in 0..f.blocks[b].insts.len() {
                    if super::instrument::pos_line(u, f.blocks[b].pos[k]) == Some(l) {
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
    let args: Vec<crate::rir::VReg> = f.params.iter().copied().filter(|v| Some(*v) != f.sret).collect();
    place_call(f, u, idx, probe_slot, b, k, args);
    true
}

/// A by-name probe's calls, lowered into the body already (lower.rs ProbePoint): each becomes the
/// guarded invoke.
pub(super) fn wrap_calls(f: &mut crate::rir::Func, u: &crate::jit::Unit, idx: u64, probe_slot: u32) -> bool {
    use crate::rir::{Callee, Inst, Term};
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
fn place_call(f: &mut crate::rir::Func, u: &crate::jit::Unit, idx: u64, probe_slot: u32, b: usize, k: usize, args: Vec<crate::rir::VReg>) {
    use crate::rir::{Callee, Cls, Inst, Term};
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
        match u.tcx.tys.kind(u.tcx.tys.erase_regions(*t)) {
            TyKind::Ref(false, inner, _) => out.push((n.clone(), u.tcx.tys.erase_regions(*inner))),
            _ => return None,
        }
    }
    Some(out)
}

/// Checks the probe fn's parameters against the hooked slot's: same count and types, a by-value
/// type that needs dropping declared `ManuallyDrop<T>`, `&mut` only with `mutate`.
fn check_params(u: &crate::jit::Unit, hooked: u32, probe: crate::program::DefId, mutate: bool) -> Result<(), String> {
    use crate::types::TyKind;
    let crate::jit::FnKey::Inst(d, gargs) = &u.fns[hooked as usize].key else { return Err("the hooked slot is not a fn instance".into()) };
    let hs = u.tcx.fn_sig(&u.prog, *d).ok_or("the hooked fn has no signature")?;
    let ps = u.tcx.fn_sig(&u.prog, probe).ok_or("the probe fn has no signature")?;
    if ps.n_generics != 0 {
        return Err("the probe fn must not be generic".into());
    }
    let hp: Vec<crate::types::TyId> = hs.params.iter().map(|t| u.tcx.tys.erase_regions(u.tcx.tys.subst(*t, gargs))).collect();
    let pp: Vec<crate::types::TyId> = ps.params.iter().map(|t| u.tcx.tys.erase_regions(*t)).collect();
    let show = |t: crate::types::TyId| crate::typeck::ty_to_string(false, &u.prog, &u.tcx, t);
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
            return Err(format!("parameter {}: the hooked fn has `{}`, the probe `{}`", k + 1, show(*h), show(*p)));
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

fn add(hook: &str, src: Vec<u8>, mutate: bool) -> Result<(Arc<Probe>, Vec<String>), String> {
    let up = crate::jit::current_unit();
    if up.is_null() {
        return Err("no running unit".into());
    }
    let id = NEXT.fetch_add(1, SeqCst) as u32;
    let dir = std::env::temp_dir().join(format!("relax-probes-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("probe_{}.rs", id));
    std::fs::write(&path, &src).map_err(|e| format!("cannot stage the probe: {}", e))?;
    let hook = hook.to_string();
    let r = crate::owner::call_within(crate::owner::Class::App, 30_000, move || -> Result<(Arc<Probe>, Vec<String>), String> {
        let u = unsafe { &mut *up };
        let found = super::instrument::find(u, &hook, 8);
        if found.is_empty() {
            return Err(format!("no compiled fn or line matches `{}`", hook));
        }
        // the module depends on the hooked fn's crate and that crate's deps
        let krate = match &u.fns[found[0].0 as usize].key {
            crate::jit::FnKey::Inst(d, _) => u.prog.def(*d).krate,
            _ => return Err("the hooked slot is not a fn instance".into()),
        };
        let mut deps = vec![u.prog.syms.str(u.prog.crates[krate as usize].name).to_string()];
        deps.extend(u.prog.crates[krate as usize].deps.iter().map(|(n, _)| u.prog.syms.str(*n).to_string()));
        let name = format!("relax_probe_{}", id);
        let (pk, _, _) = unsafe { crate::modules::load_trusted(up, &name, &path.to_string_lossy(), &deps) }.map_err(|e| e.join("\n"))?;
        let u = unsafe { &mut *up };
        let root = u.prog.crates[pk as usize].root_mod;
        let sym = u.prog.syms.get("probe").ok_or("the probe module has no `fn probe`")?;
        let pd = *u.prog.mods[root as usize].values.get(&sym).ok_or("the probe module has no `fn probe`")?;
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
        let p = Arc::new(Probe {
            id,
            hook: hook.clone(),
            slots: Mutex::new(Vec::new()),
            on: AtomicBool::new(true),
            hits: AtomicU64::new(0),
            ns: AtomicU64::new(0),
            timed: AtomicU64::new(0),
            failed: Mutex::new(None),
            agg: Mutex::new(Aggregates::default()),
        });
        let idx = {
            let mut ps = PROBES.lock().unwrap_or_else(|e| e.into_inner());
            ps.push(p.clone());
            ps.len() as u64 - 1
        };
        for (slot, line) in ok {
            let fname = u.fns[slot as usize].name.clone();
            let edit = match (&by_name, line) {
                (Some(params), Some(l)) => super::patches::Edit::ProbeLocals {
                    idx,
                    point: Arc::new(crate::lower::ProbePoint { slot, line: l, probe_slot, params: params.clone() }),
                },
                _ => super::patches::Edit::Probe { idx, probe_slot, line },
            };
            match unsafe { super::patches::add(up, slot, super::patches::Owner::Probe(id), edit) } {
                Ok(true) => {
                    slots.push(slot);
                    lines.push(format!("{}: hooked", fname));
                }
                Ok(false) => lines.push(format!("{}: line not found in its code", fname)),
                Err(e) => lines.push(format!("{}: not hooked: {}", fname, e)),
            }
        }
        *p.slots.lock().unwrap_or_else(|e| e.into_inner()) = slots;
        Ok((p, lines))
    });
    r.unwrap_or_else(|| Err(format!("the owner did not take the request in 30 s ({})", crate::owner::serving_note())))
}

fn remove(id: Option<u32>) -> usize {
    let up = crate::jit::current_unit();
    let take: Vec<Arc<Probe>> = PROBES.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|p| id.map_or(true, |k| k == p.id) && !p.slots.lock().unwrap_or_else(|e| e.into_inner()).is_empty()).cloned().collect();
    for p in &take {
        p.on.store(false, SeqCst);
    }
    let ids: Vec<u32> = take.iter().map(|p| p.id).collect();
    let _ = crate::owner::call_within(crate::owner::Class::App, 10_000, move || {
        for id in ids {
            unsafe { super::patches::remove_owner(up, super::patches::Owner::Probe(id)) };
        }
    });
    for p in &take {
        p.slots.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
    take.len()
}

/// Removes every probe (the inspect session that added them ended).
pub fn remove_all() {
    remove(None);
}

fn read_json(p: &Probe) -> Vec<String> {
    let a = p.agg.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
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
    match req.pos.first().map(String::as_str).unwrap_or("list") {
        "add" => {
            let (Some(hook), Some(src)) = (req.pos.get(1), req.pos.get(2)) else {
                out.err("usage: probe add <fn|file:line> <probe.rs file | code with `fn probe(..)`> [mutate=1]");
                return;
            };
            // a file path, else the rest of the line is the code
            let code = match std::fs::read(src) {
                Ok(b) => b,
                Err(_) => req.pos[2..].join(" ").into_bytes(),
            };
            match add(hook, code, req.get("mutate") == Some("1")) {
                Ok((p, lines)) => {
                    out.measured(&format!("probe P{} at {}", p.id, p.hook), "owner: module load + instrumented builds", &[]);
                    out.line(&format!("{{\"probe\":\"P{}\",\"hooked\":{},\"notes\":[{}],\"more\":[\"probe read P{}\",\"probe list\",\"probe rm P{}\",\"events follow=1 on=probe\"]}}", p.id, p.slots.lock().unwrap_or_else(|e| e.into_inner()).len(), lines.iter().map(|l| json_str(l)).collect::<Vec<_>>().join(","), p.id, p.id));
                }
                Err(e) => out.err(&e),
            }
        }
        "list" => {
            out.measured("probes", "probe counters", &[]);
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
                    for l in read_json(&p) {
                        out.line(&l);
                    }
                }
                None => out.err(&format!("no probe P{}", id)),
            }
        }
        "rm" => {
            let w = req.pos.get(1).map(String::as_str).unwrap_or("all");
            let id = if w == "all" { None } else { w.trim_start_matches('P').parse::<u32>().ok() };
            out.line(&format!("{{\"removed\":{}}}", remove(id)));
        }
        _ => out.err("usage: probe add <fn|file:line> <probe.rs|code> [mutate=1] | list | read P<n> | rm <P<n>|all>"),
    }
}
