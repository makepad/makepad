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

use crate::wave_analysis::TrackGrid;
use makepad_asset_data::{AssetId, AssetRevisionId, BlobId, MediaType};

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

/// Crossfader gain law.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FadeCurve {
    /// Constant perceived loudness: `gain_a=cos(x·π/2)`, `gain_b=sin(x·π/2)`.
    #[default]
    EqualPower,
    Linear,
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
    }
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

/// What a deck's SYNC control is set to. The control cycles through these.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SyncMode {
    /// Free: the deck runs at its own tempo (or the operator's pitch).
    #[default]
    Off,
    /// Held against the OTHER deck — the classic DJ sync.
    Deck,
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
    pub loop_on: bool,
    pub muted: bool,
    pub gain: f32,
    pub duration_secs: f64,
    /// Analysed beat grid, once the worker has one.
    pub grid: Option<TrackGrid>,
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
    /// Pitch changes keep the key (time stretch on) instead of running the
    /// tape faster.
    pub keylock: bool,
    /// A pointer is on the waveform.
    pub scratching: bool,
    /// Three-band tone control, 1.0 = unity, 0.0 = killed.
    pub eq: [f32; 3],
    /// Which bands the kill buttons are holding down.
    pub eq_kill: [bool; 3],
    /// Bipolar sweep filter; 0.5 = off.
    pub filter: f32,
    /// Per-stem gains, in [`crate::music_dsp::StemKind`] order.
    pub stem_gain: [f32; STEM_COUNT],
    pub stem_kill: [bool; STEM_COUNT],
    /// Separated stems are loaded and the stem knobs are live.
    pub stems_ready: bool,
    /// Generation of the load currently on this deck: late-arriving
    /// analysis for an older load is dropped.
    pub load_gen: DeckGen,
}

impl Default for DeckState {
    fn default() -> Self {
        Self {
            load: DeckLoad::Empty,
            playing: false,
            loop_on: false,
            muted: false,
            gain: 1.0,
            duration_secs: 0.0,
            grid: None,
            position_secs: 0.0,
            rate: 1.0,
            pitch: 0.0,
            pitch_range: PitchRange::Narrow,
            synced: false,
            ext_sync: false,
            auto_opt_out: false,
            keylock: true,
            scratching: false,
            eq: [1.0; 3],
            eq_kill: [false; 3],
            filter: 0.5,
            stem_gain: [1.0; STEM_COUNT],
            stem_kill: [false; STEM_COUNT],
            stems_ready: false,
            load_gen: 0,
        }
    }
}

impl DeckState {
    pub fn is_loaded(&self) -> bool {
        matches!(self.load, DeckLoad::Loaded { .. })
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
            0.0
        } else {
            self.eq.get(band).copied().unwrap_or(1.0)
        }
    }

    /// The gain a stem knob resolves to. With no stems loaded the knobs are
    /// inert and the deck plays the full mix.
    pub fn stem_effective(&self, stem: usize) -> f32 {
        if !self.stems_ready {
            return 1.0;
        }
        if self.stem_kill.get(stem).copied().unwrap_or(false) {
            0.0
        } else {
            self.stem_gain.get(stem).copied().unwrap_or(1.0)
        }
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
    SetLoop { deck: DeckId, loop_on: bool },
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
    /// One tone band, 0 = kill, 1 = unity.
    SetEqBand { deck: DeckId, band: usize, gain: f32 },
    /// Bipolar sweep filter; 0.5 = off.
    SetFilter { deck: DeckId, position: f32 },
    /// One stem lane's gain, 0 = muted.
    SetStemGain { deck: DeckId, stem: usize, gain: f32 },
}

pub struct DeckEngine {
    decks: [DeckState; 2],
    next_gen: DeckGen,
    /// Crossfader position intent (0 = A, 1 = B).
    pub crossfader: f32,
    pub curve: FadeCurve,
    /// Deck that most recently received a load, for Auto tie-breaks.
    last_loaded: Option<DeckId>,
    /// Hold the non-leading deck to the leader's grid without being asked.
    pub auto_sync: bool,
    /// Tracks queued for the next free deck, in play order.
    queue: Vec<TrackItem>,
    /// Fill an idle deck from the queue as soon as one frees up.
    pub auto_load_queue: bool,
}

impl Default for DeckEngine {
    fn default() -> Self {
        Self {
            decks: [DeckState::default(), DeckState::default()],
            next_gen: 0,
            crossfader: 0.0,
            curve: FadeCurve::EqualPower,
            last_loaded: None,
            auto_sync: true,
            queue: Vec::new(),
            auto_load_queue: true,
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

    fn deck_mut(&mut self, id: DeckId) -> &mut DeckState {
        &mut self.decks[id.index()]
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

    /// Route a tile click. The chosen deck starts loading latest-wins; the
    /// other deck is untouched.
    pub fn click(&mut self, item: TrackItem, target: DeckTarget) -> Vec<DeckCmd> {
        let deck = match target {
            DeckTarget::A => DeckId::A,
            DeckTarget::B => DeckId::B,
            DeckTarget::Auto => self.auto_target(),
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
        state.position_secs = 0.0;
        state.synced = false;
        state.auto_opt_out = false;
        state.stems_ready = false;
        state.scratching = false;
        vec![DeckCmd::LoadTrack { deck, gen, item }]
    }

    /// Decode finished for `(deck, gen)`. Stale generations are dropped.
    pub fn track_ready(&mut self, deck: DeckId, gen: DeckGen, duration_secs: f64) -> Vec<DeckCmd> {
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
        // Fresh installs inherit the whole standing channel-strip intent:
        // transport, tone, stems and the rate the pitch slider is sitting at.
        let mut cmds = vec![
            DeckCmd::InstallTrack { deck },
            DeckCmd::SetLoop { deck, loop_on: state.loop_on },
            DeckCmd::SetMute { deck, muted: state.muted },
            DeckCmd::SetGain { deck, gain: state.gain },
            DeckCmd::SetKeylock { deck, on: state.keylock },
            DeckCmd::SetRate { deck, rate: state.rate },
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

    pub fn toggle_loop(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        state.loop_on = !state.loop_on;
        vec![DeckCmd::SetLoop { deck, loop_on: state.loop_on }]
    }

    pub fn toggle_mute(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        let state = self.deck_mut(deck);
        state.muted = !state.muted;
        vec![DeckCmd::SetMute { deck, muted: state.muted }]
    }

    pub fn set_gain(&mut self, deck: DeckId, gain: f32) -> Vec<DeckCmd> {
        let gain = gain.clamp(0.0, 1.5);
        self.deck_mut(deck).gain = gain;
        vec![DeckCmd::SetGain { deck, gain }]
    }

    pub fn set_crossfader(&mut self, position: f32) -> Vec<DeckCmd> {
        self.crossfader = position.clamp(0.0, 1.0);
        vec![DeckCmd::SetCrossfader { position: self.crossfader }]
    }

    /// Timed move to one side (the "fade to A/B" performance buttons).
    pub fn fade_to(&mut self, deck: DeckId, secs: f32) -> Vec<DeckCmd> {
        let position = match deck {
            DeckId::A => 0.0,
            DeckId::B => 1.0,
        };
        self.crossfader = position;
        vec![DeckCmd::FadeCrossfader { position, secs: secs.max(0.0) }]
    }

    pub fn set_curve(&mut self, curve: FadeCurve) -> Vec<DeckCmd> {
        self.curve = curve;
        vec![DeckCmd::SetCurve { curve }]
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

    /// The deck the other one should follow: whichever is audibly leading.
    /// A playing deck beats a stopped one; when both play, the side the
    /// crossfader favours leads.
    pub fn sync_leader(&self) -> Option<DeckId> {
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
        if !state.scratching {
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

    /// What the deck's SYNC control currently reads.
    pub fn sync_mode(&self, deck: DeckId) -> SyncMode {
        let state = self.deck(deck);
        match (state.ext_sync, state.synced) {
            (true, _) => SyncMode::External,
            (false, true) => SyncMode::Deck,
            (false, false) => SyncMode::Off,
        }
    }

    /// Any deck following the room. While one is, the loopback detector is
    /// the thing that knows where the beat is, so it must NOT be parked.
    pub fn any_external_sync(&self) -> bool {
        self.decks.iter().any(|state| state.ext_sync)
    }

    /// The SYNC control cycles OFF → SYNC → EXT → OFF.
    pub fn cycle_sync(&mut self, deck: DeckId) -> Vec<DeckCmd> {
        match self.sync_mode(deck) {
            SyncMode::Off => self.sync(deck, true),
            SyncMode::Deck => {
                let state = self.deck_mut(deck);
                state.synced = false;
                state.ext_sync = true;
                // Nothing to do until the next external target arrives.
                Vec::new()
            }
            SyncMode::External => {
                let state = self.deck_mut(deck);
                state.ext_sync = false;
                state.auto_opt_out = true;
                Vec::new()
            }
        }
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
            let Some(view) = state.sync_view() else { continue };
            let envelope = state.pitch_range.fraction();
            let Some(follow) = external_follow(external, &view, envelope) else { continue };
            let state = self.deck_mut(deck);
            if (state.rate - follow.rate).abs() > 1e-4 {
                state.rate = follow.rate;
                state.pitch = (follow.rate - 1.0).clamp(-0.5, 0.5);
                cmds.push(DeckCmd::SetRate { deck, rate: follow.rate });
            }
            if let Some(secs) = follow.reseek_secs {
                state.position_secs = secs;
                cmds.push(DeckCmd::SeekSeconds { deck, secs });
            }
        }
        cmds
    }

    pub fn set_auto_sync(&mut self, on: bool) -> Vec<DeckCmd> {
        self.auto_sync = on;
        if !on {
            for index in 0..2 {
                self.decks[index].synced = false;
            }
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
        let state = self.deck_mut(deck);
        state.pitch = pitch;
        state.rate = rate;
        state.synced = false;
        state.auto_opt_out = true;
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

    pub fn dequeue(&mut self, index: usize) {
        if index < self.queue.len() {
            self.queue.remove(index);
        }
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

    /// Load the head of the queue onto a free deck, if there is one of each.
    pub fn pump_queue(&mut self) -> Vec<DeckCmd> {
        if self.queue.is_empty() {
            return Vec::new();
        }
        let Some(deck) = self.free_deck() else {
            return Vec::new();
        };
        let item = self.queue.remove(0);
        let target = match deck {
            DeckId::A => DeckTarget::A,
            DeckId::B => DeckTarget::B,
        };
        self.click(item, target)
    }

    /// Load a queued track straight onto a deck now (a click in the queue).
    pub fn load_queued(&mut self, index: usize, target: DeckTarget) -> Vec<DeckCmd> {
        if index >= self.queue.len() {
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
    fn ready_install_carries_standing_loop_mute_gain_intent() {
        let mut e = DeckEngine::new();
        e.toggle_loop(DeckId::B);
        e.toggle_mute(DeckId::B);
        e.set_gain(DeckId::B, 0.5);
        let (d, g) = load_gen(&e.click(item(3), DeckTarget::B));
        let cmds = e.track_ready(d, g, 20.0);
        assert!(cmds.contains(&DeckCmd::SetLoop { deck: DeckId::B, loop_on: true }));
        assert!(cmds.contains(&DeckCmd::SetMute { deck: DeckId::B, muted: true }));
        assert!(cmds.contains(&DeckCmd::SetGain { deck: DeckId::B, gain: 0.5 }));
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
    fn the_sync_control_cycles_off_deck_ext_off() {
        let mut e = DeckEngine::new();
        let (deck, gen) = load_gen(&e.click(item(1), DeckTarget::A));
        e.track_ready(deck, gen, 120.0);
        assert_eq!(e.sync_mode(DeckId::A), SyncMode::Off);
        assert!(!e.any_external_sync());
        // With nothing to sync to, the first press still moves the control on
        // to EXT on the next click — the mode is the operator's, not the
        // other deck's.
        e.deck_mut(DeckId::A).synced = true;
        assert_eq!(e.sync_mode(DeckId::A), SyncMode::Deck);
        e.cycle_sync(DeckId::A);
        assert_eq!(e.sync_mode(DeckId::A), SyncMode::External);
        assert!(e.any_external_sync(), "the detector must stay awake for this");
        e.cycle_sync(DeckId::A);
        assert_eq!(e.sync_mode(DeckId::A), SyncMode::Off);
        assert!(!e.any_external_sync());
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
}
