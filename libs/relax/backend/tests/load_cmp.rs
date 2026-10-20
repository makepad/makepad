//! `t = load [p + o]; branch on t cmp K` (t used once) compiles to `cmp <width> [p + o], K`: the
//! compare must decide as the extended 64-bit value would, for every width, extension,
//! condition and constant the fold accepts (and stay a register compare otherwise).
#![cfg(target_arch = "x86_64")]
use relax_backend::rir::*;
use relax_backend::x64;

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

/// f(p) = if load<sz, sx>(p + 8) cond k { 1 } else { 2 }
fn cmp_fn(sz: u8, sx: bool, cond: Cond, signed: bool, k: i64) -> Func {
    let mut f = Func::new("load_cmp".to_string(), 0);
    let p = f.vreg(Cls::I);
    f.params = vec![p];
    f.rets = vec![Cls::I];
    let (b0, b1, b2) = (f.block(), f.block(), f.block());
    let (t, c, one, two) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b0, Inst::Load(Mem::Int(sz, sx), t, p, 8));
    push(&mut f, b0, Inst::ICmpI(cond, signed, c, t, k));
    f.blocks[b0 as usize].term = Term::Branch(c, b1, b2);
    push(&mut f, b1, Inst::Iconst(one, 1));
    f.blocks[b1 as usize].term = Term::Ret(vec![one]);
    push(&mut f, b2, Inst::Iconst(two, 2));
    f.blocks[b2 as usize].term = Term::Ret(vec![two]);
    f.verify().unwrap();
    f
}

fn extend(bytes: &[u8; 16], sz: u8, sx: bool) -> i64 {
    let mut v = 0u64;
    for i in 0..sz as usize {
        v |= (bytes[8 + i] as u64) << (8 * i);
    }
    let b = sz as u32 * 8;
    if b < 64 && sx && v >> (b - 1) & 1 == 1 {
        v |= !0u64 << b;
    }
    v as i64
}

fn holds(c: Cond, signed: bool, a: i64, b: i64) -> bool {
    let (ua, ub) = (a as u64, b as u64);
    match (c, signed) {
        (Cond::Eq, _) => a == b,
        (Cond::Ne, _) => a != b,
        (Cond::Lt, true) => a < b,
        (Cond::Le, true) => a <= b,
        (Cond::Gt, true) => a > b,
        (Cond::Ge, true) => a >= b,
        (Cond::Lt, false) => ua < ub,
        (Cond::Le, false) => ua <= ub,
        (Cond::Gt, false) => ua > ub,
        (Cond::Ge, false) => ua >= ub,
    }
}

#[test]
fn load_compare_branches_as_the_extended_value() {
    let env = x64::Env { slots: relax_backend::rir::SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
    let vals: [u64; 9] = [0, 1, 2, 0x7f, 0x80, 0xff, 0x7fff_ffff, 0x8000_0000, u64::MAX];
    let ks: [i64; 13] = [0, 1, 2, -1, -2, 0x7f, 0x80, 0xff, -0x80, 0x7fff, 0xffff, 0x7fff_ffff, -0x8000_0000];
    let conds = [Cond::Eq, Cond::Ne, Cond::Lt, Cond::Le, Cond::Gt, Cond::Ge];
    let mut n = 0;
    for sz in [1u8, 2, 4, 8] {
        for sx in [false, true] {
            for cond in conds {
                for signed in [false, true] {
                    for &k in &ks {
                        let f = cmp_fn(sz, sx, cond, signed, k);
                        let c = x64::compile(&relax_session::Options::default(), &f, &env).unwrap();
                        let code = relax_backend::exec::Code::new(&c.code).unwrap();
                        let run: extern "sysv64" fn(*const u8) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
                        for &v in &vals {
                            let mut buf = [0xa5u8; 16];
                            buf[8..16].copy_from_slice(&v.to_le_bytes());
                            let want = if holds(cond, signed, extend(&buf, sz, sx), k) { 1 } else { 2 };
                            assert_eq!(run(buf.as_ptr()), want, "sz {sz} sx {sx} {cond:?} signed {signed} k {k} v {v:#x}");
                            n += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(n > 0);
}

/// f(p) = if load<sz>(p + 8) { 1 } else { 2 }: the branch on a loaded value itself
fn branch_fn(sz: u8, sx: bool) -> Func {
    let mut f = Func::new("load_branch".to_string(), 0);
    let p = f.vreg(Cls::I);
    f.params = vec![p];
    f.rets = vec![Cls::I];
    let (b0, b1, b2) = (f.block(), f.block(), f.block());
    let (t, one, two) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, b0, Inst::Load(Mem::Int(sz, sx), t, p, 8));
    f.blocks[b0 as usize].term = Term::Branch(t, b1, b2);
    push(&mut f, b1, Inst::Iconst(one, 1));
    f.blocks[b1 as usize].term = Term::Ret(vec![one]);
    push(&mut f, b2, Inst::Iconst(two, 2));
    f.blocks[b2 as usize].term = Term::Ret(vec![two]);
    f.verify().unwrap();
    f
}

#[test]
fn branch_on_a_loaded_value_tests_its_bytes() {
    let env = x64::Env { slots: relax_backend::rir::SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
    for sz in [1u8, 2, 4, 8] {
        for sx in [false, true] {
            let c = x64::compile(&relax_session::Options::default(), &branch_fn(sz, sx), &env).unwrap();
            let code = relax_backend::exec::Code::new(&c.code).unwrap();
            let run: extern "sysv64" fn(*const u8) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
            for v in [0u64, 1, 0x80, 0x100, 0x1_0000, 0x1_0000_0000, u64::MAX] {
                let mut buf = [0u8; 16];
                buf[8..16].copy_from_slice(&v.to_le_bytes());
                let low = if sz == 8 { v } else { v & ((1u64 << (sz as u32 * 8)) - 1) };
                assert_eq!(run(buf.as_ptr()), if low != 0 { 1 } else { 2 }, "sz {sz} sx {sx} v {v:#x}");
            }
        }
    }
}
