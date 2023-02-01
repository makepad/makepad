use {
    super::ArcWake,
    std::{
        sync::Arc,
        task::{RawWaker, RawWakerVTable, Waker},
    },
};

/// Creates a `Waker` from an [`Arc<impl ArcWake>`][`ArcWake`].
///
/// [`Waker`]: std::task::Waker
pub fn waker<W: ArcWake>(waker: Arc<W>) -> Waker {
    let data = Arc::into_raw(waker).cast::<()>();
    unsafe { Waker::from_raw(RawWaker::new(data, vtable::<W>())) }
}

pub(super) fn vtable<W: ArcWake>() -> &'static RawWakerVTable {
    &RawWakerVTable::new(clone::<W>, wake::<W>, wake_by_ref::<W>, drop::<W>)
}

unsafe fn clone<W: ArcWake>(data: *const ()) -> RawWaker {
    use std::mem::ManuallyDrop;

    // Reconstruct the `Arc` from the raw data, and wrap it `ManuallyDrop` so we don't touch the
    // refcount.
    let waker = ManuallyDrop::new(Arc::from_raw(data.cast::<W>()));
    // Clone the `Arc` to increment the refcount.
    let _: ManuallyDrop<_> = waker.clone();
    RawWaker::new(data, vtable::<W>())
}

unsafe fn wake<W: ArcWake>(data: *const ()) {
    // Reconstruct the `Arc` from the raw data.
    let waker = Arc::from_raw(data.cast::<W>());
    waker.wake();
}

unsafe fn wake_by_ref<W: ArcWake>(data: *const ()) {
    use std::mem;

    // Reconstruct the `Arc` from the raw data, and wrap it `ManuallyDrop` so we don't touch the
    // refcount.
    let waker = mem::ManuallyDrop::new(Arc::from_raw(data.cast::<W>()));
    waker.wake_by_ref();
}

unsafe fn drop<W: ArcWake>(data: *const ()) {
    use std::mem;

    // Reconstruct the `Arc` from the raw data.
    let waker = Arc::from_raw(data.cast::<W>());
    // Drop the `Arc` to decrement the refcount.
    mem::drop(waker);
}
