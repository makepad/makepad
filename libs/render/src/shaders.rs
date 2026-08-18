//! The five game draw shaders, moved verbatim from gamemaker's game_view.rs.
//! DrawSceneTexture composites the offscreen 3D pass into the host pane; the
//! cube/alpha/sky/terrain family renders the world itself.

use makepad_draw::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    mod.draw.DrawSceneTexture = mod.std.set_type_default() do #(DrawSceneTexture::script_shader(vm)){
        ..mod.draw.DrawQuad
        scene_texture: texture_2d(float)

        pixel: fn() {
            let color = self.scene_texture.sample_as_bgra(self.pos)
            return Pal.premul(color)
        }
    }

    // The game cube: DrawCube + per-instance emission and distance fog.
    //
    // The sun and fog COLOUR are uniforms, not instances. They are identical
    // for every instance in a batch, so as instance fields they cost 12
    // floats (48 bytes) per cube of pure duplication — the single largest
    // waste in the stream on a bandwidth-bound tiler. `fog_density` stays
    // per-instance because shadows switch it off individually.
    mod.draw.DrawSceneCube = mod.std.set_type_default() do #(DrawSceneCube::script_shader(vm)){
        ..mod.draw.DrawCube
        // The platform default is OFF (draw_shader.rs:41) and DrawCube does not
        // override it, so every slab, crate, ground plane and rigid body was
        // rasterising its BACK faces too — invisible, and double the fill. A
        // tiler pays per fragment and a headset pays twice again for stereo,
        // so this is the single cheapest win available on the geometry that
        // covers most of the screen. Safe because shape_geometry_data winds
        // every primitive outward and `shape_windings_face_outward` asserts it.
        backface_culling: true
        v_fog: varying(float)
        v_direct: varying(vec3f)
        v_up: varying(float)
        v_lm_uv: varying(vec2f)
        v_lm_in: varying(float)
        fog_color: uniform(vec3(0.75, 0.87, 0.96))
        sun_color: uniform(vec3(0.72, 0.72, 0.72))
        sun_sky: uniform(vec3(0.28, 0.28, 0.28))
        sun_ground: uniform(vec3(0.28, 0.28, 0.28))
        // The scene's ground light field: one planar lightmap region over
        // the whole static footprint (terrain heights ∪ box tops), addressed
        // by world xz — no per-cube data, so the packed slab layout is
        // untouched. Zero lm_rect = no lightmap, the pre-bake path.
        light_map: texture_2d(float)
        // The field's shadow-top plane (R8, same uv as light_map): per
        // texel the ABSOLUTE world height its sun ray was blocked at,
        // decoded base + byte * range from lm_top_decode (255 = lit / no
        // blocker measured). The field stores shadow at GROUND level; a
        // fragment ABOVE the blocker is out of that shadow — a raised
        // dirt ramp must not wear the fence shadows that land on the
        // grass under its footprint, and a crate carried over a rail's
        // shadow stays lit.
        top_map: texture_2d(float)
        lm_rect: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        lm_world: uniform(vec4(0.0, 0.0, 1.0, 1.0))
        lm_top_decode: uniform(vec4(0.0, 8.0, 0.0, 0.0))
        // Realtime cascaded shadow maps (shadow_csm.rs): 3 sun-depth tiles
        // side by side in one Rf32 strip. csm_p = (tier on, one tile's
        // inverse resolution, 0, 0); csm_r*N are cascade N's world->map
        // rows; csm_bias holds each cascade's z01 depth bias. When the tier
        // is on, `csm_vis` REPLACES every baked sun-visibility path — one
        // receive path for statics, dynamics and characters alike.
        csm_map: texture_2d(float)
        csm_p: uniform(vec4(0.0, 0.001, 0.0, 0.0))
        csm_bias: uniform(vec4(0.001, 0.001, 0.001, 0.0))
        csm_rx0: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry0: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz0: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        csm_rx1: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry1: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz1: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        csm_rx2: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry2: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz2: uniform(vec4(0.0, 0.0, 1.0, 0.0))

        // One PCF tap against cascade `ci`'s tile (strip u = (u + ci)/3,
        // matching shadow_csm::CSM_CASCADES = 3). Taps clamp 1.5 texels
        // inside the tile so the kernel never reads a neighbour cascade.
        csm_tap: fn(u: float, v: float, ci: float, ref01: float) -> float {
            let m = 1.5 * self.csm_p.y
            let uu = clamp(u, m, 1.0 - m)
            let vv = clamp(v, m, 1.0 - m)
            return step(ref01, self.csm_map.sample_nearest(
                vec2((uu + ci) * 0.33333333, vv)
            ).x)
        }

        // Sun visibility from the cascades: pick the tightest cascade that
        // contains the point in full XYZ. The fitted slice spheres can
        // overlap in light-space XY without sharing a depth interval, so XY
        // alone is not coverage. Then a 3x3 PCF compare. Slope-scaled bias:
        // grazing sun needs more depth slack or every curved surface acnes.
        csm_vis: fn(wp: vec3, ndl: float) -> float {
            if self.csm_p.x < 0.5 {
                return 1.0
            }
            var ci = 0.0
            var nx = dot(self.csm_rx0.xyz, wp) + self.csm_rx0.w
            var ny = dot(self.csm_ry0.xyz, wp) + self.csm_ry0.w
            var nz = dot(self.csm_rz0.xyz, wp) + self.csm_rz0.w
            var bias = self.csm_bias.x
            if max(abs(nx), abs(ny)) > 0.99 || nz < 0.0 || nz > 1.0 {
                ci = 1.0
                nx = dot(self.csm_rx1.xyz, wp) + self.csm_rx1.w
                ny = dot(self.csm_ry1.xyz, wp) + self.csm_ry1.w
                nz = dot(self.csm_rz1.xyz, wp) + self.csm_rz1.w
                bias = self.csm_bias.y
            }
            if max(abs(nx), abs(ny)) > 0.99 || nz < 0.0 || nz > 1.0 {
                ci = 2.0
                nx = dot(self.csm_rx2.xyz, wp) + self.csm_rx2.w
                ny = dot(self.csm_ry2.xyz, wp) + self.csm_ry2.w
                nz = dot(self.csm_rz2.xyz, wp) + self.csm_rz2.w
                bias = self.csm_bias.z
            }
            if max(abs(nx), abs(ny)) > 0.99 || nz < 0.0 || nz > 1.0 {
                return 1.0
            }
            let u = nx * 0.5 + 0.5
            let v = 0.5 - ny * 0.5
            let ref01 = nz - bias * (1.0 + (1.0 - clamp(ndl, 0.0, 1.0)) * 2.0)
            let e = self.csm_p.y
            var s = 0.0
            s = s + self.csm_tap(u - e, v - e, ci, ref01)
            s = s + self.csm_tap(u, v - e, ci, ref01)
            s = s + self.csm_tap(u + e, v - e, ci, ref01)
            s = s + self.csm_tap(u - e, v, ci, ref01)
            s = s + self.csm_tap(u, v, ci, ref01)
            s = s + self.csm_tap(u + e, v, ci, ref01)
            s = s + self.csm_tap(u - e, v + e, ci, ref01)
            s = s + self.csm_tap(u, v + e, ci, ref01)
            s = s + self.csm_tap(u + e, v + e, ci, ref01)
            return s / 9.0
        }
        // Per-frame dynamic lights, up to 8 (renderer.rs write_light_uniforms):
        // dl_posN = xyz + radius (0 = empty slot), dl_colN = rgb + spot amount.
        // The cube family receives TRANSIENT lights only (firework flashes,
        // host frame lights) — street lamps are already baked into the atlas
        // RGB, so summing them here would double-light every static. Computed
        // in the PIXEL stage: slabs are 8-vertex boxes, and a vertex-lit flash
        // pops whole road segments on and off.
        dl_pos0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        v_dl_pos: varying(vec3f)
        v_dl_nrm: varying(vec3f)

        // One dynamic light's contribution at world point `wp` with world
        // normal `n`. Attenuation (1 - d/r)^2; the spot factor mirrors
        // lightmap.rs's lamp pass exactly (SPILL = 0.35, squared, mixed by
        // the spot amount) with the emission axis fixed straight DOWN — the
        // harvested street lamps' convention. Empty slots (radius 0) and
        // out-of-radius fragments return early, so the common case of zero
        // active transients costs 8 uniform reads and 8 branches.
        dl_term: fn(wp: vec3, n: vec3, lp: vec4, lc: vec4) -> vec3 {
            if lp.w <= 0.0 {
                return vec3(0.0, 0.0, 0.0)
            }
            let l = lp.xyz - wp
            let d = max(length(l), 0.0001)
            if d >= lp.w {
                return vec3(0.0, 0.0, 0.0)
            }
            let att = 1.0 - d / lp.w
            let ndl = max(dot(n, l * (1.0 / d)), 0.0)
            let cone = clamp((l.y * (1.0 / d) + 0.35) / 1.35, 0.0, 1.0)
            let s = ndl * att * att * (cone * cone * lc.w + (1.0 - lc.w))
            return lc.xyz * s
        }

        dl_sum: fn(wp: vec3, n: vec3) -> vec3 {
            var dl = vec3(0.0, 0.0, 0.0)
            dl = dl + self.dl_term(wp, n, self.dl_pos0, self.dl_col0)
            dl = dl + self.dl_term(wp, n, self.dl_pos1, self.dl_col1)
            dl = dl + self.dl_term(wp, n, self.dl_pos2, self.dl_col2)
            dl = dl + self.dl_term(wp, n, self.dl_pos3, self.dl_col3)
            dl = dl + self.dl_term(wp, n, self.dl_pos4, self.dl_col4)
            dl = dl + self.dl_term(wp, n, self.dl_pos5, self.dl_col5)
            dl = dl + self.dl_term(wp, n, self.dl_pos6, self.dl_col6)
            dl = dl + self.dl_term(wp, n, self.dl_pos7, self.dl_col7)
            return dl
        }

        vertex: fn() {
            let pos = self.get_size() * self.geom.geom_pos + self.get_pos()
            // TRUE world position first (the stage/view transform must not
            // move the light field), then the view-space chain on top.
            let wpos = self.transform * vec4(pos.x, pos.y, pos.z, 1.0)
            let model_view = self.draw_list.view_transform * self.transform
            let normal4 = model_view * vec4(
                self.geom.geom_normal.x,
                self.geom.geom_normal.y,
                self.geom.geom_normal.z,
                0.0
            )
            let normal = normalize(normal4.xyz)
            self.world = self.draw_list.view_transform * wpos
            let view_pos = self.draw_pass.camera_view * self.world
            let dp = max(dot(normal, normalize(self.light_dir)), 0.0)
            self.lit_color = self.get_color(dp, normal.y)
            // The direct sun term rides its own varying so the PIXEL stage
            // can gate it by the baked sun-visibility SDF.
            self.v_direct = self.color.xyz * (self.sun_color * dp)
            self.v_up = normal.y
            let lw = max(self.lm_world.zw, vec2(0.000001, 0.000001))
            let lraw = (wpos.xz - self.lm_world.xy) / lw
            let lf = clamp(lraw, vec2(0.0, 0.0), vec2(1.0, 1.0))
            self.v_lm_uv = self.lm_rect.xy + lf * self.lm_rect.zw
            // Outside the field: fully lit, never a clamp-smeared border.
            self.v_lm_in = step(0.0, lraw.x) * step(lraw.x, 1.0)
                * step(0.0, lraw.y) * step(lraw.y, 1.0)
            // TRUE world position + normal for the pixel-stage dynamic
            // lights (the stage/view transform must not move a light).
            // Cube faces are flat, so the interpolated normal is constant
            // per face and needs no per-fragment renormalize.
            self.v_dl_pos = wpos.xyz
            self.v_dl_nrm = normalize((self.transform * vec4(
                self.geom.geom_normal.x,
                self.geom.geom_normal.y,
                self.geom.geom_normal.z,
                0.0
            )).xyz)
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        // One lighting model for every game shader (sun.rs): hemisphere
        // ambient by surface-up-ness plus emission. The sun's DIRECT term
        // moved to v_direct (gated per fragment by the light field); with
        // the default flat sun this composes to exactly the old constants.
        get_color: fn(dp: float, nrm_y: float) {
            let hemi = clamp(nrm_y * 0.5 + 0.5, 0.0, 1.0)
            let ambient = mix(self.sun_ground, self.sun_sky, hemi)
            let lit = self.color.xyz * ambient
            // Emission: glowing eyes, beacons, bolts (energy ramps at runtime).
            let glowing = lit + self.color.xyz * self.glow * 0.6
            return vec4(glowing, self.color.w)
        }

        pixel: fn() {
            // Baked ground light on UP-facing fragments only (feathered by
            // up-ness): the field stores what lands on top surfaces at that
            // xz — walls would smear it vertically. Dynamics sample it too,
            // deliberately: a crate rolling through a house's shadow should
            // darken, and this is the only shadow-receiving dynamics get.
            let lm = self.light_map.sample_as_bgra(self.v_lm_uv)
            let has_lm = step(0.000001, self.lm_rect.z)
                * clamp(self.v_up * 4.0, 0.0, 1.0) * self.v_lm_in
            // Shadow-top comparison: the A channel says what reaches the
            // GROUND at this xz, the R8 plane says how high its blocker
            // sits. A fragment above the blocker rejects the ground's
            // shadow; at ground level top_h is above the fragment and this
            // collapses to the old behaviour.
            let top_h = self.lm_top_decode.x
                + self.top_map.sample(self.v_lm_uv).x * self.lm_top_decode.y
            let occ = 1.0 - smoothstep(top_h - 0.15, top_h + 0.15, self.v_dl_pos.y)
            // Realtime: the cascades replace the whole baked ground path.
            let ndl_c = max(dot(normalize(self.v_dl_nrm), normalize(self.light_dir)), 0.0)
            let sun_vis = mix(
                mix(1.0, smoothstep(0.2, 0.8, lm.w), has_lm * occ),
                self.csm_vis(self.v_dl_pos, ndl_c),
                self.csm_p.x
            )
            let lamps = lm.xyz * (2.0 * has_lm)
            let dl = self.dl_sum(self.v_dl_pos, self.v_dl_nrm)
            let c = self.lit_color.xyz + self.v_direct * sun_vis
                + self.color.xyz * (lamps + dl)
            let fogged = mix(c, self.fog_color, self.v_fog)
            return vec4(fogged, self.lit_color.w)
        }
    }

    // Same shading, alpha-blended: water, sensor ghosts, blob shadows, and the
    // particle batch.
    mod.draw.DrawSceneAlpha = mod.std.set_type_default() do #(DrawSceneAlpha::script_shader(vm)){
        ..mod.draw.DrawSceneCube
        alpha_blend: true
        // DELIBERATE, do not "fix": this batch carries flat single-sided
        // geometry — blob shadows and water surfaces — whose winding is not
        // guaranteed to face the viewer, and culling a blended surface changes
        // the composite rather than merely hiding a hidden face. Overriding the
        // `true` now inherited from DrawSceneCube.
        backface_culling: false
    }

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

    // Video screen: ONE upright textured quad in world space, fed a texture
    // the host updates per frame (in-world video playback). Reuses the shared
    // flare quad geometry (geom_pos.xy = corner in -0.5..0.5, geom_uv 0..1).
    // Opaque and depth-written, so the world occludes it like any solid.
    mod.draw.DrawSceneScreen = mod.std.set_type_default() do #(DrawSceneScreen::script_shader(vm)){
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
            let world3 = vec3(
                center.x + right.x * corner.x * self.screen_size.x,
                center.y + corner.y * self.screen_size.y,
                center.z + right.z * corner.x * self.screen_size.x
            )
            self.world = self.draw_list.view_transform
                * vec4(world3.x, world3.y, world3.z, 1.0)
            // Video rows are top-first; quad uv.y grows upward. Flip v.
            self.v_uv = vec2(self.geom.geom_uv.x, 1.0 - self.geom.geom_uv.y)
            self.vertex_pos = self.draw_pass.camera_projection
                * (self.draw_pass.camera_view * self.world)
            return self.vertex_pos
        }

        pixel: fn() {
            let color = self.tex.sample_as_bgra(self.v_uv)
            return vec4(color.x, color.y, color.z, 1.0)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // Sky dome: a big cube around the camera, gradient by view direction
    // (the Godot ProceduralSkyMaterial look).
    mod.draw.DrawSceneSky = mod.std.set_type_default() do #(DrawSceneSky::script_shader(vm)){
        ..mod.draw.DrawCube
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
            let color = mix(ground, sky, step(0.0, y))
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
        ..mod.draw.DrawCube
        // Same deliberate choice as DrawSceneSky: the camera sits INSIDE
        // the dome, every visible face is a back face.
        backface_culling: false
        // Night-sky panorama (equirectangular; NASA SVS Deep Star Map —
        // see the sandbox's resources/sky/ATTRIBUTION.txt). A 1x1 black
        // stand-in binds when no map is loaded; star_r0.w gates it.
        star_tex: texture_2d(float)
        v_dir: varying(vec3f)

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
            // The Preetham daylight model. Everything sun/turbidity-
            // dependent arrives precomputed from sky.rs; this evaluates
            // the per-pixel Perez product in Yxy, converts to linear sRGB
            // and tone-maps (see sky.rs docs).
            let ct = max(v.y, 0.01)
            let sun_d = self.sun_e.xyz
            let cg = clamp(dot(v, sun_d), 0.0 - 1.0, 1.0)
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
            // Reinhard on LUMINANCE (chromaticity survives), then
            // XYZ -> linear sRGB, hue-preserving normalise, display
            // gamma — the mirror of sky.rs's yxy_to_rgb.
            var yt = max(yl * self.sun_e.w, 0.0)
            yt = yt / (1.0 + yt)
            let bx = xc * (yt / yc)
            let bz = (1.0 - xc - yc) * (yt / yc)
            let r = max(3.2406 * bx - 1.5372 * yt - 0.4986 * bz, 0.0)
            let gr = max((0.0 - 0.9689) * bx + 1.8758 * yt + 0.0415 * bz, 0.0)
            let b = max(0.0557 * bx - 0.204 * yt + 1.057 * bz, 0.0)
            let m = max(max(r, gr), max(b, 1.0))
            var day = pow(vec3(r / m, gr / m, b / m), vec3(0.4545, 0.4545, 0.4545))
            // Below the horizon the model has no answer: carry the
            // horizon colour down, dimmed — it reads as ground haze
            // wherever terrain does not already cover the dome.
            day = day * mix(1.0, 0.35, clamp((0.0 - v.y) * 3.0, 0.0, 1.0))
            // A visible sun that SETS properly — the robobo1221 recipe
            // (shadertoy atmospheric scattering): NO hand-tuned fades.
            // The disc is a sharp smoothstep limb at high intensity, and
            // everything else is ABSORPTION physics — optical depth
            // blows up near the horizon (density / pow(elevation, 3/4)),
            // transmittance exp(-skyColor * depth) kills blue first, so
            // the disc reddens and dims through thickening air while the
            // horizon slices a chord through its sharp limb: semicircle,
            // sliver, gone. The broad Mie glow is tinted by the SUN's
            // own absorption, so the afterglow reds out and dies
            // naturally as the sun sinks — never a dot dimming in place.
            // All horizon references sit at the VISIBLE ground line, not
            // eye level: a standing camera sees flat ground meet the sky
            // ~2 degrees BELOW v.y = 0, so cutting or absorbing at 0 made
            // the sun vanish visibly above the line ("sets halfway into
            // the sky"). The 0.033 rad offset drops the disc cut and both
            // absorption knees to where the ground edge actually is;
            // terrain z-wins over the dome wherever it covers, so the
            // offset only shapes the open haze band.
            let nb = self.zenith.w
            // The disc's absorption floor is the HORIZON-CONTACT optical
            // depth (~9, ballpark of real ~38 air masses scaled to taste)
            // — with an unbounded divergence the atmosphere annihilated
            // the disc in the last half degree above the line, so it
            // dimmed to nothing while touching it ("stuck at the horizon,
            // fades out") and the geometric slice below was never seen.
            // At contact the disc must still be a vivid red circle; the
            // horizon then cuts it: semicircle, sliver, gone.
            // Disc/Mie/afterglow follow the TRUE sun (sun_e is clamped
            // ~2 deg up for the Perez normaliser — painting the disc
            // around it froze the sun at the horizon while it faded).
            let sun_t = self.sun_true.xyz
            let gt = acos(clamp(dot(v, sun_t), 0.0 - 1.0, 1.0))
            let absf = exp(vec3(0.39, 0.57, 1.0)
                * (0.0 - 0.485 / pow(max(v.y + 0.033, 0.02), 0.75))) * 2.0
            let abss = exp(vec3(0.39, 0.57, 1.0)
                * (0.0 - 0.485 / pow(max(sun_t.y + 0.033, 0.012), 0.75))) * 2.0
            // NOT the reference's reversed-edge smoothstep(hi, lo, x) —
            // edge0 > edge1 is undefined on Metal and returns no disc.
            let limb = 1.0 - smoothstep(0.048, 0.055, gt)
            let mie_d = clamp(1.0 - pow(gt * 0.55, 0.1), 0.0, 1.0)
            let mie = mie_d * mie_d * (3.0 - 2.0 * mie_d) * 1.4
            // x20 core (the reference uses x50): saturated through the
            // absorption right down to the cut — the framebuffer clamp is
            // the tonemap here, a blinding disc is the point.
            day = day + (absf * (limb * 20.0) + abss * mie)
                * clamp((v.y + 0.033) * 90.0 + 0.5, 0.0, 1.0)
            // Night: near-pitch-black dome (the star field carries it), a
            // whisper of blue kept so terrain still separates at the
            // horizon.
            let nsky = mix(
                vec3(0.010, 0.012, 0.020),
                vec3(0.002, 0.003, 0.006),
                clamp(v.y * 1.4, 0.0, 1.0)
            )
            // Stars: rotate the view dir into the celestial frame (the
            // day cycle wheels the dome around the sun arc's pole, so the
            // sky turns with the earth), sample the equirect panorama,
            // fade in with night and cut below the horizon.
            let sd = vec3(
                self.star_r0.x * v.x + self.star_r0.y * v.y + self.star_r0.z * v.z,
                self.star_r1.x * v.x + self.star_r1.y * v.y + self.star_r1.z * v.z,
                self.star_r2.x * v.x + self.star_r2.y * v.y + self.star_r2.z * v.z
            )
            let su = atan2(sd.z, sd.x) * 0.15915494 + 0.5
            let sv = 0.5 - asin(clamp(sd.y, 0.0 - 1.0, 1.0)) * 0.31830989
            let star_fade = nb * self.star_r0.w * clamp(v.y * 6.0 + 0.1, 0.0, 1.0)
            // The Milky Way is a series of PINPRICKS with high contrast —
            // no equirect photo can deliver that magnified across a dome,
            // so the NASA panorama serves as the DENSITY MAP and the
            // stars themselves are procedural points: per-cell hash
            // candidates whose keep-threshold follows the map's
            // luminance (dense sharp grains inside the band, lone stars
            // in the void), sharp radial falloff, hashed brightness. A
            // second, sparser layer adds the scattered bright stars. All
            // in the ROTATED frame (su/sv) so everything wheels with the
            // earth; only a faint residual of the photo remains as
            // nebulosity behind the points.
            let smap = self.star_tex.sample_as_bgra(vec2(su, sv)).xyz
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
            // Dome-anchored hash dither: the near-black night gradient is
            // the worst 8-bit banding case on the whole dome. ±1 LSB.
            let hash = fract(
                sin(dot(v.xy + v.zz, vec2(12.9898, 78.233))) * 43758.5453
            )
            return vec4(
                mix(day, nsky, nb) + stars
                    + vec3(1.0, 1.0, 1.0) * ((hash - 0.5) * 0.008),
                1.0
            )
        }
    }

    // Skinned character mesh: PbrVertex stream (CPU-skinned per frame, uv in
    // ny_nz_uv.zw), textured, lit and fogged like the terrain.
    mod.draw.DrawSceneSkinned = mod.std.set_type_default() do #(DrawSceneSkinned::script_shader(vm)){
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertexAo, geom.GameMeshAoGeom)
        tex: texture_2d(float)
        // Baked occlusion for this pack, sampled per FRAGMENT. Per vertex it
        // would carry exactly as much information as a vertex bake, which is
        // the thing the atlas exists to escape.
        ao_map: texture_2d(float)
        // The scene's baked-light atlas (lightmap.rs): A = sun-visibility
        // SDF, RGB = lamp light x0.5. Every static draw binds it (a 1x1
        // "fully lit" stand-in before the first bake delivers).
        light_map: texture_2d(float)
        // The ground field's shadow-top plane (R8, same uv as the ground
        // region): the ABSOLUTE height each shadowed texel's sun ray was
        // blocked at, decoded via lm_top_decode. Dynamics compare their
        // vertex height against it so a crate lifted above a fence rail's
        // shadow comes out of it (see DrawSceneCube).
        top_map: texture_2d(float)
        v_ao_uv: varying(vec2f)
        v_lm_uv: varying(vec2f)
        v_ambient: varying(vec3f)
        v_direct: varying(vec3f)
        v_uv: varying(vec2f)
        v_tint: varying(vec4f)
        world: varying(vec4f)
        v_fog: varying(float)
        // Per-frame dynamic lights, up to 8, summed in the VERTEX stage
        // (props carry enough vertices for that to read smoothly). Slot
        // layout from renderer.rs write_light_uniforms: TRANSIENT lights
        // (firework flashes, host lights) occupy slots [0, dl_split);
        // baked street lamps fill the rest. Statics (dl_apply = 0) sum only
        // the transient prefix — their lamp light is already in the baked
        // atlas and adding it again would double-light every facade —
        // while dynamic instances (dl_apply = 1) sum everything.
        dl_split: uniform(0.0)
        dl_pos0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        v_dl: varying(vec3f)
        // The GROUND region of the light atlas, for DYNAMIC instances only
        // (dl_apply gates it): a driven car crossing a house's shadow
        // darkens. A channel only — statics gate their sun through their
        // own chart region, and nobody reads the ground RGB here (lamps for
        // dynamics arrive analytically through dl_*).
        lm_ground_rect: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        lm_ground_world: uniform(vec4(0.0, 0.0, 1.0, 1.0))
        // Decode for top_map: x = base world height, y = range; absolute
        // blocked height = x + byte * y.
        lm_top_decode: uniform(vec4(0.0, 8.0, 0.0, 0.0))
        // xy = ground-region uv, z = in-field gate, w = TRUE world height
        // of the vertex (for the shadow-top comparison).
        v_lmg: varying(vec4f)
        // Realtime cascades (see DrawSceneCube's block — same contract, same
        // uniforms, one receive path for every family). v_csm = (true world
        // position, N.L) for the pixel-stage compare.
        csm_map: texture_2d(float)
        // Q3 / Unreal detail overlay. Last texture so CSM stays slot 4.
        detail_map: texture_2d(float)
        detail_st: instance(vec2(0.0, 0.0))
        // 1 = COLOR_0 is a baked lightmap (Q3 worlds). Analytic sun
        // leaves inward vaults black; the tint already *is* the light.
        prelit: instance(0.0)
        csm_p: uniform(vec4(0.0, 0.001, 0.0, 0.0))
        csm_bias: uniform(vec4(0.001, 0.001, 0.001, 0.0))
        csm_rx0: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry0: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz0: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        csm_rx1: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry1: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz1: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        csm_rx2: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry2: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz2: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        v_csm: varying(vec4f)

        csm_tap: fn(u: float, v: float, ci: float, ref01: float) -> float {
            let m = 1.5 * self.csm_p.y
            let uu = clamp(u, m, 1.0 - m)
            let vv = clamp(v, m, 1.0 - m)
            return step(ref01, self.csm_map.sample_nearest(
                vec2((uu + ci) * 0.33333333, vv)
            ).x)
        }

        csm_vis: fn(wp: vec3, ndl: float) -> float {
            if self.csm_p.x < 0.5 {
                return 1.0
            }
            var ci = 0.0
            var nx = dot(self.csm_rx0.xyz, wp) + self.csm_rx0.w
            var ny = dot(self.csm_ry0.xyz, wp) + self.csm_ry0.w
            var nz = dot(self.csm_rz0.xyz, wp) + self.csm_rz0.w
            var bias = self.csm_bias.x
            if max(abs(nx), abs(ny)) > 0.99 || nz < 0.0 || nz > 1.0 {
                ci = 1.0
                nx = dot(self.csm_rx1.xyz, wp) + self.csm_rx1.w
                ny = dot(self.csm_ry1.xyz, wp) + self.csm_ry1.w
                nz = dot(self.csm_rz1.xyz, wp) + self.csm_rz1.w
                bias = self.csm_bias.y
            }
            if max(abs(nx), abs(ny)) > 0.99 || nz < 0.0 || nz > 1.0 {
                ci = 2.0
                nx = dot(self.csm_rx2.xyz, wp) + self.csm_rx2.w
                ny = dot(self.csm_ry2.xyz, wp) + self.csm_ry2.w
                nz = dot(self.csm_rz2.xyz, wp) + self.csm_rz2.w
                bias = self.csm_bias.z
            }
            if max(abs(nx), abs(ny)) > 0.99 || nz < 0.0 || nz > 1.0 {
                return 1.0
            }
            let u = nx * 0.5 + 0.5
            let v = 0.5 - ny * 0.5
            let ref01 = nz - bias * (1.0 + (1.0 - clamp(ndl, 0.0, 1.0)) * 2.0)
            let e = self.csm_p.y
            var s = 0.0
            s = s + self.csm_tap(u - e, v - e, ci, ref01)
            s = s + self.csm_tap(u, v - e, ci, ref01)
            s = s + self.csm_tap(u + e, v - e, ci, ref01)
            s = s + self.csm_tap(u - e, v, ci, ref01)
            s = s + self.csm_tap(u, v, ci, ref01)
            s = s + self.csm_tap(u + e, v, ci, ref01)
            s = s + self.csm_tap(u - e, v + e, ci, ref01)
            s = s + self.csm_tap(u, v + e, ci, ref01)
            s = s + self.csm_tap(u + e, v + e, ci, ref01)
            return s / 9.0
        }

        // One dynamic light at world point `wp`, world normal `n`.
        // Attenuation (1 - d/r)^2; the spot factor mirrors lightmap.rs's
        // lamp pass (SPILL = 0.35, squared, mixed by lc.w) with the
        // emission axis fixed straight DOWN — the street-lamp convention.
        dl_term: fn(wp: vec3, n: vec3, lp: vec4, lc: vec4) -> vec3 {
            if lp.w <= 0.0 {
                return vec3(0.0, 0.0, 0.0)
            }
            let l = lp.xyz - wp
            let d = max(length(l), 0.0001)
            if d >= lp.w {
                return vec3(0.0, 0.0, 0.0)
            }
            let att = 1.0 - d / lp.w
            let ndl = max(dot(n, l * (1.0 / d)), 0.0)
            let cone = clamp((l.y * (1.0 / d) + 0.35) / 1.35, 0.0, 1.0)
            let s = ndl * att * att * (cone * cone * lc.w + (1.0 - lc.w))
            return lc.xyz * s
        }

        // The 8-slot sum with the per-instance static gate: slot i counts
        // when the instance is dynamic (dl_apply = 1) OR i < dl_split.
        dl_sum_gated: fn(wp: vec3, n: vec3) -> vec3 {
            var dl = vec3(0.0, 0.0, 0.0)
            let g = self.dl_apply
            dl = dl + self.dl_term(wp, n, self.dl_pos0, self.dl_col0)
                * clamp(g + step(0.5, self.dl_split), 0.0, 1.0)
            dl = dl + self.dl_term(wp, n, self.dl_pos1, self.dl_col1)
                * clamp(g + step(1.5, self.dl_split), 0.0, 1.0)
            dl = dl + self.dl_term(wp, n, self.dl_pos2, self.dl_col2)
                * clamp(g + step(2.5, self.dl_split), 0.0, 1.0)
            dl = dl + self.dl_term(wp, n, self.dl_pos3, self.dl_col3)
                * clamp(g + step(3.5, self.dl_split), 0.0, 1.0)
            dl = dl + self.dl_term(wp, n, self.dl_pos4, self.dl_col4)
                * clamp(g + step(4.5, self.dl_split), 0.0, 1.0)
            dl = dl + self.dl_term(wp, n, self.dl_pos5, self.dl_col5)
                * clamp(g + step(5.5, self.dl_split), 0.0, 1.0)
            dl = dl + self.dl_term(wp, n, self.dl_pos6, self.dl_col6)
                * clamp(g + step(6.5, self.dl_split), 0.0, 1.0)
            dl = dl + self.dl_term(wp, n, self.dl_pos7, self.dl_col7)
                * clamp(g + step(7.5, self.dl_split), 0.0, 1.0)
            return dl
        }

        // Octahedral decode: the inverse of skin.rs's oct_encode. Two f16
        // lanes carry a unit normal that would otherwise cost three floats.
        // sign is inlined rather than shared: the builtin sign() returns 0 at
        // 0, which would collapse the fold on an axis-aligned normal.
        oct_decode: fn(e: vec2f) -> vec3f {
            let nz = 1.0 - abs(e.x) - abs(e.y)
            let t = max(0.0 - nz, 0.0)
            // step(0,v)*2-1 is +1 for v>=0 and -1 for v<0, branchless and
            // without the sign() builtin's zero case.
            let sx = step(0.0, e.x) * 2.0 - 1.0
            let sy = step(0.0, e.y) * 2.0 - 1.0
            return normalize(vec3(e.x - t * sx, e.y - t * sy, nz))
        }

        vertex: fn() {
            let pos = vec3(self.geom.px, self.geom.py, self.geom.pz)
            // ao_uv is unorm16x2 (model.rs pack_ao_uv), NOT an f16 pair — f16
            // spacing near 1.0 is a full texel of a 1024 atlas. Each axis is
            // (lo + 256*hi)/257 of the two unpacked bytes: 255*257 = 65535.
            let ao_uv_b = unpack4u8(self.geom.ao_uv)
            self.v_ao_uv = vec2(
                (ao_uv_b.x + ao_uv_b.y * 256.0) / 257.0,
                (ao_uv_b.z + ao_uv_b.w * 256.0) / 257.0
            )
            // The lightmap REUSES the chart parameterisation: this instance's
            // atlas window is one offset/scale over the same uv.
            self.v_lm_uv = self.lm_rect.xy + self.v_ao_uv * self.lm_rect.zw
            let normal_in = self.oct_decode(unpack2f16(self.geom.nrm))
            let model_view = self.draw_list.view_transform * self.transform
            let world_normal = normalize((model_view * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let dp = max(dot(world_normal, normalize(self.light_dir)), 0.0)
            let hemi = clamp(world_normal.y * 0.5 + 0.5, 0.0, 1.0)
            self.v_ambient = mix(self.sun_ground, self.sun_sky, hemi)
            self.v_direct = self.sun_color * dp
            // Dynamic lights in TRUE world space (pre stage/view transform —
            // light positions are world coordinates, and the stage must not
            // move them).
            let dl_wp = (self.transform * vec4(pos.x, pos.y, pos.z, 1.0)).xyz
            let dl_n = normalize((self.transform * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
            self.v_dl = self.dl_sum_gated(dl_wp, dl_n)
            // Ground-field sun shadow for DYNAMIC instances (dl_apply = 1).
            // The field stores GROUND-level visibility, so the sample is
            // projected ALONG THE SUN RAY from this vertex down to the
            // instance's ground plane — a vertex at height h is shadowed
            // iff the sun ray through it lands on shadowed ground. This is
            // what slants the boundary across a body correctly and stops a
            // wall's shadow at its feet from climbing the whole object.
            let dl_h = max(dl_wp.y - self.ground_y, 0.0)
            let dl_sun = normalize(self.light_dir)
            let dl_gxz = dl_wp.xz - dl_sun.xz * (dl_h / max(dl_sun.y, 0.2))
            let lgw = max(self.lm_ground_world.zw, vec2(0.000001, 0.000001))
            let lgraw = (dl_gxz - self.lm_ground_world.xy) / lgw
            let lgf = clamp(lgraw, vec2(0.0, 0.0), vec2(1.0, 1.0))
            let lg_in = self.dl_apply * step(0.000001, self.lm_ground_rect.z)
                * step(0.0, lgraw.x) * step(lgraw.x, 1.0)
                * step(0.0, lgraw.y) * step(lgraw.y, 1.0)
            let lg_uv = self.lm_ground_rect.xy + lgf * self.lm_ground_rect.zw
            self.v_lmg = vec4(lg_uv.x, lg_uv.y, lg_in, dl_wp.y)
            self.v_csm = vec4(dl_wp.x, dl_wp.y, dl_wp.z, dp)
            self.v_uv = unpack2f16(self.geom.uv)
            // rgb is the material tint (x the per-character wash); the ALPHA
            // lane carries baked self-AO from model.rs, not opacity — this
            // shader has always returned opaque, so the lane was free.
            let vc = unpack4u8(self.geom.color)
            self.v_tint = vec4(vc.x * self.tint.x, vc.y * self.tint.y, vc.z * self.tint.z, vc.w)
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            // Depth-tie breaker: uniform view-space scale toward the camera.
            // The perspective divide cancels it in x/y (the image does not
            // move); only the stored depth shifts, so coplanar stacked
            // pieces resolve by placement order instead of z-fighting.
            let zk = 1.0 - self.depth_bias
            self.vertex_pos = self.draw_pass.camera_projection
                * vec4(view_pos.x * zk, view_pos.y * zk, view_pos.z * zk, view_pos.w)
        }

        pixel: fn() {
            // Atlas x vertex tint. Kenney ships both conventions — most packs
            // UV-map into one colormap (tint = white), nature-kit and friends
            // carry no texture and colour per material (atlas = white 1x1).
            // Multiplying serves both without a branch or a second shader.
            // REPEAT + raw UVs (not fract): fract() wraps in software but
            // explodes screen-space derivatives at every tile seam, so the
            // GPU picks the tiniest mip and distant walls turn to noise.
            let tex = self.tex.sample_as_bgra_repeat(self.v_uv)
            // BUILD punch-through: palette 255 / magenta is the overlay key.
            let magenta = (tex.x > 0.75) && (tex.z > 0.75) && (tex.y < 0.22)
            if tex.w < 0.5 || magenta {
                discard()
            }
            var albedo = vec3(tex.x * self.v_tint.x, tex.y * self.v_tint.y, tex.z * self.v_tint.z)
            // Detail: blendFunc GL_DST_COLOR GL_SRC_COLOR = 2 * dest * src.
            // Mean-127 overlay is identity; far mips go gray and drop out.
            if self.detail_st.x > 0.001 {
                let det = self.detail_map.sample_as_bgra_repeat(self.v_uv * self.detail_st)
                albedo = vec3(albedo.x * det.x * 2.0, albedo.y * det.y * 2.0, albedo.z * det.z * 2.0)
            }
            // AO scales AMBIENT only. Ambient is light arriving from
            // everywhere, which is exactly what a crevice blocks; direct
            // sunlight is already zero where the surface faces away. Folding
            // it into both would darken a lit wall twice for the same reason.
            // Occlusion from the ATLAS when the pack has one, else from the
            // vertex lane. Both live in [AO_FLOOR, 1].
            let baked = self.ao_map.sample(self.v_ao_uv).x
            // Dithered: the atlas is 8-bit and magnified well past a texel per
            // pixel, so a shallow wall gradient otherwise lands as visible
            // bands of piecewise-linear bilinear. Hash noise anchored in WORLD
            // space (screen-anchored grain swims when the camera moves) at
            // ±1.5% breaks the bands without reading as dirt on flat colour.
            let hash = fract(
                sin(dot(self.world.xy + self.world.zz, vec2(12.9898, 78.233))) * 43758.5453
            )
            let ao = clamp(
                mix(self.v_tint.w, baked, self.ao_enabled) + (hash - 0.5) * 0.03,
                0.0, 1.0
            )
            // AO scales ambient FULLY and direct partially. Ambient-only is
            // the physically tidy answer and it is why the bake was invisible:
            // ambient is about a quarter of the light here, so even a properly
            // dark corner moved the pixel by a few percent. Letting occlusion
            // take some of the direct term too is what every stylised renderer
            // does, and it is what makes a crease read as a crease.
            let ao_direct = mix(1.0, ao, 0.75)
            // Baked light: A gates the analytic sun through a smoothstep over
            // the signed-distance field — the penumbra width is the decode
            // WINDOW ([`LM_SUN_SOFT`]), a runtime knob, not a bake product.
            // RGB adds the lamps (x2: half range stored for overbright).
            let lm = self.light_map.sample_as_bgra(self.v_lm_uv)
            let has_lm = step(0.000001, self.lm_rect.z)
            let sun_vis = mix(1.0, smoothstep(0.2, 0.8, lm.w), has_lm)
            // Dynamics gate their sun through the GROUND region instead
            // (statics have v_lmg.z = 0, dynamics have lm_rect = 0, so the
            // two gates never both engage). The shadow-top plane rejects
            // the ground's shadow for vertices ABOVE the blocker along the
            // sun ray: a fence rail shades shins, never the head over it.
            let lmg = self.light_map.sample_as_bgra(self.v_lmg.xy)
            let top_g = self.lm_top_decode.x
                + self.top_map.sample(self.v_lmg.xy).x * self.lm_top_decode.y
            let occ_g = 1.0 - smoothstep(top_g - 0.15, top_g + 0.15, self.v_lmg.w)
            let sun_vis_g = mix(1.0, smoothstep(0.2, 0.8, lmg.w), self.v_lmg.z * occ_g)
            // Realtime: the cascades replace BOTH baked gates (own chart
            // and ground projection) — one receive path for every family.
            let sun_all = mix(
                sun_vis * sun_vis_g,
                self.csm_vis(self.v_csm.xyz, self.v_csm.w),
                self.csm_p.x
            )
            let lamps = lm.xyz * (2.0 * has_lm)
            let analytic = self.v_ambient * ao
                + self.v_direct * (ao_direct * sun_all)
                + (lamps + self.v_dl) * ao_direct
            // prelit: albedo already carries COLOR_0 = LM×4. Multiplying
            // the sun again zeros any face that looks inward or down.
            let lit = albedo * mix(analytic, vec3(1.0, 1.0, 1.0), self.prelit)
            // AO DEBUG: show baked occlusion alone, contrast-stretched. AO
            // lives in [AO_FLOOR, 1] (0.52..1), so raw it is a wash of pale
            // greys and judging whether a 90-degree corner actually darkens is
            // guesswork. Remapped to full black-to-white, a corner that works
            // is unmistakable and one that does not is equally so.
            if self.ao_debug > 0.5 {
                // HARD PINK, heavily accentuated. Greyscale AO on grey-white
                // Kenney walls is unreadable — the whole reason the last three
                // bakes looked "a bit smudgy" is that a real defect and a
                // correct result differ by a few percent of luminance. Hue
                // separates occlusion from albedo completely, and the cube
                // curve pushes even slight darkening to saturation, so where
                // AO does anything at all it is obvious.
                // LINEAR over AO's actual range. An earlier version cubed this
                // to make faint occlusion obvious and that made the view lie:
                // a barely-shaded wall at ao=0.9 came out 37% pink, so the
                // whole house read as heavily occluded while the atlas was in
                // fact 74% unoccluded. A debug view that exaggerates is worse
                // than none — it hides the very problem it is there to show.
                let occ = clamp((1.0 - ao) / 0.70, 0.0, 1.0)
                return vec4(mix(vec3(1.0, 1.0, 1.0), vec3(1.0, 0.0, 0.55), occ), 1.0)
            }
            // LM DEBUG: the ACTIVE sun tier alone — red in shadow, green in
            // sun, lamps added as their own colour on top. Albedo suppressed
            // so a faint lamp or a misplaced shadow reads instantly.
            if self.lm_debug > 0.5 {
                return vec4(
                    mix(vec3(0.6, 0.1, 0.1), vec3(0.1, 0.6, 0.1), sun_all) + lamps,
                    1.0
                )
            }
            return vec4(mix(lit, self.fog_color, self.v_fog), 1.0)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // Camera-space FPS held mesh. This is intentionally a small sibling of
    // DrawSceneSkinned, not a mode inside the world shader: view geometry gets
    // one texture sample and analytic daylight, and has no lightmap, top-map,
    // CSM, fog or dynamic-light instructions to execute on low-end devices.
    mod.draw.DrawSceneViewModel = mod.std.set_type_default() do #(DrawSceneViewModel::script_shader(vm)){
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertexAo, geom.GameMeshAoGeom)
        tex: texture_2d(float)
        v_uv: varying(vec2f)
        v_color: varying(vec3f)

        oct_decode: fn(e: vec2f) -> vec3f {
            let nz = 1.0 - abs(e.x) - abs(e.y)
            let t = max(0.0 - nz, 0.0)
            let sx = step(0.0, e.x) * 2.0 - 1.0
            let sy = step(0.0, e.y) * 2.0 - 1.0
            return normalize(vec3(e.x - t * sx, e.y - t * sy, nz))
        }

        vertex: fn() {
            let pos = vec3(self.geom.px, self.geom.py, self.geom.pz)
            let normal_in = self.oct_decode(unpack2f16(self.geom.nrm))
            let model_view = self.draw_list.view_transform * self.transform
            let normal = normalize((model_view * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
            let world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * world
            let clip = self.draw_pass.camera_projection * view_pos
            let dp = max(dot(normal, normalize(self.light_dir)), 0.0)
            let hemi = clamp(normal.y * 0.5 + 0.5, 0.0, 1.0)
            let vc = unpack4u8(self.geom.color)
            self.v_color = vc.xyz * (
                mix(self.sun_ground, self.sun_sky, hemi) * vc.w
                + self.sun_color * dp * mix(1.0, vc.w, 0.35)
            )
            self.v_uv = unpack2f16(self.geom.uv)
            // Portable late overlay: 0..w clip depth is valid on Metal/D3D/
            // Vulkan and also inside GL's -w..w range. Retaining a tiny slice
            // of original depth preserves the pistol's triangle ordering.
            let original_01 = clamp(clip.z / clip.w * 0.5 + 0.5, 0.0, 1.0)
            self.vertex_pos = vec4(
                clip.x,
                clip.y,
                clip.w * (0.0001 + original_01 * 0.0008),
                clip.w
            )
        }

        pixel: fn() {
            let tex = self.tex.sample_as_bgra(self.v_uv)
            return vec4(tex.xyz * self.v_color, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // GPU-skinned character mesh: the REST mesh (geom.GameMeshVertexSkin,
    // uploaded once per rig) blended in the VERTEX stage against a joint
    // palette texture — 3 RGBA32F texels per joint, the top three rows of
    // each 3x4 matrix, all characters packed into one texture per frame with
    // a per-instance texel offset. What used to be a full posed vertex
    // stream per character per frame is now its palette.
    //
    // Deliberately a SIBLING of DrawSceneSkinned rather than a flag inside it
    // (the DrawSceneFoliage pattern): props draw with that shader and have no
    // joints, and this one costs up to 12 vertex texture fetches that the
    // static world must never pay. Lighting/fog match DrawSceneSkinned minus
    // the AO path — a deforming mesh cannot carry a baked occlusion atlas.
    mod.draw.DrawSceneSkinnedGpu = mod.std.set_type_default() do #(DrawSceneSkinnedGpu::script_shader(vm)){
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertexSkin, geom.GameMeshSkinGeom)
        tex: texture_2d(float)
        joint_tex: texture_2d(float)
        // Rest-pose AO chart atlas, one per rig, sampled per FRAGMENT: the
        // per-vertex bake it replaced interpolated an ear's darkness across
        // the whole low-poly skull dome (same failure that moved the props
        // to their atlas).
        ao_map: texture_2d(float)
        // The scene's baked-light atlas, addressed via the GROUND region by
        // world xz (the cube family's idiom): a character walking through a
        // house's shadow darkens. ONLY the A channel (sun visibility) is
        // read — lamp light arrives through the analytic dl_* array, and
        // adding the baked RGB too would double-light near every pole.
        // Zero lm_rect = no field, fully sunlit.
        light_map: texture_2d(float)
        // The ground field's shadow-top plane (R8, same uv): the ABSOLUTE
        // height each shadowed texel's sun ray was blocked at, decoded via
        // lm_top_decode. A vertex above the blocker rejects the ground's
        // shadow — a head clears a fence rail's shadow while the shins keep
        // it, and a jump rises out of a roof's shadow at roof height.
        top_map: texture_2d(float)
        lm_rect: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        lm_world: uniform(vec4(0.0, 0.0, 1.0, 1.0))
        // Decode for top_map: absolute blocked height = x + byte * y.
        lm_top_decode: uniform(vec4(0.0, 8.0, 0.0, 0.0))
        // xy = ground-region uv, z = in-field gate, w = TRUE world height
        // of the vertex (for the shadow-top comparison).
        v_lmg: varying(vec4f)
        // Realtime cascades (see DrawSceneCube's block — same contract).
        // v_csm = (true world position, N.L).
        csm_map: texture_2d(float)
        csm_p: uniform(vec4(0.0, 0.001, 0.0, 0.0))
        csm_bias: uniform(vec4(0.001, 0.001, 0.001, 0.0))
        csm_rx0: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry0: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz0: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        csm_rx1: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry1: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz1: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        csm_rx2: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry2: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz2: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        v_csm: varying(vec4f)
        v_ambient: varying(vec3f)
        v_direct: varying(vec3f)
        v_uv: varying(vec2f)
        v_ao_uv: varying(vec2f)
        world: varying(vec4f)
        v_fog: varying(float)
        // Per-frame dynamic lights, up to 8, summed in the VERTEX stage.
        // Characters are always dynamic, so unlike DrawSceneSkinned there is
        // no static gate: every slot counts — street lamps, firework
        // flashes and host lights alike (renderer.rs write_light_uniforms).
        dl_pos0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        v_dl: varying(vec3f)

        oct_decode: fn(e: vec2f) -> vec3f {
            let nz = 1.0 - abs(e.x) - abs(e.y)
            let t = max(0.0 - nz, 0.0)
            let sx = step(0.0, e.x) * 2.0 - 1.0
            let sy = step(0.0, e.y) * 2.0 - 1.0
            return normalize(vec3(e.x - t * sx, e.y - t * sy, nz))
        }

        csm_tap: fn(u: float, v: float, ci: float, ref01: float) -> float {
            let m = 1.5 * self.csm_p.y
            let uu = clamp(u, m, 1.0 - m)
            let vv = clamp(v, m, 1.0 - m)
            return step(ref01, self.csm_map.sample_nearest(
                vec2((uu + ci) * 0.33333333, vv)
            ).x)
        }

        csm_vis: fn(wp: vec3, ndl: float) -> float {
            if self.csm_p.x < 0.5 {
                return 1.0
            }
            var ci = 0.0
            var nx = dot(self.csm_rx0.xyz, wp) + self.csm_rx0.w
            var ny = dot(self.csm_ry0.xyz, wp) + self.csm_ry0.w
            var nz = dot(self.csm_rz0.xyz, wp) + self.csm_rz0.w
            var bias = self.csm_bias.x
            if max(abs(nx), abs(ny)) > 0.99 || nz < 0.0 || nz > 1.0 {
                ci = 1.0
                nx = dot(self.csm_rx1.xyz, wp) + self.csm_rx1.w
                ny = dot(self.csm_ry1.xyz, wp) + self.csm_ry1.w
                nz = dot(self.csm_rz1.xyz, wp) + self.csm_rz1.w
                bias = self.csm_bias.y
            }
            if max(abs(nx), abs(ny)) > 0.99 || nz < 0.0 || nz > 1.0 {
                ci = 2.0
                nx = dot(self.csm_rx2.xyz, wp) + self.csm_rx2.w
                ny = dot(self.csm_ry2.xyz, wp) + self.csm_ry2.w
                nz = dot(self.csm_rz2.xyz, wp) + self.csm_rz2.w
                bias = self.csm_bias.z
            }
            if max(abs(nx), abs(ny)) > 0.99 || nz < 0.0 || nz > 1.0 {
                return 1.0
            }
            let u = nx * 0.5 + 0.5
            let v = 0.5 - ny * 0.5
            let ref01 = nz - bias * (1.0 + (1.0 - clamp(ndl, 0.0, 1.0)) * 2.0)
            let e = self.csm_p.y
            var s = 0.0
            s = s + self.csm_tap(u - e, v - e, ci, ref01)
            s = s + self.csm_tap(u, v - e, ci, ref01)
            s = s + self.csm_tap(u + e, v - e, ci, ref01)
            s = s + self.csm_tap(u - e, v, ci, ref01)
            s = s + self.csm_tap(u, v, ci, ref01)
            s = s + self.csm_tap(u + e, v, ci, ref01)
            s = s + self.csm_tap(u - e, v + e, ci, ref01)
            s = s + self.csm_tap(u, v + e, ci, ref01)
            s = s + self.csm_tap(u + e, v + e, ci, ref01)
            return s / 9.0
        }

        // Same term as DrawSceneSkinned: (1 - d/r)^2 falloff, spot factor
        // mirroring lightmap.rs's lamp pass (SPILL = 0.35, emission axis
        // straight down), empty slots rejected on radius.
        dl_term: fn(wp: vec3, n: vec3, lp: vec4, lc: vec4) -> vec3 {
            if lp.w <= 0.0 {
                return vec3(0.0, 0.0, 0.0)
            }
            let l = lp.xyz - wp
            let d = max(length(l), 0.0001)
            if d >= lp.w {
                return vec3(0.0, 0.0, 0.0)
            }
            let att = 1.0 - d / lp.w
            let ndl = max(dot(n, l * (1.0 / d)), 0.0)
            let cone = clamp((l.y * (1.0 / d) + 0.35) / 1.35, 0.0, 1.0)
            let s = ndl * att * att * (cone * cone * lc.w + (1.0 - lc.w))
            return lc.xyz * s
        }

        dl_sum: fn(wp: vec3, n: vec3) -> vec3 {
            var dl = vec3(0.0, 0.0, 0.0)
            dl = dl + self.dl_term(wp, n, self.dl_pos0, self.dl_col0)
            dl = dl + self.dl_term(wp, n, self.dl_pos1, self.dl_col1)
            dl = dl + self.dl_term(wp, n, self.dl_pos2, self.dl_col2)
            dl = dl + self.dl_term(wp, n, self.dl_pos3, self.dl_col3)
            dl = dl + self.dl_term(wp, n, self.dl_pos4, self.dl_col4)
            dl = dl + self.dl_term(wp, n, self.dl_pos5, self.dl_col5)
            dl = dl + self.dl_term(wp, n, self.dl_pos6, self.dl_col6)
            dl = dl + self.dl_term(wp, n, self.dl_pos7, self.dl_col7)
            return dl
        }

        // One palette row by flat texel index. sample_nearest with explicit
        // lod: the vertex stage cannot use implicit gradients, and RGBA32F is
        // not linearly filterable on every GLES/WebGPU device — nearest at
        // texel centres asks nothing of the filter.
        jrow: fn(t: float) -> vec4f {
            let dim = self.joint_tex.size()
            let y = floor(t / dim.x)
            let x = t - y * dim.x
            return self.joint_tex.sample_nearest(
                vec2((x + 0.5) / dim.x, (y + 0.5) / dim.y),
                0.0
            )
        }

        vertex: fn() {
            let rest = vec4(self.geom.px, self.geom.py, self.geom.pz, 1.0)
            let rn = self.oct_decode(unpack2f16(self.geom.nrm))
            let jj = unpack4u8(self.geom.joints)
            let jw = unpack4u8(self.geom.weights)
            var pos = vec3(0.0, 0.0, 0.0)
            var nrm = vec3(0.0, 0.0, 0.0)
            // Up to 4 influences; these rigs carry 1-2 on most vertices, so
            // the zero-weight branches skip their fetches.
            if jw.x > 0.0 {
                let b = self.joint_base + floor(jj.x * 255.0 + 0.5) * 3.0
                let r0 = self.jrow(b)
                let r1 = self.jrow(b + 1.0)
                let r2 = self.jrow(b + 2.0)
                pos = pos + vec3(dot(r0, rest), dot(r1, rest), dot(r2, rest)) * jw.x
                nrm = nrm + vec3(dot(r0.xyz, rn), dot(r1.xyz, rn), dot(r2.xyz, rn)) * jw.x
            }
            if jw.y > 0.0 {
                let b = self.joint_base + floor(jj.y * 255.0 + 0.5) * 3.0
                let r0 = self.jrow(b)
                let r1 = self.jrow(b + 1.0)
                let r2 = self.jrow(b + 2.0)
                pos = pos + vec3(dot(r0, rest), dot(r1, rest), dot(r2, rest)) * jw.y
                nrm = nrm + vec3(dot(r0.xyz, rn), dot(r1.xyz, rn), dot(r2.xyz, rn)) * jw.y
            }
            if jw.z > 0.0 {
                let b = self.joint_base + floor(jj.z * 255.0 + 0.5) * 3.0
                let r0 = self.jrow(b)
                let r1 = self.jrow(b + 1.0)
                let r2 = self.jrow(b + 2.0)
                pos = pos + vec3(dot(r0, rest), dot(r1, rest), dot(r2, rest)) * jw.z
                nrm = nrm + vec3(dot(r0.xyz, rn), dot(r1.xyz, rn), dot(r2.xyz, rn)) * jw.z
            }
            if jw.w > 0.0 {
                let b = self.joint_base + floor(jj.w * 255.0 + 0.5) * 3.0
                let r0 = self.jrow(b)
                let r1 = self.jrow(b + 1.0)
                let r2 = self.jrow(b + 2.0)
                pos = pos + vec3(dot(r0, rest), dot(r1, rest), dot(r2, rest)) * jw.w
                nrm = nrm + vec3(dot(r0.xyz, rn), dot(r1.xyz, rn), dot(r2.xyz, rn)) * jw.w
            }
            let model_view = self.draw_list.view_transform * self.transform
            let world_normal = normalize((model_view * vec4(nrm.x, nrm.y, nrm.z, 0.0)).xyz)
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let dp = max(dot(world_normal, normalize(self.light_dir)), 0.0)
            let hemi = clamp(world_normal.y * 0.5 + 0.5, 0.0, 1.0)
            self.v_ambient = mix(self.sun_ground, self.sun_sky, hemi)
            self.v_direct = self.sun_color * dp
            // Dynamic lights in TRUE world space (light positions are world
            // coordinates; the stage/view transform must not move them).
            let dl_wp = (self.transform * vec4(pos.x, pos.y, pos.z, 1.0)).xyz
            let dl_n = normalize((self.transform * vec4(nrm.x, nrm.y, nrm.z, 0.0)).xyz)
            self.v_dl = self.dl_sum(dl_wp, dl_n)
            // Ground-field uv for the baked sun shadow. The field stores
            // GROUND-level visibility, so the sample is projected ALONG THE
            // SUN RAY from this vertex down to the character's ground plane
            // (per-instance ground_y): a vertex at height h is shadowed iff
            // the sun ray through it lands on shadowed ground. The shadow
            // boundary slants across the body as they walk through it, and
            // a jumping character rises out of it.
            let dl_h = max(dl_wp.y - self.ground_y, 0.0)
            let dl_sun = normalize(self.light_dir)
            let dl_gxz = dl_wp.xz - dl_sun.xz * (dl_h / max(dl_sun.y, 0.2))
            let lgw = max(self.lm_world.zw, vec2(0.000001, 0.000001))
            let lgraw = (dl_gxz - self.lm_world.xy) / lgw
            let lgf = clamp(lgraw, vec2(0.0, 0.0), vec2(1.0, 1.0))
            let lg_in = step(0.000001, self.lm_rect.z)
                * step(0.0, lgraw.x) * step(lgraw.x, 1.0)
                * step(0.0, lgraw.y) * step(lgraw.y, 1.0)
            let lg_uv = self.lm_rect.xy + lgf * self.lm_rect.zw
            self.v_lmg = vec4(lg_uv.x, lg_uv.y, lg_in, dl_wp.y)
            self.v_csm = vec4(dl_wp.x, dl_wp.y, dl_wp.z, dp)
            self.v_uv = unpack2f16(self.geom.uv)
            // ao_uv is unorm16x2 (model.rs pack_ao_uv), NOT an f16 pair — f16
            // spacing near 1.0 is a full texel of the atlas. Each axis is
            // (lo + 256*hi)/257 of the two unpacked bytes: 255*257 = 65535.
            // Rest-pose bake, valid in every pose: the crevice moves with
            // the surface because the topology never changes.
            let ao_uv_b = unpack4u8(self.geom.ao_uv)
            self.v_ao_uv = vec2(
                (ao_uv_b.x + ao_uv_b.y * 256.0) / 257.0,
                (ao_uv_b.z + ao_uv_b.w * 256.0) / 257.0
            )
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            let tex = self.tex.sample_as_bgra(self.v_uv)
            let albedo = vec3(tex.x * self.tint.x, tex.y * self.tint.y, tex.z * self.tint.z)
            // Same occlusion idiom as DrawSceneSkinned: per-fragment atlas
            // sample, world-anchored hash dither against 8-bit banding,
            // ambient scaled fully and direct partially — a crease should
            // read as a crease even in sunlight, but never darken a lit
            // wall twice.
            let baked = self.ao_map.sample(self.v_ao_uv).x
            let hash = fract(
                sin(dot(self.world.xy + self.world.zz, vec2(12.9898, 78.233))) * 43758.5453
            )
            let ao = clamp(baked + (hash - 0.5) * 0.03, 0.0, 1.0)
            let ao_direct = mix(1.0, ao, 0.75)
            // OnChange: baked sun shadow off the ground field gates the
            // DIRECT term only; A channel only — lamps come from v_dl,
            // never from the field's RGB (that would double-light under
            // every pole). The shadow-top plane rejects the ground's shadow
            // for vertices ABOVE the blocker along the sun ray: a fence
            // rail shades the shins, never the head over it.
            let lmg = self.light_map.sample_as_bgra(self.v_lmg.xy)
            let top_g = self.lm_top_decode.x
                + self.top_map.sample(self.v_lmg.xy).x * self.lm_top_decode.y
            let occ_g = 1.0 - smoothstep(top_g - 0.15, top_g + 0.15, self.v_lmg.w)
            // Realtime: the per-frame cascades replace the ground path
            // entirely — characters receive (and cast) through the same
            // maps as every other surface.
            let sun_vis = mix(
                mix(1.0, smoothstep(0.2, 0.8, lmg.w), self.v_lmg.z * occ_g),
                self.csm_vis(self.v_csm.xyz, self.v_csm.w),
                self.csm_p.x
            )
            let lit = albedo * (
                self.v_ambient * ao + (self.v_direct * sun_vis + self.v_dl) * ao_direct
            )
            return vec4(mix(lit, self.fog_color, self.v_fog), 1.0)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // Generated foliage: the OPT-IN variant that adds growth and wind.
    //
    // Deliberately a sibling of DrawSceneSkinned rather than a flag inside it.
    // Wind costs ~20 vertex ALU and growth ~6; the cube shader draws most of
    // the world and must not pay either. A plant opts in by being drawn with
    // this shader; everything else keeps the cheap path untouched.
    //
    // Both animation weights ride in ONE unorm8 lane (the colour's alpha, high
    // nibble = growth order, low = wind flex), so the variant costs zero extra
    // vertex BYTES over the shared 24-byte layout — which is the bottleneck we
    // actually measured.
    mod.draw.DrawSceneFoliage = mod.std.set_type_default() do #(DrawSceneFoliage::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertex, geom.GameMeshGeom)
        v_ambient: varying(vec3f)
        v_direct: varying(vec3f)
        v_color: varying(vec3f)
        world: varying(vec4f)
        v_fog: varying(float)

        oct_decode: fn(e: vec2f) -> vec3f {
            let nz = 1.0 - abs(e.x) - abs(e.y)
            let t = max(0.0 - nz, 0.0)
            let sx = step(0.0, e.x) * 2.0 - 1.0
            let sy = step(0.0, e.y) * 2.0 - 1.0
            return normalize(vec3(e.x - t * sx, e.y - t * sy, nz))
        }

        vertex: fn() {
            let pos = vec3(self.geom.px, self.geom.py, self.geom.pz)
            let rgba = unpack4u8(self.geom.color)
            // High nibble = growth order along the skeleton, low = wind flex.
            let packed = floor(rgba.w * 255.0 + 0.5)
            let growth_t = floor(packed / 16.0) / 15.0
            let flex = (packed - floor(packed / 16.0) * 16.0) / 15.0

            // Growth reveal: each vertex has its own threshold, so the plant
            // unfurls root-first instead of scaling up as a whole. The band
            // hides the 16-level quantisation of growth_t.
            let reveal = smoothstep(growth_t - self.growth_band, growth_t, self.growth)
            let grown = pos * reveal

            // Per-instance phase from the instance's world position, so a
            // forest sways individually rather than in lockstep — this is the
            // detail that makes it read as wind rather than a global wobble.
            let origin = (self.transform * vec4(0.0, 0.0, 0.0, 1.0)).xyz
            let phase = origin.x * 0.7 + origin.z * 1.3
            let t = self.wind_time
            // Two frequencies: a slow sway plus a faster flutter.
            let sway = sin(t * 1.1 + phase) * self.wind_strength
            let flutter = sin(t * 3.7 + phase * 1.7) * self.wind_gust
            // Clamped so a strong gust bends the plant instead of shearing it.
            let amount = clamp((sway + flutter) * flex, 0.0 - 0.6, 0.6)
            let bent = grown + self.wind_dir * amount

            let normal_in = self.oct_decode(unpack2f16(self.geom.nrm))
            let model_view = self.draw_list.view_transform * self.transform
            let world_normal = normalize((model_view * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
            self.world = model_view * vec4(bent.x, bent.y, bent.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            // Two-sided foliage: cards are lit by the absolute facing so a
            // leaf seen from behind is not black.
            let dp = abs(dot(world_normal, normalize(self.light_dir)))
            let hemi = clamp(world_normal.y * 0.5 + 0.5, 0.0, 1.0)
            self.v_ambient = mix(self.sun_ground, self.sun_sky, hemi)
            self.v_direct = self.sun_color * dp
            self.v_color = rgba.xyz
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            let lit = self.v_color * (self.v_ambient + self.v_direct)
            return vec4(mix(lit, self.fog_color, self.v_fog), 1.0)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // Silhouette shadow mesh (shadow_mesh.rs): every caster's hull for the
    // whole frame, in ONE geometry and ONE draw call.
    //
    // Z-fighting is handled structurally rather than by tuning:
    //   * geometry is offset along the RECEIVER's normal on the CPU, with a
    //     slope-scaled term (world-up would slide the shadow along a slope),
    //   * `depth_write: false` — shadows never occlude each other or anything
    //     else, so overlapping casters cannot fight for the depth buffer,
    //   * depth TEST stays on, so a shadow is still hidden by geometry in
    //     front of it.
    // Per-vertex alpha (colour.w) gives the soft rim for free.
    // Shadow + contact-AO geometry, draped on whatever it lands on.
    mod.draw.DrawSceneShadow = mod.std.set_type_default() do #(DrawSceneShadow::script_shader(vm)){
        alpha_blend: true
        depth_write: false
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertex, geom.GameMeshGeom)
        v_alpha: varying(float)
        world: varying(vec4f)

        vertex: fn() {
            // Packed layout: 6 f32 slots instead of PbrVertex's 16. A shadow
            // needs a position and a coverage value, nothing else.
            let pos = vec3(self.geom.px, self.geom.py, self.geom.pz)
            self.world = self.draw_list.view_transform * vec4(pos.x, pos.y, pos.z, 1.0)
            self.v_alpha = unpack4u8(self.geom.color).w
            let view_pos = self.draw_pass.camera_view * self.world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            // SHADOW DEBUG: saturated magenta at boosted alpha. Overlapping
            // geometry compounds toward white-pink and sliver triangles are
            // unmistakable — structure a black-on-ground shadow hides.
            if self.shadow_debug > 0.5 {
                return vec4(1.0, 0.0, 0.6, clamp(self.v_alpha * 2.0, 0.0, 1.0))
            }
            // Premultiplied black: RGB 0 leaves exactly ground*(1-a), a true
            // multiplicative shadow. Unpremultiplied dark RGB would ADD light.
            return vec4(0.0, 0.0, 0.0, self.v_alpha * self.shadow_scale)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // SDF silhouette shadow — THE dynamic shadow shader: ONE ground-aligned
    // quad per caster (character or driven car); the PIXEL stage samples
    // the caster's baked silhouette-SDF atlas (shadow_sdf.rs — 16 relative
    // yaws x (idle + 8 walk phases) of 32x32 R8 distance cells; rigid
    // models carry one yaw row) at the 2 yaw-neighbour x 2 phase-neighbour
    // cells and LERPS THE DISTANCES. Lerping distances MORPHS the
    // silhouette between poses — a sprite crossfade would double-expose a
    // mid-stride walker into four ghost legs; the moving iso-line cannot.
    // The atlas is baked against a canonical light azimuth, so the quad
    // rotates the sprite into the owning light's world frame (instance
    // axis) and one atlas serves the sun from any direction and any lamp.
    // Edge width widens with distance toward the shadow tip — the far
    // texels were cast by high body parts, which is contact hardening for
    // free. Blend/depth conventions match DrawSceneShadow: premultiplied
    // dark, depth-tested, never depth-written, receiver lift on the CPU.
    mod.draw.DrawSceneShadowSdf = mod.std.set_type_default() do #(DrawSceneShadowSdf::script_shader(vm)){
        alpha_blend: true
        depth_write: false
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        // The shared one-quad sheet (flare geometry): geom_pos.xy is the
        // corner in -0.5..0.5.
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        sdf_tex: texture_2d(float)
        world: varying(vec4f)
        // Cell-local uv (0..1 across the sprite window) and the fragment's
        // window-local coordinates in sprite units (for contact hardening).
        v_uv: varying(vec2f)
        v_local: varying(vec2f)

        vertex: fn() {
            let corner = self.geom.geom_pos.xy + vec2(0.5, 0.5)
            self.v_uv = corner
            // Window-local position in unscaled sprite units.
            let local = self.sdf_d.xy + corner * self.sdf_d.zw
            self.v_local = local
            // Rotate into the owning light's frame: local +x axis = the
            // horizontal direction TOWARD the light, so the baked shadow
            // (which extends toward -x) lands away from it. perp is the
            // frame's +z image; scale is footprint x anchor compression.
            let axis = self.sdf_b.xy
            let perp = vec2(0.0 - axis.y, axis.x)
            let s = self.sdf_b.z
            let xz = self.sdf_a.xz + axis * (local.x * s) + perp * (local.y * s)
            self.world = self.draw_list.view_transform
                * vec4(xz.x, self.sdf_a.y + self.sdf_a.w, xz.y, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
            return self.vertex_pos
        }

        // One atlas cell tap: bilinear WITHIN the cell (uv clamped half a
        // texel inside so neighbouring cells never bleed), returning the
        // encoded distance (0.5 = silhouette boundary, > 0.5 outside).
        cell_d: fn(k: float, row: float) -> float {
            let dim = self.sdf_tex.size()
            let t = clamp(self.v_uv * 32.0, vec2(0.5, 0.5), vec2(31.5, 31.5))
            let uv = (vec2(k, row) * 32.0 + t) / dim
            return self.sdf_tex.sample(uv).x
        }

        // Yaw-pair lerped distance of one pose row.
        yaw_d: fn(row: float, k0: float, k1: float, kf: float) -> float {
            return mix(self.cell_d(k0, row), self.cell_d(k1, row), kf)
        }

        pixel: fn() {
            // Relative yaw -> two stations + blend. The wrap test must see
            // k0 + 1: k0 itself tops out at exactly 15, so step(15.5, k0)
            // never fired and the last sector lerped toward cell 16 — off
            // the atlas' right edge, where the clamp-to-edge sampler reads
            // the border padding ("far outside") and the shadow faded to
            // nothing across one 22.5-degree heading band. Same pattern as
            // the phase wrap below: compare the SUCCESSOR against the last
            // valid index + 0.5.
            let station = fract(self.sdf_c.x / 6.2831855) * 16.0
            let k0 = floor(station)
            let kf = station - k0
            let k1 = k0 + 1.0 - 16.0 * step(15.5, k0 + 1.0)
            // Idle row, then the walk-phase pair (rows 1..rows-1, wrapping)
            // mixed in by the gait blend — THE DISTANCES are what lerp, at
            // every step.
            var d = self.yaw_d(0.0, k0, k1, kf)
            let rows = self.sdf_c.w
            let blend = self.sdf_c.z
            if blend > 0.001 {
                let g = rows - 1.0
                if g > 0.5 {
                    let pp = fract(self.sdf_c.y) * g
                    let p0 = floor(pp)
                    let pf = pp - p0
                    let p1 = p0 + 1.0 - g * step(g - 0.5, p0 + 1.0)
                    let dw = mix(
                        self.yaw_d(1.0 + p0, k0, k1, kf),
                        self.yaw_d(1.0 + p1, k0, k1, kf),
                        pf
                    )
                    d = mix(d, dw, blend)
                }
            }
            // Contact hardening: widen the edge band with distance toward
            // the shadow tip (cast by high sources). w is in encoded-d
            // units, precomputed by the CPU from the atlas band.
            let w = self.sdf_e.x + self.sdf_e.y * max(0.0 - self.v_local.x, 0.0)
            let a = 1.0 - smoothstep(0.5 - w, 0.5 + w, d)
            if self.shadow_debug > 0.5 {
                return vec4(1.0, 0.0, 0.6, clamp(a * 2.0, 0.0, 1.0))
            }
            // Premultiplied black — the shadow layer's multiplicative
            // convention (see DrawSceneShadow).
            return vec4(0.0, 0.0, 0.0, a * self.sdf_b.w * self.shadow_scale)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // The smooth terrain mesh: per-vertex colored triangles, flat normals.
    mod.draw.DrawSceneTerrain = mod.std.set_type_default() do #(DrawSceneTerrain::script_shader(vm)){
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.PbrVertex, geom.PbrGeom)
        // The scene's baked-light atlas: A = sun-visibility SDF, RGB = lamps.
        light_map: texture_2d(float)
        // Realtime cascades (see DrawSceneCube's block — same contract).
        csm_map: texture_2d(float)
        csm_p: uniform(vec4(0.0, 0.001, 0.0, 0.0))
        csm_bias: uniform(vec4(0.001, 0.001, 0.001, 0.0))
        csm_rx0: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry0: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz0: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        csm_rx1: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry1: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz1: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        csm_rx2: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry2: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz2: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        // Ambient and direct split into separate varyings so the PIXEL stage
        // can gate the direct term by the sampled sun SDF — folded together
        // (the old lit_color) there is nothing left to gate.
        lit_color: varying(vec4f)
        v_direct_col: varying(vec3f)
        v_albedo: varying(vec3f)
        v_lm_uv: varying(vec2f)
        v_lm_in: varying(float)
        world: varying(vec4f)
        v_fog: varying(float)
        // Per-frame TRANSIENT lights only (firework flashes, host lights) —
        // street lamps are baked into the atlas RGB, adding them here would
        // double-light the ground. PIXEL stage: terrain vertices are coarse,
        // a vertex-lit flash pops whole cells (see DrawSceneCube).
        dl_pos0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_pos7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        dl_col7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        v_dl_pos: varying(vec3f)
        v_dl_nrm: varying(vec3f)

        dl_term: fn(wp: vec3, n: vec3, lp: vec4, lc: vec4) -> vec3 {
            if lp.w <= 0.0 {
                return vec3(0.0, 0.0, 0.0)
            }
            let l = lp.xyz - wp
            let d = max(length(l), 0.0001)
            if d >= lp.w {
                return vec3(0.0, 0.0, 0.0)
            }
            let att = 1.0 - d / lp.w
            let ndl = max(dot(n, l * (1.0 / d)), 0.0)
            let cone = clamp((l.y * (1.0 / d) + 0.35) / 1.35, 0.0, 1.0)
            let s = ndl * att * att * (cone * cone * lc.w + (1.0 - lc.w))
            return lc.xyz * s
        }

        dl_sum: fn(wp: vec3, n: vec3) -> vec3 {
            var dl = vec3(0.0, 0.0, 0.0)
            dl = dl + self.dl_term(wp, n, self.dl_pos0, self.dl_col0)
            dl = dl + self.dl_term(wp, n, self.dl_pos1, self.dl_col1)
            dl = dl + self.dl_term(wp, n, self.dl_pos2, self.dl_col2)
            dl = dl + self.dl_term(wp, n, self.dl_pos3, self.dl_col3)
            dl = dl + self.dl_term(wp, n, self.dl_pos4, self.dl_col4)
            dl = dl + self.dl_term(wp, n, self.dl_pos5, self.dl_col5)
            dl = dl + self.dl_term(wp, n, self.dl_pos6, self.dl_col6)
            dl = dl + self.dl_term(wp, n, self.dl_pos7, self.dl_col7)
            return dl
        }

        csm_tap: fn(u: float, v: float, ci: float, ref01: float) -> float {
            let m = 1.5 * self.csm_p.y
            let uu = clamp(u, m, 1.0 - m)
            let vv = clamp(v, m, 1.0 - m)
            return step(ref01, self.csm_map.sample_nearest(
                vec2((uu + ci) * 0.33333333, vv)
            ).x)
        }

        csm_vis: fn(wp: vec3, ndl: float) -> float {
            if self.csm_p.x < 0.5 {
                return 1.0
            }
            var ci = 0.0
            var nx = dot(self.csm_rx0.xyz, wp) + self.csm_rx0.w
            var ny = dot(self.csm_ry0.xyz, wp) + self.csm_ry0.w
            var nz = dot(self.csm_rz0.xyz, wp) + self.csm_rz0.w
            var bias = self.csm_bias.x
            if max(abs(nx), abs(ny)) > 0.99 || nz < 0.0 || nz > 1.0 {
                ci = 1.0
                nx = dot(self.csm_rx1.xyz, wp) + self.csm_rx1.w
                ny = dot(self.csm_ry1.xyz, wp) + self.csm_ry1.w
                nz = dot(self.csm_rz1.xyz, wp) + self.csm_rz1.w
                bias = self.csm_bias.y
            }
            if max(abs(nx), abs(ny)) > 0.99 || nz < 0.0 || nz > 1.0 {
                ci = 2.0
                nx = dot(self.csm_rx2.xyz, wp) + self.csm_rx2.w
                ny = dot(self.csm_ry2.xyz, wp) + self.csm_ry2.w
                nz = dot(self.csm_rz2.xyz, wp) + self.csm_rz2.w
                bias = self.csm_bias.z
            }
            if max(abs(nx), abs(ny)) > 0.99 || nz < 0.0 || nz > 1.0 {
                return 1.0
            }
            let u = nx * 0.5 + 0.5
            let v = 0.5 - ny * 0.5
            let ref01 = nz - bias * (1.0 + (1.0 - clamp(ndl, 0.0, 1.0)) * 2.0)
            let e = self.csm_p.y
            var s = 0.0
            s = s + self.csm_tap(u - e, v - e, ci, ref01)
            s = s + self.csm_tap(u, v - e, ci, ref01)
            s = s + self.csm_tap(u + e, v - e, ci, ref01)
            s = s + self.csm_tap(u - e, v, ci, ref01)
            s = s + self.csm_tap(u, v, ci, ref01)
            s = s + self.csm_tap(u + e, v, ci, ref01)
            s = s + self.csm_tap(u - e, v + e, ci, ref01)
            s = s + self.csm_tap(u, v + e, ci, ref01)
            s = s + self.csm_tap(u + e, v + e, ci, ref01)
            return s / 9.0
        }

        vertex: fn() {
            let pos = vec3(self.geom.pos_nx.x, self.geom.pos_nx.y, self.geom.pos_nx.z)
            let normal_in = vec3(self.geom.pos_nx.w, self.geom.ny_nz_uv.x, self.geom.ny_nz_uv.y)
            let model_view = self.draw_list.view_transform * self.transform
            let world_normal = normalize((model_view * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let dp = max(dot(world_normal, normalize(self.light_dir)), 0.0)
            let hemi = clamp(world_normal.y * 0.5 + 0.5, 0.0, 1.0)
            let ambient = mix(self.sun_ground, self.sun_sky, hemi)
            self.lit_color = vec4(self.geom.color.xyz * ambient, self.geom.color.w)
            self.v_direct_col = self.geom.color.xyz * (self.sun_color * dp)
            self.v_albedo = self.geom.color.xyz
            // A heightfield is its own lightmap parameterisation: uv comes
            // straight from the MESH's world xz (pre-view — the stage
            // transform must not move the map), remapped into the terrain's
            // atlas window.
            let lw = max(self.lm_world.zw, vec2(0.000001, 0.000001))
            let lraw = (pos.xz - self.lm_world.xy) / lw
            let lf = clamp(lraw, vec2(0.0, 0.0), vec2(1.0, 1.0))
            self.v_lm_uv = self.lm_rect.xy + lf * self.lm_rect.zw
            // Terrain past the field's rect is fully lit — the field covers
            // the statics, not the whole map.
            self.v_lm_in = step(0.0, lraw.x) * step(lraw.x, 1.0)
                * step(0.0, lraw.y) * step(lraw.y, 1.0)
            // TRUE world position + normal for the pixel-stage transient
            // lights (pre view/stage — the terrain transform is identity in
            // practice, but stay principled). Vertex normals are unit and
            // terrain is smooth, so the interpolated normal is close enough
            // to skip a per-fragment renormalize.
            self.v_dl_pos = (self.transform * vec4(pos.x, pos.y, pos.z, 1.0)).xyz
            self.v_dl_nrm = (self.transform * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            let lm = self.light_map.sample_as_bgra(self.v_lm_uv)
            let has_lm = step(0.000001, self.lm_rect.z) * self.v_lm_in
            // Realtime: the cascades replace the baked A channel.
            let ndl_t = max(dot(normalize(self.v_dl_nrm), normalize(self.light_dir)), 0.0)
            let sun_vis = mix(
                mix(1.0, smoothstep(0.2, 0.8, lm.w), has_lm),
                self.csm_vis(self.v_dl_pos, ndl_t),
                self.csm_p.x
            )
            let lamps = lm.xyz * (2.0 * has_lm)
            if self.lm_debug > 0.5 {
                return vec4(
                    mix(vec3(0.6, 0.1, 0.1), vec3(0.1, 0.6, 0.1), sun_vis) + lamps,
                    1.0
                )
            }
            let dl = self.dl_sum(self.v_dl_pos, self.v_dl_nrm)
            let c = self.lit_color.xyz + self.v_direct_col * sun_vis
                + self.v_albedo * (lamps + dl)
            return vec4(mix(c, self.fog_color, self.v_fog), self.lit_color.w)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // The water sheet (mix.md D7/W1): a flat grid per `game.water` volume,
    // displaced in the VERTEX stage by the same Gerstner sum the sim
    // evaluates CPU-side. The coefficients arrive as uniforms UNMODIFIED
    // from the sim's WaterWave fields (renderer::pack_wave_uniforms — the
    // pin test in renderer.rs holds this file to the same expression), so
    // physics and visuals agree by construction; the sheet is visual-only
    // and physics never reads it. Unused wave slots have amp 0 and
    // contribute nothing — no branch on a count.
    //
    // Translucent like the legacy sensor sheet, and no backface culling for
    // the same reason as DrawSceneAlpha: a wave trough seen from a flat angle
    // shows the sheet's underside.
    mod.draw.DrawSceneWater = mod.std.set_type_default() do #(DrawSceneWater::script_shader(vm)){
        alpha_blend: true
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.PbrVertex, geom.PbrGeom)
        lit_color: varying(vec4f)
        v_direct_col: varying(vec3f)
        world: varying(vec4f)
        v_fog: varying(float)
        // Per-volume wave coefficients: wave_aN = (dir_x, dir_z, k, omega),
        // wave_bN = (amp, phase, group, 0). water_params = (unused, t, 0, 0)
        // with t the sim's own f32 tick-time.
        wave_a0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_a1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_a2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_a3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_a4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_a5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_a6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_a7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wave_b7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        water_params: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // THE canonical wave kernel, mirrored from sim::water::wave_terms:
        // returns (height, dh/dx, dh/dz). The envelope's derivative is
        // omitted from the slope exactly as it is CPU-side.
        wave_term: fn(p: vec2, wa: vec4, wb: vec4, t: float) -> vec3 {
            let phase = wa.z * (wa.x * p.x + wa.y * p.y) - wa.w * t + wb.y
            var env = 1.0
            if wb.z > 0.0 {
                let e = 0.5 + 0.5 * cos(phase / wb.z)
                env = e * e
            }
            let slope = wb.x * env * cos(phase) * wa.z
            return vec3(wb.x * env * sin(phase), slope * wa.x, slope * wa.y)
        }

        vertex: fn() {
            let pos_in = vec3(self.geom.pos_nx.x, self.geom.pos_nx.y, self.geom.pos_nx.z)
            let t = self.water_params.y
            var acc = vec3(0.0, 0.0, 0.0)
            acc = acc + self.wave_term(pos_in.xz, self.wave_a0, self.wave_b0, t)
            acc = acc + self.wave_term(pos_in.xz, self.wave_a1, self.wave_b1, t)
            acc = acc + self.wave_term(pos_in.xz, self.wave_a2, self.wave_b2, t)
            acc = acc + self.wave_term(pos_in.xz, self.wave_a3, self.wave_b3, t)
            acc = acc + self.wave_term(pos_in.xz, self.wave_a4, self.wave_b4, t)
            acc = acc + self.wave_term(pos_in.xz, self.wave_a5, self.wave_b5, t)
            acc = acc + self.wave_term(pos_in.xz, self.wave_a6, self.wave_b6, t)
            acc = acc + self.wave_term(pos_in.xz, self.wave_a7, self.wave_b7, t)
            let pos = vec3(pos_in.x, pos_in.y + acc.x, pos_in.z)
            // Analytic wave normal — same construction as
            // WaterVolume::surface_normal.
            let normal_in = normalize(vec3(0.0 - acc.y, 1.0, 0.0 - acc.z))
            let model_view = self.draw_list.view_transform * self.transform
            let world_normal = normalize((model_view * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let dp = max(dot(world_normal, normalize(self.light_dir)), 0.0)
            let hemi = clamp(world_normal.y * 0.5 + 0.5, 0.0, 1.0)
            let ambient = mix(self.sun_ground, self.sun_sky, hemi)
            self.lit_color = vec4(self.geom.color.xyz * ambient, self.geom.color.w)
            self.v_direct_col = self.geom.color.xyz * (self.sun_color * dp)
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            let c = self.lit_color.xyz + self.v_direct_col
            return vec4(mix(c, self.fog_color, self.v_fog), self.lit_color.w)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // ------------------------------------------------------------------
    // GPU lightmap baker passes (gpu_lightmap.rs). Every shader below is a
    // BAKE pass, never a scene pass: they render into scratch targets and
    // the light atlas itself, and the material shaders above consume the
    // result unchanged. Coordinate conventions used throughout:
    //   * target position: uv in [0,1] with (0,0) the TOP-LEFT texel, so
    //     clip = (u*2-1, 1-2v). Sampling the produced texture with the same
    //     uv reads the texel that was written.
    //   * sun cameras are three row vec4s (rx, ry, rz): row.xyz dotted with
    //     a world point + row.w gives ndc x / ndc y / z01 directly.
    //   * depth scratches are R32F (`color_format: @Rf32`), sampled with
    //     sample_nearest — a shadow test is one exact texel read, never a
    //     filtered one.

    // Sun-view depth: all static geometry (mesh instances + the occluder
    // boxes packed into one world-space geometry) rasterized from a
    // region's fitted ortho sun camera. Double-sided on purpose — the kits
    // are double-sided and the CPU rays hit both faces.
    mod.draw.DrawLmSunDepth = mod.std.set_type_default() do #(DrawLmSunDepth::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Rf32
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertexAo, geom.GameMeshAoGeom)
        v_d: varying(float)
        v_clip: varying(vec2f)

        vertex: fn() {
            let wp = self.transform * vec4(self.geom.px, self.geom.py, self.geom.pz, 1.0)
            let nx = dot(self.sun_rx.xyz, wp.xyz) + self.sun_rx.w
            let ny = dot(self.sun_ry.xyz, wp.xyz) + self.sun_ry.w
            let nz = dot(self.sun_rz.xyz, wp.xyz) + self.sun_rz.w
            self.v_d = nz
            self.v_clip = vec2(nx, ny)
            // flip_a.x = 1: FLIPPED depth test — LessEqual on (1 - z01)
            // keeps the LARGEST z01 = the surface nearest the GROUND along
            // the sun ray. The far map feeds the shadow-top plane, which
            // wants the CPU rays' first-hit (a slab's underside), not the
            // sun's view of its top.
            let zq = nz + self.flip_a.x * (1.0 - 2.0 * nz)
            // tile_a places the cascade pass's tiles in the strip; the
            // atlas passes use the identity tile.
            self.vertex_pos = vec4(
                nx * self.tile_a.x + self.tile_a.z,
                ny * self.tile_a.y + self.tile_a.w,
                zq,
                1.0
            )
        }

        pixel: fn() {
            // Fragments past their own tile's edge belong to a NEIGHBOUR
            // cascade's tile — the hardware only clips at the strip's
            // border. No-op for full-target passes (nothing rasterizes
            // past ndc 1).
            if abs(self.v_clip.x) > 1.001 {
                discard()
            }
            if abs(self.v_clip.y) > 1.001 {
                discard()
            }
            return vec4(self.v_d, 0.0, 0.0, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // Sun-view depth, SKINNED variant: the same projection over a rig's
    // REST mesh (geom.GameMeshVertexSkin), position-blended in the vertex
    // stage against the frame's joint-palette texture — exactly
    // DrawSceneSkinnedGpu's skinning minus the normal work a depth map never
    // reads. The Realtime cascades rasterize characters with this, so
    // their shadows land in the maps like every other caster's.
    // skin_a.x = the instance's first palette texel (joint_base).
    mod.draw.DrawLmSunDepthSkinned = mod.std.set_type_default() do #(DrawLmSunDepthSkinned::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Rf32
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertexSkin, geom.GameMeshSkinGeom)
        joint_tex: texture_2d(float)
        v_d: varying(float)
        v_clip: varying(vec2f)

        // Palette row fetch, verbatim from DrawSceneSkinnedGpu: nearest at
        // texel centres with explicit lod (vertex stage, RGBA32F).
        jrow: fn(t: float) -> vec4f {
            let dim = self.joint_tex.size()
            let y = floor(t / dim.x)
            let x = t - y * dim.x
            return self.joint_tex.sample_nearest(
                vec2((x + 0.5) / dim.x, (y + 0.5) / dim.y),
                0.0
            )
        }

        // Position-only 4-influence blend (DrawSceneSkinnedGpu's, normals
        // dropped). MUST stay in lockstep with the visible draw or the
        // baked shadow detaches from the body that casts it.
        skinned_pos: fn() -> vec3f {
            let rest = vec4(self.geom.px, self.geom.py, self.geom.pz, 1.0)
            let jj = unpack4u8(self.geom.joints)
            let jw = unpack4u8(self.geom.weights)
            var pos = vec3(0.0, 0.0, 0.0)
            if jw.x > 0.0 {
                let b = self.skin_a.x + floor(jj.x * 255.0 + 0.5) * 3.0
                pos = pos + vec3(
                    dot(self.jrow(b), rest),
                    dot(self.jrow(b + 1.0), rest),
                    dot(self.jrow(b + 2.0), rest)
                ) * jw.x
            }
            if jw.y > 0.0 {
                let b = self.skin_a.x + floor(jj.y * 255.0 + 0.5) * 3.0
                pos = pos + vec3(
                    dot(self.jrow(b), rest),
                    dot(self.jrow(b + 1.0), rest),
                    dot(self.jrow(b + 2.0), rest)
                ) * jw.y
            }
            if jw.z > 0.0 {
                let b = self.skin_a.x + floor(jj.z * 255.0 + 0.5) * 3.0
                pos = pos + vec3(
                    dot(self.jrow(b), rest),
                    dot(self.jrow(b + 1.0), rest),
                    dot(self.jrow(b + 2.0), rest)
                ) * jw.z
            }
            if jw.w > 0.0 {
                let b = self.skin_a.x + floor(jj.w * 255.0 + 0.5) * 3.0
                pos = pos + vec3(
                    dot(self.jrow(b), rest),
                    dot(self.jrow(b + 1.0), rest),
                    dot(self.jrow(b + 2.0), rest)
                ) * jw.w
            }
            return pos
        }

        vertex: fn() {
            let pos = self.skinned_pos()
            let wp = self.transform * vec4(pos.x, pos.y, pos.z, 1.0)
            let nx = dot(self.sun_rx.xyz, wp.xyz) + self.sun_rx.w
            let ny = dot(self.sun_ry.xyz, wp.xyz) + self.sun_ry.w
            let nz = dot(self.sun_rz.xyz, wp.xyz) + self.sun_rz.w
            self.v_d = nz
            self.v_clip = vec2(nx, ny)
            // Same flip and tile contracts as DrawLmSunDepth.
            let zq = nz + self.flip_a.x * (1.0 - 2.0 * nz)
            self.vertex_pos = vec4(
                nx * self.tile_a.x + self.tile_a.z,
                ny * self.tile_a.y + self.tile_a.w,
                zq,
                1.0
            )
        }

        pixel: fn() {
            if abs(self.v_clip.x) > 1.001 {
                discard()
            }
            if abs(self.v_clip.y) > 1.001 {
                discard()
            }
            return vec4(self.v_d, 0.0, 0.0, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // Lamp-view depth: the same geometry seen from a lamp, six 90-degree
    // faces tiled 3x2 into one scratch. The face view is three row vec4s
    // like the sun camera; the tile mapping rides in clip space (linear in
    // clip components, so the hardware interpolates it correctly) and
    // fragments that rasterize past their tile's frustum are discarded.
    // Near plane = LIGHT_CLEARANCE: geometry hugging the bulb (the lamp
    // fixture itself) clips out of the map instead of eclipsing the light.
    mod.draw.DrawLmLampDepth = mod.std.set_type_default() do #(DrawLmLampDepth::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Rf32
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertexAo, geom.GameMeshAoGeom)
        v_view: varying(vec3f)

        vertex: fn() {
            let wp = self.transform * vec4(self.geom.px, self.geom.py, self.geom.pz, 1.0)
            let vx = dot(self.face_rx.xyz, wp.xyz) + self.face_rx.w
            let vy = dot(self.face_ry.xyz, wp.xyz) + self.face_ry.w
            let vz = dot(self.face_rz.xyz, wp.xyz) + self.face_rz.w
            self.v_view = vec3(vx, vy, vz)
            let near = self.lamp_range.x
            let far = self.lamp_range.y
            self.vertex_pos = vec4(
                vx * self.tile_a.x + vz * self.tile_a.z,
                vy * self.tile_a.y + vz * self.tile_a.w,
                (vz - near) * far / max(far - near, 0.0001),
                vz
            )
        }

        pixel: fn() {
            let vz = max(self.v_view.z, 0.0001)
            let nx = self.v_view.x / vz
            let ny = self.v_view.y / vz
            if abs(nx) > 0.999 {
                discard()
            }
            if abs(ny) > 0.999 {
                discard()
            }
            let d01 = (self.v_view.z - self.lamp_range.x)
                / max(self.lamp_range.y - self.lamp_range.x, 0.0001)
            return vec4(d01, 0.0, 0.0, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // Sun visibility gather, MESH regions: the region's own geometry
    // rasterized in LIGHTMAP-UV space at 4x the region's atlas resolution.
    // The fragment is one supersample: backface-vs-sun and the depth-map
    // compare reproduce lightmap.rs's sun_bit. Output: R = lit, G = covered
    // (clear = uncovered).
    mod.draw.DrawLmSunGatherMesh = mod.std.set_type_default() do #(DrawLmSunGatherMesh::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Bgra8NoBlend
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertexAo, geom.GameMeshAoGeom)
        depth_tex: texture_2d(float)
        v_world: varying(vec3f)
        v_normal: varying(vec3f)

        oct_decode: fn(e: vec2f) -> vec3f {
            let nz = 1.0 - abs(e.x) - abs(e.y)
            let t = max(0.0 - nz, 0.0)
            let sx = step(0.0, e.x) * 2.0 - 1.0
            let sy = step(0.0, e.y) * 2.0 - 1.0
            return normalize(vec3(e.x - t * sx, e.y - t * sy, nz))
        }

        vertex: fn() {
            let ao_uv_b = unpack4u8(self.geom.ao_uv)
            let cu = (ao_uv_b.x + ao_uv_b.y * 256.0) / 257.0
            let cv = (ao_uv_b.z + ao_uv_b.w * 256.0) / 257.0
            let tp = self.target_a.xy + vec2(cu, cv) * self.target_a.zw
            let wp = self.transform * vec4(self.geom.px, self.geom.py, self.geom.pz, 1.0)
            self.v_world = wp.xyz
            let n = self.oct_decode(unpack2f16(self.geom.nrm))
            self.v_normal = (self.transform * vec4(n.x, n.y, n.z, 0.0)).xyz
            self.vertex_pos = vec4(tp.x * 2.0 - 1.0, 1.0 - 2.0 * tp.y, 0.5, 1.0)
        }

        pixel: fn() {
            var lit = 0.0
            if self.params_a.y > 0.5 {
                let n = normalize(self.v_normal)
                let ndl = dot(n, self.sun_dir_p.xyz)
                if ndl > 0.0 {
                    let p = self.v_world + n * self.sun_dir_p.w
                    let sx = dot(self.sun_rx.xyz, p) + self.sun_rx.w
                    let sy = dot(self.sun_ry.xyz, p) + self.sun_ry.w
                    let sz = dot(self.sun_rz.xyz, p) + self.sun_rz.w
                    let duv = vec2(sx * 0.5 + 0.5, 0.5 - sy * 0.5)
                    let blk = self.depth_tex.sample_nearest(duv).x
                    if sz - self.params_a.x <= blk {
                        lit = 1.0
                    }
                }
            }
            return vec4(lit, 1.0, 0.0, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // Sun visibility gather, the GROUND planar region: one quad over the
    // region's 4x area; the heightfield texture is the surface. Bilinear
    // height and central-difference normal mirror LmHeightField exactly.
    mod.draw.DrawLmSunGatherGround = mod.std.set_type_default() do #(DrawLmSunGatherGround::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Bgra8NoBlend
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)
        depth_tex: texture_2d(float)
        height_tex: texture_2d(float)
        v_uv: varying(vec2f)

        hf_h: fn(x: float, z: float) -> float {
            let n = self.hf_a.w
            let fx = clamp((x - self.hf_a.x) / self.hf_a.z, 0.0, n - 1.0001)
            let fz = clamp((z - self.hf_a.y) / self.hf_a.z, 0.0, n - 1.0001)
            let ix = floor(fx)
            let iz = floor(fz)
            let tx = fx - ix
            let tz = fz - iz
            let inv = 1.0 / n
            let h00 = self.height_tex.sample_nearest(vec2((ix + 0.5) * inv, (iz + 0.5) * inv)).x
            let h10 = self.height_tex.sample_nearest(vec2((ix + 1.5) * inv, (iz + 0.5) * inv)).x
            let h01 = self.height_tex.sample_nearest(vec2((ix + 0.5) * inv, (iz + 1.5) * inv)).x
            let h11 = self.height_tex.sample_nearest(vec2((ix + 1.5) * inv, (iz + 1.5) * inv)).x
            let a = h00 * (1.0 - tx) + h10 * tx
            let b = h01 * (1.0 - tx) + h11 * tx
            return a * (1.0 - tz) + b * tz
        }

        hf_n: fn(x: float, z: float) -> vec3 {
            let e = self.hf_a.z * 0.5
            let dx = self.hf_h(x + e, z) - self.hf_h(x - e, z)
            let dz = self.hf_h(x, z + e) - self.hf_h(x, z - e)
            return normalize(vec3(0.0 - dx, 2.0 * e, 0.0 - dz))
        }

        vertex: fn() {
            let tp = self.quad_a.xy + self.geom.pos * self.quad_a.zw
            self.v_uv = self.geom.pos
            self.vertex_pos = vec4(tp.x * 2.0 - 1.0, 1.0 - 2.0 * tp.y, 0.5, 1.0)
        }

        pixel: fn() {
            var lit = 0.0
            if self.params_a.y > 0.5 {
                let wx = self.ground_a.x + self.v_uv.x * self.ground_a.z
                let wz = self.ground_a.y + self.v_uv.y * self.ground_a.w
                let wy = self.hf_h(wx, wz)
                let n = self.hf_n(wx, wz)
                let ndl = dot(n, self.sun_dir_p.xyz)
                if ndl > 0.0 {
                    let p = vec3(wx, wy, wz) + n * self.sun_dir_p.w
                    let sx = dot(self.sun_rx.xyz, p) + self.sun_rx.w
                    let sy = dot(self.sun_ry.xyz, p) + self.sun_ry.w
                    let sz = dot(self.sun_rz.xyz, p) + self.sun_rz.w
                    let duv = vec2(sx * 0.5 + 0.5, 0.5 - sy * 0.5)
                    let blk = self.depth_tex.sample_nearest(duv).x
                    if sz - self.params_a.x <= blk {
                        lit = 1.0
                    }
                }
            }
            return vec4(lit, 1.0, 0.0, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // 3x3 despeckle vote over the 4x mask, mirroring despeckle_mask: a
    // covered texel flips when >= 6 covered neighbours disagree and none
    // agree. src_a = (inv_w, inv_h, area_w, area_h) of the mask scratch.
    mod.draw.DrawLmDespeckle = mod.std.set_type_default() do #(DrawLmDespeckle::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Bgra8NoBlend
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)
        mask_tex: texture_2d(float)
        v_local: varying(vec2f)

        nb: fn(off: vec2, own_lit: float) -> vec2 {
            let c = self.v_local + off
            if c.x < 0.0 {
                return vec2(0.0, 0.0)
            }
            if c.y < 0.0 {
                return vec2(0.0, 0.0)
            }
            if c.x > self.src_a.z {
                return vec2(0.0, 0.0)
            }
            if c.y > self.src_a.w {
                return vec2(0.0, 0.0)
            }
            let s = self.mask_tex.sample_nearest(c * self.src_a.xy)
            if s.y < 0.5 {
                return vec2(0.0, 0.0)
            }
            if abs(s.x - own_lit) < 0.5 {
                return vec2(1.0, 0.0)
            }
            return vec2(0.0, 1.0)
        }

        vertex: fn() {
            let tp = self.quad_a.xy + self.geom.pos * self.quad_a.zw
            self.v_local = self.geom.pos * self.src_a.zw
            self.vertex_pos = vec4(tp.x * 2.0 - 1.0, 1.0 - 2.0 * tp.y, 0.5, 1.0)
        }

        pixel: fn() {
            let own = self.mask_tex.sample_nearest(self.v_local * self.src_a.xy)
            if own.y < 0.5 {
                return own
            }
            var acc = vec2(0.0, 0.0)
            acc = acc + self.nb(vec2(-1.0, -1.0), own.x)
            acc = acc + self.nb(vec2(0.0, -1.0), own.x)
            acc = acc + self.nb(vec2(1.0, -1.0), own.x)
            acc = acc + self.nb(vec2(-1.0, 0.0), own.x)
            acc = acc + self.nb(vec2(1.0, 0.0), own.x)
            acc = acc + self.nb(vec2(-1.0, 1.0), own.x)
            acc = acc + self.nb(vec2(0.0, 1.0), own.x)
            acc = acc + self.nb(vec2(1.0, 1.0), own.x)
            var out_lit = own.x
            if acc.y >= 5.5 {
                if acc.x < 0.5 {
                    out_lit = 1.0 - own.x
                }
            }
            return vec4(out_lit, 1.0, 0.0, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // 4x -> 1x coverage downsample into the atlas-layout coverage texture:
    // R = lit fraction OF COVERED subs, G = covered fraction of 16.
    mod.draw.DrawLmDownsample = mod.std.set_type_default() do #(DrawLmDownsample::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Bgra8NoBlend
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)
        mask_tex: texture_2d(float)
        v_local: varying(vec2f)

        sub_tap: fn(base: vec2, ox: float, oy: float) -> vec2 {
            let s = self.mask_tex.sample_nearest((base + vec2(ox, oy)) * self.src_a.xy)
            let cov = step(0.5, s.y)
            return vec2(s.x * cov, cov)
        }

        vertex: fn() {
            let tp = self.quad_a.xy + self.geom.pos * self.quad_a.zw
            self.v_local = self.geom.pos * self.rect_a.zw
            self.vertex_pos = vec4(tp.x * 2.0 - 1.0, 1.0 - 2.0 * tp.y, 0.5, 1.0)
        }

        pixel: fn() {
            let base = floor(self.v_local) * 4.0
            var acc = vec2(0.0, 0.0)
            acc = acc + self.sub_tap(base, 0.5, 0.5)
            acc = acc + self.sub_tap(base, 1.5, 0.5)
            acc = acc + self.sub_tap(base, 2.5, 0.5)
            acc = acc + self.sub_tap(base, 3.5, 0.5)
            acc = acc + self.sub_tap(base, 0.5, 1.5)
            acc = acc + self.sub_tap(base, 1.5, 1.5)
            acc = acc + self.sub_tap(base, 2.5, 1.5)
            acc = acc + self.sub_tap(base, 3.5, 1.5)
            acc = acc + self.sub_tap(base, 0.5, 2.5)
            acc = acc + self.sub_tap(base, 1.5, 2.5)
            acc = acc + self.sub_tap(base, 2.5, 2.5)
            acc = acc + self.sub_tap(base, 3.5, 2.5)
            acc = acc + self.sub_tap(base, 0.5, 3.5)
            acc = acc + self.sub_tap(base, 1.5, 3.5)
            acc = acc + self.sub_tap(base, 2.5, 3.5)
            acc = acc + self.sub_tap(base, 3.5, 3.5)
            var lit_frac = 0.0
            if acc.y > 0.5 {
                lit_frac = acc.x / acc.y
            }
            return vec4(lit_frac, acc.y / 16.0, 0.0, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // Distance transform over the 1x coverage, as iterated 3/4-chamfer
    // relaxation (mode 0 = seed from coverage, mode 1 = one min-propagation
    // step). R/G carry SIGNED distances to the anti-aliased shadow edge
    // (R measured entering the lit region, G entering the shadowed one),
    // dead-reckoned from FRACTIONAL seeds: a boundary texel with lit
    // fraction f holds the edge (0.5 - f) texels from its centre, and that
    // sub-texel offset seeds BOTH channels (Gustavson's anti-aliased DT).
    // Chamfer min-propagation then offsets smooth, sub-texel-accurate
    // contours outward. Seeding hard 0/0 instead measured every distance
    // from the binary 1x boundary's texel centres — ±0.5-texel contour
    // wobble that the decode window magnified into visibly GRAINY penumbra
    // (the prebaked .shadowsdf quads supersample their DT and average
    // distances, which is why they looked clean next to this).
    //
    // Storage: byte = (d_texels + 0.5) / 6.5, so the half-texel NEGATIVE
    // range survives the unsigned channel; 1.0 = "far" (6 texels, beyond
    // the 4-texel encode band). Neighbour reads clamp to the region rect —
    // regions never bleed.
    mod.draw.DrawLmChamfer = mod.std.set_type_default() do #(DrawLmChamfer::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Bgra8NoBlend
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)
        cov_tex: texture_2d(float)
        dt_tex: texture_2d(float)
        v_local: varying(vec2f)

        nb2: fn(off: vec2, w: float) -> vec2 {
            let c = self.v_local + off
            if c.x < 0.0 {
                return vec2(2.0, 2.0)
            }
            if c.y < 0.0 {
                return vec2(2.0, 2.0)
            }
            if c.x > self.rect_px.z {
                return vec2(2.0, 2.0)
            }
            if c.y > self.rect_px.w {
                return vec2(2.0, 2.0)
            }
            let uv = (self.rect_px.xy + c) * vec2(self.misc_a.x, self.misc_a.x)
            let s = self.dt_tex.sample_nearest(uv)
            return vec2(s.x + w, s.y + w)
        }

        vertex: fn() {
            let tp = self.quad_a.xy + self.geom.pos * self.quad_a.zw
            self.v_local = self.geom.pos * self.rect_px.zw
            self.vertex_pos = vec4(tp.x * 2.0 - 1.0, 1.0 - 2.0 * tp.y, 0.5, 1.0)
        }

        pixel: fn() {
            let uv = (self.rect_px.xy + self.v_local) * vec2(self.misc_a.x, self.misc_a.x)
            if self.misc_a.y < 0.5 {
                let c = self.cov_tex.sample_nearest(uv)
                // Stored zero point: d = -0.5 texels maps to byte 0, so
                // "at the centre" (d = 0) stores 0.5/6.5.
                var dl = 1.0
                var ds = 1.0
                if c.y > 0.001 {
                    if c.x >= 0.999 {
                        dl = 0.076923077
                    } else {
                        if c.x <= 0.001 {
                            ds = 0.076923077
                        } else {
                            // The boundary texel: lit fraction f puts the
                            // edge (0.5 - f) texels from the centre. Both
                            // channels take the SIGNED offset — stored
                            // (d + 0.5)/6.5 these collapse to (1-f)/6.5
                            // and f/6.5.
                            dl = (1.0 - c.x) * 0.153846154
                            ds = c.x * 0.153846154
                        }
                    }
                }
                return vec4(dl, ds, 0.0, 1.0)
            }
            let own = self.dt_tex.sample_nearest(uv)
            var dl = own.x
            var ds = own.y
            // Chamfer steps in stored units: 1 texel = 1/6.5 axial, the
            // 4/3-texel chamfer diagonal = (4/3)/6.5.
            var n = self.nb2(vec2(-1.0, 0.0), 0.153846154)
            dl = min(dl, n.x)
            ds = min(ds, n.y)
            n = self.nb2(vec2(1.0, 0.0), 0.153846154)
            dl = min(dl, n.x)
            ds = min(ds, n.y)
            n = self.nb2(vec2(0.0, -1.0), 0.153846154)
            dl = min(dl, n.x)
            ds = min(ds, n.y)
            n = self.nb2(vec2(0.0, 1.0), 0.153846154)
            dl = min(dl, n.x)
            ds = min(ds, n.y)
            n = self.nb2(vec2(-1.0, -1.0), 0.205128205)
            dl = min(dl, n.x)
            ds = min(ds, n.y)
            n = self.nb2(vec2(1.0, -1.0), 0.205128205)
            dl = min(dl, n.x)
            ds = min(ds, n.y)
            n = self.nb2(vec2(-1.0, 1.0), 0.205128205)
            dl = min(dl, n.x)
            ds = min(ds, n.y)
            n = self.nb2(vec2(1.0, 1.0), 0.205128205)
            dl = min(dl, n.x)
            ds = min(ds, n.y)
            return vec4(min(dl, 1.0), min(ds, 1.0), 0.0, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // Lamp gather, MESH regions: region geometry in lightmap-uv space at 1x,
    // additive into the lamp accumulation texture. mode (lamp_c.w) 0 writes
    // the coverage prepass (rgb 0, alpha 1 marks "holds light"); mode 1 adds
    // one lamp's contribution with alpha 0 (pure add under premul blending).
    // The lamp math mirrors lightmap.rs's lamp loop: N.L x (1-d/r)^2 x
    // cone^2 with SPILL = 0.35, visibility from the 6-face depth scratch.
    mod.draw.DrawLmLampGatherMesh = mod.std.set_type_default() do #(DrawLmLampGatherMesh::script_shader(vm)){
        alpha_blend: true
        backface_culling: false
        depth_write: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertexAo, geom.GameMeshAoGeom)
        depth_tex: texture_2d(float)
        v_world: varying(vec3f)
        v_normal: varying(vec3f)

        oct_decode: fn(e: vec2f) -> vec3f {
            let nz = 1.0 - abs(e.x) - abs(e.y)
            let t = max(0.0 - nz, 0.0)
            let sx = step(0.0, e.x) * 2.0 - 1.0
            let sy = step(0.0, e.y) * 2.0 - 1.0
            return normalize(vec3(e.x - t * sx, e.y - t * sy, nz))
        }

        face_v: fn(l: vec3) -> vec4 {
            let a = abs(l)
            if a.x >= a.y {
                if a.x >= a.z {
                    if l.x >= 0.0 {
                        return vec4(0.0 - l.z, l.y, a.x, 0.0)
                    }
                    return vec4(l.z, l.y, a.x, 1.0)
                }
                if l.z >= 0.0 {
                    return vec4(l.x, l.y, a.z, 4.0)
                }
                return vec4(0.0 - l.x, l.y, a.z, 5.0)
            }
            if a.y >= a.z {
                if l.y >= 0.0 {
                    return vec4(l.x, 0.0 - l.z, a.y, 2.0)
                }
                return vec4(l.x, l.z, a.y, 3.0)
            }
            if l.z >= 0.0 {
                return vec4(l.x, l.y, a.z, 4.0)
            }
            return vec4(0.0 - l.x, l.y, a.z, 5.0)
        }

        lamp_term: fn(world: vec3, n: vec3) -> vec4 {
            let to = self.lamp_a.xyz - world
            let d = length(to)
            if d >= self.lamp_a.w {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            if d < 0.0001 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let dir = to * (1.0 / d)
            let ndl = dot(n, dir)
            if ndl <= 0.0 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let origin = world + n * self.lamp_d.w
            let l = origin - self.lamp_a.xyz
            let fv = self.face_v(l)
            let vz = max(fv.z, 0.0001)
            var col = fv.w
            var row = 0.0
            if fv.w > 2.5 {
                col = fv.w - 3.0
                row = 1.0
            }
            let tu = clamp(fv.x / vz * 0.5 + 0.5, 0.001, 0.999)
            let tv = clamp(0.5 - fv.y / vz * 0.5, 0.001, 0.999)
            let duv = vec2((col + tu) / 3.0, (row + tv) / 2.0)
            let blk01 = self.depth_tex.sample_nearest(duv).x
            let near = self.lamp_d.x
            let blk = near + blk01 * max(self.lamp_d.y - near, 0.0001)
            let scaled = fv.z * max(d - 0.25, 0.0) / d
            if blk < scaled - self.lamp_d.z {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let att = 1.0 - d / self.lamp_a.w
            var s = ndl * att * att
            if self.lamp_b.w > 0.0 {
                let cone = clamp((dot(dir * -1.0, self.lamp_c.xyz) + 0.35) / 1.35, 0.0, 1.0)
                s = s * (cone * cone * self.lamp_b.w + (1.0 - self.lamp_b.w))
            }
            return vec4(self.lamp_b.xyz * (s * 0.5), 0.0)
        }

        vertex: fn() {
            let ao_uv_b = unpack4u8(self.geom.ao_uv)
            let cu = (ao_uv_b.x + ao_uv_b.y * 256.0) / 257.0
            let cv = (ao_uv_b.z + ao_uv_b.w * 256.0) / 257.0
            let tp = self.target_a.xy + vec2(cu, cv) * self.target_a.zw
            let wp = self.transform * vec4(self.geom.px, self.geom.py, self.geom.pz, 1.0)
            self.v_world = wp.xyz
            let n = self.oct_decode(unpack2f16(self.geom.nrm))
            self.v_normal = (self.transform * vec4(n.x, n.y, n.z, 0.0)).xyz
            self.vertex_pos = vec4(tp.x * 2.0 - 1.0, 1.0 - 2.0 * tp.y, 0.5, 1.0)
        }

        pixel: fn() {
            if self.lamp_c.w < 0.5 {
                return vec4(0.0, 0.0, 0.0, 1.0)
            }
            return self.lamp_term(self.v_world, normalize(self.v_normal))
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // Lamp gather, GROUND region: heightfield quad, same lamp math.
    mod.draw.DrawLmLampGatherGround = mod.std.set_type_default() do #(DrawLmLampGatherGround::script_shader(vm)){
        alpha_blend: true
        backface_culling: false
        depth_write: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)
        depth_tex: texture_2d(float)
        height_tex: texture_2d(float)
        v_uv: varying(vec2f)

        hf_h: fn(x: float, z: float) -> float {
            let n = self.hf_a.w
            let fx = clamp((x - self.hf_a.x) / self.hf_a.z, 0.0, n - 1.0001)
            let fz = clamp((z - self.hf_a.y) / self.hf_a.z, 0.0, n - 1.0001)
            let ix = floor(fx)
            let iz = floor(fz)
            let tx = fx - ix
            let tz = fz - iz
            let inv = 1.0 / n
            let h00 = self.height_tex.sample_nearest(vec2((ix + 0.5) * inv, (iz + 0.5) * inv)).x
            let h10 = self.height_tex.sample_nearest(vec2((ix + 1.5) * inv, (iz + 0.5) * inv)).x
            let h01 = self.height_tex.sample_nearest(vec2((ix + 0.5) * inv, (iz + 1.5) * inv)).x
            let h11 = self.height_tex.sample_nearest(vec2((ix + 1.5) * inv, (iz + 1.5) * inv)).x
            let a = h00 * (1.0 - tx) + h10 * tx
            let b = h01 * (1.0 - tx) + h11 * tx
            return a * (1.0 - tz) + b * tz
        }

        hf_n: fn(x: float, z: float) -> vec3 {
            let e = self.hf_a.z * 0.5
            let dx = self.hf_h(x + e, z) - self.hf_h(x - e, z)
            let dz = self.hf_h(x, z + e) - self.hf_h(x, z - e)
            return normalize(vec3(0.0 - dx, 2.0 * e, 0.0 - dz))
        }

        face_v: fn(l: vec3) -> vec4 {
            let a = abs(l)
            if a.x >= a.y {
                if a.x >= a.z {
                    if l.x >= 0.0 {
                        return vec4(0.0 - l.z, l.y, a.x, 0.0)
                    }
                    return vec4(l.z, l.y, a.x, 1.0)
                }
                if l.z >= 0.0 {
                    return vec4(l.x, l.y, a.z, 4.0)
                }
                return vec4(0.0 - l.x, l.y, a.z, 5.0)
            }
            if a.y >= a.z {
                if l.y >= 0.0 {
                    return vec4(l.x, 0.0 - l.z, a.y, 2.0)
                }
                return vec4(l.x, l.z, a.y, 3.0)
            }
            if l.z >= 0.0 {
                return vec4(l.x, l.y, a.z, 4.0)
            }
            return vec4(0.0 - l.x, l.y, a.z, 5.0)
        }

        lamp_term: fn(world: vec3, n: vec3) -> vec4 {
            let to = self.lamp_a.xyz - world
            let d = length(to)
            if d >= self.lamp_a.w {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            if d < 0.0001 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let dir = to * (1.0 / d)
            let ndl = dot(n, dir)
            if ndl <= 0.0 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let origin = world + n * self.lamp_d.w
            let l = origin - self.lamp_a.xyz
            let fv = self.face_v(l)
            let vz = max(fv.z, 0.0001)
            var col = fv.w
            var row = 0.0
            if fv.w > 2.5 {
                col = fv.w - 3.0
                row = 1.0
            }
            let tu = clamp(fv.x / vz * 0.5 + 0.5, 0.001, 0.999)
            let tv = clamp(0.5 - fv.y / vz * 0.5, 0.001, 0.999)
            let duv = vec2((col + tu) / 3.0, (row + tv) / 2.0)
            let blk01 = self.depth_tex.sample_nearest(duv).x
            let near = self.lamp_d.x
            let blk = near + blk01 * max(self.lamp_d.y - near, 0.0001)
            let scaled = fv.z * max(d - 0.25, 0.0) / d
            if blk < scaled - self.lamp_d.z {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let att = 1.0 - d / self.lamp_a.w
            var s = ndl * att * att
            if self.lamp_b.w > 0.0 {
                let cone = clamp((dot(dir * -1.0, self.lamp_c.xyz) + 0.35) / 1.35, 0.0, 1.0)
                s = s * (cone * cone * self.lamp_b.w + (1.0 - self.lamp_b.w))
            }
            return vec4(self.lamp_b.xyz * (s * 0.5), 0.0)
        }

        vertex: fn() {
            let tp = self.quad_a.xy + self.geom.pos * self.quad_a.zw
            self.v_uv = self.geom.pos
            self.vertex_pos = vec4(tp.x * 2.0 - 1.0, 1.0 - 2.0 * tp.y, 0.5, 1.0)
        }

        pixel: fn() {
            if self.lamp_c.w < 0.5 {
                return vec4(0.0, 0.0, 0.0, 1.0)
            }
            let wx = self.ground_a.x + self.v_uv.x * self.ground_a.z
            let wz = self.ground_a.y + self.v_uv.y * self.ground_a.w
            let wy = self.hf_h(wx, wz)
            let n = self.hf_n(wx, wz)
            return self.lamp_term(vec3(wx, wy, wz), n)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // Lamp rim fill + smooth, mirroring dilate_rgb: mode 0/1 = one ring of
    // averaging into non-holding texels, mode 2 = the coverage-weighted
    // 4/2/1 smooth over holding texels. Alpha carries "holds light".
    mod.draw.DrawLmLampDilate = mod.std.set_type_default() do #(DrawLmLampDilate::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Bgra8NoBlend
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)
        lamp_tex: texture_2d(float)
        v_local: varying(vec2f)

        rg: fn(off: vec2) -> vec4 {
            let c = self.v_local + off
            if c.x < 0.0 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            if c.y < 0.0 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            if c.x > self.rect_px.z {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            if c.y > self.rect_px.w {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let uv = (self.rect_px.xy + c) * vec2(self.misc_a.x, self.misc_a.x)
            let s = self.lamp_tex.sample_nearest(uv)
            if s.w < 0.5 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            return vec4(s.xyz, 1.0)
        }

        sm: fn(off: vec2, w: float) -> vec4 {
            let s = self.rg(off)
            return vec4(s.xyz * w, s.w * w)
        }

        vertex: fn() {
            let tp = self.quad_a.xy + self.geom.pos * self.quad_a.zw
            self.v_local = self.geom.pos * self.rect_px.zw
            self.vertex_pos = vec4(tp.x * 2.0 - 1.0, 1.0 - 2.0 * tp.y, 0.5, 1.0)
        }

        pixel: fn() {
            let uv = (self.rect_px.xy + self.v_local) * vec2(self.misc_a.x, self.misc_a.x)
            let own = self.lamp_tex.sample_nearest(uv)
            if self.misc_a.y > 1.5 {
                if own.w < 0.5 {
                    return vec4(0.0, 0.0, 0.0, 0.0)
                }
                var acc = vec4(own.xyz * 4.0, 4.0)
                acc = acc + self.sm(vec2(-1.0, -1.0), 1.0)
                acc = acc + self.sm(vec2(0.0, -1.0), 2.0)
                acc = acc + self.sm(vec2(1.0, -1.0), 1.0)
                acc = acc + self.sm(vec2(-1.0, 0.0), 2.0)
                acc = acc + self.sm(vec2(1.0, 0.0), 2.0)
                acc = acc + self.sm(vec2(-1.0, 1.0), 1.0)
                acc = acc + self.sm(vec2(0.0, 1.0), 2.0)
                acc = acc + self.sm(vec2(1.0, 1.0), 1.0)
                return vec4(acc.xyz / max(acc.w, 1.0), 1.0)
            }
            if own.w > 0.5 {
                return vec4(own.xyz, 1.0)
            }
            var acc = vec4(0.0, 0.0, 0.0, 0.0)
            acc = acc + self.rg(vec2(-1.0, -1.0))
            acc = acc + self.rg(vec2(0.0, -1.0))
            acc = acc + self.rg(vec2(1.0, -1.0))
            acc = acc + self.rg(vec2(-1.0, 0.0))
            acc = acc + self.rg(vec2(1.0, 0.0))
            acc = acc + self.rg(vec2(-1.0, 1.0))
            acc = acc + self.rg(vec2(0.0, 1.0))
            acc = acc + self.rg(vec2(1.0, 1.0))
            if acc.w > 0.5 {
                return vec4(acc.xyz / acc.w, 1.0)
            }
            return vec4(0.0, 0.0, 0.0, 0.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // Final encode into the light atlas: A = 128-centred signed distance
    // over the 4-texel band (exactly the CPU convention, so the material
    // shaders' smoothstep(0.2, 0.8, lm.w) needs no change), RGB = the
    // dilated lamp accumulation. Quads are the region rects EXPANDED one
    // texel so the padding ring encodes the same "fully lit, no lamps"
    // default the CPU left there.
    mod.draw.DrawLmEncode = mod.std.set_type_default() do #(DrawLmEncode::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Bgra8NoBlend
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)
        dt_tex: texture_2d(float)
        cov_tex: texture_2d(float)
        lamp_tex: texture_2d(float)
        v_local: varying(vec2f)

        vertex: fn() {
            let tp = self.quad_a.xy + self.geom.pos * self.quad_a.zw
            self.v_local = self.geom.pos * self.rect_px.zw
            self.vertex_pos = vec4(tp.x * 2.0 - 1.0, 1.0 - 2.0 * tp.y, 0.5, 1.0)
        }

        pixel: fn() {
            let uv = (self.rect_px.xy + self.v_local) * vec2(self.misc_a.x, self.misc_a.x)
            let dt = self.dt_tex.sample_nearest(uv)
            let cov = self.cov_tex.sample_nearest(uv)
            let lamps = self.lamp_tex.sample_nearest(uv)
            // Debug taps (MAKEPAD_GPU_LM_SHOW): 1 = coverage as A, 2 = the
            // distance transform's shadow distance as A.
            if self.misc_a.y > 1.5 {
                return vec4(dt.xyz, 1.0 - dt.y)
            }
            if self.misc_a.y > 0.5 {
                return vec4(cov.xyz, cov.x * step(0.001, cov.y))
            }
            let dl = dt.x * 6.0
            let ds = dt.y * 6.0
            var sd = 0.0
            if dl <= ds {
                sd = min(ds, 4.0)
            } else {
                sd = 0.0 - min(dl, 4.0)
            }
            if cov.y > 0.001 {
                if cov.x > 0.001 {
                    if cov.x < 0.999 {
                        let c = (cov.x - 0.5) * 2.0
                        let w = clamp(1.0 - abs(sd), 0.0, 1.0)
                        sd = sd + (c - sd) * w
                    }
                }
            }
            let a = clamp((128.0 + sd * 31.75) / 255.0, 0.0, 1.0)
            return vec4(lamps.xyz, a)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // Shadow-top plane: for a SHADOWED ground texel, the absolute world
    // height its sun ray was blocked at, encoded (h - base) / range in
    // 0..254/255; lit = 1.0 (byte 255, decodes to "blocker far overhead" —
    // harmless because the atlas gates no shadow there anyway).
    // top_a = (zr, sun_dir.y, base, range).
    //
    // A shadowed texel whose ray finds NO blocker above the depth bias
    // encodes the GROUND SURFACE height instead of the lit marker. Two ways
    // to get here, both meaning "this shadow belongs to something at ground
    // level": the SDF penumbra band reaches texels outside the geometric
    // shadow (no blocker exists on their ray at all), and a blocker hugging
    // the surface within the bias (a slab underside). Writing 255 here made
    // those texels shadow a dynamic at ANY height — 255 decodes to a
    // blocker ~10 units up, so occ_g kept a static shadow's whole penumbra
    // fringe on every body that crossed it. Ground height keeps the shadow
    // for ground-level fragments (terrain, feet) and rejects it a
    // compare-band above — the height-correct answer for a ground-level
    // blocker. (OnChange path: Realtime serves dynamics through the
    // cascades and never consults this plane.)
    mod.draw.DrawLmTop = mod.std.set_type_default() do #(DrawLmTop::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Bgra8NoBlend
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)
        atlas_tex: texture_2d(float)
        depth_tex: texture_2d(float)
        height_tex: texture_2d(float)
        v_uv: varying(vec2f)

        hf_h: fn(x: float, z: float) -> float {
            let n = self.hf_a.w
            let fx = clamp((x - self.hf_a.x) / self.hf_a.z, 0.0, n - 1.0001)
            let fz = clamp((z - self.hf_a.y) / self.hf_a.z, 0.0, n - 1.0001)
            let ix = floor(fx)
            let iz = floor(fz)
            let tx = fx - ix
            let tz = fz - iz
            let inv = 1.0 / n
            let h00 = self.height_tex.sample_nearest(vec2((ix + 0.5) * inv, (iz + 0.5) * inv)).x
            let h10 = self.height_tex.sample_nearest(vec2((ix + 1.5) * inv, (iz + 0.5) * inv)).x
            let h01 = self.height_tex.sample_nearest(vec2((ix + 0.5) * inv, (iz + 1.5) * inv)).x
            let h11 = self.height_tex.sample_nearest(vec2((ix + 1.5) * inv, (iz + 1.5) * inv)).x
            let a = h00 * (1.0 - tx) + h10 * tx
            let b = h01 * (1.0 - tx) + h11 * tx
            return a * (1.0 - tz) + b * tz
        }

        hf_n: fn(x: float, z: float) -> vec3 {
            let e = self.hf_a.z * 0.5
            let dx = self.hf_h(x + e, z) - self.hf_h(x - e, z)
            let dz = self.hf_h(x, z + e) - self.hf_h(x, z - e)
            return normalize(vec3(0.0 - dx, 2.0 * e, 0.0 - dz))
        }

        vertex: fn() {
            let tp = self.quad_a.xy + self.geom.pos * self.quad_a.zw
            self.v_uv = self.geom.pos
            self.vertex_pos = vec4(tp.x * 2.0 - 1.0, 1.0 - 2.0 * tp.y, 0.5, 1.0)
        }

        pixel: fn() {
            if self.params_a.x < 0.5 {
                return vec4(1.0, 1.0, 1.0, 1.0)
            }
            let auv = self.quad_a.xy + self.v_uv * self.quad_a.zw
            let lm_a = self.atlas_tex.sample_nearest(auv).w
            // Lit gate at the decode window's TOP edge (LM_SUN_SOFT.1, held
            // in lockstep by shaders.rs's pin test): every texel the
            // material shaders would darken AT ALL — the outer penumbra
            // included — must carry a blocker height, or that fringe
            // shadows dynamics at any altitude.
            if lm_a >= 0.8 {
                return vec4(1.0, 1.0, 1.0, 1.0)
            }
            let wx = self.ground_a.x + self.v_uv.x * self.ground_a.z
            let wz = self.ground_a.y + self.v_uv.y * self.ground_a.w
            let wy = self.hf_h(wx, wz)
            let n = self.hf_n(wx, wz)
            let p = vec3(wx, wy, wz) + n * self.params_a.z
            let sx = dot(self.sun_rx.xyz, p) + self.sun_rx.w
            let sy = dot(self.sun_ry.xyz, p) + self.sun_ry.w
            let sz = dot(self.sun_rz.xyz, p) + self.sun_rz.w
            let duv = vec2(sx * 0.5 + 0.5, 0.5 - sy * 0.5)
            let blk = self.depth_tex.sample_nearest(duv).x
            if blk >= sz - self.params_a.y {
                // Shadowed but no blocker above the bias: the blocker is AT
                // the surface (contact texel / penumbra fringe). Encode the
                // ground height — never the lit marker (see header).
                let eg = clamp((wy - self.top_a.z) / max(self.top_a.w, 0.0001), 0.0, 0.99607843)
                return vec4(eg, eg, eg, 1.0)
            }
            let bt = (sz - blk) * self.top_a.x
            let h = p.y + bt * self.top_a.y
            let e = clamp((h - self.top_a.z) / max(self.top_a.w, 0.0001), 0.0, 0.99607843)
            return vec4(e, e, e, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // Two rings of min-dilation of blocker heights into unmeasured (255)
    // texels — dilate_top_min, one ring per pass.
    mod.draw.DrawLmTopDilate = mod.std.set_type_default() do #(DrawLmTopDilate::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Bgra8NoBlend
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)
        top_tex: texture_2d(float)
        v_local: varying(vec2f)

        tp_at: fn(off: vec2) -> float {
            let c = self.v_local + off
            if c.x < 0.0 {
                return 1.0
            }
            if c.y < 0.0 {
                return 1.0
            }
            if c.x > self.rect_px.z {
                return 1.0
            }
            if c.y > self.rect_px.w {
                return 1.0
            }
            let uv = (self.rect_px.xy + c) * vec2(self.misc_a.x, self.misc_a.x)
            return self.top_tex.sample_nearest(uv).x
        }

        vertex: fn() {
            let tp = self.quad_a.xy + self.geom.pos * self.quad_a.zw
            self.v_local = self.geom.pos * self.rect_px.zw
            self.vertex_pos = vec4(tp.x * 2.0 - 1.0, 1.0 - 2.0 * tp.y, 0.5, 1.0)
        }

        pixel: fn() {
            let uv = (self.rect_px.xy + self.v_local) * vec2(self.misc_a.x, self.misc_a.x)
            let own = self.top_tex.sample_nearest(uv).x
            if own < 0.999 {
                return vec4(own, own, own, 1.0)
            }
            var best = own
            best = min(best, self.tp_at(vec2(-1.0, -1.0)))
            best = min(best, self.tp_at(vec2(0.0, -1.0)))
            best = min(best, self.tp_at(vec2(1.0, -1.0)))
            best = min(best, self.tp_at(vec2(-1.0, 0.0)))
            best = min(best, self.tp_at(vec2(1.0, 0.0)))
            best = min(best, self.tp_at(vec2(-1.0, 1.0)))
            best = min(best, self.tp_at(vec2(0.0, 1.0)))
            best = min(best, self.tp_at(vec2(1.0, 1.0)))
            return vec4(best, best, best, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawSceneTexture {
    #[deref]
    pub draw_super: DrawQuad,
}

/// DrawCube + per-instance emission (`glow`) and per-instance fog density.
///
/// Instance-field rule: only #[live] instance fields after the deref chain —
/// `DrawVars::as_slice` reads them contiguously. The sun terms and fog colour
/// are deliberately NOT here: they are shader uniforms (see the script block
/// above) set once per frame through [`crate::sun::SunLight::write_uniforms`],
/// which keeps 48 bytes of identical data out of every instance.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneCube {
    #[deref]
    pub cube: DrawCube,
    #[live(0.0)]
    pub glow: f32,
    #[live(0.0)]
    pub fog_density: f32,
}

/// Alpha-blended variant: water, sensor ghosts, blob shadows.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneAlpha {
    #[deref]
    pub cube: DrawSceneCube,
}

/// One firework shell. Every field is an instance: the GPU derives all
/// `SPARKS_PER_SHELL` sparks from these numbers alone.
///
/// Packed into `vec4`s ON PURPOSE. A `Vec3f` instance is tightly packed on the
/// Rust side but a `vec3` obeys 16-byte alignment in the shader ABI, so the
/// three floats are read back misaligned and every field after them shifts —
/// which presents as the whole burst rendering at the world origin, on the
/// ground, instead of where it was placed. The other shaders here get away
/// with `Vec3f` because theirs are declared `uniform`, not instance.
///
/// Four floats at a time is the shape the hardware wants anyway; the packing
/// costs nothing and removes the class of bug entirely.
///
/// # Why this derefs DrawVars and not DrawCube
///
/// It used to deref `DrawCube`, and every instance field read back garbage —
/// the burst rendered at the world origin and the spark size ignored whatever
/// Rust wrote. Encoding the instance values as colour on a fixed clip-space
/// quad settled it: the decoded numbers CHANGED WHEN THE CAMERA ROTATED.
/// Instance data cannot depend on the view, so the shader was reading view
/// memory — the fields were bound at the wrong offsets, not merely wrong.
///
/// Inheriting `DrawCube` brings its own instance fields along, and appending
/// more after them puts these at offsets the script-side layout does not
/// account for. `DrawSceneShadow` — the one shader here that instances
/// correctly — derefs `DrawVars` and declares its uniform buffers, vertex
/// buffer and varyings explicitly, so its instance fields are the only ones
/// and the layout is unambiguous. This now follows that pattern.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneFirework {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(1.0)]
    pub depth_clip: f32,
    /// xyz = burst point, w = seconds since the burst (negative = climbing).
    #[live(vec4(0.0, 30.0, 0.0, 0.0))]
    pub origin_age: Vec4f,
    /// xyz = launch point, w = spark lifetime.
    #[live(vec4(0.0, 0.0, 0.0, 2.0))]
    pub launch_life: Vec4f,
    /// x = spark speed, y = seed, z = spark size, w unused.
    #[live(vec4(12.0, 0.0, 0.6, 0.0))]
    pub params: Vec4f,
    #[live(vec4(1.0, 0.8, 0.4, 1.0))]
    pub color: Vec4f,
    #[live(vec4(1.0, 0.3, 0.1, 1.0))]
    pub color_tail: Vec4f,
}

/// Sky dome, authored-gradient mode: scripts that set their own sky
/// colours keep this four-colour dome. Default-sky worlds draw
/// [`DrawSceneSkyAnalytic`] instead (the split is deliberate — one combined
/// pixel fn sat exactly at a script-shader capacity limit).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneSky {
    #[deref]
    pub cube: DrawCube,
    #[live(vec3(0.32, 0.58, 0.9))]
    pub sky_top: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub sky_horizon: Vec3f,
    #[live(vec3(0.68, 0.75, 0.66))]
    pub sky_ground: Vec3f,
    #[live(vec3(0.3, 0.4, 0.3))]
    pub sky_bottom: Vec3f,
}

/// Sky dome, analytic mode: the Preetham daylight model whose
/// sun-dependent halves arrive from [`crate::sky`], plus the setting sun
/// disc and the rotating night star dome. Drawn for worlds that keep the
/// DEFAULT sky colours; authored gradients use [`DrawSceneSky`].
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneSkyAnalytic {
    #[deref]
    pub cube: DrawCube,
    /// Perez A,B,C,D per channel (E rides in `pz_e`).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub pz_y: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub pz_x: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub pz_yc: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub pz_e: Vec4f,
    /// 1 / F(0, theta_s) per channel.
    #[live(vec4(1.0, 1.0, 1.0, 0.0))]
    pub pz_f0: Vec4f,
    /// Zenith Yxy + night blend in w.
    #[live(vec4(1.0, 0.31, 0.32, 0.0))]
    pub zenith: Vec4f,
    /// MODEL sun direction (clamped ~2 deg for Perez) + exposure in w.
    #[live(vec4(0.0, 1.0, 0.0, 0.12))]
    pub sun_e: Vec4f,
    /// TRUE sun direction, unclamped — the disc/Mie/afterglow ride it
    /// below the horizon.
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub sun_true: Vec4f,
    /// Celestial rotation rows (world dir -> star map dir); row 0's w is
    /// the star gain (0 = no map bound).
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub star_r0: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub star_r1: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 0.0))]
    pub star_r2: Vec4f,
}

/// Skinned character mesh (PbrVertex layout, uv in ny_nz_uv.zw, textured).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneSkinned {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(1.0)]
    pub depth_clip: f32,
    /// 1.0 = show baked AO alone, contrast-stretched (SANDBOX_AO_DEBUG=1).
    #[live(0.0)]
    pub ao_debug: f32,
    /// 1.0 when this pack has a baked AO atlas bound.
    #[live(0.0)]
    pub ao_enabled: f32,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub fog_color: Vec3f,
    #[live(0.0)]
    pub fog_density: f32,
    /// Sun terms, written every frame from one [`crate::sun::SunLight`].
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
    /// Per-character wash over the vertex tint. One rig serves a whole village,
    /// so without this every passer-by is the same knight in the same colours —
    /// the identical-clones failure the prop variety work just fixed. Costs one
    /// vec4 on an instance stream that carries a handful of characters, and one
    /// multiply in the vertex stage.
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub tint: Vec4f,
    /// This instance's window into the baked light atlas (offset uv, scale
    /// uv). Zero scale = no lightmap: full analytic sun, no lamp light —
    /// dynamics and unbaked models render exactly as before.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub lm_rect: Vec4f,
    /// 1.0 = show the baked light alone (SANDBOX_LM_DEBUG=1).
    #[live(0.0)]
    pub lm_debug: f32,
    /// Dynamic-light gate: 1.0 for dynamic instances (sum every light slot),
    /// 0.0 for statics (sum only the transient prefix — their lamp light is
    /// already baked into the atlas, and statics and dynamics of one model
    /// share a draw item, so this must ride the instance stream).
    #[live(0.0)]
    pub dl_apply: f32,
    /// Ground height under a DYNAMIC instance: the baked-shadow sample is
    /// projected along the sun ray down to this plane. Unused by statics.
    #[live(0.0)]
    pub ground_y: f32,
    /// Depth-tie breaker for coplanar stacked statics. The vertex stage
    /// scales the view-space position by (1 - depth_bias), which the
    /// perspective divide cancels — the on-screen image is EXACTLY the
    /// flush geometry, only the depth-buffer value moves toward the
    /// camera. Placement order feeds it (renderer: depth_order * 1e-3),
    /// so a prop resting on a floor plate wins the z-tie against it
    /// deterministically instead of being physically lifted off it.
    #[live(0.0)]
    pub depth_bias: f32,
    /// Q3 / Unreal detail UV scale. Zero disables the overlay.
    #[live(vec2(0.0, 0.0))]
    pub detail_st: Vec2f,
    /// 1 = vertex COLOR_0 is baked lighting (do not multiply the sun).
    #[live(0.0)]
    pub prelit: f32,
}

/// Minimal camera-space held-model shader. The transform is the only instance
/// lane; daylight is uniform per view and material color comes from the same
/// packed model vertex/texture format as ordinary stock props.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneViewModel {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
}

/// Old-school lamp lens flare: one additive billboard per visible light.
/// Instance fields are vec4-packed for the same shader-ABI reason as
/// [`DrawSceneFirework`] (a vec3 instance reads back misaligned).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneFlare {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(1.0)]
    pub depth_clip: f32,
    /// xyz = world position of the glow, w = billboard size in world units.
    #[live(vec4(0.0, 0.0, 0.0, 1.0))]
    pub flare_pos: Vec4f,
    /// rgb = glow colour, w = intensity multiplier.
    #[live(vec4(1.0, 0.9, 0.6, 1.0))]
    pub flare_col: Vec4f,
}

/// In-world video screen: one upright textured quad, texture updated per
/// frame by the host. Instance fields are vec4-packed for the same
/// shader-ABI reason as [`DrawSceneFlare`].
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneScreen {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(1.0)]
    pub depth_clip: f32,
    /// xyz = world position of the screen centre, w = yaw in radians
    /// (yaw == camera orbit yaw faces that camera squarely).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub screen_pos: Vec4f,
    /// x = width, y = height in world units; zw unused.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub screen_size: Vec4f,
}

/// GPU-skinned character mesh: rest geometry + joint-palette texture.
///
/// A sibling of [`DrawSceneSkinned`] (the DrawSceneFoliage pattern): props keep
/// the cheap fetch-free path, characters opt in to the palette blend. Field
/// order mirrors DrawSceneSkinned — every `#[live]` after the deref is an
/// instance field read contiguously by `DrawVars::as_slice`, and characters
/// of one rig batch into a single draw item, so the whole crowd's state
/// rides this stream. `joint_base` is the instance's first texel in the
/// frame's palette texture.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneSkinnedGpu {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub fog_color: Vec3f,
    #[live(0.0)]
    pub fog_density: f32,
    /// Sun terms, written every frame from one [`crate::sun::SunLight`].
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
    /// Per-character wash over the atlas colours (see DrawSceneSkinned::tint).
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub tint: Vec4f,
    /// First texel of this character's palette in the joint texture.
    #[live(0.0)]
    pub joint_base: f32,
    /// Ground height under this character: the baked-shadow sample is
    /// projected along the sun ray down to this plane (OnChange only — the
    /// Realtime cascades need no ground plane).
    #[live(0.0)]
    pub ground_y: f32,
}

/// Generated foliage: vertex-coloured mesh with growth reveal and wind sway.
///
/// A sibling of [`DrawSceneSkinned`] rather than a mode inside it — the shared
/// shaders draw most of the world and must not carry wind ALU they never use.
/// Both animation weights ride in the packed vertex's existing alpha lane, so
/// opting in costs vertex instructions but zero extra vertex bytes.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneFoliage {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub fog_color: Vec3f,
    #[live(0.0)]
    pub fog_density: f32,
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
    /// Reveal threshold in [0, 1]. 1 = fully grown; defaults so a plant that
    /// nobody animates is simply present.
    #[live(1.0)]
    pub growth: f32,
    /// Width of the smoothstep band that hides growth_t's 16-level
    /// quantisation and makes tips unfurl instead of popping.
    #[live(0.12)]
    pub growth_band: f32,
    #[live(vec3(1.0, 0.0, 0.0))]
    pub wind_dir: Vec3f,
    #[live(0.0)]
    pub wind_strength: f32,
    #[live(0.0)]
    pub wind_gust: f32,
    #[live(0.0)]
    pub wind_time: f32,
}

/// Silhouette shadow mesh: all casters' hulls in one geometry, one draw call.
/// Position and per-vertex alpha only — no lighting, no fog (a shadow lies on
/// ground that is already fogged; fogging it again mixes it toward the bright
/// horizon and a distant shadow comes out lighter than what it darkens).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneShadow {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(1.0)]
    pub depth_clip: f32,
    /// Global dimmer, so a device can soften shadows without a rebuild.
    #[live(1.0)]
    pub shadow_scale: f32,
    /// Debug overlay: magenta at boosted alpha (SANDBOX_SHADOW_DEBUG).
    #[live(0.0)]
    pub shadow_debug: f32,
}

/// SDF silhouette shadow — the dynamic shadow quad every character and
/// driven car draws, shaded in the pixel stage from the caster's baked
/// silhouette-SDF atlas (shadow_sdf.rs,
/// [`crate::shadow_sdf::SDF_CELL`]-square cells — the shader's cell math
/// hardcodes 32 to match). Instance fields are vec4-packed for the
/// shader-ABI reason documented on [`DrawSceneFirework`], and the layout is
/// asserted below.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneShadowSdf {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(1.0)]
    pub depth_clip: f32,
    /// Global dimmer, mirroring [`DrawSceneShadow::shadow_scale`].
    #[live(1.0)]
    pub shadow_scale: f32,
    /// Magenta debug overlay (SANDBOX_SHADOW_DEBUG).
    #[live(0.0)]
    pub shadow_debug: f32,
    /// xyz = quad anchor (y at the receiver), w = receiver lift.
    #[live(vec4(0.0, 0.0, 0.0, 0.012))]
    pub sdf_a: Vec4f,
    /// xy = the owning light's horizontal direction (unit, toward the
    /// light — the sprite's local +x axis in world), z = sprite scale
    /// (footprint x anchor compression), w = final alpha.
    #[live(vec4(1.0, 0.0, 1.0, 0.3))]
    pub sdf_b: Vec4f,
    /// x = relative yaw (character yaw - light azimuth, radians), y = gait
    /// phase 0..1, z = idle-to-walk blend, w = atlas pose rows.
    #[live(vec4(0.0, 0.0, 0.0, 1.0))]
    pub sdf_c: Vec4f,
    /// The atlas window in sprite units: xy = (min_along, min_across),
    /// zw = (size_along, size_across) — every cell shares it.
    #[live(vec4(-1.0, -1.0, 2.0, 2.0))]
    pub sdf_d: Vec4f,
    /// x = base edge half-width in encoded-d units, y = widening per sprite
    /// unit toward the shadow tip (contact hardening), zw unused.
    #[live(vec4(0.06, 0.05, 0.0, 0.0))]
    pub sdf_e: Vec4f,
}

/// The firework ABI lesson, enforced at compile time: everything after
/// `DrawVars` must be exactly the instance lanes the script shader reads —
/// 3 floats + 5 vec4s, contiguous. Any accidental field among them (a bool,
/// an Option, a #[rust]) shifts these offsets and corrupts the GPU instance
/// stream, which presents as shadows at garbage positions. (The struct's
/// total size is NOT asserted: DrawVars aligns to 8, so tail padding could
/// hide a stray trailing f32 — the offsets can't.)
const _: () = {
    let base = std::mem::size_of::<DrawVars>();
    assert!(std::mem::offset_of!(DrawSceneShadowSdf, depth_clip) == base);
    assert!(std::mem::offset_of!(DrawSceneShadowSdf, sdf_a) == base + 3 * 4);
    assert!(std::mem::offset_of!(DrawSceneShadowSdf, sdf_c) == base + 3 * 4 + 2 * 16);
    assert!(std::mem::offset_of!(DrawSceneShadowSdf, sdf_e) == base + 3 * 4 + 4 * 16);
};

/// The smooth terrain mesh (PbrVertex layout: per-vertex color).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneTerrain {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub fog_color: Vec3f,
    #[live(0.0)]
    pub fog_density: f32,
    /// Sun terms, written every frame from one [`crate::sun::SunLight`].
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
    /// The terrain's window into the baked light atlas; zero = none.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub lm_rect: Vec4f,
    /// World xz rect that window covers: (x0, z0, width, depth).
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub lm_world: Vec4f,
    /// 1.0 = show the baked light alone (SANDBOX_LM_DEBUG=1).
    #[live(0.0)]
    pub lm_debug: f32,
}

/// The water sheet (W1): a per-volume grid displaced in the vertex stage by
/// the sim's own wave sum. Wave coefficients ride as UNIFORMS (per draw
/// item, one per volume — a coefficient change starts a new item), so the
/// instance stream stays as small as the terrain's.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneWater {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub fog_color: Vec3f,
    #[live(0.0)]
    pub fog_density: f32,
    /// Sun terms, written every frame from one [`crate::sun::SunLight`].
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
}

// ---------------------------------------------------------------------------
// GPU lightmap baker draw structs (gpu_lightmap.rs). Instance-field rule
// applies throughout: ONLY #[live] instance lanes after the DrawVars deref,
// vec4/mat4-packed (the DrawSceneFirework ABI lesson).
// ---------------------------------------------------------------------------

/// Sun-view depth pass: geometry from a region's fitted ortho sun camera.
/// `sun_r*` are the camera rows: dot(row.xyz, world) + row.w = ndc x/y/z01.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmSunDepth {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub sun_rx: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub sun_ry: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 0.0))]
    pub sun_rz: Vec4f,
    /// x = 1: flipped depth test (far map for the shadow-top plane).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub flip_a: Vec4f,
    /// (sx, sy, ox, oy) clip-space tile mapping: the cascade pass renders
    /// CSM_CASCADES tiles of one strip through this same shader; (1,1,0,0)
    /// = the whole target (every atlas pass).
    #[live(vec4(1.0, 1.0, 0.0, 0.0))]
    pub tile_a: Vec4f,
}

/// Lamp-view depth pass: six 90-degree faces tiled 3x2. `face_r*` are the
/// face view rows; `tile_a` = (sx, sy, ox, oy) clip-space tile mapping;
/// `lamp_range` = (near, far).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmLampDepth {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub face_rx: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub face_ry: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 0.0))]
    pub face_rz: Vec4f,
    #[live(vec4(1.0, 1.0, 0.0, 0.0))]
    pub tile_a: Vec4f,
    #[live(vec4(0.25, 8.0, 0.0, 0.0))]
    pub lamp_range: Vec4f,
}

/// Skinned sun-view depth pass (Realtime characters in the bake): the
/// DrawLmSunDepth projection over a rig's rest mesh, position-skinned
/// against the frame's joint-palette texture. `skin_a.x` = joint_base.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmSunDepthSkinned {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub sun_rx: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub sun_ry: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 0.0))]
    pub sun_rz: Vec4f,
    /// x = 1: flipped depth test (far map for the shadow-top plane).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub flip_a: Vec4f,
    /// x = first palette texel of this caster (joint_base); yzw unused
    /// (vec4-packed per this block's instance-lane rule).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub skin_a: Vec4f,
    /// (sx, sy, ox, oy) clip-space tile mapping — see [`DrawLmSunDepth`].
    #[live(vec4(1.0, 1.0, 0.0, 0.0))]
    pub tile_a: Vec4f,
}

/// Sun gather over a mesh region's chart, at 4x. `target_a` maps chart uv
/// into the mask scratch; `sun_dir_p` = (sun dir, RAY_OFFSET); `params_a` =
/// (depth bias in z01 units, sun-up flag).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmSunGatherMesh {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub sun_rx: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub sun_ry: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 0.0))]
    pub sun_rz: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub target_a: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.02))]
    pub sun_dir_p: Vec4f,
    #[live(vec4(0.001, 1.0, 0.0, 0.0))]
    pub params_a: Vec4f,
}

/// Sun gather over the ground heightfield. `quad_a` places the region quad
/// in the target; `ground_a` = world rect (x0, z0, sx, sz); `hf_a` =
/// (origin_x, origin_z, cell, n).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmSunGatherGround {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub sun_rx: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub sun_ry: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 0.0))]
    pub sun_rz: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.02))]
    pub sun_dir_p: Vec4f,
    #[live(vec4(0.001, 1.0, 0.0, 0.0))]
    pub params_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub ground_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 2.0))]
    pub hf_a: Vec4f,
}

/// 3x3 despeckle vote. `src_a` = (inv_w, inv_h, area_w, area_h).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmDespeckle {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub src_a: Vec4f,
}

/// 4x -> 1x coverage downsample. `rect_a` = dest rect (x, y, w, h) texels.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmDownsample {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(1.0, 1.0, 0.0, 0.0))]
    pub src_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub rect_a: Vec4f,
}

/// Chamfer distance-transform pass. `rect_px` = region rect in atlas
/// texels; `misc_a` = (1/atlas_size, mode).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmChamfer {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub rect_px: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub misc_a: Vec4f,
}

/// Lamp gather over a mesh region's chart at 1x. `lamp_a` = (pos, radius),
/// `lamp_b` = (color, spot), `lamp_c` = (dir, mode: 0 coverage / 1 lamp),
/// `lamp_d` = (near, far, bias, RAY_OFFSET).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmLampGatherMesh {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub target_a: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 8.0))]
    pub lamp_a: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 0.0))]
    pub lamp_b: Vec4f,
    #[live(vec4(0.0, -1.0, 0.0, 1.0))]
    pub lamp_c: Vec4f,
    #[live(vec4(0.25, 8.0, 0.05, 0.02))]
    pub lamp_d: Vec4f,
}

/// Lamp gather over the ground heightfield at 1x.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmLampGatherGround {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub ground_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 2.0))]
    pub hf_a: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 8.0))]
    pub lamp_a: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 0.0))]
    pub lamp_b: Vec4f,
    #[live(vec4(0.0, -1.0, 0.0, 1.0))]
    pub lamp_c: Vec4f,
    #[live(vec4(0.25, 8.0, 0.05, 0.02))]
    pub lamp_d: Vec4f,
}

/// Lamp rim fill / smooth. `misc_a` = (1/atlas_size, mode 0|1 ring, 2 smooth).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmLampDilate {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub rect_px: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub misc_a: Vec4f,
}

/// Final atlas encode. `rect_px` = the EXPANDED rect in atlas texels.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmEncode {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub rect_px: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub misc_a: Vec4f,
}

/// Shadow-top plane conversion. `top_a` = (zr, sun_dir.y, base, range);
/// `params_a` = (sun_up, depth bias z01, RAY_OFFSET).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmTop {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub sun_rx: Vec4f,
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub sun_ry: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 0.0))]
    pub sun_rz: Vec4f,
    #[live(vec4(1.0, 1.0, 0.0, 8.0))]
    pub top_a: Vec4f,
    #[live(vec4(1.0, 0.001, 0.02, 0.0))]
    pub params_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub ground_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 2.0))]
    pub hf_a: Vec4f,
}

/// One ring of min-dilation of the shadow-top plane.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLmTopDilate {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub quad_a: Vec4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub rect_px: Vec4f,
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub misc_a: Vec4f,
}

/// THE baked-shadow penumbra: the smoothstep window every material shader
/// decodes the light atlas's signed-distance A channel through (128 = the
/// silhouette edge; the encode keeps a ±4-texel band, so the window is a
/// pure runtime knob — no re-bake to change it). This pair reads ±2.4
/// texels of the band: on the village ground region (~11.4 texels/unit)
/// that is a ~42 cm penumbra, on model charts (32 texels/unit) ~15 cm —
/// the (0.33, 0.67) window before it measured 24 cm / 8 cm and the user
/// still read the edges as hard. Widening the WINDOW is the whole lever
/// while it stays inside the encoded ±4: the band itself only needs to
/// grow (encode + chamfer step count + this window's world math) if a
/// future look wants penumbras past ±4 texels. Both modes share the atlas
/// encode, so both get it.
///
/// The macro'd shader text cannot interpolate a Rust const, so the pair is
/// written INLINE at every sampler; the test below holds the sites in
/// lockstep with this constant — edit the constant, then the sites it
/// flags. DrawLmTop's lit gate (`lm_a >=`) rides the TOP edge: every texel
/// the window would darken at all must carry a blocker height.
pub const LM_SUN_SOFT: (f32, f32) = (0.2, 0.8);

#[cfg(test)]
mod lm_soft_tests {
    use super::LM_SUN_SOFT;

    /// Every atlas sampler must decode the sun SDF through the SAME window,
    /// or the penumbra differs per material (a cube's shadow edge softer
    /// than the terrain's beside it). Scans the shader source for the
    /// decode idiom and pins each site to [`LM_SUN_SOFT`].
    #[test]
    fn every_sampler_decodes_the_same_penumbra_window() {
        let src = include_str!("shaders.rs");
        let expect = format!("smoothstep({}, {}, ", LM_SUN_SOFT.0, LM_SUN_SOFT.1);
        let mut sites = 0;
        for line in src.lines() {
            // The decode idiom: a smoothstep whose LAST argument is the
            // atlas A channel (`, lm.w)` / `, lmg.w)`) — the shadow-top
            // height compares also smoothstep near `v_lmg.w` but take it
            // as the edge argument, not the sample.
            if line.contains("smoothstep(")
                && (line.contains(", lm.w)") || line.contains(", lmg.w)"))
            {
                assert!(
                    line.contains(&expect),
                    "atlas decode window drifted from LM_SUN_SOFT {:?}: {}",
                    LM_SUN_SOFT,
                    line.trim()
                );
                sites += 1;
            }
        }
        assert!(
            sites >= 5,
            "expected the cube/skinned/skinned-gpu/terrain samplers, found {sites}"
        );
    }

    /// DrawLmTop's lit gate must sit at the decode window's TOP edge: a
    /// texel the window darkens AT ALL (outer penumbra included) needs a
    /// blocker height in the top plane, or that fringe shadows dynamics at
    /// any altitude (the Realtime "road shadows the villagers" mechanism).
    #[test]
    fn top_plane_lit_gate_matches_the_window_edge() {
        let src = include_str!("shaders.rs");
        // Needle assembled in two halves so this test's own source lines
        // never match the scan (the first test dodges the same trap by
        // splitting its needle across lines).
        let gate = format!("if lm_a{}", " >= ");
        let expect = format!("{}{} {{", gate, LM_SUN_SOFT.1);
        let mut sites = 0;
        for line in src.lines() {
            if line.contains(&gate) {
                assert!(
                    line.contains(&expect),
                    "DrawLmTop lit gate drifted from LM_SUN_SOFT.1 {}: {}",
                    LM_SUN_SOFT.1,
                    line.trim()
                );
                sites += 1;
            }
        }
        assert_eq!(sites, 1, "expected exactly the DrawLmTop gate");
    }
}
