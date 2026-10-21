//! A remainder next to a division of the same operands is computed from the quotient
//! (passes::div_rem_pairs), in either order, for unsigned and signed narrow types; results must
//! equal Rust's.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func) -> Vec<u8> {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
    let c = relax_backend::x64::compile(&relax_session::Options::default(), f, &env).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

#[cfg(target_arch = "aarch64")]
fn compile(f: &Func) -> Vec<u8> {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
    let c = relax_backend::arm64::compile(&relax_session::Options::default(), f, &env).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// f(a, b) = (a / b) * 1000 + a % b in `it`, the remainder first when `rem_first`; every value
/// extended to 64 bits by its type
fn build(it: IntTy, rem_first: bool) -> Func {
    let w = IntTy { bits: 64, signed: false };
    let mut f = Func::new("div_rem_pairs".into(), 0);
    let (a, b) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![a, b];
    f.rets = vec![Cls::I];
    let blk = f.block();
    let v: Vec<VReg> = (0..6).map(|_| f.vreg(Cls::I)).collect();
    push(&mut f, blk, Inst::Conv(Conv::IntToInt(it), v[0], a));
    push(&mut f, blk, Inst::Conv(Conv::IntToInt(it), v[1], b));
    let (div, rem) = (Inst::IBin(IOp::Div, it, v[2], v[0], v[1]), Inst::IBin(IOp::Rem, it, v[3], v[0], v[1]));
    if rem_first {
        push(&mut f, blk, rem);
        push(&mut f, blk, Inst::IBinI(IOp::Mul, w, v[4], v[3], 7));
        push(&mut f, blk, div);
    } else {
        push(&mut f, blk, div);
        push(&mut f, blk, Inst::IBinI(IOp::Mul, w, v[4], v[2], 7));
        push(&mut f, blk, rem);
    }
    push(&mut f, blk, Inst::IBinI(IOp::Mul, w, v[5], v[2], 1000));
    push(&mut f, blk, Inst::IBin(IOp::Add, w, v[5], v[5], v[3]));
    push(&mut f, blk, Inst::IBin(IOp::Add, w, v[5], v[5], v[4]));
    f.blocks[blk as usize].term = Term::Ret(vec![v[5]]);
    f.verify().unwrap();
    f
}

fn divs(f: &Func) -> usize {
    f.blocks.iter().flat_map(|b| b.insts.iter()).filter(|i| matches!(i, Inst::IBin(IOp::Div | IOp::Rem, ..))).count()
}

#[test]
fn remainder_from_quotient() {
    let tys = [IntTy { bits: 64, signed: false }, IntTy { bits: 32, signed: false }, IntTy { bits: 32, signed: true }, IntTy { bits: 16, signed: true }];
    for it in tys {
        for rem_first in [false, true] {
            let mut f = build(it, rem_first);
            assert!(relax_backend::passes::div_rem_pairs(&mut f));
            f.verify().unwrap();
            assert_eq!(divs(&f), 1, "{}", f.dump());
            let code = Code::new(&compile(&f)).unwrap();
            let run: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
            let mut s = 0x9e3779b97f4a7c15u64;
            for _ in 0..2000 {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                let (x, mut y) = (s, s.rotate_left(23) >> (s % 60));
                // narrow the operands to the type (and avoid zero / MIN / -1)
                let ext = |v: u64| -> i128 {
                    let sh = 64 - it.bits as u32;
                    if it.signed { (((v << sh) as i64) >> sh) as i128 } else { ((v << sh) >> sh) as i128 }
                };
                if ext(y) == 0 || ext(y) == -1 {
                    y = 3;
                }
                let (xa, yb) = (ext(x), ext(y));
                let (q, r) = (xa / yb, xa % yb);
                let k = if rem_first { r } else { q };
                let want = (q as u64).wrapping_mul(1000).wrapping_add(r as u64).wrapping_add((k as u64).wrapping_mul(7));
                assert_eq!(run(x, y), want, "{:?} rem_first {} x {:#x} y {:#x}", it, rem_first, x, y);
            }
        }
    }
}
