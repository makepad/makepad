//! The display transform for documents (Scene3D, Motion's HDR layers): the
//! host step between the graph's `@hdr` and `@display` stages. It resolves
//! supersampling, applies the exposure, the tone curve and the colour grade,
//! encodes sRGB and premultiplies by coverage, so a transparent background
//! stays transparent.
//!
//! The curves are the standard ones (`none`, `linear`, `reinhard`, `aces`,
//! `agx`, `filmic`), computed as the Scene3D renderer always did; a document
//! may replace the curve with its own Splash function (`curve: fn(c: vec3)
//! -> vec3`, installed on the draw like a material hook), so a look never
//! needs Rust. The Sandbox lane keeps its own composite (composite.rs).
use makepad_draw::*;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.draw.DrawToneMap = mod.std.set_type_default() do #(DrawToneMap::script_shader(vm)){
        ..mod.draw.DrawQuad
        scene_texture: texture_2d(float)

        aces: fn(x: vec3) -> vec3 {
            let a = x * (x * 2.51 + vec3(0.03, 0.03, 0.03))
            let b = x * (x * 2.43 + vec3(0.59, 0.59, 0.59)) + vec3(0.14, 0.14, 0.14)
            return clamp(a / b, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
        }
        filmic: fn(x: vec3) -> vec3 {
            let a = 0.15
            let b = 0.5
            let c = 0.1
            let d = 0.2
            let e = 0.02
            let f = 0.3
            return ((x * (x * a + vec3(c * b, c * b, c * b)) + vec3(d * e, d * e, d * e)) / (x * (x * a + vec3(b, b, b)) + vec3(d * f, d * f, d * f))) - vec3(e / f, e / f, e / f)
        }
        agx_contrast: fn(x: vec3) -> vec3 {
            let x2 = x * x
            let x4 = x2 * x2
            return x4 * x2 * 15.5 - x4 * x * 40.14 + x4 * 31.96 - x2 * x * 6.868 + x2 * 0.4298 + x * 0.1191 - vec3(0.00232, 0.00232, 0.00232)
        }
        agx: fn(c: vec3) -> vec3 {
            let v = vec3(
                c.x * 0.842479 + c.y * 0.078434 + c.z * 0.079224,
                c.x * 0.042328 + c.y * 0.878469 + c.z * 0.079166,
                c.x * 0.042376 + c.y * 0.078434 + c.z * 0.879143
            )
            let lo = -12.47393
            let hi = 4.026069
            let e = (clamp(log2(max(v, vec3(0.0000001, 0.0000001, 0.0000001))), vec3(lo, lo, lo), vec3(hi, hi, hi)) - vec3(lo, lo, lo)) / (hi - lo)
            let s = self.agx_contrast(e)
            let o = vec3(
                s.x * 1.196879 + s.y * -0.098021 + s.z * -0.099029,
                s.x * -0.052897 + s.y * 1.151903 + s.z * -0.098935,
                s.x * -0.052968 + s.y * -0.098043 + s.z * 1.151073
            )
            return pow(max(o, vec3(0.0, 0.0, 0.0)), vec3(2.2, 2.2, 2.2))
        }
        // The tone curve, linear in and out (tone.x picks the standard
        // one). A document's `curve: fn(c: vec3) -> vec3` replaces this.
        // The shoulder (PDOOM R5) for a 2D frame: identity up to 1, so its
        // ordinary colours show exactly as authored; above, the hue is kept
        // (the brightest channel held at 1) and very bright light
        // desaturates toward white.
        shoulder: fn(c: vec3) -> vec3 {
            let x = max(c, vec3(0.0, 0.0, 0.0))
            let m = max(x.x, max(x.y, x.z))
            if m <= 1.0 {
                return x
            }
            let white = clamp((m - 1.0) / 8.0, 0.0, 1.0)
            return mix(x / m, vec3(1.0, 1.0, 1.0), white * white)
        }
        curve: fn(c: vec3) -> vec3 {
            let m = self.tone.x
            if m > 5.5 {
                return self.shoulder(c)
            }
            if m < 1.5 {
                return clamp(c, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
            }
            if m < 2.5 {
                return c / (c + vec3(1.0, 1.0, 1.0))
            }
            if m < 3.5 {
                return self.aces(c * 0.6)
            }
            if m < 4.5 {
                return self.agx(c)
            }
            let w = self.filmic(vec3(11.2, 11.2, 11.2))
            return clamp(self.filmic(c * 2.0) / w, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
        }
        // White balance (temperature, tint), lift/gamma/gain, contrast about
        // mid grey and saturation, on the tone-mapped image (g3.w = on).
        grade_color: fn(c0: vec3) -> vec3 {
            if self.g3.w < 0.5 {
                return c0
            }
            var c = c0 * vec3(1.0 + self.g0.z * 0.1, 1.0 + self.g0.w * 0.1, 1.0 - self.g0.z * 0.1)
            c = max(c * self.g3.xyz + self.g1.xyz * (vec3(1.0, 1.0, 1.0) - c), vec3(0.0, 0.0, 0.0))
            c = pow(c, vec3(1.0, 1.0, 1.0) / max(self.g2.xyz, vec3(0.01, 0.01, 0.01)))
            c = (c - vec3(0.18, 0.18, 0.18)) * self.g0.x + vec3(0.18, 0.18, 0.18)
            let y = dot(c, vec3(0.2126, 0.7152, 0.0722))
            c = mix(vec3(y, y, y), c, self.g0.y)
            return clamp(c, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
        }
        srgb: fn(c: vec3) -> vec3 {
            let lo = c * 12.92
            let hi = pow(max(c, vec3(0.0, 0.0, 0.0)), vec3(0.41666, 0.41666, 0.41666)) * 1.055 - vec3(0.055, 0.055, 0.055)
            return mix(lo, hi, step(vec3(0.0031308, 0.0031308, 0.0031308), c))
        }
        // One scene texel, exposed and tone mapped; the scene is
        // premultiplied by coverage (alpha).
        mapped: fn(uv: vec2) -> vec4 {
            let src = self.scene_texture.sample_nearest(uv)
            let a = clamp(src.w, 0.0, 1.0)
            var c = vec3(0.0, 0.0, 0.0)
            if a > 0.0001 {
                c = src.xyz / a
            }
            return vec4(self.curve(c * self.tone.y), a)
        }
        pixel: fn() {
            // Supersampling: average tone.z x tone.z scene texels after the
            // tone curve, so bright edges resolve smoothly.
            let ss = max(self.tone.z, 1.0)
            var acc = vec4(0.0, 0.0, 0.0, 0.0)
            var j = 0.0
            while j < ss * ss {
                let o = vec2(floor(j / ss), j - floor(j / ss) * ss) - vec2((ss - 1.0) * 0.5, (ss - 1.0) * 0.5)
                let m = self.mapped(self.pos + o * self.texel)
                acc = acc + vec4(m.xyz * m.w, m.w)
                j = j + 1.0
            }
            acc = acc / (ss * ss)
            var c = vec3(0.0, 0.0, 0.0)
            if acc.w > 0.0001 {
                c = acc.xyz / acc.w
            }
            var rgb = self.srgb(self.grade_color(c))
            // Dither against banding, then premultiply.
            let dn = fract(sin(dot(floor(self.pos / self.texel), vec2(41.3, 289.1))) * 13758.5453) - 0.5
            rgb = rgb + vec3(dn, dn, dn) / 255.0
            return vec4(rgb * acc.w, acc.w)
        }
    }
}

/// The standard tone curves, by their `tone.x` code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToneCurve {
    None,
    Linear,
    Reinhard,
    Aces,
    Agx,
    Filmic,
    /// Identity up to 1, then hue-keeping with a roll to white (the HDR
    /// Motion frame's default: its ordinary colours show unchanged).
    Shoulder,
}

impl ToneCurve {
    pub fn code(self) -> f32 {
        match self {
            ToneCurve::None => 0.0,
            ToneCurve::Linear => 1.0,
            ToneCurve::Reinhard => 2.0,
            ToneCurve::Aces => 3.0,
            ToneCurve::Agx => 4.0,
            ToneCurve::Filmic => 5.0,
            ToneCurve::Shoulder => 6.0,
        }
    }
}

/// The colour grade on the tone-mapped image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grade {
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub tint: f32,
    pub lift: [f32; 3],
    pub gamma: [f32; 3],
    pub gain: [f32; 3],
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawToneMap {
    #[deref]
    pub draw_super: DrawQuad,
    /// x = curve code, y = exposure, z = supersampling factor, w unused.
    #[live(vec4(4.0, 1.0, 1.0, 0.0))]
    pub tone: Vec4f,
    /// One scene texel in uv.
    #[live(vec2(0.001, 0.001))]
    pub texel: Vec2f,
    /// Grade: g0 = (contrast, saturation, temperature, tint), g1 = lift,
    /// g2 = gamma, g3 = (gain, on).
    #[live(vec4(1.0, 1.0, 0.0, 0.0))]
    pub g0: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub g1: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 0.0))]
    pub g2: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 0.0))]
    pub g3: Vec4f,
}

impl DrawToneMap {
    /// Set the transform for a frame: `scene_texels` is the scene target's
    /// size (for the supersampling taps and the dither).
    pub fn set(&mut self, curve: ToneCurve, exposure: f32, supersample: u32, scene_texels: (usize, usize), grade: Option<Grade>) {
        self.tone = vec4(curve.code(), if exposure.is_finite() { exposure.max(0.0) } else { 1.0 }, supersample.clamp(1, 4) as f32, 0.0);
        self.texel = vec2(1.0 / scene_texels.0.max(1) as f32, 1.0 / scene_texels.1.max(1) as f32);
        match grade {
            Some(g) => {
                self.g0 = vec4(g.contrast, g.saturation, g.temperature, g.tint);
                self.g1 = vec4(g.lift[0], g.lift[1], g.lift[2], 0.0);
                self.g2 = vec4(g.gamma[0], g.gamma[1], g.gamma[2], 0.0);
                self.g3 = vec4(g.gain[0], g.gain[1], g.gain[2], 1.0);
            }
            None => self.g3.w = 0.0,
        }
    }
}
