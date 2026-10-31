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

/// How far a fingertip may be from a card to touch it (m).
pub const POKE_M: f32 = 0.015;

impl Card {
    /// Standing up from the left controller's top (or the left wrist of a tracked hand), tilted
    /// back toward the eyes.
    pub fn wrist(c: &XrController, h: &XrHand, size: Vec2f) -> Option<Card> {
        let pose = if c.active() { c.grip_pose } else { h.joint_pose_checked(XrHand::WRIST)? };
        let q = pose.orientation;
        let right = q.rotate_vec3(&vec3f(1.0, 0.0, 0.0));
        let fwd = q.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        let up0 = q.rotate_vec3(&vec3f(0.0, 1.0, 0.0));
        // tilted 0.5 rad back toward the eyes
        let (s, k) = (0.5f32.sin(), 0.5f32.cos());
        let up = (up0 * k - fwd * s).normalize();
        let normal = Vec3f::cross(right, up).normalize();
        let center = pose.position + up0 * 0.05 + up * (size.y * 0.5) + fwd * 0.01;
        Some(Card { center, right, up, normal, size })
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
        let denom = dir.dot(self.normal);
        if denom.abs() < 1e-5 {
            return None;
        }
        let t = (self.center - origin).dot(self.normal) / denom;
        if t < 0.0 {
            return None;
        }
        let p = origin + dir * t - self.center;
        let (u, v) = (p.dot(self.right) / self.size.x + 0.5, 0.5 - p.dot(self.up) / self.size.y);
        ((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)).then(|| (vec2f(u, v), t))
    }

    /// Where a fingertip touches the card (within [`POKE_M`] of its plane).
    pub fn poke(&self, tip: Vec3f) -> Option<Vec2f> {
        let d = (tip - self.center).dot(self.normal);
        if d.abs() > POKE_M {
            return None;
        }
        self.hit(tip + self.normal * 0.02, self.normal * -1.0).map(|(uv, _)| uv)
    }

    /// Where a hand points at the card: its aim ray, else (a tracked hand) its index tip.
    pub fn pointed(&self, c: &XrController, h: &XrHand) -> Option<Vec2f> {
        if let Some(a) = input::aim(c, h) {
            let (o, d) = input::ray(&a);
            if let Some((uv, _)) = self.hit(o, d) {
                return Some(uv);
            }
        }
        if c.active() {
            return None;
        }
        self.poke(h.tip_pos_checked(XrHand::INDEX_TIP)?)
    }
}
