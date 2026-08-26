//! Lane B. Every shader the viewport owns, and the widget registration.
//!
//! Three of the four passes are here; the fourth (the lit pass) is
//! `libs/render` and has no shader of ours at all.
//!
//! | shader | pass | what it is |
//! |---|---|---|
//! | `DrawFabAux` | aux (RGBA32F + D32) | the CAD G-buffer: view depth, octahedral view normal, signed element id. Also the depth prepass the lit pass reuses, and the authority on visibility / explode / section clipping. |
//! | `DrawFabComposite` | composite | every shading mode, cavity, SSAO, selection outline, hover, x-ray, section caps, background gradient — one full-screen quad over `lit` + `aux`. |
//! | `DrawFabGrid` | composite | the infinite adaptive ground grid + axis lines, occluded against `aux` rather than a depth buffer so it survives wireframe and ink modes. |
//! | `DrawFabLine` | composite | the scene's authored contour edges as screen-space-wide quads; hidden segments dashed by comparing against `aux` depth. |
//!
//! The aux buffer's channels, once, so the composite and the line shader
//! cannot drift apart:
//!
//! ```text
//! r = view-space distance in meters (0 = nothing drawn here)
//! g = octahedral normal x   (view space, folded toward the camera)
//! b = octahedral normal y
//! a = signed element id: +(id+1) on a front face, -(id+1) on a back face,
//!     0 = background. The sign is what makes a section cap findable: with a
//!     plane active the only back faces left on screen are the inside of a
//!     cut solid.
//! ```

use super::*;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.fab_geom = {
        FabVertex: #(fab_vertex_pod(vm))
        FabLineVertex: #(fab_line_vertex_pod(vm))
        FabGeom: #(fab_placeholder_geom(vm))
        FabLineGeom: #(fab_placeholder_line_geom(vm))
    }

    // =====================================================================
    // AUX — the CAD G-buffer
    // =====================================================================
    mod.draw.DrawFabAux = mod.std.set_type_default() do #(DrawFabAux::script_shader(vm)){
        alpha_blend: false
        depth_write: true
        // Back faces stay: with a section plane on, the inside of a cut
        // solid IS the cap, and it is the only thing left to draw there.
        backface_culling: false
        color_format: @Rgba32F
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(mod.fab_geom.FabVertex, mod.fab_geom.FabGeom)
        elem_map: texture_2d(float)
        // x = lut width, y = lut height, z unused, w = enabled
        elem_ctl: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // x = plane count (0..6); planes keep a*x+b*y+c*z+d >= 0, Fab space
        clip_ctl: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        v_view: varying(vec3f)
        v_vnormal: varying(vec3f)
        v_world: varying(vec3f)
        v_elem: varying(float)

        oct_encode: fn(n: vec3f) -> vec2f {
            let l1 = abs(n.x) + abs(n.y) + abs(n.z)
            if l1 < 0.00000001 {
                return vec2(0.0, 0.0)
            }
            let e = n / l1
            if e.z >= 0.0 {
                return vec2(e.x, e.y)
            }
            let sx = step(0.0, e.x) * 2.0 - 1.0
            let sy = step(0.0, e.y) * 2.0 - 1.0
            return vec2((1.0 - abs(e.y)) * sx, (1.0 - abs(e.x)) * sy)
        }

        vertex: fn() {
            var pos = self.geom.pos
            if self.elem_ctl.w > 0.5 {
                let ew = max(self.elem_ctl.x, 1.0)
                let eh = max(self.elem_ctl.y, 1.0)
                let ex = (modf(self.geom.element, ew) + 0.5) / ew
                let ey = (floor(self.geom.element / ew) + 0.5) / eh
                let f = self.elem_map.sample_nearest(vec2(ex, ey))
                if f.x < 0.5 {
                    // Zero-area triangle: a hidden element costs no fragments.
                    pos = vec3(0.0, 0.0, 0.0)
                } else {
                    // The lookup carries the offset in the LIT model's space
                    // (Y up); this pass draws the Fab stream (Z up).
                    pos = pos + vec3(f.y, 0.0 - f.w, f.z)
                }
            }
            self.v_world = pos
            self.v_elem = self.geom.element
            let view4 = self.draw_pass.camera_view * vec4(pos.x, pos.y, pos.z, 1.0)
            self.v_view = view4.xyz
            self.v_vnormal = (self.draw_pass.camera_view
                * vec4(self.geom.normal.x, self.geom.normal.y, self.geom.normal.z, 0.0)).xyz
            // Render-only part priority from the scene's measured coplanar
            // groups. One rank is slightly larger than a D32 ULP; the clamp
            // bounds the effect even for pathological conflict chains.
            let priority = min(max(self.geom.pad, 0.0), 4095.0)
            let bias = min(priority * 0.0000001, 0.00005)
            let clip = self.draw_pass.camera_projection * view4
            self.vertex_pos = vec4(clip.x, clip.y, clip.z - bias * clip.w, clip.w)
        }

        clipped: fn(w: vec3f) -> bool {
            if self.clip_ctl.x < 0.5 {
                return false
            }
            if dot(self.clip0.xyz, w) + self.clip0.w < 0.0 { return true }
            if self.clip_ctl.x > 1.5 {
                if dot(self.clip1.xyz, w) + self.clip1.w < 0.0 { return true }
            }
            if self.clip_ctl.x > 2.5 {
                if dot(self.clip2.xyz, w) + self.clip2.w < 0.0 { return true }
            }
            if self.clip_ctl.x > 3.5 {
                if dot(self.clip3.xyz, w) + self.clip3.w < 0.0 { return true }
            }
            if self.clip_ctl.x > 4.5 {
                if dot(self.clip4.xyz, w) + self.clip4.w < 0.0 { return true }
            }
            if self.clip_ctl.x > 5.5 {
                if dot(self.clip5.xyz, w) + self.clip5.w < 0.0 { return true }
            }
            return false
        }

        pixel: fn() -> vec4f {
            if self.clipped(self.v_world) {
                discard()
            }
            let depth = length(self.v_view)
            var n = normalize(self.v_vnormal)
            // Facing: in view space the camera sits at the origin, so a
            // front face has its normal pointing back along the eye ray.
            let toward = dot(n, normalize(self.v_view))
            var sign_id = 1.0
            if toward > 0.0 {
                sign_id = 0.0 - 1.0
                n = 0.0 - n
            }
            let e = self.oct_encode(n)
            return vec4(depth, e.x, e.y, (self.v_elem + 1.0) * sign_id)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // =====================================================================
    // COMPOSITE — every shading mode, in one quad
    // =====================================================================
    mod.draw.DrawFabComposite = mod.std.set_type_default() do #(DrawFabComposite::script_shader(vm)){
        ..mod.draw.DrawQuad
        lit: texture_2d(float)
        aux: texture_2d(float)
        // x = shading (0 wire, 1 solid, 2 material, 3 realtime,
        //              4 rendered fallback, 5 ink)
        // y = x-ray, z = section active, w = wire-on-shaded
        u_mode: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        // x = outlines (0/1), y = cavity strength (Solid default 0.25),
        // z = ssao strength (0 in Solid), w = unused / probe
        u_flags: uniform(vec4(1.0, 0.25, 0.0, 0.0))
        // x = hovered element + 1 (0 = none), y = model radius, zw unused
        u_hover: uniform(vec4(0.0, 10.0, 0.0, 0.0))
        // xy = 1/pixel size, zw = pixel size
        u_texel: uniform(vec4(0.001, 0.001, 1000.0, 1000.0))
        // x = ambient-occlusion strength (how dark full occlusion goes).
        // The occlusion itself — world-metre radius, coincident-plane bias,
        // blur — is computed by the SsaoPass chain (libs/render ssao.rs)
        // and arrives here as `ssao_tex`.
        u_ao: uniform(vec4(0.45, 0.0, 0.0, 0.0))
        u_bg_top: uniform(vec4(0.247, 0.247, 0.247, 1.0))
        u_bg_bottom: uniform(vec4(0.169, 0.169, 0.169, 1.0))
        u_select: uniform(vec4(0.914, 0.416, 0.169, 1.0))
        u_select_dim: uniform(vec4(0.647, 0.275, 0.114, 1.0))
        u_hover_col: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        u_cap: uniform(vec4(0.541, 0.541, 0.541, 1.0))
        u_ink: uniform(vec4(0.102, 0.102, 0.102, 1.0))
        u_paper: uniform(vec4(0.957, 0.949, 0.933, 1.0))
        elem_map: texture_2d(float)
        elem_ctl: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // The SsaoPass output (x = occlusion, 1 = unoccluded), same pass
        // rect as `lit`/`aux`. Only sampled while u_flags.z > 0, which the
        // host raises only on frames the chain actually recorded.
        ssao_tex: texture_2d(float)
        // Nothing to clip against: this quad IS the pass.
        depth_clip: 0.0

        // STRAIGHT TO CLIP SPACE, and it is load-bearing.
        //
        // `DrawQuad::vertex` ends in `draw_pass.camera_projection *
        // (draw_pass.camera_view * world)` — which in a normal 2D pass is the
        // ORTHO pair makepad installs in `set_ortho_matrix`. This pass is not
        // a normal 2D pass: it carries `keep_camera_matrix = true` and
        // `set_pass_camera` writes the 3D PERSPECTIVE pair into exactly those
        // uniforms, because the grid and the contour lines drawn after this
        // quad are real 3D geometry and read them. Left on the stock path the
        // composite is projected into the world as a flat billboard somewhere
        // out in the scene, the pass keeps its black clear colour, and the
        // realtime pane shows nothing but the grid — the whole lit image
        // never reaches the screen.
        //
        // The composite always covers the entire pass, so it needs no matrix
        // at all: `geom.pos` (0..1, y down) maps to NDC directly, and `pos`
        // stays the 0..1 uv the pixel stage samples `lit` / `aux` with.
        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(
                self.geom.pos.x * 2.0 - 1.0
                1.0 - self.geom.pos.y * 2.0
                0.0
                1.0
            )
        }

        oct_decode: fn(e: vec2f) -> vec3f {
            let nz = 1.0 - abs(e.x) - abs(e.y)
            let t = max(0.0 - nz, 0.0)
            let sx = step(0.0, e.x) * 2.0 - 1.0
            let sy = step(0.0, e.y) * 2.0 - 1.0
            return normalize(vec3(e.x - t * sx, e.y - t * sy, nz))
        }

        // Selection state of an element id, straight out of the same
        // lookup the lit pass reads: 0 hidden, 1 visible, 2 selected, 3 active.
        elem_state: fn(id: float) -> float {
            if self.elem_ctl.w < 0.5 {
                return 1.0
            }
            let ew = max(self.elem_ctl.x, 1.0)
            let eh = max(self.elem_ctl.y, 1.0)
            let ex = (modf(id, ew) + 0.5) / ew
            let ey = (floor(id / ew) + 0.5) / eh
            return self.elem_map.sample_nearest(vec2(ex, ey)).x
        }

        pixel: fn() -> vec4f {
            let uv = self.pos
            if self.u_flags.w > 0.5 {
                // FAB_SHOW=probe: paint the view index, nothing else.
                return vec4(self.u_hover.z, 1.0 - self.u_hover.z, 0.5, 1.0)
            }
            let g = self.aux.sample_nearest(uv)
            let ink = self.u_mode.x > 4.5
            let wire = self.u_mode.x < 0.5
            // Background: Fab's vertical viewport gradient, or ink's paper.
            var bg = self.u_bg_top.xyz.mix(self.u_bg_bottom.xyz, uv.y)
            if ink {
                bg = self.u_paper.xyz
            }
            // Realtime and the Rendered fallback show the renderer's sky;
            // CAD modes keep Fab's flat viewport gradient.
            if self.u_mode.x > 2.5 && self.u_mode.x < 4.5 {
                bg = self.lit.sample_as_bgra(uv).xyz
            }
            if g.w == 0.0 {
                return vec4(bg, 1.0)
            }

            let depth = g.x
            let n = self.oct_decode(g.yz)
            let front = g.w > 0.0
            let id = abs(g.w) - 1.0
            let state = self.elem_state(id)

            // ---- section cap ------------------------------------------
            // With a plane on, a surviving back face is the inside of a cut
            // solid: paint it flat so the building reads as a solid model
            // rather than a hollow shell.
            let is_cap = (self.u_mode.z > 0.5) && (front == false)

            // ---- neighbours (one tap ring, shared by every effect) -----
            let tx = self.u_texel.x
            let ty = self.u_texel.y
            let gl = self.aux.sample_nearest(uv + vec2(0.0 - tx, 0.0))
            let gr = self.aux.sample_nearest(uv + vec2(tx, 0.0))
            let gu = self.aux.sample_nearest(uv + vec2(0.0, 0.0 - ty))
            let gd = self.aux.sample_nearest(uv + vec2(0.0, ty))

            // ---- cavity (full-res curvature, Fab ~0.25) ------------
            // A 1-pixel ring on a tessellated wall reads every triangle
            // edge as a crease and prints hatching. A few-pixel ring at
            // full resolution, gated by depth, keeps only architectural
            // concave/convex detail. u_flags.y is the strength (0.25 in
            // Solid, 1.0 elsewhere, 0 when the overlay is off).
            var cavity = 1.0
            if self.u_flags.y > 0.001 && !ink && !wire {
                let stepx = tx * 3.0
                let stepy = ty * 3.0
                let cl = self.aux.sample_nearest(uv + vec2(0.0 - stepx, 0.0))
                let cr = self.aux.sample_nearest(uv + vec2(stepx, 0.0))
                let cu = self.aux.sample_nearest(uv + vec2(0.0, 0.0 - stepy))
                let cd = self.aux.sample_nearest(uv + vec2(0.0, stepy))
                let nl = self.oct_decode(cl.yz)
                let nr = self.oct_decode(cr.yz)
                let nu = self.oct_decode(cu.yz)
                let ndn = self.oct_decode(cd.yz)
                var curve = 0.0
                curve = curve + (dot(n, nl) - 1.0)
                curve = curve + (dot(n, nr) - 1.0)
                curve = curve + (dot(n, nu) - 1.0)
                curve = curve + (dot(n, ndn) - 1.0)
                let same = step(abs(cl.x - depth), depth * 0.03)
                    * step(abs(cr.x - depth), depth * 0.03)
                    * step(abs(cu.x - depth), depth * 0.03)
                    * step(abs(cd.x - depth), depth * 0.03)
                let valley = clamp(0.0 - curve * 1.4, 0.0, 1.0) * same
                cavity = 1.0 - valley * self.u_flags.y
            }

            // ---- ambient occlusion (the SsaoPass output) ---------------
            // Computed by the dedicated chain between aux and lit: a dozen
            // hemisphere taps at a WORLD-metre radius, a coincident-plane
            // bias so millimetre construction layers never shade each
            // other, a stable pixel-hash rotation, and a depth-aware blur.
            // This quad only reads the result and applies the strength.
            // Off in Solid (u_flags.z = 0) so clay stays clay; Realtime
            // consumes the same texture inside the renderer's ambient term
            // instead, never here.
            var ao = 1.0
            if self.u_flags.z > 0.001 && !ink && !wire {
                let s = self.ssao_tex.sample_nearest(uv).x
                ao = 1.0 - (1.0 - s) * self.u_ao.x
            }

            // ---- the lit image ----------------------------------------
            // Solid mode is Fab's: one neutral clay under a studio
            // light. The lit pass already carries the material tint, so
            // Solid pulls the saturation back out rather than running a
            // second shading pipeline.
            var col = self.lit.sample_as_bgra(uv).xyz
            if self.u_mode.x < 1.5 && self.u_mode.x > 0.5 {
                let luma = dot(col, vec3(0.2126, 0.7152, 0.0722))
                col = col.mix(vec3(luma, luma, luma), 0.85)
            }
            if is_cap {
                // Flat cut face, lit only by its own facing so the cut plane
                // reads as one clean surface.
                let key = clamp(0.55 + 0.45 * n.z, 0.0, 1.0)
                col = self.u_cap.xyz * key
            }
            col = col * cavity * ao

            if wire || ink {
                col = bg
            }

            // ---- silhouette / crease edges ----------------------------
            // Solid never composites these (ink / wire-on-shaded only).
            // Silhouette = id or background break. Crease = a true fold
            // (> 60°, cos = 0.5) on the same surface — a depth jump was
            // picking up z-fight and every triangle edge as hatching.
            let idl = abs(gl.w) - 1.0
            let idr = abs(gr.w) - 1.0
            let idu = abs(gu.w) - 1.0
            let idd = abs(gd.w) - 1.0
            var edge = 0.0
            edge = max(edge, step(0.5, abs(idl - id)) * step(0.5, abs(gl.w)))
            edge = max(edge, step(0.5, abs(idr - id)) * step(0.5, abs(gr.w)))
            edge = max(edge, step(0.5, abs(idu - id)) * step(0.5, abs(gu.w)))
            edge = max(edge, step(0.5, abs(idd - id)) * step(0.5, abs(gd.w)))
            // Silhouette against the background counts too.
            edge = max(edge, step(0.5, 1.0 - step(0.5, abs(gl.w))))
            edge = max(edge, step(0.5, 1.0 - step(0.5, abs(gr.w))))
            edge = max(edge, step(0.5, 1.0 - step(0.5, abs(gu.w))))
            edge = max(edge, step(0.5, 1.0 - step(0.5, abs(gd.w))))
            var crease = 0.0
            if ink || self.u_mode.w > 0.5 {
                let nl = self.oct_decode(gl.yz)
                let nr = self.oct_decode(gr.yz)
                let nu = self.oct_decode(gu.yz)
                let ndn = self.oct_decode(gd.yz)
                let sl = step(abs(gl.x - depth), depth * 0.05) * step(0.5, abs(gl.w))
                let sr = step(abs(gr.x - depth), depth * 0.05) * step(0.5, abs(gr.w))
                let su = step(abs(gu.x - depth), depth * 0.05) * step(0.5, abs(gu.w))
                let sd = step(abs(gd.x - depth), depth * 0.05) * step(0.5, abs(gd.w))
                crease = max(crease, step(dot(n, nl), 0.5) * sl)
                crease = max(crease, step(dot(n, nr), 0.5) * sr)
                crease = max(crease, step(dot(n, nu), 0.5) * su)
                crease = max(crease, step(dot(n, ndn), 0.5) * sd)
            }

            if ink {
                let line = max(edge, crease)
                col = col.mix(self.u_ink.xyz, line)
            }

            // ---- x-ray ------------------------------------------------
            var alpha = 1.0
            if self.u_mode.y > 0.5 && !ink {
                col = bg.mix(col, 0.38)
                col = col + vec3(edge, edge, edge) * 0.10
            }

            // ---- wire-on-shaded ---------------------------------------
            if self.u_mode.w > 0.5 && !wire && !ink {
                col = col.mix(vec3(0.0, 0.0, 0.0), max(edge, crease) * 0.55)
            }

            // ---- hover + selection outline ----------------------------
            if self.u_flags.x > 0.5 {
                // The outline is an OUTER ring: a neighbour that belongs to a
                // selected element while this pixel does not.
                var near_sel = 0.0
                var near_act = 0.0
                let sl = self.elem_state(idl)
                let sr = self.elem_state(idr)
                let su = self.elem_state(idu)
                let sd = self.elem_state(idd)
                near_sel = max(near_sel, step(1.5, sl) * step(0.5, abs(gl.w)))
                near_sel = max(near_sel, step(1.5, sr) * step(0.5, abs(gr.w)))
                near_sel = max(near_sel, step(1.5, su) * step(0.5, abs(gu.w)))
                near_sel = max(near_sel, step(1.5, sd) * step(0.5, abs(gd.w)))
                near_act = max(near_act, step(2.5, sl) * step(0.5, abs(gl.w)))
                near_act = max(near_act, step(2.5, sr) * step(0.5, abs(gr.w)))
                near_act = max(near_act, step(2.5, su) * step(0.5, abs(gu.w)))
                near_act = max(near_act, step(2.5, sd) * step(0.5, abs(gd.w)))
                let outer = step(state, 1.5)
                let ring_sel = near_sel * outer * edge
                let ring_act = near_act * outer * edge
                col = col.mix(self.u_select_dim.xyz, ring_sel)
                col = col.mix(self.u_select.xyz, ring_act)
                // The selected element itself keeps a warm wash so a
                // multi-selection is readable without counting outlines.
                if state > 1.5 {
                    col = col.mix(self.u_select.xyz, 0.18)
                }
            }
            if self.u_hover.x > 0.5 {
                if abs(id + 1.0 - self.u_hover.x) < 0.5 {
                    col = col.mix(self.u_hover_col.xyz, 0.12)
                }
            }
            return vec4(col, alpha)
        }
    }

    // =====================================================================
    // GRID — infinite, adaptive, axis-coloured, occluded against `aux`
    // =====================================================================
    mod.draw.DrawFabGrid = mod.std.set_type_default() do #(DrawFabGrid::script_shader(vm)){
        alpha_blend: true
        depth_write: false
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(mod.fab_geom.FabVertex, mod.fab_geom.FabGeom)
        aux: texture_2d(float)
        u_grid: uniform(vec4(0.313, 0.313, 0.313, 1.0))
        u_grid_major: uniform(vec4(0.353, 0.353, 0.353, 1.0))
        u_axis_x: uniform(vec4(1.0, 0.2, 0.32, 1.0))
        u_axis_y: uniform(vec4(0.545, 0.863, 0.0, 1.0))
        // x = axes on, y = grid on, zw unused
        u_grid_flags: uniform(vec4(1.0, 1.0, 0.0, 0.0))
        v_world: varying(vec3f)
        v_view: varying(vec3f)
        v_clip: varying(vec4f)

        camera_world_pos: fn() -> vec3f {
            let c = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
            let w = max(c.w, 0.00001)
            return vec3(c.x / w, c.y / w, c.z / w)
        }

        vertex: fn() {
            let world = self.transform * vec4(self.geom.pos.x, self.geom.pos.y, self.geom.pos.z, 1.0)
            self.v_world = world.xyz
            let view4 = self.draw_pass.camera_view * world
            self.v_view = view4.xyz
            let clip = self.draw_pass.camera_projection * view4
            self.v_clip = clip
            self.vertex_pos = clip
        }

        grid_line: fn(p: vec2f, spacing: float, fw: vec2f) -> float {
            let g = abs(fract(p / spacing - 0.5) - 0.5) * spacing / max(fw, vec2(0.000001, 0.000001))
            return 1.0 - min(min(g.x, g.y), 1.0)
        }

        pixel: fn() -> vec4f {
            // Occlusion against the CAD G-buffer: this pass owns no depth
            // buffer, so a grid line behind the building is rejected by
            // comparing view distances.
            let ndc = self.v_clip.xy / max(self.v_clip.w, 0.000001)
            let uv = vec2(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5)
            let g = self.aux.sample_nearest(uv)
            let my = length(self.v_view)
            if g.w != 0.0 {
                // The bias ADDS here, where the contour lines' subtracts: a
                // ground slab sitting exactly on z = 0 is coplanar with the
                // grid, and the model has to win that tie. Subtracting let
                // the grid draw straight through every floor plate.
                if g.x < my + (my * 0.0015 + 0.01) {
                    discard()
                }
            }
            let cam = self.camera_world_pos()
            let p = self.v_world.xy
            let dist = length(cam - self.v_world)
            let fw = abs(dFdx(p)) + abs(dFdy(p))
            // One decade per ~10 screen-relative minor lines, cross-faded so
            // zooming never pops a whole grid in or out.
            let l10 = log(max(dist * 0.12, 0.0001)) / log(10.0)
            let lvl = floor(l10)
            let t = l10 - lvl
            let minor = pow(10.0, lvl)
            let major = minor * 10.0
            let a_minor = self.grid_line(p, minor, fw) * (1.0 - t) * 0.55 * self.u_grid_flags.y
            let a_major = self.grid_line(p, major, fw) * 0.85 * self.u_grid_flags.y
            let ax = (1.0 - min(abs(p.y) / max(fw.y * 1.6, 0.000001), 1.0)) * self.u_grid_flags.x
            let ay = (1.0 - min(abs(p.x) / max(fw.x * 1.6, 0.000001), 1.0)) * self.u_grid_flags.x
            let fade = 1.0 - smoothstep(self.fade_far * 0.35, self.fade_far, dist)
            var col = self.u_grid.xyz
            var alpha = a_minor
            col = col.mix(self.u_grid_major.xyz, a_major)
            alpha = max(alpha, a_major)
            col = col.mix(self.u_axis_y.xyz, ay)
            alpha = max(alpha, ay)
            col = col.mix(self.u_axis_x.xyz, ax)
            alpha = max(alpha, ax)
            alpha = alpha * fade * self.opacity
            if alpha < 0.002 {
                discard()
            }
            return vec4(col * alpha, alpha)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // =====================================================================
    // LINES — the authored contour edges, hidden segments dashed
    // =====================================================================
    mod.draw.DrawFabLine = mod.std.set_type_default() do #(DrawFabLine::script_shader(vm)){
        alpha_blend: true
        depth_write: false
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(mod.fab_geom.FabLineVertex, mod.fab_geom.FabLineGeom)
        aux: texture_2d(float)
        elem_map: texture_2d(float)
        elem_ctl: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // xy = 1/pixel size, zw = pixel size
        u_texel: uniform(vec4(0.001, 0.001, 1000.0, 1000.0))
        // x = hidden-line style (0 skip, 1 dashed), y = dash period in px,
        // z = hidden alpha, w = unused
        u_hidden: uniform(vec4(1.0, 7.0, 0.35, 0.0))
        v_along: varying(float)
        v_view: varying(vec3f)
        v_clip: varying(vec4f)

        vertex: fn() {
            var a = self.geom.a
            var b = self.geom.b
            var hidden = 0.0
            if self.elem_ctl.w > 0.5 {
                let ew = max(self.elem_ctl.x, 1.0)
                let eh = max(self.elem_ctl.y, 1.0)
                let ex = (modf(self.geom.element, ew) + 0.5) / ew
                let ey = (floor(self.geom.element / ew) + 0.5) / eh
                let f = self.elem_map.sample_nearest(vec2(ex, ey))
                if f.x < 0.5 {
                    hidden = 1.0
                } else {
                    let off = vec3(f.y, 0.0 - f.w, f.z)
                    a = a + off
                    b = b + off
                }
            }
            let ca = self.draw_pass.camera_projection * (self.draw_pass.camera_view * vec4(a.x, a.y, a.z, 1.0))
            let cb = self.draw_pass.camera_projection * (self.draw_pass.camera_view * vec4(b.x, b.y, b.z, 1.0))
            // Screen-space widening: project both ends, take the
            // perpendicular, push out half a line width in pixels.
            let wa = max(abs(ca.w), 0.0001)
            let wb = max(abs(cb.w), 0.0001)
            let sa = vec2(ca.x / wa, ca.y / wa)
            let sb = vec2(cb.x / wb, cb.y / wb)
            let px = vec2(self.u_texel.z, self.u_texel.w)
            let da = (sb - sa) * px
            var dir = vec2(1.0, 0.0)
            if length(da) > 0.000001 {
                dir = normalize(da)
            }
            let perp = vec2(0.0 - dir.y, dir.x) / max(px, vec2(1.0, 1.0))
            var c = ca
            var vv = self.draw_pass.camera_view * vec4(a.x, a.y, a.z, 1.0)
            if self.geom.end > 0.5 {
                c = cb
                vv = self.draw_pass.camera_view * vec4(b.x, b.y, b.z, 1.0)
            }
            let w = max(abs(c.w), 0.0001)
            let half = self.width * 0.5
            let o = perp * self.geom.side * half * 2.0
            self.v_view = vv.xyz
            self.v_along = length((sb - sa) * px) * self.geom.end
            var outp = vec4(c.x + o.x * w, c.y + o.y * w, c.z, c.w)
            if hidden > 0.5 {
                // Behind the near plane: the whole quad is clipped away.
                outp = vec4(0.0, 0.0, 0.0 - 2.0, 1.0)
            }
            self.v_clip = outp
            self.vertex_pos = outp
        }

        pixel: fn() -> vec4f {
            let ndc = self.v_clip.xy / max(self.v_clip.w, 0.000001)
            let uv = vec2(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5)
            let g = self.aux.sample_nearest(uv)
            let my = length(self.v_view)
            var alpha = self.color.w
            // Slope-scaled depth bias: a grazing wall's aux depth changes
            // a lot across one pixel, so a fixed epsilon either buries the
            // 1 px ink or lets it z-fight. Slack grows with the local
            // depth derivative and with distance.
            let gl = self.aux.sample_nearest(uv + vec2(self.u_texel.x, 0.0))
            let gu = self.aux.sample_nearest(uv + vec2(0.0, self.u_texel.y))
            let slope = abs(gl.x - g.x) + abs(gu.x - g.x)
            let bias = 0.02 + 0.006 * my + slope * 1.25
            if g.w != 0.0 {
                if g.x < my - bias {
                    // Behind something: skipped, or dashed for CAD ink.
                    if self.u_hidden.x < 0.5 {
                        discard()
                    }
                    let period = max(self.u_hidden.y, 1.0)
                    if modf(self.v_along, period) > period * 0.55 {
                        discard()
                    }
                    alpha = alpha * self.u_hidden.z
                }
            }
            return vec4(self.color.xyz * alpha, alpha)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // =====================================================================
    // The composite back into the 2D pass.
    // =====================================================================
    mod.draw.DrawFabSceneTexture = mod.std.set_type_default() do #(DrawFabSceneTexture::script_shader(vm)){
        ..mod.draw.DrawQuad
        scene_texture: texture_2d(float)
        pixel: fn() {
            let color = self.scene_texture.sample_as_bgra(self.pos)
            return Pal.premul(color)
        }
    }

    mod.widgets.FabViewportBase = #(FabViewport::register_widget(vm))
    mod.widgets.FabViewport = set_type_default() do mod.widgets.FabViewportBase{
        width: Fill
        height: Fill
        view: 0
        clear_color: fab.color_vp_bg_top
        draw_bg: mod.draw.DrawFabSceneTexture{}
        draw_aux: mod.draw.DrawFabAux{}
        draw_comp: mod.draw.DrawFabComposite{
            u_bg_top: fab.color_vp_bg_top
            u_bg_bottom: fab.color_vp_bg_bottom
            u_select: fab.color_vp_select
            u_select_dim: fab.color_vp_select_dim
            u_hover_col: fab.color_vp_hover
            u_cap: fab.color_vp_cap
            u_ink: fab.color_vp_ink
            u_paper: fab.color_vp_paper
        }
        draw_grid: mod.draw.DrawFabGrid{
            u_grid: fab.color_vp_grid
            u_grid_major: fab.color_vp_grid_major
            u_axis_x: fab.color_vp_axis_x
            u_axis_y: fab.color_vp_axis_y
        }
        draw_line: mod.draw.DrawFabLine{}
        // The plate the Rendered-pane badge sits on. Same floating-chrome
        // token as the T toolbar and the N sidebar, so a caption over the
        // render belongs to the same viewport furniture as everything else.
        draw_badge_bg: mod.draw.DrawQuad{
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                sdf.fill_keep(fab.color_float)
                sdf.stroke(fab.color_float_border, 1.0)
                return sdf.result
            }
        }
        // The Rendered-pane badge: "converging · N spp" / "renderer pending".
        draw_badge: mod.draw.DrawText{
            text_style: theme.font_regular{
                font_size: fab.font_size_vp
            }
            color: fab.color_text_active
        }
        // The libs/render lanes. Sun terms are written every frame from
        // `SunSettings`; these are only the boot values.
        draw_cube: mod.draw.DrawSceneCube{}
        draw_alpha: mod.draw.DrawSceneAlpha{}
        draw_sky: mod.draw.DrawSceneSky{}
        draw_sky_analytic: mod.draw.DrawSceneSkyAnalytic{}
        draw_terrain: mod.draw.DrawSceneTerrain{}
        draw_shadow: mod.draw.DrawSceneShadow{}
        draw_models: mod.draw.DrawSceneSkinned{}
    }
}
