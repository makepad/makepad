//! A hand's controls the same way for a controller and a tracked hand: the aim (the
//! controller's aim pose, a hand's system aim), the trigger (an index pinch), the grip (a middle
//! pinch), with press and release thresholds, and the face buttons' press edges.

use makepad_widgets::makepad_platform::event::xr::{XrController, XrHand, XrState};
use makepad_widgets::*;

/// The press and release thresholds of grips and triggers.
pub const PRESS: f32 = 0.55;
pub const RELEASE: f32 = 0.35;

/// The hand's aim pose: the controller's, else a tracked hand's when the system gives one.
pub fn aim(c: &XrController, h: &XrHand) -> Option<Pose> {
    if c.active() {
        Some(c.aim_pose)
    } else if h.aim_valid() {
        Some(h.aim_pose)
    } else {
        None
    }
}

/// An aim pose's ray: (origin, unit direction).
pub fn ray(p: &Pose) -> (Vec3f, Vec3f) {
    (p.position, p.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0)))
}

/// The trigger is down (`down`: it was, so the release threshold applies), or the index pinch.
pub fn trigger(c: &XrController, h: &XrHand, down: bool) -> bool {
    if c.active() {
        c.trigger >= if down { RELEASE } else { PRESS }
    } else {
        h.pinch_index()
    }
}

/// A hand's grabbing pose while it grabs: the controller's grip, or a tracked hand's middle
/// pinch (`down`: it was grabbing, so the release threshold applies).
pub fn grip_pose(c: &XrController, h: &XrHand, down: bool) -> Option<Pose> {
    if c.active() {
        let t = if down { RELEASE } else { PRESS };
        return (c.grip >= t).then_some(c.grip_pose);
    }
    h.pinch_middle().then(|| h.tracking_pose()).flatten()
}

/// The controllers' face buttons, as press edges per frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Buttons {
    last: [u16; 2],
    /// This frame's presses (left, right): `XrController::CLICK_*` bits.
    pub pressed: [u16; 2],
}

impl Buttons {
    pub fn step(&mut self, s: &XrState) -> [u16; 2] {
        for (i, c) in [&s.left_controller, &s.right_controller].into_iter().enumerate() {
            let b = if c.active() { c.buttons } else { 0 };
            self.pressed[i] = b & !self.last[i];
            self.last[i] = b;
        }
        self.pressed
    }

    pub fn hit(&self, hand: usize, bit: u16) -> bool {
        self.pressed[hand & 1] & bit != 0
    }
}

/// A trigger's press edge (with its hysteresis).
#[derive(Clone, Copy, Debug, Default)]
pub struct Trigger {
    down: bool,
}

impl Trigger {
    /// True on the frame the trigger (or index pinch) goes down.
    pub fn pressed(&mut self, c: &XrController, h: &XrHand) -> bool {
        let down = trigger(c, h, self.down);
        let pressed = down && !self.down;
        self.down = down;
        pressed
    }

    pub fn down(&self) -> bool {
        self.down
    }
}
