//! The domain-base read of a function that names domain statics sits in the nearest block that
//! dominates every use. A table dispatch (arm64::jump_tables) enters the case blocks straight from
//! the chain's head, past the chain's link blocks: a read placed in a link block was skipped by
//! the dispatch and the case read an unset frame slot (a warm-cached `match` in Stage's parser
//! faulted at a small address). The dispatch's edges are successors of the head.
use relax_codegen::regalloc;
use relax_codegen::rir::*;
use std::collections::HashMap;

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// b0 -> head (b1) testing x against 0..6 through link blocks b2..b6; case block of k is 7 + k;
/// the default is b13. Domain addresses are named in the case blocks of 3 and 5 only.
fn chain() -> (Func, u32, Vec<u32>, u32) {
    let mut f = Func::new("dom_table".to_string(), 0);
    let x = f.vreg(Cls::I);
    f.params = vec![x];
    let blocks: Vec<u32> = (0..14).map(|_| f.block()).collect();
    f.blocks[0].term = Term::Jump(blocks[1]);
    let heads = [1u32, 2, 3, 4, 5, 6];
    for (k, &b) in heads.iter().enumerate() {
        let c = f.vreg(Cls::I);
        push(&mut f, b, Inst::ICmpI(Cond::Eq, false, c, x, k as i64));
        let next = if k + 1 < heads.len() { heads[k + 1] } else { 13 };
        f.blocks[b as usize].term = Term::Branch(c, 7 + k as u32, next);
    }
    for k in 0..6u32 {
        let b = 7 + k;
        if k == 3 || k == 5 {
            let d = f.vreg(Cls::I);
            push(&mut f, b, Inst::Addr(d, Sym { id: 1, off: 0 }));
        }
        f.blocks[b as usize].term = Term::Ret(Vec::new());
    }
    f.blocks[13].term = Term::Ret(Vec::new());
    let targets: Vec<u32> = (0..6).map(|k| 7 + k).collect();
    (f, x.0, targets, 13)
}

#[test]
fn table_dispatch_edges_count_for_the_domain_base_block() {
    let (f, x, targets, default) = chain();
    // sym id 1 is a Domain symbol (class byte 3)
    static CLASS: [u8; 2] = [0, 3];
    let table = CLASS.as_ptr() as u64;
    let reach = regalloc::reachable(&f);
    // without the table the read lands in the link block that tests 3, which the dispatch skips
    let plain = regalloc::domain_base_block(&f, table, &reach, &HashMap::new()).unwrap();
    assert_eq!(plain, 4);
    // with it the head dominates the cases
    let mut plan = HashMap::new();
    plan.insert(1u32, (VReg(x), 0i64, targets, default));
    let with = regalloc::domain_base_block(&f, table, &reach, &plan).unwrap();
    assert_eq!(with, 1);
}
