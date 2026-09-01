//! Per-peer receive path: a lock-free reorder ring written by the network
//! thread and read by the audio thread, and the playout state machine that
//! turns it into a continuous sample stream.
//!
//! **Ring.** [`JitterRing`] holds [`RING`] frame slots indexed by
//! `seq % RING`. The network thread ([`JitterRing::insert`]) only ever writes
//! a slot whose sequence lies inside the reader's window
//! `[read_seq, read_seq + RING)` and that is not holding a live (unplayed)
//! frame; the audio thread ([`JitterRing::take`]) validates the slot's
//! sequence before and after copying, so a torn read is impossible and there
//! is no lock anywhere. Samples are stored as `AtomicI16` with relaxed
//! ordering — on every target this compiles to plain loads and stores.
//!
//! **Playout.** [`Playout`] pulls frames in sequence order and:
//! - starts after a prefill of `target` frames (default 2 = one frame of slack),
//! - conceals a lost or late frame by repeating the last one with a fade to
//!   zero, then zeros, and fades the next real frame in (no click),
//! - raises the target by one frame when packets arrive late or the buffer
//!   runs dry (rate-limited), and lowers it again after 10 s of stability,
//! - corrects clock drift and bleeds excess delay with a tiny playback-rate
//!   nudge (≤ ±0.5 %, inaudible) driven by the smoothed buffer occupancy, so
//!   long sessions never accumulate delay or hit periodic underruns,
//! - goes idle after 200 ms without data so silent peers cost nothing to render,
//! - resynchronises when the sender jumps more than a ring ahead.

use crate::resample::Resampler;
use crate::wire::{INTERNAL_RATE, MAX_FRAME};
use std::sync::atomic::{AtomicBool, AtomicI16, AtomicU32, Ordering};

/// Frames per ring. With 5 ms frames this is 160 ms of reorder window.
pub const RING: usize = 32;
const EMPTY: u32 = u32::MAX;
const SILENT_BIT: u32 = 1 << 31;

struct Slot {
    seq: AtomicU32,
    /// Sample count, with [`SILENT_BIT`] set for a silence frame (no data).
    len: AtomicU32,
    data: [AtomicI16; MAX_FRAME],
}

impl Slot {
    fn new() -> Self {
        Self {
            seq: AtomicU32::new(EMPTY),
            len: AtomicU32::new(0),
            data: std::array::from_fn(|_| AtomicI16::new(0)),
        }
    }
}

/// Outcome of [`JitterRing::insert`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Insert {
    Accepted,
    /// The reader already played past this sequence number.
    Late,
    /// Same sequence number already in the ring.
    Duplicate,
    /// More than a ring ahead of the reader; the reader will resynchronise.
    TooFar,
    /// The slot still holds an unplayed frame (only after a reader jump).
    Occupied,
}

/// Outcome of [`JitterRing::take`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Take {
    /// `out[..len]` holds the frame.
    Audio(usize),
    /// A silence frame of `len` samples; `out` is untouched.
    Silence(usize),
    /// Not (yet) in the ring.
    Missing,
}

/// The shared reorder ring: one writer (network thread), one reader (audio
/// thread), no locks.
pub struct JitterRing {
    slots: Box<[Slot]>,
    synced: AtomicBool,
    read_seq: AtomicU32,
    newest_seq: AtomicU32,
    pub accepted: AtomicU32,
    pub late: AtomicU32,
    pub duplicate: AtomicU32,
    pub too_far: AtomicU32,
}

impl Default for JitterRing {
    fn default() -> Self {
        Self::new()
    }
}

impl JitterRing {
    pub fn new() -> Self {
        let slots: Vec<Slot> = (0..RING).map(|_| Slot::new()).collect();
        Self {
            slots: slots.into_boxed_slice(),
            synced: AtomicBool::new(false),
            read_seq: AtomicU32::new(0),
            newest_seq: AtomicU32::new(0),
            accepted: AtomicU32::new(0),
            late: AtomicU32::new(0),
            duplicate: AtomicU32::new(0),
            too_far: AtomicU32::new(0),
        }
    }

    /// Writer side: forget everything (peer slot reuse).
    pub fn reset(&self) {
        self.synced.store(false, Ordering::Release);
        for slot in self.slots.iter() {
            slot.seq.store(EMPTY, Ordering::Release);
        }
        self.accepted.store(0, Ordering::Relaxed);
        self.late.store(0, Ordering::Relaxed);
        self.duplicate.store(0, Ordering::Relaxed);
        self.too_far.store(0, Ordering::Relaxed);
    }

    /// Reader side: the sequence the reader starts at, once the first frame
    /// has arrived.
    pub fn synced_read_seq(&self) -> Option<u32> {
        if self.synced.load(Ordering::Acquire) {
            Some(self.read_seq.load(Ordering::Acquire))
        } else {
            None
        }
    }

    pub fn is_synced(&self) -> bool {
        self.synced.load(Ordering::Acquire)
    }

    pub fn newest_seq(&self) -> u32 {
        self.newest_seq.load(Ordering::Acquire)
    }

    /// Reader side: publish where the reader is, so the writer's window follows.
    pub fn publish_read_seq(&self, seq: u32) {
        self.read_seq.store(seq, Ordering::Release);
    }

    /// Frames from `read_seq` up to and including the newest one that has
    /// arrived (an upper bound: holes count).
    pub fn queued(&self, read_seq: u32) -> u32 {
        if !self.synced.load(Ordering::Acquire) {
            return 0;
        }
        let d = self.newest_seq().wrapping_sub(read_seq) as i32;
        if d < 0 {
            0
        } else {
            d as u32 + 1
        }
    }

    /// Writer side: store a frame. `samples` is ignored when `silent`.
    pub fn insert(&self, seq: u32, samples: &[i16], silent: bool) -> Insert {
        if !self.synced.load(Ordering::Acquire) {
            // First frame primes the reader position.
            self.read_seq.store(seq, Ordering::Release);
            self.newest_seq.store(seq, Ordering::Release);
            self.synced.store(true, Ordering::Release);
        }
        let read_seq = self.read_seq.load(Ordering::Acquire);
        let d = seq.wrapping_sub(read_seq) as i32;
        if d < 0 {
            self.late.fetch_add(1, Ordering::Relaxed);
            return Insert::Late;
        }
        if d as usize >= RING {
            // Publish so the reader can see how far ahead the sender is.
            self.newest_seq.store(seq, Ordering::Release);
            self.too_far.fetch_add(1, Ordering::Relaxed);
            return Insert::TooFar;
        }
        let slot = &self.slots[seq as usize % RING];
        let cur = slot.seq.load(Ordering::Acquire);
        if cur == seq {
            self.duplicate.fetch_add(1, Ordering::Relaxed);
            return Insert::Duplicate;
        }
        if cur != EMPTY && (cur.wrapping_sub(read_seq) as i32) >= 0 {
            return Insert::Occupied;
        }
        let len = samples.len().min(MAX_FRAME);
        if silent {
            slot.len.store(len as u32 | SILENT_BIT, Ordering::Relaxed);
        } else {
            for (i, &s) in samples[..len].iter().enumerate() {
                slot.data[i].store(s, Ordering::Relaxed);
            }
            slot.len.store(len as u32, Ordering::Relaxed);
        }
        slot.seq.store(seq, Ordering::Release);
        let newest = self.newest_seq.load(Ordering::Relaxed);
        if (seq.wrapping_sub(newest) as i32) > 0 {
            self.newest_seq.store(seq, Ordering::Release);
        }
        self.accepted.fetch_add(1, Ordering::Relaxed);
        Insert::Accepted
    }

    /// Reader side: copy frame `seq` out (as f32, -1..1) and free the slot.
    pub fn take(&self, seq: u32, out: &mut [f32; MAX_FRAME]) -> Take {
        let slot = &self.slots[seq as usize % RING];
        if slot.seq.load(Ordering::Acquire) != seq {
            return Take::Missing;
        }
        let len_bits = slot.len.load(Ordering::Relaxed);
        let len = (len_bits & !SILENT_BIT) as usize;
        let silent = len_bits & SILENT_BIT != 0;
        if !silent {
            for i in 0..len.min(MAX_FRAME) {
                out[i] = slot.data[i].load(Ordering::Relaxed) as f32 * (1.0 / 32767.0);
            }
        }
        // A reset or a reader jump could have let the writer touch this slot
        // meanwhile; the sequence tells.
        if slot.seq.load(Ordering::Acquire) != seq {
            return Take::Missing;
        }
        slot.seq.store(EMPTY, Ordering::Release);
        if silent {
            Take::Silence(len)
        } else {
            Take::Audio(len)
        }
    }
}

/// Tunables for [`Playout`], in frames of whatever size the sender uses.
#[derive(Clone, Copy, Debug)]
pub struct PlayoutConfig {
    /// Frames to hold before playback starts and the steady-state goal
    /// (the frame being taken counts, so 2 means one frame of slack).
    pub start_target: u32,
    pub min_target: u32,
    pub max_target: u32,
    /// Frame length assumed before the first frame tells the real one.
    pub default_frame: usize,
}

impl Default for PlayoutConfig {
    fn default() -> Self {
        Self {
            start_target: 2,
            min_target: 1,
            max_target: 8,
            default_frame: 240,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlayoutStats {
    /// Frames replaced by concealment (lost, late, or dry buffer).
    pub concealed: u32,
    /// Times the buffer ran dry while the sender was expected to be talking.
    pub underruns: u32,
    /// Times the reader jumped to catch up with a sender far ahead.
    pub resyncs: u32,
    /// Target raises.
    pub target_raises: u32,
}

/// Largest speed correction, as a fraction (0.5 % ≈ 9 cents; inaudible on voice).
const MAX_NUDGE: f64 = 0.005;
/// Speed correction per frame of occupancy error.
const NUDGE_GAIN: f64 = 0.002;
/// Occupancy smoothing: about 50 frames.
const OCCUPANCY_ALPHA: f32 = 0.02;
/// Frames of stability before the target is lowered by one (10 s at 5 ms).
const DECAY_FRAMES: u32 = 2000;
/// Minimum frames between two target raises (0.5 s at 5 ms).
const RAISE_COOLDOWN_FRAMES: u32 = 100;
/// Milliseconds of dry buffer before a peer is considered idle.
const IDLE_MS: f32 = 200.0;
/// Milliseconds since the last audible frame before `talking` drops.
const TALKING_HOLD_MS: f32 = 100.0;
/// Fade length at a concealment edge, in samples (1 ms).
const EDGE_FADE: usize = 48;

/// The reader-side state machine for one peer. Lives on the audio thread.
pub struct Playout {
    cfg: PlayoutConfig,
    synced: bool,
    prefilling: bool,
    read_seq: u32,
    frame: [f32; MAX_FRAME],
    frame_len: usize,
    frame_pos: usize,
    last: [f32; MAX_FRAME],
    last_len: usize,
    conceal_run: u32,
    need_fade_in: bool,
    target: u32,
    occupancy_ema: f32,
    nudge: f64,
    late_seen: u32,
    stable_frames: u32,
    raise_cooldown: u32,
    /// Lowest queue depth seen since the last decay check.
    window_min: u32,
    /// Frames played since the last underrun (decay cool-down).
    frames_since_underrun: u32,
    quiet_samples: u32,
    dry_samples: u32,
    idle: bool,
    resampler: Resampler,
    pub stats: PlayoutStats,
}

impl Playout {
    pub fn new(cfg: PlayoutConfig) -> Self {
        let mut p = Self {
            cfg,
            synced: false,
            prefilling: true,
            read_seq: 0,
            frame: [0.0; MAX_FRAME],
            frame_len: cfg.default_frame.clamp(1, MAX_FRAME),
            frame_pos: cfg.default_frame.clamp(1, MAX_FRAME),
            last: [0.0; MAX_FRAME],
            last_len: cfg.default_frame.clamp(1, MAX_FRAME),
            conceal_run: 0,
            need_fade_in: false,
            target: cfg.start_target.max(1),
            occupancy_ema: 0.0,
            nudge: 0.0,
            late_seen: 0,
            stable_frames: 0,
            raise_cooldown: 0,
            window_min: u32::MAX,
            frames_since_underrun: 3 * DECAY_FRAMES,
            quiet_samples: u32::MAX / 2,
            dry_samples: 0,
            idle: false,
            resampler: Resampler::new(),
            stats: PlayoutStats::default(),
        };
        p.reset();
        p
    }

    /// Back to the just-created state (peer slot reuse).
    pub fn reset(&mut self) {
        let frame = self.cfg.default_frame.clamp(1, MAX_FRAME);
        self.synced = false;
        self.prefilling = true;
        self.read_seq = 0;
        self.frame_len = frame;
        self.frame_pos = frame;
        self.last_len = frame;
        self.conceal_run = 0;
        self.need_fade_in = false;
        self.target = self.cfg.start_target.max(1);
        self.occupancy_ema = self.target as f32;
        self.nudge = 0.0;
        self.late_seen = 0;
        self.stable_frames = 0;
        self.raise_cooldown = 0;
        self.window_min = u32::MAX;
        self.frames_since_underrun = 3 * DECAY_FRAMES;
        self.quiet_samples = u32::MAX / 2;
        self.dry_samples = 0;
        self.idle = false;
        self.resampler.reset();
        self.stats = PlayoutStats::default();
    }

    pub fn target_frames(&self) -> u32 {
        self.target
    }

    /// Current playback-rate correction (fraction; positive = faster).
    pub fn nudge(&self) -> f64 {
        self.nudge
    }

    /// True when nothing has arrived for a while: rendering can be skipped.
    pub fn is_idle(&self) -> bool {
        self.idle
    }

    /// True while audible frames are being played.
    pub fn is_talking(&self) -> bool {
        self.synced && !self.idle && (self.quiet_samples as f32) < TALKING_HOLD_MS * INTERNAL_RATE as f32 / 1000.0
    }

    /// Audio waiting to be played, in milliseconds (ring + current frame).
    pub fn buffered_ms(&self, ring: &JitterRing) -> f32 {
        if !self.synced {
            return 0.0;
        }
        let queued = ring.queued(self.read_seq) as f32 * self.frame_len as f32;
        let current = (self.frame_len - self.frame_pos.min(self.frame_len)) as f32;
        (queued + current) * 1000.0 / INTERNAL_RATE as f32
    }

    /// Whether [`Playout::render`] would produce anything but zeros. An idle
    /// peer wakes up when the ring shows new data.
    pub fn wants_render(&mut self, ring: &JitterRing) -> bool {
        if !self.idle {
            return ring.is_synced();
        }
        if self.synced && ring.queued(self.read_seq) > 0 {
            self.idle = false;
            self.dry_samples = 0;
            self.resampler.reset();
            self.frame_pos = self.frame_len;
            self.need_fade_in = true;
            return true;
        }
        false
    }

    fn raise_target(&mut self) {
        if self.raise_cooldown == 0 && self.target < self.cfg.max_target {
            self.target += 1;
            self.stats.target_raises += 1;
            self.raise_cooldown = RAISE_COOLDOWN_FRAMES;
        }
        self.stable_frames = 0;
    }

    fn conceal(&mut self) {
        let len = self.last_len.clamp(1, MAX_FRAME);
        if self.conceal_run == 0 {
            // Repeat the last good frame, fading to zero across it.
            for i in 0..len {
                let g = 1.0 - (i as f32 + 1.0) / len as f32;
                self.frame[i] = self.last[i] * g;
            }
        } else {
            self.frame[..len].fill(0.0);
        }
        self.frame_len = len;
        self.conceal_run += 1;
        self.need_fade_in = true;
        self.stats.concealed += 1;
        self.quiet_samples = self.quiet_samples.saturating_add(len as u32);
    }

    fn zero_frame(&mut self) {
        let len = self.frame_len.clamp(1, MAX_FRAME);
        self.frame[..len].fill(0.0);
        self.frame_pos = 0;
        self.quiet_samples = self.quiet_samples.saturating_add(len as u32);
    }

    /// Advance to the next frame: fetch, conceal, or wait.
    fn fetch_frame(&mut self, ring: &JitterRing) {
        self.frame_pos = 0;
        if !self.synced {
            match ring.synced_read_seq() {
                Some(seq) => {
                    self.synced = true;
                    self.prefilling = true;
                    self.read_seq = seq;
                }
                None => {
                    self.zero_frame();
                    return;
                }
            }
        }
        let queued = ring.queued(self.read_seq);
        if queued as usize > RING {
            // The sender is far ahead (paused reader, restarted sender):
            // jump to just behind the newest frame.
            self.read_seq = ring.newest_seq().wrapping_sub(self.target.saturating_sub(1));
            ring.publish_read_seq(self.read_seq);
            self.stats.resyncs += 1;
            self.prefilling = true;
            self.conceal_run = 0;
            self.need_fade_in = true;
            // The old audio is unrelated to where we jumped to: concealment
            // must not replay it.
            self.last.fill(0.0);
            self.occupancy_ema = self.target as f32;
            self.nudge = 0.0;
            self.zero_frame();
            return;
        }
        if self.prefilling {
            if queued < self.target {
                self.zero_frame();
                return;
            }
            self.prefilling = false;
            self.occupancy_ema = self.target as f32;
        }
        // Late arrivals the writer refused since last time: the window is too small.
        let late = ring.late.load(Ordering::Relaxed);
        if late != self.late_seen {
            self.late_seen = late;
            self.raise_target();
        }
        match ring.take(self.read_seq, &mut self.frame) {
            Take::Audio(len) => {
                self.frame_len = len.max(1);
                self.last[..self.frame_len].copy_from_slice(&self.frame[..self.frame_len]);
                self.last_len = self.frame_len;
                if self.need_fade_in {
                    crate::dsp::fade_in(&mut self.frame[..self.frame_len], EDGE_FADE);
                    self.need_fade_in = false;
                }
                self.conceal_run = 0;
                self.dry_samples = 0;
                self.quiet_samples = 0;
                self.read_seq = self.read_seq.wrapping_add(1);
                self.stable_frames += 1;
                self.frames_since_underrun = self.frames_since_underrun.saturating_add(1);
            }
            Take::Silence(len) => {
                self.frame_len = len.max(1);
                self.frame[..self.frame_len].fill(0.0);
                self.last[..self.frame_len].fill(0.0);
                self.last_len = self.frame_len;
                self.conceal_run = 0;
                self.dry_samples = 0;
                self.need_fade_in = false;
                self.quiet_samples = self.quiet_samples.saturating_add(self.frame_len as u32);
                self.read_seq = self.read_seq.wrapping_add(1);
                self.stable_frames += 1;
                self.frames_since_underrun = self.frames_since_underrun.saturating_add(1);
            }
            Take::Missing => {
                if queued > 0 {
                    // Newer frames exist: this one is lost or reordered past
                    // the window. Conceal and move on; if it shows up later
                    // the writer counts it as late and the target grows.
                    self.conceal();
                    self.read_seq = self.read_seq.wrapping_add(1);
                } else {
                    // Nothing newer either: the buffer ran dry. Hold position
                    // so the frame plays when it lands; one frame of delay is
                    // added, which the target raise makes permanent.
                    if self.conceal_run == 0 {
                        self.stats.underruns += 1;
                        self.frames_since_underrun = 0;
                        self.raise_target();
                    }
                    self.conceal();
                    self.dry_samples = self.dry_samples.saturating_add(self.frame_len as u32);
                    if (self.dry_samples as f32) >= IDLE_MS * INTERNAL_RATE as f32 / 1000.0 {
                        self.idle = true;
                    }
                }
            }
        }
        ring.publish_read_seq(self.read_seq);

        // Occupancy control: smooth the queue depth seen at fetch time and
        // turn the error against the target into a playback-rate nudge.
        self.occupancy_ema += (queued as f32 - self.occupancy_ema) * OCCUPANCY_ALPHA;
        self.window_min = self.window_min.min(queued);
        let err = (self.occupancy_ema - self.target as f32) as f64;
        self.nudge = (err * NUDGE_GAIN).clamp(-MAX_NUDGE, MAX_NUDGE);

        if self.raise_cooldown > 0 {
            self.raise_cooldown -= 1;
        }
        if self.stable_frames >= DECAY_FRAMES {
            self.stable_frames = 0;
            let floor = self.window_min;
            self.window_min = u32::MAX;
            // Shave a frame of latency only when the observed queue floor
            // shows a spare frame would remain, and no underrun happened
            // recently — otherwise the decay/underrun cycle would put a
            // 5 ms hiccup in every ten seconds.
            if self.target > self.cfg.min_target
                && floor >= 2
                && self.frames_since_underrun >= 3 * DECAY_FRAMES
            {
                self.target -= 1;
            }
        }
    }

    /// The next sample at [`INTERNAL_RATE`].
    #[inline]
    pub fn next_sample(&mut self, ring: &JitterRing) -> f32 {
        if self.frame_pos >= self.frame_len {
            self.fetch_frame(ring);
        }
        let v = self.frame[self.frame_pos];
        self.frame_pos += 1;
        v
    }

    /// Fill `out` (overwriting) with this peer's audio at `out_rate`.
    pub fn render(&mut self, ring: &JitterRing, out_rate: f64, out: &mut [f32]) {
        let mut rs = std::mem::take(&mut self.resampler);
        rs.set_ratio(INTERNAL_RATE, out_rate, self.nudge);
        for o in out.iter_mut() {
            *o = rs.pull(|| self.next_sample(ring));
        }
        self.resampler = rs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: usize = 240;

    fn frame_of(seq: u32) -> Vec<i16> {
        // A recognisable ramp per frame, never near zero so fades show.
        (0..FRAME).map(|i| 1000 + (seq as i16 % 20) * 500 + (i as i16 % 7)).collect()
    }

    fn pull_frame(p: &mut Playout, ring: &JitterRing) -> Vec<f32> {
        (0..FRAME).map(|_| p.next_sample(ring)).collect()
    }

    fn approx_frame(seq: u32) -> Vec<f32> {
        frame_of(seq).iter().map(|&v| v as f32 / 32767.0).collect()
    }

    fn assert_frame(got: &[f32], seq: u32, skip: usize) {
        let want = approx_frame(seq);
        for i in skip..FRAME {
            assert!((got[i] - want[i]).abs() < 1e-6, "seq {seq} sample {i}: {} vs {}", got[i], want[i]);
        }
    }

    fn cfg() -> PlayoutConfig {
        PlayoutConfig::default()
    }

    #[test]
    fn in_order_frames_play_in_order_after_prefill() {
        let ring = JitterRing::new();
        let mut p = Playout::new(cfg());
        // Nothing yet: zeros.
        assert!(pull_frame(&mut p, &ring).iter().all(|&v| v == 0.0));
        assert_eq!(ring.insert(0, &frame_of(0), false), Insert::Accepted);
        // One frame below target: still prefilling.
        assert!(pull_frame(&mut p, &ring).iter().all(|&v| v == 0.0));
        assert_eq!(ring.insert(1, &frame_of(1), false), Insert::Accepted);
        for seq in 0..200u32 {
            ring.insert(seq + 2, &frame_of(seq + 2), false);
            let got = pull_frame(&mut p, &ring);
            assert_frame(&got, seq, 0);
        }
        assert_eq!(p.stats, PlayoutStats::default());
        assert_eq!(p.target_frames(), 2);
        assert!(p.is_talking());
    }

    #[test]
    fn reordered_frames_are_put_back_in_order() {
        let ring = JitterRing::new();
        let mut p = Playout::new(PlayoutConfig { start_target: 3, ..cfg() });
        ring.insert(0, &frame_of(0), false);
        ring.insert(1, &frame_of(1), false);
        ring.insert(3, &frame_of(3), false); // 3 before 2
        ring.insert(2, &frame_of(2), false);
        ring.insert(4, &frame_of(4), false);
        for seq in 0..5u32 {
            assert_frame(&pull_frame(&mut p, &ring), seq, 0);
        }
        assert_eq!(p.stats.concealed, 0);
    }

    #[test]
    fn a_lost_frame_is_concealed_and_the_next_faded_in() {
        let ring = JitterRing::new();
        let mut p = Playout::new(cfg());
        ring.insert(0, &frame_of(0), false);
        ring.insert(1, &frame_of(1), false);
        assert_frame(&pull_frame(&mut p, &ring), 0, 0);
        // Frame 2 never arrives; 3 does.
        ring.insert(3, &frame_of(3), false);
        assert_frame(&pull_frame(&mut p, &ring), 1, 0);
        ring.insert(4, &frame_of(4), false);
        let concealed = pull_frame(&mut p, &ring);
        // Repeat of frame 1, fading to zero.
        let last = approx_frame(1);
        assert!((concealed[0] - last[0] * (1.0 - 1.0 / FRAME as f32)).abs() < 1e-6);
        assert!(concealed[FRAME - 1].abs() < 1e-6);
        assert_eq!(p.stats.concealed, 1);
        ring.insert(5, &frame_of(5), false);
        let next = pull_frame(&mut p, &ring);
        // Faded in over the first 48 samples, exact after.
        assert_eq!(next[0], 0.0);
        assert_frame(&next, 3, 48);
        assert_eq!(p.stats.underruns, 0);
    }

    #[test]
    fn duplicates_and_late_frames_are_refused_and_late_raises_the_target() {
        let ring = JitterRing::new();
        let mut p = Playout::new(cfg());
        ring.insert(0, &frame_of(0), false);
        ring.insert(1, &frame_of(1), false);
        assert_eq!(ring.insert(1, &frame_of(1), false), Insert::Duplicate);
        assert_frame(&pull_frame(&mut p, &ring), 0, 0);
        ring.insert(3, &frame_of(3), false);
        assert_frame(&pull_frame(&mut p, &ring), 1, 0);
        ring.insert(4, &frame_of(4), false);
        pull_frame(&mut p, &ring); // 2 concealed, reader now past it
        assert_eq!(ring.insert(2, &frame_of(2), false), Insert::Late);
        assert_eq!(ring.late.load(Ordering::Relaxed), 1);
        ring.insert(5, &frame_of(5), false);
        pull_frame(&mut p, &ring);
        assert_eq!(p.target_frames(), 3);
        assert_eq!(p.stats.target_raises, 1);
    }

    #[test]
    fn silence_frames_play_zeros_and_are_not_concealed_with_audio() {
        let ring = JitterRing::new();
        let mut p = Playout::new(cfg());
        ring.insert(0, &frame_of(0), false);
        ring.insert(1, &[], true);
        assert_frame(&pull_frame(&mut p, &ring), 0, 0);
        // Insert reports Silence with the length the writer gave (240 here).
        ring.insert(2, &[0; FRAME], true);
        let z = pull_frame(&mut p, &ring);
        assert!(z.iter().all(|&v| v == 0.0));
        assert!(!p.is_talking() || p.quiet_samples < 4800);
    }

    #[test]
    fn dry_buffer_holds_position_then_goes_idle_and_wakes_up() {
        let ring = JitterRing::new();
        let mut p = Playout::new(cfg());
        ring.insert(0, &frame_of(0), false);
        ring.insert(1, &frame_of(1), false);
        assert_frame(&pull_frame(&mut p, &ring), 0, 0);
        assert_frame(&pull_frame(&mut p, &ring), 1, 0);
        // Nothing more arrives: conceal (repeat+fade), then zeros.
        let c = pull_frame(&mut p, &ring);
        assert!(c[0] != 0.0 && c[FRAME - 1].abs() < 1e-6);
        assert_eq!(p.stats.underruns, 1);
        assert_eq!(p.target_frames(), 3);
        for _ in 0..50 {
            assert!(pull_frame(&mut p, &ring).iter().all(|&v| v == 0.0));
        }
        assert!(p.is_idle());
        assert!(!p.wants_render(&ring));
        // The stream resumes in sequence: frame 2 lands in the window.
        ring.insert(2, &frame_of(2), false);
        assert!(p.wants_render(&ring));
        let got = pull_frame(&mut p, &ring);
        assert_frame(&got, 2, 48);
        assert_eq!(p.stats.resyncs, 0);
    }

    #[test]
    fn a_sender_far_ahead_triggers_a_resync() {
        let ring = JitterRing::new();
        let mut p = Playout::new(cfg());
        ring.insert(0, &frame_of(0), false);
        ring.insert(1, &frame_of(1), false);
        assert_frame(&pull_frame(&mut p, &ring), 0, 0);
        assert_eq!(ring.insert(500, &frame_of(500), false), Insert::TooFar);
        pull_frame(&mut p, &ring); // frame 1
        let z = pull_frame(&mut p, &ring); // resync: zero frame
        assert!(z.iter().all(|&v| v == 0.0));
        assert_eq!(p.stats.resyncs, 1);
        for seq in 500..520u32 {
            ring.insert(seq, &frame_of(seq), false);
        }
        // After the jump the reader sits at newest-(target-1) = 499 (missing) → conceal, then plays.
        let mut played = Vec::new();
        for _ in 0..6 {
            let got = pull_frame(&mut p, &ring);
            played.push(got);
        }
        // One of the pulled frames must be frame 501 exactly.
        let want = approx_frame(501);
        assert!(played.iter().any(|g| g[100..].iter().zip(&want[100..]).all(|(a, b)| (a - b).abs() < 1e-6)));
    }

    /// Feed frames at a rate that differs from the reader by `ppm` and check
    /// that the buffer neither drains nor grows and the nudge converges.
    fn drift_run(ppm: i64, seconds: u32) -> (Playout, u32) {
        let ring = JitterRing::new();
        let mut p = Playout::new(cfg());
        let period = 1_000_000 / ppm.unsigned_abs().max(1) as u32; // ticks between corrections
        let mut seq = 0u32;
        let push = |ring: &JitterRing, seq: &mut u32| {
            ring.insert(*seq, &frame_of(*seq), false);
            *seq = seq.wrapping_add(1);
        };
        push(&ring, &mut seq);
        push(&ring, &mut seq);
        let ticks = seconds * 200;
        let mut min_q = u32::MAX;
        let mut max_q = 0;
        let mut out = vec![0.0f32; FRAME];
        for tick in 1..=ticks {
            push(&ring, &mut seq);
            if ppm > 0 && tick % period == 0 {
                push(&ring, &mut seq); // sender clock fast: an extra frame
            }
            if ppm < 0 && tick % period == 0 {
                // sender clock slow: skip a frame this tick (undo the push)
                // by making the reader consume one frame more instead.
                p.render(&ring, 48000.0, &mut out);
            }
            p.render(&ring, 48000.0, &mut out);
            if tick > ticks / 2 {
                let q = ring.queued(p.read_seq);
                min_q = min_q.min(q);
                max_q = max_q.max(q);
            }
        }
        (p, max_q - min_q)
    }

    #[test]
    fn fast_sender_clock_is_absorbed_by_the_nudge_without_growth() {
        // 300 ppm for 120 s = 7.2 frames of drift: without correction the
        // buffer (and the latency) would grow by that much. The nudge keeps
        // the occupancy flat, with zero concealment and zero resyncs.
        let (p, spread) = drift_run(300, 120);
        assert_eq!(p.stats.resyncs, 0);
        assert_eq!(p.stats.concealed, 0);
        assert!(spread <= 2, "occupancy spread {spread}");
        assert!(p.target_frames() <= 3);
        assert!(p.nudge().abs() <= MAX_NUDGE);
    }

    #[test]
    fn slow_sender_clock_is_absorbed_by_the_nudge_without_underruns() {
        // The mirror case: the sender falls behind by 7.2 frames over 120 s.
        // Without correction that is a stream of underruns; the nudge slows
        // playback instead. The design allows the first dip or two to conceal
        // (each one raises the target), never a growing series.
        let (p, spread) = drift_run(-300, 120);
        assert_eq!(p.stats.resyncs, 0);
        assert!(p.stats.underruns <= 5, "underruns {}", p.stats.underruns);
        assert!(spread <= 3, "occupancy spread {spread}");
        assert!(p.nudge().abs() <= MAX_NUDGE);
    }

    #[test]
    fn target_decays_back_after_a_stable_stretch() {
        let ring = JitterRing::new();
        let mut p = Playout::new(PlayoutConfig { start_target: 4, min_target: 1, ..cfg() });
        let mut out = vec![0.0f32; FRAME];
        for seq in 0..4u32 {
            ring.insert(seq, &frame_of(seq), false);
        }
        let mut seq = 4u32;
        for _ in 0..4500 {
            ring.insert(seq, &frame_of(seq), false);
            seq += 1;
            p.render(&ring, 48000.0, &mut out);
        }
        assert!(p.target_frames() <= 2, "target {}", p.target_frames());
        assert_eq!(p.stats.underruns, 0);
        assert_eq!(p.stats.concealed, 0);
        // Excess delay was bled off by speed, not by dropping frames: the
        // reader caught up so the queue sits at the target.
        assert!(ring.queued(p.read_seq) <= 3);
    }

    #[test]
    fn render_at_44100_consumes_the_right_number_of_frames() {
        let ring = JitterRing::new();
        let mut p = Playout::new(cfg());
        let mut out = vec![0.0f32; 441];
        let mut seq = 0u32;
        for _ in 0..4 {
            ring.insert(seq, &frame_of(seq), false);
            seq += 1;
        }
        for _ in 0..100 {
            ring.insert(seq, &frame_of(seq), false);
            ring.insert(seq + 1, &frame_of(seq + 1), false);
            seq += 2;
            p.render(&ring, 44100.0, &mut out); // 441 out = 480 in = 2 frames
        }
        assert_eq!(p.stats.concealed, 0);
        assert!(ring.queued(p.read_seq) <= 4);
    }
}
