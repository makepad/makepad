use std::{
    error, fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
};

#[derive(Clone, Debug)]
pub struct Sender<T> {
    channel: Arc<Mutex<Channel<T>>>,
}

impl<T> Sender<T> {
    pub fn send(self, message: T) -> Result<(), SendError<T>> {
        let mut channel = self.channel.lock().unwrap();
        if channel.is_complete {
            return Err(SendError(message));
        }
        channel.message = Some(message);
        if let Some(recv_task) = channel.recv_task.take() {
            recv_task.wake();
        }
        Ok(())
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut channel = self.channel.lock().unwrap();
        channel.is_complete = true;
        if let Some(recv_task) = channel.recv_task.take() {
            recv_task.wake();
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SendError<T>(pub T);

impl<T> error::Error for SendError<T> {}

impl<T> fmt::Debug for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendError").finish_non_exhaustive()
    }
}

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sending on a closed channel")
    }
}

#[derive(Debug)]
pub struct Receiver<T> {
    channel: Arc<Mutex<Channel<T>>>,
}

impl<T> Future for Receiver<T> {
    type Output = Result<T, RecvError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut channel = self.channel.lock().unwrap();
        match channel.message.take() {
            Some(message) => Poll::Ready(Ok(message)),
            None => {
                if channel.is_complete {
                    Poll::Ready(Err(RecvError { _priv: () }))
                } else {
                    channel.recv_task = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let channel = self.channel.lock().unwrap();
        channel.is_complete = true;
        channel.message = None;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecvError {
    _priv: (),
}

impl error::Error for RecvError {}

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "receiving from a closed channel")
    }
}

#[derive(Debug)]
struct Channel<T> {
    is_complete: bool,
    message: Option<T>,
    recv_task: Option<Waker>,
}

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let channel = Arc::new(Mutex::new(Channel {
        is_complete: false,
        message: None,
        recv_task: None,
    }));
    (
        Sender {
            channel: channel.clone(),
        },
        Receiver { channel },
    )
}
