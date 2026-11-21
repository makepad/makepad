//! Host memory: the machine's free memory, a model's host-memory estimate, and its
//! measured peak.
//!
//! A model that does not fit must never take the hub down with every job on it (a song
//! model held 32 GB of host memory on a 30 GB node, and the kernel's out-of-memory killer
//! ended the hub twice). So a load is admitted by measure, like its GPU admission
//! (`crate::residency`):
//!
//! - a model's estimate ([`host_estimate`]) is the newest peak measured for it on any box of
//!   this OS ([`HostPeaks`]: this hub's own, or one a routing snapshot carried that is newer;
//!   a Mac's peak is a different quantity, unified memory and another allocator), else
//!   its weight files as its registry entry says its loader takes them (`host_load`: one that
//!   maps them, or streams them to the device a tensor at a time, takes a few percent of them;
//!   one that reads them, all; one that converts them, more), else the files themselves;
//! - before a load the worker compares it plus a reserve ([`host_reserve_mb`]) with the free
//!   memory now ([`query_ram`]); short, it unloads the idle models, then waits while another job
//!   runs there, else refuses the job saying why and what fits there (a routed job moves on);
//!   routing prefers a box where it fits now, then one where it fits once memory frees;
//! - whatever the estimate, a load never runs the box out: each weight read keeps the reserve
//!   free of the box's available memory, else it stops the load with a `host-memory:` refusal
//!   (`makepad_ai_loader::host_floor`), and the hub lives;
//! - after each load from cold, its peak ([`PeakWindow`]: the process's anonymous memory over
//!   the load and its first job) replaces the estimate, here and, through `/models`, for every
//!   router choosing among boxes of this OS (`/health` `os`).
//!
//! This module only measures and classifies. The caller owns model loading,
//! unloading, and taking a fresh measurement after each eviction.

/// Default RAM left for the OS and the rest of the user's applications.
pub const DEFAULT_RESERVE_MB: u64 = 4096;

/// The OS this process measures its peaks on, as Rust names it (`linux`, `macos`, `windows`).
pub const OS: &str = std::env::consts::OS;

const BYTES_PER_MB: u64 = 1024 * 1024;
const FIXED_OVERHEAD_MB: u64 = 512;
// A model-independent proxy for the KV/recurrent cache plus graph workspace.
// The exact value is architecture-dependent; 64 KiB/token is the explicit
// planning scale chosen for the shared Qwen-class model this gate protects.
const CONTEXT_BYTES_PER_TOKEN: u64 = 64 * 1024;

/// One fresh view of physical system memory, in binary megabytes (MiB).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RamFacts {
    pub free_mb: u64,
    pub total_mb: u64,
}

/// Measures physical RAM without spawning a helper process.
///
/// Returns `None` when the platform call, file read, or parse fails. In
/// particular, it does not substitute a guessed page size or memory total.
pub fn query_ram() -> Option<RamFacts> {
    platform::query_ram()
}

/// Estimates the hot resident footprint of one CPU GGUF model, in MiB.
///
/// Formula: ceil(GGUF file bytes / MiB) + 512 MiB fixed runtime overhead +
/// ceil(max_context * 64 KiB / MiB) for the context cache/graph arena. The
/// file is mmap-backed, but its weight pages count once they have become hot.
/// Without parsing architecture-specific tensor metadata, the context term is
/// deliberately one visible Qwen-class planning coefficient rather than a
/// claim of byte precision. Arithmetic saturates rather than wrapping.
pub fn estimate_model_ram_mb(gguf_file_len: u64, max_context: u32) -> u64 {
    let weights_mb = bytes_to_mb_ceil(gguf_file_len);
    let context_bytes = u64::from(max_context).saturating_mul(CONTEXT_BYTES_PER_TOKEN);
    weights_mb
        .saturating_add(bytes_to_mb_ceil(context_bytes))
        .saturating_add(FIXED_OVERHEAD_MB)
}

/// Result of comparing a fresh RAM measurement with one load estimate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admit {
    Ok,
    /// Evict at least this much resident RAM, then measure again.
    NeedsEviction { short_mb: u64 },
    /// The load cannot fit even if every other resident is retired.
    Never { reason: String },
}

/// Pure RAM admission math.
pub fn admit(facts: RamFacts, estimate_mb: u64, reserve_mb: u64) -> Admit {
    let Some(required_mb) = estimate_mb.checked_add(reserve_mb) else {
        return Admit::Never {
            reason: "model estimate plus RAM reserve overflows u64".to_string(),
        };
    };
    if required_mb > facts.total_mb {
        return Admit::Never {
            reason: format!(
                "need {required_mb} MB (estimate {estimate_mb} MB + reserve {reserve_mb} MB), but the machine has only {} MB total",
                facts.total_mb
            ),
        };
    }
    if facts.free_mb >= required_mb {
        Admit::Ok
    } else {
        Admit::NeedsEviction {
            short_mb: required_mb - facts.free_mb,
        }
    }
}

/// Returns resident rows in deterministic least-recently-used order.
///
/// A `last_used_ms` of zero naturally sorts as never/oldest. Model id breaks
/// timestamp ties, and `resident_mb` stays attached so the caller can retire
/// rows until the shortfall is covered, measuring again after each unload.
pub fn lru_order<T: Clone + Ord>(rows: &[(T, u64, u64)]) -> Vec<(T, u64, u64)> {
    let mut ordered = rows.to_vec();
    ordered.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    ordered
}

fn bytes_to_mb_ceil(bytes: u64) -> u64 {
    bytes / BYTES_PER_MB + u64::from(bytes % BYTES_PER_MB != 0)
}

#[cfg(any(target_os = "macos", target_os = "linux", windows, test))]
fn facts_from_bytes(total: u64, free: u64) -> Option<RamFacts> {
    let total_mb = total / BYTES_PER_MB;
    let free_mb = free / BYTES_PER_MB;
    (total_mb > 0 && free_mb <= total_mb).then_some(RamFacts { free_mb, total_mb })
}

#[cfg(any(target_os = "linux", test))]
fn parse_linux_meminfo(text: &str) -> Option<RamFacts> {
    fn kib_value(text: &str, wanted: &str) -> Option<u64> {
        text.lines().find_map(|line| {
            let (name, tail) = line.split_once(':')?;
            if name.trim() != wanted {
                return None;
            }
            let mut fields = tail.split_whitespace();
            let value = fields.next()?.parse::<u64>().ok()?;
            (fields.next() == Some("kB")).then_some(value)
        })
    }

    let total = kib_value(text, "MemTotal")?.checked_mul(1024)?;
    let available = kib_value(text, "MemAvailable")?.checked_mul(1024)?;
    facts_from_bytes(total, available)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{facts_from_bytes, RamFacts};
    use std::ffi::{c_int, c_uint, c_void};

    type MachPort = u32;
    type KernReturn = c_int;

    const CTL_HW: c_int = 6;
    const HW_PAGESIZE: c_int = 7;
    const HW_MEMSIZE: c_int = 24;
    const HOST_VM_INFO64: c_int = 4;
    const HOST_VM_INFO64_COUNT: u32 = (std::mem::size_of::<VmStatistics64>() / 4) as u32;

    #[link(name = "System", kind = "dylib")]
    extern "C" {
        fn sysctl(
            name: *const c_int,
            namelen: c_uint,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *const c_void,
            newlen: usize,
        ) -> c_int;
        fn mach_host_self() -> MachPort;
        fn host_statistics64(
            host: MachPort,
            flavor: c_int,
            host_info_out: *mut c_void,
            host_info_out_count: *mut u32,
        ) -> KernReturn;
    }

    /// `struct vm_statistics64` (mach/vm_statistics.h). 152 bytes.
    #[repr(C)]
    #[derive(Default)]
    struct VmStatistics64 {
        free_count: u32,
        active_count: u32,
        inactive_count: u32,
        wire_count: u32,
        zero_fill_count: u64,
        reactivations: u64,
        pageins: u64,
        pageouts: u64,
        faults: u64,
        cow_faults: u64,
        lookups: u64,
        hits: u64,
        purges: u64,
        purgeable_count: u32,
        speculative_count: u32,
        decompressions: u64,
        compressions: u64,
        swapins: u64,
        swapouts: u64,
        compressor_page_count: u32,
        throttled_count: u32,
        external_page_count: u32,
        internal_page_count: u32,
        total_uncompressed_pages_in_compressor: u64,
    }

    const _: () = assert!(std::mem::size_of::<VmStatistics64>() == 152);
    const _: () = assert!(HOST_VM_INFO64_COUNT == 38);

    fn sysctl_value<T: Default>(mib: &[c_int]) -> Option<T> {
        let mut value = T::default();
        let mut size = std::mem::size_of::<T>();
        // SAFETY: oldp points at exactly `size` bytes of a T we own.
        let rc = unsafe {
            sysctl(
                mib.as_ptr(),
                mib.len() as c_uint,
                (&mut value as *mut T).cast(),
                &mut size,
                std::ptr::null(),
                0,
            )
        };
        (rc == 0 && size == std::mem::size_of::<T>()).then_some(value)
    }

    pub(super) fn query_ram() -> Option<RamFacts> {
        let total = sysctl_value::<u64>(&[CTL_HW, HW_MEMSIZE])?;
        let page_size = sysctl_value::<u64>(&[CTL_HW, HW_PAGESIZE])
            .filter(|size| *size > 0)
            .or_else(|| {
                sysctl_value::<u32>(&[CTL_HW, HW_PAGESIZE])
                    .filter(|size| *size > 0)
                    .map(u64::from)
            })?;

        let mut stats = VmStatistics64::default();
        let mut count = HOST_VM_INFO64_COUNT;
        // SAFETY: host_info_out points at a VmStatistics64 sized exactly count
        // 32-bit words, which is what HOST_VM_INFO64 writes.
        let rc = unsafe {
            host_statistics64(
                mach_host_self(),
                HOST_VM_INFO64,
                (&mut stats as *mut VmStatistics64).cast(),
                &mut count,
            )
        };
        if rc != 0 {
            return None;
        }
        let available_pages = u64::from(stats.free_count)
            .checked_add(u64::from(stats.inactive_count))?;
        let available = available_pages.checked_mul(page_size)?;
        facts_from_bytes(total, available)
    }

    /// `task_info(TASK_VM_INFO).phys_footprint`: what the process holds, compressed pages
    /// included (as `platform::memory_watchdog` reads it).
    pub(super) fn process_anon_bytes() -> Option<u64> {
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
        }
        const TASK_VM_INFO: c_int = 22;
        extern "C" {
            fn mach_task_self() -> MachPort;
            fn task_info(task: MachPort, flavor: c_int, task_info_out: *mut TaskVmInfo, task_info_out_count: *mut u32) -> KernReturn;
        }
        let mut info = TaskVmInfo::default();
        let mut count = (std::mem::size_of::<TaskVmInfo>() / 4) as u32;
        // SAFETY: task_info_out points at a TaskVmInfo sized exactly `count` 32-bit words; the
        // kernel writes at most that many.
        let rc = unsafe { task_info(mach_task_self(), TASK_VM_INFO, &mut info, &mut count) };
        (rc == 0 && info.phys_footprint > 0).then_some(info.phys_footprint)
    }

    /// Nothing that works: the system allocator keeps freed blocks of every size a loader reads
    /// (1 to 64 MB, measured) in the process's footprint and reuses them for the next
    /// allocations, `malloc_zone_pressure_relief` returns none of them, and the system takes them
    /// back only under memory pressure. A peak measured here right after a large free reads low.
    pub(super) fn trim_heap() {}
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{parse_linux_meminfo, RamFacts};

    pub(super) fn query_ram() -> Option<RamFacts> {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        parse_linux_meminfo(&text)
    }

    /// `RssAnon` of /proc/self/status.
    pub(super) fn process_anon_bytes() -> Option<u64> {
        let text = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = text.lines().find(|line| line.starts_with("RssAnon:"))?;
        let kib: u64 = line["RssAnon:".len()..].trim().trim_end_matches("kB").trim().parse().ok()?;
        kib.checked_mul(1024)
    }

    /// glibc's `malloc_trim`: every arena's free pages go back to the system.
    pub(super) fn trim_heap() {
        #[cfg(target_env = "gnu")]
        {
            extern "C" {
                fn malloc_trim(pad: usize) -> i32;
            }
            // SAFETY: a libc call that takes no pointers.
            unsafe {
                malloc_trim(0);
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::{facts_from_bytes, RamFacts};

    use makepad_windows_sys::Win32::System::SystemInformation::{
        GlobalMemoryStatusEx, MEMORYSTATUSEX,
    };

    pub(super) fn query_ram() -> Option<RamFacts> {
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..MEMORYSTATUSEX::default()
        };
        // SAFETY: one MEMORYSTATUSEX we own with dwLength set as required.
        if unsafe { GlobalMemoryStatusEx(&mut status) }.is_err() {
            return None;
        }
        facts_from_bytes(status.ullTotalPhys, status.ullAvailPhys)
    }

    /// The process's private commit (`PagefileUsage`).
    pub(super) fn process_anon_bytes() -> Option<u64> {
        use makepad_windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use makepad_windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut counters = PROCESS_MEMORY_COUNTERS::default();
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        // SAFETY: one PROCESS_MEMORY_COUNTERS we own, its cb set to its size as required.
        let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
        ok.as_bool().then_some(counters.PagefileUsage as u64)
    }

    /// Nothing to do: the process heap gives a block the size of a tensor read (beyond its
    /// virtual-memory threshold, about 1 MB) back to the system as it is freed.
    pub(super) fn trim_heap() {}
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod platform {
    use super::RamFacts;

    pub(super) fn query_ram() -> Option<RamFacts> {
        None
    }

    pub(super) fn process_anon_bytes() -> Option<u64> {
        None
    }

    pub(super) fn trim_heap() {}
}

// ---------------------------------------------------------------------------
// Host-memory admission for every model: estimate, measured peaks, the process's own memory.
// ---------------------------------------------------------------------------

/// Host memory kept free beside a model's estimate when it loads, in MiB: [`DEFAULT_RESERVE_MB`],
/// or `MAKEPAD_AI_RAM_RESERVE_MB`. Each weight read keeps the same floor
/// (`makepad_ai_loader::host_floor::reserve_mb`, which reads the same variable).
pub fn host_reserve_mb() -> u64 {
    std::env::var("MAKEPAD_AI_RAM_RESERVE_MB")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(DEFAULT_RESERVE_MB)
}

/// Whether running out of host memory ends the hub here, so a load waits for room: Linux (the
/// kernel's out-of-memory killer) and Windows (a commit that fails aborts the process). macOS
/// compresses and swaps instead: there the facts are measured and published, and a load never
/// waits for them.
pub const HOST_GATE: bool = cfg!(any(target_os = "linux", windows));

/// One measured peak: MiB above the process's memory before the load, and when (ms since the
/// epoch).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peak {
    pub mb: u64,
    pub measured_ms: u64,
}

/// A model's host-memory estimate: its newest measured peak, else its weight files as its
/// loader takes them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEstimate {
    pub mb: u64,
    /// When it was measured; `None` when it comes from the model's files.
    pub measured_ms: Option<u64>,
    /// Without a measurement: how the registry says its loader takes its files.
    pub declared: Option<crate::registry::HostLoad>,
}

impl HostEstimate {
    pub fn source(&self) -> String {
        match (self.measured_ms, self.declared) {
            (Some(_), _) => "measured".into(),
            (None, Some(load)) => format!("from its files: its loader {} them", load.as_str()),
            (None, None) => "from its files".into(),
        }
    }

    /// Measured, or declared by its registry entry: not only its files' size.
    pub fn known(&self) -> bool {
        self.measured_ms.is_some() || self.declared.is_some()
    }
}

/// The host memory `spec` is expected to take: the newest peak measured for it on any box of
/// this OS, else
/// its weight files as its registry entry's `host_load` says its loader takes them, else its
/// files. Undeclared, the llm backend counts nothing until measured: it maps its GGUF from the
/// file, and those pages are the kernel's to reclaim.
pub fn host_estimate(spec: &crate::registry::ModelSpec) -> HostEstimate {
    if let Some(peak) = peaks().get(&spec.id, OS) {
        return HostEstimate { mb: peak.mb, measured_ms: Some(peak.measured_ms), declared: None };
    }
    match spec.host_load {
        Some(load) => HostEstimate {
            mb: files_mb(spec).saturating_mul(load.percent_of_files()).div_ceil(100),
            measured_ms: None,
            declared: Some(load),
        },
        None => HostEstimate { mb: file_prior_mb(spec), measured_ms: None, declared: None },
    }
}

/// The size of `spec`'s weight files in MiB (the registry's sizes; optional files left out), as
/// an undeclared model's estimate: 0 for the llm backend, which maps its files.
pub fn file_prior_mb(spec: &crate::registry::ModelSpec) -> u64 {
    if spec.backend == "llm" {
        return 0;
    }
    files_mb(spec)
}

/// The size of `spec`'s weight files in MiB (the registry's sizes; optional files left out).
fn files_mb(spec: &crate::registry::ModelSpec) -> u64 {
    let bytes = spec
        .files
        .iter()
        .filter(|file| !file.optional)
        .filter_map(|file| file.size)
        .fold(0u64, u64::saturating_add);
    bytes_to_mb_ceil(bytes)
}

/// The newest measured host peak of each model on each OS, from this hub or a box it routed
/// through. The hub keeps it in `<cache>/host-peaks.tsv` ([`HostPeaks::keep_at`]; one
/// `model<TAB>mb<TAB>measured_ms<TAB>os` line each), so a restart remembers it.
#[derive(Default)]
pub struct HostPeaks {
    rows: std::sync::Mutex<std::collections::HashMap<(String, String), Peak>>,
    store: std::sync::Mutex<Option<std::path::PathBuf>>,
}

/// This process's table.
pub fn peaks() -> &'static HostPeaks {
    static PEAKS: std::sync::OnceLock<HostPeaks> = std::sync::OnceLock::new();
    PEAKS.get_or_init(HostPeaks::default)
}

impl HostPeaks {
    /// Reads the table kept at `path`, and keeps every later change there. A line without its
    /// OS (written before peaks were kept per OS, adopted ones among them) is dropped: whose OS
    /// it was is not known, and the model is measured again at its next load from cold.
    pub fn keep_at(&self, path: std::path::PathBuf) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
            for line in text.lines() {
                let mut fields = line.split('\t');
                let (Some(model), Some(mb), Some(at), Some(os)) = (fields.next(), fields.next(), fields.next(), fields.next()) else { continue };
                let (Ok(mb), Ok(measured_ms)) = (mb.parse(), at.parse()) else { continue };
                let peak = Peak { mb, measured_ms };
                let key = (model.to_string(), os.to_string());
                if rows.get(&key).is_none_or(|known| known.measured_ms < measured_ms) {
                    rows.insert(key, peak);
                }
            }
        }
        *self.store.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
    }

    /// The newest peak measured for `model` on `os`.
    pub fn get(&self, model: &str, os: &str) -> Option<Peak> {
        self.rows.lock().unwrap_or_else(|e| e.into_inner()).get(&(model.to_string(), os.to_string())).copied()
    }

    /// This hub measured `model`'s peak, on this OS.
    pub fn record(&self, model: &str, peak: Peak) {
        self.rows.lock().unwrap_or_else(|e| e.into_inner()).insert((model.to_string(), OS.to_string()), peak);
        self.save();
    }

    /// A peak a box of `os` measured: kept when it is newer than the one known here for that OS.
    /// True when kept.
    pub fn adopt(&self, model: &str, os: &str, peak: Peak) -> bool {
        {
            let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
            let key = (model.to_string(), os.to_string());
            if rows.get(&key).is_some_and(|known| known.measured_ms >= peak.measured_ms) {
                return false;
            }
            rows.insert(key, peak);
        }
        self.save();
        true
    }

    fn save(&self) {
        let Some(path) = self.store.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
        let mut rows: Vec<((String, String), Peak)> =
            self.rows.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(key, peak)| (key.clone(), *peak)).collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        let text: String = rows
            .iter()
            .map(|((model, os), peak)| format!("{model}\t{}\t{}\t{os}\n", peak.mb, peak.measured_ms))
            .collect();
        let mut temporary = path.clone().into_os_string();
        temporary.push(".new");
        let written = std::fs::write(&temporary, text).and_then(|()| std::fs::rename(&temporary, &path));
        if let Err(error) = written {
            eprintln!("host memory: could not keep the measured peaks at {}: {error}", path.display());
        }
    }
}

/// Hands the heap's freed pages back to the system once a model is unloaded or its load stopped
/// (the hub's `unload_backend`), and logs what that returned. glibc keeps freed blocks below its
/// mmap threshold, which rises to 32 MB as a model's big tensors are freed, in its arenas: the
/// process would still hold what the model freed, the box's available memory would not show it,
/// and the next model's load would reuse it, so its measured peak read low (kokoro measured 5 MB
/// after a stopped ACE-Step load, against about 437 MB).
pub fn return_freed_heap(after: &str) {
    let before = process_anon_mb();
    let started = std::time::Instant::now();
    platform::trim_heap();
    if let (Some(before), Some(now)) = (before, process_anon_mb()) {
        if before > now {
            eprintln!(
                "host memory: {after}: {} MB of freed heap returned to the system in {:.1} ms",
                before - now,
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
}

/// The hub process's anonymous memory in MiB, the memory that runs a machine out (file pages
/// are the kernel's to reclaim): Linux `RssAnon`, macOS `phys_footprint`, Windows private bytes.
pub fn process_anon_mb() -> Option<u64> {
    platform::process_anon_bytes().map(|bytes| bytes / BYTES_PER_MB)
}

/// The peak of the process's anonymous memory while a model loads and runs its first job,
/// above the reading when it opened. One long-lived sampler thread, started by the first
/// window, reads it every [`SAMPLE_EVERY`] while any window is open and sleeps otherwise; a
/// load holds what it takes for seconds, so a reading in between does not miss it.
pub struct PeakWindow {
    id: u64,
    before_mb: u64,
}

/// Between two readings of an open [`PeakWindow`].
pub const SAMPLE_EVERY: std::time::Duration = std::time::Duration::from_millis(50);

#[derive(Default)]
struct Sampler {
    open: std::sync::Mutex<(u64, std::collections::HashMap<u64, u64>)>,
    wake: std::sync::Condvar,
}

fn sampler() -> &'static Sampler {
    static SAMPLER: std::sync::OnceLock<&'static Sampler> = std::sync::OnceLock::new();
    SAMPLER.get_or_init(|| {
        let sampler: &'static Sampler = Box::leak(Box::default());
        let spawned = std::thread::Builder::new().name("host-memory-peaks".into()).spawn(move || loop {
            {
                let mut open = sampler.open.lock().unwrap_or_else(|e| e.into_inner());
                while open.1.is_empty() {
                    open = sampler.wake.wait(open).unwrap_or_else(|e| e.into_inner());
                }
                if let Some(now) = process_anon_mb() {
                    for max in open.1.values_mut() {
                        *max = (*max).max(now);
                    }
                }
            }
            std::thread::sleep(SAMPLE_EVERY);
        });
        if let Err(error) = spawned {
            eprintln!("host memory: no peak sampler ({error}); peaks are read at the window's ends only");
        }
        sampler
    })
}

impl PeakWindow {
    /// None where this platform gives no reading.
    pub fn open() -> Option<Self> {
        let before_mb = process_anon_mb()?;
        let sampler = sampler();
        let mut open = sampler.open.lock().unwrap_or_else(|e| e.into_inner());
        open.0 += 1;
        let id = open.0;
        open.1.insert(id, before_mb);
        sampler.wake.notify_one();
        Some(Self { id, before_mb })
    }

    /// The peak over the window, in MiB above the reading at its opening.
    pub fn close(self) -> u64 {
        let max = sampler().open.lock().unwrap_or_else(|e| e.into_inner()).1.remove(&self.id).unwrap_or(self.before_mb);
        let max = process_anon_mb().map_or(max, |now| max.max(now));
        max.saturating_sub(self.before_mb)
    }
}

impl Drop for PeakWindow {
    fn drop(&mut self) {
        sampler().open.lock().unwrap_or_else(|e| e.into_inner()).1.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_boundaries_are_exact() {
        let facts = RamFacts {
            free_mb: 12_288,
            total_mb: 16_384,
        };
        assert_eq!(admit(facts, 8_192, 4_096), Admit::Ok);
        assert_eq!(
            admit(RamFacts { free_mb: 12_287, ..facts }, 8_192, 4_096),
            Admit::NeedsEviction { short_mb: 1 }
        );
        assert_eq!(
            admit(RamFacts { free_mb: 4_096, ..facts }, 8_192, 4_096),
            Admit::NeedsEviction { short_mb: 8_192 }
        );
        assert_eq!(
            admit(
                RamFacts {
                    free_mb: 16_384,
                    ..facts
                },
                12_288,
                4_096
            ),
            Admit::Ok,
            "a requirement equal to total RAM is still possible"
        );

        let impossible = admit(facts, 12_289, 4_096);
        assert!(matches!(impossible, Admit::Never { .. }));
        assert!(matches!(
            admit(facts, u64::MAX, 1),
            Admit::Never { .. }
        ));
    }

    #[test]
    fn lru_is_oldest_first_and_deterministic_on_ties() {
        let rows = [
            ("new", 300, 900),
            ("tie-b", 100, 700),
            ("never", 0, 500),
            ("tie-a", 100, 600),
        ];
        assert_eq!(
            lru_order(&rows),
            vec![
                ("never", 0, 500),
                ("tie-a", 100, 600),
                ("tie-b", 100, 700),
                ("new", 300, 900),
            ]
        );
        assert!(lru_order::<&str>(&[]).is_empty());
    }

    #[test]
    fn estimate_is_monotonic_in_file_size_and_context() {
        assert_eq!(estimate_model_ram_mb(0, 0), FIXED_OVERHEAD_MB);
        assert_eq!(estimate_model_ram_mb(BYTES_PER_MB, 0), FIXED_OVERHEAD_MB + 1);
        assert_eq!(estimate_model_ram_mb(0, 16), FIXED_OVERHEAD_MB + 1);

        let file_sizes = [0, 1, BYTES_PER_MB, BYTES_PER_MB + 1, u64::MAX];
        let contexts = [0, 1, 16, 32_768, u32::MAX];
        for pair in file_sizes.windows(2) {
            assert!(estimate_model_ram_mb(pair[0], 32_768) <= estimate_model_ram_mb(pair[1], 32_768));
        }
        for pair in contexts.windows(2) {
            assert!(estimate_model_ram_mb(9 * 1024 * BYTES_PER_MB, pair[0])
                <= estimate_model_ram_mb(9 * 1024 * BYTES_PER_MB, pair[1]));
        }
    }

    #[test]
    fn linux_meminfo_requires_total_and_available() {
        assert_eq!(
            parse_linux_meminfo("MemTotal: 16384 kB\nMemAvailable: 4096 kB\n"),
            Some(RamFacts { free_mb: 4, total_mb: 16 })
        );
        assert_eq!(parse_linux_meminfo("MemTotal: 16384 kB\n"), None);
        assert_eq!(
            parse_linux_meminfo("MemTotal: 16384 kB\nMemAvailable: nope kB\n"),
            None
        );
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    fn query_ram_reports_sane_host_values() {
        let facts = query_ram().expect("host RAM query must succeed");
        assert!(facts.total_mb > 0);
        assert!(facts.free_mb > 0);
        assert!(facts.free_mb <= facts.total_mb);
    }
}
