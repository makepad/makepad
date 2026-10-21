//! Windows (x86_64-pc-windows-gnu): kernel32 / kernelbase / advapi32 declarations (`extern
//! "system"` = the Win64 C convention), the OS items generic std code reaches as `sys::os::*`
//! (futex, clocks, thread names, error kinds), and `portable`: the sys API posix.rs has in its
//! POSIX form (threads, thread-local keys, stdio, errors), re-exported as `sys::*`.
//! Semantics follow real std's sys/pal/windows (library/std/src/sys/{pal,io,thread,...}/windows).

#![allow(non_snake_case, non_camel_case_types, dead_code)]

use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::{c_char, c_void};
use core::sync::atomic::AtomicU32;
use core::time::Duration;

pub type HANDLE = *mut c_void;
pub type BOOL = i32;
pub type DWORD = u32;
pub type WCHAR = u16;

pub const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;
pub const INFINITE: DWORD = 0xffff_ffff;
pub const STD_INPUT_HANDLE: DWORD = -10i32 as DWORD;
pub const STD_OUTPUT_HANDLE: DWORD = -11i32 as DWORD;
pub const STD_ERROR_HANDLE: DWORD = -12i32 as DWORD;

pub const ERROR_FILE_NOT_FOUND: DWORD = 2;
pub const ERROR_PATH_NOT_FOUND: DWORD = 3;
pub const ERROR_ACCESS_DENIED: DWORD = 5;
pub const ERROR_INVALID_HANDLE: DWORD = 6;
pub const ERROR_NOT_ENOUGH_MEMORY: DWORD = 8;
pub const ERROR_OUTOFMEMORY: DWORD = 14;
pub const ERROR_INVALID_DRIVE: DWORD = 15;
pub const ERROR_NOT_SAME_DEVICE: DWORD = 17;
pub const ERROR_NO_MORE_FILES: DWORD = 18;
pub const ERROR_WRITE_PROTECT: DWORD = 19;
pub const ERROR_SHARING_VIOLATION: DWORD = 32;
pub const ERROR_HANDLE_EOF: DWORD = 38;
pub const ERROR_HANDLE_DISK_FULL: DWORD = 39;
pub const ERROR_BAD_NETPATH: DWORD = 53;
pub const ERROR_BAD_NET_NAME: DWORD = 67;
pub const ERROR_FILE_EXISTS: DWORD = 80;
pub const ERROR_INVALID_PARAMETER: DWORD = 87;
pub const ERROR_BROKEN_PIPE: DWORD = 109;
pub const ERROR_DISK_FULL: DWORD = 112;
pub const ERROR_CALL_NOT_IMPLEMENTED: DWORD = 120;
pub const ERROR_SEM_TIMEOUT: DWORD = 121;
pub const ERROR_INSUFFICIENT_BUFFER: DWORD = 122;
pub const ERROR_INVALID_NAME: DWORD = 123;
pub const ERROR_SEEK_ON_DEVICE: DWORD = 132;
pub const ERROR_DIR_NOT_EMPTY: DWORD = 145;
pub const ERROR_BAD_PATHNAME: DWORD = 161;
pub const ERROR_BUSY: DWORD = 170;
pub const ERROR_ALREADY_EXISTS: DWORD = 183;
pub const ERROR_FILENAME_EXCED_RANGE: DWORD = 206;
pub const ERROR_FILE_TOO_LARGE: DWORD = 223;
pub const ERROR_NO_DATA: DWORD = 232;
pub const WAIT_TIMEOUT: DWORD = 258;
pub const ERROR_DIRECTORY: DWORD = 267;
pub const ERROR_ENVVAR_NOT_FOUND: DWORD = 203;
pub const ERROR_OPERATION_ABORTED: DWORD = 995;
pub const ERROR_POSSIBLE_DEADLOCK: DWORD = 1131;
pub const ERROR_TOO_MANY_LINKS: DWORD = 1142;
pub const ERROR_TIMEOUT: DWORD = 1460;
pub const ERROR_CANT_RESOLVE_FILENAME: DWORD = 1921;
pub const ERROR_HOST_UNREACHABLE: DWORD = 1232;
pub const ERROR_NETWORK_UNREACHABLE: DWORD = 1231;
pub const ERROR_DISK_QUOTA_EXCEEDED: DWORD = 1295;
pub const ERROR_DIRECTORY_NOT_SUPPORTED: DWORD = 336;

pub const WSAEINTR: i32 = 10004;
pub const WSAEACCES: i32 = 10013;
pub const WSAEINVAL: i32 = 10022;
pub const WSAEWOULDBLOCK: i32 = 10035;
pub const WSAEINPROGRESS: i32 = 10036;
pub const WSAEADDRINUSE: i32 = 10048;
pub const WSAEADDRNOTAVAIL: i32 = 10049;
pub const WSAENETDOWN: i32 = 10050;
pub const WSAENETUNREACH: i32 = 10051;
pub const WSAECONNABORTED: i32 = 10053;
pub const WSAECONNRESET: i32 = 10054;
pub const WSAENOTCONN: i32 = 10057;
pub const WSAESHUTDOWN: i32 = 10058;
pub const WSAETIMEDOUT: i32 = 10060;
pub const WSAECONNREFUSED: i32 = 10061;
pub const WSAEHOSTUNREACH: i32 = 10065;
pub const WSAEDQUOT: i32 = 10069;

/// No interrupted system calls on Windows (real std: is_interrupted is always false); the
/// Winsock code stands in where generic code compares with it.
pub const EINTR: i32 = WSAEINTR;
pub const EBADF: i32 = ERROR_INVALID_HANDLE as i32;
pub const ERANGE: i32 = ERROR_INSUFFICIENT_BUFFER as i32;
pub const EINVAL: i32 = ERROR_INVALID_PARAMETER as i32;
pub const EWOULDBLOCK: i32 = WSAEWOULDBLOCK;
pub const EINPROGRESS: i32 = WSAEINPROGRESS;

pub const FORMAT_MESSAGE_FROM_HMODULE: DWORD = 0x0800;
pub const FORMAT_MESSAGE_FROM_SYSTEM: DWORD = 0x1000;
pub const FORMAT_MESSAGE_IGNORE_INSERTS: DWORD = 0x0200;

#[repr(C)]
pub struct SYSTEM_INFO {
    pub wProcessorArchitecture: u16,
    pub wReserved: u16,
    pub dwPageSize: DWORD,
    pub lpMinimumApplicationAddress: *mut c_void,
    pub lpMaximumApplicationAddress: *mut c_void,
    pub dwActiveProcessorMask: usize,
    pub dwNumberOfProcessors: DWORD,
    pub dwProcessorType: DWORD,
    pub dwAllocationGranularity: DWORD,
    pub wProcessorLevel: u16,
    pub wProcessorRevision: u16,
}

#[link(name = "kernel32")]
extern "system" {
    pub fn GetLastError() -> DWORD;
    pub fn SetLastError(e: DWORD);
    pub fn CloseHandle(h: HANDLE) -> BOOL;
    pub fn GetStdHandle(which: DWORD) -> HANDLE;
    pub fn WriteFile(h: HANDLE, buf: *const u8, n: DWORD, written: *mut DWORD, overlapped: *mut c_void) -> BOOL;
    pub fn ReadFile(h: HANDLE, buf: *mut u8, n: DWORD, read: *mut DWORD, overlapped: *mut c_void) -> BOOL;
    pub fn GetConsoleMode(h: HANDLE, mode: *mut DWORD) -> BOOL;
    pub fn WriteConsoleW(h: HANDLE, buf: *const u16, n: DWORD, written: *mut DWORD, reserved: *mut c_void) -> BOOL;
    pub fn GetFileType(h: HANDLE) -> DWORD;
    pub fn FormatMessageW(flags: DWORD, source: *const c_void, id: DWORD, lang: DWORD, buf: *mut u16, n: DWORD, args: *const c_void) -> DWORD;
    pub fn GetModuleHandleW(name: *const u16) -> HANDLE;
    pub fn GetModuleFileNameW(module: HANDLE, buf: *mut u16, n: DWORD) -> DWORD;
    pub fn GetSystemInfo(info: *mut SYSTEM_INFO);
    pub fn SwitchToThread() -> BOOL;
    pub fn Sleep(ms: DWORD);
    pub fn GetCurrentThreadId() -> DWORD;
    pub fn GetCurrentThread() -> HANDLE;
    pub fn GetCurrentProcessId() -> DWORD;
    pub fn CreateThread(attrs: *mut c_void, stack: usize, start: extern "system" fn(*mut c_void) -> DWORD, arg: *mut c_void, flags: DWORD, id: *mut DWORD) -> HANDLE;
    pub fn WaitForSingleObject(h: HANDLE, ms: DWORD) -> DWORD;
    pub fn SetThreadDescription(t: HANDLE, desc: *const u16) -> i32;
    pub fn FlsAlloc(callback: extern "system" fn(*mut c_void)) -> DWORD;
    pub fn FlsFree(index: DWORD) -> BOOL;
    pub fn FlsGetValue(index: DWORD) -> *mut c_void;
    pub fn FlsSetValue(index: DWORD, v: *const c_void) -> BOOL;
    pub fn QueryPerformanceCounter(v: *mut i64) -> BOOL;
    pub fn QueryPerformanceFrequency(v: *mut i64) -> BOOL;
    pub fn GetSystemTimePreciseAsFileTime(ft: *mut u64);
    pub fn CreateWaitableTimerExW(attrs: *mut c_void, name: *const u16, flags: DWORD, access: DWORD) -> HANDLE;
    pub fn SetWaitableTimer(t: HANDLE, due: *const i64, period: i32, routine: *mut c_void, arg: *mut c_void, resume: BOOL) -> BOOL;
    pub fn ExitProcess(code: u32) -> !;
}

// WaitOnAddress & co. are kernelbase exports (API-MS-Win-Core-Synch-l1-2-0)
#[link(name = "kernelbase")]
extern "system" {
    pub fn WaitOnAddress(addr: *const c_void, compare: *const c_void, size: usize, ms: DWORD) -> BOOL;
    pub fn WakeByAddressSingle(addr: *const c_void);
    pub fn WakeByAddressAll(addr: *const c_void);
}

// RtlGenRandom
#[link(name = "advapi32")]
extern "system" {
    #[link_name = "SystemFunction036"]
    pub fn RtlGenRandom(buf: *mut u8, len: u32) -> u8;
}

/// `s` as a NUL-terminated UTF-16 string (an interior NUL is InvalidInput, as real std's to_u16s).
pub fn to_wide(s: &crate::ffi::OsStr) -> crate::io::Result<Vec<u16>> {
    let mut v: Vec<u16> = crate::os::windows::ffi::OsStrExt::encode_wide(s).collect();
    if v.iter().any(|&c| c == 0) {
        return Err(crate::io::const_error!(crate::io::ErrorKind::InvalidInput, "strings passed to WinAPI cannot contain NULs"));
    }
    v.push(0);
    Ok(v)
}

/// Calls `f(buf, len)` with growing buffers until the returned length fits (real std's
/// fill_utf16_buf): `f` returns the length written, or the size needed when the buffer was too
/// small (> len), or 0 with GetLastError set on failure.
pub fn fill_utf16_buf<F: FnMut(*mut u16, DWORD) -> DWORD>(mut f: F) -> crate::io::Result<Vec<u16>> {
    let mut n = 512usize;
    loop {
        let mut buf: Vec<u16> = alloc::vec![0; n];
        unsafe { SetLastError(0) };
        let k = f(buf.as_mut_ptr(), n as DWORD) as usize;
        if k == 0 && unsafe { GetLastError() } != 0 {
            return Err(crate::io::Error::last_os_error());
        }
        if k == n && unsafe { GetLastError() } == ERROR_INSUFFICIENT_BUFFER {
            n *= 2;
            continue;
        }
        if k > n {
            n = k;
            continue;
        }
        buf.truncate(k);
        return Ok(buf);
    }
}

/// A nonzero BOOL is success; FALSE -> the last OS error.
pub fn cvt(r: BOOL) -> crate::io::Result<BOOL> {
    if r == 0 {
        Err(crate::io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

// ---- items generic std code reaches as sys::os::*

pub type clockid_t = u32;
pub const CLOCK_REALTIME: clockid_t = 0;
pub const CLOCK_INSTANT: clockid_t = 1;
pub const CLOCK_MONOTONIC: clockid_t = 1;

/// Unix-epoch reading of `clock`: CLOCK_REALTIME from GetSystemTimePreciseAsFileTime (100 ns
/// since 1601), the monotonic clock from QueryPerformanceCounter (real std's Instant).
pub fn now(clock: clockid_t) -> super::time::Timespec {
    if clock == CLOCK_REALTIME {
        let mut ft: u64 = 0;
        unsafe { GetSystemTimePreciseAsFileTime(&mut ft) };
        const UNIX_EPOCH_FT: u64 = 116_444_736_000_000_000;
        let t = ft as i128 - UNIX_EPOCH_FT as i128;
        let secs = t.div_euclid(10_000_000) as i64;
        let nanos = (t.rem_euclid(10_000_000) * 100) as u32;
        return super::time::Timespec::new(secs, nanos as i64);
    }
    let (mut c, mut f) = (0i64, 0i64);
    unsafe {
        QueryPerformanceCounter(&mut c);
        QueryPerformanceFrequency(&mut f);
    }
    let secs = c / f;
    let nanos = ((c % f) as i128 * 1_000_000_000 / f as i128) as u32;
    super::time::Timespec::new(secs, nanos as i64)
}

/// Sleeps for `d`: a high-resolution waitable timer (real std's thread::sleep), else Sleep in ms
/// rounded up.
pub fn sleep(d: Duration) {
    const CREATE_WAITABLE_TIMER_HIGH_RESOLUTION: DWORD = 0x2;
    const TIMER_ALL_ACCESS: DWORD = 0x1f0003;
    unsafe {
        let t = CreateWaitableTimerExW(core::ptr::null_mut(), core::ptr::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS);
        if !t.is_null() {
            // negative = relative, in 100 ns units
            let due: i64 = -((d.as_nanos() / 100).min(i64::MAX as u128) as i64).max(1);
            if SetWaitableTimer(t, &due, 0, core::ptr::null_mut(), core::ptr::null_mut(), 0) != 0 {
                WaitForSingleObject(t, INFINITE);
                CloseHandle(t);
                return;
            }
            CloseHandle(t);
        }
        let mut ms = d.as_millis() + if d.subsec_nanos() % 1_000_000 != 0 { 1 } else { 0 };
        while ms > 0 {
            let chunk = ms.min((INFINITE - 1) as u128);
            Sleep(chunk as DWORD);
            ms -= chunk;
        }
    }
}

fn dur2timeout(d: Duration) -> DWORD {
    // in milliseconds, rounded up, saturating below INFINITE (real std's dur2timeout)
    let ms = d.as_secs().checked_mul(1000).and_then(|m| m.checked_add(((d.subsec_nanos() as u64) + 999_999) / 1_000_000));
    match ms {
        Some(ms) if ms < INFINITE as u64 => ms as DWORD,
        _ => INFINITE - 1,
    }
}

/// Waits while `*futex == expected` (WaitOnAddress); false = timed out.
pub fn futex_wait(futex: &AtomicU32, expected: u32, timeout: Option<Duration>) -> bool {
    let t = timeout.map(dur2timeout).unwrap_or(INFINITE);
    let r = unsafe { WaitOnAddress(futex.as_ptr() as *const c_void, &expected as *const u32 as *const c_void, 4, t) };
    r != 0 || unsafe { GetLastError() } != ERROR_TIMEOUT
}

pub fn futex_wake(futex: &AtomicU32) -> bool {
    unsafe { WakeByAddressSingle(futex.as_ptr() as *const c_void) };
    false
}

pub fn futex_wake_all(futex: &AtomicU32) {
    unsafe { WakeByAddressAll(futex.as_ptr() as *const c_void) };
}

/// The OS thread id real std prints in panic messages.
pub fn thread_os_id() -> u64 {
    unsafe { GetCurrentThreadId() as u64 }
}

/// The process's first thread: the one that ran Relax's boot (the runtime records it).
static MAIN_TID: AtomicU32 = AtomicU32::new(0);

pub fn is_main_thread() -> bool {
    use core::sync::atomic::Ordering;
    let me = unsafe { GetCurrentThreadId() };
    match MAIN_TID.compare_exchange(0, me, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => true,
        Err(t) => t == me,
    }
}

pub fn set_thread_name(name: *const c_char) {
    let mut n = 0;
    unsafe {
        while *name.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf8_lossy(core::slice::from_raw_parts(name as *const u8, n));
        let mut w: Vec<u16> = s.encode_utf16().collect();
        w.push(0);
        SetThreadDescription(GetCurrentThread(), w.as_ptr());
    }
}

/// The program's argv (UTF-8, as the runtime records it at boot).
pub fn args() -> (usize, *const *const c_char) {
    super::rt::args()
}

pub fn fill_random(buf: &mut [u8]) {
    for chunk in buf.chunks_mut(u32::MAX as usize) {
        if unsafe { RtlGenRandom(chunk.as_mut_ptr(), chunk.len() as u32) } == 0 {
            panic!("RtlGenRandom failed");
        }
    }
}

/// GetLastError / WSAGetLastError code -> ErrorKind (real std's sys/io/error/windows.rs).
pub fn decode_error_kind(code: i32) -> crate::io::ErrorKind {
    use crate::io::ErrorKind::*;
    match code as u32 {
        ERROR_ACCESS_DENIED => return PermissionDenied,
        ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS => return AlreadyExists,
        ERROR_BROKEN_PIPE | ERROR_NO_DATA => return BrokenPipe,
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_INVALID_DRIVE | ERROR_BAD_NETPATH | ERROR_BAD_NET_NAME => return NotFound,
        ERROR_INVALID_NAME | ERROR_BAD_PATHNAME | ERROR_FILENAME_EXCED_RANGE => return InvalidFilename,
        ERROR_INVALID_PARAMETER => return InvalidInput,
        ERROR_NOT_ENOUGH_MEMORY | ERROR_OUTOFMEMORY => return OutOfMemory,
        ERROR_SEM_TIMEOUT | WAIT_TIMEOUT | ERROR_OPERATION_ABORTED | ERROR_TIMEOUT => return TimedOut,
        ERROR_CALL_NOT_IMPLEMENTED => return Unsupported,
        ERROR_HOST_UNREACHABLE => return HostUnreachable,
        ERROR_NETWORK_UNREACHABLE => return NetworkUnreachable,
        ERROR_DIRECTORY => return NotADirectory,
        ERROR_DIRECTORY_NOT_SUPPORTED => return IsADirectory,
        ERROR_DIR_NOT_EMPTY => return DirectoryNotEmpty,
        ERROR_WRITE_PROTECT => return ReadOnlyFilesystem,
        ERROR_DISK_FULL | ERROR_HANDLE_DISK_FULL => return StorageFull,
        ERROR_SEEK_ON_DEVICE => return NotSeekable,
        ERROR_DISK_QUOTA_EXCEEDED => return QuotaExceeded,
        ERROR_FILE_TOO_LARGE => return FileTooLarge,
        ERROR_BUSY => return ResourceBusy,
        ERROR_POSSIBLE_DEADLOCK => return Deadlock,
        ERROR_NOT_SAME_DEVICE => return CrossesDevices,
        ERROR_TOO_MANY_LINKS => return TooManyLinks,
        ERROR_CANT_RESOLVE_FILENAME => return FilesystemLoop,
        _ => {}
    }
    match code {
        WSAEACCES => PermissionDenied,
        WSAEADDRINUSE => AddrInUse,
        WSAEADDRNOTAVAIL => AddrNotAvailable,
        WSAECONNABORTED => ConnectionAborted,
        WSAECONNREFUSED => ConnectionRefused,
        WSAECONNRESET => ConnectionReset,
        WSAEINVAL => InvalidInput,
        WSAENOTCONN => NotConnected,
        WSAEWOULDBLOCK => WouldBlock,
        WSAETIMEDOUT => TimedOut,
        WSAEHOSTUNREACH => HostUnreachable,
        WSAENETDOWN => NetworkDown,
        WSAENETUNREACH => NetworkUnreachable,
        WSAEDQUOT => QuotaExceeded,
        WSAESHUTDOWN => BrokenPipe,
        _ => Uncategorized,
    }
}

pub mod portable {
    use super::*;

    /// The calling thread's last OS error (GetLastError).
    pub fn errno() -> i32 {
        unsafe { GetLastError() as i32 }
    }

    pub fn set_errno(v: i32) {
        unsafe { SetLastError(v as DWORD) }
    }

    /// FormatMessageW's text for `code`, trailing CRLF trimmed (real std's error_string).
    pub fn error_string(mut code: i32) -> String {
        const FACILITY_NT_BIT: i32 = 0x1000_0000;
        let mut buf = [0u16; 2048];
        unsafe {
            let mut module: HANDLE = core::ptr::null_mut();
            let mut flags = 0;
            if code & FACILITY_NT_BIT != 0 {
                let ntdll: [u16; 10] = [b'N' as u16, b'T' as u16, b'D' as u16, b'L' as u16, b'L' as u16, b'.' as u16, b'D' as u16, b'L' as u16, b'L' as u16, 0];
                module = GetModuleHandleW(ntdll.as_ptr());
                if !module.is_null() {
                    code ^= FACILITY_NT_BIT;
                    flags = FORMAT_MESSAGE_FROM_HMODULE;
                }
            }
            let n = FormatMessageW(flags | FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS, module, code as DWORD, 0, buf.as_mut_ptr(), buf.len() as DWORD, core::ptr::null()) as usize;
            if n == 0 {
                return alloc::format!("OS Error {} (FormatMessageW() returned error {})", code, errno());
            }
            match String::from_utf16(&buf[..n]) {
                Ok(mut msg) => {
                    let len = msg.trim_end().len();
                    msg.truncate(len);
                    msg
                }
                Err(_) => alloc::format!("OS Error {} (FormatMessageW() returned invalid UTF-16)", code),
            }
        }
    }

    pub fn thread_yield() {
        unsafe {
            SwitchToThread();
        }
    }

    pub fn available_parallelism() -> usize {
        let mut info: SYSTEM_INFO = unsafe { core::mem::zeroed() };
        unsafe { GetSystemInfo(&mut info) };
        (info.dwNumberOfProcessors as usize).max(1)
    }

    pub fn current_thread() -> usize {
        unsafe { GetCurrentThreadId() as usize }
    }

    /// An owned thread handle, closed after joining or when detached by Drop.
    pub struct NativeThread(HANDLE);
    unsafe impl Send for NativeThread {}
    unsafe impl Sync for NativeThread {}

    struct StartArg {
        f: fn(*mut u8),
        arg: *mut u8,
        /// the starting thread's domain static block (relax domain.rs): the new thread runs in it
        domain: u64,
        token: u64,
    }

    extern "C" {
        fn __relax_domain_base() -> u64;
        fn __relax_domain_enter(base: u64, token: u64);
        fn __relax_domain_reserve(base: u64) -> u64;
        fn __relax_domain_failed(base: u64, token: u64);
    }

    extern "system" fn windows_thread_start(p: *mut c_void) -> DWORD {
        // first, before anything that may name a static
        unsafe { __relax_domain_enter((*(p as *const StartArg)).domain, (*(p as *const StartArg)).token) };
        let a = unsafe { alloc::boxed::Box::from_raw(p as *mut StartArg) };
        (a.f)(a.arg);
        0
    }

    /// CreateThread with `stack_size` reserved (STACK_SIZE_PARAM_IS_A_RESERVATION, as real std).
    pub fn spawn_thread(stack_size: usize, f: fn(*mut u8), arg: *mut u8) -> Result<NativeThread, i32> {
        const STACK_SIZE_PARAM_IS_A_RESERVATION: DWORD = 0x0001_0000;
        let domain = unsafe { __relax_domain_base() };
        let token = unsafe { __relax_domain_reserve(domain) };
        let start = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(StartArg { f, arg, domain, token })) as *mut c_void;
        let h = unsafe { CreateThread(core::ptr::null_mut(), stack_size, windows_thread_start, start, STACK_SIZE_PARAM_IS_A_RESERVATION, core::ptr::null_mut()) };
        if h.is_null() {
            let e = errno();
            drop(unsafe { alloc::boxed::Box::from_raw(start as *mut StartArg) });
            unsafe { __relax_domain_failed(domain, token); }
            return Err(e);
        }
        Ok(NativeThread(h))
    }

    pub fn join_thread(t: NativeThread) {
        unsafe {
            WaitForSingleObject(t.0, INFINITE);
        }
    }

    impl Drop for NativeThread {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    /// Runtime-owned TLS: domain teardown waits until these destructors have completed.
    pub type Key = u32;
    extern "C" {
        fn __relax_key_create(dtor: extern "C" fn(*mut c_void), late: u32) -> u32;
        fn __relax_key_delete(key: u32);
        fn __relax_key_get(key: u32) -> *mut c_void;
        fn __relax_key_set(key: u32, value: *const c_void);
    }
    pub fn key_create(dtor: extern "C" fn(*mut c_void), late: bool) -> Option<Key> { Some(unsafe { __relax_key_create(dtor, late as u32) }) }
    pub fn key_delete(key: Key) { unsafe { __relax_key_delete(key); } }
    pub fn key_get(key: Key) -> *mut c_void { unsafe { __relax_key_get(key) } }
    pub fn key_set(key: Key, value: *const c_void) { unsafe { __relax_key_set(key, value); } }

    fn std_handle(fd: i32) -> HANDLE {
        let which = match fd {
            0 => STD_INPUT_HANDLE,
            1 => STD_OUTPUT_HANDLE,
            _ => STD_ERROR_HANDLE,
        };
        unsafe { GetStdHandle(which) }
    }

    /// Writes to stdout / stderr: a console gets the text as UTF-16 (WriteConsoleW; an incomplete
    /// UTF-8 sequence at the end is left for the next write), anything else the bytes. A missing
    /// handle swallows the output (real std: no console attached is not an error).
    pub fn stdio_write(fd: i32, buf: &[u8]) -> Result<usize, i32> {
        let h = std_handle(fd);
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return Ok(buf.len());
        }
        let mut mode: DWORD = 0;
        unsafe {
            if GetConsoleMode(h, &mut mode) != 0 {
                let valid = match core::str::from_utf8(buf) {
                    Ok(s) => s,
                    Err(e) if e.valid_up_to() > 0 => core::str::from_utf8(&buf[..e.valid_up_to()]).unwrap(),
                    // an invalid or incomplete sequence at the start: one replacement char for it
                    Err(e) => {
                        let bad = e.error_len().unwrap_or(buf.len()).max(1);
                        let r: [u16; 1] = [0xfffd];
                        let mut w: DWORD = 0;
                        if WriteConsoleW(h, r.as_ptr(), 1, &mut w, core::ptr::null_mut()) == 0 {
                            return Err(errno());
                        }
                        return Ok(bad);
                    }
                };
                let w16: Vec<u16> = valid.encode_utf16().collect();
                let mut done = 0usize;
                while done < w16.len() {
                    let mut w: DWORD = 0;
                    let n = (w16.len() - done).min(8192);
                    if WriteConsoleW(h, w16[done..].as_ptr(), n as DWORD, &mut w, core::ptr::null_mut()) == 0 {
                        return Err(errno());
                    }
                    done += w as usize;
                }
                return Ok(valid.len());
            }
            let mut w: DWORD = 0;
            let n = buf.len().min(u32::MAX as usize) as DWORD;
            if WriteFile(h, buf.as_ptr(), n, &mut w, core::ptr::null_mut()) == 0 {
                let e = errno();
                if e == ERROR_INVALID_HANDLE as i32 || e == ERROR_NO_DATA as i32 {
                    return Ok(buf.len());
                }
                return Err(e);
            }
            Ok(w as usize)
        }
    }

    /// Reads stdin; a missing handle or a closed pipe reads as end of file.
    pub fn stdio_read(fd: i32, buf: &mut [u8]) -> Result<usize, i32> {
        let h = std_handle(fd);
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return Ok(0);
        }
        let mut r: DWORD = 0;
        let n = buf.len().min(u32::MAX as usize) as DWORD;
        unsafe {
            if ReadFile(h, buf.as_mut_ptr(), n, &mut r, core::ptr::null_mut()) == 0 {
                let e = errno();
                if e == ERROR_BROKEN_PIPE as i32 || e == ERROR_INVALID_HANDLE as i32 {
                    return Ok(0);
                }
                return Err(e);
            }
        }
        Ok(r as usize)
    }

    pub fn stdio_is_terminal(fd: i32) -> bool {
        let mut mode: DWORD = 0;
        unsafe { GetConsoleMode(std_handle(fd), &mut mode) != 0 }
    }
}
