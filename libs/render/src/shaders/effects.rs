//! Fireworks, lamp flares, bullet decals and in-world screens/sprites.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // Fireworks: ONE instance per shell, expanded on the GPU into
    // `SPARKS_PER_SHELL` sparks whose positions are a closed form of
    // (spark index, seed, age). Nothing is stepped and nothing is uploaded
    // per frame — see firework.rs for why that is the whole point.
    mod.draw.DrawSceneFirework = mod.std.set_type_default() do #(DrawSceneFirework::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        // The shared spark sheet uses the CubeVertex layout: geom_pos.xy is
        // the billboard corner and geom_id is the spark index.
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        world: varying(vec4f)
        // Additive: overlapping sparks should brighten toward white the way
        // real ones do, not composite over each other and go muddy.
        alpha_blend: true
        // DELIBERATE: a billboarded quad is built facing the camera in view
        // space, so its winding depends on nothing we control here, and half
        // the sky would vanish if this culled.
        backface_culling: false
        // DELIBERATE: sparks are transparent and unordered among themselves.
        // Writing depth would make whichever drew first punch a hole in the
        // ones behind it.
        depth_write: false

        v_color: varying(vec4f)
        v_uv: varying(vec2f)

        // The one function a style overrides. Rust owns structure; this owns
        // the look. Kept deliberately small and total — no geometry, no time
        // base, no trajectory — so an override cannot break the simulation,
        // only restyle it.
        //
        //   life_t  0..1 through this spark's life
        //   heat    1 at birth, 0 within a quarter second — the burst flash
        //   rnd     stable per-spark random, 0..1
        //   speed_t 0..1, slow core .. fast outer shell
        //
        // Returns rgb + an alpha multiplier, so a style controls its own fade.
        // Extra displacement on top of the ballistic arc, in world units.
        // This is where swirl, fizzle, drift and wobble live. Additive, so a
        // style can never move a spark somewhere the burst could not reach —
        // it can only decorate the path.
        //
        //   dir     this spark's unit launch direction
        //   t       seconds since the burst
        //   rnd     stable per-spark random
        spark_motion: fn(dir: vec3, t: float, rnd: float) -> vec3 {
            return vec3(0.0, 0.0, 0.0)
        }

        // Size multiplier, 1.0 = the engine's default taper. Lets a style
        // pulse, shrink or bloom individual sparks.
        spark_size: fn(life_t: float, rnd: float, speed_t: float) -> float {
            return 1.0
        }

        spark_color: fn(life_t: float, heat: float, rnd: float, style: float) -> vec4 {
            let twinkle = 0.7 + 0.3 * sin(rnd * 63.0 + life_t * 40.0)
            let tint = mix(self.color, self.color_tail, life_t * life_t)
            let rgb = mix(tint.xyz, vec3(1.0, 1.0, 1.0), heat)
            let fade = (1.0 - life_t) * (1.0 - life_t)
            return vec4(rgb.x * twinkle, rgb.y * twinkle, rgb.z * twinkle, fade)
        }

        vertex: fn() {
            let idx = self.geom.geom_id
            // Three decorrelated randoms per spark from one cheap hash. The
            // seed shifts the whole stream, so two shells never open alike.
            let seed = self.params.y
            let h = idx * 0.6180339887 + seed * 0.7548776662
            let r1 = fract(sin(h * 12.9898) * 43758.5453)
            let r2 = fract(sin(h * 78.2330) * 24634.6345)
            let r3 = fract(sin(h * 39.4257) * 15731.7433)

            // A FIBONACCI SPHERE, not random scatter. A real shell packs its
            // stars evenly around the burst charge and lights them at once, so
            // the break is a true sphere — that even spacing is exactly why a
            // peony looks round from every angle. Hashing a direction per
            // spark gives clumps and holes, which reads as noise however many
            // sparks you throw at it.
            //
            // The golden angle steps phi so successive stars never line up,
            // and z steps linearly so they are evenly spread in AREA, not in
            // latitude (which would bunch them at the poles).
            // Each STAR is a train of particles, not one stretched rect. That
            // is what the reference photos show: every ray is beaded, a string
            // of glowing points strung along the path the star has flown.
            //
            // So the spark index splits: which star, and how far back along its
            // trail. A trail particle is the SAME star sampled at an earlier
            // time — nothing new to simulate, just the closed form evaluated
            // at t - delay.
            let trail_n = 8.0
            let star_i = floor(idx / trail_n)
            let trail_i = idx - star_i * trail_n
            // 2560 beads / 8 = 320 stars. Keep in step with SPARKS_PER_SHELL
            // and TRAIL_LEN in firework.rs.
            let n = 2560.0 / trail_n
            let fi = star_i + 0.5
            let cz = 1.0 - 2.0 * fi / n
            let sz = sqrt(max(1.0 - cz * cz, 0.0))
            // Per-shell rotation so two shells are not the same object twice.
            let phi = fi * 2.3999632297 + seed
            let dir = vec3(sz * cos(phi), cz, sz * sin(phi))

            // A shell is not a uniform ball: the burst charge throws sparks at
            // a spread of speeds, and that spread is most of what makes the
            // front edge read as a shockwave rather than a balloon.
            // Nearly UNIFORM speed. The stars are identical and ignite
            // together, so they travel together and the shell stays a crisp
            // expanding sphere. (The wide random(1,10) spread in the canvas
            // demos is a 2D trick for filling a disc — in 3D it just turns the
            // sphere to mush.) A few percent of jitter keeps the edge from
            // looking machined.
            // A willow throws its stars gently and lets them fall — that is
            // the whole effect, so it is a speed change, not a colour one.
            let is_willow = step(1.5, self.params.w)
            let speed = self.params.x * (0.94 + 0.06 * r3) * mix(1.0, 0.55, is_willow)

            // 40ms between beads: close enough to read as a continuous streak,
            // far enough that the beads are visible the way they are in a
            // photograph.
            let age = self.origin_age.w - trail_i * 0.040
            let t = max(age, 0.0)
            // Drag: v(t) = v0*exp(-k t), so displacement is v0/k*(1-exp(-k t)).
            // Sparks decelerate hard and then hang, which is the shape of a
            // real burst; ballistic-only looks like a thrown handful.
            // Matches the canvas-demo convention: speed *= 0.95 every frame at
            // 60fps is exactly e^(-kt) with k = -60*ln(0.95) = 3.08. Sparks
            // decelerate hard and then hang, which is the shape of a real
            // burst; a softer k reads as a thrown handful.
            let k = 3.08
            let drag = (1.0 - exp(0.0 - k * t)) / k
            // A star does NOT free-fall. It is a few grams of burning
            // composition with a lot of drag, so it reaches terminal velocity
            // almost immediately and then DRIFTS — which is why a real shell
            // hangs and ours plummeted.
            //
            // Vertical fall under linear drag: quadratic for the first instant,
            // then a constant descent at terminal velocity. `vt` is ~7 m/s,
            // and one world unit is one metre here (a character is 1.8 tall).
            let vt = mix(7.0, 11.0, is_willow)
            let fall = vt * (t - (1.0 - exp(0.0 - k * t)) / k)
            let origin = self.origin_age.xyz
            let burst = origin + dir * speed * drag - vec3(0.0, fall, 0.0)
                + self.spark_motion(dir, t, r1)

            // Before age 0 the shell is still climbing: draw every spark
            // stacked at the rising point so it reads as one streak.
            let rise = clamp(1.0 + age / 0.85, 0.0, 1.0)
            let launch = self.launch_life.xyz
            let climb = launch + (origin - launch) * rise
            let center = mix(climb, burst, step(0.0, age))

            // Billboard in VIEW space: offsetting after the view transform is
            // camera-facing by construction, so no camera axes are needed.
            //
            // STRETCHED ALONG THE MOTION. This is what makes it a firework
            // instead of an expanding ball of dots: a chrysanthemum is read as
            // radial STREAKS from a centre, and a round sprite can never say
            // that however many you draw. The quad is elongated along the
            // screen projection of the spark's own velocity and squeezed
            // across it, so every star draws the little comet it actually is.
            self.world = self.draw_list.view_transform * vec4(center.x, center.y, center.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let life_t = clamp(t / self.launch_life.w, 0.0, 1.0)
            // Beads shrink and dim toward the tail, so a streak has a bright
            // head and fades back toward the burst centre.
            let tail_t = trail_i / trail_n
            let taper = 1.0 - tail_t * 0.75
            let size = self.params.z * (1.0 - life_t * 0.6) * taper
                * self.spark_size(life_t, r1, r3)

            let corner = self.geom.geom_pos
            let billboard = vec4(
                view_pos.x + corner.x * size,
                view_pos.y + corner.y * size,
                view_pos.z,
                view_pos.w
            )

            // STYLE HOOK. Everything above is structure — where a spark is,
            // how big, how long it lives. Everything about how it LOOKS goes
            // through `spark_color`, which a splash script overrides without
            // touching (or needing to understand) the trajectory.
            //
            // `heat` is the birth flash, 1 at t=0 and gone within ~0.25s;
            // `rnd` is stable per spark; `speed_t` says whether this spark is
            // on the fast outer shell or the slow core, which is what lets a
            // style colour the leading edge differently from the middle.
            let heat = clamp(1.0 - t * 4.0, 0.0, 1.0)
            let speed_t = r3
            let styled = self.spark_color(life_t, heat, r1, self.params.w)
            let bead_fade = (1.0 - tail_t * 0.85)
            self.v_color = vec4(
                styled.x,
                styled.y,
                styled.z,
                styled.w * bead_fade * step(0.0, age + 0.85)
            )
            // Geometry attributes do not exist in the fragment stage, so the
            // quad's uv has to travel as a varying.
            self.v_uv = self.geom.geom_uv
            self.vertex_pos = self.draw_pass.camera_projection * billboard
            return self.vertex_pos
        }

        // The spark's sprite program. `uv` is 0..1 across the billboard and
        // `tint` is whatever `spark_color` returned, so a style can draw a
        // streak, a ring, a star or a soft dot without knowing anything about
        // where the spark is.
        spark_pixel: fn(uv: vec2, tint: vec4) -> vec4 {
            let d = length(uv - vec2(0.5, 0.5)) * 2.0
            let glow = clamp(1.0 - d, 0.0, 1.0)
            let core = glow * glow
            return vec4(tint.x * core, tint.y * core, tint.z * core, tint.w * core)
        }

        pixel: fn() {
            return self.spark_pixel(self.v_uv, self.v_color)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }

    }

    // Old-school "pin spotlight" lens flare: one additive camera-facing
    // billboard per visible lamp head, drawn from the renderer's per-frame
    // light list. Procedural in the pixel stage — a soft radial disc plus a
    // 4-point star spike, no texture. Depth-TESTED but not depth-written, so
    // the glow clips behind walls the way the 90s intended; alpha 0 output
    // under premultiplied blending makes it pure additive light.
    mod.draw.DrawSceneFlare = mod.std.set_type_default() do #(DrawSceneFlare::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        // One quad in the CubeVertex layout (geom_pos.xy = billboard corner),
        // shared by every flare instance — see ensure_flare_geometry.
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        world: varying(vec4f)
        alpha_blend: true
        // Billboarded in view space; winding is not ours to control.
        backface_culling: false
        // Transparent glow must never punch holes in later transparents.
        depth_write: false
        v_uv: varying(vec2f)

        vertex: fn() {
            let center = self.flare_pos.xyz
            self.world = self.draw_list.view_transform
                * vec4(center.x, center.y, center.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let size = self.flare_pos.w
            let corner = self.geom.geom_pos
            let billboard = vec4(
                view_pos.x + corner.x * size,
                view_pos.y + corner.y * size,
                view_pos.z,
                view_pos.w
            )
            self.v_uv = self.geom.geom_uv
            self.vertex_pos = self.draw_pass.camera_projection * billboard
            return self.vertex_pos
        }

        pixel: fn() {
            let p = (self.v_uv - vec2(0.5, 0.5)) * 2.0
            let d = length(p)
            // Hot core, wide soft halo.
            let disc = clamp(1.0 - d, 0.0, 1.0)
            let core = disc * disc * disc
            let halo = (1.0 - smoothstep(0.05, 0.95, d)) * 0.30
            // 4-point star: thin spikes along the billboard axes, faded
            // toward the rim so they taper instead of hitting the quad edge.
            let sx = clamp(1.0 - abs(p.y) * 12.0, 0.0, 1.0) * clamp(1.0 - abs(p.x), 0.0, 1.0)
            let sy = clamp(1.0 - abs(p.x) * 12.0, 0.0, 1.0) * clamp(1.0 - abs(p.y), 0.0, 1.0)
            let spike = (sx * sx + sy * sy) * 0.55
            let a = (core + halo + spike) * self.flare_col.w
            return vec4(
                self.flare_col.x * a,
                self.flare_col.y * a,
                self.flare_col.z * a,
                0.0
            )
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // A bullet hole is one tiny surface-aligned procedural quad. No texture,
    // model, entity or static-slab mutation: the CPU supplies only its current
    // world pose and this shader makes the dark centre + scorched soft rim.
    mod.draw.DrawSceneDecal = mod.std.set_type_default() do #(DrawSceneDecal::script_shader(vm)){
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
        // Marks overlap solid surfaces by design. The 2 mm world-space lift
        // wins their own depth test; not writing depth keeps overlapping marks
        // and later transparent effects well behaved.
        depth_write: false

        vertex: fn() {
            let normal = normalize(self.decal_normal.xyz)
            var hint = vec3(0.0, 1.0, 0.0)
            if abs(normal.y) > 0.92 {
                hint = vec3(1.0, 0.0, 0.0)
            }
            let base_right = normalize(cross(hint, normal))
            let base_up = cross(normal, base_right)
            let c = cos(self.decal_normal.w)
            let s = sin(self.decal_normal.w)
            let right = base_right * c + base_up * s
            let up = base_up * c - base_right * s
            let corner = self.geom.geom_pos
            let center = self.decal_pos.xyz
            let world3 = center
                + right * (corner.x * self.decal_pos.w)
                + up * (corner.y * self.decal_pos.w)
            self.world = self.draw_list.view_transform
                * vec4(world3.x, world3.y, world3.z, 1.0)
            self.v_uv = self.geom.geom_uv
            self.vertex_pos = self.draw_pass.camera_projection
                * (self.draw_pass.camera_view * self.world)
            return self.vertex_pos
        }

        pixel: fn() {
            let p = (self.v_uv - vec2(0.5, 0.5)) * 2.0
            let d = length(p)
            if d > 1.0 {
                discard()
            }
            if self.decal_color.w < 0.5 {
                let edge = 1.0 - smoothstep(0.78, 1.0, d)
                let rim = smoothstep(0.42, 0.82, d)
                let rgb = vec3(0.012, 0.011, 0.010).mix(vec3(0.105, 0.075, 0.045), rim)
                let alpha = edge * mix(0.94, 0.72, rim)
                return Pal.premul(vec4(rgb.x, rgb.y, rgb.z, alpha))
            }
            if self.decal_color.w < 1.5 {
                // A broad, soot-dark blast with a deliberately softer edge
                // than a puncture. Its 0.5 m scale comes from the mark.
                let edge = 1.0 - smoothstep(0.52, 1.0, d)
                let rim = smoothstep(0.30, 0.90, d)
                let rgb = vec3(0.010, 0.009, 0.008).mix(vec3(0.095, 0.052, 0.022), rim)
                let alpha = edge * mix(0.86, 0.48, rim)
                return Pal.premul(vec4(rgb.x, rgb.y, rgb.z, alpha))
            }
            // Energy chars the centre and leaves a coloured burn rim. The
            // tint is the projectile/tracer colour from the weapon manifest.
            let edge = 1.0 - smoothstep(0.68, 1.0, d)
            let rim = smoothstep(0.28, 0.76, d)
            let tint = clamp(self.decal_color.xyz, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
            let rgb = vec3(0.008, 0.008, 0.009).mix(tint * 0.48, rim)
            let alpha = edge * mix(0.94, 0.66, rim)
            return Pal.premul(vec4(rgb.x, rgb.y, rgb.z, alpha))
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // Video screen: ONE upright textured quad in world space, fed a texture
    // the host updates per frame (in-world video playback). Reuses the shared
    // flare quad geometry (geom_pos.xy = corner in -0.5..0.5, geom_uv 0..1).
    // Opaque and depth-written, so the world occludes it like any solid.
    mod.draw.DrawSceneScreen = mod.std.set_type_default() do #(DrawSceneScreen::script_shader(vm)){
        ..mod.draw.SceneColorAdjust,
        lin_ctl: uniform(vec4(0.0, 1.0, 1.0, 1.0))
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        tex: texture_2d(float)
        world: varying(vec4f)
        v_uv: varying(vec2f)
        // A screen is watchable from behind (mirrored); culling half the
        // orientations away buys nothing here.
        backface_culling: false

        vertex: fn() {
            let center = self.screen_pos.xyz
            let yaw = self.screen_pos.w
            let corner = self.geom.geom_pos
            // Horizontal span rotated by yaw, vertical span straight up. The
            // quad's normal is (-sin yaw, 0, cos yaw): yaw 0 faces -z,
            // matching the camera-forward convention (sin yaw, ., -cos yaw)
            // so screen_yaw == camera_yaw faces the orbit camera squarely.
            let right = vec3(cos(yaw), 0.0, sin(yaw))
            // A FLOOR card lies flat on the ground plane instead of standing
            // up: its second axis is the ground-projected camera forward
            // (sin yaw, 0, -cos yaw), so the artwork reads screen-up on a
            // top-down view and the whole card stays at one height. Asked
            // for by a NEGATIVE retained sheet height (`screen_size.w`),
            // which every upright caller leaves positive.
            var up = vec3(0.0, 1.0, 0.0)
            if self.screen_size.w < 0.0 {
                up = vec3(right.z, 0.0, -right.x)
            }
            // A GROUND-ANCHORED billboard STANDS ON `screen_pos` instead of
            // being centred on it, and every one of its fragments takes the
            // DEPTH of that one point. Asked for by a NEGATIVE retained sheet
            // WIDTH (`screen_size.z`), which every other caller leaves
            // positive; its magnitude carries `1 + pad`, where `pad` is how
            // much transparent padding the sheet packs UNDER the figure, as a
            // share of the card's height. Both halves matter, and they are
            // one decision:
            //  * a sprite means "this thing stands here", so the caller says
            //    where the FEET are; the card drops by its own padding so the
            //    drawing — not the empty cell it is packed in — touches down;
            //  * a card looking down on a strategy map leans its head TOWARD
            //    the eye, so raw per-fragment depth lets a far unit's head
            //    win against a near unit's boots. Flattening the quad onto
            //    its own ground point is the classic sprite-engine answer:
            //    pieces sort by where they STAND, which is what a commander
            //    reads, while the world still occludes them normally.
            var oy = corner.y
            var anchored = 0.0
            if self.screen_size.z < 0.0 {
                oy = corner.y + 1.5 + self.screen_size.z
                anchored = 1.0
            }
            let world3 = vec3(
                center.x + right.x * corner.x * self.screen_size.x
                    + up.x * oy * self.screen_size.y,
                center.y + up.y * oy * self.screen_size.y,
                center.z + right.z * corner.x * self.screen_size.x
                    + up.z * oy * self.screen_size.y
            )
            self.world = self.draw_list.view_transform
                * vec4(world3.x, world3.y, world3.z, 1.0)
            // Video rows are top-first; quad uv.y grows upward. Flip v, then
            // map into this instance's window on the texture.
            let q = vec2(self.geom.geom_uv.x, 1.0 - self.geom.geom_uv.y)
            self.v_uv = vec2(
                self.uv_rect.x + q.x * (self.uv_rect.z - self.uv_rect.x),
                self.uv_rect.y + q.y * (self.uv_rect.w - self.uv_rect.y)
            )
            let clip = self.draw_pass.camera_projection
                * (self.draw_pass.camera_view * self.world)
            self.vertex_pos = clip
            if anchored > 0.5 {
                let foot = self.draw_list.view_transform
                    * vec4(center.x, center.y, center.z, 1.0)
                let foot_clip = self.draw_pass.camera_projection
                    * (self.draw_pass.camera_view * foot)
                if foot_clip.w > 0.0001 {
                    self.vertex_pos = vec4(
                        clip.x,
                        clip.y,
                        foot_clip.z * clip.w / foot_clip.w,
                        clip.w
                    )
                }
            }
            return self.vertex_pos
        }

        pixel: fn() {
            // Sprite sheets are pixel art: one fragment chooses one exact
            // source texel. Explicit level zero also keeps a generic decoded
            // image's optional mip chain out of this path. The same shader is
            // shared with smooth video, so that lane leaves `pixelated` off.
            var color = self.tex.sample(self.v_uv)
            if self.pixelated > 0.5 {
                color = self.tex.sample_nearest(self.v_uv, 0.0)
            }
            // Sprite quads (cutout) are cut-out artwork: the pixels outside
            // the figure are transparent and must not paint a rectangle.
            // A video screen keeps its opaque frame (cutout 0).
            if self.cutout > 0.5 && color.w < 0.5 {
                discard()
            }
            var adjusted = self.color_adjust(color.xyz, self.tint, self.color_adjust_ctl)
            // HDR lane: unlit artwork decoded to linear and pre-divided by
            // exposure, so sprites and screens keep their authored look.
            if self.lin_ctl.x > 0.5 {
                adjusted = adjusted * (adjusted * (adjusted * 0.305306011 + vec3(0.682171111, 0.682171111, 0.682171111)) + vec3(0.012522878, 0.012522878, 0.012522878)) * self.lin_ctl.z
            }
            return vec4(adjusted.x, adjusted.y, adjusted.z, color.w * self.tint.w)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}
