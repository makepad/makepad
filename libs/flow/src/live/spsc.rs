//! A bounded single-producer single-consumer ring whose two ends are owned
//! values: the type says which thread may push and which may pop. Neither
//! end allocates, locks or waits after construction, and the capacity is
//! exactly the one asked for (or one when asked for zero).
//!
//! Dropping the last endpoint drops queued values and frees storage on that
//! thread. A realtime owner must retain the other endpoint off-thread until
//! its callback shutdown is acknowledged, and return payloads for disposal.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Ring<T> {
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    limit: usize,
    mask: usize,
    /// Values pushed so far; written by the producer only.
    head: AtomicUsize,
    /// Values popped so far; written by the consumer only.
    tail: AtomicUsize,
}

// A value moves through exactly once, from the one producer to the one
// consumer; each slot is touched by one side at a time.
unsafe impl<T: Send> Send for Ring<T> {}
unsafe impl<T: Send> Sync for Ring<T> {}

pub struct Producer<T> {
    ring: Arc<Ring<T>>,
}

pub struct Consumer<T> {
    ring: Arc<Ring<T>>,
}

/// A ring holding at most `capacity` values (at least one).
///
/// Panics before allocation if the physical power-of-two capacity cannot be
/// represented. Logical admission remains exact, including non-powers of two.
pub fn ring<T: Send>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let limit = capacity.max(1);
    let physical = limit.checked_next_power_of_two().expect("SPSC capacity is too large");
    let slots = (0..physical).map(|_| UnsafeCell::new(MaybeUninit::uninit())).collect();
    let ring = Arc::new(Ring { slots, limit, mask: physical - 1, head: AtomicUsize::new(0), tail: AtomicUsize::new(0) });
    (Producer { ring: ring.clone() }, Consumer { ring })
}

impl<T> Ring<T> {
    fn len(&self) -> usize {
        self.head.load(Ordering::Acquire).wrapping_sub(self.tail.load(Ordering::Acquire))
    }
}

impl<T: Send> Producer<T> {
    /// A full ring hands the value back; the caller keeps it.
    pub fn push(&mut self, value: T) -> Result<(), T> {
        let ring = &*self.ring;
        let head = ring.head.load(Ordering::Relaxed);
        if head.wrapping_sub(ring.tail.load(Ordering::Acquire)) >= ring.limit {
            return Err(value);
        }
        // Safety: the slot at `head` is outside the consumer's range until
        // `head` is published below. Physical capacity divides the counter
        // modulus, so masking remains injective across counter wrap.
        unsafe { (*ring.slots[head & ring.mask].get()).write(value) };
        ring.head.store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Slots free right now. The consumer can only make this grow.
    pub fn free(&self) -> usize {
        self.ring.limit - self.ring.len()
    }
}

impl<T: Send> Consumer<T> {
    pub fn pop(&mut self) -> Option<T> {
        let ring = &*self.ring;
        let tail = ring.tail.load(Ordering::Relaxed);
        if tail == ring.head.load(Ordering::Acquire) {
            return None;
        }
        // Safety: the slot at `tail` was written before `head` moved past it
        // and is read exactly once.
        let value = unsafe { (*ring.slots[tail & ring.mask].get()).assume_init_read() };
        ring.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(value)
    }

    pub fn is_empty(&self) -> bool {
        self.ring.len() == 0
    }

    /// Values waiting right now. The producer can only make this grow.
    pub fn len(&self) -> usize {
        self.ring.len()
    }
}

impl<T> Drop for Ring<T> {
    fn drop(&mut self) {
        let (head, mut tail) = (*self.head.get_mut(), *self.tail.get_mut());
        while tail != head {
            unsafe { (*self.slots[tail & self.mask].get()).assume_init_drop() };
            tail = tail.wrapping_add(1);
        }
    }
}
