//! std::sync::mpsc: a Mutex<VecDeque> + Condvar channel. Same API, errors and messages
//! as real std; bounded channels block senders at capacity, capacity 0 is a rendezvous
//! (send returns once the value was received).

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::error::Error;
use core::fmt;
use core::time::Duration;

use super::{Condvar, Mutex, MutexGuard};
use crate::time::Instant;

struct State<T> {
    queue: VecDeque<T>,
    senders: usize,
    receiver_alive: bool,
    receivers_waiting: usize,
    received: u64,
}

struct Chan<T> {
    state: Mutex<State<T>>,
    recv_cv: Condvar,
    send_cv: Condvar,
    cap: Option<usize>,
}

impl<T> Chan<T> {
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        match self.state.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        }
    }
}

pub struct Sender<T> {
    chan: Arc<Chan<T>>,
}

pub struct SyncSender<T> {
    chan: Arc<Chan<T>>,
}

pub struct Receiver<T> {
    chan: Arc<Chan<T>>,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub struct SendError<T>(pub T);

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct RecvError;

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum TryRecvError {
    Empty,
    Disconnected,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum RecvTimeoutError {
    Timeout,
    Disconnected,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum TrySendError<T> {
    Full(T),
    Disconnected(T),
}

fn new_chan<T>(cap: Option<usize>) -> Arc<Chan<T>> {
    Arc::new(Chan {
        state: Mutex::new(State { queue: VecDeque::new(), senders: 1, receiver_alive: true, receivers_waiting: 0, received: 0 }),
        recv_cv: Condvar::new(),
        send_cv: Condvar::new(),
        cap,
    })
}

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let chan = new_chan(None);
    (Sender { chan: chan.clone() }, Receiver { chan })
}

pub fn sync_channel<T>(bound: usize) -> (SyncSender<T>, Receiver<T>) {
    let chan = new_chan(Some(bound));
    (SyncSender { chan: chan.clone() }, Receiver { chan })
}

impl<T> Sender<T> {
    pub fn send(&self, t: T) -> Result<(), SendError<T>> {
        let mut st = self.chan.lock();
        if !st.receiver_alive {
            return Err(SendError(t));
        }
        st.queue.push_back(t);
        drop(st);
        self.chan.recv_cv.notify_one();
        Ok(())
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Sender<T> {
        self.chan.lock().senders += 1;
        Sender { chan: self.chan.clone() }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        sender_dropped(&self.chan);
    }
}

fn sender_dropped<T>(chan: &Chan<T>) {
    let mut st = chan.lock();
    st.senders -= 1;
    let last = st.senders == 0;
    drop(st);
    if last {
        chan.recv_cv.notify_all();
    }
}

impl<T> SyncSender<T> {
    pub fn send(&self, t: T) -> Result<(), SendError<T>> {
        let cap = match self.chan.cap {
            Some(c) => c,
            None => 0,
        };
        let mut st = self.chan.lock();
        if cap > 0 {
            while st.receiver_alive && st.queue.len() >= cap {
                st = match self.chan.send_cv.wait(st) {
                    Ok(g) => g,
                    Err(e) => e.into_inner(),
                };
            }
            if !st.receiver_alive {
                return Err(SendError(t));
            }
            st.queue.push_back(t);
            drop(st);
            self.chan.recv_cv.notify_one();
            return Ok(());
        }
        // rendezvous: wait for our value to be taken
        if !st.receiver_alive {
            return Err(SendError(t));
        }
        st.queue.push_back(t);
        let ticket = st.received + st.queue.len() as u64;
        self.chan.recv_cv.notify_one();
        while st.receiver_alive && st.received < ticket {
            st = match self.chan.send_cv.wait(st) {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
        }
        Ok(())
    }

    pub fn try_send(&self, t: T) -> Result<(), TrySendError<T>> {
        let cap = match self.chan.cap {
            Some(c) => c,
            None => 0,
        };
        let mut st = self.chan.lock();
        if !st.receiver_alive {
            return Err(TrySendError::Disconnected(t));
        }
        let full = if cap == 0 { st.receivers_waiting <= st.queue.len() } else { st.queue.len() >= cap };
        if full {
            return Err(TrySendError::Full(t));
        }
        st.queue.push_back(t);
        drop(st);
        self.chan.recv_cv.notify_one();
        Ok(())
    }
}

impl<T> Clone for SyncSender<T> {
    fn clone(&self) -> SyncSender<T> {
        self.chan.lock().senders += 1;
        SyncSender { chan: self.chan.clone() }
    }
}

impl<T> Drop for SyncSender<T> {
    fn drop(&mut self) {
        sender_dropped(&self.chan);
    }
}

impl<T> Receiver<T> {
    fn took(&self, st: &mut State<T>) {
        st.received += 1;
        if self.chan.cap.is_some() {
            self.chan.send_cv.notify_all();
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let mut st = self.chan.lock();
        match st.queue.pop_front() {
            Some(t) => {
                self.took(&mut st);
                Ok(t)
            }
            None => {
                if st.senders == 0 {
                    Err(TryRecvError::Disconnected)
                } else {
                    Err(TryRecvError::Empty)
                }
            }
        }
    }

    pub fn recv(&self) -> Result<T, RecvError> {
        let mut st = self.chan.lock();
        loop {
            if let Some(t) = st.queue.pop_front() {
                self.took(&mut st);
                return Ok(t);
            }
            if st.senders == 0 {
                return Err(RecvError);
            }
            st.receivers_waiting += 1;
            st = match self.chan.recv_cv.wait(st) {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            st.receivers_waiting -= 1;
        }
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        match Instant::now().checked_add(timeout) {
            Some(deadline) => self.recv_deadline(deadline),
            None => match self.recv() {
                Ok(t) => Ok(t),
                Err(_) => Err(RecvTimeoutError::Disconnected),
            },
        }
    }

    pub fn recv_deadline(&self, deadline: Instant) -> Result<T, RecvTimeoutError> {
        let mut st = self.chan.lock();
        loop {
            if let Some(t) = st.queue.pop_front() {
                self.took(&mut st);
                return Ok(t);
            }
            if st.senders == 0 {
                return Err(RecvTimeoutError::Disconnected);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(RecvTimeoutError::Timeout);
            }
            st.receivers_waiting += 1;
            st = match self.chan.recv_cv.wait_timeout(st, deadline - now) {
                Ok((g, _)) => g,
                Err(e) => e.into_inner().0,
            };
            st.receivers_waiting -= 1;
        }
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter { rx: self }
    }

    pub fn try_iter(&self) -> TryIter<'_, T> {
        TryIter { rx: self }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut st = self.chan.lock();
        st.receiver_alive = false;
        let pending = core::mem::take(&mut st.queue);
        drop(st);
        drop(pending);
        self.chan.send_cv.notify_all();
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

impl<T> IntoIterator for Receiver<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> IntoIter<T> {
        IntoIter { rx: self }
    }
}

impl<'a, T> IntoIterator for &'a Receiver<T> {
    type Item = T;
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Sender").finish_non_exhaustive()
    }
}
impl<T> fmt::Debug for SyncSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("SyncSender").finish_non_exhaustive()
    }
}
impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Receiver").finish_non_exhaustive()
    }
}

impl<T> fmt::Debug for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("SendError").finish_non_exhaustive()
    }
}
impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("sending on a closed channel")
    }
}
impl<T> Error for SendError<T> {}

impl<T> fmt::Debug for TrySendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            TrySendError::Full(..) => f.debug_tuple("TrySendError::Full").finish_non_exhaustive(),
            TrySendError::Disconnected(..) => f.debug_tuple("TrySendError::Disconnected").finish_non_exhaustive(),
        }
    }
}
impl<T> fmt::Display for TrySendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            TrySendError::Full(..) => f.pad("sending on a full channel"),
            TrySendError::Disconnected(..) => f.pad("sending on a closed channel"),
        }
    }
}
impl<T> Error for TrySendError<T> {}
impl<T> From<SendError<T>> for TrySendError<T> {
    fn from(err: SendError<T>) -> TrySendError<T> {
        TrySendError::Disconnected(err.0)
    }
}

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("receiving on a closed channel")
    }
}
impl Error for RecvError {}

impl fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            TryRecvError::Empty => f.pad("receiving on an empty channel"),
            TryRecvError::Disconnected => f.pad("receiving on a closed channel"),
        }
    }
}
impl Error for TryRecvError {}
impl From<RecvError> for TryRecvError {
    fn from(_: RecvError) -> TryRecvError {
        TryRecvError::Disconnected
    }
}

impl fmt::Display for RecvTimeoutError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            RecvTimeoutError::Timeout => f.pad("timed out waiting on channel"),
            RecvTimeoutError::Disconnected => f.pad("channel is empty and sending half is closed"),
        }
    }
}
impl Error for RecvTimeoutError {}
impl From<RecvError> for RecvTimeoutError {
    fn from(_: RecvError) -> RecvTimeoutError {
        RecvTimeoutError::Disconnected
    }
}
