//! JITSAFE 19: with MAKEPAD_RUN_VERIFY_PASSES the backend verifies the RIR its internal rewrites
//! hand to the register allocator, and a broken function is an error naming the pass -- never
//! compiled.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::rir::*;

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func) -> Result<(), String> {
    let mut o = relax_session::Options::default();
    o.cpu = relax_backend::x64::detect_cpu(&o);
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    relax_backend::x64::compile(&o, f, &env).map(|_| ())
}

#[cfg(target_arch = "aarch64")]
fn compile(f: &Func) -> Result<(), String> {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    relax_backend::arm64::compile(&relax_session::Options::default(), f, &env).map(|_| ())
}

#[test]
fn broken_rir_never_reaches_the_allocator() {
    relax_backend::rir::force_verify_passes();
    // a function that returns one value but whose block returns none
    let mut f = Func::new("broken".into(), 0);
    let a = f.vreg(Cls::I);
    f.params = vec![a];
    f.rets = vec![Cls::I];
    f.block();
    f.blocks[0].term = Term::Ret(vec![]);
    assert!(f.verify().is_err(), "the fixture must be broken");
    let e = compile(&f).expect_err("broken RIR was compiled");
    assert!(e.contains("RIR broken by"), "{e}");
    // and a sound one still compiles
    let mut g = Func::new("ok".into(), 0);
    let b = g.vreg(Cls::I);
    g.params = vec![b];
    g.rets = vec![Cls::I];
    g.block();
    g.blocks[0].term = Term::Ret(vec![b]);
    compile(&g).unwrap();
}
