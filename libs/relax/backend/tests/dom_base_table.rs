//! The domain-base reads of a function that names domain statics dominate every use and sit on
//! the paths that use it. A jump table (`Term::Switch`) enters its case blocks straight from the
//! dispatching block: the table's targets are its successors, so each case that names a domain
//! static reads the base itself and no read sits in a block the other cases skip (a warm-cached
//! `match` in Stage's parser once faulted at a small address because a read sat in a skipped link
//! block).
use relax_backend::regalloc;
use relax_backend::rir::*;
use std::sync::Arc;

#[test]
fn switch_edges_count_for_the_domain_base_block() {
    let mut f = Func::new("dom_table".to_string(), 0);
    let x = f.vreg(Cls::I);
    f.params = vec![x];
    // b0 -> b1 (the dispatch) -> case blocks 2..8 (case k is b(2 + k)), default b8
    let blocks: Vec<u32> = (0..9).map(|_| f.block()).collect();
    f.blocks[0].term = Term::Jump(blocks[1]);
    let targets: Vec<u32> = (0..6).map(|k| 2 + k).collect();
    f.blocks[1].term = Term::Switch(x, Arc::new(SwitchTable::new(0, targets, 8)));
    for k in 0..6u32 {
        let b = 2 + k;
        if k == 3 || k == 5 {
            let d = f.vreg(Cls::I);
            f.blocks[b as usize].insts.push(Inst::Addr(d, Sym { id: 1, off: 0 }));
            f.blocks[b as usize].pos.push(0);
        }
        f.blocks[b as usize].term = Term::Ret(Vec::new());
    }
    f.blocks[8].term = Term::Ret(Vec::new());
    // sym id 1 is a Domain symbol (class byte 3)
    static CLASS: [u8; 2] = [0, 3];
    let reach = regalloc::reachable(&f);
    assert!(reach[5] && reach[7]);
    assert_eq!(regalloc::domain_base_blocks(&f, CLASS.as_ptr() as u64, &reach), vec![5, 7]);
}
