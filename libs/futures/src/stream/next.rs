use {
    super::Stream,
    std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll},
    },
};

#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Next<'a, S>
where
    S: ?Sized,
{
    stream: &'a mut S,
}

impl<S> Unpin for Next<'_, S> where S: Unpin + ?Sized {}

impl<'a, S> Next<'a, S>
where
    S: ?Sized,
{
    pub(super) fn new(stream: &'a mut S) -> Self {
        Self { stream }
    }
}

impl<S> Future for Next<'_, S>
where
    S: Unpin + Stream + ?Sized,
{
    type Output = Option<S::Item>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut *self.stream).poll_next(cx)
    }
}
