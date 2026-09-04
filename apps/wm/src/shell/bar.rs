//! `shell/plugins/bar/` — the top bar.
//!
//! The module list and its order are `config/omarchy/shell.json`:
//! left `[menu, workspaces]`, center `[indicators, clock, keyboard-layout,
//! weather, system-update]` with `centerAnchor: omarchy.clock`, right
//! `[tray, agents, bluetooth, network, audio, monitor, power]`.
//!
//! Geometry from `Bar.qml` + `Ui/WidgetButton.qml` / `BarIconButton.qml`:
//! the strip is `Style.bar.sizeHorizontal` (26) tall and filled with
//! `Color.bar.background` at α 1.0 — no border, no separators; the left and
//! right clusters sit `space(8)` off their edge, modules inside a cluster
//! touch (`Row{spacing: 0}`) and pad themselves; the center anchor module
//! is centered on the SCREEN with the modules before it flush against its
//! left edge and the ones after flush against its right. Icon buttons are a
//! 27px slot around a 16px canvas, indicators a 21px status slot at
//! `caption` 10, the clock a label with 8.75px side margins, workspace
//! pills 20 wide with 1px between them, and the module whose panel is open
//! wears a 2px accent pill inset 2px from the bar's inner edge, 15 long.
//!
//! Mouse: a press on the menu button opens the menu, a press on a
//! workspace focuses it, a press on a tray/status module opens its panel,
//! and the wheel over audio/monitor steps volume/brightness — the same
//! gestures as the original.

use makepad_widgets::*;

use super::ui::{contains, rect, DrawShellFill, Ico, ShellDraw};
use super::{alpha, fade, ShellTokens};

/// Every clickable thing in the bar, in `shell.json` id terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarModule {
    Menu,
    Workspace(usize),
    /// `widgets/ActiveWindow.qml` — not in the stock `shell.json` center
    /// list, but our bar carries it right after the workspaces.
    ActiveWindow,
    Indicator(usize),
    Clock,
    KeyboardLayout,
    Weather,
    SystemUpdate,
    Tray(usize),
    Bluetooth,
    Network,
    Audio,
    Monitor,
    Power,
    /// The window's own controls, at the far right where Windows and most
    /// Linux desktops put them: the omarchy bar IS this window's caption,
    /// and a client-sized frame draws none of its own. macOS keeps its
    /// traffic lights on the left instead.
    WindowMin,
    WindowMax,
    WindowClose,
}

/// One workspace pill.
#[derive(Clone, Debug)]
pub struct WorkspaceCell {
    pub label: String,
    pub occupied: bool,
    pub focused: bool,
}

/// A bar indicator (`plugins/bar/indicators/`): one glyph with an active
/// and an inactive reading. Inactive ones are hidden until the pointer is
/// over the indicator block, then shown at α .45.
#[derive(Clone, Debug)]
pub struct Indicator {
    pub icon: Ico,
    pub active_icon: Ico,
    pub active: bool,
    pub tooltip: &'static str,
}

/// What the bar shows. The WM fills it from real state; the gallery fills
/// it with fixtures. Nothing in here is sampled by the widget itself.
#[derive(Clone, Debug, Default)]
pub struct BarData {
    pub workspaces: Vec<WorkspaceCell>,
    pub indicators: Vec<Indicator>,
    /// The focused window's title (`ActiveWindow.qml`).
    pub active_window: Option<String>,
    /// Already formatted — `dddd HH:mm`, or the alt `d MMMM 'W'ww yyyy`.
    pub clock: String,
    pub keyboard_layout: Option<String>,
    pub weather: Option<String>,
    pub system_update: bool,
    pub tray: Vec<Ico>,
    /// `None` renders the module in its "unavailable" reading: the icon at
    /// α .45, which is what an omarchy indicator does when its service is
    /// not there.
    pub bluetooth: Option<bool>,
    pub network: Option<bool>,
    pub volume: Option<u32>,
    pub muted: bool,
    pub brightness: Option<u32>,
    pub battery: Option<Battery>,
    /// The module whose panel is open — it wears the accent pill.
    pub open_panel: Option<BarModule>,
    /// The window is maximized: the middle control shows "restore".
    pub maximized: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Battery {
    pub percent: u32,
    pub charging: bool,
}

impl BarData {
    /// The fixture the gallery (and `plugins/dev-gallery`) draws with.
    pub fn fixture() -> Self {
        Self {
            workspaces: vec![
                WorkspaceCell {
                    label: "1".into(),
                    occupied: true,
                    focused: true,
                },
                WorkspaceCell {
                    label: "2".into(),
                    occupied: true,
                    focused: false,
                },
                WorkspaceCell {
                    label: "3".into(),
                    occupied: false,
                    focused: false,
                },
                WorkspaceCell {
                    label: "4".into(),
                    occupied: false,
                    focused: false,
                },
                WorkspaceCell {
                    label: "5".into(),
                    occupied: false,
                    focused: false,
                },
            ],
            indicators: default_indicators(),
            active_window: Some("terminal — ~/makepad".into()),
            clock: "Thursday 21:34".into(),
            keyboard_layout: Some("en".into()),
            weather: None,
            system_update: false,
            tray: vec![],
            bluetooth: Some(true),
            network: Some(true),
            volume: Some(62),
            muted: false,
            brightness: Some(80),
            battery: Some(Battery {
                percent: 87,
                charging: true,
            }),
            open_panel: None,
            maximized: false,
        }
    }
}

/// `defaultIndicatorEntries` — Dictation, ScreenRecording, Reminder,
/// NightLight, Dnd, StayAwake — with the ones we can actually mean.
pub fn default_indicators() -> Vec<Indicator> {
    vec![
        Indicator {
            icon: Ico::Record,
            active_icon: Ico::Record,
            active: false,
            tooltip: "Screen Recording",
        },
        Indicator {
            icon: Ico::Moon,
            active_icon: Ico::Moon,
            active: false,
            tooltip: "Night Light",
        },
        Indicator {
            icon: Ico::Bell,
            active_icon: Ico::BellOff,
            active: false,
            tooltip: "Silence Notifications",
        },
    ]
}

/// The volume glyph ladder: muted, then ≤33 / ≤66 / above.
pub fn volume_icon(level: Option<u32>, muted: bool) -> Ico {
    match level {
        _ if muted => Ico::Volume0,
        None => Ico::Volume0,
        Some(0) => Ico::Volume0,
        Some(v) if v <= 33 => Ico::Volume1,
        Some(v) if v <= 66 => Ico::Volume2,
        Some(_) => Ico::Volume3,
    }
}

// ======================================================================
// Cheap real data (macOS)
// ======================================================================

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (cmd, args);
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let out = std::process::Command::new(cmd).args(args).output().ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8(out.stdout).ok()
    }
}

/// `date +"%A %H:%M"` — omarchy's `dddd HH:mm`.
pub fn sample_clock(alt: bool) -> String {
    let fmt = if alt { "+%-d %B W%V %Y" } else { "+%A %H:%M" };
    run("date", &[fmt])
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Everything the bar reads from the OS, gathered OFF the main thread.
///
/// Every sampler below is a `fork+exec+wait` (`osascript`, `date`, `pmset`,
/// `route`, `defaults`) that can take hundreds of milliseconds under load.
/// Running them on the main thread's 1s status timer blocked the whole
/// event loop ~0.5s every second — which starved the 8ms Ticks that drive
/// every hosted tile, freezing all child apps in visible hiccups (the
/// "0.5s of nothing while dragging" report; a `sample` showed 614/1646
/// main-thread samples inside update_status, 326 in sample_volume).
#[derive(Clone, Default)]
pub struct SampledStatus {
    pub volume: Option<u32>,
    pub muted: bool,
    pub clock: String,
    pub clock_alt: String,
    pub battery: Option<Battery>,
    pub network: Option<bool>,
    pub bluetooth: Option<bool>,
}

/// Spawn the lifetime sampler worker: refreshes the cheap facts every second
/// and the slow ones every fifth round, then sends an immutable snapshot to
/// the UI. Dropping the receiver ends the worker.
pub fn start_status_sampler(
    spawner: &makepad_widgets::makepad_platform::thread::ThreadSpawner,
) -> Result<
    (
        std::sync::mpsc::Receiver<SampledStatus>,
        makepad_widgets::makepad_platform::thread::TaskHandle<()>,
    ),
    makepad_widgets::makepad_platform::thread::SpawnError,
> {
    use makepad_widgets::makepad_platform::thread::{CancellationToken, SignalToUI, ThreadOptions};
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = spawner.spawn_worker(
        ThreadOptions {
            name: Some("wm-status".into()),
            ..Default::default()
        },
        move || {
            let mut round: u32 = 0;
            let mut status = SampledStatus::default();
            let wait = CancellationToken::new();
            loop {
                let (volume, muted) = sample_volume();
                status.volume = volume;
                status.muted = muted;
                status.clock = sample_clock(false);
                status.clock_alt = sample_clock(true);
                if round % 5 == 0 {
                    status.battery = sample_battery();
                    status.network = sample_network();
                    status.bluetooth = sample_bluetooth();
                }
                if tx.send(status.clone()).is_err() {
                    break;
                }
                SignalToUI::set_ui_signal();
                round = round.wrapping_add(1);
                let _ = wait.wait_until(makepad_widgets::Cx::monotonic_now() + 1.0);
            }
        },
    )?;
    Ok((rx, worker))
}

/// Output volume and mute, from the shared system mixer.
pub fn sample_volume() -> (Option<u32>, bool) {
    #[cfg(target_os = "macos")]
    {
        let level = run(
            "osascript",
            &["-e", "output volume of (get volume settings)"],
        )
        .and_then(|s| s.trim().parse::<u32>().ok());
        let muted = run("osascript", &["-e", "output muted of (get volume settings)"])
            .map(|s| s.trim() == "true")
            .unwrap_or(false);
        (level, muted)
    }
    #[cfg(not(target_os = "macos"))]
    {
        (None, false)
    }
}

/// `pmset -g batt` — percent plus whether it is charging.
pub fn sample_battery() -> Option<Battery> {
    #[cfg(target_os = "macos")]
    {
        let out = run("pmset", &["-g", "batt"])?;
        let percent = out
            .split('\t')
            .nth(1)
            .or_else(|| out.split(';').next())
            .and_then(|s| s.split('%').next())
            .and_then(|s| s.rsplit(|c: char| !c.is_ascii_digit()).next())
            .and_then(|s| s.parse::<u32>().ok())?;
        let charging = out.contains("AC Power") || out.contains("charging");
        Some(Battery { percent, charging })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Whether we have a default route with an address on it.
pub fn sample_network() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        let iface = run("route", &["-n", "get", "default"])?;
        let dev = iface
            .lines()
            .find_map(|l| l.trim().strip_prefix("interface: "))
            .map(|s| s.trim().to_string())?;
        Some(run("ipconfig", &["getifaddr", &dev]).map(|s| !s.trim().is_empty()) == Some(true))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// The bluetooth controller's power state — the one bluetooth fact that is
/// cheap to read here (no device list, so the panel says so).
pub fn sample_bluetooth() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        let out = run(
            "defaults",
            &[
                "read",
                "/Library/Preferences/com.apple.Bluetooth",
                "ControllerPowerState",
            ],
        )?;
        Some(out.trim() == "1")
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

// ======================================================================
// The surface
// ======================================================================

/// `Bar.qml`: the clusters sit `space(8)` off their edge.
const EDGE_MARGIN: f64 = 8.0;
/// `WidgetButton.horizontalMargin` for the menu button.
const MENU_MARGIN: f64 = 7.5;
/// `WidgetButton.horizontalMargin` for the clock.
const CLOCK_MARGIN: f64 = 8.75;
/// `Workspaces.qml`: `Style.space(20)` per pill, 1px column spacing, and
/// `space(1.5)` of trailing gap after the grid.
const WS_WIDTH: f64 = 20.0;
const WS_SPACING: f64 = 1.0;
const WS_TRAILING: f64 = 1.5;
/// The open-panel pill: 2px thick, inset 2px, `max(10, round(slot*0.55))`.
const PANEL_PILL_THICKNESS: f64 = 2.0;
const PANEL_PILL_INSET: f64 = 2.0;
/// `ActiveWindow.qml`: the title never grows past 280px.
const ACTIVE_WINDOW_MAX: f64 = 280.0;
/// `Ui/Button.qml` tooltip `delay` — 400ms of hover before it shows.
const TOOLTIP_DELAY: f64 = 0.4;
/// The window controls: three slots of this width at the bar's far right,
/// the usual caption-button proportion (wider than an icon slot, so a
/// close is an easy target), a gap before the status modules.
const WINDOW_CONTROL_WIDTH: f64 = 36.0;
const WINDOW_CONTROL_GAP: f64 = 6.0;

/// The module whose recorded hit rect holds `p` — what the WM's drag query
/// asks (`shell_bar_claims`): a claimed point is a button, not a drag.
pub fn module_at_in(hits: &[(BarModule, Rect)], p: Vec2d) -> Option<BarModule> {
    hits.iter().find(|(_, r)| contains(*r, p)).map(|(m, _)| *m)
}

/// Whether this platform draws its window controls in the bar: every
/// platform but macOS, whose traffic lights sit natively on the left.
pub const fn window_controls_default() -> bool {
    !cfg!(target_os = "macos")
}

/// Where the right side of the bar starts, given the strip's right edge:
/// the x the window controls occupy from (`None` without them) and the x
/// the status modules are laid out leftwards from. Pure, so the layout is
/// tested without a window.
pub fn right_cluster_layout(bar_right: f64, controls: bool) -> (Option<f64>, f64) {
    let edge = bar_right - EDGE_MARGIN;
    if !controls {
        return (None, edge);
    }
    let controls_left = edge - 3.0 * WINDOW_CONTROL_WIDTH;
    (Some(controls_left), controls_left - WINDOW_CONTROL_GAP)
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ShellBarBase = #(ShellBar::register_widget(vm))
    mod.widgets.ShellBar = set_type_default() do mod.widgets.ShellBarBase {
        width: Fill
        height: 26
        draw_bg +: {}
        d +: {}
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum ShellBarAction {
    /// A module was pressed (left button).
    Press(BarModule),
    /// A module was right-pressed — the clock cycles its format, audio
    /// mutes, power toggles the percentage.
    RightPress(BarModule),
    /// A module was middle-pressed (`ActiveWindow.qml`'s
    /// `Qt.MiddleButton` arm: only the active-window title answers to
    /// this, closing the focused client exactly like a right click does).
    MiddlePress(BarModule),
    /// Wheel over a module: +1 / -1 notch.
    Wheel(BarModule, f64),
    #[default]
    None,
}

/// `MouseDown.button` -> the press action it raises. Middle is checked
/// before secondary — a mouse reporting both bits on the same event (some
/// drivers do, for a chorded click) still reads as the tertiary button,
/// matching `ActiveWindow.qml`'s own `Qt.MiddleButton` branch order.
fn press_action(module: BarModule, button: MouseButton) -> ShellBarAction {
    if button.contains(MouseButton::MIDDLE) {
        ShellBarAction::MiddlePress(module)
    } else if button.contains(MouseButton::SECONDARY) {
        ShellBarAction::RightPress(module)
    } else {
        ShellBarAction::Press(module)
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ShellBar {
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
    pub data: BarData,
    /// Content starts here, so the OS window buttons stay clear.
    #[rust]
    pub pad_left: f64,
    #[rust]
    area: Area,
    /// The rect the bar was last drawn into — the gallery drives these
    /// surfaces directly, without a turtle of their own.
    #[rust]
    screen: Rect,
    #[rust]
    hits: Vec<(BarModule, Rect)>,
    #[rust]
    hover: Option<BarModule>,
    #[rust]
    hover_time: f64,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_time: f64,
    /// The indicator block reveals its inactive glyphs while hovered.
    #[rust]
    reveal_indicators: bool,
    #[rust]
    pub inert: bool,
    /// Draw the window's min/max/close at the far right
    /// ([`window_controls_default`]; the gallery turns it on to show them).
    #[rust]
    pub window_controls: bool,
}

impl ShellBar {
    pub fn set_data(&mut self, cx: &mut Cx, data: BarData) {
        self.data = data;
        self.redraw(cx);
    }

    /// A live theme switch: new tokens, redraw.
    pub fn set_tokens(&mut self, cx: &mut Cx, tokens: ShellTokens) {
        self.tokens = tokens;
        self.redraw(cx);
    }

    fn icon_slot(&self) -> f64 {
        self.tokens.bar.icon_slot
    }

    fn status_slot(&self) -> f64 {
        self.tokens.bar.status_slot
    }

    /// The status modules on the right, in `shell.json` order, with the
    /// icon each one shows right now.
    fn right_modules(&self) -> Vec<(BarModule, Ico, bool)> {
        let mut v: Vec<(BarModule, Ico, bool)> = Vec::new();
        for (i, ico) in self.data.tray.iter().enumerate() {
            v.push((BarModule::Tray(i), *ico, true));
        }
        v.push((
            BarModule::Bluetooth,
            match self.data.bluetooth {
                Some(true) => Ico::Bluetooth,
                _ => Ico::BluetoothOff,
            },
            self.data.bluetooth.is_some(),
        ));
        v.push((
            BarModule::Network,
            match self.data.network {
                Some(true) => Ico::Wifi,
                _ => Ico::WifiOff,
            },
            self.data.network.is_some(),
        ));
        v.push((
            BarModule::Audio,
            volume_icon(self.data.volume, self.data.muted),
            self.data.volume.is_some(),
        ));
        v.push((
            BarModule::Monitor,
            Ico::Monitor,
            self.data.brightness.is_some(),
        ));
        v.push((
            BarModule::Power,
            if self.data.battery.is_some() {
                Ico::Battery
            } else {
                Ico::Power
            },
            self.data.battery.is_some(),
        ));
        v
    }

    /// Draw the bar into `r`. Hit rects are recorded for the next event
    /// pass. Under glass the whole bar draws in its own overlay list.
    pub fn draw_bar(&mut self, cx: &mut Cx2d, r: Rect) {
        let tokens = self.tokens;
        self.d.begin_surface(cx, &tokens);
        self.draw_bar_inner(cx, r);
        self.d.end_surface(cx);
    }

    fn draw_bar_inner(&mut self, cx: &mut Cx2d, r: Rect) {
        let tok = self.tokens;
        let fg = tok.bar.text;
        let accent = tok.bar.active;
        let slot = self.icon_slot();
        let canvas = tok.bar.icon_canvas;
        self.hits.clear();
        self.screen = r;

        // The strip itself: `Color.bar.background`, no border. Under glass
        // the material paints it and the fill stays as an invisible redraw
        // anchor (it is the widget's `#[redraw]` area).
        self.draw_bg.color = if tok.material.is_glass() {
            Vec4f::default()
        } else {
            alpha(tok.bar.background, tok.bar.background_alpha)
        };
        self.draw_bg.draw_abs(cx, r);
        if tok.material.is_glass() {
            self.d.glass_strip(cx, r);
        }

        // ---- left: menu, workspaces
        let mut x = r.pos.x + self.pad_left.max(EDGE_MARGIN);
        let menu_w = (canvas + MENU_MARGIN * 2.0).max(12.0);
        let menu_rect = rect(x, r.pos.y, menu_w, r.size.y);
        self.d
            .icon_centered(cx, Ico::Menu, menu_rect, canvas, fg);
        self.hits.push((BarModule::Menu, menu_rect));
        x += menu_w;

        for (i, ws) in self.data.workspaces.clone().iter().enumerate() {
            let cell = rect(x, r.pos.y, WS_WIDTH, r.size.y);
            let lit = ws.occupied || ws.focused;
            let color = fade(fg, if lit { 1.0 } else { 0.5 });
            if ws.focused {
                // The focused workspace is a dot, not its number.
                self.d
                    .icon_centered(cx, Ico::Dot, cell, tok.bar.icon_font * 0.5, color);
            } else {
                self.d.label(
                    cx,
                    cell,
                    false,
                    tok.bar.icon_font,
                    color,
                    super::ui::HAlign::Center,
                    &ws.label,
                );
            }
            self.hits.push((BarModule::Workspace(i), cell));
            x += WS_WIDTH + WS_SPACING;
        }
        x += WS_TRAILING;

        // The active window's title: `min(280, implicitWidth) + controlPaddingX*2`,
        // `body` at α .85, elided right, with the full title in the tooltip.
        if let Some(title) = self.data.active_window.clone() {
            let text_w = self
                .d
                .measure(cx, false, tok.font.body, &title)
                .min(ACTIVE_WINDOW_MAX);
            let w = text_w + tok.spacing.control_padding_x * 2.0;
            let cell = rect(x, r.pos.y, w, r.size.y);
            self.d.label_elided(
                cx,
                rect(
                    cell.pos.x + tok.spacing.control_padding_x,
                    cell.pos.y,
                    text_w,
                    cell.size.y,
                ),
                false,
                tok.font.body,
                fade(fg, 0.85),
                super::ui::HAlign::Left,
                &title,
            );
            self.hits.push((BarModule::ActiveWindow, cell));
        }

        // ---- center: the clock is the anchor, centered on the bar itself
        let clock_w = self
            .d
            .measure(cx, false, tok.font.body, &self.data.clock)
            + CLOCK_MARGIN * 2.0;
        let clock_rect = rect(
            (r.pos.x + (r.size.x - clock_w) * 0.5).floor(),
            r.pos.y,
            clock_w,
            r.size.y,
        );
        self.d.label(
            cx,
            clock_rect,
            false,
            tok.font.body,
            fg,
            super::ui::HAlign::Center,
            &self.data.clock,
        );
        self.hits.push((BarModule::Clock, clock_rect));

        // Indicators sit flush against the anchor's left edge.
        let status = self.status_slot();
        let indicators = self.data.indicators.clone();
        let mut ix = clock_rect.pos.x;
        for (i, ind) in indicators.iter().enumerate().rev() {
            let visible = ind.active || self.reveal_indicators;
            if !visible {
                continue;
            }
            ix -= status;
            let cell = rect(ix, r.pos.y, status, r.size.y);
            let color = if ind.active {
                accent
            } else {
                fade(fg, 0.45)
            };
            let ico = if ind.active { ind.active_icon } else { ind.icon };
            self.d
                .icon_centered(cx, ico, cell, tok.font.caption * 1.3, color);
            self.hits.push((BarModule::Indicator(i), cell));
        }

        // Keyboard layout, weather and the update dot follow the anchor.
        let mut cx_right = clock_rect.pos.x + clock_rect.size.x;
        if let Some(layout) = self.data.keyboard_layout.clone() {
            let w = self.d.measure(cx, false, tok.font.caption, &layout) + 6.0 * 2.0;
            let cell = rect(cx_right, r.pos.y, w, r.size.y);
            self.d.label(
                cx,
                cell,
                false,
                tok.font.caption,
                fg,
                super::ui::HAlign::Center,
                &layout,
            );
            self.hits.push((BarModule::KeyboardLayout, cell));
            cx_right += w;
        }
        if let Some(weather) = self.data.weather.clone() {
            let w = self.d.measure(cx, false, tok.font.caption, &weather) + 6.0 * 2.0;
            let cell = rect(cx_right, r.pos.y, w, r.size.y);
            self.d.label(
                cx,
                cell,
                false,
                tok.font.caption,
                fg,
                super::ui::HAlign::Center,
                &weather,
            );
            self.hits.push((BarModule::Weather, cell));
            cx_right += w;
        }
        if self.data.system_update {
            let cell = rect(cx_right, r.pos.y, status, r.size.y);
            self.d
                .icon_centered(cx, Ico::Refresh, cell, tok.font.caption * 1.3, accent);
            self.hits.push((BarModule::SystemUpdate, cell));
        }

        // ---- right: the window controls at the very edge (where the
        // platform draws none), then tray and the status modules, laid out
        // right to left
        let (controls_left, modules_right) =
            right_cluster_layout(r.pos.x + r.size.x, self.window_controls);
        if let Some(mut cx_ctrl) = controls_left {
            let controls = [
                (BarModule::WindowMin, Ico::WindowMin),
                (
                    BarModule::WindowMax,
                    if self.data.maximized { Ico::WindowRestore } else { Ico::WindowMax },
                ),
                (BarModule::WindowClose, Ico::Close),
            ];
            for (module, ico) in controls {
                let cell = rect(cx_ctrl, r.pos.y, WINDOW_CONTROL_WIDTH, r.size.y);
                if self.hover == Some(module) {
                    // The hovered control lights its slot, a close in the
                    // accent, like every desktop's caption buttons.
                    let wash = if module == BarModule::WindowClose { accent } else { fg };
                    self.d.solid(cx, cell, fade(wash, 0.18));
                }
                self.d.icon_centered(cx, ico, cell, canvas * 0.8, fg);
                self.hits.push((module, cell));
                cx_ctrl += WINDOW_CONTROL_WIDTH;
            }
        }
        let modules = self.right_modules();
        let mut rx = modules_right;
        for (module, ico, available) in modules.iter().rev() {
            rx -= slot;
            let cell = rect(rx, r.pos.y, slot, r.size.y);
            let mut color = if *available { fg } else { fade(fg, 0.45) };
            // The battery goes urgent below 20% on battery power.
            if *module == BarModule::Power {
                if let Some(b) = self.data.battery {
                    if !b.charging && b.percent <= 20 {
                        color = accent;
                    }
                }
            }
            if *module == BarModule::Audio && self.data.muted {
                color = fade(fg, 0.45);
            }
            self.d.icon_centered(cx, *ico, cell, canvas, color);
            self.hits.push((*module, cell));
        }

        // The open-panel pill, at the bar's inner (bottom) edge.
        if let Some(open) = self.data.open_panel {
            if let Some((_, cell)) = self.hits.iter().find(|(m, _)| *m == open) {
                let extent = (slot * 0.55).round().max(10.0);
                let pill = rect(
                    (cell.pos.x + (cell.size.x - extent) * 0.5).floor(),
                    r.pos.y + r.size.y - PANEL_PILL_INSET - PANEL_PILL_THICKNESS,
                    extent,
                    PANEL_PILL_THICKNESS,
                );
                self.d.solid(cx, pill, fade(accent, 0.9));
            }
        }

        // The hover tooltip, once the pointer has rested 400ms.
        if let Some(module) = self.hover {
            if self.hover_time >= TOOLTIP_DELAY {
                self.draw_tooltip(cx, r, module);
            }
        }
    }

    /// The tooltip a module shows after 400ms of hover.
    fn tooltip_for(&self, module: BarModule) -> String {
        match module {
            BarModule::Menu => "Omarchy menu".into(),
            BarModule::Workspace(i) => format!("Workspace {}", i + 1),
            BarModule::ActiveWindow => self
                .data
                .active_window
                .clone()
                .unwrap_or_default(),
            BarModule::Clock => "Calendar".into(),
            BarModule::KeyboardLayout => "Keyboard layout".into(),
            BarModule::Weather => "Weather".into(),
            BarModule::SystemUpdate => "Pending updates".into(),
            BarModule::Tray(_) => "Tray".into(),
            BarModule::Bluetooth => match self.data.bluetooth {
                Some(true) => "Bluetooth on".into(),
                Some(false) => "Bluetooth off".into(),
                None => "Bluetooth unavailable".into(),
            },
            BarModule::Network => match self.data.network {
                Some(true) => "Connected".into(),
                Some(false) => "Not connected".into(),
                None => "Network unavailable".into(),
            },
            BarModule::Audio => match (self.data.volume, self.data.muted) {
                (_, true) => "Muted".into(),
                (Some(v), _) => format!("Volume {}%", v),
                (None, _) => "Audio unavailable".into(),
            },
            BarModule::Monitor => "Display".into(),
            BarModule::Power => match self.data.battery {
                Some(b) if b.charging => format!("Battery {}%, charging", b.percent),
                Some(b) => format!("Battery {}%", b.percent),
                None => "Power".into(),
            },
            BarModule::Indicator(i) => self
                .data
                .indicators
                .get(i)
                .map(|ind| ind.tooltip.to_string())
                .unwrap_or_default(),
            BarModule::WindowMin => "Minimize".into(),
            BarModule::WindowMax => {
                if self.data.maximized { "Restore".into() } else { "Maximize".into() }
            }
            BarModule::WindowClose => "Close".into(),
        }
    }

    /// `Ui/PanelToolTip.qml`: the tooltip card, `bodySmall` inside
    /// `controlPaddingX/Y`, on `[tooltip] background` behind its 1px
    /// border, 6px off the bar edge.
    fn draw_tooltip(&mut self, cx: &mut Cx2d, r: Rect, module: BarModule) {
        let tok = self.tokens;
        let text = self.tooltip_for(module);
        if text.is_empty() {
            return;
        }
        let Some((_, cell)) = self.hits.iter().find(|(m, _)| *m == module).copied() else {
            return;
        };
        let px = tok.font.body_small;
        let tw = self.d.measure(cx, false, px, &text);
        let w = tw + tok.spacing.control_padding_x * 2.0;
        let h = px * 1.4 + tok.spacing.control_padding_y * 2.0;
        let x = (cell.pos.x + cell.size.x * 0.5 - w * 0.5)
            .max(r.pos.x + 2.0)
            .min(r.pos.x + r.size.x - w - 2.0)
            .floor();
        let y = (r.pos.y + r.size.y + 6.0).floor();
        let card = rect(x, y, w, h);
        self.d.card(cx, card, &tok.tooltip);
        self.d.label(
            cx,
            card,
            false,
            px,
            tok.tooltip.text,
            super::ui::HAlign::Center,
            &text,
        );
    }

    pub fn module_at(&self, p: Vec2d) -> Option<BarModule> {
        module_at_in(&self.hits, p)
    }

    /// The rect a module occupies — the panels anchor to it.
    pub fn module_rect(&self, module: BarModule) -> Option<Rect> {
        self.hits
            .iter()
            .find(|(m, _)| *m == module)
            .map(|(_, r)| *r)
    }
}

impl Widget for ShellBar {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let r = cx.turtle().rect();
        self.draw_bar(cx, r);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.inert {
            return;
        }
        let bar_rect = self.screen;
        if let Some(ne) = self.next_frame.is_event(event) {
            if self.hover.is_some() {
                let dt = if self.last_time <= 0.0 {
                    1.0 / 60.0
                } else {
                    (ne.time - self.last_time).clamp(0.001, 0.2)
                };
                self.last_time = ne.time;
                let before = self.hover_time;
                self.hover_time += dt;
                if before < TOOLTIP_DELAY && self.hover_time >= TOOLTIP_DELAY {
                    self.redraw(cx);
                }
                if self.hover_time < TOOLTIP_DELAY {
                    self.next_frame = cx.new_next_frame();
                }
            }
        }
        match event {
            Event::MouseMove(e) => {
                let over = contains(bar_rect, e.abs);
                let module = if over { self.module_at(e.abs) } else { None };
                let reveal = matches!(module, Some(BarModule::Indicator(_)))
                    || (over
                        && self
                            .hits
                            .iter()
                            .any(|(m, r)| matches!(m, BarModule::Indicator(_)) && contains(*r, e.abs)));
                if module != self.hover || reveal != self.reveal_indicators {
                    self.hover = module;
                    self.hover_time = 0.0;
                    self.last_time = 0.0;
                    if module.is_some() {
                        self.next_frame = cx.new_next_frame();
                    }
                    self.reveal_indicators = reveal;
                    if over {
                        cx.set_cursor(MouseCursor::Hand);
                    }
                    self.redraw(cx);
                }
            }
            Event::MouseDown(e) => {
                if let Some(module) = self.module_at(e.abs) {
                    cx.widget_action(self.uid, press_action(module, e.button));
                }
            }
            Event::Scroll(e) => {
                if contains(bar_rect, e.abs) && e.scroll.y.abs() > 0.5 {
                    if let Some(module) = self.module_at(e.abs) {
                        cx.widget_action(
                            self.uid,
                            ShellBarAction::Wheel(module, -e.scroll.y.signum()),
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_volume_ladder_matches_the_osd_thresholds() {
        assert_eq!(volume_icon(Some(0), false), Ico::Volume0);
        assert_eq!(volume_icon(Some(33), false), Ico::Volume1);
        assert_eq!(volume_icon(Some(34), false), Ico::Volume2);
        assert_eq!(volume_icon(Some(66), false), Ico::Volume2);
        assert_eq!(volume_icon(Some(67), false), Ico::Volume3);
        // Muted always reads as the silent glyph, whatever the level.
        assert_eq!(volume_icon(Some(90), true), Ico::Volume0);
    }

    #[test]
    fn the_window_controls_take_the_far_right_and_push_the_modules_left() {
        // Without controls the status modules end at the edge margin.
        assert_eq!(right_cluster_layout(1000.0, false), (None, 992.0));
        // With them: three slots at the edge, a gap, then the modules.
        let (controls, modules) = right_cluster_layout(1000.0, true);
        assert_eq!(controls, Some(992.0 - 3.0 * WINDOW_CONTROL_WIDTH));
        assert_eq!(modules, 992.0 - 3.0 * WINDOW_CONTROL_WIDTH - WINDOW_CONTROL_GAP);
        assert!(modules < controls.unwrap());
        // The default follows the platform: macOS keeps its own buttons.
        assert_eq!(window_controls_default(), !cfg!(target_os = "macos"));
    }

    /// The drag query asks the bar which points are BUTTONS: the window
    /// controls must claim theirs, or a press on Close would drag the
    /// window instead. `module_at` is that answer; the hit rects the draw
    /// records are what it reads, so this pins the rects the layout gives.
    #[test]
    fn the_window_controls_claim_their_points_from_the_drag_query() {
        // What draw_bar records for the controls, laid out by the same rule.
        let r = rect(0.0, 0.0, 1000.0, 26.0);
        let (controls_left, _) = right_cluster_layout(r.pos.x + r.size.x, true);
        let left = controls_left.unwrap();
        let mut hits: Vec<(BarModule, Rect)> = Vec::new();
        let mut x = left;
        for module in [BarModule::WindowMin, BarModule::WindowMax, BarModule::WindowClose] {
            hits.push((module, rect(x, r.pos.y, WINDOW_CONTROL_WIDTH, r.size.y)));
            x += WINDOW_CONTROL_WIDTH;
        }
        // Inside each control: claimed, by that control. Just left of the
        // first one: nobody's — a drag.
        let mid = r.pos.y + 13.0;
        assert_eq!(module_at_in(&hits, dvec2(left + 18.0, mid)), Some(BarModule::WindowMin));
        assert_eq!(module_at_in(&hits, dvec2(left + 54.0, mid)), Some(BarModule::WindowMax));
        assert_eq!(module_at_in(&hits, dvec2(left + 90.0, mid)), Some(BarModule::WindowClose));
        assert_eq!(module_at_in(&hits, dvec2(left - 2.0, mid)), None);
    }

    #[test]
    fn the_fixture_bar_has_the_shell_json_modules() {
        let data = BarData::fixture();
        assert_eq!(data.workspaces.len(), 5);
        assert!(data.workspaces[0].focused);
        assert_eq!(data.indicators.len(), 3);
        assert!(!data.clock.is_empty());
    }

    /// `ActiveWindow.qml`: left activates, middle and right both close —
    /// but they are DIFFERENT actions, not middle aliased to right, so a
    /// middle click on any other module (clock, audio, …) stays a no-op
    /// instead of firing that module's right-click behavior.
    #[test]
    fn middle_and_right_press_are_distinct_actions() {
        let m = BarModule::ActiveWindow;
        assert_eq!(press_action(m, MouseButton::PRIMARY), ShellBarAction::Press(m));
        assert_eq!(
            press_action(m, MouseButton::SECONDARY),
            ShellBarAction::RightPress(m)
        );
        assert_eq!(
            press_action(m, MouseButton::MIDDLE),
            ShellBarAction::MiddlePress(m)
        );
        assert_ne!(
            press_action(m, MouseButton::MIDDLE),
            press_action(m, MouseButton::SECONDARY)
        );
    }
}
