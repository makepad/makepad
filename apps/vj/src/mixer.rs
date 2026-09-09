//! The one audio engine behind `cx.audio_output`: two video-slot buses, two
//! DJ deck voices under an equal-power crossfader, and a bounded pool of
//! one-shot SFX voices, summed through a master gain with a hard safety
//! clamp.
//!
//! Threading contract — no lock anywhere, on any thread:
//! - the device callback owns the mix state outright ([`MixEngine`]) and
//!   calls [`MixEngine::render`]; it drains a lock-free command ring, mixes,
//!   and publishes a `Copy` snapshot through a seqlock. It never waits on
//!   the UI, never allocates and never frees a payload,
//! - the UI holds a [`Mixer`] handle: every mutation is a [`MixCmd`] moved
//!   into the ring (a full ring backs up on the UI side and re-sends next
//!   frame), every read is the last snapshot or the handle's own shadow of
//!   what it sent. What a command replaces comes back to the UI as a
//!   [`Retired`] payload and is dropped there. Every audible parameter
//!   change goes through a [`Ramp`] (a few ms of slew), so gain moves,
//!   mutes, crossfades and slot fades are click-free,
//! - video decode threads push PCM into per-slot lock-free rings and read
//!   back the buffered depth for pacing; closing a slot just flushes and
//!   mutes it — nobody joins anybody.
//!
//! The device clock is the position truth: deck playheads and end-of-track
//! flags advance only inside `render`.

use crate::cue::SlotId;
use crate::verify_or;
use crate::decks::{crossfader_gains, DeckId, FadeCurve, ScratchMotion};
use crate::loop_splat::{
    SplatGrid, SplatPart, SplatRow, SplatSnapshot, SPLAT_COLS, SPLAT_ROWS,
};
use crate::decks::SpinMotion;
use crate::music_dsp::{
    audible, knob, knob64, Autopan, Bitcrusher, Compressor, DeckEcho, DeckEq, Distortion, Flanger,
    FrameSource, Freeze,
    LevelMode, MoogLadder, ParamRamp, Phaser, PlateReverb, RateReader, ScratchRamp, SlotLevel,
    StereoWidth, Stretcher, Tremolo, STEM_COUNT, STRETCH_BYPASS_EPSILON, STRETCH_ENGAGE_EPSILON,
    STRETCH_RATIO_MAX, STRETCH_RATIO_MIN, WSOLA_WINDOW,
};
use crate::music_dsp::{
    MotorEnd, BRAKE_SECS, CENSOR_FLIP_SECS, CENSOR_RATE, CENSOR_RETURN_SECS, SOFT_START_SECS,
    SPINBACK_FALL_SECS, SPINBACK_PEAK, SPINBACK_THROW_SECS,
};
use crate::wave_analysis::{DeckClock, TrackGrid};
use crate::pads::{PadKey, VoiceAlloc, VoiceId};
use crate::score_preview::{PreviewEvent, PreviewSequence};
use crate::program_mix::{
    MasterParam, MasterParams, MasterSnapshot, ProgramMix, StripId, StripSnapshot, STRIP_COUNT,
};
use crate::synth::{
    IronfishParam, IronfishPatch, RackSnapshot, StepPattern, SynthClock, SynthEngines, SynthRack,
    SynthTrack,
};
use makepad_drumkit::{DrumKit, SampleBank};
use makepad_piano_model::{Piano, PianoEvent, TimedEvent as PianoTimedEvent};
use makepad_widgets::makepad_platform::audio::AudioBuffer;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use crate::spsc::{OnceSlot, SeqCell, SpscRing, UiCell};
use std::sync::Arc;

/// Q32.32 fixed-point source-frame cursor.
const FP_ONE: u64 = 1 << 32;
/// Default parameter slew, seconds — fast enough to feel instant, slow
/// enough to never click.
/// The most the master can be turned up: a fifth over unity.
///
/// Named because three places have to agree about it and did not — the
/// clamp here, the on-screen slider's declared range, and every hand that
/// maps a 0..1 control onto it. A control surface's master fader was passed
/// through RAW, so its full stop reached only five sixths of the range,
/// while the same fader adopted through learn was scaled and could reach
/// all of it: one piece of plastic with two meanings, decided by which code
/// path claimed it.
pub const MAX_MASTER_GAIN: f32 = 1.2;

/// Where a 0..1 control sets the master.
///
/// A function rather than a multiplication written out twice, because
/// written out twice is exactly how the two paths came to disagree: one
/// scaled and one did not. Every hand that holds a normalised control and
/// wants the master goes through here.
pub fn master_from_control(position: f32) -> f32 {
    position.clamp(0.0, 1.0) * MAX_MASTER_GAIN
}

const SLEW_SECS: f32 = 0.008;
/// Autopilot blend moves: fast enough to read as a cut on the bar, slow
/// enough never to click.
const BLEND_SECS: f32 = 0.08;
/// Cap on queued video-slot audio, frames (~2s at 48k): a stalled consumer
/// can never grow a queue without bound.
const MAX_SLOT_QUEUE_FRAMES: usize = 96_000;
/// Master safety clamp.
const CLAMP: f32 = 1.0;
/// Width of the blend when separated stems first take over from the mixed
/// file on a playing deck, seconds. At unity gains the two are the same
/// signal and the blend is inaudible; under a knob already turned it is
/// what keeps the swap from being a step.
const STEM_SWAP_SECS: f32 = 0.020;
/// Width of the crossfade at a deck loop's wrap, seconds. The tail of the
/// loop blends into the run-up to IN over this window, so the seam is a
/// mix of two pieces of programme rather than a gain treatment — long
/// enough to swallow the splice, short enough to blur nothing musical.
const LOOP_XFADE_SECS: f64 = 0.010;
/// Width of the blend after a commanded jump — a timeline click, a QUANT
/// commit, an engage or RELOOP landing, a moved loop's ride-along. Shorter
/// than the wrap's: a jump is a deliberate cut and should feel like one,
/// just not sound like a spark.
const SEEK_XFADE_SECS: f64 = 0.005;
/// A launch arriving this far after a downbeat still belongs to that
/// downbeat instead of waiting almost a full bar.
const LATE_LAUNCH_BEATS: f64 = 1.0 / 16.0;
const SPLAT_XFADE_SECS: f64 = 0.005;
/// Explicit beat-sync (N beats per loop) may ask for wide rates; the
/// automatic loop-fit keeps its own ≤8% guard (`fit_loop_to_grid`).
pub const MIN_VIDEO_PLAYBACK_RATE: f64 = 0.25;
pub const MAX_VIDEO_PLAYBACK_RATE: f64 = 4.0;

/// Frames the phones consumer keeps behind the cue writer — the monitor
/// latency (~43 ms at 48k) and the underrun safety margin in one number.
const CUE_TARGET_FRAMES: u64 = 2_048;
/// Cue ring capacity in frames; a power of two, so the index is a mask.
const CUE_RING_FRAMES: usize = 16_384;
/// Platform device callbacks are at most 4096 frames; preview storage is
/// built once with the instruments and never resized by the callback.
const SCORE_PREVIEW_MAX_BLOCK: usize = 4_096;
const SCORE_PREVIEW_GAIN: f32 = 0.72;

/// Which point of the deck chain the headphone cue listens to.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CueMode {
    /// Post-EQ, pre gain/mute/crossfader: the working DJ's pre-listen —
    /// the next track at full level while its faders are still down.
    #[default]
    Pfl,
    /// The deck's actual contribution to the program, pre master.
    PostFader,
    /// The unprocessed deck frame, before the EQ.
    Raw,
}

impl CueMode {
    pub fn index(self) -> usize {
        match self {
            CueMode::Pfl => 0,
            CueMode::PostFader => 1,
            CueMode::Raw => 2,
        }
    }

    pub fn from_index(index: usize) -> CueMode {
        match index {
            1 => CueMode::PostFader,
            2 => CueMode::Raw,
            _ => CueMode::Pfl,
        }
    }
}

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

    /// A consistent read. `None` until a schedule has ever been published.
    fn snapshot(&self) -> Option<VideoTransitionSnapshot> {
        let (phase, id, rendered_frame, target_frame, fade_frames, raw_start, from, to) = loop {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let values = (
                VideoTransitionPhase::from_u32(self.phase.load(Ordering::Relaxed)),
                self.id.load(Ordering::Relaxed),
                self.rendered_frame.load(Ordering::Relaxed),
                self.target_frame.load(Ordering::Relaxed),
                self.fade_frames.load(Ordering::Relaxed),
                self.start_frame.load(Ordering::Relaxed),
                self.from.load(Ordering::Relaxed),
                self.to.load(Ordering::Relaxed),
            );
            let after = self.sequence.load(Ordering::Acquire);
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
            from: Self::decode_slot(from),
            to: Self::decode_slot(to).unwrap_or(SlotId::A),
            target_frame,
            start_frame,
            fade_frames,
            rendered_frame,
            progress,
        })
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

/// Frames per streamed chunk. A power of two, so the per-sample chunk
/// lookup on the audio thread is a shift and a mask: ~2.7 s at 48 kHz,
/// ~3 s at 44.1 kHz.
pub const STREAM_CHUNK_SHIFT: u32 = 17;
pub const STREAM_CHUNK_FRAMES: usize = 1 << STREAM_CHUNK_SHIFT;

/// A track still coming out of the decoder: whole chunks in order, every
/// one [`STREAM_CHUNK_FRAMES`] long except the last.
///
/// The table is an immutable snapshot. A new chunk makes a NEW table that
/// shares every earlier chunk (`with_chunk` clones a vector of pointers,
/// never audio), and the UI thread swaps it in under the state lock as one
/// pointer move — so the callback never sees a table mid-growth, never
/// waits, and never allocates to read it.
pub struct StreamPcm {
    pub sample_rate: u32,
    pub chunks: Vec<Arc<Vec<[i16; 2]>>>,
    /// Frames decoded so far: the sum of the chunk lengths.
    pub len: usize,
    /// The length the decoder expects the track to have (its container's
    /// duration), never less than `len`. What the strip and the time
    /// display are scaled to while the file is still arriving.
    pub expected: usize,
    /// The decoder reported the end: `len` is the whole track.
    pub complete: bool,
}

impl StreamPcm {
    pub fn new(sample_rate: u32, expected: Option<usize>) -> StreamPcm {
        let capacity = expected.map_or(0, |frames| frames.div_ceil(STREAM_CHUNK_FRAMES) + 1);
        StreamPcm {
            sample_rate,
            chunks: Vec::with_capacity(capacity),
            len: 0,
            expected: expected.unwrap_or(0),
            complete: false,
        }
    }

    /// This table plus one more chunk. The chunk before it must have been
    /// full — the read path relies on every chunk but the last being
    /// exactly [`STREAM_CHUNK_FRAMES`] — and an empty chunk only marks the
    /// end.
    pub fn with_chunk(&self, chunk: Arc<Vec<[i16; 2]>>, last: bool) -> StreamPcm {
        debug_assert!(
            self.chunks.last().map_or(true, |previous| previous.len() == STREAM_CHUNK_FRAMES),
            "a streamed chunk may only follow a full one"
        );
        debug_assert!(!self.complete, "no chunk follows the end of a stream");
        let mut chunks = Vec::with_capacity(self.chunks.capacity().max(self.chunks.len() + 1));
        chunks.extend(self.chunks.iter().cloned());
        let mut len = self.len;
        if !chunk.is_empty() {
            len += chunk.len();
            chunks.push(chunk);
        }
        let expected = if last { len } else { self.expected.max(len) };
        StreamPcm { sample_rate: self.sample_rate, chunks, len, expected, complete: last }
    }

    pub fn seconds(&self) -> f64 {
        self.len as f64 / self.sample_rate.max(1) as f64
    }

    pub fn expected_seconds(&self) -> f64 {
        self.expected.max(self.len) as f64 / self.sample_rate.max(1) as f64
    }

    #[inline]
    fn frame_f32(&self, index: usize) -> [f32; 2] {
        if index >= self.len {
            return [0.0, 0.0];
        }
        let chunk = index >> STREAM_CHUNK_SHIFT;
        let offset = index & (STREAM_CHUNK_FRAMES - 1);
        match self.chunks.get(chunk).and_then(|chunk| chunk.get(offset)) {
            Some(frame) => [frame[0] as f32 / 32768.0, frame[1] as f32 / 32768.0],
            None => [0.0, 0.0],
        }
    }
}

/// What a deck voice reads from: the whole file once it is decoded, or the
/// growing chunk table while it is being decoded. Same timeline, same
/// samples; the swap from one to the other at the end of the decode is a
/// pointer move at the playhead and cannot be heard.
#[derive(Clone)]
pub enum DeckPcm {
    Whole(Arc<TrackPcm>),
    Stream(Arc<StreamPcm>),
}

impl DeckPcm {
    /// Frames that can be read right now.
    #[inline]
    pub fn len(&self) -> usize {
        match self {
            DeckPcm::Whole(pcm) => pcm.frames.len(),
            DeckPcm::Stream(stream) => stream.len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn sample_rate(&self) -> u32 {
        match self {
            DeckPcm::Whole(pcm) => pcm.sample_rate,
            DeckPcm::Stream(stream) => stream.sample_rate,
        }
    }

    /// The track's length as far as anyone knows: exact once decoded, the
    /// decoder's expectation before that.
    pub fn expected_seconds(&self) -> f64 {
        match self {
            DeckPcm::Whole(pcm) => pcm.seconds(),
            DeckPcm::Stream(stream) => stream.expected_seconds(),
        }
    }

    pub fn expected_len(&self) -> usize {
        match self {
            DeckPcm::Whole(pcm) => pcm.frames.len(),
            DeckPcm::Stream(stream) => stream.expected.max(stream.len),
        }
    }

    /// False while the decoder is still delivering: a playhead at `len`
    /// is waiting at the edge, not at the end of the track.
    pub fn complete(&self) -> bool {
        match self {
            DeckPcm::Whole(_) => true,
            DeckPcm::Stream(stream) => stream.complete,
        }
    }

    pub fn whole(&self) -> Option<&TrackPcm> {
        match self {
            DeckPcm::Whole(pcm) => Some(pcm),
            DeckPcm::Stream(_) => None,
        }
    }

    #[inline]
    fn frame_f32(&self, index: usize) -> [f32; 2] {
        match self {
            DeckPcm::Whole(pcm) => pcm.frame_f32(index),
            DeckPcm::Stream(stream) => stream.frame_f32(index),
        }
    }
}

/// Headroom the stem lanes are stored with.
///
/// BS-RoFormer's masks are complex ratios, not a partition of unity, so a
/// stem legitimately peaks ABOVE full scale: the reference vocals stem hits
/// 1.12 and a measured drums stem 1.47. Encoding those straight to i16 hard-
/// clipped every peak past 1.0 — about 0.05% of drum samples on a real
/// track, which is exactly the transients, and it hardens them audibly.
///
/// So every lane is divided by this on the way in and multiplied back on the
/// way out, spending one bit of resolution to keep the peaks intact. The
/// on-disk span cache solves the same problem differently (a per-span peak
/// stored beside the samples); this is the in-memory playback format, where
/// a chunk has to be indexable arithmetically and cannot carry side data.
///
/// Every producer of a [`TrackStems`] lane must encode through
/// [`encode_stem_sample`], and the only consumer that reads absolute levels
/// out of one is [`DeckSource::frame`].
pub const STEM_CHUNK_HEADROOM: f32 = 2.0;

/// One stem sample (nominally -1.0..1.0, legitimately beyond) into the lane
/// format. See [`STEM_CHUNK_HEADROOM`].
pub fn encode_stem_sample(value: f32) -> i16 {
    ((value / STEM_CHUNK_HEADROOM).clamp(-1.0, 1.0) * 32767.0) as i16
}

/// A separated track: four stem lanes on the SAME timeline as the mixed
/// file, delivered in fixed chunks as the separator streams them.
///
/// Lanes are stored with [`STEM_CHUNK_HEADROOM`], not at full scale.
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplatFrameCell {
    pub col: u8,
    pub start_frames: f64,
    pub len_frames: f64,
}

/// The frame-domain form sent to the audio state. Conversion happens on the
/// caller/UI thread; the callback only indexes fixed arrays.
#[derive(Clone, Debug, PartialEq)]
pub struct SplatFrames {
    pub bar_frames: f64,
    pub first_bar_frames: f64,
    pub cells: [[Option<SplatFrameCell>; SPLAT_COLS]; SPLAT_ROWS],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeckSnapshot {
    pub position_secs: f64,
    pub duration_secs: f64,
    pub playing: bool,
    pub scratching: bool,
    /// What the platter is turning at; see [`DeckSnap::platter_rate`]. A
    /// deck with no track publishes 0.0, which is also what the default
    /// gives before the first buffer: nothing is turning either way.
    pub platter_rate: f64,
    /// The beat as the last rendered buffer saw it, for anything drawing
    /// to the music rather than to the clock on the wall.
    pub clock: DeckClock,
    pub splat: Option<SplatSnapshot>,
}

impl SplatFrames {
    pub fn from_grid(grid: &SplatGrid, source_rate: f64) -> Self {
        let mut cells = [[None; SPLAT_COLS]; SPLAT_ROWS];
        for row in SplatRow::ALL {
            for col in 0..SPLAT_COLS {
                cells[row.index()][col] = grid.cells[row.index()][col]
                    .filter(|cell| !cell.silent)
                    .map(|cell| SplatFrameCell {
                        col: col as u8,
                        start_frames: cell.span.start_secs * source_rate,
                        len_frames: cell.span.len_secs() * source_rate,
                    });
            }
        }
        Self {
            bar_frames: grid.bar_secs * source_rate,
            first_bar_frames: grid.first_bar_secs * source_rate,
            cells,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RowCell {
    col: u8,
    part: SplatPart,
    start_frames: f64,
    len_frames: f64,
    anchor_frames: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Queued {
    cell: Option<RowCell>,
    at_frames: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SplatFade {
    outgoing: Option<RowCell>,
    incoming: Option<RowCell>,
    start_frames: f64,
    len_frames: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SplatRowVoice {
    cell: Option<RowCell>,
    queued: Option<Queued>,
    fade: Option<SplatFade>,
}

struct SplatState {
    grid: Arc<SplatGrid>,
    /// Boxed so a replaced grid's frames go back to the UI whole, and the
    /// state never frees them on the audio thread.
    frames: Box<SplatFrames>,
    active: bool,
    master_frames: f64,
    phase_fade: Option<SplatPhaseFade>,
    rows: [SplatRowVoice; SPLAT_ROWS],
    /// The cell the picture follows: the one launched last. The deck's
    /// reported playhead cycles inside it, so the waveform stays on the
    /// segment that is sounding instead of walking off through the track
    /// with the master clock.
    view: Option<RowCell>,
}

impl SplatState {
    fn new(grid: Arc<SplatGrid>, frames: Box<SplatFrames>, master_frames: f64) -> Self {
        Self {
            grid,
            frames,
            active: false,
            master_frames,
            phase_fade: None,
            rows: [SplatRowVoice::default(); SPLAT_ROWS],
            view: None,
        }
    }

    /// Where the ear is: inside the cell the picture follows, else on the
    /// master clock.
    fn playhead_frames(&self) -> f64 {
        match self.view {
            Some(cell) if self.master_frames >= cell.anchor_frames => {
                cell.start_frames
                    + (self.master_frames - cell.anchor_frames).rem_euclid(cell.len_frames.max(1.0))
            }
            Some(cell) => cell.start_frames,
            None => self.master_frames,
        }
    }

    /// Nothing sounding and nothing waiting to.
    fn idle(&self) -> bool {
        self.rows.iter().all(|row| row.cell.is_none() && row.queued.is_none())
    }

    /// The cell a row is sounding, or the one it is about to.
    fn row_slot(voice: &SplatRowVoice) -> Option<RowCell> {
        voice.queued.and_then(|queued| queued.cell).or(voice.cell)
    }

    /// A launch or a stop just landed: the picture keeps following its
    /// cell while that cell sounds, moves to a row that is still sounding
    /// when its own row stopped, and goes back to the master clock when
    /// every row has.
    fn revalidate_view(&mut self) {
        let Some(view) = self.view else { return };
        if self.rows.iter().any(|voice| Self::row_slot(voice) == Some(view)) {
            return;
        }
        self.view = self.rows.iter().find_map(Self::row_slot);
    }

    fn bar_start_at_or_before(&self, master: f64) -> f64 {
        if self.frames.bar_frames <= 0.0 || master < self.frames.first_bar_frames {
            return self.frames.first_bar_frames;
        }
        let index = ((master - self.frames.first_bar_frames) / self.frames.bar_frames).floor();
        self.frames.first_bar_frames + index * self.frames.bar_frames
    }

    fn next_bar_after(&self, master: f64) -> f64 {
        let boundary = self.bar_start_at_or_before(master);
        if master <= boundary {
            return boundary;
        }
        let forgiveness = self.frames.bar_frames * 0.25 * LATE_LAUNCH_BEATS;
        if master - boundary <= forgiveness {
            boundary
        } else {
            boundary + self.frames.bar_frames
        }
    }

    /// The source span a slot names on the CURRENT frames: the cell's
    /// bars, or the `part` of them, as `(start, len)` in source frames.
    /// One derivation for a launch and for a re-launch on a replaced grid,
    /// so the same slot always means the same samples.
    fn slot_frames(&self, row: SplatRow, col: usize, part: SplatPart) -> Option<(f64, f64)> {
        if col >= SPLAT_COLS || !part.is_valid() {
            return None;
        }
        let cell = self.frames.cells[row.index()][col]?;
        let part_len = cell.len_frames / f64::from(part.den);
        Some((cell.start_frames + f64::from(part.num) * part_len, part_len.max(1.0)))
    }

    fn queue_cell(&mut self, row: SplatRow, col: usize, part: SplatPart, sync_locked: bool) {
        if !self.active {
            return;
        }
        let Some((start_frames, len_frames)) = self.slot_frames(row, col, part) else { return };
        // The first FREE launch into a silent grid starts NOW, and the master
        // clock is re-seated on the cell so the grid's bars and the loop's
        // bars are the same bars from here: the click plays exactly the
        // segment it named, at once. Later launches join on the next bar,
        // in phase with what is already running. A synced grid preserves
        // its clock even on the first launch and joins on its next bar.
        let at_frames = if self.idle() && !sync_locked {
            self.master_frames = start_frames;
            start_frames
        } else {
            self.next_bar_after(self.master_frames)
        };
        let launched = RowCell {
            col: col as u8,
            part,
            start_frames,
            len_frames,
            anchor_frames: at_frames,
        };
        self.rows[row.index()].queued = Some(Queued { cell: Some(launched), at_frames });
        self.view = Some(launched);
    }

    /// The frames were replaced under a running grid — the refined grid
    /// landed once the stems were in. Every row that is sounding or waiting
    /// re-launches the SAME slot on the new frames at the next bar, through
    /// the ordinary crossfade, and a slot the new grid no longer has stops
    /// there. What is heard is always what the grid on screen says, so a
    /// click on a cell lands on the boundaries it shows however many grids
    /// have come and gone; a slot whose span did not change is left alone.
    fn rebase_rows(&mut self) {
        let view = self.view;
        let mut rebased_view = None;
        for row in SplatRow::ALL {
            let voice = self.rows[row.index()];
            // A pending stop stands; a pending launch moves to the new
            // frames like a sounding one.
            if matches!(voice.queued, Some(Queued { cell: None, .. })) {
                continue;
            }
            let Some(old) = Self::row_slot(&voice) else { continue };
            let follows_view = view == Some(old);
            match self.slot_frames(row, usize::from(old.col), old.part) {
                Some((start_frames, len_frames))
                    if start_frames == old.start_frames && len_frames == old.len_frames => {}
                Some(_) => {
                    self.queue_cell(row, usize::from(old.col), old.part, true);
                    if follows_view {
                        rebased_view = self.rows[row.index()].queued.and_then(|queued| queued.cell);
                    }
                }
                None => self.queue_stop(row, true),
            }
        }
        // Re-launching set the picture to whichever row went last; it
        // follows the row it followed before.
        self.view = rebased_view.or(view);
    }

    /// A plain stop is immediate: the loop goes quiet on the next rendered
    /// frame through the same equal-power fade a swap uses. A timed stop
    /// (shift-click) waits for the next bar like a launch does.
    fn queue_stop(&mut self, row: SplatRow, timed: bool) {
        if !self.active {
            return;
        }
        let at_frames = if timed {
            self.next_bar_after(self.master_frames)
        } else {
            self.master_frames
        };
        self.rows[row.index()].queued = Some(Queued { cell: None, at_frames });
    }

    fn snapshot(&self) -> SplatSnapshot {
        let bar = if self.frames.bar_frames > 0.0 {
            (self.master_frames - self.frames.first_bar_frames) / self.frames.bar_frames
        } else {
            0.0
        };
        let mut snapshot = SplatSnapshot {
            active: self.active,
            clock_secs: self.master_frames * self.grid.bar_secs / self.frames.bar_frames.max(1.0),
            bar_index: bar.floor() as i64,
            bar_phase: bar.rem_euclid(1.0) as f32,
            ..SplatSnapshot::default()
        };
        for row in SplatRow::ALL {
            let voice = self.rows[row.index()];
            snapshot.playing[row.index()] = voice.cell.map(|cell| (cell.col, cell.part));
            snapshot.queued[row.index()] = voice
                .queued
                .and_then(|queued| queued.cell.map(|cell| (cell.col, cell.part)));
            snapshot.row_phase[row.index()] = voice.cell.map_or(0.0, |cell| {
                ((self.master_frames - cell.anchor_frames).rem_euclid(cell.len_frames)
                    / cell.len_frames) as f32
            });
        }
        snapshot
    }
}

/// What a deck's DSP chain reads from: the full mix, or the stem lanes
/// summed under their current gains.
struct DeckSource<'a> {
    pcm: &'a DeckPcm,
    stems: Option<&'a TrackStems>,
    /// The stem chunk the last read fell in, `(chunk, first frame)`: reads
    /// run sequentially, so the division that finds a chunk happens once
    /// per chunk instead of once per sample.
    stem_chunk: std::cell::Cell<(usize, usize)>,
    stem_gain: [f32; STEM_COUNT],
    /// How far the stem lanes have taken over from the mixed file: 1.0 once
    /// the swap-in blend has run its few milliseconds (see
    /// [`STEM_SWAP_SECS`]).
    stem_blend: f32,
}

impl FrameSource for DeckSource<'_> {
    #[inline]
    fn frame_count(&self) -> usize {
        self.pcm.len()
    }

    #[inline]
    fn frame(&self, index: usize) -> [f32; 2] {
        let Some(stems) = self.stems else {
            return self.pcm.frame_f32(index);
        };
        let (mut chunk, mut start) = self.stem_chunk.get();
        if index < start || index - start >= stems.chunk_frames {
            chunk = index / stems.chunk_frames;
            start = chunk * stems.chunk_frames;
            self.stem_chunk.set((chunk, start));
        }
        let offset = index - start;
        let mut out = [0.0f32; 2];
        let mut separated = false;
        for (lane, gain) in stems.lanes.iter().zip(self.stem_gain) {
            let Some(Some(block)) = lane.get(chunk) else { continue };
            separated = true;
            if gain <= 0.0 {
                continue;
            }
            let Some(frame) = block.get(offset) else { continue };
            // Lanes carry STEM_CHUNK_HEADROOM; undo it here so a stem that
            // peaks past full scale plays at the level it was separated at.
            let scale = STEM_CHUNK_HEADROOM / 32768.0 * gain;
            out[0] += frame[0] as f32 * scale;
            out[1] += frame[1] as f32 * scale;
        }
        // Nothing separated here yet: the mixed file plays, as it did
        // before separation existed.
        if !separated {
            return self.pcm.frame_f32(index);
        }
        // The swap-in: the stem sum IS the mixed file, so at unity gains
        // this blend is a no-op — it only softens a swap that lands under
        // knobs already turned, where the two really differ.
        if self.stem_blend < 1.0 {
            let mixed = self.pcm.frame_f32(index);
            let t = self.stem_blend.max(0.0);
            out[0] = mixed[0] + (out[0] - mixed[0]) * t;
            out[1] = mixed[1] + (out[1] - mixed[1]) * t;
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
    ///
    /// A target that is not a number is refused. It would never settle:
    /// `current != target` stays true for ever, every comparison against
    /// it is false, and the ramp spends the rest of the set walking
    /// towards nothing while whatever it feeds is silent.
    fn slew(&mut self, target: f32, secs: f32) {
        if !target.is_finite() {
            return;
        }
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

/// The callback's per-slot cursor over the slot ring, and the slot's
/// fade gain. Everything else about a slot (open, paused, rates, the
/// audio itself) lives in [`SlotShared`], lock-free between the threads.
struct VideoBus {
    /// Fractional read position past the ring's consumed edge.
    cursor: f64,
    gain: Ramp,
}

impl VideoBus {
    fn new() -> VideoBus {
        VideoBus { cursor: 0.0, gain: Ramp::at(0.0) }
    }
}


/// A few milliseconds of the outgoing stream kept alive after a commanded
/// jump, so the seek lands as a blend instead of a splice. `left/total` is
/// the outgoing share, counted down a frame per rendered frame.
struct SeekFade {
    pos: f64,
    left: f64,
    total: f64,
}

/// Fixed-size copy of the outgoing sample voices for a clock correction.
/// Keeping their active fades avoids a splice when a correction crosses
/// a queued launch or stop.
struct SplatPhaseFade {
    pos: f64,
    left: f64,
    total: f64,
    rows: [SplatRowVoice; SPLAT_ROWS],
}

/// One slot's occupant. A closed, small set matched inline -- the same
/// dispatch this file already uses for `CueMode` -- rather than a trait:
/// nothing here needs a slot to hold a kind unknown to this crate, and a
/// `match` lets the optimizer inline each unit's own crossfade math
/// directly into the chain's per-sample loop instead of going through a
/// vtable ~512 times per callback per deck.
enum EffectKind {
    Eq(DeckEq),
    Freeze(Freeze),
    Echo(DeckEcho),
    Flanger(Flanger),
    Bitcrusher(Bitcrusher),
    Tremolo(Tremolo),
    Distortion(Distortion),
    Phaser(Phaser),
    Autopan(Autopan),
    StereoWidth(StereoWidth),
    PlateReverb(PlateReverb),
    MoogLadder(MoogLadder),
    Compressor(Compressor),
}

impl EffectKind {
    #[inline]
    fn process(&mut self, frame: [f32; 2], device_rate: f32) -> [f32; 2] {
        match self {
            EffectKind::Eq(eq) => eq.process(frame, device_rate),
            EffectKind::Freeze(freeze) => freeze.process(frame, device_rate),
            EffectKind::Echo(echo) => echo.process(frame, device_rate),
            EffectKind::Flanger(flanger) => flanger.process(frame, device_rate),
            EffectKind::Bitcrusher(bitcrusher) => bitcrusher.process(frame, device_rate),
            EffectKind::Tremolo(tremolo) => tremolo.process(frame, device_rate),
            EffectKind::Distortion(distortion) => distortion.process(frame, device_rate),
            EffectKind::Phaser(phaser) => phaser.process(frame, device_rate),
            EffectKind::Autopan(autopan) => autopan.process(frame, device_rate),
            EffectKind::StereoWidth(stereo_width) => stereo_width.process(frame, device_rate),
            EffectKind::PlateReverb(plate_reverb) => plate_reverb.process(frame, device_rate),
            EffectKind::MoogLadder(moog_ladder) => moog_ladder.process(frame, device_rate),
            EffectKind::Compressor(compressor) => compressor.process(frame, device_rate),
        }
    }

    /// True when a walk through this slot would hand the frame straight
    /// back and change nothing in the unit -- each unit's own answer,
    /// which is its early return's condition made strict: off, settled
    /// there, and every ramp it ticks before looking already home.
    #[inline]
    fn transparent(&self) -> bool {
        match self {
            EffectKind::Eq(eq) => eq.transparent(),
            EffectKind::Freeze(freeze) => freeze.transparent(),
            EffectKind::Echo(echo) => echo.transparent(),
            EffectKind::Flanger(flanger) => flanger.transparent(),
            EffectKind::Bitcrusher(bitcrusher) => bitcrusher.transparent(),
            EffectKind::Tremolo(tremolo) => tremolo.transparent(),
            EffectKind::Distortion(distortion) => distortion.transparent(),
            EffectKind::Phaser(phaser) => phaser.transparent(),
            EffectKind::Autopan(autopan) => autopan.transparent(),
            EffectKind::StereoWidth(stereo_width) => stereo_width.transparent(),
            EffectKind::PlateReverb(plate_reverb) => plate_reverb.transparent(),
            EffectKind::MoogLadder(moog_ladder) => moog_ladder.transparent(),
            EffectKind::Compressor(compressor) => compressor.transparent(),
        }
    }
}

/// rate, whatever the real head is doing -- paused, scratched, jumped,
/// looping or run off the end. Letting slip go lands the deck on it, so
/// the track carries on as if the detour never happened.
#[derive(Clone, Copy, Debug)]
struct Ghost {
    /// Where it has got to, in source frames.
    pos: f64,
    /// Source frames per device frame, latched at arm time: a buffer is one
    /// multiply, and the ghost does not chase a tempo the hand is moving.
    step: f64,
    /// The span the ghost wraps through, if one was running when slip was
    /// armed. A loop engaged AFTER arming -- the roll being slipped over --
    /// deliberately does not catch it.
    span: Option<(f64, f64)>,
}

/// The ghosts a stack of rolls is keeping, newest last.
///
/// Each level latches the span that was running when THAT level engaged,
/// so releasing level two lands where level one's playback would have
/// been, and releasing level one lands where the record would have been.
#[derive(Clone, Copy, Default)]
struct RollGhosts {
    ghosts: [Option<Ghost>; ROLL_STACK_CAP],
    len: usize,
}

impl RollGhosts {
    fn push(&mut self, ghost: Ghost) -> bool {
        if self.len >= ROLL_STACK_CAP {
            return false;
        }
        self.ghosts[self.len] = Some(ghost);
        self.len += 1;
        true
    }

    fn pop(&mut self) -> Option<Ghost> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        self.ghosts[self.len].take()
    }

    fn clear(&mut self) {
        *self = RollGhosts::default();
    }

    /// Advance every level. Once per deck per buffer, never per frame.
    fn advance(&mut self, frames: f64) {
        for ghost in self.ghosts.iter_mut().take(self.len).flatten() {
            ghost.pos += ghost.step * frames;
            if let Some((start, end)) = ghost.span {
                if ghost.pos >= end {
                    ghost.pos = wrapped_into_span(ghost.pos, start, end);
                }
            }
        }
    }
}

/// Fold a playhead back inside a span, keeping the overshoot.
///
/// Modulo rather than a reset to IN: resetting discards up to a step per
/// lap, so a held loop walks audibly early, and it is also what catches a
/// playhead stranded past OUT by a live resize -- modulo continues the
/// subdivision in phase instead of re-triggering the downbeat at IN.
///
/// One function because there are three callers now: the render's wrap,
/// the resize that catches a paused deck, and slip's ghost, which has to
/// wrap exactly the way the real head does or the two land apart.
pub(crate) fn wrapped_into_span(pos: f64, start: f64, end: f64) -> f64 {
    let len = (end - start).max(1.0);
    start + (pos - start).rem_euclid(len)
}

/// The source-seconds-per-output-second this voice is turning at right
/// now: a running splat owns it outright (1.0, whatever the fader says),
/// a hand or a motor owns it next, the fader otherwise -- and a deck with
/// nothing to play turns at nothing.
///
/// Shared by the render prelude and by every setter that has to answer
/// the same question between callbacks, so the two can never disagree.
fn deck_platter(voice: &DeckVoice) -> f64 {
    if voice.frame_count() == 0 {
        0.0
    } else if voice.splat.as_ref().is_some_and(|splat| splat.active) {
        1.0
    } else if voice.scratch.active() {
        voice.scratch.rate() as f64
    } else {
        voice.rate.current() as f64
    }
}

/// A load waiting out the outgoing record's fade.
///
/// The point is WHICH track fades. Swapping the PCM on the instant the
/// decode lands ends the old one mid-sample; parking it here and
/// installing when the transport reaches silence means the track being
/// replaced is the one the room hears leave.
struct PendingLoad {
    pcm: DeckPcm,
    /// What the operator asked for: keep playing through the swap, or
    /// take the deck and stop.
    play: bool,
    /// The new record's grid, when its analysis arrived before the swap
    /// was spent. A load and its analysis are two errands and either can
    /// finish first; a grid that lands during the fade belongs to the
    /// record coming in, not the one going out.
    grid: Option<TrackGrid>,
}

/// Latch a ghost at the deck's own playhead and rate.
///
/// Latched rather than followed: a ghost that chased the tempo fader
/// would drift away from where the record would actually have been,
/// which is the one thing it exists to remember. `false` when there is
/// already one, when there is nothing loaded, or before the first
/// callback has given us a device rate to latch a step from -- a ghost
/// that cannot move is worse than none.
fn arm_ghost(d: &mut DeckVoice, device_rate: f64) -> bool {
    if d.slip.is_some() {
        return false;
    }
    let Some(pcm) = d.pcm.as_ref() else { return false };
    if !(device_rate > 0.0) {
        return false;
    }
    let natural = pcm.sample_rate().max(1) as f64 / device_rate;
    d.slip = Some(Ghost {
        pos: d.playhead_frames(),
        step: natural * d.rate.current() as f64,
        span: d.loop_span,
    });
    true
}

/// Where one rendered buffer's time went. Three phases and not five: the
/// render is ONE frame loop with no sequential per-source block to time,
/// and timing sources would mean a clock read per source per SAMPLE,
/// which costs more than the work it measures.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StageNanos {
    /// Ramps, filter coefficients, lifting the deck sources out of the loop.
    pub setup: u64,
    /// The frame loop, which is nearly all of it.
    pub mix: u64,
    /// Meters, the cue publish, the per-deck snapshots, reaping.
    pub publish: u64,
}

/// What the audio thread is managing, for the console strip.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioHealth {
    /// Whole buffers silenced by lock contention.
    ///
    /// Always zero now, and kept only so the console's line does not have
    /// to change shape: this engine has no lock for the UI to hold, so
    /// there is nothing left to contend for. It counted a hazard that has
    /// been designed out rather than reduced.
    pub contended: u64,
    /// Callbacks that found the lock poisoned and took it over. Zero for
    /// the same reason.
    pub poisoned: u64,
    /// Buffers the render could not fill inside their own playing time,
    /// since the app started. Nothing else can silence a buffer on this
    /// engine, so this is THE dropout figure -- counted since the render
    /// was first timed, and shown nowhere until now.
    pub overruns: u64,
    /// Buffers the monitor could not fill from the cue ring, since the app
    /// started. Priming a fresh or re-opened phones device does not count.
    pub phones_starved: u64,
    /// What the last rendered buffer cost.
    pub render_nanos: u64,
    /// The worst any buffer has cost since the app started.
    pub render_max_nanos: u64,
    /// Frames in the last rendered buffer, and the rate it plays at: the
    /// denominator of the budget.
    pub buffer_frames: u64,
    pub device_rate: f64,
    /// Where the last buffer's time went.
    pub stages: StageNanos,
}

impl AudioHealth {
    /// The share of the last buffer's own playing time that rendering it
    /// took. Above one the render cannot keep up. `None` before the first
    /// buffer, or from a device that reports no rate.
    pub fn budget_used(&self) -> Option<f64> {
        if self.buffer_frames == 0 || !(self.device_rate > 0.0) {
            return None;
        }
        let available_nanos = self.buffer_frames as f64 / self.device_rate * 1e9;
        (available_nanos > 0.0).then(|| self.render_nanos as f64 / available_nanos)
    }
}

/// How many loop rolls can be stacked before the oldest is dropped. A
/// hand can hold a few at once; past that the stack is a leak.
pub const ROLL_STACK_CAP: usize = 4;

/// How long a record takes to leave when another is loaded over it.
/// Forty milliseconds: long enough not to click, short enough that the
/// deck feels like it took the load immediately.
const LOAD_SWAP_SECS: f32 = 0.040;

const DECK_CHAIN_SLOTS: usize = 13;

/// The sources whose voices have no chain of their own: video, the pads
/// and the three synth tracks. Five and not seven, because the decks'
/// chains are the decks' own -- they swap with the voice and are silenced
/// on a load, which nothing else needs.
const SOURCE_CHAINS: usize = 5;

/// A deck's pre-fader tone chain: a fixed list of slots, walked in order.
/// Not a `Vec` -- sized once, at compile time, never resized. Today's
/// twelve slots are the whole roster and are permanently populated by
/// construction; a slot that can stand empty, or be reassigned, is a
/// separate decision for whenever growing the roster again asks for one.
struct DeckChain {
    slots: [EffectKind; DECK_CHAIN_SLOTS],
    /// One per slot, in the same order: the wet/dry mix and what,
    /// if anything, is done about the level that slot returns.
    levels: [SlotLevel; DECK_CHAIN_SLOTS],
    /// The policy a slot follows unless it has been pinned to one
    /// of its own. Off, so nothing changes until it is asked for.
    level_default: LevelMode,
    /// Whether every slot would hand its frame straight back and change
    /// nothing, as of this buffer's preparation. Decided once a buffer,
    /// at the END of `prepare_block` -- after each unit's own, because
    /// that is where the EQ decides its engagement and the crossovers
    /// glide, so any earlier lands a knob a buffer late -- and it holds
    /// for the buffer: nothing in a walk moves a settled ramp, and a
    /// command lands between buffers, before the next preparation.
    ///
    /// Eight chains sit in the frame loop now and most are idle most of
    /// the night; walking thirteen slots to return the input costs about
    /// a fifth of the loop, measured, which is what this buys back.
    idle: bool,
    /// Whether an idle chain is allowed to step aside. Always, outside a
    /// test that wants the walked chain to compare against.
    skip_when_idle: bool,
    /// Whether a freeze can ever be asked to hold on this chain. A deck's
    /// can; the six others have no command that reaches theirs, so the
    /// step-aside keeps the ring warm only where a hold could read it.
    holds: bool,
}

impl DeckChain {
    /// A deck's chain: every slot, and a freeze that can be asked to hold.
    fn new(sample_rate: f32) -> DeckChain {
        DeckChain {
            levels: std::array::from_fn(|_| SlotLevel::new()),
            level_default: LevelMode::Off,
            idle: false,
            skip_when_idle: true,
            holds: true,
            slots: [
                EffectKind::Eq(DeckEq::new(sample_rate)),
                EffectKind::Freeze(Freeze::new()),
                EffectKind::Echo(DeckEcho::new()),
                EffectKind::Flanger(Flanger::new()),
                EffectKind::Bitcrusher(Bitcrusher::new()),
                EffectKind::Tremolo(Tremolo::new()),
                EffectKind::Distortion(Distortion::new()),
                EffectKind::Phaser(Phaser::new()),
                EffectKind::Autopan(Autopan::new()),
                EffectKind::StereoWidth(StereoWidth::new()),
                EffectKind::PlateReverb(PlateReverb::new(sample_rate)),
                EffectKind::MoogLadder(MoogLadder::new()),
                EffectKind::Compressor(Compressor::new()),
            ],
        }
    }

    /// A chain for a source that is not a deck -- the mix, the pads, the
    /// synth tracks: the same slots, but no command can ask its freeze to
    /// hold, so an idle one need not keep the ring warm.
    fn for_source(sample_rate: f32) -> DeckChain {
        let mut chain = DeckChain::new(sample_rate);
        chain.holds = false;
        chain
    }

    #[inline]
    fn process(&mut self, frame: [f32; 2], device_rate: f32) -> [f32; 2] {
        if self.idle && self.skip_when_idle {
            // Every slot would hand the frame straight back; the two that
            // keep working while dry are fed by hand, in the walk's own
            // order. The same samples and the same state as the walk, bit
            // for bit -- a test holds the chain to that.
            if self.holds {
                self.freeze_mut().record(frame);
            }
            self.compressor_mut().listen(frame, device_rate);
            return frame;
        }
        let mut out = frame;
        for (slot, level) in self.slots.iter_mut().zip(&mut self.levels) {
            // What went in and what came back, so the slot's own policy
            // can blend them and, if it is asked to, correct the level.
            let dry = out;
            let wet = slot.process(dry, device_rate);
            out = level.apply(dry, wet, device_rate, self.level_default);
        }
        out
    }

    /// Everything this chain wants doing once a device buffer: rebuild
    /// what depends on the sample rate, and hand the musical clock to
    /// the stages that follow it.
    ///
    /// One method rather than a list at each call site. There are two
    /// chains now -- a deck's and the mix's -- and two hand-written
    /// lists of the same calls is how one of them quietly stops getting
    /// a new effect's preparation.
    fn prepare_block(
        &mut self,
        clock: &crate::wave_analysis::DeckClock,
        device_rate: f32,
        buffer_frames: usize,
    ) {
        // Filter coefficients are rebuilt once per buffer -- the trig is
        // the expensive part and a buffer is well under a millisecond.
        self.eq_mut().set_sample_rate(device_rate);
        self.eq_mut().prepare_block();
        // The reverb's tank lines are fixed lengths in FRAMES, not
        // musical time, so they take the same cheap rate compare.
        self.plate_reverb_mut().set_sample_rate(device_rate);
        // Ungridded or stopped, the counted beat -- the same default a
        // loop or a jump takes with no grid to rule on.
        let beat_secs = clock.beat_len().unwrap_or(60.0 / crate::decks::COUNTED_BPM);
        // The echo wants its beat in device FRAMES; the LFOs want the
        // whole clock, because a locked one has to know WHICH beat it is
        // on to sit right in a cycle that spans several.
        self.echo_mut().prepare_block(beat_secs * device_rate as f64);
        let buffer_secs = buffer_frames as f32 / device_rate;
        self.tremolo_mut().prepare_block(clock, buffer_secs);
        self.autopan_mut().prepare_block(clock, buffer_secs);
        self.flanger_mut().prepare_block(clock, buffer_secs);
        self.phaser_mut().prepare_block(clock, buffer_secs);
        // Last, after every unit's own preparation: the EQ's is where its
        // engagement is decided, and a verdict taken before it would be a
        // buffer stale.
        let level_default = self.level_default;
        self.idle = self
            .slots
            .iter()
            .zip(&self.levels)
            .all(|(slot, level)| slot.transparent() && level.transparent(level_default));
    }

    /// The policy the slots that have not been pinned follow.
    fn set_level_default(&mut self, mode: LevelMode) {
        self.level_default = mode;
    }

    fn level_default(&self) -> LevelMode {
        self.level_default
    }

    #[cfg(test)]
    fn idle(&self) -> bool {
        self.idle
    }

    #[cfg(test)]
    fn set_skip_when_idle(&mut self, on: bool) {
        self.skip_when_idle = on;
    }

    #[cfg(test)]
    fn slot_engaged(&mut self, slot: usize) -> bool {
        match &self.slots[slot] {
            EffectKind::Eq(eq) => !eq.at_unity(),
            EffectKind::Freeze(freeze) => freeze.held(),
            EffectKind::Echo(echo) => echo.engaged(),
            EffectKind::Flanger(x) => x.engaged(),
            EffectKind::Bitcrusher(x) => x.engaged(),
            EffectKind::Tremolo(x) => x.engaged(),
            EffectKind::Distortion(x) => x.engaged(),
            EffectKind::Phaser(x) => x.engaged(),
            EffectKind::Autopan(x) => x.engaged(),
            EffectKind::StereoWidth(x) => x.engaged(),
            EffectKind::PlateReverb(x) => x.engaged(),
            EffectKind::MoogLadder(x) => x.engaged(),
            EffectKind::Compressor(x) => x.engaged(),
        }
    }

    fn level_mut(&mut self, slot: usize) -> &mut SlotLevel {
        &mut self.levels[slot]
    }

    /// Drop every slot's measurement, so one record's levels are not
    /// carried into the next.
    fn reset_levels(&mut self) {
        for level in &mut self.levels {
            level.reset();
        }
    }

    // Fixed accessors. Every slot is populated with a known kind at a
    // known index by construction, so `unreachable!()` documents that
    // invariant rather than swallowing an error -- this stops being true
    // only once slots become dynamically reassignable.
    fn eq_mut(&mut self) -> &mut DeckEq {
        match &mut self.slots[0] {
            EffectKind::Eq(eq) => eq,
            _ => unreachable!(),
        }
    }
    fn freeze_mut(&mut self) -> &mut Freeze {
        match &mut self.slots[1] {
            EffectKind::Freeze(freeze) => freeze,
            _ => unreachable!(),
        }
    }
    fn echo_mut(&mut self) -> &mut DeckEcho {
        match &mut self.slots[2] {
            EffectKind::Echo(echo) => echo,
            _ => unreachable!(),
        }
    }
    fn flanger_mut(&mut self) -> &mut Flanger {
        match &mut self.slots[3] {
            EffectKind::Flanger(flanger) => flanger,
            _ => unreachable!(),
        }
    }
    fn bitcrusher_mut(&mut self) -> &mut Bitcrusher {
        match &mut self.slots[4] {
            EffectKind::Bitcrusher(bitcrusher) => bitcrusher,
            _ => unreachable!(),
        }
    }
    fn tremolo_mut(&mut self) -> &mut Tremolo {
        match &mut self.slots[5] {
            EffectKind::Tremolo(tremolo) => tremolo,
            _ => unreachable!(),
        }
    }
    fn distortion_mut(&mut self) -> &mut Distortion {
        match &mut self.slots[6] {
            EffectKind::Distortion(distortion) => distortion,
            _ => unreachable!(),
        }
    }
    fn phaser_mut(&mut self) -> &mut Phaser {
        match &mut self.slots[7] {
            EffectKind::Phaser(phaser) => phaser,
            _ => unreachable!(),
        }
    }
    fn autopan_mut(&mut self) -> &mut Autopan {
        match &mut self.slots[8] {
            EffectKind::Autopan(autopan) => autopan,
            _ => unreachable!(),
        }
    }
    fn stereo_width_mut(&mut self) -> &mut StereoWidth {
        match &mut self.slots[9] {
            EffectKind::StereoWidth(stereo_width) => stereo_width,
            _ => unreachable!(),
        }
    }
    fn plate_reverb_mut(&mut self) -> &mut PlateReverb {
        match &mut self.slots[10] {
            EffectKind::PlateReverb(plate_reverb) => plate_reverb,
            _ => unreachable!(),
        }
    }
    fn moog_ladder_mut(&mut self) -> &mut MoogLadder {
        match &mut self.slots[11] {
            EffectKind::MoogLadder(moog_ladder) => moog_ladder,
            _ => unreachable!(),
        }
    }

    fn compressor_mut(&mut self) -> &mut Compressor {
        match &mut self.slots[12] {
            EffectKind::Compressor(compressor) => compressor,
            _ => unreachable!(),
        }
    }
}

struct DeckVoice {
    pcm: Option<DeckPcm>,
    sync_locked: bool,
    stems: Option<Arc<TrackStems>>,
    /// The stem swap-in blend: 0.0 when a table first lands on a track that
    /// had none, ramping to 1.0 over [`STEM_SWAP_SECS`]. See
    /// [`DeckSource::frame`].
    stem_blend: ParamRamp,
    splat: Option<SplatState>,
    /// Playhead in SOURCE frames. Fractional, and free to run backwards
    /// under a hand on the waveform.
    pos: f64,
    playing: bool,
    /// The loop's IN and OUT in SOURCE frames, once a span is set. Frames
    /// rather than seconds because the render path is counting frames.
    loop_span: Option<(f64, f64)>,
    /// Armed by a commanded jump; never by the loop wrap, whose own
    /// crossfade pre-rolls into IN and would fight this one.
    seek_fade: Option<SeekFade>,
    gain: Ramp,
    mute: Ramp,
    /// Play and pause as a FADE, not a switch. A deck that stopped
    /// contributing on the instant the flag changed stepped straight to
    /// zero, which is the loudest click a transport can make.
    ///
    /// Separate from `gain` and `mute` because those are the operator's:
    /// folding a transport fade into either would fight the hand on the
    /// fader and would be undone by the next thing that touched it.
    transport: Ramp,
    /// The slip ghost: where the record WOULD be, advancing at its own
    /// latched rate whatever the real head is doing. Letting slip go lands
    /// the deck on it, so the track carries on as if the detour never
    /// happened.
    slip: Option<Ghost>,
    /// The censor armed the ghost, so releasing the censor puts it away
    /// again. A ghost the operator armed with SLIP outlives the censor.
    censor_owns_slip: bool,
    /// The rolls held over one another, newest last.
    rolls: RollGhosts,
    /// A record waiting for this one to finish leaving.
    pending: Option<PendingLoad>,
    /// Where the playhead was when pause was pressed, held until the fade
    /// reaches silence and then given back.
    ///
    /// Fading means reading and reading means advancing, so without this a
    /// pause leaves the record a few milliseconds past where the button
    /// went down. A deliberate seek cancels the promise -- see the brake,
    /// which is the one stop that means to keep the ground it covered.
    pause_at: Option<f64>,
    ended: bool,
    /// Tempo multiplier from the tempo slider / sync.
    rate: ParamRamp,
    /// Key shift as a frequency ratio, 2^(semitones/12). Pitch WITHOUT
    /// tempo: 1.0 is the track's own key. The exp2 happens in the setter so
    /// the render loop only ever multiplies.
    key_ratio: ParamRamp,
    /// Hold the key when the tempo moves.
    keylock: bool,
    scratch: ScratchRamp,
    /// True while the time stretcher owns the playhead.
    stretching: bool,
    /// The stretcher's tail after it hands over to the direct read: frames
    /// left, and the total, for the blend. The two paths hand the PLAYHEAD
    /// over exactly and disagree on PHASE, which the ordinary seek blend
    /// cannot hide -- both sides of THAT blend are the direct read, and
    /// what differs here is the overlap-add's own phase, which the source
    /// knows nothing about. So the stretcher keeps sounding, at its own
    /// place, while the direct read comes up underneath it.
    stretch_tail: Option<(f64, f64)>,
    stretch: Box<Stretcher>,
    reader: RateReader,
    /// The effect chain. Slot 0 IS the EQ this voice used to hold on its
    /// own -- the chain simply walks it first and then twelve more.
    chain: DeckChain,
    /// The grid this record is ruled by, for the musical clock below.
    /// `None` until an analysis lands, which is what makes every
    /// beat-locked stage fall back to a counted beat.
    grid: Option<TrackGrid>,
    /// Where the beat is at the END of this buffer, worked out once per
    /// buffer rather than per frame. The beat-locked LFOs and the echo
    /// read it; nothing else does.
    clock: DeckClock,
    stem_gain: [ParamRamp; STEM_COUNT],
    /// The autopilot's blend overlay on the stem lanes: multiplies the
    /// operator's gains, never moves them. 1.0 = hands off.
    blend_stem: [ParamRamp; STEM_COUNT],
}

impl DeckVoice {
    fn new() -> DeckVoice {
        DeckVoice {
            pcm: None,
            sync_locked: false,
            stems: None,
            stem_blend: ParamRamp::at(1.0),
            splat: None,
            pos: 0.0,
            playing: false,
            loop_span: None,
            seek_fade: None,
            gain: Ramp::at(1.0),
            mute: Ramp::at(1.0),
            transport: Ramp::at(0.0),
            slip: None,
            censor_owns_slip: false,
            rolls: RollGhosts::default(),
            pending: None,
            pause_at: None,
            ended: false,
            rate: ParamRamp::at(1.0),
            key_ratio: ParamRamp::at(1.0),
            keylock: true,
            scratch: ScratchRamp::default(),
            stretching: false,
            stretch_tail: None,
            stretch: Box::new(Stretcher::new()),
            reader: RateReader::default(),
            chain: DeckChain::new(48_000.0),
            grid: None,
            clock: DeckClock::default(),
            stem_gain: [ParamRamp::at(1.0); STEM_COUNT],
            blend_stem: [ParamRamp::at(1.0); STEM_COUNT],
        }
    }

    /// Snap the whole blend overlay home instantly — a fresh track never
    /// inherits a transition's ducking.
    fn reset_blend(&mut self) {
        self.blend_stem = [ParamRamp::at(1.0); STEM_COUNT];
        self.chain.eq_mut().reset_blend();
    }

    /// Frames the voice can read right now — the decoded edge while a
    /// track is still streaming in, which is what seeks clamp to.
    fn frame_count(&self) -> usize {
        self.pcm.as_ref().map(DeckPcm::len).unwrap_or(0)
    }

    /// Move the playhead and drop every bit of streaming state that was
    /// tied to the old position.
    fn seek_frames(&mut self, frames: f64) {
        let len = self.frame_count() as f64;
        self.pos = frames.clamp(0.0, len);
        if let Some(splat) = self.splat.as_mut().filter(|splat| splat.active) {
            splat.master_frames = self.pos;
            splat.phase_fade = None;
        }
        self.stretch.reset_to(self.pos);
        self.reader.reset();
        // A tail belongs to the place it was leaving; after a jump it would
        // be the old place blended under the new one.
        self.stretch_tail = None;
        self.ended = false;
    }

    /// Where the playhead really is, whichever path is driving it.
    fn playhead_frames(&self) -> f64 {
        if let Some(splat) = self.splat.as_ref().filter(|splat| splat.active) {
            return splat.playhead_frames();
        }
        if self.stretching {
            self.stretch.position()
        } else {
            self.pos
        }
    }

    /// Keep `from` sounding for a few milliseconds so the jump that just
    /// happened lands as a blend. Only a PLAYING deck needs one — a paused
    /// deck's jump makes no sound to soften.
    fn arm_seek_fade(&mut self, from: f64) {
        if !self.playing {
            return;
        }
        let Some(pcm) = self.pcm.as_ref() else { return };
        let total = (SEEK_XFADE_SECS * pcm.sample_rate().max(1) as f64).max(1.0);
        self.seek_fade = Some(SeekFade { pos: from, left: total, total });
    }
}

#[inline]
fn splat_stem_frame(stems: Option<&TrackStems>, stem: usize, index: usize) -> [f32; 2] {
    let Some(stems) = stems else { return [0.0, 0.0] };
    let chunk = index / stems.chunk_frames;
    let offset = index - chunk * stems.chunk_frames;
    let Some(Some(block)) = stems.lanes[stem].get(chunk) else { return [0.0, 0.0] };
    let Some(frame) = block.get(offset) else { return [0.0, 0.0] };
    let scale = STEM_CHUNK_HEADROOM / 32768.0;
    [frame[0] as f32 * scale, frame[1] as f32 * scale]
}

#[inline]
fn splat_cell_frame(
    row: SplatRow,
    cell: RowCell,
    master_frames: f64,
    pcm: &DeckPcm,
    stems: Option<&TrackStems>,
    stem_gain: [f32; STEM_COUNT],
) -> [f32; 2] {
    let offset = (master_frames - cell.anchor_frames).rem_euclid(cell.len_frames.max(1.0));
    let position = cell.start_frames + offset;
    let index = position.floor().max(0.0) as usize;
    let fraction = (position - index as f64) as f32;
    let next_offset = (offset + 1.0).rem_euclid(cell.len_frames.max(1.0));
    let next = (cell.start_frames + next_offset).floor().max(0.0) as usize;
    let read = |at| match row.stem() {
        Some(stem) => {
            let mut frame = splat_stem_frame(stems, stem.index(), at);
            let gain = stem_gain[stem.index()];
            frame[0] *= gain;
            frame[1] *= gain;
            frame
        }
        None => pcm.frame_f32(at),
    };
    let a = read(index);
    let b = read(next);
    [
        a[0] + (b[0] - a[0]) * fraction,
        a[1] + (b[1] - a[1]) * fraction,
    ]
}

/// Splat reads bypass the stretcher and rate reader: every source position is
/// a pure function of the shared master clock, so feeding discontinuous row
/// loops to a stateful monotonic reader would weaken the phase guarantee.
fn render_splat_source(
    splat: &mut SplatState,
    pcm: &DeckPcm,
    stems: Option<&TrackStems>,
    stem_gain: [f32; STEM_COUNT],
    source_step: f64,
) -> [f32; 2] {
    let master = splat.master_frames;
    let fade_frames = (SPLAT_XFADE_SECS * pcm.sample_rate().max(1) as f64).max(1.0);
    let mut sum = [0.0f32; 2];
    let mut landed = false;
    for row in SplatRow::ALL {
        let voice = &mut splat.rows[row.index()];
        if let Some(queued) = voice.queued.filter(|queued| master >= queued.at_frames) {
            voice.queued = None;
            voice.fade = Some(SplatFade {
                outgoing: voice.cell,
                incoming: queued.cell,
                start_frames: queued.at_frames,
                len_frames: fade_frames,
            });
            voice.cell = queued.cell;
            landed = true;
        }
        let mut frame = if let Some(fade) = voice.fade {
            let phase = ((master - fade.start_frames) / fade.len_frames).clamp(0.0, 1.0) as f32;
            let outgoing = fade.outgoing.map_or([0.0, 0.0], |cell| {
                splat_cell_frame(row, cell, master, pcm, stems, stem_gain)
            });
            let incoming = fade.incoming.map_or([0.0, 0.0], |cell| {
                splat_cell_frame(row, cell, master, pcm, stems, stem_gain)
            });
            let angle = phase * std::f32::consts::FRAC_PI_2;
            let out_gain = angle.cos();
            let in_gain = angle.sin();
            if phase >= 1.0 {
                voice.fade = None;
            }
            [
                outgoing[0] * out_gain + incoming[0] * in_gain,
                outgoing[1] * out_gain + incoming[1] * in_gain,
            ]
        } else {
            voice.cell.map_or([0.0, 0.0], |cell| {
                splat_cell_frame(row, cell, master, pcm, stems, stem_gain)
            })
        };
        if let Some(fade) = &splat.phase_fade {
            let old_voice = fade.rows[row.index()];
            let read = |cell| splat_cell_frame(row, cell, fade.pos, pcm, stems, stem_gain);
            let old = if let Some(row_fade) = old_voice.fade {
                let phase = ((fade.pos - row_fade.start_frames) / row_fade.len_frames)
                    .clamp(0.0, 1.0) as f32;
                let angle = phase * std::f32::consts::FRAC_PI_2;
                let outgoing = row_fade.outgoing.map_or([0.0; 2], read);
                let incoming = row_fade.incoming.map_or([0.0; 2], read);
                std::array::from_fn(|i| outgoing[i] * angle.cos() + incoming[i] * angle.sin())
            } else {
                old_voice.cell.map_or([0.0; 2], read)
            };
            let old_gain = (fade.left / fade.total).clamp(0.0, 1.0) as f32;
            for channel in 0..2 {
                frame[channel] = old[channel] * old_gain + frame[channel] * (1.0 - old_gain);
            }
        }
        sum[0] += frame[0];
        sum[1] += frame[1];
    }
    splat.master_frames += source_step;
    if let Some(fade) = &mut splat.phase_fade {
        fade.pos += source_step;
        fade.left -= source_step.abs();
        if fade.left <= 0.0 {
            splat.phase_fade = None;
        }
    }
    if landed {
        splat.revalidate_view();
    }
    sum
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

/// The pre-listen file player: a cue-bus-only voice over a decoded track.
/// It advances on the main device clock like every other voice (`render`
/// is the position truth) but sums into the cue ring, never the program.
struct PreviewVoice {
    pcm: Option<Arc<TrackPcm>>,
    /// Q32.32 source-frame cursor.
    cursor_fp: u64,
    playing: bool,
    /// Play/pause declick.
    gain: Ramp,
    ended: bool,
}

impl PreviewVoice {
    fn new() -> PreviewVoice {
        PreviewVoice {
            pcm: None,
            cursor_fp: 0,
            playing: false,
            gain: Ramp::at(0.0),
            ended: false,
        }
    }
}

struct ScorePreviewVoice {
    piano: Box<Piano>,
    kit: DrumKit,
    drum_bank: Option<Arc<SampleBank>>,
    sequence: Option<Arc<PreviewSequence>>,
    pos: u64,
    playing: bool,
    gain: ParamRamp,
    scratch: Vec<[f32; 2]>,
    piano_left: Vec<f32>,
    piano_right: Vec<f32>,
    piano_events: Vec<PianoTimedEvent>,
    sample_rate: u32,
}

impl ScorePreviewVoice {
    fn new(sample_rate: u32) -> Self {
        Self {
            piano: Box::new(Piano::new(sample_rate as f32)),
            kit: DrumKit::new(sample_rate as f32),
            drum_bank: None,
            sequence: None,
            pos: 0,
            playing: false,
            gain: ParamRamp::at(SCORE_PREVIEW_GAIN),
            scratch: vec![[0.0; 2]; SCORE_PREVIEW_MAX_BLOCK],
            piano_left: vec![0.0; SCORE_PREVIEW_MAX_BLOCK],
            piano_right: vec![0.0; SCORE_PREVIEW_MAX_BLOCK],
            piano_events: Vec::new(),
            sample_rate,
        }
    }

    fn replace_instruments(&mut self, piano: Box<Piano>, sample_rate: u32) -> Box<Piano> {
        let retired = std::mem::replace(&mut self.piano, piano);
        let mut kit = DrumKit::new(sample_rate as f32);
        if let Some(bank) = &self.drum_bank {
            kit.set_bank(bank.clone());
        }
        self.kit = kit;
        self.sample_rate = sample_rate;
        retired
    }

    fn set_drum_bank(&mut self, bank: Arc<SampleBank>) -> Option<Arc<SampleBank>> {
        self.kit.set_bank(bank.clone());
        self.drum_bank.replace(bank)
    }

    fn silence_piano(&mut self) {
        let mut left = [0.0];
        let mut right = [0.0];
        self.piano.process(
            &[PianoTimedEvent { offset: 0, event: PianoEvent::AllSoundOff }],
            &mut left,
            &mut right,
        );
    }

    fn stop(&mut self, reset_position: bool) {
        self.playing = false;
        if reset_position {
            self.pos = 0;
        }
        self.silence_piano();
        self.kit.all_off();
    }

    fn required_event_capacity(sequence: &PreviewSequence) -> usize {
        // One host block can cross several very short synthetic loops. Size
        // for every possible repeat here on the UI thread so `push` below
        // retains its no-allocation contract even for such test sequences.
        let repeats = (SCORE_PREVIEW_MAX_BLOCK as u64 / sequence.len_frames.max(1))
            .saturating_add(2) as usize;
        sequence
            .events
            .len()
            .saturating_add(1)
            .saturating_mul(repeats)
            .saturating_add(2)
    }

    fn play(&mut self, sequence: Arc<PreviewSequence>) -> Option<Arc<PreviewSequence>> {
        self.stop(true);
        debug_assert!(
            self.piano_events.capacity() >= Self::required_event_capacity(&sequence),
            "score preview event storage must be prepared off the audio thread"
        );
        let retired = self.sequence.replace(sequence);
        self.pos = 0;
        self.playing = true;
        self.gain.jump(SCORE_PREVIEW_GAIN);
        retired
    }

    /// Fill the pre-master preview block. Trigger discovery is sample-based
    /// so kit hits and loop resets land exactly; the piano receives the same
    /// offsets in one allocation-free timed-event call.
    fn render_block(&mut self, frames: usize, device_rate: f64) {
        let frames = frames.min(SCORE_PREVIEW_MAX_BLOCK);
        self.scratch[..frames].fill([0.0; 2]);
        self.piano_left[..frames].fill(0.0);
        self.piano_right[..frames].fill(0.0);
        self.piano_events.clear();
        if frames == 0 || !self.playing {
            return;
        }
        let Some(sequence) = self.sequence.as_ref() else {
            self.playing = false;
            return;
        };
        if sequence.sample_rate != self.sample_rate
            || (device_rate - sequence.sample_rate as f64).abs() >= 0.5
        {
            self.playing = false;
            self.kit.all_off();
            self.silence_piano();
            return;
        }

        let len = sequence.len_frames.max(1);
        let mut event_index = sequence.events.partition_point(|event| event.0 < self.pos);
        let mut reset_after_block = false;
        for frame in 0..frames {
            while let Some((at, event)) = sequence.events.get(event_index) {
                if *at != self.pos {
                    break;
                }
                match *event {
                    PreviewEvent::Piano(event) => self.piano_events.push(PianoTimedEvent {
                        offset: frame as u32,
                        event,
                    }),
                    PreviewEvent::Drum { voice, velocity } => self.kit.trigger(voice, velocity),
                }
                event_index += 1;
            }
            self.kit.process(std::slice::from_mut(&mut self.scratch[frame]));
            self.pos = self.pos.saturating_add(1);
            if self.pos < len {
                continue;
            }

            self.kit.all_off();
            if frame + 1 < frames {
                self.piano_events.push(PianoTimedEvent {
                    offset: (frame + 1) as u32,
                    event: PianoEvent::AllSoundOff,
                });
            } else {
                reset_after_block = true;
            }
            if sequence.looped {
                self.pos = 0;
                event_index = 0;
            } else {
                self.pos = len;
                self.playing = false;
                break;
            }
        }

        self.piano.process(
            &self.piano_events,
            &mut self.piano_left[..frames],
            &mut self.piano_right[..frames],
        );
        for frame in 0..frames {
            let gain = self.gain.tick(self.sample_rate as f32);
            self.scratch[frame][0] += self.piano_left[frame];
            self.scratch[frame][1] += self.piano_right[frame];
            self.scratch[frame][0] *= gain;
            self.scratch[frame][1] *= gain;
        }
        if reset_after_block {
            self.silence_piano();
        }
    }
}

/// The one-way street from `render` (device slot 0) to the phones callback
/// (device slot 1): a lock-free ring of packed stereo frames. The producer
/// never waits, the consumer never touches the mix state — on starvation
/// the phones go silent and re-prime, and the program never hears a thing.
/// The two devices free-run at their own rates; the consumer's fill servo
/// (in [`CueRing::consume`]) absorbs both the nominal mismatch and the
/// drift.
pub struct CueRing {
    /// L,R f32 bit patterns packed into one word: a frame is one atomic,
    /// so a frame can never tear.
    buf: Box<[AtomicU64]>,
    /// Absolute frames produced, published once per rendered buffer.
    write_pos: AtomicU64,
    /// Producer device rate bits, for the consumer's nominal ratio.
    main_rate_bits: AtomicU64,
    /// True only while a phones device is requested at slot 1. Gates the
    /// producer, so an unconfigured phones path costs one load per buffer.
    armed: AtomicBool,
    /// Headphone volume, f32 bits. The UI writes, the consumer smooths.
    volume_bits: AtomicU32,
    /// Buffers the monitor could not fill. Counted HERE because the ring is
    /// the only thing that knows it ran dry; priming a fresh or re-opened
    /// device is not a miss and does not count.
    starved: AtomicU64,
}

impl CueRing {
    fn new() -> CueRing {
        CueRing {
            starved: AtomicU64::new(0),
            buf: (0..CUE_RING_FRAMES).map(|_| AtomicU64::new(0)).collect(),
            write_pos: AtomicU64::new(0),
            main_rate_bits: AtomicU64::new(0),
            armed: AtomicBool::new(false),
            volume_bits: AtomicU32::new(1.0f32.to_bits()),
        }
    }

    #[inline]
    fn push(&self, pos: u64, left: f32, right: f32) {
        // Guarded going IN and not coming out: what is stored here is read
        // by a second device on a second thread, interpolated between two
        // neighbours, and one sample that is not a number would spread
        // over both of them.
        let packed = (audible(left).to_bits() as u64) | ((audible(right).to_bits() as u64) << 32);
        self.buf[(pos as usize) & (CUE_RING_FRAMES - 1)].store(packed, Ordering::Relaxed);
    }

    #[inline]
    fn frame_at(&self, pos: u64) -> (f32, f32) {
        let packed = self.buf[(pos as usize) & (CUE_RING_FRAMES - 1)].load(Ordering::Relaxed);
        (f32::from_bits(packed as u32), f32::from_bits((packed >> 32) as u32))
    }

    /// Drain the ring into one phones-device buffer, resampling from the
    /// producer's rate to `cue_rate`. The buffer must arrive zeroed; on any
    /// shortfall the remainder stays silent and the state re-primes.
    pub fn consume(&self, state: &mut CueReadState, cue_rate: f64, output: &mut AudioBuffer) {
        let frames = output.frame_count();
        let channels = output.channel_count();
        if frames == 0 || channels == 0 || cue_rate <= 0.0 {
            return;
        }
        if !self.armed.load(Ordering::Relaxed) {
            state.priming = true;
            return;
        }
        let wp = self.write_pos.load(Ordering::Acquire);
        if state.priming {
            if wp < CUE_TARGET_FRAMES {
                return;
            }
            state.cursor_fp = (wp - CUE_TARGET_FRAMES) << 32;
            state.priming = false;
            // Fade in from silence on every (re)start — a device open is
            // never a click.
            state.volume = 0.0;
        }
        let main_rate = f64::from_bits(self.main_rate_bits.load(Ordering::Relaxed));
        if main_rate <= 0.0 {
            state.priming = true;
            return;
        }
        // Lapped by the producer (a stalled consumer): jump back to depth.
        if wp.saturating_sub(state.cursor_fp >> 32) as usize > CUE_RING_FRAMES - 1_024 {
            state.cursor_fp = (wp - CUE_TARGET_FRAMES) << 32;
        }
        // The fill servo: trim the nominal ratio a hair (±0.05%) toward the
        // target depth, so mismatched rates and drifting clocks converge on
        // a steady offset instead of stepping through drops and underruns.
        let avail = wp.saturating_sub(state.cursor_fp >> 32);
        let fill_err =
            (avail as f64 - CUE_TARGET_FRAMES as f64) / CUE_TARGET_FRAMES as f64;
        let ratio = (main_rate / cue_rate) * (1.0 + fill_err.clamp(-0.25, 0.25) * 0.002);
        let step = ((ratio * FP_ONE as f64) as u64).max(1);
        let target_volume = f32::from_bits(self.volume_bits.load(Ordering::Relaxed));
        // ~1 ms one-pole: fast enough to feel instant on the slider, slow
        // enough to swallow the step.
        let volume_pole = (1.0 / (0.001 * cue_rate)).min(1.0) as f32;
        for frame in 0..frames {
            let index = state.cursor_fp >> 32;
            if index + 1 >= wp {
                // Ran dry: the rest of the buffer stays silent and the
                // next callback re-primes at depth. Counted, because a
                // monitor that keeps running out is the one number that
                // says so -- priming a fresh device is not this.
                self.starved.fetch_add(1, Ordering::Relaxed);
                state.priming = true;
                break;
            }
            let (al, ar) = self.frame_at(index);
            let (bl, br) = self.frame_at(index + 1);
            let fraction = (state.cursor_fp & (FP_ONE - 1)) as f32 / FP_ONE as f32;
            state.volume += (target_volume - state.volume) * volume_pole;
            let l = (al + (bl - al) * fraction) * state.volume;
            let r = (ar + (br - ar) * fraction) * state.volume;
            for channel in 0..channels {
                output.channel_mut(channel)[frame] = if channel == 0 { l } else { r };
            }
            state.cursor_fp = state.cursor_fp.saturating_add(step);
        }
    }
}

/// The phones callback's private cursor over the ring. Lives in the slot-1
/// closure; survives device swaps, and a swap simply re-primes.
pub struct CueReadState {
    /// Q32.32 cursor over ABSOLUTE produced frames.
    cursor_fp: u64,
    /// Waiting for the ring to reach depth before (re)starting.
    priming: bool,
    /// One-pole smoothed volume, so the modal slider never zips.
    volume: f32,
}

impl Default for CueReadState {
    fn default() -> CueReadState {
        CueReadState { cursor_fp: 0, priming: true, volume: 0.0 }
    }
}

/// A video transition as armed: carried in a command, then owned by the
/// callback's clock.
#[derive(Clone, Copy, Debug)]
pub struct ScheduledVideoTransition {
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
    rendered_frames: u64,
    scheduled_video: Option<ScheduledVideoTransition>,
    /// Per-SLOT headphone cue toggles: the cue button belongs to the
    /// channel strip, not the record, so `swap_decks` leaves these alone.
    cue_deck: [bool; 2],
    cue_mode: CueMode,
    preview: PreviewVoice,
    /// The headphone bus ends on a limiter of its own, on the master's
    /// settings. Two instances and not one because they carry different
    /// signals -- but alike, for two reasons. A clamp is a clipper, so the
    /// phones would grit where the room does not; and a limiter looks
    /// ahead, so a bus with one runs later than a bus without, and a
    /// headphone feed that is a look-ahead EARLY is a beat-match error
    /// nobody can see.
    cue_limiter: crate::music_dsp::Limiter,
    score_preview: ScorePreviewVoice,
    synth: SynthRack,
    program_mix: ProgramMix,
    /// The mix's own chain, between the master gain and the bus dynamics:
    /// after everything has been summed, so an effect here hears the
    /// whole room, and before the limiter, so whatever it adds is still
    /// caught.
    master_chain: DeckChain,
    /// The chains of the five sources whose voices have none of their
    /// own. Each runs on its source before the source enters the program
    /// mix -- the same seat ahead of the fader that the deck chains have.
    source_chains: [DeckChain; SOURCE_CHAINS],
    /// Which deck's grid every chain with no tempo of its own follows.
    /// The mix has none, and neither has a pad or a synth track; they
    /// borrow deck A's until something asks otherwise.
    chain_clock_deck: DeckId,
}

impl MixState {
    /// The chain a target names. Exhaustive and infallible on purpose:
    /// the audio callback must not regain a way to unwind, so there is no
    /// index to be out of range and no `Option` to unwrap.
    fn chain_mut(&mut self, target: ChainTarget) -> &mut DeckChain {
        match target {
            ChainTarget::DeckA => &mut self.decks[0].chain,
            ChainTarget::DeckB => &mut self.decks[1].chain,
            ChainTarget::Master => &mut self.master_chain,
            ChainTarget::Video => &mut self.source_chains[0],
            ChainTarget::Sfx => &mut self.source_chains[1],
            ChainTarget::Piano => &mut self.source_chains[2],
            ChainTarget::Ironfish => &mut self.source_chains[3],
            ChainTarget::Drums => &mut self.source_chains[4],
        }
    }

    /// Whether any effect on one target's chain is sounding at all.
    #[cfg(test)]
    fn chain_engaged(&mut self, target: ChainTarget) -> bool {
        let chain = self.chain_mut(target);
        (0..DECK_CHAIN_SLOTS).any(|slot| chain.slot_engaged(slot))
    }

    /// Let every chain step aside when idle, or forbid it -- for the
    /// report that measures what the step-aside is worth.
    #[cfg(test)]
    fn set_skip_when_idle(&mut self, on: bool) {
        for target in ChainTarget::ALL {
            self.chain_mut(target).set_skip_when_idle(on);
        }
    }

    fn new() -> MixState {
        MixState {
            video: [VideoBus::new(), VideoBus::new()],
            video_mute: Ramp::at(1.0),
            decks: [DeckVoice::new(), DeckVoice::new()],
            fader: Ramp::at(0.0),
            curve: FadeCurve::EqualPower,
            sfx: Vec::with_capacity(MAX_SFX_VOICES),
            master: Ramp::at(0.9),
            rendered_frames: 0,
            scheduled_video: None,
            cue_deck: [false; 2],
            cue_mode: CueMode::default(),
            preview: PreviewVoice::new(),
            // The rate is not known until the first buffer; the look-ahead
            // re-windows itself then, allocation-free.
            cue_limiter: crate::music_dsp::Limiter::new(0.0),
            score_preview: ScorePreviewVoice::new(48_000),
            synth: SynthRack::new(48_000),
            program_mix: ProgramMix::new(),
            master_chain: DeckChain::for_source(48_000.0),
            source_chains: std::array::from_fn(|_| DeckChain::for_source(48_000.0)),
            chain_clock_deck: DeckId::A,
        }
    }
}

/// Peak meters (f32 bits): master, video, deck A, deck B, sfx.
pub const METER_MASTER: usize = 0;
pub const METER_VIDEO: usize = 1;
pub const METER_DECK_A: usize = 2;
pub const METER_DECK_B: usize = 3;
pub const METER_SFX: usize = 4;

/// Commands queued in one go before the callback drains them. Every UI
/// frame drains the events and re-sends what did not fit, so this only
/// has to cover a burst between two frames.
const CMD_RING_SLOTS: usize = 1024;
/// Ended decks and voices plus retired payloads, in the other direction.
const EVENT_RING_SLOTS: usize = 1024;
/// SFX voice storage is reserved once, so a voice start never grows the
/// vector on the audio thread.
const MAX_SFX_VOICES: usize = 64;

/// Every change the UI can ask of the audio state. Payloads are moved in
/// whole; the audio thread never allocates for one and never frees one —
/// what a command replaces comes back to the UI as a [`Retired`] payload.
/// Where an effect chain sits: one of the seven sources that meet at the
/// mix, or the mix itself.
///
/// The two decks resolve to the chains their voices already own -- those
/// run ahead of the fader and swap with the voice, and every golden
/// reference is recorded through them. The other five sources get chains
/// of their own. Flat rather than `Strip(..) | Master`: the file tags, the
/// chip labels and `ALL` all want one enumeration, and `deck()` and
/// `strip()` are its two projections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainTarget {
    Video,
    DeckA,
    DeckB,
    Sfx,
    Piano,
    Ironfish,
    Drums,
    Master,
}

impl ChainTarget {
    pub const ALL: [ChainTarget; 8] = [
        Self::Video,
        Self::DeckA,
        Self::DeckB,
        Self::Sfx,
        Self::Piano,
        Self::Ironfish,
        Self::Drums,
        Self::Master,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// A number read from a file becomes a target, or nothing: never the
    /// nearest one.
    pub fn from_index(index: usize) -> Option<ChainTarget> {
        Self::ALL.get(index).copied()
    }

    /// The word a settings file holds for this target. Frozen the way a
    /// settings slug is; the decks keep the two letters they always had.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::DeckA => "a",
            Self::DeckB => "b",
            Self::Sfx => "sfx",
            Self::Piano => "piano",
            Self::Ironfish => "ironfish",
            Self::Drums => "drums",
            Self::Master => "master",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Video => "VIDEO",
            Self::DeckA => "A",
            Self::DeckB => "B",
            Self::Sfx => "SFX",
            Self::Piano => "PIANO",
            Self::Ironfish => "IRON",
            Self::Drums => "DRUMS",
            Self::Master => "MASTER",
        }
    }

    pub fn deck(self) -> Option<DeckId> {
        match self {
            Self::DeckA => Some(DeckId::A),
            Self::DeckB => Some(DeckId::B),
            _ => None,
        }
    }

    /// The program strip this target's source feeds; the mix has none.
    pub fn strip(self) -> Option<StripId> {
        match self {
            Self::Video => Some(StripId::Video),
            Self::DeckA => Some(StripId::DjA),
            Self::DeckB => Some(StripId::DjB),
            Self::Sfx => Some(StripId::Sfx),
            Self::Piano => Some(StripId::Piano),
            Self::Ironfish => Some(StripId::Ironfish),
            Self::Drums => Some(StripId::Drums),
            Self::Master => None,
        }
    }
}

impl From<DeckId> for ChainTarget {
    fn from(deck: DeckId) -> Self {
        match deck {
            DeckId::A => Self::DeckA,
            DeckId::B => Self::DeckB,
        }
    }
}

/// One effect parameter, on its way to the audio thread.
///
/// ONE command variant carries all of these rather than seventy of their
/// own. They are all the same shape -- a deck, a slot, a number -- and
/// seventy variants would be seventy places to forget when a slot gains a
/// knob. Small and `Copy`, so the ring moves it by value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EffectParam {
    /// The echo's delay as a beat fraction, or none to let it run free.
    Echo(Option<(u32, u32)>),
    /// The autopilot's filter overlay, which multiplies the operator's
    /// sweep rather than moving it.
    BlendFilter(f32),
    Resonance(f32),
    EchoFeedback(f32),
    EchoPingpong(bool),
    Flanger(bool),
    FlangerRate(f32),
    FlangerDepth(f32),
    FlangerFeedback(f32),
    FlangerSyncUnits(u32),
    FlangerBeatOffset(f32),
    Bitcrusher(bool),
    BitcrusherRate(f32),
    BitcrusherBits(f32),
    Tremolo(bool),
    TremoloRate(f32),
    TremoloDepth(f32),
    TremoloSyncUnits(u32),
    TremoloBeatOffset(f32),
    Distortion(bool),
    DistortionDrive(f32),
    Phaser(bool),
    PhaserRate(f32),
    PhaserFeedback(f32),
    PhaserSyncUnits(u32),
    PhaserBeatOffset(f32),
    Autopan(bool),
    Compressor(bool),
    CompressorThreshold(f32),
    CompressorRatio(f32),
    AutopanRate(f32),
    EchoMix(f32),
    EchoLevelMode(LevelMode),
    EchoCeiling(f32),
    FlangerMix(f32),
    FlangerLevelMode(LevelMode),
    FlangerCeiling(f32),
    BitcrusherMix(f32),
    BitcrusherLevelMode(LevelMode),
    BitcrusherCeiling(f32),
    TremoloMix(f32),
    TremoloLevelMode(LevelMode),
    TremoloCeiling(f32),
    DistortionMix(f32),
    DistortionLevelMode(LevelMode),
    DistortionCeiling(f32),
    PhaserMix(f32),
    PhaserLevelMode(LevelMode),
    PhaserCeiling(f32),
    AutopanMix(f32),
    AutopanLevelMode(LevelMode),
    AutopanCeiling(f32),
    StereoWidthMix(f32),
    StereoWidthLevelMode(LevelMode),
    StereoWidthCeiling(f32),
    PlateReverbMix(f32),
    PlateReverbLevelMode(LevelMode),
    PlateReverbCeiling(f32),
    MoogLadderMix(f32),
    MoogLadderLevelMode(LevelMode),
    MoogLadderCeiling(f32),
    LevelDefault(LevelMode),
    AutopanSyncUnits(u32),
    AutopanBeatOffset(f32),
    StereoWidth(bool),
    StereoWidthAmount(f32),
    PlateReverb(bool),
    PlateReverbSize(f32),
    MoogLadder(bool),
    MoogLadderCutoff(f32),
    MoogLadderResonance(f32),
    Crossovers(f32, f32),
}

pub enum MixCmd {
    OpenSlot(SlotId),
    CloseSlot(SlotId),
    FadeSlots { from: Option<SlotId>, to: SlotId, secs: f32 },
    SetVideoMix(f32),
    SetVideoMuted(bool),
    ScheduleVideo(ScheduledVideoTransition),
    CancelVideo(VideoTransitionId),
    InstallDeck { deck: DeckId, pcm: DeckPcm },
    GrowStream { deck: DeckId, stream: Arc<StreamPcm> },
    CompleteDeck { deck: DeckId, pcm: Arc<TrackPcm> },
    ClearDeck(DeckId),
    InstallStems { deck: DeckId, stems: Arc<TrackStems> },
    ClearStems(DeckId),
    SetPlaying { deck: DeckId, playing: bool },
    SetSplat { deck: DeckId, grid: Arc<SplatGrid>, frames: Box<SplatFrames> },
    SetSplatEnabled { deck: DeckId, on: bool },
    SplatLaunch { deck: DeckId, row: SplatRow, col: u8, part: SplatPart },
    SplatStopRow { deck: DeckId, row: SplatRow, timed: bool },
    SplatLaunchScene { deck: DeckId, col: u8 },
    SplatStopAll { deck: DeckId, timed: bool },
    SeekFraction { deck: DeckId, fraction: f64 },
    SeekSeconds { deck: DeckId, secs: f64 },
    /// Move by `delta_secs` from the playhead AS IT IS when this lands.
    SeekRelative { deck: DeckId, delta_secs: f64 },
    SetRate { deck: DeckId, rate: f32 },
    SetSyncLock { deck: DeckId, on: bool },
    SetKeyRatio { deck: DeckId, ratio: f32 },
    SetKeylock { deck: DeckId, on: bool },
    Scratch { deck: DeckId, motion: ScratchMotion },
    SetEqBand { deck: DeckId, band: usize, gain: f32 },
    SetFilter { deck: DeckId, position: f32 },
    SetStemGain { deck: DeckId, stem: usize, gain: f32 },
    SetLoopSpan {
        deck: DeckId,
        span: Option<(f64, f64)>,
        /// What the change MEANT for the playhead. See [`crate::decks::LoopSeek`].
        seek: crate::decks::LoopSeek,
    },
    SetMute { deck: DeckId, muted: bool },
    SetGain { deck: DeckId, gain: f32 },
    /// One knob on one slot of one chain -- a deck's, a source's or the
    /// mix's. See [`EffectParam`] for why the knobs share a variant and
    /// [`ChainTarget`] for why the chains do.
    ChainEffect { target: ChainTarget, param: EffectParam },
    ChainClockDeck(DeckId),
    SetGrid { deck: DeckId, grid: Option<TrackGrid> },
    SetSlip { deck: DeckId, on: bool, adopt: bool },
    SetCensor { deck: DeckId, on: bool },
    PushRoll { deck: DeckId },
    PopRoll { deck: DeckId, parent: Option<(f64, f64)>, adopt: bool },
    Spin { deck: DeckId, motion: SpinMotion },
    /// Source seconds to hold, or none to let go. The conversion into
    /// OUTPUT seconds happens on the audio side, where the platter's own
    /// rate and the device rate both already live.
    SetFreeze { deck: DeckId, secs: Option<f64> },
    InstallOver { deck: DeckId, pcm: DeckPcm, keep_playing: bool },
    CloneDeck { from: DeckId, to: DeckId },
    SwapDecks,
    SetCrossfader { position: f32, secs: f32 },
    SetBlendBand { deck: DeckId, band: usize, gain: f32 },
    SetBlendStem { deck: DeckId, stem: usize, gain: f32 },
    ClearBlend(DeckId),
    SetCurve(FadeCurve),
    SetMaster(f32),
    StartVoice { alloc: VoiceAlloc, pcm: Arc<TrackPcm> },
    StopVoice(VoiceId),
    SetPadVoicesGain { pad: PadKey, gain: f32 },
    SetDeckCue { deck: DeckId, on: bool },
    SetCueMode(CueMode),
    InstallPreview { pcm: Arc<TrackPcm>, autoplay: bool },
    ClearPreview,
    SetPreviewPlaying(bool),
    SeekPreviewFraction(f64),
    SetDrumBank(Arc<SampleBank>),
    ScorePreviewPlay {
        sequence: Arc<PreviewSequence>,
        piano: Option<Box<Piano>>,
        events: Option<Vec<PianoTimedEvent>>,
    },
    ScorePreviewStop,
    SetSynthClock(SynthClock),
    SetSynthPlaying(bool),
    SetSynthPattern { track: SynthTrack, pattern: StepPattern },
    SetIronfishPatch(IronfishPatch),
    SetIronfishParam { param: IronfishParam, value: f32 },
    ReplaceSynthEngines(Box<SynthEngines>),
    SetStripGain { strip: StripId, gain: f32 },
    SetStripMuted { strip: StripId, muted: bool },
    SetStripSoloed { strip: StripId, soloed: bool },
    SetMasterDynamics(MasterParams),
    SetMasterDynamicsParam { param: MasterParam, value: f32 },
    SetMasterDynamicsBypass(bool),
}

/// A payload the audio thread no longer holds, handed back so the UI
/// thread does the freeing: the last reference to a track is megabytes,
/// and a free that size has no place in a callback.
pub enum Retired {
    Pcm(DeckPcm),
    Stems(Arc<TrackStems>),
    Splat(Arc<SplatGrid>, Box<SplatFrames>),
    Track(Arc<TrackPcm>),
    Sequence(Arc<PreviewSequence>),
    Piano(Box<Piano>),
    Events(Vec<PianoTimedEvent>),
    Bank(Arc<SampleBank>),
    SynthEngines(Box<SynthEngines>),
}

/// What the callback reports back, other than the snapshot.
enum MixEvent {
    DeckEnded(DeckId),
    VoiceEnded(VoiceId),
    Retired(Retired),
}

/// One deck as the callback last saw it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DeckSnap {
    /// Playhead in source frames, whichever path is driving it.
    pub playhead_frames: f64,
    pub sample_rate: u32,
    pub playing: bool,
    pub scratching: bool,
    pub ended: bool,
    pub rate_current: f32,
    /// What the PLATTER is turning at, as a multiple of the track's own
    /// tempo: the deck's rate normally, the scratch ramp's own settled
    /// output while a hand or a motor owns the record -- negative under a
    /// reverse hold, zero at the bottom of a brake. Not the tempo fader,
    /// which is what a motor gesture is measured AGAINST rather than by.
    pub platter_rate: f64,
    /// Where the beat is, as of the END of the buffer just rendered --
    /// the same value the effect chains were prepared with, so what the
    /// screen draws and what the room hears agree about the bar.
    pub clock: DeckClock,
    pub splat: Option<SplatSnapshot>,
}

/// Everything the UI reads from the audio state, published once per
/// callback through a seqlock. `Copy`, so a read is a memcpy — never a
/// lock, never a wait on the callback.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MixSnapshot {
    pub decks: [DeckSnap; 2],
    pub fader_current: f32,
    pub preview_installed: bool,
    pub preview_position_secs: f64,
    pub preview_duration_secs: f64,
    pub preview_playing: bool,
    pub preview_ended: bool,
    pub score_playing: bool,
    pub score_pos: u64,
    pub synth: RackSnapshot,
    pub strips: [StripSnapshot; STRIP_COUNT],
    pub master_fx: MasterSnapshot,
    /// Callbacks rendered so far: how the UI tells a fresh snapshot from
    /// one published before the last command went in.
    pub serial: u64,
}

/// Frames of a video slot's audio ring. A power of two past the pacing
/// cap, so an index is a mask.
const SLOT_RING_FRAMES: usize = 1 << 17;

/// One video slot's audio, on its way from the decode thread to the
/// callback: a lock-free ring of packed stereo frames plus the flags both
/// sides read without a lock. The decode thread is the one producer, the
/// callback the one consumer; the UI only flips flags.
pub struct SlotShared {
    buf: Box<[AtomicU64]>,
    /// Absolute frames written, published by the producer per push.
    write_pos: AtomicU64,
    /// Absolute frames consumed, published by the callback per buffer.
    read_pos: AtomicU64,
    /// A flush discards everything written before `flush_at`; bumping
    /// `flush_gen` tells the callback one happened.
    flush_at: AtomicU64,
    flush_gen: AtomicU32,
    open: AtomicBool,
    paused: AtomicBool,
    playback_rate_bits: AtomicU64,
    source_rate_bits: AtomicU64,
}

impl SlotShared {
    fn new() -> SlotShared {
        SlotShared {
            buf: (0..SLOT_RING_FRAMES).map(|_| AtomicU64::new(0)).collect(),
            write_pos: AtomicU64::new(0),
            read_pos: AtomicU64::new(0),
            flush_at: AtomicU64::new(0),
            flush_gen: AtomicU32::new(0),
            open: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            playback_rate_bits: AtomicU64::new(1.0f64.to_bits()),
            source_rate_bits: AtomicU64::new(0.0f64.to_bits()),
        }
    }

    #[inline]
    fn frame_at(&self, pos: u64) -> (f32, f32) {
        let packed = self.buf[(pos as usize) & (SLOT_RING_FRAMES - 1)].load(Ordering::Relaxed);
        (f32::from_bits(packed as u32), f32::from_bits((packed >> 32) as u32))
    }

    /// Frames queued and not yet consumed.
    fn buffered_frames(&self) -> u64 {
        self.write_pos
            .load(Ordering::Acquire)
            .saturating_sub(self.read_pos.load(Ordering::Acquire))
    }

    /// Discard what is queued as of now; frames pushed after this stand.
    fn flush(&self) {
        self.flush_at.store(self.write_pos.load(Ordering::Acquire), Ordering::Release);
        self.flush_gen.fetch_add(1, Ordering::AcqRel);
    }

    fn playback_rate(&self) -> f64 {
        f64::from_bits(self.playback_rate_bits.load(Ordering::Relaxed))
    }
}

/// The callback's view of a slot for one buffer.
#[derive(Clone, Copy)]
struct SlotView {
    paused: bool,
    source_rate: f64,
    playback_rate: f64,
    base: u64,
    avail: usize,
}

/// What the UI handle and the audio engine share: only rings, atomics and
/// the snapshot cell. No mutex anywhere in it.
struct Shared {
    cmds: SpscRing<MixCmd>,
    events: SpscRing<MixEvent>,
    snapshot: SeqCell<MixSnapshot>,
    meters: [AtomicU32; 5],
    /// Pre-fader deck peaks, for the channel VU meters.
    deck_meters: [AtomicU32; 2],
    /// The most the limiter has had to pull the master back, in decibels,
    /// since this was last read. Kept the same way the peaks are and for
    /// the same reason: the limiter resets its figure every block, so a
    /// glance twenty times a second at a hundred blocks a second would see
    /// four fifths of nothing.
    limiter_reduction: AtomicU32,
    transition: TransitionAtomics,
    device_frames: AtomicU64,
    device_rate_bits: AtomicU64,
    /// One-shot proof that transport, PCM and the output callback met.
    first_non_silent: AtomicBool,
    /// Callbacks whose render outran its own buffer period: the device
    /// starves and a gap is heard. Nothing else can silence a buffer now,
    /// so this is THE dropout counter the pump reports.
    overrun_callbacks: AtomicU64,
    /// High-water render time, nanoseconds.
    render_max_nanos: AtomicU64,
    /// What the LAST buffer cost and how long it was. The high-water
    /// figure above is a lifetime worst and cannot answer the question the
    /// console actually asks, which is whether the render is coping NOW.
    render_nanos: AtomicU64,
    buffer_frames: AtomicU64,
    /// Where that buffer's time went.
    stage_nanos: crate::published::Published<StageNanos>,
    /// The headphone cue bus, written by `render`, drained by the phones
    /// device callback (slot 1).
    cue_ring: Arc<CueRing>,
    video: [SlotShared; 2],
}

/// The UI thread's own bookkeeping behind the handle: what it last asked
/// for, so a value it set reads back at once instead of a callback later,
/// and the commands a full ring handed back.
struct UiShadow {
    backlog: VecDeque<MixCmd>,
    cue_deck: [bool; 2],
    cue_mode: CueMode,
    deck: [DeckShadow; 2],
    /// A transition sent but not yet seen in the callback's atomics:
    /// reported as `Armed` under its own id until then, so the cue engine
    /// never mistakes the previous transition's `Completed` for this one.
    pending_arm: Option<ScheduledVideoTransition>,
    ended_decks: Vec<DeckId>,
    ended_voices: Vec<VoiceId>,
    score_rate: u32,
    score_event_capacity: usize,
    synth_rate: u32,
    drum_bank: Option<Arc<SampleBank>>,
    /// Snapshot serial the last drain saw, so events are not re-read.
    backlog_reported: bool,
}

#[derive(Clone, Copy, Default)]
struct DeckShadow {
    sample_rate: u32,
    expected_len: usize,
    has_pcm: bool,
    streaming: bool,
    rate: f64,
    sync_locked: bool,
    /// What the UI last asked for, so a read answers now rather than a
    /// callback later.
    slipping: bool,
    roll_depth: usize,
    /// A position and a transport the handle has asked for but no
    /// callback has applied yet, each with the snapshot serial that was
    /// current when it was sent. The published snapshot wins the moment
    /// its serial moves past that -- by then the engine has rendered a
    /// buffer with the command in it, and its answer is the true one.
    seek_intent: Option<(u64, f64)>,
    play_intent: Option<(u64, bool)>,
}

/// The UI-side handle. `Clone`, cheap, and never blocks: every mutation is
/// a command moved into a lock-free ring, every read is a copy of the
/// callback's last snapshot or the handle's own shadow of what it sent.
#[derive(Clone)]
pub struct Mixer {
    shared: Arc<Shared>,
    ui: Arc<UiCell<UiShadow>>,
    engine: Arc<OnceSlot<MixEngine>>,
}

/// The audio thread's side: owns the whole mix state outright. Built by
/// [`Mixer::new`], handed to the device callback by
/// [`Mixer::take_engine`], and from then on nothing but the callback
/// touches it.
pub struct MixEngine {
    state: MixState,
    shared: Arc<Shared>,
    slot_flush_seen: [u32; 2],
    /// Buffers rendered, for the snapshot serial.
    serial: u64,
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new()
    }
}

/// Hand a retired payload to the UI for dropping; a full ring (the UI has
/// not drained in a long while) drops it here instead, which is the one
/// free the callback still risks.
fn retire(shared: &Shared, retired: Retired) {
    if let Err(MixEvent::Retired(retired)) = shared.events.push(MixEvent::Retired(retired)) {
        drop(retired);
    }
}

fn push_event(shared: &Shared, event: MixEvent) {
    let _ = shared.events.push(event);
}

impl Mixer {
    pub fn new() -> Mixer {
        let shared = Arc::new(Shared {
            cmds: SpscRing::new(CMD_RING_SLOTS),
            events: SpscRing::new(EVENT_RING_SLOTS),
            snapshot: SeqCell::new(MixSnapshot::default()),
            meters: [
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
            ],
            deck_meters: [AtomicU32::new(0), AtomicU32::new(0)],
            limiter_reduction: AtomicU32::new(0),
            transition: TransitionAtomics::new(),
            device_frames: AtomicU64::new(0),
            device_rate_bits: AtomicU64::new(0),
            first_non_silent: AtomicBool::new(false),
            overrun_callbacks: AtomicU64::new(0),
            render_max_nanos: AtomicU64::new(0),
            render_nanos: AtomicU64::new(0),
            buffer_frames: AtomicU64::new(0),
            stage_nanos: crate::published::Published::new(StageNanos::default()),
            cue_ring: Arc::new(CueRing::new()),
            video: [SlotShared::new(), SlotShared::new()],
        });
        let engine = MixEngine {
            state: MixState::new(),
            shared: shared.clone(),
            slot_flush_seen: [0; 2],
            serial: 0,
        };
        Mixer {
            shared,
            ui: Arc::new(UiCell::new(UiShadow {
                backlog: VecDeque::new(),
                cue_deck: [false; 2],
                cue_mode: CueMode::default(),
                deck: [DeckShadow::default(); 2],
                pending_arm: None,
                ended_decks: Vec::new(),
                ended_voices: Vec::new(),
                score_rate: 48_000,
                score_event_capacity: 0,
                synth_rate: 48_000,
                drum_bank: None,
                backlog_reported: false,
            })),
            engine: Arc::new(OnceSlot::new(engine)),
        }
    }

    /// The audio-owned engine, exactly once: move it into the device
    /// callback. `None` if a callback already has it.
    pub fn take_engine(&self) -> Option<MixEngine> {
        self.engine.take()
    }

    /// `(overrun callbacks, high-water render nanos)` — a render that
    /// outran its buffer is the one way a buffer is still lost, for the
    /// pump to report.
    /// Frames between a sample entering the master bus and leaving it:
    /// the look-ahead of the limiter on it. Anything lining the output up
    /// against what went into it has to allow for this -- the tests below
    /// do, and a DAC-referenced playhead would have to as well.
    pub fn output_latency_frames(&self) -> usize {
        let rate = f64::from_bits(self.shared.device_rate_bits.load(Ordering::Relaxed));
        crate::music_dsp::limiter_latency_frames(rate as f32)
    }

    pub fn audio_health(&self) -> AudioHealth {
        AudioHealth {
            // Both zero by construction on this engine; see the struct.
            contended: 0,
            poisoned: 0,
            overruns: self.shared.overrun_callbacks.load(Ordering::Relaxed),
            phones_starved: self.shared.cue_ring.starved.load(Ordering::Relaxed),
            render_nanos: self.shared.render_nanos.load(Ordering::Relaxed),
            render_max_nanos: self.shared.render_max_nanos.load(Ordering::Relaxed),
            buffer_frames: self.shared.buffer_frames.load(Ordering::Relaxed),
            device_rate: f64::from_bits(self.shared.device_rate_bits.load(Ordering::Relaxed)),
            stages: self.shared.stage_nanos.read(),
        }
    }

    /// The output device is gone: forget the buffer it rendered last, so
    /// the budget reads as unknown rather than as that device's. Nothing
    /// else is touched -- the rate still feeds the output latency and the
    /// cue ring, and the next device's first render sets the frames again.
    pub fn forget_device(&self) {
        self.shared.buffer_frames.store(0, Ordering::Relaxed);
    }

    /// Callbacks whose render outran its own buffer period. On this engine
    /// nothing else can silence a buffer, so this is THE dropout counter.
    pub fn audio_overruns(&self) -> u64 {
        self.shared.overrun_callbacks.load(Ordering::Relaxed)
    }

    /// Queue a command for the callback. Never waits: a full ring parks
    /// the command in the UI-side backlog, which `pump` re-sends in order.
    pub fn run_cmd(&self, cmd: MixCmd) {
        self.ui.with(|ui| Self::send_in(&self.shared, ui, cmd));
    }

    fn send_in(shared: &Shared, ui: &mut UiShadow, cmd: MixCmd) {
        // Order is the whole contract: once anything is backlogged, every
        // later command queues behind it.
        if !ui.backlog.is_empty() {
            ui.backlog.push_back(cmd);
            return;
        }
        if let Err(cmd) = shared.cmds.push(cmd) {
            ui.backlog.push_back(cmd);
        }
    }

    /// Once per UI frame: re-send what a full ring refused, and take the
    /// callback's events — ended decks and voices for the next drains,
    /// retired payloads to be dropped here, on this thread.
    /// What a replaced payload is owed: a drop on the UI thread. The
    /// audio thread never frees, so it hands what it displaced back
    /// through the event ring and this is where it dies. Same call as
    /// [`Mixer::pump`], under the name the deck engine asks for it by.
    pub fn reap_retired(&self) {
        self.pump();
    }

    pub fn pump(&self) {
        let mut dropped: Vec<Retired> = Vec::new();
        self.ui.with(|ui| {
            while let Some(cmd) = ui.backlog.pop_front() {
                if let Err(cmd) = self.shared.cmds.push(cmd) {
                    ui.backlog.push_front(cmd);
                    break;
                }
            }
            let backlogged = !ui.backlog.is_empty();
            if backlogged && !ui.backlog_reported {
                ui.backlog_reported = true;
                crate::log!(
                    "audio: command ring full ({} queued on the UI side); the device callback is not draining",
                    ui.backlog.len()
                );
            } else if !backlogged {
                ui.backlog_reported = false;
            }
            while let Some(event) = self.shared.events.pop() {
                match event {
                    MixEvent::DeckEnded(deck) => ui.ended_decks.push(deck),
                    MixEvent::VoiceEnded(id) => ui.ended_voices.push(id),
                    MixEvent::Retired(retired) => dropped.push(retired),
                }
            }
        });
        drop(dropped);
    }

    /// Retired payloads the callback handed back, for a test to inspect
    /// instead of dropping.
    #[cfg(test)]
    pub fn drain_retired(&self) -> Vec<Retired> {
        let mut retired = Vec::new();
        self.ui.with(|ui| {
            while let Some(event) = self.shared.events.pop() {
                match event {
                    MixEvent::DeckEnded(deck) => ui.ended_decks.push(deck),
                    MixEvent::VoiceEnded(id) => ui.ended_voices.push(id),
                    MixEvent::Retired(payload) => retired.push(payload),
                }
            }
        });
        retired
    }

    /// Commands parked because the ring was full, right now.
    pub fn backlog_len(&self) -> usize {
        self.ui.with(|ui| ui.backlog.len())
    }

    fn snapshot(&self) -> MixSnapshot {
        self.shared.snapshot.read()
    }

    // ---- video slot buses --------------------------------------------------

    /// (Re)open a slot bus, silent, empty, unpaused.
    pub fn open_slot(&self, slot: SlotId) {
        let shared = &self.shared.video[slot.index()];
        shared.flush();
        shared.playback_rate_bits.store(1.0f64.to_bits(), Ordering::Relaxed);
        shared.paused.store(false, Ordering::Relaxed);
        shared.open.store(true, Ordering::Release);
        self.run_cmd(MixCmd::OpenSlot(slot));
    }

    /// Close = mute-and-flush; the decode thread just stops feeding it.
    pub fn close_slot(&self, slot: SlotId) {
        let shared = &self.shared.video[slot.index()];
        shared.open.store(false, Ordering::Release);
        shared.flush();
        self.ui.with(|ui| {
            if ui.pending_arm.is_some_and(|pending| pending.to == slot) {
                ui.pending_arm = None;
            }
            Self::send_in(&self.shared, ui, MixCmd::CloseSlot(slot));
        });
    }

    /// Decode-thread entry: append interleaved i16 PCM. Returns false when
    /// the slot is closed (the producer should stop). Lock-free: the ring
    /// is this thread's to write and the callback's to read.
    pub fn push_slot_audio(
        &self,
        slot: SlotId,
        samples: &[i16],
        channels: u16,
        rate: u32,
    ) -> bool {
        let shared = &self.shared.video[slot.index()];
        if !shared.open.load(Ordering::Acquire) {
            return false;
        }
        shared.source_rate_bits.store((rate as f64).to_bits(), Ordering::Relaxed);
        let ch = channels.max(1) as usize;
        let read = shared.read_pos.load(Ordering::Acquire);
        let mut write = shared.write_pos.load(Ordering::Relaxed);
        for frame in samples.chunks_exact(ch) {
            if write.saturating_sub(read) >= MAX_SLOT_QUEUE_FRAMES as u64 {
                break;
            }
            let Some(pair) = crate::dsp_math::stereo_pair(frame) else { continue };
            let l = pair[0] as f32 / 32768.0;
            let r = pair[1] as f32 / 32768.0;
            let packed = (l.to_bits() as u64) | ((r.to_bits() as u64) << 32);
            shared.buf[(write as usize) & (SLOT_RING_FRAMES - 1)].store(packed, Ordering::Relaxed);
            write += 1;
        }
        shared.write_pos.store(write, Ordering::Release);
        true
    }

    /// Buffered seconds on a slot bus (decode-thread pacing).
    pub fn slot_buffered_secs(&self, slot: SlotId) -> f64 {
        let shared = &self.shared.video[slot.index()];
        let source_rate = f64::from_bits(shared.source_rate_bits.load(Ordering::Relaxed));
        if source_rate <= 0.0 {
            return 0.0;
        }
        shared.buffered_frames() as f64
            / (source_rate * shared.playback_rate().max(MIN_VIDEO_PLAYBACK_RATE))
    }

    pub fn flush_slot_audio(&self, slot: SlotId) {
        self.shared.video[slot.index()].flush();
    }

    /// Decode-worker half of [`Self::flush_slot_audio`].
    pub fn flush_slot_audio_from_worker(&self, slot: SlotId) {
        self.shared.video[slot.index()].flush();
    }

    pub fn set_slot_paused(&self, slot: SlotId, paused: bool) {
        self.shared.video[slot.index()].paused.store(paused, Ordering::Relaxed);
    }

    /// Audio resampling rate for a video slot. The bounded range is small on
    /// purpose: it is enough to fit a visual cycle to a musical phrase while
    /// remaining perceptually safe. Deck and SFX cursors are unrelated.
    pub fn set_slot_playback_rate(&self, slot: SlotId, rate: f64) -> f64 {
        // Straight into an atomic the callback reads every buffer, so a
        // refusal has to hand back what the slot is still running at.
        let Some(rate) = knob64(rate, MIN_VIDEO_PLAYBACK_RATE, MAX_VIDEO_PLAYBACK_RATE) else {
            return f64::from_bits(
                self.shared.video[slot.index()].playback_rate_bits.load(Ordering::Relaxed),
            );
        };
        self.shared.video[slot.index()]
            .playback_rate_bits
            .store(rate.to_bits(), Ordering::Relaxed);
        rate
    }

    pub fn slot_playback_rate(&self, slot: SlotId) -> f64 {
        self.shared.video[slot.index()].playback_rate()
    }

    /// Number of output frames rendered by this mixer. This is the same
    /// clock used to trigger scheduled video transitions.
    pub fn rendered_output_frames(&self) -> u64 {
        self.shared.device_frames.load(Ordering::Acquire)
    }

    pub fn output_sample_rate(&self) -> Option<f64> {
        let rate = f64::from_bits(self.shared.device_rate_bits.load(Ordering::Acquire));
        (rate.is_finite() && rate > 0.0).then_some(rate)
    }

    pub fn has_produced_non_silent(&self) -> bool {
        self.shared.first_non_silent.load(Ordering::Acquire)
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
        if id == 0 {
            return Err(VideoTransitionError::ZeroId);
        }
        if from == Some(to) {
            return Err(VideoTransitionError::SameSlot);
        }
        if !self.shared.video[to.index()].open.load(Ordering::Acquire) {
            return Err(VideoTransitionError::DestinationClosed);
        }
        if self
            .video_transition_snapshot()
            .is_some_and(|snapshot| snapshot.phase == VideoTransitionPhase::Started)
        {
            return Err(VideoTransitionError::TransitionAlreadyStarted);
        }
        let now = self.shared.device_frames.load(Ordering::Acquire);
        let target_frame = target_frame.max(now);
        let scheduled = ScheduledVideoTransition {
            id,
            from,
            to,
            target_frame,
            fade_frames,
            started: false,
        };
        self.shared.video[to.index()].paused.store(true, Ordering::Relaxed);
        self.ui.with(|ui| {
            ui.pending_arm = Some(scheduled);
            Self::send_in(&self.shared, ui, MixCmd::ScheduleVideo(scheduled));
        });
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
        let target = self
            .shared
            .device_frames
            .load(Ordering::Acquire)
            .saturating_add(delay_frames);
        self.schedule_video_transition_at(id, from, to, target, fade_frames)
    }

    /// Cancel only while still armed. A started transition is owned by the
    /// device clock and must run to completion; callers cannot rewind it from
    /// the UI thread.
    pub fn cancel_video_transition(&self, id: VideoTransitionId) -> bool {
        let Some(snapshot) = self.video_transition_snapshot() else { return false };
        if snapshot.id != id || snapshot.phase != VideoTransitionPhase::Armed {
            return false;
        }
        self.shared.video[snapshot.to.index()].paused.store(true, Ordering::Relaxed);
        self.ui.with(|ui| {
            if ui.pending_arm.is_some_and(|pending| pending.id == id) {
                ui.pending_arm = None;
            }
            Self::send_in(&self.shared, ui, MixCmd::CancelVideo(id));
        });
        true
    }

    /// Nonblocking transition state for picture pacing, lights, and cue
    /// cleanup. `None` means no schedule has ever been published.
    pub fn video_transition_snapshot(&self) -> Option<VideoTransitionSnapshot> {
        let published = self.shared.transition.snapshot();
        // A schedule the callback has not applied yet is armed as far as
        // the UI is concerned; once its id shows up in the atomics the
        // callback's word replaces this.
        let pending = self.ui.with(|ui| {
            if let Some(pending) = ui.pending_arm {
                if published.is_some_and(|snapshot| snapshot.id == pending.id) {
                    ui.pending_arm = None;
                    return None;
                }
                return Some(pending);
            }
            None
        });
        if let Some(pending) = pending {
            return Some(VideoTransitionSnapshot {
                id: pending.id,
                phase: VideoTransitionPhase::Armed,
                from: pending.from,
                to: pending.to,
                target_frame: pending.target_frame,
                start_frame: None,
                fade_frames: pending.fade_frames,
                rendered_frame: self.shared.device_frames.load(Ordering::Acquire),
                progress: 0.0,
            });
        }
        published
    }

    /// The timed A/V crossfade: `to` ramps to 1, `from` ramps to 0. The
    /// program mute is a separate multiplier and is never touched here.
    pub fn fade_slots(&self, from: Option<SlotId>, to: SlotId, secs: f32) {
        let Some(secs) = knob(secs, SLEW_SECS, 60.0) else { return };
        self.run_cmd(MixCmd::FadeSlots { from, to, secs });
    }

    /// Operator crossfader: equal-power A/B bus gains, slewed over a few ms
    /// so a fast hand never zippers. Ignored while a scheduled transition
    /// owns the gains (it lands them itself).
    pub fn set_video_mix(&self, mix: f32) {
        let Some(mix) = knob(mix, 0.0, 1.0) else { return };
        self.run_cmd(MixCmd::SetVideoMix(mix));
    }

    /// Mute/unmute the whole video program (video-slot audio only). A ramp
    /// on the summed bus: per-slot fade targets are preserved exactly, so
    /// an unmute after any sequence of cues restores the intended level.
    pub fn set_video_muted(&self, muted: bool) {
        self.run_cmd(MixCmd::SetVideoMuted(muted));
    }

    // ---- decks -------------------------------------------------------------

    /// Install a decoded track, paused at zero. Any stems from a previous
    /// track go with it; the tone chain is reset but its settings stand.
    pub fn install_deck(&self, deck: DeckId, pcm: Arc<TrackPcm>) {
        self.install_deck_pcm(deck, DeckPcm::Whole(pcm));
    }

    /// Install a track that is still being decoded, paused at zero: the
    /// deck plays what has arrived and waits at the decoded edge for the
    /// rest. Everything else is `install_deck`.
    pub fn install_deck_stream(&self, deck: DeckId, stream: Arc<StreamPcm>) {
        self.install_deck_pcm(deck, DeckPcm::Stream(stream));
    }

    fn install_deck_pcm(&self, deck: DeckId, pcm: DeckPcm) {
        self.ui.with(|ui| {
            ui.deck[deck.index()] = DeckShadow {
                sample_rate: pcm.sample_rate(),
                expected_len: pcm.expected_len(),
                has_pcm: true,
                streaming: matches!(pcm, DeckPcm::Stream(_)),
                rate: ui.deck[deck.index()].rate,
                sync_locked: ui.deck[deck.index()].sync_locked,
                // A fresh record inherits no detour: whatever the last one
                // was slipping or rolling over went with it.
                slipping: false,
                roll_depth: 0,
                // A fresh record is at its start and stopped, and no
                // intent about the last one survives it.
                seek_intent: None,
                play_intent: None,
            };
            Self::send_in(&self.shared, ui, MixCmd::InstallDeck { deck, pcm });
        });
    }

    /// More of a streaming track arrived: swap the grown table in. One
    /// pointer move — the chunks are shared with the table already
    /// playing, so nothing is copied and nothing the callback is reading
    /// moves. A deck that is not streaming (the whole file landed, or
    /// another track took the deck) ignores it.
    pub fn grow_deck_stream(&self, deck: DeckId, stream: Arc<StreamPcm>) {
        self.ui.with(|ui| {
            let shadow = &mut ui.deck[deck.index()];
            if !shadow.streaming {
                return;
            }
            shadow.expected_len = stream.expected.max(stream.len);
            Self::send_in(&self.shared, ui, MixCmd::GrowStream { deck, stream });
        });
    }

    /// The decoder finished: the whole file takes over from the chunk
    /// table at the playhead. Same samples on the same timeline, so the
    /// transport, the stretcher and any loop keep exactly their place; a
    /// deck parked at the decoded edge simply continues.
    pub fn complete_deck(&self, deck: DeckId, pcm: Arc<TrackPcm>) {
        self.ui.with(|ui| {
            let shadow = &mut ui.deck[deck.index()];
            if !shadow.streaming {
                return;
            }
            shadow.streaming = false;
            shadow.expected_len = pcm.frames.len();
            Self::send_in(&self.shared, ui, MixCmd::CompleteDeck { deck, pcm });
        });
    }

    /// Whether the deck is playing a track that is still being decoded.
    pub fn deck_is_streaming(&self, deck: DeckId) -> bool {
        self.ui.with(|ui| ui.deck[deck.index()].streaming)
    }

    /// Drop the deck's track entirely: the voice renders silence until the
    /// next install. Settings (gain, EQ, keylock) stand, like install_deck.
    pub fn clear_deck(&self, deck: DeckId) {
        self.ui.with(|ui| {
            let rate = ui.deck[deck.index()].rate;
            ui.deck[deck.index()] = DeckShadow { rate, ..DeckShadow::default() };
            Self::send_in(&self.shared, ui, MixCmd::ClearDeck(deck));
        });
    }

    /// Attach separated stems to the track already on the deck. They must be
    /// the same timeline as the mixed file; the deck keeps playing.
    pub fn install_deck_stems(&self, deck: DeckId, stems: Arc<TrackStems>) {
        self.run_cmd(MixCmd::InstallStems { deck, stems });
    }

    pub fn clear_deck_stems(&self, deck: DeckId) {
        self.run_cmd(MixCmd::ClearStems(deck));
    }

    pub fn set_deck_playing(&self, deck: DeckId, playing: bool) {
        let serial = self.snapshot().serial;
        self.ui.with(|ui| {
            ui.deck[deck.index()].play_intent = Some((serial, playing));
            Self::send_in(&self.shared, ui, MixCmd::SetPlaying { deck, playing });
        });
    }

    /// Install or replace a grid. Frame conversion is deliberately done
    /// here, on the caller thread, before the callback sees the state.
    pub fn set_deck_splat(&self, deck: DeckId, grid: Arc<SplatGrid>) {
        self.ui.with(|ui| {
            let shadow = ui.deck[deck.index()];
            if !shadow.has_pcm {
                return;
            }
            let frames = Box::new(SplatFrames::from_grid(&grid, shadow.sample_rate.max(1) as f64));
            Self::send_in(&self.shared, ui, MixCmd::SetSplat { deck, grid, frames });
        });
    }

    pub fn set_deck_splat_enabled(&self, deck: DeckId, on: bool) {
        self.run_cmd(MixCmd::SetSplatEnabled { deck, on });
    }

    pub fn splat_launch(&self, deck: DeckId, row: SplatRow, col: u8, part: SplatPart) {
        self.run_cmd(MixCmd::SplatLaunch { deck, row, col, part });
    }

    pub fn splat_stop_row(&self, deck: DeckId, row: SplatRow, timed: bool) {
        self.run_cmd(MixCmd::SplatStopRow { deck, row, timed });
    }

    /// Launch a whole section: every STEM row of the column. The mix row is
    /// the undemixed track and never plays under its own stems.
    pub fn splat_launch_scene(&self, deck: DeckId, col: u8) {
        self.run_cmd(MixCmd::SplatLaunchScene { deck, col });
    }

    pub fn splat_stop_all(&self, deck: DeckId, timed: bool) {
        self.run_cmd(MixCmd::SplatStopAll { deck, timed });
    }

    /// A fraction of the track as the strip shows it — its EXPECTED length
    /// while it is still decoding — clamped by the callback to what has
    /// arrived, so a jump past the decoded edge waits there.
    pub fn seek_deck_fraction(&self, deck: DeckId, fraction: f64) {
        let Some(fraction) = knob64(fraction, 0.0, 1.0) else { return };
        let serial = self.snapshot().serial;
        self.ui.with(|ui| {
            let shadow = &mut ui.deck[deck.index()];
            shadow.seek_intent = Some((serial, fraction * shadow.expected_len as f64));
            Self::send_in(&self.shared, ui, MixCmd::SeekFraction { deck, fraction });
        });
    }

    /// Absolute seek in source seconds.
    pub fn seek_deck_seconds(&self, deck: DeckId, secs: f64) {
        let Some(secs) = knob64(secs, 0.0, f64::from(u32::MAX)) else { return };
        let serial = self.snapshot().serial;
        self.ui.with(|ui| {
            let shadow = &mut ui.deck[deck.index()];
            let frames = secs * shadow.sample_rate.max(1) as f64;
            shadow.seek_intent = Some((serial, frames.min(shadow.expected_len as f64)));
            Self::send_in(&self.shared, ui, MixCmd::SeekSeconds { deck, secs });
        });
    }

    /// Relative seek: `delta_secs` from wherever the playhead is when the
    /// command reaches the audio thread. The right shape for a phase
    /// correction measured against a snapshot — the error survives the
    /// trip, an absolute target does not.
    pub fn nudge_deck_seconds(&self, deck: DeckId, delta_secs: f64) {
        // A splat's own frame counter ACCUMULATES this one, so a single
        // bad number would not merely land badly: it would stay.
        let Some(delta_secs) = knob64(delta_secs, -3_600.0, 3_600.0) else { return };
        self.run_cmd(MixCmd::SeekRelative { deck, delta_secs });
    }

    /// Tempo multiplier. With key lock on the pitch is preserved; with it
    /// off the deck simply plays faster or slower.
    pub fn set_deck_rate(&self, deck: DeckId, rate: f64) {
        let rate = rate.clamp(crate::decks::RATE_MIN, crate::decks::RATE_MAX);
        self.ui.with(|ui| {
            ui.deck[deck.index()].rate = rate;
            Self::send_in(&self.shared, ui, MixCmd::SetRate { deck, rate: rate as f32 });
        });
    }

    /// A synced grid's first launch joins its running clock instead of
    /// resetting it to the selected source cell.
    pub fn set_deck_sync_lock(&self, deck: DeckId, on: bool) {
        self.ui.with(|ui| {
            if ui.deck[deck.index()].sync_locked != on {
                ui.deck[deck.index()].sync_locked = on;
                Self::send_in(&self.shared, ui, MixCmd::SetSyncLock { deck, on });
            }
        });
    }

    /// The tempo last asked for.
    pub fn deck_rate(&self, deck: DeckId) -> f64 {
        self.ui.with(|ui| ui.deck[deck.index()].rate)
    }

    /// Key shift in SEMITONES: pitch without tempo. Stored as the frequency
    /// ratio it stands for, because the render loop wants a multiplier and
    /// an exp2 per frame would be a waste.
    pub fn set_deck_key_shift(&self, deck: DeckId, semitones: f64) {
        let semitones = semitones.clamp(-crate::decks::KEY_SHIFT_MAX, crate::decks::KEY_SHIFT_MAX);
        let ratio = (semitones / 12.0).exp2() as f32;
        self.run_cmd(MixCmd::SetKeyRatio { deck, ratio });
    }

    pub fn set_deck_keylock(&self, deck: DeckId, on: bool) {
        self.run_cmd(MixCmd::SetKeylock { deck, on });
    }

    /// Vinyl-style pointer control over the playhead.
    pub fn scratch_deck(&self, deck: DeckId, motion: ScratchMotion) {
        // A drag's place and speed are measured from pointer samples, and
        // a pair of them arriving in the same microsecond is a division by
        // a zero. The ramp holds both as plain numbers, so a bad one stays
        // until the hand comes off.
        if let ScratchMotion::Move { secs, rate } = motion {
            if !secs.is_finite() || !rate.is_finite() {
                return;
            }
        }
        self.run_cmd(MixCmd::Scratch { deck, motion });
    }

    /// One tone band, 0 = kill.
    pub fn set_deck_eq_band(&self, deck: DeckId, band: usize, gain: f32) {
        self.run_cmd(MixCmd::SetEqBand { deck, band, gain });
    }

    /// Bipolar sweep filter; 0.5 = off.
    pub fn set_deck_filter(&self, deck: DeckId, position: f32) {
        self.run_cmd(MixCmd::SetFilter { deck, position });
    }

    /// One stem lane's gain. Ramped, so a knob move never zippers.
    pub fn set_deck_stem_gain(&self, deck: DeckId, stem: usize, gain: f32) {
        if stem >= STEM_COUNT {
            return;
        }
        self.run_cmd(MixCmd::SetStemGain { deck, stem, gain: gain.max(0.0) });
    }

    /// The deck's loop in source SECONDS; the callback converts against the
    /// track's own rate so the render path only ever deals in frames.
    pub fn set_deck_loop_span(
        &self,
        deck: DeckId,
        span: Option<(f64, f64)>,
        seek: crate::decks::LoopSeek,
    ) {
        if span.is_some_and(|(start, end)| !start.is_finite() || !end.is_finite()) {
            return;
        }
        self.run_cmd(MixCmd::SetLoopSpan { deck, span, seek });
    }

    pub fn set_deck_mute(&self, deck: DeckId, muted: bool) {
        self.run_cmd(MixCmd::SetMute { deck, muted });
    }

    /// The echo's delay as a beat fraction. A zero either side is not a
    /// fraction and is refused HERE rather than on the audio thread, which
    /// has no way to say no.
    pub fn set_deck_echo(&self, deck: DeckId, fraction: Option<(u32, u32)>) {
        if fraction.is_some_and(|(num, den)| num == 0 || den == 0) {
            return;
        }
        self.run_cmd(MixCmd::ChainEffect { target: deck.into(), param: EffectParam::Echo(fraction) });
    }

    pub fn set_blend_filter(&self, deck: DeckId, offset: f32) {
        self.run_cmd(MixCmd::ChainEffect { target: deck.into(), param: EffectParam::BlendFilter(offset) });
    }

    /// The grid this record is ruled by. Everything beat-locked reads the
    /// clock it makes, so a track that has just been measured starts
    /// locking without waiting for anything else to happen.
    /// Slip: the record keeps its own time while the hand takes the deck
    /// somewhere else. `adopt` keeps where it was left instead of landing
    /// back on the ghost.
    pub fn set_deck_slip(&self, deck: DeckId, on: bool, adopt: bool) {
        self.ui.with(|ui| {
            ui.deck[deck.index()].slipping = on;
            Self::send_in(&self.shared, ui, MixCmd::SetSlip { deck, on, adopt });
        });
    }

    pub fn deck_slipping(&self, deck: DeckId) -> bool {
        self.ui.with(|ui| ui.deck[deck.index()].slipping)
    }

    /// Load over a deck that is still sounding. The outgoing record fades
    /// out first and the new one is seated when it has gone -- a swap on
    /// the instant the decode lands ends the old track mid-sample.
    pub fn install_deck_over(&self, deck: DeckId, pcm: Arc<TrackPcm>, keep_playing: bool) {
        let pcm = DeckPcm::Whole(pcm);
        self.ui.with(|ui| {
            ui.deck[deck.index()] = DeckShadow {
                sample_rate: pcm.sample_rate(),
                expected_len: pcm.expected_len(),
                has_pcm: true,
                streaming: false,
                rate: ui.deck[deck.index()].rate,
                sync_locked: ui.deck[deck.index()].sync_locked,
                slipping: false,
                roll_depth: 0,
                // A fresh record is at its start and stopped, and no
                // intent about the last one survives it.
                seek_intent: None,
                play_intent: None,
            };
            Self::send_in(&self.shared, ui, MixCmd::InstallOver { deck, pcm, keep_playing });
        });
    }

    /// The instant double: the same record on the other deck, at the same
    /// place, reading the same PCM.
    pub fn clone_deck(&self, from: DeckId, to: DeckId) {
        if from == to {
            return;
        }
        self.ui.with(|ui| {
            let src = ui.deck[from.index()];
            if !src.has_pcm {
                return;
            }
            ui.deck[to.index()] = DeckShadow {
                slipping: false,
                roll_depth: 0,
                rate: ui.deck[to.index()].rate,
                sync_locked: ui.deck[to.index()].sync_locked,
                ..src
            };
            Self::send_in(&self.shared, ui, MixCmd::CloneDeck { from, to });
        });
    }

    pub fn set_deck_censor(&self, deck: DeckId, on: bool) {
        self.run_cmd(MixCmd::SetCensor { deck, on });
    }

    /// Stack another roll over what is already held. `false` when the
    /// stack is full -- the caller needs to know at once, so the answer
    /// comes from the shadow rather than a callback later.
    pub fn push_deck_roll(&self, deck: DeckId) -> bool {
        self.ui.with(|ui| {
            let depth = &mut ui.deck[deck.index()].roll_depth;
            if *depth >= ROLL_STACK_CAP {
                return false;
            }
            *depth += 1;
            Self::send_in(&self.shared, ui, MixCmd::PushRoll { deck });
            true
        })
    }

    pub fn pop_deck_roll(&self, deck: DeckId, parent: Option<(f64, f64)>, adopt: bool) {
        if parent.is_some_and(|(start, end)| !start.is_finite() || !end.is_finite()) {
            return;
        }
        self.ui.with(|ui| {
            let depth = &mut ui.deck[deck.index()].roll_depth;
            *depth = if adopt { 0 } else { depth.saturating_sub(1) };
            Self::send_in(&self.shared, ui, MixCmd::PopRoll { deck, parent, adopt });
        });
    }

    /// Brake, spin-back or soft-start: the motors that move the platter
    /// with no hand on it.
    pub fn spin_deck(&self, deck: DeckId, motion: SpinMotion) {
        self.run_cmd(MixCmd::Spin { deck, motion });
    }

    /// Hold this many SOURCE seconds, or none to let go.
    pub fn set_deck_freeze(&self, deck: DeckId, secs: Option<f64>) {
        self.run_cmd(MixCmd::SetFreeze { deck, secs });
    }

    pub fn set_deck_grid(&self, deck: DeckId, grid: Option<TrackGrid>) {
        self.run_cmd(MixCmd::SetGrid { deck, grid });
    }

    pub fn set_deck_resonance(&self, deck: DeckId, lift: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::Resonance(lift),
        });
    }

    pub fn set_deck_echo_feedback(&self, deck: DeckId, feedback: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::EchoFeedback(feedback),
        });
    }

    pub fn set_deck_echo_pingpong(&self, deck: DeckId, on: bool) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::EchoPingpong(on),
        });
    }

    pub fn set_deck_flanger(&self, deck: DeckId, on: bool) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::Flanger(on),
        });
    }

    pub fn set_deck_flanger_rate(&self, deck: DeckId, hz: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::FlangerRate(hz),
        });
    }

    pub fn set_deck_flanger_depth(&self, deck: DeckId, depth: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::FlangerDepth(depth),
        });
    }

    pub fn set_deck_flanger_feedback(&self, deck: DeckId, feedback: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::FlangerFeedback(feedback),
        });
    }

    pub fn set_deck_flanger_sync_units(&self, deck: DeckId, units: u32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::FlangerSyncUnits(units),
        });
    }

    pub fn set_deck_flanger_beat_offset(&self, deck: DeckId, offset: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::FlangerBeatOffset(offset),
        });
    }

    pub fn set_deck_bitcrusher(&self, deck: DeckId, on: bool) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::Bitcrusher(on),
        });
    }

    pub fn set_deck_bitcrusher_rate(&self, deck: DeckId, hz: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::BitcrusherRate(hz),
        });
    }

    pub fn set_deck_bitcrusher_bits(&self, deck: DeckId, bits: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::BitcrusherBits(bits),
        });
    }

    pub fn set_deck_tremolo(&self, deck: DeckId, on: bool) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::Tremolo(on),
        });
    }

    pub fn set_deck_tremolo_rate(&self, deck: DeckId, hz: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::TremoloRate(hz),
        });
    }

    pub fn set_deck_tremolo_depth(&self, deck: DeckId, depth: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::TremoloDepth(depth),
        });
    }

    pub fn set_deck_tremolo_sync_units(&self, deck: DeckId, units: u32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::TremoloSyncUnits(units),
        });
    }

    pub fn set_deck_tremolo_beat_offset(&self, deck: DeckId, offset: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::TremoloBeatOffset(offset),
        });
    }

    pub fn set_deck_distortion(&self, deck: DeckId, on: bool) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::Distortion(on),
        });
    }

    pub fn set_deck_distortion_drive(&self, deck: DeckId, drive: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::DistortionDrive(drive),
        });
    }

    pub fn set_deck_phaser(&self, deck: DeckId, on: bool) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::Phaser(on),
        });
    }

    pub fn set_deck_phaser_rate(&self, deck: DeckId, hz: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::PhaserRate(hz),
        });
    }

    pub fn set_deck_phaser_feedback(&self, deck: DeckId, feedback: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::PhaserFeedback(feedback),
        });
    }

    pub fn set_deck_phaser_sync_units(&self, deck: DeckId, units: u32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::PhaserSyncUnits(units),
        });
    }

    pub fn set_deck_phaser_beat_offset(&self, deck: DeckId, offset: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::PhaserBeatOffset(offset),
        });
    }

    pub fn set_deck_autopan(&self, deck: DeckId, on: bool) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::Autopan(on),
        });
    }

    pub fn set_deck_compressor(&self, deck: DeckId, on: bool) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::Compressor(on),
        });
    }

    pub fn set_deck_compressor_threshold(&self, deck: DeckId, db: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::CompressorThreshold(db),
        });
    }

    pub fn set_deck_compressor_ratio(&self, deck: DeckId, ratio: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::CompressorRatio(ratio),
        });
    }

    pub fn set_deck_autopan_rate(&self, deck: DeckId, hz: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::AutopanRate(hz),
        });
    }

    pub fn set_deck_echo_mix(&self, deck: DeckId, mix: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::EchoMix(mix),
        });
    }

    pub fn set_deck_echo_level_mode(&self, deck: DeckId, mode: LevelMode) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::EchoLevelMode(mode),
        });
    }

    pub fn set_deck_echo_ceiling(&self, deck: DeckId, ceiling: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::EchoCeiling(ceiling),
        });
    }

    pub fn set_deck_flanger_mix(&self, deck: DeckId, mix: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::FlangerMix(mix),
        });
    }

    pub fn set_deck_flanger_level_mode(&self, deck: DeckId, mode: LevelMode) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::FlangerLevelMode(mode),
        });
    }

    pub fn set_deck_flanger_ceiling(&self, deck: DeckId, ceiling: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::FlangerCeiling(ceiling),
        });
    }

    pub fn set_deck_bitcrusher_mix(&self, deck: DeckId, mix: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::BitcrusherMix(mix),
        });
    }

    pub fn set_deck_bitcrusher_level_mode(&self, deck: DeckId, mode: LevelMode) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::BitcrusherLevelMode(mode),
        });
    }

    pub fn set_deck_bitcrusher_ceiling(&self, deck: DeckId, ceiling: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::BitcrusherCeiling(ceiling),
        });
    }

    pub fn set_deck_tremolo_mix(&self, deck: DeckId, mix: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::TremoloMix(mix),
        });
    }

    pub fn set_deck_tremolo_level_mode(&self, deck: DeckId, mode: LevelMode) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::TremoloLevelMode(mode),
        });
    }

    pub fn set_deck_tremolo_ceiling(&self, deck: DeckId, ceiling: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::TremoloCeiling(ceiling),
        });
    }

    pub fn set_deck_distortion_mix(&self, deck: DeckId, mix: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::DistortionMix(mix),
        });
    }

    pub fn set_deck_distortion_level_mode(&self, deck: DeckId, mode: LevelMode) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::DistortionLevelMode(mode),
        });
    }

    pub fn set_deck_distortion_ceiling(&self, deck: DeckId, ceiling: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::DistortionCeiling(ceiling),
        });
    }

    pub fn set_deck_phaser_mix(&self, deck: DeckId, mix: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::PhaserMix(mix),
        });
    }

    pub fn set_deck_phaser_level_mode(&self, deck: DeckId, mode: LevelMode) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::PhaserLevelMode(mode),
        });
    }

    pub fn set_deck_phaser_ceiling(&self, deck: DeckId, ceiling: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::PhaserCeiling(ceiling),
        });
    }

    pub fn set_deck_autopan_mix(&self, deck: DeckId, mix: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::AutopanMix(mix),
        });
    }

    pub fn set_deck_autopan_level_mode(&self, deck: DeckId, mode: LevelMode) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::AutopanLevelMode(mode),
        });
    }

    pub fn set_deck_autopan_ceiling(&self, deck: DeckId, ceiling: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::AutopanCeiling(ceiling),
        });
    }

    pub fn set_deck_stereo_width_mix(&self, deck: DeckId, mix: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::StereoWidthMix(mix),
        });
    }

    pub fn set_deck_stereo_width_level_mode(&self, deck: DeckId, mode: LevelMode) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::StereoWidthLevelMode(mode),
        });
    }

    pub fn set_deck_stereo_width_ceiling(&self, deck: DeckId, ceiling: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::StereoWidthCeiling(ceiling),
        });
    }

    pub fn set_deck_plate_reverb_mix(&self, deck: DeckId, mix: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::PlateReverbMix(mix),
        });
    }

    pub fn set_deck_plate_reverb_level_mode(&self, deck: DeckId, mode: LevelMode) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::PlateReverbLevelMode(mode),
        });
    }

    pub fn set_deck_plate_reverb_ceiling(&self, deck: DeckId, ceiling: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::PlateReverbCeiling(ceiling),
        });
    }

    pub fn set_deck_moog_ladder_mix(&self, deck: DeckId, mix: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::MoogLadderMix(mix),
        });
    }

    pub fn set_deck_moog_ladder_level_mode(&self, deck: DeckId, mode: LevelMode) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::MoogLadderLevelMode(mode),
        });
    }

    pub fn set_deck_moog_ladder_ceiling(&self, deck: DeckId, ceiling: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::MoogLadderCeiling(ceiling),
        });
    }

    pub fn set_deck_level_default(&self, deck: DeckId, mode: LevelMode) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::LevelDefault(mode),
        });
    }

    pub fn set_deck_autopan_sync_units(&self, deck: DeckId, units: u32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::AutopanSyncUnits(units),
        });
    }

    pub fn set_deck_autopan_beat_offset(&self, deck: DeckId, offset: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::AutopanBeatOffset(offset),
        });
    }

    pub fn set_deck_stereo_width(&self, deck: DeckId, on: bool) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::StereoWidth(on),
        });
    }

    pub fn set_deck_stereo_width_amount(&self, deck: DeckId, width: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::StereoWidthAmount(width),
        });
    }

    pub fn set_deck_plate_reverb(&self, deck: DeckId, on: bool) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::PlateReverb(on),
        });
    }

    pub fn set_deck_plate_reverb_size(&self, deck: DeckId, size: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::PlateReverbSize(size),
        });
    }

    pub fn set_deck_moog_ladder(&self, deck: DeckId, on: bool) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::MoogLadder(on),
        });
    }

    pub fn set_deck_moog_ladder_cutoff(&self, deck: DeckId, hz: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::MoogLadderCutoff(hz),
        });
    }

    pub fn set_deck_moog_ladder_resonance(&self, deck: DeckId, resonance: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::MoogLadderResonance(resonance),
        });
    }

    pub fn set_deck_crossovers(&self, deck: DeckId, low_hz: f32, high_hz: f32) {
        self.run_cmd(MixCmd::ChainEffect {
            target: deck.into(),
            param: EffectParam::Crossovers(low_hz, high_hz),
        });
    }

    /// A channel's own gain, up to half again over unity.
    ///
    /// The ceiling is the DECK ENGINE's, and it has to be: the engine clamps
    /// to 1.5 in two places, both channel faders declare `max: 1.5`, and the
    /// control surface's channel fader sends its position times 1.5. This
    /// clamped at 1.0, so the top third of either fader moved nothing the
    /// room could hear, and loudness normalisation — which multiplies by up
    /// to 4 — could only ever turn a record DOWN, never bring a quiet one
    /// up. The console's own idea of what is audible disagreed with the
    /// audio too, because `deck_strip_gain` reports the unclamped product.
    ///
    /// Above unity is not a hazard here: the master limiter's ceiling
    /// clamps every sample below full, which is what it is for, and the
    /// master itself already accepts 1.2 while the stem lanes are uncapped.
    pub fn set_deck_gain(&self, deck: DeckId, gain: f32) {
        let Some(gain) = knob(gain, 0.0, crate::decks::MAX_DECK_GAIN) else { return };
        self.run_cmd(MixCmd::SetGain { deck, gain });
    }

    pub fn swap_decks(&self) {
        self.ui.with(|ui| {
            ui.deck.swap(0, 1);
            Self::send_in(&self.shared, ui, MixCmd::SwapDecks);
        });
    }

    pub fn set_crossfader(&self, position: f32) {
        let Some(position) = knob(position, 0.0, 1.0) else { return };
        self.run_cmd(MixCmd::SetCrossfader { position, secs: SLEW_SECS });
    }

    pub fn fade_crossfader(&self, position: f32, secs: f32) {
        let Some(position) = knob(position, 0.0, 1.0) else { return };
        let Some(secs) = knob(secs, SLEW_SECS, 60.0) else { return };
        self.run_cmd(MixCmd::SetCrossfader { position, secs });
    }

    /// Where the crossfader actually is right now, mid-ramp included. The
    /// deck surface mirrors this while a timed fade runs, so the on-screen
    /// fader travels with the audio instead of teleporting to the target.
    pub fn crossfader_position(&self) -> f32 {
        self.snapshot().fader_current
    }

    /// The autopilot's blend overlay: multiplies the operator's values,
    /// never moves them. `clear_blend` is the whole restore.
    pub fn set_blend_band(&self, deck: DeckId, band: usize, gain: f32) {
        self.run_cmd(MixCmd::SetBlendBand { deck, band, gain });
    }

    pub fn set_blend_stem(&self, deck: DeckId, stem: usize, gain: f32) {
        if stem >= STEM_COUNT {
            return;
        }
        self.run_cmd(MixCmd::SetBlendStem { deck, stem, gain: gain.clamp(0.0, 1.0) });
    }

    pub fn clear_blend(&self, deck: DeckId) {
        self.run_cmd(MixCmd::ClearBlend(deck));
    }

    pub fn set_curve(&self, curve: FadeCurve) {
        self.run_cmd(MixCmd::SetCurve(curve));
    }

    pub fn set_master(&self, gain: f32) {
        let Some(gain) = knob(gain, 0.0, MAX_MASTER_GAIN) else { return };
        self.run_cmd(MixCmd::SetMaster(gain));
    }

    /// `(position_secs, duration_secs, playing)` from the device clock.
    pub fn deck_position(&self, deck: DeckId) -> (f64, f64, bool) {
        let snapshot = self.deck_snapshot(deck);
        (snapshot.position_secs, snapshot.duration_secs, snapshot.playing)
    }

    /// Position, transport and splat state in one read. The duration is
    /// the handle's own word (exact the moment a track installs); the
    /// rest is the callback's last snapshot.
    pub fn deck_snapshot(&self, deck: DeckId) -> DeckSnapshot {
        self.deck_snapshots()[deck.index()]
    }

    /// Both playheads from one callback. Separate reads can straddle an
    /// audio buffer and manufacture a phase error between aligned decks.
    pub fn deck_snapshots(&self) -> [DeckSnapshot; 2] {
        let shadows = self.ui.with(|ui| ui.deck);
        let snapshot = self.snapshot();
        std::array::from_fn(|i| {
            Self::deck_snapshot_from(shadows[i], snapshot.decks[i], snapshot.serial)
        })
    }

    fn deck_snapshot_from(shadow: DeckShadow, snap: DeckSnap, serial: u64) -> DeckSnapshot {
        if !shadow.has_pcm {
            return DeckSnapshot {
                position_secs: 0.0,
                duration_secs: 0.0,
                playing: false,
                scratching: snap.scratching,
                platter_rate: 0.0,
                // A deck with no record has no beat, and the default says
                // so: no grid, nothing turning.
                clock: DeckClock::default(),
                splat: None,
            };
        }
        let rate = shadow.sample_rate.max(1) as f64;
        // An intent stands until a buffer has been rendered with it in:
        // the serial only moves when the callback publishes, so a command
        // sent since the last publish is still on its way.
        let fresh = |at: u64| serial <= at;
        let playhead = match shadow.seek_intent {
            Some((at, frames)) if fresh(at) => frames,
            _ => snap.playhead_frames,
        };
        let playing = match shadow.play_intent {
            Some((at, playing)) if fresh(at) => playing,
            _ => snap.playing,
        };
        DeckSnapshot {
            position_secs: playhead / rate,
            duration_secs: shadow.expected_len as f64 / rate,
            playing,
            scratching: snap.scratching,
            platter_rate: snap.platter_rate,
            clock: snap.clock,
            splat: snap.splat,
        }
    }

    /// Pre-fader peak levels for the two deck VU meters. `meters()` reports
    /// what reaches the master; these report what the channel is doing,
    /// which is what an operator sets gain against.
    /// The most the limiter pulled the master back since this was last
    /// called, in decibels. Zero means it never had to.
    ///
    /// This is what stands in for a clip light on a limited output. A clip
    /// light on this master would never come on: the ceiling clamps every
    /// sample below full scale, so the peak meter cannot reach it. What an
    /// operator needs to know is that the mix is being held down, which is
    /// this.
    ///
    /// Reading takes it, for the reason `meters` gives.
    pub fn limiter_reduction_db(&self) -> f32 {
        f32::from_bits(self.shared.limiter_reduction.swap(0, Ordering::Relaxed))
    }

    /// Reading takes them, for the reason `meters` gives.
    pub fn deck_levels(&self) -> [f32; 2] {
        [
            f32::from_bits(self.shared.deck_meters[0].swap(0, Ordering::Relaxed)),
            f32::from_bits(self.shared.deck_meters[1].swap(0, Ordering::Relaxed)),
        ]
    }

    /// True while a hand (or its release ramp) owns a deck's playhead.
    pub fn deck_scratching(&self, deck: DeckId) -> bool {
        self.snapshot().decks[deck.index()].scratching
    }

    /// Decks that ran off their end (loop off) since the last drain.
    pub fn drain_ended_decks(&self) -> Vec<DeckId> {
        self.pump();
        self.ui.with(|ui| std::mem::take(&mut ui.ended_decks))
    }

    // ---- sfx voices ---------------------------------------------------------

    pub fn start_voice(&self, alloc: VoiceAlloc, pcm: Arc<TrackPcm>) {
        let mut alloc = alloc;
        let Some(gain) = knob(alloc.gain, 0.0, 4.0) else { return };
        alloc.gain = gain;
        self.run_cmd(MixCmd::StartVoice { alloc, pcm });
    }

    pub fn stop_voice(&self, id: VoiceId) {
        self.run_cmd(MixCmd::StopVoice(id));
    }

    pub fn set_pad_voices_gain(&self, pad: PadKey, gain: f32) {
        let Some(gain) = knob(gain, 0.0, 4.0) else { return };
        self.run_cmd(MixCmd::SetPadVoicesGain { pad, gain });
    }

    /// Voices that finished naturally (ran off the end, loop off).
    pub fn drain_ended_voices(&self) -> Vec<VoiceId> {
        self.pump();
        self.ui.with(|ui| std::mem::take(&mut ui.ended_voices))
    }

    /// The highest peak on each bus since this was last called:
    /// `[master, video, deck_a, deck_b, sfx]`.
    ///
    /// Reading TAKES them. That is what makes the answer mean "since you
    /// last looked" rather than "in whichever buffer happened to be last",
    /// and it is why a second caller sees zeros: whoever reads first has
    /// the peaks. One place drives the meters (`App::pump`); anything else
    /// reading this is sampling, and will steal.
    pub fn meters(&self) -> [f32; 5] {
        let mut out = [0.0f32; 5];
        for (i, m) in self.shared.meters.iter().enumerate() {
            out[i] = f32::from_bits(m.swap(0, Ordering::Relaxed));
        }
        out
    }

    // ---- the headphone cue bus ----------------------------------------------

    /// The ring the slot-1 (phones) callback drains. The callback holds
    /// ONLY this — never the mix state, which belongs to the program.
    pub fn cue_ring(&self) -> Arc<CueRing> {
        self.shared.cue_ring.clone()
    }

    /// Armed while a phones device is actually requested at slot 1: gates
    /// the producer, and an unarmed consumer outputs silence.
    pub fn set_cue_armed(&self, armed: bool) {
        self.shared.cue_ring.armed.store(armed, Ordering::Relaxed);
    }

    pub fn set_phones_volume(&self, volume: f32) {
        let Some(volume) = knob(volume, 0.0, 1.0) else { return };
        self.shared.cue_ring.volume_bits.store(volume.to_bits(), Ordering::Relaxed);
    }

    /// Route a deck into the phones. Cue follows the deck SLOT (the channel
    /// strip), not the record: `swap_decks` deliberately leaves it alone.
    pub fn set_deck_cue(&self, deck: DeckId, on: bool) {
        self.ui.with(|ui| {
            ui.cue_deck[deck.index()] = on;
            Self::send_in(&self.shared, ui, MixCmd::SetDeckCue { deck, on });
        });
    }

    pub fn deck_cue(&self, deck: DeckId) -> bool {
        self.ui.with(|ui| ui.cue_deck[deck.index()])
    }

    /// Which point of the chain the cue listens to. A hard switch, like
    /// the monitor-select toggle on hardware.
    pub fn set_cue_mode(&self, mode: CueMode) {
        self.ui.with(|ui| {
            ui.cue_mode = mode;
            Self::send_in(&self.shared, ui, MixCmd::SetCueMode(mode));
        });
    }

    pub fn cue_mode(&self) -> CueMode {
        self.ui.with(|ui| ui.cue_mode)
    }

    /// Install (and by default start) the pre-listen player.
    pub fn install_preview(&self, pcm: Arc<TrackPcm>, autoplay: bool) {
        self.run_cmd(MixCmd::InstallPreview { pcm, autoplay });
    }

    /// Take the preview down. The (possibly huge) buffer comes back through
    /// the retired-payload path and is dropped on the UI thread.
    pub fn clear_preview(&self) {
        self.run_cmd(MixCmd::ClearPreview);
    }

    pub fn set_preview_playing(&self, playing: bool) {
        self.run_cmd(MixCmd::SetPreviewPlaying(playing));
    }

    pub fn seek_preview_fraction(&self, fraction: f64) {
        let Some(fraction) = knob64(fraction, 0.0, 1.0) else { return };
        self.run_cmd(MixCmd::SeekPreviewFraction(fraction));
    }

    /// `(position_secs, duration_secs, playing, ended)` — the pre-listen
    /// mirror of `deck_position`. `None` while no preview is installed.
    pub fn preview_position(&self) -> Option<(f64, f64, bool, bool)> {
        let snapshot = self.snapshot();
        snapshot.preview_installed.then_some((
            snapshot.preview_position_secs,
            snapshot.preview_duration_secs,
            snapshot.preview_playing,
            snapshot.preview_ended,
        ))
    }

    // ---- clocked instrument rack + program mix ---------------------------

    /// Keep the rack's modelled instruments on the physical device rate.
    /// Construction happens here, on the UI thread; the callback only swaps
    /// the completed box and returns the old one for UI-thread destruction.
    pub fn ensure_synth_rate(&self, sample_rate: u32, patch: IronfishPatch) {
        let sample_rate = sample_rate.clamp(8_000, 384_000);
        let bank = self.ui.with(|ui| {
            if ui.synth_rate == sample_rate {
                return None;
            }
            ui.synth_rate = sample_rate;
            Some(ui.drum_bank.clone())
        });
        if let Some(bank) = bank {
            self.run_cmd(MixCmd::ReplaceSynthEngines(Box::new(SynthEngines::new(
                sample_rate,
                patch,
                bank,
            ))));
        }
    }

    pub fn set_drum_bank(&self, bank: Arc<SampleBank>) {
        self.ui.with(|ui| {
            ui.drum_bank = Some(bank.clone());
            Self::send_in(&self.shared, ui, MixCmd::SetDrumBank(bank));
        });
    }

    /// Whether a rate change would rebuild the rack WITH a drum bank.
    #[cfg(test)]
    pub fn drum_bank_remembered(&self) -> bool {
        self.ui.with(|ui| ui.drum_bank.is_some())
    }

    pub fn set_synth_clock(&self, clock: SynthClock) {
        self.run_cmd(MixCmd::SetSynthClock(clock));
    }

    pub fn set_synth_playing(&self, playing: bool) {
        self.run_cmd(MixCmd::SetSynthPlaying(playing));
    }

    pub fn set_synth_pattern(&self, track: SynthTrack, pattern: StepPattern) {
        self.run_cmd(MixCmd::SetSynthPattern { track, pattern });
    }

    pub fn set_ironfish_patch(&self, patch: IronfishPatch) {
        self.run_cmd(MixCmd::SetIronfishPatch(patch));
    }

    pub fn set_ironfish_param(&self, param: IronfishParam, value: f32) {
        self.run_cmd(MixCmd::SetIronfishParam { param, value });
    }

    pub fn set_strip_gain(&self, strip: StripId, gain: f32) {
        self.run_cmd(MixCmd::SetStripGain { strip, gain });
    }

    pub fn set_strip_muted(&self, strip: StripId, muted: bool) {
        self.run_cmd(MixCmd::SetStripMuted { strip, muted });
    }

    pub fn set_strip_soloed(&self, strip: StripId, soloed: bool) {
        self.run_cmd(MixCmd::SetStripSoloed { strip, soloed });
    }

    pub fn set_master_dynamics(&self, params: MasterParams) {
        self.run_cmd(MixCmd::SetMasterDynamics(params));
    }

    pub fn set_master_dynamics_param(&self, param: MasterParam, value: f32) {
        self.run_cmd(MixCmd::SetMasterDynamicsParam { param, value });
    }

    pub fn set_master_dynamics_bypass(&self, bypass: bool) {
        self.run_cmd(MixCmd::SetMasterDynamicsBypass(bypass));
    }

    /// Which deck's grid the chains with no tempo of their own follow:
    /// the mix's, the pads', the synth tracks'. None of them has a tempo,
    /// so they borrow one.
    pub fn set_chain_clock_deck(&self, deck: DeckId) {
        self.run_cmd(MixCmd::ChainClockDeck(deck));
    }

    /// One knob on one slot of any chain: a deck's, a source's or the
    /// mix's. The per-deck setters above are this with the deck filled in.
    pub fn set_chain_effect(&self, target: ChainTarget, param: EffectParam) {
        self.run_cmd(MixCmd::ChainEffect { target, param });
    }

    pub fn synth_snapshot(&self) -> RackSnapshot {
        self.snapshot().synth
    }

    pub fn program_mix_snapshot(
        &self,
    ) -> ([StripSnapshot; STRIP_COUNT], MasterSnapshot) {
        let snapshot = self.snapshot();
        (snapshot.strips, snapshot.master_fx)
    }

    // ---- loop-score preview ------------------------------------------------

    /// Install and start a score preview. Instrument construction and event
    /// capacity growth happen here on the caller/UI thread, never in render;
    /// what they replace comes back here to be dropped.
    pub fn score_preview_play(&self, sequence: Arc<PreviewSequence>) {
        let sample_rate = sequence.sample_rate.max(1);
        let needed = ScorePreviewVoice::required_event_capacity(&sequence);
        self.ui.with(|ui| {
            let piano = (ui.score_rate != sample_rate).then(|| Box::new(Piano::new(sample_rate as f32)));
            let events = (ui.score_event_capacity < needed).then(|| Vec::with_capacity(needed));
            ui.score_rate = sample_rate;
            if let Some(events) = &events {
                ui.score_event_capacity = events.capacity();
            }
            Self::send_in(&self.shared, ui, MixCmd::ScorePreviewPlay { sequence, piano, events });
        });
    }

    pub fn score_preview_stop(&self) {
        self.run_cmd(MixCmd::ScorePreviewStop);
    }

    pub fn score_preview_state(&self) -> (bool, u64) {
        let snapshot = self.snapshot();
        (snapshot.score_playing, snapshot.score_pos)
    }
}

impl MixEngine {
    /// Apply every command queued since the last buffer. Bounded by the
    /// ring, allocation-free, and every payload it replaces goes back to
    /// the UI through the events ring.
    fn drain_commands(&mut self) {
        while let Some(cmd) = self.shared.cmds.pop() {
            self.apply(cmd);
        }
    }

    /// Commands applied and a snapshot published, without rendering. The
    /// test harness's way of asking "what would the callback see now".
    #[cfg(test)]
    pub fn sync(&mut self) {
        self.drain_commands();
        // The serial moves, exactly as a rendered buffer would move it:
        // it is what tells the handle that everything it had sent has
        // been applied, and a sync applies everything a render would.
        self.serial = self.serial.wrapping_add(1);
        Self::publish_snapshot(&self.state, &self.shared, self.serial);
    }

    #[cfg(test)]
    fn state_mut(&mut self) -> &mut MixState {
        &mut self.state
    }

    /// Apply one effect parameter to a chain. The bodies are exactly what
    /// the old locked setters ran; only the way they arrive has changed.
    fn apply_effect(chain: &mut DeckChain, param: EffectParam) {
        match param {
            EffectParam::Echo(fraction) => chain.echo_mut().set_fraction(fraction),
            EffectParam::BlendFilter(offset) => chain.eq_mut().set_blend_filter(offset),
            EffectParam::Resonance(lift) => chain.eq_mut().set_resonance(lift),
            EffectParam::EchoFeedback(feedback) => chain.echo_mut().set_feedback(feedback),
            EffectParam::EchoPingpong(on) => chain.echo_mut().set_pingpong(on),
            EffectParam::Flanger(on) => chain.flanger_mut().set_wet(if on { 1.0 } else { 0.0 }),
            EffectParam::FlangerRate(hz) => chain.flanger_mut().set_rate(hz),
            EffectParam::FlangerDepth(depth) => chain.flanger_mut().set_depth(depth),
            EffectParam::FlangerFeedback(feedback) => chain.flanger_mut().set_feedback(feedback),
            EffectParam::FlangerSyncUnits(units) => chain.flanger_mut().set_sync_units(units),
            EffectParam::FlangerBeatOffset(offset) => chain.flanger_mut().set_beat_offset(offset),
            EffectParam::Bitcrusher(on) => chain.bitcrusher_mut().set_wet(if on { 1.0 } else { 0.0 }),
            EffectParam::BitcrusherRate(hz) => chain.bitcrusher_mut().set_rate(hz),
            EffectParam::BitcrusherBits(bits) => chain.bitcrusher_mut().set_bits(bits),
            EffectParam::Tremolo(on) => chain.tremolo_mut().set_wet(if on { 1.0 } else { 0.0 }),
            EffectParam::TremoloRate(hz) => chain.tremolo_mut().set_rate(hz),
            EffectParam::TremoloDepth(depth) => chain.tremolo_mut().set_depth(depth),
            EffectParam::TremoloSyncUnits(units) => chain.tremolo_mut().set_sync_units(units),
            EffectParam::TremoloBeatOffset(offset) => chain.tremolo_mut().set_beat_offset(offset),
            EffectParam::Distortion(on) => chain.distortion_mut().set_wet(if on { 1.0 } else { 0.0 }),
            EffectParam::DistortionDrive(drive) => chain.distortion_mut().set_drive(drive),
            EffectParam::Phaser(on) => chain.phaser_mut().set_wet(if on { 1.0 } else { 0.0 }),
            EffectParam::PhaserRate(hz) => chain.phaser_mut().set_rate(hz),
            EffectParam::PhaserFeedback(feedback) => chain.phaser_mut().set_feedback(feedback),
            EffectParam::PhaserSyncUnits(units) => chain.phaser_mut().set_sync_units(units),
            EffectParam::PhaserBeatOffset(offset) => chain.phaser_mut().set_beat_offset(offset),
            EffectParam::Autopan(on) => chain.autopan_mut().set_wet(if on { 1.0 } else { 0.0 }),
            EffectParam::Compressor(on) => chain.compressor_mut().set_wet(if on { 1.0 } else { 0.0 }),
            EffectParam::CompressorThreshold(db) => chain.compressor_mut().set_threshold_db(db),
            EffectParam::CompressorRatio(ratio) => chain.compressor_mut().set_ratio(ratio),
            EffectParam::AutopanRate(hz) => chain.autopan_mut().set_rate(hz),
            EffectParam::EchoMix(mix) => chain.level_mut(2).set_mix(mix),
            EffectParam::EchoLevelMode(mode) => chain.level_mut(2).set_mode(mode),
            EffectParam::EchoCeiling(ceiling) => chain.level_mut(2).set_ceiling(ceiling),
            EffectParam::FlangerMix(mix) => chain.level_mut(3).set_mix(mix),
            EffectParam::FlangerLevelMode(mode) => chain.level_mut(3).set_mode(mode),
            EffectParam::FlangerCeiling(ceiling) => chain.level_mut(3).set_ceiling(ceiling),
            EffectParam::BitcrusherMix(mix) => chain.level_mut(4).set_mix(mix),
            EffectParam::BitcrusherLevelMode(mode) => chain.level_mut(4).set_mode(mode),
            EffectParam::BitcrusherCeiling(ceiling) => chain.level_mut(4).set_ceiling(ceiling),
            EffectParam::TremoloMix(mix) => chain.level_mut(5).set_mix(mix),
            EffectParam::TremoloLevelMode(mode) => chain.level_mut(5).set_mode(mode),
            EffectParam::TremoloCeiling(ceiling) => chain.level_mut(5).set_ceiling(ceiling),
            EffectParam::DistortionMix(mix) => chain.level_mut(6).set_mix(mix),
            EffectParam::DistortionLevelMode(mode) => chain.level_mut(6).set_mode(mode),
            EffectParam::DistortionCeiling(ceiling) => chain.level_mut(6).set_ceiling(ceiling),
            EffectParam::PhaserMix(mix) => chain.level_mut(7).set_mix(mix),
            EffectParam::PhaserLevelMode(mode) => chain.level_mut(7).set_mode(mode),
            EffectParam::PhaserCeiling(ceiling) => chain.level_mut(7).set_ceiling(ceiling),
            EffectParam::AutopanMix(mix) => chain.level_mut(8).set_mix(mix),
            EffectParam::AutopanLevelMode(mode) => chain.level_mut(8).set_mode(mode),
            EffectParam::AutopanCeiling(ceiling) => chain.level_mut(8).set_ceiling(ceiling),
            EffectParam::StereoWidthMix(mix) => chain.level_mut(9).set_mix(mix),
            EffectParam::StereoWidthLevelMode(mode) => chain.level_mut(9).set_mode(mode),
            EffectParam::StereoWidthCeiling(ceiling) => chain.level_mut(9).set_ceiling(ceiling),
            EffectParam::PlateReverbMix(mix) => chain.level_mut(10).set_mix(mix),
            EffectParam::PlateReverbLevelMode(mode) => chain.level_mut(10).set_mode(mode),
            EffectParam::PlateReverbCeiling(ceiling) => chain.level_mut(10).set_ceiling(ceiling),
            EffectParam::MoogLadderMix(mix) => chain.level_mut(11).set_mix(mix),
            EffectParam::MoogLadderLevelMode(mode) => chain.level_mut(11).set_mode(mode),
            EffectParam::MoogLadderCeiling(ceiling) => chain.level_mut(11).set_ceiling(ceiling),
            EffectParam::LevelDefault(mode) => chain.set_level_default(mode),
            EffectParam::AutopanSyncUnits(units) => chain.autopan_mut().set_sync_units(units),
            EffectParam::AutopanBeatOffset(offset) => chain.autopan_mut().set_beat_offset(offset),
            EffectParam::StereoWidth(on) => chain.stereo_width_mut().set_wet(if on { 1.0 } else { 0.0 }),
            EffectParam::StereoWidthAmount(width) => chain.stereo_width_mut().set_width(width),
            EffectParam::PlateReverb(on) => chain.plate_reverb_mut().set_wet(if on { 1.0 } else { 0.0 }),
            EffectParam::PlateReverbSize(size) => chain.plate_reverb_mut().set_size(size),
            EffectParam::MoogLadder(on) => chain.moog_ladder_mut().set_wet(if on { 1.0 } else { 0.0 }),
            EffectParam::MoogLadderCutoff(hz) => chain.moog_ladder_mut().set_cutoff(hz),
            EffectParam::MoogLadderResonance(resonance) => chain.moog_ladder_mut().set_resonance(resonance),
            EffectParam::Crossovers(low_hz, high_hz) => chain.eq_mut().set_crossovers(low_hz, high_hz),
        }
    }

    /// Everything a slot with a line or a tank is still carrying from the
    /// record BEFORE this one. The units themselves belong to the deck and
    /// are not copied or reset -- but what is in their delay lines is the
    /// last record's, and must not bleed into the one that just landed.
    fn silence_chain_tails(d: &mut DeckVoice) {
        d.chain.echo_mut().silence();
        d.chain.freeze_mut().reset();
        d.chain.flanger_mut().silence();
        d.chain.bitcrusher_mut().silence();
        d.chain.phaser_mut().reset();
        d.chain.plate_reverb_mut().silence();
        d.chain.moog_ladder_mut().reset();
    }

    /// Put a record on a silent deck. `keep_playing` is answered by the
    /// caller: a pending load carries the operator's intent from when they
    /// asked, not from when the fade happened to land.
    fn seat_record(
        shared: &Shared,
        d: &mut DeckVoice,
        pcm: DeckPcm,
        play: bool,
        grid: Option<TrackGrid>,
    ) {
        if let Some(old) = d.pcm.replace(pcm) {
            retire(shared, Retired::Pcm(old));
        }
        d.stems = None;
        d.splat = None;
        // Whatever the record brought with it, which is nothing unless its
        // analysis landed while it was waiting out the fade.
        d.grid = grid;
        d.clock = DeckClock::at(d.grid.as_ref(), 0.0, deck_platter(d), 0.0);
        d.playing = play;
        d.pause_at = None;
        d.ended = false;
        // A fresh track has nothing to fade out of, but it has something
        // to fade INTO: seated at full level it is a step from silence,
        // and the old record it replaces has just spent forty milliseconds
        // leaving so that there would be no step.
        d.transport = Ramp::at(0.0);
        if play {
            d.transport.slew(1.0, LOAD_SWAP_SECS);
        }
        d.seek_frames(0.0);
        d.chain.eq_mut().reset();
        Self::silence_chain_tails(d);
        d.reset_blend();
    }

    fn apply(&mut self, cmd: MixCmd) {
        let shared = &*self.shared;
        let s = &mut self.state;
        match cmd {
            MixCmd::OpenSlot(slot) => {
                let bus = &mut s.video[slot.index()];
                bus.cursor = 0.0;
                bus.gain = Ramp::at(0.0);
            }
            MixCmd::CloseSlot(slot) => {
                if let Some(scheduled) = s.scheduled_video.filter(|scheduled| scheduled.to == slot) {
                    s.scheduled_video = None;
                    // The device may have crossed the target just before
                    // the UI observed `Started`. If latest-click-wins closes
                    // that still-armed destination, restore the previous
                    // program instead of leaving a half-faded silence.
                    if scheduled.started {
                        if let Some(from) = scheduled.from {
                            s.video[from.index()].gain = Ramp::at(1.0);
                        }
                    }
                    shared.transition.publish_phase(
                        VideoTransitionPhase::Cancelled,
                        shared.device_frames.load(Ordering::Acquire),
                    );
                }
                let bus = &mut s.video[slot.index()];
                bus.cursor = 0.0;
                bus.gain = Ramp::at(0.0);
            }
            MixCmd::FadeSlots { from, to, secs } => {
                if let Some(scheduled) = s.scheduled_video.take() {
                    if scheduled.started {
                        // The audio clock owns a started transition: do not
                        // restart its ramp or destroy its completion.
                        s.scheduled_video = Some(scheduled);
                        return;
                    }
                    shared.transition.publish_phase(
                        VideoTransitionPhase::Cancelled,
                        shared.device_frames.load(Ordering::Acquire),
                    );
                    if scheduled.to != to {
                        shared.video[scheduled.to.index()].paused.store(true, Ordering::Relaxed);
                        s.video[scheduled.to.index()].gain = Ramp::at(0.0);
                    }
                }
                let secs = secs.max(SLEW_SECS);
                if let Some(from) = from {
                    s.video[from.index()].gain.slew(0.0, secs);
                }
                s.video[to.index()].gain.slew(1.0, secs);
            }
            MixCmd::SetVideoMix(mix) => {
                if s.scheduled_video.is_some() {
                    return;
                }
                let (a, b) = crossfader_gains(mix, FadeCurve::EqualPower);
                s.video[0].gain.slew(a, 0.015);
                s.video[1].gain.slew(b, 0.015);
            }
            MixCmd::SetVideoMuted(muted) => {
                s.video_mute.slew(if muted { 0.0 } else { 1.0 }, SLEW_SECS * 4.0);
            }
            MixCmd::ScheduleVideo(mut scheduled) => {
                if s.scheduled_video.is_some_and(|scheduled| scheduled.started) {
                    return;
                }
                if let Some(old) = s.scheduled_video.take() {
                    shared.video[old.to.index()].paused.store(true, Ordering::Relaxed);
                    s.video[old.to.index()].gain = Ramp::at(0.0);
                }
                let now = shared.device_frames.load(Ordering::Acquire);
                scheduled.target_frame = scheduled.target_frame.max(now);
                scheduled.started = false;
                shared.video[scheduled.to.index()].paused.store(true, Ordering::Relaxed);
                s.video[scheduled.to.index()].gain = Ramp::at(0.0);
                s.scheduled_video = Some(scheduled);
                shared.transition.publish_arm(scheduled, now);
            }
            MixCmd::CancelVideo(id) => {
                let Some(scheduled) = s.scheduled_video else { return };
                if scheduled.id != id || scheduled.started {
                    return;
                }
                s.scheduled_video = None;
                shared.video[scheduled.to.index()].paused.store(true, Ordering::Relaxed);
                s.video[scheduled.to.index()].gain = Ramp::at(0.0);
                shared.transition.publish_phase(
                    VideoTransitionPhase::Cancelled,
                    shared.device_frames.load(Ordering::Acquire),
                );
            }
            MixCmd::InstallDeck { deck, pcm } => {
                let d = &mut s.decks[deck.index()];
                Self::retire_deck_media(shared, d);
                d.pcm = Some(pcm);
                d.stem_blend = ParamRamp::at(1.0);
                d.playing = false;
                // The last record's grid goes with the last record. Left
                // behind, it rules the new one's beat until its own
                // analysis lands -- so loops, jumps and every beat-locked
                // effect would be measured against a tempo belonging to a
                // track that is no longer on the deck.
                d.grid = None;
                d.clock = DeckClock::default();
                // Nor does a fresh record owe the old one's pause a
                // give-back to a position that means nothing on it.
                d.pause_at = None;
                d.seek_frames(0.0);
                d.chain.eq_mut().reset();
                // A hold does not outlive the record it was holding.
                Self::silence_chain_tails(d);
                d.reset_blend();
            }
            MixCmd::GrowStream { deck, stream } => {
                let d = &mut s.decks[deck.index()];
                if matches!(d.pcm, Some(DeckPcm::Stream(_))) {
                    if let Some(old) = d.pcm.replace(DeckPcm::Stream(stream)) {
                        retire(shared, Retired::Pcm(old));
                    }
                } else {
                    retire(shared, Retired::Pcm(DeckPcm::Stream(stream)));
                }
            }
            MixCmd::CompleteDeck { deck, pcm } => {
                let d = &mut s.decks[deck.index()];
                if matches!(d.pcm, Some(DeckPcm::Stream(_))) {
                    if let Some(old) = d.pcm.replace(DeckPcm::Whole(pcm)) {
                        retire(shared, Retired::Pcm(old));
                    }
                } else {
                    retire(shared, Retired::Track(pcm));
                }
            }
            MixCmd::ClearDeck(deck) => {
                let d = &mut s.decks[deck.index()];
                d.sync_locked = false;
                Self::retire_deck_media(shared, d);
                // A record waiting out a fade goes with the one that was
                // leaving. Left parked, its fade would land on an empty
                // deck a few milliseconds later and seat the very track
                // the operator has just thrown away.
                if let Some(load) = d.pending.take() {
                    retire(shared, Retired::Pcm(load.pcm));
                }
                // And the fade itself stops, or the deck goes on counting
                // down towards a swap that has nothing left to swap.
                d.transport = Ramp::at(0.0);
                d.playing = false;
                d.grid = None;
                d.clock = DeckClock::default();
                // With no pcm the clamp parks the playhead at zero; this
                // also clears `ended`, so a later install re-arms end
                // reporting.
                d.seek_frames(0.0);
                // A hold does not outlive the record it was holding: a
                // freeze or an echo tail left running would go on sounding
                // over an empty deck.
                Self::silence_chain_tails(d);
                d.reset_blend();
            }
            MixCmd::InstallStems { deck, stems } => {
                let d = &mut s.decks[deck.index()];
                if d.pcm.is_none() || stems.is_empty() {
                    retire(shared, Retired::Stems(stems));
                    return;
                }
                // The first table on this track is the swap from the mixed
                // file to its stems: blend it in. Later tables are the same
                // stems with more chunks and need no blend.
                if d.stems.is_none() {
                    d.stem_blend = ParamRamp::at(0.0);
                    d.stem_blend.slew(1.0, STEM_SWAP_SECS);
                }
                if let Some(old) = d.stems.replace(stems) {
                    retire(shared, Retired::Stems(old));
                }
            }
            MixCmd::ClearStems(deck) => {
                if let Some(old) = s.decks[deck.index()].stems.take() {
                    retire(shared, Retired::Stems(old));
                }
            }
            MixCmd::SetPlaying { deck, playing } => {
                let d = &mut s.decks[deck.index()];
                // With a load parked, the deck the operator is aiming at is
                // the one ARRIVING. Re-aim it and leave the outgoing
                // track's fade alone: turning the transport back up here
                // means it never reaches zero, so the swap never happens
                // and the new record is stranded in the slot for good.
                if let Some(load) = d.pending.as_mut() {
                    load.play = playing;
                    d.playing = playing;
                    return;
                }
                if playing {
                    // Playing from the end restarts. A playhead at the
                    // DECODED edge of a streaming track is not at the end:
                    // it waits there.
                    if d.playhead_frames() >= d.frame_count() as f64
                        && d.pcm.as_ref().is_some_and(DeckPcm::complete)
                        && !d.splat.as_ref().is_some_and(|splat| splat.active)
                    {
                        d.seek_frames(0.0);
                    }
                    d.ended = false;
                }
                d.pause_at = if playing { None } else { Some(d.playhead_frames()) };
                d.playing = playing;
                d.transport.slew(if playing { 1.0 } else { 0.0 }, SLEW_SECS);
            }
            MixCmd::SetSplat { deck, grid, frames } => {
                let voice = &mut s.decks[deck.index()];
                if voice.pcm.is_none() {
                    retire(shared, Retired::Splat(grid, frames));
                    return;
                }
                match voice.splat.as_mut() {
                    Some(splat) => {
                        let old_grid = std::mem::replace(&mut splat.grid, grid);
                        let old_frames = std::mem::replace(&mut splat.frames, frames);
                        retire(shared, Retired::Splat(old_grid, old_frames));
                        splat.rebase_rows();
                    }
                    None => voice.splat = Some(SplatState::new(grid, frames, voice.pos)),
                }
            }
            MixCmd::SetSplatEnabled { deck, on } => {
                let voice = &mut s.decks[deck.index()];
                let frame_count = voice.frame_count() as f64;
                // Where the ear is on the plain transport, before the grid
                // takes the clock: the master starts exactly there. It is
                // NOT pulled back to a bar start — launches quantise
                // against the grid's own bars whatever the master reads,
                // and a paused deck must not be seen to move (a fresh load
                // sat at the first bar, not at zero, with the grid on).
                let heard = voice.playhead_frames().clamp(0.0, frame_count);
                let Some(splat) = voice.splat.as_mut() else { return };
                if on == splat.active {
                    return;
                }
                if on {
                    splat.master_frames = heard;
                    splat.view = None;
                    splat.active = true;
                    voice.pos = heard;
                    voice.stretching = false;
                    voice.reader.reset();
                } else {
                    // Leave the grid where the ear was: inside the cell the
                    // picture followed, not wherever the master clock got to.
                    let heard = splat.playhead_frames().clamp(0.0, frame_count);
                    splat.active = false;
                    splat.view = None;
                    voice.seek_frames(heard);
                }
            }
            MixCmd::SplatLaunch { deck, row, col, part } => {
                let voice = &mut s.decks[deck.index()];
                if let Some(splat) = voice.splat.as_mut() {
                    splat.queue_cell(row, col as usize, part, voice.sync_locked);
                }
            }
            MixCmd::SplatStopRow { deck, row, timed } => {
                if let Some(splat) = s.decks[deck.index()].splat.as_mut() {
                    splat.queue_stop(row, timed);
                }
            }
            MixCmd::SplatLaunchScene { deck, col } => {
                let voice = &mut s.decks[deck.index()];
                if let Some(splat) = voice.splat.as_mut() {
                    for row in SplatRow::ALL {
                        if row == SplatRow::Mix {
                            continue;
                        }
                        splat.queue_cell(row, col as usize, SplatPart::WHOLE, voice.sync_locked);
                    }
                }
            }
            MixCmd::SplatStopAll { deck, timed } => {
                if let Some(splat) = s.decks[deck.index()].splat.as_mut() {
                    for row in SplatRow::ALL {
                        splat.queue_stop(row, timed);
                    }
                }
            }
            MixCmd::SeekFraction { deck, fraction } => {
                let d = &mut s.decks[deck.index()];
                let len = d.pcm.as_ref().map_or(0.0, |pcm| pcm.expected_len() as f64);
                if len > 0.0 && d.frame_count() > 0 {
                    let from = d.playhead_frames();
                    d.seek_frames(fraction.clamp(0.0, 1.0) * len);
                    d.arm_seek_fade(from);
                }
            }
            MixCmd::SeekSeconds { deck, secs } => {
                let d = &mut s.decks[deck.index()];
                let Some(pcm) = d.pcm.as_ref() else { return };
                let frames = secs.max(0.0) * pcm.sample_rate().max(1) as f64;
                let from = d.playhead_frames();
                d.seek_frames(frames);
                // A deliberate move cancels the promise a pause made. A
                // pause latches where it was pressed and hands the deck
                // back there when its fade lands; a seek arriving inside
                // that fade would otherwise be undone the moment it does.
                d.pause_at = None;
                d.arm_seek_fade(from);
            }
            MixCmd::SeekRelative { deck, delta_secs } => {
                let d = &mut s.decks[deck.index()];
                let Some(pcm) = d.pcm.as_ref() else { return };
                if let Some(splat) = d.splat.as_mut().filter(|splat| splat.active) {
                    let total = (SEEK_XFADE_SECS * pcm.sample_rate().max(1) as f64).max(1.0);
                    splat.phase_fade = d.playing.then_some(SplatPhaseFade {
                        pos: splat.master_frames, left: total, total, rows: splat.rows,
                    });
                    // This clock can run beyond the file and must not be
                    // clamped or replaced by the cell's wrapped playhead.
                    splat.master_frames += delta_secs * pcm.sample_rate().max(1) as f64;
                    return;
                }
                let from = d.playhead_frames();
                let frames = from + delta_secs * pcm.sample_rate().max(1) as f64;
                d.seek_frames(frames);
                d.arm_seek_fade(from);
            }
            MixCmd::SetRate { deck, rate } => {
                // A short ramp so a sync landing mid-phrase does not step
                // the pitch.
                s.decks[deck.index()].rate.slew(rate, SLEW_SECS * 4.0);
            }
            MixCmd::SetSyncLock { deck, on } => s.decks[deck.index()].sync_locked = on,
            MixCmd::SetKeyRatio { deck, ratio } => {
                // Same ramp as the tempo: a stepped semitone glides instead
                // of clicking, and the stretcher sees a ratio that never
                // jumps.
                s.decks[deck.index()].key_ratio.slew(ratio, SLEW_SECS * 4.0);
            }
            MixCmd::SetKeylock { deck, on } => s.decks[deck.index()].keylock = on,
            MixCmd::Scratch { deck, motion } => {
                let d = &mut s.decks[deck.index()];
                let deck_rate = d.rate.current();
                match motion {
                    ScratchMotion::Grab => d.scratch.grab(deck_rate),
                    ScratchMotion::Move { secs, rate } => d.scratch.drag(secs, rate),
                    ScratchMotion::Release => d.scratch.release(deck_rate),
                }
            }
            MixCmd::ChainEffect { target, param } => {
                Self::apply_effect(s.chain_mut(target), param)
            }
            MixCmd::ChainClockDeck(deck) => s.chain_clock_deck = deck,
            MixCmd::InstallOver { deck, pcm, keep_playing } => {
                let d = &mut s.decks[deck.index()];
                // Silent already: nothing is leaving, so nothing has to be
                // waited for.
                if d.transport.current <= 0.0 {
                    Self::seat_record(shared, d, pcm, false, None);
                } else {
                    if let Some(old) = d.pending.take() {
                        // Latest wins: two loads inside one fade and only
                        // the second gets its turn.
                        retire(shared, Retired::Pcm(old.pcm));
                    }
                    // The give-back belongs to the track that is leaving,
                    // and that track is about to be gone.
                    d.pause_at = None;
                    // The flag is the operator's intent and can be answered
                    // now; the read gate keeps a deck sounding while its
                    // transport is above zero, so a stopping load leaves
                    // exactly the way a pause does.
                    d.playing = keep_playing;
                    d.pending = Some(PendingLoad { pcm, play: keep_playing, grid: None });
                    d.transport.slew(0.0, LOAD_SWAP_SECS);
                }
            }
            MixCmd::CloneDeck { from, to } => {
                if from.index() == to.index() {
                    return;
                }
                let (first, rest) = s.decks.split_at_mut(1);
                let (src, dst) = match from.index() == 0 {
                    true => (&first[0], &mut rest[0]),
                    false => (&rest[0], &mut first[0]),
                };
                let Some(pcm) = src.pcm.clone() else { return };
                let at = src.playhead_frames();
                dst.pcm = Some(pcm);
                dst.stems = src.stems.clone();
                // A splat grid is the other deck's launch state, not the
                // record.
                dst.splat = None;
                // The beat grid IS the record's and comes with it -- a
                // double onto a deck with no grid would otherwise silence
                // the double's clock.
                dst.grid = src.grid;
                dst.clock = src.clock;
                dst.loop_span = src.loop_span;
                dst.slip = None;
                dst.rolls = RollGhosts::default();
                dst.pause_at = None;
                dst.ended = false;
                dst.playing = src.playing;
                dst.transport = src.transport;
                dst.rate = src.rate;
                dst.key_ratio = src.key_ratio;
                dst.keylock = src.keylock;
                dst.seek_fade = None;
                dst.seek_frames(at);
                dst.stretch.copy_state_from(&src.stretch);
                dst.stretching = src.stretching;
                Self::silence_chain_tails(dst);
                dst.reset_blend();
            }
            MixCmd::SetSlip { deck, on, adopt } => {
                let device = f64::from_bits(shared.device_rate_bits.load(Ordering::Acquire));
                let d = &mut s.decks[deck.index()];
                if on {
                    arm_ghost(d, device);
                } else if let Some(ghost) = d.slip.take() {
                    // ADOPT keeps where the hand left the record; otherwise
                    // the deck lands on the ghost and the detour never
                    // happened.
                    if !adopt {
                        let from = d.playhead_frames();
                        d.seek_frames(ghost.pos);
                        d.pause_at = None;
                        d.arm_seek_fade(from);
                    }
                }
            }
            MixCmd::SetCensor { deck, on } => {
                let device = f64::from_bits(shared.device_rate_bits.load(Ordering::Acquire));
                let d = &mut s.decks[deck.index()];
                if on {
                    // A hand on the record outranks a motor.
                    if d.scratch.held() {
                        return;
                    }
                    // Armed ONCE: asking twice would arm a ghost and then
                    // report that it had not, and every hold would leak the
                    // one it made.
                    let armed = arm_ghost(d, device);
                    // With nothing to return to, a reverse hold is just a
                    // scratch.
                    if !armed && d.slip.is_none() {
                        return;
                    }
                    d.censor_owns_slip = armed;
                    let from = d.rate.current();
                    d.scratch.motor(from, CENSOR_RATE, CENSOR_FLIP_SECS, MotorEnd::Hold);
                } else {
                    let to = d.rate.current();
                    d.scratch.motor(d.scratch.rate(), to, CENSOR_RETURN_SECS, MotorEnd::Retire);
                    // Ask WHOSE ghost it is before taking it. `take()`
                    // empties the slot the moment it runs, and a filter
                    // after it only decides what to do with what has
                    // already been removed -- so an operator's own latched
                    // SLIP was destroyed by a censor that never owned it,
                    // and neither the landing here nor the later release
                    // of SLIP had anything left to land on.
                    if d.censor_owns_slip {
                        if let Some(ghost) = d.slip.take() {
                            let from = d.playhead_frames();
                            d.seek_frames(ghost.pos);
                            d.pause_at = None;
                            d.arm_seek_fade(from);
                        }
                        d.censor_owns_slip = false;
                    }
                }
            }
            MixCmd::PushRoll { deck } => {
                let device = f64::from_bits(shared.device_rate_bits.load(Ordering::Acquire));
                let d = &mut s.decks[deck.index()];
                let Some(pcm) = d.pcm.as_ref() else { return };
                if !(device > 0.0) {
                    return;
                }
                let natural = pcm.sample_rate().max(1) as f64 / device;
                let ghost = Ghost {
                    pos: d.playhead_frames(),
                    step: natural * d.rate.current() as f64,
                    span: d.loop_span,
                };
                d.rolls.push(ghost);
            }
            MixCmd::PopRoll { deck, parent, adopt } => {
                let d = &mut s.decks[deck.index()];
                if adopt {
                    // The loop now sounding is the deck's: every level under
                    // it stands down, and nothing goes back.
                    d.rolls.clear();
                    return;
                }
                let Some(ghost) = d.rolls.pop() else { return };
                let Some(pcm) = d.pcm.as_ref() else { return };
                let rate = pcm.sample_rate().max(1) as f64;
                let frames = d.frame_count() as f64;
                d.loop_span = parent
                    .map(|(start, end)| (start.max(0.0) * rate, (end.max(0.0) * rate).min(frames)));
                let from = d.playhead_frames();
                d.seek_frames(ghost.pos);
                d.pause_at = None;
                d.arm_seek_fade(from);
            }
            MixCmd::Spin { deck, motion } => {
                let d = &mut s.decks[deck.index()];
                let deck_rate = d.rate.current();
                match motion {
                    SpinMotion::Brake | SpinMotion::SpinBack => {
                        d.playing = false;
                        // CLEARING THIS IS THE POINT. A stop normally hands
                        // back the frames its fade sounded, so the playhead
                        // stays where the button was pressed. A brake is the
                        // opposite: the record travelled while it wound down,
                        // and it stays where it stopped.
                        d.pause_at = None;
                        d.ended = false;
                        let (target, secs, end, total) = match motion {
                            SpinMotion::SpinBack => (
                                SPINBACK_PEAK,
                                SPINBACK_THROW_SECS,
                                MotorEnd::Then(0.0, SPINBACK_FALL_SECS),
                                SPINBACK_THROW_SECS + SPINBACK_FALL_SECS,
                            ),
                            _ => (0.0, BRAKE_SECS, MotorEnd::Retire, BRAKE_SECS),
                        };
                        d.scratch.motor(deck_rate, target, secs, end);
                        // The gain falls as the pitch does, so the record is
                        // silent exactly when it has stopped rather than
                        // before it.
                        d.transport.slew(0.0, total);
                    }
                    SpinMotion::SoftStart => {
                        d.playing = true;
                        d.ended = false;
                        d.pause_at = None;
                        d.scratch.spin_up_from(0.0, deck_rate, SOFT_START_SECS);
                        d.transport.slew(1.0, SLEW_SECS);
                    }
                }
            }
            MixCmd::SetFreeze { deck, secs } => {
                let device = f64::from_bits(shared.device_rate_bits.load(Ordering::Acquire));
                let d = &mut s.decks[deck.index()];
                match secs {
                    Some(source_secs) => {
                        if d.pcm.is_none() || !d.playing || !(device > 0.0) {
                            return;
                        }
                        // Source seconds become output seconds through the
                        // platter's OWN rate: under a hand, a motor or a
                        // reverse hold that is the number that matters, and
                        // the tempo fader is not it. A stopped platter has no
                        // beat arriving to be the size of, so it falls back
                        // to unity rather than dividing by zero.
                        let platter = d.clock.platter_rate.abs();
                        let held = match platter > 1e-6 {
                            true => source_secs / platter,
                            false => source_secs,
                        };
                        let Some(held) = knob64(held, 0.0, 60.0) else { return };
                        d.chain.freeze_mut().hold((held * device) as usize, device as f32);
                    }
                    None => d.chain.freeze_mut().release(),
                }
            }
            MixCmd::SetGrid { deck, grid } => {
                let d = &mut s.decks[deck.index()];
                // A record is on its way in behind a fade: the grid is for
                // THAT one. Writing it here would re-rule the outgoing
                // track's last forty milliseconds to the incoming track's
                // tempo, and then lose it at the swap.
                if let Some(load) = d.pending.as_mut() {
                    load.grid = grid;
                    return;
                }
                d.grid = grid;
                // Re-rule the clock on the spot rather than waiting for the
                // next buffer: a grid landing mid-phrase should lock the
                // LFOs to it now, not a callback later.
                let source_rate =
                    d.pcm.as_ref().map(|pcm| pcm.sample_rate().max(1) as f64).unwrap_or(0.0);
                let pos_secs = match source_rate > 0.0 {
                    true => d.playhead_frames() / source_rate,
                    false => 0.0,
                };
                // Through the shared helper, like the render prelude and
                // the snapshot: a paused deck with a record on it still
                // has a TEMPO -- the beat is still half a second long --
                // and a splat owns the platter outright. Asking "is it
                // playing" here instead reported a grid with no beat
                // length to anything that read the clock before the deck
                // was started.
                let platter = deck_platter(d);
                d.clock = DeckClock::at(d.grid.as_ref(), pos_secs, platter, 0.0);
            }
            MixCmd::SetEqBand { deck, band, gain } => {
                s.decks[deck.index()].chain.eq_mut().set_band(band, gain)
            }
            MixCmd::SetFilter { deck, position } => {
                s.decks[deck.index()].chain.eq_mut().set_filter(position)
            }
            // The stem is the one raw number that reaches this thread as an
            // array index rather than a typed enum. The handle's setters
            // check it; a command pushed raw -- which is what every test
            // does -- did not, and the callback must not have a way to
            // unwind. Refused here, and it says so.
            MixCmd::SetStemGain { deck, stem, gain } => {
                verify_or!(stem < STEM_COUNT, { return });
                s.decks[deck.index()].stem_gain[stem].slew(gain, SLEW_SECS * 2.0);
            }
            MixCmd::SetLoopSpan { deck, span, seek } => {
                let d = &mut s.decks[deck.index()];
                let Some(pcm) = d.pcm.as_ref() else {
                    d.loop_span = None;
                    return;
                };
                let rate = pcm.sample_rate().max(1) as f64;
                // Clamp OUT to the real frame count: the seconds->frames
                // round trip can land a hair ABOVE it, and an OUT past the
                // last frame lets the end-of-track check win over the wrap
                // — a dead deck with LOOP lit. (On a streaming track that
                // is the expected length: a span past the decoded edge
                // waits there like any other read.)
                let frames = pcm.expected_len() as f64;
                // The span the head belonged to, taken before it is replaced.
                let was = d.loop_span;
                d.loop_span = span.map(|(start, end)| {
                    (start.max(0.0) * rate, (end.max(0.0) * rate).min(frames))
                });
                // A playhead stranded past the new OUT lands modulo NOW.
                // The render wrap would catch it on the next callback
                // anyway, but a PAUSED deck never renders — without this,
                // a resize on a paused deck parks the playhead outside the
                // span until play is pressed.
                if let Some((start, end)) = d.loop_span {
                    let from = d.playhead_frames();
                    // Behind IN is folded FORWARD only when the head
                    // belonged to the span that just changed. A head
                    // sitting behind a loop it was never in is playing its
                    // way into it, deliberately and audibly, and folding it
                    // would teleport it over the run-up.
                    let belonged = matches!(seek, crate::decks::LoopSeek::Changed)
                        && was.is_some_and(|(was_start, was_end)| {
                            from >= was_start && from < was_end
                        });
                    if from >= end || (belonged && from < start) {
                        d.seek_frames(wrapped_into_span(from, start, end));
                        // A live resize yanking a playing playhead is a
                        // jump like any other and gets the same blend.
                        d.arm_seek_fade(from);
                    }
                }
            }
            MixCmd::SetMute { deck, muted } => {
                s.decks[deck.index()].mute.slew(if muted { 0.0 } else { 1.0 }, SLEW_SECS);
            }
            MixCmd::SetGain { deck, gain } => s.decks[deck.index()].gain.slew(gain, SLEW_SECS),
            MixCmd::SwapDecks => s.decks.swap(0, 1),
            MixCmd::SetCrossfader { position, secs } => s.fader.slew(position, secs),
            MixCmd::SetBlendBand { deck, band, gain } => {
                s.decks[deck.index()].chain.eq_mut().set_blend_band(band, gain);
            }
            MixCmd::SetBlendStem { deck, stem, gain } => {
                verify_or!(stem < STEM_COUNT, { return });
                s.decks[deck.index()].blend_stem[stem].slew(gain, BLEND_SECS);
            }
            MixCmd::ClearBlend(deck) => {
                let d = &mut s.decks[deck.index()];
                d.chain.eq_mut().clear_blend();
                for ramp in &mut d.blend_stem {
                    ramp.slew(1.0, BLEND_SECS);
                }
            }
            MixCmd::SetCurve(curve) => s.curve = curve,
            MixCmd::SetMaster(gain) => s.master.slew(gain, SLEW_SECS),
            MixCmd::StartVoice { alloc, pcm } => {
                if s.sfx.len() >= MAX_SFX_VOICES {
                    // The pool is full: the oldest voice makes room, and
                    // its buffer goes back to the UI like any other.
                    let oldest = s.sfx.remove(0);
                    retire(shared, Retired::Track(oldest.pcm));
                }
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
            MixCmd::StopVoice(id) => {
                // Fast declick: a stopped voice ramps out over one slew and
                // is reaped by the render pass.
                for v in s.sfx.iter_mut().filter(|v| v.id == id) {
                    v.loop_on = false;
                    v.gain.slew(0.0, SLEW_SECS);
                    v.done = true;
                }
            }
            MixCmd::SetPadVoicesGain { pad, gain } => {
                for v in s.sfx.iter_mut().filter(|v| v.pad == pad && !v.done) {
                    v.gain.slew(gain, SLEW_SECS);
                }
            }
            MixCmd::SetDeckCue { deck, on } => s.cue_deck[deck.index()] = on,
            MixCmd::SetCueMode(mode) => s.cue_mode = mode,
            MixCmd::InstallPreview { pcm, autoplay } => {
                if let Some(old) = s.preview.pcm.replace(pcm) {
                    retire(shared, Retired::Track(old));
                }
                s.preview.cursor_fp = 0;
                s.preview.ended = false;
                s.preview.playing = autoplay;
                s.preview.gain = Ramp::at(0.0);
                if autoplay {
                    s.preview.gain.slew(1.0, SLEW_SECS);
                }
            }
            MixCmd::ClearPreview => {
                s.preview.playing = false;
                s.preview.ended = false;
                s.preview.cursor_fp = 0;
                s.preview.gain = Ramp::at(0.0);
                if let Some(old) = s.preview.pcm.take() {
                    retire(shared, Retired::Track(old));
                }
            }
            MixCmd::SetPreviewPlaying(playing) => {
                if s.preview.pcm.is_none() {
                    return;
                }
                // Play on a parked player starts the track over — the
                // player's one transport button should never be a dead end.
                if playing && s.preview.ended {
                    s.preview.cursor_fp = 0;
                    s.preview.ended = false;
                }
                s.preview.playing = playing;
                s.preview.gain.slew(if playing { 1.0 } else { 0.0 }, SLEW_SECS);
            }
            MixCmd::SeekPreviewFraction(fraction) => {
                let Some(pcm) = s.preview.pcm.as_ref() else { return };
                let len = pcm.frames.len() as f64;
                let frame = (fraction.clamp(0.0, 1.0) * len).clamp(0.0, (len - 1.0).max(0.0));
                s.preview.cursor_fp = (frame * FP_ONE as f64) as u64;
                s.preview.ended = false;
            }
            MixCmd::SetDrumBank(bank) => {
                if let Some(old) = s.score_preview.set_drum_bank(bank.clone()) {
                    retire(shared, Retired::Bank(old));
                }
                if let Some(old) = s.synth.set_drum_bank(bank) {
                    retire(shared, Retired::Bank(old));
                }
            }
            MixCmd::ScorePreviewPlay { sequence, piano, events } => {
                let sample_rate = sequence.sample_rate.max(1);
                if let Some(piano) = piano {
                    let old = s.score_preview.replace_instruments(piano, sample_rate);
                    retire(shared, Retired::Piano(old));
                }
                if let Some(events) = events {
                    let old = std::mem::replace(&mut s.score_preview.piano_events, events);
                    retire(shared, Retired::Events(old));
                }
                if let Some(old) = s.score_preview.play(sequence) {
                    retire(shared, Retired::Sequence(old));
                }
            }
            MixCmd::ScorePreviewStop => s.score_preview.stop(true),
            MixCmd::SetSynthClock(clock) => s.synth.set_clock(clock),
            MixCmd::SetSynthPlaying(playing) => s.synth.set_playing(playing),
            MixCmd::SetSynthPattern { track, pattern } => s.synth.set_pattern(track, pattern),
            MixCmd::SetIronfishPatch(patch) => s.synth.set_patch(patch),
            MixCmd::SetIronfishParam { param, value } => s.synth.set_param(param, value),
            MixCmd::ReplaceSynthEngines(engines) => {
                let old = s.synth.replace_engines(engines);
                retire(shared, Retired::SynthEngines(old));
            }
            MixCmd::SetStripGain { strip, gain } => s.program_mix.set_gain(strip, gain),
            MixCmd::SetStripMuted { strip, muted } => s.program_mix.set_muted(strip, muted),
            MixCmd::SetStripSoloed { strip, soloed } => s.program_mix.set_soloed(strip, soloed),
            MixCmd::SetMasterDynamics(params) => s.program_mix.set_master_params(params),
            MixCmd::SetMasterDynamicsParam { param, value } => {
                s.program_mix.set_master_param(param, value)
            }
            MixCmd::SetMasterDynamicsBypass(bypass) => {
                s.program_mix.set_master_bypass(bypass)
            }
        }
    }

    /// Everything a deck holds that is worth handing back: the track, its
    /// stems and its splat grid.
    fn retire_deck_media(shared: &Shared, d: &mut DeckVoice) {
        if let Some(pcm) = d.pcm.take() {
            retire(shared, Retired::Pcm(pcm));
        }
        if let Some(stems) = d.stems.take() {
            retire(shared, Retired::Stems(stems));
        }
        if let Some(splat) = d.splat.take() {
            retire(shared, Retired::Splat(splat.grid, splat.frames));
        }
    }

    fn publish_snapshot(s: &MixState, shared: &Shared, serial: u64) {
        let mut snapshot = MixSnapshot {
            fader_current: s.fader.current,
            preview_installed: s.preview.pcm.is_some(),
            preview_playing: s.preview.playing,
            preview_ended: s.preview.ended,
            score_playing: s.score_preview.playing,
            score_pos: s.score_preview.pos,
            synth: s.synth.snapshot(),
            strips: s.program_mix.strip_snapshots(),
            master_fx: s.program_mix.master_snapshot(),
            serial,
            ..MixSnapshot::default()
        };
        if let Some(pcm) = &s.preview.pcm {
            snapshot.preview_position_secs =
                s.preview.cursor_fp as f64 / FP_ONE as f64 / pcm.sample_rate.max(1) as f64;
            snapshot.preview_duration_secs = pcm.seconds();
        }
        for (i, d) in s.decks.iter().enumerate() {
            snapshot.decks[i] = DeckSnap {
                playhead_frames: d.playhead_frames(),
                sample_rate: d.pcm.as_ref().map_or(0, DeckPcm::sample_rate),
                playing: d.playing,
                scratching: d.scratch.active(),
                ended: d.ended,
                rate_current: d.rate.current(),
                platter_rate: deck_platter(d),
                clock: d.clock,
                splat: d.splat.as_ref().map(SplatState::snapshot),
            };
        }
        shared.snapshot.write(snapshot);
    }

    // ---- the device callback ------------------------------------------------

    /// Mix one device buffer. The buffer must already be zeroed. Nothing
    /// here waits on anyone: the commands are drained from a ring, the
    /// slot audio is read from rings, and the state is this engine's own.
    pub fn render(&mut self, device_rate: f64, output: &mut AudioBuffer) {
        // Every buffer, not once: the flag is per thread and some hosts
        // reset it between callbacks. See
        // `music_dsp::flush_denormals_to_zero`.
        crate::music_dsp::flush_denormals_to_zero();
        if device_rate <= 0.0 {
            return;
        }
        let render_started = crate::clock::Instant::now();
        self.drain_commands();
        let frames = output.frame_count();
        let shared = &*self.shared;
        shared.device_rate_bits.store(device_rate.to_bits(), Ordering::Release);
        let buffer_start = shared.device_frames.fetch_add(frames as u64, Ordering::AcqRel);
        self.serial = self.serial.wrapping_add(1);

        // The slot rings, as of this buffer: a flush the producer asked for
        // lands here, then the window the frame loop may read is fixed.
        let mut slots = [SlotView { paused: true, source_rate: 0.0, playback_rate: 1.0, base: 0, avail: 0 }; 2];
        for (i, slot) in shared.video.iter().enumerate() {
            let flush_gen = slot.flush_gen.load(Ordering::Acquire);
            let mut read = slot.read_pos.load(Ordering::Relaxed);
            if flush_gen != self.slot_flush_seen[i] {
                self.slot_flush_seen[i] = flush_gen;
                let flush_at = slot.flush_at.load(Ordering::Acquire);
                if flush_at > read {
                    read = flush_at;
                    slot.read_pos.store(read, Ordering::Release);
                }
                self.state.video[i].cursor = 0.0;
            }
            let write = slot.write_pos.load(Ordering::Acquire);
            slots[i] = SlotView {
                paused: slot.paused.load(Ordering::Relaxed),
                source_rate: f64::from_bits(slot.source_rate_bits.load(Ordering::Relaxed)),
                playback_rate: slot.playback_rate(),
                base: read,
                avail: write.saturating_sub(read) as usize,
            };
        }

        let s = &mut self.state;
        let rate = device_rate as f32;
        let channels = output.channel_count();
        let mut peaks = [0.0f32; 5];
        s.rendered_frames = buffer_start;


        if let Some(scheduled) = s.scheduled_video {
            if !scheduled.started && scheduled.target_frame < buffer_start {
                s.scheduled_video = None;
                shared.video[scheduled.to.index()].paused.store(true, Ordering::Relaxed);
                slots[scheduled.to.index()].paused = true;
                s.video[scheduled.to.index()].gain = Ramp::at(0.0);
                shared.transition
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
                    shared.transition
                        .publish_phase(VideoTransitionPhase::Completed, buffer_start);
                } else if buffer_start > scheduled.target_frame && scheduled.fade_frames > 0 {
                    // Catch a running fade up to the physical device clock
                    // after a buffer the device itself skipped.
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

        // A parked load takes over HERE, before the sources are lifted and
        // never inside the frame loop: the loop reads one reference per
        // deck for the whole buffer, so a swap inside it renders the
        // retired record at the incoming one's gain. The swap lands on the
        // first buffer boundary after the fade -- a hand-over, not a seam.
        for d in s.decks.iter_mut() {
            if d.transport.current <= 0.0 {
                if let Some(load) = d.pending.take() {
                    Self::seat_record(shared, d, load.pcm, load.play, load.grid);
                }
            }
        }

        // Deck sources are lifted out of the frame loop: one reference count
        // per buffer instead of one per sample, and the borrow checker can
        // then see that the voice state and its PCM are disjoint.
        let deck_pcm: [Option<DeckPcm>; 2] =
            [s.decks[0].pcm.clone(), s.decks[1].pcm.clone()];
        let deck_stems: [Option<Arc<TrackStems>>; 2] =
            [s.decks[0].stems.clone(), s.decks[1].stems.clone()];
        let mut deck_peaks = [0.0f32; 2];
        // Every chain that is not a deck's gets the same once-a-buffer
        // preparation the decks' do, and borrows a deck's clock: the mix,
        // the pads and the synth tracks have no tempo of their own, so a
        // beat-locked effect on any of them follows whichever deck is
        // nominated -- deck A until something asks otherwise.
        {
            let clock = s.decks[s.chain_clock_deck.index()].clock;
            s.master_chain.prepare_block(&clock, rate, frames);
            for chain in s.source_chains.iter_mut() {
                chain.prepare_block(&clock, rate, frames);
            }
        }
        for voice in s.decks.iter_mut() {
            // Where the beat will be at the END of this buffer. Read at the
            // end and not the start because that is what the locked LFOs
            // compare their own projected phase against; at the start they
            // would chase a beat one buffer stale.
            let source_rate =
                voice.pcm.as_ref().map(|pcm| pcm.sample_rate().max(1) as f64).unwrap_or(0.0);
            // The platter, not the tempo fader: a hand on the record is the
            // rate the music is actually going round at, and a stopped deck
            // is going round at nothing. Through the shared helper, so a
            // running splat -- which owns the platter outright at 1.0,
            // whatever the fader says -- reads the same here as it does in
            // the snapshot and in every setter that asks between buffers.
            let platter = deck_platter(voice);
            // The playhead and not the read head: under a running splat
            // they are different places, because the splat's cell reads
            // from its own window while the record's own position goes on
            // meaning what it always meant.
            let pos_frames = voice.playhead_frames();
            let pos_secs = match source_rate > 0.0 {
                true => pos_frames / source_rate,
                false => 0.0,
            };
            // Only a voice that will actually READ travels. A parked deck
            // whose fade has landed goes nowhere, and predicting travel for
            // it would walk its clock away from its playhead.
            //
            // A splat deck's read path takes ONE exit -- not playing -- and
            // not the ordinary four, so asking the ordinary question of it
            // keeps its clock travelling through a pause.
            let reads = if voice.splat.as_ref().is_some_and(|splat| splat.active) {
                voice.playing
            } else {
                voice.scratch.active()
                    || !(voice.transport.current <= 0.0 && (!voice.playing || voice.ended))
            };
            let travel_secs = match reads {
                true => platter * frames as f64 / device_rate,
                false => 0.0,
            };
            // A span owns the playhead on the read path (below, per frame);
            // the clock's single-shot prediction folds the same way, so a
            // roll's landing does not publish a fraction the head is about
            // to leave behind. Two-sided, because a reversed platter can
            // leave a span at IN.
            let predicted_secs = match voice.loop_span {
                Some((start, end)) if source_rate > 0.0 => {
                    let predicted = pos_frames + travel_secs * source_rate;
                    let folded = if predicted >= end || (platter < 0.0 && predicted < start) {
                        wrapped_into_span(predicted, start, end)
                    } else {
                        predicted
                    };
                    folded / source_rate
                }
                _ => pos_secs + travel_secs,
            };
            voice.clock = DeckClock::at(voice.grid.as_ref(), predicted_secs, platter, 0.0);
            let clock = voice.clock;
            // One call for the whole chain rather than a list per stage:
            // there are two chains to keep fed, and two hand-written lists
            // of the same calls is how one of them quietly stops getting a
            // new effect's preparation.
            voice.chain.prepare_block(&clock, rate, frames);
            // The ghosts move HERE, once per buffer, and not in the frame
            // loop below: their rate is latched so a buffer is one
            // multiply, and the read path has four early exits (no pcm,
            // empty pcm, splat running, paused and faded) that a ghost must
            // not be caught by. It runs whatever the record is doing.
            if let Some(ghost) = voice.slip.as_mut() {
                ghost.pos += ghost.step * frames as f64;
                if let Some((start, end)) = ghost.span {
                    if ghost.pos >= end {
                        ghost.pos = wrapped_into_span(ghost.pos, start, end);
                    }
                }
            }
            voice.rolls.advance(frames as f64);
        }
        // Mixing starts HERE and not at the frame loop: the score preview
        // and the synth voices render their whole buffer before the loop
        // that sums them, so timing from the loop alone books real mixing
        // to the setup and reports a preparation that costs four times
        // what the mix does.
        let mix_started = crate::clock::Instant::now();
        s.score_preview.render_block(frames, device_rate);
        s.synth.render_block(buffer_start, frames, device_rate);
        s.program_mix.begin_block();

        // The headphone cue bus. `buffer_start` keeps the ring's write
        // position on the device clock, so a buffer the device skipped
        // writes nothing and the phones re-prime, exactly mirroring what
        // the room heard.
        let cue_armed = shared.cue_ring.armed.load(Ordering::Relaxed);
        let cue_deck_on = s.cue_deck;
        let cue_mode = s.cue_mode;
        let mut cue_pos = buffer_start;
        if cue_armed {
            shared.cue_ring.main_rate_bits.store(device_rate.to_bits(), Ordering::Relaxed);
        }

        for frame in 0..frames {
            let output_frame = buffer_start.saturating_add(frame as u64);
            // One expression decides AND binds. This used to be a bool,
            // a block boundary, and then an `expect("checked above")` --
            // true when it was written, and a panic on the audio callback
            // the day anyone edits the gap between the two. There is no
            // gap now, so there is nothing left to get wrong.
            let starting = s.scheduled_video.filter(|scheduled| {
                !scheduled.started && output_frame >= scheduled.target_frame
            });
            if let Some(mut scheduled) = starting {
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
                shared.video[scheduled.to.index()].paused.store(false, Ordering::Relaxed);
                slots[scheduled.to.index()].paused = false;
                let destination = &mut s.video[scheduled.to.index()];
                if scheduled.fade_frames == 0 {
                    destination.gain = Ramp::at(1.0);
                } else {
                    destination.gain.slew(1.0, fade_secs);
                }
                shared.transition.publish_phase(VideoTransitionPhase::Started, output_frame);
            }

            // Video buses (summed, then the orthogonal program mute).
            let mut video = (0.0f32, 0.0f32);
            for (i, bus) in s.video.iter_mut().enumerate() {
                let gain = bus.gain.tick(rate);
                let slot = slots[i];
                if slot.paused || slot.avail < 2 || slot.source_rate <= 0.0 {
                    continue;
                }
                let index = bus.cursor as usize;
                if index + 1 >= slot.avail {
                    continue;
                }
                let fraction = (bus.cursor - index as f64) as f32;
                let (al, ar) = shared.video[i].frame_at(slot.base + index as u64);
                let (bl, br) = shared.video[i].frame_at(slot.base + index as u64 + 1);
                video.0 += (al + (bl - al) * fraction) * gain;
                video.1 += (ar + (br - ar) * fraction) * gain;
                bus.cursor += (slot.source_rate / device_rate) * slot.playback_rate;
            }
            let program_mute = s.video_mute.tick(rate);
            video.0 *= program_mute;
            video.1 *= program_mute;
            // The video bus's own chain: after the program mute, so a
            // muted bus does not keep a tail ringing, and before the strip,
            // which is the seat ahead of the fader the deck chains have.
            let video = {
                let out = s.chain_mut(ChainTarget::Video).process([video.0, video.1], rate);
                (out[0], out[1])
            };

            // Decks under the crossfader.
            let position = s.fader.tick(rate);
            let fader = crossfader_gains(position, s.curve);
            let mut deck_out = [(0.0f32, 0.0f32); 2];
            let mut cue = (0.0f32, 0.0f32);
            for (i, d) in s.decks.iter_mut().enumerate() {
                let transport = d.transport.tick(rate);
                if transport <= 0.0 {
                    if let Some(at) = d.pause_at.take() {
                        // The fade is over and nothing is audible: give back
                        // the frames it sounded, so pause leaves the
                        // playhead exactly where it was pressed.
                        d.seek_frames(at);
                    }
                }
                let gain = d.gain.tick(rate) * d.mute.tick(rate) * transport;
                let side = if i == 0 { fader.0 } else { fader.1 };
                let deck_rate = d.rate.tick(rate);
                let key_ratio = d.key_ratio.tick(rate) as f64;
                // WHERE the record is, not just how fast: the scratch ramp
                // measures its error against the finger's own place, which
                // is what stops a dropped or coalesced event becoming drift
                // the record never recovers.
                let pos_secs = d
                    .pcm
                    .as_ref()
                    .map(|pcm| d.pos / pcm.sample_rate().max(1) as f64)
                    .unwrap_or(0.0);
                let scratch_rate = d.scratch.tick(rate, deck_rate, pos_secs);
                let scratching = d.scratch.active();
                let reverse = scratching && scratch_rate < 0.0;
                let mut stem_gain = [0.0f32; STEM_COUNT];
                for ((slot, ramp), blend) in stem_gain
                    .iter_mut()
                    .zip(d.stem_gain.iter_mut())
                    .zip(d.blend_stem.iter_mut())
                {
                    *slot = ramp.tick(rate) * blend.tick(rate);
                }
                let Some(pcm) = deck_pcm[i].as_ref() else { continue };
                if pcm.is_empty() {
                    continue;
                }
                let natural_step = pcm.sample_rate() as f64 / device_rate;
                // The grid owns time on a streaming track too: a cell past
                // the decoded edge reads silence until its audio lands,
                // and a click means the same thing however far the decode
                // is.
                if let Some(splat) = d.splat.as_mut().filter(|splat| splat.active) {
                    // Every row derives from one source clock. Tempo acts
                    // on that clock, so sync and pitch moves affect all rows
                    // equally without moving their source spans.
                    if !d.playing {
                        continue;
                    }
                    let frame = render_splat_source(
                        splat,
                        pcm,
                        deck_stems[i].as_deref(),
                        stem_gain,
                        natural_step * deck_rate as f64,
                    );
                    let toned = d.chain.process(frame, rate);
                    let pre = [toned[0] * gain, toned[1] * gain];
                    deck_peaks[i] = deck_peaks[i].max(pre[0].abs()).max(pre[1].abs());
                    deck_out[i] = (pre[0] * side, pre[1] * side);
                    if cue_armed && cue_deck_on[i] {
                        let (cue_left, cue_right) = match cue_mode {
                            CueMode::Raw => (frame[0], frame[1]),
                            CueMode::Pfl => (toned[0], toned[1]),
                            CueMode::PostFader => (deck_out[i].0, deck_out[i].1),
                        };
                        cue.0 += cue_left;
                        cue.1 += cue_right;
                    }
                    continue;
                }
                // A hand on the record plays even a paused deck; that is the
                // whole point of scrubbing. And a deck whose transport has
                // not reached silence is still sounding, so it still reads:
                // that is what makes the fade a fade rather than a shorter
                // click.
                if !scratching && transport <= 0.0 && (!d.playing || d.ended) {
                    continue;
                }
                let source = DeckSource {
                    pcm,
                    stems: deck_stems[i].as_deref(),
                    stem_chunk: std::cell::Cell::new((0, 0)),
                    stem_gain,
                    stem_blend: d.stem_blend.tick(rate),
                };
                let length = pcm.len();

                // Tempo and pitch, split into the two stages that can each
                // deliver one of them. The stretcher changes duration at
                // constant pitch; the reader changes both together. Ask the
                // stretcher for `stretch_ratio` and the reader for `read_rate`
                // and their product is the tempo while the reader alone is the
                // pitch.
                //
                //   key lock ON : the shift is measured from the track's own
                //                 key, so tempo must not reach the ear.
                //   key lock OFF: the deck is a turntable — the shift rides on
                //                 top of whatever the speed already did.
                let pitch_intent = key_ratio * if d.keylock { 1.0 } else { deck_rate as f64 };
                // Clamp the STRETCHER, then recover the reader from it, so
                // the product is still exactly the tempo. At the corners
                // (rate 4.0 against a −12 shift) the pitch gives way and the
                // tempo does not: the grid, the sync and the loops are all
                // counted in tempo.
                let stretch_ratio = (deck_rate as f64 / pitch_intent)
                    .clamp(STRETCH_RATIO_MIN, STRETCH_RATIO_MAX);
                let read_rate = deck_rate as f64 / stretch_ratio;
                // The stretcher earns its place only when it has stretching to
                // do; scratching and a unity ratio both read the source
                // directly, so an untouched deck is the sample the decoder
                // produced.
                // With hysteresis: engaged past one threshold, released
                // below a smaller one, so a rate that hovers at unity does
                // not flip the path every buffer.
                let off_unity = (stretch_ratio - 1.0).abs();
                let want_stretch = !scratching
                    && length > WSOLA_WINDOW + 1
                    && if d.stretching {
                        off_unity > STRETCH_BYPASS_EPSILON
                    } else {
                        off_unity > STRETCH_ENGAGE_EPSILON
                    };
                if want_stretch != d.stretching {
                    // The one jump in the transport that never blended. The
                    // two paths hand the PLAYHEAD over exactly and disagree
                    // on PHASE, so the splice is a step on anything but a
                    // steady tone -- half full scale on a low one.
                    let from = d.playhead_frames();
                    if want_stretch {
                        d.stretch.reset_to(d.pos);
                        d.reader.reset();
                        d.stretch_tail = None;
                        // Going IN, the outgoing stream is the direct read,
                        // which the ordinary seek blend reproduces exactly.
                        d.arm_seek_fade(from);
                    } else {
                        // Continue from the frame the ear is at, not from
                        // the search's ideal anchor: the two can differ by
                        // a search width, and that difference is a skip.
                        d.pos = d.stretch.position();
                        // Coming OUT, it is not: nothing but the stretcher
                        // can produce the stretcher's tail, so it goes on
                        // producing it. Its own reader comes with it,
                        // untouched by the direct path, so the tail is a
                        // continuation of the very stream being faded.
                        let total = (SEEK_XFADE_SECS * pcm.sample_rate().max(1) as f64).max(1.0);
                        d.stretch_tail = Some((total, total));
                    }
                    d.stretching = want_stretch;
                }

                // A span owns the playhead, on both read paths. Wrap BEFORE
                // the read so no frame past the out point is ever emitted.
                if let Some((start, end)) = d.loop_span {
                    // Two-sided, because a record running backwards leaves
                    // a loop at IN. `wrapped_into_span` already folds a
                    // position below the span (it is a remainder, not a
                    // clamp), so the landing needs nothing new. The GHOST's
                    // own wrap above stays one-sided on purpose: its step
                    // is latched from the deck's tempo when it is armed and
                    // never goes negative, so it only ever leaves at OUT.
                    let head = d.playhead_frames();
                    if head >= end || (reverse && head < start) {
                        // Land MODULO the length, keeping the overshoot.
                        // Resetting to IN exactly discards up to a step
                        // per lap — a held loop walks audibly early —
                        // and it is also what catches a playhead
                        // stranded past OUT by a live resize: modulo
                        // continues the subdivision in phase instead of
                        // re-triggering the downbeat at IN.
                        let landed = wrapped_into_span(d.playhead_frames(), start, end);
                        // The finger did not come round with the record,
                        // so its target does. Without this the error is a
                        // whole loop wide and a hand on the record would
                        // drive it at the clamp until it came off.
                        let moved = landed - d.playhead_frames();
                        d.scratch.note_wrap(moved / pcm.sample_rate().max(1) as f64);
                        d.seek_frames(landed);
                    }
                }
                // Where THIS frame is read from, for the wrap crossfade
                // below — captured before the read advances anything.
                let loop_pos = d.playhead_frames();

                let mut ran_out = false;
                let frame = if d.stretching {
                    d.stretch.set_ratio(stretch_ratio);
                    let read = {
                        let stretch = &mut d.stretch;
                        let reader = &mut d.reader;
                        // The stretcher never wraps on its own any more: a
                        // span owns the wrap, and a deck without one stops
                        // at the end of the track.
                        let mut pull = || stretch.next(&source, false);
                        // Device conversion AND the pitch shift in one step:
                        // the stretcher already spent the tempo, so whatever
                        // the reader does to the rate here is heard as pitch.
                        reader.read(natural_step * read_rate, &mut pull)
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
                        ran_out = true;
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
                        // A hand on the record overrules the key shift: a
                        // scratch is pitch and tempo welded together, and
                        // that is the sound being asked for.
                        let effective =
                            if scratching { scratch_rate as f64 } else { read_rate };
                        d.pos += natural_step * effective;
                        if d.pos < 0.0 {
                            // Scrubbed off the front: the record stops there.
                            d.pos = 0.0;
                        }
                        out
                    }
                };
                // The stretcher's tail, mixed under the direct read that
                // has taken over from it.
                let frame = match d.stretch_tail {
                    Some((left, total)) if !d.stretching => {
                        let old = {
                            let stretch = &mut d.stretch;
                            let reader = &mut d.reader;
                            let mut pull = || stretch.next(&source, false);
                            reader.read(natural_step * read_rate, &mut pull)
                        };
                        d.stretch_tail =
                            if left > 1.0 { Some((left - 1.0, total)) } else { None };
                        match old {
                            Some(old) => {
                                let t = (1.0 - left / total) as f32;
                                [old[0] + (frame[0] - old[0]) * t, old[1] + (frame[1] - old[1]) * t]
                            }
                            None => frame,
                        }
                    }
                    _ => frame,
                };
                if ran_out {
                    // A span must never end the deck. The stretcher's read
                    // head cannot reach the last WSOLA window of the track,
                    // so a keylocked span whose OUT hugs the end stalls
                    // into a failed read before the wrap check can fire —
                    // catch it here and wrap to IN. Plain IN, not modulo:
                    // the stalled position would re-land in the same dead
                    // zone and the deck would hang there in silence.
                    if let Some((start, _)) = d.loop_span {
                        d.seek_frames(start);
                        continue;
                    }
                    // The decoded edge of a track still streaming in is
                    // not the end of the track: the deck waits there —
                    // silent, still playing, the playhead parked — and
                    // carries on the moment the next chunk lands.
                    if !pcm.complete() {
                        continue;
                    }
                    // Once, not once a frame: the read gate keys on the
                    // transport, so the deck keeps reading here until its
                    // ramp lands -- and without this guard the end would be
                    // announced again on every frame of the fade, which
                    // fills the event ring and makes the retire path drop
                    // payloads on this thread.
                    if !d.ended {
                        d.playing = false;
                        d.transport.slew(0.0, SLEW_SECS);
                        d.ended = true;
                        push_event(shared, MixEvent::DeckEnded(if i == 0 { DeckId::A } else { DeckId::B }));
                    }
                    continue;
                }
                // The wrap is a crossfade, not a splice and not a duck:
                // over the last few ms of the loop the tail is blended into
                // the material RUNNING UP TO IN, reaching IN exactly at the
                // wrap — the music simply keeps playing through the seam.
                // A pure function of position, so there is no fade state to
                // unwind, and LINEAR, which sums a sustained signal to
                // exactly itself where equal-power would bump it 3 dB.
                let frame = match d.loop_span {
                    // The pre-roll has to exist on the track, so a span
                    // starting at the very head plays a raw splice instead.
                    Some((start, end)) if start >= 1.0 => {
                        let xf = (LOOP_XFADE_SECS * pcm.sample_rate() as f64)
                            .min((end - start) * 0.15)
                            .min(start)
                            .max(1.0);
                        if loop_pos >= end - xf && loop_pos < end {
                            let u = loop_pos - (end - xf);
                            let src = start - xf + u;
                            let index = src as usize;
                            let fraction = (src - index as f64) as f32;
                            let a = source.frame(index.min(length - 1));
                            let b = source.frame((index + 1).min(length - 1));
                            let t = (u / xf) as f32;
                            [
                                frame[0] + (a[0] + (b[0] - a[0]) * fraction - frame[0]) * t,
                                frame[1] + (a[1] + (b[1] - a[1]) * fraction - frame[1]) * t,
                            ]
                        } else {
                            frame
                        }
                    }
                    _ => frame,
                };
                // The seek blend: after a commanded jump the OUTGOING
                // stream keeps sounding for a few ms, at its own place and
                // the deck's own speed, while the incoming one takes over.
                // Same idea as the wrap crossfade, armed by a jump instead
                // of a span — and applied after it, so a jump that lands on
                // a seam composes instead of fighting.
                let frame = match d.seek_fade.take() {
                    Some(mut fade) => {
                        let index = fade.pos as usize;
                        let fraction = (fade.pos - index as f64) as f32;
                        let a = source.frame(index.min(length - 1));
                        let b = source.frame((index + 1).min(length - 1));
                        let t = (fade.left / fade.total).clamp(0.0, 1.0) as f32;
                        let out = [
                            frame[0] + (a[0] + (b[0] - a[0]) * fraction - frame[0]) * t,
                            frame[1] + (a[1] + (b[1] - a[1]) * fraction - frame[1]) * t,
                        ];
                        fade.pos += natural_step * deck_rate as f64;
                        fade.left -= 1.0;
                        // An outgoing stream that runs off the track just
                        // ends the blend early rather than looping around.
                        if fade.left > 0.0 && fade.pos < length as f64 {
                            d.seek_fade = Some(fade);
                        }
                        out
                    }
                    None => frame,
                };
                let toned = d.chain.process(frame, rate);
                let pre = [toned[0] * gain, toned[1] * gain];
                deck_peaks[i] = deck_peaks[i].max(pre[0].abs()).max(pre[1].abs());
                deck_out[i] = (pre[0] * side, pre[1] * side);
                // The headphone tap. A deck that bailed out above (empty,
                // paused, ran off the end) never reaches here — PFL of a
                // stopped channel is silent on hardware too.
                if cue_armed && cue_deck_on[i] {
                    let (cue_left, cue_right) = match cue_mode {
                        // The final source frame (post loop-wrap and seek
                        // blends), before the EQ.
                        CueMode::Raw => (frame[0], frame[1]),
                        // Post-EQ, pre gain/mute/crossfader.
                        CueMode::Pfl => (toned[0], toned[1]),
                        // Post everything, pre master.
                        CueMode::PostFader => (deck_out[i].0, deck_out[i].1),
                    };
                    cue.0 += cue_left;
                    cue.1 += cue_right;
                }
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
                            push_event(shared, MixEvent::VoiceEnded(v.id));
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

            // The pre-listen player: cue bus only, never the program. With
            // no phones armed it neither sounds nor advances — a frozen
            // position is the honest one for an inaudible player.
            if cue_armed {
                let p = &mut s.preview;
                if let Some(pcm) = &p.pcm {
                    let gain = p.gain.tick(rate);
                    // A paused player keeps emitting its frozen frame while
                    // the gain ramp swallows the stop.
                    if (p.playing || gain > 0.0005) && !pcm.frames.is_empty() {
                        let end = (pcm.frames.len() as u64) << 32;
                        if p.cursor_fp >= end {
                            if p.playing {
                                p.playing = false;
                                p.ended = true;
                                p.gain.slew(0.0, SLEW_SECS);
                            }
                        } else {
                            let index = (p.cursor_fp >> 32) as usize;
                            let fraction =
                                (p.cursor_fp & (FP_ONE - 1)) as f32 / FP_ONE as f32;
                            let next = (index + 1).min(pcm.frames.len() - 1);
                            let a = pcm.frames[index];
                            let b = pcm.frames[next];
                            cue.0 += (a[0] as f32 + (b[0] as f32 - a[0] as f32) * fraction)
                                / 32768.0
                                * gain;
                            cue.1 += (a[1] as f32 + (b[1] as f32 - a[1] as f32) * fraction)
                                / 32768.0
                                * gain;
                            if p.playing {
                                let step = ((pcm.sample_rate as f64 / device_rate)
                                    * FP_ONE as f64) as u64;
                                p.cursor_fp = p.cursor_fp.saturating_add(step.max(1));
                            }
                        }
                    }
                }
                // `audible` before the limiter, because a non-finite
                // sample would poison its peak read and duck the bus for
                // good -- the same guard the master's own limiter takes.
                s.cue_limiter.set_sample_rate(rate as f32);
                let cued = s.cue_limiter.process([audible(cue.0), audible(cue.1)]);
                shared.cue_ring.push(cue_pos, cued[0], cued[1]);
                cue_pos = cue_pos.saturating_add(1);
            }

            let score = s.score_preview.scratch.get(frame).copied().unwrap_or([0.0; 2]);
            // Each source's own chain, on the source alone and before it
            // enters its strip. The pads' chain hears the pads and not the
            // score preview: an audition should not come through whatever
            // the operator has put on the pads, so the score joins after.
            let sfx_strip = s.chain_mut(ChainTarget::Sfx).process([sfx.0, sfx.1], rate);
            let piano = s.synth.frame(SynthTrack::Piano, frame);
            let piano = s.chain_mut(ChainTarget::Piano).process(piano, rate);
            let ironfish = s.synth.frame(SynthTrack::Ironfish, frame);
            let ironfish = s.chain_mut(ChainTarget::Ironfish).process(ironfish, rate);
            let drums = s.synth.frame(SynthTrack::Drums, frame);
            let drums = s.chain_mut(ChainTarget::Drums).process(drums, rate);
            let master = s.master.tick(rate);
            let mixed = s.program_mix.process_frame_with(
                [
                    [video.0, video.1],
                    [deck_out[0].0, deck_out[0].1],
                    [deck_out[1].0, deck_out[1].1],
                    [sfx_strip[0] + score[0], sfx_strip[1] + score[1]],
                    piano,
                    ironfish,
                    drums,
                ],
                master,
                rate,
                |summed| s.master_chain.process(summed, rate),
            );
            let l = mixed[0].clamp(-CLAMP, CLAMP);
            let r = mixed[1].clamp(-CLAMP, CLAMP);
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
            // Decided and bound together, for the same reason as the
            // start above.
            let completing = s.scheduled_video.filter(|scheduled| {
                scheduled.started
                    && s.rendered_frames
                        >= scheduled.target_frame.saturating_add(scheduled.fade_frames.max(1))
            });
            if let Some(scheduled) = completing {
                s.scheduled_video = None;
                if let Some(from) = scheduled.from {
                    s.video[from.index()].gain = Ramp::at(0.0);
                }
                s.video[scheduled.to.index()].gain = Ramp::at(1.0);
                shared.transition
                    .publish_phase(VideoTransitionPhase::Completed, s.rendered_frames);
            }
        }

        // One cue publish per buffer: the phones consumer sees whole
        // buffers or nothing.
        if cue_armed {
            shared.cue_ring.write_pos.store(cue_pos, Ordering::Release);
        }


        // Reap: consumed slot frames + fully faded stopped voices. A
        // reaped voice's buffer goes back to the UI to be dropped.
        for (i, bus) in s.video.iter_mut().enumerate() {
            let consumed = (bus.cursor as usize).min(slots[i].avail);
            if consumed > 0 {
                shared.video[i]
                    .read_pos
                    .store(slots[i].base + consumed as u64, Ordering::Release);
                bus.cursor -= consumed as f64;
            }
        }
        let mut index = 0;
        while index < s.sfx.len() {
            let v = &s.sfx[index];
            let ran_off = v.cursor_fp >= (v.pcm.frames.len() as u64) << 32 && !v.loop_on;
            let faded_out = v.done && v.gain.current <= 0.0005 && v.gain.target == 0.0;
            if ran_off || faded_out {
                let voice = s.sfx.swap_remove(index);
                retire(shared, Retired::Track(voice.pcm));
            } else {
                index += 1;
            }
        }

        // The HIGHEST peak since the meter was last read, not the newest
        // one. Buffers land around a hundred times a second and the UI reads
        // twenty, so a plain store threw four peaks in five away unseen --
        // which is the whole quantity a peak meter exists to show. The
        // transient that a meter should catch is exactly the one that lands
        // between two glances.
        //
        // `fetch_max` on the bit pattern is a real max because these are
        // peaks: non-negative, and non-negative floats sort in the same
        // order as their bit patterns. `audible` is what makes that true --
        // an infinity here would out-max everything for the rest of the
        // session, where under a store it lasted one buffer.
        for (i, p) in peaks.iter().enumerate() {
            shared.meters[i].fetch_max(audible(*p).to_bits(), Ordering::Relaxed);
        }
        for (i, p) in deck_peaks.iter().enumerate() {
            shared.deck_meters[i].fetch_max(audible(*p).to_bits(), Ordering::Relaxed);
        }
        // How hard the master was leaned on. The master CANNOT clip -- the
        // ceiling is a guarantee -- so the honest warning is not "it
        // clipped" but "it had to be held back", and this is the only place
        // that says so.
        shared.limiter_reduction.fetch_max(
            audible(s.program_mix.master_snapshot().limiter_reduction_db.max(0.0)).to_bits(),
            Ordering::Relaxed,
        );
        if peaks[METER_MASTER] > f32::EPSILON
            && shared
                .first_non_silent
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            crate::log!(
                "audio: mixer first non-silent buffer sample_rate={} frames={} channels={}",
                device_rate,
                frames,
                channels
            );
        }
        let mix_nanos = mix_started.elapsed().as_nanos() as u64;
        shared
            .transition
            .publish_rendered_frame(shared.device_frames.load(Ordering::Acquire));
        Self::publish_snapshot(s, shared, self.serial);
        let render_nanos = render_started.elapsed().as_nanos() as u64;
        // Three clock reads a buffer and not three per sample: the split
        // says whether the cost is the mixing or the per-buffer overhead
        // around it, which is the question a climbing budget raises.
        let setup_nanos = mix_started.duration_since(render_started).as_nanos() as u64;
        shared.stage_nanos.publish(StageNanos {
            setup: setup_nanos,
            mix: mix_nanos,
            publish: render_nanos.saturating_sub(setup_nanos).saturating_sub(mix_nanos),
        });
        shared.render_nanos.store(render_nanos, Ordering::Relaxed);
        shared.buffer_frames.store(frames as u64, Ordering::Relaxed);
        shared.render_max_nanos.fetch_max(render_nanos, Ordering::Relaxed);
        if overran(render_nanos, frames, device_rate) {
            shared.overrun_callbacks.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Whether a render that took `render_nanos` outran the buffer it was
/// filling: longer than the buffer's own playing time at the device's
/// rate. Pure, so the boundary the console's dropout figure counts on
/// can be pinned.
fn overran(render_nanos: u64, frames: usize, device_rate: f64) -> bool {
    let budget_nanos = (frames as f64 / device_rate * 1e9) as u64;
    render_nanos > budget_nanos
}

/// Fixtures the audio tests share: tracks with a known shape and one
/// device callback at a time.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub(crate) fn const_pcm(value: i16, frames: usize, rate: u32) -> Arc<TrackPcm> {
        Arc::new(TrackPcm { frames: vec![[value, value]; frames], sample_rate: rate })
    }

    /// A constant, but stereo: left and right held at their own separate
    /// levels for the whole clip. `const_pcm`'s left and right are
    /// identical, so side (L-R) is exactly zero throughout -- fine for a
    /// mono-summing effect's click test, but it would make a stereo-width
    /// click test vacuous: the left channel it measures never moves no
    /// matter what `width` does, since mid+side*width collapses to the
    /// same constant when side is already zero.
    pub(crate) fn const_stereo_pcm(left: i16, right: i16, frames: usize, rate: u32) -> Arc<TrackPcm> {
        Arc::new(TrackPcm { frames: vec![[left, right]; frames], sample_rate: rate })
    }

    /// First half `a`, second half `b`: a signal a raw splice cannot hide
    /// in, for testing that jumps land as blends.
    pub(crate) fn split_pcm(a: i16, b: i16, frames: usize, rate: u32) -> Arc<TrackPcm> {
        let half = frames / 2;
        let mut all = vec![[a, a]; frames];
        for frame in all.iter_mut().skip(half) {
            *frame = [b, b];
        }
        Arc::new(TrackPcm { frames: all, sample_rate: rate })
    }

    /// Silent, except one full-scale frame at `at_secs`: a mark to time a
    /// delay against.
    pub(crate) fn click_pcm(at_secs: f64, rate: u32, seconds: f64) -> Arc<TrackPcm> {
        let len = (rate as f64 * seconds) as usize;
        let at = (at_secs * rate as f64).round() as usize;
        let mut all = vec![[0i16, 0i16]; len];
        if at < len {
            all[at] = [i16::MAX, i16::MAX];
        }
        Arc::new(TrackPcm { frames: all, sample_rate: rate })
    }

    /// One device callback. A test thread is not an audio thread: the
    /// callback arms flush-to-zero on whoever calls it, and a test that
    /// runs next on this thread must not inherit that (it has its own
    /// proof, `every_callback_arms_flush_to_zero`).
    pub(crate) fn render(engine: &mut MixEngine, rate: f64, frames: usize) -> AudioBuffer {
        let mut buffer = AudioBuffer::new_with_size(frames, 2);
        engine.render(rate, &mut buffer);
        crate::music_dsp::set_flush_denormals(false);
        buffer
    }

    /// The biggest jump between neighbouring samples in a rendered block.
    ///
    /// A click IS a step: the ear hears the discontinuity, not the level. Any
    /// gain, band or lane move performed on an audible strip has to glide,
    /// and this is how a test says so in one number. Measure it over a flat
    /// signal and whatever comes back belongs to the move under test.
    pub(crate) fn worst_adjacent_step(samples: &[f32]) -> f32 {
        samples
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max)
    }

    /// A tone at `frequency`, as a deck would hold it.
    pub(crate) fn tone_pcm(frequency: f64, rate: u32, seconds: f64) -> Arc<TrackPcm> {
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

    /// A genuinely stereo tone: independent left and right frequencies,
    /// so mid and side are both nonzero throughout. `tone_pcm`'s L and R
    /// are identical, which makes side (L-R) exactly zero and would make
    /// a stereo-width test vacuous regardless of what the effect does --
    /// the same "nice value hides the bug" trap the bitcrusher's click
    /// test found in a mono constant.
    pub(crate) fn stereo_tone_pcm(
        freq_l: f64,
        freq_r: f64,
        rate: u32,
        seconds: f64,
    ) -> Arc<TrackPcm> {
        let len = (rate as f64 * seconds) as usize;
        let frames = (0..len)
            .map(|index| {
                let t = index as f64 / rate as f64;
                let l = (2.0 * std::f64::consts::PI * freq_l * t).sin();
                let r = (2.0 * std::f64::consts::PI * freq_r * t).sin();
                [(l * 12_000.0) as i16, (r * 12_000.0) as i16]
            })
            .collect();
        Arc::new(TrackPcm { frames, sample_rate: rate })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{RefCell, RefMut};
    use super::fixtures::worst_adjacent_step;

    /// Handle and engine together, the way the app has them on two
    /// threads, here on one: commands go in through the handle, the
    /// engine applies them on `render`/`sync`, and every read below
    /// syncs first so a test sees what the callback would see.
    struct TestMixer {
        mixer: Mixer,
        engine: RefCell<MixEngine>,
    }

    impl std::ops::Deref for TestMixer {
        type Target = Mixer;
        fn deref(&self) -> &Mixer {
            &self.mixer
        }
    }

    impl TestMixer {
        fn new() -> TestMixer {
            let mixer = Mixer::new();
            let engine = mixer.take_engine().expect("fresh engine");
            TestMixer { mixer, engine: RefCell::new(engine) }
        }

        fn sync(&self) {
            self.engine.borrow_mut().sync();
        }

        /// The audio-owned state, commands applied.
        fn state(&self) -> RefMut<'_, MixState> {
            let mut engine = self.engine.borrow_mut();
            engine.sync();
            RefMut::map(engine, |engine| engine.state_mut())
        }

        fn render(&self, rate: f64, output: &mut AudioBuffer) {
            self.engine.borrow_mut().render(rate, output);
        }

        fn deck_position(&self, deck: DeckId) -> (f64, f64, bool) {
            self.sync();
            self.mixer.deck_position(deck)
        }

        fn deck_snapshot(&self, deck: DeckId) -> DeckSnapshot {
            self.sync();
            self.mixer.deck_snapshot(deck)
        }

        fn deck_scratching(&self, deck: DeckId) -> bool {
            self.sync();
            self.mixer.deck_scratching(deck)
        }

        fn crossfader_position(&self) -> f32 {
            self.sync();
            self.mixer.crossfader_position()
        }

        fn preview_position(&self) -> Option<(f64, f64, bool, bool)> {
            self.sync();
            self.mixer.preview_position()
        }

        fn score_preview_state(&self) -> (bool, u64) {
            self.sync();
            self.mixer.score_preview_state()
        }

        fn drain_ended_decks(&self) -> Vec<DeckId> {
            self.sync();
            self.mixer.drain_ended_decks()
        }

        fn drain_ended_voices(&self) -> Vec<VoiceId> {
            self.sync();
            self.mixer.drain_ended_voices()
        }

        fn drain_retired(&self) -> Vec<Retired> {
            self.sync();
            self.mixer.drain_retired()
        }

        fn video_transition_snapshot(&self) -> Option<VideoTransitionSnapshot> {
            self.sync();
            self.mixer.video_transition_snapshot()
        }
    }

    fn local_drum_bank() -> Option<Arc<SampleBank>> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/score-corpus/drums/OH");
        if !dir.is_dir() {
            eprintln!("skipping score preview drum test: {} is absent", dir.display());
            return None;
        }
        Some(Arc::new(SampleBank::load(&dir).expect("load local Salamander corpus")))
    }

    fn const_pcm(value: i16, frames: usize, rate: u32) -> Arc<TrackPcm> {
        Arc::new(TrackPcm { frames: vec![[value, value]; frames], sample_rate: rate })
    }

    /// A constant with DIFFERENT left and right, for anything that needs
    /// a side signal: `const_pcm` leaves both channels equal, which has no
    /// side at all and makes a width test vacuous.
    fn const_stereo_pcm(left: i16, right: i16, frames: usize, rate: u32) -> Arc<TrackPcm> {
        Arc::new(TrackPcm { frames: vec![[left, right]; frames], sample_rate: rate })
    }

    /// First half `a`, second half `b`: a signal a raw splice cannot hide
    /// in, for testing that jumps land as blends.
    fn split_pcm(a: i16, b: i16, frames: usize, rate: u32) -> Arc<TrackPcm> {
        let half = frames / 2;
        let mut all = vec![[a, a]; frames];
        for frame in all.iter_mut().skip(half) {
            *frame = [b, b];
        }
        Arc::new(TrackPcm { frames: all, sample_rate: rate })
    }

    fn render(mixer: &TestMixer, rate: f64, frames: usize) -> AudioBuffer {
        let mut buffer = AudioBuffer::new_with_size(frames, 2);
        mixer.render(rate, &mut buffer);
        buffer
    }

    /// Render away the master bus's own latency, so what comes back next
    /// is the audio for what just happened rather than the tail of what
    /// happened before it. The limiter looks ahead, and looking ahead is
    /// a delay.
    fn flush_bus(mixer: &TestMixer, rate: f64) {
        let latency = mixer.output_latency_frames();
        if latency > 0 {
            render(mixer, rate, latency);
        }
    }

    /// The rack renders every buffer and had never been told to play: no
    /// clock, `playing` never set, so the three synth strips were silence
    /// for the life of the app. This is the contract the SYNTH page's PLAY
    /// button relies on. No samples needed: the default patterns are a demo
    /// and the piano and the synth model their own sound.
    #[test]
    fn the_rack_is_silent_until_told_to_play() {
        let mixer = TestMixer::new();
        mixer.state().master = Ramp::at(1.0);
        mixer.set_synth_clock(crate::synth::SynthClock {
            beat_frame: 0,
            frames_per_beat: 24_000.0,
            beat_index: 0,
        });
        let quiet = render(&mixer, 48_000.0, 8_192);
        assert!(quiet.channel(0).iter().all(|s| *s == 0.0), "a rack nobody started is silent");
        assert!(!mixer.synth_snapshot().playing);

        mixer.set_synth_playing(true);
        let _ = render(&mixer, 48_000.0, 8_192);
        let loud = render(&mixer, 48_000.0, 8_192);
        assert!(loud.channel(0).iter().any(|s| s.abs() > 1e-4), "PLAY makes sound");
        assert!(mixer.synth_snapshot().playing);
    }

    /// The handle's setter is what remembers the bank; `ensure_synth_rate`
    /// rebuilds the rack from that memory. The app used to hand the bank
    /// over as a raw command, which reached the audio thread and left the
    /// memory empty -- so any device not at 48 kHz was rebuilt drumless.
    /// Skips, like its neighbours, when the local corpus is absent.
    #[test]
    fn the_bank_the_mixer_was_handed_is_the_bank_a_rate_change_rebuilds_with() {
        let Some(bank) = local_drum_bank() else { return };
        let mixer = TestMixer::new();
        // The door the app used to take: the audio thread gets the bank,
        // the memory does not.
        mixer.run_cmd(MixCmd::SetDrumBank(bank.clone()));
        assert!(!mixer.drum_bank_remembered(), "a raw command is not a memory");
        // The door it takes now.
        mixer.set_drum_bank(bank);
        assert!(mixer.drum_bank_remembered());
    }

    #[test]
    fn score_preview_enters_program_before_master_and_stops_at_end() {
        let Some(bank) = local_drum_bank() else { return };
        let mixer = TestMixer::new();
        mixer.run_cmd(MixCmd::SetDrumBank(bank));
        mixer.state().master = Ramp::at(1.0);
        let sequence = Arc::new(PreviewSequence {
            sample_rate: 48_000,
            events: vec![(
                0,
                PreviewEvent::Drum { voice: makepad_drumkit::DrumVoice::Kick, velocity: 1.0 },
            )],
            len_frames: 512,
            looped: false,
        });
        mixer.score_preview_play(sequence.clone());
        let first = render(&mixer, 48_000.0, 256);
        assert!(first.channel(0).iter().any(|sample| sample.abs() > 1.0e-5));
        assert_eq!(mixer.score_preview_state(), (true, 256));
        let _ = render(&mixer, 48_000.0, 256);
        assert_eq!(mixer.score_preview_state(), (false, 512));
        mixer.score_preview_stop();
        assert_eq!(mixer.score_preview_state(), (false, 0));

        mixer.state().master = Ramp::at(0.0);
        mixer.score_preview_play(sequence);
        let muted = render(&mixer, 48_000.0, 256);
        assert!(muted.channel(0).iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn score_preview_is_block_size_deterministic() {
        let Some(bank) = local_drum_bank() else { return };
        let run = |block: usize| {
            let mixer = TestMixer::new();
            mixer.run_cmd(MixCmd::SetDrumBank(bank.clone()));
            mixer.state().master = Ramp::at(1.0);
            mixer.score_preview_play(Arc::new(PreviewSequence {
                sample_rate: 48_000,
                events: vec![
                    (
                        0,
                        PreviewEvent::Drum {
                            voice: makepad_drumkit::DrumVoice::Kick,
                            velocity: 0.8,
                        },
                    ),
                    (
                        317,
                        PreviewEvent::Drum {
                            voice: makepad_drumkit::DrumVoice::HiHatClosed,
                            velocity: 0.6,
                        },
                    ),
                ],
                len_frames: 1_024,
                looped: false,
            }));
            let mut rendered = Vec::new();
            let mut left = 1_024;
            while left > 0 {
                let count = block.min(left);
                let out = render(&mixer, 48_000.0, count);
                rendered.extend(out.channel(0).iter().map(|sample| sample.to_bits()));
                left -= count;
            }
            rendered
        };
        assert_eq!(run(64), run(256));
    }

    #[test]
    fn score_preview_piano_receives_sample_timed_events() {
        let mixer = TestMixer::new();
        mixer.state().master = Ramp::at(1.0);
        mixer.score_preview_play(Arc::new(PreviewSequence {
            sample_rate: 48_000,
            events: vec![
                (0, PreviewEvent::Piano(PianoEvent::Sustain { value: 0.0 })),
                (17, PreviewEvent::Piano(PianoEvent::NoteOn { key: 60, velocity: 96 })),
                (1_024, PreviewEvent::Piano(PianoEvent::NoteOff { key: 60 })),
            ],
            len_frames: 2_048,
            looped: false,
        }));
        let out = render(&mixer, 48_000.0, 2_048);
        assert!(out.channel(0).iter().all(|sample| sample.is_finite()));
        assert!(out.channel(0).iter().any(|sample| sample.abs() > 1.0e-5));
        assert_eq!(mixer.score_preview_state(), (false, 2_048));
    }

    #[test]
    fn deck_under_equal_power_midpoint_is_root_half() {
        let mixer = TestMixer::new();
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
    fn deck_rms(mixer: &TestMixer, rate: f64, settle: usize, frames: usize) -> f64 {
        render(mixer, rate, settle);
        let out = render(mixer, rate, frames);
        let channel = out.channel(0);
        let sum: f64 = channel.iter().map(|v| (*v as f64) * (*v as f64)).sum();
        (sum / channel.len().max(1) as f64).sqrt()
    }

    fn decibels(ratio: f64) -> f64 {
        20.0 * ratio.max(1e-12).log10()
    }

    /// A separated stem peaks above full scale — the lane format has to
    /// carry that, because clipping it is what hardened the transients.
    #[test]
    fn stem_lanes_carry_peaks_above_full_scale() {
        // The decode side of `DeckSource::frame`, at unity gain.
        let played = |value: f32| {
            encode_stem_sample(value) as f32 / 32768.0 * STEM_CHUNK_HEADROOM
        };
        // The two peaks this bug was found on: the reference vocals stem
        // and a measured drums stem.
        for peak in [1.12f32, 1.47] {
            let out = played(peak);
            assert!(
                (out - peak).abs() < 0.001,
                "a stem peaking at {peak} must survive the lane format: {out}"
            );
        }
        // Ordinary audio is unharmed, and the format still clamps — just at
        // the headroom instead of at full scale.
        assert!((played(0.5) - 0.5).abs() < 0.001);
        assert!((played(-0.5) + 0.5).abs() < 0.001);
        assert!((played(9.0) - STEM_CHUNK_HEADROOM).abs() < 0.001);
    }

    /// The "fade to A/B" buttons hand the mixer a duration; the fader must
    /// take that long to cross, not jump and land.
    #[test]
    fn a_timed_crossfade_takes_its_duration() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, tone_pcm(440.0, 48_000, 10.0));
        mixer.install_deck(DeckId::B, tone_pcm(440.0, 48_000, 10.0));
        mixer.set_deck_playing(DeckId::A, true);
        mixer.set_deck_playing(DeckId::B, true);
        // Let the initial jump to 0.0 settle before the timed move starts.
        render(&mixer, 48_000.0, 4_096);
        assert!(mixer.state().fader.current < 1e-6);

        mixer.fade_crossfader(1.0, 4.0);
        // A quarter of the way through a four-second fade.
        render(&mixer, 48_000.0, 48_000);
        let quarter = mixer.state().fader.current;
        assert!(
            quarter > 0.2 && quarter < 0.3,
            "one second into a 4s fade the fader should be near 0.25: {quarter}"
        );
        // And it must actually arrive by the end.
        render(&mixer, 48_000.0, 48_000 * 4);
        let done = mixer.state().fader.current;
        assert!((done - 1.0).abs() < 1e-6, "the fade must land on B: {done}");
    }

    // A deck's tone chain has to be in the audible path, not just in the
    // UI: these render real buffers through the real mixer.

    #[test]
    fn a_killed_band_is_removed_from_the_deck_output() {
        let rate = 48_000.0;
        let measure = |band: usize, frequency: f64, kill: bool| -> f64 {
            let mixer = TestMixer::new();
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

    /// The master cannot clip -- the ceiling clamps every sample below full
    /// scale -- so a clip light on it would be a light that never comes on.
    /// What the operator needs to know is that the mix is being HELD DOWN,
    /// and this is the only number that says so.
    #[test]
    fn the_master_reports_how_hard_the_limiter_had_to_work() {
        let mixer = TestMixer::new();
        mixer.set_master(1.2);
        mixer.set_crossfader(0.0);
        let frames = vec![[i16::MAX, i16::MAX]; 48_000 / 5];
        mixer.install_deck(
            DeckId::A,
            Arc::new(TrackPcm { frames, sample_rate: 48_000 }),
        );
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4_096);
        flush_bus(&mixer, 48_000.0);
        let _ = mixer.limiter_reduction_db();
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }

        let held = mixer.limiter_reduction_db();
        assert!(held > 0.5, "full scale into a 1.2 master must be held back: {held}");
        assert!(held.is_finite() && held < 60.0, "and by a believable amount: {held}");
        // Reading takes it, so the next look is about the next stretch of
        // time. A reading that never cleared would latch the warning on the
        // first loud moment of the night and stay there.
        mixer.set_deck_playing(DeckId::A, false);
        // Long enough for the stop ramp AND the limiter's own release --
        // the reading covers every buffer since the last read, so draining
        // before the tail has gone would only measure the tail.
        for _ in 0..200 {
            render(&mixer, 48_000.0, 512);
        }
        let _ = mixer.limiter_reduction_db();
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }
        let quiet = mixer.limiter_reduction_db();
        assert!(quiet < 0.01, "silence is not being held back: {quiet}");
    }

    /// And a mix that stays inside the ceiling is never reported as held
    /// back, or the warning means nothing.
    #[test]
    fn an_ordinary_level_is_not_reported_as_held_back() {
        let mixer = TestMixer::new();
        mixer.set_master(0.5);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, tone_pcm(1_000.0, 48_000, 0.5));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4_096);
        flush_bus(&mixer, 48_000.0);
        let _ = mixer.limiter_reduction_db();
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }
        let held = mixer.limiter_reduction_db();
        assert!(
            held < crate::console::OVERLOAD_DB,
            "a mix inside the ceiling is not an overload: {held}"
        );
    }

    /// A peak meter exists to catch the loudest thing that happened. Buffers
    /// land around a hundred times a second and the meter is read twenty
    /// times, so most of them are never looked at -- and a meter that shows
    /// only whichever buffer happened to be last shows the transient exactly
    /// when it lands in the fifth buffer, which is one glance in five.
    #[test]
    fn the_loudest_thing_since_the_last_look_is_what_a_meter_reports() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, tone_pcm(1_000.0, 48_000, 1.0));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4_096);
        flush_bus(&mixer, 48_000.0);
        let _ = mixer.meters();

        // The loud stretch, and then quiet -- several buffers of it, so a
        // meter reading only the newest one would have forgotten.
        render(&mixer, 48_000.0, 512);
        mixer.set_deck_gain(DeckId::A, 0.0);
        flush_bus(&mixer, 48_000.0);
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }

        let loud = mixer.meters()[METER_MASTER];
        assert!(loud > 0.1, "the loud buffers must still be reported: {loud}");
        // And taking it is what empties it: the next look is about the next
        // stretch of time, not the same peak again.
        let after = mixer.meters()[METER_MASTER];
        assert!(after < loud, "a peak read twice is a peak that never falls: {after}");
    }

    /// The running maximum is taken on the BIT PATTERN, which is only a
    /// real maximum while the values are non-negative numbers. A sign or a
    /// non-number slipping through would not merely misread once -- it
    /// would win every comparison until the meter was next read.
    #[test]
    fn a_meter_reads_inside_its_own_scale_however_hard_it_is_driven() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        // Every sample hard against both ends of the scale.
        let frames = vec![[i16::MAX, i16::MIN]; 48_000 / 5];
        mixer.install_deck(
            DeckId::A,
            Arc::new(TrackPcm { frames, sample_rate: 48_000 }),
        );
        mixer.set_deck_playing(DeckId::A, true);
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }
        let peak = mixer.meters()[METER_MASTER];
        assert!(peak.is_finite(), "a meter reading has to be a number: {peak}");
        assert!(peak > 0.5, "and it has to have noticed: {peak}");
        assert!(peak <= 1.0, "and stay inside the scale it is drawn on: {peak}");
    }

    #[test]
    fn an_untouched_deck_plays_the_decoded_samples_unchanged() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        let pcm = tone_pcm(1_000.0, 48_000, 1.0);
        mixer.install_deck(DeckId::A, pcm.clone());
        mixer.set_deck_playing(DeckId::A, true);
        // Settle the master/fader ramps before comparing.
        render(&mixer, 48_000.0, 4_096);
        let start = {
            let state = mixer.state();
            state.decks[0].pos as usize
        };
        // The master bus runs a look-ahead behind the mix, so the sample
        // that leaves at index N went in that many frames earlier.
        let latency = mixer.output_latency_frames();
        let out = render(&mixer, 48_000.0, 256 + latency);
        for index in 0..200 {
            let want = pcm.frames[start + index][0] as f32 / 32768.0;
            let got = out.channel(0)[index + latency];
            assert!(
                (got - want).abs() < 1e-6,
                "sample {index}: {got} vs {want} — an untouched deck must be transparent"
            );
        }
    }

    /// Fundamental of a rendered channel, by zero crossings. Good enough to
    /// tell one semitone from the next, which is all these tests ask.
    fn measured_hz(channel: &[f32], rate: f64) -> f64 {
        let mut crossings = 0usize;
        for index in 1..channel.len() {
            if channel[index - 1] <= 0.0 && channel[index] > 0.0 {
                crossings += 1;
            }
        }
        crossings as f64 * rate / channel.len() as f64
    }

    #[test]
    fn key_shift_changes_pitch_without_changing_tempo() {
        let rate = 48_000.0;
        // An octave up at the track's own tempo: the tone doubles, the
        // playhead keeps real time.
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, tone_pcm(500.0, 48_000, 10.0));
        mixer.set_deck_keylock(DeckId::A, true);
        mixer.set_deck_key_shift(DeckId::A, 12.0);
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, rate, 48_000);
        let out = render(&mixer, rate, 48_000);
        let measured = measured_hz(out.channel(0), rate);
        assert!(
            (measured - 1_000.0).abs() < 20.0,
            "an octave up should sound at 1000 Hz, got {measured:.1} Hz"
        );
        let (position, _duration, _playing) = mixer.deck_position(DeckId::A);
        assert!(
            (position - 2.0).abs() < 0.2,
            "a key shift must not move the tempo: two seconds should be ~2.0 s \
             of source, got {position:.3}"
        );
    }

    #[test]
    fn key_shift_composes_with_tempo_under_keylock() {
        let rate = 48_000.0;
        // Both faders at once: 8% fast AND an octave up. The tempo is the
        // slider's, the pitch is the shift's, and neither leaks into the
        // other.
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, tone_pcm(500.0, 48_000, 10.0));
        mixer.set_deck_keylock(DeckId::A, true);
        mixer.set_deck_rate(DeckId::A, 1.08);
        mixer.set_deck_key_shift(DeckId::A, 12.0);
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, rate, 48_000);
        let out = render(&mixer, rate, 48_000);
        let measured = measured_hz(out.channel(0), rate);
        assert!(
            (measured - 1_000.0).abs() < 20.0,
            "key lock should keep the shift at 1000 Hz whatever the tempo, \
             got {measured:.1} Hz"
        );
        let (position, _duration, _playing) = mixer.deck_position(DeckId::A);
        assert!(
            (position - 2.0 * 1.08).abs() < 0.2,
            "two seconds at 1.08x should be ~2.16 s of source, got {position:.3}"
        );
    }

    #[test]
    fn key_shift_rides_on_varispeed_when_keylock_is_off() {
        let rate = 48_000.0;
        // Key lock off is a turntable: the 8% already raised the pitch, and
        // the shift stacks an octave on top of THAT — 500 × 1.08 × 2.
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, tone_pcm(500.0, 48_000, 10.0));
        mixer.set_deck_keylock(DeckId::A, false);
        mixer.set_deck_rate(DeckId::A, 1.08);
        mixer.set_deck_key_shift(DeckId::A, 12.0);
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, rate, 48_000);
        let out = render(&mixer, rate, 48_000);
        let measured = measured_hz(out.channel(0), rate);
        assert!(
            (measured - 1_080.0).abs() < 25.0,
            "an unlocked deck should sound at 1080 Hz, got {measured:.1} Hz"
        );
        let (position, _duration, _playing) = mixer.deck_position(DeckId::A);
        assert!(
            (position - 2.0 * 1.08).abs() < 0.2,
            "two seconds at 1.08x should be ~2.16 s of source, got {position:.3}"
        );
    }

    #[test]
    fn key_lock_changes_the_tempo_without_moving_the_pitch() {
        let rate = 48_000.0;
        // Count zero crossings of a 500 Hz tone played 8% fast with key
        // lock on: the frequency must not move with the tempo.
        let mixer = TestMixer::new();
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
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, tone_pcm(440.0, 48_000, 10.0));
        // Deliberately NOT playing: a hand on the record still moves it.
        // The finger says where it is and how fast it is going; the record
        // follows the place, not the speed.
        mixer.scratch_deck(DeckId::A, ScratchMotion::Grab);
        for step in 1..=24 {
            mixer.scratch_deck(
                DeckId::A,
                ScratchMotion::Move { secs: step as f64 * 2.0 / 24.0, rate: 2.0 },
            );
            render(&mixer, 48_000.0, 1_000);
        }
        let (scrubbed, _, playing) = mixer.deck_position(DeckId::A);
        assert!(!playing, "scrubbing is not playing");
        assert!(scrubbed > 0.5, "the hand moved the record: {scrubbed:.3} s");
        assert!(mixer.deck_scratching(DeckId::A));

        // Backwards, too.
        for step in 1..=12 {
            mixer.scratch_deck(
                DeckId::A,
                ScratchMotion::Move { secs: scrubbed - step as f64 * 3.0 / 12.0, rate: -3.0 },
            );
            render(&mixer, 48_000.0, 1_000);
        }
        let (back, _, _) = mixer.deck_position(DeckId::A);
        assert!(back < scrubbed, "a backward scrub must rewind: {back:.3}");

        // Letting go of a paused deck stops it dead. The hand-off grows
        // with the momentum it was let go at, so give it its longest.
        mixer.scratch_deck(DeckId::A, ScratchMotion::Release);
        render(&mixer, 48_000.0, 48_000 * 2);
        assert!(!mixer.deck_scratching(DeckId::A), "the ramp must finish");
        let (settled, _, _) = mixer.deck_position(DeckId::A);
        render(&mixer, 48_000.0, 24_000);
        let (after, _, _) = mixer.deck_position(DeckId::A);
        assert!(
            (after - settled).abs() < 1e-6,
            "a released, paused deck must sit still: {settled:.3} -> {after:.3}"
        );
    }

    /// Full-scale PCM re-encoded into the lane format, the way every real
    /// producer does it — so a fixture measures what playback measures.
    fn stem_block(frames: &[[i16; 2]]) -> Arc<Vec<[i16; 2]>> {
        Arc::new(
            frames
                .iter()
                .map(|f| {
                    [
                        encode_stem_sample(f[0] as f32 / 32768.0),
                        encode_stem_sample(f[1] as f32 / 32768.0),
                    ]
                })
                .collect(),
        )
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
                stems.lanes[lane][index] = Some(stem_block(&pcm.frames[start..end]));
            }
        }
        Arc::new(stems)
    }

    /// A constant-valued streamed chunk of `frames` frames.
    fn stream_chunk(value: i16, frames: usize) -> Arc<Vec<[i16; 2]>> {
        Arc::new(vec![[value, value]; frames])
    }

    /// The deck's playhead in frames, straight from the state.
    fn deck_pos(mixer: &TestMixer, deck: DeckId) -> f64 {
        mixer.state().decks[deck.index()].playhead_frames()
    }

    /// The master chain sits in the path of every frame of the mix, so
    /// the one thing that has to be true before any of it is worth having
    /// is that it costs the signal nothing until an effect is switched on.
    #[test]
    fn an_idle_master_chain_leaves_the_mix_alone() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        let pcm = tone_pcm(1_000.0, 48_000, 1.0);
        mixer.install_deck(DeckId::A, pcm.clone());
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4_096);
        assert!(!mixer.state().chain_engaged(ChainTarget::Master));
        let start = deck_pos(&mixer, DeckId::A) as usize;
        let latency = mixer.output_latency_frames();
        let out = render(&mixer, 48_000.0, 256 + latency);
        for index in 0..200 {
            let want = pcm.frames[start + index][0] as f32 / 32768.0;
            let got = out.channel(0)[index + latency];
            assert!(
                (got - want).abs() < 1e-6,
                "sample {index}: {got} vs {want} — an idle master chain must be transparent"
            );
        }
    }

    /// The stem index is the one raw number that reaches the audio thread
    /// as an array index rather than a typed enum. The handle's setters
    /// check it; nothing on the audio side did, so a command pushed raw --
    /// which is what every test does -- could take the callback down.
    /// Refused there now, and the last lane is not a bin for bad numbers.
    #[test]
    fn a_stem_index_out_of_range_is_refused_rather_than_bent_onto_another() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, const_pcm(4_000, 48_000 * 2, 48_000));
        let last = STEM_COUNT - 1;
        let settled = |mixer: &TestMixer| {
            let _ = render(mixer, 48_000.0, 48_000);
            mixer.state().decks[0].stem_gain[last].current()
        };
        let before = settled(&mixer);
        mixer.run_cmd(MixCmd::SetStemGain { deck: DeckId::A, stem: STEM_COUNT, gain: 0.25 });
        mixer.run_cmd(MixCmd::SetStemGain { deck: DeckId::A, stem: 999, gain: 0.25 });
        mixer.run_cmd(MixCmd::SetBlendStem { deck: DeckId::A, stem: STEM_COUNT, gain: 0.25 });
        assert_eq!(settled(&mixer), before, "the last lane is not a bin for bad numbers");
        // Refused, not broken: a lane that exists still answers.
        mixer.run_cmd(MixCmd::SetStemGain { deck: DeckId::A, stem: last, gain: 0.25 });
        assert!((settled(&mixer) - 0.25).abs() < 1e-3);
    }

    /// And an engaged one reaches the sum. The width sits at 1.5 by
    /// default, which widens anything that is not already mono, so an
    /// out-of-phase pair is the cheapest thing to hear it on.
    #[test]
    fn an_engaged_master_effect_reaches_the_mix() {
        let quiet = |on: bool| -> f64 {
            let mixer = TestMixer::new();
            mixer.set_master(1.0);
            mixer.set_crossfader(0.0);
            // Genuinely out of phase, so there IS a side signal for
            // the width to widen. `split_pcm` splits over time and
            // leaves both channels equal, which has no side at all.
            mixer.install_deck(DeckId::A, const_stereo_pcm(12_000, -12_000, 96_000, 48_000));
            mixer.set_deck_playing(DeckId::A, true);
            if on {
                mixer.set_chain_effect(ChainTarget::Master, EffectParam::StereoWidth(true));
            }
            render(&mixer, 48_000.0, 8_192);
            let out = render(&mixer, 48_000.0, 2_048);
            out.channel(0).iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>()
        };
        let off = quiet(false);
        let on = quiet(true);
        assert!(on > off * 1.2, "the master width did not reach the mix: {off} then {on}");
    }

    /// A pad voice through the pads' own chain, with nothing on it: the
    /// chain costs the signal nothing until an effect is switched on. The
    /// same contract the master's chain keeps, on the first of the five
    /// sources that got a chain of their own.
    #[test]
    fn a_source_chain_is_transparent_until_asked() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.start_voice(
            VoiceAlloc {
                id: 1,
                pad: PadKey::from_bytes([1; 16]),
                choke_group: 0,
                loop_on: true,
                gain: 1.0,
                started_ms: 0,
            },
            const_pcm(16_384, 48_000 * 2, 48_000), // 0.5 amp
        );
        render(&mixer, 48_000.0, 4_096);
        assert!(!mixer.state().chain_engaged(ChainTarget::Sfx));
        let latency = mixer.output_latency_frames();
        let out = render(&mixer, 48_000.0, 256 + latency);
        for index in 0..200 {
            let got = out.channel(0)[index + latency];
            assert!(
                (got - 0.5).abs() < 1e-5,
                "sample {index}: {got} -- an idle source chain must be transparent"
            );
        }
    }

    /// An effect on one source reaches the mix, and an effect on a source
    /// that is silent changes nothing: the decks' samples do not move when
    /// the pads' chain is switched on.
    #[test]
    fn an_engaged_source_chain_reaches_the_mix_and_touches_nothing_else() {
        let energy = |width_on_pads: bool, with_pads: bool| -> (f64, Vec<u32>) {
            let mixer = TestMixer::new();
            mixer.set_master(1.0);
            mixer.set_crossfader(0.0);
            if with_pads {
                // Out of phase, so there is a side signal to widen.
                mixer.start_voice(
                    VoiceAlloc {
                        id: 2,
                        pad: PadKey::from_bytes([2; 16]),
                        choke_group: 0,
                        loop_on: true,
                        gain: 1.0,
                        started_ms: 0,
                    },
                    const_stereo_pcm(12_000, -12_000, 96_000, 48_000),
                );
            } else {
                mixer.install_deck(DeckId::A, tone_pcm(1_000.0, 48_000, 2.0));
                mixer.set_deck_playing(DeckId::A, true);
            }
            if width_on_pads {
                mixer.set_chain_effect(ChainTarget::Sfx, EffectParam::StereoWidth(true));
            }
            render(&mixer, 48_000.0, 8_192);
            let out = render(&mixer, 48_000.0, 2_048);
            let bits = out.channel(0).iter().map(|s| s.to_bits()).collect();
            (out.channel(0).iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>(), bits)
        };
        let (off, _) = energy(false, true);
        let (on, _) = energy(true, true);
        assert!(on > off * 1.2, "the pads' width did not reach the mix: {off} then {on}");
        let (_, deck_alone) = energy(false, false);
        let (_, deck_with_pad_width) = energy(true, false);
        assert_eq!(deck_alone, deck_with_pad_width, "an effect on the pads moved the deck");
    }

    /// Eight targets, eight chains: a knob on one lands on that one only.
    #[test]
    fn every_target_has_a_chain_of_its_own() {
        for target in ChainTarget::ALL {
            let mixer = TestMixer::new();
            mixer.set_chain_effect(target, EffectParam::Flanger(true));
            for other in ChainTarget::ALL {
                assert_eq!(
                    mixer.state().chain_engaged(other),
                    other == target,
                    "{target:?} on: {other:?} engaged",
                );
            }
        }
    }

    /// A number read from a file becomes a target, or nothing -- never the
    /// nearest one -- and the words the file holds are all different.
    #[test]
    fn a_target_index_from_a_file_falls_back_rather_than_bending_onto_another() {
        for target in ChainTarget::ALL {
            assert_eq!(ChainTarget::from_index(target.index()), Some(target));
        }
        assert_eq!(ChainTarget::from_index(ChainTarget::ALL.len()), None);
        assert_eq!(ChainTarget::from_index(usize::MAX), None);
        let tags: Vec<&str> = ChainTarget::ALL.iter().map(|t| t.tag()).collect();
        for (i, tag) in tags.iter().enumerate() {
            assert!(!tags[..i].contains(tag), "two targets share the word {tag}");
        }
        assert_eq!(ChainTarget::from(DeckId::A), ChainTarget::DeckA);
        assert_eq!(ChainTarget::from(DeckId::B).deck(), Some(DeckId::B));
        assert_eq!(ChainTarget::Master.strip(), None);
    }

    #[test]
    fn a_streaming_deck_waits_at_the_decoded_edge_and_carries_on() {
        let rate = 48_000u32;
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        // One chunk in, three expected.
        let table = StreamPcm::new(rate, Some(STREAM_CHUNK_FRAMES * 3));
        let table = Arc::new(table.with_chunk(stream_chunk(8_000, STREAM_CHUNK_FRAMES), false));
        mixer.install_deck_stream(DeckId::A, table.clone());
        assert!(mixer.deck_is_streaming(DeckId::A));
        let snapshot = mixer.deck_snapshot(DeckId::A);
        assert!((snapshot.duration_secs - 3.0 * STREAM_CHUNK_FRAMES as f64 / rate as f64).abs() < 1e-9);
        mixer.set_deck_playing(DeckId::A, true);

        // Play through the chunk: audible, then silent AT the edge, still
        // playing, the playhead parked there rather than ended.
        let audible = deck_rms(&mixer, rate as f64, 256, 4_096);
        assert!(audible > 0.1, "the decoded lead plays: {audible}");
        let _ = render(&mixer, rate as f64, STREAM_CHUNK_FRAMES);
        let parked = render(&mixer, rate as f64, 4_096);
        assert!(parked.channel(0).iter().all(|v| *v == 0.0), "past the edge is silence");
        assert_eq!(deck_pos(&mixer, DeckId::A), STREAM_CHUNK_FRAMES as f64);
        assert!(mixer.deck_snapshot(DeckId::A).playing, "waiting is not ended");
        assert!(mixer.drain_ended_decks().is_empty());

        // The next chunk lands: playback resumes from the edge, no seek.
        let table = Arc::new(table.with_chunk(stream_chunk(8_000, STREAM_CHUNK_FRAMES), false));
        mixer.grow_deck_stream(DeckId::A, table.clone());
        flush_bus(&mixer, rate as f64);
        let resumed = render(&mixer, rate as f64, 4_096);
        assert!(resumed.channel(0).iter().skip(64).all(|v| v.abs() > 0.1), "resumes on arrival");
        assert!(deck_pos(&mixer, DeckId::A) > STREAM_CHUNK_FRAMES as f64 + 4_000.0);

        // The end: the last (short) chunk, then the deck really ends.
        let table = Arc::new(table.with_chunk(stream_chunk(8_000, 1_000), true));
        mixer.grow_deck_stream(DeckId::A, table);
        let _ = render(&mixer, rate as f64, STREAM_CHUNK_FRAMES + 2_000);
        assert!(!mixer.deck_snapshot(DeckId::A).playing);
        assert_eq!(mixer.drain_ended_decks(), vec![DeckId::A]);
        // Play from the end restarts, now that the end is the end.
        mixer.set_deck_playing(DeckId::A, true);
        assert_eq!(deck_pos(&mixer, DeckId::A), 0.0);
    }

    #[test]
    fn a_seek_past_the_decoded_edge_parks_there_and_plays_when_it_can() {
        let rate = 48_000u32;
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        let table = StreamPcm::new(rate, Some(STREAM_CHUNK_FRAMES * 4));
        let table = Arc::new(table.with_chunk(stream_chunk(8_000, STREAM_CHUNK_FRAMES), false));
        mixer.install_deck_stream(DeckId::A, table.clone());
        mixer.set_deck_playing(DeckId::A, true);
        // 90% of the EXPECTED track is far past the one chunk in hand.
        mixer.seek_deck_fraction(DeckId::A, 0.9);
        assert_eq!(deck_pos(&mixer, DeckId::A), STREAM_CHUNK_FRAMES as f64);
        let parked = render(&mixer, rate as f64, 2_048);
        assert!(parked.channel(0).iter().all(|v| *v == 0.0));
        assert!(mixer.deck_snapshot(DeckId::A).playing);
        // A seek in seconds past the edge parks the same way.
        mixer.seek_deck_seconds(DeckId::A, 100.0);
        assert_eq!(deck_pos(&mixer, DeckId::A), STREAM_CHUNK_FRAMES as f64);
        // ...and a seek inside the decoded region plays at once.
        mixer.seek_deck_seconds(DeckId::A, 0.5);
        let level = deck_rms(&mixer, rate as f64, 256, 2_048);
        assert!(level > 0.1, "{level}");
        // The chunk arrives: from the edge the deck plays on.
        mixer.seek_deck_fraction(DeckId::A, 0.9);
        let table = Arc::new(table.with_chunk(stream_chunk(8_000, STREAM_CHUNK_FRAMES), false));
        mixer.grow_deck_stream(DeckId::A, table);
        let level = deck_rms(&mixer, rate as f64, 256, 2_048);
        assert!(level > 0.1, "{level}");
    }

    #[test]
    fn the_whole_file_takes_over_from_the_stream_at_the_playhead() {
        let rate = 48_000u32;
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        let whole = tone_pcm(440.0, rate, 6.0);
        // The stream is the first chunk of the very same samples.
        let table = StreamPcm::new(rate, Some(whole.frames.len()));
        let first = Arc::new(whole.frames[..STREAM_CHUNK_FRAMES].to_vec());
        let table = Arc::new(table.with_chunk(first, false));
        mixer.install_deck_stream(DeckId::A, table);
        mixer.set_deck_playing(DeckId::A, true);
        let _ = render(&mixer, rate as f64, 10_000);
        let before = deck_pos(&mixer, DeckId::A);
        mixer.complete_deck(DeckId::A, whole.clone());
        assert!(!mixer.deck_is_streaming(DeckId::A));
        assert_eq!(deck_pos(&mixer, DeckId::A), before, "the swap moves nothing");
        // The bus is a look-ahead behind: let what was already in it out
        // first, so what follows is what the whole file holds from `before`.
        let _ = render(&mixer, rate as f64, mixer.output_latency_frames());
        // What comes out after the swap is what the whole file holds there.
        let out = render(&mixer, rate as f64, 512);
        for (n, sample) in out.channel(0).iter().enumerate() {
            let want = whole.frames[before as usize + n][0] as f32 / 32768.0;
            assert!((sample - want).abs() < 1e-3, "frame {n}: {sample} vs {want}");
        }
        // The duration reads the exact length now.
        assert!((mixer.deck_snapshot(DeckId::A).duration_secs - 6.0).abs() < 1e-9);
    }

    #[test]
    fn stems_swap_in_sample_aligned_at_the_playhead() {
        let rate = 48_000u32;
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        // The mixed file numbers its frames; the stems put the SAME numbers
        // in one lane, chunked the way the separator does.
        let total = rate as usize * 4;
        let frames: Vec<[i16; 2]> = (0..total).map(|i| [(i % 20_000) as i16; 2]).collect();
        let pcm = Arc::new(TrackPcm { frames: frames.clone(), sample_rate: rate });
        let chunk = rate as usize;
        let mut stems = TrackStems::new(chunk, total.div_ceil(chunk));
        for index in 0..total.div_ceil(chunk) {
            let start = index * chunk;
            let end = (start + chunk).min(total);
            stems.lanes[0][index] = Some(stem_block(&frames[start..end]));
            for lane in 1..STEM_COUNT {
                stems.lanes[lane][index] = Some(stem_block(&vec![[0, 0]; end - start]));
            }
        }
        mixer.install_deck(DeckId::A, pcm);
        mixer.set_deck_playing(DeckId::A, true);
        let _ = render(&mixer, rate as f64, 7_777);
        let at = deck_pos(&mixer, DeckId::A) as usize;
        mixer.install_deck_stems(DeckId::A, Arc::new(stems));
        // The bus hands every frame over one look-ahead late, so the
        // swap's first frame comes out that many frames in.
        let latency = mixer.output_latency_frames();
        let out = render(&mixer, rate as f64, 4_096 + latency);
        // Frame n after the swap is source frame at+n, read from the lane:
        // the stem sum is the mix, so the blend hides nothing and the lane
        // format's one bit of headroom is the only difference allowed.
        for (n, sample) in out.channel(0).iter().skip(latency).enumerate() {
            let want = frames[at + n][0] as f32 / 32768.0;
            assert!((sample - want).abs() <= 2.5 / 32768.0, "frame {n}: {sample} vs {want}");
        }
    }

    #[test]
    fn a_stem_swap_under_a_turned_knob_is_a_blend_not_a_step() {
        let rate = 48_000u32;
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, const_pcm(8_000, rate as usize * 4, rate));
        // Vocals killed before the stems exist: the swap will actually
        // change the sound (the whole signal sits in the vocals lane).
        mixer.set_deck_stem_gain(DeckId::A, 0, 0.0);
        mixer.set_deck_playing(DeckId::A, true);
        let _ = render(&mixer, rate as f64, 4_096);
        let mut stems = TrackStems::new(rate as usize, 4);
        for index in 0..4 {
            stems.lanes[0][index] = Some(stem_block(&vec![[8_000, 8_000]; rate as usize]));
            for lane in 1..STEM_COUNT {
                stems.lanes[lane][index] = Some(stem_block(&vec![[0, 0]; rate as usize]));
            }
        }
        mixer.install_deck_stems(DeckId::A, Arc::new(stems));
        let out = render(&mixer, rate as f64, 4_096);
        let left = out.channel(0);
        let level = 8_000.0 / 32768.0;
        // The first frame is still (nearly) the mixed file; well past the
        // blend the vocals-only stems are silent; in between it ramps.
        assert!(left[0] > level * 0.9, "starts on the mix: {}", left[0]);
        assert!(left[4_000].abs() < 1e-4, "ends on the stems: {}", left[4_000]);
        let mid = left[(STEM_SWAP_SECS * rate as f32 * 0.5) as usize];
        assert!(mid > level * 0.25 && mid < level * 0.75, "halfway is a blend: {mid}");
        for pair in left.windows(2) {
            assert!((pair[1] - pair[0]).abs() < level * 0.05, "no step: {:?}", pair);
        }
    }

    #[test]
    fn stem_lanes_mix_under_their_gains() {
        let mixer = TestMixer::new();
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
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        // Audible mixed file, and stems that only cover the second half.
        mixer.install_deck(DeckId::A, tone_pcm(440.0, 48_000, 4.0));
        let mut stems = TrackStems::new(48_000, 4);
        let separated = tone_pcm(440.0, 48_000, 4.0);
        for index in 2..4 {
            let start = index * 48_000;
            for lane in 0..STEM_COUNT {
                stems.lanes[lane][index] =
                    Some(stem_block(&separated.frames[start..start + 48_000]));
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
    fn deck_end_reports_once_and_a_span_never_ends() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, const_pcm(1000, 100, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 256);
        assert_eq!(mixer.drain_ended_decks(), vec![DeckId::A]);
        assert!(mixer.drain_ended_decks().is_empty());
        let (_pos, _dur, playing) = mixer.deck_position(DeckId::A);
        assert!(!playing, "with no span, running off the end stops the deck");
        // A deck inside a span never reaches an end to report. The mixer
        // honours any span; LOOP_MIN_SECS is enforced up in `decks`.
        mixer.install_deck(DeckId::B, const_pcm(1000, 100, 48_000));
        mixer.set_deck_loop_span(DeckId::B, Some((0.0, 100.0 / 48_000.0)), crate::decks::LoopSeek::None);
        mixer.set_deck_playing(DeckId::B, true);
        render(&mixer, 48_000.0, 1024);
        assert!(mixer.drain_ended_decks().is_empty());
        let (_, _, playing) = mixer.deck_position(DeckId::B);
        assert!(playing);
    }

    #[test]
    fn a_looping_deck_never_runs_past_its_out_point() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000)); // 10 s
        mixer.set_crossfader(0.0);
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 2.0)), crate::decks::LoopSeek::None);
        mixer.seek_deck_seconds(DeckId::A, 1.0);
        mixer.set_deck_playing(DeckId::A, true);
        // Four seconds of audio through a one-second loop.
        for _ in 0..46 {
            render(&mixer, 48_000.0, 4096);
            let (position, _, _) = mixer.deck_position(DeckId::A);
            assert!(
                (1.0..2.0).contains(&position),
                "the playhead escaped the span at {position}"
            );
        }
    }

    #[test]
    fn a_span_set_mid_play_wraps_too() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_crossfader(0.0);
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 48_000); // a second of free play first
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 2.0)), crate::decks::LoopSeek::None);
        for _ in 0..46 {
            render(&mixer, 48_000.0, 4096);
            let (position, _, _) = mixer.deck_position(DeckId::A);
            assert!((1.0..2.0).contains(&position), "escaped at {position}");
        }
    }

    #[test]
    fn the_wrap_is_gapless_on_sustained_material() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_crossfader(0.0);
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 2.0)), crate::decks::LoopSeek::None);
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4096); // settle the master and gain ramps
        let steady = render(&mixer, 48_000.0, 64).channel(0)[32].abs();
        // Land just before OUT so one buffer straddles the wrap.
        mixer.seek_deck_seconds(DeckId::A, 2.0 - 0.006);
        let out = render(&mixer, 48_000.0, 1024);
        let quietest = (0..1024).map(|i| out.channel(0)[i].abs()).fold(f32::MAX, f32::min);
        let mut worst_step = 0.0f32;
        for i in 1..1024 {
            worst_step = worst_step.max((out.channel(0)[i] - out.channel(0)[i - 1]).abs());
        }
        // The requirement, verbatim: it has to sound like the music just
        // keeps playing. On sustained material the wrap must neither dip
        // (the old duck was an audible 2 Hz gate on a pad) nor step (a
        // click). A crossfade of two equal sustains is that sustain.
        assert!(
            quietest > steady * 0.9,
            "the wrap must not duck the programme: {quietest} vs steady {steady}"
        );
        assert!(worst_step < 0.02, "and it must not click: step {worst_step}");
    }

    #[test]
    fn a_deck_behind_its_loop_is_audible_on_the_way_in() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_crossfader(0.0);
        mixer.set_deck_loop_span(DeckId::A, Some((5.0, 6.0)), crate::decks::LoopSeek::None);
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4096); // settle ramps
        // The patient rule: a playhead behind IN plays at FULL level until
        // the loop catches it. The historical defect muted the whole run-up.
        mixer.seek_deck_seconds(DeckId::A, 1.0);
        render(&mixer, 48_000.0, 256);
        let out = render(&mixer, 48_000.0, 64);
        assert!(
            out.channel(0)[32].abs() > 0.2,
            "the run-up to a loop must be audible, got {}",
            out.channel(0)[32]
        );
    }

    #[test]
    fn a_shrunk_span_catches_the_playhead_modulo_not_at_in() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_crossfader(0.0);
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 5.0)), crate::decks::LoopSeek::None);
        mixer.seek_deck_seconds(DeckId::A, 3.5);
        mixer.set_deck_playing(DeckId::A, true);
        // Halve out from under the playhead: 3.5 is 2.5 into the old span,
        // which is 0.5 into the new one modulo its length — the subdivision
        // continues instead of re-triggering the downbeat at IN.
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 2.0)), crate::decks::LoopSeek::None);
        render(&mixer, 48_000.0, 256);
        let (position, _, _) = mixer.deck_position(DeckId::A);
        assert!(
            (1.45..1.65).contains(&position),
            "the playhead must keep its phase modulo the new span, got {position}"
        );
    }

    #[test]
    fn a_long_held_loop_does_not_drift_against_its_own_length() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        // 44.1k material on a 48k device: the natural step is fractional,
        // so a wrap that discards its overshoot loses ~half a source frame
        // per lap and a held loop walks audibly early over minutes.
        mixer.install_deck(DeckId::A, const_pcm(16_384, 441_000, 44_100));
        mixer.set_crossfader(0.0);
        // A length deliberately NOT commensurate with the 44.1k -> 48k step:
        // a round 0.1 s is exactly 4800 device frames and wraps with zero
        // overshoot, which would hide the discard this test exists to catch.
        mixer.set_deck_loop_span(DeckId::A, Some((0.5, 0.60001)), crate::decks::LoopSeek::None);
        mixer.seek_deck_seconds(DeckId::A, 0.5);
        mixer.set_deck_playing(DeckId::A, true);
        let step = 44_100.0 / 48_000.0;
        let buffers = 240usize; // ~200 laps of the loop
        for _ in 0..buffers {
            render(&mixer, 48_000.0, 4096);
        }
        let advanced = buffers as f64 * 4096.0 * step; // source frames
        let expected = 0.5 + (advanced % (0.10001 * 44_100.0)) / 44_100.0;
        let (position, _, _) = mixer.deck_position(DeckId::A);
        let error = (position - expected).abs();
        assert!(
            error < 0.001,
            "the wrap must keep its overshoot: {error:.4}s off after ~200 laps"
        );
    }

    #[test]
    fn crossing_into_a_loop_is_click_free() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_crossfader(0.0);
        mixer.set_deck_loop_span(DeckId::A, Some((5.0, 6.0)), crate::decks::LoopSeek::None);
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4096); // settle ramps
        // Straddle the IN crossing: the run-up must hand over to the seam
        // duck CONTINUOUSLY. A step lands a click on the very gesture the
        // patient rule exists for.
        mixer.seek_deck_seconds(DeckId::A, 5.0 - 512.0 / 48_000.0);
        let out = render(&mixer, 48_000.0, 1024);
        let mut worst = 0.0f32;
        for i in 1..1024 {
            worst = worst.max((out.channel(0)[i] - out.channel(0)[i - 1]).abs());
        }
        assert!(
            worst < 0.02,
            "crossing IN must be continuous, biggest adjacent step {worst}"
        );
    }

    #[test]
    fn a_span_ending_at_the_exact_track_end_never_ends_the_deck() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        // 48_003 frames: a length whose seconds->frames round trip lands a
        // hair ABOVE the frame count, so an unclamped OUT sits past the
        // last frame and the end-of-track check wins over the wrap.
        mixer.install_deck(DeckId::A, const_pcm(16_384, 48_003, 48_000));
        mixer.set_crossfader(0.0);
        mixer.set_deck_loop_span(DeckId::A, Some((0.0, 48_003.0 / 48_000.0)), crate::decks::LoopSeek::None);
        mixer.set_deck_playing(DeckId::A, true);
        for _ in 0..24 {
            render(&mixer, 48_000.0, 4096);
        }
        assert!(mixer.drain_ended_decks().is_empty(), "a span must never end the deck");
        let (position, duration, playing) = mixer.deck_position(DeckId::A);
        assert!(playing, "the loop must still be running");
        assert!(position < duration, "and inside the track, got {position}");
    }

    #[test]
    fn a_keylocked_deck_wraps_a_span_at_the_track_end() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000)); // 10 s
        mixer.set_crossfader(0.0);
        // Keylock is the voice default; a non-unity rate engages the
        // stretcher, whose read head cannot reach the last WSOLA window of
        // the track — so a span whose OUT hugs the end can never see
        // playhead >= end and used to die through the ran-out path.
        mixer.set_deck_rate(DeckId::A, 1.05);
        mixer.set_deck_loop_span(DeckId::A, Some((9.0, 10.0)), crate::decks::LoopSeek::None);
        mixer.seek_deck_seconds(DeckId::A, 9.0);
        mixer.set_deck_playing(DeckId::A, true);
        for _ in 0..24 {
            render(&mixer, 48_000.0, 4096);
        }
        assert!(mixer.drain_ended_decks().is_empty(), "the stretch path must wrap too");
        let (position, _, playing) = mixer.deck_position(DeckId::A);
        assert!(playing);
        assert!((9.0..10.0).contains(&position), "still looping, got {position}");
    }

    #[test]
    fn a_resize_on_a_paused_deck_lands_the_playhead_at_once() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 5.0)), crate::decks::LoopSeek::None);
        mixer.seek_deck_seconds(DeckId::A, 3.5);
        // Paused: the render loop skips this deck entirely, so the catch
        // has to happen when the span is SET or the playhead sits parked
        // outside the loop until play is pressed.
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 2.0)), crate::decks::LoopSeek::None);
        let (position, _, _) = mixer.deck_position(DeckId::A);
        assert!(
            (1.49..1.51).contains(&position),
            "a stranded paused playhead lands modulo at set time, got {position}"
        );
    }

    #[test]
    fn a_commanded_jump_is_a_blend_not_a_splice() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        // +0.5 for the first five seconds, −0.5 after: jumping across the
        // middle is a full-scale discontinuity unless something blends it.
        mixer.install_deck(DeckId::A, split_pcm(16_384, -16_384, 480_000, 48_000));
        mixer.set_crossfader(0.0);
        mixer.seek_deck_seconds(DeckId::A, 1.0);
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4096); // settle ramps
        // The splice falls BETWEEN two callbacks — the seek happens between
        // renders — so the scan has to bridge the buffer boundary or a raw
        // cut is invisible to it.
        let before = render(&mixer, 48_000.0, 64);
        let tail = before.channel(0)[63];
        mixer.seek_deck_seconds(DeckId::A, 7.0);
        let out = render(&mixer, 48_000.0, 1024);
        let mut worst = (out.channel(0)[0] - tail).abs();
        for i in 1..1024 {
            worst = worst.max((out.channel(0)[i] - out.channel(0)[i - 1]).abs());
        }
        assert!(worst < 0.05, "a jump must land as a blend, biggest step {worst}");
        // And it really did jump: the deck is playing the second half.
        let (position, _, _) = mixer.deck_position(DeckId::A);
        assert!(position >= 7.0, "landed at {position}");
    }

    #[test]
    fn a_span_follows_its_voice_through_a_swap() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 2.0)), crate::decks::LoopSeek::None);
        mixer.seek_deck_seconds(DeckId::A, 1.0);
        mixer.set_deck_playing(DeckId::A, true);
        mixer.swap_decks();
        mixer.set_crossfader(1.0);
        for _ in 0..24 {
            render(&mixer, 48_000.0, 4096);
            let (position, _, _) = mixer.deck_position(DeckId::B);
            assert!((1.0..2.0).contains(&position), "span lost in the swap at {position}");
        }
    }

    #[test]
    fn sfx_voices_overlap_and_finished_voices_are_reaped() {
        let mixer = TestMixer::new();
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
        flush_bus(&mixer, 48_000.0);
        let out = render(&mixer, 48_000.0, 64);
        // Three overlapping voices sum: 3 × 0.25 × master(1.0).
        assert!((out.channel(0)[32] - 0.75).abs() < 0.02, "{}", out.channel(0)[32]);
        // Run to the end: all three report ended and are reaped.
        render(&mixer, 48_000.0, 4_800);
        let mut ended = mixer.drain_ended_voices();
        ended.sort();
        assert_eq!(ended, vec![1, 2, 3]);
        flush_bus(&mixer, 48_000.0);
        let out = render(&mixer, 48_000.0, 64);
        assert!(out.channel(0)[32].abs() < 1e-6);
    }

    #[test]
    fn video_slot_fade_reaches_targets_and_close_silences() {
        let mixer = TestMixer::new();
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
        flush_bus(&mixer, 48_000.0);
        let out = render(&mixer, 48_000.0, 64);
        assert!(out.channel(0)[32].abs() < 1e-6);
    }

    #[test]
    fn video_mute_roundtrip_restores_pre_mute_level_exactly() {
        let mixer = TestMixer::new();
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
        let mixer = TestMixer::new();
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
        let mixer = TestMixer::new();
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
        let mixer = TestMixer::new();
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
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        render(&mixer, 48_000.0, 2048); // settle master
        mixer.open_slot(SlotId::A);
        assert!(mixer.push_slot_audio(SlotId::A, &vec![16_384; 2 * 256], 2, 48_000));
        let target = mixer.rendered_output_frames() + 5;
        mixer
            .schedule_video_transition_at(41, None, SlotId::A, target, 1)
            .unwrap();

        // The transition is scheduled on an exact frame of the MIX; the
        // bus hands that frame over one look-ahead later, so the sample
        // it lands on moves with it and the exactness is unchanged.
        let out = render(&mixer, 48_000.0, 12);
        let latency = mixer.output_latency_frames();
        let out = if latency > 0 {
            let mut all = out.channel(0).to_vec();
            all.extend_from_slice(render(&mixer, 48_000.0, latency).channel(0));
            all.drain(..latency);
            all
        } else {
            out.channel(0).to_vec()
        };
        assert!(out[..5].iter().all(|sample| sample.abs() < 1e-7));
        assert!((out[5] - 0.5).abs() < 0.02, "transition was not sample exact");
        let snapshot = mixer.video_transition_snapshot().unwrap();
        assert_eq!(snapshot.id, 41);
        assert_eq!(snapshot.start_frame, Some(target));
        assert_eq!(snapshot.phase, VideoTransitionPhase::Completed);
        assert_eq!(snapshot.progress, 1.0);
    }

    #[test]
    fn armed_destination_queue_is_not_consumed_before_target() {
        let mixer = TestMixer::new();
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
        let mixer = TestMixer::new();
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
    fn closing_just_started_destination_restores_previous_program() {
        let mixer = TestMixer::new();
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
        fn populated() -> TestMixer {
            let mixer = TestMixer::new();
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
        let control = control.state();
        let scheduled = scheduled.state();
        assert_eq!(scheduled.decks[0].pos, control.decks[0].pos);
        assert_eq!(scheduled.sfx[0].cursor_fp, control.sfx[0].cursor_fp);
    }

    #[test]
    fn video_playback_rate_is_capped_and_isolated_from_other_voices() {
        let mixer = TestMixer::new();
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
        let state = mixer.state();
        assert_eq!(state.decks[0].pos, 100.0);
        let consumed = 4_000 - mixer.shared.video[0].buffered_frames() as usize;
        assert!((107..=108).contains(&consumed));
        assert!((consumed as f64 + state.video[0].cursor - 108.0).abs() < 1e-6);
    }
    #[test]
    fn the_blend_overlay_multiplies_and_clears_without_touching_the_knobs() {
        let mixer = TestMixer::new();
        // Operator sets a stem lane to 0.8; the autopilot blends it to 0.5.
        mixer.set_deck_stem_gain(DeckId::A, 2, 0.8);
        mixer.set_blend_stem(DeckId::A, 2, 0.5);
        {
            let s = mixer.state();
            let d = &s.decks[0];
            assert!((d.stem_gain[2].target() - 0.8).abs() < 1e-6, "the knob stands");
            assert!((d.blend_stem[2].target() - 0.5).abs() < 1e-6, "the hand is on");
        }
        // Clear returns the overlay to unity; the operator's value stands.
        mixer.clear_blend(DeckId::A);
        {
            let s = mixer.state();
            let d = &s.decks[0];
            assert!((d.blend_stem[2].target() - 1.0).abs() < 1e-6);
            assert!((d.stem_gain[2].target() - 0.8).abs() < 1e-6);
        }
        // A fresh install snaps the overlay home instantly.
        mixer.set_blend_stem(DeckId::A, 0, 0.0);
        mixer.install_deck(DeckId::A, const_pcm(0, 4800, 48_000));
        {
            let s = mixer.state();
            let d = &s.decks[0];
            assert!((d.blend_stem[0].target() - 1.0).abs() < 1e-6, "install lets go");
        }
    }

    // The headphone cue bus: rendered beside the program in `render`, carried
    // to the phones device through the lock-free ring, never through the
    // program sum. These drive the real render loop and the real ring.

    /// Drain the cue ring at `cue_rate` into one buffer, the way the
    /// phones-device callback does.
    fn consume_cue(
        mixer: &TestMixer,
        state: &mut CueReadState,
        cue_rate: f64,
        frames: usize,
    ) -> AudioBuffer {
        let mut buffer = AudioBuffer::new_with_size(frames, 2);
        mixer.cue_ring().consume(state, cue_rate, &mut buffer);
        buffer
    }

    #[test]
    fn a_control_at_its_stop_reaches_the_top_of_the_master() {
        // The defect this replaces: one path scaled a 0..1 control onto the
        // master's range and the other passed it through raw, so the same
        // fader at the same stop meant two different things depending on
        // which code claimed it — and the raw one could not reach the last
        // sixth of the range at all.
        assert_eq!(master_from_control(1.0), MAX_MASTER_GAIN, "a stop is the top");
        assert_eq!(master_from_control(0.0), 0.0);
        assert!(master_from_control(0.5) < MAX_MASTER_GAIN);
        // And the mixer's own clamp agrees with it, so a full-travel control
        // is never quietly trimmed on the way in.
        assert_eq!(
            knob(master_from_control(1.0), 0.0, MAX_MASTER_GAIN),
            Some(MAX_MASTER_GAIN),
        );
    }

    /// The top third of a channel fader used to move nothing the room could
    /// hear: the engine computed up to 1.5 and the mixer clamped it to 1.0.
    #[test]
    fn a_channel_fader_above_unity_is_actually_louder() {
        let level = |gain: f32| {
            let mixer = TestMixer::new();
            mixer.set_master(1.0);
            // A quiet source, so half again over unity is nowhere near the
            // limiter and the difference is the gain's alone.
            mixer.install_deck(DeckId::A, const_pcm(6_553, 48_000 * 4, 48_000)); // 0.2 amp
            mixer.set_deck_playing(DeckId::A, true);
            mixer.set_deck_gain(DeckId::A, gain);
            render(&mixer, 48_000.0, 8_192);
            render(&mixer, 48_000.0, 512).channel(0)[256].abs()
        };
        let unity = level(1.0);
        let over = level(crate::decks::MAX_DECK_GAIN);
        assert!(
            over > unity * 1.4,
            "the fader's top third moved nothing: unity {unity}, full {over}",
        );
    }

    /// The phones end on a limiter, not a clamp.
    ///
    /// A clamp is a clipper: pushed past full scale it flat-tops every
    /// sample that got there, which is a square wave's worth of harmonics
    /// and reads as grit. The master got a limiter for exactly that reason
    /// and the phones got one with it, in the same commit -- and then the
    /// engine swap put the clamp back on the phones alone, so the room and
    /// the headphones stopped agreeing about what two hot decks sound like.
    #[test]
    fn two_hot_decks_in_the_phones_are_limited_and_not_flat_topped() {
        // Near full scale, so cueing both puts about twice full scale into
        // a bus whose ceiling is one.
        let hot = |rate: u32, seconds: f64| {
            let len = (rate as f64 * seconds) as usize;
            let frames = (0..len)
                .map(|index| {
                    let phase = 2.0 * std::f64::consts::PI * 220.0 * index as f64 / rate as f64;
                    let sample = (phase.sin() * 32_000.0) as i16;
                    [sample, sample]
                })
                .collect();
            Arc::new(TrackPcm { frames, sample_rate: rate })
        };

        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_cue_armed(true);
        for deck in [DeckId::A, DeckId::B] {
            mixer.install_deck(deck, hot(48_000, 4.0));
            mixer.set_deck_playing(deck, true);
            mixer.set_deck_cue(deck, true);
        }
        render(&mixer, 48_000.0, 8_192);
        render(&mixer, 48_000.0, 8_192);

        let mut cue_state = CueReadState::default();
        let cue = consume_cue(&mixer, &mut cue_state, 48_000.0, 512);
        // Past the ring's prime and the limiter's attack, so what is left
        // is the steady state.
        let tail = &cue.channel(0)[128..];
        let pinned = tail.iter().filter(|s| s.abs() >= 0.999).count();
        let peak = tail.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));

        assert!(peak <= 1.0 + 1e-6, "the ceiling is a ceiling: peaked at {peak}");
        assert!(peak > 0.5, "the phones went quiet instead of being limited: {peak}");
        // A clamp pins about two thirds of a sine at twice full scale; a
        // limiter turns the gain down and the shape survives, so only the
        // very tips of the wave reach the ceiling at all.
        assert!(
            pinned * 20 < tail.len(),
            "{pinned} of {} samples flat-topped: that is a clipper, not a limiter",
            tail.len(),
        );
    }

    #[test]
    fn cue_pfl_ignores_gain_mute_and_crossfader() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 48_000 * 4, 48_000)); // 0.5 amp
        mixer.set_deck_playing(DeckId::A, true);
        mixer.set_deck_gain(DeckId::A, 0.2);
        mixer.set_deck_mute(DeckId::A, true);
        mixer.set_crossfader(1.0); // hard on B: deck A leaves the program
        mixer.set_cue_armed(true);
        mixer.set_deck_cue(DeckId::A, true);
        render(&mixer, 48_000.0, 8_192);
        let out = render(&mixer, 48_000.0, 512);
        assert!(
            out.channel(0)[256].abs() < 0.001,
            "a muted, faded-away deck must leave the program silent"
        );
        let mut cue_state = CueReadState::default();
        let cue = consume_cue(&mixer, &mut cue_state, 48_000.0, 512);
        let got = cue.channel(0)[256];
        assert!(
            (got - 0.5).abs() < 0.01,
            "PFL carries the full-level deck whatever the faders do: {got}"
        );
    }

    #[test]
    fn cue_postfader_follows_gain_and_crossfader() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 48_000 * 8, 48_000)); // 0.5 amp
        mixer.set_deck_playing(DeckId::A, true);
        mixer.set_deck_gain(DeckId::A, 0.5);
        mixer.set_crossfader(1.0); // away from A
        mixer.set_cue_armed(true);
        mixer.set_deck_cue(DeckId::A, true);
        mixer.set_cue_mode(CueMode::PostFader);
        render(&mixer, 48_000.0, 8_192);
        let mut cue_state = CueReadState::default();
        let faded = consume_cue(&mixer, &mut cue_state, 48_000.0, 512).channel(0)[256].abs();
        assert!(
            faded < 0.001,
            "post-fader cue of a faded-away deck is silent: {faded}"
        );
        mixer.set_crossfader(0.0); // hard on A
        render(&mixer, 48_000.0, 8_192);
        // A fresh consumer re-primes near the write head, like a device that
        // just opened.
        let mut cue_state = CueReadState::default();
        let heard = consume_cue(&mixer, &mut cue_state, 48_000.0, 512).channel(0)[256];
        assert!(
            (heard - 0.25).abs() < 0.01,
            "post-fader cue follows gain and fader (0.5 × 0.5): {heard}"
        );
    }

    #[test]
    fn cue_raw_bypasses_the_eq() {
        let rate = 48_000.0;
        let cue_rms = |mode: CueMode| -> f64 {
            let mixer = TestMixer::new();
            mixer.set_master(1.0);
            mixer.set_crossfader(0.0);
            mixer.install_deck(DeckId::A, tone_pcm(60.0, 48_000, 6.0));
            mixer.set_deck_playing(DeckId::A, true);
            mixer.set_deck_eq_band(DeckId::A, 0, 0.0); // bass kill
            mixer.set_cue_armed(true);
            mixer.set_deck_cue(DeckId::A, true);
            mixer.set_cue_mode(mode);
            render(&mixer, rate, 24_000);
            let mut state = CueReadState::default();
            let mut sum = 0.0f64;
            let mut count = 0usize;
            for _ in 0..16 {
                render(&mixer, rate, 1_024);
                let out = consume_cue(&mixer, &mut state, rate, 1_024);
                for v in out.channel(0) {
                    sum += (*v as f64) * (*v as f64);
                }
                count += 1_024;
            }
            (sum / count.max(1) as f64).sqrt()
        };
        let pfl = cue_rms(CueMode::Pfl);
        let raw = cue_rms(CueMode::Raw);
        assert!(raw > 0.2, "the raw tap must carry the unfiltered tone: {raw}");
        assert!(
            decibels(pfl / raw) < -30.0,
            "PFL hears the bass kill, RAW must not: {:.1} dB",
            decibels(pfl / raw)
        );
    }

    #[test]
    fn preview_plays_only_into_the_cue_and_seeks() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_cue_armed(true);
        mixer.install_preview(const_pcm(16_384, 48_000 * 4, 48_000), true); // 0.5 amp
        render(&mixer, 48_000.0, 8_192);
        let out = render(&mixer, 48_000.0, 512);
        assert!(
            out.channel(0)[256].abs() < 1e-6,
            "a preview must never reach the program"
        );
        let mut state = CueReadState::default();
        let cue = consume_cue(&mixer, &mut state, 48_000.0, 512);
        let got = cue.channel(0)[256];
        assert!((got - 0.5).abs() < 0.01, "the preview sounds in the cue: {got}");

        let (pos, dur, playing, ended) =
            mixer.preview_position().expect("a preview is installed");
        assert!(playing && !ended);
        assert!((dur - 4.0).abs() < 1e-6, "duration: {dur}");
        assert!(pos > 0.15 && pos < 0.25, "8.7k frames is ~0.18 s: {pos}");

        mixer.seek_preview_fraction(0.5);
        let (pos, ..) = mixer.preview_position().expect("still installed");
        assert!((pos - 2.0).abs() < 0.05, "seek lands mid-track: {pos}");

        mixer.seek_preview_fraction(0.999);
        render(&mixer, 48_000.0, 8_192);
        let (_, _, playing, ended) = mixer.preview_position().expect("still installed");
        assert!(!playing && ended, "running off the end parks the preview");

        mixer.clear_preview();
        assert!(
            mixer.drain_retired().iter().any(|retired| matches!(retired, Retired::Track(_))),
            "the pcm comes back out, to be dropped on the UI thread"
        );
        assert!(mixer.preview_position().is_none(), "cleared means gone");
    }

    #[test]
    fn cue_ring_servo_survives_mismatched_device_rates() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, tone_pcm(440.0, 48_000, 30.0));
        mixer.set_deck_playing(DeckId::A, true);
        mixer.set_cue_armed(true);
        mixer.set_deck_cue(DeckId::A, true);
        render(&mixer, 48_000.0, 8_192);
        let mut state = CueReadState::default();
        // One emulated second per lap: the producer clocks 48 000 frames,
        // the consumer 44 100, in interleaved device-sized chunks.
        let mut underruns = 0usize;
        let mut consumed_any = false;
        for _lap in 0..12 {
            for _step in 0..100 {
                render(&mixer, 48_000.0, 480);
                let out = consume_cue(&mixer, &mut state, 44_100.0, 441);
                let peak = out
                    .channel(0)
                    .iter()
                    .fold(0.0f32, |peak, v| peak.max(v.abs()));
                let silent = peak < 1e-6;
                if consumed_any && silent {
                    underruns += 1;
                }
                if !silent {
                    consumed_any = true;
                }
            }
        }
        assert!(consumed_any, "the consumer must leave priming");
        assert!(
            underruns == 0,
            "steady mismatched rates must never underrun: {underruns}"
        );
    }

    /// The fixture's grid: a 2 s bar, one one-bar cell per column, the
    /// first bar at `first_bar_secs`. `shift_secs` moves every cell's
    /// window (a refined grid that chose other bars) and columns from
    /// `keep_cols` on have no cell at all (a refined grid that lost one).
    fn splat_grid(first_bar_secs: f64, shift_secs: f64, keep_cols: usize) -> Arc<SplatGrid> {
        use crate::loop_splat::{SplatCell, SplatSection};

        let sections = (0..SPLAT_COLS)
            .map(|col| SplatSection {
                start_secs: col as f64 * 2.0,
                end_secs: (col + 1) as f64 * 2.0,
                bars: 1,
            })
            .collect();
        let mut cells = [[None; SPLAT_COLS]; SPLAT_ROWS];
        for row in SplatRow::ALL {
            for col in 0..keep_cols.min(SPLAT_COLS) {
                cells[row.index()][col] = Some(SplatCell {
                    span: crate::decks::LoopSpan {
                        start_secs: col as f64 * 2.0 + shift_secs,
                        end_secs: (col + 1) as f64 * 2.0 + shift_secs,
                    },
                    bars: 1,
                    energy: 1.0,
                    silent: false,
                });
            }
        }
        Arc::new(SplatGrid {
            bpm: 120.0,
            bar_secs: 2.0,
            first_bar_secs,
            sections,
            cells,
            bars_per_col: [1; SPLAT_COLS],
        })
    }

    fn splat_fixture(missing_drums: bool) -> (TestMixer, u32) {

        let rate = 1_000u32;
        let frame_count = rate as usize * 16;
        let pcm = const_pcm(12_000, frame_count, rate);
        let mut stems = TrackStems::new(frame_count, 1);
        for stem in 0..STEM_COUNT {
            if missing_drums && stem == crate::music_dsp::StemKind::Drums.index() {
                continue;
            }
            let samples = (0..frame_count)
                .map(|frame| {
                    let col = (frame / 2_000).min(7);
                    let amplitude = match stem {
                        1 => {
                            0.08
                                + col as f32 * 0.025
                                + (frame % 2_000) as f32 / 2_000.0 * 0.02
                        }
                        2 => 0.06,
                        0 => 0.04,
                        _ => 0.02,
                    };
                    let value = encode_stem_sample(amplitude);
                    [value, value]
                })
                .collect();
            stems.lanes[stem][0] = Some(Arc::new(samples));
        }
        let grid = splat_grid(0.0, 0.0, SPLAT_COLS);
        let mixer = TestMixer::new();
        mixer.state().master = Ramp::at(1.0);
        mixer.install_deck(DeckId::A, pcm);
        mixer.install_deck_stems(DeckId::A, Arc::new(stems));
        mixer.set_deck_splat(DeckId::A, grid);
        mixer.set_deck_splat_enabled(DeckId::A, true);
        mixer.set_deck_playing(DeckId::A, true);
        (mixer, rate)
    }

    fn render_count(mixer: &TestMixer, rate: u32, mut frames: usize, block: usize) -> Vec<f32> {
        let mut samples = Vec::with_capacity(frames);
        while frames > 0 {
            let count = frames.min(block);
            let output = render(mixer, rate as f64, count);
            samples.extend_from_slice(output.channel(0));
            frames -= count;
        }
        samples
    }

    /// The cell a row is sounding (or, before its bar, waiting to sound).
    fn row_slot(mixer: &TestMixer, row: SplatRow) -> Option<(f64, f64)> {
        let state = mixer.state();
        let splat = state.decks[0].splat.as_ref().unwrap();
        let voice = splat.rows[row.index()];
        voice
            .queued
            .and_then(|queued| queued.cell)
            .or(voice.cell)
            .map(|cell| (cell.start_frames, cell.len_frames))
    }

    /// What the mix row reads at master frame `at`, as a source frame
    /// index: the ramp fixture stores the index in every sample.
    fn cell_read(mixer: &TestMixer, at: f64) -> f64 {
        let state = mixer.state();
        let deck = &state.decks[0];
        let splat = deck.splat.as_ref().unwrap();
        let cell = splat.rows[SplatRow::Mix.index()].cell.expect("a sounding mix cell");
        let pcm = deck.pcm.as_ref().unwrap();
        let frame = splat_cell_frame(SplatRow::Mix, cell, at, pcm, None, [1.0; STEM_COUNT]);
        frame[0] as f64 * 32768.0
    }

    /// A freshly installed deck sits at zero until it plays or is seeked —
    /// whatever else lands on it while it is paused: a grid (whose first
    /// bar is past zero), the grid switched on and off, a loop span, a
    /// rate, stems, the stream growing and completing.
    #[test]
    fn install_never_moves_a_paused_deck() {
        let rate = 1_000u32;
        let frame_count = rate as usize * 16;
        let mixer = TestMixer::new();
        mixer.state().master = Ramp::at(1.0);
        let grid = splat_grid(0.5, 0.0, SPLAT_COLS);
        let whole = const_pcm(12_000, frame_count, rate);

        mixer.install_deck(DeckId::A, whole.clone());
        mixer.install_deck_stems(DeckId::A, Arc::new(TrackStems::new(frame_count, 1)));
        mixer.set_deck_splat(DeckId::A, grid.clone());
        mixer.set_deck_splat_enabled(DeckId::A, true);
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 3.0)), crate::decks::LoopSeek::None);
        mixer.set_deck_rate(DeckId::A, 1.05);
        mixer.set_deck_keylock(DeckId::A, true);
        render_count(&mixer, rate, 500, 64);
        let snapshot = mixer.deck_snapshot(DeckId::A);
        assert!(!snapshot.playing);
        assert_eq!(snapshot.position_secs, 0.0, "the grid on: {snapshot:?}");
        assert_eq!(deck_pos(&mixer, DeckId::A), 0.0);
        mixer.set_deck_splat_enabled(DeckId::A, false);
        render_count(&mixer, rate, 100, 64);
        assert_eq!(deck_pos(&mixer, DeckId::A), 0.0, "the grid off again");
        assert_eq!(mixer.deck_snapshot(DeckId::A).position_secs, 0.0);

        // The same through the chunk table of a track still decoding
        // (every chunk but the last is a full one).
        let long = STREAM_CHUNK_FRAMES + 4_000;
        let table = StreamPcm::new(rate, Some(long));
        let first = Arc::new(table.with_chunk(stream_chunk(7, STREAM_CHUNK_FRAMES), false));
        mixer.install_deck_stream(DeckId::B, first.clone());
        mixer.set_deck_splat(DeckId::B, grid);
        mixer.set_deck_splat_enabled(DeckId::B, true);
        mixer.set_deck_loop_span(DeckId::B, Some((0.0, 2.0)), crate::decks::LoopSeek::None);
        render_count(&mixer, rate, 100, 64);
        let grown = Arc::new(first.with_chunk(stream_chunk(7, 4_000), false));
        mixer.grow_deck_stream(DeckId::B, grown);
        render_count(&mixer, rate, 100, 64);
        mixer.complete_deck(DeckId::B, const_pcm(7, long, rate));
        render_count(&mixer, rate, 100, 64);
        let snapshot = mixer.deck_snapshot(DeckId::B);
        assert!(!snapshot.playing);
        assert_eq!(snapshot.position_secs, 0.0, "{snapshot:?}");
        assert_eq!(deck_pos(&mixer, DeckId::B), 0.0);
        // A relative landing never reaches a paused deck from the engine,
        // but the mixer honours one it is sent — that is a seek.
    }

    /// A click on a cell loops exactly that cell's bars: the boundaries are
    /// the grid's bar positions in source frames, the reads walk them
    /// sample by sample and wrap on the frame, the reported playhead cycles
    /// inside them, and a second click on the same cell — or the same
    /// track through the chunk table while it is still decoding — lands on
    /// the very same frames.
    #[test]
    fn cell_loop_boundaries_are_sample_exact_and_stable() {
        let rate = 1_000u32;
        let frame_count = rate as usize * 16;
        let ramp = Arc::new(TrackPcm {
            frames: (0..frame_count).map(|i| [i as i16, i as i16]).collect(),
            sample_rate: rate,
        });
        let grid = splat_grid(0.0, 0.0, SPLAT_COLS);
        for streamed in [false, true] {
            let mixer = TestMixer::new();
            mixer.state().master = Ramp::at(1.0);
            if streamed {
                // Half the track decoded: the grid owns time on a stream too.
                let table = StreamPcm::new(rate, Some(frame_count));
                let half = Arc::new(ramp.frames[..frame_count / 2].to_vec());
                mixer.install_deck_stream(DeckId::A, Arc::new(table.with_chunk(half, false)));
            } else {
                mixer.install_deck(DeckId::A, ramp.clone());
            }
            mixer.set_deck_splat(DeckId::A, grid.clone());
            mixer.set_deck_splat_enabled(DeckId::A, true);
            mixer.set_deck_playing(DeckId::A, true);
            render_count(&mixer, rate, 300, 64);

            mixer.splat_launch(DeckId::A, SplatRow::Mix, 1, SplatPart::WHOLE);
            render_count(&mixer, rate, 1, 1);
            let (cell, anchor) = {
                let state = mixer.state();
                let splat = state.decks[0].splat.as_ref().unwrap();
                let cell = splat.rows[SplatRow::Mix.index()].cell.expect("sounding at once");
                (cell, cell.anchor_frames)
            };
            assert_eq!((cell.start_frames, cell.len_frames), (2_000.0, 2_000.0), "streamed {streamed}");
            assert_eq!(anchor, 2_000.0, "the first launch re-seats the clock on the cell");
            for lap in 0..20 {
                let base = anchor + lap as f64 * cell.len_frames;
                assert_eq!(cell_read(&mixer, base), 2_000.0, "lap {lap} start");
                assert_eq!(cell_read(&mixer, base + 1.0), 2_001.0, "lap {lap} second frame");
                assert_eq!(cell_read(&mixer, base + 1_999.0), 3_999.0, "lap {lap} last frame");
            }
            // The rendered clock: one source frame per device frame here, so
            // after every whole lap the reported playhead is back on the
            // same frame, inside the cell, twenty laps running.
            for lap in 0..20 {
                render_count(&mixer, rate, 2_000, 256);
                assert_eq!(deck_pos(&mixer, DeckId::A), 2_001.0, "lap {lap}, streamed {streamed}");
                let secs = mixer.deck_snapshot(DeckId::A).position_secs;
                assert!((2.0..4.0).contains(&secs), "the header cycles inside the bar: {secs}");
            }
            // Stop, then the same click again: the very same frames.
            mixer.splat_stop_row(DeckId::A, SplatRow::Mix, false);
            render_count(&mixer, rate, 700, 64);
            assert_eq!(row_slot(&mixer, SplatRow::Mix), None);
            mixer.splat_launch(DeckId::A, SplatRow::Mix, 1, SplatPart::WHOLE);
            render_count(&mixer, rate, 1, 1);
            assert_eq!(row_slot(&mixer, SplatRow::Mix), Some((2_000.0, 2_000.0)));
            let again = mixer.state().decks[0].splat.as_ref().unwrap().rows[SplatRow::Mix.index()]
                .cell
                .unwrap();
            assert_eq!(cell_read(&mixer, again.anchor_frames), 2_000.0);
            assert_eq!(cell_read(&mixer, again.anchor_frames + 2_000.0), 2_000.0, "wraps on the frame");
        }
    }

    /// A refined grid landing under a running loop: a slot whose bars did
    /// not change is left alone; one whose bars moved re-launches on the
    /// new frames at the next bar (the picture following it); one the new
    /// grid no longer has stops there. The sound is always the grid shown.
    #[test]
    fn a_replaced_grid_relaunches_running_rows_on_its_own_frames() {
        let (mixer, rate) = splat_fixture(false);
        render_count(&mixer, rate, 300, 64);
        mixer.splat_launch(DeckId::A, SplatRow::Drums, 1, SplatPart::WHOLE);
        mixer.splat_launch(DeckId::A, SplatRow::Bass, 7, SplatPart::WHOLE);
        render_count(&mixer, rate, 500, 64);
        assert_eq!(row_slot(&mixer, SplatRow::Drums), Some((2_000.0, 2_000.0)));
        assert_eq!(row_slot(&mixer, SplatRow::Bass), Some((14_000.0, 2_000.0)));

        // The same bars again: nothing is re-launched.
        mixer.set_deck_splat(DeckId::A, splat_grid(0.0, 0.0, SPLAT_COLS));
        render_count(&mixer, rate, 1, 1);
        {
            let state = mixer.state();
            let splat = state.decks[0].splat.as_ref().unwrap();
            assert!(splat.rows[SplatRow::Drums.index()].queued.is_none(), "unchanged slot left alone");
            assert!(splat.rows[SplatRow::Bass.index()].queued.is_none());
        }

        // Every window a bar later, and column 7 gone.
        mixer.set_deck_splat(DeckId::A, splat_grid(0.0, 2.0, 7));
        render_count(&mixer, rate, 1, 1);
        {
            let state = mixer.state();
            let splat = state.decks[0].splat.as_ref().unwrap();
            let drums = splat.rows[SplatRow::Drums.index()];
            let queued = drums.queued.and_then(|queued| queued.cell).expect("drums re-launch queued");
            assert_eq!((queued.start_frames, queued.len_frames), (4_000.0, 2_000.0));
            assert_eq!(queued.anchor_frames, 4_000.0, "on the next bar");
            assert_eq!(drums.cell.map(|cell| cell.start_frames), Some(2_000.0), "still sounding the old bars until then");
            let bass = splat.rows[SplatRow::Bass.index()];
            assert!(matches!(bass.queued, Some(Queued { cell: None, .. })), "a lost slot stops: {bass:?}");
            // The picture stays on the bass (launched last) while it sounds.
            assert_eq!(splat.view.map(|cell| cell.start_frames), Some(14_000.0));
        }
        render_count(&mixer, rate, 2_100, 256);
        assert_eq!(row_slot(&mixer, SplatRow::Drums), Some((4_000.0, 2_000.0)));
        assert_eq!(row_slot(&mixer, SplatRow::Bass), None);
        // ...and moves to what is still sounding once the bass has stopped.
        {
            let state = mixer.state();
            let splat = state.decks[0].splat.as_ref().unwrap();
            assert_eq!(splat.view.map(|cell| cell.start_frames), Some(4_000.0), "the picture follows");
        }
        let secs = mixer.deck_snapshot(DeckId::A).position_secs;
        assert!((4.0..6.0).contains(&secs), "the playhead cycles in the new bars: {secs}");
    }

    #[test]
    fn splat_launch_swap_phase_stop_and_transport_return_are_quantized() {
        let (mixer, rate) = splat_fixture(false);
        render_count(&mixer, rate, 300, 64);
        mixer.splat_launch(DeckId::A, SplatRow::Drums, 0, SplatPart::WHOLE);
        // The first launch into a silent grid plays AT ONCE: the master
        // clock is re-seated on the cell (frame 0), the equal-power fade
        // starts on the first rendered sample (zero incoming gain) and the
        // very next one is live. From here the grid's bars are the cell's.
        // Read a look-ahead late, because that is where the bus hands
        // each pair over. The later checks read the splat's own frame
        // counters, which move with the render and not with the limiter.
        let latency = mixer.output_latency_frames();
        let first = render_count(&mixer, rate, 1_700, 256);
        assert!(
            first[latency].abs() < 1e-7 && first[latency + 1].abs() > 1e-5,
            "the click sounds at once"
        );
        assert!(first[1_000 + latency].abs() > 1e-5);
        render_count(&mixer, rate, 2, 64);

        render_count(&mixer, rate, 500, 64);
        // A second launch into a running grid waits for the next bar
        // (4000 on this clock), in phase with what is already playing.
        mixer.splat_launch(DeckId::A, SplatRow::Drums, 3, SplatPart::WHOLE);
        render_count(&mixer, rate, 1_804, 256);
        {
            let state = mixer.state();
            let splat = state.decks[0].splat.as_ref().unwrap();
            let cell = splat.rows[SplatRow::Drums.index()].cell.unwrap();
            assert_eq!(cell.col, 3);
            assert!((cell.anchor_frames - 4_000.0).abs() <= 1.0);
            let derived = cell.start_frames
                + (splat.master_frames - cell.anchor_frames).rem_euclid(cell.len_frames);
            assert!((derived - 6_006.0).abs() <= 1.0, "derived read: {derived}");
        }

        mixer.splat_launch(DeckId::A, SplatRow::Bass, 0, SplatPart::WHOLE);
        render_count(&mixer, rate, 2_000, 1_024);
        let snapshot = mixer.deck_snapshot(DeckId::A).splat.unwrap();
        assert_eq!(
            snapshot.playing[SplatRow::Bass.index()],
            Some((0, SplatPart::WHOLE))
        );
        assert!(
            (snapshot.row_phase[SplatRow::Drums.index()]
                - snapshot.row_phase[SplatRow::Bass.index()])
                .abs()
                < 1e-6
        );

        mixer.splat_stop_all(DeckId::A, true);
        render_count(&mixer, rate, 2_010, 256);
        let stopped = render(&mixer, rate as f64, 32);
        assert!(stopped.channel(0).iter().all(|sample| sample.abs() < 1e-7));

        let master = mixer.deck_snapshot(DeckId::A).position_secs;
        mixer.set_deck_splat_enabled(DeckId::A, false);
        let normal = mixer.deck_snapshot(DeckId::A);
        assert!((normal.position_secs - master).abs() <= 1.0 / rate as f64);
        assert!(normal.splat.is_some_and(|splat| !splat.active));
    }

    #[test]
    fn splat_phase_correction_crossfades_the_old_voice_when_crossing_a_launch() {
        let (mixer, rate) = splat_fixture(false);
        let (reference, _) = splat_fixture(false);
        for m in [&mixer, &reference] {
            m.splat_launch(DeckId::A, SplatRow::Mix, 0, SplatPart::WHOLE);
            render_count(m, rate, 1_950, 64);
            m.splat_launch(DeckId::A, SplatRow::Mix, 3, SplatPart::WHOLE);
        }
        mixer.nudge_deck_seconds(DeckId::A, 0.2);
        let corrected = render_count(&mixer, rate, 1, 1);
        let uninterrupted = render_count(&reference, rate, 1, 1);
        assert!((corrected[0] - uninterrupted[0]).abs() < 1e-7);
        assert_eq!(mixer.deck_snapshot(DeckId::A).splat.unwrap().playing[SplatRow::Mix.index()],
            Some((3, SplatPart::WHOLE)));
    }

    #[test]
    fn a_song_and_sample_grid_keep_time_through_many_loop_wraps() {
        let (mixer, rate) = splat_fixture(false);
        mixer.set_deck_playing(DeckId::A, false);
        mixer.set_deck_rate(DeckId::A, 128.0 / 120.0);
        mixer.set_deck_sync_lock(DeckId::A, true);
        mixer.install_deck(DeckId::B, const_pcm(4_000, rate as usize * 64, rate));
        mixer.set_deck_keylock(DeckId::B, false);
        render_count(&mixer, rate, 500, 64);
        mixer.set_deck_playing(DeckId::A, true);
        mixer.set_deck_playing(DeckId::B, true);
        mixer.splat_launch(DeckId::A, SplatRow::Mix, 3, SplatPart::WHOLE);
        for _ in 0..360 {
            render_count(&mixer, rate, 100, 37);
            let snapshots = mixer.deck_snapshots();
            let loop_clock = snapshots[0].splat.unwrap().clock_secs;
            let song_clock = snapshots[1].position_secs;
            let error = loop_clock * 120.0 / 60.0 - song_clock * 128.0 / 60.0;
            assert!(error.abs() < 1e-5, "song/sample drift: {error} beats");
            assert!((6.0..8.0).contains(&snapshots[0].position_secs));
        }
    }

    #[test]
    fn splat_uses_the_matched_deck_rate_on_every_row() {
        for streamed in [false, true] {
            let (mixer, rate) = splat_fixture(streamed);
            mixer.splat_launch(DeckId::A, SplatRow::Drums, 1, SplatPart::WHOLE);
            mixer.splat_launch(DeckId::A, SplatRow::Bass, 2, SplatPart::WHOLE);
            mixer.set_deck_rate(DeckId::A, 1.125);
            render_count(&mixer, rate, 500, 64); // let the rate ramp settle
            let before = mixer.deck_snapshot(DeckId::A).splat.unwrap().clock_secs;
            render_count(&mixer, rate, 8_000, 127);
            let after = mixer.deck_snapshot(DeckId::A).splat.unwrap();
            assert!((after.clock_secs - before - 9.0).abs() < 1e-8, "{streamed}: {after:?}");
            assert!((after.row_phase[SplatRow::Drums.index()] - after.row_phase[SplatRow::Bass.index()]).abs() < 1e-6);
        }
    }

    #[test]
    fn synced_first_splat_launch_waits_for_the_clock_and_preserves_its_source_span() {
        let (mixer, rate) = splat_fixture(false);
        mixer.set_deck_sync_lock(DeckId::A, true);
        render_count(&mixer, rate, 300, 64);
        mixer.splat_launch(DeckId::A, SplatRow::Mix, 3, SplatPart::WHOLE);
        render_count(&mixer, rate, 1, 1);
        let snap = mixer.deck_snapshot(DeckId::A).splat.unwrap();
        assert!((snap.clock_secs - 0.301).abs() < 1e-9);
        assert_eq!(snap.playing[SplatRow::Mix.index()], None);
        assert_eq!(snap.queued[SplatRow::Mix.index()], Some((3, SplatPart::WHOLE)));
        render_count(&mixer, rate, 1_700, 127);
        assert_eq!(row_slot(&mixer, SplatRow::Mix), Some((6_000.0, 2_000.0)));
        let before = mixer.deck_snapshot(DeckId::A).splat.unwrap().clock_secs;
        mixer.nudge_deck_seconds(DeckId::A, 32.125);
        mixer.sync();
        let after = mixer.deck_snapshot(DeckId::A).splat.unwrap().clock_secs;
        assert!((after - before - 32.125).abs() < 1e-9, "continuous clock is not clamped to file length");
        assert_eq!(row_slot(&mixer, SplatRow::Mix), Some((6_000.0, 2_000.0)));
        render_count(&mixer, rate, 500, 64);
        assert!(mixer.state().decks[0].splat.as_ref().unwrap().phase_fade.is_none());
    }

    #[test]
    fn splat_render_is_identical_across_block_sizes() {
        let run = |block| {
            let (mixer, rate) = splat_fixture(false);
            render_count(&mixer, rate, 300, block);
            mixer.splat_launch(
                DeckId::A,
                SplatRow::Drums,
                2,
                SplatPart { num: 1, den: 2 },
            );
            mixer.splat_launch(
                DeckId::A,
                SplatRow::Bass,
                4,
                SplatPart { num: 3, den: 4 },
            );
            render_count(&mixer, rate, 5_000, block)
        };
        assert_eq!(run(64), run(256));
        assert_eq!(run(64), run(1_024));
    }

    #[test]
    fn splat_quarter_reads_and_wraps_only_the_selected_source_subspan() {
        let (mixer, rate) = splat_fixture(false);
        let part = SplatPart { num: 2, den: 4 };
        mixer.splat_launch(DeckId::A, SplatRow::Drums, 0, part);
        render_count(&mixer, rate, 1, 1);

        let state = mixer.state();
        let deck = &state.decks[DeckId::A.index()];
        let splat = deck.splat.as_ref().unwrap();
        let cell = splat.rows[SplatRow::Drums.index()].cell.unwrap();
        assert_eq!(cell.part, part);
        assert_eq!(cell.start_frames, 1_000.0);
        assert_eq!(cell.len_frames, 500.0);
        assert_eq!(
            splat.snapshot().playing[SplatRow::Drums.index()],
            Some((0, part))
        );

        let pcm = deck.pcm.as_ref().unwrap();
        let stems = deck.stems.as_deref();
        let gains = [1.0; STEM_COUNT];
        let first = splat_cell_frame(
            SplatRow::Drums,
            cell,
            cell.anchor_frames,
            pcm,
            stems,
            gains,
        );
        let last = splat_cell_frame(
            SplatRow::Drums,
            cell,
            cell.anchor_frames + cell.len_frames - 1.0,
            pcm,
            stems,
            gains,
        );
        let wrapped = splat_cell_frame(
            SplatRow::Drums,
            cell,
            cell.anchor_frames + cell.len_frames,
            pcm,
            stems,
            gains,
        );
        assert_ne!(first, last);
        assert_eq!(first, wrapped);
    }

    #[test]
    fn splat_missing_stem_chunk_is_silence_without_mix_fallback() {
        let (mixer, rate) = splat_fixture(true);
        render_count(&mixer, rate, 300, 64);
        mixer.splat_launch(DeckId::A, SplatRow::Drums, 0, SplatPart::WHOLE);
        render_count(&mixer, rate, 2_010, 256);
        let output = render(&mixer, rate as f64, 64);
        assert!(output.channel(0).iter().all(|sample| sample.abs() < 1e-7));
    }

    #[test]
    fn splat_late_launch_forgiveness_uses_the_just_passed_bar() {
        let (mixer, rate) = splat_fixture(false);
        render_count(&mixer, rate, 10, 64);
        mixer.splat_launch(DeckId::A, SplatRow::Drums, 0, SplatPart::WHOLE);
        render_count(&mixer, rate, 1, 64);
        let snapshot = mixer.deck_snapshot(DeckId::A).splat.unwrap();
        assert_eq!(
            snapshot.playing[SplatRow::Drums.index()],
            Some((0, SplatPart::WHOLE))
        );
        let state = mixer.state();
        let anchor = state.decks[0].splat.as_ref().unwrap().rows[SplatRow::Drums.index()]
            .cell
            .unwrap()
            .anchor_frames;
        assert_eq!(anchor, 0.0);
    }


    // ---- the UI/audio seam ---------------------------------------------------

    /// Commands land in the order they were sent, through a ring that is
    /// too small for the burst: the overflow parks on the UI side and
    /// re-sends in order, never dropping or reordering a command.
    #[test]
    fn a_burst_past_the_ring_backs_up_on_the_ui_side_in_order() {
        let mixer = TestMixer::new();
        let burst = CMD_RING_SLOTS + 300;
        for step in 0..burst {
            // Every command is a distinct gain target; the last one wins
            // only if all of them arrive in order.
            mixer.set_deck_gain(DeckId::A, step as f32 / burst as f32);
        }
        assert_eq!(mixer.backlog_len(), 300, "the ring took its capacity, the rest waited");
        // The callback drains the ring; the next UI pump re-sends the rest.
        mixer.sync();
        assert_eq!(mixer.shared.cmds.len(), 0);
        assert_eq!(mixer.backlog_len(), 300, "nothing re-sends until the UI pumps");
        mixer.pump();
        assert_eq!(mixer.backlog_len(), 0);
        mixer.sync();
        let target = mixer.state().decks[0].gain.target;
        assert!(
            (target - (burst - 1) as f32 / burst as f32).abs() < 1e-6,
            "the last command applied last: {target}"
        );
        // Order across the seam: a command sent while the backlog stands
        // queues BEHIND it, so an install then a seek arrive in that order.
        for step in 0..burst {
            mixer.set_deck_gain(DeckId::B, step as f32);
        }
        mixer.install_deck(DeckId::B, const_pcm(1_000, 48_000, 48_000));
        mixer.seek_deck_seconds(DeckId::B, 0.5);
        assert!(mixer.backlog_len() > 0);
        mixer.sync();
        mixer.pump();
        mixer.sync();
        let (position, _, _) = mixer.deck_position(DeckId::B);
        assert!((position - 0.5).abs() < 1e-9, "the seek followed the install: {position}");
    }

    /// What a command replaces comes back to the UI thread: the last
    /// reference to a track is never dropped by the callback.
    #[test]
    fn replaced_payloads_come_back_for_the_ui_to_drop() {
        let mixer = TestMixer::new();
        let first = const_pcm(1_000, 48_000, 48_000);
        let second = const_pcm(2_000, 48_000, 48_000);
        mixer.install_deck(DeckId::A, first.clone());
        mixer.install_deck(DeckId::A, second.clone());
        render(&mixer, 48_000.0, 64);
        // The callback holds only the second; the first is in the events
        // ring, still alive, waiting for the UI.
        assert_eq!(Arc::strong_count(&first), 2, "the callback did not free it");
        let retired = mixer.drain_retired();
        assert!(
            retired.iter().any(|r| matches!(r, Retired::Pcm(DeckPcm::Whole(pcm)) if Arc::ptr_eq(pcm, &first))),
            "the replaced track came back whole"
        );
        drop(retired);
        assert_eq!(Arc::strong_count(&first), 1, "and the UI dropped it");
        assert_eq!(Arc::strong_count(&second), 2, "the playing track stays with the callback");

        // A stream table grown chunk by chunk hands back every old table.
        let stream = Arc::new(StreamPcm::new(48_000, Some(4 * STREAM_CHUNK_FRAMES)));
        mixer.install_deck_stream(DeckId::B, stream.clone());
        let grown = Arc::new(stream.with_chunk(stream_chunk(100, STREAM_CHUNK_FRAMES), false));
        mixer.grow_deck_stream(DeckId::B, grown.clone());
        let whole = const_pcm(100, STREAM_CHUNK_FRAMES, 48_000);
        mixer.complete_deck(DeckId::B, whole.clone());
        render(&mixer, 48_000.0, 64);
        let retired = mixer.drain_retired();
        let tables = retired
            .iter()
            .filter(|r| matches!(r, Retired::Pcm(DeckPcm::Stream(_))))
            .count();
        assert_eq!(tables, 2, "the empty table and the grown table both came back");
        drop(retired);
        assert_eq!(Arc::strong_count(&stream), 1);
        assert_eq!(Arc::strong_count(&grown), 1);
        assert_eq!(Arc::strong_count(&whole), 2);
    }

    /// The callback and the UI run flat out against each other and the
    /// callback never skips a buffer: there is no lock for it to lose.
    /// Every buffer rendered while the UI hammers commands and reads is
    /// accounted for, and the UI's reads never wait on a render.
    #[test]
    fn render_never_yields_a_buffer_to_the_ui_thread() {
        let handle = Mixer::new();
        let mut engine = handle.take_engine().expect("fresh engine");
        handle.install_deck(DeckId::A, const_pcm(4_000, 48_000 * 4, 48_000));
        // Looped, because an unpaced callback runs through four seconds of
        // track in well under the test's wall time.
        handle.set_deck_loop_span(DeckId::A, Some((0.0, 3.0)), crate::decks::LoopSeek::None);
        handle.set_deck_playing(DeckId::A, true);
        handle.set_master(1.0);
        let stop = Arc::new(AtomicBool::new(false));
        let audio_stop = stop.clone();
        let audio = std::thread::spawn(move || {
            let mut rendered = 0u64;
            let mut non_silent = 0u64;
            let mut buffer = AudioBuffer::new_with_size(128, 2);
            while !audio_stop.load(Ordering::Relaxed) {
                buffer.zero();
                engine.render(48_000.0, &mut buffer);
                rendered += 1;
                if buffer.channel(0).iter().any(|s| s.abs() > 1e-6) {
                    non_silent += 1;
                }
            }
            (rendered, non_silent, engine)
        });
        // The UI side: commands and reads as fast as it can for a while.
        let started = std::time::Instant::now();
        let mut reads = 0u64;
        while started.elapsed() < std::time::Duration::from_millis(400) {
            handle.set_deck_gain(DeckId::A, 0.9);
            handle.set_deck_eq_band(DeckId::A, 1, 1.1);
            handle.set_deck_stem_gain(DeckId::A, 2, 0.8);
            let _ = handle.deck_snapshot(DeckId::A);
            let _ = handle.crossfader_position();
            let _ = handle.meters();
            handle.pump();
            reads += 1;
        }
        stop.store(true, Ordering::Relaxed);
        let (rendered, non_silent, engine) = audio.join().expect("audio thread");
        assert!(reads > 100, "the UI side kept going: {reads}");
        assert!(rendered > 100, "the callback kept going: {rendered}");
        // Once the transport was applied every buffer carried audio: no
        // buffer was skipped for anything the UI did. The first few may be
        // silent only while the install and play commands travel.
        assert!(
            rendered - non_silent <= 2,
            "silent buffers: {} of {rendered}",
            rendered - non_silent
        );
        drop(engine);
    }

    /// The seam's types promise what the threads need and nothing more.
    #[test]
    fn handle_and_engine_cross_threads_without_a_mutex() {
        fn send<T: Send>() {}
        fn send_sync<T: Send + Sync>() {}
        send::<MixEngine>();
        send_sync::<Mixer>();
        send_sync::<Shared>();
    }

    /// Buffers of 512 frames at 48 kHz, the size the device asks for.
    fn spin_render(mixer: &TestMixer, buffers: usize) {
        for _ in 0..buffers {
            render(mixer, 48_000.0, 512);
        }
    }

    fn spin_deck_a(value: i16, frames: usize) -> TestMixer {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, const_pcm(value, frames, 48_000));
        mixer
    }

    #[test]
    fn a_roll_returns_the_deck_to_where_the_record_would_have_been() {
        let mixer = spin_deck_a(16_384, 480_000);
        mixer.set_deck_playing(DeckId::A, true);
        spin_render(&mixer, 16);
        let armed_at = mixer.deck_snapshot(DeckId::A).position_secs;

        mixer.push_deck_roll(DeckId::A);
        mixer.set_deck_loop_span(
            DeckId::A,
            Some((armed_at, armed_at + 0.25)),
            crate::decks::LoopSeek::MovedOut,
        );
        assert_eq!(mixer.state().decks[DeckId::A.index()].rolls.len, 1);
        let held = 40;
        spin_render(&mixer, held);
        mixer.pop_deck_roll(DeckId::A, None, false);
        spin_render(&mixer, 1);

        let want = armed_at + (held * 512) as f64 / 48_000.0;
        let landed = mixer.deck_snapshot(DeckId::A).position_secs;
        assert!(
            (landed - want).abs() < 0.02,
            "the record carried on underneath: want {want}, got {landed}",
        );
        assert_eq!(mixer.state().decks[DeckId::A.index()].rolls.len, 0);
    }

    #[test]
    fn a_roll_held_over_another_returns_into_the_one_beneath_it() {
        let mixer = spin_deck_a(16_384, 480_000);
        mixer.set_deck_playing(DeckId::A, true);
        spin_render(&mixer, 8);
        let outer = (2.0, 3.0);

        // The outer roll: a one-second loop.
        mixer.push_deck_roll(DeckId::A);
        mixer.set_deck_loop_span(
            DeckId::A,
            Some(outer),
            crate::decks::LoopSeek::MovedOut,
        );
        // The engine sends the record into a loop it engages; here that is
        // the caller's job.
        mixer.seek_deck_seconds(DeckId::A, outer.0);
        spin_render(&mixer, 8);
        // The inner one, over it.
        mixer.push_deck_roll(DeckId::A);
        mixer.set_deck_loop_span(
            DeckId::A,
            Some((2.0, 2.125)),
            crate::decks::LoopSeek::MovedOut,
        );
        assert_eq!(mixer.state().decks[DeckId::A.index()].rolls.len, 2);
        spin_render(&mixer, 40);

        // Letting the inner one go lands INSIDE the outer one -- its ghost
        // wrapped through the outer span, not through the whole track.
        mixer.pop_deck_roll(DeckId::A, Some(outer), false);
        spin_render(&mixer, 1);
        let at = mixer.deck_snapshot(DeckId::A).position_secs;
        assert!(at >= outer.0 && at < outer.1, "back inside the outer roll, at {at}");
        assert_eq!(mixer.state().decks[DeckId::A.index()].rolls.len, 1);
    }

    #[test]
    fn adopting_a_roll_keeps_what_is_sounding_and_stands_every_level_down() {
        let mixer = spin_deck_a(16_384, 480_000);
        mixer.set_deck_playing(DeckId::A, true);
        spin_render(&mixer, 8);
        mixer.push_deck_roll(DeckId::A);
        mixer.push_deck_roll(DeckId::A);
        mixer.set_deck_loop_span(
            DeckId::A,
            Some((2.0, 2.5)),
            crate::decks::LoopSeek::MovedOut,
        );
        mixer.seek_deck_seconds(DeckId::A, 2.0);
        spin_render(&mixer, 20);
        let at = mixer.deck_snapshot(DeckId::A).position_secs;

        mixer.pop_deck_roll(DeckId::A, None, true);
        spin_render(&mixer, 1);
        assert_eq!(mixer.state().decks[DeckId::A.index()].rolls.len, 0, "every level stood down");
        let after = mixer.deck_snapshot(DeckId::A).position_secs;
        assert!(after >= 2.0 && after < 2.5, "still in the loop it adopted, at {after}");
        assert!((after - at).abs() < 0.05, "and it did not jump: {at} -> {after}");
    }

    #[test]
    fn a_roll_stack_is_bounded_and_a_pop_with_nothing_on_it_does_nothing() {
        let mixer = spin_deck_a(16_384, 480_000);
        mixer.set_deck_playing(DeckId::A, true);
        spin_render(&mixer, 8);
        for _ in 0..ROLL_STACK_CAP + 3 {
            mixer.push_deck_roll(DeckId::A);
        }
        assert_eq!(mixer.state().decks[DeckId::A.index()].rolls.len, ROLL_STACK_CAP, "a fixed depth");
        for _ in 0..ROLL_STACK_CAP {
            mixer.pop_deck_roll(DeckId::A, None, false);
        }
        let at = mixer.deck_snapshot(DeckId::A).position_secs;
        mixer.pop_deck_roll(DeckId::A, None, false);
        spin_render(&mixer, 1);
        assert_eq!(mixer.state().decks[DeckId::A.index()].rolls.len, 0);
        assert!(mixer.deck_snapshot(DeckId::A).position_secs >= at, "and nothing jumped back");
    }

    #[test]
    fn a_roll_is_click_free_both_ways() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, split_pcm(16_384, -16_384, 480_000, 48_000));
        mixer.seek_deck_seconds(DeckId::A, 7.0);
        mixer.set_deck_playing(DeckId::A, true);
        spin_render(&mixer, 8);
        let mut worst = 0.0f32;
        let mut previous: Option<f32> = None;
        for index in 0..48 {
            if index == 8 {
                mixer.push_deck_roll(DeckId::A);
                mixer.set_deck_loop_span(
                    DeckId::A,
                    Some((7.1, 7.35)),
                    crate::decks::LoopSeek::MovedOut,
                );
            }
            if index == 32 {
                mixer.pop_deck_roll(DeckId::A, None, false);
            }
            let block = render(&mixer, 48_000.0, 512);
            for sample in &block.channel(0)[..512] {
                if let Some(last) = previous {
                    worst = worst.max((sample - last).abs());
                }
                previous = Some(*sample);
            }
        }
        assert!(worst < 0.02, "a roll must blend in and out, biggest step {worst}");
    }

    #[test]
    fn slip_keeps_a_ghost_running_while_the_record_is_taken_elsewhere() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }
        mixer.set_deck_slip(DeckId::A, true, false);
        assert!(mixer.deck_slipping(DeckId::A));
        let armed_at = mixer.deck_snapshot(DeckId::A).position_secs;

        // The hand takes the record somewhere else entirely.
        mixer.seek_deck_seconds(DeckId::A, 5.0);
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }
        assert!(
            mixer.deck_snapshot(DeckId::A).position_secs > 4.9,
            "the real head went where it was told"
        );

        mixer.set_deck_slip(DeckId::A, false, false);
        let landed = mixer.deck_snapshot(DeckId::A).position_secs;
        let elapsed = 8.0 * 512.0 / 48_000.0;
        assert!(
            (landed - (armed_at + elapsed)).abs() < 0.01,
            "the deck lands where the track would have got to: {landed} against {}",
            armed_at + elapsed
        );
        assert!(!mixer.deck_slipping(DeckId::A));
    }

    #[test]
    fn keeping_what_was_scratched_leaves_the_record_where_the_hand_left_it() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 512);
        mixer.set_deck_slip(DeckId::A, true, false);
        mixer.seek_deck_seconds(DeckId::A, 5.0);
        render(&mixer, 48_000.0, 512);
        mixer.set_deck_slip(DeckId::A, false, true);
        assert!(
            (mixer.deck_snapshot(DeckId::A).position_secs - 5.0).abs() < 0.05,
            "adopting keeps the detour: {}",
            mixer.deck_snapshot(DeckId::A).position_secs
        );
    }

    #[test]
    fn a_ghost_wraps_through_the_loop_that_was_running_when_slip_was_armed() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_loop_span(DeckId::A, Some((0.0, 0.1)), crate::decks::LoopSeek::MovedOut);
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 512);
        mixer.set_deck_slip(DeckId::A, true, false);
        // Far more than the loop is long.
        for _ in 0..40 {
            render(&mixer, 48_000.0, 512);
        }
        mixer.set_deck_slip(DeckId::A, false, false);
        let landed = mixer.deck_snapshot(DeckId::A).position_secs;
        assert!(
            (0.0..=0.1).contains(&landed),
            "the ghost stayed inside the loop the operator can see: {landed}"
        );
    }

    #[test]
    fn a_ghost_runs_on_while_the_deck_is_paused() {
        // "Regardless of what the real head does" is meant literally: this
        // is what makes slip useful over a stop, not only over a scratch.
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 512);
        mixer.set_deck_slip(DeckId::A, true, false);
        let armed_at = mixer.deck_snapshot(DeckId::A).position_secs;
        mixer.set_deck_playing(DeckId::A, false);
        for _ in 0..16 {
            render(&mixer, 48_000.0, 512);
        }
        mixer.set_deck_slip(DeckId::A, false, false);
        assert!(
            mixer.deck_snapshot(DeckId::A).position_secs > armed_at + 0.1,
            "the ghost kept going while the record stood still"
        );
    }

    /// The published third tempo: what the record is turning at, through
    /// every gesture that can own it.
    #[test]
    fn the_deck_snapshot_reports_what_the_platter_is_turning_at() {
        let mixer = spin_deck_a(16_384, 480_000);
        mixer.set_deck_playing(DeckId::A, true);
        spin_render(&mixer, 8);
        let running = mixer.deck_snapshot(DeckId::A).platter_rate;
        assert!((running - 1.0).abs() < 1e-6, "the deck's own rate: {running}");

        // A hand on the record brakes it toward a stop.
        mixer.scratch_deck(DeckId::A, ScratchMotion::Grab);
        spin_render(&mixer, 8);
        let held = mixer.deck_snapshot(DeckId::A);
        assert!(held.scratching, "the ramp owns the rate");
        assert!(held.platter_rate < running, "slowing: {}", held.platter_rate);
        assert!(held.platter_rate >= 0.0, "and not through zero");

        // A hand outranks a motor, so let go before asking for one.
        mixer.scratch_deck(DeckId::A, ScratchMotion::Release);
        spin_render(&mixer, 64);
        mixer.set_deck_censor(DeckId::A, true);
        spin_render(&mixer, 64);
        let reversed = mixer.deck_snapshot(DeckId::A).platter_rate;
        assert!(reversed < 0.0, "a reverse hold turns the record back: {reversed}");
    }

#[test]
fn a_brake_leaves_the_record_where_it_wound_down() {
    let mixer = spin_deck_a(16_384, 480_000);
    mixer.set_deck_playing(DeckId::A, true);
    spin_render(&mixer, 8);
    let began = mixer.deck_snapshot(DeckId::A).position_secs;

    mixer.spin_deck(DeckId::A, SpinMotion::Brake);
    spin_render(&mixer, (48_000.0 * BRAKE_SECS / 512.0) as usize + 8);
    let stopped = mixer.deck_snapshot(DeckId::A).position_secs;
    // The record TRAVELLED while it wound down. A pause hands those
    // frames back; a brake must not.
    assert!(stopped > began, "the platter carried on: {began} -> {stopped}");
    assert!(!mixer.deck_scratching(DeckId::A), "and handed the rate back");

    spin_render(&mixer, 8);
    let after = mixer.deck_snapshot(DeckId::A).position_secs;
    assert!((after - stopped).abs() < 1e-6, "and then stayed there: {stopped} -> {after}");
}

#[test]
fn a_censor_is_refused_while_a_hand_is_on_the_record() {
    let mixer = spin_deck_a(16_384, 480_000);
    mixer.set_deck_playing(DeckId::A, true);
    spin_render(&mixer, 8);
    mixer.scratch_deck(DeckId::A, ScratchMotion::Grab);
    mixer.set_deck_censor(DeckId::A, true);
    assert!(
        mixer.state().decks[DeckId::A.index()].slip.is_none(),
        "no ghost was armed",
    );
}

#[test]
fn a_censor_runs_the_record_backwards_while_it_is_held() {
    let mixer = spin_deck_a(16_384, 480_000);
    mixer.set_deck_playing(DeckId::A, true);
    spin_render(&mixer, 16);
    let at = mixer.deck_snapshot(DeckId::A).position_secs;
    assert!(at > 0.0, "the record was running");

    mixer.set_deck_censor(DeckId::A, true);
    spin_render(&mixer, 16);
    let back = mixer.deck_snapshot(DeckId::A).position_secs;
    assert!(back < at, "the record runs backwards: {at} -> {back}");
    assert!(mixer.deck_scratching(DeckId::A), "the ramp owns the rate");
}

#[test]
fn letting_go_of_a_censor_lands_on_where_the_track_would_have_been() {
    let mixer = spin_deck_a(16_384, 480_000);
    mixer.set_deck_playing(DeckId::A, true);
    spin_render(&mixer, 16);
    let armed_at = mixer.deck_snapshot(DeckId::A).position_secs;

    mixer.set_deck_censor(DeckId::A, true);
    let held = 24;
    spin_render(&mixer, held);
    mixer.set_deck_censor(DeckId::A, false);
    spin_render(&mixer, 1);

    let want = armed_at + (held * 512) as f64 / 48_000.0;
    let landed = mixer.deck_snapshot(DeckId::A).position_secs;
    assert!(
        (landed - want).abs() < 0.02,
        "the deck lands where the record would have got to: want {want}, got {landed}",
    );
}

#[test]
fn a_spin_back_throws_the_record_backwards_before_it_stops() {
    let mixer = spin_deck_a(16_384, 480_000);
    mixer.seek_deck_seconds(DeckId::A, 5.0);
    mixer.set_deck_playing(DeckId::A, true);
    spin_render(&mixer, 8);
    let began = mixer.deck_snapshot(DeckId::A).position_secs;

    mixer.spin_deck(DeckId::A, SpinMotion::SpinBack);
    let total = SPINBACK_THROW_SECS + SPINBACK_FALL_SECS;
    spin_render(&mixer, (48_000.0 * total / 512.0) as usize + 8);
    let stopped = mixer.deck_snapshot(DeckId::A).position_secs;
    assert!(stopped < began, "it went backwards: {began} -> {stopped}");
    assert!(!mixer.deck_scratching(DeckId::A));
}

#[test]
fn a_soft_start_comes_up_to_tempo_instead_of_cutting_in() {
    let mixer = spin_deck_a(16_384, 480_000);
    mixer.spin_deck(DeckId::A, SpinMotion::SoftStart);
    let quarter = (48_000.0 * SOFT_START_SECS / 4.0 / 512.0) as usize;
    spin_render(&mixer, quarter);
    let early = mixer.deck_snapshot(DeckId::A).position_secs;
    let real = (quarter * 512) as f64 / 48_000.0;
    assert!(early < real * 0.6, "the platter is still winding up: {early} of {real}");

    spin_render(&mixer, (48_000.0 * SOFT_START_SECS / 512.0) as usize + 8);
    let settled = mixer.deck_snapshot(DeckId::A).position_secs;
    spin_render(&mixer, 8);
    let moved = mixer.deck_snapshot(DeckId::A).position_secs - settled;
    let want = (8 * 512) as f64 / 48_000.0;
    assert!((moved - want).abs() < 0.005, "and then runs at tempo: {moved} of {want}");
}

#[test]
fn reverse_inside_a_loop_wraps_back_to_the_out_point() {
    let mixer = spin_deck_a(16_384, 480_000);
    mixer.set_deck_loop_span(DeckId::A, Some((4.0, 5.0)), crate::decks::LoopSeek::MovedOut);
    mixer.seek_deck_seconds(DeckId::A, 4.5);
    mixer.set_deck_playing(DeckId::A, true);
    spin_render(&mixer, 8);
    mixer.set_deck_censor(DeckId::A, true);
    // Far longer than the span: without a two-sided wrap the record
    // walks out at IN and off the front of the track.
    spin_render(&mixer, 400);
    let at = mixer.deck_snapshot(DeckId::A).position_secs;
    assert!(at >= 4.0 && at <= 5.0, "the loop still owns the playhead, at {at}");
}

    #[test]
    fn a_head_that_never_belonged_to_the_span_is_left_to_play_its_way_in() {
        // The patient rule: a deck sitting behind a loop is playing into
        // it, deliberately and audibly. Folding it forward would teleport
        // it over the run-up.
        let mixer = spin_deck_a(16_384, 480_000);
        mixer.seek_deck_seconds(DeckId::A, 2.0);
        mixer.set_deck_loop_span(
            DeckId::A,
            Some((5.0, 9.0)),
            crate::decks::LoopSeek::Changed,
        );
        let at = mixer.deck_snapshot(DeckId::A).position_secs;
        assert!((at - 2.0).abs() < 1e-6, "left exactly where it was, at {at}");
    }

    #[test]
    fn a_resize_folds_a_head_that_belonged_to_the_old_span() {
        // The engine's mirror of the playhead is a stale 20 Hz number, so
        // it says what it MEANT and the mixer does it against the real one.
        let mixer = spin_deck_a(16_384, 480_000);
        mixer.set_deck_loop_span(
            DeckId::A,
            Some((2.0, 6.0)),
            crate::decks::LoopSeek::MovedOut,
        );
        mixer.seek_deck_seconds(DeckId::A, 5.5);
        // Halve it from IN: the head is now past the new OUT.
        mixer.set_deck_loop_span(
            DeckId::A,
            Some((2.0, 4.0)),
            crate::decks::LoopSeek::Changed,
        );
        let at = mixer.deck_snapshot(DeckId::A).position_secs;
        assert!(
            (at - 3.5).abs() < 1e-6,
            "folded by a whole length, keeping its phase, at {at}",
        );

        // And backwards: move the span forward under a head that was in it.
        mixer.set_deck_loop_span(
            DeckId::A,
            Some((6.0, 8.0)),
            crate::decks::LoopSeek::Changed,
        );
        let at = mixer.deck_snapshot(DeckId::A).position_secs;
        assert!(at >= 6.0 && at < 8.0, "and into the new span, at {at}");
    }

    #[test]
    fn a_span_folds_a_playhead_back_keeping_the_overshoot() {
        // Modulo, not a reset to IN: resetting discards up to a step a lap
        // and a held loop walks audibly early.
        assert_eq!(wrapped_into_span(105.0, 100.0, 110.0), 105.0, "already inside");
        assert_eq!(wrapped_into_span(112.0, 100.0, 110.0), 102.0, "two frames past OUT");
        assert_eq!(wrapped_into_span(130.0, 100.0, 110.0), 100.0, "three whole laps");
        assert_eq!(wrapped_into_span(98.0, 100.0, 110.0), 108.0, "and backwards");
        // A span of nothing is floored at one frame rather than dividing by
        // zero, so it parks at its own start.
        assert_eq!(wrapped_into_span(5.0, 3.0, 3.0), 3.0);
    }

    #[test]
    fn pausing_leaves_the_playhead_where_it_was_pressed() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }
        let at_press = mixer.deck_snapshot(DeckId::A).position_secs;
        mixer.set_deck_playing(DeckId::A, false);
        // Well past the fade: the deck keeps reading while it fades, and
        // hands those frames back when it reaches silence.
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }
        let after = mixer.deck_snapshot(DeckId::A).position_secs;
        assert!(
            (after - at_press).abs() < 1e-9,
            "pause moved the playhead from {at_press} to {after}"
        );
        // And it stays put, however long it sits there.
        for _ in 0..20 {
            render(&mixer, 48_000.0, 512);
        }
        let later = mixer.deck_snapshot(DeckId::A).position_secs;
        assert!((later - at_press).abs() < 1e-9, "a paused deck crept to {later}");
    }

    #[test]
    fn a_double_onto_itself_does_nothing() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, tone_pcm(220.0, 48_000, 8.0));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 512);
        let before = mixer.deck_snapshot(DeckId::A).position_secs;
        mixer.clone_deck(DeckId::A, DeckId::A);
        assert_eq!(mixer.deck_snapshot(DeckId::A).position_secs, before);
    }

    #[test]
    fn a_double_takes_the_record_and_leaves_the_channel_strip() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, tone_pcm(220.0, 48_000, 8.0));
        mixer.set_deck_gain(DeckId::A, 0.3);
        mixer.set_deck_gain(DeckId::B, 0.9);
        mixer.set_deck_rate(DeckId::A, 1.05);
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 512);
        mixer.clone_deck(DeckId::A, DeckId::B);
        let s = mixer.state();
        assert!(s.decks[1].pcm.is_some(), "the record travelled");
        assert!(
            (s.decks[1].rate.current() - s.decks[0].rate.current()).abs() < 1e-6,
            "and so did the tempo it is running at"
        );
        assert!(
            (s.decks[1].gain.current - 0.9).abs() < 1e-6,
            "the fader belongs to the slot: {}",
            s.decks[1].gain.current
        );
    }

    #[test]
    fn an_instant_double_lands_on_the_same_sample() {
        // The whole point: CloneDeck reads the source playhead as part of
        // the same command application the callback advances it with, so
        // the two decks are not a buffer apart -- which is what a flanged
        // double sounds like.
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, tone_pcm(220.0, 48_000, 8.0));
        mixer.set_deck_playing(DeckId::A, true);
        for _ in 0..10 {
            render(&mixer, 48_000.0, 512);
        }
        mixer.clone_deck(DeckId::A, DeckId::B);
        let a = mixer.deck_snapshot(DeckId::A).position_secs;
        let b = mixer.deck_snapshot(DeckId::B).position_secs;
        assert!((a - b).abs() < 1e-9, "{a} against {b}");
        assert!(mixer.deck_snapshot(DeckId::B).playing, "and it is running");
        assert!(b > 0.0, "on the record, not at its head");
    }

    /// Silent, except one full-scale frame at `at_secs`: a mark to time a
    /// delay against. `fixtures::click_pcm` (mixer.rs:6056) exists but is
    /// not imported into `mod tests` (which keeps its own copies of
    /// const_pcm/tone_pcm/split_pcm instead of importing `fixtures::*`),
    /// so this mirrors that existing duplication pattern.
    fn click_pcm(at_secs: f64, rate: u32, seconds: f64) -> Arc<TrackPcm> {
        let len = (rate as f64 * seconds) as usize;
        let at = (at_secs * rate as f64).round() as usize;
        let mut all = vec![[0i16, 0i16]; len];
        if at < len {
            all[at] = [i16::MAX, i16::MAX];
        }
        Arc::new(TrackPcm { frames: all, sample_rate: rate })
    }

    fn clock_grid(bpm: f64) -> TrackGrid {
        TrackGrid {
            bpm,
            beat_secs: 60.0 / bpm,
            first_beat_secs: 0.1,
            downbeat_phase: 0,
            confidence: 0.9,
        }
    }

    /// Render and hand back samples whose index IS the source frame.
    ///
    /// The master bus runs a look-ahead behind the mix, so what leaves at
    /// index N went in `latency` frames earlier. One extra block is
    /// rendered and that many samples dropped off the front, which puts
    /// the two back in step -- otherwise every test that looks for a
    /// click at a known time finds it late by exactly the look-ahead.
    fn render_out(mixer: &TestMixer, rate: f64, buffers: usize, block: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity((buffers + 1) * block);
        for _ in 0..=buffers {
            out.extend_from_slice(render(mixer, rate, block).channel(0));
        }
        // Asked for AFTER rendering, not before: the limiter only learns
        // the device rate inside the callback, so until one buffer has
        // been through it its look-ahead is still the one-frame default.
        let latency = mixer.output_latency_frames();
        out.drain(..latency.min(out.len()));
        out
    }

    /// The largest sample in a window around `around_secs`, and how far
    /// from the window's centre it landed.
    fn peak_near(out: &[f32], rate: f64, around_secs: f64, half_window: usize) -> (f32, i64) {
        let centre = (around_secs * rate).round() as i64;
        let lo = (centre - half_window as i64).max(0) as usize;
        let hi = ((centre + half_window as i64) as usize).min(out.len());
        let mut best = (0.0f32, centre);
        for (index, &value) in out[lo..hi].iter().enumerate() {
            if value.abs() > best.0.abs() {
                best = (value, lo as i64 + index as i64);
            }
        }
        (best.0, best.1 - centre)
    }

    #[test]
    fn a_clone_keeps_the_destinations_own_echo_setting_and_forgets_its_tail() {
        let rate = 48_000.0;
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(1.0); // deck B
        mixer.install_deck(DeckId::B, click_pcm(0.05, 48_000, 4.0));
        mixer.set_deck_keylock(DeckId::B, false);
        mixer.set_deck_grid(DeckId::B, Some(clock_grid(120.0)));
        mixer.set_deck_echo(DeckId::B, Some((1, 1))); // whole beat, 24 000 frames
        mixer.set_deck_echo_feedback(DeckId::B, 0.8);
        mixer.set_deck_playing(DeckId::B, true);
        // B's own click and its repeat both land: a real tail exists.
        render_out(&mixer, rate, 100, 512);
        // Silent, but PLAYING: a silent, paused source would clone its
        // own paused transport onto B too, and a paused deck's frame
        // loop never touches its echo at all -- which would hide a
        // leak regardless of whether this fix is in place.
        mixer.install_deck(DeckId::A, const_pcm(0, 480_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        render_out(&mixer, rate, 4, 512); // let A's transport settle
        mixer.clone_deck(DeckId::A, DeckId::B);
        assert_eq!(
            mixer.state().decks[1].chain.echo_mut().fraction(),
            Some((1, 1)),
            "the clone did not touch the destination's own setting"
        );
        // Whatever B was about to repeat next -- its own earlier click,
        // one more beat on -- must not be heard: the record under it is
        // now silence, and the stale content must have gone with it.
        let out = render_out(&mixer, rate, 100, 512);
        let (peak, _) = peak_near(&out, rate, 0.5, 24_000);
        assert!(peak.abs() < 0.05, "a stale tail bled through the clone: {peak}");
    }

    /// How long the lap it is holding actually came out, in frames. The
    /// old `Mixer::deck_freeze_lap` locked `state` directly; HEAD's
    /// equivalent read is `TestMixer::state()` (drains the ring, then
    /// hands back the engine's own `MixState`), so this is that read at
    /// the same call shape `deck_pos` already uses for `playhead_frames`.
    fn deck_freeze_lap(mixer: &TestMixer, deck: DeckId) -> Option<usize> {
        mixer.state().decks[deck.index()].chain.freeze_mut().lap_frames()
    }

    /// FREEZE repeats the SIGNAL; the playhead itself never stops.
    /// The lap arrives in SOURCE seconds and becomes output seconds
    /// through the platter's own rate, read at the instant it latches.
    /// It used to be divided on the control thread by the tempo fader,
    /// which is not the platter's rate the moment a hand is on the
    /// record -- so a beat-sized stutter grabbed during a scratch came
    /// out the wrong size, which is the one thing a lap has to get right.
    #[test]
    fn a_freeze_lap_is_measured_by_the_platter_not_the_fader() {
        let lap_at = |rate: f64| {
            let mixer = spin_deck_a(16_384, 480_000);
            mixer.set_deck_playing(DeckId::A, true);
            mixer.set_deck_rate(DeckId::A, rate);
            // Long enough that the ring has more fresh content than
            // either lap asks for: a hold is clamped to what has actually
            // been written, and a clamped lap would compare equal however
            // fast the platter was turning.
            spin_render(&mixer, 80);
            mixer.set_deck_freeze(DeckId::A, Some(0.2));
            deck_freeze_lap(&mixer, DeckId::A).expect("a lap")
        };
        // The same half-second of RECORD, with the platter turning twice
        // as fast: half the output seconds, so half the frames.
        let at_unity = lap_at(1.0);
        let at_double = lap_at(2.0);
        let ratio = at_unity as f64 / at_double as f64;
        assert!(
            (ratio - 2.0).abs() < 0.1,
            "a doubled platter should halve the lap: {at_unity} against {at_double}"
        );
    }

    /// Whether this deck's FREEZE is sounding right now -- held, or
    /// still crossfading out of one. Replaces the old public
    /// `Mixer::deck_frozen`, which locked `state` directly; the same
    /// read now goes through the harness's `TestMixer::state()`.
    fn deck_frozen(mixer: &TestMixer, deck: DeckId) -> bool {
        mixer.state().decks[deck.index()].chain.freeze_mut().held()
    }

    #[test]
    fn a_freeze_leaves_the_record_running_underneath() {
        let mixer = spin_deck_a(16_384, 480_000);
        mixer.set_deck_playing(DeckId::A, true);
        spin_render(&mixer, 8);
        mixer.set_deck_freeze(DeckId::A, Some(0.25));
        assert!(deck_frozen(&mixer, DeckId::A));
        let before = mixer.deck_snapshot(DeckId::A).position_secs;
        spin_render(&mixer, 40);
        let advanced = mixer.deck_snapshot(DeckId::A).position_secs - before;
        let expected = 40.0 * 512.0 / 48_000.0;
        assert!(
            (advanced - expected).abs() < 0.02,
            "the record must keep running: advanced {advanced}, expected {expected}"
        );
        mixer.set_deck_freeze(DeckId::A, None);
        spin_render(&mixer, 1); // past FREEZE_BLEND_SECS
        assert!(!deck_frozen(&mixer, DeckId::A));
    }

    #[test]
    fn a_freeze_on_a_stopped_or_empty_deck_does_nothing() {
        let mixer = TestMixer::new();
        mixer.set_deck_freeze(DeckId::A, Some(0.5));
        assert!(!deck_frozen(&mixer, DeckId::A), "nothing loaded");
        mixer.install_deck(DeckId::A, tone_pcm(220.0, 48_000, 4.0));
        mixer.set_deck_freeze(DeckId::A, Some(0.5));
        assert!(!deck_frozen(&mixer, DeckId::A), "loaded but not playing");
        // No callback has ever run on this mixer: there is no device
        // rate yet to turn a seconds value into a ring length.
        let fresh = TestMixer::new();
        fresh.install_deck(DeckId::A, tone_pcm(220.0, 48_000, 4.0));
        fresh.set_deck_playing(DeckId::A, true);
        fresh.set_deck_freeze(DeckId::A, Some(0.5));
        assert!(!deck_frozen(&fresh, DeckId::A), "no device rate latched yet");
    }

    /// The repeat is of the POST-FILTER signal, taken after the read
    /// path but before gain and the fader -- so PFL and the master both
    /// hear it, RAW does not, and the record itself is free to have run
    /// on somewhere else entirely by the time it is heard again.
    #[test]
    fn a_freeze_repeats_the_post_eq_signal_and_the_phones_hear_it() {
        let rate = 48_000.0;
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, split_pcm(16_384, -16_384, 480_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        spin_render(&mixer, 20); // build up real content to reach back into
        mixer.seek_deck_seconds(DeckId::A, 4.95);
        spin_render(&mixer, 5); // cross the split at 5.0 s by a small margin
        mixer.set_deck_freeze(DeckId::A, Some(0.1));
        assert!(deck_frozen(&mixer, DeckId::A));
        let out = render_out(&mixer, rate, 40, 512);
        // The frozen lap reaches back across the split, so it carries
        // both the +0.5 side and the -0.5 side, laps on laps.
        assert!(out.iter().any(|&v| v > 0.3), "the lap's +0.5 side must still be there");
        assert!(out.iter().any(|&v| v < -0.3), "the lap's -0.5 side must still be there");
        let snap = mixer.deck_snapshot(DeckId::A);
        assert!(snap.position_secs > 5.0, "the record itself ran on past the split: {}", snap.position_secs);

        mixer.set_cue_armed(true);
        mixer.set_deck_cue(DeckId::A, true);
        mixer.set_cue_mode(CueMode::Pfl);
        let mut pfl_state = CueReadState::default();
        let mut pfl_has_positive = false;
        // The phones consumer lags the writer by CUE_TARGET_FRAMES, so a
        // single buffer is not enough to prove anything; drain several,
        // the way the existing PFL/RAW test does.
        for _ in 0..16 {
            render(&mixer, rate, 512);
            let pfl = consume_cue(&mixer, &mut pfl_state, rate, 512);
            pfl_has_positive |= pfl.channel(0).iter().any(|&v| v > 0.3);
        }
        assert!(pfl_has_positive, "PFL must hear the frozen lap");

        mixer.set_cue_mode(CueMode::Raw);
        let mut raw_state = CueReadState::default();
        // Drain what the ring still owes from PFL mode before trusting
        // any of it to say what RAW actually carries now.
        for _ in 0..16 {
            render(&mixer, rate, 512);
            consume_cue(&mixer, &mut raw_state, rate, 512);
        }
        let mut raw = Vec::new();
        for _ in 0..16 {
            render(&mixer, rate, 512);
            raw.extend_from_slice(consume_cue(&mixer, &mut raw_state, rate, 512).channel(0));
        }
        assert!(raw.iter().all(|&v| v < -0.3), "RAW must hear the live -0.5, not the lap");
    }

    /// A half-beat echo repeats the click that much later, at the tempo
    /// the room actually hears -- the tempo fader included.
    #[test]
    fn an_echo_repeats_the_deck_half_a_beat_later_at_the_heard_tempo() {
        let rate = 48_000.0;
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, click_pcm(1.0, 48_000, 6.0));
        mixer.set_deck_keylock(DeckId::A, false);
        mixer.set_deck_grid(DeckId::A, Some(clock_grid(120.0))); // 0.5 s/beat
        mixer.set_deck_echo(DeckId::A, Some((1, 2)));
        mixer.set_deck_playing(DeckId::A, true);
        let out = render_out(&mixer, rate, 188, 512); // ~2 s
        // 0.25 s = 12 000 frames after the click at 1.0 s = frame 48 000.
        let (peak, offset) = peak_near(&out, rate, 1.25, 4);
        assert!(peak > 0.3, "{peak}");
        for index in 48_020..59_980 {
            assert!(out[index].abs() < 0.05, "sound before the repeat at {index}: {}", out[index]);
        }
        assert!(offset.abs() <= 4, "{offset}");

        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, click_pcm(1.0, 48_000, 6.0));
        mixer.set_deck_keylock(DeckId::A, false);
        mixer.set_deck_grid(DeckId::A, Some(clock_grid(120.0)));
        mixer.set_deck_rate(DeckId::A, 1.25);
        mixer.set_deck_echo(DeckId::A, Some((1, 2)));
        mixer.set_deck_playing(DeckId::A, true);
        let out = render_out(&mixer, rate, 150, 512); // ~1.6 s
        // A resampled read (rate != 1) interpolates a one-frame impulse
        // across its neighbours, so neither the click's own peak nor its
        // exact arrival time is the untouched 0.8 s a plain division
        // predicts -- the rate ramp settling on its way to 1.25 pushes it
        // a little further out. Find where the click ACTUALLY landed
        // first, then look for the echo the fixed delay away from THAT.
        let (click_peak, click_at) = peak_near(&out, rate, 0.8, 1_000);
        assert!(click_peak.abs() > 0.15, "{click_peak}");
        let click_secs = 0.8 + click_at as f64 / rate;
        // beat_frames(0.5, 1.25, 48_000) / 2 = 9 600, in device frames --
        // the same domain the click's own position was just measured in.
        let (peak, offset) = peak_near(&out, rate, click_secs + 9_600.0 / rate, 4);
        assert!(peak.abs() > 0.15, "{peak}");
        assert!(offset.abs() <= 4, "{offset}");
    }

    /// With no grid at all, the echo still has a beat to sit on: the
    /// counted one, the same fallback a loop takes.
    #[test]
    fn a_deck_without_a_grid_echoes_at_the_counted_beat() {
        let rate = 48_000.0;
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, click_pcm(1.0, 48_000, 6.0));
        mixer.set_deck_keylock(DeckId::A, false);
        mixer.set_deck_echo(DeckId::A, Some((1, 2)));
        mixer.set_deck_playing(DeckId::A, true);
        let out = render_out(&mixer, rate, 375, 512); // ~4 s
        // 60 / COUNTED_BPM = 1 s a beat; half of that is 24 000 frames.
        let (peak, offset) = peak_near(&out, rate, 1.0 + 24_000.0 / rate, 4);
        assert!(peak > 0.3, "{peak}");
        assert!(offset.abs() <= 4, "{offset}");
    }

    /// The chain is a data-layout change, not a DSP one: driven with the
    /// same parameters as today's hand-nested
    /// `echo.process(freeze.process(eq.process(frame, rate), rate), rate)`,
    /// its output must be bit-for-bit identical, every frame. A resonance
    /// rung, a band boost, a running ping-pong echo and a held freeze are
    /// all engaged first, so this exercises every unit's non-bypass path
    /// and not just the identity fast path they all also have.
    #[test]
    fn deck_chain_matches_todays_hand_chained_order() {
        let rate = 48_000.0f32;
        let mut chain = DeckChain::new(rate);
        let mut eq = DeckEq::new(rate);
        let mut freeze = Freeze::new();
        let mut echo = DeckEcho::new();

        chain.eq_mut().set_resonance(DeckEq::RESONANCE_RUNGS[1]);
        eq.set_resonance(DeckEq::RESONANCE_RUNGS[1]);
        chain.eq_mut().set_band(0, 1.4);
        eq.set_band(0, 1.4);
        chain.echo_mut().set_fraction(Some((1, 2)));
        echo.set_fraction(Some((1, 2)));
        chain.echo_mut().set_feedback(0.6);
        echo.set_feedback(0.6);
        chain.echo_mut().set_pingpong(true);
        echo.set_pingpong(true);

        let beat_frames = 0.5 * rate as f64;
        for buffer in 0..8u32 {
            chain.eq_mut().set_sample_rate(rate);
            chain.eq_mut().prepare_block();
            eq.set_sample_rate(rate);
            eq.prepare_block();
            chain.echo_mut().prepare_block(beat_frames);
            echo.prepare_block(beat_frames);

            // Freeze engages partway through, identically on both sides,
            // once there is real content behind it to hold.
            if buffer == 4 {
                chain.freeze_mut().hold(4096, rate);
                freeze.hold(4096, rate);
            }

            for i in 0..512u32 {
                let n = (buffer * 512 + i) as f32;
                // Detuned, non-integer-cycle-count tone: a period that
                // divides evenly into the buffer or beat length would make
                // a divergence in slot order numerically invisible.
                let s = (n * 443.0 / rate * std::f32::consts::TAU).sin() * 0.4;
                let frame = [s, s * 0.8];

                let want = echo.process(freeze.process(eq.process(frame, rate), rate), rate);
                let got = chain.process(frame, rate);
                assert_eq!(got, want, "buffer {buffer} frame {i}");
            }
        }
    }

    /// A chain with nothing engaged steps out of the walk, and the chain
    /// that stepped aside is the chain that walked: the same samples out
    /// and the same state, bit for bit. Two chains fed the same signal,
    /// one forbidden to skip; then a compressor engaged (which starts its
    /// detector where the follower says the music is -- the follower an
    /// idle chain has to keep feeding), an echo, a freeze held (which
    /// latches what the ring recorded -- the ring an idle chain has to
    /// keep feeding), everything released, and a long enough silence for
    /// the tails to ring out and the chain to idle again.
    #[test]
    fn a_chain_that_steps_aside_is_the_chain_that_walked_bit_for_bit() {
        let rate = 48_000.0f32;
        let clock = DeckClock::default();
        let mut walked = DeckChain::new(rate);
        walked.set_skip_when_idle(false);
        let mut stepped = DeckChain::new(rate);
        let (mut idle_before, mut idle_after) = (0u32, 0u32);
        let buffers = 400u32;
        for buffer in 0..buffers {
            // Each of the two hand-fed units is engaged FROM idle, with the
            // chain back at idle in between: an engage that follows a
            // walk would have been fed by the walk, and prove nothing.
            for chain in [&mut walked, &mut stepped] {
                match buffer {
                    40 => MixEngine::apply_effect(chain, EffectParam::Compressor(true)),
                    60 => MixEngine::apply_effect(chain, EffectParam::Compressor(false)),
                    100 => chain.freeze_mut().hold(4_096, rate),
                    // A short rung: the tail has to ring out inside the
                    // run, and at 0.55 feedback that is eighteen periods.
                    120 => MixEngine::apply_effect(chain, EffectParam::Echo(Some((1, 16)))),
                    140 => {
                        MixEngine::apply_effect(chain, EffectParam::Echo(None));
                        chain.freeze_mut().release();
                    }
                    _ => {}
                }
                chain.prepare_block(&clock, rate, 512);
            }
            assert_eq!(walked.idle(), stepped.idle(), "buffer {buffer}: the verdicts differ");
            if buffer == 39 || buffer == 99 {
                assert!(stepped.idle(), "buffer {buffer}: the engage that follows must come from idle");
            }
            if stepped.idle() {
                if buffer < 40 { idle_before += 1 } else if buffer > 140 { idle_after += 1 }
            }
            for i in 0..512u32 {
                let n = (buffer * 512 + i) as f32;
                let s = (n * 443.0 / rate * std::f32::consts::TAU).sin() * 0.4;
                let frame = [s, s * 0.8];
                let want = walked.process(frame, rate);
                let got = stepped.process(frame, rate);
                assert_eq!(
                    got.map(f32::to_bits),
                    want.map(f32::to_bits),
                    "buffer {buffer} frame {i}: {got:?} stepped aside, {want:?} walked"
                );
            }
        }
        assert_eq!(idle_before, 40, "an untouched chain steps aside from the first buffer");
        assert!(!walked.idle() || idle_after > 0, "and idles again once the tails have rung out");
        assert!(idle_after > 0, "the chain never idled again after the release");
    }

    /// Not a test: a report. What the idle chains cost the frame loop with
    /// the step-aside and without it, in nanoseconds per rendered frame,
    /// on a rig with one deck playing and every other chain idle. Run it
    /// with `--ignored --nocapture`; the figure that matters is the share
    /// on the console strip of a release build, and this is the check
    /// that the two move together.
    #[test]
    #[ignore]
    fn chain_cost_report() {
        for skip in [true, false, true, false] {
            let mixer = TestMixer::new();
            mixer.set_master(1.0);
            mixer.install_deck(DeckId::A, tone_pcm(443.0, 48_000, 10.0));
            mixer.set_deck_playing(DeckId::A, true);
            mixer.state().set_skip_when_idle(skip);
            render(&mixer, 48_000.0, 4_096);
            let buffers = 400u64;
            let mut mix_nanos = 0u64;
            for _ in 0..buffers {
                render(&mixer, 48_000.0, 512);
                mix_nanos += mixer.audio_health().stages.mix;
            }
            println!(
                "step aside {skip}: {:.1} ns per frame in the frame loop",
                mix_nanos as f64 / (buffers * 512) as f64
            );
        }
    }

    #[test]
    fn releasing_the_vocal_duck_is_click_free() {
        // The EQ medium holds the incoming mid band down at prep and lets it
        // go part-way through the fade, by which time that deck is audible.
        // A gain move on a live strip is exactly where a click comes from,
        // so the release has to glide.
        //
        // The signal has to live in the band under test — a flat one is all
        // low band and would sail through this while hearing nothing — so a
        // tone carries a step of its own, and the control block is what
        // separates that from the move.
        let settle = |mixer: &TestMixer| {
            mixer.set_master(1.0);
            mixer.install_deck(DeckId::A, tone_pcm(800.0, 48_000, 10.0));
            mixer.set_crossfader(0.0);
            mixer.set_deck_playing(DeckId::A, true);
        };

        // The control sits at the gain the release LANDS on: a tone twice as
        // loud steps twice as far all by itself, and comparing against the
        // ducked block would read that as a click.
        let control = TestMixer::new();
        settle(&control);
        render(&control, 48_000.0, 8192);
        let untouched = worst_adjacent_step(render(&control, 48_000.0, 8192).channel(0));

        let mixer = TestMixer::new();
        settle(&mixer);
        mixer.set_blend_band(DeckId::A, 1, crate::blend::EQ_VOCAL_DUCK);
        render(&mixer, 48_000.0, 8192); // let the duck seat
        mixer.set_blend_band(DeckId::A, 1, 1.0);
        let out = render(&mixer, 48_000.0, 8192);
        let released = worst_adjacent_step(out.channel(0));

        assert!(
            released < untouched * 1.5,
            "the release must glide: {released} against {untouched} standing still"
        );
        // And it really did travel: a release that never moved would pass
        // this for the wrong reason.
        let quietest = out.channel(0).iter().copied().fold(f32::MAX, f32::min);
        let loudest = out.channel(0).iter().copied().fold(f32::MIN, f32::max);
        assert!(
            loudest - quietest > 0.05,
            "the mid band should climb back over the block, {quietest}..{loudest}"
        );
    }

    #[test]
    fn sweeping_the_blend_filter_is_click_free() {
        // A recipe steps the filter offset rather than dragging it, and
        // every step rebuilds the sweep coefficients. Whether that lands a
        // click is a measurement, not an opinion.
        let settle = |mixer: &TestMixer| {
            mixer.set_master(1.0);
            mixer.install_deck(DeckId::A, tone_pcm(800.0, 48_000, 10.0));
            mixer.set_crossfader(0.0);
            mixer.set_deck_playing(DeckId::A, true);
        };

        let control = TestMixer::new();
        settle(&control);
        render(&control, 48_000.0, 8192);
        let untouched = worst_adjacent_step(render(&control, 48_000.0, 8192).channel(0));

        let mixer = TestMixer::new();
        settle(&mixer);
        render(&mixer, 48_000.0, 8192);
        // A quarter of the sweep: the size a recipe would step.
        mixer.set_blend_filter(DeckId::A, -0.25);
        let out = render(&mixer, 48_000.0, 8192);
        let swept = worst_adjacent_step(out.channel(0));
        assert!(
            swept < untouched * 2.0,
            "a sweep step must not crack: {swept} against {untouched} standing still"
        );
    }

    /// PFL hears the echo along with everything else on the strip; RAW,
    /// which taps ahead of the tone chain, does not.
    #[test]
    fn the_pfl_tap_hears_the_echo_and_the_raw_tap_does_not() {
        let rate = 48_000.0;
        let cue_energy = |mode: CueMode| -> f64 {
            let mixer = TestMixer::new();
            mixer.set_master(1.0);
            mixer.set_crossfader(0.0);
            mixer.install_deck(DeckId::A, click_pcm(0.2, 48_000, 3.0));
            mixer.set_deck_keylock(DeckId::A, false);
            mixer.set_deck_grid(DeckId::A, Some(clock_grid(120.0)));
            // A quarter beat and a hot feedback: many repeats inside a
            // short render, so their sum comfortably outweighs the one
            // dry click both taps carry.
            mixer.set_deck_echo(DeckId::A, Some((1, 4)));
            mixer.set_deck_echo_feedback(DeckId::A, 0.9);
            mixer.set_deck_playing(DeckId::A, true);
            mixer.set_cue_armed(true);
            mixer.set_deck_cue(DeckId::A, true);
            mixer.set_cue_mode(mode);
            let mut state = CueReadState::default();
            let mut sum = 0.0f64;
            // Integrated over the whole span the way the existing PFL/RAW
            // test does, so a lagged consumer cannot misalign a narrow
            // window against it.
            for _ in 0..200 {
                render(&mixer, rate, 512);
                let out = consume_cue(&mixer, &mut state, rate, 512);
                for v in out.channel(0) {
                    sum += (*v as f64) * (*v as f64);
                }
            }
            sum
        };
        let pfl = cue_energy(CueMode::Pfl);
        let raw = cue_energy(CueMode::Raw);
        assert!(pfl > raw * 1.8, "PFL must hear the repeats RAW skips: pfl={pfl} raw={raw}");
    }

    #[test]
    fn a_rendered_buffer_reports_what_it_cost_and_how_long_it_was() {
        // A lifetime worst tells an operator nothing about the machine they
        // are on right now: one stall while the app was starting pins it
        // for the session. The budget wants the LAST buffer's cost against
        // that buffer's own length.
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 512);
        let health = mixer.audio_health();
        assert_eq!(health.buffer_frames, 512, "the buffer that was just rendered");
        assert!(health.render_nanos > 0, "and what it cost");
        assert_eq!(health.device_rate, 48_000.0);
        assert!(health.render_max_nanos >= health.render_nanos, "the worst is still kept");
        render(&mixer, 48_000.0, 256);
        assert_eq!(mixer.audio_health().buffer_frames, 256, "the LAST buffer, not the worst");
    }

    #[test]
    fn a_budget_is_forgotten_with_the_device_that_set_it() {
        // The budget is the last buffer's cost against that buffer's own
        // length. With no output device left there is no buffer, and the
        // console's line kept quoting the one a gone device rendered last.
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 512);
        assert!(mixer.audio_health().budget_used().is_some(), "a rendered buffer has a budget");
        mixer.forget_device();
        assert_eq!(mixer.audio_health().budget_used(), None, "no device, no budget to quote");
        render(&mixer, 48_000.0, 512);
        assert!(mixer.audio_health().budget_used().is_some(), "and the next device's first buffer sets it again");
    }

    #[test]
    fn contended_target_is_reported_missed_and_never_started_late() {
        // The mutex this used to hold is gone -- `Mixer` is just a command
        // ring and a UI shadow (`handle_and_engine_cross_threads_without_a_mutex`
        // proves neither carries a lock), and `AudioHealth::contended` is
        // hardcoded to 0 for the same reason: drain-then-render is one
        // atomic step on the engine's own thread, so a schedule can no
        // longer be silently skipped past its target through the public
        // API -- `apply` re-clamps `target_frame` to the engine's own
        // device clock the instant it is drained, and every frame from
        // then on is walked by exactly one render call.
        //
        // What survives is the Missed branch itself in the frame loop,
        // kept for a schedule that could still go stale between threads
        // in production. Reach it the way this module already reaches
        // other engine-private state (`mixer.state()`): arm a transition
        // for real (so it carries a genuine id), let an ordinary render
        // move the device clock past it, then back-date the still-armed
        // schedule's target the way a lost buffer used to.
        let mixer = TestMixer::new();
        mixer.open_slot(SlotId::A);
        assert!(mixer.push_slot_audio(SlotId::A, &vec![16_384; 2 * 512], 2, 48_000));
        let before = mixer.slot_buffered_secs(SlotId::A);
        mixer.schedule_video_transition_after(88, None, SlotId::A, 1_000, 16).unwrap();
        mixer.sync();
        render(&mixer, 48_000.0, 8);
        mixer.state().scheduled_video.as_mut().expect("still armed, not yet due").target_frame = 4;
        render(&mixer, 48_000.0, 1);
        let snapshot = mixer.video_transition_snapshot().unwrap();
        assert_eq!(snapshot.phase, VideoTransitionPhase::Missed);
        assert!((mixer.slot_buffered_secs(SlotId::A) - before).abs() < 1e-9);
    }

    #[test]
    fn deck_snapshot_reads_without_the_mixer_lock() {
        // There is no more `state` lock for a thread to hold -- `Mixer` is
        // `ui: Arc<UiCell<UiShadow>>` plus `shared: Arc<Shared>`, both Sync
        // with no Mutex in sight (`handle_and_engine_cross_threads_without_a_mutex`
        // proves it at the type level) -- so the old holder thread has
        // nothing left to grab. What is still worth proving is the other
        // half of the old promise: a snapshot answers a just-issued seek
        // from a cheap `sync()` (drain the ring, republish), never from
        // running the actual DSP frame loop.
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, const_pcm(16_384, 48_000, 48_000));
        mixer.seek_deck_seconds(DeckId::A, 0.5);
        let began = std::time::Instant::now();
        let snapshot = mixer.deck_snapshot(DeckId::A);
        assert!(
            began.elapsed() < std::time::Duration::from_millis(100),
            "the snapshot must not wait on anything"
        );
        assert!((snapshot.position_secs - 0.5).abs() < 1e-9);
    }

    /// The loudest sample on the left channel.
    fn peak_of(buffer: &AudioBuffer) -> f32 {
        (0..buffer.frame_count()).map(|f| buffer.channel(0)[f].abs()).fold(0.0, f32::max)
    }

    #[test]
    fn the_lanes_fade_in_over_the_mixed_file_rather_than_cutting_to_it() {
        // A separation lands chunk by chunk while the record plays, so the
        // instant the frontier reaches the playhead the source used to flip
        // on whatever sample the pump delivered it on. The lane sum is
        // close to the mixed file but not identical, and a hard swap
        // between them is heard on the phase difference.
        // The mixed file is silence and the lanes are loud, so the output
        // IS the weight: if it jumped, the swap was a cut.
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, const_pcm(0, 48_000 * 4, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4096);
        mixer.install_deck_stems(
            DeckId::A,
            chunked_stems([8_000.0, 0.0, 0.0, 0.0], 48_000, 4.0),
        );
        let first = peak_of(&render(&mixer, 48_000.0, 512));
        for _ in 0..24 {
            render(&mixer, 48_000.0, 512);
        }
        let settled = peak_of(&render(&mixer, 48_000.0, 512));
        assert!(settled > 0.05, "the lanes did arrive: {settled}");
        assert!(
            first < settled * 0.5,
            "the lanes cut in instead of fading: {first} against a settled {settled}"
        );
    }

    /// Lanes that are present but silent: the harshest possible swap away
    /// from the mixed file, which is what makes it a good seam test.
    fn silent_stems(rate: u32, seconds: f64) -> Arc<TrackStems> {
        let chunk = rate as usize;
        let count = (rate as f64 * seconds / chunk as f64).ceil() as usize;
        let mut stems = TrackStems::new(chunk, count.max(1));
        for lane in stems.lanes.iter_mut() {
            for slot in lane.iter_mut() {
                *slot = Some(Arc::new(vec![[0i16; 2]; chunk]));
            }
        }
        Arc::new(stems)
    }

    #[test]
    fn dropping_the_lanes_takes_the_weight_with_them() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, tone_pcm(220.0, 48_000, 4.0));
        mixer.install_deck_stems(DeckId::A, silent_stems(48_000, 4.0));
        mixer.set_deck_playing(DeckId::A, true);
        for _ in 0..16 {
            render(&mixer, 48_000.0, 512);
        }
        mixer.clear_deck_stems(DeckId::A);
        // Straight back to the mixed file: there is no lane left to fade
        // out of.
        let out = render(&mixer, 48_000.0, 512);
        assert!(
            out.data.iter().any(|s| s.abs() > 0.01),
            "the mixed file is playing again at once"
        );
    }

    #[test]
    fn a_load_onto_a_silent_deck_is_still_a_cut() {
        // Why every existing golden is untouched: on a deck at rest the
        // install happens on this thread, this instant, exactly as before.
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, const_pcm(8_000, 480_000, 48_000));
        assert!((mixer.deck_snapshot(DeckId::A).duration_secs - 10.0).abs() < 1e-6);
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4_096);
        mixer.set_deck_playing(DeckId::A, false);
        // Past the pause fade the deck is silent again, so it cuts again.
        render(&mixer, 48_000.0, 4_096);
        mixer.install_deck(DeckId::A, const_pcm(8_000, 240_000, 48_000));
        assert!((mixer.deck_snapshot(DeckId::A).duration_secs - 5.0).abs() < 1e-6);
    }

    #[test]
    fn a_load_over_a_playing_deck_hands_the_old_track_back_off_the_callback() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        let outgoing = const_pcm(8_000, 480_000, 48_000);
        mixer.install_deck(DeckId::A, outgoing.clone());
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4_096);

        mixer.install_deck_over(DeckId::A, const_pcm(4_000, 192_000, 48_000), false);
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }
        // The callback moved the finished track out of the voice and onto
        // the retire event ring; it did NOT free it. Freeing a decoded
        // track is an unbounded free and the audio thread does not do
        // those.
        assert_eq!(Arc::strong_count(&outgoing), 2, "the mixer is still holding it");
        mixer.reap_retired();
        assert_eq!(Arc::strong_count(&outgoing), 1, "and this is the thread that drops it");
    }

    /// A load over a playing deck takes the parked path: the OLD track
    /// keeps sounding while it fades, and the swap itself lands later,
    /// on the audio thread's own turn. The echo is the strip's, not
    /// either record's, and neither side of that swap may drop it.
    #[test]
    fn a_load_over_a_playing_deck_keeps_the_operators_echo_setting() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, tone_pcm(220.0, 48_000, 8.0));
        mixer.set_deck_playing(DeckId::A, true);
        mixer.set_deck_echo(DeckId::A, Some((1, 2)));
        assert!(mixer.state().decks[DeckId::A.index()].chain.echo_mut().engaged());
        // Audible, so this is the parked path, not the immediate cut.
        mixer.install_deck_over(DeckId::A, tone_pcm(220.0, 48_000, 8.0), true);
        assert!(
            mixer.state().decks[DeckId::A.index()].chain.echo_mut().engaged(),
            "still parked -- nothing has moved yet either way"
        );
        // Render past the outgoing track's short fade and the swap that
        // spends the parked load, on the audio thread's own next turn.
        spin_render(&mixer, 32);
        assert!(
            mixer.state().decks[DeckId::A.index()].chain.echo_mut().engaged(),
            "the swap landed, and the operator's rung must have survived it"
        );
    }

    /// The engine's OWN idea of what a deck is playing, in seconds --
    /// unlike `deck_snapshot`'s duration_secs (which mirrors the UI
    /// shadow, updated the instant install_deck_over is called), this
    /// is untouched by a parked load until `seat_record` actually
    /// spends it.
    fn deck_engine_duration(mixer: &TestMixer, deck: DeckId) -> f64 {
        let state = mixer.state();
        state.decks[deck.index()].pcm.as_ref().unwrap().expected_seconds()
    }

    #[test]
    fn a_load_over_a_playing_deck_waits_for_its_fade_before_the_swap() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(8_000, 480_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4_096);

        mixer.install_deck_over(DeckId::A, const_pcm(4_000, 192_000, 48_000), false);
        // deck_snapshot's duration_secs now comes straight off the UI
        // shadow, which install_deck_over updates the instant it is
        // called (mixer.rs:3302-3316) -- before the engine has even seen
        // the command, let alone decided cut vs. park. So it no longer
        // proves "nothing has moved yet" the way it did on the old
        // engine; the engine's own pcm does, since a parked load leaves
        // it untouched until seat_record spends the fade
        // (mixer.rs:4724-4746, 4378-4400).
        assert!(
            (deck_engine_duration(&mixer, DeckId::A) - 10.0).abs() < 1e-6,
            "nothing has moved yet"
        );
        render(&mixer, 48_000.0, 512);
        assert!(
            (deck_engine_duration(&mixer, DeckId::A) - 10.0).abs() < 1e-6,
            "still fading the old track out"
        );

        // Past the fade plus a buffer, the swap has landed.
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }
        let snap = mixer.deck_snapshot(DeckId::A);
        assert!((snap.duration_secs - 4.0).abs() < 1e-6, "the new track is on");
        assert!(snap.position_secs.abs() < 1e-6, "at its top");
        assert!(!snap.playing, "and stopped, because the policy said so");
    }

    #[test]
    fn a_load_that_keeps_the_deck_running_comes_back_up_on_the_new_track() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4_096);

        mixer.install_deck_over(DeckId::A, const_pcm(8_192, 480_000, 48_000), true);
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }
        let snap = mixer.deck_snapshot(DeckId::A);
        assert!(snap.playing, "the deck never stopped");
        let before = snap.position_secs;
        // Let the transport climb back to unity, then read the level.
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }
        assert!(mixer.deck_snapshot(DeckId::A).position_secs > before, "and it is running");
        let block = render(&mixer, 48_000.0, 512);
        let level = block.channel(0)[511].abs();
        let want = 8_192.0 / 32_768.0;
        assert!((level - want).abs() < 0.01, "the SECOND track's level, got {level}");
    }

    #[test]
    fn a_second_load_inside_the_fade_takes_the_later_track() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        let first = const_pcm(8_000, 480_000, 48_000);
        mixer.install_deck(DeckId::A, first.clone());
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4_096);

        let never = const_pcm(4_000, 192_000, 48_000);
        mixer.install_deck_over(DeckId::A, never.clone(), false);
        render(&mixer, 48_000.0, 512);
        mixer.install_deck_over(DeckId::A, const_pcm(2_000, 96_000, 48_000), false);
        // Unlike the old lock-protected engine, the take-and-retire of a
        // superseded parked load only happens once the audio thread
        // (here, sync()) actually dispatches the second InstallOver --
        // not synchronously on this thread as it was on OLD -- and even
        // then the displaced pcm only reaches the retire event ring;
        // reap_retired() (pump) is what actually drops it, same as it
        // always was for a spent (not superseded) pending load.
        mixer.sync();
        mixer.reap_retired();
        assert_eq!(Arc::strong_count(&never), 1, "the load that never got its turn");
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }
        assert!((mixer.deck_snapshot(DeckId::A).duration_secs - 2.0).abs() < 1e-6);
        assert!(
            (deck_engine_duration(&mixer, DeckId::A) - 2.0).abs() < 1e-6,
            "the engine itself swapped onto the later track, not just the UI shadow"
        );
        assert_eq!(Arc::strong_count(&first), 2, "the original is waiting to be reaped");
        mixer.reap_retired();
        assert_eq!(Arc::strong_count(&first), 1);
    }


    #[test]
    fn every_callback_arms_flush_to_zero() {
        // Some hosts reset the flag behind the app's back between buffers,
        // so the callback cannot arm it once and trust it: each render
        // re-arms. Disarm here as such a host would, render one buffer, and
        // the thread must be flushing again.
        crate::music_dsp::set_flush_denormals(false);
        let mixer = TestMixer::new();
        let mut buffer = AudioBuffer::new_with_size(64, 2);
        mixer.render(48_000.0, &mut buffer);
        let tiny = std::hint::black_box(f32::MIN_POSITIVE);
        let half = std::hint::black_box(0.5f32);
        assert_eq!(tiny * half, 0.0, "a callback must leave flush-to-zero armed");
        crate::music_dsp::set_flush_denormals(false);
    }


    #[test]
    fn a_deck_handed_a_number_that_is_not_one_keeps_playing() {
        // One bad number out of a UI division by a zero-width widget, or a
        // learned controller scale, used to take a deck out for the night.
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4096);
        let before = render(&mixer, 48_000.0, 256).channel(0)[128];
        assert!(before.abs() > 0.1, "the deck is sounding to begin with");
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            mixer.set_deck_gain(DeckId::A, bad);
            mixer.set_deck_eq_band(DeckId::A, 1, bad);
            mixer.set_deck_filter(DeckId::A, bad);
            mixer.set_master(bad);
            mixer.set_crossfader(bad);
            mixer.set_deck_stem_gain(DeckId::A, 0, bad);
            mixer.set_deck_rate(DeckId::A, bad as f64);
            mixer.set_deck_key_shift(DeckId::A, bad as f64);
        }
        let out = render(&mixer, 48_000.0, 1024);
        assert!(
            out.channel(0).iter().chain(out.channel(1)).all(|s| s.is_finite()),
            "the output stays finite"
        );
        assert!(
            out.channel(0)[512].abs() > 0.1,
            "and the deck is still sounding, at {}",
            out.channel(0)[512]
        );
    }


    #[test]
    fn the_phones_never_carry_a_sample_that_is_not_a_number() {
        // The master sum is guarded at the mix point; the cue sum is the
        // other bus out of this callback, and it reaches an operator's ears
        // directly. A filter driven past stability on a cued deck must cost
        // that deck, not the monitor for the rest of the night.
        let ring = CueRing::new();
        ring.armed.store(true, Ordering::Relaxed);
        ring.main_rate_bits.store(48_000f64.to_bits(), Ordering::Relaxed);
        let filled = CUE_TARGET_FRAMES as u64 + 2_048;
        for pos in 0..filled {
            let bad = pos % 37 == 0;
            let (l, r) = if bad { (f32::NAN, f32::INFINITY) } else { (0.5, -0.5) };
            ring.push(pos, l, r);
        }
        ring.write_pos.store(filled, Ordering::Release);
        let mut state = CueReadState::default();
        let mut out = AudioBuffer::new_with_size(1_024, 2);
        ring.consume(&mut state, 48_000.0, &mut out);
        assert!(
            out.channel(0).iter().chain(out.channel(1)).all(|s| s.is_finite()),
            "every phones sample must be one the device can carry"
        );
        assert!(
            out.channel(0).iter().any(|s| s.abs() > 0.01),
            "and the good samples still get through"
        );
    }


    /// The frame loop is timed in three phases so a climbing budget can
    /// say whether it is the mixing or the per-buffer overhead around it.
    #[test]
    fn a_callback_says_where_its_time_went_and_the_phases_add_up() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, const_pcm(16_384, 48_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 512);
        let health = mixer.audio_health();
        let stages = health.stages;
        assert!(stages.mix > 0, "the frame loop is nearly all of it");
        let summed = stages.setup + stages.mix + stages.publish;
        assert_eq!(
            summed, health.render_nanos,
            "the three phases ARE the callback, with nothing unaccounted for"
        );
        assert!(
            stages.mix > stages.setup,
            "mixing {} should outweigh the setup {} for a playing deck",
            stages.mix,
            stages.setup
        );
    }

    /// The line between coping and not: a render that takes exactly the
    /// buffer's playing time has coped, one nanosecond more has not.
    #[test]
    fn a_render_outruns_its_buffer_one_nanosecond_past_its_playing_time() {
        // 512 frames at 48 kHz play for 10 666 666 ns.
        assert!(!overran(10_666_666, 512, 48_000.0));
        assert!(overran(10_666_667, 512, 48_000.0));
        assert!(!overran(1_000, 512, 48_000.0));
        assert!(overran(20_000_000, 512, 48_000.0));
    }

    /// The count reaches the console's health, and it counts: at a device
    /// rate of a gigahertz a 64-frame buffer plays for 64 ns, which no
    /// render finishes inside, so every buffer is an overrun.
    #[test]
    fn a_render_that_outran_its_buffer_is_counted_where_the_console_reads() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, const_pcm(16_384, 48_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        assert_eq!(mixer.audio_health().overruns, 0, "nothing rendered yet");
        render(&mixer, 1.0e9, 64);
        render(&mixer, 1.0e9, 64);
        let health = mixer.audio_health();
        assert!(health.overruns >= 2, "two impossible buffers, {} counted", health.overruns);
        assert_eq!(health.overruns, mixer.audio_overruns(), "one counter, two readers");
    }

    #[test]
    fn the_phones_say_when_they_have_run_dry() {
        // A programme dropout is counted; a monitor dropout was invisible,
        // which is the one an operator hears first and can least explain.
        let mixer = TestMixer::new();
        let ring = mixer.cue_ring();
        mixer.set_cue_armed(true);
        ring.main_rate_bits.store(48_000f64.to_bits(), Ordering::Relaxed);
        let mut state = CueReadState::default();
        let mut out = AudioBuffer::new_with_size(256, 2);
        // Priming with nothing in the ring is not starvation: it is the
        // monitor waiting to start.
        ring.consume(&mut state, 48_000.0, &mut out);
        assert_eq!(mixer.audio_health().phones_starved, 0, "priming is not a dropout");
        // Now fill it and let the monitor start.
        let filled = CUE_TARGET_FRAMES + 64;
        for pos in 0..filled {
            ring.push(pos, 0.5, 0.5);
        }
        ring.write_pos.store(filled, Ordering::Release);
        ring.consume(&mut state, 48_000.0, &mut out);
        assert_eq!(mixer.audio_health().phones_starved, 0, "a fed monitor is quiet about it");
        // The programme device stalls: nothing more is pushed, and the
        // phones device keeps asking until it has drained the ring.
        for _ in 0..16 {
            ring.consume(&mut state, 48_000.0, &mut out);
        }
        assert!(
            mixer.audio_health().phones_starved >= 1,
            "running out mid-buffer is a dropout, and is counted"
        );
    }


    /// The callback works the clock out once per buffer and the snapshot
    /// carries it: the beat's length at the platter's speed, and the
    /// fraction predicted for the END of the buffer, which is exactly
    /// where the playhead then is.
    #[test]
    fn a_deck_publishes_its_clock_from_the_callback() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, tone_pcm(220.0, 48_000, 8.0));
        assert!(!mixer.deck_snapshot(DeckId::A).clock.has_grid, "no grid yet");
        let grid = clock_grid(120.0);
        mixer.set_deck_grid(DeckId::A, Some(grid));
        let fresh = mixer.deck_snapshot(DeckId::A).clock;
        assert!(fresh.has_grid, "the setter republishes");
        // No callback has run yet, so the setter has to work out the
        // platter itself rather than read the last published one -- which
        // is still `DeckClock::default()`'s zero, indistinguishable from
        // a stopped record. A load-then-set-grid in one tick is exactly
        // the sequence a real load takes.
        assert_eq!(
            fresh.beat_len(),
            Some(0.5),
            "before any buffer has rendered, {fresh:?}"
        );
        mixer.set_deck_playing(DeckId::A, true);
        spin_render(&mixer, 8);
        render(&mixer, 48_000.0, 512);
        let snap = mixer.deck_snapshot(DeckId::A);
        assert!(snap.clock.has_grid);
        assert!((snap.clock.beat_secs_out - 0.5).abs() < 1e-9, "{}", snap.clock.beat_secs_out);
        assert_eq!(snap.clock.beat_len(), Some(snap.clock.beat_secs_out));
        assert_eq!(snap.clock.platter_rate, 1.0);
        let arrived = grid.phase_at(snap.position_secs);
        assert!(
            (snap.clock.beat_frac_end - arrived).abs() < 1e-6,
            "predicted {} for the buffer's end, the head arrived at {arrived}",
            snap.clock.beat_frac_end
        );
    }

    /// The engine sends `true_grid`, but the mixer holds the line too: a
    /// grid with no beats, or none at all, is no clock.
    #[test]
    fn a_grid_with_no_beats_is_no_grid_to_the_callback() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, tone_pcm(220.0, 48_000, 8.0));
        mixer.set_deck_playing(DeckId::A, true);
        mixer.set_deck_grid(DeckId::A, Some(TrackGrid::default()));
        spin_render(&mixer, 4);
        let clock = mixer.deck_snapshot(DeckId::A).clock;
        assert!(!clock.has_grid);
        assert_eq!(clock.beat_len(), None);
        assert_eq!(clock.platter_rate, 1.0, "the platter is still reported");
        mixer.set_deck_grid(DeckId::A, Some(clock_grid(120.0)));
        spin_render(&mixer, 4);
        assert!(mixer.deck_snapshot(DeckId::A).clock.has_grid);
        mixer.set_deck_grid(DeckId::A, None);
        spin_render(&mixer, 4);
        assert!(!mixer.deck_snapshot(DeckId::A).clock.has_grid);
    }

    /// Pause keeps the TEMPO -- the beat is still half a second long, so
    /// an echo set to a beat does not collapse -- and promises no travel,
    /// so the fraction is for where the head IS.
    #[test]
    fn a_paused_deck_keeps_its_tempo_and_does_not_travel() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, tone_pcm(220.0, 48_000, 8.0));
        let grid = clock_grid(120.0);
        mixer.set_deck_grid(DeckId::A, Some(grid));
        mixer.set_deck_playing(DeckId::A, true);
        spin_render(&mixer, 8);
        mixer.set_deck_playing(DeckId::A, false);
        spin_render(&mixer, 64);
        let snap = mixer.deck_snapshot(DeckId::A);
        assert!(!snap.playing);
        assert!(snap.clock.has_grid);
        assert!((snap.clock.beat_secs_out - 0.5).abs() < 1e-9, "{}", snap.clock.beat_secs_out);
        assert_eq!(snap.clock.beat_frac_end, grid.phase_at(snap.position_secs));
    }

    /// A running splat advances at the record's own speed whatever the
    /// tempo fader says, so its clock reports the platter at exactly one
    /// -- and hands the fader back the moment the splat stops.
    /// A running splat's own read path exits on `!playing` alone -- not
    /// the four exits an ordinary deck takes -- so the clock has to be
    /// asked separately. Missing that, a pause under a splat kept
    /// promising travel for the length of its fade and any buffer with a
    /// hand held on a paused splat deck promised it forever.
    #[test]
    fn a_paused_splat_deck_does_not_travel_either() {
        let (mixer, rate) = splat_fixture(false);
        let grid = clock_grid(120.0);
        mixer.set_deck_grid(DeckId::A, Some(grid));
        render_count(&mixer, rate, 4096, 256);
        mixer.set_deck_playing(DeckId::A, false);
        let before = mixer.deck_snapshot(DeckId::A).position_secs;
        render_count(&mixer, rate, 256, 256);
        let after = mixer.deck_snapshot(DeckId::A);
        assert_eq!(after.position_secs, before, "a paused splat's master does not move");
        assert_eq!(
            after.clock.beat_frac_end,
            grid.phase_at(after.position_secs),
            "and the clock must not predict a buffer of travel the head never made"
        );
        // A hand on the very deck the splat has parked keeps `scratch`
        // active for as long as it is held; that must not revive travel
        // either, because the splat -- not the hand -- owns the read.
        mixer.scratch_deck(DeckId::A, ScratchMotion::Grab);
        mixer.scratch_deck(DeckId::A, ScratchMotion::Move { secs: 0.01, rate: 1.0 });
        render_count(&mixer, rate, 256, 256);
        let held = mixer.deck_snapshot(DeckId::A);
        assert_eq!(held.position_secs, before, "the splat still owns the master, hand or no hand");
        assert_eq!(held.clock.beat_frac_end, grid.phase_at(held.position_secs));
    }

    #[test]
    fn a_running_splat_keeps_the_records_own_tempo() {
        let (mixer, rate) = splat_fixture(false);
        mixer.set_deck_grid(DeckId::A, Some(clock_grid(120.0)));
        mixer.set_deck_rate(DeckId::A, 1.25);
        render_count(&mixer, rate, 4096, 256);
        let clock = mixer.deck_snapshot(DeckId::A).clock;
        assert_eq!(clock.platter_rate, 1.0, "the splat owns the platter");
        assert!((clock.beat_secs_out - 0.5).abs() < 1e-9, "{}", clock.beat_secs_out);
        mixer.set_deck_splat_enabled(DeckId::A, false);
        render_count(&mixer, rate, 4096, 256);
        let clock = mixer.deck_snapshot(DeckId::A).clock;
        assert!((clock.platter_rate - 1.25).abs() < 1e-6, "{}", clock.platter_rate);
        assert!((clock.beat_secs_out - 0.4).abs() < 1e-6, "{}", clock.beat_secs_out);
    }

    /// The read path folds the playhead into an active span before every
    /// read; the clock's single-shot prediction now folds the same
    /// travel the same way, so a lap that wraps mid-buffer does not
    /// publish a fraction for a beat position the head is about to
    /// leave behind.
    #[test]
    fn the_clock_folds_a_loop_wrap_the_way_the_read_path_does() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, tone_pcm(220.0, 48_000, 8.0));
        let grid = clock_grid(120.0);
        mixer.set_deck_grid(DeckId::A, Some(grid));
        mixer.set_deck_playing(DeckId::A, true);
        // A quarter-beat span: not a whole number of beats, so a wrap
        // that ignored it would land on the wrong beat fraction.
        mixer.set_deck_loop_span(DeckId::A, Some((2.0, 2.125)), crate::decks::LoopSeek::MovedOut);
        // Well inside the span, but close enough to OUT that one buffer's
        // travel overshoots it with a clear margin, so the wrap is not
        // riding a floating-point coin toss at the boundary.
        let start_secs = (102_000.0 - 350.0) / 48_000.0;
        mixer.seek_deck_seconds(DeckId::A, start_secs);
        render(&mixer, 48_000.0, 512);
        let snap = mixer.deck_snapshot(DeckId::A);
        assert!(
            snap.position_secs >= 2.0 && snap.position_secs < 2.125,
            "the read path already wrapped the real head: {}",
            snap.position_secs
        );
        assert_eq!(
            snap.clock.beat_frac_end,
            grid.phase_at(snap.position_secs),
            "the clock must agree with where the head actually is"
        );
        let unfolded = grid.phase_at(start_secs + 512.0 / 48_000.0);
        assert!(
            (unfolded - snap.clock.beat_frac_end).abs() > 0.01,
            "and that has to be a different answer from ignoring the span entirely"
        );
    }

    /// A beat is an output length: pitch the record up and it gets shorter.
    #[test]
    fn the_clock_follows_the_tempo_fader() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, tone_pcm(220.0, 48_000, 8.0));
        mixer.set_deck_grid(DeckId::A, Some(clock_grid(120.0)));
        mixer.set_deck_playing(DeckId::A, true);
        mixer.set_deck_rate(DeckId::A, 1.25);
        spin_render(&mixer, 64);
        let clock = mixer.deck_snapshot(DeckId::A).clock;
        assert!((clock.platter_rate - 1.25).abs() < 1e-6, "{}", clock.platter_rate);
        assert!((clock.beat_secs_out - 0.4).abs() < 1e-6, "{}", clock.beat_secs_out);
    }


#[test]
fn a_censor_under_a_latched_slip_leaves_the_operators_ghost_running() {
    let mixer = spin_deck_a(16_384, 480_000);
    mixer.set_deck_playing(DeckId::A, true);
    spin_render(&mixer, 8);
    mixer.set_deck_slip(DeckId::A, true, false);
    let slipped_at = mixer.deck_snapshot(DeckId::A).position_secs;

    mixer.set_deck_censor(DeckId::A, true);
    spin_render(&mixer, 16);
    mixer.set_deck_censor(DeckId::A, false);
    spin_render(&mixer, 1);
    assert!(
        mixer.state().decks[DeckId::A.index()].slip.is_some(),
        "the operator's ghost is still theirs",
    );

    // And it is still RUNNING: releasing SLIP some buffers later lands
    // further on again, by exactly the time that passed.
    let censor_landing = mixer.deck_snapshot(DeckId::A).position_secs;
    let after = 20;
    spin_render(&mixer, after);
    mixer.set_deck_slip(DeckId::A, false, false);
    spin_render(&mixer, 1);
    let slip_landing = mixer.deck_snapshot(DeckId::A).position_secs;
    assert!(
        slip_landing > censor_landing,
        "the ghost went on: {censor_landing} -> {slip_landing}",
    );
    let want = slipped_at + ((16 + 1 + after + 1) * 512) as f64 / 48_000.0;
    assert!(
        (slip_landing - want).abs() < 0.05,
        "and by the elapsed time: want {want}, got {slip_landing}",
    );
}

    #[test]
    fn a_seek_during_a_pauses_fade_is_not_undone_by_it() {
        // The pause promises to hand back the frames its fade sounded. A
        // deliberate move afterwards -- CUE returning to its mark is one --
        // means that promise no longer applies.
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }
        let pressed_at = mixer.deck_snapshot(DeckId::A).position_secs;
        mixer.set_deck_playing(DeckId::A, false);
        mixer.seek_deck_seconds(DeckId::A, 0.0);
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }
        let landed = mixer.deck_snapshot(DeckId::A).position_secs;
        // Not exactly zero: the fade goes on sounding for its own few
        // milliseconds from the new place, which is the point of it.
        assert!(
            landed < 0.02,
            "the seek stands, give or take the fade's own length: {landed}"
        );
        assert!(
            pressed_at - landed > 0.05,
            "and it is nowhere near where pause was pressed ({pressed_at})"
        );
    }


    /// The grid is the record's: a double carries it (a different grid on
    /// the other deck proves it was carried, not kept), a clear drops it,
    /// a fresh load starts without one, and a grid that lands while a
    /// load is parked belongs to the record coming IN -- the outgoing one
    /// keeps its own beat until it is gone.
    #[test]
    fn the_grid_travels_with_the_record_and_leaves_with_it() {
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, tone_pcm(220.0, 48_000, 8.0));
        mixer.install_deck(DeckId::B, tone_pcm(330.0, 48_000, 8.0));
        mixer.set_deck_grid(DeckId::A, Some(clock_grid(120.0)));
        mixer.set_deck_grid(DeckId::B, Some(clock_grid(126.0)));
        mixer.set_deck_playing(DeckId::A, true);
        mixer.set_deck_playing(DeckId::B, true);
        spin_render(&mixer, 4);
        assert!((mixer.deck_snapshot(DeckId::B).clock.beat_secs_out - 60.0 / 126.0).abs() < 1e-9);
        mixer.clone_deck(DeckId::A, DeckId::B);
        spin_render(&mixer, 4);
        let b = mixer.deck_snapshot(DeckId::B).clock;
        assert!((b.beat_secs_out - 0.5).abs() < 1e-9, "the double reads its record's beat: {}", b.beat_secs_out);
        mixer.clear_deck(DeckId::B);
        spin_render(&mixer, 4);
        assert!(!mixer.deck_snapshot(DeckId::B).clock.has_grid, "a cleared deck has no beat");
        // A fresh load drops whatever grid was there, which a deck with
        // no grid at all cannot prove: put a real one back on B, let its
        // transport fade all the way down so the load takes the
        // immediate branch, then load over it and watch it go.
        mixer.install_deck(DeckId::B, tone_pcm(330.0, 48_000, 8.0));
        mixer.set_deck_grid(DeckId::B, Some(clock_grid(140.0)));
        mixer.set_deck_playing(DeckId::B, true);
        spin_render(&mixer, 2);
        assert!(mixer.deck_snapshot(DeckId::B).clock.has_grid, "the grid is there to lose");
        mixer.set_deck_playing(DeckId::B, false);
        spin_render(&mixer, 2); // the pause fade is one buffer; two is headroom
        mixer.install_deck(DeckId::B, tone_pcm(330.0, 48_000, 8.0));
        spin_render(&mixer, 4);
        assert!(!mixer.deck_snapshot(DeckId::B).clock.has_grid, "a fresh record has none until its analysis lands");
        // A load over the playing deck A parks the new record; the grid
        // that lands now is the new record's.
        mixer.install_deck_over(DeckId::A, tone_pcm(440.0, 48_000, 8.0), true);
        mixer.set_deck_grid(DeckId::A, Some(clock_grid(126.0)));
        let outgoing = mixer.deck_snapshot(DeckId::A).clock;
        assert!((outgoing.beat_secs_out - 0.5).abs() < 1e-9, "the outgoing record keeps its beat while it fades");
        spin_render(&mixer, 32);
        let incoming = mixer.deck_snapshot(DeckId::A).clock;
        assert!(incoming.has_grid, "the parked grid came in with the record");
        assert!(
            (incoming.beat_secs_out - 60.0 / 126.0).abs() < 1e-9,
            "and it is the new record's: {}",
            incoming.beat_secs_out
        );
    }

    #[test]
    fn an_unload_during_the_fade_cancels_the_parked_load() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(8_000, 480_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4_096);

        let parked = const_pcm(4_000, 192_000, 48_000);
        mixer.install_deck_over(DeckId::A, parked.clone(), true);
        mixer.clear_deck(DeckId::A);
        mixer.sync();
        mixer.reap_retired();
        assert_eq!(Arc::strong_count(&parked), 1, "the parked load went with the unload");
        for _ in 0..8 {
            render(&mixer, 48_000.0, 512);
        }
        // Nothing resurrects on an emptied deck.
        assert!(
            mixer.state().decks[DeckId::A.index()].pcm.is_none(),
            "nothing resurrects on an emptied deck"
        );
        assert!(mixer.deck_snapshot(DeckId::A).duration_secs.abs() < 1e-9);
    }

    #[test]
    fn play_pressed_during_the_fade_re_aims_the_load_rather_than_the_old_track() {
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 4_096);

        // Aimed to stop, then the operator changes their mind mid-fade.
        mixer.install_deck_over(DeckId::A, const_pcm(8_192, 480_000, 48_000), false);
        mixer.set_deck_playing(DeckId::A, true);
        for _ in 0..16 {
            render(&mixer, 48_000.0, 512);
        }
        let snap = mixer.deck_snapshot(DeckId::A);
        assert!(snap.playing, "the deck came up on the new track");
        assert!((snap.duration_secs - 10.0).abs() < 1e-6);
    }

    /// A load over a playing deck, and an unload, both put a held
    /// freeze away -- the swap on its own turn, the unload at once.
    #[test]
    fn a_load_and_an_unload_put_the_freeze_away() {
        let rate = 48_000.0;
        let mixer = TestMixer::new();
        mixer.set_master(1.0);
        mixer.set_crossfader(0.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        spin_render(&mixer, 20);
        mixer.set_deck_freeze(DeckId::A, Some(0.1));
        assert!(deck_frozen(&mixer, DeckId::A));
        mixer.install_deck_over(DeckId::A, const_pcm(8_192, 480_000, 48_000), true);
        spin_render(&mixer, 8); // through the outgoing fade and the swap
        assert!(!deck_frozen(&mixer, DeckId::A), "the swap must have let it go");
        let out = render_out(&mixer, rate, 4, 512);
        let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let expected = 8_192.0 / 32_768.0;
        assert!(
            (peak - expected).abs() < 0.02,
            "the new record must be heard, unfrozen: {peak} vs {expected}"
        );

        mixer.set_deck_freeze(DeckId::A, Some(0.1));
        assert!(deck_frozen(&mixer, DeckId::A));
        mixer.clear_deck(DeckId::A);
        // HEAD's ClearDeck dispatch does not reset the deck's chain (see
        // "missing"); until it does, this assertion fails.
        assert!(!deck_frozen(&mixer, DeckId::A), "an unload must have let it go");
    }


    #[test]
    fn a_seek_shows_in_the_snapshot_before_the_next_callback() {
        // The UI seeks and reads back in the same tick; the answer must not
        // lag a buffer behind.
        //
        // Read through `Mixer`'s own methods explicitly (bypassing
        // `TestMixer`'s Deref-shadowed `deck_snapshot`/`deck_position`,
        // which call the test-only `MixEngine::sync()` before every read --
        // exactly the safety net a real UI thread does not have, and which
        // would silently hide the gap this test exists to catch).
        let mixer = TestMixer::new();
        mixer.install_deck(DeckId::A, const_pcm(16_384, 96_000, 48_000));
        mixer.set_deck_playing(DeckId::A, true);
        mixer.seek_deck_seconds(DeckId::A, 1.25);
        let snapshot = Mixer::deck_snapshot(&mixer, DeckId::A);
        assert!((snapshot.position_secs - 1.25).abs() < 1e-9, "{}", snapshot.position_secs);
        assert!(snapshot.playing);
        assert!((snapshot.duration_secs - 2.0).abs() < 1e-9);
        let (position, duration, playing) = Mixer::deck_position(&mixer, DeckId::A);
        assert!((position - 1.25).abs() < 1e-9 && (duration - 2.0).abs() < 1e-9 && playing);
    }

}
