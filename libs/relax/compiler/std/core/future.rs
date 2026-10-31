//! core::future: the Future trait and small helpers (ready, pending, poll_fn, IntoFuture).

use crate::pin::Pin;
use crate::task::{Context, Poll};
use crate::ptr::NonNull;

/// The `Context` an async body is resumed with, passed as a raw pointer (coroutines cannot
/// implement `for<'a, 'b> Coroutine<&'a mut Context<'b>>`; a raw pointer also keeps futures
/// `Send` and `Sync`).
#[cfg_attr(not(relax_check), lang = "ResumeTy")]
#[derive(Debug, Copy, Clone)]
pub struct ResumeTy(NonNull<Context<'static>>);

unsafe impl Send for ResumeTy {}
unsafe impl Sync for ResumeTy {}

/// The `Context` behind a `ResumeTy` (what the `.await` desugaring polls with).
#[cfg_attr(not(relax_check), lang = "get_context")]
pub unsafe fn get_context<'a, 'b>(cx: ResumeTy) -> &'a mut Context<'b> {
    unsafe { &mut *cx.0.as_ptr().cast() }
}

#[must_use = "futures do nothing unless you `.await` or poll them"]
#[cfg_attr(not(relax_check), lang = "future_trait")]
pub trait Future {
    #[cfg_attr(not(relax_check), lang = "future_output")]
    type Output;
    #[cfg_attr(not(relax_check), lang = "poll")]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output>;
}

impl<'a, F: Future + Unpin + ?Sized> Future for &'a mut F {
    type Output = F::Output;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        F::poll(Pin::new(&mut **self), cx)
    }
}

impl<P> Future for Pin<P>
where
    P: crate::ops::DerefMut,
    P::Target: Future,
{
    type Output = <<P as crate::ops::Deref>::Target as Future>::Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe { self.get_unchecked_mut().as_mut().poll(cx) }
    }
}

impl<F: Future + Unpin + ?Sized> Future for crate::boxed::Box<F> {
    type Output = F::Output;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        F::poll(Pin::new(&mut **self), cx)
    }
}

pub trait IntoFuture {
    type Output;
    type IntoFuture: Future<Output = Self::Output>;
    #[cfg_attr(not(relax_check), lang = "into_future")]
    fn into_future(self) -> Self::IntoFuture;
}

impl<F: Future> IntoFuture for F {
    type Output = F::Output;
    type IntoFuture = F;
    fn into_future(self) -> F {
        self
    }
}

#[derive(Debug, Clone)]
pub struct Ready<T>(Option<T>);

impl<T> Unpin for Ready<T> {}

impl<T> Future for Ready<T> {
    type Output = T;
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<T> {
        Poll::Ready(self.0.take().expect("`Ready` polled after completion"))
    }
}

pub fn ready<T>(t: T) -> Ready<T> {
    Ready(Some(t))
}

#[derive(Debug)]
pub struct Pending<T> {
    _m: crate::marker::PhantomData<fn() -> T>,
}

pub fn pending<T>() -> Pending<T> {
    Pending { _m: crate::marker::PhantomData }
}

impl<T> Future for Pending<T> {
    type Output = T;
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<T> {
        Poll::Pending
    }
}

pub struct PollFn<F> {
    f: F,
}

impl<F: Unpin> Unpin for PollFn<F> {}

pub fn poll_fn<T, F: FnMut(&mut Context<'_>) -> Poll<T>>(f: F) -> PollFn<F> {
    PollFn { f }
}

impl<T, F: FnMut(&mut Context<'_>) -> Poll<T>> Future for PollFn<F> {
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        unsafe { (self.get_unchecked_mut().f)(cx) }
    }
}
