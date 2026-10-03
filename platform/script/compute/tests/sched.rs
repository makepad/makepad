//! The job scheduler: results independent of scheduling, priorities and
//! pre-emption, cancellation, counted work limits, keep-last-result streams, and
//! locked-time (synchronous) runs.

use makepad_script_compute::admission::{DeviceLimits, JobBudget, Ledger, Origin};
use makepad_script_compute::kernel::{compile, compile_with, Kernel, KernelError};
use makepad_script_compute::sched::{Executor, FnExecutor, InlineExecutor, Job, JobError, Priority, Scheduler, SchedulerConfig, Stream, ThreadExecutor};
use makepad_script_compute::Backend;
use std::sync::Arc;
use std::time::{Duration, Instant};

const FIELD: &str = "let W = 512\nlet pos = output(vec3)\nlet amp = param(4.0)\nfn vertex(i) { let x = float(i % W)\n let z = float(i / W)\n pos[i] = vec3(x, fbm2(vec2(x, z) * 0.03, 4, 2.0, 0.5) * amp, z) }";
/// About 20 µs per element natively.
const SLOW: &str = "let o = output(f32)\nfn element(i) { let s = float(i)\n for a in 0..1000 { s = s * 0.999 + sin(s) }\n o[i] = s }";

fn budget(ops: u64) -> JobBudget {
    JobBudget { work: ops }
}

const ANY: JobBudget = JobBudget::UNLIMITED;

fn sched(exec: Arc<dyn Executor>, max_running: usize) -> Scheduler {
    let ledger = Ledger::new(DeviceLimits { max_in_flight: u64::MAX, ..Default::default() });
    Scheduler::new(exec, SchedulerConfig { max_running, threads: 8, ledger: Some(ledger), queue_capacity: 64 })
}

fn field_job(k: &Arc<Kernel>, n: usize) -> Job {
    let mut j = Job::new(k.clone(), n);
    j.output("pos", vec![0.0; n * 3]).unwrap();
    j
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

#[test]
fn results_do_not_depend_on_scheduling() {
    let k = compile(FIELD).unwrap();
    let n = 512 * 67 + 3;
    // Reference: one thread, scalar, interpreter.
    let mut reference = field_job(&k, n);
    reference.set_param("amp", 2.5);
    reference.run(&InlineExecutor, 1).unwrap();
    let want = bits(reference.out("pos").unwrap());
    let ki = compile_with(FIELD, &[], Backend::Interp).unwrap();
    let mut interp = field_job(&ki, n);
    interp.set_param("amp", 2.5);
    interp.run(&InlineExecutor, 1).unwrap();
    assert_eq!(bits(interp.out("pos").unwrap()), want);
    let exec: Arc<dyn Executor> = Arc::new(ThreadExecutor::new(7));
    let s = sched(exec.clone(), 2);
    for threads in [1, 3, 8] {
        let mut j = field_job(&k, n);
        j.set_param("amp", 2.5);
        j.run(&*exec, threads).unwrap();
        assert_eq!(bits(j.out("pos").unwrap()), want, "{} threads", threads);
    }
    // Async, several at once, and sync.
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let mut j = field_job(&k, n);
            j.set_param("amp", 2.5);
            s.submit(j, Priority::Near, Origin::Host, ANY).map_err(|(_, e)| e).unwrap()
        })
        .collect();
    for h in handles {
        let j = h.wait().map_err(|(_, e)| e).unwrap();
        assert_eq!(bits(j.out("pos").unwrap()), want);
    }
    let mut j = field_job(&k, n);
    j.set_param("amp", 2.5);
    s.run_sync(&mut j, Origin::Host, ANY).unwrap();
    assert_eq!(bits(j.out("pos").unwrap()), want);
    // A host pool through closures (the TaskPool wiring) gives the same.
    let pool = Arc::new(ThreadExecutor::new(3));
    let (p1, p2) = (pool.clone(), pool.clone());
    let fe: Arc<dyn Executor> = Arc::new(FnExecutor::new(3, move |t| p1.spawn(t), move |n, f| p2.fan_out(n, f)));
    let s2 = sched(fe, 1);
    let mut j = field_job(&k, n);
    j.set_param("amp", 2.5);
    let j = s2.submit(j, Priority::Far, Origin::Host, ANY).map_err(|(_, e)| e).unwrap().wait().map_err(|(_, e)| e).unwrap();
    assert_eq!(bits(j.out("pos").unwrap()), want);
    assert_eq!(s.stats().completed, 5);
}

#[test]
fn near_jobs_preempt_far_ones_which_resume_and_finish_identically() {
    let slow = compile(SLOW).unwrap();
    let fast = compile(FIELD).unwrap();
    let exec: Arc<dyn Executor> = Arc::new(ThreadExecutor::new(3));
    let s = sched(exec, 1);
    // ~20 k elements x 20 µs = 0.4 s of work: long enough to be caught running.
    let n = 20_000;
    let mut far = Job::new(slow.clone(), n);
    far.output("o", vec![0.0; n]).unwrap();
    let far = s.submit(far, Priority::Far, Origin::Host, ANY).map_err(|(_, e)| e).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let t0 = Instant::now();
    let near = s.submit(field_job(&fast, 4096), Priority::Near, Origin::Host, ANY).map_err(|(_, e)| e).unwrap();
    let near = near.wait().map_err(|(_, e)| e).unwrap();
    let near_ms = t0.elapsed().as_secs_f64() * 1e3;
    assert!(!far.is_done(), "the far job was re-queued, not dropped");
    let far = far.wait().map_err(|(_, e)| e).unwrap();
    assert!(near.out("pos").unwrap().iter().any(|x| *x != 0.0));
    let mut reference = Job::new(slow, n);
    reference.output("o", vec![0.0; n]).unwrap();
    reference.run(&InlineExecutor, 1).unwrap();
    assert_eq!(bits(far.out("o").unwrap()), bits(reference.out("o").unwrap()));
    let st = s.stats();
    eprintln!("near job done {:.2} ms after it was submitted behind a running far job; {:?}", near_ms, st);
    assert_eq!(st.preempted, 1);
    assert!(near_ms < 100.0, "the near job waited {:.1} ms", near_ms);
}

#[test]
fn cancel_and_the_work_limit_stop_running_jobs() {
    let slow = compile(SLOW).unwrap();
    let exec: Arc<dyn Executor> = Arc::new(ThreadExecutor::new(3));
    let s = sched(exec, 2);
    let n = 400_000;
    let mut j = Job::new(slow.clone(), n);
    j.output("o", vec![0.0; n]).unwrap();
    let h = s.submit(j, Priority::Near, Origin::Host, ANY).map_err(|(_, e)| e).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    let t0 = Instant::now();
    h.cancel();
    let (_, e) = h.wait().err().expect("stopped");
    assert_eq!(e, JobError::Cancelled);
    assert!(t0.elapsed() < Duration::from_millis(25), "cancel took {:?}", t0.elapsed());
    // The work limit: what one element counts, times 1000, stops the job
    // after its thousandth element, on any machine and thread count.
    let mut one = Job::new(slow.clone(), 1);
    one.output("o", vec![0.0; 1]).unwrap();
    one.run(&InlineExecutor, 1).unwrap();
    let per = one.stats().work;
    for threads in [1, 3] {
        let mut j = Job::new(slow.clone(), 4096);
        j.output("o", vec![0.0; 4096]).unwrap();
        let s1 = Scheduler::new(Arc::new(ThreadExecutor::new(threads)), SchedulerConfig { max_running: 1, threads, ledger: Some(Ledger::new(DeviceLimits::default())), queue_capacity: 4 });
        let (j, e) = s1.submit(j, Priority::Near, Origin::Host, budget(per * 1000)).map_err(|(_, e)| e).unwrap().wait().err().expect("stopped by its count");
        assert!(matches!(e, JobError::Kernel(KernelError::OverBudget { limit, .. }) if limit == per * 1000), "{e:?}");
        assert_eq!(s1.stats().over_budget, 1);
        // The same job with the limit it needs runs to the end.
        let j = s1.submit(j, Priority::Near, Origin::Host, budget(per * 4096)).map_err(|(_, e)| e).unwrap().wait().map_err(|(_, e)| e).unwrap();
        assert_eq!(j.stats().work, per * 4096);
    }
    assert_eq!(s.stats().cancelled, 1);
}

#[test]
fn a_stream_keeps_the_last_result_and_counts_dropped_frames() {
    let k = compile("let o = output(f32)\nlet t0 = param(0.0)\nfn element(i) { o[i] = t0 + float(i) }").unwrap();
    let exec: Arc<dyn Executor> = Arc::new(ThreadExecutor::new(2));
    let s = sched(exec, 1);
    let n = 10_000;
    let mk = || {
        let mut j = Job::new(k.clone(), n);
        j.output("o", vec![0.0; n]).unwrap();
        j
    };
    let mut stream = Stream::new(mk(), mk());
    assert!(stream.latest().is_none());
    let mut frames = 0;
    for f in 0..200 {
        let _ = stream.request(&s, Priority::Near, Origin::Host, ANY, |j| {
            j.set_param("t0", f as f32);
        });
        if let Some(j) = stream.latest() {
            // Whatever frame it is, it is a whole frame.
            let o = j.out("o").unwrap();
            assert_eq!(o[1] - o[0], 1.0);
            frames += 1;
        }
        // A frame's worth of other work.
        std::thread::sleep(Duration::from_micros(500));
    }
    while stream.busy() {
        stream.poll();
        std::thread::yield_now();
    }
    let last = stream.latest().unwrap().out("o").unwrap()[0];
    assert!(last >= 0.0 && last < 200.0);
    eprintln!("200 requests: {} dropped, {} frames read", stream.dropped, frames);
    assert!(frames > 100 && stream.dropped < 100, "{} frames, {} dropped", frames, stream.dropped);
}

#[test]
fn untrusted_jobs_get_the_host_call_limit_from_admission() {
    // A run-time-sized triangulation: an untrusted job's per-call limit
    // refuses a large polygon before it runs; a small one fits.
    let k = compile("let pts = input(f32)\nlet tris = output(i32, 1, 0, tris)\nlet o = output(i32)\nlet words = param(8.0)\nfn element(i) { o[i] = poly.triangulate(pts, 0, int(words), tris, 0, 3 * int(words)) }").unwrap();
    let s = sched(Arc::new(ThreadExecutor::new(1)), 1);
    let square: Vec<f32> = vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
    let big: Vec<f32> = (0..40_000).map(|i| { let a = i as f32 * 0.000314; if i % 2 == 0 { a.cos() * 100.0 } else { a.sin() * 100.0 } }).collect();
    for (pts, words, fits) in [(square, 8.0, true), (big, 40_000.0, false)] {
        let mut j = Job::new(k.clone(), 1);
        j.set_param("words", words);
        j.input("pts", pts.into()).unwrap();
        j.output_u32("tris", vec![0; 3 * words as usize]).unwrap();
        j.output_u32("o", vec![0; 1]).unwrap();
        let j = s.submit(j, Priority::Near, Origin::Ai, ANY).map_err(|(_, e)| e).unwrap().wait().map_err(|(_, e)| e).unwrap();
        assert_eq!(!j.stats().host_error, fits, "{} words", words);
        if fits {
            assert_eq!(j.out_u32("o").unwrap(), &[2]);
        }
    }
}
