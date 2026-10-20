//! Common-subexpression elimination keeps `Iconst 0` available from a definition of a multi-def vreg
//! only while nothing redefines that vreg on a path to the use. A join below a loop (reached from
//! outside and from the loop header, so its dominator is the function entry) sits in a cycle with
//! the later redefinition of the vreg: the redefinition reaches the join again on the next iteration,
//! so the constant must not be read from the vreg there (async `for` loop awaiting a nested future:
//! a coroutine flag was stored from the loop's accumulator).
use relax_backend::rir::*;
use relax_backend::passes;

#[test]
fn zero_is_not_read_from_a_vreg_the_loop_redefines() {
    let mut f = Func::new("cse_loop_join".into(), 0);
    let (p, q, ptr) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![p, q, ptr];
    f.rets = vec![Cls::I];
    let b: Vec<u32> = (0..8).map(|_| f.block()).collect();
    let (v, z, y) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    let push = |f: &mut Func, bi: u32, i: Inst| {
        f.blocks[bi as usize].insts.push(i);
        f.blocks[bi as usize].pos.push(0);
    };
    // b0: v = 0; p ? loop entry : the other way into the join
    push(&mut f, b[0], Inst::Iconst(v, 0));
    f.blocks[0].term = Term::Branch(p, b[1], b[3]);
    f.blocks[1].term = Term::Jump(b[2]);
    // b2: loop header: q ? body : exit
    f.blocks[2].term = Term::Branch(q, b[4], b[7]);
    f.blocks[3].term = Term::Jump(b[5]);
    f.blocks[4].term = Term::Jump(b[5]);
    // b5: the join: store a fresh zero
    push(&mut f, b[5], Inst::Iconst(z, 0));
    push(&mut f, b[5], Inst::Store(Mem::Int(1, false), ptr, 0, z));
    f.blocks[5].term = Term::Jump(b[6]);
    // b6: redefine v with a loaded value, back to the header
    push(&mut f, b[6], Inst::Load(Mem::Int(8, false), y, ptr, 8));
    push(&mut f, b[6], Inst::Mov(v, y));
    f.blocks[6].term = Term::Jump(b[2]);
    f.blocks[7].term = Term::Ret(vec![v]);
    f.verify().unwrap();
    passes::simplify(&relax_session::Options::default(), &mut f, None, true, true);
    for blk in &f.blocks {
        for i in &blk.insts {
            match i {
                Inst::Store(_, _, _, val) => assert_ne!(*val, v, "the zero store reads the redefined vreg"),
                Inst::Mov(_, src) => assert_ne!(*src, v, "a zero was copied from the redefined vreg"),
                _ => {}
            }
        }
    }
}
