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
    //
    // The trampled field (crate::trample) rides `emissive_map` (the blades
    // never read an emissive map): B how flat, R and G the way pushed (128
    // at rest), over `grass_trample` (corner x, z, 1 / side; 0: off).
    //
    // Flowers (grass.rs flower_vertices) ride the same lanes, marked by a
    // yaw lane past 2: py is t up the stem (0..1), or the head's vertex (2
    // the centre, 3, 4, 5 a petal's left base, tip and right base) with the
    // petal's index in nrm. A draw holds only blades or only flowers, so the
    // split never diverges inside a draw.
    mod.draw.DrawSceneGrass = mod.std.set_type_default() do #(DrawSceneGrass::script_shader(vm)){
        ..mod.draw.DrawScenePbr
        // A blade is no model's own layer: every caster shadows it.
        csm_own_limit: fn(ci: float) -> float {
            return 2.0
        }
        // The field rides the morph weight lanes (the blades never morph;
        // the PBR stream has no room for a new lane): 0 = origin x, z,
        // 1 / width, 1 / depth; 1 = texels x, z, cell metres;
        // 2 = blade height, width, reach, wind; 3, 4 = sRGB lush and dry
        // blades; 5 = eye xyz; 6 = this patch's corner
        // x, z and side (w). Per draw: the world wind (SceneWind) and the
        // screen's density (grass.rs GrassLod): metres every blade grows out
        // to, the reach, the widest a thinned blade grows (authored widths).
        wind_state: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wind_scroll: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        grass_lod: uniform(vec4(1.0, 1.0, 1.0, 0.0))
        grass_trample: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // The flowers (grass.rs FlowerKind, GrassLod::flowers): their
        // screen density (every head out to x metres, the reach, the widest
        // a thinned head grows, the kinds); the kinds' cumulative share of
        // the flowers, their stem heights, and per kind the petals' sRGB and
        // head size, and the eye's sRGB and cup.
        grass_flower_lod: uniform(vec4(1.0, 1.0, 1.0, 0.0))
        grass_flower_cum: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        grass_flower_h: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        flower_petal0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        flower_petal1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        flower_petal2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        flower_petal3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        flower_eye0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        flower_eye1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        flower_eye2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        flower_eye3: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // The ground under a root: bilinear over the four texels around it
        // (point reads: a device may not filter a float texture).
        grass_ground: fn(xz: vec2) -> float {
            let g = (xz - self.morph_weights0.xy) / self.morph_weights1.z - vec2(0.5, 0.5)
            let i = floor(g)
            let f = g - i
            let inv = vec2(1.0 / self.morph_weights1.x, 1.0 / self.morph_weights1.y)
            let uv = (i + vec2(0.5, 0.5)) * inv
            let h00 = self.tex.sample_nearest(uv, 0.0).x
            let h10 = self.tex.sample_nearest(uv + vec2(inv.x, 0.0), 0.0).x
            let h01 = self.tex.sample_nearest(uv + vec2(0.0, inv.y), 0.0).x
            let h11 = self.tex.sample_nearest(uv + inv, 0.0).x
            return mix(mix(h00, h10, f.x), mix(h01, h11, f.x), f.y)
        }

        // One clump cell (cells of CLUMP metres, in cell units): its centre,
        // jittered by up to 0.4 cell (no rows of tufts), if it is nearer `g` than `best`
        // (centre x, z, squared distance, the cell's random).
        grass_cell: fn(c: vec2, g: vec2, best: vec4) -> vec4 {
            let h = self.wind_hash(c + vec2(0.13, 0.57))
            let centre = c + vec2(0.5, 0.5) + (vec2(h, self.wind_hash(c + vec2(5.27, 1.91))) - vec2(0.5, 0.5)) * 0.8
            let d = dot(g - centre, g - centre)
            if d < best.z {
                return vec4(centre.x, centre.y, d, h)
            }
            return best
        }

        // Pressed by a tyre or a foot (crate::trample) at a root: the way it
        // was pushed (xy, -1..1) and how flat (z), faded out over the
        // field's last 8 %.
        grass_press: fn(root: vec2) -> vec3 {
            let tr = self.grass_trample
            if tr.z > 0.0 {
                let tuv = (root - tr.xy) * tr.z
                let edge = min(min(tuv.x, 1.0 - tuv.x), min(tuv.y, 1.0 - tuv.y))
                if edge > 0.0 {
                    let pressed = self.emissive_map.sample_nearest(tuv, 0.0)
                    let tread = (pressed.xy * 255.0 - vec2(128.0, 128.0)) / 127.0
                    return vec3(tread.x, tread.y, pressed.z * smoothstep(0.0, 0.08, edge))
                }
            }
            return vec3(0.0, 0.0, 0.0)
        }

        vertex: fn() {
            let t = self.geom.py
            let side = self.geom.nrm
            let keep = self.geom.uv
            let shape = self.geom.color
            let yaw = self.geom.ao_uv * 6.2831853
            // Every patch shares one root pattern; each turns and mirrors it
            // its own way (one of 8, by its corner), so it has no 2 m period.
            let corner = self.morph_weights6.xy
            let o = floor(self.wind_hash(corner * 0.5 + vec2(0.37, 0.71)) * 7.999)
            let m = step(3.5, o)
            let r = o - 4.0 * m
            var u = vec2(mix(self.geom.px, 1.0 - self.geom.px, m), self.geom.pz)
            u = mix(u, vec2(u.y, u.x), r - 2.0 * floor(r * 0.5))
            u = vec2(mix(u.x, 1.0 - u.x, step(0.5, r) * step(r, 2.5)), mix(u.y, 1.0 - u.y, step(1.5, r)))
            let seed = corner + u * self.morph_weights6.w
            // Clumps (0.7 m cells, the nearest of the four jittered centres
            // around): the roots draw together toward their clump's centre,
            // and a clump's blades share their height, colour and density
            // and splay out from its middle, so the meadow grows in tufts.
            let g = seed * (1.0 / 0.7)
            let b = floor(g - vec2(0.5, 0.5))
            var cl = self.grass_cell(b, g, vec4(0.0, 0.0, 1.0e9, 0.0))
            cl = self.grass_cell(b + vec2(1.0, 0.0), g, cl)
            cl = self.grass_cell(b + vec2(0.0, 1.0), g, cl)
            cl = self.grass_cell(b + vec2(1.0, 1.0), g, cl)
            let centre = vec2(cl.x, cl.y) * 0.7
            let tuft = self.wind_hash(vec2(cl.x, cl.y) * 3.7 + vec2(11.3, 2.9))
            let root = mix(seed, centre, 0.2)
            let cover = self.ao_map.sample_nearest((root - self.morph_weights0.xy) * self.morph_weights0.zw, 0.0)
            let ground = self.grass_ground(root)
            // The screen's density: every blade out to grass_lod.x metres,
            // then the square of (grass_lod.x / distance) of them, widened to
            // keep the cover; the last ones sink at the reach.
            let lod = self.grass_lod
            let dist = length(vec3(root.x, ground, root.y) - self.morph_weights5.xyz)
            if self.geom.ao_uv > 1.5 {
                // A flower: kept by the bloom and its own screen density
                // (the heads' size), grown out of the ground over the last
                // 15% of its keep margin like a blade.
                let fl = self.grass_flower_lod
                let near_f = fl.x / max(dist, 0.001)
                let kept_f = min(near_f * near_f, 1.0)
                let bloom = cover.z * kept_f * (1.0 - smoothstep(fl.y * 0.8, fl.y, dist))
                let margin_f = 1.0 - keep / max(bloom, 0.0001)
                if margin_f <= 0.0 || cover.x < 0.03 {
                    self.vertex_pos = vec4(0.0, 0.0, 2.0, 1.0)
                    return
                }
                let grow_f = smoothstep(0.0, 0.15, margin_f)
                // Its kind, by its own random against the kinds' shares.
                let pick = fract(shape * 7.31 + 0.17)
                let cum = self.grass_flower_cum
                var petal_k = self.flower_petal3
                var eye_k = self.flower_eye3
                var tall = self.grass_flower_h.w
                if pick < cum.z {
                    petal_k = self.flower_petal2
                    eye_k = self.flower_eye2
                    tall = self.grass_flower_h.z
                }
                if pick < cum.y {
                    petal_k = self.flower_petal1
                    eye_k = self.flower_eye1
                    tall = self.grass_flower_h.y
                }
                if pick < cum.x {
                    petal_k = self.flower_petal0
                    eye_k = self.flower_eye0
                    tall = self.grass_flower_h.x
                }
                // The stem stands taller than a blade and nearly upright,
                // bends in the world wind and lies down where it was
                // pressed, as the blades around it do.
                let h_f = tall * (0.8 + 0.4 * shape) * (0.6 + 0.4 * cover.x) * grow_f
                let press_f = self.grass_press(root)
                let lay_f = press_f.z
                let gust_f = self.scene_wind(root, self.wind_state, self.wind_scroll)
                let wind_f = self.wind_push(gust_f.xy) * ((1.0 + self.wind_wave(self.wind_state.w, 216.0, shape * 6.37) * 0.12) * self.morph_weights2.w * (1.0 - 0.85 * lay_f))
                let splay_f = (root - centre) * 0.6 + vec2(cos(yaw * 1.7 + 1.3), sin(yaw * 1.7 + 1.3)) * 0.15
                let lean_f = splay_f * 0.3 + vec2(press_f.x, press_f.y) * (3.0 * lay_f) + wind_f * 0.45
                let upright_f = 1.0 / sqrt(1.0 + dot(lean_f, lean_f))
                let role = self.geom.py
                let st = min(role, 1.0)
                let bend_f = lean_f * (upright_f * st * st * h_f)
                let stem_f = vec3(root.x + bend_f.x, ground + st * h_f * (1.0 - st * (1.0 - upright_f)), root.y + bend_f.y)
                // The head faces up, tipped with the stem and a little
                // toward the sun (a field's heads turn one way), and on its
                // own a little; six petals around an eye, rising by the
                // kind's cup. A thinned head widens by the root of its
                // thinning (it stays as wide on screen as at grass_flower_lod.x
                // metres), so the heads keep their share of the view.
                let sun_f = normalize(self.light_dir)
                let tilt_f = vec2(cos(yaw * 2.3), sin(yaw * 2.3)) * 0.25
                let up_f = normalize(vec3(lean_f.x * 1.2 + sun_f.x * 0.35 + tilt_f.x, 1.0, lean_f.y * 1.2 + sun_f.z * 0.35 + tilt_f.y))
                let e0_f = vec3(cos(yaw), 0.0, sin(yaw))
                let tx_f = normalize(e0_f - up_f * dot(e0_f, up_f))
                let tz_f = cross(up_f, tx_f)
                let is_head = step(1.5, role)
                let is_petal = step(2.5, role)
                let is_tip = step(3.5, role) - step(4.5, role)
                let side_off = (step(4.5, role) - (step(2.5, role) - step(3.5, role))) * 0.42
                let ang = self.geom.nrm * 1.0471976 + yaw * 3.0 + side_off
                let dir_f = tx_f * cos(ang) + tz_f * sin(ang)
                let radius_f = petal_k.w * 0.5 * clamp(dist / fl.x, 1.0, fl.z) * grow_f
                let cup_f = eye_k.w
                let r_f = is_petal * mix(0.32, 1.0, is_tip)
                let rise_f = mix(0.18, cup_f * mix(0.15, 1.0, is_tip), is_petal)
                let head_f = stem_f + (dir_f * r_f + up_f * rise_f) * radius_f
                let across_f = vec3(cos(yaw), 0.0, sin(yaw))
                let stem_w = petal_k.w * 0.1 * grow_f
                let pos_f = mix(stem_f + across_f * (side * stem_w * 0.5), head_f, is_head)
                // The bowl's inside faces up and in; the stem faces across.
                let n_head = normalize(up_f - dir_f * (cup_f * 0.8 * is_petal))
                let n_stem = normalize(vec3(0.0 - across_f.z, 0.3, across_f.x))
                let to_eye_f = self.morph_weights5.xyz - pos_f
                let n_f0 = mix(n_stem, n_head, is_head)
                let n_f = n_f0 * sign(dot(n_f0, to_eye_f) + 0.0001)
                self.world = self.draw_list.view_transform * vec4(pos_f.x, pos_f.y, pos_f.z, 1.0)
                let view_f = self.draw_pass.camera_view * self.world
                self.v_csm = vec4(pos_f.x, pos_f.y, pos_f.z, max(dot(n_f, sun_f), 0.0))
                self.v_csm_n = n_f
                // The eye in the middle, the petals' colour out to their
                // tips (lighter there); a green stem. The head takes the
                // full light of a blade's tip: the sun glows through a
                // petal as through a leaf.
                let petal_col = mix(mix(eye_k.xyz, petal_k.xyz, 0.55), petal_k.xyz * 1.05, is_tip)
                let head_col = mix(eye_k.xyz, petal_col, is_petal)
                let stem_col = self.morph_weights3.xyz * mix(0.5, 0.9, st)
                let lift_f = 1.0 + (gust_f.z - 0.5) * 0.12 * self.morph_weights2.w
                self.v_tint = vec4(mix(stem_col, head_col, is_head) * (0.9 + 0.2 * shape) * lift_f, mix(st * 0.8, 1.0, is_head))
                self.v_fog = 1.0 - exp(0.0 - length(view_f.xyz) * self.fog_density)
                let clip_f = self.draw_pass.camera_projection * view_f
                self.v_spos = clip_f
                self.vertex_pos = clip_f
                return
            }
            let near = lod.x / max(dist, 0.001)
            let kept = min(near * near, 1.0)
            let fade = 1.0 - smoothstep(lod.y * 0.8, lod.y, dist)
            let density = cover.x * mix(0.55, 1.0, tuft) * kept * fade
            // A blade the shader starts to keep grows out of the ground over
            // the last 15% of its keep margin, so a moving camera never pops
            // one in.
            let margin = 1.0 - keep / max(density, 0.0001)
            if margin <= 0.0 || cover.x < 0.03 {
                // Outside clip space: the whole blade rasterises nothing.
                self.vertex_pos = vec4(0.0, 0.0, 2.0, 1.0)
                return
            }
            let grow = smoothstep(0.0, 0.15, margin)
            // Patches of the meadow (~12 m): drier and greener stretches.
            let patch = self.wind_noise(root * 0.083 + vec2(31.0, 17.0))
            // Short where the cover is thin, and sinking into the ground
            // toward the reach, where the terrain's carpet of the same
            // grass takes over (shaders/splat.rs `grass_carpet`).
            let h = self.morph_weights2.x * (0.55 + 0.7 * shape) * (0.45 + 0.55 * cover.x) * mix(0.7, 1.25, cl.w) * grow * (1.0 - smoothstep(0.55, 1.0, dist / lod.y) * 0.85)
            let across = vec2(cos(yaw), sin(yaw))
            // Splayed out from the clump's middle, plus the blade's own way.
            let splay = (root - centre) * 2.2 + vec2(cos(yaw * 1.7 + 1.3), sin(yaw * 1.7 + 1.3)) * 0.45
            // The world wind at the root: gusts cross the meadow as waves of
            // bending blades, each blade flicking on its own phase.
            let gust = self.scene_wind(root, self.wind_state, self.wind_scroll)
            let push = self.wind_push(gust.xy)
            let flick = self.wind_wave(self.wind_state.w, 216.0, shape * 6.37) * 0.12
            // Pressed by a tyre or a foot (crate::trample): laid over the
            // way it was pushed and splayed, out of the wind, paler and
            // catching more light (its face turned up), standing up again as
            // the field recovers. The read fades out over the field's last
            // 8 %.
            let press = self.grass_press(root)
            let lay = press.z
            let tread = vec2(press.x, press.y)
            let wind = push * ((1.0 + flick) * self.morph_weights2.w * (1.0 - 0.85 * lay))
            // The blade keeps its length: its tip turns from upright by
            // atan(|lean|) and the blade arcs there, so a gale lays it over
            // instead of stretching it.
            let lean = splay * (0.2 + 0.35 * shape) * (1.0 + 2.5 * lay) + tread * (3.0 * lay) + wind * 0.55
            let upright = 1.0 / sqrt(1.0 + dot(lean, lean))
            let bend = lean * (upright * t * t * h)
            // Thinned blades widen to keep the cover (up to grass_lod.z).
            let width = self.morph_weights2.y * (0.7 + 0.6 * shape) * (1.0 - 0.8 * t) * grow * min(1.0 / kept, lod.z)
            let pos = vec3(
                root.x + across.x * side * width * 0.5 + bend.x,
                ground + t * h * (1.0 - t * (1.0 - upright)),
                root.y + across.y * side * width * 0.5 + bend.y
            )
            // The blade faces across its width and is rounded across it (its
            // edges' normals turn outward); half its normal is bent up and it
            // tilts with the bend, so a meadow shades as a surface and the
            // sheen rolls with the wind.
            let face = normalize(vec3(0.0 - across.y, 0.0, across.x))
            let to_eye = self.morph_weights5.xyz - pos
            let facing = face * sign(dot(face, to_eye) + 0.0001)
            let n = normalize(facing + vec3(across.x, 0.0, across.y) * (side * 0.55) + vec3(0.0, 0.9 + 1.5 * lay, 0.0) + vec3(lean.x, 0.0, lean.y) * (t * 0.8))
            self.world = self.draw_list.view_transform * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let l = normalize(self.light_dir)
            self.v_csm = vec4(pos.x, pos.y, pos.z, max(dot(n, l), 0.0))
            self.v_csm_n = n
            // Root dark (the thatch shades it, more where the clump is
            // thick), tip lighter and drier; a clump's and a blade's own hue
            // drift on top (yellower or bluer greens); a gust lifts the
            // meadow's light a little as it passes.
            let dry = clamp(cover.y + (patch - 0.5) * 0.7 + (cl.w - 0.5) * 0.3 + t * 0.25 * shape + lay * 0.25, 0.0, 1.0)
            let col = mix(self.morph_weights3.xyz, self.morph_weights4.xyz, dry)
            let hue = (shape - 0.5) * 0.1 + (tuft - 0.5) * 0.1 + (patch - 0.5) * 0.08
            let tinted = col * vec3(1.0 + hue, 1.0, 1.0 - hue * 1.5)
            let root_dark = mix(0.55, 0.3, cover.x * tuft)
            let lift = 1.0 + (gust.z - 0.5) * 0.12 * self.morph_weights2.w
            self.v_tint = vec4(tinted * mix(root_dark, 1.12, t) * (0.84 + 0.32 * shape) * lift * (1.0 + 0.12 * lay), t)
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
            // A waxy blade's sheen, strongest toward the tip.
            let hv = normalize(l + v)
            let sheen = pow(max(dot(n, hv), 0.0), 24.0) * 0.16 * self.v_tint.w
            let lit = albedo * (ambient + self.sun_color * ((ndl * 0.85 + 0.15) * vis + through * vis)) + self.sun_color * (sheen * vis)
            return self.csm_debug_view(self.gi_display(vec4(self.scene_fogged(self.to_display(lit), self.v_fog, wp, self.fog_density), 1.0), wp, n), wp, n)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}
