//! Check both explicit conventions against rustc-built callees and callers on every x86-64 OS.
#![cfg(target_arch = "x86_64")]
use relax_backend::{exec::Code, rir::*, x64};

#[repr(C)]
#[derive(Clone, Copy)]
struct Mixed { integer: i64, float: f64 }

fn mix(a: i64, b: f64, p: Mixed, c: i64, d: i64, e: f64, f: i64, g: i64, h: i64) -> Mixed {
    Mixed { integer: a + 3 * p.integer + 5 * c + 7 * d + 11 * f + 13 * g + 17 * h, float: b + 2.0 * p.float + 4.0 * e }
}

extern "sysv64" fn sysv(a: i64, b: f64, p: Mixed, c: i64, d: i64, e: f64, f: i64, g: i64, h: i64) -> Mixed {
    mix(a, b, p, c, d, e, f, g, h)
}

extern "win64" fn win(a: i64, b: f64, p: Mixed, c: i64, d: i64, e: f64, f: i64, g: i64, h: i64) -> Mixed {
    mix(a, b, p, c, d, e, f, g, h)
}

fn signature(conv: CConv) -> CSig {
    let i = CArg::Scalar(Mem::Int(8, true));
    let f = CArg::Scalar(Mem::F64);
    let pair = CAgg { size: 16, align: 8, fields: vec![(0, Mem::Int(8, true)), (8, Mem::F64)] };
    CSig { conv, args: vec![i.clone(), f.clone(), CArg::Agg(pair.clone()), i.clone(), i.clone(), f, i.clone(), i.clone(), i],
        ret: Some(pair), n_fixed: u32::MAX, ret_narrow: None, unwinds: false }
}

fn forwarding_body(body: CConv, callee: CConv, regs: bool) -> Code {
    let mut f = Func::new("explicit_x64_abi".into(), 0);
    f.cabi = Some(Box::new(signature(body)));
    // Aggregate result address, then scalar values or addresses of aggregate arguments.
    for cls in [Cls::I, Cls::I, Cls::F64, Cls::I, Cls::I, Cls::I, Cls::F64, Cls::I, Cls::I, Cls::I] {
        let v = f.vreg(cls);
        f.params.push(v);
    }
    let target = f.vreg(Cls::I);
    let addr = match callee { CConv::SysV64 => sysv as *const () as usize, CConv::Win64 => win as *const () as usize, _ => unreachable!() };
    let b = f.block();
    f.blocks[b as usize].insts = vec![
        Inst::Iconst(target, addr as i64),
        Inst::Call(Callee::CIndirect(target, Box::new(signature(callee))), f.params.clone(), Vec::new()),
    ];
    f.blocks[b as usize].pos = vec![0, 0];
    f.blocks[b as usize].term = Term::Ret(Vec::new());
    f.verify().unwrap();
    let env = x64::Env { slots: SlotsPtr::NONE, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: regs, syms: 0, sym_class: 0, near: false };
    let c = x64::compile(&relax_session::Options::default(), &f, &env).unwrap();
    assert!(c.relocs.is_empty());
    Code::new(&c.code).unwrap()
}

#[test]
fn explicit_conventions_cross_native_boundaries() {
    for body in [CConv::SysV64, CConv::Win64] {
        for callee in [CConv::SysV64, CConv::Win64] {
            for regs in [false, true] {
                let code = forwarding_body(body, callee, regs);
                let p = Mixed { integer: 3, float: 4.0 };
                let result = unsafe {
                    match body {
                        CConv::SysV64 => {
                            let run: extern "sysv64" fn(i64, f64, Mixed, i64, i64, f64, i64, i64, i64) -> Mixed = std::mem::transmute(code.ptr());
                            run(2, 1.5, p, 5, 7, 2.25, 11, 13, 17)
                        }
                        CConv::Win64 => {
                            let run: extern "win64" fn(i64, f64, Mixed, i64, i64, f64, i64, i64, i64) -> Mixed = std::mem::transmute(code.ptr());
                            run(2, 1.5, p, 5, 7, 2.25, 11, 13, 17)
                        }
                        _ => unreachable!(),
                    }
                };
                assert_eq!(result.integer, 664, "{body:?} -> {callee:?}, regs={regs}");
                assert_eq!(result.float, 18.5, "{body:?} -> {callee:?}, regs={regs}");
            }
        }
    }
}
