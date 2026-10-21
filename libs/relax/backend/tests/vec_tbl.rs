//! `VTbl`: d byte k = the byte at [base + bidx.byte(k)] of a small table (n words at base, byte
//! indices below 4n), on both back ends (NEON: ld1 + TBL; x86-64: pshufb per 16-byte quarter),
//! for every table size the kernels use and byte indices covering each quarter, word lookups
//! (bytes 4i .. 4i + 3) and arbitrary byte orders.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
type Entry = extern "sysv64" fn(*const u8, *const u8, *mut u8);
#[cfg(target_arch = "aarch64")]
type Entry = extern "C" fn(*const u8, *const u8, *mut u8);

fn options() -> relax_session::Options {
    #[allow(unused_mut)]
    let mut o = relax_session::Options::default();
    #[cfg(target_arch = "x86_64")]
    {
        o.cpu = relax_backend::x64::detect_cpu(&o);
    }
    o
}

fn compile(f: &Func, opt_regs: bool) -> Code {
    f.verify().unwrap();
    #[cfg(target_arch = "x86_64")]
    let c = {
        let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs, syms: 0, sym_class: 0 };
        relax_backend::x64::compile(&options(), f, &env).unwrap()
    };
    #[cfg(target_arch = "aarch64")]
    let c = {
        let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs, syms: 0, sym_class: 0 };
        relax_backend::arm64::compile(&options(), f, &env).unwrap()
    };
    assert!(c.relocs.is_empty());
    Code::new(&c.code).unwrap()
}

/// fn(table: *const u8, bidx: *const [u8; 16], dst: *mut [u8; 16]) with one VTbl of n words.
fn tbl(n: u8, opt_regs: bool) -> Code {
    let mut f = Func::new("vtbl".into(), 0);
    let (pt, pi, pd) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![pt, pi, pd];
    f.block();
    let (i, d) = (f.vreg(Cls::V128), f.vreg(Cls::V128));
    let insts = vec![Inst::VLoad(i, pi, 0), Inst::VTbl(d, pt, i, n), Inst::VStore(pd, 0, d)];
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    compile(&f, opt_regs)
}

#[test]
fn vtbl_reads_each_byte_of_the_table() {
    let table: Vec<u8> = (0..64u32).map(|k| (k * 37 + 11) as u8).collect();
    let mut seed = 0x1234_5678u32;
    for n in [4u8, 8, 12, 16] {
        for opt_regs in [false, true] {
            let code = tbl(n, opt_regs);
            let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
            for round in 0..200 {
                let mut bidx = [0u8; 16];
                if round % 2 == 0 {
                    // word lookups: lane l reads word w_l
                    for l in 0..4 {
                        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                        let w = (seed >> 24) as u8 % n;
                        for k in 0..4 {
                            bidx[4 * l + k] = 4 * w + k as u8;
                        }
                    }
                } else {
                    for b in bidx.iter_mut() {
                        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                        *b = (seed >> 24) as u8 % (4 * n);
                    }
                }
                let mut out = [0u8; 16];
                run(table.as_ptr(), bidx.as_ptr(), out.as_mut_ptr());
                let want: Vec<u8> = bidx.iter().map(|b| table[*b as usize]).collect();
                assert_eq!(&out[..], &want[..], "n {} opt_regs {} bidx {:?}", n, opt_regs, bidx);
            }
        }
    }
}
