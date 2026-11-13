//! Task dispatch over a thread pool the app binds.
//!
//! Translated from `src/engine/engine_thread.cc` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2026 DeepMind Technologies Limited);
//! changed: translated to Rust; the reference's own pool is replaced by the app's ([`TaskPool`]), and a
//! parallel phase hands each task its own part of the outputs instead of sharing one Data.

use crate::data::Data;
use std::sync::{Arc, Mutex};

/// A pool of threads the engine spreads independent tasks over: the app's workpool, bound to a Data with
/// [`bind_thread_pool`] (the reference's `mju_bindThreadPool`).
pub trait TaskPool: Send + Sync {
    /// Runs `f(i)` once for every `i` in `0..n` on the pool, the calling thread among its workers, and
    /// returns when all have run.
    fn fan_out(&self, n: usize, f: &(dyn Fn(usize) + Sync));
    /// How many threads run tasks at once, the caller included.
    fn threads(&self) -> usize;
    /// What one fan-out costs its caller beyond the tasks' own work, in nanoseconds. The default is the
    /// workpool the apps bind, measured on a 16-thread Linux node: one humanoid's collision took 2.7 us on
    /// the caller and 10.9 us with its midphase and narrowphase each fanned out, so about 4 us a fan-out
    /// (the same at 4 and 16 threads). [`part_count`] keeps every part above twice this.
    fn fan_out_ns(&self) -> usize {
        4000
    }
}

/// The pool bound to a Data, if any. Copies of the Data share it.
#[derive(Clone, Default)]
pub struct ThreadPool(Option<Arc<dyn TaskPool>>);

impl std::fmt::Debug for ThreadPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some(pool) => write!(f, "ThreadPool({} threads)", pool.threads()),
            None => write!(f, "ThreadPool(none)"),
        }
    }
}

impl PartialEq for ThreadPool {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

/// Binds `pool` to `d`, or unbinds with None: its parallel phases run on the pool from the next step.
/// Results are the same with any pool or none.
pub fn bind_thread_pool(d: &mut Data, pool: Option<Arc<dyn TaskPool>>) {
    d.threadpool = ThreadPool(pool);
}

// C's `d->threadpool` test: is a pool bound?
pub(crate) fn has_pool(d: &Data) -> bool {
    d.threadpool.0.is_some()
}

pub fn num_thread(d: &Data) -> usize {
    d.threadpool.0.as_ref().map_or(1, |pool| pool.threads().max(1))
}

/// How many parts to cut `n` items into when they hold about `work_ns` nanoseconds of serial work: five a
/// thread, as the reference chunks, but each with at least twice the work a fan-out costs on the bound pool,
/// so a phase that cannot pay for one is a single part and runs on the caller. Nothing is refused or capped:
/// a small phase just stays serial.
pub(crate) fn part_count(d: &Data, n: usize, work_ns: usize) -> usize {
    match &d.threadpool.0 {
        Some(pool) if pool.threads() > 1 && n > 1 => {
            let floor = 2 * pool.fan_out_ns();
            let by_work = if floor == 0 { n } else { work_ns / floor };
            by_work.clamp(1, (5 * pool.threads()).min(n))
        }
        _ => 1,
    }
}

/// Runs `f(i, part)` once for each part: on the pool bound to `pool` when there is one and more than one
/// part, else in index order on the calling thread. Each part holds the outputs its task alone writes, so
/// the results do not depend on how many threads ran them.
///
/// On the pool each task takes the next part from the iterator under a lock held for that alone, never
/// across `f`: a part is handed out once whatever indexes the pool calls, and nothing is allocated. After
/// the fan-out every part has run, or this panics: a skipped part is a wrong step.
pub fn run_parts<P: Send>(pool: &ThreadPool, parts: impl ExactSizeIterator<Item = P> + Send, f: impl Fn(usize, P) + Sync) {
    match &pool.0 {
        Some(pool) if parts.len() > 1 => {
            let n = parts.len();
            let parts = Mutex::new(parts.enumerate());
            pool.fan_out(n, &|_| {
                let next = parts.lock().unwrap_or_else(|e| e.into_inner()).next();
                if let Some((i, part)) = next {
                    f(i, part);
                }
            });
            let mut rest = parts.into_inner().unwrap_or_else(|e| e.into_inner());
            assert!(rest.next().is_none(), "the pool ran fewer tasks than the {n} parts it was given");
        }
        _ => {
            for (i, part) in parts.enumerate() {
                f(i, part);
            }
        }
    }
}

// CpuPause, ThreadPoolContext and mju_threadpool are the reference's own pool, replaced by TaskPool; its
// per-thread stack shards, threadlock and thread ids are not needed: a task owns its part and its scratch.
