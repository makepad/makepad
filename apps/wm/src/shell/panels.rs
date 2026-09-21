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
//! * **monitor** — `space(380)`: hero, BRIGHTNESS and TEXT SIZE sliders,
//!   the SCALE pills and the display rows.
//!
//! What a Mac can answer cheaply is answered for real (volume, battery,
//! the calendar); what it cannot is drawn in the panel's own "not
//! available" reading rather than faked.

use makepad_widgets::*;

use super::bar::BarModule;
use super::ui::{contains, cut_top, inset, rect, DrawShellFill, Ico, ShellDraw};
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use super::wifi_linux::{
    KeyOutcome, RadioState, Security, StationState, WifiCommand, WifiHit, WifiPhase, WifiUi,
};
use super::{alpha, darker, CtrlState, ShellTokens};
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[path = "panels_linux.rs"]
mod linux;
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use {std::sync::Arc, super::system_linux::SystemSnapshot};
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use makepad_widgets::makepad_platform::linux_display::LinuxDisplaySnapshot;
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use makepad_widgets::makepad_platform::linux_gpu::LinuxGpuSnapshot;

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

/// Today, from `date +%Y-%m-%d`.
pub fn today() -> (i64, u32, u32) {
    let out = std::process::Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let mut parts = out.trim().split('-');
    let y = parts.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(2026);
    let m = parts.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(1);
    let d = parts.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(1);
    (y, m, d)
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
    pub brightness: Option<u32>,
    pub displays: Vec<String>,
    pub text_size: f64,
    pub network: Option<String>,
    pub bluetooth: Option<bool>,
    /// The month the calendar is showing.
    pub view: (i64, u32),
    pub today: (i64, u32, u32),
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
            brightness: None,
            displays: Vec::new(),
            text_size: 12.0,
            network: None,
            bluetooth: None,
            view: (y, m),
            today: (y, m, d),
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
            brightness: Some(80),
            displays: vec!["Built-in Retina Display".into()],
            text_size: 12.0,
            network: Some("Ethernet".into()),
            bluetooth: Some(true),
            view: (y, m),
            today: (y, m, d),
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

    /// The brightness mood names of the display panel.
    pub fn brightness_mood(&self) -> &'static str {
        match self.brightness {
            None => "Fixed brightness",
            Some(v) if v >= 95 => "Sun blast",
            Some(v) if v >= 80 => "Solar flare",
            Some(v) if v >= 65 => "Golden hour",
            Some(v) if v >= 45 => "Even day",
            Some(v) if v >= 30 => "Soft glow",
            Some(v) if v >= 20 => "Lamp light",
            Some(v) if v >= 10 => "Candlelit",
            _ => "Night owl",
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
    SetBrightness(u32),
    SetTextSize(f64),
    Close,
    /// The Wi-Fi dropdown's intent (Linux). The WM turns it into a worker
    /// command; a `ConnectWithPassphrase` is completed from the panel's own
    /// field so the passphrase never rides in an action.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    Wifi(WifiCommand),
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
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
    BrightnessSlider,
    TextSizeSlider,
    PrevMonth,
    NextMonth,
    Today,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    Wifi(WifiHit),
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    DevicePicker(bool),
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    Device(bool, usize),
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    InputMute,
    /// The Display panel's DPI slider: previews while held, applies on release.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    DpiScaleSlider,
    /// The "Optimize for" row and its list of driven outputs.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    SourcePicker,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    Source(usize),
    /// The compositor GPU row and its list: Auto (display GPU), then the GPUs.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    GpuPicker,
    /// The focused app's GPU row and its list: Follow compositor, then the GPUs.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    AppGpuPicker,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    Gpu(usize),
    /// Opens or closes the Display panel's Mouse & touchpad subview.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    PointerSettings,
    /// Pointer speed slider: `false` mouse, `true` touchpad.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    PointerSpeedSlider(bool),
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
    #[rust]
    screen: Rect,
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
    /// The Wi-Fi dropdown's state (Linux): the iwd snapshot, the password
    /// prompt, the notice line. Fed by the WM from the worker's events.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    pub wifi: WifiUi,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    pub system: Arc<SystemSnapshot>,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    pub system_notice: String,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    device_picker: Option<bool>,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    device_scroll: usize,
    /// The main window's actual layout scale as the WM last read it off the
    /// platform (`window_geom.dpi_factor`) — never the saved value. Zero
    /// until the first read; the panel then shows the launch default.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    pub current_dpi: f64,
    /// The scale under the pointer while the DPI slider is held, in
    /// hundredths. Applied on release; dropped when the flyout closes or
    /// Escape cancels, so the window never moves under a drag.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    pub dpi_preview: Option<u32>,
    /// What the renderer sees: the connected outputs, and whether the WM
    /// drives them itself (clone) or the desktop does.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    pub display_snapshot: LinuxDisplaySnapshot,
    /// The compositor GPU as the renderer reports it. Cached from
    /// `cx.linux_gpu_snapshot`; not inferred from the output list.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    pub gpu_snapshot: LinuxGpuSnapshot,
    /// A compositor or per-app GPU switch is in progress.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    pub gpu_busy: bool,
    /// Focused running native child: id, label, policy override, actual UUID.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    pub gpu_app: Option<(crate::ClientId, String, Option<[u8; 16]>, [u8; 16])>,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    display_scroll: usize,
    /// The display list did not fit the card: the wheel scrolls it.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    display_overflow: bool,
    /// The screen rect when the anchor was taken: a re-laid-out window
    /// moves the anchor with its edge until the bar's fresh rect arrives.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    anchor_screen: Rect,
    /// The "Optimize for" list is unfolded.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    source_picker: bool,
    /// The unfolded list's first shown row, so every driven output stays
    /// reachable when the screen leaves room for a single row.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    source_scroll: usize,
    /// The unfolded list did not fit: the wheel scrolls it.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    source_overflow: bool,
    /// The connector names behind the drawn `Hit::Source` rows, in row
    /// order, so a click lands on the output that was shown — never on
    /// whatever a fresh inventory put at that index.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    source_targets: Vec<String>,
    /// The compositor GPU list is unfolded.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    gpu_picker: bool,
    /// The focused-app GPU list is unfolded.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    app_gpu_picker: bool,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    gpu_scroll: usize,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    gpu_overflow: bool,
    /// The choice behind each drawn `Hit::Gpu` row: `None` is Auto / Follow
    /// compositor, `Some` a GPU's sysfs identity — bound at draw time.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    gpu_targets: Vec<Option<String>>,
    /// Client captured when the app GPU list opened, so a focus change
    /// cannot retarget the drawn rows.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    gpu_target_client: Option<crate::ClientId>,
    /// The Display panel is showing the Mouse & touchpad subview.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    pointer_settings: bool,
    /// A source the renderer was asked for and has not yet named as the
    /// primary, with when it was asked. The WM clears it on observation
    /// or reports it after a bounded wait.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    pub display_source_pending: Option<(String, f64)>,
    /// Escape cancelled a scale preview: its key-up is taken too, so no
    /// tile sees half a key press.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    swallow_escape_up: bool,
}

impl ShellPanel {
    pub fn toggle(&mut self, cx: &mut Cx, kind: PanelKind, anchor: Rect) {
        let was_open = self.open;
        if self.open == Some(kind) {
            self.open = None;
        } else {
            self.open = Some(kind);
            self.anchor = anchor;
            #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
            {
                self.anchor_screen = self.screen;
                // The outputs and compositor GPU as of this open; the WM
                // refreshes them while the panel stays up.
                if kind == PanelKind::Monitor {
                    self.display_snapshot = cx.linux_display_snapshot();
                    self.gpu_snapshot = cx.linux_gpu_snapshot();
                }
            }
        }
        self.left_panel(was_open);
        self.redraw(cx);
    }

    pub fn close(&mut self, cx: &mut Cx) {
        let was_open = self.open;
        self.open = None;
        self.left_panel(was_open);
        self.redraw(cx);
    }

    /// The bar re-laid out (a DPI change, a resize): follow the module.
    /// True when the card moved.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
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
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
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
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        {
            self.device_picker = None;
            self.device_scroll = 0;
            self.dpi_preview = None;
            self.display_scroll = 0;
            self.source_picker = false;
            self.source_scroll = 0;
            self.source_targets.clear();
            self.gpu_picker = false;
            self.app_gpu_picker = false;
            self.gpu_scroll = 0;
            self.gpu_targets.clear();
            self.gpu_target_client = None;
            self.pointer_settings = false;
        }
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
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
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        let w = w.min((screen.size.x - margin * 2.0).max(0.0));
        let anchor = if self.anchor.size.x > 0.0 {
            self.anchor
        } else {
            rect(
                screen.pos.x + screen.size.x * 0.5,
                screen.pos.y,
                0.0,
                self.tokens.bar.size_horizontal,
            )
        };
        let x = (anchor.pos.x + anchor.size.x * 0.5 - w * 0.5)
            .max(screen.pos.x + margin)
            .min(screen.pos.x + screen.size.x - w - margin)
            .floor();
        let y = (anchor.pos.y + anchor.size.y + margin).floor();
        rect(x, y, w, height.min(screen.pos.y + screen.size.y - y - margin))
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
        #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
        let fg = if enabled {
            tok.popups.text
        } else {
            alpha(tok.popups.text, 0.4)
        };
        #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
        self.d.panel_slider(
            cx,
            track,
            &tok,
            progress,
            fg,
            tok.popups.background,
            hot && enabled,
        );
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
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
        self.screen = screen;
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
        } + pad * 2.0;
        // The body is inset by the padding AND the popup border. The
        // Display panel's plan is sized to the body, so its card asks for
        // the border too; otherwise the full plan never fits and the clone
        // picture is dropped even on a tall desktop. The screen clamp and
        // the compact fallback still apply.
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        let height = if kind == PanelKind::Monitor {
            height + tok.popups.border_width * 2.0
        } else {
            height
        };
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        self.follow_screen(kind, screen);
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
            #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
            PanelKind::Network => self.draw_wifi(cx, body),
            // Elsewhere the panel keeps its honest "no service here" reading.
            #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
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

    #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
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

    #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
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

    #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
    fn draw_monitor(&mut self, cx: &mut Cx2d, body: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let (hero, rest) = cut_top(body, 40.0);
        let mood = self.data.brightness_mood();
        self.d
            .panel_hero(cx, hero, &tok, fg, Ico::Monitor, "Display", mood, 0.0);

        let (sep, rest) = cut_top(rest, tok.spacing.panel_gap);
        self.d.separator(cx, sep, fg, 0.12);
        let (header, rest) = cut_top(rest, 18.0);
        let value = match self.data.brightness {
            Some(v) => format!("{}%", v),
            None => "--".into(),
        };
        self.section_header(cx, header, "BRIGHTNESS", &value);
        let (row, rest) = cut_top(rest, 28.0);
        self.slider_row(
            cx,
            row,
            Hit::BrightnessSlider,
            self.data.brightness.unwrap_or(0) as f64 / 100.0,
            self.data.brightness.is_some(),
        );

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
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        return ((p.x - track.pos.x - 6.0) / (track.size.x - 12.0).max(1.0)).clamp(0.0, 1.0);
        #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
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

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
const WIFI_MAX_ROWS: usize = 7;
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
const WIFI_ROW_H: f64 = 28.0;
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
const WIFI_ADDR_ROW_H: f64 = 20.0;

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
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

        self.draw_wifi_action(cx, rest);
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
    /// Escape while a scale preview is held or the "Optimize for" list is
    /// unfolded.
    pub fn wants_keys(&self) -> bool {
        self.open == Some(PanelKind::Network)
            || self.dpi_preview.is_some()
            || self.source_picker
            || self.gpu_picker
            || self.app_gpu_picker
            || self.pointer_settings
            || self.swallow_escape_up
    }

    /// The DPI slider is held or its preview is up: a saved scale landing
    /// now would move the window under the pointer, so the WM waits.
    pub fn dpi_adjusting(&self) -> bool {
        self.dpi_preview.is_some() || self.dragging == Some(Hit::DpiScaleSlider)
    }

    /// Drop an unapplied scale preview; the flyout stays, nothing is
    /// emitted, and a release that follows applies nothing.
    fn cancel_dpi_preview(&mut self, cx: &mut Cx) {
        self.dpi_preview = None;
        if self.dragging == Some(Hit::DpiScaleSlider) {
            self.dragging = None;
        }
        self.redraw(cx);
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

    /// The keyboard while a flyout wants it: Escape cancels a held scale
    /// preview; with the Wi-Fi dropdown up the prompt owns the keys
    /// (`WifiUi::key`), otherwise Escape closes the dropdown. True when
    /// the key was consumed.
    pub fn key(&mut self, cx: &mut Cx, e: &KeyEvent) -> bool {
        if e.key_code == KeyCode::Escape && self.dpi_preview.is_some() {
            self.cancel_dpi_preview(cx);
            self.swallow_escape_up = true;
            return true;
        }
        // Escape folds the "Optimize for" list; the Display panel stays.
        if e.key_code == KeyCode::Escape && self.source_picker {
            self.source_picker = false;
            self.source_scroll = 0;
            self.source_targets.clear();
            self.swallow_escape_up = true;
            self.redraw(cx);
            return true;
        }
        // Same for the compositor / app GPU list.
        if e.key_code == KeyCode::Escape && (self.gpu_picker || self.app_gpu_picker) {
            self.gpu_picker = false;
            self.app_gpu_picker = false;
            self.gpu_scroll = 0;
            self.gpu_targets.clear();
            self.gpu_target_client = None;
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
            || (self.open == Some(PanelKind::Monitor)
                && (self.display_overflow
                    || (self.source_picker && self.source_overflow)
                    || ((self.gpu_picker || self.app_gpu_picker) && self.gpu_overflow))
                && contains(self.card, p))
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

#[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
impl ShellPanel {
    fn audio_height(&self) -> f64 { 300.0 }
    fn monitor_height(&self) -> f64 { 300.0 }
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
        let screen = cx.turtle().rect();
        self.draw_surface(cx, screen);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.inert || self.open.is_none() {
            return;
        }
        match event {
            Event::MouseMove(e) => {
                if let Some(hit) = self.dragging {
                    let v = self.slider_value(hit, e.abs);
                    match hit {
                        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                        Hit::InputSlider => self.system_action(cx, crate::linux_controls::ControlAction::InputVolume((v * 100.0).round() as u32)),
                        Hit::VolumeSlider => cx.widget_action(
                            self.uid,
                            ShellPanelAction::SetVolume((v * 100.0).round() as u32),
                        ),
                        Hit::BrightnessSlider => cx.widget_action(
                            self.uid,
                            ShellPanelAction::SetBrightness((v * 100.0).round() as u32),
                        ),
                        // A preview only: the window is re-laid out on release.
                        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                        Hit::DpiScaleSlider => {
                            self.dpi_preview = Some(Self::dpi_from_slider(v));
                            self.redraw(cx);
                        }
                        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                        Hit::PointerSpeedSlider(touchpad) => {
                            self.system_action(cx, crate::linux_controls::ControlAction::PointerSpeed {
                                touchpad,
                                value: Self::pointer_from_slider(v),
                            });
                        }
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
                    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                    Some(Hit::Wifi(hit)) => self.wifi_press(cx, hit),
                    Some(Hit::VolumeSlider) => {
                        self.dragging = Some(Hit::VolumeSlider);
                        let v = self.slider_value(Hit::VolumeSlider, e.abs);
                        cx.widget_action(
                            self.uid,
                            ShellPanelAction::SetVolume((v * 100.0).round() as u32),
                        );
                    }
                    Some(Hit::BrightnessSlider) => {
                        self.dragging = Some(Hit::BrightnessSlider);
                        let v = self.slider_value(Hit::BrightnessSlider, e.abs);
                        cx.widget_action(
                            self.uid,
                            ShellPanelAction::SetBrightness((v * 100.0).round() as u32),
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
                        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                        {
                            self.dragging = Some(Hit::InputSlider);
                            let v = self.slider_value(Hit::InputSlider, e.abs);
                            self.system_action(cx, crate::linux_controls::ControlAction::InputVolume((v * 100.0).round() as u32));
                        }
                    }
                    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                    Some(hit @ (Hit::DevicePicker(_) | Hit::Device(_, _) | Hit::InputMute
                        | Hit::SourcePicker | Hit::Source(_) | Hit::GpuPicker | Hit::AppGpuPicker
                        | Hit::Gpu(_) | Hit::PointerSettings)) => self.system_press(cx, hit),
                    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                    Some(Hit::DpiScaleSlider) => {
                        self.dragging = Some(Hit::DpiScaleSlider);
                        let v = self.slider_value(Hit::DpiScaleSlider, e.abs);
                        self.dpi_preview = Some(Self::dpi_from_slider(v));
                        self.redraw(cx);
                    }
                    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                    Some(hit @ Hit::PointerSpeedSlider(touchpad)) => {
                        self.dragging = Some(hit);
                        let v = self.slider_value(hit, e.abs);
                        self.system_action(cx, crate::linux_controls::ControlAction::PointerSpeed {
                            touchpad,
                            value: Self::pointer_from_slider(v),
                        });
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
                // The DPI slider applies on release only, so the window is
                // never re-laid out under a drag in progress.
                #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                if self.dragging == Some(Hit::DpiScaleSlider) {
                    self.dragging = None;
                    if let Some(value) = self.dpi_preview.take() {
                        self.system_action(cx, crate::linux_controls::ControlAction::DpiScale(value));
                    }
                    self.redraw(cx);
                }
                self.dragging = None;
            }
            // Escape on a held scale preview is taken by `key`, routed
            // ahead of every other key handler by the WM (lib.rs), not here.
            #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
            Event::Scroll(e) => {
                if self.wants_scroll(e.abs) {
                    if self.open == Some(PanelKind::Audio) { self.device_scroll_by(cx, e.scroll.y); }
                    else if self.open == Some(PanelKind::Monitor) {
                        // The unfolded source list takes the wheel while it
                        // is the thing that overflows.
                        if self.source_picker && self.source_overflow { self.source_scroll_by(cx, e.scroll.y); }
                        else if (self.gpu_picker || self.app_gpu_picker) && self.gpu_overflow { self.gpu_scroll_by(cx, e.scroll.y); }
                        else { self.display_scroll_by(cx, e.scroll.y); }
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
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
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
        d.brightness = Some(80);
        assert_eq!(d.brightness_mood(), "Solar flare");
        d.brightness = None;
        assert_eq!(d.brightness_mood(), "Fixed brightness");
    }

    #[test]
    fn panels_bind_to_their_bar_modules() {
        assert_eq!(PanelKind::for_module(BarModule::Clock), Some(PanelKind::Clock));
        assert_eq!(PanelKind::for_module(BarModule::Menu), None);
        assert_eq!(PanelKind::Clock.content_width(), 560.0);
        assert_eq!(PanelKind::Audio.content_width(), 380.0);
    }
}
