use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

#[derive(Clone, Debug)]
pub struct Ready<T> {
    value: Option<T>,
}

impl<T> Future for Ready<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<T> {
        Poll::Ready(self.value.take().expect("polling after completion"))
    }
}

impl<T> Unpin for Ready<T> {}

pub fn ready<T>(value: T) -> Ready<T> {
    Ready { value: Some(value) }
}
