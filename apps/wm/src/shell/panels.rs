//! `shell/plugins/panels/` — the bar flyouts.
//!
//! Shared chrome (`Ui/PopupCard.qml` / `Ui/KeyboardPanel.qml`): a card on
//! `[popups] background` behind a 2px `[popups] border`, `popupPadding` 14
//! all round, hard corners, no shadow and no notch, anchored under the bar
//! module that owns it, centered on it, `gapsOut` (5) off the bar edge and
//! clamped into the screen by the same margin. It fades in over 140ms
//! (ease-out-cubic) and closes on a click outside.
//!
//! The panels themselves, from their `Panel.qml`:
//! * **clock** — `space(560)` wide, centered on the bar: a 48px glyph and a
//!   52px "MMMM d" hero, a year-progress bar, then the month grid (a 32-wide
//!   week column, a 14 gutter, seven 52-wide day columns, a 16-high header
//!   row and six 34-high week rows) under a centered "MONTH YYYY" nav row.
//! * **audio** — `space(380)`: hero, separator, an OUTPUT section with the
//!   percentage on the right and a `PanelSlider`, then the device rows.
//! * **power** — `space(380)`: hero with the big percentage, a `space(8)`
//!   progress bar, the stat pairs, then the POWER PROFILE buttons.
//! * **monitor** — `space(380)`: hero, the TEXT SIZE slider,
//!   the SCALE pills and the display rows.
//!
//! What a Mac can answer cheaply is answered for real (volume, battery,
//! the calendar); what it cannot is drawn in the panel's own "not
//! available" reading rather than faked.

use makepad_widgets::*;

use super::bar::BarModule;
use crate::hub_glow::GlowTracker;
use std::time::Instant;
use super::ui::{contains, cut_top, inset, rect, DrawShellFill, Ico, ShellDraw};
#[cfg(target_os = "linux")]
use super::wifi_linux::{
    HotspotAsk, KeyOutcome, RadioState, Security, StationState, WifiCommand, WifiHit, WifiPhase, WifiUi,
};
use super::{alpha, darker, CtrlState, ShellTokens};
#[cfg(target_os = "linux")]
#[path = "panels_linux.rs"]
mod linux;
#[cfg(target_os = "linux")]
use {std::sync::Arc, super::system_linux::SystemSnapshot};
#[cfg(target_os = "linux")]
use makepad_widgets::makepad_platform::linux_display::LinuxDisplaySnapshot;
#[cfg(target_os = "linux")]
use {linux::{Chooser, DpiDrag}, super::display_menu::{Change, DisplayTrials}, makepad_widgets::makepad_platform::linux_display::DisplayPacing};

// ======================================================================
// Civil dates — the calendar grid needs real date maths, no chrono.
// ======================================================================

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64;
    let mp = ((m + 9) % 12) as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// The civil date of a day count.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 0 = Monday .. 6 = Sunday (the grid starts on Monday by default).
pub fn weekday(y: i64, m: u32, d: u32) -> u32 {
    let days = days_from_civil(y, m, d);
    (((days % 7) + 10) % 7) as u32
}

pub fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

/// ISO-8601 week number.
pub fn iso_week(y: i64, m: u32, d: u32) -> u32 {
    let day = days_from_civil(y, m, d);
    let dow = weekday(y, m, d) as i64;
    let thursday = day - dow + 3;
    let (ty, _, _) = civil_from_days(thursday);
    let jan1 = days_from_civil(ty, 1, 1);
    ((thursday - jan1) / 7 + 1) as u32
}

pub const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
pub const WEEKDAY_NAMES: [&str; 7] = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];

/// Today's local date (year, month 1–12, day 1–31), read in-process.
pub fn today() -> (i64, u32, u32) {
    match super::bar::local_now() {
        Some(now) => (now.year, now.month, now.day),
        None => (2026, 1, 1),
    }
}

// ======================================================================
// The panels
// ======================================================================

/// Which flyout is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelKind {
    Clock,
    Audio,
    Power,
    Monitor,
    Network,
    Bluetooth,
    Hub,
}

impl PanelKind {
    pub fn for_module(module: BarModule) -> Option<Self> {
        Some(match module {
            BarModule::Clock => PanelKind::Clock,
            BarModule::Audio => PanelKind::Audio,
            BarModule::Power => PanelKind::Power,
            BarModule::Monitor => PanelKind::Monitor,
            BarModule::Network => PanelKind::Network,
            BarModule::Bluetooth => PanelKind::Bluetooth,
            BarModule::Hub => PanelKind::Hub,
            _ => return None,
        })
    }

    pub fn module(self) -> BarModule {
        match self {
            PanelKind::Clock => BarModule::Clock,
            PanelKind::Audio => BarModule::Audio,
            PanelKind::Power => BarModule::Power,
            PanelKind::Monitor => BarModule::Monitor,
            PanelKind::Network => BarModule::Network,
            PanelKind::Bluetooth => BarModule::Bluetooth,
            PanelKind::Hub => BarModule::Hub,
        }
    }

    /// `contentWidth` — 560 for the calendar, 380 for the rest.
    pub fn content_width(self) -> f64 {
        match self {
            PanelKind::Clock => 560.0,
            _ => 380.0,
        }
    }
}

/// What the panels show. Sampled by the WM, fixed by the gallery.
#[derive(Clone, Debug)]
pub struct PanelData {
    pub volume: Option<u32>,
    pub muted: bool,
    pub input_volume: Option<u32>,
    pub outputs: Vec<(String, bool)>,
    pub battery: Option<super::bar::Battery>,
    pub battery_cycles: Option<u32>,
    pub power_source: String,
    pub displays: Vec<String>,
    pub text_size: f64,
    pub network: Option<String>,
    pub bluetooth: Option<bool>,
    /// The month the calendar is showing.
    pub view: (i64, u32),
    pub today: (i64, u32, u32),
    /// The AI hub, as last read (only while its panel is open).
    pub hub: crate::hub_control::HubSnapshot,
}

impl Default for PanelData {
    fn default() -> Self {
        let (y, m, d) = today();
        Self {
            volume: None,
            muted: false,
            input_volume: None,
            outputs: Vec::new(),
            battery: None,
            battery_cycles: None,
            power_source: String::new(),
            displays: Vec::new(),
            text_size: 12.0,
            network: None,
            bluetooth: None,
            view: (y, m),
            today: (y, m, d),
            hub: Default::default(),
        }
    }
}

impl PanelData {
    pub fn fixture() -> Self {
        let (y, m, d) = today();
        Self {
            volume: Some(62),
            muted: false,
            input_volume: Some(35),
            outputs: vec![
                ("MacBook Pro Speakers".into(), true),
                ("Studio Display".into(), false),
            ],
            battery: Some(super::bar::Battery {
                percent: 87,
                charging: true,
            }),
            battery_cycles: Some(214),
            power_source: "AC Power".into(),
            displays: vec!["Built-in Retina Display".into()],
            text_size: 12.0,
            network: Some("Ethernet".into()),
            bluetooth: Some(true),
            view: (y, m),
            today: (y, m, d),
            hub: Default::default(),
        }
    }

    /// `omarchy`'s mood names for the volume readout.
    pub fn volume_mood(&self) -> &'static str {
        if self.muted {
            return "Muted";
        }
        match self.volume.unwrap_or(0) {
            0 => "Silenced",
            v if v >= 100 => "Concert hall",
            v if v >= 85 => "Party mode",
            v if v >= 70 => "Cranked up",
            v if v >= 50 => "Steady groove",
            v if v >= 30 => "Easy listening",
            v if v >= 15 => "Murmur",
            _ => "Whisper",
        }
    }
}

/// `omarchy-display-text-size`'s curated stops.
pub const TEXT_SIZE_STOPS: [f64; 7] = [9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 20.0];

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ShellPanelBase = #(ShellPanel::register_widget(vm))
    mod.widgets.ShellPanel = set_type_default() do mod.widgets.ShellPanelBase {
        width: Fill
        height: Fill
        draw_bg +: {}
        d +: {}
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum ShellPanelAction {
    SetVolume(u32),
    ToggleMute,
    SetTextSize(f64),
    Close,
    /// A command for the AI hub.
    Hub(crate::hub_control::HubCommand),
    /// The Wi-Fi dropdown's intent (Linux). The WM turns it into a worker
    /// command; a `ConnectWithPassphrase` is completed from the panel's own
    /// field so the passphrase never rides in an action.
    #[cfg(target_os = "linux")]
    Wifi(WifiCommand),
    #[cfg(target_os = "linux")]
    System(crate::linux_controls::ControlAction),
    #[default]
    None,
}

/// A hit target inside the open panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    VolumeSlider,
    InputSlider,
    MuteToggle,
    TextSizeSlider,
    PrevMonth,
    NextMonth,
    Today,
    /// A model row's Load / Unload button.
    HubModel(usize),
    /// The KV precision switch: true is q8.
    HubKv(bool),
    /// The context size's step: true is the next size up.
    HubContext(bool),
    /// The WM's hub child: stop and start it again.
    HubRestart,
    /// Open or close the list of chat models.
    HubPickOpen,
    /// A chat model of that list (index into `hub_chat_models`).
    HubPick(usize),
    #[cfg(target_os = "linux")]
    Wifi(WifiHit),
    #[cfg(target_os = "linux")]
    DevicePicker(bool),
    #[cfg(target_os = "linux")]
    Device(bool, usize),
    #[cfg(target_os = "linux")]
    InputMute,
    /// The scale slider: live while dragged, saved on release.
    #[cfg(target_os = "linux")]
    DpiScaleSlider,
    /// The arrangement switch: `false` Extended, `true` Mirror.
    #[cfg(target_os = "linux")]
    Arrangement(bool),
    /// In Mirror, the screen (row) the desktop is sized for.
    #[cfg(target_os = "linux")]
    OptimiseFor(usize),
    /// A screen row's move button: the row (left to right) and the
    /// direction, -1 left or +1 right.
    #[cfg(target_os = "linux")]
    ScreenMove(usize, i32),
    /// A screen row's "Make main".
    #[cfg(target_os = "linux")]
    ScreenMain(usize),
    /// A screen row's resolution and refresh pickers.
    #[cfg(target_os = "linux")]
    ScreenResolution(usize),
    #[cfg(target_os = "linux")]
    ScreenRate(usize),
    /// A chip of the unfolded picker (`chooser_changes`).
    #[cfg(target_os = "linux")]
    ChooserItem(usize),
    /// The Pace by picker, and a chip of its unfolded list (`pace_targets`).
    #[cfg(target_os = "linux")]
    PacePicker,
    #[cfg(target_os = "linux")]
    PaceItem(usize),
    /// Keep or revert the change being tried.
    #[cfg(target_os = "linux")]
    TrialKeep,
    #[cfg(target_os = "linux")]
    TrialRevert,
    /// "Restart desktop now", then its confirm and cancel.
    #[cfg(target_os = "linux")]
    RestartDesktop,
    #[cfg(target_os = "linux")]
    RestartConfirm,
    #[cfg(target_os = "linux")]
    RestartCancel,
    /// The "Render on" row and its list: Auto (display GPU), then the GPUs.
    #[cfg(target_os = "linux")]
    GpuPicker,
    #[cfg(target_os = "linux")]
    Gpu(usize),
    /// Opens or closes the Display panel's Mouse & touchpad subview.
    #[cfg(target_os = "linux")]
    PointerSettings,
    /// Pointer speed slider: `false` mouse, `true` touchpad.
    #[cfg(target_os = "linux")]
    PointerSpeedSlider(bool),
}

/// A flyout card `w` wide and up to `height` tall under its bar module
/// `anchor` (the top of `screen`'s centre when it has none, `bar_h` tall),
/// centred on it, `margin` off the bar edge and clamped into `screen` by the
/// same margin. `screen` is the module's own screen on a multi-screen
/// desktop, so a module next to a seam keeps its card on its side.
pub(crate) fn panel_card_rect(anchor: Rect, screen: Rect, w: f64, height: f64, margin: f64, bar_h: f64) -> Rect {
    let anchor = if anchor.size.x > 0.0 {
        anchor
    } else {
        rect(screen.pos.x + screen.size.x * 0.5, screen.pos.y, 0.0, bar_h)
    };
    let x = (anchor.pos.x + anchor.size.x * 0.5 - w * 0.5)
        .max(screen.pos.x + margin)
        .min(screen.pos.x + screen.size.x - w - margin)
        .floor();
    let y = (anchor.pos.y + anchor.size.y + margin).floor();
    rect(x, y, w, height.min(screen.pos.y + screen.size.y - y - margin))
}

#[derive(Script, ScriptHook, Widget)]
pub struct ShellPanel {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawShellFill,
    #[live]
    d: ShellDraw,
    #[live]
    tokens: ShellTokens,
    #[rust]
    pub open: Option<PanelKind>,
    #[rust]
    pub data: PanelData,
    /// Where the owning bar module sits, so the card can center on it.
    #[rust]
    pub anchor: Rect,
    #[redraw]
    #[area]
    #[rust]
    area: Area,
    /// The whole overlay as last drawn (what a re-laid-out window is
    /// measured against, see `follow_screen`).
    #[rust]
    screen: Rect,
    /// The screen the flyout is clamped into instead of the whole overlay:
    /// its bar segment's screen on a multi-screen desktop (set by the WM
    /// before each draw). `None` uses the overlay, as on one screen.
    #[rust]
    pub target: Option<Rect>,
    #[rust]
    card: Rect,
    #[rust]
    hits: Vec<(Hit, Rect)>,
    #[rust]
    hot: Option<Hit>,
    #[rust]
    dragging: Option<Hit>,
    #[rust]
    pub inert: bool,
    /// Which hub models light up (see `hub_glow`), its clock, and the frame loop that
    /// runs only while something glows.
    #[rust]
    glow: GlowTracker,
    /// The hub panel's chat model list is open (the WM's hub child: a pick restarts it).
    #[rust]
    hub_pick: bool,
    #[rust]
    glow_clock: Option<Instant>,
    #[rust]
    glow_frame: NextFrame,
    /// The Wi-Fi dropdown's state (Linux): the iwd snapshot, the password
    /// prompt, the notice line. Fed by the WM from the worker's events.
    #[cfg(target_os = "linux")]
    #[rust]
    pub wifi: WifiUi,
    #[cfg(target_os = "linux")]
    #[rust]
    pub system: Arc<SystemSnapshot>,
    #[cfg(target_os = "linux")]
    #[rust]
    pub system_notice: String,
    #[cfg(target_os = "linux")]
    #[rust]
    device_picker: Option<bool>,
    #[cfg(target_os = "linux")]
    #[rust]
    device_scroll: usize,
    /// The main window's actual layout scale as the WM last read it off the
    /// platform (`window_geom.dpi_factor`) — never the saved value. Zero
    /// until the first read; the panel then shows the launch default.
    #[cfg(target_os = "linux")]
    #[rust]
    pub current_dpi: f64,
    /// What the renderer sees: the connected screens and their place in
    /// the wide desktop, and whether the WM drives them or the desktop does.
    #[cfg(target_os = "linux")]
    #[rust]
    pub display_snapshot: LinuxDisplaySnapshot,
    /// The screen rect when the anchor was taken: a re-laid-out window
    /// moves the anchor with its edge until the bar's fresh rect arrives.
    #[cfg(target_os = "linux")]
    #[rust]
    anchor_screen: Rect,
    /// The connector names behind the drawn screen rows (left to right,
    /// then the screens outside the desktop), so a click lands on the
    /// screen that was shown, never on whatever a fresh inventory put at
    /// that index.
    #[cfg(target_os = "linux")]
    #[rust]
    screen_targets: Vec<String>,
    /// How many of `screen_targets` are in the desktop (the movable ones).
    #[cfg(target_os = "linux")]
    #[rust]
    screen_placed: usize,
    /// The screen whose picker is unfolded, by name, and which picker.
    #[cfg(target_os = "linux")]
    #[rust]
    chooser: Option<(String, Chooser)>,
    /// The change behind each drawn `Hit::ChooserItem`, bound at draw time.
    #[cfg(target_os = "linux")]
    #[rust]
    chooser_changes: Vec<Change>,
    /// The change being tried and the notes, as the WM last polled them,
    /// and when (`Cx::monotonic_now`), for the revert countdown.
    #[cfg(target_os = "linux")]
    #[rust]
    pub display_trials: DisplayTrials,
    #[cfg(target_os = "linux")]
    #[rust]
    pub display_now: f64,
    /// The Pace by list is unfolded; the choice behind each drawn chip.
    #[cfg(target_os = "linux")]
    #[rust]
    pace_open: bool,
    #[cfg(target_os = "linux")]
    #[rust]
    pace_targets: Vec<DisplayPacing>,
    /// "Restart desktop now" was pressed once and asks to confirm.
    #[cfg(target_os = "linux")]
    #[rust]
    restart_confirm: bool,
    /// The restart was confirmed and is waiting for the display layout
    /// save to land before the process exits (`LinuxControls::restart_requested`,
    /// mirrored here every poll). The restart row shows "Restarting…"
    /// while this holds, in place of the button or the confirm step.
    #[cfg(target_os = "linux")]
    #[rust]
    pub restart_waiting: bool,
    /// The saved render-on GPU (`None` is Auto): the WM's working layout,
    /// so a pick shows at once. The render-on list checks it.
    #[cfg(target_os = "linux")]
    #[rust]
    pub render_on_saved: Option<String>,
    /// The compositor GPU list is unfolded.
    #[cfg(target_os = "linux")]
    #[rust]
    gpu_picker: bool,
    #[cfg(target_os = "linux")]
    #[rust]
    gpu_scroll: usize,
    #[cfg(target_os = "linux")]
    #[rust]
    gpu_overflow: bool,
    /// The choice behind each drawn `Hit::Gpu` row: `None` is Auto / Follow
    /// compositor, `Some` a GPU's sysfs identity — bound at draw time.
    #[cfg(target_os = "linux")]
    #[rust]
    gpu_targets: Vec<Option<String>>,
    /// The Display panel is showing the Mouse & touchpad subview.
    #[cfg(target_os = "linux")]
    #[rust]
    pointer_settings: bool,
    /// A source the renderer was asked for and has not yet named as the
    /// primary, with when it was asked. The WM clears it on observation
    /// or reports it after a bounded wait.
    #[cfg(target_os = "linux")]
    #[rust]
    pub display_source_pending: Option<(String, f64)>,
    /// Escape cancelled a scale preview: its key-up is taken too, so no
    /// tile sees half a key press.
    #[cfg(target_os = "linux")]
    #[rust]
    swallow_escape_up: bool,
    /// The scale slider's drag: the scale when it started (Escape goes
    /// back to it), the track in device pixels and the scale last applied.
    /// The window re-lays out under the drag, so the pointer is read in
    /// device pixels against where the track was: the value follows the
    /// hand, not the moving card.
    #[cfg(target_os = "linux")]
    #[rust]
    dpi_drag: Option<DpiDrag>,
}

impl ShellPanel {
    pub fn toggle(&mut self, cx: &mut Cx, kind: PanelKind, anchor: Rect) {
        // A scale drag the flyout leaves mid-way is saved where it got to.
        #[cfg(target_os = "linux")]
        self.dpi_drag_end(cx);
        let was_open = self.open;
        if self.open == Some(kind) {
            self.open = None;
        } else {
            self.open = Some(kind);
            self.anchor = anchor;
            #[cfg(target_os = "linux")]
            {
                self.anchor_screen = self.screen;
                // The outputs and compositor GPU as of this open; the WM
                // refreshes them while the panel stays up.
                if kind == PanelKind::Monitor {
                    self.display_snapshot = cx.linux_display_snapshot();
                }
            }
        }
        self.left_panel(was_open);
        self.redraw(cx);
    }

    /// The open flyout follows its module to another bar segment (the
    /// same module pressed on another screen): a new anchor, still open,
    /// nothing re-read.
    pub fn move_to(&mut self, cx: &mut Cx, anchor: Rect) {
        if self.open.is_none() {
            return;
        }
        self.anchor = anchor;
        #[cfg(target_os = "linux")]
        {
            self.anchor_screen = self.screen;
        }
        self.redraw(cx);
    }

    /// Seconds on the glow clock (it starts with the first read).
    fn glow_now(&mut self) -> f64 {
        self.glow_clock.get_or_insert_with(Instant::now).elapsed().as_secs_f64()
    }

    /// A new hub snapshot: the glow follows it, and the panel redraws when something
    /// changed, glows, or a "used Ns ago" note moved. An idle hub redraws nothing.
    pub fn hub_update(&mut self, cx: &mut Cx, snap: crate::hub_control::HubSnapshot) {
        let now = self.glow_now();
        self.glow.update(&snap, now);
        let notes = self.glow.notes_changed(now);
        let changed = self.data.hub != snap;
        if changed {
            self.data.hub = snap;
        }
        if changed || notes || self.glow.animating(now) {
            self.redraw(cx);
        }
    }

    pub fn close(&mut self, cx: &mut Cx) {
        // A scale drag the flyout leaves mid-way is saved where it got to.
        #[cfg(target_os = "linux")]
        self.dpi_drag_end(cx);
        let was_open = self.open;
        self.open = None;
        self.left_panel(was_open);
        self.redraw(cx);
    }

    /// The bar re-laid out (a DPI change, a resize): follow the module.
    /// True when the card moved.
    #[cfg(target_os = "linux")]
    pub fn reanchor(&mut self, cx: &mut Cx, anchor: Rect) -> bool {
        if self.open.is_none() || (self.anchor == anchor && self.anchor_screen == self.screen) {
            return false;
        }
        self.anchor = anchor;
        self.anchor_screen = self.screen;
        self.redraw(cx);
        true
    }

    /// The window was re-laid out since the anchor was taken (a DPI change
    /// moves every logical edge). Until the bar's fresh module rect arrives
    /// (`reanchor`, the next frame), keep a right-cluster module at its
    /// distance from the right edge and the clock at the centre, so the
    /// card never spends a frame off its module or off the screen.
    #[cfg(target_os = "linux")]
    fn follow_screen(&mut self, kind: PanelKind, screen: Rect) {
        if self.anchor_screen.size.x <= 0.0 || self.anchor.size.x <= 0.0 {
            self.anchor_screen = screen;
            return;
        }
        if self.anchor_screen == screen {
            return;
        }
        let old = self.anchor_screen;
        let dx = match kind {
            PanelKind::Clock => (screen.pos.x + screen.size.x * 0.5) - (old.pos.x + old.size.x * 0.5),
            _ => (screen.pos.x + screen.size.x) - (old.pos.x + old.size.x),
        };
        self.anchor.pos.x += dx;
        self.anchor.pos.y += screen.pos.y - old.pos.y;
        self.anchor_screen = screen;
    }

    /// The flyout that was up is no longer: whatever it held that must
    /// not outlive it (the Wi-Fi password prompt, an unapplied scale
    /// preview) goes now.
    fn left_panel(&mut self, was_open: Option<PanelKind>) {
        self.dragging = None;
        #[cfg(target_os = "linux")]
        {
            self.device_picker = None;
            self.device_scroll = 0;
            self.chooser = None;
            self.chooser_changes.clear();
            self.pace_open = false;
            self.pace_targets.clear();
            self.restart_confirm = false;
            self.gpu_picker = false;
            self.gpu_scroll = 0;
            self.gpu_targets.clear();
            self.pointer_settings = false;
        }
        #[cfg(target_os = "linux")]
        if was_open == Some(PanelKind::Network) && self.open != Some(PanelKind::Network) {
            self.wifi.on_close();
        }
        let _ = was_open;
    }

    /// The card rect: under the bar, centered on the module, `gapsOut` off
    /// the bar edge, clamped into the screen by the same margin.
    fn card_rect(&self, screen: Rect, kind: PanelKind, height: f64) -> Rect {
        let margin = self.tokens.spacing.gaps_out;
        let w = kind.content_width() + self.tokens.spacing.popup_padding * 2.0;
        // A window narrower than the card (a small screen at a large
        // scale) gets the card fitted between the margins, not clipped.
        #[cfg(target_os = "linux")]
        let w = w.min((screen.size.x - margin * 2.0).max(0.0));
        panel_card_rect(self.anchor, screen, w, height, margin, self.tokens.bar.size_horizontal)
    }

    fn section_header(&mut self, cx: &mut Cx2d, r: Rect, label: &str, value: &str) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        self.d.section_header(cx, r, &tok, fg, label);
        if !value.is_empty() {
            self.d.label(
                cx,
                r,
                true,
                tok.font.caption,
                darker(fg, 1.4),
                super::ui::HAlign::Right,
                value,
            );
        }
    }

    /// A `CursorSurface` slider row: the track inset by `space(6)` with the
    /// panel's own hover chrome.
    fn slider_row(&mut self, cx: &mut Cx2d, r: Rect, hit: Hit, progress: f64, enabled: bool) {
        let tok = self.tokens;
        let hot = self.hot == Some(hit) || self.dragging == Some(hit);
        if hot && enabled {
            self.d
                .cursor_surface(cx, r, &tok.controls, true, false);
        }
        let track = inset(r, 6.0);
        #[cfg(not(target_os = "linux"))]
        let fg = if enabled {
            tok.popups.text
        } else {
            alpha(tok.popups.text, 0.4)
        };
        #[cfg(not(target_os = "linux"))]
        self.d.panel_slider(
            cx,
            track,
            &tok,
            progress,
            fg,
            tok.popups.background,
            hot && enabled,
        );
        #[cfg(target_os = "linux")]
        self.d.system_slider(cx, track, &tok, progress, enabled, hot);
        if enabled { self.hits.push((hit, r)); }
    }

    fn info_pair(&mut self, cx: &mut Cx2d, r: Rect, label: &str, value: &str) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        self.d.label(
            cx,
            r,
            false,
            tok.font.body_small,
            alpha(fg, 0.6),
            super::ui::HAlign::Left,
            label,
        );
        self.d.label(
            cx,
            r,
            false,
            tok.font.body_small,
            fg,
            super::ui::HAlign::Right,
            value,
        );
    }

    /// "Not available on this OS" — the honest reading of a panel whose
    /// service does not exist here.
    /// The hub child's Restart button at the right of `row`; its width.
    fn hub_restart_button(&mut self, cx: &mut Cx2d, row: Rect) -> f64 {
        let tok = self.tokens;
        let w = self.d.button_width(cx, &tok, false, "Restart", tok.font.body_small);
        let r = rect(row.pos.x + row.size.x - w, row.pos.y + 3.0, w, row.size.y - 6.0);
        let state = if self.hot == Some(Hit::HubRestart) { CtrlState::Hover } else { CtrlState::Normal };
        self.d.button(cx, r, &tok, state, None, "Restart", tok.font.body_small, tok.popups.text, true);
        self.hits.push((Hit::HubRestart, r));
        w
    }

    fn unavailable(&mut self, cx: &mut Cx2d, r: Rect, what: &str) {
        let tok = self.tokens;
        self.d.label(
            cx,
            r,
            false,
            tok.font.body,
            alpha(tok.popups.text, 0.6),
            super::ui::HAlign::Left,
            what,
        );
    }

    pub fn draw_surface(&mut self, cx: &mut Cx2d, screen: Rect) {
        self.draw_surface_in(cx, screen, screen);
    }

    /// Draw into `window` (the whole overlay) with the card clamped into
    /// `screen` (`window` itself on one screen).
    fn draw_surface_in(&mut self, cx: &mut Cx2d, window: Rect, screen: Rect) {
        self.screen = window;
        let Some(kind) = self.open else {
            self.card = Rect::default();
            self.hits.clear();
            return;
        };
        self.hits.clear();
        let tok = self.tokens;
        let pad = tok.spacing.popup_padding;
        let gap = tok.spacing.panel_gap;
        let height = match kind {
            PanelKind::Clock => 470.0,
            PanelKind::Audio => self.audio_height(),
            PanelKind::Power => 300.0,
            PanelKind::Monitor => self.monitor_height(),
            PanelKind::Network => self.network_height(),
            PanelKind::Bluetooth => 150.0,
            PanelKind::Hub => self.hub_height(),
        } + pad * 2.0;
        // The body is inset by the padding AND the popup border. The
        // Display panel's plan is sized to the body, so its card asks for
        // the border too; otherwise the full plan never fits and the
        // arrangement picture is dropped even on a tall desktop. The screen clamp and
        // the compact fallback still apply.
        #[cfg(target_os = "linux")]
        let height = if kind == PanelKind::Monitor {
            height + tok.popups.border_width * 2.0
        } else {
            height
        };
        #[cfg(target_os = "linux")]
        self.follow_screen(kind, window);
        let card = self.card_rect(screen, kind, height);
        self.card = card;
        self.d.card(cx, card, &tok.popups);
        let body = inset(card, pad + tok.popups.border_width);
        let _ = gap;
        match kind {
            PanelKind::Clock => self.draw_clock(cx, body),
            PanelKind::Audio => self.draw_audio(cx, body),
            PanelKind::Power => self.draw_power(cx, body),
            PanelKind::Monitor => self.draw_monitor(cx, body),
            // Linux: the Wi-Fi dropdown over iwd (shell/wifi_linux.rs).
            #[cfg(target_os = "linux")]
            PanelKind::Network => self.draw_wifi(cx, body),
            // Elsewhere the panel keeps its honest "no service here" reading.
            #[cfg(not(target_os = "linux"))]
            PanelKind::Network => {
                let (hero, rest) = cut_top(body, 40.0);
                let net = self.data.network.clone();
                self.d.panel_hero(
                    cx,
                    hero,
                    &tok,
                    tok.popups.text,
                    if net.is_some() { Ico::Wifi } else { Ico::WifiOff },
                    "Network",
                    net.as_deref().unwrap_or("Not connected"),
                    0.0,
                );
                let (sep, rest) = cut_top(rest, tok.spacing.panel_gap);
                self.d.separator(cx, sep, tok.popups.text, 0.12);
                self.unavailable(
                    cx,
                    rest,
                    "Wi-Fi scanning and DNS switching need NetworkManager.",
                );
            }
            PanelKind::Hub => self.draw_hub(cx, body),
            PanelKind::Bluetooth => {
                let (hero, rest) = cut_top(body, 40.0);
                let on = self.data.bluetooth.unwrap_or(false);
                self.d.panel_hero(
                    cx,
                    hero,
                    &tok,
                    tok.popups.text,
                    if on { Ico::Bluetooth } else { Ico::BluetoothOff },
                    "Bluetooth",
                    if on { "Powered on" } else { "Turned off" },
                    0.0,
                );
                let (sep, rest) = cut_top(rest, tok.spacing.panel_gap);
                self.d.separator(cx, sep, tok.popups.text, 0.12);
                self.unavailable(cx, rest, "Device pairing needs BlueZ.");
            }
        }
    }

    // ------------------------------------------------------------- clock

    fn draw_clock(&mut self, cx: &mut Cx2d, body: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let (ty, tm, td) = self.data.today;
        let (vy, vm) = self.data.view;

        // Hero: the calendar glyph and "MMMM d".
        let (hero, rest) = cut_top(body, 56.0);
        self.d
            .icon_centered(cx, Ico::Calendar, rect(hero.pos.x, hero.pos.y, 48.0, hero.size.y), 48.0, fg);
        let date = format!("{} {}", MONTH_NAMES[(tm - 1) as usize], td);
        self.d.label(
            cx,
            rect(
                hero.pos.x + 48.0 + 22.0,
                hero.pos.y,
                hero.size.x - 70.0,
                hero.size.y,
            ),
            true,
            52.0,
            fg,
            super::ui::HAlign::Left,
            &date,
        );
        self.hits.push((Hit::Today, hero));

        // Year progress.
        let (year_row, rest) = cut_top(rest, 26.0);
        let day_of_year = days_from_civil(ty, tm, td) - days_from_civil(ty, 1, 1);
        let year_len = days_from_civil(ty + 1, 1, 1) - days_from_civil(ty, 1, 1);
        let done = day_of_year as f64 / year_len as f64;
        self.d.label(
            cx,
            year_row,
            false,
            tok.font.body_small,
            darker(fg, 1.5),
            super::ui::HAlign::Left,
            &format!("{}", ty),
        );
        self.d.label(
            cx,
            year_row,
            false,
            tok.font.body_small,
            fg,
            super::ui::HAlign::Right,
            &format!("{}%", (done * 100.0).round() as i64),
        );
        let track = rect(
            year_row.pos.x,
            year_row.pos.y + year_row.size.y - 6.0,
            year_row.size.x,
            6.0,
        );
        self.d.solid(cx, track, alpha(fg, 0.12));
        self.d.solid(
            cx,
            rect(track.pos.x, track.pos.y, track.size.x * done, track.size.y),
            alpha(fg, tok.controls.selected_fill_alpha + 0.5),
        );

        // The month grid.
        let (_, grid) = cut_top(rest, 18.0);
        let week_col = 32.0;
        let gutter = 14.0;
        let cell_w = ((grid.size.x - week_col - gutter) / 7.0).floor();
        let head_h = 16.0;
        let cell_h = 34.0;
        // Header: the week toggle then the weekday captions.
        self.d.label(
            cx,
            rect(grid.pos.x, grid.pos.y, week_col, head_h),
            true,
            tok.font.caption,
            darker(fg, 1.9),
            super::ui::HAlign::Center,
            "W",
        );
        for (i, name) in WEEKDAY_NAMES.iter().enumerate() {
            self.d.label(
                cx,
                rect(
                    grid.pos.x + week_col + gutter + i as f64 * cell_w,
                    grid.pos.y,
                    cell_w,
                    head_h,
                ),
                true,
                tok.font.caption,
                darker(fg, 1.4),
                super::ui::HAlign::Center,
                name,
            );
        }
        // A hairline in the week gutter, under the header.
        self.d.solid(
            cx,
            rect(
                grid.pos.x + week_col + gutter * 0.5,
                grid.pos.y + head_h,
                1.0,
                cell_h * 6.0,
            ),
            alpha(fg, 0.1),
        );

        let first_dow = weekday(vy, vm, 1) as i64;
        let dim = days_in_month(vy, vm) as i64;
        for row in 0..6i64 {
            let y = grid.pos.y + head_h + row as f64 * cell_h;
            // The week number of this row's Monday.
            let day_index = row * 7 - first_dow + 1;
            let monday = days_from_civil(vy, vm, 1) + (day_index - 1).max(-6);
            let (wy, wm, wd) = civil_from_days(monday);
            self.d.label(
                cx,
                rect(grid.pos.x, y, week_col, cell_h),
                false,
                tok.font.caption,
                darker(fg, 1.9),
                super::ui::HAlign::Center,
                &format!("{}", iso_week(wy, wm, wd)),
            );
            for col in 0..7i64 {
                let n = row * 7 + col - first_dow + 1;
                let cell = rect(
                    grid.pos.x + week_col + gutter + col as f64 * cell_w,
                    y,
                    cell_w,
                    cell_h,
                );
                let in_month = n >= 1 && n <= dim;
                let label = if in_month {
                    format!("{}", n)
                } else {
                    // Neighbouring months still show their numbers, dimmed.
                    let d = days_from_civil(vy, vm, 1) + (n - 1);
                    let (_, _, dd) = civil_from_days(d);
                    format!("{}", dd)
                };
                let is_today = in_month && (vy, vm, n as u32) == (ty, tm, td);
                let weekend = col >= 5;
                let color = if !in_month {
                    darker(fg, 2.2)
                } else if weekend {
                    darker(fg, 1.45)
                } else {
                    fg
                };
                if is_today {
                    self.d.control(cx, cell, &tok.controls, CtrlState::Normal);
                }
                self.d.label(
                    cx,
                    cell,
                    is_today,
                    tok.font.body,
                    color,
                    super::ui::HAlign::Center,
                    &label,
                );
            }
        }

        // The month nav row.
        let nav_y = grid.pos.y + head_h + cell_h * 6.0 + 4.0;
        let nav = rect(grid.pos.x, nav_y, grid.size.x, 24.0);
        let prev = rect(nav.pos.x, nav.pos.y, 24.0, nav.size.y);
        let next = rect(
            nav.pos.x + nav.size.x - 24.0,
            nav.pos.y,
            24.0,
            nav.size.y,
        );
        self.d
            .icon_centered(cx, Ico::ChevronLeft, prev, 14.0, darker(fg, 1.4));
        self.d
            .icon_centered(cx, Ico::ChevronRight, next, 14.0, darker(fg, 1.4));
        self.hits.push((Hit::PrevMonth, prev));
        self.hits.push((Hit::NextMonth, next));
        let title = format!("{} {}", MONTH_NAMES[(vm - 1) as usize].to_uppercase(), vy);
        self.d.label(
            cx,
            nav,
            false,
            tok.font.body,
            darker(fg, 1.4),
            super::ui::HAlign::Center,
            &title,
        );
    }

    // ------------------------------------------------------------- audio

    #[cfg(not(target_os = "linux"))]
    fn draw_audio(&mut self, cx: &mut Cx2d, body: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let mood = self.data.volume_mood();
        let level = self.data.volume;
        let (hero, rest) = cut_top(body, 40.0);
        // The mute switch rides on the right of the hero.
        let switch = self.d.toggle_switch(
            cx,
            dvec2(
                hero.pos.x + hero.size.x - 42.0,
                hero.pos.y + (hero.size.y - 22.0) * 0.5,
            ),
            &tok,
            !self.data.muted && level.is_some(),
            fg,
        );
        self.hits.push((Hit::MuteToggle, switch));
        self.d.panel_hero(
            cx,
            hero,
            &tok,
            fg,
            if self.data.muted {
                Ico::Volume0
            } else {
                Ico::Speaker
            },
            "Audio",
            mood,
            switch.size.x + 12.0,
        );

        let (sep, rest) = cut_top(rest, tok.spacing.panel_gap);
        self.d.separator(cx, sep, fg, 0.12);

        let (header, rest) = cut_top(rest, 18.0);
        let value = match level {
            Some(v) => format!("{}%", v),
            None => "--".to_string(),
        };
        self.section_header(cx, header, "OUTPUT", &value);
        let (row, rest) = cut_top(rest, 28.0);
        self.slider_row(
            cx,
            row,
            Hit::VolumeSlider,
            level.unwrap_or(0) as f64 / 100.0,
            level.is_some(),
        );

        // Output devices, one `CursorSurface` row each. Without a device
        // list the panel says so rather than showing an empty section.
        let mut rest = rest;
        if self.data.outputs.is_empty() {
            let (row, next) = cut_top(rest, 26.0);
            rest = next;
            self.unavailable(cx, row, "Device switching needs PipeWire.");
        }
        for (name, active) in self.data.outputs.clone() {
            let (row, next) = cut_top(rest, 26.0);
            rest = next;
            let ico = if name.to_lowercase().contains("head") {
                Ico::Headphone
            } else {
                Ico::Speaker
            };
            self.d.icon_centered(
                cx,
                ico,
                rect(row.pos.x, row.pos.y, 22.0, row.size.y),
                tok.font.body,
                fg,
            );
            self.d.label_elided(
                cx,
                rect(row.pos.x + 22.0 + 6.0, row.pos.y, row.size.x - 60.0, row.size.y),
                active,
                tok.font.body,
                fg,
                super::ui::HAlign::Left,
                &name,
            );
            if active {
                self.d.icon_centered(
                    cx,
                    Ico::Check,
                    rect(
                        row.pos.x + row.size.x - 16.0,
                        row.pos.y,
                        14.0,
                        row.size.y,
                    ),
                    tok.font.subtitle,
                    fg,
                );
            }
        }

        if let Some(input) = self.data.input_volume {
            let (sep, rest2) = cut_top(rest, tok.spacing.panel_gap);
            self.d.separator(cx, sep, fg, 0.12);
            let (header, rest3) = cut_top(rest2, 18.0);
            self.section_header(cx, header, "INPUT", &format!("{}%", input));
            let (row, _) = cut_top(rest3, 28.0);
            self.slider_row(cx, row, Hit::InputSlider, input as f64 / 100.0, false);
        }
    }

    // ------------------------------------------------------------- power

    fn hub_height(&self) -> f64 {
        let h = &self.data.hub;
        if !h.available {
            return match &h.process {
                Some(p) => 40.0 + 12.0 + p.phase.reason().map_or(0.0, |r| r.lines().take(HUB_REASON_LINES).count() as f64 * 16.0 + 6.0) + 34.0,
                None => 40.0 + 12.0 + 40.0,
            };
        }
        let policy = if h.policy.is_some() { 18.0 } else { 0.0 };
        let process = if h.process.is_some() { 30.0 + if self.hub_pick { hub_chat_models(h).len() as f64 * 26.0 } else { 0.0 } } else { 0.0 };
        let activity = h.activity.len().max(1) as f64 * 20.0;
        let recent = h.recent.len().min(3) as f64 * 16.0;
        let context = if h.models.iter().any(|m| Some(&m.id) == h.chat_model.as_ref()) { 30.0 } else { 0.0 };
        40.0 + 12.0 + 38.0 + policy + process + 22.0 + h.models.len() as f64 * 34.0 + context + 12.0 + 22.0 + activity + 20.0 + recent
    }

    /// The AI hub panel: GPU memory, the models with Load / Unload, what is
    /// working now and what finished, the context size and the KV switch.
    fn draw_hub(&mut self, cx: &mut Cx2d, body: Rect) {
        use crate::hub_control::{KvPrecision, ModelState};
        use super::ui::HAlign;
        let tok = self.tokens;
        let fg = tok.popups.text;
        let hub = self.data.hub.clone();
        let now = self.glow_now();
        if self.glow.animating(now) {
            self.glow_frame = cx.new_next_frame();
        }
        let (hero, rest) = cut_top(body, 40.0);
        let status = hub.status();
        self.d.panel_hero(cx, hero, &tok, fg, Ico::Chip, "AI hub", &status, 0.0);
        let (sep, rest) = cut_top(rest, 12.0);
        self.d.separator(cx, rect(sep.pos.x, sep.pos.y + 6.0, sep.size.x, 1.0), fg, 0.12);
        if !hub.available {
            match &hub.process {
                // The WM's own hub on its way up, or down: why, and Restart.
                Some(p) => {
                    let mut rest = rest;
                    if let Some(reason) = p.phase.reason() {
                        for line in reason.lines().take(HUB_REASON_LINES) {
                            let (row, r2) = cut_top(rest, 16.0);
                            rest = r2;
                            self.d.label_elided(cx, row, false, tok.font.caption, darker(fg, 1.2), HAlign::Left, line);
                        }
                        rest = cut_top(rest, 6.0).1;
                    }
                    let (row, _) = cut_top(rest, 34.0);
                    self.hub_restart_button(cx, row);
                }
                None => self.unavailable(cx, rest, "Start the AI hub to see its models."),
            }
            return;
        }
        // GPU memory.
        let (row, rest) = cut_top(rest, 22.0);
        let gib = |mb: u32| mb as f64 / 1024.0;
        self.d.label(cx, row, true, tok.font.caption, darker(fg, 1.4), HAlign::Left, "GPU memory");
        self.d.label(cx, row, false, tok.font.caption, fg, HAlign::Right,
            &format!("{:.1} / {:.1} GiB, {:.1} free", gib(hub.gpu_used_mb), gib(hub.gpu_total_mb), gib(hub.gpu_total_mb.saturating_sub(hub.gpu_used_mb))));
        let (bar_row, rest) = cut_top(rest, 16.0);
        let track = rect(bar_row.pos.x, bar_row.pos.y + 4.0, bar_row.size.x, 8.0);
        self.d.solid(cx, track, alpha(fg, 0.12));
        let used = if hub.gpu_total_mb > 0 { (hub.gpu_used_mb as f64 / hub.gpu_total_mb as f64).clamp(0.0, 1.0) } else { 0.0 };
        self.d.solid(cx, rect(track.pos.x, track.pos.y, track.size.x * used, track.size.y), fg);
        // Models, grouped by what they do (the hub sorts them): a type icon, the name, one
        // line on what it does with its state and GPU footprint, Load / Unload. The chat
        // model's context setting sits under it, capped by what the GPU budget allows.
        // What the hub's memory policy did last: a variant run, a chat model unloaded for a job.
        let rest = match &hub.policy {
            Some(note) => {
                let (row, rest) = cut_top(rest, 18.0);
                self.d.label_elided(cx, row, false, tok.font.caption, darker(fg, 1.2), HAlign::Left, note);
                rest
            }
            None => rest,
        };
        // The WM's own hub: the chat model it loads at start (a pick restarts it), and Restart.
        let rest = match &hub.process {
            Some(p) => {
                let (row, mut rest) = cut_top(rest, 30.0);
                let restart_w = self.hub_restart_button(cx, row);
                let pick_w = self.d.button_width(cx, &tok, false, "Change", tok.font.body_small);
                let pick = rect(row.pos.x + row.size.x - restart_w - 6.0 - pick_w, row.pos.y + 3.0, pick_w, row.size.y - 6.0);
                let state = if self.hub_pick { CtrlState::Selected } else if self.hot == Some(Hit::HubPickOpen) { CtrlState::Hover } else { CtrlState::Normal };
                self.d.button(cx, pick, &tok, state, None, "Change", tok.font.body_small, fg, true);
                self.hits.push((Hit::HubPickOpen, pick));
                self.d.label(cx, rect(row.pos.x, row.pos.y, 80.0, row.size.y), false, tok.font.caption, darker(fg, 1.4), HAlign::Left, "Chat model");
                let name = if p.model.is_empty() { "none at start" } else { p.model.as_str() };
                self.d.label_elided(cx, rect(row.pos.x + 80.0, row.pos.y, (pick.pos.x - 8.0 - row.pos.x - 80.0).max(0.0), row.size.y), false, tok.font.body_small, fg, HAlign::Left, name);
                if self.hub_pick {
                    for (i, id) in hub_chat_models(&hub).iter().enumerate() {
                        let (row, r2) = cut_top(rest, 26.0);
                        rest = r2;
                        let r = rect(row.pos.x + 80.0, row.pos.y + 2.0, row.size.x - 80.0, row.size.y - 4.0);
                        let state = if *id == p.model { CtrlState::Selected } else if self.hot == Some(Hit::HubPick(i)) { CtrlState::Hover } else { CtrlState::Normal };
                        self.d.button(cx, r, &tok, state, None, id, tok.font.body_small, fg, false);
                        self.hits.push((Hit::HubPick(i), r));
                    }
                }
                rest
            }
            None => rest,
        };
        let (head, mut rest) = cut_top(rest, 22.0);
        self.d.section_header(cx, rect(head.pos.x, head.pos.y + 4.0, head.size.x, head.size.y - 4.0), &tok, fg, "Models");
        for (i, m) in hub.models.iter().enumerate() {
            let (row, r2) = cut_top(rest, 34.0);
            rest = r2;
            let (label, state, disabled) = match m.state {
                ModelState::Loaded => ("Unload", CtrlState::Normal, false),
                ModelState::Unloaded | ModelState::Error => ("Load", CtrlState::Normal, false),
                ModelState::Loading => ("Loading", CtrlState::Disabled, true),
                ModelState::Downloading => ("Getting", CtrlState::Disabled, true),
            };
            // Three columns that never overlap: [icon + name / what it does] takes what is
            // left and elides, [state / GPU size] and [Load] have fixed widths on the right.
            const GAP: f64 = 10.0;
            const ICON: f64 = 18.0;
            let g = self.glow.row(&m.id, now);
            let k = (0.5 - 0.5 * (now * std::f64::consts::TAU / 1.4).cos()) as f32;
            let lit = g.strength * if g.working { 0.8 + 0.2 * k } else { 1.0 };
            if g.strength > 0.0 {
                self.d.solid(cx, rect(row.pos.x - 6.0, row.pos.y + 1.0, row.size.x + 12.0, row.size.y - 2.0), vec4(0.30, 0.78, 0.42, 0.95 * lit));
            }
            let w = self.d.button_width(cx, &tok, false, "Unload", tok.font.body_small);
            let button = rect(row.pos.x + row.size.x - w, row.pos.y + 4.0, w, row.size.y - 8.0);
            let state = if !disabled && self.hot == Some(Hit::HubModel(i)) { CtrlState::Hover } else { state };
            let lit_now = self.glow.row(&m.id, now).strength;
            let label_fg = if lit_now > 0.0 { lerp_color(fg, vec4(0.02, 0.14, 0.06, 1.0), lit_now) } else { fg };
            if lit_now > 0.0 {
                // On the green the button is a darker pad with dark ink (the themed
                // button's own label colour is light).
                self.d.solid(cx, button, vec4(0.02, 0.14, 0.06, 0.14 * lit_now));
                self.d.label(cx, button, true, tok.font.body_small, label_fg, HAlign::Center, label);
            } else {
                self.d.button(cx, button, &tok, state, None, label, tok.font.body_small, label_fg, true);
            }
            if !disabled {
                self.hits.push((Hit::HubModel(i), button));
            }
            let state_w = ["downloading", "on disk", "loaded", "loading", "failed", "99.9 GiB"]
                .iter()
                .map(|t| self.d.measure(cx, false, tok.font.caption, t))
                .fold(0.0f64, f64::max)
                .ceil();
            let state_col = rect(button.pos.x - GAP - state_w, row.pos.y, state_w, row.size.y);
            let dim = if m.state == ModelState::Loaded { fg } else { darker(fg, 1.4) };
            // A working model lights up: a soft wash and edge in the accent that breathes,
            // an accent icon and name, a live line. It fades out after the work.
            // Computing = a calm green row (brightness breathes), dark text on it.
            let ink = vec4(0.02, 0.14, 0.06, 1.0);
            let dim = if g.strength > 0.0 { lerp_color(dim, ink, g.strength) } else { dim };
            let sub = if g.strength > 0.0 { lerp_color(darker(fg, 1.4), ink, g.strength) } else { darker(fg, 1.4) };
            self.d.icon(cx, hub_icon(m.kind), rect(row.pos.x, row.pos.y + (row.size.y - ICON) * 0.5, ICON, ICON), dim);
            let text_x = row.pos.x + ICON + 8.0;
            let text = rect(text_x, row.pos.y, (state_col.pos.x - GAP - text_x).max(0.0), row.size.y);
            let (top, bottom) = (rect(text.pos.x, text.pos.y + 2.0, text.size.x, 15.0), rect(text.pos.x, text.pos.y + 17.0, text.size.x, 14.0));
            self.d.label_elided(cx, top, false, tok.font.body_small, dim, HAlign::Left, &m.name);
            if let Some(live) = &g.live {
                self.d.label_elided(cx, bottom, false, tok.font.caption, ink, HAlign::Left, live);
            } else if let Some(used) = &g.used {
                self.d.label_elided(cx, bottom, false, tok.font.caption, sub, HAlign::Left, &format!("{} \u{b7} {}", m.kind.what(), used));
            } else {
                self.d.label_elided(cx, bottom, false, tok.font.caption, sub, HAlign::Left, m.kind.what());
            }
            let state = match m.state {
                ModelState::Loaded => "loaded",
                ModelState::Loading => "loading",
                ModelState::Downloading => "downloading",
                ModelState::Error => "failed",
                ModelState::Unloaded => "on disk",
            };
            let (stop, sbottom) = (rect(state_col.pos.x, top.pos.y, state_col.size.x, top.size.y), rect(state_col.pos.x, bottom.pos.y, state_col.size.x, bottom.size.y));
            self.d.label(cx, stop, false, tok.font.caption, dim, HAlign::Right, state);
            if m.state == ModelState::Loaded && m.gpu_mb > 0 {
                self.d.label(cx, sbottom, false, tok.font.caption, sub, HAlign::Right, &format!("{:.1} GiB", gib(m.gpu_mb)));
            }
            if Some(&m.id) != hub.chat_model.as_ref() {
                continue;
            }
            // This model's context: a step down / up through the sizes the hub offers, up
            // to what fits next to the UI; then the KV precision.
            let (row, r2) = cut_top(rest, 30.0);
            rest = r2;
            let step = self.d.button_width(cx, &tok, false, "+", tok.font.body_small).max(24.0);
            let mut x = row.pos.x + 26.0;
            self.d.label_elided(cx, rect(x, row.pos.y, 52.0, row.size.y), false, tok.font.caption, darker(fg, 1.4), HAlign::Left, "Context");
            x += 56.0;
            let can_up = hub_context_up(&hub).is_some();
            for (label, up) in [("-", false), ("+", true)] {
                let r = rect(x, row.pos.y + 3.0, step, row.size.y - 6.0);
                let enabled = if up { can_up } else { hub.context_options.iter().any(|c| *c < hub.context) };
                let state = if !enabled { CtrlState::Disabled } else if self.hot == Some(Hit::HubContext(up)) { CtrlState::Hover } else { CtrlState::Normal };
                self.d.button(cx, r, &tok, state, None, label, tok.font.body_small, fg, true);
                if enabled {
                    self.hits.push((Hit::HubContext(up), r));
                }
                x += step + 4.0;
                if !up {
                    self.d.label(cx, rect(x, row.pos.y, 48.0, row.size.y), true, tok.font.body_small, fg, HAlign::Center, &kilo(hub.context));
                    x += 48.0 + 4.0;
                }
            }
            let max = match m.context_fit {
                Some(0) => "no room next to the UI".to_string(),
                Some(fit) => format!("max {}", kilo(fit)),
                None => String::new(),
            };
            let kw = self.d.button_width(cx, &tok, false, "f16", tok.font.body_small);
            self.d.label_elided(cx, rect(x + 4.0, row.pos.y, ((row.pos.x + row.size.x - 2.0 * kw - 10.0) - (x + 4.0)).max(0.0), row.size.y), false, tok.font.caption, darker(fg, 1.4), HAlign::Left, &max);
            for (k, q8) in [(KvPrecision::F16, false), (KvPrecision::Q8, true)] {
                let kx = row.pos.x + row.size.x - kw * (2.0 - q8 as u8 as f64);
                let r = rect(kx, row.pos.y + 3.0, kw, row.size.y - 6.0);
                let state = if hub.kv == k { CtrlState::Selected } else if self.hot == Some(Hit::HubKv(q8)) { CtrlState::Hover } else { CtrlState::Normal };
                self.d.button(cx, r, &tok, state, None, k.name(), tok.font.body_small, fg, true);
                self.hits.push((Hit::HubKv(q8), r));
            }
        }
        // Activity.
        let (sep, rest) = cut_top(rest, 12.0);
        self.d.separator(cx, rect(sep.pos.x, sep.pos.y + 6.0, sep.size.x, 1.0), fg, 0.12);
        let (head, mut rest) = cut_top(rest, 22.0);
        self.d.section_header(cx, rect(head.pos.x, head.pos.y + 4.0, head.size.x, head.size.y - 4.0), &tok, fg, "Working now");
        if hub.activity.is_empty() {
            let (row, r2) = cut_top(rest, 20.0);
            rest = r2;
            self.d.label(cx, row, false, tok.font.body_small, darker(fg, 1.4), HAlign::Left, "Nothing");
        }
        for a in &hub.activity {
            let (row, r2) = cut_top(rest, 20.0);
            rest = r2;
            let rate = match (a.tokens_per_s, a.progress) {
                (Some(t), _) => format!("{:.0} tok/s", t),
                (None, Some(p)) => format!("{:.0}%", p * 100.0),
                _ => String::new(),
            };
            let rate_w = self.d.measure(cx, false, tok.font.body_small, &rate);
            let left = rect(row.pos.x, row.pos.y, (row.size.x - rate_w - 10.0).max(0.0), row.size.y);
            self.d.label_elided(cx, left, false, tok.font.body_small, fg, HAlign::Left, &format!("{} for {}", a.model, a.app));
            self.d.label(cx, row, false, tok.font.body_small, fg, HAlign::Right, &rate);
        }
        // Recent requests.
        let (head, mut rest) = cut_top(rest, 20.0);
        self.d.section_header(cx, rect(head.pos.x, head.pos.y + 4.0, head.size.x, head.size.y - 4.0), &tok, fg, "Recent");
        for q in hub.recent.iter().take(3) {
            let (row, r2) = cut_top(rest, 16.0);
            rest = r2;
            let r = if q.ok { format!("{} ms", q.millis) } else { format!("{} ms, failed", q.millis) };
            let r_w = self.d.measure(cx, false, tok.font.caption, &r);
            let left = rect(row.pos.x, row.pos.y, (row.size.x - r_w - 10.0).max(0.0), row.size.y);
            self.d.label_elided(cx, left, false, tok.font.caption, darker(fg, 1.4), HAlign::Left, &format!("{} for {}", q.model, q.app));
            self.d.label(cx, row, false, tok.font.caption, darker(fg, 1.4), HAlign::Right, &r);
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn draw_power(&mut self, cx: &mut Cx2d, body: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let battery = self.data.battery;
        let (hero, rest) = cut_top(body, 44.0);
        let percent = battery.map(|b| b.percent);
        let status = match battery {
            Some(b) if b.percent >= 100 => "Fully charged",
            Some(b) if b.charging => "Pumping power",
            Some(_) => "On battery",
            None => "No battery",
        };
        self.d.panel_hero(
            cx,
            hero,
            &tok,
            fg,
            Ico::Battery,
            "Battery",
            status,
            90.0,
        );
        let big = match percent {
            Some(p) => format!("{}%", p),
            None => "--".into(),
        };
        self.d.label(
            cx,
            hero,
            true,
            tok.font.display_large,
            fg,
            super::ui::HAlign::Right,
            &big,
        );

        // The charge bar.
        let (bar_row, rest) = cut_top(rest, 16.0);
        let track = rect(
            bar_row.pos.x,
            bar_row.pos.y + 4.0,
            bar_row.size.x,
            8.0,
        );
        self.d.solid(cx, track, alpha(fg, 0.12));
        if let Some(p) = percent {
            self.d.solid(
                cx,
                rect(
                    track.pos.x,
                    track.pos.y,
                    track.size.x * (p as f64 / 100.0),
                    track.size.y,
                ),
                fg,
            );
        }

        // Stats.
        let (sep, rest) = cut_top(rest, tok.spacing.panel_gap);
        self.d.separator(cx, sep, fg, 0.12);
        let mut rest = rest;
        let cycles = self
            .data
            .battery_cycles
            .map(|c| c.to_string())
            .unwrap_or_else(|| "--".into());
        let source = if self.data.power_source.is_empty() {
            "--".to_string()
        } else {
            self.data.power_source.clone()
        };
        for (label, value) in [
            ("Power source", source.as_str()),
            ("Charge cycles", cycles.as_str()),
            (
                "Battery state",
                match battery {
                    Some(b) if b.charging => "Charging",
                    Some(_) => "Discharging",
                    None => "Unavailable",
                },
            ),
        ] {
            let (row, next) = cut_top(rest, 20.0);
            rest = next;
            self.info_pair(cx, row, label, value);
        }

        let (sep, rest) = cut_top(rest, tok.spacing.panel_gap);
        self.d.separator(cx, sep, fg, 0.12);
        let (header, rest) = cut_top(rest, 18.0);
        self.section_header(cx, header, "POWER PROFILE", "");
        let (row, _) = cut_top(rest, tok.spacing.control_height);
        let names = ["Power saver", "Balanced", "Performance"];
        let w = (row.size.x - tok.spacing.md * 2.0) / 3.0;
        for (i, name) in names.iter().enumerate() {
            let cell = rect(
                row.pos.x + i as f64 * (w + tok.spacing.md),
                row.pos.y,
                w,
                row.size.y,
            );
            // No power profiles on this OS: the buttons say so by being
            // disabled rather than by lying about a profile.
            self.d.button(
                cx,
                cell,
                &tok,
                CtrlState::Disabled,
                None,
                name,
                tok.font.body_small,
                fg,
                true,
            );
        }
    }

    // ----------------------------------------------------------- monitor

    #[cfg(not(target_os = "linux"))]
    fn draw_monitor(&mut self, cx: &mut Cx2d, body: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let (hero, rest) = cut_top(body, 40.0);
        self.d
            .panel_hero(cx, hero, &tok, fg, Ico::Monitor, "Display", "Text size and screens", 0.0);

        let (sep, rest) = cut_top(rest, tok.spacing.panel_gap);
        self.d.separator(cx, sep, fg, 0.12);
        let (header, rest) = cut_top(rest, 18.0);
        self.section_header(cx, header, "TEXT SIZE", &format!("{}px", self.data.text_size));
        let (row, rest) = cut_top(rest, 28.0);
        let idx = TEXT_SIZE_STOPS
            .iter()
            .position(|s| *s >= self.data.text_size)
            .unwrap_or(3) as f64;
        self.slider_row(
            cx,
            row,
            Hit::TextSizeSlider,
            idx / (TEXT_SIZE_STOPS.len() - 1) as f64,
            true,
        );

        let (sep, rest) = cut_top(rest, tok.spacing.panel_gap);
        self.d.separator(cx, sep, fg, 0.12);
        let (header, mut rest) = cut_top(rest, 18.0);
        self.section_header(cx, header, "DISPLAYS", "");
        for name in self.data.displays.clone() {
            let (row, next) = cut_top(rest, 26.0);
            rest = next;
            self.d.icon_centered(
                cx,
                Ico::Monitor,
                rect(row.pos.x, row.pos.y, 22.0, row.size.y),
                tok.font.subtitle,
                fg,
            );
            self.d.label_elided(
                cx,
                rect(row.pos.x + 28.0, row.pos.y, row.size.x - 50.0, row.size.y),
                false,
                tok.font.body,
                fg,
                super::ui::HAlign::Left,
                &name,
            );
            self.d.icon_centered(
                cx,
                Ico::Check,
                rect(row.pos.x + row.size.x - 16.0, row.pos.y, 14.0, row.size.y),
                tok.font.subtitle,
                fg,
            );
        }
    }

    fn hit_at(&self, p: Vec2d) -> Option<Hit> {
        self.hits
            .iter()
            .find(|(_, r)| contains(*r, p))
            .map(|(h, _)| *h)
    }

    /// Where a pointer x lands on a slider row, 0..1.
    fn slider_value(&self, hit: Hit, p: Vec2d) -> f64 {
        let Some((_, r)) = self.hits.iter().find(|(h, _)| *h == hit) else {
            return 0.0;
        };
        let track = inset(*r, 6.0);
        #[cfg(target_os = "linux")]
        return ((p.x - track.pos.x - 6.0) / (track.size.x - 12.0).max(1.0)).clamp(0.0, 1.0);
        #[cfg(not(target_os = "linux"))]
        ((p.x - track.pos.x) / track.size.x.max(1.0)).clamp(0.0, 1.0)
    }
}

// ======================================================================
// The Wi-Fi dropdown (Linux) — a compact menu in the macOS mould: the
// radio switch in the hero, the machine's addresses, the networks ranked
// by signal with the connected one on top, and one action row under the
// list (the password prompt, the connect in progress, a notice, or
// Disconnect/Forget for the connected network). Drawn from `self.wifi`
// (shell/wifi_linux.rs), which the WM feeds from the iwd worker.
// ======================================================================

#[cfg(target_os = "linux")]
const WIFI_MAX_ROWS: usize = 7;
#[cfg(target_os = "linux")]
const WIFI_ROW_H: f64 = 28.0;
#[cfg(target_os = "linux")]
const WIFI_ADDR_ROW_H: f64 = 20.0;

// ----------------------------------------------------------------------
// The hotspot section of the Wi-Fi dropdown (Linux): the switch, what it
// costs, and while it runs the join code, the password, the address and
// the devices on it. State and wording come from `self.wifi`
// (shell/wifi_linux.rs); the logic is shell/hotspot_linux.rs.
// ----------------------------------------------------------------------

#[cfg(target_os = "linux")]
const HS_LINE_H: f64 = 16.0;
#[cfg(target_os = "linux")]
const HS_CHARS: usize = 58;
#[cfg(target_os = "linux")]
const HS_QR_PX: f64 = 164.0;
#[cfg(target_os = "linux")]
const HS_BLOCK_H: f64 = 172.0;
#[cfg(target_os = "linux")]
const HS_MAX_CLIENTS: usize = 4;

#[cfg(target_os = "linux")]
impl ShellPanel {
    /// The lines under the switch: what it costs, or why it is off limits.
    fn hotspot_note(&self) -> Vec<String> {
        use super::hotspot_linux::{wrap_text, Capability};
        let h = &self.wifi.snapshot.hotspot;
        match &h.capability {
            Capability::Unsupported(why) if !h.ap_mode => wrap_text(why, HS_CHARS, 3),
            _ if h.ap_mode => Vec::new(),
            _ => wrap_text(
                "Turning it on replaces your Wi-Fi connection: this laptop and the phone have no internet. Turning it off rejoins the network.",
                HS_CHARS,
                3,
            ),
        }
    }

    /// The question being asked, as lines.
    fn hotspot_ask_lines(&self) -> Vec<String> {
        use super::hotspot_linux::wrap_text;
        match self.wifi.hotspot_ask {
            None => Vec::new(),
            Some(HotspotAsk::Enable) => {
                let from = match self.wifi.snapshot.connected_name() {
                    Some(n) => format!("Wi-Fi to {n}"),
                    None => "Wi-Fi".to_string(),
                };
                wrap_text(
                    &format!("Turn on the hotspot? {from} is switched off while it runs, so this laptop has no internet. It comes back when you turn the hotspot off."),
                    HS_CHARS,
                    4,
                )
            }
            Some(HotspotAsk::Regenerate) => wrap_text(
                "Connected devices will be disconnected and must rejoin with the new password.",
                HS_CHARS,
                3,
            ),
        }
    }

    fn hotspot_height(&self) -> f64 {
        let snap = &self.wifi.snapshot;
        if snap.unavailable.is_some() || !self.wifi.have_snapshot {
            return 0.0;
        }
        let h = &snap.hotspot;
        let mut total = self.tokens.spacing.panel_gap + 18.0 + 28.0 + self.hotspot_note().len() as f64 * HS_LINE_H;
        let ask = self.hotspot_ask_lines().len();
        if ask > 0 {
            total += 6.0 + ask as f64 * HS_LINE_H + 6.0 + 28.0;
        }
        if h.ap_mode {
            total += 6.0 + HS_BLOCK_H + 6.0 + 20.0 + 20.0 + h.clients.len().min(HS_MAX_CLIENTS) as f64 * 18.0;
        }
        total
    }

    fn draw_hotspot(&mut self, cx: &mut Cx2d, rest: Rect) {
        let snap = self.wifi.snapshot.clone();
        if snap.unavailable.is_some() || !self.wifi.have_snapshot {
            return;
        }
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let accent = tok.bar.active;
        let px = tok.font.body_small;
        let md = tok.spacing.md;
        let h = snap.hotspot.clone();
        let usable = self.wifi.hotspot_usable();
        let waiting = self.wifi.hotspot_wait.is_some();

        let (sep, rest) = cut_top(rest, tok.spacing.panel_gap);
        self.d.separator(cx, sep, fg, 0.12);
        let (header, rest) = cut_top(rest, 18.0);
        self.section_header(cx, header, "HOTSPOT", "");

        // ---- the switch row: the network name, its state, the switch
        let (row, rest) = cut_top(rest, 28.0);
        let switch = self.d.toggle_switch(
            cx,
            dvec2(row.pos.x + row.size.x - 42.0, row.pos.y + (row.size.y - 22.0) * 0.5),
            &tok,
            h.ap_mode,
            if usable { fg } else { alpha(fg, 0.4) },
        );
        let state = if waiting {
            "Working\u{2026}"
        } else if h.started {
            "On"
        } else if h.ap_mode {
            "Starting\u{2026}"
        } else {
            "Off"
        };
        let sw = self.d.measure(cx, false, px, state);
        self.d.label(
            cx,
            rect(switch.pos.x - md - sw, row.pos.y, sw, row.size.y),
            false,
            px,
            dim,
            super::ui::HAlign::Right,
            state,
        );
        self.d.label_elided(
            cx,
            rect(row.pos.x + 4.0, row.pos.y, (switch.pos.x - md - sw - 12.0 - row.pos.x).max(0.0), row.size.y),
            h.ap_mode,
            tok.font.body,
            if usable { fg } else { alpha(fg, 0.5) },
            super::ui::HAlign::Left,
            &h.ssid,
        );
        if usable && !waiting {
            self.hits.push((Hit::Wifi(WifiHit::HotspotToggle), row));
        }

        // ---- what it costs, or why it cannot be used
        let mut rest = rest;
        for line in self.hotspot_note() {
            let (r, next) = cut_top(rest, HS_LINE_H);
            rest = next;
            self.d.label_elided(cx, r, false, tok.font.caption, dim, super::ui::HAlign::Left, &line);
        }

        // ---- the question
        let ask = self.hotspot_ask_lines();
        if !ask.is_empty() {
            let yes = match self.wifi.hotspot_ask {
                Some(HotspotAsk::Regenerate) => "Regenerate",
                _ => "Turn on",
            };
            let (_, next) = cut_top(rest, 6.0);
            rest = next;
            for line in &ask {
                let (r, next) = cut_top(rest, HS_LINE_H);
                rest = next;
                self.d.label_elided(cx, r, false, tok.font.caption, fg, super::ui::HAlign::Left, line);
            }
            let (_, next) = cut_top(rest, 6.0);
            let (brow, next) = cut_top(next, 28.0);
            rest = next;
            let cw = self.d.button_width(cx, &tok, false, "Cancel", px).max(72.0);
            let cancel = rect(brow.pos.x + brow.size.x - cw, brow.pos.y, cw, brow.size.y);
            let yw = self.d.button_width(cx, &tok, false, yes, px).max(96.0);
            let confirm = rect(cancel.pos.x - md - yw, brow.pos.y, yw, brow.size.y);
            self.wifi_button(cx, confirm, WifiHit::HotspotConfirm, yes, false, fg);
            self.wifi_button(cx, cancel, WifiHit::HotspotCancel, "Cancel", false, fg);
        }

        if !h.ap_mode {
            return;
        }

        // ---- the join code, the network, the password
        let (_, next) = cut_top(rest, 6.0);
        let (block, next) = cut_top(next, HS_BLOCK_H);
        rest = next;
        let mut right_x = block.pos.x;
        if let Some((_, qr)) = self.wifi.hotspot_qr.as_ref() {
            let cells = (qr.size + 8) as f64;
            let m = (HS_QR_PX / cells).floor().max(1.0);
            let side = m * cells;
            let origin = dvec2(block.pos.x, block.pos.y + (HS_BLOCK_H - side) * 0.5);
            let white = vec4(1.0, 1.0, 1.0, 1.0);
            let black = vec4(0.0, 0.0, 0.0, 1.0);
            self.d.solid(cx, rect(origin.x, origin.y, side, side), white);
            for y in 0..qr.size {
                let mut x = 0;
                while x < qr.size {
                    if !qr.get(x, y) {
                        x += 1;
                        continue;
                    }
                    let start = x;
                    while x < qr.size && qr.get(x, y) {
                        x += 1;
                    }
                    self.d.solid(
                        cx,
                        rect(
                            origin.x + (start + 4) as f64 * m,
                            origin.y + (y + 4) as f64 * m,
                            (x - start) as f64 * m,
                            m,
                        ),
                        black,
                    );
                }
            }
            right_x = block.pos.x + side + 14.0;
        }
        let col = rect(right_x, block.pos.y, (block.pos.x + block.size.x - right_x).max(0.0), block.size.y);
        let (r, col_rest) = cut_top(col, 14.0);
        self.d.label(cx, r, false, tok.font.caption, dim, super::ui::HAlign::Left, "NETWORK");
        let (r, col_rest) = cut_top(col_rest, 18.0);
        self.d.label_elided(cx, r, true, px, fg, super::ui::HAlign::Left, &h.ssid);
        let (_, col_rest) = cut_top(col_rest, 4.0);
        let (r, mut col_rest) = cut_top(col_rest, 14.0);
        self.d.label(cx, r, false, tok.font.caption, dim, super::ui::HAlign::Left, "PASSWORD");
        match h.passphrase.as_deref() {
            Some(pass) => {
                let words: Vec<&str> = pass.split('-').collect();
                for (i, w) in words.iter().enumerate() {
                    let (r, next) = cut_top(col_rest, 22.0);
                    col_rest = next;
                    let text = if i + 1 < words.len() { format!("{w}-") } else { (*w).to_string() };
                    self.d.label_elided(cx, r, true, 18.0, fg, super::ui::HAlign::Left, &text);
                }
                let (_, next) = cut_top(col_rest, 6.0);
                let (r, _) = cut_top(next, 28.0);
                let bw = self.d.button_width(cx, &tok, false, "Regenerate", px).max(96.0);
                let btn = rect(r.pos.x, r.pos.y, bw.min(r.size.x), r.size.y);
                self.wifi_button(cx, btn, WifiHit::HotspotRegenerate, "Regenerate", waiting, fg);
            }
            None => {
                let (r, _) = cut_top(col_rest, 22.0);
                self.d.label_elided(cx, r, false, px, accent, super::ui::HAlign::Left, "No password stored");
            }
        }

        // ---- address and devices
        let (r, next) = cut_top(rest, 20.0);
        rest = next;
        self.info_pair(cx, r, "Address", h.ip.as_deref().unwrap_or("waiting for an address"));
        let (r, next) = cut_top(rest, 20.0);
        rest = next;
        self.info_pair(cx, r, "Devices", &h.clients.len().to_string());
        for c in h.clients.iter().take(HS_MAX_CLIENTS) {
            let (r, next) = cut_top(rest, 18.0);
            rest = next;
            self.d.label(cx, r, false, tok.font.caption, dim, super::ui::HAlign::Left, &c.ip);
            self.d.label(cx, r, false, tok.font.caption, dim, super::ui::HAlign::Right, &c.mac);
        }
    }
}


#[cfg(target_os = "linux")]
impl ShellPanel {
    /// The action row's height for the current state.
    fn wifi_action_height(&self) -> f64 {
        if self.wifi.entry.is_some() {
            return 18.0 + 28.0 + 6.0 + 28.0 + 18.0;
        }
        match self.wifi.phase {
            WifiPhase::Connecting { .. } | WifiPhase::Notice { .. } => 6.0 + 28.0,
            WifiPhase::Idle => {
                if self.wifi.snapshot.connected.is_some() {
                    6.0 + 28.0
                } else {
                    0.0
                }
            }
        }
    }

    /// Hero, the addresses, the network list (capped, then it scrolls)
    /// and whatever action row is up.
    fn network_height(&self) -> f64 {
        let gap = self.tokens.spacing.panel_gap;
        let snap = &self.wifi.snapshot;
        let addr_rows = snap.addresses.len().max(1) as f64;
        let rows = snap.networks.len().clamp(1, WIFI_MAX_ROWS) as f64;
        40.0 + gap
            + 18.0
            + addr_rows * WIFI_ADDR_ROW_H
            + gap
            + 18.0
            + rows * WIFI_ROW_H
            + self.wifi_action_height()
            + self.hotspot_height()
    }

    fn draw_wifi(&mut self, cx: &mut Cx2d, body: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let snap = self.wifi.snapshot.clone();
        let scanning = self.wifi.scanning();
        let usable = snap.unavailable.is_none() && self.wifi.have_snapshot;

        // ---- hero: the glyph, "Wi-Fi", the state line, the radio switch
        let (hero, rest) = cut_top(body, 40.0);
        let mut trailing = 0.0;
        if usable && snap.radio != RadioState::NoDevice {
            let switch = self.d.toggle_switch(
                cx,
                dvec2(
                    hero.pos.x + hero.size.x - 42.0,
                    hero.pos.y + (hero.size.y - 22.0) * 0.5,
                ),
                &tok,
                snap.radio == RadioState::On,
                fg,
            );
            self.hits.push((Hit::Wifi(WifiHit::Power), switch));
            trailing = switch.size.x + 12.0;
        }
        let meta = if let Some(reason) = &snap.unavailable {
            reason.clone()
        } else if !self.wifi.have_snapshot {
            "Starting".to_string()
        } else {
            match snap.radio {
                RadioState::NoDevice => "No Wi-Fi device".to_string(),
                RadioState::Off => "Off".to_string(),
                RadioState::On if snap.hotspot.ap_mode => format!("Hotspot {}", if snap.hotspot.started { "on" } else { "starting\u{2026}" }),
                RadioState::On => match snap.state {
                    StationState::Connected => {
                        format!("Connected to {}", snap.connected_name().unwrap_or("network"))
                    }
                    StationState::Connecting => "Connecting\u{2026}".to_string(),
                    StationState::Disconnecting => "Disconnecting\u{2026}".to_string(),
                    StationState::Roaming => "Roaming\u{2026}".to_string(),
                    StationState::Disconnected | StationState::Unknown => {
                        if scanning {
                            "Scanning\u{2026}".to_string()
                        } else {
                            "Not connected".to_string()
                        }
                    }
                },
            }
        };
        let ico = if usable && snap.radio == RadioState::On {
            Ico::Wifi
        } else {
            Ico::WifiOff
        };
        self.d
            .panel_hero(cx, hero, &tok, fg, ico, "Wi-Fi", &meta, trailing);

        // ---- the machine's own addresses: wired ones even with Wi-Fi off
        let (sep, rest) = cut_top(rest, tok.spacing.panel_gap);
        self.d.separator(cx, sep, fg, 0.12);
        let (header, mut rest) = cut_top(rest, 18.0);
        self.section_header(cx, header, "IP ADDRESSES", "");
        if snap.addresses.is_empty() {
            let (row, next) = cut_top(rest, WIFI_ADDR_ROW_H);
            rest = next;
            self.unavailable(cx, row, "No IPv4 address on any interface");
        }
        for a in &snap.addresses {
            let (row, next) = cut_top(rest, WIFI_ADDR_ROW_H);
            rest = next;
            self.info_pair(cx, row, &format!("{} \u{b7} {}", a.kind(), a.name), &a.ipv4);
        }

        // ---- the networks
        let (sep, rest) = cut_top(rest, tok.spacing.panel_gap);
        self.d.separator(cx, sep, fg, 0.12);
        let (header, mut rest) = cut_top(rest, 18.0);
        self.section_header(
            cx,
            header,
            "NETWORKS",
            if scanning { "SCANNING\u{2026}" } else { "" },
        );
        if !scanning && usable && snap.radio == RadioState::On {
            let slot = rect(
                header.pos.x + header.size.x - 18.0,
                header.pos.y,
                18.0,
                header.size.y,
            );
            let hot = self.hot == Some(Hit::Wifi(WifiHit::Refresh));
            self.d
                .icon_centered(cx, Ico::Refresh, slot, 12.0, if hot { fg } else { dim });
            self.hits.push((Hit::Wifi(WifiHit::Refresh), slot));
        }
        let status_line: Option<String> = if let Some(reason) = &snap.unavailable {
            Some(reason.clone())
        } else if !self.wifi.have_snapshot {
            Some("Waiting for iwd\u{2026}".to_string())
        } else {
            match snap.radio {
                RadioState::NoDevice => Some("No Wi-Fi device found".to_string()),
                RadioState::Off => Some("Wi-Fi is off".to_string()),
                RadioState::On if snap.hotspot.ap_mode => Some("Wi-Fi is off while the hotspot is on".to_string()),
                RadioState::On if snap.networks.is_empty() => Some(
                    if scanning {
                        "Scanning\u{2026}"
                    } else {
                        "No networks found"
                    }
                    .to_string(),
                ),
                RadioState::On => None,
            }
        };
        if let Some(line) = status_line {
            let (row, next) = cut_top(rest, WIFI_ROW_H);
            rest = next;
            self.unavailable(cx, row, &line);
        } else {
            let entry_path = self.wifi.entry.as_ref().map(|e| e.path.clone());
            let first = self
                .wifi
                .scroll
                .min(snap.networks.len().saturating_sub(WIFI_MAX_ROWS));
            for (i, net) in snap.networks.iter().enumerate().skip(first).take(WIFI_MAX_ROWS) {
                let (row, next) = cut_top(rest, WIFI_ROW_H);
                rest = next;
                let hit = Hit::Wifi(WifiHit::Row(i));
                let hot = self.hot == Some(hit);
                let current =
                    net.connected || entry_path.as_deref() == Some(net.path.as_str());
                self.d.cursor_surface(cx, row, &tok.controls, hot, current);
                self.wifi_bars(
                    cx,
                    rect(row.pos.x + 4.0, row.pos.y, 18.0, row.size.y),
                    net.bars(),
                    fg,
                );
                let mut right = row.pos.x + row.size.x - 4.0;
                if net.connected {
                    let r = rect(right - 14.0, row.pos.y, 14.0, row.size.y);
                    self.d.icon_centered(cx, Ico::Check, r, 14.0, fg);
                    right -= 18.0;
                }
                match net.security {
                    Security::Psk => {
                        let r = rect(right - 12.0, row.pos.y, 12.0, row.size.y);
                        self.d.icon_centered(cx, Ico::Lock, r, 11.0, dim);
                        right -= 16.0;
                    }
                    Security::Enterprise | Security::Wep | Security::Unknown => {
                        let label = net.security.label();
                        let w = self.d.measure(cx, false, tok.font.caption, label);
                        self.d.label(
                            cx,
                            rect(right - w, row.pos.y, w, row.size.y),
                            false,
                            tok.font.caption,
                            dim,
                            super::ui::HAlign::Right,
                            label,
                        );
                        right -= w + 6.0;
                    }
                    Security::Open => {}
                }
                let name_x = row.pos.x + 4.0 + 18.0 + 8.0;
                self.d.label_elided(
                    cx,
                    rect(name_x, row.pos.y, (right - name_x - 6.0).max(0.0), row.size.y),
                    net.connected,
                    tok.font.body,
                    fg,
                    super::ui::HAlign::Left,
                    &net.name,
                );
                self.hits.push((hit, row));
            }
        }

        let (action, after) = cut_top(rest, self.wifi_action_height());
        self.draw_wifi_action(cx, action);
        self.draw_hotspot(cx, after);
    }

    /// Four signal bars, `lit` of them in the foreground.
    fn wifi_bars(&mut self, cx: &mut Cx2d, slot: Rect, lit: u8, fg: Vec4f) {
        let heights = [4.0, 7.0, 10.0, 13.0];
        let bar_w = 3.0;
        let gap = 1.0;
        let base = slot.pos.y + (slot.size.y + 13.0) * 0.5;
        for (i, h) in heights.iter().enumerate() {
            let x = slot.pos.x + i as f64 * (bar_w + gap);
            let color = if (i as u8) < lit { fg } else { alpha(fg, 0.2) };
            self.d.solid(cx, rect(x, base - h, bar_w, *h), color);
        }
    }

    /// A bordered `body_small` button that records its hit unless disabled.
    fn wifi_button(
        &mut self,
        cx: &mut Cx2d,
        r: Rect,
        hit: WifiHit,
        label: &str,
        disabled: bool,
        fg: Vec4f,
    ) {
        let tok = self.tokens;
        let state = if disabled {
            CtrlState::Disabled
        } else if self.hot == Some(Hit::Wifi(hit)) {
            CtrlState::Hover
        } else {
            CtrlState::Normal
        };
        self.d
            .button(cx, r, &tok, state, None, label, tok.font.body_small, fg, true);
        if !disabled {
            self.hits.push((Hit::Wifi(hit), r));
        }
    }

    /// The row under the list: the password prompt, the connect in
    /// progress, a notice, or the connected network's Disconnect/Forget.
    fn draw_wifi_action(&mut self, cx: &mut Cx2d, rest: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let accent = tok.bar.active;
        let px = tok.font.body_small;
        let md = tok.spacing.md;

        if let Some(entry) = self.wifi.entry.as_ref() {
            let name = entry.name.clone();
            let filled = entry.secret.len_chars();
            let ready = entry.secret.problem().is_none();
            let hint = entry.hint.clone();
            let (cap, rest) = cut_top(rest, 18.0);
            self.section_header(cx, cap, &format!("PASSWORD FOR {}", name.to_uppercase()), "");
            let (field, rest) = cut_top(rest, 28.0);
            // The field draws bullets only: the text never leaves the Secret.
            let masked: String = std::iter::repeat('\u{2022}').take(filled).collect();
            let hot = self.hot == Some(Hit::Wifi(WifiHit::Field));
            self.d
                .text_field(cx, field, &tok, &masked, "Password", true, hot, fg);
            self.hits.push((Hit::Wifi(WifiHit::Field), field));
            let (_, rest) = cut_top(rest, 6.0);
            let (row, rest) = cut_top(rest, 28.0);
            let connect_w = self.d.button_width(cx, &tok, false, "Connect", px).max(84.0);
            let cancel_w = self.d.button_width(cx, &tok, false, "Cancel", px).max(72.0);
            let connect = rect(row.pos.x + row.size.x - connect_w, row.pos.y, connect_w, row.size.y);
            let cancel = rect(connect.pos.x - md - cancel_w, row.pos.y, cancel_w, row.size.y);
            self.wifi_button(cx, connect, WifiHit::Connect, "Connect", !ready, fg);
            self.wifi_button(cx, cancel, WifiHit::Cancel, "Cancel", false, fg);
            let (hint_row, _) = cut_top(rest, 18.0);
            match hint {
                Some(h) => self.d.label_elided(
                    cx,
                    hint_row,
                    false,
                    tok.font.caption,
                    accent,
                    super::ui::HAlign::Left,
                    &h,
                ),
                None => self.d.label_elided(
                    cx,
                    hint_row,
                    false,
                    tok.font.caption,
                    dim,
                    super::ui::HAlign::Left,
                    "8\u{2013}63 characters \u{b7} Enter connects, Esc cancels",
                ),
            }
            return;
        }

        let phase_row = match &self.wifi.phase {
            WifiPhase::Connecting { name, .. } => {
                Some((format!("Connecting to {name}\u{2026}"), false, "Cancel", WifiHit::Cancel))
            }
            WifiPhase::Notice { text, error } => Some((text.clone(), *error, "OK", WifiHit::Dismiss)),
            WifiPhase::Idle => None,
        };
        if let Some((text, error, button, hit)) = phase_row {
            let (_, rest) = cut_top(rest, 6.0);
            let (row, _) = cut_top(rest, 28.0);
            let bw = self.d.button_width(cx, &tok, false, button, px).max(64.0);
            let btn = rect(row.pos.x + row.size.x - bw, row.pos.y, bw, row.size.y);
            self.d.label_elided(
                cx,
                rect(row.pos.x, row.pos.y, (row.size.x - bw - md).max(0.0), row.size.y),
                false,
                px,
                if error { accent } else { fg },
                super::ui::HAlign::Left,
                &text,
            );
            self.wifi_button(cx, btn, hit, button, false, fg);
            return;
        }

        if self.wifi.snapshot.connected.is_some() {
            let known = self
                .wifi
                .snapshot
                .networks
                .iter()
                .find(|n| n.connected)
                .and_then(|n| n.known.clone());
            let (_, rest) = cut_top(rest, 6.0);
            let (row, _) = cut_top(rest, 28.0);
            let dw = self.d.button_width(cx, &tok, false, "Disconnect", px).max(96.0);
            let disc = rect(row.pos.x + row.size.x - dw, row.pos.y, dw, row.size.y);
            self.wifi_button(cx, disc, WifiHit::Disconnect, "Disconnect", false, fg);
            if known.is_some() {
                let fw = self.d.button_width(cx, &tok, false, "Forget", px).max(72.0);
                let forget = rect(disc.pos.x - md - fw, row.pos.y, fw, row.size.y);
                self.wifi_button(cx, forget, WifiHit::Forget, "Forget", false, fg);
            }
        }
    }

    /// A press on one of the dropdown's targets.
    fn wifi_press(&mut self, cx: &mut Cx, hit: WifiHit) {
        let mut command = None;
        match hit {
            WifiHit::Power => {
                let on = self.wifi.snapshot.radio == RadioState::On;
                self.wifi.dismiss_notice();
                command = Some(WifiCommand::SetPowered(!on));
            }
            WifiHit::Refresh => {
                self.wifi.scan_requested_at = Some(Cx::monotonic_now());
                self.wifi.dismiss_notice();
                command = Some(WifiCommand::Scan);
            }
            WifiHit::Row(i) => command = self.wifi.activate_row(i),
            WifiHit::Field => {}
            WifiHit::Connect => command = self.wifi.submit(),
            WifiHit::Cancel => {
                if !self.wifi.cancel_entry()
                    && matches!(self.wifi.phase, WifiPhase::Connecting { .. })
                {
                    self.wifi.phase = WifiPhase::Idle;
                    command = Some(WifiCommand::CancelConnect);
                }
            }
            WifiHit::Disconnect => {
                self.wifi.phase = WifiPhase::Idle;
                command = Some(WifiCommand::Disconnect);
            }
            WifiHit::Forget => {
                let known = self
                    .wifi
                    .snapshot
                    .networks
                    .iter()
                    .find(|n| n.connected)
                    .and_then(|n| n.known.clone());
                if let Some(known) = known {
                    command = Some(WifiCommand::Forget { known });
                }
            }
            WifiHit::Dismiss => self.wifi.dismiss_notice(),
            WifiHit::HotspotToggle => command = self.wifi.hotspot_toggle(),
            WifiHit::HotspotRegenerate => self.wifi.hotspot_regenerate_ask(),
            WifiHit::HotspotConfirm => command = self.wifi.hotspot_confirm(),
            WifiHit::HotspotCancel => self.wifi.hotspot_cancel(),
        }
        if let Some(cmd) = command {
            cx.widget_action(self.uid, ShellPanelAction::Wifi(cmd));
        }
        self.redraw(cx);
    }

    /// The password prompt is up: every key and every piece of typed text
    /// is the prompt's, nothing reaches a tile underneath.
    pub fn secret_input_active(&self) -> bool {
        self.open == Some(PanelKind::Network) && self.wifi.entering()
    }

    /// The keys the flyout takes before the WM binds, the AI pane and the
    /// tiles (lib.rs routes them here first): the Wi-Fi prompt's, and
    /// Escape while a Display picker or list is unfolded or a restart asks
    /// to confirm.
    pub fn wants_keys(&self) -> bool {
        self.open == Some(PanelKind::Network)
            || self.chooser.is_some()
            || self.pace_open
            || self.dpi_drag.is_some()
            || self.restart_confirm
            || self.gpu_picker

            || self.pointer_settings
            || self.swallow_escape_up
    }

    /// The key-up half of a consumed Escape, and every key-up while the
    /// password prompt is up. True when consumed.
    pub fn key_up(&mut self, e: &KeyEvent) -> bool {
        if self.swallow_escape_up && e.key_code == KeyCode::Escape {
            self.swallow_escape_up = false;
            return true;
        }
        self.secret_input_active()
    }

    /// The keyboard while a flyout wants it: with the Wi-Fi dropdown up
    /// the prompt owns the keys (`WifiUi::key`), otherwise Escape folds an
    /// unfolded Display picker or closes the dropdown. True when the key
    /// was consumed.
    pub fn key(&mut self, cx: &mut Cx, e: &KeyEvent) -> bool {
        // Escape during a scale drag puts the scale back where it was.
        if e.key_code == KeyCode::Escape {
            if let Some(drag) = self.dpi_drag.take() {
                self.dragging = None;
                self.system_action(cx, crate::linux_controls::ControlAction::DpiScaleLive(drag.start));
                self.swallow_escape_up = true;
                return true;
            }
        }
        // Escape folds a screen's picker, or drops a restart confirm; the
        // Display panel stays.
        if e.key_code == KeyCode::Escape && (self.chooser.is_some() || self.pace_open || self.restart_confirm) {
            self.close_chooser();
            self.restart_confirm = false;
            self.swallow_escape_up = true;
            self.redraw(cx);
            return true;
        }
        // Same for the compositor / app GPU list.
        if e.key_code == KeyCode::Escape && (self.gpu_picker) {
            self.gpu_picker = false;
            self.gpu_scroll = 0;
            self.gpu_targets.clear();
            self.swallow_escape_up = true;
            self.redraw(cx);
            return true;
        }
        if e.key_code == KeyCode::Escape && self.pointer_settings {
            self.pointer_settings = false;
            self.dragging = None;
            self.swallow_escape_up = true;
            self.redraw(cx);
            return true;
        }
        if self.open != Some(PanelKind::Network) {
            return false;
        }
        if self.wifi.entering() {
            match self.wifi.key(e) {
                KeyOutcome::Ignored => return false,
                KeyOutcome::Consumed => {}
                KeyOutcome::Submit => {
                    if let Some(cmd) = self.wifi.submit() {
                        cx.widget_action(self.uid, ShellPanelAction::Wifi(cmd));
                    }
                }
                KeyOutcome::Cancel => {
                    self.wifi.cancel_entry();
                }
            }
            self.redraw(cx);
            return true;
        }
        if e.key_code == KeyCode::Escape && !e.modifiers.logo {
            self.close(cx);
            cx.widget_action(self.uid, ShellPanelAction::Close);
            return true;
        }
        false
    }

    /// Typed text for the password prompt. True when it was consumed.
    pub fn wifi_text_input(&mut self, cx: &mut Cx, input: &str) -> bool {
        if !self.secret_input_active() {
            return false;
        }
        self.wifi.text_input(input);
        self.redraw(cx);
        true
    }

    /// The network list scrolls with the wheel once it is longer than the
    /// dropdown shows.
    pub fn wants_scroll(&self, p: Vec2d) -> bool {
        (self.open == Some(PanelKind::Audio) && self.device_picker.is_some() && contains(self.card, p))
            || (self.open == Some(PanelKind::Monitor) && self.gpu_picker && self.gpu_overflow && contains(self.card, p))
            || (self.open == Some(PanelKind::Network)
            && contains(self.card, p)
            && self.wifi.snapshot.networks.len() > WIFI_MAX_ROWS)
    }

    fn wifi_scroll(&mut self, cx: &mut Cx, dy: f64) {
        let max = self.wifi.snapshot.networks.len().saturating_sub(WIFI_MAX_ROWS);
        if dy > 0.5 {
            self.wifi.scroll = (self.wifi.scroll + 1).min(max);
        } else if dy < -0.5 {
            self.wifi.scroll = self.wifi.scroll.saturating_sub(1);
        }
        self.redraw(cx);
    }
}

#[cfg(not(target_os = "linux"))]
impl ShellPanel {
    fn audio_height(&self) -> f64 { 300.0 }
    fn monitor_height(&self) -> f64 { 240.0 }
    fn network_height(&self) -> f64 {
        150.0
    }

    pub fn wants_scroll(&self, _p: Vec2d) -> bool {
        false
    }
}

impl Widget for ShellPanel {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let window = cx.turtle().rect();
        let screen = self.target.unwrap_or(window);
        self.draw_surface_in(cx, window, screen);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.inert || self.open.is_none() {
            return;
        }
        if self.glow_frame.is_event(event).is_some() {
            self.redraw(cx);
        }
        match event {
            Event::MouseMove(e) => {
                if let Some(hit) = self.dragging {
                    let v = self.slider_value(hit, e.abs);
                    match hit {
                        #[cfg(target_os = "linux")]
                        Hit::InputSlider => self.system_action(cx, crate::linux_controls::ControlAction::InputVolume((v * 100.0).round() as u32)),
                        Hit::VolumeSlider => cx.widget_action(
                            self.uid,
                            ShellPanelAction::SetVolume((v * 100.0).round() as u32),
                        ),
                        #[cfg(target_os = "linux")]
                        Hit::PointerSpeedSlider(touchpad) => {
                            self.system_action(cx, crate::linux_controls::ControlAction::PointerSpeed {
                                touchpad,
                                value: Self::pointer_from_slider(v),
                            });
                        }
                        #[cfg(target_os = "linux")]
                        Hit::DpiScaleSlider => self.dpi_drag_to(cx, e.abs),
                        _ => {}
                    }
                    return;
                }
                let hot = self.hit_at(e.abs);
                if hot != self.hot {
                    self.hot = hot;
                    self.redraw(cx);
                }
            }
            Event::MouseDown(e) => {
                if !contains(self.card, e.abs) {
                    // A click outside closes the flyout.
                    self.close(cx);
                    cx.widget_action(self.uid, ShellPanelAction::Close);
                    return;
                }
                match self.hit_at(e.abs) {
                    #[cfg(target_os = "linux")]
                    Some(Hit::Wifi(hit)) => self.wifi_press(cx, hit),
                    Some(Hit::VolumeSlider) => {
                        self.dragging = Some(Hit::VolumeSlider);
                        let v = self.slider_value(Hit::VolumeSlider, e.abs);
                        cx.widget_action(
                            self.uid,
                            ShellPanelAction::SetVolume((v * 100.0).round() as u32),
                        );
                    }
                    Some(Hit::TextSizeSlider) => {
                        let v = self.slider_value(Hit::TextSizeSlider, e.abs);
                        let i = ((v * (TEXT_SIZE_STOPS.len() - 1) as f64).round() as usize)
                            .min(TEXT_SIZE_STOPS.len() - 1);
                        cx.widget_action(self.uid, ShellPanelAction::SetTextSize(TEXT_SIZE_STOPS[i]));
                    }
                    // The input level is read-only here: this OS has no
                    // cheap way to set it, so the row shows and does not lie.
                    Some(Hit::InputSlider) => {
                        #[cfg(target_os = "linux")]
                        {
                            self.dragging = Some(Hit::InputSlider);
                            let v = self.slider_value(Hit::InputSlider, e.abs);
                            self.system_action(cx, crate::linux_controls::ControlAction::InputVolume((v * 100.0).round() as u32));
                        }
                    }
                    #[cfg(target_os = "linux")]
                    Some(hit @ (Hit::DevicePicker(_) | Hit::Device(_, _) | Hit::InputMute
                        | Hit::Arrangement(_) | Hit::OptimiseFor(_)
                        | Hit::ScreenMove(..) | Hit::ScreenMain(_) | Hit::ScreenResolution(_) | Hit::ScreenRate(_)
                        | Hit::ChooserItem(_) | Hit::TrialKeep | Hit::TrialRevert | Hit::PacePicker | Hit::PaceItem(_)
                        | Hit::RestartDesktop | Hit::RestartConfirm | Hit::RestartCancel
                        | Hit::GpuPicker
                        | Hit::Gpu(_) | Hit::PointerSettings)) => self.system_press(cx, hit),
                    #[cfg(target_os = "linux")]
                    Some(Hit::DpiScaleSlider) => self.dpi_drag_begin(cx, e.abs),
                    #[cfg(target_os = "linux")]
                    Some(hit @ Hit::PointerSpeedSlider(touchpad)) => {
                        self.dragging = Some(hit);
                        let v = self.slider_value(hit, e.abs);
                        self.system_action(cx, crate::linux_controls::ControlAction::PointerSpeed {
                            touchpad,
                            value: Self::pointer_from_slider(v),
                        });
                    }
                    Some(Hit::HubModel(i)) => {
                        use crate::hub_control::{HubCommand, ModelState};
                        if let Some(m) = self.data.hub.models.get(i) {
                            let cmd = match m.state {
                                ModelState::Loaded => Some(HubCommand::Unload(m.id.clone())),
                                ModelState::Unloaded | ModelState::Error => Some(HubCommand::Load(m.id.clone())),
                                ModelState::Loading | ModelState::Downloading => None,
                            };
                            if let Some(cmd) = cmd {
                                cx.widget_action(self.uid, ShellPanelAction::Hub(cmd));
                            }
                        }
                    }
                    Some(Hit::HubContext(up)) => {
                        let hub = &self.data.hub;
                        let now = hub.context;
                        let next = if up { hub_context_up(hub) } else { hub.context_options.iter().rev().copied().find(|c| *c < now) };
                        if let Some(c) = next {
                            cx.widget_action(self.uid, ShellPanelAction::Hub(crate::hub_control::HubCommand::SetContext(c)));
                        }
                    }
                    Some(Hit::HubRestart) => {
                        cx.widget_action(self.uid, ShellPanelAction::Hub(crate::hub_control::HubCommand::Restart));
                    }
                    Some(Hit::HubPickOpen) => {
                        self.hub_pick = !self.hub_pick;
                        self.redraw(cx);
                    }
                    Some(Hit::HubPick(i)) => {
                        let picked = hub_chat_models(&self.data.hub).get(i).cloned();
                        self.hub_pick = false;
                        if let Some(model) = picked.filter(|m| self.data.hub.process.as_ref().is_some_and(|p| p.model != *m)) {
                            cx.widget_action(self.uid, ShellPanelAction::Hub(crate::hub_control::HubCommand::SetModel(model)));
                        }
                        self.redraw(cx);
                    }
                    Some(Hit::HubKv(q8)) => {
                        let kv = if q8 { crate::hub_control::KvPrecision::Q8 } else { crate::hub_control::KvPrecision::F16 };
                        cx.widget_action(self.uid, ShellPanelAction::Hub(crate::hub_control::HubCommand::SetKv(kv)));
                    }
                    Some(Hit::MuteToggle) => {
                        cx.widget_action(self.uid, ShellPanelAction::ToggleMute);
                    }
                    Some(Hit::PrevMonth) => {
                        let (y, m) = self.data.view;
                        self.data.view = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
                        self.redraw(cx);
                    }
                    Some(Hit::NextMonth) => {
                        let (y, m) = self.data.view;
                        self.data.view = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
                        self.redraw(cx);
                    }
                    Some(Hit::Today) => {
                        let (y, m, _) = self.data.today;
                        self.data.view = (y, m);
                        self.redraw(cx);
                    }
                    None => {}
                }
            }
            Event::MouseUp(_) => {
                #[cfg(target_os = "linux")]
                self.dpi_drag_end(cx);
                self.dragging = None;
            }
            #[cfg(target_os = "linux")]
            Event::Scroll(e) => {
                if self.wants_scroll(e.abs) {
                    if self.open == Some(PanelKind::Audio) { self.device_scroll_by(cx, e.scroll.y); }
                    else if self.open == Some(PanelKind::Monitor) {
                        // The unfolded Render on list is the one thing that can overflow.
                        self.gpu_scroll_by(cx, e.scroll.y);
                    }
                    else { self.wifi_scroll(cx, e.scroll.y); }
                }
            }
            _ => {}
        }
    }

    /// What a remote snapshot (`/snap`) reads off the open flyout: the
    /// Wi-Fi dropdown's nonsecret state, so a proof can see its rows and
    /// prompt without a picture. A typed password is never part of it.
    #[cfg(target_os = "linux")]
    fn text(&self) -> String {
        match self.open {
            Some(PanelKind::Network) => self.wifi.summary(),
            Some(PanelKind::Audio | PanelKind::Monitor | PanelKind::Power) => self.system_summary(),
            Some(kind) => format!("panel={:?}", kind).to_lowercase(),
            None => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(days_from_civil(2026, 8, 27)), (2026, 8, 27));
        // 2026-08-27 is a Thursday (weekday 3 with Monday = 0).
        assert_eq!(weekday(2026, 8, 27), 3);
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2025, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
    }

    #[test]
    fn iso_weeks_match_the_calendar() {
        // 2026-01-01 is a Thursday, so it is week 1 of 2026.
        assert_eq!(iso_week(2026, 1, 1), 1);
        // 2021-01-01 is a Friday: still week 53 of 2020.
        assert_eq!(iso_week(2021, 1, 1), 53);
        assert_eq!(iso_week(2026, 12, 28), 53);
    }

    #[test]
    fn moods_match_the_thresholds() {
        let mut d = PanelData::fixture();
        d.volume = Some(62);
        assert_eq!(d.volume_mood(), "Steady groove");
        d.volume = Some(100);
        assert_eq!(d.volume_mood(), "Concert hall");
        d.muted = true;
        assert_eq!(d.volume_mood(), "Muted");
        d.muted = false;
        d.volume = Some(0);
        assert_eq!(d.volume_mood(), "Silenced");
    }

    #[test]
    fn panels_bind_to_their_bar_modules() {
        assert_eq!(PanelKind::for_module(BarModule::Clock), Some(PanelKind::Clock));
        assert_eq!(PanelKind::for_module(BarModule::Menu), None);
        assert_eq!(PanelKind::Clock.content_width(), 560.0);
        assert_eq!(PanelKind::Audio.content_width(), 380.0);
    }
}

fn lerp_color(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    let t = t.clamp(0.0, 1.0);
    vec4(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t, a.w + (b.w - a.w) * t)
}

/// The panel's icon for what a hub model does.
fn hub_icon(kind: crate::hub_control::ModelKind) -> Ico {
    use crate::hub_control::ModelKind;
    match kind {
        ModelKind::Chat => Ico::Prompt,
        ModelKind::Vision => Ico::Photo,
        ModelKind::SpeechToText => Ico::Mic,
        ModelKind::TextToSpeech => Ico::Speaker,
        ModelKind::Image => Ico::Palette,
        ModelKind::Music => Ico::Pulse,
        ModelKind::Stems => Ico::Headphone,
        ModelKind::Video => Ico::Play,
        ModelKind::Mesh | ModelKind::Other => Ico::Chip,
    }
}

/// The next larger context the hub offers that still fits next to the UI (the chat
/// model's `context_fit`; unknown = not capped).
fn hub_context_up(hub: &crate::hub_control::HubSnapshot) -> Option<u32> {
    let fit = hub.models.iter().find(|m| Some(&m.id) == hub.chat_model.as_ref()).and_then(|m| m.context_fit);
    hub.context_options.iter().copied().find(|c| *c > hub.context && fit.is_none_or(|f| *c <= f))
}

/// Lines of a hub crash's or failed build's reason the panel shows.
const HUB_REASON_LINES: usize = 6;

/// The chat models of the hub's registry, for the WM's chat model pick.
fn hub_chat_models(hub: &crate::hub_control::HubSnapshot) -> Vec<String> {
    hub.chat_models.clone()
}

/// 49152 -> "48k".
fn kilo(tokens: u32) -> String {
    if tokens >= 1024 && tokens % 1024 == 0 { format!("{}k", tokens / 1024) } else { tokens.to_string() }
}

#[cfg(test)]
mod hub_panel_tests {
    use super::*;
    use crate::hub_control::{HubSnapshot, ModelInfo, ModelKind, ModelState};

    fn hub(fit: Option<u32>) -> HubSnapshot {
        HubSnapshot {
            available: true,
            models: vec![ModelInfo { id: "q".into(), name: "q".into(), kind: ModelKind::Chat, state: ModelState::Loaded, context_fit: fit, gpu_mb: 0, device: String::new() }],
            context: 32768,
            chat_model: Some("q".into()),
            context_options: vec![16384, 32768, 49152, 65536],
            ..Default::default()
        }
    }

    #[test]
    fn the_context_steps_up_only_as_far_as_the_budget_allows() {
        assert_eq!(hub_context_up(&hub(Some(57344))), Some(49152));
        assert_eq!(hub_context_up(&hub(Some(40000))), None);
        assert_eq!(hub_context_up(&hub(None)), Some(49152), "not known yet: not capped");
        assert_eq!(kilo(49152), "48k");
        assert_eq!(hub_icon(ModelKind::SpeechToText), Ico::Mic);
    }
}
