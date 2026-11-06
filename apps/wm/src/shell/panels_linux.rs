//! Linux status dropdowns, drawn from the worker's observed device state.
//!
//! The Display dropdown is about what the screens show, packed tight so
//! the whole of it fits a short screen without scrolling:
//!
//! * The arrangement diagram: the screens side by side, as the desktop has
//!   them, each box its screen at one scale for all, with the chips of the
//!   monitors it shows (the monitor's name, its connector, its mode) inside
//!   it, or under it when the boxes are too small for them. A screen of two
//!   or more monitors is a mirror ("Mirrored ×2"): rendered once at the
//!   size of the monitor it is optimised for (its chip says "optimised")
//!   and scaled to the others ("scaled from W×H"). The main screen says
//!   Main.
//! * Drag a chip onto a screen to mirror it, into a gap or the New screen
//!   slot for a screen of its own, or onto the Off strip; drag a screen
//!   into a gap to move it. The title row says what the drop does and
//!   the target lights up.
//! * A click picks a monitor; its details follow the Off strip: its mode
//!   (the resolution and refresh pickers, unfolding as chips), its screen
//!   (Make main and, in a mirror, Optimise for) and Move to (each screen,
//!   New screen, Off).
//! * Every edit goes into a draft and Apply sends all of it at once. One
//!   that reinitialises no monitor (a move, optimise for, main) is saved
//!   as soon as it shows; one that does (a mode, a monitor on or off) asks
//!   "Keep these settings? Reverting in 15 s" with Keep and Revert. A
//!   failure is noted under the row and the previous settings are sent
//!   again (`shell::display_menu`).
//! * The Desktop row chooses where the desktop's resolution comes from:
//!   "Panel" (the monitors, as above) or "Virtual" (one fixed virtual
//!   desktop the panels show scaled to fit, or not at all). Virtual adds
//!   its Size (follow the Desk Link viewer, a preset, or a custom size)
//!   and Panels (Mirror (scaled) / Off) rows in place of the diagram; the
//!   scale slider is then the virtual desktop's.
//! * Pace by picks the rate frames follow while several monitors show; it
//!   applies at once.
//! * Scale: one slider, live while dragged and saved on release, snapping
//!   to the multiples (ticks). It is one value for every screen: the
//!   desktop is one window with one scale, so a per-screen scale is not
//!   something the WM can show (the row says so).
//! * The last row opens Mouse & touchpad and the Render on GPU list.
use super::*;
use super::super::ui::HAlign;
use crate::linux_controls::ControlAction;
use crate::shell::display_layout::{connector_suffix, screens_span_gpus};
use crate::shell::display_menu::{
    mode_for, monitor_label, native_resolution, output_failure, pacing_choices, pacing_label, rate_after_resolution, rate_label, rates_for,
    resolution_label, resolutions, shown_mode, step_custom, virtual_mode_label, virtual_size_choices, virtual_size_label, CustomField, DisplayConfig, Phase,
    Place,
};
use crate::shell::system_linux::{Availability, BatteryStatus, GpuInfo, SystemCommand, DISPLAY_SCALE_MAX, DISPLAY_SCALE_MIN};
use makepad_widgets::makepad_platform::linux_display::{DisplayPacing, LinuxDisplayOutput, VirtualPanels};
use makepad_widgets::makepad_platform::linux_input::{
    natural_scroll, pointer_speed, POINTER_SPEED_MAX, POINTER_SPEED_MIN,
};

/// What a monitor's chip shows in the arrangement diagram.
struct OutputChip<'a> {
    name: &'a str,
    rate: &'a str,
    size: &'a (String, bool),
    optimised: bool,
    picked: bool,
    faded: bool,
}

const DEVICE_ROWS: usize = 5;
/// The launch default (`-scale=1.3`) shown until the platform reports the
/// window's actual scale.
const DPI_FALLBACK_HUNDREDTHS: u32 = 130;
// The Display dropdown's metrics, in logical points.
/// The title row.
const TITLE_H: f64 = 30.0;
/// A separator between sections.
const SEP_H: f64 = 9.0;
/// The picked monitor's title line.
const SCREEN_LINE_H: f64 = 22.0;
/// A row of controls (pickers, chips, Keep/Revert).
const CONTROL_H: f64 = 28.0;
/// The arrangement diagram: its padding, the gap between two screens (a
/// drop target: a new screen there, or a screen moved there), the New
/// screen slot, the tallest screen's box, the header over each box, a
/// monitor's chip and its narrowest, the gap between chips and a chip's
/// inset inside a box.
const DIAGRAM_PAD: f64 = 8.0;
const DIAGRAM_GAP: f64 = 14.0;
const NEW_SLOT_W: f64 = 56.0;
const BOX_MAX_H: f64 = 150.0;
const BOX_HEAD_H: f64 = 20.0;
const OUTPUT_CHIP_H: f64 = 48.0;
const CHIP_MIN_W: f64 = 92.0;
const CHIP_GAP: f64 = 3.0;
const CHIP_INSET: f64 = 4.0;
/// A screen with no mode yet is drawn this size.
const UNSIZED_SCREEN: (f64, f64) = (1920.0, 1080.0);
/// The Off strip under the diagram.
const OFF_ROW_H: f64 = 32.0;
/// How far a press on the diagram moves before it is a drag.
const DRAG_START: f64 = 4.0;
/// One line of a note (an error takes two).
const NOTE_LINE_H: f64 = 16.0;
/// A chip of an unfolded picker.
const CHIP_H: f64 = 26.0;
const RESOLUTION_CHIPS: usize = 3;
const RATE_CHIPS: usize = 4;
const PACE_CHIPS: usize = 3;
/// The Size picker's chips per row.
const SIZE_CHIPS: usize = 3;
/// The scale row's caption.
const CAPTION_H: f64 = 16.0;
/// The footer row and the Render on list's rows.
const PICKER_ROW_H: f64 = 26.0;
const LIST_ITEM_H: f64 = 26.0;
const GPU_ITEMS_MAX: usize = 4;
/// "Next start: …" under the Render on list.
const GPU_STATUS_H: f64 = 18.0;
const NOTICE_H: f64 = 18.0;
// The Mouse & touchpad subview.
const HERO_H: f64 = 40.0;
const HERO_COMPACT_H: f64 = 32.0;
const POINTER_SEP_H: f64 = 12.0;
const HEADER_H: f64 = 22.0;
const SLIDER_H: f64 = 34.0;
const ENDS_H: f64 = 14.0;
const POINTER_CAPTION_H: f64 = 18.0;

/// The scale slider row.
const SCALE_SLIDER_H: f64 = 30.0;

/// A scale slider drag (`ShellPanel::dpi_drag`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct DpiDrag {
    /// The scale when the drag began, in hundredths.
    pub start: u32,
    /// The track's left end and usable width, in device pixels.
    pub x0: f64,
    pub width: f64,
    /// The scale last applied, in hundredths.
    pub last: u32,
}

/// The multiples the slider snaps to and marks with ticks, in hundredths.
const SCALE_SNAPS: [u32; 6] = [100, 125, 150, 200, 250, 300];
/// How close (in hundredths) a drag has to come to a snap point to land on it.
const SCALE_SNAP_RADIUS: u32 = 4;

/// `value` on the nearest snap point when it is within the snap radius.
fn snap_dpi(value: u32) -> u32 {
    SCALE_SNAPS.into_iter().find(|s| s.abs_diff(value) <= SCALE_SNAP_RADIUS).unwrap_or(value)
}

/// The slider's 0..1 as a scale in hundredths, 100..=300.
fn dpi_from_slider(v: f64) -> u32 {
    let span = (DISPLAY_SCALE_MAX - DISPLAY_SCALE_MIN) as f64;
    (DISPLAY_SCALE_MIN as f64 + v.clamp(0.0, 1.0) * span).round() as u32
}

fn dpi_slider_progress(hundredths: u32) -> f64 {
    let clamped = hundredths.clamp(DISPLAY_SCALE_MIN, DISPLAY_SCALE_MAX);
    (clamped - DISPLAY_SCALE_MIN) as f64 / (DISPLAY_SCALE_MAX - DISPLAY_SCALE_MIN) as f64
}

/// The scale under device-pixel `x` on a track that started at `x0` and is
/// `width` device pixels wide.
fn dpi_at(x: f64, x0: f64, width: f64) -> u32 {
    snap_dpi(dpi_from_slider((x - x0) / width.max(1.0)))
}

/// Which picker of a screen is unfolded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Chooser {
    Resolution,
    Rate,
}

/// Changes made in the Display menu and not applied yet, made from `base`
/// (what ran then): a draft whose base no longer runs is dropped.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DisplayDraft {
    pub base: DisplayConfig,
    pub config: DisplayConfig,
}

/// What a press on the arrangement diagram holds: a monitor's chip or a
/// screen's box.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DragItem {
    Output(String),
    Screen(usize),
}

/// A press on the diagram: a click until the pointer has moved
/// `DRAG_START`, a drag after. `grab` is where in `origin` (the pressed
/// rect) it was taken, so the carried chip stays under the hand.
#[derive(Clone, Debug)]
pub(crate) struct DisplayDrag {
    pub item: DragItem,
    pub start: Vec2d,
    pub at: Vec2d,
    pub grab: Vec2d,
    pub origin: Rect,
    pub active: bool,
}

/// Where a drag can land: on screen `i` (a monitor mirrors it), into the
/// gap before screen `i` (`i` = the screen count: the New screen slot), or
/// on the Off strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DropAt {
    Screen(usize),
    Insert(usize),
    Off,
}

/// Where one screen sits in the arrangement diagram, relative to its area.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScreenSlot {
    /// All of it, top to bottom of the diagram: the drop target, and what a
    /// press drags.
    pub column: Rect,
    /// Its number and badges, over the box.
    pub head: Rect,
    /// The screen itself, to scale.
    pub screen: Rect,
    /// Its monitors' chips, top to bottom.
    pub chips: Vec<Rect>,
    /// The chips sit inside the box (else under it).
    pub inside: bool,
}

/// The arrangement diagram for screens of `screens` (their size in pixels
/// and how many monitors each shows), left to right along their top edges,
/// in an area `width` wide; and the area's height. One scale for every
/// screen, width and height alike, as large as the width allows with the
/// tallest box at most `BOX_MAX_H`. When every box then holds its chips,
/// they go inside; otherwise each screen's chips go under its box, in a
/// column at least `CHIP_MIN_W` wide (narrower only when the screens are
/// too many for that), and the scale shrinks until those columns fit.
fn diagram_layout(screens: &[((f64, f64), usize)], width: f64) -> (Vec<ScreenSlot>, f64) {
    let stack = |chips: usize| chips as f64 * (OUTPUT_CHIP_H + CHIP_GAP) - CHIP_GAP;
    let n = screens.len();
    if n == 0 {
        return (Vec::new(), DIAGRAM_PAD * 2.0 + BOX_HEAD_H + OUTPUT_CHIP_H);
    }
    let size: Vec<(f64, f64)> = screens.iter().map(|((w, h), _)| if *w >= 1.0 && *h >= 1.0 { (*w, *h) } else { UNSIZED_SCREEN }).collect();
    let room = (width - DIAGRAM_GAP * (n + 1) as f64 - NEW_SLOT_W - DIAGRAM_PAD).max(n as f64);
    let chip_w = CHIP_MIN_W.min(room / n as f64);
    let cap = BOX_MAX_H / size.iter().map(|s| s.1).fold(1.0, f64::max);
    let widest = (room / size.iter().map(|s| s.0).sum::<f64>()).min(cap);
    let inside = size
        .iter()
        .zip(screens)
        .all(|((w, h), (_, chips))| w * widest >= chip_w + CHIP_INSET * 2.0 && h * widest >= stack(*chips) + CHIP_INSET * 2.0);
    let scale = if inside {
        widest
    } else {
        // A box narrower than a chip gets a chip-wide column; the others
        // share what is left, at one scale.
        let mut floored = vec![false; n];
        let mut scale = room / size.iter().map(|s| s.0).sum::<f64>();
        loop {
            let short: Vec<usize> = (0..n).filter(|&i| !floored[i] && size[i].0 * scale < chip_w).collect();
            if short.is_empty() {
                break;
            }
            for i in short {
                floored[i] = true;
            }
            let free: f64 = (0..n).filter(|&i| !floored[i]).map(|i| size[i].0).sum();
            if free <= 0.0 {
                break;
            }
            scale = (room - chip_w * floored.iter().filter(|f| **f).count() as f64) / free;
        }
        scale.min(cap)
    };
    let top = DIAGRAM_PAD;
    let box_top = top + BOX_HEAD_H;
    let mut x = DIAGRAM_GAP;
    let mut slots = Vec::with_capacity(n);
    let mut bottom: f64 = box_top;
    for ((w, h), (_, chips)) in size.iter().zip(screens) {
        let (bw, bh) = ((w * scale).floor().max(1.0), (h * scale).round().max(1.0));
        let cw = if inside { bw } else { bw.max(chip_w.floor()) };
        let screen = rect(x + ((cw - bw) * 0.5).floor(), box_top, bw, bh);
        let chip_rects: Vec<Rect> = (0..*chips)
            .map(|k| {
                let step = k as f64 * (OUTPUT_CHIP_H + CHIP_GAP);
                if inside {
                    rect(screen.pos.x + CHIP_INSET, screen.pos.y + CHIP_INSET + step, bw - CHIP_INSET * 2.0, OUTPUT_CHIP_H)
                } else {
                    rect(x, box_top + bh + CHIP_INSET + step, cw, OUTPUT_CHIP_H)
                }
            })
            .collect();
        bottom = bottom.max(chip_rects.last().map_or(box_top + bh, |r| r.pos.y + r.size.y)).max(box_top + bh);
        slots.push(ScreenSlot { column: rect(x, top, cw, 0.0), head: rect(x, top, cw, BOX_HEAD_H), screen, chips: chip_rects, inside });
        x += cw + DIAGRAM_GAP;
    }
    let bottom = bottom.ceil();
    for slot in &mut slots {
        slot.column.size.y = bottom - top;
    }
    (slots, bottom + DIAGRAM_PAD)
}

/// A row of two buttons on the right: (its text, the left button, the
/// right button, what is left below).
fn button_row(rest: Rect) -> (Rect, Rect, Rect, Rect) {
    let button_w = 76.0;
    let (row, rest) = cut_top(rest, CONTROL_H);
    let inner = rect(row.pos.x, row.pos.y + 2.0, row.size.x, row.size.y - 4.0);
    let right = rect(inner.pos.x + inner.size.x - button_w, inner.pos.y, button_w, inner.size.y);
    let left = rect(right.pos.x - 4.0 - button_w, inner.pos.y, button_w, inner.size.y);
    let text = rect(inner.pos.x + 2.0, row.pos.y, (left.pos.x - inner.pos.x - 8.0).max(0.0), row.size.y);
    (text, left, right, rest)
}

impl ShellPanel {
    pub(super) fn audio_height(&self) -> f64 {
        let rows = match self.device_picker {
            Some(true) => self.system.audio.inputs.len(),
            Some(false) => self.system.audio.outputs.len(),
            None => 0,
        };
        288.0 + rows.min(DEVICE_ROWS) as f64 * 26.0
    }

    pub(super) fn system_action(&mut self, cx: &mut Cx, action: ControlAction) {
        cx.widget_action(self.uid, ShellPanelAction::System(action));
        self.redraw(cx);
    }

    pub(crate) fn close_gpu_lists(&mut self) {
        self.gpu_picker = false;
        self.gpu_scroll = 0;
        self.gpu_targets.clear();
    }

    /// Fold a screen's unfolded picker (its chips go with it).
    pub(super) fn close_chooser(&mut self) {
        self.chooser = None;
        self.chooser_changes.clear();
        self.pace_open = false;
        self.pace_targets.clear();
        self.virtual_size_open = false;
        self.virtual_size_targets.clear();
    }

    /// One list at a time: fold every Display list and drop a restart
    /// confirm that is waiting.
    pub(super) fn close_display_lists(&mut self) {
        self.close_gpu_lists();
        self.close_chooser();
        self.restart_confirm = false;
    }

    pub(super) fn system_press(&mut self, cx: &mut Cx, hit: Hit) {
        match hit {
            Hit::DevicePicker(input) => {
                self.device_picker = if self.device_picker == Some(input) { None } else { Some(input) };
                self.device_scroll = 0;
            }
            Hit::Device(input, index) => {
                let devices = if input { &self.system.audio.inputs } else { &self.system.audio.outputs };
                if let Some(device) = devices.get(index) {
                    let command = if input { SystemCommand::SelectInput(device.id.clone()) }
                        else { SystemCommand::SelectOutput(device.id.clone()) };
                    self.system_action(cx, ControlAction::Command(command));
                }
                self.device_picker = None;
            }
            Hit::InputMute => self.system_action(cx, ControlAction::Command(SystemCommand::ToggleInputMute)),
            Hit::OutputResolution | Hit::OutputRate => {
                let kind = if hit == Hit::OutputRate { Chooser::Rate } else { Chooser::Resolution };
                let name = self.display_selected(&self.display_config());
                let open = name.as_ref().is_some_and(|name| self.chooser.as_ref() != Some(&(name.clone(), kind)));
                self.close_display_lists();
                if open {
                    self.chooser = name.map(|name| (name, kind));
                }
            }
            Hit::ChooserItem(index) => {
                let change = self.chooser_changes.get(index).cloned();
                self.close_display_lists();
                if let Some((name, mode)) = change {
                    self.edit_display(|c| c.set_mode(&name, mode));
                }
            }
            Hit::DisplayOptimise(index) => {
                let name = self.display_optimise.get(index).cloned();
                self.close_display_lists();
                if let Some(name) = name {
                    self.edit_display(|c| match c.place_of(&name) {
                        Some(Place::Screen(screen)) => c.set_optimise_for(screen, &name),
                        _ => false,
                    });
                }
            }
            Hit::DisplayMakeMain => {
                let config = self.display_config();
                let screen = self.display_selected(&config).and_then(|name| config.place_of(&name));
                self.close_display_lists();
                if let Some(Place::Screen(screen)) = screen {
                    self.edit_display(|c| c.set_main(screen));
                }
            }
            Hit::DisplayMoveTo(index) => {
                let to = self.display_moves.get(index).copied();
                let name = self.display_selected(&self.display_config());
                self.close_display_lists();
                if let (Some(to), Some(name)) = (to, name) {
                    self.edit_display(|c| c.move_output(&name, to));
                }
            }
            Hit::DisplayApply => {
                let draft = self.display_drafted().cloned();
                self.close_display_lists();
                self.display_draft = None;
                if let Some(draft) = draft {
                    self.system_action(cx, ControlAction::DisplayApply(draft.config));
                }
            }
            Hit::DisplayDiscard => {
                self.close_display_lists();
                self.display_draft = None;
            }
            Hit::PacePicker => {
                let open = !self.pace_open;
                self.close_display_lists();
                self.pace_open = open;
            }
            Hit::PaceItem(index) => {
                let pacing = self.pace_targets.get(index).cloned();
                self.close_display_lists();
                if let Some(pacing) = pacing.filter(|p| *p != self.display_snapshot.pacing) {
                    self.system_action(cx, ControlAction::DisplayPacing(pacing));
                }
            }
            Hit::Desktop(virtual_) => {
                let now = self.virtual_desktop();
                self.close_display_lists();
                if virtual_ != now {
                    self.system_action(cx, if virtual_ { ControlAction::DesktopVirtual } else { ControlAction::DesktopPanel });
                }
            }
            Hit::VirtualSizePicker => {
                let open = !self.virtual_size_open;
                self.close_display_lists();
                self.virtual_size_open = open;
            }
            Hit::VirtualSizeItem(index) => {
                let size = self.virtual_size_targets.get(index).copied();
                self.close_display_lists();
                if let Some(size) = size {
                    self.system_action(cx, ControlAction::VirtualSize(size));
                }
            }
            Hit::VirtualCustom(field, dir) => {
                let size = self.virtual_saved.map(|saved| step_custom(&saved.mode, field, dir));
                self.close_display_lists();
                if let Some(size) = size {
                    self.system_action(cx, ControlAction::VirtualSize(size));
                }
            }
            Hit::VirtualPanels(off) => {
                self.close_display_lists();
                self.system_action(cx, ControlAction::VirtualPanels(if off { VirtualPanels::Off } else { VirtualPanels::Mirror }));
            }
            Hit::TrialKeep => self.system_action(cx, ControlAction::DisplayKeep),
            Hit::TrialRevert => self.system_action(cx, ControlAction::DisplayRevert),
            Hit::RestartDesktop => {
                self.close_display_lists();
                self.restart_confirm = true;
            }
            Hit::RestartCancel => self.restart_confirm = false,
            Hit::RestartConfirm => {
                // Only while a restart would still change something.
                let still = self.restart_row_shown();
                self.close_display_lists();
                if still {
                    self.system_action(cx, ControlAction::RestartDesktop);
                }
            }
            Hit::GpuPicker => {
                let open = !self.gpu_picker;
                self.close_display_lists();
                self.gpu_picker = open;
                self.gpu_scroll = if open { self.gpu_saved_row() } else { 0 };
            }
            Hit::PointerSettings => {
                self.pointer_settings = !self.pointer_settings;
                self.dragging = None;
                self.close_display_lists();
            }
            Hit::NaturalScroll(touchpad) => {
                self.system_action(cx, ControlAction::NaturalScroll { touchpad, on: !natural_scroll(touchpad) });
            }
            Hit::Gpu(row) => {
                let choice = self.gpu_targets.get(row).cloned();
                self.close_gpu_lists();
                if let Some(choice) = choice {
                    if choice.as_deref().is_none_or(|identity| self.gpu_identity_available(identity)) {
                        self.system_action(cx, ControlAction::GpuChoice(choice));
                    }
                }
            }
            _ => {}
        }
        self.redraw(cx);
    }


    pub(super) fn device_scroll_by(&mut self, cx: &mut Cx, dy: f64) {
        let n = if self.device_picker == Some(true) { self.system.audio.inputs.len() }
            else { self.system.audio.outputs.len() };
        self.device_scroll = if dy > 0.0 { (self.device_scroll + 1).min(n.saturating_sub(DEVICE_ROWS)) }
            else { self.device_scroll.saturating_sub(1) };
        self.redraw(cx);
    }

    fn draw_device_picker(&mut self, cx: &mut Cx2d, body: Rect, input: bool) -> Rect {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let snapshot = self.system.clone();
        let devices = if input { &snapshot.audio.inputs } else { &snapshot.audio.outputs };
        let selected = devices.iter().find(|d| d.is_default);
        let (row, mut rest) = cut_top(body, 30.0);
        let hit = Hit::DevicePicker(input);
        self.d.cursor_surface(cx, row, &tok.controls, self.hot == Some(hit), self.device_picker == Some(input));
        self.d.label_elided(cx, rect(row.pos.x + 5.0, row.pos.y, (row.size.x - 35.0).max(0.0), row.size.y), false, tok.font.body, fg,
            HAlign::Left,
            selected.map(|d| d.label.as_str()).unwrap_or(if input { "Choose input" } else { "Choose output" }));
        self.d.icon_centered(cx, Ico::ChevronDown,
            rect(row.pos.x + row.size.x - 22.0, row.pos.y, 18.0, row.size.y), 12.0, fg);
        if !devices.is_empty() { self.hits.push((hit, row)); }
        if self.device_picker == Some(input) {
            self.device_scroll = self.device_scroll.min(devices.len().saturating_sub(DEVICE_ROWS));
            for (index, device) in devices.iter().enumerate().skip(self.device_scroll).take(DEVICE_ROWS) {
                let (row, next) = cut_top(rest, 26.0); rest = next;
                let hit = Hit::Device(input, index);
                self.d.cursor_surface(cx, row, &tok.controls, self.hot == Some(hit), device.is_default);
                self.d.label_elided(cx, rect(row.pos.x + 8.0, row.pos.y, row.size.x - 35.0, row.size.y),
                    device.is_default, tok.font.body_small, fg, HAlign::Left, &device.label);
                if device.is_default {
                    self.d.icon_centered(cx, Ico::Check,
                        rect(row.pos.x + row.size.x - 22.0, row.pos.y, 18.0, row.size.y), 12.0, fg);
                }
                self.hits.push((hit, row));
            }
        }
        rest
    }

    pub(super) fn draw_audio(&mut self, cx: &mut Cx2d, body: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let snapshot = self.system.clone();
        let audio = &snapshot.audio;
        let (hero, mut rest) = cut_top(body, 40.0);
        self.d.panel_hero(cx, hero, &tok, fg, Ico::Speaker, "Sound", "Output and microphone", 0.0);
        for input in [false, true] {
            let device = if input { audio.default_input() } else { audio.default_output() };
            let (sep, next) = cut_top(rest, 12.0); rest = next;
            self.d.separator(cx, sep, fg, 0.12);
            let (header, next) = cut_top(rest, 24.0); rest = next;
            let value = device.map(|d| if d.muted { "Muted".into() } else { format!("{}%", d.percent) })
                .unwrap_or_else(|| "Unavailable".into());
            self.section_header(cx, rect(header.pos.x, header.pos.y, header.size.x - 56.0, header.size.y),
                if input { "INPUT" } else { "OUTPUT" }, &value);
            if let Some(device) = device {
                let switch = self.d.toggle_switch(cx, dvec2(header.pos.x + header.size.x - 42.0, header.pos.y),
                    &tok, !device.muted, fg);
                self.hits.push((if input { Hit::InputMute } else { Hit::MuteToggle }, switch));
            }
            let (slider, next) = cut_top(rest, 34.0); rest = next;
            self.slider_row(cx, slider, if input { Hit::InputSlider } else { Hit::VolumeSlider },
                device.map_or(0.0, |d| d.percent as f64 / 100.0), device.is_some());
            rest = self.draw_device_picker(cx, rest, input);
        }
        let message = if !self.system_notice.is_empty() { self.system_notice.clone() }
            else if let Availability::Unavailable(reason) = &audio.availability { reason.clone() }
            else { String::new() };
        if !message.is_empty() {
            self.d.label_elided(cx, rest, false, tok.font.caption, fg, HAlign::Left, &message);
        }
    }

    // ----------------------------------------------------------- monitor

    /// The slider's 0..1 as a pointer speed in hundredths, 25..=300.
    pub(super) fn pointer_from_slider(v: f64) -> u32 {
        let span = (POINTER_SPEED_MAX - POINTER_SPEED_MIN) as f64;
        (POINTER_SPEED_MIN as f64 + v.clamp(0.0, 1.0) * span).round() as u32
    }

    fn pointer_slider_progress(hundredths: u32) -> f64 {
        let clamped = hundredths.clamp(POINTER_SPEED_MIN, POINTER_SPEED_MAX);
        (clamped - POINTER_SPEED_MIN) as f64 / (POINTER_SPEED_MAX - POINTER_SPEED_MIN) as f64
    }

    /// A press on the scale slider: the track is kept in device pixels and
    /// the scale under the pointer applies at once.
    pub(super) fn dpi_drag_begin(&mut self, cx: &mut Cx, p: Vec2d) {
        let Some((_, r)) = self.hits.iter().find(|(h, _)| *h == Hit::DpiScaleSlider).copied() else { return };
        // The same usable track as `slider_value`.
        let track = inset(r, 6.0);
        let dpi = self.dpi_hundredths() as f64 / 100.0;
        let start = self.dpi_hundredths();
        self.dragging = Some(Hit::DpiScaleSlider);
        self.dpi_drag = Some(DpiDrag { start, x0: (track.pos.x + 6.0) * dpi, width: (track.size.x - 12.0) * dpi, last: start });
        self.dpi_drag_to(cx, p);
    }

    /// The pointer moved during a scale drag: a new value applies live.
    pub(super) fn dpi_drag_to(&mut self, cx: &mut Cx, p: Vec2d) {
        let Some(mut drag) = self.dpi_drag else { return };
        let dpi = self.dpi_hundredths() as f64 / 100.0;
        let value = dpi_at(p.x * dpi, drag.x0, drag.width);
        if value != drag.last {
            drag.last = value;
            self.dpi_drag = Some(drag);
            self.system_action(cx, ControlAction::DpiScaleLive(value));
        }
    }

    /// The release: the scale reached is saved.
    pub(super) fn dpi_drag_end(&mut self, cx: &mut Cx) {
        if let Some(drag) = self.dpi_drag.take() {
            self.system_action(cx, ControlAction::DpiScale(drag.last));
        }
    }

    /// A scale drag is under way: a saved scale landing now would move the
    /// window under the pointer, so the WM waits.
    pub(crate) fn dpi_adjusting(&self) -> bool {
        self.dpi_drag.is_some()
    }

    /// The window's actual scale in hundredths, or the launch default until
    /// the platform has reported one.
    fn dpi_hundredths(&self) -> u32 {
        if self.current_dpi.is_finite() && self.current_dpi > 0.0 {
            (self.current_dpi * 100.0).round().clamp(1.0, u32::MAX as f64) as u32
        } else {
            DPI_FALLBACK_HUNDREDTHS
        }
    }

    /// How many screens make up the desktop, as the renderer reports them.
    fn display_status(&self) -> String {
        let s = &self.display_snapshot;
        if !s.direct {
            return "Managed by the desktop".into();
        }
        let mirrored = s.screens.iter().filter(|screen| screen.outputs.len() > 1).count();
        let screens = match (s.outputs.len(), s.screens.len()) {
            (0, _) => return "No outputs".into(),
            (_, 0) => return "No active output".into(),
            (_, 1) => "One screen".to_string(),
            (_, n) => format!("{n} screens"),
        };
        if mirrored > 0 { format!("{screens} \u{b7} {mirrored} mirrored") } else { screens }
    }

    /// The GPU driving `output`: the sysfs label of the card with its PCI
    /// address, else its card name.
    fn screen_gpu_label(&self, output: &LinuxDisplayOutput) -> String {
        output
            .pci
            .as_deref()
            .and_then(|pci| self.system.gpus.iter().find(|gpu| !gpu.pci.is_empty() && gpu.pci == pci))
            .map(|gpu| gpu.label())
            .unwrap_or_else(|| output.card.clone())
    }

    /// The desktop is the virtual display (chosen, or the renderer shows it).
    fn virtual_desktop(&self) -> bool {
        self.virtual_saved.is_some() || self.display_snapshot.virtual_display.is_some()
    }

    /// The arrangement diagram and its details show: the WM drives the
    /// monitors, the desktop is theirs (not the virtual display) and one
    /// of them is on a screen.
    fn arranging(&self) -> bool {
        self.display_snapshot.direct && !self.virtual_desktop() && !self.display_config().arrangement.screens.is_empty()
    }

    /// The Desktop block: the switch, and while virtual the Size row (its
    /// unfolded chips), the Custom steppers and the Panels row; then a
    /// separator.
    fn desktop_height(&self) -> f64 {
        if !self.display_snapshot.direct {
            return 0.0;
        }
        let mut h = CONTROL_H;
        if let Some(saved) = &self.virtual_saved {
            h += CONTROL_H;
            if self.virtual_size_open {
                h += virtual_size_choices(saved).len().div_ceil(SIZE_CHIPS) as f64 * CHIP_H;
            }
            if !saved.follow {
                h += CONTROL_H;
            }
            h += CONTROL_H;
        }
        h + SEP_H
    }

    // ---- the configuration ---------------------------------------------

    /// What the renderer runs, or the configuration being tried or restored.
    fn display_running(&self) -> DisplayConfig {
        self.display_trials.shown(DisplayConfig::running(&self.display_snapshot))
    }

    /// The configuration the menu shows: the draft, else what runs.
    pub(super) fn display_config(&self) -> DisplayConfig {
        let running = self.display_running();
        match &self.display_draft {
            Some(draft) if draft.base == running => draft.config.clone(),
            _ => running,
        }
    }

    /// The draft that still applies: made from what runs now.
    fn display_drafted(&self) -> Option<&DisplayDraft> {
        let running = self.display_running();
        self.display_draft.as_ref().filter(|d| d.base == running)
    }

    /// `config` is the draft; one that is what runs is no draft.
    fn set_display_draft(&mut self, config: DisplayConfig) {
        let base = self.display_running();
        self.display_draft = (config != base).then_some(DisplayDraft { base, config });
    }

    /// One edit of the configuration shown; kept as the draft when it
    /// changed something.
    fn edit_display(&mut self, edit: impl FnOnce(&mut DisplayConfig) -> bool) {
        let mut config = self.display_config();
        if edit(&mut config) {
            self.set_display_draft(config);
        }
    }

    /// The monitor whose details show: the one picked, else the main
    /// screen's first.
    fn display_selected(&self, config: &DisplayConfig) -> Option<String> {
        self.display_selected
            .clone()
            .filter(|name| config.place_of(name).is_some())
            .or_else(|| config.main.clone())
            .or_else(|| config.arrangement.screens.first().and_then(|s| s.outputs.first().cloned()))
    }

    /// What dropping `item` on `on` makes of `config`: `None` when it
    /// changes nothing (a monitor dropped where it is, a screen next to
    /// itself, the last monitor turned off).
    fn display_drop_result(config: &DisplayConfig, item: &DragItem, on: DropAt) -> Option<DisplayConfig> {
        let mut out = config.clone();
        let changed = match (item, on) {
            (DragItem::Output(name), DropAt::Screen(i)) => out.move_output(name, Place::Screen(i)),
            (DragItem::Output(name), DropAt::Insert(k)) => out.move_output(name, Place::NewScreen(k)),
            (DragItem::Output(name), DropAt::Off) => out.move_output(name, Place::Off),
            (DragItem::Screen(i), DropAt::Insert(k)) => out.move_screen(*i, k),
            (DragItem::Screen(_), _) => false,
        };
        changed.then_some(out)
    }

    /// The drop target under `p` for the drag in progress, when dropping
    /// there would change something. A screen dropped on another lands on
    /// the nearer side of it.
    fn display_drop_at(&self, config: &DisplayConfig, p: Vec2d) -> Option<DropAt> {
        let drag = self.display_drag.as_ref().filter(|d| d.active)?;
        let (on, r) = self.display_drops.iter().find(|(_, r)| contains(*r, p)).copied()?;
        let on = match (&drag.item, on) {
            (DragItem::Screen(_), DropAt::Screen(i)) => DropAt::Insert(if p.x < r.pos.x + r.size.x * 0.5 { i } else { i + 1 }),
            (_, on) => on,
        };
        Self::display_drop_result(config, &drag.item, on).map(|_| on)
    }

    // ---- heights -------------------------------------------------------

    /// The chips of `output`'s unfolded picker: how many rows they take.
    fn chooser_rows(&self, output: &LinuxDisplayOutput, mode: Option<&str>) -> usize {
        if self.chooser.as_ref().is_none_or(|(name, _)| *name != output.name) {
            return 0;
        }
        match self.chooser.as_ref().map(|(_, kind)| *kind) {
            Some(Chooser::Resolution) => resolutions(&output.modes, native_resolution(output)).len().div_ceil(RESOLUTION_CHIPS),
            Some(Chooser::Rate) => {
                let (w, h, _) = shown_mode(output, mode).unwrap_or_default();
                rates_for(&output.modes, w, h).len().div_ceil(RATE_CHIPS)
            }
            None => 0,
        }
    }

    /// The diagram's screens for `config` in a body `width` wide, and its height.
    fn diagram_plan(&self, config: &DisplayConfig, width: f64) -> (Vec<ScreenSlot>, f64) {
        let snapshot = &self.display_snapshot;
        let screens: Vec<((f64, f64), usize)> = config
            .arrangement
            .screens
            .iter()
            .enumerate()
            .map(|(i, screen)| {
                let (w, h) = config.screen_size(snapshot, i);
                ((w as f64, h as f64), screen.outputs.len())
            })
            .collect();
        diagram_layout(&screens, width)
    }

    /// The Off strip, the separator and the picked monitor's details.
    fn details_height(&self, config: &DisplayConfig) -> f64 {
        let mut h = OFF_ROW_H + SEP_H;
        let Some(name) = self.display_selected(config) else { return h };
        let Some(output) = self.display_snapshot.output(&name) else { return h };
        h += SCREEN_LINE_H + CONTROL_H + self.chooser_rows(output, config.mode(&name)) as f64 * CHIP_H;
        if let Some(Place::Screen(i)) = config.place_of(&name) {
            h += CONTROL_H;
            if config.arrangement.screens[i].outputs.len() > 1 {
                h += CONTROL_H;
            }
        }
        h + CONTROL_H
    }

    /// The trial's Keep / Revert row, the draft's Apply row, then the note.
    fn apply_height(&self) -> f64 {
        let mut h = 0.0;
        if self.display_trials.trial.is_some() {
            h += CONTROL_H;
        }
        if self.display_drafted().is_some() {
            h += CONTROL_H;
        }
        h + match &self.display_trials.note {
            Some(note) if note.error => NOTE_LINE_H * 2.0,
            Some(_) => NOTE_LINE_H,
            None => 0.0,
        }
    }

    /// Pace by is offered while more than one monitor shows.
    fn pace_shown(&self) -> bool {
        self.display_snapshot.direct
            && !self.virtual_desktop()
            && self.display_snapshot.outputs.iter().filter(|o| o.active).count() > 1
    }

    fn pace_height(&self) -> f64 {
        if !self.pace_shown() {
            return 0.0;
        }
        let rows = if self.pace_open { pacing_choices(&self.display_snapshot).len().div_ceil(PACE_CHIPS) } else { 0 };
        CONTROL_H + rows as f64 * CHIP_H
    }

    fn gpu_items(&self) -> usize {
        if self.gpu_picker { (1 + self.system.gpus.len()).min(GPU_ITEMS_MAX) } else { 0 }
    }

    fn gpu_status_shown(&self) -> bool {
        self.display_snapshot.direct && !self.system.gpus.is_empty()
    }

    /// The card height the Display dropdown asks for in a body `width`
    /// wide: exactly what `draw_monitor` lays out.
    pub(super) fn monitor_height(&self, width: f64) -> f64 {
        if self.pointer_settings {
            return self.pointer_settings_height();
        }
        let mut h = TITLE_H + SEP_H;
        if self.arranging() {
            let config = self.display_config();
            h += self.diagram_plan(&config, width).1 + self.details_height(&config);
        } else {
            h += SCREEN_LINE_H;
        }
        h += self.apply_height() + SEP_H + self.desktop_height() + self.pace_height();
        h += SEP_H + CAPTION_H + SCALE_SLIDER_H + SEP_H + PICKER_ROW_H;
        h += self.gpu_items() as f64 * LIST_ITEM_H;
        if self.gpu_status_shown() {
            h += GPU_STATUS_H;
        }
        if self.restart_row_shown() {
            h += PICKER_ROW_H;
        }
        if !self.system_notice.is_empty() {
            h += NOTICE_H;
        }
        h
    }

    fn pointer_settings_height(&self) -> f64 {
        let notice = if self.system_notice.is_empty() { 0.0 } else { NOTICE_H };
        HERO_H + POINTER_SEP_H + PICKER_ROW_H
            + POINTER_SEP_H + HEADER_H + SLIDER_H + ENDS_H + PICKER_ROW_H
            + POINTER_SEP_H + HEADER_H + SLIDER_H + ENDS_H + PICKER_ROW_H
            + POINTER_CAPTION_H
            + notice
    }

    /// The wheel over the unfolded "Render on" list; the draw clamps.
    pub(super) fn gpu_scroll_by(&mut self, cx: &mut Cx, dy: f64) {
        self.gpu_scroll = if dy > 0.0 { self.gpu_scroll + 1 } else { self.gpu_scroll.saturating_sub(1) };
        self.redraw(cx);
    }

    // ---- drawing -------------------------------------------------------

    pub(super) fn draw_monitor(&mut self, cx: &mut Cx2d, body: Rect) {
        if self.pointer_settings {
            self.draw_pointer_settings(cx, body);
            return;
        }
        let tok = self.tokens;
        let fg = tok.popups.text;
        let snapshot = self.display_snapshot.clone();
        self.chooser_changes.clear();
        self.display_chips.clear();
        self.display_moves.clear();
        self.display_optimise.clear();
        if self.display_draft.is_some() && self.display_drafted().is_none() {
            // What runs changed under the draft (a monitor came or went,
            // a change was applied): it no longer says what to change.
            self.display_draft = None;
        }
        let config = self.display_config();
        if self.chooser.as_ref().is_some_and(|(name, _)| config.place_of(name).is_none()) {
            self.chooser = None;
        }
        if self.display_drag.as_ref().is_some_and(|d| match &d.item {
            DragItem::Output(name) => config.place_of(name).is_none(),
            DragItem::Screen(i) => *i >= config.arrangement.screens.len(),
        }) {
            self.display_drag = None;
        }
        let arranging = self.arranging();
        // Where the drag would land, against the drop targets drawn last
        // frame (nothing moves under a drag: only the drop changes it).
        let target = self.display_drag.as_ref().filter(|d| d.active).and_then(|d| self.display_drop_at(&config, d.at));
        self.display_drops.clear();

        let (title, mut rest) = cut_top(body, TITLE_H);
        self.draw_title_row(cx, title, &config, target);
        let (sep, next) = cut_top(rest, SEP_H);
        rest = next;
        self.d.separator(cx, sep, fg, 0.12);
        if arranging {
            let (plan, height) = self.diagram_plan(&config, body.size.x);
            let (area, next) = cut_top(rest, height);
            rest = next;
            self.draw_diagram(cx, area, &config, &plan, target);
            rest = self.draw_details(cx, rest, &config, target);
        } else {
            let (row, next) = cut_top(rest, SCREEN_LINE_H);
            rest = next;
            let line = match snapshot.virtual_display {
                Some(mode) if snapshot.direct => format!("Virtual display \u{b7} {} \u{b7} panels {}", virtual_mode_label(&mode),
                    if mode.panels == VirtualPanels::Off { "off" } else { "mirror the desktop" }),
                _ if snapshot.direct && self.virtual_desktop() => "Virtual display".to_string(),
                _ if snapshot.direct => "No connected screen".to_string(),
                _ => "The desktop owns the screens; the scale below still applies".to_string(),
            };
            self.unavailable(cx, row, &line);
        }
        rest = self.draw_apply(cx, rest);
        let (sep, next) = cut_top(rest, SEP_H);
        rest = next;
        self.d.separator(cx, sep, fg, 0.12);
        if snapshot.direct {
            let (area, next) = cut_top(rest, self.desktop_height() - SEP_H);
            rest = next;
            self.draw_desktop(cx, area);
            let (sep, next) = cut_top(rest, SEP_H);
            rest = next;
            self.d.separator(cx, sep, fg, 0.12);
        } else {
            self.virtual_size_open = false;
            self.virtual_size_targets.clear();
        }
        if self.pace_shown() {
            let (area, next) = cut_top(rest, self.pace_height());
            rest = next;
            self.draw_pace(cx, area);
        } else {
            self.pace_open = false;
            self.pace_targets.clear();
        }
        let (sep, next) = cut_top(rest, SEP_H);
        rest = next;
        self.d.separator(cx, sep, fg, 0.12);
        rest = self.draw_scale(cx, rest);
        let (sep, next) = cut_top(rest, SEP_H);
        rest = next;
        self.d.separator(cx, sep, fg, 0.12);
        rest = self.draw_footer(cx, rest);
        if !self.system_notice.is_empty() {
            let notice = self.system_notice.clone();
            let (row, _) = cut_top(rest, NOTICE_H);
            self.d.label_elided(cx, row, false, tok.font.caption, fg, HAlign::Left, &notice);
        }
        if arranging {
            self.draw_drag_ghost(cx, body);
        }
    }

    /// `[▭] Display · 3 screens · 1 mirrored`; while a monitor or a screen
    /// is dragged, what dropping it where it is would do.
    fn draw_title_row(&mut self, cx: &mut Cx2d, row: Rect, config: &DisplayConfig, target: Option<DropAt>) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        self.d.icon_centered(cx, Ico::Monitor, rect(row.pos.x, row.pos.y, 22.0, row.size.y), 16.0, fg);
        let title_x = row.pos.x + 28.0;
        let title_w = self.d.measure(cx, true, tok.font.body, "Display");
        self.d.label(cx, rect(title_x, row.pos.y, title_w, row.size.y), true, tok.font.body, fg, HAlign::Left, "Display");
        let (status, color) = match self.display_drag.as_ref().filter(|d| d.active) {
            Some(drag) => (self.drop_hint(config, &drag.item, target), tok.bar.active),
            None => (self.display_status(), dim),
        };
        let status_x = title_x + title_w + 8.0;
        self.d.label_elided(cx, rect(status_x, row.pos.y, (row.pos.x + row.size.x - status_x).max(0.0), row.size.y),
            false, tok.font.caption, color, HAlign::Left, &status);
    }

    /// What dropping `item` on `on` does, in words.
    fn drop_hint(&self, config: &DisplayConfig, item: &DragItem, on: Option<DropAt>) -> String {
        let screens = config.arrangement.screens.len();
        let title = |name: &str| self.display_snapshot.output(name).map_or_else(|| connector_suffix(name), monitor_label);
        match (item, on) {
            (DragItem::Output(name), Some(DropAt::Screen(i))) => format!("{} will mirror screen {}", title(name), i + 1),
            (DragItem::Output(name), Some(DropAt::Insert(k))) if k >= screens => format!("{} on a new screen at the right", title(name)),
            (DragItem::Output(name), Some(DropAt::Insert(k))) => format!("{} on a new screen left of screen {}", title(name), k + 1),
            (DragItem::Output(name), Some(DropAt::Off)) => format!("Turn {} off", title(name)),
            (DragItem::Screen(i), Some(DropAt::Insert(k))) if k >= screens => format!("Screen {} to the right end", i + 1),
            (DragItem::Screen(i), Some(DropAt::Insert(k))) => format!("Screen {} left of screen {}", i + 1, k + 1),
            (DragItem::Output(_), _) => "Drop on a screen to mirror it, in a gap for a new screen".into(),
            (DragItem::Screen(_), _) => "Drop in a gap between screens to move it".into(),
        }
    }

    /// The arrangement diagram: the screens to scale, left to right along
    /// their top edges as the desktop has them, each under its number and
    /// badges, with the chips of the monitors it shows inside it or under
    /// it (`diagram_layout`), and a New screen slot at the right end. A
    /// screen's column takes a monitor (mirroring it); the gaps between
    /// columns take a monitor (a new screen there) or a screen (moved
    /// there).
    fn draw_diagram(&mut self, cx: &mut Cx2d, area: Rect, config: &DisplayConfig, plan: &[ScreenSlot], target: Option<DropAt>) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let accent = tok.bar.active;
        let snapshot = self.display_snapshot.clone();
        let selected = self.display_selected(config);
        let main = config.main_screen();
        let drag = self.display_drag.clone().filter(|d| d.active);
        let at = |r: Rect| rect(area.pos.x + r.pos.x, area.pos.y + r.pos.y, r.size.x, r.size.y);
        let n = plan.len();
        let top = area.pos.y + DIAGRAM_PAD;
        let tall = (area.size.y - DIAGRAM_PAD * 2.0).max(0.0);
        self.d.bordered(cx, area, alpha(fg, 0.03), alpha(fg, 0.08), alpha(fg, 0.08), 0.0, 1.0);

        // The gaps: a bar marks where the drop lands.
        for k in 0..=n {
            let left = if k == 0 { area.pos.x } else { at(plan[k - 1].column).pos.x + plan[k - 1].column.size.x };
            let right = if k == n { left + DIAGRAM_GAP } else { at(plan[k].column).pos.x };
            let gap = rect(left, top, (right - left).max(1.0), tall);
            self.display_drops.push((DropAt::Insert(k), gap));
            if target == Some(DropAt::Insert(k)) && k < n {
                self.d.solid(cx, rect(gap.pos.x + gap.size.x * 0.5 - 1.5, gap.pos.y, 3.0, gap.size.y), accent);
            }
        }
        // The New screen slot.
        let slot_x = plan.last().map_or(area.pos.x, |slot| at(slot.column).pos.x + slot.column.size.x) + DIAGRAM_GAP;
        let slot = rect(slot_x, top, (area.pos.x + area.size.x - DIAGRAM_PAD - slot_x).max(0.0), tall);
        let hot = target == Some(DropAt::Insert(n));
        let (fill, line) = if hot { (alpha(accent, 0.16), accent) } else { (alpha(fg, 0.02), alpha(fg, 0.16)) };
        self.d.bordered(cx, slot, fill, line, line, 0.0, if hot { 2.0 } else { 1.0 });
        let text = if hot { accent } else { dim };
        self.d.label(cx, rect(slot.pos.x, slot.pos.y + slot.size.y * 0.5 - 20.0, slot.size.x, 18.0), false, tok.font.body, text, HAlign::Center, "+");
        let words = self.d.wrap(cx, false, tok.font.caption, "New screen", (slot.size.x - 4.0).max(1.0), 2);
        for (i, word) in words.iter().enumerate() {
            let r = rect(slot.pos.x, slot.pos.y + slot.size.y * 0.5 + i as f64 * 13.0 - 2.0, slot.size.x, 13.0);
            self.d.label(cx, r, false, tok.font.caption, text, HAlign::Center, word);
        }
        self.display_drops.push((DropAt::Insert(n), slot));

        let dragged_screen = drag.as_ref().and_then(|d| match d.item {
            DragItem::Screen(i) => Some(i),
            DragItem::Output(_) => None,
        });
        for (i, screen) in config.arrangement.screens.iter().enumerate() {
            let Some(place) = plan.get(i) else { break };
            let column = at(place.column);
            let b = at(place.screen);
            let mirrored = screen.outputs.len() > 1;
            let is_main = main == Some(i);
            let drop_here = target == Some(DropAt::Screen(i));
            let picked = selected.as_ref().is_some_and(|s| screen.outputs.contains(s));
            let lifted = dragged_screen == Some(i);
            let fade: f32 = if lifted { 0.4 } else { 1.0 };
            if drop_here {
                // The whole column takes the drop: box, header and chips.
                self.d.bordered(cx, inset(column, -3.0), alpha(accent, 0.14), accent, accent, 0.0, 2.0);
            }
            let base = alpha(fg, if is_main { 0.12 } else { 0.07 });
            let (line, width) = if picked { (alpha(accent, 0.8), 1.5) } else { (alpha(fg, 0.28), 1.0) };
            self.d.bordered(cx, b, alpha(base, base.w * fade), alpha(line, line.w * fade), alpha(line, line.w * fade), 0.0, width);
            // The header: the screen's number, Main, and the mirror's badge.
            let head = at(place.head);
            let number = format!("{}", i + 1);
            let number_w = self.d.measure(cx, true, tok.font.body_small, &number) + 6.0;
            self.d.label(cx, rect(head.pos.x, head.pos.y, number_w, head.size.y), true, tok.font.body_small, alpha(fg, fg.w * fade), HAlign::Left, &number);
            let right = head.pos.x + head.size.x;
            let mut x = head.pos.x + number_w;
            if is_main {
                x = self.draw_badge(cx, x, head, right, "Main", fg, true);
            }
            if mirrored {
                self.draw_badge(cx, x, head, right, &format!("Mirrored \u{d7}{}", screen.outputs.len()), fg, false);
            }
            // The monitors it shows; in a mirror the one it is optimised
            // for gives the size, the others scale it.
            let (screen_w, screen_h) = config.screen_size(&snapshot, i);
            if !place.inside && screen_w > 0 && b.size.y >= 14.0 {
                // The box is the screen: its size, in the middle.
                let size = resolution_label(screen_w, screen_h);
                self.d.label_elided(cx, inset(b, 3.0), false, tok.font.caption, alpha(dim, dim.w * fade), HAlign::Center, &size);
            }
            let optimise = config.optimise_for(&snapshot, i).map(str::to_string);
            for (k, name) in screen.outputs.iter().enumerate() {
                let Some(r) = place.chips.get(k).map(|r| at(*r)) else { break };
                let chosen = optimise.as_deref() == Some(name.as_str());
                let connector = connector_suffix(name);
                let shown = snapshot.output(name).and_then(|o| shown_mode(o, config.mode(name)));
                let rate = match shown {
                    Some((_, _, hz)) => format!("{connector} \u{b7} {}", rate_label(hz)),
                    None => connector,
                };
                let size = match snapshot.output(name) {
                    Some(o) => match output_failure(o) {
                        Some(reason) => (reason.to_string(), true),
                        None if mirrored && !chosen && config.output_size(&snapshot, name) != (screen_w, screen_h) => {
                            (format!("scaled from {}", resolution_label(screen_w, screen_h)), false)
                        }
                        None => (shown.map_or_else(|| "Automatic".to_string(), |(w, h, _)| resolution_label(w, h)), false),
                    },
                    None => ("not connected".to_string(), true),
                };
                self.draw_output_chip(cx, r, &OutputChip {
                    name, rate: &rate, size: &size, optimised: mirrored && chosen,
                    picked: selected.as_deref() == Some(name.as_str()), faded: lifted,
                });
            }
            self.display_drops.push((DropAt::Screen(i), column));
            self.hits.push((Hit::DisplayBox(i), column));
        }
    }

    /// A pill in a box's header at `x`, when it fits before `right`; the x
    /// after it.
    fn draw_badge(&mut self, cx: &mut Cx2d, x: f64, head: Rect, right: f64, text: &str, color: Vec4f, strong: bool) -> f64 {
        let tok = self.tokens;
        let w = self.d.measure(cx, strong, tok.font.caption, text) + 12.0;
        if x + w > right {
            return x;
        }
        let pill = rect(x, head.pos.y + 3.0, w, head.size.y - 5.0);
        self.d.bordered(cx, pill, alpha(color, 0.16), alpha(color, 0.5), alpha(color, 0.5), 0.0, 1.0);
        self.d.label(cx, pill, strong, tok.font.caption, color, HAlign::Center, text);
        x + w + 4.0
    }

    /// A monitor's chip, three lines: its name; its connector and rate;
    /// its resolution, or "scaled from W×H" in a mirror sized for another
    /// (or what failed, `size.1`). The monitor a mirror is optimised for
    /// carries an "optimised" tag on its first line. A press picks it or
    /// starts dragging it.
    fn draw_output_chip(&mut self, cx: &mut Cx2d, r: Rect, chip: &OutputChip<'_>) {
        let OutputChip { name, rate, size, optimised, picked, faded } = *chip;
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let index = self.display_chips.len();
        self.display_chips.push(name.to_string());
        let hit = Hit::DisplayChip(index);
        let lifted = faded || self.display_drag.as_ref().is_some_and(|d| d.active && d.item == DragItem::Output(name.to_string()));
        let state = if picked { CtrlState::Selected } else if self.hot == Some(hit) { CtrlState::Hover } else { CtrlState::Normal };
        if lifted {
            // Its place while it is carried: an outline.
            self.d.bordered(cx, r, alpha(fg, 0.02), alpha(fg, 0.25), alpha(fg, 0.25), 0.0, 1.0);
        } else {
            self.d.control(cx, r, &tok.controls, state);
        }
        let fade: f32 = if lifted { 0.35 } else { 1.0 };
        let mut title_right = r.pos.x + r.size.x - 6.0;
        if optimised {
            // The screen takes this monitor's size and rate.
            let text = "optimised";
            let w = self.d.measure(cx, false, tok.font.caption, text) + 10.0;
            let tag = rect(title_right - w, r.pos.y + 4.0, w, 14.0);
            let color = alpha(fg, fg.w * fade);
            self.d.bordered(cx, tag, alpha(fg, 0.10 * fade), alpha(fg, 0.40 * fade), alpha(fg, 0.40 * fade), 0.0, 1.0);
            self.d.label(cx, tag, false, tok.font.caption, color, HAlign::Center, text);
            title_right = tag.pos.x - 4.0;
        }
        let x = r.pos.x + 7.0;
        let title = self.display_snapshot.output(name).map_or_else(|| connector_suffix(name), monitor_label);
        self.d.label_elided(cx, rect(x, r.pos.y + 3.0, (title_right - x).max(0.0), 15.0), true, tok.font.body_small, alpha(fg, fg.w * fade), HAlign::Left, &title);
        let text_w = (r.size.x - 14.0).max(0.0);
        self.d.label_elided(cx, rect(x, r.pos.y + 18.0, text_w, 13.0), false, tok.font.caption, alpha(dim, dim.w * fade), HAlign::Left, rate);
        let color = if size.1 { tok.bar.active } else { dim };
        self.d.label_elided(cx, rect(x, r.pos.y + 31.0, text_w, 13.0), false, tok.font.caption, alpha(color, color.w * fade), HAlign::Left, &size.0);
        self.hits.push((hit, r));
    }

    /// The Off strip (the monitors not driven; a drop target), then the
    /// picked monitor's details: its mode, its screen (main, optimise for)
    /// and where it can go (Move to: the chip's menu).
    fn draw_details(&mut self, cx: &mut Cx2d, rest: Rect, config: &DisplayConfig, target: Option<DropAt>) -> Rect {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let accent = tok.bar.active;
        let caption_w = 84.0;
        let snapshot = self.display_snapshot.clone();
        let carrying = self.display_drag.as_ref().is_some_and(|d| d.active && matches!(d.item, DragItem::Output(_)));
        let selected = self.display_selected(config);

        // Off.
        let (strip, mut rest) = cut_top(rest, OFF_ROW_H);
        let strip = rect(strip.pos.x, strip.pos.y + 4.0, strip.size.x, strip.size.y - 4.0);
        let hot = target == Some(DropAt::Off);
        if carrying {
            let (fill, line) = if hot { (alpha(accent, 0.16), accent) } else { (alpha(fg, 0.03), alpha(fg, 0.18)) };
            self.d.bordered(cx, strip, fill, line, line, 0.0, if hot { 2.0 } else { 1.0 });
        }
        self.d.label(cx, rect(strip.pos.x + 6.0, strip.pos.y, 36.0, strip.size.y), false, tok.font.body_small, dim, HAlign::Left, "Off");
        let off = config.arrangement.off.clone();
        let chips_x = strip.pos.x + 44.0;
        let chips_w = (strip.pos.x + strip.size.x - 4.0 - chips_x).max(0.0);
        if off.is_empty() {
            self.d.label_elided(cx, rect(chips_x, strip.pos.y, chips_w, strip.size.y), false, tok.font.caption,
                if hot { accent } else { dim }, HAlign::Left, "Drag a monitor here to turn it off");
        } else {
            let w = (chips_w / off.len().max(2) as f64 - 4.0).max(40.0);
            for (k, name) in off.iter().enumerate() {
                let r = rect(chips_x + k as f64 * (w + 4.0), strip.pos.y + 2.0, w, strip.size.y - 4.0);
                self.draw_off_chip(cx, r, name, selected.as_deref() == Some(name.as_str()));
            }
        }
        self.display_drops.push((DropAt::Off, strip));
        let (sep, next) = cut_top(rest, SEP_H);
        rest = next;
        self.d.separator(cx, sep, fg, 0.12);

        let Some(name) = selected else { return rest };
        let Some(output) = snapshot.output(&name).cloned() else { return rest };
        let place = config.place_of(&name);

        // The monitor: its name, connector, GPU and where it is.
        let (line, next) = cut_top(rest, SCREEN_LINE_H);
        rest = next;
        self.d.icon_centered(cx, Ico::Monitor, rect(line.pos.x, line.pos.y, 22.0, line.size.y), 14.0, fg);
        let title = monitor_label(&output);
        let name_x = line.pos.x + 28.0;
        let name_w = self.d.measure(cx, true, tok.font.body_small, &title).min(line.size.x * 0.5);
        self.d.label_elided(cx, rect(name_x, line.pos.y, name_w, line.size.y), true, tok.font.body_small, fg, HAlign::Left, &title);
        let mut detail = vec![connector_suffix(&name)];
        if screens_span_gpus(&snapshot) {
            detail.push(self.screen_gpu_label(&output));
        }
        detail.push(match place {
            Some(Place::Screen(i)) if config.arrangement.screens[i].outputs.len() > 1 => {
                format!("screen {}, mirrored \u{d7}{}", i + 1, config.arrangement.screens[i].outputs.len())
            }
            Some(Place::Screen(i)) => format!("screen {}", i + 1),
            _ => "off".to_string(),
        });
        let detail_x = name_x + name_w + 8.0;
        self.d.label_elided(cx, rect(detail_x, line.pos.y, (line.pos.x + line.size.x - detail_x).max(0.0), line.size.y),
            false, tok.font.caption, dim, HAlign::Left, &detail.join(" \u{b7} "));

        // Its mode: today's resolution and refresh pickers.
        let (row, next) = cut_top(rest, CONTROL_H);
        rest = next;
        self.d.label(cx, rect(row.pos.x, row.pos.y, caption_w, row.size.y), false, tok.font.body_small, dim, HAlign::Left, "Mode");
        let inner = rect(row.pos.x + caption_w, row.pos.y + 2.0, (row.size.x - caption_w).max(0.0), row.size.y - 4.0);
        let mode = config.mode(&name).map(str::to_string);
        let shown = shown_mode(&output, mode.as_deref());
        let enabled = !output.modes.is_empty();
        let res_w = (inner.size.x * 0.6).floor();
        let res = rect(inner.pos.x, inner.pos.y, res_w - 4.0, inner.size.y);
        let rate = rect(inner.pos.x + res_w, inner.pos.y, inner.size.x - res_w, inner.size.y);
        let native = native_resolution(&output);
        let (res_text, rate_text) = match shown {
            Some((w, h, hz)) => (
                if native == Some((w, h)) { format!("{} \u{b7} native", resolution_label(w, h)) } else { resolution_label(w, h) },
                rate_label(hz),
            ),
            None => (mode.clone().unwrap_or_else(|| "Automatic".into()), String::new()),
        };
        let open = |kind| self.chooser.as_ref().is_some_and(|(n, k)| *n == name && *k == kind);
        let (res_open, rate_open) = (open(Chooser::Resolution), open(Chooser::Rate));
        self.picker_face(cx, res, enabled.then_some(Hit::OutputResolution), res_open, &res_text);
        self.picker_face(cx, rate, (enabled && shown.is_some()).then_some(Hit::OutputRate), rate_open, &rate_text);
        if res_open || rate_open {
            rest = self.draw_chooser(cx, rest, &output, shown, if res_open { Chooser::Resolution } else { Chooser::Rate });
        }

        // Its screen: main, and in a mirror the monitor it is optimised for.
        if let Some(Place::Screen(i)) = place {
            let (row, next) = cut_top(rest, CONTROL_H);
            rest = next;
            self.d.label(cx, rect(row.pos.x, row.pos.y, caption_w, row.size.y), false, tok.font.body_small, dim, HAlign::Left, &format!("Screen {}", i + 1));
            let button = rect(row.pos.x + caption_w, row.pos.y + 2.0, 128.0, row.size.y - 4.0);
            if config.main_screen() == Some(i) {
                self.small_button(cx, button, None, true, Some(Ico::Check), "Main screen");
            } else {
                self.small_button(cx, button, Some(Hit::DisplayMakeMain), false, None, "Make main");
            }
            let (w, h) = config.screen_size(&snapshot, i);
            let size_x = button.pos.x + button.size.x + 8.0;
            self.d.label_elided(cx, rect(size_x, row.pos.y, (row.pos.x + row.size.x - size_x).max(0.0), row.size.y),
                false, tok.font.caption, dim, HAlign::Left, &format!("desktop {}", resolution_label(w, h)));
            let screen = config.arrangement.screens[i].clone();
            if screen.outputs.len() > 1 {
                let (row, next) = cut_top(rest, CONTROL_H);
                rest = next;
                self.d.label(cx, rect(row.pos.x, row.pos.y, caption_w, row.size.y), false, tok.font.body_small, dim, HAlign::Left, "Optimise for");
                let chosen = config.optimise_for(&snapshot, i).map(str::to_string);
                let x0 = row.pos.x + caption_w;
                let gap = 4.0;
                let count = screen.outputs.len() as f64;
                let w = ((row.pos.x + row.size.x - x0) - gap * (count - 1.0)) / count;
                for (k, o) in screen.outputs.iter().enumerate() {
                    let r = rect(x0 + k as f64 * (w + gap), row.pos.y + 2.0, w, row.size.y - 4.0);
                    let label = snapshot.output(o).map_or_else(|| connector_suffix(o), monitor_label);
                    let current = chosen.as_deref() == Some(o.as_str());
                    let index = self.display_optimise.len();
                    self.display_optimise.push(o.clone());
                    self.small_button(cx, r, (!current).then_some(Hit::DisplayOptimise(index)), current, None, &label);
                }
            }
        }

        // Move to: the chip's menu.
        let (row, next) = cut_top(rest, CONTROL_H);
        rest = next;
        self.d.label(cx, rect(row.pos.x, row.pos.y, caption_w, row.size.y), false, tok.font.body_small, dim, HAlign::Left, "Move to");
        let screens = config.arrangement.screens.len();
        let mut items: Vec<(String, Place)> = (0..screens).map(|j| (format!("Screen {}", j + 1), Place::Screen(j))).collect();
        items.push(("New screen".into(), Place::NewScreen(screens)));
        items.push(("Off".into(), Place::Off));
        let x0 = row.pos.x + caption_w;
        let gap = 4.0;
        let count = items.len() as f64;
        let w = ((row.pos.x + row.size.x - x0) - gap * (count - 1.0)) / count;
        for (k, (label, to)) in items.into_iter().enumerate() {
            let r = rect(x0 + k as f64 * (w + gap), row.pos.y + 2.0, w, row.size.y - 4.0);
            let current = place == Some(to);
            let can = !current && config.clone().move_output(&name, to);
            let index = self.display_moves.len();
            self.display_moves.push(to);
            self.small_button(cx, r, can.then_some(Hit::DisplayMoveTo(index)), current, None, &label);
        }
        rest
    }

    /// An off monitor's chip, one line; dragged onto a screen to turn it on.
    fn draw_off_chip(&mut self, cx: &mut Cx2d, r: Rect, name: &str, picked: bool) {
        let tok = self.tokens;
        let dim = darker(tok.popups.text, 1.4);
        let index = self.display_chips.len();
        self.display_chips.push(name.to_string());
        let hit = Hit::DisplayChip(index);
        let lifted = self.display_drag.as_ref().is_some_and(|d| d.active && d.item == DragItem::Output(name.to_string()));
        let state = if picked { CtrlState::Selected } else if self.hot == Some(hit) { CtrlState::Hover } else { CtrlState::Normal };
        self.d.control(cx, r, &tok.controls, state);
        let title = self.display_snapshot.output(name).map_or_else(|| connector_suffix(name), monitor_label);
        let text = format!("{title} \u{b7} {}", connector_suffix(name));
        self.d.label_elided(cx, rect(r.pos.x + 6.0, r.pos.y, (r.size.x - 12.0).max(0.0), r.size.y), false, tok.font.caption,
            alpha(dim, dim.w * if lifted { 0.4 } else { 1.0 }), HAlign::Left, &text);
        self.hits.push((hit, r));
    }

    /// The trial's Revert / Keep with the countdown, the draft's Discard /
    /// Apply, then the note.
    fn draw_apply(&mut self, cx: &mut Cx2d, rest: Rect) -> Rect {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let accent = tok.bar.active;
        let mut rest = rest;
        if let Some(trial) = self.display_trials.trial.clone() {
            let (text_r, left, right, next) = button_row(rest);
            rest = next;
            let confirming = matches!(trial.phase, Phase::Confirming { .. });
            let text = if confirming {
                let left = self.display_trials.seconds_left(self.display_now).unwrap_or(0);
                format!("Keep these settings? Reverting in {left} s")
            } else {
                "Applying\u{2026}".to_string()
            };
            self.d.label_elided(cx, text_r, confirming, tok.font.body_small, if confirming { accent } else { dim }, HAlign::Left, &text);
            self.small_button(cx, left, Some(Hit::TrialRevert), false, None, "Revert");
            self.small_button(cx, right, confirming.then_some(Hit::TrialKeep), false, Some(Ico::Check), "Keep");
        }
        if let Some(draft) = self.display_drafted().cloned() {
            let (text_r, left, right, next) = button_row(rest);
            rest = next;
            let text = if draft.config.modesets(&draft.base) {
                "Not applied yet \u{b7} you'll be asked to keep it"
            } else {
                "Not applied yet"
            };
            self.d.label_elided(cx, text_r, false, tok.font.body_small, dim, HAlign::Left, text);
            self.small_button(cx, left, Some(Hit::DisplayDiscard), false, None, "Discard");
            self.small_button(cx, right, Some(Hit::DisplayApply), true, Some(Ico::Check), "Apply");
        }
        if let Some(note) = self.display_trials.note.clone() {
            let lines = if note.error { 2 } else { 1 };
            let (area, next) = cut_top(rest, NOTE_LINE_H * lines as f64);
            rest = next;
            let color = if note.error { accent } else { dim };
            let wrapped = self.d.wrap(cx, false, tok.font.caption, &note.text, area.size.x, lines);
            for (i, line) in wrapped.iter().enumerate() {
                let r = rect(area.pos.x, area.pos.y + i as f64 * NOTE_LINE_H, area.size.x, NOTE_LINE_H);
                self.d.label(cx, r, false, tok.font.caption, color, HAlign::Left, line);
            }
        }
        rest
    }

    /// What is carried, under the pointer: a monitor's chip or a screen's
    /// box, kept inside the panel so its name stays readable. Drawn last,
    /// over the rest; it takes no hits.
    fn draw_drag_ghost(&mut self, cx: &mut Cx2d, body: Rect) {
        let Some(drag) = self.display_drag.clone().filter(|d| d.active) else { return };
        let tok = self.tokens;
        let fg = tok.popups.text;
        let accent = tok.bar.active;
        let (w, h) = (drag.origin.size.x.min(body.size.x), drag.origin.size.y.min(OUTPUT_CHIP_H));
        let x = (drag.at.x - drag.grab.x).clamp(body.pos.x, body.pos.x + body.size.x - w);
        let y = (drag.at.y - drag.grab.y.min(h)).clamp(body.pos.y, (body.pos.y + body.size.y - h).max(body.pos.y));
        let r = rect(x, y, w, h);
        // Draw calls of its own, after everything else: the box and its text
        // would otherwise join the earlier ones and the chips' text would
        // show through it.
        self.d.chrome.new_draw_call(cx);
        self.d.text_bold.new_draw_call(cx);
        self.d.bordered(cx, r, tok.popups.bg(), accent, accent, 0.0, 1.5);
        let text = match &drag.item {
            DragItem::Output(name) => self.display_snapshot.output(name).map_or_else(|| connector_suffix(name), monitor_label),
            DragItem::Screen(i) => format!("Screen {}", i + 1),
        };
        self.d.label_elided(cx, rect(r.pos.x + 7.0, r.pos.y, (r.size.x - 14.0).max(0.0), r.size.y), true, tok.font.body_small, fg, HAlign::Left, &text);
    }

    // ---- pointer -------------------------------------------------------

    /// A press on a monitor's chip or a screen's box: a click, or a drag
    /// once the pointer moves.
    pub(super) fn display_press(&mut self, hit: Hit, p: Vec2d) {
        let Some(origin) = self.hits.iter().find(|(h, _)| *h == hit).map(|(_, r)| *r) else { return };
        let item = match hit {
            Hit::DisplayChip(i) => match self.display_chips.get(i) {
                Some(name) => DragItem::Output(name.clone()),
                None => return,
            },
            Hit::DisplayBox(i) => DragItem::Screen(i),
            _ => return,
        };
        self.display_drag = Some(DisplayDrag { item, start: p, at: p, grab: dvec2(p.x - origin.pos.x, p.y - origin.pos.y), origin, active: false });
    }

    /// The press follows the pointer; past a few points it is a drag.
    pub(super) fn display_drag_to(&mut self, cx: &mut Cx, p: Vec2d) {
        let Some(drag) = self.display_drag.as_mut() else { return };
        drag.at = p;
        if !drag.active && (p.x - drag.start.x).hypot(p.y - drag.start.y) > DRAG_START {
            drag.active = true;
            self.close_display_lists();
        }
        if self.display_drag.as_ref().is_some_and(|d| d.active) {
            self.redraw(cx);
        }
    }

    /// The press ends: a click picks the monitor (a box's, unless one of
    /// it is picked already); a drag drops onto what is under it.
    pub(super) fn display_release(&mut self, cx: &mut Cx, p: Vec2d) {
        let Some(drag) = self.display_drag.clone() else { return };
        let config = self.display_config();
        let on = self.display_drop_at(&config, p);
        self.display_drag = None;
        if !drag.active {
            self.close_display_lists();
            match &drag.item {
                DragItem::Output(name) => self.display_selected = Some(name.clone()),
                DragItem::Screen(i) => {
                    let on_it = self.display_selected(&config).is_some_and(|s| config.place_of(&s) == Some(Place::Screen(*i)));
                    if !on_it {
                        self.display_selected = config.arrangement.screens.get(*i).and_then(|s| s.outputs.first().cloned());
                    }
                }
            }
        } else if let Some(changed) = on.and_then(|on| Self::display_drop_result(&config, &drag.item, on)) {
            self.set_display_draft(changed);
            if let DragItem::Output(name) = &drag.item {
                self.display_selected = Some(name.clone());
            }
        }
        self.redraw(cx);
    }

    /// A picker's face: the value and a chevron; no hit when disabled.
    fn picker_face(&mut self, cx: &mut Cx2d, r: Rect, hit: Option<Hit>, open: bool, value: &str) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let state = match hit {
            None => CtrlState::Disabled,
            Some(_) if open => CtrlState::Selected,
            Some(hit) if self.hot == Some(hit) => CtrlState::Hover,
            Some(_) => CtrlState::Normal,
        };
        self.d.control(cx, r, &tok.controls, state);
        let chevron_w = if hit.is_some() { 18.0 } else { 4.0 };
        self.d.label_elided(cx, rect(r.pos.x + 7.0, r.pos.y, (r.size.x - 7.0 - chevron_w).max(0.0), r.size.y),
            false, tok.font.body_small, if hit.is_some() { fg } else { dim }, HAlign::Left, value);
        if let Some(hit) = hit {
            self.d.icon_centered(cx, Ico::ChevronDown,
                rect(r.pos.x + r.size.x - 18.0, r.pos.y, 16.0, r.size.y), 10.0, fg);
            self.hits.push((hit, r));
        }
    }

    /// The unfolded picker of `output` as a grid of chips: its resolutions
    /// (native marked), or the rates of the shown resolution. The change
    /// behind each chip is bound here, so a click tries what was drawn.
    fn draw_chooser(&mut self, cx: &mut Cx2d, rest: Rect, output: &LinuxDisplayOutput, shown: Option<(u32, u32, f64)>, kind: Chooser) -> Rect {
        let (w_now, h_now, hz_now) = shown.unwrap_or_default();
        // (label, mode, current, native)
        let items: Vec<(String, Option<String>, bool, bool)> = match kind {
            Chooser::Resolution => resolutions(&output.modes, native_resolution(output))
                .into_iter()
                .filter_map(|r| {
                    let hz = rate_after_resolution(&output.modes, r.width, r.height, hz_now)?;
                    let current = r.width == w_now && r.height == h_now;
                    Some((resolution_label(r.width, r.height), mode_for(&output.modes, native_resolution(output), r.width, r.height, hz), current, r.native))
                })
                .collect(),
            Chooser::Rate => rates_for(&output.modes, w_now, h_now)
                .into_iter()
                .map(|hz| (rate_label(hz), mode_for(&output.modes, native_resolution(output), w_now, h_now, hz), hz.round() == hz_now.round(), false))
                .collect(),
        };
        let per_row = if kind == Chooser::Resolution { RESOLUTION_CHIPS } else { RATE_CHIPS };
        let rows = items.len().div_ceil(per_row);
        let (area, rest) = cut_top(rest, rows as f64 * CHIP_H);
        let x0 = area.pos.x + 84.0;
        let gap = 4.0;
        let w = ((area.pos.x + area.size.x - x0) - gap * (per_row - 1) as f64) / per_row as f64;
        for (i, (label, mode, current, native)) in items.into_iter().enumerate() {
            let r = rect(x0 + (i % per_row) as f64 * (w + gap), area.pos.y + (i / per_row) as f64 * CHIP_H + 2.0, w, CHIP_H - 4.0);
            let index = self.chooser_changes.len();
            self.chooser_changes.push((output.name.clone(), mode));
            self.small_button(cx, r, (!current).then_some(Hit::ChooserItem(index)), current, None, &label);
            if native {
                // The screen's own resolution: a dot in the chip's corner.
                let tok = self.tokens;
                self.d.icon_centered(cx, Ico::Dot, rect(r.pos.x + r.size.x - 10.0, r.pos.y, 8.0, r.size.y), 6.0, tok.bar.active);
            }
        }
        rest
    }

    /// `Desktop  [Panel | Virtual]`, and while Virtual `Size  [Follow Desk
    /// Link viewer · 3840×2160 ▾]` (unfolded: Follow and the presets as
    /// chips), the Custom steppers when the size is fixed, and `Panels
    /// [Mirror (scaled) | Off]`.
    fn draw_desktop(&mut self, cx: &mut Cx2d, area: Rect) {
        let tok = self.tokens;
        let dim = darker(tok.popups.text, 1.4);
        let caption_w = 84.0;
        let virtual_ = self.virtual_desktop();
        let (row, mut rest) = cut_top(area, CONTROL_H);
        self.d.label(cx, rect(row.pos.x, row.pos.y, caption_w, row.size.y), false, tok.font.body_small, dim, HAlign::Left, "Desktop");
        let inner = rect(row.pos.x + caption_w, row.pos.y + 2.0, row.size.x - caption_w, row.size.y - 4.0);
        let half = (inner.size.x / 2.0).floor();
        let panel = rect(inner.pos.x, inner.pos.y, half, inner.size.y);
        let virt = rect(inner.pos.x + half, inner.pos.y, inner.size.x - half, inner.size.y);
        self.small_button(cx, panel, virtual_.then_some(Hit::Desktop(false)), !virtual_, None, "Panel");
        self.small_button(cx, virt, (!virtual_).then_some(Hit::Desktop(true)), virtual_, None, "Virtual");
        self.virtual_size_targets.clear();
        let Some(saved) = self.virtual_saved else {
            self.virtual_size_open = false;
            return;
        };

        // Size.
        let (row, next) = cut_top(rest, CONTROL_H);
        rest = next;
        self.d.label(cx, rect(row.pos.x, row.pos.y, caption_w, row.size.y), false, tok.font.body_small, dim, HAlign::Left, "Size");
        let picker = rect(row.pos.x + caption_w, row.pos.y + 2.0, row.size.x - caption_w, row.size.y - 4.0);
        let label = virtual_size_label(&saved, self.display_snapshot.virtual_display.as_ref());
        let open = self.virtual_size_open;
        self.picker_face(cx, picker, Some(Hit::VirtualSizePicker), open, &label);
        if open {
            let choices = virtual_size_choices(&saved);
            let (chips, next) = cut_top(rest, choices.len().div_ceil(SIZE_CHIPS) as f64 * CHIP_H);
            rest = next;
            let gap = 4.0;
            let x0 = chips.pos.x + caption_w;
            let w = ((chips.pos.x + chips.size.x - x0) - gap * (SIZE_CHIPS - 1) as f64) / SIZE_CHIPS as f64;
            for (i, (label, size, current)) in choices.into_iter().enumerate() {
                let r = rect(x0 + (i % SIZE_CHIPS) as f64 * (w + gap), chips.pos.y + (i / SIZE_CHIPS) as f64 * CHIP_H + 2.0, w, CHIP_H - 4.0);
                self.virtual_size_targets.push(size);
                self.small_button(cx, r, (!current).then_some(Hit::VirtualSizeItem(i)), current, None, &label);
            }
        }

        // Custom: width, height and refresh steppers, while the size is fixed.
        if !saved.follow {
            let (row, next) = cut_top(rest, CONTROL_H);
            rest = next;
            self.d.label(cx, rect(row.pos.x, row.pos.y, caption_w, row.size.y), false, tok.font.body_small, dim, HAlign::Left, "Custom");
            let inner = rect(row.pos.x + caption_w, row.pos.y + 2.0, row.size.x - caption_w, row.size.y - 4.0);
            let gap = 6.0;
            let group_w = (inner.size.x - gap * 2.0) / 3.0;
            let fields = [
                (CustomField::Width, format!("W {}", saved.mode.width)),
                (CustomField::Height, format!("H {}", saved.mode.height)),
                (CustomField::Refresh, rate_label(saved.mode.refresh_hz())),
            ];
            let button_w = 22.0;
            for (i, (field, text)) in fields.into_iter().enumerate() {
                let x = inner.pos.x + i as f64 * (group_w + gap);
                let minus = rect(x, inner.pos.y, button_w, inner.size.y);
                let plus = rect(x + group_w - button_w, inner.pos.y, button_w, inner.size.y);
                self.small_button(cx, minus, Some(Hit::VirtualCustom(field, -1)), false, Some(Ico::ChevronLeft), "");
                self.small_button(cx, plus, Some(Hit::VirtualCustom(field, 1)), false, Some(Ico::ChevronRight), "");
                let text_r = rect(x + button_w, inner.pos.y, group_w - 2.0 * button_w, inner.size.y);
                self.d.label_elided(cx, text_r, false, tok.font.body_small, tok.popups.text, HAlign::Center, &text);
            }
        }

        // Panels.
        let (row, _) = cut_top(rest, CONTROL_H);
        self.d.label(cx, rect(row.pos.x, row.pos.y, caption_w, row.size.y), false, tok.font.body_small, dim, HAlign::Left, "Panels");
        let inner = rect(row.pos.x + caption_w, row.pos.y + 2.0, row.size.x - caption_w, row.size.y - 4.0);
        let half = (inner.size.x / 2.0).floor();
        let off = saved.mode.panels == VirtualPanels::Off;
        let mirror = rect(inner.pos.x, inner.pos.y, half, inner.size.y);
        let off_r = rect(inner.pos.x + half, inner.pos.y, inner.size.x - half, inner.size.y);
        self.small_button(cx, mirror, off.then_some(Hit::VirtualPanels(false)), !off, None, "Mirror (scaled)");
        self.small_button(cx, off_r, (!off).then_some(Hit::VirtualPanels(true)), off, None, "Off");
    }

    /// `Pace by  [Auto (each screen at its own rate) ▾]`, then, unfolded,
    /// Auto, Slowest screen, Fastest screen and each screen as chips.
    fn draw_pace(&mut self, cx: &mut Cx2d, area: Rect) {
        let tok = self.tokens;
        let dim = darker(tok.popups.text, 1.4);
        let snapshot = self.display_snapshot.clone();
        let (row, rest) = cut_top(area, CONTROL_H);
        let caption_w = 84.0;
        self.d.label(cx, rect(row.pos.x, row.pos.y, caption_w, row.size.y), false, tok.font.body_small, dim, HAlign::Left, "Pace by");
        let picker = rect(row.pos.x + caption_w, row.pos.y + 2.0, row.size.x - caption_w, row.size.y - 4.0);
        let open = self.pace_open;
        self.picker_face(cx, picker, Some(Hit::PacePicker), open, &pacing_label(&snapshot.pacing, &snapshot));
        self.pace_targets.clear();
        if !open {
            return;
        }
        let x0 = rest.pos.x;
        let gap = 4.0;
        let w = (rest.size.x - gap * (PACE_CHIPS - 1) as f64) / PACE_CHIPS as f64;
        for (i, pacing) in pacing_choices(&snapshot).into_iter().enumerate() {
            let r = rect(x0 + (i % PACE_CHIPS) as f64 * (w + gap), rest.pos.y + (i / PACE_CHIPS) as f64 * CHIP_H + 2.0, w, CHIP_H - 4.0);
            let current = pacing == snapshot.pacing;
            // Short on the chips; the picker reads the whole choice.
            let label = match &pacing {
                DisplayPacing::Auto => "Auto".to_string(),
                DisplayPacing::Slowest => "Slowest screen".to_string(),
                DisplayPacing::Fastest => "Fastest screen".to_string(),
                other => pacing_label(other, &snapshot),
            };
            self.pace_targets.push(pacing);
            self.small_button(cx, r, (!current).then_some(Hit::PaceItem(i)), current, None, &label);
        }
    }

    /// `SCALE  2.00× · one for every screen`, then the slider: continuous,
    /// live while dragged, with ticks at the multiples it snaps to.
    fn draw_scale(&mut self, cx: &mut Cx2d, rest: Rect) -> Rect {
        let tok = self.tokens;
        let dim = darker(tok.popups.text, 1.4);
        let now = self.dpi_drag.map_or_else(|| self.dpi_hundredths(), |drag| drag.last);
        let (caption, rest) = cut_top(rest, CAPTION_H);
        self.d.section_header(cx, caption, &tok, tok.popups.text, "SCALE");
        let header_w = self.d.measure(cx, true, tok.font.caption, "SCALE") + 10.0;
        self.d.label_elided(cx, rect(caption.pos.x + header_w, caption.pos.y, (caption.size.x - header_w).max(0.0), caption.size.y),
            false, tok.font.caption, dim, HAlign::Right,
            &format!("{:.2}\u{d7} \u{b7} one for all screens: the desktop is one window", now as f64 / 100.0));
        let (slider, rest) = cut_top(rest, SCALE_SLIDER_H);
        // Ticks under the track at the snap points, where the knob centre
        // lands for them (the same usable track as `slider_value`).
        let track = inset(slider, 6.0);
        let (x0, w) = (track.pos.x + 6.0, (track.size.x - 12.0).max(1.0));
        for snap in SCALE_SNAPS {
            let x = (x0 + dpi_slider_progress(snap) * w).round();
            let on = snap == now;
            let mid = slider.pos.y + slider.size.y * 0.5;
            self.d.solid(cx, rect(x - 0.75, mid + 6.0, 1.5, 5.0), if on { tok.popups.text } else { dim });
        }
        self.slider_row(cx, slider, Hit::DpiScaleSlider, dpi_slider_progress(now), true);
        rest
    }

    /// `[Mouse & touchpad ›]  [GPU  NVIDIA · nvidia ▾]`, the GPU list when
    /// unfolded, its status line and the restart row.
    fn draw_footer(&mut self, cx: &mut Cx2d, rest: Rect) -> Rect {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let direct = self.display_snapshot.direct;
        let (row, mut rest) = cut_top(rest, PICKER_ROW_H);
        let half = (row.size.x / 2.0).floor();
        let pointer = rect(row.pos.x, row.pos.y, half - 3.0, row.size.y);
        let gpu = rect(row.pos.x + half + 3.0, row.pos.y, row.size.x - half - 3.0, row.size.y);
        if direct {
            let hit = Hit::PointerSettings;
            self.d.cursor_surface(cx, pointer, &tok.controls, self.hot == Some(hit), false);
            self.d.label_elided(cx, rect(pointer.pos.x + 5.0, pointer.pos.y, (pointer.size.x - 26.0).max(0.0), pointer.size.y),
                false, tok.font.body_small, fg, HAlign::Left, "Mouse & touchpad");
            self.d.icon_centered(cx, Ico::ChevronRight,
                rect(pointer.pos.x + pointer.size.x - 20.0, pointer.pos.y, 16.0, pointer.size.y), 10.0, fg);
            self.hits.push((hit, pointer));
        }
        if !self.gpu_picker {
            self.gpu_targets.clear();
            self.gpu_overflow = false;
        }
        let hit = Hit::GpuPicker;
        let value = if direct { self.render_on_saved_label() } else { "Managed by the desktop".into() };
        if direct {
            self.d.cursor_surface(cx, gpu, &tok.controls, self.hot == Some(hit), self.gpu_picker);
        }
        let caption_w = 30.0;
        self.d.label(cx, rect(gpu.pos.x + 5.0, gpu.pos.y, caption_w, gpu.size.y), false, tok.font.body_small, dim, HAlign::Left, "GPU");
        let chevron_w = if direct { 20.0 } else { 4.0 };
        self.d.label_elided(cx, rect(gpu.pos.x + 5.0 + caption_w, gpu.pos.y, (gpu.size.x - 5.0 - caption_w - chevron_w).max(0.0), gpu.size.y),
            false, tok.font.body_small, if direct { fg } else { dim }, HAlign::Right, &value);
        if direct {
            self.d.icon_centered(cx, Ico::ChevronDown,
                rect(gpu.pos.x + gpu.size.x - 18.0, gpu.pos.y, 16.0, gpu.size.y), 10.0, fg);
            self.hits.push((hit, gpu));
        }
        if self.gpu_picker {
            rest = self.draw_gpu_choice_list(cx, rest, "Auto", self.gpu_saved_row(), direct);
        }
        if self.gpu_status_shown() {
            let (line, remaining) = cut_top(rest, GPU_STATUS_H);
            rest = remaining;
            let text = format!("Render on, next start: {}", self.render_on_next_label());
            self.d.label_elided(cx, line, false, tok.font.caption, dim, HAlign::Left, &text);
        }
        if self.restart_row_shown() {
            let (row, remaining) = cut_top(rest, PICKER_ROW_H);
            rest = remaining;
            self.draw_restart_row(cx, row);
        } else {
            self.restart_confirm = false;
        }
        rest
    }


    // ---- the GPU the compositor runs on -----------------------------

    fn gpu_identity_available(&self, identity: &str) -> bool {
        self.system.gpus.iter().any(|gpu| gpu.matches(identity))
    }

    /// The render-on list's checked row: the saved choice (Auto, row 0,
    /// when nothing is saved), or none when the saved GPU is not listed.
    fn gpu_saved_row(&self) -> usize {
        match self.render_on_saved.as_deref() {
            None => 0,
            Some(identity) => self
                .system
                .gpus
                .iter()
                .position(|gpu| gpu.matches(identity))
                .map(|i| i + 1)
                .unwrap_or(usize::MAX),
        }
    }

    /// The saved render-on choice as the person reads it.
    fn render_on_saved_label(&self) -> String {
        match self.render_on_saved.as_deref() {
            None => "Auto".into(),
            Some(identity) => self
                .system
                .gpus
                .iter()
                .find(|gpu| gpu.matches(identity))
                .map(|gpu| gpu.label())
                .unwrap_or_else(|| "Auto (saved GPU not present)".into()),
        }
    }

    /// What the next start renders on, as the status line says it: an
    /// external `MAKEPAD_VULKAN_COMPOSITOR_*` wins over the saved choice.
    fn render_on_next_label(&self) -> String {
        if self.system.gpu_env.is_some() {
            "set by the environment".into()
        } else {
            self.render_on_saved_label()
        }
    }

    /// "Restart desktop now" is offered: the saved settings have been read
    /// (an unread file is not "Auto"), the WM drives the screens, the
    /// saved render-on GPU is present. Device changes apply at the next start.
    fn restart_row_shown(&self) -> bool {
        self.display_snapshot.direct
            && self.system.display_settings_loaded
            && saved_gpu_present(self.render_on_saved.as_deref(), &self.system.gpus)
            && self.system.gpu_env.is_none()

    }

    fn gpu_picker_enabled(&self) -> bool {
        self.display_snapshot.direct
    }

    fn gpu_list_open(&self) -> bool {
        self.gpu_picker
    }

    fn draw_pointer_settings(&mut self, cx: &mut Cx2d, body: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let compact = body.size.y < self.pointer_settings_height();
        let (hero, rest) = cut_top(body, if compact { HERO_COMPACT_H } else { HERO_H });
        self.d.panel_hero(cx, hero, &tok, fg, Ico::Monitor, "Mouse & touchpad", "", 0.0);
        let (sep, rest) = cut_top(rest, POINTER_SEP_H); self.d.separator(cx, sep, fg, 0.12);
        let (back, mut rest) = cut_top(rest, PICKER_ROW_H);
        let hit = Hit::PointerSettings;
        self.d.cursor_surface(cx, back, &tok.controls, self.hot == Some(hit), false);
        self.d.icon_centered(cx, Ico::ChevronLeft,
            rect(back.pos.x, back.pos.y, 22.0, back.size.y), 12.0, fg);
        self.d.label(cx, rect(back.pos.x + 22.0, back.pos.y, (back.size.x - 22.0).max(0.0), back.size.y),
            false, tok.font.body, fg, HAlign::Left, "Back to Display");
        self.hits.push((hit, back));
        for touchpad in [false, true] {
            let (sep, next) = cut_top(rest, POINTER_SEP_H); rest = next; self.d.separator(cx, sep, fg, 0.12);
            let (header, next) = cut_top(rest, HEADER_H); rest = next;
            let value = pointer_speed(touchpad);
            self.section_header(
                cx,
                header,
                if touchpad { "TOUCHPAD SPEED" } else { "MOUSE SPEED" },
                &format!("{:.2}×", value as f64 / 100.0),
            );
            let (row, next) = cut_top(rest, SLIDER_H); rest = next;
            self.slider_row(cx, row, Hit::PointerSpeedSlider(touchpad), Self::pointer_slider_progress(value), true);
            if !compact {
                let (ends, next) = cut_top(rest, ENDS_H); rest = next;
                self.d.label(cx, ends, false, tok.font.caption, dim, HAlign::Left, "Slow");
                self.d.label(cx, ends, false, tok.font.caption, dim, HAlign::Right, "Fast");
            }
            // the content follows the wheel or the fingers, as on a Mac (off: traditional)
            let (row, next) = cut_top(rest, PICKER_ROW_H); rest = next;
            self.d.label(cx, row, false, tok.font.body, fg, HAlign::Left, "Natural scrolling");
            self.d.toggle_switch(cx, dvec2(row.pos.x + row.size.x - 48.0, row.pos.y + 4.0), &tok, natural_scroll(touchpad), fg);
            self.hits.push((Hit::NaturalScroll(touchpad), row));
        }
        let (caption, rest) = cut_top(rest, POINTER_CAPTION_H);
        self.d.label_elided(cx, caption, false, tok.font.caption, dim, HAlign::Left,
            "Lift and reposition to keep moving");
        if !self.system_notice.is_empty() {
            let notice = self.system_notice.clone();
            let (row, _) = cut_top(rest, NOTICE_H);
            self.d.label_elided(cx, row, false, tok.font.caption, fg, HAlign::Left, &notice);
        }
    }

    /// The "N more above / below" caption on an unfolded list's edge rows,
    /// returning the right edge the row's text must stay left of.
    fn list_edge_hint(&mut self, cx: &mut Cx2d, item: Rect, above: usize, below: usize) -> f64 {
        let tok = self.tokens;
        let dim = darker(tok.popups.text, 1.4);
        let mut right = item.pos.x + item.size.x - 26.0;
        if above == 0 && below == 0 {
            return right;
        }
        let hint = if above > 0 && below > 0 { format!("{above} above \u{b7} {below} below") }
            else if above > 0 { format!("{above} more above") }
            else { format!("{below} more below") };
        let hint_w = self.d.measure(cx, false, tok.font.caption, &hint).min(item.size.x * 0.4);
        self.d.label(cx, rect(right - hint_w, item.pos.y, hint_w, item.size.y),
            false, tok.font.caption, dim, HAlign::Right, &hint);
        right -= hint_w + 8.0;
        right
    }

    /// "Restart desktop now · apps will close", then, once pressed,
    /// "Cancel" and "Restart": restarting closes every running app. Once
    /// confirmed and waiting for the display layout save to land
    /// (`restart_waiting`), the row shows "Restarting…" instead, with no
    /// hit: the confirm has already been sent and there is nothing left
    /// to press until it either restarts (the process exits) or the App
    /// abandons it (a save failure or timeout, reported in the notice).
    fn draw_restart_row(&mut self, cx: &mut Cx2d, row: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        if self.restart_waiting {
            self.d.icon_centered(cx, Ico::Refresh, rect(row.pos.x, row.pos.y, 22.0, row.size.y), 12.0, dim);
            self.d.label_elided(cx, rect(row.pos.x + 24.0, row.pos.y, (row.size.x - 28.0).max(0.0), row.size.y),
                false, tok.font.body_small, dim, HAlign::Left, "Restarting\u{2026}");
            return;
        }
        if !self.restart_confirm {
            let hit = Hit::RestartDesktop;
            self.d.cursor_surface(cx, row, &tok.controls, self.hot == Some(hit), false);
            self.d.icon_centered(cx, Ico::Refresh, rect(row.pos.x, row.pos.y, 22.0, row.size.y), 12.0, tok.bar.active);
            self.d.label_elided(cx, rect(row.pos.x + 24.0, row.pos.y, (row.size.x - 28.0).max(0.0), row.size.y),
                false, tok.font.body_small, tok.bar.active, HAlign::Left, "Restart desktop now \u{b7} apps will close");
            self.hits.push((hit, row));
            return;
        }
        let button_w = 72.0;
        let gap = 6.0;
        let inner = rect(row.pos.x, row.pos.y + 2.0, row.size.x, (row.size.y - 4.0).max(0.0));
        let restart = rect(inner.pos.x + inner.size.x - button_w, inner.pos.y, button_w, inner.size.y);
        let cancel = rect(restart.pos.x - gap - button_w, inner.pos.y, button_w, inner.size.y);
        let caption_w = (cancel.pos.x - gap - inner.pos.x - 5.0).max(0.0);
        self.d.label_elided(cx, rect(inner.pos.x + 5.0, row.pos.y, caption_w, row.size.y),
            false, tok.font.body_small, dim, HAlign::Left, "Apps will close");
        self.small_button(cx, cancel, Some(Hit::RestartCancel), false, None, "Cancel");
        self.small_button(cx, restart, Some(Hit::RestartConfirm), true, Some(Ico::Refresh), "Restart");
    }

    /// A small control face: `hit` `None` draws it disabled (no hit),
    /// `selected` in the chosen state; an icon, a label, or both.
    pub(super) fn small_button(&mut self, cx: &mut Cx2d, r: Rect, hit: Option<Hit>, selected: bool, icon: Option<Ico>, text: &str) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let state = match hit {
            _ if selected => CtrlState::Selected,
            None => CtrlState::Disabled,
            Some(hit) if self.hot == Some(hit) => CtrlState::Hover,
            Some(_) => CtrlState::Normal,
        };
        self.d.control(cx, r, &tok.controls, state);
        let color = if hit.is_some() || selected { fg } else { alpha(dim, 0.6) };
        match (icon, text.is_empty()) {
            (Some(icon), true) => self.d.icon_centered(cx, icon, r, 12.0, color),
            (icon, _) => {
                let icon_w = if icon.is_some() { 16.0 } else { 0.0 };
                let text_w = self.d.measure(cx, false, tok.font.body_small, text);
                let total = (icon_w + text_w).min(r.size.x - 6.0);
                let x = r.pos.x + ((r.size.x - total) * 0.5).max(3.0);
                if let Some(icon) = icon {
                    self.d.icon_centered(cx, icon, rect(x, r.pos.y, 14.0, r.size.y), 11.0, color);
                }
                self.d.label_elided(cx, rect(x + icon_w, r.pos.y, (r.pos.x + r.size.x - x - icon_w - 3.0).max(0.0), r.size.y),
                    false, tok.font.body_small, color, HAlign::Left, text);
            }
        }
        if let Some(hit) = hit {
            self.hits.push((hit, r));
        }
    }

    /// Auto / Follow first, then sysfs GPUs. Checkmark is `current_row`.
    /// Saved PCI identities are applied by HAL device selection at the next start.
    fn draw_gpu_choice_list(
        &mut self,
        cx: &mut Cx2d,
        rest: Rect,
        follow_label: &str,
        current_row: usize,
        picking: bool,
    ) -> Rect {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let gpus = self.system.gpus.clone();
        let mut items: Vec<(Option<String>, String, bool)> = Vec::with_capacity(1 + gpus.len());
        items.push((None, follow_label.to_string(), true));
        for gpu in &gpus {
            let selectable = !gpu.pci.is_empty();
            let detail = if gpu.connected.is_empty() {
                "no display connected".to_string()
            } else {
                gpu.connected.join(", ")
            };
            items.push((Some(gpu.identity()), format!("{} \u{b7} {detail}", gpu.label()), selectable));
        }
        self.gpu_targets.clear();
        self.gpu_overflow = false;
        let mut rest = rest;
        let shown = self.gpu_items().min(items.len());
        if shown == 0 {
            return rest;
        }
        let hidden = items.len() - shown;
        self.gpu_overflow = hidden > 0;
        self.gpu_scroll = self.gpu_scroll.min(hidden);
        let first = self.gpu_scroll;
        let after = items.len() - first - shown;
        for (row, (choice, text, selectable)) in items.iter().skip(first).take(shown).enumerate() {
            let (item, remaining) = cut_top(rest, LIST_ITEM_H);
            rest = remaining;
            let hit = Hit::Gpu(row);
            let current = first + row == current_row;
            let live = picking && *selectable;
            if live {
                self.d.cursor_surface(cx, item, &tok.controls, self.hot == Some(hit), current);
            }
            let above = if row == 0 { first } else { 0 };
            let below = if row + 1 == shown { after } else { 0 };
            let right = self.list_edge_hint(cx, item, above, below);
            self.d.label_elided(
                cx,
                rect(item.pos.x + 8.0, item.pos.y, (right - item.pos.x - 8.0).max(0.0), item.size.y),
                current,
                tok.font.body_small,
                if live { fg } else { dim },
                HAlign::Left,
                text,
            );
            if current {
                self.d.icon_centered(
                    cx,
                    Ico::Check,
                    rect(item.pos.x + item.size.x - 22.0, item.pos.y, 18.0, item.size.y),
                    12.0,
                    fg,
                );
            }
            self.gpu_targets.push(choice.clone());
            if live {
                self.hits.push((hit, item));
            }
        }
        rest
    }

    pub(super) fn draw_power(&mut self, cx: &mut Cx2d, body: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let snapshot = self.system.clone();
        let power = &snapshot.power;
        let status = match power.status {
            BatteryStatus::Charging => "Charging", BatteryStatus::Discharging => "On battery",
            BatteryStatus::Full => "Fully charged", BatteryStatus::NotCharging => "Not charging",
            BatteryStatus::Unknown => if power.batteries.is_empty() { "No system battery" } else { "Status unavailable" },
        };
        let (hero, rest) = cut_top(body, 44.0);
        self.d.panel_hero(cx, hero, &tok, fg, Ico::Battery, "Battery", status, 85.0);
        let value = power.percent.map_or("—".into(), |p| format!("{p}%"));
        self.d.label(cx, hero, true, tok.font.display_large, fg, HAlign::Right, &value);
        let (row, rest) = cut_top(rest, 18.0);
        self.d.solid(cx, rect(row.pos.x, row.pos.y + 5.0, row.size.x, 6.0), alpha(fg, 0.15));
        if let Some(percent) = power.percent {
            self.d.solid(cx, rect(row.pos.x, row.pos.y + 5.0, row.size.x * percent as f64 / 100.0, 6.0), fg);
        }
        let (row, mut rest) = cut_top(rest, 26.0);
        self.info_pair(cx, row, "Power source", match power.ac_online { Some(true) => "Power adapter", Some(false) => "Battery", None => "—" });
        for battery in &power.batteries {
            let (row, next) = cut_top(rest, 26.0); rest = next;
            let value = battery.percent.map_or("—".into(), |p| format!("{p}%"));
            self.info_pair(cx, row, battery.model.as_deref().unwrap_or(&battery.name), &value);
        }
    }

    pub(super) fn system_summary(&self) -> String {
        let s = &self.system;
        let mut text = format!("panel={:?} output_volume={:?} input_volume={:?} battery={:?} charging={:?} notice={}",
            self.open, s.audio.default_output().map(|d| d.percent), s.audio.default_input().map(|d| d.percent),
            s.power.percent, s.power.status, self.system_notice);
        for (direction, devices) in [("output", &s.audio.outputs), ("input", &s.audio.inputs)] {
            for device in devices {
                text.push_str(&format!("\n{direction}: {} default={} muted={}", device.label, device.is_default, device.muted));
            }
        }
        // The Display dropdown: the actual window scale, the saved settings,
        // the renderer's snapshot, the configuration shown (the draft, else
        // what runs), the trial and its note, and the diagram as drawn.
        let d = &self.display_snapshot;
        let t = &self.display_trials;
        let running = self.display_running();
        let config = self.display_config();
        let draft = self.display_drafted();
        text.push_str(&format!(
            "\ndpi={:.2} dpi_hundredths={} dpi_saved={:?} display_settings_loaded={} display_direct={} display_outputs={} display_status={} arrangement={} running={} shown={} main={:?} main_screen={:?}",
            self.current_dpi, self.dpi_hundredths(), s.dpi_scale, s.display_settings_loaded, d.direct, d.outputs.len(), self.display_status(),
            d.arrangement.text(), running.arrangement.text(), config.arrangement.text(), config.main, config.main_screen()
        ));
        text.push_str(&format!(
            "\ndraft={} draft_modesets={} trial={:?} seconds_left={:?} restoring={} note={:?}",
            draft.is_some(), draft.is_some_and(|d| d.config.modesets(&d.base)),
            t.trial.as_ref().map(|t| (t.config.arrangement.text(), t.previous.arrangement.text(), t.modeset, &t.phase)),
            t.seconds_left(self.display_now), t.restoring.is_some(), t.note
        ));
        text.push_str(&format!(
            "\npacing={:?} paced_by={:?} pace_label={:?} pace_shown={} pace_open={} pace_targets={:?}",
            d.pacing, d.paced_by, pacing_label(&d.pacing, d), self.pace_shown(), self.pace_open, self.pace_targets
        ));
        text.push_str(&format!(
            "\nselected={:?} drag={:?} screens_span_gpus={} chooser={:?} chooser_changes={:?}",
            self.display_selected(&config), self.display_drag.as_ref().map(|d| (&d.item, d.active)), screens_span_gpus(d), self.chooser, self.chooser_changes
        ));
        for (i, screen) in config.arrangement.screens.iter().enumerate() {
            let (w, h) = config.screen_size(d, i);
            text.push_str(&format!(
                "\nscreen: {} outputs={:?} optimise_for={:?} size={w}x{h} main={}",
                i + 1, screen.outputs, config.optimise_for(d, i), config.main_screen() == Some(i)
            ));
        }
        for name in config.arrangement.screens.iter().flat_map(|s| s.outputs.iter()).chain(config.arrangement.off.iter()) {
            let Some(output) = d.output(name) else { continue };
            let shown = shown_mode(output, config.mode(name));
            text.push_str(&format!(
                "\nmonitor: {name} title={:?} gpu={} place={:?} mode={:?} shown={:?} failure={:?} resolutions={} rates_here={}",
                monitor_label(output), self.screen_gpu_label(output), config.place_of(name), config.mode(name), shown, output_failure(output),
                resolutions(&output.modes, native_resolution(output)).len(), shown.map_or(0, |(w, h, _)| rates_for(&output.modes, w, h).len())
            ));
        }
        text.push_str(&format!(
            "\nrender_on_saved={:?} render_on_next={:?} gpu_env={:?} restart_row={} restart_confirm={} restart_waiting={} gpu_picker={} gpu_scroll={} gpu_rows={:?}",
            self.render_on_saved, self.render_on_next_label(), s.gpu_env, self.restart_row_shown(),
            self.restart_confirm, self.restart_waiting, self.gpu_picker, self.gpu_scroll, self.gpu_targets
        ));
        text.push_str(&format!(
            "\npointer_settings={} pointer_speed_mouse={} pointer_speed_touchpad={} pointer_saved={:?} input_settings_loaded={}",
            self.pointer_settings, pointer_speed(false), pointer_speed(true), s.pointer_speeds, s.input_settings_loaded
        ));
        for gpu in &s.gpus {
            text.push_str(&format!(
                "\ngpu: {} pci={} ids={}:{} driver={} label={} connected={:?} builtin={}",
                gpu.card, gpu.pci, gpu.vendor, gpu.device, gpu.driver, gpu.label(), gpu.connected, gpu.has_builtin()
            ));
        }
        for output in &d.outputs {
            text.push_str(&format!(
                "\ndisplay: {} {}x{} @ {:.2} Hz main={} active={} status={}",
                output.name, output.width, output.height, output.refresh_hz, d.screen_of(&output.name).is_some_and(|s| s.primary), output.active, output.status
            ));
        }
        // Native hit geometry makes the custom dropdown accessible to the
        // WM's existing app-remote protocol without reading any credentials.
        for (hit, r) in &self.hits {
            text.push_str(&format!("\nhit={hit:?} x={} y={} w={} h={}", r.pos.x, r.pos.y, r.size.x, r.size.y));
        }
        text
    }
}


/// Whether the saved render-on choice can take effect at the next start:
/// Auto always can; a GPU only while a listed card matches its identity.
fn saved_gpu_present(saved: Option<&str>, gpus: &[GpuInfo]) -> bool {
    saved.map_or(true, |identity| gpus.iter().any(|gpu| gpu.matches(identity)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_only_for_a_saved_gpu_that_is_present() {
        let gpu = GpuInfo {
            card: "card1".into(),
            pci: "0000:01:00.0".into(),
            vendor: "1234".into(),
            device: "5678".into(),
            driver: "nvidia".into(),
            connected: Vec::new(),
        };
        assert!(saved_gpu_present(None, &[]));
        assert!(saved_gpu_present(Some("0000:01:00.0 1234:5678"), std::slice::from_ref(&gpu)));
        // Moved card or another model at that address: the next start
        // falls back to Auto, so a restart would change nothing.
        assert!(!saved_gpu_present(Some("0000:02:00.0 1234:5678"), std::slice::from_ref(&gpu)));
        assert!(!saved_gpu_present(Some("0000:01:00.0 1234:5679"), &[gpu]));
    }

    #[test]
    fn the_scale_slider_maps_device_pixels_to_hundredths() {
        assert_eq!(dpi_from_slider(0.0), 100);
        assert_eq!(dpi_from_slider(1.0), 300);
        assert_eq!(dpi_from_slider(0.5), 200);
        assert_eq!(dpi_from_slider(-1.0), 100);
        assert!((dpi_slider_progress(200) - 0.5).abs() < 1e-9);
        assert_eq!(dpi_slider_progress(50), 0.0);
        // A track 400 px wide from x = 1000: the same hand position gives the
        // same scale whatever the window's scale did meanwhile.
        assert_eq!(dpi_at(1000.0, 1000.0, 400.0), 100);
        assert_eq!(dpi_at(1200.0, 1000.0, 400.0), 200);
        assert_eq!(dpi_at(1500.0, 1000.0, 400.0), 300);
        assert_eq!(dpi_at(1100.0, 1000.0, 400.0), 150);
        // Near a multiple the slider snaps onto it; between them it is continuous.
        assert_eq!(snap_dpi(197), 200);
        assert_eq!(snap_dpi(204), 200);
        assert_eq!(snap_dpi(205), 205);
        assert_eq!(snap_dpi(175), 175);
        assert_eq!(snap_dpi(128), 125);
        assert_eq!(dpi_at(1195.0, 1000.0, 400.0), 200);
        assert_eq!(dpi_at(1180.0, 1000.0, 400.0), 190);
    }

    /// Every box is its screen at one scale (to the pixels it was rounded
    /// to), and the columns fit the area left to right with the New
    /// screen slot after them.
    fn assert_to_scale(slots: &[ScreenSlot], sizes: &[(f64, f64)], width: f64) {
        let widest = (0..sizes.len()).max_by(|a, b| sizes[*a].0.total_cmp(&sizes[*b].0)).unwrap();
        let scale = slots[widest].screen.size.x / sizes[widest].0;
        for (slot, (w, h)) in slots.iter().zip(sizes) {
            assert!((slot.screen.size.x - w * scale).abs() <= 2.0, "{w}x{h} at {scale}: {:?}", slot.screen);
            assert!((slot.screen.size.y - h * scale).abs() <= 2.0, "{w}x{h} at {scale}: {:?}", slot.screen);
            assert!(slot.screen.pos.x >= slot.column.pos.x && slot.screen.pos.x + slot.screen.size.x <= slot.column.pos.x + slot.column.size.x);
        }
        for pair in slots.windows(2) {
            let gap = pair[1].column.pos.x - (pair[0].column.pos.x + pair[0].column.size.x);
            assert!(gap >= DIAGRAM_GAP, "a drop gap between every two screens: {gap}");
            assert_eq!(pair[0].screen.pos.y, pair[1].screen.pos.y, "top-aligned, as the desktop lays them out");
        }
        assert!(slots[0].column.pos.x >= DIAGRAM_GAP, "a gap before the first screen too");
        let last = slots.last().unwrap().column;
        assert!(last.pos.x + last.size.x + DIAGRAM_GAP + NEW_SLOT_W <= width + 0.5, "the New screen slot fits after the last: {last:?}");
        assert!(slots.iter().all(|s| s.screen.size.y <= BOX_MAX_H));
    }

    #[test]
    fn the_diagram_shows_the_users_screens_to_scale_with_room_for_their_chips() {
        // [B] [A] [C+D]: 1920×1080, 2560×1440, and a 4K mirror of two.
        let width = 558.0;
        let sizes = [(1920.0, 1080.0), (2560.0, 1440.0), (3840.0, 2160.0)];
        let (slots, height) = diagram_layout(&[(sizes[0], 1), (sizes[1], 1), (sizes[2], 2)], width);
        assert_eq!(slots.len(), 3);
        assert_to_scale(&slots, &sizes, width);
        let ratio = slots[2].screen.size.x / slots[0].screen.size.x;
        assert!((ratio - 2.0).abs() < 0.03, "3840 over 1920: {ratio}");
        assert!(slots[2].screen.size.x > 180.0, "the room goes to the screens: {:?}", slots[2].screen);
        // At the menu's width every box holds its chips.
        for slot in &slots {
            assert!(slot.inside, "{slot:?}");
            for chip in &slot.chips {
                assert!(chip.pos.x >= slot.screen.pos.x && chip.pos.x + chip.size.x <= slot.screen.pos.x + slot.screen.size.x);
                assert!(chip.pos.y >= slot.screen.pos.y && chip.pos.y + chip.size.y <= slot.screen.pos.y + slot.screen.size.y);
            }
        }
        assert_eq!(slots[2].chips.len(), 2);
        assert_eq!(height, (slots[2].screen.pos.y + slots[2].screen.size.y).ceil() + DIAGRAM_PAD);
        assert!(slots.iter().all(|s| s.column.size.y == slots[0].column.size.y), "every column the diagram's height");

        // Narrower, B's box is too small for its chip: every chip goes under
        // its box, in a column at least a chip wide.
        let width = 460.0;
        let (slots, height) = diagram_layout(&[(sizes[0], 1), (sizes[1], 1), (sizes[2], 2)], width);
        assert_to_scale(&slots, &sizes, width);
        for slot in &slots {
            assert!(!slot.inside);
            assert!(slot.column.size.x >= CHIP_MIN_W);
            for chip in &slot.chips {
                assert!(chip.pos.y >= slot.screen.pos.y + slot.screen.size.y, "under the box");
                assert_eq!((chip.pos.x, chip.size.x), (slot.column.pos.x, slot.column.size.x));
            }
        }
        let last = slots[2].chips[1];
        assert_eq!(height, (last.pos.y + last.size.y).ceil() + DIAGRAM_PAD);
    }

    #[test]
    fn few_screens_hold_their_chips_inside() {
        let width = 558.0;
        let sizes = [(1920.0, 1080.0), (2560.0, 1440.0)];
        let (slots, _) = diagram_layout(&[(sizes[0], 1), (sizes[1], 2)], width);
        assert_to_scale(&slots, &sizes, width);
        for slot in &slots {
            assert!(slot.inside, "{slot:?}");
            for chip in &slot.chips {
                assert!(chip.pos.x >= slot.screen.pos.x && chip.pos.x + chip.size.x <= slot.screen.pos.x + slot.screen.size.x);
                assert!(chip.pos.y >= slot.screen.pos.y && chip.pos.y + chip.size.y <= slot.screen.pos.y + slot.screen.size.y);
            }
        }
    }

    #[test]
    fn many_or_odd_screens_still_fit_the_menu() {
        let width = 558.0;
        // Six equal screens: equal columns, narrower chips.
        let six = [((1920.0, 1080.0), 1); 6];
        let (slots, _) = diagram_layout(&six, width);
        assert_to_scale(&slots, &[(1920.0, 1080.0); 6], width);
        assert!(slots.iter().all(|s| s.column.size.x == slots[0].column.size.x), "equal screens, equal columns");
        // A small screen next to a huge one keeps its proportion; only its
        // column widens for its chip.
        let sizes = [(800.0, 480.0), (7680.0, 4320.0)];
        let (slots, _) = diagram_layout(&[(sizes[0], 1), (sizes[1], 1)], width);
        assert_to_scale(&slots, &sizes, width);
        assert!(slots[0].screen.size.x < 40.0 && slots[0].column.size.x >= CHIP_MIN_W, "{:?}", slots[0]);
        // A portrait screen of three under the height cap, and one with no
        // mode yet drawn at a common size.
        let (slots, _) = diagram_layout(&[((0.0, 0.0), 1), ((2160.0, 3840.0), 3)], width);
        assert_to_scale(&slots, &[UNSIZED_SCREEN, (2160.0, 3840.0)], width);
        assert_eq!(slots[1].screen.size.y, BOX_MAX_H);
        assert_eq!(slots[1].chips.len(), 3);
        // A narrow panel shrinks the whole diagram, still to scale.
        let narrow = 300.0;
        let sizes = [(1920.0, 1080.0), (2560.0, 1440.0), (3840.0, 2160.0)];
        let (slots, _) = diagram_layout(&[(sizes[0], 1), (sizes[1], 1), (sizes[2], 2)], narrow);
        assert_to_scale(&slots, &sizes, narrow);
    }

    #[test]
    fn a_drop_changes_the_draft_only_where_it_lands() {
        use crate::shell::display_menu::gallery_fixture;
        let (snap, _) = gallery_fixture();
        let running = DisplayConfig::running(&snap);
        let (a, b, c, d) = ("card0-DP-1", "card0-DP-2", "card0-DP-3", "card0-DP-4");
        let drop = |item: &DragItem, on| ShellPanel::display_drop_result(&running, item, on);
        // D off the mirror into a screen of its own at the right: four screens.
        let alone = drop(&DragItem::Output(d.into()), DropAt::Insert(3)).unwrap();
        assert_eq!(alone.arrangement.text(), format!("{b},{a},{c},{d}"));
        // B onto A's screen: A mirrored on B, three screens become two.
        let mirrored = drop(&DragItem::Output(b.into()), DropAt::Screen(1)).unwrap();
        assert_eq!(mirrored.arrangement.text(), format!("{a}+{b},{c}+{d}"));
        assert_eq!(mirrored.main_screen(), Some(0), "the main screen goes with A");
        // C off: the mirror is D alone.
        let off = drop(&DragItem::Output(c.into()), DropAt::Off).unwrap();
        assert_eq!(off.arrangement.text(), format!("{b},{a},{d},off:{c}"));
        assert!(off.modesets(&running), "an output turned off asks to be kept");
        // The mirror screen to the far left; the main screen stays A.
        let moved = drop(&DragItem::Screen(2), DropAt::Insert(0)).unwrap();
        assert_eq!(moved.arrangement.text(), format!("{c}+{d},{b},{a}"));
        assert_eq!((moved.main.as_deref(), moved.main_screen()), (Some(a), Some(2)));
        assert!(!moved.modesets(&running), "a move lays the desktop out again, no modeset");
        // Where they are already: nothing to apply.
        assert!(drop(&DragItem::Output(d.into()), DropAt::Screen(2)).is_none());
        assert!(drop(&DragItem::Output(b.into()), DropAt::Insert(0)).is_none(), "B is alone at the left already");
        assert!(drop(&DragItem::Output(b.into()), DropAt::Insert(1)).is_none());
        assert!(drop(&DragItem::Screen(1), DropAt::Insert(1)).is_none() && drop(&DragItem::Screen(1), DropAt::Insert(2)).is_none());
        assert!(drop(&DragItem::Screen(0), DropAt::Off).is_none(), "a screen is not turned off whole");
    }
}
