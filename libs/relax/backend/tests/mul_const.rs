//! Multiplies by constants the backends strength-reduce (arm64: 2^k + 1 as one shifted add,
//! 2^k - 1 as a shift and a subtract, powers of two as shifts) against Rust's wrapping
//! multiply, for i64 / u32 / i32 / u8 results, run on the host.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func) -> Code {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    Code::new(&relax_backend::x64::compile(&relax_session::Options::default(), f, &env).unwrap().code).unwrap()
}

#[cfg(target_arch = "aarch64")]
fn compile(f: &Func) -> Code {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    Code::new(&relax_backend::arm64::compile(&relax_session::Options::default(), f, &env).unwrap().code).unwrap()
}

fn func(it: IntTy, k: i64) -> Func {
    let mut f = Func::new("mul_const".into(), 0);
    let x = f.vreg(Cls::I);
    f.params = vec![x];
    f.rets = vec![Cls::I];
    f.block();
    let d = f.vreg(Cls::I);
    f.blocks[0].insts.push(Inst::IBinI(IOp::Mul, it, d, x, k));
    f.blocks[0].pos.push(0);
    f.blocks[0].term = Term::Ret(vec![d]);
    f.verify().unwrap();
    f
}

#[test]
fn constant_multiplies_wrap_like_rust() {
    let tys = [IntTy { bits: 64, signed: true }, U32, IntTy { bits: 32, signed: true }, IntTy { bits: 8, signed: false }];
    let xs: [u64; 6] = [0, 1, 7, 0x7fff_ffff, 0xdead_beef_1234_5678, u64::MAX];
    for it in tys {
        for k in [2i64, 3, 5, 7, 9, 15, 17, 31, 33, 255, 257, 1023, 1 << 20, (1 << 30) + 1, (1i64 << 31) - 1, 6, 10] {
            let code = compile(&func(it, k));
            let run: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
            for x in xs {
                // inputs are extended like the backends keep them (by the type's signedness)
                let xin = match (it.bits, it.signed) {
                    (64, _) => x,
                    (32, false) => x as u32 as u64,
                    (32, true) => x as u32 as i32 as i64 as u64,
                    _ => x as u8 as u64,
                };
                let p = xin.wrapping_mul(k as u64);
                let want = match (it.bits, it.signed) {
                    (64, _) => p,
                    (32, false) => p as u32 as u64,
                    (32, true) => p as u32 as i32 as i64 as u64,
                    _ => p as u8 as u64,
                };
                assert_eq!(run(xin), want, "{it:?} x {x:#x} * {k}");
            }
        }
    }
}

/// Integer constants of every shape (one 16-bit half, movn forms, logical immediates such as
/// SWAR masks, arbitrary 64-bit values) come back exactly.
#[test]
fn integer_constants_materialise_exactly() {
    let vals: [u64; 14] = [0, 1, 0xffff, 0x1_0000, 0xffff_ffff_ffff_ffff, 0xffff_ffff_ffff_0000, 0x8080_8080_8080_8080, 0x0101_0101_0101_0101, 0x00ff_00ff_00ff_00ff, 0x7f7f_7f7f_7f7f_7f7f, 0x5555_5555_5555_5555, 0x0000_ffff_0000_ffff, 0xdead_beef_cafe_f00d, 0x0f0f_0f0f_0f0f_0f0e];
    for v in vals {
        let mut f = Func::new("k".into(), 0);
        f.params = vec![];
        f.rets = vec![Cls::I];
        f.block();
        let d = f.vreg(Cls::I);
        f.blocks[0].insts.push(Inst::Iconst(d, v as i64));
        f.blocks[0].pos.push(0);
        f.blocks[0].term = Term::Ret(vec![d]);
        f.verify().unwrap();
        let code = compile(&f);
        let run: extern "C" fn() -> u64 = unsafe { std::mem::transmute(code.ptr()) };
        assert_eq!(run(), v, "{v:#x}");
    }
}
