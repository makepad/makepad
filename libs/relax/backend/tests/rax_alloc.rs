//! x64 rax as an allocatable register (keeps_rax): more integer values live across a loop than
//! the other registers hold, the loop body stores and loads through indexed accesses on a stack
//! buffer (spilled indices and values take r10 / r11, never rax) and dispatches through a jump
//! table (Term::Switch works in r10 / r11, values live into the cases may sit in rax). Results must
//! equal the same computation in Rust with rax allocatable and without (MAKEPAD_RUN_OPT=-rax), and
//! rax must change the code.
#![cfg(all(target_arch = "x86_64", unix))]
use relax_backend::{exec::Code, rir::*};
use std::sync::Arc;

fn options(off: &[&str]) -> relax_session::Options {
    let mut o = relax_session::Options::default();
    o.opt = off.iter().map(|s| s.to_string()).collect();
    o
}

fn compile(o: &relax_session::Options, f: &Func, thorough: bool) -> Vec<u8> {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: thorough, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile_freq(o, f, &env, None).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

const W: IntTy = IntTy { bits: 64, signed: false };

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

fn build(k_n: usize) -> Func {
    let mut f = Func::new("rax_alloc".into(), 0);
    let (n, seed) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![n, seed];
    f.rets = vec![Cls::I];
    // b0 entry, b1 header, b2 body (ends in the switch), b3..b6 cases, b7 default, b8 latch, b9 exit
    let bl: Vec<u32> = (0..10).map(|_| f.block()).collect();
    let slot = f.slot(64, 8);
    let sa = f.vreg(Cls::I);
    let acc: Vec<VReg> = (0..k_n).map(|_| f.vreg(Cls::I)).collect();
    let (i, c) = (f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, bl[0], Inst::SlotAddr(sa, slot));
    let zero = f.vreg(Cls::I);
    push(&mut f, bl[0], Inst::Iconst(zero, 0));
    for j in 0..8 {
        push(&mut f, bl[0], Inst::Store(Mem::Int(8, false), sa, 8 * j, zero));
    }
    for k in 0..k_n {
        push(&mut f, bl[0], Inst::IBinI(IOp::Add, W, acc[k], seed, (k as i64 + 1) * 0x10001));
    }
    push(&mut f, bl[0], Inst::Iconst(i, 0));
    f.blocks[bl[0] as usize].term = Term::Jump(bl[1]);
    push(&mut f, bl[1], Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[bl[1] as usize].term = Term::Branch(c, bl[2], bl[9]);
    for k in 0..k_n {
        let (t, x, y, z, u) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
        push(&mut f, bl[2], Inst::IBinI(IOp::Add, W, t, i, k as i64));
        push(&mut f, bl[2], Inst::IBinI(IOp::And, W, x, t, 7));
        push(&mut f, bl[2], Inst::StoreX(Mem::Int(8, false), sa, x, 3, 0, acc[k]));
        push(&mut f, bl[2], Inst::IBinI(IOp::Mul, W, y, t, 3));
        push(&mut f, bl[2], Inst::IBinI(IOp::And, W, z, y, 7));
        push(&mut f, bl[2], Inst::LoadX(Mem::Int(8, false), u, sa, z, 3, 0));
        let r = f.vreg(Cls::I);
        push(&mut f, bl[2], Inst::IBinI(IOp::Rotl, W, r, u, 9));
        push(&mut f, bl[2], Inst::IBin(IOp::Xor, W, acc[k], acc[k], r));
    }
    let sel = f.vreg(Cls::I);
    push(&mut f, bl[2], Inst::IBinI(IOp::And, W, sel, i, 7));
    let targets = vec![bl[3], bl[4], bl[5], bl[6], bl[3], bl[4], bl[5]];
    f.blocks[bl[2] as usize].term = Term::Switch(sel, Arc::new(SwitchTable::new(0, targets, bl[7])));
    for (case, b) in [bl[3], bl[4], bl[5], bl[6], bl[7]].into_iter().enumerate() {
        let j = (case * 3) % k_n;
        push(&mut f, b, Inst::IBinI(IOp::Add, W, acc[j], acc[j], case as i64 * 17 + 1));
        f.blocks[b as usize].term = Term::Jump(bl[8]);
    }
    push(&mut f, bl[8], Inst::IBinI(IOp::Add, W, i, i, 1));
    f.blocks[bl[8] as usize].term = Term::Jump(bl[1]);
    let r = f.vreg(Cls::I);
    push(&mut f, bl[9], Inst::Iconst(r, 0));
    for k in 0..k_n {
        let t = f.vreg(Cls::I);
        push(&mut f, bl[9], Inst::IBinI(IOp::Rotl, W, t, r, 7));
        push(&mut f, bl[9], Inst::IBin(IOp::Xor, W, r, t, acc[k]));
    }
    f.blocks[bl[9] as usize].term = Term::Ret(vec![r]);
    f.verify().unwrap();
    f
}

fn reference(n: u64, seed: u64, k_n: usize) -> u64 {
    let mut buf = [0u64; 8];
    let mut acc: Vec<u64> = (0..k_n as u64).map(|k| seed.wrapping_add((k + 1) * 0x10001)).collect();
    let mut i = 0u64;
    while i < n {
        for k in 0..k_n {
            let t = i.wrapping_add(k as u64);
            buf[(t & 7) as usize] = acc[k];
            let u = buf[(t.wrapping_mul(3) & 7) as usize];
            acc[k] ^= u.rotate_left(9);
        }
        let case = match i & 7 {
            0 | 4 => 0,
            1 | 5 => 1,
            2 | 6 => 2,
            3 => 3,
            _ => 4,
        };
        let j = (case * 3) % k_n;
        acc[j] = acc[j].wrapping_add(case as u64 * 17 + 1);
        i += 1;
    }
    let mut r = 0u64;
    for k in 0..k_n {
        r = r.rotate_left(7) ^ acc[k];
    }
    r
}

#[test]
fn rax_holds_values_across_indexed_accesses_and_switches() {
    let mut changed = false;
    for k_n in [6usize, 11, 16] {
        let f = build(k_n);
        for thorough in [false, true] {
            let mut codes = Vec::new();
            for off in [&[][..], &["-rax"][..], &["+evict"][..]] {
                let bytes = compile(&options(off), &f, thorough);
                let code = Code::new(&bytes).unwrap();
                let run: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
                for (n, seed) in [(0u64, 1u64), (1, 7), (9, 0x1234), (64, 0x9e3779b97f4a7c15), (301, 3)] {
                    assert_eq!(run(n, seed), reference(n, seed, k_n), "K {} thorough {} off {:?} n {}", k_n, thorough, off, n);
                }
                codes.push(bytes);
            }
            changed |= codes[0] != codes[1];
        }
    }
    assert!(changed, "rax never allocated: the test no longer covers it");
}
