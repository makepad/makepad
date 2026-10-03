//! Host components: the registry, calls from native and interpreted
//! kernels, clamped slices, failures that never unwind into kernel code,
//! and the validator's checks.

use makepad_script_compute::host::{self, HostError, HostFn, HostSlice, SliceSig, Tier};
use makepad_script_compute::ir::{self, Op, Program, Region, Regions, SliceArg, Stmt, Ty, Val};
use makepad_script_compute::kernel::{compile, compile_with, Kernel};
use makepad_script_compute::sched::{InlineExecutor, Job};
use makepad_script_compute::Backend;
use std::sync::Arc;

fn sum3(args: &[u32], _s: &[HostSlice], rets: &mut [u32]) -> Result<(), HostError> {
    rets[0] = (f32::from_bits(args[0]) + f32::from_bits(args[1]) + f32::from_bits(args[2])).to_bits();
    Ok(())
}

fn boom(_args: &[u32], _s: &[HostSlice], _rets: &mut [u32]) -> Result<(), HostError> {
    panic!("a host component bug");
}

/// Writes `value` to every word of its slice; returns the words written.
fn fill(args: &[u32], s: &[HostSlice], rets: &mut [u32]) -> Result<(), HostError> {
    for i in 0..s[0].len() {
        s[0].set(i, args[0]);
    }
    rets[0] = s[0].len() as u32;
    Ok(())
}

fn register() {
    for f in [
        HostFn { name: "test.sum3", params: &[Ty::F32, Ty::F32, Ty::F32], slices: &[], rets: &[Ty::F32], cost: |_| 4, misses: 0, tier: Tier::X, call: sum3, doc: "" },
        HostFn { name: "test.boom", params: &[Ty::I32], slices: &[], rets: &[Ty::I32], cost: |_| 4, misses: 0, tier: Tier::X, call: boom, doc: "" },
        HostFn {
            name: "test.fill",
            params: &[Ty::I32],
            slices: &[SliceSig { max_words: 8, writable: true }],
            rets: &[Ty::I32],
            cost: |l| 16 + l[0] as u64,
            misses: 1,
            tier: Tier::D,
            call: fill,
            doc: "test.fill(buf, off, words, value)",
        },
    ] {
        host::register(f).unwrap();
    }
}

fn run(k: &Arc<Kernel>, n: usize, out: &str, words: usize, extra: &[(&str, usize)]) -> (Vec<u32>, Vec<Vec<u32>>, bool) {
    let mut j = Job::new(k.clone(), n);
    j.output_u32(out, vec![0; n * words]).unwrap();
    for (name, len) in extra {
        j.output_u32(name, vec![0; *len]).unwrap();
    }
    let st = j.run(&InlineExecutor, 1).unwrap().clone();
    (j.out_u32(out).unwrap().to_vec(), extra.iter().map(|(name, _)| j.out_u32(name).unwrap().to_vec()).collect(), st.host_error)
}

#[test]
fn registered_functions_run_natively_and_interpreted() {
    register();
    let src = "let o = output(f32)\nfn element(i) { o[i] = test.sum3(float(i), 0.5, 2.0) }";
    for backend in [Backend::Native, Backend::Interp] {
        let k = compile_with(src, &[], backend).unwrap_or_else(|e| panic!("{:?}", e));
        assert!(!k.simd(), "host calls run on the scalar code");
        let (o, _, err) = run(&k, 5, "o", 1, &[]);
        assert_eq!(o, (0..5).map(|i| (i as f32 + 2.5).to_bits()).collect::<Vec<_>>());
        assert!(!err);
    }
    // Values live across the call stay intact (caller-saved registers).
    let src = "let o = output(vec4)\nfn element(i) { let a = float(i) * 3.0\n let b = sin(a)\n let c = test.sum3(a, b, 1.0)\n o[i] = vec4(a, b, c, a + b + c) }";
    let kn = compile_with(src, &[], Backend::Native).unwrap();
    let ki = compile_with(src, &[], Backend::Interp).unwrap();
    assert_eq!(run(&kn, 33, "o", 4, &[]).0, run(&ki, 33, "o", 4, &[]).0);
}

#[test]
fn failures_are_reported_and_never_unwind_into_kernels() {
    register();
    let src = "let o = output(i32)\nfn element(i) { o[i] = test.boom(i) + 7 }";
    for backend in [Backend::Native, Backend::Interp] {
        let k = compile_with(src, &[], backend).unwrap();
        let (o, _, err) = run(&k, 3, "o", 1, &[]);
        assert_eq!(o, vec![7, 7, 7], "results are zero after a panic");
        assert!(err, "the error word is set");
    }
}

#[test]
fn slices_are_clamped_to_their_buffers_and_their_maximum() {
    register();
    // Offsets and lengths from the element: some past the buffer, some
    // longer than the function's maximum (8 words).
    let src = "let o = output(i32)\nlet dst = output(i32, 1, 0, dst)\nfn element(i) { o[i] = test.fill(dst, i * 5 - 3, i * 4, 9) }";
    for backend in [Backend::Native, Backend::Interp] {
        let k = compile_with(src, &[], backend).unwrap_or_else(|e| panic!("{:?}", e));
        assert!(!k.parallel_safe, "a written slice keeps the kernel on one thread");
        let (o, bufs, err) = run(&k, 6, "o", 1, &[("dst", 20)]);
        // i = 0: off -3 wraps to a huge u32 -> clamped to the end, 0 words.
        // i = 1: off 2, 4 words; i = 2: off 7, 8 words; i = 3: 12, 12 -> 8
        // (clamped: error); i = 4: 17, 16 -> 3 (buffer end: error); 5: 22 -> 0.
        assert_eq!(o, vec![0, 4, 8, 8, 3, 0]);
        let mut want = vec![9u32; 20];
        // Words 0, 1 and 6 are in no element's slice.
        for w in [0, 1, 6] {
            want[w] = 0;
        }
        assert_eq!(bufs[0], want);
        assert!(err);
    }
}

#[test]
fn compile_errors_name_the_contract() {
    register();
    let e = compile("let o = output(f32)\nfn element(i) { o[i] = test.sum3(1.0, 2.0) }").map(|_| ()).unwrap_err();
    assert!(e[0].message.contains("takes 3 arguments"), "{}", e[0].message);
    let e = compile("let src = input(i32)\nlet o = output(i32)\nfn element(i) { o[i] = test.fill(src, 0, 4, 1) }").map(|_| ()).unwrap_err();
    assert!(e[0].message.contains("writes this buffer"), "{}", e[0].message);
    let e = compile("let math = portable\nlet o = output(i32)\nlet d = output(i32)\nfn element(i) { o[i] = test.fill(d, 0, 1, 1) }").map(|_| ()).unwrap_err();
    assert!(e[0].message.contains("portable"), "{}", e[0].message);
}

#[test]
fn triangulate_a_concave_polygon() {
    // An L shape, clockwise, and a square counter-clockwise.
    let src = "let pts = input(f32)\nlet tris = output(i32, 1, 0, tris)\nlet o = output(i32)\nfn element(i) { o[i] = poly.triangulate(pts, i * 12, if i == 0 { 12 } else { 8 }, tris, i * 12, 12) }";
    let l = [0.0f32, 0.0, 0.0, 2.0, 1.0, 2.0, 1.0, 1.0, 2.0, 1.0, 2.0, 0.0];
    let sq = [0.0f32, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
    let mut pts = l.to_vec();
    pts.extend_from_slice(&sq);
    pts.resize(24, 0.0);
    let mut outs = Vec::new();
    for backend in [Backend::Native, Backend::Interp] {
        let k = compile_with(src, &[], backend).unwrap_or_else(|e| panic!("{:?}", e));
        let mut j = Job::new(k, 2);
        j.input("pts", pts.clone().into()).unwrap();
        j.output_u32("tris", vec![u32::MAX; 24]).unwrap();
        j.output_u32("o", vec![0; 2]).unwrap();
        let err = j.run(&InlineExecutor, 1).unwrap().host_error;
        assert!(!err);
        outs.push((j.out_u32("o").unwrap().to_vec(), j.out_u32("tris").unwrap().to_vec()));
    }
    assert_eq!(outs[0], outs[1], "native == interpreter");
    let (counts, tris) = &outs[0];
    assert_eq!(counts, &vec![4, 2]);
    // The triangles cover the polygons' areas exactly.
    let area = |p: &[f32], t: &[u32]| {
        t.chunks(3).map(|c| {
            let (a, b, cc) = (c[0] as usize, c[1] as usize, c[2] as usize);
            ((p[2 * b] - p[2 * a]) * (p[2 * cc + 1] - p[2 * a + 1]) - (p[2 * b + 1] - p[2 * a + 1]) * (p[2 * cc] - p[2 * a])).abs() * 0.5
        }).sum::<f32>()
    };
    assert_eq!(area(&l, &tris[..12]), 3.0);
    assert_eq!(area(&sq, &tris[12..18]), 1.0);
}

#[test]
fn the_validator_checks_host_calls() {
    register();
    let f = host::find("test.fill").unwrap();
    let r = Regions { ctx: 32, state: 1, shared: 1, frame: 1, shared_writable: false, bufs: vec![false, false, true], io: false };
    let prog = |slices: Vec<SliceArg>, args: Vec<Val>, rets: Vec<Val>, f: u16| Program {
        vals: vec![Ty::I32, Ty::I32, Ty::F32],
        vars: vec![],
        body: vec![Stmt::Def(Val(0), Op::ConstI(1)), Stmt::Def(Val(2), Op::ConstF(1.0)), Stmt::CallHost { f, args, slices, rets }],
        frame_words: 1,
        ..Default::default()
    };
    let s = |buf| vec![SliceArg { buf, off: Val(0), len: Val(0) }];
    assert!(ir::validate(&prog(s(2), vec![Val(0)], vec![Val(1)], f), &r).is_ok());
    assert!(ir::validate(&prog(s(1), vec![Val(0)], vec![Val(1)], f), &r).is_err(), "writes a read-only buffer");
    assert!(ir::validate(&prog(s(9), vec![Val(0)], vec![Val(1)], f), &r).is_err(), "no such buffer");
    assert!(ir::validate(&prog(s(2), vec![Val(2)], vec![Val(1)], f), &r).is_err(), "argument type");
    assert!(ir::validate(&prog(s(2), vec![Val(0)], vec![Val(0)], f), &r).is_err(), "result defined twice");
    assert!(ir::validate(&prog(s(2), vec![Val(0)], vec![], f), &r).is_err(), "arity");
    assert!(ir::validate(&prog(s(2), vec![Val(0)], vec![Val(1)], 60000), &r).is_err(), "no such function");
    let audio = Regions { io: true, ..r.clone() };
    assert!(ir::validate(&prog(s(2), vec![Val(0)], vec![Val(1)], f), &audio).is_err(), "audio programs never call out");
    // A call's result is visible after it, and its cost is the function's.
    let p = prog(s(2), vec![Val(0)], vec![Val(1)], f);
    assert!(p.total_cost() >= 16);
    let _ = Region::Frame;
}

/// A comb: n teeth whose reflex vertices sit near every ear candidate
/// (ear clipping's hard case), counter-clockwise.
fn comb(teeth: usize) -> Vec<f32> {
    let mut p = Vec::new();
    for k in 0..teeth {
        let x = k as f32;
        p.extend_from_slice(&[x, 0.0, x + 0.5, 10.0]);
    }
    p.extend_from_slice(&[teeth as f32, 0.0, teeth as f32, -1.0, 0.0, -1.0]);
    // As built the teeth run clockwise along the top: reverse for ccw.
    let n = p.len() / 2;
    let mut r = Vec::with_capacity(p.len());
    for i in (0..n).rev() {
        r.extend_from_slice(&p[2 * i..2 * i + 2]);
    }
    r
}

#[test]
fn triangulate_cost_is_honest_and_budgets_refuse_large_inputs() {
    let f = host::find("poly.triangulate").unwrap();
    let h = host::get(f).unwrap();
    for teeth in [64usize, 512, 2048] {
        for pts in [comb(teeth), { let mut c = comb(teeth); let n = c.len() / 2; let mut r = Vec::new(); for i in (0..n).rev() { r.extend_from_slice(&c[2 * i..2 * i + 2]); } c.clear(); r }] {
            let mut pts = pts;
            let n = pts.len() / 2;
            let mut tris = vec![0u32; 3 * n];
            let bufs = [
                ir::RawBuf { ptr: pts.as_mut_ptr() as *mut u32, len: pts.len(), writable: false },
                ir::RawBuf { ptr: tris.as_mut_ptr(), len: tris.len(), writable: true },
            ];
            let sl = [host::SliceRaw { buf: 0, off: 0, len: pts.len() as u32 }, host::SliceRaw { buf: 1, off: 0, len: tris.len() as u32 }];
            let mut rets = [0u32];
            assert!(host::invoke(f, &[], &sl, &mut rets, &bufs));
            assert_eq!(rets[0] as usize, n - 2, "{} points", n);
            // Honest: the ops it counts on this input stay within what it
            // declares (the same on every machine).
            let declared = h.cost_of(&[pts.len() as u32, tris.len() as u32]);
            let ran = host::triangulate_ops(&pts);
            assert!(ran <= declared, "{} points ran {} ops, declared {}", n, ran, declared);
            // A per-call limit below the input's cost refuses it before it runs.
            let limit = h.cost_of(&[pts.len() as u32, tris.len() as u32]) - 1;
            let mut rets = [7u32];
            assert!(!host::invoke_limited(f, &[], &sl, &mut rets, &bufs, limit));
            assert_eq!(rets, [0]);
        }
    }
    // The same from a kernel: the limit word refuses the call, and it is reported.
    let src = "let pts = input(f32)\nlet tris = output(i32, 1, 0, tris)\nlet o = output(i32)\nfn element(i) { o[i] = poly.triangulate(pts, 0, 1030, tris, 0, 1542) }";
    let k = compile(src).unwrap();
    let mut j = Job::new(k, 1);
    j.input("pts", comb(256).into()).unwrap();
    j.output_u32("tris", vec![0; 1542]).unwrap();
    j.output_u32("o", vec![0; 1]).unwrap();
    j.set_host_call_limit(1000);
    assert!(j.run(&InlineExecutor, 1).unwrap().host_error);
    assert_eq!(j.out_u32("o").unwrap(), &[0]);
    j.set_host_call_limit(0);
    assert!(!j.run(&InlineExecutor, 1).unwrap().host_error);
    assert_eq!(j.out_u32("o").unwrap(), &[513]);
}
