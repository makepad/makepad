//! `regalloc::share_index_bases` on an unrolled copy loop whose load and store runs interleave
//! (`d[j + 8k] = s[i + 8k]`, k = 0..5): every access of both runs is rewritten to one address
//! register per run (the second group's accesses used to be skipped: splicing the first group's
//! address moved them off their indices), and the copy still copies (arm64, run on the host).
#![cfg(all(target_arch = "aarch64", unix))]
use relax_backend::{exec::Code, rir::*};

fn func() -> Func {
    let mut f = Func::new("copy_loop".into(), 0);
    let (b, i, j, n) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![b, i, j, n];
    f.rets = vec![];
    let (b0, b1, b2) = (f.block(), f.block(), f.block());
    let u = IntTy { bits: 64, signed: false };
    let (vi, vj) = (f.vreg(Cls::I), f.vreg(Cls::I));
    let mut ins = vec![];
    for k in 0..5 {
        let v = f.vreg(Cls::I);
        ins.push(Inst::LoadX(Mem::Int(8, false), v, b, vi, 0, 8 * k));
        ins.push(Inst::StoreX(Mem::Int(8, false), b, vj, 0, 8 * k, v));
    }
    let (ni, nj, c) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    ins.extend([Inst::IBinI(IOp::Add, u, ni, vi, 40), Inst::Mov(vi, ni), Inst::IBinI(IOp::Add, u, nj, vj, 40), Inst::Mov(vj, nj), Inst::ICmp(Cond::Lt, false, c, vj, n)]);
    f.blocks[b0 as usize].insts = vec![Inst::Mov(vi, i), Inst::Mov(vj, j)];
    f.blocks[b0 as usize].pos = vec![0; 2];
    f.blocks[b0 as usize].term = Term::Jump(b1);
    f.blocks[b1 as usize].pos = vec![0; ins.len()];
    f.blocks[b1 as usize].insts = ins;
    f.blocks[b1 as usize].term = Term::Branch(c, b1, b2);
    f.blocks[b2 as usize].term = Term::Ret(vec![]);
    f.verify().unwrap();
    f
}

#[test]
fn interleaved_load_and_store_runs_each_share_one_address() {
    let f = func();
    let g = relax_backend::regalloc::share_index_bases(&f).expect("both runs share an address");
    let indexed = g.blocks[1].insts.iter().filter(|i| matches!(i, Inst::LoadX(..) | Inst::StoreX(..))).count();
    assert_eq!(indexed, 0, "{}", g.dump());
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::arm64::compile(&relax_session::Options::default(), &f, &env).unwrap();
    let code = Code::new(&c.code).unwrap();
    let run: extern "C" fn(*mut u8, u64, u64, u64) = unsafe { std::mem::transmute(code.ptr()) };
    let mut buf: Vec<u8> = (0..400u32).map(|x| (x * 13 + 1) as u8).collect();
    let orig = buf.clone();
    // two 40-byte rounds: [0, 80) copied to [200, 280)
    run(buf.as_mut_ptr(), 0, 200, 280);
    assert_eq!(&buf[200..280], &orig[0..80]);
    assert_eq!(&buf[..200], &orig[..200]);
    assert_eq!(&buf[280..], &orig[280..]);
}
