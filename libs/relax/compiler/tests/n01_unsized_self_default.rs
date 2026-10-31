// A trait's `Self` is `?Sized`: a provided method passing `&mut Self` to a
// constructor of a `?Sized`-generic struct needs no `Self: Sized`, in the
// generic body and in every instance (makepad futures' `Stream::next`).
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

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
    fn new(stream: &'a mut S) -> Self {
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

pub trait Stream {
    type Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>>;

    fn next(&mut self) -> Next<'_, Self>
    where
        Self: Unpin,
    {
        Next::new(self)
    }
}

struct Countdown(u32);

impl Stream for Countdown {
    type Item = u32;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<u32>> {
        if self.0 == 0 {
            return Poll::Ready(None);
        }
        self.0 -= 1;
        Poll::Ready(Some(self.0))
    }
}

struct Queue<T>(Vec<T>);

impl<T: Unpin> Stream for Queue<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<T>> {
        Poll::Ready(self.0.pop())
    }
}

fn poll_once<F: Future + Unpin>(future: &mut F) -> Poll<F::Output> {
    let mut cx = Context::from_waker(Waker::noop());
    Pin::new(future).poll(&mut cx)
}

fn drain<S: Stream + Unpin>(stream: &mut S) -> Vec<S::Item> {
    let mut out = Vec::new();
    while let Poll::Ready(Some(item)) = poll_once(&mut stream.next()) {
        out.push(item);
    }
    out
}

#[test]
fn unsized_self_provided_method_instances() {
    let mut countdown = Countdown(3);
    assert_eq!(drain(&mut countdown), vec![2, 1, 0]);
    let mut queue = Queue(vec!["a", "b"]);
    assert_eq!(drain(&mut queue), vec!["b", "a"]);
    let mut queue = Queue(vec![1u8]);
    let mut next = queue.next();
    assert_eq!(poll_once(&mut next), Poll::Ready(Some(1)));
}
