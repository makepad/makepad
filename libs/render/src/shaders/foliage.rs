//! DrawSceneFoliageLit: the lane swaying trees and bushes draw through.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // A model whose layers sway in the wind (makepadShading.wind) draws
    // here instead of the PBR lane: the same vertex stage (the wind bend
    // lives in DrawSceneSkinned), the same lane binding, and a pixel made
    // for leaves. A tree wall fills a large part of the screen, and leaf
    // cards overdraw several deep, so the per-pixel cost is what foliage
    // costs: no specular lobe, no sky reflection, no normal map, no
    // clustered lights, one cascade tap (csm_vis_fast). What it keeps:
    // albedo x tint, baked AO, a wrapped sun term (a crown is a soft
    // volume, not a hard surface), sunlight through the leaves when the
    // camera looks toward the sun, the hemisphere fill, SSAO and fog.
    // The cut-out test goes through `clip`, so the opaque layers (trunks,
    // crown cores) draw through this lane's no-discard variant (opaque.rs).
    mod.draw.DrawSceneFoliageLit = mod.std.set_type_default() do #(DrawSceneFoliageLit::script_shader(vm)){
        ..mod.draw.DrawScenePbr

        pixel: fn() {
            let tex = self.base_texel()
            if tex.w < 0.5 { self.clip() }
            let wp = self.v_csm.xyz
            let v = normalize(self.eye.xyz - wp)
            var n = normalize(self.v_csm_n)
            // Cards are two-sided: light the side the camera sees.
            if dot(n, v) < 0.0 { n = n * (0.0 - 1.0) }
            let l = normalize(self.light_dir)
            let ndl = dot(n, l)
            // A far stand-in (tex_mag.y < 0) is lit without a shadow lookup:
            // at impostor range a tree's own shading is its silhouette.
            var vis = 1.0
            if self.tex_mag.y > -0.5 { vis = self.csm_vis_fast(wp, n, max(ndl, 0.0)) }
            let albedo = self.to_scene(tex.xyz) * self.to_lin(self.v_tint.xyz)
            let ao = clamp(mix(self.v_tint.w, 1.0, self.surface_on), 0.0, 1.0)
            var sao = 1.0
            if self.ssao_ctl.x > 0.001 {
                let sp = self.v_spos.xy / max(self.v_spos.w, 0.000001)
                sao = 1.0 - (1.0 - self.ssao_map.sample_nearest(vec2(sp.x * 0.5 + 0.5, 0.5 - sp.y * 0.5)).x) * self.ssao_ctl.x
            }
            let wrap = clamp(ndl * 0.7 + 0.3, 0.0, 1.0)
            let back = max(dot(v * (0.0 - 1.0), l), 0.0)
            let through = back * back * back * 0.5 * step(0.5, self.alpha_mode)
            let ambient = mix(self.sun_ground, self.sun_sky, clamp(n.y * 0.5 + 0.5, 0.0, 1.0)) * (ao * sao)
            let lit = albedo * (ambient + self.sun_color * ((wrap + through) * vis * mix(1.0, ao, 0.5)))
                + self.to_scene(self.emissive_map.sample_repeat(self.v_uv).xyz) * self.emissive
            return self.csm_debug_view(vec4(self.scene_fogged(self.to_display(lit), self.v_fog, wp, self.fog_density), 1.0), wp, n)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}
