//! IOp::Crc32B/H/W/X (Arm `crc32b/h/w/x`, what std's aarch64 `__crc32b/d` lower to), IOp::Crc32CB/H/W/X
//! (Arm `crc32cb/h/w/x`, x64 SSE4.2 `crc32`: std's `__crc32c*` and `_mm_crc32_*`) and VX::Clmul (x64
//! `pclmulqdq`, arm64 `pmull .1q`, what std's x86_64 `_mm_clmulepi64_si128` lowers to): the host
//! backend's code equals the folder / a bitwise reference, every payload width and lane selector,
//! register and constant operands, a result sharing a register with an operand.
#![cfg(any(all(target_arch = "x86_64", unix), all(target_arch = "aarch64", unix)))]
use relax_backend::{exec::Code, passes, rir::*};

#[cfg(target_arch = "x86_64")]
fn compile(f: &Func) -> Vec<u8> {
    let mut o = relax_session::Options::default();
    o.cpu = relax_backend::x64::detect_cpu(&o);
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

fn push(f: &mut Func, i: Inst) {
    f.blocks[0].insts.push(i);
    f.blocks[0].pos.push(0);
}

const VALS: [u64; 8] = [0, 1, u64::MAX, 0x8000_0000_0000_0001, 0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210, 0xffff_ffff, 0xdead_beef_0000_00ff];

/// The folder (`passes::crc32`, every backend's meaning): CRC-32's and CRC-32C's check values over
/// "123456789" (pre/post inversion around the per-byte steps) and the wide steps equal to byte steps.
#[test]
fn crc32_folder_is_crc32() {
    for (c, check) in [(false, 0xcbf4_3926), (true, 0xe306_9283)] {
        let mut r = 0xffff_ffffu64;
        for &x in b"123456789" {
            r = passes::crc32(8, c, r, x as u64);
        }
        assert_eq!(r ^ 0xffff_ffff, check, "castagnoli {c}");
        for &a in &VALS {
            for &b in &VALS {
                let bytes = (0..8).fold(a & 0xffff_ffff, |r, i| passes::crc32(8, c, r, (b >> (8 * i)) & 0xff));
                assert_eq!(passes::crc32(64, c, a, b), bytes, "castagnoli {c} a {a:#x} b {b:#x}");
                assert_eq!(passes::crc32(16, c, a, b), passes::crc32(8, c, passes::crc32(8, c, a, b), b >> 8));
                assert_eq!(passes::crc32(32, c, a, b), passes::crc32(16, c, passes::crc32(16, c, a, b), b >> 16));
            }
        }
    }
}

/// f(a, b) = crc8(a, b) ^ crc16(b, a) << 1 ^ crc32(crc64(a, b), a) ^ crc64(a, 0x1234_5678_9abc_def0)
/// [constant data], each width once, the second with its operands swapped; `ops`: the 8/16/32/64-bit ops.
fn crc_matches_the_folder(ops: [IOp; 4]) {
    let u64t = IntTy { bits: 64, signed: false };
    let mut f = Func::new("crc32".into(), 0);
    let (a, b) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![a, b];
    f.rets = vec![Cls::I];
    f.block();
    let v: Vec<VReg> = (0..9).map(|_| f.vreg(Cls::I)).collect();
    push(&mut f, Inst::IBin(ops[0], u64t, v[0], a, b));
    push(&mut f, Inst::IBin(ops[1], u64t, v[1], b, a));
    push(&mut f, Inst::IBinI(IOp::Shl, u64t, v[2], v[1], 1));
    push(&mut f, Inst::IBin(ops[3], u64t, v[3], a, b));
    push(&mut f, Inst::IBin(ops[2], u64t, v[8], v[3], a));
    push(&mut f, Inst::IBinI(ops[3], u64t, v[4], a, 0x1234_5678_9abc_def0));
    push(&mut f, Inst::IBin(IOp::Xor, u64t, v[5], v[0], v[2]));
    push(&mut f, Inst::IBin(IOp::Xor, u64t, v[6], v[5], v[8]));
    push(&mut f, Inst::IBin(IOp::Xor, u64t, v[7], v[6], v[4]));
    f.blocks[0].term = Term::Ret(vec![v[7]]);
    f.verify().unwrap();
    let code = Code::new(&compile(&f)).unwrap();
    let run: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(code.ptr()) };
    let c = |op: IOp, x: u64, y: u64| passes::fold_ibin(op, u64t, x as i64, y as i64).unwrap() as u64;
    for &x in &VALS {
        for &y in &VALS {
            let want = c(ops[0], x, y) ^ c(ops[1], y, x) << 1 ^ c(ops[2], c(ops[3], x, y), x) ^ c(ops[3], x, 0x1234_5678_9abc_def0);
            assert_eq!(run(x, y), want, "{ops:?} a {x:#x} b {y:#x}");
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[test]
fn crc32_matches_the_folder() {
    crc_matches_the_folder([IOp::Crc32B, IOp::Crc32H, IOp::Crc32W, IOp::Crc32X]);
}

#[test]
fn crc32c_matches_the_folder() {
    crc_matches_the_folder([IOp::Crc32CB, IOp::Crc32CH, IOp::Crc32CW, IOp::Crc32CX]);
}

/// carry-less product of a's lane k & 1 and b's lane k >> 1
fn clmul(a: [u64; 2], b: [u64; 2], k: u8) -> [u64; 2] {
    let (x, y) = (a[(k & 1) as usize], b[(k >> 1 & 1) as usize]);
    let mut r = 0u128;
    for i in 0..64 {
        if y >> i & 1 != 0 {
            r ^= (x as u128) << i;
        }
    }
    [r as u64, (r >> 64) as u64]
}

#[cfg(target_arch = "x86_64")]
type Entry = extern "sysv64" fn(*const u64, *const u64, *mut u64);
#[cfg(target_arch = "aarch64")]
type Entry = extern "C" fn(*const u64, *const u64, *mut u64);

/// *pd = clmul_k(*pa, *pb), and with `alias` the result in a's register: a = clmul_k(a, b)
fn clmul_fn(k: u8, alias: bool) -> Func {
    let mut f = Func::new("clmul".into(), 0);
    let (pa, pb, pd) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![pa, pb, pd];
    f.block();
    let (va, vb) = (f.vreg(Cls::V128), f.vreg(Cls::V128));
    let d = if alias { va } else { f.vreg(Cls::V128) };
    push(&mut f, Inst::VLoad(va, pa, 0));
    push(&mut f, Inst::VLoad(vb, pb, 0));
    push(&mut f, Inst::VX(VX::Clmul(k), d, va, vb));
    push(&mut f, Inst::VStore(pd, 0, d));
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    f
}

#[test]
fn clmul_matches_the_reference() {
    for k in 0..4u8 {
        for alias in [false, true] {
            let code = Code::new(&compile(&clmul_fn(k, alias))).unwrap();
            let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
            for &x0 in &VALS {
                for &y1 in &VALS {
                    let (a, b) = ([x0, y1.rotate_left(17)], [x0.rotate_right(9) ^ 0x55, y1]);
                    let mut d = [0u64; 2];
                    run(a.as_ptr(), b.as_ptr(), d.as_mut_ptr());
                    assert_eq!(d, clmul(a, b, k), "k {k} alias {alias} a {a:x?} b {b:x?}");
                }
            }
        }
    }
}
