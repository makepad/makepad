//! `f32/f64::min/max` return the other operand when one is NaN. The AVX lowering (min/max, an
//! unordered compare of the second operand and a blend) must give the bits of the SSE lowering
//! (min/max, compare, and/andn/or) for every pair, NaNs of both signs and payloads, zeros and infinities;
//! and so must the constant fold (passes::fold_fbin) wherever one operand is a number: `0.0`
//! against `-0.0` gives the second operand in all three, so folded and run code agree. A constant
//! operand gives itself against an equal zero on either side (as rustc's code does).
#![cfg(target_arch = "x86_64")]
use relax_backend::{exec::Code, rir::*, x64};

fn build(op: FOp, w: bool, no_avx: bool) -> Code {
    let cls = if w { Cls::F64 } else { Cls::F32 };
    let mut f = Func::new("fminmax".into(), 0);
    let (a, b, d) = (f.vreg(cls), f.vreg(cls), f.vreg(cls));
    f.params = vec![a, b];
    f.rets = vec![cls];
    f.block();
    f.blocks[0].insts.push(Inst::FBin(op, w, d, a, b));
    f.blocks[0].pos.push(0);
    f.blocks[0].term = Term::Ret(vec![d]);
    f.verify().unwrap();
    let mut options = relax_session::Options::default();
    options.no_avx = no_avx;
    let env = x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = x64::compile(&options, &f, &env).unwrap();
    Code::new(&c.code).unwrap()
}

/// `op(c, x)` (`first`) or `op(x, c)` of a parameter `x` and a constant `c`
fn build_const(op: FOp, c: f64, first: bool, no_avx: bool) -> Code {
    let mut f = Func::new("fminmax_k".into(), 0);
    let (x, k, d) = (f.vreg(Cls::F64), f.vreg(Cls::F64), f.vreg(Cls::F64));
    f.params = vec![x];
    f.rets = vec![Cls::F64];
    f.block();
    f.blocks[0].insts.push(Inst::Fconst(k, c.to_bits(), true));
    f.blocks[0].insts.push(if first { Inst::FBin(op, true, d, k, x) } else { Inst::FBin(op, true, d, x, k) });
    f.blocks[0].pos = vec![0, 0];
    f.blocks[0].term = Term::Ret(vec![d]);
    f.verify().unwrap();
    let mut options = relax_session::Options::default();
    options.no_avx = no_avx;
    let env = x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = x64::compile(&options, &f, &env).unwrap();
    Code::new(&c.code).unwrap()
}

#[test]
fn a_constant_operand_gives_itself_against_an_equal_zero() {
    for op in [FOp::Min, FOp::Max] {
        for c in [0.0f64, -0.0] {
            for first in [true, false] {
                for no_avx in [false, true] {
                    let code = build_const(op, c, first, no_avx);
                    let g: extern "sysv64" fn(f64) -> f64 = unsafe { std::mem::transmute(code.ptr()) };
                    let r = g(-c);
                    assert_eq!(r.to_bits(), c.to_bits(), "{op:?} with the constant {c:?} {}: {r:?}", if first { "first" } else { "second" });
                    assert_eq!(g(f64::NAN).to_bits(), c.to_bits());
                    assert_eq!(g(1.0), if op == FOp::Min { c } else { 1.0 });
                }
            }
        }
    }
}

#[test]
fn avx_min_max_match_the_sse_lowering() {
    let f64s = [0.0, -0.0, 1.0, -1.0, 2.5, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, -f64::NAN, f64::from_bits(0x7ff0000000000001), f64::MAX, f64::MIN_POSITIVE, f64::from_bits(1)];
    let f32s: Vec<f32> = f64s.iter().map(|x| *x as f32).chain([f32::from_bits(0x7f800001), f32::from_bits(0xffc12345)]).collect();
    for op in [FOp::Min, FOp::Max] {
        let (a64, s64) = (build(op, true, false), build(op, true, true));
        let (ga, gs): (extern "sysv64" fn(f64, f64) -> f64, extern "sysv64" fn(f64, f64) -> f64) = unsafe { (std::mem::transmute(a64.ptr()), std::mem::transmute(s64.ptr())) };
        for &x in &f64s {
            for &y in &f64s {
                let (r, s) = (ga(x, y), gs(x, y));
                assert!(r.to_bits() == s.to_bits(), "f64 {op:?}({x:e}, {y:e}): avx {:#x} sse {:#x}", r.to_bits(), s.to_bits());
                if x.is_nan() && !y.is_nan() {
                    assert_eq!(r.to_bits(), y.to_bits());
                }
                if y.is_nan() && !x.is_nan() {
                    assert_eq!(r.to_bits(), x.to_bits());
                }
                if !(x.is_nan() && y.is_nan()) {
                    let k = relax_backend::passes::fold_fbin(op, true, x.to_bits(), y.to_bits());
                    assert!(k == r.to_bits(), "f64 {op:?}({x:e}, {y:e}): fold {k:#x} run {:#x}", r.to_bits());
                }
            }
        }
        let (a32, s32) = (build(op, false, false), build(op, false, true));
        let (ga, gs): (extern "sysv64" fn(f32, f32) -> f32, extern "sysv64" fn(f32, f32) -> f32) = unsafe { (std::mem::transmute(a32.ptr()), std::mem::transmute(s32.ptr())) };
        for &x in &f32s {
            for &y in &f32s {
                let (r, s) = (ga(x, y), gs(x, y));
                assert!(r.to_bits() == s.to_bits(), "f32 {op:?}({x:e}, {y:e}): avx {:#x} sse {:#x}", r.to_bits(), s.to_bits());
                if !(x.is_nan() && y.is_nan()) {
                    let k = relax_backend::passes::fold_fbin(op, false, x.to_bits() as u64, y.to_bits() as u64);
                    assert!(k == r.to_bits() as u64, "f32 {op:?}({x:e}, {y:e}): fold {k:#x} run {:#x}", r.to_bits());
                }
            }
        }
    }
}
