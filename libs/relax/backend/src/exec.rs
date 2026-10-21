//! Executable memory for standalone generated code (kernels): one immutable mapping per
//! code object, W^X (macOS: MAP_JIT + write protection per thread; Linux: written, then
//! mprotect'ed to read+execute; Windows: written, then VirtualProtect'ed to execute-read),
//! unmapped on drop.

#[cfg(unix)]
mod sys {
    use std::ffi::c_void;
    pub const PROT_READ: i32 = 1;
    pub const PROT_WRITE: i32 = 2;
    #[cfg(target_os = "macos")]
    pub const PROT_EXEC: i32 = 4;
    #[cfg(not(target_os = "macos"))]
    pub const PROT_EXEC: i32 = 4;
    pub const MAP_PRIVATE: i32 = 0x02;
    #[cfg(target_os = "macos")]
    pub const MAP_ANON: i32 = 0x1000;
    #[cfg(target_os = "macos")]
    pub const MAP_JIT: i32 = 0x800;
    #[cfg(not(target_os = "macos"))]
    pub const MAP_ANON: i32 = 0x20;
    extern "C" {
        pub fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
        pub fn munmap(addr: *mut c_void, len: usize) -> i32;
        #[cfg(not(target_os = "macos"))]
        pub fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
        #[cfg(target_os = "macos")]
        pub fn pthread_jit_write_protect_np(enabled: i32);
        #[cfg(target_os = "macos")]
        pub fn sys_icache_invalidate(start: *mut c_void, len: usize);
        #[cfg(all(not(target_os = "macos"), target_arch = "aarch64"))]
        pub fn __clear_cache(start: *mut c_void, end: *mut c_void);
    }
}

#[cfg(windows)]
mod win {
    pub use relax_session::win32::{
        FlushInstructionCache, GetCurrentProcess, VirtualAlloc, VirtualFree, VirtualProtect, MEM_COMMIT, MEM_RELEASE,
        MEM_RESERVE, PAGE_READWRITE,
    };
    pub const PAGE_EXECUTE_READ: u32 = 0x20;
}

/// Machine code in its own executable mapping. Immutable once built; unmapped on drop.
pub struct Code {
    ptr: *mut u8,
    map_len: usize,
    len: usize,
}

// SAFETY: the mapping is immutable after construction; the code's mutable data comes in
// through its arguments.
unsafe impl Send for Code {}
unsafe impl Sync for Code {}

impl Code {
    /// Maps `bytes` as executable code (None where executable memory is unavailable).
    pub fn new(bytes: &[u8]) -> Option<Code> {
        #[cfg(windows)]
        unsafe {
            let len = bytes.len().max(1);
            let map_len = (len + 65535) & !65535;
            let ptr = win::VirtualAlloc(std::ptr::null_mut(), map_len, win::MEM_COMMIT | win::MEM_RESERVE, win::PAGE_READWRITE);
            if ptr.is_null() {
                return None;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
            let mut old = 0u32;
            if win::VirtualProtect(ptr, map_len, win::PAGE_EXECUTE_READ, &mut old) == 0 {
                win::VirtualFree(ptr, 0, win::MEM_RELEASE);
                return None;
            }
            win::FlushInstructionCache(win::GetCurrentProcess(), ptr, len);
            return Some(Code { ptr: ptr as *mut u8, map_len, len: bytes.len() });
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android", windows)))]
        {
            let _ = bytes;
            return None;
        }
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "android"))]
        unsafe {
            let len = bytes.len().max(1);
            let map_len = (len + 16383) & !16383;
            #[cfg(target_os = "macos")]
            let ptr = sys::mmap(std::ptr::null_mut(), map_len, sys::PROT_READ | sys::PROT_WRITE | sys::PROT_EXEC, sys::MAP_PRIVATE | sys::MAP_ANON | sys::MAP_JIT, -1, 0);
            #[cfg(not(target_os = "macos"))]
            let ptr = sys::mmap(std::ptr::null_mut(), map_len, sys::PROT_READ | sys::PROT_WRITE, sys::MAP_PRIVATE | sys::MAP_ANON, -1, 0);
            if ptr.is_null() || ptr as isize == -1 {
                return None;
            }
            #[cfg(target_os = "macos")]
            sys::pthread_jit_write_protect_np(0);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
            #[cfg(target_os = "macos")]
            {
                sys::pthread_jit_write_protect_np(1);
                sys::sys_icache_invalidate(ptr, len);
            }
            #[cfg(not(target_os = "macos"))]
            {
                if sys::mprotect(ptr, map_len, sys::PROT_READ | sys::PROT_EXEC) != 0 {
                    sys::munmap(ptr, map_len);
                    return None;
                }
                #[cfg(target_arch = "aarch64")]
                sys::__clear_cache(ptr, (ptr as *mut u8).add(len) as *mut _);
            }
            Some(Code { ptr: ptr as *mut u8, map_len, len: bytes.len() })
        }
    }

    /// Entry address.
    pub fn ptr(&self) -> *const u8 {
        self.ptr
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The code bytes (disassembly, tests).
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: the mapping holds `len` readable bytes for the life of `self`.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for Code {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            sys::munmap(self.ptr as *mut _, self.map_len);
        }
        #[cfg(windows)]
        unsafe {
            let _ = self.map_len;
            win::VirtualFree(self.ptr as *mut _, 0, win::MEM_RELEASE);
        }
    }
}
