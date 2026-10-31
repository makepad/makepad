//! Sibling calls (tail.rs): a call in tail position is a jump -- a recursion of ten million calls
//! runs in constant stack, a C body forwards to a C callee with its frame gone -- and stays a call
//! when a frame address escapes on a path to it.
#![cfg(target_arch = "x86_64")]
use relax_backend::{exec::Code, rir::*, x64};

fn env() -> x64::Env {
    x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false }
}

fn block(f: &mut Func, insts: Vec<Inst>, term: Term) -> u32 {
    let b = f.block();
    f.blocks[b as usize].pos = vec![0; insts.len()];
    f.blocks[b as usize].insts = insts;
    f.blocks[b as usize].term = term;
    b
}

/// `f(n, self) = if n == 0 { 0xabc + extra } else { self(n - 1, self) }` (Relax ABI, through the
/// fn pointer it gets), with `escape`: a slot's address stored to memory first.
fn countdown(escape: bool) -> Func {
    let mut f = Func::new("countdown".into(), 0);
    let (n, me) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![n, me];
    f.rets = vec![Cls::I];
    let (z, done, m, r, k) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    let mut entry = vec![Inst::ICmpI(Cond::Eq, false, z, n, 0)];
    if escape {
        let s = f.slot(8, 8);
        let (a, cell) = (f.vreg(Cls::I), f.vreg(Cls::I));
        entry.push(Inst::SlotAddr(a, s));
        entry.push(Inst::Iconst(cell, std::ptr::addr_of!(ESCAPED) as i64));
        entry.push(Inst::Store(Mem::Int(8, false), cell, 0, a));
    }
    block(&mut f, entry, Term::Branch(z, 1, 2));
    block(&mut f, vec![Inst::Iconst(done, 0xabc)], Term::Ret(vec![done]));
    block(&mut f, vec![Inst::IBinI(IOp::Sub, U64, m, n, 1), Inst::Call(Callee::Indirect(me), vec![m, me], vec![r]), Inst::Mov(k, r)], Term::Ret(vec![k]));
    f.verify().unwrap();
    f
}

static mut ESCAPED: u64 = 0;

fn has_call(code: &[u8]) -> bool {
    // call r11 (41 ff d3): the indirect call's encoding
    code.windows(3).any(|w| w == [0x41, 0xff, 0xd3])
}

#[test]
fn a_recursion_in_tail_position_runs_in_constant_stack() {
    let c = x64::compile(&relax_session::Options::default(), &countdown(false), &env()).unwrap();
    assert!(!has_call(&c.code), "the recursive call is a jump");
    let code = Code::new(&c.code).unwrap();
    let run: extern "sysv64" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
    // on a 256 KB thread: ten million frames of a call would not fit
    let p = code.ptr() as u64;
    let r = std::thread::Builder::new().stack_size(256 << 10).spawn(move || run(10_000_000, p)).unwrap().join().unwrap();
    assert_eq!(r, 0xabc);
}

#[test]
fn an_escaped_frame_address_keeps_the_call() {
    let c = x64::compile(&relax_session::Options::default(), &countdown(true), &env()).unwrap();
    assert!(has_call(&c.code), "a slot address stored before the call: the callee may read it");
    let code = Code::new(&c.code).unwrap();
    let run: extern "sysv64" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
    assert_eq!(run(1000, code.ptr() as u64), 0xabc);
}

#[test]
fn the_opt_switch_keeps_every_call() {
    let mut o = relax_session::Options::default();
    o.opt.push("-tailcall".to_string());
    let c = x64::compile(&o, &countdown(false), &env()).unwrap();
    assert!(has_call(&c.code));
}

extern "sysv64" fn mix(a: i64, b: i64, c: i64, d: u32, e: i64, x: f32, y: f64, g: i64) -> i64 {
    a + 2 * b + 3 * c + 5 * d as i64 + 7 * e + 11 * g + (x * 4.0) as i64 + (y * 8.0) as i64
}

#[test]
fn a_c_body_forwards_to_a_c_callee_with_its_frame_gone() {
    let i8s = CArg::Scalar(Mem::Int(8, true));
    let sig = CSig {
        conv: CConv::SysV64,
        args: vec![i8s.clone(), i8s.clone(), i8s.clone(), CArg::Scalar(Mem::Int(4, false)), i8s.clone(), CArg::Scalar(Mem::F32), CArg::Scalar(Mem::F64), i8s],
        ret: None,
        n_fixed: u32::MAX,
        ret_narrow: None,
        unwinds: false,
    };
    let mut f = Func::new("forward".into(), 0);
    f.cabi = Some(Box::new(sig.clone()));
    for cls in [Cls::I, Cls::I, Cls::I, Cls::I, Cls::I, Cls::F32, Cls::F64, Cls::I] {
        let v = f.vreg(cls);
        f.params.push(v);
    }
    f.rets = vec![Cls::I];
    let (t, a, r) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    let mut args = f.params.clone();
    let p0 = args[0];
    args[0] = a;
    block(&mut f, vec![
        Inst::Iconst(t, mix as *const () as i64),
        Inst::IBinI(IOp::Add, U64, a, p0, 100),
        Inst::Call(Callee::CIndirect(t, Box::new(sig)), args, vec![r]),
    ], Term::Ret(vec![r]));
    f.verify().unwrap();
    let c = x64::compile(&relax_session::Options::default(), &f, &env()).unwrap();
    // jmp r11 (41 ff e3), no call
    assert!(c.code.windows(3).any(|w| w == [0x41, 0xff, 0xe3]) && !has_call(&c.code));
    let code = Code::new(&c.code).unwrap();
    let run: extern "sysv64" fn(i64, i64, i64, u32, i64, f32, f64, i64) -> i64 = unsafe { std::mem::transmute(code.ptr()) };
    assert_eq!(run(1, 2, 3, 4, 5, 1.5, 2.25, 6), mix(101, 2, 3, 4, 5, 1.5, 2.25, 6));
}
