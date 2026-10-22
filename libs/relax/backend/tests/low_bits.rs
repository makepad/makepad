//! Narrow results whose upper register bits nobody reads skip their re-extension
//! (passes::low_bits_only): a u8 subtract stored back as a byte, and a u8 cast used as a 64-bit
//! shift count. A narrow value that is also compared keeps its extension. Results must equal
//! Rust's.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func) -> Vec<u8> {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile(&relax_session::Options::default(), f, &env).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

#[cfg(target_arch = "aarch64")]
fn compile(f: &Func) -> Vec<u8> {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::arm64::compile(&relax_session::Options::default(), f, &env).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// f(p, x, n): p[0] = (p[0] as u8 - n as u8); s = x >> (n as u8) (64-bit shift); c = (x as u8 - n as u8);
/// returns s + (c < 16) + p[0] (the stored byte read back)
fn build() -> Func {
    let u8t = IntTy { bits: 8, signed: false };
    let w = IntTy { bits: 64, signed: false };
    let mut f = Func::new("low_bits".into(), 0);
    let (p, x, n) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![p, x, n];
    f.rets = vec![Cls::I];
    let b = f.block();
    let v: Vec<VReg> = (0..12).map(|_| f.vreg(Cls::I)).collect();
    push(&mut f, b, Inst::Conv(Conv::IntToInt(u8t), v[0], n));
    push(&mut f, b, Inst::Load(Mem::Int(1, false), v[1], p, 0));
    push(&mut f, b, Inst::IBin(IOp::Sub, u8t, v[2], v[1], v[0]));
    push(&mut f, b, Inst::Store(Mem::Int(1, false), p, 0, v[2]));
    push(&mut f, b, Inst::IBin(IOp::Shr, w, v[3], x, v[0]));
    push(&mut f, b, Inst::Conv(Conv::IntToInt(u8t), v[4], x));
    push(&mut f, b, Inst::IBin(IOp::Sub, u8t, v[5], v[4], v[0]));
    push(&mut f, b, Inst::ICmpI(Cond::Lt, false, v[6], v[5], 16));
    push(&mut f, b, Inst::IBin(IOp::Add, w, v[7], v[3], v[6]));
    push(&mut f, b, Inst::Load(Mem::Int(1, false), v[8], p, 0));
    push(&mut f, b, Inst::IBin(IOp::Add, w, v[9], v[7], v[8]));
    f.blocks[b as usize].term = Term::Ret(vec![v[9]]);
    f.verify().unwrap();
    f
}

#[test]
fn narrow_results_read_only_low_bits() {
    let code = Code::new(&compile(&build())).unwrap();
    let run: extern "C" fn(*mut u8, u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
    let mut s = 0x9e3779b97f4a7c15u64;
    for _ in 0..4000 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let (x, n, b0) = (s.rotate_left(29), s & 0x3f | (s & 0xff00), (s >> 40) as u8);
        let mut byte = b0;
        let got = run(&mut byte, x, n);
        let stored = b0.wrapping_sub(n as u8);
        let want = (x >> ((n as u8) & 63)) + (((x as u8).wrapping_sub(n as u8) < 16) as u64) + stored as u64;
        assert_eq!(byte, stored);
        assert_eq!(got, want, "x {:#x} n {:#x}", x, n);
    }
}

/// g(p, x, y): a = x as i8 - y as i8; b = a ^ 0x55 (i8); r = b as i64 (reads b's full register);
/// c = x as i16 + y as i16; d = c | (y as i16); p[0] = d as a byte (only low bits read);
/// returns r. An And / Or / Xor result is never re-extended, so its operands keep theirs unless
/// the result itself reads only low bits (x64 runs these ops in 64-bit forms).
fn build_bitwise() -> Func {
    let i8t = IntTy { bits: 8, signed: true };
    let i16t = IntTy { bits: 16, signed: true };
    let w = IntTy { bits: 64, signed: true };
    let mut f = Func::new("low_bits_bitwise".into(), 0);
    let (p, x, y) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![p, x, y];
    f.rets = vec![Cls::I];
    let b = f.block();
    let v: Vec<VReg> = (0..10).map(|_| f.vreg(Cls::I)).collect();
    push(&mut f, b, Inst::Conv(Conv::IntToInt(i8t), v[0], x));
    push(&mut f, b, Inst::Conv(Conv::IntToInt(i8t), v[1], y));
    push(&mut f, b, Inst::IBin(IOp::Sub, i8t, v[2], v[0], v[1]));
    push(&mut f, b, Inst::IBinI(IOp::Xor, i8t, v[3], v[2], 0x55));
    push(&mut f, b, Inst::Conv(Conv::IntToInt(w), v[4], v[3]));
    push(&mut f, b, Inst::Conv(Conv::IntToInt(i16t), v[5], x));
    push(&mut f, b, Inst::Conv(Conv::IntToInt(i16t), v[6], y));
    push(&mut f, b, Inst::IBin(IOp::Add, i16t, v[7], v[5], v[6]));
    push(&mut f, b, Inst::IBin(IOp::Or, i16t, v[8], v[7], v[6]));
    push(&mut f, b, Inst::Store(Mem::Int(1, false), p, 0, v[8]));
    f.blocks[b as usize].term = Term::Ret(vec![v[4]]);
    f.verify().unwrap();
    f
}

#[test]
fn bitwise_results_keep_operand_extension() {
    let code = Code::new(&compile(&build_bitwise())).unwrap();
    let run: extern "C" fn(*mut u8, i64, i64) -> i64 = unsafe { std::mem::transmute(code.ptr()) };
    let mut s = 0x2545f4914f6cdd1du64;
    for _ in 0..4000 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let (x, y) = (s as i64, s.rotate_left(23) as i64);
        let mut byte = 0u8;
        let got = run(&mut byte, x, y);
        let want = ((x as i8).wrapping_sub(y as i8) ^ 0x55) as i64;
        let c = (x as i16).wrapping_add(y as i16) | (y as i16);
        assert_eq!(byte, c as u8);
        assert_eq!(got, want, "x {:#x} y {:#x}", x, y);
    }
}
