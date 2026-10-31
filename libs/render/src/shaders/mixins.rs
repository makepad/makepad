//! Shared scene-shader mixins: fur shells and the colour-adjust law.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // The same root-space pattern follows both static and skinned surfaces.
    // Each extra shell reuses the base mesh; strand count never becomes CPU
    // geometry, skin weights, editable topology or collision primitives.
    mod.draw.SceneFurSurface = {
        v_fur_root: varying(vec3)
        v_fur_normal: varying(vec3)
        fur_hash: fn(p: vec2) -> float {
            return fract(sin(dot(p, vec2(127.1, 311.7)) + self.fur.w * 0.123) * 43758.5453)
        }
        fur_mask: fn() -> float {
            if self.fur.x <= 0.0 || self.fur_layer.x <= 0.0 { return 1.0 }
            let n = abs(self.v_fur_normal)
            var uv = self.v_fur_root.xy
            if n.y >= n.x && n.y >= n.z { uv = self.v_fur_root.xz }
            else if n.x > n.z { uv = self.v_fur_root.yz }
            let p = uv * self.fur.z
            let cell = floor(p)
            let seed = self.fur_hash(cell)
            if seed > self.fur.y { return 0.0 }
            let center = vec2(self.fur_hash(cell + vec2(17.0, 3.0)), self.fur_hash(cell + vec2(7.0, 29.0))) * 0.5 + vec2(0.25, 0.25)
            let radius = 0.43 * (1.0 - self.fur_layer.x * 0.8)
            let delta = fract(p) - center
            return step(dot(delta, delta), radius * radius)
        }
        fur_shade: fn(lit: vec3, n: vec3, view: vec3) -> vec3 {
            if self.fur.x <= 0.0 { return lit }
            let rim = 1.0 - abs(dot(normalize(n), normalize(view)))
            return lit * (0.72 + 0.28 * self.fur_layer.x + 0.18 * rim * rim)
        }
    }

    // One appearance law for every asset family. Hue is a rotation around
    // the neutral-grey axis, so textured/multi-colour assets keep their
    // shading while changing family; saturation and value are optional
    // scalars in the same vec4 lane. Tint is deliberately last.
    mod.draw.SceneColorAdjust = {
        color_adjust: fn(albedo: vec3, tint: vec4, adjust: vec4) -> vec3 {
            let angle = adjust.x * 0.01745329252
            let axis = vec3(0.57735026919, 0.57735026919, 0.57735026919)
            let rotated = albedo * cos(angle)
                + cross(axis, albedo) * sin(angle)
                + axis * dot(axis, albedo) * (1.0 - cos(angle))
            let luma = dot(rotated, vec3(0.2126, 0.7152, 0.0722))
            let saturated = mix(vec3(luma, luma, luma), rotated, max(adjust.y, 0.0))
            return max(saturated * max(adjust.z, 0.0), vec3(0.0, 0.0, 0.0)) * tint.xyz
        }
    }
}
