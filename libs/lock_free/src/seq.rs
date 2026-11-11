//! A seqlock over a `Copy` snapshot: one writer that never waits, readers that re-read while a write is in flight.
//! The writer is the realtime side (an audio callback publishing what it did this block). A reader that catches
//! the writer mid-copy spins until the copy ends, so the writer must be a thread that is never preempted for long:
//! never give a SeqCell a writer whose value a realtime thread reads (hand that over through a ring instead).

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

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

    /// The one writer (the realtime side).
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn seqcell_reads_are_never_torn() {
        #[derive(Clone, Copy, PartialEq, Debug)]
        struct Pair(u64, u64);
        let cell: Arc<SeqCell<Pair>> = Arc::new(SeqCell::new(Pair(0, !0)));
        let writer = {
            let cell = cell.clone();
            std::thread::spawn(move || {
                for value in 1..20_000u64 {
                    cell.write(Pair(value, !value));
                }
            })
        };
        for _ in 0..20_000 {
            let pair = cell.read();
            assert_eq!(pair.1, !pair.0, "torn read {pair:?}");
        }
        writer.join().unwrap();
    }
}
