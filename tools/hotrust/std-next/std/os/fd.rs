//! std::os::fd: owned and borrowed file descriptors.

use core::fmt;
use core::marker::PhantomData;

use crate::io;
use crate::sys;

pub type RawFd = core::ffi::c_int;

pub trait AsRawFd {
    fn as_raw_fd(&self) -> RawFd;
}

pub trait FromRawFd {
    unsafe fn from_raw_fd(fd: RawFd) -> Self;
}

pub trait IntoRawFd {
    fn into_raw_fd(self) -> RawFd;
}

pub trait AsFd {
    fn as_fd(&self) -> BorrowedFd<'_>;
}

/// An open file descriptor, closed on drop.
#[repr(transparent)]
pub struct OwnedFd {
    fd: RawFd,
}

#[derive(Copy, Clone)]
#[repr(transparent)]
pub struct BorrowedFd<'fd> {
    fd: RawFd,
    _phantom: PhantomData<&'fd OwnedFd>,
}

#[cfg(target_os = "macos")]
const F_DUPFD_CLOEXEC: core::ffi::c_int = 67;
#[cfg(target_os = "linux")]
const F_DUPFD_CLOEXEC: core::ffi::c_int = 1030;

impl OwnedFd {
    pub fn try_clone(&self) -> io::Result<OwnedFd> {
        self.as_fd().try_clone_to_owned()
    }
}

impl<'fd> BorrowedFd<'fd> {
    /// # Safety: `fd` stays open for 'fd.
    pub const unsafe fn borrow_raw(fd: RawFd) -> BorrowedFd<'fd> {
        BorrowedFd { fd, _phantom: PhantomData }
    }
    pub fn try_clone_to_owned(&self) -> io::Result<OwnedFd> {
        let fd = unsafe { sys::fcntl(self.fd, F_DUPFD_CLOEXEC, 3 as core::ffi::c_int) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(OwnedFd { fd })
    }
}

impl Drop for OwnedFd {
    fn drop(&mut self) {
        unsafe {
            sys::close(self.fd);
        }
    }
}

impl AsRawFd for OwnedFd {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}
impl IntoRawFd for OwnedFd {
    fn into_raw_fd(self) -> RawFd {
        let fd = self.fd;
        core::mem::forget(self);
        fd
    }
}
impl FromRawFd for OwnedFd {
    unsafe fn from_raw_fd(fd: RawFd) -> OwnedFd {
        if fd == -1 {
            panic!("assertion failed: fd != u32::MAX as RawFd");
        }
        OwnedFd { fd }
    }
}
impl AsFd for OwnedFd {
    fn as_fd(&self) -> BorrowedFd<'_> {
        unsafe { BorrowedFd::borrow_raw(self.fd) }
    }
}
impl<'fd> AsRawFd for BorrowedFd<'fd> {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}
impl<'fd> AsFd for BorrowedFd<'fd> {
    fn as_fd(&self) -> BorrowedFd<'_> {
        *self
    }
}
impl AsRawFd for RawFd {
    fn as_raw_fd(&self) -> RawFd {
        *self
    }
}
impl IntoRawFd for RawFd {
    fn into_raw_fd(self) -> RawFd {
        self
    }
}
impl FromRawFd for RawFd {
    unsafe fn from_raw_fd(fd: RawFd) -> RawFd {
        fd
    }
}
impl<T: AsFd + ?Sized> AsFd for &T {
    fn as_fd(&self) -> BorrowedFd<'_> {
        T::as_fd(self)
    }
}
impl<T: AsFd + ?Sized> AsFd for &mut T {
    fn as_fd(&self) -> BorrowedFd<'_> {
        T::as_fd(self)
    }
}

impl AsRawFd for crate::io::Stdin {
    fn as_raw_fd(&self) -> RawFd {
        0
    }
}
impl AsRawFd for crate::io::Stdout {
    fn as_raw_fd(&self) -> RawFd {
        1
    }
}
impl AsRawFd for crate::io::Stderr {
    fn as_raw_fd(&self) -> RawFd {
        2
    }
}
impl AsFd for crate::io::Stdin {
    fn as_fd(&self) -> BorrowedFd<'_> {
        unsafe { BorrowedFd::borrow_raw(0) }
    }
}
impl AsFd for crate::io::Stdout {
    fn as_fd(&self) -> BorrowedFd<'_> {
        unsafe { BorrowedFd::borrow_raw(1) }
    }
}
impl AsFd for crate::io::Stderr {
    fn as_fd(&self) -> BorrowedFd<'_> {
        unsafe { BorrowedFd::borrow_raw(2) }
    }
}

impl fmt::Debug for OwnedFd {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OwnedFd").field("fd", &self.fd).finish()
    }
}
impl<'fd> fmt::Debug for BorrowedFd<'fd> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("BorrowedFd").field("fd", &self.fd).finish()
    }
}
