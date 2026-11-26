//! Helpers lowered as functions (big, called from several places): the
//! element's own state (its index, the output record's address, its
//! locals) survives the calls on every backend.

use makepad_script_compute_sched::kernel::KernelExt;
use makepad_script_compute::boundary::{LayoutFieldView, LayoutView};
use makepad_script_compute::kernel::{compile_with, FieldTy};
use makepad_script_compute::Backend;

fn segment() -> LayoutView<'static> {
    const FIELDS: &[LayoutFieldView<'static>] = &[
        LayoutFieldView { name: "a", ty: FieldTy::Vec3, offset: 0 },
        LayoutFieldView { name: "b", ty: FieldTy::Vec3, offset: 3 },
        LayoutFieldView { name: "color", ty: FieldTy::Vec4, offset: 6 },
        LayoutFieldView { name: "width", ty: FieldTy::F32, offset: 10 },
    ];
    LayoutView { name: "Segment", stride: 11, fields: FIELDS }
}

const SRC: &str = r#"
let out = output(Segment)
fn step(h, x) {
    let k = (h ^ (int(x) * 1000003)) * 16777619
    let m = (k ^ (k >>> 13)) * 0x5bd1e995
    m ^ (m >>> 15)
}
fn big(a, b, c) { float(step(step(step(step(step(step(0x811C9DC5, a), b), c), a + b), b + c), a + c) >>> 8) * (1.0 / 16777216.0) }
fn element(i) {
    let x = float(i) * 0.37 - 9.1
    let y = float(i) * 0.53 + 2.2
    let w = big(floor(x), floor(y), 5.0) + big(floor(x) + 1.0, floor(y), 5.0) + big(floor(x), floor(y) + 1.0, 5.0)
    out[i].a = vec3(100.0, 100.0 + float(i), 0.0)
    out[i].b = vec3(101.0, 100.0, 0.0)
    out[i].color = vec4(1.0, 0.5, 0.25, 1.0)
    out[i].width = 5.0 + w
}
"#;

#[test]
fn records_survive_function_calls() {
    let n = 9;
    for backend in [Backend::Native, Backend::Interp] {
        let k = compile_with(SRC, &[segment()], backend).unwrap_or_else(|e| panic!("{:?}", e));
        assert!(!k.program().funcs.is_empty(), "the helper is a function");
        for mode in 0..3 {
            let mut out = vec![-1.0f32; 11 * n];
            let mut c = k.call();
            c.output("out", &mut out).unwrap();
            match mode {
                0 => c.run(n).unwrap(),
                1 => c.run_interp(n).unwrap(),
                _ => c.run_parallel(n, 2).unwrap(),
            };
            for (i, r) in out.chunks_exact(11).enumerate() {
                assert_eq!(&r[..10], &[100.0, 100.0 + i as f32, 0.0, 101.0, 100.0, 0.0, 1.0, 0.5, 0.25, 1.0], "record {i} mode {mode}: {r:?}");
                assert!(r[10] >= 5.0 && r[10] < 8.0, "record {i} mode {mode}: {r:?}");
            }
        }
    }
}

#[test]
fn compile_time_cancel_escapes_nested_continue() {
    use makepad_script_compute::{ir::{self, Block, Elem, Op, RawBuf, Region, Stmt, Ty, Val}, lower, parse};
    use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
    use std::time::{Duration, Instant};

    let src = "let table = [0.0; 2]
        fn init() {
            for i in 0..1000000000 {
                for j in 0..32768 {
                    if j == 0 { continue }
                    table[0] = 1.0
                }
            }
            table[1] = 2.0
        }
        fn voice() { table[0] }";
    let tokens = parse::lex(src).unwrap();
    let items = parse::Parser::new(&tokens).items().unwrap();
    let lowered = lower::lower_with_rate(&items, src.len() + 1, &|_| None, lower::RateOverride::default(), lower::CompileTime { word: Some(&AtomicU32::new(0)), ..Default::default() }).unwrap();
    let mut program = lowered.init.unwrap();
    assert!(program.total_cost() > lower::MAX_INIT_COST, "audio init runs past the kernel's admission cost");
    // Interlock immediately before the source continue. This test-owned buffer
    // is bound only here, after the compiler has finished injecting its polls.
    // Reading the same cancel word acknowledges the sender. The break below
    // releases only the interlock; the source then continues to its own loop head.
    fn interlock(block: &mut Block, one: Val, ack: Val) -> bool {
        if let Some(at) = block.iter().position(|s| matches!(s, Stmt::Continue(_))) {
            block.splice(at..at, [
                Stmt::Store { region: Region::Buf(1), base: 0, extent: 1, off: None, val: one },
                Stmt::Loop { cap: u32::MAX, body: vec![
                    Stmt::Def(ack, Op::Load { region: Region::Buf(0), base: 0, extent: 1, off: None }),
                    Stmt::If(ack, vec![Stmt::Break(0)], vec![]),
                ] },
            ]);
            return true;
        }
        block.iter_mut().any(|s| match s {
            Stmt::If(_, yes, no) => interlock(yes, one, ack) || interlock(no, one, ack),
            Stmt::Loop { body, .. } => interlock(body, one, ack),
            _ => false,
        })
    }
    let one = Val(program.vals.len() as u32);
    let ack = Val(one.0 + 1);
    program.vals.extend([Ty::I32, Ty::Bool]);
    program.body.insert(0, Stmt::Def(one, Op::ConstI(1)));
    assert!(interlock(&mut program.body, one, ack), "source continue lowered to AIR");

    ir::validate(&program, &ir::Regions { ctx: 0, state: 0, shared: 2, frame: program.frame_words, shared_writable: true, bufs: vec![false, true], io: false }).unwrap();
    for backend in [Backend::Interp, #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))] Backend::Native] {
        let control = Arc::new(AtomicU32::new(0));
        let signal = Arc::new(AtomicU32::new(0));
        let worker_signal = signal.clone();
        let worker_control = control.clone();
        let p = program.clone();
        let (done, completion) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut shared = [0; 2];
            let ptr = worker_control.as_ptr();
            let signal_ptr = worker_signal.as_ptr();
            match backend {
                Backend::Interp => {
                    let bufs = [
                        RawBuf { ptr, len: 1, writable: false, elem: Elem::Word },
                        RawBuf { ptr: signal_ptr, len: 1, writable: true, elem: Elem::Word },
                    ];
                    let mut mem = ir::Mem { ctx: &mut [], state: &mut [], shared: ir::Shared::Write(&mut shared), bufs: &bufs };
                    ir::run_compile_time(&p, &mut vec![0; p.scratch_words()], &mut mem, &mut ir::Io::default(), true, None).unwrap();
                }
                #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
                Backend::Native => {
                    let code = makepad_script_compute::native::compile_with(&p, 2).expect("emit native init");
                    let table = [ptr as u64, 1, signal_ptr as u64, 1];
                    // SAFETY: the validated program accesses these two live descriptors
                    // and the private two-word shared table only.
                    unsafe { code.run_kernel([].as_mut_ptr(), [].as_mut_ptr(), shared.as_mut_ptr(), table.as_ptr(), 1) };
                }
                #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
                Backend::Native => return,
            }
            done.send(shared).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while signal.load(Ordering::Relaxed) == 0 && Instant::now() < deadline && !worker.is_finished() {
            std::thread::yield_now();
        }
        control.store(1, Ordering::Relaxed);
        assert_eq!(signal.load(Ordering::Relaxed), 1, "{backend:?} never reached the inner continue");
        assert_eq!(completion.recv_timeout(Duration::from_secs(2)).unwrap(), [0, 0], "{backend:?} ran beyond the inner loop");
        worker.join().unwrap();
    }
}
