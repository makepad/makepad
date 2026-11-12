//! Runtime-owned Relax std TLS. Drain JIT destructors before retiring a domain thread; OS key
//! destructor ordering otherwise lets the domain block disappear before a later key uses it.
use std::ffi::c_void;

/// The thread's record (word 1 of its thread-local block). The first three words are read by
/// Relax std's `LocalKey` (std/thread/local.rs) as an inline fast path -- the layout is shared:
/// the values of the domain `cache_base` (its key table: `cache_vals`, `cache_len` entries).
/// The table moves only in `key_set`, which refreshes the cache; any other domain's access
/// goes through `key_get`.
#[repr(C)]
pub(crate) struct ThreadData {
    cache_base: u64,
    cache_vals: *const usize,
    cache_len: usize,
    pub registrations: Vec<(u64, u64)>,
    values: Vec<(u64, Vec<usize>)>,
    /// (domain, key) in the order this thread first set each key: destructors run last-first, as
    /// std's (library/std/src/sys/thread_local/destructors/list.rs: registered at a key's first
    /// initialisation on the thread, run by popping the list). A key's value never returns to 0
    /// (its destructor leaves LocalKey's DESTROYED marker), so each key is registered once.
    order: Vec<(u64, u32)>,
}

impl ThreadData {
    fn refresh(&mut self, base: u64) {
        match self.values.iter().find(|v| v.0 == base) {
            Some(v) => {
                self.cache_base = base;
                self.cache_vals = v.1.as_ptr();
                self.cache_len = v.1.len();
            }
            None => self.cache_len = 0,
        }
    }
}

pub(crate) unsafe fn data(block: u64) -> &'static mut ThreadData {
    let slot = (block as *mut u64).add(1);
    if *slot == 0 {
        // the thread's record lives as long as the thread, whatever phase it is in when it is
        // first asked for: the kept heap
        let _kept = crate::zone_alloc::kept();
        *slot = Box::into_raw(Box::new(ThreadData {
            cache_base: 0,
            cache_vals: std::ptr::null(),
            cache_len: 0,
            registrations: Vec::new(),
            values: Vec::new(),
            order: Vec::new(),
        })) as u64;
    }
    &mut *(*slot as *mut ThreadData)
}

/// `late`: the key's type has no drop glue. As rustc's thread-locals of such types (never
/// destroyed), its value stays readable in every other key's destructor; its `dtor` only frees
/// the slot, after the last of those ran (`finish`).
pub extern "C" fn key_create(dtor: usize, late: u32) -> u32 {
    crate::domain::tls_key_create(crate::domain::current(), dtor, late != 0)
}
pub extern "C" fn key_delete(key: u32) {
    crate::domain::tls_key_delete(crate::domain::current(), key);
}
pub extern "C" fn key_get(key: u32) -> *mut c_void {
    let base = crate::domain::current();
    unsafe {
        let t = data(crate::jit::tls_block());
        // the next reads of this domain's keys take the inline path
        if t.cache_base != base || t.cache_len == 0 {
            t.refresh(base);
        }
        t.values
            .iter()
            .find(|v| v.0 == base)
            .and_then(|v| v.1.get(key as usize))
            .copied()
            .unwrap_or(0) as *mut c_void
    }
}
pub extern "C" fn key_set(key: u32, value: *const c_void) {
    unsafe {
        let _kept = crate::zone_alloc::kept();
        let base = crate::domain::current();
        let t = data(crate::jit::tls_block());
        let i = match t.values.iter().position(|v| v.0 == base) {
            Some(i) => i,
            None => {
                t.values.push((base, Vec::new()));
                t.values.len() - 1
            }
        };
        let values = &mut t.values[i].1;
        if values.len() <= key as usize {
            values.resize(key as usize + 1, 0);
        }
        if values[key as usize] == 0 && value as usize != 0 {
            t.order.push((base, key));
        }
        values[key as usize] = value as usize;
        t.refresh(base);
    }
}

/// The TLS block is installed while callbacks run, and the registration stays live even if a
/// destructor blocks forever. No arbitrary destructor limit drops app data.
pub(crate) unsafe fn finish(block: u64) -> Vec<(u64, u64)> {
    let slot = (block as *mut u64).add(1);
    if *slot == 0 {
        return Vec::new();
    }
    let p = *slot as *mut ThreadData;
    // std's destructor list (destructors/list.rs `run`): keys with drop glue, last-registered
    // first; a destructor that initialises another key registers it and it runs next. LocalKey's
    // destructor marks its key DESTROYED for good (native/lazy.rs `destroy`: later access fails
    // with AccessError, nothing re-initialises it). Late keys (no drop glue: std registers no
    // destructor, the value is never destroyed) stay set and readable meanwhile.
    // (A destructor in the code of a run that ended never runs: the values go with the domain.)
    while let Some((key_base, key)) = (*p).order.pop() {
        let Some((dtor, false)) = crate::domain::tls_destructor(key_base, key) else { continue };
        if crate::stale::in_ended_run(dtor as u64) {
            continue;
        }
        // End the borrow before JIT runs: the destructor can initialise another TLS key.
        let value = (*p)
            .values
            .iter()
            .find(|v| v.0 == key_base)
            .and_then(|v| v.1.get(key as usize).copied())
            .unwrap_or(0);
        if value == 0 {
            continue;
        }
        crate::domain::enter(key_base);
        let f: extern "C" fn(*mut c_void) = std::mem::transmute(dtor);
        f(value as *mut c_void);
    }
    // No program code runs any more: the late keys' slots are freed (their destructor drops no
    // value, so it touches no other key).
    let late: Vec<(u64, usize, usize)> = (*p)
        .values
        .iter()
        .flat_map(|(base, values)| {
            values
                .iter()
                .enumerate()
                .filter(|(_, v)| **v != 0)
                .map(move |(key, v)| (*base, key, *v))
        })
        .filter_map(|(base, key, v)| match crate::domain::tls_destructor(base, key as u32) {
            Some((dtor, true)) if !crate::stale::in_ended_run(dtor as u64) => Some((base, dtor, v)),
            _ => None,
        })
        .collect();
    for (key_base, dtor, value) in late {
        crate::domain::enter(key_base);
        let f: extern "C" fn(*mut c_void) = std::mem::transmute(dtor);
        f(value as *mut c_void);
    }
    *slot = 0;
    let registrations = std::mem::take(&mut (*p).registrations);
    drop(Box::from_raw(p));
    crate::zone_alloc::domain::enter(std::ptr::null());
    registrations
}

// ---- the program's threads

/// A thread the program starts: native code of the runtime starts it and runs around its JIT code,
/// so it always has somewhere to go when the run ends under it. No JIT frame is on its OS stack.
struct Start {
    enter: u64,
    base: u64,
    token: u64,
    body: Body,
}

enum Body {
    /// Relax std's thread body: `f(arg)`, Relax's ABI
    Relax { f: u64, arg: u64 },
    /// a start routine the program gave the OS's own call (`pthread_create`, `CreateThread`)
    Os(OsBody),
}

#[repr(C)]
struct OsBody {
    f: OsEntry,
    arg: *mut c_void,
    ret: OsRet,
}

#[cfg(unix)]
type OsRet = *mut c_void;
#[cfg(windows)]
type OsRet = u32;
#[cfg(unix)]
type OsEntry = extern "C" fn(*mut c_void) -> *mut c_void;
#[cfg(windows)]
type OsEntry = extern "system" fn(*mut c_void) -> u32;

// The OS's start routine on Relax's ABI: what `run_thread_body` can call.
#[cfg(all(unix, target_arch = "x86_64"))]
extern "sysv64" fn os_body(b: *mut OsBody) {
    unsafe { (*b).ret = ((*b).f)((*b).arg) };
}
#[cfg(all(unix, not(target_arch = "x86_64")))]
extern "C" fn os_body(b: *mut OsBody) {
    unsafe { (*b).ret = ((*b).f)((*b).arg) };
}
#[cfg(windows)]
extern "sysv64" fn os_body(b: *mut OsBody) {
    unsafe { (*b).ret = ((*b).f)((*b).arg) };
}

fn thread_main(start: *mut Start) -> OsRet {
    let s = {
        let _kept = crate::zone_alloc::kept();
        *unsafe { Box::from_raw(start) }
    };
    crate::jit::ensure_altstack();
    crate::domain::relax_domain_enter(s.base, s.token);
    // a thread of the program: its signal handlers run here (os/progsig.rs)
    let _program = crate::os::program_thread();
    let on_stack = |enter: u64, f: u64, arg: u64| {
        crate::stack::on_program_stack(crate::jit::program_stack_changed, || crate::jit::run_thread_body(enter, f, arg));
    };
    match s.body {
        Body::Relax { f, arg } => {
            on_stack(s.enter, f, arg);
            unsafe { std::mem::zeroed::<OsRet>() }
        }
        Body::Os(mut b) => {
            on_stack(s.enter, os_body as *const () as usize as u64, &mut b as *mut OsBody as u64);
            b.ret
        }
    }
}

#[cfg(unix)]
extern "C" fn os_thread_start(p: *mut c_void) -> *mut c_void {
    thread_main(p as *mut Start)
}
#[cfg(windows)]
extern "system" fn os_thread_start(p: *mut c_void) -> u32 {
    thread_main(p as *mut Start)
}

/// The record a new thread starts from, with the place it holds in the domain of the calling thread.
fn new_start(body: Body) -> *mut Start {
    let base = crate::domain::current();
    let token = crate::domain::relax_domain_reserve(base);
    let enter = unsafe { (*crate::jit::cur_unit()).enter };
    let _kept = crate::zone_alloc::kept();
    Box::into_raw(Box::new(Start { enter, base, token, body }))
}

/// The thread did not start: its record and its place go.
unsafe fn start_failed(start: *mut Start) {
    let s = {
        let _kept = crate::zone_alloc::kept();
        *Box::from_raw(start)
    };
    crate::domain::thread_finished(s.base, s.token);
}

#[cfg(unix)]
extern "C" {
    #[link_name = "pthread_create"]
    fn os_pthread_create(t: *mut u64, attr: *const c_void, f: OsEntry, arg: *mut c_void) -> i32;
    fn pthread_attr_init(attr: *mut u64) -> i32;
    fn pthread_attr_setstacksize(attr: *mut u64, size: usize) -> i32;
    fn pthread_attr_destroy(attr: *mut u64) -> i32;
}

/// For Relax std (`__relax_thread_spawn`): starts a thread running `f(arg)` (Relax's ABI) with a
/// stack of at least `stack_size` bytes. Writes its native handle; returns 0 or the OS's error.
#[cfg(unix)]
pub extern "C" fn spawn(out: *mut u64, stack_size: usize, f: u64, arg: u64) -> i32 {
    crate::jit::host_boundary("relax_thread_spawn", move || unsafe {
        let start = new_start(Body::Relax { f, arg });
        // (64 bytes hold pthread_attr_t on every unix Relax runs on)
        let mut attr = [0u64; 8];
        let a = attr.as_mut_ptr();
        let mut r = pthread_attr_init(a);
        if r == 0 {
            const PAGE: usize = 16 * 1024;
            pthread_attr_setstacksize(a, stack_size.div_ceil(PAGE) * PAGE);
            r = os_pthread_create(out, a as *const c_void, os_thread_start, start as *mut c_void);
            pthread_attr_destroy(a);
        }
        if r != 0 {
            start_failed(start);
        }
        r
    })
}

/// `pthread_create` as JIT code calls it: a start routine in JIT code runs inside the runtime's
/// native thread start, with its place in the domain. Native code's goes to the OS as it is.
#[cfg(unix)]
pub extern "C" fn pthread_create(t: *mut u64, attr: *const c_void, f: OsEntry, arg: *mut c_void) -> i32 {
    crate::jit::host_boundary("pthread_create", move || unsafe {
        if !crate::jit::in_jit_code(f as *const () as usize as u64) {
            return os_pthread_create(t, attr, f, arg);
        }
        let start = new_start(Body::Os(OsBody { f, arg, ret: std::ptr::null_mut() }));
        let r = os_pthread_create(t, attr, os_thread_start, start as *mut c_void);
        if r != 0 {
            start_failed(start);
        }
        r
    })
}

#[cfg(windows)]
extern "system" {
    #[link_name = "CreateThread"]
    fn os_create_thread(attrs: *mut c_void, stack: usize, f: OsEntry, arg: *mut c_void, flags: u32, id: *mut u32) -> *mut c_void;
    fn GetLastError() -> u32;
}

/// For Relax std (`__relax_thread_spawn`): as the unix one, with `stack_size` bytes reserved
/// (STACK_SIZE_PARAM_IS_A_RESERVATION, as real std).
#[cfg(windows)]
pub extern "C" fn spawn(out: *mut u64, stack_size: usize, f: u64, arg: u64) -> i32 {
    crate::jit::host_boundary("relax_thread_spawn", move || unsafe {
        const STACK_SIZE_PARAM_IS_A_RESERVATION: u32 = 0x0001_0000;
        let start = new_start(Body::Relax { f, arg });
        let h = os_create_thread(std::ptr::null_mut(), stack_size, os_thread_start, start as *mut c_void, STACK_SIZE_PARAM_IS_A_RESERVATION, std::ptr::null_mut());
        if h.is_null() {
            let e = GetLastError() as i32;
            start_failed(start);
            return e;
        }
        *out = h as u64;
        0
    })
}

/// `CreateThread` as JIT code calls it (see `pthread_create`).
#[cfg(windows)]
pub extern "system" fn create_thread(attrs: *mut c_void, stack: usize, f: OsEntry, arg: *mut c_void, flags: u32, id: *mut u32) -> *mut c_void {
    crate::jit::host_boundary("CreateThread", move || unsafe {
        if !crate::jit::in_jit_code(f as *const () as usize as u64) {
            return os_create_thread(attrs, stack, f, arg, flags, id);
        }
        let start = new_start(Body::Os(OsBody { f, arg, ret: 0 }));
        let h = os_create_thread(attrs, stack, os_thread_start, start as *mut c_void, flags, id);
        if h.is_null() {
            start_failed(start);
        }
        h
    })
}

// ---- the program's waits on Windows

/// No signal ends a wait on Windows, so the calls JIT code waits with come here (`lookup_interposer`):
/// the same wait, which the end of the thread's run ends as well. Back in JIT code the thread faults
/// and ends as on unix, so it never sees what the wait returned. Blocking paths only: nothing on the
/// way into the system call but this.
#[cfg(windows)]
pub(crate) mod waits {
    use crate::os::win32::*;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[link(name = "api-ms-win-core-synch-l1-2-0", kind = "raw-dylib")]
    extern "system" {
        fn WaitOnAddress(addr: *const c_void, cmp: *const c_void, size: usize, ms: u32) -> i32;
        fn WakeByAddressAll(addr: *const c_void);
    }

    const MAXIMUM_WAIT_OBJECTS: usize = 64;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_FAILED: u32 = u32::MAX;
    const WAIT_TIMEOUT: u32 = 258;
    const INFINITE: u32 = u32::MAX;

    thread_local! {
        /// The address the thread waits on, for the run's end to wake.
        static WAITING: AtomicUsize = const { AtomicUsize::new(0) };
    }

    pub(crate) fn cell() -> usize {
        WAITING.with(|w| w as *const AtomicUsize as usize)
    }

    /// Wakes whatever waits on the address the thread of `cell` waits on: only threads of the run
    /// can, and a wait on an address may always wake for nothing. Its next kick catches a thread on
    /// its way there.
    pub(crate) fn wake(cell: usize) {
        if cell != 0 {
            let at = unsafe { &*(cell as *const AtomicUsize) }.load(Ordering::Acquire);
            if at != 0 {
                unsafe { WakeByAddressAll(at as *const c_void) };
            }
        }
    }

    pub(crate) extern "system" fn wait_on_address(addr: *const c_void, cmp: *const c_void, size: usize, ms: u32) -> i32 {
        WAITING.with(|w| w.store(addr as usize, Ordering::Release));
        let r = unsafe { WaitOnAddress(addr, cmp, size, ms) };
        WAITING.with(|w| w.store(0, Ordering::Release));
        r
    }

    /// The wait for `handles` (none: for the time alone), or the event of the run's end.
    fn wait(handles: &[HANDLE], all: bool, ms: u32, alertable: i32) -> u32 {
        let unit = crate::jit::cur_unit();
        let end = if unit.is_null() { 0 } else { unsafe { (*unit).end_event } };
        let plain = || unsafe {
            if handles.is_empty() {
                SleepEx(ms, alertable)
            } else {
                WaitForMultipleObjectsEx(handles.len() as u32, handles.as_ptr(), all as i32, ms, alertable)
            }
        };
        // (a wait for all of them has no room for "or the run ended"; it can be named on the ledger)
        if end == 0 || all || handles.len() >= MAXIMUM_WAIT_OBJECTS {
            return plain();
        }
        let mut set = [0 as HANDLE; MAXIMUM_WAIT_OBJECTS];
        set[..handles.len()].copy_from_slice(handles);
        set[handles.len()] = end;
        let start = std::time::Instant::now();
        let n = handles.len() as u32;
        let r = unsafe { WaitForMultipleObjectsEx(n + 1, set.as_ptr(), 0, ms, alertable) };
        if r != WAIT_OBJECT_0 + n {
            return if handles.is_empty() && r == WAIT_TIMEOUT { 0 } else { r };
        }
        if crate::domain::current_is_ending() {
            return WAIT_FAILED;
        }
        // a thread of this unit that is not in the run (the compile domain's): its wait goes on
        if ms == INFINITE {
            return plain();
        }
        let spent = start.elapsed().as_millis().min(ms as u128) as u32;
        let rest = ms - spent;
        unsafe {
            if handles.is_empty() {
                SleepEx(rest, alertable)
            } else {
                WaitForMultipleObjectsEx(n, handles.as_ptr(), 0, rest, alertable)
            }
        }
    }

    pub(crate) extern "system" fn wait_for_single_object(h: HANDLE, ms: u32) -> u32 {
        wait(&[h], false, ms, 0)
    }
    pub(crate) extern "system" fn wait_for_single_object_ex(h: HANDLE, ms: u32, alertable: i32) -> u32 {
        wait(&[h], false, ms, alertable)
    }
    pub(crate) extern "system" fn wait_for_multiple_objects(n: u32, hs: *const HANDLE, all: i32, ms: u32) -> u32 {
        wait(unsafe { std::slice::from_raw_parts(hs, n as usize) }, all != 0, ms, 0)
    }
    pub(crate) extern "system" fn wait_for_multiple_objects_ex(n: u32, hs: *const HANDLE, all: i32, ms: u32, alertable: i32) -> u32 {
        wait(unsafe { std::slice::from_raw_parts(hs, n as usize) }, all != 0, ms, alertable)
    }
    pub(crate) extern "system" fn sleep(ms: u32) {
        wait(&[], false, ms, 0);
    }
    pub(crate) extern "system" fn sleep_ex(ms: u32, alertable: i32) -> u32 {
        wait(&[], false, ms, alertable)
    }
}
