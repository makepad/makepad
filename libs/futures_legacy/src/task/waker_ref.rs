use {
    super::{waker, ArcWake},
    std::{marker::PhantomData, mem::ManuallyDrop, ops::Deref, sync::Arc, task::Waker},
};

/// A reference to a `Waker`.
/// 
/// [`Waker`]: std::task::Waker
#[derive(Debug)]
pub struct WakerRef<'a> {
    waker: ManuallyDrop<Waker>,
    phantom: PhantomData<&'a ()>,
}

impl<'a> Deref for WakerRef<'a> {
    type Target = Waker;

    fn deref(&self) -> &Self::Target {
        &self.waker
    }
}

/// Creates a reference to a `Waker` from a reference to an [`Arc<impl ArcWake>`][`ArcWake`].
///
/// [`Waker`]: std::task::Waker
pub fn waker_ref<W: ArcWake>(waker: &Arc<W>) -> WakerRef<'_> {
    use std::task::RawWaker;

    let data = Arc::as_ptr(waker).cast::<()>();
    let waker =
        ManuallyDrop::new(unsafe { Waker::from_raw(RawWaker::new(data, waker::vtable::<W>())) });
    WakerRef {
        waker,
        phantom: PhantomData,
    }
}
