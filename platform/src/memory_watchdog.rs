//! Process memory watchdog — catches runaway leaks before they take the
//! machine into swap death (map apps have a history: 157 GB observed
//! 2026-07-30 on the tile/mesh path).
//!
//! `start_memory_watchdog(None)` samples the process physical footprint
//! every 2s on a background thread. Crossing the soft limit logs an
//! escalating warning with the growth rate; crossing the hard limit logs
//! a fatal report and aborts the process — a deliberate crash beats an
//! unusable machine and leaves a clean signal in the log.
//!
//! Limits default to 24 GB soft / 48 GB hard, overridable with
//! `MAKEPAD_MEM_LIMIT_GB` (hard; soft = half) or explicit arguments.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static WATCHDOG_STARTED: AtomicBool = AtomicBool::new(false);
static ORPHAN_WATCHDOG_STARTED: AtomicBool = AtomicBool::new(false);

/// How often the orphan guard looks at its parent. A hosted child must not
/// outlive its host by more than about a second.
const ORPHAN_POLL_MS: u64 = 250;

/// Unix seconds of the last message received from the studio host in
/// `--stdin-loop` mode (0 = never / not in that mode).
static STDIN_LAST_HOST_MSG_UNIX: AtomicU64 = AtomicU64::new(0);
/// A stdin-loop app that hears nothing from studio for this long is an
/// abandoned build (ClearBuild leaves the websocket half-open — the
/// historic zombie-instance leak) and exits itself.
const STDIN_HOST_SILENCE_LIMIT_S: u64 = 300;

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Called by the stdin event loop on every received host message.
pub fn note_stdin_host_message() {
    STDIN_LAST_HOST_MSG_UNIX.store(now_unix(), Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug)]
pub struct MemoryLimits {
    pub soft_bytes: u64,
    pub hard_bytes: u64,
}

impl Default for MemoryLimits {
    fn default() -> Self {
        let hard_gb = std::env::var("MAKEPAD_MEM_LIMIT_GB")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(48.0)
            .max(1.0);
        Self {
            soft_bytes: (hard_gb * 0.5 * 1e9) as u64,
            hard_bytes: (hard_gb * 1e9) as u64,
        }
    }
}

/// Start the watchdog thread (idempotent). `limits: None` = defaults/env.
pub fn start_memory_watchdog(limits: Option<MemoryLimits>) {
    if WATCHDOG_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let limits = limits.unwrap_or_default();
    if process_footprint_bytes().is_none() {
        crate::log!("memory watchdog: footprint sampling unsupported on this platform");
        return;
    }
    std::thread::Builder::new()
        .name("memory-watchdog".into())
        .spawn(move || watchdog_main(limits))
        .ok();
}

fn gb(bytes: u64) -> f64 {
    bytes as f64 / 1e9
}

/// A `--stdin-loop` app renders into studio through its cargo parent; when
/// that parent dies without taking us along (studio's process-group kill
/// has a flaky fallback), we are a zombie holding gigabytes and rendering
/// into a void — the historic "157 GB map process". Detect the orphaning
/// (reparented to init/launchd) and exit.
fn orphaned_stdin_app() -> bool {
    #[cfg(unix)]
    {
        if !crate::app_main::should_run_stdin_loop_from_env() {
            return false;
        }
        extern "C" {
            fn getppid() -> i32;
        }
        unsafe { getppid() == 1 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Start the orphan guard every `--stdin-loop` child needs (idempotent,
/// and a no-op outside that mode).
///
/// The host owns the child's whole reason to run: it draws into the host's
/// swapchain and its only input is the host's message stream. When the host
/// dies the child must follow, and its own event loop cannot be trusted to
/// notice — a busy handler (an mpterm tile parsing a `yes` flood) can sit
/// between two socket reads for minutes, which is exactly how orphans that
/// burn a core were surviving their mpwm. This is a separate, tiny thread so
/// the check keeps running no matter what the event loop is doing, and it is
/// independent of the (opt-in) memory watchdog, which almost no app starts.
pub fn start_stdin_orphan_watchdog() {
    if !crate::app_main::should_run_stdin_loop_from_env() {
        return;
    }
    if ORPHAN_WATCHDOG_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("stdin-orphan-watchdog".into())
        .spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_millis(ORPHAN_POLL_MS));
            if orphaned_stdin_app() {
                crate::error!(
                    "stdin-loop host is gone (reparented to init) — exiting orphaned app instance"
                );
                std::thread::sleep(std::time::Duration::from_millis(100));
                std::process::exit(0);
            }
        })
        .ok();
}

fn watchdog_main(limits: MemoryLimits) {
    let mut last_warn = std::time::Instant::now() - std::time::Duration::from_secs(3600);
    let mut prev_sample: Option<(std::time::Instant, u64)> = None;
    let mut peak: u64 = 0;
    loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        if orphaned_stdin_app() {
            crate::error!(
                "memory watchdog: stdin-loop host is gone (orphaned) — exiting to avoid a zombie app instance"
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
            std::process::exit(0);
        }
        // Host-silence guard: parent may outlive us (studio spawns apps
        // directly), so also exit when the studio websocket goes quiet.
        let last_msg = STDIN_LAST_HOST_MSG_UNIX.load(Ordering::Relaxed);
        if last_msg != 0 && now_unix().saturating_sub(last_msg) > STDIN_HOST_SILENCE_LIMIT_S {
            crate::error!(
                "memory watchdog: no studio host traffic for {}s — exiting abandoned stdin-loop instance",
                STDIN_HOST_SILENCE_LIMIT_S
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
            std::process::exit(0);
        }
        let Some(footprint) = process_footprint_bytes() else {
            return;
        };
        peak = peak.max(footprint);
        let rate_gb_min = prev_sample
            .map(|(t, b)| {
                let dt = t.elapsed().as_secs_f64().max(0.001);
                (footprint as f64 - b as f64) / 1e9 / (dt / 60.0)
            })
            .unwrap_or(0.0);
        prev_sample = Some((std::time::Instant::now(), footprint));

        if footprint >= limits.hard_bytes {
            crate::error!(
                "memory watchdog: HARD LIMIT — footprint {:.1} GB >= {:.1} GB (peak {:.1} GB, growing {:+.2} GB/min). Aborting to protect the machine.",
                gb(footprint), gb(limits.hard_bytes), gb(peak), rate_gb_min
            );
            // Give the log a moment to flush, then die loudly.
            std::thread::sleep(std::time::Duration::from_millis(300));
            std::process::abort();
        }
        if footprint >= limits.soft_bytes && last_warn.elapsed().as_secs() >= 30 {
            last_warn = std::time::Instant::now();
            crate::log!(
                "memory watchdog: footprint {:.1} GB over soft limit {:.1} GB (hard {:.1} GB, growing {:+.2} GB/min)",
                gb(footprint), gb(limits.soft_bytes), gb(limits.hard_bytes), rate_gb_min
            );
        }
    }
}

/// Physical footprint of this process in bytes (what Activity Monitor's
/// "Memory" column shows on macOS — includes compressed pages that RSS
/// hides).
pub fn process_footprint_bytes() -> Option<u64> {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
    {
        // task_info(TASK_VM_INFO).phys_footprint — plain mach syscall, no
        // external deps.
        #[repr(C)]
        #[derive(Default)]
        struct TaskVmInfo {
            virtual_size: u64,
            region_count: i32,
            page_size: i32,
            resident_size: u64,
            resident_size_peak: u64,
            device: u64,
            device_peak: u64,
            internal: u64,
            internal_peak: u64,
            external: u64,
            external_peak: u64,
            reusable: u64,
            reusable_peak: u64,
            purgeable_volatile_pmap: u64,
            purgeable_volatile_resident: u64,
            purgeable_volatile_virtual: u64,
            compressed: u64,
            compressed_peak: u64,
            compressed_lifetime: u64,
            phys_footprint: u64,
            // struct continues in newer revisions; count below stops here
        }
        const TASK_VM_INFO: u32 = 22;
        // natural_t count up to and including phys_footprint
        let count =
            (std::mem::size_of::<TaskVmInfo>() / std::mem::size_of::<u32>()) as u32;
        extern "C" {
            fn mach_task_self() -> u32;
            fn task_info(
                task: u32,
                flavor: u32,
                task_info_out: *mut TaskVmInfo,
                task_info_out_count: *mut u32,
            ) -> i32;
        }
        let mut info = TaskVmInfo::default();
        let mut out_count = count;
        let kr = unsafe { task_info(mach_task_self(), TASK_VM_INFO, &mut info, &mut out_count) };
        if kr == 0 && info.phys_footprint > 0 {
            return Some(info.phys_footprint);
        }
        if kr == 0 {
            return Some(info.resident_size);
        }
        None
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kb: u64 = rest.trim().trim_end_matches("kB").trim().parse().ok()?;
                return Some(kb * 1024);
            }
        }
        None
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos",
        target_os = "linux",
        target_os = "android"
    )))]
    {
        None
    }
}
