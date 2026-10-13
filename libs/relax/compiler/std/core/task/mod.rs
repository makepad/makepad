//! core::task: Poll, Context, Waker, RawWaker, RawWakerVTable (and alloc's Wake trait).
//! makepad's executors (libs/futures) build wakers from RawWakerVTable and from Arc<impl Wake>.

use crate::fmt;
use crate::sync::Arc;

#[must_use]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[cfg_attr(not(relax_check), lang = "Poll")]
pub enum Poll<T> {
    #[cfg_attr(not(relax_check), lang = "Ready")]
    Ready(T),
    #[cfg_attr(not(relax_check), lang = "Pending")]
    Pending,
}

pub use Poll::{Pending, Ready};

impl<T> Poll<T> {
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Poll<U> {
        match self {
            Poll::Ready(t) => Poll::Ready(f(t)),
            Poll::Pending => Poll::Pending,
        }
    }
    pub const fn is_ready(&self) -> bool {
        matches!(*self, Poll::Ready(_))
    }
    pub const fn is_pending(&self) -> bool {
        !self.is_ready()
    }
}

impl<T, E> Poll<Result<T, E>> {
    pub fn map_ok<U, F: FnOnce(T) -> U>(self, f: F) -> Poll<Result<U, E>> {
        match self {
            Poll::Ready(Ok(t)) => Poll::Ready(Ok(f(t))),
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
    pub fn map_err<U, F: FnOnce(E) -> U>(self, f: F) -> Poll<Result<T, U>> {
        match self {
            Poll::Ready(Ok(t)) => Poll::Ready(Ok(t)),
            Poll::Ready(Err(e)) => Poll::Ready(Err(f(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T> From<T> for Poll<T> {
    fn from(t: T) -> Poll<T> {
        Poll::Ready(t)
    }
}

/// A raw waker: data pointer + vtable.
#[derive(PartialEq, Debug)]
pub struct RawWaker {
    data: *const (),
    vtable: &'static RawWakerVTable,
}

impl RawWaker {
    #[rustc_promotable]
    pub const fn new(data: *const (), vtable: &'static RawWakerVTable) -> RawWaker {
        RawWaker { data, vtable }
    }
    pub fn data(&self) -> *const () {
        self.data
    }
    pub fn vtable(&self) -> &'static RawWakerVTable {
        self.vtable
    }
}

#[derive(PartialEq, Copy, Clone, Debug)]
pub struct RawWakerVTable {
    clone: unsafe fn(*const ()) -> RawWaker,
    wake: unsafe fn(*const ()),
    wake_by_ref: unsafe fn(*const ()),
    drop: unsafe fn(*const ()),
}

impl RawWakerVTable {
    #[rustc_promotable]
    pub const fn new(
        clone: unsafe fn(*const ()) -> RawWaker,
        wake: unsafe fn(*const ()),
        wake_by_ref: unsafe fn(*const ()),
        drop: unsafe fn(*const ()),
    ) -> RawWakerVTable {
        RawWakerVTable { clone, wake, wake_by_ref, drop }
    }
}

pub struct Waker {
    waker: RawWaker,
}

unsafe impl Send for Waker {}
unsafe impl Sync for Waker {}

impl Waker {
    pub unsafe fn from_raw(waker: RawWaker) -> Waker {
        Waker { waker }
    }
    pub unsafe fn new(data: *const (), vtable: &'static RawWakerVTable) -> Waker {
        Waker { waker: RawWaker { data, vtable } }
    }
    pub fn wake(self) {
        let this = crate::mem::ManuallyDrop::new(self);
        unsafe { (this.waker.vtable.wake)(this.waker.data) };
    }
    pub fn wake_by_ref(&self) {
        unsafe { (self.waker.vtable.wake_by_ref)(self.waker.data) }
    }
    pub fn will_wake(&self, other: &Waker) -> bool {
        self.waker == other.waker
    }
    pub fn data(&self) -> *const () {
        self.waker.data
    }
    pub fn vtable(&self) -> &'static RawWakerVTable {
        self.waker.vtable
    }
    pub fn noop() -> &'static Waker {
        &NOOP_WAKER
    }
}

unsafe fn noop_clone(_p: *const ()) -> RawWaker {
    RawWaker::new(crate::ptr::null(), &NOOP_VTABLE)
}
unsafe fn noop(_p: *const ()) {}

static NOOP_VTABLE: RawWakerVTable = RawWakerVTable::new(noop_clone, noop, noop, noop);
static NOOP_WAKER: Waker = Waker { waker: RawWaker { data: 0 as *const (), vtable: &NOOP_VTABLE } };

impl Clone for Waker {
    fn clone(&self) -> Waker {
        Waker { waker: unsafe { (self.waker.vtable.clone)(self.waker.data) } }
    }
}

impl Drop for Waker {
    fn drop(&mut self) {
        unsafe { (self.waker.vtable.drop)(self.waker.data) }
    }
}

impl fmt::Debug for Waker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let vtable_ptr = self.waker.vtable as *const RawWakerVTable;
        f.debug_struct("Waker").field("data", &self.waker.data).field("vtable", &vtable_ptr).finish()
    }
}

/// The context of an asynchronous task.
#[cfg_attr(not(relax_check), lang = "Context")]
pub struct Context<'a> {
    waker: &'a Waker,
}

impl<'a> Context<'a> {
    pub const fn from_waker(waker: &'a Waker) -> Context<'a> {
        Context { waker }
    }
    pub const fn waker(&self) -> &'a Waker {
        self.waker
    }
}

impl<'a> fmt::Debug for Context<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Context").field("waker", &self.waker).finish()
    }
}

/// alloc::task::Wake: wakers from `Arc<W>`.
pub trait Wake {
    fn wake(self: Arc<Self>);
    fn wake_by_ref(self: &Arc<Self>) {
        self.clone().wake();
    }
}

impl<W: Wake + Send + Sync + 'static> From<Arc<W>> for Waker {
    fn from(waker: Arc<W>) -> Waker {
        unsafe { Waker::from_raw(raw_waker(waker)) }
    }
}

impl<W: Wake + Send + Sync + 'static> From<Arc<W>> for RawWaker {
    fn from(waker: Arc<W>) -> RawWaker {
        raw_waker(waker)
    }
}

fn raw_waker<W: Wake + Send + Sync + 'static>(waker: Arc<W>) -> RawWaker {
    unsafe fn clone_waker<W: Wake + Send + Sync + 'static>(waker: *const ()) -> RawWaker {
        Arc::<W>::increment_strong_count(waker as *const W);
        RawWaker::new(waker, arc_vtable::<W>())
    }
    unsafe fn wake<W: Wake + Send + Sync + 'static>(waker: *const ()) {
        let waker = Arc::from_raw(waker as *const W);
        <W as Wake>::wake(waker);
    }
    unsafe fn wake_by_ref<W: Wake + Send + Sync + 'static>(waker: *const ()) {
        let waker = crate::mem::ManuallyDrop::new(Arc::from_raw(waker as *const W));
        <W as Wake>::wake_by_ref(&waker);
    }
    unsafe fn drop_waker<W: Wake + Send + Sync + 'static>(waker: *const ()) {
        Arc::<W>::decrement_strong_count(waker as *const W);
    }
    fn arc_vtable<W: Wake + Send + Sync + 'static>() -> &'static RawWakerVTable {
        // one vtable per W, promoted to a static by the compiler as in alloc::task
        &RawWakerVTable { clone: clone_waker::<W>, wake: wake::<W>, wake_by_ref: wake_by_ref::<W>, drop: drop_waker::<W> }
    }
    RawWaker::new(Arc::into_raw(waker) as *const (), arc_vtable::<W>())
}

// ported from library/core/src/task/poll.rs:233-291 @8d1a764
impl<T, E> crate::ops::Try for Poll<Result<T, E>> {
    type Output = Poll<T>;
    type Residual = Result<crate::convert::Infallible, E>;

    fn from_output(c: Self::Output) -> Self {
        c.map(Ok)
    }

    fn branch(self) -> crate::ops::ControlFlow<Self::Residual, Self::Output> {
        match self {
            Poll::Ready(Ok(x)) => crate::ops::ControlFlow::Continue(Poll::Ready(x)),
            Poll::Ready(Err(e)) => crate::ops::ControlFlow::Break(Err(e)),
            Poll::Pending => crate::ops::ControlFlow::Continue(Poll::Pending),
        }
    }
}

impl<T, E, F: From<E>> crate::ops::FromResidual<Result<crate::convert::Infallible, E>> for Poll<Result<T, F>> {
    fn from_residual(x: Result<crate::convert::Infallible, E>) -> Self {
        match x {
            Err(e) => Poll::Ready(Err(From::from(e))),
            Ok(never) => match never {},
        }
    }
}

impl<T, E> crate::ops::Try for Poll<Option<Result<T, E>>> {
    type Output = Poll<Option<T>>;
    type Residual = Result<crate::convert::Infallible, E>;

    fn from_output(c: Self::Output) -> Self {
        c.map(|x| x.map(Ok))
    }

    fn branch(self) -> crate::ops::ControlFlow<Self::Residual, Self::Output> {
        match self {
            Poll::Ready(Some(Ok(x))) => crate::ops::ControlFlow::Continue(Poll::Ready(Some(x))),
            Poll::Ready(Some(Err(e))) => crate::ops::ControlFlow::Break(Err(e)),
            Poll::Ready(None) => crate::ops::ControlFlow::Continue(Poll::Ready(None)),
            Poll::Pending => crate::ops::ControlFlow::Continue(Poll::Pending),
        }
    }
}

impl<T, E, F: From<E>> crate::ops::FromResidual<Result<crate::convert::Infallible, E>> for Poll<Option<Result<T, F>>> {
    fn from_residual(x: Result<crate::convert::Infallible, E>) -> Self {
        match x {
            Err(e) => Poll::Ready(Some(Err(From::from(e)))),
            Ok(never) => match never {},
        }
    }
}