//! A streamed level's solid geometry, as the few sim queries that must see
//! it are allowed to see it.
//!
//! A `game.map` level is thousands of welded triangles. They are NOT bodies
//! in this sim (no box approximates them, and mirroring them into box3d
//! would put a level's worth of mesh into every snapshot of the rollback
//! ring), so by default every sim query passes clean through a map's walls.
//! The host resolves walkers against the level after each tick; that leaves
//! two consumers that need the level DURING a query rather than after it:
//!
//! - the raycast suspension of a wheeled body, which must find the map's
//!   floor under each wheel the way it finds the terrain heightfield;
//! - the third-person camera boom, which must stop at a ceiling or a wall
//!   the way it stops at a placed box.
//!
//! Both ask the same two questions — "where does this ray hit" and "what
//! floor does this spot belong to" — and this trait is exactly those. The
//! implementation lives with the level (the render crate's
//! `LevelCollision`); the sim only holds an `Arc<dyn LevelSolid>` on the
//! world while a map is installed. Determinism is unaffected: every peer
//! carries the same level, and the queries are pure functions of it.

use makepad_math::*;
use std::sync::Arc;

/// A wheeled chassis may body-step at least this high.  Classic 8/16/24
/// unit risers import as 0.25/0.50/0.75 metres, so this deliberately treats
/// the whole staircase as a friendly ramp rather than asking wheel radius
/// or suspension to climb a square edge.
pub const WHEELED_STEP_MIN: f32 = 0.8;

/// A static level is allowed to stop or slide a body, never catapult it.
/// This is only a final numerical/bounce allowance; body-step itself adds
/// no vertical velocity.
pub const LEVEL_CONTACT_UP_SPEED_MAX: f32 = 1.0;

/// The driveable-step contract. Wheels may be more forgiving than feet,
/// but can never make a staircase undriveable when it is walkable.
pub fn wheeled_max_step(walkable_step: f32) -> f32 {
    if walkable_step.is_finite() {
        walkable_step.max(WHEELED_STEP_MIN)
    } else {
        WHEELED_STEP_MIN
    }
}

pub trait LevelSolid: Send + Sync {
    /// Nearest level surface along `dir` (unit length) from `from`, within
    /// `max`: `(distance, normal)`, the normal facing back along the ray.
    fn ray_hit(&self, from: Vec3f, dir: Vec3f, max: f32) -> Option<(f32, Vec3f)>;

    /// The floor a body at `(x, near_y, z)` belongs on: the level surface
    /// under that spot with standing room above it, nearest to `near_y`
    /// (a body spawned INSIDE a raised floor is lifted onto it; one spawned
    /// in the air drops to the floor below). `None` where the level has no
    /// floor at all — outside the map.
    fn ground_under(&self, x: f32, z: f32, near_y: f32) -> Option<f32>;

    /// The room around `at`: `(headroom, span)` — floor-to-ceiling height
    /// and the narrowest straight horizontal line through the spot at
    /// bumper height. `None` when the spot is not under a ceiling (open
    /// ground), which is the answer "anything fits".
    fn room_at(&self, at: Vec3f) -> Option<(f32, f32)>;

    /// Standing height of the level's own declared body (its importer's
    /// walker config) — the map's species yardstick. The reference human
    /// when the level declares nothing.
    fn body_height(&self) -> f32 {
        1.75
    }

    /// A thick ray: the centre ray plus four rays offset `radius` across
    /// the two axes perpendicular to `dir`, nearest hit wins. What a camera
    /// lens needs — a point ray between two wall edges reports free space
    /// where a lens of any width would be inside the plaster.
    fn swept_hit(&self, from: Vec3f, dir: Vec3f, max: f32, radius: f32) -> Option<f32> {
        let mut best = self.ray_hit(from, dir, max).map(|(t, _)| t);
        if radius > 0.0 {
            // Any vector not parallel to `dir` seeds the perpendicular frame.
            let seed = if dir.y.abs() < 0.9 {
                vec3f(0.0, 1.0, 0.0)
            } else {
                vec3f(1.0, 0.0, 0.0)
            };
            let u = crate::vec3_normalize(Vec3f::cross(dir, seed));
            let v = crate::vec3_normalize(Vec3f::cross(dir, u));
            for off in [u * radius, u * -radius, v * radius, v * -radius] {
                if let Some((t, _)) = self.ray_hit(from + off, dir, max) {
                    if best.map_or(true, |b| t < b) {
                        best = Some(t);
                    }
                }
            }
        }
        best
    }
}

/// The shared handle the world carries. Cloning a world clones the pointer:
/// a level is immutable while installed, so every snapshot sees one level.
pub type LevelSolidRef = Arc<dyn LevelSolid>;

#[cfg(test)]
pub(crate) mod test_level {
    use super::*;

    #[test]
    fn driveable_step_is_never_lower_than_walkable_step() {
        for walkable in [0.0, 0.35, 0.55, 0.8, 1.1] {
            assert!(wheeled_max_step(walkable) >= walkable);
        }
        assert!(wheeled_max_step(f32::NAN) >= WHEELED_STEP_MIN);
    }

    /// A test level made of axis-aligned planes: floors (`y = h`, facing
    /// up and down) and walls (`x = c` / `z = c`). Enough to pin the sim's
    /// use of the trait without the render crate.
    #[derive(Default)]
    pub struct Planes {
        pub floors: Vec<f32>,
        pub walls_x: Vec<f32>,
        pub walls_z: Vec<f32>,
    }

    impl LevelSolid for Planes {
        fn ray_hit(&self, from: Vec3f, dir: Vec3f, max: f32) -> Option<(f32, Vec3f)> {
            let mut best: Option<(f32, Vec3f)> = None;
            let mut consider = |t: f32, n: Vec3f| {
                if t >= 0.0 && t <= max && best.map_or(true, |(b, _)| t < b) {
                    best = Some((t, n));
                }
            };
            for &h in &self.floors {
                if dir.y.abs() > 1.0e-6 {
                    let t = (h - from.y) / dir.y;
                    consider(t, vec3f(0.0, -dir.y.signum(), 0.0));
                }
            }
            for &c in &self.walls_x {
                if dir.x.abs() > 1.0e-6 {
                    let t = (c - from.x) / dir.x;
                    consider(t, vec3f(-dir.x.signum(), 0.0, 0.0));
                }
            }
            for &c in &self.walls_z {
                if dir.z.abs() > 1.0e-6 {
                    let t = (c - from.z) / dir.z;
                    consider(t, vec3f(0.0, 0.0, -dir.z.signum()));
                }
            }
            best
        }

        fn ground_under(&self, _x: f32, _z: f32, near_y: f32) -> Option<f32> {
            // Nearest floor at or below near_y, else the lowest floor above.
            self.floors
                .iter()
                .copied()
                .filter(|h| *h <= near_y)
                .fold(None, |best: Option<f32>, h| Some(best.map_or(h, |b| b.max(h))))
                .or_else(|| {
                    self.floors
                        .iter()
                        .copied()
                        .fold(None, |best: Option<f32>, h| Some(best.map_or(h, |b| b.min(h))))
                })
        }

        fn room_at(&self, _at: Vec3f) -> Option<(f32, f32)> {
            None
        }
    }
}
