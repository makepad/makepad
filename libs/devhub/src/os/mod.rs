//! The node's per-OS backend: how a lease run is started, kept together and ended, process start
//! times, the home dir, file sizes, and (Windows) the machine's numbers. Unix and Windows give the
//! same functions; the callers carry no OS cases.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

/// What a run's processes have done so far: CPU time, µs (user and system, with the children
/// they reaped), and bytes read and written. Only a change means anything: the node's idle look
/// compares two of them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Activity {
    pub cpu_us: u64,
    pub io_bytes: u64,
}

impl std::ops::Add for Activity {
    type Output = Activity;
    fn add(self, o: Activity) -> Activity {
        Activity { cpu_us: self.cpu_us + o.cpu_us, io_bytes: self.io_bytes + o.io_bytes }
    }
}
