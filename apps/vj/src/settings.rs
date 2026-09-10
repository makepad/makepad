//! One way to keep an operator's preferences.
//!
//! There was no store before this: thirteen files under one directory,
//! written by thirteen hand-rolled writes, in five text dialects that had
//! no reader, no writer and no version between them. Three of them were
//! positional -- six bare lines with no keys at all -- so inserting a
//! setting in the middle silently re-read every one after it as something
//! else.
//!
//! This is a flat `group.key value` file with a version on the first line.
//! A value is text until somebody asks for it as something, and asking for
//! it wrongly gives the default rather than a panic: a torn write, a
//! hand-edit or a setting from a newer build must never take the app down
//! on the way up. Keys this build does not know are kept and written back,
//! so running an older build for one set does not throw away what a newer
//! one had stored.

use std::collections::BTreeMap;

/// The format this build writes. A file with no version line at all is
/// version 0: that is every file written before this module existed.
pub const VERSION: u32 = 1;

#[derive(Clone, Default, PartialEq, Debug)]
pub struct Settings {
    version: u32,
    /// `group.key` to the raw text after it. Ordered, so the file is stable
    /// between writes and a diff shows only what actually changed.
    values: BTreeMap<String, String>,
}

impl Settings {
    /// An empty store at the current version.
    pub fn new() -> Settings {
        Settings { version: VERSION, values: BTreeMap::new() }
    }

    /// What version the file said. Zero means it predates this module.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Say this store is now at the current version, after whatever
    /// migration the caller had to do.
    pub fn set_current_version(&mut self) {
        self.version = VERSION;
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Whether a key is present at all, whatever it says.
    pub fn has(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    /// The raw text of a key.
    pub fn raw(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|value| value.as_str())
    }

    pub fn text(&self, key: &str, default: &str) -> String {
        self.values.get(key).cloned().unwrap_or_else(|| default.to_string())
    }

    pub fn set_text(&mut self, key: &str, value: &str) {
        // A newline would become a second setting on the next read.
        let flat: String = value.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        self.values.insert(key.to_string(), flat);
    }

    /// Forget a key. Returns what it said, if it was there. A store that
    /// is read, edited and written back has to be able to shorten a list
    /// as well as lengthen it, and setting a key to nothing is not the
    /// same: a bare key is still written, and still read.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.values.remove(key)
    }

    pub fn bool(&self, key: &str, default: bool) -> bool {
        match self.values.get(key).map(|value| value.trim()) {
            Some("1") | Some("true") | Some("on") | Some("yes") => true,
            Some("0") | Some("false") | Some("off") | Some("no") => false,
            _ => default,
        }
    }

    pub fn set_bool(&mut self, key: &str, value: bool) {
        self.set_text(key, if value { "1" } else { "0" });
    }

    /// A number, refusing anything that is not one. NaN and infinity parse
    /// perfectly well and then poison whatever they are handed to, so they
    /// are not numbers as far as this is concerned.
    pub fn f64(&self, key: &str, default: f64) -> f64 {
        self.values
            .get(key)
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite())
            .unwrap_or(default)
    }

    pub fn set_f64(&mut self, key: &str, value: f64) {
        if !value.is_finite() {
            return;
        }
        self.set_text(key, &value.to_string());
    }

    pub fn usize(&self, key: &str, default: usize) -> usize {
        self.values
            .get(key)
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(default)
    }

    pub fn set_usize(&mut self, key: &str, value: usize) {
        self.set_text(key, &value.to_string());
    }

    /// Read a store. Anything that is not `key value` is skipped, so a torn
    /// write costs the settings on the torn line and nothing else.
    pub fn from_text(text: &str) -> Settings {
        let mut store = Settings { version: 0, values: BTreeMap::new() };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = match line.split_once(char::is_whitespace) {
                Some((key, value)) => (key, value.trim()),
                // A key on its own is a key set to nothing, which is how an
                // empty device name comes back.
                None => (line, ""),
            };
            if key == "version" {
                store.version = value.parse().unwrap_or(0);
                continue;
            }
            store.values.insert(key.to_string(), value.to_string());
        }
        store
    }

    pub fn to_text(&self) -> String {
        let mut text = format!("version {}\n", self.version);
        for (key, value) in &self.values {
            text.push_str(key);
            if !value.is_empty() {
                text.push(' ');
                text.push_str(value);
            }
            text.push('\n');
        }
        text
    }
}

/// The dialects this store replaces, kept for as long as a file written
/// before it exists on somebody's disk.
///
/// Each is a pure function with tests against the bytes actually sitting in
/// `local/vj` today, because the loaders they came from had none at all —
/// and a positional format is exactly the kind that fails quietly, by
/// reading every setting after the changed one as something else.
pub mod legacy {
    use super::Settings;

    /// `autopilot.txt` at version 0: six bare numbers, in this order, with
    /// no keys. Brain 3 means RANDOM rather than a fourth brain.
    pub fn autopilot(text: &str) -> Settings {
        let mut store = Settings::new();
        let mut lines = text.lines();
        let mut next = |key: &str, fallback: usize| {
            let value = lines.next().and_then(|line| line.trim().parse().ok()).unwrap_or(fallback);
            store.set_usize(key, value);
        };
        next("auto.brain", 2);
        next("auto.style", 0);
        next("auto.vocal_guard", 1);
        next("auto.phrase_snap", 1);
        next("queue.repeat", 0);
        next("queue.shuffle", 0);
        store
    }

    /// `headphones.txt` at version 0: the device NAME on the first line —
    /// which is why this one could never have been key-and-value by
    /// accident, since a device name holds spaces and may be empty — then
    /// volume, placement and cue mode.
    pub fn phones(text: &str) -> Settings {
        let mut store = Settings::new();
        let mut lines = text.lines();
        store.set_text("phones.device", lines.next().unwrap_or("").trim());
        if let Some(volume) = lines.next().and_then(|line| line.trim().parse::<f64>().ok()) {
            store.set_f64("phones.volume", volume);
        }
        for key in ["phones.placement", "phones.cue_mode"] {
            if let Some(index) = lines.next().and_then(|line| line.trim().parse::<usize>().ok()) {
                store.set_usize(key, index);
            }
        }
        store
    }

    /// `gen-panel.txt` at version 0: ten bare lines, one of which is the
    /// PROMPT — free text that may be empty, sitting in the middle, which
    /// is exactly the arrangement a positional format handles worst.
    ///
    /// The last line stays optional on purpose: only a remembered pick
    /// counts as the operator having chosen an image model, so an absent
    /// line must not pin one.
    pub fn gen_panel(text: &str) -> Settings {
        fn number(store: &mut Settings, lines: &mut std::str::Lines, key: &str) {
            if let Some(value) = lines.next().and_then(|line| line.trim().parse::<usize>().ok()) {
                store.set_usize(key, value);
            }
        }
        let mut store = Settings::new();
        let mut lines = text.lines();
        number(&mut store, &mut lines, "gen.profile");
        number(&mut store, &mut lines, "gen.video_length");
        number(&mut store, &mut lines, "gen.continuous");
        // Not trimmed and not parsed: it is whatever the operator typed.
        if let Some(prompt) = lines.next() {
            store.set_text("gen.prompt", prompt);
        }
        number(&mut store, &mut lines, "gen.panel_open");
        number(&mut store, &mut lines, "ui.lower_tab");
        number(&mut store, &mut lines, "ui.monitor_audio");
        number(&mut store, &mut lines, "import.convert_video");
        number(&mut store, &mut lines, "gen.video_size");
        number(&mut store, &mut lines, "gen.image_model");
        store
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_forgets_a_key_and_returns_what_it_said() {
        let mut store = Settings::new();
        store.set_text("a.b", "one");
        store.set_text("a.c", "two");
        assert_eq!(store.remove("a.b").as_deref(), Some("one"));
        assert!(!store.has("a.b"), "gone, not blanked");
        assert_eq!(store.remove("a.b"), None, "and gone is gone");
        assert!(!store.to_text().contains("a.b"), "a removed key writes nothing");
        assert_eq!(store.text("a.c", ""), "two", "its neighbour is untouched");
    }

    #[test]
    fn the_autopilot_file_on_disk_today_reads_into_the_right_keys() {
        // Verbatim from local/vj/autopilot.txt: RANDOM brain, body style,
        // both guards on, queue plain.
        let store = legacy::autopilot("3\n1\n1\n1\n0\n0\n");
        assert_eq!(store.usize("auto.brain", 0), 3, "3 is RANDOM, not a fourth brain");
        assert_eq!(store.usize("auto.style", 0), 1);
        assert_eq!(store.usize("auto.vocal_guard", 0), 1);
        assert_eq!(store.usize("auto.phrase_snap", 0), 1);
        assert_eq!(store.usize("queue.repeat", 9), 0);
        assert_eq!(store.usize("queue.shuffle", 9), 0);
    }

    #[test]
    fn the_headphones_file_on_disk_today_reads_into_the_right_keys() {
        // Verbatim from local/vj/headphones.txt. The device name holds
        // spaces AND brackets, which is why that line was never a key.
        let store = legacy::phones(
            "Mindframe Headset (2- OMEN Mindframe Prime)\n0.85\n0\n0\n",
        );
        assert_eq!(
            store.text("phones.device", ""),
            "Mindframe Headset (2- OMEN Mindframe Prime)"
        );
        assert_eq!(store.f64("phones.volume", 0.0), 0.85);
        assert_eq!(store.usize("phones.placement", 9), 0);
        assert_eq!(store.usize("phones.cue_mode", 9), 0);
    }

    #[test]
    fn a_headphones_file_with_no_device_chosen_still_says_so() {
        let store = legacy::phones("\n0.5\n1\n2\n");
        assert!(store.has("phones.device"));
        assert_eq!(store.text("phones.device", "unused"), "");
        assert_eq!(store.usize("phones.placement", 9), 1);
        assert_eq!(store.usize("phones.cue_mode", 9), 2);
    }

    #[test]
    fn the_gen_panel_file_on_disk_today_reads_into_the_right_keys() {
        // Verbatim from local/vj/gen-panel.txt, empty prompt line and all.
        let store = legacy::gen_panel("2\n3\n0\n\n0\n0\n0\n0\n1\n");
        assert_eq!(store.usize("gen.profile", 9), 2);
        assert_eq!(store.usize("gen.video_length", 9), 3);
        assert!(!store.bool("gen.continuous", true));
        assert_eq!(store.text("gen.prompt", "unused"), "", "an empty prompt is a prompt");
        assert!(!store.bool("gen.panel_open", true));
        assert_eq!(store.usize("ui.lower_tab", 9), 0);
        assert_eq!(store.usize("gen.video_size", 9), 1);
        assert!(
            !store.has("gen.image_model"),
            "nine lines, so nobody picked a model, and none may be pinned"
        );
    }

    #[test]
    fn a_gen_panel_prompt_keeps_the_spaces_the_operator_typed() {
        let store = legacy::gen_panel("0\n0\n0\na slow  drifting  city at night\n1\n");
        assert_eq!(store.text("gen.prompt", ""), "a slow  drifting  city at night");
        assert!(store.bool("gen.panel_open", false), "and the line after it still lines up");
    }

    #[test]
    fn a_short_autopilot_file_leaves_the_rest_at_the_defaults_the_old_loader_used() {
        let store = legacy::autopilot("0\n");
        assert_eq!(store.usize("auto.brain", 9), 0);
        assert_eq!(store.usize("auto.style", 9), 0);
        assert_eq!(store.usize("auto.vocal_guard", 9), 1, "the guards defaulted ON");
        assert_eq!(store.usize("auto.phrase_snap", 9), 1);
        assert_eq!(store.usize("queue.repeat", 9), 0);
    }

    /// The level the room was left at rides in the phones file, because
    /// it is the same rig. A file written before it existed reads as the
    /// face's own default rather than as silence.
    #[test]
    fn the_master_level_comes_back_and_an_older_file_reads_as_the_default() {
        let mut store = Settings::new();
        store.set_f64("mix.master", 0.42);
        let back = Settings::from_text(&store.to_text());
        assert_eq!(back.f64("mix.master", 0.9), 0.42);
        // An older file, which has every other key and not this one.
        let older = Settings::from_text("phones.volume 0.5
phones.placement 1
");
        assert_eq!(older.f64("mix.master", 0.9), 0.9, "the default stands");
    }

    /// The crossfader's own law and which decks are in the cans: both were
    /// set by hand every launch because neither was written down. An older
    /// file reads as what the tab did before, not as a surprise.
    #[test]
    fn the_fade_curve_and_the_cue_latches_come_back() {
        let mut store = Settings::new();
        store.set_usize("deck.curve", 5);
        store.set_bool("phones.cue_a", true);
        store.set_bool("phones.cue_b", false);
        let back = Settings::from_text(&store.to_text());
        assert_eq!(back.usize("deck.curve", 0), 5);
        assert!(back.bool("phones.cue_a", false));
        assert!(!back.bool("phones.cue_b", false));
        // A file from before either key: equal power, and nothing cued.
        let older = Settings::from_text("auto.set_curve 2
phones.volume 0.5
");
        assert_eq!(older.usize("deck.curve", 0), 0, "equal power");
        assert!(!older.bool("phones.cue_a", false));
        assert!(!older.bool("phones.cue_b", false));
    }

    #[test]
    fn a_setting_comes_back_as_what_it_was_put_in_as() {
        let mut store = Settings::new();
        store.set_bool("phones.split", true);
        store.set_f64("phones.volume", 0.42);
        store.set_usize("phones.placement", 2);
        store.set_text("phones.device", "Focusrite Scarlett 2i2");
        let back = Settings::from_text(&store.to_text());
        assert!(back.bool("phones.split", false));
        assert_eq!(back.f64("phones.volume", 0.0), 0.42);
        assert_eq!(back.usize("phones.placement", 0), 2);
        assert_eq!(back.text("phones.device", ""), "Focusrite Scarlett 2i2");
        assert_eq!(back.version(), VERSION);
    }

    #[test]
    fn a_key_nobody_set_is_the_default() {
        let store = Settings::new();
        assert!(store.bool("nothing.here", true));
        assert!(!store.bool("nothing.here", false));
        assert_eq!(store.f64("nothing.here", 1.5), 1.5);
        assert_eq!(store.usize("nothing.here", 7), 7);
        assert_eq!(store.text("nothing.here", "fallback"), "fallback");
    }

    #[test]
    fn a_value_that_is_not_what_was_asked_for_is_the_default_and_never_a_panic() {
        let store = Settings::from_text("version 1\na.number wobble\na.count -3\na.flag maybe\n");
        assert_eq!(store.f64("a.number", 2.5), 2.5);
        assert_eq!(store.usize("a.count", 9), 9, "a negative is not a count");
        assert!(store.bool("a.flag", true), "and an opinion is not a flag");
    }

    #[test]
    fn a_number_that_is_not_finite_is_not_a_number() {
        // These parse perfectly well and then poison whatever holds them.
        let store = Settings::from_text("version 1\na.gain NaN\na.rate inf\na.trim -inf\n");
        assert_eq!(store.f64("a.gain", 1.0), 1.0);
        assert_eq!(store.f64("a.rate", 48000.0), 48000.0);
        assert_eq!(store.f64("a.trim", 0.5), 0.5);
        let mut store = Settings::new();
        store.set_f64("a.gain", f64::NAN);
        assert!(!store.has("a.gain"), "and one is never written down");
    }

    #[test]
    fn a_setting_from_a_newer_build_survives_this_one_writing_the_file() {
        let newer = "version 9\nfuture.thing 42\nphones.volume 0.3\n";
        let mut store = Settings::from_text(newer);
        store.set_f64("phones.volume", 0.7);
        let text = store.to_text();
        assert!(text.contains("future.thing 42"), "kept: {text}");
        let back = Settings::from_text(&text);
        assert_eq!(back.f64("phones.volume", 0.0), 0.7);
        assert_eq!(back.usize("future.thing", 0), 42);
    }

    #[test]
    fn a_file_with_no_version_line_is_the_world_before_this_module() {
        let store = Settings::from_text("phones.volume 0.3\n");
        assert_eq!(store.version(), 0, "which is what a migration hangs off");
        assert_eq!(store.f64("phones.volume", 0.0), 0.3);
    }

    #[test]
    fn a_value_may_hold_spaces_because_device_names_do() {
        let mut store = Settings::new();
        store.set_text("phones.device", "Speakers (Realtek High Definition Audio)");
        let back = Settings::from_text(&store.to_text());
        assert_eq!(
            back.text("phones.device", ""),
            "Speakers (Realtek High Definition Audio)"
        );
    }

    #[test]
    fn an_empty_value_comes_back_empty_rather_than_missing() {
        // No device chosen is a real state, and it is not the same as never
        // having been asked.
        let mut store = Settings::new();
        store.set_text("phones.device", "");
        let back = Settings::from_text(&store.to_text());
        assert!(back.has("phones.device"));
        assert_eq!(back.text("phones.device", "some default"), "");
    }

    #[test]
    fn a_newline_in_a_value_cannot_become_a_second_setting() {
        let mut store = Settings::new();
        store.set_text("a.label", "first\nb.injected 1");
        let back = Settings::from_text(&store.to_text());
        assert!(!back.has("b.injected"), "a value stays one value");
        assert_eq!(back.text("a.label", ""), "firstb.injected 1");
    }

    #[test]
    fn a_torn_line_costs_only_itself() {
        let store = Settings::from_text("version 1\na.one 1\n\u{0}\u{0}\u{0}\na.two 2\n");
        assert_eq!(store.usize("a.one", 0), 1);
        assert_eq!(store.usize("a.two", 0), 2);
    }

    #[test]
    fn the_file_reads_the_same_twice_so_a_diff_shows_only_real_changes() {
        let mut store = Settings::new();
        for key in ["z.last", "a.first", "m.middle"] {
            store.set_usize(key, 1);
        }
        let once = store.to_text();
        let twice = Settings::from_text(&once).to_text();
        assert_eq!(once, twice);
        let order: Vec<&str> = once.lines().skip(1).collect();
        assert_eq!(order, vec!["a.first 1", "m.middle 1", "z.last 1"]);
    }
}
