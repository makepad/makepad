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
use crate::decks::{crossfader_gains, DeckId, FadeCurve, ScratchMotion};
use crate::music_dsp::{
    DeckEq, FrameSource, ParamRamp, RateReader, ScratchRamp, Stretcher, STEM_COUNT,
    STRETCH_BYPASS_EPSILON, WSOLA_WINDOW,
};
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
/// Explicit beat-sync (N beats per loop) may ask for wide rates; the
/// automatic loop-fit keeps its own ≤8% guard (`fit_loop_to_grid`).
pub const MIN_VIDEO_PLAYBACK_RATE: f64 = 0.25;
pub const MAX_VIDEO_PLAYBACK_RATE: f64 = 4.0;

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

    #[inline]
    fn frame_f32(&self, index: usize) -> [f32; 2] {
        match self.frames.get(index) {
            Some(frame) => [frame[0] as f32 / 32768.0, frame[1] as f32 / 32768.0],
            None => [0.0, 0.0],
        }
    }
}

/// A separated track: four stem lanes on the SAME timeline as the mixed
/// file, delivered in fixed chunks as the separator streams them.
///
/// A chunk that has not arrived is not silence — the deck falls back to the
/// mixed file there, so playback is never interrupted by separation and the
/// knobs simply become live as the track is covered.
pub struct TrackStems {
    /// Track frames per chunk.
    pub chunk_frames: usize,
    /// Per lane (vocals, drums, bass, other), one slot per chunk.
    pub lanes: [Vec<Option<Arc<Vec<[i16; 2]>>>>; STEM_COUNT],
}

impl TrackStems {
    pub fn new(chunk_frames: usize, chunk_count: usize) -> TrackStems {
        TrackStems {
            chunk_frames: chunk_frames.max(1),
            lanes: [
                vec![None; chunk_count],
                vec![None; chunk_count],
                vec![None; chunk_count],
                vec![None; chunk_count],
            ],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lanes.iter().all(|lane| lane.iter().all(Option::is_none))
    }

    /// Whether the chunk covering `frame` has been separated.
    pub fn covers(&self, frame: usize) -> bool {
        let index = frame / self.chunk_frames;
        self.lanes[0].get(index).is_some_and(Option::is_some)
    }

    /// Fraction of the track separated so far.
    pub fn coverage(&self) -> f32 {
        let total = self.lanes[0].len();
        if total == 0 {
            return 0.0;
        }
        let done = self.lanes[0].iter().filter(|slot| slot.is_some()).count();
        done as f32 / total as f32
    }
}

/// What a deck's DSP chain reads from: the full mix, or the stem lanes
/// summed under their current gains.
struct DeckSource<'a> {
    pcm: &'a TrackPcm,
    stems: Option<&'a TrackStems>,
    stem_gain: [f32; STEM_COUNT],
}

impl FrameSource for DeckSource<'_> {
    #[inline]
    fn frame_count(&self) -> usize {
        self.pcm.frames.len()
    }

    #[inline]
    fn frame(&self, index: usize) -> [f32; 2] {
        let Some(stems) = self.stems else {
            return self.pcm.frame_f32(index);
        };
        let chunk = index / stems.chunk_frames;
        let offset = index - chunk * stems.chunk_frames;
        let mut out = [0.0f32; 2];
        let mut separated = false;
        for (lane, gain) in stems.lanes.iter().zip(self.stem_gain) {
            let Some(Some(block)) = lane.get(chunk) else { continue };
            separated = true;
            if gain <= 0.0 {
                continue;
            }
            let Some(frame) = block.get(offset) else { continue };
            out[0] += frame[0] as f32 / 32768.0 * gain;
            out[1] += frame[1] as f32 / 32768.0 * gain;
        }
        // Nothing separated here yet: the mixed file plays, as it did
        // before separation existed.
        if !separated {
            return self.pcm.frame_f32(index);
        }
        out
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
    stems: Option<Arc<TrackStems>>,
    /// Playhead in SOURCE frames. Fractional, and free to run backwards
    /// under a hand on the waveform.
    pos: f64,
    playing: bool,
    loop_on: bool,
    gain: Ramp,
    mute: Ramp,
    ended: bool,
    /// Tempo multiplier from the pitch slider / sync.
    rate: ParamRamp,
    /// Hold the key when the tempo moves.
    keylock: bool,
    scratch: ScratchRamp,
    /// True while the time stretcher owns the playhead.
    stretching: bool,
    stretch: Box<Stretcher>,
    reader: RateReader,
    eq: DeckEq,
    stem_gain: [ParamRamp; STEM_COUNT],
}

impl DeckVoice {
    fn new() -> DeckVoice {
        DeckVoice {
            pcm: None,
            stems: None,
            pos: 0.0,
            playing: false,
            loop_on: false,
            gain: Ramp::at(1.0),
            mute: Ramp::at(1.0),
            ended: false,
            rate: ParamRamp::at(1.0),
            keylock: true,
            scratch: ScratchRamp::default(),
            stretching: false,
            stretch: Box::new(Stretcher::new()),
            reader: RateReader::default(),
            eq: DeckEq::new(48_000.0),
            stem_gain: [ParamRamp::at(1.0); STEM_COUNT],
        }
    }

    fn frame_count(&self) -> usize {
        self.pcm.as_ref().map(|pcm| pcm.frames.len()).unwrap_or(0)
    }

    /// Move the playhead and drop every bit of streaming state that was
    /// tied to the old position.
    fn seek_frames(&mut self, frames: f64) {
        let len = self.frame_count() as f64;
        self.pos = frames.clamp(0.0, len);
        self.stretch.reset_to(self.pos);
        self.reader.reset();
        self.ended = false;
    }

    /// Where the playhead really is, whichever path is driving it.
    fn playhead_frames(&self) -> f64 {
        if self.stretching {
            self.stretch.position()
        } else {
            self.pos
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
    /// Pre-fader deck peaks, for the channel VU meters.
    deck_meters: Arc<[AtomicU32; 2]>,
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
            deck_meters: Arc::new([AtomicU32::new(0), AtomicU32::new(0)]),
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
    /// Operator crossfader: equal-power A/B bus gains, slewed over a few ms
    /// so a fast hand never zippers. Ignored while a scheduled transition
    /// owns the gains (it lands them itself).
    pub fn set_video_mix(&self, mix: f32) {
        let mut s = self.state.lock().unwrap();
        if s.scheduled_video.is_some() {
            return;
        }
        let (a, b) = crate::decks::crossfader_gains(mix, crate::decks::FadeCurve::EqualPower);
        s.video[0].gain.slew(a, 0.015);
        s.video[1].gain.slew(b, 0.015);
    }

    pub fn set_video_muted(&self, muted: bool) {
        self.state
            .lock()
            .unwrap()
            .video_mute
            .slew(if muted { 0.0 } else { 1.0 }, SLEW_SECS * 4.0);
    }

    // ---- decks -------------------------------------------------------------

    /// Install a decoded track, paused at zero. Any stems from a previous
    /// track go with it; the tone chain is reset but its settings stand.
    pub fn install_deck(&self, deck: DeckId, pcm: Arc<TrackPcm>) {
        let mut s = self.state.lock().unwrap();
        let d = &mut s.decks[deck.index()];
        d.pcm = Some(pcm);
        d.stems = None;
        d.playing = false;
        d.seek_frames(0.0);
        d.eq.reset();
    }

    /// Attach separated stems to the track already on the deck. They must be
    /// the same timeline as the mixed file; the deck keeps playing.
    pub fn install_deck_stems(&self, deck: DeckId, stems: Arc<TrackStems>) {
        let mut s = self.state.lock().unwrap();
        let d = &mut s.decks[deck.index()];
        if d.pcm.is_none() || stems.is_empty() {
            return;
        }
        d.stems = Some(stems);
    }

    pub fn clear_deck_stems(&self, deck: DeckId) {
        self.state.lock().unwrap().decks[deck.index()].stems = None;
    }

    pub fn set_deck_playing(&self, deck: DeckId, playing: bool) {
        let mut s = self.state.lock().unwrap();
        let d = &mut s.decks[deck.index()];
        if playing {
            // Playing from the end restarts.
            if d.pos >= d.frame_count() as f64 {
                d.seek_frames(0.0);
            }
            d.ended = false;
        }
        d.playing = playing;
    }

    pub fn seek_deck_fraction(&self, deck: DeckId, fraction: f64) {
        let mut s = self.state.lock().unwrap();
        let d = &mut s.decks[deck.index()];
        let len = d.frame_count() as f64;
        if len > 0.0 {
            d.seek_frames(fraction.clamp(0.0, 1.0) * len);
        }
    }

    /// Absolute seek in source seconds.
    pub fn seek_deck_seconds(&self, deck: DeckId, secs: f64) {
        let mut s = self.state.lock().unwrap();
        let d = &mut s.decks[deck.index()];
        let Some(pcm) = d.pcm.as_ref() else { return };
        let frames = secs.max(0.0) * pcm.sample_rate.max(1) as f64;
        d.seek_frames(frames);
    }

    /// Tempo multiplier. With key lock on the pitch is preserved; with it
    /// off the deck simply plays faster or slower.
    pub fn set_deck_rate(&self, deck: DeckId, rate: f64) {
        let rate = rate.clamp(crate::decks::RATE_MIN, crate::decks::RATE_MAX) as f32;
        // A short ramp so a sync landing mid-phrase does not step the pitch.
        self.state.lock().unwrap().decks[deck.index()].rate.slew(rate, SLEW_SECS * 4.0);
    }

    pub fn deck_rate(&self, deck: DeckId) -> f64 {
        self.state.lock().unwrap().decks[deck.index()].rate.target() as f64
    }

    pub fn set_deck_keylock(&self, deck: DeckId, on: bool) {
        self.state.lock().unwrap().decks[deck.index()].keylock = on;
    }

    /// Vinyl-style pointer control over the playhead.
    pub fn scratch_deck(&self, deck: DeckId, motion: ScratchMotion) {
        let mut s = self.state.lock().unwrap();
        let d = &mut s.decks[deck.index()];
        let deck_rate = d.rate.current();
        match motion {
            ScratchMotion::Grab => d.scratch.grab(deck_rate),
            ScratchMotion::Move { rate } => d.scratch.drag(rate),
            ScratchMotion::Release => d.scratch.release(deck_rate),
        }
    }

    /// One tone band, 0 = kill.
    pub fn set_deck_eq_band(&self, deck: DeckId, band: usize, gain: f32) {
        self.state.lock().unwrap().decks[deck.index()].eq.set_band(band, gain);
    }

    /// Bipolar sweep filter; 0.5 = off.
    pub fn set_deck_filter(&self, deck: DeckId, position: f32) {
        self.state.lock().unwrap().decks[deck.index()].eq.set_filter(position);
    }

    /// One stem lane's gain. Ramped, so a knob move never zippers.
    pub fn set_deck_stem_gain(&self, deck: DeckId, stem: usize, gain: f32) {
        if stem >= STEM_COUNT {
            return;
        }
        self.state.lock().unwrap().decks[deck.index()].stem_gain[stem]
            .slew(gain.max(0.0), SLEW_SECS * 2.0);
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

    /// Where the crossfader actually is right now, mid-ramp included. The
    /// deck surface mirrors this while a timed fade runs, so the on-screen
    /// fader travels with the audio instead of teleporting to the target.
    pub fn crossfader_position(&self) -> f32 {
        self.state.lock().unwrap().fader.current
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
                let position = d.playhead_frames() / pcm.sample_rate.max(1) as f64;
                (position, pcm.seconds(), d.playing)
            }
        }
    }

    /// `(position_secs, duration_secs, playing, scratching)` in ONE lock.
    /// The per-frame UI path uses this: the audio callback only `try_lock`s,
    /// so every extra grab from the UI is a chance of a silent buffer.
    pub fn deck_snapshot(&self, deck: DeckId) -> (f64, f64, bool, bool) {
        let s = self.state.lock().unwrap();
        let d = &s.decks[deck.index()];
        let scratching = d.scratch.active();
        match &d.pcm {
            None => (0.0, 0.0, false, scratching),
            Some(pcm) => {
                let position = d.playhead_frames() / pcm.sample_rate.max(1) as f64;
                (position, pcm.seconds(), d.playing, scratching)
            }
        }
    }

    /// Pre-fader peak levels for the two deck VU meters. `meters()` reports
    /// what reaches the master; these report what the channel is doing,
    /// which is what an operator sets gain against.
    pub fn deck_levels(&self) -> [f32; 2] {
        [
            f32::from_bits(self.deck_meters[0].load(Ordering::Relaxed)),
            f32::from_bits(self.deck_meters[1].load(Ordering::Relaxed)),
        ]
    }

    /// True while a hand (or its release ramp) owns a deck's playhead.
    pub fn deck_scratching(&self, deck: DeckId) -> bool {
        self.state.lock().unwrap().decks[deck.index()].scratch.active()
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

        // Deck sources are lifted out of the frame loop: one reference count
        // per buffer instead of one per sample, and the borrow checker can
        // then see that the voice state and its PCM are disjoint.
        let deck_pcm: [Option<Arc<TrackPcm>>; 2] =
            [s.decks[0].pcm.clone(), s.decks[1].pcm.clone()];
        let deck_stems: [Option<Arc<TrackStems>>; 2] =
            [s.decks[0].stems.clone(), s.decks[1].stems.clone()];
        let mut deck_peaks = [0.0f32; 2];
        for voice in s.decks.iter_mut() {
            // Filter coefficients are rebuilt once per buffer — the trig is
            // the expensive part and a buffer is well under a millisecond.
            voice.eq.set_sample_rate(rate);
            voice.eq.prepare_block();
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
            let position = s.fader.tick(rate);
            let fader = crossfader_gains(position, s.curve);
            let mut deck_out = [(0.0f32, 0.0f32); 2];
            for (i, d) in s.decks.iter_mut().enumerate() {
                let gain = d.gain.tick(rate) * d.mute.tick(rate);
                let side = if i == 0 { fader.0 } else { fader.1 };
                let deck_rate = d.rate.tick(rate);
                let scratch_rate = d.scratch.tick(rate, deck_rate);
                let scratching = d.scratch.active();
                let mut stem_gain = [0.0f32; STEM_COUNT];
                for (slot, ramp) in stem_gain.iter_mut().zip(d.stem_gain.iter_mut()) {
                    *slot = ramp.tick(rate);
                }
                let Some(pcm) = deck_pcm[i].as_ref() else { continue };
                if pcm.frames.is_empty() {
                    continue;
                }
                // A hand on the record plays even a paused deck; that is the
                // whole point of scrubbing.
                if !scratching && (!d.playing || d.ended) {
                    continue;
                }
                let source = DeckSource {
                    pcm,
                    stems: deck_stems[i].as_deref(),
                    stem_gain,
                };
                let natural_step = pcm.sample_rate as f64 / device_rate;
                let length = pcm.frames.len();

                // Key lock engages the stretcher; scratching and unity rate
                // both read the source directly, so an untouched deck is the
                // sample the decoder produced.
                let want_stretch = d.keylock
                    && !scratching
                    && ((deck_rate as f64) - 1.0).abs() > STRETCH_BYPASS_EPSILON
                    && length > WSOLA_WINDOW + 1;
                if want_stretch != d.stretching {
                    if want_stretch {
                        d.stretch.reset_to(d.pos);
                        d.reader.reset();
                    } else {
                        d.pos = d.stretch.position();
                    }
                    d.stretching = want_stretch;
                }

                let mut ran_out = false;
                let frame = if d.stretching {
                    d.stretch.set_ratio(deck_rate as f64);
                    let read = {
                        let stretch = &mut d.stretch;
                        let reader = &mut d.reader;
                        let loop_on = d.loop_on;
                        let mut pull = || stretch.next(&source, loop_on);
                        reader.read(natural_step, &mut pull)
                    };
                    match read {
                        Some(frame) => {
                            d.pos = d.stretch.position();
                            frame
                        }
                        None => {
                            ran_out = true;
                            [0.0, 0.0]
                        }
                    }
                } else {
                    if d.pos >= length as f64 {
                        if d.loop_on {
                            d.seek_frames(0.0);
                        } else {
                            ran_out = true;
                        }
                    }
                    if ran_out {
                        [0.0, 0.0]
                    } else {
                        let index = d.pos.max(0.0) as usize;
                        let fraction = (d.pos - index as f64) as f32;
                        let a = source.frame(index.min(length - 1));
                        let b = source.frame((index + 1).min(length - 1));
                        let out = [
                            a[0] + (b[0] - a[0]) * fraction,
                            a[1] + (b[1] - a[1]) * fraction,
                        ];
                        let effective = if scratching { scratch_rate } else { deck_rate };
                        d.pos += natural_step * effective as f64;
                        if d.pos < 0.0 {
                            // Scrubbed off the front: the record stops there.
                            d.pos = 0.0;
                        }
                        out
                    }
                };
                if ran_out {
                    if d.loop_on {
                        d.seek_frames(0.0);
                    } else {
                        d.playing = false;
                        d.ended = true;
                        s.ended_decks.push(if i == 0 { DeckId::A } else { DeckId::B });
                    }
                    continue;
                }
                let toned = d.eq.process(frame, rate);
                let pre = [toned[0] * gain, toned[1] * gain];
                deck_peaks[i] = deck_peaks[i].max(pre[0].abs()).max(pre[1].abs());
                deck_out[i] = (pre[0] * side, pre[1] * side);
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
        for (i, p) in deck_peaks.iter().enumerate() {
            self.deck_meters[i].store(p.to_bits(), Ordering::Relaxed);
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

    /// A tone at `frequency`, as a deck would hold it.
    fn tone_pcm(frequency: f64, rate: u32, seconds: f64) -> Arc<TrackPcm> {
        let len = (rate as f64 * seconds) as usize;
        let frames = (0..len)
            .map(|index| {
                let value = (2.0 * std::f64::consts::PI * frequency * index as f64
                    / rate as f64)
                    .sin();
                let sample = (value * 12_000.0) as i16;
                [sample, sample]
            })
            .collect();
        Arc::new(TrackPcm { frames, sample_rate: rate })
    }

    /// RMS of the mixer's left output over `frames`, after `settle` frames.
    fn deck_rms(mixer: &Mixer, rate: f64, settle: usize, frames: usize) -> f64 {
        render(mixer, rate, settle);
        let out = render(mixer, rate, frames);
        let channel = out.channel(0);
        let sum: f64 = channel.iter().map(|v| (*v as f64) * (*v as f64)).sum();
        (sum / channel.len().max(1) as f64).sqrt()
    }

    fn decibels(ratio: f64) -> f64 {
        20.0 * ratio.max(1e-12).log10()
    }

    /// The "fade to A/B" buttons hand the mixer a duration; the fader must
    /// take that long to cross, not jump and land.
    #[test]
    fn a_timed_crossfade_takes_its_duration() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, tone_pcm(440.0, 48_000, 10.0));
        mixer.install_deck(DeckId::B, tone_pcm(440.0, 48_000, 10.0));
        mixer.set_deck_playing(DeckId::A, true);
        mixer.set_deck_playing(DeckId::B, true);
        // Let the initial jump to 0.0 settle before the timed move starts.
        render(&mixer, 48_000.0, 4_096);
        assert!(mixer.state.lock().unwrap().fader.current < 1e-6);

        mixer.fade_crossfader(1.0, 4.0);
        // A quarter of the way through a four-second fade.
        render(&mixer, 48_000.0, 48_000);
        let quarter = mixer.state.lock().unwrap().fader.current;
        assert!(
            quarter > 0.2 && quarter < 0.3,
            "one second into a 4s fade the fader should be near 0.25: {quarter}"
        );
        // And it must actually arrive by the end.
        render(&mixer, 48_000.0, 48_000 * 4);
        let done = mixer.state.lock().unwrap().fader.current;
        assert!((done - 1.0).abs() < 1e-6, "the fade must land on B: {done}");
    }

    // A deck's tone chain has to be in the audible path, not just in the
    // UI: these render real buffers through the real mixer.

    #[test]
    fn a_killed_band_is_removed_from_the_deck_output() {
        let rate = 48_000.0;
        let measure = |band: usize, frequency: f64, kill: bool| -> f64 {
            let mixer = Mixer::new();
            mixer.set_master(1.0);
            mixer.set_crossfader(0.0);
            mixer.install_deck(DeckId::A, tone_pcm(frequency, 48_000, 6.0));
            mixer.set_deck_playing(DeckId::A, true);
            if kill {
                mixer.set_deck_eq_band(DeckId::A, band, 0.0);
            }
            deck_rms(&mixer, rate, 24_000, 24_000)
        };

        // Bass kill: 60 Hz goes, 5 kHz stays.
        let open = measure(0, 60.0, false);
        let killed = measure(0, 60.0, true);
        assert!(
            decibels(killed / open) < -40.0,
            "killing the low band left {:.1} dB of 60 Hz",
            decibels(killed / open)
        );
        let open_high = measure(0, 5_000.0, false);
        let killed_high = measure(0, 5_000.0, true);
        assert!(
            decibels(killed_high / open_high).abs() < 1.0,
            "killing bass moved 5 kHz by {:.2} dB",
            decibels(killed_high / open_high)
        );

        // Treble kill: the mirror image.
        let open = measure(2, 10_000.0, false);
        let killed = measure(2, 10_000.0, true);
        assert!(
            decibels(killed / open) < -40.0,
            "killing the high band left {:.1} dB of 10 kHz",
            decibels(killed / open)
        );
    }

    #[test]
    fn an_untouched_deck_plays_the_decoded_samples_unchanged() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        let pcm = tone_pcm(1_000.0, 48_000, 1.0);
        mixer.install_deck(DeckId::A, pcm.clone());
        mixer.set_deck_playing(DeckId::A, true);
        // Settle the master/fader ramps before comparing.
        render(&mixer, 48_000.0, 4_096);
        let start = {
            let state = mixer.state.lock().unwrap();
            state.decks[0].pos as usize
        };
        let out = render(&mixer, 48_000.0, 256);
        for index in 0..200 {
            let want = pcm.frames[start + index][0] as f32 / 32768.0;
            let got = out.channel(0)[index];
            assert!(
                (got - want).abs() < 1e-6,
                "sample {index}: {got} vs {want} — an untouched deck must be transparent"
            );
        }
    }

    #[test]
    fn key_lock_changes_the_tempo_without_moving_the_pitch() {
        let rate = 48_000.0;
        // Count zero crossings of a 500 Hz tone played 8% fast with key
        // lock on: the frequency must not move with the tempo.
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, tone_pcm(500.0, 48_000, 10.0));
        mixer.set_deck_keylock(DeckId::A, true);
        mixer.set_deck_rate(DeckId::A, 1.08);
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, rate, 48_000);
        let out = render(&mixer, rate, 48_000);
        let channel = out.channel(0);
        let mut crossings = 0usize;
        for index in 1..channel.len() {
            if channel[index - 1] <= 0.0 && channel[index] > 0.0 {
                crossings += 1;
            }
        }
        let measured = crossings as f64 * rate / channel.len() as f64;
        assert!(
            (measured - 500.0).abs() < 8.0,
            "key lock let the pitch drift to {measured:.1} Hz"
        );
        // …while the playhead really did move 8% further than real time.
        let (position, _duration, _playing) = mixer.deck_position(DeckId::A);
        assert!(
            (position - 2.0 * 1.08).abs() < 0.2,
            "two seconds at 1.08x should be ~2.16 s of source, got {position:.3}"
        );
    }

    #[test]
    fn scratching_moves_a_paused_deck_and_release_hands_it_back() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, tone_pcm(440.0, 48_000, 10.0));
        // Deliberately NOT playing: a hand on the record still moves it.
        mixer.scratch_deck(DeckId::A, ScratchMotion::Grab);
        mixer.scratch_deck(DeckId::A, ScratchMotion::Move { rate: 2.0 });
        render(&mixer, 48_000.0, 24_000);
        let (scrubbed, _, playing) = mixer.deck_position(DeckId::A);
        assert!(!playing, "scrubbing is not playing");
        assert!(scrubbed > 0.5, "the hand moved the record: {scrubbed:.3} s");
        assert!(mixer.deck_scratching(DeckId::A));

        // Backwards, too.
        mixer.scratch_deck(DeckId::A, ScratchMotion::Move { rate: -3.0 });
        render(&mixer, 48_000.0, 12_000);
        let (back, _, _) = mixer.deck_position(DeckId::A);
        assert!(back < scrubbed, "a backward scrub must rewind: {back:.3}");

        // Letting go of a paused deck stops it dead.
        mixer.scratch_deck(DeckId::A, ScratchMotion::Release);
        render(&mixer, 48_000.0, 48_000);
        assert!(!mixer.deck_scratching(DeckId::A), "the ramp must finish");
        let (settled, _, _) = mixer.deck_position(DeckId::A);
        render(&mixer, 48_000.0, 24_000);
        let (after, _, _) = mixer.deck_position(DeckId::A);
        assert!(
            (after - settled).abs() < 1e-6,
            "a released, paused deck must sit still: {settled:.3} -> {after:.3}"
        );
    }

    /// Four chunked stem lanes over a four-second track.
    fn chunked_stems(tones: [f64; 4], rate: u32, seconds: f64) -> Arc<TrackStems> {
        let chunk = rate as usize;
        let frames = (rate as f64 * seconds) as usize;
        let count = frames.div_ceil(chunk);
        let mut stems = TrackStems::new(chunk, count);
        for (lane, frequency) in tones.iter().enumerate() {
            if *frequency <= 0.0 {
                continue;
            }
            let pcm = tone_pcm(*frequency, rate, seconds);
            for index in 0..count {
                let start = index * chunk;
                let end = (start + chunk).min(frames);
                stems.lanes[lane][index] =
                    Some(Arc::new(pcm.frames[start..end].to_vec()));
            }
        }
        Arc::new(stems)
    }

    #[test]
    fn stem_lanes_mix_under_their_gains() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        // The mixed file is silence here, so anything audible is a stem.
        mixer.install_deck(DeckId::A, const_pcm(0, 48_000 * 4, 48_000));
        mixer.install_deck_stems(DeckId::A, chunked_stems([1_000.0, 80.0, 0.0, 0.0], 48_000, 4.0));
        mixer.set_deck_playing(DeckId::A, true);
        let both = deck_rms(&mixer, 48_000.0, 8_192, 24_000);
        assert!(both > 0.01, "stems must be audible: {both}");

        // Killing the vocal lane drops the level; killing both silences it.
        mixer.set_deck_stem_gain(DeckId::A, 0, 0.0);
        let one = deck_rms(&mixer, 48_000.0, 8_192, 24_000);
        assert!(one < both * 0.9, "a killed stem must drop the level");
        mixer.set_deck_stem_gain(DeckId::A, 1, 0.0);
        let none = deck_rms(&mixer, 48_000.0, 8_192, 24_000);
        assert!(none < 1e-3, "every stem killed is silence: {none}");
    }

    #[test]
    fn an_unseparated_stretch_plays_the_mixed_file() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        // Audible mixed file, and stems that only cover the second half.
        mixer.install_deck(DeckId::A, tone_pcm(440.0, 48_000, 4.0));
        let mut stems = TrackStems::new(48_000, 4);
        let separated = tone_pcm(440.0, 48_000, 4.0);
        for index in 2..4 {
            let start = index * 48_000;
            for lane in 0..STEM_COUNT {
                stems.lanes[lane][index] = Some(Arc::new(
                    separated.frames[start..start + 48_000].to_vec(),
                ));
            }
        }
        assert!(!stems.covers(0) && stems.covers(2 * 48_000));
        assert!((stems.coverage() - 0.5).abs() < 1e-6);
        mixer.install_deck_stems(DeckId::A, Arc::new(stems));
        // Kill every stem: the covered half goes quiet, the rest plays on.
        for lane in 0..STEM_COUNT {
            mixer.set_deck_stem_gain(DeckId::A, lane, 0.0);
        }
        mixer.set_deck_playing(DeckId::A, true);
        let uncovered = deck_rms(&mixer, 48_000.0, 8_192, 24_000);
        assert!(
            uncovered > 0.05,
            "an unseparated stretch must still play: {uncovered}"
        );
        // Jump into the separated half.
        mixer.seek_deck_seconds(DeckId::A, 2.5);
        let covered = deck_rms(&mixer, 48_000.0, 4_096, 16_000);
        assert!(
            covered < uncovered * 0.1,
            "killed stems must silence the separated stretch: {covered} vs {uncovered}"
        );
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
        assert_eq!(scheduled.decks[0].pos, control.decks[0].pos);
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
        // 1.08x: 100 device frames consume ~108 source frames; decks are
        // untouched by a slot's rate.
        assert_eq!(mixer.set_slot_playback_rate(SlotId::A, 1.08), 1.08);
        mixer.install_deck(DeckId::A, const_pcm(1_000, 4_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        assert!(mixer.push_slot_audio(SlotId::A, &vec![1_000; 2 * 4_000], 2, 48_000));
        mixer.fade_slots(None, SlotId::A, 0.008);
        render(&mixer, 48_000.0, 100);
        let state = mixer.state.lock().unwrap();
        assert_eq!(state.decks[0].pos, 100.0);
        let consumed = 4_000 - state.video[0].queue.len();
        assert!((107..=108).contains(&consumed));
        assert!((consumed as f64 + state.video[0].cursor - 108.0).abs() < 1e-6);
    }
}
