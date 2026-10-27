//! A triple buffer: one writer publishes whole values, one reader always
//! sees the newest published one. Neither side locks, waits or allocates;
//! they trade slot indices through one atomic.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

const INDEX: u8 = 0b011;
const DIRTY: u8 = 0b100;

struct Shared<T> {
    slots: [UnsafeCell<T>; 3],
    /// Index of the slot neither side owns, plus `DIRTY` when it holds a
    /// value the reader has not taken yet.
    back: AtomicU8,
}

// Each slot is only ever touched by the side that currently owns its index;
// ownership moves by atomic swap, so a slot is never shared.
unsafe impl<T: Send> Sync for Shared<T> {}

pub struct Writer<T> {
    shared: Arc<Shared<T>>,
    slot: u8,
}

pub struct Reader<T> {
    shared: Arc<Shared<T>>,
    slot: u8,
}

pub fn triple_buffer<T: Clone + Send>(initial: T) -> (Writer<T>, Reader<T>) {
    let shared = Arc::new(Shared {
        slots: [
            UnsafeCell::new(initial.clone()),
            UnsafeCell::new(initial.clone()),
            UnsafeCell::new(initial),
        ],
        back: AtomicU8::new(1),
    });
    (
        Writer { shared: shared.clone(), slot: 0 },
        Reader { shared, slot: 2 },
    )
}

impl<T: Send> Writer<T> {
    /// Publish `value`. The value it replaces is dropped here, on the
    /// writer's thread.
    pub fn publish(&mut self, value: T) {
        // Safety: `self.slot` is owned by the writer until the swap below.
        unsafe {
            *self.shared.slots[self.slot as usize].get() = value;
        }
        let previous = self.shared.back.swap(self.slot | DIRTY, Ordering::AcqRel);
        self.slot = previous & INDEX;
    }

    /// Publish by rewriting the writer's own slot in place: `write` sees
    /// whatever value that slot last held (one published before, or the
    /// initial) and must set every part of it. Nothing is allocated or
    /// dropped, so a value with buffers sized up front publishes from a
    /// realtime thread.
    pub fn update(&mut self, write: impl FnOnce(&mut T)) {
        // Safety: `self.slot` is owned by the writer until the swap below.
        unsafe {
            write(&mut *self.shared.slots[self.slot as usize].get());
        }
        let previous = self.shared.back.swap(self.slot | DIRTY, Ordering::AcqRel);
        self.slot = previous & INDEX;
    }
}

impl<T: Send> Reader<T> {
    /// The newest published value (the initial one until a first publish).
    pub fn latest(&mut self) -> &T {
        if self.shared.back.load(Ordering::Acquire) & DIRTY != 0 {
            let previous = self.shared.back.swap(self.slot, Ordering::AcqRel);
            self.slot = previous & INDEX;
        }
        // Safety: `self.slot` is owned by the reader until its next swap.
        unsafe { &*self.shared.slots[self.slot as usize].get() }
    }
}
