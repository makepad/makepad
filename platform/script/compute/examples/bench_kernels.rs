//! Kernel benchmarks: a 1M-vertex heightfield (fbm noise + normals) and a
//! 100k-instance transform (position, quaternion, scale -> mat4 + colour),
//! on the NEON ×4 code, the scalar code and the reference interpreter, on
//! one thread and on N workers of a persistent pool. Every multi-thread and
//! vector result is checked bit-identical to the one-thread scalar run.
//!
//! The heightfield runs two ways: one pass that evaluates the height three
//! times per vertex (for finite-difference normals), and the two-pass idiom
//! (heights first, then normals from the neighbouring heights), which is
//! how terrain kernels are written.
//!
//! cargo run --release -p makepad-script-compute --example bench_kernels
//! (BENCH_INTERP=1 also times the interpreter, which takes seconds.)

use makepad_script_compute::kernel::{compile_with, Kernel};
use makepad_script_compute::sched::{Executor, InlineExecutor, Job, ThreadExecutor};
use makepad_script_compute::Backend;
use std::sync::Arc;
use std::time::Instant;

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

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

/// Best of `reps` wall times (ms) of `f`.
fn best(reps: usize, mut f: impl FnMut()) -> f64 {
    (0..reps)
        .map(|_| {
            let t0 = Instant::now();
            f();
            t0.elapsed().as_secs_f64() * 1e3
        })
        .fold(f64::MAX, f64::min)
}

struct Run {
    label: &'static str,
    simd: bool,
    threads: usize,
}

fn main() {
    let threads = 8;
    let exec = ThreadExecutor::new(threads - 1);
    let n = W * W;
    let base: Arc<[f32]> = (0..n).map(|i| ((i % 97) as f32) * 0.01).collect::<Vec<_>>().into();
    let interp = std::env::var("BENCH_INTERP").is_ok();
    let runs = [Run { label: "scalar", simd: false, threads: 1 }, Run { label: "neon", simd: true, threads: 1 }, Run { label: "neon", simd: true, threads }];
    let run = |job: &mut Job, r: &Run| {
        job.set_simd(r.simd);
        let e: &dyn Executor = if r.threads > 1 { &exec } else { &InlineExecutor };
        job.run(e, r.threads).unwrap();
    };
    let native = |src: &str| compile_with(src, &[], Backend::Native).unwrap_or_else(|e| panic!("{:?}", e));

    // -- one pass: three height evaluations per vertex --------------------------
    let k = native(ONE_PASS);
    println!("heightfield, 1M vertices (NEON code: {})", k.simd());
    let mut job = Job::new(k.clone(), n);
    job.input("base", base.clone()).unwrap();
    job.output("pos", vec![0.0; n * 3]).unwrap();
    job.output("nrm", vec![0.0; n * 3]).unwrap();
    let mut reference = None;
    for r in &runs {
        let ms = best(3, || run(&mut job, r));
        let out = (bits(job.out("pos").unwrap()), bits(job.out("nrm").unwrap()));
        let same = match &reference {
            None => {
                reference = Some(out);
                "reference"
            }
            Some(want) => {
                assert!(*want == out, "{} x{} differs from the one-thread scalar run", r.label, r.threads);
                "bit-identical"
            }
        };
        println!("  one pass (3 fbm/vertex)  {:6} x{} threads: {:7.2} ms  ({:5.1} ns/vertex) {}", r.label, r.threads, ms, ms * 1e6 / n as f64, same);
    }
    if interp {
        let ki = compile_with(ONE_PASS, &[], Backend::Interp).unwrap();
        let mut j = Job::new(ki, n);
        j.input("base", base.clone()).unwrap();
        j.output("pos", vec![0.0; n * 3]).unwrap();
        j.output("nrm", vec![0.0; n * 3]).unwrap();
        let ms = best(1, || {
            j.run(&InlineExecutor, 1).unwrap();
        });
        assert!(reference.as_ref().unwrap().0 == bits(j.out("pos").unwrap()));
        println!("  one pass                 interp x1 threads: {:7.1} ms  bit-identical", ms);
    }

    // -- two passes: heights, then normals from the heights ----------------------
    let kh = native(HEIGHTS);
    let kn = native(NORMALS);
    let mut jh = Job::new(kh.clone(), n);
    jh.input("base", base.clone()).unwrap();
    jh.output("pos", vec![0.0; n * 3]).unwrap();
    jh.output("hgt", vec![0.0; n]).unwrap();
    let mut reference: Option<(Vec<u32>, Vec<u32>)> = None;
    for r in &runs {
        let mut jn: Option<Job> = None;
        let mut hms = f64::MAX;
        let mut nms = f64::MAX;
        let ms = best(5, || {
            let t0 = Instant::now();
            run(&mut jh, r);
            hms = hms.min(t0.elapsed().as_secs_f64() * 1e3);
            // Pass 2 reads pass 1's heights (shared, read-only).
            let h: Arc<[f32]> = jh.out("hgt").unwrap().into();
            let j = jn.get_or_insert_with(|| {
                let mut j = Job::new(kn.clone(), n);
                j.output("nrm", vec![0.0; n * 3]).unwrap();
                j
            });
            j.input("hgt", h).unwrap();
            let t1 = Instant::now();
            run(j, r);
            nms = nms.min(t1.elapsed().as_secs_f64() * 1e3);
        });
        let out = (bits(jh.out("pos").unwrap()), bits(jn.as_ref().unwrap().out("nrm").unwrap()));
        let same = match &reference {
            None => {
                reference = Some(out);
                "reference"
            }
            Some(want) => {
                assert!(*want == out, "{} x{} differs from the one-thread scalar run", r.label, r.threads);
                "bit-identical"
            }
        };
        println!(
            "  two passes (heights + normals) {:6} x{} threads: {:7.2} ms = {:.2} + {:.2}  ({:4.1} ns/vertex) {}",
            r.label,
            r.threads,
            ms,
            hms,
            nms,
            ms * 1e6 / n as f64,
            same
        );
    }

    // -- instances -------------------------------------------------------------------
    let m = 100_000;
    let src: Arc<[f32]> = (0..m * 8).map(|i| ((i * 7919) % 1000) as f32 * 0.001 + 0.1).collect::<Vec<_>>().into();
    let k: Arc<Kernel> = native(INSTANCES);
    println!("instances, 100k (NEON code: {})", k.simd());
    let mut job = Job::new(k, m);
    job.set_param("t", 1.5);
    job.input("src", src).unwrap();
    job.output("inst", vec![0.0; m * 24]).unwrap();
    let mut reference = None;
    for r in &runs {
        let ms = best(5, || run(&mut job, r));
        let out = bits(job.out("inst").unwrap());
        let same = match &reference {
            None => {
                reference = Some(out);
                "reference"
            }
            Some(want) => {
                assert!(*want == out, "instances {} x{} differ", r.label, r.threads);
                "bit-identical"
            }
        };
        println!("  {:6} x{} threads: {:6.2} ms  ({:5.1} ns/instance) {}", r.label, r.threads, ms, ms * 1e6 / m as f64, same);
    }
}
