//! The standard post kits, written in Splash: each is a template that
//! builds ordinary passes (`Pass{}` objects) from the author's parameters.
//! No look is Rust: the graph provides passes, slots, accumulation and
//! formats, and these files are what an AI or an author can read, copy and
//! change at runtime.
//!
//! A host installs them into its document module with
//! [`kit_source`]`(module)`: the host has registered each kit's type (its own
//! marker, see [`KITS`]); the source sets the defaults on those prototypes
//! and defines `kit_<name>` templates. Reading a kit object, the host calls
//! its template with the object and reads the returned list of passes like
//! any `Pass{}`, prefixing the kit's pass names so two kits never collide
//! ([`crate::pass::namespace`]). Every parameter becomes a uniform, so parameters stay keyable
//! (`strength: fn(t) {...}`, tweens) and never change the plan.

/// One kit: the type name documents use, its template's name, the
/// parameters it takes.
pub struct KitDef {
    pub name: &'static str,
    pub kind: &'static str,
    pub params: &'static [&'static str],
}

pub const KITS: &[KitDef] = &[
    KitDef { name: "Glow", kind: "glow", params: &["strength", "radius", "threshold", "tint", "source"] },
    KitDef { name: "Halation", kind: "halation", params: &["strength", "threshold", "tint"] },
    KitDef { name: "Bloom", kind: "bloom", params: &["strength", "threshold", "knee", "radius", "halation", "halation_tint"] },
    KitDef { name: "Shoulder", kind: "shoulder", params: &["knee", "white_from", "white_to", "white"] },
    KitDef { name: "Aberration", kind: "aberration", params: &["amount", "falloff"] },
    KitDef { name: "Grain", kind: "grain", params: &["amount", "size"] },
    KitDef { name: "Vignette", kind: "vignette", params: &["amount", "softness", "color", "mode"] },
    KitDef { name: "FramePost", kind: "frame_post", params: &["shake", "zoom", "flash", "flash_color", "fade", "invert", "mode"] },
    KitDef { name: "Lut", kind: "lut", params: &["image", "amount", "size"] },
    KitDef { name: "DepthOfField", kind: "dof", params: &["focus", "aperture", "max_blur"] },
    KitDef { name: "Outline", kind: "outline", params: &["color", "thickness", "threshold", "mode"] },
    KitDef { name: "Upsample", kind: "upsample", params: &["src", "depth", "guide", "guide_view", "sigma"] },
    KitDef { name: "VelocityBlur", kind: "velocity_blur", params: &["amount", "samples"] },
];

pub fn kit(kind: &str) -> Option<&'static KitDef> {
    KITS.iter().find(|k| k.kind == kind || k.name == kind)
}

/// The kits' Splash source for the host module `module` (for example
/// `mod.m3`), setting defaults on `module.<Name>` and defining
/// `module.kit_<kind>`: all of them, or only the kinds in `only` (a host
/// that has its own node of the same name leaves that kit out).
pub fn kit_source(module: &str, only: Option<&[&str]>) -> String {
    let mut s = String::from("use mod.std.*\nuse mod.math.*\nuse mod.pod.*\n");
    for (kind, src) in SOURCES {
        if only.is_none_or(|o| o.contains(kind)) {
            s.push_str(&src.replace("$M", module));
            s.push('\n');
        }
    }
    s.push_str("true\n");
    s
}

/// Each kit's Splash source (`$M` is the host module).
const SOURCES: &[(&str, &str)] = &[
    ("glow", r##"
// ---------------------------------------------------------------- Glow
// Bloom of what glows: a threshold (a soft knee on the brightest channel)
// of `source` (@color, or
// @glow where the renderer has an emission attachment: then only emissive
// things glow, whatever their brightness), a dual-filter pyramid down to
// 1/32 and back, added to the frame. `radius` 0..1 weights the wide
// levels; `strength` is how much is added.
$M.Glow = $M.Glow{strength: 0.6 radius: 0.6 threshold: 1.0 tint: #ffffff source: @color}
$M.kit_glow = fn(p) {
    let down = "fn() -> vec4 {
        let t = self.texel() * 0.5
        let c = self.src.sample(self.uv()) * 4.0 + self.src.sample(self.uv() + vec2(t.x, t.y)) + self.src.sample(self.uv() - vec2(t.x, t.y)) + self.src.sample(self.uv() + vec2(t.x, 0.0 - t.y)) + self.src.sample(self.uv() - vec2(t.x, 0.0 - t.y))
        return vec4(c.xyz / 8.0, 1.0)
    }"
    let up = "fn() -> vec4 {
        let t = self.texel()
        var c = self.small.sample(self.uv() + vec2(0.0 - t.x * 2.0, 0.0)) + self.small.sample(self.uv() + vec2(t.x * 2.0, 0.0)) + self.small.sample(self.uv() + vec2(0.0, 0.0 - t.y * 2.0)) + self.small.sample(self.uv() + vec2(0.0, t.y * 2.0))
        c = c + (self.small.sample(self.uv() + vec2(0.0 - t.x, t.y)) + self.small.sample(self.uv() + vec2(t.x, t.y)) + self.small.sample(self.uv() + vec2(0.0 - t.x, 0.0 - t.y)) + self.small.sample(self.uv() + vec2(t.x, 0.0 - t.y))) * 2.0
        let r = clamp(self.radius, 0.0, 1.0)
        return vec4(self.same.sample(self.uv()).xyz + c.xyz / 12.0 * (0.35 + 0.65 * r), 1.0)
    }"
    return [
        {at: @hdr name: "pre" reads: [p.source] slots: ["source"] scale: 0.5 uniforms: {threshold: p.threshold}
            pixel: "fn() -> vec4 {
                let t = self.texel() * 0.5
                var c = vec3(0.0, 0.0, 0.0)
                var w = 0.0
                for k in 0..4 {
                    let o = vec2((float(k % 2) * 2.0 - 1.0) * t.x, (float(k / 2) * 2.0 - 1.0) * t.y)
                    let s = max(self.source.sample(self.uv() + o).xyz, vec3(0.0, 0.0, 0.0))
                    let l = self.luma(s)
                    // a soft knee on the brightest channel, from the
                    // threshold to 1.5x it (nothing at or below the
                    // threshold glows, a saturated emissive colour does);
                    // Karis weight so one hot texel cannot flicker as a blob
                    let knee = max(self.threshold, 0.0001)
                    let m = max(s.x, max(s.y, s.z))
                    let over = clamp((m - knee) / (knee * 0.5), 0.0, 1.0)
                    let kw = 1.0 / (1.0 + l)
                    c = c + s * over * over * kw
                    w = w + kw
                }
                return vec4(c / max(w, 0.0001), 1.0)
            }"}
        {at: @hdr name: "d1" reads: ["pre"] slots: ["src"] scale: 0.25 pixel: down}
        {at: @hdr name: "d2" reads: ["d1"] slots: ["src"] scale: 0.125 pixel: down}
        {at: @hdr name: "d3" reads: ["d2"] slots: ["src"] scale: 0.0625 pixel: down}
        {at: @hdr name: "d4" reads: ["d3"] slots: ["src"] scale: 0.03125 pixel: down}
        {at: @hdr name: "u3" reads: ["d4", "d3"] slots: ["small", "same"] scale: 0.0625 uniforms: {radius: p.radius} pixel: up}
        {at: @hdr name: "u2" reads: ["u3", "d2"] slots: ["small", "same"] scale: 0.125 uniforms: {radius: p.radius} pixel: up}
        {at: @hdr name: "u1" reads: ["u2", "d1"] slots: ["small", "same"] scale: 0.25 uniforms: {radius: p.radius} pixel: up}
        {at: @hdr name: "u0" reads: ["u1", "pre"] slots: ["small", "same"] scale: 0.5 uniforms: {radius: p.radius} pixel: up}
        {map: true at: @hdr reads: [@color, "u0"] uniforms: {strength: p.strength tint: p.tint}
            pixel: "fn() -> vec4 {
                let c = self.color.sample(self.uv())
                let g = self.u0.sample(self.uv()).xyz * self.tint.xyz * self.strength
                return vec4(c.xyz + g, c.w)
            }"}
    ]
}
"##),
    ("bloom", r##"
// ---------------------------------------------------------------- Bloom
// A wide physical bloom: everything bright spreads, with a soft knee so
// light a little under the threshold already glows a little. The frame
// at half size (a 4-tap box, clamped to 40, the knee on the brightest
// channel: `threshold` where it starts, `knee` how soft), six 13-tap
// reductions down to 1/128, then back up with a 9-tap tent of radius
// 0.5 + `radius` texels, each level added. The sum goes onto the frame by
// `strength` / 3. `halation`: film halation from the same pyramid (the
// 1/16 level's luma, tinted by `halation_tint`: linear 1, 0.18, 0.04), added too.
$M.Bloom = $M.Bloom{strength: 0.7 threshold: 0.85 knee: 0.5 radius: 0.75 halation: 0 halation_tint: #ff7638}
$M.kit_bloom = fn(p) {
    let down = "fn() -> vec4 {
        let t = self.texel() * 0.5
        let uv = self.uv()
        let a = self.src.sample(uv + vec2(0.0 - 2.0 * t.x, 2.0 * t.y)).xyz
        let b = self.src.sample(uv + vec2(0.0, 2.0 * t.y)).xyz
        let c = self.src.sample(uv + vec2(2.0 * t.x, 2.0 * t.y)).xyz
        let d = self.src.sample(uv + vec2(0.0 - 2.0 * t.x, 0.0)).xyz
        let e = self.src.sample(uv).xyz
        let f = self.src.sample(uv + vec2(2.0 * t.x, 0.0)).xyz
        let g = self.src.sample(uv + vec2(0.0 - 2.0 * t.x, 0.0 - 2.0 * t.y)).xyz
        let h = self.src.sample(uv + vec2(0.0, 0.0 - 2.0 * t.y)).xyz
        let i = self.src.sample(uv + vec2(2.0 * t.x, 0.0 - 2.0 * t.y)).xyz
        let j = self.src.sample(uv + vec2(0.0 - t.x, t.y)).xyz
        let k = self.src.sample(uv + vec2(t.x, t.y)).xyz
        let l = self.src.sample(uv + vec2(0.0 - t.x, 0.0 - t.y)).xyz
        let m = self.src.sample(uv + vec2(t.x, 0.0 - t.y)).xyz
        let o = e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125
        return vec4(o, 1.0)
    }"
    let up = "fn() -> vec4 {
        let t = self.texel() * 0.5 * (0.5 + self.radius)
        let uv = self.uv()
        var c = self.small.sample(uv).xyz * 4.0
        c = c + (self.small.sample(uv + vec2(t.x, 0.0)).xyz + self.small.sample(uv - vec2(t.x, 0.0)).xyz + self.small.sample(uv + vec2(0.0, t.y)).xyz + self.small.sample(uv - vec2(0.0, t.y)).xyz) * 2.0
        c = c + self.small.sample(uv + vec2(t.x, t.y)).xyz + self.small.sample(uv - vec2(t.x, t.y)).xyz + self.small.sample(uv + vec2(t.x, 0.0 - t.y)).xyz + self.small.sample(uv - vec2(t.x, 0.0 - t.y)).xyz
        return vec4(self.same.sample(uv).xyz + c / 16.0, 1.0)
    }"
    return [
        {at: @hdr name: "d0" reads: [@color] slots: ["source"] scale: 0.5 uniforms: {threshold: p.threshold knee: p.knee}
            pixel: "fn() -> vec4 {
                let t = self.texel() * 0.25
                let uv = self.uv()
                var s = self.source.sample(uv + vec2(t.x, t.y)).xyz + self.source.sample(uv - vec2(t.x, t.y)).xyz + self.source.sample(uv + vec2(t.x, 0.0 - t.y)).xyz + self.source.sample(uv - vec2(t.x, 0.0 - t.y)).xyz
                s = clamp(s * 0.25, vec3(0.0, 0.0, 0.0), vec3(40.0, 40.0, 40.0))
                let l = max(s.x, max(s.y, s.z))
                let kn = max(self.knee, 0.0001)
                var rq = clamp(l - self.threshold + kn, 0.0, 2.0 * kn)
                rq = rq * rq / (4.0 * kn)
                let w = max(rq, l - self.threshold) / max(l, 0.0001)
                return vec4(s * w, 1.0)
            }"}
        {at: @hdr name: "d1" reads: ["d0"] slots: ["src"] scale: 0.25 pixel: down}
        {at: @hdr name: "d2" reads: ["d1"] slots: ["src"] scale: 0.125 pixel: down}
        {at: @hdr name: "d3" reads: ["d2"] slots: ["src"] scale: 0.0625 pixel: down}
        {at: @hdr name: "d4" reads: ["d3"] slots: ["src"] scale: 0.03125 pixel: down}
        {at: @hdr name: "d5" reads: ["d4"] slots: ["src"] scale: 0.015625 pixel: down}
        {at: @hdr name: "d6" reads: ["d5"] slots: ["src"] scale: 0.0078125 pixel: down}
        {at: @hdr name: "u5" reads: ["d6", "d5"] slots: ["small", "same"] scale: 0.015625 uniforms: {radius: p.radius} pixel: up}
        {at: @hdr name: "u4" reads: ["u5", "d4"] slots: ["small", "same"] scale: 0.03125 uniforms: {radius: p.radius} pixel: up}
        {at: @hdr name: "u3" reads: ["u4", "d3"] slots: ["small", "same"] scale: 0.0625 uniforms: {radius: p.radius} pixel: up}
        {at: @hdr name: "u2" reads: ["u3", "d2"] slots: ["small", "same"] scale: 0.125 uniforms: {radius: p.radius} pixel: up}
        {at: @hdr name: "u1" reads: ["u2", "d1"] slots: ["small", "same"] scale: 0.25 uniforms: {radius: p.radius} pixel: up}
        {at: @hdr name: "u0" reads: ["u1", "d0"] slots: ["small", "same"] scale: 0.5 uniforms: {radius: p.radius} pixel: up}
        {map: true at: @hdr reads: [@color, "u0", "u3"] slots: ["color", "bloom", "wide"] uniforms: {strength: p.strength halation: p.halation tint: p.halation_tint}
            pixel: "fn() -> vec4 {
                let c = self.color.sample(self.uv())
                let b = self.bloom.sample(self.uv()).xyz * (self.strength / 3.0)
                let h = self.tint.xyz * (self.luma(self.wide.sample(self.uv()).xyz) * self.halation)
                return vec4(c.xyz + b + h, c.w)
            }"}
    ]
}
"##),
    ("shoulder", r##"
// ---------------------------------------------------------------- Shoulder
// A film shoulder in linear light, before the display transform: each
// channel is itself up to `knee`, then rolls off toward 1; very hot light
// (the brightest channel from `white_from` to `white_to`) goes toward
// white by up to `white`, so bright cores burn white-hot while their halo
// keeps its colour. Its output stays within 0..1, which the display
// transform shows unchanged. Put it after Bloom and before a linear
// Vignette.
$M.Shoulder = $M.Shoulder{knee: 0.72 white_from: 2 white_to: 12 white: 0.85}
$M.kit_shoulder = fn(p) {
    return [
        {map: true at: @hdr reads: [@color] uniforms: {knee: p.knee white_from: p.white_from white_to: p.white_to white: p.white}
            pixel: "fn() -> vec4 {
                let c = self.color.sample(self.uv())
                let a = clamp(c.w, 0.0, 1.0)
                if a < 0.0001 {
                    return c
                }
                let x = max(c.xyz / a, vec3(0.0, 0.0, 0.0))
                let k = clamp(self.knee, 0.0, 0.999)
                let r = vec3(k, k, k) + (1.0 - k) * (vec3(1.0, 1.0, 1.0) - exp((vec3(k, k, k) - x) / (1.0 - k)))
                var y = mix(x, r, step(vec3(k, k, k), x))
                let m = max(x.x, max(x.y, x.z))
                y = mix(y, vec3(1.0, 1.0, 1.0), smoothstep(self.white_from, self.white_to, m) * self.white)
                return vec4(y * a, c.w)
            }"}
    ]
}
"##),
    ("halation", r##"
// ---------------------------------------------------------------- Halation
// Film halation: light scattered back through the film base glows red-
// orange around bright edges. A tinted luma of a mid pyramid level (1/8)
// of what is above `threshold`, added.
$M.Halation = $M.Halation{strength: 0.35 threshold: 0.8 tint: #ff3a12}
$M.kit_halation = fn(p) {
    return [
        {at: @hdr name: "pre" reads: [@color] scale: 0.5 uniforms: {threshold: p.threshold}
            pixel: "fn() -> vec4 {
                let l = self.luma(max(self.color.sample(self.uv()).xyz, vec3(0.0, 0.0, 0.0)))
                let h = max(l - self.threshold, 0.0) / (1.0 + l)
                return vec4(h, h, h, 1.0)
            }"}
        {at: @hdr name: "d1" reads: ["pre"] slots: ["src"] scale: 0.25
            pixel: "fn() -> vec4 {
                let t = self.texel()
                let c = self.src.sample(self.uv() + vec2(t.x, t.y) * 0.5) + self.src.sample(self.uv() - vec2(t.x, t.y) * 0.5) + self.src.sample(self.uv() + vec2(t.x, 0.0 - t.y) * 0.5) + self.src.sample(self.uv() - vec2(t.x, 0.0 - t.y) * 0.5)
                return vec4(c.xyz * 0.25, 1.0)
            }"}
        {at: @hdr name: "d2" reads: ["d1"] slots: ["src"] scale: 0.125
            pixel: "fn() -> vec4 {
                let t = self.texel()
                var c = vec3(0.0, 0.0, 0.0)
                for k in 0..9 {
                    let o = vec2(float(k % 3) - 1.0, float(k / 3) - 1.0)
                    c = c + self.src.sample(self.uv() + o * t).xyz * (2.0 - abs(o.x)) * (2.0 - abs(o.y))
                }
                return vec4(c / 16.0, 1.0)
            }"}
        {map: true at: @hdr reads: [@color, "d2", "d1"] uniforms: {strength: p.strength tint: p.tint}
            pixel: "fn() -> vec4 {
                let c = self.color.sample(self.uv())
                let h = (self.d2.sample(self.uv()).x * 0.7 + self.d1.sample(self.uv()).x * 0.3) * self.strength
                return vec4(c.xyz + self.tint.xyz * h, c.w)
            }"}
    ]
}
"##),
    ("aberration", r##"
// ---------------------------------------------------------------- Aberration
// Lateral chromatic aberration: red and blue pulled apart toward the frame's
// edges. `falloff` 1: linear from the centre, `amount` pixels (at 1080p) at
// the corners; 2: quadratic (a lens: nothing near the centre), the offset
// dc * |dc * (aspect, 1)|² * amount * 4 / width for dc = uv - 0.5.
$M.Aberration = $M.Aberration{amount: 2.0 falloff: 1}
$M.kit_aberration = fn(p) {
    return [
        {at: @hdr reads: [@color] uniforms: {amount: p.amount falloff: p.falloff}
            pixel: "fn() -> vec4 {
                let dc = self.uv() - vec2(0.5, 0.5)
                var d = dc * vec2(1.0 / self.aspect(), 1.0) * (self.amount / 1080.0)
                if self.falloff > 1.5 {
                    let q = dc * vec2(self.aspect(), 1.0)
                    d = dc * dot(q, q) * self.amount * 4.0 / self.size().x
                }
                let c = self.color.sample(self.uv())
                return vec4(self.color.sample(self.uv() + d).x, c.y, self.color.sample(self.uv() - d).z, c.w)
            }"}
    ]
}
"##),
    ("grain", r##"
// ---------------------------------------------------------------- Grain
// Film grain after the tone map, on the display-encoded picture: two
// scales of noise (grains of `size` pixels at 1080p, and twice that),
// strongest in the mid-tones and still present in the blacks, a new
// pattern every frame and the same one across the frame's shutter; then a
// dither of one 8-bit step.
$M.Grain = $M.Grain{amount: 0.055 size: 1}
$M.kit_grain = fn(p) {
    return [
        {map: true at: @final reads: [@color] uniforms: {amount: p.amount grain: p.size}
            pixel: "fn() -> vec4 {
                let c = self.color.sample(self.uv())
                let px = floor(self.uv() * self.size())
                let cell = floor(px / max(self.grain * self.size().y / 1080.0, 1.0))
                let g1 = self.hash(cell + vec2(self.frame_hash(1.0), self.frame_hash(2.0)) * 1000.0) - 0.5
                let g2 = self.hash(floor(cell / 2.0) + vec2(self.frame_hash(3.0), self.frame_hash(4.0)) * 1000.0) - 0.5
                let l = clamp(self.luma(c.xyz), 0.0, 1.0)
                let k = self.amount * (0.55 + 1.2 * l * (1.0 - l))
                let dn = (self.hash(px * 1.37 + vec2(self.frame_hash(5.0), self.frame_hash(6.0)) * 100.0) - 0.5) / 255.0
                let n = (0.6 * g1 + 0.4 * g2) * k + dn
                return vec4(clamp(c.xyz + vec3(n, n, n), vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)), c.w)
            }"}
    ]
}
"##),
    ("vignette", r##"
// ---------------------------------------------------------------- Vignette
// `mode` 0: toward `color` on the display picture, from 1 - `softness` of
// the half diagonal out; 1: a lens falloff multiplied in linear light
// (after a Shoulder, before the display transform): darkest by `amount` at
// the corners, from 0.25 to 0.95 of an oval (uv - 0.5) * (1, 0.8).
$M.Vignette = $M.Vignette{amount: 0.35 softness: 0.55 color: #000000 mode: 0}
$M.kit_vignette = fn(p) {
    if p.mode == 1 {
        return [
            {map: true at: @hdr reads: [@color] uniforms: {amount: p.amount}
                pixel: "fn() -> vec4 {
                    let c = self.color.sample(self.uv())
                    let dc = (self.uv() - vec2(0.5, 0.5)) * vec2(1.0, 0.8)
                    let v = mix(1.0, smoothstep(0.95, 0.25, length(dc)), self.amount)
                    return vec4(c.xyz * v, c.w)
                }"}
        ]
    }
    return [
        {map: true at: @final reads: [@color] uniforms: {amount: p.amount softness: p.softness tone: p.color}
            pixel: "fn() -> vec4 {
                let c = self.color.sample(self.uv())
                let q = (self.uv() - vec2(0.5, 0.5)) * vec2(self.aspect(), 1.0)
                let r = length(q) / length(vec2(self.aspect() * 0.5, 0.5))
                let v = smoothstep(1.0 - self.softness, 1.0 + 0.001, r) * self.amount
                return vec4(mix(c.xyz, self.tone.xyz, v), c.w)
            }"}
    ]
}
"##),
    ("frame_post", r##"
// ---------------------------------------------------------------- FramePost
// The frame-point effects (read once per frame, at 1/8 of the shutter after
// the frame time, however many sub-frames the frame has, like every post
// parameter): shake (fraction
// of the frame height), zoom (1 = none), a flash toward `flash_color`, a
// fade to black and an invert, each 0..1. `mode` 0: on the display
// picture, at the end; 1: in linear light at @hdr, where it stands in the
// post list (shake and zoom first, flash and fade after a Shoulder and a
// linear Vignette: two FramePosts).
$M.FramePost = $M.FramePost{shake: 0 zoom: 1 flash: 0 flash_color: #ffffff fade: 0 invert: 0 mode: 0}
$M.kit_frame_post = fn(p) {
    return [
        {at: if p.mode == 1 { @hdr } else { @final } reads: [@color]
            uniforms: {shake: p.shake zoom: p.zoom flash: p.flash flash_color: p.flash_color fade: p.fade invert: p.invert}
            pixel: "fn() -> vec4 {
                let j = vec2(self.frame_hash(3.0) - 0.5, self.frame_hash(4.0) - 0.5) * 2.0 * self.shake * vec2(1.0 / self.aspect(), 1.0)
                let uv = (self.uv() - vec2(0.5, 0.5)) / max(self.zoom, 0.01) + vec2(0.5, 0.5) + j
                var c = self.color.sample(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
                c = vec4(mix(c.xyz, vec3(1.0, 1.0, 1.0) - c.xyz, clamp(self.invert, 0.0, 1.0)), c.w)
                c = vec4(mix(c.xyz, self.flash_color.xyz, clamp(self.flash, 0.0, 1.0)), c.w)
                return vec4(c.xyz * (1.0 - clamp(self.fade, 0.0, 1.0)), c.w)
            }"}
    ]
}
"##),
    ("lut", r##"
// ---------------------------------------------------------------- Lut
// A colour lookup from a strip image named by `image` (the host provides it:
// `size` x size² pixels, blue across the tiles), mixed in by `amount`.
$M.Lut = $M.Lut{image: "lut" amount: 1 size: 16}
$M.kit_lut = fn(p) {
    return [
        {map: true at: @display reads: [@color, p.image] slots: ["color", "lut"] uniforms: {amount: p.amount cells: p.size}
            pixel: "fn() -> vec4 {
                let c = self.color.sample(self.uv())
                let n = max(self.cells, 2.0)
                let x = clamp(c.xyz, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)) * (n - 1.0)
                let b0 = floor(x.z)
                let b1 = min(b0 + 1.0, n - 1.0)
                let px = vec2(x.x + 0.5, x.y + 0.5) / vec2(n * n, n)
                let a = self.lut.sample(px + vec2(b0 / n, 0.0)).xyz
                let b = self.lut.sample(px + vec2(b1 / n, 0.0)).xyz
                return vec4(mix(c.xyz, mix(a, b, x.z - b0), clamp(self.amount, 0.0, 1.0)), c.w)
            }"}
    ]
}
"##),
    ("dof", r##"
// ---------------------------------------------------------------- DepthOfField
// A thin lens: circle of confusion from the view distance against `focus`
// (metres), `aperture` its size (pixels at 1080p for something at
// infinity), capped at `max_blur`; a gathered disc at half resolution.
$M.DepthOfField = $M.DepthOfField{focus: 5 aperture: 8 max_blur: 16}
$M.kit_dof = fn(p) {
    return [
        {at: @hdr name: "coc" reads: [@color, @depth] scale: 0.5 uniforms: {focus: p.focus aperture: p.aperture max_blur: p.max_blur}
            pixel: "fn() -> vec4 {
                let z = self.view_depth(self.depth.sample_nearest(self.uv()).x)
                let coc = min(abs(1.0 - self.focus / max(z, 0.001)) * self.aperture, self.max_blur) * self.size().y / 540.0
                return vec4(self.color.sample(self.uv()).xyz, coc)
            }"}
        {at: @hdr name: "blur" reads: ["coc"] scale: 0.5
            pixel: "fn() -> vec4 {
                let centre = self.coc.sample(self.uv())
                var sum = centre.xyz
                var w = 1.0
                for k in 0..24 {
                    let a = float(k) * 2.39996
                    let r = sqrt((float(k) + 0.5) / 24.0)
                    let o = vec2(cos(a), sin(a)) * r * centre.w * self.texel()
                    let s = self.coc.sample(self.uv() + o)
                    // a sample only spreads as far as its own blur
                    let sw = clamp(s.w / max(centre.w * r, 0.001), 0.0, 1.0)
                    sum = sum + s.xyz * sw
                    w = w + sw
                }
                return vec4(sum / w, centre.w)
            }"}
        {map: true at: @hdr reads: [@color, "blur"]
            pixel: "fn() -> vec4 {
                let c = self.color.sample(self.uv())
                let b = self.blur.sample(self.uv())
                return vec4(mix(c.xyz, b.xyz, clamp(b.w / 2.0, 0.0, 1.0)), c.w)
            }"}
    ]
}
"##),
    ("outline", r##"
// ---------------------------------------------------------------- Outline
// Ink edges where depth jumps (the re-draw-free fallback; items flagged
// for the id attachment get exact outlines where the renderer has it):
// `thickness` pixels at 1080p, `threshold` the relative depth step.
// `mode` 1: from the renderer's @id map (surface ids, x the low byte and
// y the high byte, 0 none): where the id changes within the thickness,
// around each outlined surface and between two of them (on the lower
// id's side, once).
$M.Outline = $M.Outline{color: #000000 thickness: 1.5 threshold: 0.08 mode: 0}
$M.kit_outline = fn(p) {
    if p.mode == 1 {
        return [
            {map: true at: @display reads: [@color, @id] slots: ["color", "ids"] uniforms: {ink: p.color thickness: p.thickness}
                helpers: "surface: fn(uv: vec2) -> float {
                    let c = self.ids.sample_nearest(uv)
                    return floor(c.y * 255.0 + 0.5) * 256.0 + floor(c.x * 255.0 + 0.5)
                }"
                pixel: "fn() -> vec4 {
                    let c = self.color.sample(self.uv())
                    let r = max(self.thickness * self.size().y / 1080.0, self.px_scale())
                    let own = self.surface(self.uv())
                    var hit = 0.0
                    for k in 0..16 {
                        let a = float(k) * 0.3926991
                        let q = self.surface(self.uv() + vec2(cos(a), sin(a)) * r * self.texel())
                        if q > 0.5 && q > own + 0.5 {
                            hit = 1.0
                        }
                    }
                    // Mixed in linear light; the ink raises coverage, so
                    // an outline over nothing (an alpha export) shows.
                    let e = hit * self.ink.w
                    let rgb = self.to_srgb(self.from_srgb(c.xyz) * (1.0 - e) + self.ink.xyz * e)
                    return vec4(rgb, c.w * (1.0 - e) + e)
                }"}
        ]
    }
    return [
        {map: true at: @display reads: [@color, @depth] uniforms: {ink: p.color thickness: p.thickness threshold: p.threshold}
            pixel: "fn() -> vec4 {
                let c = self.color.sample(self.uv())
                let t = self.texel() * max(self.thickness * self.size().y / 1080.0, 0.5)
                let z = self.view_depth(self.depth.sample_nearest(self.uv()).x)
                let zx0 = self.view_depth(self.depth.sample_nearest(self.uv() + vec2(t.x, 0.0)).x)
                let zx1 = self.view_depth(self.depth.sample_nearest(self.uv() - vec2(t.x, 0.0)).x)
                let zy0 = self.view_depth(self.depth.sample_nearest(self.uv() + vec2(0.0, t.y)).x)
                let zy1 = self.view_depth(self.depth.sample_nearest(self.uv() - vec2(0.0, t.y)).x)
                let e = max(max(abs(zx0 - z) / max(min(zx0, z), 0.001), abs(zx1 - z) / max(min(zx1, z), 0.001)), max(abs(zy0 - z) / max(min(zy0, z), 0.001), abs(zy1 - z) / max(min(zy1, z), 0.001)))
                let a = smoothstep(self.threshold, self.threshold * 1.5, e) * self.ink.w
                return vec4(mix(c.xyz, self.ink.xyz, a), c.w)
            }"}
    ]
}
"##),
    ("upsample", r##"
// ---------------------------------------------------------------- Upsample
// A depth-aware upsample of a reduced-resolution pass (a raymarch at
// `scale: 0.5` with a view-distance output) over the frame: each full-size
// pixel takes the four nearest low-resolution texels weighted by how close
// their depth is to its own (so an edge stays sharp instead of bleeding),
// then goes over @color (premultiplied), except where the guide (the
// scene) is nearer than the marched surface. `src`: the pass; `depth`: its
// view-distance output; `guide`: the full-size depth (@depth, the scene's
// depth buffer, or a pass writing view distance with `guide_view: 1`);
// `sigma`: the relative depth step that separates two surfaces.
$M.Upsample = $M.Upsample{src: "march" depth: "march_depth" guide: @depth guide_view: 0 sigma: 0.05}
$M.kit_upsample = fn(p) {
    return [
        {at: @hdr reads: [@color, p.src, p.depth, p.guide] slots: ["color", "lo", "lo_depth", "guide"] uniforms: {sigma: p.sigma guide_view: p.guide_view}
            pixel: "fn() -> vec4 {
                let g = self.guide.sample_nearest(self.uv()).x
                var z = g
                if self.guide_view < 0.5 {
                    z = self.view_depth(g)
                }
                let ts = vec2(1.0, 1.0) / max(self.lo.size(), vec2(1.0, 1.0))
                let base = (floor(self.uv() / ts - vec2(0.5, 0.5)) + vec2(0.5, 0.5)) * ts
                var sum = vec4(0.0, 0.0, 0.0, 0.0)
                var w = 0.0
                for k in 0..4 {
                    let at = base + vec2(float(k % 2), float(k / 2)) * ts
                    let zl = self.lo_depth.sample_nearest(at).x
                    let bx = 1.0 - clamp(abs(self.uv().x - at.x) / ts.x, 0.0, 1.0)
                    let by = 1.0 - clamp(abs(self.uv().y - at.y) / ts.y, 0.0, 1.0)
                    let dw = exp(0.0 - abs(zl - z) / max(z * self.sigma, 0.0001))
                    let ww = max(bx * by, 0.0001) * dw
                    sum = sum + self.lo.sample_nearest(at) * ww
                    w = w + ww
                }
                var up = self.lo.sample(self.uv())
                if w > 0.00001 {
                    up = sum / w
                }
                let c = self.color.sample(self.uv())
                // Depth-tested against the guide: where the scene is nearer
                // than the marched surface, the scene shows.
                let zm = self.lo_depth.sample_nearest(self.uv()).x
                if zm > 0.0 && z < zm * (1.0 - self.sigma) {
                    return c
                }
                return up + c * (1.0 - clamp(up.w, 0.0, 1.0))
            }"}
    ]
}
"##),
    ("velocity_blur", r##"
// ---------------------------------------------------------------- VelocityBlur
// Realtime motion blur from the camera's motion: each pixel's world point
// (from @depth) projected with last frame's camera gives its screen motion,
// and the colour is averaged along it over the open shutter (`amount`: the
// shutter's fraction of a frame, 0.5 for 180 degrees). The camera's motion
// only (an object moving on its own is not smeared); locked time sums
// sub-frames instead. Needs the host's camera and @depth.
$M.VelocityBlur = $M.VelocityBlur{amount: 0.5 samples: 12}
$M.kit_velocity_blur = fn(p) {
    return [
        {at: @hdr reads: [@color, @depth] uniforms: {amount: p.amount taps: p.samples}
            pixel: "fn() -> vec4 {
                let uv = self.uv()
                // The pixel's motion, or a neighbour's when that is longer
                // (a moving edge smears over what is behind it: the motion
                // is dilated over 8 px).
                let r = self.texel() * 8.0
                var v = vec2(0.0, 0.0)
                for k in 0..9 {
                    let o = vec2(float(k % 3) - 1.0, float(k / 3) - 1.0) * r
                    let q = uv + o
                    let w = self.world_at(q, self.depth.sample_nearest(q).x)
                    let vk = (q - self.prev_uv(w)) * self.amount
                    if dot(vk, vk) > dot(v, v) {
                        v = vk
                    }
                }
                // At most a tenth of the frame per shutter (a cut is not a
                // smear).
                let l = length(v)
                if l > 0.1 {
                    v = v * (0.1 / l)
                }
                let n = clamp(self.taps, 2.0, 32.0)
                var sum = vec4(0.0, 0.0, 0.0, 0.0)
                for k in 0..32 {
                    if float(k) < n {
                        let f = (float(k) + 0.5) / n - 0.5
                        sum = sum + self.color.sample(uv - v * f)
                    }
                }
                return sum / n
            }"}
    ]
}
"##),
];
