//! Four-wide SIMD128 code: a kernel's element loop run four elements per
//! iteration, the masked SPMD of [`crate::spmd`] (the analysis the NEON ×4
//! backend uses) emitted as wasm, bit-identical to the interpreter.
//!
//! - **Values.** A varying value lives in a `v128` local, one element per
//!   lane, holding the bits the scalar code holds for that element (bools
//!   as 0/1 words, or as all-ones lane masks where [`spmd::bool_masks`]
//!   allows; a mask becomes its 0/1 word where it meets raw bits or
//!   memory). A **uniform** value is computed once by the scalar emitter
//!   ([`super::Sc`]: the same exact op sequences) into a scalar local, and
//!   splatted into a `v128` local once at its definition when a vector op
//!   reads it: loop counters, params, uniform loads, branch conditions and
//!   constants cost no lane work, and a constant hoisted out of the element
//!   loop is splatted once per call.
//! - **Ops.** Lane-wise IEEE ops are the scalar ops; min/max are
//!   compare-and-select (AIR's `a < b ? a : b`, not wasm's NaN rules);
//!   round is half away from zero spelled out; `f32 -> i32` is
//!   `trunc_sat` (Rust `as`); a fused multiply-add is `relaxed_madd` only
//!   when the host proved it fused ([`super::Target::relaxed_fma`]), else
//!   exact through f64 halves with the scalar code's midpoint fix-up per
//!   lane when any lane needs it. Integer division by a constant uses the
//!   magic multiplier (64-bit lane products), by a power of two shifts;
//!   division by anything else, non-power-of-two wraps past 2^31 and
//!   per-lane shift amounts run lane by lane through the scalar sequences.
//! - **Control flow.** A branch on a uniform condition is a wasm `if` on
//!   the scalar condition. A divergent one runs both sides under the
//!   execution mask (a `v128` local), each skipped when no lane takes it,
//!   the masks joined after. A loop whose lanes may leave at different
//!   iterations keeps `break`/`continue` masks and stops when no lane
//!   runs; plain breaks are `br`. Variables written while not every lane
//!   runs are blended into their old value.
//! - **Memory.** Every access clamps per lane exactly like the
//!   interpreter, and stores of the running lanes happen in lane order (a
//!   word two lanes store ends as the later element's). Fast paths only
//!   after one bounds check proves every lane inside: four consecutive
//!   words (one `v128.load`/`store`), records `st` words apart (lane loads
//!   and stores at static offsets), and record groups (n consecutive words
//!   at one varying offset: one check on the largest lane offset, then
//!   lane loads/stores). Each lane has its own frame: word w of lane l at
//!   `frame + 16 w + 4 l` (so a uniform frame access is one `v128` access).
//!
//! The entry runs `n` elements, `n` a multiple of 4 (the element loop's
//! `i >= n` exit is uniform by that); the host runs the remainder on the
//! scalar entry with ctx's element base advanced. Only element-local
//! kernels whose outputs hold every record may run four wide (the caller
//! checks `parallel_safe`). [`body`] declines what [`crate::spmd::supported`]
//! declines (f64, state, host calls, audio I/O, reductions, large frames)
//! and any program without the element loop's shape.

use super::{div_s, fma_tail, op, rem_s, wrap_rem, Body, Label, Sc, Target, P_FRAME};
use crate::ir::{self, Bin, Block, Cmp, Fma, Op, Program, Region, Stmt, Ty, Un, Val, Var};
use crate::spmd::{self, id, Info};

const NONE: u32 = u32::MAX;

/// Bytes at the frame's start before the lanes' words (the exact
/// multiply-add's operands and result).
pub(super) const FRAME_HEAD: u32 = 64;

/// Whether [`body`] compiles `p` (the analysis alone).
pub(super) fn supported(p: &Program) -> bool {
    analysis(p).is_some()
}

fn analysis(p: &Program) -> Option<(spmd::Shape<'_>, Info)> {
    let sh = spmd::shape(p)?;
    let Stmt::Loop { body, .. } = sh.element else { return None };
    if !spmd::supported(p, body) {
        return None;
    }
    let info = spmd::analyse(p, &sh)?;
    Some((sh, info))
}

/// The four-wide body of kernel `p` and whether it calls the exact
/// multiply-add fix-up (function `fix`, [`fma_fix`]), or None (declined:
/// run it scalar).
pub(super) fn body(p: &Program, target: Target, fix: u32) -> Option<(Body, bool)> {
    let (sh, info) = analysis(p)?;
    let (mask, var_mask) = spmd::bool_masks(p);
    let (step, consts) = spmd::steps(p, &info, sh.i);
    let sc = Sc::new(p, target, Body::new(), &|k| !info.vval[k], &|k| !info.vvar[k]);
    let mut w = W {
        sc,
        p,
        mask,
        var_mask,
        step,
        consts,
        vl: Vec::new(),
        vv: Vec::new(),
        us: Vec::new(),
        m: 0,
        full: true,
        i: sh.i,
        loops: Vec::new(),
        slots: Vec::new(),
        depth: 0,
        tv: [0; 8],
        tl: [0; 4],
        relaxed: target.relaxed_fma,
        fix,
        uses_fix: false,
        tables: Vec::new(),
        defs: defs(p),
        vconsts: Vec::new(),
        addr_terms: Vec::new(),
        dummy: NONE,
        pre: Body::new(),
        info,
    };
    let want = w.want_splats();
    let lt = super::lifetimes(p);
    let f = &mut w.sc.f;
    w.vl = super::assign_locals(&lt, p.vals.len(), f, &|k| w.info.vval[k].then_some(op::V128));
    w.vv = (0..p.vars.len()).map(|k| if w.info.vvar[k] { f.local(op::V128) } else { NONE }).collect();
    w.us = super::assign_locals(&lt, p.vals.len(), f, &|k| want[k].then_some(op::V128));
    w.m = f.local(op::V128);
    w.tv = std::array::from_fn(|_| f.local(op::V128));
    w.tl = std::array::from_fn(|_| f.local(op::I32));
    // Where the start code goes: after the buffers' pointers are read.
    let pre_at = w.sc.f.code.len();
    w.load_tables();
    // Every lane runs.
    w.vconst([u32::MAX; 4]);
    w.sc.f.set(w.m);
    for s in sh.prelude {
        match s {
            Stmt::Set(var, _) if *var == w.i => {
                // The lanes' element offsets.
                w.vconst([0, 1, 2, 3]);
                let l = w.vv[w.i.0 as usize];
                w.sc.f.set(l);
            }
            s => w.stmt(s),
        }
    }
    w.stmt(sh.element);
    // The constants' locals and the address terms, set before the body.
    let mut head = Body::new();
    for (words, l) in &w.vconsts {
        head.fd(op::V128_CONST);
        for x in words {
            head.code.extend_from_slice(&x.to_le_bytes());
        }
        head.set(*l);
    }
    head.code.extend_from_slice(&w.pre.code);
    let rest = w.sc.f.code.split_off(pre_at);
    w.sc.f.code.extend_from_slice(&head.code);
    w.sc.f.code.extend_from_slice(&rest);
    Some((w.sc.f, w.uses_fix))
}


fn defs(p: &Program) -> Vec<Option<Op>> {
    fn walk(b: &Block, out: &mut Vec<Option<Op>>) {
        for s in b {
            match s {
                Stmt::Def(v, op) => out[v.0 as usize] = Some(*op),
                Stmt::If(_, t, e) => {
                    walk(t, out);
                    walk(e, out);
                }
                Stmt::Loop { body, .. } => walk(body, out),
                _ => {}
            }
        }
    }
    let mut out = vec![None; p.vals.len()];
    walk(&p.body, &mut out);
    out
}

#[derive(Clone, Copy)]
struct LoopCtx {
    masked: bool,
    brk: u32,
    cont: u32,
}

struct W<'a> {
    /// The scalar emitter: the body, uniform values' locals, labels.
    sc: Sc<'a>,
    p: &'a Program,
    info: Info,
    /// Bool values / variables held as lane masks.
    mask: Vec<bool>,
    var_mask: Vec<bool>,
    /// Lanes `lane0 + l * step` (i32); every ConstI.
    step: Vec<Option<i64>>,
    consts: Vec<Option<i32>>,
    /// `v128` local per varying value / variable (NONE: uniform).
    vl: Vec<u32>,
    vv: Vec<u32>,
    /// The splat of a uniform value read by vector code (NONE: none made).
    us: Vec<u32>,
    /// The execution mask.
    m: u32,
    /// Every lane statically runs.
    full: bool,
    i: Var,
    loops: Vec<LoopCtx>,
    /// Mask locals by construct depth (two per level).
    slots: Vec<u32>,
    depth: u32,
    tv: [u32; 8],
    tl: [u32; 4],
    relaxed: bool,
    /// The fix-up function's index, and whether it is called.
    fix: u32,
    uses_fix: bool,
    /// Small shared tables held in `v128` locals for the call: (first
    /// word, words, the locals of its 4-word chunks).
    tables: Vec<(u32, u32, Vec<u32>)>,
    /// Every value's defining op.
    defs: Vec<Option<Op>>,
    /// Pooled constant vectors and their locals.
    vconsts: Vec<([u32; 4], u32)>,
    /// Address terms set at the function's start ([`W::addr_term`]).
    addr_terms: Vec<((Region, u32), u32, u32)>,
    /// The masked-off lanes' store address (NONE until needed).
    dummy: u32,
    /// Code run once at the start, after the buffers' pointers are read.
    pre: Body,
}

impl W<'_> {
    // -- encoding -------------------------------------------------------------

    fn fd(&mut self, sub: u32) {
        self.sc.f.fd(sub);
    }

    /// A SIMD op with a lane index.
    fn lane_op(&mut self, sub: u32, lane: u32) {
        self.sc.f.fd(sub);
        self.sc.f.b(lane as u8);
    }

    /// A SIMD memory op (alignment as log2 bytes).
    fn vmem(&mut self, sub: u32, align: u32, offset: u32) {
        self.sc.f.fd(sub);
        self.sc.f.u(align);
        self.sc.f.u(offset);
    }

    /// `v128.load32_lane` / `store32_lane` at a static byte offset.
    fn vlane_mem(&mut self, sub: u32, offset: u32, lane: u32) {
        self.vmem(sub, 2, offset);
        self.sc.f.b(lane as u8);
    }

    /// Pushes a constant vector: a local set once at the function's start
    /// (2-3 bytes per use instead of 18; an optimizing engine sees the
    /// constant itself).
    fn vconst(&mut self, words: [u32; 4]) {
        let l = match self.vconsts.iter().find(|(w, _)| *w == words) {
            Some((_, l)) => *l,
            None => {
                let l = self.sc.f.local(op::V128);
                self.vconsts.push((words, l));
                l
            }
        };
        self.get(l);
    }

    fn shuffle(&mut self, lanes: [u8; 16]) {
        self.sc.f.fd(op::I8X16_SHUFFLE);
        self.sc.f.code.extend_from_slice(&lanes);
    }

    fn get(&mut self, l: u32) {
        self.sc.f.get(l);
    }

    fn set(&mut self, l: u32) {
        self.sc.f.set(l);
    }

    fn open_if(&mut self, ty: u8) {
        self.sc.f.b(op::IF);
        self.sc.f.b(ty);
        self.sc.labels.push(Label::If);
    }

    fn else_(&mut self) {
        self.sc.f.b(op::ELSE);
    }

    fn end(&mut self) {
        self.sc.labels.pop();
        self.sc.f.b(op::END);
    }

    fn slot(&mut self, k: u32) -> u32 {
        let at = (2 * self.depth + k) as usize;
        while self.slots.len() <= at {
            let l = self.sc.f.local(op::V128);
            self.slots.push(l);
        }
        self.slots[at]
    }

    /// Finds the small tables (windows of at most 16 shared words that
    /// at least two varying reads with offsets proven inside them fall in)
    /// and loads them into chunk locals: a read is then a few swizzles.
    fn load_tables(&mut self) {
        fn walk(b: &Block, w: &W, out: &mut Vec<(u32, u32)>) {
            for s in b {
                match s {
                    Stmt::Def(_, Op::Load { region: Region::Shared, base, extent, off: Some(o) }) if !w.uniform(*o) && w.sc.bounds[o.0 as usize].is_some_and(|x| x < *extent) => {
                        out.push((*base, base + extent));
                    }
                    Stmt::If(_, t, e) => {
                        walk(t, w, out);
                        walk(e, w, out);
                    }
                    Stmt::Loop { body, .. } => walk(body, w, out),
                    _ => {}
                }
            }
        }
        let mut reads = Vec::new();
        walk(&self.p.body, self, &mut reads);
        reads.sort();
        // Merge overlapping windows, counting their reads.
        let mut merged: Vec<(u32, u32, u32)> = Vec::new();
        for (a, b) in reads {
            match merged.last_mut() {
                Some((_, e, n)) if a < *e => {
                    *e = (*e).max(b);
                    *n += 1;
                }
                _ => merged.push((a, b, 1)),
            }
        }
        for (a, e, n) in merged {
            if n < 2 || e - a > 16 {
                continue;
            }
            let len = e - a;
            let mut chunks = Vec::new();
            for c in 0..len.div_ceil(4) {
                let l = self.sc.f.local(op::V128);
                let words = (len - 4 * c).min(4);
                if words == 4 {
                    self.get(super::P_SHARED);
                    self.vmem(op::V128_LOAD, 2, 4 * (a + 4 * c));
                } else {
                    // A partial chunk: only words inside the table.
                    self.vconst([0; 4]);
                    for j in 0..words {
                        self.set(l);
                        self.get(super::P_SHARED);
                        self.get(l);
                        self.vlane_mem(op::V128_LOAD32_LANE, 4 * (a + 4 * c + j), j);
                    }
                }
                self.set(l);
                chunks.push(l);
            }
            self.tables.push((a, len, chunks));
        }
    }

    /// The resident table a read of words `base .. base + extent` lies in.
    fn table_of(&self, base: u32, extent: u32) -> Option<usize> {
        self.tables.iter().position(|(a, len, _)| base >= *a && base + extent <= a + len)
    }

    /// Pushes a read of resident table `t` at word `base + o` (o varying,
    /// proven below the read's extent): byte indices 4 (base - first + o)
    /// + 0..3 per lane, one swizzle per 4-word chunk (an index outside a
    /// chunk reads 0 there), or-ed.
    fn table_read(&mut self, t: usize, base: u32, o: Val) {
        let (a, _, chunks) = self.tables[t].clone();
        let idx = self.tv[4];
        self.vget(o);
        self.vconst([0x0404_0404; 4]);
        self.fd(op::I32X4_MUL);
        self.vconst([(base - a).wrapping_mul(0x0404_0404).wrapping_add(0x0302_0100); 4]);
        self.fd(op::I32X4_ADD);
        self.set(idx);
        for (c, l) in chunks.iter().enumerate() {
            self.get(*l);
            self.get(idx);
            if c > 0 {
                self.vconst([(16 * c as u32) * 0x0101_0101; 4]);
                self.fd(op::I8X16_SUB);
            }
            self.fd(op::I8X16_SWIZZLE);
            if c > 0 {
                self.fd(op::V128_OR);
            }
        }
    }

    // -- values ---------------------------------------------------------------

    fn uniform(&self, v: Val) -> bool {
        !self.info.vval[v.0 as usize]
    }

    fn is_bool(&self, v: Val) -> bool {
        self.p.vals[v.0 as usize] == Ty::Bool
    }

    /// Which uniform values vector code reads (their splat is made once,
    /// at the definition; a read this misses splats where it reads).
    fn want_splats(&self) -> Vec<bool> {
        fn walk(w: &W, b: &Block, out: &mut Vec<bool>) {
            let vary = |v: &Val| w.info.vval[v.0 as usize];
            for s in b {
                match s {
                    Stmt::Def(v, op) => {
                        let uses = ir::op_uses(op);
                        if vary(v) || uses.iter().any(vary) {
                            for u in uses {
                                out[u.0 as usize] |= !vary(&u);
                            }
                        }
                    }
                    Stmt::Set(var, x) => {
                        if w.info.vvar[var.0 as usize] && !vary(x) {
                            out[x.0 as usize] = true;
                        }
                    }
                    Stmt::Store { region, off, val, .. } => {
                        if !vary(val) && (*region == Region::Frame || off.as_ref().is_some_and(vary)) {
                            out[val.0 as usize] = true;
                        }
                    }
                    Stmt::If(_, t, e) => {
                        walk(w, t, out);
                        walk(w, e, out);
                    }
                    Stmt::Loop { body, .. } => walk(w, body, out),
                    _ => {}
                }
            }
        }
        let mut out = vec![false; self.p.vals.len()];
        walk(self, &self.p.body, &mut out);
        out
    }

    /// Pushes the splat of uniform `v` (a mask bool as all ones / zero).
    fn splat_scalar(&mut self, v: Val) {
        let l = self.sc.val[v.0 as usize];
        if self.p.vals[v.0 as usize] == Ty::F32 {
            self.get(l);
            self.fd(op::F32X4_SPLAT);
        } else if self.mask[v.0 as usize] {
            self.sc.f.i32c(0);
            self.get(l);
            self.sc.f.b(op::I32_SUB);
            self.fd(op::I32X4_SPLAT);
        } else {
            self.get(l);
            self.fd(op::I32X4_SPLAT);
        }
    }

    /// Pushes `v` as held (a vector).
    fn vget(&mut self, v: Val) {
        let k = v.0 as usize;
        if self.vl[k] != NONE {
            self.get(self.vl[k]);
        } else if self.us[k] != NONE {
            self.get(self.us[k]);
        } else {
            self.splat_scalar(v);
        }
    }

    /// Pushes `v` as the scalar code's words when `word` (a mask becomes
    /// 0/1), else as held.
    fn vget_word(&mut self, v: Val, word: bool) {
        if word && self.mask[v.0 as usize] {
            if self.uniform(v) {
                let l = self.sc.val[v.0 as usize];
                self.get(l);
                self.fd(op::I32X4_SPLAT);
            } else {
                self.vget(v);
                self.sc.f.i32c(31);
                self.fd(op::I32X4_SHR_U);
            }
        } else {
            self.vget(v);
        }
    }

    /// Pushes bool `v` as a selector mask (raw bits: nonzero is true).
    fn vget_sel(&mut self, v: Val) {
        self.vget(v);
        if !self.mask[v.0 as usize] {
            self.vconst([0; 4]);
            self.fd(op::I32X4_NE);
        }
    }

    /// Pushes lane `l` of `v` as an i32 (uniform: the scalar).
    fn lane_i32(&mut self, v: Val, l: u32) {
        if self.uniform(v) {
            let s = self.sc.val[v.0 as usize];
            self.get(s);
        } else {
            self.vget(v);
            self.lane_op(op::I32X4_EXTRACT_LANE, l);
        }
    }

    // -- statements -----------------------------------------------------------

    fn block(&mut self, b: &Block) {
        let mut k = 0;
        while k < b.len() {
            let n = self.record(&b[k..]);
            if n > 0 {
                k += n;
                continue;
            }
            let s = &b[k];
            self.stmt(s);
            if self.info.escapes.contains(&id(s)) {
                self.full = false;
            }
            k += 1;
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Def(v, op) => self.def(*v, op),
            Stmt::Set(var, v) => self.set_var(*var, *v),
            Stmt::Store { region, base, extent, off, val } => self.store(*region, *base, *extent, *off, *val),
            Stmt::If(c, t, e) if !self.info.div_if.contains(&id(s)) => {
                let l = self.sc.val[c.0 as usize];
                self.get(l);
                self.open_if(op::VOID);
                let full = self.full;
                self.block(t);
                if !e.is_empty() {
                    self.else_();
                    self.full = full;
                    self.block(e);
                }
                self.end();
                self.full = full;
            }
            Stmt::If(c, t, e) => {
                // Divergent: both sides under masks, then the union.
                let (else_m, then_m) = (self.slot(0), self.slot(1));
                self.depth += 1;
                let tc = self.tv[0];
                self.vget_sel(*c);
                self.set(tc);
                self.get(self.m);
                self.get(tc);
                self.fd(op::V128_ANDNOT);
                self.set(else_m);
                self.get(self.m);
                self.get(tc);
                self.fd(op::V128_AND);
                self.set(self.m);
                let full = self.full;
                self.full = false;
                self.get(self.m);
                self.fd(op::V128_ANY_TRUE);
                self.open_if(op::VOID);
                self.block(t);
                self.end();
                if e.is_empty() {
                    self.get(self.m);
                    self.get(else_m);
                } else {
                    self.get(self.m);
                    self.set(then_m);
                    self.get(else_m);
                    self.set(self.m);
                    self.get(self.m);
                    self.fd(op::V128_ANY_TRUE);
                    self.open_if(op::VOID);
                    self.full = false;
                    self.block(e);
                    self.end();
                    self.get(self.m);
                    self.get(then_m);
                }
                self.fd(op::V128_OR);
                self.set(self.m);
                self.depth -= 1;
                self.full = full;
            }
            Stmt::Loop { cap, body } => {
                let masked = self.info.masked.contains(&id(s));
                let has_exit = self.info.escapes.contains(&id(s));
                let (brk, cont) = (self.slot(0), self.slot(1));
                if masked {
                    self.depth += 1;
                    self.vconst([0; 4]);
                    self.set(brk);
                }
                let full = self.full;
                // A plain loop's mask shrinks only through masked exits to
                // outer loops; with none inside, it keeps the entry mask.
                self.full = full && !masked && !has_exit;
                let cnt = self.sc.f.local(op::I32);
                self.sc.f.i32c(0);
                self.set(cnt);
                self.sc.f.b(op::BLOCK);
                self.sc.f.b(op::VOID);
                self.sc.labels.push(Label::Exit);
                self.sc.f.b(op::LOOP);
                self.sc.f.b(op::VOID);
                self.sc.labels.push(Label::Top);
                self.get(cnt);
                self.sc.f.i32c(*cap as i32);
                self.sc.f.b(op::I32_GE_U);
                self.sc.f.b(op::BR_IF);
                self.sc.f.u(1);
                if masked || has_exit {
                    self.get(self.m);
                    self.fd(op::V128_ANY_TRUE);
                    self.sc.f.b(op::I32_EQZ);
                    self.sc.f.b(op::BR_IF);
                    self.sc.f.u(1);
                }
                self.get(cnt);
                self.sc.f.i32c(1);
                self.sc.f.b(op::I32_ADD);
                self.set(cnt);
                if masked {
                    self.vconst([0; 4]);
                    self.set(cont);
                }
                self.sc.f.b(op::BLOCK);
                self.sc.f.b(op::VOID);
                self.sc.labels.push(Label::IterEnd);
                self.loops.push(LoopCtx { masked, brk, cont });
                self.block(body);
                self.loops.pop();
                self.sc.labels.pop();
                self.sc.f.b(op::END);
                if masked {
                    self.get(self.m);
                    self.get(cont);
                    self.fd(op::V128_OR);
                    self.set(self.m);
                }
                self.sc.f.b(op::BR);
                self.sc.f.u(0);
                self.sc.labels.pop();
                self.sc.f.b(op::END);
                self.sc.labels.pop();
                self.sc.f.b(op::END);
                if masked {
                    self.get(self.m);
                    self.get(brk);
                    self.fd(op::V128_OR);
                    self.set(self.m);
                    self.depth -= 1;
                }
                self.full = full;
            }
            Stmt::Break(d) | Stmt::Continue(d) => {
                let is_break = matches!(s, Stmt::Break(_));
                let lc = self.loops[self.loops.len() - 1 - *d as usize];
                if lc.masked {
                    let slot = if is_break { lc.brk } else { lc.cont };
                    self.get(slot);
                    self.get(self.m);
                    self.fd(op::V128_OR);
                    self.set(slot);
                }
                if self.info.jump.contains(&id(s)) {
                    // Every running lane leaves together. A masked loop may
                    // still hold continued lanes: they go on at the
                    // iteration end (breaking lanes are parked in `brk`).
                    if lc.masked && is_break {
                        self.vconst([0; 4]);
                        self.set(self.m);
                    }
                    let to = if is_break && !lc.masked { Label::Exit } else { Label::IterEnd };
                    let depth = self.sc.depth_of(to, *d);
                    self.sc.f.b(op::BR);
                    self.sc.f.u(depth);
                } else {
                    // These lanes stop until their loop's exit or next
                    // iteration.
                    self.vconst([0; 4]);
                    self.set(self.m);
                    self.full = false;
                }
            }
            Stmt::Out { .. } | Stmt::CallHost { .. } => unreachable!("declined"),
        }
    }

    fn set_var(&mut self, var: Var, v: Val) {
        let k = var.0 as usize;
        if var == self.i {
            // The element counter steps by the four elements.
            self.get(self.vv[k]);
            self.vconst([4; 4]);
            self.fd(op::I32X4_ADD);
            self.set(self.vv[k]);
            return;
        }
        if !self.info.vvar[k] {
            self.sc.v(v);
            self.set(self.sc.var[k]);
            return;
        }
        // A raw bool variable takes a mask as its 0/1 word.
        let word = self.is_bool(v) && !self.var_mask[k];
        self.vget_word(v, word);
        if !self.full {
            // Only the running lanes take the new value.
            self.get(self.vv[k]);
            self.get(self.m);
            self.fd(op::V128_BITSELECT);
        }
        self.set(self.vv[k]);
    }

    /// Runs `body` when any lane runs.
    fn guard_any(&mut self, body: impl FnOnce(&mut Self)) {
        if self.full {
            body(self);
            return;
        }
        self.get(self.m);
        self.fd(op::V128_ANY_TRUE);
        self.open_if(op::VOID);
        body(self);
        self.end();
    }

    /// A record: consecutive loads or stores (n >= 2) of one host buffer
    /// at one varying offset whose lanes are `o0 + l * st` (an element's
    /// record, st its stride), every lane running: one bounds check on the
    /// last lane's last word, then lane loads/stores at static offsets from
    /// lane 0's address (statement by statement, lanes in order, as the
    /// accesses one by one would); otherwise the accesses one by one.
    /// Returns the statements handled.
    fn record(&mut self, b: &[Stmt]) -> usize {
        let key = |s: &Stmt| -> Option<(bool, u8, Val)> {
            match s {
                Stmt::Store { region: Region::Buf(k), off: Some(o), .. } => Some((true, *k, *o)),
                Stmt::Def(_, Op::Load { region: Region::Buf(k), off: Some(o), .. }) => Some((false, *k, *o)),
                _ => None,
            }
        };
        let Some((store, k, o)) = b.first().and_then(key) else { return 0 };
        let st = match self.step[o.0 as usize] {
            Some(st) if st > 0 && st <= 1 << 16 => st as u32,
            _ => return 0,
        };
        if self.uniform(o) || (store && !self.full) {
            return 0;
        }
        let mut n = 1;
        while n < b.len() && key(&b[n]) == Some((store, k, o)) {
            n += 1;
        }
        let base_of = |s: &Stmt| match s {
            Stmt::Store { base, .. } | Stmt::Def(_, Op::Load { base, .. }) => *base,
            _ => unreachable!(),
        };
        let top = b[..n].iter().map(base_of).max().unwrap() as u64 + 3 * st as u64;
        if n < 2 || top >= 1 << 31 {
            return 0;
        }
        // Every lane's words inside: o0 + max(base) + 3 st <= last (64 bits).
        let (ptr, last) = self.sc.buf[k as usize].expect("used buffer");
        let a = self.tl[2];
        self.vget(o);
        self.lane_op(op::I32X4_EXTRACT_LANE, 0);
        self.set(a);
        self.get(a);
        self.sc.f.b(op::I64_EXTEND_I32_U);
        self.sc.f.i64c(top as i64);
        self.sc.f.b(op::I64_ADD);
        self.get(last);
        self.sc.f.b(op::I64_EXTEND_I32_U);
        self.sc.f.b(op::I64_LE_U);
        self.open_if(op::VOID);
        // a = ptr + 4 o0
        self.get(a);
        self.sc.f.i32c(2);
        self.sc.f.b(op::I32_SHL);
        self.get(ptr);
        self.sc.f.b(op::I32_ADD);
        self.set(a);
        let v = self.tv[7];
        for s in &b[..n] {
            let at = 4 * base_of(s);
            match s {
                Stmt::Store { val, .. } => {
                    self.vget_word(*val, true);
                    self.set(v);
                    for l in 0..4 {
                        self.get(a);
                        self.get(v);
                        self.vlane_mem(op::V128_STORE32_LANE, at + 4 * l * st, l);
                    }
                }
                Stmt::Def(d, _) => {
                    let d = self.vl[d.0 as usize];
                    self.vconst([0; 4]);
                    for l in 0..4 {
                        self.set(d);
                        self.get(a);
                        self.get(d);
                        self.vlane_mem(op::V128_LOAD32_LANE, at + 4 * l * st, l);
                    }
                    self.set(d);
                }
                _ => unreachable!(),
            }
        }
        self.else_();
        for s in &b[..n] {
            self.stmt(s);
        }
        self.end();
        n
    }

    // -- memory -------------------------------------------------------------------

    /// Pushes the byte address of a frame word for a uniform (or no)
    /// offset: lane 0's word (the four lanes' words are the 16 bytes there).
    fn frame_addr(&mut self, base: u32, extent: u32, off: Option<Val>) {
        let [t0, t1, _] = self.sc.ti;
        match off {
            None => self.sc.f.i32c(base as i32),
            Some(o) => {
                self.sc.v(o);
                if !self.sc.bounds[o.0 as usize].is_some_and(|b| b < extent) {
                    self.sc.f.i32c(extent as i32 - 1);
                    self.sc.f.min_u(t0, t1);
                }
                if base > 0 {
                    self.sc.f.i32c(base as i32);
                    self.sc.f.b(op::I32_ADD);
                }
            }
        }
        self.sc.f.i32c(4);
        self.sc.f.b(op::I32_SHL);
        self.get(P_FRAME);
        self.sc.f.b(op::I32_ADD);
        self.sc.f.i32c(FRAME_HEAD as i32);
        self.sc.f.b(op::I32_ADD);
    }

    /// A `v128` local holding, per lane, the address term of an access
    /// to `region` at word `base`, set once at the function's start:
    /// - a buffer: splat(ptr + 4 add), with the clamp splat(lim) as a
    ///   second local, where `base + o` clamped to the last word is
    ///   `min(o, lim) + add` (lim = last - base, add = base when base <=
    ///   last; else lim = 0, add = last);
    /// - the frame: frame + 64 + 16 base + 4 l (lane l's own word);
    /// - ctx or shared: splat(region + 4 base).
    fn addr_term(&mut self, region: Region, base: u32) -> (u32, u32) {
        let key = (region, base);
        if let Some((_, a, b)) = self.addr_terms.iter().find(|(k, _, _)| *k == key) {
            return (*a, *b);
        }
        let a = self.sc.f.local(op::V128);
        let mut lim = NONE;
        let f = &mut self.pre;
        match region {
            Region::Buf(k) => {
                let (ptr, last) = self.sc.buf[k as usize].expect("used buffer");
                lim = self.sc.f.local(op::V128);
                // base <= last
                let inside = |f: &mut Body| {
                    f.i32c(base as i32);
                    f.get(last);
                    f.b(op::I32_LE_U);
                };
                f.get(last);
                f.i32c(base as i32);
                f.b(op::I32_SUB);
                f.i32c(0);
                inside(f);
                f.b(op::SELECT);
                f.fd(op::I32X4_SPLAT);
                f.set(lim);
                f.get(ptr);
                f.i32c(base as i32);
                f.get(last);
                inside(f);
                f.b(op::SELECT);
                f.i32c(2);
                f.b(op::I32_SHL);
                f.b(op::I32_ADD);
                f.fd(op::I32X4_SPLAT);
                f.set(a);
            }
            Region::Frame => {
                f.get(P_FRAME);
                f.i32c((FRAME_HEAD + 16 * base) as i32);
                f.b(op::I32_ADD);
                f.fd(op::I32X4_SPLAT);
                f.fd(op::V128_CONST);
                for x in [0u32, 4, 8, 12] {
                    f.code.extend_from_slice(&x.to_le_bytes());
                }
                f.fd(op::I32X4_ADD);
                f.set(a);
            }
            r => {
                f.get(if r == Region::Ctx { super::P_CTX } else { super::P_SHARED });
                f.i32c((4 * base) as i32);
                f.b(op::I32_ADD);
                f.fd(op::I32X4_SPLAT);
                f.set(a);
            }
        }
        self.addr_terms.push((key, a, lim));
        (a, lim)
    }

    /// Pushes the four lanes' byte addresses of an access at varying offset
    /// `o`, each clamped as the interpreter clamps (vector ops: min, shift,
    /// add).
    fn vaddr(&mut self, region: Region, base: u32, extent: u32, o: Val) {
        let (a, lim) = self.addr_term(region, base);
        self.vget(o);
        let shift = match region {
            Region::Buf(_) => {
                self.get(lim);
                self.fd(op::I32X4_MIN_U);
                2
            }
            r => {
                if !self.sc.bounds[o.0 as usize].is_some_and(|b| b < extent) {
                    self.vconst([extent - 1; 4]);
                    self.fd(op::I32X4_MIN_U);
                }
                if r == Region::Frame {
                    4
                } else {
                    2
                }
            }
        };
        self.sc.f.i32c(shift);
        self.fd(op::I32X4_SHL);
        self.get(a);
        self.fd(op::I32X4_ADD);
    }

    /// The addresses on the stack with the lanes that do not run sent to a
    /// scratch word (the frame head's last), so stores need no branch.
    fn mask_addr(&mut self) {
        if self.full {
            return;
        }
        if self.dummy == NONE {
            self.dummy = self.sc.f.local(op::V128);
            let f = &mut self.pre;
            f.get(P_FRAME);
            f.i32c(FRAME_HEAD as i32 - 4);
            f.b(op::I32_ADD);
            f.fd(op::I32X4_SPLAT);
            f.set(self.dummy);
        }
        self.get(self.dummy);
        self.get(self.m);
        self.fd(op::V128_BITSELECT);
    }

    /// Stores lane by lane, in lane order: addresses in local `a`, words
    /// in local `v` (lanes `lanes`).
    fn lane_stores(&mut self, a: u32, v: u32, lanes: std::ops::Range<u32>) {
        for l in lanes {
            self.get(a);
            self.lane_op(op::I32X4_EXTRACT_LANE, l);
            self.get(v);
            self.vlane_mem(op::V128_STORE32_LANE, 0, l);
        }
    }

    /// Pushes 1 when every lane's word of a buffer access lies inside the
    /// buffer, for lanes `o0 + l` (consecutive): base + o0 + 3 at most the
    /// last word (64 bits); o0 into `tl[1]`.
    fn lanes_inside(&mut self, k: u8, base: u32, o: Val) {
        let (_, last) = self.sc.buf[k as usize].expect("used buffer");
        let t = self.tl[1];
        self.vget(o);
        self.lane_op(op::I32X4_EXTRACT_LANE, 0);
        self.set(t);
        self.get(t);
        self.sc.f.b(op::I64_EXTEND_I32_U);
        self.sc.f.i64c(base as i64 + 3);
        self.sc.f.b(op::I64_ADD);
        self.get(last);
        self.sc.f.b(op::I64_EXTEND_I32_U);
        self.sc.f.b(op::I64_LE_U);
    }

    /// Pushes the byte address of lane 0's word (after `lanes_inside`).
    fn lane0_addr(&mut self, k: u8, base: u32) {
        let (ptr, _) = self.sc.buf[k as usize].expect("used buffer");
        self.get(self.tl[1]);
        if base > 0 {
            self.sc.f.i32c(base as i32);
            self.sc.f.b(op::I32_ADD);
        }
        self.sc.f.i32c(2);
        self.sc.f.b(op::I32_SHL);
        self.get(ptr);
        self.sc.f.b(op::I32_ADD);
    }

    /// Pushes a varying load.
    fn load(&mut self, region: Region, base: u32, extent: u32, off: Option<Val>) {
        let uniform_off = off.is_none_or(|o| self.uniform(o));
        if region == Region::Frame && uniform_off {
            self.frame_addr(base, extent, off);
            self.vmem(op::V128_LOAD, 4, 0);
            return;
        }
        let o = off.expect("a varying load has a varying offset");
        if region == Region::Shared && self.sc.bounds[o.0 as usize].is_some_and(|b| b < extent) {
            if let Some(t) = self.table_of(base, extent) {
                self.table_read(t, base, o);
                return;
            }
        }
        match region {
            Region::Buf(k) if self.step[o.0 as usize] == Some(1) => {
                // Four consecutive words inside: one vector load.
                self.lanes_inside(k, base, o);
                self.open_if(op::V128);
                self.lane0_addr(k, base);
                self.vmem(op::V128_LOAD, 2, 0);
                self.else_();
                self.load_lanes(region, base, extent, o);
                self.end();
            }
            _ => self.load_lanes(region, base, extent, o),
        }
    }

    /// Pushes a load at varying offset `o`, lane by lane at clamped
    /// addresses.
    fn load_lanes(&mut self, region: Region, base: u32, extent: u32, o: Val) {
        let (a, acc) = (self.tv[6], self.tv[7]);
        self.vaddr(region, base, extent, o);
        self.set(a);
        self.vconst([0; 4]);
        for l in 0..4 {
            self.set(acc);
            self.get(a);
            self.lane_op(op::I32X4_EXTRACT_LANE, l);
            self.get(acc);
            self.vlane_mem(op::V128_LOAD32_LANE, 0, l);
        }
    }

    fn store(&mut self, region: Region, base: u32, extent: u32, off: Option<Val>, val: Val) {
        let uniform_off = off.is_none_or(|o| self.uniform(o));
        let (a, v) = (self.tv[6], self.tv[7]);
        if region == Region::Frame && uniform_off {
            // The four lanes' words: one vector store (running lanes).
            let at = self.tl[2];
            self.frame_addr(base, extent, off);
            self.set(at);
            self.get(at);
            self.vget_word(val, true);
            if !self.full {
                self.get(at);
                self.vmem(op::V128_LOAD, 4, 0);
                self.get(self.m);
                self.fd(op::V128_BITSELECT);
            }
            self.vmem(op::V128_STORE, 4, 0);
            return;
        }
        if uniform_off {
            if self.uniform(val) {
                // Every running lane writes the same word: once if any runs.
                self.guard_any(|w| {
                    w.sc.address(region, base, extent, off);
                    w.sc.v(val);
                    let opc = if w.p.vals[val.0 as usize] == Ty::F32 { op::F32_STORE } else { op::I32_STORE };
                    w.sc.f.mem(opc, 0);
                });
                return;
            }
            // The running lanes in lane order: the last one's word stays.
            self.sc.address(region, base, extent, off);
            self.fd(op::I32X4_SPLAT);
            self.mask_addr();
            self.set(a);
            self.vget_word(val, true);
            self.set(v);
            let lanes = if self.full { 3..4 } else { 0..4 };
            self.lane_stores(a, v, lanes);
            return;
        }
        let o = off.expect("varying offset");
        if let (Region::Buf(k), Some(1)) = (region, self.step[o.0 as usize]) {
            // Four consecutive words inside: one vector store.
            self.lanes_inside(k, base, o);
            self.open_if(op::VOID);
            let at = self.tl[2];
            self.lane0_addr(k, base);
            self.set(at);
            self.get(at);
            self.vget_word(val, true);
            if !self.full {
                self.get(at);
                self.vmem(op::V128_LOAD, 2, 0);
                self.get(self.m);
                self.fd(op::V128_BITSELECT);
            }
            self.vmem(op::V128_STORE, 2, 0);
            self.else_();
            self.store_lanes(region, base, extent, o, val);
            self.end();
            return;
        }
        self.store_lanes(region, base, extent, o, val);
    }

    /// The running lanes' stores at varying offset `o`, in lane order, at
    /// clamped addresses (lanes that do not run write the scratch word).
    fn store_lanes(&mut self, region: Region, base: u32, extent: u32, o: Val, val: Val) {
        let (a, v) = (self.tv[6], self.tv[7]);
        self.vaddr(region, base, extent, o);
        self.mask_addr();
        self.set(a);
        self.vget_word(val, true);
        self.set(v);
        self.lane_stores(a, v, 0..4);
    }

    // -- ops --------------------------------------------------------------------------

    fn def(&mut self, v: Val, op: &Op) {
        let k = v.0 as usize;
        if self.info.vval[k] {
            self.vop(v, op);
            self.set(self.vl[k]);
            return;
        }
        if ir::op_uses(op).iter().any(|u| self.info.vval[u.0 as usize]) {
            // Uniform by the program's shape (the element loop's exit):
            // computed lane-wise, lane 0 taken.
            self.vop(v, op);
            if self.p.vals[k] == Ty::F32 {
                self.lane_op(op::F32X4_EXTRACT_LANE, 0);
            } else {
                self.lane_op(op::I32X4_EXTRACT_LANE, 0);
                if self.mask[k] {
                    self.sc.f.i32c(1);
                    self.sc.f.b(op::I32_AND);
                }
            }
        } else {
            self.sc.op(v, op);
        }
        self.set(self.sc.val[k]);
        if self.us[k] != NONE {
            self.splat_scalar(v);
            self.set(self.us[k]);
        }
    }

    /// Pushes the lane-wise value of `op` (as `v` is held).
    fn vop(&mut self, v: Val, op: &Op) {
        match *op {
            Op::Get(var) => {
                let l = self.vv[var.0 as usize];
                self.get(l);
            }
            Op::Un(u, a) => self.un(v, u, a),
            Op::Bin(b, x, y) => self.bin(v, b, x, y),
            Op::CmpF(cc, x, y) | Op::CmpI(cc, x, y) => {
                let float = matches!(op, Op::CmpF(..));
                // Bools compare as masks when both are (equality only), else
                // as 0/1 words.
                let both = self.mask[x.0 as usize] && self.mask[y.0 as usize] && matches!(cc, Cmp::Eq | Cmp::Ne);
                let raw = !float && self.is_bool(x) && !both;
                self.vget_word(x, raw);
                self.vget_word(y, raw);
                self.fd(match (float, cc) {
                    (true, Cmp::Lt) => op::F32X4_LT,
                    (true, Cmp::Le) => op::F32X4_LE,
                    (true, Cmp::Gt) => op::F32X4_GT,
                    (true, Cmp::Ge) => op::F32X4_GE,
                    (true, Cmp::Eq) => op::F32X4_EQ,
                    (true, Cmp::Ne) => op::F32X4_NE,
                    (false, Cmp::Lt) => op::I32X4_LT_S,
                    (false, Cmp::Le) => op::I32X4_LE_S,
                    (false, Cmp::Gt) => op::I32X4_GT_S,
                    (false, Cmp::Ge) => op::I32X4_GE_S,
                    (false, Cmp::Eq) => op::I32X4_EQ,
                    (false, Cmp::Ne) => op::I32X4_NE,
                });
                if !self.mask[v.0 as usize] {
                    self.sc.f.i32c(31);
                    self.fd(op::I32X4_SHR_U);
                }
            }
            Op::Sel(c, x, y) => {
                // A bool select of raw words takes masks as their 0/1 word.
                let raw = self.is_bool(v) && !self.mask[v.0 as usize];
                self.vget_word(x, raw);
                self.vget_word(y, raw);
                self.vget_sel(c);
                self.fd(op::V128_BITSELECT);
            }
            Op::Fma(k, a, b, c) => self.fma(k, a, b, c),
            Op::Wrap(x, len) => self.wrap(x, len),
            Op::Load { region, base, extent, off } => self.load(region, base, extent, off),
            Op::ConstF(_) | Op::ConstI(_) | Op::ConstB(_) | Op::FrameCount | Op::BufLen(_) => unreachable!("uniform"),
            Op::ConstD(_) | Op::CmpD(..) | Op::In { .. } => unreachable!("declined"),
        }
    }

    fn un(&mut self, v: Val, u: Un, a: Val) {
        match u {
            Un::BitsFI | Un::BitsIF => self.vget(a),
            Un::NotB => {
                // A mask: NOT of a mask, or raw bits == 0 (as the scalar
                // code tests them).
                self.vget(a);
                if self.mask[a.0 as usize] {
                    self.fd(op::V128_NOT);
                } else {
                    self.vconst([0; 4]);
                    self.fd(op::I32X4_EQ);
                }
                if !self.mask[v.0 as usize] {
                    self.sc.f.i32c(31);
                    self.fd(op::I32X4_SHR_U);
                }
            }
            Un::RoundF => {
                // t = trunc(x); |x - t| >= 0.5 ? t + copysign(1, x) : t
                let (x, t) = (self.tv[0], self.tv[1]);
                self.vget(a);
                self.set(x);
                self.get(x);
                self.fd(op::F32X4_TRUNC);
                self.set(t);
                self.get(t);
                self.get(x);
                self.vconst([0x8000_0000; 4]);
                self.fd(op::V128_AND);
                self.vconst([1f32.to_bits(); 4]);
                self.fd(op::V128_OR);
                self.fd(op::F32X4_ADD);
                self.get(t);
                self.get(x);
                self.get(t);
                self.fd(op::F32X4_SUB);
                self.fd(op::F32X4_ABS);
                self.vconst([0.5f32.to_bits(); 4]);
                self.fd(op::F32X4_GE);
                self.fd(op::V128_BITSELECT);
            }
            _ => {
                self.vget(a);
                self.fd(match u {
                    Un::NegF => op::F32X4_NEG,
                    Un::AbsF => op::F32X4_ABS,
                    Un::SqrtF => op::F32X4_SQRT,
                    Un::FloorF => op::F32X4_FLOOR,
                    Un::CeilF => op::F32X4_CEIL,
                    Un::TruncF => op::F32X4_TRUNC,
                    Un::F2I => op::I32X4_TRUNC_SAT_F32X4_S,
                    Un::I2F => op::F32X4_CONVERT_I32X4_S,
                    Un::NegI => op::I32X4_NEG,
                    _ => unreachable!("f64 is declined"),
                });
            }
        }
    }

    fn bin(&mut self, v: Val, b: Bin, x: Val, y: Val) {
        match b {
            Bin::MinF | Bin::MaxF => {
                // a < b ? a : b  /  a > b ? a : b (false on NaN: b).
                self.vget(x);
                self.vget(y);
                self.vget(x);
                self.vget(y);
                self.fd(if b == Bin::MinF { op::F32X4_LT } else { op::F32X4_GT });
                self.fd(op::V128_BITSELECT);
            }
            Bin::DivI | Bin::RemI => self.div_rem(b, x, y),
            Bin::ShlI | Bin::ShrI | Bin::ShrUI => {
                let vop = match b {
                    Bin::ShlI => op::I32X4_SHL,
                    Bin::ShrI => op::I32X4_SHR_S,
                    _ => op::I32X4_SHR_U,
                };
                if self.uniform(y) {
                    // One amount (wasm takes it mod 32, as AIR).
                    self.vget(x);
                    match self.consts[y.0 as usize] {
                        Some(c) => self.sc.f.i32c(c & 31),
                        None => self.sc.v(y),
                    }
                    self.fd(vop);
                } else {
                    self.var_shift(b, x, y);
                }
            }
            Bin::AndI | Bin::AndB | Bin::OrI | Bin::OrB | Bin::XorI => {
                // And/or of bools: masks when both are, else 0/1 words.
                let raw = matches!(b, Bin::AndB | Bin::OrB) && !self.mask[v.0 as usize];
                self.vget_word(x, raw);
                self.vget_word(y, raw);
                self.fd(match b {
                    Bin::AndI | Bin::AndB => op::V128_AND,
                    Bin::OrI | Bin::OrB => op::V128_OR,
                    _ => op::V128_XOR,
                });
            }
            _ => {
                self.vget(x);
                self.vget(y);
                self.fd(match b {
                    Bin::AddF => op::F32X4_ADD,
                    Bin::SubF => op::F32X4_SUB,
                    Bin::MulF => op::F32X4_MUL,
                    Bin::DivF => op::F32X4_DIV,
                    Bin::AddI => op::I32X4_ADD,
                    Bin::SubI => op::I32X4_SUB,
                    Bin::MulI => op::I32X4_MUL,
                    _ => unreachable!("f64 is declined"),
                });
            }
        }
    }

    /// Pushes 2^(k - amt) per lane (`down`) or 2^amt, for the amounts in
    /// local `amt` (exponents in 0..32): the f32 with that exponent,
    /// converted (exact).
    fn pow2(&mut self, down: Option<u32>, amt: u32) {
        match down {
            Some(k) => {
                self.vconst([127 + k; 4]);
                self.get(amt);
                self.fd(op::I32X4_SUB);
            }
            None => {
                self.get(amt);
                self.vconst([127; 4]);
                self.fd(op::I32X4_ADD);
            }
        }
        self.sc.f.i32c(23);
        self.fd(op::I32X4_SHL);
        self.fd(op::I32X4_TRUNC_SAT_F32X4_U);
    }

    /// A shift by per-lane amounts (SIMD128 shifts take one amount):
    /// `x << s` is x * 2^s (wrapping); `x >>> s` the 64-bit product
    /// x * 2^(31 - s) shifted right by 31; `x >> s` is `(x ^ m) >>> s ^ m`
    /// with m the sign mask. Amounts mod 32 unless proven below 32.
    fn var_shift(&mut self, b: Bin, x: Val, y: Val) {
        let (tx, amt, tp, sign) = (self.tv[0], self.tv[1], self.tv[2], self.tv[3]);
        // A right shift by `a + c` (c >= 1, the sum proven below 32, as in
        // the hash's `s >>> ((s >>> 28) + 4)`): x * 2^(32 - c - a) fits 32
        // bits, its product's high word is the shift, and c folds into
        // the exponent.
        let sum = match self.defs[y.0 as usize] {
            Some(Op::Bin(Bin::AddI, p, q)) if b != Bin::ShlI => {
                let c = |v: Val, w: &W| if w.uniform(v) { w.consts[v.0 as usize] } else { None };
                let (a, c) = match (c(p, self), c(q, self)) {
                    (_, Some(c)) => (p, c),
                    (Some(c), _) => (q, c),
                    _ => (p, 0),
                };
                self.sc.bounds[a.0 as usize].filter(|m| c >= 1 && (*m as u64) + (c as u64) < 32).map(|_| (a, c as u32))
            }
            _ => None,
        };
        match sum {
            Some((a, _)) => self.vget(a),
            None => {
                self.vget(y);
                if !self.sc.bounds[y.0 as usize].is_some_and(|m| m < 32) {
                    self.vconst([31; 4]);
                    self.fd(op::V128_AND);
                }
            }
        }
        self.set(amt);
        if b == Bin::ShlI {
            self.vget(x);
            self.pow2(None, amt);
            self.fd(op::I32X4_MUL);
            return;
        }
        self.vget(x);
        if b == Bin::ShrI {
            self.set(tx);
            self.get(tx);
            self.sc.f.i32c(31);
            self.fd(op::I32X4_SHR_S);
            self.set(sign);
            self.get(tx);
            self.get(sign);
            self.fd(op::V128_XOR);
        }
        self.set(tx);
        let k = match sum {
            Some((_, c)) => 32 - c,
            None => 31,
        };
        self.pow2(Some(k), amt);
        self.set(tp);
        for half in [op::I64X2_EXTMUL_LOW_I32X4_U, op::I64X2_EXTMUL_HIGH_I32X4_U] {
            self.get(tx);
            self.get(tp);
            self.fd(half);
            if sum.is_none() {
                self.sc.f.i32c(31);
                self.fd(op::I64X2_SHR_U);
            }
        }
        if sum.is_some() {
            // The high words.
            self.shuffle([4, 5, 6, 7, 12, 13, 14, 15, 20, 21, 22, 23, 28, 29, 30, 31]);
        } else {
            self.shuffle([0, 1, 2, 3, 8, 9, 10, 11, 16, 17, 18, 19, 24, 25, 26, 27]);
        }
        if b == Bin::ShrI {
            self.get(sign);
            self.fd(op::V128_XOR);
        }
    }

    /// Pushes `x / d` per lane (truncating) for a constant d in 2..2^31 by
    /// the magic multiplier; x in local `tx`.
    fn magic_div(&mut self, tx: u32, d: u32) {
        let (m, sh) = ir::magic_s32(d);
        for half in [op::I64X2_EXTMUL_LOW_I32X4_S, op::I64X2_EXTMUL_HIGH_I32X4_S] {
            self.get(tx);
            self.vconst([m as u32; 4]);
            self.fd(half);
        }
        // The high words of the four 64-bit products.
        self.shuffle([4, 5, 6, 7, 12, 13, 14, 15, 20, 21, 22, 23, 28, 29, 30, 31]);
        if m < 0 {
            self.get(tx);
            self.fd(op::I32X4_ADD);
        }
        if sh > 0 {
            self.sc.f.i32c(sh as i32);
            self.fd(op::I32X4_SHR_S);
        }
        self.get(tx);
        self.sc.f.i32c(31);
        self.fd(op::I32X4_SHR_U);
        self.fd(op::I32X4_ADD);
    }

    fn div_rem(&mut self, b: Bin, x: Val, y: Val) {
        let div = b == Bin::DivI;
        let c = if self.uniform(y) { self.consts[y.0 as usize] } else { None };
        let (tx, tq) = (self.tv[0], self.tv[1]);
        match c {
            Some(0) if div => self.vconst([0; 4]),
            Some(0) => self.vget(x),
            Some(1) if div => self.vget(x),
            Some(1 | -1) if !div => self.vconst([0; 4]),
            Some(-1) => {
                self.vget(x);
                self.fd(op::I32X4_NEG);
            }
            Some(c) if c > 1 => {
                self.vget(x);
                self.set(tx);
                if (c as u32).is_power_of_two() {
                    // q = (x + ((x >> 31) >>> (32 - k))) >> k (toward zero)
                    let k = (c as u32).trailing_zeros();
                    self.get(tx);
                    self.get(tx);
                    self.sc.f.i32c(31);
                    self.fd(op::I32X4_SHR_S);
                    self.sc.f.i32c(32 - k as i32);
                    self.fd(op::I32X4_SHR_U);
                    self.fd(op::I32X4_ADD);
                    self.sc.f.i32c(k as i32);
                    self.fd(op::I32X4_SHR_S);
                } else {
                    self.magic_div(tx, c as u32);
                }
                if !div {
                    // r = x - q * c
                    self.set(tq);
                    self.get(tx);
                    self.get(tq);
                    self.vconst([c as u32; 4]);
                    self.fd(op::I32X4_MUL);
                    self.fd(op::I32X4_SUB);
                }
            }
            _ => {
                // Lane by lane, with AIR's rules for 0 and -1.
                let [a, d, t, _] = self.tl;
                self.vget(x);
                for l in 0..4 {
                    self.lane_i32(x, l);
                    self.set(a);
                    self.lane_i32(y, l);
                    self.set(d);
                    if div {
                        div_s(&mut self.sc.f, a, d, t);
                    } else {
                        rem_s(&mut self.sc.f, a, d);
                    }
                    self.lane_op(op::I32X4_REPLACE_LANE, l);
                }
            }
        }
    }

    fn wrap(&mut self, x: Val, len: u32) {
        let (tx, tr) = (self.tv[0], self.tv[1]);
        if len.is_power_of_two() {
            self.vget(x);
            self.vconst([len - 1; 4]);
            self.fd(op::V128_AND);
        } else if len < 1 << 31 {
            // r = x - (x / len) * len; r + (r < 0 ? len : 0)
            self.vget(x);
            self.set(tx);
            self.get(tx);
            self.magic_div(tx, len);
            self.vconst([len; 4]);
            self.fd(op::I32X4_MUL);
            self.fd(op::I32X4_SUB);
            self.set(tr);
            self.get(tr);
            self.get(tr);
            self.sc.f.i32c(31);
            self.fd(op::I32X4_SHR_S);
            self.vconst([len; 4]);
            self.fd(op::V128_AND);
            self.fd(op::I32X4_ADD);
        } else {
            let t = self.tl[0];
            self.vget(x);
            for l in 0..4 {
                self.lane_i32(x, l);
                wrap_rem(&mut self.sc.f, len, t);
                self.lane_op(op::I32X4_REPLACE_LANE, l);
            }
        }
    }

    fn fma(&mut self, k: Fma, a: Val, b: Val, c: Val) {
        if k == Fma::MulAddI {
            self.vget(a);
            self.vget(b);
            self.fd(op::I32X4_MUL);
            self.vget(c);
            self.fd(op::I32X4_ADD);
            return;
        }
        if self.relaxed {
            self.vget(a);
            self.vget(b);
            self.vget(c);
            if k == Fma::Sub {
                self.fd(op::F32X4_NEG);
            }
            self.fd(if k == Fma::SubFrom { op::F32X4_RELAXED_NMADD } else { op::F32X4_RELAXED_MADD });
            return;
        }
        // Exact: per half, p = a * b in f64 (exact: 48 bits) and s = p + c;
        // s rounded to f32 is the fused result unless s lands on an f32
        // midpoint or in the f32 subnormal range (the scalar code's test).
        // Those rare lanes are fixed through the scalar sequence ([`fma_fix`],
        // out of line); the operands and the result pass through the
        // frame's head (V8 keeps the loop fast around a rare branch only
        // when that branch's code touches no vector). Round to odd without
        // a branch (TwoSum's error nudging the last bit) measured slower.
        let [ta, tb, tc, plo, phi, slo, shi, _] = self.tv;
        self.vget(a);
        if k == Fma::SubFrom {
            self.fd(op::F32X4_NEG);
        }
        self.set(ta);
        self.vget(b);
        self.set(tb);
        self.vget(c);
        if k == Fma::Sub {
            self.fd(op::F32X4_NEG);
        }
        self.set(tc);
        for (t, at) in [(ta, 16), (tb, 32), (tc, 48)] {
            self.get(P_FRAME);
            self.get(t);
            self.vmem(op::V128_STORE, 4, at);
        }
        const HI: [u8; 16] = [8, 9, 10, 11, 12, 13, 14, 15, 8, 9, 10, 11, 12, 13, 14, 15];
        for (hi, p, s) in [(false, plo, slo), (true, phi, shi)] {
            for t in [ta, tb] {
                self.get(t);
                if hi {
                    self.get(t);
                    self.shuffle(HI);
                }
                self.fd(op::F64X2_PROMOTE_LOW_F32X4);
            }
            self.fd(op::F64X2_MUL);
            self.set(p);
            self.get(p);
            self.get(tc);
            if hi {
                self.get(tc);
                self.shuffle(HI);
            }
            self.fd(op::F64X2_PROMOTE_LOW_F32X4);
            self.fd(op::F64X2_ADD);
            self.set(s);
        }
        // The rounded result, at the frame's head.
        self.get(P_FRAME);
        self.get(slo);
        self.fd(op::F32X4_DEMOTE_F64X2_ZERO);
        self.get(shi);
        self.fd(op::F32X4_DEMOTE_F64X2_ZERO);
        self.shuffle([0, 1, 2, 3, 4, 5, 6, 7, 16, 17, 18, 19, 20, 21, 22, 23]);
        self.vmem(op::V128_STORE, 4, 0);
        // Any lane on a midpoint (low 29 bits 1 << 28) or 0 < |s| < 2^-125
        // (the high word below 0x3820_0000 without its sign)? The four
        // sums' low words, then their high words, as one i32x4 each.
        const LO: [u8; 16] = [0, 1, 2, 3, 8, 9, 10, 11, 16, 17, 18, 19, 24, 25, 26, 27];
        const HI_W: [u8; 16] = [4, 5, 6, 7, 12, 13, 14, 15, 20, 21, 22, 23, 28, 29, 30, 31];
        self.get(slo);
        self.get(shi);
        self.shuffle(LO);
        self.vconst([0x1FFF_FFFF; 4]);
        self.fd(op::V128_AND);
        self.vconst([0x1000_0000; 4]);
        self.fd(op::I32X4_EQ);
        self.get(slo);
        self.get(shi);
        self.shuffle(HI_W);
        self.vconst([0x7FFF_FFFF; 4]);
        self.fd(op::V128_AND);
        // 0 < |s| < 2^-125: an exact zero is already the fused result (a
        // product of f32s and an f32 sum to 0 or to at least 2^-298).
        self.vconst([1; 4]);
        self.fd(op::I32X4_SUB);
        self.vconst([0x381F_FFFF; 4]);
        self.fd(op::I32X4_LT_U);
        self.fd(op::V128_OR);
        self.fd(op::V128_ANY_TRUE);
        self.open_if(op::VOID);
        self.get(P_FRAME);
        self.sc.f.b(op::CALL);
        self.sc.f.u(self.fix);
        self.uses_fix = true;
        self.end();
        self.get(P_FRAME);
        self.vmem(op::V128_LOAD, 4, 0);
    }
}

/// The exact multiply-add's fix-up, `fix(at)`: the four lanes' a, b, c at
/// `at + 16`, `+ 32`, `+ 48` (f32x4), each lane's fused result through the
/// scalar sequence into `at` (the rare lanes where rounding the f64 sum to
/// f32 would round twice; the others give the same bits again). One
/// function per module, called out of line: inlined at every multiply-add
/// it made modules many times larger for no speed.
pub(super) fn fma_fix() -> Body {
    let mut f = Body::with_params(1);
    let tf = [f.local(op::F64), f.local(op::F64), f.local(op::F64), f.local(op::F64)];
    let bits = f.local(op::I64);
    fma_fix_lanes(&mut f, 0, tf, bits);
    f
}

fn fma_fix_lanes(f: &mut Body, at_local: u32, tf: [u32; 4], bits: u32) {
    for l in 0..4u32 {
        f.get(at_local);
        for (k, at) in [(None, 16), (Some(0), 32), (Some(1), 48)] {
            f.get(at_local);
            f.mem(op::F32_LOAD, at + 4 * l);
            f.b(op::F64_PROMOTE_F32);
            if k == Some(0) {
                f.b(op::F64_MUL);
            }
            if let Some(k) = k {
                f.set(tf[k]);
            }
        }
        fma_tail(f, tf, bits);
        f.mem(op::F32_STORE, 4 * l);
    }
}

