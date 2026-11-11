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
    /// The logical capacity of the ring.
    pub fn capacity(&self) -> usize {
        self.ring.limit
    }

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

impl<T: Copy + Send> Producer<T> {
    /// Pushes as many of `values` as fit, in order; returns how many. The rest stays with the caller.
    pub fn push_slice(&mut self, values: &[T]) -> usize {
        let ring = &*self.ring;
        let head = ring.head.load(Ordering::Relaxed);
        let free = ring.limit - head.wrapping_sub(ring.tail.load(Ordering::Acquire));
        let n = values.len().min(free);
        if n == 0 {
            return 0;
        }
        let start = head & ring.mask;
        let first = n.min(ring.slots.len() - start);
        // Safety: these free slots are outside the consumer's range until
        // `head` is published below. UnsafeCell<MaybeUninit<T>> has T's
        // layout and the pointers come from the whole slot slice, so each run
        // is contiguous, aligned and disjoint from `values`.
        unsafe {
            std::ptr::copy_nonoverlapping(
                values.as_ptr(),
                UnsafeCell::raw_get(ring.slots.as_ptr().add(start)).cast::<T>(),
                first,
            );
            if first < n {
                std::ptr::copy_nonoverlapping(
                    values.as_ptr().add(first),
                    UnsafeCell::raw_get(ring.slots.as_ptr()).cast::<T>(),
                    n - first,
                );
            }
        }
        ring.head.store(head.wrapping_add(n), Ordering::Release);
        n
    }
}

impl<T: Send> Consumer<T> {
    /// The logical capacity of the ring.
    pub fn capacity(&self) -> usize {
        self.ring.limit
    }

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

impl<T: Copy + Send> Consumer<T> {
    /// Pops as many values as fit in `out`, in order; returns how many.
    pub fn pop_slice(&mut self, out: &mut [T]) -> usize {
        let ring = &*self.ring;
        let tail = ring.tail.load(Ordering::Relaxed);
        let len = ring.head.load(Ordering::Acquire).wrapping_sub(tail);
        let n = out.len().min(len);
        if n == 0 {
            return 0;
        }
        let start = tail & ring.mask;
        let first = n.min(ring.slots.len() - start);
        // Safety: these slots were initialized before `head` was published
        // and remain owned by the consumer until `tail` is published below.
        // UnsafeCell<MaybeUninit<T>> has T's layout and the pointers come
        // from the whole slot slice, so each run is contiguous,
        // aligned and disjoint from `out`; T: Copy needs no drop on removal.
        unsafe {
            std::ptr::copy_nonoverlapping(
                UnsafeCell::raw_get(ring.slots.as_ptr().add(start)).cast::<T>(),
                out.as_mut_ptr(),
                first,
            );
            if first < n {
                std::ptr::copy_nonoverlapping(
                    UnsafeCell::raw_get(ring.slots.as_ptr()).cast::<T>(),
                    out.as_mut_ptr().add(first),
                    n - first,
                );
            }
        }
        ring.tail.store(tail.wrapping_add(n), Ordering::Release);
        n
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn ring_keeps_order_and_refuses_when_full() {
        let (mut tx, mut rx) = ring::<u32>(4);
        assert_eq!(tx.capacity(), 4);
        assert_eq!(rx.capacity(), 4);
        for value in 0..4 {
            assert_eq!(tx.push(value), Ok(()));
        }
        // Full: the value comes back, nothing is lost or reordered.
        assert_eq!(tx.push(99), Err(99));
        assert_eq!(rx.len(), 4);
        assert_eq!(rx.pop(), Some(0));
        assert_eq!(tx.push(4), Ok(()));
        for expect in 1..5 {
            assert_eq!(rx.pop(), Some(expect));
        }
        assert_eq!(rx.pop(), None);
        assert!(rx.is_empty());
    }

    #[test]
    fn ring_moves_payloads_without_leaking_or_double_dropping() {
        let payload = Arc::new(vec![1u8; 1 << 16]);
        let (mut tx, mut rx) = ring::<Arc<Vec<u8>>>(8);
        for _ in 0..5 {
            tx.push(payload.clone()).unwrap();
        }
        assert_eq!(Arc::strong_count(&payload), 6);
        let taken = rx.pop().unwrap();
        assert_eq!(Arc::strong_count(&payload), 6);
        drop(taken);
        assert_eq!(Arc::strong_count(&payload), 5);
        // The last endpoint's drop releases what was never popped.
        drop(tx);
        assert_eq!(Arc::strong_count(&payload), 5);
        drop(rx);
        assert_eq!(Arc::strong_count(&payload), 1);
    }

    #[test]
    fn ring_carries_values_across_threads_in_order() {
        let (mut tx, mut rx) = ring::<u64>(64);
        let producer = std::thread::spawn(move || {
            let mut next = 0u64;
            while next < 10_000 {
                if tx.push(next).is_ok() {
                    next += 1;
                } else {
                    std::thread::yield_now();
                }
            }
        });
        let mut expect = 0u64;
        while expect < 10_000 {
            match rx.pop() {
                Some(value) => {
                    assert_eq!(value, expect);
                    expect += 1;
                }
                None => std::thread::yield_now(),
            }
        }
        producer.join().unwrap();
    }

    #[test]
    fn slices_cross_the_wrap_in_order() {
        let (mut tx, mut rx) = ring::<u32>(5);
        assert_eq!(tx.capacity(), 5);
        assert_eq!(rx.capacity(), 5);
        assert_eq!(tx.push_slice(&[0, 1, 2]), 3);
        let mut pair = [0; 2];
        assert_eq!(rx.pop_slice(&mut pair), 2);
        assert_eq!(pair, [0, 1]);
        assert_eq!(tx.push_slice(&[3, 4, 5, 6, 7, 8, 9]), 4);
        assert_eq!(tx.free(), 0);
        let mut out = [0; 10];
        assert_eq!(rx.pop_slice(&mut out), 5);
        assert_eq!(&out[..5], &[2, 3, 4, 5, 6]);
        for round in 0..3 {
            let start = 7 + round * 3;
            let values = [start, start + 1, start + 2];
            assert_eq!(tx.push_slice(&values), 3);
            assert_eq!(rx.pop_slice(&mut out[..3]), 3);
            assert_eq!(&out[..3], &values);
        }
    }
}
