//! The wasm backend (`relax_backend::wasm`): RIR functions compiled to a module and run by the
//! in-tree interpreter (tests/wasm_rt), against hand-computed results and, for random
//! integer programs with branches and loops, against a direct RIR evaluator.

mod wasm_rt;

use relax_backend::rir::*;
use relax_backend::wasm::{self, Builtin, HostBind, Link, Module, Options};
use wasm_rt::{Instance, Trap};

struct L;
impl Link for L {
    fn sym(&self, id: u32) -> u64 {
        0x10_0000 + id as u64 * 64
    }
    fn host(&self, id: u32) -> HostBind {
        match id {
            1 => HostBind::Import("env".into(), "add7".into()),
            2 => HostBind::Builtin(Builtin::Memcpy),
            3 => HostBind::Builtin(Builtin::CatchPanic),
            4 => HostBind::Builtin(Builtin::Memset),
            _ => HostBind::Trap,
        }
    }
}

fn opts() -> Options {
    Options { stack_size: 64 * 1024, heap_pages: 1, ..Options::default() }
}

/// Builds a module of `fs` (slot ids 0..), exports function k as "f{k}".
fn build(fs: &[Func], o: Options) -> (Vec<u8>, wasm::Stats) {
    let options = relax_session::Options::default();
    let mut m = Module::new(o);
    for (k, f) in fs.iter().enumerate() {
        f.verify().unwrap();
        m.add_func(k as u32, f);
    }
    for k in 0..fs.len() {
        m.export_func(k as u32, &format!("f{}", k));
    }
    let a = m.place(16, 16);
    m.data_at(a, b"hello, data\0\0\0\0\0".to_vec());
    let out = m.finish(&options, &L);
    assert!(out.stats.failed.is_empty(), "{:?}", out.stats.failed);
    (out.bytes, out.stats)
}

/// RELAX_WASM_DUMP=dir: every module a test builds is written there (validated outside by an
/// engine, e.g. `WebAssembly.validate`).
fn dump(bytes: &[u8]) {
    if let Some(d) = std::env::var_os("RELAX_WASM_DUMP") {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n < 400 {
            std::fs::write(std::path::Path::new(&d).join(format!("m{}.wasm", n)), bytes).unwrap();
        }
    }
}

fn inst(bytes: &[u8]) -> Instance {
    dump(bytes);
    let mut i = Instance::new(bytes).unwrap();
    i.bind("env", "add7", Box::new(|_, a| Ok(vec![a[0].wrapping_add(7)])));
    i.bind("env", "u", Box::new(|_, a| Err(Trap::Trap(format!("untraced {}", a[0])))));
    i
}

fn run(fs: &[Func], k: usize, args: &[u64]) -> Result<Vec<u64>, Trap> {
    let (b, _) = build(fs, opts());
    let mut i = inst(&b);
    i.call_export(&format!("f{}", k), args)
}

struct B {
    f: Func,
    cur: u32,
}

impl B {
    fn new(params: &[Cls], rets: &[Cls]) -> B {
        let mut f = Func::new("t".into(), 0);
        for c in params {
            let v = f.vreg(*c);
            f.params.push(v);
        }
        f.rets = rets.to_vec();
        let b0 = f.block();
        B { f, cur: b0 }
    }
    fn p(&self, k: usize) -> VReg {
        self.f.params[k]
    }
    fn v(&mut self, c: Cls) -> VReg {
        self.f.vreg(c)
    }
    fn i(&mut self, x: Inst) {
        let b = &mut self.f.blocks[self.cur as usize];
        b.insts.push(x);
        b.pos.push(0);
    }
    fn blk(&mut self) -> u32 {
        self.f.block()
    }
    fn at(&mut self, b: u32) {
        self.cur = b;
    }
    fn term(&mut self, t: Term) {
        self.f.blocks[self.cur as usize].term = t;
    }
    fn iconst(&mut self, k: i64) -> VReg {
        let v = self.v(Cls::I);
        self.i(Inst::Iconst(v, k));
        v
    }
    fn bin(&mut self, op: IOp, it: IntTy, a: VReg, b: VReg) -> VReg {
        let v = self.v(Cls::I);
        self.i(Inst::IBin(op, it, v, a, b));
        v
    }
    fn done(self) -> Func {
        self.f
    }
}

const U8: IntTy = IntTy { bits: 8, signed: false };
const I8: IntTy = IntTy { bits: 8, signed: true };
const I16: IntTy = IntTy { bits: 16, signed: true };

#[test]
fn integer_ops_and_narrow_types() {
    let cases: Vec<(IOp, IntTy, u64, u64, u64)> = vec![
        (IOp::Add, I64, 5, 7, 12),
        (IOp::Sub, I64, 5, 7, (-2i64) as u64),
        (IOp::Mul, I64, 1 << 40, 1 << 30, 0),
        (IOp::Add, U8, 200, 100, 44),
        (IOp::Add, I8, 100, 100, (-56i64) as u64),
        (IOp::Mul, I16, 300, 300, (90000i64 as i16) as i64 as u64),
        (IOp::Add, U32, 0xFFFF_FFFF, 2, 1),
        (IOp::Add, I32, 0x7FFF_FFFF, 1, (i32::MIN as i64) as u64),
        (IOp::Shl, U32, 1, 33, 2),
        (IOp::Shr, U32, 0x8000_0000, 31, 1),
        (IOp::Shr, I64, (-16i64) as u64, 2, (-4i64) as u64),
        (IOp::Shr, U64, (-16i64) as u64, 60, 15),
        (IOp::Div, I64, (-7i64) as u64, 2, (-3i64) as u64),
        (IOp::Rem, I64, (-7i64) as u64, 2, (-1i64) as u64),
        (IOp::Div, U64, u64::MAX, 2, u64::MAX / 2),
        (IOp::Div, I32, (i32::MIN as i64) as u64, (-1i64) as u64, (i32::MIN as i64) as u64),
        (IOp::MulHi, U64, u64::MAX, u64::MAX, u64::MAX - 1),
        (IOp::MulHi, I64, (-3i64) as u64, 5, u64::MAX),
        (IOp::MulHi, I64, 1 << 62, 8, 2),
        (IOp::Rotl, U64, 0x8000_0000_0000_0001, 4, 0x18),
        (IOp::Rotl, U32, 0x8000_0001, 4, 0x18),
        (IOp::Xor, I64, 0xF0, 0xFF, 0x0F),
    ];
    for (op, it, a, b, want) in cases {
        let mut f = B::new(&[Cls::I, Cls::I], &[Cls::I]);
        let (x, y) = (f.p(0), f.p(1));
        let r = f.bin(op, it, x, y);
        f.term(Term::Ret(vec![r]));
        let got = run(&[f.done()], 0, &[a, b]).unwrap();
        assert_eq!(got, vec![want], "{:?} {:?} {} {}", op, it, a, b);
    }
    // bit ops and conversions
    for (op, a, want) in [(BitOp::Popcnt, 0xF0F0u64, 8u64), (BitOp::Clz, 1, 63), (BitOp::Ctz, 0, 64), (BitOp::Bswap, 0x0102030405060708, 0x0807060504030201), (BitOp::Rev, 1, 1 << 63), (BitOp::Rev, 0x0F, 0xF0 << 56)] {
        let mut f = B::new(&[Cls::I], &[Cls::I]);
        let x = f.p(0);
        let d = f.v(Cls::I);
        f.i(Inst::IBit(op, d, x));
        f.term(Term::Ret(vec![d]));
        assert_eq!(run(&[f.done()], 0, &[a]).unwrap(), vec![want], "{:?}", op);
    }
}

#[test]
fn floats() {
    let bits = |x: f64| x.to_bits();
    let check = |op: FOp, a: f64, b: f64, want: f64| {
        let mut f = B::new(&[Cls::F64, Cls::F64], &[Cls::F64]);
        let (x, y) = (f.p(0), f.p(1));
        let d = f.v(Cls::F64);
        f.i(Inst::FBin(op, true, d, x, y));
        f.term(Term::Ret(vec![d]));
        let got = f64::from_bits(run(&[f.done()], 0, &[bits(a), bits(b)]).unwrap()[0]);
        assert!(got == want || (got.is_nan() && want.is_nan()), "{:?}({}, {}) = {} want {}", op, a, b, got, want);
    };
    check(FOp::Add, 1.5, 2.25, 3.75);
    check(FOp::Min, f64::NAN, 2.0, 2.0);
    check(FOp::Min, 3.0, f64::NAN, 3.0);
    check(FOp::Max, -1.0, 4.0, 4.0);
    check(FOp::MinLt, f64::NAN, 1.0, 1.0);
    check(FOp::MaxGt, 2.0, 1.0, 2.0);
    for (x, want) in [(2.5, 3.0), (-2.5, -3.0), (2.4999, 2.0), (-0.5, -1.0), (0.49999999999999994, 0.0), (1e300, 1e300)] {
        let mut f = B::new(&[Cls::F64], &[Cls::F64]);
        let a = f.p(0);
        let d = f.v(Cls::F64);
        f.i(Inst::FUnary(FUn::RoundAway, true, d, a));
        f.term(Term::Ret(vec![d]));
        assert_eq!(f64::from_bits(run(&[f.done()], 0, &[bits(x)]).unwrap()[0]), want, "round {}", x);
    }
    // saturating float -> int, int -> float, f32 consts
    for (x, to, want) in [(300.7, U8, 255u64), (-5.0, U8, 0), (f64::NAN, I32, 0), (1e20, I64, i64::MAX as u64), (-129.0, I8, (-128i64) as u64), (-3.9, I16, (-3i64) as u64)] {
        let mut f = B::new(&[Cls::F64], &[Cls::I]);
        let a = f.p(0);
        let d = f.v(Cls::I);
        f.i(Inst::Conv(Conv::FToInt(true, to), d, a));
        f.term(Term::Ret(vec![d]));
        assert_eq!(run(&[f.done()], 0, &[bits(x)]).unwrap(), vec![want], "{} as {:?}", x, to);
    }
    let mut f = B::new(&[Cls::I], &[Cls::F32]);
    let a = f.p(0);
    let d = f.v(Cls::F32);
    f.i(Inst::Conv(Conv::IntToF(U64, false), d, a));
    let k = f.v(Cls::F32);
    f.i(Inst::Fconst(k, 0.5f32.to_bits() as u64, false));
    let s = f.v(Cls::F32);
    f.i(Inst::FBin(FOp::Add, false, s, d, k));
    f.term(Term::Ret(vec![s]));
    assert_eq!(f32::from_bits(run(&[f.done()], 0, &[u64::MAX]).unwrap()[0] as u32), u64::MAX as f32 + 0.5);
    // fma (f64: one rounding)
    let mut f = B::new(&[Cls::F64, Cls::F64, Cls::F64], &[Cls::F64]);
    let (a, b2, c) = (f.p(0), f.p(1), f.p(2));
    let d = f.v(Cls::F64);
    f.i(Inst::FMulAdd(false, true, true, d, a, b2, c));
    f.term(Term::Ret(vec![d]));
    let fm = f.done();
    let (a, b2) = (1.0 + f64::EPSILON, 1.0 - f64::EPSILON);
    let c = a * b2;
    let got = f64::from_bits(run(&[fm], 0, &[bits(a), bits(b2), bits(c)]).unwrap()[0]);
    assert_eq!(got, a.mul_add(b2, -c));
}

/// A fused multiply-add with one NaN operand (either sign, quiet or signaling, a payload), every
/// negation form, f32 and f64: that NaN quieted, its sign flipped when the form negates that
/// operand (rir::fma_nan_f32 / f64; the native back ends give the same). Several NaNs: as the
/// engine's f64 ops pick (wasm leaves NaN bits nondeterministic), not pinned.
#[test]
fn fma_one_nan_operand() {
    for w in [false, true] {
        let cls = if w { Cls::F64 } else { Cls::F32 };
        let nans: Vec<u64> = if w {
            vec![0x7ff8_0000_0000_0000, 0xfff8_0000_0000_0000, 0x7ff8_0000_0000_1234, 0xfff0_0000_0000_0089]
        } else {
            vec![0x7fc0_0000, 0xffc0_0000, 0x7fc3_872e, 0xff80_0089]
        };
        let others: Vec<u64> = if w { [1.5f64, -2.0, 3.0].iter().map(|x| x.to_bits()).collect() } else { [1.5f32, -2.0, 3.0].iter().map(|x| x.to_bits() as u64).collect() };
        for (np, na) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut f = B::new(&[cls, cls, cls], &[cls]);
            let (a, b2, c) = (f.p(0), f.p(1), f.p(2));
            let d = f.v(cls);
            f.i(Inst::FMulAdd(np, na, w, d, a, b2, c));
            f.term(Term::Ret(vec![d]));
            let fm = f.done();
            for &q in &nans {
                for pos in 0..3 {
                    let mut args = [others[0], others[1], others[2]];
                    args[pos] = q;
                    let got = run(&[fm.clone()], 0, &args).unwrap()[0];
                    let want = if w {
                        let x = |k: usize| f64::from_bits(args[k]);
                        let (a, c) = (if np { -x(0) } else { x(0) }, if na { -x(2) } else { x(2) });
                        fma_nan_f64(a, x(1), c).unwrap()
                    } else {
                        let x = |k: usize| f32::from_bits(args[k] as u32);
                        let (a, c) = (if np { -x(0) } else { x(0) }, if na { -x(2) } else { x(2) });
                        fma_nan_f32(a, x(1), c).unwrap() as u64
                    };
                    let got = if w { got } else { got & 0xffff_ffff };
                    assert_eq!(got, want, "w {} np {} na {} NaN {:x} at {}", w, np, na, q, pos);
                }
            }
        }
    }
}

/// Sum of 0..n with a loop; nested loops; early exits.
#[test]
fn loops_and_branches() {
    // fn(n) { s = 0; i = 0; while i < n { if i % 3 == 0 { s += i } i += 1 } s }
    let mut f = B::new(&[Cls::I], &[Cls::I]);
    let n = f.p(0);
    let s = f.iconst(0);
    let i = f.iconst(0);
    let head = f.blk();
    let body = f.blk();
    let add = f.blk();
    let next = f.blk();
    let exit = f.blk();
    f.term(Term::Jump(head));
    f.at(head);
    let c = f.v(Cls::I);
    f.i(Inst::ICmp(Cond::Lt, true, c, i, n));
    f.term(Term::Branch(c, body, exit));
    f.at(body);
    let r = f.v(Cls::I);
    f.i(Inst::IBinI(IOp::Rem, I64, r, i, 3));
    let z = f.v(Cls::I);
    f.i(Inst::ICmpI(Cond::Eq, true, z, r, 0));
    f.term(Term::Branch(z, add, next));
    f.at(add);
    f.i(Inst::IBin(IOp::Add, I64, s, s, i));
    f.term(Term::Jump(next));
    f.at(next);
    f.i(Inst::IBinI(IOp::Add, I64, i, i, 1));
    f.term(Term::Jump(head));
    f.at(exit);
    f.term(Term::Ret(vec![s]));
    let fu = f.done();
    let want: u64 = (0..100u64).filter(|x| x % 3 == 0).sum();
    assert_eq!(run(&[fu], 0, &[100]).unwrap(), vec![want]);
}

/// An irreducible CFG (two entries into one cycle) runs through the dispatch loop.
#[test]
fn irreducible() {
    // b0: branch p ? b1 : b2 ; b1: x += 1; if x < 10 -> b2 else b3 ; b2: x += 2; -> b1 ; b3: ret x
    let mut f = B::new(&[Cls::I, Cls::I], &[Cls::I]);
    let (p, x) = (f.p(0), f.p(1));
    let b1 = f.blk();
    let b2 = f.blk();
    let b3 = f.blk();
    f.term(Term::Branch(p, b1, b2));
    f.at(b1);
    f.i(Inst::IBinI(IOp::Add, I64, x, x, 1));
    let c = f.v(Cls::I);
    f.i(Inst::ICmpI(Cond::Lt, true, c, x, 10));
    f.term(Term::Branch(c, b2, b3));
    f.at(b2);
    f.i(Inst::IBinI(IOp::Add, I64, x, x, 2));
    f.term(Term::Jump(b1));
    f.at(b3);
    f.term(Term::Ret(vec![x]));
    let fu = f.done();
    let reference = |p: u64, mut x: i64| -> i64 {
        let mut at = if p != 0 { 1 } else { 2 };
        loop {
            match at {
                1 => {
                    x += 1;
                    at = if x < 10 { 2 } else { 3 };
                }
                2 => {
                    x += 2;
                    at = 1;
                }
                _ => return x,
            }
        }
    };
    for (p, x) in [(1u64, 0i64), (0, 0), (0, 20), (1, 9)] {
        assert_eq!(run(&[fu.clone()], 0, &[p, x as u64]).unwrap(), vec![reference(p, x) as u64]);
    }
}

#[test]
fn switch_table() {
    let mut f = B::new(&[Cls::I], &[Cls::I]);
    let x = f.p(0);
    let arms: Vec<u32> = (0..5).map(|_| f.blk()).collect();
    let join = f.blk();
    let r = f.v(Cls::I);
    f.term(Term::Switch(x, std::sync::Arc::new(SwitchTable::new(10, vec![arms[0], arms[1], arms[2], arms[1]], arms[3]))));
    for (k, a) in arms.iter().enumerate() {
        f.at(*a);
        f.i(Inst::Iconst(r, 100 + k as i64));
        f.term(if k == 4 { Term::Ret(vec![r]) } else { Term::Jump(join) });
    }
    f.at(join);
    f.term(Term::Ret(vec![r]));
    let fu = f.done();
    for (x, want) in [(10u64, 100u64), (11, 101), (12, 102), (13, 101), (14, 103), (9, 103), (u64::MAX, 103), (1 << 40, 103)] {
        assert_eq!(run(&[fu.clone()], 0, &[x]).unwrap(), vec![want], "x = {}", x);
    }
}

/// Slots, loads/stores of every width, memcpy, data, and an address fold.
#[test]
fn memory_and_slots() {
    let mut f = B::new(&[Cls::I], &[Cls::I]);
    let x = f.p(0);
    let s = f.f.slot(32, 8);
    let p = f.v(Cls::I);
    f.i(Inst::SlotAddr(p, s));
    f.i(Inst::Store(Mem::Int(8, false), p, 0, x));
    f.i(Inst::Store(Mem::Int(1, false), p, 9, x));
    let q = f.v(Cls::I);
    f.i(Inst::IBinI(IOp::Add, I64, q, p, 16));
    f.i(Inst::Copy(q, p, 8));
    let a = f.v(Cls::I);
    f.i(Inst::Load(Mem::Int(1, true), a, q, 0));
    let b = f.v(Cls::I);
    f.i(Inst::Load(Mem::Int(2, false), b, p, 8));
    let k = f.iconst(3);
    let c = f.v(Cls::I);
    f.i(Inst::LoadX(Mem::Int(4, false), c, p, k, 2, 4));
    let d = f.bin(IOp::Add, I64, a, b);
    let e = f.bin(IOp::Add, I64, d, c);
    // data placed by the link step after the 64 KiB stack: "hello, data"
    let h = f.v(Cls::I);
    f.i(Inst::Iconst(h, 64 * 1024 + 7));
    let hv = f.v(Cls::I);
    f.i(Inst::Load(Mem::Int(1, false), hv, h, 0));
    let g = f.bin(IOp::Add, I64, e, hv);
    f.term(Term::Ret(vec![g]));
    let fu = f.done();
    // x = 0x1_0000_00F5: [p] = x, p[9] = 0xF5, q = copy of p, a = (i8)0xF5 = -11, b = p[8..10] = 0xF500, c = q[16..20]? (p + 3*4 + 4 = p + 16) = low 32 of x
    let xv: u64 = 0x1_0000_00F5;
    let want = (-11i64 as u64).wrapping_add(0xF500).wrapping_add(0xF5).wrapping_add(b'd' as u64);
    assert_eq!(run(&[fu], 0, &[xv]).unwrap(), vec![want]);
}

/// An atomic read-modify-write reads its address twice: an address computed just before it
/// (one counted use) must stay in its local (the tee shortcut once dropped the set: the
/// add went to address 0, P(DOOM)'s store_lock drop counter stayed 0).
#[test]
fn atomic_rmw_address_just_set() {
    let mut f = B::new(&[Cls::I], &[Cls::I]);
    let x = f.p(0);
    let s = f.f.slot(16, 8);
    let p = f.v(Cls::I);
    f.i(Inst::SlotAddr(p, s));
    f.i(Inst::Store(Mem::Int(8, false), p, 8, x));
    let a = f.v(Cls::I);
    f.i(Inst::IBinI(IOp::Add, I64, a, p, 8));
    let one = f.iconst(1);
    let old = f.v(Cls::I);
    f.i(Inst::AtomicRmw(RmwOp::Add, Mem::Int(8, false), AtomOrd::SeqCst, old, a, one));
    let r = f.v(Cls::I);
    f.i(Inst::Load(Mem::Int(8, false), r, p, 8));
    let sum = f.bin(IOp::Add, I64, r, old);
    f.term(Term::Ret(vec![sum]));
    // [p+8] = 5, old = 5, new = 6: 11
    assert_eq!(run(&[f.done()], 0, &[5]).unwrap(), vec![11]);
}

/// Direct, recursive, indirect, host and untraced calls; multiple results.
#[test]
fn calls() {
    // f0(n) = n < 2 ? n : f0(n-1) + f0(n-2)
    let mut f = B::new(&[Cls::I], &[Cls::I]);
    let n = f.p(0);
    let rec = f.blk();
    let base = f.blk();
    let c = f.v(Cls::I);
    f.i(Inst::ICmpI(Cond::Lt, true, c, n, 2));
    f.term(Term::Branch(c, base, rec));
    f.at(base);
    f.term(Term::Ret(vec![n]));
    f.at(rec);
    let a = f.v(Cls::I);
    f.i(Inst::IBinI(IOp::Sub, I64, a, n, 1));
    let r1 = f.v(Cls::I);
    f.i(Inst::Call(Callee::Fn(0), vec![a], vec![r1]));
    let b = f.v(Cls::I);
    f.i(Inst::IBinI(IOp::Sub, I64, b, n, 2));
    let r2 = f.v(Cls::I);
    f.i(Inst::Call(Callee::Fn(0), vec![b], vec![r2]));
    let s = f.bin(IOp::Add, I64, r1, r2);
    f.term(Term::Ret(vec![s]));
    let fib = f.done();
    // f1(x) -> (x + 1, x * 2)
    let mut f = B::new(&[Cls::I], &[Cls::I, Cls::I]);
    let x = f.p(0);
    let a = f.v(Cls::I);
    f.i(Inst::IBinI(IOp::Add, I64, a, x, 1));
    let b = f.v(Cls::I);
    f.i(Inst::IBinI(IOp::Mul, I64, b, x, 2));
    f.term(Term::Ret(vec![a, b]));
    let pair = f.done();
    // f2(x) = let (a, b) = f1(x); fnptr(f0)(a) + host add7(b)
    let mut f = B::new(&[Cls::I], &[Cls::I]);
    let x = f.p(0);
    let (a, b) = (f.v(Cls::I), f.v(Cls::I));
    f.i(Inst::Call(Callee::Fn(1), vec![x], vec![a, b]));
    let fp = f.v(Cls::I);
    f.i(Inst::FnAddr(fp, 0));
    let r = f.v(Cls::I);
    f.i(Inst::Call(Callee::Indirect(fp), vec![a], vec![r]));
    let h = f.v(Cls::I);
    f.i(Inst::Call(Callee::Host(Sym::new(1)), vec![b], vec![h]));
    let s = f.bin(IOp::Add, I64, r, h);
    f.term(Term::Ret(vec![s]));
    let main = f.done();
    // f3(): calls slot 99 (untraced)
    let mut f = B::new(&[], &[Cls::I]);
    let r = f.v(Cls::I);
    f.i(Inst::Call(Callee::Fn(99), vec![], vec![r]));
    f.term(Term::Ret(vec![r]));
    let un = f.done();
    let fs = [fib, pair, main, un];
    let fibr = |n: u64| -> u64 {
        let (mut a, mut b) = (0u64, 1u64);
        for _ in 0..n {
            let t = a + b;
            a = b;
            b = t;
        }
        a
    };
    assert_eq!(run(&fs, 0, &[20]).unwrap(), vec![fibr(20)]);
    assert_eq!(run(&fs, 2, &[9]).unwrap(), vec![fibr(10) + 18 + 7]);
    assert_eq!(run(&fs, 3, &[]), Err(Trap::Trap("untraced 99".into())));
    // without the import the untraced call is a bare trap
    let (b, st) = build(&fs, Options { untraced_import: false, ..opts() });
    assert_eq!(st.untraced_sites, 1);
    let mut i = inst(&b);
    assert_eq!(i.call_export("f3", &[]), Err(Trap::Trap("unreachable".into())));
}

/// Panics: Resume throws, Invoke catches into the cleanup pad, CatchPanic stops it, the shadow
/// stack is restored.
#[test]
fn unwinding() {
    // f0(x): if x == 0 { Resume } else ret x
    let mut f = B::new(&[Cls::I], &[Cls::I]);
    let x = f.p(0);
    let bad = f.blk();
    let ok = f.blk();
    f.term(Term::Branch(x, ok, bad));
    f.at(bad);
    f.term(Term::Resume);
    f.at(ok);
    f.term(Term::Ret(vec![x]));
    let thrower = f.done();
    // f1(p, x): with a slot; invoke f0(x): normal -> ret r; unwind -> store 77 at [p], resume
    let mut f = B::new(&[Cls::I, Cls::I], &[Cls::I]);
    let (p, x) = (f.p(0), f.p(1));
    let s = f.f.slot(16, 8);
    let sa = f.v(Cls::I);
    f.i(Inst::SlotAddr(sa, s));
    f.i(Inst::Store(Mem::Int(8, false), sa, 0, x));
    let r = f.v(Cls::I);
    f.i(Inst::Call(Callee::Fn(0), vec![x], vec![r]));
    let normal = f.blk();
    let pad = f.blk();
    f.term(Term::Invoke(normal, pad));
    f.at(normal);
    f.term(Term::Ret(vec![r]));
    f.at(pad);
    let k = f.iconst(77);
    f.i(Inst::Store(Mem::Int(8, false), p, 0, k));
    f.term(Term::Resume);
    let middle = f.done();
    // f2(p, x): catch_panic(f3, p) with f3 = |p| f1(p, x?) -- f3(p) calls f1(p, 0)
    let mut f = B::new(&[Cls::I], &[]);
    let p = f.p(0);
    let z = f.iconst(0);
    let r = f.v(Cls::I);
    f.i(Inst::Call(Callee::Fn(1), vec![p, z], vec![r]));
    f.term(Term::Ret(vec![]));
    let inner = f.done();
    let mut f = B::new(&[Cls::I], &[Cls::I]);
    let p = f.p(0);
    let fp = f.v(Cls::I);
    f.i(Inst::FnAddr(fp, 2));
    let r = f.v(Cls::I);
    f.i(Inst::Call(Callee::Host(Sym::new(3)), vec![fp, p], vec![r]));
    f.term(Term::Ret(vec![r]));
    let catcher = f.done();
    let fs = [thrower, middle, inner, catcher];
    let (b, _) = build(&fs, opts());
    let mut i = inst(&b);
    let sp0 = i.globals[0];
    assert_eq!(i.call_export("f1", &[1000, 5]).unwrap(), vec![5]);
    assert_eq!(i.globals[0], sp0);
    assert_eq!(i.call_export("f3", &[2000]).unwrap(), vec![1]);
    assert_eq!(&i.mem[2000..2008], &77u64.to_le_bytes());
    assert_eq!(i.globals[0], sp0, "the catch restores the shadow stack");
    assert_eq!(i.call_export("f1", &[1000, 0]), Err(Trap::Exception));
}

#[test]
fn vectors() {
    // f(p): v = load p; w = v + splat(3) (i32 lanes); store p+16; extract lane 2
    let mut f = B::new(&[Cls::I], &[Cls::I]);
    let p = f.p(0);
    let v = f.v(Cls::V128);
    f.i(Inst::VLoad(v, p, 0));
    let k = f.v(Cls::V128);
    f.i(Inst::VConst(k, 3));
    let w = f.v(Cls::V128);
    f.i(Inst::VX(VX::Add(VW::S), w, v, k));
    f.i(Inst::VStore(p, 16, w));
    let e = f.v(Cls::I);
    f.i(Inst::VExtract(e, w, 2));
    f.term(Term::Ret(vec![e]));
    let fu = f.done();
    let (b, _) = build(&[fu], opts());
    let mut i = inst(&b);
    for (k, x) in [1u32, 2, 0xFFFF_FFFE, 4].iter().enumerate() {
        i.mem[3000 + 4 * k..3004 + 4 * k].copy_from_slice(&x.to_le_bytes());
    }
    assert_eq!(i.call_export("f0", &[3000]).unwrap(), vec![1]);
    assert_eq!(&i.mem[3016..3020], &4u32.to_le_bytes());
}

// ---- random integer programs against a direct RIR evaluator

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn eval(f: &Func, args: &[u64], fuel: &mut u32) -> Option<u64> {
    let mut r = vec![0u64; f.vregs.len()];
    for (p, a) in f.params.iter().zip(args) {
        r[p.0 as usize] = *a;
    }
    let mut b = 0u32;
    loop {
        *fuel = fuel.checked_sub(1)?;
        for i in &f.blocks[b as usize].insts {
            match i {
                Inst::Iconst(d, k) => r[d.0 as usize] = *k as u64,
                Inst::Mov(d, a) => r[d.0 as usize] = r[a.0 as usize],
                Inst::IBin(op, it, d, a, x) => r[d.0 as usize] = ib(*op, *it, r[a.0 as usize], r[x.0 as usize]),
                Inst::IBinI(op, it, d, a, k) => r[d.0 as usize] = ib(*op, *it, r[a.0 as usize], *k as u64),
                Inst::ICmp(c, s, d, a, x) => r[d.0 as usize] = cmp(*c, *s, r[a.0 as usize], r[x.0 as usize]) as u64,
                Inst::Sel(d, c, a, x) => r[d.0 as usize] = if r[c.0 as usize] != 0 { r[a.0 as usize] } else { r[x.0 as usize] },
                _ => unreachable!(),
            }
        }
        match &f.blocks[b as usize].term {
            Term::Jump(t) => b = *t,
            Term::Branch(c, t, e) => b = if r[c.0 as usize] != 0 { *t } else { *e },
            Term::Ret(v) => return Some(r[v[0].0 as usize]),
            _ => unreachable!(),
        }
    }
}

fn ext(v: u64, it: IntTy) -> u64 {
    extend_const(v as i64, it) as u64
}

fn ib(op: IOp, it: IntTy, a: u64, b: u64) -> u64 {
    let r = match op {
        IOp::Add => a.wrapping_add(b),
        IOp::Sub => a.wrapping_sub(b),
        IOp::Mul => a.wrapping_mul(b),
        IOp::And => return a & b,
        IOp::Or => return a | b,
        IOp::Xor => return a ^ b,
        IOp::Shl if it.bits == 32 && !it.signed => ((a as u32) << (b & 31)) as u64,
        IOp::Shr if it.bits == 32 && !it.signed => ((a as u32) >> (b & 31)) as u64,
        IOp::Shl => a << (b & 63),
        IOp::Shr => {
            if it.signed {
                ((a as i64) >> (b & 63)) as u64
            } else {
                a >> (b & 63)
            }
        }
        _ => unreachable!(),
    };
    ext(r, it)
}

fn cmp(c: Cond, s: bool, a: u64, b: u64) -> bool {
    if s {
        let (a, b) = (a as i64, b as i64);
        match c {
            Cond::Eq => a == b,
            Cond::Ne => a != b,
            Cond::Lt => a < b,
            Cond::Le => a <= b,
            Cond::Gt => a > b,
            Cond::Ge => a >= b,
        }
    } else {
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

fn random_func(rng: &mut Rng) -> Func {
    let np = 1 + rng.below(3) as usize;
    let mut f = B::new(&vec![Cls::I; np], &[Cls::I]);
    let nv = 4 + rng.below(6) as usize;
    let mut vs: Vec<VReg> = f.f.params.clone();
    while vs.len() < nv {
        let v = f.iconst(rng.below(100) as i64 - 20);
        vs.push(v);
    }
    let nb = 2 + rng.below(7) as u32;
    let blocks: Vec<u32> = (0..nb).map(|k| if k == 0 { 0 } else { f.blk() }).collect();
    let tys = [I64, U64, I32, U32, U8, I8, I16];
    let ops = [IOp::Add, IOp::Sub, IOp::Mul, IOp::And, IOp::Or, IOp::Xor, IOp::Shl, IOp::Shr];
    let conds = [Cond::Eq, Cond::Ne, Cond::Lt, Cond::Le, Cond::Gt, Cond::Ge];
    for (k, &b) in blocks.iter().enumerate() {
        f.at(b);
        let n = rng.below(6);
        for _ in 0..n {
            fn pick(vs: &[VReg], rng: &mut Rng) -> VReg {
                vs[rng.below(vs.len() as u64) as usize]
            }
            // fresh single-use temporaries feed folding; reuse of old vregs feeds colouring
            let fresh = rng.below(3) != 0;
            let d = if fresh { f.v(Cls::I) } else { pick(&vs, rng) };
            let pick = |rng: &mut Rng| pick(&vs, rng);
            match rng.below(5) {
                0 => {
                    let (a, x) = (pick(rng), pick(rng));
                    f.i(Inst::IBin(ops[rng.below(8) as usize], tys[rng.below(7) as usize], d, a, x));
                }
                1 => {
                    let a = pick(rng);
                    f.i(Inst::IBinI(ops[rng.below(8) as usize], tys[rng.below(7) as usize], d, a, rng.below(40) as i64 - 8));
                }
                2 => {
                    let (a, x) = (pick(rng), pick(rng));
                    f.i(Inst::ICmp(conds[rng.below(6) as usize], rng.below(2) == 0, d, a, x));
                }
                3 => {
                    let (c, a, x) = (pick(rng), pick(rng), pick(rng));
                    f.i(Inst::Sel(d, c, a, x));
                }
                _ => {
                    let a = pick(rng);
                    f.i(Inst::Mov(d, a));
                }
            }
            if fresh {
                vs.push(d);
            }
        }
        let pick = |rng: &mut Rng| vs[rng.below(vs.len() as u64) as usize];
        let last = k as u32 + 1 == nb;
        let t = match rng.below(if last { 1 } else { 4 }) {
            0 => Term::Ret(vec![pick(rng)]),
            1 => Term::Jump(blocks[rng.below(nb as u64) as usize]),
            _ => {
                let c = pick(rng);
                Term::Branch(c, blocks[rng.below(nb as u64) as usize], blocks[rng.below(nb as u64) as usize])
            }
        };
        f.term(t);
    }
    f.done()
}

#[test]
fn random_programs_match_rir() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut checked = 0;
    let options = relax_session::Options::default();
    for _ in 0..3000 {
        let fu = random_func(&mut rng);
        fu.verify().unwrap();
        let args: Vec<u64> = (0..fu.params.len()).map(|_| rng.below(64).wrapping_sub(16)).collect();
        let mut fuel = 200;
        let want = eval(&fu, &args, &mut fuel);
        let mut m = Module::new(opts());
        m.add_func(0, &fu);
        m.export_func(0, "f");
        let out = m.finish(&options, &L);
        assert!(out.stats.failed.is_empty(), "{:?}\n{}", out.stats.failed, fu.dump());
        let mut i = inst(&out.bytes);
        i.max_steps = 200_000;
        let got = i.call_export("f", &args);
        match want {
            Some(w) => {
                assert_eq!(got, Ok(vec![w]), "args {:?}\n{}", args, fu.dump());
                checked += 1;
            }
            None => assert!(matches!(got, Err(Trap::Trap(ref m)) if m == "step limit") || got.is_ok(), "{:?}", got),
        }
    }
    assert!(checked > 1500, "only {} programs terminated", checked);
}

/// Size features: one deduplicated type per signature, no name section by default.
#[test]
fn size_basics() {
    let mk = || {
        let mut f = B::new(&[Cls::I, Cls::I], &[Cls::I]);
        let (x, y) = (f.p(0), f.p(1));
        let r = f.bin(IOp::Add, I64, x, y);
        f.term(Term::Ret(vec![r]));
        f.done()
    };
    let fs: Vec<Func> = (0..10).map(|_| mk()).collect();
    let (b, st) = build(&fs, opts());
    assert_eq!(st.n_types, 1);
    assert_eq!(st.names, 0);
    // a two-operand add whose operands are params: local.get 0, local.get 1, i64.add, end
    let mut i = inst(&b);
    assert_eq!(i.call_export("f3", &[40, 2]).unwrap(), vec![42]);
    let (_, st2) = build(&fs, Options { names: true, ..opts() });
    assert!(st2.names > 0);
}
