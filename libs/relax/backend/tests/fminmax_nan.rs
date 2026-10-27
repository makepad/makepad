//! `f32/f64::min/max` return the other operand when one is NaN. The AVX lowering (min/max, an
//! unordered compare of the second operand and a blend) must give the bits of the SSE lowering
//! (min/max, compare, and/andn/or) for every pair, NaNs of both signs and payloads, zeros and infinities.
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
            }
        }
        let (a32, s32) = (build(op, false, false), build(op, false, true));
        let (ga, gs): (extern "sysv64" fn(f32, f32) -> f32, extern "sysv64" fn(f32, f32) -> f32) = unsafe { (std::mem::transmute(a32.ptr()), std::mem::transmute(s32.ptr())) };
        for &x in &f32s {
            for &y in &f32s {
                let (r, s) = (ga(x, y), gs(x, y));
                assert!(r.to_bits() == s.to_bits(), "f32 {op:?}({x:e}, {y:e}): avx {:#x} sse {:#x}", r.to_bits(), s.to_bits());
            }
        }
    }
}
