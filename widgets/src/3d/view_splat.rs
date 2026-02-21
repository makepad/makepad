use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};
use makepad_splat::{load_splat_from_bytes, SplatScene};
use std::{path::PathBuf, rc::Rc};

use super::scene_3d::{apply_scene_to_draw_pbr, scene_state_from_scope};
use crate::makepad_draw::shader::draw_pbr::PbrMeshHandle;

script_mod! {
    use mod.prelude.widgets_internal.*

    set_type_default() do #(DrawSplatPbr::script_shader(vm)){
        ..mod.draw.DrawPbr
        render_size: vec2(1024.0, 768.0)
        splat_std_dev: 2.8
        min_pixel_radius: 0.0
        max_pixel_radius: 512.0
        blur_pixels: 0.3
        alpha_cutoff: 0.002
        dither_depth_cutout: 0.0
        dither_scale: 1.0
        v_ndc: varying(vec2f)

        quat_rotate: fn(q: vec4, v: vec3) {
            let t = 2.0 * cross(q.xyz, v);
            return v + q.w * t + cross(q.xyz, t)
        }

        vertex: fn() {
            let quad = vec2(self.geom.ny_nz_uv.z, self.geom.ny_nz_uv.w);
            let center_local = vec3(self.geom.pos_nx.x, self.geom.pos_nx.y, self.geom.pos_nx.z);
            let scale_local = vec3(
                max(abs(self.geom.pos_nx.w), 0.000001),
                max(abs(self.geom.ny_nz_uv.x), 0.000001),
                max(abs(self.geom.ny_nz_uv.y), 0.000001)
            );
            let q_raw = self.geom.tangent;
            let q_len = length(q_raw);
            let q = if q_len > 0.000001 {
                q_raw / q_len
            } else {
                vec4(0.0, 0.0, 0.0, 1.0)
            };

            let axis_local_0 = self.quat_rotate(q, vec3(scale_local.x, 0.0, 0.0));
            let axis_local_1 = self.quat_rotate(q, vec3(0.0, scale_local.y, 0.0));
            let axis_local_2 = self.quat_rotate(q, vec3(0.0, 0.0, scale_local.z));

            let center_world4 = self.model_matrix * vec4(center_local.x, center_local.y, center_local.z, 1.0);
            let center_world = vec3(center_world4.x, center_world4.y, center_world4.z);
            let axis_world_0 = (self.model_matrix * vec4(axis_local_0.x, axis_local_0.y, axis_local_0.z, 0.0)).xyz;
            let axis_world_1 = (self.model_matrix * vec4(axis_local_1.x, axis_local_1.y, axis_local_1.z, 0.0)).xyz;
            let axis_world_2 = (self.model_matrix * vec4(axis_local_2.x, axis_local_2.y, axis_local_2.z, 0.0)).xyz;
            let center_view4 = self.view_matrix * vec4(center_world.x, center_world.y, center_world.z, 1.0);
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

            let axis_view_0 = (self.view_matrix * vec4(axis_world_0.x, axis_world_0.y, axis_world_0.z, 0.0)).xyz;
            let axis_view_1 = (self.view_matrix * vec4(axis_world_1.x, axis_world_1.y, axis_world_1.z, 0.0)).xyz;
            let axis_view_2 = (self.view_matrix * vec4(axis_world_2.x, axis_world_2.y, axis_world_2.z, 0.0)).xyz;

            let c00 = axis_view_0.x * axis_view_0.x + axis_view_1.x * axis_view_1.x + axis_view_2.x * axis_view_2.x;
            let c01 = axis_view_0.x * axis_view_0.y + axis_view_1.x * axis_view_1.y + axis_view_2.x * axis_view_2.y;
            let c02 = axis_view_0.x * axis_view_0.z + axis_view_1.x * axis_view_1.z + axis_view_2.x * axis_view_2.z;
            let c11 = axis_view_0.y * axis_view_0.y + axis_view_1.y * axis_view_1.y + axis_view_2.y * axis_view_2.y;
            let c12 = axis_view_0.y * axis_view_0.z + axis_view_1.y * axis_view_1.z + axis_view_2.y * axis_view_2.z;
            let c22 = axis_view_0.z * axis_view_0.z + axis_view_1.z * axis_view_1.z + axis_view_2.z * axis_view_2.z;

            let proj_x = self.projection_matrix * vec4(1.0, 0.0, 0.0, 0.0);
            let proj_y = self.projection_matrix * vec4(0.0, 1.0, 0.0, 0.0);
            let safe_render = vec2(max(self.render_size.x, 1.0), max(self.render_size.y, 1.0));
            let focal = vec2(
                max(abs(proj_x.x), 0.00001) * safe_render.x * 0.5,
                max(abs(proj_y.y), 0.00001) * safe_render.y * 0.5
            );

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

            let max_radius = if self.max_pixel_radius > 0.0 { self.max_pixel_radius } else { 1000000.0 };
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
            let ndc_offset = vec2(
                (2.0 * pixel_offset.x) / safe_render.x,
                (2.0 * pixel_offset.y) / safe_render.y
            );
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
        sort_back_to_front: true
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSplatPbr {
    #[deref]
    pub draw_super: DrawPbr,
    #[live(vec2(1024.0, 768.0))]
    pub render_size: Vec2f,
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
    sort_back_to_front: bool,

    #[rust]
    scene: Option<SplatScene>,
    #[rust]
    loaded_src_handle: Option<ScriptHandle>,
    #[rust]
    loaded_env_handle: Option<ScriptHandle>,
    #[rust]
    splat_mesh: Option<PbrMeshHandle>,
    #[rust(vec3(0.0, 0.0, 0.0))]
    scene_center: Vec3f,
    #[rust(1.0)]
    scene_unit_scale: f32,
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
    fn resource_metadata_by_handle(cx: &mut Cx, handle: ScriptHandle) -> Option<(PathBuf, bool)> {
        let resources = cx.script_data.resources.resources.borrow();
        let resource = resources.iter().find(|resource| resource.handle == handle)?;
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
                        self.scene = Some(scene);
                        self.update_scene_fit();
                        self.splat_mesh = None;
                    }
                    Err(error) => {
                        log!("ViewSplat parse error ({}): {}", abs_path.display(), error);
                        self.scene = None;
                        self.splat_mesh = None;
                    }
                }
                self.loaded_src_handle = Some(handle);
            }
            ResourceResolve::Error { handle } => {
                self.scene = None;
                self.splat_mesh = None;
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
        let mut normals: Vec<[f32; 3]> = Vec::with_capacity(estimated * 4); // packed splat scale xyz (local)
        let mut tangents: Vec<[f32; 4]> = Vec::with_capacity(estimated * 4); // packed quaternion xyzw
        let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(estimated * 4);
        let mut colors: Vec<[f32; 4]> = Vec::with_capacity(estimated * 4);
        let mut indices: Vec<u32> = Vec::with_capacity(estimated * 6);

        let quad_coords = [
            [-1.0_f32, -1.0_f32],
            [1.0_f32, -1.0_f32],
            [1.0_f32, 1.0_f32],
            [-1.0_f32, 1.0_f32],
        ];

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

            let base = positions.len() as u32;
            for i in 0..4 {
                positions.push(center_local);
                normals.push([sx, sy, sz]);
                tangents.push(q);
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
        self.draw_splat.render_size = vec2(
            scene_state.viewport_rect.size.x.max(1.0) as f32,
            scene_state.viewport_rect.size.y.max(1.0) as f32,
        );
        self.draw_splat.draw_super.set_depth_write(false);
        self.draw_splat.draw_super.model_matrix = node_matrix;

        let _ = self.draw_splat.draw_super.draw_mesh(cx, splat_mesh);

        DrawStep::done()
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}
