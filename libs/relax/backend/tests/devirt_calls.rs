//! A call through a fn pointer read from immutable data at a constant address (a thread-local
//! key's getter, a vtable word of a known vtable) is a direct call of the function the word
//! names, when the embedder says the word's symbol is that function's callable address
//! (`RoReader::fn_slot`); every other indirect call stays one, as do all of them with
//! RELAX_OPT=-devirt. Optimiser only (`passes::simplify`).
use relax_backend::passes::{simplify, RoRead, RoReader, FN_QUERY};
use relax_backend::rir::*;

/// Data symbol 1 holds two pointer words: at 0 function slot 7's callable address (symbol 2), at 8
/// a pointer into data (symbol 3). Anything else is no immutable data.
fn read(_ctx: usize, s: Sym, n: u32) -> RoRead {
    match (s.id, s.off, n) {
        (2, 0, FN_QUERY) => RoRead::Fn(7),
        (_, _, FN_QUERY) => RoRead::No,
        (1, 0, 8) => RoRead::Ptr(Sym { id: 2, off: 0 }),
        (1, 8, 8) => RoRead::Ptr(Sym { id: 3, off: 0 }),
        _ => RoRead::No,
    }
}

/// `f(p) = (*(sym1 + off))() + p()`: an indirect call through the word at `off` of data symbol
/// 1, and one through the parameter.
fn caller(off: i32) -> Func {
    let mut f = Func::new("caller".into(), 0);
    let p = f.vreg(Cls::I);
    f.params = vec![p];
    f.rets = vec![Cls::I];
    let (a, w, x, y, s) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    let b = f.block();
    let insts = vec![
        Inst::Addr(a, Sym { id: 1, off: 0 }),
        Inst::Load(Mem::Int(8, false), w, a, off),
        Inst::Call(Callee::Indirect(w), vec![], vec![x]),
        Inst::Call(Callee::Indirect(p), vec![], vec![y]),
        Inst::IBin(IOp::Add, I64, s, x, y),
    ];
    let blk = &mut f.blocks[b as usize];
    blk.pos = vec![0; insts.len()];
    blk.insts = insts;
    blk.term = Term::Ret(vec![s]);
    f.verify().unwrap();
    f
}

/// The callees after simplify: `Some(slot)` a direct call, None an indirect one.
fn callees(options: &relax_session::Options, off: i32) -> Vec<Option<u32>> {
    let mut f = caller(off);
    simplify(options, &mut f, Some(RoReader::new(read, 0)), true, true);
    f.blocks.iter().flat_map(|b| &b.insts).filter_map(|i| match i {
        Inst::Call(Callee::Fn(id), ..) => Some(Some(*id)),
        Inst::Call(Callee::Indirect(_), ..) => Some(None),
        _ => None,
    }).collect()
}

#[test]
fn a_call_through_a_constant_fn_pointer_is_a_direct_call_of_its_function() {
    let plain = relax_session::Options::default();
    let mut off = relax_session::Options::default();
    off.opt.push("-devirt".into());
    assert_eq!(callees(&plain, 0), vec![Some(7), None], "the word naming slot 7: a call of slot 7; the parameter: indirect");
    assert_eq!(callees(&plain, 8), vec![None, None], "a word pointing into data is no function: indirect");
    assert_eq!(callees(&off, 0), vec![None, None], "RELAX_OPT=-devirt: indirect");
}
