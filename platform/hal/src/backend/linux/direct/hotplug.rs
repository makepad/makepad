//! Kernel DRM hotplug/property notifications, with the native periodic sysfs fallback.
use std::{ffi::{c_int,c_void}, os::fd::{AsRawFd,FromRawFd,OwnedFd}};
#[repr(C)]
struct Address { family:u16, pad:u16, pid:u32, groups:u32 }
extern "C" {
    fn socket(domain:c_int,ty:c_int,protocol:c_int) -> c_int;
    fn bind(fd:c_int,address:*const c_void,len:u32) -> c_int;
    fn recv(fd:c_int,bytes:*mut c_void,len:usize,flags:c_int) -> isize;
}
pub(super) struct Watch(OwnedFd);
impl Watch {
    pub fn open() -> Option<Self> {
        let fd = unsafe { socket(16,3 | 0o4000 | 0o2000000,15) };
        if fd < 0 { return None; }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let address = Address { family:16,pad:0,pid:0,groups:1 };
        if unsafe { bind(fd.as_raw_fd(),(&address as *const Address).cast(),std::mem::size_of::<Address>() as u32) } < 0 { return None; }
        Some(Self(fd))
    }
    pub fn fd(&self) -> i32 { self.0.as_raw_fd() }
    pub fn changed(&self) -> bool {
        let mut changed = false;
        let mut bytes = [0u8;4096];
        loop {
            let len = unsafe { recv(self.fd(),bytes.as_mut_ptr().cast(),bytes.len(),0) };
            if len < 0 {
                // ENOBUFS means a lost kernel notification: force a complete rescan.
                changed |= std::io::Error::last_os_error().raw_os_error() == Some(105);
                break;
            }
            if len == 0 { break; }
            let fields:Vec<_> = bytes[..len as usize].split(|b| *b == 0).collect();
            changed |= fields.contains(&b"SUBSYSTEM=drm".as_slice())
                && fields.iter().any(|f| *f == b"HOTPLUG=1" || f.starts_with(b"CONNECTOR="));
        }
        changed
    }
}
