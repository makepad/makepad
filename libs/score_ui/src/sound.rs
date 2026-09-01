//! The whole piano sound as one value.
//!
//! The sound panel offers two instruments and two controls, and this is what
//! they edit: one plain `Copy` struct. The UI changes it,
//! [`crate::playback::PlaybackBridge::set_sound`] publishes it into the
//! shared cell, and the audio thread applies it with the setters
//! `makepad_piano_model` documents. Nothing here allocates and nothing here
//! touches the synth.
//!
//! There used to be twenty-one physical presets, six electric ones, seven
//! voicing sliders and a five-control EQ. The mechanisms they drove are all
//! still in `makepad_piano_model` and still reachable from here — the panel
//! simply does not offer them, because one good piano beats thirty choices.

use crate::playback::RoomSettings;
use makepad_piano_model::{
    fx::ReverbPreset, learned::EngineKind, PianoPreset, Voicing, PIANO_PRESETS,
};

/// The synthesis kinds the application can build.
///
/// [`ScoreEngine::Hybrid`] is the physical model with its per-partial targets
/// pulled toward the learned engine's measured ladder (see [`crate::hybrid`]).
/// It is built and its table is baked, but listening said it is worse than
/// either engine it is made from, so it is not in [`ENGINES`] and nothing in
/// the app can select it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScoreEngine {
    /// Strings, hammers and soundboard, simulated.
    Physical,
    /// The physical model, corrected toward what a recorded piano measures.
    Hybrid,
    /// The trained network on its own — the electric voice.
    Learned,
}

impl ScoreEngine {
    /// The `makepad_piano_model` synthesis underneath. Hybrid IS the physical
    /// model — that is the whole point of it.
    pub const fn kind(self) -> EngineKind {
        match self {
            Self::Physical | Self::Hybrid => EngineKind::Physical,
            Self::Learned => EngineKind::Learned,
        }
    }
}

/// The engines an instrument can name. Hybrid is deliberately absent; see
/// [`ScoreEngine`].
pub const ENGINES: [ScoreEngine; 2] = [ScoreEngine::Physical, ScoreEngine::Learned];

/// One instrument in the list.
///
/// The engine is a PROPERTY of the instrument, not a mode the reader chooses
/// first: picking a row routes to the right synthesis by itself. The indices
/// are kept even though each table currently holds one entry, so adding an
/// instrument back is one row in a table and nothing else.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstrumentId {
    /// A modelled acoustic instrument: index into `PIANO_PRESETS`.
    Acoustic(usize),
    /// A learned electric voice: index into [`LEARNED_PRESETS`].
    Electric(usize),
}

impl InstrumentId {
    /// Which engine plays it. The reader never chooses this; it follows.
    pub const fn engine(self) -> ScoreEngine {
        match self {
            Self::Acoustic(_) => ScoreEngine::Physical,
            Self::Electric(_) => ScoreEngine::Learned,
        }
    }
}

/// One row of the instrument list.
pub struct InstrumentEntry {
    pub id: InstrumentId,
    pub name: &'static str,
    pub description: &'static str,
}

/// Every instrument the application can play, in one flat list.
pub fn instrument_list() -> Vec<InstrumentEntry> {
    let mut out = Vec::with_capacity(PIANO_PRESETS.len() + LEARNED_PRESETS.len());
    for (index, preset) in PIANO_PRESETS.iter().enumerate() {
        out.push(InstrumentEntry {
            id: InstrumentId::Acoustic(index),
            name: preset.name,
            description: preset.description,
        });
    }
    for (index, preset) in LEARNED_PRESETS.iter().enumerate() {
        out.push(InstrumentEntry {
            id: InstrumentId::Electric(index),
            name: preset.name,
            description: preset.description,
        });
    }
    out
}

/// The learned engine's electric voices.
///
/// One entry. The engine's voicing amounts are inert and it has no physical
/// design, so everything that can differ between electric voices is the
/// output desk and the room — six names for one instrument in five rooms was
/// not worth the list. This is the clean reference voice, close-miked.
pub const LEARNED_PRESETS: &[ElectricPreset] = &[ElectricPreset {
    name: "Electric Piano",
    description: "Clean, even and bell-like — the learned network, close-miked",
    room: ReverbPreset::Studio,
    reverb_mix: 0.18,
    is_default: true,
}];

/// One entry of the electric family: a room over the one learned voice.
pub struct ElectricPreset {
    pub name: &'static str,
    pub description: &'static str,
    pub room: ReverbPreset,
    pub reverb_mix: f32,
    pub is_default: bool,
}

/// The learned engine stores a voicing and uses none of it, so it carries the
/// reference one rather than pretending to differ.
const LEARNED_VOICING: Voicing = Voicing {
    body_tap: 1.0,
    knock: 1.0,
    roughness: 1.0,
    phantoms: 1.0,
    attack_noise: 1.0,
    attack_body: 0.0,
    sympathetic: 1.0,
};

/// Where the brightness shelf sits.
///
/// Brightness is ONE treble shelf, at a fixed corner, over whatever is
/// playing. 3.5 kHz is where a piano reads as bright or dull rather than airy
/// or dark, and it is the only tone control that reaches BOTH engines — the
/// felt-hardness route would be more physical but would leave the electric
/// voice with a dead slider.
pub const BRIGHTNESS_HZ: f32 = 3500.0;

/// The engine's own defaults for everything the panel no longer offers.
const FLAT_BELL_HZ: f32 = 2500.0;
const FLAT_BELL_Q: f32 = 1.0;
const DEFAULT_EARLY: f32 = 0.7;

/// Every number that shapes the piano, in one place.
///
/// Only `eq_shelf_db` (brightness) and `room` (the room and its dry/wet) are
/// reachable from the panel. The rest are the instrument's own shipped values
/// — kept as fields because the audio thread applies the whole struct in one
/// pass, and because putting a control back means exposing a field that is
/// already published rather than building a path for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundSettings {
    /// Which synthesis is playing.
    pub engine: ScoreEngine,
    /// Index into this engine's instrument table.
    pub preset: usize,
    pub voicing: Voicing,
    /// Brightness: the treble shelf's gain, in dB, at [`BRIGHTNESS_HZ`].
    pub eq_shelf_db: f32,
    pub eq_shelf_hz: f32,
    pub eq_bell_hz: f32,
    pub eq_bell_db: f32,
    pub eq_bell_q: f32,
    pub tone_bass_db: f32,
    pub tone_treble_db: f32,
    pub master_gain: f32,
    pub room: RoomSettings,
    pub early_reflections: f32,
}

impl Default for SoundSettings {
    fn default() -> Self {
        let mut settings = Self {
            engine: ScoreEngine::Physical,
            preset: default_preset_index(ScoreEngine::Physical),
            voicing: Voicing::default(),
            eq_shelf_db: 0.0,
            eq_shelf_hz: BRIGHTNESS_HZ,
            eq_bell_hz: FLAT_BELL_HZ,
            eq_bell_db: 0.0,
            eq_bell_q: FLAT_BELL_Q,
            tone_bass_db: 0.0,
            tone_treble_db: 0.0,
            master_gain: 1.0,
            room: RoomSettings::default(),
            early_reflections: DEFAULT_EARLY,
        };
        settings.apply_preset(settings.preset);
        settings
    }
}

/// Which of the engine's instruments the app starts on.
pub fn default_preset_index(engine: ScoreEngine) -> usize {
    match engine {
        ScoreEngine::Learned => LEARNED_PRESETS.iter().position(|p| p.is_default),
        _ => PIANO_PRESETS.iter().position(|p| p.is_default),
    }
    .unwrap_or(0)
}

/// How many instruments this engine ships.
pub fn preset_count(engine: ScoreEngine) -> usize {
    match engine {
        ScoreEngine::Learned => LEARNED_PRESETS.len(),
        _ => PIANO_PRESETS.len(),
    }
}

/// The line under an instrument's name in the panel.
pub fn preset_description(engine: ScoreEngine, index: usize) -> &'static str {
    match engine {
        ScoreEngine::Learned => LEARNED_PRESETS[index.min(LEARNED_PRESETS.len() - 1)].description,
        _ => PIANO_PRESETS[index.min(PIANO_PRESETS.len() - 1)].description,
    }
}

/// The `PianoPreset` an engine is constructed from. The learned engine has no
/// physical design, so it is built from the shipped one and ignores it.
pub fn build_preset(engine: ScoreEngine, index: usize) -> &'static PianoPreset {
    match engine {
        ScoreEngine::Learned => &PIANO_PRESETS[0],
        _ => &PIANO_PRESETS[index.min(PIANO_PRESETS.len() - 1)],
    }
}

/// An instrument by its name. Names are what preferences store, so the
/// shipped list may grow or be reordered without silently moving anyone's
/// instrument; an unknown name simply has no index.
pub fn preset_index_by_name(engine: ScoreEngine, name: &str) -> Option<usize> {
    match engine {
        ScoreEngine::Learned => LEARNED_PRESETS.iter().position(|p| p.name == name),
        _ => PIANO_PRESETS.iter().position(|p| p.name == name),
    }
}

/// An instrument's name.
pub fn preset_name(engine: ScoreEngine, index: usize) -> &'static str {
    match engine {
        ScoreEngine::Learned => LEARNED_PRESETS[index.min(LEARNED_PRESETS.len() - 1)].name,
        _ => PIANO_PRESETS[index.min(PIANO_PRESETS.len() - 1)].name,
    }
}

impl SoundSettings {
    /// Adopt an instrument whole: its voicing and the room it is heard in.
    ///
    /// Brightness is deliberately NOT reset. It is one control over both
    /// instruments — the engineer's, not the instrument's — so changing
    /// instrument does not silently undo it.
    pub fn apply_preset(&mut self, index: usize) {
        let index = index.min(preset_count(self.engine).saturating_sub(1));
        self.preset = index;
        self.early_reflections = DEFAULT_EARLY;
        self.master_gain = 1.0;
        self.eq_shelf_hz = BRIGHTNESS_HZ;
        self.eq_bell_hz = FLAT_BELL_HZ;
        self.eq_bell_db = 0.0;
        self.eq_bell_q = FLAT_BELL_Q;
        self.tone_bass_db = 0.0;
        self.tone_treble_db = 0.0;
        match self.engine {
            ScoreEngine::Learned => {
                let preset = &LEARNED_PRESETS[index.min(LEARNED_PRESETS.len() - 1)];
                self.voicing = LEARNED_VOICING;
                self.room.preset = preset.room;
                self.room.mix = preset.reverb_mix;
            }
            _ => {
                let preset = &PIANO_PRESETS[index.min(PIANO_PRESETS.len() - 1)];
                self.voicing = preset.voicing;
                self.room.preset = preset.room;
                self.room.mix = preset.reverb_mix;
            }
        }
    }

    /// The settings this instrument would produce, for comparison.
    pub fn from_preset(engine: ScoreEngine, index: usize, room: RoomSettings) -> Self {
        let mut settings = Self {
            engine,
            room,
            ..Self::default()
        };
        settings.apply_preset(index);
        settings
    }
}

/// One continuous control of the sound panel.
///
/// Two of them. Every variant is backed by a documented
/// `makepad_piano_model` setter, so a slider that exists is a slider that
/// reaches the audio thread — and both of these reach BOTH engines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoundParam {
    /// One treble shelf at [`BRIGHTNESS_HZ`]: `set_eq_shelf`.
    Brightness,
    /// Tail send on top of the dry instrument: `set_reverb_mix`.
    Reverb,
}

impl SoundParam {
    pub const ALL: [Self; 2] = [Self::Brightness, Self::Reverb];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Brightness => "Brightness",
            Self::Reverb => "Reverb",
        }
    }

    /// One line saying what moving this actually does.
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Brightness => {
                "Voices the instrument brighter or duller: one treble shelf at 3.5 kHz, \
                 the whole way up and the whole way down. 0 dB is how it is voiced."
            }
            Self::Reverb => {
                "Tail send on top of the dry instrument — 0% is the instrument on its own, \
                 in no room at all."
            }
        }
    }

    /// Inclusive `(min, max)` in the parameter's own unit.
    pub const fn range(self) -> (f32, f32) {
        match self {
            // +/-9 dB is the whole useful travel of a voicing shelf: past it
            // the piano stops sounding like a piano rather than sounding
            // brighter.
            Self::Brightness => (-9.0, 9.0),
            Self::Reverb => (0.0, 1.0),
        }
    }

    /// The value the instrument considers neutral: what a non-finite value
    /// falls back to, and where brightness detents.
    pub const fn neutral(self) -> f32 {
        match self {
            Self::Brightness => 0.0,
            Self::Reverb => 0.0,
        }
    }

    pub fn get(self, settings: &SoundSettings) -> f32 {
        match self {
            Self::Brightness => settings.eq_shelf_db,
            Self::Reverb => settings.room.mix,
        }
    }

    /// Write a value, clamped to this parameter's documented range. A
    /// non-finite value is refused rather than propagated into the engine.
    pub fn set(self, settings: &mut SoundSettings, value: f32) {
        let (min, max) = self.range();
        let value = if value.is_finite() {
            value.clamp(min, max)
        } else {
            self.neutral()
        };
        match self {
            Self::Brightness => {
                // A detent at flat. The shelf bypasses itself below 0.01 dB,
                // so anything inside this band already IS flat; snapping to
                // exactly 0 means the control can be put back where it
                // started with the mouse, and says "+0.0 dB" when it is.
                settings.eq_shelf_db = if value.abs() < 0.1 { 0.0 } else { value };
                settings.eq_shelf_hz = BRIGHTNESS_HZ;
            }
            Self::Reverb => settings.room.mix = value,
        }
    }

    /// Slider travel (0..=1) for a value. Both controls are linear in their
    /// own unit — dB already is a perceptual scale, and a dry/wet is a ratio.
    pub fn to_position(self, value: f32) -> f64 {
        let (min, max) = self.range();
        let value = value.clamp(min, max) as f64;
        let (min, max) = (min as f64, max as f64);
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    }

    /// The value a slider at `position` (0..=1) asks for.
    pub fn from_position(self, position: f64) -> f32 {
        let position = if position.is_finite() {
            position.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (min, max) = self.range();
        let value = min as f64 + (max as f64 - min as f64) * position;
        (value as f32).clamp(min, max)
    }

    /// The value as the panel writes it beside the slider.
    pub fn format(self, value: f32) -> String {
        match self {
            Self::Brightness => format!("{value:+.1} dB"),
            Self::Reverb => format!("{:.0}%", value * 100.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app opens on the shipped physical piano — the instrument the user
    /// approved — with its own room and nothing trimmed on top.
    #[test]
    fn the_default_sound_is_the_shipped_physical_piano() {
        let settings = SoundSettings::default();
        assert_eq!(settings.engine, ScoreEngine::Physical);
        assert_eq!(settings.preset, 0);
        assert_eq!(settings.voicing, PIANO_PRESETS[0].voicing);
        assert_eq!(settings.room.preset, PIANO_PRESETS[0].room);
        assert_eq!(settings.room.mix, PIANO_PRESETS[0].reverb_mix);
        // Flat: the brightness shelf is bypassed at 0 dB, so the default is
        // the instrument itself and not the instrument through an EQ.
        assert_eq!(settings.eq_shelf_db, 0.0);
        assert_eq!(settings.eq_bell_db, 0.0);
        assert_eq!(settings.tone_bass_db, 0.0);
        assert_eq!(settings.tone_treble_db, 0.0);
        assert_eq!(settings.master_gain, 1.0);
    }

    /// The whole model: two instruments, each naming the engine that plays
    /// it, each with something to say about itself.
    #[test]
    fn the_list_is_two_instruments() {
        let list = instrument_list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, InstrumentId::Acoustic(0));
        assert_eq!(list[0].id.engine(), ScoreEngine::Physical);
        assert_eq!(list[1].id, InstrumentId::Electric(0));
        assert_eq!(list[1].id.engine(), ScoreEngine::Learned);
        for entry in &list {
            assert!(!entry.name.is_empty());
            assert!(!entry.description.is_empty());
        }
    }

    /// Preferences store a name, so a list that grows or is reordered never
    /// silently changes anyone's instrument.
    #[test]
    fn instruments_resolve_by_name_and_unknown_names_do_not() {
        for (index, preset) in PIANO_PRESETS.iter().enumerate() {
            assert_eq!(
                preset_index_by_name(ScoreEngine::Physical, preset.name),
                Some(index)
            );
        }
        assert_eq!(
            preset_index_by_name(ScoreEngine::Physical, "Harpsichord From Another App"),
            None
        );
        assert_eq!(
            preset_index_by_name(ScoreEngine::Learned, LEARNED_PRESETS[0].name),
            Some(0)
        );
    }

    /// Choosing the electric voice adopts its room; brightness is the
    /// engineer's and survives the change.
    #[test]
    fn switching_instrument_takes_the_room_and_keeps_brightness() {
        let mut settings = SoundSettings::default();
        SoundParam::Brightness.set(&mut settings, 4.0);
        settings.engine = ScoreEngine::Learned;
        settings.apply_preset(0);
        assert_eq!(settings.room.preset, LEARNED_PRESETS[0].room);
        assert_eq!(settings.room.mix, LEARNED_PRESETS[0].reverb_mix);
        assert_eq!(settings.eq_shelf_db, 4.0);
        assert_eq!(settings.eq_shelf_hz, BRIGHTNESS_HZ);
    }

    /// The centre of the brightness slider is exactly flat, so a reader who
    /// moved it can put it back without typing a number.
    #[test]
    fn brightness_snaps_to_flat_at_the_centre_of_its_travel() {
        let mut settings = SoundSettings::default();
        SoundParam::Brightness.set(&mut settings, SoundParam::Brightness.from_position(0.5));
        assert_eq!(settings.eq_shelf_db, 0.0);
        assert_eq!(SoundParam::Brightness.format(settings.eq_shelf_db), "+0.0 dB");
        // And a real move is left alone.
        SoundParam::Brightness.set(&mut settings, 4.5);
        assert_eq!(settings.eq_shelf_db, 4.5);
    }

    #[test]
    fn both_controls_round_trip_through_their_slider_position() {
        for param in SoundParam::ALL {
            let (min, max) = param.range();
            for step in 0..=20 {
                let position = step as f64 / 20.0;
                let value = param.from_position(position);
                assert!(value >= min && value <= max, "{param:?} left its range");
                let back = param.to_position(value);
                assert!(
                    (back - position).abs() < 1.0e-4,
                    "{param:?} at {position} came back as {back}"
                );
            }
            let mut settings = SoundSettings::default();
            param.set(&mut settings, param.neutral());
            let held = param.get(&settings);
            assert_eq!(
                param.from_position(param.to_position(held)),
                held,
                "{param:?} does not survive a redraw"
            );
        }
    }

    #[test]
    fn a_non_finite_value_never_reaches_the_engine() {
        let mut settings = SoundSettings::default();
        for param in SoundParam::ALL {
            param.set(&mut settings, f32::NAN);
            assert_eq!(param.get(&settings), param.neutral());
            param.set(&mut settings, f32::INFINITY);
            assert!(param.get(&settings).is_finite());
        }
    }
}

/// The claims the sound panel makes, checked against the engines themselves.
#[cfg(test)]
mod engine_truth {
    use super::*;
    use makepad_piano_model::{learned::PianoEngine, PianoEvent, TimedEvent};

    const RATE: f32 = 48_000.0;
    const TAIL: usize = 24_000;

    fn build_for_test(engine: ScoreEngine, preset: &PianoPreset) -> PianoEngine {
        match engine {
            ScoreEngine::Hybrid => {
                let mut piano = makepad_piano_model::Piano::new_with_preset(RATE, preset);
                crate::hybrid::apply_targets(&mut piano);
                PianoEngine::Physical(Box::new(piano))
            }
            other => PianoEngine::new(other.kind(), RATE, preset),
        }
    }

    fn peak_difference(a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0f32, f32::max)
    }

    /// One note through a freshly built engine, with the published settings
    /// applied the way the audio thread applies them.
    fn voice(id: InstrumentId, tweak: impl FnOnce(&mut SoundSettings)) -> Vec<f32> {
        let engine = id.engine();
        let index = match id {
            InstrumentId::Acoustic(index) | InstrumentId::Electric(index) => index,
        };
        let mut settings = SoundSettings {
            engine,
            ..SoundSettings::default()
        };
        settings.apply_preset(index);
        tweak(&mut settings);
        let mut piano = build_for_test(engine, build_preset(engine, index));
        piano.set_voicing(settings.voicing);
        piano.set_reverb_preset(settings.room.preset);
        piano.set_reverb_mix(settings.room.mix);
        piano.set_eq_shelf(settings.eq_shelf_db, settings.eq_shelf_hz);
        piano.set_eq_bell(settings.eq_bell_hz, settings.eq_bell_db, settings.eq_bell_q);
        piano.set_tone(settings.tone_bass_db, settings.tone_treble_db);
        let events = [TimedEvent {
            offset: 0,
            event: PianoEvent::NoteOn {
                key: 60,
                velocity: 96,
            },
        }];
        let (mut left, mut right) = (vec![0.0f32; TAIL], vec![0.0f32; TAIL]);
        piano.process(&events, &mut left, &mut right);
        left
    }

    /// Brightness is one control over BOTH instruments — that is why it is a
    /// shelf and not the model's felt hardness, which the learned engine has
    /// no equivalent of. Measured on each, not assumed.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "renders four takes; run with --release")]
    fn brightness_reaches_both_instruments_and_is_flat_at_zero() {
        for id in [InstrumentId::Acoustic(0), InstrumentId::Electric(0)] {
            let flat = voice(id, |_| {});
            let bright = voice(id, |s| SoundParam::Brightness.set(s, 9.0));
            let dull = voice(id, |s| SoundParam::Brightness.set(s, -9.0));
            assert!(
                peak_difference(&flat, &bright) > 1.0e-3,
                "{id:?}: brightness must bite upward"
            );
            assert!(
                peak_difference(&flat, &dull) > 1.0e-3,
                "{id:?}: brightness must bite downward"
            );
            // And the default is the bare instrument: the shelf is bypassed
            // at 0 dB, so a take at 0 dB is the take with no EQ at all.
            let untouched = voice(id, |s| s.eq_shelf_db = 0.0);
            assert_eq!(
                peak_difference(&flat, &untouched),
                0.0,
                "{id:?}: 0 dB brightness is not a bypass"
            );
        }
    }

    /// THE approved instrument, sample for sample.
    ///
    /// The user approved the physical model at its shipped default, and this
    /// application must not quietly become a different piano. So: render the
    /// bare instrument, then render it again through the whole published
    /// settings path the audio thread uses (`PlaybackBridge::set_sound` ->
    /// `apply_sound`), and require the two to be BIT-IDENTICAL. Any control
    /// whose default is not a true no-op — a shelf that filters at 0 dB, a
    /// tone stage that is not bypassed flat, a gain that is not unity —
    /// fails here rather than in someone's ears.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "renders two takes; run with --release")]
    fn the_default_settings_are_the_bare_instrument() {
        let settings = SoundSettings::default();
        assert_eq!(settings.engine, ScoreEngine::Physical);
        let events = [TimedEvent {
            offset: 0,
            event: PianoEvent::NoteOn {
                key: 60,
                velocity: 96,
            },
        }];

        // The instrument as `makepad_piano_model` builds it, untouched.
        let mut bare = makepad_piano_model::Piano::new_with_preset(
            RATE,
            build_preset(ScoreEngine::Physical, settings.preset),
        );
        let (mut bl, mut br) = (vec![0.0f32; TAIL], vec![0.0f32; TAIL]);
        bare.process(&events, &mut bl, &mut br);

        // The same instrument with the app's default settings published onto
        // it, in the order the audio thread applies them.
        let mut played = makepad_piano_model::Piano::new_with_preset(
            RATE,
            build_preset(ScoreEngine::Physical, settings.preset),
        );
        played.set_reverb_preset(settings.room.preset);
        played.set_reverb_mix(settings.room.mix);
        played.set_perspective(settings.room.perspective);
        played.set_early_reflection_level(settings.early_reflections);
        played.set_voicing(settings.voicing);
        played.set_eq_shelf(settings.eq_shelf_db, settings.eq_shelf_hz);
        played.set_eq_bell(settings.eq_bell_hz, settings.eq_bell_db, settings.eq_bell_q);
        played.set_tone(settings.tone_bass_db, settings.tone_treble_db);
        played.set_master_gain(settings.master_gain);
        let (mut pl, mut pr) = (vec![0.0f32; TAIL], vec![0.0f32; TAIL]);
        played.process(&events, &mut pl, &mut pr);

        assert_eq!(bl, pl, "the default settings changed the left channel");
        assert_eq!(br, pr, "the default settings changed the right channel");
    }

    /// The two instruments are two instruments, not one name twice.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "renders two takes; run with --release")]
    fn the_two_instruments_are_audibly_different() {
        let acoustic = voice(InstrumentId::Acoustic(0), |s| s.room.mix = 0.0);
        let electric = voice(InstrumentId::Electric(0), |s| s.room.mix = 0.0);
        assert!(peak_difference(&acoustic, &electric) > 1.0e-3);
    }
}
