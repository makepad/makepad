//! DrawSceneGrass: blades grown from the grass field (grass.rs).

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // DrawScenePbr's sibling (the lane binding, cascades, fog and GI are
    // inherited); its own vertex stage places the patch's blades on the
    // field and its own pixel lights them. The height raster rides slot 0
    // (`tex`) and the cover raster slot 1 (`ao_map`): the blades never use
    // an albedo map or a baked AO atlas, and the model shaders have no free
    // texture slot left. No discard anywhere: the blades are opaque
    // geometry and the tile GPU keeps its hidden-surface removal.
    //
    // Vertex lanes (grass.rs patch_vertices): px, pz = root in the unit
    // patch, py = t along the blade, nrm = side (-1, 0 tip, 1), uv = the
    // keep threshold, color = shape random, ao_uv = yaw random.
    mod.draw.DrawSceneGrass = mod.std.set_type_default() do #(DrawSceneGrass::script_shader(vm)){
        ..mod.draw.DrawScenePbr
        // The field rides the morph weight lanes (the blades never morph;
        // the PBR stream has no room for a new lane, its uniform block no
        // room for a uniform): 0 = origin x, z, 1 / width, 1 / depth;
        // 1 = texels x, z, cell metres, height min; 2 = blade height, width,
        // radius, wind; 3, 4 = sRGB lush and dry blades (4.w = height
        // range); 5 = eye xyz, wind clock;
        // 6 = this patch's corner x, z and side (w).

        // A height texel: 16 bits, hi byte in G and lo byte in A (grass.rs),
        // over [morph_weights1.w, + morph_weights4.w].
        grass_h: fn(t: vec4) -> float {
            let q = floor(t.y * 255.0 + 0.5) * 256.0 + floor(t.w * 255.0 + 0.5)
            return self.morph_weights1.w + q / 65535.0 * self.morph_weights4.w
        }

        // The ground under a root: bilinear over the four texels around it
        // (point reads: the 16-bit height is two bytes).
        grass_ground: fn(xz: vec2) -> float {
            let g = (xz - self.morph_weights0.xy) / self.morph_weights1.z - vec2(0.5, 0.5)
            let i = floor(g)
            let f = g - i
            let inv = vec2(1.0 / self.morph_weights1.x, 1.0 / self.morph_weights1.y)
            let uv = (i + vec2(0.5, 0.5)) * inv
            let h00 = self.grass_h(self.tex.sample_nearest(uv, 0.0))
            let h10 = self.grass_h(self.tex.sample_nearest(uv + vec2(inv.x, 0.0), 0.0))
            let h01 = self.grass_h(self.tex.sample_nearest(uv + vec2(0.0, inv.y), 0.0))
            let h11 = self.grass_h(self.tex.sample_nearest(uv + inv, 0.0))
            return mix(mix(h00, h10, f.x), mix(h01, h11, f.x), f.y)
        }

        vertex: fn() {
            let unit = vec2(self.geom.px, self.geom.pz)
            let t = self.geom.py
            let side = self.geom.nrm
            let keep = self.geom.uv
            let shape = self.geom.color
            let yaw = self.geom.ao_uv * 6.2831853
            let root = self.morph_weights6.xy + unit * self.morph_weights6.w
            let cover = self.ao_map.sample_nearest((root - self.morph_weights0.xy) * self.morph_weights0.zw, 0.0)
            let radius = self.morph_weights2.z
            let d = length(root - self.morph_weights5.xz) / radius
            // Blades that survive at this distance: every one close in, then
            // half every 0.3 radius (the ring geometries hold exactly those),
            // and none past the radius.
            let fade = exp2(0.0 - max(d - 0.1, 0.0) / 0.3) * (1.0 - smoothstep(0.8, 1.0, d))
            if keep >= fade * cover.x || cover.x < 0.03 {
                // Outside clip space: the whole blade rasterises nothing.
                self.vertex_pos = vec4(0.0, 0.0, 2.0, 1.0)
                return
            }
            // Short where the cover is thin, and sinking into the ground
            // toward the radius so the terrain's own grass takes over.
            let h = self.morph_weights2.x * (0.55 + 0.7 * shape) * (0.45 + 0.55 * cover.x) * (1.0 - smoothstep(0.55, 1.0, d) * 0.85)
            let across = vec2(cos(yaw), sin(yaw))
            let lean_dir = vec2(cos(yaw * 1.7 + 1.3), sin(yaw * 1.7 + 1.3))
            let clock = self.morph_weights5.w
            let gust = sin(clock * 1.7 + root.x * 0.31 + root.y * 0.23) * 0.5 + 0.5
            let flick = sin(clock * 5.3 + shape * 40.0) * 0.12
            let wind = vec2(0.8, 0.6) * ((0.25 + 0.75 * gust) * self.morph_weights2.w + flick * self.morph_weights2.w)
            let bend = (lean_dir * (0.18 + 0.3 * shape) + wind * 0.55) * (t * t * h)
            let width = self.morph_weights2.y * (0.7 + 0.6 * shape) * (1.0 - 0.8 * t)
            let ground = self.grass_ground(root)
            let pos = vec3(
                root.x + across.x * side * width * 0.5 + bend.x,
                ground + t * h - length(bend) * t * 0.35,
                root.y + across.y * side * width * 0.5 + bend.y
            )
            // The blade faces across its width; half its normal is bent up so
            // a meadow shades as a surface rather than as needles.
            let face = normalize(vec3(0.0 - across.y, 0.0, across.x))
            let to_eye = self.morph_weights5.xyz - pos
            let facing = face * sign(dot(face, to_eye) + 0.0001)
            let n = normalize(facing + vec3(0.0, 1.1, 0.0) + vec3(bend.x, 0.0, bend.y) * 0.5)
            self.world = self.draw_list.view_transform * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let l = normalize(self.light_dir)
            self.v_csm = vec4(pos.x, pos.y, pos.z, max(dot(n, l), 0.0))
            self.v_csm_n = n
            // Root dark (self-shadowed thatch), tip lighter and drier.
            let col = mix(self.morph_weights3.xyz, self.morph_weights4.xyz, clamp(cover.y + t * 0.25 * shape, 0.0, 1.0))
            self.v_tint = vec4(col * mix(0.42, 1.12, t) * (0.82 + 0.36 * shape), t)
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            let clip_out = self.draw_pass.camera_projection * view_pos
            self.v_spos = clip_out
            self.vertex_pos = clip_out
        }

        pixel: fn() {
            let wp = self.v_csm.xyz
            let n = normalize(self.v_csm_n)
            let l = normalize(self.light_dir)
            let v = normalize(self.eye.xyz - wp)
            let ndl = max(dot(n, l), 0.0)
            let vis = self.csm_vis_fast(wp, n, ndl)
            let albedo = self.to_scene(self.v_tint.xyz)
            let occ = mix(0.35, 1.0, self.v_tint.w)
            let ambient = self.gi_ambient(wp, n, mix(self.sun_ground, self.sun_sky, clamp(n.y * 0.5 + 0.5, 0.0, 1.0))) * occ
            // Sun through the blade toward the eye: a thin leaf glows.
            let back = max(dot(v * (0.0 - 1.0), l), 0.0)
            let through = back * back * back * 0.45 * self.v_tint.w
            let lit = albedo * (ambient + self.sun_color * ((ndl * 0.85 + 0.15) * vis + through * vis))
            return self.csm_debug_view(self.gi_display(vec4(self.scene_fogged(self.to_display(lit), self.v_fog, wp, self.fog_density), 1.0), wp, n), wp, n)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}
