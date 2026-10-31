use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker { noop_raw() }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VT)
}

fn block_on<F: Future>(mut future: F) -> F::Output {
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut future = unsafe { Pin::new_unchecked(&mut future) };
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) { return value; }
    }
}

#[test]
fn integer_collect() {
    let v: Vec<_> = (0..16).map(|i| i * 2).collect();
    assert_eq!(v.len(), 16);
    assert_eq!(v[15], 30);
}

async fn collect_async() {
    let v: Vec<_> = (0..16).map(|i| i * 2).collect();
    assert_eq!(v.len(), 16);
    assert_eq!(v[15], 30);
}

#[test]
fn async_integer_collect() { block_on(collect_async()); }

struct PreparedBox<T>(std::marker::PhantomData<T>);
impl<T> PreparedBox<T> {
    fn new() -> Self { Self(std::marker::PhantomData) }
    fn initialize(self, value: T) -> Box<T> { Box::new(value) }
}

struct RetirementBatch<P> { items: [Option<P>; 64] }
struct TaskHandle<T>(Option<T>);
impl<T> TaskHandle<T> {
    fn try_take(&mut self) -> Option<Result<T, ()>> { self.0.take().map(Ok) }
}

struct TaskSlot;
impl TaskSlot {
    fn submit_internal_named<R: Send + 'static, F: FnOnce() -> R + Send + 'static>(
        self, _name: &str, task: F,
    ) -> TaskHandle<R> {
        TaskHandle(Some(task()))
    }
}

struct RetirementBatches<P: Send + 'static> {
    available: Vec<Box<RetirementBatch<P>>>,
    preparing: Option<TaskHandle<Vec<Box<RetirementBatch<P>>>>>,
    initialized: bool,
}

impl<P: Send + 'static> RetirementBatches<P> {
    // The task field supplies collect's expected type through the outer closure, as in
    // draw_list.rs's retirement poll. The integer range and inner closure stay unannotated.
    // rustc_hir_typeck/src/method/probe.rs:2205 and
    // rustc_type_ir/src/relate/solver_relating.rs:198 (@8d1a764) preserve pending obligations.
    async fn poll(&mut self) -> bool {
        if let Some(result) = self.preparing.as_mut().and_then(TaskHandle::try_take) {
            self.preparing = None;
            if let Ok(available) = result {
                self.available = available;
                self.initialized = true;
            }
        }
        if !self.initialized {
            if self.preparing.is_none() {
                self.preparing = Some(TaskSlot.submit_internal_named("retirement batch storage", || {
                    (0..16)
                        .map(|_| {
                            PreparedBox::new().initialize(RetirementBatch {
                                items: std::array::from_fn(|_| None),
                            })
                        })
                        .collect()
                }));
            }
            return false;
        }
        true
    }
}

#[test]
fn generic_async_poll_collect_from_enclosing_closure() {
    let mut batches = RetirementBatches::<u32> {
        available: Vec::new(), preparing: None, initialized: false,
    };
    assert!(!block_on(batches.poll()));
    assert!(block_on(batches.poll()));
    assert_eq!(batches.available.len(), 16);
    for batch in &batches.available {
        assert_eq!(batch.items.len(), 64);
        for item in &batch.items { assert!(item.is_none()); }
    }
}

struct FloatRange<T> { next: T, end: T }
impl Iterator for FloatRange<f32> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.next >= self.end { return None; }
        let value = self.next;
        self.next += 1.0;
        Some(value)
    }
}
impl Iterator for FloatRange<f64> {
    type Item = f64;
    fn next(&mut self) -> Option<f64> {
        if self.next >= self.end { return None; }
        let value = self.next;
        self.next += 1.0;
        Some(value)
    }
}

#[test]
fn float_collect() {
    let v: Vec<_> = FloatRange { next: 0.0, end: 4.0 }.map(|i| i * 2.0).collect();
    assert_eq!(v.len(), 4);
    assert_eq!(v[3], 6.0);
}

trait Value { fn value(&self) -> u32 { 7 } }
struct Deferred<T>(Option<T>);
impl Value for Deferred<bool> {}
impl Value for Deferred<char> {}

#[test]
fn general_inference_collects_candidate_before_arguments_resolve() {
    let mut value = Deferred(None);
    assert_eq!(value.value(), 7);
    value.0 = Some(true);
    assert_eq!(value.value(), 7);
}
