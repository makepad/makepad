// Default object lifetime bounds inside Fn-sugar arguments of an impl's where-clause
// (`F: Fn(usize, &(dyn Fn(usize) + Sync))`): the elided `&` is a fresh late-bound region and
// the trait object defaults to it (rustc resolve_bound_vars / object_lifetime_default).
// The impl's bounds are checked from another body when the executor is built and coerced
// to `Arc<dyn Executor>` (script compute sched.rs FnExecutor, render_kernels engine.rs).
use std::sync::Arc;

pub trait Executor: Send + Sync {
    fn workers(&self) -> usize;
    fn spawn(&self, task: Arc<dyn Fn() + Send + Sync>) -> bool;
    fn fan_out(&self, n: usize, f: &(dyn Fn(usize) + Sync));
}

pub struct FnExecutor<S, F> {
    workers: usize,
    spawn: S,
    fan_out: F,
}

impl<S, F> FnExecutor<S, F>
where
    S: Fn(Arc<dyn Fn() + Send + Sync>) -> bool + Send + Sync,
    F: Fn(usize, &(dyn Fn(usize) + Sync)) + Send + Sync,
{
    pub fn new(workers: usize, spawn: S, fan_out: F) -> Self {
        FnExecutor { workers, spawn, fan_out }
    }
}

impl<S, F> Executor for FnExecutor<S, F>
where
    S: Fn(Arc<dyn Fn() + Send + Sync>) -> bool + Send + Sync,
    F: Fn(usize, &(dyn Fn(usize) + Sync)) + Send + Sync,
{
    fn workers(&self) -> usize {
        self.workers
    }
    fn spawn(&self, task: Arc<dyn Fn() + Send + Sync>) -> bool {
        (self.spawn)(task)
    }
    fn fan_out(&self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        (self.fan_out)(n, f)
    }
}

fn make_executor(workers: usize) -> Arc<dyn Executor> {
    Arc::new(FnExecutor::new(
        workers,
        move |task: Arc<dyn Fn() + Send + Sync>| {
            task();
            true
        },
        move |n: usize, f: &(dyn Fn(usize) + Sync)| (0..n).for_each(|w| f(w)),
    ))
}

#[test]
fn object_lifetime_default_impl_where_fn_sugar() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let e = make_executor(3);
    assert_eq!(e.workers(), 3);
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    assert!(e.spawn(Arc::new(move || {
        h.fetch_add(1, Ordering::Relaxed);
    })));
    let sum = AtomicUsize::new(0);
    e.fan_out(4, &|i| {
        sum.fetch_add(i, Ordering::Relaxed);
    });
    assert_eq!(hits.load(Ordering::Relaxed), 1);
    assert_eq!(sum.load(Ordering::Relaxed), 6);
}
