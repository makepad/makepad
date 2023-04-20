use {
    crate::futures::Stream,
    std::{
        collections::VecDeque,
        error, fmt,
        pin::Pin,
        sync::{Arc, Mutex},
        task::{Context, Poll, Waker},
    },
};

#[derive(Debug)]
pub struct UnboundedSender<T> {
    channel: Arc<Mutex<UnboundedChannel<T>>>,
}

impl<T> UnboundedSender<T> {
    pub fn send(&self, message: T) -> Result<(), SendError<T>> {
        let mut channel = self.channel.lock().unwrap();
        if channel.is_closed {
            return Err(SendError(message));
        }
        channel.message_queue.push_back(message);
        if let Some(recv_task) = channel.recv_task.take() {
            recv_task.wake();
        }
        Ok(())
    }
}

impl<T> Clone for UnboundedSender<T> {
    fn clone(&self) -> Self {
        let mut channel = self.channel.lock().unwrap();
        channel.sender_count += 1;
        Self {
            channel: self.channel.clone(),
        }
    }
}

impl<T> Drop for UnboundedSender<T> {
    fn drop(&mut self) {
        let mut channel = self.channel.lock().unwrap();
        channel.sender_count -= 1;
        if channel.sender_count == 0 {
            if let Some(recv_task) = channel.recv_task.take() {
                recv_task.wake();
            }
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
pub struct UnboundedReceiver<T> {
    channel: Arc<Mutex<UnboundedChannel<T>>>,
}

impl<T> Drop for UnboundedReceiver<T> {
    fn drop(&mut self) {
        let mut channel = self.channel.lock().unwrap();
        channel.is_closed = true;
        channel.message_queue.clear();
    }
}

impl<T> Stream for UnboundedReceiver<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut channel = self.channel.lock().unwrap();
        match channel.message_queue.pop_front() {
            Some(message) => Poll::Ready(Some(message)),
            None => {
                if channel.sender_count == 0 {
                    Poll::Ready(None)
                } else {
                    channel.recv_task = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }
    }
}

#[derive(Debug)]
struct UnboundedChannel<T> {
    is_closed: bool,
    sender_count: usize,
    message_queue: VecDeque<T>,
    recv_task: Option<Waker>,
}

pub fn unbounded<T>() -> (UnboundedSender<T>, UnboundedReceiver<T>) {
    let channel = Arc::new(Mutex::new(UnboundedChannel {
        is_closed: false,
        sender_count: 1,
        message_queue: VecDeque::new(),
        recv_task: None,
    }));
    (
        UnboundedSender {
            channel: channel.clone(),
        },
        UnboundedReceiver { channel },
    )
}
