//! Cheap GPU info for `/health`: Metal's advisory working set or physical memory
//! on macOS, otherwise VRAM via `nvidia-smi` when present. A short-lived cache avoids
//! probing the device on every health request, and its refresh runs off the
//! request path ([`GpuCache`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Default)]
pub struct GpuInfo {
    pub name: Option<String>,
    pub vram_free_mb: Option<u64>,
    pub vram_total_mb: Option<u64>,
    /// Unified memory figures guide sizing and eviction, never refuse a load.
    pub unified: bool,
    /// CUDA compute capability as reported by nvidia-smi (e.g. 8.9 for a
    /// GPU-A, 12.0 for Blackwell). Backends use this to fail closed on
    /// architecture-gated checkpoints (NVFP4 tiers).
    pub compute_cap: Option<f64>,
}

#[cfg(all(target_os = "macos", feature = "ai-common"))]
pub fn query_gpu() -> GpuInfo {
    let Ok((free, total)) = makepad_ai_common::backend::gpu_memory_info() else {
        return GpuInfo::default();
    };
    GpuInfo {
        name: makepad_ai_common::backend::device_name(),
        vram_free_mb: Some((free as u64) / (1 << 20)),
        vram_total_mb: Some((total as u64) / (1 << 20)),
        compute_cap: None,
        unified: true,
    }
}

#[cfg(all(target_os = "macos", not(feature = "ai-common")))]
pub fn query_gpu() -> GpuInfo {
    use std::ffi::{c_char, c_void};
    use std::mem::size_of;

    extern "C" {
        fn sysctlbyname(
            name: *const c_char,
            old: *mut c_void,
            old_len: *mut usize,
            new: *mut c_void,
            new_len: usize,
        ) -> i32;
    }
    let mut bytes = 0u64;
    let mut len = size_of::<u64>();
    // SAFETY: hw.memsize writes one u64 into the supplied buffer.
    let rc = unsafe {
        sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&mut bytes as *mut u64).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    GpuInfo {
        vram_total_mb: (rc == 0 && len == size_of::<u64>()).then_some(bytes / (1 << 20)),
        unified: true,
        ..Default::default()
    }
}

/// The process's one nvidia-smi probe: whether the tool is there, and the wait after a failure.
#[cfg(not(target_os = "macos"))]
static PROBE: nvidia::Probe = nvidia::Probe::new();

#[cfg(not(target_os = "macos"))]
pub fn query_gpu() -> GpuInfo {
    PROBE.query_with(Instant::now, || {
        crate::child_process::output(std::process::Command::new("nvidia-smi").args([
            "--query-gpu=name,memory.free,memory.total,compute_cap",
            "--format=csv,noheader,nounits",
        ]))
    })
}

/// Whether nvidia-smi may be asked now ([`nvidia::Probe::usable`]); never on macOS, which has none.
pub(crate) fn nvidia_smi_usable() -> bool {
    #[cfg(not(target_os = "macos"))]
    { PROBE.usable(Instant::now()) }
    #[cfg(target_os = "macos")]
    { false }
}

#[cfg(any(not(target_os = "macos"), test))]
mod nvidia {
    use super::{Arc, AtomicBool, Duration, GpuInfo, Instant, Mutex, OnceLock, Ordering};
    use std::{io, process::Output};

    pub(super) struct Probe {
        missing: AtomicBool,
        state: Mutex<State>,
    }

    struct State {
        // Overlapping callers share one sample, including its failure.
        in_flight: Option<Arc<OnceLock<GpuInfo>>>,
        // Next permitted attempt and the delay that led to it.
        retry: Option<(Instant, Duration)>,
    }

    impl Probe {
        pub(super) const fn new() -> Self {
            Self {
                missing: AtomicBool::new(false),
                state: Mutex::new(State { in_flight: None, retry: None }),
            }
        }

        pub(super) fn query_with(
            &self,
            now: impl Fn() -> Instant,
            run: impl FnOnce() -> io::Result<Output>,
        ) -> GpuInfo {
            if self.missing.load(Ordering::Relaxed) {
                return GpuInfo::default();
            }
            let start = now();
            let sample = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if self.missing.load(Ordering::Relaxed)
                    || state.retry.is_some_and(|(at, _)| start < at)
                {
                    return GpuInfo::default();
                }
                Arc::clone(state.in_flight.get_or_insert_with(Arc::default))
            };
            sample
                .get_or_init(|| {
                    let result = run().and_then(parse_output);
                    self.finish(result, now())
                })
                .clone()
        }

        /// Whether another nvidia-smi query may run at `now`: the tool is there and no failure's
        /// wait is pending. Every other caller of the tool asks this first, so a missing tool or a
        /// failing driver costs one spawn, not one per caller.
        pub(super) fn usable(&self, now: Instant) -> bool {
            !self.missing.load(Ordering::Relaxed)
                && !self.state.lock().unwrap_or_else(|e| e.into_inner()).retry.is_some_and(|(at, _)| now < at)
        }

        fn finish(&self, result: io::Result<GpuInfo>, finished: Instant) -> GpuInfo {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.in_flight = None;
            let previous = state.retry.take();
            match result {
                Ok(info) => {
                    drop(state);
                    if previous.is_some() {
                        eprintln!("gpu facts: nvidia-smi queries work again");
                    }
                    info
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.missing.store(true, Ordering::Relaxed);
                    drop(state);
                    eprintln!("gpu facts: no nvidia-smi on this machine; GPU facts stay unknown");
                    GpuInfo::default()
                }
                Err(error) => {
                    let delay = previous.map_or(Duration::from_secs(2), |(_, delay)| {
                        (delay * 2).min(Duration::from_secs(600))
                    });
                    state.retry = Some((finished + delay, delay));
                    drop(state);
                    if previous.is_none() {
                        eprintln!("gpu facts: nvidia-smi query failed: {error}");
                    }
                    GpuInfo::default()
                }
            }
        }
    }

    fn parse_output(output: Output) -> io::Result<GpuInfo> {
        if !output.status.success() {
            return Err(io::Error::other(output.status.to_string()));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines().next().and_then(parse_line).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "expected a GPU row from nvidia-smi")
        })
    }

    fn parse_line(line: &str) -> Option<GpuInfo> {
        let mut parts = line.split(',').map(str::trim);
        let name = parts.next()?;
        let free = parts.next()?;
        let total = parts.next()?;
        let cap = parts.next()?;
        if name.is_empty() || parts.next().is_some() {
            return None;
        }
        let info = GpuInfo {
            name: Some(name.to_string()),
            vram_free_mb: free.parse().ok(),
            vram_total_mb: total.parse().ok(),
            compute_cap: cap.parse().ok(),
            unified: false,
        };
        // A driver can report unavailable fields alongside the facts it knows.
        let unavailable = |s| matches!(s, "N/A" | "[N/A]" | "[Not Supported]");
        (info.vram_free_mb.is_some() || info.vram_total_mb.is_some() || info.compute_cap.is_some()
            || [free, total, cap].into_iter().all(unavailable)).then_some(info)
    }
}

/// TTL cache around `query_gpu`, refreshed off the request path. A read gets the last facts at
/// once; facts older than [`CACHE_TTL`] ask the refresher thread for new ones. A query runs
/// `nvidia-smi` (about 100 ms, more on a busy box), and the hub's one request loop answers
/// `/health` and `/models` (every router's probe) beside the job polls and submits: a query
/// there held all of them up. Only the first read waits for a query.
#[derive(Default)]
pub struct GpuCache {
    slot: Arc<Slot>,
}

#[derive(Default)]
struct Slot {
    facts: Mutex<Option<(Instant, GpuInfo)>>,
    /// A refresh is asked for and not done yet (at most one per cache).
    asked: AtomicBool,
}

impl Slot {
    fn refresh(&self) {
        let info = query_gpu();
        *self.facts.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), info));
        self.asked.store(false, Ordering::Release);
    }
}

/// The one long-lived thread that queries for every cache; `None` when it could not start (then
/// a stale read queries in place, as before).
fn refresher() -> Option<&'static Mutex<mpsc::Sender<Arc<Slot>>>> {
    static REFRESHER: OnceLock<Option<Mutex<mpsc::Sender<Arc<Slot>>>>> = OnceLock::new();
    REFRESHER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Arc<Slot>>();
        let spawned = std::thread::Builder::new().name("ai-hub-gpu-facts".into()).spawn(move || {
            for slot in receiver { slot.refresh(); }
        });
        match spawned {
            Ok(_) => Some(Mutex::new(sender)),
            Err(error) => {
                eprintln!("gpu facts: no refresher thread ({error}); a stale read queries in place");
                None
            }
        }
    }).as_ref()
}

impl GpuCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// A cache that answers `info` for good and never probes the host, so a test whose outcome
    /// depends on the card (unified or not, its figures) states it instead of taking the machine's.
    #[cfg(test)]
    pub fn fixed(info: GpuInfo) -> Self {
        // The refresh counts as asked for and never runs, so the facts never age out.
        let slot = Slot { facts: Mutex::new(Some((Instant::now(), info))), asked: AtomicBool::new(true) };
        Self { slot: Arc::new(slot) }
    }

    pub fn get(&self) -> GpuInfo {
        let mut guard = self.slot.facts.lock().unwrap_or_else(|e| e.into_inner());
        let Some((when, info)) = guard.as_ref() else {
            let info = query_gpu();
            *guard = Some((Instant::now(), info.clone()));
            return info;
        };
        let info = info.clone();
        if when.elapsed() >= CACHE_TTL && !self.slot.asked.swap(true, Ordering::AcqRel) {
            drop(guard);
            let sent = refresher().is_some_and(|sender| {
                sender.lock().unwrap_or_else(|e| e.into_inner()).send(self.slot.clone()).is_ok()
            });
            if !sent {
                self.slot.refresh();
                return self.slot.facts.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(info, |(_, fresh)| fresh.clone());
            }
        }
        info
    }
}

#[cfg(test)]
mod tests {
    #[cfg(any(unix, windows))]
    #[test]
    fn gpu_facts_and_probe_retries() {
        use super::{nvidia::Probe, Duration, GpuInfo, Instant};
        use std::cell::Cell;
        use std::io;
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt;
        use std::process::{ExitStatus, Output};

        #[cfg(all(target_os = "macos", not(feature = "ai-common")))]
        {
            // Only the non-macOS query compiles the subprocess call.
            let info = super::query_gpu();
            assert!(info.unified, "macOS should report unified memory");
            assert!(info.vram_total_mb.is_some(), "hw.memsize should supply total memory");
            assert!(
                info.vram_free_mb.is_none() && info.name.is_none() && info.compute_cap.is_none(),
                "hw.memsize should supply only the memory total"
            );
            assert!(!super::nvidia_smi_usable(), "a Mac should never ask nvidia-smi");
        }

        let start = Instant::now();
        let calls = Cell::new(0);
        let query = |probe: &Probe, seconds, result| {
            probe.query_with(|| start + Duration::from_secs(seconds), || {
                calls.set(calls.get() + 1);
                result
            })
        };
        let output = |status, text: &str| Ok(Output {
            status: ExitStatus::from_raw(status),
            stdout: text.as_bytes().to_vec(),
            stderr: Vec::new(),
        });
        let valid = || output(0, "GPU-A, 1000, 2000, 8.9");
        let unknown = |info: GpuInfo| {
            !info.unified && info.name.is_none() && info.vram_free_mb.is_none()
                && info.vram_total_mb.is_none() && info.compute_cap.is_none()
        };

        let probe = Probe::new();
        query(&probe, 0, Err(io::ErrorKind::NotFound.into()));
        for at in [0, 1, 3600] {
            assert!(unknown(query(&probe, at, valid())), "a missing tool should leave every fact unknown");
            assert_eq!(calls.get(), 1, "a missing tool should never be retried");
            assert!(!probe.usable(start + Duration::from_secs(at)), "a missing tool should keep every other caller from spawning it");
        }

        let probe = Probe::new();
        calls.set(0);
        let mut at = 0;
        for (i, wait) in [2, 4, 8, 16, 32, 64, 128, 256, 512, 600, 600].into_iter().enumerate() {
            let failure = match i {
                0 => Err(io::ErrorKind::PermissionDenied.into()),
                1 => output(1 << 8, "GPU-A, 1000, 2000, 8.9"),
                2 => output(0, ""),
                3 => output(0, "invalid row\nGPU-A, 1000, 2000, 8.9"),
                _ => output(0, "GPU-A, bad, bad, bad"),
            };
            assert!(unknown(query(&probe, at, failure)), "failure {i} should return unknown facts");
            assert_eq!(calls.get(), i + 1, "retry {i} should run at its deadline");
            at += wait;
            assert!(unknown(query(&probe, at - 1, valid())), "facts should stay unknown during the {wait}s wait");
            assert_eq!(calls.get(), i + 1, "the runner should wait the full {wait}s");
            assert!(!probe.usable(start + Duration::from_secs(at - 1)) && probe.usable(start + Duration::from_secs(at)), "other callers should wait out the same {wait}s");
        }

        let before = calls.get();
        query(&probe, at, valid());
        assert_eq!(calls.get(), before + 1, "retries should continue at the ten-minute cap");

        let probe = Probe::new();
        calls.set(0);
        query(&probe, 0, Err(io::ErrorKind::PermissionDenied.into()));
        let info = query(&probe, 2, valid());
        assert_eq!(info.name.as_deref(), Some("GPU-A"), "recovery should report the device name");
        assert_eq!(info.vram_free_mb, Some(1000), "recovery should report free VRAM");
        assert_eq!(info.vram_total_mb, Some(2000), "recovery should report total VRAM");
        assert_eq!(info.compute_cap, Some(8.9), "recovery should report compute capability");
        assert!(!info.unified, "the NVIDIA query should report discrete memory");
        query(&probe, 2, Err(io::ErrorKind::PermissionDenied.into()));
        assert_eq!(calls.get(), 3, "success should permit an immediate new probe");
        let partial = query(&probe, 4, output(0, "GPU-A, N/A, 2000, [Not Supported]"));
        assert_eq!(calls.get(), 4, "the first failure after recovery should wait only 2s");
        assert_eq!(partial.vram_total_mb, Some(2000), "unavailable fields should preserve known facts");
        assert!(partial.vram_free_mb.is_none() && partial.compute_cap.is_none(), "unavailable fields should stay unknown");
    }
}
