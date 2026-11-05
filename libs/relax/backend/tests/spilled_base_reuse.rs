//! Two loads in a row through one spilled base (a grid's `at[c]`, `at[c + 1]` with the `at`
//! pointer on the stack): the x64 emitter reloads the base into r11 once, and the second load
//! reads r11 again (`mov r11, [rbp-0x60]` was emitted twice back to back). Results must equal
//! the reference.
#![cfg(all(target_arch = "x86_64", unix))]
use relax_backend::regalloc::{allocate_split, Loc};
use relax_backend::{exec::Code, rir::*};

const W: IntTy = IntTy { bits: 64, signed: false };
/// temporaries live over the loads: more than the registers, so the base spills
const T: usize = 40;

fn options() -> relax_session::Options {
    let mut o = relax_session::Options::default();
    o.opt.push("+checkra".into());
    o
}

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// f(p, q): t_m = q + m * 97 (m < T, all live); two rounds of acc = rotl(acc ^ t_m, 5) over every
/// m; q1 = q + 1; a = p[q]; b = p[q1] (u32); acc += a + b * 7; a third round; acc. The base is
/// read only by the loads (the temporaries, read three times each, keep the registers).
fn build() -> (Func, VReg) {
    let mut f = Func::new("spilled_base_reuse".into(), 0);
    let (p, q) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![p, q];
    f.rets = vec![Cls::I];
    let b0 = f.block();
    let t: Vec<VReg> = (0..T).map(|_| f.vreg(Cls::I)).collect();
    for m in 0..T {
        push(&mut f, b0, Inst::IBinI(IOp::Add, W, t[m], q, m as i64 * 97));
    }
    let (q1, a, b, b7, ab, acc) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b0, Inst::Iconst(acc, 0));
    let round = |f: &mut Func| {
        for m in 0..T {
            let x = f.vreg(Cls::I);
            push(f, b0, Inst::IBin(IOp::Xor, W, x, acc, t[m]));
            push(f, b0, Inst::IBinI(IOp::Rotl, W, acc, x, 5));
        }
    };
    round(&mut f);
    round(&mut f);
    push(&mut f, b0, Inst::IBinI(IOp::Add, W, q1, q, 1));
    push(&mut f, b0, Inst::LoadX(Mem::Int(4, false), a, p, q, 2, 0));
    push(&mut f, b0, Inst::LoadX(Mem::Int(4, false), b, p, q1, 2, 0));
    push(&mut f, b0, Inst::IBinI(IOp::Mul, W, b7, b, 7));
    push(&mut f, b0, Inst::IBin(IOp::Add, W, ab, a, b7));
    push(&mut f, b0, Inst::IBin(IOp::Add, W, acc, acc, ab));
    round(&mut f);
    f.blocks[b0 as usize].term = Term::Ret(vec![acc]);
    f.verify().unwrap();
    (f, p)
}

fn reference(p: &[u32], q: u64) -> u64 {
    let (a, b) = (p[q as usize] as u64, p[q as usize + 1] as u64);
    let mut acc = 0u64;
    for r in 0..3 {
        if r == 2 {
            acc = acc.wrapping_add(a.wrapping_add(b.wrapping_mul(7)));
        }
        for m in 0..T as u64 {
            acc = (acc ^ q.wrapping_add(m * 97)).rotate_left(5);
        }
    }
    acc
}

fn compile(f: &Func) -> Vec<u8> {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile_freq(&options(), f, &env, None).unwrap();
    c.code[..c.text_len as usize].to_vec()
}

/// every `mov r11, [rbp + d]` (disp8 / disp32 forms): (offset, length, d)
fn r11_reloads(code: &[u8]) -> Vec<(usize, usize, i32)> {
    let mut v = Vec::new();
    for k in 0..code.len().saturating_sub(3) {
        if code[k] == 0x4c && code[k + 1] == 0x8b {
            match code[k + 2] {
                0x5d => v.push((k, 4, code[k + 3] as i8 as i32)),
                0x9d if k + 7 <= code.len() => v.push((k, 7, i32::from_le_bytes([code[k + 3], code[k + 4], code[k + 5], code[k + 6]]))),
                _ => {}
            }
        }
    }
    v
}

#[test]
fn the_second_load_through_a_spilled_base_reads_r11_again() {
    let (f, p) = build();
    let (g, al) = allocate_split(&options(), &f, &relax_backend::x64::reg_config(&relax_session::Options::default()), true, None);
    let _ = g;
    assert!(matches!(al.loc[p.0 as usize], Loc::Stack(_)), "the fixture needs the base spilled: {:?}", al.loc[p.0 as usize]);
    let code = compile(&f);
    let rl = r11_reloads(&code);
    assert!(!rl.is_empty(), "no reload into r11 at all: the fixture lost its spill");
    // the same slot again right after one instruction (a load through r11: at most 6 bytes)
    let again = rl.windows(2).filter(|w| w[0].2 == w[1].2 && w[1].0 <= w[0].0 + w[0].1 + 6).count();
    assert_eq!(again, 0, "a slot reloaded into r11 right after its last reload: {:?}", rl);
    let mem: Vec<u32> = (0..64u32).map(|k| k.wrapping_mul(2654435761)).collect();
    let c = Code::new(&code).unwrap();
    let run: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(c.ptr()) };
    for q in [0u64, 1, 17, 62] {
        assert_eq!(run(mem.as_ptr() as u64, q), reference(&mem, q), "q {}", q);
    }
}
