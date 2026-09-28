//! DrawSceneTexture: composites the offscreen 3D pass into the host pane.
//!
//! Legacy hosts render the scene in display space into BGRA8 and this is a
//! plain blit. A host that opted into HDR output (`Renderer::set_hdr_output`)
//! renders linear, scene-referred light into RGBA16F; the post chain here is
//! exposure -> AgX tone map (display-encoded) -> FXAA on the tone-mapped
//! image -> dither to the 8-bit swapchain.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    mod.draw.DrawSceneTexture = mod.std.set_type_default() do #(DrawSceneTexture::script_shader(vm)){
        ..mod.draw.DrawQuad,
        scene_texture: texture_2d(float)
        // HDR lane post inputs (post.rs BloomPass): the half-res bloom and
        // the 1x1 adapted mean luminance.
        bloom_texture: texture_2d(float)
        exposure_texture: texture_2d(float)

        // AgX (Sobotka), the minimal polynomial fit (Wrensch): inset into the
        // AgX primaries, log2 encode over [-12.47, 4.03] EV, a sigmoid, then
        // back out. Output is display-encoded (no extra sRGB OETF). A light
        // "punch" restores the contrast and saturation plain AgX gives up.
        agx_curve: fn(x: vec3) -> vec3 {
            let x2 = x * x
            let x4 = x2 * x2
            return x4 * x2 * 15.5 - x4 * x * 40.14 + x4 * 31.96 - x2 * x * 6.868 + x2 * 0.4298 + x * 0.1191 - vec3(0.00232, 0.00232, 0.00232)
        }
        agx: fn(c: vec3) -> vec3 {
            let i = vec3(
                dot(c, vec3(0.842479062253094, 0.0784335999999992, 0.0792237451477643)),
                dot(c, vec3(0.0423282422610123, 0.878468636469772, 0.0791661274605434)),
                dot(c, vec3(0.0423756549057051, 0.0784336, 0.879142973793104))
            )
            let lo = -12.47393
            let hi = 4.026069
            let e = (clamp(log2(max(i, vec3(0.0000001, 0.0000001, 0.0000001))), vec3(lo, lo, lo), vec3(hi, hi, hi)) - vec3(lo, lo, lo)) / (hi - lo)
            let s = self.agx_curve(e)
            let o = vec3(
                dot(s, vec3(1.19687900512017, -0.0980208811401368, -0.0990297440797205)),
                dot(s, vec3(-0.0528968517574562, 1.15190312990417, -0.0989611768448433)),
                dot(s, vec3(-0.0529716355144438, -0.0980434501171241, 1.15107367264116))
            )
            let p = pow(clamp(o, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)), vec3(1.1, 1.1, 1.1))
            let luma = dot(p, vec3(0.2126, 0.7152, 0.0722))
            return clamp(vec3(luma, luma, luma) + (p - vec3(luma, luma, luma)) * 1.35, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
        }
        // One tone-mapped scene texel at exposure `e`, bloom mixed in
        // (energy-conserving: a few percent of the blurred image replaces
        // the same share of the sharp one).
        mapped: fn(uv: vec2, e: float) -> vec3 {
            var c = self.scene_texture.sample(uv).xyz
            if self.post2.x > 0.0 {
                c = mix(c, self.bloom_texture.sample(uv).xyz * self.post2.w, self.post2.x)
            }
            return self.agx(c * e)
        }
        // Perceptual luma of an HDR texel for FXAA's edge search, without
        // paying the full tone map per tap.
        edge_luma: fn(uv: vec2, e: float) -> float {
            let l = dot(self.scene_texture.sample(uv).xyz, vec3(0.2126, 0.7152, 0.0722)) * e
            return sqrt(l / (1.0 + l))
        }
        // The exposure this frame: the host's metered value, or with
        // auto-exposure the adapted scene mean mapped to the key, held
        // within a band around the metered value (night stays night).
        exposure: fn() -> float {
            if self.post.w < 0.5 { return self.post.y }
            let mean = max(self.exposure_texture.sample_nearest(vec2(0.5, 0.5)).x, 0.00001)
            return clamp(self.post2.y / mean, self.post.y * self.post2.z, self.post.y * 1.6)
        }

        pixel: fn() {
            if self.post.x < 0.5 {
                let color = self.scene_texture.sample_as_bgra(self.pos)
                return Pal.premul(color)
            }
            let uv = self.pos
            let ex = self.exposure()
            var c = vec3(0.0, 0.0, 0.0)
            if self.post.z > 0.5 {
                // FXAA (Lottes, the console/lite variant): one directional
                // blur along the local edge, rejected when it overshoots.
                let t = self.texel
                let nw = self.edge_luma(uv + vec2(0.0 - t.x, 0.0 - t.y), ex)
                let ne = self.edge_luma(uv + vec2(t.x, 0.0 - t.y), ex)
                let sw = self.edge_luma(uv + vec2(0.0 - t.x, t.y), ex)
                let se = self.edge_luma(uv + vec2(t.x, t.y), ex)
                let m = self.edge_luma(uv, ex)
                let lmin = min(m, min(min(nw, ne), min(sw, se)))
                let lmax = max(m, max(max(nw, ne), max(sw, se)))
                if lmax - lmin < max(0.0312, lmax * 0.125) {
                    c = self.mapped(uv, ex)
                } else {
                    var dir = vec2(0.0 - ((nw + ne) - (sw + se)), (nw + sw) - (ne + se))
                    let reduce = max((nw + ne + sw + se) * 0.03125, 0.0078125)
                    let rcp = 1.0 / (min(abs(dir.x), abs(dir.y)) + reduce)
                    dir = clamp(dir * rcp, vec2(-8.0, -8.0), vec2(8.0, 8.0)) * t
                    let a = (self.mapped(uv + dir * (1.0 / 3.0 - 0.5), ex) + self.mapped(uv + dir * (2.0 / 3.0 - 0.5), ex)) * 0.5
                    let b = a * 0.5 + (self.mapped(uv - dir * 0.5, ex) + self.mapped(uv + dir * 0.5, ex)) * 0.25
                    let lb = dot(b, vec3(0.299, 0.587, 0.114))
                    c = b
                    if lb < lmin - 0.02 || lb > lmax + 0.02 { c = a }
                }
            } else {
                c = self.mapped(uv, ex)
            }
            // Half-LSB triangular dither in screen space: breaks the 8-bit
            // banding of night skies and fog without drifting in world space.
            let q = floor(uv / self.texel)
            let n1 = fract(sin(dot(q, vec2(12.9898, 78.233))) * 43758.5453)
            let n2 = fract(sin(dot(q, vec2(39.3468, 11.1352))) * 24634.6345)
            let d = (n1 + n2 - 1.0) / 255.0
            return vec4(c + vec3(d, d, d), 1.0)
        }
    }
}
