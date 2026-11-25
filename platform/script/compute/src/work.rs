//! Counted work: a kernel counts the ops it actually runs, so its budget is
//! a number of ops, the same on every machine and at any load, never a
//! clock.
//!
//! [`count`] rewrites a kernel's program (after optimization, before any
//! backend sees it) so every backend counts the same way, at block
//! granularity: a register add of a precomputed constant per loop pass,
//! or one count after a loop with a counter, never an add per op.
//!
//! - every pass through a loop body adds that body's ops (its nested loops
//!   aside: they count their own passes) plus two for the iteration, as
//!   [`Program::cost`] charges them; a branch counts the arm it takes;
//! - the element body's own ops (its pass of the element loop) are a
//!   constant the runtime adds per element: no code runs for them;
//! - a call pays its callee's fixed cost. If the callee has variable
//!   work, it returns the two words of that additional count;
//! - a load or store of a host buffer counts [`MEMORY_OPS`] (memory-bound
//!   work counts what it takes, not one op per miss);
//! - a host call counts its call overhead here; its own work, at the input
//!   it was actually given, is added by the runtime (`K_HOST_WORK`);
//! - variable work accumulates in a 64-bit counter per lane. The kernel
//!   stores their sum in the two ctx words starting at `K_WORK`. The host
//!   adds this and the fixed cost per element to the call's total.
//!
//! The counting statements are not counted. Four-wide code counts each
//! lane's own passes (a lane that left a loop adds nothing), so the count
//! is the same for any backend, thread count or slicing.

use crate::ir::{Bin, Block, Cmp, Op, Program, Region, Stmt, Ty, Val, Var};
use crate::lower::kernel::{ELEMENT_CAP, K_WORK};

/// What a load or store of a host buffer counts: about what a cache and
/// TLB miss takes next to an op, so work that walks memory counts what it
/// takes. (The control word, buffer 0, is one op.)
pub const MEMORY_OPS: u64 = 256;

/// What one statement counts by itself (a branch, a loop and a function
/// body aside: a branch is 1 plus its selected arm, a loop counts its
/// passes, a function its body on entry; a host call's own work is added at
/// run time).
pub fn op(s: &Stmt) -> u64 {
    match s {
        Stmt::Def(_, Op::Load { region: Region::Buf(k), .. }) | Stmt::Store { region: Region::Buf(k), .. } if *k > 0 => MEMORY_OPS,
        Stmt::CallHost { args, .. } => 8 + args.len() as u64,
        // Its body counts (as an inlined copy would); the call itself no more.
        Stmt::Call { .. } => 0,
        Stmt::Mark(_) | Stmt::Loop { .. } => 0,
        _ => 1,
    }
}

/// What a kernel's functions count where they are called.
pub struct Fns {
    /// Each function's body by its fixed parts (its loops aside), counted
    /// where it is called: what its inlined copy counts in place.
    pub statics: Vec<u64>,
    /// The function (or a callee) has variable work and returns its count.
    pub looped: Vec<bool>,
}

/// The ops one pass through `b` counts, its nested loops aside, a call at
/// its function's fixed parts ([`Fns::statics`]): the same as its inlined
/// copy, so a program and its inlined form count the same.
pub fn local(b: &[Stmt], fixed: &Fns, consts: &Consts) -> u64 {
    b.iter()
        .enumerate()
        .map(|(k, s)| match s {
            // Both arms pay their common cost here; the dearer arm pays the
            // difference on entry, even when its work is a fixed-trip loop.
            Stmt::If(_, t, e) => 1u64.saturating_add(local(t, fixed, consts).min(local(e, fixed, consts))),
            // A one-pass block (a function's `return` wrapper) counts inline.
            // A one-pass block: its first segment (see `segments`) counts
            // here, the rest where they are reached.
            Stmt::Loop { cap: 1, body } => local(&body[segments(body)[0].clone()], fixed, consts),
            // A loop of a known number of passes: all of them here.
            Stmt::Loop { cap, body } if trips(&b[..k], body, *cap, consts).is_some() => {
                trips(&b[..k], body, *cap, consts).unwrap().saturating_mul(local(body, fixed, consts).saturating_add(2))
            }
            // A loop with a counter: its last pass here (the others after it).
            Stmt::Loop { cap, body } if counter(body, consts).is_some() => {
                let cost = local(body, fixed, consts).saturating_add(2);
                if word_product(*cap, cost) { cost } else { 0 }
            }
            Stmt::Call { f, .. } => fixed.statics.get(*f as usize).copied().unwrap_or(u64::MAX / 4),
            s => op(s),
        })
        .fold(0, u64::saturating_add)
}

/// Each function's fixed parts and whether it loops (see [`Fns`]).
/// Functions are not recursive: resolved callees first.
fn fns(p: &Program) -> Fns {
    fn callees(b: &Block, out: &mut Vec<u16>) {
        for s in b {
            match s {
                Stmt::Call { f, .. } => out.push(*f),
                Stmt::If(_, t, e) => {
                    callees(t, out);
                    callees(e, out);
                }
                Stmt::Loop { body, .. } => callees(body, out),
                _ => {}
            }
        }
    }
    let n = p.funcs.len();
    let mut done = vec![false; n];
    let mut out = Fns { statics: vec![0; n], looped: vec![false; n] };
    for _ in 0..=n {
        for (k, g) in p.funcs.iter().enumerate() {
            if done[k] {
                continue;
            }
            let mut cs = Vec::new();
            callees(&g.body, &mut cs);
            if cs.iter().all(|c| done.get(*c as usize).copied().unwrap_or(false)) {
                let consts = const_ints(&g.body);
                out.statics[k] = local(&g.body, &out, &consts);
                out.looped[k] = dynamic(&g.body, &out, &consts);
                done[k] = true;
            }
        }
    }
    out
}

struct Fresh<'a> {
    vals: &'a mut Vec<Ty>,
}

impl Fresh<'_> {
    fn val(&mut self, t: Ty) -> Val {
        self.vals.push(t);
        Val(self.vals.len() as u32 - 1)
    }

    fn def(&mut self, out: &mut Block, ty: Ty, op: Op) -> Val {
        let v = self.val(ty);
        out.push(Stmt::Def(v, op));
        v
    }

    fn word(&mut self, out: &mut Block, n: u32) -> Val {
        self.def(out, Ty::I32, Op::ConstI(n as i32))
    }
}

// AIR integer registers are 32-bit. Keep an unsigned low and high word so
// scalar, SIMD and wasm need neither floating-point arithmetic nor a new op.
#[derive(Clone, Copy)]
struct Counter { lo: Var, hi: Var }

impl Counter {
    fn new(vars: &mut Vec<Ty>) -> Self {
        let lo = vars.len() as u32;
        vars.extend([Ty::I32, Ty::I32]);
        Self { lo: Var(lo), hi: Var(lo + 1) }
    }

    fn init(self, f: &mut Fresh, n: u64) -> Block {
        let mut out = Vec::new();
        for (var, word) in [(self.lo, n as u32), (self.hi, (n >> 32) as u32)] {
            let v = f.word(&mut out, word);
            out.push(Stmt::Set(var, v));
        }
        out
    }

    fn store(self, f: &mut Fresh, b: &mut Block, word: u32) {
        for (base, var) in [(word, self.lo), (word + 1, self.hi)] {
            let val = f.def(b, Ty::I32, Op::Get(var));
            b.push(Stmt::Store { region: Region::Ctx, base, extent: 1, off: None, val });
        }
    }
}

fn add(f: &mut Fresh, sink: Counter, n: u64) -> Block {
    let mut out = Vec::new();
    let lo = f.word(&mut out, n as u32);
    let hi = ((n >> 32) != 0).then(|| f.word(&mut out, (n >> 32) as u32));
    add_val(f, &mut out, sink, lo, hi);
    out
}

fn add_val(f: &mut Fresh, out: &mut Block, sink: Counter, lo: Val, hi: Option<Val>) {
    let old = f.def(out, Ty::I32, Op::Get(sink.lo));
    let sum = f.def(out, Ty::I32, Op::Bin(Bin::AddI, old, lo));
    // Unsigned sum < old is the carry. Bias both words for AIR's signed compare.
    let sign = f.word(out, 1 << 31);
    let a = f.def(out, Ty::I32, Op::Bin(Bin::XorI, sum, sign));
    let b = f.def(out, Ty::I32, Op::Bin(Bin::XorI, old, sign));
    let carry = f.def(out, Ty::Bool, Op::CmpI(Cmp::Lt, a, b));
    let one = f.word(out, 1);
    let zero = f.word(out, 0);
    let carry = f.def(out, Ty::I32, Op::Sel(carry, one, zero));
    let high = f.def(out, Ty::I32, Op::Get(sink.hi));
    let high = if let Some(hi) = hi { f.def(out, Ty::I32, Op::Bin(Bin::AddI, high, hi)) } else { high };
    let high = f.def(out, Ty::I32, Op::Bin(Bin::AddI, high, carry));
    out.extend([Stmt::Set(sink.lo, sum), Stmt::Set(sink.hi, high)]);
}

/// Reads the two little-endian words written by the counting pass.
pub fn read_count(ctx: &[u32], word: usize) -> u64 {
    u64::from(ctx[word]) | (u64::from(ctx[word + 1]) << 32)
}

pub(crate) fn write_count(ctx: &mut [u32], word: usize, count: u64) {
    ctx[word] = count as u32;
    ctx[word + 1] = (count >> 32) as u32;
}

/// A one-pass block's segments: up to and including each statement that
/// may leave it (a `return`, a `break` out of it), and the rest. Only the
/// first is sure to run when the block is entered.
fn segments(body: &Block) -> Vec<std::ops::Range<usize>> {
    fn leaves(b: &[Stmt], depth: u32) -> bool {
        b.iter().any(|s| match s {
            Stmt::Break(d) | Stmt::Continue(d) => *d >= depth,
            Stmt::If(_, t, e) => leaves(t, depth) || leaves(e, depth),
            Stmt::Loop { body, .. } => leaves(body, depth + 1),
            _ => false,
        })
    }
    let mut out = Vec::new();
    let mut start = 0;
    for (k, s) in body.iter().enumerate() {
        if leaves(std::slice::from_ref(s), 0) && crate::ir::unmarked(&body[k + 1..]).next().is_some() {
            out.push(start..k + 1);
            start = k + 1;
        }
    }
    out.push(start..body.len());
    out
}

/// A program's integer constants by value id.
pub type Consts = std::collections::HashMap<u32, i32>;

/// Every integer constant of a program (wherever optimization moved it).
fn const_ints(b: &Block) -> std::collections::HashMap<u32, i32> {
    fn walk(b: &Block, out: &mut std::collections::HashMap<u32, i32>) {
        for s in b {
            match s {
                Stmt::Def(v, Op::ConstI(c)) => {
                    out.insert(v.0, *c);
                }
                Stmt::If(_, t, e) => {
                    walk(t, out);
                    walk(e, out);
                }
                Stmt::Loop { body, .. } => walk(body, out),
                _ => {}
            }
        }
    }
    let mut out = std::collections::HashMap::new();
    walk(b, &mut out);
    out
}

/// A counter that tells how many passes a loop made: a variable set once
/// in the body, unconditionally at its top level, to itself plus `step`
/// (1 or -1), where nothing continues the loop or leaves an outer one from
/// inside it. Then the passes are at most `(end - start) * step + 1`
/// (exactly, when the loop leaves before the set), counted after the loop
/// instead of in every pass.
fn counter(body: &Block, consts: &Consts) -> Option<(Var, i32)> {
    // Asked again for the same loop while a program is counted (its parts
    // count it, then its passes): the answer for its body as it was.
    let key = body.as_ptr() as usize;
    if let Some(known) = COUNTERS.with(|c| c.borrow().get(&key).copied()) {
        return known;
    }
    let found = find_counter(body, consts);
    COUNTERS.with(|c| c.borrow_mut().insert(key, found));
    found
}

thread_local! {
    /// [`counter`]'s answers during one count, by loop body (bodies are
    /// asked about before they are rewritten: a rewritten body's buffer is
    /// a new allocation, never asked about).
    static COUNTERS: std::cell::RefCell<std::collections::HashMap<usize, Option<(Var, i32)>>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Forgets [`counter`]'s answers (a count starts or ends).
fn forget_counters() {
    COUNTERS.with(|c| c.borrow_mut().clear());
}

fn find_counter(body: &Block, consts: &Consts) -> Option<(Var, i32)> {
    fn sets(b: &Block, v: Var) -> usize {
        b.iter()
            .map(|s| match s {
                Stmt::Set(x, _) => (*x == v) as usize,
                Stmt::If(_, t, e) => sets(t, v) + sets(e, v),
                Stmt::Loop { body, .. } => sets(body, v),
                _ => 0,
            })
            .sum()
    }
    // Exits of this loop (`Break(depth)`), and whether anything continues
    // it or leaves an outer loop.
    fn exits(b: &Block, depth: u32, breaks: &mut bool) -> bool {
        for s in b {
            let ok = match s {
                Stmt::Break(d) if *d == depth => {
                    *breaks = true;
                    true
                }
                Stmt::Break(d) => *d < depth,
                Stmt::Continue(d) => *d < depth,
                Stmt::If(_, t, e) => exits(t, depth, breaks) && exits(e, depth, breaks),
                Stmt::Loop { body, .. } => exits(body, depth + 1, breaks),
                _ => true,
            };
            if !ok {
                return false;
            }
        }
        true
    }
    let defs: std::collections::HashMap<u32, &Op> = body.iter().filter_map(|s| if let Stmt::Def(v, op) = s { Some((v.0, op)) } else { None }).collect();
    for s in body {
        let Stmt::Set(v, next) = s else { continue };
        let Some(Op::Bin(Bin::AddI, a, c)) = defs.get(&next.0) else { continue };
        let step = match (defs.get(&a.0), consts.get(&c.0)) {
            (Some(Op::Get(g)), Some(st)) if g == v => *st,
            _ => continue,
        };
        if !(step == 1 || step == -1) || sets(body, *v) != 1 {
            continue;
        }
        // An exit after the set ends a pass it counted: the count after
        // the loop then charges one pass more than ran (an upper bound).
        let mut breaks = false;
        if !exits(body, 0, &mut breaks) {
            continue;
        }
        return Some((*v, step));
    }
    None
}

/// The passes a loop makes when they are known: a [`counter`] from 0 by 1
/// whose only exit is `counter >= B` for a constant B (`for i in 0..4`):
/// `min(B + 1, cap)`, counted where the loop is, nothing after it.
fn trips(before: &[Stmt], body: &Block, cap: u32, consts: &Consts) -> Option<u64> {
    let (v, step) = counter(body, consts)?;
    if step != 1 || !starts_at_zero(before, v, consts) {
        return None;
    }
    fn breaks(b: &Block, depth: u32) -> usize {
        b.iter()
            .map(|s| match s {
                Stmt::Break(d) => (*d == depth) as usize,
                Stmt::If(_, t, e) => breaks(t, depth) + breaks(e, depth),
                Stmt::Loop { body, .. } => breaks(body, depth + 1),
                _ => 0,
            })
            .sum()
    }
    if breaks(body, 0) != 1 {
        return None;
    }
    let defs: std::collections::HashMap<u32, &Op> = body.iter().filter_map(|s| if let Stmt::Def(x, op) = s { Some((x.0, op)) } else { None }).collect();
    let bound = body.iter().find_map(|s| match s {
        Stmt::If(c, t, e) if matches!((crate::ir::unmarked(t).next(), crate::ir::unmarked(t).nth(1)), (Some(Stmt::Break(0)), None)) && crate::ir::unmarked(e).next().is_none() => match defs.get(&c.0) {
            Some(Op::CmpI(crate::ir::Cmp::Ge, a, n)) if matches!(defs.get(&a.0), Some(Op::Get(g)) if *g == v) => consts.get(&n.0).copied(),
            _ => None,
        },
        _ => None,
    })?;
    Some((bound.max(0) as u64 + 1).min(cap as u64))
}

/// Whether `v` holds 0 at the end of `before`: its last set there is a
/// constant 0, and nothing after that set may set it again.
fn starts_at_zero(before: &[Stmt], v: Var, consts: &Consts) -> bool {
    fn sets(s: &Stmt, v: Var) -> bool {
        match s {
            Stmt::Set(x, _) => *x == v,
            Stmt::If(_, t, e) => t.iter().chain(e).any(|s| sets(s, v)),
            Stmt::Loop { body, .. } => body.iter().any(|s| sets(s, v)),
            _ => false,
        }
    }
    for s in before.iter().rev() {
        match s {
            Stmt::Set(x, val) if *x == v => return consts.get(&val.0) == Some(&0),
            s if sets(s, v) => return false,
            _ => {}
        }
    }
    false
}

// When the product could overflow an AIR integer, count each executed
// iteration directly in the wide accumulator.
fn word_product(cap: u32, cost: u64) -> bool {
    cost <= u64::from(u32::MAX) / u64::from(cap.max(1))
}

/// Counts every loop of `b` (and of loops inside branches): a loop with a
/// [`counter`] counts its passes but the last once after it (one
/// multiply-add; the last is in its parent's fixed count, see [`local`]);
/// any other adds its body's ops in every pass.
fn loops(b: &mut Block, f: &mut Fresh, sink: Counter, fixed: &Fns, consts: &Consts) {
    let mut k = 0;
    while k < b.len() {
        let known = matches!(&b[k], Stmt::Loop { cap, body } if trips(&b[..k], body, *cap, consts).is_some());
        match &mut b[k] {
            Stmt::If(_, t, e) => {
                let taken = local(t, fixed, consts);
                let other = local(e, fixed, consts);
                loops(t, f, sink, fixed, consts);
                loops(e, f, sink, fixed, consts);
                if taken > other {
                    t.splice(0..0, add(f, sink, taken - other));
                } else if other > taken {
                    e.splice(0..0, add(f, sink, other - taken));
                }
            }
            // At most one pass (a function's `return` wrapper): its first
            // segment counts in its parent, each later one where it starts
            // (reached only when no `return` before it left).
            Stmt::Loop { cap: 1, body } => {
                let parts = segments(body);
                let costs: Vec<u64> = parts.iter().map(|r| local(&body[r.clone()], fixed, consts)).collect();
                let mut rebuilt = Vec::with_capacity(body.len());
                let mut rest = std::mem::take(body);
                let mut taken = 0;
                for (n, r) in parts.iter().enumerate() {
                    let mut part: Block = rest.drain(..r.end - taken).collect();
                    taken = r.end;
                    loops(&mut part, f, sink, fixed, consts);
                    if n > 0 && costs[n] > 0 {
                        rebuilt.extend(add(f, sink, costs[n]));
                    }
                    rebuilt.append(&mut part);
                }
                *body = rebuilt;
            }
            Stmt::Loop { cap, body } => {
                let n = local(body, fixed, consts).saturating_add(2);
                let fits_word = word_product(*cap, n);
                let passes = counter(body, consts);
                loops(body, f, sink, fixed, consts);
                if known {
                    // All its passes are counted where it is (see `local`).
                    k += 1;
                    continue;
                }
                match passes {
                    Some((v, step)) if fits_word => {
                        let mut after = Vec::new();
                        let end = f.val(Ty::I32);
                        after.push(Stmt::Def(end, Op::Get(v)));
                        // A counter set to 0 just before the loop (`for i in
                        // 0..n`) counts its passes by its end alone: nothing
                        // stays live through the loop for the count.
                        let d = if step == 1 && starts_at_zero(&b[..k], v, consts) {
                            end
                        } else {
                            let start = f.val(Ty::I32);
                            b.insert(k, Stmt::Def(start, Op::Get(v)));
                            k += 1;
                            let d = f.val(Ty::I32);
                            after.push(Stmt::Def(d, if step == 1 { Op::Bin(Bin::SubI, end, start) } else { Op::Bin(Bin::SubI, start, end) }));
                            d
                        };
                        let c = f.word(&mut after, n as u32);
                        let charge = f.def(&mut after, Ty::I32, Op::Bin(Bin::MulI, d, c));
                        add_val(f, &mut after, sink, charge, None);
                        let len = after.len();
                        b.splice(k + 1..k + 1, after);
                        k += len;
                    }
                    _ => {
                        let mut counted = add(f, sink, n);
                        counted.append(body);
                        *body = counted;
                    }
                }
            }
            _ => {}
        }
        k += 1;
    }
}

/// Add the variable part of each callee's count when the call runs.
fn calls(b: &mut Block, f: &mut Fresh, sink: Counter, fixed: &Fns) {
    let mut k = 0;
    while k < b.len() {
        match &mut b[k] {
            Stmt::If(_, t, e) => {
                calls(t, f, sink, fixed);
                calls(e, f, sink, fixed);
            }
            Stmt::Loop { body, .. } => calls(body, f, sink, fixed),
            Stmt::Call { f: callee, rets, .. } if fixed.looped.get(*callee as usize).copied().unwrap_or(true) => {
                let lo = f.val(Ty::I32);
                let hi = f.val(Ty::I32);
                rets.extend([lo, hi]);
                let mut added = Vec::new();
                add_val(f, &mut added, sink, lo, Some(hi));
                let n = added.len();
                b.splice(k + 1..k + 1, added);
                k += n;
            }
            _ => {}
        }
        k += 1;
    }
}

/// A function with variable work returns that count. The caller pays its fixed part.
fn count_functions(p: &mut Program) -> Fns {
    let fixed = fns(p);
    for (k, g) in p.funcs.iter_mut().enumerate() {
        if !fixed.looped[k] {
            continue;
        }
        // Its fixed parts are counted where it is called; it counts what
        // its loops (and looping callees) run, and returns that.
        let fv = Counter::new(&mut g.vars);
        let consts = const_ints(&g.body);
        let mut f = Fresh { vals: &mut g.vals };
        loops(&mut g.body, &mut f, fv, &fixed, &consts);
        calls(&mut g.body, &mut f, fv, &fixed);
        let mut body = fv.init(&mut f, 0);
        body.append(&mut g.body);
        for var in [fv.lo, fv.hi] {
            g.results.push(f.def(&mut body, Ty::I32, Op::Get(var)));
        }
        g.body = body;
    }
    fixed
}

/// Whether a loop, branch or callee can vary the cost of this block.
fn dynamic(b: &[Stmt], fixed: &Fns, consts: &Consts) -> bool {
    b.iter().enumerate().any(|(k, s)| match s {
        Stmt::Loop { cap: 1, body } => dynamic(body, fixed, consts) || segments(body).len() > 1,
        Stmt::Loop { cap, body } if trips(&b[..k], body, *cap, consts).is_some() => dynamic(body, fixed, consts),
        Stmt::Loop { .. } => true,
        Stmt::If(_, t, e) => local(t, fixed, consts) != local(e, fixed, consts) || dynamic(t, fixed, consts) || dynamic(e, fixed, consts),
        Stmt::Call { f, .. } => fixed.looped.get(*f as usize).copied().unwrap_or(true),
        _ => false,
    })
}

/// Makes `p` (a kernel's entry program) count its work (see the module
/// docs). Returns what each element counts by itself (its pass of the
/// element loop, which the runtime adds per element), or None when `p` is
/// not a kernel's element loop.
pub(crate) fn count(p: &mut Program) -> Option<u64> {
    forget_counters();
    let r = count_program(p);
    forget_counters();
    r
}

fn count_program(p: &mut Program) -> Option<u64> {
    let fixed = count_functions(p);
    let consts = const_ints(&p.body);
    let at = p.body.iter().rposition(|s| !matches!(s, Stmt::Mark(_)))?;
    let Stmt::Loop { cap: ELEMENT_CAP, body: element } = &p.body[at] else { return None };
    let Some(Stmt::Set(i, _)) = crate::ir::unmarked(element).next_back() else { return None };
    // The final range check is outside the n elements billed by the host.
    let start = crate::spmd::element_range_check(&p.body[..at], element, *i).map_or(0, |(k, _)| k + 1);
    let n = local(element, &fixed, &consts).saturating_add(2);
    let dynamic = dynamic(&element[start..], &fixed, &consts);
    if !dynamic {
        return Some(n);
    }
    // What runs a varying amount (loops, functions with loops) counts into
    // a variable that runs on across the run's elements (one per lane in
    // four-wide code), stored once after the loop.
    let wv = Counter::new(&mut p.vars);
    let mut f = Fresh { vals: &mut p.vals };
    let Some(Stmt::Loop { body: element, .. }) = p.body.get_mut(at) else { return None };
    let mut payload = element.split_off(start);
    loops(&mut payload, &mut f, wv, &fixed, &consts);
    calls(&mut payload, &mut f, wv, &fixed);
    element.append(&mut payload);
    p.body.splice(at..at, wv.init(&mut f, 0));
    wv.store(&mut f, &mut p.body, K_WORK);
    Some(n)
}

/// Makes `p` (any program: an audio shader's render, say) count the ops it
/// runs, as [`count`] does for a kernel's elements, and store the total of
/// each run in two ctx words starting at `ctx_word`.
pub fn count_runs(p: &mut Program, ctx_word: u32) {
    forget_counters();
    count_program_runs(p, ctx_word);
    forget_counters();
}

fn count_program_runs(p: &mut Program, ctx_word: u32) {
    let fixed = count_functions(p);
    let consts = const_ints(&p.body);
    let wv = Counter::new(&mut p.vars);
    let mut f = Fresh { vals: &mut p.vals };
    let n = local(&p.body, &fixed, &consts);
    loops(&mut p.body, &mut f, wv, &fixed, &consts);
    calls(&mut p.body, &mut f, wv, &fixed);
    let mut body = wv.init(&mut f, n);
    body.append(&mut p.body);
    wv.store(&mut f, &mut body, ctx_word);
    p.body = body;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ir, lower, parse};

    #[test]
    fn a_guard_charges_only_the_path_run() {
        fn play(src: &str, left: f32, frames: u32) -> (u64, f32) {
            let tokens = parse::lex(src).unwrap();
            let items = parse::Parser::new(&tokens).items().unwrap();
            let mut shader = lower::lower(&items, src.len() + 1).unwrap();
            let word = lower::CTX_PARAMS;
            count_runs(&mut shader.render, word);
            let mut ctx = vec![0; word as usize + 2];
            let mut mem = ir::Mem { ctx: &mut ctx, state: &mut shader.state_init,
                shared: ir::Shared::Read(&shader.shared_init), bufs: &[] };
            let (mut l, mut r) = ([0.0; 2], [0.0; 2]);
            ir::run(&shader.render, &mut vec![0; shader.render.scratch_words()], &mut mem,
                &mut ir::Io { ins: &[&[left; 2], &[1.0; 2]], outs: &mut [&mut l, &mut r] }, frames);
            (read_count(&ctx, word as usize), l[0])
        }
        let shader = |body: &str| format!("var t=[0.0;128]\nfn effect(l,r){{ {body} t[0] }}");
        let whole_loop = "for i in 0..128 { t[i]+=r }";
        for visits in [1, 3] {
            let source = |body: &str| {
                if visits == 1 { shader(body) } else { shader(&format!("for j in 0..{visits} {{ {body} }}")) }
            };
            let guarded = source(&format!("if l>0 {{ {whole_loop} }}"));
            let reference = source("if l>0 { t[0]+=r }");
            let (skipped, zero) = play(&guarded, -1.0, 1);
            let (taken, output) = play(&guarded, 1.0, 1);
            assert_eq!(zero, 0.0);
            assert_eq!(output, visits as f32);
            assert_eq!(skipped, play(&reference, -1.0, 1).0, "an unvisited loop was charged");
            // Compare an additional active frame, excluding the render loop's
            // final bounds check and its conservative fixed cost per invocation.
            let unguarded = source(whole_loop);
            let full = play(&unguarded, 1.0, 2).0 - play(&unguarded, 1.0, 1).0;
            let active = play(&guarded, 1.0, 2).0 - taken;
            assert!(active >= full && full >= 128 * visits, "guarded {active}, unconditional {full}");
        }
    }

    #[test]
    fn counted_work_carries_into_the_high_word() {
        let mut p = Program::default();
        let sink = Counter::new(&mut p.vars);
        let mut f = Fresh { vals: &mut p.vals };
        p.body = sink.init(&mut f, u64::from(u32::MAX) - 1);
        p.body.extend(add(&mut f, sink, (2u64 << 32) + 3));
        sink.store(&mut f, &mut p.body, 0);
        let mut ctx = [0; 2];
        let mut mem = ir::Mem { ctx: &mut ctx, state: &mut [], shared: ir::Shared::Read(&[]), bufs: &[] };
        ir::run(&p, &mut vec![0; p.scratch_words()], &mut mem, &mut ir::Io::default(), 1);
        assert_eq!(read_count(&ctx, 0), (3u64 << 32) + 1);
    }
}
