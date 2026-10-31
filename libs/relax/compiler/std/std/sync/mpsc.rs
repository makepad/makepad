//! std::sync::mpsc wrappers over a port of real std's mpmc channels (MIT/Apache-2.0).
use core::error::Error;
use core::fmt;
use core::time::Duration;
use crate::time::Instant;
use super::mpmc;

pub struct Sender<T> { chan: mpmc::Sender<T> }
pub struct SyncSender<T> { chan: mpmc::Sender<T> }
pub struct Receiver<T> {
    chan: mpmc::Receiver<T>,
    // A receiver may move between threads but may not be shared.
    marker: core::marker::PhantomData<core::cell::Cell<()>>,
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


pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let (s, r) = mpmc::channel();
    (Sender { chan: s }, Receiver { chan: r, marker: core::marker::PhantomData })
}
pub fn sync_channel<T>(bound: usize) -> (SyncSender<T>, Receiver<T>) {
    let (s, r) = mpmc::sync_channel(bound);
    (SyncSender { chan: s }, Receiver { chan: r, marker: core::marker::PhantomData })
}
impl<T> Sender<T> {
    pub fn send(&self, t: T) -> Result<(), SendError<T>> { self.chan.send(t) }
}
impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self { Sender { chan: self.chan.clone() } }
}
impl<T> SyncSender<T> {
    pub fn send(&self, t: T) -> Result<(), SendError<T>> { self.chan.send(t) }
    pub fn try_send(&self, t: T) -> Result<(), TrySendError<T>> { self.chan.try_send(t) }
}
impl<T> Clone for SyncSender<T> {
    fn clone(&self) -> Self { SyncSender { chan: self.chan.clone() } }
}
impl<T> Receiver<T> {
    pub fn try_recv(&self) -> Result<T, TryRecvError> { self.chan.try_recv() }
    pub fn recv(&self) -> Result<T, RecvError> { self.chan.recv(None).map_err(|_| RecvError) }
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        self.chan.recv(Instant::now().checked_add(timeout))
    }
    pub fn recv_deadline(&self, deadline: Instant) -> Result<T, RecvTimeoutError> {
        self.chan.recv(Some(deadline))
    }
    pub fn iter(&self) -> Iter<'_, T> { Iter { rx: self } }
    pub fn try_iter(&self) -> TryIter<'_, T> { TryIter { rx: self } }
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
