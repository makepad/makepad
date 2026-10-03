//! Linux status dropdowns, drawn from the worker's observed device state.
//!
//! The Display dropdown adds what the other platforms do not have:
//!
//! * DPI SCALE — 1.00×–3.00× in hundredths; previewed while held, applied
//!   on release through the platform's `dpi_override`, persisted by the
//!   system worker.
//! * DISPLAYS — the wide desktop as the renderer reports it: a picture
//!   of the screens side by side, to scale by `desktop_position`, with
//!   the main screen highlighted; then one two-line row per screen, left
//!   to right, then the screens outside the desktop with the renderer's
//!   own status. Each row moves its screen one place left or right, makes
//!   it the main screen (where the dock goes), and picks its mode
//!   ("Automatic (fastest)" or one the screen offers). Every edit is
//!   applied live through the renderer and saved as the whole
//!   `display-layout` (`shell::display_layout`).
//! * Render on — the GPU the desktop renders on. The list is Auto plus
//!   the GPUs; the check marks the saved choice, and the status line says
//!   which GPU renders now and which one the next start uses. While the
//!   screens span GPUs the renderer cannot switch live, so a pick is only
//!   saved, and "Restart desktop now" (two steps: apps close) appears
//!   while the saved choice is not the running one. On a one-GPU desktop
//!   the pick switches live and is saved once the switch is done. The
//!   same list can set the focused app's GPU, for this session only.
//! * Mouse & touchpad — a row (only while the WM drives the outputs) that
//!   opens a subview with mouse and touchpad speed sliders. The speeds are
//!   the process's actual values; a drag applies immediately.
//!
//! The card is bounded by the screen. At 1080p ×3 (360 logical points in
//! all) the sliders and the picker rows still fit: the arrangement
//! picture, the backlight caption, the GPU status line, then the hero's
//! and the sliders' slack give way first, then the screen rows down to
//! one, which scroll, and an unfolded list shrinks to what fits and
//! scrolls too. The Mouse & touchpad, Render on and restart rows are kept.
use super::*;
use super::super::ui::HAlign;
use crate::linux_controls::ControlAction;
use crate::shell::display_layout::{connector_suffix, screen_rows, screens_span_gpus, swap_neighbour};
use crate::shell::system_linux::{
    Availability, BatteryStatus, GpuInfo, SystemCommand, DISPLAY_SCALE_MAX, DISPLAY_SCALE_MIN,
};
use makepad_widgets::makepad_platform::linux_display::LinuxDisplayOutput;
use makepad_widgets::makepad_platform::linux_gpu::LinuxGpuDevice;
use makepad_widgets::makepad_platform::linux_input::{
    pointer_speed, POINTER_SPEED_MAX, POINTER_SPEED_MIN,
};

const DEVICE_ROWS: usize = 5;
/// The launch default (`-scale=1.3`) shown until the platform reports the
/// window's actual scale.
const DPI_FALLBACK_HUNDREDTHS: u32 = 130;
// The Display dropdown's metrics, in logical points.
const HERO_H: f64 = 40.0;
const HERO_COMPACT_H: f64 = 32.0;
const SEP_FIRST_H: f64 = 16.0;
const SEP_H: f64 = 12.0;
const HEADER_H: f64 = 22.0;
const SLIDER_H: f64 = 34.0;
const SLIDER_COMPACT_H: f64 = 30.0;
const BACKLIGHT_CAPTION_H: f64 = 30.0;
const ENDS_H: f64 = 14.0;
/// The "Render on", "App GPU", restart and Mouse & touchpad rows.
const PICKER_ROW_H: f64 = 30.0;
const PICKER_ROW_COMPACT_H: f64 = 26.0;
/// A row of an unfolded list (a screen's modes, the GPUs).
const LIST_ITEM_H: f64 = 26.0;
const GPU_ITEMS_MAX: usize = 4;
const MODE_ITEMS_MAX: usize = 5;
/// "Now on … · next start: …" under the Render on rows.
const GPU_STATUS_H: f64 = 18.0;
/// One line of a screen row; a row is two (name and status, then the
/// controls).
const SCREEN_LINE_H: f64 = 26.0;
const SCREEN_ROW_H: f64 = 2.0 * SCREEN_LINE_H + 2.0;
/// Screen rows the card asks for before the list scrolls; the screen may
/// bound it further.
const SCREEN_ROWS_MAX: usize = 4;
/// The arrangement picture, drawn only when it fits above the rows.
const DISPLAY_PICTURE_H: f64 = 64.0;
const NOTICE_H: f64 = 18.0;
/// Caption under the pointer-speed sliders.
const POINTER_CAPTION_H: f64 = 18.0;
/// The first mode choice: no override, the screen's fastest native mode.
const MODE_AUTOMATIC: &str = "Automatic (fastest)";

/// What the Display dropdown draws at the height it actually has.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MonitorPlan {
    hero: f64,
    sep_first: f64,
    slider: f64,
    /// The "1.00 / 3.00" endpoints row under the DPI slider.
    ends: f64,
    picker_row: f64,
    backlight_caption: bool,
    /// The arrangement picture.
    picture: bool,
    gpu_status: bool,
    /// Rows of the one unfolded GPU list (render-on or app).
    gpu_items: usize,
    /// The focused-app GPU picker row.
    app_gpu_row: bool,
    /// "Restart desktop now", while the saved render-on GPU is not the
    /// running one.
    restart_row: bool,
    /// Screen rows shown; the rest scroll.
    rows: usize,
    /// One row's height: two lines, or one for the "no screens" message.
    row_h: f64,
    /// Rows of an unfolded mode list, under the screen row it belongs to.
    mode_items: usize,
    /// The "Mouse & touchpad" row, only while the WM drives the outputs.
    pointer_row: bool,
}

impl MonitorPlan {
    fn list_open(&self) -> bool {
        self.mode_items > 0 || self.gpu_items > 0
    }
}

/// The height `p` takes, with the notice line when there is one.
fn plan_height(p: &MonitorPlan, notice: bool) -> f64 {
    p.hero + p.sep_first + HEADER_H + p.slider
        + if p.backlight_caption { BACKLIGHT_CAPTION_H } else { 0.0 }
        + SEP_H + HEADER_H + p.slider + p.ends
        + if p.pointer_row { SEP_H + p.picker_row } else { 0.0 }
        + SEP_H + HEADER_H
        + if p.picture { DISPLAY_PICTURE_H } else { 0.0 }
        + p.rows as f64 * p.row_h
        + p.mode_items as f64 * LIST_ITEM_H
        + p.picker_row + if p.app_gpu_row { p.picker_row } else { 0.0 }
        + p.gpu_items as f64 * LIST_ITEM_H
        + if p.gpu_status { GPU_STATUS_H } else { 0.0 }
        + if p.restart_row { p.picker_row } else { 0.0 }
        + if notice { NOTICE_H } else { 0.0 }
}

/// `full` fitted to `avail` points. The arrangement picture goes first,
/// then the backlight caption, then the GPU status line, then the hero,
/// the endpoints and the sliders' slack, then the screen rows give way
/// down to one — to none while the GPU list is unfolded, never below the
/// one an unfolded mode list belongs to — then an unfolded list shrinks
/// to what fits, then the one remaining row, and last of all the hero.
/// The sliders, the Render on, App GPU, restart and Mouse & touchpad rows
/// are never dropped.
fn fit_monitor_plan(full: MonitorPlan, notice: bool, avail: f64) -> MonitorPlan {
    let height = |p: &MonitorPlan| plan_height(p, notice);
    let mut plan = full;
    if height(&plan) > avail {
        plan.picture = false;
    }
    if height(&plan) > avail {
        plan.backlight_caption = false;
    }
    if height(&plan) > avail {
        plan.gpu_status = false;
    }
    if height(&plan) > avail {
        plan.hero = HERO_COMPACT_H;
        plan.sep_first = SEP_H;
        plan.ends = 0.0;
        plan.slider = SLIDER_COMPACT_H;
        plan.picker_row = PICKER_ROW_COMPACT_H;
    }
    if height(&plan) > avail {
        let rows_wanted = plan.rows;
        plan.rows = 0;
        let fixed = height(&plan);
        let fit = ((avail - fixed) / plan.row_h).floor().max(0.0) as usize;
        let floor = if plan.mode_items > 0 || plan.gpu_items == 0 { 1 } else { 0 };
        plan.rows = fit.min(rows_wanted).max(floor.min(rows_wanted));
    }
    if plan.mode_items > 1 && height(&plan) > avail {
        let items_wanted = plan.mode_items;
        plan.mode_items = 0;
        let fixed = height(&plan);
        let fit = ((avail - fixed) / LIST_ITEM_H).floor().max(0.0) as usize;
        plan.mode_items = fit.clamp(1, items_wanted);
    }
    if plan.gpu_items > 1 && height(&plan) > avail {
        let items_wanted = plan.gpu_items;
        plan.gpu_items = 0;
        let fixed = height(&plan);
        let fit = ((avail - fixed) / LIST_ITEM_H).floor().max(0.0) as usize;
        plan.gpu_items = fit.clamp(1, items_wanted);
    }
    if plan.rows == 1 && plan.mode_items == 0 && height(&plan) > avail {
        // The DISPLAYS header still counts the screens; the pickers and
        // the sliders matter more than the one row that would clip.
        plan.rows = 0;
    }
    if height(&plan) > avail {
        // On short screens keep the controls and an unfolded choice
        // reachable; their section labels can stand in for the hero.
        plan.hero = 0.0;
        plan.sep_first = 0.0;
    }
    plan
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
        self.app_gpu_picker = false;
        self.gpu_scroll = 0;
        self.gpu_targets.clear();
        self.gpu_target_client = None;
    }

    /// Fold the mode list (its targets go with it).
    pub(super) fn close_mode_list(&mut self) {
        self.mode_picker = None;
        self.mode_scroll = 0;
        self.mode_targets.clear();
        self.mode_target_screen = None;
    }

    /// One list at a time: fold every Display list and drop a restart
    /// confirm that is waiting.
    pub(super) fn close_display_lists(&mut self) {
        self.close_gpu_lists();
        self.close_mode_list();
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
                let name = self.screen_targets.get(row).cloned().filter(|name| {
                    self.display_snapshot.outputs.iter().any(|o| o.name == *name && o.active && self.in_desktop(o))
                });
                self.close_display_lists();
                if let Some(name) = name {
                    self.system_action(cx, ControlAction::DisplaySource(name));
                }
            }
            Hit::ScreenModePicker(row) => {
                let name = self.screen_targets.get(row).cloned();
                let open = name.is_some() && self.mode_picker != name;
                self.close_display_lists();
                if open {
                    // Unfolded with the current mode as the first shown row,
                    // so it is in view however few rows the screen allows.
                    self.mode_scroll = name
                        .as_ref()
                        .and_then(|name| self.display_snapshot.outputs.iter().find(|o| o.name == *name))
                        .and_then(|o| mode_choice_current(&mode_choices(&o.modes), o.mode_override.as_deref()))
                        .unwrap_or(0);
                    self.mode_picker = name;
                }
            }
            Hit::ScreenMode(row) => {
                let mode = self.mode_targets.get(row).cloned();
                let screen = self.mode_target_screen.clone();
                self.close_display_lists();
                if let (Some(mode), Some(name)) = (mode, screen) {
                    self.system_action(cx, ControlAction::DisplayMode { name, mode });
                }
            }
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
            Hit::AppGpuPicker => {
                let open = !self.app_gpu_picker;
                let client = self.gpu_app.as_ref().map(|(id, _, _, _)| *id);
                self.close_display_lists();
                self.app_gpu_picker = open;
                self.gpu_target_client = if open { client } else { None };
                self.gpu_scroll = if open { self.gpu_app_current_row() } else { 0 };
            }
            Hit::PointerSettings => {
                self.pointer_settings = !self.pointer_settings;
                self.dragging = None;
                self.dpi_preview = None;
                self.close_display_lists();
            }
            Hit::Gpu(row) => {
                let choice = self.gpu_targets.get(row).cloned();
                let app_client = self.gpu_target_client;
                self.close_gpu_lists();
                if self.gpu_busy {
                    // Progress is the notice line; do not start another switch.
                } else if let Some(choice) = choice {
                    let usable = match choice.as_deref() {
                        None => true,
                        Some(identity) => self.gpu_identity_available(identity),
                    };
                    if usable {
                        if let Some(client) = app_client {
                            self.system_action(cx, ControlAction::AppGpuChoice { client, choice });
                        } else {
                            self.system_action(cx, ControlAction::GpuChoice(choice));
                        }
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

    /// The slider's 0..1 as a scale in hundredths, 100..=300.
    pub(super) fn dpi_from_slider(v: f64) -> u32 {
        let span = (DISPLAY_SCALE_MAX - DISPLAY_SCALE_MIN) as f64;
        (DISPLAY_SCALE_MIN as f64 + v.clamp(0.0, 1.0) * span).round() as u32
    }

    fn dpi_slider_progress(hundredths: u32) -> f64 {
        let clamped = hundredths.clamp(DISPLAY_SCALE_MIN, DISPLAY_SCALE_MAX);
        (clamped - DISPLAY_SCALE_MIN) as f64 / (DISPLAY_SCALE_MAX - DISPLAY_SCALE_MIN) as f64
    }

    /// The slider's 0..1 as a pointer speed in hundredths, 25..=300.
    pub(super) fn pointer_from_slider(v: f64) -> u32 {
        let span = (POINTER_SPEED_MAX - POINTER_SPEED_MIN) as f64;
        (POINTER_SPEED_MIN as f64 + v.clamp(0.0, 1.0) * span).round() as u32
    }

    fn pointer_slider_progress(hundredths: u32) -> f64 {
        let clamped = hundredths.clamp(POINTER_SPEED_MIN, POINTER_SPEED_MAX);
        (clamped - POINTER_SPEED_MIN) as f64 / (POINTER_SPEED_MAX - POINTER_SPEED_MIN) as f64
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

    /// The DISPLAYS header's reading of the renderer's snapshot: how many
    /// screens make up the desktop.
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

    /// A screen row's trailing status and whether it is a problem: "Main"
    /// for the main screen of the desktop, nothing for another screen in
    /// it, "Main…" while a main-screen pick waits for the renderer, and
    /// the renderer's own status for a screen outside the desktop (failed,
    /// another GPU's screen still to join, …) — never a saved main screen
    /// shown as pending.
    fn screen_status(&self, output: &LinuxDisplayOutput) -> (String, bool) {
        if self.in_desktop(output) {
            let pending = self.display_source_pending.as_ref().is_some_and(|(name, _)| *name == output.name);
            if output.primary {
                ("Main".into(), false)
            } else if pending {
                ("Main\u{2026}".into(), false)
            } else {
                (String::new(), false)
            }
        } else if output.status.is_empty() {
            ("Inactive".into(), false)
        } else {
            (output.status.clone(), true)
        }
    }

    /// Part of the desktop: placed, or active while nothing is placed yet.
    /// An active screen left out by the GPU's limits is not.
    fn in_desktop(&self, output: &LinuxDisplayOutput) -> bool {
        output.desktop_position.is_some() || (output.active && !self.any_placed())
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
        let direct = self.display_snapshot.direct;
        (direct && row > 0 && row < placed, direct && row + 1 < placed)
    }

    /// The output whose mode list is unfolded, while it is still listed.
    fn mode_picker_output(&self) -> Option<&LinuxDisplayOutput> {
        let name = self.mode_picker.as_ref()?;
        self.display_snapshot.outputs.iter().find(|o| o.name == *name)
    }

    // ---- the GPU the compositor runs on -----------------------------

    /// The sysfs GPU index compositing now: the snapshot's `renderer`
    /// matched onto `system.gpus` by exact PCI address and vendor/device
    /// when PCI is known, or by a unique vendor/device pair when it is
    /// not. `None` when the renderer is unknown, unmatched, or ambiguous.
    /// Never taken from the output list.
    fn gpu_now(&self) -> Option<usize> {
        let renderer = self.gpu_snapshot.renderer.as_ref()?;
        match renderer.pci_address.as_deref().filter(|pci| !pci.is_empty()) {
            Some(pci) => self.system.gpus.iter().position(|g| {
                !g.pci.is_empty() && g.pci == pci && gpu_vendor_device_match(g, renderer)
            }),
            None => {
                let mut found = None;
                for (i, gpu) in self.system.gpus.iter().enumerate() {
                    if gpu_vendor_device_match(gpu, renderer) {
                        if found.is_some() {
                            return None;
                        }
                        found = Some(i);
                    }
                }
                found
            }
        }
    }

    /// Sysfs label when `gpu_now` matched a card; otherwise the renderer's
    /// own `name`. "unknown" only when there is no renderer name either.
    fn gpu_label(&self, index: Option<usize>) -> String {
        if let Some(gpu) = index.and_then(|i| self.system.gpus.get(i)) {
            gpu.label()
        } else if let Some(name) = self
            .gpu_snapshot
            .renderer
            .as_ref()
            .map(|r| r.name.trim())
            .filter(|n| !n.is_empty())
        {
            name.to_string()
        } else {
            "unknown".into()
        }
    }

    fn gpu_matches_snapshot(&self, gpu: &GpuInfo) -> bool {
        if gpu.pci.is_empty() {
            return false;
        }
        self.gpu_snapshot.devices.iter().any(|device| {
            device.pci_address.as_deref() == Some(gpu.pci.as_str()) && gpu_vendor_device_match(gpu, device)
        })
    }

    fn gpu_identity_available(&self, identity: &str) -> bool {
        self.system.gpus.iter().any(|gpu| gpu.matches(identity) && self.gpu_matches_snapshot(gpu))
    }

    fn gpu_index_for_uuid(&self, uuid: [u8; 16]) -> Option<usize> {
        let device = self.gpu_snapshot.devices.iter().find(|d| d.uuid == uuid)?;
        let pci = device.pci_address.as_deref().filter(|pci| !pci.is_empty())?;
        self.system.gpus.iter().position(|gpu| {
            !gpu.pci.is_empty() && gpu.pci == pci && gpu_vendor_device_match(gpu, device)
        })
    }

    fn gpu_label_for_uuid(&self, uuid: [u8; 16]) -> String {
        if let Some(gpu) = self.gpu_index_for_uuid(uuid).and_then(|i| self.system.gpus.get(i)) {
            gpu.label()
        } else if let Some(name) = self
            .gpu_snapshot
            .devices
            .iter()
            .find(|d| d.uuid == uuid)
            .map(|d| d.name.trim())
            .filter(|n| !n.is_empty())
        {
            name.to_string()
        } else {
            "unknown".into()
        }
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

    /// The saved render-on choice is not what renders now, so it takes a
    /// restart: only when the renderer is known, the session environment
    /// does not pin the GPU, and no switch is in progress.
    fn render_on_differs(&self) -> bool {
        let running = self.gpu_now().map(|i| self.system.gpus[i].identity());
        let renderer = self.gpu_snapshot.renderer.as_ref().map(|g| g.uuid);
        let display = self.gpu_snapshot.display.as_ref().map(|g| g.uuid);
        let running_is_display = renderer.zip(display).map(|(r, d)| r == d);
        next_start_differs(self.render_on_saved.as_deref(), running.as_deref(), running_is_display)
    }

    /// "Restart desktop now" is offered: the saved settings have been read
    /// (an unread file is not "Auto"), the WM drives the screens, the
    /// saved render-on GPU is present (the next start can only fall back
    /// to Auto for one that is not, so a restart would change nothing),
    /// and it is not the running one.
    fn restart_row_shown(&self) -> bool {
        self.display_snapshot.direct
            && self.system.display_settings_loaded
            && saved_gpu_present(self.render_on_saved.as_deref(), &self.system.gpus)
            && !self.gpu_busy
            && self.system.gpu_env.is_none()
            && self.render_on_differs()
    }

    /// Follow compositor is row 0; otherwise the policy GPU in sysfs order.
    fn gpu_app_current_row(&self) -> usize {
        match self.gpu_app.as_ref() {
            Some((_, _, None, _)) => 0,
            Some((_, _, Some(uuid), _)) => self.gpu_index_for_uuid(*uuid).map(|i| i + 1).unwrap_or(0),
            None => 0,
        }
    }

    fn gpu_picker_enabled(&self) -> bool {
        self.display_snapshot.direct && !self.gpu_busy
    }

    fn gpu_list_open(&self) -> bool {
        self.gpu_picker || (self.app_gpu_picker && self.gpu_app.is_some())
    }

    // ---- layout --------------------------------------------------------

    /// Everything, at full size: what the card asks the screen for.
    fn full_plan(&self) -> MonitorPlan {
        MonitorPlan {
            hero: HERO_H,
            sep_first: SEP_FIRST_H,
            slider: SLIDER_H,
            ends: ENDS_H,
            picker_row: PICKER_ROW_H,
            backlight_caption: true,
            picture: self.any_placed(),
            gpu_status: self.display_snapshot.direct && (!self.system.gpus.is_empty() || self.gpu_snapshot.renderer.is_some()),
            gpu_items: if self.gpu_list_open() { (1 + self.system.gpus.len()).min(GPU_ITEMS_MAX) } else { 0 },
            app_gpu_row: self.gpu_app.is_some(),
            restart_row: self.restart_row_shown(),
            rows: self.display_snapshot.outputs.len().clamp(1, SCREEN_ROWS_MAX),
            row_h: if self.display_snapshot.outputs.is_empty() { SCREEN_LINE_H } else { SCREEN_ROW_H },
            mode_items: self
                .mode_picker_output()
                .filter(|o| !o.modes.is_empty())
                .map_or(0, |o| mode_choices(&o.modes).len().min(MODE_ITEMS_MAX)),
            pointer_row: self.display_snapshot.direct,
        }
    }

    /// The plan for the height the card really has (the screen bounds
    /// it); see [`fit_monitor_plan`].
    fn monitor_plan(&self, avail: f64) -> MonitorPlan {
        fit_monitor_plan(self.full_plan(), !self.system_notice.is_empty(), avail)
    }

    /// The card height the Display dropdown asks for; `card_rect` bounds it
    /// to the screen and `monitor_plan` fits the content to what is left.
    pub(super) fn monitor_height(&self) -> f64 {
        if self.pointer_settings {
            return self.pointer_settings_height();
        }
        plan_height(&self.full_plan(), !self.system_notice.is_empty())
    }

    fn pointer_settings_height(&self) -> f64 {
        let notice = if self.system_notice.is_empty() { 0.0 } else { NOTICE_H };
        HERO_H + SEP_H + PICKER_ROW_H
            + SEP_H + HEADER_H + SLIDER_H + ENDS_H
            + SEP_H + HEADER_H + SLIDER_H + ENDS_H
            + POINTER_CAPTION_H
            + notice
    }

    /// The wheel over an overflowing screen list; the draw clamps.
    pub(super) fn display_scroll_by(&mut self, cx: &mut Cx, dy: f64) {
        self.display_scroll = if dy > 0.0 { self.display_scroll + 1 } else { self.display_scroll.saturating_sub(1) };
        self.redraw(cx);
    }

    /// The wheel over an unfolded mode list; the draw clamps.
    pub(super) fn mode_scroll_by(&mut self, cx: &mut Cx, dy: f64) {
        self.mode_scroll = if dy > 0.0 { self.mode_scroll + 1 } else { self.mode_scroll.saturating_sub(1) };
        self.redraw(cx);
    }

    /// The wheel over the unfolded "Render on" list; the draw clamps.
    pub(super) fn gpu_scroll_by(&mut self, cx: &mut Cx, dy: f64) {
        self.gpu_scroll = if dy > 0.0 { self.gpu_scroll + 1 } else { self.gpu_scroll.saturating_sub(1) };
        self.redraw(cx);
    }

    pub(super) fn draw_monitor(&mut self, cx: &mut Cx2d, body: Rect) {
        if self.pointer_settings {
            self.draw_pointer_settings(cx, body);
            return;
        }
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let plan = self.monitor_plan(body.size.y);
        let snapshot = self.system.clone();
        let brightness = snapshot.brightness.percent();
        let (hero, rest) = cut_top(body, plan.hero);
        if plan.hero > 0.0 {
            self.d.panel_hero(cx, hero, &tok, fg, Ico::Monitor, "Display", "Brightness, scale and outputs", 0.0);
        }
        let (sep, rest) = cut_top(rest, plan.sep_first); self.d.separator(cx, sep, fg, 0.12);
        let (header, rest) = cut_top(rest, HEADER_H);
        self.section_header(cx, header, "BRIGHTNESS", &brightness.map_or("—".into(), |v| format!("{v}%")));
        let (row, mut rest) = cut_top(rest, plan.slider);
        self.slider_row(cx, row, Hit::BrightnessSlider, brightness.unwrap_or(0) as f64 / 100.0, brightness.is_some());
        if plan.backlight_caption {
            let (row, next) = cut_top(rest, BACKLIGHT_CAPTION_H); rest = next;
            self.unavailable(cx, row, snapshot.brightness.primary().map(|b| b.name.as_str())
                .unwrap_or("Use this monitor’s brightness buttons"));
        }

        // ---- DPI scale: the window's actual scale, or the held preview.
        let (sep, next) = cut_top(rest, SEP_H); rest = next; self.d.separator(cx, sep, fg, 0.12);
        let (header, next) = cut_top(rest, HEADER_H); rest = next;
        let shown = self.dpi_preview.unwrap_or_else(|| self.dpi_hundredths());
        self.section_header(cx, header, "DPI SCALE", &format!("{:.2}×", shown as f64 / 100.0));
        let (row, next) = cut_top(rest, plan.slider); rest = next;
        self.slider_row(cx, row, Hit::DpiScaleSlider, Self::dpi_slider_progress(shown), true);
        if plan.ends > 0.0 {
            let (ends, next) = cut_top(rest, plan.ends); rest = next;
            self.d.label(cx, ends, false, tok.font.caption, dim, HAlign::Left, "1.00");
            self.d.label(cx, ends, false, tok.font.caption, dim, HAlign::Right, "3.00");
            if self.dpi_preview.is_some() {
                self.d.label(cx, ends, false, tok.font.caption, tok.bar.active, HAlign::Center,
                    "Release to apply · Esc cancels");
            }
        }

        if plan.pointer_row {
            let (sep, next) = cut_top(rest, SEP_H); rest = next; self.d.separator(cx, sep, fg, 0.12);
            let (row, next) = cut_top(rest, plan.picker_row); rest = next;
            self.draw_pointer_entry(cx, row);
        }

        // ---- the screens: the arrangement, a row each, the render GPU.
        let (sep, next) = cut_top(rest, SEP_H); rest = next; self.d.separator(cx, sep, fg, 0.12);
        let (header, next) = cut_top(rest, HEADER_H); rest = next;
        let status = self.display_status();
        self.section_header(cx, header, "DISPLAYS", &status);
        let rest = self.draw_screens(cx, rest, &plan);
        let rest = self.draw_gpu_picker(cx, rest, &plan);
        if !self.system_notice.is_empty() {
            let notice = self.system_notice.clone();
            let (row, _) = cut_top(rest, NOTICE_H);
            self.d.label_elided(cx, row, false, tok.font.caption, fg, HAlign::Left, &notice);
        }
    }

    /// `Mouse & touchpad` with the live speeds on the right. Wider caption
    /// than [`Self::picker_row`] so the label is not clipped.
    fn draw_pointer_entry(&mut self, cx: &mut Cx2d, row: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let hit = Hit::PointerSettings;
        self.d.cursor_surface(cx, row, &tok.controls, self.hot == Some(hit), self.pointer_settings);
        let label_w = 148.0;
        self.d.label(cx, rect(row.pos.x + 5.0, row.pos.y, label_w, row.size.y),
            false, tok.font.body_small, dim, HAlign::Left, "Mouse & touchpad");
        let mouse = pointer_speed(false);
        let pad = pointer_speed(true);
        let value = format!("{:.2}× · {:.2}×", mouse as f64 / 100.0, pad as f64 / 100.0);
        let chevron_w = 26.0;
        let value_x = row.pos.x + label_w + 5.0;
        self.d.label_elided(cx, rect(value_x, row.pos.y, (row.pos.x + row.size.x - chevron_w - value_x).max(0.0), row.size.y),
            false, tok.font.body, fg, HAlign::Right, &value);
        self.d.icon_centered(cx, Ico::ChevronDown,
            rect(row.pos.x + row.size.x - 22.0, row.pos.y, 18.0, row.size.y), 12.0, fg);
        self.hits.push((hit, row));
    }

    fn draw_pointer_settings(&mut self, cx: &mut Cx2d, body: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let compact = body.size.y < self.pointer_settings_height();
        let (hero, rest) = cut_top(body, if compact { HERO_COMPACT_H } else { HERO_H });
        self.d.panel_hero(cx, hero, &tok, fg, Ico::Monitor, "Mouse & touchpad", "", 0.0);
        let (sep, rest) = cut_top(rest, SEP_H); self.d.separator(cx, sep, fg, 0.12);
        let (back, mut rest) = cut_top(rest, PICKER_ROW_H);
        let hit = Hit::PointerSettings;
        self.d.cursor_surface(cx, back, &tok.controls, self.hot == Some(hit), false);
        self.d.icon_centered(cx, Ico::ChevronLeft,
            rect(back.pos.x, back.pos.y, 22.0, back.size.y), 12.0, fg);
        self.d.label(cx, rect(back.pos.x + 22.0, back.pos.y, (back.size.x - 22.0).max(0.0), back.size.y),
            false, tok.font.body, fg, HAlign::Left, "Back to Display");
        self.hits.push((hit, back));
        for touchpad in [false, true] {
            let (sep, next) = cut_top(rest, SEP_H); rest = next; self.d.separator(cx, sep, fg, 0.12);
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

    /// One picker row: the caption on the left, the value on the right,
    /// the chevron and the hit when enabled.
    #[allow(clippy::too_many_arguments)]
    fn picker_row(&mut self, cx: &mut Cx2d, row: Rect, hit: Hit, open: bool, enabled: bool,
        caption: &str, value: &str, color: Vec4f) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        if enabled {
            self.d.cursor_surface(cx, row, &tok.controls, self.hot == Some(hit), open);
        }
        let label_w = 92.0;
        self.d.label(cx, rect(row.pos.x + 5.0, row.pos.y, label_w, row.size.y),
            false, tok.font.body_small, dim, HAlign::Left, caption);
        let chevron_w = if enabled { 26.0 } else { 4.0 };
        let value_x = row.pos.x + label_w + 5.0;
        self.d.label_elided(cx, rect(value_x, row.pos.y, (row.pos.x + row.size.x - chevron_w - value_x).max(0.0), row.size.y),
            false, tok.font.body, color, HAlign::Right, value);
        if enabled {
            self.d.icon_centered(cx, Ico::ChevronDown,
                rect(row.pos.x + row.size.x - 22.0, row.pos.y, 18.0, row.size.y), 12.0, fg);
            self.hits.push((hit, row));
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

    /// The "Render on" row (the saved choice) and its list, the optional
    /// app row, the one unfolded GPU list immediately under the row that
    /// owns it, the "Now on … · next start: …" line, and "Restart desktop
    /// now" while the saved choice is not the running one.
    fn draw_gpu_picker(&mut self, cx: &mut Cx2d, rest: Rect, plan: &MonitorPlan) -> Rect {
        if !self.gpu_picker && !self.app_gpu_picker {
            self.gpu_targets.clear();
            self.gpu_overflow = false;
        }
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let direct = self.display_snapshot.direct;
        let enabled = self.gpu_picker_enabled();
        let now_label = self.gpu_label(self.gpu_now());
        let (row, mut rest) = cut_top(rest, plan.picker_row);
        let (value, color) = if !direct {
            ("Managed by the desktop".to_string(), dim)
        } else {
            (self.render_on_saved_label(), if enabled { fg } else { dim })
        };
        self.picker_row(cx, row, Hit::GpuPicker, self.gpu_picker, enabled, "Render on", &value, color);
        if self.gpu_picker {
            rest = self.draw_gpu_choice_list(cx, rest, plan, "Auto", self.gpu_saved_row(), enabled);
        }
        if plan.app_gpu_row {
            if let Some((_, label, policy, _)) = self.gpu_app.clone() {
                let (row, next) = cut_top(rest, plan.picker_row);
                rest = next;
                let gpu = match policy {
                    None => "Follow compositor".to_string(),
                    Some(uuid) => self.gpu_label_for_uuid(uuid),
                };
                let value = format!("{label}: {gpu}");
                self.picker_row(
                    cx,
                    row,
                    Hit::AppGpuPicker,
                    self.app_gpu_picker,
                    enabled,
                    "App GPU",
                    &value,
                    if enabled { fg } else { dim },
                );
                if self.app_gpu_picker {
                    rest = self.draw_gpu_choice_list(
                        cx,
                        rest,
                        plan,
                        "Follow compositor",
                        self.gpu_app_current_row(),
                        enabled,
                    );
                }
            }
        }
        if plan.gpu_status {
            let (line, remaining) = cut_top(rest, GPU_STATUS_H);
            rest = remaining;
            let next = self.render_on_next_label();
            let text = match &self.gpu_app {
                Some((.., actual)) => format!(
                    "Now on {now_label} \u{b7} next start: {next} \u{b7} app: {}",
                    self.gpu_label_for_uuid(*actual)
                ),
                None => format!("Now on {now_label} \u{b7} next start: {next}"),
            };
            self.d.label_elided(cx, line, false, tok.font.caption, dim, HAlign::Left, &text);
        }
        if plan.restart_row {
            let (row, remaining) = cut_top(rest, plan.picker_row);
            rest = remaining;
            self.draw_restart_row(cx, row);
        } else {
            self.restart_confirm = false;
        }
        rest
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
    /// Only snapshot-matched PCI identities are selectable.
    fn draw_gpu_choice_list(
        &mut self,
        cx: &mut Cx2d,
        rest: Rect,
        plan: &MonitorPlan,
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
            let selectable = self.gpu_matches_snapshot(gpu);
            let detail = if !selectable {
                "not available to Vulkan".to_string()
            } else if gpu.connected.is_empty() {
                "no display connected".to_string()
            } else {
                gpu.connected.join(", ")
            };
            items.push((Some(gpu.identity()), format!("{} \u{b7} {detail}", gpu.label()), selectable));
        }
        self.gpu_targets.clear();
        self.gpu_overflow = false;
        let mut rest = rest;
        let shown = plan.gpu_items.min(items.len());
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

    /// The arrangement picture when the plan has room, then one row per
    /// screen, left to right, then the screens outside the desktop; an
    /// unfolded mode list sits under its screen's row. Rows that do not
    /// fit scroll under the wheel; the notice line's room is already kept
    /// out of the plan. The names behind the rows are bound here, so a
    /// click acts on the screen that was drawn.
    fn draw_screens(&mut self, cx: &mut Cx2d, rest: Rect, plan: &MonitorPlan) -> Rect {
        let snapshot = self.display_snapshot.clone();
        let mut rest = rest;
        if plan.picture {
            let (picture, next) = cut_top(rest, DISPLAY_PICTURE_H);
            rest = next;
            self.draw_arrangement(cx, picture);
        }
        let (order, placed) = screen_rows(&snapshot);
        self.screen_targets = order.iter().map(|&i| snapshot.outputs[i].name.clone()).collect();
        self.screen_placed = placed;
        self.mode_targets.clear();
        self.mode_target_screen = None;
        self.mode_overflow = false;
        let picker_row = self
            .mode_picker
            .as_ref()
            .and_then(|name| self.screen_targets.iter().position(|target| target == name));
        if picker_row.is_none() && self.mode_picker.is_some() {
            // Its screen went away: nothing left to pick for.
            self.close_mode_list();
        }
        if order.is_empty() {
            self.display_overflow = false;
            self.display_scroll = 0;
            if plan.rows == 0 {
                return rest;
            }
            let (row, next) = cut_top(rest, SCREEN_LINE_H);
            self.unavailable(cx, row, if snapshot.direct { "No connected output" }
                else { "The desktop owns the outputs; the scale above still applies" });
            return next;
        }
        let rows = order.len();
        let visible = plan.rows.min(rows);
        self.display_overflow = visible > 0 && rows > visible;
        self.display_scroll = self.display_scroll.min(rows - visible);
        if let (Some(row), true) = (picker_row, visible > 0) {
            // Keep the row an unfolded list belongs to in view.
            if row < self.display_scroll {
                self.display_scroll = row;
            } else if row >= self.display_scroll + visible {
                self.display_scroll = row + 1 - visible;
            }
        }
        for row in self.display_scroll..self.display_scroll + visible {
            let output = &snapshot.outputs[order[row]];
            let (area, next) = cut_top(rest, SCREEN_ROW_H);
            rest = next;
            self.draw_screen_row(cx, area, row, output, placed);
            if picker_row == Some(row) {
                rest = self.draw_mode_list(cx, rest, plan, output);
            }
        }
        rest
    }

    /// `[icon] HDMI-A-1  NVIDIA RTX 5090 …  Main`, then
    /// `[◀] [▶] [Make main] [3840×2160 · 60 Hz ▾]`. The status is "Main",
    /// nothing for another screen of the desktop, or the renderer's own
    /// reason a screen is outside it.
    fn draw_screen_row(&mut self, cx: &mut Cx2d, area: Rect, row: usize, output: &LinuxDisplayOutput, placed: usize) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let direct = self.display_snapshot.direct;
        let (line1, line2) = cut_top(area, SCREEN_LINE_H);
        let line2 = rect(line2.pos.x, line2.pos.y, line2.size.x, SCREEN_LINE_H.min(line2.size.y));
        let in_desktop = self.in_desktop(output);
        let text = if in_desktop { fg } else { dim };
        let icon_w = 22.0;
        self.d.icon_centered(cx, Ico::Monitor, rect(line1.pos.x, line1.pos.y, icon_w, line1.size.y), 14.0, text);
        let (status, problem) = self.screen_status(output);
        let mut right = line1.pos.x + line1.size.x;
        if !status.is_empty() {
            let status_w = self.d.measure(cx, false, tok.font.body_small, &status).min(line1.size.x * 0.45);
            self.d.label_elided(cx, rect(right - status_w, line1.pos.y, status_w, line1.size.y),
                output.primary, tok.font.body_small, if problem { tok.bar.active } else { fg }, HAlign::Right, &status);
            right -= status_w + 10.0;
        }
        let name = connector_suffix(&output.name);
        let name_x = line1.pos.x + icon_w + 6.0;
        let name_w = self.d.measure(cx, output.primary, tok.font.body, &name).min((right - name_x).max(0.0));
        self.d.label_elided(cx, rect(name_x, line1.pos.y, name_w, line1.size.y),
            output.primary, tok.font.body, text, HAlign::Left, &name);
        let gpu_x = name_x + name_w + 8.0;
        let gpu = self.screen_gpu_label(output);
        self.d.label_elided(cx, rect(gpu_x, line1.pos.y, (right - gpu_x).max(0.0), line1.size.y),
            false, tok.font.caption, dim, HAlign::Left, &gpu);

        // Line two: move, main, mode.
        let inner = rect(line2.pos.x + icon_w + 6.0, line2.pos.y + 1.0,
            (line2.size.x - icon_w - 6.0).max(0.0), (line2.size.y - 2.0).max(0.0));
        let (left_ok, right_ok) = self.screen_moves(row, placed);
        let move_w = 28.0;
        let gap = 6.0;
        let left = rect(inner.pos.x, inner.pos.y, move_w, inner.size.y);
        let right_btn = rect(left.pos.x + move_w + 2.0, inner.pos.y, move_w, inner.size.y);
        self.small_button(cx, left, left_ok.then_some(Hit::ScreenMove(row, -1)), false, Some(Ico::ChevronLeft), "");
        self.small_button(cx, right_btn, right_ok.then_some(Hit::ScreenMove(row, 1)), false, Some(Ico::ChevronRight), "");
        let main_w = 92.0;
        let main = rect(right_btn.pos.x + move_w + gap, inner.pos.y, main_w, inner.size.y);
        if output.primary {
            self.small_button(cx, main, None, true, Some(Ico::Check), "Main");
        } else {
            let can = direct && output.active && in_desktop;
            self.small_button(cx, main, can.then_some(Hit::ScreenMain(row)), false, None, "Make main");
        }
        let picker_x = main.pos.x + main_w + gap;
        let picker = rect(picker_x, inner.pos.y, (inner.pos.x + inner.size.x - picker_x).max(0.0), inner.size.y);
        let choices = mode_choices(&output.modes);
        let enabled = direct && !output.modes.is_empty();
        let value = if enabled {
            mode_picker_value(&choices, output.mode_override.as_deref())
        } else {
            output.mode_override.clone().unwrap_or_else(|| "Automatic".into())
        };
        let hit = Hit::ScreenModePicker(row);
        let open = self.mode_picker.as_deref() == Some(output.name.as_str());
        let state = if !enabled { CtrlState::Disabled } else if open { CtrlState::Selected }
            else if self.hot == Some(hit) { CtrlState::Hover } else { CtrlState::Normal };
        self.d.control(cx, picker, &tok.controls, state);
        let chevron_w = if enabled { 20.0 } else { 4.0 };
        self.d.label_elided(cx, rect(picker.pos.x + 7.0, picker.pos.y, (picker.size.x - 7.0 - chevron_w).max(0.0), picker.size.y),
            false, tok.font.body_small, if enabled { fg } else { dim }, HAlign::Left, &value);
        if enabled {
            self.d.icon_centered(cx, Ico::ChevronDown,
                rect(picker.pos.x + picker.size.x - 19.0, picker.pos.y, 16.0, picker.size.y), 11.0, fg);
            self.hits.push((hit, picker));
        }
    }

    /// The unfolded mode list of `output`: "Automatic (fastest)", then
    /// every mode it offers, the current one checked. The modes behind the
    /// drawn rows, and the screen, are bound here.
    fn draw_mode_list(&mut self, cx: &mut Cx2d, rest: Rect, plan: &MonitorPlan, output: &LinuxDisplayOutput) -> Rect {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let choices = mode_choices(&output.modes);
        let current = mode_choice_current(&choices, output.mode_override.as_deref());
        let mut rest = rest;
        let shown = plan.mode_items.min(choices.len());
        if shown == 0 || output.modes.is_empty() {
            return rest;
        }
        self.mode_target_screen = Some(output.name.clone());
        let hidden = choices.len() - shown;
        self.mode_overflow = hidden > 0;
        self.mode_scroll = self.mode_scroll.min(hidden);
        let first = self.mode_scroll;
        let after = choices.len() - first - shown;
        for (row, (mode, label)) in choices.iter().skip(first).take(shown).enumerate() {
            let (item, next) = cut_top(rest, LIST_ITEM_H);
            rest = next;
            let hit = Hit::ScreenMode(row);
            let is_current = current == Some(first + row);
            self.d.cursor_surface(cx, item, &tok.controls, self.hot == Some(hit), is_current);
            let above = if row == 0 { first } else { 0 };
            let below = if row + 1 == shown { after } else { 0 };
            let right = self.list_edge_hint(cx, item, above, below);
            let x = item.pos.x + 34.0;
            self.d.label_elided(cx, rect(x, item.pos.y, (right - x).max(0.0), item.size.y),
                is_current, tok.font.body_small, fg, HAlign::Left, label);
            if is_current {
                self.d.icon_centered(cx, Ico::Check,
                    rect(item.pos.x + item.size.x - 22.0, item.pos.y, 18.0, item.size.y), 12.0, fg);
            }
            self.mode_targets.push(mode.clone());
            self.hits.push((hit, item));
        }
        rest
    }

    /// The screens of the desktop side by side, to scale by their
    /// `desktop_position` and size, the main screen highlighted, each
    /// named when its card is wide enough.
    fn draw_arrangement(&mut self, cx: &mut Cx2d, area: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let placed: Vec<&LinuxDisplayOutput> =
            self.display_snapshot.outputs.iter().filter(|o| o.desktop_position.is_some()).collect();
        let boxes: Vec<(f64, f64, f64, f64)> = placed
            .iter()
            .map(|o| {
                let (x, y) = o.desktop_position.unwrap_or((0, 0));
                let (w, h) = if o.width > 0 && o.height > 0 { (o.width, o.height) } else { (1920, 1080) };
                (x as f64, y as f64, w as f64, h as f64)
            })
            .collect();
        let inner = rect(area.pos.x + 4.0, area.pos.y + 4.0, (area.size.x - 8.0).max(0.0), (area.size.y - 8.0).max(0.0));
        let rects = arrangement_rects(&boxes, inner.size.x, inner.size.y);
        let names: Vec<String> = placed.iter().map(|o| connector_suffix(&o.name)).collect();
        for (i, (x, y, w, h)) in rects.into_iter().enumerate() {
            // A hairline gap between neighbours.
            let r = rect(inner.pos.x + x + 1.0, inner.pos.y + y + 1.0, (w - 2.0).max(0.0), (h - 2.0).max(0.0));
            let state = if placed[i].primary { CtrlState::Selected } else { CtrlState::Normal };
            let border = tok.controls.border(state);
            self.d.bordered(cx, r, tok.controls.fill(state), border, border, 0.0, 1.0);
            let label_w = self.d.measure(cx, false, tok.font.caption, &names[i]);
            if label_w + 8.0 <= r.size.x && r.size.y >= 16.0 {
                self.d.label(cx, r, false, tok.font.caption, if placed[i].primary { fg } else { dim }, HAlign::Center, &names[i]);
            }
        }
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
        let mut text = format!("panel={:?} output_volume={:?} input_volume={:?} brightness={:?} battery={:?} charging={:?} notice={}",
            self.open, s.audio.default_output().map(|d| d.percent), s.audio.default_input().map(|d| d.percent),
            s.brightness.percent(), s.power.percent, s.power.status, self.system_notice);
        for (direction, devices) in [("output", &s.audio.outputs), ("input", &s.audio.inputs)] {
            for device in devices {
                text.push_str(&format!("\n{direction}: {} default={} muted={}", device.label, device.is_default, device.muted));
            }
        }
        // The Display dropdown's scale and screens: the actual window
        // scale, the held preview, the saved settings, and the renderer's
        // snapshot as shown — with the screen rows, the mode list and the
        // render-on choice as drawn.
        let d = &self.display_snapshot;
        text.push_str(&format!(
            "\ndpi={:.2} dpi_hundredths={} dpi_preview={:?} dpi_saved={:?} display_settings_loaded={} display_direct={} display_outputs={} display_status={} display_scroll={} display_overflow={}",
            self.current_dpi, self.dpi_hundredths(), self.dpi_preview, s.dpi_scale, s.display_settings_loaded,
            d.direct, d.outputs.len(), self.display_status(), self.display_scroll, self.display_overflow
        ));
        text.push_str(&format!(
            "\nmain={:?} main_pending={:?} main_saved={:?} screen_rows={:?} screens_placed={} screens_span_gpus={} mode_picker={:?} mode_scroll={} mode_overflow={} mode_screen={:?} mode_rows={:?}",
            d.outputs.iter().find(|o| o.primary).map(|o| o.name.as_str()),
            self.display_source_pending.as_ref().map(|(name, _)| name.as_str()),
            s.display_layout.main_name(d), self.screen_targets, self.screen_placed, screens_span_gpus(d),
            self.mode_picker, self.mode_scroll, self.mode_overflow, self.mode_target_screen, self.mode_targets
        ));
        let (order, placed) = screen_rows(d);
        for (row, &index) in order.iter().enumerate() {
            let output = &d.outputs[index];
            let (status, _) = self.screen_status(output);
            let (left, right) = self.screen_moves(row, placed);
            let choices = mode_choices(&output.modes);
            text.push_str(&format!(
                "\nscreen: {row} {} gpu={} status={:?} position={:?} main={} move_left={} move_right={} make_main={} mode={:?} mode_override={:?} modes={}",
                output.name, self.screen_gpu_label(output), status, output.desktop_position, output.primary, left, right,
                d.direct && output.active && self.in_desktop(output) && !output.primary,
                if output.modes.is_empty() { output.mode_override.clone().unwrap_or_else(|| "Automatic".into()) }
                    else { mode_picker_value(&choices, output.mode_override.as_deref()) },
                output.mode_override, output.modes.len()
            ));
        }
        text.push_str(&format!(
            "\nrender_on_saved={:?} render_on_label={:?} render_on_next={:?} render_on_now={:?} render_on_differs={} gpu_env={:?} restart_row={} restart_confirm={} restart_waiting={}",
            self.render_on_saved, self.render_on_saved_label(), self.render_on_next_label(), self.gpu_label(self.gpu_now()),
            self.render_on_differs(), s.gpu_env, self.restart_row_shown(), self.restart_confirm, self.restart_waiting
        ));
        text.push_str(&format!(
            "\ngpu_now={:?} gpu_busy={} gpu_picker={} app_gpu_picker={} gpu_picker_enabled={} gpu_scroll={} gpu_overflow={} gpu_rows={:?} gpu_target_client={:?} gpu_app={:?}",
            self.gpu_now().map(|i| s.gpus[i].card.as_str()),
            self.gpu_busy, self.gpu_picker, self.app_gpu_picker, self.gpu_picker_enabled(),
            self.gpu_scroll, self.gpu_overflow, self.gpu_targets, self.gpu_target_client,
            self.gpu_app.as_ref().map(|(id, label, policy, actual)| (id, label.as_str(), policy.is_some(), gpu_uuid_hex(actual)))
        ));
        text.push_str(&format!(
            "\ngpu_renderer_uuid={:?} gpu_renderer_generation={} gpu_display_uuid={:?}",
            self.gpu_snapshot.renderer.as_ref().map(|r| gpu_uuid_hex(&r.uuid)),
            self.gpu_snapshot.renderer_generation,
            self.gpu_snapshot.display.as_ref().map(|d| gpu_uuid_hex(&d.uuid)),
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

/// `GpuInfo.vendor` / `device` are sysfs hex without `0x` (lowercase).
fn parse_pci_id(s: &str) -> Option<u32> {
    u32::from_str_radix(s, 16).ok()
}

fn gpu_vendor_device_match(gpu: &GpuInfo, device: &LinuxGpuDevice) -> bool {
    parse_pci_id(&gpu.vendor) == Some(device.vendor_id)
        && parse_pci_id(&gpu.device) == Some(device.device_id)
}

fn gpu_uuid_hex(uuid: &[u8; 16]) -> String {
    uuid.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A mode as the renderer's mode syntax takes it: `3840x2160@60`, the
/// refresh rounded to whole hertz the way the renderer matches it; just
/// `3840x2160` when the refresh is unknown.
fn mode_string(width: u32, height: u32, hz: f64) -> String {
    if hz.is_finite() && hz > 0.0 {
        format!("{width}x{height}@{}", hz.round() as u32)
    } else {
        format!("{width}x{height}")
    }
}

/// `3840×2160 · 60 Hz`, with the refresh to two places only when it is
/// not a whole number, and without it when unknown.
fn mode_label(width: u32, height: u32, hz: f64) -> String {
    let refresh = if !hz.is_finite() || hz <= 0.0 {
        String::new()
    } else if (hz - hz.round()).abs() < 0.05 {
        format!(" \u{b7} {:.0} Hz", hz)
    } else {
        format!(" \u{b7} {:.2} Hz", hz)
    };
    format!("{width}\u{d7}{height}{refresh}")
}

/// A screen's mode list: "Automatic (fastest)" (no override), then each
/// mode it offers as `(mode string, label)`, in the renderer's order
/// (largest and fastest first). Modes that come to the same mode string
/// (59.94 and 60 Hz) are one choice, the first; the renderer picks the
/// fastest of them anyway.
fn mode_choices(modes: &[(u32, u32, f64)]) -> Vec<(Option<String>, String)> {
    let mut choices: Vec<(Option<String>, String)> = vec![(None, MODE_AUTOMATIC.to_string())];
    for &(width, height, hz) in modes {
        let mode = mode_string(width, height, hz);
        if !choices.iter().any(|(m, _)| m.as_deref() == Some(mode.as_str())) {
            choices.push((Some(mode), mode_label(width, height, hz)));
        }
    }
    choices
}

/// The choice `mode_override` selects: Automatic without one; the same
/// mode string (`-Hz` read as `@Hz`); for a bare `WxH` that resolution's
/// first (fastest) mode. `None` when the screen does not offer it.
fn mode_choice_current(choices: &[(Option<String>, String)], mode_override: Option<&str>) -> Option<usize> {
    let Some(wanted) = mode_override.map(str::trim).filter(|m| !m.is_empty()) else {
        return Some(0);
    };
    let wanted = match wanted.split_once('-') {
        Some((resolution, hz)) => format!("{resolution}@{hz}"),
        None => wanted.to_string(),
    };
    choices.iter().position(|(mode, _)| match mode.as_deref() {
        None => false,
        Some(mode) if wanted.contains('@') => mode == wanted,
        Some(mode) => mode.split('@').next() == Some(wanted.as_str()),
    })
}

/// The mode picker's value: the current choice's label, the override as
/// written when the screen does not offer it, or "Automatic" for a screen
/// whose modes are not known yet.
fn mode_picker_value(choices: &[(Option<String>, String)], mode_override: Option<&str>) -> String {
    if choices.len() <= 1 {
        return mode_override.map(str::to_string).unwrap_or_else(|| "Automatic".into());
    }
    match mode_choice_current(choices, mode_override) {
        Some(index) => choices[index].1.clone(),
        None => mode_override.unwrap_or("Automatic").to_string(),
    }
}

/// Whether the saved render-on choice (`saved`, `None` for Auto, an
/// identity `<pci> <vendor>:<device>` otherwise) is not what renders now:
/// `running` is the renderer's identity, `running_is_display` whether it
/// is the display GPU (what Auto picks). Unknown means not different, so
/// a restart is never offered on a guess.
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

/// `boxes` (`x, y, w, h` in the desktop's pixels) scaled as one picture
/// into `width` × `height`, centred: the same scale for every box, so the
/// arrangement keeps its proportions.
fn arrangement_rects(boxes: &[(f64, f64, f64, f64)], width: f64, height: f64) -> Vec<(f64, f64, f64, f64)> {
    if boxes.is_empty() {
        return Vec::new();
    }
    let min_x = boxes.iter().map(|b| b.0).fold(f64::INFINITY, f64::min);
    let min_y = boxes.iter().map(|b| b.1).fold(f64::INFINITY, f64::min);
    let max_x = boxes.iter().map(|b| b.0 + b.2).fold(f64::NEG_INFINITY, f64::max);
    let max_y = boxes.iter().map(|b| b.1 + b.3).fold(f64::NEG_INFINITY, f64::max);
    let total_w = (max_x - min_x).max(1.0);
    let total_h = (max_y - min_y).max(1.0);
    let scale = (width / total_w).min(height / total_h);
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 0.0 };
    let offset_x = (width - total_w * scale) * 0.5;
    let offset_y = (height - total_h * scale) * 0.5;
    boxes
        .iter()
        .map(|&(x, y, w, h)| ((x - min_x) * scale + offset_x, (y - min_y) * scale + offset_y, w * scale, h * scale))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_strings_and_labels() {
        assert_eq!(mode_string(3840, 2160, 60.0), "3840x2160@60");
        assert_eq!(mode_string(3840, 2160, 59.94), "3840x2160@60");
        assert_eq!(mode_string(1920, 1080, 0.0), "1920x1080");
        assert_eq!(mode_label(3840, 2160, 30.0), "3840\u{d7}2160 \u{b7} 30 Hz");
        assert_eq!(mode_label(2560, 1440, 143.86), "2560\u{d7}1440 \u{b7} 143.86 Hz");
        assert_eq!(mode_label(1920, 1080, f64::NAN), "1920\u{d7}1080");
    }

    #[test]
    fn mode_choices_start_automatic_and_drop_duplicate_strings() {
        let choices = mode_choices(&[(3840, 2160, 60.0), (3840, 2160, 59.94), (3840, 2160, 30.0), (1920, 1080, 60.0)]);
        let strings: Vec<Option<&str>> = choices.iter().map(|(mode, _)| mode.as_deref()).collect();
        assert_eq!(strings, [None, Some("3840x2160@60"), Some("3840x2160@30"), Some("1920x1080@60")]);
        assert_eq!(choices[0].1, "Automatic (fastest)");
        assert_eq!(choices[2].1, "3840\u{d7}2160 \u{b7} 30 Hz");
        assert_eq!(mode_choices(&[]).len(), 1);
    }

    #[test]
    fn current_mode_choice_follows_the_override() {
        let choices = mode_choices(&[(3840, 2160, 60.0), (3840, 2160, 30.0), (1920, 1080, 60.0)]);
        assert_eq!(mode_choice_current(&choices, None), Some(0));
        assert_eq!(mode_choice_current(&choices, Some("3840x2160@30")), Some(2));
        assert_eq!(mode_choice_current(&choices, Some("3840x2160-30")), Some(2));
        // A bare resolution is that resolution's fastest mode.
        assert_eq!(mode_choice_current(&choices, Some("1920x1080")), Some(3));
        assert_eq!(mode_choice_current(&choices, Some("1280x720")), None);
        assert_eq!(mode_picker_value(&choices, Some("3840x2160@30")), "3840\u{d7}2160 \u{b7} 30 Hz");
        assert_eq!(mode_picker_value(&choices, Some("1280x720")), "1280x720");
        assert_eq!(mode_picker_value(&choices, None), "Automatic (fastest)");
        // A peer's row before it has modes: the override, or Automatic.
        let none = mode_choices(&[]);
        assert_eq!(mode_picker_value(&none, Some("3840x2160@30")), "3840x2160@30");
        assert_eq!(mode_picker_value(&none, None), "Automatic");
    }

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

    fn full_plan() -> MonitorPlan {
        MonitorPlan {
            hero: HERO_H,
            sep_first: SEP_FIRST_H,
            slider: SLIDER_H,
            ends: ENDS_H,
            picker_row: PICKER_ROW_H,
            backlight_caption: true,
            picture: true,
            gpu_status: true,
            gpu_items: 0,
            app_gpu_row: true,
            restart_row: true,
            rows: 3,
            row_h: SCREEN_ROW_H,
            mode_items: 0,
            pointer_row: true,
        }
    }

    /// Everything droppable dropped: what the plan may not shrink below.
    fn floor_reached(p: &MonitorPlan) -> bool {
        !p.picture && !p.backlight_caption && !p.gpu_status && p.hero == 0.0
            && p.rows <= 1 && p.mode_items <= 1 && p.gpu_items <= 1
    }

    #[test]
    fn monitor_plan_degrades_without_overflow() {
        for (mode_items, gpu_items) in [(0, 0), (5, 0), (0, 4)] {
            let full = MonitorPlan { mode_items, gpu_items, ..full_plan() };
            for notice in [false, true] {
                assert_eq!(fit_monitor_plan(full, notice, plan_height(&full, notice)), full);
                for step in 0..=120 {
                    let avail = step as f64 * 10.0;
                    let plan = fit_monitor_plan(full, notice, avail);
                    assert!(
                        plan_height(&plan, notice) <= avail || floor_reached(&plan),
                        "overflow at {avail}: {plan:?}"
                    );
                    assert!(plan.rows <= full.rows && plan.mode_items <= full.mode_items && plan.gpu_items <= full.gpu_items);
                    // An unfolded mode list keeps the row it belongs to.
                    if mode_items > 0 {
                        assert!(plan.rows >= 1 && plan.mode_items >= 1);
                    }
                    // The controls are never dropped.
                    assert!(plan.restart_row && plan.app_gpu_row && plan.pointer_row);
                }
            }
        }
        // A 1080p ×3 screen (360 pt) still shows a screen row.
        let plan = fit_monitor_plan(MonitorPlan { restart_row: false, app_gpu_row: false, ..full_plan() }, false, 360.0);
        assert!(plan.rows >= 1, "{plan:?}");
    }

    #[test]
    fn arrangement_is_to_scale_and_centred() {
        // Two 4K screens side by side into a 300 x 60 area.
        let rects = arrangement_rects(&[(0.0, 0.0, 3840.0, 2160.0), (3840.0, 0.0, 3840.0, 2160.0)], 300.0, 60.0);
        assert_eq!(rects.len(), 2);
        let (x0, y0, w0, h0) = rects[0];
        let (x1, _, w1, h1) = rects[1];
        assert!((w0 - w1).abs() < 1e-9 && (h0 - h1).abs() < 1e-9);
        assert!((w0 / h0 - 16.0 / 9.0).abs() < 1e-6);
        assert!((x1 - (x0 + w0)).abs() < 1e-9);
        assert!(h0 <= 60.0 && y0 >= 0.0);
        // Centred horizontally.
        assert!((x0 - (300.0 - (x1 + w1))).abs() < 1e-6);
        assert!(arrangement_rects(&[], 300.0, 60.0).is_empty());
        assert_eq!(arrangement_rects(&[(0.0, 0.0, 0.0, 0.0)], 300.0, 60.0).len(), 1);
    }
}
