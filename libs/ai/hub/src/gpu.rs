//! Cheap GPU info for `/health`: name and VRAM via `nvidia-smi` when present
//! (the content boxes are NVIDIA/Windows), otherwise nulls. Queried through a
//! short-lived cache so /health polling does not spawn a process per request.

use crate::child_process;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Default)]
pub struct GpuInfo {
    pub name: Option<String>,
    pub vram_free_mb: Option<u64>,
    pub vram_total_mb: Option<u64>,
    /// CUDA compute capability as reported by nvidia-smi (e.g. 8.9 for a
    /// RTX 4090, 12.0 for Blackwell). Backends use this to fail closed on
    /// architecture-gated checkpoints (NVFP4 tiers).
    pub compute_cap: Option<f64>,
}

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
    }
}

/// TTL cache around `query_gpu`.
#[derive(Default)]
pub struct GpuCache {
    inner: Mutex<Option<(Instant, GpuInfo)>>,
}

impl GpuCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self) -> GpuInfo {
        let mut guard = self.inner.lock().unwrap();
        if let Some((when, info)) = guard.as_ref() {
            if when.elapsed() < CACHE_TTL {
                return info.clone();
            }
        }
        let info = query_gpu();
        *guard = Some((Instant::now(), info.clone()));
        info
    }
}
