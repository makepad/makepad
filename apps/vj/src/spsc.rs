//! One producer, one consumer, never a lock: the hand-off primitive for
//! everything that crosses the audio thread's boundary in either direction.
//!
//! The ring is a power-of-two number of slots indexed by two monotonic
//! counters, so a wrap is a mask and the counters never need resetting. The
//! producer writes slots, then publishes its counter with release; the
//! consumer acquires that counter, reads, and publishes its own the same
//! way. Beyond `push` and `pop`, both ends can take their free or filled
//! slots as at most two contiguous slices — a producer can render straight
//! into ring memory and a consumer can encode straight out of it, with no
//! copy between — and the consumer can flush everything unread, which is
//! what a stream that reconnects needs so it does not send its backlog.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The slots and the two counters both ends share.
struct Ring<T> {
    slots: Box<[UnsafeCell<T>]>,
    mask: usize,
    /// Items the producer has committed, ever.
    tail: AtomicU64,
    /// Items the consumer has released, ever.
    head: AtomicU64,
}

// SAFETY: the producer only touches slots in `[tail, head + capacity)` and
// the consumer only slots in `[head, tail)`; each counter is written by one
// side with release and read by the other with acquire, so no slot is ever
// touched from two threads at once.
unsafe impl<T: Send> Send for Ring<T> {}
unsafe impl<T: Send> Sync for Ring<T> {}

/// The writing end. Owned by one thread.
pub struct Producer<T> {
    ring: Arc<Ring<T>>,
    tail: u64,
}

/// The reading end. Owned by one thread.
pub struct Consumer<T> {
    ring: Arc<Ring<T>>,
    head: u64,
}

/// A ring of at least `capacity` slots, rounded up to a power of two, with
/// every slot usable.
pub fn channel<T: Copy + Default>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let capacity = capacity.max(2).next_power_of_two();
    let slots = (0..capacity).map(|_| UnsafeCell::new(T::default())).collect();
    let ring = Arc::new(Ring {
        slots,
        mask: capacity - 1,
        tail: AtomicU64::new(0),
        head: AtomicU64::new(0),
    });
    (Producer { ring: ring.clone(), tail: 0 }, Consumer { ring, head: 0 })
}

impl<T: Copy> Producer<T> {
    pub fn capacity(&self) -> usize {
        self.ring.mask + 1
    }

    /// Slots the producer may write right now.
    pub fn free(&self) -> usize {
        let head = self.ring.head.load(Ordering::Acquire);
        self.capacity() - (self.tail - head) as usize
    }

    /// One item, or the item back when the ring is full.
    pub fn push(&mut self, item: T) -> Result<(), T> {
        if self.free() == 0 {
            return Err(item);
        }
        let slot = (self.tail as usize) & self.ring.mask;
        // SAFETY: this slot is in the producer's range (see `Ring`).
        unsafe { *self.ring.slots[slot].get() = item };
        self.commit(1);
        Ok(())
    }

    /// The free slots as at most two runs: the tail of the buffer, then its
    /// head. Write into them, then `commit` how many.
    pub fn write_regions(&mut self) -> (&mut [T], &mut [T]) {
        let free = self.free();
        let start = (self.tail as usize) & self.ring.mask;
        let first = free.min(self.capacity() - start);
        let base = self.ring.slots.as_ptr() as *mut T;
        // SAFETY: both runs lie in the producer's range, they do not overlap,
        // and they borrow `self` for as long as they live.
        unsafe {
            (
                std::slice::from_raw_parts_mut(base.add(start), first),
                std::slice::from_raw_parts_mut(base, free - first),
            )
        }
    }

    /// Publish `count` slots written through `write_regions`.
    pub fn commit(&mut self, count: usize) {
        debug_assert!(count <= self.free(), "committing more than was free");
        self.tail += count as u64;
        self.ring.tail.store(self.tail, Ordering::Release);
    }
}

impl<T: Copy> Consumer<T> {
    /// Items waiting.
    pub fn len(&self) -> usize {
        (self.ring.tail.load(Ordering::Acquire) - self.head) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.is_empty() {
            return None;
        }
        let slot = (self.head as usize) & self.ring.mask;
        // SAFETY: this slot is in the consumer's range (see `Ring`).
        let item = unsafe { *self.ring.slots[slot].get() };
        self.release(1);
        Some(item)
    }

    /// The waiting items as at most two runs, oldest first. Read them, then
    /// `release` how many.
    pub fn read_regions(&self) -> (&[T], &[T]) {
        let len = self.len();
        let start = (self.head as usize) & self.ring.mask;
        let first = len.min(self.ring.mask + 1 - start);
        let base = self.ring.slots.as_ptr() as *const T;
        // SAFETY: both runs lie in the consumer's range and borrow `self`.
        unsafe {
            (
                std::slice::from_raw_parts(base.add(start), first),
                std::slice::from_raw_parts(base, len - first),
            )
        }
    }

    /// Give `count` slots read through `read_regions` back to the producer.
    pub fn release(&mut self, count: usize) {
        debug_assert!(count <= self.len(), "releasing more than was waiting");
        self.head += count as u64;
        self.ring.head.store(self.head, Ordering::Release);
    }

    /// Drop everything unread.
    pub fn flush(&mut self) {
        self.head = self.ring.tail.load(Ordering::Acquire);
        self.ring.head.store(self.head, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_come_out_in_the_order_they_went_in() {
        let (mut tx, mut rx) = channel::<u32>(4);
        assert_eq!(rx.pop(), None);
        tx.push(1).unwrap();
        tx.push(2).unwrap();
        tx.push(3).unwrap();
        assert_eq!(rx.len(), 3);
        assert_eq!(rx.pop(), Some(1));
        assert_eq!(rx.pop(), Some(2));
        assert_eq!(rx.pop(), Some(3));
        assert_eq!(rx.pop(), None);
    }

    #[test]
    fn capacity_rounds_up_and_a_full_ring_refuses_the_next_item() {
        let (mut tx, mut rx) = channel::<u8>(5);
        assert_eq!(tx.capacity(), 8);
        for item in 0..8 {
            tx.push(item).unwrap();
        }
        assert_eq!(tx.free(), 0);
        assert_eq!(tx.push(9), Err(9), "a full ring hands the item back");
        assert_eq!(rx.pop(), Some(0));
        assert_eq!(tx.free(), 1);
    }

    #[test]
    fn regions_hand_out_the_free_slots_across_the_wrap() {
        let (mut tx, mut rx) = channel::<u32>(8);
        for item in 0..6 {
            tx.push(item).unwrap();
        }
        for _ in 0..6 {
            rx.pop();
        }
        // Both counters sit at 6: the free run is slots 6 and 7, then 0..6.
        let (first, second) = tx.write_regions();
        assert_eq!((first.len(), second.len()), (2, 6));
        first[0] = 100;
        first[1] = 101;
        second[0] = 102;
        tx.commit(3);
        assert_eq!(rx.len(), 3);
        let (a, b) = rx.read_regions();
        assert_eq!(a, &[100, 101]);
        assert_eq!(b, &[102]);
        rx.release(3);
        assert_eq!(rx.len(), 0);
    }

    #[test]
    fn flush_drops_everything_unread() {
        let (mut tx, mut rx) = channel::<u32>(4);
        tx.push(1).unwrap();
        tx.push(2).unwrap();
        rx.flush();
        assert_eq!(rx.len(), 0);
        assert_eq!(rx.pop(), None);
        tx.push(3).unwrap();
        assert_eq!(rx.pop(), Some(3), "the ring keeps working after a flush");
    }

    #[test]
    fn two_threads_hand_over_every_item_in_order() {
        const COUNT: u64 = 200_000;
        let (mut tx, mut rx) = channel::<u64>(1024);
        let producer = std::thread::spawn(move || {
            for item in 0..COUNT {
                let mut pending = item;
                loop {
                    match tx.push(pending) {
                        Ok(()) => break,
                        Err(back) => {
                            pending = back;
                            std::hint::spin_loop();
                        }
                    }
                }
            }
        });
        let mut expected = 0;
        while expected < COUNT {
            if let Some(item) = rx.pop() {
                assert_eq!(item, expected, "nothing lost, nothing reordered");
                expected += 1;
            } else {
                std::hint::spin_loop();
            }
        }
        producer.join().unwrap();
    }

    #[test]
    fn a_hand_off_allocates_nothing() {
        use crate::music_dsp::alloc_probe;
        let (mut tx, mut rx) = channel::<[f32; 2]>(256);
        let before = alloc_probe::count();
        for i in 0..1000 {
            tx.push([i as f32, -(i as f32)]).unwrap();
            let (a, b) = tx.write_regions();
            let _ = (a.len(), b.len());
            rx.pop();
            let (c, d) = rx.read_regions();
            let _ = (c.len(), d.len());
        }
        assert_eq!(alloc_probe::count(), before, "the ring must not allocate after construction");
    }
}
