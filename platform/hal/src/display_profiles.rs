//! Saved display profiles: the desktop's screens for one set of connected monitors.
//!
//! A profile is keyed by the monitors that are connected (their EDID fingerprints,
//! `LinuxDisplayOutput::fingerprint`, as a sorted multiset), so plugging in the same set
//! restores its arrangement, modes, main screen and pacing, on any boot and however the
//! cards are numbered. The direct backend reads the saved profiles (the file at
//! `layout_path`) at start-up and every time the set of connected monitors
//! changes, and applies the one saved for that set. A host writes them (the window manager's
//! `display-layout`), next to lines of its own that this parser skips.
//!
//! ```text
//! profile 00000000000000a1,00000000000000b2,00000000000000c3,00000000000000d4
//! screen
//! output 0000:01:00.0 DP-2 00000000000000b2
//! screen main
//! output 0000:01:00.0 DP-4 00000000000000a1 mode=2560x1440@144
//! screen
//! output 0000:01:00.0 DP-1 00000000000000c3 optimise
//! output 0000:01:00.0 DP-3 00000000000000d4
//! pace auto
//! ```
//!
//! `screen` starts the next screen of the profile (left to right; `main` marks the main
//! screen), `output` adds an output to it (`mode=` its mode, `optimise` the output the screen
//! is optimised for), `off` lists an output that is not driven, `pace` is `auto`, `slowest`,
//! `fastest` or `output <key>`. An output's key is its card's PCI address, its connector name
//! without the `cardN-` prefix and its monitor's fingerprint; a monitor that moved to another
//! connector is still found by its fingerprint. Lines before the first `profile`, unknown
//! keywords and malformed lines are skipped.
//!
//! Pure: no files, no Vulkan; `layout_path` only reads the environment.

use crate::direct::{mode_string_is_valid, ArrangedScreen, DisplayArrangement, DisplayPacing, LinuxDisplayOutput};
use std::path::PathBuf;

/// The saved profiles' file, which the backend reads and the window manager writes: its
/// `display-layout` in its config dir (`$XDG_CONFIG_HOME/makepad/wm`, or
/// `$HOME/.config/makepad/wm`). `MAKEPAD_DISPLAY_LAYOUT` names another file instead, for
/// tests. `None` when neither root is an absolute path.
pub fn layout_path() -> Option<PathBuf> {
    let var = |key: &str| std::env::var_os(key).map(PathBuf::from);
    layout_path_from(var("MAKEPAD_DISPLAY_LAYOUT"), var("XDG_CONFIG_HOME"), var("HOME"))
}

/// `layout_path` for these values of `MAKEPAD_DISPLAY_LAYOUT`, `XDG_CONFIG_HOME` and `HOME`.
fn layout_path_from(pinned: Option<PathBuf>, config: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(path) = pinned.filter(|p| !p.as_os_str().is_empty()) {
        return Some(path);
    }
    let root = config.filter(|p| p.is_absolute()).or_else(|| home.filter(|p| p.is_absolute()).map(|home| home.join(".config")))?;
    Some(root.join("makepad").join("wm").join("display-layout"))
}

/// One output across boots: the card's PCI address, the connector without its `cardN-`
/// prefix, and the monitor's fingerprint.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OutputKey {
    pub pci: String,
    pub connector: String,
    pub fingerprint: u64,
}

impl OutputKey {
    /// `name` is a connector name with or without its `cardN-` prefix.
    pub fn new(pci: &str, name: &str, fingerprint: u64) -> Self {
        Self { pci: pci.to_owned(), connector: connector_suffix(name).to_owned(), fingerprint }
    }
    /// `None` when the output's card has no PCI address (it could never be found again).
    pub fn of(output: &LinuxDisplayOutput) -> Option<Self> {
        Some(Self::new(output.pci.as_deref()?, &output.name, output.fingerprint))
    }
    fn text(&self) -> String {
        format!("{} {} {:016x}", self.pci, self.connector, self.fingerprint)
    }
    fn parse<'a>(tokens: &mut impl Iterator<Item = &'a str>) -> Option<Self> {
        let pci = tokens.next()?;
        let connector = tokens.next()?;
        let fingerprint = u64::from_str_radix(tokens.next()?, 16).ok()?;
        Some(Self { pci: pci.to_owned(), connector: connector.to_owned(), fingerprint })
    }
}

/// `name` without its `cardN-` prefix (`card1-DP-2` -> `DP-2`), unchanged otherwise.
pub fn connector_suffix(name: &str) -> &str {
    match name.strip_prefix("card").and_then(|rest| rest.split_once('-')) {
        Some((digits, connector)) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => connector,
        _ => name,
    }
}

/// The key of a set of monitors: their fingerprints, sorted (two identical monitors count twice).
pub fn monitor_set(fingerprints: impl IntoIterator<Item = u64>) -> Vec<u64> {
    let mut set: Vec<u64> = fingerprints.into_iter().collect();
    set.sort_unstable();
    set
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileOutput {
    pub key: OutputKey,
    pub mode: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ProfileScreen {
    pub outputs: Vec<ProfileOutput>,
    /// Index into `outputs` of the output the screen is optimised for.
    pub optimise_for: Option<usize>,
    pub main: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ProfilePace {
    #[default]
    Auto,
    Slowest,
    Fastest,
    Output(OutputKey),
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DisplayProfile {
    /// `monitor_set` of the monitors it is for.
    pub monitors: Vec<u64>,
    /// Left to right.
    pub screens: Vec<ProfileScreen>,
    pub off: Vec<ProfileOutput>,
    pub pace: ProfilePace,
}

/// What a profile asks of the connected outputs, by this boot's connector names.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ProfileSettings {
    pub arrangement: DisplayArrangement,
    /// Every output of the profile with its mode (`None`: the monitor's own).
    pub modes: Vec<(String, Option<String>)>,
    /// An output of the main screen.
    pub main: Option<String>,
    pub pacing: DisplayPacing,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DisplayProfiles {
    pub profiles: Vec<DisplayProfile>,
}

impl DisplayProfiles {
    pub fn parse(text: &str) -> Self {
        let mut profiles: Vec<DisplayProfile> = Vec::new();
        // A profile for a set already read is skipped whole: the first one wins.
        let mut skipping = true;
        for line in text.lines() {
            let mut tokens = line.split_whitespace();
            let Some(keyword) = tokens.next() else { continue };
            if keyword == "profile" {
                let monitors: Option<Vec<u64>> = tokens.next().map(|list| {
                    list.split(',').map(|fp| u64::from_str_radix(fp, 16).ok()).collect::<Option<Vec<u64>>>()
                }).flatten();
                skipping = match monitors.map(monitor_set).filter(|m| !m.is_empty()) {
                    Some(monitors) if !profiles.iter().any(|p| p.monitors == monitors) => {
                        profiles.push(DisplayProfile { monitors, ..Default::default() });
                        false
                    }
                    _ => true,
                };
                continue;
            }
            if skipping { continue; }
            let profile = profiles.last_mut().unwrap();
            let output = |tokens: &mut std::str::SplitWhitespace<'_>| -> Option<(ProfileOutput, bool)> {
                let key = OutputKey::parse(tokens)?;
                let mut mode = None;
                let mut optimise = false;
                for token in tokens {
                    if let Some(value) = token.strip_prefix("mode=") {
                        mode = mode_string_is_valid(value).then(|| value.to_owned());
                    } else if token == "optimise" {
                        optimise = true;
                    }
                }
                Some((ProfileOutput { key, mode }, optimise))
            };
            match keyword {
                "screen" => profile.screens.push(ProfileScreen { main: tokens.any(|t| t == "main"), ..Default::default() }),
                "output" => {
                    let (Some(screen), Some((output, optimise))) = (profile.screens.last_mut(), output(&mut tokens)) else { continue };
                    if optimise && screen.optimise_for.is_none() { screen.optimise_for = Some(screen.outputs.len()); }
                    screen.outputs.push(output);
                }
                "off" => {
                    if let Some((output, _)) = output(&mut tokens) { profile.off.push(output); }
                }
                "pace" => {
                    profile.pace = match tokens.next() {
                        Some("slowest") => ProfilePace::Slowest,
                        Some("fastest") => ProfilePace::Fastest,
                        Some("output") => match OutputKey::parse(&mut tokens) { Some(key) => ProfilePace::Output(key), None => continue },
                        _ => ProfilePace::Auto,
                    };
                }
                _ => {}
            }
        }
        for profile in &mut profiles {
            profile.screens.retain(|s| !s.outputs.is_empty());
            // At most one main screen: the first.
            let mut seen = false;
            for screen in profile.screens.iter_mut().filter(|s| s.main) {
                screen.main = !seen;
                seen = true;
            }
        }
        Self { profiles }
    }

    pub fn text(&self) -> String {
        let mut out = String::new();
        let line = |o: &ProfileOutput, optimise: bool| {
            let mut s = o.key.text();
            if let Some(mode) = &o.mode { s.push_str(" mode="); s.push_str(mode); }
            if optimise { s.push_str(" optimise"); }
            s
        };
        for p in &self.profiles {
            out.push_str("profile ");
            out.push_str(&p.monitors.iter().map(|fp| format!("{fp:016x}")).collect::<Vec<_>>().join(","));
            out.push('\n');
            for s in &p.screens {
                out.push_str(if s.main { "screen main\n" } else { "screen\n" });
                for (i, o) in s.outputs.iter().enumerate() {
                    out.push_str("output ");
                    out.push_str(&line(o, s.optimise_for == Some(i)));
                    out.push('\n');
                }
            }
            for o in &p.off {
                out.push_str("off ");
                out.push_str(&line(o, false));
                out.push('\n');
            }
            match &p.pace {
                ProfilePace::Auto => out.push_str("pace auto\n"),
                ProfilePace::Slowest => out.push_str("pace slowest\n"),
                ProfilePace::Fastest => out.push_str("pace fastest\n"),
                ProfilePace::Output(key) => { out.push_str("pace output "); out.push_str(&key.text()); out.push('\n'); }
            }
        }
        out
    }

    /// The profile saved for exactly these monitors (`monitor_set`).
    pub fn find(&self, monitors: &[u64]) -> Option<&DisplayProfile> {
        self.profiles.iter().find(|p| p.monitors == monitors)
    }

    /// Saves `profile`, replacing the one for the same monitors.
    pub fn set(&mut self, profile: DisplayProfile) {
        match self.profiles.iter_mut().find(|p| p.monitors == profile.monitors) {
            Some(slot) => *slot = profile,
            None => self.profiles.push(profile),
        }
    }
}

impl DisplayProfile {
    /// The profile over `connected` (this boot's name and key of every connected output): each
    /// saved output is found by its whole key, else (a monitor moved to another connector) by its
    /// monitor's fingerprint; a saved output found in neither way is left out.
    pub fn resolve(&self, connected: &[(String, OutputKey)]) -> ProfileSettings {
        let saved: Vec<&ProfileOutput> = self.screens.iter().flat_map(|s| s.outputs.iter()).chain(self.off.iter()).collect();
        let mut names: Vec<Option<String>> = vec![None; saved.len()];
        let mut used = vec![false; connected.len()];
        for exact in [true, false] {
            for (slot, o) in saved.iter().enumerate() {
                if names[slot].is_some() { continue; }
                let found = connected.iter().enumerate().position(|(i, (_, key))| {
                    !used[i] && if exact { *key == o.key } else { key.fingerprint == o.key.fingerprint }
                });
                if let Some(i) = found {
                    used[i] = true;
                    names[slot] = Some(connected[i].0.clone());
                }
            }
        }
        let name_of = |o: &ProfileOutput| saved.iter().position(|s| std::ptr::eq(*s, o)).and_then(|i| names[i].clone());
        let mut settings = ProfileSettings::default();
        for screen in &self.screens {
            let outputs: Vec<String> = screen.outputs.iter().filter_map(name_of).collect();
            if outputs.is_empty() { continue; }
            if screen.main { settings.main = outputs.first().cloned(); }
            let optimise_for = screen.optimise_for.and_then(|i| screen.outputs.get(i)).and_then(name_of);
            settings.arrangement.screens.push(ArrangedScreen { outputs, optimise_for });
        }
        settings.arrangement.off = self.off.iter().filter_map(name_of).collect();
        settings.modes = saved.iter().filter_map(|o| Some((name_of(o)?, o.mode.clone()))).collect();
        let named = |key: &OutputKey| saved.iter().position(|o| o.key == *key).and_then(|i| names[i].clone());
        settings.pacing = match &self.pace {
            ProfilePace::Auto => DisplayPacing::Auto,
            ProfilePace::Slowest => DisplayPacing::Slowest,
            ProfilePace::Fastest => DisplayPacing::Fastest,
            ProfilePace::Output(key) => named(key).map_or(DisplayPacing::Auto, DisplayPacing::Screen),
        };
        settings
    }

    /// The profile for the monitors `connected` now (name and key of each) running `settings`:
    /// every connected output is in it, on its screen (`DisplayArrangement::group`) or off.
    pub fn capture(settings: &ProfileSettings, connected: &[(String, OutputKey)]) -> DisplayProfile {
        let names: Vec<&str> = connected.iter().map(|(n, _)| n.as_str()).collect();
        let entry = |i: usize| ProfileOutput {
            key: connected[i].1.clone(),
            mode: settings.modes.iter().find(|(n, _)| *n == connected[i].0).and_then(|(_, m)| m.clone()),
        };
        let screens = settings.arrangement.group(&names).into_iter().map(|g| ProfileScreen {
            optimise_for: g.optimise_for.and_then(|o| g.outputs.iter().position(|i| *i == o)),
            main: settings.main.as_deref().is_some_and(|m| g.outputs.iter().any(|i| names[*i] == m)),
            outputs: g.outputs.into_iter().map(entry).collect(),
        }).collect();
        let off = (0..connected.len()).filter(|i| settings.arrangement.is_off(names[*i])).map(entry).collect();
        let pace = match &settings.pacing {
            DisplayPacing::Auto => ProfilePace::Auto,
            DisplayPacing::Slowest => ProfilePace::Slowest,
            DisplayPacing::Fastest => ProfilePace::Fastest,
            DisplayPacing::Screen(name) => connected.iter().find(|(n, _)| n == name).map_or(ProfilePace::Auto, |(_, k)| ProfilePace::Output(k.clone())),
        };
        DisplayProfile { monitors: monitor_set(connected.iter().map(|(_, k)| k.fingerprint)), screens, off, pace }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_lives_in_the_window_managers_config_dir_unless_a_test_names_another() {
        let p = |s: &str| Some(PathBuf::from(s));
        assert_eq!(layout_path_from(None, p("/cfg"), p("/home/a")), p("/cfg/makepad/wm/display-layout"));
        assert_eq!(layout_path_from(None, None, p("/home/a")), p("/home/a/.config/makepad/wm/display-layout"));
        // A relative root is not a config dir.
        assert_eq!(layout_path_from(None, p("cfg"), p("/home/a")), p("/home/a/.config/makepad/wm/display-layout"));
        assert_eq!(layout_path_from(None, p("cfg"), p("home")), None);
        assert_eq!(layout_path_from(p("/tmp/test-layout"), p("/cfg"), p("/home/a")), p("/tmp/test-layout"));
        assert_eq!(layout_path_from(p(""), p("/cfg"), None), p("/cfg/makepad/wm/display-layout"));
    }

    fn connected(outputs: &[(&str, u64)]) -> Vec<(String, OutputKey)> {
        outputs.iter().map(|(name, fp)| (name.to_string(), OutputKey::new("0000:01:00.0", name, *fp))).collect()
    }

    #[test]
    fn the_users_profile_round_trips_and_restores_by_monitor() {
        // [B] [A main] [C+D], optimised for D; A at 2560x1440@144
        let now = connected(&[("card1-DP-1", 0xa1), ("card1-DP-2", 0xb2), ("card1-DP-3", 0xc3), ("card1-DP-4", 0xd4)]);
        let settings = ProfileSettings {
            arrangement: DisplayArrangement::parse("card1-DP-2,card1-DP-1,card1-DP-3+*card1-DP-4").unwrap(),
            modes: vec![("card1-DP-1".into(), Some("2560x1440@144".into()))],
            main: Some("card1-DP-1".into()),
            pacing: DisplayPacing::Screen("card1-DP-4".into()),
        };
        let profile = DisplayProfile::capture(&settings, &now);
        assert_eq!(profile.monitors, vec![0xa1, 0xb2, 0xc3, 0xd4]);
        let mut saved = DisplayProfiles::default();
        saved.set(profile.clone());
        let text = format!("makepad-display-layout 2\nrender-on 0000:01:00.0 1234:5678\nscreen 0000:01:00.0 DP-4 main\n{}", saved.text());
        let read = DisplayProfiles::parse(&text);
        assert_eq!(read, saved, "the host's own lines and an old file's screen lines are skipped");
        assert_eq!(read.find(&monitor_set([0xd4, 0xc3, 0xb2, 0xa1])).unwrap().resolve(&now), ProfileSettings {
            modes: vec![("card1-DP-2".into(), None), ("card1-DP-1".into(), Some("2560x1440@144".into())),
                ("card1-DP-3".into(), None), ("card1-DP-4".into(), None)],
            ..settings.clone()
        });
        // another boot: the card renumbered and C, D swapped connectors; the monitors are the same
        let later = vec![
            ("card0-DP-1".to_string(), OutputKey::new("0000:01:00.0", "DP-1", 0xa1)),
            ("card0-DP-2".to_string(), OutputKey::new("0000:01:00.0", "DP-2", 0xb2)),
            ("card0-DP-3".to_string(), OutputKey::new("0000:01:00.0", "DP-3", 0xd4)),
            ("card0-DP-4".to_string(), OutputKey::new("0000:01:00.0", "DP-4", 0xc3)),
        ];
        let restored = read.find(&monitor_set([0xa1, 0xb2, 0xc3, 0xd4])).unwrap().resolve(&later);
        assert_eq!(restored.arrangement.text(), "card0-DP-2,card0-DP-1,card0-DP-4+*card0-DP-3", "D is still the one optimised for");
        assert_eq!(restored.main.as_deref(), Some("card0-DP-1"));
        assert_eq!(restored.pacing, DisplayPacing::Screen("card0-DP-3".into()));
        assert!(read.find(&monitor_set([0xa1])).is_none(), "only A connected: another set");
    }

    #[test]
    fn a_profile_lists_every_output_with_off_ones_and_one_main() {
        let now = connected(&[("card1-DP-1", 1), ("card1-DP-2", 2), ("card1-DP-3", 2)]);
        let settings = ProfileSettings { arrangement: DisplayArrangement::parse("off:card1-DP-1").unwrap(), main: Some("card1-DP-3".into()), ..Default::default() };
        let profile = DisplayProfile::capture(&settings, &now);
        assert_eq!(profile.monitors, vec![1, 2, 2], "two identical monitors count twice");
        assert_eq!(profile.screens.len(), 2, "unarranged outputs are saved as screens of their own");
        assert_eq!(profile.off.len(), 1);
        assert_eq!(profile.screens.iter().filter(|s| s.main).count(), 1);
        let text = "profile 0000000000000001\nscreen main\noutput 0000:01:00.0 DP-1 0000000000000001\nscreen main\noutput 0000:01:00.0 DP-2 0000000000000001\n\
            screen\noutput bad\nprofile 0000000000000001\nscreen\nprofile zz\nscreen\n";
        let read = DisplayProfiles::parse(text);
        assert_eq!(read.profiles.len(), 1, "a second profile for the same set and a malformed one are skipped");
        assert_eq!(read.profiles[0].screens.iter().map(|s| s.main).collect::<Vec<_>>(), vec![true, false], "one main screen, empty screens dropped");
    }
}
