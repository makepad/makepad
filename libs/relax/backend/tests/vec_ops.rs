//! Lane-wise vector ops (`Inst::VX`, `Inst::VShuffle`) against a Rust reference of their RIR
//! meaning, every lane width, edge and random inputs (NaN payloads, -0, infinities, lane limits),
//! run on the host. Each op runs as the encoder emits it and forced through the vector legaliser
//! (`MAKEPAD_RUN_OPT=-vxnative`); on x86-64 also without AVX.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
type Entry = extern "sysv64" fn(*const u8, *const u8, *mut u8);
#[cfg(target_arch = "aarch64")]
type Entry = extern "C" fn(*const u8, *const u8, *mut u8);

#[cfg(target_arch = "x86_64")]
fn compile(options: &relax_session::Options, f: &Func) -> (Code, u32) {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
    let c = relax_backend::x64::compile(options, f, &env).unwrap();
    assert!(c.relocs.is_empty());
    (Code::new(&c.code).unwrap(), c.text_len)
}

#[cfg(target_arch = "x86_64")]
fn native(options: &relax_session::Options, op: VX) -> bool {
    relax_backend::x64::vx_native(options, op)
}

#[cfg(target_arch = "aarch64")]
fn compile(options: &relax_session::Options, f: &Func) -> (Code, u32) {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
    let c = relax_backend::arm64::compile(options, f, &env).unwrap();
    assert!(c.relocs.is_empty());
    (Code::new(&c.code).unwrap(), c.text_len)
}

#[cfg(target_arch = "aarch64")]
fn native(_: &relax_session::Options, op: VX) -> bool {
    relax_backend::arm64::vx_native(op)
}

/// The option sets every op runs under: (name, options).
fn modes() -> Vec<(&'static str, relax_session::Options)> {
    #[allow(unused_mut)]
    let mut native = relax_session::Options::default();
    #[cfg(target_arch = "x86_64")]
    {
        native.cpu = relax_backend::x64::detect_cpu(&native);
    }
    let mut legal = native.clone();
    legal.opt.push("-vxnative".into());
    #[allow(unused_mut)]
    let mut v = vec![("legalised", legal), ("native", native)];
    #[cfg(target_arch = "x86_64")]
    {
        let mut sse = relax_session::Options::default();
        sse.no_avx = true;
        sse.cpu = relax_backend::x64::detect_cpu(&sse);
        v.push(("sse", sse));
    }
    v
}

enum Body {
    Vx(VX),
    Shuffle([u8; 16]),
}

/// fn(a: *const [u8; 16], b: *const [u8; 16], d: *mut [u8; 16]) { *d = op(*a, *b) }
/// (a scalar result is stored as a u64)
fn func(body: &Body) -> Func {
    let mut f = Func::new("vec_op".into(), 0);
    let (pa, pb, pd) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![pa, pb, pd];
    f.rets = vec![];
    f.block();
    let (va, vb) = (f.vreg(Cls::V128), f.vreg(Cls::V128));
    let mut insts = vec![Inst::VLoad(va, pa, 0), Inst::VLoad(vb, pb, 0)];
    match *body {
        Body::Vx(op) => {
            let scalar = op.scalar_result();
            let d = f.vreg(if scalar { Cls::I } else { Cls::V128 });
            insts.push(Inst::VX(op, d, va, if op.arity() == 1 { va } else { vb }));
            insts.push(if scalar { Inst::Store(Mem::Int(8, false), pd, 0, d) } else { Inst::VStore(pd, 0, d) });
        }
        Body::Shuffle(idx) => {
            let d = f.vreg(Cls::V128);
            insts.push(Inst::VShuffle(d, va, vb, idx));
            insts.push(Inst::VStore(pd, 0, d));
        }
    }
    for i in insts {
        f.blocks[0].insts.push(i);
        f.blocks[0].pos.push(0);
    }
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    f
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn bits(w: VW) -> u32 {
    w.bits()
}
fn lanes(w: VW) -> usize {
    (128 / bits(w)) as usize
}
fn mask(w: VW) -> u64 {
    if bits(w) == 64 { u64::MAX } else { (1u64 << bits(w)) - 1 }
}
fn get(v: &[u8; 16], w: VW, k: usize) -> u64 {
    let n = (bits(w) / 8) as usize;
    let mut x = 0u64;
    for j in 0..n {
        x |= (v[k * n + j] as u64) << (8 * j);
    }
    x
}
fn put(v: &mut [u8; 16], w: VW, k: usize, x: u64) {
    let n = (bits(w) / 8) as usize;
    for j in 0..n {
        v[k * n + j] = (x >> (8 * j)) as u8;
    }
}
fn sext(w: VW, x: u64) -> i64 {
    let s = 64 - bits(w);
    ((x << s) as i64) >> s
}

/// Lane values worth testing: limits, small numbers and float specials of width `w`.
fn edges(w: VW) -> Vec<u64> {
    let m = mask(w);
    let mut v = vec![0, 1, 2, 3, 7, m, m - 1, m >> 1, (m >> 1) + 1, (m >> 1) - 1, (m >> 1) + 2];
    match w {
        VW::S => {
            for f in [0.0f32, -0.0, 1.0, -1.0, 0.5, -0.5, 1.5, 2.5, -2.5, 0.49999997, 3.0e9, -3.0e9, 2147483520.0, 2147483648.0, -2147483648.0, -2147483904.0, f32::INFINITY, f32::NEG_INFINITY, f32::MAX, f32::MIN, f32::MIN_POSITIVE, 1.0e-40, 16777217.0] {
                v.push(f.to_bits() as u64);
            }
            for nan in [0x7fc0_0000u64, 0xffc0_0000, 0x7f80_0001, 0xff81_2345, 0x7fc1_2345, 0x7fbf_ffff] {
                v.push(nan);
            }
        }
        VW::D => {
            for f in [0.0f64, -0.0, 1.0, -1.0, 0.5, -0.5, 1.5, 2.5, -2.5, 0.49999999999999994, 1.0e19, -1.0e19, 9223372036854774784.0, 9223372036854775808.0, -9223372036854775808.0, 3.0e38, 1.0e-310, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, f64::MIN, f64::MIN_POSITIVE, 4503599627370497.0, 3.4028235677973366e38, 1.0e-46] {
                v.push(f.to_bits());
            }
            for nan in [0x7ff8_0000_0000_0000u64, 0xfff8_0000_0000_0000, 0x7ff0_0000_0000_0001, 0xfff4_0000_1234_5678, 0x7ff8_0000_1234_5678] {
                v.push(nan);
            }
        }
        _ => {}
    }
    v
}

/// Input pairs for width `w`: every lane from the edge set or random bits.
fn inputs(w: VW, rng: &mut Rng, n: usize) -> Vec<([u8; 16], [u8; 16])> {
    let e = edges(w);
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let mut ab = [[0u8; 16]; 2];
        for v in ab.iter_mut() {
            for k in 0..lanes(w) {
                let r = rng.next();
                let x = if r % 3 == 0 { rng.next() & mask(w) } else { e[(r >> 8) as usize % e.len()] };
                put(v, w, k, x);
            }
        }
        out.push((ab[0], ab[1]));
    }
    out
}

/// How a result lane is compared with the reference.
#[derive(Clone, Copy, PartialEq)]
enum Cmp {
    Exact,
    /// a NaN reference lane accepts any NaN (Rust leaves NaN payloads unspecified)
    NanAny,
    /// NanAny, and a zero accepts either sign (minNum/maxNum of equal zeros)
    NanZeroAny,
}

fn fl(w: VW, x: u64) -> f64 {
    if w == VW::D { f64::from_bits(x) } else { f32::from_bits(x as u32) as f64 }
}
fn is_nan(w: VW, x: u64) -> bool {
    fl(w, x).is_nan()
}
fn fbits(w: VW, x: f64) -> u64 {
    if w == VW::D { x.to_bits() } else { (x as f32).to_bits() as u64 }
}

fn cond(c: Cond, lt: bool, eq: bool) -> bool {
    match c {
        Cond::Eq => eq,
        Cond::Ne => !eq,
        Cond::Lt => lt,
        Cond::Le => lt || eq,
        Cond::Gt => !lt && !eq,
        Cond::Ge => !lt,
    }
}

/// The reference result lane of lane-wise `op` (lane bits in and out).
fn lane_ref(op: VX, w: VW, x: u64, y: u64) -> (u64, Cmp) {
    let m = mask(w);
    let (sx, sy) = (sext(w, x), sext(w, y));
    let b = bits(w) as i64;
    let (smin, smax) = (i64::MIN >> (64 - b), i64::MAX >> (64 - b));
    let all = |t: bool| if t { m } else { 0 };
    // float binary op in the lane's precision
    let f2 = |g32: fn(f32, f32) -> f32, g64: fn(f64, f64) -> f64| -> u64 {
        if w == VW::D { g64(f64::from_bits(x), f64::from_bits(y)).to_bits() } else { g32(f32::from_bits(x as u32), f32::from_bits(y as u32)).to_bits() as u64 }
    };
    let f1 = |g32: fn(f32) -> f32, g64: fn(f64) -> f64| -> u64 {
        if w == VW::D { g64(f64::from_bits(x)).to_bits() } else { g32(f32::from_bits(x as u32)).to_bits() as u64 }
    };
    let r = match op {
        VX::Add(_) => x.wrapping_add(y) & m,
        VX::Sub(_) => x.wrapping_sub(y) & m,
        VX::MulLo(_) => x.wrapping_mul(y) & m,
        VX::MinS(_) => if sx < sy { x } else { y },
        VX::MaxS(_) => if sx > sy { x } else { y },
        VX::MinU(_) => x.min(y),
        VX::MaxU(_) => x.max(y),
        VX::CmpEq(_) => all(x == y),
        VX::CmpGtS(_) => all(sx > sy),
        VX::ICmp(c, _) => all(cond(c, sx < sy, sx == sy)),
        VX::UCmp(c, _) => all(cond(c, x < y, x == y)),
        VX::AddSatU(_) => (x as u128 + y as u128).min(m as u128) as u64,
        VX::SubSatU(_) => x.saturating_sub(y),
        VX::AddSatS(_) => ((sx as i128 + sy as i128).clamp(smin as i128, smax as i128) as u64) & m,
        VX::SubSatS(_) => ((sx as i128 - sy as i128).clamp(smin as i128, smax as i128) as u64) & m,
        VX::AvgU(_) => (x + y + 1) >> 1,
        VX::And => x & y,
        VX::Or => x | y,
        VX::Xor => x ^ y,
        VX::AndNot => x & !y,
        VX::ShlV(_) => (x << y) & m,
        VX::ShrUV(_) => x >> y,
        VX::ShrSV(_) => (sx >> y) as u64 & m,
        VX::ShlI(_, k) => if k as i64 >= b { 0 } else { (x << k) & m },
        VX::ShrUI(_, k) => if k as i64 >= b { 0 } else { x >> k },
        VX::ShrSI(_, k) => (sx >> (k as i64).min(b - 1)) as u64 & m,
        VX::Neg(_) => x.wrapping_neg() & m,
        VX::Abs(_) => sx.unsigned_abs() & m,
        VX::FAdd(..) => return (f2(|a, b| a + b, |a, b| a + b), Cmp::NanAny),
        VX::FSub(..) => return (f2(|a, b| a - b, |a, b| a - b), Cmp::NanAny),
        VX::FMul(..) => return (f2(|a, b| a * b, |a, b| a * b), Cmp::NanAny),
        VX::FDiv(..) => return (f2(|a, b| a / b, |a, b| a / b), Cmp::NanAny),
        VX::FMinLt(_) => if fl(w, x) < fl(w, y) { x } else { y },
        VX::FMaxGt(_) => if fl(w, x) > fl(w, y) { x } else { y },
        VX::FMinNum(_) | VX::FMaxNum(_) => {
            let r = match (is_nan(w, x), is_nan(w, y)) {
                (true, true) => return (fbits(w, f64::NAN), Cmp::NanAny),
                (true, false) => y,
                (false, true) => x,
                _ if fl(w, x) == fl(w, y) => return (x, Cmp::NanZeroAny),
                _ => {
                    let lt = fl(w, x) < fl(w, y);
                    if lt == matches!(op, VX::FMinNum(_)) { x } else { y }
                }
            };
            return (r, Cmp::Exact);
        }
        VX::FCmp(c, _) => {
            let (a, b) = (fl(w, x), fl(w, y));
            all(match c {
                Cond::Eq => a == b,
                Cond::Ne => a != b,
                Cond::Lt => a < b,
                Cond::Le => a <= b,
                Cond::Gt => a > b,
                Cond::Ge => a >= b,
            })
        }
        VX::FNeg(_) => x ^ (1 << (b - 1)),
        VX::FAbs(_) => x & !(1 << (b - 1)),
        VX::FSqrt(..) => return (f1(f32::sqrt, f64::sqrt), Cmp::NanAny),
        VX::FRound(_, t) => {
            let r = match t {
                RoundTo::Floor => f1(f32::floor, f64::floor),
                RoundTo::Ceil => f1(f32::ceil, f64::ceil),
                RoundTo::Trunc => f1(f32::trunc, f64::trunc),
                RoundTo::Away => f1(f32::round, f64::round),
            };
            return (r, Cmp::NanAny);
        }
        VX::FToI(_) => if w == VW::D { f64::from_bits(x) as i64 as u64 } else { f32::from_bits(x as u32) as i32 as u32 as u64 },
        VX::IToF(_) => if w == VW::D { (sx as f64).to_bits() } else { (sx as i32 as f32).to_bits() as u64 },
        _ => unreachable!("{op:?}"),
    };
    (r, Cmp::Exact)
}

fn same(w: VW, got: u64, want: u64, how: Cmp) -> bool {
    if got == want {
        return true;
    }
    match how {
        Cmp::Exact => false,
        Cmp::NanAny => is_nan(w, want) && is_nan(w, got),
        Cmp::NanZeroAny => (is_nan(w, want) && is_nan(w, got)) || (fl(w, want) == 0.0 && fl(w, got) == 0.0),
    }
}

/// The reference result of `op` on whole registers (lane checks: `Cmp` per lane of width w).
fn reference(op: VX, a: &[u8; 16], b: &[u8; 16]) -> ([u8; 16], VW, Vec<Cmp>) {
    let mut d = [0u8; 16];
    match op {
        VX::Narrow(w) => {
            let h = match w {
                VW::H => VW::B,
                VW::S => VW::H,
                _ => VW::S,
            };
            let n = lanes(w);
            for j in 0..2 * n {
                let x = get(if j < n { a } else { b }, w, j % n);
                put(&mut d, h, j, x & mask(h));
            }
            (d, h, vec![Cmp::Exact; lanes(h)])
        }
        VX::FCvtLo => {
            for j in 0..2 {
                put(&mut d, VW::D, j, (f32::from_bits(get(a, VW::S, j) as u32) as f64).to_bits());
            }
            (d, VW::D, vec![Cmp::NanAny; 2])
        }
        VX::FCvtNarrow => {
            for j in 0..4 {
                let x = f64::from_bits(get(if j < 2 { a } else { b }, VW::D, j % 2));
                put(&mut d, VW::S, j, (x as f32).to_bits() as u64);
            }
            (d, VW::S, vec![Cmp::NanAny; 4])
        }
        VX::MoveMask(w) => {
            let mut r = 0u64;
            for j in 0..lanes(w) {
                r |= (get(a, w, j) >> (bits(w) - 1)) << j;
            }
            d[..8].copy_from_slice(&r.to_le_bytes());
            (d, VW::D, vec![Cmp::Exact; 2])
        }
        _ => {
            let w = width(op);
            let mut how = Vec::new();
            for k in 0..lanes(w) {
                let (x, y) = (get(a, w, k), get(b, w, k));
                let (r, c) = lane_ref(op, w, x, if op.arity() == 1 { x } else { y });
                put(&mut d, w, k, r);
                how.push(c);
            }
            (d, w, how)
        }
    }
}

fn width(op: VX) -> VW {
    match op {
        VX::And | VX::Or | VX::Xor | VX::AndNot => VW::D,
        VX::Add(w) | VX::Sub(w) | VX::MulLo(w) | VX::MinS(w) | VX::MaxS(w) | VX::MinU(w) | VX::MaxU(w) => w,
        VX::AddSatU(w) | VX::SubSatU(w) | VX::AddSatS(w) | VX::SubSatS(w) | VX::AvgU(w) => w,
        VX::ICmp(_, w) | VX::UCmp(_, w) | VX::CmpEq(w) | VX::CmpGtS(w) => w,
        VX::ShlV(w) | VX::ShrSV(w) | VX::ShrUV(w) | VX::ShlI(w, _) | VX::ShrUI(w, _) | VX::ShrSI(w, _) => w,
        VX::Neg(w) | VX::Abs(w) => w,
        VX::FAdd(w, _) | VX::FSub(w, _) | VX::FMul(w, _) | VX::FDiv(w, _) => w,
        VX::FMinLt(w) | VX::FMaxGt(w) | VX::FMinNum(w) | VX::FMaxNum(w) | VX::FCmp(_, w) => w,
        VX::FNeg(w) | VX::FAbs(w) | VX::FSqrt(w, _) | VX::FRound(w, _) | VX::FToI(w) | VX::IToF(w) => w,
        VX::Narrow(w) | VX::MoveMask(w) => w,
        VX::FCvtLo => VW::S,
        VX::FCvtNarrow => VW::D,
        _ => unreachable!("{op:?}"),
    }
}

fn run(code: &Code, a: &[u8; 16], b: &[u8; 16]) -> [u8; 16] {
    let f: Entry = unsafe { std::mem::transmute(code.ptr()) };
    let mut d = [0u8; 16];
    f(a.as_ptr(), b.as_ptr(), d.as_mut_ptr());
    d
}

fn hex(v: &[u8; 16]) -> String {
    v.iter().rev().map(|x| format!("{x:02x}")).collect()
}

/// Runs `op` under every mode over `n` input pairs; returns the failures (at most a few per op).
fn check(op: VX, n: usize, fails: &mut Vec<String>) {
    let f = func(&Body::Vx(op));
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (format!("{op:?}").len() as u64 * 0x1234_5679));
    let mut ins = inputs(width(op), &mut rng, n);
    // per-lane shift amounts are in 0..lane bits
    if let VX::ShlV(w) | VX::ShrSV(w) | VX::ShrUV(w) = op {
        for (_, b) in ins.iter_mut() {
            for k in 0..lanes(w) {
                let c = get(b, w, k) % bits(w) as u64;
                put(b, w, k, c);
            }
        }
    }
    let mut legal_len = 0;
    for (mode, options) in modes() {
        let (code, len) = compile(&options, &f);
        if mode == "legalised" {
            legal_len = len;
        }
        // a native op is not expanded (the expansion is per-lane scalar code: longer)
        if mode != "legalised" && native(&options, op) && relax_backend::legal::expandable(op) && legal_len > 0 {
            assert!(len < legal_len, "{op:?} [{mode}]: native code {len} bytes, legalised {legal_len}");
        }
        let mut bad = 0;
        for (a, b) in &ins {
            let got = run(&code, a, b);
            let (want, w, how) = reference(op, a, b);
            let ok = (0..how.len()).all(|k| same(w, get(&got, w, k), get(&want, w, k), how[k]));
            if !ok && bad < 3 {
                fails.push(format!("{op:?} [{mode}]: a {} b {}: got {} want {}", hex(a), hex(b), hex(&got), hex(&want)));
            }
            bad += !ok as usize;
        }
    }
}

const W: [VW; 4] = [VW::B, VW::H, VW::S, VW::D];
const F: [VW; 2] = [VW::S, VW::D];
const C: [Cond; 6] = [Cond::Eq, Cond::Ne, Cond::Lt, Cond::Le, Cond::Gt, Cond::Ge];

fn int_ops() -> Vec<VX> {
    let mut v = vec![VX::And, VX::Or, VX::Xor, VX::AndNot];
    for w in W {
        v.extend([VX::Add(w), VX::Sub(w), VX::MulLo(w), VX::MinS(w), VX::MaxS(w), VX::MinU(w), VX::MaxU(w)]);
        v.extend([VX::AddSatU(w), VX::SubSatU(w), VX::AddSatS(w), VX::SubSatS(w), VX::CmpEq(w), VX::CmpGtS(w)]);
        v.extend([VX::Neg(w), VX::Abs(w), VX::ShlV(w), VX::ShrSV(w), VX::ShrUV(w)]);
        for c in C {
            v.extend([VX::ICmp(c, w), VX::UCmp(c, w)]);
        }
        let b = w.bits() as u8;
        for k in [0, 1, b / 2 + 1, b - 1, b, b + 3] {
            v.extend([VX::ShlI(w, k), VX::ShrUI(w, k), VX::ShrSI(w, k)]);
        }
        v.push(VX::MoveMask(w));
    }
    v.extend([VX::AvgU(VW::B), VX::AvgU(VW::H)]);
    v.extend([VX::Narrow(VW::H), VX::Narrow(VW::S), VX::Narrow(VW::D)]);
    v
}

fn float_ops() -> Vec<VX> {
    let mut v = vec![VX::FCvtLo, VX::FCvtNarrow];
    for w in F {
        let h = NanBits::Host;
        v.extend([VX::FAdd(w, h), VX::FSub(w, h), VX::FMul(w, h), VX::FDiv(w, h), VX::FSqrt(w, h)]);
        v.extend([VX::FMinLt(w), VX::FMaxGt(w), VX::FMinNum(w), VX::FMaxNum(w), VX::FNeg(w), VX::FAbs(w)]);
        v.extend([VX::FToI(w), VX::IToF(w)]);
        for t in [RoundTo::Floor, RoundTo::Ceil, RoundTo::Trunc, RoundTo::Away] {
            v.push(VX::FRound(w, t));
        }
        for c in C {
            v.push(VX::FCmp(c, w));
        }
    }
    v
}

fn report(fails: Vec<String>) {
    assert!(fails.is_empty(), "{} failures:\n{}", fails.len(), fails.join("\n"));
}

#[test]
fn integer_vector_ops_match_the_reference() {
    let mut fails = Vec::new();
    for op in int_ops() {
        check(op, 300, &mut fails);
    }
    report(fails);
}

#[test]
fn float_vector_ops_match_the_reference() {
    let mut fails = Vec::new();
    for op in float_ops() {
        check(op, 400, &mut fails);
    }
    report(fails);
}

#[test]
fn shuffles_match_the_reference() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut fails = Vec::new();
    let mut idxs: Vec<[u8; 16]> = vec![
        std::array::from_fn(|i| i as u8),
        std::array::from_fn(|i| 31 - i as u8),
        std::array::from_fn(|i| if i % 2 == 0 { (i / 2) as u8 } else { 16 + (i / 2) as u8 }),
        std::array::from_fn(|i| (i as u8 + 4) % 16),
        std::array::from_fn(|i| if i < 8 { i as u8 } else { 0x80 }),
    ];
    for _ in 0..12 {
        idxs.push(std::array::from_fn(|_| {
            let r = rng.next();
            if r % 7 == 0 { 32 + (r >> 8) as u8 % 200 } else { (r >> 8) as u8 % 32 }
        }));
    }
    let ins = inputs(VW::B, &mut rng, 20);
    for idx in idxs {
        let f = func(&Body::Shuffle(idx));
        for (mode, options) in modes() {
            let (code, _) = compile(&options, &f);
            for (a, b) in &ins {
                let got = run(&code, a, b);
                let want: [u8; 16] = std::array::from_fn(|i| match idx[i] {
                    j @ 0..=15 => a[j as usize],
                    j @ 16..=31 => b[j as usize - 16],
                    _ => 0,
                });
                if got != want {
                    fails.push(format!("VShuffle {idx:?} [{mode}]: got {} want {}", hex(&got), hex(&want)));
                    break;
                }
            }
        }
    }
    report(fails);
}
