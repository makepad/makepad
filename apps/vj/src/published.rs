//! One writer, any readers, never a lock: a value the audio callback
//! publishes once per buffer and the UI reads once per frame.
//!
//! The writer bumps a sequence to odd, writes, and bumps it to even; a
//! reader takes the sequence, copies the value, and takes the sequence
//! again, keeping the copy only when both reads agree on an even number.
//! A reader that lands inside a write simply copies again. Readers never
//! block the writer and never see a value half from one publish and half
//! from the next.

use std::cell::UnsafeCell;
use std::sync::atomic::{fence, AtomicU64, Ordering};

pub struct Published<T: Copy> {
    /// Odd while a publish is in progress.
    seq: AtomicU64,
    value: UnsafeCell<T>,
}

// SAFETY: the one writer and the readers are kept apart by the sequence: a
// reader only keeps a copy taken between two equal, even sequence reads,
// and the writer bumps the sequence to odd before it touches the value and
// to even after. Readers copy through a volatile read so the compiler
// cannot assume the value holds still under them.
unsafe impl<T: Copy + Send> Sync for Published<T> {}
unsafe impl<T: Copy + Send> Send for Published<T> {}

impl<T: Copy> Published<T> {
    pub const fn new(value: T) -> Self {
        Published { seq: AtomicU64::new(0), value: UnsafeCell::new(value) }
    }

    /// Write the next value. One writer at a time: the caller keeps that
    /// promise, usually by publishing from under a lock it already holds.
    pub fn publish(&self, value: T) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        // SAFETY: readers only trust a copy bracketed by equal even
        // sequences, and this write sits between two odd-to-even bumps.
        unsafe { std::ptr::write_volatile(self.value.get(), value) };
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
    }

    /// The latest complete value. Never blocks the writer; retries only if
    /// a publish was in flight.
    pub fn read(&self) -> T {
        loop {
            let before = self.seq.load(Ordering::Acquire);
            if before & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            // SAFETY: see the type; a copy that overlapped a write is thrown
            // away below because the sequence moved.
            let value = unsafe { std::ptr::read_volatile(self.value.get()) };
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == before {
                return value;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn a_reader_never_sees_a_torn_value() {
        // Two halves that only ever agree when a copy came from one publish.
        let slot: Arc<Published<(u64, u64)>> = Arc::new(Published::new((0, 0)));
        let done = Arc::new(AtomicBool::new(false));
        let (writer_slot, writer_done) = (slot.clone(), done.clone());
        let writer = std::thread::spawn(move || {
            for n in 1..=400_000u64 {
                writer_slot.publish((n, n));
            }
            writer_done.store(true, Ordering::Release);
        });
        let mut reads = 0u64;
        let mut last = 0u64;
        while !done.load(Ordering::Acquire) || reads == 0 {
            let (a, b) = slot.read();
            assert_eq!(a, b, "a copy must come from one publish");
            assert!(a >= last, "publishes are seen in order");
            last = a;
            reads += 1;
        }
        writer.join().unwrap();
        assert_eq!(slot.read(), (400_000, 400_000));
    }

    #[test]
    fn a_fresh_slot_holds_its_first_value() {
        let slot = Published::new(7u32);
        assert_eq!(slot.read(), 7);
        slot.publish(9);
        assert_eq!(slot.read(), 9);
    }
}
