//! Indexed vector accesses (`Inst::VLoadX` / `VStoreX`, made by `passes::addr_modes`) at every
//! shift the encoders take and with and without a displacement: arm64's register-offset form
//! (`ldr q, [x, x, lsl #4]`), its shifted-add fallback, and x86-64's SIB forms, run on the host.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
type Entry = extern "sysv64" fn(*const u8, *mut u8, u64);
#[cfg(target_arch = "aarch64")]
type Entry = extern "C" fn(*const u8, *mut u8, u64);

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func) -> Code {
    let options = &relax_session::Options::default();
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
    let c = relax_backend::x64::compile(options, f, &env).unwrap();
    assert!(c.relocs.is_empty());
    Code::new(&c.code).unwrap()
}

#[cfg(target_arch = "aarch64")]
fn compile(f: &Func) -> Code {
    let options = &relax_session::Options::default();
    let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0 };
    let c = relax_backend::arm64::compile(options, f, &env).unwrap();
    assert!(c.relocs.is_empty());
    Code::new(&c.code).unwrap()
}

/// fn(src, dst, i) { dst[(i << sh) + off ..][..16] = src[(i << sh) + off ..][..16] } through the
/// indexed forms (VLoadX into VStoreX)
fn func(sh: u8, off: i32) -> Func {
    let mut f = Func::new("vec_index".into(), 0);
    let (ps, pd, pi) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![ps, pd, pi];
    f.rets = vec![];
    f.block();
    let v = f.vreg(Cls::V128);
    f.blocks[0].insts = vec![Inst::VLoadX(v, ps, pi, sh, off), Inst::VStoreX(pd, pi, sh, off, v)];
    f.blocks[0].pos = vec![Default::default(); 2];
    f.blocks[0].term = Term::Ret(vec![]);
    f
}

#[test]
fn indexed_vector_accesses_address_base_plus_scaled_index_plus_offset() {
    let src: Vec<u8> = (0..256u32).map(|x| (x * 7 + 3) as u8).collect();
    let max_sh = if cfg!(target_arch = "aarch64") { 4 } else { 3 };
    for sh in 0..=max_sh {
        for off in [0i32, 16, -16, 4] {
            let code = compile(&func(sh, off));
            let entry: Entry = unsafe { std::mem::transmute(code.ptr()) };
            for i in [1u64, 3, 5] {
                let at = ((i << sh) as i64 + off as i64 + 32) as usize;
                let mut dst = vec![0u8; 256];
                // base pointers 32 bytes in, so negative displacements stay inside
                entry(src[32..].as_ptr(), dst[32..].as_mut_ptr(), i);
                assert_eq!(&dst[at..at + 16], &src[at..at + 16], "sh {sh} off {off} i {i}");
                assert!(dst[..at].iter().chain(&dst[at + 16..]).all(|b| *b == 0), "sh {sh} off {off} i {i}: wrote outside");
            }
        }
    }
}
