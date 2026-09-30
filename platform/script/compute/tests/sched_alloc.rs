//! Steady state allocates nothing: a realtime stream of kernel jobs on the
//! headless pool, and synchronous runs, counted with a global allocator.

use makepad_script_compute::admission::{DeviceLimits, JobBudget, Ledger, Origin};
use makepad_script_compute::kernel::compile;
use makepad_script_compute::sched::{Executor, Job, Priority, Scheduler, SchedulerConfig, Stream, ThreadExecutor};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        System.realloc(p, l, n)
    }
}

#[global_allocator]
static A: Counting = Counting;

#[test]
fn steady_state_jobs_allocate_nothing() {
    let k = compile("let W = 256\nlet pos = output(vec3)\nlet t = param(0.0)\nfn vertex(i) { let x = float(i % W)\n let z = float(i / W)\n pos[i] = vec3(x, sin(x * 0.1 + t) * cos(z * 0.1), z) }").unwrap();
    let exec: Arc<dyn Executor> = Arc::new(ThreadExecutor::new(3));
    let ledger = Ledger::new(DeviceLimits::default());
    let s = Scheduler::new(exec.clone(), SchedulerConfig { max_running: 1, threads: 4, ledger: Some(ledger), queue_capacity: 8 });
    let n = 256 * 256;
    let mk = || {
        let mut j = Job::new(k.clone(), n);
        j.output("pos", vec![0.0; n * 3]).unwrap();
        j
    };
    let mut stream = Stream::new(mk(), mk());
    let budget = JobBudget { wall: Duration::from_secs(1) };
    let frame = |stream: &mut Stream, f: u32| {
        stream.request(&s, Priority::Near, Origin::Host, budget, |j| j.set_time(f as f32)).unwrap();
        while stream.busy() {
            stream.poll();
            std::thread::yield_now();
        }
    };
    for f in 0..20 {
        frame(&mut stream, f);
    }
    let before = ALLOCS.load(Ordering::Relaxed);
    for f in 20..120 {
        frame(&mut stream, f);
    }
    let stream_allocs = ALLOCS.load(Ordering::Relaxed) - before;
    // Locked-time runs.
    let mut job = mk();
    for _ in 0..5 {
        s.run_sync(&mut job, Origin::Host, budget).unwrap();
    }
    let before = ALLOCS.load(Ordering::Relaxed);
    for f in 0..50 {
        job.set_time(f as f32);
        s.run_sync(&mut job, Origin::Host, budget).unwrap();
    }
    let sync_allocs = ALLOCS.load(Ordering::Relaxed) - before;
    eprintln!("100 streamed jobs: {} allocations; 50 synchronous jobs: {}", stream_allocs, sync_allocs);
    assert_eq!(stream_allocs, 0);
    assert_eq!(sync_allocs, 0);
}
