//! Where the app's frame time goes, by what its passes draw for: the headset's eyes (an XR pass
//! and the passes under it), the desktop's windows (their passes and the passes under them), and
//! other offscreen passes (no window: a stream picture's pack, a bake). Per class the passes' GPU
//! time (every pass is a frame packet of its own, so its `FrameDone` GPU time is its own), and the
//! CPU time of the app's draws. For a stream's diagnostics: "the desktop view takes 9 ms of the
//! GPU per headset frame".
//!
//! Off until asked (`Cx::set_pass_load_enabled`); on, a pass adds one entry to a small queue
//! that its `FrameDone` takes back.
// (the web has no HAL frames: nothing there paints or finishes a pass through this)
#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_mut))]

use std::collections::VecDeque;

/// What a pass draws for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassClass {
    /// The headset's eyes: an XR pass and the passes it samples.
    Eyes = 0,
    /// A desktop window: its pass and the passes it samples (a 3D view, a mirror).
    Desktop = 1,
    /// A pass under no window (a stream picture's pack, a bake).
    Offscreen = 2,
}

/// The load since the last take.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PassLoadSample {
    /// The time covered (s).
    pub secs: f64,
    /// GPU time per class (`PassClass` order), ms in all.
    pub gpu_ms: [f64; 3],
    /// Passes painted per class.
    pub passes: [u32; 3],
    /// Passes whose GPU time the backend did not report (a backend without timestamps).
    pub untimed: u32,
    /// The app's draws and their CPU time (ms in all).
    pub draws: u32,
    pub draw_ms: f64,
}

impl PassLoadSample {
    /// GPU ms per second of a class.
    pub fn gpu_ms_per_s(&self, class: PassClass) -> f64 {
        if self.secs > 0.0 { self.gpu_ms[class as usize] / self.secs } else { 0.0 }
    }

    /// The draws' CPU ms per second.
    pub fn draw_ms_per_s(&self) -> f64 {
        if self.secs > 0.0 { self.draw_ms / self.secs } else { 0.0 }
    }
}

/// Packets whose `FrameDone` is still to come, kept at most (older ones are forgotten).
const PENDING: usize = 256;

#[derive(Default)]
pub(crate) struct PassLoad {
    on: bool,
    /// (packet serial, class) of the passes painted and not done yet.
    pending: VecDeque<(u64, PassClass)>,
    acc: PassLoadSample,
    #[cfg(not(target_arch = "wasm32"))]
    since: Option<std::time::Instant>,
    #[cfg(not(target_arch = "wasm32"))]
    draw_at: Option<std::time::Instant>,
}

impl PassLoad {
    pub(crate) fn enabled(&self) -> bool {
        self.on
    }

    pub(crate) fn set_enabled(&mut self, on: bool) {
        if on != self.on {
            *self = PassLoad { on, ..Default::default() };
        }
    }

    /// Packet `serial` paints a pass of `class`.
    pub(crate) fn painted(&mut self, serial: u64, class: PassClass) {
        if !self.on {
            return;
        }
        if self.pending.len() >= PENDING {
            self.pending.pop_front();
        }
        self.pending.push_back((serial, class));
        self.acc.passes[class as usize] += 1;
    }

    /// Packet `serial` is done on the GPU after `gpu_ns` (0: not measured).
    pub(crate) fn done(&mut self, serial: u64, gpu_ns: u64) {
        if !self.on {
            return;
        }
        let Some(i) = self.pending.iter().position(|(s, _)| *s == serial) else { return };
        let (_, class) = self.pending.remove(i).unwrap();
        if gpu_ns > 0 {
            self.acc.gpu_ms[class as usize] += gpu_ns as f64 / 1e6;
        } else {
            self.acc.untimed += 1;
        }
    }

    pub(crate) fn draw_begin(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.on {
            self.draw_at = Some(std::time::Instant::now());
        }
    }

    pub(crate) fn draw_end(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(at) = self.draw_at.take().filter(|_| self.on) {
            self.acc.draws += 1;
            self.acc.draw_ms += at.elapsed().as_secs_f64() * 1000.0;
        }
    }

    pub(crate) fn take(&mut self) -> PassLoadSample {
        let mut out = std::mem::take(&mut self.acc);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let now = std::time::Instant::now();
            out.secs = self.since.map_or(0.0, |t| (now - t).as_secs_f64());
            self.since = Some(now);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_time_lands_in_its_class() {
        let mut l = PassLoad::default();
        l.painted(1, PassClass::Desktop);
        assert_eq!(l.take().passes, [0; 3], "off: nothing counted");
        l.set_enabled(true);
        l.painted(1, PassClass::Desktop);
        l.painted(2, PassClass::Eyes);
        l.painted(3, PassClass::Offscreen);
        l.done(2, 7_000_000);
        l.done(1, 9_000_000);
        l.done(3, 0);
        l.done(99, 5_000_000);
        let s = l.take();
        assert_eq!(s.gpu_ms, [7.0, 9.0, 0.0]);
        assert_eq!(s.passes, [1, 1, 1]);
        assert_eq!(s.untimed, 1);
        assert_eq!(l.take().gpu_ms, [0.0; 3], "taken: starts again");
    }
}
