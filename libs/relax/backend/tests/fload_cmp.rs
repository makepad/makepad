//! `t = load f32/f64 [p + o]; branch on fcmp x, y` with t one of them (used once) compiles to
//! `ucomiss/ucomisd reg, [p + o]` where t is the operand the compare reads from r/m: the branch
//! must decide as the two-register compare does for every condition, operand order and value,
//! NaN and signed zeros included (and stay a register compare where the load is the other side).
#![cfg(target_arch = "x86_64")]
use relax_backend::rir::*;
use relax_backend::x64;

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// f(p) = if (a = load(p), t = load(p + 8); cond on (a, t) or (t, a)) { 1 } else { 2 }
fn cmp_fn(f64_: bool, cond: Cond, t_first: bool) -> Func {
    let mut f = Func::new("fload_cmp".to_string(), 0);
    let p = f.vreg(Cls::I);
    f.params = vec![p];
    f.rets = vec![Cls::I];
    let (b0, b1, b2) = (f.block(), f.block(), f.block());
    let fc = if f64_ { Cls::F64 } else { Cls::F32 };
    let (a, t, c, one, two) = (f.vreg(fc), f.vreg(fc), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    let m = if f64_ { Mem::F64 } else { Mem::F32 };
    push(&mut f, b0, Inst::Load(m, a, p, 0));
    push(&mut f, b0, Inst::Load(m, t, p, 8));
    let (x, y) = if t_first { (t, a) } else { (a, t) };
    push(&mut f, b0, Inst::FCmp(cond, f64_, c, x, y));
    f.blocks[b0 as usize].term = Term::Branch(c, b1, b2);
    push(&mut f, b1, Inst::Iconst(one, 1));
    f.blocks[b1 as usize].term = Term::Ret(vec![one]);
    push(&mut f, b2, Inst::Iconst(two, 2));
    f.blocks[b2 as usize].term = Term::Ret(vec![two]);
    f.verify().unwrap();
    f
}

fn holds(c: Cond, x: f64, y: f64) -> bool {
    match c {
        Cond::Eq => x == y,
        Cond::Ne => x != y,
        Cond::Lt => x < y,
        Cond::Le => x <= y,
        Cond::Gt => x > y,
        Cond::Ge => x >= y,
    }
}

#[test]
fn float_load_compare_branches_as_the_register_compare() {
    let env = x64::Env { slots: relax_backend::rir::SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let vals: [f64; 8] = [0.0, -0.0, 1.0, -1.5, 2.5, f64::INFINITY, f64::NEG_INFINITY, f64::NAN];
    let conds = [Cond::Eq, Cond::Ne, Cond::Lt, Cond::Le, Cond::Gt, Cond::Ge];
    let mut n = 0;
    for f64_ in [false, true] {
        for cond in conds {
            for t_first in [false, true] {
                let f = cmp_fn(f64_, cond, t_first);
                let c = x64::compile(&relax_session::Options::default(), &f, &env).unwrap();
                // ucomis* reg, [p + 8] (0F 2E, modrm mod 01, disp8 8) exactly where t is the r/m operand
                let fused = c.code.windows(4).any(|w| w[0] == 0x0f && w[1] == 0x2e && w[2] >> 6 == 1 && w[3] == 8);
                let want_fused = match cond { Cond::Eq | Cond::Ne => true, Cond::Lt | Cond::Le => t_first, Cond::Gt | Cond::Ge => !t_first };
                assert_eq!(fused, want_fused, "f64 {f64_} {cond:?} t_first {t_first}");
                let code = relax_backend::exec::Code::new(&c.code).unwrap();
                let run: extern "sysv64" fn(*const u8) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
                for &av in &vals {
                    for &tv in &vals {
                        let mut buf = [0u8; 16];
                        if f64_ {
                            buf[0..8].copy_from_slice(&av.to_le_bytes());
                            buf[8..16].copy_from_slice(&tv.to_le_bytes());
                        } else {
                            buf[0..4].copy_from_slice(&(av as f32).to_le_bytes());
                            buf[8..12].copy_from_slice(&(tv as f32).to_le_bytes());
                        }
                        let (x, y) = if t_first { (tv, av) } else { (av, tv) };
                        let want = if holds(cond, x, y) { 1 } else { 2 };
                        assert_eq!(run(buf.as_ptr()), want, "f64 {f64_} {cond:?} t_first {t_first} a {av} t {tv}");
                        n += 1;
                    }
                }
            }
        }
    }
    assert!(n > 0);
}
