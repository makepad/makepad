//! Selects after a compare read its flags (arm64: `cmp; csel` without materialising the bool). A
//! select of another bool in between tests that bool (`cmp x, #0`), which overwrites the flags: a
//! later select of the first compare must not read them. Shape: three compares of one value, each
//! read by two selects in interleaved order (a Splash kernel's tan once its repeated compares were
//! value-numbered: tan(-1) returned -1).
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
type Entry = extern "sysv64" fn(i64, i64, i64, i64, i64, *mut i64);
#[cfg(target_arch = "aarch64")]
type Entry = extern "C" fn(i64, i64, i64, i64, i64, *mut i64);

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

#[test]
fn a_select_of_another_bool_ends_the_compare_chain() {
    let mut f = Func::new("sel_chain".into(), 0);
    let ps: Vec<VReg> = (0..6).map(|_| f.vreg(Cls::I)).collect();
    f.params = ps.clone();
    let (v, a, b, c, e, out) = (ps[0], ps[1], ps[2], ps[3], ps[4], ps[5]);
    f.block();
    let r: Vec<VReg> = (0..9).map(|_| f.vreg(Cls::I)).collect();
    let (c2, s1, c1, s2, c0, s3, t1, t2, t3) = (r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7], r[8]);
    let insts = vec![
        Inst::ICmpI(Cond::Eq, true, c2, v, 2),
        Inst::Sel(s1, c2, a, b),
        Inst::ICmpI(Cond::Eq, true, c1, v, 1),
        Inst::Sel(s2, c1, c, s1),
        Inst::ICmpI(Cond::Eq, true, c0, v, 0),
        Inst::Sel(s3, c0, e, s2),
        Inst::Sel(t1, c2, b, e),
        Inst::Sel(t2, c1, a, t1),
        Inst::Sel(t3, c0, c, t2),
        Inst::Store(Mem::Int(8, false), out, 0, s3),
        Inst::Store(Mem::Int(8, false), out, 8, t3),
    ];
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    let code = compile(&f);
    let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
    let (a, b, c, e) = (10i64, 20, 30, 40);
    for v in -1..4i64 {
        let mut o = [0i64; 2];
        run(v, a, b, c, e, o.as_mut_ptr());
        let s1 = if v == 2 { a } else { b };
        let s2 = if v == 1 { c } else { s1 };
        let s3 = if v == 0 { e } else { s2 };
        let t1 = if v == 2 { b } else { e };
        let t2 = if v == 1 { a } else { t1 };
        let t3 = if v == 0 { c } else { t2 };
        assert_eq!(o, [s3, t3], "v = {v}");
    }
}
