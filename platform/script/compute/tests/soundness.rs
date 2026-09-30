//! Regression tests for the kernel soundness review (findings 1–5, 19):
//! record overlap and capacity, memory access through raw words, compile-
//! and init-time budgets, the IR validator as a panic-free barrier, and
//! cancellation reaching running native code.

use makepad_script_compute::ir::{self, Bin, Block, Cmp, Op, Program, Region, Regions, Stmt, Ty, Un, Val, Var};
use makepad_script_compute::kernel::{compile, compile_with, KernelError};
use makepad_script_compute::Backend;
use std::time::{Duration, Instant};

fn err_of(src: &str) -> String {
    match compile(src) {
        Ok(_) => panic!("expected a compile error for:\n{}", src),
        Err(e) => e[0].message.clone(),
    }
}

// -- 1. records never overlap; capacity is checked --------------------------

#[test]
fn strides_smaller_than_a_record_are_rejected() {
    assert!(err_of("let o = output(vec3, 2)\nfn element(i) { o[i] = vec3(1.0) }").contains("smaller than"));
    assert!(err_of("let o = output(f32, 0)\nfn element(i) { o[i] = 1.0 }").contains("smaller than"));
    assert!(err_of("let o = output(vec3, 4, 2)\nfn element(i) { o[i] = vec3(1.0) }").contains("does not fit"));
}

#[test]
fn views_with_different_strides_are_not_split_across_threads() {
    let k = compile("let a = output(f32, 1, 0, buf)\nlet b = output(vec2, 2, 0, buf)\nfn element(i) { a[i] = 1.0\n b[i] = vec2(2.0) }").unwrap();
    assert!(!k.parallel_safe);
    let k = compile("let a = output(f32, 4, 0, buf)\nlet b = output(vec2, 4, 1, buf)\nfn element(i) { a[i] = 1.0\n b[i] = vec2(2.0) }").unwrap();
    assert!(k.parallel_safe, "same stride, disjoint fields: element-local");
}

#[test]
fn undersized_outputs_are_refused_before_a_split_run() {
    let k = compile("let o = output(vec3)\nfn element(i) { o[i] = vec3(float(i)) }").unwrap();
    assert!(k.parallel_safe);
    let n = 20000;
    let mut small = vec![0.0f32; n * 3 - 1];
    let mut c = k.call();
    c.output("o", &mut small).unwrap();
    match c.run_parallel(n, 4) {
        Err(KernelError::TooSmall { need, have, .. }) => assert_eq!((need, have), ((n * 3) as u64, n * 3 - 1)),
        other => panic!("expected TooSmall, got {:?}", other.map(|s| s.elements)),
    }
    // Emit buffers are always checked (their slots are per element).
    let e = compile("let s = emit_buffer(2, 4)\nfn primitive(i) { emit(s, 1.0, 2.0) }").unwrap();
    let mut data = vec![0.0f32; 10];
    let mut counts = vec![0u32; 5];
    let mut c = e.call();
    c.output("s", &mut data).unwrap();
    c.output_u32("s_count", &mut counts).unwrap();
    assert!(matches!(c.run(5), Err(KernelError::TooSmall { .. })));
}

#[test]
fn a_right_sized_split_run_equals_the_serial_run() {
    let k = compile("let o = output(vec4, 8, 2)\nfn element(i) { o[i] = vec4(float(i), hash01(i, 1), 0.5, -float(i)) }").unwrap();
    let n = 50_000;
    let mut a = vec![0.0f32; n * 8];
    let mut b = vec![0.0f32; n * 8];
    let mut c = k.call();
    c.output("o", &mut a).unwrap();
    c.run(n).unwrap();
    let mut c = k.call();
    c.output("o", &mut b).unwrap();
    c.run_parallel(n, 8).unwrap();
    assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()));
}

#[test]
fn addresses_do_not_wrap_before_the_clamp() {
    // base + off past 2^32 must clamp to the last word, not wrap to 0.
    let k = compile("let o = output(f32, 4, 3)\nfn element(i) { o[1073741823 + i] = 7.0 }").unwrap();
    for interp in [false, true] {
        let mut out = vec![0.0f32; 8];
        let mut c = k.call();
        c.output("o", &mut out).unwrap();
        if interp { c.run_interp(1).unwrap() } else { c.run(1).unwrap() };
        assert_eq!(out, vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 7.0], "interp {}", interp);
    }
}

// -- 2. read-only memory stays read-only ---------------------------------------

#[test]
fn one_slice_bound_to_two_inputs_is_fine_and_inputs_are_never_written() {
    let k = compile("let a = input(f32)\nlet b = input(f32)\nlet o = output(f32)\nfn element(i) { o[i] = a[i] + b[i] }").unwrap();
    let data = vec![1.5f32; 64];
    for interp in [false, true] {
        let mut out = vec![0.0f32; 64];
        let mut c = k.call();
        c.input("a", &data).unwrap();
        c.input("b", &data).unwrap();
        c.output("o", &mut out).unwrap();
        if interp { c.run_interp(64).unwrap() } else { c.run(64).unwrap() };
        assert!(out.iter().all(|x| *x == 3.0));
    }
    assert!(data.iter().all(|x| *x == 1.5));
    // Writing an input is a compile error, and binding a written buffer
    // read-only is refused.
    assert!(err_of("let a = input(f32)\nfn element(i) { a[i] = 1.0 }").contains("read-only"));
    let k = compile("let o = output(f32)\nfn element(i) { o[i] = 1.0 }").unwrap();
    let ro = vec![0.0f32; 4];
    let mut c = k.call();
    assert!(matches!(c.input("o", &ro), Err(KernelError::ReadOnly(_))));
}

// -- 3. budgets hold before work happens ----------------------------------------

#[test]
fn huge_initializers_fail_fast_without_allocating() {
    let t0 = Instant::now();
    let e = err_of("let T = [[[0.0; 4096]; 4096]; 4096]\nlet o = output(f32)\nfn element(i) { o[i] = T[0][0][0] }");
    assert!(e.contains("limit"), "{}", e);
    assert!(t0.elapsed() < Duration::from_secs(1), "rejected after {:?}", t0.elapsed());
}

#[test]
fn inlining_blowups_are_rejected() {
    // 16^5 unrolled calls.
    let src = "let o = output(f32)\nfn f(x) { x * 1.0001 + 0.5 }\nfn element(i) { let s = 0.0\n for a in 0..16 { for b in 0..16 { for c in 0..16 { for d in 0..16 { for e in 0..16 { s = f(s) } } } } }\n o[i] = s }";
    let t0 = Instant::now();
    let e = err_of(src);
    assert!(e.contains("too large") || e.contains("too much work"), "{}", e);
    assert!(t0.elapsed() < Duration::from_secs(20));
}

#[test]
fn init_is_budgeted_and_cannot_see_host_buffers() {
    let e = err_of("let src = input(f32)\nlet T = [0.0; 16]\nfn init() { T[0] = src[0] }\nlet o = output(f32)\nfn element(i) { o[i] = T[0] }");
    assert!(e.contains("unknown name `src`"), "{}", e);
    let e = err_of(
        "let T = [0.0; 16]\nfn init() { let s = 0.0\n for a in 0..100000 { for b in 0..100000 { s = s + 1.0 } }\n T[0] = s }\nlet o = output(f32)\nfn element(i) { o[i] = T[0] }",
    );
    assert!(e.contains("init()") && e.contains("limit"), "{}", e);
}

#[test]
fn work_admission_refuses_oversized_jobs() {
    let k = compile("let o = output(f32)\nfn element(i) { let s = 0.0\n for a in 0..1000 { s = s + sin(float(a)) }\n o[i] = s }").unwrap();
    let mut out = vec![0.0f32; 1000];
    let mut c = k.call();
    c.output("o", &mut out).unwrap();
    c.set_work_limit(k.cost * 10);
    assert!(matches!(c.run(1000), Err(KernelError::OverBudget { .. })));
    c.set_work_limit(k.cost * 1000);
    assert!(c.run(1000).is_ok());
}

// -- 4. the validator never panics and rejects malformed IR -----------------------

fn regions() -> Regions {
    Regions { ctx: 8, state: 4, shared: 4, frame: 4, shared_writable: false, bufs: vec![false, true], io: false }
}

fn prog(vals: Vec<Ty>, vars: Vec<Ty>, body: Block) -> Program {
    Program { vals, vars, body, frame_words: 4 }
}

#[test]
fn validator_rejects_the_classic_mistakes() {
    use Stmt::*;
    let r = regions();
    let c = |v: u32, x: i32| Def(Val(v), Op::ConstI(x));
    let b = |v: u32| Def(Val(v), Op::ConstB(true));
    // A value defined in a branch, used after it.
    let p = prog(vec![Ty::Bool, Ty::I32, Ty::I32], vec![], vec![b(0), If(Val(0), vec![c(1, 1)], vec![]), Def(Val(2), Op::Bin(Bin::AddI, Val(1), Val(1)))]);
    assert!(ir::validate(&p, &r).is_err());
    // Out-of-range identifiers.
    let p = prog(vec![Ty::I32], vec![], vec![Def(Val(5), Op::ConstI(1))]);
    assert!(ir::validate(&p, &r).is_err());
    let p = prog(vec![Ty::I32], vec![], vec![Def(Val(0), Op::Get(Var(3)))]);
    assert!(ir::validate(&p, &r).is_err());
    // Extent overflow and real frame size.
    let p = prog(vec![Ty::F32], vec![], vec![Def(Val(0), Op::Load { region: Region::Frame, base: u32::MAX, extent: 2, off: None })]);
    assert!(ir::validate(&p, &r).is_err());
    let p = prog(vec![Ty::F32], vec![], vec![Def(Val(0), Op::Load { region: Region::Frame, base: 3, extent: 2, off: None })]);
    assert!(ir::validate(&p, &r).is_err());
    // Buffers: missing, and read-only.
    let p = prog(vec![Ty::I32], vec![], vec![c(0, 1), Store { region: Region::Buf(7), base: 0, extent: 1, off: None, val: Val(0) }]);
    assert!(ir::validate(&p, &r).is_err());
    let p = prog(vec![Ty::I32], vec![], vec![c(0, 1), Store { region: Region::Buf(0), base: 0, extent: 1, off: None, val: Val(0) }]);
    assert!(ir::validate(&p, &r).is_err());
    let p = prog(vec![Ty::I32], vec![], vec![c(0, 1), Store { region: Region::Shared, base: 0, extent: 1, off: None, val: Val(0) }]);
    assert!(ir::validate(&p, &r).is_err());
    // Operand types.
    let p = prog(vec![Ty::I32, Ty::F32], vec![], vec![c(0, 1), Def(Val(1), Op::Un(Un::SqrtF, Val(0)))]);
    assert!(ir::validate(&p, &r).is_err());
    let p = prog(vec![Ty::F32, Ty::Bool], vec![], vec![Def(Val(0), Op::ConstF(1.0)), Def(Val(1), Op::CmpI(Cmp::Lt, Val(0), Val(0)))]);
    assert!(ir::validate(&p, &r).is_err());
    // Audio I/O where there is none.
    let p = prog(vec![Ty::I32, Ty::F32], vec![], vec![c(0, 0), Def(Val(1), Op::In { ch: 0, idx: Val(0) })]);
    assert!(ir::validate(&p, &r).is_err());
    // And a well-formed one passes.
    let p = prog(vec![Ty::I32, Ty::I32], vec![Ty::I32], vec![c(0, 1), Set(Var(0), Val(0)), Def(Val(1), Op::Get(Var(0))), Store { region: Region::Buf(1), base: 0, extent: 1, off: Some(Val(1)), val: Val(1) }]);
    assert!(ir::validate(&p, &r).is_ok(), "{:?}", ir::validate(&p, &r));
}

/// A tiny deterministic generator.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn random_program(r: &mut Rng) -> Program {
    let tys = [Ty::F32, Ty::I32, Ty::Bool];
    let nvals = 1 + r.below(24) as usize;
    let vals: Vec<Ty> = (0..nvals).map(|_| tys[r.below(3) as usize]).collect();
    let vars: Vec<Ty> = (0..r.below(4) as usize).map(|_| tys[r.below(3) as usize]).collect();
    fn block(r: &mut Rng, depth: u32, nvals: usize, nvars: usize) -> Block {
        let regions = [Region::Ctx, Region::State, Region::Shared, Region::Frame, Region::Buf(0), Region::Buf(1), Region::Buf(9)];
        let mut b = Vec::new();
        for _ in 0..r.below(8) {
            let v = |r: &mut Rng| Val(r.below(nvals as u64 + 2) as u32);
            let s = match r.below(9) {
                0 => Stmt::Def(v(r), Op::ConstF(r.next() as f32)),
                1 => Stmt::Def(v(r), Op::Bin(Bin::AddI, v(r), v(r))),
                2 => Stmt::Def(v(r), Op::Load { region: regions[r.below(7) as usize], base: r.below(12) as u32, extent: r.below(5) as u32, off: if r.below(2) == 0 { None } else { Some(v(r)) } }),
                3 => Stmt::Store { region: regions[r.below(7) as usize], base: r.next() as u32, extent: r.next() as u32, off: Some(v(r)), val: v(r) },
                4 => Stmt::Set(Var(r.below(nvars as u64 + 1) as u32), v(r)),
                5 => Stmt::If(v(r), block(r, depth, nvals, nvars), block(r, depth, nvals, nvars)),
                6 if depth < 3 => Stmt::Loop { cap: r.below(5) as u32, body: block(r, depth + 1, nvals, nvars) },
                7 => Stmt::Break(r.below(3) as u32),
                _ => Stmt::Def(v(r), Op::Sel(v(r), v(r), v(r))),
            };
            b.push(s);
        }
        b
    }
    let nvars = vars.len();
    Program { body: block(r, 0, nvals, nvars), vals, vars, frame_words: r.below(6) as u32 }
}

#[test]
fn validator_fuzz_never_panics_and_valid_programs_run() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    let reg = regions();
    let mut valid = 0;
    for _ in 0..50_000 {
        let p = random_program(&mut r);
        let res = std::panic::catch_unwind(|| ir::validate(&p, &reg));
        let ok = res.expect("validate panicked");
        if ok.is_ok() {
            valid += 1;
            // A valid program runs in the interpreter without panicking.
            let mut ctx = vec![0u32; 8];
            let mut state = vec![0u32; 4];
            let shared = vec![0u32; 4];
            let mut b0 = [0u32; 2];
            let mut b1 = [0u32; 3];
            let bufs = [
                ir::RawBuf { ptr: b0.as_mut_ptr(), len: 2, writable: false },
                ir::RawBuf { ptr: b1.as_mut_ptr(), len: 3, writable: true },
            ];
            let mut scratch = vec![0u32; p.scratch_words()];
            let zeros = [0f32; 1];
            let (mut o0, mut o1) = ([0f32; 1], [0f32; 1]);
            let mut mem = ir::Mem { ctx: &mut ctx, state: &mut state, shared: ir::Shared::Read(&shared), bufs: &bufs };
            let mut io = ir::Io { ins: [&zeros, &zeros], outs: [&mut o0, &mut o1] };
            ir::run(&p, &mut scratch, &mut mem, &mut io, 1);
            assert_eq!(b0, [0, 0], "a read-only buffer was written");
            // Native code on the same program: same memory, bit for bit, and
            // guard words around the buffers stay untouched.
            #[cfg(target_arch = "aarch64")]
            if let Some(code) = makepad_script_compute::arm64::compile(&p) {
                let mut nctx = vec![0u32; 8];
                let mut nstate = vec![0u32; 4];
                let nshared = vec![0u32; 4];
                let mut g0 = [0xDEADu32; 6];
                let mut g1 = [0xDEADu32; 7];
                g0[2..4].fill(0);
                g1[2..5].fill(0);
                let table = [g0[2..].as_mut_ptr() as u64, 2, g1[2..].as_mut_ptr() as u64, 3];
                unsafe { code.run_kernel(nctx.as_mut_ptr(), nstate.as_mut_ptr(), nshared.as_ptr() as *mut u32, table.as_ptr(), 1) };
                assert_eq!(&g0[2..4], &[0, 0], "native wrote a read-only buffer");
                assert_eq!(&g1[2..5], &b1, "native and interpreter disagree on the buffer");
                assert_eq!(nctx, ctx, "native and interpreter disagree on ctx");
                assert_eq!(nstate, state, "native and interpreter disagree on state");
                assert!(g0[..2].iter().chain(&g0[4..]).chain(&g1[..2]).chain(&g1[5..]).all(|w| *w == 0xDEAD), "native wrote outside a buffer");
                assert_eq!(nshared, shared, "native wrote shared tables");
            }
        }
    }
    assert!(valid > 100, "the fuzzer produced only {} valid programs", valid);
}

// -- 5. cancellation reaches running native code ------------------------------------

#[test]
fn cancel_stops_a_running_kernel_within_milliseconds() {
    // ~20 µs per element natively, 500k elements: ~10 s if not cancelled.
    let src = "let o = output(f32)\nfn element(i) { let s = float(i)\n for a in 0..1000 { s = s * 0.999 + sin(s) } \n o[i] = s }";
    for backend in [Backend::Native, Backend::Interp] {
        let k = compile_with(src, &[], backend).unwrap();
        let n = 500_000;
        let mut out = vec![0.0f32; n];
        let mut c = k.call();
        c.output("o", &mut out).unwrap();
        let token = c.cancel_token();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let at = Instant::now();
            token.cancel();
            tx.send(at).unwrap();
        });
        let r = c.run(n);
        let done = Instant::now();
        let cancelled_at = rx.recv().unwrap();
        let latency = done.duration_since(cancelled_at);
        eprintln!("{:?}: cancel -> return {:.2} ms", backend, latency.as_secs_f64() * 1e3);
        assert!(matches!(r, Err(KernelError::Cancelled)), "{:?}", r.map(|s| s.elements));
        assert!(latency < Duration::from_millis(25), "{:?} took {:?} to stop", backend, latency);
    }
}

// -- 19. emit_buffer(Layout, n) and whole records ------------------------------------

#[test]
fn emit_takes_layout_records() {
    use makepad_script_compute::kernel::{compact, FieldTy, Layout, LayoutField};
    let seg = Layout {
        name: "Seg".into(),
        stride: 6,
        fields: vec![LayoutField { name: "a".into(), ty: FieldTy::Vec3, offset: 0 }, LayoutField { name: "w".into(), ty: FieldTy::F32, offset: 4 }],
    };
    let src = "let s = emit_buffer(Seg, 2)\nfn primitive(i) { let r = Seg{}\n r.a = vec3(float(i), 1.0, 2.0)\n r.w = 0.5\n emit(s, r) }";
    let k = compile_with(src, &[seg], Backend::Native).unwrap_or_else(|e| panic!("{:?}", e));
    let mut data = vec![0.0f32; 3 * 2 * 6];
    let mut counts = vec![0u32; 3];
    let mut c = k.call();
    c.output("s", &mut data).unwrap();
    c.output_u32("s_count", &mut counts).unwrap();
    c.run(3).unwrap();
    let flat = compact(&data, &counts, 6, 2);
    assert_eq!(flat, vec![0.0, 1.0, 2.0, 0.0, 0.5, 0.0, 1.0, 1.0, 2.0, 0.0, 0.5, 0.0, 2.0, 1.0, 2.0, 0.0, 0.5, 0.0]);
}
