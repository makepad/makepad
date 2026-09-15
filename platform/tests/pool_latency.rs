//! Headless, release-only scheduling gate. `--baseline` records without gating.
#[cfg(not(gpusim))]
fn main() {
    eprintln!("pool_latency: use MAKEPAD=gpusim cargo test --release -p makepad-platform --test pool_latency");
}

#[cfg(gpusim)]
fn main() {
    use makepad_platform::{
        thread::{machine_topology, Lane, PriorityStatus, ShutdownMode},
        Cx, CxThreadPriority,
    };
    use std::{
        hint::black_box,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };

    assert!(
        !cfg!(debug_assertions),
        "latency measurement requires --release"
    );
    let baseline = std::env::args().any(|arg| arg == "--baseline");
    let ui_priority = Cx::set_thread_priority(CxThreadPriority::UserInteractive);
    println!(
        "pool_latency UI UserInteractive={ui_priority:?} topology={:?}",
        machine_topology()
    );
    assert_eq!(ui_priority, PriorityStatus::Applied);
    let cx = Cx::new(Box::new(|_, _| {}));
    let pool = cx.task_pool();
    let ready = Arc::new(AtomicUsize::new(0));
    let start = Arc::new(std::sync::OnceLock::<Instant>::new());
    let mut jobs = Vec::new();
    for seed in 0..pool.heavy_workers() {
        let ready = ready.clone();
        let start = start.clone();
        jobs.push(
            pool.submit(Lane::Heavy, move || {
                ready.fetch_add(1, Ordering::Release);
                while start.get().is_none() {
                    std::thread::sleep(Duration::from_micros(100));
                }
                let deadline = *start.get().unwrap() + Duration::from_secs(4);
                let mut value = seed as u64 + 1;
                while Instant::now() < deadline {
                    for _ in 0..4096 {
                        value = black_box(value.wrapping_mul(6364136223846793005).wrapping_add(1));
                    }
                }
                black_box(value)
            })
            .unwrap(),
        );
    }
    let startup_deadline = Instant::now() + Duration::from_secs(10);
    while ready.load(Ordering::Acquire) != pool.heavy_workers() {
        assert!(Instant::now() < startup_deadline, "workers did not start");
        std::thread::sleep(Duration::from_millis(1));
    }
    let begin = Instant::now();
    start.set(begin).unwrap();
    let mut samples = Vec::with_capacity(4000);
    while begin.elapsed() < Duration::from_secs(4) {
        let due = Instant::now() + Duration::from_millis(1);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
        samples.push(Instant::now().saturating_duration_since(due).as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
    println!("pool_latency mode={} logical={} heavy={} light={} duration_s=4 samples={} wake_ms_p50={:.3} wake_ms_p95={p95:.3} wake_ms_max={:.3}",
        if baseline { "baseline" } else { "gate" }, std::thread::available_parallelism().unwrap(),
        pool.heavy_workers(), pool.light_reserve(), samples.len(), samples[samples.len()/2], samples[samples.len()-1]);
    let mut shutdown = pool.shutdown(ShutdownMode::Drain);
    let shutdown_deadline = Instant::now() + Duration::from_secs(10);
    while shutdown.try_take().is_none() {
        assert!(Instant::now() < shutdown_deadline, "pool did not stop");
        std::thread::sleep(Duration::from_millis(1));
    }
    for mut job in jobs {
        assert!(job.try_take().unwrap().is_ok());
    }
    println!("{}", pool.summary());
    assert_eq!(pool.stats().priority_applied, pool.worker_count());
    assert!(
        baseline || p95 <= 2.0,
        "main-thread wake p95 {p95:.3} ms exceeds 2 ms"
    );
}
