//! simplify on core::arch lane shuffles and vector loads: `_mm_shuffle_ps(a, a, m)` (two lane
//! shuffles of one vector and a 64-bit zip) becomes one lane shuffle; repeated vector loads of one
//! address with no store between them become one load (a by-reference vector argument read at
//! every use).
use relax_backend::passes;
use relax_backend::rir::*;

fn shuffles(k1: u8, k2: u8, same: bool) -> Func {
    let mut f = Func::new("shuffle_fold".into(), 0);
    let (pa, pb, pd) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![pa, pb, pd];
    f.block();
    let (a, b, x, y, d) = (f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128));
    let insts = vec![
        Inst::VLoad(a, pa, 0),
        Inst::VLoad(b, pb, 0),
        Inst::VX(VX::ShufS(k1), x, a, a),
        Inst::VX(VX::ShufS(k2), y, if same { a } else { b }, if same { a } else { b }),
        Inst::VX(VX::ZipLo(VW::D), d, x, y),
        Inst::VStore(pd, 0, d),
    ];
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    f
}

fn shufs(f: &Func) -> Vec<(u8, VReg)> {
    f.blocks[0].insts.iter().filter_map(|i| match i {
        Inst::VX(VX::ShufS(k), _, a, _) => Some((*k, *a)),
        _ => None,
    }).collect()
}

#[test]
fn a_shuffle_of_one_vector_is_one_lane_shuffle() {
    // _MM_SHUFFLE(3, 0, 2, 1) as std's _mm_shuffle_ps builds it: lanes 1, 2 by the low nibble,
    // 0, 3 by the high one
    const M: u8 = 0b11_00_10_01;
    let mut f = shuffles(M & 0x0f, M >> 4, true);
    passes::simplify(&relax_session::Options::default(), &mut f, None, true, true);
    f.verify().unwrap();
    assert_eq!(shufs(&f), vec![(M, VReg(3))]);
    assert!(!f.blocks[0].insts.iter().any(|i| matches!(i, Inst::VX(VX::ZipLo(_), ..))));
}

#[test]
fn a_shuffle_of_two_vectors_keeps_the_zip() {
    let mut f = shuffles(0b11_00_10_01, 0b00_01_10_11, false);
    passes::simplify(&relax_session::Options::default(), &mut f, None, true, true);
    f.verify().unwrap();
    assert!(f.blocks[0].insts.iter().any(|i| matches!(i, Inst::VX(VX::ZipLo(VW::D), ..))));
    assert_eq!(shufs(&f).len(), 2);
}

#[test]
fn repeated_vector_loads_are_one() {
    let mut f = Func::new("vload_cse".into(), 0);
    let (p, q, pd) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![p, q, pd];
    f.block();
    let (a, b, s, c, t) = (f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128));
    let insts = vec![
        Inst::VLoad(a, p, 0),
        Inst::VLoad(b, p, 0),
        Inst::VX(VX::FSub(VW::S, NanBits::Host), s, a, b),
        Inst::VStore(pd, 0, s),
        // after a store through another pointer the load stays
        Inst::VLoad(c, p, 0),
        Inst::VX(VX::FAdd(VW::S, NanBits::Host), t, c, s),
        Inst::VStore(q, 0, t),
    ];
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    passes::simplify(&relax_session::Options::default(), &mut f, None, true, true);
    passes::local_cse(&relax_session::Options::default(), &mut f);
    f.verify().unwrap();
    let loads = f.blocks[0].insts.iter().filter(|i| matches!(i, Inst::VLoad(..))).count();
    assert_eq!(loads, 2, "{}", f.dump());
}
