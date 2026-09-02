//! Gaussian-splat scene renderer (.ply / .sog) for the XR scene graph.
//!
//! One instanced draw of ONE quad: the instance stream is the list of
//! visible splat ids, far to near, produced asynchronously by the CPU sorter
//! (`splat_sort`), and the per-splat data lives in two textures read in the
//! vertex stage (`splat_pack`: 16 bytes per splat + 32 bytes per 256-splat
//! chunk). The vertex shader decodes the record, projects the 3D covariance
//! to a screen-space ellipse and emits the oriented quad; the fragment
//! shader evaluates the Gaussian and blends premultiplied, depth-tested
//! against the scene z-buffer with depth writes off.
//!
//! Between sorts the previous order keeps rendering; a new sort is asked
//! for only when the camera moved/rotated past `sort_min_camera_*`, at most
//! one is in flight and the newest camera wins.

use crate::obj::splat_pack::{
    morton_sorted, PackedScene, SplatRecord, CHUNKS_PER_ROW, RECORDS_PER_ROW,
};
use crate::obj::splat_sort::{
    run_sort_worker, SortCamera, SortRequest, SortResult, SortScene, SortStats,
};
use makepad_splat::{load_splat_from_bytes, SplatFileFormat};
use makepad_widgets::{makepad_derive_widget::*, makepad_draw::*, widget::*};
use std::{mem, path::PathBuf, rc::Rc, sync::mpsc::TryRecvError};

use crate::util::clock::Instant;
use crate::util::scene_draw::{
    compose_scene_node_transform, scene_node_world_transform_from_cx, scene_state_from_cx,
    SceneState3D,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawSplat = set_type_default() do #(DrawSplat::script_shader(vm)){
        alpha_blend: true
        depth_write: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)
        // 4 BGRAu8 texels per splat (splat_pack record), 8192 texels wide.
        splat_data: texture_2d(float)
        // 2 RGBA32F texels per 256-splat chunk: min xyz, extent xyz.
        chunk_bounds: texture_2d(float)

        // Node matrix as rows (xyzw dot products), set per draw.
        mm_r0: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        mm_r1: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        mm_r2: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        // x,y: 1/data texture size; z,w: 1/chunk texture size.
        u_layout: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        // x: ln(min scale), y: ln range / 255.
        u_scale: uniform(vec4(-7.0, 0.0, 0.0, 0.0))

        render_size: uniform(vec2(1024.0, 768.0))
        focal_pixels: uniform(vec2(512.0, 384.0))
        ndc_per_pixel: uniform(vec2(0.001953125, 0.0026041667))
        coarse_cull_guard: uniform(2.0)
        fast_project_mode: uniform(0.0)
        splat_std_dev: uniform(2.8)
        min_pixel_radius: uniform(0.0)
        max_pixel_radius: uniform(512.0)
        blur_pixels: uniform(0.3)
        alpha_cutoff: uniform(0.002)
        dither_depth_cutout: uniform(0.0)
        dither_scale: uniform(1.0)

        v_uv: varying(vec2f)
        v_color: varying(vec4f)
        v_ndc: varying(vec2f)

        // One BGRAu8 texel back to its little-endian u32 word. Channels are
        // exact multiples of 1/255, so the rounding here is bit-exact and
        // no packed bits ever pass through a float (denormal-safe).
        word_of: fn(c: vec4) -> u32 {
            let b0 = uint(floor(c.z * 255.0 + 0.5))
            let b1 = uint(floor(c.y * 255.0 + 0.5))
            let b2 = uint(floor(c.x * 255.0 + 0.5))
            let b3 = uint(floor(c.w * 255.0 + 0.5))
            return b0 | (b1 << u32(8)) | (b2 << u32(16)) | (b3 << u32(24))
        }

        // Every vertex of a culled splat goes to the same clip point.
        cull: fn() {
            self.v_uv = vec2(0.0, 0.0)
            self.v_ndc = vec2(4.0, 4.0)
            self.v_color = vec4(0.0, 0.0, 0.0, 0.0)
            self.vertex_pos = vec4(2.0, 2.0, 2.0, 1.0)
        }

        vertex: fn() {
            let quad = self.geom.pos * 2.0 - vec2(1.0, 1.0)
            let id = self.splat_id
            let row = floor(id * 0.00048828125)
            let col = id - row * 2048.0
            let v = (row + 0.5) * self.u_layout.y
            let x0 = col * 4.0 + 0.5
            let c0 = self.splat_data.sample_nearest(vec2(x0 * self.u_layout.x, v), 0.0)
            let c1 = self.splat_data.sample_nearest(vec2((x0 + 1.0) * self.u_layout.x, v), 0.0)
            let c2 = self.splat_data.sample_nearest(vec2((x0 + 2.0) * self.u_layout.x, v), 0.0)
            let c3 = self.splat_data.sample_nearest(vec2((x0 + 3.0) * self.u_layout.x, v), 0.0)
            let w1 = self.word_of(c1)
            let w2 = self.word_of(c2)
            let w3 = self.word_of(c3)
            let px = float(w1 & u32(16383))
            let py = float((w1 >> u32(14)) & u32(16383))
            let pz = float(((w1 >> u32(28)) & u32(15)) | ((w2 & u32(1023)) << u32(4)))
            let sx = float((w2 >> u32(10)) & u32(255))
            let sy = float((w2 >> u32(18)) & u32(255))
            let sz = float(((w2 >> u32(26)) & u32(63)) | ((w3 & u32(3)) << u32(6)))
            let q0 = float((w3 >> u32(2)) & u32(511))
            let q1 = float((w3 >> u32(11)) & u32(511))
            let q2 = float((w3 >> u32(20)) & u32(511))
            let qi = float((w3 >> u32(29)) & u32(3))

            let chunk = floor(id * 0.00390625)
            let crow = floor(chunk * 0.0009765625)
            let ccol = chunk - crow * 1024.0
            let cv = (crow + 0.5) * self.u_layout.w
            let cmin = self.chunk_bounds.sample_nearest(vec2((ccol * 2.0 + 0.5) * self.u_layout.z, cv), 0.0)
            let cext = self.chunk_bounds.sample_nearest(vec2((ccol * 2.0 + 1.5) * self.u_layout.z, cv), 0.0)
            let center_local = cmin.xyz + vec3(px, py, pz) * (cext.xyz * 0.00006103888)

            let scale_0 = exp(self.u_scale.x + sx * self.u_scale.y)
            let scale_1 = exp(self.u_scale.x + sy * self.u_scale.y)
            let axis_2_len = max(exp(self.u_scale.x + sz * self.u_scale.y), 0.000001)

            let qa = (q0 * 0.0019569472 - 0.5) * 1.41421356
            let qb = (q1 * 0.0019569472 - 0.5) * 1.41421356
            let qc = (q2 * 0.0019569472 - 0.5) * 1.41421356
            let qbig = sqrt(max(0.0, 1.0 - qa * qa - qb * qb - qc * qc))
            let mut q = vec4(qa, qb, qc, qbig)
            if qi < 0.5 {
                q = vec4(qbig, qa, qb, qc)
            } else if qi < 1.5 {
                q = vec4(qa, qbig, qb, qc)
            } else if qi < 2.5 {
                q = vec4(qa, qb, qbig, qc)
            }
            // Rotation matrix columns of q = (x, y, z, w).
            let xx = q.x * q.x
            let yy = q.y * q.y
            let zz = q.z * q.z
            let xy = q.x * q.y
            let xz = q.x * q.z
            let yz = q.y * q.z
            let xw = q.x * q.w
            let yw = q.y * q.w
            let zw = q.z * q.w
            let axis_local_0 = vec3(1.0 - 2.0 * (yy + zz), 2.0 * (xy + zw), 2.0 * (xz - yw)) * scale_0
            let axis_local_1 = vec3(2.0 * (xy - zw), 1.0 - 2.0 * (xx + zz), 2.0 * (yz + xw)) * scale_1
            let axis_local_2_dir = vec3(2.0 * (xz + yw), 2.0 * (yz - xw), 1.0 - 2.0 * (xx + yy))

            let center_local4 = vec4(center_local.x, center_local.y, center_local.z, 1.0)
            let center_world4 = vec4(
                dot(self.mm_r0, center_local4),
                dot(self.mm_r1, center_local4),
                dot(self.mm_r2, center_local4),
                1.0
            )
            let center_view4 = self.draw_pass.camera_view * center_world4
            let center_view = center_view4.xyz
            if center_view.z >= -0.000001 {
                self.cull()
                return
            }

            let clip_center = self.draw_pass.camera_projection * center_view4
            let center_inv_w = 1.0 / max(abs(clip_center.w), 0.000001)
            let center_ndc = clip_center.xy * center_inv_w

            let focal = vec2(max(self.focal_pixels.x, 0.00001), max(self.focal_pixels.y, 0.00001))
            let inv_depth = 1.0 / max(-center_view.z, 0.000001)
            // The projected ellipse's semi-axes (in std devs) never exceed the
            // longest 3D axis; the decoded lengths are exact, no component bound.
            let max_scale = max(scale_0, max(scale_1, axis_2_len))
            let cull_guard = max(self.coarse_cull_guard, 0.0)
            let ndc_guard = 1.0 + cull_guard * max(abs(center_ndc.x), abs(center_ndc.y))
            let rough_radius_px = self.splat_std_dev * max_scale * max(focal.x, focal.y) * inv_depth * ndc_guard
            if rough_radius_px < self.min_pixel_radius {
                self.cull()
                return
            }

            let ndc_per_pixel = vec2(max(self.ndc_per_pixel.x, 0.000001), max(self.ndc_per_pixel.y, 0.000001))
            let rough_ndc = vec2(rough_radius_px * ndc_per_pixel.x, rough_radius_px * ndc_per_pixel.y)
            if center_ndc.x < (-1.0 - rough_ndc.x)
                || center_ndc.x > (1.0 + rough_ndc.x)
                || center_ndc.y < (-1.0 - rough_ndc.y)
                || center_ndc.y > (1.0 + rough_ndc.y)
            {
                self.cull()
                return
            }

            let a0 = vec4(axis_local_0.x, axis_local_0.y, axis_local_0.z, 0.0)
            let a1 = vec4(axis_local_1.x, axis_local_1.y, axis_local_1.z, 0.0)
            let axis_world_0 = vec3(dot(self.mm_r0, a0), dot(self.mm_r1, a0), dot(self.mm_r2, a0))
            let axis_world_1 = vec3(dot(self.mm_r0, a1), dot(self.mm_r1, a1), dot(self.mm_r2, a1))
            let axis_view_0 = (self.draw_pass.camera_view * vec4(axis_world_0.x, axis_world_0.y, axis_world_0.z, 0.0)).xyz
            let axis_view_1 = (self.draw_pass.camera_view * vec4(axis_world_1.x, axis_world_1.y, axis_world_1.z, 0.0)).xyz

            let max_radius = if self.max_pixel_radius > 0.0 { self.max_pixel_radius } else { 1000000.0 }
            if self.fast_project_mode > 0.5 {
                let clip_axis_0 = self.draw_pass.camera_projection
                    * (center_view4 + vec4(axis_view_0.x, axis_view_0.y, axis_view_0.z, 0.0))
                let clip_axis_1 = self.draw_pass.camera_projection
                    * (center_view4 + vec4(axis_view_1.x, axis_view_1.y, axis_view_1.z, 0.0))

                let inv_w0 = 1.0 / max(abs(clip_axis_0.w), 0.000001)
                let inv_w1 = 1.0 / max(abs(clip_axis_1.w), 0.000001)
                let ndc_axis_0 = clip_axis_0.xy * inv_w0 - center_ndc
                let ndc_axis_1 = clip_axis_1.xy * inv_w1 - center_ndc

                let mut px_axis_0 = vec2(
                    (ndc_axis_0.x / ndc_per_pixel.x) * self.splat_std_dev,
                    (ndc_axis_0.y / ndc_per_pixel.y) * self.splat_std_dev,
                )
                let mut px_axis_1 = vec2(
                    (ndc_axis_1.x / ndc_per_pixel.x) * self.splat_std_dev,
                    (ndc_axis_1.y / ndc_per_pixel.y) * self.splat_std_dev,
                )

                let radius_0 = length(px_axis_0)
                let radius_1 = length(px_axis_1)
                if radius_0 < self.min_pixel_radius && radius_1 < self.min_pixel_radius {
                    self.cull()
                    return
                }
                if radius_0 > max_radius {
                    px_axis_0 = px_axis_0 * (max_radius / max(radius_0, 0.000001))
                }
                if radius_1 > max_radius {
                    px_axis_1 = px_axis_1 * (max_radius / max(radius_1, 0.000001))
                }

                let px_offset = px_axis_0 * quad.x + px_axis_1 * quad.y
                let ndc = center_ndc + vec2(px_offset.x * ndc_per_pixel.x, px_offset.y * ndc_per_pixel.y)
                self.v_uv = quad * self.splat_std_dev
                self.v_ndc = ndc
                self.v_color = c0
                self.vertex_pos = vec4(ndc.x * clip_center.w, ndc.y * clip_center.w, clip_center.z, clip_center.w)
                return
            }

            let axis_local_2 = axis_local_2_dir * axis_2_len
            let a2 = vec4(axis_local_2.x, axis_local_2.y, axis_local_2.z, 0.0)
            let axis_world_2 = vec3(dot(self.mm_r0, a2), dot(self.mm_r1, a2), dot(self.mm_r2, a2))
            let axis_view_2 = (self.draw_pass.camera_view * vec4(axis_world_2.x, axis_world_2.y, axis_world_2.z, 0.0)).xyz

            // 3D covariance in view space = sum of outer products of the axes.
            let c00 = axis_view_0.x * axis_view_0.x + axis_view_1.x * axis_view_1.x + axis_view_2.x * axis_view_2.x
            let c01 = axis_view_0.x * axis_view_0.y + axis_view_1.x * axis_view_1.y + axis_view_2.x * axis_view_2.y
            let c02 = axis_view_0.x * axis_view_0.z + axis_view_1.x * axis_view_1.z + axis_view_2.x * axis_view_2.z
            let c11 = axis_view_0.y * axis_view_0.y + axis_view_1.y * axis_view_1.y + axis_view_2.y * axis_view_2.y
            let c12 = axis_view_0.y * axis_view_0.z + axis_view_1.y * axis_view_1.z + axis_view_2.y * axis_view_2.z
            let c22 = axis_view_0.z * axis_view_0.z + axis_view_1.z * axis_view_1.z + axis_view_2.z * axis_view_2.z

            // Project with the perspective Jacobian to a 2D covariance (pixels).
            let inv_z = 1.0 / center_view.z
            let jx = focal.x * inv_z
            let jy = focal.y * inv_z
            let jzx = -(jx * center_view.x) * inv_z
            let jzy = -(jy * center_view.y) * inv_z

            let mut a = jx * jx * c00 + 2.0 * jx * jzx * c02 + jzx * jzx * c22
            let mut b = jx * jy * c01 + jx * jzy * c02 + jzx * jy * c12 + jzx * jzy * c22
            let mut d = jy * jy * c11 + 2.0 * jy * jzy * c12 + jzy * jzy * c22

            // Anti-alias dilation (3DGS): add blur, keep the integral.
            let det_orig = a * d - b * b
            a = a + self.blur_pixels
            d = d + self.blur_pixels
            let det = a * d - b * b
            let blur_adjust = if det_orig > 0.0 && det > 0.0 {
                sqrt(max(det_orig / det, 0.0))
            } else {
                1.0
            }

            // Eigen-decompose: the quad axes are the ellipse axes at splat_std_dev sigma.
            let eigen_avg = 0.5 * (a + d)
            let eigen_delta = sqrt(max(0.0, eigen_avg * eigen_avg - det))
            let eigen_0 = max(eigen_avg + eigen_delta, 0.000001)
            let eigen_1 = max(eigen_avg - eigen_delta, 0.000001)
            let axis_0 = if abs(b) < 0.001 {
                vec2(1.0, 0.0)
            } else {
                normalize(vec2(b, eigen_0 - a))
            }
            let axis_1 = vec2(axis_0.y, -axis_0.x)

            let scale_px_0 = min(max_radius, self.splat_std_dev * sqrt(eigen_0))
            let scale_px_1 = min(max_radius, self.splat_std_dev * sqrt(eigen_1))
            if scale_px_0 < self.min_pixel_radius && scale_px_1 < self.min_pixel_radius {
                self.cull()
                return
            }
            let pixel_offset = axis_0 * (quad.x * scale_px_0) + axis_1 * (quad.y * scale_px_1)
            let ndc_offset = vec2(pixel_offset.x * ndc_per_pixel.x, pixel_offset.y * ndc_per_pixel.y)
            let ndc = center_ndc + ndc_offset

            self.v_uv = quad * self.splat_std_dev
            self.v_ndc = ndc
            self.v_color = vec4(c0.x, c0.y, c0.z, c0.w * blur_adjust)
            self.vertex_pos = vec4(ndc.x * clip_center.w, ndc.y * clip_center.w, clip_center.z, clip_center.w)
        }

        pixel: fn() {
            let r2 = dot(self.v_uv, self.v_uv)
            let max_r2 = self.splat_std_dev * self.splat_std_dev
            if r2 > max_r2 {
                discard()
            }

            let alpha = exp(-0.5 * r2) * self.v_color.w
            if alpha < self.alpha_cutoff {
                discard()
            }

            let rgb = self.v_color.xyz
            if self.dither_depth_cutout > 0.5 {
                // Pixel-space stochastic transparency: alpha-to-coverage style
                // cutout with depth writes enabled.
                let ndc = self.v_ndc
                let ndc_dx = vec2(dFdx(ndc.x), dFdy(ndc.x))
                let ndc_dy = vec2(dFdx(ndc.y), dFdy(ndc.y))
                let px_ndc = vec2(max(length(ndc_dx), 0.000001), max(length(ndc_dy), 0.000001))
                let frag_px = ndc / px_ndc
                let cell = floor(frag_px * self.dither_scale + vec2(0.5, 0.5))
                let threshold = Math.random_2d(cell)
                if alpha < threshold {
                    discard()
                }
                return vec4(rgb.x, rgb.y, rgb.z, 1.0)
            }
            return vec4(rgb.x * alpha, rgb.y * alpha, rgb.z * alpha, alpha)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
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
        sort_min_camera_angle_deg: 1.0
        sort_min_camera_move: 0.02
        sort_cull_margin: 0.5
        sort_behind_margin: 0.05
    }
}

/// The splat draw shader: one quad, `splat_id` per instance; everything
/// else is uniforms and the two data textures (see module docs).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSplat {
    #[deref]
    pub draw_vars: DrawVars,
    /// Instance stream: splat record index (exact integer in f32).
    #[live(0.0)]
    pub splat_id: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ViewSplat {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[redraw]
    #[live]
    draw_splat: DrawSplat,

    #[live]
    src: Option<ScriptHandleRef>,
    /// Accepted for compatibility; splats are unlit so no environment map is
    /// used.
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
    /// Camera turn that triggers a re-sort. 1 degree: far inside the
    /// sorter's 0.5-NDC cull margin, and a view-z order is still right to
    /// ~1.7% relative depth.
    #[live(1.0)]
    sort_min_camera_angle_deg: f32,
    #[live(0.02)]
    sort_min_camera_move: f32,
    /// NDC kept beyond the frustum edge when the sorter culls, so splats
    /// entering the view between two sorts already render.
    #[live(0.5)]
    sort_cull_margin: f32,
    /// Distance behind the camera plane (local units) the sorter still keeps.
    #[live(0.05)]
    sort_behind_margin: f32,

    #[rust]
    loaded_src_handle: Option<ScriptHandle>,
    #[rust]
    scene_format: Option<SplatFileFormat>,
    #[rust]
    scene_antialias: bool,
    #[rust]
    pending_scene: Option<makepad_splat::SplatScene>,
    #[rust]
    gpu_scene: Option<GpuScene>,
    #[rust(vec3(0.0, 0.0, 0.0))]
    scene_center: Vec3f,
    #[rust(1.0)]
    scene_unit_scale: f32,

    /// Instance stream currently drawn: visible ids, far to near (or the
    /// identity order when sorting is off).
    #[rust]
    instance_order: Vec<f32>,
    #[rust]
    spare_order: Option<Vec<f32>>,
    #[rust]
    depth_sort_request_tx: FromUISender<SortRequest>,
    #[rust]
    depth_sort_result_rx: ToUIReceiver<SortResult>,
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
    depth_sort_pending_result: Option<SortResult>,
    #[rust]
    depth_sort_last_camera_pos: Option<Vec3f>,
    #[rust]
    depth_sort_last_camera_forward: Option<Vec3f>,
    #[rust]
    depth_sort_last_model: Option<Mat4f>,
    #[rust]
    depth_sort_request_started: Option<Instant>,
    #[rust]
    stats: ViewSplatStats,
}

/// GPU-resident scene: the two data textures plus the sorter's mirror.
struct GpuScene {
    splat_data: Texture,
    chunk_bounds: Texture,
    count: usize,
    layout: Vec4f,
    scale: Vec4f,
    /// Moved to the sort worker on first use.
    sort_scene: Option<SortScene>,
    /// The textures' CPU-side data is released once the backend has uploaded
    /// it (16 B/splat of host RAM otherwise kept for the scene's lifetime).
    cpu_mirrors_released: bool,
}

impl GpuScene {
    fn release_cpu_mirrors(&mut self, cx: &mut Cx) {
        if self.cpu_mirrors_released {
            return;
        }
        let uploaded = |format: &TextureFormat| match format {
            TextureFormat::VecBGRAu8_32 { updated, data, .. } => {
                updated.is_empty() && data.is_some()
            }
            TextureFormat::VecRGBAf32 { updated, data, .. } => {
                updated.is_empty() && data.is_some()
            }
            _ => false,
        };
        if !uploaded(self.splat_data.get_format(cx)) || !uploaded(self.chunk_bounds.get_format(cx)) {
            return;
        }
        drop(self.splat_data.take_vec_u32(cx));
        drop(self.chunk_bounds.take_vec_f32(cx));
        self.cpu_mirrors_released = true;
    }
}

/// Measured costs of the splat renderer, read by benchmarks/tests.
/// Times are milliseconds; byte counts are what was handed to the GPU.
#[derive(Clone, Debug, Default)]
pub struct ViewSplatStats {
    /// Splats in the GPU scene (after opacity/scale filtering).
    pub splat_count: usize,
    /// Splats in the current instance stream (after the sorter's culling).
    pub visible_count: usize,
    /// File decode (`load_splat_from_bytes`).
    pub load_ms: f64,
    /// Building the GPU-side representation from the decoded scene.
    pub build_ms: f64,
    /// Static per-scene upload: texture bytes.
    pub static_upload_bytes: u64,
    /// Bytes of the instance stream delivered by the last applied sort.
    pub last_sort_upload_bytes: u64,
    /// Bytes pushed into the instance buffer on the last draw (re-uploaded
    /// by the backend every redraw).
    pub last_frame_instance_bytes: u64,
    /// Number of depth-sort results applied.
    pub sorts_applied: u64,
    /// Worker-side time of the last sort.
    pub last_sort_ms: f64,
    /// Sum of worker-side sort times.
    pub total_sort_ms: f64,
    /// Request-to-applied latency of the last sort.
    pub last_sort_latency_ms: f64,
    /// Main-thread time of applying the last sort result.
    pub last_sort_apply_ms: f64,
    /// Upper-bound estimate of fragments shaded per viewport pixel for the
    /// last sorted view (sum of culling-bound quad areas / viewport area).
    pub est_quad_overdraw: f32,
    /// Main-thread time of the last draw (uniforms + instance copy).
    pub last_draw_ms: f64,
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

impl ViewSplat {
    /// Node-transform scale; hosts use it to fix per-source axis conventions
    /// (generated worlds are y-up, scan-class plys are y-down and need a
    /// (1,-1,1) flip). Takes effect on the next draw — no re-mesh needed.
    pub fn set_scale(&mut self, scale: Vec3f) {
        self.scale = scale;
    }

    /// Measured load/build/upload/sort costs (see [`ViewSplatStats`]).
    pub fn stats(&self) -> &ViewSplatStats {
        &self.stats
    }

    /// True once the source resource has been decoded (or failed) and the
    /// GPU representation for it exists.
    pub fn is_scene_ready(&self) -> bool {
        self.loaded_src_handle.is_some() && self.gpu_scene.is_some()
    }

    fn resource_metadata_by_handle(cx: &mut Cx, handle: ScriptHandle) -> Option<(PathBuf, bool)> {
        let resources = cx.script_data.resources.resources.borrow();
        let resource = resources
            .iter()
            .find(|resource| resource.has_handle(handle))?;
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

        cx.load_script_resource(handle);

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

    fn next_sort_generation(&mut self) {
        self.depth_sort_generation = self.depth_sort_generation.wrapping_add(1);
        if self.depth_sort_generation == 0 {
            self.depth_sort_generation = 1;
        }
    }

    fn reset_depth_sort_state_for_new_scene(&mut self) {
        self.gpu_scene = None;
        self.instance_order.clear();
        self.spare_order = None;
        self.depth_sort_pending_result = None;
        self.depth_sort_in_flight = false;
        self.depth_sort_scene_uploaded_generation = 0;
        self.depth_sort_next_request_id = 0;
        self.depth_sort_last_applied_request_id = 0;
        self.depth_sort_last_camera_pos = None;
        self.depth_sort_last_camera_forward = None;
        self.depth_sort_last_model = None;
        self.next_sort_generation();
    }

    fn ensure_depth_sort_thread(&mut self, cx: &mut CxDraw) {
        if self.depth_sort_thread_started {
            return;
        }
        self.depth_sort_request_tx.new_channel();
        let request_rx = self
            .depth_sort_request_tx
            .receiver()
            .expect("depth sort receiver is taken exactly once per channel");
        let result_tx = self.depth_sort_result_rx.sender();
        if let Ok(task) = cx.spawn_thread(move || run_sort_worker(request_rx, result_tx)) {
            task.detach();
        }
        self.depth_sort_thread_started = true;
    }

    fn upload_sort_scene_to_worker(&mut self, cx: &mut CxDraw) -> bool {
        let Some(sort_scene) = self
            .gpu_scene
            .as_mut()
            .and_then(|scene| scene.sort_scene.take())
        else {
            return false;
        };
        self.ensure_depth_sort_thread(cx);
        let generation = self.depth_sort_generation;
        let request = SortRequest::SetScene {
            generation,
            scene: sort_scene,
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

    fn model_changed(a: &Mat4f, b: &Mat4f) -> bool {
        a.v.iter().zip(b.v.iter()).any(|(x, y)| (x - y).abs() >= 1e-6)
    }

    /// A new sort is worth asking for when the node moved, or the camera
    /// moved / turned past the thresholds. Between sorts the previous order
    /// (and visibility set, with its NDC margin) keeps drawing.
    fn should_request_depth_sort(&self, scene_state: &SceneState3D, model_matrix: &Mat4f) -> bool {
        let Some(last_pos) = self.depth_sort_last_camera_pos else {
            return true;
        };
        let Some(last_forward) = self.depth_sort_last_camera_forward else {
            return true;
        };
        if self
            .depth_sort_last_model
            .map(|last| Self::model_changed(&last, model_matrix))
            .unwrap_or(true)
        {
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

    fn sort_camera(&self, cx: &mut Cx, scene_state: &SceneState3D, model_matrix: Mat4f) -> SortCamera {
        let render_w = scene_state.viewport_rect.size.x.max(1.0) as f32;
        let render_h = scene_state.viewport_rect.size.y.max(1.0) as f32;
        let proj = scene_state.projection;
        let focal_x = proj.v[0].abs().max(0.00001) * render_w * 0.5;
        let focal_y = proj.v[5].abs().max(0.00001) * render_h * 0.5;
        let mut get = |id: LiveId, default: f32| -> f32 {
            let mut value = [default];
            // The shader knobs are uniforms (script-set or overridden); cull
            // with the live values so the CPU side matches the GPU side.
            self.draw_splat.draw_vars.get_uniform(cx, id, &mut value);
            value[0]
        };
        SortCamera {
            view: scene_state.view,
            model: model_matrix,
            projection: proj,
            radial: self.scene_format == Some(SplatFileFormat::Ply),
            focal_px: focal_x.max(focal_y),
            ndc_per_px: vec2(2.0 / render_w, 2.0 / render_h),
            splat_std_dev: get(live_id!(splat_std_dev), 2.8),
            coarse_cull_guard: get(live_id!(coarse_cull_guard), 2.0),
            min_pixel_radius: get(live_id!(min_pixel_radius), 0.0),
            max_pixel_radius: get(live_id!(max_pixel_radius), 512.0),
            cull_margin_ndc: self.sort_cull_margin,
            behind_margin: self.sort_behind_margin,
            viewport_px: render_w * render_h,
        }
    }

    fn request_depth_sort_if_needed(
        &mut self,
        cx: &mut CxDraw,
        scene_state: &SceneState3D,
        model_matrix: Mat4f,
    ) {
        if !self.sort_back_to_front || self.gpu_scene.is_none() {
            return;
        }
        if self.depth_sort_scene_uploaded_generation != self.depth_sort_generation
            && !self.upload_sort_scene_to_worker(cx)
        {
            return;
        }
        if self.depth_sort_in_flight || !self.should_request_depth_sort(scene_state, &model_matrix) {
            return;
        }

        self.depth_sort_next_request_id = self.depth_sort_next_request_id.wrapping_add(1);
        if self.depth_sort_next_request_id == 0 {
            self.depth_sort_next_request_id = 1;
        }
        let request_id = self.depth_sort_next_request_id;
        let request = SortRequest::Sort {
            generation: self.depth_sort_generation,
            request_id,
            camera: self.sort_camera(cx.cx, scene_state, model_matrix),
            recycled: self.spare_order.take(),
        };
        if self.depth_sort_request_tx.send(request).is_ok() {
            self.depth_sort_in_flight = true;
            self.depth_sort_request_started = Some(Instant::now());
            self.depth_sort_last_camera_pos = Some(scene_state.camera_pos);
            self.depth_sort_last_camera_forward =
                Some(Self::camera_forward_from_view(&scene_state.view));
            self.depth_sort_last_model = Some(model_matrix);
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

    fn apply_pending_depth_sort(&mut self) {
        let Some(result) = self.depth_sort_pending_result.take() else {
            return;
        };
        if result.generation != self.depth_sort_generation
            || result.request_id <= self.depth_sort_last_applied_request_id
        {
            return;
        }
        let apply_started = Instant::now();
        let previous = mem::replace(&mut self.instance_order, result.order);
        self.spare_order = Some(previous);
        self.depth_sort_last_applied_request_id = result.request_id;
        let stats: SortStats = result.stats;
        self.stats.sorts_applied += 1;
        self.stats.visible_count = stats.visible;
        self.stats.last_sort_ms = stats.sort_ms;
        self.stats.total_sort_ms += stats.sort_ms;
        self.stats.est_quad_overdraw = stats.est_quad_overdraw;
        self.stats.last_sort_upload_bytes =
            (self.instance_order.len() * mem::size_of::<f32>()) as u64;
        self.stats.last_sort_latency_ms = self
            .depth_sort_request_started
            .map(|started| started.elapsed().as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        self.stats.last_sort_apply_ms = apply_started.elapsed().as_secs_f64() * 1000.0;
    }

    /// Sorting off: draw everything in record order.
    fn ensure_identity_order(&mut self) {
        let Some(count) = self.gpu_scene.as_ref().map(|scene| scene.count) else {
            return;
        };
        if self.instance_order.len() != count
            || self.depth_sort_last_applied_request_id != 0
        {
            self.instance_order.clear();
            self.instance_order.extend((0..count).map(|i| i as f32));
            self.depth_sort_last_applied_request_id = 0;
            self.stats.visible_count = count;
        }
    }

    fn ensure_scene_loaded(&mut self, cx: &mut CxDraw) {
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
                let load_started = Instant::now();
                let loaded = load_splat_from_bytes(&data, Some(abs_path.as_path()));
                self.stats = ViewSplatStats {
                    load_ms: load_started.elapsed().as_secs_f64() * 1000.0,
                    ..Default::default()
                };
                match loaded {
                    Ok(scene) => {
                        self.scene_antialias = scene.antialias;
                        self.scene_format = Some(scene.format);
                        self.update_scene_fit(&scene);
                        self.pending_scene = Some(scene);
                        self.reset_depth_sort_state_for_new_scene();
                    }
                    Err(error) => {
                        log!("ViewSplat parse error ({}): {}", abs_path.display(), error);
                        self.pending_scene = None;
                        self.scene_format = None;
                        self.reset_depth_sort_state_for_new_scene();
                    }
                }
                self.loaded_src_handle = Some(handle);
            }
            ResourceResolve::Error { handle } => {
                self.pending_scene = None;
                self.scene_format = None;
                self.reset_depth_sort_state_for_new_scene();
                self.loaded_src_handle = Some(handle);
            }
            ResourceResolve::Pending { handle } => {
                let _ = handle;
            }
            ResourceResolve::Missing => {}
        }
    }

    fn update_scene_fit(&mut self, scene: &makepad_splat::SplatScene) {
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

    /// Decoded scene -> renderer records (normalized, filtered, scaled).
    fn scene_records(&self, scene: &makepad_splat::SplatScene) -> Vec<SplatRecord> {
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

        let mut records = Vec::with_capacity(max_splats);
        for splat in scene.splats.iter().take(max_splats) {
            let alpha = (splat.color[3] * opacity_scale).clamp(0.0, 1.0);
            if alpha <= 0.001 {
                continue;
            }
            let sx = (splat.scale[0].abs() * normalize_scale).max(min_radius) * radius_scale;
            let sy = (splat.scale[1].abs() * normalize_scale).max(min_radius) * radius_scale;
            let sz = (splat.scale[2].abs() * normalize_scale).max(min_radius) * radius_scale;
            if sx <= 0.0 && sy <= 0.0 && sz <= 0.0 {
                continue;
            }
            let q = {
                let [x, y, z, w] = splat.rotation;
                let len2 = x * x + y * y + z * z + w * w;
                if len2 <= f32::EPSILON {
                    [0.0, 0.0, 0.0, 1.0]
                } else {
                    let inv_len = len2.sqrt().recip();
                    [x * inv_len, y * inv_len, z * inv_len, w * inv_len]
                }
            };
            records.push(SplatRecord {
                center: [
                    (splat.position[0] - center.x) * normalize_scale,
                    (splat.position[1] - center.y) * normalize_scale,
                    (splat.position[2] - center.z) * normalize_scale,
                ],
                scales: [sx, sy, sz],
                rotation: q,
                color: [
                    splat.color[0].clamp(0.0, 1.0),
                    splat.color[1].clamp(0.0, 1.0),
                    splat.color[2].clamp(0.0, 1.0),
                    alpha,
                ],
            });
        }
        records
    }

    fn ensure_gpu_scene(&mut self, cx: &mut CxDraw) -> bool {
        if self.gpu_scene.is_some() {
            return true;
        }
        let Some(scene) = self.pending_scene.take() else {
            return false;
        };
        let build_started = Instant::now();
        let records = self.scene_records(&scene);
        drop(scene);
        if records.is_empty() {
            return false;
        }
        // Instance ids are exact f32 integers and the data texture has at
        // most 16384 rows of 2048 records.
        let max_records = (1usize << 24).min(16384 * RECORDS_PER_ROW);
        let records = if records.len() > max_records {
            log!(
                "ViewSplat: {} splats exceed the {} record limit; drawing the first {}",
                records.len(),
                max_records,
                max_records
            );
            records[..max_records].to_vec()
        } else {
            records
        };
        let (order, codes) = morton_sorted(&records);
        let ordered: Vec<SplatRecord> = order.iter().map(|&i| records[i as usize]).collect();
        drop(records);
        drop(order);
        let packed = PackedScene::build(&ordered, &codes);
        drop(ordered);

        let splat_data = Texture::new_with_format(
            cx.cx,
            TextureFormat::VecBGRAu8_32 {
                width: RECORDS_PER_ROW * 4,
                height: packed.rows,
                data: Some(packed.words),
                updated: TextureUpdated::Full,
            },
        );
        let chunk_bounds = Texture::new_with_format(
            cx.cx,
            TextureFormat::VecRGBAf32 {
                width: CHUNKS_PER_ROW * 2,
                height: packed.chunk_rows,
                data: Some(packed.chunk_texels),
                updated: TextureUpdated::Full,
            },
        );
        let static_upload_bytes = (packed.rows * RECORDS_PER_ROW * 16) as u64
            + (packed.chunk_rows * CHUNKS_PER_ROW * 32) as u64;
        let count = packed.count;
        self.gpu_scene = Some(GpuScene {
            splat_data,
            chunk_bounds,
            count,
            layout: vec4(
                1.0 / (RECORDS_PER_ROW * 4) as f32,
                1.0 / packed.rows as f32,
                1.0 / (CHUNKS_PER_ROW * 2) as f32,
                1.0 / packed.chunk_rows as f32,
            ),
            scale: vec4(
                packed.scale_range.ln_min,
                packed.scale_range.ln_range / 255.0,
                0.0,
                0.0,
            ),
            sort_scene: Some(SortScene::new(
                packed.centers,
                packed.radius_bound,
                packed.axis_product,
            )),
            cpu_mirrors_released: false,
        });
        self.instance_order.clear();
        self.spare_order = None;
        self.depth_sort_pending_result = None;
        self.depth_sort_in_flight = false;
        self.depth_sort_scene_uploaded_generation = 0;
        self.depth_sort_next_request_id = 0;
        self.depth_sort_last_applied_request_id = 0;
        self.depth_sort_last_camera_pos = None;
        self.depth_sort_last_camera_forward = None;
        self.depth_sort_last_model = None;
        self.next_sort_generation();
        self.stats.splat_count = packed.records;
        self.stats.build_ms = build_started.elapsed().as_secs_f64() * 1000.0;
        self.stats.static_upload_bytes = static_upload_bytes;
        true
    }

    fn node_matrix(&self) -> Mat4f {
        compose_scene_node_transform(self.position, self.rotation, self.scale)
    }

    fn set_draw_uniforms(&mut self, cx: &mut CxDraw, scene_state: &SceneState3D, node_matrix: Mat4f) {
        let render_w = scene_state.viewport_rect.size.x.max(1.0) as f32;
        let render_h = scene_state.viewport_rect.size.y.max(1.0) as f32;
        let proj = scene_state.projection;
        let focal = [
            proj.v[0].abs().max(0.00001) * render_w * 0.5,
            proj.v[5].abs().max(0.00001) * render_h * 0.5,
        ];
        let m = &node_matrix.v;
        let dv = &mut self.draw_splat.draw_vars;
        dv.set_uniform(cx.cx, live_id!(render_size), &[render_w, render_h]);
        dv.set_uniform(cx.cx, live_id!(focal_pixels), &focal);
        dv.set_uniform(
            cx.cx,
            live_id!(ndc_per_pixel),
            &[2.0 / render_w.max(1.0), 2.0 / render_h.max(1.0)],
        );
        dv.set_uniform(cx.cx, live_id!(mm_r0), &[m[0], m[4], m[8], m[12]]);
        dv.set_uniform(cx.cx, live_id!(mm_r1), &[m[1], m[5], m[9], m[13]]);
        dv.set_uniform(cx.cx, live_id!(mm_r2), &[m[2], m[6], m[10], m[14]]);
        if let Some(scene) = self.gpu_scene.as_ref() {
            let l = scene.layout;
            let s = scene.scale;
            dv.set_uniform(cx.cx, live_id!(u_layout), &[l.x, l.y, l.z, l.w]);
            dv.set_uniform(cx.cx, live_id!(u_scale), &[s.x, s.y, s.z, s.w]);
            dv.set_texture(0, &scene.splat_data);
            dv.set_texture(1, &scene.chunk_bounds);
        }
        if self.auto_antialias_blur {
            let blur = if self.scene_antialias { 0.3 } else { 0.0 };
            dv.set_uniform(cx.cx, live_id!(blur_pixels), &[blur]);
        }
        dv.options.depth_write = false;
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

    fn draw_3d(&mut self, cx: &mut Cx3d, _scope: &mut Scope) -> DrawStep {
        let Some(scene_state) = scene_state_from_cx(cx) else {
            return DrawStep::done();
        };
        let draw_started = Instant::now();
        let node_matrix = Mat4f::mul(&scene_node_world_transform_from_cx(cx), &self.node_matrix());

        self.ensure_scene_loaded(cx);
        if !self.ensure_gpu_scene(cx) {
            return DrawStep::done();
        }
        if let Some(scene) = self.gpu_scene.as_mut() {
            scene.release_cpu_mirrors(cx.cx);
        }

        let _ = self.poll_depth_sort_results();
        if self.sort_back_to_front {
            self.apply_pending_depth_sort();
            self.request_depth_sort_if_needed(cx, &scene_state, node_matrix);
        } else {
            self.depth_sort_pending_result = None;
            self.depth_sort_in_flight = false;
            self.depth_sort_last_camera_pos = None;
            self.depth_sort_last_camera_forward = None;
            self.depth_sort_last_model = None;
            self.ensure_identity_order();
        }
        if self.instance_order.is_empty() {
            // First sort still in flight: nothing to draw yet.
            return DrawStep::done();
        }

        self.set_draw_uniforms(cx, &scene_state, node_matrix);
        if let Some(mut instances) = cx.begin_many_instances(&self.draw_splat.draw_vars) {
            instances.instances.extend_from_slice(&self.instance_order);
            let area = cx.end_many_instances(instances);
            self.draw_splat.draw_vars.area =
                cx.update_area_refs(self.draw_splat.draw_vars.area, area);
        }
        self.stats.last_frame_instance_bytes =
            (self.instance_order.len() * mem::size_of::<f32>()) as u64;
        self.stats.last_draw_ms = draw_started.elapsed().as_secs_f64() * 1000.0;

        DrawStep::done()
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}
