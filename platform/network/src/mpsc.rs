//! Multi-producer, single-consumer channels with the `std::sync::mpsc` API
//! and a small per-message-type code footprint.
//!
//! `std::sync::mpsc` is the `mpmc` channel underneath: three flavors (list,
//! array, zero), each with its own send/recv/timeout/disconnect paths and
//! waker machinery, all monomorphized per message type -- about 4k LLVM lines
//! for every `T` a crate sends. This one is a `VecDeque<T>` guarded by a
//! non-generic `Mutex<Counts>` with two condvars: locking, waiting, counting
//! and waking are compiled once; the part per `T` is the queue push/pop and
//! the drop of what is left.
//!
//! Same semantics as std for what the platform uses:
//! - `channel()` never blocks the sender; `send` fails only once the
//!   receiver is gone, handing the value back.
//! - `sync_channel(n)` blocks `send` while `n` values are queued and
//!   `try_send` returns `Full` instead. A capacity of 0 is treated as 1
//!   (std's rendezvous hand-over is not implemented; nothing here uses it).
//! - `recv` blocks until a value arrives or every sender is gone (values
//!   still queued are delivered first), `try_recv` never blocks,
//!   `recv_timeout` waits at most the given time.
//! - Dropping the receiver drops the queued values (outside the lock) and
//!   fails every later send.
//!
//! On wasm32 the lock is taken with `try_lock` in a spin loop: a browser UI
//! or AudioWorklet thread must not reach the futex wait a contended
//! `Mutex::lock` performs (the critical sections are a queue push or pop).

use std::{
    cell::UnsafeCell,
    collections::VecDeque,
    fmt,
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

pub use std::sync::mpsc::{RecvError, RecvTimeoutError, SendError, TryRecvError, TrySendError};

/// Channel state that does not depend on `T`; its mutex also guards the queue.
struct Counts {
    /// Values queued.
    len: usize,
    /// Queue bound; `usize::MAX` for an unbounded channel.
    cap: usize,
    senders: usize,
    receiver: bool,
}

/// The non-generic half of a channel: locking, waiting, counting and waking
/// are compiled once, not once per message type.
struct Core {
    counts: Mutex<Counts>,
    /// A value was queued or the last sender went away.
    readable: Condvar,
    /// A value was taken from a bounded queue or the receiver went away.
    writable: Condvar,
}

enum SendFail {
    Full,
    Disconnected,
}

enum Wait {
    No,
    Forever,
    Until(Instant),
}

enum RecvFail {
    Empty,
    Timeout,
    Disconnected,
}

impl Core {
    fn new(cap: usize) -> Self {
        Core {
            counts: Mutex::new(Counts {
                len: 0,
                cap,
                senders: 1,
                receiver: true,
            }),
            readable: Condvar::new(),
            writable: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Counts> {
        #[cfg(target_arch = "wasm32")]
        loop {
            match self.counts.try_lock() {
                Ok(guard) => return guard,
                Err(std::sync::TryLockError::Poisoned(error)) => return error.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => std::hint::spin_loop(),
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.counts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The lock, once there is room for one more value (`block`: wait for
    /// it in a full bounded queue).
    fn begin_send(&self, block: bool) -> Result<MutexGuard<'_, Counts>, SendFail> {
        let mut counts = self.lock();
        loop {
            if !counts.receiver {
                return Err(SendFail::Disconnected);
            }
            if counts.len < counts.cap {
                return Ok(counts);
            }
            if !block {
                return Err(SendFail::Full);
            }
            counts = self
                .writable
                .wait(counts)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn end_send(&self, mut counts: MutexGuard<'_, Counts>) {
        counts.len += 1;
        drop(counts);
        self.readable.notify_one();
    }

    /// The lock, once a value is queued.
    fn begin_recv(&self, wait: Wait) -> Result<MutexGuard<'_, Counts>, RecvFail> {
        let mut counts = self.lock();
        loop {
            if counts.len > 0 {
                return Ok(counts);
            }
            if counts.senders == 0 {
                return Err(RecvFail::Disconnected);
            }
            counts = match wait {
                Wait::No => return Err(RecvFail::Empty),
                Wait::Forever => self
                    .readable
                    .wait(counts)
                    .unwrap_or_else(PoisonError::into_inner),
                Wait::Until(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(RecvFail::Timeout);
                    }
                    self.readable
                        .wait_timeout(counts, deadline - now)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
    }

    fn end_recv(&self, mut counts: MutexGuard<'_, Counts>) {
        counts.len -= 1;
        let bounded = counts.cap != usize::MAX;
        drop(counts);
        if bounded {
            self.writable.notify_one();
        }
    }

    fn add_sender(&self) {
        self.lock().senders += 1;
    }

    fn drop_sender(&self) {
        let last = {
            let mut counts = self.lock();
            counts.senders -= 1;
            counts.senders == 0
        };
        if last {
            self.readable.notify_all();
        }
    }

    /// Marks the receiver gone; the caller empties the queue under the
    /// returned lock, then calls `receiver_closed`.
    fn close_receiver(&self) -> MutexGuard<'_, Counts> {
        let mut counts = self.lock();
        counts.receiver = false;
        counts.len = 0;
        counts
    }

    fn receiver_closed(&self) {
        self.writable.notify_all();
    }
}

struct Chan<T> {
    core: Core,
    /// Only touched while holding `core.counts`.
    queue: UnsafeCell<VecDeque<T>>,
}

// SAFETY: the queue is only accessed under `core.counts`' lock (see the
// `Chan` methods), so the channel hands values between threads like a
// `Mutex<VecDeque<T>>` does.
unsafe impl<T: Send> Send for Chan<T> {}
unsafe impl<T: Send> Sync for Chan<T> {}

impl<T> Chan<T> {
    fn new(cap: usize) -> Arc<Self> {
        Arc::new(Chan {
            core: Core::new(cap),
            queue: UnsafeCell::new(VecDeque::new()),
        })
    }

    fn send(&self, value: T, block: bool) -> Result<(), TrySendError<T>> {
        match self.core.begin_send(block) {
            Ok(counts) => {
                // SAFETY: `counts` is the lock guarding the queue.
                unsafe { (*self.queue.get()).push_back(value) };
                self.core.end_send(counts);
                Ok(())
            }
            Err(SendFail::Full) => Err(TrySendError::Full(value)),
            Err(SendFail::Disconnected) => Err(TrySendError::Disconnected(value)),
        }
    }

    fn recv(&self, wait: Wait) -> Result<T, RecvFail> {
        let counts = self.core.begin_recv(wait)?;
        // SAFETY: `counts` is the lock guarding the queue, and it holds
        // `len > 0` values.
        let value = unsafe { (*self.queue.get()).pop_front() };
        self.core.end_recv(counts);
        Ok(value.expect("channel length out of step with its queue"))
    }

    fn drop_receiver(&self) {
        let counts = self.core.close_receiver();
        // SAFETY: `counts` is the lock guarding the queue.
        let queued = unsafe { std::mem::take(&mut *self.queue.get()) };
        drop(counts);
        self.core.receiver_closed();
        // The values' own drops run outside the lock: one may send on this
        // very channel.
        drop(queued);
    }
}

/// Creates an unbounded channel: `send` never blocks.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let chan = Chan::new(usize::MAX);
    (Sender { chan: chan.clone() }, Receiver { chan })
}

/// Creates a bounded channel: `send` blocks while `bound` values are queued
/// (a `bound` of 0 is treated as 1; see the module notes).
pub fn sync_channel<T>(bound: usize) -> (SyncSender<T>, Receiver<T>) {
    let chan = Chan::new(bound.max(1));
    (SyncSender { chan: chan.clone() }, Receiver { chan })
}

/// The sending half of [`channel`].
pub struct Sender<T> {
    chan: Arc<Chan<T>>,
}

/// The sending half of [`sync_channel`].
pub struct SyncSender<T> {
    chan: Arc<Chan<T>>,
}

/// The receiving half of [`channel`] and [`sync_channel`].
pub struct Receiver<T> {
    chan: Arc<Chan<T>>,
}

impl<T> Sender<T> {
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        self.chan.send(value, true).map_err(|error| match error {
            TrySendError::Disconnected(value) | TrySendError::Full(value) => SendError(value),
        })
    }
}

impl<T> SyncSender<T> {
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        self.chan.send(value, true).map_err(|error| match error {
            TrySendError::Disconnected(value) | TrySendError::Full(value) => SendError(value),
        })
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        self.chan.send(value, false)
    }
}

impl<T> Receiver<T> {
    pub fn recv(&self) -> Result<T, RecvError> {
        self.chan.recv(Wait::Forever).map_err(|_| RecvError)
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.chan.recv(Wait::No).map_err(|fail| match fail {
            RecvFail::Disconnected => TryRecvError::Disconnected,
            RecvFail::Empty | RecvFail::Timeout => TryRecvError::Empty,
        })
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        let wait = match Instant::now().checked_add(timeout) {
            Some(deadline) => Wait::Until(deadline),
            None => Wait::Forever,
        };
        self.chan.recv(wait).map_err(|fail| match fail {
            RecvFail::Disconnected => RecvTimeoutError::Disconnected,
            RecvFail::Empty | RecvFail::Timeout => RecvTimeoutError::Timeout,
        })
    }

    /// Blocks for each value until every sender is gone.
    pub fn iter(&self) -> Iter<'_, T> {
        Iter { rx: self }
    }

    /// The values queued now, without blocking.
    pub fn try_iter(&self) -> TryIter<'_, T> {
        TryIter { rx: self }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.chan.core.add_sender();
        Sender {
            chan: self.chan.clone(),
        }
    }
}

impl<T> Clone for SyncSender<T> {
    fn clone(&self) -> Self {
        self.chan.core.add_sender();
        SyncSender {
            chan: self.chan.clone(),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.chan.core.drop_sender();
    }
}

impl<T> Drop for SyncSender<T> {
    fn drop(&mut self) {
        self.chan.core.drop_sender();
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.chan.drop_receiver();
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sender { .. }")
    }
}

impl<T> fmt::Debug for SyncSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SyncSender { .. }")
    }
}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Receiver { .. }")
    }
}

pub struct Iter<'a, T> {
    rx: &'a Receiver<T>,
}

pub struct TryIter<'a, T> {
    rx: &'a Receiver<T>,
}

pub struct IntoIter<T> {
    rx: Receiver<T>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.rx.recv().ok()
    }
}

impl<'a, T> Iterator for TryIter<'a, T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.rx.try_recv().ok()
    }
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.rx.recv().ok()
    }
}

impl<'a, T> IntoIterator for &'a Receiver<T> {
    type Item = T;
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<T> IntoIterator for Receiver<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> IntoIter<T> {
        IntoIter { rx: self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn unbounded_delivers_in_order_then_disconnects() {
        let (tx, rx) = channel();
        for i in 0..100 {
            tx.send(i).unwrap();
        }
        drop(tx);
        assert_eq!(rx.iter().collect::<Vec<_>>(), (0..100).collect::<Vec<_>>());
        assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
        assert_eq!(rx.recv(), Err(RecvError));
    }

    #[test]
    fn send_after_receiver_drop_returns_the_value() {
        let (tx, rx) = channel();
        drop(rx);
        assert_eq!(tx.send(7), Err(SendError(7)));
        let (tx, rx) = sync_channel(1);
        drop(rx);
        assert_eq!(tx.try_send(8), Err(TrySendError::Disconnected(8)));
        assert_eq!(tx.send(9), Err(SendError(9)));
    }

    #[test]
    fn bounded_try_send_reports_full() {
        let (tx, rx) = sync_channel(2);
        tx.try_send(1).unwrap();
        tx.try_send(2).unwrap();
        assert_eq!(tx.try_send(3), Err(TrySendError::Full(3)));
        assert_eq!(rx.try_recv(), Ok(1));
        tx.try_send(3).unwrap();
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), vec![2, 3]);
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn bounded_send_blocks_until_room() {
        let (tx, rx) = sync_channel(1);
        let producer = thread::spawn(move || {
            for i in 0..1000 {
                tx.send(i).unwrap();
            }
        });
        let got: Vec<i32> = rx.iter().collect();
        producer.join().unwrap();
        assert_eq!(got, (0..1000).collect::<Vec<_>>());
    }

    #[test]
    fn recv_timeout_times_out_and_disconnects() {
        let (tx, rx) = channel::<u32>();
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(5)),
            Err(RecvTimeoutError::Timeout)
        );
        tx.send(1).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_millis(5)), Ok(1));
        drop(tx);
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(5)),
            Err(RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn many_producers_under_contention() {
        let (tx, rx) = channel();
        let producers: Vec<_> = (0..8)
            .map(|p| {
                let tx = tx.clone();
                thread::spawn(move || {
                    for i in 0..10_000u64 {
                        tx.send(p * 1_000_000 + i).unwrap();
                    }
                })
            })
            .collect();
        drop(tx);
        let mut last = [None::<u64>; 8];
        let mut count = 0;
        for value in rx.iter() {
            let p = (value / 1_000_000) as usize;
            let i = value % 1_000_000;
            // Each producer's values arrive in its own send order.
            assert!(last[p].map_or(true, |prev| prev < i));
            last[p] = Some(i);
            count += 1;
        }
        for producer in producers {
            producer.join().unwrap();
        }
        assert_eq!(count, 80_000);
    }

    #[test]
    fn bounded_contention_with_blocking_receiver() {
        let (tx, rx) = sync_channel(4);
        let producers: Vec<_> = (0..4)
            .map(|_| {
                let tx = tx.clone();
                thread::spawn(move || {
                    for i in 0..5_000u64 {
                        tx.send(i).unwrap();
                    }
                })
            })
            .collect();
        drop(tx);
        let sum: u64 = rx.iter().sum();
        for producer in producers {
            producer.join().unwrap();
        }
        assert_eq!(sum, 4 * (0..5_000u64).sum::<u64>());
    }

    #[test]
    fn receiver_drop_drops_queued_values_and_wakes_blocked_sender() {
        let marker = Arc::new(());
        let (tx, rx) = sync_channel(1);
        tx.send(marker.clone()).unwrap();
        let blocked = {
            let marker = marker.clone();
            thread::spawn(move || tx.send(marker).is_err())
        };
        thread::sleep(Duration::from_millis(20));
        drop(rx);
        assert!(blocked.join().unwrap());
        assert_eq!(Arc::strong_count(&marker), 1);
    }
}
