//! VRAM residency/admission policy for the single-GPU service worker.
//!
//! The service never assumes an unload "worked": every eviction is followed
//! by a fresh `nvidia-smi` free-memory read, and a heavy model only loads
//! once fresh free VRAM covers its registry estimate plus a safety reserve.
//! The policy is deliberately simple and observable:
//!
//! - estimated peak per model = registry `vram_gb` (0/None = treated as
//!   VRAM-free: CPU backends, subprocess workers that manage their own
//!   lifetime, testpattern);
//! - default = ONE heavy resident at a time: before a different model loads,
//!   every other truthful resident is retired (LRU first), except explicitly
//!   pinned small/hot models (`MAKEPAD_AI_PIN_MODELS`);
//! - admission gate: fresh free MB >= estimate + reserve
//!   (`MAKEPAD_AI_VRAM_RESERVE_MB`, default 2048). Pins are evicted too when
//!   the gate cannot be met without them;
//! - after each unload the worker polls free VRAM until it actually rose
//!   (serialized teardown verification — Windows/WDDM releases lag);
//! - a classified CUDA OOM evicts everything else once and retries once;
//!   there is NO silent CPU/Python/other-node fallback — the second failure
//!   is the job's explicit error;
//! - a client can hold a model for a while (`POST /control/model {"action":
//!   "hold"}`, [`Holds`]): held, it is kept like a pin until its holders
//!   release it or stop renewing, and it still goes when a load cannot find
//!   the memory otherwise or the UI asks for memory, with a log line naming
//!   its holders;
//! - unified memory uses the figures to choose evictions and window sizes;
//!   after reclamation it admits even beyond the recommended working set;
//! - machines without a memory probe skip the byte gating but keep the
//!   deterministic single-resident retire semantics.
//!
//! Everything here is pure math/classification plus the polling helper; the
//! worker in `server.rs` owns the actual backend map and unload calls.

use crate::error::AssetAiError;
use crate::gpu::query_gpu;
use crate::registry::ModelSpec;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Default admission safety reserve on top of a model's estimated peak.
pub const DEFAULT_RESERVE_MB: u64 = 2048;
/// How long to wait for freed VRAM to become visible after one unload.
pub const EVICT_VERIFY_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const UNKNOWN_USABLE_MB: u64 = u64::MAX;

/// Last measured post-eviction VRAM ceiling. The measurement itself happens
/// only at startup or when the service has retired every resident; ordinary
/// `/health` polling must not mistake a currently loaded model for permanent
/// driver/display overhead.
#[derive(Debug)]
pub struct UsableVram(AtomicU64);

impl UsableVram {
    pub fn new(value: Option<u64>) -> Self {
        Self(AtomicU64::new(value.unwrap_or(UNKNOWN_USABLE_MB)))
    }

    pub fn get(&self) -> Option<u64> {
        match self.0.load(Ordering::Relaxed) {
            UNKNOWN_USABLE_MB => None,
            value => Some(value),
        }
    }

    pub fn refresh(&self) -> Option<u64> {
        self.refresh_from_gpu(&query_gpu())
    }

    pub fn refresh_from_gpu(&self, gpu: &crate::gpu::GpuInfo) -> Option<u64> {
        let value = usable_vram_mb(gpu);
        if let Some(value) = value {
            self.0.store(value, Ordering::Relaxed);
            Some(value)
        } else {
            self.get()
        }
    }
}

/// Fresh VRAM samples bracketing an allocator-pool trim. The final admission
/// poll may observe more of the asynchronous driver release than the immediate
/// `after` sample, so diagnostics compute the released amount against the
/// final value with [`Self::released_mb_at`].
#[derive(Clone, Debug)]
pub struct CachedPoolTrim {
    pub before_free_mb: Option<u64>,
    pub after_gpu: crate::gpu::GpuInfo,
}

impl CachedPoolTrim {
    pub fn after_free_mb(&self) -> Option<u64> {
        self.after_gpu.vram_free_mb
    }

    pub fn released_mb_at(&self, final_free_mb: u64) -> u64 {
        self.before_free_mb
            .map(|before| final_free_mb.saturating_sub(before))
            .unwrap_or(0)
    }
}

/// Trim cached device allocations between two uncached GPU samples. Keeping
/// the sampler and trim operation injectable makes the admission rule testable
/// without CUDA or `nvidia-smi`.
pub fn trim_cached_pool_with<Q, T>(mut query: Q, trim: T) -> CachedPoolTrim
where
    Q: FnMut() -> crate::gpu::GpuInfo,
    T: FnOnce(),
{
    let before_free_mb = query().vram_free_mb;
    trim();
    CachedPoolTrim {
        before_free_mb,
        after_gpu: query(),
    }
}

/// Admission decision from the post-trim sample. `previous_free_mb` keeps a
/// briefly stale NVML sample from making free memory appear to go backwards.
pub fn admission_after_trim(
    trimmed: &CachedPoolTrim,
    previous_free_mb: u64,
    need_mb: u64,
) -> (u64, bool) {
    let free_mb = trimmed
        .after_free_mb()
        .map(|after| after.max(previous_free_mb))
        .unwrap_or(previous_free_mb);
    (free_mb, free_mb >= need_mb)
}

/// `total - non_evictable_baseline`, expressed from one idle-card sample.
/// Clamping protects the public health contract from inconsistent driver
/// samples that briefly report free memory above total memory.
pub fn usable_vram_mb(gpu: &crate::gpu::GpuInfo) -> Option<u64> {
    Some(gpu.vram_free_mb?.min(gpu.vram_total_mb?))
}

/// Why a model can never pass the load-time byte gate on this node.
pub fn too_small_note(spec: &ModelSpec, reserve_mb: u64, usable_mb: u64) -> Option<String> {
    let estimate_mb = estimated_peak_mb(spec);
    let need_mb = estimate_mb.saturating_add(reserve_mb);
    (estimate_mb > 0 && need_mb > usable_mb)
        .then(|| format!("needs {need_mb} MB, this card can free {usable_mb}"))
}

pub fn insufficient_vram_message(
    model_id: &str,
    estimate_mb: u64,
    reserve_mb: u64,
    free_mb: u64,
    released_pool_mb: u64,
) -> String {
    let need_mb = estimate_mb.saturating_add(reserve_mb);
    format!(
        "insufficient VRAM for {model_id}: need {need_mb} MB (estimate {estimate_mb} MB + reserve {reserve_mb} MB), only {free_mb} MB free after evicting every resident and releasing {released_pool_mb} MB of cached pool — refusing to load (no CPU/other-node fallback)"
    )
}

/// The longest a hold lasts without a renewal: a holder that went away
/// without releasing its model frees it after this at the latest.
pub const MAX_HOLD: Duration = Duration::from_secs(600);

/// Models clients keep resident for a while, say an app whose people are
/// about to use it again any moment. Per model, its holders and until when.
/// A held model is kept like a pin: a switch to another model loads beside
/// it, and idle eviction passes it by. It still goes when a load cannot find
/// the memory otherwise, or when the UI asks for memory.
#[derive(Debug, Default)]
pub struct Holds(Mutex<HashMap<String, HashMap<String, Instant>>>);

impl Holds {
    /// `holder` keeps `model` for `time` (at most [`MAX_HOLD`]) from now on.
    /// True when the holder did not hold it already (not a renewal).
    pub fn hold(&self, model: &str, holder: &str, time: Duration) -> bool {
        self.hold_at(model, holder, time, Instant::now())
    }

    fn hold_at(&self, model: &str, holder: &str, time: Duration, now: Instant) -> bool {
        let mut holds = self.0.lock().unwrap_or_else(|p| p.into_inner());
        prune(&mut holds, now);
        holds.entry(model.to_string()).or_default().insert(holder.to_string(), now + time.min(MAX_HOLD)).is_none()
    }

    /// `holder` lets `model` go; true when it held it.
    pub fn release(&self, model: &str, holder: &str) -> bool {
        let mut holds = self.0.lock().unwrap_or_else(|p| p.into_inner());
        prune(&mut holds, Instant::now());
        let released = holds.get_mut(model).is_some_and(|by| by.remove(holder).is_some());
        holds.retain(|_, by| !by.is_empty());
        released
    }

    /// Who holds `model` now, by name.
    pub fn holders(&self, model: &str) -> Vec<String> {
        self.holders_at(model, Instant::now())
    }

    fn holders_at(&self, model: &str, now: Instant) -> Vec<String> {
        let holds = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let mut holders: Vec<String> =
            holds.get(model).map(|by| by.iter().filter(|(_, until)| **until > now).map(|(holder, _)| holder.clone()).collect()).unwrap_or_default();
        holders.sort();
        holders
    }

    pub fn held(&self, model: &str) -> bool {
        !self.holders(model).is_empty()
    }
}

fn prune(holds: &mut HashMap<String, HashMap<String, Instant>>, now: Instant) {
    holds.retain(|_, by| {
        by.retain(|_, until| *until > now);
        !by.is_empty()
    });
}

#[derive(Clone, Debug)]
pub struct ResidencyConfig {
    /// Safety reserve in MB added to every admission target.
    pub reserve_mb: u64,
    /// Model ids kept resident through ordinary model switches (evicted only
    /// when admission cannot be met without them, and exempt from idle
    /// eviction). For genuinely small/hot models only.
    pub pins: HashSet<String>,
    /// Evict residents idle longer than this between jobs; `None` = off.
    pub idle_evict: Option<Duration>,
}

impl Default for ResidencyConfig {
    fn default() -> Self {
        Self {
            reserve_mb: DEFAULT_RESERVE_MB,
            pins: HashSet::new(),
            idle_evict: None,
        }
    }
}

impl ResidencyConfig {
    /// Reads `MAKEPAD_AI_VRAM_RESERVE_MB`, `MAKEPAD_AI_PIN_MODELS` (comma
    /// separated model ids) and `MAKEPAD_AI_IDLE_EVICT_SECS` (0/absent =
    /// off). Malformed values fall back to defaults — a typo must not turn
    /// the policy off silently, so defaults are the conservative ones.
    pub fn from_env() -> Self {
        Self::from_values(
            std::env::var("MAKEPAD_AI_VRAM_RESERVE_MB").ok().as_deref(),
            std::env::var("MAKEPAD_AI_PIN_MODELS").ok().as_deref(),
            std::env::var("MAKEPAD_AI_IDLE_EVICT_SECS").ok().as_deref(),
        )
    }

    pub fn from_values(
        reserve_mb: Option<&str>,
        pins: Option<&str>,
        idle_evict_secs: Option<&str>,
    ) -> Self {
        let mut config = Self::default();
        if let Some(value) = reserve_mb {
            if let Ok(mb) = value.trim().parse::<u64>() {
                config.reserve_mb = mb;
            }
        }
        if let Some(list) = pins {
            config.pins = list
                .split(',')
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty())
                .collect();
        }
        if let Some(value) = idle_evict_secs {
            if let Ok(secs) = value.trim().parse::<u64>() {
                if secs > 0 {
                    config.idle_evict = Some(Duration::from_secs(secs));
                }
            }
        }
        config
    }
}

/// Registry `vram_gb` -> MB estimate; 0 for models that declare none (CPU
/// backends, subprocess workers, testpattern) — those skip byte gating.
pub fn estimated_peak_mb(spec: &ModelSpec) -> u64 {
    match spec.vram_gb {
        Some(gb) if gb.is_finite() && gb > 0.0 => (gb * 1024.0).ceil() as u64,
        _ => 0,
    }
}

/// Fresh (uncached) free VRAM in MB; `None` on machines without nvidia-smi.
pub fn fresh_free_mb() -> Option<u64> {
    query_gpu().vram_free_mb
}

/// Classifies backend/CUDA error text as an out-of-memory failure eligible
/// for the evict-once-and-retry path. Conservative substring set — a
/// misclassification retries a deterministic failure once (wasteful but
/// safe); missing one leaves the job with its explicit error (also safe).
pub fn error_is_oom(error: &AssetAiError) -> bool {
    let text = match error {
        AssetAiError::Backend(m)
        | AssetAiError::Io(m)
        | AssetAiError::Http(m)
        | AssetAiError::Download(m)
        | AssetAiError::Unavailable(m) => m.as_str(),
        _ => return false,
    };
    let lower = text.to_ascii_lowercase();
    lower.contains("out of memory")
        || lower.contains("outofmemory")
        || lower.contains("cuda_error_out_of_memory")
        || lower.contains("cudaerrormemoryallocation")
        || lower.contains("cublas_status_alloc_failed")
        || lower.contains("cuda error 2 ")
        || lower.contains("erroroutofdevicememory")
        || lower.contains("alloc failed")
        || lower.contains("allocation failure")
}

/// Polls fresh free VRAM until it reaches `target_mb` or `timeout` passes.
/// Returns the last observed value (`None` = no NVML on this machine, which
/// callers treat as "gating not applicable").
pub fn wait_free_at_least(target_mb: u64, timeout: Duration) -> Option<u64> {
    let deadline = Instant::now() + timeout;
    let mut seen = fresh_free_mb()?;
    while seen < target_mb && Instant::now() < deadline {
        std::thread::sleep(POLL_INTERVAL);
        seen = fresh_free_mb()?;
    }
    Some(seen)
}

/// How long free VRAM may stay flat before an admission wait concludes that
/// nothing more is coming (the driver publishes a free within a poll or two).
pub const ADMIT_PLATEAU: Duration = Duration::from_millis(1500);

/// [`wait_free_at_least`] for an eviction check: ends when `target_mb` is
/// reached, when the timeout passes, or as soon as free memory has stopped
/// rising for `plateau`.
pub fn wait_free_settled(target_mb: u64, timeout: Duration, plateau: Duration) -> Option<u64> {
    wait_free_settled_with(target_mb, timeout, plateau, POLL_INTERVAL, fresh_free_mb, std::thread::sleep)
}

fn wait_free_settled_with(
    target_mb: u64,
    timeout: Duration,
    plateau: Duration,
    poll: Duration,
    mut sample: impl FnMut() -> Option<u64>,
    mut sleep: impl FnMut(Duration),
) -> Option<u64> {
    let mut seen = sample()?;
    let mut best = seen;
    let mut waited = Duration::ZERO;
    let mut flat = Duration::ZERO;
    while seen < target_mb && waited < timeout && flat < plateau {
        sleep(poll);
        waited += poll;
        seen = sample()?;
        if seen > best {
            best = seen;
            flat = Duration::ZERO;
        } else {
            flat += poll;
        }
    }
    Some(seen)
}

/// The final admission gate. Ends with `Some(seen)` once `target_mb` is free,
/// or when free memory has stopped rising for `plateau` AND no other job is
/// running (nothing more can be coming: waiting longer only stalls the job
/// behind memory other processes hold). While another job runs or unloads its
/// memory may still be handed back, so the wait has no elapsed-time limit; each
/// reading is reported through `note` as "waiting for GPU memory". `Err` only
/// for the client's cancel. `None` = no NVML on this machine.
pub fn wait_for_admission(
    target_mb: u64,
    others_active: impl FnMut() -> bool,
    cancel: &crate::backend::CancelToken,
    note: &mut dyn FnMut(&str),
) -> Result<Option<u64>, AssetAiError> {
    wait_for_admission_with(
        target_mb,
        ADMIT_PLATEAU,
        POLL_INTERVAL,
        fresh_free_mb,
        std::thread::sleep,
        others_active,
        || cancel.is_cancelled(),
        note,
    )
}

#[allow(clippy::too_many_arguments)]
fn wait_for_admission_with(
    target_mb: u64,
    plateau: Duration,
    poll: Duration,
    mut sample: impl FnMut() -> Option<u64>,
    mut sleep: impl FnMut(Duration),
    mut others_active: impl FnMut() -> bool,
    mut cancelled: impl FnMut() -> bool,
    note: &mut dyn FnMut(&str),
) -> Result<Option<u64>, AssetAiError> {
    let Some(mut seen) = sample() else { return Ok(None) };
    let mut best = seen;
    let mut flat = Duration::ZERO;
    let mut reported = None;
    while seen < target_mb {
        if cancelled() {
            return Err(AssetAiError::Cancelled);
        }
        if flat >= plateau && !others_active() {
            break;
        }
        if reported != Some(seen) {
            reported = Some(seen);
            note(&format!("waiting for GPU memory: need {target_mb} MB, free {seen} MB"));
        }
        sleep(poll);
        let Some(next) = sample() else { return Ok(None) };
        seen = next;
        if seen > best {
            best = seen;
            flat = Duration::ZERO;
        } else {
            flat += poll;
        }
    }
    Ok(Some(seen))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Domain;

    #[test]
    fn a_hold_lasts_until_released_or_its_time_runs_out() {
        let holds = Holds::default();
        let now = Instant::now();
        assert!(holds.hold_at("flux", "party", Duration::from_secs(120), now), "a new hold");
        assert!(!holds.hold_at("flux", "party", Duration::from_secs(120), now), "the same holder renews");
        assert!(holds.hold_at("flux", "stage", Duration::from_secs(30), now), "a second holder");
        assert_eq!(holds.holders_at("flux", now), vec!["party".to_string(), "stage".to_string()]);
        assert_eq!(holds.holders_at("flux", now + Duration::from_secs(60)), vec!["party".to_string()], "a hold not renewed runs out");
        assert!(holds.holders_at("flux", now + Duration::from_secs(121)).is_empty(), "and so does the other");
        assert!(holds.holders_at("whisper", now).is_empty(), "nobody holds another model");
        holds.hold_at("flux", "party", Duration::from_secs(3600), now);
        assert!(holds.holders_at("flux", now + MAX_HOLD + Duration::from_secs(1)).is_empty(), "no hold outlasts MAX_HOLD without a renewal");
        holds.hold("flux", "party", Duration::from_secs(120));
        assert!(holds.release("flux", "party"), "a release says it held");
        assert!(!holds.release("flux", "party"), "a second release has nothing to let go");
        assert_eq!(holds.holders("flux"), vec!["stage".to_string()], "the other holder keeps it");
        assert!(holds.release("flux", "stage"));
        assert!(!holds.held("flux"));
    }

    #[test]
    fn an_admission_wait_ends_when_free_memory_stops_rising() {
        // 100 MB free for ever, 5000 wanted: ends after the plateau, not the timeout.
        let mut sleeps = 0u32;
        let seen = wait_free_settled_with(
            5000,
            Duration::from_secs(20),
            Duration::from_millis(1500),
            Duration::from_millis(250),
            || Some(100),
            |_| sleeps += 1,
        );
        assert_eq!(seen, Some(100));
        assert_eq!(sleeps, 6, "1.5 s of flat readings, not 20 s");
    }

    #[test]
    fn a_rising_free_keeps_the_wait_open_until_the_target() {
        let mut readings = vec![100u64, 300, 900, 2000, 4000, 5200].into_iter();
        let seen = wait_free_settled_with(
            5000,
            Duration::from_secs(20),
            Duration::from_millis(1500),
            Duration::from_millis(250),
            || readings.next(),
            |_| {},
        );
        assert_eq!(seen, Some(5200));
    }

    fn admit(
        target: u64,
        mut readings: impl FnMut() -> Option<u64>,
        mut others: impl FnMut() -> bool,
        mut cancelled: impl FnMut() -> bool,
    ) -> (Result<Option<u64>, AssetAiError>, u32, Vec<String>) {
        let (mut sleeps, mut notes) = (0u32, Vec::new());
        let result = wait_for_admission_with(
            target,
            Duration::from_millis(1500),
            Duration::from_millis(250),
            &mut readings,
            |_| sleeps += 1,
            &mut others,
            &mut cancelled,
            &mut |text: &str| notes.push(text.to_string()),
        );
        (result, sleeps, notes)
    }

    #[test]
    fn admission_waits_without_a_time_limit_while_another_job_runs() {
        // Flat 100 MB for 100,000 polls (7 hours of 250 ms) with a job running:
        // never given up on, never an elapsed-time failure; then the job unloads.
        let mut polls = 0u64;
        let (result, sleeps, notes) = admit(
            5000,
            || {
                polls += 1;
                Some(if polls > 100_000 { 6000 } else { 100 })
            },
            || true,
            || false,
        );
        assert_eq!(result.unwrap(), Some(6000));
        assert_eq!(sleeps, 100_000);
        assert_eq!(notes, ["waiting for GPU memory: need 5000 MB, free 100 MB"], "reported once per reading");
    }

    #[test]
    fn admission_gives_up_only_when_nothing_is_running_and_free_is_flat() {
        let (result, sleeps, _) = admit(5000, || Some(100), || false, || false);
        assert_eq!(result.unwrap(), Some(100));
        assert_eq!(sleeps, 6, "the plateau, as before");
    }

    #[test]
    fn admission_wait_ends_on_the_clients_cancel_and_without_nvml() {
        let mut polls = 0;
        let (result, _, _) = admit(5000, || Some(100), || true, || {
            polls += 1;
            polls > 3
        });
        assert!(matches!(result, Err(AssetAiError::Cancelled)));
        let (result, _, _) = admit(5000, || None, || true, || false);
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn the_timeout_still_bounds_a_slow_climb() {
        let mut n = 0u64;
        let seen = wait_free_settled_with(
            u64::MAX,
            Duration::from_secs(1),
            Duration::from_secs(10),
            Duration::from_millis(250),
            || {
                n += 1;
                Some(n)
            },
            |_| {},
        );
        assert_eq!(seen, Some(5));
    }
    use std::cell::Cell;

    fn spec(vram_gb: Option<f64>) -> ModelSpec {
        ModelSpec {
            id: "m".into(),
            domain: Domain::Image,
            backend: "testpattern".into(),
            available: true,
            gated: false,
            vram_gb,
            min_vram_gb: None,
            min_compute_cap: None,
            note: None,
            license: None,
            files: Vec::new(),
        }
    }

    #[test]
    fn estimates_follow_registry_and_treat_none_as_free() {
        assert_eq!(estimated_peak_mb(&spec(None)), 0);
        assert_eq!(estimated_peak_mb(&spec(Some(0.0))), 0);
        assert_eq!(estimated_peak_mb(&spec(Some(-1.0))), 0);
        assert_eq!(estimated_peak_mb(&spec(Some(f64::NAN))), 0);
        assert_eq!(estimated_peak_mb(&spec(Some(24.0))), 24 * 1024);
        assert_eq!(estimated_peak_mb(&spec(Some(0.5))), 512);
        // Rounds UP: a 1.1 GB estimate must not admit at 1.0 GB free.
        assert_eq!(estimated_peak_mb(&spec(Some(1.1))), 1127);
    }

    #[test]
    fn usable_vram_is_the_idle_free_ceiling_and_drives_too_small_notes() {
        let gpu = crate::gpu::GpuInfo {
            vram_free_mb: Some(30_000),
            vram_total_mb: Some(32_000),
            unified: false,
            ..Default::default()
        };
        assert_eq!(usable_vram_mb(&gpu), Some(30_000));
        assert_eq!(
            too_small_note(&spec(Some(29.0)), DEFAULT_RESERVE_MB, 30_000).as_deref(),
            Some("needs 31744 MB, this card can free 30000")
        );
        assert!(too_small_note(&spec(Some(29.0)), DEFAULT_RESERVE_MB, 36_000).is_none());
    }

    fn trim_fake_pool(total_mb: u64, free_mb: u64, cached_mb: u64) -> CachedPoolTrim {
        let free = Cell::new(free_mb);
        let cached = Cell::new(cached_mb);
        trim_cached_pool_with(
            || crate::gpu::GpuInfo {
                vram_free_mb: Some(free.get()),
                vram_total_mb: Some(total_mb),
                unified: false,
                ..Default::default()
            },
            || {
                free.set(free.get().saturating_add(cached.replace(0)).min(total_mb));
            },
        )
    }

    #[test]
    fn incident_card_admits_after_releasing_cached_pool() {
        let trimmed = trim_fake_pool(32_000, 30_500, 1_000);
        let (free, admitted) = admission_after_trim(&trimmed, 30_500, 29_696 + 1_024);
        assert_eq!(free, 31_500);
        assert_eq!(trimmed.released_mb_at(31_500), 1_000);
        assert!(admitted);
    }

    #[test]
    fn card_still_short_after_pool_trim_refuses_with_release_amount() {
        let trimmed = trim_fake_pool(32_000, 29_000, 1_000);
        let (free, admitted) = admission_after_trim(&trimmed, 29_000, 29_696 + 1_024);
        assert!(!admitted);
        assert_eq!(
            insufficient_vram_message(
                "flux2-dev",
                29_696,
                1_024,
                free,
                trimmed.released_mb_at(free),
            ),
            "insufficient VRAM for flux2-dev: need 30720 MB (estimate 29696 MB + reserve 1024 MB), only 30000 MB free after evicting every resident and releasing 1000 MB of cached pool — refusing to load (no CPU/other-node fallback)"
        );
    }

    #[test]
    fn usable_vram_is_published_from_the_post_trim_sample() {
        let usable = UsableVram::new(Some(30_500));
        let trimmed = trim_fake_pool(32_000, 30_500, 1_000);
        usable.refresh_from_gpu(&trimmed.after_gpu);
        assert_eq!(usable.get(), Some(31_500));
    }

    #[test]
    fn config_parses_env_shapes_and_survives_garbage() {
        let config = ResidencyConfig::from_values(Some("4096"), Some("kokoro, sa3-sfx ,"), Some("600"));
        assert_eq!(config.reserve_mb, 4096);
        assert!(config.pins.contains("kokoro"));
        assert!(config.pins.contains("sa3-sfx"));
        assert_eq!(config.pins.len(), 2);
        assert_eq!(config.idle_evict, Some(Duration::from_secs(600)));

        let config = ResidencyConfig::from_values(Some("nope"), None, Some("0"));
        assert_eq!(config.reserve_mb, DEFAULT_RESERVE_MB);
        assert!(config.pins.is_empty());
        assert_eq!(config.idle_evict, None);
    }

    #[test]
    fn oom_classification_is_substring_based_and_conservative() {
        let oom = |m: &str| error_is_oom(&AssetAiError::Backend(m.to_string()));
        assert!(oom("CUDA error: out of memory (denoise step 3)"));
        assert!(oom("cudaErrorMemoryAllocation while uploading unet"));
        assert!(oom("CUBLAS_STATUS_ALLOC_FAILED"));
        assert!(oom("gpu pool alloc failed: 2048 MB"));
        assert!(!oom("shape mismatch: expected 64x64"));
        assert!(!error_is_oom(&AssetAiError::Cancelled));
        assert!(!error_is_oom(&AssetAiError::Busy));
        // Params errors are caller bugs, never retried as OOM.
        assert!(!error_is_oom(&AssetAiError::Params(
            "out of memory".to_string()
        )));
    }

}
