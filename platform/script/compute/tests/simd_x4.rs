//! Four-wide (x4) differential fuzz: random kernel-shaped AIR programs (divergent
//! and uniform branches, loops with breaks and continues at every depth,
//! per-lane frames, hostile loads, integer division, shifts, selects,
//! variables written under masks) run on the interpreter, scalar native
//! code and x4 vector code (the shared back end: NEON on AArch64, SSE4.1/AVX2 on
//! x86-64), with guard words around every buffer: every word must agree, and
//! nothing outside a buffer may change.
#![cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]

use makepad_script_compute_sched::kernel::KernelExt;
use makepad_script_compute::ir::{self, Block, Program, Region, Regions, Stmt};
use makepad_script_compute::kernel::{compile, compile_with, K_BASE};
use makepad_script_compute::{native, native_vec, Backend};

#[path = "support/kernel_gen.rs"]
mod kernel_gen;
use kernel_gen::*;

/// Runs p on the interpreter, scalar and x4 code: (interpreter,
/// scalar, x4) buffers and ctx, and whether the guards held.
fn run3(p: &Program, n: usize, ctx0: &[u32], seed: u64) -> Option<[(Vec<Vec<u32>>, Vec<u32>); 3]> {
    let vcode = native_vec::compile(p)?;
    let scode = native::compile(p)?;
    let mut ib = Bufs::new(&mut Rng(seed), n);
    let mut ictx = ctx0.to_vec();
    {
        let bufs: Vec<ir::RawBuf> = ib
            .words
            .iter_mut()
            .enumerate()
            .map(|(k, w)| {
                let len = w.len() - 2 * G;
                ir::RawBuf { ptr: w[G..].as_mut_ptr(), len, writable: k >= 2 }
            })
            .collect();
        let mut state = [0u32; 1];
        let shared = SHARED;
        let mut scratch = vec![0u32; p.scratch_words()];
        let zeros = [0f32; 1];
        let (mut o0, mut o1) = ([0f32; 1], [0f32; 1]);
        let mut mem = ir::Mem { ctx: &mut ictx, state: &mut state, shared: ir::Shared::Read(&shared), bufs: &bufs };
        let mut io = ir::Io { ins: [&zeros, &zeros], outs: [&mut o0, &mut o1] };
        ir::run(p, &mut scratch, &mut mem, &mut io, n as u32);
    }
    let mut state = [0u32; 1];
    let shared = SHARED;
    let mut sb = Bufs::new(&mut Rng(seed), n);
    let mut sctx = ctx0.to_vec();
    let t = sb.table();
    unsafe { scode.run_kernel(sctx.as_mut_ptr(), state.as_mut_ptr(), shared.as_ptr() as *mut u32, t.as_ptr(), n as u32) };
    let mut vb = Bufs::new(&mut Rng(seed), n);
    let mut vctx = ctx0.to_vec();
    let t = vb.table();
    let n4 = n & !3;
    if n4 > 0 {
        unsafe { vcode.run_kernel(vctx.as_mut_ptr(), state.as_mut_ptr(), shared.as_ptr() as *mut u32, t.as_ptr(), n4 as u32) };
    }
    if n4 < n {
        vctx[K_BASE as usize] += n4 as u32;
        unsafe { scode.run_kernel(vctx.as_mut_ptr(), state.as_mut_ptr(), shared.as_ptr() as *mut u32, t.as_ptr(), (n - n4) as u32) };
        vctx[K_BASE as usize] -= n4 as u32;
    }
    assert!(sb.guards_intact() && vb.guards_intact(), "a guard word changed");
    Some([(ib.words, ictx), (sb.words, sctx), (vb.words, vctx)])
}

/// Deletes statements while x4 code still disagrees with scalar code.
fn minimize(p: Program, n: usize, ctx0: &[u32], seed: u64, regions: &Regions) -> Program {
    minimize_by(p, n, ctx0, seed, regions, |[_, s, v]| s != v)
}

fn minimize_by(mut p: Program, n: usize, ctx0: &[u32], seed: u64, regions: &Regions, bad: impl Fn(&[(Vec<Vec<u32>>, Vec<u32>); 3]) -> bool) -> Program {
    fn count(b: &Block) -> usize {
        b.iter().map(|s| 1 + match s {
            Stmt::If(_, t, e) => count(t) + count(e),
            Stmt::Loop { body, .. } => count(body),
            _ => 0,
        }).sum()
    }
    // The per-element initialization (frame zeroing, variable setup) at
    // the top of the element body is kept: without it a program reads
    // uninitialized memory, which no backend defines.
    fn keep(s: &Stmt, depth: u32) -> bool {
        depth == 1 && matches!(s, Stmt::Store { region: Region::Frame, off: None, .. } | Stmt::Set(..))
    }
    fn remove(b: &mut Block, k: &mut usize, depth: u32) -> bool {
        let mut i = 0;
        while i < b.len() {
            if *k == 0 && !keep(&b[i], depth) {
                b.remove(i);
                return true;
            }
            *k = k.saturating_sub(1);
            let done = match &mut b[i] {
                Stmt::If(_, t, e) => remove(t, k, depth) || remove(e, k, depth),
                Stmt::Loop { body, .. } => remove(body, k, depth + 1),
                _ => false,
            };
            if done {
                return true;
            }
            i += 1;
        }
        false
    }
    let fails = |p: &Program| match run3(p, n, ctx0, seed) {
        Some(r) => bad(&r),
        None => false,
    };
    loop {
        let mut shrunk = false;
        let total = count(&p.body);
        let mut k = 0;
        while k < total {
            let mut q = p.clone();
            let mut kk = k;
            if remove(&mut q.body, &mut kk, 0) && ir::validate(&q, regions).is_ok() && native_vec::compile(&q).is_some() && fails(&q) {
                p = q;
                shrunk = true;
            } else {
                k += 1;
            }
            if count(&p.body) != total {
                break;
            }
        }
        if !shrunk {
            return p;
        }
    }
}

#[test]
fn x4_equals_scalar_and_interpreter_on_random_kernels() {
    // X4_FUZZ_SEED / X4_FUZZ_ROUNDS for longer runs.
    let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let mut r = Rng(env("X4_FUZZ_SEED", 0xC0FF_EE12_3456_789B));
    let regions = Regions { ctx: CTX as u32, state: 1, shared: SHARED.len() as u32, frame: FRAME, shared_writable: false, bufs: vec![false, false, true, true, true], io: false };
    let (mut tried, mut vectorized) = (0, 0);
    let rounds = env("X4_FUZZ_ROUNDS", 30_000);
    for round in 0..rounds {
        let p = random_kernel(&mut r);
        if let Err(e) = ir::validate(&p, &regions) {
            panic!("round {}: the generator made an invalid program: {}", round, e);
        }
        // A program with functions compiles flattened and optimized: one
        // whose effects all prove dead loses its element loop (nothing to
        // vectorize).
        if !matches!(ir::flat(&p).body.last(), Some(Stmt::Loop { .. })) {
            continue;
        }
        tried += 1;
        let Some(vcode) = native_vec::compile(&p) else { continue };
        // X4_FUZZ_TRACE: the program about to run (a crash leaves it).
        if let Ok(path) = std::env::var("X4_FUZZ_TRACE") {
            std::fs::write(path, format!("round {}\n{:?}", round, p.body)).ok();
        }
        let scode = native::compile(&p).expect("scalar code");
        vectorized += 1;
        let n = [1usize, 3, 4, 5, 8, 13, 16, 31][r.below(8) as usize];
        let ctx0 = params(&mut r);
        let seed = r.next();
        // Interpreter.
        let mut ib = Bufs::new(&mut Rng(seed), n);
        let mut ictx = ctx0.clone();
        {
            let bufs: Vec<ir::RawBuf> = ib
                .words
                .iter_mut()
                .enumerate()
                .map(|(k, w)| {
                    let len = w.len() - 2 * G;
                    ir::RawBuf { ptr: w[G..].as_mut_ptr(), len, writable: k >= 2 }
                })
                .collect();
            let mut state = [0u32; 1];
            let shared = SHARED;
            let mut scratch = vec![0u32; p.scratch_words()];
            let zeros = [0f32; 1];
            let (mut o0, mut o1) = ([0f32; 1], [0f32; 1]);
            let mut mem = ir::Mem { ctx: &mut ictx, state: &mut state, shared: ir::Shared::Read(&shared), bufs: &bufs };
            let mut io = ir::Io { ins: [&zeros, &zeros], outs: [&mut o0, &mut o1] };
            ir::run(&p, &mut scratch, &mut mem, &mut io, n as u32);
        }
        // Scalar native.
        let mut sb = Bufs::new(&mut Rng(seed), n);
        let mut sctx = ctx0.clone();
        let t = sb.table();
        let mut state = [0u32; 1];
        let shared = SHARED;
        unsafe { scode.run_kernel(sctx.as_mut_ptr(), state.as_mut_ptr(), shared.as_ptr() as *mut u32, t.as_ptr(), n as u32) };
        // x4 for n & !3, the rest scalar (as the runtime does).
        let mut vb = Bufs::new(&mut Rng(seed), n);
        let mut vctx = ctx0.clone();
        let t = vb.table();
        let n4 = n & !3;
        if n4 > 0 {
            unsafe { vcode.run_kernel(vctx.as_mut_ptr(), state.as_mut_ptr(), shared.as_ptr() as *mut u32, t.as_ptr(), n4 as u32) };
        }
        if n4 < n {
            vctx[K_BASE as usize] += n4 as u32;
            unsafe { scode.run_kernel(vctx.as_mut_ptr(), state.as_mut_ptr(), shared.as_ptr() as *mut u32, t.as_ptr(), (n - n4) as u32) };
            vctx[K_BASE as usize] -= n4 as u32;
        }
        assert!(sb.guards_intact() && vb.guards_intact(), "round {}: a guard word changed", round);
        if ib.words != sb.words || ictx != sctx {
            let min = minimize_by(p.clone(), n, &ctx0, seed, &regions, |[i, s, _]| i != s);
            let [i, s, _] = run3(&min, n, &ctx0, seed).unwrap();
            panic!("round {}: scalar != interpreter, minimized: {:?}\ninterp {:x?}\nscalar {:x?}\n", round, min.body, i, s);
        }
        if vb.words != sb.words || vctx != sctx {
            let min = minimize(p.clone(), n, &ctx0, seed, &regions);
            // X4_FUZZ_DUMP=<dir>: the minimized program's RIR and code (kernel<k>.rir/.bin)
            if let Ok(dir) = std::env::var("X4_FUZZ_DUMP") {
                std::env::set_var("COMPUTE_NATIVE_DUMP", dir);
            }
            let [_, s, v] = run3(&min, n, &ctx0, seed).unwrap();
            panic!("minimized: {:?}\nx4 {:x?}\nscalar {:x?}\n", min.body, v, s);
        }
    }
    eprintln!("{} random kernels, {} vectorized, all bit-equal", tried, vectorized);
    assert!(vectorized * 10 > tried * 9, "only {} of {} vectorized", vectorized, tried);
}

// -- kernels from source -----------------------------------------------------------

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

/// Runs a one-output kernel natively with and without x4 code and on the
/// interpreter, at sizes around multiples of 4, single and multi-thread.
fn same_everywhere(src: &str, out: &str, words: usize) {
    let k = compile(src).unwrap_or_else(|e| panic!("{:?}\n{}", e, src));
    assert!(k.simd(), "expected x4 code for:\n{}", src);
    let ki = compile_with(src, &[], Backend::Interp).unwrap();
    for n in [1usize, 4, 7, 4096 + 5, 20_003] {
        let run = |k: &makepad_script_compute::kernel::Kernel, simd: bool, threads: usize| {
            let mut o = vec![0.0f32; n * words];
            let mut c = k.call();
            c.set_simd(simd);
            c.set_param("amp", 3.0);
            c.output(out, &mut o).unwrap();
            if threads > 1 { c.run_parallel(n, threads).unwrap() } else { c.run(n).unwrap() };
            bits(&o)
        };
        let v = run(&k, true, 1);
        assert_eq!(v, run(&k, false, 1), "x4 != scalar, n {}:\n{}", n, src);
        assert_eq!(v, run(&ki, false, 1), "x4 != interpreter, n {}:\n{}", n, src);
        assert_eq!(v, run(&k, true, 4), "x4 on 4 threads != one thread, n {}:\n{}", n, src);
    }
}

#[test]
fn kernels_from_source_agree_with_x4() {
    same_everywhere(
        "let W = 97\nlet pos = output(vec3)\nlet amp = param(8.0)\nfn vertex(i) { let x = float(i % W)\n let z = float(i / W)\n pos[i] = vec3(x, fbm2(vec2(x, z) * 0.02, 5, 2.0, 0.5) * amp + ridged2(vec2(z, x) * 0.01, 3), z) }",
        "pos",
        3,
    );
    // Divergent branches, a data-dependent loop with break, locals in the frame.
    same_everywhere(
        "let o = output(vec4)\nlet amp = param(1.0)\nfn element(i) {\n let a = [0.0; 5]\n let s = 0.0\n let k = 0\n while k < i % 13 { a[k % 5] = a[k % 5] + float(k) * amp\n if hash01(i, k) > 0.8 { break }\n k = k + 1 }\n if i % 3 == 0 { s = sin(float(i)) } elif i % 3 == 1 { s = sqrt(float(i)) } else { s = -1.0 }\n o[i] = vec4(a[0] + a[4], s, float(k), hash01(i, 7)) }",
        "o",
        4,
    );
    // Table rows (vec2, vec3, vec4) read at consecutive and scattered
    // rows, under full and partial masks, near the table's ends: the
    // de-interleaving row loads and their fallback.
    same_everywhere(
        "let T4 = [vec4(1.0, 2.0, 3.0, 4.0), vec4(5.0, 6.0, 7.0, 8.0), vec4(9.0, 10.0, 11.0, 12.0), vec4(13.0, 14.0, 15.0, 16.0), vec4(17.0, 18.0, 19.0, 20.0), vec4(-1.0, -2.0, -3.0, -4.0), vec4(0.5, 0.25, 0.125, 2.0)]\nlet T3 = [vec3(1.0, 2.0, 3.0), vec3(4.0, 5.0, 6.0), vec3(7.0, 8.0, 9.0), vec3(10.0, 11.0, 12.0), vec3(13.0, 14.0, 15.0)]\nlet T2 = [vec2(1.5, 2.5), vec2(3.5, 4.5), vec2(5.5, 6.5)]\nlet o = output(vec4)\nfn element(i) {\n let a = T4[i]\n let b = T3[i % 5]\n let c = T2[(i * 7) % 3]\n var d = vec4(0.0, 0.0, 0.0, 0.0)\n if i % 3 == 1 { d = T4[i / 2 + 3] }\n o[i] = vec4(a.x + b.y, a.w * c.x + d.y, b.z - c.y + d.x, a.y + a.z + b.x + d.w) }",
        "o",
        4,
    );
    // vec4 rows at hashed (scattered) indices: one q load per lane and a
    // transpose; and at consecutive indices near the end.
    same_everywhere(
        "let P = [vec4(1.0, 2.0, 3.0, 4.0), vec4(5.0, 6.0, 7.0, 8.0), vec4(-1.5, 2.5, -3.5, 4.5), vec4(0.25, 0.5, 0.75, 1.0), vec4(9.0, 8.0, 7.0, 6.0), vec4(-9.0, -8.0, -7.0, -6.0), vec4(0.0, 0.0, 1.0, 0.0), vec4(3.0, 1.0, 4.0, 1.5), vec4(2.0, 7.0, 1.0, 8.0)]\nlet o = output(vec4)\nfn element(i) {\n let a = P[hash(i) % 9]\n let b = P[(i + 5) % 9]\n o[i] = vec4(a.x * b.w, a.y + b.z, a.z - b.y, a.w * 2.0 + b.x) }",
        "o",
        4,
    );
    // Small tables read at hashed indices (8 and 16 words: held in vector
    // registers, read by TBL), next to a 12-word one (per-lane loads).
    same_everywhere(
        "let A8 = [0.5, -1.0, 2.0, 3.5, -4.25, 5.0, 6.125, -7.0]\nlet B16 = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, -16.0]\nlet C12 = [1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5, 8.5, 9.5, 10.5, 11.5, 12.5]\nlet o = output(vec4)\nfn element(i) {\n let h = hash(i)\n let a = A8[h & 7] + A8[(h >> 3) & 7]\n let b = B16[(h >> 6) & 15] * B16[i]\n var c = 0.0\n if (h & 1) == 1 { c = A8[i] - B16[h >> 28] }\n o[i] = vec4(a, b, c, C12[h % 12]) }",
        "o",
        4,
    );
    // Early return from a helper under divergence; integer division and shifts.
    same_everywhere(
        "let o = output(vec2)\nfn f(i) { if i % 5 == 2 { return float(i / 3) }\n let x = i * 7919\n return float((x >> 3) ^ (x << 5) % 11) }\nfn element(i) { o[i] = vec2(f(i), f(i + 1)) }",
        "o",
        2,
    );
}

#[test]
fn emit_kernels_agree_with_x4() {
    let src = "let seg = emit_buffer(3, 4)\nfn primitive(i) { for k in 0..(i % 5) { if hash01(i, k) > 0.3 { emit(seg, float(i), float(k), hash01(i, k)) } } }";
    let k = compile(src).unwrap();
    assert!(k.simd());
    for n in [6usize, 4099] {
        let mut outs = Vec::new();
        for simd in [true, false] {
            let mut data = vec![0.0f32; n * 4 * 3];
            let mut counts = vec![0u32; n];
            let mut c = k.call();
            c.set_simd(simd);
            c.output("seg", &mut data).unwrap();
            c.output_u32("seg_count", &mut counts).unwrap();
            let st = c.run(n).unwrap();
            outs.push((bits(&data), counts, st.overflowed));
        }
        assert_eq!(outs[0], outs[1]);
    }
}

#[test]
fn what_x4_declines_still_runs_scalar() {
    // A reduction reads and writes its accumulator per element.
    let k = compile("let v = input(f32)\nfn reduce_sum(i) { v[i] * 2.0 }").unwrap();
    assert!(!k.simd());
    // A non-local write.
    let k = compile("let o = output(f32)\nfn element(i) { o[i / 2] = 1.0 }").unwrap();
    assert!(!k.simd());
}

#[test]
fn optimized_random_kernels_give_the_same_bits() {
    // The AIR passes (forwarding, LICM, if-conversion, CSE, DCE) change
    // where values are computed, never a value: the optimized program on
    // every backend equals the original on the interpreter.
    let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let mut r = Rng(env("OPT_FUZZ_SEED", 0x0B7F_5EED_1234_5678));
    let regions = Regions { ctx: CTX as u32, state: 1, shared: SHARED.len() as u32, frame: FRAME, shared_writable: false, bufs: vec![false, false, true, true, true], io: false };
    let rounds = env("OPT_FUZZ_ROUNDS", 10_000);
    let mut ran = 0;
    for round in 0..rounds {
        let p = random_kernel(&mut r);
        let mut q = p.clone();
        makepad_script_compute::opt::optimize(&mut q);
        if let Err(e) = ir::validate(&q, &regions) {
            panic!("round {}: the optimized program is invalid: {}\n{:?}", round, e, q.body);
        }
        let n = [1usize, 3, 4, 5, 8, 13, 16, 31][r.below(8) as usize];
        let ctx0 = params(&mut r);
        let seed = r.next();
        let (Some(a), Some(b)) = (run3(&p, n, &ctx0, seed), run3(&q, n, &ctx0, seed)) else { continue };
        ran += 1;
        let [ia, _, _] = &a;
        let [ib, sb, vb] = &b;
        if ia != ib || ia != sb || ia != vb {
            panic!("round {}: optimized differs\noriginal {:?}\noptimized {:?}", round, p.body, q.body);
        }
        // Fused multiply-adds (math: fast): every backend gives the
        // interpreter's fused bits.
        let mut f = q.clone();
        makepad_script_compute::opt::fuse_fma(&mut f);
        if let Err(e) = ir::validate(&f, &regions) {
            panic!("round {}: the fused program is invalid: {}\n{:?}", round, e, f.body);
        }
        if let Some([fi, fs, fv]) = run3(&f, n, &ctx0, seed) {
            if fi != fs || fi != fv {
                panic!("round {}: fused backends differ\n{:?}\ninterp {:x?}\nscalar {:x?}\nx4 {:x?}", round, f.body, fi, fs, fv);
            }
        }
    }
    eprintln!("{} optimized random kernels bit-equal on every backend", ran);
}

// -- the shared x4 path's features, shown present in the RIR it makes --------------

/// The x4 RIR of a kernel source (before the back end's passes).
fn x4_rir(src: &str) -> String {
    let k = compile(src).unwrap_or_else(|e| panic!("{:?}\n{}", e, src));
    native_vec::rir_of(k.program()).expect("x4 code").dump()
}

#[test]
fn x4_store_groups_check_once_and_store_lane_records() {
    // a record of consecutive words at one varying offset, under divergence and with every
    // lane running: one range check on the largest lane offset (MaxAcrossU), then each lane's
    // words without clamps; the results are the scalar code's
    let srcs = [
        "let o = output(vec4)\nfn element(i) { o[i] = vec4(float(i), 2.0, float(i * 3), -1.0) }",
        "let o = output(vec3)\nfn element(i) { if hash01(i, 3) > 0.4 { o[i] = vec3(float(i), sqrt(float(i)), 0.5) } }",
        "let seg = emit_buffer(3, 4)\nfn primitive(i) { for k in 0..(i % 5) { if hash01(i, k) > 0.3 { emit(seg, float(i), float(k), hash01(i, k)) } } }",
    ];
    for src in srcs {
        assert!(x4_rir(src).contains("MaxAcrossU"), "no store group in the x4 code of:\n{}", src);
    }
    same_everywhere(srcs[0], "o", 4);
    same_everywhere(srcs[1], "o", 3);
}

/// Runs a kernel with input `inp` (`inp_words` words per element, run sizes around multiples
/// of 4, inputs as long as the run and shorter: a short input clamps) and one output natively
/// with and without x4 code and on the interpreter: the output words must agree.
fn same_with_input(src: &str, inp: &str, inp_words: usize, out: &str, words: usize) {
    let k = compile(src).unwrap_or_else(|e| panic!("{:?}\n{}", e, src));
    assert!(k.simd(), "expected x4 code for:\n{}", src);
    let ki = compile_with(src, &[], Backend::Interp).unwrap();
    for n in [1usize, 4, 7, 64, 4096 + 5] {
        for short in [0usize, 1, 3, 5, 13] {
            let len = (n * inp_words).saturating_sub(short).max(1);
            let data: Vec<f32> = (0..len).map(|j| ((j * 7919) % 1000) as f32 * 0.01 - 3.0).collect();
            let run = |k: &makepad_script_compute::kernel::Kernel, simd: bool, threads: usize| {
                let mut o = vec![0.0f32; n * words];
                let mut c = k.call();
                c.set_simd(simd);
                c.set_param("t", 0.75);
                c.input(inp, &data).unwrap();
                c.output(out, &mut o).unwrap();
                if threads > 1 { c.run_parallel(n, threads).unwrap() } else { c.run(n).unwrap() };
                bits(&o)
            };
            let v = run(&k, true, 1);
            assert_eq!(v, run(&k, false, 1), "x4 != scalar, n {} short {}:\n{}", n, short, src);
            assert_eq!(v, run(&ki, false, 1), "x4 != interpreter, n {} short {}:\n{}", n, short, src);
            assert_eq!(v, run(&k, true, 4), "x4 on 4 threads != one thread, n {} short {}:\n{}", n, short, src);
        }
    }
}

#[test]
fn x4_record_loads_are_lane_rows_and_a_transpose() {
    // a record's fields read per lane at root + c: four words a 16-byte load per lane and a
    // transpose (ZipLo of D lanes), checked once on the largest lane root; inputs shorter than
    // the run take the clamped gathers. Records 8 words apart (two chunks), 5 apart (overlapping
    // reads), fields read out of order, under divergence.
    let srcs = [
        "let src = input(f32)\nlet o = output(vec4)\nlet t = param(0.0)\nfn element(i) {\n let p = vec3(src[i * 8], src[i * 8 + 1], src[i * 8 + 2])\n let q = vec4(src[i * 8 + 3], src[i * 8 + 4], src[i * 8 + 5], src[i * 8 + 6])\n o[i] = vec4(p.x + q.w, p.y * q.z, p.z - q.y + src[i * 8 + 7], q.x + t) }",
        "let src = input(f32)\nlet o = output(vec4)\nlet t = param(0.0)\nfn element(i) {\n let b = i * 5\n o[i] = vec4(src[b + 3], src[b], src[b + 1] * t, src[b + 2] + src[b + 4]) }",
        "let src = input(f32)\nlet o = output(vec2)\nlet t = param(0.0)\nfn element(i) {\n var a = 0.0\n var b = 1.0\n if hash01(i, 2) > 0.5 { a = src[i * 6 + 1] + src[i * 6 + 2]\n b = src[i * 6] } \n o[i] = vec2(a, b * t) }",
    ];
    for (src, w) in srcs.iter().zip([8usize, 5, 6]) {
        let rir = x4_rir(src);
        assert!(rir.contains("ZipLo(D)"), "no load group in the x4 code of:\n{}", src);
        let words = if src.contains("output(vec2)") { 2 } else { 4 };
        same_with_input(src, "src", w, "o", words);
    }
}

#[test]
fn x4_store_groups_of_four_words_are_a_transpose_and_lane_stores() {
    // records of 4+ words at one varying offset: rows of four words (a transpose) stored per lane,
    // the rest word by word; disjoint records (24 words apart) four words of all lanes at a time,
    // overlapping ones (offsets 2 words apart) lane after lane, and under divergence
    let srcs = [
        ("let xf = output(mat4, 24, 0, inst)\nlet col = output(vec4, 24, 16, inst)\nlet t = param(0.0)\nfn instance(i) {\n let a = float(i) * 0.5\n xf[i] = mat4(a, 1.0, 2.0, a * a, -a, 3.0, t, 4.0, a + 1.0, 5.0, 6.0, 7.0, 8.0, a - t, 9.0, 10.0)\n col[i] = vec4(hash01(i, 1), hash01(i, 2), a, 1.0) }", "inst", 24),
        ("let o = output(vec4)\nfn element(i) { if hash01(i, 5) > 0.3 { o[i] = vec4(float(i), float(i * 2), sqrt(float(i)), -2.0) } }", "o", 4),
    ];
    for (src, out, words) in srcs {
        let rir = x4_rir(src);
        assert!(rir.contains("ZipLo(D)") && rir.contains("VStore"), "no transposed store group in the x4 code of:\n{}", src);
        same_everywhere(src, out, words);
    }
}

#[test]
#[cfg(target_arch = "aarch64")]
fn x4_small_tables_are_one_tbl() {
    // tables of up to 16 words (noise gradients, small lookup rows) read at varying indices: on
    // NEON one ld1 of the table and one TBL by byte indices (neon.rs small_table), not a gather
    let src = "let A8 = [0.5, -1.0, 2.0, 3.5, -4.25, 5.0, 6.125, -7.0]\nlet C12 = [1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5, 8.5, 9.5, 10.5, 11.5, 12.5]\nlet o = output(vec4)\nfn element(i) {\n let h = hash(i)\n o[i] = vec4(A8[h & 7], C12[h % 12], gradient_noise2(vec2(float(i) * 0.37, float(h & 255) * 0.11)), A8[i]) }";
    let rir = x4_rir(src);
    assert!(rir.contains("VTbl"), "no table lookup in the x4 code of:\n{}", src);
    same_everywhere(src, "o", 4);
}

#[test]
fn x4_table_rows_are_lane_rows_and_a_transpose() {
    // rows of a constant table read per lane at hashed (scattered) and consecutive rows: one
    // 16-byte load per lane and a transpose when no lane's row clamps and the loads stay inside the
    // table (the last vec3 row takes the gathers), the scalar code's words either way
    let srcs = [
        "let P = [vec4(1.0, 2.0, 3.0, 4.0), vec4(5.0, 6.0, 7.0, 8.0), vec4(-1.5, 2.5, -3.5, 4.5), vec4(0.25, 0.5, 0.75, 1.0), vec4(9.0, 8.0, 7.0, 6.0), vec4(-9.0, -8.0, -7.0, -6.0), vec4(0.0, 0.0, 1.0, 0.0), vec4(3.0, 1.0, 4.0, 1.5), vec4(2.0, 7.0, 1.0, 8.0)]\nlet o = output(f32)\nfn element(i) {\n let a = P[hash(i) % 9]\n o[i] = a.x * a.w + a.y - a.z }",
        "let T3 = [vec3(1.0, 2.0, 3.0), vec3(4.0, 5.0, 6.0), vec3(7.0, 8.0, 9.0), vec3(10.0, 11.0, 12.0), vec3(13.0, 14.0, 15.0), vec3(-1.0, -2.0, -3.0)]\nlet o = output(f32)\nfn element(i) {\n let b = T3[i % 6]\n let c = T3[(i * 5) % 6]\n o[i] = b.x - b.y * c.z + c.x * b.z }",
    ];
    for src in srcs {
        let rir = x4_rir(src);
        assert!(rir.contains("ZipLo(D)"), "no table row group in the x4 code of:\n{}", src);
        same_everywhere(src, "o", 1);
    }
}
