//! The five game draw shaders, moved verbatim from gamemaker's game_view.rs.
//! DrawGameTexture composites the offscreen 3D pass into the host pane; the
//! cube/alpha/sky/terrain family renders the world itself.

use makepad_draw::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    mod.draw.DrawGameTexture = mod.std.set_type_default() do #(DrawGameTexture::script_shader(vm)){
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
    mod.draw.DrawGameCube = mod.std.set_type_default() do #(DrawGameCube::script_shader(vm)){
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
        fog_color: uniform(vec3(0.75, 0.87, 0.96))
        sun_color: uniform(vec3(0.72, 0.72, 0.72))
        sun_sky: uniform(vec3(0.28, 0.28, 0.28))
        sun_ground: uniform(vec3(0.28, 0.28, 0.28))

        vertex: fn() {
            let pos = self.get_size() * self.geom.geom_pos + self.get_pos()
            let model_view = self.draw_list.view_transform * self.transform
            let normal4 = model_view * vec4(
                self.geom.geom_normal.x,
                self.geom.geom_normal.y,
                self.geom.geom_normal.z,
                0.0
            )
            let normal = normalize(normal4.xyz)
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let dp = max(dot(normal, normalize(self.light_dir)), 0.0)
            self.lit_color = self.get_color(dp, normal.y)
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        // One lighting model for every game shader (sun.rs): hemisphere
        // ambient by surface-up-ness, plus the sun's direct term. With the
        // default flat sun (sky == ground == 0.28, color 0.72) this is
        // exactly the constant split the shaders each used to hardcode.
        get_color: fn(dp: float, nrm_y: float) {
            let hemi = clamp(nrm_y * 0.5 + 0.5, 0.0, 1.0)
            let ambient = mix(self.sun_ground, self.sun_sky, hemi)
            let lit = self.color.xyz * (ambient + self.sun_color * dp)
            // Emission: glowing eyes, beacons, bolts (energy ramps at runtime).
            let glowing = lit + self.color.xyz * self.glow * 0.6
            return vec4(glowing, self.color.w)
        }

        pixel: fn() {
            let fogged = mix(self.lit_color.xyz, self.fog_color, self.v_fog)
            return vec4(fogged, self.lit_color.w)
        }
    }

    // Same shading, alpha-blended: water, sensor ghosts, blob shadows, and the
    // particle batch.
    mod.draw.DrawGameAlpha = mod.std.set_type_default() do #(DrawGameAlpha::script_shader(vm)){
        ..mod.draw.DrawGameCube
        alpha_blend: true
        // DELIBERATE, do not "fix": this batch carries flat single-sided
        // geometry — blob shadows and water surfaces — whose winding is not
        // guaranteed to face the viewer, and culling a blended surface changes
        // the composite rather than merely hiding a hidden face. Overriding the
        // `true` now inherited from DrawGameCube.
        backface_culling: false
    }

    // Fireworks: ONE instance per shell, expanded on the GPU into
    // `SPARKS_PER_SHELL` sparks whose positions are a closed form of
    // (spark index, seed, age). Nothing is stepped and nothing is uploaded
    // per frame — see firework.rs for why that is the whole point.
    mod.draw.DrawGameFirework = mod.std.set_type_default() do #(DrawGameFirework::script_shader(vm)){
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

        spark_color: fn(life_t: float, heat: float, rnd: float, speed_t: float) -> vec4 {
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
            let n = 512.0 / trail_n
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
            let speed = self.params.x * (0.94 + 0.06 * r3)

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
            let fall = 0.5 * 7.5 * t * t
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
            let styled = self.spark_color(life_t, heat, r1, speed_t)
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

    // Sky dome: a big cube around the camera, gradient by view direction
    // (the Godot ProceduralSkyMaterial look).
    mod.draw.DrawGameSky = mod.std.set_type_default() do #(DrawGameSky::script_shader(vm)){
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
            let y = normalize(self.v_dir).y
            let up = clamp(y * 2.2, 0.0, 1.0)
            let down = clamp((0.0 - y) * 2.2, 0.0, 1.0)
            let sky = mix(self.sky_horizon, self.sky_top, up)
            let ground = mix(self.sky_ground, self.sky_bottom, down)
            let color = mix(ground, sky, step(0.0, y))
            return vec4(color, 1.0)
        }
    }

    // Skinned character mesh: PbrVertex stream (CPU-skinned per frame, uv in
    // ny_nz_uv.zw), textured, lit and fogged like the terrain.
    mod.draw.DrawGameSkinned = mod.std.set_type_default() do #(DrawGameSkinned::script_shader(vm)){
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertex, geom.GameMeshGeom)
        tex: texture_2d(float)
        v_ambient: varying(vec3f)
        v_direct: varying(vec3f)
        v_uv: varying(vec2f)
        v_tint: varying(vec4f)
        world: varying(vec4f)
        v_fog: varying(float)

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
            let normal_in = self.oct_decode(unpack2f16(self.geom.nrm))
            let model_view = self.draw_list.view_transform * self.transform
            let world_normal = normalize((model_view * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let dp = max(dot(world_normal, normalize(self.light_dir)), 0.0)
            let hemi = clamp(world_normal.y * 0.5 + 0.5, 0.0, 1.0)
            self.v_ambient = mix(self.sun_ground, self.sun_sky, hemi)
            self.v_direct = self.sun_color * dp
            self.v_uv = unpack2f16(self.geom.uv)
            // rgb is the material tint (x the per-character wash); the ALPHA
            // lane carries baked self-AO from model.rs, not opacity — this
            // shader has always returned opaque, so the lane was free.
            let vc = unpack4u8(self.geom.color)
            self.v_tint = vec4(vc.x * self.tint.x, vc.y * self.tint.y, vc.z * self.tint.z, vc.w)
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            // Atlas x vertex tint. Kenney ships both conventions — most packs
            // UV-map into one colormap (tint = white), nature-kit and friends
            // carry no texture and colour per material (atlas = white 1x1).
            // Multiplying serves both without a branch or a second shader.
            let tex = self.tex.sample_as_bgra(self.v_uv)
            let albedo = vec3(tex.x * self.v_tint.x, tex.y * self.v_tint.y, tex.z * self.v_tint.z)
            // AO scales AMBIENT only. Ambient is light arriving from
            // everywhere, which is exactly what a crevice blocks; direct
            // sunlight is already zero where the surface faces away. Folding
            // it into both would darken a lit wall twice for the same reason.
            let lit = albedo * (self.v_ambient * self.v_tint.w + self.v_direct)
            return vec4(mix(lit, self.fog_color, self.v_fog), 1.0)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // Generated foliage: the OPT-IN variant that adds growth and wind.
    //
    // Deliberately a sibling of DrawGameSkinned rather than a flag inside it.
    // Wind costs ~20 vertex ALU and growth ~6; the cube shader draws most of
    // the world and must not pay either. A plant opts in by being drawn with
    // this shader; everything else keeps the cheap path untouched.
    //
    // Both animation weights ride in ONE unorm8 lane (the colour's alpha, high
    // nibble = growth order, low = wind flex), so the variant costs zero extra
    // vertex BYTES over the shared 24-byte layout — which is the bottleneck we
    // actually measured.
    mod.draw.DrawGameFoliage = mod.std.set_type_default() do #(DrawGameFoliage::script_shader(vm)){
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
    mod.draw.DrawGameShadow = mod.std.set_type_default() do #(DrawGameShadow::script_shader(vm)){
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
            // Premultiplied black: RGB 0 leaves exactly ground*(1-a), a true
            // multiplicative shadow. Unpremultiplied dark RGB would ADD light.
            return vec4(0.0, 0.0, 0.0, self.v_alpha * self.shadow_scale)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // The smooth terrain mesh: per-vertex colored triangles, flat normals.
    mod.draw.DrawGameTerrain = mod.std.set_type_default() do #(DrawGameTerrain::script_shader(vm)){
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.PbrVertex, geom.PbrGeom)
        lit_color: varying(vec4f)
        world: varying(vec4f)
        v_fog: varying(float)

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
            self.lit_color = vec4(
                self.geom.color.xyz * (ambient + self.sun_color * dp),
                self.geom.color.w
            )
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            return vec4(mix(self.lit_color.xyz, self.fog_color, self.v_fog), self.lit_color.w)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawGameTexture {
    #[deref]
    pub draw_super: DrawQuad,
}

/// DrawCube + per-instance emission (`glow`) and per-instance fog density.
///
/// Instance-field rule: only #[live] instance fields after the deref chain —
/// `DrawVars::as_slice` reads them contiguously. The sun terms and fog colour
/// are deliberately NOT here: they are shader uniforms (see the script block
/// above) set once per frame through [`crate::sun::GameSun::write_uniforms`],
/// which keeps 48 bytes of identical data out of every instance.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGameCube {
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
pub struct DrawGameAlpha {
    #[deref]
    pub cube: DrawGameCube,
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
/// account for. `DrawGameShadow` — the one shader here that instances
/// correctly — derefs `DrawVars` and declares its uniform buffers, vertex
/// buffer and varyings explicitly, so its instance fields are the only ones
/// and the layout is unambiguous. This now follows that pattern.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGameFirework {
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

/// Sky dome gradient (colors are instances so Rust sets them per frame).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGameSky {
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

/// Skinned character mesh (PbrVertex layout, uv in ny_nz_uv.zw, textured).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGameSkinned {
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
    /// Sun terms, written every frame from one [`crate::sun::GameSun`].
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
}

/// Generated foliage: vertex-coloured mesh with growth reveal and wind sway.
///
/// A sibling of [`DrawGameSkinned`] rather than a mode inside it — the shared
/// shaders draw most of the world and must not carry wind ALU they never use.
/// Both animation weights ride in the packed vertex's existing alpha lane, so
/// opting in costs vertex instructions but zero extra vertex bytes.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGameFoliage {
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
pub struct DrawGameShadow {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(1.0)]
    pub depth_clip: f32,
    /// Global dimmer, so a device can soften shadows without a rebuild.
    #[live(1.0)]
    pub shadow_scale: f32,
}

/// The smooth terrain mesh (PbrVertex layout: per-vertex color).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGameTerrain {
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
    /// Sun terms, written every frame from one [`crate::sun::GameSun`].
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
}
