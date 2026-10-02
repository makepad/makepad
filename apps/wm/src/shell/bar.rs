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
    Style,
    Appearance,
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

/// One screen's share of the bar strip on a multi-screen desktop: the
/// screen's x range (desk coordinates, which the strip shares), what it
/// shows, and whether it is the active screen's (only that one wears the
/// focused workspace at full strength).
#[derive(Clone, Debug)]
pub struct BarSegment {
    pub x0: f64,
    pub x1: f64,
    pub data: BarData,
    pub active: bool,
}

/// One layout's workspace cluster: the active workspace (shown as a dot,
/// even when empty) and every populated one, by number (workspace 10 reads
/// "0"); empty ones are hidden. The second Vec is the workspace each cell
/// stands for, what a click on it maps to.
pub fn workspace_cells(layout: &crate::layout::WmLayout) -> (Vec<WorkspaceCell>, Vec<usize>) {
    let mut cells = Vec::new();
    let mut shown = Vec::new();
    for i in 0..crate::layout::WORKSPACES {
        let populated = !layout.clients_on(i).is_empty();
        if i != layout.active && !populated {
            continue;
        }
        shown.push(i);
        cells.push(WorkspaceCell {
            label: format!("{}", (i + 1) % 10),
            occupied: populated,
            focused: i == layout.active,
        });
    }
    (cells, shown)
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
    pub style: crate::desktop::DesktopStyle,
    pub dark: bool,
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
            style: Default::default(),
            dark: false,
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

/// The wall clock in local time, read in-process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i64,
    /// 1–12.
    pub month: u32,
    /// 1–31.
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    /// 0 = Monday.
    pub weekday: u32,
}

/// Now, in the local time zone, from the shared host clock
/// (`localtime_r` on Unix, `SystemTimeToTzSpecificLocalTime` on Windows).
/// The sampler used to run `date` twice a second; on the phone each
/// fork+exec of a process with the WM's address space cost about 5% of a
/// core and showed up in every swipe profile as `execve`.
pub fn local_now() -> Option<LocalTime> {
    let now = makepad_civil_time::local(makepad_civil_time::now_secs())?;
    Some(LocalTime {
        year: now.year as i64,
        month: now.month,
        day: now.day,
        hour: now.hour,
        minute: now.minute,
        weekday: now.weekday(),
    })
}

/// English full weekday names, Monday first — `date`'s `%A`.
const WEEKDAY_FULL: [&str; 7] =
    ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

/// `%A %H:%M` — omarchy's `dddd HH:mm`; `alt` is `%-d %B W%V %Y`.
pub fn sample_clock(alt: bool) -> String {
    let Some(now) = local_now() else { return String::new() };
    if alt {
        format!(
            "{} {} W{:02} {}",
            now.day,
            super::panels::MONTH_NAMES[(now.month - 1) as usize],
            super::panels::iso_week(now.year, now.month, now.day),
            now.year
        )
    } else {
        format!("{} {:02}:{:02}", WEEKDAY_FULL[now.weekday as usize], now.hour, now.minute)
    }
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
                // The clock is not sampled here: `localtime_r` reads the
                // process environment, which the UI thread writes (child
                // env, MAKEPAD_WM_ROOT); the UI thread formats it itself.
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
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    {
        // A default route in the kernel's table, read straight from procfs:
        // no process, no daemon. Wired or wireless alike — the Wi-Fi
        // dropdown says which.
        let table = std::fs::read_to_string("/proc/net/route").ok()?;
        Some(table.lines().skip(1).any(|line| {
            let mut fields = line.split_whitespace();
            fields.next().is_some() && fields.next() == Some("00000000")
        }))
    }
    #[cfg(not(any(target_os = "macos", all(target_os = "linux", not(target_env = "ohos")))))]
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

/// The segment and module whose recorded hit rect holds `p` — what the
/// WM's drag query asks (`shell_bar_claims`): a claimed point is a button,
/// not a drag.
pub fn hit_at_in(hits: &[(usize, BarModule, Rect)], p: Vec2d) -> Option<(usize, BarModule)> {
    hits.iter().find(|(_, _, r)| contains(*r, p)).map(|(s, m, _)| (*s, *m))
}

/// A bar module with a flyout was pressed in segment `seg` while
/// `same_open` (its own flyout is already up) in segment `open_seg`: true
/// when the press should move that flyout to `seg` rather than toggle it
/// closed (the same module on another screen's segment).
pub fn flyout_press_moves(same_open: bool, open_seg: usize, seg: usize) -> bool {
    same_open && seg != open_seg
}

/// Each segment's rect across the strip `r`, from the screens' x ranges
/// (`spans`, left to right): the first starts at the strip's left edge
/// (the AI pane may clip the leftmost screen, never the bar), each ends
/// where the next begins, and the last ends at the strip's right edge.
/// No segment, or one, is the whole strip — today's bar.
pub fn segment_rects(r: Rect, spans: &[(f64, f64)]) -> Vec<Rect> {
    if spans.len() <= 1 {
        return vec![r];
    }
    let left = r.pos.x;
    let right = r.pos.x + r.size.x;
    let mut starts: Vec<f64> = Vec::with_capacity(spans.len());
    for (i, (x0, _)) in spans.iter().enumerate() {
        let prev = starts.last().copied().unwrap_or(left);
        starts.push(if i == 0 { left } else { x0.clamp(prev, right) });
    }
    (0..spans.len())
        .map(|i| {
            let x1 = starts.get(i + 1).copied().unwrap_or(right);
            rect(starts[i], r.pos.y, x1 - starts[i], r.size.y)
        })
        .collect()
}

/// The left cluster of segment `seg` drawn into `r`: the menu button then
/// one pill per workspace, as hit rects, and the x the style switch starts
/// at. Pure, so the bar's layout is tested without a window.
pub fn left_cluster(
    seg: usize,
    r: Rect,
    pad_left: f64,
    canvas: f64,
    workspaces: usize,
) -> (Vec<(usize, BarModule, Rect)>, f64) {
    let mut hits = Vec::with_capacity(workspaces + 1);
    let mut x = r.pos.x + pad_left.max(EDGE_MARGIN);
    let menu_w = (canvas + MENU_MARGIN * 2.0).max(12.0);
    hits.push((seg, BarModule::Menu, rect(x, r.pos.y, menu_w, r.size.y)));
    x += menu_w;
    for i in 0..workspaces {
        hits.push((seg, BarModule::Workspace(i), rect(x, r.pos.y, WS_WIDTH, r.size.y)));
        x += WS_WIDTH + WS_SPACING;
    }
    (hits, x + WS_TRAILING)
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

/// The presses carry the segment they landed in (0 on a one-segment bar):
/// on a multi-screen desktop segment `i` is live screen `i`'s.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ShellBarAction {
    /// A module was pressed (left button).
    Press(usize, BarModule),
    /// A module was right-pressed — the clock cycles its format, audio
    /// mutes, power toggles the percentage.
    RightPress(usize, BarModule),
    /// A module was middle-pressed (`ActiveWindow.qml`'s
    /// `Qt.MiddleButton` arm: only the active-window title answers to
    /// this, closing the focused client exactly like a right click does).
    MiddlePress(usize, BarModule),
    /// Wheel over a module: +1 / -1 notch.
    Wheel(BarModule, f64),
    #[default]
    None,
}

/// `MouseDown.button` -> the press action it raises. Middle is checked
/// before secondary — a mouse reporting both bits on the same event (some
/// drivers do, for a chorded click) still reads as the tertiary button,
/// matching `ActiveWindow.qml`'s own `Qt.MiddleButton` branch order.
fn press_action(seg: usize, module: BarModule, button: MouseButton) -> ShellBarAction {
    if button.contains(MouseButton::MIDDLE) {
        ShellBarAction::MiddlePress(seg, module)
    } else if button.contains(MouseButton::SECONDARY) {
        ShellBarAction::RightPress(seg, module)
    } else {
        ShellBarAction::Press(seg, module)
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
    /// One per live screen on a multi-screen desktop, left to right; empty
    /// (or one) draws `data` across the whole strip, as always.
    #[rust]
    pub segments: Vec<BarSegment>,
    /// Content starts here, so the OS window buttons stay clear.
    #[rust]
    pub pad_left: f64,
    #[rust]
    area: Area,
    /// The rect the bar was last drawn into — the gallery drives these
    /// surfaces directly, without a turtle of their own.
    #[rust]
    screen: Rect,
    /// (segment, module, rect), recorded by the last draw.
    #[rust]
    hits: Vec<(usize, BarModule, Rect)>,
    #[rust]
    hover: Option<(usize, BarModule)>,
    #[rust]
    hover_time: f64,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_time: f64,
    /// The indicator block reveals its inactive glyphs while hovered (in
    /// the hovered segment only).
    #[rust]
    reveal_indicators: Option<usize>,
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

    fn icon_slot(&self) -> f64 {
        self.tokens.bar.icon_slot
    }

    fn status_slot(&self) -> f64 {
        self.tokens.bar.status_slot
    }

    /// The status modules on the right, in `shell.json` order, with the
    /// icon each one shows right now.
    fn right_modules(data: &BarData) -> Vec<(BarModule, Ico, bool)> {
        let mut v: Vec<(BarModule, Ico, bool)> = Vec::new();
        for (i, ico) in data.tray.iter().enumerate() {
            v.push((BarModule::Tray(i), *ico, true));
        }
        v.push((
            BarModule::Bluetooth,
            match data.bluetooth {
                Some(true) => Ico::Bluetooth,
                _ => Ico::BluetoothOff,
            },
            data.bluetooth.is_some(),
        ));
        v.push((
            BarModule::Network,
            match data.network {
                Some(true) => Ico::Wifi,
                _ => Ico::WifiOff,
            },
            data.network.is_some(),
        ));
        v.push((
            BarModule::Audio,
            volume_icon(data.volume, data.muted),
            data.volume.is_some(),
        ));
        v.push((
            BarModule::Monitor,
            Ico::Monitor,
            data.brightness.is_some(),
        ));
        v.push((
            BarModule::Power,
            if data.battery.is_some() {
                Ico::Battery
            } else {
                Ico::Power
            },
            data.battery.is_some(),
        ));
        v
    }

    /// Draw the bar into `r`: `data` across the whole strip, or with two
    /// or more `segments` each screen's share of it. Hit rects are recorded
    /// for the next event pass.
    pub fn draw_bar(&mut self, cx: &mut Cx2d, r: Rect) {
        self.hits.clear();
        self.screen = r;
        let segments = std::mem::take(&mut self.segments);
        let spans: Vec<(f64, f64)> = segments.iter().map(|s| (s.x0, s.x1)).collect();
        let rects = segment_rects(r, &spans);
        let datas: Vec<BarData> = if segments.len() <= 1 {
            vec![segments.first().map_or_else(|| self.data.clone(), |s| s.data.clone())]
        } else {
            segments.iter().map(|s| s.data.clone()).collect()
        };
        let last = rects.len() - 1;
        for (i, (sr, data)) in rects.iter().zip(&datas).enumerate() {
            // One segment is the whole bar and always the active one.
            let active = last == 0 || segments[i].active;
            self.draw_segment(cx, i, data, *sr, i == 0, i == last, active);
        }
        // The hover tooltip, once the pointer has rested 400ms, over
        // everything else.
        if let Some((seg, module)) = self.hover {
            if self.hover_time >= TOOLTIP_DELAY {
                if let (Some(sr), Some(data)) = (rects.get(seg), datas.get(seg)) {
                    self.draw_tooltip(cx, *sr, data, seg, module);
                }
            }
        }
        self.segments = segments;
    }

    /// One segment: menu, workspaces, style switch and the window title on
    /// the left, the clock centred on the segment, the status modules on
    /// the right. The OS caption inset (`pad_left`) applies to the first
    /// segment only and the window controls to the last; an inactive
    /// segment dims its focused workspace.
    #[allow(clippy::too_many_arguments)]
    fn draw_segment(
        &mut self,
        cx: &mut Cx2d,
        seg: usize,
        data: &BarData,
        r: Rect,
        first: bool,
        last: bool,
        active: bool,
    ) {
        let tok = self.tokens;
        let fg = tok.bar.text;
        let accent = tok.bar.active;
        let slot = self.icon_slot();
        let canvas = tok.bar.icon_canvas;
        let hit_start = self.hits.len();

        // The strip itself: `Color.bar.background`, no border.
        self.draw_bg.color = alpha(tok.bar.background, tok.bar.background_alpha);
        self.draw_bg.draw_abs(cx, r);

        // ---- left: menu, workspaces
        let pad_left = if first { self.pad_left } else { 0.0 };
        let (left, mut x) = left_cluster(seg, r, pad_left, canvas, data.workspaces.len());
        let menu_rect = left[0].2;
        self.d
            .icon_centered(cx, Ico::Menu, menu_rect, canvas, fg);
        for (ws, (_, _, cell)) in data.workspaces.iter().zip(&left[1..]) {
            let cell = *cell;
            let lit = ws.occupied || ws.focused;
            let color = fade(fg, if lit { 1.0 } else { 0.5 });
            if ws.focused {
                // The focused workspace is a dot, not its number; another
                // screen's focused workspace is a dimmed one.
                let color = if active { color } else { fade(fg, 0.45) };
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
        }
        self.hits.extend(left);
        let style_label = format!("{}  ▾", data.style.label());
        let style_width = self.d.measure(cx, false, tok.font.body, &style_label) + 18.0;
        let style_rect = rect(x, r.pos.y, style_width, r.size.y);
        self.d.label(cx, style_rect, false, tok.font.body, fg, super::ui::HAlign::Center, &style_label);
        self.hits.push((seg, BarModule::Style, style_rect));
        x += style_width + 6.0;

        if data.style.supports_dark() {
            let label=if data.dark {"Dark"}else{"Light"};
            let width=self.d.measure(cx,false,tok.font.body,label)+18.0;
            let button=rect(x,r.pos.y,width,r.size.y);
            self.d.label(cx,button,false,tok.font.body,fg,super::ui::HAlign::Center,label);
            self.hits.push((seg, BarModule::Appearance,button));
            x+=width+6.0;
        }
        // The active window's title: `min(280, implicitWidth) + controlPaddingX*2`,
        // `body` at α .85, elided right, with the full title in the tooltip.
        if let Some(title) = data.active_window.as_deref() {
            let text_w = self
                .d
                .measure(cx, false, tok.font.body, title)
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
                title,
            );
            self.hits.push((seg, BarModule::ActiveWindow, cell));
        }

        // ---- center: the clock is the anchor, centered on the bar itself
        let clock_w = self
            .d
            .measure(cx, false, tok.font.body, &data.clock)
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
            &data.clock,
        );
        self.hits.push((seg, BarModule::Clock, clock_rect));

        // Indicators sit flush against the anchor's left edge.
        let status = self.status_slot();
        let indicators = &data.indicators;
        let mut ix = clock_rect.pos.x;
        for (i, ind) in indicators.iter().enumerate().rev() {
            let visible = ind.active || self.reveal_indicators == Some(seg);
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
            self.hits.push((seg, BarModule::Indicator(i), cell));
        }

        // Keyboard layout, weather and the update dot follow the anchor.
        let mut cx_right = clock_rect.pos.x + clock_rect.size.x;
        if let Some(layout) = data.keyboard_layout.as_deref() {
            let w = self.d.measure(cx, false, tok.font.caption, layout) + 6.0 * 2.0;
            let cell = rect(cx_right, r.pos.y, w, r.size.y);
            self.d.label(
                cx,
                cell,
                false,
                tok.font.caption,
                fg,
                super::ui::HAlign::Center,
                layout,
            );
            self.hits.push((seg, BarModule::KeyboardLayout, cell));
            cx_right += w;
        }
        if let Some(weather) = data.weather.as_deref() {
            let w = self.d.measure(cx, false, tok.font.caption, weather) + 6.0 * 2.0;
            let cell = rect(cx_right, r.pos.y, w, r.size.y);
            self.d.label(
                cx,
                cell,
                false,
                tok.font.caption,
                fg,
                super::ui::HAlign::Center,
                weather,
            );
            self.hits.push((seg, BarModule::Weather, cell));
            cx_right += w;
        }
        if data.system_update {
            let cell = rect(cx_right, r.pos.y, status, r.size.y);
            self.d
                .icon_centered(cx, Ico::Refresh, cell, tok.font.caption * 1.3, accent);
            self.hits.push((seg, BarModule::SystemUpdate, cell));
        }

        // ---- right: the window controls at the very edge (where the
        // platform draws none), then tray and the status modules, laid out
        // right to left
        let (controls_left, modules_right) =
            right_cluster_layout(r.pos.x + r.size.x, self.window_controls && last);
        if let Some(mut cx_ctrl) = controls_left {
            let controls = [
                (BarModule::WindowMin, Ico::WindowMin),
                (
                    BarModule::WindowMax,
                    if data.maximized { Ico::WindowRestore } else { Ico::WindowMax },
                ),
                (BarModule::WindowClose, Ico::Close),
            ];
            for (module, ico) in controls {
                let cell = rect(cx_ctrl, r.pos.y, WINDOW_CONTROL_WIDTH, r.size.y);
                if self.hover == Some((seg, module)) {
                    // The hovered control lights its slot, a close in the
                    // accent, like every desktop's caption buttons.
                    let wash = if module == BarModule::WindowClose { accent } else { fg };
                    self.d.solid(cx, cell, fade(wash, 0.18));
                }
                self.d.icon_centered(cx, ico, cell, canvas * 0.8, fg);
                self.hits.push((seg, module, cell));
                cx_ctrl += WINDOW_CONTROL_WIDTH;
            }
        }
        let modules = Self::right_modules(data);
        let mut rx = modules_right;
        for (module, ico, available) in modules.iter().rev() {
            #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
            if *module == BarModule::Power {
                if let Some(battery) = data.battery {
                    let label = format!("{}%{}", battery.percent, if battery.charging { " +" } else { "" });
                    let width = self.d.measure(cx, false, tok.font.caption, &label) + 8.0;
                    rx -= width;
                    let label_rect = rect(rx, r.pos.y, width, r.size.y);
                    self.d.label(cx, label_rect, false, tok.font.caption, fg, super::ui::HAlign::Center, &label);
                    self.hits.push((seg, *module, label_rect));
                }
            }
            rx -= slot;
            let cell = rect(rx, r.pos.y, slot, r.size.y);
            let mut color = if *available { fg } else { fade(fg, 0.45) };
            // The battery goes urgent below 20% on battery power.
            if *module == BarModule::Power {
                if let Some(b) = data.battery {
                    if !b.charging && b.percent <= 20 {
                        color = accent;
                    }
                }
            }
            if *module == BarModule::Audio && data.muted {
                color = fade(fg, 0.45);
            }
            self.d.icon_centered(cx, *ico, cell, canvas, color);
            self.hits.push((seg, *module, cell));
        }

        // The open-panel pill, at the bar's inner (bottom) edge.
        if let Some(open) = data.open_panel {
            if let Some((_, _, cell)) = self.hits[hit_start..].iter().find(|(_, m, _)| *m == open) {
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
    }

    /// The tooltip a module shows after 400ms of hover.
    fn tooltip_for(data: &BarData, module: BarModule) -> String {
        match module {
            BarModule::Style => "Choose operating system style".into(),
            BarModule::Appearance => "Toggle light / dark appearance".into(),
            BarModule::Menu => "Applications".into(),
            BarModule::Workspace(i) => format!("Workspace {}", i + 1),
            BarModule::ActiveWindow => data.active_window
                .clone()
                .unwrap_or_default(),
            BarModule::Clock => "Calendar".into(),
            BarModule::KeyboardLayout => "Keyboard layout".into(),
            BarModule::Weather => "Weather".into(),
            BarModule::SystemUpdate => "Pending updates".into(),
            BarModule::Tray(_) => "Tray".into(),
            BarModule::Bluetooth => match data.bluetooth {
                Some(true) => "Bluetooth on".into(),
                Some(false) => "Bluetooth off".into(),
                None => "Bluetooth unavailable".into(),
            },
            BarModule::Network => match data.network {
                Some(true) => "Connected".into(),
                Some(false) => "Not connected".into(),
                None => "Network unavailable".into(),
            },
            BarModule::Audio => match (data.volume, data.muted) {
                (_, true) => "Muted".into(),
                (Some(v), _) => format!("Volume {}%", v),
                (None, _) => "Audio unavailable".into(),
            },
            BarModule::Monitor => "Display".into(),
            BarModule::Power => match data.battery {
                Some(b) if b.charging => format!("Battery {}%, charging", b.percent),
                Some(b) => format!("Battery {}%", b.percent),
                None => "Power".into(),
            },
            BarModule::Indicator(i) => data.indicators
                .get(i)
                .map(|ind| ind.tooltip.to_string())
                .unwrap_or_default(),
            BarModule::WindowMin => "Minimize".into(),
            BarModule::WindowMax => {
                if data.maximized { "Restore".into() } else { "Maximize".into() }
            }
            BarModule::WindowClose => "Close".into(),
        }
    }

    /// `Ui/PanelToolTip.qml`: the tooltip card, `bodySmall` inside
    /// `controlPaddingX/Y`, on `[tooltip] background` behind its 1px
    /// border, 6px off the bar edge.
    fn draw_tooltip(&mut self, cx: &mut Cx2d, r: Rect, data: &BarData, seg: usize, module: BarModule) {
        let tok = self.tokens;
        let text = Self::tooltip_for(data, module);
        if text.is_empty() {
            return;
        }
        let Some((_, _, cell)) = self.hits.iter().find(|(s, m, _)| *s == seg && *m == module).copied() else {
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
        self.hit_at(p).map(|(_, m)| m)
    }

    /// The segment and module under `p`.
    pub fn hit_at(&self, p: Vec2d) -> Option<(usize, BarModule)> {
        hit_at_in(&self.hits, p)
    }

    /// The rect a module occupies in segment `seg` — the panels anchor to
    /// it, so one opened from a segment opens on that screen. A segment
    /// the bar no longer has falls back to the module's first rect.
    pub fn module_rect(&self, seg: usize, module: BarModule) -> Option<Rect> {
        self.hits
            .iter()
            .find(|(s, m, _)| *s == seg && *m == module)
            .or_else(|| self.hits.iter().find(|(_, m, _)| *m == module))
            .map(|(_, _, r)| *r)
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
                let module = if over { self.hit_at(e.abs) } else { None };
                let reveal = match module {
                    Some((seg, BarModule::Indicator(_))) => Some(seg),
                    _ => None,
                };
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
                if let Some((seg, module)) = self.hit_at(e.abs) {
                    cx.widget_action(self.uid, press_action(seg, module, e.button));
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
    fn the_same_flyout_pressed_on_another_segment_moves() {
        // Open on segment 0, pressed again on segment 1: it moves.
        assert!(flyout_press_moves(true, 0, 1));
        // Pressed again on its own segment: it toggles closed.
        assert!(!flyout_press_moves(true, 1, 1));
        // Another flyout (or none) up: the press opens this one.
        assert!(!flyout_press_moves(false, 0, 1));
        assert!(!flyout_press_moves(false, 0, 0));
    }

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
        let mut hits: Vec<(usize, BarModule, Rect)> = Vec::new();
        let mut x = left;
        for module in [BarModule::WindowMin, BarModule::WindowMax, BarModule::WindowClose] {
            hits.push((0, module, rect(x, r.pos.y, WINDOW_CONTROL_WIDTH, r.size.y)));
            x += WINDOW_CONTROL_WIDTH;
        }
        // Inside each control: claimed, by that control. Just left of the
        // first one: nobody's — a drag.
        let mid = r.pos.y + 13.0;
        assert_eq!(hit_at_in(&hits, dvec2(left + 18.0, mid)), Some((0, BarModule::WindowMin)));
        assert_eq!(hit_at_in(&hits, dvec2(left + 54.0, mid)), Some((0, BarModule::WindowMax)));
        assert_eq!(hit_at_in(&hits, dvec2(left + 90.0, mid)), Some((0, BarModule::WindowClose)));
        assert_eq!(hit_at_in(&hits, dvec2(left - 2.0, mid)), None);
    }

    fn layout_fixture() -> crate::layout::WmLayout {
        let mut l = crate::layout::WmLayout::new();
        let area = crate::layout::LRect::new(0.0, 0.0, 1000.0, 800.0);
        l.insert_on(0, 1, area, 4.0);
        l.insert_on(3, 2, area, 4.0);
        l.insert_on(9, 3, area, 4.0);
        // The active workspace is the empty second one.
        l.switch_workspace(1);
        l
    }

    /// Omarchy's cluster: the active workspace (a dot, even when empty),
    /// the populated ones by number (workspace 10 reads "0"), the empty
    /// ones hidden; `shown` maps each cell back to its workspace.
    #[test]
    fn workspace_cells_show_the_active_and_the_populated_workspaces() {
        let (cells, shown) = workspace_cells(&layout_fixture());
        assert_eq!(shown, vec![0, 1, 3, 9]);
        let labels: Vec<&str> = cells.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["1", "2", "4", "0"]);
        let occupied: Vec<bool> = cells.iter().map(|c| c.occupied).collect();
        assert_eq!(occupied, vec![true, false, true, true]);
        let focused: Vec<bool> = cells.iter().map(|c| c.focused).collect();
        assert_eq!(focused, vec![false, true, false, false]);
    }

    /// No segments, or one, is today's bar: the whole strip, whatever x
    /// range the one screen has (the AI pane clips the desk, not the bar),
    /// and the left cluster records the same hits it always did.
    #[test]
    fn one_segment_draws_the_same_hits_as_the_bare_bar() {
        let r = rect(0.0, 0.0, 1920.0, 26.0);
        assert_eq!(segment_rects(r, &[]), vec![r]);
        assert_eq!(segment_rects(r, &[(400.0, 1520.0)]), vec![r]);
        let data = BarData::fixture();
        let canvas = 16.0;
        let n = data.workspaces.len();
        let (bare, bare_x) = left_cluster(0, r, 0.0, canvas, n);
        let (one, one_x) = left_cluster(0, segment_rects(r, &[(400.0, 1520.0)])[0], 0.0, canvas, n);
        assert_eq!(bare, one);
        assert_eq!(bare_x, one_x);
        // Today's arithmetic: the menu off the edge margin, then the pills.
        let menu_w = (canvas + MENU_MARGIN * 2.0).max(12.0);
        assert_eq!(bare[0], (0, BarModule::Menu, rect(EDGE_MARGIN, 0.0, menu_w, 26.0)));
        for i in 0..n {
            let x = EDGE_MARGIN + menu_w + i as f64 * (WS_WIDTH + WS_SPACING);
            assert_eq!(bare[1 + i], (0, BarModule::Workspace(i), rect(x, 0.0, WS_WIDTH, 26.0)));
        }
        assert_eq!(bare_x, EDGE_MARGIN + menu_w + n as f64 * (WS_WIDTH + WS_SPACING) + WS_TRAILING);
    }

    /// Two screens share the strip: each segment spans its screen's x
    /// range, the first stretched to the strip's left edge (the AI pane may
    /// clip the desk) and the last to its right edge, and a click resolves
    /// to the segment it lands in.
    #[test]
    fn a_click_in_segment_one_returns_segment_ones_module() {
        let r = rect(0.0, 0.0, 3840.0, 26.0);
        let segs = segment_rects(r, &[(400.0, 1920.0), (1920.0, 3800.0)]);
        assert_eq!(segs, vec![rect(0.0, 0.0, 1920.0, 26.0), rect(1920.0, 0.0, 1920.0, 26.0)]);
        let (mut hits, _) = left_cluster(0, segs[0], 0.0, 16.0, 3);
        hits.extend(left_cluster(1, segs[1], 0.0, 16.0, 2).0);
        let center = |seg: usize, m: BarModule| {
            let r = hits.iter().find(|(s, mm, _)| *s == seg && *mm == m).unwrap().2;
            dvec2(r.pos.x + r.size.x * 0.5, r.pos.y + r.size.y * 0.5)
        };
        // Segment 1's cluster starts off ITS left edge.
        assert_eq!(hits.iter().find(|(s, m, _)| *s == 1 && *m == BarModule::Menu).unwrap().2.pos.x, 1920.0 + EDGE_MARGIN);
        assert_eq!(hit_at_in(&hits, center(1, BarModule::Workspace(1))), Some((1, BarModule::Workspace(1))));
        assert_eq!(hit_at_in(&hits, center(0, BarModule::Workspace(1))), Some((0, BarModule::Workspace(1))));
        assert_eq!(hit_at_in(&hits, center(1, BarModule::Menu)), Some((1, BarModule::Menu)));
        assert_eq!(hit_at_in(&hits, dvec2(1000.0, 13.0)), None);
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
        assert_eq!(press_action(1, m, MouseButton::PRIMARY), ShellBarAction::Press(1, m));
        assert_eq!(
            press_action(1, m, MouseButton::SECONDARY),
            ShellBarAction::RightPress(1, m)
        );
        assert_eq!(
            press_action(1, m, MouseButton::MIDDLE),
            ShellBarAction::MiddlePress(1, m)
        );
        assert_ne!(
            press_action(1, m, MouseButton::MIDDLE),
            press_action(1, m, MouseButton::SECONDARY)
        );
    }
}
