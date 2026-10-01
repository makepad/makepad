//! Masked SPMD over a kernel's element loop: the target-independent
//! analysis the four-wide backends share (NEON ×4 in [`crate::neon`],
//! wasm SIMD128 in [`crate::wasm`]).
//!
//! A kernel's element loop (the shape `lower_kernel` emits, [`shape`]) runs
//! four elements per iteration, one per vector lane. [`analyse`] decides
//! once, for every value and variable, whether it is **uniform** (every
//! lane holds the same bits: constants, ctx words, loads at uniform
//! addresses, ops of uniform values) or varying; which ifs branch on a
//! varying condition (both sides then run under an execution mask, the
//! masks joined after); which loops lanes may leave at different
//! iterations (they keep per-loop `break` and `continue` masks); which
//! breaks and continues are plain jumps; and which statements contain a
//! masked exit (the mask may shrink across them). [`bool_masks`] says which
//! bools a backend may hold as all-ones lane masks rather than the scalar
//! code's 0/1 words, [`steps`] which integers are arithmetic sequences
//! across lanes (contiguous and strided memory fast paths), and
//! [`supported`] what the four-wide form declines (the scalar code runs).

use crate::ir::{Bin, Block, Cmp, Op, Program, Region, Stmt, Un, Val, Var};
use crate::lower::kernel::ELEMENT_CAP;
use std::collections::{HashMap, HashSet};

/// Largest frame (words per lane) vectorized: 4 lanes x 4096 words = 64 KiB.
pub(crate) const MAX_FRAME_WORDS: u32 = 4096;

pub(crate) type Id = usize;

pub(crate) fn id(s: &Stmt) -> Id {
    s as *const Stmt as usize
}

#[derive(Default)]
pub(crate) struct Info {
    pub(crate) vval: Vec<bool>,
    pub(crate) vvar: Vec<bool>,
    /// Ifs whose condition varies across lanes.
    pub(crate) div_if: HashSet<Id>,
    /// Loops whose lanes may leave at different iterations.
    pub(crate) masked: HashSet<Id>,
    /// Break/continue statements that are plain jumps (every construct
    /// between them and their loop is a uniform if or a plain loop).
    pub(crate) jump: HashSet<Id>,
    /// Statements containing a masked break/continue that leaves them.
    pub(crate) escapes: HashSet<Id>,
    /// The element loop's `i >= n` condition (uniform: n % 4 == 0).
    pub(crate) uniform_by_shape: HashSet<u32>,
}

/// A construct between a break and its loop: an if (divergent?) or a loop.
enum Frame {
    If(bool),
    Loop(Id),
}

impl Info {
    fn varying_op(&self, op: &Op) -> bool {
        let vv = |v: &Val| self.vval[v.0 as usize];
        match op {
            Op::Get(var) => self.vvar[var.0 as usize],
            // Per-lane memory (every lane has its own frame).
            Op::Load { region: Region::Frame, .. } => true,
            op => crate::ir::op_uses(op).iter().any(vv),
        }
    }

    /// One pass; returns whether anything changed.
    fn pass(&mut self, b: &Block, div: bool) -> bool {
        let mut changed = false;
        for s in b {
            match s {
                Stmt::Def(v, op) => {
                    let x = !self.uniform_by_shape.contains(&v.0) && self.varying_op(op);
                    if x && !self.vval[v.0 as usize] {
                        self.vval[v.0 as usize] = true;
                        changed = true;
                    }
                }
                Stmt::Set(var, v) => {
                    if (div || self.vval[v.0 as usize]) && !self.vvar[var.0 as usize] {
                        self.vvar[var.0 as usize] = true;
                        changed = true;
                    }
                }
                Stmt::If(c, t, e) => {
                    let d = self.vval[c.0 as usize];
                    if d && self.div_if.insert(id(s)) {
                        changed = true;
                    }
                    changed |= self.pass(t, div || d);
                    changed |= self.pass(e, div || d);
                }
                Stmt::Loop { body, .. } => {
                    let m = self.masked.contains(&id(s));
                    changed |= self.pass(body, div || m);
                }
                _ => {}
            }
        }
        changed
    }

    /// Classifies break/continue forms and masked loops; returns whether a
    /// loop became masked.
    fn forms(&mut self, b: &Block, stack: &mut Vec<Frame>) -> bool {
        let mut changed = false;
        for s in b {
            match s {
                Stmt::If(_, t, e) => {
                    stack.push(Frame::If(self.div_if.contains(&id(s))));
                    changed |= self.forms(t, stack);
                    changed |= self.forms(e, stack);
                    stack.pop();
                }
                Stmt::Loop { body, .. } => {
                    stack.push(Frame::Loop(id(s)));
                    changed |= self.forms(body, stack);
                    stack.pop();
                }
                Stmt::Break(d) | Stmt::Continue(d) => {
                    let mut loops_seen = 0;
                    let mut plain_path = true;
                    let mut target = None;
                    for f in stack.iter().rev() {
                        match f {
                            Frame::If(div) => plain_path &= !div,
                            Frame::Loop(lid) => {
                                if loops_seen == *d {
                                    target = Some(*lid);
                                    break;
                                }
                                plain_path &= !self.masked.contains(lid);
                                loops_seen += 1;
                            }
                        }
                    }
                    if plain_path {
                        self.jump.insert(id(s));
                    } else {
                        self.jump.remove(&id(s));
                        if let Some(t) = target {
                            if self.masked.insert(t) {
                                changed = true;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        changed
    }

    /// Marks statements that contain a masked break/continue leaving them
    /// (their end mask may be smaller than their start mask). Returns the
    /// loop depths (relative to `b`) that masked exits inside `b` reach.
    fn mark_escapes(&mut self, b: &Block) -> Vec<u32> {
        let mut out = Vec::new();
        for s in b {
            let inner: Vec<u32> = match s {
                Stmt::Break(d) | Stmt::Continue(d) if !self.jump.contains(&id(s)) => vec![*d],
                Stmt::If(_, t, e) => {
                    let mut v = self.mark_escapes(t);
                    v.extend(self.mark_escapes(e));
                    v
                }
                Stmt::Loop { body, .. } => self.mark_escapes(body).into_iter().filter(|d| *d > 0).map(|d| d - 1).collect(),
                _ => vec![],
            };
            if !inner.is_empty() {
                if !matches!(s, Stmt::Break(_) | Stmt::Continue(_)) {
                    self.escapes.insert(id(s));
                }
                out.extend(inner);
            }
        }
        out
    }
}

/// The element loop's parts, from the shape `lower_kernel` emits.
pub(crate) struct Shape<'a> {
    /// Top-level statements before the element loop (uniform).
    pub(crate) prelude: &'a [Stmt],
    pub(crate) element: &'a Stmt,
    /// The element counter.
    pub(crate) i: Var,
}

pub(crate) fn shape(p: &Program) -> Option<Shape<'_>> {
    let (last, prelude) = p.body.split_last()?;
    let Stmt::Loop { cap, body } = last else { return None };
    if *cap != ELEMENT_CAP || !prelude.iter().all(|s| matches!(s, Stmt::Def(..) | Stmt::Set(..))) {
        return None;
    }
    // The last statement steps the counter: Set(i, iv + 1), iv = Get(i).
    let Some(Stmt::Set(i, next)) = body.last() else { return None };
    // Loop-invariant code motion may have hoisted the step's constant.
    let defs: HashMap<u32, &Op> = body.iter().chain(prelude).filter_map(|s| if let Stmt::Def(v, op) = s { Some((v.0, op)) } else { None }).collect();
    let Some(Op::Bin(Bin::AddI, iv, one)) = defs.get(&next.0) else { return None };
    if !matches!(defs.get(&iv.0), Some(Op::Get(g)) if g == i) || !matches!(defs.get(&one.0), Some(Op::ConstI(1))) {
        return None;
    }
    // i is set once before the loop (to 0) and once at the end of the body.
    let pre_sets: Vec<&Stmt> = prelude.iter().filter(|s| matches!(s, Stmt::Set(v, _) if v == i)).collect();
    let pre_defs: HashMap<u32, &Op> = prelude.iter().filter_map(|s| if let Stmt::Def(v, op) = s { Some((v.0, op)) } else { None }).collect();
    match pre_sets[..] {
        [Stmt::Set(_, z)] if matches!(pre_defs.get(&z.0), Some(Op::ConstI(0))) => {}
        _ => return None,
    }
    fn sets_var(b: &Block, i: Var) -> usize {
        b.iter()
            .map(|s| match s {
                Stmt::Set(v, _) => (*v == i) as usize,
                Stmt::If(_, t, e) => sets_var(t, i) + sets_var(e, i),
                Stmt::Loop { body, .. } => sets_var(body, i),
                _ => 0,
            })
            .sum()
    }
    if sets_var(body, *i) != 1 {
        return None;
    }
    Some(Shape { prelude, element: last, i: *i })
}


/// Which bool values the vector code holds as lane masks: compares,
/// constants and negations always; and/or/select of masks; gets of
/// variables every set of which is a mask. Loaded bools (any bits) stay
/// raw words, as do and/or/select touching one, so every op keeps the
/// scalar code's exact bits where they can differ (and/or of raw words);
/// a mask becomes the 0/1 word where it meets raw bits or memory.
pub(crate) fn bool_masks(p: &Program) -> (Vec<bool>, Vec<bool>) {
    use crate::ir::Ty;
    let mut mask = vec![false; p.vals.len()];
    let mut var_mask: Vec<bool> = p.vars.iter().map(|t| *t == Ty::Bool).collect();
    loop {
        fn vals(b: &Block, p: &Program, mask: &mut Vec<bool>, var_mask: &[bool]) {
            for s in b {
                match s {
                    Stmt::Def(v, op) if p.vals[v.0 as usize] == Ty::Bool => {
                        mask[v.0 as usize] = match *op {
                            Op::CmpF(..) | Op::CmpI(..) | Op::CmpD(..) | Op::ConstB(_) | Op::Un(Un::NotB, _) => true,
                            Op::Bin(Bin::AndB | Bin::OrB, a, b) => mask[a.0 as usize] && mask[b.0 as usize],
                            Op::Sel(_, a, b) => mask[a.0 as usize] && mask[b.0 as usize],
                            Op::Get(var) => var_mask[var.0 as usize],
                            _ => false,
                        };
                    }
                    Stmt::If(_, t, e) => {
                        vals(t, p, mask, var_mask);
                        vals(e, p, mask, var_mask);
                    }
                    Stmt::Loop { body, .. } => vals(body, p, mask, var_mask),
                    _ => {}
                }
            }
        }
        // Values in program order twice (gets in a loop see later sets).
        vals(&p.body, p, &mut mask, &var_mask);
        vals(&p.body, p, &mut mask, &var_mask);
        let mut next = var_mask.clone();
        fn sets(b: &Block, mask: &[bool], next: &mut Vec<bool>) {
            for s in b {
                match s {
                    Stmt::Set(var, v) => {
                        if !mask[v.0 as usize] {
                            next[var.0 as usize] = false;
                        }
                    }
                    Stmt::If(_, t, e) => {
                        sets(t, mask, next);
                        sets(e, mask, next);
                    }
                    Stmt::Loop { body, .. } => sets(body, mask, next),
                    _ => {}
                }
            }
        }
        sets(&p.body, &mask, &mut next);
        for (k, t) in p.vars.iter().enumerate() {
            if *t != Ty::Bool {
                next[k] = false;
            }
        }
        if next == var_mask {
            return (mask, var_mask);
        }
        var_mask = next;
    }
}


/// What the vector code cannot express (the scalar code runs instead).
pub(crate) fn supported(p: &Program, body: &Block) -> bool {
    if p.uses_f64() || p.frame_words > MAX_FRAME_WORDS {
        return false;
    }
    // Ctx words written per element must not be read per element (a
    // reduction's running value would be read once for four elements).
    let mut ctx_loads = HashSet::new();
    let mut ctx_stores = HashSet::new();
    fn walk(b: &Block, loads: &mut HashSet<(u32, u32)>, stores: &mut HashSet<(u32, u32)>) -> bool {
        for s in b {
            let ok = match s {
                Stmt::Def(_, Op::In { .. }) | Stmt::Out { .. } | Stmt::CallHost { .. } => false,
                Stmt::Def(_, Op::Load { region: Region::State, .. }) | Stmt::Store { region: Region::State, .. } => false,
                Stmt::Def(_, Op::Load { region: Region::Ctx, base, extent, .. }) => {
                    loads.insert((*base, *extent));
                    true
                }
                Stmt::Store { region: Region::Ctx, base, extent, .. } => {
                    stores.insert((*base, *extent));
                    true
                }
                Stmt::If(_, t, e) => walk(t, loads, stores) && walk(e, loads, stores),
                Stmt::Loop { body, .. } => walk(body, loads, stores),
                _ => true,
            };
            if !ok {
                return false;
            }
        }
        true
    }
    if !walk(body, &mut ctx_loads, &mut ctx_stores) {
        return false;
    }
    let overlaps = |a: &(u32, u32), b: &(u32, u32)| a.0 < b.0.saturating_add(b.1) && b.0 < a.0.saturating_add(a.1);
    !ctx_stores.iter().any(|s| ctx_loads.iter().any(|l| overlaps(s, l)))
}

pub(crate) fn analyse(p: &Program, sh: &Shape) -> Option<Info> {
    let Stmt::Loop { body, .. } = sh.element else { return None };
    let mut info = Info { vval: vec![false; p.vals.len()], vvar: vec![false; p.vars.len()], ..Default::default() };
    info.vvar[sh.i.0 as usize] = true;
    // The `i >= n` exit: a CmpI(Ge, Get(i), FrameCount) guarding a
    // Break(0) at the top of the element body.
    let defs: HashMap<u32, &Op> = body.iter().chain(sh.prelude).filter_map(|s| if let Stmt::Def(v, op) = s { Some((v.0, op)) } else { None }).collect();
    for s in body.iter().take(4) {
        if let Stmt::If(c, t, e) = s {
            if matches!(t[..], [Stmt::Break(0)]) && e.is_empty() {
                if let Some(Op::CmpI(Cmp::Ge, a, n)) = defs.get(&c.0) {
                    if matches!(defs.get(&a.0), Some(Op::Get(g)) if *g == sh.i) && matches!(defs.get(&n.0), Some(Op::FrameCount)) {
                        info.uniform_by_shape.insert(c.0);
                    }
                }
            }
        }
    }
    if info.uniform_by_shape.is_empty() {
        return None;
    }
    // Fixpoint: varying values/vars, divergent ifs, masked loops.
    for _ in 0..64 {
        let mut changed = info.pass(&p.body, false);
        changed |= info.forms(&p.body, &mut Vec::new());
        if !changed {
            info.mark_escapes(&p.body);
            // The element loop must stay plain (its exits uniform).
            if info.masked.contains(&id(sh.element)) {
                return None;
            }
            return Some(info);
        }
    }
    None
}


/// Per value, the lane step when its lanes are an arithmetic sequence
/// (`lane0 + l * step`, wrapping i32): the element counter steps by 1,
/// uniform values by 0; sums, differences and constant multiples follow.
/// Also every ConstI.
pub(crate) fn steps(p: &Program, info: &Info, i: Var) -> (Vec<Option<i64>>, Vec<Option<i32>>) {
    let mut step = vec![None; p.vals.len()];
    let mut consts = vec![None; p.vals.len()];
    fn walk(b: &Block, info: &Info, i: Var, step: &mut Vec<Option<i64>>, consts: &mut Vec<Option<i32>>) {
        for s in b {
            match s {
                Stmt::Def(v, op) => {
                    let st = |x: &Val| step[x.0 as usize];
                    let r = if !info.vval[v.0 as usize] {
                        Some(0)
                    } else {
                        match *op {
                            Op::Get(var) if var == i => Some(1),
                            Op::Bin(Bin::AddI, a, b) => st(&a).zip(st(&b)).map(|(x, y)| x + y),
                            Op::Bin(Bin::SubI, a, b) => st(&a).zip(st(&b)).map(|(x, y)| x - y),
                            Op::Bin(Bin::MulI, a, b) => match (st(&a), consts[b.0 as usize], consts[a.0 as usize], st(&b)) {
                                (Some(x), Some(c), _, _) | (_, _, Some(c), Some(x)) => Some(x * c as i64),
                                _ => None,
                            },
                            Op::Fma(crate::ir::Fma::MulAddI, a, b, c) => match (st(&a), consts[b.0 as usize], consts[a.0 as usize], st(&b), st(&c)) {
                                (Some(x), Some(k), _, _, Some(z)) | (_, _, Some(k), Some(x), Some(z)) => Some(x * k as i64 + z),
                                _ => None,
                            },
                            Op::Bin(Bin::ShlI, a, b) => match (st(&a), consts[b.0 as usize]) {
                                (Some(x), Some(k)) if (0..31).contains(&k) => Some(x << k),
                                _ => None,
                            },
                            _ => None,
                        }
                    };
                    // Keep steps small: they only describe addresses.
                    step[v.0 as usize] = r.filter(|x| x.abs() <= 1 << 20);
                    if let Op::ConstI(c) = op {
                        consts[v.0 as usize] = Some(*c);
                    }
                }
                Stmt::If(_, t, e) => {
                    walk(t, info, i, step, consts);
                    walk(e, info, i, step, consts);
                }
                Stmt::Loop { body, .. } => walk(body, info, i, step, consts),
                _ => {}
            }
        }
    }
    walk(&p.body, info, i, &mut step, &mut consts);
    (step, consts)
}

