//! RegConfig::remat_cost (arm64): a 64-bit constant takes four instructions to build (movz + 3
//! movk). With more values live across a loop than there are registers, a constant read many times
//! in the loop ranks by what a rebuild costs and, if it still finds no register, spills (one ldr per
//! read) instead of being rebuilt at its reads (Loc::None). Results must equal Rust's.
#![cfg(all(target_arch = "aarch64", unix))]
use relax_backend::{exec::Code, rir::*};

const W: IntTy = IntTy { bits: 64, signed: false };
const K: i64 = 0x9e37_79b9_7f4a_7c15u64 as i64;
const L: usize = 60;
const USES: usize = 4;

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// acc[l] = seed + l; loop n times: for u in 0..USES { acc[u % L] = (acc[u % L] ^ K) * 3 + acc[(u + 1) % L] };
/// then every acc[l] += i (all live across the loop); fold.
fn build() -> Func {
    let mut f = Func::new("remat_cost".into(), 0);
    let (n, seed) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![n, seed];
    f.rets = vec![Cls::I];
    let (b0, b1, b2, b3) = (f.block(), f.block(), f.block(), f.block());
    let acc: Vec<VReg> = (0..L).map(|_| f.vreg(Cls::I)).collect();
    let (i, c, k) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    for l in 0..L {
        push(&mut f, b0, Inst::IBinI(IOp::Add, W, acc[l], seed, l as i64));
    }
    push(&mut f, b0, Inst::Iconst(k, K));
    push(&mut f, b0, Inst::Iconst(i, 0));
    f.blocks[b0 as usize].term = Term::Jump(b1);
    push(&mut f, b1, Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[b1 as usize].term = Term::Branch(c, b2, b3);
    for u in 0..USES {
        let (x, y) = (f.vreg(Cls::I), f.vreg(Cls::I));
        let (a, nx) = (acc[u % L], acc[(u + 1) % L]);
        push(&mut f, b2, Inst::IBin(IOp::Xor, W, x, a, k));
        push(&mut f, b2, Inst::IBinI(IOp::Mul, W, y, x, 3));
        push(&mut f, b2, Inst::IBin(IOp::Add, W, a, y, nx));
    }
    for l in 0..L {
        push(&mut f, b2, Inst::IBin(IOp::Add, W, acc[l], acc[l], i));
    }
    push(&mut f, b2, Inst::IBinI(IOp::Add, W, i, i, 1));
    f.blocks[b2 as usize].term = Term::Jump(b1);
    let r = f.vreg(Cls::I);
    push(&mut f, b3, Inst::Iconst(r, 0));
    for l in 0..L {
        let t = f.vreg(Cls::I);
        push(&mut f, b3, Inst::IBinI(IOp::Rotl, W, t, r, 7));
        push(&mut f, b3, Inst::IBin(IOp::Xor, W, r, t, acc[l]));
    }
    f.blocks[b3 as usize].term = Term::Ret(vec![r]);
    f.verify().unwrap();
    f
}

fn reference(n: u64, seed: u64) -> u64 {
    let mut acc: Vec<u64> = (0..L as u64).map(|l| seed.wrapping_add(l)).collect();
    let mut i = 0u64;
    while i < n {
        for u in 0..USES {
            let v = (acc[u % L] ^ K as u64).wrapping_mul(3).wrapping_add(acc[(u + 1) % L]);
            acc[u % L] = v;
        }
        for l in 0..L {
            acc[l] = acc[l].wrapping_add(i);
        }
        i += 1;
    }
    let mut r = 0u64;
    for l in 0..L {
        r = r.rotate_left(7) ^ acc[l];
    }
    r
}

#[test]
fn dear_constants_are_not_rebuilt_per_read() {
    let f = build();
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::arm64::compile_freq(&relax_session::Options::default(), &f, &env, None).unwrap();
    let bytes = c.code[..c.text_len as usize].to_vec();
    let code = Code::new(&bytes).unwrap();
    let run: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
    for (n, seed) in [(0u64, 1u64), (1, 7), (9, 0x1234), (100, 3)] {
        assert_eq!(run(n, seed), reference(n, seed), "n {}", n);
    }
    // the allocation itself (without loop splitting): the constant has a register or a slot
    let cfg = relax_backend::arm64::reg_config();
    let al = relax_backend::regalloc::allocate_with(&relax_session::Options::default(), &f, &cfg, true, None);
    let k = f.blocks[0].insts.iter().find_map(|i| if let Inst::Iconst(d, v) = i { (*v == K).then_some(*d) } else { None }).unwrap();
    assert!(!matches!(al.loc[k.0 as usize], relax_backend::regalloc::Loc::None), "the 64-bit constant is rebuilt at its reads");
}
