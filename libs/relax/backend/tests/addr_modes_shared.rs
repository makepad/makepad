//! `passes::addr_modes` with an element address read by both a load and a store (`a[i] += x`):
//! arm64 folds the shared add (and a shared shift of the access's own scale) into each access;
//! the result must still read and write exactly element i. Scalar f32 and q-register forms, run on
//! the host after the pass.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
type Entry = extern "sysv64" fn(*mut u8, u64);
#[cfg(target_arch = "aarch64")]
type Entry = extern "C" fn(*mut u8, u64);

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func) -> Code {
    let options = &relax_session::Options::default();
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
    Code::new(&relax_backend::x64::compile(options, f, &env).unwrap().code).unwrap()
}

#[cfg(target_arch = "aarch64")]
fn compile(f: &Func) -> Code {
    let options = &relax_session::Options::default();
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
    Code::new(&relax_backend::arm64::compile(options, f, &env).unwrap().code).unwrap()
}

/// fn(p, i) { let a = p + (i << sh); *a = *a + *a; (the second access through the same address) }
fn func(vector: bool, shared_shift: bool) -> Func {
    let sh = if vector { 4 } else { 2 };
    let mut f = Func::new("addr_shared".into(), 0);
    let (p, i) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![p, i];
    f.rets = vec![];
    f.block();
    let (s, a) = (f.vreg(Cls::I), f.vreg(Cls::I));
    let mut insts = vec![Inst::IBinI(IOp::Shl, IntTy { bits: 64, signed: false }, s, i, sh), Inst::IBin(IOp::Add, IntTy { bits: 64, signed: false }, a, p, s)];
    // a second address from the same shift (for the shared-shift form)
    let a2 = f.vreg(Cls::I);
    if shared_shift {
        insts.push(Inst::IBin(IOp::Add, IntTy { bits: 64, signed: false }, a2, p, s));
    }
    let st = if shared_shift { a2 } else { a };
    if vector {
        let (x, y) = (f.vreg(Cls::V128), f.vreg(Cls::V128));
        insts.extend([Inst::VLoad(x, a, 0), Inst::VX(VX::FAdd(VW::S, NanBits::Host), y, x, x), Inst::VStore(st, 0, y)]);
    } else {
        let (x, y) = (f.vreg(Cls::F32), f.vreg(Cls::F32));
        insts.extend([Inst::Load(Mem::F32, x, a, 0), Inst::FBin(FOp::Add, false, y, x, x), Inst::Store(Mem::F32, st, 0, y)]);
    }
    for k in insts {
        f.blocks[0].insts.push(k);
        f.blocks[0].pos.push(0);
    }
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    relax_backend::passes::addr_modes(&mut f);
    // arm64: both accesses take the register-offset form; the add and the shift go
    #[cfg(target_arch = "aarch64")]
    assert!(!f.blocks[0].insts.iter().any(|k| matches!(k, Inst::IBin(IOp::Add, ..) | Inst::IBinI(IOp::Shl, ..))), "{}", f.dump());
    f
}

#[test]
fn an_address_shared_by_a_load_and_a_store_reads_and_writes_its_element() {
    for vector in [false, true] {
        for shared_shift in [false, true] {
            let code = compile(&func(vector, shared_shift));
            let entry: Entry = unsafe { std::mem::transmute(code.ptr()) };
            let mut buf: Vec<f32> = (0..64).map(|x| x as f32).collect();
            let i = 3u64;
            entry(buf.as_mut_ptr() as *mut u8, i);
            let lanes = if vector { 4 } else { 1 };
            let at = i as usize * lanes;
            for (k, v) in buf.iter().enumerate() {
                let want = if (at..at + lanes).contains(&k) { 2.0 * k as f32 } else { k as f32 };
                assert_eq!(*v, want, "vector {vector} shared_shift {shared_shift} element {k}");
            }
        }
    }
}
