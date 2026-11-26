//! The engine on the platform's `TaskPool`: installed once, jobs of every
//! priority and a pipeline run on it, bit-identical to a run on the caller
//! alone; a Visible job's Near and Far runs never wait on Background work,
//! and the tasks the engine starts take their starter's class. (Its own
//! test binary: the engine is per process.)

use makepad_platform::thread::{current_class, Class};
use makepad_platform::Cx;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use makepad_render_kernels::dispatch::sched::{InlineExecutor, Job, Priority};
use makepad_render_kernels::pipeline::Count;
use makepad_render_kernels::{engine, install, Pipeline};

const SRC: &str = "let pos = output(vec3)\nlet amp = param(1)\nfn vertex(i) { let x = float(i % 512) * 0.1\n let z = float(i / 512) * 0.1\n pos[i] = vec3(x, fbm2(vec2(x, z), 4, 2.0, 0.5) * amp, z) }";

#[test]
fn kernel_jobs_run_on_the_platform_task_pool() {
    let cx = Cx::new(Box::new(|_, _| {}));
    assert!(install(&cx), "the first install wins");
    assert!(!install(&cx), "and only the first");
    let e = engine();
    assert!(e.on_task_pool());
    let k = makepad_render_kernels::compile(SRC, &[], &[]).unwrap();
    let n = 512 * 300;
    let mut reference = Job::new(k.clone(), n);
    reference.output_u32("pos", vec![0; n * 3]).unwrap();
    reference.set_param("amp", 3.0);
    reference.run(&InlineExecutor, 1).unwrap();
    let want = reference.take_output_u32("pos").unwrap();
    for p in [Priority::MustComplete, Priority::Near, Priority::Far, Priority::Cosmetic] {
        let mut job = Job::new(k.clone(), n);
        job.output_u32("pos", vec![0; n * 3]).unwrap();
        job.set_param("amp", 3.0);
        let mut job = e.run(job, p).unwrap_or_else(|(_, err)| panic!("{:?}: {}", p, err));
        assert_eq!(job.take_output_u32("pos").unwrap(), want, "{:?}", p);
    }
    let mut pipe = Pipeline::new();
    pipe.pass(k.clone(), Count::Fixed(n)).set_param("amp", 3.0);
    pipe.run_on(e, Priority::Near).unwrap();
    assert_eq!(pipe.buffer("pos").unwrap(), &want[..]);
    assert!(e.scheduler().stats().completed >= 4);
    assert!(cx.task_pool_summary().contains("pool"), "{}", cx.task_pool_summary());

    // Every general worker holds a parked Background job, with more queued
    // behind them. A Visible job's Near and Far runs still finish: they run
    // on their caller at its class, never on a Background driver queued
    // behind that work.
    let pool = cx.task_pool();
    let (open, gate) = channel::<()>();
    let gate = Arc::new(Mutex::new(gate));
    for _ in 0..pool.worker_count() * 2 + 4 {
        let gate = gate.clone();
        pool.submit(Class::Background, move || { let _ = gate.lock().unwrap().recv(); }).unwrap().detach();
    }
    let exec = makepad_render_kernels::engine::task_pool_executor(pool.clone(), std::thread::current().id());
    let (done, results) = channel();
    let (started, classes) = channel();
    let (caller_pool, caller_exec, kernel, started_here) = (pool.clone(), exec.clone(), k.clone(), started.clone());
    std::thread::spawn(move || caller_pool.fan_out_as(Class::Visible, 1, 1, |_| {
        for p in [Priority::Near, Priority::Far] {
            let mut job = Job::new(kernel.clone(), n);
            job.output_u32("pos", vec![0; n * 3]).unwrap();
            job.set_param("amp", 3.0);
            let out = engine().run(job, p).ok().and_then(|mut job| job.take_output_u32("pos"));
            done.send((p, current_class(), out)).unwrap();
        }
        // a task the engine's executor starts from here runs at Visible
        let started = started_here.clone();
        assert!(caller_exec.spawn(Arc::new(move || { let _ = started.send(("a Visible job", current_class())); })));
    }));
    for _ in 0..2 {
        let (p, class, out) = results.recv_timeout(Duration::from_secs(60))
            .expect("a Visible job's Near and Far runs finish while Background work holds every worker");
        assert_eq!(class, Some(Class::Visible), "{p:?} runs in its caller's Visible job");
        assert_eq!(out.as_deref(), Some(&want[..]), "{p:?} on the caller is bit-identical");
    }
    // and one started outside any job runs at Background
    assert!(exec.spawn(Arc::new(move || { let _ = started.send(("outside a job", current_class())); })));
    drop(open);
    let mut seen: Vec<_> = (0..2).map(|_| classes.recv_timeout(Duration::from_secs(60)).expect("both started tasks run")).collect();
    seen.sort();
    assert_eq!(seen, vec![("a Visible job", Some(Class::Visible)), ("outside a job", Some(Class::Background))],
        "a task the executor starts takes the class of the job that starts it");
}
