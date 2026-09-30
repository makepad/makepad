//! Scheduling overhead: what a kernel call costs besides its elements.
//!
//! For a cheap kernel (a multiply-add per element) and a noise kernel
//! (fbm heights, ~17 ns/element on NEON), at sizes from 16 to 256k
//! elements: the median wall time of one call run on the caller
//! (`Call::run`), split over the process pool (`Call::run_parallel` with
//! 2, 4 and 8 threads, as Stage's Scene3D kernels run), and as a
//! scheduler job (`Scheduler::run_sync`, admission included, the way the
//! render-kernels engine runs MustComplete work) and a submitted job
//! (`submit` + `wait`, Near priority). Also the empty call (1 element) and
//! the chunking each size gets.
//!
//! cargo run --release -p makepad-script-compute --example bench_dispatch

use makepad_script_compute::kernel::{compile, Kernel};
use makepad_script_compute::admission::{JobBudget, Origin};
use makepad_script_compute::sched::{Executor, Job, Priority, Scheduler, SchedulerConfig, ThreadExecutor};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CHEAP: &str = "let x = input(f32)\nlet y = output(f32)\nlet a = param(2.0)\nfn element(i) { y[i] = x[i] * a + 1.0 }";
const NOISE: &str = "let W = 1024\nlet base = input(f32)\nlet pos = output(vec3)\nlet hgt = output(f32)\nlet amp = param(6.0)\nfn vertex(i) { let x = float(i % W)\n let z = float(i / W)\n let h = base[i] + fbm2(vec2(x, z) * 0.01, 4, 2.0, 0.5) * amp\n pos[i] = vec3(x, h, z)\n hgt[i] = h }";

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Median microseconds of `f` over enough repetitions for ~40 ms.
fn time(mut f: impl FnMut()) -> f64 {
    f();
    let t0 = Instant::now();
    f();
    let one = t0.elapsed().as_secs_f64();
    let reps = ((0.04 / one.max(1e-7)) as usize).clamp(5, 20_000);
    median((0..reps).map(|_| {
        let t = Instant::now();
        f();
        t.elapsed().as_secs_f64() * 1e6
    }).collect())
}

fn bind<'a>(k: &'a Kernel, cheap: bool, input: &'a [f32], out: &'a mut [f32], out2: &'a mut [f32]) -> makepad_script_compute::kernel::Call<'a> {
    let mut c = k.call();
    if cheap {
        c.input("x", input).unwrap();
        c.output("y", out).unwrap();
    } else {
        c.input("base", input).unwrap();
        c.output("pos", out).unwrap();
        c.output("hgt", out2).unwrap();
    }
    c
}

fn main() {
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    println!("{} hardware threads; the process pool has {} workers", workers, ThreadExecutor::shared().workers());
    let sched = Scheduler::new(Arc::new(ThreadExecutor::new(7)), SchedulerConfig { threads: 8, ..SchedulerConfig::default() });
    let sizes = [1usize, 16, 64, 256, 1024, 4096, 16384, 65536, 262144];
    for (name, src, cheap) in [("madd", CHEAP, true), ("fbm", NOISE, false)] {
        let k = compile(src).unwrap();
        println!("\n{} kernel (NEON {}, {} AIR ops per element worst case): median us per call", name, k.simd(), k.cost);
        println!("{:>7} {:>9} {:>9} {:>9} {:>9} {:>10} {:>10} {:>8}  {}", "count", "run x1", "par x2", "par x4", "par x8", "sched x8", "submit x8", "ns/el x1", "best");
        for &n in &sizes {
            let input: Vec<f32> = (0..n.max(1)).map(|i| (i % 97) as f32 * 0.01).collect();
            let w = if cheap { 1 } else { 3 };
            let mut out = vec![0.0f32; n * w];
            let mut out2 = vec![0.0f32; n];
            let t1 = time(|| {
                bind(&k, cheap, &input, &mut out, &mut out2).run(n).unwrap();
            });
            let par: Vec<f64> = [2, 4, 8]
                .iter()
                .map(|&t| {
                    time(|| {
                        bind(&k, cheap, &input, &mut out, &mut out2).run_parallel(n, t).unwrap();
                    })
                })
                .collect();
            // A reusable job: its buffers stay allocated across runs.
            let mut job = Job::new(k.clone(), n);
            let inp: Arc<[f32]> = input.clone().into();
            if cheap {
                job.input("x", inp).unwrap();
                job.output("y", vec![0.0; n.max(1)]).unwrap();
            } else {
                job.input("base", inp).unwrap();
                job.output("pos", vec![0.0; (3 * n).max(1)]).unwrap();
                job.output("hgt", vec![0.0; n.max(1)]).unwrap();
            }
            let budget = JobBudget { wall: Duration::from_secs(10) };
            let ts = time(|| {
                sched.run_sync(&mut job, Origin::Host, budget).unwrap();
            });
            let mut slot = Some(job);
            let tq = time(|| {
                let j = slot.take().unwrap();
                let h = sched.submit(j, Priority::Near, Origin::Host, budget).unwrap_or_else(|(_, e)| panic!("{:?}", e));
                slot = Some(h.wait().unwrap_or_else(|(_, e)| panic!("{:?}", e)));
            });
            let all = [t1, par[0], par[1], par[2], ts, tq];
            let labels = ["x1", "par2", "par4", "par8", "sched", "submit"];
            let (bi, _) = all.iter().enumerate().min_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap();
            println!(
                "{:>7} {:>9.2} {:>9.2} {:>9.2} {:>9.2} {:>10.2} {:>10.2} {:>8.2}  {}",
                n, t1, par[0], par[1], par[2], ts, tq, t1 * 1e3 / n as f64, labels[bi]
            );
        }
    }
}
