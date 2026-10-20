//! `if !(a <= b)` and friends: a compare that only feeds a negated branch condition branches on
//! the flags directly (no materialised bool), and NaN operands keep Rust's semantics: every
//! ordered compare is false on NaN so its negation is true; `!=` is true on NaN so its negation
//! is false.
#![cfg(target_arch = "x86_64")]
use relax_backend::{exec::Code, rir::*, x64};
use relax_backend::passes;

const CONDS: [Cond; 6] = [Cond::Lt, Cond::Le, Cond::Gt, Cond::Ge, Cond::Eq, Cond::Ne];

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// f(a, b) = if NOT(a cc b) { 11 } else { 22 }, the NOT spelt `form`: 0 `== 0`, 1 `^ 1`, 2 none
/// (a plain branch), 3 `!= 0`.
fn build(w: bool, cc: Cond, form: u8, order: bool) -> Func {
    let cls = if w { Cls::F64 } else { Cls::F32 };
    let mut f = Func::new("branch_not".into(), 0);
    let (a, b) = (f.vreg(cls), f.vreg(cls));
    f.params = vec![a, b];
    f.rets = vec![Cls::I];
    let (b0, b1, b2) = (f.block(), f.block(), f.block());
    let (c, d, k) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b0, Inst::FCmp(cc, w, c, a, b));
    let cond = match form {
        0 => {
            push(&mut f, b0, Inst::ICmpI(Cond::Eq, false, d, c, 0));
            d
        }
        1 => {
            push(&mut f, b0, Inst::IBinI(IOp::Xor, IntTy { bits: 8, signed: false }, d, c, 1));
            d
        }
        3 => {
            push(&mut f, b0, Inst::ICmpI(Cond::Ne, false, d, c, 0));
            d
        }
        _ => c,
    };
    // block order decides which successor falls through
    f.blocks[b0 as usize].term = if order { Term::Branch(cond, b1, b2) } else { Term::Branch(cond, b2, b1) };
    push(&mut f, b1, Inst::Iconst(k, 11));
    f.blocks[b1 as usize].term = Term::Ret(vec![k]);
    let k2 = f.vreg(Cls::I);
    push(&mut f, b2, Inst::Iconst(k2, 22));
    f.blocks[b2 as usize].term = Term::Ret(vec![k2]);
    f.verify().unwrap();
    f
}

fn truth(cc: Cond, a: f64, b: f64) -> bool {
    match cc {
        Cond::Lt => a < b,
        Cond::Le => a <= b,
        Cond::Gt => a > b,
        Cond::Ge => a >= b,
        Cond::Eq => a == b,
        Cond::Ne => a != b,
    }
}

#[test]
fn negated_compare_branches_on_the_flags() {
    let vals = [0.0, -0.0, 1.0, -1.0, 2.5, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, -f64::NAN];
    let options = relax_session::Options::default();
    for w in [false, true] {
        for cc in CONDS {
            for form in 0..4u8 {
                for order in [false, true] {
                    let mut f = build(w, cc, form, order);
                    passes::simplify(&options, &mut f, None, true, true);
                    if form != 2 {
                        // the negation is gone: the compare is the block's last instruction and
                        // the branch reads it
                        let b0 = &f.blocks[0];
                        assert!(matches!(b0.insts.last(), Some(Inst::FCmp(..))), "{w} {cc:?} {form}: {:?}", b0.insts);
                        assert!(b0.insts.iter().all(|i| !matches!(i, Inst::ICmpI(..) | Inst::IBinI(..))));
                    }
                    for regs in [false, true] {
                        let env = x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: regs, syms: 0, sym_class: 0 };
                        let c = x64::compile(&options, &f, &env).unwrap();
                        let code = Code::new(&c.code).unwrap();
                        for a in vals {
                            for b in vals {
                                let cmp = truth(cc, a, b);
                                let negated = form == 0 || form == 1;
                                let taken_b1 = if negated { !cmp } else { cmp } == order;
                                let want: u64 = if taken_b1 { 11 } else { 22 };
                                let got = if w {
                                    let run: extern "sysv64" fn(f64, f64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
                                    run(a, b)
                                } else {
                                    let run: extern "sysv64" fn(f32, f32) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
                                    run(a as f32, b as f32)
                                };
                                assert_eq!(got, want, "w={w} {cc:?} form={form} order={order} regs={regs} a={a} b={b}");
                            }
                        }
                    }
                }
            }
        }
    }
}
