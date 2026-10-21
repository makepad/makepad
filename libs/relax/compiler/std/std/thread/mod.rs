//! std::thread over pthreads. A spawned thread runs its closure inside the runtime's
//! `catch_panic`, so a panic ends only that thread and `join()` returns Err, as in real std.

use alloc::boxed::Box;
use alloc::ffi::CString;
use alloc::string::String;
use alloc::sync::Arc;
use core::any::Any;
use core::cell::UnsafeCell;
use core::fmt;
use core::marker::PhantomData;
use core::num::NonZeroUsize;
use core::sync::atomic::Ordering::{Relaxed, SeqCst};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use core::time::Duration;

use crate::io;
use crate::sync::raw::Parker;
use crate::sys;

mod local;

pub use self::local::{AccessError, LocalKey};

pub type Result<T> = core::result::Result<T, Box<dyn Any + Send + 'static>>;

const DEFAULT_MIN_STACK_SIZE: usize = 2 * 1024 * 1024;

/// The stack of a thread spawned without `Builder::stack_size`: `RUST_MIN_STACK` (bytes) when it is set and
/// parses, else 2 MiB; read once (library/std/src/thread/lifecycle.rs: 0 is the not-yet-read sentinel).
fn min_stack() -> usize {
    static MIN: AtomicUsize = AtomicUsize::new(0);
    match MIN.load(Relaxed) {
        0 => {}
        n => return n - 1,
    }
    let amt = crate::env::var_os("RUST_MIN_STACK")
        .and_then(|s| s.to_str().and_then(|s| s.parse().ok()))
        .unwrap_or(DEFAULT_MIN_STACK_SIZE);
    MIN.store(amt + 1, Relaxed);
    amt
}

// ---- ThreadId / Thread

#[derive(Eq, PartialEq, Clone, Copy, Hash, Debug, Ord, PartialOrd)]
pub struct ThreadId(core::num::NonZeroU64);

static THREAD_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

impl ThreadId {
    fn new() -> ThreadId {
        let id = THREAD_ID_COUNTER.fetch_add(1, Relaxed) + 1;
        match core::num::NonZeroU64::new(id) {
            Some(n) => ThreadId(n),
            None => panic!("failed to generate unique thread ID: bitspace exhausted"),
        }
    }
    pub fn as_u64(&self) -> core::num::NonZeroU64 {
        self.0
    }
}

struct Inner {
    name: Option<CString>,
    id: ThreadId,
    parker: Parker,
}

#[derive(Clone)]
pub struct Thread {
    inner: Arc<Inner>,
}

impl Thread {
    fn new(name: Option<CString>) -> Thread {
        Thread { inner: Arc::new(Inner { name, id: ThreadId::new(), parker: Parker::new() }) }
    }
    pub fn id(&self) -> ThreadId {
        self.inner.id
    }
    pub fn name(&self) -> Option<&str> {
        match &self.inner.name {
            Some(n) => n.to_str().ok(),
            None => None,
        }
    }
    pub fn unpark(&self) {
        self.inner.parker.unpark();
    }
}

impl fmt::Debug for Thread {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Thread").field("id", &self.id()).field("name", &self.name()).finish_non_exhaustive()
    }
}

/// The current thread's Thread handle (set by thread_start, created lazily otherwise).
static CURRENT: LocalKey<UnsafeCell<Option<Thread>>> = LocalKey::new(current_cell_init);

fn current_cell_init() -> UnsafeCell<Option<Thread>> {
    UnsafeCell::new(None)
}

fn set_current(thread: Thread) {
    CURRENT.with(|c| unsafe { *c.get() = Some(thread) });
}

fn is_main_thread() -> bool {
    sys::os::is_main_thread()
}

pub fn current() -> Thread {
    let r = CURRENT.try_with(|c| unsafe {
        let slot = &mut *c.get();
        match slot {
            Some(t) => t.clone(),
            None => {
                let name = if is_main_thread() { CString::new("main").ok() } else { None };
                let t = Thread::new(name);
                *slot = Some(t.clone());
                t
            }
        }
    });
    match r {
        Ok(t) => t,
        Err(_) => panic!("use of std::thread::current() is not possible after the thread's local data has been destroyed"),
    }
}

// ---- free functions

pub fn sleep(dur: Duration) {
    sys::time::sleep(dur)
}

pub fn sleep_ms(ms: u32) {
    sleep(Duration::from_millis(ms as u64))
}

pub fn yield_now() {
    sys::thread_yield()
}

pub fn park() {
    current().inner.parker.park();
}

pub fn park_timeout(dur: Duration) {
    current().inner.parker.park_timeout(dur);
}

pub fn park_timeout_ms(ms: u32) {
    park_timeout(Duration::from_millis(ms as u64))
}

pub fn panicking() -> bool {
    sys::rt::panicking()
}

pub fn available_parallelism() -> io::Result<NonZeroUsize> {
    match NonZeroUsize::new(sys::available_parallelism()) {
        Some(n) => Ok(n),
        None => Err(io::const_error!(io::ErrorKind::NotFound, "The number of hardware threads is not known for the target platform")),
    }
}

// ---- spawning

#[derive(Debug)]
pub struct Builder {
    name: Option<String>,
    stack_size: Option<usize>,
}

impl Builder {
    pub fn new() -> Builder {
        Builder { name: None, stack_size: None }
    }
    pub fn name(mut self, name: String) -> Builder {
        self.name = Some(name);
        self
    }
    pub fn stack_size(mut self, size: usize) -> Builder {
        self.stack_size = Some(size);
        self
    }
    pub fn spawn<F, T>(self, f: F) -> io::Result<JoinHandle<T>>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        unsafe { Ok(JoinHandle(self.spawn_unchecked_(f, None)?)) }
    }

    pub fn spawn_scoped<'scope, 'env, F, T>(self, scope: &'scope Scope<'scope, 'env>, f: F) -> io::Result<ScopedJoinHandle<'scope, T>>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        unsafe { Ok(ScopedJoinHandle(self.spawn_unchecked_(f, Some(scope.data.clone()))?)) }
    }

    /// # Safety: `f` and `T` must outlive the thread (callers: 'static, or a scope that joins).
    unsafe fn spawn_unchecked_<'a, F, T>(self, f: F, scope_data: Option<Arc<ScopeData>>) -> io::Result<JoinInner<'a, T>>
    where
        F: FnOnce() -> T + Send + 'a,
        T: Send + 'a,
    {
        let stack_size = match self.stack_size {
            Some(s) => s,
            None => min_stack(),
        };
        let cname = match self.name {
            Some(name) => match CString::new(name) {
                Ok(c) => Some(c),
                Err(_) => panic!("thread name may not contain interior null bytes"),
            },
            None => None,
        };
        let my_thread = Thread::new(cname);
        let their_thread = my_thread.clone();
        let my_packet: Arc<Packet<'a, T>> = Arc::new(Packet { scope: scope_data, result: UnsafeCell::new(None), _marker: PhantomData });
        // The thread's reference to the packet lives in `finish`, never in `main`: a panic resets
        // the stack without dropping main's captures (R11), and a leaked packet reference would
        // keep a scope waiting for this thread forever.
        let their_packet = my_packet.clone();
        let result_ptr = SendPtr(Arc::as_ptr(&their_packet) as *const u8);
        if let Some(scope) = &my_packet.scope {
            scope.increment_num_running_threads();
        }
        let main = move || {
            crate::panic::prepare_payload();
            let r = f();
            let packet = result_ptr;
            unsafe { *(*(packet.0 as *const Packet<'a, T>)).result.get() = Some(Ok(r)) };
        };
        let finish = move |panicked: bool| {
            if panicked {
                let payload: Box<dyn Any + Send> = crate::panic::take_payload();
                unsafe { *their_packet.result.get() = Some(Err(payload)) };
            }
            drop(their_packet);
        };
        let main: Box<dyn FnOnce() + Send + 'a> = Box::new(main);
        let finish: Box<dyn FnOnce(bool) + Send + 'a> = Box::new(finish);
        // lifetime erased: the thread is joined before 'a ends (scope) or 'a is 'static
        let main: Box<dyn FnOnce() + Send + 'static> = core::mem::transmute(main);
        let finish: Box<dyn FnOnce(bool) + Send + 'static> = core::mem::transmute(finish);
        let start = Box::new(ThreadStart { thread: their_thread, main: Some(main), finish: Some(finish) });
        match spawn_native(stack_size, start) {
            Ok(native) => Ok(JoinInner { native, thread: my_thread, packet: my_packet }),
            Err(e) => {
                if let Some(scope) = &my_packet.scope {
                    scope.decrement_num_running_threads(false);
                }
                Err(e)
            }
        }
    }
}

impl Default for Builder {
    fn default() -> Builder {
        Builder::new()
    }
}

struct ThreadStart {
    thread: Thread,
    main: Option<Box<dyn FnOnce() + Send + 'static>>,
    /// Runs after main, with whether it panicked: stores the panic's payload, then drops the
    /// thread's packet reference (which tells a scope this thread is done).
    finish: Option<Box<dyn FnOnce(bool) + Send + 'static>>,
}

/// A raw pointer moved into a thread's main closure (the packet outlives it: `finish` holds it).
#[derive(Clone, Copy)]
struct SendPtr(*const u8);
unsafe impl Send for SendPtr {}

/// Relax's code needs more stack per call than rustc's: a frame holds what LLVM keeps in
/// registers or shares between slots (a type parser 1000 generics deep, rustc 1 MiB, Relax over
/// 2 MiB). A thread gets this multiple of the stack the program asked for (or of the 2 MiB
/// default), so recursion that fits natively fits here too; the stack is reserved address space,
/// pages are committed as they are touched.
const STACK_SCALE: usize = 4;

fn spawn_native(stack_size: usize, start: Box<ThreadStart>) -> io::Result<sys::NativeThread> {
    let arg = Box::into_raw(start) as *mut u8;
    match sys::spawn_thread(stack_size.saturating_mul(STACK_SCALE), thread_start, arg) {
        Ok(native) => Ok(native),
        Err(r) => {
            drop(unsafe { Box::from_raw(arg as *mut ThreadStart) });
            Err(io::Error::from_raw_os_error(r))
        }
    }
}

fn run_main(p: *mut u8) {
    let slot = unsafe { &mut *(p as *mut Option<Box<dyn FnOnce() + Send + 'static>>) };
    if let Some(main) = slot.take() {
        main();
    }
}

fn thread_start(arg: *mut u8) {
    let mut start = unsafe { Box::from_raw(arg as *mut ThreadStart) };
    if let Some(name) = &start.thread.inner.name {
        sys::os::set_thread_name(name.as_ptr());
        extern "C" { fn __relax_domain_name(name: *const u8); }
        unsafe { __relax_domain_name(name.as_ptr() as *const u8); }
    }
    set_current(start.thread.clone());
    let mut main = start.main.take();
    let panicked = sys::rt::catch_panic(run_main, &mut main as *mut Option<Box<dyn FnOnce() + Send + 'static>> as *mut u8);
    if let Some(finish) = start.finish.take() {
        finish(panicked);
    }
    drop(main);
    drop(start);
}

struct Packet<'scope, T> {
    scope: Option<Arc<ScopeData>>,
    result: UnsafeCell<Option<Result<T>>>,
    _marker: PhantomData<Option<&'scope ScopeData>>,
}

unsafe impl<'scope, T: Send> Sync for Packet<'scope, T> {}
unsafe impl<'scope, T: Send> Send for Packet<'scope, T> {}

impl<'scope, T> Drop for Packet<'scope, T> {
    fn drop(&mut self) {
        let unhandled_panic = match self.result.get_mut() {
            Some(Err(_)) => true,
            _ => false,
        };
        // drop the result before telling the scope we are done
        *self.result.get_mut() = None;
        if let Some(scope) = &self.scope {
            scope.decrement_num_running_threads(unhandled_panic);
        }
    }
}

struct JoinInner<'scope, T> {
    native: sys::NativeThread,
    thread: Thread,
    packet: Arc<Packet<'scope, T>>,
}

impl<'scope, T> JoinInner<'scope, T> {
    fn join(self) -> Result<T> {
        sys::join_thread(self.native);
        let packet = &self.packet;
        match unsafe { (*packet.result.get()).take() } {
            Some(r) => r,
            None => panic!("thread result missing after join"),
        }
    }
    fn is_finished(&self) -> bool {
        Arc::strong_count(&self.packet) == 1
    }
}

pub struct JoinHandle<T>(JoinInner<'static, T>);

unsafe impl<T> Send for JoinHandle<T> {}
unsafe impl<T> Sync for JoinHandle<T> {}

impl<T> JoinHandle<T> {
    pub fn thread(&self) -> &Thread {
        &self.0.thread
    }
    pub fn join(self) -> Result<T> {
        self.0.join()
    }
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
}

impl<T> fmt::Debug for JoinHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("JoinHandle").finish_non_exhaustive()
    }
}

pub fn spawn<F, T>(f: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    match Builder::new().spawn(f) {
        Ok(h) => h,
        Err(e) => panic!("failed to spawn thread: {:?}", e),
    }
}

// ---- scoped threads

struct ScopeData {
    num_running_threads: AtomicUsize,
    a_thread_panicked: AtomicBool,
    main_thread: Thread,
}

impl ScopeData {
    fn increment_num_running_threads(&self) {
        if self.num_running_threads.fetch_add(1, Relaxed) > usize::MAX / 2 {
            self.decrement_num_running_threads(false);
            panic!("too many running threads in thread scope");
        }
    }
    fn decrement_num_running_threads(&self, panic: bool) {
        if panic {
            self.a_thread_panicked.store(true, Relaxed);
        }
        if self.num_running_threads.fetch_sub(1, SeqCst) == 1 {
            self.main_thread.unpark();
        }
    }
}

pub struct Scope<'scope, 'env: 'scope> {
    data: Arc<ScopeData>,
    scope: PhantomData<&'scope mut &'scope ()>,
    env: PhantomData<&'env mut &'env ()>,
}

pub struct ScopedJoinHandle<'scope, T>(JoinInner<'scope, T>);

pub fn scope<'env, F, T>(f: F) -> T
where
    F: for<'scope> FnOnce(&'scope Scope<'scope, 'env>) -> T,
{
    let scope = Scope {
        data: Arc::new(ScopeData { num_running_threads: AtomicUsize::new(0), a_thread_panicked: AtomicBool::new(false), main_thread: current() }),
        scope: PhantomData,
        env: PhantomData,
    };
    let result = f(&scope);
    while scope.data.num_running_threads.load(SeqCst) != 0 {
        park();
    }
    if scope.data.a_thread_panicked.load(Relaxed) {
        panic!("a scoped thread panicked");
    }
    result
}

impl<'scope, 'env> Scope<'scope, 'env> {
    pub fn spawn<F, T>(&'scope self, f: F) -> ScopedJoinHandle<'scope, T>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        match Builder::new().spawn_scoped(self, f) {
            Ok(h) => h,
            Err(e) => panic!("failed to spawn thread: {:?}", e),
        }
    }
}

impl<'scope, T> ScopedJoinHandle<'scope, T> {
    pub fn thread(&self) -> &Thread {
        &self.0.thread
    }
    pub fn join(self) -> Result<T> {
        self.0.join()
    }
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
}

impl<'scope, 'env> fmt::Debug for Scope<'scope, 'env> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Scope")
            .field("num_running_threads", &self.data.num_running_threads.load(Relaxed))
            .field("a_thread_panicked", &self.data.a_thread_panicked.load(Relaxed))
            .field("main_thread", &self.data.main_thread)
            .finish_non_exhaustive()
    }
}

impl<'scope, T> fmt::Debug for ScopedJoinHandle<'scope, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ScopedJoinHandle").finish_non_exhaustive()
    }
}
