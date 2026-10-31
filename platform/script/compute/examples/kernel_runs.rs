//! (VEC6 tool, not timing) compiles bench_kernels' kernels; COMPUTE_NATIVE_DUMP=<dir> writes their RIR + code.
//! `kernel_runs <one_pass|heights|normals|instances> <scalar|x4|x8> <reps>` also runs one kernel one-thread `reps`
//! times (for instruction counts: perf stat -e instructions:u, minus a reps=0 run).
//! `kernel_runs compile <x4|x8> <reps>`: compiles the four kernels `reps` times each at that width (no kernel store) and
//! prints the thread CPU ns per compile of each (compile time per kernel).
use makepad_script_compute::kernel::compile_with;
use makepad_script_compute::{native_vec, Backend};
use makepad_script_compute_sched::sched::{InlineExecutor, Job};
use std::sync::Arc;

#[path = "../tests/support/geometry.rs"]
mod geometry;
use geometry::*;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        for (name, s) in [("one_pass", ONE_PASS), ("heights", HEIGHTS), ("normals", NORMALS), ("instances", INSTANCES)] {
            let k = compile_with(s, &[], Backend::Native).unwrap_or_else(|e| panic!("{:?}", e));
            println!("{} simd={}", name, k.simd());
        }
        return;
    }
    let (simd, reps) = (a[2] != "scalar", a[3].parse::<usize>().unwrap());
    let lanes = if a[2] == "x8" { 8 } else { 4 };
    if a[1] == "compile" {
        for (name, s) in [("one_pass", ONE_PASS), ("heights", HEIGHTS), ("normals", NORMALS), ("instances", INSTANCES)] {
            let t0 = relax_backend::cpu_ns();
            for _ in 0..reps {
                let k = native_vec::with_lanes(lanes, || compile_with(s, &[], Backend::Native)).unwrap_or_else(|e| panic!("{:?}", e));
                assert_eq!(k.vector_lanes(), lanes);
            }
            let ns = (relax_backend::cpu_ns() - t0) / reps.max(1) as u64;
            println!("compile {} x{} {:.3} ms/compile (scalar + vector code, thread CPU, {} reps)", name, lanes, ns as f64 * 1e-6, reps);
        }
        return;
    }
    let n = W * W;
    let base: Arc<[f32]> = (0..n).map(|i| ((i % 97) as f32) * 0.01).collect::<Vec<_>>().into();
    let (src, setup): (&str, &dyn Fn(&mut Job)) = match a[1].as_str() {
        "one_pass" => (ONE_PASS, &|j: &mut Job| { j.input("base", base.clone()).unwrap(); j.output("pos", vec![0.0; n * 3]).unwrap(); j.output("nrm", vec![0.0; n * 3]).unwrap(); }),
        "heights" => (HEIGHTS, &|j: &mut Job| { j.input("base", base.clone()).unwrap(); j.output("pos", vec![0.0; n * 3]).unwrap(); j.output("hgt", vec![0.0; n]).unwrap(); }),
        "normals" => (NORMALS, &|j: &mut Job| { j.input("hgt", base.clone()).unwrap(); j.output("nrm", vec![0.0; n * 3]).unwrap(); }),
        _ => (INSTANCES, &|j: &mut Job| {
            let m = 100_000;
            let s: Arc<[f32]> = (0..m * 8).map(|i| ((i * 7919) % 1000) as f32 * 0.001 + 0.1).collect::<Vec<_>>().into();
            j.set_param("t", 1.5);
            j.input("src", s).unwrap();
            j.output("inst", vec![0.0; m * 24]).unwrap();
        }),
    };
    let count = if a[1] == "instances" { 100_000 } else { n };
    let k = native_vec::with_lanes(lanes, || compile_with(src, &[], Backend::Native)).unwrap_or_else(|e| panic!("{:?}", e));
    assert!(!simd || k.vector_lanes() == lanes, "no x{} code", lanes);
    let mut job = Job::new(k, count);
    setup(&mut job);
    job.set_simd(simd);
    for _ in 0..reps {
        job.run(&InlineExecutor, 1).unwrap();
    }
    println!("{} {} reps={} count={}", a[1], a[2], reps, count);
}

