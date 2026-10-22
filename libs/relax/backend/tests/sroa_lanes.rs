//! SROA vector promotion: a 16-byte slot cell written as four f32 lanes (core::arch `_mm_setr_ps`
//! through memory) and read as one vector, or written as a vector and read lane by lane, becomes a
//! V128 register with VInsert / VExtract; on x64 the promoted code must compute the same bits as
//! the unpromoted one.
use relax_backend::rir::*;

const N: usize = 6;

/// fn(src: *const f32, dst: *mut f32): for each i < N the slot cell i gets lanes src[4i + j] (f32
/// stores), is read back as a vector v_i and as lane 2 (an f32 load) and lane 1 (a 4-byte integer
/// load); dst[4i..4i+4] = v_i + v_i, dst[4N + i] = lane 2, ((u32 *)dst)[5N + i] = lane 1's bits.
fn build() -> Func {
    let mut f = Func::new("sroa_lanes".into(), 0);
    let (ps, pd) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![ps, pd];
    f.block();
    let sl = f.slot(16 * N as u32, 16);
    let a = f.vreg(Cls::I);
    let mut insts = vec![Inst::SlotAddr(a, sl)];
    for i in 0..N {
        for j in 0..4 {
            let x = f.vreg(Cls::F32);
            insts.push(Inst::Load(Mem::F32, x, ps, (16 * i + 4 * j) as i32));
            insts.push(Inst::Store(Mem::F32, a, (16 * i + 4 * j) as i32, x));
        }
    }
    for i in 0..N {
        let (v, s, l2, l1) = (f.vreg(Cls::V128), f.vreg(Cls::V128), f.vreg(Cls::F32), f.vreg(Cls::I));
        insts.push(Inst::VLoad(v, a, 16 * i as i32));
        insts.push(Inst::VX(VX::FAdd(VW::S, NanBits::Host), s, v, v));
        insts.push(Inst::VStore(pd, 16 * i as i32, s));
        insts.push(Inst::Load(Mem::F32, l2, a, 16 * i as i32 + 8));
        insts.push(Inst::Store(Mem::F32, pd, (16 * N + 4 * i) as i32, l2));
        insts.push(Inst::Load(Mem::Int(4, false), l1, a, 16 * i as i32 + 4));
        insts.push(Inst::Store(Mem::Int(4, false), pd, (20 * N + 4 * i) as i32, l1));
    }
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    f
}

fn slot_accesses(f: &Func) -> usize {
    f.blocks.iter().flat_map(|b| b.insts.iter()).filter(|i| matches!(i, Inst::SlotAddr(..))).count()
}

#[test]
fn lane_stores_and_vector_loads_promote() {
    let mut f = build();
    assert!(relax_backend::passes::sroa(&mut f), "sroa promoted nothing");
    relax_backend::passes::dce(&mut f);
    assert_eq!(slot_accesses(&f), 0, "the slot is still addressed:\n{}", f.dump());
    let ins = f.blocks.iter().flat_map(|b| b.insts.iter());
    assert_eq!(ins.clone().filter(|i| matches!(i, Inst::VInsert(..))).count(), 4 * N);
    assert_eq!(ins.filter(|i| matches!(i, Inst::VExtract(..))).count(), 2 * N);
    f.verify().unwrap();
}

#[cfg(all(target_arch = "x86_64", unix))]
#[test]
fn promoted_lanes_compute_the_same_bits() {
    use relax_backend::exec::Code;
    type Entry = extern "sysv64" fn(*const u8, *mut u8);
    let mut o = relax_session::Options::default();
    o.cpu = relax_backend::x64::detect_cpu(&o);
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let src: Vec<f32> = (0..4 * N).map(|k| (k as f32 - 7.5) * 1.25).collect();
    let mut outs = Vec::new();
    for promote in [false, true] {
        let mut f = build();
        if promote {
            assert!(relax_backend::passes::sroa(&mut f));
            relax_backend::passes::dce(&mut f);
        }
        let c = relax_backend::x64::compile(&o, &f, &env).unwrap();
        let code = Code::new(&c.code).unwrap();
        let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
        let mut dst = vec![0u32; 6 * N];
        run(src.as_ptr() as *const u8, dst.as_mut_ptr() as *mut u8);
        outs.push(dst);
    }
    assert_eq!(outs[0], outs[1]);
    for i in 0..N {
        assert_eq!(f32::from_bits(outs[1][4 * i + 3]), src[4 * i + 3] * 2.0);
        assert_eq!(f32::from_bits(outs[1][4 * N + i]), src[4 * i + 2]);
        assert_eq!(outs[1][5 * N + i], src[4 * i + 1].to_bits());
    }
}

/// x64: VSplat + VInsert chains whose inserted f32 comes from the Load right before it (the load is
/// folded into vinsertps' memory operand), many vectors live (xmm16..31 on AVX-512VL hosts).
#[cfg(all(target_arch = "x86_64", unix))]
#[test]
fn vinsert_from_memory_builds_the_lanes() {
    use relax_backend::exec::Code;
    type Entry = extern "sysv64" fn(*const u8, *mut u8);
    const M: usize = 40;
    let mut o = relax_session::Options::default();
    o.cpu = relax_backend::x64::detect_cpu(&o);
    let mut f = Func::new("vinsert_mem".into(), 0);
    let (ps, pd) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![ps, pd];
    f.block();
    let mut insts = Vec::new();
    let mut vs = Vec::new();
    for i in 0..M {
        let x0 = f.vreg(Cls::F32);
        insts.push(Inst::Load(Mem::F32, x0, ps, (4 * ((i * 3) % 64)) as i32));
        let mut v = f.vreg(Cls::V128);
        insts.push(Inst::VSplat(v, x0));
        for j in 1..4u8 {
            let x = f.vreg(Cls::F32);
            insts.push(Inst::Load(Mem::F32, x, ps, (4 * ((i * 3 + 5 * j as usize) % 64)) as i32));
            let v2 = f.vreg(Cls::V128);
            insts.push(Inst::VInsert(v2, v, j, x));
            v = v2;
        }
        vs.push(v);
    }
    for (i, v) in vs.iter().enumerate() {
        insts.push(Inst::VStore(pd, 16 * i as i32, *v));
    }
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    let env = relax_backend::x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: true, syms: 0, sym_class: 0, near: false };
    let c = relax_backend::x64::compile(&o, &f, &env).unwrap();
    let code = Code::new(&c.code).unwrap();
    let run: Entry = unsafe { std::mem::transmute(code.ptr()) };
    let src: Vec<f32> = (0..64).map(|k| k as f32 * 0.5 - 3.0).collect();
    let mut dst = vec![0f32; 4 * M];
    run(src.as_ptr() as *const u8, dst.as_mut_ptr() as *mut u8);
    for i in 0..M {
        for j in 0..4 {
            assert_eq!(dst[4 * i + j].to_bits(), src[(i * 3 + 5 * j) % 64].to_bits(), "vector {} lane {}", i, j);
        }
    }
}
