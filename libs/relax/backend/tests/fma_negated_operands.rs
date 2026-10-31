//! `FMulAdd` / `VFma` with `neg_prod` / `neg_add` negate the operands `a` / `c` themselves:
//! d = (neg_prod ? -a : a) * b + (neg_add ? -c : c), one rounding, so a NaN operand's sign flips
//! as in `(-a).mul_add(b, c)`. x86-64's vfnmadd/vfmsub negate after the multiply (a NaN kept its
//! sign); the encoder must give those bits for NaNs of both signs, quiet and signaling, one or
//! several at once (the NaN rule: rir::fma_nan_f32 / fma_nan_f64). The reference spells the NaN
//! out: Rust's own `mul_add` NaN is unspecified (rustc with the fma feature keeps a NaN `a`'s
//! sign in `(-a).mul_add(b, c)`, without it flips it).
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
        let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
        relax_backend::x64::compile(&options(), f, &env).unwrap()
    };
    #[cfg(target_arch = "aarch64")]
    let c = {
        let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
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
    let b = f64::from_bits;
    vec![0.0, -0.0, 1.0, -1.5, 3.25, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, -f64::NAN, b(0x7ff8_0000_0000_1234), b(0xfff8_0000_0000_4321), b(0x7ff0_0000_0000_0567), b(0xfff0_0000_0000_0089)]
}

fn values32() -> Vec<f32> {
    let b = f32::from_bits;
    vec![0.0, -0.0, 1.0, -1.5, 3.25, f32::INFINITY, f32::NEG_INFINITY, f32::NAN, -f32::NAN, b(0x7fc0_1234), b(0xffc0_4321), b(0x7f80_0567), b(0xff80_0089)]
}

/// The bits of the RIR op: the value of the fused op, or its NaN by the rule
fn want64(np: bool, na: bool, a: f64, b: f64, c: f64) -> u64 {
    let (a, c) = (if np { -a } else { a }, if na { -c } else { c });
    fma_nan_f64(a, b, c).unwrap_or_else(|| a.mul_add(b, c).to_bits())
}

fn want32(np: bool, na: bool, a: f32, b: f32, c: f32) -> u32 {
    let (a, c) = (if np { -a } else { a }, if na { -c } else { c });
    fma_nan_f32(a, b, c).unwrap_or_else(|| a.mul_add(b, c).to_bits())
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
                    assert_eq!(d.to_bits(), want64(np, na, a, b, c), "f64 np={np} na={na} ({:x}, {:x}, {:x})", a.to_bits(), b.to_bits(), c.to_bits());
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
                    assert_eq!(d.to_bits(), want32(np, na, a, b, c), "f32 np={np} na={na} ({:x}, {:x}, {:x})", a.to_bits(), b.to_bits(), c.to_bits());
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
                        assert_eq!(d[l].to_bits(), want32(np, na, a, b, c), "f32x4 np={np} na={na} ({:x}, {:x}, {:x})", a.to_bits(), b.to_bits(), c.to_bits());
                    }
                }
            }
        }
    }
}
