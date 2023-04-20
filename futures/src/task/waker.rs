use std::{
    sync::Arc,
    task::{RawWaker, RawWakerVTable, Wake, Waker},
};

pub fn waker<T>(waker: Arc<T>) -> Waker
where
    T: Wake + 'static,
{
    unsafe {
        Waker::from_raw(RawWaker::new(
            Arc::into_raw(waker).cast::<()>(),
            vtable::<T>(),
        ))
    }
}

fn vtable<T: Wake>() -> &'static RawWakerVTable {
    unsafe fn clone<T: Wake>(data: *const ()) -> RawWaker {
        use std::mem::ManuallyDrop;

        let waker = ManuallyDrop::new(Arc::from_raw(data.cast::<T>()));
        let _ = waker.clone();
        RawWaker::new(data, vtable::<T>())
    }

    unsafe fn wake<T: Wake>(data: *const ()) {
        let waker = Arc::from_raw(data.cast::<T>());
        waker.wake();
    }

    unsafe fn wake_by_ref<T: Wake>(data: *const ()) {
        use std::mem::ManuallyDrop;

        let waker = ManuallyDrop::new(Arc::from_raw(data.cast::<T>()));
        waker.wake_by_ref();
    }

    unsafe fn drop<T: Wake>(data: *const ()) {
        use std::mem;

        mem::drop(Arc::from_raw(data.cast::<T>()))
    }

    &RawWakerVTable::new(clone::<T>, wake::<T>, wake_by_ref::<T>, drop::<T>)
}
