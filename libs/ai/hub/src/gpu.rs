//! Cheap GPU info for `/health`: Metal's advisory working set on macOS,
//! otherwise VRAM via `nvidia-smi` when present. A short-lived cache avoids
//! probing the device on every health request, and its refresh runs off the
//! request path ([`GpuCache`]).

#[cfg(not(all(target_os = "macos", feature = "ai-common")))]
use crate::child_process;
#[cfg(not(all(target_os = "macos", feature = "ai-common")))]
use std::process::Command;
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

#[cfg(not(all(target_os = "macos", feature = "ai-common")))]
pub fn query_gpu() -> GpuInfo {
    // nvidia-smi is on PATH on the GPU boxes (Windows and Linux). On machines
    // without it (macOS dev laptops) this just fails fast and we report nulls.
    let output = child_process::output(
        Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.free,memory.total,compute_cap",
            "--format=csv,noheader,nounits",
        ]),
    );
    let output = match output {
        Ok(output) if output.status.success() => output,
        _ => return GpuInfo::default(),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let line = match text.lines().next() {
        Some(line) if !line.trim().is_empty() => line,
        _ => return GpuInfo::default(),
    };
    let mut parts = line.split(',').map(|part| part.trim());
    GpuInfo {
        name: parts.next().map(|name| name.to_string()),
        vram_free_mb: parts.next().and_then(|v| v.parse().ok()),
        vram_total_mb: parts.next().and_then(|v| v.parse().ok()),
        compute_cap: parts.next().and_then(|v| v.parse().ok()),
        unified: false,
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
