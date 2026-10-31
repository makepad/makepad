//! A compare of a branch-free `Ordering` (`a < b ? -1 : (a != b)`, if-converted `Ord::cmp`),
//! maybe narrowed by `as u8` (a low mask) or an i8 conversion, against a constant: simplify folds it
//! into the tree of the compare's outcomes (constants, the compares or their negations), so a test
//! that cannot hold for any outcome (`Option<Ordering>`'s niche value) is a constant and no select
//! of the Ordering is left. Every form computes what the unfolded code does (x64 run).
use relax_backend::passes;
use relax_backend::rir::*;

const CONDS: [Cond; 6] = [Cond::Lt, Cond::Le, Cond::Gt, Cond::Ge, Cond::Eq, Cond::Ne];
const U64: IntTy = IntTy { bits: 64, signed: false };

fn push(f: &mut Func, i: Inst) {
    f.blocks[0].insts.push(i);
    f.blocks[0].pos.push(0);
}

/// f(a, b) = (narrow(a < b ? -1 : (a == b) ^ 1)) cc k; narrow 0 none, 1 `& 255`, 2 i8 conversion,
/// 3 i8 conversion then `& 255` (`Option<Ordering>` read as a byte)
fn build(cc: Cond, signed: bool, k: i64, narrow: u8) -> Func {
    let mut f = Func::new("ord_fold".into(), 0);
    let (a, b) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![a, b];
    f.rets = vec![Cls::I];
    f.block();
    let [lt, eq, ne, m1, ord, x, d, x8] = [(); 8].map(|_| f.vreg(Cls::I));
    push(&mut f, Inst::ICmp(Cond::Lt, false, lt, a, b));
    push(&mut f, Inst::ICmp(Cond::Eq, false, eq, a, b));
    push(&mut f, Inst::IBinI(IOp::Xor, U64, ne, eq, 1));
    push(&mut f, Inst::Iconst(m1, -1));
    push(&mut f, Inst::Sel(ord, lt, m1, ne));
    let x = match narrow {
        1 => {
            push(&mut f, Inst::IBinI(IOp::And, U64, x, ord, 255));
            x
        }
        2 => {
            push(&mut f, Inst::Conv(Conv::IntToInt(IntTy { bits: 8, signed: true }), x, ord));
            x
        }
        3 => {
            push(&mut f, Inst::Conv(Conv::IntToInt(IntTy { bits: 8, signed: true }), x8, ord));
            push(&mut f, Inst::IBinI(IOp::And, U64, x, x8, 255));
            x
        }
        _ => ord,
    };
    push(&mut f, Inst::ICmpI(cc, signed, d, x, k));
    f.blocks[0].term = Term::Ret(vec![d]);
    f.verify().unwrap();
    f
}

fn want(cc: Cond, signed: bool, k: i64, narrow: u8, a: u64, b: u64) -> u64 {
    let ord: i64 = if a < b { -1 } else { (a != b) as i64 };
    let x = match narrow {
        1 | 3 => ord & 255,
        2 => ord as i8 as i64,
        _ => ord,
    };
    let r = if signed {
        let (x, k) = (x, k);
        match cc {
            Cond::Lt => x < k,
            Cond::Le => x <= k,
            Cond::Gt => x > k,
            Cond::Ge => x >= k,
            Cond::Eq => x == k,
            Cond::Ne => x != k,
        }
    } else {
        let (x, k) = (x as u64, k as u64);
        match cc {
            Cond::Lt => x < k,
            Cond::Le => x <= k,
            Cond::Gt => x > k,
            Cond::Ge => x >= k,
            Cond::Eq => x == k,
            Cond::Ne => x != k,
        }
    };
    r as u64
}

fn sels_left(f: &Func) -> usize {
    f.blocks.iter().flat_map(|b| &b.insts).filter(|i| matches!(i, Inst::Sel(..))).count()
}

#[test]
fn ordering_niche_test_is_a_constant() {
    let options = relax_session::Options::default();
    for narrow in [1u8, 2, 3] {
        // (ord as u8) != 254: no outcome is 254
        let mut f = build(Cond::Ne, false, if narrow == 2 { -2 } else { 254 }, narrow);
        passes::simplify(&options, &mut f, None, true, true);
        let ret = match &f.blocks[0].term {
            Term::Ret(v) => v[0],
            _ => unreachable!(),
        };
        let konst = f.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(i, Inst::Iconst(d, 1) if *d == ret));
        assert!(konst, "narrow {}: {:?}", narrow, f.blocks[0].insts);
    }
}

#[test]
fn ordering_compares_fold_into_outcomes() {
    let options = relax_session::Options::default();
    for narrow in 0..4u8 {
        for cc in [Cond::Eq, Cond::Ne] {
            for k in [-1i64, 0, 1] {
                let k = if narrow == 1 || narrow == 3 { k & 255 } else { k };
                let mut f = build(cc, false, k, narrow);
                passes::simplify(&options, &mut f, None, true, true);
                // the Ordering's select is gone: at most the outcome select on `a < b`
                assert!(sels_left(&f) <= 1, "narrow {} {:?} {}: {:?}", narrow, cc, k, f.blocks[0].insts);
                assert!(!f.blocks[0].insts.iter().any(|i| matches!(i, Inst::Iconst(_, -1))), "narrow {} {:?} {}: {:?}", narrow, cc, k, f.blocks[0].insts);
            }
        }
    }
}

#[cfg(all(target_arch = "x86_64", unix))]
#[test]
fn folded_ordering_compares_compute_the_same() {
    use relax_backend::{exec::Code, x64};
    type Entry = extern "sysv64" fn(u64, u64) -> u64;
    let options = relax_session::Options::default();
    let vals = [0u64, 1, 2, 7, u64::MAX];
    for narrow in 0..4u8 {
        for cc in CONDS {
            for signed in [false, true] {
                for k in [-2i64, -1, 0, 1, 2, 254, 255] {
                    let mut f = build(cc, signed, k, narrow);
                    passes::simplify(&options, &mut f, None, true, true);
                    let env = x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
                    let c = x64::compile(&options, &f, &env).unwrap();
                    let code = Code::new(&c.code).unwrap();
                    let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
                    for a in vals {
                        for b in vals {
                            assert_eq!(run(a, b) & 1, want(cc, signed, k, narrow, a, b), "narrow {} {:?} signed {} k {} a {} b {}", narrow, cc, signed, k, a, b);
                        }
                    }
                }
            }
        }
    }
}
