//! NV12 pictures packed by the GPU for a hardware encoder: an image's luma and chroma drawn into
//! a BGRA8 texture whose bytes ARE the NV12 planes (`makepad_video`'s `nv12_picture` layout):
//! texture rows 0..h four luma samples per texel ([`DrawNv12Luma`], memory order B G R A =
//! samples 2 1 0 3 of the draw's output), rows h..3h/2 U0 V0 U1 V1 per texel, 4:2:0 2x2
//! averages ([`DrawNv12Chroma`]); for 4:4:4 (`full`) w/4 x 3h, each chroma row the U V pairs of
//! every pixel of one source row over two texture rows. The encoder copies the texture's rows into
//! its picture planes on the GPU (no CPU pass). Or (`lanes: 1`) one sample per pixel into the
//! planes of an NV12 texture itself (`Nv12Target`). Colour: BT.709 limited range.

use crate::{makepad_platform::*, shader::draw_quad::DrawQuad};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // Four luma samples of row j, columns 4i..4i+3 of the source rect, in memory order B G R A.
    // `scale` > 1 (source pixels per output pixel, a supersampled image): each sample filters
    // its footprint, four bilinear taps around its centre (2x: the 2x2 box exactly).
    mod.draw.DrawNv12Luma = mod.std.set_type_default() do #(DrawNv12Luma::script_shader(vm)) {
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        source: vec4(0.0, 0.0, 1.0, 1.0)
        extent: vec2(1.0, 1.0)
        scale: vec2(1.0, 1.0)
        lanes: 4.0
        luma_at: fn(c: vec2, d: vec2, inv: vec2) -> float {
            let w = vec3(0.2126, 0.7152, 0.0722)
            var rgb = self.image.sample(c * inv).xyz
            if d.x + d.y > 0.0 {
                rgb = (self.image.sample((c + vec2(-d.x, -d.y)) * inv).xyz + self.image.sample((c + vec2(d.x, -d.y)) * inv).xyz
                    + self.image.sample((c + vec2(-d.x, d.y)) * inv).xyz + self.image.sample((c + vec2(d.x, d.y)) * inv).xyz) * 0.25
            }
            return (16.0 + 219.0 * dot(rgb, w)) / 255.0
        }
        pixel: fn() {
            let t = floor(self.pos * self.rect_size)
            let inv = vec2(1.0 / self.extent.x, 1.0 / self.extent.y)
            let s = self.scale
            // the taps' offset: (s - 1) / 2 up to 2x, s / 4 above
            let d = min((s - vec2(1.0, 1.0)) * 0.5, s * 0.25)
            let row = self.source.y + (t.y + 0.5) * s.y
            // an NV12 texture's luma plane: one sample per pixel
            if self.lanes < 1.5 {
                let y = self.luma_at(vec2(self.source.x + (t.x + 0.5) * s.x, row), d, inv)
                return vec4(y, y, y, y)
            }
            let x = t.x * 4.0 + 0.5
            let y0 = self.luma_at(vec2(self.source.x + x * s.x, row), d, inv)
            let y1 = self.luma_at(vec2(self.source.x + (x + 1.0) * s.x, row), d, inv)
            let y2 = self.luma_at(vec2(self.source.x + (x + 2.0) * s.x, row), d, inv)
            let y3 = self.luma_at(vec2(self.source.x + (x + 3.0) * s.x, row), d, inv)
            return vec4(y2, y1, y0, y3)
        }
    }

    // Two chroma samples, memory order U0 V0 U1 V1: 4:2:0 each the average of a 2x2 block (one
    // bilinear tap at its centre); 4:4:4 (`full`) two neighbouring pixels of one row as they
    // are, a source row over two texture rows (its left half, then its right half). `scale` > 1
    // (a supersampled image): each sample filters its footprint with four bilinear taps.
    mod.draw.DrawNv12Chroma = mod.std.set_type_default() do #(DrawNv12Chroma::script_shader(vm)) {
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        source: vec4(0.0, 0.0, 1.0, 1.0)
        extent: vec2(1.0, 1.0)
        full: 0.0
        scale: vec2(1.0, 1.0)
        lanes: 4.0
        rgb_at: fn(c: vec2, d: vec2, inv: vec2) -> vec3 {
            if d.x + d.y > 0.0 {
                return (self.image.sample((c + vec2(-d.x, -d.y)) * inv).xyz + self.image.sample((c + vec2(d.x, -d.y)) * inv).xyz
                    + self.image.sample((c + vec2(-d.x, d.y)) * inv).xyz + self.image.sample((c + vec2(d.x, d.y)) * inv).xyz) * 0.25
            }
            return self.image.sample(c * inv).xyz
        }
        pixel: fn() {
            let t = floor(self.pos * self.rect_size)
            let inv = vec2(1.0 / self.extent.x, 1.0 / self.extent.y)
            let s = self.scale
            // which half of its source row this texture row holds (Splash shaders have no `mod`)
            let half = t.y - floor(t.y * 0.5) * 2.0
            let full_p = vec2(half * self.rect_size.x * 2.0 + t.x * 2.0 + 0.5, floor(t.y * 0.5) + 0.5)
            let sub_p = vec2(t.x * 4.0 + 1.0, t.y * 2.0 + 1.0)
            let p0 = self.source.xy + mix(sub_p, full_p, self.full) * s
            let step = mix(2.0, 1.0, self.full) * s.x
            // the taps' offset: a 2x2 block's footprint (4:2:0) or a pixel's (4:4:4)
            let sub_d = s * 0.5
            let full_d = min((s - vec2(1.0, 1.0)) * 0.5, s * 0.25)
            var d = mix(sub_d, full_d, self.full)
            if s.x * s.y <= 1.0 {
                d = vec2(0.0, 0.0)
            }
            let c0 = self.rgb_at(p0, d, inv)
            let c1 = self.rgb_at(p0 + vec2(step, 0.0), d, inv)
            let cu = vec3(-0.1146, -0.3854, 0.5)
            let cv = vec3(0.5, -0.4542, -0.0458)
            // an NV12 texture's chroma plane (4:2:0): one U V pair per pixel, its 2x2 block
            if self.lanes < 1.5 {
                let c = self.rgb_at(self.source.xy + vec2(t.x * 2.0 + 1.0, t.y * 2.0 + 1.0) * s, d, inv)
                let u = (128.0 + 224.0 * dot(c, cu)) / 255.0
                let v = (128.0 + 224.0 * dot(c, cv)) / 255.0
                return vec4(u, v, u, v)
            }
            let u0 = (128.0 + 224.0 * dot(c0, cu)) / 255.0
            let v0 = (128.0 + 224.0 * dot(c0, cv)) / 255.0
            let u1 = (128.0 + 224.0 * dot(c1, cu)) / 255.0
            let v1 = (128.0 + 224.0 * dot(c1, cv)) / 255.0
            return vec4(u1, v0, u0, v1)
        }
    }

}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawNv12Luma {
    #[deref]
    pub draw_super: DrawQuad,
    /// The packed rect of the image, in its pixels.
    #[live]
    pub source: Vec4f,
    /// The image's size in pixels.
    #[live]
    pub extent: Vec2f,
    /// Source pixels per output pixel (1: as they are; a supersampled image: filtered down).
    #[live]
    pub scale: Vec2f,
    /// Samples per output texel: 4 (a BGRA8 texture holding the planes' bytes) or 1 (an NV12
    /// texture's luma plane itself).
    #[live]
    pub lanes: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawNv12Chroma {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub source: Vec4f,
    #[live]
    pub extent: Vec2f,
    /// 1: 4:4:4 (every pixel's chroma), 0: 4:2:0.
    #[live]
    pub full: f32,
    /// Source pixels per output pixel (as [`DrawNv12Luma::scale`]).
    #[live]
    pub scale: Vec2f,
    /// As [`DrawNv12Luma::lanes`]: 4 (two U V pairs per BGRA8 texel) or 1 (an NV12 texture's
    /// chroma plane, one pair per pixel; 4:2:0 only).
    #[live]
    pub lanes: f32,
}

