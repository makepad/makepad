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

/// Byte limits of a node's exec, including each string's terminating NUL.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExecLimits {
    pub total: usize,
    pub per_string: Option<usize>,
}

impl ExecLimits {
    pub fn to_json(&self) -> String {
        format!("{{\"total\":{},\"per_string\":{}}}", self.total, self.per_string.map_or("null".into(), |n| n.to_string()))
    }

    pub fn parse(v: &makepad_strict_json::Value) -> Option<Self> {
        let number = |v: &makepad_strict_json::Value| v.as_u64().and_then(|n| usize::try_from(n).ok()).filter(|n| *n > 0);
        Some(Self { total: number(v.get("total")?)?, per_string: match v.get("per_string") { Some(v) if !v.is_null() => Some(number(v)?), _ => None } })
    }

    pub fn word_over(&self, label: &str, bytes: usize, pointer: usize) -> Option<String> {
        if let Some(one) = self.per_string.filter(|one| bytes > *one) {
            Some(format!("{label} is {}, and Linux execs at most {} in one argument or env value (including its NUL)", exec_size(bytes), exec_size(one)))
        } else if bytes.saturating_add(pointer) > self.total {
            Some(format!("{label} is {}, and this box execs at most {} of command and env together", exec_size(bytes), exec_size(self.total)))
        } else { None }
    }

    pub fn total_over(&self, bytes: usize) -> Option<String> {
        (bytes > self.total).then(|| format!("the command and env are {} together, and this box execs at most {} of them", exec_size(bytes), exec_size(self.total)))
    }
}

fn exec_size(n: usize) -> String {
    if n >= 1024 * 1024 { format!("{:.1} MiB", n as f64 / (1024.0 * 1024.0)) } else { format!("{:.0} KiB", n as f64 / 1024.0) }
}

pub fn exec_refusal(over: &str) -> String {
    format!("{over}: pass it in a file the run reads (a script after sh -c or python -c goes through one by itself)")
}
