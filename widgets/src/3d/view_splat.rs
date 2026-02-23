use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};
use makepad_splat::{load_splat_from_bytes, SplatFileFormat, SplatScene};
use std::{mem, path::PathBuf, rc::Rc, sync::mpsc::TryRecvError};

use super::scene_3d::{
    apply_scene_to_draw_pbr, register_last_draw_call_anchor, scene_state_from_scope, SceneState3D,
};
use crate::makepad_draw::shader::draw_pbr::PbrMeshHandle;

script_mod! {
    use mod.prelude.widgets_internal.*

    set_type_default() do #(DrawSplatPbr::script_shader(vm)){
        ..mod.draw.DrawPbr
        render_size: vec2(1024.0, 768.0)
        focal_pixels: vec2(512.0, 384.0)
        ndc_per_pixel: vec2(0.001953125, 0.0026041667)
        coarse_cull_guard: 2.0
        fast_project_mode: 0.0
        splat_std_dev: 2.8
        min_pixel_radius: 0.0
        max_pixel_radius: 512.0
        blur_pixels: 0.3
        alpha_cutoff: 0.002
        dither_depth_cutout: 0.0
        dither_scale: 1.0
        v_ndc: varying(vec2f)

        vertex: fn() {
            let quad = vec2(self.geom.ny_nz_uv.z, self.geom.ny_nz_uv.w);
            let center_local = vec3(self.geom.pos_nx.x, self.geom.pos_nx.y, self.geom.pos_nx.z);
            let axis_local_0 = vec3(self.geom.pos_nx.w, self.geom.ny_nz_uv.x, self.geom.ny_nz_uv.y);
            let axis_local_1 = vec3(self.geom.tangent.x, self.geom.tangent.y, self.geom.tangent.z);
            let axis_2_len = max(abs(self.geom.tangent.w), 0.000001);
            let center_world4 = self.model_matrix * vec4(center_local.x, center_local.y, center_local.z, 1.0);
            let center_world = vec3(center_world4.x, center_world4.y, center_world4.z);
            let center_view4 = self.view_matrix * center_world4;
            let center_view = center_view4.xyz;
            if center_view.z >= -0.000001 {
                self.v_world = center_world;
                self.v_normal = vec3(0.0, 0.0, 1.0);
                self.v_tangent = vec4(1.0, 0.0, 0.0, 1.0);
                self.v_uv = vec2(0.0, 0.0);
                self.v_ndc = vec2(4.0, 4.0);
                self.v_color = vec4(0.0, 0.0, 0.0, 0.0);
                self.vertex_pos = vec4(2.0, 2.0, 2.0, 1.0);
                return;
            }

            let clip_center = self.projection_matrix * center_view4;
            let center_inv_w = 1.0 / max(abs(clip_center.w), 0.000001);
            let center_ndc = clip_center.xy * center_inv_w;

            let focal = vec2(max(self.focal_pixels.x, 0.00001), max(self.focal_pixels.y, 0.00001));
            let inv_depth = 1.0 / max(-center_view.z, 0.000001);
            let axis0_bound = max(abs(axis_local_0.x), max(abs(axis_local_0.y), abs(axis_local_0.z)));
            let axis1_bound = max(abs(axis_local_1.x), max(abs(axis_local_1.y), abs(axis_local_1.z)));
            let max_scale = 1.732051 * max(axis0_bound, max(axis1_bound, axis_2_len));
            let cull_guard = max(self.coarse_cull_guard, 0.0);
            let ndc_guard = 1.0 + cull_guard * max(abs(center_ndc.x), abs(center_ndc.y));
            let rough_radius_px = self.splat_std_dev * max_scale * max(focal.x, focal.y) * inv_depth * ndc_guard;
            if rough_radius_px < self.min_pixel_radius {
                self.v_world = center_world;
                self.v_normal = vec3(0.0, 0.0, 1.0);
                self.v_tangent = vec4(1.0, 0.0, 0.0, 1.0);
                self.v_uv = vec2(0.0, 0.0);
                self.v_ndc = vec2(4.0, 4.0);
                self.v_color = vec4(0.0, 0.0, 0.0, 0.0);
                self.vertex_pos = vec4(2.0, 2.0, 2.0, 1.0);
                return;
            }

            let ndc_per_pixel = vec2(max(self.ndc_per_pixel.x, 0.000001), max(self.ndc_per_pixel.y, 0.000001));
            let rough_ndc = vec2(rough_radius_px * ndc_per_pixel.x, rough_radius_px * ndc_per_pixel.y);
            if center_ndc.x < (-1.0 - rough_ndc.x)
                || center_ndc.x > (1.0 + rough_ndc.x)
                || center_ndc.y < (-1.0 - rough_ndc.y)
                || center_ndc.y > (1.0 + rough_ndc.y)
            {
                self.v_world = center_world;
                self.v_normal = vec3(0.0, 0.0, 1.0);
                self.v_tangent = vec4(1.0, 0.0, 0.0, 1.0);
                self.v_uv = vec2(0.0, 0.0);
                self.v_ndc = vec2(4.0, 4.0);
                self.v_color = vec4(0.0, 0.0, 0.0, 0.0);
                self.vertex_pos = vec4(2.0, 2.0, 2.0, 1.0);
                return;
            }

            let axis_world_0 = (self.model_matrix * vec4(axis_local_0.x, axis_local_0.y, axis_local_0.z, 0.0)).xyz;
            let axis_world_1 = (self.model_matrix * vec4(axis_local_1.x, axis_local_1.y, axis_local_1.z, 0.0)).xyz;
            let axis_view_0 = (self.view_matrix * vec4(axis_world_0.x, axis_world_0.y, axis_world_0.z, 0.0)).xyz;
            let axis_view_1 = (self.view_matrix * vec4(axis_world_1.x, axis_world_1.y, axis_world_1.z, 0.0)).xyz;

            let max_radius = if self.max_pixel_radius > 0.0 { self.max_pixel_radius } else { 1000000.0 };
            if self.fast_project_mode > 0.5 {
                let clip_axis_0 = self.projection_matrix
                    * (center_view4 + vec4(axis_view_0.x, axis_view_0.y, axis_view_0.z, 0.0));
                let clip_axis_1 = self.projection_matrix
                    * (center_view4 + vec4(axis_view_1.x, axis_view_1.y, axis_view_1.z, 0.0));

                let inv_w0 = 1.0 / max(abs(clip_axis_0.w), 0.000001);
                let inv_w1 = 1.0 / max(abs(clip_axis_1.w), 0.000001);
                let ndc_axis_0 = clip_axis_0.xy * inv_w0 - center_ndc;
                let ndc_axis_1 = clip_axis_1.xy * inv_w1 - center_ndc;

                let mut px_axis_0 = vec2(
                    (ndc_axis_0.x / ndc_per_pixel.x) * self.splat_std_dev,
                    (ndc_axis_0.y / ndc_per_pixel.y) * self.splat_std_dev,
                );
                let mut px_axis_1 = vec2(
                    (ndc_axis_1.x / ndc_per_pixel.x) * self.splat_std_dev,
                    (ndc_axis_1.y / ndc_per_pixel.y) * self.splat_std_dev,
                );

                let radius_0 = length(px_axis_0);
                let radius_1 = length(px_axis_1);
                if radius_0 < self.min_pixel_radius && radius_1 < self.min_pixel_radius {
                    self.v_world = center_world;
                    self.v_normal = vec3(0.0, 0.0, 1.0);
                    self.v_tangent = vec4(1.0, 0.0, 0.0, 1.0);
                    self.v_uv = vec2(0.0, 0.0);
                    self.v_ndc = vec2(4.0, 4.0);
                    self.v_color = vec4(0.0, 0.0, 0.0, 0.0);
                    self.vertex_pos = vec4(2.0, 2.0, 2.0, 1.0);
                    return;
                }

                if radius_0 > max_radius {
                    px_axis_0 = px_axis_0 * (max_radius / max(radius_0, 0.000001));
                }
                if radius_1 > max_radius {
                    px_axis_1 = px_axis_1 * (max_radius / max(radius_1, 0.000001));
                }

                let px_offset = px_axis_0 * quad.x + px_axis_1 * quad.y;
                let ndc = center_ndc + vec2(px_offset.x * ndc_per_pixel.x, px_offset.y * ndc_per_pixel.y);

                self.v_world = center_world;
                self.v_normal = vec3(0.0, 0.0, 1.0);
                self.v_tangent = vec4(1.0, 0.0, 0.0, 1.0);
                self.v_uv = quad * self.splat_std_dev;
                self.v_ndc = ndc;
                self.v_color = vec4(self.geom.color.x, self.geom.color.y, self.geom.color.z, self.geom.color.w);
                self.vertex_pos = vec4(ndc.x * clip_center.w, ndc.y * clip_center.w, clip_center.z, clip_center.w);
                return;
            }

            let axis_local_2_raw = cross(axis_local_0, axis_local_1);
            let axis_local_2_raw_len = length(axis_local_2_raw);
            let axis_local_2 = if axis_local_2_raw_len > 0.000001 {
                axis_local_2_raw * (axis_2_len / axis_local_2_raw_len)
            } else {
                vec3(0.0, 0.0, axis_2_len)
            };
            let axis_world_2 = (self.model_matrix * vec4(axis_local_2.x, axis_local_2.y, axis_local_2.z, 0.0)).xyz;
            let axis_view_2 = (self.view_matrix * vec4(axis_world_2.x, axis_world_2.y, axis_world_2.z, 0.0)).xyz;

            let c00 = axis_view_0.x * axis_view_0.x + axis_view_1.x * axis_view_1.x + axis_view_2.x * axis_view_2.x;
            let c01 = axis_view_0.x * axis_view_0.y + axis_view_1.x * axis_view_1.y + axis_view_2.x * axis_view_2.y;
            let c02 = axis_view_0.x * axis_view_0.z + axis_view_1.x * axis_view_1.z + axis_view_2.x * axis_view_2.z;
            let c11 = axis_view_0.y * axis_view_0.y + axis_view_1.y * axis_view_1.y + axis_view_2.y * axis_view_2.y;
            let c12 = axis_view_0.y * axis_view_0.z + axis_view_1.y * axis_view_1.z + axis_view_2.y * axis_view_2.z;
            let c22 = axis_view_0.z * axis_view_0.z + axis_view_1.z * axis_view_1.z + axis_view_2.z * axis_view_2.z;

            let inv_z = 1.0 / center_view.z;
            let jx = focal.x * inv_z;
            let jy = focal.y * inv_z;
            let jzx = -(jx * center_view.x) * inv_z;
            let jzy = -(jy * center_view.y) * inv_z;

            let mut a = jx * jx * c00 + 2.0 * jx * jzx * c02 + jzx * jzx * c22;
            let mut b = jx * jy * c01 + jx * jzy * c02 + jzx * jy * c12 + jzx * jzy * c22;
            let mut d = jy * jy * c11 + 2.0 * jy * jzy * c12 + jzy * jzy * c22;

            let det_orig = a * d - b * b;
            a = a + self.blur_pixels;
            d = d + self.blur_pixels;
            let det = a * d - b * b;
            let blur_adjust = if det_orig > 0.0 && det > 0.0 {
                sqrt(max(det_orig / det, 0.0))
            } else {
                1.0
            };

            let eigen_avg = 0.5 * (a + d);
            let eigen_delta = sqrt(max(0.0, eigen_avg * eigen_avg - det));
            let eigen_0 = max(eigen_avg + eigen_delta, 0.000001);
            let eigen_1 = max(eigen_avg - eigen_delta, 0.000001);
            let axis_0 = if abs(b) < 0.001 {
                vec2(1.0, 0.0)
            } else {
                normalize(vec2(b, eigen_0 - a))
            };
            let axis_1 = vec2(axis_0.y, -axis_0.x);

            let scale_0 = min(max_radius, self.splat_std_dev * sqrt(eigen_0));
            let scale_1 = min(max_radius, self.splat_std_dev * sqrt(eigen_1));
            if scale_0 < self.min_pixel_radius && scale_1 < self.min_pixel_radius {
                self.v_world = center_world;
                self.v_normal = vec3(0.0, 0.0, 1.0);
                self.v_tangent = vec4(1.0, 0.0, 0.0, 1.0);
                self.v_uv = vec2(0.0, 0.0);
                self.v_ndc = vec2(4.0, 4.0);
                self.v_color = vec4(0.0, 0.0, 0.0, 0.0);
                self.vertex_pos = vec4(2.0, 2.0, 2.0, 1.0);
                return;
            }
            let pixel_offset = axis_0 * (quad.x * scale_0) + axis_1 * (quad.y * scale_1);
            let ndc_offset = vec2(pixel_offset.x * ndc_per_pixel.x, pixel_offset.y * ndc_per_pixel.y);
            let ndc = center_ndc + ndc_offset;

            self.v_world = center_world;
            self.v_normal = vec3(0.0, 0.0, 1.0);
            self.v_tangent = vec4(1.0, 0.0, 0.0, 1.0);
            self.v_uv = quad * self.splat_std_dev;
            self.v_ndc = ndc;
            self.v_color = vec4(self.geom.color.x, self.geom.color.y, self.geom.color.z, self.geom.color.w * blur_adjust);
            self.vertex_pos = vec4(ndc.x * clip_center.w, ndc.y * clip_center.w, clip_center.z, clip_center.w);
        }

        pixel: fn() {
            let r2 = dot(self.v_uv, self.v_uv);
            let max_r2 = self.splat_std_dev * self.splat_std_dev;
            if r2 > max_r2 {
                discard();
            }

            let alpha = exp(-0.5 * r2) * self.v_color.w;
            if alpha < self.alpha_cutoff {
                discard();
            }

            let rgb = self.v_color.xyz;
            if self.dither_depth_cutout > 0.5 {
                // Pixel-space stochastic transparency: alpha-to-coverage style cutout
                // with depth writes enabled.
                let ndc = self.v_ndc;

                // Reconstruct approximate pixel coordinates from NDC using derivatives.
                let ndc_dx = vec2(dFdx(ndc.x), dFdy(ndc.x));
                let ndc_dy = vec2(dFdx(ndc.y), dFdy(ndc.y));
                let px_ndc = vec2(max(length(ndc_dx), 0.000001), max(length(ndc_dy), 0.000001));
                let frag_px = ndc / px_ndc;
                let cell = floor(frag_px * self.dither_scale + vec2(0.5, 0.5));
                let threshold = Math.random_2d(cell);
                if alpha < threshold {
                    discard();
                }
                return vec4(rgb.x, rgb.y, rgb.z, 1.0)
            }
            return vec4(rgb.x * alpha, rgb.y * alpha, rgb.z * alpha, alpha)
        }
    }

    mod.widgets.ViewSplatBase = #(ViewSplat::register_widget(vm))
    mod.widgets.ViewSplat = set_type_default() do mod.widgets.ViewSplatBase{
        draw_splat +: {
            render_size: vec2(1024.0, 768.0)
            focal_pixels: vec2(512.0, 384.0)
            ndc_per_pixel: vec2(0.001953125, 0.0026041667)
            coarse_cull_guard: 2.0
            fast_project_mode: 0.0
            splat_std_dev: 2.8
            min_pixel_radius: 0.0
            max_pixel_radius: 512.0
            blur_pixels: 0.3
            alpha_cutoff: 0.002
            dither_depth_cutout: 0.0
            dither_scale: 1.0
        }
        max_splats: 0
        radius_scale: 1.1
        min_radius: 0.0015
        normalize_fit: 2.2
        opacity_scale: 1.0
        auto_normalize: true
        auto_antialias_blur: true
        sort_back_to_front: true
        sort_min_camera_angle_deg: 0.25
        sort_min_camera_move: 0.02
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSplatPbr {
    #[deref]
    pub draw_super: DrawPbr,
    #[live(vec2(1024.0, 768.0))]
    pub render_size: Vec2f,
    #[live(vec2(512.0, 384.0))]
    pub focal_pixels: Vec2f,
    #[live(vec2(0.001953125, 0.0026041667))]
    pub ndc_per_pixel: Vec2f,
    #[live(2.0)]
    pub coarse_cull_guard: f32,
    #[live(0.0)]
    pub fast_project_mode: f32,
    #[live(2.8)]
    pub splat_std_dev: f32,
    #[live(0.0)]
    pub min_pixel_radius: f32,
    #[live(512.0)]
    pub max_pixel_radius: f32,
    #[live(0.3)]
    pub blur_pixels: f32,
    #[live(0.002)]
    pub alpha_cutoff: f32,
    #[live(0.0)]
    pub dither_depth_cutout: f32,
    #[live(1.0)]
    pub dither_scale: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ViewSplat {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[redraw]
    #[live]
    draw_splat: DrawSplatPbr,

    #[live]
    src: Option<ScriptHandleRef>,
    #[live]
    env_src: Option<ScriptHandleRef>,

    #[live(vec3(0.0, 0.0, 0.0))]
    position: Vec3f,
    #[live(vec3(0.0, 0.0, 0.0))]
    rotation: Vec3f,
    #[live(vec3(1.0, 1.0, 1.0))]
    scale: Vec3f,

    #[live(0u32)]
    max_splats: u32,
    #[live(1.1)]
    radius_scale: f32,
    #[live(0.0015)]
    min_radius: f32,
    #[live(2.2)]
    normalize_fit: f32,
    #[live(1.0)]
    opacity_scale: f32,
    #[live(true)]
    auto_normalize: bool,
    #[live(true)]
    auto_antialias_blur: bool,
    #[live(true)]
    sort_back_to_front: bool,
    #[live(0.25)]
    sort_min_camera_angle_deg: f32,
    #[live(0.02)]
    sort_min_camera_move: f32,

    #[rust]
    scene: Option<SplatScene>,
    #[rust]
    loaded_src_handle: Option<ScriptHandle>,
    #[rust]
    loaded_env_handle: Option<ScriptHandle>,
    #[rust]
    splat_mesh: Option<PbrMeshHandle>,
    #[rust]
    base_indices: Vec<u32>,
    #[rust(true)]
    base_indices_applied: bool,
    #[rust(vec3(0.0, 0.0, 0.0))]
    scene_center: Vec3f,
    #[rust(1.0)]
    scene_unit_scale: f32,
    #[rust]
    depth_sort_request_tx: FromUISender<SplatSortRequest>,
    #[rust]
    depth_sort_result_rx: ToUIReceiver<SplatSortResult>,
    #[rust(false)]
    depth_sort_thread_started: bool,
    #[rust(1u64)]
    depth_sort_generation: u64,
    #[rust(0u64)]
    depth_sort_scene_uploaded_generation: u64,
    #[rust(0u64)]
    depth_sort_next_request_id: u64,
    #[rust(0u64)]
    depth_sort_last_applied_request_id: u64,
    #[rust(false)]
    depth_sort_in_flight: bool,
    #[rust]
    depth_sort_centers_local: Vec<[f32; 3]>,
    #[rust]
    depth_sort_pending_result: Option<SplatSortResult>,
    #[rust]
    depth_sort_last_camera_pos: Option<Vec3f>,
    #[rust]
    depth_sort_last_camera_forward: Option<Vec3f>,
    #[rust]
    depth_sort_last_depth_plane: Option<Vec4f>,
}

enum ResourceResolve {
    Ready {
        handle: ScriptHandle,
        abs_path: PathBuf,
        data: Rc<Vec<u8>>,
    },
    Pending {
        handle: ScriptHandle,
    },
    Error {
        handle: ScriptHandle,
    },
    Missing,
}

enum SplatSortRequest {
    SetScene {
        generation: u64,
        centers_local: Vec<[f32; 3]>,
    },
    Sort {
        generation: u64,
        request_id: u64,
        view_matrix: Mat4f,
        model_matrix: Mat4f,
        camera_pos: Vec3f,
        sort_radial: bool,
    },
}

struct SplatSortResult {
    generation: u64,
    request_id: u64,
    indices: Vec<u32>,
}

fn float_depth_key(depth: f32) -> u32 {
    let bits = depth.to_bits();
    if (bits & 0x8000_0000) != 0 {
        !bits
    } else {
        bits ^ 0x8000_0000
    }
}

fn local_depth_from_view_model(view_matrix: &Mat4f, model_matrix: &Mat4f, center: [f32; 3]) -> f32 {
    let local = vec4(center[0], center[1], center[2], 1.0);
    let world = model_matrix.transform_vec4(local);
    let view = view_matrix.transform_vec4(world);
    view.z
}

fn local_sort_metric(
    view_matrix: &Mat4f,
    model_matrix: &Mat4f,
    camera_pos: Vec3f,
    center: [f32; 3],
    sort_radial: bool,
) -> f32 {
    if sort_radial {
        let local = vec4(center[0], center[1], center[2], 1.0);
        let world = model_matrix.transform_vec4(local);
        let dx = world.x - camera_pos.x;
        let dy = world.y - camera_pos.y;
        let dz = world.z - camera_pos.z;
        -(dx * dx + dy * dy + dz * dz)
    } else {
        local_depth_from_view_model(view_matrix, model_matrix, center)
    }
}

fn sort_splats_radix(
    view_matrix: Mat4f,
    model_matrix: Mat4f,
    camera_pos: Vec3f,
    sort_radial: bool,
    centers_local: &[[f32; 3]],
    keys: &mut Vec<u32>,
    order_a: &mut Vec<u32>,
    order_b: &mut Vec<u32>,
) {
    let count = centers_local.len();
    keys.resize(count, 0);
    order_a.resize(count, 0);
    order_b.resize(count, 0);

    for (index, center) in centers_local.iter().enumerate() {
        let metric = local_sort_metric(
            &view_matrix,
            &model_matrix,
            camera_pos,
            *center,
            sort_radial,
        );
        keys[index] = float_depth_key(metric);
        order_a[index] = index as u32;
    }

    let mut counts = [0usize; 256];
    let mut offsets = [0usize; 256];
    for shift in [0u32, 8, 16, 24] {
        counts.fill(0);
        for &idx in order_a.iter().take(count) {
            let bucket = ((keys[idx as usize] >> shift) & 0xff) as usize;
            counts[bucket] += 1;
        }

        let mut prefix = 0usize;
        for bucket in 0..256 {
            offsets[bucket] = prefix;
            prefix += counts[bucket];
        }

        for &idx in order_a.iter().take(count) {
            let bucket = ((keys[idx as usize] >> shift) & 0xff) as usize;
            let out_idx = offsets[bucket];
            order_b[out_idx] = idx;
            offsets[bucket] = out_idx + 1;
        }
        mem::swap(order_a, order_b);
    }
}

fn build_sorted_triangle_indices(order: &[u32], indices: &mut Vec<u32>) {
    let count = order.len();
    indices.resize(count.saturating_mul(6), 0);
    for (i, &splat_index) in order.iter().enumerate() {
        let base = splat_index * 4;
        let dst = i * 6;
        indices[dst] = base;
        indices[dst + 1] = base + 1;
        indices[dst + 2] = base + 2;
        indices[dst + 3] = base;
        indices[dst + 4] = base + 2;
        indices[dst + 5] = base + 3;
    }
}

fn run_depth_sort_worker(
    request_rx: FromUIReceiver<SplatSortRequest>,
    result_tx: ToUISender<SplatSortResult>,
) {
    let mut scene_generation = 0u64;
    let mut centers_local = Vec::<[f32; 3]>::new();
    let mut keys = Vec::<u32>::new();
    let mut order_a = Vec::<u32>::new();
    let mut order_b = Vec::<u32>::new();
    let mut sorted_indices = Vec::<u32>::new();

    while let Ok(request) = request_rx.recv() {
        match request {
            SplatSortRequest::SetScene {
                generation,
                centers_local: centers,
            } => {
                scene_generation = generation;
                centers_local = centers;
                keys.clear();
                order_a.clear();
                order_b.clear();
                sorted_indices.clear();
            }
            SplatSortRequest::Sort {
                generation,
                request_id,
                view_matrix,
                model_matrix,
                camera_pos,
                sort_radial,
            } => {
                if generation == scene_generation && !centers_local.is_empty() {
                    sort_splats_radix(
                        view_matrix,
                        model_matrix,
                        camera_pos,
                        sort_radial,
                        &centers_local,
                        &mut keys,
                        &mut order_a,
                        &mut order_b,
                    );
                    build_sorted_triangle_indices(&order_a, &mut sorted_indices);
                    let _ = result_tx.send(SplatSortResult {
                        generation,
                        request_id,
                        indices: mem::take(&mut sorted_indices),
                    });
                } else {
                    let _ = result_tx.send(SplatSortResult {
                        generation,
                        request_id,
                        indices: Vec::new(),
                    });
                }
            }
        }
    }
}

impl ViewSplat {
    fn resource_metadata_by_handle(cx: &mut Cx, handle: ScriptHandle) -> Option<(PathBuf, bool)> {
        let resources = cx.script_data.resources.resources.borrow();
        let resource = resources
            .iter()
            .find(|resource| resource.handle == handle)?;
        Some((PathBuf::from(&resource.abs_path), resource.is_error()))
    }

    fn resolve_resource(cx: &mut Cx, handle_ref: &ScriptHandleRef) -> ResourceResolve {
        let handle = handle_ref.as_handle();

        if let Some(data) = cx.get_resource(handle) {
            let abs_path = Self::resource_metadata_by_handle(cx, handle)
                .map(|metadata| metadata.0)
                .unwrap_or_else(|| PathBuf::from("resource"));
            return ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            };
        }

        cx.load_all_script_resources();

        if let Some(data) = cx.get_resource(handle) {
            let abs_path = Self::resource_metadata_by_handle(cx, handle)
                .map(|metadata| metadata.0)
                .unwrap_or_else(|| PathBuf::from("resource"));
            return ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            };
        }

        if let Some((_, is_error)) = Self::resource_metadata_by_handle(cx, handle) {
            if is_error {
                return ResourceResolve::Error { handle };
            }
            return ResourceResolve::Pending { handle };
        }

        ResourceResolve::Missing
    }

    fn update_scene_fit(&mut self) {
        let Some(scene) = self.scene.as_ref() else {
            self.scene_center = vec3(0.0, 0.0, 0.0);
            self.scene_unit_scale = 1.0;
            return;
        };

        let min_v = scene.bounds_min;
        let max_v = scene.bounds_max;

        self.scene_center = vec3(
            (min_v[0] + max_v[0]) * 0.5,
            (min_v[1] + max_v[1]) * 0.5,
            (min_v[2] + max_v[2]) * 0.5,
        );

        let extent_x = max_v[0] - min_v[0];
        let extent_y = max_v[1] - min_v[1];
        let extent_z = max_v[2] - min_v[2];
        let max_extent = extent_x.max(extent_y).max(extent_z).max(1e-6);
        self.scene_unit_scale = 1.0 / max_extent;
    }

    fn next_sort_generation(&mut self) {
        self.depth_sort_generation = self.depth_sort_generation.wrapping_add(1);
        if self.depth_sort_generation == 0 {
            self.depth_sort_generation = 1;
        }
    }

    fn reset_depth_sort_state_for_new_scene(&mut self) {
        self.splat_mesh = None;
        self.base_indices.clear();
        self.base_indices_applied = true;
        self.depth_sort_centers_local.clear();
        self.depth_sort_pending_result = None;
        self.depth_sort_in_flight = false;
        self.depth_sort_scene_uploaded_generation = 0;
        self.depth_sort_next_request_id = 0;
        self.depth_sort_last_applied_request_id = 0;
        self.depth_sort_last_camera_pos = None;
        self.depth_sort_last_camera_forward = None;
        self.depth_sort_last_depth_plane = None;
        self.next_sort_generation();
    }

    fn ensure_depth_sort_thread(&mut self, cx: &mut Cx2d) {
        if self.depth_sort_thread_started {
            return;
        }
        self.depth_sort_request_tx.new_channel();
        let request_rx = self.depth_sort_request_tx.receiver();
        let result_tx = self.depth_sort_result_rx.sender();
        cx.spawn_thread(move || run_depth_sort_worker(request_rx, result_tx));
        self.depth_sort_thread_started = true;
    }

    fn upload_sort_scene_to_worker(&mut self, cx: &mut Cx2d) -> bool {
        if self.depth_sort_centers_local.is_empty() {
            return false;
        }
        self.ensure_depth_sort_thread(cx);
        let generation = self.depth_sort_generation;
        let request = SplatSortRequest::SetScene {
            generation,
            centers_local: self.depth_sort_centers_local.clone(),
        };
        if self.depth_sort_request_tx.send(request).is_ok() {
            self.depth_sort_scene_uploaded_generation = generation;
            true
        } else {
            self.depth_sort_thread_started = false;
            self.depth_sort_scene_uploaded_generation = 0;
            false
        }
    }

    fn camera_forward_from_view(view: &Mat4f) -> Vec3f {
        vec3(-view.v[8], -view.v[9], -view.v[10]).normalize()
    }

    fn depth_plane_from_view_and_model(view_matrix: &Mat4f, model_matrix: &Mat4f) -> Vec4f {
        let depth_origin = local_depth_from_view_model(view_matrix, model_matrix, [0.0, 0.0, 0.0]);
        let depth_x = local_depth_from_view_model(view_matrix, model_matrix, [1.0, 0.0, 0.0]);
        let depth_y = local_depth_from_view_model(view_matrix, model_matrix, [0.0, 1.0, 0.0]);
        let depth_z = local_depth_from_view_model(view_matrix, model_matrix, [0.0, 0.0, 1.0]);
        vec4(
            depth_x - depth_origin,
            depth_y - depth_origin,
            depth_z - depth_origin,
            depth_origin,
        )
    }

    fn depth_plane_delta(a: Vec4f, b: Vec4f) -> f32 {
        (a.x - b.x)
            .abs()
            .max((a.y - b.y).abs())
            .max((a.z - b.z).abs())
            .max((a.w - b.w).abs())
    }

    fn should_request_depth_sort(
        &self,
        scene_state: &SceneState3D,
        view_matrix: &Mat4f,
        model_matrix: &Mat4f,
    ) -> bool {
        let Some(last_pos) = self.depth_sort_last_camera_pos else {
            return true;
        };
        let Some(last_forward) = self.depth_sort_last_camera_forward else {
            return true;
        };

        let depth_plane = Self::depth_plane_from_view_and_model(view_matrix, model_matrix);
        let depth_plane_changed = self
            .depth_sort_last_depth_plane
            .map(|last| Self::depth_plane_delta(last, depth_plane) >= 0.0005)
            .unwrap_or(true);
        if depth_plane_changed {
            return true;
        }

        let min_move = self.sort_min_camera_move.max(0.0);
        let moved = (scene_state.camera_pos - last_pos).length() >= min_move;

        let min_angle_rad = self.sort_min_camera_angle_deg.max(0.0).to_radians();
        let cos_threshold = min_angle_rad.cos();
        let forward = Self::camera_forward_from_view(&scene_state.view);
        let dot = forward.dot(last_forward).clamp(-1.0, 1.0);
        let rotated = dot <= cos_threshold;

        moved || rotated
    }

    fn request_depth_sort_if_needed(
        &mut self,
        cx: &mut Cx2d,
        scene_state: &SceneState3D,
        view_matrix: Mat4f,
        model_matrix: Mat4f,
    ) {
        if !self.sort_back_to_front {
            return;
        }
        if self.depth_sort_centers_local.is_empty() {
            return;
        }
        if self.depth_sort_scene_uploaded_generation != self.depth_sort_generation
            && !self.upload_sort_scene_to_worker(cx)
        {
            return;
        }
        if self.depth_sort_in_flight
            || !self.should_request_depth_sort(scene_state, &view_matrix, &model_matrix)
        {
            return;
        }

        self.depth_sort_next_request_id = self.depth_sort_next_request_id.wrapping_add(1);
        if self.depth_sort_next_request_id == 0 {
            self.depth_sort_next_request_id = 1;
        }
        let sort_radial = self
            .scene
            .as_ref()
            .map(|scene| scene.format == SplatFileFormat::Ply)
            .unwrap_or(false);
        let request_id = self.depth_sort_next_request_id;
        let request = SplatSortRequest::Sort {
            generation: self.depth_sort_generation,
            request_id,
            view_matrix,
            model_matrix,
            camera_pos: scene_state.camera_pos,
            sort_radial,
        };
        if self.depth_sort_request_tx.send(request).is_ok() {
            self.depth_sort_in_flight = true;
            self.depth_sort_last_camera_pos = Some(scene_state.camera_pos);
            self.depth_sort_last_camera_forward =
                Some(Self::camera_forward_from_view(&scene_state.view));
            self.depth_sort_last_depth_plane = Some(Self::depth_plane_from_view_and_model(
                &view_matrix,
                &model_matrix,
            ));
        } else {
            self.depth_sort_thread_started = false;
            self.depth_sort_scene_uploaded_generation = 0;
            self.depth_sort_in_flight = false;
        }
    }

    fn poll_depth_sort_results(&mut self) -> bool {
        match self.depth_sort_result_rx.try_recv_flush() {
            Ok(result) => {
                if result.generation != self.depth_sort_generation {
                    return false;
                }
                self.depth_sort_in_flight = false;
                if result.request_id <= self.depth_sort_last_applied_request_id {
                    return false;
                }
                self.depth_sort_pending_result = Some(result);
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.depth_sort_thread_started = false;
                self.depth_sort_scene_uploaded_generation = 0;
                self.depth_sort_in_flight = false;
                false
            }
        }
    }

    fn apply_pending_depth_sort(&mut self, cx: &mut Cx2d, mesh: PbrMeshHandle) {
        let Some(result) = self.depth_sort_pending_result.take() else {
            return;
        };
        if result.generation != self.depth_sort_generation {
            return;
        }
        if result.request_id <= self.depth_sort_last_applied_request_id {
            return;
        }
        if result.indices.is_empty() {
            self.depth_sort_last_applied_request_id = result.request_id;
            return;
        }
        match self
            .draw_splat
            .draw_super
            .update_mesh_indices(cx, mesh, result.indices)
        {
            Ok(()) => {
                self.depth_sort_last_applied_request_id = result.request_id;
                self.base_indices_applied = false;
            }
            Err(error) => {
                log!("ViewSplat depth-sort index update error: {}", error);
            }
        }
    }

    fn restore_base_indices_if_needed(&mut self, cx: &mut Cx2d, mesh: PbrMeshHandle) {
        if self.base_indices_applied || self.base_indices.is_empty() {
            return;
        }
        match self
            .draw_splat
            .draw_super
            .update_mesh_indices(cx, mesh, self.base_indices.clone())
        {
            Ok(()) => {
                self.base_indices_applied = true;
            }
            Err(error) => {
                log!("ViewSplat base-index restore error: {}", error);
            }
        }
    }

    fn ensure_env_loaded(&mut self, cx: &mut Cx2d) {
        let Some(handle_ref) = self.env_src.as_ref() else {
            return;
        };
        let handle = handle_ref.as_handle();
        if self.loaded_env_handle == Some(handle) {
            return;
        }

        match Self::resolve_resource(cx, handle_ref) {
            ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            } => {
                let _ = self
                    .draw_splat
                    .draw_super
                    .load_default_env_equirect_from_bytes(cx, &data, Some(&abs_path));
                self.loaded_env_handle = Some(handle);
            }
            ResourceResolve::Error { handle } => {
                self.loaded_env_handle = Some(handle);
            }
            ResourceResolve::Pending { handle } => {
                let _ = handle;
            }
            ResourceResolve::Missing => {}
        }
    }

    fn ensure_scene_loaded(&mut self, cx: &mut Cx2d) {
        let Some(handle_ref) = self.src.as_ref() else {
            return;
        };

        let handle = handle_ref.as_handle();
        if self.loaded_src_handle == Some(handle) {
            return;
        }

        match Self::resolve_resource(cx, handle_ref) {
            ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            } => {
                match load_splat_from_bytes(&data, Some(abs_path.as_path())) {
                    Ok(scene) => {
                        if self.auto_antialias_blur {
                            self.draw_splat.blur_pixels = if scene.antialias { 0.3 } else { 0.0 };
                        }
                        self.scene = Some(scene);
                        self.update_scene_fit();
                        self.reset_depth_sort_state_for_new_scene();
                    }
                    Err(error) => {
                        log!("ViewSplat parse error ({}): {}", abs_path.display(), error);
                        self.scene = None;
                        self.reset_depth_sort_state_for_new_scene();
                    }
                }
                self.loaded_src_handle = Some(handle);
            }
            ResourceResolve::Error { handle } => {
                self.scene = None;
                self.reset_depth_sort_state_for_new_scene();
                self.loaded_src_handle = Some(handle);
            }
            ResourceResolve::Pending { handle } => {
                let _ = handle;
            }
            ResourceResolve::Missing => {}
        }
    }

    fn ensure_splat_mesh(&mut self, cx: &mut Cx2d) -> Option<PbrMeshHandle> {
        if let Some(mesh) = self.splat_mesh {
            return Some(mesh);
        }

        let Some(scene) = self.scene.as_ref() else {
            return None;
        };
        if scene.splats.is_empty() {
            return None;
        }

        let max_splats = if self.max_splats == 0 {
            scene.splats.len()
        } else {
            scene.splats.len().min(self.max_splats as usize)
        };

        let normalize_scale = if self.auto_normalize {
            self.scene_unit_scale * self.normalize_fit.max(0.0001)
        } else {
            1.0
        };
        let center = if self.auto_normalize {
            self.scene_center
        } else {
            vec3(0.0, 0.0, 0.0)
        };

        let min_radius = self.min_radius.max(0.0);
        let radius_scale = self.radius_scale.max(0.0);
        let opacity_scale = self.opacity_scale.max(0.0);

        let estimated = max_splats.min(scene.splats.len());
        let mut positions: Vec<[f32; 3]> = Vec::with_capacity(estimated * 4); // packed center_local xyz
        let mut normals: Vec<[f32; 3]> = Vec::with_capacity(estimated * 4); // packed local axis_0 xyz
        let mut tangents: Vec<[f32; 4]> = Vec::with_capacity(estimated * 4); // packed local axis_1 xyz + axis_2_len
        let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(estimated * 4);
        let mut colors: Vec<[f32; 4]> = Vec::with_capacity(estimated * 4);
        let mut indices: Vec<u32> = Vec::with_capacity(estimated * 6);
        let mut centers_local: Vec<[f32; 3]> = Vec::with_capacity(estimated);

        let quad_coords = [
            [-1.0_f32, -1.0_f32],
            [1.0_f32, -1.0_f32],
            [1.0_f32, 1.0_f32],
            [-1.0_f32, 1.0_f32],
        ];
        let quat_rotate = |q: [f32; 4], v: [f32; 3]| -> [f32; 3] {
            let qx = q[0];
            let qy = q[1];
            let qz = q[2];
            let qw = q[3];
            let tx = 2.0 * (qy * v[2] - qz * v[1]);
            let ty = 2.0 * (qz * v[0] - qx * v[2]);
            let tz = 2.0 * (qx * v[1] - qy * v[0]);
            let cx = qy * tz - qz * ty;
            let cy = qz * tx - qx * tz;
            let cz = qx * ty - qy * tx;
            [
                v[0] + qw * tx + cx,
                v[1] + qw * ty + cy,
                v[2] + qw * tz + cz,
            ]
        };

        for splat in scene.splats.iter().take(max_splats) {
            let alpha = (splat.color[3] * opacity_scale).clamp(0.0, 1.0);
            if alpha <= 0.001 {
                continue;
            }

            let center_local = [
                (splat.position[0] - center.x) * normalize_scale,
                (splat.position[1] - center.y) * normalize_scale,
                (splat.position[2] - center.z) * normalize_scale,
            ];

            let sx = (splat.scale[0].abs() * normalize_scale).max(min_radius) * radius_scale;
            let sy = (splat.scale[1].abs() * normalize_scale).max(min_radius) * radius_scale;
            let sz = (splat.scale[2].abs() * normalize_scale).max(min_radius) * radius_scale;
            if sx <= 0.0 && sy <= 0.0 && sz <= 0.0 {
                continue;
            }
            let q = {
                let x = splat.rotation[0];
                let y = splat.rotation[1];
                let z = splat.rotation[2];
                let w = splat.rotation[3];
                let len2 = x * x + y * y + z * z + w * w;
                if len2 <= f32::EPSILON {
                    [0.0, 0.0, 0.0, 1.0]
                } else {
                    let inv_len = len2.sqrt().recip();
                    [x * inv_len, y * inv_len, z * inv_len, w * inv_len]
                }
            };
            let axis_local_0 = quat_rotate(q, [sx, 0.0, 0.0]);
            let axis_local_1 = quat_rotate(q, [0.0, sy, 0.0]);

            let splat_index = centers_local.len() as u32;
            centers_local.push(center_local);
            let base = splat_index * 4;
            for i in 0..4 {
                positions.push(center_local);
                normals.push(axis_local_0);
                tangents.push([axis_local_1[0], axis_local_1[1], axis_local_1[2], sz]);
                uvs.push(quad_coords[i]);
                colors.push([
                    splat.color[0].clamp(0.0, 1.0),
                    splat.color[1].clamp(0.0, 1.0),
                    splat.color[2].clamp(0.0, 1.0),
                    alpha,
                ]);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }

        if positions.is_empty() {
            return None;
        }

        match self.draw_splat.draw_super.upload_indexed_triangles_mesh(
            cx,
            &positions,
            Some(&normals),
            Some(&tangents),
            Some(&uvs),
            Some(&colors),
            &indices,
        ) {
            Ok(mesh) => {
                self.splat_mesh = Some(mesh);
                self.base_indices = indices;
                self.base_indices_applied = true;
                self.depth_sort_centers_local = centers_local;
                self.depth_sort_pending_result = None;
                self.depth_sort_in_flight = false;
                self.depth_sort_scene_uploaded_generation = 0;
                self.depth_sort_next_request_id = 0;
                self.depth_sort_last_applied_request_id = 0;
                self.depth_sort_last_camera_pos = None;
                self.depth_sort_last_camera_forward = None;
                self.depth_sort_last_depth_plane = None;
                self.next_sort_generation();
                Some(mesh)
            }
            Err(error) => {
                log!("ViewSplat splat mesh upload error: {}", error);
                None
            }
        }
    }

    fn node_matrix(&self) -> Mat4f {
        Mat4f::mul(
            &Mat4f::translation(self.position),
            &Mat4f::mul(
                &Mat4f::rotation(self.rotation),
                &Mat4f::nonuniform_scaled_translation(self.scale, vec3(0.0, 0.0, 0.0)),
            ),
        )
    }
}

impl Widget for ViewSplat {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::Signal = event {
            if self.poll_depth_sort_results() {
                self.draw_splat.redraw(cx);
            }
        }
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        let Some(scene_state) = scene_state_from_scope(scope) else {
            return DrawStep::done();
        };
        let cx = &mut Cx2d::new(cx.cx);

        self.ensure_env_loaded(cx);
        self.ensure_scene_loaded(cx);
        let Some(splat_mesh) = self.ensure_splat_mesh(cx) else {
            return DrawStep::done();
        };

        apply_scene_to_draw_pbr(&mut self.draw_splat.draw_super, cx, &scene_state);

        let node_matrix = self.node_matrix();
        let render_w = scene_state.viewport_rect.size.x.max(1.0) as f32;
        let render_h = scene_state.viewport_rect.size.y.max(1.0) as f32;
        self.draw_splat.render_size = vec2(render_w, render_h);
        let proj = self.draw_splat.draw_super.projection_matrix;
        self.draw_splat.focal_pixels = vec2(
            proj.v[0].abs().max(0.00001) * render_w * 0.5,
            proj.v[5].abs().max(0.00001) * render_h * 0.5,
        );
        self.draw_splat.ndc_per_pixel = vec2(2.0 / render_w.max(1.0), 2.0 / render_h.max(1.0));
        self.draw_splat.draw_super.set_depth_write(false);
        self.draw_splat.draw_super.model_matrix = node_matrix;
        let _ = self.poll_depth_sort_results();
        if self.sort_back_to_front {
            self.apply_pending_depth_sort(cx, splat_mesh);
            self.request_depth_sort_if_needed(cx, &scene_state, scene_state.view, node_matrix);
        } else {
            self.depth_sort_pending_result = None;
            self.depth_sort_in_flight = false;
            self.depth_sort_last_camera_pos = None;
            self.depth_sort_last_camera_forward = None;
            self.depth_sort_last_depth_plane = None;
            self.restore_base_indices_if_needed(cx, splat_mesh);
        }

        let draw_result = self.draw_splat.draw_super.draw_mesh(cx, splat_mesh);
        if draw_result.is_ok() {
            let world = node_matrix.transform_vec4(vec4(0.0, 0.0, 0.0, 1.0));
            register_last_draw_call_anchor(cx, scope, vec3(world.x, world.y, world.z));
        }

        DrawStep::done()
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}
