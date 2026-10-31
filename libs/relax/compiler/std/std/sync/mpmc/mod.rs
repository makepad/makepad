//! Port of real std's mpmc flavor dispatch (MIT/Apache-2.0).
mod array;
mod list;
mod zero;
mod counter;
mod context;
mod select;
mod waker;
mod utils;
mod error {
    pub use super::super::mpsc::{TrySendError, TryRecvError, RecvTimeoutError};
    pub enum SendTimeoutError<T> { Timeout(T), Disconnected(T) }
}
use error::*;
use crate::time::Instant;
pub(crate) enum Sender<T> {
    Array(counter::Sender<array::Channel<T>>),
    List(counter::Sender<list::Channel<T>>),
    Zero(counter::Sender<zero::Channel<T>>),
}
unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Sync for Sender<T> {}
pub(crate) enum Receiver<T> {
    Array(counter::Receiver<array::Channel<T>>),
    List(counter::Receiver<list::Channel<T>>),
    Zero(counter::Receiver<zero::Channel<T>>),
}
unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Sync for Receiver<T> {}
pub(crate) fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let (s, r) = counter::new(list::Channel::new());
    (Sender::List(s), Receiver::List(r))
}
pub(crate) fn sync_channel<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
    if cap == 0 {
        let (s, r) = counter::new(zero::Channel::new());
        (Sender::Zero(s), Receiver::Zero(r))
    } else {
        let (s, r) = counter::new(array::Channel::with_capacity(cap));
        (Sender::Array(s), Receiver::Array(r))
    }
}
impl<T> Sender<T> {
    pub(crate) fn send(&self, msg: T) -> Result<(), super::mpsc::SendError<T>> {
        let result = match self {
            Sender::Array(c) => c.send(msg, None),
            Sender::List(c) => c.send(msg, None),
            Sender::Zero(c) => c.send(msg, None),
        };
        result.map_err(|e| match e { SendTimeoutError::Disconnected(v) | SendTimeoutError::Timeout(v) => super::mpsc::SendError(v) })
    }
    pub(crate) fn try_send(&self, msg: T) -> Result<(), TrySendError<T>> {
        let result = match self {
            Sender::Array(c) => c.try_send(msg),
            Sender::List(c) => c.try_send(msg),
            Sender::Zero(c) => c.try_send(msg),
        };
        result
    }
}
impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        unsafe { match self {
            Sender::Array(c) => c.release(|c| c.disconnect_senders()),
            Sender::List(c) => c.release(|c| c.disconnect_senders()),
            Sender::Zero(c) => c.release(|c| c.disconnect()),
        } }
    }
}
impl<T> Receiver<T> {
    pub(crate) fn recv(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        let result = match self {
            Receiver::Array(c) => c.recv(deadline),
            Receiver::List(c) => c.recv(deadline),
            Receiver::Zero(c) => c.recv(deadline),
        };
        result
    }
    pub(crate) fn try_recv(&self, ) -> Result<T, TryRecvError> {
        let result = match self {
            Receiver::Array(c) => c.try_recv(),
            Receiver::List(c) => c.try_recv(),
            Receiver::Zero(c) => c.try_recv(),
        };
        result
    }
}
impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        unsafe { match self {
            Receiver::Array(c) => c.release(|c| c.disconnect_receivers()),
            Receiver::List(c) => c.release(|c| c.disconnect_receivers()),
            Receiver::Zero(c) => c.release(|c| c.disconnect()),
        } }
    }
}
impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        match self {
            Sender::Array(c) => Sender::Array(c.acquire()),
            Sender::List(c) => Sender::List(c.acquire()),
            Sender::Zero(c) => Sender::Zero(c.acquire()),
        }
    }
}
