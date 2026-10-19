//! Fork/join sections on persistent worker threads (a drop-in for `std::thread::scope`).
//!
//! The front end forks short sections all through a cold start (demand planning, outline
//! discovery, derive pre-expansion, build scripts): ~1,100 `thread::scope` threads per cold
//! run, each with a thread start, a stack, and its own 2 MB arena chunk to commit and decommit
//! (1.6 GB committed against 192 MB live, 571k minor faults). Here a section's tasks run on
//! cached workers instead: `spawn` hands the task to an idle worker, or starts a new one when
//! none is idle, so every task starts at once exactly as it did on its own thread (tasks may
//! wait for each other, a section may nest in a task) and the number of workers is the peak
//! number of tasks in flight, which is the `jobs` setting for the sections that size themselves
//! by it. A worker keeps its thread, its arena chunks and its thread-local caches between tasks.
//!
//! The borrows stay sound the way `std::thread::scope` keeps them: `scope` does not return
//! (also not when its closure panics) before every task it spawned has finished and dropped
//! what it captured, so tasks may borrow the caller's locals.
//!
//! A worker holds no compiler-generation state while it idles, but its thread-local caches and
//! arena chunks belong to the generation it last ran in: the zone swap calls `retire_idle`
//! and waits for the workers to end before it destroys the old generation.
use crate::zone_alloc;
use relax_session::sync::{Condvar, Mutex};
use std::any::Any;
use std::marker::PhantomData;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

type Task = Box<dyn FnOnce() + Send + 'static>;

enum Slot {
    /// Waiting for a task.
    Idle,
    Task(Task),
    /// Ends the worker.
    Retire,
}

struct Worker {
    slot: Mutex<Slot>,
    wake: Condvar,
    /// Set by the spawner right after the thread starts; taken to join the worker when it retires
    /// (a native join also waits for the thread's TLS destructors, which walk the caches of the
    /// generation it last ran in).
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

thread_local! {
    /// The task that just ran on this worker panicked: the worker does not take another task, so
    /// no thread-local state of a half-run task (a flag set without a restoring guard) carries over.
    static TASK_PANICKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct Pool {
    /// Workers waiting for a task, most recently idle last. Only touched in the kept phase: the
    /// list outlives every compiler generation.
    idle: Mutex<Vec<Arc<Worker>>>,
    /// Workers that ended after a panicking task, not yet joined (`retire_idle` joins them).
    dying: Mutex<Vec<Arc<Worker>>>,
    started: AtomicUsize,
    alive: AtomicUsize,
}

static POOL: Pool = Pool { idle: Mutex::new(Vec::new()), dying: Mutex::new(Vec::new()), started: AtomicUsize::new(0), alive: AtomicUsize::new(0) };

/// (workers started, workers alive) so far.
pub fn stats() -> (usize, usize) {
    (POOL.started.load(Ordering::Relaxed), POOL.alive.load(Ordering::Relaxed))
}

impl Worker {
    fn give(&self, what: Slot) {
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = what;
        self.wake.notify_one();
    }
}

fn worker_main(w: Arc<Worker>) {
    let _z = zone_alloc::compiler_thread();
    loop {
        let task = {
            let mut s = w.slot.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                match std::mem::replace(&mut *s, Slot::Idle) {
                    Slot::Task(t) => break Some(t),
                    Slot::Retire => break None,
                    Slot::Idle => s = w.wake.wait(s).unwrap_or_else(|e| e.into_inner()),
                }
            }
        };
        let Some(task) = task else { break };
        // the task catches its own panics; a panic that escapes it (a result dropped in the task
        // itself) is handled the same way
        let escaped = std::panic::catch_unwind(AssertUnwindSafe(task)).is_err();
        if escaped || TASK_PANICKED.with(|p| p.replace(false)) {
            // end this worker: a fresh thread has none of the half-run task's thread-local state
            let _k = zone_alloc::kept();
            POOL.dying.lock().unwrap_or_else(|e| e.into_inner()).push(w.clone());
            break;
        }
        // what the worker left in its chunks is for every thread to reuse, not held for its next task
        zone_alloc::compiler_hand_back();
        let _k = zone_alloc::kept();
        POOL.idle.lock().unwrap_or_else(|e| e.into_inner()).push(w.clone());
    }
    POOL.alive.fetch_sub(1, Ordering::Relaxed);
}

fn dispatch(task: Task) -> Result<(), (Task, std::io::Error)> {
    let idle = {
        let _k = zone_alloc::kept();
        POOL.idle.lock().unwrap_or_else(|e| e.into_inner()).pop()
    };
    if let Some(w) = idle {
        w.give(Slot::Task(task));
        return Ok(());
    }
    // no idle worker: a new one, which starts with the task
    let _k = zone_alloc::kept();
    let (w, spawned) = {
        let w = Arc::new(Worker { slot: Mutex::new(Slot::Task(task)), wake: Condvar::new(), handle: Mutex::new(None) });
        let id = POOL.started.fetch_add(1, Ordering::Relaxed);
        POOL.alive.fetch_add(1, Ordering::Relaxed);
        let w2 = w.clone();
        let spawned = std::thread::Builder::new().name(format!("run-pool-{id}")).spawn(move || worker_main(w2));
        (w, spawned)
    };
    match spawned {
        Ok(h) => {
            *w.handle.lock().unwrap_or_else(|e| e.into_inner()) = Some(h);
            Ok(())
        }
        Err(e) => {
            POOL.alive.fetch_sub(1, Ordering::Relaxed);
            let task = match std::mem::replace(&mut *w.slot.lock().unwrap_or_else(|e| e.into_inner()), Slot::Idle) {
                Slot::Task(t) => t,
                _ => unreachable!("the new worker never started"),
            };
            Err((task, e))
        }
    }
}

/// Waits for a worker's thread to end completely (TLS destructors included).
fn join_worker(w: &Worker) {
    let _k = zone_alloc::kept();
    loop {
        let h = w.handle.lock().unwrap_or_else(|e| e.into_inner()).take();
        match h {
            Some(h) => {
                let _ = h.join();
                return;
            }
            // the spawner has not stored the handle yet (it does so right after the spawn)
            None => std::thread::yield_now(),
        }
    }
}

/// Ends the workers that are idle now and joins them, and the ones that ended after a panic: when
/// this returns they are gone, TLS destructors included (each hands its arena chunks back as it
/// goes). A worker that is running a task idles afterwards: call again. Returns how many ended.
pub fn retire_idle() -> usize {
    let (idle, dying) = {
        let _k = zone_alloc::kept();
        (
            std::mem::take(&mut *POOL.idle.lock().unwrap_or_else(|e| e.into_inner())),
            std::mem::take(&mut *POOL.dying.lock().unwrap_or_else(|e| e.into_inner())),
        )
    };
    let n = idle.len() + dying.len();
    for w in &idle {
        w.give(Slot::Retire);
    }
    for w in idle.iter().chain(dying.iter()) {
        join_worker(w);
    }
    let _k = zone_alloc::kept();
    drop(idle);
    drop(dying);
    n
}

struct ScopeData {
    pending: AtomicUsize,
    /// A task panicked and nobody joined it.
    panicked: AtomicBool,
    lock: Mutex<()>,
    done: Condvar,
}

impl ScopeData {
    fn finish_one(&self) {
        if self.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
            // under the lock, so the waiter is either before its check (and sees 0) or asleep
            let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
            self.done.notify_all();
        }
    }
    fn wait_all(&self) {
        let mut g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        while self.pending.load(Ordering::Acquire) != 0 {
            g = self.done.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }
}

struct Packet<T> {
    result: Mutex<Option<std::thread::Result<T>>>,
    ready: Condvar,
    scope: Arc<ScopeData>,
}

impl<T> Drop for Packet<T> {
    fn drop(&mut self) {
        // a panic nobody asked for through `join`
        if matches!(self.result.get_mut().unwrap_or_else(|e| e.into_inner()), Some(Err(_))) {
            self.scope.panicked.store(true, Ordering::Release);
        }
    }
}

/// Like `std::thread::Scope`.
pub struct Scope<'scope, 'env: 'scope> {
    data: Arc<ScopeData>,
    scope: PhantomData<&'scope mut &'scope ()>,
    env: PhantomData<&'env mut &'env ()>,
}

/// Like `std::thread::ScopedJoinHandle`.
pub struct ScopedJoinHandle<'scope, T> {
    packet: Arc<Packet<T>>,
    scope: PhantomData<&'scope ()>,
}

/// Runs `f`, whose spawned tasks run on pool workers, and returns when all of them are done.
/// Panics (after the tasks are done) if a task panicked and its handle was not joined.
pub fn scope<'env, F, T>(f: F) -> T
where
    F: for<'scope> FnOnce(&'scope Scope<'scope, 'env>) -> T,
{
    let scope = Scope {
        data: Arc::new(ScopeData {
            pending: AtomicUsize::new(0),
            panicked: AtomicBool::new(false),
            lock: Mutex::new(()),
            done: Condvar::new(),
        }),
        scope: PhantomData,
        env: PhantomData,
    };
    let result = catch_unwind(AssertUnwindSafe(|| f(&scope)));
    scope.data.wait_all();
    match result {
        Err(e) => resume_unwind(e),
        Ok(_) if scope.data.panicked.load(Ordering::Acquire) => panic!("a scoped thread panicked"),
        Ok(r) => r,
    }
}

impl<'scope, 'env> Scope<'scope, 'env> {
    /// Starts `f` on an idle worker (or a new one) and returns at once.
    pub fn spawn<F, T>(&'scope self, f: F) -> ScopedJoinHandle<'scope, T>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        let packet = Arc::new(Packet { result: Mutex::new(None), ready: Condvar::new(), scope: self.data.clone() });
        self.data.pending.fetch_add(1, Ordering::Relaxed);
        let (p, data) = (packet.clone(), self.data.clone());
        let main = move || {
            // finishes the task whatever happens below, also when dropping a result panics
            struct Finish(Arc<ScopeData>);
            impl Drop for Finish {
                fn drop(&mut self) {
                    self.0.finish_one();
                }
            }
            let _finish = Finish(data);
            let r = catch_unwind(AssertUnwindSafe(f));
            if r.is_err() {
                TASK_PANICKED.with(|t| t.set(true));
            }
            *p.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
            p.ready.notify_all();
            // everything the task borrowed or returned is released before the scope may end
            drop(p);
        };
        let boxed: Box<dyn FnOnce() + Send + 'scope> = Box::new(main);
        // SAFETY: `scope` waits for `pending` to reach zero before it returns, also on a panic of
        // its closure, and `main` decrements it only after it dropped `f`'s result handle; the
        // erased lifetime is never used past that point.
        let task: Task = unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + 'scope>, Task>(boxed) };
        if let Err((task, e)) = dispatch(task) {
            // like std::thread::scope: no thread, no task; the section must not wait for it
            drop(task);
            self.data.pending.fetch_sub(1, Ordering::Relaxed);
            panic!("failed to spawn thread: {e}");
        }
        ScopedJoinHandle { packet, scope: PhantomData }
    }
}

impl<'scope, T> ScopedJoinHandle<'scope, T> {
    /// Waits for the task; `Err` is its panic.
    pub fn join(self) -> Result<T, Box<dyn Any + Send + 'static>> {
        let mut r = self.packet.result.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(v) = r.take() {
                return v;
            }
            r = self.packet.ready.wait(r).unwrap_or_else(|e| e.into_inner());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn tasks_borrow_the_callers_locals_and_the_scope_waits() {
        let mut data = vec![1u64; 64];
        let total = AtomicU64::new(0);
        scope(|s| {
            for chunk in data.chunks_mut(8) {
                let total = &total;
                s.spawn(move || {
                    for x in chunk.iter_mut() {
                        *x += 1;
                    }
                    total.fetch_add(chunk.iter().sum::<u64>(), Ordering::Relaxed);
                });
            }
        });
        assert!(data.iter().all(|&x| x == 2));
        assert_eq!(total.load(Ordering::Relaxed), 128);
    }

    #[test]
    fn join_returns_the_value_or_the_panic() {
        let (a, b) = scope(|s| {
            let ok = s.spawn(|| 40 + 2);
            let bad = s.spawn(|| -> u32 { panic!("boom") });
            (ok.join(), bad.join())
        });
        assert_eq!(a.unwrap(), 42);
        assert!(b.is_err());
    }

    #[test]
    #[should_panic(expected = "a scoped thread panicked")]
    fn an_unjoined_panic_panics_the_scope_after_the_tasks_ended() {
        let done = AtomicBool::new(false);
        let r = std::panic::catch_unwind(AssertUnwindSafe(|| {
            scope(|s| {
                s.spawn(|| panic!("boom"));
                s.spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    done.store(true, Ordering::SeqCst);
                });
            })
        }));
        assert!(done.load(Ordering::SeqCst), "the scope waited for the slow task");
        std::panic::resume_unwind(r.unwrap_err());
    }

    #[test]
    fn a_panic_of_the_closure_still_waits_for_the_tasks() {
        let done = AtomicBool::new(false);
        let r = std::panic::catch_unwind(AssertUnwindSafe(|| {
            scope(|s| {
                s.spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    done.store(true, Ordering::SeqCst);
                });
                panic!("closure");
            })
        }));
        assert!(r.is_err());
        assert!(done.load(Ordering::SeqCst));
    }

    #[test]
    fn tasks_that_wait_for_each_other_all_run_at_once() {
        // a barrier of 6: with fewer than 6 running tasks this would never end
        let barrier = std::sync::Barrier::new(6);
        scope(|s| {
            for _ in 0..6 {
                s.spawn(|| {
                    barrier.wait();
                });
            }
        });
    }

    #[test]
    fn sections_nest_in_tasks() {
        let n = AtomicUsize::new(0);
        scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    scope(|inner| {
                        for _ in 0..4 {
                            inner.spawn(|| {
                                n.fetch_add(1, Ordering::Relaxed);
                            });
                        }
                    });
                });
            }
        });
        assert_eq!(n.load(Ordering::Relaxed), 32);
    }

    #[test]
    fn workers_are_reused_across_sections() {
        let (before, _) = stats();
        for _ in 0..500 {
            scope(|s| {
                for _ in 0..4 {
                    s.spawn(|| std::thread::yield_now());
                }
            });
        }
        let (after, _) = stats();
        // other tests run at the same time; 2000 tasks must not start 2000 threads
        assert!(after - before < 200, "started {}", after - before);
    }

    #[test]
    fn sections_from_many_threads_at_once() {
        let total = AtomicUsize::new(0);
        std::thread::scope(|ts| {
            for _ in 0..8 {
                ts.spawn(|| {
                    for _ in 0..200 {
                        scope(|s| {
                            for _ in 0..3 {
                                s.spawn(|| {
                                    total.fetch_add(1, Ordering::Relaxed);
                                });
                            }
                        });
                    }
                });
            }
        });
        assert_eq!(total.load(Ordering::Relaxed), 8 * 200 * 3);
    }

    #[test]
    fn retired_workers_end_and_new_tasks_still_run() {
        scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {});
            }
        });
        // a worker may still be on its way to idle: retire until none is left idle
        let t0 = std::time::Instant::now();
        while retire_idle() == 0 && t0.elapsed() < std::time::Duration::from_secs(2) {
            std::thread::yield_now();
        }
        let ran = AtomicUsize::new(0);
        scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    ran.fetch_add(1, Ordering::Relaxed);
                });
            }
        });
        assert_eq!(ran.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn a_worker_that_ran_a_panicking_task_ends_and_retire_joins_it() {
        thread_local! { static MARK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
        let survived = AtomicUsize::new(0);
        for _ in 0..20 {
            scope(|s| {
                let h = s.spawn(|| {
                    MARK.with(|m| m.set(true));
                    panic!("boom");
                });
                let _ = h.join();
            });
            // later tasks never see the mark of the panicked one
            scope(|s| {
                s.spawn(|| {
                    if MARK.with(|m| m.get()) {
                        survived.fetch_add(1, Ordering::Relaxed);
                    }
                });
            });
        }
        assert_eq!(survived.load(Ordering::Relaxed), 0);
        retire_idle();
    }
}
