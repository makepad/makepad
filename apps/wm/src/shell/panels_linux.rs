//! Linux status dropdowns, drawn from the worker's observed device state.
//!
//! The Display dropdown is about what the screens show, packed tight so
//! the whole of it fits a short screen without scrolling:
//!
//! * The title row carries the arrangement switch: "Extended" (one wide
//!   desktop) or "Mirror" (one desktop on every screen). In Mirror an
//!   "Optimise for" row picks the screen the desktop is sized for; it gets
//!   its native mode, the others show the same picture scaled to fit.
//! * One compact block per screen, left to right: its title ("Built-in ·
//!   eDP-1", "HDMI · 3840×2160") with move and main-screen buttons, then a
//!   resolution picker (the screen's modes, native marked) and a refresh
//!   picker (the rates of the chosen resolution). A picker unfolds as a
//!   grid of chips under its screen.
//! * Extended / Mirror and "Optimise for" switch at once (no output is
//!   reinitialised) and are saved when the renderer takes them; a refusal
//!   is noted under the switch.
//! * Every resolution or refresh change is tried first: "Keep this mode?
//!   Reverting in 15 s" with Keep and Revert under the controls. A change
//!   that fails shows the renderer's error there and the previous one is
//!   sent again; the controls stay usable, so another mode can be tried at
//!   once. Only kept changes are saved (`shell::display_menu`).
//! * Scale: one slider, live while dragged and saved on release, snapping
//!   to the multiples (ticks). It is one value for every screen: the
//!   desktop is one window with one scale, so a per-screen scale is not
//!   something the WM can show (the row says so).
//! * The last row opens Mouse & touchpad and the Render on GPU list.
use super::*;
use super::super::ui::HAlign;
use crate::linux_controls::ControlAction;
use crate::shell::display_layout::{screen_rows, screens_span_gpus, swap_neighbour};
use crate::shell::display_menu::{
    mode_for, native_resolution, rate_after_resolution, rate_label, rates_for, resolution_label, resolutions, screen_title,
    shown_mode, pacing_choices, pacing_label, Change, Phase, ARRANGEMENT_SLOT,
};
use crate::shell::system_linux::{Availability, BatteryStatus, GpuInfo, SystemCommand, DISPLAY_SCALE_MAX, DISPLAY_SCALE_MIN};
use makepad_widgets::makepad_platform::linux_display::{DisplayArrangement, DisplayPacing, LinuxDisplayOutput};
use makepad_widgets::makepad_platform::linux_input::{
    pointer_speed, POINTER_SPEED_MAX, POINTER_SPEED_MIN,
};

const DEVICE_ROWS: usize = 5;
/// The launch default (`-scale=1.3`) shown until the platform reports the
/// window's actual scale.
const DPI_FALLBACK_HUNDREDTHS: u32 = 130;
// The Display dropdown's metrics, in logical points.
/// The title row with the Extended | Mirror switch.
const TITLE_H: f64 = 30.0;
/// A separator between sections.
const SEP_H: f64 = 9.0;
/// A screen's title line.
const SCREEN_LINE_H: f64 = 22.0;
/// A row of controls (pickers, chips, Keep/Revert).
const CONTROL_H: f64 = 28.0;
/// Between two screen blocks.
const SCREEN_GAP: f64 = 6.0;
/// One line of a note (an error takes two).
const NOTE_LINE_H: f64 = 16.0;
/// A chip of an unfolded picker.
const CHIP_H: f64 = 26.0;
const RESOLUTION_CHIPS: usize = 3;
const RATE_CHIPS: usize = 4;
const PACE_CHIPS: usize = 3;
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
    }

    /// One list at a time: fold every Display list and drop a restart
    /// confirm that is waiting.
    pub(super) fn close_display_lists(&mut self) {
        self.close_gpu_lists();
        self.close_chooser();
        self.restart_confirm = false;
    }

    /// The screen drawn at `row`, while the renderer still lists it.
    fn screen_at(&self, row: usize) -> Option<LinuxDisplayOutput> {
        let name = self.screen_targets.get(row)?;
        self.display_snapshot.outputs.iter().find(|o| o.name == *name).cloned()
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
            Hit::ScreenMove(row, dir) => {
                // The left-to-right order that was drawn, with this screen
                // swapped with its neighbour. Only placed screens move.
                let placed = &self.screen_targets[..self.screen_placed.min(self.screen_targets.len())];
                let order = (row < placed.len()).then(|| swap_neighbour(placed, row, dir)).flatten();
                self.close_display_lists();
                if let Some(order) = order {
                    self.system_action(cx, ControlAction::DisplayOrder(order));
                }
            }
            Hit::ScreenMain(row) => {
                // The name drawn on that row, while the renderer still
                // drives it; the renderer confirms it through `primary`.
                let name = self.screen_at(row).filter(|o| o.active && self.in_desktop(o)).map(|o| o.name);
                self.close_display_lists();
                if let Some(name) = name {
                    self.system_action(cx, ControlAction::DisplaySource(name));
                }
            }
            Hit::ScreenResolution(row) | Hit::ScreenRate(row) => {
                let kind = if matches!(hit, Hit::ScreenRate(_)) { Chooser::Rate } else { Chooser::Resolution };
                let name = self.screen_targets.get(row).cloned();
                let open = name.as_ref().is_some_and(|name| self.chooser.as_ref() != Some(&(name.clone(), kind)));
                self.close_display_lists();
                if open {
                    self.chooser = name.map(|name| (name, kind));
                }
            }
            Hit::ChooserItem(index) => {
                let change = self.chooser_changes.get(index).cloned();
                self.close_display_lists();
                if let Some(change) = change {
                    self.system_action(cx, ControlAction::DisplayTry(change));
                }
            }
            Hit::Arrangement(mirror) => {
                let snapshot = &self.display_snapshot;
                let now = self.display_trials.selected_arrangement(&snapshot.arrangement).clone();
                let wanted = if !mirror {
                    Some(DisplayArrangement::Extended)
                } else if matches!(now, DisplayArrangement::Mirror { .. }) {
                    None
                } else {
                    // Sized for the main screen, else the first one shown.
                    snapshot
                        .outputs
                        .iter()
                        .find(|o| o.primary)
                        .or_else(|| snapshot.outputs.iter().find(|o| o.active))
                        .map(|o| DisplayArrangement::Mirror { optimise_for: o.name.clone() })
                };
                self.close_display_lists();
                if let Some(wanted) = wanted.filter(|wanted| *wanted != now) {
                    self.system_action(cx, ControlAction::DisplayArrangement(wanted));
                }
            }
            Hit::OptimiseFor(row) => {
                let name = self.screen_at(row).map(|o| o.name);
                self.close_display_lists();
                if let Some(optimise_for) = name {
                    self.system_action(cx, ControlAction::DisplayArrangement(DisplayArrangement::Mirror { optimise_for }));
                }
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
        let placed = s.outputs.iter().filter(|o| o.desktop_position.is_some()).count();
        let shown = if placed > 0 { placed } else { s.outputs.iter().filter(|o| o.active).count() };
        match (s.outputs.len(), shown) {
            (0, _) => "No outputs".into(),
            (_, 0) => "No active output".into(),
            (_, 1) => "One screen".into(),
            (_, n) => format!("{n} screens"),
        }
    }

    /// A screen's status and whether it is a problem: nothing for a screen
    /// of the desktop, the renderer's own status for one outside it
    /// (failed, another GPU's screen still to join, …).
    fn screen_status(&self, output: &LinuxDisplayOutput) -> (String, bool) {
        if self.in_desktop(output) {
            (String::new(), false)
        } else if output.status.is_empty() {
            ("Inactive".into(), false)
        } else {
            (output.status.clone(), true)
        }
    }

    /// Part of the desktop: placed, or active while nothing is placed yet.
    /// An active screen left out by the GPU's limits is not.
    /// In Mirror only the screen the desktop is sized for is placed; every
    /// other active screen shows the same desktop, so it counts too.
    fn in_desktop(&self, output: &LinuxDisplayOutput) -> bool {
        let mirror = self.display_snapshot.arrangement != DisplayArrangement::Extended;
        output.desktop_position.is_some() || (output.active && (mirror || !self.any_placed()))
    }

    fn any_placed(&self) -> bool {
        self.display_snapshot.outputs.iter().any(|o| o.desktop_position.is_some())
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

    /// Whether row `row` (of `screen_rows`, `placed` of them in the
    /// desktop) can move left and right.
    fn screen_moves(&self, row: usize, placed: usize) -> (bool, bool) {
        let direct = self.display_snapshot.direct && !self.mirror_shown();
        (direct && row > 0 && row < placed, direct && row + 1 < placed)
    }

    /// The arrangement the switch shows: the one being tried, else the
    /// renderer's.
    fn arrangement_shown(&self) -> DisplayArrangement {
        self.display_trials.selected_arrangement(&self.display_snapshot.arrangement).clone()
    }

    fn mirror_shown(&self) -> bool {
        matches!(self.arrangement_shown(), DisplayArrangement::Mirror { .. })
    }

    /// The mode the controls of `output` show (`None` is automatic).
    fn mode_shown<'a>(&'a self, output: &'a LinuxDisplayOutput) -> Option<&'a str> {
        self.display_trials.selected_mode(&output.name, output.mode_override.as_deref())
    }

    // ---- heights -------------------------------------------------------

    /// What a slot (a screen, or the arrangement) adds under its controls:
    /// the Keep / Revert row while a change of it is tried, then its note
    /// (an error gets two lines).
    fn slot_extra_height(&self, slot: &str) -> f64 {
        let trial = if self.display_trials.trial_for(slot).is_some() { CONTROL_H } else { 0.0 };
        let note = match self.display_trials.note_for(slot) {
            Some(note) if note.error => NOTE_LINE_H * 2.0,
            Some(_) => NOTE_LINE_H,
            None => 0.0,
        };
        trial + note
    }

    /// The chips of `output`'s unfolded picker: how many rows they take.
    fn chooser_rows(&self, output: &LinuxDisplayOutput) -> usize {
        let Some((name, kind)) = self.chooser.as_ref().filter(|(name, _)| *name == output.name) else { return 0 };
        let _ = name;
        match kind {
            Chooser::Resolution => resolutions(&output.modes, native_resolution(output)).len().div_ceil(RESOLUTION_CHIPS),
            Chooser::Rate => {
                let (w, h, _) = shown_mode(output, self.mode_shown(output)).unwrap_or_default();
                rates_for(&output.modes, w, h).len().div_ceil(RATE_CHIPS)
            }
        }
    }

    fn screen_block_height(&self, output: &LinuxDisplayOutput) -> f64 {
        SCREEN_LINE_H + CONTROL_H + self.chooser_rows(output) as f64 * CHIP_H + self.slot_extra_height(&output.name)
    }

    /// Pace by is offered while more than one screen shows.
    fn pace_shown(&self) -> bool {
        self.display_snapshot.direct && self.display_snapshot.outputs.iter().filter(|o| o.active).count() > 1
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

    /// The card height the Display dropdown asks for: exactly what
    /// `draw_monitor` lays out.
    pub(super) fn monitor_height(&self) -> f64 {
        if self.pointer_settings {
            return self.pointer_settings_height();
        }
        let snapshot = &self.display_snapshot;
        let mut h = TITLE_H;
        if snapshot.direct && self.mirror_shown() {
            h += CONTROL_H;
        }
        h += self.slot_extra_height(ARRANGEMENT_SLOT) + SEP_H;
        if snapshot.outputs.is_empty() {
            h += SCREEN_LINE_H;
        }
        for output in &snapshot.outputs {
            h += self.screen_block_height(output) + SCREEN_GAP;
        }
        h += self.pace_height();
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
            + POINTER_SEP_H + HEADER_H + SLIDER_H + ENDS_H
            + POINTER_SEP_H + HEADER_H + SLIDER_H + ENDS_H
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
        let (order, placed) = screen_rows(&snapshot);
        self.screen_targets = order.iter().map(|&i| snapshot.outputs[i].name.clone()).collect();
        self.screen_placed = placed;
        self.chooser_changes.clear();
        if self.chooser.as_ref().is_some_and(|(name, _)| !self.screen_targets.contains(name)) {
            // Its screen went away: nothing left to pick for.
            self.chooser = None;
        }
        let mirror = snapshot.direct && self.mirror_shown();

        let (title, mut rest) = cut_top(body, TITLE_H);
        self.draw_title_row(cx, title);
        if mirror {
            let (row, next) = cut_top(rest, CONTROL_H);
            rest = next;
            self.draw_optimise_for(cx, row, &order);
        }
        // A switch takes no confirm; only a refusal is noted here.
        rest = self.draw_slot_extra(cx, rest, ARRANGEMENT_SLOT, "");
        let (sep, next) = cut_top(rest, SEP_H);
        rest = next;
        self.d.separator(cx, sep, fg, 0.12);

        if order.is_empty() {
            let (row, next) = cut_top(rest, SCREEN_LINE_H);
            rest = next;
            self.unavailable(cx, row, if snapshot.direct { "No connected screen" }
                else { "The desktop owns the screens; the scale below still applies" });
        }
        for (row, &index) in order.iter().enumerate() {
            let output = &snapshot.outputs[index];
            let (block, next) = cut_top(rest, self.screen_block_height(output));
            rest = cut_top(next, SCREEN_GAP).1;
            self.draw_screen_block(cx, block, row, output, placed, mirror);
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
    }

    /// `[▭] Display · 2 screens        [Extended | Mirror]`.
    fn draw_title_row(&mut self, cx: &mut Cx2d, row: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let direct = self.display_snapshot.direct;
        let switch_w = if direct { 168.0 } else { 0.0 };
        self.d.icon_centered(cx, Ico::Monitor, rect(row.pos.x, row.pos.y, 22.0, row.size.y), 16.0, fg);
        let title_x = row.pos.x + 28.0;
        let title_w = self.d.measure(cx, true, tok.font.body, "Display");
        self.d.label(cx, rect(title_x, row.pos.y, title_w, row.size.y), true, tok.font.body, fg, HAlign::Left, "Display");
        let status = self.display_status();
        let status_x = title_x + title_w + 8.0;
        let status_w = (row.pos.x + row.size.x - switch_w - 8.0 - status_x).max(0.0);
        self.d.label_elided(cx, rect(status_x, row.pos.y, status_w, row.size.y), false, tok.font.caption, dim, HAlign::Left, &status);
        if !direct {
            return;
        }
        let mirror = self.mirror_shown();
        let can_mirror = self.display_snapshot.outputs.iter().filter(|o| o.active).count() >= 2 || mirror;
        let inner = rect(row.pos.x + row.size.x - switch_w, row.pos.y + 2.0, switch_w, row.size.y - 4.0);
        let half = (inner.size.x / 2.0).floor();
        let extended = rect(inner.pos.x, inner.pos.y, half, inner.size.y);
        let mirrored = rect(inner.pos.x + half, inner.pos.y, inner.size.x - half, inner.size.y);
        self.small_button(cx, extended, (mirror).then_some(Hit::Arrangement(false)), !mirror, None, "Extended");
        self.small_button(cx, mirrored, (!mirror && can_mirror).then_some(Hit::Arrangement(true)), mirror, None, "Mirror");
    }

    /// `Optimise for  [Built-in · eDP-1] [HDMI · 3840×2160]`.
    fn draw_optimise_for(&mut self, cx: &mut Cx2d, row: Rect, order: &[usize]) {
        let tok = self.tokens;
        let dim = darker(tok.popups.text, 1.4);
        let caption_w = 84.0;
        self.d.label(cx, rect(row.pos.x, row.pos.y, caption_w, row.size.y), false, tok.font.body_small, dim, HAlign::Left, "Optimise for");
        let target = match self.arrangement_shown() {
            DisplayArrangement::Mirror { optimise_for } => Some(optimise_for),
            DisplayArrangement::Extended => None,
        };
        let screens: Vec<(usize, LinuxDisplayOutput)> = order
            .iter()
            .enumerate()
            .map(|(row, &i)| (row, self.display_snapshot.outputs[i].clone()))
            .filter(|(_, o)| !o.modes.is_empty())
            .collect();
        if screens.is_empty() {
            return;
        }
        let x0 = row.pos.x + caption_w;
        let gap = 4.0;
        let w = ((row.pos.x + row.size.x - x0) - gap * (screens.len() - 1) as f64) / screens.len() as f64;
        for (i, (screen_row, output)) in screens.iter().enumerate() {
            let r = rect(x0 + i as f64 * (w + gap), row.pos.y + 2.0, w, row.size.y - 4.0);
            let selected = target.as_deref() == Some(output.name.as_str());
            self.small_button(cx, r, (!selected).then_some(Hit::OptimiseFor(*screen_row)), selected, None, &screen_title(output));
        }
    }

    /// The Keep / Revert row while a change of `slot` is tried, then its
    /// note. Returns what is left below.
    fn draw_slot_extra(&mut self, cx: &mut Cx2d, rest: Rect, slot: &str, question: &str) -> Rect {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let mut rest = rest;
        if let Some(trial) = self.display_trials.trial_for(slot).cloned() {
            let (row, next) = cut_top(rest, CONTROL_H);
            rest = next;
            let inner = rect(row.pos.x, row.pos.y + 2.0, row.size.x, row.size.y - 4.0);
            let button_w = 64.0;
            let keep = rect(inner.pos.x + inner.size.x - button_w, inner.pos.y, button_w, inner.size.y);
            let revert = rect(keep.pos.x - 4.0 - button_w, inner.pos.y, button_w, inner.size.y);
            let text = match trial.phase {
                Phase::Applying { .. } => "Applying\u{2026}".to_string(),
                Phase::Confirming { .. } => {
                    let left = self.display_trials.seconds_left(self.display_now).unwrap_or(0);
                    format!("{question} Reverting in {left} s")
                }
            };
            let confirming = matches!(trial.phase, Phase::Confirming { .. });
            self.d.label_elided(cx, rect(inner.pos.x + 2.0, row.pos.y, (revert.pos.x - inner.pos.x - 8.0).max(0.0), row.size.y),
                confirming, tok.font.body_small, if confirming { tok.bar.active } else { dim }, HAlign::Left, &text);
            self.small_button(cx, revert, Some(Hit::TrialRevert), false, None, "Revert");
            self.small_button(cx, keep, confirming.then_some(Hit::TrialKeep), false, Some(Ico::Check), "Keep");
        }
        if let Some(note) = self.display_trials.note_for(slot).cloned() {
            let lines = if note.error { 2 } else { 1 };
            let (area, next) = cut_top(rest, NOTE_LINE_H * lines as f64);
            rest = next;
            let color = if note.error { tok.bar.active } else { dim };
            let wrapped = self.d.wrap(cx, false, tok.font.caption, &note.text, area.size.x, lines);
            for (i, line) in wrapped.iter().enumerate() {
                let r = rect(area.pos.x, area.pos.y + i as f64 * NOTE_LINE_H, area.size.x, NOTE_LINE_H);
                self.d.label(cx, r, false, tok.font.caption, color, HAlign::Left, line);
            }
        }
        rest
    }

    /// `[▭] Built-in · eDP-1  Intel · xe          [◀][▶] [Main]`, then
    /// `[2560×1600 · native ▾] [240 Hz ▾]` (Mirror: what the screen shows),
    /// the unfolded chips, the Keep / Revert row and the note.
    fn draw_screen_block(&mut self, cx: &mut Cx2d, area: Rect, row: usize, output: &LinuxDisplayOutput, placed: usize, mirror: bool) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let direct = self.display_snapshot.direct;
        let in_desktop = self.in_desktop(output);
        let text = if in_desktop { fg } else { dim };

        // Line one: the title, the GPU, then move and main.
        let (line, rest) = cut_top(area, SCREEN_LINE_H);
        let icon_w = 22.0;
        self.d.icon_centered(cx, Ico::Monitor, rect(line.pos.x, line.pos.y, icon_w, line.size.y), 14.0, text);
        let mut right = line.pos.x + line.size.x;
        if direct && !mirror {
            let main_w = 80.0;
            let main = rect(right - main_w, line.pos.y + 1.0, main_w, line.size.y - 2.0);
            if output.primary {
                self.small_button(cx, main, None, true, Some(Ico::Check), "Main");
            } else {
                let can = output.active && in_desktop;
                self.small_button(cx, main, can.then_some(Hit::ScreenMain(row)), false, None, "Make main");
            }
            let (left_ok, right_ok) = self.screen_moves(row, placed);
            if placed > 1 && row < placed {
                let move_w = 22.0;
                let r = rect(main.pos.x - 4.0 - move_w, main.pos.y, move_w, main.size.y);
                let l = rect(r.pos.x - 2.0 - move_w, main.pos.y, move_w, main.size.y);
                self.small_button(cx, l, left_ok.then_some(Hit::ScreenMove(row, -1)), false, Some(Ico::ChevronLeft), "");
                self.small_button(cx, r, right_ok.then_some(Hit::ScreenMove(row, 1)), false, Some(Ico::ChevronRight), "");
                right = l.pos.x - 8.0;
            } else {
                right = main.pos.x - 8.0;
            }
        }
        let title = screen_title(output);
        let name_x = line.pos.x + icon_w + 6.0;
        let name_w = self.d.measure(cx, true, tok.font.body_small, &title).min((right - name_x).max(0.0));
        self.d.label_elided(cx, rect(name_x, line.pos.y, name_w, line.size.y), true, tok.font.body_small, text, HAlign::Left, &title);
        let (status, problem) = self.screen_status(output);
        let detail = if !status.is_empty() {
            status
        } else if mirror {
            let target = matches!(self.arrangement_shown(), DisplayArrangement::Mirror { optimise_for } if optimise_for == output.name);
            if target { "mirror size".into() } else { "scaled to fit".into() }
        } else {
            self.screen_gpu_label(output)
        };
        let detail_x = name_x + name_w + 8.0;
        self.d.label_elided(cx, rect(detail_x, line.pos.y, (right - detail_x).max(0.0), line.size.y),
            false, tok.font.caption, if problem { tok.bar.active } else { dim }, HAlign::Left, &detail);

        // Line two: the pickers, or in Mirror what the screen shows.
        let (controls, mut rest) = cut_top(rest, CONTROL_H);
        let inner = rect(controls.pos.x + icon_w + 6.0, controls.pos.y + 2.0,
            (controls.size.x - icon_w - 6.0).max(0.0), controls.size.y - 4.0);
        let shown = shown_mode(output, self.mode_shown(output));
        // Every screen keeps its own resolution and refresh in Mirror too:
        // the mirror is rendered at the optimised screen's mode (changing it
        // resizes the mirror), the others scale it into theirs.
        {
            let enabled = direct && !output.modes.is_empty();
            let res_w = (inner.size.x * 0.6).floor();
            let res = rect(inner.pos.x, inner.pos.y, res_w - 4.0, inner.size.y);
            let rate = rect(inner.pos.x + res_w, inner.pos.y, inner.size.x - res_w, inner.size.y);
            let native = native_resolution(output);
            let (res_text, rate_text) = match shown {
                Some((w, h, hz)) => (
                    if native == Some((w, h)) { format!("{} \u{b7} native", resolution_label(w, h)) } else { resolution_label(w, h) },
                    rate_label(hz),
                ),
                None => (self.mode_shown(output).unwrap_or("Automatic").to_string(), String::new()),
            };
            let open = |kind| self.chooser.as_ref().is_some_and(|(name, k)| *name == output.name && *k == kind);
            let (res_open, rate_open) = (open(Chooser::Resolution), open(Chooser::Rate));
            self.picker_face(cx, res, enabled.then_some(Hit::ScreenResolution(row)), res_open, &res_text);
            self.picker_face(cx, rate, (enabled && shown.is_some()).then_some(Hit::ScreenRate(row)), rate_open, &rate_text);
            if res_open || rate_open {
                rest = self.draw_chooser(cx, rest, output, shown, if res_open { Chooser::Resolution } else { Chooser::Rate });
            }
        }
        self.draw_slot_extra(cx, rest, &output.name, "Keep this mode?");
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
        let x0 = area.pos.x + 28.0;
        let gap = 4.0;
        let w = ((area.pos.x + area.size.x - x0) - gap * (per_row - 1) as f64) / per_row as f64;
        for (i, (label, mode, current, native)) in items.into_iter().enumerate() {
            let r = rect(x0 + (i % per_row) as f64 * (w + gap), area.pos.y + (i / per_row) as f64 * CHIP_H + 2.0, w, CHIP_H - 4.0);
            let index = self.chooser_changes.len();
            self.chooser_changes.push(Change::Mode { name: output.name.clone(), mode });
            self.small_button(cx, r, (!current).then_some(Hit::ChooserItem(index)), current, None, &label);
            if native {
                // The screen's own resolution: a dot in the chip's corner.
                let tok = self.tokens;
                self.d.icon_centered(cx, Ico::Dot, rect(r.pos.x + r.size.x - 10.0, r.pos.y, 8.0, r.size.y), 6.0, tok.bar.active);
            }
        }
        rest
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
    fn small_button(&mut self, cx: &mut Cx2d, r: Rect, hit: Option<Hit>, selected: bool, icon: Option<Ico>, text: &str) {
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
        // the renderer's snapshot as shown, the change being tried and the
        // notes, with the screen rows and the chips as drawn.
        let d = &self.display_snapshot;
        let t = &self.display_trials;
        text.push_str(&format!(
            "\ndpi={:.2} dpi_hundredths={} dpi_saved={:?} display_settings_loaded={} display_direct={} display_outputs={} display_status={} arrangement={:?} arrangement_shown={:?}",
            self.current_dpi, self.dpi_hundredths(), s.dpi_scale, s.display_settings_loaded,
            d.direct, d.outputs.len(), self.display_status(), d.arrangement, self.arrangement_shown()
        ));
        text.push_str(&format!(
            "\ntrial={:?} seconds_left={:?} restoring={:?} notes={:?}",
            t.trial.as_ref().map(|t| (&t.change, &t.previous, &t.phase)), t.seconds_left(self.display_now), t.restoring, t.notes
        ));
        text.push_str(&format!(
            "\npacing={:?} paced_by={:?} pace_label={:?} pace_shown={} pace_open={} pace_targets={:?}",
            d.pacing, d.paced_by, pacing_label(&d.pacing, d), self.pace_shown(), self.pace_open, self.pace_targets
        ));
        text.push_str(&format!(
            "\nmain={:?} main_pending={:?} main_saved={:?} screen_rows={:?} screens_placed={} screens_span_gpus={} chooser={:?} chooser_changes={:?}",
            d.outputs.iter().find(|o| o.primary).map(|o| o.name.as_str()),
            self.display_source_pending.as_ref().map(|(name, _)| name.as_str()),
            s.display_layout.main_name(d), self.screen_targets, self.screen_placed, screens_span_gpus(d),
            self.chooser, self.chooser_changes
        ));
        let (order, placed) = screen_rows(d);
        for (row, &index) in order.iter().enumerate() {
            let output = &d.outputs[index];
            let (status, _) = self.screen_status(output);
            let (left, right) = self.screen_moves(row, placed);
            let shown = shown_mode(output, self.mode_shown(output));
            text.push_str(&format!(
                "\nscreen: {row} {} title={:?} gpu={} status={:?} position={:?} main={} move_left={} move_right={} shown={:?} mode_shown={:?} mode_override={:?} resolutions={} rates_here={}",
                output.name, screen_title(output), self.screen_gpu_label(output), status, output.desktop_position, output.primary,
                left, right, shown, self.mode_shown(output), output.mode_override, resolutions(&output.modes, native_resolution(output)).len(),
                shown.map_or(0, |(w, h, _)| rates_for(&output.modes, w, h).len())
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
                "\ndisplay: {} {}x{} @ {:.2} Hz primary={} active={} status={}",
                output.name, output.width, output.height, output.refresh_hz, output.primary, output.active, output.status
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


/// Whether the saved render-on choice (`saved`, `None` for Auto, an
/// identity `<pci> <vendor>:<device>` otherwise) is not what renders now:
/// `running` is the renderer's identity, `running_is_display` whether it
/// is the display GPU (what Auto picks). Unknown means not different, so
/// a restart is never offered on a guess.
#[cfg(test)]
fn next_start_differs(saved: Option<&str>, running: Option<&str>, running_is_display: Option<bool>) -> bool {
    match saved {
        None => running_is_display == Some(false),
        Some(saved) => running.is_some_and(|running| running != saved),
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
    fn next_start_differs_only_when_known_and_different() {
        let a = "0000:00:02.0 8086:a780";
        let b = "0000:01:00.0 10de:2b85";
        assert!(!next_start_differs(Some(a), Some(a), Some(false)));
        assert!(next_start_differs(Some(b), Some(a), Some(true)));
        // Auto is the display GPU.
        assert!(!next_start_differs(None, Some(a), Some(true)));
        assert!(next_start_differs(None, Some(b), Some(false)));
        // Nothing known about the renderer: never offer a restart.
        assert!(!next_start_differs(Some(b), None, None));
        assert!(!next_start_differs(None, None, None));
    }

    #[test]
    fn restart_only_for_a_saved_gpu_that_is_present() {
        let gpu = GpuInfo {
            card: "card1".into(),
            pci: "0000:01:00.0".into(),
            vendor: "10de".into(),
            device: "2b85".into(),
            driver: "nvidia".into(),
            connected: Vec::new(),
        };
        assert!(saved_gpu_present(None, &[]));
        assert!(saved_gpu_present(Some("0000:01:00.0 10de:2b85"), std::slice::from_ref(&gpu)));
        // Moved card or another model at that address: the next start
        // falls back to Auto, so a restart would change nothing.
        assert!(!saved_gpu_present(Some("0000:02:00.0 10de:2b85"), std::slice::from_ref(&gpu)));
        assert!(!saved_gpu_present(Some("0000:01:00.0 10de:2b84"), &[gpu]));
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
}
