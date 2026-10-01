//! The pointer's field over a control, for a material that answers the
//! pointer's approach (a magnetic liquid whose spikes rise as it comes
//! near): a spring toward one while the pointer is within reach, the
//! pointer's place in the control's frame, and a clock that runs while the
//! field is up. And the arbiter that keeps the cost down: at any pointer
//! position only the nearest two controls carry a field, so one can fade
//! out while the next fades in and the rest stay still.
use crate::*;
use std::cell::RefCell;

/// The two nearest reports at a pointer position decide who carries the
/// field; the reports of the position before decide for the current one,
/// so every control sees the same answer whatever order they report in,
/// one pointer event late, which is never seen.
#[derive(Default)]
pub struct FieldArbiter {
    at: Option<DVec2>,
    cur: [(f64, u64); 2],
    cur_n: usize,
    prev: [(f64, u64); 2],
    prev_n: usize,
}

thread_local! {
    static ARBITER: RefCell<FieldArbiter> = RefCell::new(FieldArbiter::default());
}

impl FieldArbiter {
    /// A control reports its distance to the pointer at `abs` (points,
    /// zero inside it) under its own id, and learns whether it is one of
    /// the two nearest. The UI thread alone calls this; no lock is taken.
    pub fn report(abs: DVec2, id: u64, dist: f64) -> bool {
        ARBITER.with(|a| a.borrow_mut().report_in(abs, id, dist))
    }

    fn report_in(&mut self, abs: DVec2, id: u64, dist: f64) -> bool {
        let moved = match self.at {
            Some(p) => (p.x - abs.x).abs() > 0.01 || (p.y - abs.y).abs() > 0.01,
            None => true,
        };
        if moved {
            self.prev = self.cur;
            self.prev_n = self.cur_n;
            self.cur_n = 0;
            self.at = Some(abs);
        }
        // Keep the two smallest distances for this position.
        let mut slot = None;
        for i in 0..self.cur_n {
            if self.cur[i].1 == id {
                slot = Some(i);
            }
        }
        match slot {
            Some(i) => self.cur[i].0 = dist,
            None => {
                if self.cur_n < 2 {
                    self.cur[self.cur_n] = (dist, id);
                    self.cur_n += 1;
                } else {
                    let far = if self.cur[0].0 >= self.cur[1].0 { 0 } else { 1 };
                    if dist < self.cur[far].0 {
                        self.cur[far] = (dist, id);
                    }
                }
            }
        }
        // The decision: among the nearest two of the position before, or
        // nobody has reported yet.
        if self.prev_n < 2 {
            return true;
        }
        (0..self.prev_n).any(|i| self.prev[i].1 == id)
    }
}

/// The field itself, stepped on the frames after a kick until it rests.
#[derive(Default)]
pub struct PointerField {
    hover: f64,
    hover_v: f64,
    target: f64,
    pointer: Option<(f64, f64)>,
    time: f64,
    next_frame: Option<NextFrame>,
    last_time: Option<f64>,
}

impl PointerField {
    /// The pointer read against the control: `rel` its place in the
    /// control's frame in points from the centre, `near` whether it is
    /// within reach and the control may answer it. Asks for a frame when
    /// anything changed that the material would show.
    pub fn pointer(&mut self, cx: &mut Cx, rel: (f64, f64), near: bool) {
        let target = if near { 1.0 } else { 0.0 };
        let moved = match self.pointer {
            Some(p) => (p.0 - rel.0).abs() > 0.01 || (p.1 - rel.1).abs() > 0.01,
            None => true,
        };
        self.pointer = Some(rel);
        if target != self.target || (moved && self.strength(0.0) > 0.001) {
            self.target = target;
            self.kick(cx);
        }
    }

    /// The pointer left the window: the field falls.
    pub fn leave(&mut self, cx: &mut Cx) {
        if self.target != 0.0 {
            self.target = 0.0;
            self.kick(cx);
        }
    }

    /// Starts the frames, if they are not running.
    pub fn kick(&mut self, cx: &mut Cx) {
        if self.next_frame.is_none() {
            self.last_time = None;
            self.next_frame = Some(cx.new_next_frame());
        }
    }

    /// One step on the field's frame; true when a step ran, so the caller
    /// hands the material the new values. `press` is the control's own
    /// press 0..1, which joins the field.
    pub fn tick(&mut self, cx: &mut Cx, event: &Event, press: f64) -> bool {
        let Some(nf) = self.next_frame else {
            return false;
        };
        let Some(ne) = nf.is_event(event) else {
            return false;
        };
        self.next_frame = None;
        let dt = match self.last_time {
            Some(t) => (ne.time - t).clamp(0.001, 0.05),
            None => 1.0 / 60.0,
        };
        self.last_time = Some(ne.time);
        // Critically damped, so it settles without crossing zero.
        let a = 40.0 * (self.target - self.hover) - 12.7 * self.hover_v;
        self.hover_v += a * dt;
        self.hover += self.hover_v * dt;
        self.hover = self.hover.max(0.0);
        if self.strength(press) > 0.001 {
            self.time += dt;
        }
        let active = self.hover_v.abs() > 1e-3
            || (self.hover - self.target).abs() > 1e-3
            || self.strength(press) > 0.001;
        if active {
            self.next_frame = Some(cx.new_next_frame());
        }
        true
    }

    /// The field's strength: most of the hover plus the press, within 0..1.
    pub fn strength(&self, press: f64) -> f64 {
        (0.7 * self.hover + press).clamp(0.0, 1.0)
    }

    /// What the material reads: the strength, the pointer along and across
    /// the control from its centre, and the clock.
    pub fn read(&self, press: f64) -> (f32, f32, f32, f32) {
        let (a, c) = self.pointer.unwrap_or((0.0, 0.0));
        (self.strength(press) as f32, a as f32, c as f32, self.time as f32)
    }

    /// True while the field is up or moving.
    pub fn active(&self) -> bool {
        self.next_frame.is_some() || self.hover > 0.001
    }
}
