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
use crate::loop_splat::{
    SplatGrid, SplatPart, SplatRow, SplatSnapshot, SPLAT_COLS, SPLAT_ROWS,
};
use crate::music_dsp::{
    DeckEq, FrameSource, ParamRamp, RateReader, ScratchRamp, Stretcher, STEM_COUNT,
    STRETCH_BYPASS_EPSILON, STRETCH_RATIO_MAX, STRETCH_RATIO_MIN, WSOLA_WINDOW,
};
use crate::pads::{PadKey, VoiceAlloc, VoiceId};
use crate::score_preview::{PreviewEvent, PreviewSequence};
use makepad_drumkit::{DrumKit, SampleBank};
use makepad_piano_model::{Piano, PianoEvent, TimedEvent as PianoTimedEvent};
use makepad_widgets::makepad_platform::audio::AudioBuffer;
use makepad_widgets::makepad_platform::thread::lock_from_ui;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Q32.32 fixed-point source-frame cursor.
const FP_ONE: u64 = 1 << 32;
/// Default parameter slew, seconds — fast enough to feel instant, slow
/// enough to never click.
const SLEW_SECS: f32 = 0.008;
/// Autopilot blend moves: fast enough to read as a cut on the bar, slow
/// enough never to click.
const BLEND_SECS: f32 = 0.08;
/// Cap on queued video-slot audio, frames (~2s at 48k): a stalled consumer
/// can never grow a queue without bound.
const MAX_SLOT_QUEUE_FRAMES: usize = 96_000;
/// Master safety clamp.
const CLAMP: f32 = 1.0;
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
    frames: SplatFrames,
    active: bool,
    master_frames: f64,
    rows: [SplatRowVoice; SPLAT_ROWS],
}

impl SplatState {
    fn new(grid: Arc<SplatGrid>, frames: SplatFrames, master_frames: f64) -> Self {
        Self {
            grid,
            frames,
            active: false,
            master_frames,
            rows: [SplatRowVoice::default(); SPLAT_ROWS],
        }
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

    fn queue_cell(&mut self, row: SplatRow, col: usize, part: SplatPart) {
        if !self.active || col >= SPLAT_COLS || !part.is_valid() {
            return;
        }
        let at_frames = self.next_bar_after(self.master_frames);
        let Some(cell) = self.frames.cells[row.index()][col] else { return };
        let denominator = f64::from(part.den);
        let part_len = cell.len_frames / denominator;
        self.rows[row.index()].queued = Some(Queued {
            cell: Some(RowCell {
                col: cell.col,
                part,
                start_frames: cell.start_frames + f64::from(part.num) * part_len,
                len_frames: part_len.max(1.0),
                anchor_frames: at_frames,
            }),
            at_frames,
        });
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

/// A few milliseconds of the outgoing stream kept alive after a commanded
/// jump, so the seek lands as a blend instead of a splice. `left/total` is
/// the outgoing share, counted down a frame per rendered frame.
struct SeekFade {
    pos: f64,
    left: f64,
    total: f64,
}

struct DeckVoice {
    pcm: Option<Arc<TrackPcm>>,
    stems: Option<Arc<TrackStems>>,
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
    stretch: Box<Stretcher>,
    reader: RateReader,
    eq: DeckEq,
    stem_gain: [ParamRamp; STEM_COUNT],
    /// The autopilot's blend overlay on the stem lanes: multiplies the
    /// operator's gains, never moves them. 1.0 = hands off.
    blend_stem: [ParamRamp; STEM_COUNT],
}

impl DeckVoice {
    fn new() -> DeckVoice {
        DeckVoice {
            pcm: None,
            stems: None,
            splat: None,
            pos: 0.0,
            playing: false,
            loop_span: None,
            seek_fade: None,
            gain: Ramp::at(1.0),
            mute: Ramp::at(1.0),
            ended: false,
            rate: ParamRamp::at(1.0),
            key_ratio: ParamRamp::at(1.0),
            keylock: true,
            scratch: ScratchRamp::default(),
            stretching: false,
            stretch: Box::new(Stretcher::new()),
            reader: RateReader::default(),
            eq: DeckEq::new(48_000.0),
            stem_gain: [ParamRamp::at(1.0); STEM_COUNT],
            blend_stem: [ParamRamp::at(1.0); STEM_COUNT],
        }
    }

    /// Snap the whole blend overlay home instantly — a fresh track never
    /// inherits a transition's ducking.
    fn reset_blend(&mut self) {
        self.blend_stem = [ParamRamp::at(1.0); STEM_COUNT];
        self.eq.reset_blend();
    }

    fn frame_count(&self) -> usize {
        self.pcm.as_ref().map(|pcm| pcm.frames.len()).unwrap_or(0)
    }

    /// Move the playhead and drop every bit of streaming state that was
    /// tied to the old position.
    fn seek_frames(&mut self, frames: f64) {
        let len = self.frame_count() as f64;
        self.pos = frames.clamp(0.0, len);
        if let Some(splat) = self.splat.as_mut().filter(|splat| splat.active) {
            splat.master_frames = self.pos;
        }
        self.stretch.reset_to(self.pos);
        self.reader.reset();
        self.ended = false;
    }

    /// Where the playhead really is, whichever path is driving it.
    fn playhead_frames(&self) -> f64 {
        if let Some(splat) = self.splat.as_ref().filter(|splat| splat.active) {
            return splat.master_frames;
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
        let total = (SEEK_XFADE_SECS * pcm.sample_rate.max(1) as f64).max(1.0);
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
    pcm: &TrackPcm,
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
    pcm: &TrackPcm,
    stems: Option<&TrackStems>,
    stem_gain: [f32; STEM_COUNT],
    source_step: f64,
) -> [f32; 2] {
    let master = splat.master_frames;
    let fade_frames = (SPLAT_XFADE_SECS * pcm.sample_rate.max(1) as f64).max(1.0);
    let mut sum = [0.0f32; 2];
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
        }
        let frame = if let Some(fade) = voice.fade {
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
        sum[0] += frame[0];
        sum[1] += frame[1];
    }
    splat.master_frames += source_step;
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
}

impl CueRing {
    fn new() -> CueRing {
        CueRing {
            buf: (0..CUE_RING_FRAMES).map(|_| AtomicU64::new(0)).collect(),
            write_pos: AtomicU64::new(0),
            main_rate_bits: AtomicU64::new(0),
            armed: AtomicBool::new(false),
            volume_bits: AtomicU32::new(1.0f32.to_bits()),
        }
    }

    #[inline]
    fn push(&self, pos: u64, left: f32, right: f32) {
        let packed = (left.to_bits() as u64) | ((right.to_bits() as u64) << 32);
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
                // next callback re-primes at depth.
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
    /// Per-SLOT headphone cue toggles: the cue button belongs to the
    /// channel strip, not the record, so `swap_decks` leaves these alone.
    cue_deck: [bool; 2],
    cue_mode: CueMode,
    preview: PreviewVoice,
    score_preview: ScorePreviewVoice,
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
    /// One-shot proof that transport, PCM and the output callback met.
    first_non_silent: Arc<AtomicBool>,
    /// Callbacks that found the state lock held and went out SILENT — every
    /// count here is an audible gap in the programme. The pump reports
    /// growth, so a dropout heard in the room can be told apart from a
    /// device-level glitch by whether this moved.
    contended_callbacks: Arc<AtomicU64>,
    /// High-water render time, nanoseconds, for the other failure class: a
    /// render that outruns its buffer starves the device with the lock
    /// UNCONTENDED.
    render_max_nanos: Arc<AtomicU64>,
    /// The headphone cue bus, written by `render`, drained by the phones
    /// device callback (slot 1).
    cue_ring: Arc<CueRing>,
}

/// Infrequent UI-to-audio-state handoffs that carry prepared immutable
/// resources rather than scalar deck controls.
pub enum MixCmd {
    SetDrumBank(Arc<SampleBank>),
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
                cue_deck: [false; 2],
                cue_mode: CueMode::default(),
                preview: PreviewVoice::new(),
                score_preview: ScorePreviewVoice::new(48_000),
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
            first_non_silent: Arc::new(AtomicBool::new(false)),
            contended_callbacks: Arc::new(AtomicU64::new(0)),
            render_max_nanos: Arc::new(AtomicU64::new(0)),
            cue_ring: Arc::new(CueRing::new()),
        }
    }

    /// `(silent contended callbacks, high-water render nanos)` — the two
    /// distinguishable causes of a heard dropout, for the pump to report.
    pub fn audio_health(&self) -> (u64, u64) {
        (
            self.contended_callbacks.load(Ordering::Relaxed),
            self.render_max_nanos.load(Ordering::Relaxed),
        )
    }

    // ---- video slot buses --------------------------------------------------

    /// (Re)open a slot bus, silent, empty, unpaused.
    pub fn open_slot(&self, slot: SlotId) {
        let mut s = lock_from_ui(&self.state);
        let bus = &mut s.video[slot.index()];
        bus.flush();
        bus.open = true;
        bus.paused = false;
        bus.playback_rate = 1.0;
        bus.gain = Ramp::at(0.0);
    }

    /// Close = mute-and-flush; the decode thread just stops feeding it.
    pub fn close_slot(&self, slot: SlotId) {
        let mut s = lock_from_ui(&self.state);
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
        lock_from_ui(&self.state).video[slot.index()].flush();
    }

    /// Decode-worker half of [`Self::flush_slot_audio`].
    pub fn flush_slot_audio_from_worker(&self, slot: SlotId) {
        self.state.lock().unwrap().video[slot.index()].flush();
    }

    pub fn set_slot_paused(&self, slot: SlotId, paused: bool) {
        lock_from_ui(&self.state).video[slot.index()].paused = paused;
    }

    /// Audio resampling rate for a video slot. The bounded range is small on
    /// purpose: it is enough to fit a visual cycle to a musical phrase while
    /// remaining perceptually safe. Deck and SFX cursors are unrelated.
    pub fn set_slot_playback_rate(&self, slot: SlotId, rate: f64) -> f64 {
        let rate = rate.clamp(MIN_VIDEO_PLAYBACK_RATE, MAX_VIDEO_PLAYBACK_RATE);
        lock_from_ui(&self.state).video[slot.index()].playback_rate = rate;
        rate
    }

    pub fn slot_playback_rate(&self, slot: SlotId) -> f64 {
        lock_from_ui(&self.state).video[slot.index()].playback_rate
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

    pub fn has_produced_non_silent(&self) -> bool {
        self.first_non_silent.load(Ordering::Acquire)
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
        let mut state = lock_from_ui(&self.state);
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
        let mut state = lock_from_ui(&self.state);
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
        let mut state = lock_from_ui(&self.state);
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
        let mut s = lock_from_ui(&self.state);
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
        let mut s = lock_from_ui(&self.state);
        if s.scheduled_video.is_some() {
            return;
        }
        let (a, b) = crate::decks::crossfader_gains(mix, crate::decks::FadeCurve::EqualPower);
        s.video[0].gain.slew(a, 0.015);
        s.video[1].gain.slew(b, 0.015);
    }

    pub fn set_video_muted(&self, muted: bool) {
        lock_from_ui(&self.state)
            .video_mute
            .slew(if muted { 0.0 } else { 1.0 }, SLEW_SECS * 4.0);
    }

    // ---- decks -------------------------------------------------------------

    /// Install a decoded track, paused at zero. Any stems from a previous
    /// track go with it; the tone chain is reset but its settings stand.
    pub fn install_deck(&self, deck: DeckId, pcm: Arc<TrackPcm>) {
        let mut s = lock_from_ui(&self.state);
        let d = &mut s.decks[deck.index()];
        d.pcm = Some(pcm);
        d.stems = None;
        d.splat = None;
        d.playing = false;
        d.seek_frames(0.0);
        d.eq.reset();
        d.reset_blend();
    }

    /// Drop the deck's track entirely: the voice renders silence until the
    /// next install. Settings (gain, EQ, keylock) stand, like install_deck.
    pub fn clear_deck(&self, deck: DeckId) {
        let mut s = lock_from_ui(&self.state);
        let d = &mut s.decks[deck.index()];
        d.pcm = None;
        d.stems = None;
        d.splat = None;
        d.playing = false;
        // With no pcm the clamp parks the playhead at zero; this also
        // clears `ended`, so a later install re-arms end reporting.
        d.seek_frames(0.0);
        d.reset_blend();
    }

    /// Attach separated stems to the track already on the deck. They must be
    /// the same timeline as the mixed file; the deck keeps playing.
    pub fn install_deck_stems(&self, deck: DeckId, stems: Arc<TrackStems>) {
        let mut s = lock_from_ui(&self.state);
        let d = &mut s.decks[deck.index()];
        if d.pcm.is_none() || stems.is_empty() {
            return;
        }
        d.stems = Some(stems);
    }

    pub fn clear_deck_stems(&self, deck: DeckId) {
        lock_from_ui(&self.state).decks[deck.index()].stems = None;
    }

    pub fn set_deck_playing(&self, deck: DeckId, playing: bool) {
        let mut s = lock_from_ui(&self.state);
        let d = &mut s.decks[deck.index()];
        if playing {
            // Playing from the end restarts.
            if d.playhead_frames() >= d.frame_count() as f64
                && !d.splat.as_ref().is_some_and(|splat| splat.active)
            {
                d.seek_frames(0.0);
            }
            d.ended = false;
        }
        d.playing = playing;
    }

    /// Install or replace a grid. Frame conversion is deliberately done
    /// here, on the caller thread, before the callback sees the state.
    pub fn set_deck_splat(&self, deck: DeckId, grid: Arc<SplatGrid>) {
        let mut state = lock_from_ui(&self.state);
        let voice = &mut state.decks[deck.index()];
        let Some(pcm) = voice.pcm.as_ref() else { return };
        let frames = SplatFrames::from_grid(&grid, pcm.sample_rate.max(1) as f64);
        match voice.splat.as_mut() {
            Some(splat) => {
                splat.grid = grid;
                splat.frames = frames;
            }
            None => voice.splat = Some(SplatState::new(grid, frames, voice.pos)),
        }
    }

    pub fn set_deck_splat_enabled(&self, deck: DeckId, on: bool) {
        let mut state = lock_from_ui(&self.state);
        let voice = &mut state.decks[deck.index()];
        let frame_count = voice.frame_count() as f64;
        let Some(splat) = voice.splat.as_mut() else { return };
        if on == splat.active {
            return;
        }
        if on {
            splat.master_frames = splat.bar_start_at_or_before(voice.pos).clamp(0.0, frame_count);
            splat.active = true;
            voice.stretching = false;
            voice.reader.reset();
        } else {
            let master = splat.master_frames.clamp(0.0, frame_count);
            splat.active = false;
            voice.seek_frames(master);
        }
    }

    pub fn splat_launch(&self, deck: DeckId, row: SplatRow, col: u8, part: SplatPart) {
        let mut state = lock_from_ui(&self.state);
        if let Some(splat) = state.decks[deck.index()].splat.as_mut() {
            splat.queue_cell(row, col as usize, part);
        }
    }

    pub fn splat_stop_row(&self, deck: DeckId, row: SplatRow, timed: bool) {
        let mut state = lock_from_ui(&self.state);
        if let Some(splat) = state.decks[deck.index()].splat.as_mut() {
            splat.queue_stop(row, timed);
        }
    }

    /// Launch a whole section: every STEM row of the column. The mix row is
    /// the undemixed track and never plays under its own stems.
    pub fn splat_launch_scene(&self, deck: DeckId, col: u8) {
        let mut state = lock_from_ui(&self.state);
        if let Some(splat) = state.decks[deck.index()].splat.as_mut() {
            for row in SplatRow::ALL {
                if row == SplatRow::Mix {
                    continue;
                }
                splat.queue_cell(row, col as usize, SplatPart::WHOLE);
            }
        }
    }

    pub fn splat_stop_all(&self, deck: DeckId, timed: bool) {
        let mut state = lock_from_ui(&self.state);
        if let Some(splat) = state.decks[deck.index()].splat.as_mut() {
            for row in SplatRow::ALL {
                splat.queue_stop(row, timed);
            }
        }
    }

    pub fn seek_deck_fraction(&self, deck: DeckId, fraction: f64) {
        let mut s = lock_from_ui(&self.state);
        let d = &mut s.decks[deck.index()];
        let len = d.frame_count() as f64;
        if len > 0.0 {
            let from = d.playhead_frames();
            d.seek_frames(fraction.clamp(0.0, 1.0) * len);
            d.arm_seek_fade(from);
        }
    }

    /// Absolute seek in source seconds.
    pub fn seek_deck_seconds(&self, deck: DeckId, secs: f64) {
        let mut s = lock_from_ui(&self.state);
        let d = &mut s.decks[deck.index()];
        let Some(pcm) = d.pcm.as_ref() else { return };
        let frames = secs.max(0.0) * pcm.sample_rate.max(1) as f64;
        let from = d.playhead_frames();
        d.seek_frames(frames);
        d.arm_seek_fade(from);
    }

    /// Tempo multiplier. With key lock on the pitch is preserved; with it
    /// off the deck simply plays faster or slower.
    pub fn set_deck_rate(&self, deck: DeckId, rate: f64) {
        let rate = rate.clamp(crate::decks::RATE_MIN, crate::decks::RATE_MAX) as f32;
        // A short ramp so a sync landing mid-phrase does not step the pitch.
        lock_from_ui(&self.state).decks[deck.index()].rate.slew(rate, SLEW_SECS * 4.0);
    }

    pub fn deck_rate(&self, deck: DeckId) -> f64 {
        lock_from_ui(&self.state).decks[deck.index()].rate.target() as f64
    }

    /// Key shift in SEMITONES: pitch without tempo. Stored as the frequency
    /// ratio it stands for, because the render loop wants a multiplier and
    /// an exp2 per frame would be a waste.
    pub fn set_deck_key_shift(&self, deck: DeckId, semitones: f64) {
        let semitones = semitones.clamp(-crate::decks::KEY_SHIFT_MAX, crate::decks::KEY_SHIFT_MAX);
        let ratio = (semitones / 12.0).exp2() as f32;
        // Same ramp as the tempo: a stepped semitone glides instead of
        // clicking, and the stretcher sees a ratio that never jumps.
        lock_from_ui(&self.state).decks[deck.index()].key_ratio.slew(ratio, SLEW_SECS * 4.0);
    }

    pub fn set_deck_keylock(&self, deck: DeckId, on: bool) {
        lock_from_ui(&self.state).decks[deck.index()].keylock = on;
    }

    /// Vinyl-style pointer control over the playhead.
    pub fn scratch_deck(&self, deck: DeckId, motion: ScratchMotion) {
        let mut s = lock_from_ui(&self.state);
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
        lock_from_ui(&self.state).decks[deck.index()].eq.set_band(band, gain);
    }

    /// Bipolar sweep filter; 0.5 = off.
    pub fn set_deck_filter(&self, deck: DeckId, position: f32) {
        lock_from_ui(&self.state).decks[deck.index()].eq.set_filter(position);
    }

    /// One stem lane's gain. Ramped, so a knob move never zippers.
    pub fn set_deck_stem_gain(&self, deck: DeckId, stem: usize, gain: f32) {
        if stem >= STEM_COUNT {
            return;
        }
        lock_from_ui(&self.state).decks[deck.index()].stem_gain[stem]
            .slew(gain.max(0.0), SLEW_SECS * 2.0);
    }

    /// The deck's loop in source SECONDS, converted here against the
    /// track's own rate so the render path only ever deals in frames.
    pub fn set_deck_loop_span(&self, deck: DeckId, span: Option<(f64, f64)>) {
        let mut s = lock_from_ui(&self.state);
        let d = &mut s.decks[deck.index()];
        let Some(pcm) = d.pcm.as_ref() else {
            d.loop_span = None;
            return;
        };
        let rate = pcm.sample_rate.max(1) as f64;
        // Clamp OUT to the real frame count: the seconds->frames round trip
        // can land a hair ABOVE it, and an OUT past the last frame lets the
        // end-of-track check win over the wrap — a dead deck with LOOP lit.
        let frames = pcm.frames.len() as f64;
        d.loop_span = span.map(|(start, end)| {
            (start.max(0.0) * rate, (end.max(0.0) * rate).min(frames))
        });
        // A playhead stranded past the new OUT lands modulo NOW. The render
        // wrap would catch it on the next callback anyway, but a PAUSED
        // deck never renders — without this, a resize on a paused deck
        // parks the playhead outside the span until play is pressed.
        if let Some((start, end)) = d.loop_span {
            let len = (end - start).max(1.0);
            if d.playhead_frames() >= end {
                let from = d.playhead_frames();
                let over = (from - start).rem_euclid(len);
                d.seek_frames(start + over);
                // A live resize yanking a playing playhead is a jump like
                // any other and gets the same blend.
                d.arm_seek_fade(from);
            }
        }
    }

    pub fn set_deck_mute(&self, deck: DeckId, muted: bool) {
        lock_from_ui(&self.state).decks[deck.index()]
            .mute
            .slew(if muted { 0.0 } else { 1.0 }, SLEW_SECS);
    }

    pub fn set_deck_gain(&self, deck: DeckId, gain: f32) {
        lock_from_ui(&self.state).decks[deck.index()].gain.slew(gain, SLEW_SECS);
    }

    pub fn swap_decks(&self) {
        lock_from_ui(&self.state).decks.swap(0, 1);
    }

    pub fn set_crossfader(&self, position: f32) {
        lock_from_ui(&self.state).fader.slew(position.clamp(0.0, 1.0), SLEW_SECS);
    }

    pub fn fade_crossfader(&self, position: f32, secs: f32) {
        lock_from_ui(&self.state).fader.slew(position.clamp(0.0, 1.0), secs.max(SLEW_SECS));
    }

    /// Where the crossfader actually is right now, mid-ramp included. The
    /// deck surface mirrors this while a timed fade runs, so the on-screen
    /// fader travels with the audio instead of teleporting to the target.
    pub fn crossfader_position(&self) -> f32 {
        lock_from_ui(&self.state).fader.current
    }

    /// The autopilot's blend overlay: multiplies the operator's values,
    /// never moves them. `clear_blend` is the whole restore.
    pub fn set_blend_band(&self, deck: DeckId, band: usize, gain: f32) {
        lock_from_ui(&self.state).decks[deck.index()].eq.set_blend_band(band, gain);
    }

    pub fn set_blend_stem(&self, deck: DeckId, stem: usize, gain: f32) {
        if stem >= STEM_COUNT {
            return;
        }
        lock_from_ui(&self.state).decks[deck.index()].blend_stem[stem]
            .slew(gain.clamp(0.0, 1.0), BLEND_SECS);
    }

    pub fn clear_blend(&self, deck: DeckId) {
        let mut s = lock_from_ui(&self.state);
        let d = &mut s.decks[deck.index()];
        d.eq.clear_blend();
        for ramp in &mut d.blend_stem {
            ramp.slew(1.0, BLEND_SECS);
        }
    }

    pub fn set_curve(&self, curve: FadeCurve) {
        lock_from_ui(&self.state).curve = curve;
    }

    pub fn set_master(&self, gain: f32) {
        lock_from_ui(&self.state).master.slew(gain.clamp(0.0, 1.2), SLEW_SECS);
    }

    /// `(position_secs, duration_secs, playing)` from the device clock.
    pub fn deck_position(&self, deck: DeckId) -> (f64, f64, bool) {
        let s = lock_from_ui(&self.state);
        let d = &s.decks[deck.index()];
        match &d.pcm {
            None => (0.0, 0.0, false),
            Some(pcm) => {
                let position = d.playhead_frames() / pcm.sample_rate.max(1) as f64;
                (position, pcm.seconds(), d.playing)
            }
        }
    }

    /// Position, transport and splat state in one lock. The per-frame UI path
    /// uses this because every extra grab competes with the callback's
    /// `try_lock`.
    pub fn deck_snapshot(&self, deck: DeckId) -> DeckSnapshot {
        let s = lock_from_ui(&self.state);
        let d = &s.decks[deck.index()];
        let scratching = d.scratch.active();
        match &d.pcm {
            None => DeckSnapshot {
                position_secs: 0.0,
                duration_secs: 0.0,
                playing: false,
                scratching,
                splat: None,
            },
            Some(pcm) => DeckSnapshot {
                position_secs: d.playhead_frames() / pcm.sample_rate.max(1) as f64,
                duration_secs: pcm.seconds(),
                playing: d.playing,
                scratching,
                splat: d.splat.as_ref().map(SplatState::snapshot),
            },
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
        lock_from_ui(&self.state).decks[deck.index()].scratch.active()
    }

    /// Decks that ran off their end (loop off) since the last drain.
    pub fn drain_ended_decks(&self) -> Vec<DeckId> {
        std::mem::take(&mut lock_from_ui(&self.state).ended_decks)
    }

    // ---- sfx voices ---------------------------------------------------------

    pub fn start_voice(&self, alloc: VoiceAlloc, pcm: Arc<TrackPcm>) {
        let mut s = lock_from_ui(&self.state);
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
        let mut s = lock_from_ui(&self.state);
        // Fast declick: a stopped voice ramps out over one slew and is
        // reaped by the render pass.
        for v in s.sfx.iter_mut().filter(|v| v.id == id) {
            v.loop_on = false;
            v.gain.slew(0.0, SLEW_SECS);
            v.done = true;
        }
    }

    pub fn set_pad_voices_gain(&self, pad: PadKey, gain: f32) {
        let mut s = lock_from_ui(&self.state);
        for v in s.sfx.iter_mut().filter(|v| v.pad == pad && !v.done) {
            v.gain.slew(gain, SLEW_SECS);
        }
    }

    /// Voices that finished naturally (ran off the end, loop off).
    pub fn drain_ended_voices(&self) -> Vec<VoiceId> {
        std::mem::take(&mut lock_from_ui(&self.state).ended_voices)
    }

    /// Current peak meters: `[master, video, deck_a, deck_b, sfx]`.
    pub fn meters(&self) -> [f32; 5] {
        let mut out = [0.0f32; 5];
        for (i, m) in self.meters.iter().enumerate() {
            out[i] = f32::from_bits(m.load(Ordering::Relaxed));
        }
        out
    }

    // ---- the headphone cue bus ----------------------------------------------

    /// The ring the slot-1 (phones) callback drains. The callback holds
    /// ONLY this — never the mix state, whose lock belongs to the program.
    pub fn cue_ring(&self) -> Arc<CueRing> {
        self.cue_ring.clone()
    }

    /// Armed while a phones device is actually requested at slot 1: gates
    /// the producer, and an unarmed consumer outputs silence.
    pub fn set_cue_armed(&self, armed: bool) {
        self.cue_ring.armed.store(armed, Ordering::Relaxed);
    }

    pub fn set_phones_volume(&self, volume: f32) {
        self.cue_ring
            .volume_bits
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Route a deck into the phones. Cue follows the deck SLOT (the channel
    /// strip), not the record: `swap_decks` deliberately leaves it alone.
    pub fn set_deck_cue(&self, deck: DeckId, on: bool) {
        lock_from_ui(&self.state).cue_deck[deck.index()] = on;
    }

    pub fn deck_cue(&self, deck: DeckId) -> bool {
        lock_from_ui(&self.state).cue_deck[deck.index()]
    }

    /// Which point of the chain the cue listens to. A hard switch, like
    /// the monitor-select toggle on hardware.
    pub fn set_cue_mode(&self, mode: CueMode) {
        lock_from_ui(&self.state).cue_mode = mode;
    }

    pub fn cue_mode(&self) -> CueMode {
        lock_from_ui(&self.state).cue_mode
    }

    /// Install (and by default start) the pre-listen player.
    pub fn install_preview(&self, pcm: Arc<TrackPcm>, autoplay: bool) {
        let mut s = lock_from_ui(&self.state);
        s.preview.pcm = Some(pcm);
        s.preview.cursor_fp = 0;
        s.preview.ended = false;
        s.preview.playing = autoplay;
        s.preview.gain = Ramp::at(0.0);
        if autoplay {
            s.preview.gain.slew(1.0, SLEW_SECS);
        }
    }

    /// Take the preview down and HAND BACK the pcm, so the (possibly huge)
    /// buffer is dropped by the caller, outside this lock.
    pub fn clear_preview(&self) -> Option<Arc<TrackPcm>> {
        let mut s = lock_from_ui(&self.state);
        s.preview.playing = false;
        s.preview.ended = false;
        s.preview.cursor_fp = 0;
        s.preview.gain = Ramp::at(0.0);
        s.preview.pcm.take()
    }

    pub fn set_preview_playing(&self, playing: bool) {
        let mut s = lock_from_ui(&self.state);
        if s.preview.pcm.is_none() {
            return;
        }
        // Play on a parked player starts the track over — the player's one
        // transport button should never be a dead end.
        if playing && s.preview.ended {
            s.preview.cursor_fp = 0;
            s.preview.ended = false;
        }
        s.preview.playing = playing;
        s.preview.gain.slew(if playing { 1.0 } else { 0.0 }, SLEW_SECS);
    }

    pub fn seek_preview_fraction(&self, fraction: f64) {
        let mut s = lock_from_ui(&self.state);
        let Some(pcm) = s.preview.pcm.as_ref() else { return };
        let len = pcm.frames.len() as f64;
        let frame = (fraction.clamp(0.0, 1.0) * len).clamp(0.0, (len - 1.0).max(0.0));
        s.preview.cursor_fp = (frame * FP_ONE as f64) as u64;
        s.preview.ended = false;
    }

    /// `(position_secs, duration_secs, playing, ended)` — the pre-listen
    /// mirror of `deck_position`. `None` while no preview is installed.
    pub fn preview_position(&self) -> Option<(f64, f64, bool, bool)> {
        let s = lock_from_ui(&self.state);
        let p = &s.preview;
        let pcm = p.pcm.as_ref()?;
        let position =
            p.cursor_fp as f64 / FP_ONE as f64 / pcm.sample_rate.max(1) as f64;
        Some((position, pcm.seconds(), p.playing, p.ended))
    }

    // ---- loop-score preview ------------------------------------------------

    /// Deliver a prepared resource to the state owned by the audio callback.
    /// The short mutex handoff is the same path used by deck commands; the
    /// callback itself still only `try_lock`s and never blocks.
    pub fn run_cmd(&self, command: MixCmd) {
        let retired = {
            let mut state = lock_from_ui(&self.state);
            match command {
                MixCmd::SetDrumBank(bank) => state.score_preview.set_drum_bank(bank),
            }
        };
        // A replaced sample bank may own large buffers. Release its last Arc
        // outside the shared audio-state lock.
        drop(retired);
    }

    /// Install and start a score preview. Instrument construction and event
    /// capacity growth happen here on the caller/UI thread, never in render.
    pub fn score_preview_play(&self, sequence: Arc<PreviewSequence>) {
        let sample_rate = sequence.sample_rate.max(1);
        let needed = ScorePreviewVoice::required_event_capacity(&sequence);
        let (rebuild, grow_events) = {
            let state = lock_from_ui(&self.state);
            (
                state.score_preview.sample_rate != sample_rate,
                state.score_preview.piano_events.capacity() < needed,
            )
        };
        let piano = rebuild.then(|| Box::new(Piano::new(sample_rate as f32)));
        let events = grow_events.then(|| Vec::with_capacity(needed));
        let mut state = lock_from_ui(&self.state);
        let retired_piano = piano.map(|piano| {
            state.score_preview.replace_instruments(piano, sample_rate)
        });
        let retired_events = events.map(|events| {
            std::mem::replace(&mut state.score_preview.piano_events, events)
        });
        let retired_sequence = state.score_preview.play(sequence);
        drop(state);
        drop(retired_piano);
        drop(retired_events);
        drop(retired_sequence);
    }

    pub fn score_preview_stop(&self) {
        lock_from_ui(&self.state).score_preview.stop(true);
    }

    pub fn score_preview_state(&self) -> (bool, u64) {
        let state = lock_from_ui(&self.state);
        (state.score_preview.playing, state.score_preview.pos)
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
        let Ok(mut s) = self.state.try_lock() else {
            self.contended_callbacks.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let render_started = crate::clock::Instant::now();
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
        s.score_preview.render_block(frames, device_rate);

        // The headphone cue bus. `buffer_start` keeps the ring's write
        // position monotonic across contended-silent buffers: a skipped
        // buffer writes nothing and the phones re-prime, exactly mirroring
        // the silence the room heard.
        let cue_armed = self.cue_ring.armed.load(Ordering::Relaxed);
        let cue_deck_on = s.cue_deck;
        let cue_mode = s.cue_mode;
        let mut cue_pos = buffer_start;
        if cue_armed {
            self.cue_ring.main_rate_bits.store(device_rate.to_bits(), Ordering::Relaxed);
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
            let mut cue = (0.0f32, 0.0f32);
            for (i, d) in s.decks.iter_mut().enumerate() {
                let gain = d.gain.tick(rate) * d.mute.tick(rate);
                let side = if i == 0 { fader.0 } else { fader.1 };
                let deck_rate = d.rate.tick(rate);
                let key_ratio = d.key_ratio.tick(rate) as f64;
                let scratch_rate = d.scratch.tick(rate, deck_rate);
                let scratching = d.scratch.active();
                let mut stem_gain = [0.0f32; STEM_COUNT];
                for ((slot, ramp), blend) in stem_gain
                    .iter_mut()
                    .zip(d.stem_gain.iter_mut())
                    .zip(d.blend_stem.iter_mut())
                {
                    *slot = ramp.tick(rate) * blend.tick(rate);
                }
                let Some(pcm) = deck_pcm[i].as_ref() else { continue };
                if pcm.frames.is_empty() {
                    continue;
                }
                let natural_step = pcm.sample_rate as f64 / device_rate;
                if let Some(splat) = d.splat.as_mut().filter(|splat| splat.active) {
                    // Splat owns source time. Rate, key lock and scratch are
                    // intentionally ignored; the shared master advances at
                    // the track's natural rate and every row derives from it.
                    if !d.playing {
                        continue;
                    }
                    let frame = render_splat_source(
                        splat,
                        pcm,
                        deck_stems[i].as_deref(),
                        stem_gain,
                        natural_step,
                    );
                    let toned = d.eq.process(frame, rate);
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
                // whole point of scrubbing.
                if !scratching && (!d.playing || d.ended) {
                    continue;
                }
                let source = DeckSource {
                    pcm,
                    stems: deck_stems[i].as_deref(),
                    stem_gain,
                };
                let length = pcm.frames.len();

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
                let want_stretch = !scratching
                    && (stretch_ratio - 1.0).abs() > STRETCH_BYPASS_EPSILON
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

                // A span owns the playhead, on both read paths. Wrap BEFORE
                // the read so no frame past the out point is ever emitted.
                if let Some((start, end)) = d.loop_span {
                    if d.playhead_frames() >= end {
                        // Land MODULO the length, keeping the overshoot.
                        // Resetting to IN exactly discards up to a step
                        // per lap — a held loop walks audibly early —
                        // and it is also what catches a playhead
                        // stranded past OUT by a live resize: modulo
                        // continues the subdivision in phase instead of
                        // re-triggering the downbeat at IN.
                        let len = (end - start).max(1.0);
                        let over = (d.playhead_frames() - start).rem_euclid(len);
                        d.seek_frames(start + over);
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
                    d.playing = false;
                    d.ended = true;
                    s.ended_decks.push(if i == 0 { DeckId::A } else { DeckId::B });
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
                        let xf = (LOOP_XFADE_SECS * pcm.sample_rate as f64)
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
                let toned = d.eq.process(frame, rate);
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
                self.cue_ring.push(
                    cue_pos,
                    cue.0.clamp(-CLAMP, CLAMP),
                    cue.1.clamp(-CLAMP, CLAMP),
                );
                cue_pos = cue_pos.saturating_add(1);
            }

            let score = s.score_preview.scratch.get(frame).copied().unwrap_or([0.0; 2]);
            let master = s.master.tick(rate);
            let l = ((video.0 + deck_out[0].0 + deck_out[1].0 + sfx.0 + score[0]) * master)
                .clamp(-CLAMP, CLAMP);
            let r = ((video.1 + deck_out[0].1 + deck_out[1].1 + sfx.1 + score[1]) * master)
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

        // One cue publish per buffer: the phones consumer sees whole
        // buffers or nothing.
        if cue_armed {
            self.cue_ring.write_pos.store(cue_pos, Ordering::Release);
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
        if peaks[METER_MASTER] > f32::EPSILON
            && self
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
        self.transition
            .publish_rendered_frame(self.device_frames.load(Ordering::Acquire));
        self.render_max_nanos
            .fetch_max(render_started.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn render(mixer: &Mixer, rate: f64, frames: usize) -> AudioBuffer {
        let mut buffer = AudioBuffer::new_with_size(frames, 2);
        mixer.render(rate, &mut buffer);
        buffer
    }

    #[test]
    fn score_preview_enters_program_before_master_and_stops_at_end() {
        let Some(bank) = local_drum_bank() else { return };
        let mixer = Mixer::new();
        mixer.run_cmd(MixCmd::SetDrumBank(bank));
        mixer.state.lock().unwrap().master = Ramp::at(1.0);
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

        mixer.state.lock().unwrap().master = Ramp::at(0.0);
        mixer.score_preview_play(sequence);
        let muted = render(&mixer, 48_000.0, 256);
        assert!(muted.channel(0).iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn score_preview_is_block_size_deterministic() {
        let Some(bank) = local_drum_bank() else { return };
        let run = |block: usize| {
            let mixer = Mixer::new();
            mixer.run_cmd(MixCmd::SetDrumBank(bank.clone()));
            mixer.state.lock().unwrap().master = Ramp::at(1.0);
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
        let mixer = Mixer::new();
        mixer.state.lock().unwrap().master = Ramp::at(1.0);
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
        let mixer = Mixer::new();
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
        let mixer = Mixer::new();
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
        let mixer = Mixer::new();
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
        let mixer = Mixer::new();
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
        mixer.set_deck_loop_span(DeckId::B, Some((0.0, 100.0 / 48_000.0)));
        mixer.set_deck_playing(DeckId::B, true);
        render(&mixer, 48_000.0, 1024);
        assert!(mixer.drain_ended_decks().is_empty());
        let (_, _, playing) = mixer.deck_position(DeckId::B);
        assert!(playing);
    }

    #[test]
    fn a_looping_deck_never_runs_past_its_out_point() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000)); // 10 s
        mixer.set_crossfader(0.0);
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 2.0)));
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
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_crossfader(0.0);
        mixer.set_deck_playing(DeckId::A, true);
        render(&mixer, 48_000.0, 48_000); // a second of free play first
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 2.0)));
        for _ in 0..46 {
            render(&mixer, 48_000.0, 4096);
            let (position, _, _) = mixer.deck_position(DeckId::A);
            assert!((1.0..2.0).contains(&position), "escaped at {position}");
        }
    }

    #[test]
    fn the_wrap_is_gapless_on_sustained_material() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_crossfader(0.0);
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 2.0)));
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
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_crossfader(0.0);
        mixer.set_deck_loop_span(DeckId::A, Some((5.0, 6.0)));
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
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_crossfader(0.0);
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 5.0)));
        mixer.seek_deck_seconds(DeckId::A, 3.5);
        mixer.set_deck_playing(DeckId::A, true);
        // Halve out from under the playhead: 3.5 is 2.5 into the old span,
        // which is 0.5 into the new one modulo its length — the subdivision
        // continues instead of re-triggering the downbeat at IN.
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 2.0)));
        render(&mixer, 48_000.0, 256);
        let (position, _, _) = mixer.deck_position(DeckId::A);
        assert!(
            (1.45..1.65).contains(&position),
            "the playhead must keep its phase modulo the new span, got {position}"
        );
    }

    #[test]
    fn a_long_held_loop_does_not_drift_against_its_own_length() {
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        // 44.1k material on a 48k device: the natural step is fractional,
        // so a wrap that discards its overshoot loses ~half a source frame
        // per lap and a held loop walks audibly early over minutes.
        mixer.install_deck(DeckId::A, const_pcm(16_384, 441_000, 44_100));
        mixer.set_crossfader(0.0);
        // A length deliberately NOT commensurate with the 44.1k -> 48k step:
        // a round 0.1 s is exactly 4800 device frames and wraps with zero
        // overshoot, which would hide the discard this test exists to catch.
        mixer.set_deck_loop_span(DeckId::A, Some((0.5, 0.60001)));
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
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_crossfader(0.0);
        mixer.set_deck_loop_span(DeckId::A, Some((5.0, 6.0)));
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
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        // 48_003 frames: a length whose seconds->frames round trip lands a
        // hair ABOVE the frame count, so an unclamped OUT sits past the
        // last frame and the end-of-track check wins over the wrap.
        mixer.install_deck(DeckId::A, const_pcm(16_384, 48_003, 48_000));
        mixer.set_crossfader(0.0);
        mixer.set_deck_loop_span(DeckId::A, Some((0.0, 48_003.0 / 48_000.0)));
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
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000)); // 10 s
        mixer.set_crossfader(0.0);
        // Keylock is the voice default; a non-unity rate engages the
        // stretcher, whose read head cannot reach the last WSOLA window of
        // the track — so a span whose OUT hugs the end can never see
        // playhead >= end and used to die through the ran-out path.
        mixer.set_deck_rate(DeckId::A, 1.05);
        mixer.set_deck_loop_span(DeckId::A, Some((9.0, 10.0)));
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
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 5.0)));
        mixer.seek_deck_seconds(DeckId::A, 3.5);
        // Paused: the render loop skips this deck entirely, so the catch
        // has to happen when the span is SET or the playhead sits parked
        // outside the loop until play is pressed.
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 2.0)));
        let (position, _, _) = mixer.deck_position(DeckId::A);
        assert!(
            (1.49..1.51).contains(&position),
            "a stranded paused playhead lands modulo at set time, got {position}"
        );
    }

    #[test]
    fn a_commanded_jump_is_a_blend_not_a_splice() {
        let mixer = Mixer::new();
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
        let mixer = Mixer::new();
        mixer.set_master(1.0);
        mixer.install_deck(DeckId::A, const_pcm(16_384, 480_000, 48_000));
        mixer.set_deck_loop_span(DeckId::A, Some((1.0, 2.0)));
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
    #[test]
    fn the_blend_overlay_multiplies_and_clears_without_touching_the_knobs() {
        let mixer = Mixer::new();
        // Operator sets a stem lane to 0.8; the autopilot blends it to 0.5.
        mixer.set_deck_stem_gain(DeckId::A, 2, 0.8);
        mixer.set_blend_stem(DeckId::A, 2, 0.5);
        {
            let s = mixer.state.lock().unwrap();
            let d = &s.decks[0];
            assert!((d.stem_gain[2].target() - 0.8).abs() < 1e-6, "the knob stands");
            assert!((d.blend_stem[2].target() - 0.5).abs() < 1e-6, "the hand is on");
        }
        // Clear returns the overlay to unity; the operator's value stands.
        mixer.clear_blend(DeckId::A);
        {
            let s = mixer.state.lock().unwrap();
            let d = &s.decks[0];
            assert!((d.blend_stem[2].target() - 1.0).abs() < 1e-6);
            assert!((d.stem_gain[2].target() - 0.8).abs() < 1e-6);
        }
        // A fresh install snaps the overlay home instantly.
        mixer.set_blend_stem(DeckId::A, 0, 0.0);
        mixer.install_deck(DeckId::A, const_pcm(0, 4800, 48_000));
        {
            let s = mixer.state.lock().unwrap();
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
        mixer: &Mixer,
        state: &mut CueReadState,
        cue_rate: f64,
        frames: usize,
    ) -> AudioBuffer {
        let mut buffer = AudioBuffer::new_with_size(frames, 2);
        mixer.cue_ring().consume(state, cue_rate, &mut buffer);
        buffer
    }

    #[test]
    fn cue_pfl_ignores_gain_mute_and_crossfader() {
        let mixer = Mixer::new();
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
        let mixer = Mixer::new();
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
            let mixer = Mixer::new();
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
        let mixer = Mixer::new();
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

        assert!(mixer.clear_preview().is_some(), "the pcm comes back out");
        assert!(mixer.preview_position().is_none(), "cleared means gone");
    }

    #[test]
    fn cue_ring_servo_survives_mismatched_device_rates() {
        let mixer = Mixer::new();
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

    fn splat_fixture(missing_drums: bool) -> (Mixer, u32) {
        use crate::loop_splat::{SplatCell, SplatSection};

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
        let sections = (0..SPLAT_COLS)
            .map(|col| SplatSection {
                start_secs: col as f64 * 2.0,
                end_secs: (col + 1) as f64 * 2.0,
                bars: 1,
            })
            .collect();
        let mut cells = [[None; SPLAT_COLS]; SPLAT_ROWS];
        for row in SplatRow::ALL {
            for col in 0..SPLAT_COLS {
                cells[row.index()][col] = Some(SplatCell {
                    span: crate::decks::LoopSpan {
                        start_secs: col as f64 * 2.0,
                        end_secs: (col + 1) as f64 * 2.0,
                    },
                    bars: 1,
                    energy: 1.0,
                    silent: false,
                });
            }
        }
        let grid = Arc::new(SplatGrid {
            bpm: 120.0,
            bar_secs: 2.0,
            first_bar_secs: 0.0,
            sections,
            cells,
            bars_per_col: [1; SPLAT_COLS],
        });
        let mixer = Mixer::new();
        mixer.state.lock().unwrap().master = Ramp::at(1.0);
        mixer.install_deck(DeckId::A, pcm);
        mixer.install_deck_stems(DeckId::A, Arc::new(stems));
        mixer.set_deck_splat(DeckId::A, grid);
        mixer.set_deck_splat_enabled(DeckId::A, true);
        mixer.set_deck_playing(DeckId::A, true);
        (mixer, rate)
    }

    fn render_count(mixer: &Mixer, rate: u32, mut frames: usize, block: usize) -> Vec<f32> {
        let mut samples = Vec::with_capacity(frames);
        while frames > 0 {
            let count = frames.min(block);
            let output = render(mixer, rate as f64, count);
            samples.extend_from_slice(output.channel(0));
            frames -= count;
        }
        samples
    }

    #[test]
    fn splat_launch_swap_phase_stop_and_transport_return_are_quantized() {
        let (mixer, rate) = splat_fixture(false);
        render_count(&mixer, rate, 300, 64);
        mixer.splat_launch(DeckId::A, SplatRow::Drums, 0, SplatPart::WHOLE);
        let before = render_count(&mixer, rate, 1_700, 256);
        assert!(before.iter().all(|sample| sample.abs() < 1e-7));
        // The equal-power fade begins on source frame 2000. Its first sample
        // has zero incoming gain; the immediately following sample is live.
        let onset = render_count(&mixer, rate, 2, 64);
        assert!(onset[0].abs() < 1e-7 && onset[1].abs() > 1e-5);

        render_count(&mixer, rate, 500, 64);
        mixer.splat_launch(DeckId::A, SplatRow::Drums, 3, SplatPart::WHOLE);
        render_count(&mixer, rate, 1_504, 256);
        {
            let state = mixer.state.lock().unwrap();
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

        let state = mixer.state.lock().unwrap();
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
        let state = mixer.state.lock().unwrap();
        let anchor = state.decks[0].splat.as_ref().unwrap().rows[SplatRow::Drums.index()]
            .cell
            .unwrap()
            .anchor_frames;
        assert_eq!(anchor, 0.0);
    }

}
