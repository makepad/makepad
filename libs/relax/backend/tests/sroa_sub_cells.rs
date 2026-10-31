//! SROA sub-word cells: a `Result<u64, (u32, u32)>`-like slot -- an 8-byte Ok word at offset 8 over the
//! Err payload's two u32 fields at 8 and 12, the variant picked at runtime, read back after a join --
//! becomes a register: the word as a 64-bit cell, the parts as shifts and masked merges. The promoted
//! code must compute the same results as the slot in memory (x64 and arm64).
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

const U64: IntTy = IntTy { bits: 64, signed: false };

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func) -> Vec<u8> {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile(&relax_session::Options::default(), f, &env).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

#[cfg(target_arch = "aarch64")]
fn compile(f: &Func) -> Vec<u8> {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::arm64::compile(&relax_session::Options::default(), f, &env).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// f(x, y): slot r (16 bytes); if x & 1 { r.tag = 0; r.ok = x * 3 (8 bytes at 8) }
/// else { r.tag = 1; r.e0 = y as u32 (4 at 8); r.e1 = (x >> 7) as u32 (4 at 12); r.b = 0x5a (1 at 13) }
/// join: if r.tag == 0 { r.ok + 1 } else { r.e0 as u64 * 1000 + r.e1 (4 at 12, signed) as i64 + r.lo16 (2 at 8) }
fn build() -> Func {
    let mut f = Func::new("sroa_sub_cells".into(), 0);
    let (x, y) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![x, y];
    f.rets = vec![Cls::I];
    let sl = f.slot(16, 8);
    let v: Vec<VReg> = (0..24).map(|_| f.vreg(Cls::I)).collect();
    let b: Vec<u32> = (0..6).map(|_| f.block()).collect();
    push(&mut f, b[0], Inst::SlotAddr(v[0], sl));
    push(&mut f, b[0], Inst::IBinI(IOp::And, U64, v[1], x, 1));
    f.blocks[b[0] as usize].term = Term::Branch(v[1], b[1], b[2]);
    // Ok
    push(&mut f, b[1], Inst::Iconst(v[2], 0));
    push(&mut f, b[1], Inst::Store(Mem::Int(4, false), v[0], 0, v[2]));
    push(&mut f, b[1], Inst::IBinI(IOp::Mul, U64, v[3], x, 3));
    push(&mut f, b[1], Inst::Store(Mem::Int(8, false), v[0], 8, v[3]));
    f.blocks[b[1] as usize].term = Term::Jump(b[3]);
    // Err
    push(&mut f, b[2], Inst::Iconst(v[4], 1));
    push(&mut f, b[2], Inst::Store(Mem::Int(4, false), v[0], 0, v[4]));
    push(&mut f, b[2], Inst::Store(Mem::Int(4, false), v[0], 8, y));
    push(&mut f, b[2], Inst::IBinI(IOp::Shr, U64, v[5], x, 7));
    push(&mut f, b[2], Inst::Store(Mem::Int(4, false), v[0], 12, v[5]));
    push(&mut f, b[2], Inst::Iconst(v[6], 0x5a));
    push(&mut f, b[2], Inst::Store(Mem::Int(1, false), v[0], 13, v[6]));
    f.blocks[b[2] as usize].term = Term::Jump(b[3]);
    // join
    push(&mut f, b[3], Inst::Load(Mem::Int(4, false), v[7], v[0], 0));
    push(&mut f, b[3], Inst::ICmpI(Cond::Eq, false, v[8], v[7], 0));
    f.blocks[b[3] as usize].term = Term::Branch(v[8], b[4], b[5]);
    push(&mut f, b[4], Inst::Load(Mem::Int(8, false), v[9], v[0], 8));
    push(&mut f, b[4], Inst::IBinI(IOp::Add, U64, v[10], v[9], 1));
    f.blocks[b[4] as usize].term = Term::Ret(vec![v[10]]);
    push(&mut f, b[5], Inst::Load(Mem::Int(4, false), v[11], v[0], 8));
    push(&mut f, b[5], Inst::IBinI(IOp::Mul, U64, v[12], v[11], 1000));
    push(&mut f, b[5], Inst::Load(Mem::Int(4, true), v[13], v[0], 12));
    push(&mut f, b[5], Inst::IBin(IOp::Add, U64, v[14], v[12], v[13]));
    push(&mut f, b[5], Inst::Load(Mem::Int(2, false), v[15], v[0], 8));
    push(&mut f, b[5], Inst::IBin(IOp::Add, U64, v[16], v[14], v[15]));
    f.blocks[b[5] as usize].term = Term::Ret(vec![v[16]]);
    f.verify().unwrap();
    f
}

fn slot_accesses(f: &Func) -> usize {
    f.blocks.iter().flat_map(|b| b.insts.iter()).filter(|i| matches!(i, Inst::Load(..) | Inst::Store(..))).count()
}

#[test]
fn overlapping_variant_payloads_promote_and_agree() {
    let mut g = build();
    assert!(relax_backend::passes::sroa(&mut g), "sroa promoted nothing");
    relax_backend::passes::dce(&mut g);
    g.verify().unwrap();
    assert_eq!(slot_accesses(&g), 0, "the slot is still accessed:\n{}", g.dump());
    let codes = [Code::new(&compile(&build())).unwrap(), Code::new(&compile(&g)).unwrap()];
    for (x, y) in [(1u64, 2u64), (6, 0xdead_beef), (0x1234_5678_9abc, 77), (u64::MAX, 3), (0xffff_ff00, 0xffff_ffff), (2, 0x8000_0001)] {
        let r: Vec<u64> = codes.iter().map(|c| {
            let run: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(c.ptr()) };
            run(x, y)
        }).collect();
        assert_eq!(r[0], r[1], "x {:#x} y {:#x}", x, y);
    }
}
