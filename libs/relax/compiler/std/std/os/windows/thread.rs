//! std::os::windows::thread: a thread's handle (real std's library/std/src/os/windows/thread.rs).

use crate::os::windows::io::{AsRawHandle, IntoRawHandle, RawHandle};
use crate::thread;

impl<T> AsRawHandle for thread::JoinHandle<T> {
    fn as_raw_handle(&self) -> RawHandle {
        self.as_raw() as RawHandle
    }
}

/// The handle, which the caller now closes; the thread is no longer joinable.
impl<T> IntoRawHandle for thread::JoinHandle<T> {
    fn into_raw_handle(self) -> RawHandle {
        self.into_raw() as RawHandle
    }
}
