//! The VFX draw shaders: `DrawSceneVfx` (every particle, expanded on the GPU
//! from burst records — see particles.rs) and `DrawSceneVfxDecal` (device-
//! local surface marks).

use makepad_draw::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // One instance = one burst record (or one explicit particle). The
    // geometry is a sheet of PARTICLE_SHEET quads; `geom_id` is the quad's
    // index, so each quad evaluates its own particle from the closed form
    // in `eval_particle` (particles.rs) — the two are the same expression.
    mod.draw.DrawSceneVfx = mod.std.set_type_default() do #(DrawSceneVfx::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        world: varying(vec4f)
        // Premultiplied over: an additive particle writes alpha 0 and only
        // adds light; an alpha one occludes what is behind it.
        alpha_blend: true
        // Billboards are built in view space; their winding is not ours.
        backface_culling: false
        // Transparent and unordered within a burst: writing depth would
        // punch holes in the particles behind.
        depth_write: false

        atlas: texture_2d(float)
        scene_depth: texture_2d(float)
        // x = 1 when the atlas is resident (else procedural fallbacks).
        vfx_ctl: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // x = 1: soft particles against `scene_depth`, a hardware depth
        // buffer of a GL-style projection; y, z = its m[2][2], m[3][2];
        // w = 1 when the backend stores clip z/w unmapped (Metal).
        vfx_depth: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // (linear on, exposure, 1/exposure, emission gain) — the lanes' HDR
        // convention (renderer/settings.rs).
        lin_ctl: uniform(vec4(0.0, 1.0, 1.0, 1.0))
        light_dir: uniform(vec3(0.35, 0.8, 0.45))
        sun_color: uniform(vec3(0.72, 0.72, 0.72))
        sun_sky: uniform(vec3(0.28, 0.28, 0.28))
        fog_color: uniform(vec3(0.75, 0.87, 0.96))
        fog_density: uniform(0.0)

        v_uv: varying(vec2f)
        v_color: varying(vec4f)
        // sprite, life 0..1, variant 0..1, additive
        v_info: varying(vec4f)
        // intensity, lit, view depth, soft distance
        v_look: varying(vec4f)
        v_light: varying(vec3f)
        v_clip: varying(vec4f)

        bit: fn(flags: float, b: float) -> float {
            let q = floor(flags / b)
            return q - floor(q * 0.5) * 2.0
        }

        rnd: fn(h: float, k: float, m: float) -> float {
            return fract(sin(h * k) * m)
        }

        vertex: fn() {
            let flags = self.p_kind.w
            let explicit = self.bit(flags, 4.0)
            let flat = self.bit(flags, 2.0)
            let collide = self.bit(flags, 1.0)
            let idx = self.geom.geom_id + self.p_extra.w
            let axis = normalize(self.p_dir.xyz + vec3(0.0, 0.00001, 0.0))
            var hint = vec3(0.0, 1.0, 0.0)
            if abs(axis.y) > 0.99 {
                hint = vec3(1.0, 0.0, 0.0)
            }
            let t1 = normalize(cross(hint, axis))
            let t2 = cross(axis, t1)

            var center = self.p_origin.xyz
            var vel = self.p_dir.xyz
            var size = self.p_size.x
            var col = self.p_color
            var rot = self.p_inherit.w
            var life_t = clamp(self.p_origin.w / max(self.p_life.x, 0.001), 0.0, 1.0)
            var variant = fract(self.p_kind.z * 0.618034)
            var alive = 1.0
            if explicit > 0.5 {
                alive = 1.0 - step(0.5, idx)
            } else {
                let count = max(self.p_extra2.x, 1.0)
                let h = idx * 0.618034 + self.p_kind.z * 0.7548777
                let r1 = self.rnd(h, 12.9898, 43758.547)
                let r2 = self.rnd(h, 78.233, 24634.635)
                let r3 = self.rnd(h, 39.4257, 15731.743)
                let r4 = self.rnd(h, 26.651, 29417.23)
                let r5 = self.rnd(h, 91.117, 35729.91)
                let r6 = self.rnd(h, 53.791, 19541.37)
                let birth = self.p_inherit.w + self.p_path.w * (idx + r1) / count
                let age = self.p_origin.w - birth
                let life = mix(self.p_life.x, self.p_life.y, r2)
                alive = (1.0 - step(count - 0.5, idx)) * step(0.0, age) * step(age, life) * step(0.0001, life)
                let t = clamp(age / max(life, 0.0001), 0.0, 1.0)
                let cos_t = 1.0 + (cos(self.p_dir.w) - 1.0) * r3
                let sin_t = sqrt(max(1.0 - cos_t * cos_t, 0.0))
                let phi = r4 * 6.2831853
                let d = axis * cos_t + (t1 * cos(phi) + t2 * sin(phi)) * sin_t
                let speed = mix(self.p_life.z, self.p_life.w, r5)
                let v0 = d * speed + self.p_inherit.xyz
                let jitter = vec3(r6 * 2.0 - 1.0, r1 * 2.0 - 1.0, r3 * 2.0 - 1.0) * self.p_extra.x
                let p0 = self.p_origin.xyz + self.p_path.xyz * (birth - self.p_inherit.w) + jitter
                // Linear drag + gravity, in closed form: v(t) = v0 e^-kt +
                // g (1 - e^-kt)/k, integrated.
                let k = max(self.p_size.w, 0.001)
                let e = exp(0.0 - k * max(age, 0.0))
                let g = vec3(0.0, 0.0 - self.p_size.z, 0.0)
                center = p0 + v0 * ((1.0 - e) / k) + g * (max(age, 0.0) / k - (1.0 - e) / (k * k))
                vel = v0 * e + g * ((1.0 - e) / k)
                let wobble = self.p_extra2.z
                center = center + (t1 * sin(age * 2.7 + r2 * 6.283) + t2 * cos(age * 2.1 + r4 * 6.283)) * wobble
                if collide > 0.5 && center.y < self.p_extra.z {
                    center = vec3(center.x, self.p_extra.z + 0.01, center.z)
                    vel = vec3(vel.x, 0.0, vel.z)
                }
                size = mix(self.p_size.x, self.p_size.y, t) * (0.75 + 0.5 * r6)
                col = mix(self.p_color, self.p_color_end, t)
                if self.p_extra2.y > 0.0 {
                    col = vec4(col.x, col.y, col.z, col.w * min(t / self.p_extra2.y, 1.0))
                }
                rot = r2 * 6.2831853 + self.p_look.w * (r3 - 0.5) * 2.0 * age
                life_t = t
                variant = r4
            }

            let cs = cos(rot)
            let sn = sin(rot)
            let corner = self.geom.geom_pos.xy * alive
            var world4 = self.draw_list.view_transform * vec4(center.x, center.y, center.z, 1.0)
            var view_pos = self.draw_pass.camera_view * world4
            // Quad axes in view space (for lighting the sprite's normals).
            var ax = vec2(cs, sn)
            if flat > 0.5 {
                // Lying in the plane across the effect's axis: a ground ring,
                // a ripple.
                let wp = center + (t1 * (corner.x * cs - corner.y * sn) + t2 * (corner.x * sn + corner.y * cs)) * size
                world4 = self.draw_list.view_transform * vec4(wp.x, wp.y, wp.z, 1.0)
                view_pos = self.draw_pass.camera_view * world4
            } else {
                var off = vec2(corner.x * cs - corner.y * sn, corner.x * sn + corner.y * cs) * size
                let stretch = self.p_look.z
                if stretch > 0.0 {
                    // Stretched along the screen projection of the velocity,
                    // head at the particle: a streak, not a dot.
                    let vv = (self.draw_pass.camera_view * (self.draw_list.view_transform * vec4(vel.x, vel.y, vel.z, 0.0))).xy
                    let len = length(vv)
                    if len > 0.0001 {
                        let dir = vv / len
                        let perp = vec2(0.0 - dir.y, dir.x)
                        let tail = len * stretch
                        off = dir * (corner.x * (size + tail) - tail * 0.5) + perp * (corner.y * size)
                        ax = dir
                    }
                }
                view_pos = vec4(view_pos.x + off.x, view_pos.y + off.y, view_pos.z, view_pos.w)
            }
            self.world = world4

            let lv = normalize((self.draw_pass.camera_view * (self.draw_list.view_transform * vec4(self.light_dir.x, self.light_dir.y, self.light_dir.z, 0.0))).xyz)
            self.v_light = vec3(dot(lv.xy, ax), dot(lv.xy, vec2(0.0 - ax.y, ax.x)), lv.z)
            self.v_uv = self.geom.geom_uv
            self.v_color = col
            self.v_info = vec4(self.p_kind.x, life_t, variant, self.p_kind.y)
            self.v_look = vec4(self.p_look.x, self.p_look.y, 0.0 - view_pos.z, max(self.p_extra.y, 0.001))
            let clip = self.draw_pass.camera_projection * view_pos
            self.v_clip = clip
            self.vertex_pos = clip
            return self.vertex_pos
        }

        cell: fn(col: float, row: float, uv: vec2) -> vec4 {
            return self.atlas.sample(vec2((col + 0.02 + uv.x * 0.96) / 8.0, (row + 0.02 + uv.y * 0.96) / 8.0))
        }

        // Two neighbouring frames of a 16-frame strip starting at `row`,
        // cross-faded.
        flip: fn(row: float, t: float, uv: vec2) -> vec4 {
            let f = clamp(t, 0.0, 1.0) * 14.999
            let f0 = floor(f)
            let f1 = min(f0 + 1.0, 15.0)
            let a = self.cell(f0 - floor(f0 / 8.0) * 8.0, row + floor(f0 / 8.0), uv)
            let b = self.cell(f1 - floor(f1 / 8.0) * 8.0, row + floor(f1 / 8.0), uv)
            return mix(a, b, f - f0)
        }

        pixel: fn() {
            let uv = self.v_uv
            let p = (uv - vec2(0.5, 0.5)) * 2.0
            let d = length(p)
            let sprite = self.v_info.x
            let t = self.v_info.y
            let variant = self.v_info.z
            let atlas_on = self.vfx_ctl.x
            // Coverage, emissive boost, and a normal in the quad's frame.
            var a = 0.0
            var heat = 0.0
            var n = vec3(p.x * 0.6, p.y * 0.6, sqrt(max(1.0 - d * d * 0.36, 0.0)))
            var cool = 0.0
            if sprite < 0.5 {
                // Dot: a soft glow with a hot centre.
                a = exp(0.0 - d * d * 5.0) * clamp(1.0 - d, 0.0, 1.0)
                heat = a * a
            } else if sprite < 1.5 {
                // Spark: white-hot core fading to its colour.
                let core = clamp(1.0 - d, 0.0, 1.0)
                a = core * core
                heat = core * core * core
            } else if sprite < 3.5 || sprite > 12.5 || (sprite > 4.5 && sprite < 5.5) {
                // Smoke, wisp, dust, fireball: flipbook density + normal.
                if atlas_on > 0.5 {
                    var s = vec4(0.0, 0.0, 0.0, 0.0)
                    if sprite < 2.5 || (sprite > 4.5 && sprite < 5.5) {
                        s = self.flip(0.0, t, uv)
                    } else if sprite < 3.5 {
                        s = self.flip(6.0, t, uv)
                    } else {
                        let c = floor(variant * 4.99)
                        s = self.cell(mix(0.0, 3.0 + c, step(0.5, c)), 4.0, uv)
                    }
                    a = s.w
                    n = normalize(vec3(s.x * 2.0 - 1.0, s.y * 2.0 - 1.0, max(s.z, 0.05)))
                } else {
                    a = clamp(1.0 - d, 0.0, 1.0)
                    a = a * a
                }
                if sprite > 4.5 && sprite < 5.5 {
                    // Fireball: burning gas early, soot late; the densest
                    // parts burn longest.
                    let burn = clamp(1.0 - t * 1.6, 0.0, 1.0)
                    heat = burn * burn * (0.35 + a)
                    cool = 1.0 - burn
                }
            } else if sprite < 4.5 {
                // Fire flipbook: heat in r.
                if atlas_on > 0.5 {
                    let s = self.flip(2.0, fract(t + variant * 0.35), uv)
                    a = s.w
                    heat = s.x
                } else {
                    a = clamp(1.0 - d, 0.0, 1.0)
                    heat = a
                }
            } else if sprite < 6.5 {
                // Debris chip.
                if atlas_on > 0.5 {
                    let s = self.cell(1.0 + floor(variant * 2.99), 4.0, uv)
                    a = step(0.5, s.w)
                    n = normalize(vec3(s.x * 2.0 - 1.0, s.y * 2.0 - 1.0, max(s.z, 0.05)))
                } else {
                    a = step(max(abs(p.x), abs(p.y)), 0.7)
                }
            } else if sprite < 7.5 {
                // Ring: a bright thin band.
                a = smoothstep(0.62, 0.84, d) * (1.0 - smoothstep(0.86, 1.0, d))
                heat = a * 0.5
            } else if sprite < 8.5 {
                // Droplet: a soft bead with a highlight.
                a = smoothstep(1.0, 0.55, d)
                heat = smoothstep(0.35, 0.0, length(p - vec2(0.0 - 0.25, 0.3))) * 0.8
            } else if sprite < 9.5 {
                // Flash: a hot core and uneven spikes.
                let ang = atan2(p.y, p.x)
                let spikes = pow(max(cos(ang * 3.0 + variant * 6.28), 0.0), 6.0) * 0.75
                    + pow(max(cos(ang * 5.0 + variant * 11.0), 0.0), 10.0) * 0.5
                let core = clamp(1.0 - d * 1.6, 0.0, 1.0)
                a = clamp(core * core + spikes * clamp(1.0 - d, 0.0, 1.0), 0.0, 1.0)
                heat = core * core
            } else if sprite < 10.5 {
                // Star: four thin spikes and a glow.
                let sx = clamp(1.0 - abs(p.y) * 10.0, 0.0, 1.0) * clamp(1.0 - abs(p.x), 0.0, 1.0)
                let sy = clamp(1.0 - abs(p.x) * 10.0, 0.0, 1.0) * clamp(1.0 - abs(p.y), 0.0, 1.0)
                let core = exp(0.0 - d * d * 12.0)
                a = clamp(sx + sy + core, 0.0, 1.0)
                heat = core
            } else if sprite < 11.5 {
                // Rain streak: thin across, soft at both ends.
                a = (1.0 - smoothstep(0.1, 0.9, abs(p.y))) * smoothstep(1.0, 0.4, abs(p.x))
            } else {
                // Snowflake.
                a = smoothstep(1.0, 0.3, d)
            }
            a = a * self.v_color.w
            if a < 0.002 {
                discard()
            }
            let add = self.v_info.w

            // Soft particles: fade out as the particle meets the scene
            // behind it, and hide it where the scene is in front.
            if self.vfx_depth.x > 0.5 {
                let ndc = self.v_clip.xy / self.v_clip.w
                let suv = vec2(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5)
                let zd = self.scene_depth.sample_nearest(suv).x
                if zd < 0.99999 {
                    // w = 1: the backend stores clip z/w as is (Metal);
                    // else the GL convention, z/w * 0.5 + 0.5.
                    let ndc_z = mix(zd * 2.0 - 1.0, zd, self.vfx_depth.w)
                    let scene_z = self.vfx_depth.z / (ndc_z + self.vfx_depth.y)
                    let gap = scene_z - self.v_look.z
                    if gap <= 0.0 {
                        discard()
                    }
                    a = a * clamp(gap / self.v_look.w, 0.0, 1.0)
                }
            }

            var base = self.v_color.xyz
            if self.lin_ctl.x > 0.5 {
                base = base * (base * (base * 0.305306011 + vec3(0.682171111, 0.682171111, 0.682171111)) + vec3(0.012522878, 0.012522878, 0.012522878))
            }
            // Emissive in display units (HDR: pre-divided by exposure so an
            // intensity of 1 lands at display white, above that blooms).
            var glow = self.v_look.x
            if self.lin_ctl.x > 0.5 {
                glow = glow * self.lin_ctl.z
            }
            // Hot cores go white, except flames, whose cores go yellow: an
            // orange mixed toward white reads as peach-pink.
            let is_fire = step(3.5, sprite) * step(sprite, 4.5)
            let hot = mix(base, mix(vec3(1.0, 1.0, 1.0), vec3(1.0, 0.72, 0.22), is_fire), clamp(heat * 0.7, 0.0, 1.0))
            let emissive = hot * glow * (1.0 + heat * 1.5)
            // Sun + sky on the sprite's normal, wrapped so the shadow side
            // of a puff is dim rather than black.
            let ndl = clamp(dot(n, normalize(self.v_light)) * 0.6 + 0.4, 0.0, 1.0)
            let lit_rgb = base * (self.sun_color * ndl + self.sun_sky)
            let lit = clamp(self.v_look.y + cool, 0.0, 1.0)
            var rgb = mix(emissive, lit_rgb, lit)
            if sprite > 4.5 && sprite < 5.5 {
                rgb = lit_rgb + emissive * (1.0 - cool)
            }
            let fog = 1.0 - exp(0.0 - self.v_look.z * self.fog_density)
            if add > 0.5 {
                rgb = rgb * (1.0 - fog)
                return vec4(rgb.x * a, rgb.y * a, rgb.z * a, 0.0)
            }
            rgb = mix(rgb, self.fog_color, fog)
            return vec4(rgb.x * a, rgb.y * a, rgb.z * a, a)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // A device-local surface mark: an oriented rectangle lying on its
    // surface (the CPU lifts it 4 mm off it), procedural per kind. Unlit
    // albedo times the frame's ambient + sun, so a mark sits in the scene's
    // light rather than glowing in the dark.
    mod.draw.DrawSceneVfxDecal = mod.std.set_type_default() do #(DrawSceneVfxDecal::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        world: varying(vec4f)
        v_uv: varying(vec2f)
        alpha_blend: true
        backface_culling: false
        depth_write: false
        // Ambient + sun reaching a surface, in the lane's units.
        decal_light: uniform(vec3(1.0, 1.0, 1.0))
        lin_ctl: uniform(vec4(0.0, 1.0, 1.0, 1.0))

        vertex: fn() {
            let normal = normalize(self.d_normal.xyz)
            let along = normalize(self.d_dir.xyz)
            let across = cross(normal, along)
            let corner = self.geom.geom_pos
            let w = self.d_pos.xyz + along * (corner.x * 2.0 * self.d_pos.w) + across * (corner.y * 2.0 * self.d_normal.w)
            self.world = self.draw_list.view_transform * vec4(w.x, w.y, w.z, 1.0)
            self.v_uv = self.geom.geom_uv
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
            return self.vertex_pos
        }

        hash2: fn(q: vec2) -> float {
            return fract(sin(dot(q, vec2(127.1, 311.7))) * 43758.5453)
        }
        noise2: fn(q: vec2) -> float {
            let i = floor(q)
            let f = fract(q)
            let u = f * f * (vec2(3.0, 3.0) - f * 2.0)
            let a = self.hash2(i)
            let b = self.hash2(i + vec2(1.0, 0.0))
            let c = self.hash2(i + vec2(0.0, 1.0))
            let e = self.hash2(i + vec2(1.0, 1.0))
            return mix(mix(a, b, u.x), mix(c, e, u.x), u.y)
        }
        fbm2: fn(q: vec2) -> float {
            return self.noise2(q) * 0.55 + self.noise2(q * 2.1 + vec2(5.2, 1.3)) * 0.3 + self.noise2(q * 4.3 + vec2(1.7, 9.2)) * 0.15
        }

        pixel: fn() {
            let p = (self.v_uv - vec2(0.5, 0.5)) * 2.0
            let d = length(p)
            let kind = self.d_dir.w
            let seed = self.d_extra.x * 17.0
            let nz = self.fbm2(p * 3.0 + vec2(seed, seed * 1.3))
            var rgb = vec3(0.0, 0.0, 0.0)
            var a = 0.0
            if kind < 0.5 {
                // Bullet hole: a dark puncture with a scorched soft rim.
                let rim = smoothstep(0.42, 0.82, d)
                rgb = mix(vec3(0.012, 0.011, 0.010), vec3(0.105, 0.075, 0.045), rim)
                a = (1.0 - smoothstep(0.78, 1.0, d)) * mix(0.94, 0.72, rim)
            } else if kind < 1.5 {
                // Scorch: soot, ragged at the edge.
                let r = d + (nz - 0.5) * 0.45
                rgb = mix(vec3(0.01, 0.009, 0.008), vec3(0.08, 0.05, 0.03), smoothstep(0.2, 0.9, r))
                a = (1.0 - smoothstep(0.45, 1.0, r)) * 0.92
            } else if kind < 2.5 {
                // Blood: a splat of blobs and droplets.
                let r = d + (nz - 0.5) * 0.9
                let drops = step(0.78, self.fbm2(p * 7.0 + vec2(seed * 2.0, 3.0))) * (1.0 - smoothstep(0.7, 1.0, d))
                a = max(1.0 - smoothstep(0.35, 0.55, r), drops) * 0.9
                rgb = vec3(0.16, 0.01, 0.01) * (0.7 + 0.3 * nz)
            } else if kind < 3.5 {
                // Dirt splash.
                let r = d + (nz - 0.5) * 0.7
                a = (1.0 - smoothstep(0.3, 0.95, r)) * 0.75
                rgb = vec3(0.22, 0.17, 0.11) * (0.8 + 0.4 * nz)
            } else if kind < 4.5 {
                // Skid: tread ribs across a dark band, feathered at the sides.
                let ribs = 0.75 + 0.25 * sin(p.y * 22.0)
                let side = 1.0 - smoothstep(0.55, 1.0, abs(p.y))
                a = side * ribs * (0.55 + 0.25 * nz)
                rgb = vec3(0.02, 0.02, 0.02)
            } else if kind < 5.5 {
                // Crack: dark radial fractures.
                let ang = atan2(p.y, p.x)
                let lines = pow(max(cos(ang * 5.0 + seed + nz * 3.0), 0.0), 40.0)
                a = (lines + (1.0 - smoothstep(0.0, 0.15, d))) * (1.0 - smoothstep(0.6, 1.0, d)) * 0.85
                rgb = vec3(0.03, 0.03, 0.03)
            } else if kind < 6.5 {
                // Energy burn: charred centre, tinted rim.
                let rim = smoothstep(0.28, 0.76, d)
                rgb = mix(vec3(0.008, 0.008, 0.009), self.d_color.xyz * 0.48, rim)
                a = (1.0 - smoothstep(0.68, 1.0, d)) * mix(0.94, 0.66, rim)
            } else {
                // Wet patch: darkens whatever it lies on.
                let r = d + (nz - 0.5) * 0.6
                a = (1.0 - smoothstep(0.4, 1.0, r)) * 0.4
                rgb = vec3(0.0, 0.0, 0.0)
            }
            if kind < 5.5 || kind > 6.5 {
                rgb = rgb * self.d_color.xyz
            }
            // The kinds above are authored in display terms: linearise for
            // the HDR lane, then light them like the surface under them.
            if self.lin_ctl.x > 0.5 {
                rgb = rgb * rgb
            }
            rgb = rgb * self.decal_light
            a = a * self.d_color.w
            if a < 0.003 {
                discard()
            }
            return vec4(rgb.x * a, rgb.y * a, rgb.z * a, a)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}

/// One particle burst record per instance: the fields ARE
/// [`crate::particles::ParticleInstance`], vec4-packed.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneVfx {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub p_origin: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub p_dir: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub p_path: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub p_inherit: Vec4f,
    #[live(vec4(1.0, 1.0, 0.0, 0.0))]
    pub p_life: Vec4f,
    #[live(vec4(0.1, 0.1, 0.0, 0.0))]
    pub p_size: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub p_color: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 0.0))]
    pub p_color_end: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub p_look: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub p_kind: Vec4f,
    #[live(vec4(0.0, 0.1, -1000000000.0, 0.0))]
    pub p_extra: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub p_extra2: Vec4f,
}

impl DrawSceneVfx {
    pub fn set_record(&mut self, r: &crate::particles::ParticleInstance) {
        self.p_origin = r.origin;
        self.p_dir = r.dir;
        self.p_path = r.path;
        self.p_inherit = r.inherit;
        self.p_life = r.life;
        self.p_size = r.size;
        self.p_color = r.color;
        self.p_color_end = r.color_end;
        self.p_look = r.look;
        self.p_kind = r.kind;
        self.p_extra = r.extra;
        self.p_extra2 = r.extra2;
    }
}

/// One surface mark per instance.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneVfxDecal {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(1.0)]
    pub depth_clip: f32,
    /// xyz centre (lifted), w half length.
    #[live(vec4(0.0, 0.0, 0.0, 0.1))]
    pub d_pos: Vec4f,
    /// xyz normal, w half width.
    #[live(vec4(0.0, 1.0, 0.0, 0.1))]
    pub d_normal: Vec4f,
    /// xyz in-plane axis, w kind.
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub d_dir: Vec4f,
    /// rgb tint, a opacity (already faded).
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub d_color: Vec4f,
    /// x seed, yzw unused.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub d_extra: Vec4f,
}
