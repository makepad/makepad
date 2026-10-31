//! Barrier.

use core::fmt;

use super::{Condvar, Mutex};

pub struct Barrier {
    lock: Mutex<BarrierState>,
    cvar: Condvar,
    num_threads: usize,
}

struct BarrierState {
    count: usize,
    generation_id: usize,
}

pub struct BarrierWaitResult(bool);

impl Barrier {
    pub const fn new(n: usize) -> Barrier {
        Barrier { lock: Mutex::new(BarrierState { count: 0, generation_id: 0 }), cvar: Condvar::new(), num_threads: n }
    }

    pub fn wait(&self) -> BarrierWaitResult {
        let mut lock = self.lock.lock().unwrap();
        let local_gen = lock.generation_id;
        lock.count += 1;
        if lock.count < self.num_threads {
            while local_gen == lock.generation_id {
                lock = self.cvar.wait(lock).unwrap();
            }
            BarrierWaitResult(false)
        } else {
            lock.count = 0;
            lock.generation_id = lock.generation_id.wrapping_add(1);
            self.cvar.notify_all();
            BarrierWaitResult(true)
        }
    }
}

impl BarrierWaitResult {
    pub fn is_leader(&self) -> bool {
        self.0
    }
}

impl fmt::Debug for Barrier {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Barrier").finish_non_exhaustive()
    }
}

impl fmt::Debug for BarrierWaitResult {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("BarrierWaitResult").field("is_leader", &self.is_leader()).finish()
    }
}
