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
    assert!(opt::coalesce_slot_copies(&mut f, true));
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
    opt::coalesce_slot_copies(&mut f, true);
    f.verify().unwrap();
    for n in [0u64, 1, 2, 3, 10, 1000] {
        assert_eq!(run(&f, n), reference(n, true), "after, n {}", n);
    }
    // B and C stay apart; D = C still joins
    assert_eq!(copies(&f), 1, "{}", f.dump());
}

/// The prefix case: B (16 bytes) is copied into the start of the 24-byte C, whose last field (a
/// tag) is written past B's end while B is still read; D (16 bytes) takes C's first 16 bytes.
/// f(n): acc = 0; for i < n { if i % 3 != 0 { B = (i, 3i); C[..16] = B; C.tag = i; acc += B.0
/// [clobber: B.0 = 99 after the copy, read with C still live] } else { C.0 = 7 }
/// D = C[..16]; acc = acc * 31 + (D.0 ^ D.1) + C.tag [+ B.0 when clobber] }
fn build_prefix(clobber: bool) -> Func {
    let mut f = Func::new("slot_coalesce_prefix".into(), 0);
    let n = f.vreg(Cls::I);
    f.params.push(n);
    f.rets.push(Cls::I);
    let (sb, sc_, sd) = (f.slot(16, 8), f.slot(24, 8), f.slot(16, 8));
    for _ in 0..7 {
        f.block();
    }
    let v = |f: &mut Func| f.vreg(Cls::I);
    let (pb, pc, pd, acc, i, z, c) = (v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f));
    push(&mut f, 0, Inst::SlotAddr(pb, sb));
    push(&mut f, 0, Inst::SlotAddr(pc, sc_));
    push(&mut f, 0, Inst::SlotAddr(pd, sd));
    push(&mut f, 0, Inst::Iconst(z, 0));
    for o in [0, 8, 16] {
        push(&mut f, 0, Inst::Store(Mem::Int(8, false), pc, o, z));
    }
    if clobber {
        // B is read in every iteration: zero it as Rust's reference does
        push(&mut f, 0, Inst::Store(Mem::Int(8, false), pb, 0, z));
        push(&mut f, 0, Inst::Store(Mem::Int(8, false), pb, 8, z));
    }
    push(&mut f, 0, Inst::Iconst(acc, 0));
    push(&mut f, 0, Inst::Iconst(i, 0));
    f.blocks[0].term = Term::Jump(1);
    push(&mut f, 1, Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[1].term = Term::Branch(c, 2, 6);
    let (r, t, three) = (v(&mut f), v(&mut f), v(&mut f));
    push(&mut f, 2, Inst::Iconst(three, 3));
    push(&mut f, 2, Inst::IBin(IOp::Rem, U64, r, i, three));
    push(&mut f, 2, Inst::ICmpI(Cond::Ne, false, t, r, 0));
    f.blocks[2].term = Term::Branch(t, 3, 4);
    let (i3, b0) = (v(&mut f), v(&mut f));
    push(&mut f, 3, Inst::Store(Mem::Int(8, false), pb, 0, i));
    push(&mut f, 3, Inst::IBinI(IOp::Mul, U64, i3, i, 3));
    push(&mut f, 3, Inst::Store(Mem::Int(8, false), pb, 8, i3));
    push(&mut f, 3, Inst::Copy(pc, pb, 16));
    push(&mut f, 3, Inst::Store(Mem::Int(8, false), pc, 16, i));
    push(&mut f, 3, Inst::Load(Mem::Int(8, false), b0, pb, 0));
    push(&mut f, 3, Inst::IBin(IOp::Add, U64, acc, acc, b0));
    if clobber {
        let k = v(&mut f);
        push(&mut f, 3, Inst::Iconst(k, 99));
        push(&mut f, 3, Inst::Store(Mem::Int(8, false), pb, 0, k));
    }
    f.blocks[3].term = Term::Jump(5);
    let seven = v(&mut f);
    push(&mut f, 4, Inst::Iconst(seven, 7));
    push(&mut f, 4, Inst::Store(Mem::Int(8, false), pc, 0, seven));
    f.blocks[4].term = Term::Jump(5);
    let (a0, a1, x, m, tag) = (v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f));
    push(&mut f, 5, Inst::Copy(pd, pc, 16));
    push(&mut f, 5, Inst::Load(Mem::Int(8, false), a0, pd, 0));
    push(&mut f, 5, Inst::Load(Mem::Int(8, false), a1, pd, 8));
    push(&mut f, 5, Inst::Load(Mem::Int(8, false), tag, pc, 16));
    push(&mut f, 5, Inst::IBin(IOp::Xor, U64, x, a0, a1));
    push(&mut f, 5, Inst::IBinI(IOp::Mul, U64, m, acc, 31));
    push(&mut f, 5, Inst::IBin(IOp::Add, U64, acc, m, x));
    push(&mut f, 5, Inst::IBin(IOp::Add, U64, acc, acc, tag));
    if clobber {
        let b1 = v(&mut f);
        push(&mut f, 5, Inst::Load(Mem::Int(8, false), b1, pb, 0));
        push(&mut f, 5, Inst::IBin(IOp::Add, U64, acc, acc, b1));
    }
    push(&mut f, 5, Inst::IBinI(IOp::Add, U64, i, i, 1));
    f.blocks[5].term = Term::Jump(1);
    f.blocks[6].term = Term::Ret(vec![acc]);
    f.verify().unwrap();
    f
}

fn reference_prefix(n: u64, clobber: bool) -> u64 {
    let (mut b, mut c) = ([0u64; 2], [0u64; 3]);
    let mut acc = 0u64;
    for i in 0..n {
        if i % 3 != 0 {
            b = [i, i.wrapping_mul(3)];
            c[0] = b[0];
            c[1] = b[1];
            c[2] = i;
            acc = acc.wrapping_add(b[0]);
            if clobber {
                b[0] = 99;
            }
        } else {
            c[0] = 7;
        }
        let d = [c[0], c[1]];
        acc = acc.wrapping_mul(31).wrapping_add(d[0] ^ d[1]).wrapping_add(c[2]);
        if clobber {
            acc = acc.wrapping_add(b[0]);
        }
    }
    acc
}

#[test]
fn prefix_copies_join_into_the_larger_slot() {
    let f0 = build_prefix(false);
    let mut f = f0.clone();
    assert_eq!(copies(&f), 2);
    assert!(opt::coalesce_slot_copies(&mut f, true));
    f.verify().unwrap();
    // B becomes C's start (C's tag write lies past B's end), D too
    assert_eq!(copies(&f), 0, "{}", f.dump());
    for n in [0u64, 1, 2, 3, 10, 1000] {
        assert_eq!(run(&f0, n), reference_prefix(n, false), "before, n {}", n);
        assert_eq!(run(&f, n), reference_prefix(n, false), "after, n {}", n);
    }
    // off: equal sizes only, nothing joins here
    let mut g = f0.clone();
    assert!(!opt::coalesce_slot_copies(&mut g, false));
}

#[test]
fn interfering_prefix_slots_stay_apart() {
    let f0 = build_prefix(true);
    let mut f = f0.clone();
    opt::coalesce_slot_copies(&mut f, true);
    f.verify().unwrap();
    for n in [0u64, 1, 2, 3, 10, 1000] {
        assert_eq!(run(&f, n), reference_prefix(n, true), "after, n {}", n);
    }
    // B is written after the copy while C holds its bytes: apart; D = C[..16] still joins
    assert_eq!(copies(&f), 1, "{}", f.dump());
}

/// B = (n, 1); g(&B); C = B; [g(&B)]; return C.0 + C.1 -- `g` keeps no copy of its argument
/// (f.nocapture) or may (no fact). `again`: B is handed to g once more while C is live (g may
/// write B: they interfere).
fn nocapture_build(fact: bool, again: bool) -> Func {
    let mut f = Func::new("slot_coalesce_nocapture".into(), 0);
    let n = f.vreg(Cls::I);
    f.params.push(n);
    f.rets.push(Cls::I);
    let (sb, sc_) = (f.slot(16, 8), f.slot(16, 8));
    f.block();
    let v = |f: &mut Func| f.vreg(Cls::I);
    let (pb, pc, one, a, b, s) = (v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f));
    let g = Callee::Fn(5);
    push(&mut f, 0, Inst::SlotAddr(pb, sb));
    push(&mut f, 0, Inst::SlotAddr(pc, sc_));
    push(&mut f, 0, Inst::Iconst(one, 1));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pb, 0, n));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pb, 8, one));
    push(&mut f, 0, Inst::Call(g.clone(), vec![pb], vec![]));
    push(&mut f, 0, Inst::Copy(pc, pb, 16));
    if again {
        push(&mut f, 0, Inst::Call(g.clone(), vec![pb], vec![]));
    }
    push(&mut f, 0, Inst::Load(Mem::Int(8, false), a, pc, 0));
    push(&mut f, 0, Inst::Load(Mem::Int(8, false), b, pc, 8));
    push(&mut f, 0, Inst::IBin(IOp::Add, U64, s, a, b));
    f.blocks[0].term = Term::Ret(vec![s]);
    if fact {
        f.nocapture.push((g, 0, false, None));
    }
    f
}

#[test]
fn slots_passed_to_nocapture_calls_join() {
    let mut f = nocapture_build(true, false);
    assert!(opt::coalesce_slot_copies(&mut f, true));
    assert_eq!(copies(&f), 0, "the copy of a slot only handed to a nocapture call goes");
}

#[test]
fn slots_passed_to_capturing_calls_stay_apart() {
    let mut f = nocapture_build(false, false);
    assert!(!opt::coalesce_slot_copies(&mut f, true));
    assert_eq!(copies(&f), 1);
}

#[test]
fn nocapture_call_writing_a_slot_while_its_partner_lives_keeps_them_apart() {
    let mut f = nocapture_build(true, true);
    assert!(!opt::coalesce_slot_copies(&mut f, true));
    assert_eq!(copies(&f), 1);
}

/// B = (n, 1); g(&B.0); C = B; g(&B.1); return C.0 + C.1 (`tail`: C.0 only) -- `g` keeps no copy
/// of its argument and touches `ext` of it. Writing [0, 8), the second call writes only B.1,
/// whose bytes in C are dead after it when only C.0 is read: B and C join. Only reading, it writes
/// nothing: they join with C.1 read too (Arc::drop_slow reads the `&mut Arc` of reconcile's entry
/// tuple). Not knowing its access, the call may write B.0 while C.0 is live: apart.
fn nocapture_ext_build(ext: Extent, tail: bool) -> Func {
    let mut f = Func::new("slot_coalesce_nocapture_ext".into(), 0);
    let n = f.vreg(Cls::I);
    f.params.push(n);
    f.rets.push(Cls::I);
    let (sb, sc_) = (f.slot(16, 8), f.slot(16, 8));
    f.block();
    let v = |f: &mut Func| f.vreg(Cls::I);
    let (pb, pb8, pc, one, a, b, s) = (v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f));
    let g = Callee::Fn(5);
    push(&mut f, 0, Inst::SlotAddr(pb, sb));
    push(&mut f, 0, Inst::IBinI(IOp::Add, U64, pb8, pb, 8));
    push(&mut f, 0, Inst::SlotAddr(pc, sc_));
    push(&mut f, 0, Inst::Iconst(one, 1));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pb, 0, n));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pb, 8, one));
    push(&mut f, 0, Inst::Call(g.clone(), vec![pb], vec![]));
    push(&mut f, 0, Inst::Copy(pc, pb, 16));
    push(&mut f, 0, Inst::Call(g.clone(), vec![pb8], vec![]));
    push(&mut f, 0, Inst::Load(Mem::Int(8, false), a, pc, 0));
    if tail {
        f.blocks[0].term = Term::Ret(vec![a]);
    } else {
        push(&mut f, 0, Inst::Load(Mem::Int(8, false), b, pc, 8));
        push(&mut f, 0, Inst::IBin(IOp::Add, U64, s, a, b));
        f.blocks[0].term = Term::Ret(vec![s]);
    }
    f.nocapture.push((g, 0, false, ext));
    f
}

const RW8: Access = Access { read: (0, 8), write: (0, 8) };
const R8: Access = Access { read: (0, 8), write: (0, 0) };

#[test]
fn nocapture_call_writing_only_dead_bytes_joins() {
    let mut f = nocapture_ext_build(Some(RW8), true);
    assert!(opt::coalesce_slot_copies(&mut f, true), "{}", f.dump());
    assert_eq!(copies(&f), 0);
}

#[test]
fn nocapture_call_writing_live_bytes_keeps_them_apart() {
    let mut f = nocapture_ext_build(Some(RW8), false);
    assert!(!opt::coalesce_slot_copies(&mut f, true));
    assert_eq!(copies(&f), 1);
}

#[test]
fn nocapture_call_only_reading_joins() {
    let mut f = nocapture_ext_build(Some(R8), false);
    assert!(opt::coalesce_slot_copies(&mut f, true), "{}", f.dump());
    assert_eq!(copies(&f), 0);
}

#[test]
fn nocapture_call_of_unknown_access_keeps_them_apart() {
    let mut f = nocapture_ext_build(None, true);
    assert!(!opt::coalesce_slot_copies(&mut f, true));
    assert_eq!(copies(&f), 1);
}

/// param_escape's extent: loads, stores and copies at known offsets from the parameter and the
/// extents of the calls it is passed to, shifted by where it is passed; an access at an offset
/// not known (an add of a variable) makes it unknown.
fn extent_callee(variable: bool) -> Func {
    let mut f = Func::new("extent_callee".into(), 0);
    let (p, x) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params.push(p);
    f.params.push(x);
    f.block();
    let v = |f: &mut Func| f.vreg(Cls::I);
    let (a, q, r, w) = (v(&mut f), v(&mut f), v(&mut f), v(&mut f));
    push(&mut f, 0, Inst::Load(Mem::Int(8, false), a, p, 0));
    push(&mut f, 0, Inst::IBinI(IOp::Add, U64, q, p, 8));
    push(&mut f, 0, Inst::Store(Mem::Int(4, false), q, 4, a));
    push(&mut f, 0, Inst::IBinI(IOp::Add, U64, r, p, 16));
    push(&mut f, 0, Inst::Call(Callee::Fn(9), vec![r], vec![]));
    if variable {
        push(&mut f, 0, Inst::IBin(IOp::Add, U64, w, p, x));
        push(&mut f, 0, Inst::Store(Mem::Int(1, false), w, 0, a));
    }
    f.blocks[0].term = Term::Ret(vec![]);
    f
}

#[test]
fn param_extent_from_accesses_and_calls() {
    let sub = |c: &Callee, _k: usize| matches!(c, Callee::Fn(9)).then_some((false, Some(Access { read: (-2, 6), write: (0, 0) })));
    // reads: [0, 8) and the call's [14, 22); writes: [12, 16)
    assert_eq!(opt::param_escape(&extent_callee(false), 0, &sub), Some((false, Some(Access { read: (0, 22), write: (12, 16) }))));
    assert_eq!(opt::param_escape(&extent_callee(true), 0, &sub), Some((false, None)));
    let unknown = |c: &Callee, _k: usize| matches!(c, Callee::Fn(9)).then_some((false, None));
    assert_eq!(opt::param_escape(&extent_callee(false), 0, &unknown), Some((false, None)));
    // the second parameter is not used here: nothing is touched through it
    assert_eq!(opt::param_escape(&extent_callee(false), 1, &sub), Some((false, Some(Access { read: (0, 0), write: (0, 0) }))));
}

/// f(n): A = (n, 2n); C = A (a move); flag = `moved`; C.0 = n + 5; then `if flag == 0 { A.0 }
/// else { C.0 + C.1 }` -- A's read sits behind the drop flag, as a moved-from value's drop in an
/// unwind cleanup does. `moved`: the flag is 1 at C's write, so A is dead there and A and C join;
/// else A's read runs and they stay apart.
fn build_flagged(moved: bool) -> Func {
    let mut f = Func::new("slot_coalesce_flag".into(), 0);
    let n = f.vreg(Cls::I);
    f.params.push(n);
    f.rets.push(Cls::I);
    let (sa, sc_) = (f.slot(16, 8), f.slot(16, 8));
    for _ in 0..4 {
        f.block();
    }
    let v = |f: &mut Func| f.vreg(Cls::I);
    let (pa, pc, flag, n2, n5, x, y, z, s) = (v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f));
    push(&mut f, 0, Inst::SlotAddr(pa, sa));
    push(&mut f, 0, Inst::SlotAddr(pc, sc_));
    push(&mut f, 0, Inst::Iconst(flag, 0));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pa, 0, n));
    push(&mut f, 0, Inst::IBinI(IOp::Mul, U64, n2, n, 2));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pa, 8, n2));
    push(&mut f, 0, Inst::Copy(pc, pa, 16));
    push(&mut f, 0, Inst::Iconst(flag, if moved { 1 } else { 0 }));
    push(&mut f, 0, Inst::IBinI(IOp::Add, U64, n5, n, 5));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pc, 0, n5));
    f.blocks[0].term = Term::Jump(1);
    f.blocks[1].term = Term::Branch(flag, 3, 2);
    push(&mut f, 2, Inst::Load(Mem::Int(8, false), x, pa, 0));
    f.blocks[2].term = Term::Ret(vec![x]);
    push(&mut f, 3, Inst::Load(Mem::Int(8, false), y, pc, 0));
    push(&mut f, 3, Inst::Load(Mem::Int(8, false), z, pc, 8));
    push(&mut f, 3, Inst::IBin(IOp::Add, U64, s, y, z));
    f.blocks[3].term = Term::Ret(vec![s]);
    f
}

#[test]
fn a_read_behind_a_set_drop_flag_does_not_keep_the_moved_from_slot_live() {
    let f = build_flagged(true);
    f.verify().unwrap();
    let want = run(&f, 10);
    assert_eq!(want, 15 + 20);
    let mut g = f.clone();
    assert!(opt::coalesce_slot_copies(&mut g, true));
    g.verify().unwrap();
    assert_eq!(copies(&g), 0);
    assert_eq!(run(&g, 10), want);
}

#[test]
fn a_read_behind_a_clear_drop_flag_keeps_the_slots_apart() {
    let f = build_flagged(false);
    f.verify().unwrap();
    let want = run(&f, 10);
    assert_eq!(want, 10);
    let mut g = f.clone();
    opt::coalesce_slot_copies(&mut g, true);
    assert_eq!(copies(&g), 1);
    assert_eq!(run(&g, 10), want);
}

/// f(n): acc = 0; for i in 0..n { if i odd { H = Some(i) (tag 1, payload at 8) } else { H.tag = `none` }
/// T = H; if T.tag != 2 { T.payload += 100; acc += T.payload } } -- an Option built in place and
/// matched through a copy. `none` = 2: the None path's tag is known at the match, so the payload
/// bytes the copy takes on that path are never read and H and T join (the payload write meets no
/// live byte of H). `none` = 3: the "None" path reaches the payload too (it reads the previous
/// iteration's payload of H), so they stay apart. `retest`: the match tests `T.tag == 2` and its
/// None arm goes on to a second test `T.tag != 2` before the payload read (a `match` and the drop
/// of the scrutinee after it): one condition, both edges the other way round.
fn build_tagged(none: i64, retest: bool) -> Func {
    let mut f = Func::new("slot_coalesce_tag".into(), 0);
    let n = f.vreg(Cls::I);
    f.params.push(n);
    f.rets.push(Cls::I);
    let (sh, st) = (f.slot(16, 8), f.slot(16, 8));
    for _ in 0..10 {
        f.block();
    }
    let v = |f: &mut Func| f.vreg(Cls::I);
    let (x2, c3) = (v(&mut f), v(&mut f));
    let (ph, pt, acc, i, c, r, one, k_none, x, c2, p, p2, i2, z) =
        (v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f));
    push(&mut f, 0, Inst::SlotAddr(ph, sh));
    push(&mut f, 0, Inst::SlotAddr(pt, st));
    push(&mut f, 0, Inst::Iconst(acc, 0));
    push(&mut f, 0, Inst::Iconst(i, 0));
    push(&mut f, 0, Inst::Iconst(one, 1));
    push(&mut f, 0, Inst::Iconst(k_none, none));
    // H starts as None with a zero payload (the payload of a never-Some H is read when none = 3)
    push(&mut f, 0, Inst::Iconst(z, 0));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), ph, 8, z));
    f.blocks[0].term = Term::Jump(1);
    push(&mut f, 1, Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[1].term = Term::Branch(c, 2, 8);
    push(&mut f, 2, Inst::IBinI(IOp::And, U64, r, i, 1));
    f.blocks[2].term = Term::Branch(r, 3, 4);
    // Some(i)
    push(&mut f, 3, Inst::Store(Mem::Int(4, false), ph, 0, one));
    push(&mut f, 3, Inst::Store(Mem::Int(8, false), ph, 8, i));
    f.blocks[3].term = Term::Jump(5);
    // "None": the tag only
    push(&mut f, 4, Inst::Store(Mem::Int(4, false), ph, 0, k_none));
    f.blocks[4].term = Term::Jump(5);
    // T = H; match T.tag
    push(&mut f, 5, Inst::Copy(pt, ph, 16));
    push(&mut f, 5, Inst::Load(Mem::Int(4, false), x, pt, 0));
    if retest {
        push(&mut f, 5, Inst::ICmpI(Cond::Eq, false, c3, x, 2));
        f.blocks[5].term = Term::Branch(c3, 9, 6);
        push(&mut f, 9, Inst::Load(Mem::Int(4, false), x2, pt, 0));
        push(&mut f, 9, Inst::ICmpI(Cond::Ne, false, c2, x2, 2));
        f.blocks[9].term = Term::Branch(c2, 6, 7);
    } else {
        push(&mut f, 5, Inst::ICmpI(Cond::Ne, false, c2, x, 2));
        f.blocks[5].term = Term::Branch(c2, 6, 7);
        f.blocks[9].term = Term::Jump(7);
    }
    push(&mut f, 6, Inst::Load(Mem::Int(8, false), p, pt, 8));
    push(&mut f, 6, Inst::IBinI(IOp::Add, U64, p2, p, 100));
    push(&mut f, 6, Inst::Store(Mem::Int(8, false), pt, 8, p2));
    push(&mut f, 6, Inst::IBin(IOp::Add, U64, acc, acc, p2));
    f.blocks[6].term = Term::Jump(7);
    push(&mut f, 7, Inst::IBinI(IOp::Add, U64, i2, i, 1));
    push(&mut f, 7, Inst::Mov(i, i2));
    f.blocks[7].term = Term::Jump(1);
    f.blocks[8].term = Term::Ret(vec![acc]);
    f
}

#[test]
fn a_known_tag_on_the_none_path_lets_the_option_and_its_match_copy_join() {
    for retest in [false, true] {
        let f = build_tagged(2, retest);
        f.verify().unwrap();
        let want = run(&f, 10);
        assert_eq!(want, (1 + 3 + 5 + 7 + 9) + 5 * 100);
        let mut g = f.clone();
        assert!(opt::coalesce_slot_copies(&mut g, true), "retest {}", retest);
        g.verify().unwrap();
        assert_eq!(copies(&g), 0);
        assert_eq!(run(&g, 10), want);
    }
}

#[test]
fn a_tag_that_reaches_the_payload_keeps_the_option_and_its_copy_apart() {
    for retest in [false, true] {
        let f = build_tagged(3, retest);
        f.verify().unwrap();
        let want = run(&f, 10);
        let mut g = f.clone();
        opt::coalesce_slot_copies(&mut g, true);
        assert_eq!(copies(&g), 1, "retest {}", retest);
        assert_eq!(run(&g, 10), want);
    }
}

/// DFREE (Sandbox: glibc "double free or corruption (out)"): HashMap::entry inlined. The key K (40
/// bytes: String, bool, live-edit ext word) moves into the Entry result R (64 bytes) at offset 0 on
/// the Vacant path and at offset 8 on the Occupied path (behind the -1 niche word). Joining K into R
/// along the Vacant copy made the Occupied move a copy of one slot onto itself shifted by a word,
/// copied forward in 16-byte pieces: the moved key's words came out [w0, w1, w1, w3, w3].
/// f(n): K = [n, 3n, n + 5, n ^ 0xff, 7n]; odd n: R = Occupied(K) (R.0 = -1, K at R+8); even n:
/// R = Vacant(K, 11, 12) (K at R+0); returns a hash of what R holds.
fn build_entry_move() -> Func {
    let mut f = Func::new("entry_move".into(), 0);
    let n = f.vreg(Cls::I);
    f.params.push(n);
    f.rets.push(Cls::I);
    let (sk, sr) = (f.slot(40, 8), f.slot(64, 8));
    for _ in 0..6 {
        f.block();
    }
    let v = |f: &mut Func| f.vreg(Cls::I);
    let (pk, pr) = (v(&mut f), v(&mut f));
    push(&mut f, 0, Inst::SlotAddr(pk, sk));
    push(&mut f, 0, Inst::SlotAddr(pr, sr));
    let (w1, w2, w3, w4) = (v(&mut f), v(&mut f), v(&mut f), v(&mut f));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pk, 0, n));
    push(&mut f, 0, Inst::IBinI(IOp::Mul, U64, w1, n, 3));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pk, 8, w1));
    push(&mut f, 0, Inst::IBinI(IOp::Add, U64, w2, n, 5));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pk, 16, w2));
    push(&mut f, 0, Inst::IBinI(IOp::Xor, U64, w3, n, 0xff));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pk, 24, w3));
    push(&mut f, 0, Inst::IBinI(IOp::Mul, U64, w4, n, 7));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pk, 32, w4));
    let (odd, t) = (v(&mut f), v(&mut f));
    push(&mut f, 0, Inst::IBinI(IOp::And, U64, odd, n, 1));
    push(&mut f, 0, Inst::ICmpI(Cond::Ne, false, t, odd, 0));
    f.blocks[0].term = Term::Branch(t, 1, 2);
    // b1: Occupied: the key at R+8, the niche word -1 at R+0
    let (p8, m1) = (v(&mut f), v(&mut f));
    push(&mut f, 1, Inst::IBinI(IOp::Add, U64, p8, pr, 8));
    push(&mut f, 1, Inst::Copy(p8, pk, 40));
    push(&mut f, 1, Inst::Iconst(m1, -1));
    push(&mut f, 1, Inst::Store(Mem::Int(8, false), pr, 0, m1));
    f.blocks[1].term = Term::Jump(3);
    // b2: Vacant: the key at R+0, then two more fields
    let (a, b) = (v(&mut f), v(&mut f));
    push(&mut f, 2, Inst::Copy(pr, pk, 40));
    push(&mut f, 2, Inst::Iconst(a, 11));
    push(&mut f, 2, Inst::Store(Mem::Int(8, false), pr, 40, a));
    push(&mut f, 2, Inst::Iconst(b, 12));
    push(&mut f, 2, Inst::Store(Mem::Int(8, false), pr, 48, b));
    f.blocks[2].term = Term::Jump(3);
    // b3: occupied (R.0 == -1)? hash R+8..R+48 : hash R+0..R+56
    let (tag, occ) = (v(&mut f), v(&mut f));
    push(&mut f, 3, Inst::Load(Mem::Int(8, false), tag, pr, 0));
    push(&mut f, 3, Inst::ICmpI(Cond::Eq, false, occ, tag, -1));
    f.blocks[3].term = Term::Branch(occ, 4, 5);
    for (blk, from, words) in [(4u32, 8i32, 5), (5, 0, 7)] {
        let acc = v(&mut f);
        push(&mut f, blk, Inst::Iconst(acc, 0));
        for k in 0..words {
            let (x, m) = (v(&mut f), v(&mut f));
            push(&mut f, blk, Inst::Load(Mem::Int(8, false), x, pr, from + 8 * k));
            push(&mut f, blk, Inst::IBinI(IOp::Mul, U64, m, acc, 1_000_003));
            push(&mut f, blk, Inst::IBin(IOp::Add, U64, acc, m, x));
        }
        f.blocks[blk as usize].term = Term::Ret(vec![acc]);
    }
    f.verify().unwrap();
    f
}

fn entry_move_reference(n: u64) -> u64 {
    let k = [n, n.wrapping_mul(3), n.wrapping_add(5), n ^ 0xff, n.wrapping_mul(7)];
    let ws: Vec<u64> = if n & 1 != 0 { k.to_vec() } else { k.iter().copied().chain([11, 12]).collect() };
    ws.iter().fold(0u64, |acc, &x| acc.wrapping_mul(1_000_003).wrapping_add(x))
}

#[test]
fn a_move_to_another_offset_keeps_the_slots_apart() {
    let f0 = build_entry_move();
    for n in [6u64, 7, 1000, 1001] {
        assert_eq!(run(&f0, n), entry_move_reference(n), "before the pass, n = {n}");
    }
    let mut f = f0.clone();
    opt::coalesce_slot_copies(&mut f, true);
    assert!(f.overlapping_copy().is_none(), "the pass left a copy between overlapping bytes of one slot:\n{}", f.dump());
    f.verify().unwrap();
    for n in [6u64, 7, 1000, 1001] {
        assert_eq!(run(&f, n), entry_move_reference(n), "after the pass, n = {n}\n{}", f.dump());
    }
}

/// The backends' guard (frame::copy_overlap): a copy between overlapping bytes of the frame --
/// here the joined form of the DFREE miscompile, one 64-byte slot, the 40 bytes at 0 copied to 8
/// and to -8 of a second one -- is emitted in memmove order outside verify mode (a correct copy,
/// never a shifted one), and the RIR verifier names it.
#[test]
fn an_overlapping_frame_copy_is_a_memmove() {
    for up in [true, false] {
        let mut f = Func::new("overlap_copy".into(), 0);
        let n = f.vreg(Cls::I);
        f.params.push(n);
        f.rets.push(Cls::I);
        let s = f.slot(64, 8);
        f.block();
        let v = |f: &mut Func| f.vreg(Cls::I);
        let (p, q, src) = (v(&mut f), v(&mut f), v(&mut f));
        push(&mut f, 0, Inst::SlotAddr(p, s));
        // up: [8..48) <- [0..40); down: [0..40) <- [8..48)
        let (from, to) = if up { (0, 8) } else { (8, 0) };
        push(&mut f, 0, Inst::IBinI(IOp::Add, U64, src, p, from));
        push(&mut f, 0, Inst::IBinI(IOp::Add, U64, q, p, to));
        for k in 0..6 {
            let w = v(&mut f);
            push(&mut f, 0, Inst::IBinI(IOp::Add, U64, w, n, 100 * (k + 1)));
            push(&mut f, 0, Inst::Store(Mem::Int(8, false), p, 8 * k as i32, w));
        }
        push(&mut f, 0, Inst::Copy(q, src, 40));
        let acc = v(&mut f);
        push(&mut f, 0, Inst::Iconst(acc, 0));
        for k in 0..6 {
            let (x, m) = (v(&mut f), v(&mut f));
            push(&mut f, 0, Inst::Load(Mem::Int(8, false), x, p, 8 * k));
            push(&mut f, 0, Inst::IBinI(IOp::Mul, U64, m, acc, 1_000_003));
            push(&mut f, 0, Inst::IBin(IOp::Add, U64, acc, m, x));
        }
        f.blocks[0].term = Term::Ret(vec![acc]);
        assert!(f.overlapping_copy().is_some(), "the verifier names the overlapping copy");
        let reference = |n: u64| {
            let mut w: Vec<u64> = (0..6).map(|k| n.wrapping_add(100 * (k + 1))).collect();
            if up { w.copy_within(0..5, 1) } else { w.copy_within(1..6, 0) }
            w.iter().fold(0u64, |a, &x| a.wrapping_mul(1_000_003).wrapping_add(x))
        };
        for n in [0u64, 5, 77] {
            assert_eq!(run(&f, n), reference(n), "up {up}, n = {n}");
        }
    }
}

/// A value moved into a by-value argument copy (`Slot::arg`) for one call: `E = (n, 1); A = E;
/// call g(&A)`, E read again after the call only when `reread` (then E and A must stay apart:
/// the callee owns A and may write it). GAPB: `out.push(entry)` into an outlined push.
fn arg_copy_build(reread: bool) -> Func {
    let mut f = Func::new("slot_coalesce_arg_copy".into(), 0);
    let n = f.vreg(Cls::I);
    f.params.push(n);
    f.rets.push(Cls::I);
    let (se, sa) = (f.slot(16, 8), f.slot(16, 8));
    f.slots[sa as usize].arg = true;
    f.block();
    let v = |f: &mut Func| f.vreg(Cls::I);
    let (pe, pa, one, a, b, s) = (v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f), v(&mut f));
    push(&mut f, 0, Inst::SlotAddr(pe, se));
    push(&mut f, 0, Inst::SlotAddr(pa, sa));
    push(&mut f, 0, Inst::Iconst(one, 1));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pe, 0, n));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), pe, 8, one));
    push(&mut f, 0, Inst::Copy(pa, pe, 16));
    push(&mut f, 0, Inst::Call(Callee::Fn(5), vec![pa], vec![]));
    if reread {
        push(&mut f, 0, Inst::Load(Mem::Int(8, false), a, pe, 0));
        push(&mut f, 0, Inst::Load(Mem::Int(8, false), b, pe, 8));
        push(&mut f, 0, Inst::IBin(IOp::Add, U64, s, a, b));
    } else {
        push(&mut f, 0, Inst::Mov(s, n));
    }
    f.blocks[0].term = Term::Ret(vec![s]);
    f
}

#[test]
fn a_value_moved_into_an_argument_copy_joins_it() {
    let mut f = arg_copy_build(false);
    assert!(opt::coalesce_slot_copies(&mut f, true));
    assert_eq!(copies(&f), 0, "the moved value is built in the argument copy");
}

#[test]
fn a_value_read_after_the_call_owning_its_copy_stays_apart() {
    let mut f = arg_copy_build(true);
    assert!(!opt::coalesce_slot_copies(&mut f, true));
    assert_eq!(copies(&f), 1, "the callee may write its copy: the value read after it is another one");
}
