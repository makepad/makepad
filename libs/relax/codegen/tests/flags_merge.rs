//! A branch on `x == 0` at the start of a block reuses the flags of the ALU op that just wrote
//! x only when that op is in the same block: a block start is a jump target, and a predecessor
//! jumping in brings other flags. (shader_ir::ty's O3 code: the merge block of
//! HashMap::get's two results branched on the `add` of the found path while its None path
//! jumped in with ZF=0, and loaded through the null.)
#![cfg(target_arch = "x86_64")]
use relax_codegen::rir::*;
use relax_codegen::x64;

const U64: IntTy = IntTy { bits: 64, signed: false };

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// f(x, take_add) = { v = take_add ? x + 12 : 0; if v == 0 { 111 } else { v } }, laid out so the
/// `add` block falls through into the branch-only merge block and the other path jumps in after
/// an `add` that leaves ZF clear.
fn merge_fn() -> Func {
    let mut f = Func::new("flags_merge".to_string(), 0);
    let (x, flag) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![x, flag];
    f.rets = vec![Cls::I];
    let (b0, b1, b2, b3, b4, b5) = (f.block(), f.block(), f.block(), f.block(), f.block(), f.block());
    let (z, c0, w, v, c, k) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b0, Inst::Iconst(z, 0));
    push(&mut f, b0, Inst::ICmpI(Cond::Ne, false, c0, flag, 0));
    f.blocks[b0 as usize].term = Term::Branch(c0, b2, b1);
    // the None path: an add with a nonzero result (ZF clear), then v = 0 by a move
    push(&mut f, b1, Inst::IBinI(IOp::Add, U64, w, x, 1));
    push(&mut f, b1, Inst::Mov(v, z));
    f.blocks[b1 as usize].term = Term::Jump(b3);
    // the found path: v = x + 12, falling through into the merge block
    push(&mut f, b2, Inst::IBinI(IOp::Add, U64, v, x, 12));
    f.blocks[b2 as usize].term = Term::Jump(b3);
    push(&mut f, b3, Inst::ICmpI(Cond::Eq, false, c, v, 0));
    f.blocks[b3 as usize].term = Term::Branch(c, b4, b5);
    push(&mut f, b4, Inst::Iconst(k, 111));
    f.blocks[b4 as usize].term = Term::Ret(vec![k]);
    f.blocks[b5 as usize].term = Term::Ret(vec![v]);
    let _ = w;
    f.verify().unwrap();
    f
}

#[test]
fn branch_at_block_start_compares_again() {
    let f = merge_fn();
    let env = x64::Env { slots: relax_codegen::rir::SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: false, syms: 0, sym_class: 0 };
    let c = x64::compile(&relax_session::Options::default(), &f, &env).unwrap();
    assert!(c.relocs.is_empty());
    let code = relax_codegen::exec::Code::new(&c.code).unwrap();
    let run: extern "sysv64" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
    // the None path: v == 0 must take the branch whatever the flags it jumped in with
    assert_eq!(run(5, 0), 111);
    assert_eq!(run(5, 1), 17);
    assert_eq!(run(u64::MAX - 11, 1), 111);
}
