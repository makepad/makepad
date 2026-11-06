//! Linux desktop controls. The UI owns handles and immutable snapshots;
//! all device operations belong to the system worker.
//!
//! Two display settings are the UI's own to apply, with only their
//! *persistence* going to the worker:
//!
//! * The display scale: a preset goes to `Cx::set_window_dpi_override` on
//!   the main window (the platform rewrites the geometry and every child
//!   tile follows). The saved value is applied once at start-up, when the
//!   window exists, unless the person has set a scale this session.
//! * The display layout (`display-layout`, see `shell::display_layout`):
//!   the render-on GPU, the virtual desktop and one profile per set of
//!   connected monitors (screens, mirrors, modes, main screen, pacing). The
//!   renderer applies the profiles itself, at start-up and whenever the
//!   monitors change; the WM only saves them. The WM keeps a working copy;
//!   every edit updates it and queues the whole file for saving (one slot,
//!   newest intent). The Display menu's Apply sends a whole configuration
//!   (`shell::display_menu`): one that reinitialises no output is saved as
//!   soon as the renderer shows it, one that does is kept by the person
//!   within 15 s or reverted; a failure is reported under the Apply row and
//!   the previous configuration sent again. Pacing applies at once and is
//!   saved when the renderer takes it. The saved virtual desktop is
//!   restored once at start-up unless the environment pins it.
//! * Pointer speed: each drag goes to `linux_input::set_pointer_speed`
//!   (process-wide, mouse and touchpad separately). The saved values are
//!   applied once at start-up unless the person already set that device
//!   this session; a late read never undoes a speed set here.
use crate::{shell, App};
use makepad_widgets::*;
use makepad_widgets::makepad_platform::linux_input::{
    set_pointer_speed, POINTER_SPEED_MAX, POINTER_SPEED_MIN,
};
use makepad_widgets::makepad_platform::linux_display::display_profiles::ProfileSettings;
use makepad_widgets::makepad_platform::linux_display::{DisplayPacing, LinuxDisplayCommand, LinuxDisplaySnapshot, VirtualPanels};
use shell::display_layout::{virtual_env_pinned, virtual_restore, DisplayLayout, SavedVirtual, VirtualSize};
use shell::display_menu::{DisplayConfig, DisplayTrials, TrialOut};
use shell::panels::{PanelKind, ShellPanel};
use shell::system_linux::{
    validate_gpu_choice, BatteryStatus, CommandResult, GpuInfo, Seq,
    SystemCommand, SystemController, DISPLAY_SCALE_MAX, DISPLAY_SCALE_MIN,
};
use std::collections::VecDeque;

/// How long a confirmed restart may wait for its `SaveDisplayLayout` to be
/// confirmed before it is abandoned (a wedged worker thread: alive by
/// `worker_alive()`'s own test, `!is_finished()`, but not answering).
const RESTART_SAVE_TIMEOUT: f64 = 5.0;

#[derive(Clone, Debug, PartialEq)]
pub enum ControlAction {
    OutputVolume(u32),
    InputVolume(u32),
    /// The display scale in hundredths (130 = 1.30×): a preset or the
    /// slider's release, applied and saved.
    DpiScale(u32),
    /// The scale slider while dragged: applied, not saved.
    DpiScaleLive(u32),
    /// The Display menu's Apply: the whole configuration (screens, mirrors,
    /// modes, main screen) at once. Saved once shown; one that reinitialises
    /// an output only once kept (`DisplayKeep`), reverted by
    /// `DisplayRevert` or after 15 s.
    DisplayApply(DisplayConfig),
    DisplayKeep,
    DisplayRevert,
    /// Which screen paces the frames: applied at once (no modeset, so no
    /// confirm) and saved.
    DisplayPacing(DisplayPacing),
    /// Render-on GPU: `Some(identity)` as `GpuInfo::identity` gives it,
    /// `None` for Auto (the display GPU). While the screens span GPUs the
    /// renderer cannot switch live, so it is only saved for the next
    /// start; otherwise it is applied now and saved once the switch
    /// finishes.
    GpuChoice(Option<String>),
    /// Desktop: the panel's own mode, rendered once (the virtual display
    /// off). The normal display scale applies again.
    DesktopPanel,
    /// Desktop: the virtual display (the saved choices, else Follow viewer
    /// with the 7680x2160@120 fallback, panels Mirror, scale 2.00x).
    DesktopVirtual,
    /// The virtual desktop's size: follow the Desk Link viewer, or a
    /// preset / custom size. Applied (no modeset) and saved.
    VirtualSize(VirtualSize),
    /// What the panels show while the desktop is virtual: the desktop
    /// scaled to fit (Mirror) or nothing (Off). Re-drives the panels.
    VirtualPanels(VirtualPanels),
    /// The virtual desktop's scale in hundredths: applied and saved as the
    /// virtual desktop's own (the normal display scale is left alone).
    VirtualScale(u32),
    /// Pointer speed in hundredths (100 = 1.00×). Applied on every update;
    /// `touchpad` selects the device (`false` mouse, `true` touchpad).
    PointerSpeed { touchpad: bool, value: u32 },
    /// Natural scrolling (the Mac direction) for mouse wheels or touchpads: applied in the
    /// direct backend at once and saved beside the theme's state (`host`).
    NaturalScroll { touchpad: bool, on: bool },
    /// "Restart desktop now", confirmed: restart the WM so the session
    /// script applies the saved render-on GPU (apps close).
    RestartDesktop,
    /// A BlackShark headset edit. It bypasses the command queue: the
    /// worker keeps the newest value per setting until the headset
    /// confirms it, so a full queue can never drop one.
    Headset(shell::blackshark::HeadsetEdit),
    Command(SystemCommand),
}

#[derive(Default)]
pub struct LinuxControls {
    controller: Option<SystemController>,
    attempted: bool,
    waiting: VecDeque<SystemCommand>,
    seen: VecDeque<u32>,
    notice: String,
    /// The scale still to be persisted: one slot, so a newer release
    /// replaces an older one that the full queue held back.
    dpi_save: Option<u32>,
    /// The person set a scale this session; a start-up read that lands
    /// afterwards must not undo it.
    dpi_user_set: bool,
    /// The start-up read has been applied (or deliberately skipped) once.
    dpi_loaded_applied: bool,
    /// The working display layout: the saved one once the worker's
    /// start-up read lands, with this session's edits on top. `None` until
    /// either happens.
    layout: Option<DisplayLayout>,
    /// The worker's start-up read has been taken into `layout`.
    layout_loaded: bool,
    /// The whole layout still to be persisted, same one-slot rule. Held
    /// until `layout_loaded`: a whole-file save made before the start-up
    /// read is taken in would drop what the file held.
    layout_save: Option<DisplayLayout>,
    /// This session edited the layout (any part); once the start-up read
    /// is merged in, the merged layout is queued for saving again.
    layout_edited: bool,
    /// The person chose a render-on GPU this session; a late read keeps it.
    gpu_user_set: bool,
    /// The person chose the desktop (Panel / Virtual and its choices) this
    /// session; a late read keeps it and the restore stops.
    virtual_user_set: bool,
    /// The active Desk Link viewer's window as last reported: device
    /// pixels, refresh (mHz), scale (hundredths). Kept after the viewer
    /// leaves, so a followed size stays until the next one reports.
    viewer: Option<(u32, u32, u32, u32)>,
    /// The normal display scale (the saved or last chosen one), put back
    /// when the desktop goes from Virtual to Panel.
    panel_scale: Option<u32>,
    /// The saved virtual desktop has been restored (or deliberately
    /// skipped) once.
    virtual_restore_done: bool,
    /// Newest unsent pointer speed per device (0 mouse, 1 touchpad).
    pointer_save: [Option<u32>; 2],
    /// The person set that device's speed this session; a start-up read
    /// that lands afterwards must not undo it.
    pointer_user_set: [bool; 2],
    /// The start-up read has been applied (or deliberately skipped) once
    /// per device.
    pointer_loaded_applied: [bool; 2],
    /// Armed by a main-window geometry change: the frame after it, the open
    /// flyout re-reads its bar module's rect, which the bar has re-laid out.
    reanchor_frame: NextFrame,
    /// "Restart desktop now" was confirmed (the saved render-on GPU was
    /// present at the time): `poll_linux_controls` calls `cx.quit()` once
    /// the pending or last `SaveDisplayLayout`'s outcome has landed, and
    /// `Event::Shutdown` reads this to exit 75 instead of 0. Cleared back
    /// to `false` if that save fails, times out (`RESTART_SAVE_TIMEOUT`)
    /// or cannot be waited on at all (no controller, or the worker died),
    /// abandoning the restart.
    restart_requested: bool,
    /// `cx.quit()` has already been called for the armed restart, so a
    /// later poll (before `Event::Shutdown` arrives) does not call it again.
    restart_quit_called: bool,
    /// `Cx::monotonic_now()` when the restart was armed: `restart_timed_out`
    /// measures the bounded wait for the save from here.
    restart_since: Option<f64>,
    /// The sequence of the `SaveDisplayLayout` a restart (or an ordinary
    /// edit) is waiting on, until its outcome (success or failure) lands.
    layout_save_seq: Option<Seq>,
    /// The configuration being tried and the note under the Display menu's
    /// Apply row.
    trials: DisplayTrials,
}

/// `slot`'s layout to send now: nothing until the start-up read has been
/// taken in (`loaded`), since a whole-file save before it would drop what
/// the file held.
fn take_layout_save(loaded: bool, slot: &mut Option<DisplayLayout>) -> Option<DisplayLayout> {
    if loaded { slot.take() } else { None }
}

fn valid_dpi(dpi: f64) -> bool {
    dpi.is_finite() && dpi > 0.0
}

/// Whether "Restart desktop now" can still do something: the saved
/// render-on choice (`None` is Auto, always present) resolves to a GPU
/// this boot lists. Mirrors the panel's own pre-press check (the same
/// shape as its `saved_gpu_present`): once the saved GPU is gone, a
/// restart would only fall back to Auto, which is not what was promised,
/// so it is refused and reported instead.
fn restart_gate(saved: Option<&str>, gpus: &[GpuInfo]) -> bool {
    saved.map_or(true, |identity| gpus.iter().any(|gpu| gpu.matches(identity)))
}

/// Whether an armed restart, confirmed at `since`, has waited too long at
/// `now` for its `SaveDisplayLayout`'s outcome to land: past
/// `RESTART_SAVE_TIMEOUT`. A wedged worker thread
/// (alive by `worker_alive()`'s own test but never answering) would
/// otherwise leave the restart waiting forever with no feedback.
fn restart_timed_out(since: f64, now: f64) -> bool {
    now - since > RESTART_SAVE_TIMEOUT
}

/// The process exit code for `Event::Shutdown`: `75` (`EX_TEMPFAIL`) once
/// "Restart desktop now" asked for one, so systemd's
/// `RestartForceExitStatus=75`/`SuccessExitStatus=75` treats it as a
/// deliberate, successful restart and starts the WM again (re-running the
/// session script, which re-reads `display-layout`); `0`, unchanged, for
/// an ordinary quit, which both units leave down.
pub(crate) fn exit_code(restart_requested: bool) -> i32 {
    if restart_requested { 75 } else { 0 }
}

impl LinuxControls {
    /// Edit the working layout and queue the whole file for saving.
    fn edit_layout(&mut self, edit: impl FnOnce(&mut DisplayLayout)) {
        let layout = self.layout.get_or_insert_with(DisplayLayout::default);
        edit(layout);
        self.layout_edited = true;
        self.layout_save = Some(layout.clone());
    }

    /// The layout to send to the worker now, if any: nothing until the
    /// start-up read has been taken in.
    #[cfg(test)]
    fn take_layout_save(&mut self) -> Option<DisplayLayout> {
        take_layout_save(self.layout_loaded, &mut self.layout_save)
    }

    /// The virtual desktop's scale: the followed viewer's while one has
    /// reported, else the saved one.
    fn virtual_scale(&self, saved: &SavedVirtual) -> u32 {
        let viewer = self.viewer.map(|(_, _, _, scale)| scale).filter(|scale| *scale > 0);
        viewer.filter(|_| saved.follow).unwrap_or(saved.scale).clamp(DISPLAY_SCALE_MIN, DISPLAY_SCALE_MAX)
    }

    /// The saved virtual desktop, as this session holds it.
    fn virtual_saved(&self) -> Option<SavedVirtual> {
        self.layout.as_ref().and_then(|layout| layout.virtual_display)
    }

    /// The person chose the desktop: the start-up restore is over.
    fn virtual_set_by_person(&mut self) {
        self.virtual_user_set = true;
        self.virtual_restore_done = true;
    }

    /// The worker's start-up read: the working copy unless this session
    /// already edited it first, in which case it is merged in
    /// (`DisplayLayout::merge_loaded`: the person's edits stand, every
    /// other saved profile and an untouched render-on come from the read).
    /// Marks the read as taken in; a layout edited before it is queued
    /// again, merged, so the whole-file save keeps what the file held.
    fn adopt_loaded_layout(&mut self, loaded: &DisplayLayout) {
        self.layout_loaded = true;
        match self.layout.as_mut() {
            None => self.layout = Some(loaded.clone()),
            Some(layout) => {
                layout.merge_loaded(loaded, self.gpu_user_set, self.virtual_user_set)
            }
        }
        if self.layout_edited {
            self.layout_save = self.layout.clone();
        }
    }
}

impl App {
    /// The main window, once the platform has created it.
    fn main_window_id(&self, cx: &mut Cx) -> Option<WindowId> {
        let window_id = self.ui.window(cx, ids!(main_window)).window_id()?;
        (cx.windows.is_valid(window_id) && cx.windows[window_id].is_created).then_some(window_id)
    }

    /// The main window's effective layout scale, as the platform holds it.
    fn main_window_dpi(&self, cx: &mut Cx) -> Option<f64> {
        let window_id = self.main_window_id(cx)?;
        Some(cx.windows[window_id].window_geom.dpi_factor).filter(|dpi| valid_dpi(*dpi))
    }

    /// Apply a display scale to the main window. The platform queues the
    /// `WindowGeomChange`; the flyout is re-anchored on the frame after it.
    fn apply_dpi_scale(&mut self, cx: &mut Cx, hundredths: u32) -> bool {
        let Some(window_id) = self.main_window_id(cx) else {
            self.linux_controls.notice = "Display scale: the desktop window is not up yet".into();
            log!("wm: display scale {hundredths} not applied: no main window");
            return false;
        };
        cx.set_window_dpi_override(window_id, Some(hundredths as f64 / 100.0));
        self.linux_controls.reanchor_frame = cx.new_next_frame();
        true
    }

    /// Bring the renderer to the virtual desktop `saved` asks for: its
    /// size (the followed viewer's window when one has reported), refresh
    /// and panels. Nothing is sent when the snapshot already shows it.
    fn send_virtual(&mut self, cx: &mut Cx, saved: &SavedVirtual) -> Result<(), String> {
        let viewer = self.linux_controls.viewer.map(|(w, h, mhz, _)| (w, h, mhz));
        let target = saved.target(viewer);
        if cx.linux_display_snapshot().virtual_display == Some(target) {
            return Ok(());
        }
        log!("wm: virtual display {} panels {}", target.mode_string(), target.panels.as_str());
        cx.linux_set_virtual_display(Some(target))
    }

    /// Apply the virtual desktop's scale (the person's own choice, so a
    /// late start-up read of the normal scale does not undo it).
    fn apply_virtual_scale(&mut self, cx: &mut Cx, saved: &SavedVirtual) {
        self.linux_controls.dpi_user_set = true;
        self.linux_controls.dpi_loaded_applied = true;
        let scale = self.linux_controls.virtual_scale(saved);
        self.apply_dpi_scale(cx, scale);
    }

    /// Save `saved` as the virtual desktop's choices.
    fn save_virtual(&mut self, saved: SavedVirtual) {
        self.linux_controls.virtual_set_by_person();
        self.linux_controls.edit_layout(|layout| layout.virtual_display = Some(saved));
    }

    /// The Desk Link viewer's window: device pixels, its display refresh
    /// (mHz), its scale (hundredths). Applies only while the desktop is
    /// Virtual with Follow viewer on, and only when it differs from what
    /// the desktop already is; otherwise it only remembers the report.
    /// The scale applies as the window's dpi but is not saved: the saved
    /// virtual scale stays the fallback.
    pub(crate) fn desk_link_viewer_window(&mut self, cx: &mut Cx, width: u32, height: u32, refresh_mhz: u32, scale: u32) {
        let report = (width, height, refresh_mhz, scale);
        if self.linux_controls.viewer == Some(report) {
            return;
        }
        self.linux_controls.viewer = Some(report);
        let Some(saved) = self.linux_controls.virtual_saved().filter(|saved| saved.follow) else { return };
        if let Err(message) = self.send_virtual(cx, &saved) {
            self.linux_controls.notice = format!("Desktop: {message}");
            log!("wm: virtual display for the viewer's window refused: {message}");
        }
        if scale > 0 {
            let scale = self.linux_controls.virtual_scale(&saved);
            self.apply_dpi_scale(cx, scale);
        }
        self.redraw_all(cx);
    }

    /// "Restart desktop now", confirmed. The panel re-checked the saved
    /// render-on GPU was present right before sending this; the click and
    /// this handler can straddle a poll, so it is checked again here
    /// against the panel's own cached fields (`render_on_saved`,
    /// `system.gpus`) with the same `restart_gate` the panel's own check
    /// uses. Once armed, `poll_linux_controls` shows "Restarting…" in the
    /// panel (`restart_waiting`) and calls `cx.quit()` as soon as the
    /// pending or last `SaveDisplayLayout`'s outcome has landed
    /// (`layout_save`/`layout_save_seq` both empty); `Event::Shutdown`
    /// then exits 75. A save that fails, or does not land within
    /// `RESTART_SAVE_TIMEOUT`, cancels the restart there instead and
    /// reports it.
    fn request_restart(&mut self, cx: &mut Cx) {
        self.linux_controls.notice.clear();
        let panel = self.ui.widget(cx, ids!(shell_panel));
        let (saved, gpus) = panel
            .borrow::<ShellPanel>()
            .map(|p| (p.render_on_saved.clone(), p.system.gpus.clone()))
            .unwrap_or((None, Vec::new()));
        if restart_gate(saved.as_deref(), &gpus) {
            self.linux_controls.restart_requested = true;
            self.linux_controls.restart_quit_called = false;
            self.linux_controls.restart_since = Some(Cx::monotonic_now());
            log!("wm: restart desktop requested, waiting for the display layout save to land");
        } else {
            self.linux_controls.notice = "Restart desktop now: the saved GPU is no longer present".into();
            log!("wm: restart desktop refused: saved render-on GPU is no longer present");
        }
        self.poll_linux_controls(cx);
        self.redraw_all(cx);
    }

    /// An open flyout follows its bar module: after the bar re-lays out
    /// (a DPI change, a window resize) the module's rect is read again.
    fn reanchor_shell_panel(&mut self, cx: &mut Cx) {
        let panel = self.ui.widget(cx, ids!(shell_panel));
        let Some(kind) = panel.borrow::<ShellPanel>().and_then(|p| p.open) else {
            return;
        };
        let bar = self.ui.widget(cx, ids!(shell_bar));
        let anchor = bar.borrow::<shell::bar::ShellBar>().and_then(|b| b.module_rect(self.shell_panel_segment, kind.module()));
        if let Some(anchor) = anchor {
            if let Some(mut p) = panel.borrow_mut::<ShellPanel>() {
                p.reanchor(cx, anchor);
            }
        }
    }

    /// Events the controls watch besides the tick and the signal: the
    /// re-anchor frame and the main window's geometry changes.
    pub(crate) fn linux_controls_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.linux_controls.reanchor_frame.is_event(event).is_some() {
            self.reanchor_shell_panel(cx);
        }
        if let Event::LinuxDisplayResult(ev) = event {
            self.display_result(cx, &ev.command, &ev.result);
        }
        if let Event::WindowGeomChange(ev) = event {
            if Some(ev.window_id) != self.main_window_id(cx) {
                return;
            }
            // The flyout's readout and its anchor move with the window: the
            // readout now, the anchor once the bar has drawn at the new size.
            let panel = self.ui.widget(cx, ids!(shell_panel));
            if let Some(mut p) = panel.borrow_mut::<ShellPanel>() {
                if valid_dpi(ev.new_geom.dpi_factor) {
                    p.current_dpi = ev.new_geom.dpi_factor;
                }
                if p.open.is_some() {
                    p.redraw(cx);
                }
            }
            self.linux_controls.reanchor_frame = cx.new_next_frame();
        }
    }

    pub(crate) fn linux_control(&mut self, cx: &mut Cx, action: ControlAction) {
        self.poll_linux_controls(cx);
        let action = match action {
            // While the desktop is virtual the scale slider is the virtual
            // desktop's: its release saves the virtual scale, not the
            // normal one.
            ControlAction::DpiScale(value) if self.linux_controls.virtual_saved().is_some() => {
                self.linux_control(cx, ControlAction::VirtualScale(value));
                return;
            }
            ControlAction::DpiScaleLive(value) => {
                let value = value.clamp(DISPLAY_SCALE_MIN, DISPLAY_SCALE_MAX);
                self.linux_controls.dpi_user_set = true;
                self.linux_controls.dpi_loaded_applied = true;
                self.apply_dpi_scale(cx, value);
                self.redraw_all(cx);
                return;
            }
            ControlAction::DpiScale(value) => {
                // The scale is applied here whatever the worker's state: the
                // person sees it at once, and only the saving can fail.
                let value = value.clamp(DISPLAY_SCALE_MIN, DISPLAY_SCALE_MAX);
                self.linux_controls.notice.clear();
                self.linux_controls.dpi_user_set = true;
                self.linux_controls.dpi_loaded_applied = true;
                self.apply_dpi_scale(cx, value);
                self.linux_controls.panel_scale = Some(value);
                self.linux_controls.dpi_save = Some(value);
                self.poll_linux_controls(cx);
                self.redraw_all(cx);
                return;
            }
            ControlAction::DisplayApply(config) => {
                self.linux_controls.notice.clear();
                let running = DisplayConfig::running(&cx.linux_display_snapshot());
                let out = self.linux_controls.trials.apply(config, running, cx.linux_display_generation(), Cx::monotonic_now());
                self.apply_trial_out(cx, out);
                self.poll_linux_controls(cx);
                self.redraw_all(cx);
                return;
            }
            ControlAction::DisplayPacing(pacing) => {
                self.linux_controls.notice.clear();
                // Saved once the renderer takes it (`display_result`).
                match cx.linux_set_display_pacing(&pacing) {
                    Ok(()) => log!("wm: display pacing {pacing:?} asked"),
                    Err(message) => self.linux_controls.notice = format!("Pace by: {message}"),
                }
                self.poll_linux_controls(cx);
                self.redraw_all(cx);
                return;
            }
            ControlAction::DesktopPanel => {
                self.linux_controls.notice.clear();
                if self.linux_controls.virtual_saved().is_some() {
                    match cx.linux_set_virtual_display(None) {
                        Ok(()) => {
                            self.linux_controls.virtual_set_by_person();
                            self.linux_controls.edit_layout(|layout| layout.virtual_display = None);
                            if let Some(scale) = self.linux_controls.panel_scale {
                                self.linux_controls.dpi_user_set = true;
                                self.linux_controls.dpi_loaded_applied = true;
                                self.apply_dpi_scale(cx, scale);
                            }
                        }
                        Err(message) => self.linux_controls.notice = format!("Desktop: {message}"),
                    }
                }
                self.poll_linux_controls(cx);
                self.redraw_all(cx);
                return;
            }
            ControlAction::DesktopVirtual => {
                self.linux_controls.notice.clear();
                if self.linux_controls.virtual_saved().is_none() {
                    let saved = SavedVirtual::first_pick();
                    if self.linux_controls.panel_scale.is_none() {
                        self.linux_controls.panel_scale = self.main_window_dpi(cx).map(|dpi| (dpi * 100.0).round() as u32);
                    }
                    match self.send_virtual(cx, &saved) {
                        Ok(()) => {
                            self.save_virtual(saved);
                            self.apply_virtual_scale(cx, &saved);
                        }
                        Err(message) => self.linux_controls.notice = format!("Desktop: {message}"),
                    }
                }
                self.poll_linux_controls(cx);
                self.redraw_all(cx);
                return;
            }
            ControlAction::VirtualSize(size) => {
                self.linux_controls.notice.clear();
                if let Some(saved) = self.linux_controls.virtual_saved() {
                    let saved = saved.with_size(size);
                    match self.send_virtual(cx, &saved) {
                        Ok(()) => {
                            self.save_virtual(saved);
                            self.apply_virtual_scale(cx, &saved);
                        }
                        Err(message) => self.linux_controls.notice = format!("Desktop size: {message}"),
                    }
                }
                self.poll_linux_controls(cx);
                self.redraw_all(cx);
                return;
            }
            ControlAction::VirtualPanels(panels) => {
                self.linux_controls.notice.clear();
                if let Some(mut saved) = self.linux_controls.virtual_saved() {
                    saved.mode.panels = panels;
                    match self.send_virtual(cx, &saved) {
                        Ok(()) => self.save_virtual(saved),
                        Err(message) => self.linux_controls.notice = format!("Panels: {message}"),
                    }
                }
                self.poll_linux_controls(cx);
                self.redraw_all(cx);
                return;
            }
            ControlAction::VirtualScale(value) => {
                self.linux_controls.notice.clear();
                if let Some(mut saved) = self.linux_controls.virtual_saved() {
                    saved.scale = value.clamp(DISPLAY_SCALE_MIN, DISPLAY_SCALE_MAX);
                    // The person's value stands over a followed viewer's
                    // scale until the viewer's window next changes.
                    self.linux_controls.dpi_user_set = true;
                    self.linux_controls.dpi_loaded_applied = true;
                    self.apply_dpi_scale(cx, saved.scale);
                    self.save_virtual(saved);
                }
                self.poll_linux_controls(cx);
                self.redraw_all(cx);
                return;
            }
            ControlAction::DisplayKeep => {
                let out = self.linux_controls.trials.keep();
                self.apply_trial_out(cx, out);
                self.poll_linux_controls(cx);
                self.redraw_all(cx);
                return;
            }
            ControlAction::DisplayRevert => {
                let out = self.linux_controls.trials.revert(Cx::monotonic_now());
                self.apply_trial_out(cx, out);
                self.poll_linux_controls(cx);
                self.redraw_all(cx);
                return;
            }
            ControlAction::RestartDesktop => {
                self.request_restart(cx);
                return;
            }
            ControlAction::Headset(edit) => {
                match self.linux_controls.controller.as_mut().filter(|c| c.worker_alive()) {
                    Some(controller) => controller.headset_edit(edit),
                    None => self.linux_controls.notice = "System controls are unavailable".into(),
                }
                self.poll_linux_controls(cx);
                return;
            }
            ControlAction::GpuChoice(choice) => {
                self.linux_controls.notice.clear();
                if let Some(message) = choice.as_deref().and_then(|identity| validate_gpu_choice(identity).err()) {
                    self.linux_controls.notice = format!("Render on: {message}");
                } else {
                    self.linux_controls.gpu_user_set = true;
                    self.linux_controls.edit_layout(|layout| layout.render_on = choice);
                    self.linux_controls.notice = "GPU choice saved; restart the desktop to apply it".into();
                }
                self.poll_linux_controls(cx);
                self.redraw_all(cx);
                return;
            }
            ControlAction::NaturalScroll { touchpad, on } => {
                crate::host::persist_natural_scroll(touchpad, on);
                crate::host::apply_natural_scroll(touchpad, on);
                self.redraw_all(cx);
                return;
            }
            ControlAction::PointerSpeed { touchpad, value } => {
                // Applied here whatever the worker's state: the person sees
                // it at once, and only the saving can fail.
                let value = value.clamp(POINTER_SPEED_MIN, POINTER_SPEED_MAX);
                let i = touchpad as usize;
                self.linux_controls.notice.clear();
                self.linux_controls.pointer_user_set[i] = true;
                self.linux_controls.pointer_loaded_applied[i] = true;
                set_pointer_speed(touchpad, value);
                self.linux_controls.pointer_save[i] = Some(value);
                self.poll_linux_controls(cx);
                self.redraw_all(cx);
                return;
            }
            other => other,
        };
        let state = &mut self.linux_controls;
        state.notice.clear();
        if let Some(controller) = state.controller.as_mut().filter(|c| c.worker_alive()) {
            match action {
                ControlAction::OutputVolume(value) => { controller.set_output_volume(value); }
                ControlAction::InputVolume(value) => { controller.set_input_volume(value); }
                ControlAction::DpiScale(_)
                | ControlAction::DpiScaleLive(_)
                | ControlAction::DisplayApply(_)
                | ControlAction::DisplayKeep
                | ControlAction::DisplayRevert
                | ControlAction::DisplayPacing(_)
                | ControlAction::DesktopPanel
                | ControlAction::DesktopVirtual
                | ControlAction::VirtualSize(_)
                | ControlAction::VirtualPanels(_)
                | ControlAction::VirtualScale(_)
                | ControlAction::RestartDesktop
                | ControlAction::GpuChoice(_)
                | ControlAction::NaturalScroll { .. }
                | ControlAction::Headset(_)
                | ControlAction::PointerSpeed { .. } => {}
                ControlAction::Command(command) => {
                    if state.waiting.len() < 32 {
                        state.waiting.push_back(command);
                    } else {
                        state.notice = "Controls are busy; please try again".into();
                        log!("wm: system control queue full");
                    }
                }
            }
        } else {
            state.notice = "System controls are unavailable".into();
        }
        self.poll_linux_controls(cx);
        self.redraw_all(cx);
    }

    pub(crate) fn poll_linux_controls(&mut self, cx: &mut Cx) {
        if self.gallery { return; }
        let state = &mut self.linux_controls;
        if !state.attempted {
            state.attempted = true;
            match SystemController::start(&cx.thread_spawner()) {
                Ok(controller) => state.controller = Some(controller),
                Err(error) => {
                    state.notice = format!("System controls could not start: {error}");
                    log!("wm: {}", state.notice);
                }
            }
        }
        let mut changed = false;
        let mut headset_pending = None;
        let snapshot = if let Some(controller) = state.controller.as_mut() {
            changed = controller.poll();
            headset_pending = Some(controller.headset_pending());
            if controller.worker_alive() {
                // The display settings first: each is the newest intent in
                // its own slot, so a backlog of other commands never drops it.
                if let Some(value) = state.dpi_save.take() {
                    if controller.send(SystemCommand::SaveDpiScale(value)).is_err() {
                        state.dpi_save = Some(value);
                    }
                }
                if let Some(layout) = take_layout_save(state.layout_loaded, &mut state.layout_save) {
                    match controller.send(SystemCommand::SaveDisplayLayout(layout)) {
                        Ok(seq) => state.layout_save_seq = Some(seq),
                        Err(SystemCommand::SaveDisplayLayout(layout)) => state.layout_save = Some(layout),
                        Err(_) => {}
                    }
                }
                for i in 0..2 {
                    if let Some(value) = state.pointer_save[i].take() {
                        let touchpad = i != 0;
                        if controller
                            .send(SystemCommand::SavePointerSpeed { touchpad, value })
                            .is_err()
                        {
                            state.pointer_save[i] = Some(value);
                        }
                    }
                }
                while let Some(command) = state.waiting.pop_front() {
                    if let Err(command) = controller.send(command) {
                        state.waiting.push_front(command);
                        break;
                    }
                }
            } else {
                state.waiting.clear();
                let unsaved = state.dpi_save.take().is_some()
                    | state.layout_save.take().is_some()
                    | state.pointer_save[0].take().is_some()
                    | state.pointer_save[1].take().is_some();
                state.layout_save_seq = None;
                if state.restart_requested && !state.restart_quit_called {
                    // The worker is gone: no `Applied` can ever land, so the
                    // restart cannot be confirmed. Leaving is pointless too
                    // (the saved layout may be stale), so it is abandoned.
                    state.restart_requested = false;
                    state.restart_since = None;
                    log!("wm: restart desktop cancelled: system controls stopped");
                }
                state.notice = if unsaved {
                    "Setting applied, but not saved: system controls stopped".into()
                } else {
                    "System controls stopped; restart the WM to retry".into()
                };
            }
            let snapshot = controller.snapshot().clone();
            for outcome in &snapshot.outcomes {
                if state.seen.contains(&outcome.seq) { continue; }
                if state.seen.len() == 64 { state.seen.pop_front(); }
                state.seen.push_back(outcome.seq);
                if state.layout_save_seq == Some(outcome.seq) {
                    state.layout_save_seq = None;
                    if matches!(outcome.result, CommandResult::Failed(_)) && state.restart_requested {
                        state.restart_requested = false;
                        state.restart_since = None;
                        log!("wm: restart desktop cancelled: the display layout save failed");
                    }
                }
                if let CommandResult::Failed(message) = &outcome.result {
                    state.notice = message.clone();
                    log!("wm: system control {:?} failed: {}", outcome.kind, message);
                }
            }
            if state.restart_requested && !state.restart_quit_called {
                // A worker thread stays `worker_alive()` (`!is_finished()`)
                // even wedged (a blocking I/O stall, a stuck command ahead
                // of ours in its queue), so a dead/missing worker is not
                // the only way this never resolves: without this bound, a
                // wedged worker would leave the restart (and the panel's
                // "Restarting…") waiting forever with no feedback.
                if state.restart_since.is_some_and(|since| restart_timed_out(since, Cx::monotonic_now())) {
                    state.restart_requested = false;
                    state.restart_since = None;
                    state.notice = "Couldn't save the display layout; the desktop was not restarted".into();
                    log!("wm: {}", state.notice);
                } else if state.layout_save.is_none() && state.layout_save_seq.is_none() {
                    state.restart_quit_called = true;
                    log!("wm: display layout saved; restarting the desktop");
                    cx.quit();
                }
            }
            Some(snapshot)
        } else {
            if state.restart_requested && !state.restart_quit_called {
                // No controller at all: the same as the worker having died.
                state.restart_requested = false;
                state.restart_since = None;
                log!("wm: restart desktop cancelled: system controls are unavailable");
            }
            None
        };
        // The saved settings, once each. A late read after the person
        // already acted is ignored; nothing saved means the launch (scale)
        // or the renderer's own (layout) default stands.
        let mut initial_scale = None;
        let mut initial_pointer = [None; 2];
        if let Some(snapshot) = snapshot.as_ref().filter(|s| s.display_settings_loaded) {
            if state.panel_scale.is_none() {
                state.panel_scale = snapshot.dpi_scale;
            }
            if !state.layout_loaded {
                state.adopt_loaded_layout(&snapshot.display_layout);
            }
        }
        if let Some(snapshot) = snapshot.as_ref().filter(|s| s.display_settings_loaded) {
            // The scale after the layout: a virtual desktop has its own.
            if !state.dpi_loaded_applied {
                if state.dpi_user_set {
                    state.dpi_loaded_applied = true;
                } else if let Some(value) = state.virtual_saved().map(|saved| state.virtual_scale(&saved)).or(snapshot.dpi_scale) {
                    initial_scale = Some(value);
                } else {
                    state.dpi_loaded_applied = true;
                }
            }
        }
        let restore_pending = state.layout_loaded && !state.virtual_restore_done;
        if let Some(snapshot) = snapshot.as_ref().filter(|s| s.input_settings_loaded) {
            for i in 0..2 {
                if !state.pointer_loaded_applied[i] {
                    if state.pointer_user_set[i] {
                        state.pointer_loaded_applied[i] = true;
                    } else if let Some(value) = snapshot.pointer_speeds[i] {
                        initial_pointer[i] = Some(value);
                    } else {
                        state.pointer_loaded_applied[i] = true;
                    }
                }
            }
        }
        let panel = self.ui.widget(cx, ids!(shell_panel));
        let (adjusting, monitor_open) = panel
            .borrow::<ShellPanel>()
            .map(|p| (p.dpi_adjusting(), p.open == Some(PanelKind::Monitor)))
            .unwrap_or((false, false));
        if let Some(value) = initial_scale {
            // Not under a drag: a saved value landing mid-drag would move the
            // window under the slider. It waits for the next poll.
            if !adjusting && self.main_window_id(cx).is_some() {
                self.linux_controls.dpi_loaded_applied = true;
                log!("wm: applying saved display scale {value}");
                self.apply_dpi_scale(cx, value);
                changed = true;
            }
        }
        for (i, value) in initial_pointer.into_iter().enumerate() {
            if let Some(value) = value {
                self.linux_controls.pointer_loaded_applied[i] = true;
                let name = if i == 0 { "mouse" } else { "touchpad" };
                log!("wm: applying saved {name} speed {value}");
                set_pointer_speed(i != 0, value);
                changed = true;
            }
        }
        // The renderer's inventory, whenever something here needs it: the
        // open Display panel, the saved virtual desktop to restore, a
        // configuration being tried. Immutable cached snapshots, no lock, no
        // Vulkan, no filesystem. GPU identity is only for the open Display readout.
        let trying = self.linux_controls.trials.trial.is_some() || self.linux_controls.trials.restoring.is_some();
        let displays = (monitor_open || restore_pending || trying).then(|| cx.linux_display_snapshot());
        if let (true, Some(displays)) = (restore_pending, displays.as_ref()) {
            changed |= self.restore_virtual(cx, displays);
        }
        // The configuration being tried: up, failed, or out of time.
        if let Some(displays) = displays.as_ref() {
            let before = self.linux_controls.trials.clone();
            let out = self.linux_controls.trials.observe(displays, cx.linux_display_generation(), Cx::monotonic_now());
            self.apply_trial_out(cx, out);
            changed |= self.linux_controls.trials != before;
        }
        let actual_dpi = self.main_window_dpi(cx);
        let state = &mut self.linux_controls;
        if let Some(mut panel) = panel.borrow_mut::<ShellPanel>() {
            // The readout is the window's actual scale, never the saved one.
            if let Some(dpi) = actual_dpi {
                if panel.current_dpi != dpi {
                    panel.current_dpi = dpi;
                    changed = true;
                }
            }
            if let Some(displays) = displays {
                if panel.display_snapshot != displays {
                    panel.display_snapshot = displays;
                    changed = true;
                }
            }
            panel.system_notice = state.notice.clone();
            if panel.display_trials != state.trials {
                panel.display_trials = state.trials.clone();
                changed = true;
            }
            // The revert countdown ticks with the 1 s status poll.
            let now = Cx::monotonic_now();
            if state.trials.seconds_left(now) != state.trials.seconds_left(panel.display_now) {
                changed = true;
            }
            panel.display_now = now;
            // The saved render-on GPU as this session last set it, else as
            // the worker read it.
            let render_on = match (state.layout.as_ref(), snapshot.as_ref()) {
                (Some(layout), _) => layout.render_on.clone(),
                (None, Some(snapshot)) => snapshot.display_layout.render_on.clone(),
                (None, None) => None,
            };
            if panel.virtual_saved != state.virtual_saved() {
                panel.virtual_saved = state.virtual_saved();
                changed = true;
            }
            if panel.render_on_saved != render_on {
                panel.render_on_saved = render_on;
                changed = true;
            }
            if panel.restart_waiting != state.restart_requested {
                // `draw_restart_row` checks `restart_waiting` first, so a
                // leftover `restart_confirm` from the click that led here
                // draws nothing: there is no confirm step to show once the
                // restart is already armed.
                panel.restart_waiting = state.restart_requested;
                changed = true;
            }
            if let Some(pending) = headset_pending.filter(|p| *p != panel.headset_pending) {
                panel.headset_pending = pending;
                changed = true;
            }
            if let Some(snapshot) = snapshot {
                let audio = &snapshot.audio;
                self.bar_sample.volume = audio.default_output().map(|d| d.percent);
                self.bar_sample.muted = audio.default_output().is_some_and(|d| d.muted);
                self.bar_sample.battery = snapshot.power.percent.map(|percent| shell::bar::Battery {
                    percent,
                    charging: snapshot.power.status == BatteryStatus::Charging,
                });
                panel.data.game_controllers.clone_from(&snapshot.game_controllers);
                panel.data.volume = self.bar_sample.volume;
                panel.data.muted = self.bar_sample.muted;
                panel.data.input_volume = audio.default_input().map(|d| d.percent);
                panel.data.battery = self.bar_sample.battery;
                panel.system = snapshot;
            }
            if changed { panel.redraw(cx); }
        }
        if changed { self.update_bar(cx); }
        self.reanchor_shell_panel(cx);
    }

    /// Carry out what the Display menu's trials ask for: send a
    /// configuration to the renderer (a refusal goes back to the trials,
    /// which send the previous one), or save one that is in place.
    fn apply_trial_out(&mut self, cx: &mut Cx, out: Vec<TrialOut>) {
        let mut queue: std::collections::VecDeque<TrialOut> = out.into();
        while let Some(step) = queue.pop_front() {
            match step {
                TrialOut::Send { to, from } => {
                    for command in to.commands(&from) {
                        let sent = match &command {
                            LinuxDisplayCommand::Mode { name, mode } => cx.linux_set_display_mode(name, mode.as_deref()),
                            LinuxDisplayCommand::Arrangement(arrangement) => cx.linux_set_display_arrangement(arrangement),
                            LinuxDisplayCommand::Source(name) => cx.linux_set_display_source(name),
                            LinuxDisplayCommand::Pacing(_) | LinuxDisplayCommand::Virtual(_) => Ok(()),
                        };
                        match sent {
                            Ok(()) => log!("wm: display: asking {command:?}"),
                            Err(message) => {
                                log!("wm: display: {command:?} refused: {message}");
                                queue.extend(self.linux_controls.trials.refused(&message, Cx::monotonic_now()));
                                break;
                            }
                        }
                    }
                }
                TrialOut::Save(config) => {
                    log!("wm: display: in place {}", config.arrangement.text());
                    let displays = cx.linux_display_snapshot();
                    let settings = ProfileSettings {
                        arrangement: config.arrangement,
                        modes: config.modes,
                        main: config.main,
                        pacing: displays.pacing.clone(),
                    };
                    self.save_profile(&settings, &displays);
                }
            }
        }
    }

    /// `settings` become the saved profile of the monitors connected now.
    fn save_profile(&mut self, settings: &ProfileSettings, displays: &LinuxDisplaySnapshot) {
        let mut saved = false;
        self.linux_controls.edit_layout(|layout| saved = layout.save_profile(settings, displays));
        if !saved {
            log!("wm: display profile not saved: no monitor's card has a PCI address");
        }
    }

    /// A pacing the renderer took goes into the profile and is saved, with
    /// the kept configuration (the one a trial would go back to).
    fn save_pacing(&mut self, cx: &mut Cx, pacing: &DisplayPacing) {
        log!("wm: display pacing {pacing:?}");
        let displays = cx.linux_display_snapshot();
        let config = self.linux_controls.trials.trial.as_ref().map(|t| t.previous.clone())
            .unwrap_or_else(|| DisplayConfig::running(&displays));
        let settings = ProfileSettings { arrangement: config.arrangement, modes: config.modes, main: config.main, pacing: pacing.clone() };
        self.save_profile(&settings, &displays);
    }

    /// The renderer's answer to a display request: a refused part of a
    /// configuration being tried undoes it.
    fn display_result(&mut self, cx: &mut Cx, command: &LinuxDisplayCommand, result: &Result<(), String>) {
        match (command, result) {
            (LinuxDisplayCommand::Pacing(pacing), Ok(())) => self.save_pacing(cx, pacing),
            (LinuxDisplayCommand::Pacing(pacing), Err(message)) => {
                self.linux_controls.notice = format!("Pace by: {message}");
                log!("wm: display pacing {pacing:?} refused: {message}");
            }
            (LinuxDisplayCommand::Virtual(_), Err(message)) => {
                self.linux_controls.notice = format!("Desktop: {message}");
                log!("wm: virtual display refused: {message}");
            }
            (LinuxDisplayCommand::Mode { .. } | LinuxDisplayCommand::Arrangement(_) | LinuxDisplayCommand::Source(_), Err(message)) => {
                log!("wm: display: {command:?} refused: {message}");
                let out = self.linux_controls.trials.refused(message, Cx::monotonic_now());
                self.apply_trial_out(cx, out);
            }
            (_, Ok(())) => return,
        }
        self.poll_linux_controls(cx);
        self.redraw_all(cx);
    }

    /// The saved virtual desktop, once at start-up, unless the environment
    /// pins it or the person chose the desktop first. Returns whether it
    /// asked the renderer for anything.
    fn restore_virtual(&mut self, cx: &mut Cx, displays: &LinuxDisplaySnapshot) -> bool {
        if displays.outputs.is_empty() {
            // No inventory yet: wait.
            return false;
        }
        self.linux_controls.virtual_restore_done = true;
        let pinned = virtual_env_pinned(
            std::env::var("MAKEPAD_VIRTUAL_DISPLAY").ok().as_deref(),
            std::env::var("MAKEPAD_WM_VIRTUAL_FROM_SAVED").ok().as_deref(),
        );
        let layout = self.linux_controls.layout.clone().unwrap_or_default();
        if !displays.direct || pinned || self.linux_controls.virtual_user_set || virtual_restore(&layout, displays).is_none() {
            return false;
        }
        let Some(saved) = layout.virtual_display else { return false };
        log!("wm: applying saved virtual display");
        if let Err(message) = self.send_virtual(cx, &saved) {
            log!("wm: saved virtual display refused: {message}");
        }
        true
    }

    pub(crate) fn shutdown_linux_controls(&mut self) {
        if let Some(mut controller) = self.linux_controls.controller.take() { controller.shutdown(); }
        self.linux_controls.waiting.clear();
        self.linux_controls.dpi_save = None;
        self.linux_controls.layout_save = None;
        self.linux_controls.layout_save_seq = None;
        self.linux_controls.pointer_save = [None; 2];
        // `restart_requested` is left as it is: `Event::Shutdown`'s glue
        // reads it right after this call to pick the exit code.
    }

    /// Whether `Event::Shutdown` should exit 75 (a confirmed restart)
    /// instead of 0 (an ordinary quit).
    pub(crate) fn restart_requested(&self) -> bool {
        self.linux_controls.restart_requested
    }

    /// The headset as the worker last published it, with the edits still
    /// waiting for it: the WM test service's `headset_status`.
    pub(crate) fn headset_status_text(&self) -> Option<String> {
        let controller = self.linux_controls.controller.as_ref()?;
        Some(shell::blackshark::device_linux::describe(&controller.snapshot().headset, &controller.headset_pending()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(text: &str) -> DisplayLayout {
        DisplayLayout::parse(text)
    }

    #[test]
    fn exit_code_is_75_only_for_a_requested_restart() {
        assert_eq!(exit_code(false), 0);
        assert_eq!(exit_code(true), 75);
    }

    #[test]
    fn restart_gate_only_for_a_saved_gpu_that_is_present() {
        let gpu = GpuInfo {
            card: "card1".into(),
            pci: "0000:01:00.0".into(),
            vendor: "1234".into(),
            device: "5678".into(),
            driver: "nvidia".into(),
            connected: Vec::new(),
        };
        // Auto is always present: a restart cannot be stuck on a GPU.
        assert!(restart_gate(None, &[]));
        assert!(restart_gate(Some("0000:01:00.0 1234:5678"), std::slice::from_ref(&gpu)));
        // A moved card or another model at that address: the next start
        // would only fall back to Auto, so the restart is refused.
        assert!(!restart_gate(Some("0000:02:00.0 1234:5678"), std::slice::from_ref(&gpu)));
        assert!(!restart_gate(Some("0000:01:00.0 1234:5679"), &[gpu]));
    }

    #[test]
    fn restart_times_out_past_the_bound_since_it_was_armed() {
        assert!(!restart_timed_out(0.0, 0.0));
        assert!(!restart_timed_out(0.0, RESTART_SAVE_TIMEOUT));
        assert!(restart_timed_out(0.0, RESTART_SAVE_TIMEOUT + 0.001));
        // The bound is since the restart was armed, not since the epoch.
        assert!(!restart_timed_out(100.0, 100.0 + RESTART_SAVE_TIMEOUT - 0.001));
        assert!(restart_timed_out(100.0, 100.0 + RESTART_SAVE_TIMEOUT + 0.001));
    }

    #[test]
    fn layout_save_waits_for_the_start_up_read_and_merges_it() {
        let mut controls = LinuxControls::default();
        // A GPU switch committed before the worker's read landed.
        controls.gpu_user_set = true;
        controls.edit_layout(|l| l.render_on = Some("0000:01:00.0 1234:5678".to_string()));
        assert_eq!(controls.take_layout_save(), None);
        let saved = layout("render-on 0000:00:02.0 8086:a780\nvirtual 1920x1080@60\n");
        controls.adopt_loaded_layout(&saved);
        let queued = controls.take_layout_save().expect("merged layout re-queued");
        assert_eq!(queued.render_on, Some("0000:01:00.0 1234:5678".to_string()));
        assert_eq!(queued.virtual_display, saved.virtual_display);
        assert_eq!(controls.take_layout_save(), None);
    }

    #[test]
    fn untouched_read_is_not_saved_back() {
        let mut controls = LinuxControls::default();
        controls.adopt_loaded_layout(&layout("render-on 0000:00:02.0 8086:a780\n"));
        assert_eq!(controls.take_layout_save(), None);
        assert_eq!(controls.layout, Some(layout("render-on 0000:00:02.0 8086:a780\n")));
    }
}
