//! unix process extension traits.

use crate::ffi::OsStr;
use crate::process::{Command, ExitStatus};

/// Returns the OS-assigned process identifier of this process's parent.
#[must_use]
pub fn parent_id() -> u32 {
    unsafe { crate::sys::getppid() as u32 }
}

pub trait CommandExt {
    fn arg0<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Command;
    fn process_group(&mut self, pgroup: i32) -> &mut Command;
    fn exec(&mut self) -> crate::io::Error;
    /// Runs `f` in the child after fork, before exec (real std's contract: async-signal-safe
    /// work only; errors abort the spawn and are returned by spawn()).
    unsafe fn pre_exec<F>(&mut self, f: F) -> &mut Command
    where
        F: FnMut() -> crate::io::Result<()> + Send + Sync + 'static;
    #[deprecated(note = "should be unsafe, use `pre_exec` instead")]
    #[rustc_deprecated_safe_2024(audit_that = "the closure is async-signal-safe")]
    unsafe fn before_exec<F>(&mut self, f: F) -> &mut Command
    where
        F: FnMut() -> crate::io::Result<()> + Send + Sync + 'static,
    {
        unsafe { self.pre_exec(f) }
    }
}

impl CommandExt for Command {
    fn arg0<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Command {
        self.set_arg0(arg.as_ref());
        self
    }
    fn process_group(&mut self, pgroup: i32) -> &mut Command {
        self.set_pgroup(pgroup);
        self
    }
    fn exec(&mut self) -> crate::io::Error {
        self.exec_replace()
    }
    unsafe fn pre_exec<F>(&mut self, f: F) -> &mut Command
    where
        F: FnMut() -> crate::io::Result<()> + Send + Sync + 'static,
    {
        self.add_pre_exec(alloc::boxed::Box::new(f));
        self
    }
}

pub trait ExitStatusExt {
    fn from_raw(raw: i32) -> Self;
    fn signal(&self) -> Option<i32>;
    fn core_dumped(&self) -> bool;
    fn into_raw(self) -> i32;
}

impl ExitStatusExt for ExitStatus {
    fn from_raw(raw: i32) -> ExitStatus {
        ExitStatus::from_raw_status(raw)
    }
    fn signal(&self) -> Option<i32> {
        self.signal_number()
    }
    fn core_dumped(&self) -> bool {
        self.signal_number().is_some() && self.raw() & 0x80 != 0
    }
    fn into_raw(self) -> i32 {
        self.raw()
    }
}
