//! Main-thread frame monitor feeding the PerfGraph widget (widgets crate).
//!
//! A fixed ring of per-frame samples: the paint-to-paint gap (frame pacing —
//! spikes here are the hiccups the user sees) plus per-channel CPU times.
//! Built-in channels cover the platform (event dispatch, script GC, pass
//! encode, drawable wait); apps register their own channels for anything
//! else they want plotted (physics, script tick, audio…):
//!
//!     let ch = cx.perf_monitor_channel("physics", 0x6aa9ff);
//!     ...
//!     cx.perf_monitor_add(ch, t0.elapsed().as_micros() as u64);
//!
//! Collection is off until something (normally the PerfGraph widget) calls
//! `set_enabled(true)`; disabled adds are a single branch.

pub const PERF_MONITOR_HISTORY: usize = 240;
pub const PERF_MONITOR_MAX_CHANNELS: usize = 12;

/// GPU completion handlers run off-thread; they park each presented frame's
/// GPU time here and the next `frame_boundary` folds it into the ring.
static GPU_ACCUM_US: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static GPU_COLLECT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Call from a GPU command-buffer completion handler (any thread) with one
/// presented frame's GPU interval in seconds.
pub fn perf_gpu_frame_completed(seconds: f64) {
    use std::sync::atomic::Ordering;
    if GPU_COLLECT.load(Ordering::Relaxed) && seconds.is_finite() && seconds > 0.0 {
        GPU_ACCUM_US.fetch_add((seconds * 1e6) as u32, Ordering::Relaxed);
    }
}

/// Index of a registered channel; hand out once, add to it every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerfChannel(pub usize);

/// Built-in channels, registered in `PerfMonitor::default()`.
pub const PERF_CHANNEL_EVENT: PerfChannel = PerfChannel(0);
/// Splash/script VM execution — embedders add their eval/call time here.
pub const PERF_CHANNEL_SCRIPT: PerfChannel = PerfChannel(1);
pub const PERF_CHANNEL_GC: PerfChannel = PerfChannel(2);
pub const PERF_CHANNEL_DRAW: PerfChannel = PerfChannel(3);
pub const PERF_CHANNEL_DRAWABLE_WAIT: PerfChannel = PerfChannel(4);
/// GPU time of a presented frame (command-buffer start→end, completion
/// handler thread). Runs CONCURRENT with the CPU channels — read it as its
/// own series, not as part of the main-thread total.
pub const PERF_CHANNEL_GPU: PerfChannel = PerfChannel(5);

#[derive(Clone, Copy, Default)]
pub struct PerfMonitorFrame {
    /// Time between this paint and the previous one, milliseconds.
    pub gap_ms: f32,
    /// Per-channel CPU time this frame, microseconds.
    pub channel_us: [u32; PERF_MONITOR_MAX_CHANNELS],
}

#[derive(Clone)]
pub struct PerfChannelInfo {
    pub name: String,
    /// 0xRRGGBB plot color hint.
    pub color: u32,
}

pub struct PerfMonitor {
    enabled: bool,
    channels: Vec<PerfChannelInfo>,
    ring: Vec<PerfMonitorFrame>,
    at: usize,
    cur: PerfMonitorFrame,
    last_frame_time: Option<f64>,
    /// inner_call_event_handler recurses (Paint inside Timer); only the
    /// outermost dispatch is timed.
    pub(crate) event_depth: u32,
    /// Time app channels attributed while inside an event dispatch; deducted
    /// from the "event" channel so the stacked plot doesn't double-count.
    event_deduct: u32,
}

impl Default for PerfMonitor {
    fn default() -> Self {
        Self {
            enabled: false,
            channels: vec![
                PerfChannelInfo { name: "event".into(), color: 0x4fd06a },
                PerfChannelInfo { name: "script".into(), color: 0x58b6ff },
                PerfChannelInfo { name: "gc".into(), color: 0xd0c24f },
                PerfChannelInfo { name: "draw".into(), color: 0xff9a4f },
                PerfChannelInfo { name: "wait".into(), color: 0xe05555 },
                PerfChannelInfo { name: "gpu".into(), color: 0xb08cff },
            ],
            ring: Vec::new(),
            at: 0,
            cur: Default::default(),
            last_frame_time: None,
            event_depth: 0,
            event_deduct: 0,
        }
    }
}

impl PerfMonitor {
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        GPU_COLLECT.store(on, std::sync::atomic::Ordering::Relaxed);
        if !on {
            self.last_frame_time = None;
            self.cur = Default::default();
            self.event_deduct = 0;
            GPU_ACCUM_US.store(0, std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Register (or find by name) an app channel. Indexes are stable for the
    /// life of the process; past MAX_CHANNELS you share the last slot.
    pub fn channel(&mut self, name: &str, color: u32) -> PerfChannel {
        if let Some(index) = self.channels.iter().position(|c| c.name == name) {
            return PerfChannel(index);
        }
        if self.channels.len() >= PERF_MONITOR_MAX_CHANNELS {
            return PerfChannel(PERF_MONITOR_MAX_CHANNELS - 1);
        }
        self.channels.push(PerfChannelInfo { name: name.into(), color });
        PerfChannel(self.channels.len() - 1)
    }

    pub fn channels(&self) -> &[PerfChannelInfo] {
        &self.channels
    }

    pub fn add(&mut self, channel: PerfChannel, us: u64) {
        if !self.enabled {
            return;
        }
        let mut us = us as u32;
        if channel == PERF_CHANNEL_EVENT {
            // "event" is what's left of the dispatch after the app attributed
            // its own channels (script, physics, …) inside it.
            us = us.saturating_sub(self.event_deduct);
            self.event_deduct = 0;
        } else if self.event_depth > 0 {
            self.event_deduct = self.event_deduct.saturating_add(us);
        }
        let slot = &mut self.cur.channel_us[channel.0.min(PERF_MONITOR_MAX_CHANNELS - 1)];
        *slot = slot.saturating_add(us);
    }

    /// Close the frame being accumulated and start the next. Called by the
    /// platform at the start of every window repaint.
    pub fn frame_boundary(&mut self, time: f64) {
        if !self.enabled {
            return;
        }
        if self.ring.is_empty() {
            self.ring.resize(PERF_MONITOR_HISTORY, Default::default());
        }
        // Fold in GPU time completed since the last boundary (one frame late
        // by construction — fine for a monitor).
        let gpu_us = GPU_ACCUM_US.swap(0, std::sync::atomic::Ordering::Relaxed);
        if gpu_us > 0 {
            let slot = &mut self.cur.channel_us[PERF_CHANNEL_GPU.0];
            *slot = slot.saturating_add(gpu_us);
        }
        if let Some(last) = self.last_frame_time {
            self.cur.gap_ms = ((time - last) * 1000.0) as f32;
            self.ring[self.at] = self.cur;
            self.at = (self.at + 1) % PERF_MONITOR_HISTORY;
        }
        self.last_frame_time = Some(time);
        self.cur = Default::default();
    }

    /// Copy the history oldest→newest. Empty until enabled + first frames.
    pub fn read(&self, out: &mut Vec<PerfMonitorFrame>) {
        out.clear();
        if self.ring.is_empty() {
            return;
        }
        for i in 0..PERF_MONITOR_HISTORY {
            out.push(self.ring[(self.at + i) % PERF_MONITOR_HISTORY]);
        }
    }
}
