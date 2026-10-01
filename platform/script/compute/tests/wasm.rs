//! The wasm backend's scalar form against the interpreter, run natively on
//! stitch (a wasm interpreter): random kernel-shaped programs (the NEON
//! fuzz generator), their optimized and fused-multiply-add forms, kernels
//! from source (f64 portable math included) and fused multiply-adds at
//! double-rounding midpoints, every word of memory compared with guard
//! words around every buffer.
//!
//! The four-wide SIMD128 form needs an engine with SIMD (stitch implements
//! a subset); it is checked in V8 by `examples/wasm_diff.rs` (see there).
//! WASM_FUZZ_SEED / WASM_FUZZ_ROUNDS for longer runs.

#[path = "support/kernel_gen.rs"]
mod kernel_gen;
#[path = "support/wasm_case.rs"]
mod wasm_case;

use kernel_gen::*;
use makepad_script_compute::ir::{self, Regions};
use makepad_script_compute::wasm;
use makepad_stitch as stitch;
use wasm_case::*;

/// Runs export `run0` of `module` on a copy of the case's image; the
/// memory after it.
fn run_stitch(case: &Case, module: &[u8]) -> Result<Vec<u32>, String> {
    let engine = stitch::Engine::new();
    let m = stitch::Module::new(&engine, module).map_err(|e| format!("decode: {:?}", e))?;
    let mut store = stitch::Store::new(engine);
    let pages = (case.image.len() * 4).div_ceil(65536) as u32;
    let mem = stitch::Mem::new(&mut store, stitch::MemType { limits: stitch::Limits { min: pages, max: None } });
    let mut linker = stitch::Linker::new();
    linker.define("env", "memory", mem);
    let inst = linker.instantiate(&mut store, &m).map_err(|e| format!("instantiate: {:?}", e))?;
    let f = inst.exported_func("run0").ok_or("no run0")?;
    {
        let bytes = mem.bytes_mut(&mut store);
        for (k, w) in case.image.iter().enumerate() {
            bytes[4 * k..4 * k + 4].copy_from_slice(&w.to_le_bytes());
        }
    }
    let args: Vec<stitch::Val> = [case.ctx, case.state, case.shared, case.table, case.n, case.frame].iter().map(|x| stitch::Val::I32(*x as i32)).collect();
    f.call(&mut store, &args, &mut []).map_err(|e| format!("trap: {:?}", e))?;
    let bytes = mem.bytes(&store);
    Ok((0..case.image.len()).map(|k| u32::from_le_bytes(bytes[4 * k..4 * k + 4].try_into().unwrap())).collect())
}

/// The scalar module on stitch equals the interpreter (None: declined).
fn check(case: &Case) -> Option<Result<(), String>> {
    let module = wasm::module(&[wasm::Entry { program: &case.program, simd: false }], wasm::Target::default())?;
    let got = match run_stitch(case, &module) {
        Ok(g) => g,
        Err(e) => return Some(Err(e)),
    };
    match case.first_difference(&got) {
        None => Some(Ok(())),
        Some(w) => Some(Err(format!("word {} (byte {}): interpreter {:08x}, wasm {:08x}", w, 4 * w, case.expected[w], got[w]))),
    }
}

fn random_case(r: &mut Rng, p: makepad_script_compute::ir::Program, name: String) -> Case {
    let n = [1u32, 3, 4, 5, 8, 13, 16, 31][r.below(8) as usize];
    let ctx = params(r);
    let seed = r.next();
    let bufs = Bufs::new(&mut Rng(seed), n as usize);
    let bufs: Vec<(Vec<u32>, bool)> = bufs.words.iter().enumerate().map(|(k, w)| (w[G..w.len() - G].to_vec(), k >= 2)).collect();
    layout(name, p, true, &ctx, &SHARED, &bufs, n)
}

#[test]
fn scalar_wasm_equals_interpreter_on_random_kernels() {
    let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let mut r = Rng(env("WASM_FUZZ_SEED", 0x5CA1_AB1E_0DD5_EED5));
    let regions = Regions { ctx: CTX as u32, state: 1, shared: SHARED.len() as u32, frame: FRAME, shared_writable: false, bufs: vec![false, false, true, true, true], io: false };
    let rounds = env("WASM_FUZZ_ROUNDS", 10_000);
    let mut ran = 0;
    for round in 0..rounds {
        let p = random_kernel(&mut r);
        ir::validate(&p, &regions).unwrap_or_else(|e| panic!("round {}: invalid program: {}", round, e));
        let mut forms = vec![("plain", p.clone())];
        if round % 2 == 0 {
            // Optimized, then with fused multiply-adds (the exact f64 path).
            let mut q = p.clone();
            makepad_script_compute::opt::optimize(&mut q);
            makepad_script_compute::opt::fuse_fma(&mut q);
            if ir::validate(&q, &regions).is_ok() {
                forms.push(("fused", q));
            }
        }
        for (form, p) in forms {
            let case = random_case(&mut r, p, format!("round {} {}", round, form));
            match check(&case) {
                None => panic!("{}: the scalar backend declined", case.name),
                Some(Err(e)) => panic!("{}: {}\n{:?}", case.name, e, case.program.body),
                Some(Ok(())) => ran += 1,
            }
        }
    }
    eprintln!("{} random kernels: scalar wasm on stitch bit-equal to the interpreter", ran);
}

#[test]
fn scalar_wasm_equals_interpreter_on_kernels_from_source() {
    for (name, src) in CORPUS {
        for n in [1u32, 7, 1029] {
            let case = source_case(name, src, &[], &[("amp", 3.0)], 1.25, n);
            match check(&case) {
                None => panic!("{}: declined", case.name),
                Some(Err(e)) => panic!("{}: {}", case.name, e),
                Some(Ok(())) => {}
            }
        }
    }
}

#[test]
fn scalar_wasm_fused_multiply_add_is_exact_at_midpoints() {
    // The inputs reach the cases where rounding twice differs.
    let mut w = vec![0u32; 3 * 4096];
    fma_inputs(1, &mut w);
    let f = |x: u32| f32::from_bits(x);
    let twice = w.chunks(3).filter(|t| ((f(t[0]) as f64 * f(t[1]) as f64 + f(t[2]) as f64) as f32).to_bits() != f(t[0]).mul_add(f(t[1]), f(t[2])).to_bits()).count();
    assert!(twice > 200, "only {} double-rounding cases", twice);
    for (seed, src) in (1..10u64).zip(FMA_KERNELS.iter().cycle()) {
        let case = source_case_with("fma", src, &[], &[], 0.0, 4096, &|k, w| {
            if k == 1 {
                fma_inputs(seed, w)
            }
        });
        assert!(case.program.body.iter().any(|s| format!("{:?}", s).contains("Fma(")), "the kernel has no fused multiply-add");
        match check(&case) {
            None => panic!("declined"),
            Some(Err(e)) => panic!("seed {}: {}", seed, e),
            Some(Ok(())) => {}
        }
    }
}

/// The four-wide code's size against the scalar code's, per kernel (a
/// diagnostic: the target is 1-2x; a ratio past 3x is reported as a
/// warning to investigate; only a gross blowup fails).
#[test]
fn four_wide_code_stays_near_scalar_size() {
    let size = |p: &makepad_script_compute::ir::Program, simd: bool, relaxed: bool| {
        let t = wasm::Target { relaxed_fma: relaxed, ..Default::default() };
        wasm::module(&[wasm::Entry { program: p, simd }], t).map(|m| m.len())
    };
    let mut worst = 0.0f64;
    let kernels: Vec<(&str, &str)> = CORPUS.iter().copied().chain(FMA_KERNELS.iter().map(|s| ("fma", *s))).collect();
    for (name, src) in kernels {
        let case = source_case(name, src, &[], &[], 0.0, 4);
        if !wasm::simd_supported(&case.program) {
            continue;
        }
        for relaxed in [false, true] {
            let (s, v) = (size(&case.program, false, relaxed).unwrap(), size(&case.program, true, relaxed).unwrap());
            let ratio = v as f64 / s as f64;
            worst = worst.max(ratio);
            eprintln!("{:10} relaxed {:5}: scalar {:7} bytes, four wide {:7} bytes, {:.2}x{}", name, relaxed, s, v, ratio, if ratio > 3.0 { "  WARNING: past 3x" } else { "" });
        }
    }
    let mut r = Rng(0x51_2E5);
    for _ in 0..300 {
        let p = random_kernel(&mut r);
        let (Some(s), Some(v)) = (size(&p, false, false), size(&p, true, false)) else { continue };
        worst = worst.max(v as f64 / s as f64);
    }
    eprintln!("worst four wide / scalar size: {:.2}x", worst);
    assert!(worst < 6.0, "the four-wide code is {:.1}x the scalar code somewhere (a gross blowup)", worst);
}
