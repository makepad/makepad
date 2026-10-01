//! The random kernel generator the four-wide differential tests share
//! (tests/neon.rs, tests/wasm.rs, examples/wasm_diff.rs): kernel-shaped AIR
//! programs with divergent and uniform branches, loops with breaks and
//! continues at every depth, per-lane frames, hostile loads, integer
//! division, shifts, selects and variables written under masks, and
//! buffers with guard words.
#![allow(dead_code)]

use makepad_script_compute::ir::{Bin, Block, Cmp, Fma, Op, Program, Region, Stmt, Ty, Un, Val, Var};
use makepad_script_compute::kernel::{ELEMENT_CAP, K_BASE, K_OVERFLOW, K_PARAMS};

pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    pub fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
}

pub const FRAME: u32 = 6;
pub const STRIDE: u32 = 3;
// Buffers: 0 control, 1 input (hostile reads), 2 and 3 outputs (element
// records of STRIDE words).
pub const NPARAMS: u32 = 4;
/// The shared tables random kernels read (16 words: resident-table reads).
pub const SHARED: [u32; 16] = [
    0x3F80_0000, 0xBF00_0000, 0x4040_0000, 0x7FC0_0000, 0x0000_0001, 0x8000_0000, 0x4120_0000, 0xC2C8_0000, 0x3E80_0000, 0x7F80_0000, 0x1234_5678, 0xFFFF_FFFF, 0x4000_0000, 0x3F00_0000, 0xC000_0000, 0x0080_0000,
];

struct Gen<'r> {
    r: &'r mut Rng,
    p: Program,
    /// Visible values per scope level.
    scope: Vec<Vec<Val>>,
    vars: Vec<Var>,
    e: Val,
    /// Loop depth inside the element body.
    loops: u32,
    budget: u32,
    /// The functions made so far: (param types, result types); the first
    /// param of each is the element index.
    funcs: Vec<(Vec<Ty>, Vec<Ty>)>,
}

impl Gen<'_> {
    fn val(&mut self, ty: Ty) -> Val {
        self.p.vals.push(ty);
        Val(self.p.vals.len() as u32 - 1)
    }

    fn visible(&self, ty: Ty) -> Vec<Val> {
        self.scope.iter().flatten().copied().filter(|v| self.p.vals[v.0 as usize] == ty).collect()
    }

    fn def(&mut self, b: &mut Block, ty: Ty, op: Op) -> Val {
        let v = self.val(ty);
        b.push(Stmt::Def(v, op));
        self.scope.last_mut().unwrap().push(v);
        v
    }

    fn konst(&mut self, b: &mut Block, ty: Ty) -> Val {
        let op = match ty {
            Ty::F32 => {
                const F: [f32; 12] = [0.0, -0.0, 1.0, -1.0, 0.5, 3.75, 1e30, -1e-30, f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 2.1e9];
                Op::ConstF(if self.r.chance(70) { F[self.r.below(12) as usize] } else { f32::from_bits(self.r.next() as u32) })
            }
            Ty::I32 => {
                const I: [i32; 12] = [0, 1, -1, 2, 3, 7, 31, 32, 33, i32::MIN, i32::MAX, 1000];
                Op::ConstI(if self.r.chance(70) { I[self.r.below(12) as usize] } else { self.r.next() as i32 })
            }
            _ => Op::ConstB(self.r.chance(50)),
        };
        self.def(b, ty, op)
    }

    /// A visible value of `ty` (or a fresh constant).
    fn pick(&mut self, b: &mut Block, ty: Ty) -> Val {
        let vis = self.visible(ty);
        if vis.is_empty() || self.r.chance(10) {
            return self.konst(b, ty);
        }
        vis[self.r.below(vis.len() as u64) as usize]
    }

    fn arith(&mut self, b: &mut Block) {
        let ty = [Ty::F32, Ty::I32, Ty::Bool][self.r.below(3) as usize];
        match ty {
            Ty::F32 => {
                let op = match self.r.below(6) {
                    5 => {
                        let k = [Fma::Add, Fma::SubFrom, Fma::Sub][self.r.below(3) as usize];
                        Op::Fma(k, self.pick(b, Ty::F32), self.pick(b, Ty::F32), self.pick(b, Ty::F32))
                    }
                    0 => {
                        let u = [Un::NegF, Un::AbsF, Un::SqrtF, Un::FloorF, Un::CeilF, Un::TruncF, Un::RoundF][self.r.below(7) as usize];
                        let a = self.pick(b, Ty::F32);
                        Op::Un(u, a)
                    }
                    1 => {
                        let a = self.pick(b, Ty::I32);
                        Op::Un(if self.r.chance(50) { Un::I2F } else { Un::BitsIF }, a)
                    }
                    2 => {
                        let c = self.pick(b, Ty::Bool);
                        let x = self.pick(b, Ty::F32);
                        let y = self.pick(b, Ty::F32);
                        Op::Sel(c, x, y)
                    }
                    _ => {
                        let o = [Bin::AddF, Bin::SubF, Bin::MulF, Bin::DivF, Bin::MinF, Bin::MaxF][self.r.below(6) as usize];
                        let x = self.pick(b, Ty::F32);
                        let y = self.pick(b, Ty::F32);
                        Op::Bin(o, x, y)
                    }
                };
                self.def(b, Ty::F32, op);
            }
            Ty::I32 => {
                let op = match self.r.below(8) {
                    6 => Op::Fma(Fma::MulAddI, self.pick(b, Ty::I32), self.pick(b, Ty::I32), self.pick(b, Ty::I32)),
                    // (FrameCount is the call's element count: it depends on
                    // how a run is split, as the counter does; kernels read
                    // their count from ctx.)
                    7 => Op::BufLen(self.r.below(5) as u8),
                    0 => {
                        let a = self.pick(b, Ty::F32);
                        Op::Un(if self.r.chance(50) { Un::F2I } else { Un::BitsFI }, a)
                    }
                    1 => {
                        let a = self.pick(b, Ty::I32);
                        Op::Un(Un::NegI, a)
                    }
                    2 => {
                        let a = self.pick(b, Ty::I32);
                        let len = [1u32, 2, 3, 5, 8, 7, 1000, 1 << 20, 1505, 0x7FFF_FFFF, 0x8000_0001, 641][self.r.below(12) as usize];
                        Op::Wrap(a, len)
                    }
                    3 => {
                        let c = self.pick(b, Ty::Bool);
                        let x = self.pick(b, Ty::I32);
                        let y = self.pick(b, Ty::I32);
                        Op::Sel(c, x, y)
                    }
                    4 => Op::Bin(Bin::AddI, self.e, self.pick(b, Ty::I32)),
                    _ => {
                        let o = [Bin::AddI, Bin::SubI, Bin::MulI, Bin::DivI, Bin::RemI, Bin::AndI, Bin::OrI, Bin::XorI, Bin::ShlI, Bin::ShrI, Bin::ShrUI]
                            [self.r.below(11) as usize];
                        let x = self.pick(b, Ty::I32);
                        let y = self.pick(b, Ty::I32);
                        Op::Bin(o, x, y)
                    }
                };
                self.def(b, Ty::I32, op);
            }
            _ => {
                let cc = [Cmp::Lt, Cmp::Le, Cmp::Gt, Cmp::Ge, Cmp::Eq, Cmp::Ne][self.r.below(6) as usize];
                let op = match self.r.below(7) {
                    0 => Op::CmpF(cc, self.pick(b, Ty::F32), self.pick(b, Ty::F32)),
                    1 => Op::CmpI(cc, self.pick(b, Ty::I32), self.pick(b, Ty::I32)),
                    // Bools compared (ordered too: true is above false) and
                    // selected.
                    5 => Op::CmpI(cc, self.pick(b, Ty::Bool), self.pick(b, Ty::Bool)),
                    6 => Op::Sel(self.pick(b, Ty::Bool), self.pick(b, Ty::Bool), self.pick(b, Ty::Bool)),
                    2 => Op::Un(Un::NotB, self.pick(b, Ty::Bool)),
                    3 => Op::Bin(if self.r.chance(50) { Bin::AndB } else { Bin::OrB }, self.pick(b, Ty::Bool), self.pick(b, Ty::Bool)),
                    // Element-dependent conditions (divergence).
                    _ => {
                        let mk = 1 + self.r.below(5) as i32;
                        let m = self.konst_i(b, mk);
                        let w = self.def(b, Ty::I32, Op::Bin(Bin::RemI, self.e, m));
                        let zk = self.r.below(3) as i32;
                        let z = self.konst_i(b, zk);
                        Op::CmpI(cc, w, z)
                    }
                };
                self.def(b, Ty::Bool, op);
            }
        }
    }

    fn konst_i(&mut self, b: &mut Block, x: i32) -> Val {
        self.def(b, Ty::I32, Op::ConstI(x))
    }

    /// e * STRIDE: the element's record.
    fn record(&mut self, b: &mut Block) -> Val {
        let s = self.konst_i(b, STRIDE as i32);
        self.def(b, Ty::I32, Op::Bin(Bin::MulI, self.e, s))
    }

    fn memory(&mut self, b: &mut Block) {
        match self.r.below(8) {
            // Hostile reads of the input.
            0 => {
                let ty = if self.r.chance(50) { Ty::F32 } else { Ty::I32 };
                let off = if self.r.chance(30) { None } else { Some(self.pick(b, Ty::I32)) };
                let base = [0u32, 1, 5, u32::MAX - 2][self.r.below(4) as usize];
                self.def(b, ty, Op::Load { region: Region::Buf(1), base, extent: u32::MAX, off });
            }
            // Own-record reads and writes of the outputs.
            1 | 2 => {
                let k = 2 + self.r.below(2) as u8;
                let rec = self.record(b);
                let base = self.r.below(STRIDE as u64) as u32;
                if self.r.chance(35) {
                    let ty = if self.r.chance(50) { Ty::F32 } else { Ty::I32 };
                    self.def(b, ty, Op::Load { region: Region::Buf(k), base, extent: u32::MAX, off: Some(rec) });
                } else {
                    let ty = [Ty::F32, Ty::I32, Ty::Bool][self.r.below(3) as usize];
                    let val = self.pick(b, ty);
                    b.push(Stmt::Store { region: Region::Buf(k), base, extent: u32::MAX, off: Some(rec), val });
                }
            }
            // A one-word-per-element output (contiguous vector stores).
            7 => {
                if self.r.chance(30) {
                    self.def(b, Ty::F32, Op::Load { region: Region::Buf(4), base: 0, extent: u32::MAX, off: Some(self.e) });
                } else {
                    let ty = [Ty::F32, Ty::I32][self.r.below(2) as usize];
                    let val = self.pick(b, ty);
                    b.push(Stmt::Store { region: Region::Buf(4), base: 0, extent: u32::MAX, off: Some(self.e), val });
                }
            }
            // The frame, uniform and per-lane offsets.
            3 | 4 => {
                let base = self.r.below(FRAME as u64) as u32;
                let extent = 1 + self.r.below((FRAME - base) as u64) as u32;
                let off = if self.r.chance(30) { None } else { Some(self.pick(b, Ty::I32)) };
                let extent = if off.is_none() { 1 } else { extent };
                if self.r.chance(50) {
                    let ty = [Ty::F32, Ty::I32, Ty::Bool][self.r.below(3) as usize];
                    self.def(b, ty, Op::Load { region: Region::Frame, base, extent, off });
                } else {
                    let ty = [Ty::F32, Ty::I32][self.r.below(2) as usize];
                    let val = self.pick(b, ty);
                    b.push(Stmt::Store { region: Region::Frame, base, extent, off, val });
                }
            }
            // Params (uniform, or at a varying clamped offset), and the
            // shared table (at offsets proven inside it, or clamped).
            5 => match self.r.below(4) {
                0 => {
                    let k = self.r.below(NPARAMS as u64) as u32;
                    self.def(b, Ty::F32, Op::Load { region: Region::Ctx, base: K_PARAMS + k, extent: 1, off: None });
                }
                1 => {
                    let o = self.pick(b, Ty::I32);
                    self.def(b, Ty::F32, Op::Load { region: Region::Ctx, base: K_PARAMS, extent: NPARAMS, off: Some(o) });
                }
                2 => {
                    let x = self.pick(b, Ty::I32);
                    let len = [16u32, 8, 13][self.r.below(3) as usize];
                    let o = self.def(b, Ty::I32, Op::Wrap(x, len));
                    let base = self.r.below((SHARED.len() as u32 - len + 1) as u64) as u32;
                    let ty = if self.r.chance(70) { Ty::F32 } else { Ty::I32 };
                    self.def(b, ty, Op::Load { region: Region::Shared, base, extent: len, off: Some(o) });
                }
                _ => {
                    let o = self.pick(b, Ty::I32);
                    let base = self.r.below(SHARED.len() as u64) as u32;
                    let extent = SHARED.len() as u32 - base;
                    self.def(b, Ty::F32, Op::Load { region: Region::Shared, base, extent, off: Some(o) });
                }
            },
            _ => {
                let one = self.konst_i(b, 1);
                b.push(Stmt::Store { region: Region::Ctx, base: K_OVERFLOW, extent: 1, off: None, val: one });
            }
        }
    }

    fn block(&mut self, depth: u32) -> Block {
        let mut b = Vec::new();
        self.scope.push(Vec::new());
        let n = 1 + self.r.below(7);
        for _ in 0..n {
            if self.budget == 0 {
                break;
            }
            self.budget -= 1;
            match self.r.below(13) {
                // A call of a function made earlier (the element index, then
                // visible arguments; its results become visible).
                12 if !self.funcs.is_empty() => {
                    let f = self.r.below(self.funcs.len() as u64) as usize;
                    let (ps, rs) = self.funcs[f].clone();
                    let mut args = vec![self.e];
                    for t in &ps[1..] {
                        args.push(self.pick(&mut b, *t));
                    }
                    let rets: Vec<Val> = rs.iter().map(|t| self.val(*t)).collect();
                    b.push(Stmt::Call { f: f as u16, args, rets: rets.clone() });
                    self.scope.last_mut().unwrap().extend(rets);
                }
                0..=3 => self.arith(&mut b),
                4 | 5 => self.memory(&mut b),
                6 => {
                    let var = self.vars[self.r.below(self.vars.len() as u64) as usize];
                    let ty = self.p.vars[var.0 as usize];
                    let v = self.pick(&mut b, ty);
                    b.push(Stmt::Set(var, v));
                }
                7 => {
                    let var = self.vars[self.r.below(self.vars.len() as u64) as usize];
                    let ty = self.p.vars[var.0 as usize];
                    self.def(&mut b, ty, Op::Get(var));
                }
                8 | 9 if depth < 4 => {
                    let c = self.pick(&mut b, Ty::Bool);
                    let t = self.block(depth + 1);
                    let e = if self.r.chance(50) { self.block(depth + 1) } else { vec![] };
                    b.push(Stmt::If(c, t, e));
                }
                10 if depth < 4 => {
                    let cap = [0u32, 1, 2, 3, 5][self.r.below(5) as usize];
                    self.loops += 1;
                    let body = self.block(depth + 1);
                    self.loops -= 1;
                    b.push(Stmt::Loop { cap, body });
                }
                11 if self.loops > 0 => {
                    // A break or continue, usually conditional.
                    let d = self.r.below(self.loops as u64) as u32;
                    let s = if self.r.chance(50) { Stmt::Break(d) } else { Stmt::Continue(d) };
                    if self.r.chance(75) {
                        let c = self.pick(&mut b, Ty::Bool);
                        b.push(Stmt::If(c, vec![s], vec![]));
                    } else {
                        b.push(s);
                    }
                }
                _ => self.arith(&mut b),
            }
        }
        self.scope.pop();
        b
    }
}

/// A random function of a kernel: the element index and up to three more
/// params, variables set on entry, a random body (branches, loops with
/// breaks, memory, calls of the earlier functions `sigs`), and up to three
/// results from its top level.
fn random_function(r: &mut Rng, sigs: &[(Vec<Ty>, Vec<Ty>)]) -> Program {
    let mut p = Program::default();
    let mut params = Vec::new();
    for k in 0..1 + r.below(4) {
        let ty = if k == 0 { Ty::I32 } else { [Ty::F32, Ty::I32, Ty::Bool][r.below(3) as usize] };
        p.vals.push(ty);
        params.push(Val(p.vals.len() as u32 - 1));
    }
    let e = params[0];
    let mut vars = Vec::new();
    for _ in 0..1 + r.below(3) {
        p.vars.push([Ty::F32, Ty::I32, Ty::Bool][r.below(3) as usize]);
        vars.push(Var(p.vars.len() as u32 - 1));
    }
    let mut g = Gen { r, p, scope: vec![params.clone()], vars: vars.clone(), e, loops: 0, budget: 25, funcs: sigs.to_vec() };
    let mut body = Vec::new();
    for v in &vars {
        let ty = g.p.vars[v.0 as usize];
        let c = g.konst(&mut body, ty);
        body.push(Stmt::Set(*v, c));
    }
    body.extend(g.block(0));
    // Results: params or values the top level defines.
    let top: Vec<Val> = params.iter().copied().chain(body.iter().filter_map(|s| if let Stmt::Def(v, _) = s { Some(*v) } else { None })).collect();
    let n = g.r.below(4) as usize;
    let results: Vec<Val> = (0..n).map(|_| top[g.r.below(top.len() as u64) as usize]).collect();
    let mut p = g.p;
    p.body = body;
    p.params = params;
    p.results = results;
    p
}

/// A random kernel in the shape `lower_kernel` emits.
pub fn random_kernel(r: &mut Rng) -> Program {
    let mut p = Program::default();
    let mut top: Block = Vec::new();
    let val = |p: &mut Program, ty: Ty| {
        p.vals.push(ty);
        Val(p.vals.len() as u32 - 1)
    };
    let n = val(&mut p, Ty::I32);
    top.push(Stmt::Def(n, Op::FrameCount));
    let base = val(&mut p, Ty::I32);
    top.push(Stmt::Def(base, Op::Load { region: Region::Ctx, base: K_BASE, extent: 1, off: None }));
    p.vars.push(Ty::I32);
    let i = Var(0);
    let z = val(&mut p, Ty::I32);
    top.push(Stmt::Def(z, Op::ConstI(0)));
    top.push(Stmt::Set(i, z));
    // User variables: every one initialized per element (no garbage reads).
    let nvars = 1 + r.below(5) as usize;
    let mut vars = Vec::new();
    for _ in 0..nvars {
        p.vars.push([Ty::F32, Ty::I32, Ty::Bool][r.below(3) as usize]);
        vars.push(Var(p.vars.len() as u32 - 1));
    }
    let mut body: Block = Vec::new();
    let iv = val(&mut p, Ty::I32);
    body.push(Stmt::Def(iv, Op::Get(i)));
    let done = val(&mut p, Ty::Bool);
    body.push(Stmt::Def(done, Op::CmpI(Cmp::Ge, iv, n)));
    body.push(Stmt::If(done, vec![Stmt::Break(0)], vec![]));
    let cw = val(&mut p, Ty::I32);
    body.push(Stmt::Def(cw, Op::Load { region: Region::Buf(0), base: 0, extent: 1, off: None }));
    let cz = val(&mut p, Ty::I32);
    body.push(Stmt::Def(cz, Op::ConstI(0)));
    let stop = val(&mut p, Ty::Bool);
    body.push(Stmt::Def(stop, Op::CmpI(Cmp::Ne, cw, cz)));
    body.push(Stmt::If(stop, vec![Stmt::Break(0)], vec![]));
    let e = val(&mut p, Ty::I32);
    body.push(Stmt::Def(e, Op::Bin(Bin::AddI, base, iv)));
    for w in 0..FRAME {
        body.push(Stmt::Store { region: Region::Frame, base: w, extent: 1, off: None, val: cz });
    }
    // Only the element index is visible (the counter and the call's base
    // depend on how a range is split, as in real kernels).
    // Functions (each may call the ones before it).
    let mut funcs = Vec::new();
    let mut sigs: Vec<(Vec<Ty>, Vec<Ty>)> = Vec::new();
    if r.chance(40) {
        for _ in 0..1 + r.below(2) {
            let g = random_function(r, &sigs);
            sigs.push((g.params.iter().map(|v| g.vals[v.0 as usize]).collect(), g.results.iter().map(|v| g.vals[v.0 as usize]).collect()));
            funcs.push(g);
        }
    }
    let mut g = Gen { r, p, scope: vec![vec![z, cz, e]], vars: vars.clone(), e, loops: 0, budget: 60, funcs: sigs };
    for v in &vars {
        let ty = g.p.vars[v.0 as usize];
        let c = g.konst(&mut body, ty);
        body.push(Stmt::Set(*v, c));
    }
    // Register pressure: many values live across the whole body (spills).
    let mut live = Vec::new();
    if g.r.chance(30) {
        for _ in 0..40 {
            let x = g.pick(&mut body, Ty::I32);
            let y = g.pick(&mut body, Ty::I32);
            live.push(g.def(&mut body, Ty::I32, Op::Bin(Bin::XorI, x, e)));
            live.push(g.def(&mut body, Ty::F32, Op::Un(Un::I2F, y)));
        }
    }
    let user = g.block(0);
    body.extend(user);
    g.p.funcs = funcs;
    if !live.is_empty() {
        let rec = g.record(&mut body);
        for (k, v) in live.iter().enumerate() {
            let base = (k % STRIDE as usize) as u32;
            body.push(Stmt::Store { region: Region::Buf(2 + (k % 2) as u8), base, extent: u32::MAX, off: Some(rec), val: *v });
        }
    }
    let mut p = g.p;
    let one = {
        p.vals.push(Ty::I32);
        Val(p.vals.len() as u32 - 1)
    };
    body.push(Stmt::Def(one, Op::ConstI(1)));
    let next = {
        p.vals.push(Ty::I32);
        Val(p.vals.len() as u32 - 1)
    };
    body.push(Stmt::Def(next, Op::Bin(Bin::AddI, iv, one)));
    body.push(Stmt::Set(i, next));
    top.push(Stmt::Loop { cap: ELEMENT_CAP, body });
    p.body = top;
    p.frame_words = FRAME;
    p
}

pub const CTX: usize = (K_PARAMS + NPARAMS) as usize;

/// Buffers with 3 guard words on each side.
pub struct Bufs {
    pub words: Vec<Vec<u32>>,
}

pub const G: usize = 3;
pub const GUARD: u32 = 0xDEAD_BEEF;

impl Bufs {
    pub fn new(r: &mut Rng, n: usize) -> Bufs {
        let lens = [1usize, 7 + r.below(20) as usize, n * STRIDE as usize, n * STRIDE as usize, n];
        let words = lens
            .iter()
            .enumerate()
            .map(|(k, len)| {
                let mut w = vec![GUARD; len + 2 * G];
                for x in &mut w[G..G + len] {
                    *x = if k == 1 { r.next() as u32 } else { 0 };
                }
                w
            })
            .collect();
        Bufs { words }
    }

    pub fn table(&mut self) -> Vec<u64> {
        let mut t = Vec::new();
        for w in &mut self.words {
            let len = w.len() - 2 * G;
            t.push(w[G..].as_mut_ptr() as u64);
            t.push(len as u64);
        }
        t
    }

    pub fn guards_intact(&self) -> bool {
        self.words.iter().all(|w| w[..G].iter().chain(&w[w.len() - G..]).all(|x| *x == GUARD))
    }
}

pub fn params(r: &mut Rng) -> Vec<u32> {
    let mut ctx = vec![0u32; CTX];
    for k in 0..NPARAMS as usize {
        ctx[K_PARAMS as usize + k] = [1.5f32, -0.0, f32::NAN, 7.0][k].to_bits() ^ (r.below(2) as u32);
    }
    ctx
}

