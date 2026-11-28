//! Proxy rays shared by traced passes, and the outside and inside surface passes.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.draw.TracedProxy = {
        v_trace_p: varying(vec3f)
        v_trace_ray: varying(vec3f)
        v_trace_lo: varying(vec3f)
        v_trace_hi: varying(vec3f)
        v_trace_payload: varying(vec4f)

        trace_proxy_vertex: fn() {
            self.v_trace_p = self.traced_point()
            self.v_trace_lo = self.traced_min()
            self.v_trace_hi = self.traced_max()
            // Decode before interpolation. The packed float can be a NaN.
            self.v_trace_payload = unpack4u8(self.geom.color)
        }
        // Undo an orthogonal, uniformly scaled basis, including its handedness.
        trace_local_ray: fn(basis: mat4x4f, ray: vec3) -> vec3 {
            let ax = (basis * vec4(1.0, 0.0, 0.0, 0.0)).xyz
            let ay = (basis * vec4(0.0, 1.0, 0.0, 0.0)).xyz
            let az = (basis * vec4(0.0, 0.0, 1.0, 0.0)).xyz
            return vec3(dot(ax, ray), dot(ay, ray), dot(az, ray)) / dot(ax, ax)
        }
        // Entry, exit and time per cell along one axis. A parallel ray owns the
        // low side of a shared plane, so adjacent boxes cannot both claim it.
        trace_slab: fn(p: float, d: float, lo: float, hi: float) -> vec3 {
            if d == 0.0 {
                if p < lo || p >= hi { return vec3(1.0e30, -1.0e30, 0.0) }
                return vec3(-1.0e30, 1.0e30, 0.0)
            }
            let a = (lo - p) / d
            let b = (hi - p) / d
            return vec3(min(a, b), max(a, b), min(abs(1.0 / d), 1.0e30))
        }
        // A parallel axis containing p imposes no lower bound on entry.
        trace_axis_entry: fn(p: float, d: float, lo: float, hi: float) -> float {
            if d > 0.0 { return (lo - p) / d }
            if d < 0.0 { return (hi - p) / d }
            return -1.0e30
        }
        trace_entry_face: fn(p: vec3, d: vec3) -> bool {
            let lo = self.v_trace_lo
            let hi = self.v_trace_hi
            let entry = max(self.trace_axis_entry(p.x, d.x, lo.x, hi.x),
                max(self.trace_axis_entry(p.y, d.y, lo.y, hi.y), self.trace_axis_entry(p.z, d.z, lo.z, hi.z)))
            return entry >= -0.001
        }
    }

    mod.draw.DrawSceneTraced = #(DrawScenePbr::script_shader(vm)){
        ..mod.draw.ScenePbrSurface,
        ..mod.draw.TracedProxy
        cull_mode: @None
        depth_out: mod.shader.fragment_depth(@greater)
        trace_pixel: mod.shader.fragment_position()
        trace_inv_viewport: uniform(vec2(1.0, 1.0))
        trace_depth_map: uniform(vec2(1.0, 0.0))
        trace_volume: texture_3d(float)
        trace_palette: texture_2d(float)
        v_trace_origin_scale: varying(vec4f)
        v_trace_aux: varying(vec4f)
        ibl_ctl: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        voxel_ao_strength: uniform(1.0)
        mat_ibl_meta: mod.draw.mat_ibl_meta
        mat_ibl_dir: mod.draw.mat_ibl_dir
        mat_ibl_level: mod.draw.mat_ibl_level
        mat_ibl_lookup: mod.draw.mat_ibl_lookup
        mat_ibl_sky_env: mod.draw.mat_ibl_sky_env
        mat_ibl_ambient: mod.draw.mat_ibl_ambient
        mat_ibl_lane_sky_env_lit: mod.draw.mat_ibl_lane_sky_env_lit
        mat_ibl_lane_ambient: mod.draw.mat_ibl_lane_ambient

        sky_env_lit: fn(r: vec3, rough: float, vis: float) -> vec3 {
            if self.ibl_ctl.x > 0.5 { return self.mat_ibl_lane_sky_env_lit(r, rough, vis) }
            return self.sky_env(r, rough)
        }
        mat_ambient: fn(n: vec3, fallback: vec3) -> vec3 {
            if self.ibl_ctl.x > 0.5 { return self.mat_ibl_lane_ambient(n, fallback) }
            return fallback
        }
        coat_treeline: fn(env: vec3, r: vec3) -> vec3 {
            if self.ibl_ctl.x > 0.5 { return env }
            let az = atan2(r.z, r.x)
            let ridge = 0.035 + 0.05 * self.tn_noise(vec2(az * 9.0, 0.5)) + 0.03 * self.tn_noise(vec2(az * 37.0, 1.5))
            let tree = (1.0 - smoothstep(ridge - 0.01, ridge + 0.01, r.y)) * smoothstep(-0.02, 0.0, r.y)
            return mix(env, self.sun_ground * 0.28 + self.fog_color * 0.12, tree * 0.85)
        }
        traced_ao: fn(hit: vec4, p: vec3, d: vec3) -> float { return 1.0 }
        material_occlusion_at: fn(uv: vec2) -> float { return uv.x }
        surface_clear_coat: fn(coat_uv: vec2) -> vec2 { return coat_uv }
        gi_receive_early: fn(wp: vec3, n_geo: vec3) -> vec4 { return vec4(0.0, 0.0, 0.0, 1.0) }

        traced_point: fn() -> vec3 { return vec3(self.geom.px, self.geom.py, self.geom.pz) }
        traced_scale: fn() -> float { return 1.0 }
        traced_min: fn() -> vec3 { return vec3(-1.0, -1.0, -1.0) }
        traced_max: fn() -> vec3 { return vec3(1.0, 1.0, 1.0) }

        // Unit sphere. Distance is measured from p along the normalized d.
        trace_sphere_hit: fn(p: vec3, d: vec3) -> vec4 {
            let b = dot(p, d)
            let c = dot(p, p) - 1.0
            let disc = b * b - c
            if disc < 0.0 { return vec4(-1.0, 0.0, 0.0, 0.0) }
            let t = -b - sqrt(disc)
            if t < -0.000001 { return vec4(-1.0, 0.0, 0.0, 0.0) }
            let n = normalize(p + d * max(t, 0.0))
            let oct = n / (abs(n.x) + abs(n.y) + abs(n.z))
            var xy = oct.xy
            if oct.z < 0.0 {
                xy = (vec2(1.0, 1.0) - abs(oct.yx))
                    * (step(vec2(0.0, 0.0), oct.xy) * 2.0 - vec2(1.0, 1.0))
            }
            return vec4(max(t, 0.0), xy.x, xy.y, 0.0)
        }
        traced_hit: fn(p: vec3, d: vec3) -> vec4 { return self.trace_sphere_hit(p, d) }
        traced_hit_interval: fn(p: vec3, d: vec3, t_enter: float, exit_t: float) -> vec4 {
            return self.traced_hit(p, d)
        }
        traced_base: fn(material: float, p: vec3) -> vec4 { return vec4(1.0, 1.0, 1.0, 1.0) }
        traced_mr: fn(material: float) -> vec4 { return vec4(0.0, 0.5, 0.0, 0.0) }
        traced_emission: fn(material: float) -> vec3 { return vec3(0.0, 0.0, 0.0) }

        csm_own_limit: fn(ci: float) -> float { return 2.0 }

        vertex: fn() {
            let p = vec3(self.geom.px, self.geom.py, self.geom.pz)
            self.trace_proxy_vertex()
            let scale = self.traced_scale()
            self.v_trace_origin_scale = vec4(p * scale - self.v_trace_p, scale)
            self.v_trace_aux = unpack4u8(self.geom.ao_uv)
            let basis = self.draw_list.view_transform * self.transform
            let world = basis * vec4(p, 1.0)
            let view = self.draw_pass.camera_view * world
            let eye = self.trace_model_eye(self.draw_pass.camera_projection, 1.0 - self.depth_bias, self.draw_pass.camera_view * basis)
            // Keep this affine in p so perspective interpolation preserves the ray.
            self.v_trace_ray = p * eye.w - eye.xyz
            self.vertex_pos = self.draw_pass.camera_projection * vec4(view.xyz * (1.0 - self.depth_bias), view.w)
        }

        // Each XR eye occupies either a layer or one eye-width of a shared target.
        trace_ndc: fn(pixel: vec2, inv_size: vec2) -> vec2 {
            let uv = pixel * inv_size
            return vec2(fract(uv.x) * 2.0 - 1.0, 1.0 - uv.y * 2.0)
        }
        // Pull a clip-space plane back through the same bias and matrices as vertex().
        trace_model_plane: fn(plane: vec4, projection: mat4x4f, bias_scale: float, model_view: mat4x4f) -> vec4 {
            let view_plane = plane * projection
            return vec4(view_plane.xyz * bias_scale, view_plane.w) * model_view
        }
        // Every pixel ray passes through the intersection of the clip X, Y and W
        // planes. Its homogeneous coordinates are their signed 3x3 minors.
        trace_model_eye: fn(projection: mat4x4f, bias_scale: float, model_view: mat4x4f) -> vec4 {
            let x = self.trace_model_plane(vec4(1.0, 0.0, 0.0, 0.0), projection, bias_scale, model_view)
            let y = self.trace_model_plane(vec4(0.0, 1.0, 0.0, 0.0), projection, bias_scale, model_view)
            let w = self.trace_model_plane(vec4(0.0, 0.0, 0.0, 1.0), projection, bias_scale, model_view)
            let xy = cross(x.xyz, y.xyz)
            let eye = vec4(-x.w * cross(y.xyz, w.xyz) - y.w * cross(w.xyz, x.xyz) - w.w * xy, dot(w.xyz, xy))
            // Choose positive W for a finite eye. A parallel view has W=0:
            // point back towards the camera, opposite trace_direction().
            if eye.w < 0.0 || (eye.w == 0.0 && (model_view * vec4(eye.xyz, 0.0)).z < 0.0) { return -eye }
            return eye
        }
        trace_direction: fn(line: vec3, model_view: mat4x4f) -> vec3 {
            let d = normalize(line)
            if (model_view * vec4(line, 0.0)).z > 0.0 { return -d }
            return d
        }
        // Interpolation identifies only the rasterized box plane. Its other two
        // coordinates can disagree between adjacent triangles at the same pixel.
        trace_raster_plane: fn(q: vec3, lo: vec3, hi: vec3) -> vec4 {
            let near_lo = abs(q - lo)
            let near_hi = abs(hi - q)
            let gap = min(near_lo, near_hi)
            if gap.x <= gap.y && gap.x <= gap.z {
                if near_lo.x < near_hi.x { return vec4(-1.0, 0.0, 0.0, -lo.x) }
                return vec4(1.0, 0.0, 0.0, hi.x)
            }
            if gap.y <= gap.z {
                if near_lo.y < near_hi.y { return vec4(0.0, -1.0, 0.0, -lo.y) }
                return vec4(0.0, 1.0, 0.0, hi.y)
            }
            if near_lo.z < near_hi.z { return vec4(0.0, 0.0, -1.0, -lo.z) }
            return vec4(0.0, 0.0, 1.0, hi.z)
        }
        // a and b contain the pixel's ray. For line = a.xyz × b.xyz and any
        // point r on it, r × line = a.w * b.xyz - b.w * a.xyz. Intersect that
        // one body-local line with the selected plane, in model units.
        trace_face_point: fn(a: vec4, b: vec4, line: vec3, face: vec4, frame: vec4) -> vec3 {
            let moment = a.w * b.xyz - b.w * a.xyz
            let offset = (face.w + dot(face.xyz, frame.xyz)) / frame.w
            return (cross(face.xyz, moment) + offset * line) / dot(face.xyz, line)
        }

        trace_ground: fn(wp: vec3, sun: vec3) -> vec4 {
            let h = max(wp.y - self.ground_y, 0.0)
            let xz = wp.xz - sun.xz * (h / max(sun.y, 0.2))
            let uv = (xz - self.lm_ground_world.xy) / max(self.lm_ground_world.zw, vec2(0.000001, 0.000001))
            let valid = self.dl_apply * step(0.000001, self.lm_ground_rect.z)
                * step(0.0, uv.x) * step(uv.x, 1.0) * step(0.0, uv.y) * step(uv.y, 1.0)
            return vec4(self.lm_ground_rect.xy + clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)) * self.lm_ground_rect.zw, valid, wp.y)
        }
        fragment: fn() {
            let face = self.trace_raster_plane(self.v_trace_p, self.v_trace_lo, self.v_trace_hi)
            if dot(face.xyz, self.v_trace_ray) >= 0.0 { discard() }
            let ndc = self.trace_ndc(self.trace_pixel.xy, self.trace_inv_viewport)
            let model_view = self.draw_pass.camera_view * (self.draw_list.view_transform * self.transform)
            let projection = self.draw_pass.camera_projection
            let bias_scale = 1.0 - self.depth_bias
            let a = self.trace_model_plane(vec4(1.0, 0.0, 0.0, -ndc.x), projection, bias_scale, model_view)
            let b = self.trace_model_plane(vec4(0.0, 1.0, 0.0, -ndc.y), projection, bias_scale, model_view)
            let line = cross(a.xyz, b.xyz)
            let d = self.trace_direction(line, model_view)
            let frame = self.v_trace_origin_scale
            let entry = self.trace_face_point(a, b, line, face, frame)
            let p = entry * frame.w - frame.xyz
            let hit = self.traced_hit(p, d)
            if hit.x < 0.0 { discard() }
            let local = entry + d * (hit.x / frame.w)
            let wp = self.transform * vec4(local, 1.0)
            let stage = self.draw_list.view_transform * wp
            let view = self.draw_pass.camera_view * stage
            let clip = self.draw_pass.camera_projection * vec4(view.xyz * (1.0 - self.depth_bias), view.w)
            self.host_cut_at(wp.xyz, -1.0)
            self.door_window_at(clip)
            self.occluder_fade_at(wp.xyz, clip)

            let hit_depth = (clip.z / clip.w) * self.trace_depth_map.x + self.trace_depth_map.y
            self.depth_out = max(self.depth_out, hit_depth)
            self.fb0 = self.trace_shade_hit(hit, p, d, wp, stage, view, clip)
        }

        trace_shade_hit: fn(hit: vec4, p: vec3, d: vec3, wp: vec4, stage: vec4, view: vec4, clip: vec4) -> vec4 {
            let n_world = normalize((self.transform * vec4(self.oct_decode(hit.yz), 0.0)).xyz)
            let n_stage = normalize((self.draw_list.view_transform * vec4(n_world, 0.0)).xyz)
            let ray_world = normalize((self.transform * vec4(d, 0.0)).xyz)
            var side = 1.0
            if dot(n_world, ray_world) > 0.0 { side = -1.0 }
            let geo = n_world * side
            let sun = normalize(self.light_dir)
            let ndl = max(dot(n_stage * side, sun), 0.0)
            let hemi = clamp(n_stage.y * side * 0.5 + 0.5, 0.0, 1.0)
            let fog = 1.0 - exp(-length(view.xyz) * self.fog_density)
            let base = self.traced_base(hit.w, p + d * hit.x)
            let albedo = self.color_adjust(self.to_scene(base.xyz), self.tint, self.color_adjust_ctl)
            let mr_raw = self.traced_mr(hit.w)
            let mr = clamp(mr_raw.xy, vec2(0.0, 0.045), vec2(1.0, 1.0))
            let voxel_ao = self.traced_ao(hit, p, d)
            let gi_pre = self.gi_receive_early(wp.xyz, geo)
            let f0 = mix(vec3(0.04, 0.04, 0.04), albedo, mr.x)
            // Restore eye distance for clustered lights. In an orthographic pass
            // the eye lies on the reference plane through the active camera.
            let eye_view = (self.draw_pass.camera_projection * vec4(0.0, 0.0, -1.0, 0.0)).w
            var distance = length(view.xyz)
            if eye_view <= 0.5 { distance = -view.z }
            let stage_units = length((self.draw_list.view_transform * vec4(ray_world, 0.0)).xyz)
            let to_eye = -ray_world * (distance / stage_units)
            let zero_uv = vec2(0.0, 0.0)
            let lit = self.shade_mr_surface_at(stage, vec4(wp.xyz, ndl), geo, to_eye,
                self.model_screen_uv(clip), fog, vec4(1.0, 1.0, 1.0, 1.0), vec2(voxel_ao, 0.0), mr_raw.zw, zero_uv,
                self.trace_ground(wp.xyz, sun), self.dl_sum_gated(wp.xyz, geo),
                self.sun_color * ndl, mix(self.sun_ground, self.sun_sky, hemi), albedo, base.w,
                mr, geo, vec3(0.0, 0.0, 0.0), f0, 1.0, self.traced_emission(hit.w), gi_pre)
            return self.surface_depth_clip_at(stage, lit)
        }
    }

    mod.draw.DrawSceneTracedInside = mod.draw.DrawSceneTraced{
        depth_out: mod.shader.fragment_depth(@less)
        cull_mode: @None
        v_trace_near: varying(vec4f)

        trace_inside_cull_box: fn(near: vec4, lo_m: vec3, hi_m: vec3, margin_m: float) -> bool {
            let n_corner = vec3(
                if near.x >= 0.0 { lo_m.x } else { hi_m.x },
                if near.y >= 0.0 { lo_m.y } else { hi_m.y },
                if near.z >= 0.0 { lo_m.z } else { hi_m.z }
            )
            let p_corner = lo_m + hi_m - n_corner
            let min_s = dot(near.xyz, n_corner) + near.w
            let max_s = dot(near.xyz, p_corner) + near.w
            let eps_s = length(near.xyz) * margin_m
            return min_s > eps_s || max_s < -eps_s
        }
        // The positive corner maximizes signed distance over the whole model-space box.
        trace_inside_outside_plane: fn(plane: vec4, lo_m: vec3, hi_m: vec3, margin_m: float) -> bool {
            let p_corner = vec3(
                if plane.x >= 0.0 { hi_m.x } else { lo_m.x },
                if plane.y >= 0.0 { hi_m.y } else { lo_m.y },
                if plane.z >= 0.0 { hi_m.z } else { lo_m.z }
            )
            let max_s = dot(plane.xyz, p_corner) + plane.w
            return max_s < -length(plane.xyz) * margin_m
        }

        vertex: fn() {
            let p = vec3(self.geom.px, self.geom.py, self.geom.pz)
            self.trace_proxy_vertex()
            let scale = self.traced_scale()
            self.v_trace_origin_scale = vec4(p * scale - self.v_trace_p, scale)
            self.v_trace_aux = unpack4u8(self.geom.ao_uv)
            let model_view = self.draw_pass.camera_view * (self.draw_list.view_transform * self.transform)
            let projection = self.draw_pass.camera_projection
            let bias_scale = 1.0 - self.depth_bias
            let near = self.trace_model_plane(vec4(0.0, 0.0, self.trace_depth_map.x, self.trace_depth_map.y),
                projection, bias_scale, model_view)
            self.v_trace_near = near
            let frame = self.v_trace_origin_scale
            let lo_m = (frame.xyz + self.v_trace_lo) / frame.w
            let hi_m = (frame.xyz + self.v_trace_hi) / frame.w
            let margin_m = 0.05 / frame.w
            if self.trace_inside_cull_box(near, lo_m, hi_m, margin_m) {
                self.vertex_pos = vec4(2.0, 2.0, 2.0, 1.0)
                return
            }
            let left = self.trace_model_plane(vec4(1.0, 0.0, 0.0, 1.0), projection, bias_scale, model_view)
            let right = self.trace_model_plane(vec4(-1.0, 0.0, 0.0, 1.0), projection, bias_scale, model_view)
            let bottom = self.trace_model_plane(vec4(0.0, 1.0, 0.0, 1.0), projection, bias_scale, model_view)
            let top = self.trace_model_plane(vec4(0.0, -1.0, 0.0, 1.0), projection, bias_scale, model_view)
            if self.trace_inside_outside_plane(left, lo_m, hi_m, margin_m)
                || self.trace_inside_outside_plane(right, lo_m, hi_m, margin_m)
                || self.trace_inside_outside_plane(bottom, lo_m, hi_m, margin_m)
                || self.trace_inside_outside_plane(top, lo_m, hi_m, margin_m) {
                self.vertex_pos = vec4(2.0, 2.0, 2.0, 1.0)
                return
            }
            let eye = self.trace_model_eye(projection, bias_scale, model_view)
            self.v_trace_ray = p * eye.w - eye.xyz
            let view = model_view * vec4(p, 1.0)
            self.vertex_pos = projection * vec4(view.xyz * bias_scale, view.w)
        }

        fragment: fn() {
            let face = self.trace_raster_plane(self.v_trace_p, self.v_trace_lo, self.v_trace_hi)
            if dot(face.xyz, self.v_trace_ray) <= 0.0 { discard() }
            let ndc = self.trace_ndc(self.trace_pixel.xy, self.trace_inv_viewport)
            let model_view = self.draw_pass.camera_view * (self.draw_list.view_transform * self.transform)
            let projection = self.draw_pass.camera_projection
            let bias_scale = 1.0 - self.depth_bias
            let a = self.trace_model_plane(vec4(1.0, 0.0, 0.0, -ndc.x), projection, bias_scale, model_view)
            let b = self.trace_model_plane(vec4(0.0, 1.0, 0.0, -ndc.y), projection, bias_scale, model_view)
            let line = cross(a.xyz, b.xyz)
            let d = self.trace_direction(line, model_view)
            let frame = self.v_trace_origin_scale
            let near = self.v_trace_near
            let moment = a.w * b.xyz - b.w * a.xyz
            let denom = dot(near.xyz, line)
            if abs(denom) <= 1.0e-12 { discard() }
            let near_local = (cross(near.xyz, moment) - near.w * line) / denom
            let p_near = near_local * frame.w - frame.xyz
            let lo = self.v_trace_lo
            let hi = self.v_trace_hi
            let sx = self.trace_slab(p_near.x, d.x, lo.x, hi.x)
            let sy = self.trace_slab(p_near.y, d.y, lo.y, hi.y)
            let sz = self.trace_slab(p_near.z, d.z, lo.z, hi.z)
            let t_enter = max(sx.x, max(sy.x, sz.x))
            let t_exit = min(sx.y, min(sy.y, sz.y))
            if t_enter > 0.002 || t_exit <= 0.0 || t_enter >= t_exit { discard() }
            let hit = self.traced_hit_interval(p_near, d, max(t_enter, 0.0), t_exit)
            if hit.x < 0.0 { discard() }
            let local = near_local + d * (hit.x / frame.w)
            let wp = self.transform * vec4(local, 1.0)
            let stage = self.draw_list.view_transform * wp
            let view = self.draw_pass.camera_view * stage
            let clip = self.draw_pass.camera_projection * vec4(view.xyz * (1.0 - self.depth_bias), view.w)
            self.host_cut_at(wp.xyz, -1.0)
            self.door_window_at(clip)
            self.occluder_fade_at(wp.xyz, clip)
            let hit_depth = (clip.z / clip.w) * self.trace_depth_map.x + self.trace_depth_map.y
            self.depth_out = min(self.depth_out, max(hit_depth, 0.0))
            self.fb0 = self.trace_shade_hit(hit, p_near, d, wp, stage, view, clip)
        }
    }
}
