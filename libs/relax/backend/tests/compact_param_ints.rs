//! `passes::compact_vregs` renames the declared parameter types (`Func::param_ints`) with their
//! vregs: a stale number names another parameter, and the wasm backend narrows a declared
//! <= 32-bit parameter to i32 (P(DOOM)'s size-first export: a closure's u64 LiveId parameter
//! took the declared u32 of the pointer before it and lost its high half).
use relax_backend::passes;
use relax_backend::rir::*;

#[test]
fn declared_parameter_types_follow_their_renamed_vregs() {
    let mut f = Func::new("compact_param_ints".into(), 0);
    let env = f.vreg(Cls::I);
    let _gone = f.vreg(Cls::I);
    let vm = f.vreg(Cls::I);
    let _gone2 = f.vreg(Cls::I);
    let key = f.vreg(Cls::I);
    f.params = vec![env, vm, key];
    f.param_ints = vec![(env, 4, false), (vm, 4, false), (key, 8, false)];
    f.block();
    let d = f.vreg(Cls::I);
    f.blocks[0].insts.push(Inst::IBin(IOp::Or, IntTy { bits: 64, signed: false }, d, vm, key));
    f.blocks[0].pos.push(0);
    f.blocks[0].term = Term::Ret(vec![]);
    passes::compact_vregs(&mut f);
    assert_eq!(f.params, vec![VReg(0), VReg(1), VReg(2)]);
    let declared: Vec<(VReg, u8)> = f.param_ints.iter().map(|(v, n, _)| (*v, *n)).collect();
    assert_eq!(declared, vec![(VReg(0), 4), (VReg(1), 4), (VReg(2), 8)]);
}
