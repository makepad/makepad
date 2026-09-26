//! The clock surface: the analog face, the alarm book, the stopwatch and
//! the countdown timer, and the DSL for both resident faces — the full
//! clock and the compact home-widget tile the host switches to with
//! `HostedViewMode` over `Event::Custom` (the same message a standalone
//! window and a module's isolate both receive; the `HostedView` child
//! below reads it and swaps faces on its own — nothing here parses it).
//!
//! The full face is one quiet App background with the time typography
//! dominant and one floating navigation capsule; each tab keeps its own
//! page. The alarm editor is a bottom sheet; a ringing alarm or a finished
//! timer is a centred modal panel. The tile is the analog dial by
//! default, a digital face by choice, and whichever activity is live
//! (a ringing alarm, a finished or running timer, a running stopwatch)
//! takes it over while it lasts.
//!
//! Everything the standalone window's `App` used to own — the tick and
//! fast timers, the stopwatch and countdown state, the alarm book and its
//! persistence, the alarm sound — lives on [`ClockView`], so a module
//! host gets the same clock a window does. [`ClockView::ensure_started`]
//! seeds it on the first event or draw, whichever the host gives it
//! first: a window has a `Startup` event, a module instance does not.

use crate::alarm::{self, AlarmBook};
use crate::alarm_list::{AlarmList, AlarmListAction};
use crate::digits::TabularLabel;
use crate::face::ClockFace;
use crate::laps::LapList;
use crate::local_time::LocalTime;
use crate::ring::TimerRing;
use crate::tabs::{PhoneTabs, PhoneTabsAction};
use crate::wheel::TimeWheel;
use makepad_widgets::makepad_platform::storage::{StorageHandle, StorageRequestId, StorageResponse, StorageResult};
use makepad_widgets::*;
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::time::Instant;

/// Elapsed time accumulated across start/pause cycles, with the split
/// times at which laps were taken (each lap's duration is the difference).
#[derive(Default)]
pub struct Stopwatch {
    base: f64,
    started: Option<Instant>,
    marks: Vec<f64>,
}

impl Stopwatch {
    pub fn running(&self) -> bool {
        self.started.is_some()
    }
    pub fn elapsed(&self) -> f64 {
        self.base + self.started.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0)
    }
    pub fn toggle(&mut self) {
        match self.started.take() {
            Some(s) => self.base += s.elapsed().as_secs_f64(),
            None => self.started = Some(Instant::now()),
        }
    }
    pub fn reset(&mut self) {
        self.base = 0.0;
        self.started = None;
        self.marks.clear();
    }
    pub fn lap(&mut self) {
        if self.running() && self.marks.len() < 99 {
            self.marks.push(self.elapsed());
        }
    }
    /// Completed lap durations, oldest first.
    pub fn laps(&self) -> Vec<f64> {
        let mut last = 0.0;
        self.marks
            .iter()
            .map(|m| {
                let d = m - last;
                last = *m;
                d
            })
            .collect()
    }
    /// The current lap's running duration, once laps exist.
    pub fn current_lap(&self) -> Option<f64> {
        if self.marks.is_empty() && !self.running() {
            return None;
        }
        Some(self.elapsed() - self.marks.last().copied().unwrap_or(0.0))
    }
}

/// A countdown that counts wall time while running; it only ever reports
/// reaching zero on screen (no notification is delivered).
pub struct Countdown {
    /// The duration Reset returns to, in seconds.
    pub preset: f64,
    /// Seconds left at the moment it was last started or paused.
    remaining_at: f64,
    started: Option<Instant>,
    /// The person's label for this timer.
    pub label: String,
}

impl Default for Countdown {
    fn default() -> Self {
        Self { preset: 300.0, remaining_at: 300.0, started: None, label: String::new() }
    }
}

impl Countdown {
    pub fn running(&self) -> bool {
        self.started.is_some()
    }
    pub fn remaining(&self) -> f64 {
        let run = self.started.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
        (self.remaining_at - run).max(0.0)
    }
    pub fn finished(&self) -> bool {
        self.remaining() <= 0.0 && self.preset > 0.0
    }
    /// True while a timer is set: running, paused mid-way, or done.
    pub fn active(&self) -> bool {
        self.running() || self.remaining_at < self.preset || self.finished()
    }
    /// Freeze the running clock into `remaining_at`; stops when it hit zero.
    pub fn settle(&mut self) {
        if let Some(s) = self.started {
            let left = (self.remaining_at - s.elapsed().as_secs_f64()).max(0.0);
            if left <= 0.0 {
                self.remaining_at = 0.0;
                self.started = None;
            }
        }
    }
    pub fn start(&mut self, seconds: f64) {
        self.preset = seconds.clamp(0.0, 24.0 * 3600.0);
        self.remaining_at = self.preset;
        self.started = if self.preset > 0.0 { Some(Instant::now()) } else { None };
    }
    pub fn toggle(&mut self) {
        match self.started.take() {
            Some(s) => self.remaining_at = (self.remaining_at - s.elapsed().as_secs_f64()).max(0.0),
            None => {
                if self.remaining_at <= 0.0 {
                    self.remaining_at = self.preset;
                }
                if self.remaining_at > 0.0 {
                    self.started = Some(Instant::now());
                }
            }
        }
    }
    pub fn reset(&mut self) {
        self.started = None;
        self.remaining_at = self.preset;
    }
    /// Back to the setup: nothing set.
    pub fn cancel(&mut self) {
        self.started = None;
        self.remaining_at = self.preset;
    }
    pub fn fraction(&self) -> f64 {
        if self.preset <= 0.0 { 0.0 } else { self.remaining() / self.preset }
    }
}

/// Which face the home tile shows when nothing is live.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TileFace {
    #[default]
    Analog,
    Digital,
}

/// What takes the tile over, in priority order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    AlarmRinging,
    TimerDone,
    TimerRunning,
    StopwatchRunning,
    Clock,
}

pub fn activity(alarm_ringing: bool, timer_done: bool, timer_running: bool, stopwatch_running: bool) -> Activity {
    if alarm_ringing {
        Activity::AlarmRinging
    } else if timer_done {
        Activity::TimerDone
    } else if timer_running {
        Activity::TimerRunning
    } else if stopwatch_running {
        Activity::StopwatchRunning
    } else {
        Activity::Clock
    }
}

/// `MM:SS.hh`, or `HH:MM:SS` past an hour (hundredths belong in a
/// trailing box then; the readout drops them).
pub fn stopwatch_text(secs: f64) -> String {
    let hundredths = (secs * 100.0).floor() as u64;
    let (m, s, h) = (hundredths / 6000, (hundredths / 100) % 60, hundredths % 100);
    if m >= 60 {
        format!("{}:{:02}:{:02}", m / 60, m % 60, s)
    } else {
        format!("{:02}:{:02}.{:02}", m, s, h)
    }
}

pub fn hh_mm_ss(secs: f64) -> String {
    let whole = secs.ceil().max(0.0) as u64;
    let (h, m, s) = (whole / 3600, (whole / 60) % 60, whole % 60);
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}

/// "Ends 10:24": the wall time a running timer reaches zero.
pub fn ends_text(now: LocalTime, remaining: f64) -> String {
    let total = (now.hour * 3600 + now.minute * 60 + now.second) as f64 + remaining.ceil();
    let total = (total as u64) % 86_400;
    format!("Ends {:02}:{:02}", total / 3600, (total / 60) % 60)
}

/// One of two side-by-side columns `gap` apart in `page_w`, at most `max`:
/// they always fit, from the 650 pt landscape breakpoint up.
pub fn column_width(page_w: f64, gap: f64, max: f64) -> f64 {
    ((page_w - gap) * 0.5).min(max).max(0.0)
}

const SHEET_OPEN_SECS: f64 = 0.30;
const SHEET_CLOSE_SECS: f64 = 0.22;
const PRESETS: [f64; 3] = [60.0, 300.0, 900.0];

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    // Inter 300 for the time readouts: the theme's family with its latin
    // member re-weighted, every fallback member kept.
    let Light = theme.font_regular{
        font_family +: {latin := FontMember{res: crate_resource("makepad_widgets:resources/Inter.ttf") weight: 300.0 asc: 0.0 desc: 0.0}}
    }
    let Text = Label{padding: 0 draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 12.75}}}
    let Secondary = Text{draw_text.color: theme.color_text_disabled draw_text.text_style.font_size: 11.25}
    let TextAction = ButtonFlat{
        height: 44 padding: Inset{left: 8 right: 8 top: 0 bottom: 0} margin: 0
        draw_text +: {color: #c86400 color_hover: #c86400 color_down: #a05000 text_style: theme.font_regular{font_size: 12.75}}
        draw_bg +: {pixel: fn(){ return vec4(0.0, 0.0, 0.0, 0.0) }}
    }
    // A round or pill action: Surface with the action colour mixed in.
    let Action = Button{
        width: 80 height: 80 padding: 0 margin: 0 text: "Start"
        draw_text +: {
            color: theme.color_text color_hover: theme.color_text color_down: theme.color_text color_focus: theme.color_text
            text_style: theme.font_bold{font_size: 12.75}
            tone: instance(vec4(0.0, 0.0, 0.0, 0.0))
            get_color: fn() {
                return mix(mix(self.color, self.tone, self.tone.w), self.color_disabled, self.disabled)
            }
        }
        draw_bg +: {
            color: theme.color_inset
            border_size: 0.0
            border_radius: 20.0
            tone: instance(vec4(0.0, 0.0, 0.0, 0.0))
            pixel: fn(){
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.border_radius)
                let base = mix(self.color, self.tone, self.tone.w * 0.14)
                let pressed = mix(base, self.tone, self.down * 0.12)
                sdf.fill(mix(pressed, mix(self.color, self.color_disabled, 0.5), self.disabled))
                return sdf.result
            }
        }
    }
    // A 48 pt icon target: a 24 pt glyph and a round press wash.
    let IconAction = Button{
        width: 48 height: 48 padding: 0 margin: 0 text: "" spacing: 0 align: Align{x: 0.5 y: 0.5}
        icon_walk: Walk{width: 24 height: 24}
        draw_icon +: {color: theme.color_text}
        draw_bg +: {
            pixel: fn(){
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, self.rect_size.x * 0.5)
                sdf.fill(vec4(theme.color_text.xyz, 0.08 * self.hover + 0.06 * self.down))
                return sdf.result
            }
        }
    }
    let MenuItem = ButtonFlat{
        width: Fill height: 48 padding: Inset{left: 16 right: 16} margin: 0 align: Align{x: 0.0 y: 0.5}
        draw_text +: {color: theme.color_text color_hover: theme.color_text color_down: theme.color_text text_style: theme.font_regular{font_size: 10.5}}
        draw_bg +: {pixel: fn(){ return Pal.premul(vec4(theme.color_text.xyz, 0.06 * self.hover + 0.06 * self.down)) }}
    }
    let Pill = Action{width: Fill height: 48 draw_bg.border_radius: 12.0}
    let ModalAction = Action{width: Fill height: 56 draw_bg.border_radius: 14.0}
    let SheetRow = View{width: Fill height: 44 flow: Right align: Align{y: 0.5} padding: Inset{left: 20 right: 20} spacing: 12}
    let Hairline = View{width: Fill height: 0.5 show_bg: true draw_bg.color: theme.color_text_disabled margin: Inset{left: 20 right: 20}}

    mod.widgets.ClockViewBase = #(ClockView::register_widget(vm))
    mod.widgets.ClockView = set_type_default() do mod.widgets.ClockViewBase{
        width: Fill height: Fill
        app_view := HostedView{
            full: View{width: Fill height: Fill flow: Overlay
                show_bg: true draw_bg.color: theme.color_bg_app
                // The pages sit above a reserved band for the tab bar (64 pt,
                // 8 pt off the bottom, 8 pt clear above it), so nothing
                // scrolls or rests underneath it.
                content := View{width: Fill height: Fill flow: Down
                    padding: Inset{left: 16 right: 16 top: 0 bottom: 80}
                    // One heading row: the title, and the page's one action
                    // at the trailing edge (the overflow on Clock, Add on
                    // Alarms). The 48 pt targets hang 12 pt past the margin
                    // so their 24 pt glyphs line up with it.
                    head := View{width: Fill height: 56 flow: Right align: Align{y: 0.5} margin: Inset{right: -12}
                        title := Text{text: "Clock" draw_text.text_style: theme.font_bold{font_size: 21}}
                        View{width: Fill height: Fill}
                        alarm_add := IconAction{visible: false icon_walk: Walk{width: 16 height: 16}
                            draw_icon +: {svg: crate_resource("self:resources/icons/plus.svg")}
                        }
                        more_action := IconAction{icon_walk: Walk{width: 18 height: 18}
                            draw_icon +: {svg: crate_resource("self:resources/icons/more.svg")}
                        }
                    }
                    modes := PageFlip{width: Fill height: Fill active_page: @clock_page
                        // Portrait: dial over the readout; landscape: the two
                        // side by side (`apply_sizing` turns the flow).
                        clock_page := ScrollYView{width: Fill height: Fill flow: Down align: Align{x: 0.5}
                            padding: Inset{top: 8 bottom: 16}
                            face_full := ClockFace{width: 292 height: 292}
                            clock_info := View{width: Fill height: Fit flow: Down align: Align{x: 0.5}
                                time_full := TabularLabel{width: Fill height: 64 margin: Inset{top: 16} draw_text.text_style: Light{font_size: 42}}
                                date_full := Secondary{margin: Inset{top: 8} draw_text.text_style.font_size: 10.5}
                                next_alarm := RoundedView{width: Fill height: 68 margin: Inset{top: 24} flow: Right spacing: 16
                                    padding: Inset{left: 16 right: 16} align: Align{y: 0.5} cursor: MouseCursor.Hand
                                    draw_bg +: {color: theme.color_inset border_radius: 8.0}
                                    Icon{width: 24 height: 24 icon_walk: Walk{width: 24 height: 24}
                                        draw_icon +: {svg: crate_resource("self:resources/icons/bell.svg") color: theme.color_text}}
                                    View{width: Fill height: Fit flow: Down spacing: 2
                                        na_time := Text{draw_text.text_style: theme.font_bold{font_size: 12}}
                                        na_repeat := Secondary{draw_text.text_style.font_size: 10.5}
                                    }
                                    Icon{width: 20 height: 20 icon_walk: Walk{width: 20 height: 20}
                                        draw_icon +: {svg: crate_resource("self:resources/icons/chevron.svg") color: theme.color_text_disabled}}
                                }
                            }
                        }
                        alarm_page := View{width: Fill height: Fill flow: Down
                            alarm_list := AlarmList{width: Fill height: Fill footer: "Sounds while Clock is running."}
                        }
                        stopwatch_page := View{width: Fill height: Fill flow: Down
                            sw_top := View{width: Fill height: Fit flow: Down
                                sw_gap := View{width: Fill height: 56}
                                sw_readout := TabularLabel{width: Fill height: 96 draw_text.text_style: Light{font_size: 57}}
                                sw_buttons := View{width: Fill height: 80 margin: Inset{top: 40} flow: Right padding: Inset{left: 4 right: 4}
                                    sw_left := Action{text: "Lap"}
                                    View{width: Fill height: Fill}
                                    sw_right := Action{text: "Start"}
                                }
                            }
                            laps_wrap := View{width: Fill height: Fill
                                laps := LapList{width: Fill height: Fill margin: Inset{top: 24}}
                            }
                        }
                        timer_page := PageFlip{width: Fill height: Fill active_page: @setup
                            setup := ScrollYView{width: Fill height: Fill flow: Down align: Align{x: 0.5} padding: Inset{bottom: 16}
                                setup_wheel := View{width: Fill height: Fit flow: Down align: Align{x: 0.5} padding: Inset{top: 14}
                                    duration_wheel := DurationWheel{width: Fill height: 220}
                                }
                                setup_form := View{width: Fill height: Fit flow: Down align: Align{x: 0.5}
                                    timer_label := TextInput{width: Fill height: 52 margin: Inset{top: 18} empty_text: "Label"}
                                    presets := View{width: Fill height: 48 margin: Inset{top: 16} flow: Right spacing: 8
                                        preset_1 := Pill{text: "1 min"}
                                        preset_5 := Pill{text: "5 min"}
                                        preset_15 := Pill{text: "15 min"}
                                    }
                                    timer_start := Action{width: 96 height: 64 margin: Inset{top: 32} text: "Start" draw_bg.border_radius: 16.0}
                                }
                            }
                            running := View{width: Fill height: Fill flow: Down align: Align{x: 0.5}
                                ring := TimerRing{width: 308 height: 308 margin: Inset{top: 18}}
                                run_controls := View{width: Fill height: Fit flow: Down align: Align{x: 0.5}
                                    run_buttons := View{width: Fill height: 80 margin: Inset{top: 42} flow: Right padding: Inset{left: 4 right: 4}
                                        timer_cancel := Action{text: "Cancel"}
                                        View{width: Fill height: Fill}
                                        timer_pause := Action{text: "Pause"}
                                    }
                                    timer_reset := TextAction{text: "Reset" visible: false margin: Inset{top: 8}}
                                }
                            }
                        }
                    }
                }
                // The navigation bar: one opaque tonal surface (16 pt corners,
                // one soft shadow) centred 8 pt above the bottom.
                View{width: Fill height: Fill align: Align{x: 0.5 y: 1.0} padding: Inset{bottom: 8}
                    tabs_bar := RoundedShadowView{width: 370 height: 64
                        draw_bg +: {color: theme.color_inset border_radius: 8.0 shadow_color: #0000001f shadow_radius: 8.0 shadow_offset: vec2(0.0, 2.0)}
                        tabs := PhoneTabs{width: Fill height: Fill}
                    }
                }
                // The overflow menu under the heading's trailing action.
                menu_layer := View{visible: false width: Fill height: Fill flow: Overlay
                    menu_scrim := View{width: Fill height: Fill show_bg: true draw_bg.color: #0000 cursor: MouseCursor.Default}
                    View{width: Fill height: Fill align: Align{x: 1.0 y: 0.0} padding: Inset{top: 52 right: 12}
                        menu := RoundedShadowView{width: 224 height: Fit flow: Down padding: Inset{top: 8 bottom: 8}
                            draw_bg +: {color: theme.color_inset border_radius: 6.0 shadow_color: #00000029 shadow_radius: 10.0 shadow_offset: vec2(0.0, 3.0)}
                            tile_face_action := MenuItem{text: "Digital home tile"}
                        }
                    }
                }
                sheet_layer := View{visible: false width: Fill height: Fill flow: Overlay
                    scrim := View{width: Fill height: Fill show_bg: true draw_bg.color: #00000066}
                    View{width: Fill height: Fill align: Align{y: 1.0}
                        sheet := RoundedView{width: Fill height: Fit flow: Down
                            draw_bg +: {color: theme.color_inset border_radius: 14.0}
                            View{width: Fill height: 56 flow: Right align: Align{y: 0.5} padding: Inset{left: 12 right: 12}
                                alarm_cancel := TextAction{text: "Cancel"}
                                View{width: Fill height: Fill align: Align{x: 0.5 y: 0.5}
                                    editor_title := Text{text: "New alarm" draw_text.text_style: theme.font_bold{font_size: 12.75}}
                                }
                                alarm_save := TextAction{text: "Save"}
                            }
                            alarm_wheel := TimeWheel{width: Fill height: 220 margin: Inset{left: 20 right: 20}}
                            SheetRow{margin: Inset{top: 8}
                                Text{text: "Repeat" width: Fill}
                                Secondary{text: "Every day" draw_text.text_style.font_size: 12.75}
                            }
                            Hairline{}
                            SheetRow{
                                Text{text: "Label"}
                                alarm_label := TextInput{width: Fill height: 44 empty_text: "Alarm"}
                            }
                            Secondary{text: "Sounds while Clock is running." draw_text.text_style.font_size: 9.75 margin: Inset{left: 20 top: 12}}
                            View{width: Fill height: 24}
                        }
                    }
                }
                modal := View{visible: false width: Fill height: Fill flow: Overlay
                    View{width: Fill height: Fill show_bg: true draw_bg.color: #00000066}
                    View{width: Fill height: Fill align: Align{x: 0.5 y: 0.5} padding: 20
                        panel := RoundedView{width: Fill height: Fit flow: Down padding: 24 spacing: 16
                            draw_bg +: {color: theme.color_inset border_radius: 14.0}
                            modal_title := Text{text: "Alarm" draw_text.text_style: theme.font_bold{font_size: 21}}
                            modal_time := TabularLabel{width: Fill height: 52 align_x: 0.0 draw_text.text_style: Light{font_size: 33}}
                            modal_label := Secondary{draw_text.text_style.font_size: 12.75}
                            modal_primary := ModalAction{text: "Snooze 5 min"}
                            modal_secondary := ModalAction{text: "Stop"}
                        }
                    }
                }
            }
            tile: View{width: Fill height: Fill flow: Overlay
                show_bg: true draw_bg.color: theme.color_inset
                analog := View{width: Fill height: Fill align: Align{x: 0.5 y: 0.5}
                    face_tile := ClockFace{width: 154 height: 154 numeral_size: 9.0}
                }
                digital := View{visible: false width: Fill height: Fill flow: Down padding: 16
                    place := Secondary{draw_text.text_style: theme.font_bold{font_size: 9.75}}
                    time_tile := TabularLabel{width: Fill height: 58 margin: Inset{top: 12} align_x: 0.0 draw_text.text_style: Light{font_size: 34.5}}
                    date_tile := Text{margin: Inset{top: 10} draw_text.text_style.font_size: 9.75}
                    extra_tile := Text{margin: Inset{top: 5} draw_text.color: #c86400 draw_text.text_style: theme.font_bold{font_size: 8.25}}
                }
                activity := View{visible: false width: Fill height: Fill flow: Down align: Align{x: 0.5} padding: Inset{top: 20}
                    act_ring := TimerRing{width: 96 height: 96 draw_ring.stroke: 3.0 readout.text_style.font_size: 21 status.text_style.font_size: 0.75}
                    act_title := Text{margin: Inset{top: 8} draw_text.text_style: theme.font_bold{font_size: 9.75}}
                    act_status := Secondary{margin: Inset{top: 5} draw_text.text_style.font_size: 9}
                }
            }
        }
    }
}

/// The clock, the alarm book, the stopwatch and the countdown timer: one
/// widget with two resident faces (`app_view`, a `HostedView`). Owns
/// everything the standalone window's `App` used to own, so a module host
/// gets the same clock.
#[derive(Script, ScriptHook, Widget)]
pub struct ClockView {
    #[deref]
    view: View,
    /// Set once, on the first event or draw: a window seats this widget
    /// before its `Startup` event, a module host after `create` mints it.
    #[rust]
    started: bool,
    /// Once a second: wall time, the paused stopwatch, the alarm book.
    #[rust]
    tick: Option<Timer>,
    /// Only while the stopwatch or the timer runs: tenths on screen.
    #[rust]
    fast: Option<Timer>,
    #[rust]
    stopwatch: Stopwatch,
    #[rust]
    countdown: Countdown,
    #[rust]
    last_time: Option<LocalTime>,
    #[rust]
    mode: usize,
    #[rust]
    alarm: AlarmBook,
    #[rust]
    alarm_edit_id: Option<u64>,
    #[rust]
    alarm_load: Option<StorageRequestId>,
    #[rust]
    alarm_write: Option<StorageRequestId>,
    #[rust]
    alarm_dirty: bool,
    #[rust]
    alarm_changed: bool,
    #[rust]
    alarm_signal: Arc<AtomicBool>,
    #[rust]
    countdown_alert: bool,
    #[rust]
    last_size: Vec2d,
    #[rust]
    last_face: HostedViewMode,
    #[rust]
    tile_face: TileFace,
    /// The last sizing was landscape (the stopwatch reads it for its split).
    #[rust]
    landscape: bool,
    /// The pages' width inside the side margins, at the last sizing.
    #[rust]
    page_w: f64,
    /// The theme ink the secondary text was last derived from.
    #[rust]
    last_ink: Vec4f,
    /// The stopwatch column width last applied (None = Fill), to re-lay it
    /// only when laps appear or go.
    #[rust]
    sw_split: Option<Option<f64>>,
    #[rust]
    tile_face_load: Option<StorageRequestId>,
    /// The editor sheet's slide, 0 closed … 1 open, and its direction.
    #[rust]
    sheet_open: bool,
    #[rust]
    sheet_pos: f64,
    #[rust]
    sheet_start: f64,
    #[rust]
    sheet_from: f64,
    #[rust]
    sheet_frame: NextFrame,
    /// The instance's storage jail (module) or its own namespace
    /// (standalone): alarms and the tile face persist there. Set before
    /// the first event by the module's `create` or the standalone
    /// window's startup.
    #[rust]
    storage: Option<StorageHandle>,
}

impl ClockView {
    /// A host handed this instance its storage: alarms load from and save
    /// to it from now on.
    pub fn set_storage(&mut self, storage: StorageHandle) {
        self.storage = Some(storage);
    }

    /// The face the host asked for (`HostedViewMode`, read off the
    /// embedded `HostedView`).
    pub fn face(&self, cx: &Cx) -> HostedViewMode {
        self.view.widget(cx, ids!(app_view)).borrow::<HostedView>().map(|h| h.mode()).unwrap_or_default()
    }

    /// One line for the assistant: local time and date, the next alarm,
    /// and whether a timer or the stopwatch is running.
    pub fn ai_summary(&self) -> String {
        let now = self.last_time.unwrap_or_else(LocalTime::now);
        let mut parts = vec![format!("{} · {}", now.hm(), now.date_text())];
        if self.alarm.ringing() {
            parts.push("an alarm is ringing now".to_string());
        } else if let Some(next) = self.alarm.next(now) {
            parts.push(format!("next alarm {} · Every day", next.time()));
        } else {
            parts.push("no alarm set".to_string());
        }
        if self.countdown.running() {
            parts.push(format!("timer running, {} left", hh_mm_ss(self.countdown.remaining())));
        } else if self.countdown.finished() {
            parts.push("timer done".to_string());
        }
        if self.stopwatch.running() {
            parts.push(format!("stopwatch running at {}", stopwatch_text(self.stopwatch.elapsed())));
        } else if self.stopwatch.elapsed() > 0.0 {
            parts.push(format!("stopwatch paused at {}", stopwatch_text(self.stopwatch.elapsed())));
        }
        parts.join(" · ")
    }

    /// Timers stop; nothing else this widget owns outlives its isolate.
    pub fn shutdown(&mut self, cx: &mut Cx) {
        if let Some(t) = self.tick.take() {
            cx.stop_timer(t);
        }
        if let Some(t) = self.fast.take() {
            cx.stop_timer(t);
        }
    }

    fn select_mode(&mut self, cx: &mut Cx, mode: usize, animate: bool) {
        self.mode = mode.min(3);
        let (page, title) = match self.mode {
            0 => (live_id!(clock_page), "Clock"),
            1 => (live_id!(alarm_page), "Alarms"),
            2 => (live_id!(stopwatch_page), "Stopwatch"),
            _ => (live_id!(timer_page), "Timer"),
        };
        self.view.page_flip(cx, ids!(modes)).set_active_page(cx, page);
        self.view.label(cx, ids!(title)).set_text(cx, title);
        if let Some(mut tabs) = self.view.widget(cx, ids!(tabs)).borrow_mut::<PhoneTabs>() {
            tabs.set_active(cx, self.mode, animate);
        }
        // The heading's trailing action: the overflow on Clock, Add on Alarms.
        self.view.widget(cx, ids!(more_action)).set_visible(cx, self.mode == 0);
        self.view.widget(cx, ids!(alarm_add)).set_visible(cx, self.mode == 1);
        self.set_menu(cx, false);
        self.view.redraw(cx);
    }

    /// The heading's overflow menu (the home tile's face).
    fn set_menu(&mut self, cx: &mut Cx, open: bool) {
        let layer = self.view.widget(cx, ids!(menu_layer));
        if layer.visible() != open {
            layer.set_visible(cx, open);
            self.view.redraw(cx);
        }
    }

    fn menu_open(&self, cx: &Cx) -> bool {
        self.view.widget(cx, ids!(menu_layer)).visible()
    }

    // ---- the alarm editor sheet ----

    fn open_editor(&mut self, cx: &mut Cx, id: Option<u64>) {
        self.alarm_edit_id = id;
        let spec = id.and_then(|id| self.alarm.find(id)).cloned();
        let (hour, minute) = spec.as_ref().map(|a| (a.hour, a.minute)).unwrap_or((7, 0));
        if let Some(mut wheel) = self.view.widget(cx, ids!(alarm_wheel)).borrow_mut::<TimeWheel>() {
            wheel.set_time(cx, hour, minute);
        }
        self.view.text_input(cx, ids!(alarm_label)).set_text(cx, spec.as_ref().map(|a| a.label.as_str()).unwrap_or(""));
        self.view.label(cx, ids!(editor_title)).set_text(cx, if id.is_some() { "Edit alarm" } else { "New alarm" });
        self.set_sheet(cx, true);
    }

    fn set_sheet(&mut self, cx: &mut Cx, open: bool) {
        if self.sheet_open == open {
            return;
        }
        self.sheet_open = open;
        self.sheet_from = self.sheet_pos;
        self.sheet_start = Cx::monotonic_now();
        self.view.widget(cx, ids!(sheet_layer)).set_visible(cx, true);
        self.sheet_frame = cx.new_next_frame();
        self.apply_sheet(cx);
    }

    fn ease_open(t: f64) -> f64 {
        let u = 1.0 - t.clamp(0.0, 1.0);
        1.0 - u * u * u
    }

    fn ease_close(t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        t * t * t
    }

    fn step_sheet(&mut self, cx: &mut Cx) {
        let secs = if self.sheet_open { SHEET_OPEN_SECS } else { SHEET_CLOSE_SECS };
        let t = ((Cx::monotonic_now() - self.sheet_start) / secs).clamp(0.0, 1.0);
        let target = if self.sheet_open { 1.0 } else { 0.0 };
        let e = if self.sheet_open { Self::ease_open(t) } else { Self::ease_close(t) };
        self.sheet_pos = self.sheet_from + (target - self.sheet_from) * e;
        if t < 1.0 {
            self.sheet_frame = cx.new_next_frame();
        } else {
            self.sheet_pos = target;
            if !self.sheet_open {
                self.view.widget(cx, ids!(sheet_layer)).set_visible(cx, false);
            }
        }
        self.apply_sheet(cx);
    }

    fn apply_sheet(&mut self, cx: &mut Cx) {
        // The sheet slides up from below its own height; the scrim fades with it.
        let sheet_h = self.view.widget(cx, ids!(sheet)).area().rect(cx).size.y.max(560.0);
        let offset = (1.0 - self.sheet_pos) * sheet_h;
        let mut sheet = self.view.widget(cx, ids!(sheet));
        // Top and bottom margins paired: the bottom-anchored sheet moves
        // down by `offset` without its layout extent changing.
        script_apply_eval!(cx, sheet, { margin.top: #(offset) margin.bottom: #(-offset) });
        let alpha = (0.4 * self.sheet_pos) as f32;
        let mut scrim = self.view.widget(cx, ids!(scrim));
        script_apply_eval!(cx, scrim, { draw_bg.color: #(vec4(0.0, 0.0, 0.0, alpha)) });
        self.view.redraw(cx);
    }

    fn save_alarm(&mut self, cx: &mut Cx) {
        let time = self.view.widget(cx, ids!(alarm_wheel)).borrow::<TimeWheel>().map(|wheel| wheel.time()).unwrap_or((7, 0));
        let label = self.view.text_input(cx, ids!(alarm_label)).text().chars().take(80).collect();
        self.alarm.save(self.alarm_edit_id, time.0, time.1, label);
        self.alarms_modified(cx);
        self.set_sheet(cx, false);
    }

    fn alarms_modified(&mut self, cx: &mut Cx) {
        self.alarm_changed = true;
        self.alarm_dirty = true;
        self.persist_alarms(cx);
        self.refresh_alarm(cx);
    }

    fn persist_alarms(&mut self, cx: &mut Cx) {
        if !self.alarm_dirty || self.alarm_write.is_some() {
            return;
        }
        let Some(storage) = self.storage.as_ref() else {
            // No jail yet: keep the edit dirty and try again once one
            // arrives (the module's `create` or the window's startup set
            // it before the first event either reaches this widget).
            return;
        };
        self.alarm_write = Some(storage.set(cx, "alarms", self.alarm.encode()));
        self.alarm_dirty = false;
    }

    fn on_storage(&mut self, cx: &mut Cx, responses: &[StorageResponse]) {
        for response in responses {
            if self.alarm_load == Some(response.request_id) {
                self.alarm_load = None;
                if !self.alarm_changed {
                    if let Ok(StorageResult::Value(Some(bytes))) = &response.result {
                        if let Some(book) = AlarmBook::decode(bytes) {
                            self.alarm = book;
                            self.refresh_alarm(cx);
                        }
                    }
                }
            }
            if self.tile_face_load == Some(response.request_id) {
                self.tile_face_load = None;
                if let Ok(StorageResult::Value(Some(bytes))) = &response.result {
                    if bytes.as_slice() == b"digital" {
                        self.tile_face = TileFace::Digital;
                        self.refresh_tile(cx);
                    }
                }
            }
            if self.alarm_write == Some(response.request_id) {
                self.alarm_write = None;
                if let Err(error) = &response.result {
                    log!("clock: alarm save failed: {}", error);
                    if let Some(mut list) = self.view.widget(cx, ids!(alarm_list)).borrow_mut::<AlarmList>() {
                        list.set_footer(cx, "Could not save alarms. Changes remain available this session.");
                    }
                }
                self.persist_alarms(cx);
            }
        }
    }

    fn toggle_tile_face(&mut self, cx: &mut Cx) {
        self.tile_face = match self.tile_face {
            TileFace::Analog => TileFace::Digital,
            TileFace::Digital => TileFace::Analog,
        };
        if let Some(storage) = self.storage.as_ref() {
            let value = if self.tile_face == TileFace::Digital { "digital" } else { "analog" };
            let _ = storage.set(cx, "tile_face", value.as_bytes().to_vec());
        }
        self.refresh_tile(cx);
    }

    // ---- refreshes ----

    fn refresh_alarm(&mut self, cx: &mut Cx) {
        if let Some(mut list) = self.view.widget(cx, ids!(alarm_list)).borrow_mut::<AlarmList>() {
            list.set_rows(cx, self.alarm.specs());
        }
        let now = LocalTime::now();
        match self.alarm.next(now) {
            Some(next) => {
                let label = if next.label.trim().is_empty() { "Alarm".to_string() } else { next.label.clone() };
                self.view.label(cx, ids!(na_time)).set_text(cx, &format!("{} · {}", next.time(), label));
                self.view.label(cx, ids!(na_repeat)).set_text(cx, "Every day");
            }
            None => {
                self.view.label(cx, ids!(na_time)).set_text(cx, "Add an alarm");
                self.view.label(cx, ids!(na_repeat)).set_text(cx, "No alarm set");
            }
        }
        self.refresh_modal(cx);
        self.refresh_tile(cx);
    }

    /// The centred panel for a ringing alarm or a finished timer.
    fn refresh_modal(&mut self, cx: &mut Cx) {
        let ringing = self.alarm.ringing();
        let done = self.countdown_alert;
        self.alarm_signal.store(ringing || done, Ordering::Relaxed);
        self.view.widget(cx, ids!(modal)).set_visible(cx, ringing || done);
        if ringing {
            let now = LocalTime::now();
            let spec = self.alarm.alarms.iter().find(|a| a.ringing).map(|a| a.spec.clone());
            self.view.label(cx, ids!(modal_title)).set_text(cx, "Alarm");
            if let Some(mut t) = self.view.widget(cx, ids!(modal_time)).borrow_mut::<TabularLabel>() {
                t.set_text(cx, &spec.as_ref().map(|s| s.time()).unwrap_or_else(|| now.hm()));
            }
            let label = spec.map(|s| s.label).filter(|l| !l.trim().is_empty()).unwrap_or_else(|| "Every day".into());
            self.view.label(cx, ids!(modal_label)).set_text(cx, &label);
            self.view.button(cx, ids!(modal_primary)).set_text(cx, "Snooze 5 min");
            self.view.button(cx, ids!(modal_secondary)).set_text(cx, "Stop");
        } else if done {
            self.view.label(cx, ids!(modal_title)).set_text(cx, "Timer");
            if let Some(mut t) = self.view.widget(cx, ids!(modal_time)).borrow_mut::<TabularLabel>() {
                t.set_text(cx, &hh_mm_ss(self.countdown.preset));
            }
            let label = if self.countdown.label.trim().is_empty() { "Time is up".to_string() } else { self.countdown.label.clone() };
            self.view.label(cx, ids!(modal_label)).set_text(cx, &label);
            self.view.button(cx, ids!(modal_primary)).set_text(cx, "Repeat");
            self.view.button(cx, ids!(modal_secondary)).set_text(cx, "Stop");
        }
    }

    fn refresh_clock(&mut self, cx: &mut Cx, force: bool) {
        let now = LocalTime::now();
        if !force && self.last_time == Some(now) {
            return;
        }
        // Showing only the tile (no second hand, minutes on the digital
        // face): a new second changes nothing it shows, and redrawing the
        // full face behind it made the app (and its host) repaint every
        // second at rest. The full face catches up when it shows (`force`).
        if !force && !self.countdown.running() && self.face(cx) == HostedViewMode::Tile && self.last_time.is_some_and(|t| t.hour == now.hour && t.minute == now.minute && t.date_text() == now.date_text()) {
            return;
        }
        self.last_time = Some(now);
        for id in [ids!(face_full), ids!(face_tile)] {
            if let Some(mut face) = self.view.widget(cx, id).borrow_mut::<ClockFace>() {
                face.set_time(cx, now.hour, now.minute, now.second);
            }
        }
        if let Some(mut t) = self.view.widget(cx, ids!(time_full)).borrow_mut::<TabularLabel>() {
            t.set_text(cx, &now.hm());
        }
        let date = if now.zoned { now.date_text() } else { format!("{} · UTC", now.date_text().trim_end_matches(" (UTC)")) };
        self.view.label(cx, ids!(date_full)).set_text(cx, &date);
        if let Some(mut t) = self.view.widget(cx, ids!(time_tile)).borrow_mut::<TabularLabel>() {
            t.set_text(cx, &now.hm());
        }
        self.view.label(cx, ids!(date_tile)).set_text(cx, &now.date_text());
        self.view.label(cx, ids!(place)).set_text(cx, if now.zoned { "Local time" } else { "UTC" });
        if self.countdown.running() {
            self.refresh_countdown(cx);
        }
    }

    fn set_action(&mut self, cx: &mut Cx, id: &[LiveId], text: &str, tone: Option<Vec4f>, enabled: bool) {
        let button = self.view.button(cx, id);
        button.set_text(cx, text);
        button.set_enabled(cx, enabled);
        let tone = tone.unwrap_or(vec4(0.0, 0.0, 0.0, 0.0));
        let mut b = self.view.widget(cx, id);
        script_apply_eval!(cx, b, { draw_bg.tone: #(tone) draw_text.tone: #(tone) });
    }

    fn refresh_stopwatch(&mut self, cx: &mut Cx) {
        let start = vec4(0.094, 0.482, 0.212, 1.0);
        let stop = vec4(0.773, 0.184, 0.208, 1.0);
        if let Some(mut t) = self.view.widget(cx, ids!(sw_readout)).borrow_mut::<TabularLabel>() {
            t.set_text(cx, &stopwatch_text(self.stopwatch.elapsed()));
        }
        let running = self.stopwatch.running();
        let has_time = self.stopwatch.elapsed() > 0.0;
        self.set_action(cx, ids!(sw_left), if running { "Lap" } else { "Reset" }, None, running || has_time);
        self.set_action(cx, ids!(sw_right), if running { "Stop" } else { "Start" }, Some(if running { stop } else { start }), true);
        self.layout_stopwatch(cx);
        let laps = self.stopwatch.laps();
        if let Some(mut list) = self.view.widget(cx, ids!(laps)).borrow_mut::<LapList>() {
            list.set(cx, &laps, self.stopwatch.current_lap().filter(|_| !laps.is_empty() || running));
        }
        self.refresh_tile(cx);
    }

    fn refresh_countdown(&mut self, cx: &mut Cx) {
        let was_running = self.countdown.running();
        self.countdown.settle();
        if was_running && self.countdown.finished() {
            self.countdown_alert = true;
            self.refresh_modal(cx);
        }
        let active = self.countdown.active();
        self.view.page_flip(cx, ids!(timer_page)).set_active_page(cx, if active { live_id!(running) } else { live_id!(setup) });
        let now = LocalTime::now();
        let status = if self.countdown.finished() {
            "Done".to_string()
        } else if self.countdown.running() {
            ends_text(now, self.countdown.remaining())
        } else {
            "Paused".to_string()
        };
        if let Some(mut ring) = self.view.widget(cx, ids!(ring)).borrow_mut::<TimerRing>() {
            ring.set(cx, self.countdown.fraction(), &hh_mm_ss(self.countdown.remaining()), &status);
        }
        let start = vec4(0.094, 0.482, 0.212, 1.0);
        let stop = vec4(0.773, 0.184, 0.208, 1.0);
        let running = self.countdown.running();
        self.set_action(cx, ids!(timer_pause), if running { "Pause" } else { "Resume" }, Some(if running { stop } else { start }), !self.countdown.finished());
        self.set_action(cx, ids!(timer_cancel), "Cancel", None, true);
        self.view.widget(cx, ids!(timer_reset)).set_visible(cx, active && !running && !self.countdown.finished());
        // Setup: Start is disabled at a zero duration.
        let duration = self.view.widget(cx, ids!(duration_wheel)).borrow::<TimeWheel>().map(|w| w.duration()).unwrap_or(0);
        self.set_action(cx, ids!(timer_start), "Start", Some(start), duration > 0);
        self.sync_fast_timer(cx);
        self.refresh_tile(cx);
    }

    /// The tile: whichever activity is live, else the chosen clock face.
    fn refresh_tile(&mut self, cx: &mut Cx) {
        let act = activity(self.alarm.ringing(), self.countdown_alert, self.countdown.running() || (self.countdown.active() && !self.countdown.finished()), self.stopwatch.running());
        let (analog, digital, activity_face) = match act {
            Activity::Clock => (self.tile_face == TileFace::Analog, self.tile_face == TileFace::Digital, false),
            Activity::StopwatchRunning | Activity::AlarmRinging => (false, true, false),
            Activity::TimerDone | Activity::TimerRunning => (false, false, true),
        };
        self.view.widget(cx, ids!(analog)).set_visible(cx, analog);
        self.view.widget(cx, ids!(digital)).set_visible(cx, digital);
        self.view.widget(cx, ids!(activity)).set_visible(cx, activity_face);
        match act {
            Activity::StopwatchRunning => {
                self.view.label(cx, ids!(place)).set_text(cx, "Stopwatch");
                if let Some(mut t) = self.view.widget(cx, ids!(time_tile)).borrow_mut::<TabularLabel>() {
                    t.set_text(cx, &hh_mm_ss(self.stopwatch.elapsed().floor()));
                }
                self.view.label(cx, ids!(extra_tile)).set_text(cx, "Running");
            }
            Activity::AlarmRinging => {
                self.view.label(cx, ids!(place)).set_text(cx, "Alarm");
                self.view.label(cx, ids!(extra_tile)).set_text(cx, "Ringing · tap to open");
            }
            Activity::TimerDone | Activity::TimerRunning => {
                if let Some(mut ring) = self.view.widget(cx, ids!(act_ring)).borrow_mut::<TimerRing>() {
                    ring.set(cx, self.countdown.fraction(), &hh_mm_ss(self.countdown.remaining()), "");
                }
                let title = if self.countdown.label.trim().is_empty() { "Timer".to_string() } else { self.countdown.label.clone() };
                self.view.label(cx, ids!(act_title)).set_text(cx, &title);
                let status = if self.countdown.finished() { "Done".to_string() } else if self.countdown.running() { ends_text(LocalTime::now(), self.countdown.remaining()) } else { "Paused".into() };
                self.view.label(cx, ids!(act_status)).set_text(cx, &status);
            }
            Activity::Clock => {
                let now = LocalTime::now();
                self.view.label(cx, ids!(place)).set_text(cx, if now.zoned { "Local time" } else { "UTC" });
                if let Some(mut t) = self.view.widget(cx, ids!(time_tile)).borrow_mut::<TabularLabel>() {
                    t.set_text(cx, &now.hm());
                }
                let extra = self.alarm.next(now).map(|a| format!("Alarm {}", a.time())).unwrap_or_default();
                self.view.label(cx, ids!(extra_tile)).set_text(cx, &extra);
            }
        }
        self.view.button(cx, ids!(tile_face_action)).set_text(cx, if self.tile_face == TileFace::Analog { "Digital home tile" } else { "Analog home tile" });
        let mut face = self.view.widget(cx, ids!(face_tile));
        // Tile dials step once a second; the second hand stays on the full face only.
        if let Some(mut f) = face.borrow_mut::<ClockFace>() {
            f.set_show_seconds(cx, false);
        }
        let _ = &mut face;
    }

    /// Landscape: with laps, the readout and buttons in a 360 pt column
    /// left of them; without, that column alone, centred at 400 pt.
    fn layout_stopwatch(&mut self, cx: &mut Cx) {
        let split = self.landscape && self.stopwatch.current_lap().is_some();
        let width = if !self.landscape { None } else if split { Some((self.page_w * 0.5).min(360.0)) } else { Some(self.page_w.min(400.0)) };
        if self.sw_split == Some(width) {
            return;
        }
        self.sw_split = Some(width);
        let mut top = self.view.widget(cx, ids!(sw_top));
        match width {
            Some(width) => script_apply_eval!(cx, top, { width: #(width) }),
            None => script_apply_eval!(cx, top, { width: mod.turtle.Fill }),
        }
        self.view.widget(cx, ids!(laps_wrap)).set_visible(cx, !self.landscape || split);
    }

    fn sync_fast_timer(&mut self, cx: &mut Cx) {
        if self.stopwatch.running() || self.countdown.running() {
            if self.fast.is_none() {
                self.fast = Some(cx.start_interval(1.0 / 30.0));
            }
        } else if let Some(t) = self.fast.take() {
            cx.stop_timer(t);
        }
    }

    /// The date and the alarm card's caption in the theme's ink at 72 %:
    /// the theme's disabled grey reads at under 3:1 on the inset surfaces
    /// (1.2:1 dark), this keeps ≥ 4.5:1 in both appearances. Re-derived
    /// whenever a restyle changes the ink.
    fn apply_secondary_ink(&mut self, cx: &mut Cx) {
        let Some(ink) = self.view.widget(cx, ids!(time_full)).borrow::<TabularLabel>().map(|t| t.ink()) else {
            return;
        };
        if ink == self.last_ink {
            return;
        }
        self.last_ink = ink;
        let secondary = vec4(ink.x, ink.y, ink.z, 0.72);
        for id in [ids!(date_full), ids!(na_repeat)] {
            let mut w = self.view.widget(cx, id);
            script_apply_eval!(cx, w, { draw_text.color: #(secondary) });
        }
    }

    /// Face sizing off this widget's own draw rect: the tile viewport
    /// under a module host, the window's inner size standalone.
    fn apply_sizing(&mut self, cx: &mut Cx, size: Vec2d) {
        let landscape = size.x >= 650.0 && size.y < 500.0;
        // Type in makepad points: the design's logical sizes times 3/4.
        // Heading row 56 pt with a 28 pt title; 48 pt with 22 pt landscape.
        let (head_h, title_size) = if landscape { (48.0, 16.5) } else { (56.0, 21.0) };
        let mut head = self.view.widget(cx, ids!(head));
        script_apply_eval!(cx, head, { height: #(head_h) });
        let mut title = self.view.widget(cx, ids!(title));
        script_apply_eval!(cx, title, { draw_text.text_style.font_size: #(title_size) });
        // What the pages get: the viewport less the heading and the tab band,
        // and less the 16 pt side margins.
        let page_h = (size.y - head_h - 80.0).max(120.0);
        let page_w = (size.x - 32.0).max(120.0);
        let set_layout = |view: &mut View, flow: Flow, spacing: f64, align: Align| {
            view.layout.flow = flow;
            view.layout.spacing = spacing;
            view.layout.align = align;
        };
        let set_width = |cx: &mut Cx, mut w: WidgetRef, width: Option<f64>| match width {
            Some(width) => script_apply_eval!(cx, w, { width: #(width) }),
            None => script_apply_eval!(cx, w, { width: mod.turtle.Fill }),
        };

        // Clock: the 292 pt dial over a 56/64 time, a 14/20 date and the
        // 68 pt alarm card; landscape, a 192 pt dial left of a 40/48 time,
        // the date and a 64 pt card, 24 pt apart, the pair centred.
        let dial = if landscape { (page_h - 12.0).clamp(120.0, 192.0) } else { 292.0 };
        let mut full = self.view.widget(cx, ids!(face_full));
        script_apply_eval!(cx, full, { width: #(dial) height: #(dial) });
        if let Some(mut page) = self.view.view(cx, ids!(clock_page)).borrow_mut() {
            if landscape {
                set_layout(&mut page, Flow::right(), 24.0, Align { x: 0.5, y: 0.5 });
                page.layout.padding = Inset { top: 0.0, bottom: 8.0, left: 0.0, right: 0.0 };
            } else {
                set_layout(&mut page, Flow::Down, 0.0, Align { x: 0.5, y: 0.0 });
                page.layout.padding = Inset { top: 8.0, bottom: 16.0, left: 0.0, right: 0.0 };
            }
        }
        if let Some(mut info) = self.view.view(cx, ids!(clock_info)).borrow_mut() {
            info.layout.align = Align { x: if landscape { 0.0 } else { 0.5 }, y: 0.0 };
        }
        set_width(cx, self.view.widget(cx, ids!(clock_info)), landscape.then_some((page_w - dial - 24.0).min(320.0)));
        let (time_size, time_h, time_top, card_h, card_top) = if landscape { (30.0, 48.0, 0.0, 64.0, 16.0) } else { (42.0, 64.0, 16.0, 68.0, 24.0) };
        let mut time = self.view.widget(cx, ids!(time_full));
        script_apply_eval!(cx, time, { height: #(time_h) margin.top: #(time_top) draw_text.text_style.font_size: #(time_size) });
        if let Some(mut t) = time.borrow_mut::<TabularLabel>() {
            t.set_align_x(cx, if landscape { 0.0 } else { 0.5 });
        }
        let mut card = self.view.widget(cx, ids!(next_alarm));
        script_apply_eval!(cx, card, { height: #(card_h) margin.top: #(card_top) });

        // Stopwatch: landscape puts the readout and its buttons left of the laps.
        if let Some(mut page) = self.view.view(cx, ids!(stopwatch_page)).borrow_mut() {
            if landscape {
                set_layout(&mut page, Flow::right(), 24.0, Align { x: 0.5, y: 0.0 });
            } else {
                set_layout(&mut page, Flow::Down, 0.0, Align { x: 0.0, y: 0.0 });
            }
        }
        self.landscape = landscape;
        self.page_w = page_w;
        self.sw_split = None;
        self.layout_stopwatch(cx);
        let (gap, readout_size, readout_h, buttons_top) = if landscape { (8.0, 42.0, 72.0, 16.0) } else { (56.0, 57.0, 96.0, 40.0) };
        let mut sw_gap = self.view.widget(cx, ids!(sw_gap));
        script_apply_eval!(cx, sw_gap, { height: #(gap) });
        let mut readout = self.view.widget(cx, ids!(sw_readout));
        script_apply_eval!(cx, readout, { height: #(readout_h) draw_text.text_style.font_size: #(readout_size) });
        let mut sw_buttons = self.view.widget(cx, ids!(sw_buttons));
        script_apply_eval!(cx, sw_buttons, { margin.top: #(buttons_top) });
        let laps_top = if landscape { 8.0 } else { 24.0 };
        let mut laps = self.view.widget(cx, ids!(laps));
        script_apply_eval!(cx, laps, { margin.top: #(laps_top) });

        // Timer setup: landscape puts the wheel left of the label, presets and Start.
        if let Some(mut page) = self.view.view(cx, ids!(setup)).borrow_mut() {
            if landscape {
                set_layout(&mut page, Flow::right(), 24.0, Align { x: 0.5, y: 0.0 });
            } else {
                set_layout(&mut page, Flow::Down, 0.0, Align { x: 0.5, y: 0.0 });
            }
        }
        // Two columns of at most 320 pt, 24 apart, sharing whatever is there.
        let setup_col = column_width(page_w, 24.0, 320.0);
        set_width(cx, self.view.widget(cx, ids!(setup_wheel)), landscape.then_some(setup_col));
        set_width(cx, self.view.widget(cx, ids!(setup_form)), landscape.then_some(setup_col));
        let (wheel_h, wheel_top, label_top, start_top) = if landscape { ((page_h - 16.0).clamp(140.0, 220.0), 8.0, 8.0, 12.0) } else { (220.0, 14.0, 18.0, 32.0) };
        let mut wheel = self.view.widget(cx, ids!(duration_wheel));
        script_apply_eval!(cx, wheel, { height: #(wheel_h) });
        if let Some(mut v) = self.view.view(cx, ids!(setup_wheel)).borrow_mut() {
            v.layout.padding = Inset { top: wheel_top, bottom: 0.0, left: 0.0, right: 0.0 };
        }
        let (label_h, presets_top, start_h) = if landscape { (48.0, 12.0, 56.0) } else { (52.0, 16.0, 64.0) };
        let mut label = self.view.widget(cx, ids!(timer_label));
        script_apply_eval!(cx, label, { height: #(label_h) margin.top: #(label_top) });
        let mut presets = self.view.widget(cx, ids!(presets));
        script_apply_eval!(cx, presets, { margin.top: #(presets_top) });
        let mut start = self.view.widget(cx, ids!(timer_start));
        script_apply_eval!(cx, start, { height: #(start_h) margin.top: #(start_top) });

        // The alarm sheet: its wheel gives way so Label stays on a short screen.
        let sheet_wheel = (size.y - 210.0).clamp(120.0, 220.0);
        let mut alarm_wheel = self.view.widget(cx, ids!(alarm_wheel));
        script_apply_eval!(cx, alarm_wheel, { height: #(sheet_wheel) });

        // Timer running: the ring, and landscape its controls beside it.
        if let Some(mut page) = self.view.view(cx, ids!(running)).borrow_mut() {
            if landscape {
                set_layout(&mut page, Flow::right(), 32.0, Align { x: 0.5, y: 0.5 });
            } else {
                set_layout(&mut page, Flow::Down, 0.0, Align { x: 0.5, y: 0.0 });
            }
        }
        let ring = if landscape { (page_h - 12.0).clamp(120.0, 200.0) } else { 308.0 };
        set_width(cx, self.view.widget(cx, ids!(run_controls)), landscape.then_some((page_w - ring - 32.0).min(320.0)));
        let ring_top = if landscape { 0.0 } else { 18.0 };
        let mut ring_w = self.view.widget(cx, ids!(ring));
        script_apply_eval!(cx, ring_w, { width: #(ring) height: #(ring) margin.top: #(ring_top) });
        if let Some(mut r) = ring_w.borrow_mut::<TimerRing>() {
            r.set_stroke(cx, if landscape { 4.0 } else { 6.0 });
        }
        let run_top = if landscape { 0.0 } else { 42.0 };
        let mut run_buttons = self.view.widget(cx, ids!(run_buttons));
        script_apply_eval!(cx, run_buttons, { margin.top: #(run_top) });
        // The tile: the dial centred at 154, enlarged to 162 at 178, or a
        // landscape strip that puts the digital face beside a small dial.
        let tile_landscape = size.y < 120.0 && size.x > size.y * 1.4;
        let small = if tile_landscape { (size.y - 20.0).clamp(40.0, 104.0) } else if size.x >= 176.0 && size.y >= 176.0 { 162.0 } else { (size.y - 24.0).clamp(52.0, 154.0) };
        let mut tile = self.view.widget(cx, ids!(face_tile));
        script_apply_eval!(cx, tile, { width: #(small) height: #(small) });
        let pad = if tile_landscape { 10.0 } else { 16.0 };
        if let Some(mut d) = self.view.view(cx, ids!(digital)).borrow_mut() {
            d.layout.padding = Inset { left: pad, right: pad, top: pad, bottom: pad };
        }
        let time_size = if tile_landscape { 21.0 } else if size.x < 150.0 { 27.0 } else { 34.5 };
        let mut time_tile = self.view.widget(cx, ids!(time_tile));
        script_apply_eval!(cx, time_tile, { draw_text.text_style.font_size: #(time_size) height: #(time_size * 1.7) });
    }

    /// Start on the first event or draw, whichever comes first: a window
    /// seats this view before its `Startup` event, a module host after
    /// `create` mints it — either way before anything else reaches it.
    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        self.alarm_signal = alarm::install_sound(cx);
        if let Some(storage) = self.storage.as_ref() {
            self.alarm_load = Some(storage.get(cx, "alarms"));
            self.tile_face_load = Some(storage.get(cx, "tile_face"));
        }
        if let Some(mut wheel) = self.view.widget(cx, ids!(duration_wheel)).borrow_mut::<TimeWheel>() {
            wheel.set_duration(cx, 300);
        }
        self.select_mode(cx, 0, false);
        self.refresh_alarm(cx);
        self.tick = Some(cx.start_interval(1.0));
        self.refresh_clock(cx, true);
        self.refresh_stopwatch(cx);
        self.refresh_countdown(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let tabs_uid = self.view.widget(cx, ids!(tabs)).widget_uid();
        for action in actions.filter_widget_actions_cast::<PhoneTabsAction>(tabs_uid) {
            if let PhoneTabsAction::Selected(index) = action {
                self.select_mode(cx, index, false);
            }
        }
        if self.view.button(cx, ids!(more_action)).clicked(actions) {
            self.set_menu(cx, true);
        }
        if self.view.button(cx, ids!(tile_face_action)).clicked(actions) {
            self.toggle_tile_face(cx);
            self.set_menu(cx, false);
        }
        if self.view.view(cx, ids!(menu_scrim)).finger_down(actions).is_some() {
            self.set_menu(cx, false);
        }
        // A press taken away (a list or the host took the finger) is no tap.
        if self.view.view(cx, ids!(next_alarm)).finger_up(actions).is_some_and(|e| !e.cancelled) {
            self.select_mode(cx, 1, true);
        }
        if self.view.button(cx, ids!(alarm_add)).clicked(actions) {
            self.open_editor(cx, None);
        }
        if self.view.button(cx, ids!(alarm_cancel)).clicked(actions) {
            self.set_sheet(cx, false);
        }
        if self.view.button(cx, ids!(alarm_save)).clicked(actions) {
            self.save_alarm(cx);
        }
        if self.view.view(cx, ids!(scrim)).finger_down(actions).is_some() {
            self.set_sheet(cx, false);
        }
        let list_uid = self.view.widget(cx, ids!(alarm_list)).widget_uid();
        for action in actions.filter_widget_actions_cast::<AlarmListAction>(list_uid) {
            match action {
                AlarmListAction::Edit(id) => self.open_editor(cx, Some(id)),
                AlarmListAction::Toggle(id) => {
                    self.alarm.toggle(id);
                    self.alarms_modified(cx);
                }
                AlarmListAction::Remove(id) => {
                    self.alarm.remove(id);
                    self.alarms_modified(cx);
                }
                AlarmListAction::Add => self.open_editor(cx, None),
                AlarmListAction::None => {}
            }
        }
        // The modal: Snooze/Stop for an alarm, Repeat/Stop for a timer.
        if self.view.button(cx, ids!(modal_primary)).clicked(actions) {
            if self.alarm.ringing() {
                self.alarm.snooze();
            } else if self.countdown_alert {
                self.countdown_alert = false;
                let preset = self.countdown.preset;
                self.countdown.start(preset);
                self.refresh_countdown(cx);
            }
            self.refresh_alarm(cx);
        }
        if self.view.button(cx, ids!(modal_secondary)).clicked(actions) {
            self.alarm.stop();
            if self.countdown_alert {
                self.countdown_alert = false;
                self.countdown.cancel();
                self.refresh_countdown(cx);
            }
            self.refresh_alarm(cx);
        }
        // The stopwatch.
        if self.view.button(cx, ids!(sw_right)).clicked(actions) {
            self.stopwatch.toggle();
            self.sync_fast_timer(cx);
            self.refresh_stopwatch(cx);
        }
        if self.view.button(cx, ids!(sw_left)).clicked(actions) {
            if self.stopwatch.running() {
                self.stopwatch.lap();
            } else {
                self.stopwatch.reset();
                self.sync_fast_timer(cx);
            }
            self.refresh_stopwatch(cx);
        }
        // The timer.
        for (id, seconds) in [(ids!(preset_1), PRESETS[0]), (ids!(preset_5), PRESETS[1]), (ids!(preset_15), PRESETS[2])] {
            if self.view.button(cx, id).clicked(actions) {
                if let Some(mut wheel) = self.view.widget(cx, ids!(duration_wheel)).borrow_mut::<TimeWheel>() {
                    wheel.set_duration(cx, seconds as u64);
                }
                self.refresh_countdown(cx);
            }
        }
        if self.view.button(cx, ids!(timer_start)).clicked(actions) {
            let duration = self.view.widget(cx, ids!(duration_wheel)).borrow::<TimeWheel>().map(|w| w.duration()).unwrap_or(0);
            if duration > 0 {
                self.countdown.label = self.view.text_input(cx, ids!(timer_label)).text().chars().take(60).collect();
                self.countdown.start(duration as f64);
                self.countdown_alert = false;
                self.refresh_countdown(cx);
            }
        }
        if self.view.button(cx, ids!(timer_pause)).clicked(actions) {
            self.countdown.toggle();
            self.refresh_countdown(cx);
        }
        if self.view.button(cx, ids!(timer_reset)).clicked(actions) {
            self.countdown.reset();
            self.refresh_countdown(cx);
        }
        if self.view.button(cx, ids!(timer_cancel)).clicked(actions) {
            self.countdown_alert = false;
            self.countdown.cancel();
            self.refresh_modal(cx);
            self.refresh_countdown(cx);
        }
    }
}

impl Widget for ClockView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ensure_started(cx);
        if let Event::Storage(responses) = event {
            self.on_storage(cx, responses);
        }
        if let Event::AudioDevices(devices) = event {
            cx.use_audio_outputs(&devices.default_output());
        }
        if matches!(event, Event::LiveEdit) {
            self.select_mode(cx, self.mode, false);
            self.refresh_clock(cx, true);
            self.refresh_alarm(cx);
        }
        if self.tick.as_ref().is_some_and(|t| t.is_event(event).is_some()) {
            self.alarm.tick(LocalTime::now());
            self.refresh_alarm(cx);
            self.refresh_clock(cx, false);
            if !self.countdown.running() {
                self.refresh_countdown(cx);
            }
            if !self.stopwatch.running() {
                self.refresh_stopwatch(cx);
            }
        }
        if self.fast.as_ref().is_some_and(|t| t.is_event(event).is_some()) {
            if self.stopwatch.running() {
                self.refresh_stopwatch(cx);
            }
            if self.countdown.running() {
                self.refresh_countdown(cx);
            }
        }
        if self.sheet_frame.is_event(event).is_some() {
            self.step_sheet(cx);
        }
        if let Event::BackPressed { .. } = event {
            if self.sheet_open && event.back_pressed() {
                self.set_sheet(cx, false);
                return;
            }
            if self.menu_open(cx) && event.back_pressed() {
                self.set_menu(cx, false);
                return;
            }
        }
        if let Event::Actions(actions) = event {
            self.handle_actions(cx, actions);
        }
        // The sheet and the modal capture input above the content: while
        // one is up, the pages behind never see a press.
        let modal_up = self.view.widget(cx, ids!(modal)).visible() || self.sheet_open;
        let input = matches!(event, Event::MouseDown(_) | Event::MouseUp(_) | Event::MouseMove(_) | Event::TouchUpdate(_) | Event::Scroll(_));
        if modal_up && input {
            self.view.widget(cx, ids!(sheet_layer)).handle_event(cx, event, scope);
            self.view.widget(cx, ids!(modal)).handle_event(cx, event, scope);
            self.view.widget(cx, ids!(app_view)).borrow::<HostedView>();
        } else if self.menu_open(cx) && input {
            // The open menu takes every press: an item, or anywhere else to dismiss.
            self.view.widget(cx, ids!(menu_layer)).handle_event(cx, event, scope);
        } else {
            self.view.handle_event(cx, event, scope);
        }
        // Opened from an activity on the tile: land on that activity's page.
        let face = self.face(cx);
        if face != self.last_face {
            self.last_face = face;
            // The full face was left behind while only the tile showed.
            self.refresh_clock(cx, true);
            if face == HostedViewMode::Full {
                match activity(self.alarm.ringing(), self.countdown_alert, self.countdown.active(), self.stopwatch.running()) {
                    Activity::AlarmRinging => self.select_mode(cx, 1, false),
                    Activity::TimerDone | Activity::TimerRunning => self.select_mode(cx, 3, false),
                    Activity::StopwatchRunning => self.select_mode(cx, 2, false),
                    Activity::Clock => {}
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_started(cx);
        self.apply_secondary_ink(cx);
        let size = cx.turtle().rect().size;
        if size.x >= 1.0 && (size.x - self.last_size.x).abs() + (size.y - self.last_size.y).abs() > 0.5 {
            self.last_size = size;
            self.apply_sizing(cx, size);
        }
        self.view.draw_walk(cx, scope, walk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readouts_format() {
        assert_eq!(stopwatch_text(0.0), "00:00.00");
        assert_eq!(stopwatch_text(61.257), "01:01.25");
        assert_eq!(stopwatch_text(3661.0), "1:01:01");
        assert_eq!(hh_mm_ss(300.0), "05:00");
        assert_eq!(hh_mm_ss(3661.0), "1:01:01");
        let now = LocalTime { hour: 10, minute: 20, second: 30, ..Default::default() };
        assert_eq!(ends_text(now, 4.0 * 60.0), "Ends 10:24");
        assert_eq!(ends_text(LocalTime { hour: 23, minute: 59, second: 0, ..Default::default() }, 120.0), "Ends 00:01");
    }

    #[test]
    fn laps_are_durations_not_splits_and_the_current_lap_runs_on() {
        let mut sw = Stopwatch::default();
        assert_eq!(sw.current_lap(), None, "nothing to show before a start");
        sw.base = 10.0;
        sw.marks = vec![4.0, 10.0];
        assert_eq!(sw.laps(), vec![4.0, 6.0]);
        sw.base = 12.5;
        assert_eq!(sw.current_lap(), Some(2.5));
        sw.reset();
        assert!(sw.laps().is_empty() && sw.elapsed() == 0.0);
    }

    #[test]
    fn the_timer_starts_from_a_duration_pauses_resets_and_cancels() {
        let mut cd = Countdown::default();
        assert!(!cd.active());
        cd.start(90.0);
        assert!(cd.running() && cd.active());
        assert!(cd.fraction() > 0.99);
        cd.toggle();
        assert!(!cd.running() && cd.active(), "paused mid-way still counts as set");
        cd.reset();
        assert_eq!(cd.remaining(), 90.0);
        cd.cancel();
        assert!(!cd.active());
        cd.start(0.0);
        assert!(!cd.running() && !cd.finished(), "a zero duration never starts or reports done");
    }

    #[test]
    fn landscape_timer_columns_fit_from_the_breakpoint_up() {
        for width in [650.0, 700.0, 892.0, 1400.0] {
            let page_w = width - 32.0;
            let col = column_width(page_w, 24.0, 320.0);
            assert!(col * 2.0 + 24.0 <= page_w + 1e-9, "{width}: {col}");
        }
        assert_eq!(column_width(892.0 - 32.0, 24.0, 320.0), 320.0);
        assert_eq!(column_width(650.0 - 32.0, 24.0, 320.0), 297.0);
    }

    #[test]
    fn the_tile_activity_follows_the_priority_order() {
        assert_eq!(activity(true, true, true, true), Activity::AlarmRinging);
        assert_eq!(activity(false, true, true, true), Activity::TimerDone);
        assert_eq!(activity(false, false, true, true), Activity::TimerRunning);
        assert_eq!(activity(false, false, false, true), Activity::StopwatchRunning);
        assert_eq!(activity(false, false, false, false), Activity::Clock);
    }
}
