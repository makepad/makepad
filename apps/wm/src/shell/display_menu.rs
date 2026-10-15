//! The Display menu's model: each screen's resolutions and the refresh
//! rates of a resolution, the screen's title, the try → confirm /
//! auto-revert state machine every resolution or refresh change goes
//! through, and the Extended / Mirror switch, which applies at once (it
//! reinitialises no output) and is saved as soon as the renderer takes it.
//!
//! A change is sent at once and shown as "Keep this mode? Reverting in
//! 15 s". Keep saves it; Revert, or the 15 s running out, sends the
//! previous one again. A change the renderer refuses, or that comes up
//! failed (or showing another mode), is reported next to the controls of
//! its screen and the previous one is sent again; the controls stay usable,
//! so another mode can be tried at once. Only kept changes are saved
//! (`TrialOut::Save`); the caller owns the saving and the sending.
//!
//! Pure: no `Cx`, no files. Linux only, like `display_layout`.
#![cfg(target_os = "linux")]

use makepad_widgets::makepad_platform::linux_display::{DisplayArrangement, DisplayPacing, LinuxDisplayOutput, LinuxDisplaySnapshot};

use super::display_layout::{connector_suffix, PEER_PENDING_STATUS};

/// How long a change waits for Keep before it is reverted.
pub const CONFIRM_SECONDS: f64 = 15.0;
/// How long a change may take to come up before it counts as failed.
pub const APPLY_TIMEOUT: f64 = 10.0;
/// The message slot of the arrangement (Extended / Mirror): a refused
/// switch is reported there. A screen's slot is its connector name.
pub const ARRANGEMENT_SLOT: &str = "";

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

// --------------------------------------------------------------- screens

/// `Built-in · eDP-1` for a laptop panel; `HDMI · 3840×2160`,
/// `DisplayPort · 2560×1440` for a monitor (its native resolution), or the
/// connector when nothing better is known.
pub fn screen_title(output: &LinuxDisplayOutput) -> String {
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

/// The laptop's own panel.
pub fn is_built_in(output: &LinuxDisplayOutput) -> bool {
    let connector = connector_suffix(&output.name);
    let kind = connector.rsplit_once('-').map_or(connector.as_str(), |(kind, _)| kind);
    matches!(kind, "eDP" | "LVDS" | "DSI")
}

/// `Built-in`, `HDMI`, `DisplayPort`, `DVI`, `VGA`; `None` for anything else.
pub fn screen_kind(output: &LinuxDisplayOutput) -> Option<&'static str> {
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

/// `HDMI · 60 Hz`: a screen by its kind and the rate it shows.
pub fn screen_rate_label(output: &LinuxDisplayOutput) -> String {
    let kind = screen_kind(output).map(str::to_string).unwrap_or_else(|| connector_suffix(&output.name));
    format!("{kind} \u{b7} {}", rate_label(output.refresh_hz))
}

/// The screen Auto paces a mirror by: the slowest external screen that
/// shows (the projector or audience screen never judders), else the
/// built-in panel. `None` in Extended: every screen runs at its own rate.
pub fn auto_pacer(snapshot: &LinuxDisplaySnapshot) -> Option<&LinuxDisplayOutput> {
    if snapshot.arrangement == DisplayArrangement::Extended {
        return None;
    }
    let shown = || snapshot.outputs.iter().filter(|o| o.active && o.refresh_hz > 0.0);
    shown()
        .filter(|o| !is_built_in(o))
        .min_by(|a, b| a.refresh_hz.total_cmp(&b.refresh_hz))
        .or_else(|| shown().find(|o| is_built_in(o)))
}

/// What the Pace by picker reads for `pacing`.
pub fn pacing_label(pacing: &DisplayPacing, snapshot: &LinuxDisplaySnapshot) -> String {
    match pacing {
        DisplayPacing::Auto => {
            if snapshot.arrangement == DisplayArrangement::Extended {
                return "Auto (each screen at its own rate)".into();
            }
            let by = snapshot
                .paced_by
                .as_ref()
                .and_then(|name| snapshot.outputs.iter().find(|o| o.name == *name))
                .or_else(|| auto_pacer(snapshot));
            match by {
                Some(output) => format!("Auto (paced by {})", screen_rate_label(output)),
                None => "Auto".into(),
            }
        }
        DisplayPacing::Slowest => "Slowest screen (saves GPU)".into(),
        DisplayPacing::Fastest => "Fastest screen (smoothest)".into(),
        DisplayPacing::Screen(name) => match snapshot.outputs.iter().find(|o| o.name == *name) {
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

// ---------------------------------------------------------------- trials

/// What Extended / Mirror reads as in a message.
pub fn arrangement_label(arrangement: &DisplayArrangement, snapshot: &LinuxDisplaySnapshot) -> String {
    match arrangement {
        DisplayArrangement::Extended => "Extended desktop".into(),
        DisplayArrangement::Mirror { optimise_for } => {
            let title = snapshot.outputs.iter().find(|o| o.name == *optimise_for).map(screen_title);
            format!("Mirror for {}", title.unwrap_or_else(|| connector_suffix(optimise_for)))
        }
    }
}

/// One change the menu can try: a screen's resolution and refresh.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// Screen `name` in `mode` (the renderer's syntax), `None` automatic.
    Mode { name: String, mode: Option<String> },
}

impl Change {
    /// The message slot: the screen's name.
    pub fn slot(&self) -> &str {
        match self {
            Change::Mode { name, .. } => name,
        }
    }

    /// What it reads as in a message.
    pub fn label(&self, snapshot: &LinuxDisplaySnapshot) -> String {
        match self {
            Change::Mode { name, mode } => {
                let output = snapshot.outputs.iter().find(|o| o.name == *name);
                match output.and_then(|o| resolve_mode(&o.modes, native_resolution(o), mode.as_deref())) {
                    Some((w, h, hz)) => mode_label(w, h, hz),
                    None => mode.clone().unwrap_or_else(|| "automatic".into()),
                }
            }
        }
    }

    /// Whether the renderer shows it.
    fn shown_by(&self, snapshot: &LinuxDisplaySnapshot) -> bool {
        match self {
            Change::Mode { name, mode } => {
                snapshot.outputs.iter().any(|o| o.name == *name && mode_matches(o, mode.as_deref()))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Sent at `since` with the display generation it was sent at; judged
    /// on later generations only, so an older failure is not taken for it.
    Applying { since: f64, generation: u64 },
    /// Shown; reverted at `deadline` unless kept.
    Confirming { deadline: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Trial {
    pub change: Change,
    /// The last kept change of the same slot: what Revert sends.
    pub previous: Change,
    /// What ran when this was sent: `previous`, or an earlier trial of the
    /// same slot it replaced (still unconfirmed).
    pub replaced: Change,
    pub phase: Phase,
}

/// What the caller does next.
#[derive(Clone, Debug, PartialEq)]
pub enum TrialOut {
    /// Ask the renderer for it.
    Send(Change),
    /// Save it: kept.
    Save(Change),
}

/// A line under a slot's controls.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub slot: String,
    pub text: String,
    pub error: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayTrials {
    pub trial: Option<Trial>,
    /// A failed or reverted change being undone: its slot shows `previous`
    /// until the renderer does (or the apply timeout passes).
    pub restoring: Option<(Change, f64)>,
    pub notes: Vec<Note>,
    /// An Extended / Mirror switch sent and not yet in the renderer's
    /// snapshot: the switch shows it meanwhile.
    pub arrangement_pending: Option<DisplayArrangement>,
}

impl DisplayTrials {
    /// Try `change`; `running` is what its slot runs (and keeps) now.
    /// Another slot's unconfirmed trial is reverted first: one change at a
    /// time. Picking the kept value again ends the trial with a revert.
    pub fn begin(&mut self, change: Change, running: Change, generation: u64, now: f64) -> Vec<TrialOut> {
        let mut out = Vec::new();
        self.clear_note(change.slot());
        let (previous, replaced) = match self.trial.take() {
            Some(trial) if trial.change.slot() == change.slot() => (trial.previous, trial.change),
            Some(trial) => {
                out.push(TrialOut::Send(trial.previous));
                (running.clone(), running)
            }
            None => (running.clone(), running),
        };
        if change == previous {
            if replaced != previous {
                out.push(TrialOut::Send(previous));
            }
            return out;
        }
        self.restoring = None;
        out.push(TrialOut::Send(change.clone()));
        self.trial = Some(Trial { change, previous, replaced, phase: Phase::Applying { since: now, generation } });
        out
    }

    /// The renderer (or the local check) refused the change being tried:
    /// nothing of it ran. What it replaced is undone when that was not the
    /// kept value.
    pub fn refused(&mut self, change: &Change, message: &str, snapshot: &LinuxDisplaySnapshot, now: f64) -> Vec<TrialOut> {
        let Some(trial) = self.trial.take_if(|t| t.change == *change) else { return Vec::new() };
        self.note(trial.change.slot(), format!("Couldn't set {}: {message}", trial.change.label(snapshot)), true);
        if trial.replaced != trial.previous {
            self.restoring = Some((trial.previous.clone(), now));
            vec![TrialOut::Send(trial.previous)]
        } else {
            Vec::new()
        }
    }

    /// One look at the renderer's snapshot (`generation` is its display
    /// generation) at `now`.
    pub fn observe(&mut self, snapshot: &LinuxDisplaySnapshot, generation: u64, now: f64) -> Vec<TrialOut> {
        if self.arrangement_pending.as_ref() == Some(&snapshot.arrangement) {
            self.arrangement_pending = None;
        }
        if let Some((change, since)) = &self.restoring {
            if change.shown_by(snapshot) || now - since > APPLY_TIMEOUT {
                self.restoring = None;
            }
        }
        let Some(trial) = self.trial.as_mut() else { return Vec::new() };
        let failure = match &trial.change {
            Change::Mode { name, .. } => match snapshot.outputs.iter().find(|o| o.name == *name) {
                None => Some("the screen was disconnected".to_string()),
                Some(output) => output_failure(output).map(str::to_string),
            },
        };
        match trial.phase {
            Phase::Applying { since, generation: sent } => {
                if generation > sent && trial.change.shown_by(snapshot) {
                    trial.phase = Phase::Confirming { deadline: now + CONFIRM_SECONDS };
                    return Vec::new();
                }
                let failure = if generation > sent { failure.or_else(|| other_mode(&trial.change, snapshot)) } else { None };
                match failure {
                    Some(reason) => self.fail(snapshot, &reason, now),
                    None if now - since > APPLY_TIMEOUT => {
                        self.fail(snapshot, &format!("it did not come up within {} s", APPLY_TIMEOUT as u32), now)
                    }
                    None => Vec::new(),
                }
            }
            Phase::Confirming { deadline } => match failure {
                Some(reason) => self.fail(snapshot, &reason, now),
                None if now >= deadline => {
                    let trial = self.trial.take().unwrap();
                    self.note(
                        trial.change.slot(),
                        format!("Not kept: back to {}", trial.previous.label(snapshot)),
                        false,
                    );
                    self.restoring = Some((trial.previous.clone(), now));
                    vec![TrialOut::Send(trial.previous)]
                }
                None => Vec::new(),
            },
        }
    }

    /// "Keep": saves a change that is up.
    pub fn keep(&mut self) -> Vec<TrialOut> {
        match self.trial.take_if(|t| matches!(t.phase, Phase::Confirming { .. })) {
            Some(trial) => vec![TrialOut::Save(trial.change)],
            None => Vec::new(),
        }
    }

    /// "Revert": back to the kept value now.
    pub fn revert(&mut self, now: f64) -> Vec<TrialOut> {
        match self.trial.take() {
            Some(trial) => {
                self.restoring = Some((trial.previous.clone(), now));
                vec![TrialOut::Send(trial.previous)]
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

    /// The trial of `slot`, if any.
    pub fn trial_for(&self, slot: &str) -> Option<&Trial> {
        self.trial.as_ref().filter(|t| t.change.slot() == slot)
    }

    /// The mode the controls of screen `name` show: the one being tried,
    /// the one being restored, else `running` (its renderer override).
    pub fn selected_mode<'a>(&'a self, name: &str, running: Option<&'a str>) -> Option<&'a str> {
        let pick = |change: &'a Change| match change {
            Change::Mode { name: n, mode } if n == name => Some(mode.as_deref()),
            _ => None,
        };
        self.trial
            .as_ref()
            .and_then(|t| pick(&t.change))
            .or_else(|| self.restoring.as_ref().and_then(|(c, _)| pick(c)))
            .unwrap_or(running)
    }

    /// The arrangement the switch shows: one sent and on its way, else
    /// `running` (the renderer's).
    pub fn selected_arrangement<'a>(&'a self, running: &'a DisplayArrangement) -> &'a DisplayArrangement {
        self.arrangement_pending.as_ref().unwrap_or(running)
    }

    /// Switch Extended / Mirror (or the screen a mirror is sized for): no
    /// confirm, it reinitialises no output. True when `wanted` is to be
    /// sent; nothing to send when the switch already shows it.
    pub fn switch_arrangement(&mut self, wanted: DisplayArrangement, running: &DisplayArrangement) -> bool {
        self.clear_note(ARRANGEMENT_SLOT);
        if *self.selected_arrangement(running) == wanted {
            return false;
        }
        self.arrangement_pending = Some(wanted);
        true
    }

    /// The renderer refused a switch: the switch goes back to what runs and
    /// the reason is noted under it.
    pub fn arrangement_refused(&mut self, wanted: &DisplayArrangement, message: &str, snapshot: &LinuxDisplaySnapshot) {
        if self.arrangement_pending.as_ref() == Some(wanted) {
            self.arrangement_pending = None;
        }
        self.note(ARRANGEMENT_SLOT, format!("Couldn't switch to {}: {message}", arrangement_label(wanted, snapshot)), true);
    }

    pub fn note_for(&self, slot: &str) -> Option<&Note> {
        self.notes.iter().find(|n| n.slot == slot)
    }

    /// A screen's messages go with it when it is unplugged.
    pub fn forget_missing(&mut self, snapshot: &LinuxDisplaySnapshot) {
        self.notes.retain(|n| n.slot == ARRANGEMENT_SLOT || snapshot.outputs.iter().any(|o| o.name == n.slot));
    }

    fn fail(&mut self, snapshot: &LinuxDisplaySnapshot, reason: &str, now: f64) -> Vec<TrialOut> {
        let trial = self.trial.take().unwrap();
        self.note(
            trial.change.slot(),
            format!("Couldn't set {}: {reason}. Back to {}", trial.change.label(snapshot), trial.previous.label(snapshot)),
            true,
        );
        self.restoring = Some((trial.previous.clone(), now));
        vec![TrialOut::Send(trial.previous)]
    }

    fn note(&mut self, slot: &str, text: String, error: bool) {
        self.clear_note(slot);
        self.notes.push(Note { slot: slot.to_string(), text, error });
    }

    fn clear_note(&mut self, slot: &str) {
        self.notes.retain(|n| n.slot != slot);
    }
}

/// The gallery's Display menu: a laptop panel on one GPU and a 4K
/// monitor on another (node 149's pair), a 144 Hz mode on the monitor
/// waiting to be kept, and a failed try noted under the panel.
pub fn gallery_fixture() -> (LinuxDisplaySnapshot, DisplayTrials) {
    let hdmi_modes = vec![
        (3840, 2160, 239.995), (3840, 2160, 144.0), (3840, 2160, 120.0), (3840, 2160, 60.0), (3840, 2160, 50.0),
        (3840, 2160, 30.0), (3840, 2160, 24.0), (2560, 1440, 239.97), (2560, 1440, 144.0), (2560, 1440, 120.0),
        (2560, 1440, 60.0), (1920, 1080, 240.0), (1920, 1080, 144.0), (1920, 1080, 120.0), (1920, 1080, 60.0),
        (1920, 1080, 50.0), (1680, 1050, 60.0), (1280, 1024, 60.0), (1280, 720, 60.0), (1280, 720, 50.0),
        (1024, 768, 60.0), (800, 600, 60.0), (720, 480, 60.0), (640, 480, 60.0),
    ];
    let edp = LinuxDisplayOutput {
        name: "card0-eDP-1".into(),
        width: 2560,
        height: 1600,
        refresh_hz: 240.0,
        primary: true,
        card: "card0".into(),
        desktop_position: Some((0, 0)),
        active: true,
        status: "active".into(),
        modes: vec![(2560, 1600, 240.0), (2560, 1600, 60.0)],
        ..Default::default()
    };
    let hdmi = LinuxDisplayOutput {
        name: "card1-HDMI-A-2".into(),
        width: 3840,
        height: 2160,
        refresh_hz: 144.0,
        card: "card1".into(),
        desktop_position: Some((2560, 0)),
        active: true,
        status: "active".into(),
        modes: hdmi_modes,
        mode_override: Some("3840x2160@144".into()),
        ..Default::default()
    };
    let snapshot = LinuxDisplaySnapshot { direct: true, outputs: vec![edp, hdmi], ..Default::default() };
    let tried = Change::Mode { name: "card1-HDMI-A-2".into(), mode: Some("3840x2160@144".into()) };
    let trials = DisplayTrials {
        trial: Some(Trial {
            change: tried,
            previous: Change::Mode { name: "card1-HDMI-A-2".into(), mode: None },
            replaced: Change::Mode { name: "card1-HDMI-A-2".into(), mode: None },
            phase: Phase::Confirming { deadline: CONFIRM_SECONDS },
        }),
        restoring: None,
        arrangement_pending: None,
        notes: vec![Note {
            slot: "card0-eDP-1".into(),
            text: "Couldn't set 2560\u{d7}1600 \u{b7} 60 Hz: failed: VK_ERROR_INITIALIZATION_FAILED. Back to 2560\u{d7}1600 \u{b7} 240 Hz".into(),
            error: true,
        }],
    };
    (snapshot, trials)
}

/// A screen that took the request but runs another mode: the renderer's
/// fallback when the requested one cannot be shown.
fn other_mode(change: &Change, snapshot: &LinuxDisplaySnapshot) -> Option<String> {
    let Change::Mode { name, mode } = change;
    let output = snapshot.outputs.iter().find(|o| o.name == *name)?;
    (output.active && output.mode_override == *mode && output.width > 0)
        .then(|| format!("the screen shows {} instead", mode_label(output.width, output.height, output.refresh_hz)))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            desktop_position: Some((2560, 0)),
            modes: HDMI.to_vec(),
            mode_override: mode.map(str::to_string),
            ..Default::default()
        }
    }

    fn snap(outputs: Vec<LinuxDisplayOutput>) -> LinuxDisplaySnapshot {
        LinuxDisplaySnapshot { direct: true, outputs, ..Default::default() }
    }

    fn mode(m: Option<&str>) -> Change {
        Change::Mode { name: "card1-HDMI-A-2".into(), mode: m.map(str::to_string) }
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
    fn pacing_auto_follows_the_slowest_external_screen_in_mirror_only() {
        let mut edp = hdmi(None, 2560, 1600, 240.0);
        edp.name = "card0-eDP-1".into();
        let tv = hdmi(None, 3840, 2160, 60.0);
        let mut dp = hdmi(None, 2560, 1440, 144.0);
        dp.name = "card1-DP-1".into();
        let mut s = snap(vec![edp.clone(), tv, dp]);
        assert!(auto_pacer(&s).is_none());
        assert_eq!(pacing_label(&DisplayPacing::Auto, &s), "Auto (each screen at its own rate)");
        s.arrangement = DisplayArrangement::Mirror { optimise_for: "card0-eDP-1".into() };
        assert_eq!(auto_pacer(&s).unwrap().name, "card1-HDMI-A-2");
        assert_eq!(pacing_label(&DisplayPacing::Auto, &s), "Auto (paced by HDMI \u{b7} 60 Hz)");
        // The renderer's own word wins.
        s.paced_by = Some("card1-DP-1".into());
        assert_eq!(pacing_label(&DisplayPacing::Auto, &s), "Auto (paced by DisplayPort \u{b7} 144 Hz)");
        // No external screen: the built-in panel.
        let only = LinuxDisplaySnapshot { arrangement: s.arrangement.clone(), ..snap(vec![edp]) };
        assert_eq!(auto_pacer(&only).unwrap().name, "card0-eDP-1");
        assert_eq!(pacing_label(&DisplayPacing::Screen("card1-HDMI-A-2".into()), &s), "HDMI \u{b7} 60 Hz");
        assert_eq!(pacing_label(&DisplayPacing::Fastest, &s), "Fastest screen (smoothest)");
        assert_eq!(pacing_choices(&s).len(), 6);
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

    #[test]
    fn confirmed_mode_is_saved_only_on_keep() {
        let mut t = DisplayTrials::default();
        let out = t.begin(mode(Some("1920x1080@60")), mode(None), 5, 100.0);
        assert_eq!(out, [TrialOut::Send(mode(Some("1920x1080@60")))]);
        // Same generation: not judged yet, even though nothing matches.
        assert!(t.observe(&snap(vec![hdmi(None, 3840, 2160, 239.995)]), 5, 100.5).is_empty());
        assert!(matches!(t.trial.as_ref().unwrap().phase, Phase::Applying { .. }));
        // Up in the new mode: confirming, 15 s.
        let up = snap(vec![hdmi(Some("1920x1080@60"), 1920, 1080, 60.0)]);
        assert!(t.observe(&up, 6, 101.0).is_empty());
        assert_eq!(t.seconds_left(101.0), Some(15));
        assert_eq!(t.seconds_left(110.2), Some(6));
        assert_eq!(t.selected_mode("card1-HDMI-A-2", None), Some("1920x1080@60"));
        assert_eq!(t.keep(), [TrialOut::Save(mode(Some("1920x1080@60")))]);
        assert!(t.trial.is_none());
        assert!(t.keep().is_empty());
    }

    #[test]
    fn unconfirmed_mode_reverts_after_15_s_and_is_not_saved() {
        let mut t = DisplayTrials::default();
        t.begin(mode(Some("2560x1440@120")), mode(None), 1, 0.0);
        let up = snap(vec![hdmi(Some("2560x1440@120"), 2560, 1440, 120.0)]);
        t.observe(&up, 2, 1.0);
        assert!(t.observe(&up, 2, 15.9).is_empty());
        let out = t.observe(&up, 2, 16.0);
        assert_eq!(out, [TrialOut::Send(mode(None))]);
        assert!(t.trial.is_none());
        let note = t.note_for("card1-HDMI-A-2").unwrap();
        assert!(!note.error && note.text.starts_with("Not kept"), "{note:?}");
        // Shows the kept value while the renderer goes back.
        assert_eq!(t.selected_mode("card1-HDMI-A-2", Some("2560x1440@120")), None);
        t.observe(&snap(vec![hdmi(None, 3840, 2160, 239.995)]), 3, 17.0);
        assert!(t.restoring.is_none());
    }

    #[test]
    fn revert_sends_the_kept_mode() {
        let mut t = DisplayTrials::default();
        t.begin(mode(Some("3840x2160@60")), mode(Some("3840x2160@144")), 1, 0.0);
        assert_eq!(t.revert(2.0), [TrialOut::Send(mode(Some("3840x2160@144")))]);
        assert!(t.trial.is_none());
    }

    #[test]
    fn a_failed_apply_restores_the_previous_mode_and_stays_retryable() {
        let mut t = DisplayTrials::default();
        let previous = mode(Some("3840x2160@144"));
        t.begin(mode(Some("3840x2160@60")), previous.clone(), 1, 0.0);
        let mut failed = hdmi(Some("3840x2160@60"), 0, 0, 0.0);
        failed.active = false;
        failed.status = "failed: VK_ERROR_INITIALIZATION_FAILED".into();
        let out = t.observe(&snap(vec![failed.clone()]), 2, 1.0);
        assert_eq!(out, [TrialOut::Send(previous.clone())]);
        assert!(t.trial.is_none());
        // The model shows the previous mode, with the error next to it.
        assert_eq!(t.selected_mode("card1-HDMI-A-2", Some("3840x2160@60")), Some("3840x2160@144"));
        let note = t.note_for("card1-HDMI-A-2").unwrap();
        assert!(note.error && note.text.contains("VK_ERROR_INITIALIZATION_FAILED"), "{note:?}");
        // Retry on the still-failed screen: the old failure is not taken for
        // the new try (same generation), the note goes, the kept value stays.
        let out = t.begin(mode(Some("1920x1080@60")), previous.clone(), 2, 2.0);
        assert_eq!(out, [TrialOut::Send(mode(Some("1920x1080@60")))]);
        assert!(t.note_for("card1-HDMI-A-2").is_none());
        assert!(t.observe(&snap(vec![failed]), 2, 2.5).is_empty());
        let up = snap(vec![hdmi(Some("1920x1080@60"), 1920, 1080, 60.0)]);
        t.observe(&up, 3, 3.0);
        assert_eq!(t.keep(), [TrialOut::Save(mode(Some("1920x1080@60")))]);
    }

    #[test]
    fn a_fallback_mode_counts_as_failed() {
        let mut t = DisplayTrials::default();
        t.begin(mode(Some("3840x2160@30")), mode(None), 1, 0.0);
        // The renderer fell back to the last working mode.
        let mut back = hdmi(Some("3840x2160@30"), 3840, 2160, 239.995);
        back.status = "requested mode failed, showing 3840x2160@240".into();
        assert_eq!(t.observe(&snap(vec![back.clone()]), 2, 1.0), [TrialOut::Send(mode(None))]);
        // Same without the status line: running another mode than asked.
        t.begin(mode(Some("3840x2160@30")), mode(None), 2, 2.0);
        back.status = "active".into();
        assert_eq!(t.observe(&snap(vec![back]), 3, 3.0), [TrialOut::Send(mode(None))]);
        assert!(t.note_for("card1-HDMI-A-2").unwrap().text.contains("instead"));
    }

    #[test]
    fn a_refused_request_leaves_the_kept_mode_and_undoes_an_unconfirmed_one() {
        let s = snap(vec![hdmi(None, 3840, 2160, 239.995)]);
        let mut t = DisplayTrials::default();
        t.begin(mode(Some("1920x1080@60")), mode(None), 1, 0.0);
        // Refused before anything ran: nothing to undo.
        assert!(t.refused(&mode(Some("1920x1080@60")), "busy", &s, 0.1).is_empty());
        assert!(t.note_for("card1-HDMI-A-2").unwrap().error);
        // An unconfirmed mode replaced by another that is refused: back to the kept one.
        t.begin(mode(Some("1920x1080@60")), mode(None), 1, 1.0);
        t.observe(&snap(vec![hdmi(Some("1920x1080@60"), 1920, 1080, 60.0)]), 2, 1.5);
        t.begin(mode(Some("2560x1440@120")), mode(Some("1920x1080@60")), 2, 2.0);
        assert_eq!(t.refused(&mode(Some("2560x1440@120")), "busy", &s, 2.1), [TrialOut::Send(mode(None))]);
    }

    #[test]
    fn a_mode_that_never_comes_up_times_out() {
        let mut t = DisplayTrials::default();
        t.begin(mode(Some("1920x1080@60")), mode(None), 1, 0.0);
        let mut joining = hdmi(Some("1920x1080@60"), 0, 0, 0.0);
        joining.active = false;
        joining.status = "retired: reacquiring after mode change".into();
        assert!(t.observe(&snap(vec![joining.clone()]), 2, 5.0).is_empty());
        assert_eq!(t.observe(&snap(vec![joining]), 2, 10.5), [TrialOut::Send(mode(None))]);
    }

    #[test]
    fn picking_the_kept_mode_again_reverts() {
        let mut t = DisplayTrials::default();
        t.begin(mode(Some("1920x1080@60")), mode(None), 1, 0.0);
        assert_eq!(t.begin(mode(None), mode(Some("1920x1080@60")), 1, 1.0), [TrialOut::Send(mode(None))]);
        assert!(t.trial.is_none());
        // Nothing tried: picking what runs sends nothing.
        assert!(t.begin(mode(None), mode(None), 1, 2.0).is_empty());
    }

    fn mirror(name: &str) -> DisplayArrangement {
        DisplayArrangement::Mirror { optimise_for: name.into() }
    }

    #[test]
    fn mirror_and_extended_switch_at_once_without_a_trial() {
        let mut s = snap(vec![hdmi(None, 3840, 2160, 239.995)]);
        let mut t = DisplayTrials::default();
        assert!(t.switch_arrangement(mirror("card0-eDP-1"), &s.arrangement));
        // No confirm: nothing to keep, nothing that reverts.
        assert!(t.trial.is_none() && t.seconds_left(0.0).is_none());
        assert_eq!(t.selected_arrangement(&s.arrangement), &mirror("card0-eDP-1"));
        // Asking again for what is on its way sends nothing.
        assert!(!t.switch_arrangement(mirror("card0-eDP-1"), &s.arrangement));
        s.arrangement = mirror("card0-eDP-1");
        assert!(t.observe(&s, 2, 1.0).is_empty());
        assert!(t.arrangement_pending.is_none());
        // Long after: still mirrored, nothing sent back.
        assert!(t.observe(&s, 2, 60.0).is_empty());
        assert!(t.switch_arrangement(DisplayArrangement::Extended, &s.arrangement));
        s.arrangement = DisplayArrangement::Extended;
        assert!(t.observe(&s, 3, 61.0).is_empty());
        assert_eq!(t.selected_arrangement(&s.arrangement), &DisplayArrangement::Extended);
    }

    #[test]
    fn optimise_for_another_screen_switches_and_a_refusal_keeps_what_runs() {
        let mut s = snap(vec![hdmi(None, 3840, 2160, 239.995)]);
        s.arrangement = mirror("card0-eDP-1");
        let mut t = DisplayTrials::default();
        assert!(t.switch_arrangement(mirror("card1-HDMI-A-2"), &s.arrangement));
        assert_eq!(t.selected_arrangement(&s.arrangement), &mirror("card1-HDMI-A-2"));
        t.arrangement_refused(&mirror("card1-HDMI-A-2"), "card1-HDMI-A-2 is not connected", &s);
        assert_eq!(t.selected_arrangement(&s.arrangement), &mirror("card0-eDP-1"));
        let note = t.note_for(ARRANGEMENT_SLOT).unwrap();
        assert!(note.error && note.text.contains("Mirror for HDMI") && note.text.contains("not connected"), "{note:?}");
        // The next switch clears the note.
        assert!(t.switch_arrangement(DisplayArrangement::Extended, &s.arrangement));
        assert!(t.note_for(ARRANGEMENT_SLOT).is_none());
    }

    #[test]
    fn a_mode_change_in_mirror_is_tried_kept_and_reverted_like_in_extended() {
        // Mirror sized for the panel; the HDMI has no rectangle of its own.
        let mut edp = hdmi(None, 2560, 1600, 240.0);
        edp.name = "card0-eDP-1".into();
        edp.modes = vec![(2560, 1600, 240.0), (2560, 1600, 60.0), (1920, 1200, 60.0)];
        let mut tv = hdmi(None, 3840, 2160, 239.995);
        tv.desktop_position = None;
        let mut s = snap(vec![edp.clone(), tv.clone()]);
        s.arrangement = mirror("card0-eDP-1");
        let mut t = DisplayTrials::default();
        // The audience screen's own refresh: tried, shown, kept.
        t.begin(mode(Some("3840x2160@60")), mode(None), 1, 0.0);
        let mut tv60 = tv.clone();
        tv60.mode_override = Some("3840x2160@60".into());
        tv60.refresh_hz = 60.0;
        t.observe(&LinuxDisplaySnapshot { outputs: vec![edp.clone(), tv60], ..s.clone() }, 2, 1.0);
        assert_eq!(t.seconds_left(1.0), Some(15));
        assert_eq!(t.keep(), [TrialOut::Save(mode(Some("3840x2160@60")))]);
        // The optimised screen's resolution (the mirror's size): tried, not
        // kept, reverted after 15 s.
        let panel = |m: Option<&str>| Change::Mode { name: "card0-eDP-1".into(), mode: m.map(str::to_string) };
        t.begin(panel(Some("1920x1200@60")), panel(None), 2, 2.0);
        let mut small = edp.clone();
        small.mode_override = Some("1920x1200@60".into());
        small.width = 1920;
        small.height = 1200;
        small.refresh_hz = 60.0;
        let up = LinuxDisplaySnapshot { outputs: vec![small, tv], ..s.clone() };
        t.observe(&up, 3, 3.0);
        assert!(t.seconds_left(3.0).is_some());
        assert_eq!(t.observe(&up, 3, 18.0), [TrialOut::Send(panel(None))]);
        // The switch stayed on Mirror throughout.
        assert_eq!(t.selected_arrangement(&s.arrangement), &mirror("card0-eDP-1"));
    }

    #[test]
    fn an_arrangement_switch_leaves_a_mode_trial_alone() {
        let s = snap(vec![hdmi(None, 3840, 2160, 239.995)]);
        let mut t = DisplayTrials::default();
        t.begin(mode(Some("1920x1080@60")), mode(None), 1, 0.0);
        assert!(t.switch_arrangement(mirror("card1-HDMI-A-2"), &s.arrangement));
        assert_eq!(t.trial_for("card1-HDMI-A-2").unwrap().change, mode(Some("1920x1080@60")));
    }
}
