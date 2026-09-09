//! What the MIDI page decides, as pure rules: which input ports are
//! heard, how the device rows read, and what `midi.txt` holds after a
//! save.
//!
//! No `Cx`, no widgets, no clock: everything here is a function of what
//! the machine reported and what the operator ticked, so every rule is
//! tested on its own and the page in `main.rs` is a view over it. The
//! per-binding rules -- how a learned control reads the number it is
//! sent, what a press does -- land beside these.

use crate::settings::Settings;
use makepad_widgets::*;

/// The surface's ports are heard unless the operator switched them off.
///
/// A named const rather than a bare `#[rust]` on the field: a bare bool
/// is false, and on a fresh data dir the first run writes the
/// constructed value into `midi.txt` before anything reads it back --
/// so the field, the loader's default and the fresh-install test all
/// read this one value.
pub const SURFACE_ON_DEFAULT: bool = true;

/// Every input port to open: the surface's own while it is on, plus any
/// the operator named. The surface's ports are the surface's -- a name
/// that also matches one of them does not open it behind the switch.
pub fn open_input_set(
    ports: &[MidiPortDesc],
    surface: &[MidiPortId],
    surface_on: bool,
    extras: &[String],
    matches: impl Fn(&str, &str) -> bool,
) -> Vec<MidiPortId> {
    let mut open: Vec<MidiPortId> = match surface_on {
        true => surface.to_vec(),
        false => Vec::new(),
    };
    for desc in ports {
        if !desc.port_type.is_input()
            || surface.contains(&desc.port_id)
            || open.contains(&desc.port_id)
        {
            continue;
        }
        if extras.iter().any(|wanted| matches(&desc.name, wanted)) {
            open.push(desc.port_id);
        }
    }
    open
}

/// The device rows: every input port that is not the surface's own, in
/// the machine's order. The surface has a row of its own.
pub fn device_rows<'a>(ports: &'a [MidiPortDesc], surface: &[MidiPortId]) -> Vec<&'a MidiPortDesc> {
    ports
        .iter()
        .filter(|desc| desc.port_type.is_input() && !surface.contains(&desc.port_id))
        .collect()
}

/// Tick or untick a device row. On, the port's whole name joins the list
/// unless an entry already matches it -- a name typed in by hand keeps
/// standing for the port it matched. Off, every entry that matches the
/// row's name leaves: a hand-typed word covering two ports leaves for
/// both, because removing only exact names would leave a row the
/// operator cannot untick. `false` when a tick was refused because the
/// file holds no more names.
pub fn toggle_extra_input(
    extras: &mut Vec<String>,
    name: &str,
    on: bool,
    max: usize,
    matches: impl Fn(&str, &str) -> bool,
) -> bool {
    if on {
        if extras.iter().any(|wanted| matches(name, wanted)) {
            return true;
        }
        if extras.len() >= max {
            return false;
        }
        extras.push(name.to_string());
        return true;
    }
    extras.retain(|wanted| !matches(name, wanted));
    true
}

/// The word a device row shows for whether it is heard, and if not, why.
pub fn device_state_word(passed_over: bool, heard: bool, loopback: bool) -> &'static str {
    match (heard, passed_over, loopback) {
        (true, _, _) => "open",
        (false, true, _) => "surface, passed over",
        (false, false, true) => "loopback",
        (false, false, false) => "closed",
    }
}

/// The whole of what `midi.txt` holds after a save, as a function of the
/// file that is there and the switches.
///
/// Load-or-default first, so a key this build does not know rides
/// through: the store keeps unknown keys, but only if it was handed the
/// file rather than built empty, which is what the save used to do -- a
/// page that rewrites the file at runtime would otherwise drop a newer
/// build's keys on the first tick. Then the device list is rewritten
/// whole, because a shorter list would otherwise leave stale rows and a
/// key set to nothing is still a key. The monitor is the one switch the
/// environment can hold on: while it does, the file's own answer is left
/// exactly as it was, set only if absent.
pub fn merge_midi_settings(
    existing: Option<&str>,
    surface_on: bool,
    soft_takeover: bool,
    monitor: bool,
    monitor_env: bool,
    extras: &[String],
    max: usize,
) -> Settings {
    let mut store = existing.map(Settings::from_text).unwrap_or_default();
    // `Settings::default()` is version 0, which is a file from before the
    // store existed; what is written is this build's.
    store.set_current_version();
    store.set_bool("midi.surface", surface_on);
    store.set_bool("midi.soft_takeover", soft_takeover);
    if !monitor_env || !store.has("midi.monitor") {
        store.set_bool("midi.monitor", monitor);
    }
    for index in 0..max {
        store.remove(&format!("midi.open.{index}"));
    }
    for (index, name) in extras.iter().take(max).enumerate() {
        store.set_text(&format!("midi.open.{index}"), name);
    }
    store
}

// ---------------------------------------------------------------------------
// what a learned control makes of the number it is sent
// ---------------------------------------------------------------------------

/// A binding's source: channel and number, the bare pair the code has
/// always used -- the fader tables and the pick-up rule key on it.
pub type Source = (u8, u8);

/// How a learned knob reads the number it is sent. Plain is what every
/// binding has always done: the number is the position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Transform {
    #[default]
    Plain,
    /// The other way up: a fader mounted upside down, a knob wired backwards.
    Invert,
    /// Down from the middle: a pad or a switch that sends a level.
    Switch,
    /// An endless encoder counting in two's complement: 1..63 clockwise,
    /// 127..65 anticlockwise.
    Relative,
    /// An endless encoder sprung at 64: 65 is one tick clockwise, 63 one
    /// anticlockwise, and a burst past that is squeezed rather than
    /// believed.
    Relative64,
    /// A sprung fader at 64 that reads as a position: 64 is the middle,
    /// both ends reachable.
    Spread64,
}

/// What a message asks of the control: a position, or a turn from where
/// it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    At(f32),
    By(f32),
}

/// A burst past the first tick from a sprung encoder is squeezed by this
/// much: sixty-four plus nine is two steps, not nine. An encoder that
/// reports how hard it was flicked is reporting a hand, not a distance.
pub const BURST_DIVISOR: f32 = 8.0;

/// The sensitivities the page offers, as multiples of one 7-bit step.
pub const SENS_RUNGS: [f32; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

impl Transform {
    /// Page order.
    pub const ALL: [Transform; 6] = [
        Transform::Plain,
        Transform::Invert,
        Transform::Switch,
        Transform::Relative,
        Transform::Relative64,
        Transform::Spread64,
    ];

    /// Read a raw 7-bit number. Sensitivity scales a step and never a
    /// position: a fader is where it is.
    pub fn read(self, raw: u8, sensitivity: f32) -> Motion {
        let raw = raw & 0x7f;
        let step = sensitivity / 127.0;
        match self {
            Transform::Plain => Motion::At(raw as f32 / 127.0),
            Transform::Invert => Motion::At(1.0 - raw as f32 / 127.0),
            Transform::Switch => Motion::At(if raw >= 64 { 1.0 } else { 0.0 }),
            Transform::Relative => {
                let ticks = if raw < 64 { raw as f32 } else { raw as f32 - 128.0 };
                Motion::By(ticks * step)
            }
            Transform::Relative64 => {
                let d = raw as i32 - 64;
                if d == 0 {
                    return Motion::By(0.0);
                }
                let magnitude = d.abs() as f32;
                let ticks = if magnitude <= 1.0 {
                    1.0
                } else {
                    1.0 + (magnitude - 1.0) / BURST_DIVISOR
                };
                Motion::By(ticks * step * d.signum() as f32)
            }
            Transform::Spread64 => {
                Motion::At(((raw as f32 - 64.0) / 126.0 + 0.5).clamp(0.0, 1.0))
            }
        }
    }

    pub fn is_relative(self) -> bool {
        matches!(self, Transform::Relative | Transform::Relative64)
    }

    /// The word the map file holds.
    pub fn word(self) -> &'static str {
        match self {
            Transform::Plain => "plain",
            Transform::Invert => "invert",
            Transform::Switch => "switch",
            Transform::Relative => "relative",
            Transform::Relative64 => "relative64",
            Transform::Spread64 => "spread64",
        }
    }

    pub fn from_word(word: &str) -> Option<Transform> {
        Transform::ALL.into_iter().find(|t| t.word() == word)
    }
}

/// Where a motion lands: a position is itself, a turn moves from where
/// the control is and stops at the ends.
pub fn settle(motion: Motion, current: f32) -> f32 {
    match motion {
        Motion::At(value) => value,
        Motion::By(delta) => (current + delta).clamp(0.0, 1.0),
    }
}

/// The rung nearest a sensitivity, for the picker: a hand-typed value
/// the page does not offer shows as the closest one.
pub fn sens_rung(sensitivity: f32) -> usize {
    let mut best = 0;
    for (index, rung) in SENS_RUNGS.iter().enumerate() {
        if (sensitivity - rung).abs() < (sensitivity - SENS_RUNGS[best]).abs() {
            best = index;
        }
    }
    best
}

/// What a press on a learned button does. Named here because a binding
/// carries it and a learnable button has one of its own; what each one
/// DOES on a press is the press machine's, beside this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behaviour {
    /// Follows the hand: on while held.
    Push,
    /// Flips on the press, ignores the release.
    Toggle,
    /// A tap latches, a hold is momentary.
    TapHold,
    /// A long press latches; the next press lets go.
    LongPress,
    /// Only ever turns it on.
    Trigger,
}

impl Behaviour {
    /// Page order.
    pub const ALL: [Behaviour; 5] = [
        Behaviour::Push,
        Behaviour::Toggle,
        Behaviour::TapHold,
        Behaviour::LongPress,
        Behaviour::Trigger,
    ];

    pub fn word(self) -> &'static str {
        match self {
            Behaviour::Push => "push",
            Behaviour::Toggle => "toggle",
            Behaviour::TapHold => "tap",
            Behaviour::LongPress => "long",
            Behaviour::Trigger => "trigger",
        }
    }

    pub fn from_word(word: &str) -> Option<Behaviour> {
        Behaviour::ALL.into_iter().find(|b| b.word() == word)
    }
}

/// One learned control's record: where it is driven from and what it
/// makes of what arrives. A new binding is Plain at one step with no
/// press of its own, which is exactly the bare pair it used to be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Binding {
    pub source: Source,
    pub transform: Transform,
    pub sensitivity: f32,
    pub press: Option<Behaviour>,
}

impl Binding {
    pub fn new(source: Source) -> Binding {
        Binding { source, transform: Transform::Plain, sensitivity: 1.0, press: None }
    }

    /// The words that follow the three tokens on a map line. Defaults
    /// write nothing, so a binding at its defaults writes the exact line
    /// today's build writes.
    pub fn words(&self) -> String {
        let mut words = Vec::new();
        if self.transform != Transform::Plain {
            words.push(format!("read={}", self.transform.word()));
        }
        if (self.sensitivity - 1.0).abs() > 1e-6 {
            words.push(format!("sens={}", self.sensitivity));
        }
        if let Some(press) = self.press {
            words.push(format!("press={}", press.word()));
        }
        words.join(" ")
    }

    /// Read the words back. An unknown word is ignored, which is how a
    /// line from a newer build reads here; a sensitivity that is not a
    /// finite positive number is one step.
    pub fn with_words<'a>(mut self, words: impl Iterator<Item = &'a str>) -> Binding {
        for word in words {
            let Some((key, value)) = word.split_once('=') else { continue };
            match key {
                "read" => {
                    if let Some(transform) = Transform::from_word(value) {
                        self.transform = transform;
                    }
                }
                "sens" => {
                    self.sensitivity = value
                        .parse::<f32>()
                        .ok()
                        .filter(|s| s.is_finite() && *s > 0.0)
                        .unwrap_or(1.0);
                }
                "press" => {
                    if let Some(press) = Behaviour::from_word(value) {
                        self.press = Some(press);
                    }
                }
                _ => {}
            }
        }
        self
    }
}

/// What kind of thing a learnable is, and for a button, what a press
/// does on its own when its binding has not said otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Learnable {
    Knob,
    Wheel,
    Button(Behaviour),
}

#[cfg(test)]
mod transform_tests {
    use super::*;

    fn at(motion: Motion) -> f32 {
        match motion {
            Motion::At(v) => v,
            Motion::By(_) => panic!("a position was expected, got {motion:?}"),
        }
    }

    fn by(motion: Motion) -> f32 {
        match motion {
            Motion::By(d) => d,
            Motion::At(_) => panic!("a turn was expected, got {motion:?}"),
        }
    }

    #[test]
    fn plain_is_what_it_always_was() {
        for raw in [0u8, 1, 64, 127] {
            assert_eq!(at(Transform::Plain.read(raw, 1.0)), raw as f32 / 127.0);
        }
    }

    #[test]
    fn inverted_reads_the_other_way_up() {
        assert_eq!(at(Transform::Invert.read(0, 1.0)), 1.0);
        assert_eq!(at(Transform::Invert.read(127, 1.0)), 0.0);
        assert!((at(Transform::Invert.read(64, 1.0)) - (1.0 - 64.0 / 127.0)).abs() < 1e-6);
    }

    #[test]
    fn a_switch_is_down_from_the_middle() {
        assert_eq!(at(Transform::Switch.read(63, 1.0)), 0.0);
        assert_eq!(at(Transform::Switch.read(64, 1.0)), 1.0);
        assert_eq!(at(Transform::Switch.read(127, 1.0)), 1.0);
    }

    #[test]
    fn twos_complement_counts_down_from_the_top() {
        let step = 1.0 / 127.0;
        assert!((by(Transform::Relative.read(1, 1.0)) - step).abs() < 1e-6);
        assert!((by(Transform::Relative.read(2, 1.0)) - 2.0 * step).abs() < 1e-6);
        assert!((by(Transform::Relative.read(127, 1.0)) + step).abs() < 1e-6);
        assert!((by(Transform::Relative.read(126, 1.0)) + 2.0 * step).abs() < 1e-6);
        assert_eq!(by(Transform::Relative.read(0, 1.0)), 0.0);
    }

    #[test]
    fn sixty_four_is_rest_and_a_burst_is_compressed() {
        let step = 1.0 / 127.0;
        assert_eq!(by(Transform::Relative64.read(64, 1.0)), 0.0);
        assert!((by(Transform::Relative64.read(65, 1.0)) - step).abs() < 1e-6);
        assert!((by(Transform::Relative64.read(63, 1.0)) + step).abs() < 1e-6);
        assert!((by(Transform::Relative64.read(73, 1.0)) - 2.0 * step).abs() < 1e-6, "64+9 is two steps");
        assert!((by(Transform::Relative64.read(55, 1.0)) + 2.0 * step).abs() < 1e-6);
    }

    #[test]
    fn spread_rests_at_sixty_four_exactly_and_reaches_both_ends() {
        assert_eq!(at(Transform::Spread64.read(64, 1.0)), 0.5);
        assert_eq!(at(Transform::Spread64.read(127, 1.0)), 1.0);
        assert_eq!(at(Transform::Spread64.read(1, 1.0)), 0.0);
        assert_eq!(at(Transform::Spread64.read(0, 1.0)), 0.0, "clamped, not below");
        // Absolute: sensitivity leaves it alone.
        assert_eq!(at(Transform::Spread64.read(127, 4.0)), 1.0);
    }

    #[test]
    fn a_relative_turn_stops_at_the_ends() {
        assert_eq!(settle(Motion::By(0.5), 0.8), 1.0);
        assert_eq!(settle(Motion::By(-0.5), 0.2), 0.0);
        assert!((settle(Motion::By(0.1), 0.5) - 0.6).abs() < 1e-6);
        assert_eq!(settle(Motion::At(0.3), 0.9), 0.3);
    }

    #[test]
    fn sensitivity_scales_a_step_and_never_a_position() {
        assert!((by(Transform::Relative.read(1, 0.5)) - 0.5 / 127.0).abs() < 1e-6);
        assert!((by(Transform::Relative64.read(65, 4.0)) - 4.0 / 127.0).abs() < 1e-6);
        assert_eq!(at(Transform::Plain.read(127, 0.5)), 1.0);
        assert_eq!(at(Transform::Invert.read(0, 4.0)), 1.0);
    }

    #[test]
    fn the_nearest_rung_is_shown_for_a_hand_typed_sensitivity() {
        assert_eq!(sens_rung(1.0), 2);
        assert_eq!(sens_rung(0.3), 0);
        assert_eq!(sens_rung(0.7), 1);
        assert_eq!(sens_rung(3.0), 3);
        assert_eq!(sens_rung(40.0), 4);
    }

    #[test]
    fn words_round_trip_and_unknown_words_are_kept_out_of_the_way() {
        let mut binding = Binding::new((0, 14));
        binding.transform = Transform::Relative64;
        binding.sensitivity = 0.5;
        binding.press = Some(Behaviour::TapHold);
        let words = binding.words();
        assert_eq!(words, "read=relative64 sens=0.5 press=tap");
        let back = Binding::new((0, 14)).with_words(words.split_whitespace());
        assert_eq!(back, binding);
        let stranger = Binding::new((0, 14)).with_words("colour=red read=invert sens=nope".split_whitespace());
        assert_eq!(stranger.transform, Transform::Invert);
        assert_eq!(stranger.sensitivity, 1.0, "a sensitivity that is not a number is one step");
        for transform in Transform::ALL {
            assert_eq!(Transform::from_word(transform.word()), Some(transform));
        }
        for press in Behaviour::ALL {
            assert_eq!(Behaviour::from_word(press.word()), Some(press));
        }
    }

    #[test]
    fn a_bare_line_from_todays_build_reads_as_plain_at_one() {
        let binding = Binding::new((3, 74)).with_words(std::iter::empty());
        assert_eq!(binding, Binding::new((3, 74)));
        assert_eq!(binding.transform, Transform::Plain);
        assert_eq!(binding.sensitivity, 1.0);
        assert_eq!(binding.press, None);
    }

    #[test]
    fn a_default_binding_writes_the_line_it_always_wrote() {
        assert_eq!(Binding::new((3, 74)).words(), "", "no words: the three tokens alone");
    }
}

#[cfg(test)]
mod port_tests {
    use super::*;

    fn port(name: &str, id: u64, input: bool) -> MidiPortDesc {
        MidiPortDesc {
            name: name.to_string(),
            port_id: MidiPortId(LiveId(id)),
            port_type: if input { MidiPortType::Input } else { MidiPortType::Output },
        }
    }

    /// The matching the console uses: case and spacing folded, substring.
    fn matches(name: &str, wanted: &str) -> bool {
        let fold = |text: &str| -> String {
            text.chars().filter(|c| c.is_ascii_alphanumeric()).flat_map(char::to_lowercase).collect()
        };
        let (name, wanted) = (fold(name), fold(wanted));
        !wanted.is_empty() && name.contains(&wanted)
    }

    fn rig() -> (Vec<MidiPortDesc>, Vec<MidiPortId>) {
        let ports = vec![
            port("Surface In", 1, true),
            port("Surface Out", 2, false),
            port("house keys", 3, true),
            port("Midi Through", 4, true),
            port("keys out", 5, false),
        ];
        (ports, vec![MidiPortId(LiveId(1))])
    }

    #[test]
    fn the_surface_is_heard_unless_switched_off() {
        let (ports, surface) = rig();
        let on = open_input_set(&ports, &surface, true, &[], matches);
        assert_eq!(on, vec![MidiPortId(LiveId(1))]);
        let off = open_input_set(&ports, &surface, false, &[], matches);
        assert!(off.is_empty(), "off means not heard: {off:?}");
        // Naming the surface's own port does not open it behind the switch.
        let named = open_input_set(&ports, &surface, false, &["surface".to_string()], matches);
        assert!(named.is_empty(), "{named:?}");
    }

    #[test]
    fn a_port_the_operator_ticked_is_heard_and_one_they_did_not_is_not() {
        let (ports, surface) = rig();
        let open = open_input_set(&ports, &surface, true, &["house keys".to_string()], matches);
        assert_eq!(open, vec![MidiPortId(LiveId(1)), MidiPortId(LiveId(3))]);
        assert!(!open.contains(&MidiPortId(LiveId(4))), "the loopback was not asked for");
        assert!(!open.contains(&MidiPortId(LiveId(5))), "an output is never an input");
    }

    #[test]
    fn a_surface_port_is_never_listed_twice() {
        let (ports, surface) = rig();
        let open = open_input_set(&ports, &surface, true, &["in".to_string()], matches);
        assert_eq!(open.iter().filter(|p| **p == MidiPortId(LiveId(1))).count(), 1);
    }

    #[test]
    fn device_rows_leave_the_surface_out_and_keep_the_machines_order() {
        let (ports, surface) = rig();
        let rows: Vec<&str> = device_rows(&ports, &surface).iter().map(|d| d.name.as_str()).collect();
        assert_eq!(rows, vec!["house keys", "Midi Through"]);
    }

    #[test]
    fn a_ticked_device_is_written_down_by_its_whole_name() {
        let mut extras = Vec::new();
        assert!(toggle_extra_input(&mut extras, "house keys", true, 16, matches));
        assert_eq!(extras, vec!["house keys".to_string()]);
    }

    #[test]
    fn ticking_a_device_a_second_time_does_not_write_it_twice() {
        // A hand-typed word already stands for the port.
        let mut extras = vec!["keys".to_string()];
        assert!(toggle_extra_input(&mut extras, "house keys", true, 16, matches));
        assert_eq!(extras, vec!["keys".to_string()]);
    }

    #[test]
    fn unticking_removes_every_entry_that_matched_the_row() {
        let mut extras = vec!["keys".to_string(), "house keys".to_string(), "pads".to_string()];
        assert!(toggle_extra_input(&mut extras, "house keys", false, 16, matches));
        assert_eq!(extras, vec!["pads".to_string()]);
    }

    #[test]
    fn the_file_holds_sixteen_names_and_says_so() {
        let mut extras: Vec<String> = (0..16).map(|i| format!("device {i}")).collect();
        assert!(!toggle_extra_input(&mut extras, "one more", true, 16, matches), "refused");
        assert_eq!(extras.len(), 16);
        // Unticking always works, whatever the count.
        assert!(toggle_extra_input(&mut extras, "device 3", false, 16, matches));
        assert_eq!(extras.len(), 15);
    }

    #[test]
    fn a_device_row_says_why_it_is_not_heard() {
        assert_eq!(device_state_word(false, true, false), "open");
        assert_eq!(device_state_word(true, true, true), "open");
        assert_eq!(device_state_word(true, false, false), "surface, passed over");
        assert_eq!(device_state_word(false, false, true), "loopback");
        assert_eq!(device_state_word(false, false, false), "closed");
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    const MAX: usize = 16;

    #[test]
    fn a_fresh_install_writes_the_surface_on() {
        let store = merge_midi_settings(None, SURFACE_ON_DEFAULT, false, false, false, &[], MAX);
        let text = store.to_text();
        assert!(text.starts_with("version 1\n"), "{text}");
        assert!(text.contains("midi.surface 1\n"), "{text}");
        assert!(text.contains("midi.soft_takeover 0\n"), "{text}");
        assert!(text.contains("midi.monitor 0\n"), "{text}");
        assert!(!text.contains("midi.open"), "{text}");
    }

    #[test]
    fn an_older_settings_file_means_the_surface_is_on() {
        let old = "version 1\nmidi.monitor 0\nmidi.soft_takeover 1\nmidi.open.0 house keys\n";
        let store = Settings::from_text(old);
        assert!(store.bool("midi.surface", SURFACE_ON_DEFAULT));
    }

    #[test]
    fn rewriting_the_file_keeps_a_key_from_a_newer_build() {
        let existing = "version 1\nmidi.future 7\nmidi.monitor 0\n";
        let store = merge_midi_settings(Some(existing), true, true, false, false, &[], MAX);
        let text = store.to_text();
        assert!(text.contains("midi.future 7\n"), "{text}");
        assert!(text.contains("midi.soft_takeover 1\n"), "{text}");
    }

    #[test]
    fn a_shorter_device_list_leaves_no_stale_rows() {
        let existing = "version 1\nmidi.open.0 a\nmidi.open.1 b\nmidi.open.2 c\nmidi.open.3 d\n";
        let two = ["house keys".to_string(), "pads".to_string()];
        let store = merge_midi_settings(Some(existing), true, false, false, false, &two, MAX);
        let text = store.to_text();
        assert!(text.contains("midi.open.0 house keys\n"), "{text}");
        assert!(text.contains("midi.open.1 pads\n"), "{text}");
        assert!(!text.contains("midi.open.2"), "{text}");
        assert!(!text.contains("midi.open.3"), "{text}");
        assert!(!store.has("midi.open.3"), "gone, not blanked");
    }

    #[test]
    fn the_environment_holds_the_monitor_and_the_file_is_left_alone() {
        let existing = "version 1\nmidi.monitor 0\n";
        let store = merge_midi_settings(Some(existing), true, false, true, true, &[], MAX);
        assert!(store.to_text().contains("midi.monitor 0\n"), "{}", store.to_text());
        // A file that never said is told what the run has, as the first
        // run under the environment always wrote.
        let store = merge_midi_settings(Some("version 1\n"), true, false, true, true, &[], MAX);
        assert!(store.to_text().contains("midi.monitor 1\n"), "{}", store.to_text());
        // And without the environment the switch is the file's.
        let store = merge_midi_settings(Some(existing), true, false, true, false, &[], MAX);
        assert!(store.to_text().contains("midi.monitor 1\n"), "{}", store.to_text());
    }
}
