//! coalesce_slot_copies: slots joined along whole copies when byte liveness shows they never hold
//! different live bytes -- around loops too -- and kept apart when a write of one meets live bytes
//! of the other. Results compiled before and after the pass must equal the Rust reference.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};
use relax_codegen::opt;

const U64: IntTy = IntTy { bits: 64, signed: false };
fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(1);
}

#[cfg(target_arch = "x86_64")]
fn run(f: &Func, n: u64) -> u64 {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile(&relax_session::Options::default(), f, &env).unwrap();
    let code = Code::new(&c.code[..c.text_len as usize]).unwrap();
    let g: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
    g(n)
}

#[cfg(target_arch = "aarch64")]
fn run(f: &Func, n: u64) -> u64 {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::arm64::compile(&relax_session::Options::default(), f, &env).unwrap();
    let code = Code::new(&c.code[..c.text_len as usize]).unwrap();
    let g: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
    g(n)
}

fn copies(f: &Func) -> usize {
    f.blocks.iter().flat_map(|b| b.insts.iter()).filter(|i| matches!(i, Inst::Copy(..))).count()
}

/// f(n): acc = 0; i = 0; while i < n { if i % 3 != 0 { B = (i, 3i); C = B } else { C.0 = 7 }
/// D = C; acc = acc * 31 + (D.0 ^ D.1); [clobber] ; i += 1 } -- C's second field on the `else` path
/// is the previous iteration's (initially zero). `clobber`: after `C = B`, B.0 = 99 while C is still
/// read (B and C interfere: they must stay apart).
fn build(clobber: bool) -> Func {
    let mut f = Func::new("slot_coalesce".into(), 0);
    let n = f.vreg(Cls::I);
    f.params.push(n);
    f.rets.push(Cls::I);
    let (sb, sc_, sd) = (f.slot(16, 8), f.slot(16, 8), f.slot(16, 8));
    for _ in 0..7 {
        f.block();
    }
    let v = |f: &mut Func| f.vreg(Cls::I);
    let (pb, pc, pd, acc, i, z, c) = (v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f));
    // b0: slots, zero C, acc = i = 0
    push(&mut f, 0, Inst::SlotAddr(pb, sb));
    push(&mut f, 0, Inst::SlotAddr(pc, sc_));
    push(&mut f, 0, Inst::SlotAddr(pd, sd));
    push(&mut f, 0, Inst::Iconst(z, 0));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pc, 0, z));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pc, 8, z));
    if clobber {
        // B is read in every iteration: zero it as Rust's reference does
        push(&mut f, 0, Inst::Store(Mem::Int(8, false), pb, 0, z));
        push(&mut f, 0, Inst::Store(Mem::Int(8, false), pb, 8, z));
    }
    push(&mut f, 0, Inst::Iconst(acc, 0));
    push(&mut f, 0, Inst::Iconst(i, 0));
    f.blocks[0].term = Term::Jump(1);
    // b1: header
    push(&mut f, 1, Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[1].term = Term::Branch(c, 2, 6);
    // b2: i % 3 != 0 ?
    let (r, t) = (v(&mut f), v(&mut f));
    let three = v(&mut f);
    push(&mut f, 2, Inst::Iconst(three, 3));
    push(&mut f, 2, Inst::IBin(IOp::Rem, U64, r, i, three));
    push(&mut f, 2, Inst::ICmpI(Cond::Ne, false, t, r, 0));
    f.blocks[2].term = Term::Branch(t, 3, 4);
    // b3: B = (i, 3i); C = B
    let i3 = v(&mut f);
    push(&mut f, 3, Inst::Store(Mem::Int(8, false), pb, 0, i));
    push(&mut f, 3, Inst::IBinI(IOp::Mul, U64, i3, i, 3));
    push(&mut f, 3, Inst::Store(Mem::Int(8, false), pb, 8, i3));
    push(&mut f, 3, Inst::Copy(pc, pb, 16));
    if clobber {
        let k = v(&mut f);
        push(&mut f, 3, Inst::Iconst(k, 99));
        push(&mut f, 3, Inst::Store(Mem::Int(8, false), pb, 0, k));
    }
    f.blocks[3].term = Term::Jump(5);
    // b4: C.0 = 7
    let seven = v(&mut f);
    push(&mut f, 4, Inst::Iconst(seven, 7));
    push(&mut f, 4, Inst::Store(Mem::Int(8, false), pc, 0, seven));
    f.blocks[4].term = Term::Jump(5);
    // b5: D = C; acc = acc * 31 + (D.0 ^ D.1) [+ B.0 when clobber]; i += 1
    let (a0, a1, x, m) = (v(&mut f), v(&mut f), v(&mut f), v(&mut f));
    push(&mut f, 5, Inst::Copy(pd, pc, 16));
    push(&mut f, 5, Inst::Load(Mem::Int(8, false), a0, pd, 0));
    push(&mut f, 5, Inst::Load(Mem::Int(8, false), a1, pd, 8));
    push(&mut f, 5, Inst::IBin(IOp::Xor, U64, x, a0, a1));
    push(&mut f, 5, Inst::IBinI(IOp::Mul, U64, m, acc, 31));
    push(&mut f, 5, Inst::IBin(IOp::Add, U64, acc, m, x));
    if clobber {
        let b0 = v(&mut f);
        push(&mut f, 5, Inst::Load(Mem::Int(8, false), b0, pb, 0));
        push(&mut f, 5, Inst::IBin(IOp::Add, U64, acc, acc, b0));
    }
    push(&mut f, 5, Inst::IBinI(IOp::Add, U64, i, i, 1));
    f.blocks[5].term = Term::Jump(1);
    f.blocks[6].term = Term::Ret(vec![acc]);
    f.verify().unwrap();
    f
}

fn reference(n: u64, clobber: bool) -> u64 {
    let (mut b, mut c) = ([0u64; 2], [0u64; 2]);
    let mut acc = 0u64;
    for i in 0..n {
        if i % 3 != 0 {
            b = [i, i.wrapping_mul(3)];
            c = b;
            if clobber {
                b[0] = 99;
            }
        } else {
            c[0] = 7;
        }
        let d = c;
        acc = acc.wrapping_mul(31).wrapping_add(d[0] ^ d[1]);
        if clobber {
            acc = acc.wrapping_add(b[0]);
        }
    }
    acc
}

#[test]
fn loop_copy_chain_joins_into_one_slot() {
    let f0 = build(false);
    let mut f = f0.clone();
    assert_eq!(copies(&f), 2);
    assert!(opt::coalesce_slot_copies(&mut f));
    f.verify().unwrap();
    // both join: B's fields are written only where C's bytes are dead (C = B follows), and D is
    // written only by its copy
    assert_eq!(copies(&f), 0, "{}", f.dump());
    for n in [0u64, 1, 2, 3, 10, 1000] {
        assert_eq!(run(&f0, n), reference(n, false), "before, n {}", n);
        assert_eq!(run(&f, n), reference(n, false), "after, n {}", n);
    }
}

#[test]
fn interfering_slots_stay_apart() {
    let f0 = build(true);
    let mut f = f0.clone();
    opt::coalesce_slot_copies(&mut f);
    f.verify().unwrap();
    for n in [0u64, 1, 2, 3, 10, 1000] {
        assert_eq!(run(&f, n), reference(n, true), "after, n {}", n);
    }
    // B and C stay apart; D = C still joins
    assert_eq!(copies(&f), 1, "{}", f.dump());
}
