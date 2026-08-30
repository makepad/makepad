//! The whole piano sound as one value.
//!
//! Everything the sound panel offers — the instrument preset, the six voicing
//! amounts, the output EQ and the room — is one plain `Copy` struct. The UI
//! edits it, [`crate::playback::PlaybackBridge::set_sound`] publishes it into
//! the shared cell, and the audio thread applies it with the setters
//! `makepad_piano_model` documents. Nothing here allocates and nothing here
//! touches the synth.
//!
//! Two kinds of preset change exist, and the difference is `PianoPreset::
//! needs_rebuild`: a voicing+room preset is a value change and travels through
//! the shared cell like any slider; a preset with a construction-time `design`
//! override is a *different instrument* and has to be built (on this thread)
//! and handed to the audio thread whole.

use crate::playback::RoomSettings;
use makepad_piano_model::{PianoPreset, Voicing, PIANO_PRESETS};

/// The deliberate-exaggeration ceiling `Voicing` clamps to. Above 1.0 the
/// sympathetic amount progressively lifts the resonance bed's dampers, which
/// is the whole "play with the dampers off" sound, so the sliders run all the
/// way up rather than stopping at the reference level.
pub const VOICING_MAX: f32 = 2.5;

/// The point on the sympathetic slider above which the dampers are audibly
/// coming off the strings, used only to caption the control honestly.
pub const DAMPERS_LIFTING: f32 = 1.25;

/// Every number that shapes the piano, in one place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundSettings {
    /// Index into [`PIANO_PRESETS`]; the sound this was last set from.
    pub preset: usize,
    pub voicing: Voicing,
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

/// The engine's own defaults for the controls no preset describes: flat EQ,
/// flat tone, unity output, the shipped early-reflection level.
const FLAT_SHELF_HZ: f32 = 6000.0;
const FLAT_BELL_HZ: f32 = 2500.0;
const FLAT_BELL_Q: f32 = 1.0;
const DEFAULT_EARLY: f32 = 0.7;

impl Default for SoundSettings {
    fn default() -> Self {
        let mut settings = Self {
            preset: default_preset_index(),
            voicing: Voicing::default(),
            eq_shelf_db: 0.0,
            eq_shelf_hz: FLAT_SHELF_HZ,
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

/// The instrument the app opens on when nothing is stored.
///
/// The library's own `is_default` is the Concert Grand — the right default for
/// a piano *library*, since it is the reference-matched instrument everything
/// else is a departure from. This application starts somewhere else: the felt
/// piano is the sound people practise and read to, and it is what the user
/// asked to land on.
pub const DEFAULT_PRESET: &str = "Felt Piano";

/// Which shipped preset the app starts on.
pub fn default_preset_index() -> usize {
    preset_index_by_name(DEFAULT_PRESET)
        .or_else(|| PIANO_PRESETS.iter().position(|preset| preset.is_default))
        .unwrap_or(0)
}

/// A preset by its library name. Names are what preferences store, so a
/// shipped list that grows or is reordered never silently changes anyone's
/// instrument; a name that is no longer in the list simply has no index.
pub fn preset_index_by_name(name: &str) -> Option<usize> {
    PIANO_PRESETS.iter().position(|preset| preset.name == name)
}

pub fn preset(index: usize) -> &'static PianoPreset {
    &PIANO_PRESETS[index.min(PIANO_PRESETS.len() - 1)]
}

/// A preset's name without the `(effect)` suffix it carries in the library;
/// the panel says "effect" in its own column instead of inside the name.
pub fn preset_name(index: usize) -> &'static str {
    preset(index).name.trim_end_matches(" (effect)")
}

pub fn preset_is_effect(index: usize) -> bool {
    preset(index).name.ends_with("(effect)")
}

impl SoundSettings {
    /// Adopt a preset whole: its voicing, its suggested room and reverb
    /// amount, and a clean slate for the engineer's trim on top. The
    /// listening position is deliberately kept — where you are sitting is not
    /// a property of the instrument.
    pub fn apply_preset(&mut self, index: usize) {
        let index = index.min(PIANO_PRESETS.len() - 1);
        let preset = preset(index);
        self.preset = index;
        self.voicing = preset.voicing;
        self.room.preset = preset.room;
        self.room.mix = preset.reverb_mix;
        self.early_reflections = DEFAULT_EARLY;
        self.eq_shelf_db = 0.0;
        self.eq_shelf_hz = FLAT_SHELF_HZ;
        self.eq_bell_hz = FLAT_BELL_HZ;
        self.eq_bell_db = 0.0;
        self.eq_bell_q = FLAT_BELL_Q;
        self.tone_bass_db = 0.0;
        self.tone_treble_db = 0.0;
        self.master_gain = 1.0;
    }

    /// The settings this preset would produce, for comparison.
    pub fn from_preset(index: usize, room: RoomSettings) -> Self {
        let mut settings = Self {
            room,
            ..Self::default()
        };
        settings.apply_preset(index);
        settings
    }

    /// True while every control still sits where the preset put it.
    pub fn matches_preset(&self) -> bool {
        *self == Self::from_preset(self.preset, self.room)
    }

    /// The controls the user has moved away from the preset.
    pub fn diverged(&self) -> impl Iterator<Item = SoundParam> + '_ {
        let reference = Self::from_preset(self.preset, self.room);
        SoundParam::ALL
            .into_iter()
            .filter(move |param| param.get(self) != param.get(&reference))
    }

    pub fn is_diverged(&self, param: SoundParam) -> bool {
        let reference = Self::from_preset(self.preset, self.room);
        param.get(self) != param.get(&reference)
    }

    /// True when this preset describes a different instrument rather than a
    /// different voicing of the reference one — it has to be rebuilt.
    pub fn preset_needs_rebuild(&self) -> bool {
        preset(self.preset).needs_rebuild()
    }
}

/// One continuous control of the sound panel.
///
/// Every variant is backed by a documented `makepad_piano_model` setter, so a
/// slider that exists is a slider that reaches the audio thread. The panel
/// draws them straight from [`SoundParam::ALL`]: label, range, formatting and
/// the slider mapping all live here, not in the shell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoundParam {
    // Character: the runtime mechanism mix (Voicing).
    BodyTap,
    Knock,
    Roughness,
    Phantoms,
    AttackNoise,
    Sympathetic,
    // Tone: the output EQ and trim.
    ShelfDb,
    ShelfHz,
    BellHz,
    BellDb,
    BellQ,
    ToneBass,
    ToneTreble,
    MasterGain,
    // Room.
    ReverbMix,
    EarlyReflections,
}

/// How a slider's 0..1 travel maps onto the parameter's range. Frequencies and
/// Q are logarithmic; a linear 200 Hz..12 kHz slider spends 85% of its travel
/// above 2 kHz and is unusable in the range that matters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scale {
    Linear,
    Log,
}

/// How the value is written next to its slider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Unit {
    /// A multiplier on the reference-matched level.
    Amount,
    Decibels,
    Hertz,
    Quality,
    Percent,
}

impl SoundParam {
    pub const ALL: [Self; 16] = [
        Self::BodyTap,
        Self::Knock,
        Self::Roughness,
        Self::Phantoms,
        Self::AttackNoise,
        Self::Sympathetic,
        Self::ShelfDb,
        Self::ShelfHz,
        Self::BellHz,
        Self::BellDb,
        Self::BellQ,
        Self::ToneBass,
        Self::ToneTreble,
        Self::MasterGain,
        Self::ReverbMix,
        Self::EarlyReflections,
    ];

    /// The mechanism sliders, in the order the panel groups them.
    pub const CHARACTER: [Self; 6] = [
        Self::BodyTap,
        Self::Knock,
        Self::Roughness,
        Self::Phantoms,
        Self::AttackNoise,
        Self::Sympathetic,
    ];
    pub const TONE: [Self; 8] = [
        Self::ShelfDb,
        Self::ShelfHz,
        Self::BellHz,
        Self::BellDb,
        Self::BellQ,
        Self::ToneBass,
        Self::ToneTreble,
        Self::MasterGain,
    ];
    pub const ROOM: [Self; 2] = [Self::ReverbMix, Self::EarlyReflections];

    pub const fn label(self) -> &'static str {
        match self {
            Self::BodyTap => "Body tap",
            Self::Knock => "Hammer knock",
            Self::Roughness => "Contact grit",
            Self::Phantoms => "Phantom partials",
            Self::AttackNoise => "Key & action noise",
            Self::Sympathetic => "Sympathetic strings",
            Self::ShelfDb => "Air shelf",
            Self::ShelfHz => "Shelf corner",
            Self::BellHz => "Presence centre",
            Self::BellDb => "Presence",
            Self::BellQ => "Presence width",
            Self::ToneBass => "Bass",
            Self::ToneTreble => "Treble",
            Self::MasterGain => "Output",
            Self::ReverbMix => "Reverb",
            Self::EarlyReflections => "Early reflections",
        }
    }

    /// One line saying what moving this actually does to the instrument.
    pub const fn hint(self) -> &'static str {
        match self {
            Self::BodyTap => "Diffuse wooden body in the attack. 0 gives a digital-clean strike.",
            Self::Knock => "The blow reaching the bridge — the percussive front edge.",
            Self::Roughness => "Hammer contact roughness: fortissimo grit.",
            Self::Phantoms => "Longitudinal string modes: the bass's metallic sheen.",
            Self::AttackNoise => "Key-bottom thump and action resonance.",
            Self::Sympathetic => "Open strings, damped-string coupling and the duplex scale. Above 1.0 the dampers come off.",
            Self::ShelfDb => "Treble shelf on the output.",
            Self::ShelfHz => "Where the treble shelf starts.",
            Self::BellHz => "Centre of the parametric presence bell.",
            Self::BellDb => "Cut or lift at the presence centre.",
            Self::BellQ => "How narrow the presence bell is.",
            Self::ToneBass => "Gentle output shelf at 120 Hz.",
            Self::ToneTreble => "Gentle output shelf at 6 kHz.",
            Self::MasterGain => "Output level on the calibrated instrument.",
            Self::ReverbMix => "Tail send on top of the dry piano — 0 is the dry instrument.",
            Self::EarlyReflections => "Lid and wall slapback: the room's size cue.",
        }
    }

    /// Inclusive `(min, max)` in the parameter's own unit.
    pub const fn range(self) -> (f32, f32) {
        match self {
            Self::BodyTap
            | Self::Knock
            | Self::Roughness
            | Self::Phantoms
            | Self::AttackNoise
            | Self::Sympathetic => (0.0, VOICING_MAX),
            Self::ShelfDb | Self::BellDb => (-24.0, 12.0),
            Self::ShelfHz => (1000.0, 16000.0),
            Self::BellHz => (200.0, 12000.0),
            Self::BellQ => (0.3, 8.0),
            Self::ToneBass | Self::ToneTreble => (-12.0, 12.0),
            Self::MasterGain => (0.0, 2.0),
            Self::ReverbMix => (0.0, 1.0),
            Self::EarlyReflections => (0.0, 1.5),
        }
    }

    /// The value the preset/engine considers neutral, drawn as a tick.
    pub const fn neutral(self) -> f32 {
        match self {
            Self::BodyTap
            | Self::Knock
            | Self::Roughness
            | Self::Phantoms
            | Self::AttackNoise
            | Self::Sympathetic
            | Self::MasterGain => 1.0,
            Self::ShelfHz => FLAT_SHELF_HZ,
            Self::BellHz => FLAT_BELL_HZ,
            Self::BellQ => FLAT_BELL_Q,
            Self::EarlyReflections => DEFAULT_EARLY,
            _ => 0.0,
        }
    }

    const fn scale(self) -> Scale {
        match self {
            Self::ShelfHz | Self::BellHz | Self::BellQ => Scale::Log,
            _ => Scale::Linear,
        }
    }

    const fn unit(self) -> Unit {
        match self {
            Self::ShelfDb | Self::BellDb | Self::ToneBass | Self::ToneTreble => Unit::Decibels,
            Self::ShelfHz | Self::BellHz => Unit::Hertz,
            Self::BellQ => Unit::Quality,
            Self::ReverbMix => Unit::Percent,
            _ => Unit::Amount,
        }
    }

    pub fn get(self, settings: &SoundSettings) -> f32 {
        match self {
            Self::BodyTap => settings.voicing.body_tap,
            Self::Knock => settings.voicing.knock,
            Self::Roughness => settings.voicing.roughness,
            Self::Phantoms => settings.voicing.phantoms,
            Self::AttackNoise => settings.voicing.attack_noise,
            Self::Sympathetic => settings.voicing.sympathetic,
            Self::ShelfDb => settings.eq_shelf_db,
            Self::ShelfHz => settings.eq_shelf_hz,
            Self::BellHz => settings.eq_bell_hz,
            Self::BellDb => settings.eq_bell_db,
            Self::BellQ => settings.eq_bell_q,
            Self::ToneBass => settings.tone_bass_db,
            Self::ToneTreble => settings.tone_treble_db,
            Self::MasterGain => settings.master_gain,
            Self::ReverbMix => settings.room.mix,
            Self::EarlyReflections => settings.early_reflections,
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
            Self::BodyTap => settings.voicing.body_tap = value,
            Self::Knock => settings.voicing.knock = value,
            Self::Roughness => settings.voicing.roughness = value,
            Self::Phantoms => settings.voicing.phantoms = value,
            Self::AttackNoise => settings.voicing.attack_noise = value,
            Self::Sympathetic => settings.voicing.sympathetic = value,
            Self::ShelfDb => settings.eq_shelf_db = value,
            Self::ShelfHz => settings.eq_shelf_hz = value,
            Self::BellHz => settings.eq_bell_hz = value,
            Self::BellDb => settings.eq_bell_db = value,
            Self::BellQ => settings.eq_bell_q = value,
            Self::ToneBass => settings.tone_bass_db = value,
            Self::ToneTreble => settings.tone_treble_db = value,
            Self::MasterGain => settings.master_gain = value,
            Self::ReverbMix => settings.room.mix = value,
            Self::EarlyReflections => settings.early_reflections = value,
        }
    }

    /// Slider travel (0..=1) for a value.
    pub fn to_position(self, value: f32) -> f64 {
        let (min, max) = self.range();
        let value = value.clamp(min, max) as f64;
        let (min, max) = (min as f64, max as f64);
        match self.scale() {
            Scale::Linear => ((value - min) / (max - min)).clamp(0.0, 1.0),
            Scale::Log => ((value / min).ln() / (max / min).ln()).clamp(0.0, 1.0),
        }
    }

    /// The value a slider at `position` (0..=1) asks for.
    pub fn from_position(self, position: f64) -> f32 {
        let position = if position.is_finite() {
            position.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (min, max) = self.range();
        let (min_f, max_f) = (min as f64, max as f64);
        let value = match self.scale() {
            Scale::Linear => min_f + (max_f - min_f) * position,
            Scale::Log => min_f * (max_f / min_f).powf(position),
        };
        (value as f32).clamp(min, max)
    }

    /// The value as the panel writes it beside the slider.
    pub fn format(self, value: f32) -> String {
        match self.unit() {
            Unit::Amount => format!("{value:.2}"),
            Unit::Decibels => format!("{value:+.1} dB"),
            Unit::Hertz => {
                if value >= 1000.0 {
                    format!("{:.2} kHz", value / 1000.0)
                } else {
                    format!("{value:.0} Hz")
                }
            }
            Unit::Quality => format!("Q {value:.2}"),
            Unit::Percent => format!("{:.0}%", value * 100.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app opens on the felt piano, whole: its voicing, its room, and
    /// nothing trimmed on top. It is a *different instrument* (it carries a
    /// construction-time design override), so the caller has to build it —
    /// which is what `preset_needs_rebuild` is asked here to prove.
    #[test]
    fn the_default_sound_is_the_felt_piano() {
        let settings = SoundSettings::default();
        assert_eq!(preset_name(settings.preset), DEFAULT_PRESET);
        assert_eq!(settings.preset, default_preset_index());
        assert!(settings.matches_preset());
        assert_eq!(settings.diverged().count(), 0);
        assert!(settings.preset_needs_rebuild());
    }

    /// Preferences store a name, so the shipped list may grow or be reordered
    /// without moving anyone's instrument; an unknown name has no index at all
    /// rather than resolving to whatever now sits at that position.
    #[test]
    fn presets_resolve_by_name_and_unknown_names_do_not() {
        for (index, preset) in PIANO_PRESETS.iter().enumerate() {
            assert_eq!(preset_index_by_name(preset.name), Some(index));
        }
        assert_eq!(preset_index_by_name("Harpsichord From Another App"), None);
        assert_eq!(preset_index_by_name(DEFAULT_PRESET), Some(default_preset_index()));
    }

    /// Picking a preset must set the whole sound — voicing *and* the room it
    /// suggests — and leave every slider free to move afterwards.
    #[test]
    fn a_preset_sets_voicing_and_room_together_and_divergence_is_visible() {
        let mut settings = SoundSettings::default();
        let cathedral = PIANO_PRESETS
            .iter()
            .position(|preset| preset.name == "Cathedral Wash")
            .expect("the shipped preset list has Cathedral Wash");
        settings.apply_preset(cathedral);
        assert_eq!(settings.voicing, preset(cathedral).voicing);
        assert_eq!(settings.room.preset, preset(cathedral).room);
        assert_eq!(settings.room.mix, preset(cathedral).reverb_mix);
        assert!(settings.matches_preset());

        SoundParam::Knock.set(&mut settings, 2.4);
        assert!(!settings.matches_preset());
        assert!(settings.is_diverged(SoundParam::Knock));
        assert!(!settings.is_diverged(SoundParam::BodyTap));
        assert_eq!(settings.diverged().collect::<Vec<_>>(), vec![SoundParam::Knock]);
    }

    /// The user's "all the dampers off" wash: sympathetic has to run well past
    /// the reference level, not stop at it.
    #[test]
    fn the_sympathetic_slider_reaches_past_the_reference_level() {
        let (min, max) = SoundParam::Sympathetic.range();
        assert_eq!(min, 0.0);
        assert_eq!(max, VOICING_MAX);
        assert!(max > DAMPERS_LIFTING);
        let mut settings = SoundSettings::default();
        SoundParam::Sympathetic.set(&mut settings, 9.0);
        assert_eq!(settings.voicing.sympathetic, VOICING_MAX);
        // And a shipped preset already lives up there, so the range is real.
        let lifted = PIANO_PRESETS
            .iter()
            .find(|preset| preset.name == "Dampers Lifted")
            .expect("the shipped preset list has Dampers Lifted");
        assert!(lifted.voicing.sympathetic > 2.0);
    }

    #[test]
    fn every_parameter_round_trips_through_its_slider_position() {
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
            // Reading a live value and writing it back must not move it.
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

    #[test]
    fn effect_presets_are_named_as_effects_and_the_panel_can_say_so() {
        let effects: Vec<&str> = (0..PIANO_PRESETS.len())
            .filter(|index| preset_is_effect(*index))
            .map(preset_name)
            .collect();
        assert!(effects.contains(&"Wire Cembalo"), "{effects:?}");
        assert!(effects.contains(&"Toy Piano"), "{effects:?}");
        assert!(effects.contains(&"Phantom Metal"), "{effects:?}");
        // The stripped name never keeps the marker.
        assert!(effects.iter().all(|name| !name.contains("effect")));
        // And a plain preset is not marked.
        assert!(!preset_is_effect(default_preset_index()));
    }

    /// Every preset in the shipped list has to be reachable and describable.
    #[test]
    fn every_shipped_preset_has_a_name_and_a_description() {
        assert!(PIANO_PRESETS.len() >= 20);
        for index in 0..PIANO_PRESETS.len() {
            assert!(!preset_name(index).is_empty());
            assert!(!preset(index).description.is_empty());
            let settings = SoundSettings::from_preset(index, RoomSettings::default());
            assert_eq!(settings.preset, index);
            assert!(settings.matches_preset());
        }
    }
}
