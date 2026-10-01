//! Differential check of the wasm backend in V8 (the engine of Chrome and
//! node), where its SIMD128 form runs: every case is run by the AIR
//! interpreter here, then by the generated modules in node (scalar `run0`,
//! four-wide `run1` with the scalar tail, each with exact and, when the
//! engine's `relaxed_madd` is fused, relaxed multiply-adds), every word of
//! memory compared with guard words around every buffer.
//!
//! Cases: random kernel-shaped programs (the NEON fuzz generator: divergent
//! and uniform branches, loops with breaks and continues at every depth,
//! per-lane frames, hostile loads, integer division, shifts, selects) and
//! their optimized fused-multiply-add forms; the kernels-from-source corpus
//! at sizes around multiples of 4; multiply-adds at double-rounding
//! midpoints; and the plate kernels when `--plates <dir>` names them.
//!
//! ```text
//! cargo run --release -p makepad-script-compute --example wasm_diff -- \
//!     [--rounds 3000] [--seed N] [--tier turbofan|liftoff|both] \
//!     [--plates local/agent_state/edits/scratch/KC1/kernels] [--dir <scratch>]
//! ```
//!
//! Needs `node` (with wasm SIMD) on the PATH. A failing random program is
//! minimized (statements deleted while it still fails) and printed.

#[path = "../tests/support/kernel_gen.rs"]
mod kernel_gen;
#[path = "../tests/support/wasm_case.rs"]
mod wasm_case;

use kernel_gen::*;
use makepad_script_compute::ir::{self, Block, Program, Region, Regions, Stmt};
use makepad_script_compute::kernel::{FieldTy, Layout, LayoutField};
use makepad_script_compute::wasm;
use std::collections::BTreeMap;
use wasm_case::*;

/// How to rebuild a random case around a changed program.
#[derive(Clone)]
struct Setup {
    n: u32,
    ctx: Vec<u32>,
    seed: u64,
}

fn random_case(p: Program, s: &Setup, name: String) -> Case {
    let bufs = Bufs::new(&mut Rng(s.seed), s.n as usize);
    let bufs: Vec<(Vec<u32>, bool)> = bufs.words.iter().enumerate().map(|(k, w)| (w[G..w.len() - G].to_vec(), k >= 2)).collect();
    layout(name, p, true, &s.ctx, &SHARED, &bufs, s.n)
}

fn has_fma(b: &Block) -> bool {
    b.iter().any(|s| match s {
        Stmt::Def(_, ir::Op::Fma(k, ..)) => *k != ir::Fma::MulAddI,
        Stmt::If(_, t, e) => has_fma(t) || has_fma(e),
        Stmt::Loop { body, .. } => has_fma(body),
        _ => false,
    })
}

/// The batch file node reads (see wasm_diff.mjs).
fn batch(cases: &[Case]) -> Vec<u8> {
    let mut out = Vec::new();
    let u = |out: &mut Vec<u8>, x: u32| out.extend_from_slice(&x.to_le_bytes());
    u(&mut out, 0x3146_4457);
    let probe = wasm::fma_probe();
    u(&mut out, probe.len() as u32);
    out.extend_from_slice(&probe);
    u(&mut out, cases.len() as u32);
    for c in cases {
        u(&mut out, c.name.len() as u32);
        out.extend_from_slice(c.name.as_bytes());
        for x in [c.n, c.k_base(), c.ctx, c.state, c.shared, c.table, c.frame] {
            u(&mut out, x);
        }
        u(&mut out, c.image.len() as u32);
        for w in &c.image {
            u(&mut out, *w);
        }
        // The frame (scratch) is not compared; the rest of the image is.
        let (fa, fb) = c.frame_words();
        u(&mut out, fa as u32);
        u(&mut out, fb as u32);
        for w in &c.expected {
            u(&mut out, *w);
        }
        let relaxed = has_fma(&c.program.body);
        let mods: Vec<(u32, Vec<u8>)> = [false, true]
            .into_iter()
            .filter(|r| !*r || relaxed)
            .filter_map(|r| c.module(r).map(|(m, simd)| (r as u32 | (simd as u32) << 1, m)))
            .collect();
        u(&mut out, mods.len() as u32);
        for (flags, m) in mods {
            u(&mut out, flags);
            u(&mut out, m.len() as u32);
            out.extend_from_slice(&m);
        }
    }
    out
}

struct NodeRun {
    probe: String,
    /// Failing case index -> its first failure.
    fails: BTreeMap<usize, String>,
    runs: usize,
}

fn run_node(cases: &[Case], dir: &std::path::Path, flags: &[&str]) -> NodeRun {
    let path = dir.join("batch.bin");
    std::fs::write(&path, batch(cases)).expect("write the batch");
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/wasm_diff.mjs");
    let out = std::process::Command::new("node").args(flags).arg(script).arg(&path).output().expect("run node (is it on the PATH?)");
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || !text.contains("DONE") {
        panic!("node failed: {}\n{}", text, String::from_utf8_lossy(&out.stderr));
    }
    let mut r = NodeRun { probe: String::new(), fails: BTreeMap::new(), runs: 0 };
    for l in text.lines() {
        let w: Vec<&str> = l.splitn(3, ' ').collect();
        match w[0] {
            "PROBE" => r.probe = w[1].to_string(),
            "FAIL" => {
                let k: usize = w[1].parse().unwrap();
                r.fails.entry(k).or_insert_with(|| w[2].to_string());
            }
            "DONE" => r.runs = l.split(' ').nth(2).and_then(|x| x.parse().ok()).unwrap_or(0),
            _ => {}
        }
    }
    r
}

/// Deletes statements while the program still fails in node.
fn minimize(mut p: Program, s: &Setup, regions: &Regions, dir: &std::path::Path, flags: &[&str]) -> (Program, String) {
    fn count(b: &Block) -> usize {
        b.iter()
            .map(|s| {
                1 + match s {
                    Stmt::If(_, t, e) => count(t) + count(e),
                    Stmt::Loop { body, .. } => count(body),
                    _ => 0,
                }
            })
            .sum()
    }
    // The per-element initialization (frame zeroing, variable setup) stays:
    // without it a program reads uninitialized memory.
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
    let mut why = String::new();
    loop {
        let total = count(&p.body);
        let cands: Vec<Program> = (0..total)
            .filter_map(|k| {
                let mut q = p.clone();
                let mut kk = k;
                (remove(&mut q.body, &mut kk, 0) && ir::validate(&q, regions).is_ok() && wasm::simd_supported(&q)).then_some(q)
            })
            .collect();
        let cases: Vec<Case> = cands.iter().enumerate().map(|(k, q)| random_case(q.clone(), s, format!("candidate {}", k))).collect();
        let r = run_node(&cases, dir, flags);
        match r.fails.iter().next() {
            Some((k, w)) => {
                p = cands[*k].clone();
                why = w.clone();
            }
            None => return (p, why),
        }
    }
}

fn segment() -> Layout {
    Layout {
        name: "Segment".into(),
        stride: 11,
        fields: vec![
            LayoutField { name: "a".into(), ty: FieldTy::Vec3, offset: 0 },
            LayoutField { name: "b".into(), ty: FieldTy::Vec3, offset: 3 },
            LayoutField { name: "color".into(), ty: FieldTy::Vec4, offset: 6 },
            LayoutField { name: "width".into(), ty: FieldTy::F32, offset: 10 },
        ],
    }
}

/// The plate kernels' params (as the kernel bench sets them).
fn plate_params(name: &str) -> (Vec<(String, f32)>, f32) {
    let v = |x: &[(&str, f32)]| x.iter().map(|(n, v)| (n.to_string(), *v)).collect::<Vec<_>>();
    match name {
        "sk_lines" => (
            v(&[
                ("pa_x", 3.0), ("pa_y", 8.0), ("pa_z", 3.0), ("pa_w", 0.4), ("pb_x", 1.0), ("pb_y", 1.6), ("pb_z", 0.8), ("pb_w", -40.0),
                ("pc_x", 0.5), ("pc_y", -60.0), ("pc_z", 45.0), ("pc_w", 50.0), ("pd_x", 1.2), ("pd_y", 1.0), ("pd_z", 0.3), ("pd_w", 0.2),
                ("pe_x", 0.5), ("pe_y", 0.2), ("pe_z", -0.3), ("pe_w", 0.0),
            ]),
            0.0,
        ),
        "k_myc" => (
            v(&[
                ("c_px", 0.0), ("c_py", 1.5), ("c_pz", 6.0), ("c_rx", 1.0), ("c_ry", 0.0), ("c_rz", 0.0), ("c_ux", 0.0), ("c_uy", 1.0), ("c_uz", 0.0),
                ("c_fx", 0.0), ("c_fy", 0.0), ("c_fz", -1.0), ("c_ff", 1100.0), ("c_wa", 3.0), ("c_wt", 31.5), ("c_lk", 0.0), ("b_th", 0.3), ("b_sd", 1.0),
                ("c_am", 1.0), ("m_front", 40.0),
            ]),
            5.0,
        ),
        "k_room" => {
            let mut p = Vec::new();
            for q in 0..8 {
                for (f, v) in [("px", 0.0), ("py", 1.5), ("pz", 6.0), ("rx", 1.0), ("ry", 0.0), ("rz", 0.0), ("ux", 0.0), ("uy", 1.0), ("uz", 0.0), ("ff", 1100.0), ("wa", 3.0), ("wt", 31.5), ("lk", 0.0), ("am", 1.0)] {
                    p.push((format!("q{}_{}", q, f), v));
                }
            }
            p.push(("b_th".into(), 0.3));
            p.push(("b_sd".into(), 1.0));
            (p, 5.0)
        }
        _ => (Vec::new(), 0.0),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let rounds: u64 = arg("--rounds").and_then(|x| x.parse().ok()).unwrap_or(3000);
    let seed: u64 = arg("--seed").and_then(|x| x.parse().ok()).unwrap_or(0xC0FF_EE12_3456_789B);
    let dir = arg("--dir").map(std::path::PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("wasm_diff"));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let tiers: Vec<(&str, Vec<&str>)> = match arg("--tier").as_deref().unwrap_or("both") {
        "turbofan" => vec![("turbofan", vec!["--no-liftoff"])],
        "liftoff" => vec![("liftoff", vec!["--liftoff-only"])],
        _ => vec![("turbofan", vec!["--no-liftoff"]), ("liftoff", vec!["--liftoff-only"])],
    };
    let regions = Regions { ctx: CTX as u32, state: 1, shared: SHARED.len() as u32, frame: FRAME, shared_writable: false, bufs: vec![false, false, true, true, true], io: false };

    // Random programs.
    let mut r = Rng(seed);
    let mut cases = Vec::new();
    let mut setups: Vec<Option<Setup>> = Vec::new();
    for round in 0..rounds {
        let p = random_kernel(&mut r);
        ir::validate(&p, &regions).unwrap_or_else(|e| panic!("round {}: invalid program: {}", round, e));
        let mut forms = vec![("plain", p.clone())];
        if round % 2 == 0 {
            let mut q = p;
            makepad_script_compute::opt::optimize(&mut q);
            makepad_script_compute::opt::fuse_fma(&mut q);
            if ir::validate(&q, &regions).is_ok() {
                forms.push(("fused", q));
            }
        }
        for (form, p) in forms {
            let s = Setup { n: [1u32, 3, 4, 5, 8, 13, 16, 31][r.below(8) as usize], ctx: params(&mut r), seed: r.next() };
            cases.push(random_case(p, &s, format!("round {} {}", round, form)));
            setups.push(Some(s));
        }
    }
    let random = cases.len();
    // Kernels from source.
    for (name, src) in CORPUS {
        for n in [4u32, 7, 37, 1029] {
            cases.push(source_case(name, src, &[], &[("amp", 3.0)], 1.25, n));
            setups.push(None);
        }
    }
    for (k, src) in FMA_KERNELS.iter().enumerate() {
        for seed in 1..4u64 {
            cases.push(source_case_with(&format!("fma{} seed {}", k, seed), src, &[], &[], 0.0, 4099, &|b, w| {
                if b == 1 {
                    fma_inputs(seed, w)
                }
            }));
            setups.push(None);
        }
    }
    if let Some(dir) = arg("--plates") {
        for name in ["sk_lines", "k_myc", "k_room"] {
            let Ok(src) = std::fs::read_to_string(format!("{}/{}.splash", dir, name)) else {
                eprintln!("no plate {} in {}", name, dir);
                continue;
            };
            let (params, time) = plate_params(name);
            let params: Vec<(&str, f32)> = params.iter().map(|(n, v)| (n.as_str(), *v)).collect();
            for n in [1027u32, 2050] {
                cases.push(source_case(name, &src, &[segment()], &params, time, n));
                setups.push(None);
            }
        }
    }
    let simd = cases.iter().filter(|c| c.parallel_safe && wasm::simd_supported(&c.program)).count();
    eprintln!("{} cases ({} random), {} four wide", cases.len(), random, simd);
    let scalar_only: Vec<&str> = cases[random..].iter().filter(|c| !(c.parallel_safe && wasm::simd_supported(&c.program))).map(|c| c.name.as_str()).collect();
    if !scalar_only.is_empty() {
        eprintln!("scalar only: {}", scalar_only.join(", "));
    }

    let mut failed = false;
    for (tier, flags) in &tiers {
        let t0 = std::time::Instant::now();
        let r = run_node(&cases, &dir, flags);
        eprintln!("{}: relaxed_madd {}, {} runs, {} failing cases ({:.1} s)", tier, r.probe, r.runs, r.fails.len(), t0.elapsed().as_secs_f64());
        for (k, why) in r.fails.iter().take(8) {
            eprintln!("  case {} ({}): {}", k, cases[*k].name, why);
        }
        if let Some((k, _)) = r.fails.iter().find(|(k, _)| setups[**k].is_some()) {
            let s = setups[*k].clone().unwrap();
            let (min, why) = minimize(cases[*k].program.clone(), &s, &regions, &dir, flags);
            eprintln!("minimized case {} ({}): {}\n{:?}", k, cases[*k].name, why, min.body);
        }
        failed |= !r.fails.is_empty();
    }
    std::fs::remove_file(dir.join("batch.bin")).ok();
    if failed {
        std::process::exit(1);
    }
    eprintln!("all bit-equal to the interpreter");
}
