//! Copy a bounded stack while suspended, resume, then use StackWalk64 on the
//! copy. DbgHelp/loader/heap operations must never run with the UI suspended.
//! Minimal SDK ABI here because the repository's stripped windows bindings
//! omit DbgHelp. No new dependency or symbol-server download.
use std::{cell::Cell, ffi::{c_void, CStr}, sync::Mutex};
type Handle = *mut c_void;
#[repr(C, align(16))]
struct Context([u64; 256]);
#[repr(C)]
#[derive(Default)]
struct Address { offset: u64, segment: u16, mode: u32 }
#[repr(C)]
#[derive(Default)]
struct StackFrame {
    pc: Address, ret: Address, frame: Address, stack: Address, backing: Address,
    function: usize, params: [u64; 4], far: i32, virtual_frame: i32,
    reserved: [u64; 3], kdhelp: [u64; 14],
}
#[repr(C)]
struct Symbol {
    size: u32, type_index: u32, reserved: [u64; 2], index: u32, symbol_size: u32,
    module: u64, flags: u32, value: u64, address: u64, register: u32, scope: u32,
    tag: u32, name_len: u32, max_name: u32, name: [u8; 1024],
}
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn GetCurrentThread() -> Handle;
    fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
    fn DuplicateHandle(source: Handle, handle: Handle, target: Handle, result: *mut Handle, access: u32, inherit: i32, options: u32) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn SuspendThread(thread: Handle) -> u32;
    fn ResumeThread(thread: Handle) -> u32;
    fn GetThreadContext(thread: Handle, context: *mut Context) -> i32;
    fn ReadProcessMemory(process: Handle, source: *const c_void, destination: *mut c_void, size: usize, read: *mut usize) -> i32;
}
#[link(name = "dbghelp")]
extern "system" {
    fn SymInitializeW(process: Handle, path: *const u16, invade: i32) -> i32;
    fn SymCleanup(process: Handle) -> i32;
    fn SymFromAddr(process: Handle, address: u64, displacement: *mut u64, symbol: *mut Symbol) -> i32;
    fn SymFunctionTableAccess64(process: Handle, address: u64) -> *mut c_void;
    fn SymGetModuleBase64(process: Handle, address: u64) -> u64;
    fn StackWalk64(machine: u32, process: Handle, thread: Handle, frame: *mut StackFrame, context: *mut Context,
        read: Option<unsafe extern "system" fn(Handle, u64, *mut c_void, u32, *mut u32) -> i32>,
        functions: Option<unsafe extern "system" fn(Handle, u64) -> *mut c_void>,
        modules: Option<unsafe extern "system" fn(Handle, u64) -> u64>, translate: *mut c_void) -> i32;
}
static DBGHELP: Mutex<()> = Mutex::new(());
struct Snapshot { start: usize, bytes: Vec<u8>, bottom: usize, top: usize }
thread_local! { static SNAPSHOT: Cell<*const Snapshot> = const { Cell::new(std::ptr::null()) }; }

pub(super) struct Target { thread: usize, process: usize, bottom: usize, top: usize, symbols: Cell<bool> }
impl Target {
    pub(super) fn current() -> Option<Self> {
        unsafe {
            let process = GetCurrentProcess();
            let mut thread_handle = std::ptr::null_mut();
            let mut process_handle = std::ptr::null_mut();
            if DuplicateHandle(process, GetCurrentThread(), process, &mut thread_handle, 0, 0, 2) == 0 { return None; }
            if DuplicateHandle(process, process, process, &mut process_handle, 0, 0, 2) == 0 {
                CloseHandle(thread_handle); return None;
            }
            let (mut bottom, mut top) = (0, 0);
            GetCurrentThreadStackLimits(&mut bottom, &mut top);
            Some(Self { thread: thread_handle as usize, process: process_handle as usize, bottom, top, symbols: Cell::new(false) })
        }
    }
    fn initialize_symbols(&self) {
        if !self.symbols.get() {
            self.symbols.set(unsafe { SymInitializeW(self.process as Handle, [0u16].as_ptr(), 1) != 0 });
        }
    }
    pub(super) fn sample(&self) -> Vec<usize> {
        let mut context = Context([0; 256]);
        #[cfg(target_arch = "x86_64")]
        let (machine, pc, sp, fp) = { context.0[6] = 0x100003; (0x8664, 31, 19, 20) };
        #[cfg(target_arch = "aarch64")]
        let (machine, pc, sp, fp) = { context.0[0] = 0x400003; (0xaa64, 33, 32, 30) };
        let mut snapshot = Snapshot { start: 0, bytes: vec![0; 256 * 1024], bottom: self.bottom, top: self.top };
        let mut captured = false;
        let mut read = 0;
        unsafe {
            if SuspendThread(self.thread as Handle) == u32::MAX { return Vec::new(); }
            if GetThreadContext(self.thread as Handle, &mut context) != 0 {
                captured = true;
                snapshot.start = context.0[sp] as usize;
                if snapshot.start >= self.bottom && snapshot.start < self.top {
                    let len = snapshot.bytes.len().min(self.top - snapshot.start);
                    ReadProcessMemory(self.process as Handle, snapshot.start as *const c_void, snapshot.bytes.as_mut_ptr().cast(), len, &mut read);
                }
            }
            ResumeThread(self.thread as Handle);
        }
        if !captured { return Vec::new(); }
        snapshot.bytes.truncate(read);
        let mut frames = vec![context.0[pc] as usize];
        let Ok(_symbols) = DBGHELP.lock() else { return frames; };
        self.initialize_symbols();
        let mut frame = StackFrame::default();
        frame.pc = Address { offset: context.0[pc], mode: 3, ..Default::default() };
        frame.frame = Address { offset: context.0[fp], mode: 3, ..Default::default() };
        frame.stack = Address { offset: context.0[sp], mode: 3, ..Default::default() };
        SNAPSHOT.with(|s| s.set(&snapshot));
        for _ in 0..63 {
            let ok = unsafe { StackWalk64(machine, self.process as Handle, self.thread as Handle,
                &mut frame, &mut context, Some(read_snapshot), Some(SymFunctionTableAccess64), Some(SymGetModuleBase64), std::ptr::null_mut()) };
            if ok == 0 || frame.pc.offset == 0 { break; }
            let address = frame.pc.offset as usize;
            if frames.last() != Some(&address) { frames.push(address); }
        }
        SNAPSHOT.with(|s| s.set(std::ptr::null()));
        frames
    }
    pub(super) fn symbolize(&self, pc: usize) -> String {
        let Ok(_symbols) = DBGHELP.lock() else { return format!("0x{pc:x}"); };
        self.initialize_symbols();
        unsafe {
            let mut symbol: Symbol = std::mem::zeroed();
            // sizeof(SYMBOL_INFO), including Name[1] and tail alignment.
            symbol.size = 88;
            symbol.max_name = 1023;
            let mut displacement = 0;
            if SymFromAddr(self.process as Handle, pc as u64, &mut displacement, &mut symbol) != 0 {
                symbol.name[1023] = 0;
                format!("{}+0x{displacement:x}", super::demangle::demangle(&CStr::from_ptr(symbol.name.as_ptr().cast()).to_string_lossy()))
            } else { format!("0x{pc:x}") }
        }
    }
}
impl Drop for Target {
    fn drop(&mut self) {
        unsafe {
            if let Ok(_symbols) = DBGHELP.lock() {
                if self.symbols.get() { SymCleanup(self.process as Handle); }
            }
            CloseHandle(self.thread as Handle);
            CloseHandle(self.process as Handle);
        }
    }
}

unsafe extern "system" fn read_snapshot(process: Handle, address: u64, output: *mut c_void, len: u32, count: *mut u32) -> i32 {
    let ptr = SNAPSHOT.with(Cell::get);
    if ptr.is_null() { return 0; }
    let snapshot = &*ptr;
    let address = address as usize;
    let Some(end) = address.checked_add(len as usize) else { return 0; };
    if address >= snapshot.start && end <= snapshot.start + snapshot.bytes.len() {
        std::ptr::copy_nonoverlapping(snapshot.bytes.as_ptr().add(address - snapshot.start), output.cast(), len as usize);
        if !count.is_null() { *count = len; }
        return 1;
    }
    // Never fall back to the live thread's changing stack. Module unwind
    // metadata outside that interval can be read safely after resumption.
    if address < snapshot.top && end > snapshot.bottom { return 0; }
    let mut read = 0;
    let ok = ReadProcessMemory(process, address as *const c_void, output, len as usize, &mut read);
    if !count.is_null() { *count = read as u32; }
    ok
}
