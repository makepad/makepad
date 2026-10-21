//! Adjacent loads and stores off one base (8/4-byte ints, f64/f32, both offset orders, a stack
//! slot base): arm64 pairs them (ldp / stp); the values must be the single accesses' (u32 loads
//! zero-extended), run on the host.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func) -> Code {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
    Code::new(&relax_backend::x64::compile(&relax_session::Options::default(), f, &env).unwrap().code).unwrap()
}

#[cfg(target_arch = "aarch64")]
fn compile(f: &Func) -> Code {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
    Code::new(&relax_backend::arm64::compile(&relax_session::Options::default(), f, &env).unwrap().code).unwrap()
}

/// fn(src, dst): a = src[o1], b = src[o2] (m); dst[o2] = a, dst[o1] = b (swapped), and through a
/// stack slot: slot[0] = a, slot[w] = b, dst[16 + ..] = slot[..] reloaded as a pair
fn func(m: Mem, w: i32, swap: bool) -> Func {
    let mut f = Func::new("pairs".into(), 0);
    let (ps, pd) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![ps, pd];
    f.rets = vec![];
    f.block();
    let cls = match m {
        Mem::F64 => Cls::F64,
        Mem::F32 => Cls::F32,
        _ => Cls::I,
    };
    let slot = f.slot(16, 8);
    let (a, b, c, d, sa) = (f.vreg(cls), f.vreg(cls), f.vreg(cls), f.vreg(cls), f.vreg(Cls::I));
    let (o1, o2) = if swap { (w, 0) } else { (0, w) };
    let insts = vec![
        Inst::Load(m, a, ps, o1),
        Inst::Load(m, b, ps, o2),
        Inst::Store(m, pd, o2, a),
        Inst::Store(m, pd, o1, b),
        Inst::SlotAddr(sa, slot),
        Inst::Store(m, sa, 0, a),
        Inst::Store(m, sa, w, b),
        Inst::Load(m, c, sa, 0),
        Inst::Load(m, d, sa, w),
        Inst::Store(m, pd, 16, c),
        Inst::Store(m, pd, 16 + w, d),
    ];
    for i in insts {
        f.blocks[0].insts.push(i);
        f.blocks[0].pos.push(0);
    }
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    f
}

#[test]
fn paired_accesses_move_the_same_words() {
    let src: Vec<u8> = (0..32u32).map(|x| (x * 37 + 200) as u8).collect();
    for (m, w) in [(Mem::Int(8, false), 8), (Mem::Int(4, false), 4), (Mem::F64, 8), (Mem::F32, 4)] {
        for swap in [false, true] {
            let code = compile(&func(m, w, swap));
            let run: extern "C" fn(*const u8, *mut u8) = unsafe { std::mem::transmute(code.ptr()) };
            let mut dst = vec![0u8; 32];
            run(src.as_ptr(), dst.as_mut_ptr());
            let w = w as usize;
            let (o1, o2) = if swap { (w, 0) } else { (0, w) };
            let mut want = vec![0u8; 32];
            want[o2..o2 + w].copy_from_slice(&src[o1..o1 + w]);
            want[o1..o1 + w].copy_from_slice(&src[o2..o2 + w]);
            want[16..16 + w].copy_from_slice(&src[o1..o1 + w]);
            want[16 + w..16 + 2 * w].copy_from_slice(&src[o2..o2 + w]);
            assert_eq!(dst, want, "{m:?} swap {swap}");
        }
    }
}
