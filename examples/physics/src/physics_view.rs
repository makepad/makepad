use makepad_physics::{PhysicsOp, PhysicsWorld};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PhysicsViewBase = #(PhysicsView::register_widget(vm))

    mod.widgets.PhysicsView = set_type_default() do mod.widgets.PhysicsViewBase{
        width: Fill
        height: Fill
        draw_bg +: {
            color: #x131922
            draw_depth: -99.0
        }
        draw_pbr +: {
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.25
            spec_power: 128.0
            spec_strength: 0.9
        }
    }
}

// --- Cube colors (one per body, cycling) ---

const CUBE_COLORS: &[[f32; 3]] = &[
    [0.90, 0.30, 0.25], // red
    [0.25, 0.75, 0.45], // green
    [0.30, 0.50, 0.90], // blue
    [0.95, 0.75, 0.20], // yellow
    [0.80, 0.40, 0.85], // purple
    [0.20, 0.80, 0.80], // cyan
    [0.95, 0.55, 0.25], // orange
    [0.60, 0.85, 0.35], // lime
];

const GROUND_COLOR: [f32; 3] = [0.35, 0.38, 0.42];

// --- PhysicsView widget ---

#[derive(Script, ScriptHook, Widget)]
pub struct PhysicsView {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[redraw]
    #[live]
    draw_pbr: DrawPbr,
    #[live]
    draw_list_3d: DrawList2d,
    #[live(45.0)]
    camera_fov_y: f32,
    #[live(18.0)]
    camera_distance: f32,
    #[live(1.0)]
    camera_distance_min: f32,
    #[live(80.0)]
    camera_distance_max: f32,
    #[live(0.1)]
    wheel_zoom_step: f32,
    #[live(0.05)]
    camera_near: f32,
    #[live(200.0)]
    camera_far: f32,
    #[live(vec2(0.6, 0.98))]
    depth_range: Vec2f,
    #[live(0.7)]
    depth_forward_bias: f32,
    #[rust]
    cube_mesh: Option<usize>,
    #[rust]
    world: Option<PhysicsWorld>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    time: f64,
    #[rust]
    area: Area,
    #[rust(0.6)]
    orbit_yaw: f32,
    #[rust(0.4)]
    orbit_pitch: f32,
    #[rust]
    drag_last_abs: Option<DVec2>,
    #[rust]
    initialized: bool,
}

impl PhysicsView {
    fn ensure_initialized(&mut self, cx: &mut Cx2d) {
        if self.initialized {
            return;
        }
        self.initialized = true;

        // Upload a unit cube mesh (half-extent = 1.0, centered at origin).
        // We scale it per-body using the model transform.
        let positions: Vec<[f32; 3]> = UNIT_CUBE_POSITIONS.to_vec();
        let normals: Vec<[f32; 3]> = UNIT_CUBE_NORMALS.to_vec();
        let indices: Vec<u32> = UNIT_CUBE_INDICES.to_vec();
        match self.draw_pbr.upload_indexed_triangles_mesh(
            cx,
            &positions,
            Some(&normals),
            None,
            None,
            None,
            &indices,
        ) {
            Ok(handle) => self.cube_mesh = Some(handle),
            Err(e) => log!("Failed to upload cube mesh: {}", e),
        }

        // Create physics world and spawn cubes
        let mut world = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), 1.0 / 60.0);

        let mut ops = Vec::new();
        let grid = 5;
        let half = 0.5f32;
        let spacing = 1.1f32;
        for y in 0..grid {
            for x in 0..grid {
                for z in 0..grid {
                    ops.push(PhysicsOp::SpawnDynamic {
                        position: vec3f(
                            (x as f32 - grid as f32 / 2.0 + 0.5) * spacing,
                            2.0 + y as f32 * spacing,
                            (z as f32 - grid as f32 / 2.0 + 0.5) * spacing,
                        ),
                        half_extents: vec3f(half, half, half),
                        velocity: Vec3f::default(),
                        density: 1.0,
                    });
                }
            }
        }
        world.step(&ops);
        self.world = Some(world);
    }

    fn draw_scene(&mut self, cx: &mut Cx2d, rect: Rect) {
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return;
        }
        let pass_size = cx.current_pass_size();
        if pass_size.x <= 1.0 || pass_size.y <= 1.0 {
            return;
        }
        let Some(draw_list_id) = cx.get_current_draw_list_id() else {
            return;
        };
        let Some(cube_mesh) = self.cube_mesh else {
            return;
        };

        // Compute view/projection (orbit camera, same pattern as GltfView)
        let pass_id = cx.draw_lists[draw_list_id].draw_pass_id.unwrap();
        let pass_from_world = Mat4f::mul(
            &cx.passes[pass_id].pass_uniforms.camera_projection,
            &cx.passes[pass_id].pass_uniforms.camera_view,
        );
        let pass_from_world_inv = pass_from_world.invert();

        let clip_ndc = clip_ndc_for_rect(rect, pass_size);
        let viewport = clip_space_viewport_matrix(clip_ndc);
        let aspect = (rect.size.x / rect.size.y).max(0.001) as f32;
        let fov = self.camera_fov_y.clamp(1.0, 179.0);
        let near = self.camera_near.max(0.001);
        let far = self.camera_far.max(near + 0.001);
        let projection = Mat4f::perspective(fov, aspect, near, far);

        let camera_target = vec3(0.0, 3.0, 0.0);
        let distance = self.camera_distance.max(0.001);
        let yaw = self.orbit_yaw;
        let pitch = self.orbit_pitch.clamp(-1.45, 1.45);
        let cos_pitch = pitch.cos();
        let camera_pos = vec3(
            distance * yaw.sin() * cos_pitch,
            distance * pitch.sin(),
            distance * yaw.cos() * cos_pitch,
        ) + camera_target;
        let view = Mat4f::look_at(camera_pos, camera_target, vec3(0.0, 1.0, 0.0));

        let clip_from_world = Mat4f::mul(&viewport, &Mat4f::mul(&view, &projection));
        let draw_list_view = Mat4f::mul(&pass_from_world_inv, &clip_from_world);
        cx.draw_lists[draw_list_id]
            .draw_list_uniforms
            .view_transform = draw_list_view;

        self.draw_pbr.set_clip_ndc(clip_ndc);
        self.draw_pbr
            .set_depth_range(self.depth_range.x, self.depth_range.y);
        self.draw_pbr
            .set_depth_forward_bias(self.depth_forward_bias);
        self.draw_pbr.set_view_projection(view, projection);
        self.draw_pbr.camera_pos = camera_pos;

        // No textures — pure material colors
        self.draw_pbr.set_base_color_texture(None);
        self.draw_pbr.set_metal_roughness_texture(None);
        self.draw_pbr.set_normal_texture(None);
        self.draw_pbr.set_occlusion_texture(None);
        self.draw_pbr.set_emissive_texture(None);

        // Set up default env cubemap
        let env_tex = self.draw_pbr.default_env_texture(cx);
        self.draw_pbr.set_env_texture(Some(env_tex));

        // Draw ground plane (large flat cube)
        {
            let ground_transform = Mat4f {
                v: [
                    50.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.0, 0.0, 0.0, 0.0, 50.0, 0.0, 0.0, -0.1, 0.0,
                    1.0,
                ],
            };
            self.draw_pbr.set_transform(ground_transform);
            self.draw_pbr.set_base_color_factor(vec4(
                GROUND_COLOR[0],
                GROUND_COLOR[1],
                GROUND_COLOR[2],
                1.0,
            ));
            self.draw_pbr.set_metal_roughness(0.0, 0.85);
            let _ = self.draw_pbr.draw_mesh(cx, cube_mesh);
        }

        // Draw physics bodies
        self.draw_pbr.set_metal_roughness(0.0, 0.55);

        if let Some(world) = &self.world {
            for (i, body) in world.bodies.iter().enumerate() {
                let color = CUBE_COLORS[i % CUBE_COLORS.len()];
                let he = body.half_extents;
                let pose_mat = body.pose.to_mat4();

                // model = pose * scale(he.x, he.y, he.z)
                let model = Mat4f {
                    v: [
                        pose_mat.v[0] * he.x,
                        pose_mat.v[1] * he.x,
                        pose_mat.v[2] * he.x,
                        pose_mat.v[3],
                        pose_mat.v[4] * he.y,
                        pose_mat.v[5] * he.y,
                        pose_mat.v[6] * he.y,
                        pose_mat.v[7],
                        pose_mat.v[8] * he.z,
                        pose_mat.v[9] * he.z,
                        pose_mat.v[10] * he.z,
                        pose_mat.v[11],
                        pose_mat.v[12],
                        pose_mat.v[13],
                        pose_mat.v[14],
                        pose_mat.v[15],
                    ],
                };

                self.draw_pbr.set_transform(model);
                self.draw_pbr
                    .set_base_color_factor(vec4(color[0], color[1], color[2], 1.0));
                let _ = self.draw_pbr.draw_mesh(cx, cube_mesh);
            }
        }
    }
}

impl Widget for PhysicsView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits_with_capture_overload(cx, self.area, true) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.drag_last_abs = Some(fe.abs);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(fe) => {
                if let Some(last_abs) = self.drag_last_abs {
                    let delta = fe.abs - last_abs;
                    let sensitivity = 0.01_f32;
                    self.orbit_yaw -= (delta.x as f32) * sensitivity;
                    self.orbit_pitch =
                        (self.orbit_pitch + (delta.y as f32) * sensitivity).clamp(-1.45, 1.45);
                    self.drag_last_abs = Some(fe.abs);
                    self.area.redraw(cx);
                }
            }
            Hit::FingerScroll(fs) => {
                let scroll = if fs.scroll.y.abs() > f64::EPSILON {
                    fs.scroll.y
                } else {
                    fs.scroll.x
                };
                let step = self.wheel_zoom_step.max(0.001);
                let zoom_factor = if scroll > 0.0 {
                    1.0 / (1.0 - step)
                } else {
                    1.0 - step
                };
                self.camera_distance = (self.camera_distance * zoom_factor)
                    .clamp(self.camera_distance_min.max(0.01), self.camera_distance_max);
                self.area.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                if self.drag_last_abs.take().is_some() {
                    if fe.is_over {
                        cx.set_cursor(MouseCursor::Grab);
                    } else {
                        cx.set_cursor(MouseCursor::Default);
                    }
                }
            }
            Hit::FingerHoverIn(_) => {
                if self.drag_last_abs.is_some() {
                    cx.set_cursor(MouseCursor::Grabbing);
                } else {
                    cx.set_cursor(MouseCursor::Grab);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.drag_last_abs.is_none() {
                    cx.set_cursor(MouseCursor::Default);
                }
            }
            _ => {}
        }

        // Step physics and redraw every frame
        if let Event::NextFrame(ne) = event {
            self.time = ne.time;
            if let Some(world) = &mut self.world {
                world.step(&[]);
            }
            self.area.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
        if let Event::Startup = event {
            self.next_frame = cx.new_next_frame();
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_initialized(cx);

        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();

        self.draw_list_3d.begin_always(cx);
        self.draw_scene(cx, rect);
        self.draw_list_3d.end(cx);
        DrawStep::done()
    }
}

// --- Helper functions (from GltfView pattern) ---

fn clip_ndc_for_rect(rect: Rect, pass_size: Vec2d) -> Vec4f {
    let pass_w = pass_size.x.max(1.0) as f32;
    let pass_h = pass_size.y.max(1.0) as f32;
    let x0 = (2.0 * rect.pos.x as f32 / pass_w) - 1.0;
    let x1 = (2.0 * (rect.pos.x + rect.size.x) as f32 / pass_w) - 1.0;
    let y0 = 1.0 - (2.0 * rect.pos.y as f32 / pass_h);
    let y1 = 1.0 - (2.0 * (rect.pos.y + rect.size.y) as f32 / pass_h);
    vec4(x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
}

fn clip_space_viewport_matrix(clip_ndc: Vec4f) -> Mat4f {
    let sx = (clip_ndc.z - clip_ndc.x) * 0.5;
    let sy = (clip_ndc.w - clip_ndc.y) * 0.5;
    let tx = (clip_ndc.z + clip_ndc.x) * 0.5;
    let ty = (clip_ndc.w + clip_ndc.y) * 0.5;
    Mat4f {
        v: [
            sx, 0.0, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, tx, ty, 0.0, 1.0,
        ],
    }
}

// --- Unit cube geometry (half-extent = 1.0, 24 verts with face normals, 36 indices) ---

#[rustfmt::skip]
const UNIT_CUBE_POSITIONS: &[[f32; 3]] = &[
    // +X face
    [ 1.0, -1.0, -1.0], [ 1.0, -1.0,  1.0], [ 1.0,  1.0,  1.0], [ 1.0,  1.0, -1.0],
    // -X face
    [-1.0, -1.0,  1.0], [-1.0, -1.0, -1.0], [-1.0,  1.0, -1.0], [-1.0,  1.0,  1.0],
    // +Y face
    [-1.0,  1.0, -1.0], [ 1.0,  1.0, -1.0], [ 1.0,  1.0,  1.0], [-1.0,  1.0,  1.0],
    // -Y face
    [-1.0, -1.0,  1.0], [ 1.0, -1.0,  1.0], [ 1.0, -1.0, -1.0], [-1.0, -1.0, -1.0],
    // +Z face
    [-1.0, -1.0,  1.0], [-1.0,  1.0,  1.0], [ 1.0,  1.0,  1.0], [ 1.0, -1.0,  1.0],
    // -Z face
    [ 1.0, -1.0, -1.0], [ 1.0,  1.0, -1.0], [-1.0,  1.0, -1.0], [-1.0, -1.0, -1.0],
];

#[rustfmt::skip]
const UNIT_CUBE_NORMALS: &[[f32; 3]] = &[
    // +X
    [ 1.0, 0.0, 0.0], [ 1.0, 0.0, 0.0], [ 1.0, 0.0, 0.0], [ 1.0, 0.0, 0.0],
    // -X
    [-1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [-1.0, 0.0, 0.0],
    // +Y
    [ 0.0, 1.0, 0.0], [ 0.0, 1.0, 0.0], [ 0.0, 1.0, 0.0], [ 0.0, 1.0, 0.0],
    // -Y
    [ 0.0,-1.0, 0.0], [ 0.0,-1.0, 0.0], [ 0.0,-1.0, 0.0], [ 0.0,-1.0, 0.0],
    // +Z
    [ 0.0, 0.0, 1.0], [ 0.0, 0.0, 1.0], [ 0.0, 0.0, 1.0], [ 0.0, 0.0, 1.0],
    // -Z
    [ 0.0, 0.0,-1.0], [ 0.0, 0.0,-1.0], [ 0.0, 0.0,-1.0], [ 0.0, 0.0,-1.0],
];

#[rustfmt::skip]
const UNIT_CUBE_INDICES: &[u32] = &[
     0,  1,  2,   2,  3,  0, // +X
     4,  5,  6,   6,  7,  4, // -X
     8,  9, 10,  10, 11,  8, // +Y
    12, 13, 14,  14, 15, 12, // -Y
    16, 17, 18,  18, 19, 16, // +Z
    20, 21, 22,  22, 23, 20, // -Z
];
