//! Orbit camera + empty stage for Asset UI / VJ mesh preview.
//!
//! Viewers do not own a GameWorld. They pass a [`PreviewLook`] and optional
//! ground/sky flags; the renderer builds the one-frame dummy the existing
//! scene pass still wants.

use makepad_draw::*;
use makepad_game_sim::{BodyKind, Entity, GameWorld, SkyConfig};

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

/// Optional ground slab + default sky for a statue/character stage.
#[derive(Clone, Copy, Debug)]
pub struct PreviewStage {
    pub ground: bool,
    pub sky: bool,
    pub ground_half: f32,
    pub ground_color: Vec4f,
    /// Near-black ground + night sky + dim sun. Independent of CSM.
    pub dark: bool,
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
        }
    }

    pub fn empty() -> Self {
        Self {
            ground: false,
            sky: false,
            ground_half: 8.0,
            ground_color: vec4(0.0, 0.0, 0.0, 1.0),
            dark: false,
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

fn preview_world(look: PreviewLook, stage: PreviewStage) -> GameWorld {
    let mut world = GameWorld::new();
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
            collide: true,
            scale: vec3f(1.0, 1.0, 1.0),
            scale_target: vec3f(1.0, 1.0, 1.0),
            ..Default::default()
        }];
        world.next_id = 2;
    } else {
        world.entities.clear();
    }
    world.sky = if !stage.sky {
        None
    } else if stage.dark {
        Some(SkyConfig {
            top: vec4(0.008, 0.01, 0.02, 1.0),
            horizon: vec4(0.03, 0.035, 0.05, 1.0),
            ground: vec4(0.02, 0.022, 0.025, 1.0),
            ground_bottom: vec4(0.01, 0.011, 0.013, 1.0),
            fog: 0.01,
        })
    } else {
        Some(SkyConfig::default())
    };
    if stage.dark {
        world.sun.color = Some(vec3f(0.05, 0.055, 0.07));
        world.sun.ambient = Some(vec3f(0.035, 0.035, 0.04));
        world.sun.shadow_alpha = Some(0.85);
    }
    world.terrain = None;
    world.cam_target = look.target;
    world.cam_distance = look.distance;
    world.cam_fov = look.fov;
    world.mark_render_dirty();
    world
}

impl Renderer {
    /// Draw models + optional skinned batch on a preview stage.
    /// No caller-owned GameWorld.
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
        let world = preview_world(look, stage);
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
