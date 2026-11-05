//! `x.min(y)` and `x.max(y)` of one compare with arithmetic between them (a box distance:
//! `(min - p).max(p - max)`): the second select moves next to the first (regalloc::pair_selects),
//! and on x64 both cmovs read the compare's flags (no setcc to keep the condition, no test to read
//! it back). Results must equal Rust's on both targets.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func) -> Vec<u8> {
    let o = relax_session::Options::default();
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

const W: IntTy = IntTy { bits: 64, signed: true };

fn push(f: &mut Func, i: Inst) {
    f.blocks[0].insts.push(i);
    f.blocks[0].pos.push(0);
}

/// f(a, b, p) = max(min(a, b) - p, p - max(a, b)) + k: one compare for min and max
fn box_gap(k: i64) -> Func {
    let mut f = Func::new("box_gap".into(), 0);
    let (a, b, p) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![a, b, p];
    f.rets = vec![Cls::I];
    f.block();
    let v: Vec<VReg> = (0..8).map(|_| f.vreg(Cls::I)).collect();
    push(&mut f, Inst::ICmp(Cond::Lt, true, v[0], a, b));
    push(&mut f, Inst::Sel(v[1], v[0], a, b));
    push(&mut f, Inst::IBin(IOp::Sub, W, v[2], v[1], p));
    push(&mut f, Inst::Sel(v[3], v[0], b, a));
    push(&mut f, Inst::IBin(IOp::Sub, W, v[4], p, v[3]));
    push(&mut f, Inst::ICmp(Cond::Gt, true, v[5], v[2], v[4]));
    push(&mut f, Inst::Sel(v[6], v[5], v[2], v[4]));
    push(&mut f, Inst::IBinI(IOp::Add, W, v[7], v[6], k));
    f.blocks[0].term = Term::Ret(vec![v[7]]);
    f.verify().unwrap();
    f
}

#[test]
fn min_and_max_of_one_compare_sit_together() {
    let g = relax_backend::regalloc::pair_selects(&box_gap(0)).expect("the max select moves up");
    let sels: Vec<usize> = g.blocks[0].insts.iter().enumerate().filter(|(_, i)| matches!(i, Inst::Sel(_, c, ..) if *c == VReg(3))).map(|(k, _)| k).collect();
    assert_eq!(sels, vec![1, 2], "{:?}", g.blocks[0].insts);
    assert!(g.verify().is_ok());
}

#[test]
fn box_gap_matches_rust() {
    const VALS: [i64; 9] = [0, 1, -1, 7, -7, 1 << 40, -(1 << 40), i64::MAX / 4, i64::MIN / 4];
    for k in [0, 5] {
        let code = Code::new(&compile(&box_gap(k))).unwrap();
        let run: extern "C" fn(i64, i64, i64) -> i64 = unsafe { std::mem::transmute(code.ptr()) };
        for &a in &VALS {
            for &b in &VALS {
                for &p in &VALS {
                    let want = (a.min(b) - p).max(p - a.max(b)) + k;
                    assert_eq!(run(a, b, p), want, "a {a} b {b} p {p} k {k}");
                }
            }
        }
    }
}

/// x64: no setcc (0F 90+cc with a register operand) -- both cmovs read the compare's flags
#[cfg(target_arch = "x86_64")]
#[test]
fn no_condition_kept_in_a_register() {
    let code = compile(&box_gap(5));
    let setcc = code.windows(3).filter(|w| w[0] == 0x0f && (0x90..=0x9f).contains(&w[1]) && w[2] >= 0xc0).count();
    assert_eq!(setcc, 0, "{:02x?}", code);
}
