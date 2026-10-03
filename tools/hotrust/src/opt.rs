//! Cheap, target-independent tier-0 optimisations on RIR. Every pass is linear
//! (or a few linear sweeps) in the function size:
//! - inlining of tiny leaf callees (bottom-up: callees are already optimised),
//! - constant folding incl. loads from immutable data (const arrays, literals),
//! - copy propagation for single-definition vregs,
//! - immediate operand forms, branch folding, jump threading, dead code removal,
//! - block re-layout so fall-through follows the hot edge.

use crate::regalloc::{reachable, term_uses, uses_defs};
use std::cell::Cell;

thread_local! {
    /// CPU ns per pass: 0 fold/copy-prop, 1 cse, 2 local copy-prop, 3 dce, 4 jumps+merge,
    /// 5 inline, 6 sink, 7 layout
    pub static PASS_NS: [Cell<u64>; 8] = const { [const { Cell::new(0) }; 8] };
}

/// Per-pass timing is opt-in (HOTRUST_PASS_TIMES): the thread CPU clock is a syscall.
pub fn pass_clock() -> u64 {
    if PASS_ON.with(|c| c.get()) {
        crate::jit::cpu_ns()
    } else {
        0
    }
}

thread_local! {
    static PASS_ON: Cell<bool> = Cell::new(std::env::var("HOTRUST_PASS_TIMES").is_ok());
}

pub fn pass_time(i: usize, t0: u64) {
    if t0 == 0 {
        return;
    }
    let now = crate::jit::cpu_ns();
    PASS_NS.with(|p| p[i].set(p[i].get() + (now - t0)));
}

pub const PASS_NAMES: [&str; 8] = ["fold", "cse", "lcopy", "dce", "jumps", "inline", "sink", "layout"];
use crate::rir::*;

#[derive(Clone, Copy)]
pub struct OptCfg {
    /// tier 0: inline HotRust core's builtins (scalar/math/slice helpers) into callers
    pub inline: bool,
    /// also inline small non-recursive leaf functions of user code (budget-based, tier 0)
    pub inline_all: bool,
    pub fold: bool,
    pub cse: bool,
    pub sroa: bool,
    pub licm: bool,
    pub imm: bool,
    pub layout: bool,
}

impl OptCfg {
    /// `HOTRUST_OPT=none` or a comma list of passes to disable (`-inline,-fold`).
    pub fn from_env() -> OptCfg {
        let mut c = OptCfg { inline: true, inline_all: true, fold: true, cse: true, sroa: true, licm: true, imm: true, layout: true };
        if let Ok(v) = std::env::var("HOTRUST_OPT") {
            if v == "none" {
                c = OptCfg { inline: false, inline_all: false, fold: false, cse: false, sroa: false, licm: false, imm: false, layout: false };
            }
            for part in v.split(',') {
                match part {
                    "-inline" => c.inline = false,
                    "-fold" => c.fold = false,
                    "-cse" => c.cse = false,
                    "-sroa" => c.sroa = false,
                    "-licm" => c.licm = false,
                    "-imm" => c.imm = false,
                    "-layout" => c.layout = false,
                    "+inline_all" => c.inline_all = true,
                    "-inline_all" => c.inline_all = false,
                    _ => {}
                }
            }
        }
        c
    }
}

pub const INLINE_MAX_INSTS: usize = 40;
pub const INLINE_MAX_BLOCKS: usize = 8;
pub const INLINE_BUDGET: usize = 600;

/// Leaf-inlining budget (insts, blocks); HOTRUST_INLINE_LEAF=insts,blocks overrides.
pub fn leaf_budget() -> (usize, usize) {
    thread_local! {
        static B: (usize, usize) = {
            match std::env::var("HOTRUST_INLINE_LEAF") {
                Ok(v) => {
                    let mut it = v.split(',');
                    let a = it.next().and_then(|x| x.parse().ok()).unwrap_or(INLINE_MAX_INSTS);
                    let b = it.next().and_then(|x| x.parse().ok()).unwrap_or(INLINE_MAX_BLOCKS);
                    (a, b)
                }
                Err(_) => (INLINE_MAX_INSTS, INLINE_MAX_BLOCKS),
            }
        };
    }
    B.with(|b| *b)
}

/// Inlining rules (tier 0, fixed and cheap; counted per rule in the stats):
pub const RULE_TINY: usize = 0; // body no bigger than the call sequence it replaces
pub const RULE_ALWAYS: usize = 1; // #[inline(always)], or #[inline] and small
pub const RULE_SMALL_LEAF: usize = 2; // small leaf under the size budget
pub const RULE_NAMES: [&str; 3] = ["tiny(<=call)", "inline-attr", "small-leaf"];
pub const ATTR_ALWAYS: u8 = 1;
pub const ATTR_HINT: u8 = 2;

fn callee_shape(f: &Func, callee_id: u32) -> (usize, bool, bool) {
    // (instruction count, leaf, recursive)
    let mut n = 0;
    let mut leaf = true;
    let mut rec = false;
    for b in &f.blocks {
        n += b.insts.len();
        for i in &b.insts {
            match i {
                Inst::Call(Callee::Host(_), _, _) | Inst::Call(Callee::CHost(..), _, _) => {}
                Inst::Call(Callee::Fn(c), _, _) => {
                    leaf = false;
                    if *c == callee_id {
                        rec = true;
                    }
                }
                Inst::Call(..) => leaf = false,
                _ => {}
            }
        }
    }
    (n, leaf, rec)
}

/// Which rule (if any) inlines this call.
pub fn inline_rule(f: &Func, callee_id: u32, nargs: usize, nrets: usize, attrs: u8) -> Option<usize> {
    let (n, leaf, rec) = callee_shape(f, callee_id);
    if rec {
        return None;
    }
    if n <= nargs + nrets + 2 && f.blocks.len() <= 2 {
        return Some(RULE_TINY);
    }
    if attrs & ATTR_ALWAYS != 0 && n <= 200 {
        return Some(RULE_ALWAYS);
    }
    if attrs & ATTR_HINT != 0 && n <= 60 && leaf {
        return Some(RULE_ALWAYS);
    }
    let (max_insts, max_blocks) = leaf_budget();
    if leaf && n <= max_insts && f.blocks.len() <= max_blocks {
        return Some(RULE_SMALL_LEAF);
    }
    None
}

/// Inlines calls chosen by `inline_rule`. `get(id)` returns a callee's optimised RIR and
/// its inline attributes. Returns (inlined callee slot, rule) per inlined call site.
pub fn inline_calls(f: &mut Func, self_id: u32, get: &dyn Fn(u32) -> Option<(*const Func, u8)>) -> Vec<(u32, usize)> {
    let mut inlined = Vec::new();
    let mut budget = INLINE_BUDGET;
    let mut bi = 0;
    while bi < f.blocks.len() {
        let mut ii = 0;
        while ii < f.blocks[bi].insts.len() {
            let (id, args, rets) = match &f.blocks[bi].insts[ii] {
                Inst::Call(Callee::Fn(id), a, r) if *id != self_id => (*id, a.clone(), r.clone()),
                _ => {
                    ii += 1;
                    continue;
                }
            };
            let (cp, attrs) = match get(id) {
                Some(p) => p,
                None => {
                    ii += 1;
                    continue;
                }
            };
            let callee: &Func = unsafe { &*cp };
            let mut size = 0;
            for b in &callee.blocks {
                size += b.insts.len();
            }
            let rule = inline_rule(callee, id, args.len(), rets.len(), attrs);
            let rule = match rule {
                Some(r) if size <= budget && callee.params.len() == args.len() && callee.rets.len() == rets.len() => r,
                _ => {
                    ii += 1;
                    continue;
                }
            };
            budget -= size;
            inlined.push((id, rule));
            // split the block after the call
            let blk = &mut f.blocks[bi];
            let tail = blk.insts.split_off(ii + 1);
            let tail_pos = blk.pos.split_off(ii + 1);
            blk.insts.pop();
            let call_pos = blk.pos.pop().unwrap_or(0);
            let old_term = std::mem::replace(&mut blk.term, Term::Unreachable);
            let old_term_pos = blk.term_pos;
            let cont = f.blocks.len() as u32;
            f.blocks.push(Block { insts: tail, pos: tail_pos, term: old_term, term_pos: old_term_pos });
            // map callee vregs and slots
            let mut vmap = Vec::with_capacity(callee.vregs.len());
            for c in &callee.vregs {
                vmap.push(f.vreg(*c));
            }
            let slot_base = f.slots.len() as u32;
            for s in &callee.slots {
                f.slot(s.size, s.align);
            }
            let boff = f.blocks.len() as u32;
            // parameter moves, then jump into the callee body
            for (k, p) in callee.params.iter().enumerate() {
                let d = vmap[p.0 as usize];
                f.blocks[bi].insts.push(Inst::Mov(d, args[k]));
                f.blocks[bi].pos.push(call_pos);
            }
            f.blocks[bi].term = Term::Jump(boff);
            f.blocks[bi].term_pos = call_pos;
            let m = |v: VReg| vmap[v.0 as usize];
            for cb in &callee.blocks {
                let mut insts = Vec::with_capacity(cb.insts.len());
                for x in &cb.insts {
                    insts.push(map_inst(x, &m, slot_base));
                }
                let mut pos = cb.pos.clone();
                let term = match &cb.term {
                    Term::Jump(t) => Term::Jump(t + boff),
                    Term::Branch(c, t, e) => Term::Branch(m(*c), t + boff, e + boff),
                    Term::Ret(vals) => {
                        for (k, v) in vals.iter().enumerate() {
                            insts.push(Inst::Mov(rets[k], m(*v)));
                            pos.push(cb.term_pos);
                        }
                        Term::Jump(cont)
                    }
                    Term::Unreachable => Term::Unreachable,
                };
                f.blocks.push(Block { insts, pos, term, term_pos: cb.term_pos });
            }
            break;
        }
        bi += 1;
    }
    inlined
}

fn map_inst(x: &Inst, m: &dyn Fn(VReg) -> VReg, slot_base: u32) -> Inst {
    match x {
        Inst::Iconst(d, v) => Inst::Iconst(m(*d), *v),
        Inst::Fconst(d, v, w) => Inst::Fconst(m(*d), *v, *w),
        Inst::Mov(d, a) => Inst::Mov(m(*d), m(*a)),
        Inst::IBin(o, t, d, a, b) => Inst::IBin(*o, *t, m(*d), m(*a), m(*b)),
        Inst::IBinI(o, t, d, a, k) => Inst::IBinI(*o, *t, m(*d), m(*a), *k),
        Inst::INeg(t, d, a) => Inst::INeg(*t, m(*d), m(*a)),
        Inst::INot(t, d, a) => Inst::INot(*t, m(*d), m(*a)),
        Inst::ICmp(c, s, d, a, b) => Inst::ICmp(*c, *s, m(*d), m(*a), m(*b)),
        Inst::ICmpI(c, s, d, a, k) => Inst::ICmpI(*c, *s, m(*d), m(*a), *k),
        Inst::FBin(o, w, d, a, b) => Inst::FBin(*o, *w, m(*d), m(*a), m(*b)),
        Inst::FUnary(o, w, d, a) => Inst::FUnary(*o, *w, m(*d), m(*a)),
        Inst::FCmp(c, w, d, a, b) => Inst::FCmp(*c, *w, m(*d), m(*a), m(*b)),
        Inst::Conv(c, d, a) => Inst::Conv(*c, m(*d), m(*a)),
        Inst::Load(k, d, b, o) => Inst::Load(*k, m(*d), m(*b), *o),
        Inst::Store(k, b, o, s) => Inst::Store(*k, m(*b), *o, m(*s)),
        Inst::SlotAddr(d, s) => Inst::SlotAddr(m(*d), s + slot_base),
        Inst::Addr(d, a) => Inst::Addr(m(*d), *a),
        Inst::FnAddr(d, id) => Inst::FnAddr(m(*d), *id),
        Inst::Call(c, a, r) => {
            let c2 = match c {
                Callee::Indirect(v) => Callee::Indirect(m(*v)),
                Callee::Fn(i) => Callee::Fn(*i),
                Callee::Host(h) => Callee::Host(*h),
                Callee::CHost(h, sig) => Callee::CHost(*h, sig.clone()),
                Callee::CIndirect(v, sig) => Callee::CIndirect(m(*v), sig.clone()),
                Callee::CFn(i, sig) => Callee::CFn(*i, sig.clone()),
            };
            let mut a2 = Vec::new();
            for v in a {
                a2.push(m(*v));
            }
            let mut r2 = Vec::new();
            for v in r {
                r2.push(m(*v));
            }
            Inst::Call(c2, a2, r2)
        }
        Inst::Copy(a, b, n) => Inst::Copy(m(*a), m(*b), *n),
        Inst::LoadX(k, d, b, x, sh, o) => Inst::LoadX(*k, m(*d), m(*b), m(*x), *sh, *o),
        Inst::StoreX(k, b, x, sh, o, v) => Inst::StoreX(*k, m(*b), m(*x), *sh, *o, m(*v)),
        Inst::AtomicLoad(k, o, d, a) => Inst::AtomicLoad(*k, *o, m(*d), m(*a)),
        Inst::AtomicStore(k, o, a, v) => Inst::AtomicStore(*k, *o, m(*a), m(*v)),
        Inst::AtomicRmw(op, k, o, d, a, v) => Inst::AtomicRmw(*op, *k, *o, m(*d), m(*a), m(*v)),
        Inst::AtomicCas(k, o1, o2, d, a, e, n) => Inst::AtomicCas(*k, *o1, *o2, m(*d), m(*a), m(*e), m(*n)),
        Inst::Fence(o, st) => Inst::Fence(*o, *st),
        Inst::TlsAddr(d, off) => Inst::TlsAddr(m(*d), *off),
    }
}

// ------------------------------------------------------------ folding

#[derive(Clone, Copy, PartialEq, Debug)]
enum K {
    I(i64),
    F(u64, bool),
    A(u64),
}

fn ext(v: i64, it: IntTy) -> i64 {
    crate::lower::extend_const(v, it)
}

fn fold_ibin(op: IOp, it: IntTy, a: i64, b: i64) -> Option<i64> {
    let r = match op {
        IOp::Add => a.wrapping_add(b),
        IOp::Sub => a.wrapping_sub(b),
        IOp::Mul => a.wrapping_mul(b),
        IOp::And => a & b,
        IOp::Or => a | b,
        IOp::Xor => a ^ b,
        IOp::Shl => a.wrapping_shl((b & 63) as u32),
        IOp::Shr => {
            if it.signed {
                a.wrapping_shr((b & 63) as u32)
            } else {
                ((a as u64) >> (b & 63)) as i64
            }
        }
        IOp::Div | IOp::Rem => {
            if b == 0 || (it.signed && b == -1) {
                return None;
            }
            if it.signed {
                if op == IOp::Div {
                    a / b
                } else {
                    a % b
                }
            } else if op == IOp::Div {
                ((a as u64) / (b as u64)) as i64
            } else {
                ((a as u64) % (b as u64)) as i64
            }
        }
    };
    Some(ext(r, it))
}

fn cmp_i(c: Cond, signed: bool, a: i64, b: i64) -> bool {
    if signed {
        match c {
            Cond::Eq => a == b,
            Cond::Ne => a != b,
            Cond::Lt => a < b,
            Cond::Le => a <= b,
            Cond::Gt => a > b,
            Cond::Ge => a >= b,
        }
    } else {
        let (a, b) = (a as u64, b as u64);
        match c {
            Cond::Eq => a == b,
            Cond::Ne => a != b,
            Cond::Lt => a < b,
            Cond::Le => a <= b,
            Cond::Gt => a > b,
            Cond::Ge => a >= b,
        }
    }
}

fn fval(bits: u64, w: bool) -> f64 {
    if w {
        f64::from_bits(bits)
    } else {
        f32::from_bits(bits as u32) as f64
    }
}

fn fold_fbin(op: FOp, w: bool, a: u64, b: u64) -> u64 {
    if w {
        let (x, y) = (f64::from_bits(a), f64::from_bits(b));
        let r = match op {
            FOp::Add => x + y,
            FOp::Sub => x - y,
            FOp::Mul => x * y,
            FOp::Div => x / y,
            FOp::Min => x.min(y),
            FOp::Max => x.max(y),
        };
        r.to_bits()
    } else {
        let (x, y) = (f32::from_bits(a as u32), f32::from_bits(b as u32));
        let r = match op {
            FOp::Add => x + y,
            FOp::Sub => x - y,
            FOp::Mul => x * y,
            FOp::Div => x / y,
            FOp::Min => x.min(y),
            FOp::Max => x.max(y),
        };
        r.to_bits() as u64
    }
}

fn fold_fun(op: FUn, w: bool, a: u64) -> u64 {
    if w {
        let x = f64::from_bits(a);
        let r = match op {
            FUn::Neg => -x,
            FUn::Abs => x.abs(),
            FUn::Sqrt => x.sqrt(),
            FUn::Floor => x.floor(),
            FUn::Ceil => x.ceil(),
            FUn::Trunc => x.trunc(),
            FUn::RoundEven => x.round_ties_even(),
        };
        r.to_bits()
    } else {
        let x = f32::from_bits(a as u32);
        let r = match op {
            FUn::Neg => -x,
            FUn::Abs => x.abs(),
            FUn::Sqrt => x.sqrt(),
            FUn::Floor => x.floor(),
            FUn::Ceil => x.ceil(),
            FUn::Trunc => x.trunc(),
            FUn::RoundEven => x.round_ties_even(),
        };
        r.to_bits() as u64
    }
}

fn cmp_f(c: Cond, a: f64, b: f64) -> bool {
    match c {
        Cond::Eq => a == b,
        Cond::Ne => a != b,
        Cond::Lt => a < b,
        Cond::Le => a <= b,
        Cond::Gt => a > b,
        Cond::Ge => a >= b,
    }
}

fn f_to_int(x: f64, it: IntTy) -> i64 {
    let v = match (it.bits, it.signed) {
        (8, true) => x as i8 as i64,
        (8, false) => x as u8 as i64,
        (16, true) => x as i16 as i64,
        (16, false) => x as u16 as i64,
        (32, true) => x as i32 as i64,
        (32, false) => x as u32 as i64,
        (_, true) => x as i64,
        (_, false) => x as u64 as i64,
    };
    v
}

fn fold_conv(c: Conv, k: K) -> Option<K> {
    Some(match (c, k) {
        (Conv::IntToInt(it), K::I(v)) => K::I(ext(v, it)),
        (Conv::IntToInt(_), K::A(a)) => K::A(a),
        (Conv::IntToF(from, w), K::I(v)) => {
            let x = if from.bits == 64 && !from.signed { (v as u64) as f64 } else { v as f64 };
            if w {
                K::F(x.to_bits(), true)
            } else {
                // single rounding to f32
                let y = if from.bits == 64 && !from.signed { (v as u64) as f32 } else { v as f32 };
                K::F(y.to_bits() as u64, false)
            }
        }
        (Conv::FToInt(w, it), K::F(b, _)) => K::I(f_to_int(fval(b, w), it)),
        (Conv::F32ToF64, K::F(b, _)) => K::F((f32::from_bits(b as u32) as f64).to_bits(), true),
        (Conv::F64ToF32, K::F(b, _)) => K::F((f64::from_bits(b) as f32).to_bits() as u64, false),
        (Conv::BitsToF(w), K::I(v)) => K::F(if w { v as u64 } else { v as u32 as u64 }, w),
        (Conv::FToBits(w), K::F(b, _)) => K::I(if w { b as i64 } else { b as u32 as i64 }),
        _ => return None,
    })
}

fn swap_cond(c: Cond) -> Cond {
    match c {
        Cond::Lt => Cond::Gt,
        Cond::Le => Cond::Ge,
        Cond::Gt => Cond::Lt,
        Cond::Ge => Cond::Le,
        x => x,
    }
}

fn is_pure(i: &Inst) -> bool {
    !is_mem_effect(i)
}

/// Instructions with effects beyond their result vregs (memory writes, calls, ordering).
fn is_mem_effect(i: &Inst) -> bool {
    matches!(i, Inst::Call(..) | Inst::Store(..) | Inst::StoreX(..) | Inst::Copy(..) | Inst::AtomicLoad(..) | Inst::AtomicStore(..) | Inst::AtomicRmw(..) | Inst::AtomicCas(..) | Inst::Fence(..))
}

fn const_inst(d: VReg, k: K) -> Inst {
    match k {
        K::I(v) => Inst::Iconst(d, v),
        K::F(b, w) => Inst::Fconst(d, b, w),
        K::A(a) => Inst::Addr(d, a),
    }
}

/// Folding, copy propagation, immediates, branch folding and DCE.
/// `ro` is the immutable data range (loads from it at constant addresses fold).
pub fn simplify(f: &mut Func, ro: (u64, u64), imm: bool, cse: bool) {
    let nv = f.vregs.len();
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    for _round in 0..4 {
        let tf = pass_clock();
        let reach = reachable(f);
        // definition counts (params count as a definition)
        let mut ndef = vec![0u32; nv];
        for p in &f.params {
            ndef[p.0 as usize] += 1;
        }
        for (bi, b) in f.blocks.iter().enumerate() {
            if !reach[bi] {
                continue;
            }
            for i in &b.insts {
                uses_defs(i, &mut uses, &mut defs);
                for d in &defs {
                    ndef[d.0 as usize] += 1;
                }
            }
        }
        let single = |v: VReg, ndef: &Vec<u32>| ndef[v.0 as usize] == 1;
        // known constants and copy aliases of single-def vregs
        let mut kn: Vec<Option<K>> = vec![None; nv];
        let mut alias: Vec<u32> = (0..nv as u32).collect();
        let mut def_inst: Vec<Option<(IOp, VReg, i64)>> = vec![None; nv];
        // single-def integer compares: (cond, signed, a, b-or-imm, is_imm); operands must be
        // single-def too so the copy reads the same values
        let mut cmp_def: Vec<Option<(Cond, bool, VReg, i64, bool)>> = vec![None; nv];
        for (bi, b) in f.blocks.iter().enumerate() {
            if !reach[bi] {
                continue;
            }
            for i in &b.insts {
                match i {
                    Inst::ICmpI(c, sg, d, a, k) if single(*d, &ndef) && single(*a, &ndef) => cmp_def[d.0 as usize] = Some((*c, *sg, *a, *k, true)),
                    Inst::ICmp(c, sg, d, a, b2) if single(*d, &ndef) && single(*a, &ndef) && single(*b2, &ndef) => cmp_def[d.0 as usize] = Some((*c, *sg, *a, b2.0 as i64, false)),
                    _ => {}
                }
            }
        }
        for (bi, b) in f.blocks.iter().enumerate() {
            if !reach[bi] {
                continue;
            }
            for i in &b.insts {
                match i {
                    Inst::Iconst(d, v) if single(*d, &ndef) => kn[d.0 as usize] = Some(K::I(*v)),
                    Inst::Fconst(d, v, w) if single(*d, &ndef) => kn[d.0 as usize] = Some(K::F(*v, *w)),
                    Inst::Addr(d, a) if single(*d, &ndef) => kn[d.0 as usize] = Some(K::A(*a)),
                    Inst::Mov(d, s) if single(*d, &ndef) && single(*s, &ndef) => alias[d.0 as usize] = s.0,
                    Inst::IBinI(IOp::Add, _, d, a, k) if single(*d, &ndef) && single(*a, &ndef) => def_inst[d.0 as usize] = Some((IOp::Add, *a, *k)),
                    _ => {}
                }
            }
        }
        // resolve alias chains
        for v in 0..nv {
            let mut r = alias[v];
            let mut guard = 0;
            while alias[r as usize] != r && guard < 64 {
                r = alias[r as usize];
                guard += 1;
            }
            alias[v] = r;
        }
        for v in 0..nv {
            if alias[v] != v as u32 && kn[v].is_none() {
                kn[v] = kn[alias[v] as usize];
            }
        }
        let mut changed = false;
        let a = |v: VReg| VReg(alias[v.0 as usize]);
        for bi in 0..f.blocks.len() {
            if !reach[bi] {
                continue;
            }
            for ii in 0..f.blocks[bi].insts.len() {
                // skip instructions with nothing to rename or fold (most of them)
                uses_defs(&f.blocks[bi].insts[ii], &mut uses, &mut defs);
                let mut interesting = false;
                for u in &uses {
                    if alias[u.0 as usize] != u.0 || kn[u.0 as usize].is_some() || def_inst[u.0 as usize].is_some() {
                        interesting = true;
                        break;
                    }
                }
                if !interesting && !matches!(f.blocks[bi].insts[ii], Inst::IBin(..) | Inst::ICmp(..)) {
                    continue;
                }
                let old = f.blocks[bi].insts[ii].clone();
                let mut new = rename(&old, &a);
                let kv = |v: VReg, kn: &Vec<Option<K>>| kn[v.0 as usize];
                let folded: Option<(VReg, K)> = match &new {
                    Inst::Mov(d, s) => kv(*s, &kn).map(|k| (*d, k)),
                    Inst::IBin(op, it, d, x, y) => match (kv(*x, &kn), kv(*y, &kn)) {
                        (Some(K::I(p)), Some(K::I(q))) => fold_ibin(*op, *it, p, q).map(|r| (*d, K::I(r))),
                        (Some(K::A(p)), Some(K::I(q))) if *op == IOp::Add => Some((*d, K::A(p.wrapping_add(q as u64)))),
                        (Some(K::I(p)), Some(K::A(q))) if *op == IOp::Add => Some((*d, K::A(q.wrapping_add(p as u64)))),
                        _ => None,
                    },
                    Inst::IBinI(op, it, d, x, q) => match kv(*x, &kn) {
                        Some(K::I(p)) => fold_ibin(*op, *it, p, *q).map(|r| (*d, K::I(r))),
                        Some(K::A(p)) if *op == IOp::Add => Some((*d, K::A(p.wrapping_add(*q as u64)))),
                        // x & 0, x * 0
                        _ if *q == 0 && matches!(op, IOp::And | IOp::Mul) => Some((*d, K::I(0))),
                        _ => None,
                    },
                    Inst::INeg(it, d, x) => match kv(*x, &kn) {
                        Some(K::I(p)) => Some((*d, K::I(ext(p.wrapping_neg(), *it)))),
                        _ => None,
                    },
                    Inst::INot(it, d, x) => match kv(*x, &kn) {
                        Some(K::I(p)) => Some((*d, K::I(ext(!p, *it)))),
                        _ => None,
                    },
                    Inst::ICmp(c, s, d, x, y) => match (kv(*x, &kn), kv(*y, &kn)) {
                        (Some(K::I(p)), Some(K::I(q))) => Some((*d, K::I(cmp_i(*c, *s, p, q) as i64))),
                        _ => None,
                    },
                    Inst::ICmpI(c, s, d, x, q) => match kv(*x, &kn) {
                        Some(K::I(p)) => Some((*d, K::I(cmp_i(*c, *s, p, *q) as i64))),
                        _ => None,
                    },
                    Inst::FBin(op, w, d, x, y) => match (kv(*x, &kn), kv(*y, &kn)) {
                        (Some(K::F(p, _)), Some(K::F(q, _))) => Some((*d, K::F(fold_fbin(*op, *w, p, q), *w))),
                        _ => None,
                    },
                    Inst::FUnary(op, w, d, x) => match kv(*x, &kn) {
                        Some(K::F(p, _)) => Some((*d, K::F(fold_fun(*op, *w, p), *w))),
                        _ => None,
                    },
                    Inst::FCmp(c, w, d, x, y) => match (kv(*x, &kn), kv(*y, &kn)) {
                        (Some(K::F(p, _)), Some(K::F(q, _))) => Some((*d, K::I(cmp_f(*c, fval(p, *w), fval(q, *w)) as i64))),
                        _ => None,
                    },
                    Inst::Conv(c, d, x) => match kv(*x, &kn) {
                        Some(k) => fold_conv(*c, k).map(|r| (*d, r)),
                        None => None,
                    },
                    Inst::Load(m, d, b, off) => match kv(*b, &kn) {
                        Some(K::A(addr)) => {
                            let at = addr.wrapping_add(*off as i64 as u64);
                            let n = match m {
                                Mem::Int(n, _) => *n as u64,
                                Mem::F32 => 4,
                                Mem::F64 => 8,
                            };
                            if at >= ro.0 && at + n <= ro.1 {
                                let p = at as *const u8;
                                let k = unsafe {
                                    match m {
                                        Mem::Int(1, s) => K::I(if *s { *p as i8 as i64 } else { *p as i64 }),
                                        Mem::Int(2, s) => {
                                            let v = (p as *const u16).read_unaligned();
                                            K::I(if *s { v as i16 as i64 } else { v as i64 })
                                        }
                                        Mem::Int(4, s) => {
                                            let v = (p as *const u32).read_unaligned();
                                            K::I(if *s { v as i32 as i64 } else { v as i64 })
                                        }
                                        Mem::Int(_, _) => K::I((p as *const u64).read_unaligned() as i64),
                                        Mem::F32 => K::F((p as *const u32).read_unaligned() as u64, false),
                                        Mem::F64 => K::F((p as *const u64).read_unaligned(), true),
                                    }
                                };
                                Some((*d, k))
                            } else {
                                None
                            }
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if let Some((d, k)) = folded {
                    new = const_inst(d, k);
                    if single(d, &ndef) && kn[d.0 as usize].is_none() {
                        kn[d.0 as usize] = Some(k);
                    }
                } else {
                    // address arithmetic into load/store offsets; immediates
                    new = match new {
                        Inst::Load(m, d, b, off) => match def_inst[b.0 as usize] {
                            Some((IOp::Add, x, k)) if (off as i64 + k).abs() < (1 << 30) => Inst::Load(m, d, x, off + k as i32),
                            _ => Inst::Load(m, d, b, off),
                        },
                        Inst::Store(m, b, off, s) => match def_inst[b.0 as usize] {
                            Some((IOp::Add, x, k)) if (off as i64 + k).abs() < (1 << 30) => Inst::Store(m, x, off + k as i32, s),
                            _ => Inst::Store(m, b, off, s),
                        },
                        Inst::IBin(op, it, d, x, y) if imm && op != IOp::Div && op != IOp::Rem => {
                            let fits = |k: Option<K>| match k {
                                Some(K::I(v)) if v >= i32::MIN as i64 && v <= i32::MAX as i64 => Some(v),
                                _ => None,
                            };
                            let commutative = matches!(op, IOp::Add | IOp::Mul | IOp::And | IOp::Or | IOp::Xor);
                            if let Some(v) = fits(kn[y.0 as usize]) {
                                Inst::IBinI(op, it, d, x, v)
                            } else if commutative && fits(kn[x.0 as usize]).is_some() {
                                Inst::IBinI(op, it, d, y, fits(kn[x.0 as usize]).unwrap())
                            } else {
                                Inst::IBin(op, it, d, x, y)
                            }
                        }
                        Inst::ICmp(c, s, d, x, y) if imm => {
                            let fits = |k: Option<K>| match k {
                                Some(K::I(v)) if v >= i32::MIN as i64 && v <= i32::MAX as i64 => Some(v),
                                _ => None,
                            };
                            if let Some(v) = fits(kn[y.0 as usize]) {
                                Inst::ICmpI(c, s, d, x, v)
                            } else if let Some(v) = fits(kn[x.0 as usize]) {
                                Inst::ICmpI(swap_cond(c), s, d, y, v)
                            } else {
                                Inst::ICmp(c, s, d, x, y)
                            }
                        }
                        other => other,
                    };
                    if let Inst::IBinI(IOp::Xor, _, d, x, 1) = new {
                        // !cmp -> the inverted integer compare
                        if single(x, &ndef) {
                            if let Some((c, sg, a, kk, is_imm)) = cmp_def[x.0 as usize] {
                                let inv = match c {
                                    Cond::Eq => Cond::Ne,
                                    Cond::Ne => Cond::Eq,
                                    Cond::Lt => Cond::Ge,
                                    Cond::Ge => Cond::Lt,
                                    Cond::Le => Cond::Gt,
                                    Cond::Gt => Cond::Le,
                                };
                                new = if is_imm { Inst::ICmpI(inv, sg, d, a, kk) } else { Inst::ICmp(inv, sg, d, a, VReg(kk as u32)) };
                            }
                        }
                    }
                    if let Inst::IBinI(op, it, d, x, k) = new {
                        let ident = match op {
                            IOp::Add | IOp::Sub | IOp::Or | IOp::Xor | IOp::Shl | IOp::Shr => k == 0,
                            IOp::Mul | IOp::Div => k == 1,
                            _ => false,
                        };
                        // narrow types: the operand is already extended by its type
                        if k == 0 && matches!(op, IOp::And | IOp::Mul) {
                            new = Inst::Iconst(d, 0);
                            if single(d, &ndef) {
                                kn[d.0 as usize] = Some(K::I(0));
                            }
                        } else if ident {
                            new = Inst::Mov(d, x);
                        } else if op == IOp::And && k == -1 {
                            new = Inst::Mov(d, x);
                        } else if op == IOp::Mul && k > 1 && (k & (k - 1)) == 0 && k < (1 << 30) {
                            new = Inst::IBinI(IOp::Shl, it, d, x, k.trailing_zeros() as i64);
                        }
                    }
                    if let Inst::IBinI(IOp::Add, _, d, x, k) = new {
                        if single(d, &ndef) && single(x, &ndef) {
                            def_inst[d.0 as usize] = Some((IOp::Add, x, k));
                        }
                    }
                }
                if !same_inst(&old, &new) {
                    changed = true;
                    f.blocks[bi].insts[ii] = new;
                }
            }
            // terminator
            let t = f.blocks[bi].term.clone();
            let nt = match t {
                Term::Branch(c, x, y) => {
                    let c = a(c);
                    match kn[c.0 as usize] {
                        Some(K::I(v)) => Term::Jump(if v != 0 { x } else { y }),
                        _ => Term::Branch(c, x, y),
                    }
                }
                Term::Ret(v) => {
                    let mut n = Vec::new();
                    for x in v {
                        n.push(a(x));
                    }
                    Term::Ret(n)
                }
                other => other,
            };
            f.blocks[bi].term = nt;
        }
        pass_time(0, tf);
        let t = pass_clock();
        changed |= local_copy_prop(f);
        pass_time(2, t);
        let t = pass_clock();
        if cse {
            changed |= local_cse(f);
        }
        pass_time(1, t);
        let t = pass_clock();
        changed |= dce(f);
        pass_time(3, t);
        let t = pass_clock();
        thread_jumps(f);
        changed |= merge_blocks(f);
        pass_time(4, t);
        if !changed {
            break;
        }
    }
}

fn same_inst(a: &Inst, b: &Inst) -> bool {
    a == b
}

fn rename(i: &Inst, a: &dyn Fn(VReg) -> VReg) -> Inst {
    // operands only: definitions keep their vreg
    match i {
        Inst::Mov(d, s) => Inst::Mov(*d, a(*s)),
        Inst::IBin(o, t, d, x, y) => Inst::IBin(*o, *t, *d, a(*x), a(*y)),
        Inst::IBinI(o, t, d, x, k) => Inst::IBinI(*o, *t, *d, a(*x), *k),
        Inst::INeg(t, d, x) => Inst::INeg(*t, *d, a(*x)),
        Inst::INot(t, d, x) => Inst::INot(*t, *d, a(*x)),
        Inst::ICmp(c, s, d, x, y) => Inst::ICmp(*c, *s, *d, a(*x), a(*y)),
        Inst::ICmpI(c, s, d, x, k) => Inst::ICmpI(*c, *s, *d, a(*x), *k),
        Inst::FBin(o, w, d, x, y) => Inst::FBin(*o, *w, *d, a(*x), a(*y)),
        Inst::FUnary(o, w, d, x) => Inst::FUnary(*o, *w, *d, a(*x)),
        Inst::FCmp(c, w, d, x, y) => Inst::FCmp(*c, *w, *d, a(*x), a(*y)),
        Inst::Conv(c, d, x) => Inst::Conv(*c, *d, a(*x)),
        Inst::Load(m, d, b, o) => Inst::Load(*m, *d, a(*b), *o),
        Inst::Store(m, b, o, s) => Inst::Store(*m, a(*b), *o, a(*s)),
        Inst::Call(c, args, r) => {
            let c2 = match c {
                Callee::Indirect(v) => Callee::Indirect(a(*v)),
                Callee::CIndirect(v, sig) => Callee::CIndirect(a(*v), sig.clone()),
                x => x.clone(),
            };
            let mut n = Vec::new();
            for v in args {
                n.push(a(*v));
            }
            Inst::Call(c2, n, r.clone())
        }
        Inst::Copy(x, y, n) => Inst::Copy(a(*x), a(*y), *n),
        Inst::AtomicLoad(k, o, d, x) => Inst::AtomicLoad(*k, *o, *d, a(*x)),
        Inst::AtomicStore(k, o, x, v) => Inst::AtomicStore(*k, *o, a(*x), a(*v)),
        Inst::AtomicRmw(op, k, o, d, x, v) => Inst::AtomicRmw(*op, *k, *o, *d, a(*x), a(*v)),
        Inst::AtomicCas(k, o1, o2, d, x, e, n) => Inst::AtomicCas(*k, *o1, *o2, *d, a(*x), a(*e), a(*n)),
        other => other.clone(),
    }
}

/// Removes pure instructions whose results are unused (and `Mov(x, x)`), with a
/// worklist: removing an instruction decrements its operands' use counts.
fn dce(f: &mut Func) -> bool {
    let reach = reachable(f);
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let nv = f.vregs.len();
    let mut cnt = vec![0u32; nv];
    // defining instruction of single-def vregs, for the worklist
    let mut def_at: Vec<(u32, u32)> = vec![(u32::MAX, 0); nv];
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        for (k, i) in b.insts.iter().enumerate() {
            uses_defs(i, &mut uses, &mut defs);
            for u in &uses {
                cnt[u.0 as usize] += 1;
            }
            for d in &defs {
                let e = &mut def_at[d.0 as usize];
                *e = if e.0 == u32::MAX { (bi as u32, k as u32) } else { (u32::MAX - 1, 0) };
            }
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            cnt[u.0 as usize] += 1;
        }
    }
    let mut dead = vec![false; 0];
    let mut work: Vec<(u32, u32)> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        for (k, i) in b.insts.iter().enumerate() {
            work.push((bi as u32, k as u32));
            let _ = i;
        }
    }
    let mut removed_set: Vec<Vec<bool>> = Vec::with_capacity(f.blocks.len());
    for b in &f.blocks {
        removed_set.push(vec![false; b.insts.len()]);
    }
    let _ = &mut dead;
    let mut any = false;
    while let Some((bi, k)) = work.pop() {
        if removed_set[bi as usize][k as usize] {
            continue;
        }
        let i = &f.blocks[bi as usize].insts[k as usize];
        let is_dead = match i {
            Inst::Mov(d, s) if d == s => true,
            i if is_pure(i) => {
                uses_defs(i, &mut uses, &mut defs);
                let mut all_dead = !defs.is_empty();
                for d in &defs {
                    if cnt[d.0 as usize] != 0 {
                        all_dead = false;
                    }
                }
                all_dead
            }
            _ => false,
        };
        if !is_dead {
            continue;
        }
        removed_set[bi as usize][k as usize] = true;
        any = true;
        uses_defs(i, &mut uses, &mut defs);
        for u in uses.iter() {
            let c = &mut cnt[u.0 as usize];
            *c -= 1;
            if *c == 0 {
                let (db, dk) = def_at[u.0 as usize];
                if db < u32::MAX - 1 {
                    work.push((db, dk));
                }
            }
        }
    }
    if any {
        for (bi, b) in f.blocks.iter_mut().enumerate() {
            let rs = &removed_set[bi];
            let mut k = 0;
            let mut j = 0;
            b.insts.retain(|_| {
                let keep = !rs[k];
                k += 1;
                keep
            });
            b.pos.retain(|_| {
                let keep = !rs[j];
                j += 1;
                keep
            });
        }
    }
    any
}

/// Redirects jumps through empty `Jump` blocks.
fn thread_jumps(f: &mut Func) {
    let n = f.blocks.len();
    let mut target: Vec<u32> = (0..n as u32).collect();
    for b in 0..n {
        let mut t = b as u32;
        let mut guard = 0;
        while guard < 16 {
            let blk = &f.blocks[t as usize];
            match blk.term {
                Term::Jump(x) if blk.insts.is_empty() && x != t => t = x,
                _ => break,
            }
            guard += 1;
        }
        target[b] = t;
    }
    for b in &mut f.blocks {
        b.term = match b.term.clone() {
            Term::Jump(x) => Term::Jump(target[x as usize]),
            Term::Branch(c, x, y) => {
                let (x, y) = (target[x as usize], target[y as usize]);
                if x == y {
                    Term::Jump(x)
                } else {
                    Term::Branch(c, x, y)
                }
            }
            t => t,
        };
    }
}

/// Reorders reachable blocks so a block's jump/false-branch target follows it, drops
/// unreachable blocks and puts panic paths (blocks ending in `Unreachable`) last.
pub fn relayout(f: &mut Func) {
    let n = f.blocks.len();
    let reach = reachable(f);
    let mut placed = vec![false; n];
    let mut order: Vec<u32> = Vec::new();
    let mut cold: Vec<u32> = Vec::new();
    let mut next = 0u32;
    loop {
        let mut b = next;
        while !placed[b as usize] && reach[b as usize] {
            placed[b as usize] = true;
            if matches!(f.blocks[b as usize].term, Term::Unreachable) && b != 0 {
                cold.push(b);
            } else {
                order.push(b);
            }
            b = match f.blocks[b as usize].term {
                Term::Jump(t) => t,
                Term::Branch(_, t, e) => {
                    // fall through to the true successor (then-block, loop body) unless it is
                    // placed already or a panic path
                    let t_cold = matches!(f.blocks[t as usize].term, Term::Unreachable);
                    if !placed[t as usize] && !t_cold {
                        t
                    } else {
                        e
                    }
                }
                _ => break,
            };
        }
        let mut found = false;
        for k in 0..n {
            if !placed[k] && reach[k] {
                next = k as u32;
                found = true;
                break;
            }
        }
        if !found {
            break;
        }
    }
    order.extend(cold);
    let mut map = vec![u32::MAX; n];
    for (i, b) in order.iter().enumerate() {
        map[*b as usize] = i as u32;
    }
    let old = std::mem::take(&mut f.blocks);
    let mut slots: Vec<Option<Block>> = Vec::with_capacity(n);
    for b in old {
        slots.push(Some(b));
    }
    for b in &order {
        let mut blk = slots[*b as usize].take().unwrap();
        blk.term = match blk.term {
            Term::Jump(t) => Term::Jump(map[t as usize]),
            Term::Branch(c, t, e) => Term::Branch(c, map[t as usize], map[e as usize]),
            t => t,
        };
        f.blocks.push(blk);
    }
}

pub fn inst_count(f: &Func) -> usize {
    let mut n = 0;
    for b in &f.blocks {
        n += b.insts.len() + 1;
    }
    n
}

/// Appends a block's single-predecessor jump target to it (straight-line code after
/// inlining and branch folding), so compares end up next to their branches.
fn merge_blocks(f: &mut Func) -> bool {
    let n = f.blocks.len();
    let reach = reachable(f);
    let mut preds = vec![0u32; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        if !reach[bi] {
            continue;
        }
        match b.term {
            Term::Jump(t) => preds[t as usize] += 1,
            Term::Branch(_, t, e) => {
                preds[t as usize] += 1;
                preds[e as usize] += 1;
            }
            _ => {}
        }
    }
    preds[0] += 1; // entry
    let mut changed = false;
    for bi in 0..n {
        if !reach[bi] {
            continue;
        }
        loop {
            let t = match f.blocks[bi].term {
                Term::Jump(t) if t as usize != bi && preds[t as usize] == 1 => t as usize,
                _ => break,
            };
            let tb = std::mem::replace(&mut f.blocks[t], Block { insts: Vec::new(), pos: Vec::new(), term: Term::Unreachable, term_pos: 0 });
            let b = &mut f.blocks[bi];
            b.insts.extend(tb.insts);
            b.pos.extend(tb.pos);
            b.term = tb.term;
            b.term_pos = tb.term_pos;
            preds[t] = 0;
            changed = true;
        }
    }
    changed
}

/// Moves single-use constant definitions next to their use in the same block, so
/// constants do not occupy registers across long expressions.
pub fn sink_consts(f: &mut Func) {
    let nv = f.vregs.len();
    let mut ndef = vec![0u32; nv];
    let mut nuse = vec![0u32; nv];
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            uses_defs(i, &mut uses, &mut defs);
            for u in &uses {
                nuse[u.0 as usize] += 1;
            }
            for d in &defs {
                ndef[d.0 as usize] += 1;
            }
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            nuse[u.0 as usize] += 1;
        }
    }
    // constants used only on a cold path (a block ending in Unreachable: panics) move there
    let nb = f.blocks.len();
    let mut use_block = vec![u32::MAX; nv]; // u32::MAX-1: several blocks
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            uses_defs(i, &mut uses, &mut defs);
            for u in &uses {
                let ub = &mut use_block[u.0 as usize];
                if *ub == u32::MAX {
                    *ub = bi as u32;
                } else if *ub != bi as u32 {
                    *ub = u32::MAX - 1;
                }
            }
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            let ub = &mut use_block[u.0 as usize];
            if *ub == u32::MAX {
                *ub = bi as u32;
            } else if *ub != bi as u32 {
                *ub = u32::MAX - 1;
            }
        }
    }
    for bi in 0..nb {
        let mut k = 0;
        while k < f.blocks[bi].insts.len() {
            let d = match &f.blocks[bi].insts[k] {
                Inst::Fconst(d, _, _) | Inst::Iconst(d, _) | Inst::Addr(d, _) if ndef[d.0 as usize] == 1 => *d,
                _ => {
                    k += 1;
                    continue;
                }
            };
            let ub = use_block[d.0 as usize];
            if ub < nb as u32 && ub != bi as u32 && matches!(f.blocks[ub as usize].term, Term::Unreachable) {
                let i = f.blocks[bi].insts.remove(k);
                let p = f.blocks[bi].pos.remove(k);
                f.blocks[ub as usize].insts.insert(0, i);
                f.blocks[ub as usize].pos.insert(0, p);
            } else {
                k += 1;
            }
        }
    }
    for b in &mut f.blocks {
        let n = b.insts.len();
        let mut out: Vec<Inst> = Vec::with_capacity(n);
        let mut out_pos: Vec<u64> = Vec::with_capacity(n);
        // constants waiting for their use: (vreg, inst, pos)
        let mut held: Vec<(u32, Inst, u64)> = Vec::new();
        for k in 0..n {
            let i = b.insts[k].clone();
            let p = b.pos[k];
            let movable = match &i {
                Inst::Fconst(d, _, _) | Inst::Iconst(d, _) | Inst::Addr(d, _) => ndef[d.0 as usize] == 1 && nuse[d.0 as usize] == 1,
                _ => false,
            };
            if movable {
                let d = match &i {
                    Inst::Fconst(d, _, _) | Inst::Iconst(d, _) | Inst::Addr(d, _) => d.0,
                    _ => 0,
                };
                held.push((d, i, p));
                continue;
            }
            uses_defs(&i, &mut uses, &mut defs);
            for u in &uses {
                let mut j = 0;
                while j < held.len() {
                    if held[j].0 == u.0 {
                        let (_, hi, hp) = held.remove(j);
                        out.push(hi);
                        out_pos.push(hp);
                    } else {
                        j += 1;
                    }
                }
            }
            out.push(i);
            out_pos.push(p);
        }
        // the rest are used by the terminator or in other blocks
        for (_, hi, hp) in held {
            out.push(hi);
            out_pos.push(hp);
        }
        b.insts = out;
        b.pos = out_pos;
    }
}

/// Within a block, uses of `a` after `Mov(a, b)` read `b` directly until either is
/// redefined (handles copies of multi-definition vregs that global propagation skips).
fn local_copy_prop(f: &mut Func) -> bool {
    let mut changed = false;
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut map: Vec<(VReg, VReg)> = Vec::new();
    for b in &mut f.blocks {
        map.clear();
        for k in 0..b.insts.len() {
            if !map.is_empty() {
                let look = |v: VReg| -> VReg {
                    for (a, s) in map.iter() {
                        if *a == v {
                            return *s;
                        }
                    }
                    v
                };
                let n = rename(&b.insts[k], &look);
                if n != b.insts[k] {
                    b.insts[k] = n;
                    changed = true;
                }
            }
            uses_defs(&b.insts[k], &mut uses, &mut defs);
            for d in &defs {
                map.retain(|(a, s)| a != d && s != d);
            }
            if let Inst::Mov(d, s) = b.insts[k] {
                if d != s {
                    map.push((d, s));
                }
            }
        }
        if !map.is_empty() {
            let look = |v: VReg| -> VReg {
                for (a, s) in map.iter() {
                    if *a == v {
                        return *s;
                    }
                }
                v
            };
            let t = match &b.term {
                Term::Branch(c, x, y) => Term::Branch(look(*c), *x, *y),
                Term::Ret(v) => {
                    let mut n = Vec::new();
                    for x in v {
                        n.push(look(*x));
                    }
                    Term::Ret(n)
                }
                t => t.clone(),
            };
            b.term = t;
        }
    }
    changed
}

/// Immediate dominators (Cooper/Harvey/Kennedy) over reachable blocks; entry's idom is itself.
pub fn idoms(f: &Func) -> Vec<u32> {
    let n = f.blocks.len();
    // reverse postorder
    let mut rpo: Vec<u32> = Vec::new();
    let mut seen = vec![false; n];
    let mut stack: Vec<(u32, u8)> = vec![(0, 0)];
    while let Some((b, st)) = stack.pop() {
        if st == 0 {
            if seen[b as usize] {
                continue;
            }
            seen[b as usize] = true;
            stack.push((b, 1));
            match f.blocks[b as usize].term {
                Term::Jump(t) => stack.push((t, 0)),
                Term::Branch(_, t, e) => {
                    stack.push((e, 0));
                    stack.push((t, 0));
                }
                _ => {}
            }
        } else {
            rpo.push(b);
        }
    }
    rpo.reverse();
    let mut order = vec![u32::MAX; n];
    for (i, b) in rpo.iter().enumerate() {
        order[*b as usize] = i as u32;
    }
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); n];
    for &b in &rpo {
        match f.blocks[b as usize].term {
            Term::Jump(t) => preds[t as usize].push(b),
            Term::Branch(_, t, e) => {
                preds[t as usize].push(b);
                if e != t {
                    preds[e as usize].push(b);
                }
            }
            _ => {}
        }
    }
    let mut idom = vec![u32::MAX; n];
    idom[0] = 0;
    let mut changed = true;
    while changed {
        changed = false;
        for &b in rpo.iter().skip(1) {
            let mut new = u32::MAX;
            for &p in &preds[b as usize] {
                if idom[p as usize] == u32::MAX {
                    continue;
                }
                if new == u32::MAX {
                    new = p;
                } else {
                    let (mut x, mut y) = (p, new);
                    while x != y {
                        while order[x as usize] > order[y as usize] {
                            x = idom[x as usize];
                        }
                        while order[y as usize] > order[x as usize] {
                            y = idom[y as usize];
                        }
                    }
                    new = x;
                }
            }
            if new != u32::MAX && idom[b as usize] != new {
                idom[b as usize] = new;
                changed = true;
            }
        }
    }
    idom
}

#[derive(Clone, Default)]
struct CseState {
    /// (expression key, dest, operands, is load)
    avail: Vec<([u64; 3], VReg, [VReg; 2], bool)>,
    /// branch facts: vreg has this value
    facts: Vec<(VReg, i64)>,
    /// unsigned upper bounds: vreg <u bound
    ubound: Vec<(VReg, i64)>,
    /// unsigned lower bounds: vreg >u bound
    lbound: Vec<(VReg, i64)>,
    copies: Vec<(VReg, VReg)>,
}

/// CSE over the dominator tree. A block entered through its only predecessor keeps all
/// state of that predecessor plus the branch fact of the edge (`c` true/false, and for an
/// unsigned `x < k` compare the bound `x <u k`). A block reached from its immediate
/// dominator through a join keeps only state about single-definition vregs (their values
/// cannot change on the way). Repeated compares fold, so bounds checks implied by a loop
/// condition (also through copies of the loop variable) disappear.
fn local_cse(f: &mut Func) -> bool {
    let n = f.blocks.len();
    let nv = f.vregs.len();
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            uses_defs(i, &mut uses, &mut defs);
            for d in &defs {
                ndef[d.0 as usize] += 1;
            }
        }
    }
    let idom = idoms(f);
    let mut npred = vec![0u32; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        if idom[bi] == u32::MAX {
            continue;
        }
        match b.term {
            Term::Jump(t) => npred[t as usize] += 1,
            Term::Branch(_, t, e) => {
                npred[t as usize] += 1;
                if e != t {
                    npred[e as usize] += 1;
                }
            }
            _ => {}
        }
    }
    let mut children: Vec<Vec<u32>> = vec![Vec::new(); n];
    for b in 1..n {
        if idom[b] != u32::MAX && idom[b] as usize != b {
            children[idom[b] as usize].push(b as u32);
        }
    }
    let single = |v: VReg| ndef[v.0 as usize] == 1;
    // pointer roots of single-def address vregs: 1+slot for stack slots, 1<<40|vreg for
    // noalias params, 0 unknown
    let mut root = vec![0u64; nv];
    for p in &f.noalias {
        if ndef[p.0 as usize] == 1 {
            root[p.0 as usize] = (1u64 << 40) | p.0 as u64;
        }
    }
    // defs in order of appearance in reverse postorder approximated by block order
    for _ in 0..2 {
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::SlotAddr(d, sl) if single(*d) => root[d.0 as usize] = 1 + *sl as u64,
                    Inst::Mov(d, x) | Inst::IBinI(IOp::Add | IOp::Sub, _, d, x, _) if single(*d) => root[d.0 as usize] = root[x.0 as usize],
                    Inst::IBin(IOp::Add, _, d, x, y) if single(*d) => {
                        let (rx, ry) = (root[x.0 as usize], root[y.0 as usize]);
                        root[d.0 as usize] = if rx != 0 && ry == 0 { rx } else if ry != 0 && rx == 0 { ry } else { 0 };
                    }
                    _ => {}
                }
            }
        }
    }
    let may_alias = |a: VReg, b: VReg| -> bool {
        if a == b {
            return true;
        }
        let (ra, rb) = (root[a.0 as usize], root[b.0 as usize]);
        !(ra != 0 && rb != 0 && ra != rb)
    };
    // definition blocks per vreg (for multi-def vregs crossing joins)
    let mut def_blocks: Vec<Vec<u32>> = vec![Vec::new(); nv];
    let mut def_sites: Vec<Vec<(u32, u32)>> = vec![Vec::new(); nv];
    for (bi, b) in f.blocks.iter().enumerate() {
        for (k, i) in b.insts.iter().enumerate() {
            uses_defs(i, &mut uses, &mut defs);
            for d in &defs {
                if ndef[d.0 as usize] > 1 {
                    if !def_blocks[d.0 as usize].contains(&(bi as u32)) {
                        def_blocks[d.0 as usize].push(bi as u32);
                    }
                    def_sites[d.0 as usize].push((bi as u32, k as u32));
                }
            }
        }
    }
    let dominates = |a: u32, b: u32| -> bool {
        let mut x = b;
        let mut guard = 0;
        loop {
            if x == a {
                return true;
            }
            let p = idom[x as usize];
            if p == u32::MAX || p == x || guard > 10000 {
                return false;
            }
            x = p;
            guard += 1;
        }
    };
    // loop headers: blocks with a predecessor they dominate
    let mut is_header = vec![false; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        if idom[bi] == u32::MAX {
            continue;
        }
        let mut succ = Vec::new();
        match b.term {
            Term::Jump(t) => succ.push(t),
            Term::Branch(_, t, e) => {
                succ.push(t);
                succ.push(e);
            }
            _ => {}
        }
        for t in succ {
            if dominates(t, bi as u32) {
                is_header[t as usize] = true;
            }
        }
    }
    // Loop header `h` entered from `d`: is `v` only ever decreased inside the loop (each
    // in-loop definition is `v = w` with `w = v - c`, c > 0, guarded by `v > c-1`)? Then an
    // upper bound known when entering the loop holds at the header on every iteration.
    let blocks_ptr: *const Vec<Block> = &f.blocks;
    let non_increasing = |v: VReg, d: u32, h: u32| -> bool {
        let blocks = unsafe { &*blocks_ptr };
        for &(db, dk) in &def_sites[v.0 as usize] {
            if dominates(db, d) {
                continue;
            }
            if !dominates(h, db) {
                return false;
            }
            let w = match blocks[db as usize].insts[dk as usize] {
                Inst::Mov(_, w) => w,
                _ => return false,
            };
            // find w's definition: `w = v - c` with a guard on v dominating it
            let mut ok = false;
            'search: for (bi2, b2) in blocks.iter().enumerate() {
                for i2 in &b2.insts {
                    if let Inst::IBinI(IOp::Sub, it, wd, x, c) = i2 {
                        if *wd == w && *x == v && !it.signed && *c > 0 {
                            // a block on the dominator chain of bi2 entered by the true edge
                            // of `v > k` (k >= c-1) or `v != 0` (c == 1)
                            let mut xb = bi2 as u32;
                            let mut guard = 0;
                            while xb != h && guard < 1000 {
                                let p = idom[xb as usize];
                                if p == u32::MAX || p == xb {
                                    break;
                                }
                                if npred[xb as usize] == 1 {
                                    if let Term::Branch(cv, tt, _) = blocks[p as usize].term {
                                        if tt == xb {
                                            for i3 in &blocks[p as usize].insts {
                                                match i3 {
                                                    Inst::ICmpI(Cond::Gt, false, cd, cx, k) if *cd == cv && *cx == v && *k >= *c - 1 => ok = true,
                                                    Inst::ICmpI(Cond::Ne, _, cd, cx, 0) if *cd == cv && *cx == v && *c == 1 => ok = true,
                                                    _ => {}
                                                }
                                            }
                                        }
                                    }
                                }
                                xb = p;
                                guard += 1;
                            }
                            break 'search;
                        }
                    }
                }
            }
            if !ok {
                return false;
            }
        }
        true
    };
    let mut changed = false;
    let mut stack: Vec<(u32, CseState)> = vec![(0, CseState::default())];
    while let Some((bi, mut st)) = stack.pop() {
        let bi = bi as usize;
        let b = &mut f.blocks[bi];
        for k in 0..b.insts.len() {
            let mut hit_copy = false;
            if !st.copies.is_empty() {
                uses_defs(&b.insts[k], &mut uses, &mut defs);
                for u in &uses {
                    for (d, _) in &st.copies {
                        if d == u {
                            hit_copy = true;
                        }
                    }
                }
            }
            if hit_copy {
                let cp = &st.copies;
                let look = |v: VReg| -> VReg {
                    for (d, s2) in cp.iter() {
                        if *d == v {
                            return *s2;
                        }
                    }
                    v
                };
                let n2 = rename(&b.insts[k], &look);
                if n2 != b.insts[k] {
                    b.insts[k] = n2;
                    changed = true;
                }
            }
            let i = b.insts[k].clone();
            // range fact folds an unsigned `y < k2` when y <u k <= k2 is known
            if let Inst::ICmpI(Cond::Lt, false, d, y, k2) = &i {
                let mut known = false;
                for (v, bound) in &st.ubound {
                    if *v == *y && (*bound as u64) <= (*k2 as u64) {
                        known = true;
                    }
                }
                if known {
                    b.insts[k] = Inst::Iconst(*d, 1);
                    changed = true;
                }
            }
            let i = b.insts[k].clone();
            let cand = match &i {
                Inst::Mov(..) | Inst::SlotAddr(..) => None,
                i if is_mem_effect(i) => None,
                _ => {
                    uses_defs(&i, &mut uses, &mut defs);
                    if defs.len() == 1 && !uses.contains(&defs[0]) {
                        Some(defs[0])
                    } else {
                        None
                    }
                }
            };
            let mut replaced = false;
            let key = match cand {
                Some(_) => cse_key(&i),
                None => None,
            };
            if let (Some(d), Some(key)) = (cand, key) {
                let mut hit = None;
                for (ki, e, _, _) in &st.avail {
                    if *ki == key {
                        hit = Some(*e);
                        break;
                    }
                }
                if let Some(e) = hit {
                    if e != d {
                        let mut fact = None;
                        for (fv, fx) in &st.facts {
                            if *fv == e {
                                fact = Some(*fx);
                            }
                        }
                        b.insts[k] = match fact {
                            Some(x) => Inst::Iconst(d, x),
                            None => Inst::Mov(d, e),
                        };
                        changed = true;
                        replaced = true;
                    }
                }
            }
            match &i {
                Inst::Call(..) | Inst::Copy(..) | Inst::AtomicLoad(..) | Inst::AtomicStore(..) | Inst::AtomicRmw(..) | Inst::AtomicCas(..) | Inst::Fence(..) => st.avail.retain(|x| !x.3),
                Inst::Store(m, base, off, v) => {
                    let (base, off, v, m) = (*base, *off as i64, *v, *m);
                    let sz = mem_size(m) as i64;
                    st.avail.retain(|x| {
                        if !x.3 {
                            return true;
                        }
                        let lb = x.2[0];
                        if !may_alias(lb, base) {
                            return true;
                        }
                        if lb == base {
                            // same base: disjoint byte ranges survive
                            let lo = x.0[2] as i64;
                            let lsz = load_key_size(x.0[0]);
                            return lo + lsz <= off || off + sz <= lo;
                        }
                        false
                    });
                    // forward the stored value to later loads of the same cell
                    if st.avail.len() < 48 {
                        let key = cse_key(&Inst::Load(m, VReg(0), base, off as i32)).unwrap();
                        st.avail.push((key, v, [base, v], true));
                    }
                }
                _ => {}
            }
            uses_defs(&b.insts[k], &mut uses, &mut defs);
            for d in &defs {
                let d = *d;
                st.avail.retain(|(_, e, ops, _)| *e != d && ops[0] != d && ops[1] != d);
                st.facts.retain(|(v, _)| *v != d);
                st.ubound.retain(|(v, _)| *v != d);
                st.lbound.retain(|(v, _)| *v != d);
                st.copies.retain(|(x, y)| *x != d && *y != d);
            }
            if let Inst::IBinI(IOp::Sub, it, d, x, c) = b.insts[k] {
                if !it.signed && c > 0 {
                    let mut lb_ok = false;
                    for (v, lb) in &st.lbound {
                        if *v == x && *lb >= c - 1 {
                            lb_ok = true;
                        }
                    }
                    if lb_ok {
                        let mut ub = None;
                        for (v, bnd) in &st.ubound {
                            if *v == x {
                                ub = Some(*bnd);
                            }
                        }
                        if let Some(bnd) = ub {
                            st.ubound.push((d, bnd));
                        }
                    }
                }
            }
            if let Inst::Mov(d, s2) = b.insts[k] {
                if d != s2 {
                    if st.copies.len() < 32 {
                        st.copies.push((d, s2));
                    }
                    // a copy inherits the source's bound (snapshot of the same value)
                    let mut inherit = Vec::new();
                    for (v, bound) in &st.ubound {
                        if *v == s2 {
                            inherit.push((d, *bound));
                        }
                    }
                    st.ubound.extend(inherit);
                }
            }
            if let (Some(d), false, Some(key)) = (cand, replaced, key) {
                if st.avail.len() < 48 {
                    uses_defs(&i, &mut uses, &mut defs);
                    let o0 = if !uses.is_empty() { uses[0] } else { VReg(u32::MAX) };
                    let o1 = if uses.len() > 1 { uses[1] } else { VReg(u32::MAX) };
                    st.avail.push((key, d, [o0, o1], matches!(i, Inst::Load(..))));
                }
            }
        }
        if !st.copies.is_empty() {
            let cp = st.copies.clone();
            let look = |v: VReg| -> VReg {
                for (d, s2) in cp.iter() {
                    if *d == v {
                        return *s2;
                    }
                }
                v
            };
            b.term = match &b.term {
                Term::Branch(c, x, y) => Term::Branch(look(*c), *x, *y),
                Term::Ret(v) => {
                    let mut n2 = Vec::new();
                    for x in v {
                        n2.push(look(*x));
                    }
                    Term::Ret(n2)
                }
                t => t.clone(),
            };
        }
        if let Term::Branch(c, t, e) = b.term {
            for (fv, fx) in &st.facts {
                if *fv == c {
                    b.term = Term::Jump(if *fx != 0 { t } else { e });
                    changed = true;
                    break;
                }
            }
        }
        // the compare behind a branch condition, for edge bounds
        // compare behind the branch condition (unsigned compares give range facts)
        let mut cmp_of_cond: Option<(Cond, VReg, i64)> = None;
        if let Term::Branch(c, _, _) = b.term {
            // the compare defining `c` in this block
            for k in 0..b.insts.len() {
                if let Inst::ICmpI(cc, false, d, x, kk) = &b.insts[k] {
                    if *d == c {
                        cmp_of_cond = Some((*cc, *x, *kk));
                    }
                }
            }
        }
        let term = b.term.clone();
        // children in the dominator tree; the last one takes the state by move
        let nch = children[bi].len();
        let mut st_opt = Some(st);
        for ci in 0..nch {
            let ch = children[bi][ci];
            let direct = npred[ch as usize] == 1;
            let base = st_opt.as_ref().unwrap();
            let mut cs;
            if direct {
                cs = if ci + 1 == nch { st_opt.take().unwrap() } else { base.clone() };
                if let Term::Branch(c, t, e) = term {
                    let val = if ch == t { 1 } else if ch == e { 0 } else { -1 };
                    if val >= 0 {
                        cs.facts.push((c, val));
                        if let Some((cc, x, k)) = cmp_of_cond {
                            // x <u k on the true edge of `x < k`, the false edge of `x >= k`
                            if (cc == Cond::Lt && val == 1) || (cc == Cond::Ge && val == 0) {
                                cs.ubound.push((x, k));
                            }
                            if (cc == Cond::Le && val == 1) || (cc == Cond::Gt && val == 0) {
                                cs.ubound.push((x, k.wrapping_add(1)));
                            }
                            // x >u k on the true edge of `x > k`, the false edge of `x <= k`
                            if (cc == Cond::Gt && val == 1) || (cc == Cond::Le && val == 0) {
                                cs.lbound.push((x, k));
                            }
                            if cc == Cond::Ne && val == 1 && k == 0 {
                                cs.lbound.push((x, 0));
                            }
                        }
                    }
                }
            } else {
                cs = CseState::default();
                let header = is_header[ch as usize];
                // a value is unchanged on every path from this block to the join when it
                // has one definition, or (non-loop joins) each of its definitions either
                // dominates this block or comes after the join
                let stable = |v: VReg| -> bool {
                    if v.0 == u32::MAX || single(v) {
                        return true;
                    }
                    if header {
                        return false;
                    }
                    let blocks = unsafe { &*blocks_ptr };
                    for &db in &def_blocks[v.0 as usize] {
                        // a latch that only jumps back to this block redefines v before the
                        // next pass through here: the last pass to the join sees no def
                        let latch_to_here = matches!(blocks[db as usize].term, Term::Jump(t) if t == bi as u32);
                        if !(dominates(db, bi as u32) || dominates(ch, db) || latch_to_here) {
                            return false;
                        }
                    }
                    true
                };
                for e in &base.avail {
                    if !e.3 && stable(e.1) && stable(e.2[0]) && stable(e.2[1]) {
                        cs.avail.push(*e);
                    }
                }
                for x in &base.facts {
                    if stable(x.0) {
                        cs.facts.push(*x);
                    }
                }
                for x in &base.ubound {
                    if stable(x.0) || (header && non_increasing(x.0, bi as u32, ch)) {
                        cs.ubound.push(*x);
                    }
                }
                for x in &base.lbound {
                    if stable(x.0) {
                        cs.lbound.push(*x);
                    }
                }
                for x in &base.copies {
                    if stable(x.0) && stable(x.1) {
                        cs.copies.push(*x);
                    }
                }
            }
            stack.push((ch, cs));
        }
    }
    changed
}

/// Compact CSE key of a pure single-result instruction (dest excluded).
/// Byte size of the load encoded in a `cse_key` tag word.
fn load_key_size(k0: u64) -> i64 {
    let mk = k0 >> 8;
    if mk & (1 << 16) != 0 {
        4
    } else if mk & (2 << 16) != 0 {
        8
    } else {
        (mk & 0xff) as i64
    }
}

fn cse_key(i: &Inst) -> Option<[u64; 3]> {
    fn it(t: &IntTy) -> u64 {
        t.bits as u64 | ((t.signed as u64) << 8)
    }
    fn cond(c: &Cond) -> u64 {
        *c as u64
    }
    let v = |r: &VReg| r.0 as u64;
    Some(match i {
        Inst::Iconst(_, x) => [1, *x as u64, 0],
        Inst::Fconst(_, x, w) => [2 | ((*w as u64) << 8), *x, 0],
        Inst::IBin(o, t, _, a, b) => [3 | ((*o as u64) << 8) | (it(t) << 16), v(a), v(b)],
        Inst::IBinI(o, t, _, a, k) => [4 | ((*o as u64) << 8) | (it(t) << 16), v(a), *k as u64],
        Inst::INeg(t, _, a) => [5 | (it(t) << 16), v(a), 0],
        Inst::INot(t, _, a) => [6 | (it(t) << 16), v(a), 0],
        Inst::ICmp(c, s2, _, a, b) => [7 | (cond(c) << 8) | ((*s2 as u64) << 16), v(a), v(b)],
        Inst::ICmpI(c, s2, _, a, k) => [8 | (cond(c) << 8) | ((*s2 as u64) << 16), v(a), *k as u64],
        Inst::FBin(o, w, _, a, b) => [9 | ((*o as u64) << 8) | ((*w as u64) << 16), v(a), v(b)],
        Inst::FUnary(o, w, _, a) => [10 | ((*o as u64) << 8) | ((*w as u64) << 16), v(a), 0],
        Inst::FCmp(c, w, _, a, b) => [11 | (cond(c) << 8) | ((*w as u64) << 16), v(a), v(b)],
        Inst::Conv(c, _, a) => {
            let ck = match c {
                Conv::IntToInt(t) => 1 | (it(t) << 8),
                Conv::IntToF(t, w) => 2 | (it(t) << 8) | ((*w as u64) << 24),
                Conv::FToInt(w, t) => 3 | (it(t) << 8) | ((*w as u64) << 24),
                Conv::F32ToF64 => 4,
                Conv::F64ToF32 => 5,
                Conv::BitsToF(w) => 6 | ((*w as u64) << 8),
                Conv::FToBits(w) => 7 | ((*w as u64) << 8),
            };
            [12 | (ck << 8), v(a), 0]
        }
        Inst::Load(m, _, b, o) => {
            let mk = match m {
                Mem::Int(n, s2) => *n as u64 | ((*s2 as u64) << 8),
                Mem::F32 => 1 << 16,
                Mem::F64 => 2 << 16,
            };
            [13 | (mk << 8), v(b), *o as i64 as u64]
        }
        Inst::Addr(_, x) => [14, *x, 0],
        Inst::FnAddr(_, x) => [15, *x as u64, 0],
        _ => return None,
    })
}


/// Scalar replacement of stack slots: a slot whose address is only used as the base of
/// loads/stores at constant offsets (never stored, passed, copied or computed with) has
/// each (offset, kind) cell turned into a virtual register. Small structs, tuples and
/// enums built in slots then live in registers.
pub fn sroa(f: &mut Func) -> bool {
    let nv = f.vregs.len();
    let ns = f.slots.len();
    if ns == 0 {
        return false;
    }
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    // vreg -> slot it holds the address of (single SlotAddr def)
    let mut addr_of: Vec<u32> = vec![u32::MAX; nv];
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            uses_defs(i, &mut uses, &mut defs);
            for d in &defs {
                ndef[d.0 as usize] += 1;
            }
            if let Inst::SlotAddr(d, sl) = i {
                addr_of[d.0 as usize] = *sl;
            }
        }
    }
    for v in 0..nv {
        if ndef[v] != 1 {
            addr_of[v] = u32::MAX;
        }
    }
    let mut ok = vec![true; ns];
    // cells per slot: (offset, mem)
    let mut cells: Vec<Vec<(i32, Mem)>> = vec![Vec::new(); ns];
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::Load(m, _, base, off) | Inst::Store(m, base, off, _) if addr_of[base.0 as usize] != u32::MAX => {
                    let sl = addr_of[base.0 as usize] as usize;
                    // the stored value must not be the address itself
                    if let Inst::Store(_, _, _, v) = i {
                        if addr_of[v.0 as usize] != u32::MAX {
                            ok[addr_of[v.0 as usize] as usize] = false;
                        }
                    }
                    let mut found = false;
                    for c in &cells[sl] {
                        if c.0 == *off {
                            if c.1 != *m {
                                ok[sl] = false;
                            }
                            found = true;
                        }
                    }
                    if !found {
                        cells[sl].push((*off, *m));
                    }
                }
                Inst::SlotAddr(..) => {}
                _ => {
                    uses_defs(i, &mut uses, &mut defs);
                    for u in &uses {
                        if addr_of[u.0 as usize] != u32::MAX {
                            ok[addr_of[u.0 as usize] as usize] = false;
                        }
                    }
                }
            }
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            if addr_of[u.0 as usize] != u32::MAX {
                ok[addr_of[u.0 as usize] as usize] = false;
            }
        }
    }
    // overlapping cells (different sizes at nearby offsets) disqualify the slot
    for sl in 0..ns {
        let c = &cells[sl];
        for a in 0..c.len() {
            for b in 0..c.len() {
                if a != b {
                    let (oa, na) = (c[a].0, mem_size(c[a].1));
                    let (ob, _) = (c[b].0, mem_size(c[b].1));
                    if ob > oa && ob < oa + na {
                        ok[sl] = false;
                    }
                }
            }
        }
    }
    let mut any = false;
    for sl in 0..ns {
        if ok[sl] && !cells[sl].is_empty() {
            any = true;
        }
    }
    if !any {
        return false;
    }
    // one vreg per cell
    let mut cell_reg: Vec<Vec<(i32, VReg)>> = vec![Vec::new(); ns];
    for sl in 0..ns {
        if !ok[sl] {
            continue;
        }
        for k in 0..cells[sl].len() {
            let (off, m) = cells[sl][k];
            let c = match m {
                Mem::F32 => Cls::F32,
                Mem::F64 => Cls::F64,
                Mem::Int(..) => Cls::I,
            };
            let r = f.vreg(c);
            cell_reg[sl].push((off, r));
        }
    }
    let find = |cr: &Vec<Vec<(i32, VReg)>>, sl: usize, off: i32| -> VReg {
        for (o, r) in &cr[sl] {
            if *o == off {
                return *r;
            }
        }
        VReg(0)
    };
    for b in &mut f.blocks {
        for i in b.insts.iter_mut() {
            let n = match i {
                Inst::Load(m, d, base, off) if addr_of[base.0 as usize] != u32::MAX && ok[addr_of[base.0 as usize] as usize] => {
                    let r = find(&cell_reg, addr_of[base.0 as usize] as usize, *off);
                    match m {
                        // narrow integer cells: the stored value was extended by its type;
                        // a load of the same kind reads it back unchanged
                        _ => Some(Inst::Mov(*d, r)),
                    }
                }
                Inst::Store(m, base, off, v) if addr_of[base.0 as usize] != u32::MAX && ok[addr_of[base.0 as usize] as usize] => {
                    let r = find(&cell_reg, addr_of[base.0 as usize] as usize, *off);
                    match m {
                        Mem::Int(n, sg) if *n < 8 => Some(Inst::Conv(Conv::IntToInt(IntTy { bits: *n * 8, signed: *sg }), r, *v)),
                        _ => Some(Inst::Mov(r, *v)),
                    }
                }
                _ => None,
            };
            if let Some(n) = n {
                *i = n;
            }
        }
    }
    true
}

fn mem_size(m: Mem) -> i32 {
    match m {
        Mem::Int(n, _) => n as i32,
        Mem::F32 => 4,
        Mem::F64 => 8,
    }
}

/// Loop-invariant code motion: pure, non-trapping, single-definition instructions inside a
/// natural loop whose operands come from outside it (or from other hoisted instructions)
/// move to the end of the loop's preheader (its single outside predecessor ending in a
/// jump to the header). Loads stay (stores/calls in the loop), integer div/rem stay (trap).
pub fn licm(f: &mut Func) -> bool {
    let n = f.blocks.len();
    let idom = idoms(f);
    let dominates = |a: u32, b: u32| -> bool {
        let mut x = b;
        let mut guard = 0;
        loop {
            if x == a {
                return true;
            }
            let p = idom[x as usize];
            if p == u32::MAX || p == x || guard > 10000 {
                return false;
            }
            x = p;
            guard += 1;
        }
    };
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (bi, b) in f.blocks.iter().enumerate() {
        if idom[bi] == u32::MAX {
            continue;
        }
        match b.term {
            Term::Jump(t) => preds[t as usize].push(bi as u32),
            Term::Branch(_, t, e) => {
                preds[t as usize].push(bi as u32);
                if e != t {
                    preds[e as usize].push(bi as u32);
                }
            }
            _ => {}
        }
    }
    // natural loops per header
    let mut loops: Vec<(u32, Vec<bool>)> = Vec::new();
    for h in 0..n {
        let mut latches = Vec::new();
        for &p in &preds[h] {
            if dominates(h as u32, p) {
                latches.push(p);
            }
        }
        if latches.is_empty() {
            continue;
        }
        let mut body = vec![false; n];
        body[h] = true;
        let mut stack = latches.clone();
        while let Some(x) = stack.pop() {
            if body[x as usize] {
                continue;
            }
            body[x as usize] = true;
            for &p in &preds[x as usize] {
                stack.push(p);
            }
        }
        loops.push((h as u32, body));
    }
    // inner loops first
    loops.sort_by_key(|l| l.1.iter().filter(|x| **x).count());
    let nv = f.vregs.len();
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    let mut def_block = vec![u32::MAX; nv];
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            uses_defs(i, &mut uses, &mut defs);
            for d in &defs {
                ndef[d.0 as usize] += 1;
                def_block[d.0 as usize] = bi as u32;
            }
        }
    }
    let mut changed = false;
    for (h, body) in &loops {
        let outside: Vec<u32> = preds[*h as usize].iter().copied().filter(|p| !body[*p as usize]).collect();
        if outside.len() != 1 {
            continue;
        }
        let pre = outside[0];
        if !matches!(f.blocks[pre as usize].term, Term::Jump(t) if t == *h) {
            continue;
        }
        let mut hoisted: Vec<(Inst, u64)> = Vec::new();
        let mut progress = true;
        while progress {
            progress = false;
            for bi in 0..n {
                if !body[bi] {
                    continue;
                }
                let mut k = 0;
                while k < f.blocks[bi].insts.len() {
                    let i = &f.blocks[bi].insts[k];
                    let pure = match i {
                        Inst::IBin(IOp::Div | IOp::Rem, ..) | Inst::IBinI(IOp::Div | IOp::Rem, ..) => false,
                        Inst::Load(..) | Inst::Mov(..) => false,
                        i if is_mem_effect(i) => false,
                        _ => true,
                    };
                    if !pure {
                        k += 1;
                        continue;
                    }
                    uses_defs(i, &mut uses, &mut defs);
                    let mut inv = defs.len() == 1 && ndef[defs[0].0 as usize] == 1;
                    for u in &uses {
                        let db = def_block[u.0 as usize];
                        let in_loop = db != u32::MAX && body[db as usize];
                        if in_loop || (ndef[u.0 as usize] > 1 && db != u32::MAX && body[db as usize]) {
                            inv = false;
                        }
                        // multi-def operands are invariant only if no definition is in the loop
                        if ndef[u.0 as usize] > 1 {
                            for b2 in 0..n {
                                if body[b2] {
                                    for i2 in &f.blocks[b2].insts {
                                        let mut u3 = Vec::new();
                                        let mut d3 = Vec::new();
                                        uses_defs(i2, &mut u3, &mut d3);
                                        if d3.contains(u) {
                                            inv = false;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // bare constants are not worth a register across the loop on their own
                    let is_const = matches!(i, Inst::Iconst(..) | Inst::Fconst(..) | Inst::Addr(..) | Inst::FnAddr(..) | Inst::SlotAddr(..));
                    if inv && is_const {
                        // hoisted only when some invariant user needs it (next round)
                        let d = defs[0];
                        let mut needed = false;
                        for b2 in 0..n {
                            if !body[b2] {
                                continue;
                            }
                            for i2 in &f.blocks[b2].insts {
                                let mut u3 = Vec::new();
                                let mut d3 = Vec::new();
                                uses_defs(i2, &mut u3, &mut d3);
                                if u3.contains(&d) && !matches!(i2, Inst::Load(..) | Inst::Store(..) | Inst::Call(..) | Inst::Copy(..) | Inst::Mov(..)) {
                                    let mut others_inv = true;
                                    for u in &u3 {
                                        if *u != d {
                                            let db = def_block[u.0 as usize];
                                            if db != u32::MAX && body[db as usize] {
                                                others_inv = false;
                                            }
                                        }
                                    }
                                    if others_inv && d3.len() == 1 && ndef[d3[0].0 as usize] == 1 {
                                        needed = true;
                                    }
                                }
                            }
                        }
                        inv = needed;
                    }
                    if inv {
                        let i = f.blocks[bi].insts.remove(k);
                        let p = f.blocks[bi].pos.remove(k);
                        def_block[defs[0].0 as usize] = pre;
                        hoisted.push((i, p));
                        progress = true;
                        changed = true;
                    } else {
                        k += 1;
                    }
                }
            }
        }
        for (i, p) in hoisted {
            f.blocks[pre as usize].insts.push(i);
            f.blocks[pre as usize].pos.push(p);
        }
    }
    changed
}

/// Backend address modes (run on the codegen copy only, after every IR pass): a load or
/// store whose base is `p + (i << k)` (k <= 3) or `p + i`, computed in the same block for
/// this access only, becomes LoadX/StoreX and the address arithmetic disappears.
pub fn addr_modes(f: &mut Func) {
    let nv = f.vregs.len();
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut nuse = vec![0u32; nv];
    let mut ndef = vec![0u32; nv];
    for p in &f.params {
        ndef[p.0 as usize] += 1;
    }
    for b in &f.blocks {
        for i in &b.insts {
            uses_defs(i, &mut uses, &mut defs);
            for u in &uses {
                nuse[u.0 as usize] += 1;
            }
            for d in &defs {
                ndef[d.0 as usize] += 1;
            }
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            nuse[u.0 as usize] += 1;
        }
    }
    for b in &mut f.blocks {
        let n = b.insts.len();
        // index of the defining inst in this block for single-def vregs
        let mut def_at: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
        let mut dead = vec![false; n];
        for k in 0..n {
            let (m, base, off) = match &b.insts[k] {
                Inst::Load(m, _, base, off) => (*m, *base, *off),
                Inst::Store(m, base, off, _) => (*m, *base, *off),
                i => {
                    uses_defs(i, &mut uses, &mut defs);
                    for d in &defs {
                        if ndef[d.0 as usize] == 1 {
                            def_at.insert(d.0, k);
                        }
                    }
                    continue;
                }
            };
            let _ = m;
            let ai = match def_at.get(&base.0) {
                Some(&ai) if nuse[base.0 as usize] == 1 && !dead[ai] => ai,
                _ => continue,
            };
            let (p, q) = match b.insts[ai] {
                Inst::IBin(IOp::Add, it, _, p, q) if it.bits == 64 => (p, q),
                _ => continue,
            };
            // which operand is the scaled index
            let mut found: Option<(VReg, VReg, u8, Option<usize>)> = None;
            for (pp, qq) in [(p, q), (q, p)] {
                if let Some(&si) = def_at.get(&qq.0) {
                    if nuse[qq.0 as usize] == 1 && !dead[si] {
                        if let Inst::IBinI(IOp::Shl, it, _, idx, sh) = b.insts[si] {
                            if it.bits == 64 && sh <= 3 {
                                found = Some((pp, idx, sh as u8, Some(si)));
                                break;
                            }
                        }
                    }
                }
            }
            let (pp, idx, sh, si) = match found {
                Some(x) => x,
                None => (p, q, 0, None),
            };
            // `idx = x +- c` computed in this block: index by x, fold c into the offset
            let mut idx = idx;
            let mut extra: i64 = 0;
            if let Some(&ii) = def_at.get(&idx.0) {
                match b.insts[ii] {
                    Inst::IBinI(IOp::Add, it, _, x, c) if it.bits == 64 && c.abs() < (1 << 20) => {
                        let mut ok2 = true;
                        for j in ii + 1..k {
                            uses_defs(&b.insts[j], &mut uses, &mut defs);
                            if defs.contains(&x) {
                                ok2 = false;
                            }
                        }
                        if ok2 {
                            idx = x;
                            extra = c << sh;
                        }
                    }
                    Inst::IBinI(IOp::Sub, it, _, x, c) if it.bits == 64 && c.abs() < (1 << 20) => {
                        let mut ok2 = true;
                        for j in ii + 1..k {
                            uses_defs(&b.insts[j], &mut uses, &mut defs);
                            if defs.contains(&x) {
                                ok2 = false;
                            }
                        }
                        if ok2 {
                            idx = x;
                            extra = -(c << sh);
                        }
                    }
                    _ => {}
                }
            }
            // operands must not be redefined between their use in the arithmetic and here
            let from = match si {
                Some(si) => si.min(ai),
                None => ai,
            };
            let mut clobbered = false;
            for j in from + 1..k {
                uses_defs(&b.insts[j], &mut uses, &mut defs);
                if defs.contains(&pp) || defs.contains(&idx) {
                    clobbered = true;
                }
            }
            if clobbered {
                continue;
            }
            b.insts[k] = match b.insts[k] {
                Inst::Load(m, d, _, off2) => Inst::LoadX(m, d, pp, idx, sh, off2 + extra as i32),
                Inst::Store(m, _, off2, v) => Inst::StoreX(m, pp, idx, sh, off2 + extra as i32, v),
                ref other => other.clone(),
            };
            let _ = off;
            dead[ai] = true;
            if let Some(si) = si {
                dead[si] = true;
            }
        }
        if dead.iter().any(|x| *x) {
            let mut k = 0;
            let mut j = 0;
            b.insts.retain(|_| {
                let keep = !dead[k];
                k += 1;
                keep
            });
            b.pos.retain(|_| {
                let keep = !dead[j];
                j += 1;
                keep
            });
        }
    }
}

/// Loop rotation on the codegen copy: a latch ending in `jump H` to a loop header whose
/// block is only a short compare-and-branch gets a copy of that test, so each iteration
/// runs one conditional branch at the bottom instead of a jump plus the header test.
pub fn rotate_loops(f: &mut Func) {
    let n = f.blocks.len();
    let idom = idoms(f);
    let dominates = |a: u32, b: u32| -> bool {
        let mut x = b;
        let mut guard = 0;
        loop {
            if x == a {
                return true;
            }
            let p = idom[x as usize];
            if p == u32::MAX || p == x || guard > 10000 {
                return false;
            }
            x = p;
            guard += 1;
        }
    };
    let mut uses = Vec::new();
    let mut defs = Vec::new();
    let mut nuse = vec![0u32; f.vregs.len()];
    for b in &f.blocks {
        for i in &b.insts {
            uses_defs(i, &mut uses, &mut defs);
            for u in &uses {
                nuse[u.0 as usize] += 1;
            }
        }
        term_uses(&b.term, &mut uses);
        for u in &uses {
            nuse[u.0 as usize] += 1;
        }
    }
    for l in 0..n {
        let h = match f.blocks[l].term {
            Term::Jump(h) if idom[l] != u32::MAX && dominates(h, l as u32) => h as usize,
            _ => continue,
        };
        let hb = f.blocks[h].clone();
        if hb.insts.len() > 4 || !matches!(hb.term, Term::Branch(..)) {
            continue;
        }
        // header defs must be used only inside the header (they get fresh copies)
        let mut ok = true;
        let mut local_uses = vec![0u32; f.vregs.len()];
        for i in &hb.insts {
            match i {
                Inst::Iconst(..) | Inst::Fconst(..) | Inst::ICmp(..) | Inst::ICmpI(..) | Inst::FCmp(..) | Inst::IBinI(..) | Inst::IBin(..) | Inst::Mov(..) | Inst::Conv(..) => {}
                _ => ok = false,
            }
            uses_defs(i, &mut uses, &mut defs);
            for u in &uses {
                local_uses[u.0 as usize] += 1;
            }
        }
        term_uses(&hb.term, &mut uses);
        for u in &uses {
            local_uses[u.0 as usize] += 1;
        }
        let mut map: Vec<(VReg, VReg)> = Vec::new();
        for i in &hb.insts {
            uses_defs(i, &mut uses, &mut defs);
            for d in &defs {
                if nuse[d.0 as usize] != local_uses[d.0 as usize] {
                    ok = false;
                }
                if matches!(i, Inst::IBin(IOp::Div | IOp::Rem, ..)) {
                    ok = false;
                }
                let c = f.vregs[d.0 as usize];
                map.push((*d, VReg(u32::MAX)));
                let _ = c;
            }
        }
        if !ok {
            continue;
        }
        for m in map.iter_mut() {
            let c = f.vregs[m.0 .0 as usize];
            m.1 = f.vreg(c);
        }
        let look = |v: VReg| -> VReg {
            for (a, b) in &map {
                if *a == v {
                    return *b;
                }
            }
            v
        };
        for (k, i) in hb.insts.iter().enumerate() {
            let ni = map_inst(i, &look, 0);
            f.blocks[l].insts.push(ni);
            f.blocks[l].pos.push(hb.pos[k]);
        }
        f.blocks[l].term = match hb.term {
            Term::Branch(c, t, e) => Term::Branch(look(c), t, e),
            t => t,
        };
        f.blocks[l].term_pos = hb.term_pos;
    }
}
