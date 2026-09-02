//! Two music decks + central crossfader: pure routing/state engine.
//!
//! The engine owns deck INTENT (what is loading/loaded where, transport
//! mirrors, tempo/sync/tone/stem settings, the play queue, crossfader
//! position/curve); actual sample playback lives in the mixer, driven by the
//! commands returned here. Everything is deterministic and clock-free so the
//! whole surface is hermetically testable:
//!
//! - tile clicks route to an explicit deck or, on `Auto`, to the inactive
//!   deck — never interrupting the live deck,
//! - per-deck loads are latest-wins by generation; stale decode completions
//!   are ignored,
//! - the crossfader is equal-power (`cos/sin` quarter-cycle) by default,
//!   with a linear curve option and timed fade-to-side moves,
//! - tempo matching is arithmetic over the analysed beat grids: a sync sets
//!   the follower's rate so the audible tempos match, then lands its
//!   playhead on a grid boundary. Auto sync re-runs that whenever the master
//!   changes, and steps aside the moment the operator touches the follower's
//!   own pitch.
//!
//! The engine never reads a clock: the host feeds deck positions in through
//! [`DeckEngine::observe`], so every sync decision in the tests is exactly
//! the decision the running app makes.

use crate::loop_splat::{SplatGrid, SplatPart, SplatRow, SplatSnapshot, SPLAT_COLS};
use crate::wave_analysis::TrackGrid;
use makepad_asset_data::{AssetId, AssetRevisionId, BlobId, MediaType};
use std::sync::Arc;

pub type DeckGen = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeckId {
    A,
    B,
}

impl DeckId {
    pub fn other(self) -> DeckId {
        match self {
            DeckId::A => DeckId::B,
            DeckId::B => DeckId::A,
        }
    }
    pub fn index(self) -> usize {
        match self {
            DeckId::A => 0,
            DeckId::B => 1,
        }
    }
}

/// Explicit routing choice for a tile click.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DeckTarget {
    #[default]
    Auto,
    A,
    B,
    /// Clicks load nothing: the operator wants to pick rows and drag them
    /// to a deck or the queue by hand.
    Off,
    /// Auto, and then some: the pick plays and the console fades to it.
    Mix,
}

/// Analysis the STORE already holds for a track, as blob references off its
/// manifest: four Ogg Vorbis stems in `FileRole::STEMS` order (drums, bass,
/// vocals, other) and the word-aligned lyrics JSON.
///
/// This is the fetch-or-compute switch. Present means the expensive work was
/// done once, somewhere, and this deck downloads a few hundred kilobytes
/// instead of spending a third of the track's duration on the GPU; absent
/// means the local separation/bake path runs exactly as it always has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackSideChannels {
    /// `(blob, byte_len)` per stem, in `FileRole::STEMS` order. The contract
    /// is all-four-or-none, so this is one option over the whole set.
    pub stems: Option<[(BlobId, u64); 4]>,
    pub lyrics: Option<(BlobId, u64)>,
}

/// What a music tile resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackItem {
    pub asset: AssetId,
    pub revision: AssetRevisionId,
    pub title: String,
    pub media_blob: BlobId,
    pub media_len: u64,
    pub media: MediaType,
    /// Precomputed stems/lyrics on the store, when this revision carries any.
    pub side: TrackSideChannels,
}

/// Crossfader gain law — the shape of the hand-over between the decks.
///
/// The set a DJ mixer offers, in the order the panel lists them. What
/// separates them is where the two gains cross and how much of the sweep
/// they spend at full: a dipped curve crosses BELOW unity and audibly sags
/// through the middle, an equal-power curve crosses at 0.707 and holds the
/// perceived loudness flat, and the cut curves hand over in a few
/// millimetres of travel and sit at full everywhere else — which is what
/// makes a fader scratchable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FadeCurve {
    /// Constant perceived loudness: `gain_a=cos(x·π/2)`, `gain_b=sin(x·π/2)`.
    #[default]
    EqualPower,
    Linear,
    /// Crosses well under unity: the mix sags through the middle, which is
    /// what you want when the two tracks would otherwise pile up.
    Dipped,
    /// Full for most of the sweep, with long gentle shoulders.
    SlowFade,
    /// Shorter shoulders: a hand-over inside a third of the travel.
    SlowCut,
    /// The scratch curve: full within a few millimetres of the end stop.
    FastCut,
    /// One deck at full while the other walks: a straight hand-over at the
    /// halfway point, each deck owning its own half outright.
    Transition,
}

/// The dropdown's order, and the words on its rows.
pub const FADE_CURVES: [(FadeCurve, &str); 7] = [
    (FadeCurve::EqualPower, "Equal power"),
    (FadeCurve::Linear, "Linear"),
    (FadeCurve::Dipped, "Dipped"),
    (FadeCurve::SlowFade, "Slow fade"),
    (FadeCurve::SlowCut, "Slow cut"),
    (FadeCurve::FastCut, "Fast cut"),
    (FadeCurve::Transition, "Transition"),
];

/// A ramp from 0 to 1 across `[from, to]`, eased at both ends. The cut
/// curves are this ramp with the shoulders pulled in.
fn shoulder(x: f32, from: f32, to: f32) -> f32 {
    if to <= from {
        return if x < from { 0.0 } else { 1.0 };
    }
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Per-deck gains for a crossfader position in [0,1] (0 = full A, 1 = full B).
pub fn crossfader_gains(pos: f32, curve: FadeCurve) -> (f32, f32) {
    let x = pos.clamp(0.0, 1.0);
    match curve {
        FadeCurve::EqualPower => {
            let angle = x * std::f32::consts::FRAC_PI_2;
            (angle.cos(), angle.sin())
        }
        FadeCurve::Linear => (1.0 - x, x),
        // Crossing at 0.35 rather than 0.5: the dip is the point.
        FadeCurve::Dipped => ((1.0 - x).powf(1.5), x.powf(1.5)),
        // Each deck reaches full at the midpoint and holds it to the far end.
        FadeCurve::SlowFade => (1.0 - shoulder(x, 0.5, 1.0), shoulder(x, 0.0, 0.5)),
        FadeCurve::SlowCut => (1.0 - shoulder(x, 0.72, 1.0), shoulder(x, 0.0, 0.28)),
        FadeCurve::FastCut => (1.0 - shoulder(x, 0.94, 1.0), shoulder(x, 0.0, 0.06)),
        // Flat until the halfway point, then a straight walk down — and the
        // mirror of that for the other deck.
        FadeCurve::Transition => {
            let a = if x <= 0.5 { 1.0 } else { 2.0 - 2.0 * x };
            let b = if x >= 0.5 { 1.0 } else { 2.0 * x };
            (a, b)
        }
    }
}

/// xorshift64*: tiny, deterministic, and plenty for picking queue rows.
fn xorshift64star(mut x: u64) -> u64 {
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum DeckLoad {
    #[default]
    Empty,
    /// Media fetch + decode in flight for this generation.
    Loading { gen: DeckGen, item: TrackItem },
    Loaded { item: TrackItem },
    Failed { item: TrackItem, error: String },
}

/// Number of stem lanes a separated track carries.
pub const STEM_COUNT: usize = crate::music_dsp::STEM_COUNT;

/// Pitch slider travel. The narrow range is the everyday one; the wide range
/// is for pulling a stubborn track into line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PitchRange {
    #[default]
    Narrow,
    Wide,
}

impl PitchRange {
    pub fn fraction(self) -> f64 {
        match self {
            PitchRange::Narrow => 0.08,
            PitchRange::Wide => 0.16,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            PitchRange::Narrow => "±8%",
            PitchRange::Wide => "±16%",
        }
    }
    pub fn toggled(self) -> PitchRange {
        match self {
            PitchRange::Narrow => PitchRange::Wide,
            PitchRange::Wide => PitchRange::Narrow,
        }
    }
}

/// Widest tempo ratio a sync will ever ask for before it tries half/double
/// time instead.
const SYNC_RATE_MIN: f64 = 0.80;
const SYNC_RATE_MAX: f64 = 1.25;
/// Hard clamp on any rate the engine emits.
pub const RATE_MIN: f64 = 0.25;
pub const RATE_MAX: f64 = 4.0;
/// How far the key can be shifted, in semitones either way. An octave: past
/// that a mix has left the track behind anyway.
pub const KEY_SHIFT_MAX: f64 = 12.0;

/// What the pointer is doing to a deck's waveform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScratchMotion {
    /// A hand landed on the record: brake to a stop.
    Grab,
    /// Scrub at this rate; negative runs backwards.
    Move { rate: f32 },
    /// Let go: spin back up to the deck's tempo.
    Release,
}

/// How tightly a sync lands the follower.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncQuantize {
    /// Nearest beat — at most half a beat of jump, safe under a playing deck.
    Beat,
    /// Nearest downbeat — aligns bars, for a deck that is cued or stopped.
    Bar,
}

/// Everything the sync arithmetic needs about one deck.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyncView {
    pub grid: TrackGrid,
    pub position_secs: f64,
    pub rate: f64,
}

/// The result of a tempo match: the follower's new rate, and where to put
/// its playhead so the grids line up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyncPlan {
    pub rate: f64,
    /// `None` when the follower is already in phase.
    pub seek_secs: Option<f64>,
}

/// Tempo-match `follower` to `leader` and phase-align it.
///
/// The rate makes the audible tempos equal; halving or doubling keeps the
/// ratio inside a musical range when the two tracks are an octave apart.
/// The seek moves the follower to the nearest grid boundary whose phase
/// matches the leader's, so it is never more than half a unit of jump.
pub fn sync_plan(
    leader: &SyncView,
    follower: &SyncView,
    quantize: SyncQuantize,
) -> Option<SyncPlan> {
    if !leader.grid.has_grid() || !follower.grid.has_grid() {
        return None;
    }
    let target_bpm = leader.grid.bpm * leader.rate;
    let mut rate = target_bpm / follower.grid.bpm;
    if !rate.is_finite() || rate <= 0.0 {
        return None;
    }
    // Half/double time: a 150 BPM track under a 75 BPM one plays at 1.0, not
    // 0.5 — the grids still line up, one beat in two.
    while rate > SYNC_RATE_MAX {
        rate *= 0.5;
    }
    while rate < SYNC_RATE_MIN {
        rate *= 2.0;
    }
    let rate = rate.clamp(RATE_MIN, RATE_MAX);

    // Phase, in whole units of the chosen quantization.
    let (leader_units, follower_units, unit_secs) = match quantize {
        SyncQuantize::Beat => (
            leader.grid.beat_at(leader.position_secs),
            follower.grid.beat_at(follower.position_secs),
            follower.grid.beat_secs,
        ),
        SyncQuantize::Bar => (
            leader.grid.bar_at(leader.position_secs),
            follower.grid.bar_at(follower.position_secs),
            follower.grid.beat_secs * 4.0,
        ),
    };
    let leader_phase = leader_units.rem_euclid(1.0);
    let to_secs = |units: f64| match quantize {
        SyncQuantize::Beat => follower.grid.secs_at_beat(units),
        SyncQuantize::Bar => {
            follower.grid.secs_at_beat(units * 4.0 - follower.grid.downbeat_phase as f64)
        }
    };
    let mut want_units = (follower_units - leader_phase).round() + leader_phase;
    let mut want_secs = to_secs(want_units);
    // The nearest in-phase landing can fall before the start of the file
    // (a deck cued at zero, a leader late in its bar). Step forward to the
    // first one that exists rather than refusing to sync.
    let mut guard = 0;
    while want_secs < 0.0 && guard < 64 {
        want_units += 1.0;
        want_secs = to_secs(want_units);
        guard += 1;
    }
    if want_secs < 0.0 {
        return Some(SyncPlan { rate, seek_secs: None });
    }
    let drift = want_secs - follower.position_secs;
    // Do not move for a difference nobody can hear (a thousandth of a unit).
    let seek_secs = (drift.abs() > unit_secs * 0.001).then_some(want_secs);
    Some(SyncPlan { rate, seek_secs })
}

/// How far a deck's rate may be trimmed to hold phase against an external
/// clock, on top of the tempo match. A percent or two is inaudible under a
/// beat; more than that and the room hears the deck wobble.
const EXT_PHASE_TRIM: f64 = 0.02;
/// Fraction of the phase error taken per beat of following.
const EXT_PHASE_GAIN: f64 = 0.25;
/// A deck further out of phase than this was not drifting, it was MOVED (a
/// seek, a scratch, a fresh EXT engage). Land it rather than trim for bars.
pub const EXT_RESEEK_BEATS: f64 = 0.25;

/// What a deck should do to keep following an external clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExternalFollow {
    /// Rate to run at: the tempo match plus the phase trim.
    pub rate: f64,
    /// Phase error in external beats; positive = the deck is behind.
    pub error_beats: f64,
    /// False when the tempo match asks for more stretch than this deck's
    /// pitch envelope allows — the external tempo has walked out of range
    /// and the operator has to see that, not silently hear it.
    pub within_envelope: bool,
    /// Set when the deck is too far out to trim back.
    pub reseek_secs: Option<f64>,
}

/// Tempo-match and phase-follow a deck to an EXTERNAL clock (the room's
/// beat, as the disciplined clock publishes it).
///
/// This is deck-to-deck [`sync_plan`] with the leader replaced by a clock
/// nobody controls, and one difference that matters: the correction is
/// spent as a bounded RATE TRIM rather than a seek, because the external
/// clock is continuous by contract and a deck chasing it must be too. A
/// seek is only for the case where the deck was moved out from under the
/// lock.
pub fn external_follow(
    external: &SyncView,
    follower: &SyncView,
    envelope: f64,
) -> Option<ExternalFollow> {
    if !external.grid.has_grid() || !follower.grid.has_grid() {
        return None;
    }
    let target_bpm = external.grid.bpm * external.rate;
    let mut rate = target_bpm / follower.grid.bpm;
    if !rate.is_finite() || rate <= 0.0 {
        return None;
    }
    // Half/double time, exactly as the deck-to-deck path folds it; `fold`
    // remembers how many deck beats one external beat became.
    let mut fold = 1.0f64;
    while rate > SYNC_RATE_MAX {
        rate *= 0.5;
        fold *= 0.5;
    }
    while rate < SYNC_RATE_MIN {
        rate *= 2.0;
        fold *= 2.0;
    }
    let within_envelope = (rate - 1.0).abs() <= envelope + 1e-9;
    let rate = rate.clamp(RATE_MIN, RATE_MAX);

    // Phase, in EXTERNAL beats: the deck's beat counter runs `fold` times
    // faster than the external one, so divide before comparing.
    let external_beats = external.grid.beat_at(external.position_secs);
    let follower_beats = follower.grid.beat_at(follower.position_secs) / fold.max(1e-9);
    let mut error = (external_beats - follower_beats).rem_euclid(1.0);
    if error > 0.5 {
        error -= 1.0;
    }
    let reseek_secs = match error.abs() > EXT_RESEEK_BEATS {
        true => sync_plan(external, follower, SyncQuantize::Beat).and_then(|plan| plan.seek_secs),
        false => None,
    };
    let trim = (error * EXT_PHASE_GAIN).clamp(-EXT_PHASE_TRIM, EXT_PHASE_TRIM);
    Some(ExternalFollow {
        rate: (rate * (1.0 + trim)).clamp(RATE_MIN, RATE_MAX),
        error_beats: error,
        within_envelope,
        reseek_secs,
    })
}

/// The shortest loop worth having. Below this the two seam ramps the mixer
/// puts either side of the wrap are most of the span, so a shorter loop
/// would be all edge and no music.
pub const LOOP_MIN_SECS: f64 = 0.05;

/// What a deck is willing to spend on a DERIVED product — separation,
/// transcription. Three states, and the middle one is the interesting
/// one: use what already exists, but never start the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessMode {
    /// Compute it if it is missing, and use it.
    Live,
    /// Never compute it; use it only if it is already there.
    Cached,
    /// Neither compute it nor use it.
    Off,
}

impl ProcessMode {
    /// The click order: live, then cached, then off, then round again.
    pub fn next(self) -> ProcessMode {
        match self {
            ProcessMode::Live => ProcessMode::Cached,
            ProcessMode::Cached => ProcessMode::Off,
            ProcessMode::Off => ProcessMode::Live,
        }
    }

    /// May a worker be started for this?
    pub fn computes(self) -> bool {
        matches!(self, ProcessMode::Live)
    }

    /// May the product be heard or seen, however it got here?
    pub fn shows(self) -> bool {
        !matches!(self, ProcessMode::Off)
    }
}

/// The armed count past 64: the BOOKMARK rung. `[` drops an in point
/// with no out — no loop, just a position saved as a marker to jump to.
pub const LOOP_BEATS_INF: u32 = u32::MAX;

/// How many loops a track can keep as blue markers. Eight is more than a
/// set ever needs and small enough that the strip stays readable.
pub const LOOP_SLOT_CAP: usize = 8;

/// How many scanner-found loops a track keeps as yellow markers. Twice the
/// blue cap: a "find the 10 best" scan must fit with room to spare.
pub const FOUND_LOOP_CAP: usize = 16;

/// A loop, in SOURCE seconds — the timebase the beat grid and the wave
/// tiles already share, so neither placing one nor drawing one has to
/// convert.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopSpan {
    pub start_secs: f64,
    pub end_secs: f64,
}

#[derive(Clone, Debug)]
pub struct SplatUiState {
    pub grid: Arc<SplatGrid>,
    pub enabled: bool,
    pub last: SplatSnapshot,
}

impl LoopSpan {
    pub fn len_secs(&self) -> f64 {
        self.end_secs - self.start_secs
    }
}

/// What a deck's SYNC control is set to. The control cycles through these.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SyncMode {
    /// Free: the deck runs at its own tempo (or the operator's pitch).
    #[default]
    Off,
    /// Held against the sync master — the classic DJ sync.
    Deck,
    /// This deck IS the group's tempo reference: never corrected, its moves
    /// carry every follower.
    Master,
    /// Held against the room: the VJ's published beat clock, so a deck can
    /// be played over another DJ or a live source.
    External,
}

/// Transport mirror of one deck. `playing/loop/mute/gain` echo what the
/// mixer was last told; the mixer's device clock stays the position truth
/// (the host mirrors it back in through [`DeckEngine::observe`]).
#[derive(Clone, Debug)]
pub struct DeckState {
    pub load: DeckLoad,
    pub playing: bool,
    /// The operator flipped the grid onto the other pulse (see
    /// [`DeckEngine::flip_beat_phase`]); a second flip undoes the first.
    pub phase_flipped: bool,
    /// Armed loop length in beats; 0 = MAN, free placement. This says what
    /// `[` and `]` will do NEXT and nothing else — a running manual span
    /// has no beat count to describe it.
    pub loop_beats: u32,
    /// The loop that is actually running. A deck loops when it has a span;
    /// a bool beside one is a second truth that can disagree with the first.
    pub loop_span: Option<LoopSpan>,
    /// MAN: an IN point is placed and `]` is waiting to close it.
    pub loop_armed: Option<f64>,
    /// The last span, so RELOOP can get back into it after an exit.
    pub loop_memory: Option<LoopSpan>,
    /// Saved loops — the blue markers. Positions on THIS track, so a
    /// fresh install clears them with the span.
    pub loop_slots: Vec<LoopSpan>,
    /// Scanner-found loops — the yellow markers on the strip's bottom
    /// edge. Positions on THIS track; a fresh install clears them.
    pub found_loops: Vec<LoopSpan>,
    /// Where CUE sends the deck — the red marker. A position on THIS
    /// track, so a fresh install puts it back at the top.
    pub cue_secs: f64,
    /// The current BOOKMARK — an in point with no out, placed by `[` on
    /// the infinity rung. Green until its chip is clicked into the saved
    /// row. Mutually exclusive with a running span: the count dial
    /// converts one into the other.
    pub bookmark: Option<f64>,
    pub muted: bool,
    pub gain: f32,
    /// Level-match trim for the track on this deck, measured from its own
    /// audio. 1.0 until something measures it; spent only while NORMALISE
    /// is latched, so the fader still reads what the operator set.
    pub norm_gain: f32,
    pub duration_secs: f64,
    /// Analysed beat grid, once the worker has one.
    pub grid: Option<TrackGrid>,
    pub splat: Option<SplatUiState>,
    /// Source-time playhead, mirrored from the mixer.
    pub position_secs: f64,
    /// Playback rate multiplier; 1.0 = the track's own tempo.
    pub rate: f64,
    /// Operator pitch offset as a fraction (−0.08 = 8% slow).
    pub pitch: f64,
    pub pitch_range: PitchRange,
    /// Tempo/phase are being held against the other deck.
    pub synced: bool,
    /// Tempo/phase are being held against the EXTERNAL clock — the room's
    /// beat rather than the other deck.
    pub ext_sync: bool,
    /// The operator moved this deck's own pitch, so auto sync leaves it be
    /// until it is loaded again or synced by hand.
    pub auto_opt_out: bool,
    /// Tempo changes keep the key (time stretch on) instead of running the
    /// tape faster. It also decides where a key shift is measured from: with
    /// the lock on, from the track's own key; with it off, from whatever the
    /// tempo already did to the pitch.
    pub keylock: bool,
    /// Operator key shift in SEMITONES: pitch without tempo. 0 = the track's
    /// own key.
    pub key_shift: f64,
    /// A pointer is on the waveform.
    pub scratching: bool,
    /// Three-band tone control, 1.0 = unity, 0.0 = killed.
    pub eq: [f32; 3],
    /// Which bands the kill buttons are holding down.
    pub eq_kill: [bool; 3],
    /// Soloed bands, the isolator move: any solo silences the bands
    /// outside the set, additively, and a band's own mute still wins.
    pub eq_solo: [bool; 3],
    /// Bipolar sweep filter; 0.5 = off.
    pub filter: f32,
    /// Per-stem gains, in [`crate::music_dsp::StemKind`] order.
    pub stem_gain: [f32; STEM_COUNT],
    pub stem_kill: [bool; STEM_COUNT],
    /// Soloed lanes. Any solo active silences every lane outside the set;
    /// solos are additive, and a lane's own mute still beats its solo.
    pub stem_solo: [bool; STEM_COUNT],
    /// Separated stems are loaded and the stem knobs are live.
    pub stems_ready: bool,
    /// The separation switch. Deck intent, so it survives track loads.
    pub stems_mode: ProcessMode,
    /// Generation of the load currently on this deck: late-arriving
    /// analysis for an older load is dropped.
    pub load_gen: DeckGen,
}

impl Default for DeckState {
    fn default() -> Self {
        Self {
            load: DeckLoad::Empty,
            playing: false,
            loop_beats: 4,
            phase_flipped: false,
            loop_span: None,
            loop_armed: None,
            loop_memory: None,
            loop_slots: Vec::new(),
            found_loops: Vec::new(),
            cue_secs: 0.0,
            bookmark: None,
            muted: false,
            gain: 1.0,
            norm_gain: 1.0,
            duration_secs: 0.0,
            grid: None,
            splat: None,
            position_secs: 0.0,
            rate: 1.0,
            pitch: 0.0,
            pitch_range: PitchRange::Narrow,
            synced: false,
            ext_sync: false,
            auto_opt_out: false,
            keylock: true,
            key_shift: 0.0,
            scratching: false,
            eq: [1.0; 3],
            eq_kill: [false; 3],
            eq_solo: [false; 3],
            filter: 0.5,
            stem_gain: [1.0; STEM_COUNT],
            stem_kill: [false; STEM_COUNT],
            stem_solo: [false; STEM_COUNT],
            stems_ready: false,
            stems_mode: ProcessMode::Live,
            load_gen: 0,
        }
    }
}

impl DeckState {
    /// What this deck should actually be sent: the fader the operator set,
    /// times the level-match trim when NORMALISE is asking for one.
    pub fn effective_gain(&self, normalise: bool) -> f32 {
        if normalise {
            (self.gain * self.norm_gain).clamp(0.0, 1.5)
        } else {
            self.gain
        }
    }
    pub fn is_loaded(&self) -> bool {
        matches!(self.load, DeckLoad::Loaded { .. })
    }

    /// A deck loops when it has a span, and only then.
    pub fn loop_on(&self) -> bool {
        self.loop_span.is_some()
    }

    pub fn title(&self) -> Option<&str> {
        match &self.load {
            DeckLoad::Empty => None,
            DeckLoad::Loading { item, .. }
            | DeckLoad::Loaded { item }
            | DeckLoad::Failed { item, .. } => Some(item.title.as_str()),
        }
    }

    pub fn item(&self) -> Option<&TrackItem> {
        match &self.load {
            DeckLoad::Empty => None,
            DeckLoad::Loading { item, .. }
            | DeckLoad::Loaded { item }
            | DeckLoad::Failed { item, .. } => Some(item),
        }
    }

    /// The tempo actually being played, or `None` without a grid.
    pub fn effective_bpm(&self) -> Option<f64> {
        self.grid
            .filter(|grid| grid.has_grid())
            .map(|grid| grid.effective_bpm(self.rate))
    }

    /// A view for the sync arithmetic.
    pub fn sync_view(&self) -> Option<SyncView> {
        let grid = self.grid.filter(|grid| grid.has_grid())?;
        Some(SyncView {
            grid,
            position_secs: self.position_secs,
            rate: self.rate,
        })
    }

    /// The gain a band knob resolves to once its kill button is applied.
    pub fn eq_effective(&self, band: usize) -> f32 {
        if self.eq_kill.get(band).copied().unwrap_or(false) {
            return 0.0;
        }
        let any_solo = self.eq_solo.iter().any(|solo| *solo);
        if any_solo && !self.eq_solo.get(band).copied().unwrap_or(false) {
            return 0.0;
        }
        self.eq.get(band).copied().unwrap_or(1.0)
    }

    /// The gain a stem knob resolves to. With no stems loaded the knobs are
    /// inert and the deck plays the full mix. Console law: the lane's own
    /// mute wins first, then an active solo set silences everyone outside
    /// it, then the knob has its say.
    pub fn stem_effective(&self, stem: usize) -> f32 {
        if !self.stems_ready {
            return 1.0;
        }
        if self.stem_kill.get(stem).copied().unwrap_or(false) {
            return 0.0;
        }
        let any_solo = self.stem_solo.iter().any(|solo| *solo);
        if any_solo && !self.stem_solo.get(stem).copied().unwrap_or(false) {
            return 0.0;
        }
        self.stem_gain.get(stem).copied().unwrap_or(1.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeckCmd {
    /// Fetch + decode the track for `deck` under `gen` (stale results drop).
    LoadTrack { deck: DeckId, gen: DeckGen, item: TrackItem },
    /// Install the decoded track on the mixer deck voice, paused at zero.
    InstallTrack { deck: DeckId },
    SetPlaying { deck: DeckId, playing: bool },
    SeekFraction { deck: DeckId, fraction: f64 },
    /// The deck's loop span in source seconds, or `None` to run free.
    SetLoopSpan { deck: DeckId, span: Option<LoopSpan> },
    SetMute { deck: DeckId, muted: bool },
    SetGain { deck: DeckId, gain: f32 },
    /// Jump the crossfader (mixer slews internally against clicks).
    SetCrossfader { position: f32 },
    /// Ramp the crossfader to `position` over `secs`.
    FadeCrossfader { position: f32, secs: f32 },
    SetCurve { curve: FadeCurve },
    /// Swap the two mixer deck voices (contents, transport, everything).
    SwapVoices,
    /// Playback rate multiplier (tempo). Pitch-preserving when key lock is on.
    SetRate { deck: DeckId, rate: f64 },
    /// Absolute playhead in source seconds.
    SeekSeconds { deck: DeckId, secs: f64 },
    /// Pointer on the waveform: vinyl-style rate override.
    Scratch { deck: DeckId, motion: ScratchMotion },
    /// Keep the key when the tempo changes (time stretch) or let it slide.
    SetKeylock { deck: DeckId, on: bool },
    /// Key shift in semitones: pitch WITHOUT tempo.
    SetKeyShift { deck: DeckId, semitones: f64 },
    /// One tone band, 0 = kill, 1 = unity.
    SetEqBand { deck: DeckId, band: usize, gain: f32 },
    /// Bipolar sweep filter; 0.5 = off.
    SetFilter { deck: DeckId, position: f32 },
    /// One stem lane's gain, 0 = muted.
    SetStemGain { deck: DeckId, stem: usize, gain: f32 },
    SplatSet { deck: DeckId, grid: Arc<SplatGrid> },
    SplatEnable { deck: DeckId, on: bool },
    SplatLaunch { deck: DeckId, row: SplatRow, col: u8, part: SplatPart },
    /// `timed`: wait for the next bar; otherwise stop at once.
    SplatStopRow { deck: DeckId, row: SplatRow, timed: bool },
    SplatLaunchScene { deck: DeckId, col: u8 },
    SplatStopAll { deck: DeckId, timed: bool },
    /// Drop the deck's track entirely: mixer voice cleared, host mirrors
    /// wiped. The channel strip stands, exactly as it does across a load.
    UnloadTrack { deck: DeckId },
}

pub struct DeckEngine {
    decks: [DeckState; 2],
    next_gen: DeckGen,
    /// Crossfader position intent (0 = A, 1 = B).
    pub crossfader: f32,
    /// Level-matching: every deck is sent its trim as well as its fader, so
    /// a quiet master does not vanish beside a loud one. Off by default —
    /// the fader means what it says until the operator asks for this.
    pub normalise: bool,
    pub curve: FadeCurve,
    /// Deck that most recently received a load, for Auto tie-breaks.
    last_loaded: Option<DeckId>,
    /// Hold the non-leading deck to the leader's grid without being asked.
    pub auto_sync: bool,
    /// QUANT's unit in beats, 0 = off. One global value: snapping is a
    /// property of how the operator is working, not of a deck.
    pub snap_beats: u32,
    /// Tracks queued for the next free deck, in play order.
    queue: Vec<TrackItem>,
    /// Fill an idle deck from the queue as soon as one frees up.
    pub auto_load_queue: bool,
    /// Recycle finished tracks to the queue tail (read by the autopilot's
    /// hand-back; the engine itself never requeues on its own).
    pub repeat: bool,
    /// Random queue picks instead of the head.
    pub shuffle: bool,
    /// xorshift64* state for the shuffle draw. Seeded by the host once at
    /// startup; tests seed a constant, which is what keeps the draw
    /// assertable.
    shuffle_rng: u64,
    /// The asset the last hand-back pushed, spared from the very next
    /// shuffle draw so a two-track queue alternates instead of repeating.
    last_requeued: Option<AssetId>,
    /// The deck an autopilot fade is retiring: auto sync must not re-seek
    /// it when the fader crosses the middle and leadership flips.
    auto_fade_hold: Option<DeckId>,
    /// The deck the sync group follows, PINNED at the first successful lock
    /// so corrections never change direction mid-mix. The crossfader
    /// heuristic only elects; once elected, the master stands until it is
    /// ejected, replaced by handover, or the group dissolves.
    sync_master: Option<DeckId>,
    /// How long a SeekSeconds takes to reach the audio (UI pump + command
    /// delivery + the next block). A phase landing is computed from
    /// positions that are this stale, so the follower is placed where the
    /// lock will be true when the seek LANDS, not where it was true when it
    /// was computed. 0 = uncompensated (the tests' frame of reference).
    pub land_lookahead_secs: f64,
}

impl Default for DeckEngine {
    fn default() -> Self {
        Self {
            decks: [DeckState::default(), DeckState::default()],
            next_gen: 0,
            crossfader: 0.0,
            normalise: false,
            curve: FadeCurve::EqualPower,
            last_loaded: None,
            auto_sync: true,
            snap_beats: 0,
            queue: Vec::new(),
            auto_load_queue: true,
            repeat: false,
            shuffle: false,
            shuffle_rng: 1,
            last_requeued: None,
            auto_fade_hold: None,
            sync_master: None,
            land_lookahead_secs: 0.0,
        }
    }
}

impl DeckEngine {
    pub fn new() -> DeckEngine {
        DeckEngine::default()
    }

    pub fn deck(&self, id: DeckId) -> &DeckState {
        &self.decks[id.index()]
    }

    pub fn splat(&self, deck: DeckId) -> Option<&SplatUiState> {
        self.deck(deck).splat.as_ref()
    }

    fn deck_mut(&mut self, id: DeckId) -> &mut DeckState {
        &mut self.decks[id.index()]
    }

    pub fn splat_set(&mut self, deck: DeckId, grid: Arc<SplatGrid>) -> Vec<DeckCmd> {
        if !self.deck(deck).is_loaded() {
            return Vec::new();
        }
        let enabled = self.splat(deck).is_some_and(|splat| splat.enabled);
        let last = self.splat(deck).map(|splat| splat.last).unwrap_or_default();
        self.deck_mut(deck).splat = Some(SplatUiState {
            grid: grid.clone(),
            enabled,
            last,
        });
        vec![DeckCmd::SplatSet { deck, grid }]
    }

    pub fn splat_enable(&mut self, deck: DeckId, on: bool) -> Vec<DeckCmd> {
        let Some(splat) = self.deck_mut(deck).splat.as_mut() else { return Vec::new() };
        splat.enabled = on;
        vec![DeckCmd::SplatEnable { deck, on }]
    }

    pub fn splat_launch(
        &mut self,
        deck: DeckId,
        row: SplatRow,
        col: u8,
        part: SplatPart,
    ) -> Vec<DeckCmd> {
        let Some(splat) = self.splat(deck) else { return Vec::new() };
        let col_index = col as usize;
        if !part.is_valid()
            || col_index >= SPLAT_COLS
            || splat.grid.cells[row.index()][col_index].is_none_or(|cell| cell.silent)
        {
            return Vec::new();
        }
        vec![DeckCmd::SplatLaunch { deck, row, col, part }]
    }

    pub fn splat_stop_row(&mut self, deck: DeckId, row: SplatRow, timed: bool) -> Vec<DeckCmd> {
        self.splat(deck)
            .is_some()
            .then_some(DeckCmd::SplatStopRow { deck, row, timed })
            .into_iter()
            .collect()
    }

    pub fn splat_scene(&mut self, deck: DeckId, col: u8) -> Vec<DeckCmd> {
        if self.splat(deck).is_none() || col as usize >= SPLAT_COLS {
            return Vec::new();
        }
        vec![DeckCmd::SplatLaunchScene { deck, col }]
    }

    pub fn splat_stop_all(&mut self, deck: DeckId, timed: bool) -> Vec<DeckCmd> {
        self.splat(deck)
            .is_some()
            .then_some(DeckCmd::SplatStopAll { deck, timed })
            .into_iter()
            .collect()
    }

    pub fn observe_splat(&mut self, deck: DeckId, snapshot: Option<SplatSnapshot>) {
        if let (Some(state), Some(snapshot)) = (self.deck_mut(deck).splat.as_mut(), snapshot) {
            state.enabled = snapshot.active;
            state.last = snapshot;
        }
    }

    /// The deck a new track should land on when the caller says `Auto`:
    /// never the live one. Preference order — an empty deck, then a
    /// non-playing deck, then the deck the crossfader is turned away from,
    /// then the deck that was loaded less recently.
    pub fn auto_target(&self) -> DeckId {
        let a = self.deck(DeckId::A);
        let b = self.deck(DeckId::B);
        let empty = |d: &DeckState| matches!(d.load, DeckLoad::Empty);
        match (empty(a), empty(b)) {
            (true, false) => return DeckId::A,
            (false, true) => return DeckId::B,
            // Both empty: the fader says nothing about content — tie-break.
            (true, true) => {
                return match self.last_loaded {
                    Some(DeckId::A) => DeckId::B,
                    _ => DeckId::A,
                }
            }
            (false, false) => {}
        }
        match (a.playing, b.playing) {
            (false, true) => return DeckId::A,
            (true, false) => return DeckId::B,
            _ => {}
        }
        // Epsilon comparison: at dead center cos/sin differ by ulps only,
        // and that must be a tie, not a side.
        let (gain_a, gain_b) = crossfader_gains(self.crossfader, self.curve);
        if gain_b - gain_a > 1e-5 {
            return DeckId::A;
        }
        if gain_a - gain_b > 1e-5 {
            return DeckId::B;
        }
        match self.last_loaded {
            Some(DeckId::A) => DeckId::B,
            _ => DeckId::A,
        }
    }

    /// The deck the last click, drop or pump put a track on.
    pub fn last_loaded(&self) -> Option<DeckId> {
        self.last_loaded
    }

    /// Route a tile click. The chosen deck starts loading latest-wins; the
    /// other deck is untouched.
    pub fn click(&mut self, item: TrackItem, target: DeckTarget) -> Vec<DeckCmd> {
        // OFF is a hands-off list: nothing loads from a click, so a row can
        // be picked up and dragged where the operator wants it instead.
        if target == DeckTarget::Off {
            return Vec::new();
        }
        let deck = match target {
            DeckTarget::A => DeckId::A,
            DeckTarget::B => DeckId::B,
            DeckTarget::Auto | DeckTarget::Off | DeckTarget::Mix => self.auto_target(),
        };
        self.next_gen += 1;
        let gen = self.next_gen;
        self.last_loaded = Some(deck);
        let state = self.deck_mut(deck);
        state.load = DeckLoad::Loading { gen, item: item.clone() };
        state.load_gen = gen;
        // A new track brings its own grid; the old one must not linger and
        // sync the next load to a tempo it never had. Tone and stem knobs
        // stay where the operator left them, like a real channel strip.
        state.grid = None;
        state.splat = None;
        state.position_secs = 0.0;
        state.synced = false;
        state.auto_opt_out = false;
        state.stems_ready = false;
        state.scratching = false;
        vec![DeckCmd::LoadTrack { deck, gen, item }]
    }

    /// Decode finished for `(deck, gen)`. Stale generations are dropped.
    pub fn track_ready(&mut self, deck: DeckId, gen: DeckGen, duration_secs: f64) -> Vec<DeckCmd> {
        let normalise = self.normalise;
        let state = self.deck_mut(deck);
        let DeckLoad::Loading { gen: want, item } = state.load.clone() else {
            return Vec::new();
        };
        if want != gen {
            return Vec::new();
        }
        state.load = DeckLoad::Loaded { item };
        state.playing = false;
        state.duration_secs = duration_secs;
        state.position_secs = 0.0;
        state.splat = None;
        // A span was measured against the OUTGOING track's beats and means
        // nothing on this one, so it goes — along with anything half-placed
        // or remembered. The armed LENGTH is the operator's, and stays.
        state.loop_span = None;
        state.loop_armed = None;
        state.loop_memory = None;
        state.loop_slots.clear();
        state.found_loops.clear();
        state.cue_secs = 0.0;
        state.bookmark = None;
        // Fresh installs inherit the whole standing channel-strip intent:
        // transport, tone, stems and the rate the pitch slider is sitting at.
        let mut cmds = vec![
            DeckCmd::InstallTrack { deck },
            DeckCmd::SetLoopSpan { deck, span: None },
            DeckCmd::SetMute { deck, muted: state.muted },
            DeckCmd::SetGain { deck, gain: state.effective_gain(normalise) },
            DeckCmd::SetKeylock { deck, on: state.keylock },
            DeckCmd::SetRate { deck, rate: state.rate },
            DeckCmd::SetKeyShift { deck, semitones: state.key_shift },
            DeckCmd::SetFilter { deck, position: state.filter },
        ];
        for band in 0..3 {
            cmds.push(DeckCmd::SetEqBand { deck, band, gain: state.eq_effective(band) });
        }
        for stem in 0..STEM_COUNT {
            cmds.push(DeckCmd::SetStemGain { deck, stem, gain: state.stem_effective(stem) });
        }
        cmds
    }

    pub fn track_failed(&mut self, deck: DeckId, gen: DeckGen, error: String) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        if let DeckLoad::Loading { gen: want, item } = state.load.clone() {
            if want == gen {
                state.load = DeckLoad::Failed { item, error };
                state.playing = false;
                state.duration_secs = 0.0;
            }
        }
        Vec::new()
    }

    pub fn play_pause(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        if !matches!(state.load, DeckLoad::Loaded { .. }) {
            return Vec::new();
        }
        state.playing = !state.playing;
        let mut cmds = vec![DeckCmd::SetPlaying { deck, playing: state.playing }];
        // Starting a deck changes who is leading, so the grid lock is
        // re-decided here rather than waiting for the next observation.
        cmds.extend(self.apply_auto_sync());
        cmds
    }

    pub fn seek(&mut self, deck: DeckId, fraction: f64) -> Vec<DeckCmd> {
        if !matches!(self.deck(deck).load, DeckLoad::Loaded { .. }) {
            return Vec::new();
        }
        vec![DeckCmd::SeekFraction { deck, fraction: fraction.clamp(0.0, 1.0) }]
    }

    /// RELOOP / EXIT, the CDJ's third loop button. Drop out of the running
    /// loop keeping it for later, or jump back into the last one. Inert
    /// with nothing to return to — the fall-out-and-slam-back-in move is
    /// the whole reason this is not just another way to spell "off".
    pub fn toggle_loop(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        if let Some(span) = state.loop_span.take() {
            state.loop_memory = Some(span);
            return vec![DeckCmd::SetLoopSpan { deck, span: None }];
        }
        let Some(span) = state.loop_memory else { return Vec::new() };
        state.loop_span = Some(span);
        vec![
            DeckCmd::SeekSeconds { deck, secs: span.start_secs },
            DeckCmd::SetLoopSpan { deck, span: Some(span) },
        ]
    }

    /// Seconds one armed loop is worth on this deck, or `None` when the
    /// count is MAN or the track has no grid to measure a beat against.
    fn armed_secs(&self, deck: DeckId) -> Option<f64> {
        let state = self.deck(deck);
        if state.loop_beats == 0 || state.loop_beats == LOOP_BEATS_INF {
            return None;
        }
        let grid = state.grid.filter(|grid| grid.has_grid())?;
        Some(grid.beat_secs * state.loop_beats as f64)
    }

    /// A span is only worth engaging if it fits inside the track and is
    /// long enough to be music rather than two seam ramps back to back.
    fn usable_span(&self, deck: DeckId, start: f64, end: f64) -> Option<LoopSpan> {
        let duration = self.deck(deck).duration_secs;
        if start < 0.0 || end > duration || end - start < LOOP_MIN_SECS {
            return None;
        }
        Some(LoopSpan { start_secs: start, end_secs: end })
    }

    /// Engage `span`. With `seek`, jump to IN when the playhead is not
    /// already inside it: `[` engages from IN and is silent until the first
    /// wrap; `]` engages from OUT and so wraps immediately.
    ///
    /// A resize passes `seek: false` and emits only the span. Two reasons:
    /// `position_secs` here is a 20 Hz mirror, up to ~50 ms stale, so any
    /// seek computed from it lands the phase wrong — and the mixer's wrap
    /// is modulo the length, so a playhead stranded past the new OUT gets
    /// caught IN PHASE on the next audio callback, continuing the
    /// subdivision instead of re-triggering the downbeat at IN.
    fn engage_loop(&mut self, deck: DeckId, span: LoopSpan, seek: bool) -> Vec<DeckCmd> {
        let position = self.deck(deck).position_secs;
        let state = self.deck_mut(deck);
        state.loop_span = Some(span);
        state.loop_armed = None;
        state.bookmark = None;
        state.loop_memory = Some(span);
        let mut cmds = Vec::new();
        if seek && (position < span.start_secs || position >= span.end_secs) {
            cmds.push(DeckCmd::SeekSeconds { deck, secs: span.start_secs });
        }
        cmds.push(DeckCmd::SetLoopSpan { deck, span: Some(span) });
        cmds
    }

    /// `[` — set IN here. With a beat count armed the loop closes itself N
    /// beats later; in MAN it waits for `]`.
    pub fn loop_in(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let position = self.deck(deck).position_secs;
        if self.deck(deck).loop_beats == LOOP_BEATS_INF {
            // The bookmark rung: `[` places the current bookmark — GREEN,
            // like a fresh loop, and clicking its chip is what saves it.
            // It replaces a running loop the way `[` always replaces.
            let state = self.deck_mut(deck);
            state.bookmark = Some(position);
            state.loop_armed = None;
            if state.loop_span.take().is_some() {
                return vec![DeckCmd::SetLoopSpan { deck, span: None }];
            }
            return Vec::new();
        }
        if let Some(len) = self.armed_secs(deck) {
            return match self.usable_span(deck, position, position + len) {
                Some(span) => self.engage_loop(deck, span, true),
                None => Vec::new(),
            };
        }
        // A beat count with no grid behind it has nothing honest to do —
        // and must not fall through into arming MAN.
        if self.deck(deck).loop_beats != 0 {
            return Vec::new();
        }
        self.deck_mut(deck).loop_armed = Some(position);
        Vec::new()
    }

    /// `]` — set OUT here. With a beat count armed IN lands N beats BACK,
    /// so the phrase you just heard becomes the loop; in MAN it closes
    /// whatever `[` armed.
    pub fn loop_out(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let position = self.deck(deck).position_secs;
        if let Some(len) = self.armed_secs(deck) {
            return match self.usable_span(deck, position - len, position) {
                Some(span) => self.engage_loop(deck, span, true),
                None => Vec::new(),
            };
        }
        if self.deck(deck).loop_beats != 0 {
            return Vec::new();
        }
        let Some(armed) = self.deck(deck).loop_armed else { return Vec::new() };
        match self.usable_span(deck, armed, position) {
            Some(span) => self.engage_loop(deck, span, true),
            None => {
                // Scrubbed back behind the arm, or landed on top of it:
                // re-arm here rather than close a span that runs backwards.
                self.deck_mut(deck).loop_armed = Some(position);
                Vec::new()
            }
        }
    }

    /// `<` and `>` differ only in the factor. They move the armed count AND
    /// cut the running loop by the same factor, anchored on IN. The cut is
    /// on DURATION, not beat count, so an off-grid manual span cuts just as
    /// well as a measured one — and for a beat loop the two are the same
    /// arithmetic anyway.
    fn loop_scale(&mut self, deck: DeckId, factor: f64) -> Vec<DeckCmd> {
        let beats = self.deck(deck).loop_beats;
        // The bookmark rung's transitions come first: they change what the
        // current object IS, never its size, and the direct pick owns
        // that logic.
        if factor >= 1.0 && (beats == 64 || beats == LOOP_BEATS_INF) {
            return self.set_loop_beats(deck, LOOP_BEATS_INF);
        }
        if factor < 1.0 && beats == LOOP_BEATS_INF {
            return self.set_loop_beats(deck, 64);
        }
        // Halving 1 lands on MAN rather than sticking at 1, and doubling
        // out of MAN has to special-case zero or it would stay there.
        let next = match (factor < 1.0, beats) {
            (true, _) => beats / 2,
            (false, 0) => 1,
            (false, _) => (beats * 2).min(64),
        };
        let Some(span) = self.deck(deck).loop_span else {
            self.deck_mut(deck).loop_beats = next;
            return Vec::new();
        };
        let end = span.start_secs + span.len_secs() * factor;
        let Some(resized) = self.usable_span(deck, span.start_secs, end) else {
            // Refused. The count holds too: it is supposed to describe what
            // the brackets will do, and a loop that would not fit is not it.
            return Vec::new();
        };
        let state = self.deck_mut(deck);
        state.loop_beats = next;
        // A resized loop that IS a saved marker carries the marker along:
        // the blue chip keeps aiming at the same IN, and its stored
        // duration follows what the ear now hears.
        if let Some(slot) = state.loop_slots.iter_mut().find(|slot| {
            (slot.start_secs - span.start_secs).abs() < 1e-6
                && (slot.end_secs - span.end_secs).abs() < 1e-6
        }) {
            *slot = resized;
        }
        self.engage_loop(deck, resized, false)
    }

    pub fn loop_halve(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        self.loop_scale(deck, 0.5)
    }

    pub fn loop_double(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        self.loop_scale(deck, 2.0)
    }

    /// Drag the running loop somewhere else. The span keeps its length —
    /// this is a move, not a resize — and QUANT measures the translation
    /// against the span's OWN in point, so a snapped drag walks the loop
    /// by whole units and it stays the same musical object.
    pub fn move_loop(&mut self, deck: DeckId, start_secs: f64) -> Vec<DeckCmd> {
        let Some(span) = self.deck(deck).loop_span else { return Vec::new() };
        let unit = self.snap_beats;
        let start = match self.deck(deck).grid {
            Some(grid) => grid.snap_translate(start_secs, span.start_secs, unit),
            None => start_secs,
        };
        let Some(moved) = self.usable_span(deck, start, start + span.len_secs()) else {
            // Off the end. Ignored rather than clamped: clamping would
            // slide the loop off the phase the snap just preserved, so the
            // band simply stops at the last position that fits.
            return Vec::new();
        };
        // The playhead rides along, keeping its place inside the loop, so
        // the move is audible the moment it commits.
        let position = self.deck(deck).position_secs;
        let inside = position >= span.start_secs && position < span.end_secs;
        let state = self.deck_mut(deck);
        state.loop_span = Some(moved);
        state.loop_memory = Some(moved);
        let mut cmds = Vec::new();
        if inside {
            let secs = moved.start_secs + (position - span.start_secs);
            state.position_secs = secs;
            // A raw seek on purpose: the ride is derived, already in phase
            // because the whole span moved by whole units, and an auto-sync
            // re-lock has no business firing inside a hand gesture.
            cmds.push(DeckCmd::SeekSeconds { deck, secs });
        }
        cmds.push(DeckCmd::SetLoopSpan { deck, span: Some(moved) });
        cmds
    }

    /// A direct pick from the count dropdown — and the one owner of the
    /// bookmark rung's conversions, which the `<` `>` dial delegates to.
    /// Picking the infinity count collapses a running loop to a bookmark
    /// at its IN; picking a length out of infinity grows the out point
    /// back and loop mode with it; picking a length over a running loop
    /// resizes it in place, marker following.
    pub fn set_loop_beats(&mut self, deck: DeckId, beats: u32) -> Vec<DeckCmd> {
        let current = self.deck(deck).loop_beats;
        if beats == current {
            return Vec::new();
        }
        if beats == LOOP_BEATS_INF {
            self.deck_mut(deck).loop_beats = LOOP_BEATS_INF;
            let state = self.deck_mut(deck);
            let Some(span) = state.loop_span.take() else { return Vec::new() };
            state.bookmark = Some(span.start_secs);
            state.loop_memory = Some(span);
            return vec![DeckCmd::SetLoopSpan { deck, span: None }];
        }
        self.deck_mut(deck).loop_beats = beats;
        if current == LOOP_BEATS_INF {
            let Some(inpoint) = self.deck(deck).bookmark else { return Vec::new() };
            let Some(len) = self.armed_secs(deck) else { return Vec::new() };
            return match self.usable_span(deck, inpoint, inpoint + len) {
                Some(span) => self.engage_loop(deck, span, true),
                None => Vec::new(),
            };
        }
        if beats == 0 {
            return Vec::new();
        }
        let Some(span) = self.deck(deck).loop_span else { return Vec::new() };
        let Some(len) = self.armed_secs(deck) else { return Vec::new() };
        let Some(resized) = self.usable_span(deck, span.start_secs, span.start_secs + len)
        else {
            return Vec::new();
        };
        let state = self.deck_mut(deck);
        if let Some(slot) = state.loop_slots.iter_mut().find(|slot| {
            (slot.start_secs - span.start_secs).abs() < 1e-6
                && (slot.end_secs - span.end_secs).abs() < 1e-6
        }) {
            *slot = resized;
        }
        self.engage_loop(deck, resized, false)
    }

    /// Marks read back from disk on a track's install.
    pub fn restore_loop_slots(&mut self, deck: DeckId, mut slots: Vec<LoopSpan>) {
        slots.truncate(LOOP_SLOT_CAP);
        self.deck_mut(deck).loop_slots = slots;
    }

    /// REMOVE USER LOOPS: drop every blue mark and the bookmark in one act
    /// — the operator's own marks, gone. No stash to press again: the scan
    /// dialog's CANCEL is the undo now, and a second meaning for this call
    /// would only fight it. The running span keeps sounding; this touches
    /// memory, never audio.
    pub fn clear_loop_slots(&mut self, deck: DeckId) {
        let state = self.deck_mut(deck);
        state.loop_slots.clear();
        state.bookmark = None;
    }

    /// Put a snapshot of the operator's marks back — CANCEL's undo path,
    /// cap-respecting like `restore_loop_slots` because a snapshot taken
    /// before a restore-from-disk could carry more than the row holds.
    pub fn restore_marks(
        &mut self,
        deck: DeckId,
        mut slots: Vec<LoopSpan>,
        bookmark: Option<f64>,
    ) {
        slots.truncate(LOOP_SLOT_CAP);
        let state = self.deck_mut(deck);
        state.loop_slots = slots;
        state.bookmark = bookmark;
    }

    /// Dragging the red marker: move where CUE sends the deck. Under a
    /// QUANT unit the drag steps in whole units against the cue's own
    /// phase — the same law as dragging the loop band — and exact with
    /// QUANT off. Nothing sounds until the CUE button is pressed.
    pub fn set_cue(&mut self, deck: DeckId, secs: f64) {
        let unit = self.snap_beats;
        let state = self.deck(deck);
        let target = match state.grid {
            Some(grid) => grid.snap_translate(secs, state.cue_secs, unit),
            None => secs,
        };
        let duration = state.duration_secs;
        let state = self.deck_mut(deck);
        state.cue_secs = if duration > 0.0 {
            target.clamp(0.0, duration)
        } else {
            target.max(0.0)
        };
    }

    /// The green marker click: keep the running span as a blue marker.
    /// Deduped and capped; returns whether anything was added.
    pub fn save_loop(&mut self, deck: DeckId) -> bool {
        let state = self.deck_mut(deck);
        let span = match (state.loop_span, state.bookmark) {
            (Some(span), _) => span,
            (None, Some(mark)) => LoopSpan { start_secs: mark, end_secs: mark },
            (None, None) => return false,
        };
        let same = |a: &LoopSpan| {
            (a.start_secs - span.start_secs).abs() < 1e-6
                && (a.end_secs - span.end_secs).abs() < 1e-6
        };
        if state.loop_slots.iter().any(same) || state.loop_slots.len() >= LOOP_SLOT_CAP {
            return false;
        }
        state.loop_slots.push(span);
        true
    }

    /// Dragging a blue marker off its spot: forget that saved loop. The
    /// running span is untouched — this deletes the memory, not the sound.
    pub fn delete_loop_slot(&mut self, deck: DeckId, index: usize) {
        let state = self.deck_mut(deck);
        if index < state.loop_slots.len() {
            state.loop_slots.remove(index);
        }
    }

    /// The blue marker click: go into that loop again, running loop or
    /// not — and if that loop IS the one running, the second click exits
    /// it, the same gesture as the RELOOP/EXIT button.
    pub fn recall_loop(&mut self, deck: DeckId, index: usize) -> Vec<DeckCmd> {
        let Some(span) = self.deck(deck).loop_slots.get(index).copied() else {
            return Vec::new();
        };
        if span.len_secs() < 1e-9 {
            // A bookmark: an exact jump to the point, engaging nothing.
            return self.seek_secs(deck, span.start_secs);
        }
        if let Some(running) = self.deck(deck).loop_span {
            if (running.start_secs - span.start_secs).abs() < 1e-6
                && (running.end_secs - span.end_secs).abs() < 1e-6
            {
                return self.toggle_loop(deck);
            }
        }
        self.engage_loop(deck, span, true)
    }

    /// Scanner results land here, replacing the previous scan wholesale —
    /// a re-scan IS the clear. Capped like the blue row.
    pub fn install_found_loops(&mut self, deck: DeckId, mut spans: Vec<LoopSpan>) {
        spans.truncate(FOUND_LOOP_CAP);
        self.deck_mut(deck).found_loops = spans;
    }

    /// The yellow marker click: same contract as the blue one — engage,
    /// and a second click on the running one exits.
    pub fn recall_found(&mut self, deck: DeckId, index: usize) -> Vec<DeckCmd> {
        let Some(span) = self.deck(deck).found_loops.get(index).copied() else {
            return Vec::new();
        };
        if span.len_secs() < 1e-9 {
            return self.seek_secs(deck, span.start_secs);
        }
        if let Some(running) = self.deck(deck).loop_span {
            if (running.start_secs - span.start_secs).abs() < 1e-6
                && (running.end_secs - span.end_secs).abs() < 1e-6
            {
                return self.toggle_loop(deck);
            }
        }
        self.engage_loop(deck, span, true)
    }

    /// Dragging a yellow marker off its spot: forget that finding. The
    /// running span is untouched — memory, not sound.
    pub fn delete_found(&mut self, deck: DeckId, index: usize) {
        let state = self.deck_mut(deck);
        if index < state.found_loops.len() {
            state.found_loops.remove(index);
        }
    }

    /// The × button: forget the loop entirely, including the memory RELOOP
    /// would have returned to.
    pub fn loop_clear(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        let had_span = state.loop_span.take().is_some();
        state.loop_armed = None;
        state.loop_memory = None;
        match had_span {
            true => vec![DeckCmd::SetLoopSpan { deck, span: None }],
            false => Vec::new(),
        }
    }

    pub fn toggle_mute(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        state.muted = !state.muted;
        vec![DeckCmd::SetMute { deck, muted: state.muted }]
    }

    /// The per-deck separation switch. Standing it fully down also stands
    /// the knobs down (stems_ready), so `stem_effective` is unity again and
    /// a late worker result cannot re-arm them.
    pub fn set_stems_mode(&mut self, deck: DeckId, mode: ProcessMode) {
        let state = self.deck_mut(deck);
        state.stems_mode = mode;
        if !mode.shows() {
            state.stems_ready = false;
        }
    }

    /// The measured trim for a freshly loaded track.
    pub fn set_norm_gain(&mut self, deck: DeckId, gain: f32) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        state.norm_gain = gain.clamp(0.05, 4.0);
        let gain = self.deck(deck).effective_gain(self.normalise);
        vec![DeckCmd::SetGain { deck, gain }]
    }

    /// The NORMALISE latch. Both decks are re-sent at once: the switch has
    /// to land on what is already playing, not only on the next load.
    pub fn set_normalise(&mut self, on: bool) -> Vec<DeckCmd> {
        self.normalise = on;
        [DeckId::A, DeckId::B]
            .into_iter()
            .map(|deck| DeckCmd::SetGain { deck, gain: self.deck(deck).effective_gain(on) })
            .collect()
    }

    pub fn set_gain(&mut self, deck: DeckId, gain: f32) -> Vec<DeckCmd> {
        let gain = gain.clamp(0.0, 1.5);
        self.deck_mut(deck).gain = gain;
        let gain = self.deck(deck).effective_gain(self.normalise);
        vec![DeckCmd::SetGain { deck, gain }]
    }

    pub fn set_crossfader(&mut self, position: f32) -> Vec<DeckCmd> {
        self.crossfader = position.clamp(0.0, 1.0);
        vec![DeckCmd::SetCrossfader { position: self.crossfader }]
    }

    /// Timed move to one side (the "fade to A/B" performance buttons).
    ///
    /// `secs` is the time for a FULL sweep, not for this particular move: a
    /// fader already halfway there takes half of it. The hand is asking for
    /// a RATE — the same travel speed whether the fader has the whole width
    /// to cross or a sliver — which is what makes the duration mean anything
    /// when it is spent from wherever the last move left off.
    ///
    /// `crossfader` is NOT jumped to the target here: the fade takes the
    /// operator's chosen seconds, and this field is what the on-screen fader
    /// mirrors. Landing it now made the fader teleport while the audio was
    /// still crossing — the move looked instant and untrusted even though it
    /// was running. The host walks it across (`track_crossfade`) instead.
    pub fn fade_to(&mut self, deck: DeckId, secs: f32) -> Vec<DeckCmd> {
        let position = match deck {
            DeckId::A => 0.0,
            DeckId::B => 1.0,
        };
        let distance = (position - self.crossfader).abs().clamp(0.0, 1.0);
        vec![DeckCmd::FadeCrossfader { position, secs: secs.max(0.0) * distance }]
    }

    pub fn set_curve(&mut self, curve: FadeCurve) -> Vec<DeckCmd> {
        self.curve = curve;
        vec![DeckCmd::SetCurve { curve }]
    }

    /// Start a deck if it can start: a start, never a toggle, so a caller
    /// that acts on last tick's observation cannot accidentally pause.
    pub fn play(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        if !matches!(state.load, DeckLoad::Loaded { .. }) || state.playing {
            return Vec::new();
        }
        state.playing = true;
        let mut cmds = vec![DeckCmd::SetPlaying { deck, playing: true }];
        cmds.extend(self.apply_auto_sync());
        cmds
    }

    /// Place the playhead for a planned transition. Unlike `seek_secs` this
    /// does NOT re-run auto sync: a cue is a cue, and the one phase lock the
    /// autopilot wants runs explicitly through `sync()`.
    pub fn cue_deck(&mut self, deck: DeckId, secs: f64) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        if !matches!(state.load, DeckLoad::Loaded { .. }) {
            return Vec::new();
        }
        let secs = secs.clamp(0.0, state.duration_secs.max(0.0));
        state.position_secs = secs;
        vec![DeckCmd::SeekSeconds { deck, secs }]
    }

    /// Retire a deck's track: load to Empty so the queue can take the deck.
    /// A load in flight is never ejected — latest-wins holds. Resets what
    /// `click()` resets; the channel strip stands.
    pub fn eject(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        if matches!(self.deck(deck).load, DeckLoad::Loading { .. }) {
            return Vec::new();
        }
        if self.auto_fade_hold == Some(deck) {
            self.auto_fade_hold = None;
        }
        // Retire the generation too: analysis, stems and lyrics for the
        // ejected load may still be in flight, and every host guard keys on
        // load_gen alone — a stale arrival must not dress an Empty deck in
        // the retired track's waveform and stems.
        self.next_gen += 1;
        let gen = self.next_gen;
        let state = self.deck_mut(deck);
        state.load = DeckLoad::Empty;
        state.load_gen = gen;
        state.playing = false;
        state.duration_secs = 0.0;
        state.grid = None;
        state.splat = None;
        state.position_secs = 0.0;
        state.synced = false;
        state.ext_sync = false;
        state.auto_opt_out = false;
        state.stems_ready = false;
        state.scratching = false;
        // An ejected master hands the pin to the remaining group member
        // (or the group ends with it).
        if self.sync_master == Some(deck) {
            let other = deck.other();
            let state = self.deck(other);
            self.sync_master = (state.synced
                && state.is_loaded()
                && state.sync_view().is_some())
            .then_some(other);
        }
        vec![DeckCmd::UnloadTrack { deck }]
    }

    /// Hold auto sync off the retiring deck while an autopilot fade runs:
    /// once the fader crosses the middle, leadership flips and the standing
    /// auto sync would beat-seek the still-audible outgoing track.
    pub fn begin_auto_fade(&mut self, out: DeckId) {
        self.auto_fade_hold = Some(out);
    }

    pub fn end_auto_fade(&mut self) {
        self.auto_fade_hold = None;
    }

    /// Swap deck contents AND invert the fader so the audible program is
    /// unchanged by the swap.
    pub fn swap(&mut self) -> Vec<DeckCmd> {
        self.decks.swap(0, 1);
        self.last_loaded = self.last_loaded.map(DeckId::other);
        self.crossfader = 1.0 - self.crossfader;
        vec![
            DeckCmd::SwapVoices,
            DeckCmd::SetCrossfader { position: self.crossfader },
        ]
    }

    /// Mixer reports a deck ran off the end with looping off. With queue
    /// auto-load on, the next queued track takes the free deck.
    pub fn track_ended(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        if matches!(state.load, DeckLoad::Loaded { .. }) {
            state.playing = false;
        }
        if self.auto_load_queue {
            return self.pump_queue();
        }
        Vec::new()
    }

    // ---- analysis -----------------------------------------------------------

    /// The whole-track analysis landed. Stale generations are dropped; a
    /// fresh grid is what makes the deck syncable, so auto sync re-runs.
    pub fn grid_ready(&mut self, deck: DeckId, gen: DeckGen, grid: TrackGrid) -> Vec<DeckCmd> {
        {
            let state = self.deck_mut(deck);
            if state.load_gen != gen || !matches!(state.load, DeckLoad::Loaded { .. }) {
                return Vec::new();
            }
            state.grid = Some(grid);
        }
        self.apply_auto_sync()
    }

    /// The stem separation for this deck's track is available.
    pub fn stems_ready(&mut self, deck: DeckId, gen: DeckGen) -> Vec<DeckCmd> {
        if !self.deck(deck).stems_mode.shows() {
            return Vec::new();
        }
        let state = self.deck_mut(deck);
        if state.load_gen != gen || !matches!(state.load, DeckLoad::Loaded { .. }) {
            return Vec::new();
        }
        state.stems_ready = true;
        (0..STEM_COUNT)
            .map(|stem| DeckCmd::SetStemGain { deck, stem, gain: state.stem_effective(stem) })
            .collect()
    }

    /// Mirror the mixer's playheads back in. This is the engine's only
    /// notion of time: every sync decision is made against these numbers.
    pub fn observe(&mut self, deck: DeckId, position_secs: f64, playing: bool) {
        let state = self.deck_mut(deck);
        state.position_secs = position_secs.max(0.0);
        state.playing = playing;
    }

    // ---- tempo, sync, scratch ----------------------------------------------

    /// The deck the other one should follow. A pinned master stands first:
    /// once a lock has engaged, corrections must keep the same direction
    /// however the crossfader moves. Only with no master does the audible
    /// heuristic elect one.
    pub fn sync_leader(&self) -> Option<DeckId> {
        self.sync_master_valid().or_else(|| self.heuristic_leader())
    }

    /// The pinned master, only while it can actually lead (loaded, with a
    /// grid). Playing is deliberately not required here: a paused master
    /// still owns the group's tempo — the pump's handover is what moves the
    /// pin onto a playing deck.
    fn sync_master_valid(&self) -> Option<DeckId> {
        self.sync_master.filter(|id| {
            let state = self.deck(*id);
            state.is_loaded() && state.sync_view().is_some()
        })
    }

    /// The deck the group's master is, as the UI reads it.
    pub fn sync_master(&self) -> Option<DeckId> {
        self.sync_master_valid()
    }

    /// Whichever deck is audibly leading: a playing deck beats a stopped
    /// one; when both play, the side the crossfader favours.
    fn heuristic_leader(&self) -> Option<DeckId> {
        let a = self.deck(DeckId::A);
        let b = self.deck(DeckId::B);
        let ready = |state: &DeckState| state.is_loaded() && state.sync_view().is_some();
        match (ready(a) && a.playing, ready(b) && b.playing) {
            (true, false) => return Some(DeckId::A),
            (false, true) => return Some(DeckId::B),
            (false, false) => return None,
            (true, true) => {}
        }
        let (gain_a, gain_b) = crossfader_gains(self.crossfader, self.curve);
        if gain_a - gain_b > 1e-5 {
            Some(DeckId::A)
        } else if gain_b - gain_a > 1e-5 {
            Some(DeckId::B)
        } else {
            // Dead centre: the deck that has been playing keeps the grid.
            self.last_loaded.map(DeckId::other)
        }
    }

    /// Keep the pin honest: drop a master that can no longer lead, and hand
    /// the pin to a playing group member when the master has stopped — a
    /// paused playhead is a frozen phase, and a servo chasing it would drag
    /// a live deck backwards. Called once per pump and from the events that
    /// change who could lead.
    fn refresh_sync_master(&mut self) {
        let Some(master) = self.sync_master else { return };
        let valid = {
            let state = self.deck(master);
            state.is_loaded() && state.sync_view().is_some()
        };
        let successor = |engine: &DeckEngine| {
            let other = master.other();
            let state = engine.deck(other);
            (state.synced && state.is_loaded() && state.sync_view().is_some())
                .then_some(other)
        };
        if !valid {
            self.sync_master = successor(self);
            return;
        }
        if !self.deck(master).playing {
            if let Some(next) = successor(self).filter(|id| self.deck(*id).playing) {
                self.sync_master = Some(next);
            }
        }
    }

    /// Set the follower's rate + phase to the leader's grid. `manual` marks
    /// an operator SYNC press, which also clears an auto-sync opt-out.
    pub fn sync(&mut self, follower: DeckId, manual: bool) -> Vec<DeckCmd> {
        let Some(leader) = self.sync_leader().filter(|id| *id != follower) else {
            return Vec::new();
        };
        if manual {
            self.deck_mut(follower).auto_opt_out = false;
        }
        self.sync_to(leader, follower)
    }

    fn sync_to(&mut self, leader: DeckId, follower: DeckId) -> Vec<DeckCmd> {
        self.sync_to_with(leader, follower, None)
    }

    fn sync_to_with(
        &mut self,
        leader: DeckId,
        follower: DeckId,
        quantize: Option<SyncQuantize>,
    ) -> Vec<DeckCmd> {
        let (Some(lead), Some(follow)) = (
            self.deck(leader).sync_view(),
            self.deck(follower).sync_view(),
        ) else {
            return Vec::new();
        };
        // A deck that is stopped or cued can take a whole-bar jump; one that
        // is already playing gets the gentler nearest-beat landing. A caller
        // that just moved a playhead asks for the nearest beat explicitly,
        // so the operator lands where they clicked, give or take half a beat.
        let quantize = quantize.unwrap_or(if self.deck(follower).playing {
            SyncQuantize::Beat
        } else {
            SyncQuantize::Bar
        });
        let Some(plan) = sync_plan(&lead, &follow, quantize) else {
            return Vec::new();
        };
        // The first successful lock PINS the master: from here the group has
        // one fixed reference, and the crossfader stops re-deciding who
        // corrects whom at every event.
        if self.sync_master_valid().is_none() {
            self.sync_master = Some(leader);
        }
        // A paused leader is a frozen phase: match the tempo so the decks
        // run together when it starts, but never jump a playhead to align
        // with a playhead that is not moving. The play() re-lock lands the
        // phase when the leader actually runs.
        let leader_playing = self.deck(leader).playing;
        let lookahead = self.land_lookahead_secs;
        let mut cmds = Vec::new();
        let state = self.deck_mut(follower);
        state.synced = true;
        if (state.rate - plan.rate).abs() > 1e-9 {
            state.rate = plan.rate;
            // Show the operator the rate the sync chose on the pitch slider.
            state.pitch = (plan.rate - 1.0).clamp(-0.5, 0.5);
            cmds.push(DeckCmd::SetRate { deck: follower, rate: plan.rate });
        }
        // A hand on the record owns the playhead; the phase lock waits.
        if !state.scratching && leader_playing && state.playing {
            if let Some(secs) = plan.seek_secs {
                // Land where the lock is true when the seek ARRIVES: both
                // decks keep moving while the command crosses to the audio
                // thread, so an uncompensated landing is late by exactly
                // that much, every time.
                let secs = secs + plan.rate * lookahead;
                state.position_secs = secs;
                cmds.push(DeckCmd::SeekSeconds { deck: follower, secs });
            }
        } else if !state.scratching && !state.playing {
            // A stopped follower can be placed freely — no lookahead: it is
            // not moving, so the landing cannot go stale.
            if let Some(secs) = plan.seek_secs {
                state.position_secs = secs;
                cmds.push(DeckCmd::SeekSeconds { deck: follower, secs });
            }
        }
        cmds
    }

    /// Re-run auto sync: hold every non-leading deck to the leader's grid.
    pub fn apply_auto_sync(&mut self) -> Vec<DeckCmd> {
        self.apply_auto_sync_with(None)
    }

    /// The same, with an explicit landing granularity. After a playhead
    /// move the caller asks for [`SyncQuantize::Beat`], so re-locking never
    /// drags the deck more than half a beat from where it was put.
    pub fn apply_auto_sync_with(&mut self, quantize: Option<SyncQuantize>) -> Vec<DeckCmd> {
        if !self.auto_sync {
            return Vec::new();
        }
        let Some(leader) = self.sync_leader() else {
            return Vec::new();
        };
        let follower = leader.other();
        // A deck an autopilot fade is retiring must not be re-seeked when
        // the fader crosses the middle and leadership flips onto it.
        if self.auto_fade_hold == Some(follower) {
            return Vec::new();
        }
        let state = self.deck(follower);
        if !state.is_loaded() || state.auto_opt_out || state.scratching {
            return Vec::new();
        }
        if state.sync_view().is_none() {
            return Vec::new();
        }
        self.sync_to_with(leader, follower, quantize)
    }

    // ---- external sync (the room is the leader) -----------------------------

    /// What the deck's SYNC control currently reads. Master outranks Deck:
    /// a synced deck holding the pin is the reference, not a follower.
    pub fn sync_mode(&self, deck: DeckId) -> SyncMode {
        let state = self.deck(deck);
        if state.ext_sync {
            return SyncMode::External;
        }
        if !state.synced {
            return SyncMode::Off;
        }
        match self.sync_master_valid() == Some(deck) {
            true => SyncMode::Master,
            false => SyncMode::Deck,
        }
    }

    /// Any deck following the room. While one is, the loopback detector is
    /// the thing that knows where the beat is, so it must NOT be parked.
    pub fn any_external_sync(&self) -> bool {
        self.decks.iter().any(|state| state.ext_sync)
    }

    /// The SYNC control is a plain toggle: join the sync group, leave it.
    /// (EXT is its own toggle — `toggle_ext_sync` — not a hidden third
    /// position that a "make sure it's on" second press falls into.)
    ///
    /// Joining with another deck to follow locks to it; joining with
    /// nothing to follow pins THIS deck as the waiting master, so the
    /// press on the leading deck is never a dead button: it claims the
    /// reference the next deck will lock to. Leaving hands the pin to the
    /// remaining group member and opts the deck out of the standing auto
    /// sync — off means off until asked again.
    pub fn toggle_sync(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        if self.deck(deck).ext_sync {
            let state = self.deck_mut(deck);
            state.ext_sync = false;
            state.auto_opt_out = true;
            return Vec::new();
        }
        if self.deck(deck).synced {
            // Leave the group.
            let state = self.deck_mut(deck);
            state.synced = false;
            state.auto_opt_out = true;
            if self.sync_master == Some(deck) {
                let other = deck.other();
                let state = self.deck(other);
                self.sync_master = (state.synced
                    && state.is_loaded()
                    && state.sync_view().is_some())
                .then_some(other);
            } else if !self.deck(deck.other()).synced {
                // The last follower left: the group is dissolved.
                self.sync_master = None;
            }
            return Vec::new();
        }
        // Join. A deck that cannot hold a grid cannot be in the group.
        if !self.deck(deck).is_loaded() || self.deck(deck).sync_view().is_none() {
            return Vec::new();
        }
        self.deck_mut(deck).auto_opt_out = false;
        match self.sync_leader().filter(|id| *id != deck) {
            Some(_) => self.sync(deck, true),
            None => {
                // Nothing to follow: this deck IS the reference.
                self.sync_master = Some(deck);
                self.deck_mut(deck).synced = true;
                Vec::new()
            }
        }
    }

    /// EXT on/off: hold this deck against the room's published clock
    /// instead of the other deck.
    pub fn toggle_ext_sync(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let on = !self.deck(deck).ext_sync;
        self.set_ext_sync(deck, on);
        if !on {
            self.deck_mut(deck).auto_opt_out = true;
        }
        Vec::new()
    }

    pub fn set_ext_sync(&mut self, deck: DeckId, on: bool) {
        let state = self.deck_mut(deck);
        state.ext_sync = on;
        if on {
            state.synced = false;
        }
    }

    /// Hold every EXT deck against the published clock. Called once per
    /// pump with the clock's current view of the room.
    ///
    /// The deck follows a clock that is continuous by contract, so what
    /// comes out here is a gently walking rate — never a jerk — and a seek
    /// only when the deck was moved out from under the lock.
    pub fn follow_external(&mut self, external: &SyncView) -> Vec<DeckCmd> {
        let mut cmds = Vec::new();
        for deck in [DeckId::A, DeckId::B] {
            let state = self.deck(deck);
            if !state.ext_sync || !state.playing || state.scratching {
                continue;
            }
            cmds.extend(self.follow_view(deck, external));
        }
        cmds
    }

    /// Hold every synced follower against the pinned master, once per pump.
    ///
    /// This is what makes deck SYNC a LOCK instead of a one-shot: the
    /// event-time landing puts the decks together, and this servo keeps
    /// them there with the same bounded rate trim the EXT path uses — an
    /// analysed grid is never exactly the record, so without a held
    /// correction two "synced" decks walk apart and the next event snaps
    /// them back with an audible jump.
    pub fn hold_deck_sync(&mut self) -> Vec<DeckCmd> {
        self.refresh_sync_master();
        let Some(master) = self.sync_master_valid() else { return Vec::new() };
        // A paused master is a frozen phase — the followers free-run at the
        // matched tempo until it plays (or the pin hands over).
        if !self.deck(master).playing {
            return Vec::new();
        }
        let Some(view) = self.deck(master).sync_view() else { return Vec::new() };
        let mut cmds = Vec::new();
        for deck in [DeckId::A, DeckId::B] {
            if deck == master || self.auto_fade_hold == Some(deck) {
                continue;
            }
            let state = self.deck(deck);
            if !state.synced || state.ext_sync || !state.playing || state.scratching {
                continue;
            }
            cmds.extend(self.follow_view(deck, &view));
        }
        cmds
    }

    /// One deck held against one continuous reference: the bounded rate
    /// trim, and a landing only when the deck was moved out from under the
    /// lock (that landing takes the same lookahead as an event lock — the
    /// reference keeps moving while the seek crosses to the audio thread).
    fn follow_view(&mut self, deck: DeckId, reference: &SyncView) -> Vec<DeckCmd> {
        let state = self.deck(deck);
        let Some(view) = state.sync_view() else { return Vec::new() };
        let envelope = state.pitch_range.fraction();
        let Some(follow) = external_follow(reference, &view, envelope) else {
            return Vec::new();
        };
        let lookahead = self.land_lookahead_secs;
        let mut cmds = Vec::new();
        let state = self.deck_mut(deck);
        if (state.rate - follow.rate).abs() > 1e-4 {
            state.rate = follow.rate;
            state.pitch = (follow.rate - 1.0).clamp(-0.5, 0.5);
            cmds.push(DeckCmd::SetRate { deck, rate: follow.rate });
        }
        if let Some(secs) = follow.reseek_secs {
            let secs = secs + follow.rate * lookahead;
            state.position_secs = secs;
            cmds.push(DeckCmd::SeekSeconds { deck, secs });
        }
        cmds
    }

    /// No commands: unlike auto sync, a new unit changes nothing until
    /// the next seek, so there is nothing to emit.
    pub fn set_snap_beats(&mut self, beats: u32) {
        self.snap_beats = beats;
    }

    pub fn set_auto_sync(&mut self, on: bool) -> Vec<DeckCmd> {
        self.auto_sync = on;
        if !on {
            for index in 0..2 {
                self.decks[index].synced = false;
            }
            // No group without members: the pin goes with them.
            self.sync_master = None;
            return Vec::new();
        }
        // Turning it back on forgives every opt-out.
        for index in 0..2 {
            self.decks[index].auto_opt_out = false;
        }
        self.apply_auto_sync()
    }

    /// Operator pitch slider, as a fraction of the selected range.
    /// Touching it is a deliberate override: this deck leaves auto sync.
    pub fn set_pitch(&mut self, deck: DeckId, fraction: f64) -> Vec<DeckCmd> {
        let range = self.deck(deck).pitch_range.fraction();
        let pitch = (fraction.clamp(-1.0, 1.0)) * range;
        let rate = (1.0 + pitch).clamp(RATE_MIN, RATE_MAX);
        // Moving the MASTER's pitch is how the group is driven — it stays
        // in the group. Moving a FOLLOWER's pitch is a deliberate override:
        // that deck leaves the lock until asked back.
        let is_master = self.sync_master_valid() == Some(deck);
        let state = self.deck_mut(deck);
        state.pitch = pitch;
        state.rate = rate;
        if !is_master {
            state.synced = false;
            state.auto_opt_out = true;
        }
        let mut cmds = vec![DeckCmd::SetRate { deck, rate }];
        // A tempo move on the LEADER propagates: the follower keeps up.
        if self.sync_leader() == Some(deck) {
            cmds.extend(self.apply_auto_sync());
        }
        cmds
    }

    /// Nudge the pitch by a small step (the ± buttons / an encoder).
    pub fn nudge_pitch(&mut self, deck: DeckId, steps: f64) -> Vec<DeckCmd> {
        let state = self.deck(deck);
        let range = state.pitch_range.fraction();
        let fraction = state.pitch / range + steps * 0.01;
        self.set_pitch(deck, fraction)
    }

    pub fn toggle_pitch_range(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        let range = state.pitch_range.toggled();
        state.pitch_range = range;
        // Keep the audible tempo: re-express the same pitch in the new range.
        let fraction = (state.pitch / range.fraction()).clamp(-1.0, 1.0);
        self.set_pitch(deck, fraction)
    }

    /// Drop the pitch back to the track's own tempo.
    pub fn reset_pitch(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        self.set_pitch(deck, 0.0)
    }

    /// Operator key shift, in semitones. Pure pitch: the tempo, the grid and
    /// the phase lock are all untouched, so unlike the tempo slider this is
    /// NOT a sync opt-out — a deck can be locked to the beat and transposed
    /// into the mix at the same time.
    pub fn set_key_shift(&mut self, deck: DeckId, semitones: f64) -> Vec<DeckCmd> {
        let semitones = semitones.clamp(-KEY_SHIFT_MAX, KEY_SHIFT_MAX);
        self.deck_mut(deck).key_shift = semitones;
        vec![DeckCmd::SetKeyShift { deck, semitones }]
    }

    /// Step the key by whole semitones (the ± buttons).
    pub fn nudge_key_shift(&mut self, deck: DeckId, steps: f64) -> Vec<DeckCmd> {
        let semitones = self.deck(deck).key_shift + steps;
        self.set_key_shift(deck, semitones)
    }

    /// Back to the track's own key.
    pub fn reset_key_shift(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        self.set_key_shift(deck, 0.0)
    }

    /// Key lock: hold the key while the tempo moves. It also sets where a key
    /// shift is measured from — the track's own key with the lock on, the
    /// already-varisped key with it off.
    pub fn toggle_keylock(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        state.keylock = !state.keylock;
        vec![DeckCmd::SetKeylock { deck, on: state.keylock }]
    }

    /// Pointer on the waveform. A grab suspends the phase lock; the release
    /// re-locks against the leader if auto sync is on.
    pub fn scratch(&mut self, deck: DeckId, motion: ScratchMotion) -> Vec<DeckCmd> {
        if !self.deck(deck).is_loaded() {
            return Vec::new();
        }
        let mut cmds = vec![DeckCmd::Scratch { deck, motion }];
        match motion {
            ScratchMotion::Grab => self.deck_mut(deck).scratching = true,
            ScratchMotion::Move { .. } => {}
            ScratchMotion::Release => {
                self.deck_mut(deck).scratching = false;
                // Letting go re-locks against the leader, from wherever the
                // hand left the record.
                if self.deck(deck).synced || self.auto_sync {
                    cmds.extend(self.apply_auto_sync_with(Some(SyncQuantize::Beat)));
                }
            }
        }
        cmds
    }

    /// Absolute seek in source seconds (an overview click, a cue recall).
    ///
    /// A seek is a jump anywhere in the track, so under auto sync it is
    /// immediately followed by a phase re-lock: the tempo match is
    /// untouched, and the playhead settles on the nearest grid-consistent
    /// offset — at most half a beat from where the operator put it — so the
    /// two decks come back into step without a jump-cut.
    pub fn seek_secs(&mut self, deck: DeckId, secs: f64) -> Vec<DeckCmd> {
        if !self.deck(deck).is_loaded() {
            return Vec::new();
        }
        let duration = self.deck(deck).duration_secs;
        let secs = if duration > 0.0 { secs.clamp(0.0, duration) } else { secs.max(0.0) };
        self.deck_mut(deck).position_secs = secs;
        let mut cmds = vec![DeckCmd::SeekSeconds { deck, secs }];
        cmds.extend(self.apply_auto_sync_with(Some(SyncQuantize::Beat)));
        cmds
    }

    /// Beat jump: move the playhead by whole beats of the deck's own grid.
    /// A whole-beat move keeps the deck's phase, so the beat-quantized
    /// re-lock that follows every seek lands it exactly where it was put.
    pub fn beat_jump(&mut self, deck: DeckId, beats: f64) -> Vec<DeckCmd> {
        let state = self.deck(deck);
        if !state.is_loaded() || !beats.is_finite() {
            return Vec::new();
        }
        let beat_secs = state
            .grid
            .filter(|grid| grid.has_grid())
            .map(|grid| grid.beat_secs)
            .unwrap_or(0.5);
        let secs = state.position_secs + beats * beat_secs;
        self.seek_secs(deck, secs)
    }

    /// Flip the deck's grid half a beat. The analyser's known failure mode
    /// is a perfectly steady grid on the OFF pulse: same tempo, every ruling
    /// on a real transient, and sync then holds the two tracks exactly half
    /// a beat apart. Moving every ruling by half a beat puts the grid on the
    /// other pulse; the caller re-publishes the flipped grid wherever else
    /// it lives (analysis, loop grid, cache). Returns the flipped grid.
    pub fn flip_beat_phase(&mut self, deck: DeckId) -> Option<(TrackGrid, Vec<DeckCmd>)> {
        let state = self.deck_mut(deck);
        let grid = state.grid.as_mut()?;
        if !grid.has_grid() {
            return None;
        }
        // The rulings land in the same places either way; which way the
        // DOWNBEAT moves is the choice. Forward the first time, back the
        // second, so two presses are exactly no presses.
        let half = grid.beat_secs * 0.5;
        if state.phase_flipped {
            grid.first_beat_secs -= half;
            if grid.first_beat_secs < 0.0 {
                // The first ruling at or after zero is now the old first
                // beat's successor, one beat later in the bar.
                grid.first_beat_secs += grid.beat_secs;
                grid.downbeat_phase = (grid.downbeat_phase + 1) % 4;
            }
        } else {
            grid.first_beat_secs += half;
            if grid.first_beat_secs >= grid.beat_secs {
                // The first ruling at or after zero is now the one BEFORE
                // the old first beat, one beat earlier in the bar.
                grid.first_beat_secs -= grid.beat_secs;
                grid.downbeat_phase = (grid.downbeat_phase + 3) % 4;
            }
        }
        state.phase_flipped = !state.phase_flipped;
        let flipped = *grid;
        let cmds = if self.deck(deck).synced || self.auto_sync {
            self.apply_auto_sync_with(Some(SyncQuantize::Beat))
        } else {
            Vec::new()
        };
        Some((flipped, cmds))
    }

    /// The phase a snapped landing must preserve: the one that SURVIVES.
    /// Every seek is followed by a beat-quantized auto-sync re-lock, so on
    /// a follower deck the deck's own playhead phase is about to be
    /// discarded — anchoring on it would move the deck twice, and the
    /// two-stage move can land a full beat from the best position. Anchor
    /// instead on the deck's position corrected to the leader's phase (the
    /// same `sync_plan` arithmetic the re-lock uses), so the re-lock's
    /// dead-band finds nothing to do. The leader, an unsynced deck, or a
    /// deck with nothing to follow keeps its own playhead. The guards
    /// mirror `apply_auto_sync_with` exactly: a deck the re-lock would
    /// skip must not be anchored to a phase that will not be imposed.
    fn snap_reference(&self, deck: DeckId) -> f64 {
        let own = self.deck(deck).position_secs;
        if !self.auto_sync {
            return own;
        }
        let Some(leader) = self.sync_leader() else { return own };
        if leader == deck {
            return own;
        }
        let state = self.deck(deck);
        if !state.is_loaded() || state.auto_opt_out || state.scratching {
            return own;
        }
        let (Some(lead), Some(follow)) =
            (self.deck(leader).sync_view(), state.sync_view())
        else {
            return own;
        };
        sync_plan(&lead, &follow, SyncQuantize::Beat)
            .and_then(|plan| plan.seek_secs)
            .unwrap_or(own)
    }

    /// An operator-chosen seek, run through QUANT first. Only the overview
    /// strip uses this. Everything else — CUE, a lyric line, RELOOP,
    /// engaging a loop, every sync correction — seeks a target that must
    /// not be displaced.
    pub fn seek_secs_snapped(&mut self, deck: DeckId, secs: f64) -> Vec<DeckCmd> {
        let unit = self.snap_beats;
        let snapped = match self.deck(deck).grid {
            Some(grid) => grid.snap_translate(secs, self.snap_reference(deck), unit),
            None => secs,
        };
        self.seek_secs(deck, snapped)
    }

    // ---- tone + stems -------------------------------------------------------

    pub fn set_eq(&mut self, deck: DeckId, band: usize, gain: f32) -> Vec<DeckCmd> {
        if band >= 3 {
            return Vec::new();
        }
        let state = self.deck_mut(deck);
        state.eq[band] = gain.clamp(0.0, crate::music_dsp::EQ_MAX_GAIN);
        vec![DeckCmd::SetEqBand { deck, band, gain: state.eq_effective(band) }]
    }

    /// Kill button: a held band is silent whatever its knob says, and
    /// releasing it restores the knob exactly.
    pub fn toggle_eq_kill(&mut self, deck: DeckId, band: usize) -> Vec<DeckCmd> {
        if band >= 3 {
            return Vec::new();
        }
        let state = self.deck_mut(deck);
        state.eq_kill[band] = !state.eq_kill[band];
        vec![DeckCmd::SetEqBand { deck, band, gain: state.eq_effective(band) }]
    }

    /// A mute touches one band; a solo redraws all three — the bands
    /// OUTSIDE the solo set are the ones that change.
    pub fn toggle_eq_solo(&mut self, deck: DeckId, band: usize) -> Vec<DeckCmd> {
        if band >= 3 {
            return Vec::new();
        }
        let state = self.deck_mut(deck);
        state.eq_solo[band] = !state.eq_solo[band];
        (0..3)
            .map(|band| DeckCmd::SetEqBand { deck, band, gain: state.eq_effective(band) })
            .collect()
    }

    pub fn set_filter(&mut self, deck: DeckId, position: f32) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        state.filter = position.clamp(0.0, 1.0);
        vec![DeckCmd::SetFilter { deck, position: state.filter }]
    }

    /// Stem knob. Inert until the separated stems are loaded — the deck is
    /// playing the full mix and there is nothing to turn down.
    pub fn set_stem(&mut self, deck: DeckId, stem: usize, gain: f32) -> Vec<DeckCmd> {
        if stem >= STEM_COUNT {
            return Vec::new();
        }
        let state = self.deck_mut(deck);
        state.stem_gain[stem] = gain.clamp(0.0, crate::music_dsp::EQ_MAX_GAIN);
        if !state.stems_ready {
            return Vec::new();
        }
        vec![DeckCmd::SetStemGain { deck, stem, gain: state.stem_effective(stem) }]
    }

    pub fn toggle_stem_kill(&mut self, deck: DeckId, stem: usize) -> Vec<DeckCmd> {
        if stem >= STEM_COUNT {
            return Vec::new();
        }
        let state = self.deck_mut(deck);
        state.stem_kill[stem] = !state.stem_kill[stem];
        if !state.stems_ready {
            return Vec::new();
        }
        vec![DeckCmd::SetStemGain { deck, stem, gain: state.stem_effective(stem) }]
    }

    /// A mute touches one lane; a solo redraws the whole bus — lanes
    /// OUTSIDE the solo set are the ones that change — so every lane is
    /// re-published.
    pub fn toggle_stem_solo(&mut self, deck: DeckId, stem: usize) -> Vec<DeckCmd> {
        if stem >= STEM_COUNT {
            return Vec::new();
        }
        let state = self.deck_mut(deck);
        state.stem_solo[stem] = !state.stem_solo[stem];
        if !state.stems_ready {
            return Vec::new();
        }
        (0..STEM_COUNT)
            .map(|stem| DeckCmd::SetStemGain { deck, stem, gain: state.stem_effective(stem) })
            .collect()
    }

    // ---- queue --------------------------------------------------------------

    pub fn queue(&self) -> &[TrackItem] {
        &self.queue
    }

    /// Put a track at the back of the queue (no duplicates).
    pub fn enqueue(&mut self, item: TrackItem) -> Vec<DeckCmd> {
        if self.queue.iter().any(|queued| queued.asset == item.asset) {
            return Vec::new();
        }
        self.queue.push(item);
        if self.auto_load_queue {
            return self.pump_queue();
        }
        Vec::new()
    }

    /// A finished track back onto the tail. Never pumps (the hand-back runs
    /// its single deliberate pump afterwards) and keeps the dedupe: a track
    /// the operator already re-queued is not doubled.
    pub fn requeue(&mut self, item: TrackItem) {
        // The spare names the just-finished asset whether or not the push
        // happens: a track the operator already re-queued mid-play must
        // still be spared from the very next shuffle draw.
        self.last_requeued = Some(item.asset);
        if self.queue.iter().any(|queued| queued.asset == item.asset) {
            return;
        }
        self.queue.push(item);
    }

    pub fn seed_shuffle(&mut self, seed: u64) {
        self.shuffle_rng = seed.max(1);
    }

    /// Which queue index the next pump takes: the head, or a shuffle draw
    /// that spares the track the last hand-back pushed (unless it is all
    /// there is).
    fn pick_index(&mut self) -> usize {
        if !self.shuffle || self.queue.len() < 2 {
            return 0;
        }
        let spare = self.last_requeued;
        let candidates: Vec<usize> = (0..self.queue.len())
            .filter(|&index| spare != Some(self.queue[index].asset))
            .collect();
        let pool = if candidates.is_empty() {
            (0..self.queue.len()).collect()
        } else {
            candidates
        };
        self.shuffle_rng = xorshift64star(self.shuffle_rng);
        pool[(self.shuffle_rng % pool.len() as u64) as usize]
    }

    pub fn dequeue(&mut self, index: usize) {
        if index < self.queue.len() {
            self.queue.remove(index);
        }
    }

    /// Move a queued track to another spot in the play order.
    ///
    /// The play order is the operator's set list, so it has to be
    /// rearrangeable without emptying and refilling it. Returns whether
    /// anything actually moved, so a drag can skip a redraw that would
    /// show the same rows.
    pub fn move_queued(&mut self, from: usize, to: usize) -> bool {
        if from >= self.queue.len() || to >= self.queue.len() || from == to {
            return false;
        }
        let item = self.queue.remove(from);
        self.queue.insert(to, item);
        true
    }

    pub fn clear_queue(&mut self) {
        self.queue.clear();
    }

    /// A deck that a queued track may take over: empty, or loaded but idle
    /// and not the audible one.
    fn free_deck(&self) -> Option<DeckId> {
        let candidate = self.auto_target();
        let state = self.deck(candidate);
        let busy = state.playing
            || matches!(state.load, DeckLoad::Loading { .. })
            || self.sync_leader() == Some(candidate);
        (!busy && matches!(state.load, DeckLoad::Empty | DeckLoad::Failed { .. }))
            .then_some(candidate)
    }

    /// Load one queued track onto a free deck, if there is one of each.
    /// The head in order, or a shuffle draw.
    pub fn pump_queue(&mut self) -> Vec<DeckCmd> {
        if self.queue.is_empty() {
            return Vec::new();
        }
        let Some(deck) = self.free_deck() else {
            return Vec::new();
        };
        let index = self.pick_index();
        self.last_requeued = None;
        let item = self.queue.remove(index);
        let target = match deck {
            DeckId::A => DeckTarget::A,
            DeckId::B => DeckTarget::B,
        };
        self.click(item, target)
    }

    /// Load a queued track straight onto a deck now (a click in the queue).
    pub fn load_queued(&mut self, index: usize, target: DeckTarget) -> Vec<DeckCmd> {
        // The guard comes BEFORE the remove: under OFF the click loads
        // nothing, and a row that loaded nothing must still be in the queue.
        if index >= self.queue.len() || target == DeckTarget::Off {
            return Vec::new();
        }
        let item = self.queue.remove(index);
        self.click(item, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(seed: u8) -> TrackItem {
        TrackItem {
            asset: AssetId::from_bytes([seed; 16]),
            revision: AssetRevisionId::from_bytes([seed; 32]),
            title: format!("track {seed}"),
            media_blob: BlobId::from_bytes([seed ^ 0xff; 32]),
            media_len: 4000 + seed as u64,
            media: MediaType::Wav,
            side: TrackSideChannels::default(),
        }
    }

    fn load_gen(cmds: &[DeckCmd]) -> (DeckId, DeckGen) {
        cmds.iter()
            .find_map(|c| match c {
                DeckCmd::LoadTrack { deck, gen, .. } => Some((*deck, *gen)),
                _ => None,
            })
            .expect("load command")
    }

    #[test]
    fn equal_power_endpoints_and_midpoint() {
        let (a, b) = crossfader_gains(0.0, FadeCurve::EqualPower);
        assert!((a - 1.0).abs() < 1e-6 && b.abs() < 1e-6);
        let (a, b) = crossfader_gains(1.0, FadeCurve::EqualPower);
        assert!(a.abs() < 1e-6 && (b - 1.0).abs() < 1e-6);
        let (a, b) = crossfader_gains(0.5, FadeCurve::EqualPower);
        let root_half = std::f32::consts::FRAC_1_SQRT_2;
        assert!((a - root_half).abs() < 1e-6, "midpoint A {a}");
        assert!((b - root_half).abs() < 1e-6, "midpoint B {b}");
        // Constant power across the whole travel.
        for i in 0..=20 {
            let (a, b) = crossfader_gains(i as f32 / 20.0, FadeCurve::EqualPower);
            assert!((a * a + b * b - 1.0).abs() < 1e-5);
        }
        // Out-of-range positions clamp.
        assert_eq!(crossfader_gains(-1.0, FadeCurve::Linear), (1.0, 0.0));
        assert_eq!(crossfader_gains(2.0, FadeCurve::Linear), (0.0, 1.0));
    }

    /// The three crossing curves have to be three DIFFERENT curves: dipped
    /// under linear, linear under equal power. Two rungs a hand cannot tell
    /// apart are one rung with a spare name — which is what happened to the
    /// intermediate rung, and why it is gone.
    #[test]
    fn the_crossing_curves_are_ordered_and_distinct() {
        let mid = |curve| crossfader_gains(0.5, curve).0;
        let dipped = mid(FadeCurve::Dipped);
        let linear = mid(FadeCurve::Linear);
        let equal = mid(FadeCurve::EqualPower);
        assert!(dipped < linear, "dipped {dipped} should sag under linear {linear}");
        assert!(linear < equal, "linear {linear} should sit under equal power {equal}");
        // And far enough apart to see: a plot row is ~16 points tall, so
        // anything under a twentieth of full scale is one pixel of nothing.
        for (name, a, b) in [("dipped/linear", dipped, linear), ("linear/equal", linear, equal)] {
            assert!((b - a) > 0.05, "{name} differ by only {}", b - a);
        }
    }

    #[test]
    fn auto_routing_prefers_empty_then_non_playing_then_quiet_side() {
        let mut e = DeckEngine::new();
        // Both empty: A first, then (after A loads) B.
        assert_eq!(e.auto_target(), DeckId::A);
        let (d1, g1) = load_gen(&e.click(item(1), DeckTarget::Auto));
        assert_eq!(d1, DeckId::A);
        assert_eq!(e.auto_target(), DeckId::B);
        e.track_ready(d1, g1, 60.0);
        let (d2, g2) = load_gen(&e.click(item(2), DeckTarget::Auto));
        assert_eq!(d2, DeckId::B);
        e.track_ready(d2, g2, 60.0);

        // A playing, B idle → Auto routes to B (never interrupts the live deck).
        e.play_pause(DeckId::A);
        assert_eq!(e.auto_target(), DeckId::B);
        // Both playing → the side the crossfader is turned away from.
        e.play_pause(DeckId::B);
        e.set_crossfader(1.0); // full B → A is silent → replace A
        assert_eq!(e.auto_target(), DeckId::A);
        e.set_crossfader(0.0);
        assert_eq!(e.auto_target(), DeckId::B);
        // Dead-center tie → the deck loaded less recently.
        e.set_crossfader(0.5);
        assert_eq!(e.auto_target(), DeckId::A); // B was last loaded
    }

    #[test]
    fn loading_a_deck_never_touches_the_other() {
        let mut e = DeckEngine::new();
        let (da, ga) = load_gen(&e.click(item(1), DeckTarget::A));
        e.track_ready(da, ga, 30.0);
        e.play_pause(DeckId::A);
        // Load B while A is live.
        let cmds = e.click(item(2), DeckTarget::B);
        assert!(cmds.iter().all(|c| !matches!(
            c,
            DeckCmd::SetPlaying { deck: DeckId::A, .. } | DeckCmd::InstallTrack { deck: DeckId::A }
        )));
        assert!(e.deck(DeckId::A).playing);
        let (db, gb) = load_gen(&cmds);
        assert_eq!(db, DeckId::B);
        let cmds = e.track_ready(db, gb, 45.0);
        assert!(cmds.contains(&DeckCmd::InstallTrack { deck: DeckId::B }));
        assert!(e.deck(DeckId::A).playing, "live deck must keep playing");
        assert!(!e.deck(DeckId::B).playing, "fresh load installs paused");
    }

    #[test]
    fn rapid_clicks_same_deck_latest_wins() {
        let mut e = DeckEngine::new();
        let (_, g1) = load_gen(&e.click(item(1), DeckTarget::A));
        let (_, g2) = load_gen(&e.click(item(2), DeckTarget::A));
        assert!(g2 > g1);
        // The stale decode lands: ignored.
        assert!(e.track_ready(DeckId::A, g1, 30.0).is_empty());
        assert!(matches!(e.deck(DeckId::A).load, DeckLoad::Loading { .. }));
        // The winner installs.
        let cmds = e.track_ready(DeckId::A, g2, 40.0);
        assert!(cmds.contains(&DeckCmd::InstallTrack { deck: DeckId::A }));
        match &e.deck(DeckId::A).load {
            DeckLoad::Loaded { item } => assert_eq!(item.title, "track 2"),
            other => panic!("unexpected {other:?}"),
        }
        // A stale failure is equally silent.
        assert!(e.track_failed(DeckId::A, g1, "late".into()).is_empty());
        assert!(matches!(e.deck(DeckId::A).load, DeckLoad::Loaded { .. }));
    }

    #[test]
    fn transport_requires_a_loaded_track() {
        let mut e = DeckEngine::new();
        assert!(e.play_pause(DeckId::A).is_empty());
        assert!(e.seek(DeckId::A, 0.3).is_empty());
        let (d, g) = load_gen(&e.click(item(1), DeckTarget::A));
        assert!(e.play_pause(DeckId::A).is_empty(), "still loading");
        e.track_ready(d, g, 30.0);
        assert_eq!(
            e.play_pause(DeckId::A),
            vec![DeckCmd::SetPlaying { deck: DeckId::A, playing: true }]
        );
        assert_eq!(
            e.seek(DeckId::A, 2.0),
            vec![DeckCmd::SeekFraction { deck: DeckId::A, fraction: 1.0 }]
        );
        // End-of-track with loop off stops the transport mirror.
        e.track_ended(DeckId::A);
        assert!(!e.deck(DeckId::A).playing);
    }

    #[test]
    fn ready_install_carries_mute_and_gain_but_drops_the_loop() {
        let mut e = DeckEngine::new();
        e.deck_mut(DeckId::B).loop_span = Some(LoopSpan { start_secs: 1.0, end_secs: 2.0 });
        e.toggle_mute(DeckId::B);
        e.set_gain(DeckId::B, 0.5);
        let (d, g) = load_gen(&e.click(item(3), DeckTarget::B));
        let cmds = e.track_ready(d, g, 20.0);
        assert!(cmds.contains(&DeckCmd::SetLoopSpan { deck: DeckId::B, span: None }));
        assert!(cmds.contains(&DeckCmd::SetMute { deck: DeckId::B, muted: true }));
        assert!(cmds.contains(&DeckCmd::SetGain { deck: DeckId::B, gain: 0.5 }));
    }

    // -----------------------------------------------------------------
    // the loop: RELOOP/EXIT, clear, and what survives a load or a swap
    // -----------------------------------------------------------------

    #[test]
    fn reloop_exits_keeping_the_span_and_re_enters_it() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_span = Some(LoopSpan { start_secs: 8.0, end_secs: 10.0 });
        // Exit: the span goes, the memory keeps it, and the playhead stays.
        let cmds = e.toggle_loop(DeckId::A);
        assert!(!e.deck(DeckId::A).loop_on(), "exit must clear the running span");
        assert_eq!(
            e.deck(DeckId::A).loop_memory,
            Some(LoopSpan { start_secs: 8.0, end_secs: 10.0 })
        );
        assert!(seek_of(&cmds, DeckId::A).is_none(), "exiting must not move the playhead");
        // Re-enter: back to the remembered span, and the playhead lands on IN.
        let cmds = e.toggle_loop(DeckId::A);
        assert!(e.deck(DeckId::A).loop_on());
        assert_eq!(seek_of(&cmds, DeckId::A), Some(8.0), "reloop jumps to IN");
    }

    #[test]
    fn clear_forgets_the_span_the_memory_and_the_arm() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        let state = e.deck_mut(DeckId::A);
        state.loop_span = Some(LoopSpan { start_secs: 8.0, end_secs: 10.0 });
        state.loop_memory = Some(LoopSpan { start_secs: 8.0, end_secs: 10.0 });
        state.loop_armed = Some(4.0);
        e.loop_clear(DeckId::A);
        let state = e.deck(DeckId::A);
        assert!(state.loop_span.is_none() && state.loop_memory.is_none());
        assert!(state.loop_armed.is_none());
        // Nothing left to re-enter, so LOOP is inert.
        assert!(e.toggle_loop(DeckId::A).is_empty(), "LOOP with nothing remembered does nothing");
    }

    #[test]
    fn a_fresh_install_drops_the_span_but_keeps_the_armed_length() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::B, 2, 120.0, 0.0);
        let state = e.deck_mut(DeckId::B);
        state.loop_span = Some(LoopSpan { start_secs: 8.0, end_secs: 10.0 });
        state.loop_memory = Some(LoopSpan { start_secs: 8.0, end_secs: 10.0 });
        state.loop_armed = Some(4.0);
        state.loop_beats = 16;
        let (d, g) = load_gen(&e.click(item(3), DeckTarget::B));
        let cmds = e.track_ready(d, g, 20.0);
        let state = e.deck(DeckId::B);
        assert!(state.loop_span.is_none(), "a span belongs to the track it was measured on");
        assert!(state.loop_memory.is_none() && state.loop_armed.is_none());
        assert_eq!(state.loop_beats, 16, "the armed length is an operator preference");
        assert!(cmds.contains(&DeckCmd::SetLoopSpan { deck: DeckId::B, span: None }));
    }

    /// A deck with a track and no analysis: MAN's home ground.
    fn load_unanalysed(engine: &mut DeckEngine, deck: DeckId, seed: u8) {
        let target = match deck {
            DeckId::A => DeckTarget::A,
            DeckId::B => DeckTarget::B,
        };
        let (deck, gen) = load_gen(&engine.click(item(seed), target));
        engine.track_ready(deck, gen, 300.0);
    }

    #[test]
    fn bracket_in_builds_n_beats_forward_from_the_playhead() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // 0.5 s a beat, 300 s long
        e.deck_mut(DeckId::A).loop_beats = 4;
        e.observe(DeckId::A, 10.25, true);
        let cmds = e.loop_in(DeckId::A);
        let span = e.deck(DeckId::A).loop_span.expect("a span");
        assert!((span.start_secs - 10.25).abs() < 1e-9, "IN sits exactly at the playhead");
        assert!((span.len_secs() - 2.0).abs() < 1e-9, "4 beats at 120 BPM = 2 s");
        assert!(cmds.contains(&DeckCmd::SetLoopSpan { deck: DeckId::A, span: Some(span) }));
        assert!(seek_of(&cmds, DeckId::A).is_none(), "the playhead is already at IN");
    }

    #[test]
    fn bracket_out_builds_n_beats_back_from_the_playhead() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_beats = 8;
        e.observe(DeckId::A, 10.0, true);
        let cmds = e.loop_out(DeckId::A);
        let span = e.deck(DeckId::A).loop_span.expect("a span");
        assert!((span.end_secs - 10.0).abs() < 1e-9, "OUT sits at the playhead");
        assert!((span.start_secs - 6.0).abs() < 1e-9, "8 beats at 120 BPM = 4 s back");
        // The playhead is AT the out point, so it has to be sent back at
        // once — that jump is the whole point of `]`.
        assert_eq!(seek_of(&cmds, DeckId::A), Some(6.0));
    }

    #[test]
    fn beat_brackets_are_refused_without_a_grid() {
        let mut e = DeckEngine::new();
        load_unanalysed(&mut e, DeckId::A, 1);
        e.deck_mut(DeckId::A).loop_beats = 4;
        e.observe(DeckId::A, 10.0, true);
        assert!(e.loop_in(DeckId::A).is_empty(), "N > 0 has no beat to measure");
        assert!(e.loop_out(DeckId::A).is_empty());
        assert!(e.deck(DeckId::A).loop_span.is_none());
        assert!(e.deck(DeckId::A).loop_armed.is_none(), "a beat count must not arm MAN");
    }

    #[test]
    fn man_arms_on_in_and_closes_on_out_with_no_grid_at_all() {
        let mut e = DeckEngine::new();
        load_unanalysed(&mut e, DeckId::A, 1);
        e.deck_mut(DeckId::A).loop_beats = 0; // MAN
        e.observe(DeckId::A, 10.0, true);
        assert!(e.loop_in(DeckId::A).is_empty(), "arming makes no sound and no command");
        assert_eq!(e.deck(DeckId::A).loop_armed, Some(10.0));
        e.observe(DeckId::A, 13.5, true);
        let cmds = e.loop_out(DeckId::A);
        let span = e.deck(DeckId::A).loop_span.expect("a span");
        assert!((span.start_secs - 10.0).abs() < 1e-9 && (span.end_secs - 13.5).abs() < 1e-9);
        assert!(e.deck(DeckId::A).loop_armed.is_none(), "closing consumes the arm");
        assert_eq!(seek_of(&cmds, DeckId::A), Some(10.0), "closing at OUT wraps to IN");
    }

    #[test]
    fn man_out_before_in_re_arms_instead_of_closing_backwards() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_beats = 0;
        e.observe(DeckId::A, 20.0, true);
        e.loop_in(DeckId::A);
        e.observe(DeckId::A, 5.0, true); // scrubbed back behind the arm
        e.loop_out(DeckId::A);
        assert!(e.deck(DeckId::A).loop_span.is_none(), "no backwards span");
        assert_eq!(e.deck(DeckId::A).loop_armed, Some(5.0), "the arm moves to here");
    }

    #[test]
    fn man_out_with_nothing_armed_is_ignored() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_beats = 0;
        e.observe(DeckId::A, 10.0, true);
        assert!(e.loop_out(DeckId::A).is_empty());
        assert!(e.deck(DeckId::A).loop_span.is_none());
    }

    #[test]
    fn a_span_that_runs_off_the_end_is_refused() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // 300 s
        e.deck_mut(DeckId::A).loop_beats = 64; // 32 s
        e.observe(DeckId::A, 299.0, true);
        assert!(e.loop_in(DeckId::A).is_empty(), "OUT would land past the track end");
        assert!(e.deck(DeckId::A).loop_span.is_none());
        // Symmetrically, `]` near the start would put IN before zero.
        e.observe(DeckId::A, 1.0, true);
        assert!(e.loop_out(DeckId::A).is_empty());
        assert!(e.deck(DeckId::A).loop_span.is_none());
    }

    #[test]
    fn a_span_shorter_than_the_floor_is_refused() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_beats = 0;
        e.observe(DeckId::A, 10.0, true);
        e.loop_in(DeckId::A);
        // A hair after IN: all seam, no music.
        e.observe(DeckId::A, 10.0 + LOOP_MIN_SECS * 0.5, true);
        e.loop_out(DeckId::A);
        assert!(e.deck(DeckId::A).loop_span.is_none(), "under the floor, nothing engages");
    }

    #[test]
    fn the_count_halves_to_man_and_doubles_back_out_of_it() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_beats = 2;
        e.loop_halve(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_beats, 1);
        e.loop_halve(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_beats, 0, "1 halves into MAN");
        e.loop_halve(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_beats, 0, "MAN is the floor");
        e.loop_double(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_beats, 1, "0 doubles to 1, not to 0");
        for _ in 0..6 {
            e.loop_double(DeckId::A);
        }
        assert_eq!(e.deck(DeckId::A).loop_beats, 64, "the last measured rung");
        e.loop_double(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_beats, LOOP_BEATS_INF, "then the bookmark rung");
    }

    #[test]
    fn halving_a_running_loop_anchors_on_in_and_emits_no_seek() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_beats = 4;
        e.observe(DeckId::A, 10.0, true);
        e.loop_in(DeckId::A); // 10.0 .. 12.0
        e.observe(DeckId::A, 11.5, true); // three quarters through
        let cmds = e.loop_halve(DeckId::A);
        let span = e.deck(DeckId::A).loop_span.expect("a span");
        assert!((span.start_secs - 10.0).abs() < 1e-9, "IN is the anchor");
        assert!((span.len_secs() - 1.0).abs() < 1e-9, "the span halves with the count");
        assert_eq!(e.deck(DeckId::A).loop_beats, 2);
        // No seek: the engine's mirror of the playhead is a stale 20 Hz
        // number, so the MIXER catches a stranded playhead — modulo the new
        // length, keeping the subdivision's phase instead of re-triggering
        // IN. Seeking from here was the historical stutter.
        assert!(seek_of(&cmds, DeckId::A).is_none(), "a resize must not seek");
        assert!(cmds.contains(&DeckCmd::SetLoopSpan { deck: DeckId::A, span: Some(span) }));
    }

    #[test]
    fn the_cutter_works_on_a_manual_span_with_no_grid() {
        let mut e = DeckEngine::new();
        load_unanalysed(&mut e, DeckId::A, 1);
        e.deck_mut(DeckId::A).loop_beats = 0;
        e.observe(DeckId::A, 10.0, true);
        e.loop_in(DeckId::A);
        e.observe(DeckId::A, 14.0, true);
        e.loop_out(DeckId::A); // 10.0 .. 14.0, an off-grid span
        e.loop_halve(DeckId::A);
        let span = e.deck(DeckId::A).loop_span.expect("a span");
        assert!((span.len_secs() - 2.0).abs() < 1e-9, "duration halves, no grid needed");
        assert!((span.start_secs - 10.0).abs() < 1e-9);
    }

    #[test]
    fn a_resize_that_will_not_fit_is_refused_and_the_count_holds() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // 300 s
        e.deck_mut(DeckId::A).loop_beats = 4;
        e.observe(DeckId::A, 297.0, true);
        e.loop_in(DeckId::A); // 297.0 .. 299.0
        let cmds = e.loop_double(DeckId::A); // would end at 301, past the track
        assert!(cmds.is_empty());
        assert_eq!(e.deck(DeckId::A).loop_beats, 4, "a refused resize does not move N");
        let span = e.deck(DeckId::A).loop_span.expect("a span");
        assert!((span.end_secs - 299.0).abs() < 1e-9, "the running span is untouched");
    }

    // -----------------------------------------------------------------
    // QUANT: operator seeks land the same distance into the unit
    // -----------------------------------------------------------------

    #[test]
    fn a_snapped_seek_keeps_the_playheads_offset_into_the_unit() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // 0.5 s a beat, 300 s
        e.set_snap_beats(4);
        // Playhead 0.2 s into a beat.
        e.observe(DeckId::A, 10.2, true);
        let cmds = e.seek_secs_snapped(DeckId::A, 63.37);
        let landed = seek_of(&cmds, DeckId::A).expect("a seek");
        let grid = e.deck(DeckId::A).grid.expect("a grid");
        let steps = (grid.beat_at(landed) - grid.beat_at(10.2)) / 4.0;
        assert!((steps - steps.round()).abs() < 1e-9, "moved {steps} bars");
        assert!((landed - 63.37).abs() <= 2.0, "landed {landed}, aimed 63.37");
    }

    #[test]
    fn snap_off_seeks_exactly_where_asked() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        assert_eq!(e.snap_beats, 0, "off is the default");
        e.observe(DeckId::A, 10.2, true);
        let cmds = e.seek_secs_snapped(DeckId::A, 63.37);
        assert_eq!(seek_of(&cmds, DeckId::A), Some(63.37));
    }

    #[test]
    fn a_snapped_seek_without_a_grid_is_exact() {
        let mut e = DeckEngine::new();
        load_unanalysed(&mut e, DeckId::A, 1);
        e.set_snap_beats(4);
        e.observe(DeckId::A, 10.2, true);
        let cmds = e.seek_secs_snapped(DeckId::A, 63.37);
        assert_eq!(seek_of(&cmds, DeckId::A), Some(63.37));
    }

    #[test]
    fn a_snapped_seek_still_clamps_and_mirrors_like_a_plain_one() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // 300 s long
        e.set_snap_beats(4);
        e.observe(DeckId::A, 10.2, true);
        e.seek_secs_snapped(DeckId::A, 10_000.0);
        let position = e.deck(DeckId::A).position_secs;
        assert!(position <= 300.0, "seek_secs must still clamp: {position}");
    }

    #[test]
    fn a_snapped_seek_on_the_follower_lands_in_the_leaders_phase_once() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // the leader
        load_analysed(&mut e, DeckId::B, 2, 120.0, 0.2); // follower, grid shifted
        assert!(e.auto_sync, "auto sync is the default this exists for");
        e.observe(DeckId::A, 32.0, true); // playing, so A leads
        e.observe(DeckId::B, 10.2, false); // parked follower, out of A's phase
        e.set_snap_beats(4);
        let cmds = e.seek_secs_snapped(DeckId::B, 63.0);
        let seeks: Vec<f64> = cmds
            .iter()
            .filter_map(|cmd| match cmd {
                DeckCmd::SeekSeconds { deck: DeckId::B, secs } => Some(*secs),
                _ => None,
            })
            .collect();
        // One gesture, one seek: the translation is anchored on the phase
        // the re-lock would impose, so the re-lock has nothing to do.
        assert_eq!(seeks.len(), 1, "snap then re-lock is a double move: {seeks:?}");
        // And the proof: forcing another re-lock right now finds the deck
        // already in the leader's phase.
        let relock = e.apply_auto_sync_with(Some(SyncQuantize::Beat));
        assert!(
            seek_of(&relock, DeckId::B).is_none(),
            "the landing must already be in the leader's phase"
        );
    }

    #[test]
    fn a_dragged_loop_moves_by_whole_units_against_its_own_phase() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.set_snap_beats(4);
        e.deck_mut(DeckId::A).loop_span =
            Some(LoopSpan { start_secs: 10.2, end_secs: 12.2 });
        e.observe(DeckId::A, 50.0, true); // outside the span
        e.move_loop(DeckId::A, 40.9);
        let span = e.deck(DeckId::A).loop_span.expect("a span");
        let grid = e.deck(DeckId::A).grid.expect("a grid");
        let steps = (grid.beat_at(span.start_secs) - grid.beat_at(10.2)) / 4.0;
        assert!((steps - steps.round()).abs() < 1e-9, "moved {steps} bars");
        assert!((span.len_secs() - 2.0).abs() < 1e-9, "a move must not resize");
    }

    #[test]
    fn a_dragged_loop_updates_the_memory_reloop_returns_to() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_span =
            Some(LoopSpan { start_secs: 10.0, end_secs: 12.0 });
        e.observe(DeckId::A, 50.0, true);
        e.move_loop(DeckId::A, 40.0);
        let span = e.deck(DeckId::A).loop_span.expect("a span");
        assert_eq!(e.deck(DeckId::A).loop_memory, Some(span), "RELOOP must return here");
    }

    #[test]
    fn the_playhead_rides_a_dragged_loop_at_the_same_offset() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_span =
            Some(LoopSpan { start_secs: 10.0, end_secs: 14.0 });
        e.observe(DeckId::A, 11.0, true); // a quarter of the way in
        let cmds = e.move_loop(DeckId::A, 50.0);
        assert_eq!(seek_of(&cmds, DeckId::A), Some(51.0), "same quarter, new span");
        assert!((e.deck(DeckId::A).position_secs - 51.0).abs() < 1e-9, "mirror the ride");
    }

    #[test]
    fn a_playhead_outside_a_dragged_loop_stays_put() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_span =
            Some(LoopSpan { start_secs: 10.0, end_secs: 14.0 });
        e.observe(DeckId::A, 5.0, true); // scrubbed out behind IN
        let cmds = e.move_loop(DeckId::A, 50.0);
        assert!(seek_of(&cmds, DeckId::A).is_none(), "the patient rule still holds");
        assert!((e.deck(DeckId::A).position_secs - 5.0).abs() < 1e-9);
    }

    #[test]
    fn a_move_that_runs_off_the_track_is_ignored() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // 300 s
        e.deck_mut(DeckId::A).loop_span =
            Some(LoopSpan { start_secs: 10.0, end_secs: 14.0 });
        e.observe(DeckId::A, 50.0, true);
        assert!(e.move_loop(DeckId::A, 299.0).is_empty(), "OUT would pass the end");
        assert!(e.move_loop(DeckId::A, -5.0).is_empty(), "IN would pass zero");
        let span = e.deck(DeckId::A).loop_span.expect("a span");
        assert!((span.start_secs - 10.0).abs() < 1e-9, "the span must not budge");
    }

    #[test]
    fn moving_with_no_loop_running_does_nothing() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        assert!(e.move_loop(DeckId::A, 40.0).is_empty());
        assert!(e.deck(DeckId::A).loop_span.is_none());
    }

    #[test]
    fn a_running_loop_saves_once_and_a_saved_one_recalls_later() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_beats = 4;
        e.observe(DeckId::A, 10.0, true);
        e.loop_in(DeckId::A); // 10.0 .. 12.0
        assert!(e.save_loop(DeckId::A), "the green marker click");
        assert!(!e.save_loop(DeckId::A), "saving the same span twice is a no-op");
        assert_eq!(e.deck(DeckId::A).loop_slots.len(), 1);
        // Exit, wander off, recall: back inside the saved span.
        e.toggle_loop(DeckId::A);
        e.observe(DeckId::A, 50.0, true);
        let cmds = e.recall_loop(DeckId::A, 0);
        let span = e.deck(DeckId::A).loop_span.expect("recalled");
        assert!((span.start_secs - 10.0).abs() < 1e-9 && (span.len_secs() - 2.0).abs() < 1e-9);
        assert_eq!(seek_of(&cmds, DeckId::A), Some(10.0), "recall jumps in");
        // Recall with nothing there is inert.
        assert!(e.recall_loop(DeckId::A, 7).is_empty());
        // The marker is a toggle: clicking it again while ITS loop runs
        // exits, exactly like the RELOOP/EXIT button — and a third click
        // goes back in.
        let cmds = e.recall_loop(DeckId::A, 0);
        assert!(!e.deck(DeckId::A).loop_on(), "second click exits");
        assert!(cmds.contains(&DeckCmd::SetLoopSpan { deck: DeckId::A, span: None }));
        e.recall_loop(DeckId::A, 0);
        assert!(e.deck(DeckId::A).loop_on(), "third click re-enters");
        // Dragging the marker away deletes the slot; the running span and
        // an out-of-range index are both left alone.
        e.delete_loop_slot(DeckId::A, 5);
        assert_eq!(e.deck(DeckId::A).loop_slots.len(), 1);
        e.delete_loop_slot(DeckId::A, 0);
        assert!(e.deck(DeckId::A).loop_slots.is_empty());
        assert!(e.deck(DeckId::A).loop_span.is_some(), "the sound is untouched");
    }

    #[test]
    fn the_cue_point_drags_in_quant_units_and_dies_with_the_track() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // 0.5 s a beat, 300 s
        assert_eq!(e.deck(DeckId::A).cue_secs, 0.0, "CUE starts at the top");
        // QUANT off: the marker lands exactly where dropped, clamped.
        e.set_cue(DeckId::A, 10.3);
        assert!((e.deck(DeckId::A).cue_secs - 10.3).abs() < 1e-9);
        e.set_cue(DeckId::A, 10_000.0);
        assert!((e.deck(DeckId::A).cue_secs - 300.0).abs() < 1e-9);
        // QUANT on: whole units against the cue's own phase, the loop-drag law.
        e.set_cue(DeckId::A, 10.3);
        e.set_snap_beats(4);
        e.set_cue(DeckId::A, 20.0);
        let moved = e.deck(DeckId::A).cue_secs - 10.3;
        let steps = moved / 2.0; // 4 beats at 120 BPM
        assert!((steps - steps.round()).abs() < 1e-9, "moved {steps} units");
        // A fresh install puts the marker back at the top.
        let (d, g) = load_gen(&e.click(item(2), DeckTarget::A));
        e.track_ready(d, g, 200.0);
        assert_eq!(e.deck(DeckId::A).cue_secs, 0.0);
    }

    #[test]
    fn infinity_arms_bookmarks_that_jump_and_never_loop() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_beats = 64;
        e.loop_double(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_beats, LOOP_BEATS_INF, "64 doubles into ∞");
        e.loop_double(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_beats, LOOP_BEATS_INF, "∞ is the top");
        // With ∞ armed, `[` places a GREEN bookmark: an IN with no OUT —
        // no loop, no sound change, and nothing saved yet.
        e.observe(DeckId::A, 10.2, true);
        assert!(e.loop_in(DeckId::A).is_empty());
        assert!(e.deck(DeckId::A).loop_span.is_none(), "a bookmark never loops");
        assert_eq!(e.deck(DeckId::A).bookmark, Some(10.2));
        assert!(e.deck(DeckId::A).loop_slots.is_empty(), "green until clicked");
        // `]` has no meaning without an OUT.
        assert!(e.loop_out(DeckId::A).is_empty());
        assert!(e.deck(DeckId::A).loop_armed.is_none());
        // The chip click saves it — same gesture as a loop's green chip.
        assert!(e.save_loop(DeckId::A), "the green bookmark chip click");
        let slot = e.deck(DeckId::A).loop_slots[0];
        assert!((slot.start_secs - 10.2).abs() < 1e-9 && slot.len_secs() < 1e-9);
        // Clicking the saved bookmark is a plain exact jump.
        e.observe(DeckId::A, 50.0, true);
        let cmds = e.recall_loop(DeckId::A, 0);
        assert_eq!(seek_of(&cmds, DeckId::A), Some(10.2));
        assert!(e.deck(DeckId::A).loop_span.is_none());
    }

    #[test]
    fn the_count_dial_converts_between_bookmark_and_loop() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // 0.5 s a beat
        e.deck_mut(DeckId::A).loop_beats = LOOP_BEATS_INF;
        e.observe(DeckId::A, 10.2, true);
        e.loop_in(DeckId::A); // the current bookmark
        // Dial down: the out point returns 64 beats later and loop mode
        // becomes active — the playhead is outside, so it jumps in.
        e.observe(DeckId::A, 50.0, true);
        let cmds = e.loop_halve(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_beats, 64);
        let span = e.deck(DeckId::A).loop_span.expect("loop mode is back");
        assert!((span.start_secs - 10.2).abs() < 1e-9);
        assert!((span.len_secs() - 32.0).abs() < 1e-9, "64 beats at 120 BPM");
        assert!(e.deck(DeckId::A).bookmark.is_none(), "the bookmark became the loop");
        assert_eq!(seek_of(&cmds, DeckId::A), Some(10.2));
        // Dial back up to ∞: loop mode exits, the out point is gone, and
        // the current object is a bookmark at the same IN again.
        let cmds = e.loop_double(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_beats, LOOP_BEATS_INF);
        assert!(e.deck(DeckId::A).loop_span.is_none(), "no out point, no loop");
        assert_eq!(e.deck(DeckId::A).bookmark, Some(10.2));
        assert!(cmds.contains(&DeckCmd::SetLoopSpan { deck: DeckId::A, span: None }));
        // And a plain 4-beat loop dialed all the way up collapses too.
        e.deck_mut(DeckId::A).loop_beats = 4;
        e.observe(DeckId::A, 20.0, true);
        e.loop_in(DeckId::A);
        assert!(e.deck(DeckId::A).bookmark.is_none(), "engaging clears the bookmark");
        for _ in 0..5 {
            e.loop_double(DeckId::A); // 8, 16, 32, 64, ∞
        }
        assert!(e.deck(DeckId::A).loop_span.is_none());
        assert_eq!(e.deck(DeckId::A).bookmark, Some(20.0));
    }

    #[test]
    fn picking_a_count_resizes_in_place_and_converts_at_the_ends() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_beats = 4;
        e.observe(DeckId::A, 10.0, true);
        e.loop_in(DeckId::A); // 10.0 .. 12.0
        assert!(e.save_loop(DeckId::A));
        // Pick 16: the running loop resizes to exactly 16 beats from IN,
        // marker following like the cutter.
        e.set_loop_beats(DeckId::A, 16);
        let span = e.deck(DeckId::A).loop_span.expect("still looping");
        assert!((span.len_secs() - 8.0).abs() < 1e-9);
        assert!((e.deck(DeckId::A).loop_slots[0].len_secs() - 8.0).abs() < 1e-9);
        // Pick the infinity count: collapse to a bookmark; pick 8: the out
        // point returns at the picked length.
        e.set_loop_beats(DeckId::A, LOOP_BEATS_INF);
        assert!(e.deck(DeckId::A).loop_span.is_none());
        assert_eq!(e.deck(DeckId::A).bookmark, Some(10.0));
        e.set_loop_beats(DeckId::A, 8);
        let span = e.deck(DeckId::A).loop_span.expect("loop mode is back");
        assert!((span.len_secs() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn clearing_marks_takes_the_bookmark_too_and_a_snapshot_puts_them_back() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_beats = 4;
        e.observe(DeckId::A, 10.0, true);
        e.loop_in(DeckId::A);
        e.save_loop(DeckId::A);
        e.deck_mut(DeckId::A).bookmark = Some(42.0);
        let slots = e.deck(DeckId::A).loop_slots.clone();
        e.clear_loop_slots(DeckId::A);
        assert!(e.deck(DeckId::A).loop_slots.is_empty(), "blue marks gone");
        assert!(e.deck(DeckId::A).bookmark.is_none(), "the bookmark goes with them");
        assert!(e.deck(DeckId::A).loop_span.is_some(), "audio untouched");
        // CANCEL's undo: the snapshot goes back exactly as it was.
        e.restore_marks(DeckId::A, slots, Some(42.0));
        assert_eq!(e.deck(DeckId::A).loop_slots.len(), 1);
        assert_eq!(e.deck(DeckId::A).bookmark, Some(42.0));
        // A snapshot longer than the row still respects the cap.
        let many = (0..20)
            .map(|i| LoopSpan { start_secs: i as f64, end_secs: i as f64 })
            .collect();
        e.restore_marks(DeckId::A, many, None);
        assert_eq!(e.deck(DeckId::A).loop_slots.len(), LOOP_SLOT_CAP);
        assert!(e.deck(DeckId::A).bookmark.is_none());
    }

    #[test]
    fn resizing_a_saved_loop_updates_its_marker() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_beats = 4;
        e.observe(DeckId::A, 10.0, true);
        e.loop_in(DeckId::A); // 10.0 .. 12.0
        assert!(e.save_loop(DeckId::A));
        // Halve the running loop: the marker's stored span follows.
        e.loop_halve(DeckId::A);
        let slot = e.deck(DeckId::A).loop_slots[0];
        assert!((slot.start_secs - 10.0).abs() < 1e-9, "the IN holds");
        assert!((slot.len_secs() - 1.0).abs() < 1e-9, "the duration followed");
        // A resize of an UNSAVED loop touches no markers.
        e.delete_loop_slot(DeckId::A, 0);
        e.loop_double(DeckId::A);
        assert!(e.deck(DeckId::A).loop_slots.is_empty());
    }

    #[test]
    fn saved_loops_die_with_the_track_and_respect_the_cap() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_beats = 1;
        for i in 0..(LOOP_SLOT_CAP + 2) {
            e.observe(DeckId::A, 10.0 + i as f64, true);
            e.loop_in(DeckId::A);
            e.save_loop(DeckId::A);
        }
        assert_eq!(e.deck(DeckId::A).loop_slots.len(), LOOP_SLOT_CAP, "the cap holds");
        // A fresh install forgets them: they were positions on THAT track.
        let (d, g) = load_gen(&e.click(item(2), DeckTarget::A));
        e.track_ready(d, g, 200.0);
        assert!(e.deck(DeckId::A).loop_slots.is_empty());
    }

    #[test]
    fn the_snap_unit_is_one_global_shared_by_both_decks() {
        let mut e = DeckEngine::new();
        e.set_snap_beats(8);
        assert_eq!(e.snap_beats, 8);
        // There is no per-deck unit to disagree with it.
        e.set_snap_beats(0);
        assert_eq!(e.snap_beats, 0);
    }

    #[test]
    fn swap_carries_the_whole_loop_state() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 128.0, 0.0);
        e.deck_mut(DeckId::A).loop_beats = 16;
        e.deck_mut(DeckId::A).loop_span = Some(LoopSpan { start_secs: 8.0, end_secs: 10.0 });
        e.deck_mut(DeckId::B).loop_beats = 2;
        e.swap();
        assert_eq!(e.deck(DeckId::B).loop_beats, 16);
        assert_eq!(
            e.deck(DeckId::B).loop_span,
            Some(LoopSpan { start_secs: 8.0, end_secs: 10.0 })
        );
        assert_eq!(e.deck(DeckId::A).loop_beats, 2);
    }

    // -----------------------------------------------------------------
    // tempo matching, auto sync, scratch, tone, stems, queue
    // -----------------------------------------------------------------

    fn grid(bpm: f64, first_beat_secs: f64) -> TrackGrid {
        TrackGrid {
            bpm,
            beat_secs: 60.0 / bpm,
            first_beat_secs,
            downbeat_phase: 0,
            confidence: 0.9,
        }
    }

    /// Load a deck, complete its decode, and land its analysis.
    fn load_analysed(
        engine: &mut DeckEngine,
        deck: DeckId,
        seed: u8,
        bpm: f64,
        first_beat_secs: f64,
    ) {
        let target = match deck {
            DeckId::A => DeckTarget::A,
            DeckId::B => DeckTarget::B,
        };
        let (deck, gen) = load_gen(&engine.click(item(seed), target));
        engine.track_ready(deck, gen, 300.0);
        engine.grid_ready(deck, gen, grid(bpm, first_beat_secs));
    }

    fn rate_of(cmds: &[DeckCmd], want: DeckId) -> Option<f64> {
        cmds.iter().rev().find_map(|cmd| match cmd {
            DeckCmd::SetRate { deck, rate } if *deck == want => Some(*rate),
            _ => None,
        })
    }

    fn seek_of(cmds: &[DeckCmd], want: DeckId) -> Option<f64> {
        cmds.iter().rev().find_map(|cmd| match cmd {
            DeckCmd::SeekSeconds { deck, secs } if *deck == want => Some(*secs),
            _ => None,
        })
    }

    #[test]
    fn sync_matches_tempo_and_lands_the_follower_in_phase() {
        let leader = SyncView { grid: grid(128.0, 0.1), position_secs: 10.0, rate: 1.0 };
        let follower = SyncView { grid: grid(124.0, 0.05), position_secs: 30.0, rate: 1.0 };
        let plan = sync_plan(&leader, &follower, SyncQuantize::Beat).expect("plan");

        // Tempos match after the rate change.
        assert!(
            (follower.grid.effective_bpm(plan.rate) - leader.grid.effective_bpm(leader.rate))
                .abs()
                < 1e-9,
            "rate {} gives {} against {}",
            plan.rate,
            follower.grid.effective_bpm(plan.rate),
            leader.grid.bpm
        );
        // The seek lands on a beat boundary that shares the leader's phase.
        let landed = plan.seek_secs.expect("a phase move");
        let leader_phase = leader.grid.phase_at(leader.position_secs);
        let follower_phase = follower.grid.phase_at(landed);
        assert!(
            (follower_phase - leader_phase).abs() < 1e-9,
            "phase {follower_phase} vs {leader_phase}"
        );
        // …and it is the NEAREST such boundary: never more than half a beat.
        assert!(
            (landed - follower.position_secs).abs() <= follower.grid.beat_secs * 0.5 + 1e-9,
            "jumped {} s",
            landed - follower.position_secs
        );
    }

    #[test]
    fn sync_uses_half_or_double_time_across_an_octave() {
        // A 150 BPM track under a 75 BPM one plays at its own speed: one
        // beat in two lines up, and nobody hears a chipmunk.
        let leader = SyncView { grid: grid(150.0, 0.0), position_secs: 4.0, rate: 1.0 };
        let follower = SyncView { grid: grid(75.0, 0.0), position_secs: 9.0, rate: 1.0 };
        let plan = sync_plan(&leader, &follower, SyncQuantize::Beat).expect("plan");
        assert!((plan.rate - 1.0).abs() < 1e-9, "rate {}", plan.rate);

        // And the other way round.
        let plan = sync_plan(&follower, &leader, SyncQuantize::Beat).expect("plan");
        assert!((plan.rate - 1.0).abs() < 1e-9, "rate {}", plan.rate);
    }

    #[test]
    fn flipping_the_pulse_moves_every_ruling_half_a_beat_and_keeps_the_bars() {
        let mut engine = DeckEngine::new();
        let (deck, gen) = load_gen(&engine.click(item(1), DeckTarget::A));
        engine.track_ready(deck, gen, 240.0);
        // 120 BPM, first beat at 0.4 s and it is beat 2 of its bar.
        let grid = TrackGrid {
            bpm: 120.0,
            beat_secs: 0.5,
            first_beat_secs: 0.4,
            downbeat_phase: 2,
            confidence: 0.9,
        };
        engine.grid_ready(DeckId::A, gen, grid);
        let (flipped, _) = engine.flip_beat_phase(DeckId::A).expect("a grid to flip");
        // 0.4 + 0.25 = 0.65 wraps to 0.15: the ruling before the old first
        // beat, one beat earlier in the bar.
        assert!((flipped.first_beat_secs - 0.15).abs() < 1e-9, "{flipped:?}");
        assert_eq!(flipped.downbeat_phase, 1);
        // The downbeat's absolute time moved by exactly half a beat.
        let old_downbeat: f64 = 0.4 + 2.0 * 0.5;
        let new_downbeat = 0.15 + 3.0 * 0.5;
        assert!((new_downbeat - old_downbeat).abs() - 0.25 < 1e-9);
        // Flipping again goes BACK half a beat: exactly the original grid,
        // bars included.
        let (again, _) = engine.flip_beat_phase(DeckId::A).unwrap();
        assert!((again.first_beat_secs - 0.4).abs() < 1e-9, "{again:?}");
        assert_eq!(again.downbeat_phase, 2);
        assert!(!engine.deck(DeckId::A).phase_flipped);
    }

    #[test]
    fn a_beat_jump_moves_by_whole_beats_of_the_decks_grid() {
        let mut engine = DeckEngine::new();
        let (deck, gen) = load_gen(&engine.click(item(1), DeckTarget::A));
        engine.track_ready(deck, gen, 240.0);
        engine.grid_ready(
            DeckId::A,
            gen,
            TrackGrid { bpm: 120.0, beat_secs: 0.5, first_beat_secs: 0.0, downbeat_phase: 0, confidence: 0.9 },
        );
        engine.seek_secs(DeckId::A, 10.0);
        let cmds = engine.beat_jump(DeckId::A, 16.0);
        assert!(cmds.iter().any(|cmd| matches!(cmd, DeckCmd::SeekSeconds { secs, .. } if (*secs - 18.0).abs() < 1e-9)), "{cmds:?}");
        assert!((engine.deck(DeckId::A).position_secs - 18.0).abs() < 1e-9);
        engine.beat_jump(DeckId::A, -64.0);
        assert_eq!(engine.deck(DeckId::A).position_secs, 0.0, "clamped at the start");
    }

    #[test]
    fn a_bar_sync_lands_on_a_downbeat() {
        let leader = SyncView { grid: grid(120.0, 0.0), position_secs: 8.0, rate: 1.0 };
        let follower = SyncView { grid: grid(120.0, 0.0), position_secs: 33.3, rate: 1.0 };
        let plan = sync_plan(&leader, &follower, SyncQuantize::Bar).expect("plan");
        let landed = plan.seek_secs.expect("a move");
        // The leader is exactly on a downbeat (8 s at 120 = beat 16 = bar 4),
        // so the follower must land on one too.
        let beat = follower.grid.beat_at(landed);
        assert!((beat - beat.round()).abs() < 1e-9, "beat {beat}");
        assert!(
            follower.grid.is_downbeat(beat.round() as i64),
            "landed on beat {beat}, not a downbeat"
        );
    }

    #[test]
    fn sync_needs_two_grids() {
        let with = SyncView { grid: grid(120.0, 0.0), position_secs: 1.0, rate: 1.0 };
        let without = SyncView { grid: TrackGrid::default(), position_secs: 1.0, rate: 1.0 };
        assert!(sync_plan(&with, &without, SyncQuantize::Beat).is_none());
        assert!(sync_plan(&without, &with, SyncQuantize::Beat).is_none());
    }

    #[test]
    fn auto_sync_holds_the_cued_deck_to_the_playing_one() {
        let mut engine = DeckEngine::new();
        assert!(engine.auto_sync, "auto sync is on out of the box");
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.1);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 20.0, true);

        // Loading B while A plays: the analysis arriving is what engages it.
        let target = DeckTarget::B;
        let (deck, gen) = load_gen(&engine.click(item(2), target));
        engine.track_ready(deck, gen, 240.0);
        let cmds = engine.grid_ready(deck, gen, grid(100.0, 0.0));

        assert_eq!(engine.sync_leader(), Some(DeckId::A));
        assert!(engine.deck(DeckId::B).synced, "B must be held to A");
        let rate = rate_of(&cmds, DeckId::B).expect("a rate for B");
        assert!(
            (100.0 * rate - 128.0).abs() < 1e-9,
            "B plays at {} BPM",
            100.0 * rate
        );
        // A cued deck gets the bar-accurate landing: the same position
        // WITHIN the bar as the leader, so both hit their downbeat together.
        let landed = seek_of(&cmds, DeckId::B).expect("a phase move for B");
        let leader_bar = engine.deck(DeckId::A).grid.unwrap().bar_at(20.0).rem_euclid(1.0);
        let follower_bar =
            engine.deck(DeckId::B).grid.unwrap().bar_at(landed).rem_euclid(1.0);
        assert!(
            (leader_bar - follower_bar).abs() < 1e-9,
            "bar phase {follower_bar} vs {leader_bar}"
        );
        // The leader is untouched.
        assert!(rate_of(&cmds, DeckId::A).is_none());
        assert!((engine.deck(DeckId::A).rate - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_leaders_tempo_change_carries_the_follower_with_it() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 100.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 12.0, true);
        engine.observe(DeckId::B, 5.0, false);
        engine.apply_auto_sync();
        assert!(engine.deck(DeckId::B).synced);

        // Pull the leader 4% up: the follower must follow to the same tempo.
        let cmds = engine.set_pitch(DeckId::A, 0.5); // half of ±8% = +4%
        let leader_rate = engine.deck(DeckId::A).rate;
        assert!((leader_rate - 1.04).abs() < 1e-9, "leader rate {leader_rate}");
        let follower_rate = rate_of(&cmds, DeckId::B).expect("B follows");
        assert!(
            (100.0 * follower_rate - 120.0 * leader_rate).abs() < 1e-9,
            "follower at {} vs leader {}",
            100.0 * follower_rate,
            120.0 * leader_rate
        );
        // Nudging the LEADER does not opt the leader out of leading.
        assert_eq!(engine.sync_leader(), Some(DeckId::A));
    }

    #[test]
    fn touching_the_followers_own_pitch_disengages_auto_sync() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 100.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 12.0, true);
        engine.apply_auto_sync();
        assert!(engine.deck(DeckId::B).synced);

        engine.set_pitch(DeckId::B, -0.25);
        assert!(!engine.deck(DeckId::B).synced, "a manual nudge breaks the lock");
        assert!(engine.deck(DeckId::B).auto_opt_out);
        let manual_rate = engine.deck(DeckId::B).rate;

        // Auto sync now leaves B alone, however the leader moves.
        engine.observe(DeckId::A, 20.0, true);
        engine.apply_auto_sync();
        assert!((engine.deck(DeckId::B).rate - manual_rate).abs() < 1e-12);
        assert!(!engine.deck(DeckId::B).synced);

        // A deliberate SYNC press takes it back.
        let cmds = engine.sync(DeckId::B, true);
        assert!(engine.deck(DeckId::B).synced);
        assert!(!engine.deck(DeckId::B).auto_opt_out);
        assert!(rate_of(&cmds, DeckId::B).is_some());
    }

    #[test]
    fn scratching_suspends_the_phase_lock_and_relocks_on_release() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 120.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 10.0, true);
        engine.apply_auto_sync();

        let cmds = engine.scratch(DeckId::B, ScratchMotion::Grab);
        assert_eq!(
            cmds.first(),
            Some(&DeckCmd::Scratch { deck: DeckId::B, motion: ScratchMotion::Grab })
        );
        assert!(engine.deck(DeckId::B).scratching);
        // While a hand is on the record no phase move is issued.
        engine.observe(DeckId::A, 10.4, true);
        engine.observe(DeckId::B, 3.17, false);
        let cmds = engine.apply_auto_sync();
        assert!(seek_of(&cmds, DeckId::B).is_none(), "no seek under a hand");
        let cmds = engine.scratch(DeckId::B, ScratchMotion::Move { rate: -1.5 });
        assert!(seek_of(&cmds, DeckId::B).is_none());

        // Letting go re-locks it.
        let cmds = engine.scratch(DeckId::B, ScratchMotion::Release);
        assert!(!engine.deck(DeckId::B).scratching);
        assert!(seek_of(&cmds, DeckId::B).is_some(), "release must re-lock: {cmds:?}");
    }

    /// Phase difference between the decks, in beats of the follower's grid.
    fn phase_gap(engine: &DeckEngine) -> f64 {
        let a = engine.deck(DeckId::A);
        let b = engine.deck(DeckId::B);
        let (Some(ga), Some(gb)) = (a.grid, b.grid) else { return f64::NAN };
        let pa = ga.beat_at(a.position_secs).rem_euclid(1.0);
        let pb = gb.beat_at(b.position_secs).rem_euclid(1.0);
        let raw = (pa - pb).abs();
        raw.min(1.0 - raw)
    }

    #[test]
    fn a_seek_on_a_synced_deck_re_locks_the_phase() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.1);
        load_analysed(&mut engine, DeckId::B, 2, 124.0, 0.05);
        engine.play_pause(DeckId::A);
        engine.play_pause(DeckId::B);
        engine.set_crossfader(0.0); // A is the audible one
        engine.observe(DeckId::A, 30.0, true);
        engine.observe(DeckId::B, 20.0, true);
        engine.apply_auto_sync();
        assert_eq!(engine.sync_leader(), Some(DeckId::A));
        assert!(phase_gap(&engine) < 1e-9, "starts locked");

        // Click somewhere arbitrary in deck B — over and over, at offsets
        // that have nothing to do with either grid.
        let beat = engine.deck(DeckId::B).grid.unwrap().beat_secs;
        for want in [61.234_f64, 17.77, 145.001, 3.14159] {
            let cmds = engine.seek_secs(DeckId::B, want);
            let landed = engine.deck(DeckId::B).position_secs;
            assert!(
                phase_gap(&engine) < 1e-9,
                "seek to {want} left a phase gap of {} beats",
                phase_gap(&engine)
            );
            assert!(
                (landed - want).abs() <= beat * 0.5 + 1e-9,
                "seek to {want} landed at {landed}, {} beats away",
                (landed - want).abs() / beat
            );
            // The tempo match is untouched by a seek.
            assert!(
                (124.0 * engine.deck(DeckId::B).rate - 128.0).abs() < 1e-9,
                "a seek must not change the tempo"
            );
            assert!(cmds
                .iter()
                .any(|cmd| matches!(cmd, DeckCmd::SeekSeconds { deck: DeckId::B, .. })));
        }
    }

    #[test]
    fn seeking_the_leader_pulls_the_follower_back_into_phase() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 96.0, 0.3);
        engine.play_pause(DeckId::A);
        engine.play_pause(DeckId::B);
        engine.set_crossfader(0.0);
        engine.observe(DeckId::A, 12.0, true);
        engine.observe(DeckId::B, 40.0, true);
        engine.apply_auto_sync();
        let follower_before = engine.deck(DeckId::B).position_secs;

        // Move the LIVE deck: the follower has to come with it.
        engine.seek_secs(DeckId::A, 91.618);
        assert!(
            phase_gap(&engine) < 1e-9,
            "leader seek left {} beats of gap",
            phase_gap(&engine)
        );
        let moved = (engine.deck(DeckId::B).position_secs - follower_before).abs();
        let beat = engine.deck(DeckId::B).grid.unwrap().beat_secs;
        assert!(moved <= beat * 0.5 + 1e-9, "follower jumped {moved} s");
    }

    #[test]
    fn a_seek_without_auto_sync_moves_nothing_else() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 96.0, 0.3);
        engine.set_auto_sync(false);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 12.0, true);
        let before = engine.deck(DeckId::B).position_secs;
        let cmds = engine.seek_secs(DeckId::A, 33.3);
        assert_eq!(cmds, vec![DeckCmd::SeekSeconds { deck: DeckId::A, secs: 33.3 }]);
        assert_eq!(engine.deck(DeckId::B).position_secs, before);
    }

    #[test]
    fn scratch_needs_a_loaded_deck() {
        let mut engine = DeckEngine::new();
        assert!(engine.scratch(DeckId::A, ScratchMotion::Grab).is_empty());
        assert!(!engine.deck(DeckId::A).scratching);
    }

    #[test]
    fn auto_sync_can_be_turned_off_and_back_on() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 100.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 12.0, true);
        engine.apply_auto_sync();
        assert!(engine.deck(DeckId::B).synced);

        engine.set_auto_sync(false);
        assert!(!engine.deck(DeckId::B).synced);
        engine.observe(DeckId::A, 24.0, true);
        assert!(engine.apply_auto_sync().is_empty(), "off means off");

        engine.set_auto_sync(true);
        assert!(engine.deck(DeckId::B).synced);
        // …and it is genuinely tempo-matched again, whether or not the rate
        // needed to change to get there.
        assert!(
            (engine.deck(DeckId::B).effective_bpm().unwrap()
                - engine.deck(DeckId::A).effective_bpm().unwrap())
            .abs()
                < 1e-9
        );
    }

    #[test]
    fn a_stale_analysis_never_reaches_the_deck() {
        let mut engine = DeckEngine::new();
        let (deck, first) = load_gen(&engine.click(item(1), DeckTarget::A));
        let (_, second) = load_gen(&engine.click(item(2), DeckTarget::A));
        engine.track_ready(deck, second, 100.0);
        assert!(engine.grid_ready(deck, first, grid(120.0, 0.0)).is_empty());
        assert!(engine.deck(DeckId::A).grid.is_none(), "stale grid must not land");
        engine.grid_ready(deck, second, grid(126.0, 0.0));
        assert_eq!(engine.deck(DeckId::A).grid.map(|g| g.bpm), Some(126.0));
        // Loading again clears it rather than syncing to the old tempo.
        engine.click(item(3), DeckTarget::A);
        assert!(engine.deck(DeckId::A).grid.is_none());
    }

    #[test]
    fn eq_kills_zero_a_band_and_release_restores_the_knob() {
        let mut engine = DeckEngine::new();
        let cmds = engine.set_eq(DeckId::A, 0, 0.7);
        assert_eq!(cmds, vec![DeckCmd::SetEqBand { deck: DeckId::A, band: 0, gain: 0.7 }]);
        let cmds = engine.toggle_eq_kill(DeckId::A, 0);
        assert_eq!(cmds, vec![DeckCmd::SetEqBand { deck: DeckId::A, band: 0, gain: 0.0 }]);
        assert!(engine.deck(DeckId::A).eq_kill[0]);
        // The knob value survives the kill.
        assert!((engine.deck(DeckId::A).eq[0] - 0.7).abs() < 1e-6);
        let cmds = engine.toggle_eq_kill(DeckId::A, 0);
        assert_eq!(cmds, vec![DeckCmd::SetEqBand { deck: DeckId::A, band: 0, gain: 0.7 }]);
        // Out-of-range bands are refused, not clamped into a neighbour.
        assert!(engine.set_eq(DeckId::A, 7, 0.0).is_empty());
        assert!(engine.toggle_eq_kill(DeckId::A, 7).is_empty());
        // The filter is per deck and does not leak across.
        engine.set_filter(DeckId::A, 0.2);
        assert!((engine.deck(DeckId::A).filter - 0.2).abs() < 1e-6);
        assert!((engine.deck(DeckId::B).filter - 0.5).abs() < 1e-6);
    }

    #[test]
    fn solo_isolates_additively_and_mute_wins_on_its_own_lane() {
        let mut engine = DeckEngine::new();
        let (deck, gen) = load_gen(&engine.click(item(1), DeckTarget::A));
        engine.track_ready(deck, gen, 100.0);
        engine.stems_ready(DeckId::A, gen);
        // Solo lane 0: the rest of the bus goes quiet, lane 0 plays.
        let cmds = engine.toggle_stem_solo(DeckId::A, 0);
        assert_eq!(cmds.len(), STEM_COUNT, "a solo re-publishes every lane");
        assert!((engine.deck(DeckId::A).stem_effective(0) - 1.0).abs() < 1e-6);
        for stem in 1..STEM_COUNT {
            assert_eq!(engine.deck(DeckId::A).stem_effective(stem), 0.0, "lane {stem}");
        }
        // Additive: soloing a second lane widens the set, no radio buttons.
        engine.toggle_stem_solo(DeckId::A, 1);
        assert!((engine.deck(DeckId::A).stem_effective(1) - 1.0).abs() < 1e-6);
        assert_eq!(engine.deck(DeckId::A).stem_effective(2), 0.0);
        // Mute beats solo on its own lane, the way every console does it.
        engine.toggle_stem_kill(DeckId::A, 0);
        assert_eq!(engine.deck(DeckId::A).stem_effective(0), 0.0);
        // Clearing the solos: the mute holds, everything else comes back.
        engine.toggle_stem_solo(DeckId::A, 0);
        engine.toggle_stem_solo(DeckId::A, 1);
        assert_eq!(engine.deck(DeckId::A).stem_effective(0), 0.0);
        assert!((engine.deck(DeckId::A).stem_effective(2) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn eq_solo_is_the_isolator_move_and_mute_still_wins() {
        let mut engine = DeckEngine::new();
        let (deck, gen) = load_gen(&engine.click(item(1), DeckTarget::A));
        engine.track_ready(deck, gen, 100.0);
        // Solo the low band: mid and high go quiet, low keeps its knob.
        let cmds = engine.toggle_eq_solo(DeckId::A, 0);
        assert_eq!(cmds.len(), 3, "an EQ solo re-publishes all three bands");
        assert!((engine.deck(DeckId::A).eq_effective(0) - 1.0).abs() < 1e-6);
        assert_eq!(engine.deck(DeckId::A).eq_effective(1), 0.0);
        assert_eq!(engine.deck(DeckId::A).eq_effective(2), 0.0);
        // Additive with a second band, and a muted band stays silent even
        // while soloed.
        engine.toggle_eq_solo(DeckId::A, 2);
        assert!((engine.deck(DeckId::A).eq_effective(2) - 1.0).abs() < 1e-6);
        engine.toggle_eq_kill(DeckId::A, 0);
        assert_eq!(engine.deck(DeckId::A).eq_effective(0), 0.0);
        // Clear the solos: the mute holds, the rest of the tone returns.
        engine.toggle_eq_solo(DeckId::A, 0);
        engine.toggle_eq_solo(DeckId::A, 2);
        assert_eq!(engine.deck(DeckId::A).eq_effective(0), 0.0);
        assert!((engine.deck(DeckId::A).eq_effective(1) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn solo_is_remembered_but_silent_before_the_stems_arrive() {
        let mut engine = DeckEngine::new();
        let (deck, gen) = load_gen(&engine.click(item(1), DeckTarget::A));
        engine.track_ready(deck, gen, 100.0);
        assert!(engine.toggle_stem_solo(DeckId::A, 2).is_empty(), "no stems, no commands");
        assert!(engine.deck(DeckId::A).stem_solo[2], "but the intent is kept");
        // With no stems the deck plays the full mix regardless of solos.
        assert!((engine.deck(DeckId::A).stem_effective(0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_process_mode_cycles_and_the_middle_state_uses_without_computing() {
        // The click order, and the two questions every gate asks it.
        let live = ProcessMode::Live;
        assert_eq!(live.next(), ProcessMode::Cached);
        assert_eq!(live.next().next(), ProcessMode::Off);
        assert_eq!(live.next().next().next(), ProcessMode::Live);
        assert!(live.computes() && live.shows(), "green does both");
        assert!(!ProcessMode::Cached.computes(), "yellow starts nothing");
        assert!(ProcessMode::Cached.shows(), "but mixes what exists");
        assert!(!ProcessMode::Off.computes() && !ProcessMode::Off.shows(), "red does neither");
        // Cached keeps the knobs live — it is a compute switch, not a mute.
        let mut engine = DeckEngine::new();
        let (deck, gen) = load_gen(&engine.click(item(1), DeckTarget::A));
        engine.track_ready(deck, gen, 100.0);
        engine.stems_ready(DeckId::A, gen);
        engine.set_stems_mode(DeckId::A, ProcessMode::Cached);
        assert!(engine.deck(DeckId::A).stems_ready, "yellow still mixes");
        assert!(!engine.stems_ready(DeckId::A, gen).is_empty(), "and still accepts results");
    }

    #[test]
    fn the_stems_switch_stands_the_knobs_down_and_blocks_late_results() {
        let mut engine = DeckEngine::new();
        let (deck, gen) = load_gen(&engine.click(item(1), DeckTarget::A));
        engine.track_ready(deck, gen, 100.0);
        engine.stems_ready(DeckId::A, gen);
        engine.toggle_stem_kill(DeckId::A, 1);
        assert_eq!(engine.deck(DeckId::A).stem_effective(1), 0.0);
        // Off: full mix again, whatever the lane controls say.
        engine.set_stems_mode(DeckId::A, ProcessMode::Off);
        assert!(!engine.deck(DeckId::A).stems_ready);
        assert!((engine.deck(DeckId::A).stem_effective(1) - 1.0).abs() < 1e-6);
        // A worker result racing the toggle must not re-arm the knobs.
        assert!(engine.stems_ready(DeckId::A, gen).is_empty());
        assert!(!engine.deck(DeckId::A).stems_ready);
        // Back on: ready returns through the normal path, intent intact.
        engine.set_stems_mode(DeckId::A, ProcessMode::Live);
        engine.stems_ready(DeckId::A, gen);
        assert!(engine.deck(DeckId::A).stems_ready);
        assert_eq!(engine.deck(DeckId::A).stem_effective(1), 0.0, "the kill survived");
    }

    #[test]
    fn stem_knobs_are_inert_until_the_stems_arrive() {
        let mut engine = DeckEngine::new();
        let (deck, gen) = load_gen(&engine.click(item(1), DeckTarget::A));
        engine.track_ready(deck, gen, 100.0);
        assert!(!engine.deck(DeckId::A).stems_ready);
        // The knob remembers the setting but sends nothing to the mixer.
        assert!(engine.set_stem(DeckId::A, 0, 0.2).is_empty());
        assert!((engine.deck(DeckId::A).stem_gain[0] - 0.2).abs() < 1e-6);
        assert!(engine.toggle_stem_kill(DeckId::A, 1).is_empty());

        // When the separation lands, every lane is published at once.
        let cmds = engine.stems_ready(DeckId::A, gen);
        assert_eq!(cmds.len(), STEM_COUNT);
        assert!(cmds.contains(&DeckCmd::SetStemGain { deck: DeckId::A, stem: 0, gain: 0.2 }));
        assert!(cmds.contains(&DeckCmd::SetStemGain { deck: DeckId::A, stem: 1, gain: 0.0 }));
        assert!(cmds.contains(&DeckCmd::SetStemGain { deck: DeckId::A, stem: 3, gain: 1.0 }));
        // Now the knobs are live.
        assert_eq!(
            engine.set_stem(DeckId::A, 2, 0.5),
            vec![DeckCmd::SetStemGain { deck: DeckId::A, stem: 2, gain: 0.5 }]
        );
        // A stale stem completion is ignored.
        assert!(engine.stems_ready(DeckId::A, gen + 99).is_empty());
        // A new track starts without stems again.
        engine.click(item(2), DeckTarget::A);
        assert!(!engine.deck(DeckId::A).stems_ready);
    }

    #[test]
    fn the_queue_fills_the_free_deck_and_never_the_live_one() {
        let mut engine = DeckEngine::new();
        // A is live; the queue must land on B.
        let (deck, gen) = load_gen(&engine.click(item(1), DeckTarget::A));
        engine.track_ready(deck, gen, 100.0);
        engine.play_pause(DeckId::A);

        let cmds = engine.enqueue(item(2));
        let (target, gen_b) = load_gen(&cmds);
        assert_eq!(target, DeckId::B, "the live deck is never taken");
        assert!(engine.queue().is_empty(), "the head was consumed");
        engine.track_ready(target, gen_b, 90.0);

        // With both decks busy the queue simply waits.
        engine.enqueue(item(3));
        engine.enqueue(item(4));
        assert_eq!(engine.queue().len(), 2);
        // Duplicates are refused.
        engine.enqueue(item(3));
        assert_eq!(engine.queue().len(), 2);
        // Removing by hand works, and a click can force one onto a deck.
        engine.dequeue(0);
        assert_eq!(engine.queue().len(), 1);
        let cmds = engine.load_queued(0, DeckTarget::B);
        assert_eq!(load_gen(&cmds).0, DeckId::B);
        assert!(engine.queue().is_empty());
        engine.clear_queue();
        assert!(engine.pump_queue().is_empty());
    }

    #[test]
    fn a_queued_row_can_be_carried_to_another_spot_in_the_order() {
        let mut engine = DeckEngine::new();
        // Both decks busy, so the queue holds everything it is given.
        for (deck, target) in [(1, DeckTarget::A), (2, DeckTarget::B)] {
            let (id, gen) = load_gen(&engine.click(item(deck), target));
            engine.track_ready(id, gen, 100.0);
            engine.play_pause(id);
        }
        for track in 3..=6 {
            engine.enqueue(item(track));
        }
        let order = |engine: &DeckEngine| -> Vec<AssetId> {
            engine.queue().iter().map(|item| item.asset).collect()
        };
        let before = order(&engine);
        assert_eq!(before.len(), 4);

        // Last to first.
        assert!(engine.move_queued(3, 0));
        assert_eq!(order(&engine), vec![before[3], before[0], before[1], before[2]]);
        // And back down one spot.
        assert!(engine.move_queued(0, 1));
        assert_eq!(order(&engine), vec![before[0], before[3], before[1], before[2]]);
        // A move that goes nowhere, and moves off the end, change nothing.
        let held = order(&engine);
        assert!(!engine.move_queued(2, 2));
        assert!(!engine.move_queued(9, 0));
        assert!(!engine.move_queued(0, 9));
        assert_eq!(order(&engine), held);
    }

    #[test]
    fn a_finished_deck_takes_the_next_queued_track() {
        let mut engine = DeckEngine::new();
        let (deck, gen) = load_gen(&engine.click(item(1), DeckTarget::A));
        engine.track_ready(deck, gen, 100.0);
        engine.play_pause(DeckId::A);
        let (deck_b, gen_b) = load_gen(&engine.click(item(2), DeckTarget::B));
        engine.track_ready(deck_b, gen_b, 100.0);
        engine.play_pause(DeckId::B);
        engine.enqueue(item(3));
        assert_eq!(engine.queue().len(), 1, "both decks are playing");

        // B runs out: the queue takes the deck that just freed up.
        engine.set_crossfader(0.0); // A is the audible one
        let cmds = engine.track_ended(DeckId::B);
        assert!(!engine.deck(DeckId::B).playing);
        // The ended deck still holds its track, so nothing auto-loads until
        // the operator clears it; the queue stays intact rather than
        // silently replacing what is on the deck.
        assert!(cmds.is_empty());
        assert_eq!(engine.queue().len(), 1);
    }

    #[test]
    fn pitch_range_and_reset_keep_the_slider_honest() {
        let mut engine = DeckEngine::new();
        engine.set_pitch(DeckId::A, 1.0);
        assert!((engine.deck(DeckId::A).rate - 1.08).abs() < 1e-9, "±8% at full travel");
        // Widening the range keeps the audible tempo.
        engine.toggle_pitch_range(DeckId::A);
        assert_eq!(engine.deck(DeckId::A).pitch_range, PitchRange::Wide);
        assert!((engine.deck(DeckId::A).rate - 1.08).abs() < 1e-9, "tempo held");
        // …and the slider now has headroom to 16%.
        engine.set_pitch(DeckId::A, 1.0);
        assert!((engine.deck(DeckId::A).rate - 1.16).abs() < 1e-9);
        let cmds = engine.reset_pitch(DeckId::A);
        assert_eq!(cmds, vec![DeckCmd::SetRate { deck: DeckId::A, rate: 1.0 }]);
        assert!((engine.deck(DeckId::A).pitch).abs() < 1e-12);
        // Nudges step in whole percent of the range.
        engine.nudge_pitch(DeckId::A, 1.0);
        assert!((engine.deck(DeckId::A).rate - 1.0016).abs() < 1e-9, "{}", engine.deck(DeckId::A).rate);
    }

    #[test]
    fn seeking_clamps_to_the_track_and_needs_a_loaded_deck() {
        let mut engine = DeckEngine::new();
        assert!(engine.seek_secs(DeckId::A, 5.0).is_empty());
        let (deck, gen) = load_gen(&engine.click(item(1), DeckTarget::A));
        engine.track_ready(deck, gen, 30.0);
        assert_eq!(
            engine.seek_secs(DeckId::A, 999.0),
            vec![DeckCmd::SeekSeconds { deck: DeckId::A, secs: 30.0 }]
        );
        assert_eq!(
            engine.seek_secs(DeckId::A, -4.0),
            vec![DeckCmd::SeekSeconds { deck: DeckId::A, secs: 0.0 }]
        );
        assert!((engine.deck(DeckId::A).position_secs).abs() < 1e-12);
    }

    #[test]
    fn a_fresh_load_carries_the_channel_strip_onto_the_deck() {
        let mut engine = DeckEngine::new();
        engine.set_eq(DeckId::B, 2, 0.3);
        engine.toggle_eq_kill(DeckId::B, 0);
        engine.set_filter(DeckId::B, 0.8);
        engine.set_pitch(DeckId::B, 0.5);
        let (deck, gen) = load_gen(&engine.click(item(5), DeckTarget::B));
        let cmds = engine.track_ready(deck, gen, 60.0);
        assert!(cmds.contains(&DeckCmd::SetEqBand { deck: DeckId::B, band: 0, gain: 0.0 }));
        assert!(cmds.contains(&DeckCmd::SetEqBand { deck: DeckId::B, band: 2, gain: 0.3 }));
        assert!(cmds.contains(&DeckCmd::SetFilter { deck: DeckId::B, position: 0.8 }));
        assert!(cmds.contains(&DeckCmd::SetRate { deck: DeckId::B, rate: 1.04 }));
        assert!(cmds.contains(&DeckCmd::SetKeylock { deck: DeckId::B, on: true }));
    }

    #[test]
    fn key_lock_toggles_per_deck() {
        let mut engine = DeckEngine::new();
        assert!(engine.deck(DeckId::A).keylock, "key lock is the default");
        assert_eq!(
            engine.toggle_keylock(DeckId::A),
            vec![DeckCmd::SetKeylock { deck: DeckId::A, on: false }]
        );
        assert!(!engine.deck(DeckId::A).keylock);
        assert!(engine.deck(DeckId::B).keylock, "the other deck is untouched");
    }

    #[test]
    fn key_shift_steps_clamps_and_resets() {
        let mut engine = DeckEngine::new();
        assert_eq!(engine.deck(DeckId::A).key_shift, 0.0, "a fresh deck is in its own key");
        assert_eq!(
            engine.nudge_key_shift(DeckId::A, 1.0),
            vec![DeckCmd::SetKeyShift { deck: DeckId::A, semitones: 1.0 }]
        );
        engine.nudge_key_shift(DeckId::A, 1.0);
        engine.nudge_key_shift(DeckId::A, 1.0);
        assert_eq!(engine.deck(DeckId::A).key_shift, 3.0, "three steps up is three semitones");
        assert_eq!(engine.deck(DeckId::B).key_shift, 0.0, "the other deck is untouched");
        // An octave is the end of the travel, however hard the button is hit.
        for _ in 0..20 {
            engine.nudge_key_shift(DeckId::A, 1.0);
        }
        assert_eq!(engine.deck(DeckId::A).key_shift, KEY_SHIFT_MAX);
        assert_eq!(
            engine.reset_key_shift(DeckId::A),
            vec![DeckCmd::SetKeyShift { deck: DeckId::A, semitones: 0.0 }]
        );
        assert_eq!(engine.deck(DeckId::A).key_shift, 0.0);
    }

    #[test]
    fn key_shift_never_opts_out_of_auto_sync() {
        // The tempo slider is an override and drops the deck out of sync.
        // The key is not tempo: a deck can be locked to the beat and
        // transposed at the same time.
        let mut engine = DeckEngine::new();
        let cmds = engine.set_key_shift(DeckId::A, 5.0);
        assert!(!engine.deck(DeckId::A).auto_opt_out, "a key shift is not a tempo override");
        assert!(
            !cmds.iter().any(|cmd| matches!(cmd, DeckCmd::SetRate { .. })),
            "a key shift must not touch the rate"
        );
    }

    #[test]
    fn key_shift_replays_on_a_fresh_load() {
        let mut engine = DeckEngine::new();
        engine.set_key_shift(DeckId::B, 3.0);
        let (deck, gen) = load_gen(&engine.click(item(7), DeckTarget::B));
        let cmds = engine.track_ready(deck, gen, 60.0);
        assert!(cmds.contains(&DeckCmd::SetKeyShift { deck: DeckId::B, semitones: 3.0 }));
    }

    #[test]
    fn swap_inverts_fader_so_the_audible_program_is_unchanged() {
        let mut e = DeckEngine::new();
        let (da, ga) = load_gen(&e.click(item(1), DeckTarget::A));
        e.track_ready(da, ga, 30.0);
        e.set_crossfader(0.2);
        let before = crossfader_gains(e.crossfader, e.curve);
        let cmds = e.swap();
        assert!(cmds.contains(&DeckCmd::SwapVoices));
        let after = crossfader_gains(e.crossfader, e.curve);
        // Deck contents swapped and gains mirrored: what was A's gain is now
        // applied to the voice that moved to B.
        assert!((before.0 - after.1).abs() < 1e-6);
        assert!((before.1 - after.0).abs() < 1e-6);
        match &e.deck(DeckId::B).load {
            DeckLoad::Loaded { item } => assert_eq!(item.title, "track 1"),
            other => panic!("unexpected {other:?}"),
        }
    }

    // ---- external sync (following the room) -------------------------------

    /// The published clock as a leader: a grid with its origin at zero, so
    /// `position_secs` IS the beat position in seconds.
    fn external(bpm: f64, beats: f64) -> SyncView {
        SyncView { grid: grid(bpm, 0.0), position_secs: beats * 60.0 / bpm, rate: 1.0 }
    }

    #[test]
    fn ext_matches_the_rooms_tempo_and_trims_toward_its_phase() {
        // A 124 BPM track under a 128 BPM room, exactly in phase.
        let room = external(128.0, 8.0);
        let deck = SyncView { grid: grid(124.0, 0.0), position_secs: 8.0 * 60.0 / 124.0, rate: 1.0 };
        let follow = external_follow(&room, &deck, 0.08).expect("both have grids");
        assert!((follow.error_beats).abs() < 1e-9, "{follow:?}");
        assert!((follow.rate - 128.0 / 124.0).abs() < 1e-9, "{follow:?}");
        assert!(follow.within_envelope, "3.2% is inside ±8%");
        assert!(follow.reseek_secs.is_none());
    }

    #[test]
    fn ext_speeds_up_when_the_deck_is_behind_and_never_by_much() {
        let room = external(128.0, 8.2);
        // The deck is a fifth of a beat behind the room.
        let deck = SyncView { grid: grid(128.0, 0.0), position_secs: 8.0 * 60.0 / 128.0, rate: 1.0 };
        let follow = external_follow(&room, &deck, 0.08).unwrap();
        assert!(follow.error_beats > 0.15, "{follow:?}");
        assert!(follow.rate > 1.0, "behind means catch up: {follow:?}");
        assert!(follow.rate <= 1.0 + EXT_PHASE_TRIM + 1e-9, "and gently: {follow:?}");
        assert!(follow.reseek_secs.is_none(), "a fifth of a beat is trimmable");

        // Half a beat out is not drift — it was moved. Land it.
        let deck = SyncView { grid: grid(128.0, 0.0), position_secs: 8.7 * 60.0 / 128.0, rate: 1.0 };
        let follow = external_follow(&room, &deck, 0.08).unwrap();
        assert!(follow.reseek_secs.is_some(), "{follow:?}");
    }

    #[test]
    fn ext_folds_octaves_and_reports_walking_out_of_the_envelope() {
        // A 64 BPM track under a 128 BPM room plays at 1.0, one beat in two.
        let room = external(128.0, 4.0);
        let deck = SyncView { grid: grid(64.0, 0.0), position_secs: 2.0 * 60.0 / 64.0, rate: 1.0 };
        let follow = external_follow(&room, &deck, 0.08).unwrap();
        assert!((follow.rate - 1.0).abs() < 0.03, "{follow:?}");
        assert!(follow.within_envelope);
        // A room 12% faster than the track needs more stretch than ±8%: it
        // still follows, but the operator has to be told.
        let room = external(140.0, 0.0);
        let deck = SyncView { grid: grid(125.0, 0.0), position_secs: 0.0, rate: 1.0 };
        let follow = external_follow(&room, &deck, 0.08).unwrap();
        assert!(!follow.within_envelope, "{follow:?}");
        assert!(external_follow(&room, &deck, 0.16).unwrap().within_envelope, "±16% covers it");
    }

    #[test]
    fn the_sync_control_is_a_toggle_and_ext_is_its_own() {
        let mut e = DeckEngine::new();
        let (deck, gen) = load_gen(&e.click(item(1), DeckTarget::A));
        e.track_ready(deck, gen, 120.0);
        e.deck_mut(DeckId::A).grid = Some(grid(120.0, 0.0));
        assert_eq!(e.sync_mode(DeckId::A), SyncMode::Off);
        assert!(!e.any_external_sync());
        // With nothing to follow, SYNC claims this deck as the group's
        // reference — the press on the leading deck is never dead.
        e.toggle_sync(DeckId::A);
        assert_eq!(e.sync_mode(DeckId::A), SyncMode::Master);
        assert_eq!(e.sync_master(), Some(DeckId::A));
        // Again: let go. The group ends with its only member.
        e.toggle_sync(DeckId::A);
        assert_eq!(e.sync_mode(DeckId::A), SyncMode::Off);
        assert_eq!(e.sync_master(), None);
        // EXT is its own toggle, not a hidden third press.
        e.toggle_ext_sync(DeckId::A);
        assert_eq!(e.sync_mode(DeckId::A), SyncMode::External);
        assert!(e.any_external_sync(), "the detector must stay awake for this");
        e.toggle_ext_sync(DeckId::A);
        assert_eq!(e.sync_mode(DeckId::A), SyncMode::Off);
        assert!(!e.any_external_sync());
        // A plain SYNC press while EXT also releases it (the control is
        // one surface).
        e.toggle_ext_sync(DeckId::A);
        e.toggle_sync(DeckId::A);
        assert_eq!(e.sync_mode(DeckId::A), SyncMode::Off);
    }

    /// The lock is a LOCK: a follower drifting against the master (an
    /// analysed grid is never exactly the record) is pulled back with a
    /// bounded rate trim, never a seek — the correction the EXT path has
    /// always had, now held deck-to-deck.
    #[test]
    fn hold_deck_sync_trims_the_rate_and_never_seeks_a_drifting_follower() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 120.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::A, 32.0, true);
        engine.observe(DeckId::B, 16.0, true);
        engine.apply_auto_sync();
        assert_eq!(engine.sync_master(), Some(DeckId::A));
        assert!(engine.hold_deck_sync().is_empty(), "in phase, nothing to do");

        // B creeps 0.02 beats ahead (10 ms at 120 BPM): inaudible, exactly
        // the drift a wrong-by-a-hair BPM produces over a phrase.
        let ahead = engine.deck(DeckId::B).position_secs + 0.01;
        engine.observe(DeckId::A, 32.0, true);
        engine.observe(DeckId::B, ahead, true);
        let cmds = engine.hold_deck_sync();
        let rate = rate_of(&cmds, DeckId::B).expect("a trim, not silence: {cmds:?}");
        assert!(
            rate < 1.0 && rate > 1.0 - EXT_PHASE_TRIM - 1e-9,
            "an ahead deck is slowed within the trim bound, got {rate}"
        );
        assert!(
            seek_of(&cmds, DeckId::B).is_none(),
            "drift is trimmed, never seeked: {cmds:?}"
        );
        assert!(
            !cmds.iter().any(|cmd| matches!(
                cmd,
                DeckCmd::SetRate { deck: DeckId::A, .. }
                    | DeckCmd::SeekSeconds { deck: DeckId::A, .. }
            )),
            "the master is never touched"
        );
    }

    /// A paused master is a frozen phase: the pin hands over to the playing
    /// group member instead of dragging a live deck backwards toward it.
    #[test]
    fn a_stopped_master_hands_the_pin_to_the_playing_follower() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 126.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 130.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::A, 60.0, true);
        engine.observe(DeckId::B, 10.0, true);
        engine.apply_auto_sync();
        assert_eq!(engine.sync_master(), Some(DeckId::A));

        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 61.0, false);
        engine.hold_deck_sync();
        assert_eq!(
            engine.sync_master(),
            Some(DeckId::B),
            "the playing group member takes the pin"
        );
        // And the new master is not corrected by its own servo.
        assert!(engine.hold_deck_sync().is_empty());
    }

    /// A phase landing is placed where the lock will be true when the seek
    /// ARRIVES: with a landing latency declared, the follower leads the
    /// computed point by exactly rate × lookahead.
    #[test]
    fn a_sync_landing_leads_by_the_declared_lookahead() {
        let bare = {
            let mut engine = DeckEngine::new();
            load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.1);
            load_analysed(&mut engine, DeckId::B, 2, 124.0, 0.05);
            engine.play_pause(DeckId::A);
            engine.play_pause(DeckId::B);
            engine.observe(DeckId::A, 30.0, true);
            engine.observe(DeckId::B, 20.0, true);
            engine.apply_auto_sync();
            engine.deck(DeckId::B).position_secs
        };
        let mut engine = DeckEngine::new();
        engine.land_lookahead_secs = 0.02;
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.1);
        load_analysed(&mut engine, DeckId::B, 2, 124.0, 0.05);
        engine.play_pause(DeckId::A);
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::A, 30.0, true);
        engine.observe(DeckId::B, 20.0, true);
        engine.apply_auto_sync();
        let landed = engine.deck(DeckId::B).position_secs;
        let rate = engine.deck(DeckId::B).rate;
        assert!(
            (landed - (bare + rate * 0.02)).abs() < 1e-9,
            "landed {landed}, uncompensated {bare}, rate {rate}"
        );
    }

    /// The press ladder with both decks lit: the follower's press locks it,
    /// the leader's press claims the master role instead of doing nothing.
    /// (Auto sync off: the standing auto lock would have joined B already,
    /// making the first press a release — the manual ladder is what is
    /// under test.)
    #[test]
    fn sync_on_the_leading_deck_claims_master_instead_of_dying() {
        let mut engine = DeckEngine::new();
        engine.set_auto_sync(false);
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 100.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 12.0, true);

        // B joins: locks to A, which becomes the (unclaimed) master.
        engine.toggle_sync(DeckId::B);
        assert_eq!(engine.sync_mode(DeckId::B), SyncMode::Deck);
        assert_eq!(engine.sync_master(), Some(DeckId::A));
        assert_eq!(engine.sync_mode(DeckId::A), SyncMode::Off, "pinned, not yet claimed");

        // A's press claims the role — the old dead button.
        engine.toggle_sync(DeckId::A);
        assert_eq!(engine.sync_mode(DeckId::A), SyncMode::Master);

        // A lets go: the pin hands to B, which keeps its tempo untouched.
        let rate_before = engine.deck(DeckId::B).rate;
        engine.toggle_sync(DeckId::A);
        assert_eq!(engine.sync_master(), Some(DeckId::B));
        assert_eq!(engine.sync_mode(DeckId::B), SyncMode::Master);
        assert_eq!(engine.deck(DeckId::B).rate, rate_before);

        // B off: the group is gone.
        engine.toggle_sync(DeckId::B);
        assert_eq!(engine.sync_master(), None);
        assert!(engine.deck(DeckId::B).auto_opt_out, "off means off");
        assert!(engine.apply_auto_sync().is_empty());
    }

    #[test]
    fn an_ext_deck_follows_a_walking_room_without_jerking() {
        let mut e = DeckEngine::new();
        let (deck, gen) = load_gen(&e.click(item(1), DeckTarget::A));
        e.track_ready(deck, gen, 300.0);
        e.deck_mut(DeckId::A).grid = Some(grid(126.0, 0.0));
        e.play_pause(DeckId::A);
        e.set_ext_sync(DeckId::A, true);
        // The room walks from 128 to 130 over a minute; the deck's rate must
        // walk with it and never step.
        let mut previous = e.deck(DeckId::A).rate;
        let mut position = 0.0;
        for step in 0..120 {
            let bpm = 128.0 + 2.0 * step as f64 / 120.0;
            position += 0.5 * bpm / 60.0;
            e.observe(DeckId::A, position * 60.0 / 126.0, true);
            e.follow_external(&external(bpm, position));
            let rate = e.deck(DeckId::A).rate;
            assert!((rate - previous).abs() < 0.05, "step {step}: {previous} -> {rate}");
            previous = rate;
        }
        let rate = e.deck(DeckId::A).rate;
        assert!((rate - 130.0 / 126.0).abs() < 0.03, "{rate}");
    }
    #[test]
    fn eject_frees_the_deck_and_emits_an_unload() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 120.0, true);
        let gen_before = engine.deck(DeckId::A).load_gen;
        let cmds = engine.eject(DeckId::A);
        assert_eq!(cmds, vec![DeckCmd::UnloadTrack { deck: DeckId::A }]);
        let state = engine.deck(DeckId::A);
        assert!(matches!(state.load, DeckLoad::Empty));
        // The generation retires with the track: analysis or stems still in
        // flight for the old load must fail every host gen guard.
        assert_ne!(state.load_gen, gen_before);
        assert!(!state.playing);
        assert!(state.grid.is_none());
        assert!((state.duration_secs - 0.0).abs() < 1e-9);
        assert!((state.position_secs - 0.0).abs() < 1e-9);
        // A load in flight is never ejected: latest-wins holds.
        engine.click(item(2), DeckTarget::A);
        assert!(engine.eject(DeckId::A).is_empty());
        assert!(matches!(engine.deck(DeckId::A).load, DeckLoad::Loading { .. }));
    }

    #[test]
    fn an_ejected_deck_takes_the_queue_where_a_played_out_one_never_did() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 10.0, true);
        load_analysed(&mut engine, DeckId::B, 2, 128.0, 0.0);
        engine.play_pause(DeckId::B);
        engine.enqueue(item(3));
        assert_eq!(engine.queue().len(), 1, "both decks busy: the queue waits");
        engine.observe(DeckId::B, 300.0, false);
        engine.track_ended(DeckId::B);
        assert_eq!(engine.queue().len(), 1, "a Loaded deck still blocks the queue");
        engine.eject(DeckId::B);
        let cmds = engine.pump_queue();
        let (deck, _) = load_gen(&cmds);
        assert_eq!(deck, DeckId::B);
        assert!(engine.queue().is_empty());
    }

    #[test]
    fn requeue_goes_to_the_tail_and_never_pumps() {
        let mut engine = DeckEngine::new();
        // Both decks live so nothing can auto-load.
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        engine.play_pause(DeckId::A);
        load_analysed(&mut engine, DeckId::B, 2, 128.0, 0.0);
        engine.play_pause(DeckId::B);
        engine.enqueue(item(3));
        engine.requeue(item(4));
        assert_eq!(engine.queue().len(), 2);
        assert_eq!(engine.queue()[1].title, "track 4", "requeue appends at the tail");
        // Dedupe: a track already queued is not doubled.
        engine.requeue(item(3));
        assert_eq!(engine.queue().len(), 2);
    }

    #[test]
    fn shuffle_draws_deterministically_and_spares_the_requeued_track() {
        let mut engine = DeckEngine::new();
        engine.shuffle = true;
        engine.seed_shuffle(7);
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 10.0, true);
        engine.auto_load_queue = false;
        engine.enqueue(item(2));
        engine.enqueue(item(3));
        engine.requeue(item(4));
        assert_eq!(engine.queue().len(), 3);
        let cmds = engine.pump_queue();
        let loaded = match &cmds[0] {
            DeckCmd::LoadTrack { item, .. } => item.title.clone(),
            other => panic!("expected a load, got {other:?}"),
        };
        assert_ne!(loaded, "track 4", "the just-requeued track never jumps the queue");
        // Same seed, same queue -> same draw: determinism the tests can pin.
        let mut again = DeckEngine::new();
        again.shuffle = true;
        again.seed_shuffle(7);
        load_analysed(&mut again, DeckId::A, 1, 128.0, 0.0);
        again.play_pause(DeckId::A);
        again.observe(DeckId::A, 10.0, true);
        again.auto_load_queue = false;
        again.enqueue(item(2));
        again.enqueue(item(3));
        again.requeue(item(4));
        let cmds = again.pump_queue();
        let loaded_again = match &cmds[0] {
            DeckCmd::LoadTrack { item, .. } => item.title.clone(),
            other => panic!("expected a load, got {other:?}"),
        };
        assert_eq!(loaded, loaded_again);
    }

    #[test]
    fn a_deduped_requeue_still_spares_the_track_from_the_next_draw() {
        // The operator re-queued the playing track mid-play; when the
        // hand-back requeues it the push dedupes — but the spare must still
        // name it, or the shuffle draw can replay the track that just
        // finished.
        let mut engine = DeckEngine::new();
        engine.shuffle = true;
        engine.seed_shuffle(1);
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 10.0, true);
        engine.auto_load_queue = false;
        engine.enqueue(item(3));
        engine.enqueue(item(4));
        engine.requeue(item(3)); // dedupes, but must still set the spare
        assert_eq!(engine.queue().len(), 2);
        let cmds = engine.pump_queue();
        let loaded = match &cmds[0] {
            DeckCmd::LoadTrack { item, .. } => item.title.clone(),
            other => panic!("expected a load, got {other:?}"),
        };
        assert_eq!(loaded, "track 4", "the deduped requeue is spared the draw");
    }

    #[test]
    fn play_is_idempotent_and_needs_a_loaded_track() {
        let mut engine = DeckEngine::new();
        assert!(engine.play(DeckId::A).is_empty(), "empty deck: nothing to start");
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        let cmds = engine.play(DeckId::A);
        assert!(cmds.contains(&DeckCmd::SetPlaying { deck: DeckId::A, playing: true }));
        engine.observe(DeckId::A, 1.0, true);
        assert!(engine.play(DeckId::A).is_empty(), "already playing: a start, not a toggle");
    }

    #[test]
    fn cue_deck_seeks_without_relocking_the_phase() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 126.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 30.0, true);
        load_analysed(&mut engine, DeckId::B, 2, 130.0, 0.0);
        let cmds = engine.cue_deck(DeckId::B, 42.0);
        assert_eq!(cmds, vec![DeckCmd::SeekSeconds { deck: DeckId::B, secs: 42.0 }]);
        assert!((engine.deck(DeckId::B).position_secs - 42.0).abs() < 1e-9);
        // Clamped into the track, and inert on an unloaded deck.
        let cmds = engine.cue_deck(DeckId::B, 1e6);
        assert_eq!(cmds, vec![DeckCmd::SeekSeconds { deck: DeckId::B, secs: 300.0 }]);
        engine.eject(DeckId::B);
        assert!(engine.cue_deck(DeckId::B, 10.0).is_empty());
    }

    #[test]
    fn a_pinned_master_survives_the_fader_and_the_hold_guards_the_servo() {
        let mut engine = DeckEngine::new();
        // A starts alone and the first lock pins it as master.
        load_analysed(&mut engine, DeckId::A, 1, 126.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 60.0, true);
        load_analysed(&mut engine, DeckId::B, 2, 130.0, 0.0);
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::B, 10.0, true);
        engine.apply_auto_sync();
        assert_eq!(engine.sync_master(), Some(DeckId::A));

        // The fader crossing to B used to flip leadership and yank A.
        // Pinned, the master stands and corrections keep their direction.
        engine.set_crossfader(1.0);
        assert_eq!(engine.sync_leader(), Some(DeckId::A));
        let cmds = engine.apply_auto_sync();
        assert!(
            !cmds.iter().any(|cmd| matches!(
                cmd,
                DeckCmd::SeekSeconds { deck: DeckId::A, .. }
                    | DeckCmd::SetRate { deck: DeckId::A, .. }
            )),
            "the master is never the one corrected: {cmds:?}"
        );

        // An autopilot fade retiring B holds the servo off it even after
        // its playhead is dragged out of phase.
        engine.begin_auto_fade(DeckId::B);
        engine.observe(DeckId::B, 17.3, true);
        assert!(
            engine.hold_deck_sync().is_empty(),
            "the held outgoing deck is never corrected mid-fade"
        );
        engine.end_auto_fade();
        assert!(
            !engine.hold_deck_sync().is_empty(),
            "released, the standing lock takes over again"
        );
    }

    #[test]
    fn found_loops_install_capped_recall_like_blues_and_die_with_the_track() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // 300 s
        let spans: Vec<LoopSpan> = (0..20)
            .map(|i| LoopSpan { start_secs: i as f64 * 10.0, end_secs: i as f64 * 10.0 + 8.0 })
            .collect();
        e.install_found_loops(DeckId::A, spans);
        assert_eq!(e.deck(DeckId::A).found_loops.len(), FOUND_LOOP_CAP, "capped");
        // Recall engages exactly like a blue chip, seeking to IN.
        e.observe(DeckId::A, 55.0, true);
        let cmds = e.recall_found(DeckId::A, 0);
        let span = e.deck(DeckId::A).loop_span.expect("engaged");
        assert!((span.start_secs - 0.0).abs() < 1e-9 && (span.end_secs - 8.0).abs() < 1e-9);
        assert_eq!(seek_of(&cmds, DeckId::A), Some(0.0));
        // A second click on the RUNNING found loop exits, like RELOOP/EXIT.
        let cmds = e.recall_found(DeckId::A, 0);
        assert!(!e.deck(DeckId::A).loop_on());
        assert!(cmds.contains(&DeckCmd::SetLoopSpan { deck: DeckId::A, span: None }));
        // Deletes: out of range is inert, in range removes only the memory.
        e.recall_found(DeckId::A, 1);
        e.delete_found(DeckId::A, 99);
        assert_eq!(e.deck(DeckId::A).found_loops.len(), FOUND_LOOP_CAP);
        e.delete_found(DeckId::A, 0);
        assert_eq!(e.deck(DeckId::A).found_loops.len(), FOUND_LOOP_CAP - 1);
        assert!(e.deck(DeckId::A).loop_on(), "the sound is untouched");
        // A fresh install clears them with the other marks.
        load_analysed(&mut e, DeckId::A, 2, 120.0, 0.0);
        assert!(e.deck(DeckId::A).found_loops.is_empty());
    }

    #[test]
    fn a_replacing_install_swaps_the_whole_found_set() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.install_found_loops(DeckId::A, vec![LoopSpan { start_secs: 5.0, end_secs: 9.0 }]);
        e.install_found_loops(DeckId::A, vec![LoopSpan { start_secs: 20.0, end_secs: 28.0 }]);
        let found = &e.deck(DeckId::A).found_loops;
        assert_eq!(found.len(), 1);
        assert!((found[0].start_secs - 20.0).abs() < 1e-9, "each scan replaces");
    }

    #[test]
    fn a_zero_length_found_span_seeks_without_engaging_a_loop() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        // Install a zero-length span (bookmark-style): start == end at 42.5 secs.
        e.install_found_loops(DeckId::A, vec![LoopSpan { start_secs: 42.5, end_secs: 42.5 }]);
        e.observe(DeckId::A, 10.0, true);
        let cmds = e.recall_found(DeckId::A, 0);
        // Should seek to the point, not engage a loop.
        assert_eq!(seek_of(&cmds, DeckId::A), Some(42.5), "zero-length spans seek only");
        assert!(!e.deck(DeckId::A).loop_on(), "no loop engaged on a point");
        assert_eq!(e.deck(DeckId::A).loop_span, None);
    }

}
