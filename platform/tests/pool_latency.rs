//! The app pool on this machine leaves the UI thread its cores. With every Background token
//! taken by jobs that run until told to stop, the process main thread runs at UserInteractive,
//! each Background job runs at Background on a thread the OS ranks at Background, and Interactive
//! work runs while every one of them still runs, not after one ends. Counted, not timed: the main
//! thread's 1 ms sleep wakes under the load, and the most tokens the Background jobs held, are
//! printed for a person to read and never gated (a shared box decides the wakes; a woken thread
//! takes its lent token back over the limit, and its next checkpoint yields).

use std::time::{Duration, Instant};

/// Waits for `done`; the deadline guards against a hang (what it waits for comes at once or never).
fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < deadline, "{what}: not after 60 s");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn main() {
    use makepad_cpu_budget::{APP, BACKGROUND};
    use makepad_platform::{
        thread::{checkpoint, current_class, machine_topology, Class, PriorityStatus, ShutdownMode},
        Cx, CxThreadPriority,
    };
    use std::{
        hint::black_box,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc,
        },
    };

    let ui_priority = Cx::set_thread_priority(CxThreadPriority::UserInteractive);
    println!("pool_latency UI UserInteractive={ui_priority:?} topology={:?}", machine_topology());
    assert_eq!(ui_priority, PriorityStatus::Applied, "the main thread runs at UserInteractive");
    let cx = Cx::new(Box::new(|_, _| {}));
    let pool = cx.task_pool();
    let (cores, background_limit) = (makepad_cpu_budget::cores() as usize, makepad_cpu_budget::background_limit() as usize);
    if cores > 1 {
        assert!(background_limit < cores, "Background may take every core ({background_limit} of {cores}): none is left for the UI thread");
    }

    // As many Background jobs as hold tokens at once (its limit, or the workers when they are
    // fewer), and one more.
    let full = pool.worker_count().min(background_limit);
    let started = Arc::new(AtomicUsize::new(0));
    let ended = Arc::new(AtomicUsize::new(0));
    let misclassed = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let mut burners = Vec::new();
    for seed in 0..=full {
        let (started, ended, misclassed, stop) = (started.clone(), ended.clone(), misclassed.clone(), stop.clone());
        burners.push(
            pool.submit(Class::Background, move || {
                started.fetch_add(1, Ordering::AcqRel);
                if current_class() != Some(Class::Background) || makepad_cpu_budget::thread_class() != Some(BACKGROUND) {
                    misclassed.fetch_add(1, Ordering::Relaxed);
                }
                let mut value = seed as u64 + 1;
                while !stop.load(Ordering::Acquire) {
                    checkpoint().unwrap();
                    for _ in 0..4096 {
                        value = black_box(value.wrapping_mul(6364136223846793005).wrapping_add(1));
                    }
                }
                ended.fetch_add(1, Ordering::AcqRel);
                black_box(value)
            })
            .unwrap(),
        );
    }
    until("the Background jobs that fit start", || started.load(Ordering::Acquire) >= full);

    // The main thread's wakes under the full load, and between them the tokens the Background
    // jobs hold: printed, not gated.
    let background_tokens = || makepad_cpu_budget::tokens().holders.iter().filter(|h| h.class == BACKGROUND && h.client == APP && !h.lent && !h.above).count();
    let mut most_tokens = 0;
    let mut wakes = Vec::with_capacity(500);
    for _ in 0..500 {
        let due = Instant::now() + Duration::from_millis(1);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
        wakes.push(Instant::now().saturating_duration_since(due).as_secs_f64() * 1000.0);
        most_tokens = most_tokens.max(background_tokens());
    }
    wakes.sort_by(f64::total_cmp);

    // Interactive work beside the load: the Background jobs end only once these are done, so each
    // runs while all of them still run, at its class, and the main thread takes each result once.
    let mut picks: Vec<_> = (0..4).map(|_| pool.submit(Class::Interactive, current_class).unwrap()).collect();
    let mut classes = Vec::new();
    until("the Interactive jobs run beside a full Background load", || {
        picks.retain_mut(|pick| match pick.try_take() {
            Some(result) => {
                classes.push(result.unwrap());
                false
            }
            None => true,
        });
        picks.is_empty()
    });
    assert_eq!(classes, [Some(Class::Interactive); 4], "each Interactive job runs at Interactive, its result taken once");

    stop.store(true, Ordering::Release);
    let mut shutdown = pool.shutdown(ShutdownMode::Drain);
    until("the pool drains and stops", || shutdown.try_take().is_some());
    for mut job in burners {
        assert!(job.try_take().unwrap().is_ok(), "a Background job ends without an error");
    }
    assert_eq!(ended.load(Ordering::Acquire), full + 1, "every Background job runs to its end");
    assert_eq!(misclassed.load(Ordering::Acquire), 0, "a Background job runs at Background, on a thread at Background");
    println!(
        "pool_latency logical={} cores={cores} background_limit={background_limit} workers={} background={} tokens_max={most_tokens} wakes={} wake_ms_p50={:.3} wake_ms_p95={:.3} wake_ms_max={:.3}",
        std::thread::available_parallelism().unwrap(),
        pool.worker_count(),
        full + 1,
        wakes.len(),
        wakes[wakes.len() / 2],
        wakes[wakes.len() * 95 / 100],
        wakes[wakes.len() - 1]
    );
    println!("{}", pool.summary());
}
