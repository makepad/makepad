use std::{
    sync::Arc,
    task::{RawWaker, RawWakerVTable, Wake, Waker},
};

pub fn waker<T>(task: Arc<T>) -> Waker
where
    T: Wake + 'static,
{
    unsafe {
        Waker::from_raw(RawWaker::new(
            Arc::into_raw(task).cast::<()>(),
            vtable::<T>(),
        ))
    }
}

fn vtable<T: Wake>() -> &'static RawWakerVTable {
    &RawWakerVTable::new(clone::<T>, wake::<T>, wake_by_ref::<T>, drop::<T>)
}

unsafe fn clone<T: Wake>(data: *const ()) -> RawWaker {
    use std::mem::ManuallyDrop;

    let task = ManuallyDrop::new(Arc::from_raw(data.cast::<T>()));
    let _ = task.clone();
    RawWaker::new(data, vtable::<T>())
}

unsafe fn wake<T: Wake>(data: *const ()) {
    let task = Arc::from_raw(data.cast::<T>());
    task.wake();
}

unsafe fn wake_by_ref<T: Wake>(data: *const ()) {
    use std::mem::ManuallyDrop;

    let task = ManuallyDrop::new(Arc::from_raw(data.cast::<T>()));
    task.wake_by_ref();
}

unsafe fn drop<T: Wake>(data: *const ()) {
    use std::mem;

    mem::drop(Arc::from_raw(data.cast::<T>()))
}
