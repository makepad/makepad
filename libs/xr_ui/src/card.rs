//! A card in the room: a flat rectangle with a picture on its front. Where it stands (on the
//! left wrist, beside a hand facing the head, pinned in the room) and where a pointer meets it.

use crate::input;
use makepad_widgets::makepad_platform::event::xr::{XrController, XrHand};
use makepad_widgets::*;

/// A card's place: its centre, its unit axes (right, up; its front faces `normal`) and its size
/// (metres).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Card {
    pub center: Vec3f,
    pub right: Vec3f,
    pub up: Vec3f,
    pub normal: Vec3f,
    pub size: Vec2f,
}

/// How far a tip (a controller's or a fingertip) may be from a card to touch it (m).
pub const POKE_M: f32 = 0.015;
/// How far through a card's face a tip goes to press the cell it went through (m). It presses
/// again once it has come back out in front of the face.
pub const PUSH_M: f32 = 0.005;
/// How far a wrist card's bottom edge stands above the grip, clear of the controller's top (m).
const WRIST_CLEAR_M: f32 = 0.09;
/// How far toward the eyes a wrist card stands from the grip, with the controller behind it (m).
const WRIST_TO_EYES_M: f32 = 0.02;

impl Card {
    /// Standing above the left controller's top (or the left wrist of a tracked hand). Only its
    /// place follows the hand: its front faces the eyes and its top is up as the eyes see it,
    /// however the hand is turned, so it never shows its back or reads upside down.
    pub fn wrist(c: &XrController, h: &XrHand, head: Pose, size: Vec2f) -> Option<Card> {
        let pose = if c.active() { c.grip_pose } else { h.joint_pose_checked(XrHand::WRIST)? };
        let (_, up) = Self::to_eyes(pose.position, head);
        let center = pose.position + up * (WRIST_CLEAR_M + size.y * 0.5);
        let (normal, up) = Self::to_eyes(center, head);
        // toward the eyes till the controller stands behind it: along the normal, which stays
        let center = center + normal * (WRIST_TO_EYES_M - (center - pose.position).dot(normal));
        let right = Vec3f::cross(up, normal).normalize();
        Some(Card { center, right, up, normal, size })
    }

    /// From a point: the way to the eyes, and the eyes' up square to it (their forward where up
    /// is that same line: straight above or below a level head).
    fn to_eyes(from: Vec3f, head: Pose) -> (Vec3f, Vec3f) {
        let to_head = head.position - from;
        let normal = if to_head.length() > 1e-4 { to_head.normalize() } else { vec3f(0.0, 0.0, 1.0) };
        let square = |v: Vec3f| v - normal * v.dot(normal);
        let up = square(head.orientation.rotate_vec3(&vec3f(0.0, 1.0, 0.0)));
        let up = if up.length() > 1e-3 { up } else { square(head.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0))) };
        (normal, up.normalize())
    }

    /// Upright beside a point (a hand), turned to face the head about the vertical.
    pub fn beside(at: Vec3f, head: Vec3f, size: Vec2f) -> Card {
        let mut c = Card::facing(at, head, size);
        c.center = at + c.up * 0.06 + c.right * 0.02;
        c
    }

    /// Upright at `center`, turned to face the head about the vertical.
    pub fn facing(center: Vec3f, head: Vec3f, size: Vec2f) -> Card {
        let to_head = head - center;
        let flat = vec3f(to_head.x, 0.0, to_head.z);
        let normal = if flat.length() > 1e-4 { flat.normalize() } else { vec3f(0.0, 0.0, 1.0) };
        let up = vec3f(0.0, 1.0, 0.0);
        let right = Vec3f::cross(up, normal).normalize();
        Card { center, right, up, normal, size }
    }

    /// Upright at `center`, its front turned `yaw` from facing +Z.
    pub fn pinned(center: Vec3f, yaw: f32, size: Vec2f) -> Card {
        let normal = vec3f(yaw.sin(), 0.0, yaw.cos());
        let up = vec3f(0.0, 1.0, 0.0);
        let right = Vec3f::cross(up, normal).normalize();
        Card { center, right, up, normal, size }
    }

    /// The card's frame as a model matrix (x right, y up, z out of its front).
    pub fn matrix(&self) -> Mat4f {
        let (r, u, n, c) = (self.right, self.up, self.normal, self.center);
        Mat4f { v: [r.x, r.y, r.z, 0.0, u.x, u.y, u.z, 0.0, n.x, n.y, n.z, 0.0, c.x, c.y, c.z, 1.0] }
    }

    /// Where a ray meets the card's front: (u, v) from its top left in 0..1, and the distance.
    pub fn hit(&self, origin: Vec3f, dir: Vec3f) -> Option<(Vec2f, f32)> {
        let (uv, t) = self.project_ray(origin, dir)?;
        ((0.0..=1.0).contains(&uv.x) && (0.0..=1.0).contains(&uv.y)).then_some((uv, t))
    }

    /// A point projected onto the card's plane, including outside its edges (drag capture).
    pub fn project(&self, point: Vec3f) -> Vec2f {
        let p = point - self.center;
        vec2f(p.dot(self.right) / self.size.x + 0.5, 0.5 - p.dot(self.up) / self.size.y)
    }

    /// A ray projected onto the unbounded card plane (drag capture).
    pub fn project_ray(&self, origin: Vec3f, dir: Vec3f) -> Option<(Vec2f, f32)> {
        let denom = dir.dot(self.normal);
        if denom.abs() < 1e-5 {
            return None;
        }
        let t = (self.center - origin).dot(self.normal) / denom;
        if !t.is_finite() || t < 0.0 {
            return None;
        }
        Some((self.project(origin + dir * t), t))
    }

    /// Where a tip stands against the card: the (u, v) of its foot on the card's plane (outside
    /// 0..1 off the card) and its height above the front (m, below 0 behind it).
    pub fn touch(&self, tip: Vec3f) -> (Vec2f, f32) {
        (self.project(tip), (tip - self.center).dot(self.normal))
    }

    /// Where a tip touches the card (within [`POKE_M`] of its plane).
    pub fn poke(&self, tip: Vec3f) -> Option<Vec2f> {
        let (uv, d) = self.touch(tip);
        (d.abs() <= POKE_M && (0.0..=1.0).contains(&uv.x) && (0.0..=1.0).contains(&uv.y)).then_some(uv)
    }

    /// Where a hand points at the card: where its tip touches it (a controller's or a tracked
    /// hand's), else where its aim ray meets it.
    pub fn pointed(&self, c: &XrController, h: &XrHand) -> Option<Vec2f> {
        if let Some(uv) = input::tip(c, h).and_then(|t| self.poke(t.position)) {
            return Some(uv);
        }
        let (o, d) = input::ray(&input::aim(c, h)?);
        self.hit(o, d).map(|(uv, _)| uv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn about(axis: Vec3f, rad: f32) -> Quat {
        let (s, w) = ((rad * 0.5).sin(), (rad * 0.5).cos());
        Quat { x: axis.x * s, y: axis.y * s, z: axis.z * s, w }
    }

    fn level_head() -> Pose {
        Pose::new(about(vec3f(1.0, 0.0, 0.0), 0.0), vec3f(0.0, 1.6, 0.0))
    }

    fn controller(grip: Pose) -> XrController {
        XrController { buttons: XrController::ACTIVE, grip_pose: grip, ..Default::default() }
    }

    fn tracked_hand(wrist: Pose) -> XrHand {
        let mut h = XrHand { flags: XrHand::IN_VIEW, ..Default::default() };
        h.joints = [wrist; XrHand::JOINT_COUNT];
        h.joints[XrHand::CENTER].position = wrist.position + vec3f(0.0, 0.07, 0.0);
        h
    }

    fn assert_reads_from_the_eyes(card: &Card, head: Pose) {
        let to_eyes = (head.position - card.center).normalize();
        assert!(card.normal.dot(to_eyes) > 0.9999, "its front faces the eyes: {card:?}");
        assert!(Vec3f::cross(card.right, card.up).dot(card.normal) > 0.9999, "not mirrored: {card:?}");
        assert!(card.up.dot(card.normal).abs() < 1e-4, "its top lies in its plane: {card:?}");
    }

    #[test]
    fn a_wrist_card_faces_the_eyes_and_reads_upright_however_the_hand_is_turned() {
        let head = level_head();
        let held = vec3f(-0.2, 1.0, -0.35);
        let size = vec2f(0.2, 0.25);
        let none = XrHand::default();
        let first = Card::wrist(&controller(Pose::new(about(vec3f(1.0, 0.0, 0.0), 0.0), held)), &none, head, size).unwrap();
        assert_reads_from_the_eyes(&first, head);
        assert!(first.up.y > 0.5, "its top is up: {first:?}");
        assert!(first.right.x > 0.5, "its right is the viewer's right: {first:?}");
        for axis in [vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0), vec3f(0.6, 0.0, 0.8)] {
            for step in 0..12 {
                let grip = Pose::new(about(axis, step as f32 * std::f32::consts::TAU / 12.0), held);
                let card = Card::wrist(&controller(grip), &none, head, size).unwrap();
                assert_eq!(card, first, "the hand's turn moves nothing: {axis:?} step {step}");
            }
        }
    }

    #[test]
    fn a_wrist_card_stands_above_the_controller_and_goes_where_the_hand_goes() {
        let head = level_head();
        let held = vec3f(-0.2, 1.0, -0.35);
        let size = vec2f(0.2, 0.25);
        let none = XrHand::default();
        let grip = Pose::new(about(vec3f(1.0, 0.0, 0.0), 0.0), held);
        let card = Card::wrist(&controller(grip), &none, head, size).unwrap();
        assert!((card.center - held).dot(card.up) >= size.y * 0.5 + 0.05, "the bottom edge clears the controller: {card:?}");
        assert!((card.center - held).dot(card.normal) > 0.0, "the controller stays behind it: {card:?}");
        let moved = Pose::new(grip.orientation, held + vec3f(-0.1, 0.0, 0.0));
        let there = Card::wrist(&controller(moved), &none, head, size).unwrap();
        assert!(there.center.x < card.center.x - 0.05, "{there:?}");
        assert_reads_from_the_eyes(&there, head);
    }

    #[test]
    fn a_wrist_card_looked_down_on_has_its_top_away_from_the_viewer() {
        // eyes turned straight down at a hand below them
        let head = Pose::new(about(vec3f(1.0, 0.0, 0.0), -std::f32::consts::FRAC_PI_2), vec3f(0.0, 1.6, 0.0));
        let grip = Pose::new(about(vec3f(0.0, 0.0, 1.0), 1.0), vec3f(0.0, 1.0, 0.0));
        let card = Card::wrist(&controller(grip), &XrHand::default(), head, vec2f(0.2, 0.25)).unwrap();
        assert_reads_from_the_eyes(&card, head);
        assert!(card.up.z < -0.5, "{card:?}");
        assert!(card.right.x > 0.5, "{card:?}");
    }

    #[test]
    fn a_wrist_card_straight_below_a_level_head_has_a_whole_frame() {
        let head = level_head();
        let grip = Pose::new(about(vec3f(1.0, 0.0, 0.0), 0.3), vec3f(0.0, 1.0, 0.0));
        let card = Card::wrist(&controller(grip), &XrHand::default(), head, vec2f(0.2, 0.25)).unwrap();
        assert!(card.up.length().is_finite() && (card.up.length() - 1.0).abs() < 1e-4, "{card:?}");
        assert_reads_from_the_eyes(&card, head);
    }

    #[test]
    fn a_tracked_hand_turned_palm_up_gets_the_same_card_as_a_controller_at_its_wrist() {
        let head = level_head();
        let wrist = Pose::new(about(vec3f(0.0, 0.0, 1.0), std::f32::consts::PI), vec3f(-0.2, 1.0, -0.35));
        let size = vec2f(0.2, 0.25);
        let idle = XrController::default();
        let by_hand = Card::wrist(&idle, &tracked_hand(wrist), head, size).unwrap();
        let by_controller = Card::wrist(&controller(wrist), &XrHand::default(), head, size).unwrap();
        assert_eq!(by_hand, by_controller);
        assert!(by_hand.up.y > 0.5, "{by_hand:?}");
        assert_eq!(Card::wrist(&idle, &XrHand::default(), head, size), None, "no hand, no card");
    }
}
