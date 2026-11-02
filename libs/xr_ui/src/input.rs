//! Shared controller and tracked-hand interaction, with hysteresis and press ownership.
//!
//! OpenXR defines the grip pose at the palm, with -Z along the handle, and the aim pose
//! at the front pointing point, with -Z in the pointing direction. The controller tip is
//! exactly that aim pose: no device-independent distance from the grip is specified.
//! A tracked hand touches with its index fingertip, turned as its palm; its pinch grabs
//! at the thumb/index midpoint. A controller grabs with either trigger or grip.

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

/// The interaction point. See the module's pose definitions.
pub fn tip(c: &XrController, h: &XrHand) -> Option<Pose> {
    if c.active() {
        return c.aim_pose.is_finite().then_some(c.aim_pose);
    }
    let palm = h.tracking_pose()?;
    // XrHand packs each tip as its end joint plus its length, not as a joint index.
    let at = h.tip_active(XrHand::INDEX_TIP)
        .then(|| h.tip_pos_checked(XrHand::INDEX_TIP)).flatten()
        .or_else(|| h.pinch_anchor_pose().map(|p| p.position))?;
    Some(Pose::new(palm.orientation, at))
}

/// Both hands' tips (left, right).
pub fn tips(s: &XrState) -> [Option<Pose>; 2] {
    [tip(&s.left_controller, &s.left_hand), tip(&s.right_controller, &s.right_hand)]
}

/// The grab point while either controller trigger or grip holds, or a hand pinches (`down`:
/// it was grabbing, so the release threshold applies). The held thing follows this pose.
pub fn grab(c: &XrController, h: &XrHand, down: bool) -> Option<Pose> {
    if c.active() {
        let t = if down { RELEASE } else { PRESS };
        return (c.grip >= t || c.trigger >= t).then(|| tip(c, h)).flatten();
    }
    h.pinch_index().then(|| h.pinch_anchor_pose()).flatten()
}

/// Takes the grab (and with it a tracked hand's trigger, the same pinch) from a hand's input:
/// for the parts of an app that must not see a hand another part already holds things with.
pub fn mask_grab(c: &mut XrController, h: &mut XrHand) {
    c.grip = 0.0;
    c.trigger = 0.0;
    h.flags &= !XrHand::PINCH_INDEX;
}

/// Synthetic palm placement only: about one hand-width behind the front pointing point.
/// This is illustrative synthetic data, not a measured controller offset or a runtime correction.
const GRIP_BEHIND_AIM_M: f32 = 0.10;

/// Puts a synthetic controller's interaction tip exactly at `at`, preserving aim orientation.
pub fn place_tip(c: &mut XrController, at: Vec3f) {
    let ahead = c.aim_pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
    c.grip_pose.position = at - ahead * GRIP_BEHIND_AIM_M;
    c.aim_pose.position = at;
}

/// The controllers' face buttons, as press edges per frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Buttons {
    last: [u16; 2],
    /// This frame's presses (left, right): `XrController::CLICK_*` bits.
    pub pressed: [u16; 2],
    /// Interaction owners set this before later consumers handle the same press.
    claimed: [bool; 2],
    /// Trigger/grip holds, independent of which consumer claimed the press.
    held: [bool; 2],
}

impl Buttons {
    pub fn step(&mut self, s: &XrState) -> [u16; 2] {
        self.claimed = [false; 2];
        for (i, (c, h)) in [(&s.left_controller, &s.left_hand), (&s.right_controller, &s.right_hand)].into_iter().enumerate() {
            self.held[i] = grab(c, h, self.held[i]).is_some();
        }
        for (i, c) in [&s.left_controller, &s.right_controller].into_iter().enumerate() {
            let b = if c.active() { c.buttons } else { 0 };
            self.pressed[i] = b & !self.last[i];
            self.last[i] = b;
        }
        self.pressed
    }

    /// Claim through the release frame. Consumers still advance their edge detectors on
    /// the original input, so a held press cannot reappear when another owner releases it.
    pub fn claim(&mut self, hand: usize) {
        self.claimed[hand & 1] = true;
    }

    pub fn claimed(&self, hand: usize) -> bool {
        self.claimed[hand & 1]
    }

    /// Any trigger/grip or index pinch still held, including presses owned by a menu or
    /// selection. A hover claim alone is not an interaction hold.
    pub fn any_held(&self) -> bool {
        self.held.into_iter().any(|held| held)
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
