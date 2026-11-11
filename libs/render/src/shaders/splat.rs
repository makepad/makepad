//! DrawSceneSplat: a generated terrain tile's paint, blended per pixel in
//! one opaque draw.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // A terrain tile (gen level terrain_mesh: one part of triplanar
    // triangles, one of flat ones) draws here: DrawScenePbr's vertex stage,
    // binding and lighting (`shade_mr`), with a pixel that paints the
    // tile's layers itself. Before, every triangle drew again, alpha-cut,
    // in the overlay part of each other layer weighing at a corner (about
    // seven deep): each overlay fragment ran the triplanar read and the
    // cover rule and then mostly discarded, and a pipeline that can discard
    // costs early-Z and a tile GPU its hidden-surface removal.
    //
    // The rule is the overlays' rule, so the picture is theirs: a layer
    // covers where w + (height - 0.5) * 0.45 + (noise - 0.5) * 0.5 clears
    // 0.36 (w its weight, height its texture's alpha, noise world value
    // noise at ~2 m and ~0.5 m) and w >= 0.02; the last covering layer in
    // paint order wins, else the heaviest. Walking the layers from the last
    // down, the first that covers is the one; the dominant one needs no
    // test. Layer reads sit in branches that differ between pixels, so they
    // take explicit gradients (sample_grad_repeat).
    //
    // Bindings (the PBR lane's seventeen texture slots, retyped where the
    // lane does not read them): `tex` is the layers' albedo array (height
    // in alpha), `normal_map` their normals with relative roughness in alpha,
    // and `occlusion_map` the tile's weight map (no occlusion: occlusion_strength stays 0). Per
    // layer, splat_l<i> = (UV scale 1/m, roughness, metallic, normal
    // strength).
    mod.draw.DrawSceneSplat = mod.std.set_type_default() do #(DrawSceneSplat::script_shader(vm)){
        ..mod.draw.DrawScenePbr,
        tex: texture_2d_array(float)
        normal_map: texture_2d_array(float)
        // Weight map: world x, z of texel 0; texels per metre.
        splat_map: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        // Weight map: texels per row, rows per page, pages; layers.
        splat_size: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        // 1 blends the layers, 0 paints the heaviest only (a far tile,
        // TierBudget::terrain_blend_reach).
        splat_blend: uniform(1.0)
        splat_l0: uniform(vec4(1.0, 0.9, 0.0, 0.0))
        splat_l1: uniform(vec4(1.0, 0.9, 0.0, 0.0))
        splat_l2: uniform(vec4(1.0, 0.9, 0.0, 0.0))
        splat_l3: uniform(vec4(1.0, 0.9, 0.0, 0.0))
        splat_l4: uniform(vec4(1.0, 0.9, 0.0, 0.0))
        splat_l5: uniform(vec4(1.0, 0.9, 0.0, 0.0))
        splat_l6: uniform(vec4(1.0, 0.9, 0.0, 0.0))
        splat_l7: uniform(vec4(1.0, 0.9, 0.0, 0.0))

        // x: relief metres, y: wet, z: puddle share, w: height quantile.
        splat_relief_on: uniform(0.0)
        splat_s0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_s1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_s2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_s3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_s4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_s5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_s6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_s7: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        splat_surface_param: fn(l: float) -> vec4 {
            if l < 0.5 { return self.splat_s0 }
            if l < 1.5 { return self.splat_s1 }
            if l < 2.5 { return self.splat_s2 }
            if l < 3.5 { return self.splat_s3 }
            if l < 4.5 { return self.splat_s4 }
            if l < 5.5 { return self.splat_s5 }
            if l < 6.5 { return self.splat_s6 }
            return self.splat_s7
        }

        splat_param: fn(l: float) -> vec4 {
            if l < 0.5 { return self.splat_l0 }
            if l < 1.5 { return self.splat_l1 }
            if l < 2.5 { return self.splat_l2 }
            if l < 3.5 { return self.splat_l3 }
            if l < 4.5 { return self.splat_l4 }
            if l < 5.5 { return self.splat_l5 }
            if l < 6.5 { return self.splat_l6 }
            return self.splat_l7
        }

        splat_weight: fn(l: float, wa: vec4, wb: vec4) -> float {
            if l < 0.5 { return wa.x }
            if l < 1.5 { return wa.y }
            if l < 2.5 { return wa.z }
            if l < 3.5 { return wa.w }
            if l < 4.5 { return wb.x }
            if l < 5.5 { return wb.y }
            if l < 6.5 { return wb.z }
            return wb.w
        }

        // Layer l's texel at world position wp (height in w): the top
        // projection on a flat part, three projections blended by `bw`
        // otherwise. dx, dy: the screen derivatives of wp.
        splat_texel: fn(l: float, wp: vec3, bw: vec3, dx: vec3, dy: vec3) -> vec4 {
            let s = self.splat_param(l).x
            let ty = self.tex.sample_grad_repeat(vec3(wp.x * s, wp.z * s, l), vec2(dx.x, dx.z) * s, vec2(dy.x, dy.z) * s)
            if self.triplanar < 0.0 { return ty }
            let tx = self.tex.sample_grad_repeat(vec3(wp.z * s, (0.0 - wp.y) * s, l), vec2(dx.z, 0.0 - dx.y) * s, vec2(dy.z, 0.0 - dy.y) * s)
            let tz = self.tex.sample_grad_repeat(vec3(wp.x * s, (0.0 - wp.y) * s, l), vec2(dx.x, 0.0 - dx.y) * s, vec2(dy.x, 0.0 - dy.y) * s)
            return tx * bw.x + ty * bw.y + tz * bw.z
        }

        // Mesh UVs for a single surface; the existing world projections for
        // terrain. Explicit gradients stay those of the undisplaced surface.
        splat_relief_read: fn(l: float, wp: vec3, uv: vec2, bw: vec3, dx: vec3, dy: vec3, du1: vec2, du2: vec2) -> vec4 {
            if self.triplanar == 0.0 {
                return self.tex.sample_grad_repeat(vec3(uv.x, uv.y, l), du1, du2)
            }
            return self.splat_texel(l, wp, bw, dx, dy)
        }

        // Linear POM in normalised depth (0 at the top, 1 at the bottom).
        // A fixed loop bound, 8..24 active steps, then one secant refinement
        // between the last two samples. The secant needs no texture read.
        splat_relief_trace: fn(l: float, wp: vec3, uv: vec2, ray: vec3, uv_ray: vec2, bw: vec3, dx: vec3, dy: vec3, du1: vec2, du2: vec2, height: float, nv: float) -> float {
            let steps = ceil(mix(24.0, 8.0, clamp((nv - 0.1391731) / 0.8608269, 0.0, 1.0)))
            var depth = 0.0
            var before_depth = 0.0
            var error = 1.0 - height
            var before_error = error
            for i in 0..24 {
                if float(i) < steps && error > 0.0 {
                    before_depth = depth
                    before_error = error
                    depth = (float(i) + 1.0) / steps
                    let h = self.splat_relief_read(l, wp - ray * depth, uv - uv_ray * depth, bw, dx, dy, du1, du2).w
                    error = 1.0 - h - depth
                }
            }
            let fraction = clamp(before_error / max(before_error - error, 0.00001), 0.0, 1.0)
            return mix(before_depth, depth, fraction)
        }

        // Four height taps from the hit toward the sun; only direct sunlight
        // is attenuated. The ambient/IBL/GI paths retain their own occlusion.
        splat_relief_shadow: fn(l: float, wp: vec3, uv: vec2, ray: vec3, uv_ray: vec2, hit: float, bw: vec3, dx: vec3, dy: vec3, du1: vec2, du2: vec2) -> float {
            var blocked = 0.0
            for i in 0..4 {
                let rise = hit * ((float(i) + 1.0) * 0.25)
                let h = self.splat_relief_read(l, wp + ray * rise, uv + uv_ray * rise, bw, dx, dy, du1, du2).w
                blocked = max(blocked, smoothstep(0.015, 0.08, h - (1.0 - hit + rise)))
            }
            return 1.0 - 0.65 * blocked
        }

        // Layer l, tested from the top of the paint down (`won` < 0: none
        // has covered yet).
        splat_try: fn(l: float, won: vec4, wa: vec4, wb: vec4, dom: float, nz: float, wp: vec3, bw: vec3, dx: vec3, dy: vec3) -> vec4 {
            if won.w >= 0.0 || l >= self.splat_size.w || l == dom { return won }
            let w = self.splat_weight(l, wa, wb)
            if w < 0.02 { return won }
            let t = self.splat_texel(l, wp, bw, dx, dy)
            if w + (t.w - 0.5) * 0.45 + (nz - 0.5) * 0.5 < 0.36 { return won }
            return vec4(l, 0.0, 0.0, l)
        }

        pixel: fn() {
            self.host_cut()
            self.door_window()
            let wp = self.v_csm.xyz
            let dx = dFdx(wp)
            let dy = dFdy(wp)
            let gn = abs(normalize(self.v_csm_n))
            var bw = gn * gn
            bw = bw * bw
            bw = bw / max(bw.x + bw.y + bw.z, 0.0001)
            // The weights: bilinear between the vertices, each page's rows
            // clamped so the filter never reaches the next page.
            let sz = self.splat_size
            let g = clamp((vec2(wp.x, wp.z) - self.splat_map.xy) * self.splat_map.z, vec2(0.0, 0.0), vec2(sz.x - 1.0, sz.y - 1.0)) + vec2(0.5, 0.5)
            let rows = sz.y * sz.z
            var wa = vec4(1.0, 0.0, 0.0, 0.0)
            if sz.w > 1.5 { wa = self.occlusion_map.sample_lod(vec2(g.x / sz.x, g.y / rows), 0.0) }
            var wb = vec4(0.0, 0.0, 0.0, 0.0)
            if sz.z > 1.5 { wb = self.occlusion_map.sample_lod(vec2(g.x / sz.x, (g.y + sz.y) / rows), 0.0) }
            // The heaviest layer: what shows where none covers.
            var dom = 0.0
            var best = wa.x
            if sz.w > 1.5 && wa.y > best {
                dom = 1.0
                best = wa.y
            }
            if sz.w > 2.5 && wa.z > best {
                dom = 2.0
                best = wa.z
            }
            if sz.w > 3.5 && wa.w > best {
                dom = 3.0
                best = wa.w
            }
            if sz.w > 4.5 && wb.x > best {
                dom = 4.0
                best = wb.x
            }
            if sz.w > 5.5 && wb.y > best {
                dom = 5.0
                best = wb.y
            }
            if sz.w > 6.5 && wb.z > best {
                dom = 6.0
                best = wb.z
            }
            if sz.w > 7.5 && wb.w > best {
                dom = 7.0
                best = wb.w
            }
            var won = vec4(0.0, 0.0, 0.0, 0.0 - 1.0)
            if self.splat_blend > 0.5 && sz.w > 1.5 {
                let np = vec2(wp.x, wp.z)
                let nz = self.tn_noise(np * 0.45) * 0.6 + self.tn_noise(np * 1.9) * 0.4
                won = self.splat_try(7.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
                won = self.splat_try(6.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
                won = self.splat_try(5.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
                won = self.splat_try(4.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
                won = self.splat_try(3.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
                won = self.splat_try(2.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
                won = self.splat_try(1.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
                won = self.splat_try(0.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
            }
            var l = dom
            if won.w >= 0.0 { l = won.w }
            let p = self.splat_param(l)
            let surface_param = self.splat_surface_param(l)
            let s = p.x
            let dist = length(self.eye.xyz - wp)
            var n = normalize(self.v_csm_n)
            if self.double_sided > 0.5 && dot(n, self.toward_eye(wp)) < 0.0 { n = n * -1.0 }
            let geometric_normal = n
            // Terrain already has world projection axes. Meshes carry UVs,
            // not their generating brush axes (chamfers can inherit another
            // face's projection), so only that path uses UV derivatives.
            var axis_u = vec3(1.0, 0.0, 0.0)
            var axis_v = vec3(0.0, 0.0, 1.0)
            if n.y < 0.6 {
                if abs(n.x) > abs(n.z) {
                    axis_u = vec3(0.0, 0.0, sign(n.x))
                } else {
                    axis_u = vec3(0.0 - sign(n.z), 0.0, 0.0)
                }
                axis_v = vec3(0.0, -1.0, 0.0)
            }
            var uv = vec2(dot(wp, axis_u), dot(wp, axis_v)) * s
            var du1 = vec2(dot(dx, axis_u), dot(dx, axis_v)) * s
            var du2 = vec2(dot(dy, axis_u), dot(dy, axis_v)) * s
            if self.triplanar == 0.0 {
                uv = self.v_uv
                du1 = dFdx(self.v_uv)
                du2 = dFdy(self.v_uv)
            }
            var tex = self.splat_relief_read(l, wp, uv, bw, dx, dy, du1, du2)
            var sun_visibility = 1.0
            // Uniform gate: no ray construction, march or shadow taps for
            // draws without relief. Only the visible winning layer marches.
            if self.splat_relief_on > 0.5 {
                if surface_param.x > 0.0 && dist < 20.0 {
                    var grad_u = axis_u * s
                    var grad_v = axis_v * s
                    if self.triplanar == 0.0 {
                        let area = dot(dx, cross(dy, n))
                        grad_u = vec3(0.0, 0.0, 0.0)
                        grad_v = vec3(0.0, 0.0, 0.0)
                        if abs(area) > 0.00000001 {
                            grad_u = (cross(dy, n) * du1.x + cross(n, dx) * du2.x) / area
                            grad_v = (cross(dy, n) * du1.y + cross(n, dx) * du2.y) / area
                        }
                    }
                    let view = normalize(self.toward_eye(wp))
                    let nv = dot(n, view)
                    // sin(8 degrees)..sin(16 degrees): entirely off below
                    // eight degrees, smoothly restored above it.
                    let relief_fade = (1.0 - smoothstep(12.0, 20.0, dist)) * smoothstep(0.1391731, 0.2756374, nv)
                    let tu = grad_u - n * dot(n, grad_u)
                    if relief_fade > 0.0 && dot(tu, tu) > 0.00000001 {
                        let tangent = normalize(tu)
                        let bitangent = normalize(cross(n, tangent))
                        let view_ts = vec3(dot(view, tangent), dot(view, bitangent), nv)
                        let depth_metres = surface_param.x * relief_fade
                        let ray = (tangent * view_ts.x + bitangent * view_ts.y) * (depth_metres / nv)
                        let uv_ray = vec2(dot(ray, grad_u), dot(ray, grad_v))
                        let hit = self.splat_relief_trace(l, wp, uv, ray, uv_ray, bw, dx, dy, du1, du2, tex.w, nv)
                        let hit_wp = wp - ray * hit
                        uv = uv - uv_ray * hit
                        tex = self.splat_relief_read(l, hit_wp, uv, bw, dx, dy, du1, du2)
                        if dist < 8.0 {
                            let light = normalize(self.light_dir)
                            let nl = dot(n, light)
                            if nl > 0.0 {
                                let light_ts = vec3(dot(light, tangent), dot(light, bitangent), nl)
                                let sun_ray = (tangent * light_ts.x + bitangent * light_ts.y) * (depth_metres / max(nl, 0.0001))
                                let sun_uv = vec2(dot(sun_ray, grad_u), dot(sun_ray, grad_v))
                                let visibility = self.splat_relief_shadow(l, hit_wp, uv, sun_ray, sun_uv, hit, bw, dx, dy, du1, du2)
                                sun_visibility = mix(1.0, visibility, relief_fade * (1.0 - smoothstep(6.0, 8.0, dist)))
                            }
                        }
                    }
                }
            }
            // Anti-tiling, as DrawScenePbr's triplanar path: with distance a
            // second top-down read at an unrelated scale and offset.
            let far = smoothstep(25.0, 140.0, dist)
            if self.triplanar != 0.0 && far > 0.001 {
                let s2 = s * 0.173
                let t2 = self.tex.sample_grad_repeat(vec3(wp.x * s2 + 0.31, wp.z * s2 - 0.47, l), vec2(dx.x, dx.z) * s2, vec2(dy.x, dy.z) * s2)
                tex = vec4(mix(tex.xyz, t2.xyz, 0.5 * far * bw.y), tex.w)
            }
            // The brightness drift over ~60 m.
            if self.triplanar != 0.0 {
                let drift = self.tn_noise(vec2(wp.x, wp.z) * 0.017) * 0.65 + self.tn_noise(vec2(wp.x, wp.z) * 0.061) * 0.35
                tex = vec4(tex.xyz * (0.86 + 0.28 * drift), tex.w)
            }
            var water = 0.0
            if surface_param.z > 0.0 {
                // Low-frequency, world-anchored patches, continuous on walls
                // as well as floors. No noise texture or extra height read.
                let patch = self.tn_noise(vec2(wp.x + wp.y * 0.31, wp.z + wp.y * 0.73) * 0.22)
                let water_height = tex.w + (patch - 0.5) * (0.16 * (1.0 - surface_param.z))
                water = 1.0 - smoothstep(surface_param.w - 0.03, surface_param.w + 0.03, water_height)
                // A patch mask also dries the deepest joints between pools;
                // a tiny threshold wobble alone would wet every mortar line.
                water = water * smoothstep(0.25 * (1.0 - surface_param.z), max(0.65 * (1.0 - surface_param.z), 0.0001), patch)
                if surface_param.z >= 1.0 { water = 1.0 }
            }
            let wet = surface_param.y
            let base = self.to_scene(vec3(tex.x, tex.y, tex.z)) * ((1.0 - 0.35 * wet) * (1.0 - 0.35 * water))
            var vertex_tint = self.to_lin(self.v_tint.xyz)
            if self.triplanar == 0.0 { vertex_tint = self.to_scene(self.v_tint.xyz) }
            let surf = self.surface(vec4(self.color_adjust(
                base * vertex_tint,
                self.tint,
                self.color_adjust_ctl
            ), 1.0))
            // One read for both the normal and roughness, at the same hit.
            let normal_rough = self.normal_map.sample_grad_repeat(vec3(uv.x, uv.y, l), du1, du2)
            let determinant = du1.x * du2.y - du1.y * du2.x
            if p.w > 0.00001 && abs(determinant) > 0.00000001 {
                let orientation = sign(determinant)
                let tangent = normalize(dx * du2.y - dy * du1.y) * orientation
                let bitangent = normalize(dy * du1.x - dx * du2.x) * orientation
                let xy = normal_rough.xy * 2.0 - vec2(1.0, 1.0)
                let mapped = vec3(xy.x, xy.y, sqrt(max(1.0 - dot(xy, xy), 0.0)))
                n = normalize(tangent * (mapped.x * p.w) + bitangent * (mapped.y * p.w) + n * mapped.z)
            }
            n = self.mat_normal(n)
            if water > 0.0 { n = normalize(mix(n, geometric_normal, water)) }
            let dry_rough = clamp(p.y + (normal_rough.w - 0.5) * 0.7, 0.04, 1.0)
            let wet_rough = mix(dry_rough, 0.08, wet)
            let mr = self.mat_metal_rough(vec2(clamp(p.z, 0.0, 1.0) * (1.0 - water), mix(wet_rough, 0.03, water)))
            let f0 = mix(mix(vec3(0.04, 0.04, 0.04), surf.xyz, mr.x), vec3(0.02, 0.02, 0.02), water)
            return self.shade_mr_surface(surf.xyz, surf.w, mr, n, vec3(0.0, 0.0, 0.0), f0, sun_visibility)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}
