//! Linux status dropdowns, drawn from the worker's observed device state.
//!
//! The Display dropdown adds what the other platforms do not have:
//!
//! * DPI SCALE — 1.00×–3.00× in hundredths; previewed while held, applied
//!   on release through the platform's `dpi_override`, persisted by the
//!   system worker.
//! * DISPLAYS — what the renderer reports: the connected outputs with
//!   their native mode, and the clone-only arrangement: one framebuffer,
//!   rendered at the native pixels of the "Optimize for" output, shown on
//!   every driven output (letterboxed where the aspect differs). The
//!   "Optimize for" row picks that source among the driven outputs, the
//!   way a Mac picks which display a mirrored set is optimised for.
//!   Nothing here offers an extend mode, a per-display scale, or an output
//!   the renderer does not drive.
//! * Compositor — which GPU is compositing now is the renderer's own
//!   identity (`linux_gpu_snapshot`), not the GPU of the physical output.
//!   The row picks a live Vulkan GPU (Auto is the display GPU). A GPU
//!   does not need a connected output. The same list can set the focused
//!   app's GPU, or follow the compositor, for this session only.
//! * Mouse & touchpad — a row (only while the WM drives the outputs) that
//!   opens a subview with mouse and touchpad speed sliders. The speeds are
//!   the process's actual values; a drag applies immediately.
//!
//! The card is bounded by the screen. At 1080p ×3 (360 logical points in
//! all) the sliders and the picker rows still fit: the clone picture,
//! the backlight caption, the GPU status line, then the hero's and the
//! sliders' slack give way first, then the output rows down to one, which
//! scroll, and an unfolded list shrinks to what fits and scrolls too. The
//! Mouse & touchpad row is kept with the other picker rows.
use super::*;
use super::super::ui::HAlign;
use crate::linux_controls::ControlAction;
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
/// The "Optimize for" and "Compositor" rows.
const PICKER_ROW_H: f64 = 30.0;
const PICKER_ROW_COMPACT_H: f64 = 26.0;
const SOURCE_ITEM_H: f64 = 26.0;
const SOURCE_ITEMS_MAX: usize = 4;
const GPU_ITEMS_MAX: usize = 4;
/// "Now on …" under the GPU picker rows.
const GPU_STATUS_H: f64 = 18.0;
const DISPLAY_ROW_H: f64 = 26.0;
/// Rows the card asks for before the list scrolls; the screen may bound it
/// further.
const DISPLAY_ROWS_MAX: usize = 5;
/// The clone picture, drawn only when it fits above at least one row.
const DISPLAY_PICTURE_H: f64 = 60.0;
const NOTICE_H: f64 = 18.0;
/// Caption under the pointer-speed sliders.
const POINTER_CAPTION_H: f64 = 18.0;

/// What the Display dropdown draws at the height it actually has.
#[derive(Clone, Copy, Debug)]
struct MonitorPlan {
    hero: f64,
    sep_first: f64,
    slider: f64,
    /// The "1.00 / 3.00" endpoints row under the DPI slider.
    ends: f64,
    picker_row: f64,
    backlight_caption: bool,
    picture: bool,
    gpu_status: bool,
    /// Rows of the unfolded "Optimize for" list.
    source_items: usize,
    /// Rows of the one unfolded GPU list (compositor or app).
    gpu_items: usize,
    /// The focused-app GPU picker row.
    app_gpu_row: bool,
    /// Output rows shown; the rest scroll.
    rows: usize,
    /// The "Mouse & touchpad" row, only while the WM drives the outputs.
    pointer_row: bool,
}

impl MonitorPlan {
    fn list_open(&self) -> bool {
        self.source_items > 0 || self.gpu_items > 0
    }
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
            Hit::SourcePicker => {
                self.source_picker = !self.source_picker;
                // Unfolded with the current source as the first shown row,
                // so it is in view however few rows the screen allows.
                self.source_scroll = if self.source_picker {
                    self.eligible_sources().iter().position(|o| o.primary).unwrap_or(0)
                } else {
                    0
                };
                self.source_targets.clear();
                // One list at a time.
                self.close_gpu_lists();
            }
            Hit::Source(row) => {
                // The name that was drawn on that row — exactly as the
                // renderer lists it — and only while the current inventory
                // still drives it. The renderer confirms the choice (or
                // not) through the snapshot's `primary`.
                let name = self
                    .source_targets
                    .get(row)
                    .cloned()
                    .filter(|name| self.eligible_sources().iter().any(|o| o.name == *name));
                self.source_picker = false;
                self.source_scroll = 0;
                self.source_targets.clear();
                if let Some(name) = name {
                    self.system_action(cx, ControlAction::DisplaySource(name));
                }
            }
            Hit::GpuPicker => {
                let open = !self.gpu_picker;
                self.close_gpu_lists();
                self.gpu_picker = open;
                self.gpu_scroll = if open { self.gpu_compositor_current_row() } else { 0 };
                self.source_picker = false;
                self.source_scroll = 0;
                self.source_targets.clear();
            }
            Hit::AppGpuPicker => {
                let open = !self.app_gpu_picker;
                let client = self.gpu_app.as_ref().map(|(id, _, _, _)| *id);
                self.close_gpu_lists();
                self.app_gpu_picker = open;
                self.gpu_target_client = if open { client } else { None };
                self.gpu_scroll = if open { self.gpu_app_current_row() } else { 0 };
                self.source_picker = false;
                self.source_scroll = 0;
                self.source_targets.clear();
            }
            Hit::PointerSettings => {
                self.pointer_settings = !self.pointer_settings;
                self.dragging = None;
                self.dpi_preview = None;
                self.source_picker = false;
                self.source_scroll = 0;
                self.source_targets.clear();
                self.close_gpu_lists();
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

    /// The outputs the person may pick as the source: the ones the
    /// renderer drives now. Outputs it cannot drive (another GPU, a
    /// failure) stay in the list below with their status and are not
    /// offered. Nothing when the desktop owns the outputs.
    fn eligible_sources(&self) -> Vec<&LinuxDisplayOutput> {
        if !self.display_snapshot.direct {
            return Vec::new();
        }
        self.display_snapshot.outputs.iter().filter(|o| o.active).collect()
    }

    /// The DISPLAYS header's reading of the renderer's snapshot.
    fn display_status(&self) -> String {
        let s = &self.display_snapshot;
        if !s.direct {
            return "Managed by the desktop".into();
        }
        let active = s.outputs.iter().filter(|o| o.active).count();
        match (s.outputs.len(), active) {
            (0, _) => "No outputs".into(),
            (_, 0) => "No active output".into(),
            (_, 1) => "One display".into(),
            (_, n) => format!("Clone displays · {n}"),
        }
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

    /// Auto (row 0) is current when the compositor is the display GPU.
    fn gpu_compositor_current_row(&self) -> usize {
        let renderer = self.gpu_snapshot.renderer.as_ref();
        let display = self.gpu_snapshot.display.as_ref();
        if renderer.is_some() && renderer.map(|g| g.uuid) == display.map(|g| g.uuid) {
            return 0;
        }
        self.gpu_now().map(|i| i + 1).unwrap_or(usize::MAX)
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

    fn plan_height(&self, p: &MonitorPlan) -> f64 {
        let notice = if self.system_notice.is_empty() { 0.0 } else { NOTICE_H };
        p.hero + p.sep_first + HEADER_H + p.slider
            + if p.backlight_caption { BACKLIGHT_CAPTION_H } else { 0.0 }
            + SEP_H + HEADER_H + p.slider + p.ends
            + if p.pointer_row { SEP_H + p.picker_row } else { 0.0 }
            + SEP_H + HEADER_H
            + p.picker_row + p.source_items as f64 * SOURCE_ITEM_H
            + p.picker_row + if p.app_gpu_row { p.picker_row } else { 0.0 }
            + p.gpu_items as f64 * SOURCE_ITEM_H
            + if p.gpu_status { GPU_STATUS_H } else { 0.0 }
            + if p.picture { DISPLAY_PICTURE_H } else { 0.0 }
            + p.rows as f64 * DISPLAY_ROW_H
            + notice
    }

    /// Everything, at full size: what the card asks the screen for.
    fn full_plan(&self) -> MonitorPlan {
        MonitorPlan {
            hero: HERO_H,
            sep_first: SEP_FIRST_H,
            slider: SLIDER_H,
            ends: ENDS_H,
            picker_row: PICKER_ROW_H,
            backlight_caption: true,
            picture: self.display_snapshot.outputs.iter().any(|o| o.active),
            gpu_status: self.display_snapshot.direct && (!self.system.gpus.is_empty() || self.gpu_snapshot.renderer.is_some()),
            source_items: if self.source_picker { self.eligible_sources().len().min(SOURCE_ITEMS_MAX) } else { 0 },
            gpu_items: if self.gpu_list_open() { (1 + self.system.gpus.len()).min(GPU_ITEMS_MAX) } else { 0 },
            app_gpu_row: self.gpu_app.is_some(),
            rows: self.display_snapshot.outputs.len().clamp(1, DISPLAY_ROWS_MAX),
            pointer_row: self.display_snapshot.direct,
        }
    }

    /// The plan for the height the card really has (the screen bounds
    /// it). The clone picture goes first, then the backlight caption, then
    /// the GPU status line, then the hero, the endpoints and the sliders'
    /// slack, then the output rows give way down to one — or to none while
    /// a list is unfolded, which then shrinks to what fits — and, last of
    /// all, the one remaining row. The sliders, source and compositor
    /// picker rows, the App GPU row (when a native child is focused), and
    /// the Mouse & touchpad row (when the WM drives the outputs) are never
    /// dropped.
    fn monitor_plan(&self, avail: f64) -> MonitorPlan {
        let mut plan = self.full_plan();
        if self.plan_height(&plan) > avail {
            plan.picture = false;
        }
        if self.plan_height(&plan) > avail {
            plan.backlight_caption = false;
        }
        if self.plan_height(&plan) > avail {
            plan.gpu_status = false;
        }
        if self.plan_height(&plan) > avail {
            plan.hero = HERO_COMPACT_H;
            plan.sep_first = SEP_H;
            plan.ends = 0.0;
            plan.slider = SLIDER_COMPACT_H;
            plan.picker_row = PICKER_ROW_COMPACT_H;
        }
        if self.plan_height(&plan) > avail {
            let rows_wanted = plan.rows;
            plan.rows = 0;
            let fixed = self.plan_height(&plan);
            let fit = ((avail - fixed) / DISPLAY_ROW_H).floor().max(0.0) as usize;
            plan.rows = fit.min(rows_wanted).max(if plan.list_open() { 0 } else { 1 });
        }
        if plan.source_items > 1 && self.plan_height(&plan) > avail {
            let items_wanted = plan.source_items;
            plan.source_items = 0;
            let fixed = self.plan_height(&plan);
            let fit = ((avail - fixed) / SOURCE_ITEM_H).floor().max(0.0) as usize;
            plan.source_items = fit.clamp(1, items_wanted);
        }
        if plan.gpu_items > 1 && self.plan_height(&plan) > avail {
            let items_wanted = plan.gpu_items;
            plan.gpu_items = 0;
            let fixed = self.plan_height(&plan);
            let fit = ((avail - fixed) / SOURCE_ITEM_H).floor().max(0.0) as usize;
            plan.gpu_items = fit.clamp(1, items_wanted);
        }
        if plan.rows == 1 && self.plan_height(&plan) > avail {
            // The DISPLAYS header still counts the outputs; the pickers and
            // the sliders matter more than the one row that would clip.
            plan.rows = 0;
        }
        if self.plan_height(&plan) > avail {
            // On short screens keep the controls and an unfolded choice
            // reachable; their section labels can stand in for the hero.
            plan.hero = 0.0;
            plan.sep_first = 0.0;
        }
        plan
    }

    /// The card height the Display dropdown asks for; `card_rect` bounds it
    /// to the screen and `monitor_plan` fits the content to what is left.
    pub(super) fn monitor_height(&self) -> f64 {
        if self.pointer_settings {
            return self.pointer_settings_height();
        }
        let plan = self.full_plan();
        self.plan_height(&plan)
    }

    fn pointer_settings_height(&self) -> f64 {
        let notice = if self.system_notice.is_empty() { 0.0 } else { NOTICE_H };
        HERO_H + SEP_H + PICKER_ROW_H
            + SEP_H + HEADER_H + SLIDER_H + ENDS_H
            + SEP_H + HEADER_H + SLIDER_H + ENDS_H
            + POINTER_CAPTION_H
            + notice
    }

    /// The wheel over an overflowing display list; the draw clamps.
    pub(super) fn display_scroll_by(&mut self, cx: &mut Cx, dy: f64) {
        self.display_scroll = if dy > 0.0 { self.display_scroll + 1 } else { self.display_scroll.saturating_sub(1) };
        self.redraw(cx);
    }

    /// The wheel over the unfolded "Optimize for" list; the draw clamps.
    pub(super) fn source_scroll_by(&mut self, cx: &mut Cx, dy: f64) {
        self.source_scroll = if dy > 0.0 { self.source_scroll + 1 } else { self.source_scroll.saturating_sub(1) };
        self.redraw(cx);
    }

    /// The wheel over the unfolded "Compositor" list; the draw clamps.
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

        // ---- the outputs: the source, the GPU, the picture, the inventory.
        let (sep, next) = cut_top(rest, SEP_H); rest = next; self.d.separator(cx, sep, fg, 0.12);
        let (header, next) = cut_top(rest, HEADER_H); rest = next;
        let status = self.display_status();
        self.section_header(cx, header, "DISPLAYS", &status);
        let rest = self.draw_source_picker(cx, rest, &plan);
        let rest = self.draw_gpu_picker(cx, rest, &plan);
        let rest = self.draw_displays(cx, rest, &plan);
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

    /// `Optimize for   DP-2 ▾`: the output whose native pixels the shared
    /// framebuffer is rendered at; every other driven output shows that
    /// same frame, letterboxed to its aspect. The value is the renderer's
    /// observed primary, or the name asked for (marked) until the renderer
    /// confirms it. Disabled, with the reason, when the desktop owns the
    /// outputs or nothing is driven.
    fn draw_source_picker(&mut self, cx: &mut Cx2d, rest: Rect, plan: &MonitorPlan) -> Rect {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let snapshot = self.display_snapshot.clone();
        let eligible: Vec<(String, String, bool)> = self
            .eligible_sources()
            .into_iter()
            .map(|o| (o.name.clone(), display_mode(o), o.primary))
            .collect();
        let enabled = !eligible.is_empty();
        let (row, mut rest) = cut_top(rest, plan.picker_row);
        let pending = self.display_source_pending.as_ref().map(|(name, _)| name.clone());
        let (value, color) = if !snapshot.direct {
            ("Managed by the desktop".to_string(), dim)
        } else if let Some(name) = pending {
            (format!("{name}\u{2026}"), tok.bar.active)
        } else if let Some(primary) = snapshot.outputs.iter().find(|o| o.primary) {
            (primary.name.clone(), if enabled { fg } else { dim })
        } else {
            ("No active output".to_string(), dim)
        };
        self.picker_row(cx, row, Hit::SourcePicker, self.source_picker, enabled, "Optimize for", &value, color);
        self.source_targets.clear();
        self.source_overflow = false;
        if self.source_picker && enabled {
            // As many rows as the plan left room for, scrolled so every
            // driven output can be reached even when that is one row. The
            // offset is clamped here, so a changed inventory never leaves
            // the list past its end.
            let shown = plan.source_items.min(eligible.len());
            let hidden = eligible.len() - shown;
            self.source_overflow = hidden > 0;
            self.source_scroll = self.source_scroll.min(hidden);
            let first = self.source_scroll;
            let after = eligible.len() - first - shown;
            for (row, (name, mode, primary)) in eligible.iter().skip(first).take(shown).enumerate() {
                let (item, next) = cut_top(rest, SOURCE_ITEM_H); rest = next;
                let hit = Hit::Source(row);
                self.d.cursor_surface(cx, item, &tok.controls, self.hot == Some(hit), *primary);
                let above = if row == 0 { first } else { 0 };
                let below = if row + 1 == shown { after } else { 0 };
                let right = self.list_edge_hint(cx, item, above, below);
                let text = if mode.is_empty() { name.clone() } else { format!("{name} \u{b7} {mode}") };
                self.d.label_elided(cx, rect(item.pos.x + 8.0, item.pos.y, (right - item.pos.x - 8.0).max(0.0), item.size.y),
                    *primary, tok.font.body_small, fg, HAlign::Left, &text);
                if *primary {
                    self.d.icon_centered(cx, Ico::Check,
                        rect(item.pos.x + item.size.x - 22.0, item.pos.y, 18.0, item.size.y), 12.0, fg);
                }
                self.source_targets.push(name.clone());
                self.hits.push((hit, item));
            }
        }
        rest
    }

    /// Compositor row (actual renderer), optional app row, and the one
    /// unfolded GPU list immediately under the row that owns it.
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
            (now_label.clone(), if enabled { fg } else { dim })
        };
        self.picker_row(cx, row, Hit::GpuPicker, self.gpu_picker, enabled, "Compositor", &value, color);
        if self.gpu_picker {
            rest = self.draw_gpu_choice_list(cx, rest, plan, "Auto", self.gpu_compositor_current_row(), enabled);
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
            let text = match &self.gpu_app {
                Some((.., actual)) => {
                    format!("Now on {now_label} \u{b7} app: {}", self.gpu_label_for_uuid(*actual))
                }
                None => format!("Now on {now_label}"),
            };
            self.d.label_elided(cx, line, false, tok.font.caption, dim, HAlign::Left, &text);
        }
        rest
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
            let (item, remaining) = cut_top(rest, SOURCE_ITEM_H);
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

    /// The clone picture when the plan has room, then one row per
    /// connected output. The rows that do not fit scroll under the wheel;
    /// the notice line's room is already kept out of the plan.
    fn draw_displays(&mut self, cx: &mut Cx2d, rest: Rect, plan: &MonitorPlan) -> Rect {
        let snapshot = self.display_snapshot.clone();
        let mut rest = rest;
        if plan.picture {
            let active: Vec<&LinuxDisplayOutput> = snapshot.outputs.iter().filter(|o| o.active).collect();
            if !active.is_empty() {
                let (picture, next) = cut_top(rest, DISPLAY_PICTURE_H);
                rest = next;
                self.draw_clone_picture(cx, picture, &active);
            }
        }
        if snapshot.outputs.is_empty() {
            self.display_overflow = false;
            self.display_scroll = 0;
            if plan.rows == 0 {
                return rest;
            }
            let (row, next) = cut_top(rest, DISPLAY_ROW_H);
            self.unavailable(cx, row, if snapshot.direct { "No connected output" }
                else { "The desktop owns the outputs; the scale above still applies" });
            return next;
        }
        let rows = snapshot.outputs.len();
        let visible = plan.rows.min(rows);
        self.display_overflow = visible > 0 && rows > visible;
        self.display_scroll = self.display_scroll.min(rows - visible);
        for output in snapshot.outputs.iter().skip(self.display_scroll).take(visible) {
            let (row, next) = cut_top(rest, DISPLAY_ROW_H);
            rest = next;
            self.draw_display_row(cx, row, output);
        }
        rest
    }

    /// `[icon] name … 3840×2160 · 240 Hz  Primary` — the mode is the native
    /// one the renderer selected; the trailing word is Primary (the source
    /// the frame is rendered for), Cloned, or the renderer's own reason an
    /// output is not driven.
    fn draw_display_row(&mut self, cx: &mut Cx2d, row: Rect, output: &LinuxDisplayOutput) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let (status, color) = if output.active {
            (if output.primary { "Primary" } else { "Cloned" }.to_string(), fg)
        } else if output.status.is_empty() {
            ("Inactive".to_string(), dim)
        } else {
            (output.status.clone(), tok.bar.active)
        };
        let text = if output.active { fg } else { dim };
        let icon_w = 22.0;
        self.d.icon_centered(cx, Ico::Monitor, rect(row.pos.x, row.pos.y, icon_w, row.size.y), 14.0, text);
        let mut right = row.pos.x + row.size.x;
        let status_w = self.d.measure(cx, false, tok.font.body_small, &status).min(row.size.x * 0.4);
        self.d.label_elided(cx, rect(right - status_w, row.pos.y, status_w, row.size.y),
            false, tok.font.body_small, color, HAlign::Right, &status);
        right -= status_w + 10.0;
        let mode = display_mode(output);
        if !mode.is_empty() {
            let mode_w = self.d.measure(cx, false, tok.font.caption, &mode).min(row.size.x * 0.4);
            self.d.label_elided(cx, rect(right - mode_w, row.pos.y, mode_w, row.size.y),
                false, tok.font.caption, dim, HAlign::Right, &mode);
            right -= mode_w + 10.0;
        }
        let name_x = row.pos.x + icon_w + 6.0;
        self.d.label_elided(cx, rect(name_x, row.pos.y, (right - name_x).max(0.0), row.size.y),
            output.primary, tok.font.body, text, HAlign::Left, &output.name);
    }

    /// The clone arrangement: every driven output as a card at its own
    /// aspect, the source (primary) in front and the clones stacked behind
    /// it — one logical desktop, so they overlap rather than sit side by
    /// side — with a caption saying so.
    fn draw_clone_picture(&mut self, cx: &mut Cx2d, area: Rect, active: &[&LinuxDisplayOutput]) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let primary = active.iter().position(|o| o.primary).unwrap_or(0);
        let behind = active.len().saturating_sub(1);
        let step = 8.0;
        let rise = 5.0;
        let max_h = (area.size.y - 8.0 - behind as f64 * rise).max(20.0);
        let max_w = 96.0;
        let card_size = |o: &LinuxDisplayOutput| {
            let aspect = if o.width > 0 && o.height > 0 { o.width as f64 / o.height as f64 } else { 16.0 / 9.0 };
            let (mut w, mut h) = (max_h * aspect, max_h);
            if w > max_w {
                w = max_w;
                h = w / aspect;
            }
            (w.floor().max(12.0), h.floor().max(8.0))
        };
        let (pw, ph) = card_size(active[primary]);
        let x0 = area.pos.x + 4.0;
        let bottom = area.pos.y + area.size.y - 4.0;
        // Back to front: the farthest clone first, the source last.
        let order: Vec<usize> = (0..active.len()).filter(|i| *i != primary).chain(std::iter::once(primary)).collect();
        for (depth, index) in order.iter().enumerate() {
            let (w, h) = if *index == primary { (pw, ph) } else { card_size(active[*index]) };
            let k = (behind - depth.min(behind)) as f64;
            let r = rect(x0 + k * step, bottom - h - k * rise, w, h);
            let state = if *index == primary { CtrlState::Selected } else { CtrlState::Normal };
            let border = tok.controls.border(state);
            self.d.bordered(cx, r, tok.controls.fill(state), border, border, 0.0, 1.0);
        }
        let caption_x = x0 + pw + behind as f64 * step + 14.0;
        let caption = rect(caption_x, area.pos.y, (area.pos.x + area.size.x - caption_x).max(0.0), area.size.y);
        let (line1, line2) = cut_top(rect(caption.pos.x, caption.pos.y + 8.0, caption.size.x, caption.size.y - 8.0), 22.0);
        let (title, body) = if active.len() >= 2 {
            ("Clone displays", "Rendered for the source above, shown on every output")
        } else {
            ("One display", "A second output would show this desktop too")
        };
        self.d.label_elided(cx, line1, true, tok.font.body_small, fg, HAlign::Left, title);
        self.d.label_elided(cx, rect(line2.pos.x, line2.pos.y, line2.size.x, 18.0), false, tok.font.caption, dim, HAlign::Left, body);
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
        // The Display dropdown's scale and outputs: the actual window
        // scale, the held preview, the saved settings, and the renderer's
        // snapshot as shown — with the source and GPU selections' state.
        let d = &self.display_snapshot;
        let eligible: Vec<String> = self.eligible_sources().into_iter().map(|o| o.name.clone()).collect();
        text.push_str(&format!(
            "\ndpi={:.2} dpi_hundredths={} dpi_preview={:?} dpi_saved={:?} display_settings_loaded={} display_direct={} display_outputs={} display_status={} display_scroll={} display_overflow={}",
            self.current_dpi, self.dpi_hundredths(), self.dpi_preview, s.dpi_scale, s.display_settings_loaded,
            d.direct, d.outputs.len(), self.display_status(), self.display_scroll, self.display_overflow
        ));
        text.push_str(&format!(
            "\nsource={:?} source_pending={:?} source_saved={:?} source_picker={} source_eligible={:?} source_scroll={} source_overflow={} source_rows={:?}",
            d.outputs.iter().find(|o| o.primary).map(|o| o.name.as_str()),
            self.display_source_pending.as_ref().map(|(name, _)| name.as_str()),
            s.display_source, self.source_picker, eligible, self.source_scroll, self.source_overflow, self.source_targets
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

/// `3840×2160 · 240 Hz`, with the refresh to two places only when it is
/// not a whole number; empty when the renderer reported no mode.
fn display_mode(output: &LinuxDisplayOutput) -> String {
    if output.width == 0 || output.height == 0 {
        return String::new();
    }
    let hz = output.refresh_hz;
    let refresh = if !hz.is_finite() || hz <= 0.0 {
        String::new()
    } else if (hz - hz.round()).abs() < 0.05 {
        format!(" · {:.0} Hz", hz)
    } else {
        format!(" · {:.2} Hz", hz)
    };
    format!("{}×{}{}", output.width, output.height, refresh)
}
