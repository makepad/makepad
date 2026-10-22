//! merge_stores on byte stores of constants (an unrolled `[0u8; 8]` fill): word stores of the
//! combined constant, so sroa sees the slot as one cell.
use relax_backend::rir::*;
use relax_codegen::loops;

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(1);
}

/// b0: k_j = consts; slot bytes 0..n = k_j; x = load 8 of the slot; ret x
fn fill(bytes: &[i64]) -> (Func, VReg) {
    let mut f = Func::new("fill".into(), 0);
    f.rets.push(Cls::I);
    let s = f.slot(8, 8);
    f.block();
    let p = f.vreg(Cls::I);
    push(&mut f, 0, Inst::SlotAddr(p, s));
    let ks: Vec<VReg> = bytes.iter().map(|_| f.vreg(Cls::I)).collect();
    for (k, &b) in ks.iter().zip(bytes) {
        push(&mut f, 0, Inst::Iconst(*k, b));
    }
    for (j, k) in ks.iter().enumerate() {
        push(&mut f, 0, Inst::Store(Mem::Int(1, false), p, j as i32, *k));
    }
    let x = f.vreg(Cls::I);
    push(&mut f, 0, Inst::Load(Mem::Int(8, false), x, p, 0));
    f.blocks[0].term = Term::Ret(vec![x]);
    (f, x)
}

fn stores(f: &Func) -> Vec<(u8, i32, i64)> {
    let mut kv = std::collections::HashMap::new();
    for i in &f.blocks[0].insts {
        if let Inst::Iconst(d, k) = *i {
            kv.insert(d.0, k);
        }
    }
    f.blocks[0].insts.iter().filter_map(|i| match *i {
        Inst::Store(Mem::Int(w, _), _, off, v) => Some((w, off, kv[&v.0])),
        _ => None,
    }).collect()
}

#[test]
fn a_byte_fill_of_zeros_is_one_word_store() {
    let (mut f, _) = fill(&[0; 8]);
    assert!(loops::merge_stores(&relax_session::Options::default(), &mut f));
    f.verify().unwrap();
    assert_eq!(stores(&f), vec![(8, 0, 0)]);
    // the slot is one 8-byte cell now: sroa takes it
    assert!(relax_backend::passes::sroa(&mut f));
    assert!(!f.blocks[0].insts.iter().any(|i| matches!(i, Inst::Load(..) | Inst::Store(..))));
}

#[test]
fn constant_bytes_combine_little_endian_in_word_pieces() {
    let (mut f, _) = fill(&[1, 2, 3, 4, 5, 6, 0x87, 0xff]);
    assert!(loops::merge_stores(&relax_session::Options::default(), &mut f));
    f.verify().unwrap();
    assert_eq!(stores(&f), vec![(8, 0, 0xff87_0605_0403_0201u64 as i64)]);
    // 7 bytes: 4 + 2 + one byte left as it was
    let (mut f, _) = fill(&[1, 2, 3, 4, 5, 6, 7]);
    assert!(loops::merge_stores(&relax_session::Options::default(), &mut f));
    f.verify().unwrap();
    assert_eq!(stores(&f), vec![(4, 0, 0x0403_0201), (2, 4, 0x0605), (1, 6, 7)]);
}

#[test]
fn a_load_between_ends_the_run() {
    let (mut f, x) = fill(&[0; 4]);
    // a load of the slot between byte 1 and byte 2 reads the first two bytes as stored
    let y = f.vreg(Cls::I);
    let p = match f.blocks[0].insts[0] { Inst::SlotAddr(p, _) => p, _ => unreachable!() };
    let at = f.blocks[0].insts.iter().position(|i| matches!(i, Inst::Store(_, _, 2, _))).unwrap();
    f.blocks[0].insts.insert(at, Inst::Load(Mem::Int(2, false), y, p, 0));
    f.blocks[0].pos.insert(at, 1);
    assert!(loops::merge_stores(&relax_session::Options::default(), &mut f));
    f.verify().unwrap();
    assert_eq!(stores(&f), vec![(2, 0, 0), (2, 2, 0)]);
    let _ = x;
}
