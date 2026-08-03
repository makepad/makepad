//! The stage: how one device projects the shared world into its own space.
//!
//! game.md §"Presentation modes" — the sim runs in world units and never
//! knows how it is being looked at. Each device picks a [`Stage`]: a flat
//! window, a 1:1 VR world you stand inside, or a scaled diorama anchored to
//! the floor of the room you can still see. Two headsets in one room may run
//! different stages over the same replicated world.
//!
//! Implementation note (this is the load-bearing design decision): the stage
//! is a *draw-list uniform*, not a view matrix and not a per-instance
//! transform. Every game shader computes
//! `draw_list.view_transform * transform`, so setting the scene draw list's
//! view transform once applies the stage to sky, terrain, cubes and skinned
//! characters together — for free, and without invalidating the packed
//! static instance slabs (which hold stage-independent model transforms).
//!
//! The view matrix is *not* available for this: in XR the platform overwrites
//! `camera_view`/`camera_view_r` with the runtime's eye matrices every frame
//! (see `platform/src/os/linux/openxr_opengl.rs`), so anything the app wrote
//! there would be discarded.

use makepad_draw::*;

/// How the world is presented on this device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StageMode {
    /// Desktop/mobile window: the world at 1:1, framed by the game camera.
    #[default]
    Flat,
    /// VR with no passthrough: 1:1, the player stands inside the world, and
    /// the game supplies the whole environment (sky, fog, terrain).
    VrFullScale,
    /// Mixed reality: the world shrunk onto the real floor. The room is the
    /// environment, so the game's sky and terrain are suppressed.
    MrDiorama,
}

impl StageMode {
    /// Whether the game draws its own environment. False in MR — the room
    /// already has one, and a sky dome would paint over the passthrough.
    pub fn shows_environment(self) -> bool {
        !matches!(self, StageMode::MrDiorama)
    }

    /// Whether this mode wants passthrough enabled on the headset.
    pub fn wants_passthrough(self) -> bool {
        matches!(self, StageMode::MrDiorama)
    }
}

/// Default diorama scale: a 40-unit race track lands at ~2m across, which
/// sits on a living-room floor without the player having to back away.
pub const DIORAMA_SCALE: f32 = 1.0 / 20.0;

/// Where and how big the world appears on this device.
///
/// `origin`/`yaw` are in *tracking* space (the room); `scale` converts world
/// units to room metres. Flat and VR keep the identity mapping.
#[derive(Clone, Copy, Debug)]
pub struct Stage {
    pub mode: StageMode,
    /// Anchor point in the room that world-origin maps to.
    pub origin: Vec3f,
    /// Rotation of the world about the room's up axis, radians.
    pub yaw: f32,
    /// World units → room metres.
    pub scale: f32,
}

impl Default for Stage {
    fn default() -> Self {
        Self::flat()
    }
}

impl Stage {
    pub fn flat() -> Self {
        Self {
            mode: StageMode::Flat,
            origin: vec3f(0.0, 0.0, 0.0),
            yaw: 0.0,
            scale: 1.0,
        }
    }

    /// 1:1, player inside the world. The head pose is the camera, so the
    /// stage stays identity and the runtime's eye matrices do the work.
    pub fn vr_full_scale() -> Self {
        Self {
            mode: StageMode::VrFullScale,
            origin: vec3f(0.0, 0.0, 0.0),
            yaw: 0.0,
            scale: 1.0,
        }
    }

    /// A diorama planted at `origin` in the room (typically on the floor,
    /// in front of the player).
    pub fn mr_diorama(origin: Vec3f, yaw: f32, scale: f32) -> Self {
        Self {
            mode: StageMode::MrDiorama,
            origin,
            yaw,
            scale: scale.max(1.0e-4),
        }
    }

    pub fn shows_environment(&self) -> bool {
        self.mode.shows_environment()
    }

    /// The world→room matrix: translate(origin) · rotate_y(yaw) · scale.
    ///
    /// Column-major, matching `Mat4f::rotation`'s layout — the same
    /// convention the renderer uses when it hand-builds instance transforms.
    pub fn matrix(&self) -> Mat4f {
        let (s, c) = (self.yaw.sin(), self.yaw.cos());
        let k = self.scale;
        let mut m = Mat4f::identity();
        // rotate_y · uniform scale
        m.v[0] = c * k;
        m.v[2] = -s * k;
        m.v[5] = k;
        m.v[8] = s * k;
        m.v[10] = c * k;
        // translation
        m.v[12] = self.origin.x;
        m.v[13] = self.origin.y;
        m.v[14] = self.origin.z;
        m
    }

    /// A world-space point as it appears in the room.
    pub fn world_to_stage(&self, p: Vec3f) -> Vec3f {
        let (s, c) = (self.yaw.sin(), self.yaw.cos());
        let k = self.scale;
        vec3f(
            (p.x * c + p.z * s) * k + self.origin.x,
            p.y * k + self.origin.y,
            (-p.x * s + p.z * c) * k + self.origin.z,
        )
    }

    /// The inverse: a room point (a controller tip, a gaze hit) back into
    /// world units, so XR input can address game entities.
    pub fn stage_to_world(&self, p: Vec3f) -> Vec3f {
        let (s, c) = (self.yaw.sin(), self.yaw.cos());
        let inv_k = 1.0 / self.scale;
        let d = vec3f(
            (p.x - self.origin.x) * inv_k,
            (p.y - self.origin.y) * inv_k,
            (p.z - self.origin.z) * inv_k,
        );
        vec3f(d.x * c - d.z * s, d.y, d.x * s + d.z * c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3f, b: Vec3f) -> bool {
        (a.x - b.x).abs() < 1.0e-4 && (a.y - b.y).abs() < 1.0e-4 && (a.z - b.z).abs() < 1.0e-4
    }

    #[test]
    fn flat_and_vr_are_identity_mappings() {
        for stage in [Stage::flat(), Stage::vr_full_scale()] {
            let p = vec3f(3.0, -2.0, 7.5);
            assert!(close(stage.world_to_stage(p), p));
            assert!(close(stage.stage_to_world(p), p));
        }
    }

    #[test]
    fn diorama_scales_and_anchors() {
        // 20:1 shrink, planted 1.5m in front of the player on the floor.
        let origin = vec3f(0.0, 0.0, -1.5);
        let stage = Stage::mr_diorama(origin, 0.0, DIORAMA_SCALE);
        // The world origin lands exactly on the anchor.
        assert!(close(stage.world_to_stage(vec3f(0.0, 0.0, 0.0)), origin));
        // A 40-unit track is 2m across.
        let edge = stage.world_to_stage(vec3f(20.0, 0.0, 0.0));
        assert!((edge.x - origin.x - 1.0).abs() < 1.0e-4, "{edge:?}");
        // Height shrinks by the same factor: a 10-unit tower is 50cm.
        let top = stage.world_to_stage(vec3f(0.0, 10.0, 0.0));
        assert!((top.y - 0.5).abs() < 1.0e-4, "{top:?}");
    }

    #[test]
    fn diorama_yaw_rotates_about_the_anchor() {
        let origin = vec3f(1.0, 0.0, 2.0);
        // Quarter turn: +x in world should swing to -z in the room.
        let stage = Stage::mr_diorama(origin, std::f32::consts::FRAC_PI_2, 1.0);
        let p = stage.world_to_stage(vec3f(1.0, 0.0, 0.0));
        assert!(close(p, vec3f(1.0, 0.0, 1.0)), "{p:?}");
    }

    #[test]
    fn stage_round_trips_through_its_inverse() {
        let stage = Stage::mr_diorama(vec3f(0.3, 1.1, -1.7), 0.9, DIORAMA_SCALE);
        for p in [
            vec3f(0.0, 0.0, 0.0),
            vec3f(12.0, 3.0, -8.0),
            vec3f(-40.0, 0.5, 25.0),
        ] {
            let back = stage.stage_to_world(stage.world_to_stage(p));
            assert!(close(back, p), "{p:?} -> {back:?}");
        }
    }

    #[test]
    fn matrix_agrees_with_world_to_stage() {
        // The GPU uses matrix(); CPU-side picking uses world_to_stage(). If
        // they ever disagree, a controller ray stops matching what is drawn.
        let stage = Stage::mr_diorama(vec3f(-0.4, 0.8, -2.0), 0.6, DIORAMA_SCALE);
        let m = stage.matrix();
        for p in [
            vec3f(0.0, 0.0, 0.0),
            vec3f(5.0, 2.0, -3.0),
            vec3f(-11.0, 7.0, 13.0),
        ] {
            let v = m.transform_vec4(vec4f(p.x, p.y, p.z, 1.0));
            let by_matrix = vec3f(v.x, v.y, v.z);
            assert!(
                close(by_matrix, stage.world_to_stage(p)),
                "{p:?}: {by_matrix:?} vs {:?}",
                stage.world_to_stage(p)
            );
        }
    }

    #[test]
    fn mr_suppresses_the_game_environment_and_asks_for_passthrough() {
        assert!(!StageMode::MrDiorama.shows_environment());
        assert!(StageMode::MrDiorama.wants_passthrough());
        for mode in [StageMode::Flat, StageMode::VrFullScale] {
            assert!(mode.shows_environment());
            assert!(!mode.wants_passthrough());
        }
    }
}
