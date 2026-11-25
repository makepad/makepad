//! Differential check of the wasm backend in CEF. The page runs the generated
//! scalar and four-wide modules against the same AIR interpreter image.
//! Set WASM_FUZZ_ROUNDS to exercise more random kernels in the same sequence.

#[path = "support/kernel_gen.rs"]
mod kernel_gen;
#[path = "support/wasm_case.rs"]
mod wasm_case;

use kernel_gen::{params, random_kernel, Bufs, Rng, CTX, FRAME, G, SHARED};
use makepad_script_compute::ir::{self, Block, Program, Regions, Stmt};
use makepad_script_compute::lower::kernel::K_WORK;
use makepad_script_compute::{opt, wasm};
use std::{fs, io, path::Path, process::ExitCode};
use wasm_case::{layout, source_case, source_case_spare, source_case_with, Case, CORPUS, FMA_KERNELS};

const DIRS: &[&str] = &["tests/web"];
const PAGE: &str = include_str!("web/wasm_diff.test.mjs");
// (Generator seed, plain length, optimized length). The plain AIR traces
// exercise each family; the optimized forms also cover empty and partial vectors.
const DEFAULT_ROUNDS: &[(u64, u32, u32)] = &[
    (2148, 4, 31),     // Divergent branches, masked writes, nested calls, continue(1).
    (535, 23, 0),      // Integer division, signed shift, break(1).
    (24431, 15, 8),    // Unsigned shift, float select, break(2).
    (20880, 16, 3),    // Per-lane frame indices, buffer lengths, continue(0/2).
    (129287, 7, 13),   // Clamped hostile load, shared table, continue(3).
    (103897, 25, 5),   // Register pressure, strided outputs, float FMA, break(0/3).
];

fn random_case(seed: u64, n: u32, fused: bool, regions: &Regions) -> Case {
    let mut rng = Rng(seed);
    let mut p = random_kernel(&mut rng);
    let name = format!("seed {seed:#018x} n={n}");
    if fused {
        opt::optimize(&mut p);
        opt::fuse_fma(&mut p);
    }
    ir::validate(&p, regions).unwrap_or_else(|err| panic!("{name} fused={fused}: {err}"));
    let ctx = params(&mut rng);
    let bufs = Bufs::new(&mut rng, n as usize);
    let bufs = bufs.words.into_iter().enumerate().map(|(i, w)| (w[G..w.len() - G].to_vec(), i >= 2)).collect::<Vec<_>>();
    layout(name, p, true, &ctx, &SHARED, &bufs, n)
}

fn has_float_fma(body: &Block) -> bool {
    body.iter().any(|stmt| match stmt {
        Stmt::Def(_, ir::Op::Fma(kind, ..)) => *kind != ir::Fma::MulAddI,
        Stmt::If(_, yes, no) => has_float_fma(yes) || has_float_fma(no),
        Stmt::Loop { body, .. } => has_float_fma(body),
        _ => false,
    })
}

fn uses_fma(p: &Program) -> bool {
    has_float_fma(&p.body) || p.funcs.iter().any(|f| has_float_fma(&f.body))
}

fn word(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}

fn blob(out: &mut Vec<u8>, bytes: &[u8]) {
    word(out, bytes.len().try_into().unwrap());
    out.extend_from_slice(bytes);
}

// Each group in the batch is one page test. Serialize one case at a time so
// longer runs can release the programs and interpreter images as they go.
fn group(out: &mut Vec<u8>, name: &str, cases: impl ExactSizeIterator<Item = Case>) {
    blob(out, name.as_bytes());
    word(out, cases.len().try_into().unwrap());
    for case in cases {
        blob(out, case.name.as_bytes());
        let (start, end) = case.frame_words();
        for n in [case.n, case.k_base(), case.ctx + 4 * K_WORK, case.ctx, case.state, case.shared, case.table, case.frame, start as u32, end as u32] {
            word(out, n);
        }
        word(out, case.image.len().try_into().unwrap());
        for w in case.image.iter().chain(&case.expected) {
            word(out, *w);
        }
        let relaxed = uses_fma(&case.program);
        let modes: &[bool] = if relaxed { &[false, true] } else { &[false] };
        word(out, modes.len() as u32);
        for &relaxed_fma in modes {
            let (module, simd) = case.module(relaxed_fma).unwrap_or_else(|| panic!("{name} {}: backend declined", case.name));
            word(out, u32::from(simd));
            blob(out, &module);
        }
    }
}

fn batch() -> Vec<u8> {
    let mut out = Vec::new();
    blob(&mut out, &wasm::fma_probe(wasm::Target::default()));
    let regions = Regions { ctx: CTX as u32, state: 1, shared: SHARED.len() as u32, frame: FRAME, shared_writable: false, bufs: vec![false, false, true, true, true], io: false };
    let rounds = std::env::var("WASM_FUZZ_ROUNDS").map_or(DEFAULT_ROUNDS.len(), |s| s.parse::<usize>().expect("WASM_FUZZ_ROUNDS is a nonnegative integer"));
    for (name, fused) in [("random kernels", false), ("optimized and fused kernels", true)] {
        let mut rng = Rng(0xc0ff_ee12_3456_789b);
        let cases = (0..rounds).map(|round| {
            let (seed, plain_n, fused_n) = DEFAULT_ROUNDS.get(round).copied().unwrap_or_else(|| {
                let seed = rng.next();
                let n = (seed % 32) as u32;
                (seed, n, 31 - n)
            });
            random_case(seed, if fused { fused_n } else { plain_n }, fused, &regions)
        });
        group(&mut out, name, cases);
    }

    // Run each source kernel far enough to exercise its branches and tables.
    // The fbm kernel advances a row at index 1024.
    let source = |name: &str, src: &str, n| source_case(name, src, &[], &[("amp", 3.0)], 1.25, n);
    group(&mut out, "source corpus", CORPUS.iter().map(|&(name, src)| source(name, src, if name == "fbm" { 1029 } else { 37 })));

    // The vec4 table kernel writes a distinct record per element. Cover each
    // short tail and both sides of a vector boundary with the same program.
    let &(name, src) = CORPUS.iter().find(|(name, _)| *name == "rows").unwrap();
    group(&mut out, "vector boundaries", [0, 1, 2, 3, 4, 5, 7, 8, 9].into_iter().map(|n| source(name, src, n)));
    group(&mut out, "oversized buffers", [1, 6, 39].into_iter().map(|n| source_case_spare(name, src, &[], &[("amp", 3.0)], 1.25, n, 5, &|_, _| {})));

    group(&mut out, "multiply-add midpoints", FMA_KERNELS.iter().zip(["a*b+c", "c-a*b", "a*b-c"]).map(|(src, form)| {
        let case = source_case_with(&format!("{form} seed 1"), src, &[], &[], 0.0, 31, &|buffer, words| {
            if buffer == 1 {
                wasm_case::fma_inputs(1, words);
            }
        });
        assert!(case.parallel_safe && wasm::simd_supported(&case.program) && uses_fma(&case.program), "{}: expected a four-wide FMA", case.name);
        case
    }));
    out
}

fn run(root: &Path, data: &[u8]) -> io::Result<ExitCode> {
    let dir = root.join(DIRS[0]);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("wasm_diff.test.mjs"), PAGE)?;
    fs::write(root.join("batch.bin"), data)?;
    Ok(makepad_web_test::main(root, DIRS))
}

fn main() -> ExitCode {
    if let Some(code) = makepad_cef::run_helper_if_subprocess() {
        std::process::exit(code);
    }
    if !cfg!(target_os = "linux") || std::env::args().any(|arg| arg == "--list") {
        return makepad_web_test::main(Path::new(env!("CARGO_MANIFEST_DIR")), DIRS);
    }
    let data = batch();
    let root = std::env::temp_dir().join(format!("makepad-compute-web-wasm-{}", std::process::id()));
    fs::create_dir(&root).expect("create private batch directory");
    let result = run(&root, &data);
    fs::remove_dir_all(root).expect("remove batch directory");
    result.expect("serve wasm batch")
}
