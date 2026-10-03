//! unix process extension traits.

use crate::ffi::OsStr;
use crate::process::{Command, ExitStatus};

pub trait CommandExt {
    fn arg0<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Command;
}

impl CommandExt for Command {
    fn arg0<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Command {
        self.set_arg0(arg.as_ref());
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
