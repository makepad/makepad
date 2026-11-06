//! The Display menu's model: the configuration it edits (which screens,
//! which monitors each shows, their modes and the main screen), each
//! monitor's resolutions and the refresh rates of a resolution, titles, and
//! the try → keep / auto-revert state machine every applied configuration
//! goes through.
//!
//! The menu edits a draft; Apply sends the whole of it at once. A
//! configuration that reinitialises no output (moving monitors between
//! screens, the optimise-for choice, the main screen, the order) is saved as
//! soon as the renderer shows it. One that does (a mode, a monitor turned on
//! or off) is shown as "Keep these settings? Reverting in 15 s": Keep saves
//! it; Revert, or the 15 s running out, sends the previous one again. A
//! configuration the renderer refuses, or that comes up failed, is reported
//! under the Apply row and the previous one is sent again. Only applied and
//! kept configurations are saved (`TrialOut::Save`); the caller owns the
//! saving and the sending.
//!
//! Pure: no `Cx`, no files. Linux only, like `display_layout`.
#![cfg(target_os = "linux")]

use makepad_widgets::makepad_platform::linux_display::{
    ArrangedScreen, DisplayArrangement, DisplayPacing, LinuxDisplayCommand, LinuxDisplayOutput, LinuxDisplayScreen, LinuxDisplaySnapshot, VirtualDisplay, VIRTUAL_OUTPUT,
};

use super::display_layout::{connector_suffix, SavedVirtual, VirtualSize, PEER_PENDING_STATUS};

/// How long a change waits for Keep before it is reverted.
pub const CONFIRM_SECONDS: f64 = 15.0;
/// How long a change may take to come up before it counts as failed.
pub const APPLY_TIMEOUT: f64 = 10.0;

// ---------------------------------------------------------------- modes

/// One resolution a screen offers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
    /// The screen's own (EDID preferred) resolution.
    pub native: bool,
}

/// The screen's own resolution: its EDID preferred mode, else the largest
/// it offers.
pub fn native_resolution(output: &LinuxDisplayOutput) -> Option<(u32, u32)> {
    output.native.map(|(w, h, _)| (w, h)).or_else(|| output.modes.first().map(|&(w, h, _)| (w, h)))
}

/// The resolutions of `modes` (the renderer's list, largest and fastest
/// first) in that order, each once, `native` marked (the first when it is
/// `None`).
pub fn resolutions(modes: &[(u32, u32, f64)], native: Option<(u32, u32)>) -> Vec<Resolution> {
    let native = native.or_else(|| modes.first().map(|&(w, h, _)| (w, h)));
    let mut out: Vec<Resolution> = Vec::new();
    for &(width, height, _) in modes {
        if !out.iter().any(|r| r.width == width && r.height == height) {
            out.push(Resolution { width, height, native: native == Some((width, height)) });
        }
    }
    out
}

/// The refresh rates `modes` offers at `width`×`height`, fastest first,
/// one per whole hertz (59.94 and 60 are one rate, the renderer's mode
/// syntax matches whole hertz): the first, fastest, of each is kept.
pub fn rates_for(modes: &[(u32, u32, f64)], width: u32, height: u32) -> Vec<f64> {
    let mut rates: Vec<f64> = modes
        .iter()
        .filter(|&&(w, h, hz)| w == width && h == height && hz.is_finite() && hz > 0.0)
        .map(|&(_, _, hz)| hz)
        .collect();
    rates.sort_by(|a, b| b.total_cmp(a));
    let mut out: Vec<f64> = Vec::with_capacity(rates.len());
    for hz in rates {
        if !out.iter().any(|kept| kept.round() == hz.round()) {
            out.push(hz);
        }
    }
    out
}

/// The renderer's mode syntax: `3840x2160@60`, whole hertz.
pub fn mode_string(width: u32, height: u32, hz: f64) -> String {
    if hz.is_finite() && hz > 0.0 {
        format!("{width}x{height}@{}", hz.round() as u32)
    } else {
        format!("{width}x{height}")
    }
}

/// `3840×2160`.
pub fn resolution_label(width: u32, height: u32) -> String {
    format!("{width}\u{d7}{height}")
}

/// `240 Hz`, whole hertz.
pub fn rate_label(hz: f64) -> String {
    if hz.is_finite() && hz > 0.0 { format!("{} Hz", hz.round() as u32) } else { "\u{2014}".into() }
}

/// `3840×2160 · 240 Hz`.
pub fn mode_label(width: u32, height: u32, hz: f64) -> String {
    format!("{} \u{b7} {}", resolution_label(width, height), rate_label(hz))
}

/// `WxH`, `WxH@Hz` or `WxH-Hz` as numbers; `None` when malformed.
pub fn parse_mode(mode: &str) -> Option<(u32, u32, Option<u32>)> {
    let mode = mode.trim();
    let (resolution, hz) = match mode.split_once('@').or_else(|| mode.split_once('-')) {
        Some((resolution, hz)) => (resolution, Some(hz.parse().ok()?)),
        None => (mode, None),
    };
    let (w, h) = resolution.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?, hz))
}

/// Automatic: the native resolution at its fastest rate.
fn automatic(modes: &[(u32, u32, f64)], native: Option<(u32, u32)>) -> Option<(u32, u32, f64)> {
    match native {
        Some((w, h)) => rates_for(modes, w, h).first().map(|&hz| (w, h, hz)).or_else(|| modes.first().copied()),
        None => modes.first().copied(),
    }
}

/// The mode a choice of `width`×`height` at `hz` sends: `None`
/// (automatic) for the native resolution at its fastest rate, which is what
/// automatic shows, so a screen left at its best is saved without a pin.
pub fn mode_for(modes: &[(u32, u32, f64)], native: Option<(u32, u32)>, width: u32, height: u32, hz: f64) -> Option<String> {
    match automatic(modes, native) {
        Some((w, h, fastest)) if w == width && h == height && fastest.round() == hz.round() => None,
        _ => Some(mode_string(width, height, hz)),
    }
}

/// What `mode` (`None` is automatic) resolves to on a screen offering
/// `modes`: automatic is the native resolution at its fastest; a bare
/// `WxH` is that resolution's fastest; `WxH@Hz` the offered rate of that
/// whole hertz.
pub fn resolve_mode(modes: &[(u32, u32, f64)], native: Option<(u32, u32)>, mode: Option<&str>) -> Option<(u32, u32, f64)> {
    let Some(mode) = mode else { return automatic(modes, native) };
    let (w, h, hz) = parse_mode(mode)?;
    let rates = rates_for(modes, w, h);
    match hz {
        None => rates.first().map(|&hz| (w, h, hz)),
        Some(hz) => rates.iter().find(|r| r.round() as u32 == hz).map(|&r| (w, h, r)),
    }
}

/// The resolution and rate the controls show for `output`: what it runs
/// when it runs, else what its mode resolves to.
pub fn shown_mode(output: &LinuxDisplayOutput, mode: Option<&str>) -> Option<(u32, u32, f64)> {
    if mode == output.mode_override.as_deref() && output.width > 0 && output.height > 0 {
        return Some((output.width, output.height, output.refresh_hz));
    }
    resolve_mode(&output.modes, native_resolution(output), mode)
}

/// The rate a new resolution gets: the rate shown now when the new
/// resolution offers it, else its fastest.
pub fn rate_after_resolution(modes: &[(u32, u32, f64)], width: u32, height: u32, hz_now: f64) -> Option<f64> {
    let rates = rates_for(modes, width, height);
    rates.iter().copied().find(|r| r.round() == hz_now.round()).or_else(|| rates.first().copied())
}

// ------------------------------------------------------------- virtual

/// The Size picker's presets: width, height, refresh in Hz.
pub const VIRTUAL_PRESETS: [(u32, u32, u32); 6] =
    [(7680, 2160, 120), (5120, 1440, 120), (3840, 2160, 120), (3840, 1600, 120), (2560, 1440, 120), (1920, 1080, 60)];

/// The refresh rates the Custom stepper walks through, in Hz.
const CUSTOM_RATES: [u32; 14] = [24, 30, 48, 50, 60, 75, 90, 100, 120, 144, 165, 200, 240, 360];
/// How much one press of a Custom width or height stepper changes it.
const CUSTOM_STEP: u32 = 160;

/// A field of the Custom size entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustomField {
    Width,
    Height,
    Refresh,
}

/// `7680\u{d7}2160 \u{b7} 120 Hz`.
pub fn virtual_mode_label(mode: &VirtualDisplay) -> String {
    format!("{} \u{b7} {}", resolution_label(mode.width, mode.height), rate_label(mode.refresh_hz()))
}

/// The Size picker's face: the viewer's size while following (when it
/// has reported, else the fallback), else the fixed size.
pub fn virtual_size_label(saved: &SavedVirtual, shown: Option<&VirtualDisplay>) -> String {
    if saved.follow {
        match shown {
            Some(mode) => format!("Follow Desk Link viewer \u{b7} {}", virtual_mode_label(mode)),
            None => "Follow Desk Link viewer".into(),
        }
    } else {
        virtual_mode_label(&saved.mode)
    }
}

/// The Size picker's chips: Follow, the presets, then Custom (the fixed
/// size as it stands, which shows its stepper row). `(label, choice,
/// current)`.
pub fn virtual_size_choices(saved: &SavedVirtual) -> Vec<(String, VirtualSize, bool)> {
    let preset = |w, h, hz| (w, h, hz * 1000) == (saved.mode.width, saved.mode.height, saved.mode.refresh_mhz);
    let is_preset = VIRTUAL_PRESETS.iter().any(|&(w, h, hz)| preset(w, h, hz));
    let mut out = vec![("Follow viewer".to_string(), VirtualSize::Follow, saved.follow)];
    for (w, h, hz) in VIRTUAL_PRESETS {
        let size = VirtualSize::Fixed { width: w, height: h, refresh_mhz: hz * 1000 };
        out.push((format!("{w}\u{d7}{h}@{hz}"), size, !saved.follow && preset(w, h, hz)));
    }
    let fixed = VirtualSize::Fixed { width: saved.mode.width, height: saved.mode.height, refresh_mhz: saved.mode.refresh_mhz };
    out.push(("Custom".to_string(), fixed, !saved.follow && !is_preset));
    out
}

/// The fixed size after one press of a Custom stepper (`dir` -1 or +1):
/// width and height by `CUSTOM_STEP` within the display's limits, the
/// refresh to the next rate of `CUSTOM_RATES`.
pub fn step_custom(mode: &VirtualDisplay, field: CustomField, dir: i32) -> VirtualSize {
    let (mut width, mut height, mut refresh_mhz) = (mode.width, mode.height, mode.refresh_mhz);
    let step = |value: u32| {
        let moved = if dir < 0 { value.saturating_sub(CUSTOM_STEP) } else { value.saturating_add(CUSTOM_STEP) };
        moved.clamp(VirtualDisplay::MIN_SIZE, VirtualDisplay::MAX_SIZE)
    };
    match field {
        CustomField::Width => width = step(width),
        CustomField::Height => height = step(height),
        CustomField::Refresh => {
            let now = refresh_mhz / 1000;
            let next = if dir < 0 {
                CUSTOM_RATES.iter().rev().copied().find(|&hz| hz < now)
            } else {
                CUSTOM_RATES.iter().copied().find(|&hz| hz > now)
            };
            if let Some(hz) = next {
                refresh_mhz = hz * 1000;
            }
        }
    }
    VirtualSize::Fixed { width, height, refresh_mhz }
}

// --------------------------------------------------------------- screens

/// `Built-in · eDP-1` for a laptop panel; `HDMI · 3840×2160`,
/// `DisplayPort · 2560×1440` for a monitor (its native resolution), or the
/// connector when nothing better is known.
pub fn screen_title(output: &LinuxDisplayOutput) -> String {
    if is_virtual(output) {
        return "Virtual display".into();
    }
    let connector = connector_suffix(&output.name);
    let Some(name) = screen_kind(output) else { return connector };
    if is_built_in(output) {
        return format!("{name} \u{b7} {connector}");
    }
    match native_resolution(output) {
        Some((w, h)) => format!("{name} \u{b7} {}", resolution_label(w, h)),
        None => format!("{name} \u{b7} {connector}"),
    }
}

/// The virtual display's row (the desktop while Virtual is on): it has no
/// modes to pick, no place to move and no Pace by.
pub fn is_virtual(output: &LinuxDisplayOutput) -> bool {
    output.name == VIRTUAL_OUTPUT
}

/// The laptop's own panel.
pub fn is_built_in(output: &LinuxDisplayOutput) -> bool {
    let connector = connector_suffix(&output.name);
    let kind = connector.rsplit_once('-').map_or(connector.as_str(), |(kind, _)| kind);
    matches!(kind, "eDP" | "LVDS" | "DSI")
}

/// `Built-in`, `HDMI`, `DisplayPort`, `DVI`, `VGA`; `None` for anything else.
pub fn screen_kind(output: &LinuxDisplayOutput) -> Option<&'static str> {
    if is_virtual(output) {
        return Some("Virtual");
    }
    if is_built_in(output) {
        return Some("Built-in");
    }
    let connector = connector_suffix(&output.name);
    let kind = connector.rsplit_once('-').map_or(connector.as_str(), |(kind, _)| kind);
    match kind {
        "HDMI-A" | "HDMI-B" | "HDMI" => Some("HDMI"),
        "DP" => Some("DisplayPort"),
        "DVI-D" | "DVI-I" | "DVI-A" | "DVI" => Some("DVI"),
        "VGA" => Some("VGA"),
        _ => None,
    }
}

/// The monitor's own name (from its EDID), else its title.
pub fn monitor_label(output: &LinuxDisplayOutput) -> String {
    if output.monitor.is_empty() { screen_title(output) } else { output.monitor.clone() }
}

/// `Desk 27 · 60 Hz` (`HDMI · 60 Hz` without a monitor name): a monitor and
/// the rate it shows.
pub fn screen_rate_label(output: &LinuxDisplayOutput) -> String {
    let kind = if !output.monitor.is_empty() {
        output.monitor.clone()
    } else {
        screen_kind(output).map(str::to_string).unwrap_or_else(|| connector_suffix(&output.name))
    };
    format!("{kind} \u{b7} {}", rate_label(output.refresh_hz))
}

/// What the Pace by picker reads for `pacing`. Auto paces one screen by the
/// monitor it is optimised for (the renderer names it, `paced_by`); with
/// several screens each runs at its own rate.
pub fn pacing_label(pacing: &DisplayPacing, snapshot: &LinuxDisplaySnapshot) -> String {
    match pacing {
        DisplayPacing::Auto => match snapshot.paced_by.as_deref().and_then(|name| snapshot.output(name)) {
            Some(output) => format!("Auto (paced by {})", screen_rate_label(output)),
            None if snapshot.screens.len() > 1 => "Auto (each screen at its own rate)".into(),
            None => "Auto".into(),
        },
        DisplayPacing::Slowest => "Slowest screen (saves GPU)".into(),
        DisplayPacing::Fastest => "Fastest screen (smoothest)".into(),
        DisplayPacing::Screen(name) => match snapshot.output(name) {
            Some(output) => screen_rate_label(output),
            None => connector_suffix(name),
        },
    }
}

/// The Pace by choices: Auto, Slowest, Fastest, then each screen that shows.
pub fn pacing_choices(snapshot: &LinuxDisplaySnapshot) -> Vec<DisplayPacing> {
    let mut out = vec![DisplayPacing::Auto, DisplayPacing::Slowest, DisplayPacing::Fastest];
    out.extend(snapshot.outputs.iter().filter(|o| o.active).map(|o| DisplayPacing::Screen(o.name.clone())));
    out
}

/// The renderer's word that `output` is not showing what it was asked to:
/// a failed, lost or unsupported output, or one that fell back to another
/// mode. A screen of another GPU still to join is not a failure.
pub fn output_failure(output: &LinuxDisplayOutput) -> Option<&str> {
    if let Some(error) = output.error.as_deref() {
        return Some(error);
    }
    let status = output.status.as_str();
    let failed = status.starts_with("failed")
        || status.starts_with("lost")
        || status.starts_with("requested mode failed")
        || (status.starts_with("unsupported") && status != PEER_PENDING_STATUS);
    failed.then_some(status)
}

/// `output` runs and shows `mode` (`None` is automatic).
pub fn mode_matches(output: &LinuxDisplayOutput, mode: Option<&str>) -> bool {
    output.active
        && output_failure(output).is_none()
        && resolve_mode(&output.modes, native_resolution(output), mode).is_some_and(|(w, h, hz)| {
            output.width == w && output.height == h && output.refresh_hz.round() == hz.round()
        })
}

// ---------------------------------------------------------------- config

/// Where an output goes: onto screen `i` (mirroring what it shows), a new
/// screen inserted at `i` (left to right), or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Screen(usize),
    NewScreen(usize),
    Off,
}

/// The whole configuration the Display menu edits and Apply sends at once:
/// the screens and which outputs each shows, every output's mode and the
/// main screen. Pacing is not part of it: it applies at once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayConfig {
    /// Every connected output once: on a screen (left to right) or off.
    pub arrangement: DisplayArrangement,
    /// Each connected output's mode, `None` its own (automatic).
    pub modes: Vec<(String, Option<String>)>,
    /// An output of the main screen.
    pub main: Option<String>,
}

/// The connected outputs a configuration arranges: the virtual display aside.
fn arranged_names(snap: &LinuxDisplaySnapshot) -> Vec<&str> {
    snap.outputs.iter().filter(|o| !is_virtual(o)).map(|o| o.name.as_str()).collect()
}

/// `arrangement` over `names` (the connected outputs): each of its screens
/// with its connected outputs, a screen of its own for an output it does not
/// name, and its off outputs that are connected.
pub fn normalized(arrangement: &DisplayArrangement, names: &[&str]) -> DisplayArrangement {
    let screens = arrangement
        .group(names)
        .into_iter()
        .map(|g| ArrangedScreen {
            outputs: g.outputs.iter().map(|&i| names[i].to_string()).collect(),
            optimise_for: g.optimise_for.map(|i| names[i].to_string()),
        })
        .collect();
    let off = names.iter().filter(|n| arrangement.is_off(n)).map(|n| n.to_string()).collect();
    DisplayArrangement { screens, off }
}

impl DisplayConfig {
    /// What the renderer runs.
    pub fn running(snap: &LinuxDisplaySnapshot) -> DisplayConfig {
        let names = arranged_names(snap);
        let arrangement = normalized(&snap.arrangement, &names);
        let modes = names.iter().map(|n| (n.to_string(), snap.output(n).and_then(|o| o.mode_override.clone()))).collect();
        let main = snap
            .main_screen()
            .and_then(|s| s.outputs.first().cloned())
            .filter(|m| arrangement.screens.iter().any(|s| s.outputs.contains(m)))
            .or_else(|| arrangement.screens.first().and_then(|s| s.outputs.first().cloned()));
        DisplayConfig { arrangement, modes, main }
    }

    pub fn mode(&self, name: &str) -> Option<&str> {
        self.modes.iter().find(|(n, _)| n == name).and_then(|(_, m)| m.as_deref())
    }

    /// The screen output `name` is on, or `Off`; `None` when it is not arranged.
    pub fn place_of(&self, name: &str) -> Option<Place> {
        if self.arrangement.is_off(name) {
            return Some(Place::Off);
        }
        self.arrangement.screens.iter().position(|s| s.outputs.iter().any(|o| o == name)).map(Place::Screen)
    }

    /// The main screen's index.
    pub fn main_screen(&self) -> Option<usize> {
        let main = self.main.as_deref()?;
        self.arrangement.screens.iter().position(|s| s.outputs.iter().any(|o| o == main))
    }

    fn on_count(&self) -> usize {
        self.arrangement.screens.iter().map(|s| s.outputs.len()).sum()
    }

    /// Moves output `name` to `to`. A screen left without outputs goes; the
    /// main screen stays main (its next output carries it), or the main
    /// output takes it along when its screen went. `false`, nothing changed,
    /// for a move to where it is, or one that would turn the last output off.
    pub fn move_output(&mut self, name: &str, to: Place) -> bool {
        let Some(from) = self.place_of(name) else { return false };
        let alone = matches!(from, Place::Screen(i) if self.arrangement.screens[i].outputs.len() == 1);
        let unchanged = match (from, to) {
            (Place::Off, Place::Off) => true,
            (Place::Screen(i), Place::Screen(j)) => i == j,
            // A screen of its own moved next to itself is where it was.
            (Place::Screen(i), Place::NewScreen(j)) => alone && (j == i || j == i + 1),
            _ => false,
        };
        if unchanged || (to == Place::Off && from != Place::Off && self.on_count() == 1) {
            return false;
        }
        let was_main = self.main.as_deref() == Some(name);
        let mut to = to;
        let mut left_behind = None;
        match from {
            // Where an output is: on a screen or off, never a slot.
            Place::NewScreen(_) => return false,
            Place::Off => self.arrangement.off.retain(|o| o != name),
            Place::Screen(i) => {
                let screen = &mut self.arrangement.screens[i];
                screen.outputs.retain(|o| o != name);
                if screen.optimise_for.as_deref() == Some(name) {
                    screen.optimise_for = None;
                }
                if screen.outputs.is_empty() {
                    self.arrangement.screens.remove(i);
                    to = match to {
                        Place::Screen(j) if j > i => Place::Screen(j - 1),
                        Place::NewScreen(j) if j > i => Place::NewScreen(j - 1),
                        other => other,
                    };
                } else {
                    left_behind = screen.outputs.first().cloned();
                }
            }
        }
        match to {
            Place::Screen(j) => self.arrangement.screens[j].outputs.push(name.to_string()),
            Place::NewScreen(j) => {
                let at = j.min(self.arrangement.screens.len());
                self.arrangement.screens.insert(at, ArrangedScreen { outputs: vec![name.to_string()], optimise_for: None });
            }
            Place::Off => self.arrangement.off.push(name.to_string()),
        }
        if was_main {
            self.main = left_behind.or_else(|| (to != Place::Off).then(|| name.to_string()));
        }
        if self.main_screen().is_none() {
            self.main = self.arrangement.screens.first().and_then(|s| s.outputs.first().cloned());
        }
        true
    }

    /// Moves screen `from` into slot `to` (`0..=screens`, left to right, the
    /// slot before the screen at that index).
    pub fn move_screen(&mut self, from: usize, to: usize) -> bool {
        let len = self.arrangement.screens.len();
        if from >= len || to > len || to == from || to == from + 1 {
            return false;
        }
        let screen = self.arrangement.screens.remove(from);
        self.arrangement.screens.insert(if to > from { to - 1 } else { to }, screen);
        true
    }

    /// Screen `screen` is optimised for output `name` (one it shows).
    pub fn set_optimise_for(&mut self, screen: usize, name: &str) -> bool {
        let Some(s) = self.arrangement.screens.get_mut(screen) else { return false };
        if !s.outputs.iter().any(|o| o == name) || s.optimise_for.as_deref() == Some(name) {
            return false;
        }
        s.optimise_for = Some(name.to_string());
        true
    }

    /// Screen `screen` becomes the main screen.
    pub fn set_main(&mut self, screen: usize) -> bool {
        if self.main_screen() == Some(screen) {
            return false;
        }
        match self.arrangement.screens.get(screen).and_then(|s| s.outputs.first()) {
            Some(first) => {
                self.main = Some(first.clone());
                true
            }
            None => false,
        }
    }

    /// Output `name` in `mode` (`None` its own).
    pub fn set_mode(&mut self, name: &str, mode: Option<String>) -> bool {
        match self.modes.iter_mut().find(|(n, _)| n == name) {
            Some((_, m)) if *m == mode => false,
            Some((_, m)) => {
                *m = mode;
                true
            }
            None => {
                self.modes.push((name.to_string(), mode));
                true
            }
        }
    }

    /// Going from `from` to this reinitialises an output: a mode changes, or
    /// an output turns on or off. Moving outputs between screens, the
    /// optimise-for choice and the main screen only lay the desktop out again.
    pub fn modesets(&self, from: &DisplayConfig) -> bool {
        let mut off = self.arrangement.off.clone();
        let mut was = from.arrangement.off.clone();
        off.sort();
        was.sort();
        off != was || self.modes.iter().chain(from.modes.iter()).any(|(n, _)| self.mode(n) != from.mode(n))
    }

    /// The renderer requests that bring it from `from` to this: the modes
    /// that differ, the arrangement, then the main screen.
    pub fn commands(&self, from: &DisplayConfig) -> Vec<LinuxDisplayCommand> {
        let mut out = Vec::new();
        for (name, mode) in &self.modes {
            if from.mode(name) != mode.as_deref() {
                out.push(LinuxDisplayCommand::Mode { name: name.clone(), mode: mode.clone() });
            }
        }
        if self.arrangement != from.arrangement {
            out.push(LinuxDisplayCommand::Arrangement(self.arrangement.clone()));
        }
        if let Some(main) = self.main.as_ref().filter(|_| self.main != from.main) {
            out.push(LinuxDisplayCommand::Source(main.clone()));
        }
        out
    }

    /// The renderer shows it: its arrangement, every output on a screen in
    /// its mode, and its main screen.
    pub fn shown_by(&self, snap: &LinuxDisplaySnapshot) -> bool {
        normalized(&snap.arrangement, &arranged_names(snap)) == self.arrangement
            && self.arrangement.screens.iter().flat_map(|s| s.outputs.iter()).all(|name| {
                snap.output(name).is_some_and(|o| o.mode_override.as_deref() == self.mode(name) && mode_matches(o, self.mode(name)))
            })
            && self.main.as_deref().is_some_and(|m| snap.screen_of(m).is_some_and(|s| s.primary))
    }

    /// Why the renderer does not show it, if it says: an output on a screen
    /// is gone, failed, or runs another mode than it was asked for.
    pub fn failure(&self, snap: &LinuxDisplaySnapshot) -> Option<String> {
        for name in self.arrangement.screens.iter().flat_map(|s| s.outputs.iter()) {
            let Some(output) = snap.output(name) else {
                return Some(format!("{} was disconnected", connector_suffix(name)));
            };
            if let Some(reason) = output_failure(output) {
                return Some(format!("{}: {reason}", monitor_label(output)));
            }
            if output.active && output.mode_override.as_deref() == self.mode(name) && output.width > 0 && !mode_matches(output, self.mode(name)) {
                return Some(format!("{} shows {} instead", monitor_label(output), mode_label(output.width, output.height, output.refresh_hz)));
            }
        }
        None
    }

    /// The size output `name` runs at in this configuration: its mode as
    /// what the monitor offers resolves it, else what it shows now.
    pub fn output_size(&self, snap: &LinuxDisplaySnapshot, name: &str) -> (u32, u32) {
        let Some(output) = snap.output(name) else { return (0, 0) };
        match resolve_mode(&output.modes, native_resolution(output), self.mode(name)) {
            Some((w, h, _)) => (w, h),
            None => (output.width, output.height),
        }
    }

    /// The output screen `screen` is optimised for: its own choice, else its
    /// largest monitor (the first of equals), as the renderer picks.
    pub fn optimise_for(&self, snap: &LinuxDisplaySnapshot, screen: usize) -> Option<&str> {
        let s = self.arrangement.screens.get(screen)?;
        if let Some(name) = s.optimise_for.as_deref() {
            return Some(name);
        }
        let mut best: Option<(&str, u64)> = None;
        for name in &s.outputs {
            let (w, h) = self.output_size(snap, name);
            let area = w as u64 * h as u64;
            if best.is_none_or(|(_, a)| area > a) {
                best = Some((name, area));
            }
        }
        best.map(|(name, _)| name)
    }

    /// Screen `screen`'s size: its optimise-for output's.
    pub fn screen_size(&self, snap: &LinuxDisplaySnapshot, screen: usize) -> (u32, u32) {
        self.optimise_for(snap, screen).map_or((0, 0), |name| self.output_size(snap, name))
    }
}

// ---------------------------------------------------------------- trials

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Sent at `since` with the display generation it was sent at; judged
    /// on later generations only, so an older state is not taken for it.
    Applying { since: f64, generation: u64 },
    /// Shown; reverted at `deadline` unless kept.
    Confirming { deadline: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Trial {
    pub config: DisplayConfig,
    /// The last kept configuration: what Revert sends.
    pub previous: DisplayConfig,
    /// It reinitialises an output, so it waits for Keep.
    pub modeset: bool,
    pub phase: Phase,
}

/// What the caller does next.
#[derive(Clone, Debug, PartialEq)]
pub enum TrialOut {
    /// Ask the renderer for `to`, from `from` (`DisplayConfig::commands`).
    Send { to: DisplayConfig, from: DisplayConfig },
    /// It is in place (and kept, if it had to be): save it.
    Save(DisplayConfig),
}

/// The line under the Apply row.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub text: String,
    pub error: bool,
}

/// Apply's try → keep / auto-revert state machine. A configuration that
/// reinitialises no output is saved as soon as the renderer shows it; one
/// that does waits for Keep, and is reverted after `CONFIRM_SECONDS`, when
/// it fails, or on Revert.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayTrials {
    pub trial: Option<Trial>,
    /// A failed or reverted configuration being undone: the menu shows
    /// this one until the renderer does (or the apply timeout passes).
    pub restoring: Option<(DisplayConfig, f64)>,
    pub note: Option<Note>,
}

impl DisplayTrials {
    /// Apply `config`; `running` is what the renderer shows now. An
    /// unconfirmed trial is replaced (Revert still goes back to the kept
    /// one); applying the kept configuration again is a revert.
    pub fn apply(&mut self, config: DisplayConfig, running: DisplayConfig, generation: u64, now: f64) -> Vec<TrialOut> {
        self.note = None;
        let (previous, replaced) = match self.trial.take() {
            Some(trial) => (trial.previous, trial.config),
            None => (running.clone(), running),
        };
        if config == previous {
            if replaced == previous {
                return Vec::new();
            }
            self.restoring = Some((previous.clone(), now));
            return vec![TrialOut::Send { to: previous, from: replaced }];
        }
        self.restoring = None;
        let modeset = config.modesets(&previous);
        self.trial = Some(Trial { config: config.clone(), previous, modeset, phase: Phase::Applying { since: now, generation } });
        vec![TrialOut::Send { to: config, from: replaced }]
    }

    /// The renderer (or the local check) refused a request of the trial:
    /// whatever of it was taken is undone.
    pub fn refused(&mut self, message: &str, now: f64) -> Vec<TrialOut> {
        let Some(trial) = self.trial.take() else { return Vec::new() };
        self.note = Some(Note { text: format!("Couldn't apply: {message}"), error: true });
        self.restoring = Some((trial.previous.clone(), now));
        vec![TrialOut::Send { to: trial.previous, from: trial.config }]
    }

    /// One look at the renderer's snapshot (`generation` is its display
    /// generation) at `now`.
    pub fn observe(&mut self, snapshot: &LinuxDisplaySnapshot, generation: u64, now: f64) -> Vec<TrialOut> {
        if let Some((config, since)) = &self.restoring {
            if config.shown_by(snapshot) || now - since > APPLY_TIMEOUT {
                self.restoring = None;
            }
        }
        let Some(trial) = self.trial.as_mut() else { return Vec::new() };
        match trial.phase {
            Phase::Applying { since, generation: sent } => {
                if generation > sent && trial.config.shown_by(snapshot) {
                    if trial.modeset {
                        trial.phase = Phase::Confirming { deadline: now + CONFIRM_SECONDS };
                        return Vec::new();
                    }
                    let trial = self.trial.take().unwrap();
                    return vec![TrialOut::Save(trial.config)];
                }
                let failure = if generation > sent { trial.config.failure(snapshot) } else { None };
                match failure {
                    Some(reason) => self.fail(&reason, now),
                    None if now - since > APPLY_TIMEOUT => self.fail(&format!("it did not come up within {} s", APPLY_TIMEOUT as u32), now),
                    None => Vec::new(),
                }
            }
            Phase::Confirming { deadline } => match trial.config.failure(snapshot) {
                Some(reason) => self.fail(&reason, now),
                None if now >= deadline => {
                    let trial = self.trial.take().unwrap();
                    self.note = Some(Note { text: "Not kept: back to the previous settings".into(), error: false });
                    self.restoring = Some((trial.previous.clone(), now));
                    vec![TrialOut::Send { to: trial.previous, from: trial.config }]
                }
                None => Vec::new(),
            },
        }
    }

    /// "Keep": saves a configuration that is up.
    pub fn keep(&mut self) -> Vec<TrialOut> {
        match self.trial.take_if(|t| matches!(t.phase, Phase::Confirming { .. })) {
            Some(trial) => vec![TrialOut::Save(trial.config)],
            None => Vec::new(),
        }
    }

    /// "Revert": back to the kept configuration now.
    pub fn revert(&mut self, now: f64) -> Vec<TrialOut> {
        match self.trial.take() {
            Some(trial) => {
                self.restoring = Some((trial.previous.clone(), now));
                vec![TrialOut::Send { to: trial.previous, from: trial.config }]
            }
            None => Vec::new(),
        }
    }

    /// Whole seconds until the revert, while confirming.
    pub fn seconds_left(&self, now: f64) -> Option<u32> {
        match self.trial.as_ref()?.phase {
            Phase::Confirming { deadline } => Some((deadline - now).max(0.0).ceil() as u32),
            Phase::Applying { .. } => None,
        }
    }

    /// The configuration the menu shows: the one being tried, the one
    /// being restored, else `running`.
    pub fn shown(&self, running: DisplayConfig) -> DisplayConfig {
        self.trial
            .as_ref()
            .map(|t| t.config.clone())
            .or_else(|| self.restoring.as_ref().map(|(c, _)| c.clone()))
            .unwrap_or(running)
    }

    fn fail(&mut self, reason: &str, now: f64) -> Vec<TrialOut> {
        let trial = self.trial.take().unwrap();
        self.note = Some(Note { text: format!("Couldn't apply: {reason}. Back to the previous settings"), error: true });
        self.restoring = Some((trial.previous.clone(), now));
        vec![TrialOut::Send { to: trial.previous, from: trial.config }]
    }
}

/// The gallery's Display menu: the person's four monitors as [B] [A] [C+D],
/// left to right, the main screen in the middle and a mirror on the right
/// (a 4K wall screen with a QHD desk monitor, optimised for the larger).
pub fn gallery_fixture() -> (LinuxDisplaySnapshot, DisplayTrials) {
    let monitor = |name: &str, monitor: &str, fingerprint: u64, modes: Vec<(u32, u32, f64)>| {
        let (width, height, refresh_hz) = modes[0];
        LinuxDisplayOutput {
            name: name.into(),
            monitor: monitor.into(),
            fingerprint,
            width,
            height,
            refresh_hz,
            card: "card0".into(),
            active: true,
            status: "active".into(),
            native: Some(modes[0]),
            modes,
            pci: Some("0000:01:00.0".into()),
            ..Default::default()
        }
    };
    let qhd = || vec![(2560, 1440, 144.0), (2560, 1440, 120.0), (2560, 1440, 60.0), (1920, 1080, 144.0), (1920, 1080, 60.0), (1280, 720, 60.0)];
    let a = monitor("card0-DP-1", "Center 27", 0xa1, qhd());
    let b = monitor("card0-DP-2", "Side 24", 0xb2, vec![(1920, 1080, 60.0), (1920, 1080, 50.0), (1280, 720, 60.0)]);
    let c = monitor("card0-DP-3", "Wall 55", 0xc3, vec![(3840, 2160, 60.0), (3840, 2160, 30.0), (2560, 1440, 60.0), (1920, 1080, 60.0)]);
    let d = monitor("card0-DP-4", "Desk 27", 0xd4, qhd());
    let screen = |name: &str, outputs: &[&str], x: u32, (width, height): (u32, u32), primary: bool| LinuxDisplayScreen {
        name: name.into(),
        outputs: outputs.iter().map(|o| o.to_string()).collect(),
        optimise_for: outputs[0].into(),
        x,
        y: 0,
        width,
        height,
        primary,
    };
    let snapshot = LinuxDisplaySnapshot {
        direct: true,
        outputs: vec![a, b, c, d],
        screens: vec![
            screen("card0-DP-2", &["card0-DP-2"], 0, (1920, 1080), false),
            screen("card0-DP-1", &["card0-DP-1"], 1920, (2560, 1440), true),
            screen("card0-DP-3", &["card0-DP-3", "card0-DP-4"], 4480, (3840, 2160), false),
        ],
        arrangement: DisplayArrangement::parse("card0-DP-2,card0-DP-1,card0-DP-3+card0-DP-4").unwrap(),
        ..Default::default()
    };
    (snapshot, DisplayTrials::default())
}

/// The gallery's stand-in for the renderer: `snap` as it is once it shows
/// `config` (every output on a screen in its mode, the screens laid out
/// left to right along their tops).
pub fn gallery_shows(snap: &LinuxDisplaySnapshot, config: &DisplayConfig) -> LinuxDisplaySnapshot {
    let mut out = snap.clone();
    out.arrangement = config.arrangement.clone();
    for o in &mut out.outputs {
        let mode = config.mode(&o.name).map(str::to_string);
        if let Some((w, h, hz)) = resolve_mode(&o.modes, native_resolution(o), mode.as_deref()) {
            (o.width, o.height, o.refresh_hz) = (w, h, hz);
        }
        o.mode_override = mode;
        o.active = !config.arrangement.is_off(&o.name);
        o.status = if o.active { "active".into() } else { "off".into() };
    }
    let main = config.main_screen();
    let mut x = 0;
    out.screens = config
        .arrangement
        .screens
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let (width, height) = config.screen_size(snap, i);
            let screen = LinuxDisplayScreen {
                name: s.outputs[0].clone(),
                outputs: s.outputs.clone(),
                optimise_for: config.optimise_for(snap, i).unwrap_or_default().to_string(),
                x,
                y: 0,
                width,
                height,
                primary: main == Some(i),
            };
            x += width;
            screen
        })
        .collect();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_platform::linux_display::VirtualPanels;

    fn saved(follow: bool, width: u32, height: u32, hz: u32) -> SavedVirtual {
        SavedVirtual {
            mode: VirtualDisplay { width, height, refresh_mhz: hz * 1000, panels: VirtualPanels::Mirror },
            scale: 200,
            follow,
        }
    }

    #[test]
    fn the_virtual_row_is_titled_and_has_no_per_screen_pickers() {
        let row = LinuxDisplayOutput { name: VIRTUAL_OUTPUT.into(), width: 7680, height: 2160, refresh_hz: 120.0, active: true, ..Default::default() };
        assert!(is_virtual(&row));
        assert_eq!(screen_title(&row), "Virtual display");
        assert_eq!(screen_kind(&row), Some("Virtual"));
        assert!(!is_virtual(&LinuxDisplayOutput { name: "card0-HDMI-A-1".into(), ..Default::default() }));
    }

    #[test]
    fn size_choices_mark_follow_a_preset_or_custom() {
        let current = |s: &SavedVirtual| virtual_size_choices(s).into_iter().filter(|c| c.2).map(|c| c.0).collect::<Vec<_>>();
        assert_eq!(virtual_size_choices(&saved(true, 7680, 2160, 120)).len(), 8);
        assert_eq!(current(&saved(true, 7680, 2160, 120)), vec!["Follow viewer"]);
        assert_eq!(current(&saved(false, 5120, 1440, 120)), vec!["5120\u{d7}1440@120"]);
        assert_eq!(current(&saved(false, 3000, 1000, 75)), vec!["Custom"]);
    }

    #[test]
    fn size_label_reads_the_viewer_while_following() {
        let s = saved(true, 7680, 2160, 120);
        assert_eq!(virtual_size_label(&s, None), "Follow Desk Link viewer");
        let shown = VirtualDisplay { width: 3840, height: 2160, refresh_mhz: 60_000, panels: VirtualPanels::Mirror };
        assert!(virtual_size_label(&s, Some(&shown)).starts_with("Follow Desk Link viewer \u{b7} 3840"));
        assert_eq!(virtual_size_label(&saved(false, 2560, 1440, 120), Some(&shown)), virtual_mode_label(&saved(false, 2560, 1440, 120).mode));
    }

    #[test]
    fn custom_steppers_stay_in_limits_and_walk_the_rates() {
        let mode = saved(false, 1920, 1080, 60).mode;
        assert_eq!(step_custom(&mode, CustomField::Width, 1), VirtualSize::Fixed { width: 2080, height: 1080, refresh_mhz: 60_000 });
        assert_eq!(step_custom(&mode, CustomField::Height, -1), VirtualSize::Fixed { width: 1920, height: 920, refresh_mhz: 60_000 });
        assert_eq!(step_custom(&mode, CustomField::Refresh, 1), VirtualSize::Fixed { width: 1920, height: 1080, refresh_mhz: 75_000 });
        assert_eq!(step_custom(&mode, CustomField::Refresh, -1), VirtualSize::Fixed { width: 1920, height: 1080, refresh_mhz: 50_000 });
        let tiny = saved(false, 64, 64, 24).mode;
        assert_eq!(step_custom(&tiny, CustomField::Width, -1), VirtualSize::Fixed { width: 64, height: 64, refresh_mhz: 24_000 });
        assert_eq!(step_custom(&tiny, CustomField::Refresh, -1), VirtualSize::Fixed { width: 64, height: 64, refresh_mhz: 24_000 });
        let huge = saved(false, 16384, 16384, 360).mode;
        assert_eq!(step_custom(&huge, CustomField::Height, 1), VirtualSize::Fixed { width: 16384, height: 16384, refresh_mhz: 360_000 });
    }

    const HDMI: &[(u32, u32, f64)] = &[
        (3840, 2160, 239.995),
        (3840, 2160, 144.0),
        (3840, 2160, 60.0),
        (3840, 2160, 59.94),
        (3840, 2160, 30.0),
        (2560, 1440, 239.97),
        (2560, 1440, 120.0),
        (1920, 1080, 240.0),
        (1920, 1080, 60.0),
        (1920, 1080, 50.0),
    ];

    fn hdmi(mode: Option<&str>, width: u32, height: u32, hz: f64) -> LinuxDisplayOutput {
        LinuxDisplayOutput {
            name: "card1-HDMI-A-2".into(),
            card: "card1".into(),
            width,
            height,
            refresh_hz: hz,
            active: true,
            status: "active".into(),
            modes: HDMI.to_vec(),
            mode_override: mode.map(str::to_string),
            ..Default::default()
        }
    }

    fn snap(outputs: Vec<LinuxDisplayOutput>) -> LinuxDisplaySnapshot {
        LinuxDisplaySnapshot { direct: true, outputs, ..Default::default() }
    }

    #[test]
    fn resolutions_once_each_native_first() {
        let r = resolutions(HDMI, None);
        let sizes: Vec<(u32, u32, bool)> = r.iter().map(|r| (r.width, r.height, r.native)).collect();
        assert_eq!(sizes, [(3840, 2160, true), (2560, 1440, false), (1920, 1080, false)]);
        assert!(resolutions(&[], None).is_empty());
    }

    #[test]
    fn rates_are_per_resolution_fastest_first_whole_hertz_once() {
        assert_eq!(rates_for(HDMI, 3840, 2160), [239.995, 144.0, 60.0, 30.0]);
        assert_eq!(rates_for(HDMI, 2560, 1440), [239.97, 120.0]);
        assert_eq!(rates_for(HDMI, 1920, 1080), [240.0, 60.0, 50.0]);
        assert!(rates_for(HDMI, 1280, 720).is_empty());
        // Unordered input with a bad rate.
        assert_eq!(rates_for(&[(800, 600, 60.0), (800, 600, f64::NAN), (800, 600, 75.0)], 800, 600), [75.0, 60.0]);
        assert_eq!(rate_label(239.995), "240 Hz");
    }

    #[test]
    fn a_new_resolution_keeps_the_rate_when_it_can() {
        assert_eq!(rate_after_resolution(HDMI, 1920, 1080, 60.0), Some(60.0));
        assert_eq!(rate_after_resolution(HDMI, 2560, 1440, 60.0), Some(239.97));
        assert_eq!(rate_after_resolution(HDMI, 1920, 1080, 239.995), Some(240.0));
        assert_eq!(rate_after_resolution(HDMI, 1280, 720, 60.0), None);
    }

    #[test]
    fn native_fastest_is_automatic() {
        assert_eq!(mode_for(HDMI, None, 3840, 2160, 239.995), None);
        assert_eq!(mode_for(HDMI, None, 3840, 2160, 60.0), Some("3840x2160@60".into()));
        assert_eq!(mode_for(HDMI, None, 1920, 1080, 240.0), Some("1920x1080@240".into()));
        assert_eq!(resolve_mode(HDMI, None, None), Some((3840, 2160, 239.995)));
        assert_eq!(resolve_mode(HDMI, None, Some("1920x1080")), Some((1920, 1080, 240.0)));
        assert_eq!(resolve_mode(HDMI, None, Some("3840x2160@60")), Some((3840, 2160, 60.0)));
        assert_eq!(resolve_mode(HDMI, None, Some("3840x2160-60")), Some((3840, 2160, 60.0)));
        assert_eq!(resolve_mode(HDMI, None, Some("3840x2160@75")), None);
    }

    #[test]
    fn pacing_auto_names_the_pacer_of_one_screen_and_lets_several_run_at_their_rates() {
        let (mut s, _) = gallery_fixture();
        assert_eq!(pacing_label(&DisplayPacing::Auto, &s), "Auto (each screen at its own rate)");
        s.screens.truncate(1);
        assert_eq!(pacing_label(&DisplayPacing::Auto, &s), "Auto");
        s.paced_by = Some("card0-DP-3".into());
        assert_eq!(pacing_label(&DisplayPacing::Auto, &s), "Auto (paced by Wall 55 \u{b7} 60 Hz)");
        assert_eq!(pacing_label(&DisplayPacing::Screen("card0-DP-1".into()), &s), "Center 27 \u{b7} 144 Hz");
        assert_eq!(pacing_label(&DisplayPacing::Fastest, &s), "Fastest screen (smoothest)");
        assert_eq!(pacing_choices(&s).len(), 7);
    }

    #[test]
    fn the_edid_preferred_mode_is_native() {
        let mut o = hdmi(None, 0, 0, 0.0);
        o.native = Some((2560, 1440, 144.0));
        let n = native_resolution(&o);
        assert_eq!(n, Some((2560, 1440)));
        let r = resolutions(&o.modes, n);
        assert!(r.iter().find(|r| r.native).is_some_and(|r| (r.width, r.height) == (2560, 1440)));
        assert_eq!(resolve_mode(&o.modes, n, None), Some((2560, 1440, 239.97)));
        assert_eq!(mode_for(&o.modes, n, 2560, 1440, 239.97), None);
        assert_eq!(mode_for(&o.modes, n, 3840, 2160, 239.995), Some("3840x2160@240".into()));
        assert_eq!(screen_title(&o), "HDMI \u{b7} 2560\u{d7}1440");
    }

    #[test]
    fn titles() {
        let mut edp = hdmi(None, 2560, 1600, 240.0);
        edp.name = "card0-eDP-1".into();
        assert_eq!(screen_title(&edp), "Built-in \u{b7} eDP-1");
        assert_eq!(screen_title(&hdmi(None, 0, 0, 0.0)), "HDMI \u{b7} 3840\u{d7}2160");
        let mut dp = hdmi(None, 0, 0, 0.0);
        dp.name = "card1-DP-3".into();
        dp.modes.clear();
        assert_eq!(screen_title(&dp), "DisplayPort \u{b7} DP-3");
    }

    const A: &str = "card0-DP-1";
    const B: &str = "card0-DP-2";
    const C: &str = "card0-DP-3";
    const D: &str = "card0-DP-4";

    /// `snap` once the renderer shows `config`.
    #[test]
    fn the_users_desktop_is_three_screens_with_the_mirror_sized_by_its_choice() {
        let (snap, _) = gallery_fixture();
        let running = DisplayConfig::running(&snap);
        assert_eq!(running.arrangement.text(), format!("{B},{A},{C}+{D}"));
        assert_eq!((running.main.as_deref(), running.main_screen()), (Some(A), Some(1)), "the main screen in the middle");
        assert!(running.shown_by(&snap));
        assert_eq!(running.optimise_for(&snap, 2), Some(C), "the larger monitor by default");
        assert_eq!(running.screen_size(&snap, 2), (3840, 2160));
        let mut desk = running.clone();
        assert!(desk.set_optimise_for(2, D));
        assert_eq!(desk.screen_size(&snap, 2), (2560, 1440), "optimised for D: D's size");
        assert!(!desk.modesets(&running), "no modeset: the desktop is laid out again");
        assert_eq!(desk.commands(&running).len(), 1);
        assert!(!desk.set_optimise_for(2, D) && !desk.set_optimise_for(2, A), "already, or not on that screen");
        // A smaller mode on C makes D the larger one.
        let mut small = running.clone();
        small.set_mode(C, Some("1920x1080@60".into()));
        assert_eq!(small.optimise_for(&snap, 2), Some(D));
        assert!(small.modesets(&running));
        assert!(!running.clone().set_main(1), "already main");
    }

    #[test]
    fn monitors_drop_onto_screens_new_screens_and_off() {
        let (snap, _) = gallery_fixture();
        let running = DisplayConfig::running(&snap);
        let mut c = running.clone();
        assert!(c.move_output(D, Place::NewScreen(3)), "D gets a screen of its own on the right");
        assert_eq!(c.arrangement.text(), format!("{B},{A},{C},{D}"));
        assert!(!c.modesets(&running));
        assert!(c.move_output(D, Place::Screen(0)), "D mirrors B");
        assert_eq!(c.arrangement.text(), format!("{B}+{D},{A},{C}"));
        assert!(!c.move_output(D, Place::Screen(0)), "where it is");
        assert!(!c.move_output(C, Place::NewScreen(2)) && !c.move_output(C, Place::NewScreen(3)), "beside itself, alone");
        // The main monitor, alone, onto the mirror: the main screen goes with it.
        let mut m = running.clone();
        assert!(m.move_output(A, Place::Screen(2)));
        assert_eq!(m.arrangement.text(), format!("{B},{C}+{D}+{A}"));
        assert_eq!((m.main.as_deref(), m.main_screen()), (Some(A), Some(1)));
        // Off, and back on: a modeset both ways.
        let mut off = running.clone();
        assert!(off.move_output(C, Place::Off));
        assert_eq!(off.arrangement.text(), format!("{B},{A},{D},off:{C}"));
        assert!(off.modesets(&running));
        assert!(off.move_output(C, Place::Screen(2)));
        assert_eq!(off.arrangement.text(), format!("{B},{A},{D}+{C}"));
        // The last monitor on stays on.
        let mut last = running.clone();
        for name in [B, C, D] {
            assert!(last.move_output(name, Place::Off));
        }
        assert!(!last.move_output(A, Place::Off));
        // A mirrored main screen stays main when its first monitor leaves.
        let mut pair = running.clone();
        assert!(pair.move_output(B, Place::Screen(1)));
        assert_eq!(pair.arrangement.text(), format!("{A}+{B},{C}+{D}"));
        assert!(pair.move_output(A, Place::NewScreen(0)));
        assert_eq!((pair.arrangement.text(), pair.main.as_deref(), pair.main_screen()), (format!("{A},{B},{C}+{D}"), Some(B), Some(1)));
    }

    #[test]
    fn the_main_screen_works_in_any_position() {
        let (snap, _) = gallery_fixture();
        let running = DisplayConfig::running(&snap);
        let mut c = running.clone();
        assert!(c.move_screen(1, 0));
        assert_eq!((c.arrangement.text(), c.main_screen()), (format!("{A},{B},{C}+{D}"), Some(0)));
        assert!(c.move_screen(0, 3));
        assert_eq!((c.arrangement.text(), c.main_screen()), (format!("{B},{C}+{D},{A}"), Some(2)), "main on the right");
        assert!(!c.move_screen(0, 0) && !c.move_screen(0, 1) && !c.move_screen(5, 0) && !c.move_screen(0, 9));
        assert!(c.set_main(1));
        assert_eq!(c.main.as_deref(), Some(C));
        assert!(!c.set_main(1) && !c.set_main(7));
        let commands = c.commands(&running);
        assert!(matches!(commands.as_slice(), [LinuxDisplayCommand::Arrangement(_), LinuxDisplayCommand::Source(s)] if s == C));
        assert!(!c.modesets(&running));
        assert!(gallery_shows(&snap, &c).screens[1].primary);
        assert!(c.shown_by(&gallery_shows(&snap, &c)) && !running.shown_by(&gallery_shows(&snap, &c)));
    }

    #[test]
    fn a_layout_change_is_saved_once_shown_without_a_keep() {
        let (snap, _) = gallery_fixture();
        let running = DisplayConfig::running(&snap);
        let mut c = running.clone();
        c.move_output(D, Place::NewScreen(3));
        let mut t = DisplayTrials::default();
        assert_eq!(t.apply(c.clone(), running.clone(), 5, 100.0), [TrialOut::Send { to: c.clone(), from: running.clone() }]);
        assert_eq!(t.shown(running.clone()), c);
        assert!(t.observe(&gallery_shows(&snap, &c), 5, 100.1).is_empty(), "not judged on the generation it was sent at");
        assert_eq!(t.observe(&gallery_shows(&snap, &c), 6, 100.2), [TrialOut::Save(c.clone())]);
        assert!(t.trial.is_none() && t.seconds_left(100.2).is_none());
        // Applying what runs sends nothing.
        assert!(t.apply(running.clone(), running.clone(), 7, 101.0).is_empty());
    }

    #[test]
    fn a_mode_change_waits_for_keep_and_reverts_on_its_own() {
        let (snap, _) = gallery_fixture();
        let running = DisplayConfig::running(&snap);
        let mut c = running.clone();
        c.set_mode(A, Some("1920x1080@60".into()));
        let mut t = DisplayTrials::default();
        t.apply(c.clone(), running.clone(), 5, 100.0);
        assert!(t.observe(&gallery_shows(&snap, &c), 6, 101.0).is_empty());
        assert_eq!(t.seconds_left(101.0), Some(15));
        assert_eq!(t.seconds_left(110.2), Some(6));
        assert_eq!(t.keep(), [TrialOut::Save(c.clone())]);
        assert!(t.keep().is_empty());
        // Not kept in time: the previous settings go back.
        let mut t = DisplayTrials::default();
        t.apply(c.clone(), running.clone(), 5, 100.0);
        t.observe(&gallery_shows(&snap, &c), 6, 101.0);
        assert!(t.observe(&gallery_shows(&snap, &c), 7, 115.0).is_empty());
        assert_eq!(t.observe(&gallery_shows(&snap, &c), 8, 116.1), [TrialOut::Send { to: running.clone(), from: c.clone() }]);
        assert_eq!(t.note.as_ref().map(|n| n.error), Some(false));
        assert_eq!(t.shown(c.clone()), running, "the previous settings show while they come back");
        t.observe(&snap, 9, 116.5);
        assert!(t.restoring.is_none());
        // Revert, and applying the kept settings again, go back at once.
        let mut t = DisplayTrials::default();
        t.apply(c.clone(), running.clone(), 5, 100.0);
        assert_eq!(t.revert(100.5), [TrialOut::Send { to: running.clone(), from: c.clone() }]);
        let mut t = DisplayTrials::default();
        t.apply(c.clone(), running.clone(), 5, 100.0);
        assert_eq!(t.apply(running.clone(), c.clone(), 6, 100.5), [TrialOut::Send { to: running.clone(), from: c.clone() }]);
    }

    #[test]
    fn a_failure_a_refusal_or_a_timeout_sends_the_previous_settings_back() {
        let (snap, _) = gallery_fixture();
        let running = DisplayConfig::running(&snap);
        let mut c = running.clone();
        c.set_mode(A, Some("1280x720@60".into()));
        let back = [TrialOut::Send { to: running.clone(), from: c.clone() }];
        let mut failed = gallery_shows(&snap, &c);
        failed.outputs[0].status = "failed: no mode".into();
        let mut t = DisplayTrials::default();
        t.apply(c.clone(), running.clone(), 5, 100.0);
        assert_eq!(t.observe(&failed, 6, 101.0), back);
        assert!(t.note.as_ref().is_some_and(|n| n.error && n.text.contains("Center 27: failed: no mode")), "{:?}", t.note);
        let mut t = DisplayTrials::default();
        t.apply(c.clone(), running.clone(), 5, 100.0);
        assert_eq!(t.refused("no such mode", 100.1), back);
        assert!(t.refused("again", 100.2).is_empty(), "one undo per trial");
        let mut t = DisplayTrials::default();
        t.apply(c.clone(), running.clone(), 5, 100.0);
        assert!(t.observe(&snap, 6, 105.0).is_empty());
        assert_eq!(t.observe(&snap, 7, 110.5), back, "never came up");
        // A monitor of the configuration unplugged mid-trial.
        let mut gone = gallery_shows(&snap, &c);
        gone.outputs.retain(|o| o.name != D);
        let mut t = DisplayTrials::default();
        t.apply(c.clone(), running.clone(), 5, 100.0);
        assert_eq!(t.observe(&gone, 6, 101.0), back);
        assert!(t.note.as_ref().is_some_and(|n| n.text.contains("DP-4 was disconnected")));
    }
}
