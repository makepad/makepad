use super::mesh_generators::{stylized_leaf_mesh, tree_branch_segment_mesh};
use makepad_widgets::{draw_list_2d::ManyInstances, *};
use std::f32::consts::{FRAC_1_SQRT_2, PI};

pub const PYTHAGOREAN_TREE_ROOT_DROP: f32 = 0.60;

const TREE_BRANCH_SIDES: usize = 4;
const TREE_MAX_DEPTH: usize = 9;
const TREE_BASE_LENGTH: f32 = 0.46;
const TREE_BASE_RADIUS: f32 = 0.026;
const TREE_CHILD_SCALE: f32 = FRAC_1_SQRT_2;
const TREE_RADIUS_SCALE: f32 = 0.74;
const TREE_BRANCH_SPLIT_ANGLE: f32 = 0.70;
const TREE_POINTER_RADIUS: f32 = 0.40;
const TREE_BRANCH_PARENT_DRAG: f32 = 0.42;
const TREE_BRANCH_HAND_GAIN: f32 = 0.19;
const TREE_LEAF_HAND_GAIN: f32 = 0.34;
const TREE_MAX_POINT_PUSH: f32 = 0.24;
const TREE_LEAF_BASE_SCALE: f32 = 0.056;
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
const TREE_LEAF_WIND_ORBIT: f32 = 0.015;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawTreeBranches = mod.std.set_type_default() do #(DrawTreeBranches::script_shader(vm)){
        backface_culling: false
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

#[derive(Default)]
pub struct CpuPythagoreanTree {
    branch_geometry: Option<Geometry>,
    leaf_geometry: Option<Geometry>,
    branch_templates: Vec<BranchTemplate>,
    leaf_templates: Vec<LeafTemplate>,
    branch_runtime: Vec<BranchRuntime>,
    branch_instances: Vec<BranchInstance>,
    leaf_instances: Vec<LeafInstance>,
}

impl CpuPythagoreanTree {
    pub fn ensure_geometry(&mut self, cx: &mut Cx2d) {
        if self.branch_templates.is_empty() {
            self.rebuild_templates();
        }
        if self.branch_geometry.is_none() {
            self.branch_geometry =
                Some(tree_branch_segment_mesh(TREE_BRANCH_SIDES, 0.84).into_geometry(cx.cx.cx));
        }
        if self.leaf_geometry.is_none() {
            self.leaf_geometry = Some(stylized_leaf_mesh().into_geometry(cx.cx.cx));
        }
    }

    pub fn rebuild_instances(&mut self, root: Pose, state: &XrState) {
        if self.branch_templates.is_empty() {
            self.rebuild_templates();
        }
        self.branch_instances.clear();
        self.leaf_instances.clear();
        self.branch_runtime
            .resize(self.branch_templates.len(), BranchRuntime::default());

        let root_inverse = root.invert();
        let mut pointer_tips = [None, None];
        pointer_tips[0] = Self::pointer_tip_world(&state.left_hand)
            .map(|tip_world| root_inverse.transform_vec3(&tip_world));
        pointer_tips[1] = Self::pointer_tip_world(&state.right_hand)
            .map(|tip_world| root_inverse.transform_vec3(&tip_world));
        let time = state.time as f32;
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

            let local_dir = branch
                .local_rotation
                .rotate_vec3(&vec3f(0.0, 1.0, 0.0));
            let local_z = branch
                .local_rotation
                .rotate_vec3(&vec3f(0.0, 0.0, 1.0));
            let base_dir = Self::normalize_or(
                Self::frame_vector(parent_basis_x, parent_basis_y, parent_basis_z, local_dir),
                parent_basis_y,
            );
            let z_hint = Self::normalize_or(
                Self::frame_vector(parent_basis_x, parent_basis_y, parent_basis_z, local_z),
                parent_basis_z,
            );
            let nominal_tip = start + base_dir * branch.length;
            let midpoint = start + (nominal_tip - start) * 0.55;
            let level_t = branch.level as f32 / level_den;

            let mut point_push =
                Self::branch_wind_force(branch.seed, level_t, time) + inherited_push;
            for pointer_tip in pointer_tips.into_iter().flatten() {
                point_push += Self::pointer_force(
                    nominal_tip,
                    pointer_tip,
                    TREE_BRANCH_HAND_GAIN * (0.70 + level_t * 0.95),
                    0.82,
                );
                point_push += Self::pointer_force(
                    midpoint,
                    pointer_tip,
                    TREE_BRANCH_HAND_GAIN * (0.34 + level_t * 0.38),
                    0.56,
                ) * 0.38;
            }
            point_push = Self::clamp_len(point_push, TREE_MAX_POINT_PUSH);

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

            let origin_world = root.transform_vec3(&start);
            self.branch_instances.push(BranchInstance {
                origin: origin_world,
                basis_x: root.orientation.rotate_vec3(&basis_x),
                basis_y: root.orientation.rotate_vec3(&basis_y),
                basis_z: root.orientation.rotate_vec3(&basis_z),
                scale: vec3f(branch.radius, branch.length, branch.radius),
                level: level_t,
                seed: branch.seed,
            });
        }

        for leaf in &self.leaf_templates {
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
            let local_leaf_y = leaf
                .local_rotation
                .rotate_vec3(&vec3f(0.0, 1.0, 0.0));
            let local_leaf_z = leaf
                .local_rotation
                .rotate_vec3(&vec3f(0.0, 0.0, 1.0));
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

            let orbit_phase = time * (1.45 + level_t * 0.55) + leaf.seed * 8.7;
            let anchor_offset =
                branch_runtime.basis_x * (orbit_phase.cos() * TREE_LEAF_WIND_ORBIT * leaf.scale * 1.8)
                    + branch_runtime.basis_y
                        * (orbit_phase.sin() * TREE_LEAF_WIND_ORBIT * leaf.scale * 0.7)
                    + branch_runtime.basis_z
                        * ((orbit_phase * 1.31).sin() * TREE_LEAF_WIND_ORBIT * leaf.scale * 1.6);
            let origin = anchor + anchor_offset;
            let leaf_center = origin + nominal_leaf_y * (leaf.scale * 0.78);

            let mut leaf_push = Self::leaf_wind_force(leaf.seed, level_t, time)
                + branch_runtime.point_push * (1.35 + level_t * 0.2);
            for pointer_tip in pointer_tips.into_iter().flatten() {
                leaf_push += Self::pointer_force(
                    leaf_center,
                    pointer_tip,
                    TREE_LEAF_HAND_GAIN * (0.74 + level_t * 0.58),
                    1.0,
                );
            }
            leaf_push = Self::clamp_len(leaf_push, TREE_MAX_POINT_PUSH * 1.35);

            let leaf_y = Self::normalize_or(nominal_leaf_y + leaf_push * 1.6, nominal_leaf_y);
            let (basis_x, basis_y, basis_z) = Self::orthonormal_frame(leaf_y, leaf_z_hint);
            let origin_world = root.transform_vec3(&origin);
            let flutter = ((leaf_y - nominal_leaf_y).length() * 1.45 + leaf_push.length() * 0.3)
                .clamp(0.0, 1.0);

            self.leaf_instances.push(LeafInstance {
                origin: origin_world,
                basis_x: root.orientation.rotate_vec3(&basis_x),
                basis_y: root.orientation.rotate_vec3(&basis_y),
                basis_z: root.orientation.rotate_vec3(&basis_z),
                scale: vec3f(leaf.scale, leaf.scale, leaf.scale),
                tint: leaf.tint,
                flutter,
            });
        }
    }

    pub fn draw(
        &mut self,
        cx: &mut Cx2d,
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
        draw_leaves.draw_instances(cx, leaf_geometry.geometry_id(), camera_pos, &self.leaf_instances);
    }

    fn rebuild_templates(&mut self) {
        self.branch_templates.clear();
        self.leaf_templates.clear();
        self.push_branch(None, Quat::default(), TREE_BASE_LENGTH, TREE_BASE_RADIUS, 0, 0.137);
    }

    fn push_branch(
        &mut self,
        parent: Option<usize>,
        local_rotation: Quat,
        length: f32,
        radius: f32,
        level: usize,
        seed: f32,
    ) {
        let branch_index = self.branch_templates.len();
        self.branch_templates.push(BranchTemplate {
            parent,
            local_rotation,
            length,
            radius,
            level: level as u8,
            seed,
        });

        if level + 2 >= TREE_MAX_DEPTH {
            self.append_leaf_cluster(branch_index, level, seed);
        }
        if level + 1 >= TREE_MAX_DEPTH {
            return;
        }

        let split_angle = TREE_BRANCH_SPLIT_ANGLE + (seed * 5.31).sin() * 0.08;
        let twist = 0.42 + (seed * 9.17).cos() * 0.34 + level as f32 * 0.07;
        let tilt_axis = if level.is_multiple_of(2) {
            vec3f(1.0, 0.0, 0.0)
        } else {
            vec3f(0.0, 0.0, 1.0)
        };
        let left_rotation = Quat::multiply(
            &Quat::from_axis_angle(tilt_axis, split_angle),
            &Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), twist),
        );
        let right_rotation = Quat::multiply(
            &Quat::from_axis_angle(tilt_axis, -split_angle * 0.97),
            &Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), -twist * 0.92),
        );
        let child_length = length * TREE_CHILD_SCALE;
        let child_radius = (radius * TREE_RADIUS_SCALE).max(0.004);

        self.push_branch(
            Some(branch_index),
            left_rotation,
            child_length,
            child_radius,
            level + 1,
            seed * 1.71 + 0.23,
        );
        self.push_branch(
            Some(branch_index),
            right_rotation,
            child_length,
            child_radius,
            level + 1,
            seed * 1.47 + 0.61,
        );
    }

    fn append_leaf_cluster(&mut self, branch: usize, level: usize, seed: f32) {
        let density = match level {
            0..=3 => 0,
            4 => 2,
            5 => 3,
            6 => 4,
            7 => 4,
            _ => 5,
        };
        let branch_depth = level as f32 / (TREE_MAX_DEPTH.saturating_sub(1)).max(1) as f32;
        for index in 0..density {
            let angle = (index as f32 / density as f32) * PI * 2.0 + seed * 7.3;
            let roll = (index as f32 * 1.618 + seed * 13.7).sin() * 0.55;
            let pitch = -0.42 + (index as f32 * 0.87 + seed * 3.9).cos() * 0.18;
            let tilt_axis = vec3f(angle.cos(), 0.0, angle.sin()).normalize();
            let local_rotation = Quat::multiply(
                &Quat::from_axis_angle(vec3f(0.0, 0.0, 1.0), pitch),
                &Quat::multiply(
                    &Quat::from_axis_angle(tilt_axis, 0.48 + roll * 0.18),
                    &Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), angle),
                ),
            );
            let spread = 0.015 + branch_depth * 0.028;
            self.leaf_templates.push(LeafTemplate {
                branch,
                local_offset: vec3f(
                    angle.cos() * spread,
                    -0.018 + branch_depth * 0.018,
                    angle.sin() * spread,
                ),
                local_rotation,
                scale: TREE_LEAF_BASE_SCALE * (0.88 + branch_depth * 0.24)
                    * (0.86 + (index as f32 * 1.31 + seed * 8.1).sin().abs() * 0.24),
                tint: (0.35 + 0.5 * (seed * 11.9 + index as f32 * 0.73).sin()).clamp(0.0, 1.0),
                seed: seed * 2.13 + index as f32 * 0.37,
            });
        }
    }

    fn pointer_tip_world(hand: &XrHand) -> Option<Vec3f> {
        if !hand.in_view() || !hand.tip_active(XrHand::INDEX_TIP) {
            return None;
        }
        let tip_len = hand.tips[XrHand::INDEX_TIP].max(0.0);
        Some(
            hand.joints[XrHand::INDEX_KNUCKLE3]
                .to_mat4()
                .transform_vec4(vec4(0.0, 0.0, -tip_len, 1.0))
                .to_vec3f(),
        )
    }

    fn branch_wind_force(seed: f32, level_t: f32, time: f32) -> Vec3f {
        let sway = 0.012 + level_t * 0.040;
        vec3f(
            (time * 0.77 + seed * 6.7).sin() * 0.75 + (time * 1.33 + seed * 2.3).cos() * 0.25,
            0.0,
            (time * 0.61 + seed * 4.1).cos() * 0.68 + (time * 1.17 + seed * 3.4).sin() * 0.32,
        ) * sway
    }

    fn leaf_wind_force(seed: f32, level_t: f32, time: f32) -> Vec3f {
        let sway = 0.045 + level_t * 0.075;
        vec3f(
            (time * 1.87 + seed * 9.4).sin() * 0.6 + (time * 2.41 + seed * 3.7).cos() * 0.4,
            0.06 + (time * 2.93 + seed * 7.2).sin() * 0.12,
            (time * 2.13 + seed * 5.1).cos() * 0.7 + (time * 1.61 + seed * 1.9).sin() * 0.3,
        ) * sway
    }

    fn pointer_force(sample: Vec3f, pointer: Vec3f, gain: f32, vertical_scale: f32) -> Vec3f {
        let delta = sample - pointer;
        let distance = delta.length().max(0.0001);
        if distance >= TREE_POINTER_RADIUS {
            return vec3f(0.0, 0.0, 0.0);
        }
        let falloff = (1.0 - distance / TREE_POINTER_RADIUS).powf(2.2);
        let mut away = delta / distance;
        away.y *= vertical_scale;
        let lateral = if away.length() > 0.0001 {
            away.normalize()
        } else {
            vec3f(0.0, 0.0, 1.0)
        };
        lateral * (falloff * gain)
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

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawTreeBranches {
    #[rust]
    many_instances: Option<ManyInstances>,
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
    fn apply_uniforms(&mut self, cx: &mut Cx2d, camera_pos: Vec3f) {
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
            &[TREE_BRANCH_BARK_DARK.x, TREE_BRANCH_BARK_DARK.y, TREE_BRANCH_BARK_DARK.z],
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
        cx: &mut Cx2d,
        geometry_id: GeometryId,
        camera_pos: Vec3f,
        instances: &[BranchInstance],
    ) {
        if instances.is_empty() {
            return;
        }
        self.draw_vars.append_group_id = cx.draw_call_group_background().0;
        self.draw_vars.geometry_id = Some(geometry_id);
        self.draw_vars.options.depth_write = true;
        self.depth_clip = 1.0;
        self.apply_uniforms(cx, camera_pos);
        self.many_instances = cx.begin_many_aligned_instances(&self.draw_vars);
        for instance in instances {
            self.origin = instance.origin;
            self.basis_x = instance.basis_x;
            self.basis_y = instance.basis_y;
            self.basis_z = instance.basis_z;
            self.scale = instance.scale;
            self.level = instance.level;
            self.seed = instance.seed;
            if let Some(many_instances) = self.many_instances.as_mut() {
                many_instances
                    .instances
                    .extend_from_slice(self.draw_vars.as_slice());
            }
        }
        if let Some(many_instances) = self.many_instances.take() {
            let new_area = cx.end_many_instances(many_instances);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawTreeLeaves {
    #[rust]
    many_instances: Option<ManyInstances>,
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
    fn apply_uniforms(&mut self, cx: &mut Cx2d, camera_pos: Vec3f) {
        let light_dir = TREE_LEAF_LIGHT_DIR.normalize();
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_tree_light_dir),
            &[light_dir.x, light_dir.y, light_dir.z],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_tree_light_color),
            &[TREE_LEAF_LIGHT_COLOR.x, TREE_LEAF_LIGHT_COLOR.y, TREE_LEAF_LIGHT_COLOR.z],
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
        cx: &mut Cx2d,
        geometry_id: GeometryId,
        camera_pos: Vec3f,
        instances: &[LeafInstance],
    ) {
        if instances.is_empty() {
            return;
        }
        self.draw_vars.append_group_id = cx.draw_call_group_background().0;
        self.draw_vars.geometry_id = Some(geometry_id);
        self.draw_vars.options.depth_write = true;
        self.depth_clip = 1.0;
        self.apply_uniforms(cx, camera_pos);
        self.many_instances = cx.begin_many_aligned_instances(&self.draw_vars);
        for instance in instances {
            self.origin = instance.origin;
            self.basis_x = instance.basis_x;
            self.basis_y = instance.basis_y;
            self.basis_z = instance.basis_z;
            self.scale = instance.scale;
            self.tint = instance.tint;
            self.flutter = instance.flutter;
            if let Some(many_instances) = self.many_instances.as_mut() {
                many_instances
                    .instances
                    .extend_from_slice(self.draw_vars.as_slice());
            }
        }
        if let Some(many_instances) = self.many_instances.take() {
            let new_area = cx.end_many_instances(many_instances);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}
