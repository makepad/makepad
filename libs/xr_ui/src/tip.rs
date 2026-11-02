//! Interaction markers at [`crate::input::tip`]: a tapered controller stylus whose apex
//! stays at the interaction point, or just a bright dot for a tracked index finger. Touch
//! lights the stylus and extends its base toward the controller; it never moves the apex.
//! Two small retained meshes use the shared DrawCube mesh shader on every backend.

use makepad_widgets::makepad_draw::geometry::geometry_gen::GeometryGen;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.XrTips = set_type_default() do #(XrTips::script_component(vm)){
        draw_tip +: {depth_write: true light_dir: vec3(0.35, 0.8, 0.45)}
        draw_dot +: {depth_write: true pixel: fn() { return self.color }}
        color_idle: theme.color_text_meta
        color_touch: theme.color_focus
    }
}

const LENGTH_M: f32 = 0.06;
const BASE_RADIUS_M: f32 = 0.008;
const DOT_DIAMETER_M: f32 = 0.003;

#[derive(Script, ScriptHook)]
pub struct XrTips {
    #[live]
    pub draw_tip: DrawCube,
    #[live]
    draw_dot: DrawCube,
    #[live]
    pub color_idle: Vec4f,
    #[live]
    pub color_touch: Vec4f,
    #[rust]
    cone: Option<Geometry>,
    #[rust]
    dot: Option<Geometry>,
    #[rust([true; 2])]
    controllers: [bool; 2],
}

impl XrTips {
    /// Set marker kinds once per input update. `draw`/`draw_in` keep accepting room poses;
    /// controllers show a stylus, tracked hands show only the fingertip dot.
    pub fn set_input(&mut self, state: &XrState) {
        self.controllers = [state.left_controller.active(), state.right_controller.active()];
    }

    /// Both tips (left, right; room poses), `touching` lighting each.
    pub fn draw(&mut self, cx: &mut Cx3d, tips: [Option<Pose>; 2], touching: [bool; 2]) {
        self.draw_in(cx, tips, touching, &Mat4f::identity());
    }

    /// As [`XrTips::draw`] in a pass whose scene is not the room (`room_to_scene`).
    pub fn draw_in(&mut self, cx: &mut Cx3d, tips: [Option<Pose>; 2], touching: [bool; 2], room_to_scene: &Mat4f) {
        if tips.iter().all(Option::is_none) { return; }
        let cone = self.cone.get_or_insert_with(|| cone_mesh().into_cube_vertex_pod().into_geometry(cx.cx));
        let dot = self.dot.get_or_insert_with(|| dot_mesh().into_cube_vertex_pod().into_geometry(cx.cx));
        self.draw_tip.draw_vars.geometry_id = Some(cone.geometry_id());
        self.draw_dot.draw_vars.geometry_id = Some(dot.geometry_id());
        for (i, (tip, touch)) in tips.into_iter().zip(touching).enumerate() {
            let Some(p) = tip.filter(Pose::is_finite) else { continue };
            let color = if touch {
                let c = self.color_touch;
                vec4(c.x * 0.88 + 0.12, c.y * 0.88 + 0.12, c.z * 0.88 + 0.12, c.w)
            } else { self.color_idle };
            let transform = Mat4f::mul(room_to_scene, &p.to_mat4());
            if self.controllers[i] {
                self.draw_tip.color = color;
                self.draw_tip.transform = transform;
                self.draw_tip.cube_pos = Vec3f::default();
                // Local -Z points forward: apex z=0, base behind it at positive z.
                self.draw_tip.cube_size = vec3f(BASE_RADIUS_M, BASE_RADIUS_M, LENGTH_M * if touch { 1.12 } else { 1.0 });
                self.draw_tip.draw(cx);
            }
            self.draw_dot.color = vec4(color.x * 0.25 + 0.75, color.y * 0.25 + 0.75, color.z * 0.25 + 0.75, 1.0);
            self.draw_dot.transform = transform;
            self.draw_dot.cube_pos = Vec3f::default();
            self.draw_dot.cube_size = vec3f(DOT_DIAMETER_M, DOT_DIAMETER_M, DOT_DIAMETER_M);
            self.draw_dot.draw(cx);
        }
    }
}

fn vertex(g: &mut GeometryGen, p: Vec3f, n: Vec3f) {
    g.indices.push((g.vertices.len() / 9) as u32);
    g.vertices.extend_from_slice(&[p.x, p.y, p.z, 0.0, n.x, n.y, n.z, 0.0, 0.0]);
}

fn cone_mesh() -> GeometryGen {
    let mut g = GeometryGen::default();
    let ring = |a: f32| vec3f(a.cos(), a.sin(), 1.0);
    let normal = |a: f32| vec3f(a.cos(), a.sin(), -BASE_RADIUS_M / LENGTH_M).normalize();
    for i in 0..24 {
        let a = i as f32 * std::f32::consts::TAU / 24.0;
        let b = (i + 1) as f32 * std::f32::consts::TAU / 24.0;
        vertex(&mut g, Vec3f::default(), normal((a + b) * 0.5));
        vertex(&mut g, ring(b), normal(b));
        vertex(&mut g, ring(a), normal(a));
        for p in [vec3f(0.0, 0.0, 1.0), ring(a), ring(b)] {
            vertex(&mut g, p, vec3f(0.0, 0.0, 1.0));
        }
    }
    g
}

fn dot_mesh() -> GeometryGen {
    let mut g = GeometryGen::default();
    let point = |lat: usize, lon: usize| {
        let a = lat as f32 * std::f32::consts::PI / 8.0;
        let b = lon as f32 * std::f32::consts::TAU / 16.0;
        vec3f(a.sin() * b.cos(), a.sin() * b.sin(), a.cos())
    };
    for lat in 0..8 {
        for lon in 0..16 {
            let (a, b, c, d) = (point(lat, lon), point(lat + 1, lon), point(lat + 1, lon + 1), point(lat, lon + 1));
            for n in [a, b, c, a, c, d] {
                vertex(&mut g, n * 0.5, n);
            }
        }
    }
    g
}
