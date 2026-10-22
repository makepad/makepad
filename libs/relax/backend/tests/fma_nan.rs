//! The NaN of a fused multiply-add does not depend on the operand placement the back end picks:
//! `FMulAdd` / `VFma` give rir::fma_nan_f32 / fma_nan_f64 of the negated operands whether an
//! operand comes from memory (x64 folds a or c, never b) or which register the result shares
//! (x64: 231 with d = c, 213 with d = a, 132 with d = b; arm64: by-element fmla/fmls for a
//! splatted second factor, never a swapped first one), for every negation form and NaNs of both
//! signs, quiet and signaling, one or several at once.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
type Entry = extern "sysv64" fn(*const u8, *mut u8);
#[cfg(target_arch = "aarch64")]
type Entry = extern "C" fn(*const u8, *mut u8);

fn compile(f: &Func) -> Code {
    f.verify().unwrap();
    #[allow(unused_mut)]
    let mut o = relax_session::Options::default();
    #[cfg(target_arch = "x86_64")]
    let c = {
        o.cpu = relax_backend::x64::detect_cpu(&o);
        let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
        relax_backend::x64::compile(&o, f, &env).unwrap()
    };
    #[cfg(target_arch = "aarch64")]
    let c = {
        let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
        relax_backend::arm64::compile(&o, f, &env).unwrap()
    };
    assert!(c.relocs.is_empty());
    Code::new(&c.code).unwrap()
}

#[cfg(target_arch = "x86_64")]
fn has_fma() -> bool {
    let o = relax_session::Options::default();
    relax_backend::x64::detect_cpu(&o).fma
}
#[cfg(target_arch = "aarch64")]
fn has_fma() -> bool {
    true
}

const FORMS: [(bool, bool); 4] = [(false, false), (true, false), (false, true), (true, true)];

/// Where the operands come from and which of them outlive the op.
#[derive(Clone, Copy, Debug)]
enum Place {
    /// loaded a, b, c, the op right after c's load (x64: c from memory)
    MemC,
    /// loaded b, c, a (x64: a from memory)
    MemA,
    /// loaded a, c, b (b's load stays a load)
    MemB,
    /// all in registers; the one named dies at the op (its register can take d), the others
    /// are stored after it
    DiesC,
    DiesA,
    DiesB,
    /// all three live on (d in a register of its own)
    AllLive,
}

const PLACES: [Place; 7] = [Place::MemC, Place::MemA, Place::MemB, Place::DiesC, Place::DiesA, Place::DiesB, Place::AllLive];

/// fn(src: *const [a, b, c], dst: *mut [d, ...]) with one FMulAdd (scalar, w = f64) or one VFma
/// (vector: a, b, c and d are f32x4) placed as `p`.
fn build(p: Place, np: bool, na: bool, w: bool, vector: bool) -> Code {
    let (cls, mem, sz) = match (vector, w) {
        (true, _) => (Cls::V128, Mem::F32, 16),
        (false, true) => (Cls::F64, Mem::F64, 8),
        (false, false) => (Cls::F32, Mem::F32, 4),
    };
    let mut f = Func::new("fma_nan".into(), 0);
    let (ps, pd) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![ps, pd];
    f.block();
    let (a, b, c, d) = (f.vreg(cls), f.vreg(cls), f.vreg(cls), f.vreg(cls));
    let load = |v: VReg, k: i32| if vector { Inst::VLoad(v, ps, k * sz) } else { Inst::Load(mem, v, ps, k * sz) };
    let store = |v: VReg, k: i32| if vector { Inst::VStore(pd, k * sz, v) } else { Inst::Store(mem, pd, k * sz, v) };
    let op = if vector { Inst::VFma(np, na, d, a, b, c) } else { Inst::FMulAdd(np, na, w, d, a, b, c) };
    let mut insts = match p {
        Place::MemC => vec![load(a, 0), load(b, 1), load(c, 2)],
        Place::MemA => vec![load(b, 1), load(c, 2), load(a, 0)],
        Place::MemB => vec![load(a, 0), load(c, 2), load(b, 1)],
        _ => vec![load(a, 0), load(b, 1), load(c, 2)],
    };
    if matches!(p, Place::DiesC | Place::DiesA | Place::DiesB | Place::AllLive) {
        // something between the loads and the op: no load folds into it
        let z = f.vreg(Cls::I);
        insts.push(Inst::Iconst(z, 0));
        insts.push(Inst::Store(Mem::Int(8, false), pd, 4 * 16, z));
    }
    insts.push(op);
    insts.push(store(d, 0));
    let live: &[(VReg, i32)] = match p {
        Place::DiesC => &[(a, 1), (b, 2)],
        Place::DiesA => &[(b, 2), (c, 3)],
        Place::DiesB => &[(a, 1), (c, 3)],
        Place::AllLive => &[(a, 1), (b, 2), (c, 3)],
        _ => &[],
    };
    for &(v, k) in live {
        insts.push(store(v, k));
    }
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    compile(&f)
}

fn values64() -> Vec<f64> {
    let b = f64::from_bits;
    vec![1.0, -1.5, 0.0, f64::INFINITY, f64::NAN, -f64::NAN, b(0x7ff8_0000_0000_1234), b(0xfff8_0000_0000_4321), b(0x7ff0_0000_0000_0567), b(0xfff0_0000_0000_0089)]
}

fn values32() -> Vec<f32> {
    let b = f32::from_bits;
    vec![1.0, -1.5, 0.0, f32::INFINITY, f32::NAN, -f32::NAN, b(0x7fc0_1234), b(0xffc0_4321), b(0x7f80_0567), b(0xff80_0089)]
}

fn want64(np: bool, na: bool, a: f64, b: f64, c: f64) -> u64 {
    let (a, c) = (if np { -a } else { a }, if na { -c } else { c });
    fma_nan_f64(a, b, c).unwrap_or_else(|| a.mul_add(b, c).to_bits())
}

fn want32(np: bool, na: bool, a: f32, b: f32, c: f32) -> u32 {
    let (a, c) = (if np { -a } else { a }, if na { -c } else { c });
    fma_nan_f32(a, b, c).unwrap_or_else(|| a.mul_add(b, c).to_bits())
}

#[test]
fn the_rule_is_the_hosts_fused_op() {
    // the rule's own anchors (the hardware's answers, probed on Zen 5 and Apple M-series)
    let q = |x: u32| f32::from_bits(x);
    let (qa, qb, qc, sb) = (q(0x7fc0_000a), q(0x7fc0_000b), q(0x7fc0_000c), q(0x7f80_000b));
    #[cfg(target_arch = "x86_64")]
    {
        assert_eq!(fma_nan_f32(qa, qb, qc), Some(0x7fc0_000b)); // b first
        assert_eq!(fma_nan_f32(qa, 1.0, qc), Some(0x7fc0_000a)); // then a
        assert_eq!(fma_nan_f32(qa, sb, 1.0), Some(0x7fc0_000b)); // positional, sNaN no priority
        assert_eq!(fma_nan_f32(f32::INFINITY, 0.0, 1.0), Some(0xffc0_0000)); // negative default
        assert_eq!(fma_nan_f32(f32::INFINITY, 0.0, qc), Some(0x7fc0_000c));
    }
    #[cfg(target_arch = "aarch64")]
    {
        assert_eq!(fma_nan_f32(qa, qb, qc), Some(0x7fc0_000c)); // c first
        assert_eq!(fma_nan_f32(qa, qb, 1.0), Some(0x7fc0_000a)); // then a
        assert_eq!(fma_nan_f32(qa, sb, qc), Some(0x7fc0_000b)); // signaling first
        assert_eq!(fma_nan_f32(f32::INFINITY, 0.0, 1.0), Some(0x7fc0_0000)); // positive default
        assert_eq!(fma_nan_f32(f32::INFINITY, 0.0, qc), Some(0x7fc0_0000)); // inf * 0 + qNaN
    }
    assert_eq!(fma_nan_f32(1.0, 2.0, 3.0), None);
    assert_eq!(fma_nan_f32(-qa, 1.0, 1.0), Some(0xffc0_000a)); // a negated NaN keeps the flip
}

#[test]
fn scalar_fma_nans_whatever_the_placement() {
    if !has_fma() {
        return;
    }
    for p in PLACES {
        for (np, na) in FORMS {
            let code64 = build(p, np, na, true, false);
            let run64: Entry = unsafe { std::mem::transmute(code64.ptr()) };
            let vs = values64();
            for &a in &vs {
                for &b in &vs {
                    for &c in &vs {
                        let src = [a, b, c];
                        let mut d = [0f64; 16];
                        run64(src.as_ptr() as *const u8, d.as_mut_ptr() as *mut u8);
                        assert_eq!(d[0].to_bits(), want64(np, na, a, b, c), "f64 {p:?} np={np} na={na} ({:x}, {:x}, {:x})", a.to_bits(), b.to_bits(), c.to_bits());
                    }
                }
            }
            let code32 = build(p, np, na, false, false);
            let run32: Entry = unsafe { std::mem::transmute(code32.ptr()) };
            let vs = values32();
            for &a in &vs {
                for &b in &vs {
                    for &c in &vs {
                        let src = [a, b, c];
                        let mut d = [0f32; 32];
                        run32(src.as_ptr() as *const u8, d.as_mut_ptr() as *mut u8);
                        assert_eq!(d[0].to_bits(), want32(np, na, a, b, c), "f32 {p:?} np={np} na={na} ({:x}, {:x}, {:x})", a.to_bits(), b.to_bits(), c.to_bits());
                    }
                }
            }
        }
    }
}

#[test]
fn vector_fma_nans_whatever_the_placement() {
    if !has_fma() {
        return;
    }
    let vs = values32();
    for p in PLACES {
        for (np, na) in FORMS {
            let code = build(p, np, na, false, true);
            let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
            for &a in &vs {
                for &b in &vs {
                    // four lanes, four addends
                    for cs in vs.chunks(4) {
                        let mut c4 = [1f32; 4];
                        c4[..cs.len()].copy_from_slice(cs);
                        let mut src = [0f32; 12];
                        src[..4].copy_from_slice(&[a; 4]);
                        src[4..8].copy_from_slice(&[b; 4]);
                        src[8..].copy_from_slice(&c4);
                        let mut d = [0f32; 32];
                        run(src.as_ptr() as *const u8, d.as_mut_ptr() as *mut u8);
                        for l in 0..4 {
                            let c = c4[l];
                            assert_eq!(d[l].to_bits(), want32(np, na, a, b, c), "f32x4 {p:?} np={np} na={na} lane {l} ({:x}, {:x}, {:x})", a.to_bits(), b.to_bits(), c.to_bits());
                        }
                    }
                }
            }
        }
    }
}

/// fn(src: *const [a (f32x4), s (f32), c (f32x4)], dst): d0 = fma(a, splat(s), c), d1 = fma(splat(s), a, c)
/// (arm64 folds the second factor's splat into the by-element form; the first stays a dup)
fn build_splat(np: bool, na: bool) -> Code {
    let mut f = Func::new("fma_nan_splat".into(), 0);
    let (ps, pd) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![ps, pd];
    f.block();
    let s = f.vreg(Cls::F32);
    let (x, a, c, d0, d1) = (f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128));
    let insts = vec![
        Inst::Load(Mem::F32, s, ps, 16),
        Inst::VSplat(x, s),
        Inst::VLoad(a, ps, 0),
        Inst::VLoad(c, ps, 32),
        Inst::VFma(np, na, d0, a, x, c),
        Inst::VFma(np, na, d1, x, a, c),
        Inst::VStore(pd, 0, d0),
        Inst::VStore(pd, 16, d1),
    ];
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    compile(&f)
}

#[test]
fn splatted_factor_fma_nans() {
    if !has_fma() {
        return;
    }
    let vs = values32();
    for (np, na) in FORMS {
        let code = build_splat(np, na);
        let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
        for &s in &vs {
            for &a in &vs {
                for cs in vs.chunks(4) {
                    let mut c4 = [1f32; 4];
                    c4[..cs.len()].copy_from_slice(cs);
                    let mut src = [0f32; 12];
                    src[..4].copy_from_slice(&[a; 4]);
                    src[4] = s;
                    src[8..].copy_from_slice(&c4);
                    let mut d = [0f32; 8];
                    run(src.as_ptr() as *const u8, d.as_mut_ptr() as *mut u8);
                    for l in 0..4 {
                        let c = c4[l];
                        assert_eq!(d[l].to_bits(), want32(np, na, a, s, c), "fma(a, splat) np={np} na={na} lane {l} ({:x}, {:x}, {:x})", a.to_bits(), s.to_bits(), c.to_bits());
                        assert_eq!(d[4 + l].to_bits(), want32(np, na, s, a, c), "fma(splat, a) np={np} na={na} lane {l} ({:x}, {:x}, {:x})", s.to_bits(), a.to_bits(), c.to_bits());
                    }
                }
            }
        }
    }
}
