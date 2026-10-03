//! std::panic: hooks, catch_unwind over the runtime's catch_panic, Location re-export.
//! Under Rapid no panic unwinds through frames (R11): catch_unwind returns Err after the
//! runtime reset the stack to its frame (live mode) and Drop of the skipped frames does not run.

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

/// Runs `f`; Err if it panicked. The payload is the panic message as a String.
pub fn catch_unwind<F: FnOnce() -> R + UnwindSafe, R>(f: F) -> Result<R, Box<dyn Any + Send + 'static>> {
    let mut slot: (Option<F>, Option<R>) = (Some(f), None);
    let panicked = sys::rt::catch_panic(run_slot::<F, R>, &mut slot as *mut (Option<F>, Option<R>) as *mut u8);
    if panicked {
        return Err(Box::new(String::from("panic")));
    }
    match slot.1 {
        Some(r) => Ok(r),
        None => Err(Box::new(String::from("panic"))),
    }
}

fn run_slot<F: FnOnce() -> R, R>(p: *mut u8) {
    let slot = unsafe { &mut *(p as *mut (Option<F>, Option<R>)) };
    if let Some(f) = slot.0.take() {
        slot.1 = Some(f());
    }
}

pub fn resume_unwind(payload: Box<dyn Any + Send>) -> ! {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        panic!("{}", s);
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        panic!("{}", s);
    }
    panic!("Box<dyn Any>")
}

pub fn panic_any<M: 'static + Any + Send>(msg: M) -> ! {
    resume_unwind(Box::new(msg))
}
