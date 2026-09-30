//! Kernel benchmarks: a 1M-vertex heightfield displacement (fbm noise +
//! normals) and a 100k-instance transform (position, quaternion, scale ->
//! mat4 + colour), native vs the reference interpreter, 1 and N threads.
//!
//! cargo run --release -p makepad-script-compute --example bench_kernels

use makepad_script_compute::kernel::compile_with;
use makepad_script_compute::Backend;
use std::time::Instant;

const DISPLACE: &str = r#"
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
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let n = 1024 * 1024;
    let base: Vec<f32> = (0..n).map(|i| ((i % 97) as f32) * 0.01).collect();
    for backend in [Backend::Native, Backend::Interp] {
        let k = compile_with(DISPLACE, &[], backend).unwrap_or_else(|e| panic!("{:?}", e));
        let mut pos = vec![0.0f32; n * 3];
        let mut nrm = vec![0.0f32; n * 3];
        for t in [1, threads] {
            if backend == Backend::Interp && t > 1 {
                continue;
            }
            let mut c = k.call();
            c.input("base", &base).unwrap();
            c.output("pos", &mut pos).unwrap();
            c.output("nrm", &mut nrm).unwrap();
            let t0 = Instant::now();
            if t == 1 { c.run(n).unwrap() } else { c.run_parallel(n, t).unwrap() };
            let ms = t0.elapsed().as_secs_f64() * 1e3;
            println!("heightfield 1M verts  {:?} x{} threads: {:8.1} ms ({:.1} ns/vertex)", backend, t, ms, ms * 1e6 / n as f64);
        }
    }
    let m = 100_000;
    let src: Vec<f32> = (0..m * 8).map(|i| ((i * 7919) % 1000) as f32 * 0.001 + 0.1).collect();
    for backend in [Backend::Native, Backend::Interp] {
        let k = compile_with(INSTANCES, &[], backend).unwrap_or_else(|e| panic!("{:?}", e));
        let mut inst = vec![0.0f32; m * 24];
        for t in [1, threads] {
            if backend == Backend::Interp && t > 1 {
                continue;
            }
            let mut c = k.call();
            c.set_param("t", 1.5);
            c.input("src", &src).unwrap();
            c.output("inst", &mut inst).unwrap();
            let t0 = Instant::now();
            if t == 1 { c.run(m).unwrap() } else { c.run_parallel(m, t).unwrap() };
            let ms = t0.elapsed().as_secs_f64() * 1e3;
            println!("instances 100k        {:?} x{} threads: {:8.2} ms ({:.1} ns/instance)", backend, t, ms, ms * 1e6 / m as f64);
        }
    }
}
