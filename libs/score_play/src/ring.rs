//! Bounded single-producer/single-consumer queue for realtime boundaries.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// A fixed-capacity lock-free SPSC ring.
///
/// Exactly one thread may call [`push`](Self::push), and exactly one other thread may
/// call [`pop`](Self::pop) or [`peek`](Self::peek). Capacity is exactly `N`; overflow
/// rejects the new value and increments [`overflow_count`](Self::overflow_count).
pub struct SpscRing<T: Copy, const N: usize> {
    slots: [UnsafeCell<MaybeUninit<T>>; N],
    read: AtomicUsize,
    write: AtomicUsize,
    overflow: AtomicU64,
}

// SAFETY: the SPSC contract gives each slot one writer and one reader; acquire/release
// publication prevents the reader observing a slot before its write is complete.
unsafe impl<T: Copy + Send, const N: usize> Sync for SpscRing<T, N> {}
// SAFETY: all contained values and synchronization primitives are Send.
unsafe impl<T: Copy + Send, const N: usize> Send for SpscRing<T, N> {}

impl<T: Copy, const N: usize> Default for SpscRing<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy, const N: usize> SpscRing<T, N> {
    pub const fn new() -> Self {
        Self {
            slots: [const { UnsafeCell::new(MaybeUninit::uninit()) }; N],
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
            overflow: AtomicU64::new(0),
        }
    }

    /// Attempts to enqueue without waiting or allocating.
    pub fn push(&self, value: T) -> Result<(), T> {
        if N == 0 {
            self.overflow.fetch_add(1, Ordering::Relaxed);
            return Err(value);
        }
        let write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        if write.wrapping_sub(read) >= N {
            self.overflow.fetch_add(1, Ordering::Relaxed);
            return Err(value);
        }
        let index = write % N;
        // SAFETY: only the producer writes this unpublished slot.
        unsafe { (*self.slots[index].get()).write(value) };
        self.write.store(write.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Returns the oldest item without consuming it.
    pub fn peek(&self) -> Option<T> {
        if N == 0 {
            return None;
        }
        let read = self.read.load(Ordering::Relaxed);
        let write = self.write.load(Ordering::Acquire);
        if read == write {
            return None;
        }
        let index = read % N;
        // SAFETY: acquire observed publication and the producer cannot reuse this slot
        // until the consumer advances `read`.
        Some(unsafe { (*self.slots[index].get()).assume_init_read() })
    }

    /// Consumes the oldest item without waiting or allocating.
    pub fn pop(&self) -> Option<T> {
        let value = self.peek()?;
        let read = self.read.load(Ordering::Relaxed);
        self.read.store(read.wrapping_add(1), Ordering::Release);
        Some(value)
    }

    pub fn capacity(&self) -> usize {
        N
    }

    pub fn len(&self) -> usize {
        let write = self.write.load(Ordering::Acquire);
        let read = self.read.load(Ordering::Acquire);
        write.wrapping_sub(read).min(N)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn overflow_count(&self) -> u64 {
        self.overflow.load(Ordering::Relaxed)
    }
}
