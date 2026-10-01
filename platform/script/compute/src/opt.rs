//! AIR optimisation passes that run after lowering and before every
//! backend. They change what gets computed where, never a value: every
//! program gives the same bits before and after, on every backend and the
//! interpreter.
//!
//! - [`forward_sets`]: a `Get` of a variable whose last `Set` is known on
//!   every path to it reads the set value directly (no copy through the
//!   variable).
//! - [`licm`]: pure definitions inside a repeating loop whose operands do
//!   not change in it are hoisted in front of the loop: constants, params
//!   (ctx words the loop never stores), table and read-only buffer loads at
//!   invariant addresses, and the arithmetic on them. AIR's ops are total
//!   (no traps), so hoisting out of a branch is safe.
//! - [`cse`]: a pure definition equal to one that dominates it (defined
//!   earlier in its block or an enclosing one) is replaced by it.
//!
//! The control word (host buffer 0, the cancel token) is never hoisted or
//! merged: it is polled every element.

use crate::ir::{self, Block, Op, Program, Region, Stmt, Val};
use std::collections::{HashMap, HashSet};

thread_local! {
    /// What the functions of the program being optimized may write, all
    /// together: a call's effect (each function's passes see only its
    /// own body). Empty when the program has no functions.
    static CALLEE_WRITES: std::cell::RefCell<Writes> = std::cell::RefCell::new(Writes::default());
    /// Frame words some function of the program reads (and whether one
    /// reads at a dynamic offset): the frame is the whole call's, so a
    /// store one function makes may be read by another.
    static FRAME_READS: std::cell::RefCell<(HashSet<u32>, bool)> = std::cell::RefCell::new((HashSet::new(), false));
}

/// What any function of `p` writes.
fn callee_writes(p: &Program) -> Writes {
    let mut out = Writes::default();
    for g in &p.funcs {
        out.walk(&g.body);
    }
    out
}

/// Runs every pass (then dead code elimination) to a fixpoint-ish order,
/// on the program and each of its functions (a call is opaque: what a
/// function computes is optimized in the function).
pub fn optimize(p: &mut Program) {
    let mut written = callee_writes(p);
    written.bufs.extend(Writes::of(&p.body).bufs);
    CALLEE_WRITES.with(|w| *w.borrow_mut() = written);
    let mut reads = (HashSet::new(), false);
    for g in &p.funcs {
        frame_loads(&g.body, &mut reads.0, &mut reads.1);
    }
    if !p.funcs.is_empty() {
        frame_loads(&p.body, &mut reads.0, &mut reads.1);
    }
    FRAME_READS.with(|f| *f.borrow_mut() = reads);
    let mut funcs = std::mem::take(&mut p.funcs);
    for g in &mut funcs {
        optimize_one(g);
    }
    p.funcs = funcs;
    optimize_one(p);
    CALLEE_WRITES.with(|w| *w.borrow_mut() = Writes::default());
    FRAME_READS.with(|f| *f.borrow_mut() = (HashSet::new(), false));
}

fn optimize_one(p: &mut Program) {
    forward_sets(p);
    licm(p);
    if_convert(p);
    forward_sets(p);
    cse(p);
    dce(p);
    fold_offsets(p);
    forward_stores(p);
    dce(p);
    dead_frame_stores(p);
    dead_loops(p);
    drop_wraps(p);
    strength_reduce(p);
    cse(p);
    dce(p);
    cluster_stores(p);
    peel_offsets(p);
    cluster_loads(p);
    dce(p);
    fuse_mla(p);
    int_reassoc(p);
}

/// Integer constants fold through wrapping arithmetic (exact in 32-bit
/// two's complement): (x + c1) + c2 is x + (c1 + c2), and (x + c1) * m + k
/// (a hash of a shifted key) is x * m + (c1 * m + k).
pub fn int_reassoc(p: &mut Program) {
    let mut consts: HashMap<u32, i32> = HashMap::new();
    let mut adds: HashMap<u32, (Val, i32)> = HashMap::new();
    fn find(b: &Block, consts: &mut HashMap<u32, i32>, adds: &mut HashMap<u32, (Val, i32)>) {
        for s in b {
            match s {
                Stmt::Def(v, Op::ConstI(c)) => {
                    consts.insert(v.0, *c);
                }
                Stmt::Def(v, Op::Bin(ir::Bin::AddI, x, y)) => {
                    if let Some(c) = consts.get(&y.0) {
                        adds.insert(v.0, (*x, *c));
                    } else if let Some(c) = consts.get(&x.0) {
                        adds.insert(v.0, (*y, *c));
                    }
                }
                Stmt::If(_, t, e) => {
                    find(t, consts, adds);
                    find(e, consts, adds);
                }
                Stmt::Loop { body, .. } => find(body, consts, adds),
                _ => {}
            }
        }
    }
    find(&p.body, &mut consts, &mut adds);
    let mut new_consts: HashMap<i32, Val> = HashMap::new();
    let mut defs = Vec::new();
    let mut konst = |c: i32, vals: &mut Vec<ir::Ty>| -> Val {
        *new_consts.entry(c).or_insert_with(|| {
            vals.push(ir::Ty::I32);
            let v = Val(vals.len() as u32 - 1);
            defs.push(Stmt::Def(v, Op::ConstI(c)));
            v
        })
    };
    fn walk(b: &mut Block, consts: &HashMap<u32, i32>, adds: &HashMap<u32, (Val, i32)>, konst: &mut dyn FnMut(i32, &mut Vec<ir::Ty>) -> Val, vals: &mut Vec<ir::Ty>) {
        for s in b.iter_mut() {
            match s {
                Stmt::Def(_, op) => match *op {
                    Op::Bin(ir::Bin::AddI, x, y) => {
                        let (inner, c2) = match (adds.get(&x.0), consts.get(&y.0), adds.get(&y.0), consts.get(&x.0)) {
                            (Some(a), Some(c), _, _) | (_, _, Some(a), Some(c)) => (*a, *c),
                            _ => continue,
                        };
                        let k = konst(inner.1.wrapping_add(c2), vals);
                        *op = Op::Bin(ir::Bin::AddI, inner.0, k);
                    }
                    Op::Fma(ir::Fma::MulAddI, a, m, k) => {
                        let (Some(&(x, c1)), Some(&mv), Some(&kv)) = (adds.get(&a.0), consts.get(&m.0), consts.get(&k.0)) else { continue };
                        let nk = konst(c1.wrapping_mul(mv).wrapping_add(kv), vals);
                        *op = Op::Fma(ir::Fma::MulAddI, x, m, nk);
                    }
                    _ => {}
                },
                Stmt::If(_, t, e) => {
                    walk(t, consts, adds, konst, vals);
                    walk(e, consts, adds, konst, vals);
                }
                Stmt::Loop { body, .. } => walk(body, consts, adds, konst, vals),
                _ => {}
            }
        }
    }
    let mut vals = std::mem::take(&mut p.vals);
    walk(&mut p.body, &consts, &adds, &mut konst, &mut vals);
    p.vals = vals;
    p.body.splice(0..0, defs);
    dce(p);
}

/// A table read at `h + k` (k a constant, h proven with h + k inside the
/// read's extent) is the read of word `base + k` at `h`: no clamp applies
/// either way, so it is the same word; reads of one row at one offset then
/// share it (`GRAD2[h]`, `GRAD2[h + 1]`).
pub fn peel_offsets(p: &mut Program) {
    let b = ir::bounds(p);
    let mut adds: HashMap<u32, (Val, u32)> = HashMap::new();
    let mut consts: HashMap<u32, u32> = HashMap::new();
    fn find(blk: &Block, adds: &mut HashMap<u32, (Val, Val)>, consts: &mut HashMap<u32, u32>) {
        for s in blk {
            match s {
                Stmt::Def(v, Op::Bin(ir::Bin::AddI, x, y)) => {
                    adds.insert(v.0, (*x, *y));
                }
                Stmt::Def(v, Op::ConstI(c)) => {
                    consts.insert(v.0, *c as u32);
                }
                Stmt::If(_, t, e) => {
                    find(t, adds, consts);
                    find(e, adds, consts);
                }
                Stmt::Loop { body, .. } => find(body, adds, consts),
                _ => {}
            }
        }
    }
    let mut raw = HashMap::new();
    find(&p.body, &mut raw, &mut consts);
    for (v, (x, y)) in raw {
        if let Some(k) = consts.get(&y.0) {
            adds.insert(v, (x, *k));
        } else if let Some(k) = consts.get(&x.0) {
            adds.insert(v, (y, *k));
        }
    }
    fn walk(blk: &mut Block, adds: &HashMap<u32, (Val, u32)>, b: &[Option<u32>]) {
        for s in blk.iter_mut() {
            match s {
                Stmt::Def(_, Op::Load { region: Region::Shared, base, extent, off }) => {
                    let Some(o) = *off else { continue };
                    let Some(&(h, k)) = adds.get(&o.0) else { continue };
                    if k < *extent && b[h.0 as usize].is_some_and(|m| (m as u64) + (k as u64) < *extent as u64) {
                        *base += k;
                        *extent -= k;
                        *off = Some(h);
                    }
                }
                Stmt::If(_, t, e) => {
                    walk(t, adds, b);
                    walk(e, adds, b);
                }
                Stmt::Loop { body, .. } => walk(body, adds, b),
                _ => {}
            }
        }
    }
    walk(&mut p.body, &adds, &b);
}

/// A table read moves up to just after an earlier read at the same
/// offset in its block (its offset is defined by then; tables never
/// change), so the reads of one row sit together.
pub fn cluster_loads(p: &mut Program) {
    fn block(b: &mut Block) {
        for s in b.iter_mut() {
            match s {
                Stmt::If(_, t, e) => {
                    block(t);
                    block(e);
                }
                Stmt::Loop { body, .. } => block(body),
                _ => {}
            }
        }
        let mut k = 0;
        while k < b.len() {
            if let Stmt::Def(_, Op::Load { region: Region::Shared, off: Some(o), .. }) = b[k] {
                // The last read at this offset before k, and the run of
                // reads at it that follows.
                if let Some(j) = (0..k).rev().find(|&j| matches!(b[j], Stmt::Def(_, Op::Load { region: Region::Shared, off: Some(oo), .. }) if oo == o)) {
                    let mut at = j + 1;
                    while at < k && matches!(b[at], Stmt::Def(_, Op::Load { region: Region::Shared, off: Some(oo), .. }) if oo == o) {
                        at += 1;
                    }
                    if at < k {
                        let s = b.remove(k);
                        b.insert(at, s);
                    }
                }
            }
            k += 1;
        }
    }
    block(&mut p.body);
}

/// `Wrap(x, n)` of an x proven in `0..n` ([`ir::bounds`]) is x.
/// Integer multiplies by a power of two (from `2 * h`-style index math)
/// become shifts: the same wrapping result, shorter latency.
pub fn strength_reduce(p: &mut Program) {
    let mut consts: HashMap<u32, i32> = HashMap::new();
    let mut shift_consts: Vec<(u32, u32)> = Vec::new();
    fn find(b: &Block, consts: &mut HashMap<u32, i32>) {
        for s in b {
            match s {
                Stmt::Def(v, Op::ConstI(c)) => {
                    consts.insert(v.0, *c);
                }
                Stmt::If(_, t, e) => {
                    find(t, consts);
                    find(e, consts);
                }
                Stmt::Loop { body, .. } => find(body, consts),
                _ => {}
            }
        }
    }
    find(&p.body, &mut consts);
    fn walk(b: &mut Block, consts: &HashMap<u32, i32>, new: &mut Vec<(u32, u32)>, vals: &mut Vec<ir::Ty>) {
        for s in b.iter_mut() {
            match s {
                Stmt::Def(_, op) => {
                    if let Op::Bin(ir::Bin::MulI, x, y) = *op {
                        let k = |v: Val| consts.get(&v.0).copied().filter(|c| *c > 1 && (*c as u32).is_power_of_two());
                        let (a, c) = match (k(y), k(x)) {
                            (Some(c), _) => (x, c),
                            (None, Some(c)) => (y, c),
                            _ => continue,
                        };
                        let sh = (c as u32).trailing_zeros();
                        // A new constant for the shift amount, defined at the top.
                        vals.push(ir::Ty::I32);
                        let cv = vals.len() as u32 - 1;
                        new.push((cv, sh));
                        *op = Op::Bin(ir::Bin::ShlI, a, Val(cv));
                    }
                }
                Stmt::If(_, t, e) => {
                    walk(t, consts, new, vals);
                    walk(e, consts, new, vals);
                }
                Stmt::Loop { body, .. } => walk(body, consts, new, vals),
                _ => {}
            }
        }
    }
    walk(&mut p.body, &consts, &mut shift_consts, &mut p.vals);
    let defs: Vec<Stmt> = shift_consts.into_iter().map(|(v, sh)| Stmt::Def(Val(v), Op::ConstI(sh as i32))).collect();
    p.body.splice(0..0, defs);
}

pub fn drop_wraps(p: &mut Program) {
    let b = ir::bounds(p);
    let mut map = HashMap::new();
    fn walk(blk: &Block, b: &[Option<u32>], map: &mut HashMap<u32, Val>) {
        for s in blk {
            match s {
                Stmt::Def(v, Op::Wrap(x, n)) if b[x.0 as usize].is_some_and(|m| m < *n) => {
                    map.insert(v.0, *x);
                }
                Stmt::If(_, t, e) => {
                    walk(t, b, map);
                    walk(e, b, map);
                }
                Stmt::Loop { body, .. } => walk(body, b, map),
                _ => {}
            }
        }
    }
    walk(&p.body, &b, &mut map);
    rename(&mut p.body, &map);
    for r in &mut p.results {
        subst(r, &map);
    }
}

// ---------------------------------------------------------------------------
// Constant offsets and store-to-load forwarding
// ---------------------------------------------------------------------------

/// A load or store whose offset became a constant (a loop index or a
/// variable forwarded to one) addresses one static word: the offset
/// folds into the base as the lowering folds literal offsets (clamped
/// into the extent; for host buffers the 64-bit sum, clamped at run time
/// to the buffer as before).
pub fn fold_offsets(p: &mut Program) {
    let mut consts: HashMap<u32, u32> = HashMap::new();
    fn find(b: &Block, consts: &mut HashMap<u32, u32>) {
        for s in b {
            match s {
                Stmt::Def(v, Op::ConstI(c)) => {
                    consts.insert(v.0, *c as u32);
                }
                Stmt::If(_, t, e) => {
                    find(t, consts);
                    find(e, consts);
                }
                Stmt::Loop { body, .. } => find(body, consts),
                _ => {}
            }
        }
    }
    find(&p.body, &mut consts);
    let fold = |region: Region, base: &mut u32, extent: &mut u32, off: &mut Option<Val>| {
        let Some(c) = off.and_then(|o| consts.get(&o.0).copied()) else { return };
        let at = match region {
            Region::Buf(_) => match base.checked_add(c) {
                Some(a) => a,
                None => return,
            },
            _ => *base + ir::clamp_off(c, *extent),
        };
        *base = at;
        *extent = 1;
        *off = None;
    };
    fn walk(b: &mut Block, fold: &dyn Fn(Region, &mut u32, &mut u32, &mut Option<Val>)) {
        for s in b.iter_mut() {
            match s {
                Stmt::Def(_, Op::Load { region, base, extent, off }) => fold(*region, base, extent, off),
                Stmt::Store { region, base, extent, off, .. } => fold(*region, base, extent, off),
                Stmt::If(_, t, e) => {
                    walk(t, fold);
                    walk(e, fold);
                }
                Stmt::Loop { body, .. } => walk(body, fold),
                _ => {}
            }
        }
    }
    walk(&mut p.body, &fold);
}

/// A load of a static frame, ctx or state word after a store of it (same
/// type) with nothing between that may write it reads the stored value.
/// Host buffers are left alone (two words past the end clamp to one).
pub fn forward_stores(p: &mut Program) {
    type Known = HashMap<(Region, u32, ir::Ty), Val>;
    fn forget(known: &mut Known, region: Region) {
        known.retain(|k, _| k.0 != region);
    }
    fn written(b: &Block, out: &mut HashSet<Region>, host: &mut bool) {
        for s in b {
            match s {
                Stmt::Store { region, .. } => {
                    out.insert(*region);
                }
                Stmt::CallHost { .. } => *host = true,
                // What the functions write.
                Stmt::Call { .. } => CALLEE_WRITES.with(|w| {
                    let w = w.borrow();
                    out.extend(w.stores.iter().map(|x| x.0));
                    *host |= w.host_call;
                }),
                Stmt::If(_, t, e) => {
                    written(t, out, host);
                    written(e, out, host);
                }
                Stmt::Loop { body, .. } => written(body, out, host),
                _ => {}
            }
        }
    }
    fn walk(b: &mut Block, known: &mut Known, tys: &[ir::Ty], map: &mut HashMap<u32, Val>) {
        for s in b.iter_mut() {
            match s {
                Stmt::Def(v, op) => {
                    rename_op(op, map);
                    if let Op::Load { region, base, off: None, .. } = *op {
                        if !matches!(region, Region::Buf(_)) {
                            if let Some(x) = known.get(&(region, base, tys[v.0 as usize])) {
                                map.insert(v.0, *x);
                            }
                        }
                    }
                }
                Stmt::Store { region, base, off, val, .. } => {
                    subst(val, map);
                    if let Some(o) = off {
                        subst(o, map);
                    }
                    if off.is_none() && !matches!(region, Region::Buf(_)) {
                        // This word only (as any type), then its new value.
                        let (r, w) = (*region, *base);
                        known.retain(|k, _| !(k.0 == r && k.1 == w));
                        known.insert((r, w, tys[val.0 as usize]), *val);
                    } else {
                        forget(known, *region);
                    }
                }
                Stmt::If(c, t, e) => {
                    subst(c, map);
                    walk(t, &mut known.clone(), tys, map);
                    walk(e, &mut known.clone(), tys, map);
                    let (mut regs, mut host) = (HashSet::new(), false);
                    written(t, &mut regs, &mut host);
                    written(e, &mut regs, &mut host);
                    for r in regs {
                        forget(known, r);
                    }
                    if host {
                        forget(known, Region::Ctx);
                    }
                }
                Stmt::Loop { body, .. } => {
                    let (mut regs, mut host) = (HashSet::new(), false);
                    written(body, &mut regs, &mut host);
                    for r in &regs {
                        forget(known, *r);
                    }
                    if host {
                        forget(known, Region::Ctx);
                    }
                    walk(body, &mut known.clone(), tys, map);
                    for r in regs {
                        forget(known, r);
                    }
                }
                Stmt::CallHost { .. } => {
                    rename_stmt(s, map);
                    forget(known, Region::Ctx);
                }
                Stmt::Call { .. } => {
                    rename_stmt(s, map);
                    CALLEE_WRITES.with(|w| {
                        let w = w.borrow();
                        for x in &w.stores {
                            forget(known, x.0);
                        }
                        if w.host_call {
                            forget(known, Region::Ctx);
                        }
                    });
                }
                other => rename_stmt(other, map),
            }
        }
    }
    let tys = p.vals.clone();
    let mut map = HashMap::new();
    walk(&mut p.body, &mut HashMap::new(), &tys, &mut map);
    rename(&mut p.body, &map);
    for r in &mut p.results {
        subst(r, &map);
    }
}

// ---------------------------------------------------------------------------
// Dead frame stores and dead loops
// ---------------------------------------------------------------------------

/// The frame words `b` loads (`dynamic`: some load has a dynamic offset).
fn frame_loads(b: &Block, read: &mut HashSet<u32>, dynamic: &mut bool) {
    for s in b {
        match s {
            Stmt::Def(_, Op::Load { region: Region::Frame, base, off, .. }) => {
                if off.is_some() {
                    *dynamic = true;
                } else {
                    read.insert(*base);
                }
            }
            Stmt::If(_, t, e) => {
                frame_loads(t, read, dynamic);
                frame_loads(e, read, dynamic);
            }
            Stmt::Loop { body, .. } => frame_loads(body, read, dynamic),
            _ => {}
        }
    }
}

/// Frame (per-call scratch) stores no load can read are removed: when
/// every frame load has a static address, a store whose words no load
/// names is dead. (A struct literal emitted or read field by field leaves
/// only such stores once its loads are forwarded.)
pub fn dead_frame_stores(p: &mut Program) {
    let (mut read, mut dynamic) = FRAME_READS.with(|f| f.borrow().clone());
    frame_loads(&p.body, &mut read, &mut dynamic);
    if dynamic {
        return;
    }
    fn sweep(b: &mut Block, read: &HashSet<u32>) {
        b.retain(|s| match s {
            Stmt::Store { region: Region::Frame, base, extent, off, .. } => {
                let len = if off.is_some() { *extent } else { 1 };
                (*base..base.saturating_add(len)).any(|w| read.contains(&w))
            }
            _ => true,
        });
        for s in b.iter_mut() {
            match s {
                Stmt::If(_, t, e) => {
                    sweep(t, read);
                    sweep(e, read);
                }
                Stmt::Loop { body, .. } => sweep(body, read),
                _ => {}
            }
        }
    }
    sweep(&mut p.body, &read);
}

/// A loop with no effect is removed: nothing in it stores, outputs or
/// calls out, no break or continue in it leaves it, and no variable it
/// sets is read outside it. Loops always end (their cap), so skipping one
/// changes nothing.
pub fn dead_loops(p: &mut Program) {
    fn effects(b: &Block, depth: u32) -> bool {
        b.iter().any(|s| match s {
            Stmt::Store { .. } | Stmt::Out { .. } | Stmt::CallHost { .. } | Stmt::Call { .. } => true,
            Stmt::Break(d) | Stmt::Continue(d) => *d >= depth,
            Stmt::If(_, t, e) => effects(t, depth) || effects(e, depth),
            Stmt::Loop { body, .. } => effects(body, depth + 1),
            _ => false,
        })
    }
    fn gets(b: &Block, out: &mut HashMap<u32, u32>) {
        for s in b {
            match s {
                Stmt::Def(_, Op::Get(v)) => *out.entry(v.0).or_default() += 1,
                Stmt::If(_, t, e) => {
                    gets(t, out);
                    gets(e, out);
                }
                Stmt::Loop { body, .. } => gets(body, out),
                _ => {}
            }
        }
    }
    let mut all = HashMap::new();
    gets(&p.body, &mut all);
    fn sweep(b: &mut Block, all: &HashMap<u32, u32>) {
        for s in b.iter_mut() {
            match s {
                Stmt::If(_, t, e) => {
                    sweep(t, all);
                    sweep(e, all);
                }
                Stmt::Loop { body, .. } => sweep(body, all),
                _ => {}
            }
        }
        b.retain(|s| {
            let Stmt::Loop { body, .. } = s else { return true };
            if effects(body, 1) {
                return true;
            }
            let mut set = HashSet::new();
            sets_in(body, &mut set);
            let mut inside = HashMap::new();
            gets(body, &mut inside);
            // Some get of a variable the loop sets lies outside it.
            set.iter().any(|v| all.get(v).copied().unwrap_or(0) > inside.get(v).copied().unwrap_or(0))
        });
    }
    sweep(&mut p.body, &all);
}

// ---------------------------------------------------------------------------
// Store clustering
// ---------------------------------------------------------------------------

/// A store to a host buffer at a dynamic offset moves down its block to
/// just before the next store of the same record (same buffer, same offset
/// value, the next word), across definitions that do not read that buffer:
/// the words of `out[i].a = ..; out[i].b = ..` end up adjacent, which the
/// vector code writes per element in one go. No load of the buffer, no
/// other store to it and no control flow is crossed, so every word gets
/// the same value in the same order.
pub fn cluster_stores(p: &mut Program) {
    fn block(b: &mut Block) {
        for s in b.iter_mut() {
            match s {
                Stmt::If(_, t, e) => {
                    block(t);
                    block(e);
                }
                Stmt::Loop { body, .. } => block(body),
                _ => {}
            }
        }
        // Stores only move down, so this ends.
        while cluster_pass(b) {}
    }
    fn cluster_pass(b: &mut Block) -> bool {
        let mut moved = false;
        let mut i = 0;
        while i < b.len() {
            let Stmt::Store { region: Region::Buf(k), base, off: Some(o), .. } = b[i] else {
                i += 1;
                continue;
            };
            // The next statement that is not a definition free of buffer k.
            let mut j = i + 1;
            while j < b.len() {
                match &b[j] {
                    Stmt::Def(_, Op::Load { region: Region::Buf(r), .. }) if *r == k => break,
                    Stmt::Def(..) => j += 1,
                    _ => break,
                }
            }
            let joins = j > i + 1 && matches!(b.get(j), Some(Stmt::Store { region: Region::Buf(r), base: bn, off: Some(on), .. }) if *r == k && *on == o && *bn == base + 1);
            if joins {
                let st = b.remove(i);
                b.insert(j - 1, st);
                moved = true;
                // The statement that moved into position i is a definition.
                continue;
            }
            i += 1;
        }
        moved
    }
    block(&mut p.body);
}

// ---------------------------------------------------------------------------
// Renaming
// ---------------------------------------------------------------------------

fn subst(v: &mut Val, map: &HashMap<u32, Val>) {
    let mut x = *v;
    while let Some(y) = map.get(&x.0) {
        x = *y;
    }
    *v = x;
}

fn rename_op(op: &mut Op, map: &HashMap<u32, Val>) {
    match op {
        Op::Un(_, a) | Op::Wrap(a, _) => subst(a, map),
        Op::Bin(_, a, b) | Op::CmpF(_, a, b) | Op::CmpD(_, a, b) | Op::CmpI(_, a, b) => {
            subst(a, map);
            subst(b, map);
        }
        Op::Sel(c, a, b) | Op::Fma(_, a, b, c) => {
            subst(c, map);
            subst(a, map);
            subst(b, map);
        }
        Op::Load { off: Some(o), .. } => subst(o, map),
        Op::In { idx, .. } => subst(idx, map),
        _ => {}
    }
}

/// Replaces every use of a key of `map` by its value (definitions stay).
fn rename(b: &mut Block, map: &HashMap<u32, Val>) {
    if map.is_empty() {
        return;
    }
    for s in b {
        rename_stmt(s, map);
    }
}

fn rename_stmt(s: &mut Stmt, map: &HashMap<u32, Val>) {
    match s {
        Stmt::Def(_, op) => rename_op(op, map),
        Stmt::Set(_, v) => subst(v, map),
        Stmt::Store { off, val, .. } => {
            if let Some(o) = off {
                subst(o, map);
            }
            subst(val, map);
        }
        Stmt::Out { idx, val, .. } => {
            subst(idx, map);
            subst(val, map);
        }
        Stmt::If(c, t, e) => {
            subst(c, map);
            rename(t, map);
            rename(e, map);
        }
        Stmt::Loop { body, .. } => rename(body, map),
        Stmt::CallHost { args, slices, .. } => {
            for a in args {
                subst(a, map);
            }
            for x in slices {
                subst(&mut x.off, map);
                subst(&mut x.len, map);
            }
        }
        Stmt::Call { args, .. } => {
            for a in args {
                subst(a, map);
            }
        }
        Stmt::Break(_) | Stmt::Continue(_) => {}
    }
}

/// Variables set anywhere in `b`.
fn sets_in(b: &Block, out: &mut HashSet<u32>) {
    for s in b {
        match s {
            Stmt::Set(v, _) => {
                out.insert(v.0);
            }
            Stmt::If(_, t, e) => {
                sets_in(t, out);
                sets_in(e, out);
            }
            Stmt::Loop { body, .. } => sets_in(body, out),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Set -> Get forwarding
// ---------------------------------------------------------------------------

/// A `Get(var)` reads the value of the dominating `Set(var, x)` when no
/// other set can come between them (straight-line code, or an enclosing
/// block's set not touched by the constructs in between).
pub fn forward_sets(p: &mut Program) {
    fn walk(b: &mut Block, known: &mut HashMap<u32, Val>, map: &mut HashMap<u32, Val>) {
        for s in b.iter_mut() {
            match s {
                Stmt::Def(v, op) => {
                    rename_op(op, map);
                    if let Op::Get(var) = op {
                        if let Some(x) = known.get(&var.0) {
                            map.insert(v.0, *x);
                        }
                    }
                }
                Stmt::Set(var, v) => {
                    subst(v, map);
                    known.insert(var.0, *v);
                }
                Stmt::If(c, t, e) => {
                    subst(c, map);
                    let mut kt = known.clone();
                    walk(t, &mut kt, map);
                    let mut ke = known.clone();
                    walk(e, &mut ke, map);
                    let mut set = HashSet::new();
                    sets_in(t, &mut set);
                    sets_in(e, &mut set);
                    for v in set {
                        known.remove(&v);
                    }
                }
                Stmt::Loop { body, .. } => {
                    let mut set = HashSet::new();
                    sets_in(body, &mut set);
                    for v in &set {
                        known.remove(v);
                    }
                    let mut kb = known.clone();
                    walk(body, &mut kb, map);
                    for v in set {
                        known.remove(&v);
                    }
                }
                other => rename_stmt(other, map),
            }
        }
    }
    let mut map = HashMap::new();
    walk(&mut p.body, &mut HashMap::new(), &mut map);
    // Uses before the forwarding point were renamed on the way; a second
    // sweep covers uses the walk saw before their Def's mapping existed
    // (none in structured code, but cheap).
    rename(&mut p.body, &map);
    for r in &mut p.results {
        subst(r, &map);
    }
}

// ---------------------------------------------------------------------------
// Memory effects
// ---------------------------------------------------------------------------

/// What a block may write.
#[derive(Default, Clone)]
struct Writes {
    /// (region, word range) of stores with a static range; a dynamic store
    /// counts its whole extent.
    stores: Vec<(Region, u32, u32)>,
    /// Host buffers written (stores or host calls' slices).
    bufs: HashSet<u8>,
    host_call: bool,
    vars: HashSet<u32>,
}

impl Writes {
    fn of(b: &Block) -> Writes {
        let mut w = Writes::default();
        w.walk(b);
        w
    }

    fn walk(&mut self, b: &Block) {
        for s in b {
            match s {
                Stmt::Set(v, _) => {
                    self.vars.insert(v.0);
                }
                Stmt::Store { region, base, extent, off, .. } => {
                    if let Region::Buf(k) = region {
                        self.bufs.insert(*k);
                    } else {
                        let len = if off.is_some() { *extent } else { 1 };
                        self.stores.push((*region, *base, len));
                    }
                }
                Stmt::CallHost { slices, .. } => {
                    self.host_call = true;
                    for x in slices {
                        self.bufs.insert(x.buf);
                    }
                }
                // What the functions write (not the caller's variables).
                Stmt::Call { .. } => CALLEE_WRITES.with(|w| {
                    let w = w.borrow();
                    self.stores.extend(w.stores.iter().copied());
                    self.bufs.extend(w.bufs.iter().copied());
                    self.host_call |= w.host_call;
                }),
                Stmt::If(_, t, e) => {
                    self.walk(t);
                    self.walk(e);
                }
                Stmt::Loop { body, .. } => self.walk(body),
                _ => {}
            }
        }
    }

    /// A load of `region` words `base..base + extent` sees no write here.
    fn load_invariant(&self, region: Region, base: u32, extent: u32, never_written: &HashSet<u8>) -> bool {
        match region {
            // The control word changes under the program (cancel).
            Region::Buf(0) => false,
            // Only buffers no statement of the program writes: another
            // element (thread) never writes them either.
            Region::Buf(k) => never_written.contains(&k),
            // Per-element scratch (and per lane in vector code).
            Region::Frame => false,
            r => {
                // A host call reports into ctx.
                if r == Region::Ctx && self.host_call {
                    return false;
                }
                !self.stores.iter().any(|(sr, sb, sl)| *sr == r && *sb < base.saturating_add(extent) && base < sb.saturating_add(*sl))
            }
        }
    }
}

/// Host buffers no statement of `p` writes.
fn read_only_bufs(p: &Program) -> HashSet<u8> {
    let w = Writes::of(&p.body);
    let mut used = HashSet::new();
    fn walk(b: &Block, used: &mut HashSet<u8>) {
        for s in b {
            match s {
                Stmt::Def(_, Op::Load { region: Region::Buf(k), .. }) => {
                    used.insert(*k);
                }
                Stmt::If(_, t, e) => {
                    walk(t, used);
                    walk(e, used);
                }
                Stmt::Loop { body, .. } => walk(body, used),
                _ => {}
            }
        }
    }
    walk(&p.body, &mut used);
    let callee = CALLEE_WRITES.with(|c| c.borrow().bufs.clone());
    used.retain(|k| !w.bufs.contains(k) && !callee.contains(k) && *k != 0);
    used
}

// ---------------------------------------------------------------------------
// Loop-invariant code motion
// ---------------------------------------------------------------------------

/// Hoists invariant pure definitions out of every repeating loop (inner
/// loops first, so an invariant of both climbs out of both).
pub fn licm(p: &mut Program) {
    let ro = read_only_bufs(p);
    fn block(b: &mut Block, ro: &HashSet<u8>) {
        let mut out = Vec::with_capacity(b.len());
        for mut s in std::mem::take(b) {
            match &mut s {
                Stmt::If(_, t, e) => {
                    block(t, ro);
                    block(e, ro);
                }
                Stmt::Loop { cap, body } => {
                    block(body, ro);
                    if *cap > 1 {
                        let w = Writes::of(body);
                        // Values defined inside the loop (not yet hoisted).
                        let mut inside = HashSet::new();
                        defs_in(body, &mut inside);
                        let mut hoisted = Vec::new();
                        hoist(body, &w, ro, &mut inside, &mut hoisted);
                        out.extend(hoisted);
                    }
                }
                _ => {}
            }
            out.push(s);
        }
        *b = out;
    }
    block(&mut p.body, &ro);
}

fn defs_in(b: &Block, out: &mut HashSet<u32>) {
    for s in b {
        match s {
            Stmt::Def(v, _) => {
                out.insert(v.0);
            }
            Stmt::CallHost { rets, .. } | Stmt::Call { rets, .. } => {
                for r in rets {
                    out.insert(r.0);
                }
            }
            Stmt::If(_, t, e) => {
                defs_in(t, out);
                defs_in(e, out);
            }
            Stmt::Loop { body, .. } => defs_in(body, out),
            _ => {}
        }
    }
}

/// Moves the invariant definitions of `b` (in order, recursively through
/// branches and inner loops) into `out`.
fn hoist(b: &mut Block, w: &Writes, ro: &HashSet<u8>, inside: &mut HashSet<u32>, out: &mut Vec<Stmt>) {
    let mut keep = Vec::with_capacity(b.len());
    for mut s in std::mem::take(b) {
        let invariant = match &s {
            Stmt::Def(_, op) => {
                let operands_outside = ir::op_uses(op).iter().all(|u| !inside.contains(&u.0));
                operands_outside
                    && match *op {
                        Op::Get(var) => !w.vars.contains(&var.0),
                        Op::Load { region, base, extent, off } => {
                            let ext = if off.is_some() { extent } else { 1 };
                            w.load_invariant(region, base, ext, ro)
                        }
                        Op::In { .. } => false,
                        _ => true,
                    }
            }
            _ => false,
        };
        if invariant {
            if let Stmt::Def(v, _) = &s {
                inside.remove(&v.0);
            }
            out.push(s);
            continue;
        }
        match &mut s {
            Stmt::If(_, t, e) => {
                hoist(t, w, ro, inside, out);
                hoist(e, w, ro, inside, out);
            }
            Stmt::Loop { body, .. } => hoist(body, w, ro, inside, out),
            _ => {}
        }
        keep.push(s);
    }
    *b = keep;
}

// ---------------------------------------------------------------------------
// Common subexpressions (dominator-scoped value numbering)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    ConstF(u32),
    ConstD(u64),
    ConstI(i32),
    ConstB(bool),
    Un(ir::Un, u32),
    Bin(ir::Bin, u32, u32),
    CmpF(ir::Cmp, u32, u32),
    CmpI(ir::Cmp, u32, u32),
    CmpD(ir::Cmp, u32, u32),
    Sel(u32, u32, u32),
    Fma(ir::Fma, u32, u32, u32),
    Wrap(u32, u32),
    Load(Region, u32, u32, Option<u32>, ir::Ty),
    Get(u32),
    FrameCount,
    BufLen(u8),
}

fn key(op: &Op, ty: ir::Ty) -> Option<Key> {
    Some(match *op {
        Op::ConstF(x) => Key::ConstF(x.to_bits()),
        Op::ConstD(x) => Key::ConstD(x.to_bits()),
        Op::ConstI(x) => Key::ConstI(x),
        Op::ConstB(x) => Key::ConstB(x),
        Op::Un(u, a) => Key::Un(u, a.0),
        Op::Bin(b, x, y) => Key::Bin(b, x.0, y.0),
        Op::CmpF(c, x, y) => Key::CmpF(c, x.0, y.0),
        Op::CmpI(c, x, y) => Key::CmpI(c, x.0, y.0),
        Op::CmpD(c, x, y) => Key::CmpD(c, x.0, y.0),
        Op::Sel(c, x, y) => Key::Sel(c.0, x.0, y.0),
        Op::Fma(k, a, b, c) => Key::Fma(k, a.0, b.0, c.0),
        Op::Wrap(x, n) => Key::Wrap(x.0, n),
        Op::Load { region, base, extent, off } => Key::Load(region, base, extent, off.map(|o| o.0), ty),
        Op::Get(v) => Key::Get(v.0),
        Op::FrameCount => Key::FrameCount,
        Op::BufLen(k) => Key::BufLen(k),
        Op::In { .. } => return None,
    })
}

/// Merges equal pure definitions where one dominates the other. Loads
/// merge only from memory the program never writes (tables, params it
/// does not store, read-only buffers); gets only within straight-line
/// code with no set of the variable between them.
pub fn cse(p: &mut Program) {
    let ro = read_only_bufs(p);
    let all = Writes::of(&p.body);
    struct St<'a> {
        ro: &'a HashSet<u8>,
        all: &'a Writes,
        tys: &'a [ir::Ty],
        map: HashMap<u32, Val>,
    }
    fn stable_load(st: &St, op: &Op) -> bool {
        match *op {
            Op::Load { region, base, extent, off } => st.all.load_invariant(region, base, if off.is_some() { extent } else { 1 }, st.ro),
            _ => true,
        }
    }
    fn walk(b: &mut Block, st: &mut St, scope: &mut Vec<HashMap<Key, Val>>) {
        scope.push(HashMap::new());
        let mut keep = Vec::with_capacity(b.len());
        for mut s in std::mem::take(b) {
            match &mut s {
                Stmt::Def(v, op) => {
                    rename_op(op, &st.map);
                    if let Some(k) = key(op, st.tys[v.0 as usize]).filter(|_| stable_load(st, op)) {
                        // Gets are valid only until a set of the variable.
                        if let Some(found) = scope.iter().rev().find_map(|m| m.get(&k)) {
                            st.map.insert(v.0, *found);
                            continue;
                        }
                        scope.last_mut().unwrap().insert(k, *v);
                    }
                }
                Stmt::Set(var, x) => {
                    subst(x, &st.map);
                    let k = Key::Get(var.0);
                    for m in scope.iter_mut() {
                        m.remove(&k);
                    }
                }
                Stmt::If(c, t, e) => {
                    subst(c, &st.map);
                    walk(t, st, scope);
                    walk(e, st, scope);
                    let mut set = HashSet::new();
                    sets_in(t, &mut set);
                    sets_in(e, &mut set);
                    for v in set {
                        for m in scope.iter_mut() {
                            m.remove(&Key::Get(v));
                        }
                    }
                }
                Stmt::Loop { body, .. } => {
                    // A get outside the loop is stale inside when the loop sets it.
                    let mut set = HashSet::new();
                    sets_in(body, &mut set);
                    for m in scope.iter_mut() {
                        m.retain(|k, _| !matches!(k, Key::Get(v) if set.contains(v)));
                    }
                    walk(body, st, scope);
                }
                other => rename_stmt(other, &st.map),
            }
            keep.push(s);
        }
        *b = keep;
        scope.pop();
    }
    let tys = p.vals.clone();
    let mut st = St { ro: &ro, all: &all, tys: &tys, map: HashMap::new() };
    walk(&mut p.body, &mut st, &mut Vec::new());
    let map = st.map;
    rename(&mut p.body, &map);
    for r in &mut p.results {
        subst(r, &map);
    }
}

// ---------------------------------------------------------------------------
// If-conversion
// ---------------------------------------------------------------------------

/// Most definitions a converted `if` may run on the path it skipped.
const IF_CONVERT_DEFS: usize = 16;

/// An `if` whose sides only define values and set variables (after
/// hoisting, typically a few ops and a `Set`) becomes straight-line code:
/// both sides' definitions, then each variable set on either side takes
/// `c ? then-value : else-value` (its old value on a side that leaves it).
/// Both sides' ops are total and pure, so running the skipped one changes
/// nothing; vector code runs no masks for it, scalar code no branch.
pub fn if_convert(p: &mut Program) {
    fn flat(b: &Block) -> Option<usize> {
        let mut defs = 0;
        let mut set = HashSet::new();
        for s in b {
            match s {
                Stmt::Def(_, op) => {
                    // A get after a set of the same variable on this side
                    // would read the new value once flattened.
                    if let Op::Get(v) = op {
                        if set.contains(&v.0) {
                            return None;
                        }
                    }
                    if !matches!(op, Op::ConstF(_) | Op::ConstI(_) | Op::ConstB(_) | Op::ConstD(_)) {
                        defs += 1;
                    }
                }
                Stmt::Set(v, _) => {
                    set.insert(v.0);
                }
                _ => return None,
            }
        }
        Some(defs)
    }
    fn last_sets(b: &Block) -> Vec<(u32, Val)> {
        let mut out: Vec<(u32, Val)> = Vec::new();
        for s in b {
            if let Stmt::Set(v, x) = s {
                match out.iter_mut().find(|(w, _)| *w == v.0) {
                    Some(e) => e.1 = *x,
                    None => out.push((v.0, *x)),
                }
            }
        }
        out
    }
    // `inside`: values defined in the innermost repeating loop around the
    // block. An if whose condition is not one of them does not change
    // during the loop: it stays a branch (always predicted; vector code
    // takes it uniformly), so neither side runs for nothing.
    fn block(b: &mut Block, p_vals: &mut Vec<ir::Ty>, p_vars: &[ir::Ty], inside: Option<&HashSet<u32>>) {
        let mut out = Vec::with_capacity(b.len());
        for mut s in std::mem::take(b) {
            match &mut s {
                Stmt::If(c, t, e) => {
                    block(t, p_vals, p_vars, inside);
                    block(e, p_vals, p_vars, inside);
                    let invariant = inside.is_some_and(|d| !d.contains(&c.0));
                    if let (Some(dt), Some(de), false) = (flat(t), flat(e), invariant) {
                        if dt + de <= IF_CONVERT_DEFS {
                            let c = *c;
                            let st = last_sets(t);
                            let se = last_sets(e);
                            let mut vars: Vec<u32> = st.iter().chain(&se).map(|(v, _)| *v).collect();
                            vars.sort();
                            vars.dedup();
                            for x in t.drain(..).chain(e.drain(..)) {
                                if let Stmt::Def(..) = x {
                                    out.push(x);
                                }
                            }
                            let mut new = |ty: ir::Ty| {
                                p_vals.push(ty);
                                Val(p_vals.len() as u32 - 1)
                            };
                            let mut sets = Vec::new();
                            for v in vars {
                                let ty = p_vars[v as usize];
                                let find = |l: &[(u32, Val)]| l.iter().find(|(w, _)| *w == v).map(|x| x.1);
                                let (tv, ev) = (find(&st), find(&se));
                                let old = if tv.is_none() || ev.is_none() {
                                    let o = new(ty);
                                    out.push(Stmt::Def(o, Op::Get(ir::Var(v))));
                                    Some(o)
                                } else {
                                    None
                                };
                                let sel = new(ty);
                                out.push(Stmt::Def(sel, Op::Sel(c, tv.or(old).unwrap(), ev.or(old).unwrap())));
                                sets.push(Stmt::Set(ir::Var(v), sel));
                            }
                            out.extend(sets);
                            continue;
                        }
                    }
                }
                Stmt::Loop { cap, body } => {
                    if *cap > 1 {
                        let mut d = HashSet::new();
                        defs_in(body, &mut d);
                        block(body, p_vals, p_vars, Some(&d));
                    } else {
                        block(body, p_vals, p_vars, inside);
                    }
                }
                _ => {}
            }
            out.push(s);
        }
        *b = out;
    }
    let vars = p.vars.clone();
    block(&mut p.body, &mut p.vals, &vars, None);
}

// ---------------------------------------------------------------------------
// Fused multiply-add (math: fast only)
// ---------------------------------------------------------------------------

/// `a * b + c`, `c - a * b` and `a * b - c` whose product has no other use
/// become one fused multiply-add ([`ir::Op::Fma`], rounded once). This
/// changes values (by at most the product's rounding), so only `math:
/// fast` kernels run it; every backend and the interpreter give the same
/// fused bits.
pub fn fuse_fma(p: &mut Program) {
    for g in &mut p.funcs {
        fuse(g, true);
        negate_constant_addends(g);
    }
    fuse(p, true);
    negate_constant_addends(p);
}

/// `a * b - c` with a constant c is `a * b + (-c)` exactly (negation is
/// exact): the backends then need no negation of the addend.
fn negate_constant_addends(p: &mut Program) {
    let mut konst: HashMap<u32, f32> = HashMap::new();
    fn find(b: &Block, k: &mut HashMap<u32, f32>) {
        for s in b {
            match s {
                Stmt::Def(v, Op::ConstF(x)) => {
                    k.insert(v.0, *x);
                }
                Stmt::If(_, t, e) => {
                    find(t, k);
                    find(e, k);
                }
                Stmt::Loop { body, .. } => find(body, k),
                _ => {}
            }
        }
    }
    find(&p.body, &mut konst);
    let mut negs: HashMap<u32, Val> = HashMap::new();
    let mut new_defs = Vec::new();
    fn walk(b: &mut Block, konst: &HashMap<u32, f32>, negs: &mut HashMap<u32, Val>, new_defs: &mut Vec<Stmt>, vals: &mut Vec<ir::Ty>) {
        for s in b.iter_mut() {
            match s {
                Stmt::Def(_, op) => {
                    if let Op::Fma(ir::Fma::Sub, a, bb, c) = *op {
                        if let Some(x) = konst.get(&c.0) {
                            let n = *negs.entry(c.0).or_insert_with(|| {
                                vals.push(ir::Ty::F32);
                                let v = Val(vals.len() as u32 - 1);
                                new_defs.push(Stmt::Def(v, Op::ConstF(-x)));
                                v
                            });
                            *op = Op::Fma(ir::Fma::Add, a, bb, n);
                        }
                    }
                }
                Stmt::If(_, t, e) => {
                    walk(t, konst, negs, new_defs, vals);
                    walk(e, konst, negs, new_defs, vals);
                }
                Stmt::Loop { body, .. } => walk(body, konst, negs, new_defs, vals),
                _ => {}
            }
        }
    }
    walk(&mut p.body, &konst, &mut negs, &mut new_defs, &mut p.vals);
    p.body.splice(0..0, new_defs);
    dce(p);
}

/// i32 `a * b + c` whose product has no other use becomes one multiply-add
/// (`Fma::MulAddI`): exact, so every kernel gets it.
pub fn fuse_mla(p: &mut Program) {
    fuse(p, false);
}

fn fuse(p: &mut Program, float: bool) {
    let mul = if float { ir::Bin::MulF } else { ir::Bin::MulI };
    let mut uses = vec![0u32; p.vals.len()];
    let mut muls: HashMap<u32, (Val, Val)> = HashMap::new();
    fn count(b: &Block, uses: &mut [u32], muls: &mut HashMap<u32, (Val, Val)>, mul: ir::Bin) {
        for s in b {
            let us: Vec<Val> = match s {
                Stmt::Def(v, op) => {
                    if let Op::Bin(m, a, b) = op {
                        if *m == mul {
                            muls.insert(v.0, (*a, *b));
                        }
                    }
                    ir::op_uses(op)
                }
                Stmt::Set(_, v) => vec![*v],
                Stmt::Store { off, val, .. } => off.iter().copied().chain([*val]).collect(),
                Stmt::Out { idx, val, .. } => vec![*idx, *val],
                Stmt::If(c, t, e) => {
                    count(t, uses, muls, mul);
                    count(e, uses, muls, mul);
                    vec![*c]
                }
                Stmt::Loop { body, .. } => {
                    count(body, uses, muls, mul);
                    vec![]
                }
                Stmt::CallHost { args, slices, .. } => args.iter().copied().chain(slices.iter().flat_map(|x| [x.off, x.len])).collect(),
                Stmt::Call { args, .. } => args.clone(),
                _ => vec![],
            };
            for u in us {
                uses[u.0 as usize] += 1;
            }
        }
    }
    count(&p.body, &mut uses, &mut muls, mul);
    for r in &p.results {
        uses[r.0 as usize] += 1;
    }
    let single = |v: Val| if uses[v.0 as usize] == 1 { muls.get(&v.0).copied() } else { None };
    fn walk(b: &mut Block, f: &dyn Fn(Val) -> Option<(Val, Val)>, float: bool) {
        for s in b {
            match s {
                Stmt::Def(_, op) => {
                    let fused = match *op {
                        Op::Bin(ir::Bin::AddI, x, y) if !float => f(x).map(|(a, b)| Op::Fma(ir::Fma::MulAddI, a, b, y)).or_else(|| f(y).map(|(a, b)| Op::Fma(ir::Fma::MulAddI, a, b, x))),
                        _ if !float => None,
                        Op::Bin(ir::Bin::AddF, x, y) => f(x).map(|(a, b)| Op::Fma(ir::Fma::Add, a, b, y)).or_else(|| f(y).map(|(a, b)| Op::Fma(ir::Fma::Add, a, b, x))),
                        Op::Bin(ir::Bin::SubF, x, y) => f(x).map(|(a, b)| Op::Fma(ir::Fma::Sub, a, b, y)).or_else(|| f(y).map(|(a, b)| Op::Fma(ir::Fma::SubFrom, a, b, x))),
                        _ => None,
                    };
                    if let Some(op2) = fused {
                        *op = op2;
                    }
                }
                Stmt::If(_, t, e) => {
                    walk(t, f, float);
                    walk(e, f, float);
                }
                Stmt::Loop { body, .. } => walk(body, f, float),
                _ => {}
            }
        }
    }
    walk(&mut p.body, &single, float);
    dce(p);
}

// ---------------------------------------------------------------------------
// Dead code
// ---------------------------------------------------------------------------

/// Removes definitions nobody reads (every op is pure) and sets of
/// variables nobody gets, to a fixpoint.
pub fn dce(p: &mut Program) {
    loop {
        // Variables some Get reads.
        let mut read = vec![false; p.vars.len()];
        fn gets(b: &Block, read: &mut Vec<bool>) {
            for s in b {
                match s {
                    Stmt::Def(_, Op::Get(v)) => read[v.0 as usize] = true,
                    Stmt::If(_, t, e) => {
                        gets(t, read);
                        gets(e, read);
                    }
                    Stmt::Loop { body, .. } => gets(body, read),
                    _ => {}
                }
            }
        }
        gets(&p.body, &mut read);
        fn drop_sets(b: &mut Block, read: &[bool]) {
            b.retain(|s| !matches!(s, Stmt::Set(v, _) if !read[v.0 as usize]));
            for s in b.iter_mut() {
                match s {
                    Stmt::If(_, t, e) => {
                        drop_sets(t, read);
                        drop_sets(e, read);
                    }
                    Stmt::Loop { body, .. } => drop_sets(body, read),
                    _ => {}
                }
            }
        }
        drop_sets(&mut p.body, &read);
        let mut used = vec![false; p.vals.len()];
        fn mark(b: &Block, used: &mut Vec<bool>) {
            for s in b {
                let us: Vec<Val> = match s {
                    Stmt::Def(_, op) => ir::op_uses(op),
                    Stmt::Set(_, v) => vec![*v],
                    Stmt::Store { off, val, .. } => off.iter().copied().chain([*val]).collect(),
                    Stmt::Out { idx, val, .. } => vec![*idx, *val],
                    Stmt::If(c, t, e) => {
                        mark(t, used);
                        mark(e, used);
                        vec![*c]
                    }
                    Stmt::Loop { body, .. } => {
                        mark(body, used);
                        vec![]
                    }
                    Stmt::CallHost { args, slices, .. } => args.iter().copied().chain(slices.iter().flat_map(|x| [x.off, x.len])).collect(),
                    Stmt::Call { args, .. } => args.clone(),
                    _ => vec![],
                };
                for u in us {
                    used[u.0 as usize] = true;
                }
            }
        }
        mark(&p.body, &mut used);
        // A function's results are read by its callers.
        for r in &p.results {
            used[r.0 as usize] = true;
        }
        let mut removed = false;
        fn sweep(b: &mut Block, used: &[bool], removed: &mut bool) {
            b.retain(|s| match s {
                Stmt::Def(v, _) if !used[v.0 as usize] => {
                    *removed = true;
                    false
                }
                _ => true,
            });
            for s in b.iter_mut() {
                match s {
                    Stmt::If(_, t, e) => {
                        sweep(t, used, removed);
                        sweep(e, used, removed);
                    }
                    Stmt::Loop { body, .. } => sweep(body, used, removed),
                    _ => {}
                }
            }
        }
        sweep(&mut p.body, &used, &mut removed);
        if !removed {
            return;
        }
    }
}

