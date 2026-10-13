//! The direct desktop's event-fed inventory and pending control requests.
use crate::{cx::{Cx, OsType}, event::Event, linux_display::*};
use makepad_hal::{buffers::pack_strs, direct::DisplayInventory, CmdWriter, Id};
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct DisplayClient {
    pub(crate) window: Option<Id>,
    pub(crate) snapshot: LinuxDisplaySnapshot,
    pub(crate) generation: u64,
    /// Presentation phase on Cx::monotonic_now's clock, plus the refresh period.
    pub(crate) timings: Vec<Option<(f64, f64)>>,
    pub(crate) next_request: u64,
    pending: HashMap<u64, LinuxDisplayCommand>,
    queued: Vec<u64>,
}

impl DisplayClient {
    pub(super) fn flush(&mut self, w: &mut CmdWriter) {
        for req in self.queued.drain(..) {
            match &self.pending[&req] {
                LinuxDisplayCommand::Source(name) => w.direct_display_source(req, name.as_bytes()),
                LinuxDisplayCommand::Order(order) => {
                    let names: Vec<_> = order.iter().map(String::as_str).collect();
                    w.direct_display_order(req, names.len() as u32, &pack_strs(&names));
                }
                LinuxDisplayCommand::Arrangement(arrangement) => w.direct_display_arrangement(req, match arrangement {
                    makepad_hal::direct::DisplayArrangement::Extended => &[][..],
                    makepad_hal::direct::DisplayArrangement::Mirror { optimise_for } => optimise_for.as_bytes(),
                }),
                LinuxDisplayCommand::Pacing(pacing) => w.direct_display_pacing(req, &pack_strs(&pacing.args())),
                LinuxDisplayCommand::Mode { name, mode } => {
                    w.direct_display_mode(req, &pack_strs(&[name, mode.as_deref().unwrap_or("")]));
                }
            }
        }
    }
}

impl Cx {
    pub(crate) fn hal_queue_display(&mut self, command: LinuxDisplayCommand) -> Result<(), String> {
        if !matches!(self.os_type, OsType::LinuxDirect) {
            return Err("this session does not own direct displays".into());
        }
        command.validate()?;
        let display = &mut self.hal.direct_display;
        display.next_request += 1;
        display.pending.insert(display.next_request, command);
        display.queued.push(display.next_request);
        self.redraw_all();
        Ok(())
    }

    pub(crate) fn hal_display_inventory(&mut self, window: Id, generation: u64, inventory: &[u8]) {
        let Some(inventory) = DisplayInventory::read(inventory) else {
            crate::error!("malformed direct display inventory");
            return;
        };
        let rect = |r: [f64; 4]| crate::makepad_math::Rect {
            pos: crate::makepad_math::dvec2(r[0], r[1]), size: crate::makepad_math::dvec2(r[2], r[3]),
        };
        let names = inventory.snapshot.outputs.iter().filter(|o| o.desktop_position.is_some()).map(|o| o.name.clone());
        let mut screens: Vec<_> = names.zip(inventory.screens.into_iter().map(|s| crate::screen::ScreenGeom {
            bounds: rect(s.bounds), work_area: rect(s.work_area), is_primary: s.is_primary,
        })).collect();
        screens.sort_by(|a, b| a.1.bounds.pos.x.total_cmp(&b.1.bounds.pos.x));
        crate::screen::set_hal_direct_screens(Some(screens));
        let display = &mut self.hal.direct_display;
        display.window = (!window.is_none()).then_some(window);
        display.generation = generation;
        display.timings = vec![None; inventory.snapshot.outputs.len()];
        display.snapshot = inventory.snapshot;
        self.hal_sync_direct_scale();
        self.hal_repaint_all();
    }

    pub(crate) fn hal_display_timing(&mut self, window: Id, generation: u64, output: u32, last_present_ns: u64, interval: f64) {
        let display = &mut self.hal.direct_display;
        if display.window != Some(window) || display.generation != generation { return; }
        let Some(timing) = display.timings.get_mut(output as usize) else { return; };
        *timing = if last_present_ns != 0 && interval.is_finite() && interval > 0.0 {
            // HAL timestamps are absolute CLOCK_MONOTONIC; the program's Cx has
            // its own origin. Convert once at intake, retaining elapsed pipe time.
            let elapsed = makepad_hal::clock_ns().saturating_sub(last_present_ns) as f64 * 1e-9;
            Some((Self::monotonic_now() - elapsed, interval))
        } else { None };
    }

    /// Screen rects come in the shell's native points; the desktop window lays out in
    /// its own points once a DPI override applies. Keep `screens()` in window points.
    pub(crate) fn hal_sync_direct_scale(&self) {
        let Some(window) = self.hal.direct_display.window else { return; };
        if let Some(id) = self.windows.id_iter().find(|id| crate::event::hal_intake::hal_window_id(*id) == window) {
            let w = &self.windows[id];
            crate::screen::set_hal_direct_scale(w.native_dpi_factor() / w.effective_dpi_factor());
        }
    }

    pub(crate) fn hal_display_result(&mut self, request: u64, tag: u32, ok: bool, message: &[u8]) {
        let display = &mut self.hal.direct_display;
        if !display.pending.get(&request).is_some_and(|c| c.tag() as u32 == tag) { return; }
        let command = display.pending.remove(&request).unwrap();
        let result = if ok { Ok(()) } else { Err(String::from_utf8_lossy(message).into_owned()) };
        self.call_event_handler(&Event::LinuxDisplayResult(LinuxDisplayResultEvent { request, command, result }));
    }
}
