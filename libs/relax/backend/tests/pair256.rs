//! pair256: V128 halves fused into 256-bit AVX instructions compute bit-for-bit what the halves
//! compute. Each function runs with the pass and without it (`MAKEPAD_RUN_OPT=-pair256`) on the
//! host; the pass must fire (a V256 vreg appears) where the halves pair up.
#![cfg(all(target_arch = "x86_64", unix))]
use relax_backend::{exec::Code, rir::*};

type Entry3 = extern "sysv64" fn(*const u8, *const u8, *mut u8);
type Entry4 = extern "sysv64" fn(*mut u8, *const u8, *mut u8, u64);

fn options(pair: bool) -> relax_session::Options {
    let mut o = relax_session::Options::default();
    o.cpu = relax_backend::x64::detect_cpu(&o);
    if !pair {
        o.opt.push("-pair256".into());
    }
    o
}

fn compile(options: &relax_session::Options, f: &Func) -> Code {
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile(options, f, &env).unwrap();
    assert!(c.relocs.is_empty());
    Code::new(&c.code).unwrap()
}

fn caps() -> Option<relax_backend::pair256::Caps> {
    let o = options(true);
    o.cpu.avx.then_some(relax_backend::pair256::Caps { avx2: o.cpu.avx2, fma: o.cpu.fma })
}

/// the pass fuses `f` (on any host: a CPU with AVX2 and FMA assumed)
fn fires(f: &Func) -> bool {
    let mut g = f.clone();
    relax_backend::pair256::pair256(&mut g, relax_backend::pair256::Caps { avx2: true, fma: true }) && g.vregs.contains(&Cls::V256)
}

#[test]
fn shapes_fuse() {
    assert!(fires(&binop(VX::FAdd(VW::S, NanBits::Host))));
    assert!(fires(&indexed()));
    assert!(fires(&kernel(false)), "kernel without fma");
    assert!(fires(&kernel(true)), "kernel with fma");
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// an f32 or f64 lane value: specials, small numbers or random bits
    fn lane(&mut self, d: bool) -> u64 {
        let r = self.next();
        let sp32 = [0.0f32, -0.0, 1.0, -1.0, 0.5, f32::INFINITY, f32::NEG_INFINITY, f32::MAX, 1.0e-40];
        let sp64 = [0.0f64, -0.0, 1.0, -1.0, 0.5, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, 1.0e-310];
        match r % 5 {
            0 => self.next() & if d { u64::MAX } else { 0xffff_ffff },
            1 if d => [0x7ff8_0000_0000_0000u64, 0xfff4_0000_1234_5678][(r >> 8) as usize % 2],
            1 => [0x7fc0_0000u64, 0xff81_2345][(r >> 8) as usize % 2],
            2 if d => sp64[(r >> 8) as usize % sp64.len()].to_bits(),
            2 => sp32[(r >> 8) as usize % sp32.len()].to_bits() as u64,
            _ if d => (((r >> 11) % 2001) as f64 / 100.0 - 10.0).to_bits(),
            _ => ((((r >> 11) % 2001) as f32) / 100.0 - 10.0).to_bits() as u64,
        }
    }
    fn bytes(&mut self, d: bool, out: &mut [u8]) {
        let w = if d { 8 } else { 4 };
        for c in out.chunks_mut(w) {
            let x = self.lane(d);
            c.copy_from_slice(&x.to_le_bytes()[..w]);
        }
    }
}

fn push(f: &mut Func, b: usize, i: Inst) {
    f.blocks[b].insts.push(i);
    f.blocks[b].pos.push(0);
}

/// fn(a, b, d): d[0..32] = op(a[0..32], b[0..32]) as two 16-byte halves
fn binop(op: VX) -> Func {
    let mut f = Func::new("pair_op".into(), 0);
    let (pa, pb, pd) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![pa, pb, pd];
    f.block();
    let v: Vec<VReg> = (0..6).map(|_| f.vreg(Cls::V128)).collect();
    push(&mut f, 0, Inst::VLoad(v[0], pa, 0));
    push(&mut f, 0, Inst::VLoad(v[1], pa, 16));
    push(&mut f, 0, Inst::VLoad(v[2], pb, 0));
    push(&mut f, 0, Inst::VLoad(v[3], pb, 16));
    push(&mut f, 0, Inst::VX(op, v[4], v[0], v[2]));
    push(&mut f, 0, Inst::VX(op, v[5], v[1], v[3]));
    push(&mut f, 0, Inst::VStore(pd, 0, v[4]));
    push(&mut f, 0, Inst::VStore(pd, 16, v[5]));
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    f
}

#[test]
fn ops_match_halves() {
    let Some(_) = caps() else { return };
    let mut ops = vec![VX::And, VX::Or, VX::Xor, VX::AndNot];
    for w in [VW::S, VW::D] {
        for n in [NanBits::Host, NanBits::Intel] {
            ops.extend([VX::FAdd(w, n), VX::FSub(w, n), VX::FMul(w, n), VX::FDiv(w, n)]);
        }
        ops.extend([VX::FMinLt(w), VX::FMaxGt(w)]);
        for c in [Cond::Lt, Cond::Le, Cond::Gt, Cond::Ge, Cond::Eq, Cond::Ne] {
            ops.push(VX::FCmp(c, w));
        }
    }
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let (on, off) = (options(true), options(false));
    for op in ops {
        let f = binop(op);
        assert!(fires(&f), "{op:?}: not fused");
        let (c1, c2) = (compile(&on, &f), compile(&off, &f));
        let (e1, e2): (Entry3, Entry3) = unsafe { (std::mem::transmute(c1.ptr()), std::mem::transmute(c2.ptr())) };
        let d = matches!(op, VX::FAdd(VW::D, _) | VX::FSub(VW::D, _) | VX::FMul(VW::D, _) | VX::FDiv(VW::D, _) | VX::FMinLt(VW::D) | VX::FMaxGt(VW::D) | VX::FCmp(_, VW::D));
        for _ in 0..300 {
            let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
            rng.bytes(d, &mut a);
            rng.bytes(d, &mut b);
            let (mut r1, mut r2) = ([0u8; 32], [0u8; 32]);
            e1(a.as_ptr(), b.as_ptr(), r1.as_mut_ptr());
            e2(a.as_ptr(), b.as_ptr(), r2.as_mut_ptr());
            assert_eq!(r1, r2, "{op:?} a={a:?} b={b:?}");
        }
    }
}

/// fn(a, b, d, i): d[i*4..][0..32] = a[i*4..][0..32] * b[i*4..][0..32], indexed accesses
fn indexed() -> Func {
    let mut f = Func::new("pair_indexed".into(), 0);
    let (pa, pb, pd, i) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![pa, pb, pd, i];
    f.block();
    let v: Vec<VReg> = (0..6).map(|_| f.vreg(Cls::V128)).collect();
    let mul = VX::FMul(VW::S, NanBits::Host);
    push(&mut f, 0, Inst::VLoadX(v[0], pa, i, 2, 0));
    push(&mut f, 0, Inst::VLoadX(v[1], pa, i, 2, 16));
    push(&mut f, 0, Inst::VLoadX(v[2], pb, i, 2, 0));
    push(&mut f, 0, Inst::VLoadX(v[3], pb, i, 2, 16));
    push(&mut f, 0, Inst::VX(mul, v[4], v[0], v[2]));
    push(&mut f, 0, Inst::VX(mul, v[5], v[1], v[3]));
    push(&mut f, 0, Inst::VStoreX(pd, i, 2, 0, v[4]));
    push(&mut f, 0, Inst::VStoreX(pd, i, 2, 16, v[5]));
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    f
}

#[test]
fn indexed_matches_halves() {
    let Some(_) = caps() else { return };
    let f = indexed();
    assert!(fires(&f), "indexed: not fused");
    let (on, off) = (options(true), options(false));
    let (c1, c2) = (compile(&on, &f), compile(&off, &f));
    type E = extern "sysv64" fn(*const u8, *const u8, *mut u8, u64);
    let (e1, e2): (E, E) = unsafe { (std::mem::transmute(c1.ptr()), std::mem::transmute(c2.ptr())) };
    let mut rng = Rng(77);
    for i in 0..5u64 {
        let (mut a, mut b) = ([0u8; 64], [0u8; 64]);
        rng.bytes(false, &mut a);
        rng.bytes(false, &mut b);
        let (mut r1, mut r2) = ([0u8; 64], [0u8; 64]);
        e1(a.as_ptr(), b.as_ptr(), r1.as_mut_ptr(), i);
        e2(a.as_ptr(), b.as_ptr(), r2.as_mut_ptr(), i);
        assert_eq!(r1, r2, "i={i}");
    }
}

/// The piano modal kernel's shape (core::arch `__m256` code as halves): six state/coefficient
/// pairs live across a loop, a splat of a loaded scalar used for both halves, fused multiply-adds,
/// a loop-carried pair through a Mov, a per-iteration load/fma/store of an accumulator row, and an
/// unfused read of a half after the loop.
///
/// fn(st: *mut [f32; 48], inp: *const f32, acc: *mut [[f32; 8]; n], n)
fn kernel(fma: bool) -> Func {
    let mut f = Func::new("pair_kernel".into(), 0);
    let (st, inp, acc, n) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![st, inp, acc, n];
    for _ in 0..4 {
        f.block();
    }
    // zr, zi, cr, ci, gin, gout: two halves each
    let h: Vec<[VReg; 2]> = (0..6).map(|_| [f.vreg(Cls::V128), f.vreg(Cls::V128)]).collect();
    for (k, p) in h.iter().enumerate() {
        push(&mut f, 0, Inst::VLoad(p[0], st, 32 * k as i32));
        push(&mut f, 0, Inst::VLoad(p[1], st, 32 * k as i32 + 16));
    }
    let i = f.vreg(Cls::I);
    push(&mut f, 0, Inst::Iconst(i, 0));
    f.blocks[0].term = Term::Jump(1);
    let c = f.vreg(Cls::I);
    push(&mut f, 1, Inst::ICmp(Cond::Lt, false, c, i, n));
    f.blocks[1].term = Term::Branch(c, 2, 3);
    // body
    let (off, a, s, sp) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::F32), f.vreg(Cls::V128));
    push(&mut f, 2, Inst::IBinI(IOp::Shl, IntTy { bits: 64, signed: false }, off, i, 2));
    push(&mut f, 2, Inst::IBin(IOp::Add, IntTy { bits: 64, signed: false }, a, inp, off));
    push(&mut f, 2, Inst::Load(Mem::F32, s, a, 0));
    push(&mut f, 2, Inst::VSplat(sp, s));
    let (ao, aa) = (f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, 2, Inst::IBinI(IOp::Shl, IntTy { bits: 64, signed: false }, ao, i, 5));
    push(&mut f, 2, Inst::IBin(IOp::Add, IntTy { bits: 64, signed: false }, aa, acc, ao));
    let mul = VX::FMul(VW::S, NanBits::Host);
    let mut t = [VReg(0); 2];
    for (half, tt) in t.iter_mut().enumerate() {
        let [zr, zi, cr, ci, gin, _] = [h[0][half], h[1][half], h[2][half], h[3][half], h[4][half], h[5][half]];
        let (m1, m2, sb, t1, m3) = (f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::V128));
        push(&mut f, 2, Inst::VX(mul, m1, cr, zr));
        push(&mut f, 2, Inst::VX(mul, m2, ci, zi));
        push(&mut f, 2, Inst::VX(VX::FSub(VW::S, NanBits::Host), sb, m1, m2));
        if fma {
            push(&mut f, 2, Inst::VFma(false, false, t1, gin, sp, sb));
        } else {
            let p = f.vreg(Cls::V128);
            push(&mut f, 2, Inst::VX(mul, p, gin, sp));
            push(&mut f, 2, Inst::VX(VX::FAdd(VW::S, NanBits::Host), t1, p, sb));
        }
        push(&mut f, 2, Inst::VX(mul, m3, cr, zi));
        if fma {
            push(&mut f, 2, Inst::VFma(false, true, zi, ci, zr, m3));
        } else {
            let p = f.vreg(Cls::V128);
            push(&mut f, 2, Inst::VX(mul, p, ci, zr));
            push(&mut f, 2, Inst::VX(VX::FSub(VW::S, NanBits::Host), zi, p, m3));
        }
        *tt = t1;
    }
    for half in 0..2 {
        push(&mut f, 2, Inst::Mov(h[0][half], t[half]));
    }
    for half in 0..2 {
        let (av, r) = (f.vreg(Cls::V128), f.vreg(Cls::V128));
        push(&mut f, 2, Inst::VLoad(av, aa, 16 * half as i32));
        if fma {
            push(&mut f, 2, Inst::VFma(false, false, r, h[5][half], h[1][half], av));
        } else {
            let p = f.vreg(Cls::V128);
            push(&mut f, 2, Inst::VX(mul, p, h[5][half], h[1][half]));
            push(&mut f, 2, Inst::VX(VX::FAdd(VW::S, NanBits::Host), r, p, av));
        }
        push(&mut f, 2, Inst::VStore(aa, 16 * half as i32, r));
    }
    push(&mut f, 2, Inst::IBinI(IOp::Add, IntTy { bits: 64, signed: false }, i, i, 1));
    f.blocks[2].term = Term::Jump(1);
    // exit: state back, and lane 1 of zr's high half on its own
    for k in 0..2 {
        push(&mut f, 3, Inst::VStore(st, 32 * k as i32, h[k][0]));
        push(&mut f, 3, Inst::VStore(st, 32 * k as i32 + 16, h[k][1]));
    }
    let x = f.vreg(Cls::F32);
    push(&mut f, 3, Inst::VExtract(x, h[0][1], 1));
    push(&mut f, 3, Inst::Store(Mem::F32, st, 192, x));
    f.blocks[3].term = Term::Ret(vec![]);
    f.verify().unwrap();
    f
}

#[test]
fn kernel_matches_halves() {
    let Some(cp) = caps() else { return };
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let (on, off) = (options(true), options(false));
    for fma in [false, true] {
        if fma && !cp.fma {
            continue;
        }
        let f = kernel(fma);
        assert!(fires(&f), "kernel (fma {fma}): not fused");
        let (c1, c2) = (compile(&on, &f), compile(&off, &f));
        let (e1, e2): (Entry4, Entry4) = unsafe { (std::mem::transmute(c1.ptr()), std::mem::transmute(c2.ptr())) };
        for n in [0usize, 1, 2, 7, 37] {
            let mut st = vec![0u8; 196];
            let mut inp = vec![0u8; 4 * n.max(1)];
            let mut acc = vec![0u8; 32 * n.max(1)];
            for (k, v) in [&mut st[..192], &mut inp[..], &mut acc[..]].into_iter().enumerate() {
                for c in v.chunks_mut(4) {
                    let x = (((rng.next() >> 20) % 2001) as f32 / 1000.0 - 1.0) * if k == 2 { 3.0 } else { 0.99 };
                    c.copy_from_slice(&x.to_bits().to_le_bytes());
                }
            }
            let (mut st2, inp2, mut acc2) = (st.clone(), inp.clone(), acc.clone());
            e1(st.as_mut_ptr(), inp.as_ptr(), acc.as_mut_ptr(), n as u64);
            e2(st2.as_mut_ptr(), inp2.as_ptr(), acc2.as_mut_ptr(), n as u64);
            assert_eq!(st, st2, "state, n={n} fma={fma}");
            assert_eq!(acc, acc2, "acc, n={n} fma={fma}");
        }
    }
}
