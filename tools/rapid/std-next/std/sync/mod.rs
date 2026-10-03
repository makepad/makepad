//! std::sync: locks, once-cells, channels, plus re-exports of Arc and the atomics.

mod barrier;
pub mod mpsc;
mod mutex;
mod once;
mod poison;
pub(crate) mod raw;
mod reentrant_lock;
mod rwlock;

pub use alloc::sync::{Arc, Weak};
pub use core::sync::atomic;

pub use self::barrier::{Barrier, BarrierWaitResult};
pub use self::mutex::{Condvar, Mutex, MutexGuard, WaitTimeoutResult};
pub use self::once::{LazyLock, Once, OnceLock, OnceState};
pub use self::poison::{LockResult, PoisonError, TryLockError, TryLockResult};
pub use self::reentrant_lock::{ReentrantLock, ReentrantLockGuard};
pub use self::rwlock::{RwLock, RwLockReadGuard, RwLockWriteGuard};
