// std-core differential test (core_diff.sh): Future/Poll/Context/Waker/Wake, as libs/futures uses them.
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Wake, Waker};

struct Count(AtomicUsize);
impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Twice {
    polls: u32,
}
impl Future for Twice {
    type Output = u32;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u32> {
        self.polls += 1;
        if self.polls < 2 {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(self.polls * 10)
        }
    }
}

static RAW_WAKES: AtomicUsize = AtomicUsize::new(0);
unsafe fn r_clone(p: *const ()) -> RawWaker {
    RawWaker::new(p, &VT)
}
unsafe fn r_wake(_p: *const ()) {
    RAW_WAKES.fetch_add(1, Ordering::SeqCst);
}
unsafe fn r_wake_by_ref(_p: *const ()) {
    RAW_WAKES.fetch_add(10, Ordering::SeqCst);
}
unsafe fn r_drop(_p: *const ()) {}
static VT: RawWakerVTable = RawWakerVTable::new(r_clone, r_wake, r_wake_by_ref, r_drop);

#[test]
fn a01_arc_wake() {
    let count = Arc::new(Count(AtomicUsize::new(0)));
    let waker: Waker = Waker::from(count.clone());
    let mut cx = Context::from_waker(&waker);
    let mut f = Twice { polls: 0 };
    let mut f = Pin::new(&mut f);
    let first = f.as_mut().poll(&mut cx);
    let second = f.as_mut().poll(&mut cx);
    println!("| {} {:?}", first.is_pending(), second);
    let w2 = waker.clone();
    w2.wake();
    println!("| wakes {} strong {}", count.0.load(Ordering::SeqCst), Arc::strong_count(&count));
    drop(waker);
    println!("| strong after drop {}", Arc::strong_count(&count));
}

#[test]
fn a02_raw_waker() {
    let w = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VT)) };
    w.wake_by_ref();
    let c = w.clone();
    c.wake();
    println!("| raw {}", RAW_WAKES.load(Ordering::SeqCst));
    println!("| map {:?}", Poll::Ready(3).map(|x| x + 1));
}
