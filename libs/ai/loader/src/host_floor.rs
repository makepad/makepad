//! The host-memory floor every weight read keeps.
//!
//! The hub admits a load by its estimate (`makepad_ai_hub::ram`); this is what holds when the
//! estimate is wrong. Before a read that allocates a tensor or a file, a loader asks [`check`]:
//! when the box's available memory, less the read, would fall under the reserve, the read is
//! refused with a `host-memory:` error. The loader's error path drops what it had loaded, and
//! the process lives: no load can run the box out and end it, whatever its estimate said.
//!
//! The bound is the box's real available memory (Linux `MemAvailable`, Windows available
//! physical memory), never its total, so other processes' memory counts. macOS compresses and
//! swaps rather than ending a process, so there a read is never refused.

use std::fmt::Display;

/// Host memory a load keeps free, in MiB, unless `MAKEPAD_AI_RAM_RESERVE_MB` says otherwise.
/// The hub's admission keeps the same reserve (`makepad_ai_hub::ram::host_reserve_mb`).
pub const DEFAULT_RESERVE_MB: u64 = 4096;

/// Reads smaller than this are not checked: rows, biases and headers cannot cross a reserve of
/// gigabytes, and a check reads the system's memory figures.
const CHECKED_FROM_BYTES: u64 = 1 << 20;

const BYTES_PER_MB: u64 = 1 << 20;

/// The reserve a load keeps free, in MiB.
pub fn reserve_mb() -> u64 {
    std::env::var("MAKEPAD_AI_RAM_RESERVE_MB")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(DEFAULT_RESERVE_MB)
}

/// The box's available host memory in MiB, where running out ends a process (Linux, Windows);
/// `None` elsewhere or when it can't be read.
pub fn available_mb() -> Option<u64> {
    platform::available_bytes().map(|bytes| bytes / BYTES_PER_MB)
}

/// Whether a read of `bytes` (what it reads, for the message) keeps the reserve free; else the
/// `host-memory:` refusal, with the numbers.
pub fn check(bytes: u64, what: impl Display) -> Result<(), String> {
    if bytes < CHECKED_FROM_BYTES {
        return Ok(());
    }
    let Some(available) = available_mb() else { return Ok(()) };
    let reserve = reserve_mb();
    let need = bytes.div_ceil(BYTES_PER_MB);
    if available >= reserve.saturating_add(need) {
        return Ok(());
    }
    Err(format!(
        "host-memory: {what} takes {need} MB, {available} MB is available and {reserve} MB stays free: the load stops here and drops what it had read"
    ))
}

#[cfg(target_os = "linux")]
mod platform {
    /// `MemAvailable` of /proc/meminfo.
    pub(super) fn available_bytes() -> Option<u64> {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = text.lines().find(|line| line.starts_with("MemAvailable:"))?;
        let kib: u64 = line["MemAvailable:".len()..].trim().strip_suffix("kB")?.trim().parse().ok()?;
        kib.checked_mul(1024)
    }
}

#[cfg(windows)]
mod platform {
    use makepad_windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    pub(super) fn available_bytes() -> Option<u64> {
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..MEMORYSTATUSEX::default()
        };
        // SAFETY: one MEMORYSTATUSEX we own with dwLength set as required.
        unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
        Some(status.ullAvailPhys)
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    pub(super) fn available_bytes() -> Option<u64> {
        None
    }
}
