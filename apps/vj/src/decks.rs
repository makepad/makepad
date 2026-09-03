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
use crate::wave_analysis::{SoundSpan, TrackGrid};
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

/// What a load does when the deck it is aimed at is already playing.
///
/// Refuse is the default, and it is the only one of the three that is a
/// safety rather than a taste: a track the room is dancing to is the one
/// thing on this console that must not change because a finger landed on a
/// list. The other two are for operators who mean it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OverPlaying {
    /// Nothing loads and the deck is left alone.
    #[default]
    Refuse,
    /// The load happens; the deck ends up stopped at the new track's top.
    Stop,
    /// The deck never stops — the new track comes in where the old one
    /// went out.
    Keep,
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

/// Below this a record has stopped being in the room, for the purpose of
/// deciding which deck the group follows. A policy number, about -40 dB:
/// a record still on its way down a fader is audible; one muted, faded
/// off, or crossed all the way out is not.
///
/// Compared with a bare `>` and never against an absolute value: the
/// equal-power curve at the far end gives cos(PI/2), which in f32 is a
/// tiny NEGATIVE number, and an abs() test would call that silent for the
/// wrong reason.
pub const AUDIBLE_FLOOR: f32 = 0.01;

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

/// What a fresh load puts back to nothing.
///
/// Every switch is off by default, which is exactly what the tab did
/// before this existed: the channel strip an operator has set stands
/// across a load, because on most decks that is the point -- the EQ and
/// the trim describe the ROOM, not the record.
///
/// The groups are separable because they answer to different hands. Speed
/// and key are tempo and pitch intent; the EQ, the filter and the gain are
/// the console; the stems are the lane knobs. Note what is NOT here: a
/// tempo the LOCK worked out is dropped unconditionally, because a match
/// to a departed track is never a preference somebody chose.
///
/// Deliberately untouched by any of these, and staying that way: the pitch
/// RANGE, keylock, the armed loop count and the stem mode. Those describe
/// how the operator likes to work, not what was on the last record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoadReset {
    pub speed: bool,
    pub key: bool,
    pub eq: bool,
    pub filter: bool,
    pub gain: bool,
    pub stems: bool,
}

/// What a press of a deck's retire button did.
///
/// The button is one control with two meanings, told apart by how soon the
/// second press lands: the first clears the deck, a second inside
/// `EJECT_UNDO_MS` puts the track back. Everything the press REFUSED to do
/// is a value here too, because the console paints the refusal rather than
/// printing it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EjectPress {
    /// Nothing to clear and nothing to put back.
    Nothing,
    /// Refused on purpose: the deck is playing, or a load is in flight.
    Busy,
    Ejected,
    /// The undo landed and this track is on its way back.
    Restored { title: String },
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

/// How far the tempo fader reaches: one rung of `PITCH_RANGES`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PitchRange {
    rung: u8,
}

impl Default for PitchRange {
    fn default() -> PitchRange {
        PitchRange { rung: PITCH_RANGE_DEFAULT }
    }
}

/// How far the tempo fader reaches, rung by rung. Two was never enough:
/// a beatmatch wants a few percent under the hand, a mash-up wants half
/// the record.
pub const PITCH_RANGES: [f64; 8] = [0.04, 0.06, 0.08, 0.10, 0.16, 0.24, 0.50, 0.90];
const PITCH_RANGE_LABELS: [&str; 8] =
    ["±4%", "±6%", "±8%", "±10%", "±16%", "±24%", "±50%", "±90%"];
/// The everyday rung, and what the tab has always opened on.
const PITCH_RANGE_DEFAULT: u8 = 2;

impl PitchRange {
    pub fn fraction(self) -> f64 {
        PITCH_RANGES[self.rung as usize]
    }

    pub fn label(self) -> &'static str {
        PITCH_RANGE_LABELS[self.rung as usize]
    }

    /// One rung wider or narrower. It SATURATES rather than wrapping: a
    /// ladder that rolls from ±90% round to ±4% under a running mix is a
    /// trap, not a convenience.
    pub fn stepped(self, wider: bool) -> PitchRange {
        let last = (PITCH_RANGES.len() - 1) as u8;
        let rung = if wider {
            (self.rung + 1).min(last)
        } else {
            self.rung.saturating_sub(1)
        };
        PitchRange { rung }
    }
}

/// Widest tempo ratio a sync will ever ask for before it tries half/double
/// time instead.
const SYNC_RATE_MIN: f64 = 0.80;
const SYNC_RATE_MAX: f64 = 1.25;
/// Hard clamp on any rate the engine emits.
/// How far a held bend pushes the tempo, as a fraction of the track's own.
/// Four percent is a shove that lands a bar inside a beat; two is the
/// correction you make while listening to whether it worked.
pub const BEND_COARSE: f64 = 0.04;
pub const BEND_FINE: f64 = 0.02;

/// How far one trim press moves the tempo, as a fraction of the track's
/// own. Absolute, NOT a share of the selected range: a step that means
/// half a percent on one range and a twentieth of that on another is a
/// button nobody can learn.
pub const TRIM_COARSE: f64 = 0.005;
pub const TRIM_FINE: f64 = 0.0005;

pub const RATE_MIN: f64 = 0.25;
pub const RATE_MAX: f64 = 4.0;
/// How far the key can be shifted, in semitones either way. An octave: past
/// that a mix has left the track behind anyway.
pub const KEY_SHIFT_MAX: f64 = 12.0;

/// How close to the mark counts as being ON it. A hair over one frame at
/// 48k, so a return that lands sample-exact reads as parked while a
/// deliberate scrub of a millisecond does not.
const CUE_AT_MARK_SECS: f64 = 0.001;

/// What the CUE lamp is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CueLed {
    Dark,
    Solid,
    Blink,
}

/// Which key the lock holds, and what letting go does with it.
///
/// Three flat states rather than two switches: `Original` captures
/// nothing, so it has only one way to be released, and a fourth state
/// would show the operator two that behave identically.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeylockMode {
    /// The track's own key, whatever the tempo is doing. What the tab has
    /// always done.
    #[default]
    Original,
    /// The key that was already sounding when the lock went on, given back
    /// when it comes off.
    Current,
    /// The same anchor, but the borrowed semitones stay in the shift knob
    /// afterwards, as an interval the operator now owns.
    CurrentKept,
}

impl KeylockMode {
    /// The word the button wears. It names the key the lock will hold, so
    /// the mode can be picked before anything is pressed.
    pub fn label(self) -> &'static str {
        match self {
            KeylockMode::Original => "KEY",
            KeylockMode::Current => "NOW",
            KeylockMode::CurrentKept => "NOW+",
        }
    }

    pub fn cycled(self) -> KeylockMode {
        match self {
            KeylockMode::Original => KeylockMode::Current,
            KeylockMode::Current => KeylockMode::CurrentKept,
            KeylockMode::CurrentKept => KeylockMode::Original,
        }
    }
}

/// A tempo ratio said as the interval it shifts the pitch by: double speed
/// is an octave up. The floor is belt-and-braces -- the rate is clamped to
/// RATE_MIN everywhere it is set, but a log of zero would poison the knob.
fn semitones_of_rate(rate: f64) -> f64 {
    12.0 * rate.max(f64::MIN_POSITIVE).log2()
}

/// What a loop mutation MEANS for the playhead.
///
/// The engine decides this from a position mirror the UI pump refreshes
/// twenty times a second; the mixer owns the real one, sample by sample.
/// So the engine says what it INTENDED and the mixer does it against the
/// truth, instead of being handed a bare span and left to guess.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LoopSeek {
    /// Leave the playhead alone. A span placed around where the record
    /// already is, or moved with the head riding along inside it.
    #[default]
    None,
    /// The span changed under a head that belonged to it. Fold the head by
    /// whole loop lengths so it keeps its PHASE.
    ///
    /// Only for a head that was inside the OLD span: a head sitting behind
    /// IN is playing its way in, deliberately and audibly, and folding it
    /// forward would teleport it over the run-up.
    Changed,
    /// The head is being sent to IN outright: a recall, a fresh engage.
    MovedOut,
}

/// Which mark a nudge is aimed at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NudgeTarget {
    Cue,
    /// The span that is sounding, moved whole.
    RunningLoop,
    /// One saved loop, by its own number.
    Slot(u16),
}

/// A motor gesture on the platter: the deck stops or starts like a record
/// rather than like a switch. Distinct from `ScratchMotion`, which always
/// means a POINTER — the phase-lock rules key on the hand.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpinMotion {
    /// Wind down to a stop, leaving the record where it stopped.
    Brake,
    /// Throw it backwards and let it fall to rest.
    SpinBack,
    /// Wind up to tempo from a standstill.
    SoftStart,
}

/// What the pointer is doing to a deck's waveform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScratchMotion {
    /// A hand landed on the record: brake to a stop.
    Grab,
    /// The finger is HERE on the record, in source seconds, travelling at
    /// this speed. A place rather than a speed, because a speed alone is
    /// open-loop: every clamp, every dropped frame and every coalesced
    /// event is drift the record never recovers. The speed rides along as
    /// the feed-forward, measured where the pointer timestamps are.
    Move { secs: f64, rate: f32 },
    /// Let go: spin back up to the deck's tempo.
    Release,
}

/// The band a hand-corrected tempo has to land in.
///
/// Wider than the analyser's own fence, deliberately: the detector has to
/// choose one octave and guesses conservatively, while a hand correcting
/// it has heard the record. Narrow enough that a mis-click cannot publish
/// a grid nothing downstream can rule with.
pub const EDIT_BPM_MIN: f64 = 40.0;
pub const EDIT_BPM_MAX: f64 = 300.0;

/// How many grid corrections a deck can take back. A run of one kind is
/// one entry, so this is deep enough for a whole track's worth of
/// deciding without being an unbounded history.
pub const GRID_UNDO_CAP: usize = 16;

/// What put a step on the grid's undo stack. A run of one kind collapses,
/// and a run of taps is one decision exactly as a run of doublings is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GridStep {
    Edit(GridEdit),
    Tap,
}

/// A correction a hand makes to a measured grid.
///
/// Three, because the analyser has three ways of being wrong and they are
/// independent: the rulings can be in the wrong PLACE, the wrong ruling
/// can be the ONE, and the whole thing can be at the wrong octave.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GridEdit {
    /// Move every ruling so the nearest one lands on the playhead. The
    /// tempo is untouched.
    Adjust,
    /// Make the ruling nearest the playhead the first beat of the bar.
    /// Nothing moves; only which one is counted as the one.
    Downbeat,
    /// Multiply the tempo, hinged on the playhead so the beat being
    /// listened to stays where it is.
    Scale(f64),
}

/// What a press of SYNC is asking for.
///
/// All four have always been in the arithmetic -- a plan carries a rate
/// and a landing, separately -- and the button could only ever ask for
/// both of them, latched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncVerb {
    /// Both halves, once, and then let go: the decks are matched and the
    /// deck is the operator's again.
    Match,
    /// Both halves, and keep correcting. What the button has always done.
    Lock,
    /// The tempo only. The phase is the operator's to place.
    Tempo,
    /// The phase only. The tempo is whatever the fader says.
    Phase,
}

impl SyncVerb {
    pub fn takes_tempo(self) -> bool {
        matches!(self, SyncVerb::Match | SyncVerb::Lock | SyncVerb::Tempo)
    }
    pub fn takes_phase(self) -> bool {
        matches!(self, SyncVerb::Match | SyncVerb::Lock | SyncVerb::Phase)
    }
    /// Whether the correction goes on after the press.
    pub fn latches(self) -> bool {
        matches!(self, SyncVerb::Lock)
    }
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
    /// Where this deck is asked to sit against the lock, in beats. Read
    /// off the FOLLOWER only -- a leader is the reference and has no
    /// offset from itself.
    pub offset_beats: f64,
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
/// How far a transition may stretch a record before the room hears it.
///
/// Ours to tune, and deliberately tighter than the engine's sync envelope:
/// that one has to survive whatever the operator lands on, while this one
/// is a choice about which records to put together at all. Recorded mixes
/// sit under a few percent the overwhelming majority of the time.
pub const TEMPO_GATE: f64 = 0.08;

/// How much cheaper it is to push a record up than to drag it down.
///
/// Slowing a record is heard first — the transients smear and it sags —
/// so the two directions are not worth the same and the meeting point
/// between two tempos is not their midpoint. Ours to measure.
const SPEEDING_UP_COSTS: f64 = 0.8;

/// The tempo ratio between two records, folded onto the same pulse.
///
/// Always at or above 1.0: it is the size of the gap, not its direction.
/// A record at twice the tempo of another is on the same pulse counted
/// twice, so it folds to 1.0 rather than reading as a doubling — without
/// this every drum record looks unmixable against every house record.
pub fn tempo_ratio(a_bpm: f64, b_bpm: f64) -> f64 {
    if !(a_bpm > 0.0) || !(b_bpm > 0.0) {
        return f64::INFINITY;
    }
    let mut ratio = b_bpm / a_bpm;
    while ratio > SYNC_RATE_MAX {
        ratio *= 0.5;
    }
    while ratio < SYNC_RATE_MIN {
        ratio *= 2.0;
    }
    if ratio < 1.0 {
        1.0 / ratio
    } else {
        ratio
    }
}

/// How well two tempos sit together: 1.0 on the same pulse, falling to
/// zero at the gate, and staying there beyond it.
pub fn tempo_fit(a_bpm: f64, b_bpm: f64) -> f32 {
    let stretch = tempo_ratio(a_bpm, b_bpm) - 1.0;
    if !stretch.is_finite() {
        return 0.0;
    }
    (1.0 - stretch / TEMPO_GATE).clamp(0.0, 1.0) as f32
}

/// The tempo two records should meet at, so neither carries the whole gap.
///
/// Splitting the difference in log tempo would be the even-handed answer if
/// both directions cost the same. They do not: the record being dragged
/// down gives up more than the one being pushed up, so the meeting point
/// leans towards the faster record and the slower one travels further.
pub fn meeting_tempo(a_bpm: f64, b_bpm: f64) -> f64 {
    if !(a_bpm > 0.0) || !(b_bpm > 0.0) {
        return a_bpm.max(b_bpm).max(0.0);
    }
    let (slow, fast) = if a_bpm <= b_bpm { (a_bpm, b_bpm) } else { (b_bpm, a_bpm) };
    // Equal cost either side: the slower record pays SPEEDING_UP_COSTS per
    // unit of log tempo, the faster one pays a full unit.
    let weight = SPEEDING_UP_COSTS + 1.0;
    ((slow.ln() * SPEEDING_UP_COSTS + fast.ln()) / weight).exp()
}

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
    // The follower's kept nudge shifts where "in phase" IS: a deck asked
    // to sit a quarter beat ahead lands a quarter beat ahead, and the
    // rounding is done about that point rather than about dead phase.
    let offset = match quantize {
        SyncQuantize::Beat => follower.offset_beats,
        SyncQuantize::Bar => follower.offset_beats * 0.25,
    };
    let leader_phase = leader_phase + offset;
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

/// How long a follower is walked back after the record it follows is let
/// go, in the FOLLOWER'S OWN beats -- half a bar of a leader running at
/// half tempo, and the deck being moved is the one whose beats matter.
///
/// A ceiling, not a schedule: the walk is over the moment the phase is
/// back inside what the standing servo holds. The arithmetic behind the
/// number: the trim is bounded at two percent, the worst error the servo
/// can report is half a beat, so the slowest possible close takes about
/// twelve and a half beats. Thirty-two is that with room, and it is what
/// stops a follower whose grid is wrong from refusing to land forever.
pub const RELAND_BEATS: f64 = 32.0;

/// Where a deck is in whatever it is counting, for the indicator beside
/// its tempo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeckPulse {
    /// Counts in the lap: four for a bar, the loop's length in beats for a
    /// running loop, one for a loop shorter than a beat.
    pub span_beats: u32,
    /// Which count was crossed last, from 0.
    pub index: u32,
    /// How far past it the record is, in [0, 1).
    pub phase: f64,
    /// How fast the record is travelling and which way, as a multiple of
    /// its own tempo. Carried unclamped: a spin-back is four times speed
    /// and a throw can be five, and an indicator counting at one would be
    /// wrong during exactly the gestures worth watching.
    pub travel: f32,
    /// Whether the beats being counted were measured. False means the
    /// synthetic grid is being counted -- a beat is a second -- and the
    /// indicator says so by burning lower rather than by lying.
    pub measured: bool,
}

/// A follower being walked back to the phase a released record left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reland {
    /// What is left of the ceiling, in this deck's own beats.
    pub beats_left: f64,
    /// Where the deck was at the last pump, so travel can be measured
    /// without a clock. Re-anchored rather than spent when the playhead
    /// moves somewhere it cannot describe.
    pub last_secs: f64,
}

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
    // Where the deck is ASKED to sit, not where dead phase is: a kept
    // nudge is the target, and the error is measured against it.
    let mut error =
        (external_beats - follower_beats + follower.offset_beats).rem_euclid(1.0);
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

/// One beat, in the count's own unit.
///
/// The count is carried in THIRTY-SECONDS of a beat so the ladder can
/// reach below one: a stutter is a fraction of a beat, and a `u32` of
/// whole beats has no way to say so. Thirty-seconds because it is the
/// finest rung anyone asks for and it leaves the sentinel below alone —
/// `u32::MAX` still sorts above the ladder's top of 16384.
pub const LOOP_TICKS_PER_BEAT: u32 = 32;

/// Every loop size the tab offers, shortest first: a thirty-second of a
/// beat up to five hundred and twelve beats, doubling at each rung.
///
/// THE one table. The count dropdown draws its rows from it and the `-`
/// and `+` stepper walks it, where the two used to be a list and a piece
/// of arithmetic that had to agree with each other by hand.
///
/// The bottom rungs are honest but not always reachable: `LOOP_MIN_SECS`
/// refuses a span shorter than a grain of the time stretcher, so at 120
/// BPM the ladder engages down to an eighth of a beat and no further.
/// That floor is the stretcher's and belongs to it, not to this table.
pub const LOOP_LADDER: [(u32, &str); 15] = [
    (1, "1/32"),
    (2, "1/16"),
    (4, "1/8"),
    (8, "1/4"),
    (16, "1/2"),
    (32, "1"),
    (64, "2"),
    (128, "4"),
    (256, "8"),
    (512, "16"),
    (1024, "32"),
    (2048, "64"),
    (4096, "128"),
    (8192, "256"),
    (16384, "512"),
];

/// Which rung a count is on, or None for MAN and the bookmark.
pub fn loop_rung_index(ticks: u32) -> Option<usize> {
    LOOP_LADDER.iter().position(|(rung, _)| *rung == ticks)
}

/// The rung nearest a length in beats, on a LOG scale.
///
/// Log, not arithmetic: three beats is equidistant from two and four by
/// subtraction, and four is the musical answer.
pub fn nearest_loop_rung(beats: f64) -> u32 {
    if !(beats > 0.0) || !beats.is_finite() {
        return LOOP_TICKS_PER_BEAT;
    }
    let ticks = (beats * LOOP_TICKS_PER_BEAT as f64).log2().round();
    let ticks = 2f64.powi(ticks.clamp(0.0, 31.0) as i32) as u32;
    ticks.clamp(LOOP_LADDER[0].0, LOOP_LADDER[LOOP_LADDER.len() - 1].0)
}

/// The armed count past the top rung: the BOOKMARK rung. `[` drops an in
/// point with no out — no loop, just a position saved as a marker.
pub const LOOP_BEATS_INF: u32 = u32::MAX;

/// The tempo a beat COUNT falls back to when nothing has measured the
/// track: one beat a second, so a four-beat loop is four seconds and a
/// sixteen-beat jump is sixteen.
///
/// A unit for counting, never a tempo the deck claims to have. It reaches
/// the loop lengths and the jumps and nothing else — snapping, syncing and
/// every tempo readout ask `true_grid`, which answers only for a grid
/// something actually measured.
pub const COUNTED_BPM: f64 = 60.0;

/// How many loops a track can keep as blue markers. Eight is more than a
/// set ever needs and small enough that the strip stays readable.
pub const LOOP_SLOT_CAP: usize = 8;

/// How many scanner-found loops a track keeps as yellow markers. Twice the
/// blue cap: a "find the 10 best" scan must fit with room to spare.
pub const FOUND_LOOP_CAP: usize = 16;

/// How soon a second press of a deck's retire button reads as the operator
/// taking the first one back rather than as a fresh press. Half a second is
/// a double-click and not a stray one.
pub const EJECT_UNDO_MS: u64 = 500;

/// How many retired tracks the undo can reach. Two deep, because that is
/// all the gesture can reach: the deck's own last track, and the one before
/// it if the last is already back on a deck.
pub const EJECT_HISTORY: usize = 2;

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

/// What a slot in the bank DOES when its number is pressed.
///
/// Stored rather than inferred. The bank has always told a point from a
/// span by measuring the span's length, which works until an operator
/// wants a mark that jumps a running loop somewhere rather than replacing
/// it -- two different answers for the same zero length.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SlotKind {
    /// Send the record to the mark. A cue point.
    #[default]
    Cue,
    /// Engage the span. A saved loop.
    Loop,
    /// Carry the RUNNING loop to the mark, or seek there when none is
    /// running -- the same choice a phrase jump already makes.
    Jump,
}

/// The colours the bank hands out, one per number, in order.
///
/// Assigned by NUMBER rather than by kind, so an operator learns "the
/// third pad is green" once and it stays true whatever they put on it.
/// Red, orange, amber, green, cyan, blue, violet, magenta -- eight hues
/// around the wheel, each far enough from its neighbours to be told apart
/// on a chip nine points wide.
pub const SLOT_PALETTE: [u32; LOOP_SLOT_CAP] = [
    0xff4d4dff,
    0xff8c3aff,
    0xf5c542ff,
    0x63c08aff,
    0x3ad0d0ff,
    0x3d8bffff,
    0x9b6bffff,
    0xff5cc8ff,
];

/// A saved loop and the NUMBER the operator addresses it by.
///
/// The number is data, not a position in a list: deleting one frees its
/// number and leaves every other mark where it was, instead of moving all
/// of them onto a different pad. The number lives here rather than on
/// `LoopSpan`, because that type is also the running span, the memory and
/// every scanner finding, and none of those has a number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopSlot {
    pub slot: u16,
    pub span: LoopSpan,
    /// What pressing this number does. See `SlotKind`.
    pub kind: SlotKind,
    /// Its colour, or 0 for "the palette answers for this number".
    pub colour: u32,
}

impl SlotKind {
    /// The kind this reads and writes as on disk.
    pub fn mark_kind(self) -> crate::marks::MarkKind {
        match self {
            SlotKind::Cue => crate::marks::MarkKind::HotCue,
            SlotKind::Loop => crate::marks::MarkKind::Loop,
            SlotKind::Jump => crate::marks::MarkKind::Jump,
        }
    }

    /// And back. Anything else is not a slot at all.
    pub fn from_mark_kind(kind: crate::marks::MarkKind) -> Option<SlotKind> {
        Some(match kind {
            crate::marks::MarkKind::HotCue => SlotKind::Cue,
            crate::marks::MarkKind::Loop => SlotKind::Loop,
            crate::marks::MarkKind::Jump => SlotKind::Jump,
            _ => return None,
        })
    }
}

impl LoopSlot {
    /// The colour this slot shows: its own if it has one, and the number's
    /// otherwise. A slot past the palette's end falls back to the first
    /// hue rather than to nothing.
    pub fn shown_colour(&self) -> u32 {
        if self.colour != 0 {
            return self.colour;
        }
        SLOT_PALETTE[self.slot as usize % SLOT_PALETTE.len()]
    }
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
    /// Armed loop length in THIRTY-SECONDS of a beat; 0 = MAN, free
    /// placement, and `LOOP_BEATS_INF` the bookmark rung. Anything else is
    /// a rung of `LOOP_LADDER`. This says what `[` and `]` will do NEXT.
    pub loop_ticks: u32,
    /// The loop that is actually running. A deck loops when it has a span;
    /// a bool beside one is a second truth that can disagree with the first.
    pub loop_span: Option<LoopSpan>,
    /// MAN: an IN point is placed and `]` is waiting to close it.
    pub loop_armed: Option<f64>,
    /// The last span, so RELOOP can get back into it after an exit.
    pub loop_memory: Option<LoopSpan>,
    /// Saved loops — the blue markers. Positions on THIS track, so a
    /// fresh install clears them with the span.
    /// Ascending by number, numbers unique and every one below the cap.
    /// Every mutator below is responsible for that; the loader is the one
    /// funnel a file comes in through, and it enforces it.
    pub loop_slots: Vec<LoopSlot>,
    /// Scanner-found loops — the yellow markers on the strip's bottom
    /// edge. Positions on THIS track; a fresh install clears them.
    pub found_loops: Vec<LoopSpan>,
    /// Where CUE sends the deck — the red marker. A position on THIS
    /// track, so a fresh install puts it back at the top.
    pub cue_secs: f64,
    /// Whether a hand put the mark above where it is, rather than a load
    /// leaving it at the value it defaults to. The analysis lands after the
    /// decode does, and "nobody has cued this track" is the only condition
    /// under which it may move the mark. One bool beside the number it
    /// qualifies, the way `rate_from_lock` sits beside the rate.
    pub cue_placed: bool,
    /// Where this record's intro and outro are: four edges, the outer two
    /// from the sound scan and the inner two from the loudness envelope.
    pub shape: Option<crate::track_shape::TrackShape>,
    /// Where the grid was before each correction, newest last, with a run
    /// of one kind of correction collapsed into a single step.
    ///
    /// Not a general undo: a run of presses on the same button is one
    /// decision -- four taps of DOUBLE is "this is at the wrong octave",
    /// not four things to take back one at a time.
    pub grid_undo: Vec<(GridStep, TrackGrid)>,
    /// This grid is right and nothing may change it: no correction, no
    /// flip, and not the analysis when it lands.
    pub grid_locked: bool,
    /// Whether a hand has corrected this record's grid. Same law as the
    /// mark and the shape beside it: the analysis lands after the marks
    /// file does, and it may only fill a grid nobody has placed.
    pub grid_placed: bool,
    /// Whether a HAND put them there. Same pair, same reason, as the mark
    /// beside it: the analysis lands after the marks file does, and it may
    /// only fill a shape nobody has placed.
    pub shape_placed: bool,
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
    /// The fitted moving tempo, when the analysis found one worth having.
    ///
    /// Shared rather than copied: it is a vector, it never changes once
    /// the analysis lands, and both the engine and the host read it.
    pub tempo_map: Option<std::sync::Arc<crate::wave_analysis::TempoMap>>,
    /// Being walked back to the phase a released record left, if so.
    ///
    /// While this stands the standing servo trims the rate as usual and
    /// its LANDING is suppressed: the phase closes at two percent instead
    /// of in one cut. Describes a pair of playheads, like the lock rate
    /// beside it, and is nonsense the moment either of them is moved on
    /// purpose.
    pub reland: Option<Reland>,
    /// Playback rate multiplier; 1.0 = the track's own tempo.
    pub rate: f64,
    /// What the PLATTER is doing, as a multiple of the track's own tempo:
    /// the rate above normally, the scratch ramp's own settled output
    /// while a hand or a motor owns the record, negative under a reverse
    /// hold and zero at the bottom of a brake.
    ///
    /// The third tempo, and the one rule that keeps the three apart: this
    /// is a READOUT and never a command. `rate` is what the fader and the
    /// lock asked for and is what every sync decision is made against;
    /// this is only what the record happens to be turning at while a
    /// gesture owns it, which is nobody else's tempo to match.
    pub platter_rate: f64,
    /// Whether the rate above was worked out by SYNC rather than put there
    /// by hand.
    ///
    /// The two are the same number and mean different things. A rate the
    /// operator set is a preference and may follow them onto the next
    /// track; a rate the lock computed describes a PAIR of tracks and is
    /// nonsense the moment one of them is replaced. Nothing could tell
    /// them apart before, so a load onto a synced follower kept a match to
    /// a track that had left the deck.
    pub rate_from_lock: bool,
    /// Operator pitch offset as a fraction (−0.08 = 8% slow).
    pub pitch: f64,
    /// Where this deck is asked to sit against the lock, in beats: 0 is
    /// dead phase, positive is ahead of it.
    ///
    /// A hand's nudge, kept. The servo used to pull a bent deck straight
    /// back to dead phase the moment the button came up, which threw away
    /// the one thing the gesture was for.
    pub phase_offset_beats: f64,
    /// A HELD bend, on top of whatever the fader says, in the same units.
    /// Deliberately not part of `pitch`: moving the fader opts a follower
    /// out of the lock, and nudging a deck back into place must not cost it
    /// its sync. Zero unless a bend button is down.
    pub bend: f64,
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
    /// CUE is down and previewing from the mark. The two sync servos leave
    /// a previewing deck alone, exactly as they leave a scratched one
    /// alone: what is sounding is an audition, not the mix.
    pub cue_held: bool,
    pub keylock: bool,
    /// Which key the lock holds. See `KeylockMode`.
    pub keylock_mode: KeylockMode,
    /// The semitones the lock folded into `key_shift` when it engaged.
    ///
    /// Recorded rather than recomputed on release: the tempo may have moved
    /// while the lock was holding, and giving back an interval worked out
    /// against the NEW tempo would leave the key somewhere nobody asked
    /// for. Zero whenever no lock is holding a borrowed interval.
    pub keylock_offset: f64,
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
    /// Which resonance rung the sweep is on; 0 is flat. Part of the
    /// channel strip like the filter beside it, so a swap carries it and
    /// a load leaves it, for the same reasons.
    pub resonance: usize,
    /// Which echo rung is on; 0 is off, otherwise an index into
    /// [`crate::music_dsp::ECHO_RUNGS`] plus one. Part of the channel
    /// strip, like resonance beside it.
    pub echo_rung: usize,
    /// Whether the echo's repeats land on the other channel.
    pub echo_pingpong: bool,
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
            loop_ticks: 4 * LOOP_TICKS_PER_BEAT,
            phase_flipped: false,
            loop_span: None,
            loop_armed: None,
            loop_memory: None,
            loop_slots: Vec::new(),
            found_loops: Vec::new(),
            cue_secs: 0.0,
            cue_placed: false,
            shape: None,
            shape_placed: false,
            grid_undo: Vec::new(),
            grid_locked: false,
            grid_placed: false,
            bookmark: None,
            muted: false,
            gain: 1.0,
            norm_gain: 1.0,
            duration_secs: 0.0,
            grid: None,
            splat: None,
            position_secs: 0.0,
            tempo_map: None,
            reland: None,
            rate: 1.0,
            platter_rate: 1.0,
            rate_from_lock: false,
            phase_offset_beats: 0.0,
            bend: 0.0,
            pitch: 0.0,
            pitch_range: PitchRange::default(),
            synced: false,
            ext_sync: false,
            auto_opt_out: false,
            cue_held: false,
            keylock: true,
            keylock_mode: KeylockMode::default(),
            keylock_offset: 0.0,
            key_shift: 0.0,
            scratching: false,
            eq: [1.0; 3],
            eq_kill: [false; 3],
            eq_solo: [false; 3],
            filter: 0.5,
            resonance: 0,
            echo_rung: 0,
            echo_pingpong: false,
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
    /// The lift the mixer is sent for this deck's resonance rung.
    pub fn resonance_lift(&self) -> f32 {
        let rungs = crate::music_dsp::DeckEq::RESONANCE_RUNGS;
        rungs[self.resonance.min(rungs.len() - 1)]
    }

    /// The fraction the mixer's echo is sent for this deck's rung, or
    /// none for off.
    pub fn echo_fraction(&self) -> Option<(u32, u32)> {
        (self.echo_rung > 0)
            .then(|| crate::music_dsp::ECHO_RUNGS.get(self.echo_rung - 1))
            .flatten()
            .copied()
    }

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

    /// Whether the span standing on this deck is the whole file.
    ///
    /// A track repeat and a loop are ONE span and one wrap: there is no
    /// second mechanism and no flag beside it, which is why the loop icon
    /// lights, the overview band draws, RELOOP works and `[`, `]` and the
    /// stepper all behave with nothing new plumbed. This only tells the two
    /// apart for the gesture that placed it.
    pub fn repeats_whole_track(&self) -> bool {
        self.loop_span.is_some_and(|span| {
            span.start_secs <= 0.0 && span.end_secs >= self.duration_secs - 1e-9
        })
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
    /// The grid something MEASURED, and only that.
    ///
    /// `grid` itself can hold a default: the analysis lands on every track,
    /// and one whose tempo fell outside the detector's window comes back
    /// with a grid that has no beats in it. Everything that snaps, syncs or
    /// shows a tempo asks this, so a counted beat can never leak into a
    /// place that would treat it as a fact.
    pub fn true_grid(&self) -> Option<TrackGrid> {
        self.grid.filter(|grid| grid.has_grid())
    }

    /// Whether this deck's beats are real ones.
    pub fn has_true_beats(&self) -> bool {
        self.true_grid().is_some()
    }

    /// How long one counted beat is here. A LENGTH, with no phase: a
    /// bare number cannot be handed to anything that would place a mark
    /// or land a lock on it.
    pub fn counted_beat_secs(&self) -> f64 {
        self.true_grid().map_or(60.0 / COUNTED_BPM, |grid| grid.beat_secs)
    }

    pub fn effective_bpm(&self) -> Option<f64> {
        self.local_grid().map(|grid| grid.effective_bpm(self.rate))
    }

    /// Where this deck is in its count, given a playhead and how the
    /// record is travelling.
    ///
    /// The position is an ARGUMENT and `self.position_secs` is deliberately
    /// never read here: that mirror is refreshed at the pump's cadence, and
    /// an indicator drawn from it would stutter a fifth of a second behind
    /// the record. The caller passes what the mixer just published.
    ///
    /// Pure and clock-free, like everything else in this module: the
    /// caller turns the answer into a time.
    pub fn pulse_at(&self, position_secs: f64, travel: f32) -> Option<DeckPulse> {
        if !self.is_loaded() || !position_secs.is_finite() {
            return None;
        }
        let beat_len = self.counted_beat_secs();
        if !(beat_len > 0.0) {
            return None;
        }
        let measured = self.has_true_beats();
        // A loop is the lap, and it counts from its own IN -- that is what
        // the wrap is modulo, so counting from the track's downbeat would
        // count something that is not sounding.
        let (span_beats, beats) = match self.loop_span.filter(|_| self.loop_on()) {
            Some(span) => {
                let len = span.len_secs();
                let beats = (len / beat_len).round();
                if !(beats >= 1.0) {
                    // Every rung below a beat rounds to no beats at all, so
                    // the whole lap is the count.
                    let lap = ((position_secs - span.start_secs) / len).rem_euclid(1.0);
                    return Some(Self::pulse(1, lap, travel, measured));
                }
                (beats as u32, (position_secs - span.start_secs) / beat_len)
            }
            None => {
                let beats = match self.true_grid() {
                    // `beat_at` answers 0.0 for a grid with no beats, so the
                    // synthetic count cannot go through it.
                    Some(grid) => grid.beat_at(position_secs) + grid.downbeat_phase as f64,
                    None => position_secs / beat_len,
                };
                (4, beats)
            }
        };
        Some(Self::pulse(span_beats, beats, travel, measured))
    }

    /// One count out of a continuous coordinate: which one was crossed
    /// last, and how far past it the record is.
    ///
    /// Backwards the record crosses each count from ABOVE, so the count
    /// just passed is the one ahead and the phase runs the other way --
    /// which is a ceiling rather than a floor, and lands exactly ON a
    /// count without stepping past it.
    fn pulse(span_beats: u32, beats: f64, travel: f32, measured: bool) -> DeckPulse {
        let span = span_beats.max(1);
        let edge = if travel < 0.0 { beats.ceil() } else { beats.floor() };
        let phase = (beats - edge).abs();
        DeckPulse {
            span_beats: span,
            index: (edge as i64).rem_euclid(span as i64) as u32,
            phase: phase.clamp(0.0, 1.0),
            travel,
            measured,
        }
    }

    /// The grid as it stands AT THE PLAYHEAD.
    ///
    /// On a record made by a machine this is the published line exactly --
    /// one straight line describes it to the millisecond and there is no
    /// map to consult, which is nearly every record and is why this is
    /// safe to put in front of the arithmetic. On a record played by
    /// people it is that line re-cut to the tempo around the playhead,
    /// hinged there so the beat the deck is on does not move.
    ///
    /// Used where a tempo has to be RIGHT NOW -- the lock, the readout,
    /// the ruled grid. Placement still goes through the published line:
    /// a mark belongs to the record, not to the moment it was dropped.
    pub fn local_grid(&self) -> Option<TrackGrid> {
        let grid = self.true_grid()?;
        let Some(map) = self.tempo_map.as_deref() else { return Some(grid) };
        match map.local_bpm(self.position_secs) {
            Some(bpm) => Some(grid.hinged_at(self.position_secs, bpm)),
            None => Some(grid),
        }
    }

    /// The tempo the record is turning at THIS instant, gesture and all.
    ///
    /// Third of three, and the three are three named accessors on one
    /// type: the grid's own `bpm` is the base, [`effective_bpm`] is that
    /// scaled by the fader and the lock, and this is what the platter is
    /// actually doing. Can be negative under a reverse hold and zero at
    /// the bottom of a brake, which is why it is a readout and not a
    /// number anything is asked to match.
    ///
    /// [`effective_bpm`]: Self::effective_bpm
    pub fn live_bpm(&self) -> Option<f64> {
        self.local_grid().map(|grid| grid.effective_bpm(self.platter_rate))
    }

    /// A view for the sync arithmetic.
    ///
    /// Carries the RATE-SCALED tempo and never the platter's. A hand on a
    /// record is not a tempo the other deck should be asked to match, and
    /// this is the line that keeps the published instantaneous number from
    /// quietly becoming a rate command.
    pub fn sync_view(&self) -> Option<SyncView> {
        let grid = self.local_grid()?;
        Some(SyncView {
            grid,
            position_secs: self.position_secs,
            rate: self.rate,
            offset_beats: self.phase_offset_beats,
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
    /// Install the decoded track on the mixer deck voice at zero.
    /// `keep_playing` asks the mixer to bring the deck straight back up on
    /// the new track instead of leaving it stopped.
    InstallTrack { deck: DeckId, keep_playing: bool },
    /// A report, not a command: the pick landed on a deck the policy
    /// protects and nothing was loaded. It has to be said out loud,
    /// because a click that does nothing is indistinguishable from a click
    /// that missed.
    LoadRefused { deck: DeckId },
    SetPlaying { deck: DeckId, playing: bool },
    SeekFraction { deck: DeckId, fraction: f64 },
    /// The deck's loop span in source seconds, or `None` to run free.
    SetLoopSpan { deck: DeckId, span: Option<LoopSpan>, seek: LoopSeek },
    SetMute { deck: DeckId, muted: bool },
    SetGain { deck: DeckId, gain: f32 },
    /// Jump the crossfader (mixer slews internally against clicks).
    SetCrossfader { position: f32 },
    /// Ramp the crossfader to `position` over `secs`.
    FadeCrossfader { position: f32, secs: f32 },
    SetCurve { curve: FadeCurve },
    /// Swap the two mixer deck voices (contents, transport, everything).
    SwapVoices,
    /// Put the record on `from` onto `to` as well, on the same sample.
    /// Distinct from a swap: nothing is exchanged and no fader moves.
    CloneDeck { from: DeckId, to: DeckId },
    /// Playback rate multiplier (tempo). Pitch-preserving when key lock is on.
    SetRate { deck: DeckId, rate: f64 },
    /// The record's beat grid as the engine holds it, or none: sent from
    /// EVERY path that writes one, so the callback's musical clock reads
    /// the grid the operator sees and never one the engine has replaced.
    SetGrid { deck: DeckId, grid: Option<TrackGrid> },
    /// Absolute playhead in source seconds.
    SeekSeconds { deck: DeckId, secs: f64 },
    /// Pointer on the waveform: vinyl-style rate override.
    Scratch { deck: DeckId, motion: ScratchMotion },
    /// The reverse hold: the record runs backwards while it is held, and a
    /// ghost keeps the place it should have reached.
    Censor { deck: DeckId, on: bool },
    /// Latch a ghost for a roll about to engage.
    RollPush { deck: DeckId },
    /// Let one level of roll go: the span to put back, and whether to keep
    /// what is sounding instead of returning to the ghost. ONE command,
    /// because either order of two would leave a buffer of wrong audio.
    RollPop { deck: DeckId, parent: Option<LoopSpan>, adopt: bool },
    /// Momentary FREEZE: repeat the beat (or the armed sub-beat rung) the
    /// deck just played, post-filter, while the record runs on
    /// underneath. Seconds are at the DEVICE -- already divided by the
    /// deck's rate -- so the mixer never has to ask the engine's rate
    /// again to size the lap.
    Freeze { deck: DeckId, secs: Option<f64> },
    Spin { deck: DeckId, motion: SpinMotion },
    /// Keep the key when the tempo changes (time stretch) or let it slide.
    SetKeylock { deck: DeckId, on: bool },
    /// Key shift in semitones: pitch WITHOUT tempo.
    SetKeyShift { deck: DeckId, semitones: f64 },
    /// One tone band, 0 = kill, 1 = unity.
    SetEqBand { deck: DeckId, band: usize, gain: f32 },
    /// Bipolar sweep filter; 0.5 = off.
    SetFilter { deck: DeckId, position: f32 },
    /// How hard the sweep rings, as the lift the mixer applies.
    SetResonance { deck: DeckId, lift: f32 },
    /// The echo's rung, or none for off.
    SetEcho { deck: DeckId, fraction: Option<(u32, u32)> },
    /// Whether the echo's repeats cross channels.
    SetEchoPingpong { deck: DeckId, on: bool },
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
    /// Write this record's marks down: it is leaving a deck.
    ///
    /// Carries the whole payload rather than a deck: by the time a
    /// command list runs, the deck it came from may already hold the
    /// record that displaced this one, and re-reading it would file one
    /// track's marks under another's name.
    RetireMarks {
        item: TrackItem,
        cue_secs: Option<f64>,
        bookmark: Option<f64>,
        slots: Vec<LoopSlot>,
        shape: Option<crate::track_shape::TrackShape>,
        grid: Option<TrackGrid>,
        grid_locked: bool,
    },
}

/// The unit a fresh deck starts on. NOT zero: the off row withholds the
/// phase landing as well as the rounding, and a tab that shipped with it
/// off would tempo-match two decks and never put them in step.
///
/// One beat, so the rounding it does impose is the smallest musical
/// amount there is -- and it is what turns a press on the overview strip
/// into a previewed landing rather than a live scrub.
pub const SNAP_DEFAULT_BEATS: u32 = 1;

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
    /// What a load does to a deck that is already playing. See
    /// `OverPlaying`; the host owns this the way it owns `auto_sync`.
    pub over_playing: OverPlaying,
    /// What a fresh load puts back to nothing. Off by default; see
    /// `LoadReset`.
    pub load_reset: LoadReset,
    /// QUANT's unit in beats, 0 = off. One global value: snapping is a
    /// property of how the operator is working, not of a deck.
    snap_beats: [u32; 2],
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
    /// The tracks the operator's retire button cleared, newest first,
    /// bounded by EJECT_HISTORY. This is the MEMORY and it lives as long as
    /// the app does; nothing expires it on a timer and nothing writes it to
    /// disk. An entry leaves when it is put back.
    ejected: Vec<TrackItem>,
    /// When each deck was last retired by hand. This is the GESTURE, and it
    /// is all that tells a second press from a first. A refusal arms
    /// nothing, and an undo spends the stamp so one eject buys one undo.
    last_eject_ms: [Option<u64>; 2],
    /// The span each level of roll displaced, newest last. The mixer keeps
    /// the ghosts; this keeps what to put back when one is let go.
    roll_parents: [Vec<Option<LoopSpan>>; 2],
    /// Which decks have a FREEZE held right now -- the engine's own
    /// mirror of the mixer's latch, so a second press refuses and the
    /// chip's lit state has something to read.
    frozen: [bool; 2],
    /// The platter is under a MOTOR gesture. A hand has a release to clear
    /// its hold on; a brake or a wind-up has no event at all — it simply
    /// lands — so the engine keeps this bit and clears it when the mixer
    /// reports the motor has handed the rate back.
    spin_running: [bool; 2],
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
            over_playing: OverPlaying::default(),
            load_reset: LoadReset::default(),
            snap_beats: [SNAP_DEFAULT_BEATS; 2],
            queue: Vec::new(),
            auto_load_queue: true,
            repeat: false,
            shuffle: false,
            shuffle_rng: 1,
            last_requeued: None,
            ejected: Vec::new(),
            last_eject_ms: [None; 2],
            frozen: [false; 2],
            spin_running: [false; 2],
            roll_parents: [Vec::new(), Vec::new()],
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

    /// Which deck a routing choice resolves to, or None when the choice is
    /// to load nothing at all.
    pub fn target_deck(&self, target: DeckTarget) -> Option<DeckId> {
        match target {
            DeckTarget::Off => None,
            DeckTarget::A => Some(DeckId::A),
            DeckTarget::B => Some(DeckId::B),
            DeckTarget::Auto | DeckTarget::Mix => Some(self.auto_target()),
        }
    }

    /// The deck a load would be refused on, under the standing policy.
    ///
    /// `auto_target` already steers Auto away from the single playing deck
    /// and `free_deck` already keeps the queue pump off one, so this only
    /// ever bites on an explicit A or B, a drop, a preview verdict, or Auto
    /// with BOTH decks running.
    pub fn load_refused(&self, target: DeckTarget) -> Option<DeckId> {
        if self.over_playing != OverPlaying::Refuse {
            return None;
        }
        let deck = self.target_deck(target)?;
        self.deck(deck).playing.then_some(deck)
    }

    /// Route a tile click. The chosen deck starts loading latest-wins; the
    /// other deck is untouched.
    pub fn click(&mut self, item: TrackItem, target: DeckTarget) -> Vec<DeckCmd> {
        // OFF is a hands-off list: nothing loads from a click, so a row can
        // be picked up and dragged where the operator wants it instead.
        if target == DeckTarget::Off {
            return Vec::new();
        }
        let Some(deck) = self.target_deck(target) else {
            return Vec::new();
        };
        // BEFORE the generation moves and before `last_loaded` does: a
        // refusal must spend nothing, or the next accepted load skips a
        // generation and the autoplay latch arms a deck nobody picked.
        if self.load_refused(target).is_some() {
            return vec![DeckCmd::LoadRefused { deck }];
        }
        // The record about to be displaced is owed its marks, and this is
        // the last moment they are still its own.
        let retire = self.retire_marks(deck);
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
        state.tempo_map = None;
        state.splat = None;
        state.position_secs = 0.0;
        state.synced = false;
        state.phase_offset_beats = 0.0;
        state.auto_opt_out = false;
        state.stems_ready = false;
        state.scratching = false;
        state.reland = None;
        // A held bend belongs to the track that was under the hand.
        state.bend = 0.0;
        // A deck with a load in flight cannot lead, and when its new grid
        // lands it must not become the reference the LIVE deck is dragged
        // to. Eject has always handed the pin over; loading over a track is
        // the same loss of the deck and never did.
        self.hand_pin_over(deck);
        let mut cmds: Vec<DeckCmd> = retire.into_iter().collect();
        cmds.push(DeckCmd::LoadTrack { deck, gen, item });
        cmds
    }

    /// Decode finished for `(deck, gen)`. Stale generations are dropped.
    pub fn track_ready(&mut self, deck: DeckId, gen: DeckGen, duration_secs: f64) -> Vec<DeckCmd> {
        let normalise = self.normalise;
        let over_playing = self.over_playing;
        let state = self.deck_mut(deck);
        let DeckLoad::Loading { gen: want, item } = state.load.clone() else {
            return Vec::new();
        };
        if want != gen {
            return Vec::new();
        }
        state.load = DeckLoad::Loaded { item };
        // A stopped deck has nothing to carry on, so Keep only means
        // anything when this deck was actually running when the bytes
        // landed. Everything else installs stopped, which is the honest
        // default for a load nobody promised would run on.
        let keep_playing = state.playing && over_playing == OverPlaying::Keep;
        state.playing = keep_playing;
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
        state.cue_placed = false;
        state.shape = None;
        state.shape_placed = false;
        state.grid_placed = false;
        state.grid_locked = false;
        state.grid_undo.clear();
        state.bookmark = None;
        // A rate the LOCK worked out matched this deck to the one on the
        // other side. That match described a pair of tracks and one of them
        // has just been replaced, so it goes back to the track's own tempo
        // rather than following a stranger onto a new record. A rate the
        // operator set by hand is a preference and stays.
        if state.rate_from_lock {
            state.rate = 1.0;
            state.pitch = 0.0;
            state.rate_from_lock = false;
        }
        // Then whatever the operator asked a load to clear. Done here, in
        // front of the command list below, so every SetX carries the reset
        // value and there is no second source of truth.
        let policy = self.load_reset;
        let state = self.deck_mut(deck);
        if policy.speed {
            state.rate = 1.0;
            state.pitch = 0.0;
            state.rate_from_lock = false;
        }
        if policy.key {
            state.key_shift = 0.0;
        }
        if policy.eq {
            state.eq = [1.0; 3];
            state.eq_kill = [false; 3];
            state.eq_solo = [false; 3];
        }
        if policy.filter {
            state.filter = 0.0;
        }
        if policy.gain {
            state.gain = 1.0;
        }
        if policy.stems {
            state.stem_gain = [1.0; STEM_COUNT];
            state.stem_kill = [false; STEM_COUNT];
            state.stem_solo = [false; STEM_COUNT];
        }
        // Fresh installs inherit the whole standing channel-strip intent:
        // transport, tone, stems and the rate the pitch slider is sitting at.
        let mut cmds = vec![
            DeckCmd::InstallTrack { deck, keep_playing },
            DeckCmd::SetLoopSpan { deck, span: None, seek: LoopSeek::None },
            DeckCmd::SetMute { deck, muted: state.muted },
            DeckCmd::SetGain { deck, gain: state.effective_gain(normalise) },
            DeckCmd::SetKeylock { deck, on: state.keylock },
            DeckCmd::SetRate { deck, rate: state.rate },
            DeckCmd::SetKeyShift { deck, semitones: state.key_shift },
            DeckCmd::SetFilter { deck, position: state.filter },
            DeckCmd::SetResonance { deck, lift: state.resonance_lift() },
            DeckCmd::SetEcho { deck, fraction: state.echo_fraction() },
            DeckCmd::SetEchoPingpong { deck, on: state.echo_pingpong },
        ];
        for band in 0..3 {
            cmds.push(DeckCmd::SetEqBand { deck, band, gain: state.eq_effective(band) });
        }
        for stem in 0..STEM_COUNT {
            cmds.push(DeckCmd::SetStemGain { deck, stem, gain: state.stem_effective(stem) });
        }
        // A frozen lap was measured against the OUTGOING record; the one
        // that just landed gets none, the way a load leaves everything
        // else here unbuilt rather than half-carried over.
        cmds.extend(self.freeze_release(deck));
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
        // A walk describes a pair of PLAYHEADS: stopping or starting one
        // of them ends it, and the re-lock below is the landing.
        state.reland = None;
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

    /// RELOOP / EXIT, the third loop button. Drop out of the running
    /// loop keeping it for later, or jump back into the last one. Inert
    /// with nothing to return to — the fall-out-and-slam-back-in move is
    /// the whole reason this is not just another way to spell "off".
    pub fn toggle_loop(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        if let Some(span) = state.loop_span.take() {
            state.loop_memory = Some(span);
            return vec![DeckCmd::SetLoopSpan { deck, span: None, seek: LoopSeek::None }];
        }
        let Some(span) = state.loop_memory else { return Vec::new() };
        state.loop_span = Some(span);
        vec![
            DeckCmd::SeekSeconds { deck, secs: span.start_secs },
            DeckCmd::SetLoopSpan { deck, span: Some(span), seek: LoopSeek::MovedOut },
        ]
    }

    /// Repeat the whole track, or stop repeating it.
    ///
    /// This is the ordinary loop span set to the whole file, so the mixer's
    /// wrap, its raw splice at the head (nothing exists before frame zero to
    /// crossfade with) and its rule that a span never ends a deck all apply
    /// unchanged. Nothing new is added to the audio path.
    ///
    /// Unlike every other way of engaging a span it leaves RELOOP's memory
    /// and any placed bookmark alone. An operator who has a loop saved at
    /// the drop and then repeats the track must still get that loop back
    /// when they press RELOOP -- a repeat is a transport choice, not a
    /// replacement for the loop they were keeping.
    pub fn repeat_track(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        if self.deck(deck).repeats_whole_track() {
            let state = self.deck_mut(deck);
            state.loop_span = None;
            return vec![DeckCmd::SetLoopSpan { deck, span: None, seek: LoopSeek::None }];
        }
        if !self.deck(deck).is_loaded() {
            return Vec::new();
        }
        let duration = self.deck(deck).duration_secs;
        // The same floor every other span passes: a file too short to hold
        // one is not something to repeat.
        let Some(span) = self.usable_span(deck, 0.0, duration) else {
            return Vec::new();
        };
        let state = self.deck_mut(deck);
        state.loop_span = Some(span);
        state.loop_armed = None;
        // No seek: the playhead is inside the whole file by definition, so
        // there is nowhere for a repeat to move the record to.
        vec![DeckCmd::SetLoopSpan { deck, span: Some(span), seek: LoopSeek::None }]
    }

    /// Give the sync pin to the other deck, if it can carry it.
    ///
    /// Called wherever a deck stops being able to lead: ejected, unsynced,
    /// or loading. The same election was written out three times before
    /// this and missing from the fourth place that needed it.
    fn hand_pin_over(&mut self, deck: DeckId) {
        if self.sync_master != Some(deck) {
            return;
        }
        let other = deck.other();
        let state = self.deck(other);
        self.sync_master =
            (state.synced && state.is_loaded() && state.sync_view().is_some()).then_some(other);
    }

    /// Seconds one armed loop is worth on this deck, or `None` when the
    /// count is MAN or the track has no grid to measure a beat against.
    fn armed_secs(&self, deck: DeckId) -> Option<f64> {
        let state = self.deck(deck);
        if state.loop_ticks == 0 || state.loop_ticks == LOOP_BEATS_INF {
            return None;
        }
        // A deck with no track has no length to measure against, and
        // `click` does not clear the duration -- without this a bracket
        // pressed during a load would size itself on the outgoing record.
        if !state.is_loaded() {
            return None;
        }
        // The one line where the ladder reaching below a beat happens.
        // Everything downstream of here is already seconds.
        Some(state.counted_beat_secs() * state.loop_ticks as f64
            / LOOP_TICKS_PER_BEAT as f64)
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
        let was = state.loop_span;
        state.loop_span = Some(span);
        state.loop_armed = None;
        state.bookmark = None;
        state.loop_memory = Some(span);
        let mut cmds = Vec::new();
        if seek && (position < span.start_secs || position >= span.end_secs) {
            cmds.push(DeckCmd::SeekSeconds { deck, secs: span.start_secs });
            cmds.push(DeckCmd::SetLoopSpan { deck, span: Some(span), seek: LoopSeek::MovedOut });
            return cmds;
        }
        // A RESIZE -- the same loop with a different length -- is the one
        // case where the head may need folding: it belonged to the span
        // that just changed. A span placed somewhere new is not.
        let resized = was.is_some_and(|old| {
            (old.start_secs - span.start_secs).abs() < 1e-6
                || (old.end_secs - span.end_secs).abs() < 1e-6
        });
        let seek = if resized { LoopSeek::Changed } else { LoopSeek::None };
        cmds.push(DeckCmd::SetLoopSpan { deck, span: Some(span), seek });
        cmds
    }

    /// `[` — set IN here. With a beat count armed the loop closes itself N
    /// beats later; in MAN it waits for `]`.
    pub fn loop_in(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let position = self.deck(deck).position_secs;
        if self.deck(deck).loop_ticks == LOOP_BEATS_INF {
            // The bookmark rung: `[` places the current bookmark — GREEN,
            // like a fresh loop, and clicking its chip is what saves it.
            // It replaces a running loop the way `[` always replaces.
            let state = self.deck_mut(deck);
            state.bookmark = Some(position);
            state.loop_armed = None;
            if state.loop_span.take().is_some() {
                return vec![DeckCmd::SetLoopSpan { deck, span: None, seek: LoopSeek::None }];
            }
            return Vec::new();
        }
        if let Some(len) = self.armed_secs(deck) {
            // A SUB-BEAT rung starts on the beat's own subdivision. A
            // stutter is a subdivision of the beat, and one that starts
            // between two of them arrives late every lap -- which is
            // audible in a way a whole-beat loop's offset is not, because
            // the ear has the beat itself to compare it against.
            let ticks = self.deck(deck).loop_ticks;
            let start = match self.deck(deck).true_grid() {
                Some(grid) if ticks < LOOP_TICKS_PER_BEAT && ticks > 0 => {
                    grid.snap_to_subdivision(position, LOOP_TICKS_PER_BEAT / ticks)
                }
                _ => position,
            };
            return match self.usable_span(deck, start, start + len) {
                Some(span) => self.engage_loop(deck, span, true),
                None => Vec::new(),
            };
        }
        // A beat count with no grid behind it has nothing honest to do —
        // and must not fall through into arming MAN.
        if self.deck(deck).loop_ticks != 0 {
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
        if self.deck(deck).loop_ticks != 0 {
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
        let beats = self.deck(deck).loop_ticks;
        // The bookmark rung's transitions come first: they change what the
        // current object IS, never its size, and the direct pick owns
        // that logic.
        let top = LOOP_LADDER[LOOP_LADDER.len() - 1].0;
        let bottom = LOOP_LADDER[0].0;
        if factor >= 1.0 && (beats == top || beats == LOOP_BEATS_INF) {
            return self.set_loop_beats(deck, LOOP_BEATS_INF);
        }
        if factor < 1.0 && beats == LOOP_BEATS_INF {
            return self.set_loop_beats(deck, top);
        }
        // Halving the shortest rung lands on MAN rather than sticking, and
        // doubling out of MAN has to special-case zero or it would stay.
        let next = match (factor < 1.0, beats) {
            (true, _) if beats <= bottom => 0,
            (true, _) => beats / 2,
            (false, 0) => bottom,
            (false, _) => (beats * 2).min(top),
        };
        let Some(span) = self.deck(deck).loop_span else {
            self.deck_mut(deck).loop_ticks = next;
            return Vec::new();
        };
        let end = span.start_secs + span.len_secs() * factor;
        let Some(resized) = self.usable_span(deck, span.start_secs, end) else {
            // Refused. The count holds too: it is supposed to describe what
            // the brackets will do, and a loop that would not fit is not it.
            return Vec::new();
        };
        let state = self.deck_mut(deck);
        state.loop_ticks = next;
        // A resized loop that IS a saved marker carries the marker along:
        // the blue chip keeps aiming at the same IN, and its stored
        // duration follows what the ear now hears.
        // The resize keeps the loop on its own NUMBER: only the span moves.
        if let Some(entry) = state.loop_slots.iter_mut().find(|entry| {
            (entry.span.start_secs - span.start_secs).abs() < 1e-6
                && (entry.span.end_secs - span.end_secs).abs() < 1e-6
        }) {
            entry.span = resized;
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
        let unit = self.snap_beats(deck);
        let start = match self.deck(deck).true_grid() {
            Some(grid) => grid.snap_translate(start_secs, span.start_secs, unit),
            None => start_secs,
        };
        self.place_loop(deck, start)
    }

    /// Put the running span's IN at exactly this second, keeping its
    /// length. Where `move_loop` lands after its snap, and where a nudge
    /// lands without one.
    fn place_loop(&mut self, deck: DeckId, start: f64) -> Vec<DeckCmd> {
        let Some(span) = self.deck(deck).loop_span else { return Vec::new() };
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
        // NOT `Changed`: the ride above keeps the head at the same OFFSET
        // inside the span, and a fold would keep it at the same PHASE.
        // Those agree only when the move happens to be a whole multiple of
        // the loop's length.
        cmds.push(DeckCmd::SetLoopSpan { deck, span: Some(moved), seek: LoopSeek::None });
        cmds
    }

    /// A direct pick from the count dropdown — and the one owner of the
    /// bookmark rung's conversions, which the `<` `>` dial delegates to.
    /// Picking the infinity count collapses a running loop to a bookmark
    /// at its IN; picking a length out of infinity grows the out point
    /// back and loop mode with it; picking a length over a running loop
    /// resizes it in place, marker following.
    pub fn set_loop_beats(&mut self, deck: DeckId, beats: u32) -> Vec<DeckCmd> {
        let current = self.deck(deck).loop_ticks;
        if beats == current {
            return Vec::new();
        }
        if beats == LOOP_BEATS_INF {
            self.deck_mut(deck).loop_ticks = LOOP_BEATS_INF;
            let state = self.deck_mut(deck);
            let Some(span) = state.loop_span.take() else { return Vec::new() };
            state.bookmark = Some(span.start_secs);
            state.loop_memory = Some(span);
            return vec![DeckCmd::SetLoopSpan { deck, span: None, seek: LoopSeek::None }];
        }
        self.deck_mut(deck).loop_ticks = beats;
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
        // The resize keeps the loop on its own NUMBER: only the span moves.
        if let Some(entry) = state.loop_slots.iter_mut().find(|entry| {
            (entry.span.start_secs - span.start_secs).abs() < 1e-6
                && (entry.span.end_secs - span.end_secs).abs() < 1e-6
        }) {
            entry.span = resized;
        }
        self.engage_loop(deck, resized, false)
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

    /// Put the operator's marks back: CANCEL's undo path, and the marks
    /// file on a track's install. Cap-respecting, because a snapshot taken
    /// before a restore-from-disk could carry more than the row holds.
    /// What a record leaving a deck is owed: its marks, written down.
    ///
    /// Only a LOADED deck has any. While a load is in flight the slots
    /// still belong to the record before it, and that one was already
    /// retired when this load was asked for.
    fn retire_marks(&self, deck: DeckId) -> Option<DeckCmd> {
        let state = self.deck(deck);
        let DeckLoad::Loaded { item } = &state.load else { return None };
        Some(DeckCmd::RetireMarks {
            item: item.clone(),
            cue_secs: state.cue_placed.then_some(state.cue_secs),
            bookmark: state.bookmark,
            slots: state.loop_slots.clone(),
            shape: state.shape_placed.then_some(state.shape).flatten(),
            grid: state.grid_placed.then_some(state.grid).flatten(),
            grid_locked: state.grid_locked,
        })
    }

    pub fn restore_marks(
        &mut self,
        deck: DeckId,
        mut slots: Vec<LoopSlot>,
        bookmark: Option<f64>,
    ) {
        // The one funnel a marks file comes in through, so the invariant is
        // enforced here: numbers inside the cap, unique, ascending. A
        // truncate was right only while the number WAS the position — with
        // numbers as data it would leave a taken number reachable and let a
        // later save hand out a duplicate.
        slots.retain(|entry| entry.slot < LOOP_SLOT_CAP as u16);
        slots.sort_by_key(|entry| entry.slot);
        slots.dedup_by_key(|entry| entry.slot);
        let state = self.deck_mut(deck);
        state.loop_slots = slots;
        state.bookmark = bookmark;
    }

    /// A cue read back off disk. Deliberately not `set_cue`: a mark a
    /// hand placed is exact, and the unit that happens to be on when the
    /// track is loaded again has no business rounding it -- which would
    /// destroy precisely the hair a nudge exists to put there.
    pub fn restore_cue(&mut self, deck: DeckId, secs: f64) {
        self.place_cue(deck, secs);
    }

    /// Dragging the red marker: move where CUE sends the deck. Under a
    /// QUANT unit the drag steps in whole units against the cue's own
    /// phase — the same law as dragging the loop band — and exact with
    /// QUANT off. Nothing sounds until the CUE button is pressed.
    pub fn set_cue(&mut self, deck: DeckId, secs: f64) {
        let unit = self.snap_beats(deck);
        let state = self.deck(deck);
        let target = match state.true_grid() {
            Some(grid) => grid.snap_translate(secs, state.cue_secs, unit),
            None => secs,
        };
        self.place_cue(deck, target);
    }

    /// Put the mark at exactly this second. Where a placement lands once
    /// the snap has had its say -- and where a NUDGE lands, which skips
    /// the snap entirely: the hair a grid cannot express is the whole
    /// reason that gesture exists.
    fn place_cue(&mut self, deck: DeckId, target: f64) {
        let duration = self.deck(deck).duration_secs;
        let state = self.deck_mut(deck);
        state.cue_secs = if duration > 0.0 {
            target.clamp(0.0, duration)
        } else {
            target.max(0.0)
        };
        state.cue_placed = true;
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
        if state.loop_slots.iter().any(|entry| same(&entry.span)) {
            return false;
        }
        // The lowest free NUMBER, so a save after a delete reclaims the
        // number that was freed rather than landing at the end. A full row
        // has no free number, which is the cap check for nothing.
        let Some(slot) = (0..LOOP_SLOT_CAP as u16)
            .find(|number| !state.loop_slots.iter().any(|entry| entry.slot == *number))
        else {
            return false;
        };
        // The kind is stamped HERE, once, rather than re-derived from the
        // length on every read: a bookmark is a point, anything else is a
        // loop, and a colour of zero means the number's own hue answers.
        let kind = if span.len_secs() < 1e-9 { SlotKind::Cue } else { SlotKind::Loop };
        let at = state.loop_slots.partition_point(|entry| entry.slot < slot);
        state.loop_slots.insert(at, LoopSlot { slot, span, kind, colour: 0 });
        true
    }

    /// Dragging a blue marker off its spot: forget that saved loop. The
    /// running span is untouched — this deletes the memory, not the sound.
    /// The analysis's answer for where the intro and outro are. Refused
    /// on a shape a hand has placed, exactly as the mark beside it is.
    pub fn shape_ready(&mut self, deck: DeckId, gen: DeckGen, shape: crate::track_shape::TrackShape) {
        let state = self.deck_mut(deck);
        if state.load_gen != gen || state.shape_placed {
            return;
        }
        state.shape = Some(shape);
    }

    /// Restore a shape a hand placed on this record before.
    pub fn restore_shape(&mut self, deck: DeckId, shape: crate::track_shape::TrackShape) {
        let state = self.deck_mut(deck);
        state.shape = Some(shape);
        state.shape_placed = true;
    }

    /// Move one edge of the shape by hand.
    ///
    /// Under QUANT it lands on the grid, the way a placed mark does; a
    /// nudge asks with `snap` false, because the hair a grid cannot
    /// express is the whole reason that gesture exists. Returns whether
    /// the edge moved -- an edit that would cross a neighbour is refused,
    /// so the four stay a shape.
    pub fn set_shape_edge(
        &mut self,
        deck: DeckId,
        edge: crate::track_shape::ShapeEdge,
        secs: f64,
        snap: bool,
    ) -> bool {
        let unit = self.snap_beats(deck);
        let state = self.deck(deck);
        let Some(shape) = state.shape else { return false };
        let duration = state.duration_secs;
        let target = match state.true_grid() {
            Some(grid) if snap => grid.snap_translate(secs, shape.edge(edge), unit),
            _ => secs,
        };
        let Some(moved) = shape.with_edge(edge, target, duration) else {
            return false;
        };
        let state = self.deck_mut(deck);
        state.shape = Some(moved);
        state.shape_placed = true;
        true
    }

    /// Move ONE end of the running loop, anchored on the other.
    ///
    /// A resize rather than a move: the far end stays exactly where it is,
    /// which is what makes an edge draggable at all -- moving the whole
    /// span would take the anchor with it. The loop keeps running
    /// throughout, because a resize emits no seek and stamps the command
    /// as a change, and the mixer folds a stranded head modulo the new
    /// length against the real playhead rather than the engine's mirror.
    ///
    /// Snapped under QUANT against the loop's OWN other end, so a dragged
    /// edge lands a whole number of units from its anchor.
    pub fn set_loop_edge(&mut self, deck: DeckId, out: bool, secs: f64) -> Vec<DeckCmd> {
        let Some(span) = self.deck(deck).loop_span else { return Vec::new() };
        if !secs.is_finite() {
            return Vec::new();
        }
        let unit = self.snap_beats(deck);
        let anchor = if out { span.start_secs } else { span.end_secs };
        let target = match self.deck(deck).true_grid() {
            Some(grid) => grid.snap_translate(secs, anchor, unit),
            None => secs,
        };
        let (start, end) = if out { (span.start_secs, target) } else { (target, span.end_secs) };
        // Refused rather than clamped, the way every other span edit is:
        // a clamp would move the ANCHOR, which is the one thing this
        // gesture promises to leave alone.
        let Some(resized) = self.usable_span(deck, start, end) else {
            return Vec::new();
        };
        self.engage_loop(deck, resized, false)
    }

    /// Put a mark on THIS number, at the playhead.
    ///
    /// The bank could only ever be written to by saving whatever was
    /// running onto the lowest free number. This is the addressed press:
    /// a number an operator chose, holding what they chose to put on it.
    /// Idempotent on the number -- pressing it again replaces what is
    /// there rather than refusing or landing somewhere else.
    ///
    /// Under QUANT the mark lands on the grid rather than beside it: the
    /// track's own first beat is the phase reference, which turns the
    /// tab's translation into the quantise-to-grid this one gesture wants.
    pub fn set_slot(&mut self, deck: DeckId, slot: u16, kind: SlotKind) -> bool {
        if slot >= LOOP_SLOT_CAP as u16 || !self.deck(deck).is_loaded() {
            return false;
        }
        let unit = self.snap_beats(deck);
        let state = self.deck(deck);
        let at = match state.true_grid() {
            Some(grid) if unit != 0 => {
                grid.snap_translate(state.position_secs, grid.first_beat_secs, unit)
            }
            _ => state.position_secs,
        };
        let span = match kind {
            SlotKind::Loop => {
                let Some(len) = self.armed_secs(deck) else { return false };
                let Some(span) = self.usable_span(deck, at, at + len) else { return false };
                span
            }
            _ => LoopSpan { start_secs: at, end_secs: at },
        };
        let state = self.deck_mut(deck);
        state.loop_slots.retain(|entry| entry.slot != slot);
        let index = state.loop_slots.partition_point(|entry| entry.slot < slot);
        state.loop_slots.insert(index, LoopSlot { slot, span, kind, colour: 0 });
        true
    }

    /// Give one number a colour of its own, or 0 to hand it back to the
    /// palette. Returns whether the number holds anything.
    pub fn set_slot_colour(&mut self, deck: DeckId, slot: u16, colour: u32) -> bool {
        match self.deck_mut(deck).loop_slots.iter_mut().find(|e| e.slot == slot) {
            Some(entry) => {
                entry.colour = colour;
                true
            }
            None => false,
        }
    }

    /// Move a mark by a hair, without the grid's say.
    ///
    /// The gesture exists for exactly the placements a beat cannot
    /// express: a cue a few milliseconds behind the transient, a loop
    /// whose IN sits a hair inside the kick. Snapping it would round the
    /// step away, so the nudge goes straight to the placers.
    pub fn nudge_mark(
        &mut self,
        deck: DeckId,
        target: NudgeTarget,
        delta_secs: f64,
    ) -> Vec<DeckCmd> {
        if !delta_secs.is_finite() || !self.deck(deck).is_loaded() {
            return Vec::new();
        }
        match target {
            NudgeTarget::Cue => {
                let at = self.deck(deck).cue_secs;
                self.place_cue(deck, at + delta_secs);
                Vec::new()
            }
            NudgeTarget::RunningLoop => {
                let Some(span) = self.deck(deck).loop_span else { return Vec::new() };
                self.place_loop(deck, span.start_secs + delta_secs)
            }
            NudgeTarget::Slot(slot) => {
                let Some(span) = self
                    .deck(deck)
                    .loop_slots
                    .iter()
                    .find(|entry| entry.slot == slot)
                    .map(|entry| entry.span)
                else {
                    return Vec::new();
                };
                let start = span.start_secs + delta_secs;
                // A POINT has no length to validate, and `usable_span`
                // refuses anything shorter than a loop may be -- so the
                // wheel over a cue chip used to do nothing at all. Clamp
                // it into the track and move on.
                let moved = if span.len_secs() < LOOP_MIN_SECS {
                    let duration = self.deck(deck).duration_secs;
                    let at = if duration > 0.0 {
                        start.clamp(0.0, duration)
                    } else {
                        start.max(0.0)
                    };
                    LoopSpan { start_secs: at, end_secs: at + span.len_secs() }
                } else {
                    // Refused rather than clamped, exactly as a
                    // hand-dragged loop is: a clamp would change the
                    // LENGTH as well as the place, which is not what was
                    // asked for.
                    let Some(moved) = self.usable_span(deck, start, start + span.len_secs())
                    else {
                        return Vec::new();
                    };
                    moved
                };
                // If this chip is the loop that is sounding, the sound
                // goes with it -- the chip and the loop must never part.
                let running = self.deck(deck).loop_span.filter(|live| {
                    (live.start_secs - span.start_secs).abs() < 1e-6
                        && (live.end_secs - span.end_secs).abs() < 1e-6
                });
                let state = self.deck_mut(deck);
                if let Some(entry) =
                    state.loop_slots.iter_mut().find(|entry| entry.slot == slot)
                {
                    entry.span = moved;
                }
                if running.is_some() {
                    state.loop_span = Some(moved);
                    state.loop_memory = Some(moved);
                    return vec![DeckCmd::SetLoopSpan {
                        deck,
                        span: Some(moved),
                        seek: LoopSeek::None,
                    }];
                }
                Vec::new()
            }
        }
    }

    pub fn delete_loop_slot(&mut self, deck: DeckId, slot: u16) {
        self.deck_mut(deck).loop_slots.retain(|entry| entry.slot != slot);
    }

    /// Exchange what two numbers hold. A filing gesture: nothing about the
    /// running loop, the memory or the playhead moves, and a number that
    /// is empty simply takes the loop that was dragged onto it.
    pub fn swap_loop_slots(&mut self, deck: DeckId, a: u16, b: u16) -> bool {
        if a == b || a >= LOOP_SLOT_CAP as u16 || b >= LOOP_SLOT_CAP as u16 {
            return false;
        }
        let slots = &mut self.deck_mut(deck).loop_slots;
        let at_a = slots.iter().position(|entry| entry.slot == a);
        let at_b = slots.iter().position(|entry| entry.slot == b);
        match (at_a, at_b) {
            (Some(x), Some(y)) => {
                // The whole entry trades places, not just its span: a kind
                // left behind on the other number would make a point
                // claim to be a loop, and a colour left behind would make
                // the row lie about which mark is which.
                let (span, kind, colour) = (slots[x].span, slots[x].kind, slots[x].colour);
                slots[x].span = slots[y].span;
                slots[x].kind = slots[y].kind;
                slots[x].colour = slots[y].colour;
                slots[y].span = span;
                slots[y].kind = kind;
                slots[y].colour = colour;
                true
            }
            (Some(x), None) => {
                slots[x].slot = b;
                slots.sort_by_key(|entry| entry.slot);
                true
            }
            (None, Some(y)) => {
                slots[y].slot = a;
                slots.sort_by_key(|entry| entry.slot);
                true
            }
            (None, None) => false,
        }
    }

    /// Put the row in playing order. `pack` closes the gaps and renumbers
    /// from zero; without it the numbers the row already holds stay exactly
    /// where they are and only which loop sits on which changes.
    ///
    /// Returns whether anything moved, so an already-sorted row costs no
    /// file write.
    pub fn sort_loop_slots(&mut self, deck: DeckId, pack: bool) -> bool {
        let slots = &mut self.deck_mut(deck).loop_slots;
        let before = slots.clone();
        // The WHOLE entry travels: only which number it sits on changes.
        let mut marks = slots.clone();
        // `total_cmp`, because a value that got past the loaders must not
        // be able to panic a comparator.
        marks.sort_by(|a, b| a.span.start_secs.total_cmp(&b.span.start_secs));
        let numbers: Vec<u16> = if pack {
            (0..marks.len() as u16).collect()
        } else {
            slots.iter().map(|entry| entry.slot).collect()
        };
        *slots = numbers
            .into_iter()
            .zip(marks)
            .map(|(slot, mark)| LoopSlot { slot, ..mark })
            .collect();
        *slots != before
    }

    /// The blue marker click: go into that loop again, running loop or
    /// not — and if that loop IS the one running, the second click exits
    /// it, the same gesture as the RELOOP/EXIT button.
    pub fn recall_loop(&mut self, deck: DeckId, slot: u16) -> Vec<DeckCmd> {
        let Some((span, kind)) = self
            .deck(deck)
            .loop_slots
            .iter()
            .find(|entry| entry.slot == slot)
            .map(|entry| (entry.span, entry.kind))
        else {
            return Vec::new();
        };
        match kind {
            SlotKind::Cue => self.seek_secs(deck, span.start_secs),
            SlotKind::Jump => match self.deck(deck).loop_span {
                Some(_) => self.move_loop(deck, span.start_secs),
                None => self.seek_secs(deck, span.start_secs),
            },
            // The length refusal is a PRECONDITION on ENGAGING, ahead of
            // whatever the kind claims. A file edited by hand, or a swap
            // that put a point on a loop's number, must not be able to
            // engage a one-frame span -- which on the audio thread is a
            // stuck buzz rather than a loop.
            SlotKind::Loop if span.len_secs() < LOOP_MIN_SECS => {
                self.seek_secs(deck, span.start_secs)
            }
            SlotKind::Loop => {
                if let Some(running) = self.deck(deck).loop_span {
                    if (running.start_secs - span.start_secs).abs() < 1e-6
                        && (running.end_secs - span.end_secs).abs() < 1e-6
                    {
                        return self.toggle_loop(deck);
                    }
                }
                self.engage_loop(deck, span, true)
            }
        }
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
            true => vec![DeckCmd::SetLoopSpan { deck, span: None, seek: LoopSeek::None }],
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

    /// Timed move to one side that takes `secs` however far it has to go.
    ///
    /// The autopilot's twin of `fade_to`. A transition's length is not a
    /// travel speed: it was measured against the incoming intro, the
    /// outgoing runway and a bar of the incoming grid, and the blend
    /// choreography is laid out against that same number. Scaling it by the
    /// distance left — which is what a hand asking for a RATE wants — would
    /// land the bass swap in the wrong bar whenever the fader was not parked
    /// at one end, and drop every step past the early landing.
    pub fn fade_over(&mut self, deck: DeckId, secs: f32) -> Vec<DeckCmd> {
        let position = match deck {
            DeckId::A => 0.0,
            DeckId::B => 1.0,
        };
        vec![DeckCmd::FadeCrossfader { position, secs: secs.max(0.0) }]
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
    ///
    /// This is the AUTOPILOT's hand-back and it retires the deck whatever
    /// it is doing: a fade that has landed hands back a deck that is still
    /// sounding, and a refusal here would leave that deck stranded and the
    /// set with nowhere to load. The operator's button goes through
    /// `eject_press`, which is where the refusal lives.
    pub fn eject(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        if matches!(self.deck(deck).load, DeckLoad::Loading { .. }) {
            return Vec::new();
        }
        let retire = self.retire_marks(deck);
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
        state.grid_placed = false;
        state.grid_locked = false;
        state.tempo_map = None;
        state.splat = None;
        state.position_secs = 0.0;
        state.synced = false;
        state.phase_offset_beats = 0.0;
        state.ext_sync = false;
        state.auto_opt_out = false;
        state.stems_ready = false;
        state.scratching = false;
        state.reland = None;
        // An ejected master hands the pin to the remaining group member
        // (or the group ends with it).
        self.hand_pin_over(deck);
        let mut cmds: Vec<DeckCmd> = retire.into_iter().collect();
        cmds.push(DeckCmd::UnloadTrack { deck });
        cmds.extend(self.freeze_release(deck));
        cmds
    }

    /// Start a momentary roll: a loop of the armed length from here, held
    /// only while the button is.
    ///
    /// The engine owns spans and the mixer owns positions, so a press is
    /// two commands -- the mixer latches a ghost wrapping through whatever
    /// is running NOW, then the ordinary span install. A roll held over
    /// another therefore returns into the one beneath it.
    pub fn roll_press(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        if self.roll_parents[deck.index()].len() >= crate::mixer::ROLL_STACK_CAP {
            return Vec::new();
        }
        let Some(len) = self.armed_secs(deck) else { return Vec::new() };
        let at = self.deck(deck).position_secs;
        let Some(span) = self.usable_span(deck, at, at + len) else { return Vec::new() };
        let parent = self.deck(deck).loop_span;
        self.roll_parents[deck.index()].push(parent);
        let mut cmds = vec![DeckCmd::RollPush { deck }];
        cmds.extend(self.engage_loop(deck, span, false));
        cmds
    }

    /// Let the roll go. Without `adopt` the deck lands where the record
    /// would have got to and the span underneath comes back; with it, the
    /// loop that is sounding becomes the deck's own and every level under
    /// it stands down.
    pub fn roll_release(&mut self, deck: DeckId, adopt: bool) -> Vec<DeckCmd> {
        let stack = &mut self.roll_parents[deck.index()];
        if stack.is_empty() {
            return Vec::new();
        }
        if adopt {
            stack.clear();
            return vec![DeckCmd::RollPop { deck, parent: None, adopt: true }];
        }
        let parent = stack.pop().flatten();
        let state = self.deck_mut(deck);
        state.loop_span = parent;
        state.loop_memory = parent.or(state.loop_memory);
        vec![DeckCmd::RollPop { deck, parent, adopt: false }]
    }

    /// How many rolls this deck is holding.
    pub fn rolls_held(&self, deck: DeckId) -> usize {
        self.roll_parents[deck.index()].len()
    }

    /// Press and hold FREEZE: refuses on an empty or paused deck, on a
    /// second press while one is already held, and when the record has
    /// not yet played far enough to reach back the lap's own length --
    /// the gate that keeps a fresh load from replaying whatever the
    /// PREVIOUS record left in the ring.
    ///
    /// The lap is one counted beat, or the armed loop rung when it is
    /// shorter than a beat -- so the sub-beat ladder next to CUE is also
    /// the glitch-size selector and needs no control of its own. A
    /// splat owns its own clock (`mixer.rs`: rate, key lock and scratch
    /// are all ignored while it runs), so the length is left in SOURCE
    /// beats rather than divided by a rate the splat is not using.
    pub fn freeze_press(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        if self.frozen[deck.index()] {
            return Vec::new();
        }
        if !self.deck(deck).is_loaded() || !self.deck(deck).playing {
            return Vec::new();
        }
        let beat = self.deck(deck).counted_beat_secs();
        let len = match self.armed_secs(deck) {
            Some(armed) if armed <= beat => armed,
            _ => beat,
        };
        if self.deck(deck).position_secs < len {
            return Vec::new();
        }
        let splat_active = self.splat(deck).is_some_and(|splat| splat.enabled);
        let rate = if splat_active { 1.0 } else { self.deck(deck).rate.max(1e-6) };
        self.frozen[deck.index()] = true;
        vec![DeckCmd::Freeze { deck, secs: Some(len / rate) }]
    }

    /// Let FREEZE go. A press that was refused released nothing, so a
    /// second release is a no-op rather than a stray command.
    pub fn freeze_release(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        if !self.frozen[deck.index()] {
            return Vec::new();
        }
        self.frozen[deck.index()] = false;
        vec![DeckCmd::Freeze { deck, secs: None }]
    }

    /// Whether this deck has a FREEZE held right now.
    pub fn frozen(&self, deck: DeckId) -> bool {
        self.frozen[deck.index()]
    }

    /// The tracks the undo can reach, newest first. For the tests and for
    /// anything that wants to say what a second press would put back.
    pub fn ejected_titles(&self) -> Vec<&str> {
        self.ejected.iter().map(|i| i.title.as_str()).collect()
    }

    /// The operator's retire button, which is one control with two meanings.
    ///
    /// A first press clears the deck — unless the deck is playing, in
    /// which case it does nothing at all. A DJ does not eject the record
    /// the room is dancing to, and the one time a press could mean that is
    /// the one time it is a mistake.
    ///
    /// A second press inside `EJECT_UNDO_MS` is the operator taking the
    /// first one back, so it loads the retired track again. The undo is not
    /// refused on a playing deck: the second press is deliberate, and by
    /// then the deck is carrying something the operator did not choose.
    ///
    /// The host passes its own clock rather than the engine keeping one, so
    /// the window is pinnable in a test.
    pub fn eject_press(&mut self, deck: DeckId, now_ms: u64) -> (EjectPress, Vec<DeckCmd>) {
        let armed = self.last_eject_ms[deck.index()]
            .is_some_and(|t| now_ms.saturating_sub(t) <= EJECT_UNDO_MS);
        if armed {
            // One eject buys one undo, however many times the button is hit.
            self.last_eject_ms[deck.index()] = None;
            // Skip anything already back on a deck — that is what makes
            // the reach "the last one, or the second-last if the last is
            // already back".
            let on_decks: Vec<AssetId> = [DeckId::A, DeckId::B]
                .iter()
                .filter_map(|d| self.deck(*d).item().map(|i| i.asset))
                .collect();
            let Some(at) = self.ejected.iter().position(|i| !on_decks.contains(&i.asset)) else {
                return (EjectPress::Nothing, Vec::new());
            };
            // The queue can refill the gap within a frame of the eject. That
            // track never sounded, so it is still the NEXT track: it goes to
            // the head of the queue, not its tail. Only a load in flight is
            // reclaimed — anything the operator loaded and started by
            // hand inside the window is theirs, and is left where it is.
            if let DeckLoad::Loading { item, .. } = self.deck(deck).load.clone() {
                if !self.deck(deck).playing {
                    self.queue.insert(0, item);
                }
            }
            let item = self.ejected.remove(at);
            let title = item.title.clone();
            let target = match deck {
                DeckId::A => DeckTarget::A,
                DeckId::B => DeckTarget::B,
            };
            // `click` supersedes a load in flight, so no eject comes first.
            return (EjectPress::Restored { title }, self.click(item, target));
        }
        match self.deck(deck).load.clone() {
            // A refusal arms no undo: there is nothing to take back.
            DeckLoad::Loading { .. } => (EjectPress::Busy, Vec::new()),
            DeckLoad::Empty => (EjectPress::Nothing, Vec::new()),
            // A track that never sounded is not worth offering back —
            // the same law the autopilot's `requeue: false` follows.
            DeckLoad::Failed { .. } => (EjectPress::Ejected, self.eject(deck)),
            DeckLoad::Loaded { item } => {
                if self.deck(deck).playing {
                    return (EjectPress::Busy, Vec::new());
                }
                self.ejected.insert(0, item);
                self.ejected.truncate(EJECT_HISTORY);
                self.last_eject_ms[deck.index()] = Some(now_ms);
                (EjectPress::Ejected, self.eject(deck))
            }
        }
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
        // A held freeze belongs to the VOICE (`SwapVoices` swaps the
        // whole thing), but `frozen` here is the engine's own mirror,
        // indexed by SLOT -- swapping the array along with it would
        // leave the chip lit on the wrong side for as long as a finger
        // is still down. Let go of both first, so there is nothing left
        // to travel with the swap or disagree about afterwards.
        let mut cmds = self.freeze_release(DeckId::A);
        cmds.extend(self.freeze_release(DeckId::B));
        self.decks.swap(0, 1);
        self.last_loaded = self.last_loaded.map(DeckId::other);
        // The undo window belongs to the deck's CONTENTS, which is what a
        // swap moves. Left behind, it arms the undo on the wrong side.
        self.last_eject_ms.swap(0, 1);
        self.crossfader = 1.0 - self.crossfader;
        cmds.push(DeckCmd::SwapVoices);
        cmds.push(DeckCmd::SetCrossfader { position: self.crossfader });
        cmds
    }

    /// The same record on both decks, from the same sample.
    ///
    /// The copy goes onto the deck a new track would land on, which is
    /// never the live one -- doubling onto the deck you are playing would
    /// be a way to lose the mix, not to work it.
    pub fn instant_double(&mut self) -> Vec<DeckCmd> {
        let to = self.auto_target();
        let from = to.other();
        if !self.deck(from).is_loaded() {
            return Vec::new();
        }
        let item = match &self.deck(from).load {
            DeckLoad::Loaded { item } => item.clone(),
            _ => return Vec::new(),
        };
        // Only the fields the RECORD owns, read out before the borrow
        // turns mutable. DeckState is not Copy and most of it belongs to
        // the slot rather than the track.
        let src = self.deck(from);
        let (duration, position, playing, grid) =
            (src.duration_secs, src.position_secs, src.playing, src.grid);
        let (rate, from_lock, pitch) = (src.rate, src.rate_from_lock, src.pitch);
        let (key_shift, keylock, span, cue, cue_placed) =
            (src.key_shift, src.keylock, src.loop_span, src.cue_secs, src.cue_placed);
        // The record the destination is losing is owed its marks.
        let retire = self.retire_marks(to);
        let dst = self.deck_mut(to);
        dst.load = DeckLoad::Loaded { item };
        dst.duration_secs = duration;
        dst.position_secs = position;
        dst.playing = playing;
        dst.grid = grid;
        dst.rate = rate;
        dst.rate_from_lock = from_lock;
        dst.pitch = pitch;
        dst.key_shift = key_shift;
        dst.keylock = keylock;
        dst.loop_span = span;
        dst.cue_secs = cue;
        // The mark travels with the record, and so does whether a hand put
        // it there rather than the analysis.
        dst.cue_placed = cue_placed;
        // The record travels; the marks the operator placed on the OTHER
        // slot do not, and neither does anything half-placed. The blue
        // chips went with the record that just left this deck -- kept,
        // they would be filed under the incoming track's name.
        dst.loop_slots.clear();
        dst.loop_armed = None;
        dst.bookmark = None;
        dst.splat = None;
        let mut cmds: Vec<DeckCmd> = retire.into_iter().collect();
        cmds.push(DeckCmd::CloneDeck { from, to });
        // The destination can genuinely be held -- `to` is whichever
        // deck the crossfader currently favours less, not necessarily
        // the quiet one -- and the mixer's own clone forgets the ring
        // underneath it regardless. Without this the chip stays lit on
        // a freeze that just went silent, and the next press on `to` is
        // swallowed by the engine's own already-held gate.
        cmds.extend(self.freeze_release(to));
        cmds
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
    /// The analysis landed. Take its grid, and — if nobody has cued this
    /// track — put the main mark where the file starts making a sound.
    ///
    /// A track that opens with two seconds of nothing used to cue to its
    /// own silence, so the first press of PLAY was a wait. The mark is
    /// written DIRECTLY rather than through `set_cue`: that one snaps under
    /// QUANT, and the first sounding sample is a measured fact rather than
    /// a placement to be rounded onto a beat. `cue_placed` stays false,
    /// because this is the default and not a hand.
    pub fn grid_ready(
        &mut self,
        deck: DeckId,
        gen: DeckGen,
        grid: TrackGrid,
        sound: Option<SoundSpan>,
        tempo_map: Option<std::sync::Arc<crate::wave_analysis::TempoMap>>,
    ) -> Vec<DeckCmd> {
        let mut cmds = Vec::new();
        {
            let state = self.deck_mut(deck);
            if state.load_gen != gen || !matches!(state.load, DeckLoad::Loaded { .. }) {
                return cmds;
            }
            if !state.grid_placed && !state.grid_locked {
                state.grid = Some(grid);
                state.tempo_map = tempo_map.filter(|map| !map.is_empty());
                cmds.push(DeckCmd::SetGrid { deck, grid: state.true_grid() });
            }
            if let Some(first) = sound
                .map(|span| span.first_secs)
                .filter(|first| *first > 0.0 && !state.cue_placed)
            {
                state.cue_secs = first;
                // And take a deck still parked at its top along with it.
                // Left behind, the mark sits off zero while the playhead
                // does not: the lamp blinks, and the first CUE press reads
                // as "stopped, away from the mark" and drags the default
                // back to where it came from.
                if !state.playing && state.position_secs < CUE_AT_MARK_SECS {
                    state.position_secs = first;
                    cmds.push(DeckCmd::SeekSeconds { deck, secs: first });
                }
            }
        }
        // After the block: the sync pass cannot run while the deck is
        // borrowed, and the seek has to reach the mixer before it.
        cmds.extend(self.apply_auto_sync());
        cmds
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

    /// What the mixer says the platter is turning at. A plain readout,
    /// like the splat's: no state machine, no clamp, and no sentinel --
    /// zero and negative numbers are legal readings from a brake and a
    /// reverse hold. Only a number that is not one is refused.
    pub fn observe_platter(&mut self, deck: DeckId, rate: f64) {
        if rate.is_finite() {
            self.deck_mut(deck).platter_rate = rate;
        }
    }

    // ---- tempo, sync, scratch ----------------------------------------------

    /// The deck the other one should follow. A pinned master stands first:
    /// once a lock has engaged, corrections must keep the same direction
    /// however the crossfader moves. Only with no master does the audible
    /// heuristic elect one.
    pub fn sync_leader(&self) -> Option<DeckId> {
        self.sync_master_valid().or_else(|| self.elect_leader())
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

    /// The level this deck's channel strip is set to, as the room would
    /// hear it: the fader the operator set, the level-match trim when
    /// NORMALISE is asking for one, the mute, and the crossfader's side.
    ///
    /// The same product the render forms per block, read off the intent
    /// the engine already holds rather than measured. Scoped deliberately
    /// to the channel STRIP: the transport is ranked separately (folding
    /// it in would make a paused deck with its fader up indistinguishable
    /// from a muted one), and the tone chain, the sweep filter and the
    /// per-stem gains are not counted -- a deck with every stem killed is
    /// inaudible and this still reports its fader, which is the honest
    /// answer to the question actually being asked.
    pub fn deck_strip_gain(&self, deck: DeckId) -> f32 {
        let state = self.deck(deck);
        if !state.is_loaded() || state.muted {
            return 0.0;
        }
        let (side_a, side_b) = crossfader_gains(self.crossfader, self.curve);
        let side = match deck {
            DeckId::A => side_a,
            DeckId::B => side_b,
        };
        state.effective_gain(self.normalise) * side
    }

    /// Whether the room is hearing this deck at all.
    pub fn deck_audible(&self, deck: DeckId) -> bool {
        self.deck(deck).playing && self.deck_strip_gain(deck) > AUDIBLE_FLOOR
    }

    /// Whichever deck is audibly leading, with no pin standing.
    ///
    /// Ranked: exactly one deck the room can hear leads; with both audible
    /// the louder side leads, and dead centre the deck that has been
    /// playing keeps the grid; with neither audible a playing deck still
    /// beats a stopped one, because a record running quietly is a better
    /// reference than one that is not running at all.
    ///
    /// There is no rung for a stopped deck: with nothing running there is
    /// nothing to follow, and naming a leader anyway would let a press or
    /// a load place a parked record nobody asked to move.
    ///
    /// The HYSTERESIS is not here -- it is the pin, which `sync_leader`
    /// consults first and which this is only reached in the absence of.
    fn elect_leader(&self) -> Option<DeckId> {
        let ready = |state: &DeckState| state.is_loaded() && state.sync_view().is_some();
        let running = |id: DeckId| ready(self.deck(id)) && self.deck(id).playing;
        match (
            running(DeckId::A) && self.deck_audible(DeckId::A),
            running(DeckId::B) && self.deck_audible(DeckId::B),
        ) {
            (true, false) => return Some(DeckId::A),
            (false, true) => return Some(DeckId::B),
            // Neither is in the room: a playing record still beats a
            // parked one, and two parked records elect nobody.
            (false, false) => {
                return [DeckId::A, DeckId::B].into_iter().find(|id| running(*id))
            }
            (true, true) => {}
        }
        let (gain_a, gain_b) = (
            self.deck_strip_gain(DeckId::A),
            self.deck_strip_gain(DeckId::B),
        );
        if gain_a - gain_b > 1e-5 {
            Some(DeckId::A)
        } else if gain_b - gain_a > 1e-5 {
            Some(DeckId::B)
        } else {
            // Dead centre: the deck that has been playing keeps the grid.
            self.last_loaded.map(DeckId::other)
        }
    }

    /// Keep the pin honest: drop a master that can no longer lead, hand the
    /// pin to a playing group member when the master has stopped — a paused
    /// playhead is a frozen phase, and a servo chasing it would drag a live
    /// deck backwards — and hand it on again when the room stops hearing
    /// the master at all.
    ///
    /// Called once per pump, and that is the right cadence rather than a
    /// shortcoming: a fade is a continuous condition with no event to hang
    /// a handover on, so the answer is taken at the servo's own rate.
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
            // Returning matters: `successor` closed over the master this
            // call started with, and below it would be reasoning about the
            // deck that has just given the pin up.
            return;
        }
        // A master the room cannot hear has stopped leading, whatever it is
        // doing: the pin goes to the group member that IS audible. Nothing
        // audible moves nothing — a silent group keeps the reference it
        // had, because there is nowhere better to put it. Deliberately not
        // symmetric with a stopped master: this one is a continuous
        // condition, so between the two ends of a fade both records are up,
        // both are audible, and the standing pin keeps its direction.
        if !self.deck_audible(master) {
            if let Some(next) = successor(self).filter(|id| self.deck_audible(*id)) {
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
        self.sync_to_with(leader, follower, None, SyncVerb::Lock, true)
    }

    /// SYNC asking for one of its four meanings rather than the latch.
    ///
    /// `Lock` is what the button has always done and is what `sync` is;
    /// the other three are one-shots -- they match the decks once and hand
    /// the follower back, which is why a deck that was not already latched
    /// opts out of auto sync when it takes one.
    pub fn sync_verb(&mut self, follower: DeckId, verb: SyncVerb) -> Vec<DeckCmd> {
        let Some(leader) = self.sync_leader().filter(|id| *id != follower) else {
            return Vec::new();
        };
        if verb.latches() {
            self.deck_mut(follower).auto_opt_out = false;
        }
        self.sync_to_with(leader, follower, None, verb, true)
    }

    fn sync_to_with(
        &mut self,
        leader: DeckId,
        follower: DeckId,
        quantize: Option<SyncQuantize>,
        verb: SyncVerb,
        asked: bool,
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
        // corrects whom at every event. A one-shot pins nothing and joins
        // nothing -- it matches the decks and hands the follower back.
        if verb.latches() && self.sync_master_valid().is_none() {
            self.sync_master = Some(leader);
        }
        // A paused leader is a frozen phase: match the tempo so the decks
        // run together when it starts, but never jump a playhead to align
        // with a playhead that is not moving. The play() re-lock lands the
        // phase when the leader actually runs.
        let leader_playing = self.deck(leader).playing;
        // Both read before the follower is borrowed mutably below.
        let leader_held = self.deck(leader).scratching;
        // A press lands whatever the unit says: the off row withholds the
        // landings NOBODY asked for, and the operator's finger is not one
        // of those. Deliberately not folded into the early return above --
        // returning there would drop the tempo match too, which is the
        // opposite of what the off row means.
        let land = asked || self.phase_landing_allowed(follower);
        let lookahead = self.land_lookahead_secs;
        let mut cmds = Vec::new();
        let state = self.deck_mut(follower);
        if verb.latches() {
            state.synced = true;
        } else if !state.synced {
            // Without this the one-shot is a fiction: auto sync re-locks
            // any deck that has not opted out, so the very next pump would
            // turn a tap into the latch. A tap on a deck that IS latched
            // leaves both flags alone -- that is a re-land, not a release,
            // and only the hold toggles the latch.
            state.auto_opt_out = true;
        }
        if verb.takes_tempo() {
            // Outside the guard on purpose: a lock that works out the rate
            // the deck already has still means that rate describes a PAIR
            // of tracks, and marking it only when it moved would miss
            // exactly the case where the two tracks already agreed.
            state.rate_from_lock = true;
            if (state.rate - plan.rate).abs() > 1e-9 {
                state.rate = plan.rate;
                // Show the operator the rate the sync chose on the slider.
                state.pitch = (plan.rate - 1.0).clamp(-0.5, 0.5);
                cmds.push(DeckCmd::SetRate { deck: follower, rate: plan.rate });
            }
        }
        // A press whose meaning is "put this deck in step" is also the
        // press that gives up a kept nudge: nothing else clears it, and
        // there has to be one gesture that does.
        if verb.takes_phase() && !verb.latches() {
            state.phase_offset_beats = 0.0;
        }
        // The landing lead is worked out from the rate the deck is ACTUALLY
        // running at. With the tempo half skipped it is still the fader's,
        // and reading the plan's would put every landing wrong by the
        // difference times the lookahead.
        let landing_rate = state.rate;
        if !verb.takes_phase() {
            return cmds;
        }
        // A hand on the record owns the playhead; the phase lock waits.
        // A held leader has no phase worth landing on — the same guard the
        // servo takes, at event time. The tempo match above still runs:
        // the leader's `rate` is the fader's and stays true throughout.
        if land && !state.scratching && !leader_held && leader_playing && state.playing {
            if let Some(secs) = plan.seek_secs {
                // Land where the lock is true when the seek ARRIVES: both
                // decks keep moving while the command crosses to the audio
                // thread, so an uncompensated landing is late by exactly
                // that much, every time.
                let secs = secs + landing_rate * lookahead;
                state.position_secs = secs;
                cmds.push(DeckCmd::SeekSeconds { deck: follower, secs });
            }
        } else if land && !state.scratching && !state.playing {
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
        if !state.is_loaded() || state.auto_opt_out || state.scratching || state.cue_held {
            return Vec::new();
        }
        if state.sync_view().is_none() {
            return Vec::new();
        }
        self.sync_to_with(leader, follower, quantize, SyncVerb::Lock, false)
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
                self.hand_pin_over(deck);
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
            if !state.ext_sync || !state.playing || state.scratching || state.cue_held {
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
    /// Whether the deck the group is following has a hand or a motor on
    /// its record.
    ///
    /// Asked on `sync_leader`, which is what the room clock reads, and so
    /// deliberately NOT the same question the servo gates on: that one
    /// takes `sync_master_valid`, and with no pin standing the two
    /// disagree. Inside the engine the deck is already in hand and is
    /// tested directly; this exists for the host.
    pub fn leader_platter_held(&self) -> bool {
        self.sync_leader().is_some_and(|deck| self.deck(deck).scratching)
    }

    pub fn hold_deck_sync(&mut self) -> Vec<DeckCmd> {
        self.refresh_sync_master();
        let Some(master) = self.sync_master_valid() else { return Vec::new() };
        // A paused master is a frozen phase — the followers free-run at the
        // matched tempo until it plays (or the pin hands over).
        if !self.deck(master).playing {
            return Vec::new();
        }
        // And a record under a hand or a motor is a phase nobody should be
        // corrected to: the same law, for a master that is still running.
        // Without this the master's jerking playhead crosses the re-seek
        // threshold on essentially every pump, and the deck the room is
        // hearing gets a seek twenty times a second for the length of the
        // gesture. The followers free-run at the matched tempo instead.
        if self.deck(master).scratching {
            return Vec::new();
        }
        let Some(view) = self.deck(master).sync_view() else { return Vec::new() };
        let mut cmds = Vec::new();
        for deck in [DeckId::A, DeckId::B] {
            if deck == master || self.auto_fade_hold == Some(deck) {
                continue;
            }
            let state = self.deck(deck);
            if !state.synced || state.ext_sync || !state.playing || state.scratching || state.cue_held {
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
        // Spend the walk, if one is running: this deck's own travel since
        // the last pump, measured on its own beats, so the whole mechanism
        // reads no clock. A step that is backwards or absurdly long is a
        // wrap, a seek or a second gesture -- the anchor is moved to the
        // new place and nothing is spent, because RETIRING there would
        // hand straight back to a servo that seeks.
        let walked = state.reland.map(|mut reland| {
            let beat = state.counted_beat_secs().max(1e-9);
            let step = state.position_secs - reland.last_secs;
            if step.is_finite() && (0.0..=beat * 4.0).contains(&step) {
                reland.beats_left -= step / beat;
            }
            reland.last_secs = state.position_secs;
            // Inside the dead band the servo would not land anyway: that
            // is the clean hand-back, and the ceiling is only for a deck
            // whose grid is wrong enough never to get there.
            let done =
                reland.beats_left <= 0.0 || follow.error_beats.abs() <= EXT_RESEEK_BEATS;
            (!done).then_some(reland)
        });
        let lookahead = self.land_lookahead_secs;
        // Never asked: this is the per-pump servo, and the off row means
        // its landing is withheld. The rate trim above stays live, so a
        // deck with the unit off still runs with the room -- it just is
        // never jumped.
        let land = self.phase_landing_allowed(deck);
        let mut cmds = Vec::new();
        let state = self.deck_mut(deck);
        if let Some(reland) = walked {
            state.reland = reland;
        }
        state.rate_from_lock = true;
        if (state.rate - follow.rate).abs() > 1e-4 {
            state.rate = follow.rate;
            state.pitch = (follow.rate - 1.0).clamp(-0.5, 0.5);
            cmds.push(DeckCmd::SetRate { deck, rate: follow.rate });
        }
        if let (true, Some(secs), None) = (land, follow.reseek_secs, state.reland) {
            let secs = secs + follow.rate * lookahead;
            state.position_secs = secs;
            cmds.push(DeckCmd::SeekSeconds { deck, secs });
        }
        cmds
    }

    /// No commands: unlike auto sync, a new unit changes nothing until
    /// the next seek, so there is nothing to emit.
    /// Choose what a fresh load puts back to nothing. Emits nothing: the
    /// policy only bites on the next load, so there is nothing to send now.
    pub fn set_load_reset(&mut self, policy: LoadReset) {
        self.load_reset = policy;
    }

    /// No commands: a new unit changes nothing until the next gesture, so
    /// there is nothing to send now.
    pub fn set_snap_beats(&mut self, deck: DeckId, beats: u32) {
        self.snap_beats[deck.index()] = beats;
    }

    pub fn snap_beats(&self, deck: DeckId) -> u32 {
        self.snap_beats[deck.index()]
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
        // A hand on the tempo makes it the operator's, whatever the lock
        // had made of it before.
        state.rate_from_lock = false;
        if !is_master {
            state.synced = false;
            state.auto_opt_out = true;
        }
        let mut cmds = vec![DeckCmd::SetRate { deck, rate: self.bent(deck, rate) }];
        // A tempo move on the LEADER propagates: the follower keeps up.
        if self.sync_leader() == Some(deck) {
            cmds.extend(self.apply_auto_sync());
        }
        cmds
    }

    /// What to command this deck, with any held bend on top.
    ///
    /// Clamped rather than merely added: a bend that could drive the rate
    /// to zero or through it would stop or reverse the record, and a bend
    /// is a nudge, never a transport control.
    fn bent(&self, deck: DeckId, base: f64) -> f64 {
        (base + self.deck(deck).bend).clamp(RATE_MIN, RATE_MAX)
    }

    /// Hold a bend: `direction` is +1 to push forward, -1 to hold back.
    pub fn hold_bend(&mut self, deck: DeckId, direction: f64, fine: bool) -> Vec<DeckCmd> {
        let step = if fine { BEND_FINE } else { BEND_COARSE };
        self.deck_mut(deck).bend = direction.signum() * step;
        let base = self.deck(deck).rate;
        vec![DeckCmd::SetRate { deck, rate: self.bent(deck, base) }]
    }

    /// Let it go: straight back to whatever the fader says, and the
    /// phase the hand walked to is the phase the lock now holds.
    ///
    /// Without this the servo pulls the deck back to dead phase the
    /// instant the button comes up, and the nudge -- the whole point of
    /// the gesture -- lasts about a second.
    pub fn release_bend(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        if self.deck(deck).bend != 0.0 {
            self.keep_nudge(deck);
        }
        self.deck_mut(deck).bend = 0.0;
        let rate = self.deck(deck).rate;
        vec![DeckCmd::SetRate { deck, rate }]
    }

    /// Take where the deck is SITTING as where it is asked to sit.
    ///
    /// Measured through the servo's own error rather than re-derived, so
    /// the number kept is exactly the number the servo would have spent
    /// closing: the residual against the current target, folded into it.
    fn keep_nudge(&mut self, deck: DeckId) {
        let state = self.deck(deck);
        if !state.synced || state.ext_sync {
            return;
        }
        let Some(leader) = self.sync_leader().filter(|id| *id != deck) else { return };
        let (Some(lead), Some(follow)) =
            (self.deck(leader).sync_view(), self.deck(deck).sync_view())
        else {
            return;
        };
        let envelope = state.pitch_range.fraction();
        let Some(follow) = external_follow(&lead, &follow, envelope) else { return };
        // The servo reports how far SHORT of the target the deck is, so
        // the new target is the old one less that much. Wrapped to half a
        // beat either way, which is all the error can ever be.
        let want = (state.phase_offset_beats - follow.error_beats).rem_euclid(1.0);
        let want = if want > 0.5 { want - 1.0 } else { want };
        self.deck_mut(deck).phase_offset_beats = want;
    }

    /// Trim the tempo permanently by a small step: `direction` is +1 to
    /// speed up, -1 to slow down.
    ///
    /// The step is a share of the TRACK's tempo, not of the selected range,
    /// so widening the range changes how far the fader reaches and not what
    /// this button does. It stops at the end of the range rather than
    /// running past it.
    pub fn trim_pitch(&mut self, deck: DeckId, direction: f64, fine: bool) -> Vec<DeckCmd> {
        let step = if fine { TRIM_FINE } else { TRIM_COARSE };
        let range = self.deck(deck).pitch_range.fraction();
        let want = self.deck(deck).pitch + direction.signum() * step;
        self.set_pitch(deck, want / range)
    }

    /// Step the fader's reach one rung wider or narrower.
    ///
    /// It emits nothing, and that is the whole point: the tempo a deck is
    /// running at is not the fader's to change, so choosing how far the
    /// fader reaches must never move the music. The old toggle re-expressed
    /// the standing pitch in the new range through `set_pitch`, which
    /// clamped the tempo whenever the range narrowed -- with a ladder, most
    /// presses -- and, because `set_pitch` opts a non-master out of the
    /// lock, silently unlocked a synced deck every time the button was
    /// pressed.
    ///
    /// A tempo already outside the new range simply pins the fader at its
    /// end until the hand moves it.
    pub fn step_pitch_range(&mut self, deck: DeckId, wider: bool) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        state.pitch_range = state.pitch_range.stepped(wider);
        Vec::new()
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
        // The readout is the way back to the track's own key and it has to
        // mean it: leaving a debt standing here would drive the shift
        // NEGATIVE at the next release. `nudge_key_shift` deliberately does
        // not do this, so a step taken while the lock holds survives it.
        self.deck_mut(deck).keylock_offset = 0.0;
        self.set_key_shift(deck, 0.0)
    }

    /// Key lock: hold the key while the tempo moves. It also sets where a key
    /// shift is measured from — the track's own key with the lock on, the
    /// already-varisped key with it off.
    pub fn toggle_keylock(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        state.keylock = !state.keylock;
        let engaged = state.keylock;
        let mode = state.keylock_mode;
        let mut cmds = vec![DeckCmd::SetKeylock { deck, on: engaged }];
        if engaged {
            if mode == KeylockMode::Original {
                return cmds;
            }
            // Hand the tempo's own interval to the shift knob, and the
            // render path then holds precisely the key that was already
            // sounding. The STANDING tempo, not the bent one: a lean held
            // across the press must not become a permanent anchor.
            let rate = self.deck(deck).rate;
            let before = self.deck(deck).key_shift;
            cmds.extend(self.set_key_shift(deck, before + semitones_of_rate(rate)));
            // Recorded AFTER the clamp, so the give-back is exact even at
            // the rail.
            let after = self.deck(deck).key_shift;
            self.deck_mut(deck).keylock_offset = after - before;
            return cmds;
        }
        // Letting go. Both conditions matter: `Current` alone would emit a
        // pointless shift of nothing in the default mode, and a standing
        // debt alone would take back semitones the operator was told they
        // could keep.
        let owed = self.deck(deck).keylock_offset;
        if mode == KeylockMode::Current && owed != 0.0 {
            let now = self.deck(deck).key_shift;
            cmds.extend(self.set_key_shift(deck, now - owed));
        }
        self.deck_mut(deck).keylock_offset = 0.0;
        cmds
    }

    /// Walk the three keys the lock can hold. Moves no pitch and sends no
    /// command: it says what the NEXT press will do, and the button reads
    /// it out so the choice can be made before anything is pressed.
    pub fn cycle_keylock_mode(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        state.keylock_mode = state.keylock_mode.cycled();
        Vec::new()
    }

    /// CUE went down.
    ///
    /// Paused away from the mark, this MOVES the mark here -- that is how a
    /// cue point gets set without a second control. Anywhere else it starts
    /// a preview from the mark, which sounds for as long as the button is
    /// held.
    ///
    /// The preview deliberately does not go through `seek_secs` or `play`:
    /// both re-run auto sync, and a playing follower would be jumped to the
    /// nearest beat -- up to half a beat from the very mark it is
    /// auditioning.
    pub fn cue_press(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        if !self.deck(deck).is_loaded() {
            return Vec::new();
        }
        let state = self.deck(deck);
        let at_mark = (state.position_secs - state.cue_secs).abs() < CUE_AT_MARK_SECS;
        if !state.playing && !at_mark {
            let secs = state.position_secs;
            let state = self.deck_mut(deck);
            state.cue_secs = secs;
            state.cue_placed = true;
            return Vec::new();
        }
        let cue = state.cue_secs;
        let state = self.deck_mut(deck);
        state.cue_held = true;
        state.playing = true;
        state.position_secs = cue;
        vec![
            DeckCmd::SeekSeconds { deck, secs: cue },
            DeckCmd::SetPlaying { deck, playing: true },
        ]
    }

    /// CUE came up: the preview stops and the record goes back to the mark.
    /// A press that only moved the mark has nothing to release.
    pub fn cue_release(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        if !self.deck(deck).cue_held {
            return Vec::new();
        }
        let cue = self.deck(deck).cue_secs;
        let state = self.deck_mut(deck);
        state.cue_held = false;
        state.playing = false;
        state.position_secs = cue;
        vec![
            DeckCmd::SetPlaying { deck, playing: false },
            DeckCmd::SeekSeconds { deck, secs: cue },
        ]
    }

    /// What the CUE lamp should be doing.
    pub fn cue_led(&self, deck: DeckId) -> CueLed {
        let state = self.deck(deck);
        if !state.is_loaded() {
            return CueLed::Dark;
        }
        if state.cue_held {
            return CueLed::Solid;
        }
        // Parked exactly on the mark, ready to go: the light every hand
        // reads as "this deck is cued and waiting".
        if !state.playing && (state.position_secs - state.cue_secs).abs() < CUE_AT_MARK_SECS {
            return CueLed::Solid;
        }
        // Stopped somewhere else: pressing CUE will move the mark here, and
        // the blink is the warning that it will.
        if !state.playing {
            return CueLed::Blink;
        }
        CueLed::Dark
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
                // Letting go of a FOLLOWER re-locks it against the leader,
                // from wherever the hand left the record. Letting go of the
                // deck the group follows is the other case entirely: the
                // re-lock would move the followers, so they are walked back
                // with the rate instead and nothing is seeked.
                let led = self.platter_released(deck);
                if !led && (self.deck(deck).synced || self.auto_sync) {
                    cmds.extend(self.apply_auto_sync_with(Some(SyncQuantize::Beat)));
                }
            }
        }
        cmds
    }

    /// The reverse hold. The record runs backwards while it is held and a
    /// ghost keeps the place it should have reached; letting go lands the
    /// deck on the ghost.
    ///
    /// Deliberately NOT `scratch`: that one re-locks the phase with a
    /// beat-quantised seek on release, and fired after the ghost has just
    /// handed the deck a position it would move the deck off the very place
    /// the hold exists to return it to. The ghost landing IS the truth.
    ///
    /// `scratching` is set for the duration so the sync servos leave the
    /// deck alone while it runs.
    pub fn censor(&mut self, deck: DeckId, on: bool) -> Vec<DeckCmd> {
        let state = self.deck(deck);
        if !state.is_loaded() || (on && !state.playing) {
            return Vec::new();
        }
        match on {
            true => self.deck_mut(deck).scratching = true,
            // Through the same funnel a hand uses: letting go of a reverse
            // hold on the leading record walks the followers back too.
            false => {
                self.platter_released(deck);
            }
        }
        vec![DeckCmd::Censor { deck, on }]
    }

    /// A motor gesture on the platter. `scratching` is set for the same
    /// reason it is for a hand: the servos must not chase a playhead that
    /// is deliberately not keeping time while the platter winds up or down.
    pub fn spin(&mut self, deck: DeckId, motion: SpinMotion) -> Vec<DeckCmd> {
        if !self.deck(deck).is_loaded() {
            return Vec::new();
        }
        self.spin_running[deck.index()] = true;
        let state = self.deck_mut(deck);
        state.scratching = true;
        state.playing = matches!(motion, SpinMotion::SoftStart);
        vec![DeckCmd::Spin { deck, motion }]
    }

    /// The platter is nobody's but the deck's again.
    ///
    /// One funnel for all three ways a gesture can end -- a hand let go, a
    /// reverse hold released, a motor landed -- because what happens next
    /// depends only on WHOSE record it was. Returns whether the deck that
    /// let go is the one the group follows: if it is, its followers are
    /// walked back with the rate and the caller must not re-lock them with
    /// a seek, which is the jump-cut this exists to remove.
    ///
    /// Arms nothing when the flag was not set on the way in, so a release
    /// nothing was holding earns no walk.
    fn platter_released(&mut self, deck: DeckId) -> bool {
        if !self.deck(deck).scratching {
            return false;
        }
        self.deck_mut(deck).scratching = false;
        // Read AFTER the clear, deliberately: clearing the flag does not
        // change who leads, and asking now is asking about the state the
        // followers are about to be corrected in.
        if self.sync_leader() != Some(deck) {
            return false;
        }
        let mut armed = false;
        for other in [DeckId::A, DeckId::B] {
            if other == deck {
                continue;
            }
            let state = self.deck(other);
            if !state.is_loaded() || !state.playing || !state.synced {
                continue;
            }
            let at = state.position_secs;
            self.deck_mut(other).reland =
                Some(Reland { beats_left: RELAND_BEATS, last_secs: at });
            armed = true;
        }
        armed
    }

    /// The mixer's word on whether the platter is still under a gesture.
    ///
    /// Only ever CLEARS, and only a motor gesture: a hand owns the flag
    /// from the instant it lands, before the mixer has seen anything.
    pub fn observe_spin(&mut self, deck: DeckId, gesture: bool) {
        if self.spin_running[deck.index()] && !gesture {
            self.spin_running[deck.index()] = false;
            self.platter_released(deck);
        }
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
        let state = self.deck_mut(deck);
        state.position_secs = secs;
        // A deliberate move ends any walk on this deck: the re-lock below
        // IS the landing, and there is nothing left to close gently.
        state.reland = None;
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
        let step = beats * state.counted_beat_secs();
        // INSIDE A LOOP, a jump moves the LOOP.
        //
        // A seek would throw the record out of the span and the wrap would
        // drag it straight back to an offset nobody chose -- so the
        // gesture reads as a stutter rather than as a move, and the loop
        // the operator was holding is still where it was. Moving the span
        // takes the record with it, at the same place inside the loop, and
        // that is what a jump means while one is running.
        if let Some(span) = state.loop_span {
            return self.move_loop(deck, span.start_secs + step);
        }
        self.seek_secs(deck, state.position_secs + step)
    }

    /// Correct this record's grid by hand.
    ///
    /// Returns the new grid and the commands the correction earns, exactly
    /// as the half-beat flip beside it does: the caller re-publishes the
    /// grid wherever else it lives and writes it down with the marks.
    /// `None` when there is nothing to correct or the correction would
    /// leave a tempo nothing downstream can use.
    pub fn edit_grid(
        &mut self,
        deck: DeckId,
        edit: GridEdit,
    ) -> Option<(TrackGrid, Vec<DeckCmd>)> {
        let state = self.deck(deck);
        if state.grid_locked {
            return None;
        }
        let old = state.true_grid()?;
        let at = state.position_secs;
        let grid = match edit {
            GridEdit::Adjust => {
                // Every ruling moves by the same hair, so the tempo and the
                // bar are untouched and only the anchor changes.
                let beat = old.beat_at(at).round();
                let mut first = at - beat * old.beat_secs;
                let mut phase = old.downbeat_phase as i64;
                let steps = (first / old.beat_secs).floor();
                first -= steps * old.beat_secs;
                phase -= steps as i64;
                TrackGrid {
                    first_beat_secs: first,
                    downbeat_phase: phase.rem_euclid(4) as u32,
                    ..old
                }
            }
            GridEdit::Downbeat => {
                // `is_downbeat` adds the phase to the beat number, so the
                // phase that makes THIS beat the one is its negative.
                let beat = old.beat_at(at).round() as i64;
                TrackGrid { downbeat_phase: (-beat).rem_euclid(4) as u32, ..old }
            }
            GridEdit::Scale(ratio) => {
                if !ratio.is_finite() || ratio <= 0.0 {
                    return None;
                }
                old.hinged_at(at, old.bpm * ratio)
            }
        };
        if !grid.has_grid() || !(EDIT_BPM_MIN..=EDIT_BPM_MAX).contains(&grid.bpm) {
            return None;
        }
        let state = self.deck_mut(deck);
        // A run of the same button is one decision: the step already on
        // the stack keeps the grid from BEFORE the run, so taking it back
        // undoes the whole run at once.
        if state.grid_undo.last().map(|(step, _)| *step) != Some(GridStep::Edit(edit)) {
            if state.grid_undo.len() >= GRID_UNDO_CAP {
                state.grid_undo.remove(0);
            }
            state.grid_undo.push((GridStep::Edit(edit), old));
        }
        state.grid = Some(grid);
        state.grid_placed = true;
        // A corrected grid is a different record as far as the fitted
        // moving tempo is concerned: it was fitted against the line that
        // has just been replaced.
        state.tempo_map = None;
        let mut cmds = vec![self.grid_cmd(deck)];
        if self.deck(deck).synced || self.auto_sync {
            cmds.extend(self.apply_auto_sync_with(Some(SyncQuantize::Beat)));
        }
        Some((grid, cmds))
    }

    /// Retune this record's grid from a hand's tapping.
    ///
    /// `tapped_bpm` is what the taps measured in ROOM time and
    /// `at_secs` is where the record was at the last of them. The record's
    /// own tempo is the tapped one divided by the rate the deck is running
    /// at -- tapping along with a deck pitched up measures the pitched
    /// tempo, and the grid is a property of the record.
    ///
    /// The tapped beat becomes the first of the bar: a hand tapping a
    /// record taps the one.
    pub fn tap_grid(
        &mut self,
        deck: DeckId,
        tapped_bpm: f64,
        at_secs: f64,
    ) -> Option<(TrackGrid, Vec<DeckCmd>)> {
        let state = self.deck(deck);
        if state.grid_locked || !state.is_loaded() {
            return None;
        }
        if !tapped_bpm.is_finite() || !at_secs.is_finite() || at_secs < 0.0 {
            return None;
        }
        let rate = if state.rate.is_finite() && state.rate > 1e-6 { state.rate } else { 1.0 };
        let bpm = tapped_bpm / rate;
        if !(EDIT_BPM_MIN..=EDIT_BPM_MAX).contains(&bpm) {
            return None;
        }
        let beat_secs = 60.0 / bpm;
        // The tap is a ruling, so the first beat at or after zero is that
        // ruling walked back into its own period -- and it is the one, so
        // the bar phase is however many periods that walk took.
        let steps = (at_secs / beat_secs).floor();
        let grid = TrackGrid {
            bpm,
            beat_secs,
            first_beat_secs: at_secs - steps * beat_secs,
            downbeat_phase: (-(steps as i64)).rem_euclid(4) as u32,
            confidence: 1.0,
        };
        if !grid.has_grid() {
            return None;
        }
        let old = state.grid;
        let state = self.deck_mut(deck);
        if let Some(old) = old.filter(|grid| grid.has_grid()) {
            if state.grid_undo.last().map(|(step, _)| *step) != Some(GridStep::Tap) {
                if state.grid_undo.len() >= GRID_UNDO_CAP {
                    state.grid_undo.remove(0);
                }
                state.grid_undo.push((GridStep::Tap, old));
            }
        }
        state.grid = Some(grid);
        state.grid_placed = true;
        state.tempo_map = None;
        let mut cmds = vec![self.grid_cmd(deck)];
        if self.deck(deck).synced || self.auto_sync {
            cmds.extend(self.apply_auto_sync_with(Some(SyncQuantize::Beat)));
        }
        Some((grid, cmds))
    }

    /// Whether there is a correction to take back.
    pub fn can_undo_grid(&self, deck: DeckId) -> bool {
        !self.deck(deck).grid_undo.is_empty()
    }

    /// Take back the last correction, or the last run of one.
    ///
    /// Returns what to re-publish, the same shape a correction does. The
    /// deck stays MARKED as hand-placed even when the stack empties: the
    /// operator has looked at this grid, and the analysis landing late
    /// must not quietly replace what they decided to keep.
    pub fn undo_grid(&mut self, deck: DeckId) -> Option<(TrackGrid, Vec<DeckCmd>)> {
        if self.deck(deck).grid_locked {
            return None;
        }
        let (_, grid) = self.deck_mut(deck).grid_undo.pop()?;
        if !grid.has_grid() {
            return None;
        }
        self.deck_mut(deck).grid = Some(grid);
        let mut cmds = vec![self.grid_cmd(deck)];
        if self.deck(deck).synced || self.auto_sync {
            cmds.extend(self.apply_auto_sync_with(Some(SyncQuantize::Beat)));
        }
        Some((grid, cmds))
    }

    /// Protect this grid from every later change, or stop protecting it.
    pub fn set_grid_locked(&mut self, deck: DeckId, locked: bool) {
        self.deck_mut(deck).grid_locked = locked;
    }

    /// A grid a hand corrected on this record before, and whether they
    /// settled it. Returns the command that carries it to the mixer.
    pub fn restore_grid(&mut self, deck: DeckId, grid: TrackGrid, locked: bool) -> Vec<DeckCmd> {
        if !grid.has_grid() {
            return Vec::new();
        }
        let state = self.deck_mut(deck);
        state.grid = Some(grid);
        state.grid_placed = true;
        state.grid_locked = locked;
        state.tempo_map = None;
        vec![self.grid_cmd(deck)]
    }

    /// The command that hands this deck's grid, as it stands, to the
    /// mixer. `true_grid`, so a grid with no beats is no grid there.
    fn grid_cmd(&self, deck: DeckId) -> DeckCmd {
        DeckCmd::SetGrid { deck, grid: self.deck(deck).true_grid() }
    }

    /// Flip the deck's grid half a beat. The analyser's known failure mode
    /// is a perfectly steady grid on the OFF pulse: same tempo, every ruling
    /// on a real transient, and sync then holds the two tracks exactly half
    /// a beat apart. Moving every ruling by half a beat puts the grid on the
    /// other pulse; the caller re-publishes the flipped grid wherever else
    /// it lives (analysis, loop grid, cache). Returns the flipped grid.
    pub fn flip_beat_phase(&mut self, deck: DeckId) -> Option<(TrackGrid, Vec<DeckCmd>)> {
        if self.deck(deck).grid_locked {
            return None;
        }
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
        let mut cmds = vec![self.grid_cmd(deck)];
        if self.deck(deck).synced || self.auto_sync {
            cmds.extend(self.apply_auto_sync_with(Some(SyncQuantize::Beat)));
        }
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
        // Mirrors the re-lock exactly, and must: this anchors the snap on
        // the phase the re-lock is ABOUT to impose, so a reason the
        // re-lock declines is a reason not to anchor on it. With the
        // leader's record under a hand that phase will never be imposed,
        // and anchoring on it would land an overview click up to a beat
        // from where the operator aimed.
        if self.deck(leader).scratching {
            return own;
        }
        let state = self.deck(deck);
        if !state.is_loaded() || state.auto_opt_out || state.scratching || state.cue_held {
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

    /// Whether the engine may put this deck's playhead somewhere nobody
    /// asked for.
    ///
    /// QUANT's off row is the hand claiming the playhead. The tempo match
    /// and the bounded rate trim stay -- the decks still run together --
    /// and only the LANDING is withheld, which is the manual beatmatch and
    /// is why this is not a second spelling of SYNC off. A landing someone
    /// PRESSED for is not covered: see `sync_to_with`'s `asked`.
    pub fn phase_landing_allowed(&self, deck: DeckId) -> bool {
        self.snap_beats(deck) > 0
    }

    /// Step the playhead a whole number of beats, forward or back — the
    /// correction a hand makes when the drop lands a hair off.
    ///
    /// Measured on the deck's OWN grid, so a deck running slow steps its
    /// own slow beats and lands where the operator is looking.
    ///
    /// Nothing without a grid: a beat has no length until the analysis
    /// arrives, and inventing one would move the playhead by an amount
    /// nobody could predict. Nothing while a hand is scratching either —
    /// the hand owns the transport for as long as it is down.
    ///
    /// A WHOLE beat is deliberate. `seek_secs` re-locks the phase behind
    /// us, and a whole-beat step is already phase-consistent, so the
    /// re-lock finds nothing to undo. A fractional step would be pulled
    /// straight back and the button would look broken.
    pub fn nudge_beats(&mut self, deck: DeckId, beats: f64) -> Vec<DeckCmd> {
        let state = self.deck(deck);
        if !state.is_loaded() || state.scratching {
            return Vec::new();
        }
        let target = (state.position_secs + beats * state.counted_beat_secs()).max(0.0);
        self.seek_secs(deck, target)
    }

    /// An operator-chosen seek, run through QUANT first. Only the overview
    /// strip uses this. Everything else — CUE, a lyric line, RELOOP,
    /// engaging a loop, every sync correction — seeks a target that must
    /// not be displaced.
    pub fn seek_secs_snapped(&mut self, deck: DeckId, secs: f64) -> Vec<DeckCmd> {
        let unit = self.snap_beats(deck);
        let snapped = match self.deck(deck).true_grid() {
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

    /// Step the sweep's resonance to the next rung, round and round:
    /// flat, a lift, a bigger lift, flat. Round rather than up-and-back
    /// so the chip is one press per step in the dark.
    pub fn cycle_resonance(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let rungs = crate::music_dsp::DeckEq::RESONANCE_RUNGS.len();
        let state = self.deck_mut(deck);
        state.resonance = (state.resonance + 1) % rungs;
        vec![DeckCmd::SetResonance { deck, lift: state.resonance_lift() }]
    }

    /// Step the echo to its next rung, round and round: off, whole beat,
    /// half, quarter, off.
    pub fn cycle_echo(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let rungs = crate::music_dsp::ECHO_RUNGS.len() + 1; // + off
        let state = self.deck_mut(deck);
        state.echo_rung = (state.echo_rung + 1) % rungs;
        vec![DeckCmd::SetEcho { deck, fraction: state.echo_fraction() }]
    }

    /// Whether the echo's repeats land on the other channel.
    pub fn toggle_echo_pingpong(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        state.echo_pingpong = !state.echo_pingpong;
        vec![DeckCmd::SetEchoPingpong { deck, on: state.echo_pingpong }]
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
        // A refusal is the second way to load nothing, and it is under the
        // same law: the row stays in the queue.
        if let Some(deck) = self.load_refused(target) {
            return vec![DeckCmd::LoadRefused { deck }];
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
    fn a_double_time_record_folds_onto_its_partner() {
        // 150 under 75 is not a record played at half speed, it is the same
        // pulse counted twice. The fold has to see that before anything
        // measures a gap, or every drum record reads as unmixable.
        assert!((tempo_ratio(75.0, 150.0) - 1.0).abs() < 1e-9);
        assert!((tempo_ratio(150.0, 75.0) - 1.0).abs() < 1e-9);
        assert!((tempo_fit(75.0, 150.0) - 1.0).abs() < 1e-6);
        // And a genuine gap still reads as one.
        assert!(tempo_ratio(120.0, 126.0) > 1.0);
        assert!(tempo_fit(120.0, 126.0) < 1.0);
    }

    #[test]
    fn tempo_fit_falls_away_as_the_gap_widens_and_bottoms_out_past_the_gate() {
        let close = tempo_fit(128.0, 129.0);
        let near = tempo_fit(128.0, 132.0);
        let far = tempo_fit(128.0, 140.0);
        assert!(close > near && near > far, "{close} {near} {far}");
        assert!(close <= 1.0 && far >= 0.0);
        // Beyond the gate there is nothing left to grade.
        assert!((tempo_fit(128.0, 200.0) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn the_meeting_tempo_leans_on_the_record_that_speeds_up() {
        // Both records give way, so neither is stretched by the whole gap.
        // Dragging a record down is heard sooner than pushing one up, so the
        // meeting point sits above the halfway house, and the slower record
        // does more of the travelling.
        let (slow, fast) = (123.0, 143.0);
        let meet = meeting_tempo(slow, fast);
        assert!(meet > slow && meet < fast, "between the two: {meet}");
        let halfway = (slow * fast).sqrt();
        assert!(meet > halfway, "leaning the right way: {meet} over {halfway}");
        let up = meet / slow - 1.0;
        let down = 1.0 - meet / fast;
        assert!(up > down, "the slower record travels further: {up} vs {down}");
        // Two records already together have nowhere to meet but where they are.
        assert!((meeting_tempo(128.0, 128.0) - 128.0).abs() < 1e-9);
    }

    #[test]
    fn an_autopilot_fade_spends_its_whole_duration_wherever_the_fader_stands() {
        // `fade_to` is a RATE: the hand asks for a travel speed, so a fader
        // already halfway across takes half the time. The autopilot asks for
        // a DURATION — its length was measured against the incoming intro and
        // the outgoing runway, and the blend choreography is laid out against
        // that same number. Spend a shorter ride on a part-crossed fader and
        // the bass swap lands in the wrong bar, or never lands at all.
        let mut e = DeckEngine::new();
        e.set_crossfader(0.5);

        assert_eq!(
            e.fade_to(DeckId::B, 8.0),
            vec![DeckCmd::FadeCrossfader { position: 1.0, secs: 4.0 }],
            "the performance button keeps its travel speed"
        );
        assert_eq!(
            e.fade_over(DeckId::B, 8.0),
            vec![DeckCmd::FadeCrossfader { position: 1.0, secs: 8.0 }],
            "the autopilot's blend keeps its length"
        );
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
            DeckCmd::SetPlaying { deck: DeckId::A, .. }
                            | DeckCmd::InstallTrack { deck: DeckId::A, .. }
        )));
        assert!(e.deck(DeckId::A).playing);
        let (db, gb) = load_gen(&cmds);
        assert_eq!(db, DeckId::B);
        let cmds = e.track_ready(db, gb, 45.0);
        assert!(cmds.contains(&DeckCmd::InstallTrack { deck: DeckId::B, keep_playing: false }));
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
        assert!(cmds.contains(&DeckCmd::InstallTrack { deck: DeckId::A, keep_playing: false }));
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
        assert!(cmds.contains(&DeckCmd::SetLoopSpan { deck: DeckId::B, span: None , seek: LoopSeek::None }));
        assert!(cmds.contains(&DeckCmd::SetMute { deck: DeckId::B, muted: true }));
        assert!(cmds.contains(&DeckCmd::SetGain { deck: DeckId::B, gain: 0.5 }));
    }

    // -----------------------------------------------------------------
    // FREEZE
    // -----------------------------------------------------------------

    fn splat_grid_fixture(bpm: f64) -> std::sync::Arc<SplatGrid> {
        std::sync::Arc::new(SplatGrid {
            bpm,
            bar_secs: 60.0 / bpm * 4.0,
            first_bar_secs: 0.0,
            sections: Vec::new(),
            cells: [[None; SPLAT_COLS]; crate::loop_splat::SPLAT_ROWS],
            bars_per_col: [1; SPLAT_COLS],
        })
    }

    #[test]
    fn freeze_press_needs_a_loaded_playing_record_and_sizes_itself_on_the_heard_beat() {
        let mut e = DeckEngine::new();
        assert!(e.freeze_press(DeckId::A).is_empty(), "nothing loaded");
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // beat_secs = 0.5
        assert!(e.freeze_press(DeckId::A).is_empty(), "loaded but paused");
        e.deck_mut(DeckId::A).playing = true;
        e.deck_mut(DeckId::A).position_secs = 10.0;
        e.deck_mut(DeckId::A).rate = 1.25;
        e.deck_mut(DeckId::A).loop_ticks = 0; // MAN: no armed rung
        let cmds = e.freeze_press(DeckId::A);
        assert_eq!(cmds, vec![DeckCmd::Freeze { deck: DeckId::A, secs: Some(0.4) }]); // 0.5 / 1.25
        assert!(e.frozen(DeckId::A));
        assert!(e.freeze_press(DeckId::A).is_empty(), "a second press while held does nothing");
        e.freeze_release(DeckId::A);
        assert!(!e.frozen(DeckId::A));
        assert!(e.freeze_release(DeckId::A).is_empty(), "a second release does nothing");

        // An eighth of a beat: the ladder below a beat is the glitch size.
        e.deck_mut(DeckId::A).loop_ticks = 4; // 4 / 32
        let cmds = e.freeze_press(DeckId::A);
        assert_eq!(cmds, vec![DeckCmd::Freeze { deck: DeckId::A, secs: Some(0.05) }]); // (0.5 * 4/32) / 1.25
        e.freeze_release(DeckId::A);

        // Four beats armed is capped at one.
        e.deck_mut(DeckId::A).loop_ticks = 128; // 4 beats
        let cmds = e.freeze_press(DeckId::A);
        assert_eq!(cmds, vec![DeckCmd::Freeze { deck: DeckId::A, secs: Some(0.4) }]);
        e.freeze_release(DeckId::A);

        // A splat owns its own clock -- rate is ignored while it runs.
        e.deck_mut(DeckId::A).loop_ticks = 0;
        e.splat_set(DeckId::A, splat_grid_fixture(120.0));
        e.splat_enable(DeckId::A, true);
        let cmds = e.freeze_press(DeckId::A);
        assert_eq!(cmds, vec![DeckCmd::Freeze { deck: DeckId::A, secs: Some(0.5) }]);
        e.freeze_release(DeckId::A);
        e.splat_enable(DeckId::A, false);

        // Not far enough along to reach back the lap's own length.
        e.deck_mut(DeckId::A).position_secs = 0.1; // less than the 0.5 s beat
        assert!(e.freeze_press(DeckId::A).is_empty(), "not far enough along");
    }

    #[test]
    fn eject_and_install_release_a_held_freeze() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).playing = true;
        e.deck_mut(DeckId::A).position_secs = 10.0;
        e.freeze_press(DeckId::A);
        assert!(e.frozen(DeckId::A));
        let cmds = e.eject(DeckId::A);
        assert!(cmds.contains(&DeckCmd::Freeze { deck: DeckId::A, secs: None }));
        assert!(!e.frozen(DeckId::A));

        load_analysed(&mut e, DeckId::B, 2, 120.0, 0.0);
        e.deck_mut(DeckId::B).playing = true;
        e.deck_mut(DeckId::B).position_secs = 10.0;
        e.freeze_press(DeckId::B);
        assert!(e.frozen(DeckId::B));
        // Paused for the click itself: a load over a playing deck is its
        // own gated gesture (engine-core-c5) and not what this test is
        // about; `track_ready` landing is.
        e.deck_mut(DeckId::B).playing = false;
        let (d, g) = load_gen(&e.click(item(3), DeckTarget::B));
        let cmds = e.track_ready(d, g, 20.0);
        assert!(cmds.contains(&DeckCmd::Freeze { deck: DeckId::B, secs: None }));
        assert!(!e.frozen(DeckId::B));
    }

    /// A swap moves the whole VOICE (mixer.rs), but `frozen` here is the
    /// engine's own per-SLOT mirror; without releasing both first, the
    /// chip would light on the wrong side for as long as a finger is
    /// still down.
    #[test]
    fn swap_releases_any_held_freeze_on_either_deck() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).playing = true;
        e.deck_mut(DeckId::A).position_secs = 10.0;
        e.freeze_press(DeckId::A);
        assert!(e.frozen(DeckId::A));
        let cmds = e.swap();
        assert!(cmds.contains(&DeckCmd::Freeze { deck: DeckId::A, secs: None }));
        assert!(!e.frozen(DeckId::A) && !e.frozen(DeckId::B));
    }

    /// A DOUBLE lands on whichever deck the crossfader currently favours
    /// LESS -- not necessarily a silent one -- and that deck can
    /// genuinely have a freeze held on it. The mixer's own clone forgets
    /// the ring underneath either way; without this the engine's chip
    /// stays lit on a freeze that just went silent.
    #[test]
    fn instant_double_releases_a_held_freeze_on_the_deck_it_lands_on() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 120.0, 0.0);
        e.deck_mut(DeckId::A).playing = true;
        e.deck_mut(DeckId::A).position_secs = 10.0;
        e.deck_mut(DeckId::B).playing = true;
        e.deck_mut(DeckId::B).position_secs = 10.0;
        // The fader favours A, so DOUBLE lands on the quieter B.
        e.set_crossfader(0.0);
        assert_eq!(e.auto_target(), DeckId::B);
        e.freeze_press(DeckId::B);
        assert!(e.frozen(DeckId::B));
        let cmds = e.instant_double();
        assert!(cmds.contains(&DeckCmd::Freeze { deck: DeckId::B, secs: None }));
        assert!(!e.frozen(DeckId::B), "the destination's chip must not stay lit");
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
        state.loop_ticks = 512;
        let (d, g) = load_gen(&e.click(item(3), DeckTarget::B));
        let cmds = e.track_ready(d, g, 20.0);
        let state = e.deck(DeckId::B);
        assert!(state.loop_span.is_none(), "a span belongs to the track it was measured on");
        assert!(state.loop_memory.is_none() && state.loop_armed.is_none());
        assert_eq!(state.loop_ticks, 512, "the armed length is an operator preference");
        assert!(cmds.contains(&DeckCmd::SetLoopSpan { deck: DeckId::B, span: None , seek: LoopSeek::None }));
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
        e.deck_mut(DeckId::A).loop_ticks = 128;
        e.observe(DeckId::A, 10.25, true);
        let cmds = e.loop_in(DeckId::A);
        let span = e.deck(DeckId::A).loop_span.expect("a span");
        assert!((span.start_secs - 10.25).abs() < 1e-9, "IN sits exactly at the playhead");
        assert!((span.len_secs() - 2.0).abs() < 1e-9, "4 beats at 120 BPM = 2 s");
        assert!(cmds.contains(&DeckCmd::SetLoopSpan { deck: DeckId::A, span: Some(span) , seek: LoopSeek::None }));
        assert!(seek_of(&cmds, DeckId::A).is_none(), "the playhead is already at IN");
    }

    #[test]
    fn bracket_out_builds_n_beats_back_from_the_playhead() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_ticks = 256;
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
    fn beat_brackets_count_in_seconds_without_a_grid() {
        // The count used to be dead here, so a 4 on the dial did nothing at
        // all until the analysis landed. It counts in SECONDS now -- one
        // beat a second -- which is a unit, not a claim about the record.
        let mut e = DeckEngine::new();
        load_unanalysed(&mut e, DeckId::A, 1);
        assert!(!e.deck(DeckId::A).has_true_beats());
        e.deck_mut(DeckId::A).loop_ticks = 128;
        e.observe(DeckId::A, 10.0, true);
        assert!(!e.loop_in(DeckId::A).is_empty(), "the bracket engages");
        let span = e.deck(DeckId::A).loop_span.expect("a span");
        assert!((span.start_secs - 10.0).abs() < 1e-9);
        assert!((span.end_secs - 14.0).abs() < 1e-9, "four counted beats is four seconds");
        assert!(e.deck(DeckId::A).loop_armed.is_none(), "a beat count must not arm MAN");

        // And `]` sizes the same span backwards from where the record is.
        e.observe(DeckId::A, 20.0, true);
        assert!(!e.loop_out(DeckId::A).is_empty());
        let span = e.deck(DeckId::A).loop_span.expect("a span");
        assert!((span.start_secs - 16.0).abs() < 1e-9 && (span.end_secs - 20.0).abs() < 1e-9);
    }

    #[test]
    fn a_deck_with_no_track_still_has_no_count_to_measure() {
        let mut e = DeckEngine::new();
        e.deck_mut(DeckId::A).loop_ticks = 128;
        assert!(e.loop_in(DeckId::A).is_empty(), "no record, no length");
        assert!(e.deck(DeckId::A).loop_span.is_none());
    }

    #[test]
    fn a_measured_grid_takes_the_count_back_off_seconds() {
        let mut e = DeckEngine::new();
        let (deck, gen) = load_gen(&e.click(item(1), DeckTarget::A));
        e.track_ready(deck, gen, 300.0);
        e.deck_mut(DeckId::A).loop_ticks = 128;
        e.observe(DeckId::A, 10.0, true);
        e.loop_in(DeckId::A);
        assert!((e.deck(DeckId::A).loop_span.unwrap().end_secs - 14.0).abs() < 1e-9);
        // 120 BPM: four beats is two seconds, not four.
        e.grid_ready(deck, gen, grid(120.0, 0.0), None, None);
        assert!(e.deck(DeckId::A).has_true_beats());
        e.observe(DeckId::A, 30.0, true);
        e.loop_in(DeckId::A);
        assert!((e.deck(DeckId::A).loop_span.unwrap().end_secs - 32.0).abs() < 1e-9);
    }


    #[test]
    fn a_jump_inside_a_loop_moves_the_loop_and_takes_the_record_with_it() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // half a second a beat
        e.deck_mut(DeckId::A).loop_ticks = 4 * LOOP_TICKS_PER_BEAT;
        e.observe(DeckId::A, 10.0, true);
        e.loop_in(DeckId::A); // 10.0 .. 12.0
        e.observe(DeckId::A, 10.5, true);

        let cmds = e.beat_jump(DeckId::A, 8.0); // four seconds on
        let span = e.deck(DeckId::A).loop_span.expect("still looping");
        assert!((span.start_secs - 14.0).abs() < 1e-9, "the loop moved, to {}", span.start_secs);
        assert!((span.len_secs() - 2.0).abs() < 1e-9, "and kept its length");
        // The record keeps its place INSIDE the loop rather than being
        // thrown out of it and dragged back by the wrap.
        assert!((e.deck(DeckId::A).position_secs - 14.5).abs() < 1e-9);
        assert!(cmds.iter().any(|c| matches!(c, DeckCmd::SetLoopSpan { .. })));

        // And RELOOP remembers where it ended up.
        assert_eq!(e.deck(DeckId::A).loop_memory, Some(span));
    }

    #[test]
    fn a_jump_with_no_loop_running_is_the_seek_it_always_was() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.observe(DeckId::A, 10.0, true);
        e.beat_jump(DeckId::A, 8.0);
        assert!((e.deck(DeckId::A).position_secs - 14.0).abs() < 1e-9);
        assert!(e.deck(DeckId::A).loop_span.is_none());
    }
    #[test]
    fn a_jump_and_a_nudge_move_in_seconds_without_a_grid() {
        let mut e = DeckEngine::new();
        load_unanalysed(&mut e, DeckId::A, 1);
        e.observe(DeckId::A, 40.0, true);
        e.beat_jump(DeckId::A, 16.0);
        assert!(
            (e.deck(DeckId::A).position_secs - 56.0).abs() < 1e-9,
            "sixteen counted beats is sixteen seconds, at {}",
            e.deck(DeckId::A).position_secs,
        );
        e.nudge_beats(DeckId::A, -1.0);
        assert!((e.deck(DeckId::A).position_secs - 55.0).abs() < 1e-9);
    }

    #[test]
    fn a_counted_beat_never_reaches_the_things_that_take_a_grid_as_a_fact() {
        // The whole point of keeping the fallback out of `grid`: nothing
        // that snaps, syncs or shows a tempo may see it.
        let mut e = DeckEngine::new();
        load_unanalysed(&mut e, DeckId::A, 1);
        let state = e.deck(DeckId::A);
        assert!(state.true_grid().is_none());
        assert!(state.effective_bpm().is_none(), "no tempo is claimed");
        assert!(state.sync_view().is_none(), "and nothing can lock to it");
        assert!((state.counted_beat_secs() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn man_arms_on_in_and_closes_on_out_with_no_grid_at_all() {
        let mut e = DeckEngine::new();
        load_unanalysed(&mut e, DeckId::A, 1);
        e.deck_mut(DeckId::A).loop_ticks = 0; // MAN
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
        e.deck_mut(DeckId::A).loop_ticks = 0;
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
        e.deck_mut(DeckId::A).loop_ticks = 0;
        e.observe(DeckId::A, 10.0, true);
        assert!(e.loop_out(DeckId::A).is_empty());
        assert!(e.deck(DeckId::A).loop_span.is_none());
    }

    #[test]
    fn a_span_that_runs_off_the_end_is_refused() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // 300 s
        e.deck_mut(DeckId::A).loop_ticks = 2048; // 32 s
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
        e.deck_mut(DeckId::A).loop_ticks = 0;
        e.observe(DeckId::A, 10.0, true);
        e.loop_in(DeckId::A);
        // A hair after IN: all seam, no music.
        e.observe(DeckId::A, 10.0 + LOOP_MIN_SECS * 0.5, true);
        e.loop_out(DeckId::A);
        assert!(e.deck(DeckId::A).loop_span.is_none(), "under the floor, nothing engages");
    }

    #[test]
    fn the_count_walks_the_ladder_and_falls_off_it_into_man() {
        let bottom = LOOP_LADDER[0].0;
        let top = LOOP_LADDER[LOOP_LADDER.len() - 1].0;
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        // Down the ladder from two beats to its shortest rung, and then
        // off the bottom into MAN.
        e.deck_mut(DeckId::A).loop_ticks = 2 * LOOP_TICKS_PER_BEAT;
        e.loop_halve(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_ticks, LOOP_TICKS_PER_BEAT, "one beat");
        for _ in 0..5 {
            e.loop_halve(DeckId::A);
        }
        assert_eq!(e.deck(DeckId::A).loop_ticks, bottom, "a thirty-second of a beat");
        e.loop_halve(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_ticks, 0, "and then MAN");
        e.loop_halve(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_ticks, 0, "MAN is the floor");
        e.loop_double(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_ticks, bottom, "0 doubles onto the ladder");
        // And all the way up it to the top rung, then the bookmark.
        for _ in 0..(LOOP_LADDER.len() - 1) {
            e.loop_double(DeckId::A);
        }
        assert_eq!(e.deck(DeckId::A).loop_ticks, top, "the last measured rung");
        e.loop_double(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_ticks, LOOP_BEATS_INF, "then the bookmark rung");
    }

    #[test]
    fn every_rung_of_the_ladder_is_a_doubling_and_the_dial_shows_them_all() {
        // One table, and the count dial's rows are built from it.
        for pair in LOOP_LADDER.windows(2) {
            assert_eq!(pair[1].0, pair[0].0 * 2, "{} then {}", pair[0].1, pair[1].1);
        }
        assert_eq!(LOOP_LADDER[0].1, "1/32");
        assert_eq!(LOOP_LADDER[LOOP_LADDER.len() - 1].1, "512");
        assert_eq!(loop_rung_index(LOOP_TICKS_PER_BEAT).map(|i| LOOP_LADDER[i].1), Some("1"));
        assert_eq!(loop_rung_index(0), None, "MAN is not a rung");
        assert_eq!(loop_rung_index(LOOP_BEATS_INF), None, "nor is the bookmark");
    }

    #[test]
    fn a_hand_set_length_is_read_back_as_the_nearest_rung() {
        // Log, not arithmetic: three beats is equidistant from two and
        // four by subtraction, and four is the musical answer.
        assert_eq!(nearest_loop_rung(4.0), 4 * LOOP_TICKS_PER_BEAT);
        assert_eq!(nearest_loop_rung(3.0), 4 * LOOP_TICKS_PER_BEAT);
        assert_eq!(nearest_loop_rung(0.26), 8, "a hair over a quarter beat");
        // And it stays on the ladder whatever it is handed.
        assert_eq!(nearest_loop_rung(1e9), LOOP_LADDER[LOOP_LADDER.len() - 1].0);
        assert_eq!(nearest_loop_rung(1e-9), LOOP_LADDER[0].0);
        assert_eq!(nearest_loop_rung(f64::NAN), LOOP_TICKS_PER_BEAT);
        assert_eq!(nearest_loop_rung(-1.0), LOOP_TICKS_PER_BEAT);
    }


    #[test]
    fn a_sub_beat_loop_starts_on_the_beats_own_subdivision() {
        // A stutter that starts between two subdivisions arrives late
        // every lap, and the ear has the beat itself to compare it with.
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // beats at 0.0, 0.5, 1.0 ...
        // A quarter of a beat is 0.125 s: the slots are every 0.125 s.
        e.deck_mut(DeckId::A).loop_ticks = 8;
        e.observe(DeckId::A, 10.19, true);
        e.loop_in(DeckId::A);
        let span = e.deck(DeckId::A).loop_span.expect("engaged");
        assert!(
            (span.start_secs - 10.25).abs() < 1e-9,
            "landed on the nearer slot, at {}",
            span.start_secs,
        );
        assert!((span.len_secs() - 0.125).abs() < 1e-9);

        // A WHOLE-beat rung is untouched: it starts exactly where the
        // record is, which is what the brackets have always done.
        e.toggle_loop(DeckId::A);
        e.deck_mut(DeckId::A).loop_ticks = 4 * LOOP_TICKS_PER_BEAT;
        e.observe(DeckId::A, 10.19, true);
        e.loop_in(DeckId::A);
        let span = e.deck(DeckId::A).loop_span.expect("engaged");
        assert!((span.start_secs - 10.19).abs() < 1e-9, "at {}", span.start_secs);
    }
    #[test]
    fn a_sub_beat_rung_arms_a_sub_beat_loop() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // half a second a beat
        e.observe(DeckId::A, 10.0, true);
        // An eighth of a beat is 62.5 ms at this tempo -- above the floor
        // the stretcher's own grain sets, so it engages.
        e.deck_mut(DeckId::A).loop_ticks = 4;
        e.loop_in(DeckId::A);
        let span = e.deck(DeckId::A).loop_span.expect("engaged");
        assert!((span.len_secs() - 0.0625).abs() < 1e-9, "at {}", span.len_secs());
        // A thirty-second is 15.6 ms, under that floor, and is refused
        // rather than clamped -- the floor belongs to the stretcher.
        e.toggle_loop(DeckId::A);
        e.deck_mut(DeckId::A).loop_ticks = 1;
        assert!(e.loop_in(DeckId::A).is_empty());
    }

    #[test]
    fn halving_a_running_loop_anchors_on_in_and_emits_no_seek() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_ticks = 128;
        e.observe(DeckId::A, 10.0, true);
        e.loop_in(DeckId::A); // 10.0 .. 12.0
        e.observe(DeckId::A, 11.5, true); // three quarters through
        let cmds = e.loop_halve(DeckId::A);
        let span = e.deck(DeckId::A).loop_span.expect("a span");
        assert!((span.start_secs - 10.0).abs() < 1e-9, "IN is the anchor");
        assert!((span.len_secs() - 1.0).abs() < 1e-9, "the span halves with the count");
        assert_eq!(e.deck(DeckId::A).loop_ticks, 64);
        // No seek: the engine's mirror of the playhead is a stale 20 Hz
        // number, so the MIXER catches a stranded playhead — modulo the new
        // length, keeping the subdivision's phase instead of re-triggering
        // IN. Seeking from here was the historical stutter.
        assert!(seek_of(&cmds, DeckId::A).is_none(), "a resize must not seek");
        // But it SAYS it is a resize, so the mixer -- which has the real
        // playhead rather than a 20 Hz mirror of it -- can fold a head
        // that belonged to the old span into the new one.
        assert!(cmds.contains(&DeckCmd::SetLoopSpan {
            deck: DeckId::A,
            span: Some(span),
            seek: LoopSeek::Changed,
        }));
    }

    #[test]
    fn the_cutter_works_on_a_manual_span_with_no_grid() {
        let mut e = DeckEngine::new();
        load_unanalysed(&mut e, DeckId::A, 1);
        e.deck_mut(DeckId::A).loop_ticks = 0;
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
        e.deck_mut(DeckId::A).loop_ticks = 128;
        e.observe(DeckId::A, 297.0, true);
        e.loop_in(DeckId::A); // 297.0 .. 299.0
        let cmds = e.loop_double(DeckId::A); // would end at 301, past the track
        assert!(cmds.is_empty());
        assert_eq!(e.deck(DeckId::A).loop_ticks, 128, "a refused resize does not move N");
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
        e.set_snap_beats(DeckId::A, 4);
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
        e.set_snap_beats(DeckId::A, 0);
        e.observe(DeckId::A, 10.2, true);
        let cmds = e.seek_secs_snapped(DeckId::A, 63.37);
        assert_eq!(seek_of(&cmds, DeckId::A), Some(63.37));
    }

    #[test]
    fn a_snapped_seek_without_a_grid_is_exact() {
        let mut e = DeckEngine::new();
        load_unanalysed(&mut e, DeckId::A, 1);
        e.set_snap_beats(DeckId::A, 4);
        e.observe(DeckId::A, 10.2, true);
        let cmds = e.seek_secs_snapped(DeckId::A, 63.37);
        assert_eq!(seek_of(&cmds, DeckId::A), Some(63.37));
    }

    #[test]
    fn a_snapped_seek_still_clamps_and_mirrors_like_a_plain_one() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // 300 s long
        e.set_snap_beats(DeckId::A, 4);
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
        e.set_snap_beats(DeckId::B, 4);
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
        e.set_snap_beats(DeckId::A, 4);
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
        e.set_snap_beats(DeckId::A, 0); // exact: the refusal is under test
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
        e.deck_mut(DeckId::A).loop_ticks = 128;
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
        assert!(cmds.contains(&DeckCmd::SetLoopSpan { deck: DeckId::A, span: None , seek: LoopSeek::None }));
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
        e.set_snap_beats(DeckId::A, 0);
        e.set_cue(DeckId::A, 10.3);
        assert!((e.deck(DeckId::A).cue_secs - 10.3).abs() < 1e-9);
        e.set_cue(DeckId::A, 10_000.0);
        assert!((e.deck(DeckId::A).cue_secs - 300.0).abs() < 1e-9);
        // QUANT on: whole units against the cue's own phase, the loop-drag law.
        e.set_cue(DeckId::A, 10.3);
        e.set_snap_beats(DeckId::A, 4);
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
        e.deck_mut(DeckId::A).loop_ticks = LOOP_LADDER[LOOP_LADDER.len() - 1].0;
        e.loop_double(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_ticks, LOOP_BEATS_INF, "the top rung doubles into ∞");
        e.loop_double(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_ticks, LOOP_BEATS_INF, "∞ is the top");
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
        assert!((slot.span.start_secs - 10.2).abs() < 1e-9 && slot.span.len_secs() < 1e-9);
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
        e.deck_mut(DeckId::A).loop_ticks = LOOP_BEATS_INF;
        e.observe(DeckId::A, 10.2, true);
        e.loop_in(DeckId::A); // the current bookmark
        // Dial down: the out point returns the top rung's length later and
        // loop mode becomes active — the playhead is outside, so it jumps
        // in. Past the whole span, because the top rung is long.
        e.observe(DeckId::A, 280.0, true);
        let cmds = e.loop_halve(DeckId::A);
        let top = LOOP_LADDER[LOOP_LADDER.len() - 1].0;
        assert_eq!(e.deck(DeckId::A).loop_ticks, top);
        let span = e.deck(DeckId::A).loop_span.expect("loop mode is back");
        assert!((span.start_secs - 10.2).abs() < 1e-9);
        let want = 0.5 * top as f64 / LOOP_TICKS_PER_BEAT as f64;
        assert!((span.len_secs() - want).abs() < 1e-9, "the top rung at 120 BPM");
        assert!(e.deck(DeckId::A).bookmark.is_none(), "the bookmark became the loop");
        assert_eq!(seek_of(&cmds, DeckId::A), Some(10.2));
        // Dial back up to ∞: loop mode exits, the out point is gone, and
        // the current object is a bookmark at the same IN again.
        let cmds = e.loop_double(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_ticks, LOOP_BEATS_INF);
        assert!(e.deck(DeckId::A).loop_span.is_none(), "no out point, no loop");
        assert_eq!(e.deck(DeckId::A).bookmark, Some(10.2));
        assert!(cmds.contains(&DeckCmd::SetLoopSpan { deck: DeckId::A, span: None , seek: LoopSeek::None }));
        // And a plain 4-beat loop dialed all the way up the ladder
        // collapses too, whatever the ladder's length.
        e.deck_mut(DeckId::A).loop_ticks = 4 * LOOP_TICKS_PER_BEAT;
        e.observe(DeckId::A, 20.0, true);
        e.loop_in(DeckId::A);
        assert!(e.deck(DeckId::A).bookmark.is_none(), "engaging clears the bookmark");
        let rungs_above_four =
            LOOP_LADDER.len() - loop_rung_index(4 * LOOP_TICKS_PER_BEAT).expect("a rung");
        for _ in 0..rungs_above_four {
            e.loop_double(DeckId::A);
        }
        assert!(e.deck(DeckId::A).loop_span.is_none());
        assert_eq!(e.deck(DeckId::A).bookmark, Some(20.0));
    }

    #[test]
    fn picking_a_count_resizes_in_place_and_converts_at_the_ends() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_ticks = 128;
        e.observe(DeckId::A, 10.0, true);
        e.loop_in(DeckId::A); // 10.0 .. 12.0
        assert!(e.save_loop(DeckId::A));
        // Pick 16: the running loop resizes to exactly 16 beats from IN,
        // marker following like the cutter.
        e.set_loop_beats(DeckId::A, 512);
        let span = e.deck(DeckId::A).loop_span.expect("still looping");
        assert!((span.len_secs() - 8.0).abs() < 1e-9);
        assert!((e.deck(DeckId::A).loop_slots[0].span.len_secs() - 8.0).abs() < 1e-9);
        // Pick the infinity count: collapse to a bookmark; pick 8: the out
        // point returns at the picked length.
        e.set_loop_beats(DeckId::A, LOOP_BEATS_INF);
        assert!(e.deck(DeckId::A).loop_span.is_none());
        assert_eq!(e.deck(DeckId::A).bookmark, Some(10.0));
        e.set_loop_beats(DeckId::A, 256);
        let span = e.deck(DeckId::A).loop_span.expect("loop mode is back");
        assert!((span.len_secs() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn clearing_marks_takes_the_bookmark_too_and_a_snapshot_puts_them_back() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_ticks = 128;
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
            .map(|i| LoopSlot {
                slot: i as u16,
                span: LoopSpan { start_secs: i as f64, end_secs: i as f64 }, kind: SlotKind::Loop, colour: 0 })
            .collect();
        e.restore_marks(DeckId::A, many, None);
        assert_eq!(e.deck(DeckId::A).loop_slots.len(), LOOP_SLOT_CAP);
        assert!(e.deck(DeckId::A).bookmark.is_none());
    }

    #[test]
    fn resizing_a_saved_loop_updates_its_marker() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_ticks = 128;
        e.observe(DeckId::A, 10.0, true);
        e.loop_in(DeckId::A); // 10.0 .. 12.0
        assert!(e.save_loop(DeckId::A));
        // Halve the running loop: the marker's stored span follows.
        e.loop_halve(DeckId::A);
        let slot = e.deck(DeckId::A).loop_slots[0];
        assert!((slot.span.start_secs - 10.0).abs() < 1e-9, "the IN holds");
        assert!((slot.span.len_secs() - 1.0).abs() < 1e-9, "the duration followed");
        // A resize of an UNSAVED loop touches no markers.
        e.delete_loop_slot(DeckId::A, 0);
        e.loop_double(DeckId::A);
        assert!(e.deck(DeckId::A).loop_slots.is_empty());
    }

    #[test]
    fn saved_loops_die_with_the_track_and_respect_the_cap() {
        let mut e = DeckEngine::new();
        // Loading over a running deck: the default policy would refuse it,
        // and what this test is about is what a load DOES.
        e.over_playing = OverPlaying::Stop;
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_ticks = 32;
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

    /// The off row is the hand claiming the playhead: the tempo match and
    /// the rate trim stay, so the decks still run together, and only the
    /// LANDING is withheld. That is the manual beatmatch, and it is why
    /// this is not a second spelling of SYNC off.
    #[test]
    fn quant_off_withholds_the_landing_but_never_the_tempo_match() {
        let rig = |unit: u32| {
            let mut e = DeckEngine::new();
            load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
            load_analysed(&mut e, DeckId::B, 2, 120.0, 0.19);
            e.set_snap_beats(DeckId::B, unit);
            e.play_pause(DeckId::A);
            e.observe(DeckId::A, 30.0, true);
            e.play_pause(DeckId::B);
            e.observe(DeckId::B, 30.0, true);
            e.apply_auto_sync();
            assert_eq!(e.sync_master(), Some(DeckId::A));
            // The tempo is matched whatever the unit says.
            let matched = e.deck(DeckId::B).rate;
            assert!((matched - 128.0 / 120.0).abs() < 1e-9, "rate {matched}");
            // Now knock the follower out of phase under the lock.
            e.observe(DeckId::B, e.deck(DeckId::B).position_secs + 0.17, true);
            let cmds = e.apply_auto_sync();
            (e, cmds)
        };
        let (off, cmds) = rig(0);
        assert!(seek_of(&cmds, DeckId::B).is_none(), "never moved: {cmds:?}");
        // The engine's own mirror stays where the deck is, or the next
        // observation would fight it.
        let at = off.deck(DeckId::B).position_secs;
        assert!(seek_of(&cmds, DeckId::B).is_none() && at > 30.0, "at {at}");

        let (_, cmds) = rig(1);
        assert!(seek_of(&cmds, DeckId::B).is_some(), "with a unit it lands: {cmds:?}");
    }

    /// A landing someone PRESSED for is not a landing nobody asked for.
    #[test]
    fn a_pressed_sync_lands_a_quant_off_deck_because_someone_asked() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 120.0, 0.19);
        e.play_pause(DeckId::A);
        e.play_pause(DeckId::B);
        e.observe(DeckId::A, 30.0, true);
        e.observe(DeckId::B, 30.0, true);
        e.set_snap_beats(DeckId::B, 0);
        // The standing lock declines, on a rig where nothing has landed yet.
        let cmds = e.apply_auto_sync();
        assert!(seek_of(&cmds, DeckId::B).is_none(), "{cmds:?}");
        assert!(phase_gap(&e) > 1e-3, "still out of phase");

        // The button lands it anyway.
        let cmds = e.sync(DeckId::B, true);
        assert!(seek_of(&cmds, DeckId::B).is_some(), "a press lands: {cmds:?}");
    }

    /// The off row now withholds the phase landing, so a tab that shipped
    /// with it off would tempo-match two decks and never put them in step.
    #[test]
    fn a_fresh_deck_starts_on_a_unit_rather_than_off() {
        let e = DeckEngine::new();
        assert_eq!(e.snap_beats(DeckId::A), SNAP_DEFAULT_BEATS);
        assert_eq!(e.snap_beats(DeckId::B), SNAP_DEFAULT_BEATS);
        assert!(SNAP_DEFAULT_BEATS > 0, "or auto sync never lands");
    }

    /// A mark read back off disk is a HAND'S placement, and the unit that
    /// happens to be on now has no business rounding it.
    #[test]
    fn a_restored_cue_is_never_rounded_by_the_unit() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // beats every 0.5 s
        e.set_snap_beats(DeckId::A, 4);
        e.restore_cue(DeckId::A, 30.19);
        assert!((e.deck(DeckId::A).cue_secs - 30.19).abs() < 1e-9, "exact");
        // And the placement is remembered, so the analysis cannot overwrite it.
        assert!(e.deck(DeckId::A).cue_placed);
    }

    /// The level factor the channel strip forms, off the intent the engine
    /// already holds -- and nothing else: the transport is ranked
    /// separately, and the tone chain is somebody else's answer.
    #[test]
    fn the_strip_gain_is_the_level_the_channel_is_set_to() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 120.0, 0.0);
        e.set_crossfader(0.0); // hard over to A
        assert!((e.deck_strip_gain(DeckId::A) - 1.0).abs() < 1e-4);
        assert!(e.deck_strip_gain(DeckId::B) <= AUDIBLE_FLOOR, "B is out");

        e.set_crossfader(0.5);
        for deck in [DeckId::A, DeckId::B] {
            let at = e.deck_strip_gain(deck);
            assert!((at - 0.7071).abs() < 1e-3, "{deck:?} at {at}");
        }

        // The channel fader multiplies in.
        e.set_gain(DeckId::A, 0.5);
        assert!((e.deck_strip_gain(DeckId::A) - 0.3536).abs() < 1e-3);

        // Mute is silence whatever the faders say.
        e.toggle_mute(DeckId::A);
        assert_eq!(e.deck_strip_gain(DeckId::A), 0.0);
        e.toggle_mute(DeckId::A);

        // And an empty deck is silent by definition.
        assert_eq!(DeckEngine::new().deck_strip_gain(DeckId::A), 0.0);
    }

    /// A record the room cannot hear has stopped leading, whatever it is
    /// doing. Fading it out by hand hands the group over on its own.
    #[test]
    fn a_faded_out_master_hands_the_pin_to_the_deck_the_room_can_hear() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 120.0, 0.0);
        e.play_pause(DeckId::A);
        e.play_pause(DeckId::B);
        e.observe(DeckId::A, 10.0, true);
        e.observe(DeckId::B, 10.0, true);
        e.apply_auto_sync();
        assert_eq!(e.sync_master(), Some(DeckId::A));

        // Halfway across, both records are up: the pin does not move.
        e.set_crossfader(0.5);
        e.hold_deck_sync();
        assert_eq!(e.sync_master(), Some(DeckId::A), "the pin is the hysteresis");

        // All the way: A is out of the room, so it stops leading it.
        e.set_crossfader(1.0);
        e.hold_deck_sync();
        assert_eq!(e.sync_master(), Some(DeckId::B));
    }

    /// The same law, reached by the other control.
    #[test]
    fn a_muted_master_stops_leading_too() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 120.0, 0.0);
        e.play_pause(DeckId::A);
        e.play_pause(DeckId::B);
        e.observe(DeckId::A, 10.0, true);
        e.observe(DeckId::B, 10.0, true);
        e.set_crossfader(0.5);
        e.apply_auto_sync();
        assert_eq!(e.sync_master(), Some(DeckId::A));

        e.toggle_mute(DeckId::A);
        e.hold_deck_sync();
        assert_eq!(e.sync_master(), Some(DeckId::B), "a muted deck leads nothing");
    }

    /// Nothing audible moves nothing: a silent group keeps the reference it
    /// had, rather than handing the pin round a room hearing neither deck.
    #[test]
    fn a_silent_group_keeps_the_reference_it_had() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 120.0, 0.0);
        e.play_pause(DeckId::A);
        e.play_pause(DeckId::B);
        e.observe(DeckId::A, 10.0, true);
        e.observe(DeckId::B, 10.0, true);
        e.apply_auto_sync();
        assert_eq!(e.sync_master(), Some(DeckId::A));

        e.toggle_mute(DeckId::A);
        e.toggle_mute(DeckId::B);
        e.hold_deck_sync();
        assert_eq!(e.sync_master(), Some(DeckId::A), "nowhere better to put it");
    }

    /// With no pin standing, the unpinned chain ranks by audibility -- and
    /// still never lets a stopped deck lead a playing one.
    #[test]
    fn the_unpinned_election_ranks_by_audibility() {
        let mut e = DeckEngine::new();
        e.set_auto_sync(false);
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 120.0, 0.0);
        assert_eq!(e.sync_master(), None, "nothing pinned");

        e.play_pause(DeckId::A);
        e.observe(DeckId::A, 10.0, true);
        e.set_crossfader(1.0); // A is playing but out of the room
        assert_eq!(e.sync_leader(), Some(DeckId::A), "still the only record running");

        e.play_pause(DeckId::B);
        e.observe(DeckId::B, 10.0, true);
        assert_eq!(e.sync_leader(), Some(DeckId::B), "and now one of them is heard");

        // A stopped deck never leads a playing one, however loud it is.
        e.play_pause(DeckId::B);
        e.set_crossfader(0.0);
        assert_eq!(e.sync_leader(), Some(DeckId::A));
    }

    /// A deck with a grid, playing forward, counts the bar it is in.
    #[test]
    fn a_deck_counts_the_bar_it_is_in() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // beats every 0.5 s
        // Two and a quarter beats in: the third count of the bar, a
        // quarter of the way through it.
        let pulse = e.deck(DeckId::A).pulse_at(1.125, 1.0).expect("a pulse");
        assert_eq!(pulse.span_beats, 4);
        assert_eq!(pulse.index, 2);
        assert!((pulse.phase - 0.25).abs() < 1e-9, "{}", pulse.phase);
        assert!(pulse.measured);
        // An empty deck has nothing to count.
        assert!(DeckEngine::new().deck(DeckId::A).pulse_at(1.0, 1.0).is_none());
    }

    /// A record nothing has measured still counts: a beat is a second, and
    /// the pulse says so rather than pretending.
    #[test]
    fn an_unmeasured_record_counts_seconds_and_says_so() {
        let mut e = DeckEngine::new();
        load_unanalysed(&mut e, DeckId::A, 1);
        let pulse = e.deck(DeckId::A).pulse_at(2.5, 1.0).expect("a pulse");
        assert_eq!(pulse.span_beats, 4);
        assert_eq!(pulse.index, 2);
        assert!((pulse.phase - 0.5).abs() < 1e-9);
        assert!(!pulse.measured, "and the LED burns lower for it");
    }

    /// A loop is the lap, not the bar: the count anchors on the loop's IN,
    /// because that is what the wrap is modulo.
    #[test]
    fn a_running_loop_counts_its_own_length_from_its_own_in() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_span = Some(LoopSpan { start_secs: 10.1, end_secs: 11.1 });
        // Two beats long, anchored a tenth of a second off the grid.
        let pulse = e.deck(DeckId::A).pulse_at(10.85, 1.0).expect("a pulse");
        assert_eq!(pulse.span_beats, 2);
        assert_eq!(pulse.index, 1);
        assert!((pulse.phase - 0.5).abs() < 1e-9, "{}", pulse.phase);
    }

    /// Every rung below a beat rounds to no beats at all, so the whole lap
    /// is the count.
    #[test]
    fn a_sub_beat_loop_counts_one_pulse_a_lap() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.deck_mut(DeckId::A).loop_span = Some(LoopSpan { start_secs: 10.0, end_secs: 10.125 });
        let pulse = e.deck(DeckId::A).pulse_at(10.09375, 1.0).expect("a pulse");
        assert_eq!(pulse.span_beats, 1);
        assert_eq!(pulse.index, 0);
        assert!((pulse.phase - 0.75).abs() < 1e-9, "{}", pulse.phase);
    }

    /// Backwards, the record crosses each count from above: the count just
    /// passed is the one ahead, and the phase runs the other way.
    #[test]
    fn a_record_running_backwards_counts_down() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        let forward = e.deck(DeckId::A).pulse_at(1.125, 1.0).expect("a pulse");
        let back = e.deck(DeckId::A).pulse_at(1.125, -1.6).expect("a pulse");
        assert_eq!(forward.index, 2);
        assert_eq!(back.index, 3, "the count just crossed is the one ahead");
        assert!((back.phase - 0.75).abs() < 1e-9, "{}", back.phase);
        assert_eq!(back.travel, -1.6, "carried unclamped: a spin-back is fast");

        // Exactly ON a count, backwards, is that count: the record has
        // just arrived at it from above, so the mirrored phase wraps to
        // zero rather than reaching one.
        let on = e.deck(DeckId::A).pulse_at(1.0, -1.0).expect("a pulse");
        assert_eq!(on.phase, 0.0);
        assert_eq!(on.index, 2);
    }

    /// A record played by people: the tempo the lock and the readout
    /// answer with is the one AROUND the playhead, not the whole
    /// record's average.
    #[test]
    fn a_deck_with_a_moving_tempo_answers_at_the_playhead() {
        use crate::wave_analysis::{TempoMap, TempoSegment};
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        // Four segments walking 120 to 132 over sixteen beats each.
        let mut segments = Vec::new();
        let (mut at, mut beat) = (0.0, 0.0);
        for step in 0..4 {
            let bpm = 120.0 + 4.0 * step as f64;
            let period = 60.0 / bpm;
            segments.push(TempoSegment { start_secs: at, start_beat: beat, period_secs: period });
            at += period * 16.0;
            beat += 16.0;
        }
        let map = std::sync::Arc::new(TempoMap { segments });
        let gen = e.deck(DeckId::A).load_gen;
        e.grid_ready(DeckId::A, gen, grid(120.0, 0.0), None, Some(map.clone()));

        e.observe(DeckId::A, 1.0, true);
        let early = e.deck(DeckId::A).effective_bpm().expect("a tempo");
        e.observe(DeckId::A, 28.0, true);
        let late = e.deck(DeckId::A).effective_bpm().expect("a tempo");
        assert!((early - 120.0).abs() < 0.2, "early {early}");
        assert!(late > early + 5.0, "the record sped up: {early} -> {late}");
        // And the deck's answer IS the map's window, not something new.
        let want = map.local_bpm(28.0).expect("a tempo");
        assert!((late - want).abs() < 1e-9, "{late} vs {want}");
    }

    /// Nearly every record: one straight line describes it, there is no
    /// map, and the whole tab answers exactly what it always did.
    #[test]
    fn a_record_with_no_moving_tempo_is_the_published_line_exactly() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        e.observe(DeckId::A, 61.35, true);
        let state = e.deck(DeckId::A);
        assert_eq!(state.local_grid(), state.true_grid());
        assert_eq!(state.sync_view().map(|view| view.grid), state.true_grid());
        assert_eq!(state.effective_bpm(), Some(128.0));
    }

    /// Two locked decks, both playing and in phase, A leading.
    fn locked_pair() -> DeckEngine {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 120.0, 0.0);
        e.play_pause(DeckId::A);
        e.play_pause(DeckId::B);
        e.observe(DeckId::A, 30.0, true);
        e.observe(DeckId::B, 30.0, true);
        e.apply_auto_sync();
        assert_eq!(e.sync_master(), Some(DeckId::A));
        e
    }

    /// A bend is how a hand says "sit a little ahead of the beat". Letting
    /// go used to throw that away: the servo pulled the deck straight back
    /// to dead phase. The offset is kept now.
    #[test]
    fn a_deliberate_nudge_becomes_the_phase_the_lock_holds() {
        let mut e = locked_pair();
        // The hand pushes the deck a sixth of a beat ahead and lets go.
        e.hold_bend(DeckId::B, 1.0, false);
        e.observe(DeckId::B, 30.0 + 1.0 / 12.0, true);
        e.release_bend(DeckId::B);
        let kept = e.deck(DeckId::B).phase_offset_beats;
        assert!((kept - 1.0 / 6.0).abs() < 1e-6, "kept {kept}");

        // And the servo now holds it THERE rather than pulling it back:
        // pumped from that very position it asks for nothing.
        let cmds = e.hold_deck_sync();
        assert!(seek_of(&cmds, DeckId::B).is_none(), "{cmds:?}");
        assert!(
            rate_of(&cmds, DeckId::B).is_none_or(|rate| (rate - 1.0).abs() < 1e-3),
            "{cmds:?}"
        );
    }

    /// The offset is a property of how the operator wants the decks to
    /// sit, so it survives a seek and the lock being let go and taken
    /// again -- and it is what a landing lands on.
    #[test]
    fn a_kept_nudge_survives_a_seek_and_the_lock_being_toggled() {
        let mut e = locked_pair();
        e.deck_mut(DeckId::B).phase_offset_beats = 0.25;

        // A seek re-locks, and lands a quarter beat ahead of dead phase.
        e.seek_secs(DeckId::B, 60.13);
        let landed = e.deck(DeckId::B).position_secs;
        let phase_a = e.deck(DeckId::A).grid.unwrap().beat_at(30.0).fract();
        let phase_b = e.deck(DeckId::B).grid.unwrap().beat_at(landed).fract();
        let gap = (phase_b - phase_a).rem_euclid(1.0);
        assert!((gap - 0.25).abs() < 1e-6, "landed at {gap} of a beat");

        // Off and on again keeps it: the hand chose that offset, and only
        // a press that means "put this deck in step" clears it.
        e.toggle_sync(DeckId::B);
        e.toggle_sync(DeckId::B);
        assert!((e.deck(DeckId::B).phase_offset_beats - 0.25).abs() < 1e-9);
    }

    /// The gesture that clears it is the one whose whole meaning is "in
    /// step, now".
    #[test]
    fn a_one_shot_match_puts_a_nudged_deck_back_in_step() {
        let mut e = locked_pair();
        e.deck_mut(DeckId::B).phase_offset_beats = 0.25;
        e.sync_verb(DeckId::B, SyncVerb::Tempo);
        assert!(
            (e.deck(DeckId::B).phase_offset_beats - 0.25).abs() < 1e-9,
            "the tempo half is not a phase press"
        );
        e.sync_verb(DeckId::B, SyncVerb::Match);
        assert_eq!(e.deck(DeckId::B).phase_offset_beats, 0.0);

        // And a record leaving the deck takes it with it.
        e.deck_mut(DeckId::B).phase_offset_beats = 0.25;
        e.eject(DeckId::B);
        assert_eq!(e.deck(DeckId::B).phase_offset_beats, 0.0);
    }

    /// The mixer's clock reads the grid the engine holds, so every path
    /// that writes one sends it -- and a landing that wrote nothing,
    /// because a hand had placed or locked the grid, sends nothing.
    #[test]
    fn every_grid_the_engine_writes_is_sent_to_the_mixer() {
        fn sent(cmds: &[DeckCmd]) -> Vec<Option<f64>> {
            cmds.iter()
                .filter_map(|cmd| match cmd {
                    DeckCmd::SetGrid { deck: DeckId::A, grid } => Some(grid.map(|g| g.bpm)),
                    _ => None,
                })
                .collect()
        }
        let mut e = DeckEngine::new();
        let (deck, gen) = load_gen(&e.click(item(1), DeckTarget::A));
        e.track_ready(deck, gen, 300.0);
        // The analysis landing.
        let cmds = e.grid_ready(deck, gen, grid(120.0, 0.0), None, None);
        assert_eq!(sent(&cmds), vec![Some(120.0)]);
        // A hand's correction, in each of its forms.
        e.observe(DeckId::A, 30.08, true);
        let (_, cmds) = e.edit_grid(DeckId::A, GridEdit::Scale(2.0)).unwrap();
        assert_eq!(sent(&cmds), vec![Some(240.0)]);
        let (_, cmds) = e.tap_grid(DeckId::A, 128.0, 30.0).unwrap();
        assert_eq!(sent(&cmds), vec![Some(128.0)]);
        let (_, cmds) = e.undo_grid(DeckId::A).unwrap();
        assert_eq!(sent(&cmds), vec![Some(240.0)]);
        let (_, cmds) = e.flip_beat_phase(DeckId::A).unwrap();
        assert_eq!(sent(&cmds), vec![Some(240.0)]);
        // A grid remembered from a previous night.
        let cmds = e.restore_grid(DeckId::A, grid(96.0, 0.25), true);
        assert_eq!(sent(&cmds), vec![Some(96.0)]);
        assert!(e.restore_grid(DeckId::A, TrackGrid::default(), false).is_empty());
        // And the analysis landing on a placed, locked grid writes
        // nothing, so it sends nothing: the mixer keeps the hand's.
        let cmds = e.grid_ready(deck, gen, grid(120.0, 0.0), None, None);
        assert!(sent(&cmds).is_empty(), "{cmds:?}");
        assert_eq!(e.deck(DeckId::A).grid.unwrap().bpm, 96.0);
    }

    /// The analyser rules a grid; a hand corrects it. The nearest ruling
    /// moves ONTO the playhead and every other ruling comes with it.
    #[test]
    fn a_grid_can_be_pulled_onto_the_playhead() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // rulings on the half second
        e.observe(DeckId::A, 30.08, true); // 80 ms late
        let (grid, _) = e.edit_grid(DeckId::A, GridEdit::Adjust).expect("an edit");
        assert!((grid.bpm - 120.0).abs() < 1e-9, "the tempo is not touched");
        assert!(
            (grid.beat_at(30.08).fract()).abs() < 1e-9,
            "the playhead is on a ruling: {}",
            grid.beat_at(30.08)
        );
        assert!(e.deck(DeckId::A).grid_placed, "a hand put it there");
    }

    /// Which ruling is the ONE is a separate question from where the
    /// rulings are.
    #[test]
    fn the_beat_under_the_playhead_can_be_made_the_one() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        // Beat 61 (30.5 s) is the third of its bar; make it the first.
        e.observe(DeckId::A, 30.5, true);
        let before = e.deck(DeckId::A).grid.unwrap();
        assert!(!before.is_downbeat(61));
        let (grid, _) = e.edit_grid(DeckId::A, GridEdit::Downbeat).expect("an edit");
        assert!(grid.is_downbeat(61), "beat 61 is the one now");
        assert!((grid.first_beat_secs - before.first_beat_secs).abs() < 1e-9, "nothing moved");
        assert!((grid.bpm - before.bpm).abs() < 1e-9);
    }

    /// The analyser's other known failure is the octave. Scaling hinges on
    /// the playhead, so the beat the operator is listening to stays put.
    #[test]
    fn a_grid_can_be_doubled_halved_and_pulled_by_a_third() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.observe(DeckId::A, 30.0, true);
        let (grid, _) = e.edit_grid(DeckId::A, GridEdit::Scale(2.0)).expect("an edit");
        assert!((grid.bpm - 240.0).abs() < 1e-9);
        assert!(grid.beat_at(30.0).fract().abs() < 1e-9, "hinged where the record is");

        let (grid, _) = e.edit_grid(DeckId::A, GridEdit::Scale(0.5)).expect("an edit");
        assert!((grid.bpm - 120.0).abs() < 1e-9, "and back");

        let (grid, _) = e.edit_grid(DeckId::A, GridEdit::Scale(2.0 / 3.0)).expect("an edit");
        assert!((grid.bpm - 80.0).abs() < 1e-9);

        // A tempo that would leave the plausible band is refused rather
        // than published: nothing downstream can use it.
        assert!(e.edit_grid(DeckId::A, GridEdit::Scale(0.125)).is_none());
    }

    /// A hand's grid is not the analysis's to overwrite -- the same law
    /// the cue and the shape beside it follow.
    #[test]
    fn an_edited_grid_survives_the_analysis_landing_late() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.observe(DeckId::A, 30.0, true);
        e.edit_grid(DeckId::A, GridEdit::Scale(2.0));
        let gen = e.deck(DeckId::A).load_gen;
        e.grid_ready(DeckId::A, gen, grid(120.0, 0.0), None, None);
        assert!((e.deck(DeckId::A).grid.unwrap().bpm - 240.0).abs() < 1e-9, "kept");

        // And a record leaving the deck takes the placement with it.
        e.eject(DeckId::A);
        assert!(!e.deck(DeckId::A).grid_placed);
    }

    /// A correction can be taken back, and a RUN of the same correction is
    /// one step: four presses of the double button is one decision.
    #[test]
    fn grid_corrections_undo_and_a_run_of_one_kind_is_a_single_step() {
        let mut e = DeckEngine::new();
        // Seventy, so two doublings still land inside the band a hand may
        // publish.
        load_analysed(&mut e, DeckId::A, 1, 70.0, 0.0);
        e.observe(DeckId::A, 30.0, true);
        assert!(!e.can_undo_grid(DeckId::A), "nothing to take back yet");

        e.edit_grid(DeckId::A, GridEdit::Scale(2.0));
        e.edit_grid(DeckId::A, GridEdit::Scale(2.0));
        assert!((e.deck(DeckId::A).grid.unwrap().bpm - 280.0).abs() < 1e-9);
        assert!(e.can_undo_grid(DeckId::A));

        // One step back for both presses, straight to where the analyser
        // had it.
        e.undo_grid(DeckId::A);
        assert!((e.deck(DeckId::A).grid.unwrap().bpm - 70.0).abs() < 1e-9);
        assert!(!e.can_undo_grid(DeckId::A), "and the stack is empty again");

        // A DIFFERENT correction is its own step.
        e.edit_grid(DeckId::A, GridEdit::Scale(2.0));
        e.edit_grid(DeckId::A, GridEdit::Downbeat);
        e.undo_grid(DeckId::A);
        assert!((e.deck(DeckId::A).grid.unwrap().bpm - 140.0).abs() < 1e-9, "only the downbeat");
        e.undo_grid(DeckId::A);
        assert!((e.deck(DeckId::A).grid.unwrap().bpm - 70.0).abs() < 1e-9);
    }

    /// A grid that is right is worth protecting from the next mis-click.
    #[test]
    fn a_locked_grid_refuses_every_correction_and_the_analysis_too() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.observe(DeckId::A, 30.0, true);
        e.set_grid_locked(DeckId::A, true);

        assert!(e.edit_grid(DeckId::A, GridEdit::Scale(2.0)).is_none());
        assert!(e.flip_beat_phase(DeckId::A).is_none());
        assert!((e.deck(DeckId::A).grid.unwrap().bpm - 120.0).abs() < 1e-9);

        let gen = e.deck(DeckId::A).load_gen;
        e.grid_ready(DeckId::A, gen, grid(96.0, 0.0), None, None);
        assert!((e.deck(DeckId::A).grid.unwrap().bpm - 120.0).abs() < 1e-9, "not the analysis either");

        // Unlocked, the corrections land again.
        e.set_grid_locked(DeckId::A, false);
        assert!(e.edit_grid(DeckId::A, GridEdit::Scale(2.0)).is_some());
    }

    /// Tapping along with a record retunes its grid: the tapped tempo is
    /// the record's, and the tapped beat is the one.
    #[test]
    fn tapping_along_retunes_the_record_and_puts_the_one_where_the_hand_did() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.observe(DeckId::A, 30.0, true);
        let (grid, _) = e.tap_grid(DeckId::A, 128.0, 30.37).expect("a grid");
        assert!((grid.bpm - 128.0).abs() < 1e-9);
        // The tap is a ruling, and it is the one.
        let beat = grid.beat_at(30.37);
        assert!(beat.fract().abs() < 1e-9, "the tap is on a beat: {beat}");
        assert!(grid.is_downbeat(beat.round() as i64), "and it is the one");
        assert!(e.deck(DeckId::A).grid_placed);

        // A deck running fast measures a fast tempo; the RECORD's is what
        // gets written down.
        e.set_pitch(DeckId::A, 0.5); // +4% on the default range
        let rate = e.deck(DeckId::A).rate;
        let (grid, _) = e.tap_grid(DeckId::A, 128.0 * rate, 30.0).expect("a grid");
        assert!((grid.bpm - 128.0).abs() < 1e-9, "{} at rate {rate}", grid.bpm);

        // A run of taps is one step back, like a run of any other button.
        e.undo_grid(DeckId::A);
        assert!((e.deck(DeckId::A).grid.unwrap().bpm - 120.0).abs() < 1e-9);
        assert!(!e.can_undo_grid(DeckId::A));

        // And a locked grid refuses the tap too.
        e.set_grid_locked(DeckId::A, true);
        assert!(e.tap_grid(DeckId::A, 128.0, 30.0).is_none());
    }

    /// What the background pass asks before it starts work: is either
    /// deck waiting on a record?
    #[test]
    fn a_deck_is_loading_until_its_grid_lands() {
        let mut e = DeckEngine::new();
        let loading = |e: &DeckEngine| {
            [DeckId::A, DeckId::B].into_iter().any(|deck| {
                let state = e.deck(deck);
                matches!(state.load, DeckLoad::Loading { .. })
                    || (state.is_loaded() && state.grid.is_none())
            })
        };
        assert!(!loading(&e), "two empty decks are waiting for nothing");

        let (deck, gen) = load_gen(&e.click(item(1), DeckTarget::A));
        assert!(loading(&e), "the decode is in flight");
        e.track_ready(deck, gen, 200.0);
        assert!(loading(&e), "and the grid is not here yet");
        e.grid_ready(deck, gen, grid(120.0, 0.0), None, None);
        assert!(!loading(&e), "now it is");
    }

    #[test]
    fn each_deck_owns_its_own_snap_unit() {
        let mut e = DeckEngine::new();
        e.set_snap_beats(DeckId::A, 8);
        assert_eq!(e.snap_beats(DeckId::A), 8);
        assert_eq!(e.snap_beats(DeckId::B), SNAP_DEFAULT_BEATS, "and only its own");
        // There is no per-deck unit to disagree with it.
        e.set_snap_beats(DeckId::B, 4);
        assert_eq!(e.snap_beats(DeckId::A), 8, "still A's");
        e.set_snap_beats(DeckId::A, 0);
        assert_eq!(e.snap_beats(DeckId::A), 0);
        assert_eq!(e.snap_beats(DeckId::B), 4);
    }

    #[test]
    fn swap_carries_the_whole_loop_state() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 128.0, 0.0);
        e.deck_mut(DeckId::A).loop_ticks = 512;
        e.deck_mut(DeckId::A).loop_span = Some(LoopSpan { start_secs: 8.0, end_secs: 10.0 });
        e.deck_mut(DeckId::B).loop_ticks = 64;
        e.swap();
        assert_eq!(e.deck(DeckId::B).loop_ticks, 512);
        assert_eq!(
            e.deck(DeckId::B).loop_span,
            Some(LoopSpan { start_secs: 8.0, end_secs: 10.0 })
        );
        assert_eq!(e.deck(DeckId::A).loop_ticks, 64);
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
        engine.grid_ready(deck, gen, grid(bpm, first_beat_secs), None, None);
    }

    // ---- momentary pitch bend -------------------------------------------

    #[test]
    fn a_bend_moves_the_rate_without_moving_the_fader() {
        let mut decks = DeckEngine::new();
        let before = decks.deck(DeckId::A).pitch;
        let cmds = decks.hold_bend(DeckId::A, 1.0, false);
        assert_eq!(rate_of(&cmds, DeckId::A), Some(1.0 + BEND_COARSE));
        assert_eq!(decks.deck(DeckId::A).pitch, before, "the fader has not moved");
    }

    #[test]
    fn releasing_a_bend_puts_the_rate_back_exactly() {
        let mut decks = DeckEngine::new();
        decks.hold_bend(DeckId::A, -1.0, false);
        let cmds = decks.release_bend(DeckId::A);
        assert_eq!(rate_of(&cmds, DeckId::A), Some(1.0), "back to the track's own tempo");
        assert_eq!(decks.deck(DeckId::A).bend, 0.0);
    }

    #[test]
    fn a_fine_bend_is_smaller_than_a_coarse_one() {
        let mut decks = DeckEngine::new();
        let coarse = rate_of(&decks.hold_bend(DeckId::A, 1.0, false), DeckId::A).unwrap();
        decks.release_bend(DeckId::A);
        let fine = rate_of(&decks.hold_bend(DeckId::A, 1.0, true), DeckId::A).unwrap();
        assert!(fine > 1.0 && fine < coarse, "fine {fine} coarse {coarse}");
    }

    #[test]
    fn a_bend_does_not_drop_a_follower_out_of_sync() {
        // This is the whole reason a bend is not a pitch move: `set_pitch`
        // opts a follower out of the lock, and nudging a deck back into
        // place must not cost it its sync.
        let mut decks = DeckEngine::new();
        decks.deck_mut(DeckId::B).synced = true;
        decks.deck_mut(DeckId::B).auto_opt_out = false;
        decks.hold_bend(DeckId::B, 1.0, false);
        assert!(decks.deck(DeckId::B).synced, "still locked");
        assert!(!decks.deck(DeckId::B).auto_opt_out, "and not opted out");
    }

    #[test]
    fn a_bend_never_drives_the_deck_backwards() {
        let mut decks = DeckEngine::new();
        decks.set_pitch(DeckId::A, -1.0);
        for _ in 0..40 {
            decks.hold_bend(DeckId::A, -1.0, false);
        }
        let rate = rate_of(&decks.hold_bend(DeckId::A, -1.0, false), DeckId::A).unwrap();
        assert!(rate >= RATE_MIN, "a bend must never reverse the record: {rate}");
    }

    #[test]
    fn a_held_bend_survives_a_pitch_move_underneath_it() {
        let mut decks = DeckEngine::new();
        decks.hold_bend(DeckId::A, 1.0, false);
        let cmds = decks.set_pitch(DeckId::A, 0.5);
        let base = 1.0 + 0.5 * decks.deck(DeckId::A).pitch_range.fraction();
        assert_eq!(
            rate_of(&cmds, DeckId::A),
            Some(base + BEND_COARSE),
            "the bend rides on top of whatever the fader now says"
        );
    }

    // ---- permanent tempo trim -------------------------------------------

    #[test]
    fn a_trim_steps_the_same_tempo_whatever_the_range_is() {
        // The whole point. A nudge used to step a percent of the RANGE, so
        // the same button moved the music by 0.08% on a narrow range and
        // 0.5% on a wide one, and an operator could not learn what it did.
        let mut narrow = DeckEngine::new();
        narrow.trim_pitch(DeckId::A, 1.0, false);
        let a = narrow.deck(DeckId::A).rate;

        let mut wide = DeckEngine::new();
        wide.step_pitch_range(DeckId::A, true);
        assert_ne!(
            wide.deck(DeckId::A).pitch_range.fraction(),
            narrow.deck(DeckId::A).pitch_range.fraction(),
            "the two decks must actually differ for this to prove anything"
        );
        wide.trim_pitch(DeckId::A, 1.0, false);
        let b = wide.deck(DeckId::A).rate;

        assert!((a - b).abs() < 1e-12, "{a} against {b}");
        assert!((a - (1.0 + TRIM_COARSE)).abs() < 1e-12, "half a percent of tempo: {a}");
    }

    #[test]
    fn a_fine_trim_is_a_tenth_of_a_coarse_one() {
        let mut decks = DeckEngine::new();
        decks.trim_pitch(DeckId::A, 1.0, true);
        assert!((decks.deck(DeckId::A).rate - (1.0 + TRIM_FINE)).abs() < 1e-12);
        assert!((TRIM_COARSE - TRIM_FINE * 10.0).abs() < 1e-12);
    }

    #[test]
    fn a_trim_stops_at_the_end_of_the_range_rather_than_running_past_it() {
        let mut decks = DeckEngine::new();
        let range = decks.deck(DeckId::A).pitch_range.fraction();
        for _ in 0..1000 {
            decks.trim_pitch(DeckId::A, 1.0, false);
        }
        assert!(
            (decks.deck(DeckId::A).pitch - range).abs() < 1e-12,
            "trimmed to {} with a range of {range}",
            decks.deck(DeckId::A).pitch
        );
    }

    #[test]
    fn a_trim_down_and_back_up_returns_to_where_it_started() {
        let mut decks = DeckEngine::new();
        decks.set_pitch(DeckId::A, 0.25);
        let was = decks.deck(DeckId::A).pitch;
        decks.trim_pitch(DeckId::A, -1.0, false);
        decks.trim_pitch(DeckId::A, 1.0, false);
        assert!((decks.deck(DeckId::A).pitch - was).abs() < 1e-12);
    }

    // ---- whole-track repeat ---------------------------------------------

    #[test]
    fn repeating_a_track_engages_a_span_over_the_whole_file() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        let duration = e.deck(DeckId::A).duration_secs;
        let cmds = e.repeat_track(DeckId::A);
        assert_eq!(
            cmds,
            vec![DeckCmd::SetLoopSpan {
                deck: DeckId::A,
                span: Some(LoopSpan { start_secs: 0.0, end_secs: duration }),
                seek: LoopSeek::None,
            }],
            "one span, no seek: the playhead is already inside the whole file"
        );
        assert!(e.deck(DeckId::A).repeats_whole_track());
        assert!(e.deck(DeckId::A).loop_on(), "and it is an ordinary loop as far as everything else is concerned");
    }

    #[test]
    fn a_second_press_stops_repeating_and_leaves_no_span() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.repeat_track(DeckId::A);
        let cmds = e.repeat_track(DeckId::A);
        assert_eq!(cmds, vec![DeckCmd::SetLoopSpan { deck: DeckId::A, span: None , seek: LoopSeek::None }]);
        assert!(!e.deck(DeckId::A).loop_on());
        assert!(!e.deck(DeckId::A).repeats_whole_track());
    }

    #[test]
    fn a_repeat_does_not_cost_the_operator_the_loop_they_were_keeping() {
        // The whole reason this does not go through `engage_loop`: that
        // overwrites RELOOP's memory and throws away a placed bookmark, so
        // repeating a track would quietly lose the loop saved at the drop.
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        let kept = LoopSpan { start_secs: 30.0, end_secs: 34.0 };
        e.deck_mut(DeckId::A).loop_memory = Some(kept);
        e.deck_mut(DeckId::A).bookmark = Some(12.5);

        e.repeat_track(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_memory, Some(kept), "RELOOP still remembers it");
        assert_eq!(e.deck(DeckId::A).bookmark, Some(12.5), "and the green mark is still placed");

        e.repeat_track(DeckId::A);
        assert_eq!(e.deck(DeckId::A).loop_memory, Some(kept), "leaving the repeat does not claim it either");
        let cmds = e.toggle_loop(DeckId::A);
        assert_eq!(
            cmds,
            vec![
                DeckCmd::SeekSeconds { deck: DeckId::A, secs: 30.0 },
                DeckCmd::SetLoopSpan {
                    deck: DeckId::A,
                    span: Some(kept),
                    seek: LoopSeek::MovedOut,
                },
            ],
            "and RELOOP brings back the loop, not the whole track"
        );
    }

    #[test]
    fn an_unloaded_deck_cannot_repeat_a_track() {
        let mut e = DeckEngine::new();
        assert!(e.repeat_track(DeckId::A).is_empty(), "nothing to repeat");
        assert!(e.deck(DeckId::A).loop_span.is_none());
        // A deck with a load still in flight has a duration of zero, which
        // would be a span of nothing at all.
        e.click(item(1), DeckTarget::A);
        assert!(e.repeat_track(DeckId::A).is_empty());
        assert!(e.deck(DeckId::A).loop_span.is_none());
    }

    #[test]
    fn a_fresh_load_drops_a_whole_track_repeat_like_any_other_span() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.repeat_track(DeckId::A);
        assert!(e.deck(DeckId::A).repeats_whole_track());
        load_analysed(&mut e, DeckId::A, 2, 128.0, 0.0);
        assert!(!e.deck(DeckId::A).loop_on(), "a span measured on the last track means nothing on this one");
        assert!(!e.deck(DeckId::A).repeats_whole_track());
    }

    #[test]
    fn the_stepper_shortens_a_repeat_into_an_ordinary_loop() {
        // Halving a repeat leaves the first half, which is no longer the
        // whole file -- so the gesture that made it a repeat makes it one
        // again rather than toggling it off. Worth pinning: it is the one
        // place the toggle is not its own inverse.
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        let duration = e.deck(DeckId::A).duration_secs;
        e.repeat_track(DeckId::A);
        e.loop_halve(DeckId::A);
        assert!(!e.deck(DeckId::A).repeats_whole_track(), "half a file is a loop now");
        assert!(e.deck(DeckId::A).loop_on());
        e.repeat_track(DeckId::A);
        assert_eq!(
            e.deck(DeckId::A).loop_span,
            Some(LoopSpan { start_secs: 0.0, end_secs: duration }),
            "and the gesture puts the whole file back"
        );
    }

    // ---- what a load may and may not carry ------------------------------

    #[test]
    fn a_load_drops_a_tempo_the_lock_worked_out_but_keeps_one_set_by_hand() {
        // The lock's rate matched THIS deck to the one on the other side.
        // Replace the track under it and that match describes a record that
        // has left the building.
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 124.0, 0.0);
        e.deck_mut(DeckId::A).playing = true;
        e.sync(DeckId::B, true);
        let locked = e.deck(DeckId::B).rate;
        assert!((locked - 1.0).abs() > 1e-6, "the lock did move the rate: {locked}");
        assert!(e.deck(DeckId::B).rate_from_lock);

        // With AUTO SYNC off nothing would ever correct it, which is the
        // case that made this a fault rather than a wrinkle.
        e.auto_sync = false;
        load_analysed(&mut e, DeckId::B, 3, 100.0, 0.0);
        assert_eq!(e.deck(DeckId::B).rate, 1.0, "back to the new track's own tempo");
        assert_eq!(e.deck(DeckId::B).pitch, 0.0);
        assert!(!e.deck(DeckId::B).rate_from_lock);
    }

    #[test]
    fn dropping_the_locks_tempo_does_not_stop_auto_sync_claiming_the_new_track() {
        // The reset runs when the track lands; the grid arrives after it,
        // and AUTO SYNC locking the new record to the room is the whole
        // point of leaving it on.
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 124.0, 0.0);
        e.deck_mut(DeckId::A).playing = true;
        e.sync(DeckId::B, true);
        assert!(e.auto_sync, "on by default, and this test is about that");
        load_analysed(&mut e, DeckId::B, 3, 100.0, 0.0);
        assert!(
            (e.deck(DeckId::B).rate - 1.28).abs() < 1e-9,
            "the new track is locked to the live one, not left at its own tempo: {}",
            e.deck(DeckId::B).rate
        );
    }

    #[test]
    fn a_tempo_the_hand_set_survives_a_load() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        e.set_pitch(DeckId::A, 0.5);
        let by_hand = e.deck(DeckId::A).rate;
        assert!(!e.deck(DeckId::A).rate_from_lock, "a hand on the slider owns the rate");
        load_analysed(&mut e, DeckId::A, 2, 100.0, 0.0);
        assert_eq!(e.deck(DeckId::A).rate, by_hand, "the operator's tempo follows them");
    }

    #[test]
    fn loading_over_the_pinned_deck_hands_the_pin_on_rather_than_dragging_the_live_one() {
        // A deck with a load in flight cannot lead. Left holding the pin,
        // its new grid arrives and the LIVE deck gets pulled onto it.
        let mut e = DeckEngine::new();
        // Loading over a running deck: the default policy would refuse it,
        // and what this test is about is what a load DOES.
        e.over_playing = OverPlaying::Stop;
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 124.0, 0.0);
        e.deck_mut(DeckId::A).playing = true;
        e.deck_mut(DeckId::B).playing = true;
        e.sync(DeckId::B, true);
        e.sync_master = Some(DeckId::A);

        e.click(item(9), DeckTarget::A);
        assert_ne!(e.sync_master, Some(DeckId::A), "a loading deck does not keep the pin");
    }

    #[test]
    fn a_held_bend_does_not_follow_the_operator_onto_the_next_track() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        e.hold_bend(DeckId::A, 1.0, false);
        assert_ne!(e.deck(DeckId::A).bend, 0.0);
        e.click(item(9), DeckTarget::A);
        assert_eq!(e.deck(DeckId::A).bend, 0.0, "the lean was on the record that just left");
    }

    #[test]
    fn a_load_leaves_the_console_alone_unless_it_is_asked_not_to() {
        // The default IS today's behaviour, and a later flip of one of
        // these bools should fail here rather than move eight other tests.
        assert_eq!(LoadReset::default(), LoadReset {
            speed: false, key: false, eq: false, filter: false, gain: false, stems: false,
        });
        assert_eq!(DeckEngine::new().load_reset, LoadReset::default());

        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        e.deck_mut(DeckId::A).eq = [0.3, 0.4, 0.5];
        e.deck_mut(DeckId::A).filter = -0.7;
        e.deck_mut(DeckId::A).gain = 0.6;
        e.deck_mut(DeckId::A).key_shift = 2.0;
        e.deck_mut(DeckId::A).stem_gain = [0.1, 0.2, 0.3, 0.4];
        load_analysed(&mut e, DeckId::A, 2, 100.0, 0.0);
        let s = e.deck(DeckId::A);
        assert_eq!(s.eq, [0.3, 0.4, 0.5], "the EQ describes the room, not the record");
        assert_eq!(s.filter, -0.7);
        assert_eq!(s.gain, 0.6);
        assert_eq!(s.key_shift, 2.0);
        assert_eq!(s.stem_gain, [0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn each_switch_clears_its_own_group_and_no_other() {
        for (name, policy) in [
            ("eq", LoadReset { eq: true, ..LoadReset::default() }),
            ("filter", LoadReset { filter: true, ..LoadReset::default() }),
            ("gain", LoadReset { gain: true, ..LoadReset::default() }),
            ("key", LoadReset { key: true, ..LoadReset::default() }),
            ("stems", LoadReset { stems: true, ..LoadReset::default() }),
        ] {
            let mut e = DeckEngine::new();
            e.set_load_reset(policy);
            load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
            let d = e.deck_mut(DeckId::A);
            d.eq = [0.3, 0.4, 0.5];
            d.eq_kill = [true; 3];
            d.filter = -0.7;
            d.gain = 0.6;
            d.key_shift = 2.0;
            d.stem_gain = [0.1, 0.2, 0.3, 0.4];
            d.stem_solo = [true; STEM_COUNT];
            load_analysed(&mut e, DeckId::A, 2, 100.0, 0.0);
            let s = e.deck(DeckId::A);
            assert_eq!(s.eq == [1.0; 3], policy.eq, "{name}: eq");
            assert_eq!(s.eq_kill == [false; 3], policy.eq, "{name}: eq kills");
            assert_eq!(s.filter == 0.0, policy.filter, "{name}: filter");
            assert_eq!(s.gain == 1.0, policy.gain, "{name}: gain");
            assert_eq!(s.key_shift == 0.0, policy.key, "{name}: key");
            assert_eq!(s.stem_gain == [1.0; STEM_COUNT], policy.stems, "{name}: stem gains");
            assert_eq!(s.stem_solo == [false; STEM_COUNT], policy.stems, "{name}: stem solos");
        }
    }

    #[test]
    fn the_speed_switch_clears_a_tempo_the_hand_set() {
        let mut e = DeckEngine::new();
        e.set_load_reset(LoadReset { speed: true, ..LoadReset::default() });
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        e.set_pitch(DeckId::A, 0.5);
        assert_ne!(e.deck(DeckId::A).rate, 1.0);
        e.auto_sync = false;
        load_analysed(&mut e, DeckId::A, 2, 100.0, 0.0);
        assert_eq!(e.deck(DeckId::A).rate, 1.0, "asked for, so even the hand's tempo goes");
        assert_eq!(e.deck(DeckId::A).pitch, 0.0);
    }

    // ---- which key the lock holds ---------------------------------------

    #[test]
    fn the_default_mode_holds_the_tracks_own_key_and_touches_no_knob() {
        // The mode that has always shipped. The lock starts ON, so the
        // first press is a release.
        let mut e = DeckEngine::new();
        assert_eq!(e.deck(DeckId::A).keylock_mode, KeylockMode::Original);
        assert!(e.deck(DeckId::A).keylock, "on by default");
        e.set_pitch(DeckId::A, 1.0);
        let released = e.toggle_keylock(DeckId::A);
        assert_eq!(released, vec![DeckCmd::SetKeylock { deck: DeckId::A, on: false }]);
        let engaged = e.toggle_keylock(DeckId::A);
        assert_eq!(engaged, vec![DeckCmd::SetKeylock { deck: DeckId::A, on: true }]);
        assert_eq!(e.deck(DeckId::A).key_shift, 0.0, "nothing borrowed, nothing owed");
        assert_eq!(e.deck(DeckId::A).keylock_offset, 0.0);
    }

    #[test]
    fn holding_the_key_that_was_sounding_folds_the_tempos_own_interval_into_the_knob() {
        let mut e = DeckEngine::new();
        e.deck_mut(DeckId::A).keylock_mode = KeylockMode::Current;
        e.toggle_keylock(DeckId::A); // off
        e.set_pitch(DeckId::A, 1.0); // the widest the default range reaches
        let rate = e.deck(DeckId::A).rate;
        let want = 12.0 * rate.log2();
        e.toggle_keylock(DeckId::A); // on
        assert!(
            (e.deck(DeckId::A).key_shift - want).abs() < 1e-9,
            "the knob holds the interval the tempo was making: {} against {want}",
            e.deck(DeckId::A).key_shift
        );
        assert!((e.deck(DeckId::A).keylock_offset - want).abs() < 1e-9);
    }

    #[test]
    fn letting_go_gives_back_exactly_what_was_borrowed() {
        let mut e = DeckEngine::new();
        e.deck_mut(DeckId::A).keylock_mode = KeylockMode::Current;
        e.toggle_keylock(DeckId::A);
        e.set_pitch(DeckId::A, 1.0);
        e.toggle_keylock(DeckId::A);
        // The tempo moves WHILE the lock holds, which is the whole reason
        // the borrowed amount is recorded rather than recomputed.
        e.set_pitch(DeckId::A, -0.4);
        e.toggle_keylock(DeckId::A);
        assert!(
            e.deck(DeckId::A).key_shift.abs() < 1e-9,
            "back to the track's own key, not to an interval worked out against a tempo that moved: {}",
            e.deck(DeckId::A).key_shift
        );
        assert_eq!(e.deck(DeckId::A).keylock_offset, 0.0);
    }

    #[test]
    fn the_keeping_mode_leaves_the_borrowed_semitones_in_the_knob() {
        let mut e = DeckEngine::new();
        e.deck_mut(DeckId::A).keylock_mode = KeylockMode::CurrentKept;
        e.toggle_keylock(DeckId::A);
        e.set_pitch(DeckId::A, 1.0);
        e.toggle_keylock(DeckId::A);
        let held = e.deck(DeckId::A).key_shift;
        assert!(held > 0.0);
        e.toggle_keylock(DeckId::A);
        assert_eq!(e.deck(DeckId::A).key_shift, held, "the interval is the operator's now");
        assert_eq!(e.deck(DeckId::A).keylock_offset, 0.0, "and nothing is owed");
    }

    #[test]
    fn a_key_step_taken_while_the_lock_holds_survives_letting_go() {
        let mut e = DeckEngine::new();
        e.deck_mut(DeckId::A).keylock_mode = KeylockMode::Current;
        e.toggle_keylock(DeckId::A);
        e.set_pitch(DeckId::A, 1.0);
        e.toggle_keylock(DeckId::A);
        e.nudge_key_shift(DeckId::A, 1.0);
        e.toggle_keylock(DeckId::A);
        assert!(
            (e.deck(DeckId::A).key_shift - 1.0).abs() < 1e-9,
            "the borrow goes back, the operator's own step stays: {}",
            e.deck(DeckId::A).key_shift
        );
    }

    #[test]
    fn zeroing_the_key_clears_the_debt_so_the_next_release_cannot_go_negative() {
        let mut e = DeckEngine::new();
        e.deck_mut(DeckId::A).keylock_mode = KeylockMode::Current;
        e.toggle_keylock(DeckId::A);
        e.set_pitch(DeckId::A, 1.0);
        e.toggle_keylock(DeckId::A);
        e.reset_key_shift(DeckId::A);
        assert_eq!(e.deck(DeckId::A).keylock_offset, 0.0);
        e.toggle_keylock(DeckId::A);
        assert_eq!(e.deck(DeckId::A).key_shift, 0.0, "and it stays at the track's own key");
    }

    #[test]
    fn cycling_names_the_key_the_next_press_will_hold_and_moves_nothing() {
        let mut e = DeckEngine::new();
        e.set_pitch(DeckId::A, 1.0);
        let before = e.deck(DeckId::A).key_shift;
        assert_eq!(e.deck(DeckId::A).keylock_mode.label(), "KEY");
        assert!(e.cycle_keylock_mode(DeckId::A).is_empty(), "it sends nothing");
        assert_eq!(e.deck(DeckId::A).keylock_mode.label(), "NOW");
        e.cycle_keylock_mode(DeckId::A);
        assert_eq!(e.deck(DeckId::A).keylock_mode.label(), "NOW+");
        e.cycle_keylock_mode(DeckId::A);
        assert_eq!(e.deck(DeckId::A).keylock_mode, KeylockMode::Original, "three, then round");
        assert_eq!(e.deck(DeckId::A).key_shift, before, "and no pitch moved");
    }

    #[test]
    fn a_borrow_clamped_at_the_rail_is_given_back_only_as_far_as_it_went() {
        let mut e = DeckEngine::new();
        e.deck_mut(DeckId::A).keylock_mode = KeylockMode::Current;
        e.toggle_keylock(DeckId::A);
        e.set_key_shift(DeckId::A, KEY_SHIFT_MAX);
        e.set_pitch(DeckId::A, 1.0);
        e.toggle_keylock(DeckId::A);
        assert_eq!(e.deck(DeckId::A).key_shift, KEY_SHIFT_MAX, "the knob was already at the rail");
        assert_eq!(e.deck(DeckId::A).keylock_offset, 0.0, "so nothing was actually borrowed");
        e.toggle_keylock(DeckId::A);
        assert_eq!(
            e.deck(DeckId::A).key_shift, KEY_SHIFT_MAX,
            "and letting go must not take back semitones it never got"
        );
    }

    // ---- CUE over its own edges -----------------------------------------

    #[test]
    fn cue_pressed_on_a_stopped_deck_away_from_the_mark_moves_the_mark_here() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        e.deck_mut(DeckId::A).position_secs = 42.0;
        let cmds = e.cue_press(DeckId::A);
        assert!(cmds.is_empty(), "setting a mark makes no sound and moves nothing");
        assert_eq!(e.deck(DeckId::A).cue_secs, 42.0);
        assert!(!e.deck(DeckId::A).cue_held, "there is nothing to release");
        assert!(e.cue_release(DeckId::A).is_empty());
    }

    #[test]
    fn cue_held_on_a_cued_deck_previews_from_the_mark_and_stops_on_release() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        e.deck_mut(DeckId::A).cue_secs = 30.0;
        e.deck_mut(DeckId::A).position_secs = 30.0;
        let down = e.cue_press(DeckId::A);
        assert_eq!(down, vec![
            DeckCmd::SeekSeconds { deck: DeckId::A, secs: 30.0 },
            DeckCmd::SetPlaying { deck: DeckId::A, playing: true },
        ]);
        assert!(e.deck(DeckId::A).cue_held);
        // It ran on for a while, as a preview does.
        e.deck_mut(DeckId::A).position_secs = 33.0;
        let up = e.cue_release(DeckId::A);
        assert_eq!(up, vec![
            DeckCmd::SetPlaying { deck: DeckId::A, playing: false },
            DeckCmd::SeekSeconds { deck: DeckId::A, secs: 30.0 },
        ]);
        assert!(!e.deck(DeckId::A).cue_held);
        assert_eq!(e.deck(DeckId::A).cue_secs, 30.0, "an audition never moves the mark");
    }

    #[test]
    fn cue_pressed_while_playing_returns_to_the_mark_rather_than_moving_it() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        e.deck_mut(DeckId::A).cue_secs = 12.0;
        e.deck_mut(DeckId::A).position_secs = 90.0;
        e.deck_mut(DeckId::A).playing = true;
        e.cue_press(DeckId::A);
        assert_eq!(e.deck(DeckId::A).cue_secs, 12.0, "a playing deck's mark is not up for grabs");
        e.cue_release(DeckId::A);
        assert_eq!(e.deck(DeckId::A).position_secs, 12.0);
        assert!(!e.deck(DeckId::A).playing);
    }

    #[test]
    fn a_previewing_deck_is_left_alone_by_the_lock() {
        // The audition is not the mix. A servo that corrected it would
        // drag the preview off the very mark it is auditioning.
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 124.0, 0.0);
        e.deck_mut(DeckId::A).playing = true;
        e.sync(DeckId::B, true);
        e.deck_mut(DeckId::B).position_secs = e.deck(DeckId::B).cue_secs;
        e.cue_press(DeckId::B);
        let held = e.hold_deck_sync();
        assert!(
            !held.iter().any(|c| matches!(c, DeckCmd::SeekSeconds { deck, .. } if *deck == DeckId::B)),
            "the lock must not move a deck that is being auditioned: {held:?}"
        );
    }

    #[test]
    fn the_cue_lamp_says_what_the_button_will_do() {
        let mut e = DeckEngine::new();
        assert_eq!(e.cue_led(DeckId::A), CueLed::Dark, "an empty deck has nothing to cue");
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        assert_eq!(e.cue_led(DeckId::A), CueLed::Solid, "parked on the mark, ready");
        e.deck_mut(DeckId::A).position_secs = 42.0;
        assert_eq!(e.cue_led(DeckId::A), CueLed::Blink, "stopped elsewhere: a press moves the mark");
        e.deck_mut(DeckId::A).playing = true;
        assert_eq!(e.cue_led(DeckId::A), CueLed::Dark, "nothing to say while it plays");
        e.deck_mut(DeckId::A).cue_held = true;
        assert_eq!(e.cue_led(DeckId::A), CueLed::Solid, "except while it is being auditioned");
    }

    // ---- how far the fader reaches ---------------------------------------

    #[test]
    fn the_range_ladder_saturates_rather_than_wrapping_under_a_running_mix() {
        let mut e = DeckEngine::new();
        assert_eq!(e.deck(DeckId::A).pitch_range.label(), "±8%", "the everyday rung");
        for _ in 0..20 {
            e.step_pitch_range(DeckId::A, true);
        }
        assert_eq!(e.deck(DeckId::A).pitch_range.label(), "±90%", "and it stops at the widest");
        for _ in 0..20 {
            e.step_pitch_range(DeckId::A, false);
        }
        assert_eq!(e.deck(DeckId::A).pitch_range.label(), "±4%", "and at the narrowest");
    }

    #[test]
    fn choosing_the_range_never_moves_the_music() {
        // Either direction. The old toggle held the tempo when widening and
        // clamped it when narrowing, which with a ladder is most presses.
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        e.set_pitch(DeckId::A, 1.0);
        let running_at = e.deck(DeckId::A).rate;
        for wider in [true, false, false, false, false, true, true] {
            let cmds = e.step_pitch_range(DeckId::A, wider);
            assert!(cmds.is_empty(), "a range press sends nothing");
            assert_eq!(
                e.deck(DeckId::A).rate, running_at,
                "the tempo is not the fader's to change: {} at {}",
                e.deck(DeckId::A).rate,
                e.deck(DeckId::A).pitch_range.label()
            );
        }
    }

    #[test]
    fn choosing_the_range_does_not_unlock_a_synced_deck() {
        // It used to go through `set_pitch`, which opts a non-master out of
        // the lock -- so pressing the range button silently unlocked it.
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 124.0, 0.0);
        e.deck_mut(DeckId::A).playing = true;
        e.sync(DeckId::B, true);
        assert!(e.deck(DeckId::B).synced);
        e.step_pitch_range(DeckId::B, true);
        assert!(e.deck(DeckId::B).synced, "still locked");
        assert!(!e.deck(DeckId::B).auto_opt_out, "and not opted out");
    }

    #[test]
    fn a_tempo_outside_the_new_range_pins_the_fader_without_being_dragged_back() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 128.0, 0.0);
        e.step_pitch_range(DeckId::A, true);
        e.step_pitch_range(DeckId::A, true);
        e.set_pitch(DeckId::A, 1.0);
        let fast = e.deck(DeckId::A).rate;
        assert!(fast > 1.1, "well outside the narrow rungs: {fast}");
        for _ in 0..6 {
            e.step_pitch_range(DeckId::A, false);
        }
        assert_eq!(e.deck(DeckId::A).rate, fast, "the record keeps running at the tempo it had");
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
        let leader = SyncView { grid: grid(128.0, 0.1), position_secs: 10.0, rate: 1.0, offset_beats: 0.0 };
        let follower = SyncView { grid: grid(124.0, 0.05), position_secs: 30.0, rate: 1.0, offset_beats: 0.0 };
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
        let leader = SyncView { grid: grid(150.0, 0.0), position_secs: 4.0, rate: 1.0, offset_beats: 0.0 };
        let follower = SyncView { grid: grid(75.0, 0.0), position_secs: 9.0, rate: 1.0, offset_beats: 0.0 };
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
        engine.grid_ready(DeckId::A, gen, grid, None, None);
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
            None,
            None,
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
        let leader = SyncView { grid: grid(120.0, 0.0), position_secs: 8.0, rate: 1.0, offset_beats: 0.0 };
        let follower = SyncView { grid: grid(120.0, 0.0), position_secs: 33.3, rate: 1.0, offset_beats: 0.0 };
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
        let with = SyncView { grid: grid(120.0, 0.0), position_secs: 1.0, rate: 1.0, offset_beats: 0.0 };
        let without = SyncView { grid: TrackGrid::default(), position_secs: 1.0, rate: 1.0, offset_beats: 0.0 };
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
        let cmds = engine.grid_ready(deck, gen, grid(100.0, 0.0), None, None);

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
    fn a_censor_suspends_the_phase_lock_and_does_not_relock_when_it_ends() {
        // The difference from a hand, and the whole reason the reverse hold
        // is its own engine method: the GHOST landing is the truth, and a
        // beat-quantised seek on top of it would move the deck off the very
        // place the hold exists to return it to.
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 120.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 10.0, true);
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::B, 3.0, true);
        engine.apply_auto_sync();

        let cmds = engine.censor(DeckId::B, true);
        assert_eq!(cmds, vec![DeckCmd::Censor { deck: DeckId::B, on: true }]);
        assert!(engine.deck(DeckId::B).scratching);
        engine.observe(DeckId::A, 10.4, true);
        let cmds = engine.apply_auto_sync();
        assert!(seek_of(&cmds, DeckId::B).is_none(), "no seek under a reverse hold");

        let cmds = engine.censor(DeckId::B, false);
        assert_eq!(cmds, vec![DeckCmd::Censor { deck: DeckId::B, on: false }]);
        assert!(!engine.deck(DeckId::B).scratching);
        assert!(
            !cmds.iter().any(|c| matches!(
                c,
                DeckCmd::SeekSeconds { .. } | DeckCmd::SetRate { .. }
            )),
            "the ghost landing stands: {cmds:?}",
        );
        // And it is refused outright on a deck that is not running: there
        // would be nothing for the ghost to keep the place of.
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::B, 3.0, false);
        assert!(engine.censor(DeckId::B, true).is_empty());
    }

    #[test]
    fn a_motor_gesture_holds_the_servos_off_until_the_mixer_says_it_landed() {
        // A hand has a release to clear its hold on. A brake or a wind-up
        // has no event at all -- it simply lands -- so the mixer's word is
        // what ends it.
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 10.0, true);

        let cmds = engine.spin(DeckId::A, SpinMotion::Brake);
        assert_eq!(cmds, vec![DeckCmd::Spin { deck: DeckId::A, motion: SpinMotion::Brake }]);
        assert!(engine.deck(DeckId::A).scratching);
        assert!(!engine.deck(DeckId::A).playing, "a brake is a stop");
        // The platter is still winding down: the flag stands.
        engine.observe_spin(DeckId::A, true);
        assert!(engine.deck(DeckId::A).scratching);
        // And clears when the motor hands the rate back.
        engine.observe_spin(DeckId::A, false);
        assert!(!engine.deck(DeckId::A).scratching);

        let cmds = engine.spin(DeckId::A, SpinMotion::SoftStart);
        assert_eq!(cmds, vec![DeckCmd::Spin { deck: DeckId::A, motion: SpinMotion::SoftStart }]);
        assert!(engine.deck(DeckId::A).playing, "a soft start is a start");
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
        let cmds = engine.scratch(DeckId::B, ScratchMotion::Move { secs: 3.0, rate: -1.5 });
        assert!(seek_of(&cmds, DeckId::B).is_none());

        // Letting go re-locks it.
        let cmds = engine.scratch(DeckId::B, ScratchMotion::Release);
        assert!(!engine.deck(DeckId::B).scratching);
        assert!(seek_of(&cmds, DeckId::B).is_some(), "release must re-lock: {cmds:?}");
    }

    /// A hand on the LEADER's record is not a phase anybody should be
    /// corrected to. Every follower path already steps aside when the
    /// scratched deck is the one being moved; this is the other side.
    #[test]
    fn a_hand_on_the_leaders_record_freezes_the_follower_servo() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 120.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::A, 10.0, true);
        engine.observe(DeckId::B, 10.0, true);
        engine.apply_auto_sync();
        assert_eq!(engine.sync_master(), Some(DeckId::A));

        engine.scratch(DeckId::A, ScratchMotion::Grab);
        let held = engine.deck(DeckId::B).position_secs;
        // The drag: the leader's playhead jerks about a third of a beat at
        // a time, which crosses the re-seek threshold on every pump.
        for (index, at) in [10.4, 10.2, 10.6, 10.1, 10.5].into_iter().enumerate() {
            engine.observe(DeckId::A, at, true);
            engine.observe(DeckId::B, 10.0 + 0.5 * index as f64, true);
            let cmds = engine.hold_deck_sync();
            assert!(cmds.is_empty(), "pump {index} corrected under a hand: {cmds:?}");
        }
        assert_eq!(engine.deck(DeckId::B).position_secs, held + 2.0, "only its own travel");
    }

    /// The sharpest case: a reverse hold leaves the deck PLAYING, so the
    /// paused-master guard cannot catch it.
    #[test]
    fn a_reverse_hold_on_the_leader_does_not_seek_the_follower_every_pump() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 120.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::A, 20.0, true);
        engine.observe(DeckId::B, 20.0, true);
        engine.apply_auto_sync();
        assert_eq!(engine.sync_master(), Some(DeckId::A));

        engine.censor(DeckId::A, true);
        assert!(engine.deck(DeckId::A).playing, "a censor keeps the deck running");
        for step in 0..5 {
            // The leader walks BACKWARDS a THIRD of a beat at a time --
            // whole beats would leave the phase exactly where it was and
            // prove nothing.
            engine.observe(DeckId::A, 20.0 - (1.0 / 6.0) * (step + 1) as f64, true);
            let before = engine.deck(DeckId::B).position_secs;
            let cmds = engine.hold_deck_sync();
            assert!(cmds.is_empty(), "step {step}: {cmds:?}");
            assert_eq!(engine.deck(DeckId::B).position_secs, before);
        }
    }

    /// The third tempo is published and stays a readout: the arithmetic is
    /// made against the fader's rate and never against the platter's.
    #[test]
    fn the_platter_tempo_is_published_and_never_reaches_the_sync_view() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        engine.observe_platter(DeckId::A, -1.6);
        assert_eq!(engine.deck(DeckId::A).live_bpm(), Some(-192.0));
        assert_eq!(engine.deck(DeckId::A).rate, 1.0, "the fader is untouched");
        assert_eq!(engine.deck(DeckId::A).sync_view().unwrap().rate, 1.0);
        assert_eq!(engine.deck(DeckId::A).effective_bpm(), Some(120.0));
        // And a number that is not one is refused rather than stored.
        engine.observe_platter(DeckId::A, f64::NAN);
        assert_eq!(engine.deck(DeckId::A).live_bpm(), Some(-192.0));
    }

    /// QUANT anchors a snap on the phase the re-lock is ABOUT to impose.
    /// With the leader held, that phase will not be imposed, so the
    /// snapped landing is the operator's own — on the deck's own grid.
    #[test]
    fn quant_does_not_anchor_a_snap_on_a_scratched_leaders_phase() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 120.0, 0.13);
        engine.play_pause(DeckId::A);
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::A, 10.0, true);
        engine.observe(DeckId::B, 10.0, true);
        engine.apply_auto_sync();
        engine.set_snap_beats(DeckId::B, 4);

        engine.scratch(DeckId::A, ScratchMotion::Grab);
        engine.observe(DeckId::A, 10.37, true);
        let own = engine.deck(DeckId::B).grid.unwrap();
        let want = own.snap_translate(30.2, engine.deck(DeckId::B).position_secs, 4);
        engine.seek_secs_snapped(DeckId::B, 30.2);
        assert!(
            (engine.deck(DeckId::B).position_secs - want).abs() < 1e-9,
            "landed {} not {want}",
            engine.deck(DeckId::B).position_secs
        );
    }

    /// Two 120 BPM decks locked and playing, A leading. Returns the pair
    /// with a hand on A's record that has dragged it a third of a beat out
    /// of phase, the hand still down.
    fn dragged_leader() -> DeckEngine {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 120.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::A, 30.0, true);
        engine.observe(DeckId::B, 30.0, true);
        engine.apply_auto_sync();
        assert_eq!(engine.sync_master(), Some(DeckId::A));
        engine.scratch(DeckId::A, ScratchMotion::Grab);
        engine.observe(DeckId::A, 30.0 + 1.0 / 6.0, true);
        engine
    }

    /// Letting go of the record the group follows used to jump-cut the
    /// follower: the release re-locked, and the re-lock lands with a seek.
    /// It is walked back with the rate now.
    #[test]
    fn letting_the_leaders_record_go_walks_the_follower_back_instead_of_cutting_it() {
        let mut engine = dragged_leader();
        let cmds = engine.scratch(DeckId::A, ScratchMotion::Release);
        assert!(seek_of(&cmds, DeckId::B).is_none(), "no cut: {cmds:?}");
        assert!(engine.deck(DeckId::B).reland.is_some(), "and a walk instead");

        let gap_before = phase_gap(&engine);
        assert!(gap_before > EXT_RESEEK_BEATS, "a real error to close: {gap_before}");
        // Pump: both decks travel a beat at a time at their own rates.
        let (mut a, mut b) = (engine.deck(DeckId::A).position_secs, 30.0);
        for _ in 0..40 {
            a += 0.5;
            b += 0.5 * engine.deck(DeckId::B).rate;
            engine.observe(DeckId::A, a, true);
            engine.observe(DeckId::B, b, true);
            let cmds = engine.hold_deck_sync();
            assert!(seek_of(&cmds, DeckId::B).is_none(), "still no cut: {cmds:?}");
            if let Some(rate) = rate_of(&cmds, DeckId::B) {
                assert!(rate > 1.0, "B is behind, so it leans forward: {rate}");
                assert!((rate - 1.0).abs() <= 0.02 + 1e-9, "and only just: {rate}");
            }
            b = engine.deck(DeckId::B).position_secs;
        }
        assert!(phase_gap(&engine) < gap_before, "and it closed");
    }

    /// The walk hands back the moment the phase is inside what the standing
    /// servo holds -- not when the ceiling runs out.
    #[test]
    fn a_re_land_retires_when_the_phase_is_back_inside_what_the_servo_holds() {
        let mut engine = dragged_leader();
        engine.scratch(DeckId::A, ScratchMotion::Release);
        let (mut a, mut b) = (engine.deck(DeckId::A).position_secs, 30.0);
        let mut beats = 0;
        while engine.deck(DeckId::B).reland.is_some() && beats < 60 {
            a += 0.5;
            b += 0.5 * engine.deck(DeckId::B).rate;
            engine.observe(DeckId::A, a, true);
            engine.observe(DeckId::B, b, true);
            engine.hold_deck_sync();
            b = engine.deck(DeckId::B).position_secs;
            beats += 1;
        }
        assert!(engine.deck(DeckId::B).reland.is_none(), "it retired");
        assert!(beats < RELAND_BEATS as i32, "on the phase, not the ceiling: {beats}");
        assert!(phase_gap(&engine) <= EXT_RESEEK_BEATS + 1e-9);
    }

    /// A loop wrap moves the playhead somewhere the anchor cannot measure.
    /// Retiring on it would hand straight back to a servo that seeks --
    /// the jump-cut this exists to remove, one wrap late.
    #[test]
    fn a_loop_wrap_does_not_hand_a_re_land_back_to_a_servo_that_seeks() {
        let mut engine = dragged_leader();
        engine.scratch(DeckId::A, ScratchMotion::Release);
        let a = engine.deck(DeckId::A).position_secs;
        // The follower wraps its loop: back four beats.
        engine.observe(DeckId::A, a + 0.5, true);
        engine.observe(DeckId::B, 28.0, true);
        let cmds = engine.hold_deck_sync();
        assert!(seek_of(&cmds, DeckId::B).is_none(), "no cut on a wrap: {cmds:?}");
        assert!(engine.deck(DeckId::B).reland.is_some(), "the walk survives the wrap");
    }

    /// The other half of the same gesture, unchanged: a hand on a FOLLOWER
    /// still lands that deck on release, and never touches the leader.
    #[test]
    fn a_hand_on_a_followers_record_still_lands_it_and_never_touches_the_leader() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 120.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::A, 30.0, true);
        engine.observe(DeckId::B, 30.0, true);
        engine.apply_auto_sync();
        engine.scratch(DeckId::B, ScratchMotion::Grab);
        engine.observe(DeckId::B, 30.0 + 1.0 / 6.0, true);

        let cmds = engine.scratch(DeckId::B, ScratchMotion::Release);
        assert!(seek_of(&cmds, DeckId::B).is_some(), "it still lands: {cmds:?}");
        assert!(seek_of(&cmds, DeckId::A).is_none(), "and never the leader");
        assert!(rate_of(&cmds, DeckId::A).is_none());
        assert!(engine.deck(DeckId::A).reland.is_none(), "no walk on either deck");
        assert!(engine.deck(DeckId::B).reland.is_none());
    }

    /// A motor gesture has no release event of its own: the mixer's word
    /// ends it, and it must reach the same funnel a hand does.
    #[test]
    fn a_motor_gesture_on_the_leader_arms_the_walk_when_the_mixer_says_it_landed() {
        let mut engine = dragged_leader();
        engine.scratch(DeckId::A, ScratchMotion::Release);
        engine.deck_mut(DeckId::B).reland = None;

        engine.spin(DeckId::A, SpinMotion::SoftStart);
        assert!(engine.deck(DeckId::A).scratching);
        engine.observe(DeckId::A, 31.0, true);
        assert!(engine.hold_deck_sync().is_empty(), "frozen while it winds up");

        engine.observe_spin(DeckId::A, false);
        assert!(!engine.deck(DeckId::A).scratching, "the mixer ended it");
        assert!(engine.deck(DeckId::B).reland.is_some(), "same funnel as a hand");
    }

    /// A walk describes a PAIR of playheads and is nonsense the moment one
    /// of them is moved on purpose.
    #[test]
    fn a_deliberate_move_of_the_follower_ends_the_walk() {
        let mut engine = dragged_leader();
        engine.scratch(DeckId::A, ScratchMotion::Release);
        assert!(engine.deck(DeckId::B).reland.is_some());
        engine.seek_secs(DeckId::B, 60.0);
        assert!(engine.deck(DeckId::B).reland.is_none(), "the seek is the landing");

        let mut engine = dragged_leader();
        engine.scratch(DeckId::A, ScratchMotion::Release);
        engine.play_pause(DeckId::B);
        assert!(engine.deck(DeckId::B).reland.is_none(), "and so is a stop");
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
        assert!(engine.grid_ready(deck, first, grid(120.0, 0.0), None, None).is_empty());
        assert!(engine.deck(DeckId::A).grid.is_none(), "stale grid must not land");
        engine.grid_ready(deck, second, grid(126.0, 0.0), None, None);
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
        // Choosing the range keeps the audible tempo, in BOTH directions
        // now: it moves no music at all.
        engine.step_pitch_range(DeckId::A, true);
        assert_eq!(engine.deck(DeckId::A).pitch_range.label(), "±10%");
        assert!((engine.deck(DeckId::A).rate - 1.08).abs() < 1e-9, "tempo held");
        engine.step_pitch_range(DeckId::A, true);
        assert_eq!(engine.deck(DeckId::A).pitch_range.label(), "±16%");
        // …and the slider now has headroom to 16%.
        engine.set_pitch(DeckId::A, 1.0);
        assert!((engine.deck(DeckId::A).rate - 1.16).abs() < 1e-9);
        let cmds = engine.reset_pitch(DeckId::A);
        assert_eq!(cmds, vec![DeckCmd::SetRate { deck: DeckId::A, rate: 1.0 }]);
        assert!((engine.deck(DeckId::A).pitch).abs() < 1e-12);
        // A trim steps a share of the TRACK's tempo, so the wider range
        // above does not change what one press does.
        engine.trim_pitch(DeckId::A, 1.0, false);
        assert!(
            (engine.deck(DeckId::A).rate - (1.0 + TRIM_COARSE)).abs() < 1e-9,
            "{}",
            engine.deck(DeckId::A).rate
        );
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
        SyncView { grid: grid(bpm, 0.0), position_secs: beats * 60.0 / bpm, rate: 1.0, offset_beats: 0.0 }
    }

    #[test]
    fn ext_matches_the_rooms_tempo_and_trims_toward_its_phase() {
        // A 124 BPM track under a 128 BPM room, exactly in phase.
        let room = external(128.0, 8.0);
        let deck = SyncView { grid: grid(124.0, 0.0), position_secs: 8.0 * 60.0 / 124.0, rate: 1.0, offset_beats: 0.0 };
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
        let deck = SyncView { grid: grid(128.0, 0.0), position_secs: 8.0 * 60.0 / 128.0, rate: 1.0, offset_beats: 0.0 };
        let follow = external_follow(&room, &deck, 0.08).unwrap();
        assert!(follow.error_beats > 0.15, "{follow:?}");
        assert!(follow.rate > 1.0, "behind means catch up: {follow:?}");
        assert!(follow.rate <= 1.0 + EXT_PHASE_TRIM + 1e-9, "and gently: {follow:?}");
        assert!(follow.reseek_secs.is_none(), "a fifth of a beat is trimmable");

        // Half a beat out is not drift — it was moved. Land it.
        let deck = SyncView { grid: grid(128.0, 0.0), position_secs: 8.7 * 60.0 / 128.0, rate: 1.0, offset_beats: 0.0 };
        let follow = external_follow(&room, &deck, 0.08).unwrap();
        assert!(follow.reseek_secs.is_some(), "{follow:?}");
    }

    #[test]
    fn ext_folds_octaves_and_reports_walking_out_of_the_envelope() {
        // A 64 BPM track under a 128 BPM room plays at 1.0, one beat in two.
        let room = external(128.0, 4.0);
        let deck = SyncView { grid: grid(64.0, 0.0), position_secs: 2.0 * 60.0 / 64.0, rate: 1.0, offset_beats: 0.0 };
        let follow = external_follow(&room, &deck, 0.08).unwrap();
        assert!((follow.rate - 1.0).abs() < 0.03, "{follow:?}");
        assert!(follow.within_envelope);
        // A room 12% faster than the track needs more stretch than ±8%: it
        // still follows, but the operator has to be told.
        let room = external(140.0, 0.0);
        let deck = SyncView { grid: grid(125.0, 0.0), position_secs: 0.0, rate: 1.0, offset_beats: 0.0 };
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

    /// A verb that does not latch matches the decks and hands the deck
    /// back: no pin, no lock, and the deck opts out of auto sync so the
    /// next pump cannot turn the tap into the latch it refused.
    #[test]
    fn a_one_shot_sync_matches_the_decks_and_lets_go() {
        let mut engine = DeckEngine::new();
        engine.set_auto_sync(false);
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 100.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::A, 12.0, true);
        engine.observe(DeckId::B, 7.0, true);

        let before = engine.deck(DeckId::B).position_secs;
        engine.sync_verb(DeckId::B, SyncVerb::Match);
        assert!((engine.deck(DeckId::B).rate - 1.28).abs() < 1e-9, "tempo taken");
        assert!(engine.deck(DeckId::B).position_secs != before, "phase taken");
        assert!(!engine.deck(DeckId::B).synced, "and then let go");
        assert!(engine.deck(DeckId::B).auto_opt_out, "so auto sync cannot re-latch it");
        assert_eq!(engine.sync_master(), None, "a one-shot pins nothing");
    }

    /// The tempo half alone: the rate is the leader's, the playhead is
    /// exactly where the operator left it.
    #[test]
    fn a_tempo_only_sync_moves_no_music() {
        let mut engine = DeckEngine::new();
        engine.set_auto_sync(false);
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 100.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::A, 12.0, true);
        engine.observe(DeckId::B, 7.0, true);

        let before = engine.deck(DeckId::B).position_secs;
        let cmds = engine.sync_verb(DeckId::B, SyncVerb::Tempo);
        assert!((engine.deck(DeckId::B).rate - 1.28).abs() < 1e-9);
        assert_eq!(engine.deck(DeckId::B).position_secs, before, "no seek");
        assert!(
            !cmds.iter().any(|c| matches!(c, DeckCmd::SeekSeconds { .. })),
            "and none asked for"
        );
    }

    /// The phase half alone: the deck lands on the leader's grid at
    /// whatever tempo the fader is holding.
    #[test]
    fn a_phase_only_sync_changes_no_tempo() {
        let mut engine = DeckEngine::new();
        engine.set_auto_sync(false);
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 100.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::A, 12.0, true);
        engine.observe(DeckId::B, 7.0, true);

        let before = engine.deck(DeckId::B).position_secs;
        let cmds = engine.sync_verb(DeckId::B, SyncVerb::Phase);
        assert_eq!(engine.deck(DeckId::B).rate, 1.0, "the fader still owns the tempo");
        assert!(engine.deck(DeckId::B).position_secs != before, "phase taken");
        assert!(!cmds.iter().any(|c| matches!(c, DeckCmd::SetRate { .. })));
    }

    /// A phase-only landing leads by the DECK'S rate, not the rate the
    /// plan worked out: with the tempo half skipped the deck is still
    /// running at whatever the fader says, and reading the plan's rate puts
    /// every landing wrong by the difference times the lookahead.
    #[test]
    fn a_phase_only_landing_leads_by_the_deck_s_own_rate() {
        let landed = |lookahead: f64| {
            let mut engine = DeckEngine::new();
            engine.set_auto_sync(false);
            engine.land_lookahead_secs = lookahead;
            load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.1);
            load_analysed(&mut engine, DeckId::B, 2, 100.0, 0.05);
            engine.play_pause(DeckId::A);
            engine.play_pause(DeckId::B);
            engine.observe(DeckId::A, 30.0, true);
            engine.observe(DeckId::B, 20.0, true);
            engine.sync_verb(DeckId::B, SyncVerb::Phase);
            engine.deck(DeckId::B).position_secs
        };
        let bare = landed(0.0);
        let led = landed(0.02);
        // 1.0 is the deck's own rate; the plan's would be 1.28, which would
        // overshoot by 5.6 ms.
        assert!((led - (bare + 1.0 * 0.02)).abs() < 1e-9, "bare {bare}, led {led}");
    }

    /// A one-shot on a deck that IS latched is a re-land, not a release:
    /// only the hold toggles the lock.
    #[test]
    fn a_one_shot_on_a_latched_deck_leaves_the_lock_alone() {
        let mut engine = DeckEngine::new();
        engine.set_auto_sync(false);
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        load_analysed(&mut engine, DeckId::B, 2, 100.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::A, 12.0, true);
        engine.observe(DeckId::B, 7.0, true);
        engine.toggle_sync(DeckId::B);
        assert!(engine.deck(DeckId::B).synced);

        engine.sync_verb(DeckId::B, SyncVerb::Match);
        assert!(engine.deck(DeckId::B).synced, "still locked");
        assert!(!engine.deck(DeckId::B).auto_opt_out, "and not opted out");
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
        // The record's marks are written down BEFORE the deck lets go of
        // it, and the unload follows.
        assert!(matches!(cmds.first(), Some(DeckCmd::RetireMarks { .. })), "{cmds:?}");
        assert_eq!(cmds.last(), Some(&DeckCmd::UnloadTrack { deck: DeckId::A }));
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


    // ---- a load aimed at a deck that is already playing ------------------

    #[test]
    fn a_load_aimed_at_a_playing_deck_is_refused_by_default() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 30.0, true);
        let gen_before = engine.deck(DeckId::A).load_gen;

        assert_eq!(
            engine.click(item(2), DeckTarget::A),
            vec![DeckCmd::LoadRefused { deck: DeckId::A }],
        );
        assert_eq!(engine.deck(DeckId::A).title(), Some("track 1"));
        assert!(engine.deck(DeckId::A).playing);
        assert_eq!(engine.deck(DeckId::A).load_gen, gen_before);
        // The refusal spent no generation, so the next accepted load takes
        // the very next one — nothing downstream skips a beat.
        let (_, gen) = load_gen(&engine.click(item(3), DeckTarget::B));
        assert_eq!(gen, gen_before + 1);
        // And last_loaded still names the deck the last ACCEPTED load hit.
        assert_eq!(engine.last_loaded(), Some(DeckId::B));
    }

    #[test]
    fn a_refused_load_leaves_the_queue_row_where_it_was() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 30.0, true);
        engine.auto_load_queue = false;
        engine.enqueue(item(2));
        assert_eq!(
            engine.load_queued(0, DeckTarget::A),
            vec![DeckCmd::LoadRefused { deck: DeckId::A }],
        );
        // The guard fired before the remove — the same law OFF already keeps.
        assert_eq!(engine.queue().len(), 1);
    }

    #[test]
    fn under_stop_a_load_over_a_playing_deck_installs_it_stopped() {
        let mut engine = DeckEngine::new();
        engine.over_playing = OverPlaying::Stop;
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 30.0, true);
        let (deck, gen) = load_gen(&engine.click(item(2), DeckTarget::A));
        let cmds = engine.track_ready(deck, gen, 300.0);
        assert!(cmds.contains(&DeckCmd::InstallTrack { deck: DeckId::A, keep_playing: false }));
        assert!(!engine.deck(DeckId::A).playing);
    }

    #[test]
    fn under_keep_a_load_over_a_playing_deck_installs_it_running() {
        let mut engine = DeckEngine::new();
        engine.over_playing = OverPlaying::Keep;
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 30.0, true);
        let (deck, gen) = load_gen(&engine.click(item(2), DeckTarget::A));
        let cmds = engine.track_ready(deck, gen, 300.0);
        assert!(cmds.contains(&DeckCmd::InstallTrack { deck: DeckId::A, keep_playing: true }));
        assert!(engine.deck(DeckId::A).playing);
        // The deck never stopped, so nothing has to start it again.
        assert!(!cmds.iter().any(|c| matches!(c, DeckCmd::SetPlaying { .. })));
    }

    #[test]
    fn a_load_accepted_while_the_deck_was_idle_still_installs_when_it_lands_late() {
        // The gate is at the GESTURE, not at the landing: a load nobody
        // promised would run on installs stopped, whatever happened while
        // the bytes were in flight.
        let mut engine = DeckEngine::new();
        let (deck, gen) = load_gen(&engine.click(item(1), DeckTarget::A));
        engine.play_pause(DeckId::A);
        let cmds = engine.track_ready(deck, gen, 300.0);
        assert!(cmds.contains(&DeckCmd::InstallTrack { deck: DeckId::A, keep_playing: false }));
        assert!(!engine.deck(DeckId::A).playing);
    }


    // ---- a slot number is data, not a position --------------------------

    fn save_at(e: &mut DeckEngine, at: f64, len: f64) {
        e.observe(DeckId::A, at, true);
        e.engage_loop(DeckId::A, LoopSpan { start_secs: at, end_secs: at + len }, false);
        assert!(e.save_loop(DeckId::A));
    }

    fn numbers(e: &DeckEngine) -> Vec<(u16, f64)> {
        e.deck(DeckId::A)
            .loop_slots
            .iter()
            .map(|entry| (entry.slot, entry.span.start_secs))
            .collect()
    }




    #[test]
    fn dragging_one_end_of_a_loop_leaves_the_other_exactly_where_it_is() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.observe(DeckId::A, 10.0, true);
        e.engage_loop(DeckId::A, LoopSpan { start_secs: 10.0, end_secs: 12.0 }, false);

        // The OUT end out to 13.5: the IN does not budge.
        let cmds = e.set_loop_edge(DeckId::A, true, 13.5);
        let span = e.deck(DeckId::A).loop_span.expect("still looping");
        assert!((span.start_secs - 10.0).abs() < 1e-9, "the anchor holds");
        assert!((span.end_secs - 13.5).abs() < 1e-9, "at {}", span.end_secs);
        // A resize, so no seek -- the loop keeps running while it stretches.
        assert!(!cmds.iter().any(|c| matches!(c, DeckCmd::SeekSeconds { .. })));
        assert!(cmds.iter().any(|c| matches!(
            c,
            DeckCmd::SetLoopSpan { seek: LoopSeek::Changed, .. }
        )));

        // And the IN end in to 9.0, with the OUT anchored.
        e.set_loop_edge(DeckId::A, false, 9.0);
        let span = e.deck(DeckId::A).loop_span.expect("still looping");
        assert!((span.start_secs - 9.0).abs() < 1e-9);
        assert!((span.end_secs - 13.5).abs() < 1e-9, "the other anchor holds");
    }

    #[test]
    fn an_edge_that_would_cross_its_anchor_is_refused_rather_than_clamped() {
        // A clamp would move the ANCHOR, which is the one thing this
        // gesture promises to leave alone.
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.observe(DeckId::A, 10.0, true);
        e.engage_loop(DeckId::A, LoopSpan { start_secs: 10.0, end_secs: 12.0 }, false);
        let before = e.deck(DeckId::A).loop_span;
        assert!(e.set_loop_edge(DeckId::A, true, 9.0).is_empty(), "out behind in");
        assert_eq!(e.deck(DeckId::A).loop_span, before);
        assert!(e.set_loop_edge(DeckId::A, false, 13.0).is_empty(), "in past out");
        assert_eq!(e.deck(DeckId::A).loop_span, before);
        // And with no loop at all there is no edge to move.
        e.toggle_loop(DeckId::A);
        assert!(e.set_loop_edge(DeckId::A, true, 20.0).is_empty());
    }

    #[test]
    fn a_dragged_edge_snaps_against_its_own_anchor() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // half a second a beat
        e.set_snap_beats(DeckId::A, 1);
        e.observe(DeckId::A, 10.2, true);
        e.engage_loop(DeckId::A, LoopSpan { start_secs: 10.2, end_secs: 12.2 }, false);
        // A whole number of units from the IN, not from the track's grid:
        // the loop's own phase is what the operator is working in.
        e.set_loop_edge(DeckId::A, true, 13.35);
        let span = e.deck(DeckId::A).loop_span.expect("still looping");
        assert!((span.end_secs - 13.2).abs() < 1e-9, "at {}", span.end_secs);
        assert!((span.start_secs - 10.2).abs() < 1e-9);
    }
    // ---- the bank: a number, a kind and a colour ------------------------

    #[test]
    fn a_press_puts_a_mark_on_the_number_it_was_aimed_at() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.observe(DeckId::A, 30.0, true);
        assert!(e.set_slot(DeckId::A, 5, SlotKind::Cue));
        let entry = e.deck(DeckId::A).loop_slots[0];
        assert_eq!(entry.slot, 5, "the number it was aimed at, not the lowest free one");
        assert_eq!(entry.kind, SlotKind::Cue);
        assert!((entry.span.start_secs - 30.0).abs() < 1e-9);
        assert!(entry.span.len_secs() < 1e-9, "a point has no length");

        // Idempotent: a second press replaces what is there.
        e.observe(DeckId::A, 40.0, true);
        assert!(e.set_slot(DeckId::A, 5, SlotKind::Loop));
        assert_eq!(e.deck(DeckId::A).loop_slots.len(), 1);
        let entry = e.deck(DeckId::A).loop_slots[0];
        assert_eq!(entry.kind, SlotKind::Loop);
        assert!((entry.span.len_secs() - 2.0).abs() < 1e-9, "four beats at 120 BPM");

        // Past the end of the bank, and on an empty deck, it refuses.
        assert!(!e.set_slot(DeckId::A, LOOP_SLOT_CAP as u16, SlotKind::Cue));
        assert!(!DeckEngine::new().set_slot(DeckId::A, 0, SlotKind::Cue));
    }

    #[test]
    fn a_press_under_quant_lands_on_the_grid_rather_than_beside_it() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0); // beats every 0.5 s
        e.set_snap_beats(DeckId::A, 1);
        e.observe(DeckId::A, 30.19, true);
        e.set_slot(DeckId::A, 0, SlotKind::Cue);
        let at = e.deck(DeckId::A).loop_slots[0].span.start_secs;
        assert!((at - 30.0).abs() < 1e-9, "on the beat, at {at}");
        // With QUANT off it lands exactly where the record is.
        e.set_snap_beats(DeckId::A, 0);
        e.observe(DeckId::A, 40.19, true);
        e.set_slot(DeckId::A, 1, SlotKind::Cue);
        assert!((e.deck(DeckId::A).loop_slots[1].span.start_secs - 40.19).abs() < 1e-9);
    }

    #[test]
    fn what_a_number_does_is_stored_rather_than_measured() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.observe(DeckId::A, 10.0, true);
        e.loop_in(DeckId::A);
        e.observe(DeckId::A, 40.0, true);
        e.set_slot(DeckId::A, 0, SlotKind::Jump);

        // A JUMP carries the running loop rather than replacing it.
        let cmds = e.recall_loop(DeckId::A, 0);
        let span = e.deck(DeckId::A).loop_span.expect("still looping");
        assert!((span.start_secs - 40.0).abs() < 1e-9, "the loop came to the mark");
        assert!((span.len_secs() - 2.0).abs() < 1e-9, "and kept its length");
        assert!(cmds.iter().any(|c| matches!(c, DeckCmd::SetLoopSpan { .. })));

        // With no loop running it is an ordinary seek.
        e.toggle_loop(DeckId::A);
        e.observe(DeckId::A, 80.0, true);
        e.recall_loop(DeckId::A, 0);
        assert!((e.deck(DeckId::A).position_secs - 40.0).abs() < 1e-9);
        assert!(e.deck(DeckId::A).loop_span.is_none());
    }

    #[test]
    fn a_number_that_claims_to_be_a_loop_but_holds_a_point_cannot_engage_one() {
        // The length refusal is a PRECONDITION, ahead of whatever the kind
        // claims: a one-frame span on the audio thread is a stuck buzz.
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.observe(DeckId::A, 30.0, true);
        e.set_slot(DeckId::A, 0, SlotKind::Cue);
        // Lie about it, the way a hand-edited file could.
        e.deck_mut(DeckId::A).loop_slots[0].kind = SlotKind::Loop;
        e.observe(DeckId::A, 80.0, true);
        e.recall_loop(DeckId::A, 0);
        assert!(e.deck(DeckId::A).loop_span.is_none(), "nothing engaged");
        assert!((e.deck(DeckId::A).position_secs - 30.0).abs() < 1e-9, "it seeked instead");
    }

    #[test]
    fn a_swap_and_a_sort_carry_the_whole_mark_and_not_just_its_span() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.observe(DeckId::A, 30.0, true);
        e.set_slot(DeckId::A, 0, SlotKind::Cue);
        e.observe(DeckId::A, 10.0, true);
        e.set_slot(DeckId::A, 1, SlotKind::Loop);
        e.set_slot_colour(DeckId::A, 1, 0x112233ff);

        assert!(e.swap_loop_slots(DeckId::A, 0, 1));
        let slots = &e.deck(DeckId::A).loop_slots;
        assert_eq!(slots[0].kind, SlotKind::Loop, "the kind travelled");
        assert_eq!(slots[0].colour, 0x112233ff, "and so did the colour");
        assert_eq!(slots[1].kind, SlotKind::Cue);
        assert_eq!(slots[1].colour, 0);

        // And a sort by time keeps every entry whole. The swap already
        // left the row in playing order, so the sort reports no change --
        // and must still not have torn anything apart.
        assert!(!e.sort_loop_slots(DeckId::A, false), "already in order");
        let slots = &e.deck(DeckId::A).loop_slots;
        assert!((slots[0].span.start_secs - 10.0).abs() < 1e-9);
        assert_eq!(slots[0].kind, SlotKind::Loop);
        assert_eq!(slots[0].colour, 0x112233ff);
        assert_eq!(slots[1].kind, SlotKind::Cue);
    }

    #[test]
    fn every_number_has_a_colour_of_its_own_until_one_is_given_to_it() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.observe(DeckId::A, 10.0, true);
        for slot in 0..LOOP_SLOT_CAP as u16 {
            e.set_slot(DeckId::A, slot, SlotKind::Cue);
        }
        let shown: Vec<u32> =
            e.deck(DeckId::A).loop_slots.iter().map(|s| s.shown_colour()).collect();
        assert_eq!(shown, SLOT_PALETTE.to_vec(), "the palette, in number order");
        // Every hue is its own: eight chips nine points wide have to be
        // told apart at a glance.
        for (i, a) in SLOT_PALETTE.iter().enumerate() {
            for b in SLOT_PALETTE.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
        // A colour given by hand outranks the number's own.
        e.set_slot_colour(DeckId::A, 3, 0x00ff00ff);
        assert_eq!(e.deck(DeckId::A).loop_slots[3].shown_colour(), 0x00ff00ff);
        assert!(!e.set_slot_colour(DeckId::A, 99, 1), "and an empty number takes none");
    }

    #[test]
    fn the_wheel_can_move_a_point_where_it_used_to_do_nothing() {
        // A point has no length, and the span validator refuses anything
        // shorter than a loop may be -- so the nudge was inert on exactly
        // the marks it is most wanted for.
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.observe(DeckId::A, 30.0, true);
        e.set_slot(DeckId::A, 0, SlotKind::Cue);
        e.nudge_mark(DeckId::A, NudgeTarget::Slot(0), -0.010);
        let entry = e.deck(DeckId::A).loop_slots[0];
        assert!((entry.span.start_secs - 29.99).abs() < 1e-9, "at {}", entry.span.start_secs);
        assert!(entry.span.len_secs() < 1e-9, "and it is still a point");
        // Clamped into the track rather than refused: there is no length
        // for a refusal to protect.
        e.nudge_mark(DeckId::A, NudgeTarget::Slot(0), -1e6);
        assert!(e.deck(DeckId::A).loop_slots[0].span.start_secs.abs() < 1e-9);
    }
    // ---- marks that outlive the deck, and a hair of a move --------------

    #[test]
    fn a_record_leaving_a_deck_takes_its_marks_with_it() {
        let mut e = DeckEngine::new();
        // The saving helper leaves the deck running; loading over one is
        // what this test is about.
        e.over_playing = OverPlaying::Stop;
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        e.set_cue(DeckId::A, 12.5);
        save_at(&mut e, 30.0, 4.0);

        // Loading over it retires the OUTGOING record's marks first, and
        // carries them rather than naming the deck: by the time the host
        // runs this, the deck may hold the record that displaced it.
        let cmds = e.click(item(2), DeckTarget::A);
        match cmds.first() {
            Some(DeckCmd::RetireMarks { item, cue_secs, slots, .. }) => {
                assert_eq!(item.asset, crate::decks::tests::item(1).asset);
                assert_eq!(*cue_secs, Some(12.5));
                assert_eq!(slots.len(), 1);
                assert!((slots[0].span.start_secs - 30.0).abs() < 1e-9);
            }
            other => panic!("the outgoing record is owed its marks: {other:?}"),
        }
        assert!(matches!(cmds.last(), Some(DeckCmd::LoadTrack { .. })), "and then the load");
    }

    #[test]
    fn a_load_in_flight_is_owed_nothing() {
        // While a load is in flight the slots belong to the record before
        // it, and that one was retired when this load was asked for.
        let mut e = DeckEngine::new();
        e.click(item(1), DeckTarget::A);
        let cmds = e.click(item(2), DeckTarget::A);
        assert!(!cmds.iter().any(|c| matches!(c, DeckCmd::RetireMarks { .. })));
    }

    #[test]
    fn a_clone_gives_the_destination_the_source_record_and_none_of_its_chips() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        load_analysed(&mut e, DeckId::B, 2, 120.0, 0.0);
        e.observe(DeckId::B, 5.0, false);
        e.engage_loop(DeckId::B, LoopSpan { start_secs: 5.0, end_secs: 7.0 }, false);
        assert!(e.save_loop(DeckId::B));
        e.play_pause(DeckId::A);
        e.observe(DeckId::A, 20.0, true);

        let cmds = e.instant_double();
        assert!(
            cmds.iter().any(|c| matches!(c, DeckCmd::RetireMarks { .. })),
            "the record B is losing is owed its marks",
        );
        // And B does not keep the chips of the record it just lost.
        assert!(e.deck(DeckId::B).loop_slots.is_empty());
    }

    #[test]
    fn a_nudge_moves_a_mark_by_a_hair_the_grid_cannot_express() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        // QUANT on a whole beat: a placement would round a 10 ms step off.
        e.set_snap_beats(DeckId::A, 4);
        e.set_cue(DeckId::A, 12.0);
        let before = e.deck(DeckId::A).cue_secs;
        e.nudge_mark(DeckId::A, NudgeTarget::Cue, -0.010);
        assert!(
            (e.deck(DeckId::A).cue_secs - (before - 0.010)).abs() < 1e-9,
            "the snap does not get a say: {} -> {}",
            before,
            e.deck(DeckId::A).cue_secs,
        );
    }

    #[test]
    fn nudging_a_saved_loop_takes_the_sound_with_it_when_it_is_the_one_running() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        save_at(&mut e, 30.0, 4.0);
        let cmds = e.nudge_mark(DeckId::A, NudgeTarget::Slot(0), 0.010);
        let entry = e.deck(DeckId::A).loop_slots[0];
        assert!((entry.span.start_secs - 30.010).abs() < 1e-9, "the chip moved");
        assert!((entry.span.len_secs() - 4.0).abs() < 1e-9, "and kept its length");
        // It is the loop that is sounding, so the sound goes with it.
        let live = e.deck(DeckId::A).loop_span.expect("running");
        assert!((live.start_secs - 30.010).abs() < 1e-9);
        assert!(cmds.iter().any(|c| matches!(c, DeckCmd::SetLoopSpan { .. })));

        // Dropped, the chip still nudges and nothing sounds differently.
        e.toggle_loop(DeckId::A);
        let cmds = e.nudge_mark(DeckId::A, NudgeTarget::Slot(0), -0.010);
        assert!((e.deck(DeckId::A).loop_slots[0].span.start_secs - 30.0).abs() < 1e-9);
        assert!(cmds.is_empty());
    }

    #[test]
    fn a_nudge_off_the_end_is_refused_rather_than_clamped() {
        // A clamp would change the LENGTH as well as the place, which is
        // not what a nudge asked for.
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        save_at(&mut e, 0.0, 4.0);
        e.toggle_loop(DeckId::A);
        let before = e.deck(DeckId::A).loop_slots[0].span;
        e.nudge_mark(DeckId::A, NudgeTarget::Slot(0), -1.0);
        assert_eq!(e.deck(DeckId::A).loop_slots[0].span, before, "nothing moved");
    }
    #[test]
    fn deleting_a_loop_frees_its_number_and_moves_nothing_else() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        for at in [10.0, 20.0, 30.0] {
            save_at(&mut e, at, 2.0);
        }
        assert_eq!(numbers(&e), vec![(0, 10.0), (1, 20.0), (2, 30.0)]);

        e.delete_loop_slot(DeckId::A, 1);
        // The pad that held 20 is empty. The one that held 30 still does.
        assert_eq!(numbers(&e), vec![(0, 10.0), (2, 30.0)]);
        // And the next save reclaims the freed number rather than landing
        // at the end.
        save_at(&mut e, 40.0, 2.0);
        assert_eq!(numbers(&e), vec![(0, 10.0), (1, 40.0), (2, 30.0)]);
    }

    #[test]
    fn a_recall_addresses_the_number_and_not_the_position() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        for at in [10.0, 20.0, 30.0] {
            save_at(&mut e, at, 2.0);
        }
        e.delete_loop_slot(DeckId::A, 0);
        e.toggle_loop(DeckId::A);
        e.observe(DeckId::A, 50.0, true);
        // Slot 2 is the SECOND entry in the list now. Asking for 2 gets 30.
        e.recall_loop(DeckId::A, 2);
        let span = e.deck(DeckId::A).loop_span.expect("engaged");
        assert!((span.start_secs - 30.0).abs() < 1e-9, "at {}", span.start_secs);
        // And the number that was freed answers to nobody.
        assert!(e.recall_loop(DeckId::A, 0).is_empty());
    }

    #[test]
    fn two_loops_can_trade_numbers_without_the_music_moving() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        for at in [10.0, 20.0] {
            save_at(&mut e, at, 2.0);
        }
        let running = e.deck(DeckId::A).loop_span;
        assert!(e.swap_loop_slots(DeckId::A, 0, 1));
        assert_eq!(numbers(&e), vec![(0, 20.0), (1, 10.0)]);
        assert_eq!(e.deck(DeckId::A).loop_span, running, "a swap is filing, not sound");
        // Onto an EMPTY number it is a move.
        assert!(e.swap_loop_slots(DeckId::A, 0, 5));
        assert_eq!(numbers(&e), vec![(1, 10.0), (5, 20.0)]);
        // Nothing on either side, or the same number twice, moves nothing.
        assert!(!e.swap_loop_slots(DeckId::A, 2, 3));
        assert!(!e.swap_loop_slots(DeckId::A, 1, 1));
    }

    #[test]
    fn sorting_puts_the_row_in_playing_order_and_can_keep_the_gaps() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        for at in [30.0, 10.0, 20.0] {
            save_at(&mut e, at, 2.0);
        }
        e.delete_loop_slot(DeckId::A, 1);
        assert_eq!(numbers(&e), vec![(0, 30.0), (2, 20.0)]);
        save_at(&mut e, 5.0, 2.0);
        assert_eq!(numbers(&e), vec![(0, 30.0), (1, 5.0), (2, 20.0)]);

        // Without packing, the numbers the row holds stay exactly where
        // they are and only what sits on them changes.
        assert!(e.sort_loop_slots(DeckId::A, false));
        assert_eq!(numbers(&e), vec![(0, 5.0), (1, 20.0), (2, 30.0)]);
        // An already-sorted row costs nothing.
        assert!(!e.sort_loop_slots(DeckId::A, false));

        // Packing closes the gaps.
        e.delete_loop_slot(DeckId::A, 1);
        assert_eq!(numbers(&e), vec![(0, 5.0), (2, 30.0)]);
        assert!(e.sort_loop_slots(DeckId::A, true));
        assert_eq!(numbers(&e), vec![(0, 5.0), (1, 30.0)]);
    }

    #[test]
    fn a_marks_file_with_gaps_comes_back_with_its_gaps() {
        let mut e = DeckEngine::new();
        load_analysed(&mut e, DeckId::A, 1, 120.0, 0.0);
        let slots = vec![
            LoopSlot { slot: 5, span: LoopSpan { start_secs: 10.0, end_secs: 12.0 }, kind: SlotKind::Loop, colour: 0 },
            LoopSlot { slot: 0, span: LoopSpan { start_secs: 40.0, end_secs: 42.0 }, kind: SlotKind::Loop, colour: 0 },
            // Out of range and a duplicate: both refused at the door.
            LoopSlot { slot: 99, span: LoopSpan { start_secs: 1.0, end_secs: 2.0 }, kind: SlotKind::Loop, colour: 0 },
            LoopSlot { slot: 5, span: LoopSpan { start_secs: 3.0, end_secs: 4.0 }, kind: SlotKind::Loop, colour: 0 },
        ];
        e.restore_marks(DeckId::A, slots, None);
        assert_eq!(numbers(&e), vec![(0, 40.0), (5, 10.0)]);
    }
    // ---- the mark lands where the record starts -------------------------

    #[test]
    fn a_track_that_opens_with_silence_cues_past_it() {
        let mut e = DeckEngine::new();
        let (deck, gen) = load_gen(&e.click(item(1), DeckTarget::A));
        e.track_ready(deck, gen, 300.0);
        assert_eq!(e.deck(DeckId::A).cue_secs, 0.0, "nothing measured yet");
        let cmds =
            e.grid_ready(deck, gen, grid(120.0, 0.0), Some(SoundSpan { first_secs: 2.5, last_secs: 290.0 }), None);
        assert_eq!(e.deck(DeckId::A).cue_secs, 2.5, "the mark lands on the first sound");
        // And the deck, still parked at its top, goes with it -- left
        // behind, the lamp blinks and the first CUE press drags the mark
        // back to the silence it came from.
        assert_eq!(e.deck(DeckId::A).position_secs, 2.5);
        assert!(cmds.contains(&DeckCmd::SeekSeconds { deck: DeckId::A, secs: 2.5 }));
        // It is a default, not a placement: nothing was put there by hand.
        assert!(!e.deck(DeckId::A).cue_placed);
    }

    #[test]
    fn a_mark_a_hand_placed_outranks_the_first_sound() {
        let mut e = DeckEngine::new();
        let (deck, gen) = load_gen(&e.click(item(1), DeckTarget::A));
        e.track_ready(deck, gen, 300.0);
        e.set_cue(DeckId::A, 40.0);
        assert!(e.deck(DeckId::A).cue_placed);
        e.grid_ready(deck, gen, grid(120.0, 0.0), Some(SoundSpan { first_secs: 2.5, last_secs: 290.0 }), None);
        assert_eq!(e.deck(DeckId::A).cue_secs, 40.0, "the analysis does not move a hand's mark");
    }

    #[test]
    fn a_track_that_sounds_from_its_first_sample_leaves_the_mark_alone() {
        let mut e = DeckEngine::new();
        let (deck, gen) = load_gen(&e.click(item(1), DeckTarget::A));
        e.track_ready(deck, gen, 300.0);
        let cmds =
            e.grid_ready(deck, gen, grid(120.0, 0.0), Some(SoundSpan { first_secs: 0.0, last_secs: 300.0 }), None);
        assert_eq!(e.deck(DeckId::A).cue_secs, 0.0);
        assert!(!cmds.iter().any(|c| matches!(c, DeckCmd::SeekSeconds { .. })), "and nothing seeks");
    }

    #[test]
    fn the_first_sound_never_moves_a_deck_that_is_already_running() {
        let mut e = DeckEngine::new();
        let (deck, gen) = load_gen(&e.click(item(1), DeckTarget::A));
        e.track_ready(deck, gen, 300.0);
        e.play_pause(DeckId::A);
        e.observe(DeckId::A, 12.0, true);
        let cmds =
            e.grid_ready(deck, gen, grid(120.0, 0.0), Some(SoundSpan { first_secs: 2.5, last_secs: 290.0 }), None);
        assert_eq!(e.deck(DeckId::A).cue_secs, 2.5, "the mark still lands");
        assert_eq!(e.deck(DeckId::A).position_secs, 12.0, "but the record does not jump");
        assert!(!cmds.iter().any(|c| matches!(c, DeckCmd::SeekSeconds { .. })));
    }
    // ---- the operator's eject, and the press that takes it back ---------

    #[test]
    fn a_hand_eject_is_refused_while_the_deck_is_playing() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 30.0, true);
        let gen_before = engine.deck(DeckId::A).load_gen;
        assert_eq!(engine.eject_press(DeckId::A, 1_000), (EjectPress::Busy, vec![]));
        let state = engine.deck(DeckId::A);
        assert!(matches!(state.load, DeckLoad::Loaded { .. }), "the track stands");
        assert!(state.playing);
        // A refusal retires no generation: analysis and stems still in
        // flight for this load must keep passing the host's gen guard.
        assert_eq!(state.load_gen, gen_before);
    }

    #[test]
    fn the_autopilot_hand_back_still_retires_a_playing_deck() {
        // The guard is on the PRESS, not on eject. A landed fade hands back
        // a deck that is still sounding; refusing there would strand it.
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        engine.play_pause(DeckId::A);
        engine.observe(DeckId::A, 30.0, true);
        assert_eq!(
            engine.eject(DeckId::A).last(),
            Some(&DeckCmd::UnloadTrack { deck: DeckId::A }),
        );
        assert!(matches!(engine.deck(DeckId::A).load, DeckLoad::Empty));
    }

    #[test]
    fn a_second_press_inside_the_window_puts_the_ejected_track_back() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        let retired_gen = engine.deck(DeckId::A).load_gen;
        let (press, cmds) = engine.eject_press(DeckId::A, 1_000);
        assert_eq!(press, EjectPress::Ejected);
        assert_eq!(cmds.last(), Some(&DeckCmd::UnloadTrack { deck: DeckId::A }));

        let (press, cmds) = engine.eject_press(DeckId::A, 1_400);
        assert_eq!(press, EjectPress::Restored { title: item(1).title });
        match cmds.as_slice() {
            [DeckCmd::LoadTrack { deck, gen, item: back }] => {
                assert_eq!(*deck, DeckId::A);
                assert_eq!(back.asset, item(1).asset);
                assert!(*gen > retired_gen, "the restore is a fresh generation");
            }
            other => panic!("expected one LoadTrack, got {other:?}"),
        }
        // The undo spent the entry, and the deck now has a load in flight,
        // so a third press is a refusal rather than a second undo.
        assert_eq!(engine.eject_press(DeckId::A, 1_500), (EjectPress::Busy, vec![]));
    }

    #[test]
    fn a_press_after_the_window_closes_is_a_fresh_press_and_not_an_undo() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        assert_eq!(engine.eject_press(DeckId::A, 1_000).0, EjectPress::Ejected);
        // Half a second and one millisecond later the gesture is over. The
        // deck is Empty and there is nothing for a press to do.
        assert_eq!(engine.eject_press(DeckId::A, 1_501), (EjectPress::Nothing, vec![]));
        // It is the GESTURE that expired, not the memory: the track is
        // still on the stack, waiting for the next eject's window.
        load_analysed(&mut engine, DeckId::A, 2, 128.0, 0.0);
        assert_eq!(engine.eject_press(DeckId::A, 3_000).0, EjectPress::Ejected);
        assert_eq!(
            engine.eject_press(DeckId::A, 3_200).0,
            EjectPress::Restored { title: item(2).title },
        );
    }

    #[test]
    fn the_undo_reaches_the_second_last_track_when_the_last_one_is_back_on_a_deck() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        assert_eq!(engine.eject_press(DeckId::A, 1_000).0, EjectPress::Ejected);
        load_analysed(&mut engine, DeckId::A, 2, 128.0, 0.0);
        assert_eq!(engine.eject_press(DeckId::A, 2_000).0, EjectPress::Ejected);
        // Seed 2 is back on a deck by hand, so the newest entry is spoken
        // for and the undo has to reach past it to the one before.
        load_analysed(&mut engine, DeckId::B, 2, 128.0, 0.0);
        let (press, _) = engine.eject_press(DeckId::A, 2_300);
        assert_eq!(press, EjectPress::Restored { title: item(1).title });
        // Seed 2 stays on the stack: it was skipped, not spent.
        assert_eq!(engine.ejected_titles(), vec!["track 2"]);
    }

    #[test]
    fn the_undo_supersedes_a_track_the_queue_pumped_into_the_gap() {
        let mut engine = DeckEngine::new();
        // B holds the set together so auto_target sends the pump to A.
        load_analysed(&mut engine, DeckId::B, 9, 128.0, 0.0);
        engine.play_pause(DeckId::B);
        engine.observe(DeckId::B, 30.0, true);
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        engine.enqueue(item(7));

        assert_eq!(engine.eject_press(DeckId::A, 1_000).0, EjectPress::Ejected);
        let pumped = engine.pump_queue();
        assert!(!pumped.is_empty(), "the queue filled the gap");
        assert!(matches!(engine.deck(DeckId::A).load, DeckLoad::Loading { .. }));

        let (press, cmds) = engine.eject_press(DeckId::A, 1_300);
        assert_eq!(press, EjectPress::Restored { title: item(1).title });
        match cmds.as_slice() {
            [DeckCmd::LoadTrack { deck: DeckId::A, item: back, .. }] => {
                assert_eq!(back.asset, item(1).asset);
            }
            other => panic!("expected the ejected track back on A, got {other:?}"),
        }
        // The pumped track never sounded, so it is the NEXT track, not a
        // played-out one: it goes to the head of the queue, not its tail.
        assert_eq!(engine.queue().first().map(|q| q.asset), Some(item(7).asset));
    }

    #[test]
    fn the_ejected_stack_keeps_two_tracks_and_the_third_press_drops_the_oldest() {
        let mut engine = DeckEngine::new();
        // Spaced past the window, or presses two and three would be undos.
        for (seed, at) in [(1u8, 1_000u64), (2, 2_000), (3, 3_000)] {
            load_analysed(&mut engine, DeckId::A, seed, 128.0, 0.0);
            assert_eq!(engine.eject_press(DeckId::A, at).0, EjectPress::Ejected);
        }
        assert_eq!(
            engine.ejected_titles(),
            vec!["track 3", "track 2"],
            "newest first, bounded by what the gesture can reach",
        );
    }

    #[test]
    fn a_failed_load_is_cleared_by_the_press_but_never_enters_the_undo_stack() {
        let mut engine = DeckEngine::new();
        let (deck, gen) = load_gen(&engine.click(item(1), DeckTarget::A));
        engine.track_failed(deck, gen, "no bytes".into());
        assert!(matches!(engine.deck(DeckId::A).load, DeckLoad::Failed { .. }));
        let (press, cmds) = engine.eject_press(DeckId::A, 1_000);
        assert_eq!(press, EjectPress::Ejected);
        // A failed load never became Loaded, so it is owed no marks.
        assert_eq!(cmds, vec![DeckCmd::UnloadTrack { deck: DeckId::A }]);
        // A track that never sounded is not offered back.
        assert!(engine.ejected_titles().is_empty());
        assert_eq!(engine.eject_press(DeckId::A, 1_200), (EjectPress::Nothing, vec![]));
    }

    #[test]
    fn the_double_press_window_follows_the_decks_through_a_swap() {
        let mut engine = DeckEngine::new();
        load_analysed(&mut engine, DeckId::A, 1, 128.0, 0.0);
        assert_eq!(engine.eject_press(DeckId::A, 1_000).0, EjectPress::Ejected);
        engine.swap();
        // The window rides with the deck's CONTENTS, the way last_loaded does.
        assert_eq!(engine.eject_press(DeckId::A, 1_200), (EjectPress::Nothing, vec![]));
        assert_eq!(
            engine.eject_press(DeckId::B, 1_200).0,
            EjectPress::Restored { title: item(1).title },
        );
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

        // The fader most of the way to B with A's record still up: both
        // are audible, so the pin stands and corrections keep their
        // direction. (All the way over is the other law -- a master the
        // room cannot hear hands the group on.)
        engine.set_crossfader(0.8);
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
        // Loading over a running deck: the default policy would refuse it,
        // and what this test is about is what a load DOES.
        e.over_playing = OverPlaying::Stop;
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
        assert!(cmds.contains(&DeckCmd::SetLoopSpan { deck: DeckId::A, span: None , seek: LoopSeek::None }));
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
