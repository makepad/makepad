//! sink_consts: a constant whose uses are all on cold paths, in several blocks (the panic
//! location two bounds checks of a loop share), gets a copy in each of those blocks under a vreg
//! of its own and leaves the hot block; one with a use on a hot path stays where it is.
use relax_backend::passes;
use relax_backend::rir::*;

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// `fn(i: u64, n: u64) -> u64`: `loc = &SYM; if i >= n { panic(loc) } if i + 1 >= n { panic(loc) }
/// <hot use of loc>? ; i`.
fn build(hot_use: bool) -> Func {
    let mut f = Func::new("sink_cold_consts".into(), 0);
    let (i, n) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![i, n];
    f.rets = vec![Cls::I];
    let b: Vec<u32> = (0..5).map(|_| f.block()).collect();
    let (loc, c1, j, c2, r) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b[0], Inst::Addr(loc, Sym::new(9)));
    push(&mut f, b[0], Inst::ICmp(Cond::Ge, false, c1, i, n));
    f.blocks[b[0] as usize].term = Term::Branch(c1, b[1], b[2]);
    push(&mut f, b[1], Inst::Call(Callee::Fn(1), vec![loc], vec![]));
    f.blocks[b[1] as usize].term = Term::Unreachable;
    push(&mut f, b[2], Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: false }, j, i, 1));
    push(&mut f, b[2], Inst::ICmp(Cond::Ge, false, c2, j, n));
    f.blocks[b[2] as usize].term = Term::Branch(c2, b[3], b[4]);
    push(&mut f, b[3], Inst::Call(Callee::Fn(1), vec![loc], vec![]));
    f.blocks[b[3] as usize].term = Term::Unreachable;
    if hot_use {
        push(&mut f, b[4], Inst::Load(Mem::Int(8, false), r, loc, 0));
    } else {
        push(&mut f, b[4], Inst::Mov(r, i));
    }
    f.blocks[b[4] as usize].term = Term::Ret(vec![r]);
    f.verify().unwrap();
    f
}

/// (block, vreg) of each `Addr` of the symbol
fn addrs(f: &Func) -> Vec<(usize, VReg)> {
    let mut v = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            if let Inst::Addr(d, s) = i {
                if s.id == 9 {
                    v.push((bi, *d));
                }
            }
        }
    }
    v
}

#[test]
fn constant_used_on_cold_paths_only_is_copied_into_each() {
    let mut f = build(false);
    passes::sink_consts(&mut f);
    f.verify().unwrap();
    let a = addrs(&f);
    assert_eq!(a.iter().map(|x| x.0).collect::<Vec<_>>(), vec![1, 3], "{}", f.dump());
    assert_ne!(a[0].1, a[1].1, "one vreg defined twice");
    for (bi, d) in a {
        assert!(matches!(&f.blocks[bi].insts[1], Inst::Call(_, args, _) if args == &vec![d]), "{}", f.dump());
    }
}

#[test]
fn constant_with_a_hot_use_stays() {
    let mut f = build(true);
    passes::sink_consts(&mut f);
    f.verify().unwrap();
    assert_eq!(addrs(&f).iter().map(|x| x.0).collect::<Vec<_>>(), vec![0], "{}", f.dump());
}
