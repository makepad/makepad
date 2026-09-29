//! Sky domes: authored gradient, analytic (Preetham) and map sky surfaces.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // Sky dome: a big cube around the camera, gradient by view direction
    // (the Godot ProceduralSkyMaterial look).
    mod.draw.DrawSceneSky = mod.std.set_type_default() do #(DrawSceneSky::script_shader(vm)){
        ..mod.draw.DrawCube,
        // DELIBERATE: the sky is a cube the camera sits INSIDE, so every
        // visible face is a back face. Culling erases the sky completely.
        backface_culling: false
        v_dir: varying(vec3f)

        vertex: fn() {
            let pos = self.get_size() * self.geom.geom_pos + self.get_pos()
            let model_view = self.draw_list.view_transform * self.transform
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            self.v_dir = self.geom.geom_pos
            let view_pos = self.draw_pass.camera_view * self.world
            let clip = self.draw_pass.camera_projection * view_pos
            // Pin the sky to the far plane (z ~= w) — the skybox trick Godot's
            // background pass amounts to: the dome never clips against the far
            // plane no matter its world size, and everything else wins depth.
            self.vertex_pos = vec4(clip.x, clip.y, clip.w * 0.99995, clip.w)
        }

        pixel: fn() {
            let v = normalize(self.v_dir)
            let y = v.y
            let up = clamp(y * 2.2, 0.0, 1.0)
            let down = clamp((0.0 - y) * 2.2, 0.0, 1.0)
            let sky = mix(self.sky_horizon, self.sky_top, up)
            let ground = mix(self.sky_ground, self.sky_bottom, down)
            // A short blend across the horizon instead of a hard step: where
            // the terrain ends short of the horizon the dome's ground band
            // no longer meets the sky on a knife edge.
            let color = mix(ground, sky, smoothstep(-0.03, 0.03, y))
            // Dome-anchored hash dither (same idiom as the world shaders'
            // AO dither): a shallow gradient over 800 world units lands as
            // visible 8-bit bands otherwise. ±0.4% ~= ±1 LSB.
            let hash = fract(
                sin(dot(v.xy + v.zz, vec2(12.9898, 78.233))) * 43758.5453
            )
            return vec4(color + vec3(1.0, 1.0, 1.0) * ((hash - 0.5) * 0.008), 1.0)
        }
    }

    // The analytic (Preetham) daylight sky — a SIBLING of DrawSceneSky
    // rather than a branch inside it: the combined pixel fn sat exactly at
    // a script-shader capacity limit where one more statement silently
    // broke the whole shader, and the two skies never draw together
    // anyway. DrawSceneSky keeps the authored-gradient path; this one
    // carries Preetham + the setting sun disc + the night star dome.
    mod.draw.DrawSceneSkyAnalytic = mod.std.set_type_default() do #(DrawSceneSkyAnalytic::script_shader(vm)){
        ..mod.draw.DrawCube,
        // Same deliberate choice as DrawSceneSky: the camera sits INSIDE
        // the dome, every visible face is a back face.
        backface_culling: false
        // Night-sky panorama (equirectangular; NASA SVS Deep Star Map —
        // see the sandbox's resources/sky/ATTRIBUTION.txt). A 1x1 black
        // stand-in binds when no map is loaded; star_r0.w gates it.
        star_tex: texture_2d(float)
        v_dir: varying(vec3f)
        // HDR lane: the sky takes the same height fog as the world, so the
        // horizon has no seam. sky_fog = (density at base height x scale
        // height x layer above the eye, 0, 0, on); fog colour in rgb.
        sky_fog: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        sky_fog_color: uniform(vec3(0.0, 0.0, 0.0))

        vertex: fn() {
            let pos = self.get_size() * self.geom.geom_pos + self.get_pos()
            let model_view = self.draw_list.view_transform * self.transform
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            self.v_dir = self.geom.geom_pos
            let view_pos = self.draw_pass.camera_view * self.world
            let clip = self.draw_pass.camera_projection * view_pos
            // Pin to the far plane (z ~= w) — same skybox trick as
            // DrawSceneSky: never clips against the far plane, everything
            // else wins depth.
            self.vertex_pos = vec4(clip.x, clip.y, clip.w * 0.99995, clip.w)
        }

        pixel: fn() {
            let v = normalize(self.v_dir)
            // The established engine sky. The sun-dependent half is prepared
            // once in sky.rs; this evaluates the per-pixel Perez product.
            let ct = max(v.y, 0.01)
            let cg = clamp(dot(v, self.sun_e.xyz), 0.0 - 1.0, 1.0)
            let g = acos(cg)
            let cg2 = cg * cg
            let fy = (1.0 + self.pz_y.x * exp(self.pz_y.y / ct))
                * (1.0 + self.pz_y.z * exp(self.pz_y.w * g) + self.pz_e.x * cg2)
            let fx = (1.0 + self.pz_x.x * exp(self.pz_x.y / ct))
                * (1.0 + self.pz_x.z * exp(self.pz_x.w * g) + self.pz_e.y * cg2)
            let fc = (1.0 + self.pz_yc.x * exp(self.pz_yc.y / ct))
                * (1.0 + self.pz_yc.z * exp(self.pz_yc.w * g) + self.pz_e.z * cg2)
            let yl = self.zenith.x * fy * self.pz_f0.x
            let xc = self.zenith.y * fx * self.pz_f0.y
            let yc = max(self.zenith.z * fc * self.pz_f0.z, 0.0001)
            // sun_true.w = 1: the HDR lane wants linear radiance (the
            // composite tone maps), so no Reinhard, normalise or gamma.
            // (w is the HDR sky gain, 0 in the legacy lane.)
            let hdr = step(0.000001, self.sun_true.w)
            var yt = max(yl * self.sun_e.w, 0.0)
            yt = mix(yt / (1.0 + yt), yt * self.sun_true.w, hdr)
            let bx = xc * (yt / yc)
            let bz = (1.0 - xc - yc) * (yt / yc)
            let r = max(3.2406 * bx - 1.5372 * yt - 0.4986 * bz, 0.0)
            let gr = max((0.0 - 0.9689) * bx + 1.8758 * yt + 0.0415 * bz, 0.0)
            let b = max(0.0557 * bx - 0.204 * yt + 1.057 * bz, 0.0)
            let m = max(max(r, gr), max(b, 1.0))
            var day = pow(
                vec3(r / m, gr / m, b / m),
                vec3(0.4545454, 0.4545454, 0.4545454)
            )
            day = mix(day, vec3(r, gr, b), hdr) * mix(1.0, 0.35, clamp((0.0 - v.y) * 3.0, 0.0, 1.0))
                * self.color.xyz

            // The visible disc, Mie glow and afterglow follow the true sun;
            // sun_e is clamped above the horizon only for Perez stability.
            let nb = self.zenith.w
            let sun_t = self.sun_true.xyz
            let gt = acos(clamp(dot(v, sun_t), 0.0 - 1.0, 1.0))
            let absf = exp(vec3(0.39, 0.57, 1.0)
                * (0.0 - 0.485 / pow(max(v.y + 0.033, 0.02), 0.75))) * 2.0
            let abss = exp(vec3(0.39, 0.57, 1.0)
                * (0.0 - 0.485 / pow(max(sun_t.y + 0.033, 0.012), 0.75))) * 2.0
            let limb = 1.0 - smoothstep(0.048, 0.055, gt)
            let mie_d = clamp(1.0 - pow(gt * 0.55, 0.1), 0.0, 1.0)
            let mie = mie_d * mie_d * (3.0 - 2.0 * mie_d) * 1.4
            day = day + (absf * (limb * 20.0) + abss * mie)
                * clamp((v.y + 0.033) * 90.0 + 0.5, 0.0, 1.0)

            // The game's moonless night dome. The bright day/afterglow term
            // is completely absent once nb reaches one at civil twilight.
            let nsky = mix(
                vec3(0.010, 0.012, 0.020),
                vec3(0.002, 0.003, 0.006),
                clamp(v.y * 1.4, 0.0, 1.0)
            )

            // Rotate into the celestial frame. A bound panorama enriches
            // the catalogue; without one, both analytic point layers remain
            // visible and follow the same rotation and twilight fade.
            let sd = vec3(
                dot(self.star_r0.xyz, v),
                dot(self.star_r1.xyz, v),
                dot(self.star_r2.xyz, v)
            )
            let su = atan2(sd.z, sd.x) * 0.15915494 + 0.5
            let sv = 0.5 - asin(clamp(sd.y, 0.0 - 1.0, 1.0)) * 0.31830989
            let star_fade = nb * clamp(v.y * 6.0 + 0.1, 0.0, 1.0)
            let smap = self.star_tex.sample_as_bgra(vec2(su, sv)).xyz * self.star_r0.w
            let lum = dot(smap, vec3(0.35, 0.5, 0.15))
            let suv = vec2(su * 1600.0, sv * 800.0)
            let sh = fract(sin(dot(floor(suv), vec2(127.1, 311.7))) * 43758.5453)
            let spark = step(0.995 - lum * 0.35, sh)
                * pow(clamp(1.0 - length(fract(suv) - vec2(0.5, 0.5)) * 2.0, 0.0, 1.0), 3.0)
                * (0.3 + 0.7 * fract(sh * 57.31))
            let suv2 = vec2(su * 400.0, sv * 200.0)
            let sh2 = fract(sin(dot(floor(suv2), vec2(269.5, 183.3))) * 43758.5453)
            let spark2 = step(0.992, sh2)
                * pow(clamp(1.0 - length(fract(suv2) - vec2(0.5, 0.5)) * 2.4, 0.0, 1.0), 4.0)
                * (0.5 + 0.5 * fract(sh2 * 43.7))
            let stars = (smap * 0.18
                + vec3(0.85, 0.9, 1.0) * spark
                + vec3(1.0, 0.97, 0.9) * spark2) * star_fade
            let hash = fract(
                sin(dot(v.xy + v.zz, vec2(12.9898, 78.233))) * 43758.5453
            )
            var sky_rgb = mix(day, nsky, nb) + stars
            if self.sky_fog.w > 0.5 {
                // Optical depth of the height fog along a ray to infinity:
                // density * H * layer / sin(elevation); below the horizon
                // the ray meets the fogged ground plane.
                let fog = 1.0 - exp(0.0 - self.sky_fog.x / max(v.y, 0.02))
                sky_rgb = mix(sky_rgb, self.sky_fog_color, fog)
            }
            return vec4(sky_rgb + vec3(1.0, 1.0, 1.0) * ((hash - 0.5) * 0.008), 1.0)
        }
    }

    // A MAP's own sky surfaces (model.rs SkyPart): the faces Doom, Quake and
    // Q3 drew as "look through here", shaded by the view ray instead of by
    // the world's light.
    //
    // A sibling of DrawSceneSkinned rather than a mode inside it, for the
    // reason written above DrawSceneSkyAnalytic: that shader's pixel fn is
    // already at the script-shader's capacity, and a sky branch there would
    // also make every prop in the world pay for it. This one has no
    // lighting, no AO, no lightmap, no CSM and no fog to execute — the sky
    // is not lit, it IS the light — and it is drawn once per map.
    //
    // Depth is written normally: the faces sit where the level put them, so
    // walls in front occlude them for free and the sky never covers the
    // world.
    mod.draw.DrawSceneSkyMap = mod.std.set_type_default() do #(DrawSceneSkyMap::script_shader(vm)){
        alpha_blend: false
        // Classic sky portals are ceiling/floor sheets seen from their
        // underside as often as their front. Culling those faces exposes the
        // pass clear colour (black) even though SKY1 is correctly bound.
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        // The same packed stream the static models use, so a map's sky faces
        // upload through exactly the same path as the rest of the map.
        geom: vertex_buffer(geom.GameMeshVertexAo, geom.GameMeshAoGeom)
        sky0: texture_2d(float)
        sky1: texture_2d(float)
        // The lighting convention (see ClusteredLighting.lin_ctl): the HDR
        // lane decodes the painted sky and pre-divides it by exposure so it
        // reaches the screen at its authored brightness.
        lin_ctl: uniform(vec4(0.0, 1.0, 1.0, 1.0))
        sky_out: fn(c: vec3) -> vec4 {
            if self.lin_ctl.x < 0.5 { return vec4(c * self.brightness, 1.0) }
            let l = c * (c * (c * 0.305306011 + vec3(0.682171111, 0.682171111, 0.682171111)) + vec3(0.012522878, 0.012522878, 0.012522878))
            return vec4(l * (self.brightness * self.lin_ctl.z), 1.0)
        }
        world: varying(vec4f)
        // The TRUE world ray, camera to fragment. Interpolating the ray and
        // normalizing per fragment is what makes the sky follow the camera's
        // rotation exactly while staying pinned to real geometry.
        v_ray: varying(vec3f)

        vertex: fn() {
            let pos = vec3(self.geom.px, self.geom.py, self.geom.pz)
            let model_view = self.draw_list.view_transform * self.transform
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            // Pre-stage world position: the ray is measured where the camera
            // physically is, never in an MR diorama's shrunken space.
            let wp = (self.transform * vec4(pos.x, pos.y, pos.z, 1.0)).xyz
            self.v_ray = wp - self.eye.xyz
            self.vertex_pos = self.draw_pass.camera_projection
                * (self.draw_pass.camera_view * self.world)
        }

        // Every branch returns rather than assigning a shared colour: the
        // three projections share no work, and one `if` per fragment with an
        // early out is both the cheapest and the shape the other shaders in
        // this file already use.
        //
        // Every result is unlit, unfogged and at full brightness — the
        // prelit contract. A sky that takes fog goes grey at the horizon
        // while the painted horizon in the image says otherwise, and a sky
        // that takes sun goes black when you look away from it.
        pixel: fn() {
            let dir = normalize(self.v_ray)
            if self.sky_p.x < 1.5 {
                // CYLINDER (Doom/Duke). Yaw wraps `repeat` times round the
                // compass — Doom's 256-wide strip goes round four times —
                // and pitch covers a BAND, not the hemisphere: sky_q.x is
                // how much of a half turn the image's height spans, and both
                // ends clamp, which is what keeps the horizon row stretched
                // under the player exactly as Doom drew it.
                let u = atan2(dir.x, dir.z) * 0.15915494 * self.sky_p.y + self.sky_p.z
                let pitch = asin(clamp(dir.y, 0.0 - 1.0, 1.0))
                let v = clamp(0.5 - pitch * 0.31830989 / max(self.sky_q.x, 0.001), 0.0, 1.0)
                let c = self.sky0.sample_as_bgra_repeat(vec2(u, v))
                return self.sky_out(c.xyz)
            }
            if self.sky_p.x < 2.5 {
                // QUAKE_SCROLL. Quake flattens the sphere by stretching the
                // up axis 3x and rescaling the direction to a fixed radius
                // (6*63 units), which is what gives the classic swirl toward
                // the zenith; the two layers then slide across it at their
                // own speeds and the front one keys the back one through.
                let d = vec3(dir.x, dir.y * 3.0, dir.z)
                let l = 378.0 / max(length(d), 0.0001)
                let back = vec2(self.sky_p.z + d.x * l, self.sky_p.z + d.z * l) * 0.0078125
                let front = vec2(self.sky_p.w + d.x * l, self.sky_p.w + d.z * l) * 0.0078125
                let b = self.sky0.sample_as_bgra_repeat(back)
                let f = self.sky1.sample_as_bgra_repeat(front)
                let c = mix(b.xyz, f.xyz, f.w * self.sky_q.y)
                return self.sky_out(c)
            }
            // CUBE, sampled as its equirect twin: longitude round, latitude
            // down. v is held a hair off the poles because the wrap sampler
            // would otherwise fetch the opposite pole's row in the last texel.
            let u = atan2(dir.x, dir.z) * 0.15915494 + 0.5 + self.sky_p.z
            let v = clamp(acos(clamp(dir.y, 0.0 - 1.0, 1.0)) * 0.31830989, 0.001, 0.999)
            let c = self.sky0.sample_as_bgra_repeat(vec2(u, v))
            return self.sky_out(c.xyz)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}
