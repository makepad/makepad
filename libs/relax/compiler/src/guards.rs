//! Speculation in O3 code: guards with a fallback, failure counting and relink (BEYOND2).
//!
//! One mechanism for every speculated fact. A *site* is (caller slot, source position) of an
//! operation O3 code may specialise; its `SiteRec` lives for the run (kept heap, never freed) and
//! carries the site's value profile, its guard-failure count and its place on the escalation
//! ladder. The first speculative fact is the target of a `dyn` / fn-pointer call
//! (`Callee::Indirect`):
//!
//! 1. PROFILE: an O3 build of the caller puts a 2-row target profile in front of the call (the
//!    first row is counted inline: load, compare, add, store; any other target goes to
//!    `rt_spec_probe`). When the site has seen `ready()` calls it queues its caller for an O3
//!    rebuild (Q_RETIER, the existing tier queue).
//! 2. The rebuild decides in its snapshot (`decide`): a row with >= 30% of the calls whose target
//!    is a slot becomes the *guess* (GUARD); otherwise the site is despecialised (OFF: the plain
//!    call, no probe, for good).
//! 3. GUARD: `if target == FnAddr(guess) { Call(Fn(guess)) } else { failures += 1; Call(Indirect) }`.
//!    The direct call is a normal call the second O3 inlining round can inline; the fallback is
//!    the original indirect call, so a failing guard is still correct, it only costs a compare.
//!    `fail_limit()` failures call `rt_spec_fail`, which puts the site back to PROFILE and
//!    relinks the caller (Q_RETIER again): the failure path of the speculation.
//! 4. The ladder (C2's trap limits): a re-profile that finds the same guess makes the guard
//!    STABLE (no failure counting any more: a bimorphic site keeps its fast path for the major
//!    target); one that finds a new target re-specialises at most `MAX_RESPECS` times; after that,
//!    or when no target reaches 30%, the site is OFF.
//!
//! Code with probes or guards holds the records' addresses as immediates, so it is never written to
//! the disk cache (`Ctx::emitted`); a later run profiles again. Hot reload needs nothing extra: a
//! patched caller is built again (new code, same records), a patched guessed callee was inlined or
//! is called through its slot, and inlined callees recompile their callers as before.
//! Off by default: MAKEPAD_RUN_GUARDS=1 turns it on (MAKEPAD_RUN_SPEC is the unrelated ahead-of-call compile).
//! MAKEPAD_RUN_GUARDS_READY / _FAILS set the
//! thresholds (diff_fast uses small ones so every guard path runs), MAKEPAD_RUN_GUARDS_TRACE=1 logs
//! each transition.

include!("rt_abi.rs");

use relax_backend::rir::{Callee, Cls, Cond, Func, IOp, Inst, Mem, Sym, Term, VReg, U64};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

pub const PROFILE: u8 = 0;
pub const GUARD: u8 = 1;
pub const STABLE: u8 = 2;
pub const OFF: u8 = 3;
const STATE_NAMES: [&str; 4] = ["profile", "guard", "stable", "off"];
/// a guess must cover this share of the profiled calls (GDV's consistent-win threshold)
const GUESS_SHARE: f64 = 0.30;
/// re-specialisations of one site to a new target before it is turned off
const MAX_RESPECS: u8 = 2;

/// Field offsets the generated code uses (repr(C)).
const OFF_T0: i32 = 0;
const OFF_N0: i32 = 8;
const OFF_FAILS: i32 = 40;
const OFF_DONE: i32 = 56;

#[repr(C)]
pub struct SiteRec {
    /// two profile rows (target, count) and the calls that matched neither
    t0: AtomicU64,
    n0: AtomicU64,
    t1: AtomicU64,
    n1: AtomicU64,
    other: AtomicU64,
    /// guard failures since the guard was installed
    fails: AtomicU64,
    /// the guessed target (a slot's callable address) while GUARD / STABLE
    guess: AtomicU64,
    /// nonzero once the profile is ready: PROFILE code stops calling rt_spec_probe (a polymorphic
    /// site waiting in a long O3 queue would otherwise pay a host call per call)
    done: AtomicU64,
    slot: u32,
    pos: u64,
    state: AtomicU8,
    respecs: AtomicU8,
    /// a rebuild of the caller was queued for this site's current state
    queued: AtomicBool,
}

impl SiteRec {
    fn total(&self) -> u64 {
        self.n0.load(Ordering::Relaxed) + self.n1.load(Ordering::Relaxed) + self.other.load(Ordering::Relaxed)
    }
    fn reset_profile(&self) {
        for a in [&self.t0, &self.n0, &self.t1, &self.n1, &self.other, &self.fails, &self.done] {
            a.store(0, Ordering::Relaxed);
        }
    }
}

fn env_u64(name: &str, d: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).filter(|v| *v > 0).unwrap_or(d)
}

pub fn enabled() -> bool {
    static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *E.get_or_init(|| std::env::var("MAKEPAD_RUN_GUARDS").is_ok_and(|v| v == "1"))
}
fn ready() -> u64 {
    static E: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *E.get_or_init(|| env_u64("MAKEPAD_RUN_GUARDS_READY", 1000))
}
fn fail_limit() -> u64 {
    static E: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *E.get_or_init(|| env_u64("MAKEPAD_RUN_GUARDS_FAILS", 1 << 16))
}
fn trace() -> bool {
    static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *E.get_or_init(|| std::env::var_os("MAKEPAD_RUN_GUARDS_TRACE").is_some())
}

/// Every site of the run, by caller slot (a slot's sites are few; the map is read once per O3
/// build of a caller and written when a build meets a new site).
static SITES: relax_session::sync::Mutex<Option<HashMap<u32, Vec<&'static SiteRec>>>> = relax_session::sync::Mutex::new(None);

fn site(slot: u32, pos: u64) -> &'static SiteRec {
    // (a source place: the inline context does not split a site)
    let pos = relax_backend::rir::pos_plain(pos);
    let mut g = SITES.lock().unwrap_or_else(|e| e.into_inner());
    crate::zone_alloc::kept_with(|| {
        let v = g.get_or_insert_with(HashMap::new).entry(slot).or_default();
        if let Some(r) = v.iter().find(|r| r.pos == pos) {
            return *r;
        }
        let r: &'static SiteRec = Box::leak(Box::new(SiteRec {
            t0: AtomicU64::new(0), n0: AtomicU64::new(0), t1: AtomicU64::new(0), n1: AtomicU64::new(0),
            other: AtomicU64::new(0), fails: AtomicU64::new(0), guess: AtomicU64::new(0), done: AtomicU64::new(0),
            slot, pos, state: AtomicU8::new(PROFILE), respecs: AtomicU8::new(0), queued: AtomicBool::new(false),
        }));
        v.push(r);
        if trace() {
            static WATCH: std::sync::Once = std::sync::Once::new();
            WATCH.call_once(|| {
                let _ = std::thread::Builder::new().name("run-guards-trace".to_string()).spawn(|| loop {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    dump(&|id| format!("#{}", id));
                });
            });
        }
        r
    })
}

fn note(r: &SiteRec, what: &str) {
    if trace() {
        eprintln!("guards: slot {} pos {:#x}: {} (state {}, t0 {:#x} x{}, t1 {:#x} x{}, other {}, fails {}, respecs {})", r.slot, r.pos, what,
            STATE_NAMES[r.state.load(Ordering::Relaxed) as usize], r.t0.load(Ordering::Relaxed), r.n0.load(Ordering::Relaxed),
            r.t1.load(Ordering::Relaxed), r.n1.load(Ordering::Relaxed), r.other.load(Ordering::Relaxed), r.fails.load(Ordering::Relaxed),
            r.respecs.load(Ordering::Relaxed));
    }
}

/// Queues the site's caller for an O3 rebuild, once per state.
fn relink(r: &SiteRec, why: &str) {
    r.done.store(1, Ordering::Relaxed);
    if !r.queued.swap(true, Ordering::AcqRel) {
        note(r, why);
        crate::jit::guard_relink(r.slot);
    }
}

rt_fn! {
/// A call whose target is not the first profile row (PROFILE code).
fn rt_spec_probe(rec: u64, target: u64) {
    let _z = crate::zone_alloc::compiler();
    let r = unsafe { &*(rec as *const SiteRec) };
    if r.t0.compare_exchange(0, target, Ordering::Relaxed, Ordering::Relaxed).is_ok() || r.t0.load(Ordering::Relaxed) == target {
        r.n0.fetch_add(1, Ordering::Relaxed);
    } else if r.t1.compare_exchange(0, target, Ordering::Relaxed, Ordering::Relaxed).is_ok() || r.t1.load(Ordering::Relaxed) == target {
        r.n1.fetch_add(1, Ordering::Relaxed);
    } else {
        r.other.fetch_add(1, Ordering::Relaxed);
    }
    if r.total() >= ready() {
        relink(r, "profile ready");
    }
}
}

rt_fn! {
/// The first row reached `ready()` calls (PROFILE code, inline count).
fn rt_spec_ready(rec: u64) {
    let _z = crate::zone_alloc::compiler();
    relink(unsafe { &*(rec as *const SiteRec) }, "profile ready");
}
}

rt_fn! {
/// A guard failed `fail_limit()` times (GUARD code): profile again, then relink.
fn rt_spec_fail(rec: u64) {
    let _z = crate::zone_alloc::compiler();
    let r = unsafe { &*(rec as *const SiteRec) };
    if r.state.compare_exchange(GUARD, PROFILE, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
        r.reset_profile();
        r.queued.store(false, Ordering::Release);
        relink(r, "guard failing: profile again");
    }
}
}

/// MAKEPAD_RUN_GUARDS_TRACE: every site's final state at the end of the run (Unit drop).
pub fn dump(name: &dyn Fn(u32) -> String) {
    if !trace() {
        return;
    }
    let g = SITES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(m) = g.as_ref() else { return };
    let mut by = [0usize; 4];
    let mut stuck = 0usize;
    for (slot, v) in m {
        for r in v {
            let st = r.state.load(Ordering::Relaxed);
            by[st as usize] += 1;
            if st == PROFILE && r.queued.load(Ordering::Relaxed) {
                stuck += 1;
                eprintln!("guards: end: slot {} {} pos {:#x}: profile ready, never rebuilt (calls {})", slot, name(*slot), r.pos, r.total());
            }
        }
    }
    eprintln!("guards: end: sites profile {} guard {} stable {} off {}; ready but not rebuilt {}", by[0], by[1], by[2], by[3], stuck);
    let _ = std::io::Write::flush(&mut std::io::stderr());
}

/// The runtime entries generated code calls (interned by the snapshot, under the unit lock).
#[derive(Clone, Copy)]
pub struct Syms {
    probe: Sym,
    ready: Sym,
    fail: Sym,
}

pub fn syms(u: &crate::jit::Unit) -> Syms {
    Syms {
        probe: u.syms.intern(crate::syms::SymKind::Rt("guard_probe"), rt_spec_probe as *const () as usize as u64),
        ready: u.syms.intern(crate::syms::SymKind::Rt("guard_ready"), rt_spec_ready as *const () as usize as u64),
        fail: u.syms.intern(crate::syms::SymKind::Rt("guard_fail"), rt_spec_fail as *const () as usize as u64),
    }
}

/// The snapshot's decision for the sites of slot `id` that finished profiling (unit lock held):
/// returns (guessed target, its slot): the snapshot must hold those slots' RIR so the guarded calls
/// can inline. `slot_of` maps a profiled target (a callable address) to its slot.
pub fn decide(id: u32, slot_of: &dyn Fn(u64) -> Option<u32>) -> Vec<(u64, u32)> {
    let recs: Vec<&'static SiteRec> = SITES.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&id).cloned()).unwrap_or_default();
    let mut out = Vec::new();
    for r in recs {
        let st = r.state.load(Ordering::Acquire);
        if st == PROFILE && r.queued.load(Ordering::Acquire) {
            let total = r.total();
            let (n0, n1) = (r.n0.load(Ordering::Relaxed), r.n1.load(Ordering::Relaxed));
            let (top, n) = if n1 > n0 { (r.t1.load(Ordering::Relaxed), n1) } else { (r.t0.load(Ordering::Relaxed), n0) };
            let old = r.guess.load(Ordering::Relaxed);
            let share = if total == 0 { 0.0 } else { n as f64 / total as f64 };
            let next = match slot_of(top) {
                Some(_) if share >= GUESS_SHARE && old == top => STABLE,
                Some(_) if share >= GUESS_SHARE && old != 0 && r.respecs.load(Ordering::Relaxed) >= MAX_RESPECS => OFF,
                Some(_) if share >= GUESS_SHARE => {
                    if old != 0 {
                        r.respecs.fetch_add(1, Ordering::Relaxed);
                    }
                    GUARD
                }
                _ => OFF,
            };
            if next != OFF {
                r.guess.store(top, Ordering::Relaxed);
            }
            r.fails.store(0, Ordering::Relaxed);
            r.state.store(next, Ordering::Release);
            r.queued.store(false, Ordering::Release);
            note(r, "decided");
        }
        if matches!(r.state.load(Ordering::Acquire), GUARD | STABLE) {
            let g = r.guess.load(Ordering::Relaxed);
            if let Some(s) = slot_of(g) {
                if !out.contains(&(g, s)) {
                    out.push((g, s));
                }
            }
        }
    }
    out
}

/// One O3 build's speculation context: set by compile_tier1 around its optimisation, read by
/// the root's simplification after the first inlining round.
pub struct Ctx {
    pub root: u32,
    pub syms: Syms,
    /// guessed target -> slot (decided in the snapshot)
    pub slots: Vec<(u64, u32)>,
    /// the build holds probes or guards (never cached)
    pub emitted: std::cell::Cell<bool>,
}

thread_local! {
    static CTX: std::cell::Cell<*const Ctx> = const { std::cell::Cell::new(std::ptr::null()) };
}

/// Runs `f` with `ctx` as this thread's build context.
pub fn with_ctx<R>(ctx: &Ctx, f: impl FnOnce() -> R) -> R {
    struct Reset(*const Ctx);
    impl Drop for Reset {
        fn drop(&mut self) {
            CTX.with(|c| c.set(self.0));
        }
    }
    let _r = Reset(CTX.with(|c| c.replace(ctx as *const Ctx)));
    f()
}

/// The root build's hook (tier1_simplify, after the first inlining round, before the pipeline):
/// indirect calls at GUARD / STABLE sites get their guard, so the second round can inline the
/// guessed callee and the pipeline optimises across it.
pub fn apply(func: &mut Func, id: u32) {
    let p = CTX.with(|c| c.get());
    if p.is_null() {
        return;
    }
    let ctx = unsafe { &*p };
    if ctx.root != id {
        return;
    }
    edit(ctx, func, false);
}

/// After the O3 pipeline (compile_tier1, before code generation): indirect calls at PROFILE sites
/// get their probe. Late, so the probe's counter stores never constrain the optimiser.
pub fn probe(ctx: &Ctx, func: &mut Func) {
    edit(ctx, func, true);
}

fn edit(ctx: &Ctx, func: &mut Func, probes: bool) {
    let id = ctx.root;
    let mut sites: Vec<(usize, usize)> = Vec::new();
    for (bi, b) in func.blocks.iter().enumerate() {
        if matches!(b.term, Term::Unreachable | Term::Resume) {
            continue;
        }
        for (ii, i) in b.insts.iter().enumerate() {
            if let Inst::Call(Callee::Indirect(_), _, _) = i {
                sites.push((bi, ii));
            }
        }
    }
    // later sites of a block first: a split moves only what follows the call
    sites.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    let mut changed = 0usize;
    let n_sites = sites.len();
    for (bi, ii) in sites {
        let pos = func.blocks[bi].pos[ii];
        let r = site(id, pos);
        match r.state.load(Ordering::Acquire) {
            PROFILE if probes => {
                probe_site(func, bi, ii, r, ctx.syms);
                changed += 1;
            }
            st @ (GUARD | STABLE) if !probes => {
                let g = r.guess.load(Ordering::Relaxed);
                if let Some(&(_, slot)) = ctx.slots.iter().find(|x| x.0 == g) {
                    guard_site(func, bi, ii, r, slot, st == GUARD, ctx.syms);
                    changed += 1;
                }
            }
            _ => {}
        }
    }
    if trace() && n_sites > 0 {
        eprintln!("guards: build of slot {}: {} indirect sites, {} {}", id, n_sites, changed, if probes { "probed" } else { "guarded" });
    }
    if changed > 0 {
        ctx.emitted.set(true);
        relax_backend::passes::renumber_rpo(func);
    }
}

/// Moves the call at (bi, ii) and everything after it (with the block's terminator) into a new
/// block; returns it. Block `bi` is left without a terminator decision (Unreachable).
fn split_at_call(f: &mut Func, bi: usize, ii: usize) -> u32 {
    let nb = f.block();
    let b = &mut f.blocks[bi];
    let insts = b.insts.split_off(ii);
    let pos = b.pos.split_off(ii);
    let term = std::mem::replace(&mut b.term, Term::Unreachable);
    let tp = b.term_pos;
    let n = &mut f.blocks[nb as usize];
    n.insts = insts;
    n.pos = pos;
    n.term = term;
    n.term_pos = tp;
    nb
}

fn push(f: &mut Func, b: u32, i: Inst, pos: u64) {
    let blk = &mut f.blocks[b as usize];
    blk.insts.push(i);
    blk.pos.push(pos);
}

fn rec_addr(f: &mut Func, b: u32, r: &SiteRec, pos: u64) -> VReg {
    let a = f.vreg(Cls::I);
    push(f, b, Inst::Iconst(a, r as *const SiteRec as i64), pos);
    a
}

/// `*n += 1` at `addr + off`; the new count.
fn bump(f: &mut Func, b: u32, addr: VReg, off: i32, pos: u64) -> VReg {
    let n = f.vreg(Cls::I);
    let n1 = f.vreg(Cls::I);
    push(f, b, Inst::Load(Mem::Int(8, false), n, addr, off), pos);
    push(f, b, Inst::IBinI(IOp::Add, U64, n1, n, 1), pos);
    push(f, b, Inst::Store(Mem::Int(8, false), addr, off, n1), pos);
    n1
}

fn host_call(f: &mut Func, b: u32, sym: Sym, args: Vec<VReg>, pos: u64) {
    push(f, b, Inst::Call(Callee::Host(sym), args, Vec::new()), pos);
}

/// PROFILE: row 0 counted inline, the rest by rt_spec_probe; then the original call.
fn probe_site(f: &mut Func, bi: usize, ii: usize, r: &SiteRec, s: Syms) {
    let pos = f.blocks[bi].pos[ii];
    let Inst::Call(Callee::Indirect(v), _, _) = f.blocks[bi].insts[ii].clone() else { unreachable!() };
    let call = split_at_call(f, bi, ii);
    let b = bi as u32;
    let a = rec_addr(f, b, r, pos);
    let t = f.vreg(Cls::I);
    let c = f.vreg(Cls::I);
    push(f, b, Inst::Load(Mem::Int(8, false), t, a, OFF_T0), pos);
    push(f, b, Inst::ICmp(Cond::Eq, false, c, v, t), pos);
    let hit = f.block();
    let miss = f.block();
    f.blocks[bi].term = Term::Branch(c, hit, miss);
    f.blocks[bi].term_pos = pos;
    let a1 = rec_addr(f, hit, r, pos);
    let n1 = bump(f, hit, a1, OFF_N0, pos);
    let k = f.vreg(Cls::I);
    push(f, hit, Inst::ICmpI(Cond::Eq, false, k, n1, ready() as i64), pos);
    let rdy = f.block();
    f.blocks[hit as usize].term = Term::Branch(k, rdy, call);
    let a2 = rec_addr(f, rdy, r, pos);
    host_call(f, rdy, s.ready, vec![a2], pos);
    f.blocks[rdy as usize].term = Term::Jump(call);
    // miss: rt_spec_probe until the profile is ready
    let a3 = rec_addr(f, miss, r, pos);
    let dn = f.vreg(Cls::I);
    push(f, miss, Inst::Load(Mem::Int(8, false), dn, a3, OFF_DONE), pos);
    let rec_miss = f.block();
    f.blocks[miss as usize].term = Term::Branch(dn, call, rec_miss);
    let a4 = rec_addr(f, rec_miss, r, pos);
    host_call(f, rec_miss, s.probe, vec![a4, v], pos);
    f.blocks[rec_miss as usize].term = Term::Jump(call);
    f.blocks[rec_miss as usize].term_pos = pos;
    for x in [hit, rdy, miss] {
        f.blocks[x as usize].term_pos = pos;
    }
}

/// GUARD / STABLE: `target == FnAddr(slot)` -> direct call, else (failure count) the original call.
fn guard_site(f: &mut Func, bi: usize, ii: usize, r: &SiteRec, slot: u32, count: bool, s: Syms) {
    let pos = f.blocks[bi].pos[ii];
    let Inst::Call(Callee::Indirect(v), args, rets) = f.blocks[bi].insts[ii].clone() else { unreachable!() };
    let slow = split_at_call(f, bi, ii);
    // the slow block now starts with the original call; its rest (and terminator) is the join
    let after_term = f.blocks[slow as usize].term.clone();
    let invoke = matches!(after_term, Term::Invoke(..));
    let fast = f.block();
    {
        let tp = f.blocks[slow as usize].term_pos;
        if invoke {
            // the call is the block's last instruction: both calls end their block with the same edges
            push(f, fast, Inst::Call(Callee::Fn(slot), args.clone(), rets.clone()), pos);
            f.blocks[fast as usize].term = after_term.clone();
            f.blocks[fast as usize].term_pos = tp;
        } else {
            // split the join off the slow block: slow = [original call] -> join
            let join = f.block();
            let sb = &mut f.blocks[slow as usize];
            let rest = sb.insts.split_off(1);
            let rest_pos = sb.pos.split_off(1);
            let term = std::mem::replace(&mut sb.term, Term::Jump(join));
            let jb = &mut f.blocks[join as usize];
            jb.insts = rest;
            jb.pos = rest_pos;
            jb.term = term;
            jb.term_pos = tp;
            f.blocks[slow as usize].term_pos = pos;
            push(f, fast, Inst::Call(Callee::Fn(slot), args.clone(), rets.clone()), pos);
            f.blocks[fast as usize].term = Term::Jump(join);
            f.blocks[fast as usize].term_pos = pos;
        }
    }
    let b = bi as u32;
    let g = f.vreg(Cls::I);
    let c = f.vreg(Cls::I);
    push(f, b, Inst::FnAddr(g, slot), pos);
    push(f, b, Inst::ICmp(Cond::Eq, false, c, v, g), pos);
    let miss = if count {
        // failures counted; the limit relinks through rt_spec_fail
        let m = f.block();
        let a = rec_addr(f, m, r, pos);
        let n1 = bump(f, m, a, OFF_FAILS, pos);
        let k = f.vreg(Cls::I);
        push(f, m, Inst::ICmpI(Cond::Eq, false, k, n1, fail_limit() as i64), pos);
        let lim = f.block();
        f.blocks[m as usize].term = Term::Branch(k, lim, slow);
        f.blocks[m as usize].term_pos = pos;
        let a2 = rec_addr(f, lim, r, pos);
        host_call(f, lim, s.fail, vec![a2], pos);
        f.blocks[lim as usize].term = Term::Jump(slow);
        f.blocks[lim as usize].term_pos = pos;
        m
    } else {
        slow
    };
    f.blocks[bi].term = Term::Branch(c, fast, miss);
    f.blocks[bi].term_pos = pos;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec() -> &'static SiteRec {
        site(u32::MAX - 7, 0x1234)
    }
    fn syms() -> Syms {
        Syms { probe: Sym::new(1), ready: Sym::new(2), fail: Sym::new(3) }
    }
    /// fn(fp, x) -> r: one indirect call in the middle of a block, or (invoke) ending one with an unwind edge
    fn func(invoke: bool) -> Func {
        let mut f = Func::new("t".to_string(), 0);
        let b = f.block();
        let fp = f.vreg(Cls::I);
        let x = f.vreg(Cls::I);
        let r = f.vreg(Cls::I);
        let s = f.vreg(Cls::I);
        f.params = vec![fp, x];
        f.rets = vec![Cls::I];
        push(&mut f, b, Inst::IBinI(IOp::Add, U64, x, x, 1), 7);
        push(&mut f, b, Inst::Call(Callee::Indirect(fp), vec![x], vec![r]), 9);
        if invoke {
            let n = f.block();
            let u = f.block();
            f.blocks[b as usize].term = Term::Invoke(n, u);
            push(&mut f, n, Inst::IBinI(IOp::Add, U64, s, r, 2), 11);
            f.blocks[n as usize].term = Term::Ret(vec![s]);
            f.blocks[u as usize].term = Term::Resume;
        } else {
            push(&mut f, b, Inst::IBinI(IOp::Add, U64, s, r, 2), 11);
            f.blocks[b as usize].term = Term::Ret(vec![s]);
        }
        f
    }
    fn calls(f: &Func) -> (usize, usize, usize) {
        let (mut ind, mut direct, mut host) = (0, 0, 0);
        for b in &f.blocks {
            assert_eq!(b.insts.len(), b.pos.len());
            for (k, i) in b.insts.iter().enumerate() {
                match i {
                    Inst::Call(Callee::Indirect(_), ..) => ind += 1,
                    Inst::Call(Callee::Fn(_), ..) => direct += 1,
                    Inst::Call(Callee::Host(_), ..) => host += 1,
                    _ => continue,
                }
                // a call that unwinds stays its block's last instruction
                if matches!(b.term, Term::Invoke(..)) && !matches!(i, Inst::Call(Callee::Host(_), ..)) {
                    assert_eq!(k + 1, b.insts.len());
                }
            }
        }
        (ind, direct, host)
    }

    #[test]
    fn record_offsets() {
        assert_eq!(std::mem::offset_of!(SiteRec, t0), OFF_T0 as usize);
        assert_eq!(std::mem::offset_of!(SiteRec, n0), OFF_N0 as usize);
        assert_eq!(std::mem::offset_of!(SiteRec, fails), OFF_FAILS as usize);
        assert_eq!(std::mem::offset_of!(SiteRec, done), OFF_DONE as usize);
    }

    #[test]
    fn probe_and_guard_shapes_verify() {
        for invoke in [false, true] {
            let mut f = func(invoke);
            probe_site(&mut f, 0, 1, rec(), syms());
            f.verify().unwrap();
            assert_eq!(calls(&f), (1, 0, 2), "probe: the original call, rt_spec_ready, rt_spec_probe");
            for count in [true, false] {
                let mut f = func(invoke);
                guard_site(&mut f, 0, 1, rec(), 42, count, syms());
                f.verify().unwrap();
                assert_eq!(calls(&f), (1, 1, count as usize), "guard: the direct call, the fallback, rt_spec_fail when counting");
                let invokes = f.blocks.iter().filter(|b| matches!(b.term, Term::Invoke(..))).count();
                assert_eq!(invokes, if invoke { 2 } else { 0 }, "both calls keep the unwind edge");
                assert!(f.blocks[0].insts.iter().any(|i| matches!(i, Inst::FnAddr(_, 42))));
            }
        }
    }

    #[test]
    fn ladder() {
        let r = site(u32::MAX - 8, 1);
        let slot_of = |a: u64| (a >= 0x1000).then_some(5u32);
        // profile: 70% one target -> guard
        r.t0.store(0x1000, Ordering::Relaxed); r.n0.store(70, Ordering::Relaxed);
        r.t1.store(0x2000, Ordering::Relaxed); r.n1.store(30, Ordering::Relaxed);
        r.queued.store(true, Ordering::Relaxed);
        assert_eq!(decide(u32::MAX - 8, &slot_of), vec![(0x1000, 5)]);
        assert_eq!(r.state.load(Ordering::Relaxed), GUARD);
        // the failure limit: profile again; the same guess -> stable
        r.state.store(GUARD, Ordering::Relaxed);
        rt_spec_fail(r as *const SiteRec as u64);
        assert_eq!(r.state.load(Ordering::Relaxed), PROFILE);
        assert!(r.queued.load(Ordering::Relaxed));
        r.t0.store(0x1000, Ordering::Relaxed); r.n0.store(60, Ordering::Relaxed);
        decide(u32::MAX - 8, &slot_of);
        assert_eq!(r.state.load(Ordering::Relaxed), STABLE);
        // a new target after a failure re-specialises, MAX_RESPECS times, then the site is off
        for k in 0..=MAX_RESPECS {
            r.state.store(GUARD, Ordering::Relaxed);
            rt_spec_fail(r as *const SiteRec as u64);
            r.t0.store(0x3000 + k as u64, Ordering::Relaxed); r.n0.store(90, Ordering::Relaxed);
            decide(u32::MAX - 8, &slot_of);
            let want = if k < MAX_RESPECS { GUARD } else { OFF };
            assert_eq!(r.state.load(Ordering::Relaxed), want, "respec {}", k);
        }
        // no row reaches 30%: off
        let q = site(u32::MAX - 9, 1);
        q.t0.store(0x1000, Ordering::Relaxed); q.n0.store(20, Ordering::Relaxed);
        q.t1.store(0x2000, Ordering::Relaxed); q.n1.store(20, Ordering::Relaxed);
        q.other.store(60, Ordering::Relaxed); q.queued.store(true, Ordering::Relaxed);
        assert!(decide(u32::MAX - 9, &slot_of).is_empty());
        assert_eq!(q.state.load(Ordering::Relaxed), OFF);
    }
}
