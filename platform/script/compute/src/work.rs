//! Counted work: a kernel counts the ops it actually runs, so its budget is
//! a number of ops, the same on every machine and at any load, never a
//! clock.
//!
//! [`count`] rewrites a kernel's program (after optimization, before any
//! backend sees it) so every backend counts the same way:
//!
//! - every pass through a loop body adds that body's ops (its nested loops
//!   aside: they count their own passes) plus two for the iteration, as
//!   [`Program::cost`] charges them; a branch counts its dearer side;
//! - the element body counts its own ops once per element;
//! - a function counts its body's ops on entry and its loops as above
//!   (functions add into a frame word every function shares; the entry
//!   program into a variable);
//! - a load or store of a host buffer counts [`MEMORY_OPS`] (memory-bound
//!   work counts what it takes, not one op per miss);
//! - a host call counts its call overhead here; its own work, at the input
//!   it was actually given, is added by the runtime (`K_HOST_WORK`);
//! - at the end of each element its count goes to the hidden work buffer
//!   at `element % CHUNK`, which the runtime sums after each slice of
//!   elements (and stops the call once the sum passes its limit).
//!
//! The counting statements are not counted. Four-wide code counts each
//! lane's own passes (a lane that left a loop adds nothing), so the count
//! is the same for any backend, thread count or slicing.

use crate::ir::{Bin, Block, Op, Program, Region, Stmt, Ty, Val, Var};
use crate::kernel::CHUNK;
use crate::lower::kernel::{ELEMENT_CAP, K_BASE};

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
        Stmt::Call { args, .. } => 4 + args.len() as u64,
        Stmt::Loop { .. } => 0,
        _ => 1,
    }
}

/// The ops one pass through `b` counts, its nested loops aside.
pub fn local(b: &Block) -> u64 {
    b.iter()
        .map(|s| match s {
            Stmt::If(_, t, e) => 1 + local(t).max(local(e)),
            s => op(s),
        })
        .sum()
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

/// Where a program adds its counts: a variable (the entry program) or the
/// shared frame word (functions).
#[derive(Clone, Copy)]
enum Sink {
    Var(Var),
    Frame(u32),
}

fn add(f: &mut Fresh, sink: Sink, n: u64) -> Block {
    let mut out = Vec::new();
    let c = f.konst(&mut out, n);
    let old = f.val(Ty::I32);
    let new = f.val(Ty::I32);
    match sink {
        Sink::Var(v) => {
            out.push(Stmt::Def(old, Op::Get(v)));
            out.push(Stmt::Def(new, Op::Bin(Bin::AddI, old, c)));
            out.push(Stmt::Set(v, new));
        }
        Sink::Frame(w) => {
            out.push(Stmt::Def(old, Op::Load { region: Region::Frame, base: w, extent: 1, off: None }));
            out.push(Stmt::Def(new, Op::Bin(Bin::AddI, old, c)));
            out.push(Stmt::Store { region: Region::Frame, base: w, extent: 1, off: None, val: new });
        }
    }
    out
}

/// Counts every loop of `b` (and of loops inside branches).
fn loops(b: &mut Block, f: &mut Fresh, sink: Sink) {
    for s in b.iter_mut() {
        match s {
            Stmt::If(_, t, e) => {
                loops(t, f, sink);
                loops(e, f, sink);
            }
            Stmt::Loop { body, .. } => {
                let n = local(body) + 2;
                loops(body, f, sink);
                let mut counted = add(f, sink, n);
                counted.append(body);
                *body = counted;
            }
            _ => {}
        }
    }
}

/// Makes `p` (a kernel's entry program) count its work into host buffer
/// `work` (see the module docs). False when `p` is not a kernel's element
/// loop.
pub(crate) fn count(p: &mut Program, work: u8) -> bool {
    let frame_word = p.frame_words;
    p.frame_words += 1;
    for g in &mut p.funcs {
        let mut f = Fresh { vals: &mut g.vals };
        let n = local(&g.body);
        loops(&mut g.body, &mut f, Sink::Frame(frame_word));
        let mut body = add(&mut f, Sink::Frame(frame_word), n);
        body.append(&mut g.body);
        g.body = body;
    }
    p.vars.push(Ty::I32);
    let wv = Var(p.vars.len() as u32 - 1);
    let mut body = std::mem::take(&mut p.body);
    let ok = (|| {
        let Some(Stmt::Loop { cap: ELEMENT_CAP, body: element }) = body.last_mut() else { return false };
        // The element counter: the body's last statement steps it.
        let Some(&Stmt::Set(i, _)) = element.last() else { return false };
        let mut f = Fresh { vals: &mut p.vals };
        let n = local(element) + 2;
        loops(element, &mut f, Sink::Var(wv));
        // After the top checks (the end of the elements, the control word):
        // four-wide code looks for the first of them at the top.
        let checks: Vec<usize> = element
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s, Stmt::If(_, t, e) if matches!(t[..], [Stmt::Break(0)]) && e.is_empty()))
            .map(|(k, _)| k)
            .take(2)
            .collect();
        let at = checks.last().map_or(0, |k| k + 1);
        let mut reset = Vec::new();
        let c = f.konst(&mut reset, n);
        reset.push(Stmt::Set(wv, c));
        let zero = f.konst(&mut reset, 0);
        reset.push(Stmt::Store { region: Region::Frame, base: frame_word, extent: 1, off: None, val: zero });
        element.splice(at..at, reset);
        // Before the step: this element's count into the work buffer.
        let mut store = Vec::new();
        let own = f.val(Ty::I32);
        store.push(Stmt::Def(own, Op::Get(wv)));
        let called = f.val(Ty::I32);
        store.push(Stmt::Def(called, Op::Load { region: Region::Frame, base: frame_word, extent: 1, off: None }));
        let total = f.val(Ty::I32);
        store.push(Stmt::Def(total, Op::Bin(Bin::AddI, own, called)));
        let base = f.val(Ty::I32);
        store.push(Stmt::Def(base, Op::Load { region: Region::Ctx, base: K_BASE, extent: 1, off: None }));
        let iv = f.val(Ty::I32);
        store.push(Stmt::Def(iv, Op::Get(i)));
        let e = f.val(Ty::I32);
        store.push(Stmt::Def(e, Op::Bin(Bin::AddI, base, iv)));
        let mask = f.konst(&mut store, CHUNK as u64 - 1);
        let slot = f.val(Ty::I32);
        store.push(Stmt::Def(slot, Op::Bin(Bin::AndI, e, mask)));
        store.push(Stmt::Store { region: Region::Buf(work), base: 0, extent: u32::MAX, off: Some(slot), val: total });
        let step = element.len() - 1;
        element.splice(step..step, store);
        // The variable starts at 0 before the loop (four-wide code allows
        // only definitions and sets there).
        let z = f.val(Ty::I32);
        let at = body.len() - 1;
        body.splice(at..at, [Stmt::Def(z, Op::ConstI(0)), Stmt::Set(wv, z)]);
        true
    })();
    p.body = body;
    ok
}

/// Makes `p` (any program: an audio shader's render, say) count the ops it
/// runs, as [`count`] does for a kernel's elements, and store the total of
/// each run in ctx word `ctx_word` (which the caller adds to its region).
/// Branches count their dearer side, loops each pass, functions their body
/// on entry; host-buffer accesses [`MEMORY_OPS`].
pub fn count_runs(p: &mut Program, ctx_word: u32) {
    let frame_word = p.frame_words;
    p.frame_words += 1;
    for g in &mut p.funcs {
        let mut f = Fresh { vals: &mut g.vals };
        let n = local(&g.body);
        loops(&mut g.body, &mut f, Sink::Frame(frame_word));
        let mut body = add(&mut f, Sink::Frame(frame_word), n);
        body.append(&mut g.body);
        g.body = body;
    }
    p.vars.push(Ty::I32);
    let wv = Var(p.vars.len() as u32 - 1);
    let mut f = Fresh { vals: &mut p.vals };
    let n = local(&p.body);
    loops(&mut p.body, &mut f, Sink::Var(wv));
    let mut start = Vec::new();
    let c = f.konst(&mut start, n);
    start.push(Stmt::Set(wv, c));
    let zero = f.konst(&mut start, 0);
    start.push(Stmt::Store { region: Region::Frame, base: frame_word, extent: 1, off: None, val: zero });
    let own = f.val(Ty::I32);
    let called = f.val(Ty::I32);
    let total = f.val(Ty::I32);
    let end = [
        Stmt::Def(own, Op::Get(wv)),
        Stmt::Def(called, Op::Load { region: Region::Frame, base: frame_word, extent: 1, off: None }),
        Stmt::Def(total, Op::Bin(Bin::AddI, own, called)),
        Stmt::Store { region: Region::Ctx, base: ctx_word, extent: 1, off: None, val: total },
    ];
    start.append(&mut p.body);
    start.extend(end);
    p.body = start;
}
