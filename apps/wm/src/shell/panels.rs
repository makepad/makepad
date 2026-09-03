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
use super::{alpha, darker, CtrlState, ShellTokens};

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
}

impl ShellPanel {
    pub fn toggle(&mut self, cx: &mut Cx, kind: PanelKind, anchor: Rect) {
        if self.open == Some(kind) {
            self.open = None;
        } else {
            self.open = Some(kind);
            self.anchor = anchor;
        }
        self.redraw(cx);
    }

    pub fn close(&mut self, cx: &mut Cx) {
        self.open = None;
        self.redraw(cx);
    }

    /// A live theme switch: new tokens, redraw.
    pub fn set_tokens(&mut self, cx: &mut Cx, tokens: ShellTokens) {
        self.tokens = tokens;
        self.redraw(cx);
    }

    /// The card rect: under the bar, centered on the module, `gapsOut` off
    /// the bar edge, clamped into the screen by the same margin.
    fn card_rect(&self, screen: Rect, kind: PanelKind, height: f64) -> Rect {
        let margin = self.tokens.spacing.gaps_out;
        let w = kind.content_width() + self.tokens.spacing.popup_padding * 2.0;
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
        let fg = if enabled {
            tok.popups.text
        } else {
            alpha(tok.popups.text, 0.4)
        };
        self.d.panel_slider(
            cx,
            track,
            &tok,
            progress,
            fg,
            tok.popups.background,
            hot && enabled,
        );
        self.hits.push((hit, r));
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

    /// Draw the whole surface into `screen`. Under glass it draws in its
    /// own overlay list, claimed every frame, open or not.
    pub fn draw_surface(&mut self, cx: &mut Cx2d, screen: Rect) {
        let tokens = self.tokens;
        self.d.begin_surface(cx, &tokens);
        self.draw_surface_inner(cx, screen);
        self.d.end_surface(cx);
    }

    fn draw_surface_inner(&mut self, cx: &mut Cx2d, screen: Rect) {
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
            PanelKind::Audio => 300.0,
            PanelKind::Power => 300.0,
            PanelKind::Monitor => 300.0,
            _ => 150.0,
        } + pad * 2.0;
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
        ((p.x - track.pos.x) / track.size.x.max(1.0)).clamp(0.0, 1.0)
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
                        Hit::VolumeSlider => cx.widget_action(
                            self.uid,
                            ShellPanelAction::SetVolume((v * 100.0).round() as u32),
                        ),
                        Hit::BrightnessSlider => cx.widget_action(
                            self.uid,
                            ShellPanelAction::SetBrightness((v * 100.0).round() as u32),
                        ),
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
                    self.open = None;
                    cx.widget_action(self.uid, ShellPanelAction::Close);
                    self.redraw(cx);
                    return;
                }
                match self.hit_at(e.abs) {
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
                    Some(Hit::InputSlider) => {}
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
                self.dragging = None;
            }
            _ => {}
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
