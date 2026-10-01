//! NEON ×4 differential fuzz: random kernel-shaped AIR programs (divergent
//! and uniform branches, loops with breaks and continues at every depth,
//! per-lane frames, hostile loads, integer division, shifts, selects,
//! variables written under masks) run on the interpreter, scalar native
//! code and NEON code, with guard words around every buffer: every word
//! must agree, and nothing outside a buffer may change.
#![cfg(target_arch = "aarch64")]

use makepad_script_compute::ir::{self, Block, Program, Region, Regions, Stmt};
use makepad_script_compute::kernel::{compile, compile_with, K_BASE};
use makepad_script_compute::{arm64, neon, Backend};

#[path = "support/kernel_gen.rs"]
mod kernel_gen;
use kernel_gen::*;

/// Runs p on the interpreter, scalar and NEON code: (interpreter,
/// scalar, NEON) buffers and ctx, and whether the guards held.
fn run3(p: &Program, n: usize, ctx0: &[u32], seed: u64) -> Option<[(Vec<Vec<u32>>, Vec<u32>); 3]> {
    let vcode = neon::compile(p)?;
    let scode = arm64::compile(p)?;
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
        let shared = [0u32; 1];
        let mut scratch = vec![0u32; p.scratch_words()];
        let zeros = [0f32; 1];
        let (mut o0, mut o1) = ([0f32; 1], [0f32; 1]);
        let mut mem = ir::Mem { ctx: &mut ictx, state: &mut state, shared: ir::Shared::Read(&shared), bufs: &bufs };
        let mut io = ir::Io { ins: [&zeros, &zeros], outs: [&mut o0, &mut o1] };
        ir::run(p, &mut scratch, &mut mem, &mut io, n as u32);
    }
    let mut state = [0u32; 1];
    let shared = [0u32; 1];
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

/// Deletes statements while NEON still disagrees with scalar code.
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
            if remove(&mut q.body, &mut kk, 0) && ir::validate(&q, regions).is_ok() && neon::compile(&q).is_some() && fails(&q) {
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
fn neon_equals_scalar_and_interpreter_on_random_kernels() {
    // NEON_FUZZ_SEED / NEON_FUZZ_ROUNDS for longer runs.
    let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let mut r = Rng(env("NEON_FUZZ_SEED", 0xC0FF_EE12_3456_789B));
    let regions = Regions { ctx: CTX as u32, state: 1, shared: 1, frame: FRAME, shared_writable: false, bufs: vec![false, false, true, true, true], io: false };
    let (mut tried, mut vectorized) = (0, 0);
    let rounds = env("NEON_FUZZ_ROUNDS", 30_000);
    for round in 0..rounds {
        let p = random_kernel(&mut r);
        if let Err(e) = ir::validate(&p, &regions) {
            panic!("round {}: the generator made an invalid program: {}", round, e);
        }
        tried += 1;
        let Some(vcode) = neon::compile(&p) else { continue };
        // NEON_FUZZ_TRACE: the program about to run (a crash leaves it).
        if let Ok(path) = std::env::var("NEON_FUZZ_TRACE") {
            std::fs::write(path, format!("round {}\n{:?}", round, p.body)).ok();
        }
        let scode = arm64::compile(&p).expect("scalar code");
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
            let shared = [0u32; 1];
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
        let shared = [0u32; 1];
        unsafe { scode.run_kernel(sctx.as_mut_ptr(), state.as_mut_ptr(), shared.as_ptr() as *mut u32, t.as_ptr(), n as u32) };
        // NEON for n & !3, the rest scalar (as the runtime does).
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
            let [_, s, v] = run3(&min, n, &ctx0, seed).unwrap();
            panic!("minimized: {:?}\nneon {:x?}\nscalar {:x?}\n", min.body, v, s);
        }
    }
    eprintln!("{} random kernels, {} vectorized, all bit-equal", tried, vectorized);
    assert!(vectorized * 10 > tried * 9, "only {} of {} vectorized", vectorized, tried);
}

// -- kernels from source -----------------------------------------------------------

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

/// Runs a one-output kernel natively with and without NEON and on the
/// interpreter, at sizes around multiples of 4, single and multi-thread.
fn same_everywhere(src: &str, out: &str, words: usize) {
    let k = compile(src).unwrap_or_else(|e| panic!("{:?}\n{}", e, src));
    assert!(k.simd(), "expected NEON code for:\n{}", src);
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
        assert_eq!(v, run(&k, false, 1), "NEON != scalar, n {}:\n{}", n, src);
        assert_eq!(v, run(&ki, false, 1), "NEON != interpreter, n {}:\n{}", n, src);
        assert_eq!(v, run(&k, true, 4), "NEON x4 threads != one thread, n {}:\n{}", n, src);
    }
}

#[test]
fn kernels_from_source_agree_with_neon() {
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
fn emit_kernels_agree_with_neon() {
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
fn what_neon_declines_still_runs_scalar() {
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
    let regions = Regions { ctx: CTX as u32, state: 1, shared: 1, frame: FRAME, shared_writable: false, bufs: vec![false, false, true, true, true], io: false };
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
                panic!("round {}: fused backends differ\n{:?}\ninterp {:x?}\nscalar {:x?}\nneon {:x?}", round, f.body, fi, fs, fv);
            }
        }
    }
    eprintln!("{} optimized random kernels bit-equal on every backend", ran);
}
