//! Execute raw RIR so every select path is tested independently of if-conversion.
#![cfg(target_arch = "x86_64")]
use relax_backend::{exec::Code, rir::*, x64};

fn push(f: &mut Func, i: Inst) {
    f.blocks[0].insts.push(i);
    f.blocks[0].pos.push(0);
}

fn compile(w: bool, cond: Option<Cond>, swap_cmp: bool, swap_select: bool, shared: u8, alias: u8, generic: u8, regs: bool) -> Code {
    let cls = if w { Cls::F64 } else { Cls::F32 };
    let mut f = Func::new("float_select".into(), 0);
    let a = f.vreg(cls);
    let b = f.vreg(cls);
    let flag = f.vreg(Cls::I);
    f.params = vec![a, b, flag];
    f.rets = vec![cls];
    f.block();
    let c = f.vreg(Cls::I);
    let (x, y) = if swap_cmp { (b, a) } else { (a, b) };
    if let Some(cc) = cond {
        push(&mut f, Inst::FCmp(cc, w, c, x, y));
    } else if generic == 1 {
        push(&mut f, Inst::ICmpI(Cond::Lt, true, c, flag, 0));
    } else if generic == 2 {
        push(&mut f, Inst::ICmp(Cond::Lt, true, c, flag, flag));
    } else {
        push(&mut f, Inst::Mov(c, flag));
    }
    if shared == 2 {
        let gap = f.vreg(Cls::I);
        push(&mut f, Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: false }, gap, flag, 1));
    }
    let d = match alias {
        1 => a,
        2 => b,
        _ => f.vreg(cls),
    };
    let (t, e) = if swap_select { (b, a) } else { (a, b) };
    push(&mut f, Inst::Sel(d, c, t, e));
    if shared != 0 {
        // A second use keeps the materialized compare alive.
        let extra = f.vreg(Cls::I);
        push(&mut f, Inst::Sel(extra, c, flag, c));
        f.rets.push(Cls::I);
        f.blocks[0].term = Term::Ret(vec![d, extra]);
    } else {
        f.blocks[0].term = Term::Ret(vec![d]);
    }
    f.verify().unwrap();
    let env = x64::Env {
        slots: relax_backend::rir::SlotsPtr::NONE,
        tls_key: 0,
        tls_slow: 0,
        resume: 0,
        opt_regs: regs,
        syms: 0,
        sym_class: 0,
        near: false,
    };
    let c = x64::compile(&relax_session::Options::default(), &f, &env).unwrap();
    assert!(c.relocs.is_empty());
    Code::new(&c.code).unwrap()
}

fn compare(c: Cond, a: f64, b: f64) -> bool {
    match c {
        Cond::Gt => a > b,
        Cond::Ge => a >= b,
        Cond::Lt => a < b,
        Cond::Le => a <= b,
        Cond::Eq => a == b,
        Cond::Ne => a != b,
    }
}

#[test]
fn scalar_select_bits_and_aliases() {
    let values = [0.0, -0.0, 1.0, -1.0, f64::INFINITY, f64::NEG_INFINITY, f64::from_bits(0x7ff8000000001234), -f64::NAN];
    for w in [false, true] {
        for regs in [false, true] {
            for cc in [Cond::Gt, Cond::Ge, Cond::Lt, Cond::Le, Cond::Eq, Cond::Ne] {
                for swap_cmp in [false, true] {
                    for swap_select in [false, true] {
                        for shared in 0..3 {
                            for alias in 0..3 {
                                let code = compile(w, Some(cc), swap_cmp, swap_select, shared, alias, 0, regs);
                                for a in values {
                                    for b in values {
                                        let (x, y) = if swap_cmp { (b, a) } else { (a, b) };
                                        let take = compare(cc, x, y) != swap_select;
                                        if w {
                                            let run: extern "sysv64" fn(f64, f64, i64) -> f64 = unsafe { std::mem::transmute(code.ptr()) };
                                            assert_eq!(
                                                run(a, b, 7).to_bits(),
                                                (if take { a } else { b }).to_bits(),
                                                "{cc:?} {swap_cmp} {swap_select} {shared} {alias} {regs}"
                                            );
                                        } else {
                                            let run: extern "sysv64" fn(f32, f32, i64) -> f32 = unsafe { std::mem::transmute(code.ptr()) };
                                            assert_eq!(
                                                run(a as f32, b as f32, 7).to_bits(),
                                                (if take { a as f32 } else { b as f32 }).to_bits(),
                                                "{cc:?} {swap_cmp} {swap_select} {shared} {alias} {regs}"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn generic_and_integer_compare_selects() {
    for w in [false, true] {
        for regs in [false, true] {
            for generic in 0..3 {
                for alias in 0..3 {
                    let code = compile(w, None, false, false, 0, alias, generic, regs);
                    for flag in [-7i64, 0, 1] {
                        let take = match generic {
                            1 => flag < 0,
                            2 => false,
                            _ => flag != 0,
                        };
                        if w {
                            let run: extern "sysv64" fn(f64, f64, i64) -> f64 = unsafe { std::mem::transmute(code.ptr()) };
                            assert_eq!(run(-0.0, f64::NAN, flag).to_bits(), (if take { -0.0f64 } else { f64::NAN }).to_bits());
                        } else {
                            let run: extern "sysv64" fn(f32, f32, i64) -> f32 = unsafe { std::mem::transmute(code.ptr()) };
                            assert_eq!(run(-0.0, f32::NAN, flag).to_bits(), (if take { -0.0f32 } else { f32::NAN }).to_bits());
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn unrelated_float_compare_and_redefined_input() {
    for cc in [Cond::Gt, Cond::Ge, Cond::Lt, Cond::Le, Cond::Eq, Cond::Ne] {
        for redefine in [false, true] {
            let mut f = Func::new("unrelated_select".into(), 0);
            let a = f.vreg(Cls::F64);
            let b = f.vreg(Cls::F64);
            let x = f.vreg(Cls::F64);
            let y = f.vreg(Cls::F64);
            let c = f.vreg(Cls::I);
            let d = f.vreg(Cls::F64);
            f.params = vec![a, b, x, y];
            f.rets = vec![Cls::F64];
            f.block();
            push(&mut f, Inst::FCmp(cc, true, c, if redefine { a } else { x }, if redefine { b } else { y }));
            if redefine {
                push(&mut f, Inst::Fconst(a, 0, true));
            }
            push(&mut f, Inst::Sel(d, c, a, b));
            f.blocks[0].term = Term::Ret(vec![d]);
            f.verify().unwrap();
            let env = x64::Env {
                slots: relax_backend::rir::SlotsPtr::NONE,
                tls_key: 0,
                tls_slow: 0,
                resume: 0,
                opt_regs: true,
                syms: 0,
                sym_class: 0,
                near: false,
            };
            let compiled = x64::compile(&relax_session::Options::default(), &f, &env).unwrap();
            let code = Code::new(&compiled.code).unwrap();
            let run: extern "sysv64" fn(f64, f64, f64, f64) -> f64 = unsafe { std::mem::transmute(code.ptr()) };
            for x in [0.0, -0.0, 1.0, f64::NAN] {
                for y in [0.0, -1.0, 1.0, f64::NAN] {
                    let expected = if redefine {
                        if compare(cc, 2.0, 1.0) {
                            0.0f64
                        } else {
                            1.0
                        }
                    } else if compare(cc, x, y) {
                        2.0
                    } else {
                        1.0
                    };
                    assert_eq!(run(2.0, 1.0, x, y).to_bits(), expected.to_bits());
                }
            }
        }
    }
}

#[test]
fn f64_compare_selects_f32() {
    let mut f = Func::new("mixed_width_select".into(), 0);
    let a = f.vreg(Cls::F32);
    let b = f.vreg(Cls::F32);
    let x = f.vreg(Cls::F64);
    let y = f.vreg(Cls::F64);
    let c = f.vreg(Cls::I);
    let d = f.vreg(Cls::F32);
    f.params = vec![a, b, x, y];
    f.rets = vec![Cls::F32];
    f.block();
    push(&mut f, Inst::FCmp(Cond::Gt, true, c, x, y));
    push(&mut f, Inst::Sel(d, c, a, b));
    f.blocks[0].term = Term::Ret(vec![d]);
    f.verify().unwrap();
    let env = x64::Env {
        slots: relax_backend::rir::SlotsPtr::NONE,
        tls_key: 0,
        tls_slow: 0,
        resume: 0,
        opt_regs: true,
        syms: 0,
        sym_class: 0,
        near: false,
    };
    let compiled = x64::compile(&relax_session::Options::default(), &f, &env).unwrap();
    let code = Code::new(&compiled.code).unwrap();
    let run: extern "sysv64" fn(f32, f32, f64, f64) -> f32 = unsafe { std::mem::transmute(code.ptr()) };
    for (x, y) in [(2.0, 1.0), (1.0, 2.0), (f64::NAN, 0.0), (0.0, f64::NAN), (0.0, -0.0)] {
        assert_eq!(run(-0.0, f32::NAN, x, y).to_bits(), (if x > y { -0.0f32 } else { f32::NAN }).to_bits());
    }
}
