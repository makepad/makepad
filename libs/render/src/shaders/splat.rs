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
    // in alpha), `normal_map` their normal maps, and `occlusion_map` the
    // tile's weight map (no occlusion: occlusion_strength stays 0). Per
    // layer, splat_l<i> = (UV scale 1/m, roughness, metallic, normal
    // strength).
    mod.draw.DrawSceneSplat = mod.std.set_type_default() do #(DrawSceneSplat::script_shader(vm)){
        ..mod.draw.DrawScenePbr
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
            let wa = self.occlusion_map.sample_lod(vec2(g.x / sz.x, g.y / rows), 0.0)
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
            if self.splat_blend > 0.5 {
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
            // The winner's texel once more (a covering layer's read stays in
            // its branch; the GPU's cache has it).
            var tex = self.splat_texel(l, wp, bw, dx, dy)
            let p = self.splat_param(l)
            let s = p.x
            // Anti-tiling, as DrawScenePbr's triplanar path: with distance a
            // second top-down read at an unrelated scale and offset.
            let dist = length(self.eye.xyz - wp)
            let far = smoothstep(25.0, 140.0, dist)
            if far > 0.001 {
                let s2 = s * 0.173
                let t2 = self.tex.sample_grad_repeat(vec3(wp.x * s2 + 0.31, wp.z * s2 - 0.47, l), vec2(dx.x, dx.z) * s2, vec2(dy.x, dy.z) * s2)
                tex = vec4(mix(tex.xyz, t2.xyz, 0.5 * far * bw.y), tex.w)
            }
            // The brightness drift over ~60 m.
            let drift = self.tn_noise(vec2(wp.x, wp.z) * 0.017) * 0.65 + self.tn_noise(vec2(wp.x, wp.z) * 0.061) * 0.35
            tex = vec4(tex.xyz * (0.86 + 0.28 * drift), tex.w)
            let base = self.to_scene(vec3(tex.x, tex.y, tex.z))
            let surf = self.surface(vec4(self.color_adjust(
                base * self.to_lin(self.v_tint.xyz),
                self.tint,
                self.color_adjust_ctl
            ), 1.0))
            // The winner's normal map, on the plane the mesh UVs used: the
            // ground (normal.y >= 0.6), else the vertical plane it faces most.
            var n = normalize(self.v_csm_n)
            if p.w > 0.00001 {
                var uv = vec2(wp.x, wp.z) * s
                var du1 = vec2(dx.x, dx.z) * s
                var du2 = vec2(dy.x, dy.z) * s
                if n.y < 0.6 {
                    if abs(n.x) > abs(n.z) {
                        let sg = sign(n.x)
                        uv = vec2(wp.z * sg, 0.0 - wp.y) * s
                        du1 = vec2(dx.z * sg, 0.0 - dx.y) * s
                        du2 = vec2(dy.z * sg, 0.0 - dy.y) * s
                    } else {
                        let sg = 0.0 - sign(n.z)
                        uv = vec2(wp.x * sg, 0.0 - wp.y) * s
                        du1 = vec2(dx.x * sg, 0.0 - dx.y) * s
                        du2 = vec2(dy.x * sg, 0.0 - dy.y) * s
                    }
                }
                let determinant = du1.x * du2.y - du1.y * du2.x
                if abs(determinant) > 0.00000001 {
                    let orientation = sign(determinant)
                    let tangent = normalize(dx * du2.y - dy * du1.y) * orientation
                    let bitangent = normalize(dy * du1.x - dx * du2.x) * orientation
                    let xy = self.normal_map.sample_grad_repeat(vec3(uv.x, uv.y, l), du1, du2).xy * 2.0 - vec2(1.0, 1.0)
                    let mapped = vec3(xy.x, xy.y, sqrt(max(1.0 - dot(xy, xy), 0.0)))
                    n = normalize(tangent * (mapped.x * p.w) + bitangent * (mapped.y * p.w) + n * mapped.z)
                }
            }
            n = self.mat_normal(n)
            let mr = self.mat_metal_rough(vec2(clamp(p.z, 0.0, 1.0), clamp(p.y, 0.045, 1.0)))
            return self.shade_mr(surf.xyz, surf.w, mr, n)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}
