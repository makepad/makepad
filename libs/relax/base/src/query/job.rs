use super::{Cycle, DepNodeIndex, QueryEngine, QueryError, State};
use std::sync::Arc;
use relax_session::sync::{Condvar, Mutex};

/// Only live jobs occupy storage; a latch is allocated only on contention.
/// rustc_middle/src/query/job.rs:17,35 uses the same lazy latch ownership.
#[derive(Default)]
pub(super) struct QueryJob {
    pub active: bool,
    /// the thread running it (WaitHook::me; 0 without a hook)
    pub runner: u64,
    pub waiting_on: Option<DepNodeIndex>,
    pub latch: Option<Arc<QueryLatch>>,
}

#[derive(Clone)]
pub(super) enum Completion {
    Value(Result<(), QueryError>),
    Retry,
}

/// How a waiting thread waits, when it is the JIT's owner thread (set once by the JIT): the owner
/// never blocks on another thread; it serves requests while it waits (`pump`), and the latch's
/// completion wakes it (`wake`).
pub struct WaitHook {
    pub on_owner: fn() -> bool,
    /// waits for `done` serving requests; the first argument is the thread running the query
    pub pump: fn(u64, &dyn Fn() -> bool),
    pub wake: fn(),
    /// this thread's id (the runner recorded with an active query)
    pub me: fn() -> u64,
}

/// This thread's id as the wait hook names threads (0: no hook).
pub(super) fn runner_id() -> u64 {
    WAIT_HOOK.get().map_or(0, |h| (h.me)())
}

pub(super) static WAIT_HOOK: std::sync::OnceLock<WaitHook> = std::sync::OnceLock::new();

pub fn set_wait_hook(hook: WaitHook) {
    let _ = WAIT_HOOK.set(hook);
}

pub(super) struct QueryLatch {
    result: Mutex<Option<Completion>>,
    ready: Condvar,
}

impl QueryLatch {
    pub fn new() -> Self {
        Self { result: Mutex::new(None), ready: Condvar::new() }
    }

    pub fn wait(&self, runner: u64) -> Completion {
        let mut result = self.result.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(hook) = WAIT_HOOK.get().filter(|h| result.is_none() && (h.on_owner)()) {
            drop(result);
            (hook.pump)(runner, &|| self.result.lock().unwrap_or_else(|e| e.into_inner()).is_some());
            return self.result.lock().unwrap_or_else(|e| e.into_inner()).as_ref().unwrap().clone();
        }
        let _w = (result.is_none()).then(|| crate::waits::begin(crate::waits::Kind::Query, 0, runner, "query"));
        while result.is_none() {
            drop(result);
            crate::preempt::point();
            result = self.result.lock().unwrap_or_else(|e| e.into_inner());
            if result.is_none() {
                result = self.ready.wait_timeout(result, std::time::Duration::from_millis(5)).unwrap_or_else(|e| e.into_inner()).0;
            }
        }
        result.as_ref().unwrap().clone()
    }

    pub fn complete(&self, result: Completion) {
        *self.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        self.ready.notify_all();
        if let Some(hook) = WAIT_HOOK.get() {
            (hook.wake)();
        }
    }
}

/// Both synchronous children and condvar waits are edges, as in
/// rustc_query_impl/src/job.rs:119,131,159. Insertion and cycle checking share a lock.
/// Since each running job can await only one other job, the walk is a chain.
pub(super) fn check_cycle(state: &State, parent: DepNodeIndex, target: DepNodeIndex) -> Result<(), QueryError> {
    let mut next = Some(target);
    while let Some(index) = next {
        if index == parent {
            let mut stack = vec![state.graph.query(parent).unwrap()];
            let mut index = target;
            while index != parent {
                stack.push(state.graph.query(index).unwrap());
                index = state.jobs[&index].waiting_on.unwrap();
            }
            stack.push(state.graph.query(parent).unwrap());
            return Err(QueryError::Cycle(Cycle { stack }));
        }
        next = state.jobs.get(&index).and_then(|job| job.waiting_on);
    }
    Ok(())
}

pub(super) struct WaitGuard<'a> {
    pub engine: &'a QueryEngine,
    pub parent: Option<DepNodeIndex>,
}

impl Drop for WaitGuard<'_> {
    fn drop(&mut self) {
        if let Some(parent) = self.parent {
            // a drop during unwinding must not panic (the job map is sparse now)
            if let Some(job) = self.engine.state.lock().unwrap_or_else(|e| e.into_inner()).jobs.get_mut(&parent) { job.waiting_on = None; }
        }
    }
}

/// Unwinding poisons the node and wakes every waiter before the panic escapes.
/// rustc_query_impl/src/execution.rs:154,190,220 (ActiveJobGuard).
pub(super) struct ActiveJobGuard<'a> {
    pub engine: &'a QueryEngine,
    pub index: DepNodeIndex,
    pub finished: bool,
}

impl ActiveJobGuard<'_> {
    // Lead design round 42 extends rustc_query_impl/src/execution.rs:108-159,
    // 179-257 @8d1a764: cancellation removes the active job without publishing
    // poison or a value. Waiters retry; ordinary panic still poisons in Drop.
    pub fn cancel(&mut self) {
        let latch = {
            let mut state = self.engine.state.lock().unwrap_or_else(|e| e.into_inner());
            state.jobs.remove(&self.index).unwrap().latch
        };
        self.finished = true;
        if let Some(latch) = latch { latch.complete(Completion::Retry); }
    }

    pub fn complete(&mut self, result: Result<(), QueryError>) {
        let latch = {
            let mut state = self.engine.state.lock().unwrap_or_else(|e| e.into_inner());
            match &result {
                Err(error) => { state.graph.errors.insert(self.index, (self.engine.revision, error.clone())); }
                Ok(()) => { state.graph.errors.remove(&self.index); }
            }
            state.jobs.remove(&self.index).unwrap().latch
        };
        self.finished = true;
        if let Some(latch) = latch {
            latch.complete(Completion::Value(result));
        }
    }
}

impl Drop for ActiveJobGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let query = self.engine.state.lock().unwrap_or_else(|e| e.into_inner()).graph.query(self.index).unwrap();
            self.complete(Err(QueryError::Poisoned(query)));
        }
    }
}
