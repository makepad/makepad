//! Multiplies by a splatted f32 (`VSplat` of an F32 value into `VFma` / `VX(FMul)`): arm64 folds
//! them into the by-element forms (`fmla vd.4s, vn.4s, vm.s[0]`, arm64 fold_lane_splats); every
//! negation form and operand position against Rust's `mul_add` / `*`, run on the host.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
type Entry = extern "sysv64" fn(*const f32, *const f32, *const f32, *mut f32);
#[cfg(target_arch = "aarch64")]
type Entry = extern "C" fn(*const f32, *const f32, *const f32, *mut f32);

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func) -> Code {
    let mut options = relax_session::Options::default();
    options.cpu = relax_backend::x64::detect_cpu(&options);
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile(&options, f, &env).unwrap();
    Code::new(&c.code).unwrap()
}

#[cfg(target_arch = "aarch64")]
fn compile(f: &Func) -> Code {
    let options = relax_session::Options::default();
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::arm64::compile(&options, f, &env).unwrap();
    Code::new(&c.code).unwrap()
}

/// fn(a, s, c, d): x = splat(*s); d[0..4] = fma(a, x, c); d[4..8] = fma(x, a, c); d[8..12] = a * x;
/// d[12..16] = x * a (the fma negations np / na)
fn func(np: bool, na: bool, nb: NanBits) -> Func {
    let mut f = Func::new("lane_splat".into(), 0);
    let (pa, ps, pc, pd) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![pa, ps, pc, pd];
    f.rets = vec![];
    f.block();
    let s = f.vreg(Cls::F32);
    let (x, a, c) = (f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128));
    let (d0, d1, d2, d3) = (f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128));
    let insts = vec![
        Inst::Load(Mem::F32, s, ps, 0),
        Inst::VSplat(x, s),
        Inst::VLoad(a, pa, 0),
        Inst::VLoad(c, pc, 0),
        Inst::VFma(np, na, d0, a, x, c),
        Inst::VFma(np, na, d1, x, a, c),
        Inst::VX(VX::FMul(VW::S, nb), d2, a, x),
        Inst::VX(VX::FMul(VW::S, nb), d3, x, a),
        Inst::VStore(pd, 0, d0),
        Inst::VStore(pd, 16, d1),
        Inst::VStore(pd, 32, d2),
        Inst::VStore(pd, 48, d3),
    ];
    for i in insts {
        f.blocks[0].insts.push(i);
        f.blocks[0].pos.push(0);
    }
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    f
}

#[test]
fn splatted_multiplicands_match_lanewise_mul_add_and_mul() {
    let a = [1.5f32, -2.25, 3.0e-3, 7.0];
    let c = [0.5f32, 100.0, -0.0, 1.0e30];
    for s in [3.0f32, -0.75, 1.0e-20] {
        for (np, na) in [(false, false), (true, false), (false, true), (true, true)] {
            for nb in [NanBits::Host, NanBits::Neon] {
                let code = compile(&func(np, na, nb));
                let entry: Entry = unsafe { std::mem::transmute(code.ptr()) };
                let mut d = [0f32; 16];
                entry(a.as_ptr(), &s, c.as_ptr(), d.as_mut_ptr());
                for l in 0..4 {
                    let pa = if np { -a[l] } else { a[l] };
                    let ac = if na { -c[l] } else { c[l] };
                    let want = pa.mul_add(s, ac);
                    assert_eq!(d[l].to_bits(), want.to_bits(), "fma(a, x) lane {l} np {np} na {na} s {s}");
                    assert_eq!(d[4 + l].to_bits(), want.to_bits(), "fma(x, a) lane {l} np {np} na {na} s {s}");
                    assert_eq!(d[8 + l].to_bits(), (a[l] * s).to_bits(), "a * x lane {l}");
                    assert_eq!(d[12 + l].to_bits(), (s * a[l]).to_bits(), "x * a lane {l}");
                }
            }
        }
    }
}
