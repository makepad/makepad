//! forward_slot_loads on a small array that is also read at a runtime index (the address plus an
//! index escapes the slot): `let c = [(i, j), (i + 1, j), ..]; .. c[0] .. c[k]`. Its element
//! stores at known offsets cannot write each other's bytes, so the constant-offset loads take the
//! stored values; a store at a runtime index, or through another pointer, in between keeps the load.
use relax_backend::rir::*;
use relax_codegen::opt;

const U64: IntTy = IntTy { bits: 64, signed: false };

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

#[derive(Clone, Copy, PartialEq)]
enum Between {
    Nothing,
    /// a store at a runtime index into the same slot
    IndexedStore,
    /// a store through the pointer parameter
    PointerStore,
}

/// b0: s = slot; s[0], s[8], s[16], s[24] = a, b, a + 1, b + 1; (between); x = s[0]; y = s[8];
///     z = s[k * 8] (the escape); ret x + y, z
fn array_reads(between: Between) -> Func {
    let mut f = Func::new("arr".into(), 0);
    let (a, b, k, p) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params.extend([a, b, k, p]);
    f.rets = vec![Cls::I, Cls::I];
    let b0 = f.block();
    let sl = f.slot(32, 8);
    let v: Vec<VReg> = (0..10).map(|_| f.vreg(Cls::I)).collect();
    let (s, a1, b1, s2, x, y, sh, ad, z, xy) = (v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8], v[9]);
    push(&mut f, b0, Inst::SlotAddr(s, sl));
    push(&mut f, b0, Inst::Store(Mem::Int(8, false), s, 0, a));
    push(&mut f, b0, Inst::Store(Mem::Int(8, false), s, 8, b));
    push(&mut f, b0, Inst::IBinI(IOp::Add, U64, a1, a, 1));
    push(&mut f, b0, Inst::Store(Mem::Int(8, false), s, 16, a1));
    push(&mut f, b0, Inst::IBinI(IOp::Add, U64, b1, b, 1));
    push(&mut f, b0, Inst::Store(Mem::Int(8, false), s, 24, b1));
    match between {
        Between::Nothing => {}
        Between::IndexedStore => push(&mut f, b0, Inst::StoreX(Mem::Int(8, false), s, k, 3, 0, b1)),
        Between::PointerStore => push(&mut f, b0, Inst::Store(Mem::Int(8, false), p, 0, b1)),
    }
    push(&mut f, b0, Inst::SlotAddr(s2, sl));
    push(&mut f, b0, Inst::Load(Mem::Int(8, false), x, s2, 0));
    push(&mut f, b0, Inst::Load(Mem::Int(8, false), y, s2, 8));
    push(&mut f, b0, Inst::IBinI(IOp::Shl, U64, sh, k, 3));
    push(&mut f, b0, Inst::IBin(IOp::Add, U64, ad, s2, sh));
    push(&mut f, b0, Inst::Load(Mem::Int(8, false), z, ad, 0));
    push(&mut f, b0, Inst::IBin(IOp::Add, U64, xy, x, y));
    f.blocks[b0 as usize].term = Term::Ret(vec![xy, z]);
    f
}

fn const_offset_loads(f: &Func) -> usize {
    f.blocks[0].insts.iter().filter(|i| matches!(i, Inst::Load(_, _, base, _) if f.blocks[0].insts.iter().any(|j| matches!(j, Inst::SlotAddr(d, _) if d == base)))).count()
}

#[test]
fn element_loads_forward_past_the_other_elements_stores() {
    let mut f = array_reads(Between::Nothing);
    f.verify().unwrap();
    assert_eq!(const_offset_loads(&f), 2);
    assert!(opt::forward_slot_loads(&mut f), "nothing forwarded:\n{}", f.dump());
    f.verify().unwrap();
    assert_eq!(const_offset_loads(&f), 0, "{}", f.dump());
    // the runtime-index read stays a load
    assert_eq!(f.blocks[0].insts.iter().filter(|i| matches!(i, Inst::Load(..))).count(), 1, "{}", f.dump());
    // x and y are the stored a and b
    let (a, b) = (f.params[0], f.params[1]);
    let srcs: Vec<VReg> = f.blocks[0].insts.iter().filter_map(|i| if let Inst::Mov(_, s) = *i { Some(s) } else { None }).collect();
    assert_eq!(srcs, vec![a, b], "{}", f.dump());
}

#[test]
fn a_store_at_a_runtime_index_or_through_a_pointer_keeps_the_loads() {
    for between in [Between::IndexedStore, Between::PointerStore] {
        let mut f = array_reads(between);
        f.verify().unwrap();
        opt::forward_slot_loads(&mut f);
        f.verify().unwrap();
        assert_eq!(const_offset_loads(&f), 2, "{}", f.dump());
    }
}
