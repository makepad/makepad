use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Fixed-capacity lock-free single-producer/single-consumer queue.
///
/// One slot is reserved to distinguish full from empty, so the usable
/// capacity is `N - 1`. [`split`](Self::split) uses an exclusive borrow to
/// create the only producer and consumer handles, whose mutable methods
/// enforce the SPSC roles in safe Rust. Values are restricted to `Copy`:
/// neither side can run a destructor on the audio path.
pub struct SpscQueue<T: Copy, const N: usize> {
    slots: [UnsafeCell<MaybeUninit<T>>; N],
    head: AtomicUsize,
    tail: AtomicUsize,
}

// The SPSC protocol makes each slot exclusive to one side at a time. Release
// publication of head/tail and acquire observation establish initialization.
unsafe impl<T: Copy + Send, const N: usize> Sync for SpscQueue<T, N> {}
unsafe impl<T: Copy + Send, const N: usize> Send for SpscQueue<T, N> {}

impl<T: Copy, const N: usize> SpscQueue<T, N> {
    pub fn new() -> Self {
        Self {
            slots: core::array::from_fn(|_| UnsafeCell::new(MaybeUninit::uninit())),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    pub const fn capacity(&self) -> usize {
        N.saturating_sub(1)
    }

    pub fn split(&mut self) -> (SpscProducer<'_, T, N>, SpscConsumer<'_, T, N>) {
        let queue = &*self;
        (SpscProducer { queue }, SpscConsumer { queue })
    }

    fn push(&self, value: T) -> Result<(), T> {
        if N < 2 {
            return Err(value);
        }
        let head = self.head.load(Ordering::Relaxed);
        let next = if head + 1 == N { 0 } else { head + 1 };
        if next == self.tail.load(Ordering::Acquire) {
            return Err(value);
        }
        // SAFETY: only the producer writes at head, and observing tail != next
        // means the consumer has released this slot.
        unsafe { (*self.slots[head].get()).write(value) };
        self.head.store(next, Ordering::Release);
        Ok(())
    }

    fn pop(&self) -> Option<T> {
        if N < 2 {
            return None;
        }
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: acquire saw the producer's release for this initialized
        // slot. T is Copy, so no destructor can be duplicated or lost.
        let value = unsafe { (*self.slots[tail].get()).assume_init_read() };
        let next = if tail + 1 == N { 0 } else { tail + 1 };
        self.tail.store(next, Ordering::Release);
        Some(value)
    }

    /// A momentary SPSC-safe length snapshot.
    pub fn len(&self) -> usize {
        if N < 2 {
            return 0;
        }
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        if head >= tail { head - tail } else { N - tail + head }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The unique producer role returned by [`SpscQueue::split`].
pub struct SpscProducer<'a, T: Copy, const N: usize> {
    queue: &'a SpscQueue<T, N>,
}

impl<T: Copy, const N: usize> SpscProducer<'_, T, N> {
    /// On overflow the value is returned unchanged.
    pub fn push(&mut self, value: T) -> Result<(), T> {
        self.queue.push(value)
    }

    pub const fn capacity(&self) -> usize {
        self.queue.capacity()
    }
}

/// The unique consumer role returned by [`SpscQueue::split`].
pub struct SpscConsumer<'a, T: Copy, const N: usize> {
    queue: &'a SpscQueue<T, N>,
}

impl<T: Copy, const N: usize> SpscConsumer<'_, T, N> {
    /// Empty queues return immediately.
    pub fn pop(&mut self) -> Option<T> {
        self.queue.pop()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

impl<T: Copy, const N: usize> Default for SpscQueue<T, N> {
    fn default() -> Self {
        Self::new()
    }
}
