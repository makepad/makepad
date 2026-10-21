//! u32 / u16 byte swaps and u32 bit reversal as RIR has them (64-bit `IBit` then a shift down):
//! arm64 emits `rev w` / `rev16 w` (a zero-extended u16 from a narrow load) / `rbit w`; results
//! against Rust's swap_bytes / reverse_bits, run on the host.
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

/// fn(p: *const u64) -> u64 { x = load(p) (8 bytes, or 2 zero-extended); op(x) >> sh }
fn func(op: BitOp, load: u8, sh: i64) -> Func {
    let mut f = Func::new("narrow".into(), 0);
    let p = f.vreg(Cls::I);
    f.params = vec![p];
    f.rets = vec![Cls::I];
    f.block();
    let (x, t, d) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    for i in [Inst::Load(Mem::Int(load, false), x, p, 0), Inst::IBit(op, t, x), Inst::IBinI(IOp::Shr, IntTy { bits: 64, signed: false }, d, t, sh)] {
        f.blocks[0].insts.push(i);
        f.blocks[0].pos.push(0);
    }
    f.blocks[0].term = Term::Ret(vec![d]);
    f.verify().unwrap();
    f
}

#[test]
fn narrow_byte_swaps_and_bit_reversals() {
    for x in [0u64, 1, 0x1234, 0x8001, 0x1234_5678, 0xdead_beef_cafe_f00d, u64::MAX] {
        let cases: [(BitOp, u8, i64, u64); 5] = [
            (BitOp::Bswap, 8, 32, (x as u32).swap_bytes() as u64),
            (BitOp::Bswap, 2, 48, (x as u16).swap_bytes() as u64),
            (BitOp::Bswap, 8, 48, (x as u16).swap_bytes() as u64),
            (BitOp::Rev, 8, 32, (x as u32).reverse_bits() as u64),
            (BitOp::Bswap, 8, 16, x.swap_bytes() >> 16),
        ];
        for (op, load, sh, want) in cases {
            let code = compile(&func(op, load, sh));
            let run: extern "C" fn(*const u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
            assert_eq!(run(&x), want, "{op:?} load {load} >> {sh} of {x:#x}");
        }
    }
}
