//! merge_copy_runs: a struct copied field by field (loads from one base, each value stored at the
//! same distance from another) becomes one Copy; an intervening write that may touch either range,
//! a read of the destination in between, or a call keeps the fields. Results compiled before and
//! after the pass must agree.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};
use relax_codegen::opt;

const U64: IntTy = IntTy { bits: 64, signed: false };
fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(1);
}

#[cfg(target_arch = "x86_64")]
fn run(f: &Func, p: u64) -> u64 {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile(&relax_session::Options::default(), f, &env).unwrap();
    let code = Code::new(&c.code[..c.text_len as usize]).unwrap();
    let g: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
    g(p)
}

#[cfg(target_arch = "aarch64")]
fn run(f: &Func, p: u64) -> u64 {
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::arm64::compile(&relax_session::Options::default(), f, &env).unwrap();
    let code = Code::new(&c.code[..c.text_len as usize]).unwrap();
    let g: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
    g(p)
}

/// f(p): S = { p[0..8], p[8..12] as f32, p[12..16] as i32, p[16..24] } field by field (a derived
/// Clone of Copy fields), `between`: an instruction after the first field's store; returns a
/// checksum of S's four words read back.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Between { Nothing, StoreSrc, LoadDst, Call }
fn build(between: Between) -> Func {
    let mut f = Func::new("copy_runs".into(), 0);
    let p = f.vreg(Cls::I);
    f.params.push(p);
    f.rets.push(Cls::I);
    let s = f.slot(24, 8);
    f.block();
    let v = |f: &mut Func, c: Cls| f.vreg(c);
    let ps = v(&mut f, Cls::I);
    let (a, b, c, d) = (v(&mut f, Cls::I), v(&mut f, Cls::F32), v(&mut f, Cls::I), v(&mut f, Cls::I));
    push(&mut f, 0, Inst::SlotAddr(ps, s));
    push(&mut f, 0, Inst::Load(Mem::Int(8, false), a, p, 0));
    push(&mut f, 0, Inst::Load(Mem::F32, b, p, 8));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), ps, 0, a));
    match between {
        Between::Nothing => {}
        Between::StoreSrc => { let k = v(&mut f, Cls::I); push(&mut f, 0, Inst::Iconst(k, 5)); push(&mut f, 0, Inst::Store(Mem::Int(4, false), p, 12, k)); }
        Between::LoadDst => { let k = v(&mut f, Cls::I); push(&mut f, 0, Inst::Load(Mem::Int(8, false), k, ps, 0)); push(&mut f, 0, Inst::Store(Mem::Int(8, false), p, 32, k)); }
        Between::Call => push(&mut f, 0, Inst::Call(Callee::Fn(9), vec![], vec![])),
    }
    push(&mut f, 0, Inst::Store(Mem::F32, ps, 8, b));
    push(&mut f, 0, Inst::Load(Mem::Int(4, true), c, p, 12));
    push(&mut f, 0, Inst::Store(Mem::Int(4, true), ps, 12, c));
    push(&mut f, 0, Inst::Load(Mem::Int(8, false), d, p, 16));
    push(&mut f, 0, Inst::Store(Mem::Int(8, false), ps, 16, d));
    let (w0, w1, w2, x, y) = (v(&mut f, Cls::I), v(&mut f, Cls::I), v(&mut f, Cls::I), v(&mut f, Cls::I), v(&mut f, Cls::I));
    push(&mut f, 0, Inst::Load(Mem::Int(8, false), w0, ps, 0));
    push(&mut f, 0, Inst::Load(Mem::Int(8, false), w1, ps, 8));
    push(&mut f, 0, Inst::Load(Mem::Int(8, false), w2, ps, 16));
    push(&mut f, 0, Inst::IBinI(IOp::Mul, U64, x, w0, 3));
    push(&mut f, 0, Inst::IBin(IOp::Add, U64, x, x, w1));
    push(&mut f, 0, Inst::IBinI(IOp::Mul, U64, y, x, 5));
    push(&mut f, 0, Inst::IBin(IOp::Add, U64, y, y, w2));
    f.blocks[0].term = Term::Ret(vec![y]);
    f
}

fn copies(f: &Func) -> usize {
    f.blocks.iter().flat_map(|b| b.insts.iter()).filter(|i| matches!(i, Inst::Copy(..))).count()
}

#[test]
fn fields_copied_one_by_one_become_one_copy() {
    let f = build(Between::Nothing);
    let mut g = f.clone();
    assert!(opt::merge_copy_runs(&mut g));
    assert_eq!(copies(&g), 1, "{}", g.dump());
    g.verify().unwrap();
    let mut mem = [0x1122_3344_5566_7788u64, 0x4048_f5c3_0000_0007, 0x0102_0304_0506_0708, 0, 0];
    let p = mem.as_mut_ptr() as u64;
    assert_eq!(run(&f, p), run(&g, p));
}

#[test]
fn a_write_of_the_source_or_a_read_of_the_destination_in_between_keeps_the_fields() {
    for b in [Between::StoreSrc, Between::LoadDst, Between::Call] {
        let mut g = build(b);
        // the fields after the intervening instruction may still form a run of their own (< 16 bytes here)
        opt::merge_copy_runs(&mut g);
        assert_eq!(copies(&g), 0, "{:?}:\n{}", b, g.dump());
    }
}
