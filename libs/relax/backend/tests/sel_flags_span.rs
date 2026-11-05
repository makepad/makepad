//! A compare read by selects with flag-neutral instructions between them (the min and max of one
//! compare around a subtraction, a nearest-segment walk's bounding-box distance): arm64 reads the
//! compare's flags at each select -- no `cset` of the bool and no `cmp x, #0` to test it again --
//! and a select of a constant 0 reads xzr. Every input computes
//! max(max(min(a, b) - p, p - max(a, b)), 0) and the min / max.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, rir::*};

#[cfg(target_arch = "x86_64")]
type Entry = extern "sysv64" fn(i64, i64, i64, *mut i64);
#[cfg(target_arch = "aarch64")]
type Entry = extern "C" fn(i64, i64, i64, *mut i64);

const I64: IntTy = IntTy { bits: 64, signed: true };

fn code(f: &Func) -> Vec<u8> {
    f.verify().unwrap();
    #[allow(unused_mut)]
    let mut o = relax_session::Options::default();
    #[cfg(target_arch = "x86_64")]
    let c = {
        o.cpu = relax_backend::x64::detect_cpu(&o);
        let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
        relax_backend::x64::compile(&o, f, &env).unwrap()
    };
    #[cfg(target_arch = "aarch64")]
    let c = {
        let env = relax_backend::arm64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
        relax_backend::arm64::compile(&o, f, &env).unwrap()
    };
    assert!(c.relocs.is_empty());
    c.code[..c.text_len as usize].to_vec()
}

fn build() -> Func {
    let mut f = Func::new("bbox_dist".into(), 0);
    let ps: Vec<VReg> = (0..4).map(|_| f.vreg(Cls::I)).collect();
    f.params = ps.clone();
    let (a, b, p, out) = (ps[0], ps[1], ps[2], ps[3]);
    f.block();
    let [c, m, t, x, u, c2, w, c3, z, r] = [(); 10].map(|_| f.vreg(Cls::I));
    let insts = vec![
        Inst::ICmp(Cond::Lt, true, c, a, b),
        Inst::Sel(m, c, a, b),
        Inst::IBin(IOp::Sub, I64, t, m, p),
        Inst::Sel(x, c, b, a),
        Inst::IBin(IOp::Sub, I64, u, p, x),
        Inst::ICmp(Cond::Lt, true, c2, u, t),
        Inst::Sel(w, c2, t, u),
        Inst::ICmpI(Cond::Gt, true, c3, w, 0),
        Inst::Iconst(z, 0),
        Inst::Sel(r, c3, w, z),
        Inst::Store(Mem::Int(8, false), out, 0, r),
        Inst::Store(Mem::Int(8, false), out, 8, m),
        Inst::Store(Mem::Int(8, false), out, 16, x),
    ];
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    f
}

#[test]
fn selects_read_the_flags_across_arithmetic() {
    let bytes = code(&build());
    #[cfg(target_arch = "aarch64")]
    {
        let words: Vec<u32> = bytes.chunks(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let cset = words.iter().filter(|&&w| w & 0xFFFF_0FE0 == 0x9A9F_07E0).count();
        let cmp0 = words.iter().filter(|&&w| w & 0xFFFF_FC1F == 0xF100_001F).count();
        // csel x, x, xzr (the selected constant 0)
        let zr = words.iter().filter(|&&w| w & 0xFFE0_0C00 == 0x9A80_0000 && (w >> 16 & 31 == 31 || w >> 5 & 31 == 31)).count();
        // (the one `cmp x, #0` is w > 0's own compare)
        assert_eq!((cset, cmp0, zr), (0, 1, 1), "no materialised bool, no test of it, the 0 read from xzr: {words:08x?}");
    }
    let c = Code::new(&bytes).unwrap();
    let run: Entry = unsafe { std::mem::transmute(c.ptr()) };
    let vals = [i64::MIN / 4, -1000, -7, -1, 0, 1, 5, 6, 999, i64::MAX / 4];
    for &a in &vals {
        for &b in &vals {
            for &p in &[-1000i64, -3, 0, 4, 1000] {
                let mut o = [0i64; 3];
                run(a, b, p, o.as_mut_ptr());
                let (mn, mx) = (a.min(b), a.max(b));
                assert_eq!(o, [(mn - p).max(p - mx).max(0), mn, mx], "a {a} b {b} p {p}");
            }
        }
    }
}
