//! The Win32 API surface of makepad run, for every relax crate (session, base, codegen,
//! compiler): small explicit `extern "system"` declarations hard-linked against the system
//! libraries (no bindings crate, no COM), one signature per function (HANDLE = isize, BOOL = i32,
//! DWORD = u32, SIZE_T = usize, BOOLEAN = u8, memory addresses `*mut u8`), and the x64 CONTEXT
//! record. Nothing else in the relax crates declares a Win32 function.

#![allow(non_snake_case, clippy::upper_case_acronyms)]

pub type HANDLE = isize;

#[link(name = "kernel32")]
extern "system" {
    // memory
    pub fn VirtualAlloc(addr: *mut u8, size: usize, kind: u32, protect: u32) -> *mut u8;
    pub fn VirtualFree(addr: *mut u8, size: usize, kind: u32) -> i32;
    pub fn VirtualProtect(addr: *mut u8, size: usize, protect: u32, old: *mut u32) -> i32;
    pub fn FlushInstructionCache(process: HANDLE, addr: *const u8, size: usize) -> i32;
    // errors
    pub fn GetLastError() -> u32;
    pub fn SetLastError(e: u32);
    // processes, threads
    pub fn GetCurrentProcess() -> HANDLE;
    pub fn GetCurrentThread() -> HANDLE;
    pub fn GetThreadPriority(thread: HANDLE) -> i32;
    pub fn ReadProcessMemory(process: HANDLE, addr: *const u8, buf: *mut u8, size: usize, read: *mut usize) -> i32;
    pub fn GetCurrentThreadId() -> u32;
    pub fn OpenThread(access: u32, inherit: i32, id: u32) -> HANDLE;
    pub fn DuplicateHandle(src_process: HANDLE, src: HANDLE, dst_process: HANDLE, dst: *mut HANDLE, access: u32, inherit: i32, options: u32) -> i32;
    pub fn CloseHandle(h: HANDLE) -> i32;
    pub fn SuspendThread(t: HANDLE) -> u32;
    pub fn ResumeThread(t: HANDLE) -> u32;
    pub fn GetThreadContext(t: HANDLE, ctx: *mut Context) -> i32;
    pub fn SetThreadContext(t: HANDLE, ctx: *const Context) -> i32;
    pub fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
    pub fn SetThreadStackGuarantee(size: *mut u32) -> i32;
    pub fn SetThreadPriority(t: HANDLE, priority: i32) -> i32;
    pub fn GetThreadDescription(t: HANDLE, desc: *mut *mut u16) -> i32;
    pub fn LocalFree(p: *mut u8) -> *mut u8;
    pub fn TerminateProcess(p: HANDLE, code: u32) -> i32;
    // sampler timer
    pub fn CreateWaitableTimerExW(attrs: *const u8, name: *const u16, flags: u32, access: u32) -> HANDLE;
    pub fn SetWaitableTimer(timer: HANDLE, due: *const i64, period: i32, callback: Option<unsafe extern "system" fn(*mut u8, u32, u32)>, arg: *mut u8, resume: i32) -> i32;
    pub fn WaitForSingleObject(handle: HANDLE, ms: u32) -> u32;
    // thread-local storage
    pub fn TlsAlloc() -> u32;
    pub fn TlsGetValue(index: u32) -> *mut u8;
    pub fn TlsSetValue(index: u32, v: *mut u8) -> i32;
    // exceptions
    pub fn AddVectoredExceptionHandler(first: u32, handler: extern "system" fn(*mut ExceptionPointers) -> i32) -> *mut u8;
    pub fn SetUnhandledExceptionFilter(filter: Option<extern "system" fn(*mut ExceptionPointers) -> i32>) -> usize;
    // modules
    pub fn LoadLibraryA(name: *const u8) -> HANDLE;
    pub fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut HANDLE) -> i32;
    pub fn GetModuleFileNameW(module: HANDLE, name: *mut u16, size: u32) -> u32;
    // stdio (fault diagnostics)
    pub fn GetStdHandle(which: u32) -> HANDLE;
    pub fn WriteFile(h: HANDLE, buf: *const u8, n: u32, written: *mut u32, overlapped: *mut u8) -> i32;
    pub fn GetProcAddress(module: HANDLE, name: *const u8) -> *mut u8;
    pub fn K32EnumProcessModules(process: HANDLE, modules: *mut HANDLE, bytes: u32, needed: *mut u32) -> i32;
    // power
    pub fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
    // memory queries
    pub fn VirtualQuery(addr: *const u8, info: *mut MemoryBasicInformation, size: usize) -> usize;
    pub fn K32GetProcessMemoryInfo(process: HANDLE, counters: *mut u8, cb: u32) -> i32;
    // thread times
    pub fn QueryThreadCycleTime(thread: HANDLE, cycles: *mut u64) -> i32;
    pub fn GetThreadTimes(thread: HANDLE, creation: *mut u64, exit: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
    // processes, console
    pub fn OpenProcess(access: u32, inherit: i32, pid: u32) -> HANDLE;
    pub fn GetExitCodeProcess(process: HANDLE, code: *mut u32) -> i32;
    pub fn SetConsoleCtrlHandler(handler: Option<extern "system" fn(u32) -> i32>, add: i32) -> i32;
    // files, directory change notification
    pub fn CreateFileW(name: *const u16, access: u32, share: u32, security: *mut u8, disposition: u32, flags: u32, template: HANDLE) -> HANDLE;
    pub fn ReadDirectoryChangesW(dir: HANDLE, buf: *mut u8, len: u32, subtree: i32, filter: u32, returned: *mut u32, overlapped: *mut Overlapped, routine: *mut u8) -> i32;
    pub fn GetOverlappedResult(file: HANDLE, overlapped: *mut Overlapped, bytes: *mut u32, wait: i32) -> i32;
    pub fn CancelIoEx(file: HANDLE, overlapped: *mut Overlapped) -> i32;
    pub fn CreateDirectoryW(name: *const u16, attributes: *const SecurityAttributes) -> i32;
    pub fn GetDriveTypeW(root: *const u16) -> u32;
    pub fn CompareStringOrdinal(a: *const u16, a_len: i32, b: *const u16, b_len: i32, ignore_case: i32) -> i32;
}

#[link(name = "ntdll")]
extern "system" {
    pub fn RtlAddFunctionTable(table: *const u32, count: u32, base: u64) -> u8;
    pub fn RtlDeleteFunctionTable(table: *const u32) -> u8;
}

#[link(name = "advapi32")]
extern "system" {
    pub fn OpenProcessToken(process: HANDLE, access: u32, token: *mut HANDLE) -> i32;
    pub fn GetTokenInformation(token: HANDLE, class: u32, info: *mut u8, size: u32, needed: *mut u32) -> i32;
    pub fn GetNamedSecurityInfoW(name: *const u16, kind: u32, info: u32, owner: *mut *mut u8,
        group: *mut *mut u8, dacl: *mut *mut u8, sacl: *mut *mut u8, sd: *mut *mut u8) -> u32;
    pub fn SetNamedSecurityInfoW(name: *mut u16, kind: u32, info: u32, owner: *mut u8,
        group: *mut u8, dacl: *mut u8, sacl: *mut u8) -> u32;
    pub fn EqualSid(a: *const u8, b: *const u8) -> i32;
    pub fn IsValidSid(sid: *const u8) -> i32;
    pub fn IsValidAcl(acl: *const u8) -> i32;
    pub fn GetAce(acl: *const u8, index: u32, ace: *mut *mut u8) -> i32;
    pub fn InitializeAcl(acl: *mut u8, size: u32, revision: u32) -> i32;
    pub fn AddAccessAllowedAceEx(acl: *mut u8, revision: u32, flags: u32, mask: u32, sid: *const u8) -> i32;
    pub fn InitializeSecurityDescriptor(sd: *mut u8, revision: u32) -> i32;
    pub fn SetSecurityDescriptorDacl(sd: *mut u8, present: i32, acl: *mut u8, defaulted: i32) -> i32;
    pub fn SetSecurityDescriptorControl(sd: *mut u8, bits: u16, value: u16) -> i32;
}

#[link(name = "bcryptprimitives", kind = "raw-dylib")]
extern "system" {
    pub fn ProcessPrng(buf: *mut u8, len: usize) -> i32;
}

/// winnt.h MEMORY_BASIC_INFORMATION (x64)
#[repr(C)]
pub struct MemoryBasicInformation {
    pub base: *mut u8,
    pub allocation_base: *mut u8,
    pub allocation_protect: u32,
    pub region_size: usize,
    pub state: u32,
    pub protect: u32,
    pub kind: u32,
}

/// minwinbase.h OVERLAPPED
#[repr(C)]
pub struct Overlapped {
    pub internal: usize,
    pub internal_high: usize,
    pub offset: u32,
    pub offset_high: u32,
    pub event: HANDLE,
}

/// minwinbase.h SECURITY_ATTRIBUTES
#[repr(C)]
pub struct SecurityAttributes {
    pub length: u32,
    pub descriptor: *mut u8,
    pub inherit: i32,
}

pub const MEM_COMMIT: u32 = 0x1000;
pub const MEM_RESERVE: u32 = 0x2000;
pub const MEM_RELEASE: u32 = 0x8000;
pub const MEM_DECOMMIT: u32 = 0x4000;
pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_READWRITE: u32 = 0x04;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;

pub const THREAD_SUSPEND_RESUME: u32 = 0x0002;
pub const THREAD_GET_CONTEXT: u32 = 0x0008;
pub const THREAD_SET_CONTEXT: u32 = 0x0010;
pub const THREAD_QUERY_LIMITED_INFORMATION: u32 = 0x0800;
pub const DUPLICATE_SAME_ACCESS: u32 = 0x2;

pub const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
pub const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

/// winnt.h EXCEPTION_RECORD (x64)
#[repr(C)]
pub struct ExceptionRecord {
    pub code: u32,
    pub flags: u32,
    pub record: *mut ExceptionRecord,
    pub address: *mut u8,
    pub n_params: u32,
    pub info: [usize; 15],
}

#[repr(C)]
pub struct ExceptionPointers {
    pub record: *mut ExceptionRecord,
    pub context: *mut Context,
}

/// winnt.h CONTEXT (AMD64): 0x4d0 bytes, 16-aligned. The fields the runtime reads and writes are
/// accessed by their offsets.
#[repr(C, align(16))]
pub struct Context(pub [u8; 0x4d0]);

pub const CONTEXT_AMD64: u32 = 0x0010_0000;
pub const CONTEXT_CONTROL: u32 = CONTEXT_AMD64 | 0x1;
pub const CONTEXT_INTEGER: u32 = CONTEXT_AMD64 | 0x2;

// offsets in CONTEXT: P1Home..P6Home 0x00, ContextFlags 0x30, MxCsr 0x34, Seg* 0x38, EFlags 0x44,
// Dr0-7 0x48, Rax 0x78, Rcx 0x80, Rdx 0x88, Rbx 0x90, Rsp 0x98, Rbp 0xa0, Rsi 0xa8, Rdi 0xb0,
// R8-R15 0xb8.., Rip 0xf8
pub const CTX_FLAGS: usize = 0x30;
pub const CTX_RSP: usize = 0x98;
pub const CTX_RBP: usize = 0xa0;
pub const CTX_RSI: usize = 0xa8;
pub const CTX_RDI: usize = 0xb0;
pub const CTX_RIP: usize = 0xf8;

impl Context {
    pub fn new(flags: u32) -> Box<Context> {
        let mut c = Box::new(Context([0; 0x4d0]));
        c.0[CTX_FLAGS..CTX_FLAGS + 4].copy_from_slice(&flags.to_le_bytes());
        c
    }
    pub fn get(&self, off: usize) -> u64 {
        u64::from_le_bytes(self.0[off..off + 8].try_into().unwrap())
    }
    pub fn set(&mut self, off: usize, v: u64) {
        self.0[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }
}

#[repr(C)]
#[derive(Default)]
pub struct SystemPowerStatus {
    pub ac_line_status: u8,
    pub battery_flag: u8,
    pub battery_life_percent: u8,
    pub system_status_flag: u8,
    pub battery_life_time: u32,
    pub battery_full_life_time: u32,
}

#[link(name = "winmm")]
extern "system" {
    pub fn timeBeginPeriod(ms: u32) -> u32;
    pub fn timeEndPeriod(ms: u32) -> u32;
}

pub const CREATE_WAITABLE_TIMER_HIGH_RESOLUTION: u32 = 0x2;
pub const TIMER_MODIFY_STATE: u32 = 0x2;
pub const SYNCHRONIZE: u32 = 0x0010_0000;
