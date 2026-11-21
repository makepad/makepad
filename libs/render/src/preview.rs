//! Orbit camera and stage for mesh previews.
//!
//! Viewers supply a [`PreviewLook`] and a [`PreviewStage`]. The renderer
//! builds the world for the scene pass.

use makepad_draw::*;
use makepad_scene::{BodyKind, Entity, World, SkyConfig};

use crate::{SceneDraws, Renderer, SkinnedBatch};

pub const TICK_DT: f32 = 1.0 / 60.0;

/// Orbit camera around a look-at point. Yaw/pitch match [`CameraRig`].
#[derive(Clone, Copy, Debug)]
pub struct PreviewLook {
    pub target: Vec3f,
    pub distance: f32,
    pub fov: f32,
    pub yaw: f32,
    pub pitch: f32,
}

impl Default for PreviewLook {
    fn default() -> Self {
        Self {
            target: vec3f(0.0, 0.9, 0.0),
            distance: 4.2,
            fov: 45.0,
            yaw: 0.6,
            pitch: -0.22,
        }
    }
}

/// Ground, backdrop and lighting for a preview.
#[derive(Clone, Copy, Debug)]
pub struct PreviewStage {
    pub ground: bool,
    pub sky: bool,
    pub ground_half: f32,
    pub ground_color: Vec4f,
    /// Near-black ground + night sky. The SUN IS UNCHANGED: this is a dark
    /// backdrop for judging a lit model, not a night scene. Independent of
    /// CSM.
    pub dark: bool,
    /// Direction from the subject toward the key light in camera coordinates.
    /// +X points right, +Y up and +Z toward the eye.
    pub key_dir_view: Option<Vec3f>,
    /// Neutral ambient intensity applied equally to the sky and ground hemispheres.
    pub fill: Option<f32>,
    /// Flat RGBA backdrop. When set, the pass owner clears to this color and
    /// the renderer skips the sky dome. The ground slab has its own switch.
    pub backdrop_color: Option<Vec4f>,
}

impl PreviewStage {
    pub fn statue() -> Self {
        Self {
            ground: true,
            sky: true,
            // Wide enough that a walk-map still sees a horizon plane.
            ground_half: 256.0,
            ground_color: vec4(0.32, 0.38, 0.34, 1.0),
            dark: false,
            key_dir_view: None,
            fill: None,
            backdrop_color: None,
        }
    }

    pub fn empty() -> Self {
        Self {
            ground: false,
            sky: false,
            ground_half: 8.0,
            ground_color: vec4(0.0, 0.0, 0.0, 1.0),
            dark: false,
            key_dir_view: None,
            fill: None,
            backdrop_color: None,
        }
    }
}

pub fn preview_scene_state(look: PreviewLook, rect: Rect, time: f64) -> Option<SceneState3D> {
    if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
        return None;
    }
    let distance = look.distance.max(0.5);
    let pitch = look.pitch.clamp(-1.45, 1.45);
    let yaw = look.yaw;
    let forward = vec3f(
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    )
    .normalize();
    let camera_pos = look.target - forward * distance;
    let view = Mat4f::look_at(camera_pos, look.target, vec3f(0.0, 1.0, 0.0));
    let aspect = (rect.size.x / rect.size.y).max(0.001) as f32;
    let projection = Mat4f::perspective(look.fov.clamp(20.0, 120.0), aspect, 0.15, 500.0);
    Some(SceneState3D {
        time,
        camera_pos,
        view,
        projection,
        viewport_rect: rect,
    })
}

fn preview_world(look: PreviewLook, stage: PreviewStage, view: &Mat4f) -> World {
    let mut world = World::new();
    if stage.ground {
        let ground_color = if stage.dark {
            vec4(0.025, 0.028, 0.032, 1.0)
        } else {
            stage.ground_color
        };
        world.entities = vec![Entity {
            id: 1,
            kind: BodyKind::Static,
            pos: vec3f(0.0, -0.25, 0.0),
            half: vec3f(stage.ground_half, 0.25, stage.ground_half),
            color: ground_color,
            scale: vec3f(1.0, 1.0, 1.0),
            ..Default::default()
        }];
    } else {
        world.entities.clear();
    }
    world.sky = if !stage.sky || stage.backdrop_color.is_some() {
        None
    } else if stage.dark {
        Some(SkyConfig {
            top: vec4(0.008, 0.01, 0.02, 1.0),
            horizon: vec4(0.03, 0.035, 0.05, 1.0),
            ground: vec4(0.02, 0.022, 0.025, 1.0),
            ground_bottom: vec4(0.01, 0.011, 0.013, 1.0),
            fog: 0.01,
            ..SkyConfig::default()
        })
    } else {
        Some(SkyConfig::default())
    };
    if let Some(dir) = stage.key_dir_view {
        // The view rotation is orthonormal, so its transpose maps back to world space.
        let world_dir = view.transpose().transform_vec4(vec4(dir.x, dir.y, dir.z, 0.0));
        world.sun.dir = Some(world_dir.to_vec3f().normalize());
    }
    if let Some(fill) = stage.fill {
        world.sun.ambient = Some(Vec3f::all(fill));
    }
    if stage.dark {
        // Only the cast shadow deepens a little on the near-black slab; the
        // subject keeps its full studio lighting.
        world.sun.shadow_alpha = Some(0.85);
    }
    world.terrain = None;
    world.camera.target = look.target;
    world.camera.distance = look.distance;
    world.camera.fov = look.fov;
    world.mark_render_dirty();
    world
}

impl Renderer {
    /// Draw models + optional skinned batch on a preview stage.
    /// No caller-owned World.
    pub fn draw_preview(
        &mut self,
        cx: &mut Cx3d,
        draw_list: &mut DrawList,
        draws: &mut SceneDraws,
        look: PreviewLook,
        stage: PreviewStage,
        scene_state: SceneState3D,
        skinned: Option<SkinnedBatch>,
        models_draw: Option<&mut crate::DrawSceneSkinned>,
    ) -> crate::RenderStats {
        let world = preview_world(look, stage, &scene_state.view);
        self.draw_scene_full(
            cx,
            draw_list,
            draws,
            &world,
            scene_state,
            skinned,
            models_draw,
        )
    }
}
