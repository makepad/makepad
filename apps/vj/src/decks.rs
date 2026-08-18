//! Two DJ music decks + central crossfader: pure routing/state engine.
//!
//! The engine owns deck INTENT (what is loading/loaded where, transport
//! mirrors, crossfader position/curve); actual sample playback lives in the
//! mixer, driven by the commands returned here. Everything is deterministic
//! and clock-free so the whole surface is hermetically testable:
//!
//! - tile clicks route to an explicit deck or, on `Auto`, to the inactive
//!   deck — never interrupting the live deck,
//! - per-deck loads are latest-wins by generation; stale decode completions
//!   are ignored,
//! - the crossfader is equal-power (`cos/sin` quarter-cycle) by default,
//!   with a linear curve option and timed fade-to-side moves.

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

/// What a music tile resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackItem {
    pub asset: AssetId,
    pub revision: AssetRevisionId,
    pub title: String,
    pub media_blob: BlobId,
    pub media_len: u64,
    pub media: MediaType,
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

/// Transport mirror of one deck. `playing/loop/mute/gain` echo what the
/// mixer was last told; the mixer's device clock stays the position truth.
#[derive(Clone, Debug)]
pub struct DeckState {
    pub load: DeckLoad,
    pub playing: bool,
    pub loop_on: bool,
    pub muted: bool,
    pub gain: f32,
    pub duration_secs: f64,
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
}

pub struct DeckEngine {
    decks: [DeckState; 2],
    next_gen: DeckGen,
    /// Crossfader position intent (0 = A, 1 = B).
    pub crossfader: f32,
    pub curve: FadeCurve,
    /// Deck that most recently received a load, for Auto tie-breaks.
    last_loaded: Option<DeckId>,
}

impl Default for DeckEngine {
    fn default() -> Self {
        Self {
            decks: [DeckState::default(), DeckState::default()],
            next_gen: 0,
            crossfader: 0.0,
            curve: FadeCurve::EqualPower,
            last_loaded: None,
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
        self.deck_mut(deck).load = DeckLoad::Loading { gen, item: item.clone() };
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
        // Fresh installs inherit the deck's standing loop/mute/gain intent.
        vec![
            DeckCmd::InstallTrack { deck },
            DeckCmd::SetLoop { deck, loop_on: state.loop_on },
            DeckCmd::SetMute { deck, muted: state.muted },
            DeckCmd::SetGain { deck, gain: state.gain },
        ]
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
        vec![DeckCmd::SetPlaying { deck, playing: state.playing }]
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

    /// Mixer reports a deck ran off the end with looping off.
    pub fn track_ended(&mut self, deck: DeckId) {
        let state = self.deck_mut(deck);
        if matches!(state.load, DeckLoad::Loaded { .. }) {
            state.playing = false;
        }
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
}
