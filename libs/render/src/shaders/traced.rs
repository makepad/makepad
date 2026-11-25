//! Proxy rays shared by traced passes, and the outside surface pass.

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
        backface_culling: false
        depth_out: mod.shader.fragment_depth(@greater)
        trace_depth_map: uniform(vec2(1.0, 0.0))
        trace_volume: texture_3d(float)
        trace_palette: texture_2d(float)
        v_trace_local_scale: varying(vec4f)
        v_trace_aux: varying(vec4f)

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
        traced_base: fn(material: float, p: vec3) -> vec4 { return vec4(1.0, 1.0, 1.0, 1.0) }
        traced_mr: fn(material: float) -> vec4 { return vec4(0.0, 0.5, 0.0, 0.0) }
        traced_emission: fn(material: float) -> vec3 { return vec3(0.0, 0.0, 0.0) }

        csm_own_limit: fn(ci: float) -> float { return 2.0 }

        vertex: fn() {
            let p = vec3(self.geom.px, self.geom.py, self.geom.pz)
            self.trace_proxy_vertex()
            self.v_trace_local_scale = vec4(p, self.traced_scale())
            self.v_trace_aux = unpack4u8(self.geom.ao_uv)
            let basis = self.draw_list.view_transform * self.transform
            let world = basis * vec4(p, 1.0)
            let eye = (self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)).xyz
            var ray = world.xyz - eye
            if (self.draw_pass.camera_projection * vec4(0.0, 0.0, -1.0, 0.0)).w <= 0.5 {
                ray = (self.draw_pass.camera_inv * vec4(0.0, 0.0, -1.0, 0.0)).xyz
            }
            self.v_trace_ray = self.trace_local_ray(basis, ray)
            let view = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * vec4(view.xyz * (1.0 - self.depth_bias), view.w)
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
            let p = self.v_trace_p
            let d = normalize(self.v_trace_ray)
            if !self.trace_entry_face(p, d) { discard() }
            let hit = self.traced_hit(p, d)
            if hit.x < 0.0 { discard() }
            let local = self.v_trace_local_scale.xyz + d * (hit.x / self.v_trace_local_scale.w)
            let wp = self.transform * vec4(local, 1.0)
            let stage = self.draw_list.view_transform * wp
            let view = self.draw_pass.camera_view * stage
            let clip = self.draw_pass.camera_projection * vec4(view.xyz * (1.0 - self.depth_bias), view.w)
            self.host_cut_at(wp.xyz, -1.0)
            self.door_window_at(clip)
            self.occluder_fade_at(wp.xyz, clip)

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
            let mr = clamp(self.traced_mr(hit.w).xy, vec2(0.0, 0.045), vec2(1.0, 1.0))
            let f0 = mix(vec3(0.04, 0.04, 0.04), albedo, mr.x)
            // Restore eye distance for clustered lights. In an orthographic pass
            // the eye lies on the reference plane through the active camera.
            let eye_view = (self.draw_pass.camera_projection * vec4(0.0, 0.0, -1.0, 0.0)).w
            var distance = length(view.xyz)
            if eye_view <= 0.5 { distance = -view.z }
            let stage_units = length((self.draw_list.view_transform * vec4(ray_world, 0.0)).xyz)
            let to_eye = -ray_world * (distance / stage_units)
            let zero_uv = vec2(0.0, 0.0)
            let hit_depth = (clip.z / clip.w) * self.trace_depth_map.x + self.trace_depth_map.y
            self.depth_out = max(self.depth_out, hit_depth)
            let lit = self.shade_mr_surface_at(stage, vec4(wp.xyz, ndl), geo, to_eye,
                self.model_screen_uv(clip), fog, vec4(1.0, 1.0, 1.0, 1.0), zero_uv, zero_uv, zero_uv,
                self.trace_ground(wp.xyz, sun), self.dl_sum_gated(wp.xyz, geo),
                self.sun_color * ndl, mix(self.sun_ground, self.sun_sky, hemi), albedo, base.w,
                mr, geo, vec3(0.0, 0.0, 0.0), f0, 1.0, self.traced_emission(hit.w),
                vec4(0.0, 0.0, 0.0, 1.0))
            self.fb0 = self.surface_depth_clip_at(stage, lit)
        }
    }
}
