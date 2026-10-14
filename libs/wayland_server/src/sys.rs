//! The few libc calls the server needs, declared directly (Linux x86_64 and aarch64).
#![allow(non_camel_case_types)]
use std::ffi::c_void;

pub const SOL_SOCKET: i32 = 1;
pub const SCM_RIGHTS: i32 = 1;
pub const SO_PEERCRED: i32 = 17;
pub const MSG_DONTWAIT: i32 = 0x40;
pub const MSG_NOSIGNAL: i32 = 0x4000;
pub const MSG_CTRUNC: i32 = 8;
pub const MSG_CMSG_CLOEXEC: i32 = 0x4000_0000;
pub const EPOLLIN: u32 = 1;
pub const EPOLLOUT: u32 = 4;
pub const EPOLLERR: u32 = 8;
pub const EPOLLHUP: u32 = 0x10;
pub const EPOLLRDHUP: u32 = 0x2000;
pub const EPOLL_CTL_ADD: i32 = 1;
pub const EPOLL_CTL_DEL: i32 = 2;
pub const EPOLL_CTL_MOD: i32 = 3;
pub const EPOLL_CLOEXEC: i32 = 0x80000;
pub const EFD_CLOEXEC: i32 = 0x80000;
pub const EFD_NONBLOCK: i32 = 0x800;
pub const LOCK_EX: i32 = 2;
pub const LOCK_NB: i32 = 4;
pub const EAGAIN: i32 = 11;
pub const EINTR: i32 = 4;

#[repr(C)]
pub struct iovec { pub base: *mut c_void, pub len: usize }
#[repr(C)]
pub struct msghdr {
    pub name: *mut c_void,
    pub namelen: u32,
    pub iov: *mut iovec,
    pub iovlen: usize,
    pub control: *mut c_void,
    pub controllen: usize,
    pub flags: i32,
}
#[repr(C)]
pub struct cmsghdr { pub len: usize, pub level: i32, pub kind: i32 }
#[repr(C)]
pub struct ucred { pub pid: i32, pub uid: u32, pub gid: u32 }
/// Packed on x86_64 only, as in the kernel ABI.
#[cfg_attr(target_arch = "x86_64", repr(C, packed))]
#[cfg_attr(not(target_arch = "x86_64"), repr(C))]
#[derive(Clone, Copy)]
pub struct epoll_event { pub events: u32, pub data: u64 }

extern "C" {
    pub fn recvmsg(fd: i32, msg: *mut msghdr, flags: i32) -> isize;
    pub fn sendmsg(fd: i32, msg: *const msghdr, flags: i32) -> isize;
    pub fn getsockopt(fd: i32, level: i32, name: i32, value: *mut c_void, len: *mut u32) -> i32;
    pub fn epoll_create1(flags: i32) -> i32;
    pub fn epoll_ctl(epfd: i32, op: i32, fd: i32, ev: *mut epoll_event) -> i32;
    pub fn epoll_wait(epfd: i32, events: *mut epoll_event, max: i32, timeout: i32) -> i32;
    pub fn eventfd(init: u32, flags: i32) -> i32;
    pub fn flock(fd: i32, op: i32) -> i32;
    pub fn read(fd: i32, buf: *mut c_void, count: usize) -> isize;
    pub fn write(fd: i32, buf: *const c_void, count: usize) -> isize;
}

pub fn cmsg_align(n: usize) -> usize { (n + 7) & !7 }
pub fn cmsg_space(data: usize) -> usize { cmsg_align(std::mem::size_of::<cmsghdr>()) + cmsg_align(data) }
pub fn cmsg_len(data: usize) -> usize { cmsg_align(std::mem::size_of::<cmsghdr>()) + data }

pub const PROT_READ: i32 = 1;
pub const MAP_SHARED: i32 = 1;
pub const MAP_PRIVATE: i32 = 2;
pub const MAP_FIXED: i32 = 0x10;
pub const MAP_ANONYMOUS: i32 = 0x20;
pub const SIGBUS: i32 = 7;
pub const SA_SIGINFO: i32 = 4;
pub const SA_NODEFER: i32 = 0x4000_0000;

/// glibc/musl 64-bit layout (x86_64 and aarch64).
#[repr(C)]
pub struct sigaction_t {
    pub handler: usize,
    pub mask: [u64; 16],
    pub flags: i32,
    pub restorer: usize,
}
extern "C" {
    pub fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
    pub fn munmap(addr: *mut c_void, len: usize) -> i32;
    pub fn sigaction(sig: i32, act: *const sigaction_t, old: *mut sigaction_t) -> i32;
}

pub const MFD_CLOEXEC: u32 = 1;
pub const O_CLOEXEC: i32 = 0x80000;
extern "C" {
    pub fn memfd_create(name: *const std::ffi::c_char, flags: u32) -> i32;
    pub fn pipe2(fds: *mut i32, flags: i32) -> i32;
}

pub const POLLIN: i16 = 1;
#[repr(C)]
pub struct pollfd { pub fd: i32, pub events: i16, pub revents: i16 }
extern "C" {
    pub fn ioctl(fd: i32, request: u64, ...) -> i32;
    pub fn poll(fds: *mut pollfd, nfds: u64, timeout: i32) -> i32;
    pub fn lseek(fd: i32, offset: i64, whence: i32) -> i64;
}
pub const SEEK_END: i32 = 2;
/// struct dma_buf_export_sync_file / dma_buf_import_sync_file: { u32 flags; s32 fd }.
#[repr(C)]
pub struct dma_buf_sync_file { pub flags: u32, pub fd: i32 }
pub const DMA_BUF_IOCTL_EXPORT_SYNC_FILE: u64 = 0xC008_6202;
pub const DMA_BUF_IOCTL_IMPORT_SYNC_FILE: u64 = 0x4008_6203;
pub const DMA_BUF_SYNC_READ: u32 = 1;
pub const DMA_BUF_SYNC_WRITE: u32 = 2;

/// DRM syncobj ioctls (drm.h), struct sizes as the kernel's first version of each.
#[repr(C)] pub struct drm_syncobj_create { pub handle: u32, pub flags: u32 }
#[repr(C)] pub struct drm_syncobj_destroy { pub handle: u32, pub pad: u32 }
#[repr(C)] pub struct drm_syncobj_handle { pub handle: u32, pub flags: u32, pub fd: i32, pub pad: u32 }
#[repr(C)] pub struct drm_syncobj_transfer { pub src_handle: u32, pub dst_handle: u32, pub src_point: u64, pub dst_point: u64, pub flags: u32, pub pad: u32 }
#[repr(C)] pub struct drm_syncobj_timeline_array { pub handles: u64, pub points: u64, pub count_handles: u32, pub flags: u32 }
pub const DRM_IOCTL_SYNCOBJ_CREATE: u64 = 0xC008_64BF;
pub const DRM_IOCTL_SYNCOBJ_DESTROY: u64 = 0xC008_64C0;
pub const DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD: u64 = 0xC010_64C1;
pub const DRM_IOCTL_SYNCOBJ_FD_TO_HANDLE: u64 = 0xC010_64C2;
pub const DRM_IOCTL_SYNCOBJ_TRANSFER: u64 = 0xC020_64CC;
pub const DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL: u64 = 0xC018_64CD;
pub const DRM_SYNCOBJ_FD_TO_HANDLE_FLAGS_IMPORT_SYNC_FILE: u32 = 1;
pub const DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE: u32 = 1;
pub const DRM_SYNCOBJ_WAIT_FLAGS_WAIT_FOR_SUBMIT: u32 = 2;
