//! Light-map gather passes: sun/lamp gathers, despeckle, downsample, chamfer.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

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

        // Coverage of the texel at `off`: (lit fraction, covered fraction),
        // covered forced to 0 outside the region rect.
        cvf: fn(off: vec2) -> vec2 {
            let c = self.v_local + off
            if c.x < 0.0 {
                return vec2(0.0, 0.0)
            }
            if c.y < 0.0 {
                return vec2(0.0, 0.0)
            }
            if c.x > self.rect_px.z {
                return vec2(0.0, 0.0)
            }
            if c.y > self.rect_px.w {
                return vec2(0.0, 0.0)
            }
            let uv = (self.rect_px.xy + c) * vec2(self.misc_a.x, self.misc_a.x)
            let s = self.cov_tex.sample_nearest(uv)
            return vec2(s.x, s.y)
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
                    var f = c.x
                    if c.y < 0.996 {
                        // Chart-edge texel: its lit fraction mixes another
                        // face's lighting into this one's (the skirt steal
                        // the lamp dilate repairs), so it must not place a
                        // shadow edge. Seed from the fully-covered ring
                        // when there is one; a chart thinner than a texel
                        // keeps its own fraction — the best it has.
                        var acc = 0.0
                        var n = 0.0
                        var s = self.cvf(vec2(-1.0, -1.0))
                        if s.y > 0.996 {
                            acc = acc + s.x
                            n = n + 1.0
                        }
                        s = self.cvf(vec2(0.0, -1.0))
                        if s.y > 0.996 {
                            acc = acc + s.x
                            n = n + 1.0
                        }
                        s = self.cvf(vec2(1.0, -1.0))
                        if s.y > 0.996 {
                            acc = acc + s.x
                            n = n + 1.0
                        }
                        s = self.cvf(vec2(-1.0, 0.0))
                        if s.y > 0.996 {
                            acc = acc + s.x
                            n = n + 1.0
                        }
                        s = self.cvf(vec2(1.0, 0.0))
                        if s.y > 0.996 {
                            acc = acc + s.x
                            n = n + 1.0
                        }
                        s = self.cvf(vec2(-1.0, 1.0))
                        if s.y > 0.996 {
                            acc = acc + s.x
                            n = n + 1.0
                        }
                        s = self.cvf(vec2(0.0, 1.0))
                        if s.y > 0.996 {
                            acc = acc + s.x
                            n = n + 1.0
                        }
                        s = self.cvf(vec2(1.0, 1.0))
                        if s.y > 0.996 {
                            acc = acc + s.x
                            n = n + 1.0
                        }
                        if n > 0.5 {
                            f = acc / n
                        }
                    }
                    if f >= 0.999 {
                        dl = 0.076923077
                    } else {
                        if f <= 0.001 {
                            ds = 0.076923077
                        } else {
                            // The boundary texel: lit fraction f puts the
                            // edge (0.5 - f) texels from the centre. Both
                            // channels take the SIGNED offset — stored
                            // (d + 0.5)/6.5 these collapse to (1-f)/6.5
                            // and f/6.5.
                            dl = (1.0 - f) * 0.153846154
                            ds = f * 0.153846154
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
    // Region-scoped hard zero of a persistent bake target. The quad covers
    // one region's rect plus its reserved pad ring (LmRect::padded), and
    // blending is OFF — this is a real clear, not another accumulation. The
    // lamp accumulator loads its own previous contents (a batch touches only
    // its own regions), so a re-bake MUST start from this, not from the
    // coverage prepass's chart-shaped marks.
    mod.draw.DrawLmZero = mod.std.set_type_default() do #(DrawLmZero::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Bgra8NoBlend
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)

        vertex: fn() {
            let tp = self.quad_a.xy + self.geom.pos * self.quad_a.zw
            self.vertex_pos = vec4(tp.x * 2.0 - 1.0, 1.0 - 2.0 * tp.y, 0.5, 1.0)
        }

        pixel: fn() {
            return self.fill_a
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

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
            let range = max(self.lamp_d.y - near, 0.0001)
            let blk = near + blk01 * range
            let scaled = fv.z * max(d - 0.25, 0.0) / d
            let bias = self.lamp_d.z
            // The bulb is not a point: a lantern's glass is ~0.24 units
            // across, so a blocker throws a penumbra that widens with the
            // receiver's distance past it. 5-tap PCF whose radius follows
            // the source-size geometry — a fixture's post melts into a
            // soft wash on its own pool instead of a razor-edged bar,
            // while a far wall's shadow stays sharp (radius ~ 1/blocker
            // distance). Taps clamp inside the face tile so the test
            // never reads a neighbouring cube face.
            let pr = clamp((scaled - blk) / max(blk, 0.25), 0.0, 3.0) * 0.12
            let du = pr * 0.5 / vz
            var vis = 0.0
            if blk >= scaled - bias {
                vis = vis + 1.0
            }
            let tu1 = clamp(tu - du, 0.001, 0.999)
            let b1 = near + self.depth_tex.sample_nearest(vec2((col + tu1) / 3.0, (row + tv) / 2.0)).x * range
            if b1 >= scaled - bias {
                vis = vis + 1.0
            }
            let tu2 = clamp(tu + du, 0.001, 0.999)
            let b2 = near + self.depth_tex.sample_nearest(vec2((col + tu2) / 3.0, (row + tv) / 2.0)).x * range
            if b2 >= scaled - bias {
                vis = vis + 1.0
            }
            let tv1 = clamp(tv - du, 0.001, 0.999)
            let b3 = near + self.depth_tex.sample_nearest(vec2((col + tu) / 3.0, (row + tv1) / 2.0)).x * range
            if b3 >= scaled - bias {
                vis = vis + 1.0
            }
            let tv2 = clamp(tv + du, 0.001, 0.999)
            let b4 = near + self.depth_tex.sample_nearest(vec2((col + tu) / 3.0, (row + tv2) / 2.0)).x * range
            if b4 >= scaled - bias {
                vis = vis + 1.0
            }
            if vis < 0.5 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let att = 1.0 - d / self.lamp_a.w
            var s = ndl * att * att * (vis * 0.2)
            if self.lamp_b.w > 0.0 {
                let cone = clamp((dot(dir * -1.0, self.lamp_c.xyz) + 0.35) / 1.35, 0.0, 1.0)
                s = s * (cone * cone * self.lamp_b.w + (1.0 - self.lamp_b.w))
            }
            // 1.111111 = 1 / lightmap::LM_LAMP_CEIL — the atlas RGB encode.
            return vec4(self.lamp_b.xyz * (s * 1.111111), 0.0)
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
            let range = max(self.lamp_d.y - near, 0.0001)
            let blk = near + blk01 * range
            let scaled = fv.z * max(d - 0.25, 0.0) / d
            let bias = self.lamp_d.z
            // The bulb is not a point: a lantern's glass is ~0.24 units
            // across, so a blocker throws a penumbra that widens with the
            // receiver's distance past it. 5-tap PCF whose radius follows
            // the source-size geometry — a fixture's post melts into a
            // soft wash on its own pool instead of a razor-edged bar,
            // while a far wall's shadow stays sharp (radius ~ 1/blocker
            // distance). Taps clamp inside the face tile so the test
            // never reads a neighbouring cube face.
            let pr = clamp((scaled - blk) / max(blk, 0.25), 0.0, 3.0) * 0.12
            let du = pr * 0.5 / vz
            var vis = 0.0
            if blk >= scaled - bias {
                vis = vis + 1.0
            }
            let tu1 = clamp(tu - du, 0.001, 0.999)
            let b1 = near + self.depth_tex.sample_nearest(vec2((col + tu1) / 3.0, (row + tv) / 2.0)).x * range
            if b1 >= scaled - bias {
                vis = vis + 1.0
            }
            let tu2 = clamp(tu + du, 0.001, 0.999)
            let b2 = near + self.depth_tex.sample_nearest(vec2((col + tu2) / 3.0, (row + tv) / 2.0)).x * range
            if b2 >= scaled - bias {
                vis = vis + 1.0
            }
            let tv1 = clamp(tv - du, 0.001, 0.999)
            let b3 = near + self.depth_tex.sample_nearest(vec2((col + tu) / 3.0, (row + tv1) / 2.0)).x * range
            if b3 >= scaled - bias {
                vis = vis + 1.0
            }
            let tv2 = clamp(tv + du, 0.001, 0.999)
            let b4 = near + self.depth_tex.sample_nearest(vec2((col + tu) / 3.0, (row + tv2) / 2.0)).x * range
            if b4 >= scaled - bias {
                vis = vis + 1.0
            }
            if vis < 0.5 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let att = 1.0 - d / self.lamp_a.w
            var s = ndl * att * att * (vis * 0.2)
            if self.lamp_b.w > 0.0 {
                let cone = clamp((dot(dir * -1.0, self.lamp_c.xyz) + 0.35) / 1.35, 0.0, 1.0)
                s = s * (cone * cone * self.lamp_b.w + (1.0 - self.lamp_b.w))
            }
            // 1.111111 = 1 / lightmap::LM_LAMP_CEIL — the atlas RGB encode.
            return vec4(self.lamp_b.xyz * (s * 1.111111), 0.0)
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
}
