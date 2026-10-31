//! GPU memory pressure: when the device's free memory falls under what the UI keeps for
//! itself, or a create fails for lack of room, the HAL tells every session (`GpuPressure`
//! event). The session owning the desktop (the WM) then asks other GPU users, such as the
//! local AI hub, for room; textures whose create failed wait for that room
//! (`Starved`) and are handed back to their app (`TextureEvicted`: make it again) once it is
//! there.
//!
//! This module is the backend-neutral bookkeeping; a backend feeds it its device budget
//! (Vulkan: VK_EXT_memory_budget) and carries out what it answers.

use crate::types::Id;
use std::collections::HashMap;

const MB: u64 = 1 << 20;
/// The reserve never asks for more than this...
pub const RESERVE_MAX: u64 = 1536 * MB;
/// ...nor more than this share of the device (a small GPU keeps a smaller reserve).
pub const RESERVE_SHARE: u64 = 10;
/// A shortage is over once this much more than the reserve is free (no flapping).
pub const RELIEF_MARGIN: u64 = 128 * MB;
/// A starved texture is handed back when its bytes plus this are free.
pub const ROOM_MARGIN: u64 = 64 * MB;
/// The last resort: after this long a starved texture is handed back anyway, so its app tries
/// again (and it starves again if there is still no room).
pub const STARVE_WAIT_NS: u64 = 5_000_000_000;
/// While short, the event is repeated this often (the desktop's owner escalates on repeats).
pub const REPEAT_NS: u64 = 2_000_000_000;
/// A shortage counts only once it has lasted this long (or a create failed):
/// a desktop starting or switching outputs is short for a second or two, and
/// acting on that made the AI hub rebuild its model for nothing.
pub const SUSTAIN_NS: u64 = 4_000_000_000;
/// How often the budget is looked at while nothing failed.
pub const CHECK_NS: u64 = 250_000_000;

/// What the UI keeps free on a device of `size` bytes: `MAKEPAD_GPU_UI_RESERVE_MB`, else
/// the smaller of [`RESERVE_MAX`] and a [`RESERVE_SHARE`]th of the device.
pub fn ui_reserve(size: u64) -> u64 {
    static ENV: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    let env = *ENV.get_or_init(|| std::env::var("MAKEPAD_GPU_UI_RESERVE_MB").ok().and_then(|v| v.trim().parse::<u64>().ok()));
    env.map_or_else(|| RESERVE_MAX.min(size / RESERVE_SHARE), |mb| mb * MB)
}

/// One `GpuPressure` event, in MiB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signal {
    pub wanted_mb: u32,
    pub free_mb: u32,
    pub budget_mb: u32,
    pub reserve_mb: u32,
    pub failed: bool,
}

fn mb32(bytes: u64) -> u32 {
    bytes.div_ceil(MB).min(u32::MAX as u64) as u32
}

#[derive(Default)]
pub struct Pressure {
    active: bool,
    sent_ns: u64,
    checked_ns: u64,
    /// A create failed since the last look: look now, and say so.
    failed: bool,
    /// A GPU submission ran out of memory and waits on its worker: ask for the whole reserve
    /// at the next look, whatever the budget says (the driver had no room for it).
    submit_starved: bool,
    /// Since when free memory has been under the reserve (None = not short).
    short_since: Option<u64>,
    /// Creates that failed for lack of room: (session, id) -> (bytes, since, free bytes the
    /// driver reported when it failed).
    starved: HashMap<(u32, Id), (u64, u64, u64)>,
}

impl Pressure {
    /// A create of `bytes` failed for lack of device memory with `free` bytes reported free;
    /// the texture waits for room. Room means free memory grew by its bytes since the failure:
    /// a full GPU still reports some memory free that it cannot allocate (an answer from the
    /// budget alone handed the texture back at once, and its app failed again every frame).
    pub fn starve(&mut self, sid: u32, id: Id, bytes: u64, now: u64, free: u64) {
        self.starved.insert((sid, id), (bytes, now, free));
        self.failed = true;
    }

    /// A submission (or its fence wait) ran out of device memory: look now and ask for room.
    pub fn submit_starved(&mut self) {
        self.submit_starved = true;
        self.failed = true;
    }

    /// The object went away (destroyed, or its session closed): nothing to hand back.
    pub fn forget(&mut self, sid: u32, id: Id) {
        self.starved.remove(&(sid, id));
    }

    pub fn forget_session(&mut self, sid: u32) {
        self.starved.retain(|k, _| k.0 != sid);
    }

    pub fn is_starved(&self, sid: u32, id: Id) -> bool {
        self.starved.contains_key(&(sid, id))
    }

    /// Whether a shortage of `short` bytes at `now` is to be acted on: a create
    /// failed, or it has lasted [`SUSTAIN_NS`]. The backend frees its own idle
    /// storage and [`Self::check`] raises the event only then.
    pub fn sustained(&mut self, now: u64, short: u64) -> bool {
        if short == 0 {
            self.short_since = None;
            return false;
        }
        let since = *self.short_since.get_or_insert(now);
        self.failed || now.saturating_sub(since) >= SUSTAIN_NS
    }

    /// Whether the backend should look at the budget now.
    pub fn due(&self, now: u64) -> bool {
        self.failed || now.saturating_sub(self.checked_ns) >= CHECK_NS
    }

    /// Whether the backend must look again in [`CHECK_NS`] even with nothing else due: a
    /// shortage is on, or a texture waits.
    pub fn waiting(&self) -> bool {
        self.active || self.short_since.is_some() || !self.starved.is_empty()
    }

    pub fn starved_bytes(&self) -> u64 {
        self.starved.values().map(|(b, _, _)| *b).sum()
    }

    /// Bytes the device is short of: the reserve plus what starved creates need, minus free.
    pub fn shortfall(&self, free: u64, reserve: u64) -> u64 {
        (reserve + self.starved_bytes()).saturating_sub(free)
    }

    /// One look at the device: `free`/`budget` from the driver, `reserve` the UI's. Answers
    /// the starved textures to hand back (room is there, or they waited long enough) and the
    /// event to send, if any. `freed` is what the backend could free itself first (idle
    /// targets): the event asks only for the rest.
    pub fn check(&mut self, now: u64, free: u64, budget: u64, reserve: u64, freed: u64) -> (Vec<(u32, Id)>, Option<Signal>) {
        self.checked_ns = now;
        let failed = std::mem::take(&mut self.failed);
        let free = free + freed;
        let mut room = free.saturating_sub(ROOM_MARGIN);
        let mut back = Vec::new();
        let mut waiting: Vec<_> = self.starved.iter().map(|(k, v)| (*k, *v)).collect();
        waiting.sort_by_key(|(_, (_, since, _))| *since);
        for (key, (bytes, since, failed_free)) in waiting {
            if bytes <= room && free >= failed_free.saturating_add(bytes) {
                room -= bytes;
                back.push(key);
            } else if now.saturating_sub(since) >= STARVE_WAIT_NS {
                back.push(key);
            }
        }
        for key in &back {
            self.starved.remove(key);
        }
        let wanted = self.shortfall(free, reserve).max(if std::mem::take(&mut self.submit_starved) { reserve } else { 0 });
        // (before `sustained` was asked, a shortage counts at once)
        let held = self.short_since.is_none_or(|since| now.saturating_sub(since) >= SUSTAIN_NS);
        let signal = |wanted: u64, failed| Signal { wanted_mb: mb32(wanted), free_mb: mb32(free), budget_mb: mb32(budget), reserve_mb: mb32(reserve), failed };
        let event = if wanted > 0 {
            if !held && !failed {
                None
            } else if !self.active || failed || now.saturating_sub(self.sent_ns) >= REPEAT_NS {
                self.active = true;
                self.sent_ns = now;
                Some(signal(wanted, failed))
            } else {
                None
            }
        } else if self.active && free >= reserve + self.starved_bytes() + RELIEF_MARGIN {
            self.active = false;
            self.sent_ns = now;
            Some(signal(0, false))
        } else {
            None
        };
        (back, event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const GB: u64 = 1 << 30;

    #[test]
    fn the_reserve_is_the_smaller_of_its_cap_and_a_tenth() {
        assert_eq!(ui_reserve(24 * GB), RESERVE_MAX);
        assert_eq!(ui_reserve(4 * GB), 4 * GB / 10);
    }

    #[test]
    fn a_short_blip_is_ignored_a_sustained_shortage_is_not() {
        let mut p = Pressure::default();
        // a desktop switch: short for 2 s, then fine -- nothing is freed, nothing is asked
        assert!(!p.sustained(0, 500 * MB));
        assert_eq!(p.check(0, 1000 * MB, 20 * GB, 1536 * MB, 0).1, None);
        assert!(!p.sustained(2_000_000_000, 500 * MB));
        assert_eq!(p.check(2_000_000_000, 1000 * MB, 20 * GB, 1536 * MB, 0).1, None);
        assert!(!p.sustained(2_500_000_000, 0));
        // short again, and it lasts: acted on after SUSTAIN_NS
        let t = 10_000_000_000;
        assert!(!p.sustained(t, 500 * MB));
        assert_eq!(p.check(t, 1000 * MB, 20 * GB, 1536 * MB, 0).1, None);
        assert!(p.sustained(t + SUSTAIN_NS, 500 * MB));
        assert_eq!(p.check(t + SUSTAIN_NS, 1000 * MB, 20 * GB, 1536 * MB, 0).1.unwrap().wanted_mb, 536);
        // a failed create is acted on at once
        let mut p = Pressure::default();
        p.starve(1, Id::new(1, 1), 100 * MB, 0, 0);
        assert!(p.sustained(1, 600 * MB));
        assert!(p.check(1, 1000 * MB, 20 * GB, 1536 * MB, 0).1.unwrap().failed);
    }

    #[test]
    fn pressure_is_raised_repeated_while_short_and_relieved_with_margin() {
        let mut p = Pressure::default();
        let (back, e) = p.check(0, 1000 * MB, 20 * GB, 1536 * MB, 0);
        assert!(back.is_empty());
        let e = e.unwrap();
        assert_eq!((e.wanted_mb, e.free_mb, e.reserve_mb, e.failed), (536, 1000, 1536, false));
        // the same shortage again: quiet
        assert_eq!(p.check(CHECK_NS, 1000 * MB, 20 * GB, 1536 * MB, 0).1, None);
        // later: said again (the desktop's owner takes its next step)
        assert_eq!(p.check(REPEAT_NS + 1, 800 * MB, 20 * GB, 1536 * MB, 0).1.unwrap().wanted_mb, 736);
        // back at the reserve but inside the margin: still short as far as the UI cares
        assert_eq!(p.check(2 * REPEAT_NS, 1536 * MB + MB, 20 * GB, 1536 * MB, 0).1, None);
        let e = p.check(3 * REPEAT_NS, 1536 * MB + RELIEF_MARGIN, 20 * GB, 1536 * MB, 0).1.unwrap();
        assert_eq!(e.wanted_mb, 0, "relieved");
        assert_eq!(p.check(4 * REPEAT_NS, 4 * GB, 20 * GB, 1536 * MB, 0).1, None);
    }

    #[test]
    fn what_the_backend_freed_itself_is_not_asked_for() {
        let mut p = Pressure::default();
        assert_eq!(p.check(0, 1000 * MB, 20 * GB, 1536 * MB, 600 * MB).1, None);
    }

    #[test]
    fn a_failed_create_waits_for_room_then_goes_back_to_its_app() {
        let mut p = Pressure::default();
        let id = Id::new(7, 1);
        p.starve(1, id, 300 * MB, 0, 0);
        assert!(p.due(1) && p.is_starved(1, id));
        // no room: kept, and the event says it failed and counts the texture in
        let (back, e) = p.check(1, 200 * MB, 20 * GB, 100 * MB, 0);
        assert!(back.is_empty());
        let e = e.unwrap();
        assert!(e.failed && e.wanted_mb == 200, "{e:?}");
        assert!(p.waiting());
        // the hub gave room: handed back
        let (back, _) = p.check(CHECK_NS, 600 * MB, 20 * GB, 100 * MB, 0);
        assert_eq!(back, vec![(1, id)]);
        assert!(!p.is_starved(1, id));
    }

    #[test]
    fn a_full_gpu_that_reports_some_memory_free_keeps_the_texture_waiting() {
        let mut p = Pressure::default();
        let id = Id::new(4, 1);
        // the driver refused 50 MB with 184 MB "free": that is no room
        p.starve(1, id, 50 * MB, 0, 184 * MB);
        assert!(p.check(1, 184 * MB, 194 * MB, 1536 * MB, 0).0.is_empty());
        assert!(p.check(CHECK_NS, 200 * MB, 210 * MB, 1536 * MB, 0).0.is_empty());
        // free grew by the texture's bytes (and the margin): handed back
        assert_eq!(p.check(2 * CHECK_NS, 300 * MB, 310 * MB, 1536 * MB, 0).0, vec![(1, id)]);
    }

    #[test]
    fn a_starved_texture_is_handed_back_after_the_wait_as_a_last_resort() {
        let mut p = Pressure::default();
        let id = Id::new(3, 2);
        p.starve(2, id, 900 * MB, 0, 0);
        assert!(p.check(1, 100 * MB, 20 * GB, 0, 0).0.is_empty());
        assert_eq!(p.check(STARVE_WAIT_NS, 100 * MB, 20 * GB, 0, 0).0, vec![(2, id)]);
        p.starve(2, id, 900 * MB, 0, 0);
        p.forget_session(2);
        assert_eq!(p.starved_bytes(), 0);
    }
}
