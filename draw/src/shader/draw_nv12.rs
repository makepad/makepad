//! NV12 pictures on the GPU: [`DrawNv12`] draws a tight NV12 frame (the Y
//! plane, then the interleaved UV rows, in one byte texture made with
//! `Texture::new_nv12`) as RGB, filtered bilinearly between the four
//! converted pixels around each sample, optionally mixed with a second
//! frame of the same size (a frame blend).
//!
//! The conversion is BT.709 limited range in the integer maths of
//! `nv12_to_bgra_u32` (each byte read at its texel centre and recovered
//! exactly, the 2x2 chroma block's nearest sample, the same coefficients,
//! rounding and clamp), so a frame drawn at its own size is bit-exact with
//! the CPU conversion.

use crate::{makepad_platform::*, shader::draw_quad::DrawQuad};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.DrawNv12 = mod.std.set_type_default() do #(DrawNv12::script_shader(vm)){
        ..mod.draw.DrawQuad
        nv12: texture_2d(float)
        // A frame blend's second frame (the same size), mixed in by `blend`.
        nv12_next: texture_2d(float)

        // One converted pixel from its three bytes (0..255).
        nv12_bt709: fn(yb: float, ub: float, vb: float) -> vec3 {
            let c = yb - 16.0
            let d = ub - 128.0
            let e = vb - 128.0
            // `>> 8` of the integer path is a floor division by 256.
            let r = clamp(floor((298.0 * c + 459.0 * e + 128.0) / 256.0), 0.0, 255.0)
            let g = clamp(floor((298.0 * c - 55.0 * d - 136.0 * e + 128.0) / 256.0), 0.0, 255.0)
            let b = clamp(floor((298.0 * c + 541.0 * d + 128.0) / 256.0), 0.0, 255.0)
            return vec3(r, g, b) / 255.0
        }

        // The texel centres (uv) of pixel `p`'s Y byte (xy) and of its 2x2
        // chroma block's U byte (zw); V is one texel right of U.
        nv12_taps: fn(p: vec2) -> vec4 {
            let rows = self.nv12_size.y
            let w = self.nv12_size.x
            let h = floor(rows / 1.5 + 0.5)
            let px = clamp(p.x, 0.0, w - 1.0)
            let py = clamp(p.y, 0.0, h - 1.0)
            let cu = floor(px * 0.5) * 2.0
            let cv = h + floor(py * 0.5)
            return vec4((px + 0.5) / w, (py + 0.5) / rows, (cu + 0.5) / w, (cv + 0.5) / rows)
        }

        nv12_rgb: fn(p: vec2) -> vec3 {
            let t = self.nv12_taps(p)
            let du = 1.0 / self.nv12_size.x
            let yb = floor(self.nv12.sample(t.xy).x * 255.0 + 0.5)
            let ub = floor(self.nv12.sample(t.zw).x * 255.0 + 0.5)
            let vb = floor(self.nv12.sample(vec2(t.z + du, t.w)).x * 255.0 + 0.5)
            return self.nv12_bt709(yb, ub, vb)
        }

        nv12_next_rgb: fn(p: vec2) -> vec3 {
            let t = self.nv12_taps(p)
            let du = 1.0 / self.nv12_size.x
            let yb = floor(self.nv12_next.sample(t.xy).x * 255.0 + 0.5)
            let ub = floor(self.nv12_next.sample(t.zw).x * 255.0 + 0.5)
            let vb = floor(self.nv12_next.sample(vec2(t.z + du, t.w)).x * 255.0 + 0.5)
            return self.nv12_bt709(yb, ub, vb)
        }

        pixel: fn() {
            // The source pixel centre under this output pixel.
            let s = self.src.xy + self.pos * self.src.zw - vec2(0.5, 0.5)
            let f = s - floor(s)
            let p = floor(s)
            let top = mix(self.nv12_rgb(p), self.nv12_rgb(p + vec2(1.0, 0.0)), f.x)
            let bottom = mix(self.nv12_rgb(p + vec2(0.0, 1.0)), self.nv12_rgb(p + vec2(1.0, 1.0)), f.x)
            let rgb = mix(top, bottom, f.y)
            if self.blend > 0.0 {
                let top2 = mix(self.nv12_next_rgb(p), self.nv12_next_rgb(p + vec2(1.0, 0.0)), f.x)
                let bottom2 = mix(self.nv12_next_rgb(p + vec2(0.0, 1.0)), self.nv12_next_rgb(p + vec2(1.0, 1.0)), f.x)
                return vec4(mix(rgb, mix(top2, bottom2, f.y), self.blend), 1.0) * self.opacity
            }
            return vec4(rgb, 1.0) * self.opacity
        }
    }
}

/// An NV12 frame drawn as RGB (see the module). `src` is the source pixel
/// rectangle drawn into the quad (origin, size), `nv12_size` the byte
/// texture's size ([`DrawNv12::set_nv12`] sets both textures and it).
#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawNv12 {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub src: Vec4f,
    #[live]
    pub nv12_size: Vec2f,
    #[live(1.0)]
    pub opacity: f32,
    /// How much of the second frame: 0 = none.
    #[live]
    pub blend: f32,
}

impl DrawNv12 {
    /// The frame `width` x `height` in `texture` (from `Texture::new_nv12`),
    /// and a second frame of that size mixed in by `next.1`. `src` is set to
    /// the whole frame.
    pub fn set_nv12(&mut self, texture: &Texture, width: u32, height: u32, next: Option<(&Texture, f32)>) {
        self.draw_vars.set_texture(0, texture);
        self.draw_vars.set_texture(1, next.map_or(texture, |(next, _)| next));
        self.blend = next.map_or(0.0, |(_, blend)| blend);
        self.nv12_size = vec2(width as f32, Texture::nv12_rows(height) as f32);
        self.src = vec4(0.0, 0.0, width as f32, height as f32);
    }
}
