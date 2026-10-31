//! std::backtrace: the API surface with an unsupported capture (Relax reports its own
//! backtraces in crash reports; library/std/src/backtrace.rs `BacktraceStatus::Unsupported`
//! is what real std returns on targets without unwinding info). Display prints what real std
//! prints for that status.

use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BacktraceStatus {
    Unsupported,
    Disabled,
    Captured,
}

pub struct Backtrace {
    status: BacktraceStatus,
}

impl Backtrace {
    pub fn capture() -> Backtrace {
        Backtrace { status: BacktraceStatus::Disabled }
    }
    pub fn force_capture() -> Backtrace {
        Backtrace { status: BacktraceStatus::Unsupported }
    }
    pub const fn disabled() -> Backtrace {
        Backtrace { status: BacktraceStatus::Disabled }
    }
    pub fn status(&self) -> BacktraceStatus {
        self.status
    }
}

impl fmt::Display for Backtrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            BacktraceStatus::Unsupported => f.write_str("unsupported backtrace"),
            BacktraceStatus::Disabled => f.write_str("disabled backtrace"),
            BacktraceStatus::Captured => Ok(()),
        }
    }
}

impl fmt::Debug for Backtrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            BacktraceStatus::Unsupported => f.write_str("<unsupported>"),
            BacktraceStatus::Disabled => f.write_str("<disabled>"),
            BacktraceStatus::Captured => f.write_str("Backtrace [ ]"),
        }
    }
}
