//! Minimal Mach/pthread/dladdr ABI for the in-process UI sampler. Layouts and
//! flavors follow the macOS SDK mach/{arm,i386}/thread_status.h.
use std::ffi::{c_char, c_void};

#[repr(C)]
pub struct DlInfo {
    pub filename: *const c_char,
    pub image_base: *mut c_void,
    pub symbol_name: *const c_char,
    pub symbol_address: *mut c_void,
}

extern "C" {
    pub fn mach_thread_self() -> u32;
    pub static mach_task_self_: u32;
    pub fn mach_port_deallocate(task: u32, name: u32) -> i32;
    pub fn thread_suspend(thread: u32) -> i32;
    pub fn thread_resume(thread: u32) -> i32;
    pub fn thread_get_state(thread: u32, flavor: i32, state: *mut u32, count: *mut u32) -> i32;
    pub fn mach_vm_read_overwrite(task: u32, address: u64, size: u64, data: u64, out_size: *mut u64) -> i32;
    pub fn pthread_self() -> *mut c_void;
    pub fn pthread_get_stackaddr_np(thread: *mut c_void) -> *mut c_void;
    pub fn pthread_get_stacksize_np(thread: *mut c_void) -> usize;
    pub fn dladdr(address: *const c_void, info: *mut DlInfo) -> i32;
}
