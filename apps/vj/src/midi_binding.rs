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
