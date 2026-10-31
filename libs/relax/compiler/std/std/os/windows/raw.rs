//! std::os::windows::raw: the platform types of the raw handle and socket.

pub type HANDLE = *mut core::ffi::c_void;
pub type SOCKET = u64;
