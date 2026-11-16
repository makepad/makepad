//! The takes waiting for the hub worker. Connection threads push (a brief
//! lock); the worker thread, and only it, waits for the next one.

#![allow(clippy::disallowed_types)]

use crate::store::TakeId;
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};

#[derive(Default)]
pub struct Queue {
    takes: Mutex<VecDeque<TakeId>>,
    wake: Condvar,
}

impl Queue {
    pub fn push(&self, take: TakeId) {
        self.takes.lock().unwrap_or_else(|p| p.into_inner()).push_back(take);
        self.wake.notify_one();
    }

    /// Back to the front: a take popped that has to wait after all.
    pub fn push_front(&self, take: TakeId) {
        self.takes.lock().unwrap_or_else(|p| p.into_inner()).push_front(take);
        self.wake.notify_one();
    }

    /// The next take, waiting for one. The hub worker's thread only.
    pub fn pop_wait(&self) -> TakeId {
        let mut takes = self.takes.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if let Some(take) = takes.pop_front() {
                return take;
            }
            takes = self.wake.wait(takes).unwrap_or_else(|p| p.into_inner());
        }
    }

    /// The next take if one waits.
    pub fn pop(&self) -> Option<TakeId> {
        self.takes.lock().unwrap_or_else(|p| p.into_inner()).pop_front()
    }

    pub fn len(&self) -> usize {
        self.takes.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
