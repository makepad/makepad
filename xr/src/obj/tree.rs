use crate::scene::{
    xr_widget_world_transform, XrDrawContext, XrHandInfluencePoint, XrNode,
    XR_HAND_INFLUENCE_POINT_COUNT,
};
use crate::util::{
    mesh_generators::{stylized_leaf_mesh, tree_branch_segment_mesh},
    scene_draw::scene_state_from_cx,
};
use makepad_widgets::{makepad_derive_widget::*, makepad_draw::*, widget::*};
use std::f32::consts::PI;

pub const PYTHAGOREAN_TREE_ROOT_DROP: f32 = 0.60;

const TREE_BRANCH_SIDES: usize = 4;
const TREE_MAX_DEPTH: usize = 7;
const TREE_BASE_LENGTH: f32 = 0.46;
const TREE_BASE_RADIUS: f32 = 0.026;
const TREE_CHILD_SCALE: f32 = 0.57735026;
const TREE_RADIUS_SCALE: f32 = 0.74;
const TREE_BRANCH_SPLIT_ANGLE: f32 = 0.58;
const TREE_BRANCH_YAW_STEP: f32 = PI * 2.0 / 3.0;
const TREE_BRANCH_YAW_PHASE_STEP: f32 = PI / 3.0;
const TREE_BRANCH_PARENT_DRAG: f32 = 0.42;
const TREE_BRANCH_HAND_GAIN: f32 = 0.19;
const TREE_LEAF_HAND_GAIN: f32 = 0.34;
const TREE_MAX_POINT_PUSH: f32 = 0.24;
const TREE_BRANCH_SPRING_STIFFNESS: f32 = 42.0;
const TREE_BRANCH_SPRING_DAMPING: f32 = 11.5;
const TREE_LEAF_SPRING_STIFFNESS: f32 = 64.0;
const TREE_LEAF_SPRING_DAMPING: f32 = 15.0;
const TREE_SIM_DT_DEFAULT: f32 = 1.0 / 90.0;
const TREE_SIM_DT_MIN: f32 = 1.0 / 240.0;
const TREE_SIM_DT_MAX: f32 = 1.0 / 18.0;
const TREE_HAND_MAX_SPEED: f32 = 2.4;
const TREE_HAND_VELOCITY_BLEND: f32 = 0.58;
const TREE_LEAF_BASE_SCALE: f32 = 0.056;
const TREE_LEAF_PITCH_ANGLE: f32 = -0.34;
const TREE_LEAF_TILT_ANGLE: f32 = 0.50;
const TREE_BRANCH_LIGHT_DIR: Vec3f = Vec3f {
    x: 0.34,
    y: 0.88,
    z: 0.32,
};
const TREE_BRANCH_LIGHT_COLOR: Vec3f = Vec3f {
    x: 1.0,
    y: 0.98,
    z: 0.94,
};
const TREE_BRANCH_BARK_DARK: Vec3f = Vec3f {
    x: 0.28,
    y: 0.16,
    z: 0.09,
};
const TREE_BRANCH_BARK_LIGHT: Vec3f = Vec3f {
    x: 0.62,
    y: 0.42,
    z: 0.24,
};
const TREE_LEAF_LIGHT_DIR: Vec3f = Vec3f {
    x: 0.26,
    y: 0.90,
    z: 0.35,
};
const TREE_LEAF_LIGHT_COLOR: Vec3f = Vec3f {
    x: 1.0,
    y: 1.0,
    z: 0.96,
};
const TREE_LEAF_DARK: Vec3f = Vec3f {
    x: 0.08,
    y: 0.24,
    z: 0.07,
};
const TREE_LEAF_LIGHT: Vec3f = Vec3f {
    x: 0.26,
    y: 0.66,
    z: 0.22,
};
const TREE_LEAF_GLOW: Vec3f = Vec3f {
    x: 0.72,
    y: 0.88,
    z: 0.28,
};
const TREE_LEAF_WIND_ORBIT: f32 = 0.007;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawTreeBranches = mod.std.set_type_default() do #(DrawTreeBranches::script_shader(vm)){
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.PbrVertex, geom.PbrGeom)
        u_tree_light_dir: uniform(vec3(0.34, 0.88, 0.32))
        u_tree_light_color: uniform(vec3(1.0, 0.98, 0.94))
        u_tree_bark_dark: uniform(vec3(0.18, 0.10, 0.05))
        u_tree_bark_light: uniform(vec3(0.42, 0.26, 0.13))
        u_tree_ambient: uniform(float(0.22))
        u_tree_camera_pos: uniform(vec3(0.0, 0.0, 0.0))

        v_world_clip: varying(vec4f)
        v_world: varying(vec3f)
        v_normal: varying(vec3f)
        v_uv: varying(vec2f)
        v_level: varying(float)
        v_seed: varying(float)

        world_pos: fn(local: vec3f) -> vec3f {
            return self.origin
                + self.basis_x * (local.x * self.scale.x)
                + self.basis_y * (local.y * self.scale.y)
                + self.basis_z * (local.z * self.scale.z);
        }

        world_normal: fn(local: vec3f) -> vec3f {
            return normalize(
                self.basis_x * (local.x / max(self.scale.x, 0.0001))
                + self.basis_y * (local.y / max(self.scale.y, 0.0001))
                + self.basis_z * (local.z / max(self.scale.z, 0.0001))
            );
        }

        vertex: fn() {
            let local_pos = vec3(self.geom.pos_nx.x, self.geom.pos_nx.y, self.geom.pos_nx.z);
            let local_normal = normalize(vec3(
                self.geom.pos_nx.w,
                self.geom.ny_nz_uv.x,
                self.geom.ny_nz_uv.y
            ));
            self.v_world = self.world_pos(local_pos);
            self.v_normal = self.world_normal(local_normal);
            self.v_uv = self.geom.ny_nz_uv.zw;
            self.v_level = self.level;
            self.v_seed = self.seed;
            self.v_world_clip = vec4(self.v_world.x, self.v_world.y, self.v_world.z, 1.0);
            let view_pos = self.draw_pass.camera_view * self.v_world_clip;
            self.vertex_pos = self.draw_pass.camera_projection * view_pos;
        }

        pixel: fn() {
            let n = normalize(self.v_normal);
            let l = normalize(self.u_tree_light_dir);
            let v = normalize(self.u_tree_camera_pos - self.v_world);
            let diffuse = max(dot(n, l), 0.0);
            let rim = pow(max(1.0 - max(dot(n, v), 0.0), 0.0), 2.6);
            let bark_noise = 0.5 + 0.5 * sin(
                self.v_world.x * 7.1
                + self.v_world.z * 11.3
                + self.v_seed * 13.7
            );
            let grain = 0.5 + 0.5 * sin(self.v_world.y * (26.0 + self.v_level * 12.0) + self.v_seed * 17.0);
            let bark_mix = clamp(0.16 + bark_noise * 0.28 + grain * 0.62, 0.0, 1.0);
            let base = mix(self.u_tree_bark_dark, self.u_tree_bark_light, bark_mix);
            let lit = self.u_tree_ambient + diffuse * (0.86 - self.u_tree_ambient);
            let color = base * lit + self.u_tree_light_color * rim * 0.08;
            return vec4(color, 1.0);
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.v_world_clip, self.pixel(), self.depth_clip);
        }
    }

    mod.draw.DrawTreeLeaves = mod.std.set_type_default() do #(DrawTreeLeaves::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.PbrVertex, geom.PbrGeom)
        u_tree_light_dir: uniform(vec3(0.26, 0.90, 0.35))
        u_tree_light_color: uniform(vec3(1.0, 1.0, 0.96))
        u_tree_leaf_dark: uniform(vec3(0.08, 0.24, 0.07))
        u_tree_leaf_light: uniform(vec3(0.26, 0.66, 0.22))
        u_tree_leaf_glow: uniform(vec3(0.72, 0.88, 0.28))
        u_tree_ambient: uniform(float(0.18))
        u_tree_camera_pos: uniform(vec3(0.0, 0.0, 0.0))

        v_world_clip: varying(vec4f)
        v_world: varying(vec3f)
        v_normal: varying(vec3f)
        v_uv: varying(vec2f)
        v_tint: varying(float)
        v_flutter: varying(float)

        world_pos: fn(local: vec3f) -> vec3f {
            return self.origin
                + self.basis_x * (local.x * self.scale.x)
                + self.basis_y * (local.y * self.scale.y)
                + self.basis_z * (local.z * self.scale.z);
        }

        world_normal: fn(local: vec3f) -> vec3f {
            return normalize(
                self.basis_x * (local.x / max(self.scale.x, 0.0001))
                + self.basis_y * (local.y / max(self.scale.y, 0.0001))
                + self.basis_z * (local.z / max(self.scale.z, 0.0001))
            );
        }

        vertex: fn() {
            let local_pos = vec3(self.geom.pos_nx.x, self.geom.pos_nx.y, self.geom.pos_nx.z);
            let local_normal = normalize(vec3(
                self.geom.pos_nx.w,
                self.geom.ny_nz_uv.x,
                self.geom.ny_nz_uv.y
            ));
            self.v_world = self.world_pos(local_pos);
            self.v_normal = self.world_normal(local_normal);
            self.v_uv = self.geom.ny_nz_uv.zw;
            self.v_tint = self.tint;
            self.v_flutter = self.flutter;
            self.v_world_clip = vec4(self.v_world.x, self.v_world.y, self.v_world.z, 1.0);
            let view_pos = self.draw_pass.camera_view * self.v_world_clip;
            self.vertex_pos = self.draw_pass.camera_projection * view_pos;
        }

        pixel: fn() {
            let n = normalize(self.v_normal);
            let l = normalize(self.u_tree_light_dir);
            let v = normalize(self.u_tree_camera_pos - self.v_world);
            let diffuse = abs(dot(n, l));
            let rim = pow(max(1.0 - max(abs(dot(n, v)), 0.0), 0.0), 2.2);
            let edge = smoothstep(0.0, 0.92, 1.0 - abs(self.v_uv.x * 2.0 - 1.0));
            let tip = smoothstep(0.18, 1.0, self.v_uv.y);
            let flutter = clamp(self.v_flutter, 0.0, 1.0);
            let trans = pow(max(dot(-n, l), 0.0), 1.4) * (0.24 + flutter * 0.42);
            let tint_mix = clamp(0.35 + self.v_tint * 0.45 + tip * 0.16, 0.0, 1.0);
            let base = mix(self.u_tree_leaf_dark, self.u_tree_leaf_light, tint_mix);
            let lit = self.u_tree_ambient + diffuse * (0.92 - self.u_tree_ambient);
            let color = base * lit
                + self.u_tree_leaf_glow * (trans * 0.72 + edge * tip * 0.12 + rim * 0.04)
                + self.u_tree_light_color * rim * 0.03;
            return vec4(color, 1.0);
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.v_world_clip, self.pixel(), self.depth_clip);
        }
    }

    mod.widgets.TreeBase = #(Tree::register_widget(vm))
    mod.widgets.Tree = set_type_default() do mod.widgets.TreeBase{
        draw_branches +: {}
        draw_leaves +: {}
    }
    mod.widgets.FractalTree = set_type_default() do mod.widgets.Tree{}
}

#[derive(Clone, Copy, Debug, Default)]
struct BranchTemplate {
    parent: Option<usize>,
    local_rotation: Quat,
    length: f32,
    radius: f32,
    level: u8,
    seed: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct LeafTemplate {
    branch: usize,
    local_offset: Vec3f,
    local_rotation: Quat,
    scale: f32,
    tint: f32,
    seed: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct BranchRuntime {
    tip: Vec3f,
    basis_x: Vec3f,
    basis_y: Vec3f,
    basis_z: Vec3f,
    point_push: Vec3f,
}

#[derive(Clone, Copy, Debug, Default)]
struct BranchInstance {
    origin: Vec3f,
    basis_x: Vec3f,
    basis_y: Vec3f,
    basis_z: Vec3f,
    scale: Vec3f,
    level: f32,
    seed: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct LeafInstance {
    origin: Vec3f,
    basis_x: Vec3f,
    basis_y: Vec3f,
    basis_z: Vec3f,
    scale: Vec3f,
    tint: f32,
    flutter: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct SpringState {
    offset: Vec3f,
    velocity: Vec3f,
}

#[derive(Clone, Copy, Debug, Default)]
struct SpringContact {
    target_offset: Vec3f,
    velocity_boost: Vec3f,
}

#[derive(Clone, Copy, Debug, Default)]
struct LocalHandInfluence {
    pos: Vec3f,
    velocity: Vec3f,
    gain_scale: f32,
    radius_scale: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct HandInfluenceHistory {
    pos: Option<Vec3f>,
    velocity: Vec3f,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TreeTemplateConfig {
    branch_split_angle: f32,
    branch_yaw_step: f32,
    branch_yaw_phase_step: f32,
    child_scale: f32,
    length_scale_0: f32,
    length_scale_1: f32,
    length_scale_2: f32,
    length_scale_3: f32,
    length_scale_4: f32,
    length_scale_rest: f32,
}

impl Default for TreeTemplateConfig {
    fn default() -> Self {
        Self {
            branch_split_angle: TREE_BRANCH_SPLIT_ANGLE,
            branch_yaw_step: TREE_BRANCH_YAW_STEP,
            branch_yaw_phase_step: TREE_BRANCH_YAW_PHASE_STEP,
            child_scale: TREE_CHILD_SCALE,
            length_scale_0: 0.70,
            length_scale_1: 0.78,
            length_scale_2: 0.88,
            length_scale_3: 0.97,
            length_scale_4: 1.03,
            length_scale_rest: 1.08,
        }
    }
}

impl TreeTemplateConfig {
    fn length_scale(self, level: usize) -> f32 {
        match level {
            0 => self.length_scale_0,
            1 => self.length_scale_1,
            2 => self.length_scale_2,
            3 => self.length_scale_3,
            4 => self.length_scale_4,
            _ => self.length_scale_rest,
        }
    }
}

#[derive(Default)]
pub struct CpuPythagoreanTree {
    branch_geometry: Option<Geometry>,
    leaf_geometry: Option<Geometry>,
    template_config: Option<TreeTemplateConfig>,
    branch_templates: Vec<BranchTemplate>,
    leaf_templates: Vec<LeafTemplate>,
    branch_dynamics: Vec<SpringState>,
    leaf_dynamics: Vec<SpringState>,
    branch_runtime: Vec<BranchRuntime>,
    branch_instances: Vec<BranchInstance>,
    leaf_instances: Vec<LeafInstance>,
    hand_influence_history: [HandInfluenceHistory; XR_HAND_INFLUENCE_POINT_COUNT],
    last_rebuild_time: Option<f32>,
}

impl CpuPythagoreanTree {
    fn ensure_geometry(&mut self, cx: &mut CxDraw, config: TreeTemplateConfig) {
        if self.template_config != Some(config) || self.branch_templates.is_empty() {
            self.rebuild_templates(config);
            self.template_config = Some(config);
            self.branch_dynamics.clear();
            self.leaf_dynamics.clear();
            self.branch_runtime.clear();
            self.hand_influence_history =
                [HandInfluenceHistory::default(); XR_HAND_INFLUENCE_POINT_COUNT];
            self.last_rebuild_time = None;
        }
        if self.branch_geometry.is_none() {
            self.branch_geometry =
                Some(tree_branch_segment_mesh(TREE_BRANCH_SIDES, 0.84).into_geometry(cx.cx));
        }
        if self.leaf_geometry.is_none() {
            self.leaf_geometry = Some(stylized_leaf_mesh().into_geometry(cx.cx));
        }
    }

    fn rebuild_instances(
        &mut self,
        root_transform: Mat4f,
        time: f32,
        hand_influences_world: [Option<XrHandInfluencePoint>; XR_HAND_INFLUENCE_POINT_COUNT],
    ) {
        if self.branch_templates.is_empty() {
            self.rebuild_templates(self.template_config.unwrap_or_default());
        }
        self.branch_instances.clear();
        self.leaf_instances.clear();
        self.branch_dynamics
            .resize(self.branch_templates.len(), SpringState::default());
        self.leaf_dynamics
            .resize(self.leaf_templates.len(), SpringState::default());
        self.branch_runtime
            .resize(self.branch_templates.len(), BranchRuntime::default());
        let dt = self
            .last_rebuild_time
            .and_then(|last| {
                let delta = time - last;
                (delta.is_finite() && delta > 0.0)
                    .then_some(delta.clamp(TREE_SIM_DT_MIN, TREE_SIM_DT_MAX))
            })
            .unwrap_or(TREE_SIM_DT_DEFAULT);
        self.last_rebuild_time = Some(time);

        let root_inverse = root_transform.invert();
        let mut hand_influences = [None; XR_HAND_INFLUENCE_POINT_COUNT];
        for (slot, influence) in hand_influences_world.into_iter().enumerate() {
            let history = &mut self.hand_influence_history[slot];
            hand_influences[slot] = influence.map(|influence| {
                let local_pos = root_inverse
                    .transform_vec4(influence.pos.to_vec4())
                    .to_vec3f();
                let raw_velocity = history
                    .pos
                    .map(|last_pos| {
                        Self::clamp_len((local_pos - last_pos) / dt, TREE_HAND_MAX_SPEED)
                    })
                    .unwrap_or(vec3f(0.0, 0.0, 0.0));
                let velocity = history.velocity * (1.0 - TREE_HAND_VELOCITY_BLEND)
                    + raw_velocity * TREE_HAND_VELOCITY_BLEND;
                history.pos = Some(local_pos);
                history.velocity = velocity;
                LocalHandInfluence {
                    pos: local_pos,
                    velocity,
                    gain_scale: influence.gain_scale,
                    radius_scale: influence.radius_scale,
                }
            });
            if hand_influences[slot].is_none() {
                history.pos = None;
                history.velocity *= 0.45;
            }
        }
        let animate =
            time.abs() > f32::EPSILON || hand_influences.iter().any(|point| point.is_some());
        let level_den = (TREE_MAX_DEPTH.saturating_sub(1)).max(1) as f32;

        for (index, branch) in self.branch_templates.iter().enumerate() {
            let (start, parent_basis_x, parent_basis_y, parent_basis_z, inherited_push) =
                if let Some(parent) = branch.parent {
                    let runtime = self.branch_runtime[parent];
                    (
                        runtime.tip,
                        runtime.basis_x,
                        runtime.basis_y,
                        runtime.basis_z,
                        runtime.point_push * TREE_BRANCH_PARENT_DRAG,
                    )
                } else {
                    (
                        vec3f(0.0, 0.0, 0.0),
                        vec3f(1.0, 0.0, 0.0),
                        vec3f(0.0, 1.0, 0.0),
                        vec3f(0.0, 0.0, 1.0),
                        vec3f(0.0, 0.0, 0.0),
                    )
                };

            let local_dir = branch.local_rotation.rotate_vec3(&vec3f(0.0, 1.0, 0.0));
            let local_z = branch.local_rotation.rotate_vec3(&vec3f(0.0, 0.0, 1.0));
            let base_dir = Self::normalize_or(
                Self::frame_vector(parent_basis_x, parent_basis_y, parent_basis_z, local_dir),
                parent_basis_y,
            );
            let z_hint = Self::normalize_or(
                Self::frame_vector(parent_basis_x, parent_basis_y, parent_basis_z, local_z),
                parent_basis_z,
            );
            let nominal_tip = start + base_dir * branch.length;
            let level_t = branch.level as f32 / level_den;

            let mut point_target = vec3f(0.0, 0.0, 0.0);
            let mut point_velocity_boost = vec3f(0.0, 0.0, 0.0);
            if animate {
                point_target = Self::branch_wind_force(branch.seed, level_t, time) + inherited_push;
                let branch_pointer_radius = (branch.radius * 5.6).clamp(0.032, 0.095);
                let branch_mid_radius =
                    (branch_pointer_radius * 0.88).clamp(0.028, branch_pointer_radius);
                for influence in hand_influences.into_iter().flatten() {
                    let pointer_tip = influence.pos;
                    let closest_on_branch =
                        Self::closest_point_on_segment(start, nominal_tip, pointer_tip);
                    let closest_on_mid = Self::closest_point_on_segment(
                        start + (nominal_tip - start) * 0.22,
                        start + (nominal_tip - start) * 0.82,
                        pointer_tip,
                    );
                    let branch_contact = Self::contact_response(
                        closest_on_branch,
                        influence,
                        TREE_BRANCH_HAND_GAIN * influence.gain_scale * (1.65 + level_t * 1.10),
                        branch_pointer_radius * influence.radius_scale,
                        0.92,
                        0.76,
                    );
                    point_target += branch_contact.target_offset;
                    point_velocity_boost += branch_contact.velocity_boost;
                    let mid_contact = Self::contact_response(
                        closest_on_mid,
                        influence,
                        TREE_BRANCH_HAND_GAIN * influence.gain_scale * (0.90 + level_t * 0.62),
                        branch_mid_radius * influence.radius_scale,
                        0.54,
                        0.58,
                    );
                    point_target += mid_contact.target_offset * 0.56;
                    point_velocity_boost += mid_contact.velocity_boost * 0.56;
                }
                point_velocity_boost = Self::clamp_len(point_velocity_boost, 1.05 + level_t * 0.30);
            }
            let point_push = Self::spring_step(
                &mut self.branch_dynamics[index],
                point_target,
                point_velocity_boost,
                dt,
                TREE_BRANCH_SPRING_STIFFNESS * (0.90 + level_t * 0.45),
                TREE_BRANCH_SPRING_DAMPING * (1.0 - level_t * 0.10),
                TREE_MAX_POINT_PUSH,
            );

            let target_tip = nominal_tip + point_push;
            let direction = Self::normalize_or(target_tip - start, base_dir);
            let tip = start + direction * branch.length;
            let (basis_x, basis_y, basis_z) = Self::orthonormal_frame(direction, z_hint);

            self.branch_runtime[index] = BranchRuntime {
                tip,
                basis_x,
                basis_y,
                basis_z,
                point_push: tip - nominal_tip,
            };

            let branch_basis_x = Self::transform_direction(root_transform, basis_x);
            let branch_basis_y = Self::transform_direction(root_transform, basis_y);
            let branch_basis_z = Self::transform_direction(root_transform, basis_z);
            self.branch_instances.push(BranchInstance {
                origin: Self::transform_point(root_transform, start),
                basis_x: Self::normalize_or(branch_basis_x, vec3f(1.0, 0.0, 0.0)),
                basis_y: Self::normalize_or(branch_basis_y, vec3f(0.0, 1.0, 0.0)),
                basis_z: Self::normalize_or(branch_basis_z, vec3f(0.0, 0.0, 1.0)),
                scale: vec3f(
                    branch.radius * branch_basis_x.length().max(0.0001),
                    branch.length * branch_basis_y.length().max(0.0001),
                    branch.radius * branch_basis_z.length().max(0.0001),
                ),
                level: level_t,
                seed: branch.seed,
            });
        }

        for (leaf_index, leaf) in self.leaf_templates.iter().enumerate() {
            let branch_runtime = self.branch_runtime[leaf.branch];
            let branch = self.branch_templates[leaf.branch];
            let level_t = branch.level as f32 / level_den;

            let anchor = branch_runtime.tip
                + Self::frame_vector(
                    branch_runtime.basis_x,
                    branch_runtime.basis_y,
                    branch_runtime.basis_z,
                    leaf.local_offset,
                );
            let local_leaf_y = leaf.local_rotation.rotate_vec3(&vec3f(0.0, 1.0, 0.0));
            let local_leaf_z = leaf.local_rotation.rotate_vec3(&vec3f(0.0, 0.0, 1.0));
            let nominal_leaf_y = Self::normalize_or(
                Self::frame_vector(
                    branch_runtime.basis_x,
                    branch_runtime.basis_y,
                    branch_runtime.basis_z,
                    local_leaf_y,
                ),
                branch_runtime.basis_y,
            );
            let leaf_z_hint = Self::normalize_or(
                Self::frame_vector(
                    branch_runtime.basis_x,
                    branch_runtime.basis_y,
                    branch_runtime.basis_z,
                    local_leaf_z,
                ),
                branch_runtime.basis_z,
            );

            let anchor_offset = if animate {
                let orbit_phase = time * (1.45 + level_t * 0.55) + leaf.seed * 8.7;
                branch_runtime.basis_x
                    * (orbit_phase.cos() * TREE_LEAF_WIND_ORBIT * leaf.scale * 1.8)
                    + branch_runtime.basis_y
                        * (orbit_phase.sin() * TREE_LEAF_WIND_ORBIT * leaf.scale * 0.7)
                    + branch_runtime.basis_z
                        * ((orbit_phase * 1.31).sin() * TREE_LEAF_WIND_ORBIT * leaf.scale * 1.6)
            } else {
                vec3f(0.0, 0.0, 0.0)
            };
            let origin = anchor + anchor_offset;
            let mut leaf_target = vec3f(0.0, 0.0, 0.0);
            let mut leaf_velocity_boost = vec3f(0.0, 0.0, 0.0);
            if animate {
                leaf_target = Self::leaf_wind_force(leaf.seed, level_t, time)
                    + branch_runtime.point_push * (1.35 + level_t * 0.2);
                let leaf_tip = origin + nominal_leaf_y * leaf.scale;
                let leaf_mid = origin + nominal_leaf_y * (leaf.scale * 0.58);
                let leaf_pointer_radius = (leaf.scale * 1.35).clamp(0.045, 0.120);
                for influence in hand_influences.into_iter().flatten() {
                    let pointer_tip = influence.pos;
                    let closest_on_leaf =
                        Self::closest_point_on_segment(origin, leaf_tip, pointer_tip);
                    let closest_on_leaf_mid =
                        Self::closest_point_on_segment(origin, leaf_mid, pointer_tip);
                    let main_contact = Self::contact_response(
                        closest_on_leaf,
                        influence,
                        TREE_LEAF_HAND_GAIN * influence.gain_scale * (2.45 + level_t * 1.50),
                        leaf_pointer_radius * influence.radius_scale,
                        1.28,
                        0.96,
                    );
                    leaf_target += main_contact.target_offset;
                    leaf_velocity_boost += main_contact.velocity_boost;
                    let mid_contact = Self::contact_response(
                        closest_on_leaf_mid,
                        influence,
                        TREE_LEAF_HAND_GAIN * influence.gain_scale * (1.45 + level_t * 0.88),
                        leaf_pointer_radius * influence.radius_scale * 0.82,
                        0.92,
                        0.90,
                    );
                    leaf_target += mid_contact.target_offset * 0.55;
                    leaf_velocity_boost += mid_contact.velocity_boost * 0.55;
                }
                leaf_velocity_boost = Self::clamp_len(leaf_velocity_boost, 1.45 + level_t * 0.40);
            }
            let leaf_push = Self::spring_step(
                &mut self.leaf_dynamics[leaf_index],
                leaf_target,
                leaf_velocity_boost,
                dt,
                TREE_LEAF_SPRING_STIFFNESS * (0.95 + level_t * 0.35),
                TREE_LEAF_SPRING_DAMPING * (1.0 - level_t * 0.08),
                TREE_MAX_POINT_PUSH * 1.35,
            );

            let leaf_y = Self::normalize_or(nominal_leaf_y + leaf_push * 1.6, nominal_leaf_y);
            let (basis_x, basis_y, basis_z) = Self::orthonormal_frame(leaf_y, leaf_z_hint);
            let leaf_basis_x = Self::transform_direction(root_transform, basis_x);
            let leaf_basis_y = Self::transform_direction(root_transform, basis_y);
            let leaf_basis_z = Self::transform_direction(root_transform, basis_z);
            let flutter = ((leaf_y - nominal_leaf_y).length() * 1.45 + leaf_push.length() * 0.3)
                .clamp(0.0, 1.0);

            self.leaf_instances.push(LeafInstance {
                origin: Self::transform_point(root_transform, origin),
                basis_x: Self::normalize_or(leaf_basis_x, vec3f(1.0, 0.0, 0.0)),
                basis_y: Self::normalize_or(leaf_basis_y, vec3f(0.0, 1.0, 0.0)),
                basis_z: Self::normalize_or(leaf_basis_z, vec3f(0.0, 0.0, 1.0)),
                scale: vec3f(
                    leaf.scale * leaf_basis_x.length().max(0.0001),
                    leaf.scale * leaf_basis_y.length().max(0.0001),
                    leaf.scale * leaf_basis_z.length().max(0.0001),
                ),
                tint: leaf.tint,
                flutter,
            });
        }
    }

    fn draw(
        &mut self,
        cx: &mut CxDraw,
        draw_branches: &mut DrawTreeBranches,
        draw_leaves: &mut DrawTreeLeaves,
        camera_pos: Vec3f,
    ) {
        let (Some(branch_geometry), Some(leaf_geometry)) =
            (self.branch_geometry.as_ref(), self.leaf_geometry.as_ref())
        else {
            return;
        };
        draw_branches.draw_instances(
            cx,
            branch_geometry.geometry_id(),
            camera_pos,
            &self.branch_instances,
        );
        draw_leaves.draw_instances(
            cx,
            leaf_geometry.geometry_id(),
            camera_pos,
            &self.leaf_instances,
        );
    }

    fn rebuild_templates(&mut self, config: TreeTemplateConfig) {
        self.branch_templates.clear();
        self.leaf_templates.clear();
        self.push_branch(
            None,
            Quat::default(),
            TREE_BASE_LENGTH,
            TREE_BASE_RADIUS,
            0,
            config,
        );
    }

    fn push_branch(
        &mut self,
        parent: Option<usize>,
        local_rotation: Quat,
        length: f32,
        radius: f32,
        level: usize,
        config: TreeTemplateConfig,
    ) {
        let stored_length = length * config.length_scale(level);
        let branch_index = self.branch_templates.len();
        self.branch_templates.push(BranchTemplate {
            parent,
            local_rotation,
            length: stored_length,
            radius,
            level: level as u8,
            seed: Self::seed_from_index(branch_index),
        });

        if level + 2 >= TREE_MAX_DEPTH {
            self.append_leaf_cluster(branch_index, level);
        }
        if level + 1 >= TREE_MAX_DEPTH {
            return;
        }

        let child_length = stored_length * config.child_scale;
        let child_radius = (radius * TREE_RADIUS_SCALE).max(0.004);
        let tilt = Quat::from_axis_angle(vec3f(1.0, 0.0, 0.0), config.branch_split_angle);
        let yaw_phase = level as f32 * config.branch_yaw_phase_step;
        for child_index in 0..3 {
            let yaw = Quat::from_axis_angle(
                vec3f(0.0, 1.0, 0.0),
                yaw_phase + child_index as f32 * config.branch_yaw_step,
            );
            let child_rotation = Quat::multiply(&tilt, &yaw);
            self.push_branch(
                Some(branch_index),
                child_rotation,
                child_length,
                child_radius,
                level + 1,
                config,
            );
        }
    }

    fn append_leaf_cluster(&mut self, branch: usize, level: usize) {
        let density = match level {
            0..=3 => 0,
            4 => 2,
            5 => 3,
            6 => 4,
            7 => 4,
            _ => 5,
        };
        if density == 0 {
            return;
        }
        let branch_radius = self
            .branch_templates
            .get(branch)
            .map(|template| template.radius)
            .unwrap_or(TREE_BASE_RADIUS * 0.25);
        let twig_tip_radius = branch_radius * 0.84;
        let branch_depth = level as f32 / (TREE_MAX_DEPTH.saturating_sub(1)).max(1) as f32;
        for index in 0..density {
            let angle = (index as f32 / density as f32) * PI * 2.0;
            let tilt_axis = vec3f(angle.cos(), 0.0, angle.sin()).normalize();
            let local_rotation = Quat::multiply(
                &Quat::from_axis_angle(vec3f(0.0, 0.0, 1.0), TREE_LEAF_PITCH_ANGLE),
                &Quat::multiply(
                    &Quat::from_axis_angle(tilt_axis, TREE_LEAF_TILT_ANGLE),
                    &Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), angle),
                ),
            );
            let leaf_scale = TREE_LEAF_BASE_SCALE * (0.94 + branch_depth * 0.18);
            // Keep the leaf stem near the twig tip instead of using an absolute world-space spread.
            let spread = twig_tip_radius * 0.85 + leaf_scale * 0.06;
            let stem_sink = (twig_tip_radius * 0.75).min(leaf_scale * 0.16);
            self.leaf_templates.push(LeafTemplate {
                branch,
                local_offset: vec3f(angle.cos() * spread, -stem_sink, angle.sin() * spread),
                local_rotation,
                scale: leaf_scale,
                tint: 0.56,
                seed: Self::seed_from_index(self.leaf_templates.len() + branch * 17 + index * 31),
            });
        }
    }

    fn branch_wind_force(seed: f32, level_t: f32, time: f32) -> Vec3f {
        let sway = 0.006 + level_t * 0.018;
        vec3f(
            (time * 0.77 + seed * 6.7).sin() * 0.75 + (time * 1.33 + seed * 2.3).cos() * 0.25,
            0.0,
            (time * 0.61 + seed * 4.1).cos() * 0.68 + (time * 1.17 + seed * 3.4).sin() * 0.32,
        ) * sway
    }

    fn leaf_wind_force(seed: f32, level_t: f32, time: f32) -> Vec3f {
        let sway = 0.018 + level_t * 0.030;
        vec3f(
            (time * 1.87 + seed * 9.4).sin() * 0.6 + (time * 2.41 + seed * 3.7).cos() * 0.4,
            0.02 + (time * 2.93 + seed * 7.2).sin() * 0.05,
            (time * 2.13 + seed * 5.1).cos() * 0.7 + (time * 1.61 + seed * 1.9).sin() * 0.3,
        ) * sway
    }

    fn contact_response(
        sample: Vec3f,
        influence: LocalHandInfluence,
        gain: f32,
        radius: f32,
        velocity_gain: f32,
        vertical_scale: f32,
    ) -> SpringContact {
        let delta = sample - influence.pos;
        let distance = delta.length().max(0.0001);
        let radius = radius.max(0.0001);
        if distance >= radius {
            return SpringContact::default();
        }
        let penetration = 1.0 - distance / radius;
        let raw_normal = delta / distance;
        let mut contact_normal = raw_normal;
        contact_normal.y *= vertical_scale;
        let contact_normal = Self::normalize_or(contact_normal, raw_normal);
        let normal_push = contact_normal * (penetration * gain * radius * 1.85);
        let mut carried_velocity = influence.velocity;
        carried_velocity.y *= 0.72 + vertical_scale * 0.20;
        SpringContact {
            target_offset: normal_push,
            velocity_boost: Self::clamp_len(
                carried_velocity * (penetration * velocity_gain),
                TREE_HAND_MAX_SPEED * velocity_gain,
            ),
        }
    }

    fn spring_step(
        state: &mut SpringState,
        target: Vec3f,
        velocity_boost: Vec3f,
        dt: f32,
        stiffness: f32,
        damping: f32,
        max_len: f32,
    ) -> Vec3f {
        state.velocity += velocity_boost;
        let accel = (target - state.offset) * stiffness - state.velocity * damping;
        state.velocity += accel * dt;
        state.offset += state.velocity * dt;
        state.offset = Self::clamp_len(state.offset, max_len);
        if state.offset.length() >= max_len - 0.0001 && state.velocity.dot(state.offset) > 0.0 {
            state.velocity *= 0.72;
        }
        state.offset
    }

    fn clamp_len(v: Vec3f, max_len: f32) -> Vec3f {
        let len = v.length();
        if len <= max_len || len <= 0.0001 {
            v
        } else {
            v * (max_len / len)
        }
    }

    fn normalize_or(v: Vec3f, fallback: Vec3f) -> Vec3f {
        let len = v.length();
        if len > 0.0001 {
            v / len
        } else {
            let fallback_len = fallback.length();
            if fallback_len > 0.0001 {
                fallback / fallback_len
            } else {
                vec3f(0.0, 1.0, 0.0)
            }
        }
    }

    fn frame_vector(basis_x: Vec3f, basis_y: Vec3f, basis_z: Vec3f, local: Vec3f) -> Vec3f {
        basis_x * local.x + basis_y * local.y + basis_z * local.z
    }

    fn seed_from_index(index: usize) -> f32 {
        let x = (((index as f32) + 1.0) * 12.9898).sin() * 43_758.547;
        let fract = x.fract();
        if fract < 0.0 {
            fract + 1.0
        } else {
            fract
        }
    }

    fn closest_point_on_segment(a: Vec3f, b: Vec3f, point: Vec3f) -> Vec3f {
        let ab = b - a;
        let ab_len_sq = ab.dot(ab);
        if ab_len_sq <= 0.000001 {
            return a;
        }
        let t = ((point - a).dot(ab) / ab_len_sq).clamp(0.0, 1.0);
        a + ab * t
    }

    fn transform_point(transform: Mat4f, point: Vec3f) -> Vec3f {
        transform
            .transform_vec4(vec4(point.x, point.y, point.z, 1.0))
            .to_vec3f()
    }

    fn transform_direction(transform: Mat4f, direction: Vec3f) -> Vec3f {
        transform
            .transform_vec4(vec4(direction.x, direction.y, direction.z, 0.0))
            .to_vec3f()
    }

    fn orthonormal_frame(y_axis: Vec3f, z_hint: Vec3f) -> (Vec3f, Vec3f, Vec3f) {
        let y = Self::normalize_or(y_axis, vec3f(0.0, 1.0, 0.0));
        let mut projected_z = z_hint - y * y.dot(z_hint);
        if projected_z.length() <= 0.0001 {
            let fallback = if y.y.abs() < 0.95 {
                vec3f(0.0, 1.0, 0.0)
            } else {
                vec3f(1.0, 0.0, 0.0)
            };
            projected_z = fallback - y * y.dot(fallback);
        }
        let z = Self::normalize_or(projected_z, vec3f(0.0, 0.0, 1.0));
        let x = Self::normalize_or(Vec3f::cross(y, z), vec3f(1.0, 0.0, 0.0));
        let z = Self::normalize_or(Vec3f::cross(x, y), z);
        (x, y, z)
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Tree {
    #[redraw]
    #[live]
    draw_branches: DrawTreeBranches,
    #[redraw]
    #[live]
    draw_leaves: DrawTreeLeaves,
    #[live(TREE_BRANCH_SPLIT_ANGLE)]
    branch_split_angle: f32,
    #[live(TREE_BRANCH_YAW_STEP)]
    branch_yaw_step: f32,
    #[live(TREE_BRANCH_YAW_PHASE_STEP)]
    branch_yaw_phase_step: f32,
    #[live(TREE_CHILD_SCALE)]
    child_scale: f32,
    #[live(0.70)]
    length_scale_0: f32,
    #[live(0.78)]
    length_scale_1: f32,
    #[live(0.88)]
    length_scale_2: f32,
    #[live(0.97)]
    length_scale_3: f32,
    #[live(1.03)]
    length_scale_4: f32,
    #[live(1.08)]
    length_scale_rest: f32,
    #[rust]
    cpu_tree: CpuPythagoreanTree,
    #[cast]
    #[deref]
    node: XrNode,
}

impl Tree {
    pub fn node(&self) -> &XrNode {
        &self.node
    }

    fn template_config(&self) -> TreeTemplateConfig {
        TreeTemplateConfig {
            branch_split_angle: self.branch_split_angle,
            branch_yaw_step: self.branch_yaw_step,
            branch_yaw_phase_step: self.branch_yaw_phase_step,
            child_scale: self.child_scale,
            length_scale_0: self.length_scale_0,
            length_scale_1: self.length_scale_1,
            length_scale_2: self.length_scale_2,
            length_scale_3: self.length_scale_3,
            length_scale_4: self.length_scale_4,
            length_scale_rest: self.length_scale_rest,
        }
    }
}

impl Widget for Tree {
    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        let Some(scene) = scene_state_from_cx(cx) else {
            return DrawStep::done();
        };
        let world = xr_widget_world_transform(cx, scope, self.widget_uid(), &self.node);
        let draw_context = XrDrawContext::from_scope(scope);
        let hand_influences = draw_context.hand_influence_points();
        let time = scene.time as f32;
        let template_config = self.template_config();

        self.cpu_tree.ensure_geometry(cx, template_config);
        self.cpu_tree
            .rebuild_instances(world, time, hand_influences);
        self.cpu_tree.draw(
            cx,
            &mut self.draw_branches,
            &mut self.draw_leaves,
            scene.camera_pos,
        );

        self.node.draw_3d(cx, scope)
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawTreeBranches {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub origin: Vec3f,
    #[live(vec3(1.0, 0.0, 0.0))]
    pub basis_x: Vec3f,
    #[live(vec3(0.0, 1.0, 0.0))]
    pub basis_y: Vec3f,
    #[live(vec3(0.0, 0.0, 1.0))]
    pub basis_z: Vec3f,
    #[live(vec3(1.0, 1.0, 1.0))]
    pub scale: Vec3f,
    #[live]
    pub level: f32,
    #[live]
    pub seed: f32,
    #[live(1.0)]
    pub depth_clip: f32,
}

impl DrawTreeBranches {
    fn apply_uniforms(&mut self, cx: &mut CxDraw, camera_pos: Vec3f) {
        let light_dir = TREE_BRANCH_LIGHT_DIR.normalize();
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_tree_light_dir),
            &[light_dir.x, light_dir.y, light_dir.z],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_tree_light_color),
            &[
                TREE_BRANCH_LIGHT_COLOR.x,
                TREE_BRANCH_LIGHT_COLOR.y,
                TREE_BRANCH_LIGHT_COLOR.z,
            ],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_tree_bark_dark),
            &[
                TREE_BRANCH_BARK_DARK.x,
                TREE_BRANCH_BARK_DARK.y,
                TREE_BRANCH_BARK_DARK.z,
            ],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_tree_bark_light),
            &[
                TREE_BRANCH_BARK_LIGHT.x,
                TREE_BRANCH_BARK_LIGHT.y,
                TREE_BRANCH_BARK_LIGHT.z,
            ],
        );
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_tree_ambient), &[0.34]);
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_tree_camera_pos),
            &[camera_pos.x, camera_pos.y, camera_pos.z],
        );
    }

    fn draw_instances(
        &mut self,
        cx: &mut CxDraw,
        geometry_id: GeometryId,
        camera_pos: Vec3f,
        instances: &[BranchInstance],
    ) {
        if instances.is_empty() {
            return;
        }
        self.draw_vars.geometry_id = Some(geometry_id);
        self.draw_vars.options.depth_write = true;
        self.depth_clip = 1.0;
        self.apply_uniforms(cx, camera_pos);
        let mi = cx.begin_many_instances(&self.draw_vars);
        if let Some(mut mi) = mi {
            for instance in instances {
                self.origin = instance.origin;
                self.basis_x = instance.basis_x;
                self.basis_y = instance.basis_y;
                self.basis_z = instance.basis_z;
                self.scale = instance.scale;
                self.level = instance.level;
                self.seed = instance.seed;
                mi.instances.extend_from_slice(self.draw_vars.as_slice());
            }
            let new_area = cx.end_many_instances(mi);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawTreeLeaves {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub origin: Vec3f,
    #[live(vec3(1.0, 0.0, 0.0))]
    pub basis_x: Vec3f,
    #[live(vec3(0.0, 1.0, 0.0))]
    pub basis_y: Vec3f,
    #[live(vec3(0.0, 0.0, 1.0))]
    pub basis_z: Vec3f,
    #[live(vec3(1.0, 1.0, 1.0))]
    pub scale: Vec3f,
    #[live]
    pub tint: f32,
    #[live]
    pub flutter: f32,
    #[live(1.0)]
    pub depth_clip: f32,
}

impl DrawTreeLeaves {
    fn apply_uniforms(&mut self, cx: &mut CxDraw, camera_pos: Vec3f) {
        let light_dir = TREE_LEAF_LIGHT_DIR.normalize();
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_tree_light_dir),
            &[light_dir.x, light_dir.y, light_dir.z],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_tree_light_color),
            &[
                TREE_LEAF_LIGHT_COLOR.x,
                TREE_LEAF_LIGHT_COLOR.y,
                TREE_LEAF_LIGHT_COLOR.z,
            ],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_tree_leaf_dark),
            &[TREE_LEAF_DARK.x, TREE_LEAF_DARK.y, TREE_LEAF_DARK.z],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_tree_leaf_light),
            &[TREE_LEAF_LIGHT.x, TREE_LEAF_LIGHT.y, TREE_LEAF_LIGHT.z],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_tree_leaf_glow),
            &[TREE_LEAF_GLOW.x, TREE_LEAF_GLOW.y, TREE_LEAF_GLOW.z],
        );
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_tree_ambient), &[0.18]);
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_tree_camera_pos),
            &[camera_pos.x, camera_pos.y, camera_pos.z],
        );
    }

    fn draw_instances(
        &mut self,
        cx: &mut CxDraw,
        geometry_id: GeometryId,
        camera_pos: Vec3f,
        instances: &[LeafInstance],
    ) {
        if instances.is_empty() {
            return;
        }
        self.draw_vars.geometry_id = Some(geometry_id);
        self.draw_vars.options.depth_write = true;
        self.depth_clip = 1.0;
        self.apply_uniforms(cx, camera_pos);
        let mi = cx.begin_many_instances(&self.draw_vars);
        if let Some(mut mi) = mi {
            for instance in instances {
                self.origin = instance.origin;
                self.basis_x = instance.basis_x;
                self.basis_y = instance.basis_y;
                self.basis_z = instance.basis_z;
                self.scale = instance.scale;
                self.tint = instance.tint;
                self.flutter = instance.flutter;
                mi.instances.extend_from_slice(self.draw_vars.as_slice());
            }
            let new_area = cx.end_many_instances(mi);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}
