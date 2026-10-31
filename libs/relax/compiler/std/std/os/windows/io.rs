//! std::os::windows::io: owned and borrowed handles and sockets (real std's
//! library/std/src/os/windows/io/{handle,socket,raw}.rs).

use core::fmt;
use core::marker::PhantomData;

use crate::io;

pub type RawHandle = super::raw::HANDLE;
pub type RawSocket = super::raw::SOCKET;

#[link(name = "kernel32")]
extern "system" {
    fn CloseHandle(h: RawHandle) -> i32;
    fn GetCurrentProcess() -> RawHandle;
    fn DuplicateHandle(src_process: RawHandle, src: RawHandle, dst_process: RawHandle, dst: *mut RawHandle, access: u32, inherit: i32, options: u32) -> i32;
}

#[link(name = "ws2_32")]
extern "system" {
    fn closesocket(s: RawSocket) -> i32;
}

const DUPLICATE_SAME_ACCESS: u32 = 0x2;
const INVALID_HANDLE_VALUE: RawHandle = -1isize as RawHandle;
const INVALID_SOCKET: RawSocket = !0;

pub trait AsRawHandle {
    fn as_raw_handle(&self) -> RawHandle;
}
pub trait FromRawHandle {
    unsafe fn from_raw_handle(handle: RawHandle) -> Self;
}
pub trait IntoRawHandle {
    fn into_raw_handle(self) -> RawHandle;
}
pub trait AsHandle {
    fn as_handle(&self) -> BorrowedHandle<'_>;
}

pub trait AsRawSocket {
    fn as_raw_socket(&self) -> RawSocket;
}
pub trait FromRawSocket {
    unsafe fn from_raw_socket(sock: RawSocket) -> Self;
}
pub trait IntoRawSocket {
    fn into_raw_socket(self) -> RawSocket;
}
pub trait AsSocket {
    fn as_socket(&self) -> BorrowedSocket<'_>;
}

/// An open handle, closed on drop.
#[repr(transparent)]
pub struct OwnedHandle {
    handle: RawHandle,
}

unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

#[derive(Copy, Clone)]
#[repr(transparent)]
pub struct BorrowedHandle<'handle> {
    handle: RawHandle,
    _phantom: PhantomData<&'handle OwnedHandle>,
}

unsafe impl<'h> Send for BorrowedHandle<'h> {}
unsafe impl<'h> Sync for BorrowedHandle<'h> {}

/// A handle from an API that returns INVALID_HANDLE_VALUE on failure (CreateFileW).
#[repr(transparent)]
pub struct HandleOrInvalid(RawHandle);

/// A handle from an API that returns null on failure (CreateThread, OpenProcess).
#[repr(transparent)]
pub struct HandleOrNull(RawHandle);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidHandleError(());
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NullHandleError(());

impl fmt::Display for InvalidHandleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("A HandleOrInvalid could not be converted to a handle because it was INVALID_HANDLE_VALUE")
    }
}
impl fmt::Display for NullHandleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("A HandleOrNull could not be converted to a handle because it was null")
    }
}

impl HandleOrInvalid {
    pub unsafe fn from_raw_handle(handle: RawHandle) -> Self {
        HandleOrInvalid(handle)
    }
}
impl HandleOrNull {
    pub unsafe fn from_raw_handle(handle: RawHandle) -> Self {
        HandleOrNull(handle)
    }
}

impl TryFrom<HandleOrInvalid> for OwnedHandle {
    type Error = InvalidHandleError;
    fn try_from(h: HandleOrInvalid) -> Result<OwnedHandle, InvalidHandleError> {
        if h.0 == INVALID_HANDLE_VALUE {
            Err(InvalidHandleError(()))
        } else {
            Ok(OwnedHandle { handle: h.0 })
        }
    }
}
impl TryFrom<HandleOrNull> for OwnedHandle {
    type Error = NullHandleError;
    fn try_from(h: HandleOrNull) -> Result<OwnedHandle, NullHandleError> {
        if h.0.is_null() {
            Err(NullHandleError(()))
        } else {
            Ok(OwnedHandle { handle: h.0 })
        }
    }
}

impl OwnedHandle {
    pub fn try_clone(&self) -> io::Result<OwnedHandle> {
        self.as_handle().try_clone_to_owned()
    }
}

impl<'handle> BorrowedHandle<'handle> {
    /// # Safety: `handle` stays open for 'handle.
    pub const unsafe fn borrow_raw(handle: RawHandle) -> BorrowedHandle<'handle> {
        BorrowedHandle { handle, _phantom: PhantomData }
    }
    pub fn try_clone_to_owned(&self) -> io::Result<OwnedHandle> {
        let mut out: RawHandle = core::ptr::null_mut();
        let ok = unsafe { DuplicateHandle(GetCurrentProcess(), self.handle, GetCurrentProcess(), &mut out, 0, 0, DUPLICATE_SAME_ACCESS) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(OwnedHandle { handle: out })
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

impl AsRawHandle for OwnedHandle {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle
    }
}
impl IntoRawHandle for OwnedHandle {
    fn into_raw_handle(self) -> RawHandle {
        let h = self.handle;
        core::mem::forget(self);
        h
    }
}
impl FromRawHandle for OwnedHandle {
    unsafe fn from_raw_handle(handle: RawHandle) -> OwnedHandle {
        OwnedHandle { handle }
    }
}
impl AsHandle for OwnedHandle {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        unsafe { BorrowedHandle::borrow_raw(self.handle) }
    }
}
impl<'h> AsRawHandle for BorrowedHandle<'h> {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle
    }
}
impl<'h> AsHandle for BorrowedHandle<'h> {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        *self
    }
}
impl<T: AsHandle + ?Sized> AsHandle for &T {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        (**self).as_handle()
    }
}
impl<T: AsHandle + ?Sized> AsHandle for &mut T {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        (**self).as_handle()
    }
}

impl fmt::Debug for OwnedHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedHandle").field("handle", &self.handle).finish()
    }
}
impl<'h> fmt::Debug for BorrowedHandle<'h> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BorrowedHandle").field("handle", &self.handle).finish()
    }
}

/// An open socket, closed on drop.
#[repr(transparent)]
pub struct OwnedSocket {
    socket: RawSocket,
}

#[derive(Copy, Clone)]
#[repr(transparent)]
pub struct BorrowedSocket<'socket> {
    socket: RawSocket,
    _phantom: PhantomData<&'socket OwnedSocket>,
}

impl<'s> BorrowedSocket<'s> {
    /// # Safety: `socket` stays open for 's and is not INVALID_SOCKET.
    pub const unsafe fn borrow_raw(socket: RawSocket) -> BorrowedSocket<'s> {
        BorrowedSocket { socket, _phantom: PhantomData }
    }
}

impl Drop for OwnedSocket {
    fn drop(&mut self) {
        unsafe {
            closesocket(self.socket);
        }
    }
}

impl AsRawSocket for OwnedSocket {
    fn as_raw_socket(&self) -> RawSocket {
        self.socket
    }
}
impl IntoRawSocket for OwnedSocket {
    fn into_raw_socket(self) -> RawSocket {
        let s = self.socket;
        core::mem::forget(self);
        s
    }
}
impl FromRawSocket for OwnedSocket {
    unsafe fn from_raw_socket(socket: RawSocket) -> OwnedSocket {
        if socket == INVALID_SOCKET {
            panic!("assertion failed: socket != INVALID_SOCKET");
        }
        OwnedSocket { socket }
    }
}
impl AsSocket for OwnedSocket {
    fn as_socket(&self) -> BorrowedSocket<'_> {
        unsafe { BorrowedSocket::borrow_raw(self.socket) }
    }
}
impl<'s> AsRawSocket for BorrowedSocket<'s> {
    fn as_raw_socket(&self) -> RawSocket {
        self.socket
    }
}
impl<'s> AsSocket for BorrowedSocket<'s> {
    fn as_socket(&self) -> BorrowedSocket<'_> {
        *self
    }
}
impl fmt::Debug for OwnedSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedSocket").field("socket", &self.socket).finish()
    }
}
