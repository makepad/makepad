//! CPU RAM measurement and admission policy for resident GGUF models.
//!
//! This module only measures and classifies. The caller owns model loading,
//! unloading, and taking a fresh measurement after each eviction.

/// Default RAM left for the OS and the rest of the user's applications.
pub const DEFAULT_RESERVE_MB: u64 = 4096;

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
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{parse_linux_meminfo, RamFacts};

    pub(super) fn query_ram() -> Option<RamFacts> {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        parse_linux_meminfo(&text)
    }
}

#[cfg(windows)]
mod platform {
    use super::{facts_from_bytes, RamFacts};

    type Bool = i32;

    /// `MEMORYSTATUSEX` (sysinfoapi.h).
    #[repr(C)]
    #[derive(Default)]
    struct MemoryStatusEx {
        dw_length: u32,
        dw_memory_load: u32,
        ull_total_phys: u64,
        ull_avail_phys: u64,
        ull_total_page_file: u64,
        ull_avail_page_file: u64,
        ull_total_virtual: u64,
        ull_avail_virtual: u64,
        ull_avail_extended_virtual: u64,
    }

    const _: () = assert!(std::mem::size_of::<MemoryStatusEx>() == 64);

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> Bool;
    }

    pub(super) fn query_ram() -> Option<RamFacts> {
        let mut status = MemoryStatusEx {
            dw_length: std::mem::size_of::<MemoryStatusEx>() as u32,
            ..MemoryStatusEx::default()
        };
        // SAFETY: one MEMORYSTATUSEX we own with dwLength set as required.
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
            return None;
        }
        facts_from_bytes(status.ull_total_phys, status.ull_avail_phys)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod platform {
    use super::RamFacts;

    pub(super) fn query_ram() -> Option<RamFacts> {
        None
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
