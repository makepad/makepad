//! Task dispatch over a thread pool the app binds.
//!
//! Translated from `src/engine/engine_thread.cc` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2026 DeepMind Technologies Limited);
//! changed: translated to Rust; the reference's own pool is replaced by the app's ([`TaskPool`]), and a
//! parallel phase hands each task its own part of the outputs instead of sharing one Data.

use crate::data::Data;
use crate::model::Model;
use std::sync::{Arc, Mutex};

/// A pool of threads the engine spreads independent tasks over: the app's workpool, bound to a Data with
/// [`bind_thread_pool`] (the reference's `mju_bindThreadPool`).
pub trait TaskPool: Send + Sync {
    /// Runs `f(i)` once for every `i` in `0..n` on the pool, the calling thread among its workers, and
    /// returns when all have run.
    fn fan_out(&self, n: usize, f: &(dyn Fn(usize) + Sync));
    /// How many threads run tasks at once, the caller included.
    fn threads(&self) -> usize;
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

pub fn num_thread(d: &Data) -> usize {
    d.threadpool.0.as_ref().map_or(1, |pool| pool.threads().max(1))
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

// CpuPause, ThreadPoolContext and mju_threadpool are the reference's own pool, replaced by TaskPool. The
// island solves share Data's outputs and run in order on the calling thread.
pub fn dispatch<A>(m: &Model, d: &mut Data, func: &mut impl FnMut(&Model, &mut Data, &mut A, usize, usize), arg: &mut A, ntask: usize) {
    for i in 0..ntask {
        func(m, d, arg, 0, i);
    }
}
