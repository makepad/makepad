//! (VEC6 tool, not timing) compiles bench_kernels' kernels; COMPUTE_NATIVE_DUMP=<dir> writes their RIR + code.
//! `kernel_runs <one_pass|heights|normals|instances> <scalar|x4> <reps>` also runs one kernel one-thread `reps`
//! times (for instruction counts: perf stat -e instructions:u, minus a reps=0 run).
use makepad_script_compute::kernel::compile_with;
use makepad_script_compute::Backend;
use makepad_script_compute_sched::sched::{InlineExecutor, Job};
use std::sync::Arc;
const W: usize = 1024;

const ONE_PASS: &str = r#"
    let W = 1024
    let base = input(f32)
    let pos = output(vec3)
    let nrm = output(vec3)
    let amp = param(6.0)
    fn height(x, z) { base[int(z) * W + int(x)] + fbm2(vec2(x, z) * 0.01, 4, 2.0, 0.5) * amp }
    fn vertex(i) {
        let x = float(i % W)
        let z = float(i / W)
        let h = height(x, z)
        pos[i] = vec3(x, h, z)
        let dx = height(x + 1.0, z) - h
        let dz = height(x, z + 1.0) - h
        nrm[i] = normalize(vec3(-dx, 1.0, -dz))
    }
"#;

/// Pass 1: heights (and positions).
const HEIGHTS: &str = r#"
    let W = 1024
    let base = input(f32)
    let pos = output(vec3)
    let hgt = output(f32)
    let amp = param(6.0)
    fn vertex(i) {
        let x = float(i % W)
        let z = float(i / W)
        let h = base[i] + fbm2(vec2(x, z) * 0.01, 4, 2.0, 0.5) * amp
        pos[i] = vec3(x, h, z)
        hgt[i] = h
    }
"#;

/// Pass 2: normals from the neighbours' heights (forward differences;
/// the last row and column reuse their own height).
const NORMALS: &str = r#"
    let W = 1024
    let hgt = input(f32)
    let nrm = output(vec3)
    fn vertex(i) {
        let x = i % W
        let z = i / W
        let h = hgt[i]
        let dx = hgt[if x < W - 1 { i + 1 } else { i }] - h
        let dz = hgt[if z < count / W - 1 { i + W } else { i }] - h
        nrm[i] = normalize(vec3(-dx, 1.0, -dz))
    }
"#;

const INSTANCES: &str = r#"
    let src = input(f32)
    let xf = output(mat4, 24, 0, inst)
    let col = output(vec4, 24, 16, inst)
    let t = param(0.0)
    fn instance(i) {
        let p = vec3(src[i * 8], src[i * 8 + 1], src[i * 8 + 2])
        let q = normalize(vec4(src[i * 8 + 3], src[i * 8 + 4], src[i * 8 + 5], src[i * 8 + 6]))
        let s = src[i * 8 + 7]
        let spin = quat_axis_angle(vec3(0.0, 1.0, 0.0), t + hash01(i, 1) * TAU)
        xf[i] = mat4_trs(p + vec3(0.0, sin(t + float(i) * 0.01) * 0.2, 0.0), quat_mul(spin, q), vec3(s, s, s))
        col[i] = vec4(hash01(i, 2), hash01(i, 3), hash01(i, 4), 1.0)
    }
"#;


fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        for (name, s) in [("one_pass", ONE_PASS), ("heights", HEIGHTS), ("normals", NORMALS), ("instances", INSTANCES)] {
            let k = compile_with(s, &[], Backend::Native).unwrap_or_else(|e| panic!("{:?}", e));
            println!("{} simd={}", name, k.simd());
        }
        return;
    }
    let (simd, reps) = (a[2] == "x4", a[3].parse::<usize>().unwrap());
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
    let k = compile_with(src, &[], Backend::Native).unwrap_or_else(|e| panic!("{:?}", e));
    let mut job = Job::new(k, count);
    setup(&mut job);
    job.set_simd(simd);
    for _ in 0..reps {
        job.run(&InlineExecutor, 1).unwrap();
    }
    println!("{} {} reps={} count={}", a[1], a[2], reps, count);
}
