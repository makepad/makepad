//! GPU timing and watchdog: per-pass timestamps in a shared command buffer (moved from
//! os/apple/metal.rs `PassClock` / `PassSamples` / `PassTime`), the hang diagnostic and the
//! command-buffer error report.

use makepad_apple_sys::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

extern "C" {
    static MTLCommonCounterSetTimestamp: ObjcId;
}

/// Per-pass GPU time inside a shared command buffer: a timestamp where the pass's render encoder
/// starts and one where it ends, in a counter sample buffer (stage boundaries on Apple GPUs,
/// encoder samples elsewhere). A device with neither times each frame in a buffer of its own.
pub struct PassClock {
    device: RcObjcId,
    descriptor: RcObjcId,
    pub stage: bool,
    pool: Mutex<Vec<RcObjcId>>,
    /// (host seconds, GPU timestamp) sampled together at creation: with a second pair at resolve
    /// time, the GPU clock's rate and offset.
    anchor: (f64, u64),
}

/// hardware: timestamps in one command buffer's sample buffer; a batch with more timed passes
/// starts another command buffer.
pub const PASS_CLOCK_SAMPLES: usize = 1024;
/// Timestamps per pass: vertex start and end, fragment start and end.
pub const PASS_CLOCK_STRIDE: usize = 4;

impl PassClock {
    pub fn new(device: ObjcId) -> Option<Arc<Self>> {
        unsafe {
            let available: bool = msg_send![device, respondsToSelector: sel!(supportsCounterSampling:)];
            if !available {
                return None;
            }
            // MTLCounterSamplingPointAtStageBoundary = 0, AtDrawBoundary = 1
            let stage: bool = msg_send![device, supportsCounterSampling: 0u64];
            let draw: bool = msg_send![device, supportsCounterSampling: 1u64];
            if !stage && !draw {
                return None;
            }
            let sets: ObjcId = msg_send![device, counterSets];
            let count: u64 = if sets == nil { 0 } else { msg_send![sets, count] };
            #[cfg(not(target_arch = "wasm32"))]
            {
                let name: ObjcId = msg_send![device, name];
                let counter_sets = (0..count)
                    .map(|i| -> String {
                        let set: ObjcId = msg_send![sets, objectAtIndex: i];
                        let n: ObjcId = msg_send![set, name];
                        let counters: ObjcId = msg_send![set, counters];
                        let k: u64 = if counters == nil { 0 } else { msg_send![counters, count] };
                        format!("{} ({k} counters)", nsstring_to_string(n))
                    })
                    .collect();
                let points = ["stage boundary", "draw boundary", "dispatch boundary", "tile dispatch boundary", "blit boundary"];
                let sampling = points.iter().enumerate().filter(|(i, _)| { let ok: bool = msg_send![device, supportsCounterSampling: *i as u64]; ok }).map(|(_, p)| p.to_string()).collect();
                let mut notes = Vec::new();
                if !stage {
                    notes.push("no stage-boundary sampling: render passes are timed as whole encoders (no vertex / fragment lanes)".to_string());
                }
                crate::gpu_prof::set_device(crate::gpu_prof::DeviceInfo { name: nsstring_to_string(name), counter_sets, sampling, notes });
            }
            let set = (0..count)
                .map(|i| -> ObjcId { msg_send![sets, objectAtIndex: i] })
                .find(|set| {
                    let name: ObjcId = msg_send![*set, name];
                    msg_send![name, isEqualToString: MTLCommonCounterSetTimestamp]
                })?;
            let descriptor = RcObjcId::from_owned(NonNull::new(msg_send![class!(MTLCounterSampleBufferDescriptor), new])?);
            let () = msg_send![descriptor.as_id(), setCounterSet: set];
            let () = msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Shared];
            let () = msg_send![descriptor.as_id(), setSampleCount: PASS_CLOCK_SAMPLES as u64];
            Some(Arc::new(Self {
                device: RcObjcId::from_unowned(NonNull::new(device)?),
                descriptor,
                stage,
                pool: Mutex::new(Vec::new()),
                anchor: Self::now(device),
            }))
        }
    }

    fn now(device: ObjcId) -> (f64, u64) {
        let (mut cpu, mut gpu) = (0u64, 0u64);
        unsafe {
            let () = msg_send![device, sampleTimestamps: &mut cpu gpuTimestamp: &mut gpu];
            (CACurrentMediaTime(), gpu)
        }
    }

    pub fn take(&self) -> Option<RcObjcId> {
        if let Some(buffer) = self.pool.lock().ok().and_then(|mut pool| pool.pop()) {
            return Some(buffer);
        }
        let mut error: ObjcId = nil;
        let buffer: ObjcId = unsafe { msg_send![self.device.as_id(), newCounterSampleBufferWithDescriptor: self.descriptor.as_id() error: &mut error] };
        NonNull::new(buffer).map(RcObjcId::from_owned)
    }
}

/// One command buffer's timestamps. Its passes' completion handlers read them (resolved once, by
/// the first); the sample buffer goes back to the pool when the last handler lets go.
pub struct PassSamples {
    pub clock: Arc<PassClock>,
    pub buffer: Option<RcObjcId>,
    /// Samples reserved (backend thread only; atomic to share with handlers).
    used: AtomicUsize,
    /// per sample group: its four times in host seconds (`None`: not sampled)
    raw: OnceLock<Vec<[Option<f64>; 4]>>,
}

impl PassSamples {
    pub fn new(clock: Arc<PassClock>) -> Option<Arc<PassSamples>> {
        let buffer = clock.take()?;
        Some(Arc::new(PassSamples { clock, buffer: Some(buffer), used: AtomicUsize::new(0), raw: OnceLock::new() }))
    }

    pub fn reserve(&self) -> Option<usize> {
        let at = self.used.load(Ordering::Relaxed);
        (at + PASS_CLOCK_STRIDE <= PASS_CLOCK_SAMPLES).then(|| {
            self.used.store(at + PASS_CLOCK_STRIDE, Ordering::Relaxed);
            at
        })
    }

    /// How many more passes fit.
    pub fn reserve_room(&self) -> usize {
        (PASS_CLOCK_SAMPLES - self.used.load(Ordering::Relaxed)) / PASS_CLOCK_STRIDE
    }

    pub fn full(&self) -> bool {
        self.used.load(Ordering::Relaxed) + PASS_CLOCK_STRIDE > PASS_CLOCK_SAMPLES
    }

    /// The four sampled times of the group at `index`: vertex start / end and fragment start /
    /// end of a render pass on stage-boundary devices; encoder start and end (first and last)
    /// otherwise, and for blit and compute encoders (first two).
    pub fn raw(&self, command_buffer: ObjcId, index: usize) -> [Option<f64>; 4] {
        self.raw.get_or_init(|| self.resolve(command_buffer)).get(index / PASS_CLOCK_STRIDE).copied().unwrap_or([None; 4])
    }

    /// A render pass's own (start, end) in host seconds, the clock of the buffer's own
    /// `GPUStartTime`; `None` for a pass whose fragment stage left no time inside the buffer's span.
    pub fn span(&self, command_buffer: ObjcId, index: usize) -> Option<(f64, f64)> {
        let [v0, v1, f0, f1] = self.raw(command_buffer, index);
        if !self.clock.stage {
            // encoder samples: first and last
            return v0.zip(f1).filter(|(a, b)| b >= a);
        }
        // Apple GPUs run a pass's vertex stage early, alongside the fragment work of
        // passes before it (its tiles wait for them): start..end would count that wait.
        // The pass's own time is its vertex stage plus its fragment stage, ending where
        // its fragment stage ends. A pass that shades no pixel has its vertex stage alone.
        let vertex = v0.zip(v1).filter(|(a, b)| b >= a);
        match f0.zip(f1).filter(|(a, b)| b >= a) {
            Some((f0, f1)) => Some((f0 - vertex.map_or(0.0, |(a, b)| b - a), f1)),
            None => vertex,
        }
    }

    /// Every sample group's times in host seconds; a time outside the buffer's own span is a
    /// stage that never ran.
    fn resolve(&self, command_buffer: ObjcId) -> Vec<[Option<f64>; 4]> {
        let used = self.used.load(Ordering::Acquire);
        let Some(buffer) = &self.buffer else { return Vec::new() };
        let (host0, gpu0) = self.clock.anchor;
        let (host1, gpu1) = PassClock::now(self.clock.device.as_id());
        if gpu1 <= gpu0 || host1 <= host0 {
            return Vec::new();
        }
        let rate = (host1 - host0) / (gpu1 - gpu0) as f64;
        let (start, end): (f64, f64) = unsafe { (msg_send![command_buffer, GPUStartTime], msg_send![command_buffer, GPUEndTime]) };
        // the buffer's own span, widened by the calibration's error
        let (lo, hi) = (start - 1e-4, end + 1e-4);
        let data: ObjcId = unsafe { msg_send![buffer.as_id(), resolveCounterRange: NSRange { location: 0, length: used as _ }] };
        if data == nil {
            return Vec::new();
        }
        let bytes: u64 = unsafe { msg_send![data, length] };
        let ptr: *const u64 = unsafe { msg_send![data, bytes] };
        let count = if ptr.is_null() { 0 } else { (bytes as usize / 8).min(used) };
        // A stage that never ran leaves its slot as it was: zero, the "not sampled" marker, or a
        // stale time from an earlier buffer.
        let host = |tick: u64| {
            let t = host1 - (gpu1 as f64 - tick as f64) * rate;
            (tick != 0 && tick != u64::MAX && t >= lo && t <= hi).then_some(t)
        };
        (0..count / PASS_CLOCK_STRIDE)
            .map(|group| std::array::from_fn(|i| host(unsafe { *ptr.add(group * PASS_CLOCK_STRIDE + i) })))
            .collect()
    }
}

impl Drop for PassSamples {
    fn drop(&mut self) {
        if let (Some(buffer), Ok(mut pool)) = (self.buffer.take(), self.clock.pool.lock()) {
            pool.push(buffer);
        }
    }
}

/// Where a frame's GPU start and end come from.
#[derive(Clone)]
pub enum PassTime {
    /// The frame has its command buffer to itself.
    Buffer,
    /// Its passes' timestamps in the buffer's samples (start index of each).
    Counters(Arc<PassSamples>, Vec<usize>),
}

impl PassTime {
    /// The frame's GPU busy time and its (start, end) in host seconds; `None` when the
    /// timestamps did not resolve.
    pub fn span(&self, command_buffer: ObjcId) -> Option<(f64, f64, f64)> {
        match self {
            Self::Buffer => {
                let start: f64 = unsafe { msg_send![command_buffer, GPUStartTime] };
                let end: f64 = unsafe { msg_send![command_buffer, GPUEndTime] };
                Some((end - start, start, end))
            }
            Self::Counters(samples, indices) => {
                let (mut busy, mut first, mut last, mut any) = (0.0, f64::MAX, 0.0f64, false);
                for index in indices {
                    if let Some((a, b)) = samples.span(command_buffer, *index) {
                        busy += b - a;
                        first = first.min(a);
                        last = last.max(b);
                        any = true;
                    }
                }
                any.then_some((busy, first, last))
            }
        }
    }
}

// ------------------------------------------------------------------ in-flight registry, watchdog

/// One in-flight command buffer as the hang watchdog sees it. Entries are born when the buffer
/// is created, stamped at commit, and removed by the buffer's completion handler.
pub struct InFlightCb {
    pub seq: u64,
    pub committed_at: Option<Instant>,
    /// the passes in the buffer: name and the shaders' names
    pub passes: Vec<(String, Vec<String>)>,
}

/// The command buffers committed and not completed: pushed by the submitter thread, removed by
/// each buffer's completion handler, read by the watchdog.
pub type InFlightQueue = Arc<Mutex<VecDeque<InFlightCb>>>;

pub fn lock_in_flight(queue: &InFlightQueue) -> std::sync::MutexGuard<'_, VecDeque<InFlightCb>> {
    queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn describe_passes(passes: &[(String, Vec<String>)]) -> String {
    if passes.is_empty() {
        return "(no draw pass recorded)".to_string();
    }
    let mut out = String::new();
    for (name, shaders) in passes {
        out.push_str(&format!("\"{}\" shaders [{}]; ", if name.is_empty() { "main" } else { name }, shaders.join(", ")));
    }
    out
}

/// diagnostic: a command buffer older than this without completing is reported by the hang
/// watchdog (never acted on: the watchdog is a diagnostic, not a policy). Five seconds is far
/// beyond any frame and short of the OS GPU watchdog that resets the driver.
const GPU_HANG_DIAGNOSTIC_MS: u64 = 5000;

/// GPU-HANG DIAGNOSTIC. A runaway shader keeps its command buffer from ever completing; the
/// OS's GPU watchdog then resets the DRIVER (the user's whole desktop). This thread watches the
/// oldest committed-but-uncompleted command buffer and, once it is older than the threshold,
/// writes a diagnostic naming the passes and shaders in that buffer, then re-checks after a
/// pause instead of re-reporting every tick. It never aborts: a customer-facing app must not
/// quit itself on a stall it did not cause. A thread, not a per-frame check: a hung GPU also
/// stalls the display link, so the main loop may never get another beat.
pub fn spawn_hang_watchdog(in_flight: InFlightQueue, completed: Arc<AtomicU64>, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("makepad-metal-hang-watchdog".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(100));
            if stop.load(Ordering::Acquire) {
                return;
            }
            let diagnostic = {
                let mut queue = lock_in_flight(&in_flight);
                while queue.front().is_some_and(|entry| entry.seq <= completed.load(Ordering::Acquire)) {
                    queue.pop_front();
                }
                let Some(oldest) = queue.iter().find(|entry| entry.committed_at.is_some()) else { continue };
                let age = oldest.committed_at.unwrap().elapsed();
                if age.as_millis() as u64 <= GPU_HANG_DIAGNOSTIC_MS {
                    continue;
                }
                format!(
                    "[metal-hang] command buffer #{} committed {} ms ago has not completed (diagnostic threshold {} ms, {} buffers in flight). A runaway shader in one of its passes, or the GPU starved by another process. Passes: {}",
                    oldest.seq,
                    age.as_millis(),
                    GPU_HANG_DIAGNOSTIC_MS,
                    queue.len(),
                    describe_passes(&oldest.passes),
                )
            };
            eprintln!("{diagnostic}");
            // Re-check after a pause instead of re-reporting every tick.
            for _ in 0..50 {
                if stop.load(Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
        .expect("Metal hang watchdog")
}

/// `MTLCommandBufferStatusError`: the buffer's GPU work did not run.
const MTL_COMMAND_BUFFER_STATUS_ERROR: u64 = 5;

/// Command buffers that ended in error since launch. Bounds the log below.
static CB_ERRORS: AtomicU64 = AtomicU64::new(0);

/// A command buffer that ends in `MTLCommandBufferStatusError` produced no pixels: whatever it
/// was going to draw is missing from the frame, and nothing else in the backend notices, because
/// Metal runs the completion handler either way, so the completion frontier advances and the
/// watchdog stays quiet over a stale or black window. Names the failure and the passes that were
/// in the buffer. Returns whether the buffer failed.
pub fn report_command_buffer_error(cb: ObjcId, seq: u64, entry: Option<&InFlightCb>) -> bool {
    let status: u64 = unsafe { msg_send![cb, status] };
    if status != MTL_COMMAND_BUFFER_STATUS_ERROR {
        return false;
    }
    let count = CB_ERRORS.fetch_add(1, Ordering::Relaxed);
    if count >= 32 {
        return true;
    }
    let error: ObjcId = unsafe { msg_send![cb, error] };
    let (code, description) = if error != nil {
        let code: i64 = unsafe { msg_send![error, code] };
        let desc: ObjcId = unsafe { msg_send![error, localizedDescription] };
        (code, nsstring_to_string(desc))
    } else {
        (0, "no NSError attached".to_string())
    };
    let passes = entry.map(|entry| describe_passes(&entry.passes)).unwrap_or_else(|| "(no in-flight entry)".to_string());
    eprintln!(
        "[metal] command buffer #{seq} failed with MTLCommandBufferError {code}: {description}. Its frame did not render. Passes: {passes}{}",
        if count == 31 { " (further command-buffer errors suppressed)" } else { "" },
    );
    true
}
