use std::sync::Arc;

/// A trait for waking up a specific task.
/// 
/// Objects that implement this trait which are wrapped in an `Arc` can be converted into a
/// [`Waker`].
/// 
/// The following functions convert an `ArcWake` into a [`Waker`]:
/// * [`waker`] converts an `Arc<impl ArcWake>` into a [`Waker`].
/// * [`waker_ref`] converts a reference to an `Arc<Impl ArcWake>` into a [`WakerRef`].
/// 
/// [`Waker`]: std::task::Waker
/// [`WakerRef`]: super::WakerRef
/// [`waker`]: super::waker()
/// [`waker_ref`]: super::waker_ref()
pub trait ArcWake: Send + Sync {
    /// Wakes up the task associated with this object.
    fn wake(self: Arc<Self>) {
        Self::wake_by_ref(&self)
    }

    /// Wakes up the task associated with this object.
    fn wake_by_ref(self: &Arc<Self>);
}
