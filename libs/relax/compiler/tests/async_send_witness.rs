use std::cell::Cell;
use std::future::{poll_fn, Future};
use std::pin::{pin, Pin};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

fn assert_send<T: Send>(_: &T) {}
async fn pause() { std::future::ready(()).await; }

// rustc_mir_transform/src/coroutine/layout.rs::locals_live_across_suspend_points:
// a statement temporary and a local whose block ended have no storage at the yield.
#[test]
fn mutex_guard_temporary_ends_at_semicolon() {
    let rooms = Arc::new(Mutex::new(vec![1u32, 2]));
    let future = async move {
        rooms.lock().unwrap().clear();
        pause().await;
        assert!(rooms.lock().unwrap().is_empty());
    };
    assert_send(&future);
}

#[test]
fn non_send_block_ends_before_await() {
    let future = async {
        { let rc = Rc::new(42u32); assert_eq!(*rc, 42); }
        pause().await;
    };
    assert_send(&future);
}

async fn inner(value: String) -> (usize, String) {
    pause().await;
    (value.len(), value)
}
async fn middle(value: String) -> usize {
    let (length, text) = inner(value).await;
    pause().await;
    assert_eq!(text.len(), length);
    length
}
fn outer(value: String) -> impl Future<Output = usize> {
    async move { middle(value).await }
}

// rustc_next_trait_solver/src/solve/trait_goals.rs::consider_auto_trait_candidate_for_opaque_ty:
// auto traits of an opaque future leak through its hidden type, including nested opaques.
#[test]
fn nested_async_outputs_are_used() {
    assert_send(&outer(String::from("rooms")));
}

trait Stream {
    type Item;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>>;
}
struct Iter<I>(I);
impl<I: Iterator + Unpin> Stream for Iter<I> {
    type Item = I::Item;
    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().0.next())
    }
}
async fn next<S: Stream + ?Sized>(mut stream: Pin<&mut S>) -> Option<S::Item> {
    poll_fn(|cx| stream.as_mut().poll_next(cx)).await
}

#[test]
fn pinned_send_stream_need_not_be_sync() {
    let future = async {
        let mut stream = pin!(Iter(vec![Cell::new(1u32), Cell::new(2)].into_iter()));
        let first = next(stream.as_mut()).await.unwrap();
        pause().await;
        assert_eq!(first.get(), 1);
        assert_eq!(next(stream.as_mut()).await.unwrap().get(), 2);
    };
    assert_send(&future);
}

#[test]
fn iterator_adapters_live_across_await() {
    let future = async {
        let mut batch = vec![1u32, 2, 3].into_iter().map(|n| n + 1).filter(|n| *n > 2).peekable();
        assert_eq!(batch.peek(), Some(&3));
        pause().await;
        assert_eq!(batch.next(), Some(3));
        pause().await;
        assert_eq!(batch.next(), Some(4));
    };
    assert_send(&future);
}

struct Map<S, F> { stream: S, map: F }
impl<S: Stream + Unpin, F: FnMut(S::Item) -> T + Unpin, T> Stream for Map<S, F> {
    type Item = T;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let this = self.get_mut();
        match Pin::new(&mut this.stream).poll_next(cx) {
            Poll::Ready(Some(value)) => Poll::Ready(Some((this.map)(value))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[test]
fn mapped_async_futures_are_buffered_across_await() {
    let future = async {
        let mut stream = pin!(Map {
            stream: Iter(vec![1u32, 2, 3].into_iter()),
            map: |value| async move { pause().await; value + 1 },
        });
        let mut buffered = Vec::new();
        while let Some(task) = next(stream.as_mut()).await {
            buffered.push(task);
            pause().await;
        }
        let mut total = 0;
        for task in buffered { total += task.await; }
        assert_eq!(total, 9);
    };
    assert_send(&future);
}

struct SendOnly(*const ());
// This fixture never dereferences its pointer.
unsafe impl Send for SendOnly {}

#[test]
fn explicit_send_impl_overrides_raw_pointer_field() {
    let future = async {
        let value = SendOnly(std::ptr::null());
        pause().await;
        assert!(value.0.is_null());
    };
    assert_send(&future);
}

async fn generic<T: Send>(value: T) -> T { pause().await; value }

#[test]
fn generic_witness_keeps_argument_regions() {
    let mut value = 3u32;
    let future = generic(&mut value);
    assert_send(&future);
}

// Robrix logout_state_machine.rs: `self.transition_to(state, format!("..{}", e), 0).await?` --
// format!'s Argument array (not Send: it holds pointers) dies inside the macro's block (alloc
// `{ let res = fmt::format(format_args!(..)); res }`), so the future stays Send.
struct Machine;
impl Machine {
    async fn transition_to(&self, _state: u8, msg: String, _pct: u8) -> Result<usize, ()> {
        std::future::ready(()).await;
        Ok(msg.len())
    }
    async fn execute(&self, e: &str) -> Result<usize, ()> {
        let n = self.transition_to(1, format!("Precheck failed: {}", e), 0).await?;
        Ok(n + self.transition_to(2, format!("{e}{e}"), 0).await?)
    }
}

#[test]
fn format_temporaries_end_inside_the_macro() {
    fn assert_send<T: Send>(_: &T) {}
    let m = Machine;
    let f = m.execute("disk");
    assert_send(&f);
    let mut f = Box::pin(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let r = loop {
        if let std::task::Poll::Ready(r) = f.as_mut().poll(&mut cx) { break r; }
    };
    assert_eq!(r, Ok(21 + 8));
}

// story cut generate.rs: `input != format!(..)` with `input: &str` (PartialEq<String> for &str):
// the caller's expected type must not reach format!'s inner block
#[test]
fn format_compared_with_str() {
    let input: &str = "00000000000000ff";
    assert!(!(input != format!("{:016x}", 255u64)));
    let s: &str = &format!("{}-{}", 1, 2);
    assert_eq!(s, "1-2");
}
