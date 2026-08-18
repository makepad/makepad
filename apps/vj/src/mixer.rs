//! The one audio engine behind `cx.audio_output`: two video-slot buses, two
//! DJ deck voices under an equal-power crossfader, and a bounded pool of
//! one-shot SFX voices, summed through a master gain with a hard safety
//! clamp.
//!
//! Threading contract:
//! - the device callback calls [`Mixer::render`]; it `try_lock`s the state
//!   and leaves the (pre-zeroed) buffer silent on contention — it never
//!   blocks on the UI,
//! - UI/engine threads mutate through short-lock methods; every audible
//!   parameter change goes through a [`Ramp`] (a few ms of slew), so gain
//!   moves, mutes, crossfades and slot fades are click-free,
//! - video decode threads push PCM into per-slot queues and read back the
//!   buffered depth for pacing; closing a slot just flushes and mutes it —
//!   nobody joins anybody.
//!
//! The device clock is the position truth: deck playheads and end-of-track
//! flags advance only inside `render`.

use crate::cue::SlotId;
use crate::decks::{crossfader_gains, DeckId, FadeCurve};
use crate::pads::{PadKey, VoiceAlloc, VoiceId};
use makepad_widgets::makepad_platform::audio::AudioBuffer;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Q32.32 fixed-point source-frame cursor.
const FP_ONE: u64 = 1 << 32;
/// Default parameter slew, seconds — fast enough to feel instant, slow
/// enough to never click.
const SLEW_SECS: f32 = 0.008;
/// Cap on queued video-slot audio, frames (~2s at 48k): a stalled consumer
/// can never grow a queue without bound.
const MAX_SLOT_QUEUE_FRAMES: usize = 96_000;
/// Master safety clamp.
const CLAMP: f32 = 1.0;
pub const MIN_VIDEO_PLAYBACK_RATE: f64 = 0.92;
pub const MAX_VIDEO_PLAYBACK_RATE: f64 = 1.08;

pub type VideoTransitionId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum VideoTransitionPhase {
    Idle = 0,
    Armed = 1,
    Started = 2,
    Completed = 3,
    Cancelled = 4,
    /// The exact target elapsed during a callback that could not acquire the
    /// realtime state. The destination remains paused for host rescheduling;
    /// the mixer never starts a beat late.
    Missed = 5,
}

impl VideoTransitionPhase {
    fn from_u32(value: u32) -> Self {
        match value {
            1 => Self::Armed,
            2 => Self::Started,
            3 => Self::Completed,
            4 => Self::Cancelled,
            5 => Self::Missed,
            _ => Self::Idle,
        }
    }
}

/// Lock-free view of the transition driven by the audio device clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoTransitionSnapshot {
    pub id: VideoTransitionId,
    pub phase: VideoTransitionPhase,
    pub from: Option<SlotId>,
    pub to: SlotId,
    pub target_frame: u64,
    pub start_frame: Option<u64>,
    pub fade_frames: u64,
    pub rendered_frame: u64,
    pub progress: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoTransitionError {
    ZeroId,
    DestinationClosed,
    SameSlot,
    TransitionAlreadyStarted,
}

struct TransitionAtomics {
    sequence: AtomicU64,
    id: AtomicU64,
    phase: AtomicU32,
    from: AtomicU32,
    to: AtomicU32,
    target_frame: AtomicU64,
    start_frame: AtomicU64,
    fade_frames: AtomicU64,
    rendered_frame: AtomicU64,
}

impl TransitionAtomics {
    fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            id: AtomicU64::new(0),
            phase: AtomicU32::new(VideoTransitionPhase::Idle as u32),
            from: AtomicU32::new(0),
            to: AtomicU32::new(1),
            target_frame: AtomicU64::new(0),
            start_frame: AtomicU64::new(u64::MAX),
            fade_frames: AtomicU64::new(0),
            rendered_frame: AtomicU64::new(0),
        }
    }

    fn encode_slot(slot: Option<SlotId>) -> u32 {
        match slot {
            None => 0,
            Some(SlotId::A) => 1,
            Some(SlotId::B) => 2,
        }
    }

    fn decode_slot(value: u32) -> Option<SlotId> {
        match value {
            1 => Some(SlotId::A),
            2 => Some(SlotId::B),
            _ => None,
        }
    }

    fn publish_arm(&self, transition: ScheduledVideoTransition, rendered_frame: u64) {
        self.sequence.fetch_add(1, Ordering::AcqRel);
        self.from.store(Self::encode_slot(transition.from), Ordering::Relaxed);
        self.to.store(Self::encode_slot(Some(transition.to)), Ordering::Relaxed);
        self.target_frame.store(transition.target_frame, Ordering::Relaxed);
        self.start_frame.store(u64::MAX, Ordering::Relaxed);
        self.fade_frames.store(transition.fade_frames, Ordering::Relaxed);
        self.rendered_frame.store(rendered_frame, Ordering::Relaxed);
        self.id.store(transition.id, Ordering::Relaxed);
        self.phase.store(VideoTransitionPhase::Armed as u32, Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
    }

    fn publish_phase(&self, phase: VideoTransitionPhase, frame: u64) {
        self.sequence.fetch_add(1, Ordering::AcqRel);
        if phase == VideoTransitionPhase::Started {
            self.start_frame.store(frame, Ordering::Relaxed);
        }
        self.rendered_frame.store(frame, Ordering::Relaxed);
        self.phase.store(phase as u32, Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
    }

    fn publish_rendered_frame(&self, frame: u64) {
        self.sequence.fetch_add(1, Ordering::AcqRel);
        self.rendered_frame.store(frame, Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
    }
}

/// A fully decoded, immutable PCM clip (interleaved stereo i16).
pub struct TrackPcm {
    pub frames: Vec<[i16; 2]>,
    pub sample_rate: u32,
}

impl TrackPcm {
    pub fn seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.frames.len() as f64 / self.sample_rate as f64
    }
}

/// Linear parameter ramp advanced once per output frame.
#[derive(Clone, Copy)]
struct Ramp {
    current: f32,
    target: f32,
    /// Per-frame step; 0 = settled.
    step: f32,
}

impl Ramp {
    fn at(value: f32) -> Ramp {
        Ramp { current: value, target: value, step: 0.0 }
    }

    /// Move to `target` over `secs` — the whole move takes `secs` no matter
    /// how far it travels. `step` stores the rate in units/second.
    fn slew(&mut self, target: f32, secs: f32) {
        self.target = target;
        let distance = (target - self.current).abs();
        self.step = if secs <= 0.0 { f32::MAX } else { (distance / secs).max(1e-6) };
    }

    /// Per-frame advance at the given device rate.
    fn tick(&mut self, device_rate: f32) -> f32 {
        if self.current != self.target {
            let per_frame = self.step / device_rate.max(1.0);
            let delta = self.target - self.current;
            if delta.abs() <= per_frame {
                self.current = self.target;
            } else {
                self.current += per_frame * delta.signum();
            }
        }
        self.current
    }
}

struct VideoBus {
    open: bool,
    paused: bool,
    queue: VecDeque<(f32, f32)>,
    source_rate: f64,
    cursor: f64,
    playback_rate: f64,
    gain: Ramp,
}

impl VideoBus {
    fn new() -> VideoBus {
        VideoBus {
            open: false,
            paused: false,
            queue: VecDeque::new(),
            source_rate: 0.0,
            cursor: 0.0,
            playback_rate: 1.0,
            gain: Ramp::at(0.0),
        }
    }

    fn flush(&mut self) {
        self.queue.clear();
        self.cursor = 0.0;
    }
}

struct DeckVoice {
    pcm: Option<Arc<TrackPcm>>,
    cursor_fp: u64,
    playing: bool,
    loop_on: bool,
    gain: Ramp,
    mute: Ramp,
    ended: bool,
}

impl DeckVoice {
    fn new() -> DeckVoice {
        DeckVoice {
            pcm: None,
            cursor_fp: 0,
            playing: false,
            loop_on: false,
            gain: Ramp::at(1.0),
            mute: Ramp::at(1.0),
            ended: false,
        }
    }
}

struct SfxVoice {
    id: VoiceId,
    pad: PadKey,
    pcm: Arc<TrackPcm>,
    cursor_fp: u64,
    loop_on: bool,
    gain: Ramp,
    done: bool,
}

#[derive(Clone, Copy)]
struct ScheduledVideoTransition {
    id: VideoTransitionId,
    from: Option<SlotId>,
    to: SlotId,
    target_frame: u64,
    fade_frames: u64,
    started: bool,
}

struct MixState {
    video: [VideoBus; 2],
    /// Program-wide video mute, ORTHOGONAL to the per-slot fade gains: it
    /// multiplies the summed video bus, so muting never disturbs (and
    /// unmuting exactly restores) in-flight crossfade targets.
    video_mute: Ramp,
    decks: [DeckVoice; 2],
    fader: Ramp,
    curve: FadeCurve,
    sfx: Vec<SfxVoice>,
    master: Ramp,
    ended_decks: Vec<DeckId>,
    ended_voices: Vec<VoiceId>,
    rendered_frames: u64,
    scheduled_video: Option<ScheduledVideoTransition>,
}

/// Peak meters (f32 bits): master, video, deck A, deck B, sfx.
pub const METER_MASTER: usize = 0;
pub const METER_VIDEO: usize = 1;
pub const METER_DECK_A: usize = 2;
pub const METER_DECK_B: usize = 3;
pub const METER_SFX: usize = 4;

#[derive(Clone)]
pub struct Mixer {
    state: Arc<Mutex<MixState>>,
    meters: Arc<[AtomicU32; 5]>,
    transition: Arc<TransitionAtomics>,
    device_frames: Arc<AtomicU64>,
    device_rate_bits: Arc<AtomicU64>,
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new()
    }
}

impl Mixer {
    pub fn new() -> Mixer {
        Mixer {
            state: Arc::new(Mutex::new(MixState {
                video: [VideoBus::new(), VideoBus::new()],
                video_mute: Ramp::at(1.0),
                decks: [DeckVoice::new(), DeckVoice::new()],
                fader: Ramp::at(0.0),
                curve: FadeCurve::EqualPower,
                sfx: Vec::new(),
                master: Ramp::at(0.9),
                ended_decks: Vec::new(),
                ended_voices: Vec::new(),
                rendered_frames: 0,
                scheduled_video: None,
            })),
            meters: Arc::new([
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
            ]),
            transition: Arc::new(TransitionAtomics::new()),
            device_frames: Arc::new(AtomicU64::new(0)),
            device_rate_bits: Arc::new(AtomicU64::new(0)),
        }
    }

    // ---- video slot buses --------------------------------------------------

    /// (Re)open a slot bus, silent, empty, unpaused.
    pub fn open_slot(&self, slot: SlotId) {
        let mut s = self.state.lock().unwrap();
        let bus = &mut s.video[slot.index()];
        bus.flush();
        bus.open = true;
        bus.paused = false;
        bus.playback_rate = 1.0;
        bus.gain = Ramp::at(0.0);
    }

    /// Close = mute-and-flush; the decode thread just stops feeding it.
    pub fn close_slot(&self, slot: SlotId) {
        let mut s = self.state.lock().unwrap();
        if let Some(scheduled) = s.scheduled_video.filter(|scheduled| scheduled.to == slot) {
            s.scheduled_video = None;
            // The device may have crossed the target just before the UI
            // observed `Started`. If latest-click-wins closes that still-
            // armed destination, restore the previous program atomically
            // instead of leaving a half-faded silence.
            if scheduled.started {
                if let Some(from) = scheduled.from {
                    s.video[from.index()].gain = Ramp::at(1.0);
                }
            }
            self.transition.publish_phase(
                VideoTransitionPhase::Cancelled,
                self.device_frames.load(Ordering::Acquire),
            );
            debug_assert_eq!(scheduled.to, slot);
        }
        let bus = &mut s.video[slot.index()];
        bus.open = false;
        bus.flush();
        bus.gain = Ramp::at(0.0);
    }

    /// Decode-thread entry: append interleaved i16 PCM. Returns false when
    /// the slot is closed (the producer should stop).
    pub fn push_slot_audio(
        &self,
        slot: SlotId,
        samples: &[i16],
        channels: u16,
        rate: u32,
    ) -> bool {
        let mut s = self.state.lock().unwrap();
        let bus = &mut s.video[slot.index()];
        if !bus.open {
            return false;
        }
        bus.source_rate = rate as f64;
        let ch = channels.max(1) as usize;
        for frame in samples.chunks_exact(ch) {
            if bus.queue.len() >= MAX_SLOT_QUEUE_FRAMES {
                break;
            }
            let l = frame[0] as f32 / 32768.0;
            let r = frame[ch - 1] as f32 / 32768.0;
            bus.queue.push_back((l, r));
        }
        true
    }

    /// Buffered seconds on a slot bus (decode-thread pacing).
    pub fn slot_buffered_secs(&self, slot: SlotId) -> f64 {
        let s = self.state.lock().unwrap();
        let bus = &s.video[slot.index()];
        if bus.source_rate <= 0.0 {
            return 0.0;
        }
        (bus.queue.len() as f64 - bus.cursor).max(0.0)
            / (bus.source_rate * bus.playback_rate.max(MIN_VIDEO_PLAYBACK_RATE))
    }

    pub fn flush_slot_audio(&self, slot: SlotId) {
        self.state.lock().unwrap().video[slot.index()].flush();
    }

    pub fn set_slot_paused(&self, slot: SlotId, paused: bool) {
        self.state.lock().unwrap().video[slot.index()].paused = paused;
    }

    /// Audio resampling rate for a video slot. The bounded range is small on
    /// purpose: it is enough to fit a visual cycle to a musical phrase while
    /// remaining perceptually safe. Deck and SFX cursors are unrelated.
    pub fn set_slot_playback_rate(&self, slot: SlotId, rate: f64) -> f64 {
        let rate = rate.clamp(MIN_VIDEO_PLAYBACK_RATE, MAX_VIDEO_PLAYBACK_RATE);
        self.state.lock().unwrap().video[slot.index()].playback_rate = rate;
        rate
    }

    pub fn slot_playback_rate(&self, slot: SlotId) -> f64 {
        self.state.lock().unwrap().video[slot.index()].playback_rate
    }

    /// Number of output frames rendered by this mixer. This is the same
    /// clock used to trigger scheduled video transitions.
    pub fn rendered_output_frames(&self) -> u64 {
        self.device_frames.load(Ordering::Acquire)
    }

    pub fn output_sample_rate(&self) -> Option<f64> {
        let rate = f64::from_bits(self.device_rate_bits.load(Ordering::Acquire));
        (rate.is_finite() && rate > 0.0).then_some(rate)
    }

    /// Arm a video transition at an absolute audio-device output frame.
    /// The destination remains paused and its queue remains untouched until
    /// that exact sample is rendered.
    pub fn schedule_video_transition_at(
        &self,
        id: VideoTransitionId,
        from: Option<SlotId>,
        to: SlotId,
        target_frame: u64,
        fade_frames: u64,
    ) -> Result<u64, VideoTransitionError> {
        let mut state = self.state.lock().unwrap();
        self.schedule_video_transition_locked(
            &mut state,
            id,
            from,
            to,
            target_frame,
            fade_frames,
        )
    }

    fn schedule_video_transition_locked(
        &self,
        state: &mut MixState,
        id: VideoTransitionId,
        from: Option<SlotId>,
        to: SlotId,
        target_frame: u64,
        fade_frames: u64,
    ) -> Result<u64, VideoTransitionError> {
        if id == 0 {
            return Err(VideoTransitionError::ZeroId);
        }
        if from == Some(to) {
            return Err(VideoTransitionError::SameSlot);
        }
        if !state.video[to.index()].open {
            return Err(VideoTransitionError::DestinationClosed);
        }
        if state.scheduled_video.is_some_and(|scheduled| scheduled.started) {
            return Err(VideoTransitionError::TransitionAlreadyStarted);
        }
        if let Some(old) = state.scheduled_video.take() {
            let old_bus = &mut state.video[old.to.index()];
            old_bus.paused = true;
            old_bus.gain = Ramp::at(0.0);
        }
        let now = self.device_frames.load(Ordering::Acquire);
        let target_frame = target_frame.max(now);
        let scheduled = ScheduledVideoTransition {
            id,
            from,
            to,
            target_frame,
            fade_frames,
            started: false,
        };
        let to_bus = &mut state.video[to.index()];
        to_bus.paused = true;
        to_bus.gain = Ramp::at(0.0);
        state.scheduled_video = Some(scheduled);
        self.transition.publish_arm(scheduled, now);
        Ok(target_frame)
    }

    /// Arm relative to the current device clock. A zero delay starts at the
    /// first sample of the next successfully rendered buffer.
    pub fn schedule_video_transition_after(
        &self,
        id: VideoTransitionId,
        from: Option<SlotId>,
        to: SlotId,
        delay_frames: u64,
        fade_frames: u64,
    ) -> Result<u64, VideoTransitionError> {
        let mut state = self.state.lock().unwrap();
        let target = self
            .device_frames
            .load(Ordering::Acquire)
            .saturating_add(delay_frames);
        self.schedule_video_transition_locked(&mut state, id, from, to, target, fade_frames)
    }

    /// Cancel only while still armed. A started transition is owned by the
    /// device clock and must run to completion; callers cannot rewind it from
    /// the UI thread.
    pub fn cancel_video_transition(&self, id: VideoTransitionId) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(scheduled) = state.scheduled_video else { return false };
        if scheduled.id != id || scheduled.started {
            return false;
        }
        state.scheduled_video = None;
        let bus = &mut state.video[scheduled.to.index()];
        bus.paused = true;
        bus.gain = Ramp::at(0.0);
        self.transition.publish_phase(
            VideoTransitionPhase::Cancelled,
            self.device_frames.load(Ordering::Acquire),
        );
        true
    }

    /// Nonblocking transition state for picture pacing, lights, and cue
    /// cleanup. `None` means no schedule has ever been published.
    pub fn video_transition_snapshot(&self) -> Option<VideoTransitionSnapshot> {
        let (phase, id, rendered_frame, target_frame, fade_frames, raw_start, from, to) = loop {
            let before = self.transition.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let values = (
                VideoTransitionPhase::from_u32(self.transition.phase.load(Ordering::Relaxed)),
                self.transition.id.load(Ordering::Relaxed),
                self.transition.rendered_frame.load(Ordering::Relaxed),
                self.transition.target_frame.load(Ordering::Relaxed),
                self.transition.fade_frames.load(Ordering::Relaxed),
                self.transition.start_frame.load(Ordering::Relaxed),
                self.transition.from.load(Ordering::Relaxed),
                self.transition.to.load(Ordering::Relaxed),
            );
            let after = self.transition.sequence.load(Ordering::Acquire);
            if before == after {
                break values;
            }
        };
        if phase == VideoTransitionPhase::Idle || id == 0 {
            return None;
        }
        let start_frame = (raw_start != u64::MAX).then_some(raw_start);
        let progress = match phase {
            VideoTransitionPhase::Completed => 1.0,
            VideoTransitionPhase::Started => {
                if fade_frames == 0 {
                    1.0
                } else {
                    rendered_frame.saturating_sub(raw_start) as f32 / fade_frames as f32
                }
            }
            _ => 0.0,
        }
        .clamp(0.0, 1.0);
        Some(VideoTransitionSnapshot {
            id,
            phase,
            from: TransitionAtomics::decode_slot(from),
            to: TransitionAtomics::decode_slot(to).unwrap_or(SlotId::A),
            target_frame,
            start_frame,
            fade_frames,
            rendered_frame,
            progress,
        })
    }

    /// The timed A/V crossfade: `to` ramps to 1, `from` ramps to 0. The
    /// program mute is a separate multiplier and is never touched here.
    pub fn fade_slots(&self, from: Option<SlotId>, to: SlotId, secs: f32) {
        let mut s = self.state.lock().unwrap();
        if let Some(scheduled) = s.scheduled_video.take() {
            if scheduled.started {
                // The audio clock owns a started transition. Legacy UI code
                // may observe `Started` and call this immediate helper; do
                // not restart its ramp or destroy its completion snapshot.
                s.scheduled_video = Some(scheduled);
                return;
            }
            self.transition.publish_phase(
                VideoTransitionPhase::Cancelled,
                self.device_frames.load(Ordering::Acquire),
            );
            if scheduled.to != to {
                let bus = &mut s.video[scheduled.to.index()];
                bus.paused = true;
                bus.gain = Ramp::at(0.0);
            }
        }
        let secs = secs.max(SLEW_SECS);
        if let Some(from) = from {
            s.video[from.index()].gain.slew(0.0, secs);
        }
        s.video[to.index()].gain.slew(1.0, secs);
    }

    /// Mute/unmute the whole video program (video-slot audio only). A ramp
    /// on the summed bus: per-slot fade targets are preserved exactly, so
    /// an unmute after any sequence of cues restores the intended level.
    pub fn set_video_muted(&self, muted: bool) {
        self.state
            .lock()
            .unwrap()
            .video_mute
            .slew(if muted { 0.0 } else { 1.0 }, SLEW_SECS * 4.0);
    }

    // ---- decks -------------------------------------------------------------

    /// Install a decoded track, paused at zero.
    pub fn install_deck(&self, deck: DeckId, pcm: Arc<TrackPcm>) {
        let mut s = self.state.lock().unwrap();
        let d = &mut s.decks[deck.index()];
        d.pcm = Some(pcm);
        d.cursor_fp = 0;
        d.playing = false;
        d.ended = false;
    }

    pub fn set_deck_playing(&self, deck: DeckId, playing: bool) {
        let mut s = self.state.lock().unwrap();
        let d = &mut s.decks[deck.index()];
        if playing {
            // Playing from the end restarts.
            if let Some(pcm) = &d.pcm {
                if d.cursor_fp >= (pcm.frames.len() as u64) << 32 {
                    d.cursor_fp = 0;
                }
            }
            d.ended = false;
        }
        d.playing = playing;
    }

    pub fn seek_deck_fraction(&self, deck: DeckId, fraction: f64) {
        let mut s = self.state.lock().unwrap();
        let d = &mut s.decks[deck.index()];
        if let Some(pcm) = &d.pcm {
            let frame = (fraction.clamp(0.0, 1.0) * pcm.frames.len() as f64) as u64;
            d.cursor_fp = frame.min(pcm.frames.len() as u64) << 32;
            d.ended = false;
        }
    }

    pub fn set_deck_loop(&self, deck: DeckId, loop_on: bool) {
        self.state.lock().unwrap().decks[deck.index()].loop_on = loop_on;
    }

    pub fn set_deck_mute(&self, deck: DeckId, muted: bool) {
        self.state.lock().unwrap().decks[deck.index()]
            .mute
            .slew(if muted { 0.0 } else { 1.0 }, SLEW_SECS);
    }

    pub fn set_deck_gain(&self, deck: DeckId, gain: f32) {
        self.state.lock().unwrap().decks[deck.index()].gain.slew(gain, SLEW_SECS);
    }

    pub fn swap_decks(&self) {
        self.state.lock().unwrap().decks.swap(0, 1);
    }

    pub fn set_crossfader(&self, position: f32) {
        self.state.lock().unwrap().fader.slew(position.clamp(0.0, 1.0), SLEW_SECS);
    }

    pub fn fade_crossfader(&self, position: f32, secs: f32) {
        self.state.lock().unwrap().fader.slew(position.clamp(0.0, 1.0), secs.max(SLEW_SECS));
    }

    pub fn set_curve(&self, curve: FadeCurve) {
        self.state.lock().unwrap().curve = curve;
    }

    pub fn set_master(&self, gain: f32) {
        self.state.lock().unwrap().master.slew(gain.clamp(0.0, 1.2), SLEW_SECS);
    }

    /// `(position_secs, duration_secs, playing)` from the device clock.
    pub fn deck_position(&self, deck: DeckId) -> (f64, f64, bool) {
        let s = self.state.lock().unwrap();
        let d = &s.decks[deck.index()];
        match &d.pcm {
            None => (0.0, 0.0, false),
            Some(pcm) => {
                let pos =
                    (d.cursor_fp as f64 / FP_ONE as f64) / pcm.sample_rate.max(1) as f64;
                (pos, pcm.seconds(), d.playing)
            }
        }
    }

    /// Decks that ran off their end (loop off) since the last drain.
    pub fn drain_ended_decks(&self) -> Vec<DeckId> {
        std::mem::take(&mut self.state.lock().unwrap().ended_decks)
    }

    // ---- sfx voices ---------------------------------------------------------

    pub fn start_voice(&self, alloc: VoiceAlloc, pcm: Arc<TrackPcm>) {
        let mut s = self.state.lock().unwrap();
        s.sfx.push(SfxVoice {
            id: alloc.id,
            pad: alloc.pad,
            pcm,
            cursor_fp: 0,
            loop_on: alloc.loop_on,
            gain: Ramp::at(alloc.gain),
            done: false,
        });
    }

    pub fn stop_voice(&self, id: VoiceId) {
        let mut s = self.state.lock().unwrap();
        // Fast declick: a stopped voice ramps out over one slew and is
        // reaped by the render pass.
        for v in s.sfx.iter_mut().filter(|v| v.id == id) {
            v.loop_on = false;
            v.gain.slew(0.0, SLEW_SECS);
            v.done = true;
        }
    }

    pub fn set_pad_voices_gain(&self, pad: PadKey, gain: f32) {
        let mut s = self.state.lock().unwrap();
        for v in s.sfx.iter_mut().filter(|v| v.pad == pad && !v.done) {
            v.gain.slew(gain, SLEW_SECS);
        }
    }

    /// Voices that finished naturally (ran off the end, loop off).
    pub fn drain_ended_voices(&self) -> Vec<VoiceId> {
        std::mem::take(&mut self.state.lock().unwrap().ended_voices)
    }

    /// Current peak meters: `[master, video, deck_a, deck_b, sfx]`.
    pub fn meters(&self) -> [f32; 5] {
        let mut out = [0.0f32; 5];
        for (i, m) in self.meters.iter().enumerate() {
            out[i] = f32::from_bits(m.load(Ordering::Relaxed));
        }
        out
    }

    // ---- the device callback ------------------------------------------------

    /// Mix one device buffer. The buffer must already be zeroed; on lock
    /// contention it stays silent rather than ever blocking the device.
    pub fn render(&self, device_rate: f64, output: &mut AudioBuffer) {
        if device_rate <= 0.0 {
            return;
        }
        let frames = output.frame_count();
        self.device_rate_bits.store(device_rate.to_bits(), Ordering::Release);
        // Advance the physical device clock even when the realtime state is
        // contended and this buffer must remain silent. That lets a later
        // callback mark an exact deadline Missed instead of firing it late.
        let buffer_start = self.device_frames.fetch_add(frames as u64, Ordering::AcqRel);
        let Ok(mut s) = self.state.try_lock() else { return };
        let s = &mut *s;
        let rate = device_rate as f32;
        let channels = output.channel_count();
        let mut peaks = [0.0f32; 5];
        s.rendered_frames = buffer_start;

        if let Some(scheduled) = s.scheduled_video {
            if !scheduled.started && scheduled.target_frame < buffer_start {
                s.scheduled_video = None;
                let destination = &mut s.video[scheduled.to.index()];
                destination.paused = true;
                destination.gain = Ramp::at(0.0);
                self.transition
                    .publish_phase(VideoTransitionPhase::Missed, buffer_start);
            } else if scheduled.started {
                let end = scheduled
                    .target_frame
                    .saturating_add(scheduled.fade_frames.max(1));
                if buffer_start >= end {
                    s.scheduled_video = None;
                    if let Some(from) = scheduled.from {
                        s.video[from.index()].gain = Ramp::at(0.0);
                    }
                    s.video[scheduled.to.index()].gain = Ramp::at(1.0);
                    self.transition
                        .publish_phase(VideoTransitionPhase::Completed, buffer_start);
                } else if buffer_start > scheduled.target_frame && scheduled.fade_frames > 0 {
                    // Catch a running fade up to the physical device clock
                    // after one or more silent contention buffers.
                    let elapsed = buffer_start - scheduled.target_frame;
                    let progress = (elapsed as f32 / scheduled.fade_frames as f32).clamp(0.0, 1.0);
                    let remaining = scheduled.fade_frames.saturating_sub(elapsed).max(1);
                    let secs = remaining as f32 / rate.max(1.0);
                    if let Some(from) = scheduled.from {
                        s.video[from.index()].gain = Ramp::at(1.0 - progress);
                        s.video[from.index()].gain.slew(0.0, secs);
                    }
                    s.video[scheduled.to.index()].gain = Ramp::at(progress);
                    s.video[scheduled.to.index()].gain.slew(1.0, secs);
                }
            }
        }

        for frame in 0..frames {
            let output_frame = buffer_start.saturating_add(frame as u64);
            let starts_now = s.scheduled_video.is_some_and(|scheduled| {
                !scheduled.started && output_frame >= scheduled.target_frame
            });
            if starts_now {
                let mut scheduled = s.scheduled_video.expect("checked above");
                scheduled.started = true;
                s.scheduled_video = Some(scheduled);
                let fade_secs = if scheduled.fade_frames == 0 {
                    0.0
                } else {
                    scheduled.fade_frames as f32 / rate.max(1.0)
                };
                if let Some(from) = scheduled.from {
                    if scheduled.fade_frames == 0 {
                        s.video[from.index()].gain = Ramp::at(0.0);
                    } else {
                        s.video[from.index()].gain.slew(0.0, fade_secs);
                    }
                }
                let destination = &mut s.video[scheduled.to.index()];
                destination.paused = false;
                if scheduled.fade_frames == 0 {
                    destination.gain = Ramp::at(1.0);
                } else {
                    destination.gain.slew(1.0, fade_secs);
                }
                self.transition.publish_phase(VideoTransitionPhase::Started, output_frame);
            }

            // Video buses (summed, then the orthogonal program mute).
            let mut video = (0.0f32, 0.0f32);
            for bus in s.video.iter_mut() {
                let gain = bus.gain.tick(rate);
                if bus.paused || bus.queue.len() < 2 || bus.source_rate <= 0.0 {
                    continue;
                }
                let index = bus.cursor as usize;
                if index + 1 >= bus.queue.len() {
                    continue;
                }
                let fraction = (bus.cursor - index as f64) as f32;
                let (al, ar) = bus.queue[index];
                let (bl, br) = bus.queue[index + 1];
                video.0 += (al + (bl - al) * fraction) * gain;
                video.1 += (ar + (br - ar) * fraction) * gain;
                bus.cursor += (bus.source_rate / device_rate) * bus.playback_rate;
            }
            let program_mute = s.video_mute.tick(rate);
            video.0 *= program_mute;
            video.1 *= program_mute;

            // Decks under the crossfader.
            let pos = s.fader.tick(rate);
            let fader = crossfader_gains(pos, s.curve);
            let mut deck_out = [(0.0f32, 0.0f32); 2];
            for (i, d) in s.decks.iter_mut().enumerate() {
                let gain = d.gain.tick(rate) * d.mute.tick(rate);
                let side = if i == 0 { fader.0 } else { fader.1 };
                let Some(pcm) = &d.pcm else { continue };
                if !d.playing || d.ended {
                    continue;
                }
                let end = (pcm.frames.len() as u64) << 32;
                if d.cursor_fp >= end {
                    if d.loop_on {
                        d.cursor_fp = 0;
                    } else {
                        d.playing = false;
                        d.ended = true;
                        s.ended_decks.push(if i == 0 { DeckId::A } else { DeckId::B });
                        continue;
                    }
                }
                let index = (d.cursor_fp >> 32) as usize;
                let fraction = (d.cursor_fp & (FP_ONE - 1)) as f32 / FP_ONE as f32;
                let next = (index + 1).min(pcm.frames.len() - 1);
                let a = pcm.frames[index];
                let b = pcm.frames[next];
                let l = (a[0] as f32 + (b[0] as f32 - a[0] as f32) * fraction) / 32768.0;
                let r = (a[1] as f32 + (b[1] as f32 - a[1] as f32) * fraction) / 32768.0;
                deck_out[i] = (l * gain * side, r * gain * side);
                let step = ((pcm.sample_rate as f64 / device_rate) * FP_ONE as f64) as u64;
                d.cursor_fp = d.cursor_fp.saturating_add(step.max(1));
            }

            // SFX voices.
            let mut sfx = (0.0f32, 0.0f32);
            for v in s.sfx.iter_mut() {
                let gain = v.gain.tick(rate);
                let end = (v.pcm.frames.len() as u64) << 32;
                if v.cursor_fp >= end {
                    if v.loop_on {
                        v.cursor_fp = 0;
                    } else {
                        if !v.done {
                            v.done = true;
                            s.ended_voices.push(v.id);
                        }
                        continue;
                    }
                }
                // A stopped voice that finished its ramp-out is silent.
                if v.done && gain <= 0.0005 {
                    continue;
                }
                let index = (v.cursor_fp >> 32) as usize;
                let fraction = (v.cursor_fp & (FP_ONE - 1)) as f32 / FP_ONE as f32;
                let next = (index + 1).min(v.pcm.frames.len() - 1);
                let a = v.pcm.frames[index];
                let b = v.pcm.frames[next];
                sfx.0 += (a[0] as f32 + (b[0] as f32 - a[0] as f32) * fraction) / 32768.0 * gain;
                sfx.1 += (a[1] as f32 + (b[1] as f32 - a[1] as f32) * fraction) / 32768.0 * gain;
                let step = ((v.pcm.sample_rate as f64 / device_rate) * FP_ONE as f64) as u64;
                v.cursor_fp = v.cursor_fp.saturating_add(step.max(1));
            }

            let master = s.master.tick(rate);
            let l = ((video.0 + deck_out[0].0 + deck_out[1].0 + sfx.0) * master)
                .clamp(-CLAMP, CLAMP);
            let r = ((video.1 + deck_out[0].1 + deck_out[1].1 + sfx.1) * master)
                .clamp(-CLAMP, CLAMP);
            for channel in 0..channels {
                output.channel_mut(channel)[frame] += if channel == 0 { l } else { r };
            }
            peaks[METER_MASTER] = peaks[METER_MASTER].max(l.abs()).max(r.abs());
            peaks[METER_VIDEO] = peaks[METER_VIDEO].max(video.0.abs()).max(video.1.abs());
            peaks[METER_DECK_A] =
                peaks[METER_DECK_A].max(deck_out[0].0.abs()).max(deck_out[0].1.abs());
            peaks[METER_DECK_B] =
                peaks[METER_DECK_B].max(deck_out[1].0.abs()).max(deck_out[1].1.abs());
            peaks[METER_SFX] = peaks[METER_SFX].max(sfx.0.abs()).max(sfx.1.abs());

            s.rendered_frames = output_frame.saturating_add(1);
            let completes_now = s.scheduled_video.is_some_and(|scheduled| {
                scheduled.started
                    && s.rendered_frames
                        >= scheduled.target_frame.saturating_add(scheduled.fade_frames.max(1))
            });
            if completes_now {
                let scheduled = s.scheduled_video.take().expect("checked above");
                if let Some(from) = scheduled.from {
                    s.video[from.index()].gain = Ramp::at(0.0);
                }
                s.video[scheduled.to.index()].gain = Ramp::at(1.0);
                self.transition
                    .publish_phase(VideoTransitionPhase::Completed, s.rendered_frames);
            }
        }

        // Reap: consumed video queue frames + fully faded stopped voices.
        for bus in s.video.iter_mut() {
            let consumed = bus.cursor as usize;
            if consumed > 0 {
                bus.queue.drain(..consumed.min(bus.queue.len()));
                bus.cursor -= consumed as f64;
            }
        }
        s.sfx.retain(|v| {
            let ran_off = v.cursor_fp >= (v.pcm.frames.len() as u64) << 32 && !v.loop_on;
            let faded_out = v.done && v.gain.current <= 0.0005 && v.gain.target == 0.0;
            !(ran_off || faded_out)
        });

        for (i, p) in peaks.iter().enumerate() {
            self.meters[i].store(p.to_bits(), Ordering::Relaxed);
        }
        self.transition
            .publish_rendered_frame(self.device_frames.load(Ordering::Acquire));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn const_pcm(value: i16, frames: usize, rate: u32) -> Arc<TrackPcm> {
        Arc::new(TrackPcm { frames: vec![[value, value]; frames], sample_rate: rate })
    }

    fn render(mixer: &Mixer, rate: f64, frames: usize) -> AudioBuffer {
        let mut buffer = AudioBuffer::new_with_size(frames, 2);
        mixer.render(rate, &mut buffer);
        buffer
    }

    #[test]
    fn deck_under_equal_power_midpoint_is_root_half() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        // Settle master ramp.
        render(&mixer, 48_000.0, 64);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 48_000, 48_000)); // 0.5 amplitude
        mixer.set_deck_playing(DeckId::A, true);
        mixer.set_crossfader(0.5);
        // Let ramps settle, then measure.
        render(&mixer, 48_000.0, 2048);
        let out = render(&mixer, 48_000.0, 64);
        let expected = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
        let got = out.channel(0)[32];
        assert!(
            (got - expected).abs() < 0.01,
            "expected ~{expected}, got {got}"
        );
        // Full A: unattenuated; full B: silent.
        mixer.set_crossfader(0.0);
        render(&mixer, 48_000.0, 2048);
        let out = render(&mixer, 48_000.0, 64);
        assert!((out.channel(0)[32] - 0.5).abs() < 0.01);
        mixer.set_crossfader(1.0);
        render(&mixer, 48_000.0, 2048);
        let out = render(&mixer, 48_000.0, 64);
        assert!(out.channel(0)[32].abs() < 0.01);
    }

    #[test]
    fn deck_end_reports_once_and_loop_wraps() {
        let mixer = Mixer::new();
        mixer.install_deck(DeckId::A, const_pcm(1000, 100, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 256);
        assert_eq!(mixer.drain_ended_decks(), vec![DeckId::A]);
        assert!(mixer.drain_ended_decks().is_empty());
        let (_pos, _dur, playing) = mixer.deck_position(DeckId::A);
        assert!(!playing);
        // Looping deck never ends.
        mixer.install_deck(DeckId::B, const_pcm(1000, 100, 48_000));
        mixer.set_deck_loop(DeckId::B, true);
        mixer.set_deck_playing(DeckId::B, true);
        render(&mixer, 48_000.0, 1024);
        assert!(mixer.drain_ended_decks().is_empty());
        let (_, _, playing) = mixer.deck_position(DeckId::B);
        assert!(playing);
    }

    #[test]
    fn sfx_voices_overlap_and_finished_voices_are_reaped() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        // Let the master ramp settle before anything audible starts.
        render(&mixer, 48_000.0, 2048);
        let pcm = const_pcm(8192, 4_800, 48_000); // 0.25 amplitude, 100ms
        for id in 1..=3 {
            mixer.start_voice(
                VoiceAlloc {
                    id,
                    pad: PadKey::from_bytes([1; 16]),
                    choke_group: 0,
                    loop_on: false,
                    gain: 1.0,
                    started_ms: 0,
                },
                pcm.clone(),
            );
        }
        let out = render(&mixer, 48_000.0, 64);
        // Three overlapping voices sum: 3 × 0.25 × master(1.0).
        assert!((out.channel(0)[32] - 0.75).abs() < 0.02, "{}", out.channel(0)[32]);
        // Run to the end: all three report ended and are reaped.
        render(&mixer, 48_000.0, 4_800);
        let mut ended = mixer.drain_ended_voices();
        ended.sort();
        assert_eq!(ended, vec![1, 2, 3]);
        let out = render(&mixer, 48_000.0, 64);
        assert!(out.channel(0)[32].abs() < 1e-6);
    }

    #[test]
    fn video_slot_fade_reaches_targets_and_close_silences() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        render(&mixer, 48_000.0, 64);
        mixer.open_slot(SlotId::A);
        // 0.5 amplitude source at device rate.
        let samples: Vec<i16> = vec![16_384; 2 * 48_000];
        assert!(mixer.push_slot_audio(SlotId::A, &samples, 2, 48_000));
        mixer.fade_slots(None, SlotId::A, 0.01);
        render(&mixer, 48_000.0, 4096); // fade settles
        let out = render(&mixer, 48_000.0, 64);
        assert!((out.channel(0)[32] - 0.5).abs() < 0.02, "{}", out.channel(0)[32]);
        // Closing flushes + refuses further pushes.
        mixer.close_slot(SlotId::A);
        assert!(!mixer.push_slot_audio(SlotId::A, &samples, 2, 48_000));
        let out = render(&mixer, 48_000.0, 64);
        assert!(out.channel(0)[32].abs() < 1e-6);
    }

    #[test]
    fn video_mute_roundtrip_restores_pre_mute_level_exactly() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        render(&mixer, 48_000.0, 64);
        mixer.open_slot(SlotId::A);
        let samples: Vec<i16> = vec![16_384; 2 * 96_000]; // 0.5 amplitude
        assert!(mixer.push_slot_audio(SlotId::A, &samples, 2, 48_000));
        mixer.fade_slots(None, SlotId::A, 0.01);
        render(&mixer, 48_000.0, 4096);
        let before = render(&mixer, 48_000.0, 64).channel(0)[32];
        assert!((before - 0.5).abs() < 0.02, "{before}");
        // Mute → silent; unmute → the EXACT pre-mute level (the historical
        // bug: unmute stayed silent because mute clobbered fade targets).
        mixer.set_video_muted(true);
        render(&mixer, 48_000.0, 8192);
        let muted = render(&mixer, 48_000.0, 64).channel(0)[32];
        assert!(muted.abs() < 1e-3, "muted program must be silent, got {muted}");
        mixer.set_video_muted(false);
        render(&mixer, 48_000.0, 8192);
        let after = render(&mixer, 48_000.0, 64).channel(0)[32];
        assert!(
            (after - before).abs() < 0.02,
            "unmute must restore the pre-mute level: before {before}, after {after}"
        );
    }

    #[test]
    fn crossfade_completes_under_mute_and_unmute_hears_the_new_slot() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        render(&mixer, 48_000.0, 64);
        // Slot A live, then mute, then crossfade to slot B WHILE muted.
        mixer.open_slot(SlotId::A);
        assert!(mixer.push_slot_audio(SlotId::A, &vec![16_384i16; 2 * 96_000], 2, 48_000));
        mixer.fade_slots(None, SlotId::A, 0.01);
        render(&mixer, 48_000.0, 4096);
        mixer.set_video_muted(true);
        render(&mixer, 48_000.0, 8192);
        mixer.open_slot(SlotId::B);
        assert!(mixer.push_slot_audio(SlotId::B, &vec![8_192i16; 2 * 96_000], 2, 48_000)); // 0.25
        mixer.fade_slots(Some(SlotId::A), SlotId::B, 0.01);
        render(&mixer, 48_000.0, 8192); // fade completes silently
        let muted = render(&mixer, 48_000.0, 64).channel(0)[32];
        assert!(muted.abs() < 1e-3, "still muted, got {muted}");
        // Unmute: the NEW slot's level, not silence and not slot A.
        mixer.set_video_muted(false);
        render(&mixer, 48_000.0, 8192);
        let after = render(&mixer, 48_000.0, 64).channel(0)[32];
        assert!((after - 0.25).abs() < 0.02, "expected slot B at 0.25, got {after}");
    }

    #[test]
    fn paused_slot_bus_consumes_nothing_until_unpaused() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.open_slot(SlotId::A);
        // True preroll: bus paused, gain up — still silent, queue intact.
        mixer.set_slot_paused(SlotId::A, true);
        assert!(mixer.push_slot_audio(SlotId::A, &vec![16_384i16; 2 * 4_800], 2, 48_000));
        mixer.fade_slots(None, SlotId::A, 0.01);
        let before = mixer.slot_buffered_secs(SlotId::A);
        render(&mixer, 48_000.0, 2048);
        let out = render(&mixer, 48_000.0, 64);
        assert!(out.channel(0)[32].abs() < 1e-6, "paused slot must be silent");
        assert!(
            (mixer.slot_buffered_secs(SlotId::A) - before).abs() < 1e-6,
            "paused slot must not consume its queue"
        );
        // Unpause: audio flows from sample zero.
        mixer.set_slot_paused(SlotId::A, false);
        render(&mixer, 48_000.0, 512);
        let out = render(&mixer, 48_000.0, 64);
        assert!(out.channel(0)[32].abs() > 0.1, "unpaused slot must be audible");
    }

    #[test]
    fn deck_mute_and_gain_are_click_free_ramps_to_target() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 96_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        mixer.set_crossfader(0.0);
        render(&mixer, 48_000.0, 4096);
        mixer.set_deck_mute(DeckId::A, true);
        render(&mixer, 48_000.0, 4096);
        let out = render(&mixer, 48_000.0, 64);
        assert!(out.channel(0)[32].abs() < 1e-3, "muted deck must be silent");
        mixer.set_deck_mute(DeckId::A, false);
        mixer.set_deck_gain(DeckId::A, 0.5);
        render(&mixer, 48_000.0, 4096);
        let out = render(&mixer, 48_000.0, 64);
        assert!((out.channel(0)[32] - 0.25).abs() < 0.02, "{}", out.channel(0)[32]);
    }

    #[test]
    fn scheduled_transition_starts_on_exact_sample_inside_buffer() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        render(&mixer, 48_000.0, 2048); // settle master
        mixer.open_slot(SlotId::A);
        assert!(mixer.push_slot_audio(SlotId::A, &vec![16_384; 2 * 256], 2, 48_000));
        let target = mixer.rendered_output_frames() + 5;
        mixer
            .schedule_video_transition_at(41, None, SlotId::A, target, 1)
            .unwrap();

        let out = render(&mixer, 48_000.0, 12);
        assert!(out.channel(0)[..5].iter().all(|sample| sample.abs() < 1e-7));
        assert!((out.channel(0)[5] - 0.5).abs() < 0.02, "transition was not sample exact");
        let snapshot = mixer.video_transition_snapshot().unwrap();
        assert_eq!(snapshot.id, 41);
        assert_eq!(snapshot.start_frame, Some(target));
        assert_eq!(snapshot.phase, VideoTransitionPhase::Completed);
        assert_eq!(snapshot.progress, 1.0);
    }

    #[test]
    fn armed_destination_queue_is_not_consumed_before_target() {
        let mixer = Mixer::new();
        mixer.open_slot(SlotId::B);
        assert!(mixer.push_slot_audio(SlotId::B, &vec![8_192; 2 * 4_800], 2, 48_000));
        let before = mixer.slot_buffered_secs(SlotId::B);
        mixer
            .schedule_video_transition_after(9, None, SlotId::B, 1_000, 128)
            .unwrap();
        render(&mixer, 48_000.0, 512);
        assert_eq!(mixer.video_transition_snapshot().unwrap().phase, VideoTransitionPhase::Armed);
        assert!((mixer.slot_buffered_secs(SlotId::B) - before).abs() < 1e-9);
    }

    #[test]
    fn scheduled_transition_can_cancel_and_rearm_before_start() {
        let mixer = Mixer::new();
        mixer.open_slot(SlotId::A);
        mixer
            .schedule_video_transition_after(1, None, SlotId::A, 1_000, 64)
            .unwrap();
        assert!(mixer.cancel_video_transition(1));
        assert!(!mixer.cancel_video_transition(1));
        assert_eq!(
            mixer.video_transition_snapshot().unwrap().phase,
            VideoTransitionPhase::Cancelled
        );
        let target = mixer
            .schedule_video_transition_after(2, None, SlotId::A, 8, 4)
            .unwrap();
        let armed = mixer.video_transition_snapshot().unwrap();
        assert_eq!((armed.id, armed.phase, armed.target_frame), (2, VideoTransitionPhase::Armed, target));
        render(&mixer, 48_000.0, 16);
        assert_eq!(
            mixer.video_transition_snapshot().unwrap().phase,
            VideoTransitionPhase::Completed
        );
    }

    #[test]
    fn contended_target_is_reported_missed_and_never_started_late() {
        let mixer = Mixer::new();
        mixer.open_slot(SlotId::A);
        assert!(mixer.push_slot_audio(SlotId::A, &vec![16_384; 2 * 512], 2, 48_000));
        let before = mixer.slot_buffered_secs(SlotId::A);
        mixer
            .schedule_video_transition_after(88, None, SlotId::A, 4, 16)
            .unwrap();
        let guard = mixer.state.lock().unwrap();
        let silent = render(&mixer, 48_000.0, 8);
        assert!(silent.channel(0).iter().all(|sample| sample.abs() < 1e-7));
        drop(guard);
        render(&mixer, 48_000.0, 1);
        let snapshot = mixer.video_transition_snapshot().unwrap();
        assert_eq!(snapshot.phase, VideoTransitionPhase::Missed);
        assert!((mixer.slot_buffered_secs(SlotId::A) - before).abs() < 1e-9);
    }

    #[test]
    fn closing_just_started_destination_restores_previous_program() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        render(&mixer, 48_000.0, 2_048);
        mixer.open_slot(SlotId::A);
        assert!(mixer.push_slot_audio(SlotId::A, &vec![16_384; 2 * 4_000], 2, 48_000));
        mixer.fade_slots(None, SlotId::A, 0.008);
        render(&mixer, 48_000.0, 1_000);
        mixer.open_slot(SlotId::B);
        assert!(mixer.push_slot_audio(SlotId::B, &vec![8_192; 2 * 4_000], 2, 48_000));
        mixer
            .schedule_video_transition_after(99, Some(SlotId::A), SlotId::B, 0, 1_000)
            .unwrap();
        render(&mixer, 48_000.0, 1);
        assert_eq!(
            mixer.video_transition_snapshot().unwrap().phase,
            VideoTransitionPhase::Started
        );
        mixer.close_slot(SlotId::B);
        assert_eq!(
            mixer.video_transition_snapshot().unwrap().phase,
            VideoTransitionPhase::Cancelled
        );
        let out = render(&mixer, 48_000.0, 16);
        assert!((out.channel(0)[8] - 0.5).abs() < 0.02);
    }

    #[test]
    fn scheduling_video_does_not_perturb_deck_or_sfx_cursors() {
        fn populated() -> Mixer {
            let mixer = Mixer::new();
            mixer.install_deck(DeckId::A, const_pcm(4_000, 4_000, 48_000));
            mixer.set_deck_playing(DeckId::A, true);
            mixer.start_voice(
                VoiceAlloc {
                    id: 77,
                    pad: PadKey::from_bytes([7; 16]),
                    choke_group: 0,
                    loop_on: false,
                    gain: 0.5,
                    started_ms: 0,
                },
                const_pcm(2_000, 4_000, 48_000),
            );
            mixer
        }

        let control = populated();
        let scheduled = populated();
        scheduled.open_slot(SlotId::A);
        scheduled
            .schedule_video_transition_after(55, None, SlotId::A, 31, 17)
            .unwrap();
        render(&control, 48_000.0, 256);
        render(&scheduled, 48_000.0, 256);
        let control = control.state.lock().unwrap();
        let scheduled = scheduled.state.lock().unwrap();
        assert_eq!(scheduled.decks[0].cursor_fp, control.decks[0].cursor_fp);
        assert_eq!(scheduled.sfx[0].cursor_fp, control.sfx[0].cursor_fp);
    }

    #[test]
    fn video_playback_rate_is_capped_and_isolated_from_other_voices() {
        let mixer = Mixer::new();
        mixer.open_slot(SlotId::A);
        assert_eq!(
            mixer.set_slot_playback_rate(SlotId::A, 10.0),
            MAX_VIDEO_PLAYBACK_RATE
        );
        assert_eq!(
            mixer.set_slot_playback_rate(SlotId::B, 0.1),
            MIN_VIDEO_PLAYBACK_RATE
        );
        mixer.install_deck(DeckId::A, const_pcm(1_000, 4_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        assert!(mixer.push_slot_audio(SlotId::A, &vec![1_000; 2 * 4_000], 2, 48_000));
        mixer.fade_slots(None, SlotId::A, 0.008);
        render(&mixer, 48_000.0, 100);
        let state = mixer.state.lock().unwrap();
        assert_eq!(state.decks[0].cursor_fp, 100 * FP_ONE);
        let consumed = 4_000 - state.video[0].queue.len();
        assert!((107..=108).contains(&consumed));
        assert!((consumed as f64 + state.video[0].cursor - 108.0).abs() < 1e-6);
    }
}
