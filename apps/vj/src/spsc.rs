//! The lock-free plumbing between the UI thread and the audio callback.
//!
//! Nothing in here ever waits: the audio thread must never block on the UI
//! (a late buffer is a heard gap) and, on the web, neither thread may enter
//! `Atomics.wait` at all. So the two sides share only
//!
//! - [`SpscRing`]: a fixed-capacity single-producer single-consumer ring.
//!   The producer allocates nothing on push and the consumer frees nothing
//!   on pop: slots are preallocated and values are moved through them.
//!   A full ring refuses the value and hands it back — the caller keeps it
//!   and retries later, so an audio device that is not running (a web
//!   `AudioContext` waiting for its first gesture) can never wedge the UI.
//! - [`SeqCell`]: a seqlock over a `Copy` snapshot. The writer never
//!   waits; a reader that catches the writer mid-copy re-reads, and the
//!   copy is a few hundred bytes, so that retry is bounded by nanoseconds,
//!   never by another thread's critical section.
//! - [`UiCell`]: state that belongs to ONE thread. It is not a lock — a
//!   second thread touching it is a bug and panics at once — which is how
//!   the UI-side bookkeeping of a `Clone` handle stays lock-free and
//!   provably single-threaded.
//! - [`OnceSlot`]: a value handed over exactly once (the audio engine, from
//!   the handle that built it to the callback that owns it).

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Fixed-capacity single-producer single-consumer ring. `push` and `pop`
/// are each wait-free and allocation-free; the capacity is rounded up to
/// a power of two.
pub struct SpscRing<T> {
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    mask: usize,
    /// Next slot to write, advanced by the producer only.
    head: AtomicUsize,
    /// Next slot to read, advanced by the consumer only.
    tail: AtomicUsize,
}

// Values move through the ring exactly once, from the producer to the
// consumer, so sharing the ring between the two threads is sharing `T`
// between two threads: `T: Send` is the whole requirement.
unsafe impl<T: Send> Send for SpscRing<T> {}
unsafe impl<T: Send> Sync for SpscRing<T> {}

impl<T> SpscRing<T> {
    pub fn new(capacity: usize) -> SpscRing<T> {
        let capacity = capacity.max(2).next_power_of_two();
        let slots = (0..capacity)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        SpscRing { slots, mask: capacity - 1, head: AtomicUsize::new(0), tail: AtomicUsize::new(0) }
    }

    pub fn capacity(&self) -> usize {
        self.mask + 1
    }

    /// Values waiting to be popped, as of this instant.
    pub fn len(&self) -> usize {
        self.head
            .load(Ordering::Acquire)
            .wrapping_sub(self.tail.load(Ordering::Acquire))
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Producer side. A full ring hands the value back untouched.
    pub fn push(&self, value: T) -> Result<(), T> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= self.capacity() {
            return Err(value);
        }
        // SAFETY: the slot at `head` is past every slot the consumer can
        // still read (`tail..head`), and only the producer writes `head`.
        unsafe { (*self.slots[head & self.mask].get()).write(value) };
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Consumer side.
    pub fn pop(&self) -> Option<T> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail == head {
            return None;
        }
        // SAFETY: the producer's release store of `head` published this
        // slot's write, and only the consumer moves `tail`.
        let value = unsafe { (*self.slots[tail & self.mask].get()).assume_init_read() };
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(value)
    }
}

impl<T> Drop for SpscRing<T> {
    fn drop(&mut self) {
        while self.pop().is_some() {}
    }
}

/// A seqlock over a `Copy` value: one writer that never waits, readers
/// that retry only while a write is in flight.
pub struct SeqCell<T: Copy> {
    seq: AtomicUsize,
    value: UnsafeCell<T>,
}

unsafe impl<T: Copy + Send> Send for SeqCell<T> {}
unsafe impl<T: Copy + Send> Sync for SeqCell<T> {}

impl<T: Copy> SeqCell<T> {
    pub fn new(value: T) -> SeqCell<T> {
        SeqCell { seq: AtomicUsize::new(0), value: UnsafeCell::new(value) }
    }

    /// Writer side (one thread only).
    pub fn write(&self, value: T) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1), Ordering::Release);
        std::sync::atomic::fence(Ordering::Release);
        // SAFETY: readers treat an odd sequence as "in flight" and never
        // trust a copy taken across one; the single writer is this call.
        unsafe { std::ptr::write_volatile(self.value.get(), value) };
        std::sync::atomic::fence(Ordering::Release);
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
    }

    /// Reader side: a consistent copy.
    pub fn read(&self) -> T {
        loop {
            let before = self.seq.load(Ordering::Acquire);
            if before & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            std::sync::atomic::fence(Ordering::Acquire);
            // SAFETY: a torn copy is detected by the sequence check below
            // and discarded; the value is `Copy`, so a torn read has no
            // destructor to run.
            let copy = unsafe { std::ptr::read_volatile(self.value.get()) };
            std::sync::atomic::fence(Ordering::Acquire);
            if self.seq.load(Ordering::Acquire) == before {
                return copy;
            }
            std::hint::spin_loop();
        }
    }
}

/// State owned by exactly one thread, kept behind a shared handle. Not a
/// lock: entering it while it is already entered — from another thread,
/// or re-entrantly — is a programming error and panics.
pub struct UiCell<T> {
    entered: AtomicBool,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for UiCell<T> {}
unsafe impl<T: Send> Sync for UiCell<T> {}

impl<T> UiCell<T> {
    pub fn new(value: T) -> UiCell<T> {
        UiCell { entered: AtomicBool::new(false), value: UnsafeCell::new(value) }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        assert!(
            !self.entered.swap(true, Ordering::Acquire),
            "UiCell entered twice: it belongs to one thread and is not re-entrant"
        );
        struct Leave<'a>(&'a AtomicBool);
        impl Drop for Leave<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _leave = Leave(&self.entered);
        // SAFETY: the flag above admits one caller at a time and panics on
        // any overlap, so this is the only live reference.
        f(unsafe { &mut *self.value.get() })
    }
}

/// A value handed over exactly once.
pub struct OnceSlot<T> {
    taken: AtomicBool,
    value: UnsafeCell<Option<T>>,
}

unsafe impl<T: Send> Send for OnceSlot<T> {}
unsafe impl<T: Send> Sync for OnceSlot<T> {}

impl<T> OnceSlot<T> {
    pub fn new(value: T) -> OnceSlot<T> {
        OnceSlot { taken: AtomicBool::new(false), value: UnsafeCell::new(Some(value)) }
    }

    /// The value, the first time; `None` ever after.
    pub fn take(&self) -> Option<T> {
        if self.taken.swap(true, Ordering::AcqRel) {
            return None;
        }
        // SAFETY: the swap above admits exactly one caller here, ever.
        unsafe { (*self.value.get()).take() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn ring_keeps_order_and_refuses_when_full() {
        let ring: SpscRing<u32> = SpscRing::new(4);
        assert_eq!(ring.capacity(), 4);
        for value in 0..4 {
            assert_eq!(ring.push(value), Ok(()));
        }
        // Full: the value comes back, nothing is lost or reordered.
        assert_eq!(ring.push(99), Err(99));
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.pop(), Some(0));
        assert_eq!(ring.push(4), Ok(()));
        for expect in 1..5 {
            assert_eq!(ring.pop(), Some(expect));
        }
        assert_eq!(ring.pop(), None);
        assert!(ring.is_empty());
    }

    #[test]
    fn ring_moves_payloads_without_leaking_or_double_dropping() {
        let payload = Arc::new(vec![1u8; 1 << 16]);
        let ring: SpscRing<Arc<Vec<u8>>> = SpscRing::new(8);
        for _ in 0..5 {
            ring.push(payload.clone()).unwrap();
        }
        assert_eq!(Arc::strong_count(&payload), 6);
        let taken = ring.pop().unwrap();
        assert_eq!(Arc::strong_count(&payload), 6);
        drop(taken);
        assert_eq!(Arc::strong_count(&payload), 5);
        // The ring's own drop releases what was never popped.
        drop(ring);
        assert_eq!(Arc::strong_count(&payload), 1);
    }

    #[test]
    fn ring_carries_values_across_threads_in_order() {
        let ring: Arc<SpscRing<u64>> = Arc::new(SpscRing::new(64));
        let producer = {
            let ring = ring.clone();
            std::thread::spawn(move || {
                let mut next = 0u64;
                while next < 10_000 {
                    if ring.push(next).is_ok() {
                        next += 1;
                    } else {
                        std::thread::yield_now();
                    }
                }
            })
        };
        let mut expect = 0u64;
        while expect < 10_000 {
            match ring.pop() {
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
    fn seqcell_reads_are_never_torn() {
        #[derive(Clone, Copy, PartialEq, Debug)]
        struct Pair(u64, u64);
        let cell: Arc<SeqCell<Pair>> = Arc::new(SeqCell::new(Pair(0, !0)));
        let writer = {
            let cell = cell.clone();
            std::thread::spawn(move || {
                for value in 1..200_000u64 {
                    cell.write(Pair(value, !value));
                }
            })
        };
        for _ in 0..200_000 {
            let pair = cell.read();
            assert_eq!(pair.1, !pair.0, "torn read {pair:?}");
        }
        writer.join().unwrap();
    }

    #[test]
    fn once_slot_hands_over_exactly_once() {
        let slot = OnceSlot::new(String::from("engine"));
        assert_eq!(slot.take().as_deref(), Some("engine"));
        assert_eq!(slot.take(), None);
    }

    #[test]
    #[should_panic(expected = "UiCell entered twice")]
    fn ui_cell_refuses_reentry() {
        let cell = UiCell::new(0u32);
        cell.with(|_| cell.with(|_| ()));
    }
}
