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
    KitDef { name: "Aberration", kind: "aberration", params: &["amount"] },
    KitDef { name: "Grain", kind: "grain", params: &["amount", "size"] },
    KitDef { name: "Vignette", kind: "vignette", params: &["amount", "softness", "color"] },
    KitDef { name: "FramePost", kind: "frame_post", params: &["shake", "zoom", "flash", "flash_color", "fade", "invert"] },
    KitDef { name: "Lut", kind: "lut", params: &["image", "amount", "size"] },
    KitDef { name: "DepthOfField", kind: "dof", params: &["focus", "aperture", "max_blur"] },
    KitDef { name: "Outline", kind: "outline", params: &["color", "thickness", "threshold"] },
    KitDef { name: "Upsample", kind: "upsample", params: &["src", "depth", "guide", "guide_view", "sigma"] },
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
        {at: @hdr reads: [@color, "u0"] uniforms: {strength: p.strength tint: p.tint}
            pixel: "fn() -> vec4 {
                let c = self.color.sample(self.uv())
                let g = self.u0.sample(self.uv()).xyz * self.tint.xyz * self.strength
                return vec4(c.xyz + g, c.w)
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
        {at: @hdr reads: [@color, "d2", "d1"] uniforms: {strength: p.strength tint: p.tint}
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
// edges, `amount` pixels (at 1080p) at the corners.
$M.Aberration = $M.Aberration{amount: 2.0}
$M.kit_aberration = fn(p) {
    return [
        {at: @hdr reads: [@color] uniforms: {amount: p.amount}
            pixel: "fn() -> vec4 {
                let d = (self.uv() - vec2(0.5, 0.5)) * vec2(1.0 / self.aspect(), 1.0) * (self.amount / 1080.0)
                let c = self.color.sample(self.uv())
                return vec4(self.color.sample(self.uv() + d).x, c.y, self.color.sample(self.uv() - d).z, c.w)
            }"}
    ]
}
"##),
    ("grain", r##"
// ---------------------------------------------------------------- Grain
// Film grain after the tone map: luminance-weighted noise in grains of
// `size` pixels (at 1080p), a new pattern every frame and the same one
// across the frame's shutter.
$M.Grain = $M.Grain{amount: 0.05 size: 1.4}
$M.kit_grain = fn(p) {
    return [
        {at: @final reads: [@color] uniforms: {amount: p.amount grain: p.size}
            pixel: "fn() -> vec4 {
                let c = self.color.sample(self.uv())
                let cell = floor(self.uv() * self.size() / max(self.grain * self.size().y / 1080.0, 1.0))
                let n = self.hash(cell + vec2(self.frame_hash(1.0) * 413.0, self.frame_hash(2.0) * 211.0)) - 0.5
                let l = self.luma(c.xyz)
                let k = self.amount * (1.0 - 0.6 * l) * 2.0
                return vec4(clamp(c.xyz + vec3(n, n, n) * k, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)), c.w)
            }"}
    ]
}
"##),
    ("vignette", r##"
// ---------------------------------------------------------------- Vignette
$M.Vignette = $M.Vignette{amount: 0.35 softness: 0.55 color: #000000}
$M.kit_vignette = fn(p) {
    return [
        {at: @final reads: [@color] uniforms: {amount: p.amount softness: p.softness tone: p.color}
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
// fade to black and an invert, each 0..1.
$M.FramePost = $M.FramePost{shake: 0 zoom: 1 flash: 0 flash_color: #ffffff fade: 0 invert: 0}
$M.kit_frame_post = fn(p) {
    return [
        {at: @final reads: [@color]
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
        {at: @display reads: [@color, p.image] slots: ["color", "lut"] uniforms: {amount: p.amount cells: p.size}
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
        {at: @hdr reads: [@color, "blur"]
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
$M.Outline = $M.Outline{color: #000000 thickness: 1.5 threshold: 0.08}
$M.kit_outline = fn(p) {
    return [
        {at: @display reads: [@color, @depth] uniforms: {ink: p.color thickness: p.thickness threshold: p.threshold}
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
];
