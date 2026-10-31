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
//!   [`Program::cost`] charges them; a branch counts its dearer side;
//! - the element body's own ops (its pass of the element loop) are a
//!   constant the runtime adds per element: no code runs for them;
//! - a call counts its function's body in place, its loops aside, exactly
//!   as the inlined copy native code runs would (so a program and its
//!   inlined form count the same); a function with loops counts them into
//!   a variable of its own and returns that as an extra result, which the
//!   caller adds; a loop-free one runs no counting code;
//! - a load or store of a host buffer counts [`MEMORY_OPS`] (memory-bound
//!   work counts what it takes, not one op per miss);
//! - a host call counts its call overhead here; its own work, at the input
//!   it was actually given, is added by the runtime (`K_HOST_WORK`);
//! - what varies (loops, functions with loops) counts into a variable that
//!   runs on across a run's elements (four-wide code keeps one per lane)
//!   and is stored once after the element loop, in ctx word `K_WORK` (the
//!   lanes summed); the runtime adds it and the elements' constant after
//!   each slice of elements, and stops the call once the sum passes its
//!   limit. A kernel without loops runs no counting code at all.
//!
//! The counting statements are not counted. Four-wide code counts each
//! lane's own passes (a lane that left a loop adds nothing), so the count
//! is the same for any backend, thread count or slicing.

use crate::ir::{Bin, Block, Op, Program, Region, Stmt, Ty, Val, Var};
use crate::lower::kernel::{ELEMENT_CAP, K_WORK};

/// What a load or store of a host buffer counts: about what a cache and
/// TLB miss takes next to an op, so work that walks memory counts what it
/// takes. (The control word, buffer 0, is one op.)
pub const MEMORY_OPS: u64 = 256;

/// What one statement counts by itself (a branch, a loop and a function
/// body aside: a branch is 1 plus its dearer side, a loop counts its
/// passes, a function its body on entry; a host call's own work is added at
/// run time).
pub fn op(s: &Stmt) -> u64 {
    match s {
        Stmt::Def(_, Op::Load { region: Region::Buf(k), .. }) | Stmt::Store { region: Region::Buf(k), .. } if *k > 0 => MEMORY_OPS,
        Stmt::CallHost { args, .. } => 8 + args.len() as u64,
        // Its body counts (as an inlined copy would); the call itself no more.
        Stmt::Call { .. } => 0,
        Stmt::Loop { .. } => 0,
        _ => 1,
    }
}

/// What a kernel's functions count where they are called.
pub struct Fns {
    /// Each function's body by its fixed parts (its loops aside), counted
    /// where it is called: what its inlined copy counts in place.
    pub statics: Vec<u64>,
    /// The function (or one it calls) has loops: it returns what they
    /// counted as an extra result.
    pub looped: Vec<bool>,
}

/// The ops one pass through `b` counts, its nested loops aside, a call at
/// its function's fixed parts ([`Fns::statics`]): the same as its inlined
/// copy, so a program and its inlined form count the same.
pub fn local(b: &[Stmt], fixed: &Fns, consts: &Consts) -> u64 {
    b.iter()
        .enumerate()
        .map(|(k, s)| match s {
            Stmt::If(_, t, e) => 1 + local(t, fixed, consts).max(local(e, fixed, consts)),
            // A one-pass block (a function's `return` wrapper) counts inline.
            // A one-pass block: its first segment (see `segments`) counts
            // here, the rest where they are reached.
            Stmt::Loop { cap: 1, body } => local(&body[segments(body)[0].clone()], fixed, consts),
            // A loop of a known number of passes: all of them here.
            Stmt::Loop { cap, body } if trips(&b[..k], body, *cap, consts).is_some() => {
                trips(&b[..k], body, *cap, consts).unwrap().saturating_mul(local(body, fixed, consts) + 2)
            }
            // A loop with a counter: its last pass here (the others after it).
            Stmt::Loop { body, .. } if counter(body, consts).is_some() => local(body, fixed, consts) + 2,
            Stmt::Call { f, .. } => fixed.statics.get(*f as usize).copied().unwrap_or(u64::MAX / 4),
            s => op(s),
        })
        .sum()
}

/// Each function's fixed parts and whether it loops (see [`Fns`]).
/// Functions are not recursive: resolved callees first.
fn fns(p: &Program) -> Fns {
    // A loop counted at run time (a known number of passes is fixed).
    fn has_loop(b: &Block, consts: &Consts) -> bool {
        b.iter().enumerate().any(|(k, s)| match s {
            Stmt::Loop { cap: 1, body } => has_loop(body, consts) || segments(body).len() > 1,
            Stmt::Loop { cap, body } if trips(&b[..k], body, *cap, consts).is_some() => has_loop(body, consts),
            Stmt::Loop { .. } => true,
            Stmt::If(_, t, e) => has_loop(t, consts) || has_loop(e, consts),
            _ => false,
        })
    }
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
                out.statics[k] = local(&g.body, &out, &const_ints(&g.body));
                out.looped[k] = has_loop(&g.body, &const_ints(&g.body)) || cs.iter().any(|c| out.looped[*c as usize]);
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

    fn konst(&mut self, out: &mut Block, n: u64) -> Val {
        let c = self.val(Ty::I32);
        out.push(Stmt::Def(c, Op::ConstI(n.min(i32::MAX as u64) as i32)));
        c
    }
}

/// `sink += n`.
fn add(f: &mut Fresh, sink: Var, n: u64) -> Block {
    let mut out = Vec::new();
    let c = f.konst(&mut out, n);
    out.extend(add_val(f, sink, c));
    out
}

/// `sink += v`.
fn add_val(f: &mut Fresh, sink: Var, v: Val) -> Block {
    let old = f.val(Ty::I32);
    let new = f.val(Ty::I32);
    vec![Stmt::Def(old, Op::Get(sink)), Stmt::Def(new, Op::Bin(Bin::AddI, old, v)), Stmt::Set(sink, new)]
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
        if leaves(std::slice::from_ref(s), 0) && k + 1 < body.len() {
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
        Stmt::If(c, t, e) if matches!(t[..], [Stmt::Break(0)]) && e.is_empty() => match defs.get(&c.0) {
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

/// Counts every loop of `b` (and of loops inside branches): a loop with a
/// [`counter`] counts its passes but the last once after it (one
/// multiply-add; the last is in its parent's fixed count, see [`local`]);
/// any other adds its body's ops in every pass.
fn loops(b: &mut Block, f: &mut Fresh, sink: Var, fixed: &Fns, consts: &Consts) {
    let mut k = 0;
    while k < b.len() {
        let known = matches!(&b[k], Stmt::Loop { cap, body } if trips(&b[..k], body, *cap, consts).is_some());
        match &mut b[k] {
            Stmt::If(_, t, e) => {
                loops(t, f, sink, fixed, consts);
                loops(e, f, sink, fixed, consts);
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
            Stmt::Loop { body, .. } => {
                let n = local(body, fixed, consts) + 2;
                let passes = counter(body, consts);
                loops(body, f, sink, fixed, consts);
                if known {
                    // All its passes are counted where it is (see `local`).
                    k += 1;
                    continue;
                }
                match passes {
                    Some((v, step)) => {
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
                        let c = f.konst(&mut after, n);
                        let old = f.val(Ty::I32);
                        after.push(Stmt::Def(old, Op::Get(sink)));
                        let m = f.val(Ty::I32);
                        after.push(Stmt::Def(m, Op::Fma(crate::ir::Fma::MulAddI, d, c, old)));
                        after.push(Stmt::Set(sink, m));
                        let len = after.len();
                        b.splice(k + 1..k + 1, after);
                        k += len;
                    }
                    None => {
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

/// Every call of a function with loops takes its callee's count (an extra
/// result) and adds it.
fn calls(b: &mut Block, f: &mut Fresh, sink: Var, fixed: &Fns) {
    let mut k = 0;
    while k < b.len() {
        match &mut b[k] {
            Stmt::If(_, t, e) => {
                calls(t, f, sink, fixed);
                calls(e, f, sink, fixed);
            }
            Stmt::Loop { body, .. } => calls(body, f, sink, fixed),
            Stmt::Call { f: callee, rets, .. } if fixed.looped.get(*callee as usize).copied().unwrap_or(true) => {
                let r = f.val(Ty::I32);
                rets.push(r);
                let added = add_val(f, sink, r);
                let n = added.len();
                b.splice(k + 1..k + 1, added);
                k += n;
            }
            _ => {}
        }
        k += 1;
    }
}

/// A function with loops counts what they run into a variable of its own
/// and returns it; its fixed parts, and a loop-free function, count where
/// they are called.
fn count_functions(p: &mut Program) -> Fns {
    let fixed = fns(p);
    for (k, g) in p.funcs.iter_mut().enumerate() {
        if !fixed.looped[k] {
            continue;
        }
        // Its fixed parts are counted where it is called; it counts what
        // its loops (and looping callees) run, and returns that.
        g.vars.push(Ty::I32);
        let fv = Var(g.vars.len() as u32 - 1);
        let consts = const_ints(&g.body);
        let mut f = Fresh { vals: &mut g.vals };
        loops(&mut g.body, &mut f, fv, &fixed, &consts);
        calls(&mut g.body, &mut f, fv, &fixed);
        let mut body = Vec::new();
        let c = f.konst(&mut body, 0);
        body.push(Stmt::Set(fv, c));
        body.append(&mut g.body);
        let r = f.val(Ty::I32);
        body.push(Stmt::Def(r, Op::Get(fv)));
        g.body = body;
        g.results.push(r);
    }
    fixed
}

/// Whether `b` counts anything at run time: a loop, or a call of a
/// function with loops.
fn dynamic(b: &Block, fixed: &Fns, consts: &Consts) -> bool {
    b.iter().enumerate().any(|(k, s)| match s {
        Stmt::Loop { cap: 1, body } => dynamic(body, fixed, consts) || segments(body).len() > 1,
        Stmt::Loop { cap, body } if trips(&b[..k], body, *cap, consts).is_some() => dynamic(body, fixed, consts),
        Stmt::Loop { .. } => true,
        Stmt::If(_, t, e) => dynamic(t, fixed, consts) || dynamic(e, fixed, consts),
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
    let n = {
        let Some(Stmt::Loop { cap: ELEMENT_CAP, body: element }) = p.body.last() else { return None };
        if !matches!(element.last(), Some(Stmt::Set(..))) {
            return None;
        }
        local(element, &fixed, &consts) + 2
    };
    let dynamic = matches!(p.body.last(), Some(Stmt::Loop { body, .. }) if dynamic(body, &fixed, &consts));
    if !dynamic {
        return Some(n);
    }
    // What runs a varying amount (loops, functions with loops) counts into
    // a variable that runs on across the run's elements (one per lane in
    // four-wide code), stored once after the loop.
    p.vars.push(Ty::I32);
    let wv = Var(p.vars.len() as u32 - 1);
    let mut f = Fresh { vals: &mut p.vals };
    let at = p.body.len() - 1;
    let z = f.val(Ty::I32);
    let Some(Stmt::Loop { body: element, .. }) = p.body.last_mut() else { return None };
    loops(element, &mut f, wv, &fixed, &consts);
    calls(element, &mut f, wv, &fixed);
    let total = f.val(Ty::I32);
    // Before the loop the count starts at 0 (four-wide code allows only
    // definitions and sets there).
    p.body.splice(at..at, [Stmt::Def(z, Op::ConstI(0)), Stmt::Set(wv, z)]);
    p.body.push(Stmt::Def(total, Op::Get(wv)));
    p.body.push(Stmt::Store { region: Region::Ctx, base: K_WORK, extent: 1, off: None, val: total });
    Some(n)
}

/// Makes `p` (any program: an audio shader's render, say) count the ops it
/// runs, as [`count`] does for a kernel's elements, and store the total of
/// each run in ctx word `ctx_word` (which the caller adds to its region).
pub fn count_runs(p: &mut Program, ctx_word: u32) {
    forget_counters();
    count_program_runs(p, ctx_word);
    forget_counters();
}

fn count_program_runs(p: &mut Program, ctx_word: u32) {
    let fixed = count_functions(p);
    let consts = const_ints(&p.body);
    p.vars.push(Ty::I32);
    let wv = Var(p.vars.len() as u32 - 1);
    let mut f = Fresh { vals: &mut p.vals };
    let n = local(&p.body, &fixed, &consts);
    loops(&mut p.body, &mut f, wv, &fixed, &consts);
    calls(&mut p.body, &mut f, wv, &fixed);
    let mut body = Vec::new();
    let c = f.konst(&mut body, n);
    body.push(Stmt::Set(wv, c));
    body.append(&mut p.body);
    let total = f.val(Ty::I32);
    body.push(Stmt::Def(total, Op::Get(wv)));
    body.push(Stmt::Store { region: Region::Ctx, base: ctx_word, extent: 1, off: None, val: total });
    p.body = body;
}
