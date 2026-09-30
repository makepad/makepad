//! The engine on the platform's `TaskPool` (Heavy lane): installed once,
//! jobs of every priority and a pipeline run on it, bit-identical to a run
//! on the caller alone. (Its own test binary: the engine is per process.)

use makepad_platform::Cx;
use makepad_render_kernels::compute::sched::{InlineExecutor, Job, Priority};
use makepad_render_kernels::pipeline::Count;
use makepad_render_kernels::{engine, install, Pipeline};
use std::time::Duration;

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
        let mut job = e.run(job, p, Duration::from_secs(10)).unwrap_or_else(|(_, err)| panic!("{:?}: {}", p, err));
        assert_eq!(job.take_output_u32("pos").unwrap(), want, "{:?}", p);
    }
    let mut pipe = Pipeline::new();
    pipe.pass(k, Count::Fixed(n)).set_param("amp", 3.0);
    pipe.run_on(e, Priority::Near, Duration::from_secs(10)).unwrap();
    assert_eq!(pipe.buffer("pos").unwrap(), &want[..]);
    assert!(e.scheduler().stats().completed >= 4);
    assert!(cx.task_pool_summary().contains("pool"), "{}", cx.task_pool_summary());
}
