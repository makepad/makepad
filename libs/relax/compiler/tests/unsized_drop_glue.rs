// Public ptr::drop_in_place resolves to generated drop glue. Its ABI must
// retain the metadata needed to drop dyn values, slices and unsized tails.
use std::mem::ManuallyDrop;
use std::rc::Rc;
use std::sync::Arc;

#[test]
fn shared_dyn_closure_releases_capture() {
    let shared = Arc::new(7usize);
    let captured = shared.clone();
    let job: Arc<dyn Fn(usize) -> usize + Send + Sync> =
        Arc::new(move |i| *captured + i);
    let worker = job.clone();
    assert_eq!(worker(2), 9);
    drop(worker);
    drop(job);
    assert_eq!(Arc::try_unwrap(shared), Ok(7));

    let shared = Rc::new(11usize);
    let captured = shared.clone();
    let job: Rc<dyn Fn() -> usize> = Rc::new(move || *captured);
    assert_eq!(job(), 11);
    drop(job);
    assert_eq!(Rc::try_unwrap(shared), Ok(11));
}

#[test]
fn explicit_dyn_drop_releases_capture() {
    let shared = Arc::new(7usize);
    let captured = shared.clone();
    let mut job = ManuallyDrop::new(move || *captured);
    let ptr: *mut dyn Fn() -> usize = &mut *job;
    unsafe { std::ptr::drop_in_place(ptr) };
    assert_eq!(Arc::try_unwrap(shared), Ok(7));
}

#[test]
fn slice_drop_releases_every_element() {
    let shared = Arc::new(7usize);
    let mut values = ManuallyDrop::new([shared.clone(), shared.clone()]);
    let ptr: *mut [Arc<usize>] = &mut values[..];
    unsafe { std::ptr::drop_in_place(ptr) };
    assert_eq!(Arc::try_unwrap(shared), Ok(7));
}

#[test]
fn unsized_tail_drops_header_and_tail() {
    struct Tail<T: ?Sized> {
        _header: Arc<usize>,
        _tail: T,
    }
    let shared = Arc::new(7usize);
    let captured = shared.clone();
    let value: Arc<Tail<dyn Fn() -> usize>> = Arc::new(Tail {
        _header: shared.clone(),
        _tail: move || *captured,
    });
    drop(value);
    assert_eq!(Arc::try_unwrap(shared), Ok(7));
}
