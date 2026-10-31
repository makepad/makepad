//! Light-map finishing passes: lamp dilate, atlas encode, top-plane passes.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // Lamp rim fill + smooth, mirroring dilate_rgb: mode 0/1 = one ring of
    // averaging into non-holding texels, mode 2 = the coverage-weighted
    // 4/2/1 smooth over holding texels. Alpha carries "holds light".
    //
    // PARTIAL-COVERAGE REPAIR (modes 0/1): a chart-EDGE texel — one the 4x
    // coverage pass saw only partly inside the chart — is not trusted with
    // its rasterized value. The AO charts put face edges exactly ON texel
    // centers, so at 1x the edge texel goes to whichever face wins the
    // rasterization tie; on a tile's max edge that is the 0.16-unit
    // vertical SKIRT, whose near-horizontal normal takes ~0.4x the lamp
    // light of the top face. The material shader samples that texel at FULL
    // weight along the face's outer edge (edge uv = its center), which drew
    // a hard dark line at every static-static boundary crossing a lamp pool
    // (measured: the plaza tile's boundary row baked at a constant 0.43-0.49
    // of the ground-region truth). The repair rebuilds such a texel as the
    // LINEAR CONTINUATION of the fully-covered interior next to it — the
    // value the smooth field actually has at that texel's world position —
    // so both sides of a shared edge land on the same curve.
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
        cov_tex: texture_2d(float)
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

        // Chart coverage of the texel at `off` (cov G lane, 1.0 = the whole
        // texel lies inside rasterized chart area). Outside the rect: 0.
        covf: fn(off: vec2) -> float {
            let c = self.v_local + off
            if c.x < 0.0 {
                return 0.0
            }
            if c.y < 0.0 {
                return 0.0
            }
            if c.x > self.rect_px.z {
                return 0.0
            }
            if c.y > self.rect_px.w {
                return 0.0
            }
            let uv = (self.rect_px.xy + c) * vec2(self.misc_a.x, self.misc_a.x)
            return self.cov_tex.sample_nearest(uv).y
        }

        // Max channel of a fully-covered holding neighbor, -1.0 when the
        // texel at `off` is not trustworthy (partial, empty, outside).
        mch: fn(off: vec2) -> float {
            let s = self.rg(off)
            if s.w < 0.5 {
                return -1.0
            }
            if self.covf(off) < 0.99 {
                return -1.0
            }
            return max(max(s.x, s.y), s.z)
        }

        // A fully-covered holding neighbor's LINEAR CONTINUATION onto self:
        // value at off, extended by the gradient toward self when the texel
        // one further out is also trustworthy. Clamped to [0, 2v] so a
        // shadow edge cannot extrapolate negative or overshoot double.
        exw: fn(off: vec2, w: float) -> vec4 {
            let s = self.rg(off)
            if s.w < 0.5 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            if self.covf(off) < 0.99 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            var e = s.xyz
            let s2 = self.rg(off * 2.0)
            if s2.w > 0.5 {
                if self.covf(off * 2.0) > 0.99 {
                    // Extrapolate only across a MONOTONE-ish stretch: when
                    // the far texel has lost more than half the near one's
                    // light there is a shadow edge between them, and the
                    // difference is the shadow's, not the field's — 2a - s2
                    // through the lamp post's umbra painted a 210 bright
                    // spot on a 128 corner. Fall back to the plain value.
                    let sm = max(max(s.x, s.y), s.z)
                    let s2m = max(max(s2.x, s2.y), s2.z)
                    if s2m >= sm * 0.5 {
                        e = clamp(
                            s.xyz * 2.0 - s2.xyz,
                            vec3(0.0, 0.0, 0.0),
                            s.xyz * 1.5
                        )
                    }
                }
            }
            return vec4(e * w, w)
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
                // Partial-coverage texels carry the repaired boundary value;
                // smoothing them against the (dimmer) outside rim would eat
                // the repair back out — measured: a rebuilt 122 fell to 91.
                if self.covf(vec2(0.0, 0.0)) < 0.99 {
                    return vec4(own.xyz, 1.0)
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
                if self.covf(vec2(0.0, 0.0)) > 0.99 {
                    return vec4(own.xyz, 1.0)
                }
                // Chart-edge texel: find the DIMMEST trusted interior
                // neighbor. The steal signature is a texel darker than
                // EVERY fully-covered neighbor — the sub-texel skirt takes
                // ~0.4x the top face's light, below anything around it. A
                // texel within its neighbors' range IS the face's own
                // sample (it won the rasterization tie, or it sits on a
                // shadow edge only the raw sample gets right — measured: a
                // correct half-shadowed texel rebuilt from its lit
                // neighbors overshot 128 -> 242 beside the lamp post).
                var minv = 100000.0
                var cnt = 0.0
                var m = self.mch(vec2(-1.0, -1.0))
                if m >= 0.0 {
                    minv = min(minv, m)
                    cnt = cnt + 1.0
                }
                m = self.mch(vec2(0.0, -1.0))
                if m >= 0.0 {
                    minv = min(minv, m)
                    cnt = cnt + 1.0
                }
                m = self.mch(vec2(1.0, -1.0))
                if m >= 0.0 {
                    minv = min(minv, m)
                    cnt = cnt + 1.0
                }
                m = self.mch(vec2(-1.0, 0.0))
                if m >= 0.0 {
                    minv = min(minv, m)
                    cnt = cnt + 1.0
                }
                m = self.mch(vec2(1.0, 0.0))
                if m >= 0.0 {
                    minv = min(minv, m)
                    cnt = cnt + 1.0
                }
                m = self.mch(vec2(-1.0, 1.0))
                if m >= 0.0 {
                    minv = min(minv, m)
                    cnt = cnt + 1.0
                }
                m = self.mch(vec2(0.0, 1.0))
                if m >= 0.0 {
                    minv = min(minv, m)
                    cnt = cnt + 1.0
                }
                m = self.mch(vec2(1.0, 1.0))
                if m >= 0.0 {
                    minv = min(minv, m)
                    cnt = cnt + 1.0
                }
                if cnt < 0.5 {
                    // No trusted interior in reach (a chart thinner than a
                    // texel): the rasterized value is the best there is.
                    return vec4(own.xyz, 1.0)
                }
                // 0.85: a texel on the dark side of a legitimate gradient
                // sits up to ~one texel-step (~10-15%) below its dimmest
                // interior neighbor and must be KEPT; the skirt steal sits
                // at ~0.43x and must not be. Measured margins on the town:
                // kept-correct 128 vs threshold 119, rebuilt-steal 57 vs
                // threshold 57.8.
                let om = max(max(own.x, own.y), own.z)
                if om >= minv * 0.85 {
                    return vec4(own.xyz, 1.0)
                }
                var eac = vec4(0.0, 0.0, 0.0, 0.0)
                eac = eac + self.exw(vec2(-1.0, -1.0), 1.0)
                eac = eac + self.exw(vec2(0.0, -1.0), 2.0)
                eac = eac + self.exw(vec2(1.0, -1.0), 1.0)
                eac = eac + self.exw(vec2(-1.0, 0.0), 2.0)
                eac = eac + self.exw(vec2(1.0, 0.0), 2.0)
                eac = eac + self.exw(vec2(-1.0, 1.0), 1.0)
                eac = eac + self.exw(vec2(0.0, 1.0), 2.0)
                eac = eac + self.exw(vec2(1.0, 1.0), 1.0)
                return vec4(eac.xyz / max(eac.w, 0.5), 1.0)
            }
            // EMPTY texel. A chart texel that holds part of a face but
            // received no fragment lost the rasterization tie by an
            // epsilon (a face edge exactly on its center): it is the very
            // texel the face's outer edge SAMPLES, so fill it with the
            // field's linear continuation, not the rim's plateau — the
            // plateau displayed the strip's edge a half-texel inward and
            // drew a faint dark line down the pool at x=-12.
            if self.covf(vec2(0.0, 0.0)) > 0.001 {
                var eac = vec4(0.0, 0.0, 0.0, 0.0)
                eac = eac + self.exw(vec2(-1.0, -1.0), 1.0)
                eac = eac + self.exw(vec2(0.0, -1.0), 2.0)
                eac = eac + self.exw(vec2(1.0, -1.0), 1.0)
                eac = eac + self.exw(vec2(-1.0, 0.0), 2.0)
                eac = eac + self.exw(vec2(1.0, 0.0), 2.0)
                eac = eac + self.exw(vec2(-1.0, 1.0), 1.0)
                eac = eac + self.exw(vec2(0.0, 1.0), 2.0)
                eac = eac + self.exw(vec2(1.0, 1.0), 1.0)
                if eac.w > 0.5 {
                    return vec4(eac.xyz / eac.w, 1.0)
                }
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
            // The encode band, in THIS region's texels, delivered by the
            // baker as lightmap::LM_SUN_BAND_WORLD x the region's chart
            // density — one world-space penumbra width for every region.
            var band = self.misc_a.z
            if band < 0.5 {
                band = 4.0
            }
            let dl = dt.x * 6.0
            let ds = dt.y * 6.0
            var sd = 0.0
            if dl <= ds {
                sd = min(ds, band)
            } else {
                sd = 0.0 - min(dl, band)
            }
            // Sub-texel edge nudge from the lit fraction — FULLY covered
            // texels only. A chart-edge texel's fraction mixes the skirt's
            // lighting into the top face's (the same steal the lamp dilate
            // repairs), so its fraction places no edge.
            if cov.y > 0.996 {
                if cov.x > 0.001 {
                    if cov.x < 0.999 {
                        let c = (cov.x - 0.5) * 2.0
                        let w = clamp(1.0 - abs(sd), 0.0, 1.0)
                        sd = sd + (c - sd) * w
                    }
                }
            }
            let a = clamp((128.0 + sd * (127.0 / band)) / 255.0, 0.0, 1.0)
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
