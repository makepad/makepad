//! Random differential fuzzer of the code generators (JITSAFE fix 4): random RIR programs --
//! integer and float arithmetic, loads and stores, calls, counted loops and diamonds, with far
//! more values live at once than there are registers -- run through every tier this crate and the
//! back end have (baseline and thorough register allocation, the spill-elision and copy-reuse
//! knobs off and on, the O2 clean-up passes and the O3 loop passes) and compared with a direct
//! RIR evaluator: the result, every memory word, and (MAKEPAD_RUN_OPT=+checkra, on here) the
//! register allocation checker. A mismatch prints the seed and the knob set; `FUZZ_SEED=<n>`
//! reruns one program, `FUZZ_N=<count>` sets how many (the gate's long run on a node).
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, passes, rir::*};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
}

/// words of the memory buffer: [0, INT_WORDS) integers, [INT_WORDS, WORDS) floats
const INT_WORDS: usize = 48;
const WORDS: usize = 64;

extern "C" fn host2(a: u64, b: u64) -> u64 {
    a.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ b.rotate_left(17)
}
extern "C" fn host6(a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> u64 {
    a.wrapping_add(b.wrapping_mul(3)).wrapping_sub(c) ^ d.rotate_left(5) ^ e.wrapping_mul(f | 1)
}

const W64: IntTy = IntTy { bits: 64, signed: false };
const TYS: [IntTy; 8] = [I64, U64, I32, U32, IntTy { bits: 8, signed: false }, IntTy { bits: 8, signed: true }, IntTy { bits: 16, signed: false }, IntTy { bits: 16, signed: true }];
const OPS: [IOp; 9] = [IOp::Add, IOp::Sub, IOp::Mul, IOp::And, IOp::Or, IOp::Xor, IOp::Shl, IOp::Shr, IOp::Rotl];
const CONDS: [Cond; 6] = [Cond::Eq, Cond::Ne, Cond::Lt, Cond::Le, Cond::Gt, Cond::Ge];
const FOPS: [FOp; 5] = [FOp::Add, FOp::Sub, FOp::Mul, FOp::Min, FOp::Max];

/// A program builder: structured control flow (so every program ends), values kept live across it.
struct Gen<'a> {
    f: Func,
    r: &'a mut Rng,
    mem: VReg,
    ints: Vec<VReg>,
    flts: Vec<VReg>,
    cur: u32,
    host2: VReg,
    host6: VReg,
    budget: u32,
    /// loop counters in scope: read, never redefined (every loop ends)
    counters: Vec<VReg>,
}

impl Gen<'_> {
    fn i(&mut self, inst: Inst) {
        let b = &mut self.f.blocks[self.cur as usize];
        b.insts.push(inst);
        b.pos.push(0);
    }
    fn int(&mut self) -> VReg {
        self.ints[self.r.below(self.ints.len() as u64) as usize]
    }
    fn flt(&mut self) -> VReg {
        self.flts[self.r.below(self.flts.len() as u64) as usize]
    }
    /// the result register: a fresh value (pressure) or an old one redefined (non-SSA, colouring)
    fn dst_i(&mut self) -> VReg {
        if self.r.chance(35) {
            let v = self.f.vreg(Cls::I);
            self.ints.push(v);
            v
        } else {
            loop {
                let v = self.int();
                if !self.counters.contains(&v) {
                    return v;
                }
            }
        }
    }
    fn dst_f(&mut self) -> VReg {
        if self.r.chance(30) {
            let v = self.f.vreg(Cls::F64);
            self.flts.push(v);
            v
        } else {
            self.flt()
        }
    }
    /// `v` in `ty`'s canonical (extended) form, as lowering keeps every narrow value
    fn canon(&mut self, v: VReg, ty: IntTy) -> VReg {
        if ty.bits == 64 {
            return v;
        }
        let c = self.f.vreg(Cls::I);
        self.i(Inst::Conv(Conv::IntToInt(ty), c, v));
        c
    }
    /// a condition as lowering makes them: a compare's 0 / 1
    fn cond(&mut self) -> VReg {
        let v = self.int();
        let c = self.f.vreg(Cls::I);
        let k = self.r.below(3) as i64;
        self.i(Inst::ICmpI(Cond::Gt, false, c, v, k));
        c
    }
    /// an operation and its type (rotates too in every width and signedness: passes::rotl)
    fn op_ty(&mut self) -> (IOp, IntTy) {
        let op = OPS[self.r.below(9) as usize];
        (op, TYS[self.r.below(TYS.len() as u64) as usize])
    }
    /// an in-bounds address: mem + 8 * (x & (words - 1)) of a region
    fn addr(&mut self, float: bool) -> (VReg, i32) {
        if self.r.chance(50) {
            let k = if float { INT_WORDS + self.r.below((WORDS - INT_WORDS) as u64) as usize } else { self.r.below(INT_WORDS as u64) as usize };
            return (self.mem, 8 * k as i32);
        }
        let x = self.int();
        let (m, t, a) = (self.f.vreg(Cls::I), self.f.vreg(Cls::I), self.f.vreg(Cls::I));
        let (mask, base) = if float { ((WORDS - INT_WORDS) as i64 - 1, INT_WORDS as i64 * 8) } else { (31, 0) };
        self.i(Inst::IBinI(IOp::And, W64, m, x, mask));
        self.i(Inst::IBinI(IOp::Shl, W64, t, m, 3));
        self.i(Inst::IBin(IOp::Add, W64, a, self.mem, t));
        (a, base as i32)
    }
    fn op(&mut self) {
        match self.r.below(16) {
            0..=3 => {
                let (a, mut b) = (self.int(), self.int());
                let (op, ty) = self.op_ty();
                let a = self.canon(a, ty);
                if !matches!(op, IOp::Shl | IOp::Shr | IOp::Rotl) {
                    b = self.canon(b, ty);
                }
                // (shift counts below the width, as lowering makes them: RIR leaves wider ones open;
                // a rotate takes any count, mod the width)
                if matches!(op, IOp::Shl | IOp::Shr) {
                    let m = self.f.vreg(Cls::I);
                    self.i(Inst::IBinI(IOp::And, W64, m, b, ty.bits as i64 - 1));
                    b = m;
                }
                let d = self.dst_i();
                self.i(Inst::IBin(op, ty, d, a, b));
            }
            4 | 5 => {
                let a = self.int();
                let (op, ty) = self.op_ty();
                let a = self.canon(a, ty);
                // (an immediate in the type's canonical form, as lowering makes them)
                let k = if matches!(op, IOp::Shl | IOp::Shr) { self.r.below(ty.bits as u64) as i64 } else if op == IOp::Rotl { self.r.below(200) as i64 } else { passes::fold_ibin(IOp::Add, ty, self.r.below(200) as i64 - 60, 0).unwrap() };
                let d = self.dst_i();
                self.i(Inst::IBinI(op, ty, d, a, k));
            }
            6 => {
                let (a, b) = (self.int(), self.int());
                let d = self.dst_i();
                let (c, s) = (CONDS[self.r.below(6) as usize], self.r.chance(50));
                self.i(Inst::ICmp(c, s, d, a, b));
            }
            7 => {
                let c = self.cond();
                let (a, b) = (self.int(), self.int());
                let d = self.dst_i();
                self.i(Inst::Sel(d, c, a, b));
            }
            8 => {
                let (a, b) = (self.flt(), self.flt());
                let d = self.dst_f();
                let op = FOPS[self.r.below(5) as usize];
                self.i(Inst::FBin(op, true, d, a, b));
            }
            9 => {
                // float -> int (saturating, NaN -> 0) and int -> float
                if self.r.chance(50) {
                    let a = self.flt();
                    let d = self.dst_i();
                    self.i(Inst::Conv(Conv::FToInt(true, I64), d, a));
                } else {
                    let a = self.int();
                    let t = self.f.vreg(Cls::I);
                    self.i(Inst::IBinI(IOp::And, W64, t, a, 0xffff));
                    let d = self.dst_f();
                    self.i(Inst::Conv(Conv::IntToF(I64, true), d, t));
                }
            }
            10 | 11 => {
                let float = self.r.chance(25);
                let (a, off) = self.addr(float);
                if float {
                    let d = self.dst_f();
                    self.i(Inst::Load(Mem::F64, d, a, off));
                } else {
                    let d = self.dst_i();
                    let m = [Mem::Int(8, false), Mem::Int(4, true), Mem::Int(1, false), Mem::Int(2, true)][self.r.below(4) as usize];
                    self.i(Inst::Load(m, d, a, off));
                }
            }
            12 | 13 => {
                let float = self.r.chance(25);
                let (a, off) = self.addr(float);
                if float {
                    let v = self.flt();
                    self.i(Inst::Store(Mem::F64, a, off, v));
                } else {
                    let v = self.int();
                    let m = [Mem::Int(8, false), Mem::Int(4, false), Mem::Int(1, false)][self.r.below(3) as usize];
                    self.i(Inst::Store(m, a, off, v));
                }
            }
            14 => {
                // a call: every value live across it is saved or in a callee-saved register
                // (operands chosen before the result: a fresh result is not defined yet)
                if self.r.chance(50) {
                    let (a, b) = (self.int(), self.int());
                    let d = self.dst_i();
                    self.i(Inst::Call(Callee::Indirect(self.host2), vec![a, b], vec![d]));
                } else {
                    let args: Vec<VReg> = (0..6).map(|_| self.int()).collect();
                    let d = self.dst_i();
                    self.i(Inst::Call(Callee::Indirect(self.host6), args, vec![d]));
                }
            }
            _ => {
                let a = self.int();
                let d = self.dst_i();
                self.i(Inst::Mov(d, a));
            }
        }
    }
    /// a sequence of statements with nested diamonds and counted loops
    fn seq(&mut self, depth: u32) {
        let n = 2 + self.r.below(10);
        for _ in 0..n {
            if self.budget == 0 {
                return;
            }
            self.budget -= 1;
            match self.r.below(10) {
                0 if depth < 3 => self.diamond(depth),
                1 if depth < 2 => self.lp(depth),
                _ => self.op(),
            }
        }
    }
    fn diamond(&mut self, depth: u32) {
        let c = self.cond();
        let (t, e, j) = (self.f.block(), self.f.block(), self.f.block());
        self.f.blocks[self.cur as usize].term = Term::Branch(c, t, e);
        // values first defined in an arm are that arm's (not defined on the other path)
        let (ni, nf) = (self.ints.len(), self.flts.len());
        self.cur = t;
        self.seq(depth + 1);
        self.ints.truncate(ni);
        self.flts.truncate(nf);
        self.f.blocks[self.cur as usize].term = Term::Jump(j);
        self.cur = e;
        if self.r.chance(60) {
            self.seq(depth + 1);
            self.ints.truncate(ni);
            self.flts.truncate(nf);
        }
        self.f.blocks[self.cur as usize].term = Term::Jump(j);
        self.cur = j;
    }
    /// for i in 0..n { body } (n <= 7, from a value: some loops run zero times)
    fn lp(&mut self, depth: u32) {
        let (i, n, c) = (self.f.vreg(Cls::I), self.f.vreg(Cls::I), self.f.vreg(Cls::I));
        let src = self.int();
        self.i(Inst::IBinI(IOp::And, W64, n, src, 7));
        self.i(Inst::Iconst(i, 0));
        let (h, body, exit) = (self.f.block(), self.f.block(), self.f.block());
        self.f.blocks[self.cur as usize].term = Term::Jump(h);
        self.cur = h;
        self.i(Inst::ICmp(Cond::Lt, false, c, i, n));
        self.f.blocks[h as usize].term = Term::Branch(c, body, exit);
        self.cur = body;
        // the counter is readable in the body
        let (ni, nf) = (self.ints.len(), self.flts.len());
        self.ints.push(i);
        self.counters.push(i);
        self.seq(depth + 1);
        self.counters.pop();
        // (the body may run zero times: what it first defines is not defined after the loop)
        self.ints.truncate(ni);
        self.flts.truncate(nf);
        self.i(Inst::IBinI(IOp::Add, W64, i, i, 1));
        self.f.blocks[self.cur as usize].term = Term::Jump(h);
        self.cur = exit;
    }
}

/// A random program: f(mem, a, b, c) -> u64, every value defined at entry (no undefined reads).
fn program(seed: u64) -> Func {
    let mut r = Rng(seed | 1);
    let mut f = Func::new(format!("fuzz_{:x}", seed), 0);
    let mem = f.vreg(Cls::I);
    let ps: Vec<VReg> = (0..3).map(|_| f.vreg(Cls::I)).collect();
    f.params = std::iter::once(mem).chain(ps.iter().copied()).collect();
    f.rets = vec![Cls::I];
    let entry = f.block();
    let (h2, h6) = (f.vreg(Cls::I), f.vreg(Cls::I));
    let mut g = Gen { f, r: &mut r, mem, ints: ps.clone(), flts: Vec::new(), cur: entry, host2: h2, host6: h6, budget: 0, counters: Vec::new() };
    g.i(Inst::Iconst(h2, host2 as *const () as usize as i64));
    g.i(Inst::Iconst(h6, host6 as *const () as usize as i64));
    // the pressure: 20-60 integer values and 4-16 floats live from the entry on
    let size: u64 = std::env::var("FUZZ_SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(100);
    let ni = (20 + g.r.below(40)).min(size / 2 + 3);
    for k in 0..ni {
        let v = g.f.vreg(Cls::I);
        let p = ps[(k % 3) as usize];
        let kk = g.r.below(1000) as i64 - 300;
        g.i(Inst::IBinI(IOp::Add, W64, v, p, kk));
        g.ints.push(v);
    }
    let nf = 4 + g.r.below(12);
    for _ in 0..nf {
        let v = g.f.vreg(Cls::F64);
        let x = (g.r.below(2000) as f64 - 1000.0) / 8.0;
        g.i(Inst::Fconst(v, x.to_bits(), true));
        g.flts.push(v);
    }
    g.budget = (20 + g.r.below(100)).min(size) as u32;
    g.seq(0);
    // the result: every value folded (floats through the saturating conversion)
    let mut acc = g.f.vreg(Cls::I);
    g.i(Inst::Iconst(acc, 0));
    let (ints, flts) = (g.ints.clone(), g.flts.clone());
    for v in ints {
        let (t, a) = (g.f.vreg(Cls::I), g.f.vreg(Cls::I));
        g.i(Inst::IBinI(IOp::Rotl, W64, t, acc, 7));
        g.i(Inst::IBin(IOp::Xor, W64, a, t, v));
        acc = a;
    }
    for v in flts {
        let (t, a) = (g.f.vreg(Cls::I), g.f.vreg(Cls::I));
        g.i(Inst::Conv(Conv::FToInt(true, I64), t, v));
        g.i(Inst::IBin(IOp::Add, W64, a, acc, t));
        acc = a;
    }
    let cur = g.cur;
    g.f.blocks[cur as usize].term = Term::Ret(vec![acc]);
    g.f
}

/// Every read of a value is defined on every path to it (a must-analysis over the CFG): an
/// undefined read is a generator (or reducer) bug, never something to compare tiers on.
fn undefined_read(f: &Func) -> Option<String> {
    use relax_backend::regalloc::{for_defs, for_uses, term_uses};
    let (nb, nv) = (f.blocks.len(), f.vregs.len());
    let mut reach = vec![false; nb];
    let mut work = vec![0u32];
    while let Some(b) = work.pop() {
        if !std::mem::replace(&mut reach[b as usize], true) {
            work.extend(f.blocks[b as usize].term.succs());
        }
    }
    let preds = relax_backend::regalloc::preds_of(f, &reach);
    // out[b]: defined at b's end (start: everything, shrinking to the fixed point)
    let mut out = vec![vec![true; nv]; nb];
    let entry_in: Vec<bool> = (0..nv).map(|v| f.params.iter().any(|p| p.0 as usize == v)).collect();
    let block_in = |out: &Vec<Vec<bool>>, b: usize| -> Vec<bool> {
        if b == 0 {
            return entry_in.clone();
        }
        let mut d = vec![true; nv];
        for &p in &preds[b] {
            for v in 0..nv {
                d[v] &= out[p as usize][v];
            }
        }
        d
    };
    loop {
        let mut changed = false;
        for b in 0..nb {
            if !reach[b] {
                continue;
            }
            let mut d = block_in(&out, b);
            for i in &f.blocks[b].insts {
                for_defs(i, |v| d[v.0 as usize] = true);
            }
            if d != out[b] {
                out[b] = d;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut tu = Vec::new();
    for b in 0..nb {
        if !reach[b] {
            continue;
        }
        let mut d = block_in(&out, b);
        for (k, i) in f.blocks[b].insts.iter().enumerate() {
            let mut bad = None;
            for_uses(i, |v| {
                if !d[v.0 as usize] && bad.is_none() {
                    bad = Some(v);
                }
            });
            if let Some(v) = bad {
                return Some(format!("b{} +{} {:?} reads v{} before any definition", b, k, i, v.0));
            }
            for_defs(i, |v| d[v.0 as usize] = true);
        }
        term_uses(&f.blocks[b].term, &mut tu);
        if let Some(v) = tu.iter().find(|v| !d[v.0 as usize]) {
            return Some(format!("b{} terminator reads v{} before any definition", b, v.0));
        }
    }
    None
}

// ---- the reference: a direct evaluator of the RIR

/// None: no end (a reduction broke a loop) or a call to no known function.
fn eval(f: &Func, mem: &mut [u64; WORDS], args: [u64; 3]) -> Option<u64> {
    let mut r = vec![0u64; f.vregs.len()];
    r[f.params[0].0 as usize] = mem.as_mut_ptr() as u64;
    for k in 0..3 {
        r[f.params[k + 1].0 as usize] = args[k];
    }
    let base = mem.as_ptr() as u64;
    // (an access within one word: the generator makes aligned ones; a reduction may not)
    let at = |a: u64, off: i32, size: usize| -> Option<(usize, usize)> {
        let o = a.wrapping_add(off as i64 as u64).wrapping_sub(base) as usize;
        (o / 8 < WORDS && o % size == 0).then_some((o / 8, o % 8))
    };
    let size = |m: Mem| match m {
        Mem::Int(n, _) => n as usize,
        _ => 8,
    };
    let mut b = 0usize;
    let mut steps = 0u64;
    loop {
        steps += 1;
        if steps > 1_000_000 {
            return None;
        }
        for i in &f.blocks[b].insts {
            match *i {
                Inst::Iconst(d, k) => r[d.0 as usize] = k as u64,
                Inst::Fconst(d, k, _) => r[d.0 as usize] = k,
                Inst::Mov(d, a) => r[d.0 as usize] = r[a.0 as usize],
                Inst::IBin(op, it, d, a, x) => r[d.0 as usize] = passes::fold_ibin(op, it, r[a.0 as usize] as i64, r[x.0 as usize] as i64).expect("fold") as u64,
                Inst::IBinI(op, it, d, a, k) => r[d.0 as usize] = passes::fold_ibin(op, it, r[a.0 as usize] as i64, k).expect("fold") as u64,
                Inst::ICmp(c, s, d, a, x) => r[d.0 as usize] = passes::cmp_i(c, s, r[a.0 as usize] as i64, r[x.0 as usize] as i64) as u64,
                Inst::ICmpI(c, s, d, a, k) => r[d.0 as usize] = passes::cmp_i(c, s, r[a.0 as usize] as i64, k) as u64,
                Inst::Conv(Conv::IntToInt(it), d, a) => r[d.0 as usize] = passes::fold_ibin(IOp::Add, it, r[a.0 as usize] as i64, 0).unwrap() as u64,
                Inst::Sel(d, c, a, x) => r[d.0 as usize] = if r[c.0 as usize] != 0 { r[a.0 as usize] } else { r[x.0 as usize] },
                Inst::FBin(op, w, d, a, x) => r[d.0 as usize] = passes::fold_fbin(op, w, r[a.0 as usize], r[x.0 as usize]),
                Inst::Conv(Conv::FToInt(w, it), d, a) => r[d.0 as usize] = passes::f_to_int(passes::fval(r[a.0 as usize], w), it) as u64,
                Inst::Conv(Conv::IntToF(it, true), d, a) => {
                    let x = passes::fold_ibin(IOp::Add, it, r[a.0 as usize] as i64, 0).unwrap();
                    r[d.0 as usize] = (if it.signed { x as f64 } else { x as u64 as f64 }).to_bits();
                }
                Inst::Load(m, d, a, off) => {
                    let (w, o) = at(r[a.0 as usize], off, size(m))?;
                    let word = mem[w];
                    r[d.0 as usize] = match m {
                        Mem::F64 => word,
                        Mem::Int(n, s) => {
                            let bits = n as u32 * 8;
                            let v = if bits == 64 { word } else { (word >> (o * 8)) & ((1u64 << bits) - 1) };
                            if s && bits < 64 { (((v << (64 - bits)) as i64) >> (64 - bits)) as u64 } else { v }
                        }
                        Mem::F32 => unreachable!(),
                    };
                }
                Inst::Store(m, a, off, v) => {
                    let (w, o) = at(r[a.0 as usize], off, size(m))?;
                    let x = r[v.0 as usize];
                    match m {
                        Mem::F64 => mem[w] = x,
                        Mem::Int(n, _) => {
                            let bits = n as u32 * 8;
                            if bits == 64 {
                                mem[w] = x;
                            } else {
                                let mask = ((1u64 << bits) - 1) << (o * 8);
                                mem[w] = (mem[w] & !mask) | ((x << (o * 8)) & mask);
                            }
                        }
                        Mem::F32 => unreachable!(),
                    }
                }
                Inst::Call(Callee::Indirect(t), ref args, ref rets) => {
                    let a: Vec<u64> = args.iter().map(|v| r[v.0 as usize]).collect();
                    let tgt = r[t.0 as usize];
                    let v = if tgt == host2 as *const () as usize as u64 && a.len() == 2 {
                        host2(a[0], a[1])
                    } else if tgt == host6 as *const () as usize as u64 && a.len() == 6 {
                        host6(a[0], a[1], a[2], a[3], a[4], a[5])
                    } else {
                        return None;
                    };
                    r[rets[0].0 as usize] = v;
                }
                ref other => panic!("evaluator: {:?}", other),
            }
        }
        match &f.blocks[b].term {
            Term::Jump(t) => b = *t as usize,
            Term::Branch(c, t, e) => b = if r[c.0 as usize] != 0 { *t as usize } else { *e as usize },
            Term::Ret(v) => return Some(r[v[0].0 as usize]),
            other => panic!("evaluator: {:?}", other),
        }
    }
}

// ---- the tiers

#[cfg(target_arch = "x86_64")]
fn native(o: &relax_session::Options, f: &Func, thorough: bool) -> Result<Vec<u8>, String> {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: thorough, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile(o, f, &env)?;
    // (the float constant pool follows the text: the whole code runs; nothing to link here)
    if !c.relocs.is_empty() {
        return Err(format!("{} unlinked relocations", c.relocs.len()));
    }
    Ok(c.code)
}

#[cfg(target_arch = "aarch64")]
fn native(o: &relax_session::Options, f: &Func, thorough: bool) -> Result<Vec<u8>, String> {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: thorough, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::arm64::compile(o, f, &env)?;
    // (the float constant pool follows the text: the whole code runs; nothing to link here)
    if !c.relocs.is_empty() {
        return Err(format!("{} unlinked relocations", c.relocs.len()));
    }
    Ok(c.code)
}

fn options(knobs: &[&str]) -> relax_session::Options {
    let mut o = relax_session::Options::default();
    o.opt = knobs.iter().map(|s| s.to_string()).collect();
    // the allocation checker, fatal: a finding fails the compile
    // (FUZZ_NOCHECKRA=1: without it -- triage: a checker finding whose code still agrees)
    if std::env::var("FUZZ_NOCHECKRA").is_err() {
        o.opt.push("+checkra".to_string());
    }
    o
}

/// The middle-end pipelines applied before code generation.
#[derive(Clone, Copy, Debug)]
enum Pipe {
    Raw,
    O2,
    O3,
}

fn pipe(o: &relax_session::Options, f: &Func, p: Pipe) -> Func {
    let mut g = f.clone();
    match p {
        Pipe::Raw => {}
        Pipe::O2 => {
            passes::simplify(o, &mut g, None, true, true);
            passes::licm(o, &mut g);
            passes::simplify(o, &mut g, None, true, true);
        }
        Pipe::O3 => {
            passes::simplify(o, &mut g, None, true, true);
            relax_codegen::loops::unroll_const_loops(o, &mut g);
            relax_codegen::loops::unroll_loops(o, &mut g);
            relax_codegen::loops::merge_stores(o, &mut g);
            passes::licm(o, &mut g);
            passes::simplify(o, &mut g, None, true, true);
        }
    }
    g
}

const KNOBS: [&[&str]; 6] = [&[], &["-storeskip"], &["-saveskip"], &["-copyreuse"], &["-edgeload", "-defregion"], &["-regionwin", "-slotcolor"]];

fn inputs(seed: u64) -> Vec<([u64; 3], [u64; WORDS])> {
    let mut r = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) | 1);
    (0..3)
        .map(|_| {
            let a = [r.next(), r.below(20), r.next() >> r.below(60)];
            let mut m = [0u64; WORDS];
            for (k, w) in m.iter_mut().enumerate() {
                *w = if k < INT_WORDS { r.next() } else { ((r.below(2000) as f64 - 1000.0) / 4.0).to_bits() };
            }
            (a, m)
        })
        .collect()
}

/// Runs program `seed` through every tier; Err names the first disagreement.
fn check(seed: u64) -> Result<usize, String> {
    let f = program(seed);
    f.verify().map_err(|e| format!("seed {:#x}: generator made invalid RIR: {}", seed, e))?;
    if let Some(e) = undefined_read(&f) {
        return Err(format!("seed {:#x}: generator made an undefined read: {}", seed, e));
    }
    let cases: Vec<_> = inputs(seed).into_iter().map(|(a, m)| {
        let mut mm = m;
        let r = eval(&f, &mut mm, a).expect("generated programs end");
        (a, m, r, mm)
    }).collect();
    let mut runs = 0;
    for p in [Pipe::Raw, Pipe::O2, Pipe::O3] {
        for knobs in KNOBS {
            let o = options(knobs);
            let g = pipe(&o, &f, p);
            g.verify().map_err(|e| format!("seed {:#x} {:?}: pass made invalid RIR: {}", seed, p, e))?;
            for thorough in [false, true] {
                let bytes = native(&o, &g, thorough).map_err(|e| format!("seed {:#x} {:?} {:?} thorough={}: {}", seed, p, knobs, thorough, e))?;
                let code = Code::new(&bytes).ok_or("no executable memory")?;
                let run: extern "C" fn(*mut u64, u64, u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
                for (a, m, want, want_mem) in &cases {
                    let mut mm = *m;
                    let got = run(mm.as_mut_ptr(), a[0], a[1], a[2]);
                    let mem_ok = mm.iter().zip(want_mem.iter()).enumerate().all(|(k, (x, y))| x == y || (k >= INT_WORDS && f64::from_bits(*x).is_nan() && f64::from_bits(*y).is_nan()));
                    if got != *want || !mem_ok {
                        let w = mm.iter().zip(want_mem.iter()).position(|(x, y)| x != y);
                        return Err(format!("seed {:#x} {:?} knobs {:?} thorough={} args {:?}: got {:#x} want {:#x}{}", seed, p, knobs, thorough, a, got, want, w.map_or(String::new(), |k| format!(", memory word {} {:#x} want {:#x}", k, mm[k], want_mem[k]))));
                    }
                    runs += 1;
                }
            }
        }
    }
    Ok(runs)
}

/// Does `f` disagree with the evaluator in tier (p, knobs, thorough) on any input of `seed`?
fn disagrees(f: &Func, seed: u64, p: Pipe, knobs: &[&str], thorough: bool) -> bool {
    if f.verify().is_err() || undefined_read(f).is_some() {
        return false;
    }
    // (a reduction that breaks a loop or a call is no reduction)
    if inputs(seed).into_iter().any(|(a, mut m)| eval(f, &mut m, a).is_none()) {
        return false;
    }
    let o = options(knobs);
    let g = pipe(&o, f, p);
    let Ok(bytes) = native(&o, &g, thorough) else { return true };
    let code = Code::new(&bytes).unwrap();
    let run: extern "C" fn(*mut u64, u64, u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
    inputs(seed).into_iter().any(|(a, m)| {
        // (one buffer for both runs: a result that is the buffer's address agrees)
        let mut mm = m;
        let Some(want) = eval(f, &mut mm, a) else { return false };
        let want_mem = mm;
        mm = m;
        let got = run(mm.as_mut_ptr(), a[0], a[1], a[2]);
        got != want || mm.iter().zip(want_mem.iter()).enumerate().any(|(k, (x, y))| x != y && !(k >= INT_WORDS && f64::from_bits(*x).is_nan() && f64::from_bits(*y).is_nan()))
    })
}

/// Shrinks a failing program: each instruction in turn goes (when every read stays defined), or
/// else becomes constants for what it defines, kept when the program still disagrees in that tier;
/// branches go to one side.
fn reduce(mut f: Func, seed: u64, p: Pipe, knobs: &[&str], thorough: bool) -> Func {
    loop {
        let mut changed = false;
        for b in 0..f.blocks.len() {
            let mut k = f.blocks[b].insts.len();
            while k > 0 {
                k -= 1;
                let mut g = f.clone();
                g.blocks[b].insts.remove(k);
                g.blocks[b].pos.remove(k);
                if disagrees(&g, seed, p, knobs, thorough) {
                    f = g;
                    changed = true;
                }
            }
        }
        // an instruction with one result becomes a copy of one of its operands (chains collapse)
        for b in 0..f.blocks.len() {
            for k in 0..f.blocks[b].insts.len() {
                let mut defs = Vec::new();
                relax_backend::regalloc::for_defs(&f.blocks[b].insts[k], |d| defs.push(d));
                let mut uses = Vec::new();
                relax_backend::regalloc::for_uses(&f.blocks[b].insts[k], |u| uses.push(u));
                if defs.len() != 1 || matches!(f.blocks[b].insts[k], Inst::Mov(..)) {
                    continue;
                }
                let d = defs[0];
                for u in uses {
                    if f.vregs[u.0 as usize] != f.vregs[d.0 as usize] || u == d {
                        continue;
                    }
                    let mut g = f.clone();
                    g.blocks[b].insts[k] = Inst::Mov(d, u);
                    if disagrees(&g, seed, p, knobs, thorough) {
                        f = g;
                        changed = true;
                        break;
                    }
                }
            }
        }
        for b in 0..f.blocks.len() {
            let mut k = 0;
            while k < f.blocks[b].insts.len() {
                let mut defs = Vec::new();
                relax_backend::regalloc::for_defs(&f.blocks[b].insts[k], |d| defs.push(d));
                let mut g = f.clone();
                let repl: Vec<Inst> = defs.iter().map(|d| if g.vregs[d.0 as usize] == Cls::I { Inst::Iconst(*d, 0) } else { Inst::Fconst(*d, 0, true) }).collect();
                if repl.len() == 1 && format!("{:?}", repl[0]) == format!("{:?}", g.blocks[b].insts[k]) {
                    k += 1;
                    continue;
                }
                let n = repl.len();
                g.blocks[b].insts.splice(k..k + 1, repl);
                g.blocks[b].pos.splice(k..k + 1, std::iter::repeat(0).take(n));
                if disagrees(&g, seed, p, knobs, thorough) {
                    f = g;
                    changed = true;
                    k += n;
                } else {
                    k += 1;
                }
            }
        }
        // branches: to one side
        for b in 0..f.blocks.len() {
            if let Term::Branch(_, t, e) = f.blocks[b].term {
                for side in [t, e] {
                    let mut g = f.clone();
                    g.blocks[b].term = Term::Jump(side);
                    if disagrees(&g, seed, p, knobs, thorough) {
                        f = g;
                        changed = true;
                        break;
                    }
                }
            }
        }
        if !changed {
            return f;
        }
    }
}

#[test]
fn random_programs_agree_across_tiers() {
    // FUZZ_REDUCE=<hex seed>: the smallest program that still disagrees in its first failing tier
    if let Some(s) = std::env::var("FUZZ_REDUCE").ok().and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()) {
        let f = program(s);
        for p in [Pipe::Raw, Pipe::O2, Pipe::O3] {
            for knobs in KNOBS {
                for thorough in [false, true] {
                    if disagrees(&f, s, p, knobs, thorough) {
                        let r = reduce(f.clone(), s, p, knobs, thorough);
                        let o = options(knobs);
                        let g = pipe(&o, &r, p);
                        eprintln!("reduced {:#x} {:?} {:?} thorough={}:\n{}", s, p, knobs, thorough, r.dump());
                        eprintln!("after the pipeline:\n{}", g.dump());
                        // the allocation (MAKEPAD_RUN_RA_TRACE) and the code (FUZZ_CODE_OUT=<file>)
                        // of the reduced program, for reading the miscompile
                        std::env::set_var("MAKEPAD_RUN_RA_TRACE", &r.name);
                        let bytes = native(&o, &g, thorough).unwrap();
                        std::env::remove_var("MAKEPAD_RUN_RA_TRACE");
                        if let Ok(path) = std::env::var("FUZZ_CODE_OUT") {
                            std::fs::write(path, &bytes).unwrap();
                        }
                        for (a, m) in inputs(s) {
                            let mut wm = m;
                            let want = eval(&r, &mut wm, a).unwrap_or(0);
                            let code = Code::new(&native(&o, &g, thorough).unwrap()).unwrap();
                            let run: extern "C" fn(*mut u64, u64, u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
                            let mut mm = m;
                            let got = run(mm.as_mut_ptr(), a[0], a[1], a[2]);
                            eprintln!("args {:?}: got {:#x} want {:#x}; memory differs at {:?}", a, got, want, mm.iter().zip(wm.iter()).enumerate().filter(|(_, (x, y))| x != y).map(|(k, (x, y))| (k, *x, *y)).collect::<Vec<_>>());
                        }
                        return;
                    }
                }
            }
        }
        eprintln!("{:#x} agrees everywhere", s);
        return;
    }
    if let Some(s) = std::env::var("FUZZ_SEED").ok().and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()) {
        eprintln!("{:?}", check(s));
        if let Ok(dump) = std::env::var("FUZZ_DUMP") {
            if dump == "1" {
                eprintln!("{}", program(s).dump());
            }
        }
        check(s).unwrap();
        return;
    }
    let n: u64 = std::env::var("FUZZ_N").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
    let first: u64 = std::env::var("FUZZ_FIRST").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let mut fails = Vec::new();
    let mut runs = 0;
    for k in first..first + n {
        let seed = k.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        match check(seed) {
            Ok(r) => runs += r,
            Err(e) => {
                eprintln!("fuzz: {}", e);
                fails.push(seed);
            }
        }
    }
    eprintln!("fuzz: {} programs, {} native runs, {} disagree", n, runs, fails.len());
    assert!(fails.is_empty(), "programs disagreeing across tiers (rerun with FUZZ_SEED=<hex>): {:x?}", fails);
}

/// Seeds the clock-seeded batch found (FUZZ-NEW): they block from then on, in every tier.
/// - 3853108f782e2d2a (O2 thorough, checker only; the code agreed): `t = s as i8` read only in
///   its low bits, coalesced with s (equal_copies) in a caller-saved register, both saved around a
///   call in a loop; the checker took the cast for a copy in the register only, so the restore
///   of s's save slot (stored before t existed) dropped t.
const FOUND_SEEDS: [u64; 2] = [0x3853_108f_782e_2d2a, 0x475f_7b26_c831_6268];

#[test]
fn found_seeds_agree_across_tiers() {
    let fails: Vec<String> = FOUND_SEEDS.iter().filter_map(|&s| check(s).err()).collect();
    assert!(fails.is_empty(), "found seeds disagree again:\n{}", fails.join("\n"));
}

// ---- minimized cases the fuzzer found (each runs through every tier like a random program)

/// `x = f(.., x, ..)` in a loop with x in a caller-saved register (every callee-saved one taken
/// by values live across the call): x's ranges touch at the call and merged, so allocate_low
/// saved x around the call that defines it, and the restore after the result overwrote the
/// result with the old x (seeds 5b89fad58bb77d74, 459be4c9d7a5904 and 14 more of the first 600).
fn regress_call_redefines(n_live: usize) -> Func {
    let mut f = Func::new("regress_call_redefines".to_string(), 0);
    let mem = f.vreg(Cls::I);
    let ps: Vec<VReg> = (0..3).map(|_| f.vreg(Cls::I)).collect();
    f.params = std::iter::once(mem).chain(ps.iter().copied()).collect();
    f.rets = vec![Cls::I];
    let (b0, head, body, exit) = (f.block(), f.block(), f.block(), f.block());
    let push = |f: &mut Func, b: u32, i: Inst| {
        f.blocks[b as usize].insts.push(i);
        f.blocks[b as usize].pos.push(0);
    };
    let (h6, x, i, n, c) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b0, Inst::Iconst(h6, host6 as *const () as usize as i64));
    push(&mut f, b0, Inst::Mov(x, ps[0]));
    push(&mut f, b0, Inst::Iconst(i, 0));
    push(&mut f, b0, Inst::IBinI(IOp::And, W64, n, ps[1], 7));
    // values live across the loop and its call: they take the callee-saved registers
    let live: Vec<VReg> = (0..n_live).map(|k| {
        let v = f.vreg(Cls::I);
        push(&mut f, b0, Inst::IBinI(IOp::Add, W64, v, ps[2], 11 * k as i64 + 1));
        v
    }).collect();
    f.blocks[b0 as usize].term = Term::Jump(head);
    push(&mut f, head, Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[head as usize].term = Term::Branch(c, body, exit);
    let z = f.vreg(Cls::I);
    push(&mut f, body, Inst::Iconst(z, 0));
    // (read in the loop more than x is: they win the callee-saved registers, x gets a
    // caller-saved one)
    let mut t = z;
    for _ in 0..3 {
        for &v in &live {
            let t2 = f.vreg(Cls::I);
            push(&mut f, body, Inst::IBin(IOp::Add, W64, t2, t, v));
            t = t2;
        }
    }
    push(&mut f, body, Inst::Store(Mem::Int(8, false), mem, 0, t));
    push(&mut f, body, Inst::Call(Callee::Indirect(h6), vec![z, live[0], x, i, z, z], vec![x]));
    push(&mut f, body, Inst::IBinI(IOp::Add, W64, i, i, 1));
    f.blocks[body as usize].term = Term::Jump(head);
    let mut acc = x;
    for v in live {
        let a = f.vreg(Cls::I);
        push(&mut f, exit, Inst::IBin(IOp::Xor, W64, a, acc, v));
        acc = a;
    }
    f.blocks[exit as usize].term = Term::Ret(vec![acc]);
    f
}

#[test]
fn regress_call_redefines_its_argument() {
    for n_live in [6, 12, 16] {
        let f = regress_call_redefines(n_live);
        f.verify().unwrap();
        assert!(undefined_read(&f).is_none());
        for p in [Pipe::Raw, Pipe::O2, Pipe::O3] {
            for knobs in KNOBS {
                for thorough in [false, true] {
                    assert!(!disagrees(&f, 0x5b89_fad5_8bb7_7d74, p, knobs, thorough), "n_live {} {:?} {:?} thorough={}", n_live, p, knobs, thorough);
                }
            }
        }
    }
}

/// A narrow load of what a narrow store just wrote, the stored register wider than the store:
/// simplify's store-to-load forwarding replaced the load with the whole register (`store u8
/// [p], x; load u8 [p]` gave x, not x as u8; seeds f880dbd780695b33, fb49238b68499da6,
/// acd22286c6926577).
fn regress_narrow_forward(m: Mem) -> Func {
    let mut f = Func::new("regress_narrow_forward".to_string(), 0);
    let mem = f.vreg(Cls::I);
    let ps: Vec<VReg> = (0..3).map(|_| f.vreg(Cls::I)).collect();
    f.params = std::iter::once(mem).chain(ps.iter().copied()).collect();
    f.rets = vec![Cls::I];
    let b0 = f.block();
    let x = f.vreg(Cls::I);
    for i in [Inst::Store(m, mem, 8, ps[0]), Inst::Load(m, x, mem, 8)] {
        f.blocks[b0 as usize].insts.push(i);
        f.blocks[b0 as usize].pos.push(0);
    }
    f.blocks[b0 as usize].term = Term::Ret(vec![x]);
    f
}

#[test]
fn regress_narrow_store_forwarded_to_load() {
    for m in [Mem::Int(1, false), Mem::Int(1, true), Mem::Int(2, false), Mem::Int(2, true), Mem::Int(4, false), Mem::Int(4, true)] {
        let f = regress_narrow_forward(m);
        f.verify().unwrap();
        for p in [Pipe::Raw, Pipe::O2, Pipe::O3] {
            for thorough in [false, true] {
                assert!(!disagrees(&f, 0xf880_dbd7_8069_5b33, p, &[], thorough), "{:?} {:?} thorough={}", m, p, thorough);
            }
        }
    }
}

/// copy_reuse served a read of a spilled value from the register of its copy after the copy's
/// value was dead: `u = v; t = ..; w = t ? v : y; u = 0; .. u ..` with u's first value never read.
/// The read of u's second value kept u "read later in the block", but the allocator gives u's
/// register away between its two definitions, here to t, so the select read t for v. Seed
/// 475f7b26c8316268 (after O2: v1 spilled, `Mov v3, v1` in b29) had t a compare fused into the
/// select (never written: only the allocation check saw it); `load`: t is a load, written, and the
/// result is wrong.
fn regress_dead_copy_reuse(load: bool) -> Func {
    let mut f = Func::new("regress_dead_copy_reuse".to_string(), 0);
    let v: Vec<VReg> = (0..20).map(|_| f.vreg(Cls::I)).collect();
    f.params = vec![v[0], v[1], v[2], v[3]];
    f.rets = vec![Cls::I];
    let (b0, b1, b2, b4, b5, b6, b12, b27, b29) = (f.block(), f.block(), f.block(), f.block(), f.block(), f.block(), f.block(), f.block(), f.block());
    let push = |f: &mut Func, b: u32, i: Inst| {
        f.blocks[b as usize].insts.push(i);
        f.blocks[b as usize].pos.push(0);
    };
    push(&mut f, b0, Inst::Iconst(v[4], 0));
    for (d, s) in [(5, 4), (6, 3), (7, 3), (8, 4), (9, 3), (10, 4), (11, 3), (12, 4), (13, 4)] {
        push(&mut f, b0, Inst::Mov(v[d], v[s]));
    }
    f.blocks[b0 as usize].term = Term::Jump(b1);
    push(&mut f, b1, Inst::ICmp(Cond::Lt, false, v[14], v[13], v[2]));
    f.blocks[b1 as usize].term = Term::Branch(v[14], b2, b27);
    f.blocks[b2 as usize].term = Term::Branch(v[3], b4, b5);
    push(&mut f, b4, Inst::Mov(v[4], v[11]));
    push(&mut f, b4, Inst::Mov(v[10], v[7]));
    f.blocks[b4 as usize].term = Term::Jump(b6);
    push(&mut f, b5, Inst::Iconst(v[15], 0));
    push(&mut f, b5, Inst::Sel(v[7], v[6], v[15], v[7]));
    push(&mut f, b5, Inst::Mov(v[8], v[0]));
    f.blocks[b5 as usize].term = Term::Jump(b6);
    push(&mut f, b6, Inst::IBinI(IOp::Add, W64, v[13], v[13], 1));
    f.blocks[b6 as usize].term = Term::Jump(b1);
    let (x16, x17, x18) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b12, Inst::IBin(IOp::Xor, W64, x16, v[5], v[8]));
    push(&mut f, b12, Inst::IBin(IOp::Xor, W64, x17, x16, v[10]));
    push(&mut f, b12, Inst::IBin(IOp::Xor, W64, x18, x17, v[12]));
    f.blocks[b12 as usize].term = Term::Ret(vec![x18]);
    f.blocks[b27 as usize].term = Term::Branch(v[3], b12, b29);
    push(&mut f, b29, Inst::Mov(v[3], v[1]));
    push(&mut f, b29, if load { Inst::Load(Mem::Int(8, false), v[19], v[0], 8) } else { Inst::ICmpI(Cond::Ne, false, v[19], v[4], 0) });
    push(&mut f, b29, Inst::Sel(v[12], v[19], v[1], v[9]));
    push(&mut f, b29, Inst::Iconst(v[3], 0));
    push(&mut f, b29, Inst::Mov(v[5], v[3]));
    f.blocks[b29 as usize].term = Term::Jump(b12);
    f
}

#[test]
fn regress_dead_copy_not_reused() {
    // (the select's path needs the last parameter 0; memory word 1 = 1 selects v)
    let cases: Vec<[u64; 3]> = vec![[0x1234_5678_9abc_def1, 3, 0], [0xfeed_beef_0000_0005, 0, 0], [7, 5, 9]];
    // (the code without the allocation check first: the wrong register shows in the result)
    for check in [false, true] {
        for load in [true, false] {
            let f = regress_dead_copy_reuse(load);
            f.verify().unwrap();
            assert!(undefined_read(&f).is_none());
            for p in [Pipe::Raw, Pipe::O2, Pipe::O3] {
                for knobs in KNOBS {
                    for thorough in [false, true] {
                        let mut o = options(knobs);
                        if !check {
                            o.opt.retain(|k| k != "+checkra");
                        }
                        let g = pipe(&o, &f, p);
                        let bytes = native(&o, &g, thorough).unwrap_or_else(|e| panic!("load {} {:?} {:?} thorough={}: {}", load, p, knobs, thorough, e));
                        let code = Code::new(&bytes).unwrap();
                        let run: extern "C" fn(*mut u64, u64, u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
                        for a in &cases {
                            // (one buffer for both: the result folds in its address)
                            let mut m = [0u64; WORDS];
                            m[1] = 1;
                            let want = eval(&f, &mut m, *a).unwrap();
                            let got = run(m.as_mut_ptr(), a[0], a[1], a[2]);
                            assert_eq!(got, want, "load {} {:?} {:?} thorough={} check={} args {:?}", load, p, knobs, thorough, check, a);
                        }
                    }
                }
            }
        }
    }
}
