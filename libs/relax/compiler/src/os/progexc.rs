//! The program's Windows exception handlers run only on program threads outside a compile.
//!
//! A lazy compile runs on the calling thread. An SEH exception is synchronous to the thread
//! that raised it, so an exception inside a compile belongs to the compiler. Nothing needs to
//! be held for later delivery. APCs run only in alertable waits, which no compile makes, and
//! the runtime's watchdog suspends the thread with SuspendThread.
//!
//! The runtime installs the OS handlers. The program's imports register callbacks here. A JIT
//! address registered by a native library itself bypasses this guard, like a raw rt_sigaction
//! on unix.

use super::win32::{ExceptionPointers, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH};
use std::cell::Cell;
use std::marker::PhantomData;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering::SeqCst};
use std::sync::Mutex;

thread_local! {
    static PROGRAM_THREAD: Cell<bool> = const { Cell::new(false) };
}

pub(crate) struct ProgramThread {
    marked: bool,
    // Only the thread that took the guard may clear its mark.
    _thread: PhantomData<*mut ()>,
}

pub(crate) fn program_thread() -> super::ProgramThread {
    let marked = PROGRAM_THREAD.with(|mark| !mark.replace(true));
    ProgramThread { marked, _thread: PhantomData }
}

impl ProgramThread {
    pub(crate) fn end_with_thread(self) {
        drop(self);
    }
}

impl Drop for ProgramThread {
    fn drop(&mut self) {
        if self.marked {
            let _ = PROGRAM_THREAD.try_with(|mark| mark.set(false));
        }
    }
}

pub(crate) fn adopt() {
    let _ = PROGRAM_THREAD.try_with(|mark| mark.set(true));
}

pub(crate) fn on_program_thread() -> bool {
    PROGRAM_THREAD.try_with(Cell::get).unwrap_or(false)
}

fn may_dispatch() -> bool {
    on_program_thread() && !crate::jit::compile_innermost()
}

// A handle is the address of a boxed handler address. Each published snapshot is immutable.
type Snapshot = Vec<usize>;
struct List(AtomicPtr<Snapshot>);

static VECTORED: List = List(AtomicPtr::new(ptr::null_mut()));
static CONTINUE: List = List(AtomicPtr::new(ptr::null_mut()));
static FILTER: AtomicUsize = AtomicUsize::new(0);
static READERS: AtomicUsize = AtomicUsize::new(0);

// The writer owns the unpublished boxes until a reset finds no readers.
#[derive(Default)]
struct Retired {
    snapshots: Vec<usize>,
    entries: Vec<usize>,
}
static WRITER: Mutex<Retired> = Mutex::new(Retired { snapshots: Vec::new(), entries: Vec::new() });

impl List {
    /// The caller holds WRITER or counts in READERS for the lifetime of the slice.
    unsafe fn entries(&self) -> &[usize] {
        self.0.load(SeqCst).as_ref().map_or(&[], Vec::as_slice)
    }

    fn replace(&self, next: Snapshot, retired: &mut Retired) {
        let new = if next.is_empty() { ptr::null_mut() } else { Box::into_raw(Box::new(next)) };
        let old = self.0.swap(new, SeqCst);
        if !old.is_null() {
            retired.snapshots.push(old as usize);
        }
    }

    fn add(&self, first: u32, handler: usize) -> usize {
        if handler == 0 {
            return 0;
        }
        let _kept = crate::zone_alloc::kept();
        let mut retired = WRITER.lock().unwrap_or_else(|e| e.into_inner());
        let current = unsafe { self.entries() };
        let mut next = Vec::with_capacity(current.len() + 1);
        let entry = Box::into_raw(Box::new(handler)) as usize;
        if first != 0 {
            next.push(entry);
            next.extend_from_slice(current);
        } else {
            next.extend_from_slice(current);
            next.push(entry);
        }
        self.replace(next, &mut retired);
        entry
    }

    fn remove(&self, handle: usize) -> u32 {
        let _kept = crate::zone_alloc::kept();
        let mut retired = WRITER.lock().unwrap_or_else(|e| e.into_inner());
        let current = unsafe { self.entries() };
        let Some(index) = current.iter().position(|&entry| entry == handle) else { return 0 };
        let mut next = current.to_vec();
        next.remove(index);
        self.replace(next, &mut retired);
        retired.entries.push(handle);
        1
    }

    fn dispatch(&self, ep: *mut ExceptionPointers) -> i32 {
        if !may_dispatch() {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        // In the SeqCst order, a reader of the old snapshot entered before the writer's swap.
        READERS.fetch_add(1, SeqCst);
        let mut result = EXCEPTION_CONTINUE_SEARCH;
        for &entry in unsafe { self.entries() } {
            let handler = unsafe { *(entry as *const usize) };
            if invoke(handler, ep) == Some(EXCEPTION_CONTINUE_EXECUTION) {
                result = EXCEPTION_CONTINUE_EXECUTION;
                break;
            }
        }
        READERS.fetch_sub(1, SeqCst);
        result
    }
}

fn invoke(addr: usize, ep: *mut ExceptionPointers) -> Option<i32> {
    if addr == 0 || crate::stale::in_ended_run(addr as u64) {
        return None;
    }
    let handler: extern "system" fn(*mut ExceptionPointers) -> i32 = unsafe { std::mem::transmute(addr) };
    Some(handler(ep))
}

pub(crate) extern "system" fn add_vectored_exception_handler(first: u32, h: usize) -> usize {
    VECTORED.add(first, h)
}

pub(crate) extern "system" fn remove_vectored_exception_handler(handle: usize) -> u32 {
    VECTORED.remove(handle)
}

pub(crate) extern "system" fn add_vectored_continue_handler(first: u32, h: usize) -> usize {
    CONTINUE.add(first, h)
}

pub(crate) extern "system" fn remove_vectored_continue_handler(handle: usize) -> u32 {
    CONTINUE.remove(handle)
}

pub(crate) extern "system" fn set_unhandled_exception_filter(f: usize) -> usize {
    FILTER.swap(f, SeqCst)
}

pub(crate) fn dispatch_vectored(ep: *mut ExceptionPointers) -> i32 {
    VECTORED.dispatch(ep)
}

pub(crate) fn dispatch_continue(ep: *mut ExceptionPointers) -> i32 {
    CONTINUE.dispatch(ep)
}

pub(crate) fn program_filter(ep: *mut ExceptionPointers) -> Option<i32> {
    if may_dispatch() { invoke(FILTER.load(SeqCst), ep) } else { None }
}

pub(crate) fn reset_statics() {
    let _kept = crate::zone_alloc::kept();
    let mut retired = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    for list in [&VECTORED, &CONTINUE] {
        retired.entries.extend_from_slice(unsafe { list.entries() });
        list.replace(Vec::new(), &mut retired);
    }
    FILTER.store(0, SeqCst);
    if READERS.load(SeqCst) == 0 {
        let old = std::mem::take(&mut *retired);
        for snapshot in old.snapshots {
            unsafe { drop(Box::from_raw(snapshot as *mut Snapshot)) };
        }
        for entry in old.entries {
            unsafe { drop(Box::from_raw(entry as *mut usize)) };
        }
    }
}
