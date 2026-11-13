// An async fn moves one field out of a by-value parameter (`req.layers.into_iter()`) after an
// await that sits on a path not taken; at the return only the fields still in place drop
// (rustc's drop elaboration of a coroutine's saved locals). A slicer's live CPU trace
// double-freed `request.layers` this way.
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker { noop_raw() }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VT)
}

fn block_on<F: Future>(mut f: F) -> F::Output {
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut f = unsafe { Pin::new_unchecked(&mut f) };
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

struct Yield(bool);
impl Future for Yield {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.0 { Poll::Ready(()) } else { self.0 = true; Poll::Pending }
    }
}

thread_local! { static LOG: RefCell<Vec<String>> = RefCell::new(Vec::new()); }
fn log(s: String) { LOG.with(|l| l.borrow_mut().push(s)); }
fn take() -> String { LOG.with(|l| std::mem::take(&mut *l.borrow_mut())).join(",") }
struct D(String);
impl Drop for D { fn drop(&mut self) { log(self.0.clone()); } }

struct Req { name: D, layers: Vec<(usize, D)>, field: Option<D> }

fn req() -> Req {
    Req { name: D("name".into()), layers: vec![(1, D("l1".into())), (2, D("l2".into()))], field: Some(D("field".into())) }
}

#[derive(Clone, Copy, PartialEq)]
enum Engine { Gpu, Cpu }

impl Engine {
    async fn trace(self, request: Req, decline: bool) -> Result<Vec<(usize, usize)>, String> {
        if self == Engine::Gpu {
            Yield(false).await;
            if !decline {
                return Ok(Vec::new());
            }
        }
        let source = request.field.as_ref().map(|f| f.0.len()).ok_or("no field")?;
        let lens: Vec<usize> = (0..request.layers.len()).map(|i| request.layers[i].1 .0.len() + source).collect();
        Ok(request.layers.into_iter().map(|(k, _)| k).zip(lens).collect())
    }
}

#[test]
fn the_moved_field_drops_once_on_the_cpu_path() {
    let r = block_on(Engine::Cpu.trace(req(), false));
    assert_eq!(r, Ok(vec![(1, 7), (2, 7)]));
    assert_eq!(take(), "l1,l2,name,field");
}

#[test]
fn the_moved_field_drops_once_after_a_declined_await() {
    let r = block_on(Engine::Gpu.trace(req(), true));
    assert_eq!(r, Ok(vec![(1, 7), (2, 7)]));
    assert_eq!(take(), "l1,l2,name,field");
}

#[test]
fn an_early_return_drops_the_whole_parameter() {
    let r = block_on(Engine::Gpu.trace(req(), false));
    assert_eq!(r, Ok(Vec::new()));
    assert_eq!(take(), "name,l1,l2,field");
}

#[test]
fn a_suspended_partial_move_drops_locals_before_parameters() {
    async fn run(_first: D, request: Req, _last: D) {
        let _body = D("body".into());
        let _layers = request.layers.into_iter();
        Yield(false).await;
    }

    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut f = Box::pin(run(D("first".into()), req(), D("last".into())));
    assert!(matches!(f.as_mut().poll(&mut cx), Poll::Pending));
    assert_eq!(take(), "", "suspension keeps the captures and locals live");
    drop(f);
    assert_eq!(take(), "l1,l2,body,last,name,field,first", "cancellation drops only owned fields in scope order");

    block_on(run(D("first".into()), req(), D("last".into())));
    assert_eq!(take(), "l1,l2,body,last,name,field,first", "completion drops locals before parameters in reverse parameter order");

    drop(run(D("first".into()), req(), D("last".into())));
    assert_eq!(take(), "first,name,l1,l2,field,last", "an unpolled future still owns whole captures");
}
