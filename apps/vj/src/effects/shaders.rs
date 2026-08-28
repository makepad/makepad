//! The fx draw shaders: where the vertex-stream data comes alive.
//!
//! Every mesh-family shader reads the engine's attribute channels off the
//! CubeVertex layout (see `mesh.rs` and each engine's doc in `engines.rs`)
//! and animates them from time/beat carried as instance data — nothing on
//! the CPU moves a vertex once the stream is uploaded.
//!
//! # The standard signal block (every fx shader has it, automatically)
//!
//! | field          | contents                                            |
//! |----------------|-----------------------------------------------------|
//! | `self.time_beat` | (time, beat position, beat phase 0..1, eased pulse) |
//! | `self.sig`     | (bar phase 0..1, bpm, audio energy 0..1, dt)        |
//! | `self.user`    | (p0, p1, p2, p3) — document-bound user parameters   |
//! | `self.anim`    | (sway, sway_freq, growth, twist)                    |
//! | `self.shape` `self.flow` | engine-specific (engines.rs uniforms()) |
//! | `self.col_a/b/c/bg` | the document palette                           |
//! | `self.fog`     | (fog density, emissive gain, CONTENT MIX, unused)   |
//!
//! CONTENT COUPLING: `self.fog.z` is the document's `content` strength,
//! PRE-GATED by the host to 0 when input0 holds no real channel video (the
//! animated fallback). Every family folds input0 into its look scaled by
//! it — 0 = exactly the classic standalone look. `tex0` carries input0;
//! `has_content: uniform(0.0)` is the raw gate for behavioral switches.
//!
//! THE LOOK LIVES IN THE DOCUMENT. Everything a family paints is behind a
//! named hook a `shader:` subclass replaces, and every shipped preset
//! carries that hook inline — the defaults here are the fallback for a
//! document that declares none, not the place a preset's look lives. Hooks
//! in this file: `fx_color` (mesh / terrain / ribbon / tunnel / particles /
//! emitters), `fx_displace` (mesh), `fx_center` (the particles MOTION
//! program), `fx_sprite` (particles / emitters). Signatures and inputs:
//! CONTRACT.md, "The shader hooks".
//!
//! All instance fields are `vec4`/float — never `vec3` (instance vec3
//! misaligns in the shader ABI; see the firework shader's war story). Keep
//! additions modest: the script shader compiler has a silent size budget.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom

    // ---------------------------------------------------------------------
    // Solid emissive mesh: l-systems (wind/growth) + metaballs.
    //
    // Wind law: every displacement is a function of CONTINUOUS per-vertex
    // data (rest position, arc length from the root), so joints can never
    // tear — a child branch starts where its parent ends and both evaluate
    // the same field there. The per-branch flutter (branch hash, uv.y) is
    // deliberately small.
    // ---------------------------------------------------------------------
    mod.draw.DrawVjFxMesh = set_type_default() do #(DrawVjFxMesh::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        // Content coupling (CONTRACT.md): shape = (mode: 0 canopy tint —
        // lsystem/grass — / 1 surface refraction — metaballs —, xz half-
        // extent of the footprint, 0, 0), set by engines.rs uniforms().
        tex0: texture_2d(float)
        has_content: uniform(0.0)

        // THE AUDIO PICTURE (effects/audio_tex.rs) — the live show's sound,
        // sampleable. One float texture rewritten every frame from exactly
        // the stream the beat-sync analysis listens to:
        //   y 0 .. spec_rows-1   SPECTROGRAM ring, x = log-spaced bin
        //                        (30 Hz .. 16 kHz), value 0..1 magnitude
        //                        (0 = -72 dBFS, 1 = 0 dBFS), one row per hop
        //   y spec_rows ..       WAVEFORM ring, x = time inside the row,
        //                        value = the SIGNED sample, -1..1 (0 = silence,
        //                        which is also what an unbound slot reads)
        // audio_dim  = (bins, spec_rows, spec_cursor, wave_cursor)
        // audio_meta = (tex_w, tex_h, wave_rows, hop_secs)
        // audio_env  = (bass, mid, high, rms), smoothed 0..1 — no texture
        //              read needed for a plain level.
        // Each `*_cursor` names the NEWEST row of its ring: that is the
        // unwrap key. Use the two helpers, never a raw uv — and remember a
        // silent rig reads 0 everywhere, so give every look an idle floor.
        audio_tex: texture_2d(float)
        audio_dim: uniform(vec4(256.0, 256.0, 0.0, 0.0))
        audio_meta: uniform(vec4(256.0, 320.0, 64.0, 0.0213))
        audio_env: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // Spectrum magnitude 0..1. `f` 0..1 = log frequency (0 = 30 Hz,
        // 1 = 16 kHz); `age` 0..1 = how far back (0 = now, 1 = the oldest
        // row kept, about 5.5 s). Silence reads 0.
        audio_fft: fn(f: float, age: float) -> float {
            let rows = max(self.audio_dim.y, 1.0)
            let x = (clamp(f, 0.0, 1.0) * (self.audio_dim.x - 1.0) + 0.5) / self.audio_meta.x
            let back = clamp(age, 0.0, 1.0) * (rows - 1.0)
            let row = modf(self.audio_dim.z - back + rows * 2.0, rows)
            return self.audio_tex.sample_nearest(vec2(x, (row + 0.5) / self.audio_meta.y), 0.0).x
        }

        // Waveform sample -1..1. `t` 0..1 across the stored window
        // (1 = the newest sample, about 1.4 s deep). Silence reads 0.
        audio_wave: fn(t: float) -> float {
            let bins = max(self.audio_dim.x, 1.0)
            let wrows = max(self.audio_meta.z, 1.0)
            let back = (1.0 - clamp(t, 0.0, 1.0)) * (bins * wrows - 1.0)
            let pos = (bins - 1.0) - back
            let ro = floor(pos / bins)
            let col = pos - ro * bins
            let row = modf(self.audio_dim.w + ro + wrows * 4.0, wrows)
            let y = (self.audio_dim.y + row + 0.5) / self.audio_meta.y
            let uv = vec2((col + 0.5) / self.audio_meta.x, y)
            return self.audio_tex.sample_nearest(uv, 0.0).x
        }
        backface_culling: false
        alpha_blend: false
        depth_write: true

        v_color: varying(vec4f)
        v_normal: varying(vec3f)
        v_world: varying(vec3f)
        v_scr: varying(vec4f)

        // Document hook: extra displacement, world units.
        fx_displace: fn(pos: vec3, normal: vec3, attr: vec4) -> vec3 {
            return vec3(0.0, 0.0, 0.0)
        }

        // Document hook: the look. `t` = the engine's color parameter (arc
        // from root for l-systems, height for metaballs), `attr` the raw
        // channels (depth/dominant-blob, arc/height, hue, radius).
        fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3) -> vec4 {
            let base = self.col_a.mix(self.col_b, clamp(attr.z, 0.0, 1.0))
            let tip = self.col_c * (clamp(t, 0.0, 1.0) * 0.35)
            let flash = self.col_c * (self.time_beat.w * 0.5)
            return vec4(base.xyz + tip.xyz + flash.xyz, 1.0)
        }

        vertex: fn() {
            let attr = vec4(
                self.geom.geom_id,
                self.geom.geom_pad,
                self.geom.geom_tail_pad_0,
                self.geom.geom_tail_pad_1
            )
            let depth = attr.x
            let arc = attr.y
            let branch = self.geom.geom_uv.y
            // Growth: the front (anim.z) sweeps outward along arc length;
            // unborn tube collapses onto its centerline.
            // Growth floor 0.12: the unborn skeleton stays faintly present,
            // so a growth loop never shows an empty frame (VJ dead-air law).
            let center = self.geom.geom_pos - self.geom.geom_normal * attr.w
            let g = clamp((self.anim.z - arc) * 9.0, 0.12, 1.0)
            let grown = center + self.geom.geom_normal * (attr.w * g)
            // Wind: a smooth spatial field sampled at the REST position —
            // continuous across every joint — with amplitude growing along
            // the arc (trunk still, tips travel). This is the whole trick.
            let wf = self.time_beat.x * self.anim.y
            let bend = self.anim.x * arc * arc * 1.4
            let wind = vec3(
                sin(wf + center.y * 0.35 + center.x * 0.20),
                sin(wf * 0.63 + center.y * 0.27 + 2.1) * 0.3,
                cos(wf * 0.81 + center.z * 0.33 + center.y * 0.21)
            ) * bend
            // Flutter: per-branch phase, tiny amplitude, grows with depth.
            let flutter = sin(wf * 3.1 + branch * 41.0) * depth * 0.012 * self.anim.x
            // Twist around Y by height + beat pulse breathing.
            let tw = self.anim.w * grown.y * 0.35
            let cs = cos(tw)
            let sn = sin(tw)
            let twisted = vec3(
                grown.x * cs - grown.z * sn,
                grown.y,
                grown.x * sn + grown.z * cs
            )
            let scale = 1.0 + self.time_beat.w * 0.14
            let mut pos = twisted * scale + wind + vec3(flutter, 0.0, flutter * 0.7)
            pos = pos + self.fx_displace(pos, self.geom.geom_normal, attr)

            let world = self.draw_list.view_transform * vec4(pos.x, pos.y, pos.z, 1.0)
            self.v_world = world.xyz
            self.v_normal = self.geom.geom_normal
            let grow_dim = 0.20 + 0.80 * g
            let c = self.fx_color(arc, attr, self.geom.geom_normal, world.xyz)
            // CONTENT COUPLING — the video PAINTED ON THE PLANT. Always
            // sampled at the REST position, so wind moves the picture with
            // the geometry instead of smearing it. Two projections, chosen
            // by the engine on shape.x:
            //
            //   0 = FIELD (grass): world xz over the meadow half-extent
            //       (shape.y) — the clip lies on the ground the blades
            //       stand in, so the field reads it back as you look
            //       across it.
            //   2 = FRONT (l-system): world x over the plant half-extent
            //       (shape.y) and world y over its HEIGHT (shape.z) — a
            //       slide projector aimed at the plant. The first pass
            //       used xz here too, which for a tall thin tree sampled
            //       one pinhole of the frame and read as a mood, never a
            //       picture.
            //
            // Trunks keep a little of their own colour (arc-weighted), the
            // tips take the clip. fog.z is host-pre-gated to 0 without real
            // content: the standalone look stays classic.
            let mut crgb = c.xyz
            let cmix = self.has_content * self.fog.z
            if cmix > 0.001 && abs(self.shape.x - 1.0) > 0.5 {
                let ext = max(self.shape.y, 0.5)
                let mut cuv = vec2(twisted.x, twisted.z) / (ext * 2.0) + vec2(0.5, 0.5)
                if self.shape.x > 1.5 {
                    let hgt = max(self.shape.z, 0.5)
                    cuv = vec2(twisted.x / (ext * 2.0) + 0.5, 1.0 - twisted.y / hgt)
                }
                let texel = self.tex0.sample_nearest(
                    clamp(cuv, vec2(0.0, 0.0), vec2(1.0, 1.0)),
                    0.0
                )
                let tipness = 0.30 + 0.70 * smoothstep(0.04, 0.45, arc)
                crgb = mix(crgb, texel.xyz * 1.45, clamp(cmix * 1.35, 0.0, 1.0) * tipness)
            }
            self.v_color = vec4(crgb * grow_dim, c.w)
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
            self.v_scr = self.vertex_pos
            return self.vertex_pos
        }

        pixel: fn() {
            let n = normalize(self.v_normal)
            let cam = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
            let cam_pos = cam.xyz / max(cam.w, 0.0001)
            let view_dir = normalize(cam_pos - self.v_world)
            // Emissive core + rim so the silhouette glows against black.
            let rim = pow(1.0 - abs(dot(n, view_dir)), 2.0)
            let lit = 0.55 + 0.45 * abs(dot(n, normalize(vec3(0.4, 0.8, 0.35))))
            let d = length(self.v_world - cam_pos)
            let fog = exp(0.0 - d * self.fog.x)
            // CONTENT COUPLING, refraction mode (shape.x 1 — metaballs):
            // the blob surface refracts the live input0 like glass —
            // screen-space uv bent by the surface normal, strongest face-on
            // (the rims keep their stock glow, so the silhouette reads).
            let mut body = self.v_color.xyz * (lit * self.fog.y)
            let cmix = self.has_content * self.fog.z
            if cmix > 0.001 && self.shape.x > 0.5 {
                let ndc = self.v_scr.xy / max(self.v_scr.w, 0.0001)
                let suv = vec2(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5)
                let bent = clamp(
                    suv + vec2(n.x, 0.0 - n.y) * 0.14,
                    vec2(0.0, 0.0),
                    vec2(1.0, 1.0)
                )
                let texel = self.tex0.sample_as_bgra(bent)
                let glass = texel.xyz * ((0.45 + 0.95 * lit) * self.fog.y)
                body = mix(body, glass, clamp(cmix * 1.2, 0.0, 1.0) * (1.0 - rim * 0.45))
            }
            let glow = body + self.col_c.xyz * rim * 0.55
            let final_rgb = glow.mix(self.col_bg.xyz, 1.0 - fog)
            return vec4(final_rgb, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // ---------------------------------------------------------------------
    // Heightmap terrain: flat grid displaced by fbm (or input texture 0)
    // in the VERTEX shader; neon synthwave grid in the pixel shader.
    // ---------------------------------------------------------------------
    mod.draw.DrawVjFxTerrain = set_type_default() do #(DrawVjFxTerrain::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        tex0: texture_2d(float)
        has_content: uniform(0.0)

        // THE AUDIO PICTURE (effects/audio_tex.rs) — the live show's sound,
        // sampleable. One float texture rewritten every frame from exactly
        // the stream the beat-sync analysis listens to:
        //   y 0 .. spec_rows-1   SPECTROGRAM ring, x = log-spaced bin
        //                        (30 Hz .. 16 kHz), value 0..1 magnitude
        //                        (0 = -72 dBFS, 1 = 0 dBFS), one row per hop
        //   y spec_rows ..       WAVEFORM ring, x = time inside the row,
        //                        value = the SIGNED sample, -1..1 (0 = silence,
        //                        which is also what an unbound slot reads)
        // audio_dim  = (bins, spec_rows, spec_cursor, wave_cursor)
        // audio_meta = (tex_w, tex_h, wave_rows, hop_secs)
        // audio_env  = (bass, mid, high, rms), smoothed 0..1 — no texture
        //              read needed for a plain level.
        // Each `*_cursor` names the NEWEST row of its ring: that is the
        // unwrap key. Use the two helpers, never a raw uv — and remember a
        // silent rig reads 0 everywhere, so give every look an idle floor.
        audio_tex: texture_2d(float)
        audio_dim: uniform(vec4(256.0, 256.0, 0.0, 0.0))
        audio_meta: uniform(vec4(256.0, 320.0, 64.0, 0.0213))
        audio_env: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // Spectrum magnitude 0..1. `f` 0..1 = log frequency (0 = 30 Hz,
        // 1 = 16 kHz); `age` 0..1 = how far back (0 = now, 1 = the oldest
        // row kept, about 5.5 s). Silence reads 0.
        audio_fft: fn(f: float, age: float) -> float {
            let rows = max(self.audio_dim.y, 1.0)
            let x = (clamp(f, 0.0, 1.0) * (self.audio_dim.x - 1.0) + 0.5) / self.audio_meta.x
            let back = clamp(age, 0.0, 1.0) * (rows - 1.0)
            let row = modf(self.audio_dim.z - back + rows * 2.0, rows)
            return self.audio_tex.sample_nearest(vec2(x, (row + 0.5) / self.audio_meta.y), 0.0).x
        }

        // Waveform sample -1..1. `t` 0..1 across the stored window
        // (1 = the newest sample, about 1.4 s deep). Silence reads 0.
        audio_wave: fn(t: float) -> float {
            let bins = max(self.audio_dim.x, 1.0)
            let wrows = max(self.audio_meta.z, 1.0)
            let back = (1.0 - clamp(t, 0.0, 1.0)) * (bins * wrows - 1.0)
            let pos = (bins - 1.0) - back
            let ro = floor(pos / bins)
            let col = pos - ro * bins
            let row = modf(self.audio_dim.w + ro + wrows * 4.0, wrows)
            let y = (self.audio_dim.y + row + 0.5) / self.audio_meta.y
            let uv = vec2((col + 0.5) / self.audio_meta.x, y)
            return self.audio_tex.sample_nearest(uv, 0.0).x
        }
        backface_culling: false
        alpha_blend: false
        depth_write: true

        v_color: varying(vec4f)
        v_uv: varying(vec2f)
        v_h: varying(float)
        v_world: varying(vec3f)

        hash2: fn(p: vec2) -> float {
            return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453)
        }

        vnoise: fn(p: vec2) -> float {
            let i = floor(p)
            let f = fract(p)
            let u = f * f * (vec2(3.0, 3.0) - f * 2.0)
            let a = self.hash2(i)
            let b = self.hash2(i + vec2(1.0, 0.0))
            let c = self.hash2(i + vec2(0.0, 1.0))
            let d = self.hash2(i + vec2(1.0, 1.0))
            return mix(mix(a, b, u.x), mix(c, d, u.x), u.y)
        }

        // Height at grid uv: 3-octave fbm scrolled along v, optionally
        // replaced by input texture luminance (flow.y = tex_displace).
        //
        // SCROLL SIGN: the grid runs uv.y 0 at z = -size/2 (the FAR end —
        // the camera sits at +z looking down -z) to 1 behind the camera,
        // so a feature sampled at `uv + scroll` walks to DECREASING uv.y
        // as scroll grows: away from the viewer. That reads as flying
        // BACKWARDS over the land (reported on the jet's twin of this
        // code). Negative scroll walks it toward the camera.
        height_at: fn(uv: vec2) -> float {
            let scroll = vec2(0.0, 0.0 - self.time_beat.x * self.flow.x * 0.06)
            let p = (uv + scroll) * (self.shape.x * self.shape.z)
            let h1 = self.vnoise(p)
            let h2 = self.vnoise(p * 2.03 + vec2(11.0, 7.0))
            let h3 = self.vnoise(p * 4.07 + vec2(3.0, 23.0))
            let mut h = h1 * 0.60 + h2 * 0.28 + h3 * 0.12
            // Ridged option: fold around the middle for canyon walls.
            h = mix(h, 1.0 - abs(h * 2.0 - 1.0), self.shape.w)
            // Keep a flat corridor down the middle so the flythrough
            // always has somewhere to fly.
            let corridor = smoothstep(0.02, 0.22, abs(uv.x - 0.5))
            let hh = h * corridor
            // VERTEX-stage fetch: sample_nearest with an explicit lod is the
            // vertex-legal sampler (see the GPU-skinned palette fetch).
            // The drape is periodic in whole uv units, so it reads a
            // WRAPPED scroll — an hours-old clock must not eat the uv's
            // precision. The fbm domain above cannot wrap (the land would
            // jump), so it keeps the raw scroll.
            let wrap = vec2(0.0,
                0.0 - modf(self.time_beat.x * self.flow.x * 0.06, 1.0))
            let tex = self.tex0.sample_nearest(fract(uv + wrap), 0.0)
            let lum = dot(tex.xyz, vec3(0.299, 0.587, 0.114))
            return mix(hh, lum, self.flow.y)
        }

        vertex: fn() {
            let uv = self.geom.geom_uv
            let amp = self.shape.y * (1.0 + self.time_beat.w * 0.35)
            let h = self.height_at(uv)
            let e = 0.012
            let hx = self.height_at(uv + vec2(e, 0.0))
            let hz = self.height_at(uv + vec2(0.0, e))
            let pos = vec3(
                self.geom.geom_pos.x,
                h * amp,
                self.geom.geom_pos.z
            )
            let world = self.draw_list.view_transform * vec4(pos.x, pos.y, pos.z, 1.0)
            self.v_world = world.xyz
            self.v_uv = uv
            self.v_h = h
            // Slope-shade from the finite differences (cheap normal-ish).
            let slope = clamp(1.0 - (abs(hx - h) + abs(hz - h)) * 10.0, 0.35, 1.0)
            self.v_color = vec4(slope, 0.0, 0.0, 1.0)
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
            return self.vertex_pos
        }

        // THE LOOK — pixel stage, doc-replaceable (CONTRACT.md).
        //   t       = height 0..1
        //   attr    = (slope shade, grid line 0..1, grid uv.x, grid uv.y)
        //   content = input0 at the SCROLLED grid uv (the drape sample)
        //   cmix    = pre-gated content strength (self.fog.z; 0 = classic)
        // Returns the land's colour before fog and the glow gain.
        //
        // CONTENT — THE DRAPE. The channel video is PAINTED ON THE LAND:
        // one copy over the whole grid, sampled by the SAME scrolled uv as
        // the wireframe, so the clip streams with the surface and every
        // ridge carries its own piece of the picture. The classic surface
        // term is a 0.30 whisper under the neon — far too dark to read a
        // picture through — so the drape REPLACES it (lit by the same slope
        // shade, so the land still looks like land) instead of tinting it,
        // and only the grid keeps a tint.
        fx_color: fn(t: float, attr: vec4, content: vec4, cmix: float) -> vec4 {
            let grad = self.col_a.mix(self.col_b, t).xyz
            let beat_glow = 1.0 + self.time_beat.w * 1.1
            let drape = content.xyz * (0.42 + 0.85 * attr.x)
                * (1.0 + self.time_beat.w * 0.30)
            let surface = (grad * 0.30 * attr.x)
                .mix(drape, clamp(cmix * 1.25, 0.0, 1.0))
            let gline = grad.mix(content.xyz * 1.3, cmix * 0.55)
            let neon = gline * attr.y * beat_glow * 1.6
                + self.col_c.xyz * attr.y * t * 0.6
            return vec4(surface + neon, 1.0)
        }

        pixel: fn() {
            // Neon wireframe grid over a dark surface, colored by height.
            // Same NEGATIVE sign as `height_at`: grid, drape and land must
            // travel together, toward the camera.
            let cells = 40.0
            // BOUNDED SCROLL: grid and drape are both periodic in whole uv
            // units, so the wrap is the same picture on a small number.
            let scroll = vec2(0.0,
                0.0 - modf(self.time_beat.x * self.flow.x * 0.06, 1.0))
            let g = fract((self.v_uv + scroll) * cells)
            let dg = min(min(g.x, 1.0 - g.x), min(g.y, 1.0 - g.y))
            let line = 1.0 - smoothstep(0.0, 0.11, dg)
            let t = clamp(self.v_h * 1.4, 0.0, 1.0)
            let ttx = self.tex0.sample_as_bgra(fract(self.v_uv + scroll))
            let look = self.fx_color(
                t,
                vec4(self.v_color.x, line, self.v_uv.x, self.v_uv.y),
                ttx,
                self.fog.z
            )
            let cam = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
            let d = length(self.v_world - cam.xyz / max(cam.w, 0.0001))
            let fog = exp(0.0 - d * self.fog.x)
            let rgb = (look.xyz * self.fog.y).mix(self.col_bg.xyz, 1.0 - fog)
            return vec4(rgb, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // ---------------------------------------------------------------------
    // Ribbons: strips expanded view-facing in the vertex shader.
    // Channels: a_id ribbon, a_aux age, a_r0 hue, a_r1 side ±1,
    // uv = (speed01, age), normal = trail tangent.
    // ---------------------------------------------------------------------
    mod.draw.DrawVjFxRibbon = set_type_default() do #(DrawVjFxRibbon::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        tex0: texture_2d(float)
        has_content: uniform(0.0)

        // THE AUDIO PICTURE (effects/audio_tex.rs) — the live show's sound,
        // sampleable. One float texture rewritten every frame from exactly
        // the stream the beat-sync analysis listens to:
        //   y 0 .. spec_rows-1   SPECTROGRAM ring, x = log-spaced bin
        //                        (30 Hz .. 16 kHz), value 0..1 magnitude
        //                        (0 = -72 dBFS, 1 = 0 dBFS), one row per hop
        //   y spec_rows ..       WAVEFORM ring, x = time inside the row,
        //                        value = the SIGNED sample, -1..1 (0 = silence,
        //                        which is also what an unbound slot reads)
        // audio_dim  = (bins, spec_rows, spec_cursor, wave_cursor)
        // audio_meta = (tex_w, tex_h, wave_rows, hop_secs)
        // audio_env  = (bass, mid, high, rms), smoothed 0..1 — no texture
        //              read needed for a plain level.
        // Each `*_cursor` names the NEWEST row of its ring: that is the
        // unwrap key. Use the two helpers, never a raw uv — and remember a
        // silent rig reads 0 everywhere, so give every look an idle floor.
        audio_tex: texture_2d(float)
        audio_dim: uniform(vec4(256.0, 256.0, 0.0, 0.0))
        audio_meta: uniform(vec4(256.0, 320.0, 64.0, 0.0213))
        audio_env: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // Spectrum magnitude 0..1. `f` 0..1 = log frequency (0 = 30 Hz,
        // 1 = 16 kHz); `age` 0..1 = how far back (0 = now, 1 = the oldest
        // row kept, about 5.5 s). Silence reads 0.
        audio_fft: fn(f: float, age: float) -> float {
            let rows = max(self.audio_dim.y, 1.0)
            let x = (clamp(f, 0.0, 1.0) * (self.audio_dim.x - 1.0) + 0.5) / self.audio_meta.x
            let back = clamp(age, 0.0, 1.0) * (rows - 1.0)
            let row = modf(self.audio_dim.z - back + rows * 2.0, rows)
            return self.audio_tex.sample_nearest(vec2(x, (row + 0.5) / self.audio_meta.y), 0.0).x
        }

        // Waveform sample -1..1. `t` 0..1 across the stored window
        // (1 = the newest sample, about 1.4 s deep). Silence reads 0.
        audio_wave: fn(t: float) -> float {
            let bins = max(self.audio_dim.x, 1.0)
            let wrows = max(self.audio_meta.z, 1.0)
            let back = (1.0 - clamp(t, 0.0, 1.0)) * (bins * wrows - 1.0)
            let pos = (bins - 1.0) - back
            let ro = floor(pos / bins)
            let col = pos - ro * bins
            let row = modf(self.audio_dim.w + ro + wrows * 4.0, wrows)
            let y = (self.audio_dim.y + row + 0.5) / self.audio_meta.y
            let uv = vec2((col + 0.5) / self.audio_meta.x, y)
            return self.audio_tex.sample_nearest(uv, 0.0).x
        }
        backface_culling: false
        alpha_blend: true
        depth_write: false

        v_color: varying(vec4f)
        v_side: varying(float)
        v_cuv: varying(vec2f)

        fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3) -> vec4 {
            let hue = attr.z
            let base = self.col_a.mix(self.col_b, hue).mix(self.col_c, self.time_beat.w * 0.5)
            return base
        }

        vertex: fn() {
            let attr = vec4(
                self.geom.geom_id,
                self.geom.geom_pad,
                self.geom.geom_tail_pad_0,
                self.geom.geom_tail_pad_1
            )
            let age = attr.y
            let side = attr.w
            let speed01 = self.geom.geom_uv.x
            let world = self.draw_list.view_transform
                * vec4(self.geom.geom_pos.x, self.geom.geom_pos.y, self.geom.geom_pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * world
            // Expand across the view-projected tangent: a ribbon that always
            // faces the camera whatever the trail's 3D bend.
            let tan_view = (self.draw_pass.camera_view
                * (self.draw_list.view_transform
                    * vec4(self.geom.geom_normal.x, self.geom.geom_normal.y, self.geom.geom_normal.z, 0.0))).xyz
            let across = normalize(vec2(0.0 - tan_view.y, tan_view.x) + vec2(0.0001, 0.0))
            let taper = (1.0 - age) * (0.35 + 0.65 * smoothstep(0.0, 0.15, age))
            let width = self.shape.x * taper * (1.0 + self.time_beat.w * 0.6)
            let offset = across * (side * width)
            let billboard = vec4(view_pos.x + offset.x, view_pos.y + offset.y, view_pos.z, view_pos.w)
            let c = self.fx_color(age, attr, self.geom.geom_normal, world.xyz)
            let head = pow(1.0 - age, 1.6)
            // Faster trails run brighter — the speed01 channel at work.
            let energy = 0.35 + 1.15 * head + speed01 * 0.6
            self.v_color = vec4(c.xyz * energy, (1.0 - age) * 0.9)
            self.v_side = side
            self.vertex_pos = self.draw_pass.camera_projection * billboard
            // Content-coupling uv: THE RIBBON'S OWN SCREEN POSITION. A
            // scanline per ribbon (the first pass) gave every strip one
            // flat colour band — a palette, never a picture. Painting each
            // fragment with the video BEHIND it turns the swarm into a
            // brush that reveals the clip as it sweeps, and the strips
            // still carry their own head-bright envelope.
            let ndc = self.vertex_pos.xy / max(self.vertex_pos.w, 0.0001)
            self.v_cuv = clamp(
                vec2(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5),
                vec2(0.0, 0.0),
                vec2(1.0, 1.0)
            )
            return self.vertex_pos
        }

        pixel: fn() {
            // Soft edges across the strip; premultiplied additive.
            let edge = 1.0 - abs(self.v_side)
            let a = smoothstep(0.0, 0.6, edge + 0.55) * self.v_color.w
            // CONTENT COUPLING: the live input0 streams along the ribbon
            // (u scrolled by time). Brightness keeps the trail's own
            // head-bright envelope — motion + taper stay the identity, the
            // video becomes the ink. fog.z pre-gated: 0 = classic look.
            let mut rgb = self.v_color.xyz
            let cmix = self.has_content * self.fog.z
            if cmix > 0.001 {
                let texel = self.tex0.sample_as_bgra(self.v_cuv)
                // Clamped luminance transfer: the head stays brighter than
                // the tail, but a bright frame can never blow the ribbon
                // out to white — the video's own hue must survive.
                let lum = clamp(dot(rgb, vec3(0.299, 0.587, 0.114)), 0.0, 1.2)
                rgb = mix(rgb, texel.xyz * (0.55 + 1.1 * lum), clamp(cmix * 1.15, 0.0, 1.0))
            }
            return vec4(rgb * a, 0.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // ---------------------------------------------------------------------
    // Tunnel interior: scrolling neon bands, beat-pumped. Channels:
    // a_id around01, a_aux along01, a_r1 tube radius, uv = (around, along).
    // ---------------------------------------------------------------------
    mod.draw.DrawVjFxTunnel = set_type_default() do #(DrawVjFxTunnel::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        tex0: texture_2d(float)
        has_content: uniform(0.0)

        // THE AUDIO PICTURE (effects/audio_tex.rs) — the live show's sound,
        // sampleable. One float texture rewritten every frame from exactly
        // the stream the beat-sync analysis listens to:
        //   y 0 .. spec_rows-1   SPECTROGRAM ring, x = log-spaced bin
        //                        (30 Hz .. 16 kHz), value 0..1 magnitude
        //                        (0 = -72 dBFS, 1 = 0 dBFS), one row per hop
        //   y spec_rows ..       WAVEFORM ring, x = time inside the row,
        //                        value = the SIGNED sample, -1..1 (0 = silence,
        //                        which is also what an unbound slot reads)
        // audio_dim  = (bins, spec_rows, spec_cursor, wave_cursor)
        // audio_meta = (tex_w, tex_h, wave_rows, hop_secs)
        // audio_env  = (bass, mid, high, rms), smoothed 0..1 — no texture
        //              read needed for a plain level.
        // Each `*_cursor` names the NEWEST row of its ring: that is the
        // unwrap key. Use the two helpers, never a raw uv — and remember a
        // silent rig reads 0 everywhere, so give every look an idle floor.
        audio_tex: texture_2d(float)
        audio_dim: uniform(vec4(256.0, 256.0, 0.0, 0.0))
        audio_meta: uniform(vec4(256.0, 320.0, 64.0, 0.0213))
        audio_env: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // Spectrum magnitude 0..1. `f` 0..1 = log frequency (0 = 30 Hz,
        // 1 = 16 kHz); `age` 0..1 = how far back (0 = now, 1 = the oldest
        // row kept, about 5.5 s). Silence reads 0.
        audio_fft: fn(f: float, age: float) -> float {
            let rows = max(self.audio_dim.y, 1.0)
            let x = (clamp(f, 0.0, 1.0) * (self.audio_dim.x - 1.0) + 0.5) / self.audio_meta.x
            let back = clamp(age, 0.0, 1.0) * (rows - 1.0)
            let row = modf(self.audio_dim.z - back + rows * 2.0, rows)
            return self.audio_tex.sample_nearest(vec2(x, (row + 0.5) / self.audio_meta.y), 0.0).x
        }

        // Waveform sample -1..1. `t` 0..1 across the stored window
        // (1 = the newest sample, about 1.4 s deep). Silence reads 0.
        audio_wave: fn(t: float) -> float {
            let bins = max(self.audio_dim.x, 1.0)
            let wrows = max(self.audio_meta.z, 1.0)
            let back = (1.0 - clamp(t, 0.0, 1.0)) * (bins * wrows - 1.0)
            let pos = (bins - 1.0) - back
            let ro = floor(pos / bins)
            let col = pos - ro * bins
            let row = modf(self.audio_dim.w + ro + wrows * 4.0, wrows)
            let y = (self.audio_dim.y + row + 0.5) / self.audio_meta.y
            let uv = vec2((col + 0.5) / self.audio_meta.x, y)
            return self.audio_tex.sample_nearest(uv, 0.0).x
        }
        backface_culling: false
        alpha_blend: false
        depth_write: true

        v_color: varying(vec4f)
        v_uv: varying(vec2f)
        v_world: varying(vec3f)

        // THE LOOK — the tube's own colour, VERTEX stage, doc-replaceable.
        //   t       = along the bore 0..1
        //   attr    = (around 0..1, along 0..1, ring hash, tube radius)
        //   normal  = the wall's inward normal
        //   wpos    = world position
        //   content = input0 at THIS VERTEX's wall uv (the same wrap the
        //             drape uses), or black when nothing is bound
        //   cmix    = pre-gated content strength (self.fog.z; 0 = classic)
        // What comes back lights the beat rings and the longitudinal rails
        // and tints the unpapered wall; the DRAPE itself is `fx_wall`.
        fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3,
                     content: vec4, cmix: float) -> vec4 {
            return self.col_a.mix(self.col_b, t)
        }

        // THE DRAPE — what the live video looks like ON the bore, PIXEL
        // stage, doc-replaceable. Returns the wall's rgb; the engine mixes
        // it over the tinted wall by the content strength, so at
        // `content: 1` this function owns every wall pixel.
        //   uv      = the wall uv the engine sampled (around, scrolled along)
        //   content = input0 there
        //   cmix    = pre-gated content strength
        //   ring    = this fragment's beat-ring intensity 0..1
        fx_wall: fn(uv: vec2, content: vec4, cmix: float, ring: float) -> vec3 {
            return content.xyz * (0.55 + 0.22 * self.time_beat.w + ring * 0.30)
        }

        // NOTE: the wall uv (input0 wrapped once around the circumference,
        // three copies along the tube, scrolled with the flight) is spelled
        // out in BOTH stages rather than shared through a helper — a fn the
        // vertex path calls cannot also be called from pixel (CONTRACT.md).
        // Keep the two spellings in step.

        vertex: fn() {
            let attr = vec4(
                self.geom.geom_id,
                self.geom.geom_pad,
                self.geom.geom_tail_pad_0,
                self.geom.geom_tail_pad_1
            )
            // Radial wobble: rings breathe with the beat and a slow wave
            // travels along the tube (a_r1 = tube radius, normal inward).
            let center = self.geom.geom_pos - self.geom.geom_normal * (0.0 - attr.w)
            let wob = 1.0
                + self.time_beat.w * 0.10
                + sin(attr.y * 44.0 + self.time_beat.x * 2.2) * 0.035
            let pos = center + self.geom.geom_normal * (0.0 - attr.w * wob)
            let world = self.draw_list.view_transform * vec4(pos.x, pos.y, pos.z, 1.0)
            self.v_world = world.xyz
            self.v_uv = self.geom.geom_uv
            // The wall texel at THIS vertex, so the look hook can tint the
            // rings and rails with the video. Gated on the content strength
            // (a classic tunnel pays for no fetch), and taken with the
            // vertex-legal sampler: sample_nearest with an explicit lod.
            let vcmix = self.has_content * self.fog.z
            let mut vtex = vec4(0.0, 0.0, 0.0, 1.0)
            if vcmix > 0.001 {
                let vuv = vec2(
                    self.geom.geom_uv.x,
                    fract(self.geom.geom_uv.y * 3.0 + fract(self.time_beat.x * 0.05))
                )
                vtex = self.tex0.sample_nearest(vuv, 0.0)
            }
            self.v_color = self.fx_color(
                attr.y,
                attr,
                self.geom.geom_normal,
                world.xyz,
                vtex,
                vcmix
            )
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
            return self.vertex_pos
        }

        pixel: fn() {
            let along = self.v_uv.y
            let around = self.v_uv.x
            // Rings sweeping along the tube ON THE BEAT + longitudinal rails.
            let ring_ph = fract(along * self.shape.x - fract(self.time_beat.y * 0.25))
            let ring = pow(1.0 - min(ring_ph, 1.0 - ring_ph) * 2.0, 8.0)
            let rail_ph = fract(around * 12.0)
            let rail = pow(1.0 - min(rail_ph, 1.0 - rail_ph) * 2.0, 10.0) * 0.6
            let beat_gain = 0.65 + self.time_beat.w * 1.6
            let cam = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
            let d = length(self.v_world - cam.xyz / max(cam.w, 0.0001))
            let fog = exp(0.0 - d * self.fog.x)
            let neon = self.v_color.xyz * (ring * beat_gain) + self.col_c.xyz * rail
            // CONTENT COUPLING: the live input0 papers the tube wall —
            // wrapped once around the bore, repeated + time-scrolled along
            // it, so the video streams past with the flight and brightens
            // under each passing beat ring. The neon rings/rails stay on
            // top: the identity is the geometry and the beat, the video is
            // the wall it flies through. fog.z pre-gated: 0 = classic.
            // WHAT the wall looks like is the `fx_wall` hook's business —
            // its default is the stock drape, and a document that replaces
            // it owns every wall pixel at `content: 1`.
            let mut base = self.v_color.xyz * 0.06
            let cmix = self.has_content * self.fog.z
            if cmix > 0.001 {
                // THREE copies along the bore: `along` spans the WHOLE tube,
                // so one copy stretches a frame over tens of world units
                // and every ring shows a single smeared row — three copies
                // is roughly square once the circumference is unrolled, and
                // that is the difference between a texture and a picture.
                let cuv = vec2(around, fract(along * 3.0 + fract(self.time_beat.x * 0.05)))
                let texel = self.tex0.sample_as_bgra(cuv)
                // Wall level stays UNDER 1 in the stock drape: this family
                // usually runs a bloom stage, and a bore papered at full
                // gain bloomed the whole frame to white (measured).
                let wall = self.fx_wall(cuv, texel, cmix, ring)
                base = mix(base, wall, clamp(cmix * 1.2, 0.0, 1.0))
            }
            let rgb = ((base + neon) * self.fog.y).mix(self.col_bg.xyz, 1.0 - fog)
            return vec4(rgb, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // ---------------------------------------------------------------------
    // Stateless GPU particles: the CPU uploads one quad per particle ONCE;
    // every frame this vertex shader re-derives each particle's position
    // from (id, seeds, time, beat). Seven motion programs share the sheet.
    // ---------------------------------------------------------------------
    mod.draw.DrawVjFxParticles = set_type_default() do #(DrawVjFxParticles::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        tex0: texture_2d(float)
        has_content: uniform(0.0)

        // THE AUDIO PICTURE (effects/audio_tex.rs) — the live show's sound,
        // sampleable. One float texture rewritten every frame from exactly
        // the stream the beat-sync analysis listens to:
        //   y 0 .. spec_rows-1   SPECTROGRAM ring, x = log-spaced bin
        //                        (30 Hz .. 16 kHz), value 0..1 magnitude
        //                        (0 = -72 dBFS, 1 = 0 dBFS), one row per hop
        //   y spec_rows ..       WAVEFORM ring, x = time inside the row,
        //                        value = the SIGNED sample, -1..1 (0 = silence,
        //                        which is also what an unbound slot reads)
        // audio_dim  = (bins, spec_rows, spec_cursor, wave_cursor)
        // audio_meta = (tex_w, tex_h, wave_rows, hop_secs)
        // audio_env  = (bass, mid, high, rms), smoothed 0..1 — no texture
        //              read needed for a plain level.
        // Each `*_cursor` names the NEWEST row of its ring: that is the
        // unwrap key. Use the two helpers, never a raw uv — and remember a
        // silent rig reads 0 everywhere, so give every look an idle floor.
        audio_tex: texture_2d(float)
        audio_dim: uniform(vec4(256.0, 256.0, 0.0, 0.0))
        audio_meta: uniform(vec4(256.0, 320.0, 64.0, 0.0213))
        audio_env: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // Spectrum magnitude 0..1. `f` 0..1 = log frequency (0 = 30 Hz,
        // 1 = 16 kHz); `age` 0..1 = how far back (0 = now, 1 = the oldest
        // row kept, about 5.5 s). Silence reads 0.
        audio_fft: fn(f: float, age: float) -> float {
            let rows = max(self.audio_dim.y, 1.0)
            let x = (clamp(f, 0.0, 1.0) * (self.audio_dim.x - 1.0) + 0.5) / self.audio_meta.x
            let back = clamp(age, 0.0, 1.0) * (rows - 1.0)
            let row = modf(self.audio_dim.z - back + rows * 2.0, rows)
            return self.audio_tex.sample_nearest(vec2(x, (row + 0.5) / self.audio_meta.y), 0.0).x
        }

        // Waveform sample -1..1. `t` 0..1 across the stored window
        // (1 = the newest sample, about 1.4 s deep). Silence reads 0.
        audio_wave: fn(t: float) -> float {
            let bins = max(self.audio_dim.x, 1.0)
            let wrows = max(self.audio_meta.z, 1.0)
            let back = (1.0 - clamp(t, 0.0, 1.0)) * (bins * wrows - 1.0)
            let pos = (bins - 1.0) - back
            let ro = floor(pos / bins)
            let col = pos - ro * bins
            let row = modf(self.audio_dim.w + ro + wrows * 4.0, wrows)
            let y = (self.audio_dim.y + row + 0.5) / self.audio_meta.y
            let uv = vec2((col + 0.5) / self.audio_meta.x, y)
            return self.audio_tex.sample_nearest(uv, 0.0).x
        }
        backface_culling: false
        alpha_blend: true
        depth_write: false

        v_color: varying(vec4f)
        v_uv: varying(vec2f)

        hash1: fn(x: float) -> float {
            return fract(sin(x * 12.9898) * 43758.5453)
        }

        // Where is particle `id` at life fraction `t01` of cycle `cyc`?
        // Every mode is a closed form — nothing is integrated, so a
        // particle's whole life is one evaluation.
        // Document hook: override for custom motion programs.
        fx_center: fn(mode: float, id: float, dir: vec3, t01: float, cyc: float,
                      r0: float, r1: float) -> vec3 {
            let spread = self.shape.y
            let tau = 6.2831853
            let grav = self.flow.y
            let swirl = self.flow.z
            if mode < 0.5 {
                // BURST fireworks: shells staggered around the volume
                // (particles of one shell SHARE their spawn phase, set on
                // the CPU); drag opens each shell, gravity drifts it down.
                let shell = floor(modf(id, 10.0))
                let sh = shell + cyc * 10.0
                let origin = vec3(
                    (self.hash1(sh * 1.7 + 3.0) - 0.5) * spread * 1.4,
                    (self.hash1(sh * 2.3 + 7.0) - 0.2) * spread * 0.8,
                    (self.hash1(sh * 3.1 + 11.0) - 0.5) * spread * 1.4
                )
                let k = 3.0
                let drag = (1.0 - exp(0.0 - k * t01 * 2.2)) / k
                let speed = spread * (0.55 + 0.20 * r1)
                let fall = grav * t01 * t01 * spread * 0.35
                return origin + dir * speed * drag - vec3(0.0, fall, 0.0)
            }
            if mode < 1.5 {
                // FOUNTAIN: cone up from the floor, parabolic fall.
                let t = t01 * 2.0
                let v = vec3(dir.x * 0.4, abs(dir.y) * 1.1 + 0.9, dir.z * 0.4)
                    * (spread * (0.5 + 0.5 * r1))
                let g = grav * spread * 1.1
                return vec3(0.0, 0.0 - spread * 0.55, 0.0) + v * t
                    - vec3(0.0, 0.5 * g * t * t, 0.0)
            }
            if mode < 2.5 {
                // TUNNEL FLOW: rings of particles streaming past the camera.
                let ang = r1 * tau + self.time_beat.x * swirl * (0.3 + r0 * 0.4)
                let rad = spread * (0.45 + 0.55 * r0)
                let z = mix(0.0 - spread * 6.0, 2.5, t01)
                return vec3(cos(ang) * rad, sin(ang) * rad, z)
            }
            if mode < 3.5 {
                // VORTEX: five braided strands rising in a helix; strand
                // structure is what makes it read as a vortex, not a can.
                let arm = floor(modf(id, 5.0))
                let y = (fract(t01 + r1) * 2.0 - 1.0) * spread * 0.85
                let y01 = y / (spread * 0.85)
                let rad = spread * (0.20 + 0.30 * r0) * (0.55 + 0.45 * abs(y01))
                let ang = arm * 1.2566371 + y01 * 2.4
                    + self.time_beat.x * swirl * 0.9 + r1 * 0.45
                return vec3(cos(ang) * rad, y, sin(ang) * rad)
            }
            if mode < 4.5 {
                // RAIN: sheets falling through the volume with a little wind.
                let x = (self.hash1(id * 3.7 + cyc) - 0.5) * spread * 2.2
                let z = (self.hash1(id * 5.1 + cyc * 1.3) - 0.5) * spread * 2.2
                let y = mix(spread, 0.0 - spread, t01)
                let windx = sin(self.time_beat.x * 0.7 + id) * 0.3
                return vec3(x + windx * t01 * spread * 0.2, y, z)
            }
            if mode < 5.5 {
                // GALAXY: spiral-armed disc, inner stars orbit faster.
                let rad = sqrt(r0) * spread
                let arm = floor(modf(id, 3.0))
                let ang = rad * 1.9 + arm * 2.0944
                    + self.time_beat.x * swirl * 0.55 / (0.25 + rad / spread)
                    + r1 * 0.9
                let y = (r1 - 0.5) * spread * 0.10 * (1.2 - rad / spread)
                return vec3(cos(ang) * rad, y, sin(ang) * rad)
            }
            if mode < 6.5 {
                // IMAGE: home on a 16:9 picture-plane grid with square
                // cells (shape.w = sqrt(count*16/9), rows = w*9/16); a
                // swirl carries the pixels away and back. Spans keep the
                // square look's area: 1.6 x 0.9 = 1.2^2 * 16/9 aspect.
                let w = self.shape.w
                let h = ceil(w * 0.5625)
                let gx = modf(id, w)
                let gy = floor(id / w)
                let home = vec3(
                    (gx / w - 0.5) * spread * 1.6,
                    (0.5 - gy / h) * spread * 0.9,
                    0.0
                )
                let diss = 0.5 - 0.5 * cos(self.time_beat.y * 0.7853982)
                let ang2 = r0 * tau + self.time_beat.x * 0.8
                let away = vec3(
                    cos(ang2) * (0.4 + r1) * spread * 0.8,
                    sin(ang2 * 1.3) * (0.4 + r0) * spread * 0.5,
                    (r1 - 0.5) * spread * 1.5
                )
                return home + away * diss * diss
            }
            if mode < 7.5 {
                // CLOUDS: puff clusters drifting with the wind; each
                // particle belongs to one of 9 clusters and breathes.
                let cl = floor(modf(id, 9.0))
                let ctr = vec3(
                    (self.hash1(cl * 5.1 + 2.0) - 0.5) * spread * 2.4,
                    (self.hash1(cl * 7.3 + 5.0) - 0.2) * spread * 0.5,
                    (self.hash1(cl * 9.7 + 8.0) - 0.5) * spread * 1.6
                )
                let wind = vec3(modf(self.time_beat.x * swirl * 0.25, spread * 4.0), 0.0, 0.0)
                let puff = dir * (spread * (0.14 + 0.30 * r0))
                    * (0.9 + 0.1 * sin(self.time_beat.x * 0.6 + r1 * tau))
                let x = modf(ctr.x + wind.x + spread * 2.0, spread * 4.0) - spread * 2.0
                return vec3(x, ctr.y, ctr.z) + puff
            }
            // PHYLLOTAXIS BLOOM: the sunflower spiral; florets pulse
            // outward on the beat, golden-angle placement.
            let fi = id + 1.0
            let rad2 = sqrt(fi / 900.0) * spread * (0.9 + self.time_beat.w * 0.25)
            let ang3 = fi * 2.3999632
            let lift = sin(rad2 / spread * 3.0 + self.time_beat.x * 0.8) * spread * 0.12
            let bloom = vec3(cos(ang3) * rad2, lift, sin(ang3) * rad2)
            return bloom
        }

        // Document hook: sprite shape. Default = soft additive dot.
        fx_sprite: fn(uv: vec2, tint: vec4) -> vec4 {
            let d = length(uv - vec2(0.5, 0.5)) * 2.0
            let core = 1.0 - smoothstep(0.0, 0.5, d)
            let halo = 1.0 - smoothstep(0.15, 1.0, d)
            let a = clamp(core + halo * 0.5, 0.0, 1.0) * tint.w
            return vec4(tint.x * a, tint.y * a, tint.z * a, 0.0)
        }

        // Document hook: particle tint over its life.
        fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3) -> vec4 {
            let base = self.col_a.mix(self.col_b, attr.z)
            let hot = base.xyz + vec3(0.5, 0.4, 0.3) * pow(1.0 - t, 6.0)
            let fade = (1.0 - t) * smoothstep(0.0, 0.08, t)
            let boost = 0.85 + self.time_beat.w * 0.9
            return vec4(hot * boost, fade)
        }

        vertex: fn() {
            let mode = self.shape.x
            let id = self.geom.geom_id
            let dir = self.geom.geom_normal
            let phase = self.geom.geom_pad
            let rate = self.flow.x
            // BOUNDED LIFE CLOCK. `phase` is this particle's own 0..1 slot
            // in the cycle; added straight onto a clock hours old it loses
            // every bit it owns and the whole field collapses onto a
            // handful of shared phases. Split the clock into whole cycles
            // (a seed — precision irrelevant) and the fraction (the part
            // `phase` has to survive): same t01, same cyc, no aliasing.
            let tc = self.time_beat.x * rate
            let cycles = fract(tc) + phase
            let t01 = fract(cycles)
            let cyc = floor(cycles) + floor(tc)
            // Re-seed per respawn so a particle's next life differs — same
            // rule, the per-particle seed must survive the cycle index.
            let r0 = fract(self.geom.geom_tail_pad_0 + fract(cyc * 0.6180339))
            let r1 = fract(self.geom.geom_tail_pad_1 + fract(cyc * 0.7548776))
            let center = self.fx_center(mode, id, dir, t01, cyc, r0, r1)

            let world = self.draw_list.view_transform
                * vec4(center.x, center.y, center.z, 1.0)
            let view_pos = self.draw_pass.camera_view * world
            // Size taper + beat pump; streaks stretch along Y in view space.
            let size = self.shape.z * (0.6 + 0.8 * r0)
                * (1.0 - t01 * 0.5)
                * (1.0 + self.time_beat.w * 0.5)
            let stretch = self.flow.w
            let corner = self.geom.geom_pos
            let billboard = vec4(
                view_pos.x + corner.x * size,
                view_pos.y + corner.y * size * stretch,
                view_pos.z,
                view_pos.w
            )
            // Hue channel: bursts color PER SHELL per cycle; everything else
            // per particle.
            let mut hue = r0
            if mode < 0.5 {
                let shell = floor(modf(id, 10.0))
                hue = self.hash1(shell * 3.7 + cyc * 0.61 + 5.0)
            }
            let attr = vec4(id, phase, hue, r1)
            let mut tint = self.fx_color(t01, attr, dir, world.xyz)
            // CONTENT COUPLING (fog.z, pre-gated): every particle takes the
            // channel video's texel at its own screen position, so the
            // swarm paints the clip in dots over the shared backdrop
            // (view.rs `backdrop_level`) — motion, sprite and fade stay the
            // classic look's. Near full transfer at the default: a partial
            // mix left the palette in charge and only ever read as a tint.
            let cc = self.draw_pass.camera_projection * view_pos
            let suv = clamp(
                vec2(cc.x, 0.0 - cc.y) / max(cc.w, 0.0001) * 0.5 + vec2(0.5, 0.5),
                vec2(0.0, 0.0),
                vec2(1.0, 1.0)
            )
            let vtex = self.tex0.sample_nearest(suv, 0.0)
            tint = vec4(
                tint.xyz.mix(
                    vtex.xyz * (1.0 + 0.6 * self.time_beat.w),
                    clamp(self.fog.z * 1.2, 0.0, 1.0)
                ),
                tint.w
            )
            if mode > 5.5 && mode < 6.5 {
                // IMAGE mode: the tint IS the texel this particle carries.
                // Vertex-stage fetch, explicit lod (vertex-legal sampler).
                let w = self.shape.w
                let h = ceil(w * 0.5625)
                let uv = vec2(modf(id, w) / w, floor(id / w) / h)
                let texel = self.tex0.sample_nearest(uv, 0.0)
                tint = vec4(texel.xyz * (1.0 + self.time_beat.w * 0.5), 1.0)
            }
            self.v_color = tint
            self.v_uv = self.geom.geom_uv
            self.vertex_pos = self.draw_pass.camera_projection * billboard
            return self.vertex_pos
        }

        pixel: fn() {
            return self.fx_sprite(self.v_uv, self.v_color)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // ---------------------------------------------------------------------
    // Scriptable emitters: ONE instance per emitter (the firework law); the
    // sheet expands into that emitter's particles in this vertex shader.
    // The document's per-frame `frame:` script spawns/moves emitters — it
    // never touches a particle.
    //
    // Per-emitter instance data:
    //   e_origin = (x, y, z, age)        e_vel = (vx, vy, vz, life)
    //   e_par    = (speed, seed, size, style 0..3)
    //   e_par2   = (fraction, gravity, stagger, spread)
    //   e_col_a / e_col_b = this emitter's colors
    // Styles: 0 burst sphere, 1 jet cone along vel, 2 sparkle cloud, 3 ring.
    // ---------------------------------------------------------------------
    mod.draw.DrawVjFxEmitter = set_type_default() do #(DrawVjFxEmitter::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        tex0: texture_2d(float)
        has_content: uniform(0.0)

        // THE AUDIO PICTURE (effects/audio_tex.rs) — the live show's sound,
        // sampleable. One float texture rewritten every frame from exactly
        // the stream the beat-sync analysis listens to:
        //   y 0 .. spec_rows-1   SPECTROGRAM ring, x = log-spaced bin
        //                        (30 Hz .. 16 kHz), value 0..1 magnitude
        //                        (0 = -72 dBFS, 1 = 0 dBFS), one row per hop
        //   y spec_rows ..       WAVEFORM ring, x = time inside the row,
        //                        value = the SIGNED sample, -1..1 (0 = silence,
        //                        which is also what an unbound slot reads)
        // audio_dim  = (bins, spec_rows, spec_cursor, wave_cursor)
        // audio_meta = (tex_w, tex_h, wave_rows, hop_secs)
        // audio_env  = (bass, mid, high, rms), smoothed 0..1 — no texture
        //              read needed for a plain level.
        // Each `*_cursor` names the NEWEST row of its ring: that is the
        // unwrap key. Use the two helpers, never a raw uv — and remember a
        // silent rig reads 0 everywhere, so give every look an idle floor.
        audio_tex: texture_2d(float)
        audio_dim: uniform(vec4(256.0, 256.0, 0.0, 0.0))
        audio_meta: uniform(vec4(256.0, 320.0, 64.0, 0.0213))
        audio_env: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // Spectrum magnitude 0..1. `f` 0..1 = log frequency (0 = 30 Hz,
        // 1 = 16 kHz); `age` 0..1 = how far back (0 = now, 1 = the oldest
        // row kept, about 5.5 s). Silence reads 0.
        audio_fft: fn(f: float, age: float) -> float {
            let rows = max(self.audio_dim.y, 1.0)
            let x = (clamp(f, 0.0, 1.0) * (self.audio_dim.x - 1.0) + 0.5) / self.audio_meta.x
            let back = clamp(age, 0.0, 1.0) * (rows - 1.0)
            let row = modf(self.audio_dim.z - back + rows * 2.0, rows)
            return self.audio_tex.sample_nearest(vec2(x, (row + 0.5) / self.audio_meta.y), 0.0).x
        }

        // Waveform sample -1..1. `t` 0..1 across the stored window
        // (1 = the newest sample, about 1.4 s deep). Silence reads 0.
        audio_wave: fn(t: float) -> float {
            let bins = max(self.audio_dim.x, 1.0)
            let wrows = max(self.audio_meta.z, 1.0)
            let back = (1.0 - clamp(t, 0.0, 1.0)) * (bins * wrows - 1.0)
            let pos = (bins - 1.0) - back
            let ro = floor(pos / bins)
            let col = pos - ro * bins
            let row = modf(self.audio_dim.w + ro + wrows * 4.0, wrows)
            let y = (self.audio_dim.y + row + 0.5) / self.audio_meta.y
            let uv = vec2((col + 0.5) / self.audio_meta.x, y)
            return self.audio_tex.sample_nearest(uv, 0.0).x
        }
        backface_culling: false
        alpha_blend: true
        depth_write: false

        v_color: varying(vec4f)
        v_uv: varying(vec2f)

        hash1: fn(x: float) -> float {
            return fract(sin(x * 12.9898) * 43758.5453)
        }

        fx_sprite: fn(uv: vec2, tint: vec4) -> vec4 {
            let d = length(uv - vec2(0.5, 0.5)) * 2.0
            let core = 1.0 - smoothstep(0.0, 0.4, d)
            let halo = 1.0 - smoothstep(0.1, 0.9, d)
            let a = clamp(core + halo * 0.5, 0.0, 1.0) * tint.w
            return vec4(tint.x * a, tint.y * a, tint.z * a, 0.0)
        }

        // THE LOOK — one spark, doc-replaceable (CONTRACT.md).
        //   t       = life fraction 0..1
        //   attr    = (particle id, seconds alive, ignition rnd, spread rnd)
        //   content = input0 at this spark's screen position
        //   cmix    = pre-gated content strength (0 = classic look)
        // Returns (rgb, fade); the engine multiplies the emitter's own
        // fraction gate onto the alpha afterwards.
        //
        // CONTENT COUPLING: sparks take on the channel video's colour at
        // their screen position — fireworks burst IN the clip's palette;
        // the white-hot birth flash and the fade envelope stay classic.
        fx_color: fn(t: float, attr: vec4, content: vec4, cmix: float) -> vec4 {
            let heat = clamp(1.0 - attr.y * 3.0, 0.0, 1.0)
            let tint0 = self.e_col_a.mix(self.e_col_b, t)
            let pal = tint0.xyz.mix(
                content.xyz * (1.05 + 0.5 * self.time_beat.w),
                clamp(cmix * 1.2, 0.0, 1.0)
            )
            let rgb = mix(pal, vec3(1.0, 0.98, 0.92), heat * heat)
            let fade = (1.0 - t) * (1.0 - t)
            return vec4(rgb, fade)
        }

        vertex: fn() {
            let id = self.geom.geom_id
            let dir = self.geom.geom_normal
            let r0 = self.geom.geom_tail_pad_0
            let r1 = self.geom.geom_tail_pad_1
            let style = self.e_par.w
            let seed = self.e_par.y
            let life = max(self.e_vel.w, 0.05)
            // Ignition stagger: particles light up over `stagger` seconds.
            let ignite = r0 * self.e_par2.z
            let t = max(self.e_origin.w - ignite, 0.0)
            let life_t = clamp(t / life, 0.0, 1.0)
            // The emitter itself travels (a shell climbing its arc).
            let base = self.e_origin.xyz + self.e_vel.xyz * self.e_origin.w
            // Per-emitter re-aim of the direction seed.
            let d2 = normalize(vec3(
                dir.x + (self.hash1(seed + 1.0) - 0.5) * 0.2,
                dir.y + (self.hash1(seed + 2.0) - 0.5) * 0.2,
                dir.z + (self.hash1(seed + 3.0) - 0.5) * 0.2
            ))
            let k = 3.0
            let drag = (1.0 - exp(0.0 - k * t)) / k
            let grav = self.e_par2.y
            let mut off = vec3(0.0, 0.0, 0.0)
            if style < 0.5 {
                // BURST: sphere, drag opens it, terminal-velocity fall.
                let fall = 7.0 * (t - drag)
                off = d2 * (self.e_par.x * (0.94 + 0.06 * r1)) * drag
                    - vec3(0.0, fall * grav * 0.25, 0.0)
            } else {
                if style < 1.5 {
                    // JET: cone hugging the velocity direction, looping
                    // emission (a fountain that lives while the emitter does).
                    let vl = max(length(self.e_vel.xyz), 0.001)
                    let vn = self.e_vel.xyz / vl
                    let cone = normalize(vn * 2.2 + d2 * 0.6)
                    let tp = fract(t * (0.8 + r1 * 0.5) + r0 * 7.0) * 1.2
                    // A particle launched tp seconds ago left from where the
                    // emitter WAS: subtract the emitter's own travel since.
                    off = cone * (self.e_par.x * tp)
                        - vec3(0.0, grav * self.e_par.x * 0.35 * tp * tp, 0.0)
                        - self.e_vel.xyz * tp
                } else {
                    if style < 2.5 {
                        // SPARKLE: a slow-breathing cloud.
                        off = d2 * (self.e_par2.w * (0.3 + 0.7 * r1))
                            * (0.85 + 0.15 * sin(t * 2.0 + r0 * 6.2831853))
                            + vec3(0.0, t * 0.25, 0.0)
                    } else {
                        // RING: a planar shockwave.
                        let ang = r1 * 6.2831853
                        let ring_r = self.e_par.x * drag * 1.4
                        off = vec3(cos(ang) * ring_r, (r0 - 0.5) * 0.15, sin(ang) * ring_r)
                    }
                }
            }
            let center = base + off
            let world = self.draw_list.view_transform
                * vec4(center.x, center.y, center.z, 1.0)
            let view_pos = self.draw_pass.camera_view * world
            // Fraction gate: emitters may use only part of the sheet.
            let gate = 1.0 - step(self.e_par2.x * 4096.0, id)
            let taper = (1.0 - life_t * 0.7)
            let twinkle = 0.75 + 0.25 * sin(r0 * 63.0 + t * 24.0)
            // Pre-ignition particles (t still 0 inside the stagger window)
            // stay invisible.
            let size = self.e_par.z * taper * twinkle * gate * step(0.0005, t)
            let corner = self.geom.geom_pos
            let billboard = vec4(
                view_pos.x + corner.x * size,
                view_pos.y + corner.y * size,
                view_pos.z,
                view_pos.w
            )
            // Color: the document's look hook (emitter palette, white-hot
            // at birth, fading out), fed this spark's own content texel.
            let cc = self.draw_pass.camera_projection * view_pos
            let suv = clamp(
                vec2(cc.x, 0.0 - cc.y) / max(cc.w, 0.0001) * 0.5 + vec2(0.5, 0.5),
                vec2(0.0, 0.0),
                vec2(1.0, 1.0)
            )
            let vtex = self.tex0.sample_nearest(suv, 0.0)
            let c = self.fx_color(life_t, vec4(id, t, r0, r1), vtex, self.fog.z)
            self.v_color = vec4(c.xyz, c.w * gate)
            self.v_uv = self.geom.geom_uv
            self.vertex_pos = self.draw_pass.camera_projection * billboard
            return self.vertex_pos
        }

        pixel: fn() {
            return self.fx_sprite(self.v_uv, self.v_color)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // ---------------------------------------------------------------------
    // THE CONTENT BACKDROP — the shared structural coupling.
    //
    // A family whose classic look is BRIGHT SPARSE GEOMETRY over a near
    // black field (particles, sparks, ribbons, plants, flies, pipes,
    // shards, bars, dominoes, swarms) can never read as "the video is
    // playing" from a tint alone: there is not enough surface to carry a
    // picture. Those families get this quad drawn FIRST, at the far plane,
    // writing no depth — the channel video dimmed behind the effect, the
    // same structure the murmuration and the stock tape already prove.
    //
    // Level = fog.z (the pre-gated `content` strength) x fog.w (the
    // per-family dim published by view.rs `backdrop_level`). The host skips
    // the draw entirely when either is zero, so `content: 0` and the
    // standalone fallback stay bit-exact classic BY CONSTRUCTION.
    // ---------------------------------------------------------------------
    mod.draw.DrawVjFxBackdrop = set_type_default() do #(DrawVjFxBackdrop::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        tex0: texture_2d(float)
        backface_culling: false
        alpha_blend: false
        depth_write: false

        v_uv: varying(vec2f)

        vertex: fn() {
            // Clip-space passthrough at the far plane: the camera never
            // moves it, nothing can be drawn behind it, and it writes no
            // depth so every engine draws straight over it.
            self.v_uv = self.geom.geom_uv
            self.vertex_pos = vec4(
                self.geom.geom_pos.x,
                self.geom.geom_pos.y,
                0.9999,
                1.0
            )
            return self.vertex_pos
        }

        pixel: fn() {
            let tx = self.tex0.sample_as_bgra(self.v_uv)
            // Beat lift + a soft vignette so the effect keeps the centre.
            let lvl = clamp(
                self.fog.z * self.fog.w * (1.0 + self.time_beat.w * 0.14),
                0.0,
                1.0
            )
            let r = length(self.v_uv - vec2(0.5, 0.5))
            let vig = 1.0 - 0.35 * smoothstep(0.28, 0.78, r)
            let rgb = self.col_bg.xyz.mix(tx.xyz, lvl * vig)
            return vec4(rgb, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // ---------------------------------------------------------------------
    // THE FULLSCREEN FAMILY — `engine: "screen"` with a document shader.
    //
    // The screen engine's classic path has no scene pass at all: input0
    // goes straight into the stage chain, and that is what its ten shipped
    // presets are (their look IS the stage list). A document that DECLARES
    // a `shader:` block opts into this pass instead — one clip-space quad
    // whose whole fragment is the document's `fx_color`, with the full
    // signal block, the palette and input0 in hand, and the stage chain
    // still running on top of it. That is the cheapest thing there is to
    // author from scratch: no engine, no geometry, just a pixel function.
    //
    // Opt-in ON PURPOSE: routing the existing presets through a blit would
    // resample input0 before the chain sees it, and they are pixel-frozen.
    // ---------------------------------------------------------------------
    mod.draw.DrawVjFxScreen = set_type_default() do #(DrawVjFxScreen::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        tex0: texture_2d(float)
        has_content: uniform(0.0)

        // THE AUDIO PICTURE (effects/audio_tex.rs) — the live show's sound,
        // sampleable. One float texture rewritten every frame from exactly
        // the stream the beat-sync analysis listens to:
        //   y 0 .. spec_rows-1   SPECTROGRAM ring, x = log-spaced bin
        //                        (30 Hz .. 16 kHz), value 0..1 magnitude
        //                        (0 = -72 dBFS, 1 = 0 dBFS), one row per hop
        //   y spec_rows ..       WAVEFORM ring, x = time inside the row,
        //                        value = the SIGNED sample, -1..1 (0 = silence,
        //                        which is also what an unbound slot reads)
        // audio_dim  = (bins, spec_rows, spec_cursor, wave_cursor)
        // audio_meta = (tex_w, tex_h, wave_rows, hop_secs)
        // audio_env  = (bass, mid, high, rms), smoothed 0..1 — no texture
        //              read needed for a plain level.
        // Each `*_cursor` names the NEWEST row of its ring: that is the
        // unwrap key. Use the two helpers, never a raw uv — and remember a
        // silent rig reads 0 everywhere, so give every look an idle floor.
        audio_tex: texture_2d(float)
        audio_dim: uniform(vec4(256.0, 256.0, 0.0, 0.0))
        audio_meta: uniform(vec4(256.0, 320.0, 64.0, 0.0213))
        audio_env: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // Spectrum magnitude 0..1. `f` 0..1 = log frequency (0 = 30 Hz,
        // 1 = 16 kHz); `age` 0..1 = how far back (0 = now, 1 = the oldest
        // row kept, about 5.5 s). Silence reads 0.
        audio_fft: fn(f: float, age: float) -> float {
            let rows = max(self.audio_dim.y, 1.0)
            let x = (clamp(f, 0.0, 1.0) * (self.audio_dim.x - 1.0) + 0.5) / self.audio_meta.x
            let back = clamp(age, 0.0, 1.0) * (rows - 1.0)
            let row = modf(self.audio_dim.z - back + rows * 2.0, rows)
            return self.audio_tex.sample_nearest(vec2(x, (row + 0.5) / self.audio_meta.y), 0.0).x
        }

        // Waveform sample -1..1. `t` 0..1 across the stored window
        // (1 = the newest sample, about 1.4 s deep). Silence reads 0.
        audio_wave: fn(t: float) -> float {
            let bins = max(self.audio_dim.x, 1.0)
            let wrows = max(self.audio_meta.z, 1.0)
            let back = (1.0 - clamp(t, 0.0, 1.0)) * (bins * wrows - 1.0)
            let pos = (bins - 1.0) - back
            let ro = floor(pos / bins)
            let col = pos - ro * bins
            let row = modf(self.audio_dim.w + ro + wrows * 4.0, wrows)
            let y = (self.audio_dim.y + row + 0.5) / self.audio_meta.y
            let uv = vec2((col + 0.5) / self.audio_meta.x, y)
            return self.audio_tex.sample_nearest(uv, 0.0).x
        }
        backface_culling: false
        alpha_blend: false
        depth_write: false

        v_uv: varying(vec2f)

        // THE LOOK — the entire frame, doc-replaceable (CONTRACT.md).
        //   uv      = screen uv, (0,0) top-left
        //   content = input0 at `uv` (the channel video, or the runtime's
        //             animated fallback pattern when nothing is bound)
        //   cmix    = pre-gated content strength; 0 means the texel is the
        //             fallback, so a look that wants REAL video only can
        //             branch on it
        // The default is the plain frame — a document that overrides this
        // owns every pixel.
        fx_color: fn(uv: vec2, content: vec4, cmix: float) -> vec4 {
            return vec4(content.xyz, 1.0)
        }

        vertex: fn() {
            // Clip-space passthrough: no camera, no depth, always the
            // whole frame.
            self.v_uv = self.geom.geom_uv
            self.vertex_pos = vec4(
                self.geom.geom_pos.x,
                self.geom.geom_pos.y,
                0.5,
                1.0
            )
            return self.vertex_pos
        }

        pixel: fn() {
            let tx = self.tex0.sample_as_bgra(self.v_uv)
            return self.fx_color(self.v_uv, tx, self.fog.z)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // ---------------------------------------------------------------------
    // Post-chain quads.
    // ---------------------------------------------------------------------

    // Feedback/trails: previous output re-projected (zoom/rotate) under the
    // fresh scene. u_fb = (amount, zoom, rotate, dim).
    set_type_default() do #(DrawVjFxFeedback::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_scene: texture_2d(float)
        tex_prev: texture_2d(float)
        u_fb: uniform(vec4(0.85, 1.01, 0.0, 0.97))

        pixel: fn() {
            let uv = self.pos
            let c = uv - vec2(0.5, 0.5)
            let cs = cos(self.u_fb.z)
            let sn = sin(self.u_fb.z)
            let rot = vec2(c.x * cs - c.y * sn, c.x * sn + c.y * cs)
            let puv = rot / self.u_fb.y + vec2(0.5, 0.5)
            let scene = self.tex_scene.sample_as_bgra(uv)
            let inside = step(0.0, puv.x) * step(puv.x, 1.0) * step(0.0, puv.y) * step(puv.y, 1.0)
            let prev = self.tex_prev.sample_as_bgra(clamp(puv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
                * inside
            let trail = prev.xyz * self.u_fb.x * self.u_fb.w
            // Screen-blend the trail under the fresh frame: bright stays
            // bright, black stays black, no grey mud.
            let rgb = vec3(1.0, 1.0, 1.0)
                - (vec3(1.0, 1.0, 1.0) - scene.xyz) * (vec3(1.0, 1.0, 1.0) - trail)
            return vec4(rgb, 1.0)
        }
    }

    // Bright-pass + 13-tap downsample (first bloom stage). u_bright.x is the
    // threshold, .y the soft knee; a plain blur sets threshold 0.
    set_type_default() do #(DrawVjFxDown::script_shader(vm)){
        ..mod.draw.DrawQuad
        source_texture: texture_2d(float)
        u_bright: uniform(vec4(0.0, 0.35, 0.0, 0.0))

        sample_b: fn(uv: vec2) -> vec4 {
            let c = self.source_texture.sample_as_bgra(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
            let lum = dot(c.xyz, vec3(0.299, 0.587, 0.114))
            let gate = smoothstep(self.u_bright.x, self.u_bright.x + self.u_bright.y + 0.0001, lum)
            return vec4(c.xyz * gate, c.w)
        }

        pixel: fn() {
            let size = self.source_texture.size()
            let texel = vec2(1.0 / max(size.x, 1.0), 1.0 / max(size.y, 1.0))
            let uv = self.pos
            return self.sample_b(uv) * 0.25
                + (self.sample_b(uv + texel * vec2(-1.0, 1.0))
                    + self.sample_b(uv + texel * vec2(1.0, 1.0))
                    + self.sample_b(uv + texel * vec2(-1.0, -1.0))
                    + self.sample_b(uv + texel * vec2(1.0, -1.0))) * 0.125
                + (self.sample_b(uv + texel * vec2(0.0, 1.5))
                    + self.sample_b(uv + texel * vec2(-1.5, 0.0))
                    + self.sample_b(uv + texel * vec2(1.5, 0.0))
                    + self.sample_b(uv + texel * vec2(0.0, -1.5))) * 0.0625
        }
    }

    // Tent upsample, half-texel offsets (progressive re-home — never a raw
    // stretch of a deep mip; see the gauss stack).
    set_type_default() do #(DrawVjFxUp::script_shader(vm)){
        ..mod.draw.DrawQuad
        source_texture: texture_2d(float)

        sample_s: fn(uv: vec2) -> vec4 {
            return self.source_texture.sample_as_bgra(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
        }

        pixel: fn() {
            let size = self.source_texture.size()
            let texel = vec2(0.5 / max(size.x, 1.0), 0.5 / max(size.y, 1.0))
            let uv = self.pos
            return self.sample_s(uv) * 0.25
                + (self.sample_s(uv + texel * vec2(1.0, 0.0))
                    + self.sample_s(uv + texel * vec2(-1.0, 0.0))
                    + self.sample_s(uv + texel * vec2(0.0, 1.0))
                    + self.sample_s(uv + texel * vec2(0.0, -1.0))) * 0.125
                + (self.sample_s(uv + texel * vec2(1.0, 1.0))
                    + self.sample_s(uv + texel * vec2(-1.0, 1.0))
                    + self.sample_s(uv + texel * vec2(1.0, -1.0))
                    + self.sample_s(uv + texel * vec2(-1.0, -1.0))) * 0.0625
        }
    }

    // Bloom mix: base + blurred brights, screen-blended with gain.
    // u_mix = (strength, 0, 0, 0).
    set_type_default() do #(DrawVjFxBloomMix::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_base: texture_2d(float)
        tex_blur: texture_2d(float)
        u_mix: uniform(vec4(1.2, 0.0, 0.0, 0.0))

        pixel: fn() {
            let base = self.tex_base.sample_as_bgra(self.pos)
            let blur = self.tex_blur.sample_as_bgra(self.pos)
            let b = clamp(blur.xyz * self.u_mix.x, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
            let rgb = vec3(1.0, 1.0, 1.0)
                - (vec3(1.0, 1.0, 1.0) - base.xyz) * (vec3(1.0, 1.0, 1.0) - b)
            return vec4(rgb, 1.0)
        }
    }

    // Tiltshift mix: sharp + pyramid-blurred, mixed by a screen-Y focus
    // ramp. u_tilt = (focus 0..1, width, 0, 0).
    set_type_default() do #(DrawVjFxTiltMix::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_base: texture_2d(float)
        tex_blur: texture_2d(float)
        u_tilt: uniform(vec4(0.55, 0.25, 0.0, 0.0))

        pixel: fn() {
            let base = self.tex_base.sample_as_bgra(self.pos)
            let blur = self.tex_blur.sample_as_bgra(self.pos)
            let band = abs(self.pos.y - self.u_tilt.x)
            let t = smoothstep(self.u_tilt.y * 0.5, self.u_tilt.y, band)
            return vec4(mix(base.xyz, blur.xyz, t), 1.0)
        }
    }

    // Fullscreen warp pass: the screen-space effect family, beat-aware.
    // u_warp = (mode, p1, p2, time); u_beat = (beat, phase, pulse, aspect).
    // Modes: 0 kaleido, 1 mirror, 2 chroma, 3 pixelate, 4 swirl, 5 ripple,
    // 6 glitch, 7 posterize, 8 radial blur, 9 uv tunnel.
    set_type_default() do #(DrawVjFxWarp::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_in: texture_2d(float)
        u_warp: uniform(vec4(0.0, 0.5, 0.5, 0.0))
        u_beat: uniform(vec4(0.0, 0.0, 0.0, 1.7777))

        src: fn(uv: vec2) -> vec4 {
            return self.tex_in.sample_as_bgra(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
        }

        pixel: fn() {
            let mode = self.u_warp.x
            let p1 = self.u_warp.y
            let p2 = self.u_warp.z
            let time = self.u_warp.w
            let tau = 6.2831853
            let uv = self.pos
            let ca = self.u_beat.w
            let c = vec2((uv.x - 0.5) * ca, uv.y - 0.5)
            let back = vec2(1.0 / ca, 1.0)
            if mode < 0.5 {
                // KALEIDO: p1 segments (2..12), p2 spin speed.
                let segs = floor(mix(2.0, 12.0, clamp(p1, 0.0, 1.0)) + 0.5)
                let ang = atan2(c.y, c.x) + time * p2 * 2.0
                let slice = tau / max(segs, 2.0)
                let folded = abs(modf(ang + slice * 100.0, slice) - slice * 0.5)
                let r = length(c)
                let w = vec2(cos(folded), sin(folded)) * r
                return self.src(w * back + vec2(0.5, 0.5))
            }
            if mode < 1.5 {
                // MIRROR: p1 >= 0.5 folds x, else y; p2 blends.
                let axis = step(0.5, p1)
                let mu = vec2(abs(c.x), c.y) * axis + vec2(c.x, abs(c.y)) * (1.0 - axis)
                return self.src(uv).mix(self.src(mu * back + vec2(0.5, 0.5)), clamp(p2, 0.0, 1.0))
            }
            if mode < 2.5 {
                // CHROMA: rgb split, p1 amount, p2 angle.
                let a = p2 * tau
                let off = vec2(cos(a), sin(a)) * p1 * 0.05
                return vec4(
                    self.src(uv + off).x,
                    self.src(uv).y,
                    self.src(uv - off).z,
                    1.0
                )
            }
            if mode < 3.5 {
                // PIXELATE: p1 block size (log), p2 beat snap.
                let rows = exp(mix(log(160.0), log(6.0), clamp(p1, 0.0, 1.0)))
                let stepped = pow(2.0, floor(log(rows) / log(2.0) + 0.5))
                let ry = max(floor(mix(rows, stepped, p2) + 0.5), 2.0)
                let cells = vec2(max(floor(ry * ca + 0.5), 2.0), ry)
                return self.src((floor(uv * cells) + vec2(0.5, 0.5)) / cells)
            }
            if mode < 4.5 {
                // SWIRL: p1 twist, p2 radius.
                let r = length(c)
                let fall = 1.0 - smoothstep(0.0, mix(0.2, 1.1, p2), r)
                let a = p1 * 6.0 * fall
                let cs = cos(a)
                let sn = sin(a)
                let w = vec2(c.x * cs - c.y * sn, c.x * sn + c.y * cs)
                return self.src(w * back + vec2(0.5, 0.5))
            }
            if mode < 5.5 {
                // RIPPLE: p1 amplitude, p2 frequency; rides the beat pulse.
                let r = length(c)
                let wave = sin(r * mix(6.0, 40.0, p2) - time * 6.0)
                let dir = c / max(r, 0.0001)
                let amp = p1 * (0.5 + self.u_beat.z)
                return self.src(uv + dir * back * wave * amp * 0.08)
            }
            if mode < 6.5 {
                // GLITCH: p1 slice count, p2 jitter.
                let slices = mix(4.0, 28.0, p1)
                let row = floor(uv.y * slices)
                let n = fract(sin(row * 12.9898 + floor(self.u_beat.x * 4.0) * 7.1) * 43758.5453)
                return self.src(vec2(uv.x + (n - 0.5) * p2 * 0.35, uv.y))
            }
            if mode < 7.5 {
                // POSTERIZE: p1 levels, p2 saturation.
                let s = self.src(uv)
                let levels = mix(2.0, 10.0, p1)
                let q = floor(s.xyz * levels + vec3(0.5, 0.5, 0.5)) / levels
                let grey = vec3(dot(q, vec3(0.299, 0.587, 0.114)))
                return vec4(grey.mix(q, clamp(p2 * 2.0, 0.0, 2.0)), 1.0)
            }
            if mode < 8.5 {
                // RADIAL BLUR: 8 taps toward the center, p1 strength.
                let pull = p1 * 0.22 * (0.4 + self.u_beat.z)
                let mut acc = self.src(uv)
                let mut k = 1.0
                let mut i = 1.0
                loop {
                    if i > 7.5 {
                        break
                    }
                    let f = 1.0 - pull * (i / 8.0)
                    acc = acc + self.src(c * f * back + vec2(0.5, 0.5))
                    k = k + 1.0
                    i = i + 1.0
                }
                return acc / k
            }
            // UV TUNNEL: p1 depth speed, p2 spin.
            let r = length(c)
            let ang = atan2(c.y, c.x) / tau + time * p2 * 0.3
            let z = 0.25 / max(r, 0.02) + time * p1
            return self.src(vec2(fract(ang) * 2.0, fract(z)))
        }
    }

    // HOLD: the frame-latch stage. `tex_live` is this frame, `tex_held` the
    // frame the stage last grabbed (post.rs decides WHEN to grab — on a beat
    // slot or a rising trigger — and the grab pass runs this same shader
    // with mix 0, which is a straight copy).
    //
    // u_hold = (mix 0..1, bands, stagger 0..1, axis 0 = rows / 1 = columns)
    // u_hbeat = (beat, beat phase 0..1, pulse, time)
    //
    // ONE band (the default) is a plain freeze. MANY bands index the hold by
    // position: every band starts the beat holding and releases to live when
    // the BEAT PHASE passes its index — a scanline-indexed delay that costs
    // one stored frame, and is bounded by construction (phase is a 0..1 saw,
    // the index a function of the band number, nothing accumulates).
    set_type_default() do #(DrawVjFxHold::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_live: texture_2d(float)
        tex_held: texture_2d(float)
        u_hold: uniform(vec4(1.0, 1.0, 0.0, 0.0))
        u_hbeat: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        pixel: fn() {
            let uv = self.pos
            let live = self.tex_live.sample_as_bgra(uv)
            let amount = clamp(self.u_hold.x, 0.0, 1.0)
            // The GRAB pass runs with mix 0 and BOTH slots bound to the live
            // frame; returning early here also keeps it from ever sampling
            // the texture it is rendering into.
            if amount < 0.001 {
                return vec4(live.xyz, 1.0)
            }
            let held = self.tex_held.sample_as_bgra(uv)
            let bands = max(floor(clamp(self.u_hold.y, 1.0, 256.0) + 0.5), 1.0)
            let mut w = amount
            if bands > 1.5 {
                let coord = mix(uv.y, uv.x, step(0.5, self.u_hold.w))
                let band = floor(clamp(coord, 0.0, 0.9999) * bands)
                // Release order: down the frame, or hashed per band as
                // `stagger` opens (strips let go in scrambled order).
                let n = fract(sin(band * 12.9898 + 4.1) * 43758.5453)
                let idx = mix((band + 0.5) / bands, n, clamp(self.u_hold.z, 0.0, 1.0))
                w = amount * step(fract(self.u_hbeat.y), idx)
            }
            return vec4(mix(live.xyz, held.xyz, w), 1.0)
        }
    }

    // Final present: the chain's output onto the widget rect.
    set_type_default() do #(DrawVjFxPresent::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_out: texture_2d(float)

        pixel: fn() {
            return self.tex_out.sample_as_bgra(self.pos)
        }
    }
}

// ---------------------------------------------------------------------------
// Rust structs. Mesh family: every #[live] field AFTER #[deref] draw_vars is
// an INSTANCE field (vec4/f32 only). One instance = one engine draw.
// ---------------------------------------------------------------------------

macro_rules! fx_mesh_draw_struct {
    ($name:ident) => {
        #[derive(Script, ScriptHook)]
        #[repr(C)]
        pub struct $name {
            #[deref]
            pub draw_vars: DrawVars,
            /// (time, beat position, beat phase 0..1, eased pulse).
            #[live(vec4(0.0, 0.0, 0.0, 0.0))]
            pub time_beat: Vec4f,
            /// (bar phase 0..1, bpm, audio energy 0..1, dt).
            #[live(vec4(0.0, 120.0, 0.0, 0.0))]
            pub sig: Vec4f,
            /// Document-bound user parameters p0..p3.
            #[live(vec4(0.0, 0.0, 0.0, 0.0))]
            pub user: Vec4f,
            /// (sway, sway_freq, growth 0..1, twist).
            #[live(vec4(0.0, 1.0, 1.0, 0.0))]
            pub anim: Vec4f,
            /// Engine-specific (see engines.rs).
            #[live(vec4(0.0, 0.0, 0.0, 0.0))]
            pub shape: Vec4f,
            #[live(vec4(0.0, 0.0, 0.0, 0.0))]
            pub flow: Vec4f,
            #[live(vec4(0.28, 0.94, 1.0, 1.0))]
            pub col_a: Vec4f,
            #[live(vec4(1.0, 0.25, 0.63, 1.0))]
            pub col_b: Vec4f,
            #[live(vec4(1.0, 1.0, 1.0, 1.0))]
            pub col_c: Vec4f,
            #[live(vec4(0.01, 0.012, 0.03, 1.0))]
            pub col_bg: Vec4f,
            /// (fog density, emissive gain, tex mix, unused).
            #[live(vec4(0.05, 1.0, 0.0, 0.0))]
            pub fog: Vec4f,
        }
    };
}

fx_mesh_draw_struct!(DrawVjFxMesh);
fx_mesh_draw_struct!(DrawVjFxTerrain);
fx_mesh_draw_struct!(DrawVjFxRibbon);
fx_mesh_draw_struct!(DrawVjFxTunnel);
fx_mesh_draw_struct!(DrawVjFxParticles);
// The shared content backdrop (above): standard signal block, and it
// reads only `fog.z` (pre-gated content strength) x `fog.w` (the family
// dim) plus `col_bg`.
fx_mesh_draw_struct!(DrawVjFxBackdrop);
// The fullscreen `screen` family's own pass (above): one clip-space quad,
// the whole fragment behind `fx_color`.
fx_mesh_draw_struct!(DrawVjFxScreen);

/// The scriptable-emitter shader: shared frame state + per-EMITTER instance
/// data (one `add_instance` per live emitter per frame — the whole upload).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxEmitter {
    #[deref]
    pub draw_vars: DrawVars,
    /// (time, beat position, beat phase 0..1, eased pulse).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub time_beat: Vec4f,
    /// (bar phase 0..1, bpm, audio energy 0..1, dt).
    #[live(vec4(0.0, 120.0, 0.0, 0.0))]
    pub sig: Vec4f,
    /// (x, y, z, age).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub e_origin: Vec4f,
    /// (vx, vy, vz, life).
    #[live(vec4(0.0, 0.0, 0.0, 2.0))]
    pub e_vel: Vec4f,
    /// (speed, seed, size, style).
    #[live(vec4(4.0, 0.0, 0.1, 0.0))]
    pub e_par: Vec4f,
    /// (fraction, gravity, stagger, spread).
    #[live(vec4(1.0, 1.0, 0.0, 1.0))]
    pub e_par2: Vec4f,
    #[live(vec4(1.0, 0.8, 0.4, 1.0))]
    pub e_col_a: Vec4f,
    #[live(vec4(1.0, 0.3, 0.1, 1.0))]
    pub e_col_b: Vec4f,
    /// (fog density, emissive gain, CONTENT MIX, unused) — the standard
    /// slot; emitters read only .z (the pre-gated content strength).
    #[live(vec4(0.05, 1.0, 0.0, 0.0))]
    pub fog: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxFeedback {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxDown {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxUp {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxBloomMix {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxPresent {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxTiltMix {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxWarp {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxHold {
    #[deref]
    pub draw_super: DrawQuad,
}
