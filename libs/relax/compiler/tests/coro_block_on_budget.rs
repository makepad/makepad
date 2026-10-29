// Tokio 1.53.1's budget/block_on/yield_now path, with a std-only parker and scheduler.
use std::cell::{Cell, RefCell};
use std::future::{poll_fn, Future};
use std::marker::PhantomData;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{ready, Poll, Wake, Waker};
use std::thread::AccessError;

macro_rules! pin {
    ($($x:ident),*) => { $(
        let mut $x = $x;
        #[allow(unused_mut)]
        let mut $x = unsafe { Pin::new_unchecked(&mut $x) };
    )* };
}

mod coop {
    use super::context;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Budget(pub Option<u8>);

    impl Budget {
        const fn initial() -> Budget { Budget(Some(128)) }
        pub const fn unconstrained() -> Budget { Budget(None) }
    }

    #[inline(always)]
    pub fn budget<R>(f: impl FnOnce() -> R) -> R {
        with_budget(Budget::initial(), f)
    }

    #[inline(always)]
    fn with_budget<R>(budget: Budget, f: impl FnOnce() -> R) -> R {
        struct ResetGuard {
            prev: Budget,
        }

        impl Drop for ResetGuard {
            fn drop(&mut self) {
                let _ = context::budget(|cell| {
                    cell.set(self.prev);
                });
            }
        }

        #[allow(unused_variables)]
        let maybe_guard = context::budget(|cell| {
            let prev = cell.get();
            cell.set(budget);

            ResetGuard { prev }
        });

        f()
    }
}

mod context {
    use super::*;

    struct Context {
        thread_id: Cell<Option<usize>>,
        current: RefCell<Option<Arc<usize>>>,
        scheduler: Cell<Option<usize>>,
        current_task_id: Cell<Option<usize>>,
        runtime: Cell<bool>,
        rng: Cell<Option<u64>>,
        budget: Cell<coop::Budget>,
    }

    thread_local! {
        static CONTEXT: Context = const {
            Context {
                thread_id: Cell::new(Some(17)),
                current: RefCell::new(None),
                scheduler: Cell::new(Some(29)),
                current_task_id: Cell::new(Some(31)),
                runtime: Cell::new(true),
                rng: Cell::new(Some(37)),
                budget: Cell::new(coop::Budget::unconstrained()),
            }
        };
        static DEFERRED: Defer = Defer::new();
    }

    pub fn budget<R>(f: impl FnOnce(&Cell<coop::Budget>) -> R) -> Result<R, AccessError> {
        CONTEXT.try_with(|ctx| f(&ctx.budget))
    }

    pub fn defer(waker: &Waker) { DEFERRED.with(|d| d.defer(waker)); }
    pub fn wake_deferred() { DEFERRED.with(|d| d.wake()); }

    pub fn check() {
        CONTEXT.with(|ctx| {
            assert_eq!(ctx.thread_id.get(), Some(17));
            assert!(ctx.current.borrow().is_none());
            assert_eq!(ctx.scheduler.get(), Some(29));
            assert_eq!(ctx.current_task_id.get(), Some(31));
            assert!(ctx.runtime.get());
            assert_eq!(ctx.rng.get(), Some(37));
        });
        DEFERRED.with(|d| assert!(d.deferred.borrow().is_empty()));
    }
}

struct Defer {
    deferred: RefCell<Vec<Waker>>,
}

impl Defer {
    fn new() -> Defer { Defer { deferred: RefCell::default() } }

    fn defer(&self, waker: &Waker) {
        let mut deferred = self.deferred.borrow_mut();
        if let Some(last) = deferred.last() {
            if last.will_wake(waker) { return; }
        }
        deferred.push(waker.clone());
    }

    fn wake(&self) {
        while let Some(waker) = self.deferred.borrow_mut().pop() {
            waker.wake();
        }
    }
}

struct Notify(AtomicUsize);
impl Wake for Notify {
    fn wake(self: Arc<Self>) { self.0.fetch_add(1, Ordering::SeqCst); }
}

thread_local! {
    static CURRENT_PARKER: (Cell<usize>, Arc<Notify>) =
        (Cell::new(0), Arc::new(Notify(AtomicUsize::new(0))));
}

struct CachedParkThread {
    _anchor: PhantomData<Rc<()>>,
}

impl CachedParkThread {
    fn new() -> CachedParkThread { CachedParkThread { _anchor: PhantomData } }
    fn waker(&self) -> Result<Waker, AccessError> {
        CURRENT_PARKER.try_with(|p| Waker::from(p.1.clone()))
    }
    fn park(&mut self) {
        CURRENT_PARKER.with(|p| p.0.set(p.0.get() + 1));
        context::wake_deferred();
    }

    fn block_on<F: Future>(&mut self, f: F) -> Result<F::Output, AccessError> {
        use std::task::Context;
        use std::task::Poll::Ready;

        let waker = self.waker()?;
        let mut cx = Context::from_waker(&waker);

        pin!(f);

        loop {
            // Keep the call in the pattern: a local/probe changes scalar materialization.
            if let Ready(v) = coop::budget(|| f.as_mut().poll(&mut cx)) {
                return Ok(v);
            }

            self.park();
        }
    }
}

struct BlockingRegionGuard {
    _p: PhantomData<*mut ()>,
}

impl BlockingRegionGuard {
    fn new() -> BlockingRegionGuard { BlockingRegionGuard { _p: PhantomData } }
    fn block_on<F>(&mut self, f: F) -> Result<F::Output, AccessError>
    where
        F: Future,
    {
        let mut park = CachedParkThread::new();
        park.block_on(f)
    }
}

#[inline(always)]
fn trace_leaf() -> Poll<()> { Poll::Ready(()) }

async fn yield_now() {
    let mut yielded = false;
    poll_fn(|cx| {
        ready!(trace_leaf());

        if yielded {
            return Poll::Ready(());
        }

        yielded = true;
        context::defer(cx.waker());

        Poll::Pending
    })
    .await
}

#[test]
fn block_on_yield_with_budget_and_deferred_waker() {
    let previous = coop::Budget(Some(7));
    context::budget(|cell| cell.set(previous)).unwrap();
    let mut guard = BlockingRegionGuard::new();
    guard.block_on(yield_now()).unwrap();
    assert_eq!(context::budget(|cell| cell.get()).unwrap(), previous);
    context::check();
    CURRENT_PARKER.with(|p| {
        assert_eq!(p.0.get(), 1);
        assert_eq!(p.1.0.load(Ordering::SeqCst), 1);
    });
}
