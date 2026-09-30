//! Kernel dispatch through the platform TaskPool (the render-kernels
//! engine installed on a Cx, as Sandbox and the effect kits run): the
//! median wall time of one MustComplete job (admission, run_sync, the
//! Heavy lane's fan-out) for a cheap kernel and a noise kernel, from a
//! worker thread (on the UI thread the engine never fans out).
//!
//! cargo run --release -p makepad-render-kernels --example bench_task_pool

use makepad_platform::Cx;
use makepad_render_kernels::compute::sched::{Job, Priority};
use makepad_render_kernels::{engine, install};
use std::time::{Duration, Instant};

const CHEAP: &str = "let x = input(f32)\nlet y = output(f32)\nlet a = param(2.0)\nfn element(i) { y[i] = x[i] * a + 1.0 }";
const NOISE: &str = "let W = 1024\nlet base = input(f32)\nlet pos = output(vec3)\nlet hgt = output(f32)\nlet amp = param(6.0)\nfn vertex(i) { let x = float(i % W)\n let z = float(i / W)\n let h = base[i] + fbm2(vec2(x, z) * 0.01, 4, 2.0, 0.5) * amp\n pos[i] = vec3(x, h, z)\n hgt[i] = h }";

fn main() {
    let cx = Cx::new(Box::new(|_, _| {}));
    assert!(install(&cx));
    let e = engine();
    println!("TaskPool engine: {} threads per job (heavy workers + caller)", e.threads());
    std::thread::spawn(move || {
        for (name, src, cheap) in [("madd", CHEAP, true), ("fbm", NOISE, false)] {
            let k = makepad_render_kernels::compile(src, &[], &[]).unwrap();
            println!("\n{}: median us per MustComplete job", name);
            for n in [1usize, 256, 1024, 4096, 16384, 65536, 262144] {
                let input: std::sync::Arc<[f32]> = (0..n).map(|i| (i % 97) as f32 * 0.01).collect::<Vec<_>>().into();
                let mut job = Job::new(k.clone(), n);
                if cheap {
                    job.input("x", input).unwrap();
                    job.output("y", vec![0.0; n]).unwrap();
                } else {
                    job.input("base", input).unwrap();
                    job.output("pos", vec![0.0; 3 * n]).unwrap();
                    job.output("hgt", vec![0.0; n]).unwrap();
                }
                let mut slot = Some(job);
                let mut times = Vec::new();
                for r in 0..60 {
                    let t = Instant::now();
                    let j = e.run(slot.take().unwrap(), Priority::MustComplete, Duration::from_secs(10)).unwrap_or_else(|(_, err)| panic!("{}", err));
                    slot = Some(j);
                    if r >= 10 {
                        times.push(t.elapsed().as_secs_f64() * 1e6);
                    }
                }
                times.sort_by(|a, b| a.partial_cmp(b).unwrap());
                println!("{:>7} {:>9.2} us  ({:.2} ns/element)", n, times[times.len() / 2], times[times.len() / 2] * 1e3 / n as f64);
            }
        }
    })
    .join()
    .unwrap();
}
