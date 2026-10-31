//! IOp::Bzhi (`a & ((1 << (b mod 64)) - 1)`): the host backend's code (x64 BMI2 bzhi or its
//! fallback, arm64 movn/lslv/bic) equals the constant folder for counts 0..=255 and wide values,
//! also when the result shares a register with an operand.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func) -> Vec<u8> {
    let mut o = relax_session::Options::default();
    o.cpu = relax_backend::x64::detect_cpu(&o);
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile(&o, f, &env).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

#[cfg(target_arch = "aarch64")]
fn compile(f: &Func) -> Vec<u8> {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::arm64::compile(&relax_session::Options::default(), f, &env).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

fn push(f: &mut Func, i: Inst) {
    f.blocks[0].insts.push(i);
    f.blocks[0].pos.push(0);
}

/// f(a, b) = (a bzhi b) ^ ((b bzhi a) << 1) ^ (a bzhi 37 [constant count]): register, swapped and
/// constant-count forms in one body
fn build() -> Func {
    let mut f = Func::new("bzhi".into(), 0);
    let (a, b) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![a, b];
    f.rets = vec![Cls::I];
    f.block();
    let v: Vec<VReg> = (0..7).map(|_| f.vreg(Cls::I)).collect();
    push(&mut f, Inst::IBin(IOp::Bzhi, U64, v[0], a, b));
    push(&mut f, Inst::IBin(IOp::Bzhi, U64, v[1], b, a));
    push(&mut f, Inst::IBinI(IOp::Shl, U64, v[2], v[1], 1));
    push(&mut f, Inst::IBinI(IOp::Bzhi, U64, v[3], a, 37));
    push(&mut f, Inst::IBin(IOp::Xor, U64, v[4], v[0], v[2]));
    push(&mut f, Inst::IBin(IOp::Xor, U64, v[5], v[4], v[3]));
    f.blocks[0].term = Term::Ret(vec![v[5]]);
    f.verify().unwrap();
    f
}

fn bz(a: u64, b: u64) -> u64 {
    relax_backend::passes::fold_ibin(IOp::Bzhi, U64, a as i64, b as i64).unwrap() as u64
}

#[test]
fn bzhi_matches_the_folder() {
    type Entry = extern "C" fn(u64, u64) -> u64;
    let code = Code::new(&compile(&build())).unwrap();
    let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
    let vals = [0u64, 1, 0x8000_0000_0000_0001, u64::MAX, 0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210];
    for &a in &vals {
        for b in 0..256u64 {
            let want = bz(a, b) ^ (bz(b, a) << 1) ^ bz(a, 37);
            assert_eq!(run(a, b), want, "a {a:#x} b {b}");
        }
    }
    // the folder itself: Rust's `(1u64 << (n & 63)) - 1` mask
    for n in 0..200u64 {
        assert_eq!(bz(u64::MAX, n), (1u64 << (n & 63)).wrapping_sub(1));
    }
}
