//! `FMulAdd` / `VFma` with `neg_prod` / `neg_add` negate the operands `a` / `c` themselves:
//! d = (neg_prod ? -a : a) * b + (neg_add ? -c : c), one rounding, so a NaN operand's sign flips
//! as in `(-a).mul_add(b, c)`. x86-64's vfnmadd/vfmsub negate after the multiply (a NaN kept its
//! sign); the encoder must give the bits of the Rust reference for NaNs of both signs too.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
type Entry = extern "sysv64" fn(*const u8, *mut u8);
#[cfg(target_arch = "aarch64")]
type Entry = extern "C" fn(*const u8, *mut u8);

fn options() -> relax_session::Options {
    #[allow(unused_mut)]
    let mut o = relax_session::Options::default();
    #[cfg(target_arch = "x86_64")]
    {
        o.cpu = relax_backend::x64::detect_cpu(&o);
    }
    o
}

fn compile(f: &Func) -> Code {
    f.verify().unwrap();
    #[cfg(target_arch = "x86_64")]
    let c = {
        let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
        relax_backend::x64::compile(&options(), f, &env).unwrap()
    };
    #[cfg(target_arch = "aarch64")]
    let c = {
        let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
        relax_backend::arm64::compile(&options(), f, &env).unwrap()
    };
    assert!(c.relocs.is_empty());
    Code::new(&c.code).unwrap()
}

/// fn(src: *const [a, b, c], dst: *mut d) with one scalar FMulAdd.
fn scalar(np: bool, na: bool, w: bool) -> Code {
    let (cls, mem, sz) = if w { (Cls::F64, Mem::F64, 8) } else { (Cls::F32, Mem::F32, 4) };
    let mut f = Func::new("fma".into(), 0);
    let (ps, pd) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![ps, pd];
    f.block();
    let (a, b, c, d) = (f.vreg(cls), f.vreg(cls), f.vreg(cls), f.vreg(cls));
    let insts = vec![Inst::Load(mem, a, ps, 0), Inst::Load(mem, b, ps, sz), Inst::Load(mem, c, ps, 2 * sz), Inst::FMulAdd(np, na, w, d, a, b, c), Inst::Store(mem, pd, 0, d)];
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    compile(&f)
}

/// fn(src: *const [a, b, c] (three f32x4), dst: *mut f32x4) with one VFma.
fn vector(np: bool, na: bool) -> Code {
    let mut f = Func::new("vfma".into(), 0);
    let (ps, pd) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![ps, pd];
    f.block();
    let (a, b, c, d) = (f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128));
    let insts = vec![Inst::VLoad(a, ps, 0), Inst::VLoad(b, ps, 16), Inst::VLoad(c, ps, 32), Inst::VFma(np, na, d, a, b, c), Inst::VStore(pd, 0, d)];
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    compile(&f)
}

fn values64() -> Vec<f64> {
    vec![0.0, -0.0, 1.0, -1.5, 3.25, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, -f64::NAN, f64::from_bits(0x7ff8_0000_0000_1234), f64::from_bits(0xfff8_0000_0000_4321)]
}

fn values32() -> Vec<f32> {
    vec![0.0, -0.0, 1.0, -1.5, 3.25, f32::INFINITY, f32::NEG_INFINITY, f32::NAN, -f32::NAN, f32::from_bits(0x7fc0_1234), f32::from_bits(0xffc0_4321)]
}

const FORMS: [(bool, bool); 4] = [(false, false), (true, false), (false, true), (true, true)];

#[test]
fn scalar_fma_negates_operands() {
    for (np, na) in FORMS {
        let code = scalar(np, na, true);
        let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
        let vs = values64();
        for &a in &vs {
            for &b in &vs {
                for &c in &vs {
                    let src = [a, b, c];
                    let mut d = 0f64;
                    run(src.as_ptr() as *const u8, &mut d as *mut f64 as *mut u8);
                    let want = (if np { -a } else { a }).mul_add(b, if na { -c } else { c });
                    // a NaN result: only its sign and quietness are fixed across ISAs when one
                    // input is NaN (the payload rule differs); compare all bits when the inputs
                    // have at most one NaN, the sign otherwise
                    let nans = [a, b, c].iter().filter(|x| x.is_nan()).count();
                    if nans <= 1 {
                        assert_eq!(d.to_bits(), want.to_bits(), "f64 np={np} na={na} ({a:e}, {b:e}, {c:e})");
                    } else {
                        assert!(d.is_nan() && want.is_nan(), "f64 np={np} na={na} ({a:e}, {b:e}, {c:e})");
                    }
                }
            }
        }
        let code = scalar(np, na, false);
        let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
        let vs = values32();
        for &a in &vs {
            for &b in &vs {
                for &c in &vs {
                    let src = [a, b, c];
                    let mut d = 0f32;
                    run(src.as_ptr() as *const u8, &mut d as *mut f32 as *mut u8);
                    let want = (if np { -a } else { a }).mul_add(b, if na { -c } else { c });
                    let nans = [a, b, c].iter().filter(|x| x.is_nan()).count();
                    if nans <= 1 {
                        assert_eq!(d.to_bits(), want.to_bits(), "f32 np={np} na={na} ({a:e}, {b:e}, {c:e})");
                    } else {
                        assert!(d.is_nan() && want.is_nan(), "f32 np={np} na={na} ({a:e}, {b:e}, {c:e})");
                    }
                }
            }
        }
    }
}

#[test]
fn vector_fma_negates_operands() {
    let vs = values32();
    for (np, na) in FORMS {
        let code = vector(np, na);
        let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
        for &a in &vs {
            for &b in &vs {
                // the four lanes take four addends
                for cs in vs.chunks(4) {
                    let mut c4 = [0f32; 4];
                    c4[..cs.len()].copy_from_slice(cs);
                    let mut src = [0f32; 12];
                    src[..4].copy_from_slice(&[a; 4]);
                    src[4..8].copy_from_slice(&[b; 4]);
                    src[8..].copy_from_slice(&c4);
                    let mut d = [0f32; 4];
                    run(src.as_ptr() as *const u8, d.as_mut_ptr() as *mut u8);
                    for l in 0..4 {
                        let c = c4[l];
                        let want = (if np { -a } else { a }).mul_add(b, if na { -c } else { c });
                        let nans = [a, b, c].iter().filter(|x| x.is_nan()).count();
                        if nans <= 1 {
                            assert_eq!(d[l].to_bits(), want.to_bits(), "f32x4 np={np} na={na} ({a:e}, {b:e}, {c:e})");
                        } else {
                            assert!(d[l].is_nan() && want.is_nan(), "f32x4 np={np} na={na} ({a:e}, {b:e}, {c:e})");
                        }
                    }
                }
            }
        }
    }
}
