//! std::panic: hooks, catch_unwind over the runtime's catch_panic, Location re-export.
//! A panic unwinds the frames between it and the catch, running each frame's cleanup pads
//! (Drop of its live locals) as rustc's unwinder does; catch_unwind returns Err after that.

use alloc::boxed::Box;
use alloc::string::String;
use core::any::Any;
use core::fmt;

pub use core::panic::{AssertUnwindSafe, Location, RefUnwindSafe, UnwindSafe};

use crate::sync::RwLock;
use crate::sys;

pub type PanicInfo<'a> = PanicHookInfo<'a>;

pub struct PanicHookInfo<'a> {
    payload: &'a (dyn Any + Send),
    message: &'a str,
    location: &'a Location<'a>,
}

impl<'a> PanicHookInfo<'a> {
    pub fn payload(&self) -> &(dyn Any + Send) {
        self.payload
    }
    pub fn payload_as_str(&self) -> Option<&str> {
        Some(self.message)
    }
    pub fn location(&self) -> Option<&Location<'_>> {
        Some(self.location)
    }
    pub fn can_unwind(&self) -> bool {
        false
    }
}

impl<'a> fmt::Display for PanicHookInfo<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "panicked at {}:\n{}", self.location, self.message)
    }
}

impl<'a> fmt::Debug for PanicHookInfo<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("PanicHookInfo")
            .field("payload", &self.payload_as_str())
            .field("location", &self.location)
            .field("can_unwind", &false)
            .field("force_no_backtrace", &false)
            .finish()
    }
}

type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

static HOOK: RwLock<Option<Hook>> = RwLock::new(None);

fn trampoline(message: &str, location: &Location<'_>) {
    let payload = String::from(message);
    let info = PanicHookInfo { payload: &payload, message, location };
    let guard = match HOOK.read() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    match &*guard {
        Some(hook) => hook(&info),
        None => default_hook(&info),
    }
}

/// Real std's default hook output.
fn default_hook(info: &PanicHookInfo<'_>) {
    let thread = crate::thread::current();
    let name = match thread.name() {
        Some(n) => n,
        None => "<unnamed>",
    };
    let mut err = crate::io::stderr().lock();
    let _ = crate::io::Write::write_fmt(
        &mut err,
        format_args!("\nthread '{}' ({}) panicked at {}:\n{}\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n", name, sys::os::thread_os_id(), info.location, info.message),
    );
}

pub fn set_hook(hook: Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>) {
    if crate::thread::panicking() {
        panic!("cannot modify the panic hook from a panicking thread");
    }
    let mut guard = match HOOK.write() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    *guard = Some(hook);
    drop(guard);
    sys::rt::set_panic_trampoline(trampoline);
}

pub fn take_hook() -> Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static> {
    if crate::thread::panicking() {
        panic!("cannot modify the panic hook from a panicking thread");
    }
    let mut guard = match HOOK.write() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    match guard.take() {
        Some(h) => h,
        None => Box::new(default_hook),
    }
}

thread_local! {
    /// The payload of this thread's last panic, until catch_unwind / the thread's join packet
    /// takes it. `Some` before core_hook runs means panic_any / resume_unwind set it.
    static PAYLOAD: core::cell::Cell<Option<Box<dyn Any + Send>>> = const { core::cell::Cell::new(None) };
}

thread_local! {
    /// This thread's panic nesting (real std's LOCAL_PANIC_COUNT role), for core::panicking.
    static DEPTH: core::cell::Cell<u32> = const { core::cell::Cell::new(0) };
}

/// core::panicking's per-thread depth counter: +1 on entry (returns the new depth), -1 on exit.
pub(crate) fn panic_depth_step(delta: i32) -> u32 {
    match DEPTH.try_with(|d| {
        let n = (d.get() as i32 + delta) as u32;
        d.set(n);
        n
    }) {
        Ok(n) => n,
        Err(_) => 1,
    }
}

/// Called (through sys::rt's core hook) for every panic: real std's payload is the `&'static
/// str` of a literal-only message, else the formatted String; a payload set by panic_any /
/// resume_unwind for this panic is kept.
pub(crate) fn record_payload(message: fmt::Arguments<'_>) {
    let _ = PAYLOAD.try_with(|c| {
        let old = c.take();
        let p: Box<dyn Any + Send> = match old {
            Some(p) => p,
            None => match message.as_str() {
                Some(s) => Box::new(s),
                None => Box::new(alloc::fmt::format(message)),
            },
        };
        c.set(Some(p));
    });
}

fn clear_payload() {
    let _ = PAYLOAD.try_with(|c| drop(c.take()));
}

/// The payload of the panic that just ended this thread's catch point (taken).
pub(crate) fn take_payload() -> Box<dyn Any + Send> {
    match PAYLOAD.try_with(|c| c.take()) {
        Ok(Some(p)) => p,
        _ => Box::new("explicit panic"),
    }
}

/// Clears any stale payload and makes sure panics record theirs (thread start does the same).
pub(crate) fn prepare_payload() {
    sys::rt::install_core_hook();
    clear_payload();
}

/// Runs `f`; Err(payload) if it panicked, with real std's payload (see record_payload).
pub fn catch_unwind<F: FnOnce() -> R + UnwindSafe, R>(f: F) -> Result<R, Box<dyn Any + Send + 'static>> {
    let mut slot: (Option<F>, Option<R>) = (Some(f), None);
    prepare_payload();
    let panicked = sys::rt::catch_panic(run_slot::<F, R>, &mut slot as *mut (Option<F>, Option<R>) as *mut u8);
    if panicked {
        return Err(take_payload());
    }
    match slot.1 {
        Some(r) => Ok(r),
        None => Err(take_payload()),
    }
}

fn run_slot<F: FnOnce() -> R, R>(p: *mut u8) {
    let slot = unsafe { &mut *(p as *mut (Option<F>, Option<R>)) };
    if let Some(f) = slot.0.take() {
        slot.1 = Some(f());
    }
}

thread_local! {
    /// Set by resume_unwind: the panic it starts does not run the panic hook (std: "resumes a
    /// panic without invoking the panic hook").
    static RESUMING: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

/// Whether the panic being reported was started by resume_unwind (and clears the mark).
pub(crate) fn take_resuming() -> bool {
    RESUMING.try_with(|c| c.replace(false)).unwrap_or(false)
}

/// Panics with `payload` as the payload catch_unwind / join return. No hook runs for it and
/// inspect does not park it again; the runtime's report is the only trace.
pub fn resume_unwind(payload: Box<dyn Any + Send>) -> ! {
    let _ = RESUMING.try_with(|c| c.set(true));
    sys::rt::rethrow();
    unwind_with(payload)
}

fn unwind_with(payload: Box<dyn Any + Send>) -> ! {
    let msg: String = if let Some(s) = payload.downcast_ref::<&'static str>() {
        String::from(*s)
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        String::from("Box<dyn Any>")
    };
    sys::rt::install_core_hook();
    let _ = PAYLOAD.try_with(|c| c.set(Some(payload)));
    panic!("{}", msg)
}

pub fn panic_any<M: 'static + Any + Send>(msg: M) -> ! {
    unwind_with(Box::new(msg))
}
